//! The network manager of a page: turns `Network.*` and `Fetch.*` events into
//! [`Request`]/[`Response`] objects and events, and drives request
//! interception (a port of Playwright's `server/chromium/crNetworkManager.ts`).
//!
//! The `Fetch` domain is only enabled while routes (or HTTP credentials)
//! exist, with a catch-all pattern, like Playwright. Paused requests are
//! delivered by chromiumoxide's managed paused-request stream, which carries
//! the CDP session (OOPIF requests pause in the child session).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use base64::Engine;
use chromiumoxide::PausedRequest;
use chromiumoxide::cdp::browser_protocol::fetch::{
    AuthChallengeResponse, AuthChallengeResponseResponse, ContinueRequestParams,
    ContinueWithAuthParams, DisableParams as FetchDisableParams, EnableParams as FetchEnableParams,
    EventAuthRequired, EventRequestPaused, FulfillRequestParams, HeaderEntry, RequestPattern,
    RequestStage,
};
#[allow(deprecated)]
use chromiumoxide::cdp::browser_protocol::network::{
    EmulateNetworkConditionsParams, EventLoadingFailed, EventLoadingFinished,
    EventRequestServedFromCache, EventRequestWillBeSent, EventRequestWillBeSentExtraInfo,
    EventResponseReceived, EventResponseReceivedExtraInfo, InitiatorType, Response as CdpResponse,
    SetCacheDisabledParams, SetExtraHttpHeadersParams,
};
use chromiumoxide::cdp::browser_protocol::target::SessionId;
use futures::StreamExt;
use tokio::sync::{oneshot, watch};

use crate::events::Event;
use crate::network::{
    Header, Request, RequestInner, RequestState, ResourceTiming, ResponseData, SecurityDetails,
    apply_headers_overrides, headers_from_cdp, headers_to_cdp, merge_headers, status_text,
};
use crate::page::PageInner;
use crate::route::{Route, RouteShared};

/// Credentials for HTTP authentication (`httpCredentials`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpCredentials {
    /// User name.
    pub username: String,
    /// Password.
    pub password: String,
    /// Only send the credentials to this origin (`scheme://host[:port]`).
    pub origin: Option<String>,
}

impl HttpCredentials {
    /// Credentials sent to any origin that asks for them.
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        HttpCredentials {
            username: username.into(),
            password: password.into(),
            origin: None,
        }
    }

    /// Restrict the credentials to one origin.
    pub fn origin(mut self, origin: impl Into<String>) -> Self {
        self.origin = Some(origin.into());
        self
    }

    fn matches(&self, url: &str) -> bool {
        let Some(origin) = &self.origin else {
            return true;
        };
        match url::Url::parse(url) {
            Ok(u) => u.origin().ascii_serialization().to_lowercase() == origin.to_lowercase(),
            Err(_) => false,
        }
    }
}

/// A paused response for `route.fetch()`.
pub(crate) struct PausedResponse {
    pub(crate) event: Arc<EventRequestPaused>,
}

#[derive(Default)]
struct NmState {
    will_be_sent: HashMap<String, Arc<EventRequestWillBeSent>>,
    paused: HashMap<String, (Arc<EventRequestPaused>, SessionId)>,
    response_waiters: HashMap<String, oneshot::Sender<PausedResponse>>,
    user_interception: bool,
    protocol_interception: bool,
    attempted_auth: HashSet<String>,
    extra_info: ExtraInfoTracker,
    /// Sessions where interception/extra headers/offline were applied.
    sessions: HashSet<String>,
    /// Routes not handled yet: `Fetch.disable` waits for them.
    routes_in_flight: usize,
    pending_update: bool,
}

pub(crate) struct NetworkManager {
    state: Mutex<NmState>,
    stream_started: AtomicBool,
    /// Serializes interception updates.
    update_lock: tokio::sync::Mutex<()>,
}

impl Default for NetworkManager {
    fn default() -> Self {
        NetworkManager {
            state: Mutex::new(NmState::default()),
            stream_started: AtomicBool::new(false),
            update_lock: tokio::sync::Mutex::new(()),
        }
    }
}

pub(crate) fn wall_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn to_resource_type(
    t: Option<&chromiumoxide::cdp::browser_protocol::network::ResourceType>,
) -> String {
    use chromiumoxide::cdp::browser_protocol::network::ResourceType as R;
    match t {
        Some(R::Document) => "document",
        Some(R::Stylesheet) => "stylesheet",
        Some(R::Image) => "image",
        Some(R::Media) => "media",
        Some(R::Font) => "font",
        Some(R::Script) => "script",
        Some(R::TextTrack) => "texttrack",
        Some(R::Xhr) => "xhr",
        Some(R::Fetch) => "fetch",
        Some(R::EventSource) => "eventsource",
        Some(R::WebSocket) => "websocket",
        Some(R::Manifest) => "manifest",
        Some(R::Ping) => "ping",
        Some(R::CspViolationReport) => "cspreport",
        _ => "other",
    }
    .to_string()
}

