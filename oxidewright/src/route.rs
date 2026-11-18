//! Request routing: [`Route`], `page.route` / `context.route` and friends
//! (Playwright's `client/network.ts` `Route`/`RouteHandler` and the server's
//! `network.ts` `Route` merged together).

use std::collections::HashMap;
use std::fmt;
use std::future::{Future, IntoFuture};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use base64::Engine;
use chromiumoxide::cdp::browser_protocol::fetch::{
    ContinueRequestParams, ContinueResponseParams, FailRequestParams, FulfillRequestParams,
    GetResponseBodyParams as FetchGetResponseBodyParams,
};
use chromiumoxide::cdp::browser_protocol::network::ErrorReason;
use chromiumoxide::cdp::browser_protocol::target::SessionId;
use serde::Serialize;
use tokio::sync::{oneshot, watch};

use crate::browser::BrowserInner;
use crate::browser_context::BrowserContext;
use crate::error::{Error, Result, ResultExt};
use crate::network::{
    Header, Request, apply_headers_overrides, header_get, headers_to_map, mime_type_for_path,
    status_text,
};
use crate::network_manager::{NetworkManager, header_entries};
use crate::page::{Page, PageInner};
use crate::url_match::{UrlMatch, resolve_glob_to_regex_pattern};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// The boxed form of a route handler.
pub type RouteHandlerFn = Arc<dyn Fn(Route) -> BoxFuture<Result<()>> + Send + Sync>;

/// State of an intercepted request shared by the request (and its
/// redirects) and the [`Route`] (Playwright's `RouteImpl`).
pub(crate) struct RouteShared {
    session: SessionId,
    interception_id: Mutex<String>,
    /// Set once `route.fetch()` moved the request to the response stage.
    response_stage: AtomicBool,
    pub(crate) already_continued: Mutex<Option<ContinueRequestParams>>,
    handled: AtomicBool,
}

impl RouteShared {
    pub(crate) fn new(session: SessionId, interception_id: String) -> Self {
        RouteShared {
            session,
            interception_id: Mutex::new(interception_id),
            response_stage: AtomicBool::new(false),
            already_continued: Mutex::new(None),
            handled: AtomicBool::new(false),
        }
    }

    pub(crate) fn interception_id(&self) -> String {
        self.interception_id.lock().unwrap().clone()
    }

    async fn send<C: chromiumoxide::types::Command>(
        &self,
        browser: &Weak<BrowserInner>,
        cmd: C,
    ) -> Result<C::Response> {
        let browser = browser.upgrade().ok_or_else(Error::target_closed)?;
        browser
            .execute_with_session(cmd, Some(self.session.clone()))
            .await
    }

    pub(crate) async fn abort_raw(&self, browser: &Weak<BrowserInner>, code: &str) -> Result<()> {
        let reason = error_reason(code)?;
        let params = FailRequestParams::new(self.interception_id(), reason);
        catch_disallowed(self.send(browser, params).await.map(|_| ()))
    }
}

/// `catchDisallowedErrors`: ignore errors about requests that are already
/// gone, report invalid input.
fn catch_disallowed(result: Result<()>) -> Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(err) => {
            let message = err.to_string();
            if message.contains("Invalid http status code or phrase")
                || message.contains("Unsafe header")
            {
                Err(err)
            } else {
                tracing::debug!("ignored route error: {message}");
                Ok(())
            }
        }
    }
}

fn error_reason(code: &str) -> Result<ErrorReason> {
    Ok(match code {
        "aborted" => ErrorReason::Aborted,
        "accessdenied" => ErrorReason::AccessDenied,
        "addressunreachable" => ErrorReason::AddressUnreachable,
        "blockedbyclient" => ErrorReason::BlockedByClient,
        "blockedbyresponse" => ErrorReason::BlockedByResponse,
        "connectionaborted" => ErrorReason::ConnectionAborted,
        "connectionclosed" => ErrorReason::ConnectionClosed,
        "connectionfailed" => ErrorReason::ConnectionFailed,
        "connectionrefused" => ErrorReason::ConnectionRefused,
        "connectionreset" => ErrorReason::ConnectionReset,
        "internetdisconnected" => ErrorReason::InternetDisconnected,
        "namenotresolved" => ErrorReason::NameNotResolved,
        "timedout" => ErrorReason::TimedOut,
        "failed" => ErrorReason::Failed,
        other => {
            return Err(Error::invalid_argument(format!(
                "Unknown error code: {other}"
            )));
        }
    })
}

/// An intercepted request, passed to route handlers. Cheap to clone.
#[derive(Clone)]
pub struct Route {
    inner: Arc<RouteInner>,
}

struct RouteInner {
    request: Request,
    shared: Arc<RouteShared>,
    page: Weak<PageInner>,
    /// Playwright's client `_handlingPromise`: set while a handler runs.
    handling: Mutex<Option<oneshot::Sender<bool>>>,
    did_throw: AtomicBool,
}

impl fmt::Debug for Route {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Route")
            .field("request", &self.inner.request)
            .finish()
    }
}

/// Request overrides of [`Route::continue_`], [`Route::fallback`] and
/// [`Route::fetch`].
#[derive(Default, Clone, Debug)]
struct ContinueOverrides {
    url: Option<String>,
    method: Option<String>,
    headers: Option<Vec<Header>>,
    post_data: Option<Vec<u8>>,
}