fn post_data_of(
    request: &chromiumoxide::cdp::browser_protocol::network::Request,
) -> Option<Vec<u8>> {
    let entries = request.post_data_entries.as_ref()?;
    let mut out = Vec::new();
    let mut any = false;
    for entry in entries {
        if let Some(bytes) = &entry.bytes {
            let s: &str = bytes.as_ref();
            if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(s) {
                out.extend_from_slice(&decoded);
                any = true;
            }
        }
    }
    any.then_some(out)
}

fn strip_fragment(url: &str) -> String {
    match url.find('#') {
        Some(i) => url[..i].to_string(),
        None => url.to_string(),
    }
}

/// Send a command on a session of the page's browser.
pub(crate) async fn send<C: chromiumoxide::types::Command>(
    page: &PageInner,
    session: Option<SessionId>,
    cmd: C,
) -> crate::Result<C::Response> {
    let browser = page
        .browser
        .upgrade()
        .ok_or_else(crate::Error::target_closed)?;
    browser.execute_with_session(cmd, session).await
}

pub(crate) fn header_entries(headers: &[Header]) -> Vec<HeaderEntry> {
    headers
        .iter()
        .map(|h| HeaderEntry::new(h.name.clone(), h.value.clone()))
        .collect()
}

impl NetworkManager {
    // ------------------------------------------------------------------
    // Configuration
    // ------------------------------------------------------------------