impl Route {
    pub(crate) fn new(request: Request, shared: Arc<RouteShared>, page: Weak<PageInner>) -> Self {
        Route {
            inner: Arc::new(RouteInner {
                request,
                shared,
                page,
                handling: Mutex::new(None),
                did_throw: AtomicBool::new(false),
            }),
        }
    }

    /// The request being routed.
    pub fn request(&self) -> Request {
        self.inner.request.clone()
    }

    fn browser(&self) -> Weak<BrowserInner> {
        self.inner
            .page
            .upgrade()
            .map(|p| p.browser.clone())
            .unwrap_or_default()
    }

    fn start_handling(&self) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        *self.inner.handling.lock().unwrap() = Some(tx);
        rx
    }

    fn check_not_handled(&self) -> Result<()> {
        if self.inner.handling.lock().unwrap().is_none() {
            return Err(Error::invalid_argument("Route is already handled!"));
        }
        Ok(())
    }

    fn report_handled(&self, done: bool) {
        if let Some(tx) = self.inner.handling.lock().unwrap().take() {
            let _ = tx.send(done);
        }
    }

    async fn handle_route(&self, f: impl Future<Output = Result<()>>) -> Result<()> {
        self.check_not_handled()?;
        match f.await {
            Ok(()) => {
                self.report_handled(true);
                Ok(())
            }
            Err(err) => {
                self.inner.did_throw.store(true, Ordering::SeqCst);
                Err(err)
            }
        }
    }

    fn apply_overrides(&self, overrides: &ContinueOverrides) {
        self.inner.request.update(|s| {
            if let Some(url) = &overrides.url {
                s.overrides.url = Some(url.clone());
            }
            if let Some(method) = &overrides.method {
                s.overrides.method = Some(method.clone());
            }
            if let Some(headers) = &overrides.headers {
                s.overrides.headers = Some(headers.clone());
            }
            if let Some(post_data) = &overrides.post_data {
                s.overrides.post_data = Some(post_data.clone());
            }
        });
    }

    fn start_server_handling(&self) -> Result<()> {
        if self.inner.shared.handled.swap(true, Ordering::SeqCst) {
            return Err(Error::invalid_argument("Route is already handled!"));
        }
        if let Some(page) = self.inner.page.upgrade() {
            NetworkManager::route_done(&page);
        }
        Ok(())
    }

    /// Server side `Route.continue` + `RouteImpl.continue`.
    async fn inner_continue(&self, overrides: ContinueOverrides) -> Result<()> {
        let request = &self.inner.request;
        if let Some(url) = &overrides.url {
            let new = url::Url::parse(url)
                .map_err(|_| Error::invalid_argument(format!("Invalid URL: {url}")))?;
            let old = url::Url::parse(&request.url())
                .map_err(|_| Error::invalid_argument("Invalid request URL"))?;
            if old.scheme() != new.scheme() {
                return Err(Error::invalid_argument(
                    "New URL must have same protocol as overridden URL",
                ));
            }
        }
        let mut overrides = overrides;
        if let Some(headers) = &overrides.headers {
            overrides.headers = Some(apply_headers_overrides(&request.inner.headers, headers));
        }
        self.apply_overrides(&overrides);
        let effective = {
            let s = request.inner.state.lock().unwrap();
            s.overrides.clone()
        };
        self.start_server_handling()?;
        let shared = &self.inner.shared;
        let browser = self.browser();
        if shared.response_stage.load(Ordering::SeqCst) {
            let params = ContinueResponseParams::new(shared.interception_id());
            return catch_disallowed(shared.send(&browser, params).await.map(|_| ()));
        }
        let mut params = ContinueRequestParams::new(shared.interception_id());
        params.url = effective.url.clone();
        params.method = effective.method.clone();
        params.headers = effective.headers.as_ref().map(|h| {
            header_entries(
                &h.iter()
                    .filter(|h| !h.name.eq_ignore_ascii_case("cookie"))
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        });
        params.post_data = effective
            .post_data
            .as_ref()
            .map(|d| base64::engine::general_purpose::STANDARD.encode(d).into());
        *shared.already_continued.lock().unwrap() = Some(params.clone());
        catch_disallowed(shared.send(&browser, params).await.map(|_| ()))
    }

    /// Continue the request, optionally with overrides. Builder; `.await` it.
    pub fn continue_(&self) -> Continue {
        Continue {
            route: self.clone(),
            overrides: ContinueOverrides::default(),
            serialization_error: None,
        }
    }

    /// Pass the request to the next matching handler (or to the network),
    /// optionally with overrides. Builder; `.await` it.
    pub fn fallback(&self) -> Fallback {
        Fallback {
            route: self.clone(),
            overrides: ContinueOverrides::default(),
            serialization_error: None,
        }
    }

    /// Abort the request. Builder; `.await` it.
    pub fn abort(&self) -> Abort {
        Abort {
            route: self.clone(),
            error_code: None,
        }
    }

    /// Fulfill the request with a response. Builder; `.await` it.
    pub fn fulfill(&self) -> Fulfill {
        Fulfill {
            route: self.clone(),
            status: None,
            headers: None,
            content_type: None,
            body: None,
            json: None,
            path: None,
            response: None,
        }
    }

    /// Perform the request through the browser's own network stack (the
    /// request continues with response interception) and return the
    /// response without completing the route; use
    /// [`Fulfill::response`] to answer with it, possibly modified.
    ///
    /// Unlike Playwright, the request is not sent from outside the browser:
    /// it keeps the browser's TLS/HTTP fingerprint and cookies. It can be
    /// called once per route. A redirect response is followed (up to
    /// `.max_redirects(n)`, default 20, like Playwright) with the context's
    /// [`crate::APIRequestContext`], i.e. outside the browser; with
    /// `.max_redirects(0)` the redirect is returned as-is.
    pub fn fetch(&self) -> Fetch {
        Fetch {
            route: self.clone(),
            overrides: ContinueOverrides::default(),
            serialization_error: None,
            timeout: None,
            max_redirects: None,
            max_retries: None,
        }
    }

    async fn inner_fetch(&self, overrides: ContinueOverrides) -> Result<APIResponse> {
        self.check_not_handled()?;
        let shared = self.inner.shared.clone();
        if shared.response_stage.load(Ordering::SeqCst) {
            return Err(Error::invalid_argument(
                "route.fetch() can only be called once per route",
            ));
        }
        let page = self.inner.page.upgrade().ok_or_else(Error::target_closed)?;
        let request = &self.inner.request;
        let mut merged = {
            let s = request.inner.state.lock().unwrap();
            s.overrides.clone()
        };
        if overrides.url.is_some() {
            merged.url = overrides.url.clone();
        }
        if overrides.method.is_some() {
            merged.method = overrides.method.clone();
        }
        if let Some(h) = &overrides.headers {
            merged.headers = Some(apply_headers_overrides(&request.inner.headers, h));
        }
        if overrides.post_data.is_some() {
            merged.post_data = overrides.post_data.clone();
        }
        let rx = page.ext.network.wait_for_response_stage(&request.inner.id);
        let mut params = ContinueRequestParams::new(shared.interception_id());
        params.url = merged.url.clone();
        params.method = merged.method.clone();
        params.headers = merged.headers.as_ref().map(|h| {
            header_entries(
                &h.iter()
                    .filter(|h| !h.name.eq_ignore_ascii_case("cookie"))
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        });
        params.post_data = merged
            .post_data
            .as_ref()
            .map(|d| base64::engine::general_purpose::STANDARD.encode(d).into());
        params.intercept_response = Some(true);
        let browser = self.browser();
        shared.send(&browser, params).await?;
        let paused = rx.await.map_err(|_| Error::target_closed())?;
        let event = paused.event;
        *shared.interception_id.lock().unwrap() = event.request_id.as_ref().to_string();
        shared.response_stage.store(true, Ordering::SeqCst);
        if let Some(reason) = &event.response_error_reason {
            return Err(Error::navigation(format!(
                "net::ERR_{}",
                reason.as_ref().to_uppercase()
            )));
        }
        let status = event.response_status_code.unwrap_or(0) as u16;
        let headers: Vec<Header> = event
            .response_headers
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|h| Header::new(h.name, h.value))
            .collect();
        let body = if (300..400).contains(&status) {
            Vec::new()
        } else {
            match shared
                .send(
                    &browser,
                    FetchGetResponseBodyParams::new(event.request_id.clone()),
                )
                .await
            {
                Ok(body) => {
                    if body.base64_encoded {
                        base64::engine::general_purpose::STANDARD
                            .decode(body.body)
                            .map_err(|e| Error::serialization(e.to_string()))?
                    } else {
                        body.body.into_bytes()
                    }
                }
                Err(err) => {
                    tracing::debug!("failed to read the fetched body: {err}");
                    Vec::new()
                }
            }
        };
        Ok(APIResponse {
            url: merged.url.unwrap_or_else(|| request.url()),
            status,
            status_text: event
                .response_status_text
                .clone()
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| status_text(status).to_string()),
            headers,
            body,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn inner_fulfill(
        &self,
        status: Option<u16>,
        headers: Option<Vec<Header>>,
        content_type: Option<String>,
        body: Option<Vec<u8>>,
        json: Option<Result<String>>,
        path: Option<PathBuf>,
        response: Option<APIResponse>,
    ) -> Result<()> {
        let mut body = body;
        let mut status = status;
        let mut headers_opt = headers;
        let is_json = json.is_some();
        if let Some(json) = json {
            if body.is_some() {
                return Err(Error::invalid_argument(
                    "Can specify either body or json parameters",
                ));
            }
            body = Some(json?.into_bytes());
        }
        if let Some(response) = &response {
            if status.is_none() {
                status = Some(response.status);
            }
            if headers_opt.is_none() {
                headers_opt = Some(response.headers.clone());
            }
            if body.is_none() && path.is_none() {
                body = Some(response.body.clone());
            }
        }
        if let Some(path) = &path {
            body = Some(
                tokio::fs::read(path)
                    .await
                    .map_err(|e| Error::invalid_argument(format!("{}: {e}", path.display())))?,
            );
        }
        let mut headers: Vec<Header> = Vec::new();
        for h in headers_opt.unwrap_or_default() {
            let name = h.name.to_lowercase();
            headers.retain(|x: &Header| x.name != name);
            headers.push(Header::new(name, h.value));
        }
        let set = |headers: &mut Vec<Header>, name: &str, value: String| {
            headers.retain(|x| x.name != name);
            headers.push(Header::new(name, value));
        };
        if let Some(ct) = &content_type {
            set(&mut headers, "content-type", ct.clone());
        } else if is_json {
            set(&mut headers, "content-type", "application/json".into());
        } else if let Some(path) = &path {
            let mime =
                mime_type_for_path(&path.to_string_lossy()).unwrap_or("application/octet-stream");
            set(&mut headers, "content-type", mime.into());
        }
        let length = body.as_ref().map(|b| b.len()).unwrap_or(0);
        if length > 0 && !headers.iter().any(|h| h.name == "content-length") {
            set(&mut headers, "content-length", length.to_string());
        }
        let status = status.unwrap_or(200);

        // Server side `Route.fulfill`.
        self.start_server_handling()?;
        let body = body.unwrap_or_default();
        if !(200..400).contains(&status) {
            let b = body.clone();
            self.inner.request.update(|s| s.body_override = Some(b));
        }
        self.maybe_add_cors_headers(&mut headers);
        self.inner.request.update(|s| s.fulfilled = true);
        // `splitSetCookieHeader`
        let mut entries = Vec::new();
        for h in &headers {
            if h.name.eq_ignore_ascii_case("set-cookie") {
                for part in h.value.split('\n') {
                    entries.push(Header::new(h.name.clone(), part));
                }
            } else {
                entries.push(h.clone());
            }
        }
        let recorded = entries.clone();
        self.inner
            .request
            .update(|s| s.fulfilled_headers = Some(recorded));
        let shared = &self.inner.shared;
        let mut params = FulfillRequestParams::new(shared.interception_id(), status as i64);
        params.response_phrase = Some(status_text(status).to_string());
        params.response_headers = Some(header_entries(&entries));
        params.body = Some(
            base64::engine::general_purpose::STANDARD
                .encode(&body)
                .into(),
        );
        catch_disallowed(shared.send(&self.browser(), params).await.map(|_| ()))
    }

    fn maybe_add_cors_headers(&self, headers: &mut Vec<Header>) {
        let request_headers = self.inner.request.headers_array_now();
        let Some(origin) = header_get(&request_headers, "origin") else {
            return;
        };
        let Ok(url) = url::Url::parse(&self.inner.request.url()) else {
            return;
        };
        if !url.scheme().starts_with("http") {
            return;
        }
        if url.origin().ascii_serialization() == origin.trim() {
            return;
        }
        if headers
            .iter()
            .any(|h| h.name == "access-control-allow-origin")
        {
            return;
        }
        headers.push(Header::new("access-control-allow-origin", origin));
        headers.push(Header::new("access-control-allow-credentials", "true"));
        headers.push(Header::new("vary", "Origin"));
    }
}

fn parse_headers(headers: impl IntoIterator<Item = (String, String)>) -> Vec<Header> {
    headers
        .into_iter()
        .map(|(name, value)| Header { name, value })
        .collect()
}

macro_rules! override_methods {
    () => {
        /// Override the request URL (same protocol).
        pub fn url(mut self, url: impl Into<String>) -> Self {
            self.overrides.url = Some(url.into());
            self
        }

        /// Override the HTTP method.
        pub fn method(mut self, method: impl Into<String>) -> Self {
            self.overrides.method = Some(method.into());
            self
        }

        /// Replace the request headers.
        pub fn headers<K: Into<String>, V: Into<String>>(
            mut self,
            headers: impl IntoIterator<Item = (K, V)>,
        ) -> Self {
            self.overrides.headers = Some(parse_headers(
                headers.into_iter().map(|(k, v)| (k.into(), v.into())),
            ));
            self
        }

        /// Override the request body.
        pub fn post_data(mut self, data: impl Into<Vec<u8>>) -> Self {
            self.overrides.post_data = Some(data.into());
            self
        }

        /// Override the request body with a JSON-serialized value.
        pub fn post_data_json<T: Serialize>(mut self, value: &T) -> Self {
            match serde_json::to_vec(value) {
                Ok(v) => self.overrides.post_data = Some(v),
                Err(e) => self.serialization_error = Some(e.to_string()),
            }
            self
        }
    };
}

/// Builder returned by [`Route::continue_`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct Continue {
    route: Route,
    overrides: ContinueOverrides,
    serialization_error: Option<String>,
}

impl Continue {
    override_methods!();
}

impl IntoFuture for Continue {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            if let Some(e) = self.serialization_error {
                return Err(Error::serialization(e).with_api("route.continue"));
            }
            let route = self.route.clone();
            let overrides = self.overrides;
            route
                .handle_route(async {
                    // Playwright's client applies the overrides and runs the
                    // remaining handlers... none remain in our merged model:
                    // `continue` always goes to the network.
                    self.route.inner_continue(overrides).await
                })
                .await
                .api("route.continue")
        })
    }
}