    /// All CDP sessions of the page (main + out-of-process frames).
    async fn sessions(page: &Arc<PageInner>) -> Vec<SessionId> {
        let mut out = vec![page.cdp.session_id().clone()];
        for frame in page.frames_dfs() {
            if let Some(cdp) = frame.cdp_frame_now() {
                let id = cdp.session_id().clone();
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
        out
    }

    async fn ensure_stream(page: &Arc<PageInner>) {
        let nm = &page.ext.network;
        if nm.stream_started.swap(true, Ordering::SeqCst) {
            return;
        }
        match page.cdp.paused_requests().await {
            Ok(mut stream) => {
                let weak = Arc::downgrade(page);
                tokio::spawn(async move {
                    while let Some(paused) = stream.next().await {
                        let Some(page) = weak.upgrade() else {
                            break;
                        };
                        on_request_paused(&page, paused);
                    }
                });
            }
            Err(err) => {
                tracing::debug!("failed to register the paused request stream: {err}");
                nm.stream_started.store(false, Ordering::SeqCst);
            }
        }
    }

    async fn configure_session(
        page: &Arc<PageInner>,
        session: SessionId,
        interception: bool,
        initial: bool,
    ) -> crate::Result<()> {
        if !(initial && !interception) {
            send(
                page,
                Some(session.clone()),
                SetCacheDisabledParams::new(interception),
            )
            .await?;
            if interception {
                let mut pattern = RequestPattern::default();
                pattern.url_pattern = Some("*".into());
                pattern.request_stage = Some(RequestStage::Request);
                let mut params = FetchEnableParams::default();
                params.handle_auth_requests = Some(true);
                params.patterns = Some(vec![pattern]);
                send(page, Some(session.clone()), params).await?;
            } else {
                send(page, Some(session.clone()), FetchDisableParams::default()).await?;
            }
        }
        Ok(())
    }

    /// Recompute whether requests must be intercepted (routes or HTTP
    /// credentials on the page or its context) and update every session.
    pub(crate) async fn update_interception(page: &Arc<PageInner>) -> crate::Result<()> {
        let nm = &page.ext.network;
        let _guard = nm.update_lock.lock().await;
        let context = page.context.upgrade();
        let user = !page.ext.routes.lock().unwrap().is_empty()
            || context
                .as_ref()
                .is_some_and(|c| !c.ext.routes.lock().unwrap().is_empty());
        let credentials = context.as_ref().is_some_and(|c| {
            c.ext.http_credentials.lock().unwrap().is_some()
                || crate::proxy::context_credentials(c).is_some()
        });
        let enabled = user || credentials;
        {
            let mut state = nm.state.lock().unwrap();
            state.user_interception = user;
            if state.protocol_interception == enabled {
                return Ok(());
            }
            if !enabled && state.routes_in_flight > 0 {
                // Disabling `Fetch` while requests are paused would leave
                // them stuck: wait for the routes in flight.
                state.pending_update = true;
                return Ok(());
            }
            state.protocol_interception = enabled;
        }
        if enabled {
            Self::ensure_stream(page).await;
        }
        for session in Self::sessions(page).await {
            let main = &session == page.cdp.session_id();
            let result = Self::configure_session(page, session, enabled, false).await;
            if let Err(err) = result {
                if main && !err.is_target_closed() {
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    /// Apply the current extra HTTP headers to every session.
    pub(crate) async fn update_extra_headers(page: &Arc<PageInner>) -> crate::Result<()> {
        let headers = Self::extra_headers(page);
        for session in Self::sessions(page).await {
            let main = &session == page.cdp.session_id();
            let result = send(
                page,
                Some(session),
                SetExtraHttpHeadersParams::new(headers_to_cdp(&headers)),
            )
            .await;
            if let Err(err) = result {
                if main {
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    fn extra_headers(page: &PageInner) -> Vec<Header> {
        let context_headers = page
            .context
            .upgrade()
            .map(|c| c.ext.extra_headers.lock().unwrap().clone())
            .unwrap_or_default();
        let page_headers = page.ext.extra_headers.lock().unwrap().clone();
        merge_headers(&[&context_headers, &page_headers])
    }

    fn offline(page: &PageInner) -> bool {
        page.context
            .upgrade()
            .is_some_and(|c| c.ext.offline.load(Ordering::SeqCst))
    }

    /// Apply offline emulation to every session.
    pub(crate) async fn update_offline(page: &Arc<PageInner>) -> crate::Result<()> {
        let offline = Self::offline(page);
        for session in Self::sessions(page).await {
            let main = &session == page.cdp.session_id();
            #[allow(deprecated)]
            let params = EmulateNetworkConditionsParams::new(offline, 0.0, -1.0, -1.0);
            let result = send(page, Some(session), params).await;
            if let Err(err) = result {
                if main {
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    /// Configure a new session (page attach or a new out-of-process frame).
    pub(crate) async fn add_session(page: &Arc<PageInner>, session: SessionId) {
        {
            let mut state = page.ext.network.state.lock().unwrap();
            if !state.sessions.insert(session.as_ref().to_string()) {
                return;
            }
        }
        let interception = page.ext.network.state.lock().unwrap().protocol_interception;
        if interception {
            Self::ensure_stream(page).await;
        }
        let _ = Self::configure_session(page, session.clone(), interception, true).await;
        let headers = Self::extra_headers(page);
        if !headers.is_empty() {
            let _ = send(
                page,
                Some(session.clone()),
                SetExtraHttpHeadersParams::new(headers_to_cdp(&headers)),
            )
            .await;
        }
        if Self::offline(page) {
            #[allow(deprecated)]
            let params = EmulateNetworkConditionsParams::new(true, 0.0, -1.0, -1.0);
            let _ = send(page, Some(session), params).await;
        }
    }

    /// Register a waiter for the response-stage pause of a request.
    pub(crate) fn wait_for_response_stage(
        &self,
        network_id: &str,
    ) -> oneshot::Receiver<PausedResponse> {
        let (tx, rx) = oneshot::channel();
        self.state
            .lock()
            .unwrap()
            .response_waiters
            .insert(network_id.to_string(), tx);
        rx
    }

    /// A route got handled (continued, fulfilled or aborted).
    pub(crate) fn route_done(page: &Arc<PageInner>) {
        let update = {
            let mut state = page.ext.network.state.lock().unwrap();
            state.routes_in_flight = state.routes_in_flight.saturating_sub(1);
            let update = state.routes_in_flight == 0 && state.pending_update;
            if update {
                state.pending_update = false;
            }
            update
        };
        if update {
            let page = page.clone();
            tokio::spawn(async move {
                let _ = NetworkManager::update_interception(&page).await;
            });
        }
    }

    /// Fail every pending request (the page closed).
    pub(crate) fn close(&self, page: &PageInner) {
        let requests: Vec<Request> = page.requests.lock().unwrap().values().cloned().collect();
        for request in requests {
            request.update(|s| s.closed = true);
        }
        let redirects = std::mem::take(&mut *page.ext.redirects.lock().unwrap());
        for inner in redirects.iter().filter_map(Weak::upgrade) {
            inner.state.lock().unwrap().redirected_to = None;
        }
        let mut state = self.state.lock().unwrap();
        state.response_waiters.clear();
        state.paused.clear();
        state.will_be_sent.clear();
    }
}

// ----------------------------------------------------------------------
// Event handling
// ----------------------------------------------------------------------

pub(crate) fn on_request_will_be_sent(page: &PageInner, ev: Arc<EventRequestWillBeSent>) {
    let nm = &page.ext.network;
    let mut state = nm.state.lock().unwrap();
    if state.protocol_interception && !ev.request.url.starts_with("data:") {
        let id = ev.request_id.as_ref().to_string();
        if let Some((paused, session)) = state.paused.remove(&id) {
            on_request(page, &mut state, &ev, Some((paused, session)));
        } else {
            state.will_be_sent.insert(id, ev);
        }
    } else {
        on_request(page, &mut state, &ev, None);
    }
}

fn on_request_paused(page: &Arc<PageInner>, paused: PausedRequest) {
    let event = Arc::new(paused.event().clone());
    let session = paused.session_id().clone();
    drop(paused);
    let nm = &page.ext.network;
    let mut state = nm.state.lock().unwrap();
    let Some(network_id) = event.network_id.as_ref().map(|n| n.as_ref().to_string()) else {
        drop(state);
        continue_paused(page, &event, &session, None);
        return;
    };
    // Response stage (route.fetch).
    if event.response_status_code.is_some() || event.response_error_reason.is_some() {
        if let Some(tx) = state.response_waiters.remove(&network_id) {
            let _ = tx.send(PausedResponse { event });
        } else {
            drop(state);
            continue_response(page, &event, &session);
        }
        return;
    }
    if event.request.url.starts_with("data:") {
        return;
    }
    if let Some(ev) = state.will_be_sent.remove(&network_id) {
        on_request(page, &mut state, &ev, Some((event, session)));
        return;
    }
    let existing = page.requests.lock().unwrap().get(&network_id).cloned();
    if let Some(existing) = existing {
        if event.redirected_request_id.is_none() {
            let continued = existing
                .inner
                .route
                .as_ref()
                .and_then(|r| r.already_continued.lock().unwrap().clone());
            if continued.is_some() || existing.inner.route.is_none() {
                // The network stack restarted the request (or it was reported
                // while interception was off): continue it the same way.
                drop(state);
                continue_paused(page, &event, &session, continued);
                return;
            }
        }
    }
    state.paused.insert(network_id, (event, session));
}

/// Continue a paused request, optionally with the parameters of a previous
/// `Fetch.continueRequest`.
pub(crate) fn continue_paused(
    page: &PageInner,
    event: &EventRequestPaused,
    session: &SessionId,
    params: Option<ContinueRequestParams>,
) {
    let mut params = params.unwrap_or_else(|| ContinueRequestParams::new(event.request_id.clone()));
    params.request_id = event.request_id.clone();
    spawn_send(page, session.clone(), params);
}

fn continue_response(page: &PageInner, event: &EventRequestPaused, session: &SessionId) {
    let params = chromiumoxide::cdp::browser_protocol::fetch::ContinueResponseParams::new(
        event.request_id.clone(),
    );
    spawn_send(page, session.clone(), params);
}

fn spawn_send<C: chromiumoxide::types::Command + Send + 'static>(
    page: &PageInner,
    session: SessionId,
    cmd: C,
) where
    C::Response: Send,
{
    let browser = page.browser.clone();
    tokio::spawn(async move {
        if let Some(browser) = browser.upgrade() {
            if let Err(err) = browser.execute_with_session(cmd, Some(session)).await {
                tracing::debug!("fetch command failed: {err}");
            }
        }
    });
}

fn on_request(
    page: &PageInner,
    state: &mut NmState,
    ev: &EventRequestWillBeSent,
    paused: Option<(Arc<EventRequestPaused>, SessionId)>,
) {
    if ev.request.url.starts_with("data:") {
        return;
    }
    let request_id = ev.request_id.as_ref().to_string();
    let mut redirected_from: Option<Request> = None;
    if let Some(redirect) = &ev.redirect_response {
        let previous = page.requests.lock().unwrap().remove(&request_id);
        if let Some(previous) = previous {
            handle_redirect(
                page,
                state,
                &previous,
                redirect,
                *ev.timestamp.inner(),
                ev.redirect_has_extra_info,
            );
            redirected_from = Some(previous);
        }
    }
    let mut frame = ev
        .frame_id
        .as_ref()
        .and_then(|id| page.frame_by_id(id.as_ref()));
    if frame.is_none() {
        if let Some((p, _)) = &paused {
            frame = page.frame_by_id(p.frame_id.as_ref());
        }
    }

    let is_preflight = paused
        .as_ref()
        .is_some_and(|(p, _)| p.request.method == "OPTIONS")
        && ev.initiator.r#type == InitiatorType::Preflight;
    if is_preflight && state.user_interception {
        let (p, session) = paused.as_ref().unwrap();
        let headers = crate::network::headers_from_cdp(&p.request.headers, false);
        let get = |name: &str| {
            headers
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case(name))
                .map(|h| h.value.clone())
        };
        let mut response_headers = vec![
            HeaderEntry::new(
                "Access-Control-Allow-Origin",
                get("Origin").unwrap_or_else(|| "*".into()),
            ),
            HeaderEntry::new(
                "Access-Control-Allow-Methods",
                get("Access-Control-Request-Method")
                    .unwrap_or_else(|| "GET, POST, OPTIONS, DELETE".into()),
            ),
            HeaderEntry::new("Access-Control-Allow-Credentials", "true"),
        ];
        if let Some(h) = get("Access-Control-Request-Headers") {
            response_headers.push(HeaderEntry::new("Access-Control-Allow-Headers", h));
        }
        let mut params = FulfillRequestParams::new(p.request_id.clone(), 204);
        params.response_phrase = Some(status_text(204).into());
        params.response_headers = Some(response_headers);
        params.body = Some(String::new().into());
        spawn_send(page, session.clone(), params);
        return;
    }

    let Some(frame) = frame else {
        if let Some((p, session)) = &paused {
            continue_paused(page, p, session, None);
        }
        return;
    };

    let mut route_shared: Option<Arc<RouteShared>> = None;
    let mut headers_override: Option<Vec<Header>> = None;
    if let Some((p, session)) = &paused {
        if redirected_from.is_some() || (!state.user_interception && state.protocol_interception) {
            // Redirects are not intercepted; header overrides are carried over.
            let previous = redirected_from
                .as_ref()
                .and_then(|r| r.inner.route.as_ref())
                .and_then(|r| r.already_continued.lock().unwrap().clone())
                .and_then(|c| c.headers);
            let mut params = ContinueRequestParams::new(p.request_id.clone());
            if let Some(previous) = previous {
                let original = crate::network::headers_from_cdp(&p.request.headers, false);
                let overrides: Vec<Header> = previous
                    .into_iter()
                    .map(|h| Header::new(h.name, h.value))
                    .collect();
                let merged: Vec<Header> = apply_headers_overrides(&original, &overrides)
                    .into_iter()
                    .filter(|h| !h.name.eq_ignore_ascii_case("cookie"))
                    .collect();
                params.headers = Some(header_entries(&merged));
                headers_override = Some(merged);
            }
            spawn_send(page, session.clone(), params);
        } else {
            route_shared = Some(Arc::new(RouteShared::new(
                session.clone(),
                p.request_id.as_ref().to_string(),
            )));
        }
    }

    let is_navigation = ev.request_id.as_ref() == ev.loader_id.as_ref()
        && ev.r#type.as_ref().is_some_and(|t| {
            *t == chromiumoxide::cdp::browser_protocol::network::ResourceType::Document
        });
    let document_id = is_navigation.then(|| ev.loader_id.as_ref().to_string());
    let source = match &paused {
        Some((p, _)) => &p.request,
        None => &ev.request,
    };
    let url = strip_fragment(&source.url);
    let headers = headers_override
        .clone()
        .unwrap_or_else(|| crate::network::headers_from_cdp(&source.headers, false));
    let is_favicon = url.ends_with("/favicon.ico")
        || redirected_from.as_ref().is_some_and(|r| r.inner.is_favicon);
    let route_for_request = route_shared
        .clone()
        .or_else(|| redirected_from.as_ref().and_then(|r| r.inner.route.clone()));
    let request = Request {
        inner: Arc::new(RequestInner {
            id: request_id.clone(),
            url,
            method: source.method.clone(),
            resource_type: to_resource_type(ev.r#type.as_ref()),
            headers,
            post_data: post_data_of(source),
            frame: Some(frame.clone()),
            document_id,
            redirected_from: redirected_from.clone(),
            page: page.self_weak.clone(),
            is_favicon,
            timestamp: *ev.timestamp.inner(),
            wall_time: *ev.wall_time.inner(),
            // The frame's chromiumoxide handle can still be unresolved right
            // after the page is created: fall back to the page's own session
            // (the one that reported the request), never to the browser
            // session, where `Network.getResponseBody` and friends do not
            // exist.
            session: Mutex::new(Some(
                frame
                    .cdp_frame_now()
                    .map(|f| f.session_id().clone())
                    .unwrap_or_else(|| page.cdp.session_id().clone()),
            )),
            route: route_for_request,
            state: Mutex::new(RequestState {
                response_end: -1.0,
                ..Default::default()
            }),
            changed: watch::channel(0).0,
        }),
    };
    if let Some(previous) = &redirected_from {
        previous.update(|s| s.redirected_to = Some(request.clone()));
        page.ext
            .redirects
            .lock()
            .unwrap()
            .push(Arc::downgrade(&previous.inner));
    }
    page.requests
        .lock()
        .unwrap()
        .insert(request_id, request.clone());
    if let Some((p, _)) = &paused {
        if route_shared.is_some() {
            let raw = crate::network::headers_from_cdp(&p.request.headers, true);
            request.update(|s| {
                if s.raw_headers.is_none() {
                    s.raw_headers = Some(raw);
                }
            });
        }
    }

    // FrameManager.requestStarted
    page.network_request_started(&request);
    if request.inner.is_favicon {
        if let Some(shared) = &route_shared {
            let shared = shared.clone();
            let browser = page.browser.clone();
            tokio::spawn(async move {
                let _ = shared.abort_raw(&browser, "aborted").await;
            });
        }
        return;
    }
    emit_on_context(page, Event::Request(request.clone()));
    if let Some(shared) = route_shared {
        state.routes_in_flight += 1;
        let route = Route::new(request, shared, page.self_weak.clone());
        if let Some(page) = page.self_weak.clone().upgrade() {
            tokio::spawn(crate::route::handle_route(page, route));
        }
    }
}

/// Emit an event on the page and on its context.
pub(crate) fn emit_on_context(page: &PageInner, event: Event) {
    page.ext.listeners.emit(event.clone());
    if let Some(context) = page.context.upgrade() {
        context.ext.listeners.emit(event.clone());
        context.ext.har_record(page, &event);
    }
}

fn response_data(
    request: &Request,
    response: &CdpResponse,
    served_from_cache: bool,
) -> ResponseData {
    let timing = match (&response.timing, served_from_cache) {
        (Some(t), false) => ResourceTiming {
            start_time: (t.request_time - request.inner.timestamp + request.inner.wall_time)
                * 1000.0,
            domain_lookup_start: t.dns_start,
            domain_lookup_end: t.dns_end,
            connect_start: t.connect_start,
            secure_connection_start: t.ssl_start,
            connect_end: t.connect_end,
            request_start: t.send_start,
            response_start: t.receive_headers_end,
            response_end: -1.0,
        },
        _ => ResourceTiming {
            start_time: request.inner.wall_time * 1000.0,
            ..Default::default()
        },
    };
    let server_addr = match (&response.remote_ip_address, response.remote_port) {
        (Some(ip), Some(port)) => Some(crate::network::RemoteAddr {
            ip_address: ip.clone(),
            port: port as u16,
        }),
        _ => None,
    };
    let security_details = response.security_details.as_ref().map(|s| SecurityDetails {
        protocol: Some(s.protocol.clone()),
        subject_name: Some(s.subject_name.clone()),
        issuer: Some(s.issuer.clone()),
        valid_from: Some(*s.valid_from.inner()),
        valid_to: Some(*s.valid_to.inner()),
    });
    ResponseData {
        url: request.url(),
        status: response.status,
        status_text: response.status_text.clone(),
        headers: headers_from_cdp(&response.headers, false),
        from_cache: response.from_disk_cache == Some(true)
            || response.from_prefetch_cache == Some(true)
            || served_from_cache,
        from_service_worker: response.from_service_worker == Some(true),
        timing,
        http_version: response.protocol.clone(),
        server_addr,
        security_details,
        raw_headers: None,
        headers_size: None,
        transfer_size: None,
        encoded_body_size: None,
        finished: false,
    }
}

/// `_createResponse`: attach the response to the request.
fn create_response(
    state: &mut NmState,
    request: &Request,
    response: &CdpResponse,
    has_extra_info: bool,
) {
    let served = state.extra_info.served_from_cache(&request.inner.id);
    let mut data = response_data(request, response, served);
    request.update(|s| {
        if let Some(headers) = &s.fulfilled_headers {
            data.raw_headers = Some(headers.clone());
            data.headers_size = Some(None);
        }
        s.response = Some(data)
    });
    state
        .extra_info
        .process_response(&request.inner.id, request, has_extra_info);
}

fn response_finished(request: &Request, end: f64) {
    request.update(|s| {
        let start = s
            .response
            .as_ref()
            .map(|r| r.timing.response_start)
            .unwrap_or(-1.0);
        s.response_end = end.max(start);
        if let Some(r) = s.response.as_mut() {
            if r.timing.request_start == -1.0 {
                r.timing.request_start = s.response_end;
            }
            r.finished = true;
        }
    });
}

fn round_ms(seconds: f64) -> f64 {
    let ms = seconds * 1000.0;
    (ms * 1000.0).round() / 1000.0
}

fn handle_redirect(
    page: &PageInner,
    state: &mut NmState,
    request: &Request,
    response: &CdpResponse,
    timestamp: f64,
    has_extra_info: bool,
) {
    create_response(state, request, response, has_extra_info);
    request.update(|s| {
        if let Some(r) = s.response.as_mut() {
            r.transfer_size = Some(None);
            r.encoded_body_size = Some(None);
        }
    });
    response_finished(request, round_ms(timestamp - request.inner.timestamp));
    delete_request(state, request);
    // requestReceivedResponse + reportRequestFinished
    if !request.inner.is_favicon {
        emit_on_context(page, Event::Response(crate::Response::new(request.clone())));
    }
    request.update(|s| s.done = true);
    if page.network_request_finished(request) && !request.inner.is_favicon {
        emit_on_context(page, Event::RequestFinished(request.clone()));
    }
}

fn delete_request(state: &mut NmState, request: &Request) {
    if let Some(route) = &request.inner.route {
        let id = route.interception_id();
        state.attempted_auth.remove(&id);
    }
}

pub(crate) fn on_request_will_be_sent_extra_info(
    page: &PageInner,
    ev: &EventRequestWillBeSentExtraInfo,
) {
    let mut state = page.ext.network.state.lock().unwrap();
    let headers = headers_from_cdp(&ev.headers, true);
    state
        .extra_info
        .request_extra_info(ev.request_id.as_ref(), headers);
}

pub(crate) fn on_response_received_extra_info(
    page: &PageInner,
    ev: &EventResponseReceivedExtraInfo,
) {
    let mut state = page.ext.network.state.lock().unwrap();
    let headers = headers_from_cdp(&ev.headers, true);
    let size = ev
        .headers_text
        .as_ref()
        .map(|t| t.len() as i64)
        .unwrap_or(0);
    state
        .extra_info
        .response_extra_info(ev.request_id.as_ref(), headers, size);
}

pub(crate) fn on_request_served_from_cache(page: &PageInner, ev: &EventRequestServedFromCache) {
    let mut state = page.ext.network.state.lock().unwrap();
    state
        .extra_info
        .served_from_cache_mark(ev.request_id.as_ref());
    // Cached requests are not paused: report them now.
    if let Some(will) = state.will_be_sent.remove(ev.request_id.as_ref()) {
        on_request(page, &mut state, &will, None);
    }
}

/// A request reported by `requestWillBeSent` while interception was
/// enabled, but never paused (e.g. interception got enabled while it was in
/// flight): report it without a route.
fn take_unpaused(page: &PageInner, state: &mut NmState, request_id: &str) -> Option<Request> {
    if let Some(r) = page.requests.lock().unwrap().get(request_id).cloned() {
        return Some(r);
    }
    let will = state.will_be_sent.remove(request_id)?;
    on_request(page, state, &will, None);
    page.requests.lock().unwrap().get(request_id).cloned()
}

pub(crate) fn on_response_received(page: &PageInner, ev: &EventResponseReceived) {
    let mut state = page.ext.network.state.lock().unwrap();
    let Some(request) = take_unpaused(page, &mut state, ev.request_id.as_ref()) else {
        return;
    };
    create_response(&mut state, &request, &ev.response, ev.has_extra_info);
    if !request.inner.is_favicon {
        emit_on_context(page, Event::Response(crate::Response::new(request.clone())));
    }
}

pub(crate) fn on_loading_finished(page: &PageInner, ev: &EventLoadingFinished) {
    let mut state = page.ext.network.state.lock().unwrap();
    state.extra_info.loading_done(ev.request_id.as_ref());
    let Some(request) = take_unpaused(page, &mut state, ev.request_id.as_ref()) else {
        return;
    };
    page.requests.lock().unwrap().remove(ev.request_id.as_ref());
    if request.existing_response().is_some() {
        let transfer = ev.encoded_data_length as i64;
        request.update(|s| {
            if let Some(r) = s.response.as_mut() {
                r.transfer_size = Some(Some(transfer));
                let headers_size = r.headers_size.flatten().unwrap_or(0);
                r.encoded_body_size = Some(Some(
                    ev.encoded_body_length
                        .map(|l| l as i64)
                        .unwrap_or(transfer - headers_size),
                ));
            }
        });
        response_finished(
            &request,
            round_ms(*ev.timestamp.inner() - request.inner.timestamp),
        );
    }
    delete_request(&mut state, &request);
    request.update(|s| s.done = true);
    if page.network_request_finished(&request) && !request.inner.is_favicon {
        emit_on_context(page, Event::RequestFinished(request.clone()));
    }
}

pub(crate) fn on_loading_failed(page: &PageInner, ev: &EventLoadingFailed) {
    let mut state = page.ext.network.state.lock().unwrap();
    state.extra_info.loading_done(ev.request_id.as_ref());
    let Some(request) = take_unpaused(page, &mut state, ev.request_id.as_ref()) else {
        return;
    };
    page.requests.lock().unwrap().remove(ev.request_id.as_ref());
    let has_response = request.existing_response().is_some();
    if has_response {
        request.update(|s| {
            if let Some(r) = s.response.as_mut() {
                r.transfer_size = Some(None);
                r.encoded_body_size = Some(None);
            }
        });
        response_finished(
            &request,
            round_ms(*ev.timestamp.inner() - request.inner.timestamp),
        );
    } else {
        let provisional = request.inner.headers.clone();
        request.update(|s| {
            if s.raw_headers.is_none() {
                s.raw_headers = Some(provisional);
            }
        });
    }
    delete_request(&mut state, &request);
    let status = request
        .inner
        .state
        .lock()
        .unwrap()
        .response
        .as_ref()
        .map(|r| r.status);
    if status == Some(204) && !request.is_navigation_request() {
        request.update(|s| s.done = true);
        if page.network_request_finished(&request) && !request.inner.is_favicon {
            emit_on_context(page, Event::RequestFinished(request.clone()));
        }
        return;
    }
    let text = if !ev.error_text.is_empty() {
        ev.error_text.clone()
    } else {
        ev.blocked_reason
            .as_ref()
            .map(|r| r.as_ref().to_string())
            .unwrap_or_default()
    };
    request.update(|s| {
        s.failure = Some(text);
        s.done = true;
    });
    let was_inflight = page.network_request_failed(&request, ev.canceled == Some(true));
    if was_inflight && !request.inner.is_favicon {
        emit_on_context(page, Event::RequestFailed(request.clone()));
    }
}

pub(crate) fn on_auth_required(page: &PageInner, ev: &EventAuthRequired) {
    let context = page.context.upgrade();
    let is_proxy = matches!(
        ev.auth_challenge.source,
        Some(chromiumoxide::cdp::browser_protocol::fetch::AuthChallengeSource::Proxy)
    );
    // Proxy credentials only answer the proxy's challenge, never the website's.
    let credentials = if is_proxy {
        context
            .as_ref()
            .and_then(|c| crate::proxy::context_credentials(c))
    } else {
        context
            .as_ref()
            .and_then(|c| c.ext.http_credentials.lock().unwrap().clone())
            .filter(|c| c.matches(&ev.request.url))
    };
    let key = format!(
        "{}:{}",
        ev.request_id.as_ref(),
        if is_proxy { "proxy" } else { "server" }
    );
    let response = {
        let mut state = page.ext.network.state.lock().unwrap();
        if state.attempted_auth.contains(&key) {
            AuthChallengeResponseResponse::CancelAuth
        } else if credentials.is_some() {
            state.attempted_auth.insert(key);
            AuthChallengeResponseResponse::ProvideCredentials
        } else {
            // Playwright answers `Default`; headless Chromium then fails the
            // navigation (`ERR_INVALID_AUTH_CREDENTIALS`) instead of showing
            // the 401 response, so cancel the challenge to get the response.
            AuthChallengeResponseResponse::CancelAuth
        }
    };
    let mut auth = AuthChallengeResponse::new(response);
    if let Some(c) = credentials {
        auth.username = Some(c.username);
        auth.password = Some(c.password);
    }
    let params = ContinueWithAuthParams::new(ev.request_id.clone(), auth);
    spawn_send(page, page.cdp.session_id().clone(), params);
}

// ----------------------------------------------------------------------
// ResponseExtraInfoTracker
// ----------------------------------------------------------------------

#[derive(Default)]
struct RequestInfo {
    request_extra: Vec<Option<Vec<Header>>>,
    response_extra: Vec<Option<(Vec<Header>, i64)>>,
    responses: Vec<Weak<RequestInner>>,
    done: bool,
    served_from_cache: bool,
}

/// Aligns responses with the raw headers of `*ExtraInfo` events, which come
/// on another channel in any order (Playwright's `ResponseExtraInfoTracker`).
#[derive(Default)]
struct ExtraInfoTracker {
    requests: HashMap<String, RequestInfo>,
}

impl ExtraInfoTracker {
    fn entry(&mut self, id: &str) -> &mut RequestInfo {
        self.requests.entry(id.to_string()).or_default()
    }

    fn request_extra_info(&mut self, id: &str, headers: Vec<Header>) {
        let info = self.entry(id);
        info.request_extra.push(Some(headers));
        let index = info.request_extra.len() - 1;
        Self::patch(info, index);
        self.check_finished(id);
    }

    fn response_extra_info(&mut self, id: &str, headers: Vec<Header>, size: i64) {
        let info = self.entry(id);
        info.response_extra.push(Some((headers, size)));
        let index = info.response_extra.len() - 1;
        Self::patch(info, index);
        self.check_finished(id);
    }

    fn served_from_cache_mark(&mut self, id: &str) {
        self.entry(id).served_from_cache = true;
    }

    fn served_from_cache(&self, id: &str) -> bool {
        self.requests.get(id).is_some_and(|i| i.served_from_cache)
    }

    fn process_response(&mut self, id: &str, request: &Request, has_extra_info: bool) {
        let served = self.served_from_cache(id);
        if !has_extra_info || served {
            let provisional = request.inner.headers.clone();
            request.update(|s| {
                if s.raw_headers.is_none() {
                    s.raw_headers = Some(provisional);
                }
                if let Some(r) = s.response.as_mut() {
                    if r.headers_size.is_none() {
                        r.headers_size = Some(None);
                    }
                    if r.raw_headers.is_none() {
                        r.raw_headers = Some(r.headers.clone());
                    }
                }
            });
            return;
        }
        let info = self.entry(id);
        info.responses.push(Arc::downgrade(&request.inner));
        let index = info.responses.len() - 1;
        Self::patch(info, index);
    }

    fn loading_done(&mut self, id: &str) {
        if let Some(info) = self.requests.get_mut(id) {
            info.done = true;
        }
        self.check_finished(id);
    }

    fn patch(info: &mut RequestInfo, index: usize) {
        let Some(request) = info
            .responses
            .get(index)
            .and_then(Weak::upgrade)
            .map(|inner| Request { inner })
        else {
            return;
        };
        if let Some(slot) = info.request_extra.get_mut(index) {
            if let Some(headers) = slot.take() {
                request.update(|s| {
                    if s.raw_headers.is_none() {
                        s.raw_headers = Some(headers);
                    }
                });
            }
        }
        if let Some(slot) = info.response_extra.get_mut(index) {
            if let Some((headers, size)) = slot.take() {
                request.update(|s| {
                    if let Some(r) = s.response.as_mut() {
                        if r.headers_size.is_none() {
                            r.headers_size = Some(Some(size));
                        }
                        if r.raw_headers.is_none() {
                            r.raw_headers = Some(headers);
                        }
                    }
                });
            }
        }
    }

    fn check_finished(&mut self, id: &str) {
        let Some(info) = self.requests.get(id) else {
            return;
        };
        if !info.done {
            return;
        }
        if info.responses.len() <= info.response_extra.len() {
            self.requests.remove(id);
        }
    }
}