/// Builder returned by [`Route::fallback`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct Fallback {
    route: Route,
    overrides: ContinueOverrides,
    serialization_error: Option<String>,
}

impl Fallback {
    override_methods!();
}

impl IntoFuture for Fallback {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            if let Some(e) = self.serialization_error {
                return Err(Error::serialization(e).with_api("route.fallback"));
            }
            self.route.check_not_handled().api("route.fallback")?;
            self.route.apply_overrides(&self.overrides);
            self.route.report_handled(false);
            Ok(())
        })
    }
}

/// Builder returned by [`Route::fetch`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct Fetch {
    route: Route,
    overrides: ContinueOverrides,
    serialization_error: Option<String>,
    timeout: Option<u64>,
    max_redirects: Option<u32>,
    max_retries: Option<u32>,
}

impl Fetch {
    override_methods!();

    /// Maximum time in milliseconds (no limit by default; `0` also means
    /// no limit).
    pub fn timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout = Some(timeout_ms);
        self
    }

    /// Maximum number of redirects to follow (default 20; `0` returns the
    /// redirect response).
    pub fn max_redirects(mut self, max: u32) -> Self {
        self.max_redirects = Some(max);
        self
    }

    /// Retries of the followed redirect requests on `ECONNRESET`
    /// (default 0). The first request is the browser's own and is not
    /// retried.
    pub fn max_retries(mut self, max: u32) -> Self {
        self.max_retries = Some(max);
        self
    }

    /// Follow a redirect response with the context's request context.
    async fn follow_redirect(
        route: &Route,
        response: APIResponse,
        method: String,
        max_redirects: u32,
        max_retries: Option<u32>,
    ) -> Result<APIResponse> {
        let location = response
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("location"))
            .map(|h| h.value.clone());
        let Some(location) = location.filter(|l| !l.is_empty()) else {
            return Ok(response);
        };
        let Ok(next) = url::Url::parse(&response.url).and_then(|u| u.join(&location)) else {
            return Ok(response);
        };
        let page = route
            .inner
            .page
            .upgrade()
            .ok_or_else(Error::target_closed)?;
        let status = response.status;
        let method = if status == 303 || ((status == 301 || status == 302) && method == "POST") {
            "GET".to_string()
        } else {
            method
        };
        let mut fetch = Page::from_inner(page)
            .request()
            .fetch(next.to_string())
            .method(method)
            .max_redirects(max_redirects - 1)
            .fail_on_status_code(false);
        for header in route.inner.request.inner.headers.iter() {
            let name = header.name.to_ascii_lowercase();
            if matches!(name.as_str(), "cookie" | "host" | "content-length") {
                continue;
            }
            fetch = fetch.header(header.name.clone(), header.value.clone());
        }
        if let Some(retries) = max_retries {
            fetch = fetch.max_retries(retries);
        }
        let api = fetch.await?;
        let body = api.body().await?;
        Ok(APIResponse {
            url: api.url().to_string(),
            status: api.status(),
            status_text: api.status_text().to_string(),
            headers: api
                .headers_array()
                .into_iter()
                .map(|h| Header::new(h.name, h.value))
                .collect(),
            body,
        })
    }
}

impl IntoFuture for Fetch {
    type Output = Result<APIResponse>;
    type IntoFuture = BoxFuture<Result<APIResponse>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            if let Some(e) = self.serialization_error {
                return Err(Error::serialization(e).with_api("route.fetch"));
            }
            let max_redirects = self.max_redirects.unwrap_or(20);
            let max_retries = self.max_retries;
            let method = self
                .overrides
                .method
                .clone()
                .unwrap_or_else(|| self.route.inner.request.method());
            let route = self.route.clone();
            let run = async move {
                let response = route.inner_fetch(self.overrides).await?;
                if max_redirects > 0 && (300..400).contains(&response.status) {
                    return Fetch::follow_redirect(
                        &route,
                        response,
                        method,
                        max_redirects,
                        max_retries,
                    )
                    .await;
                }
                Ok(response)
            };
            match self.timeout {
                Some(ms) if ms > 0 => {
                    match tokio::time::timeout(std::time::Duration::from_millis(ms), run).await {
                        Ok(result) => result,
                        Err(_) => Err(Error::timeout_exceeded(ms)),
                    }
                }
                _ => run.await,
            }
            .api("route.fetch")
        })
    }
}

/// Builder returned by [`Route::abort`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct Abort {
    route: Route,
    error_code: Option<String>,
}

impl Abort {
    /// The network error: `aborted`, `accessdenied`, `addressunreachable`,
    /// `blockedbyclient`, `blockedbyresponse`, `connectionaborted`,
    /// `connectionclosed`, `connectionfailed`, `connectionrefused`,
    /// `connectionreset`, `internetdisconnected`, `namenotresolved`,
    /// `timedout` or `failed` (default).
    pub fn error_code(mut self, code: impl Into<String>) -> Self {
        self.error_code = Some(code.into());
        self
    }
}

impl IntoFuture for Abort {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let route = self.route.clone();
            let code = self.error_code.unwrap_or_else(|| "failed".to_string());
            route
                .handle_route(async {
                    error_reason(&code)?;
                    self.route.start_server_handling()?;
                    self.route
                        .inner
                        .shared
                        .abort_raw(&self.route.browser(), &code)
                        .await
                })
                .await
                .api("route.abort")
        })
    }
}

/// Builder returned by [`Route::fulfill`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct Fulfill {
    route: Route,
    status: Option<u16>,
    headers: Option<Vec<Header>>,
    content_type: Option<String>,
    body: Option<Vec<u8>>,
    json: Option<Result<String>>,
    path: Option<PathBuf>,
    response: Option<APIResponse>,
}

impl Fulfill {
    /// Response status code (default 200).
    pub fn status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    /// Response headers.
    pub fn headers<K: Into<String>, V: Into<String>>(
        mut self,
        headers: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        self.headers = Some(parse_headers(
            headers.into_iter().map(|(k, v)| (k.into(), v.into())),
        ));
        self
    }

    /// `Content-Type` header.
    pub fn content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// Response body.
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// JSON response body (sets `Content-Type: application/json` unless
    /// given).
    pub fn json<T: Serialize>(mut self, value: &T) -> Self {
        self.json = Some(serde_json::to_string(value).map_err(Error::from));
        self
    }

    /// Serve the content of a file (the content type is inferred from the
    /// extension).
    pub fn path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Use a response returned by [`Route::fetch`]; other options override
    /// its fields.
    pub fn response(mut self, response: &APIResponse) -> Self {
        self.response = Some(response.clone());
        self
    }
}

impl IntoFuture for Fulfill {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let route = self.route.clone();
            route
                .handle_route(async {
                    self.route
                        .inner_fulfill(
                            self.status,
                            self.headers,
                            self.content_type,
                            self.body,
                            self.json,
                            self.path,
                            self.response,
                        )
                        .await
                })
                .await
                .api("route.fulfill")
        })
    }
}

/// A response obtained with [`Route::fetch`].
#[derive(Clone, Debug)]
pub struct APIResponse {
    url: String,
    status: u16,
    status_text: String,
    headers: Vec<Header>,
    body: Vec<u8>,
}

impl APIResponse {
    /// Create a response (e.g. to fulfill a route with).
    pub fn new(status: u16, headers: Vec<Header>, body: impl Into<Vec<u8>>) -> Self {
        APIResponse {
            url: String::new(),
            status,
            status_text: status_text(status).to_string(),
            headers,
            body: body.into(),
        }
    }

    /// The URL of the response.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// HTTP status code.
    pub fn status(&self) -> u16 {
        self.status
    }

    /// HTTP status text.
    pub fn status_text(&self) -> &str {
        &self.status_text
    }

    /// Whether the status is in the 200-299 range.
    pub fn ok(&self) -> bool {
        (200..=299).contains(&self.status)
    }

    /// Headers with lower-cased names.
    pub fn headers(&self) -> HashMap<String, String> {
        headers_to_map(&self.headers)
    }

    /// Headers as a list.
    pub fn headers_array(&self) -> &[Header] {
        &self.headers
    }

    /// The body.
    pub async fn body(&self) -> Result<Vec<u8>> {
        Ok(self.body.clone())
    }

    /// The body as text (lossy UTF-8).
    pub async fn text(&self) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.body).into_owned())
    }

    /// The body parsed as JSON.
    pub async fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.body).map_err(Error::from)
    }
}

// ----------------------------------------------------------------------
// Route handlers
// ----------------------------------------------------------------------

/// Identifies a route registered with `route`, for `unroute().handler(id)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RouteId(u64);

static NEXT_ROUTE_ID: AtomicU64 = AtomicU64::new(1);

/// What `unroute_all` does with handlers that are still running.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UnrouteBehavior {
    /// Do not wait, errors of running handlers propagate (default).
    #[default]
    Default,
    /// Wait for running handlers to finish.
    Wait,
    /// Do not wait and ignore errors of running handlers.
    IgnoreErrors,
}

pub(crate) struct RouteHandler {
    pub(crate) id: RouteId,
    pub(crate) url: UrlMatch,
    base_url: Option<String>,
    handler: RouteHandlerFn,
    times: Option<usize>,
    handled_count: AtomicUsize,
    active: watch::Sender<usize>,
    ignore_exception: AtomicBool,
}

impl RouteHandler {
    pub(crate) fn new(
        base_url: Option<String>,
        url: UrlMatch,
        handler: RouteHandlerFn,
        times: Option<usize>,
    ) -> Self {
        RouteHandler {
            id: RouteId(NEXT_ROUTE_ID.fetch_add(1, Ordering::SeqCst)),
            url,
            base_url,
            handler,
            times,
            handled_count: AtomicUsize::new(0),
            active: watch::channel(0).0,
            ignore_exception: AtomicBool::new(false),
        }
    }

    fn matches(&self, url: &str) -> bool {
        self.url.matches(self.base_url.as_deref(), url)
    }

    fn will_expire(&self) -> bool {
        self.times
            .is_some_and(|t| self.handled_count.load(Ordering::SeqCst) + 1 >= t)
    }

    async fn handle(&self, route: Route) -> bool {
        self.active.send_modify(|n| *n += 1);
        self.handled_count.fetch_add(1, Ordering::SeqCst);
        let handled = route.start_handling();
        let fut = (self.handler)(route.clone());
        let (handled, result) = futures::future::join(handled, fut).await;
        let handled = match (handled, result) {
            (Ok(handled), Ok(())) => handled,
            (handled, Err(err)) => {
                if !self.ignore_exception.load(Ordering::SeqCst) {
                    tracing::warn!("route handler failed: {err}");
                }
                // Do not stall the request: an unhandled route falls through.
                if route.inner.handling.lock().unwrap().take().is_some() {
                    false
                } else {
                    handled.unwrap_or(true)
                }
            }
            (Err(_), Ok(())) => false,
        };
        self.active.send_modify(|n| *n -= 1);
        handled
    }

    async fn stop(&self, behavior: UnrouteBehavior) {
        match behavior {
            UnrouteBehavior::IgnoreErrors => self.ignore_exception.store(true, Ordering::SeqCst),
            UnrouteBehavior::Wait => {
                let mut rx = self.active.subscribe();
                let _ = rx.wait_for(|n| *n == 0).await;
            }
            UnrouteBehavior::Default => {}
        }
    }
}

/// Run the route through the page's and the context's handlers
/// (`Page._onRoute` then `BrowserContext._onRoute`).
pub(crate) async fn handle_route(page: Arc<PageInner>, route: Route) {
    let page_handlers: Vec<Arc<RouteHandler>> = page.ext.routes.lock().unwrap().clone();
    for handler in page_handlers {
        if page.is_closed() {
            return;
        }
        if !handler.matches(&route.request().url()) {
            continue;
        }
        {
            let mut routes = page.ext.routes.lock().unwrap();
            let Some(index) = routes.iter().position(|h| Arc::ptr_eq(h, &handler)) else {
                continue;
            };
            if handler.will_expire() {
                routes.remove(index);
            }
        }
        let handled = handler.handle(route.clone()).await;
        if page.ext.routes.lock().unwrap().is_empty() {
            let page = page.clone();
            tokio::spawn(async move {
                let _ = NetworkManager::update_interception(&page).await;
            });
        }
        if handled {
            return;
        }
    }
    let context = page.context.upgrade();
    if let Some(context) = &context {
        let context_handlers: Vec<Arc<RouteHandler>> = context.ext.routes.lock().unwrap().clone();
        for handler in context_handlers {
            if page.is_closed()
                || (BrowserContext {
                    inner: context.clone(),
                })
                .is_closed()
            {
                return;
            }
            if !handler.matches(&route.request().url()) {
                continue;
            }
            {
                let mut routes = context.ext.routes.lock().unwrap();
                let Some(index) = routes.iter().position(|h| Arc::ptr_eq(h, &handler)) else {
                    continue;
                };
                if handler.will_expire() {
                    routes.remove(index);
                }
            }
            let handled = handler.handle(route.clone()).await;
            if context.ext.routes.lock().unwrap().is_empty() {
                let context = BrowserContext {
                    inner: context.clone(),
                };
                tokio::spawn(async move {
                    context.update_interception().await;
                });
            }
            if handled {
                return;
            }
        }
    }
    // Nobody handled it: continue with the fallback overrides.
    let overrides = {
        let s = route.inner.request.inner.state.lock().unwrap();
        ContinueOverrides {
            url: s.overrides.url.clone(),
            method: s.overrides.method.clone(),
            headers: s.overrides.headers.clone(),
            post_data: s.overrides.post_data.clone(),
        }
    };
    if let Err(err) = route.inner_continue(overrides).await {
        tracing::debug!("failed to continue an unhandled route: {err}");
    }
}

/// `urlMatchesEqual`.
pub(crate) fn url_matches_equal(a: &UrlMatch, b: &UrlMatch) -> bool {
    match (a, b) {
        (UrlMatch::Glob(a), UrlMatch::Glob(b)) => a == b,
        (UrlMatch::Regex(a), UrlMatch::Regex(b)) => a.as_str() == b.as_str(),
        (UrlMatch::Predicate(a), UrlMatch::Predicate(b)) => Arc::ptr_eq(a, b),
        _ => false,
    }
}

pub(crate) fn validate_glob(base_url: Option<&str>, url: &UrlMatch) -> Result<()> {
    if let UrlMatch::Glob(glob) = url {
        let pattern = resolve_glob_to_regex_pattern(base_url, glob);
        if regex::Regex::new(&pattern).is_err() {
            return Err(Error::invalid_argument(format!(
                "Invalid glob pattern: {glob}"
            )));
        }
    }
    Ok(())
}

pub(crate) fn boxed_handler<F, Fut>(handler: F) -> RouteHandlerFn
where
    F: Fn(Route) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    Arc::new(move |route| Box::pin(handler(route)))
}

/// Where a route is registered.
#[derive(Clone)]
pub(crate) enum RouteTarget {
    Page(Page),
    Context(BrowserContext),
}

impl RouteTarget {
    fn routes(&self) -> &Mutex<Vec<Arc<RouteHandler>>> {
        match self {
            RouteTarget::Page(p) => &p.inner.ext.routes,
            RouteTarget::Context(c) => &c.inner.ext.routes,
        }
    }

    fn base_url(&self) -> Option<String> {
        match self {
            RouteTarget::Page(p) => p.inner.base_url(),
            RouteTarget::Context(c) => c.inner.options.base_url.clone(),
        }
    }

    fn prefix(&self) -> &'static str {
        match self {
            RouteTarget::Page(_) => "page",
            RouteTarget::Context(_) => "browserContext",
        }
    }

    pub(crate) async fn update(&self) -> Result<()> {
        match self {
            RouteTarget::Page(p) => NetworkManager::update_interception(&p.inner).await,
            RouteTarget::Context(c) => {
                c.update_interception().await;
                Ok(())
            }
        }
    }

    pub(crate) async fn add(&self, handler: RouteHandler) -> Result<RouteId> {
        let id = handler.id;
        self.routes().lock().unwrap().insert(0, Arc::new(handler));
        self.update().await?;
        Ok(id)
    }

    async fn remove(
        &self,
        filter: impl Fn(&RouteHandler) -> bool,
        behavior: UnrouteBehavior,
    ) -> Result<()> {
        let removed: Vec<Arc<RouteHandler>> = {
            let mut routes = self.routes().lock().unwrap();
            let (removed, remaining): (Vec<_>, Vec<_>) = routes.drain(..).partition(|h| filter(h));
            *routes = remaining;
            removed
        };
        if behavior != UnrouteBehavior::Default {
            futures::future::join_all(removed.iter().map(|h| h.stop(behavior))).await;
        }
        self.update().await
    }
}

/// Builder returned by `route`; `.await` it to register the handler.
#[must_use = "builders do nothing unless awaited"]
pub struct RouteBuilder {
    pub(crate) target: RouteTarget,
    pub(crate) url: UrlMatch,
    pub(crate) handler: RouteHandlerFn,
    pub(crate) times: Option<usize>,
}

impl RouteBuilder {
    /// Only handle the first `times` matching requests.
    pub fn times(mut self, times: usize) -> Self {
        self.times = Some(times);
        self
    }
}

impl IntoFuture for RouteBuilder {
    type Output = Result<RouteId>;
    type IntoFuture = BoxFuture<Result<RouteId>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let api = if matches!(self.target, RouteTarget::Page(_)) {
                "page.route"
            } else {
                "browserContext.route"
            };
            let base_url = self.target.base_url();
            validate_glob(base_url.as_deref(), &self.url).api(api)?;
            let handler = RouteHandler::new(base_url, self.url, self.handler, self.times);
            self.target.add(handler).await.api(api)
        })
    }
}

/// Builder returned by `unroute`; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct Unroute {
    pub(crate) target: RouteTarget,
    pub(crate) url: UrlMatch,
    pub(crate) id: Option<RouteId>,
}

impl Unroute {
    /// Only remove the handler registered with this id.
    pub fn handler(mut self, id: RouteId) -> Self {
        self.id = Some(id);
        self
    }
}

impl IntoFuture for Unroute {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let api = format!("{}.unroute", self.target.prefix());
            let url = self.url;
            let id = self.id;
            self.target
                .remove(
                    |h| url_matches_equal(&h.url, &url) && id.is_none_or(|id| h.id == id),
                    UnrouteBehavior::Default,
                )
                .await
                .api(&api)
        })
    }
}

/// Builder returned by `unroute_all`; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct UnrouteAll {
    pub(crate) target: RouteTarget,
    pub(crate) behavior: UnrouteBehavior,
}

impl UnrouteAll {
    /// What to do with handlers that are still running.
    pub fn behavior(mut self, behavior: UnrouteBehavior) -> Self {
        self.behavior = behavior;
        self
    }
}

impl IntoFuture for UnrouteAll {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let api = format!("{}.unrouteAll", self.target.prefix());
            self.target.remove(|_| true, self.behavior).await.api(&api)
        })
    }
}

impl Page {
    /// Route requests matching `url` to `handler`; the most recently added
    /// handler runs first. Enables request interception (`Fetch.enable`)
    /// while at least one route exists.
    ///
    /// ```no_run
    /// # async fn run(page: oxidewright::Page) -> oxidewright::Result<()> {
    /// page.route("**/*.png", |route| async move { route.abort().await }).await?;
    /// # Ok(()) }
    /// ```
    pub fn route<F, Fut>(&self, url: impl Into<UrlMatch>, handler: F) -> RouteBuilder
    where
        F: Fn(Route) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        RouteBuilder {
            target: RouteTarget::Page(self.clone()),
            url: url.into(),
            handler: boxed_handler(handler),
            times: None,
        }
    }

    /// Remove the routes registered for `url` (all of them, or one with
    /// [`Unroute::handler`]).
    pub fn unroute(&self, url: impl Into<UrlMatch>) -> Unroute {
        Unroute {
            target: RouteTarget::Page(self.clone()),
            url: url.into(),
            id: None,
        }
    }

    /// Remove every route of the page.
    pub fn unroute_all(&self) -> UnrouteAll {
        UnrouteAll {
            target: RouteTarget::Page(self.clone()),
            behavior: UnrouteBehavior::Default,
        }
    }
}

impl BrowserContext {
    /// Route requests of every page of the context; page routes run first.
    pub fn route<F, Fut>(&self, url: impl Into<UrlMatch>, handler: F) -> RouteBuilder
    where
        F: Fn(Route) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        RouteBuilder {
            target: RouteTarget::Context(self.clone()),
            url: url.into(),
            handler: boxed_handler(handler),
            times: None,
        }
    }

    /// Remove the routes registered for `url`.
    pub fn unroute(&self, url: impl Into<UrlMatch>) -> Unroute {
        Unroute {
            target: RouteTarget::Context(self.clone()),
            url: url.into(),
            id: None,
        }
    }

    /// Remove every route of the context.
    pub fn unroute_all(&self) -> UnrouteAll {
        UnrouteAll {
            target: RouteTarget::Context(self.clone()),
            behavior: UnrouteBehavior::Default,
        }
    }

    pub(crate) async fn update_interception(&self) {
        for page in self.pages() {
            let _ = NetworkManager::update_interception(&page.inner).await;
        }
    }
}
