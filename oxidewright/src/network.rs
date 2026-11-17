//! Network [`Request`] and [`Response`] objects (Playwright's
//! `server/network.ts` and `client/network.ts`).
//!
//! Requests are created by the page's network manager
//! ([`crate::network_manager`]) from `Network.*` and `Fetch.*` events. A
//! [`Response`] is a view on the request that received it.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, Weak};

use base64::Engine;
use chromiumoxide::cdp::browser_protocol::network::GetResponseBodyParams;
use chromiumoxide::cdp::browser_protocol::target::SessionId;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::error::{Error, Result, ResultExt};
use crate::frame::Frame;
use crate::page::PageInner;

/// A single HTTP header, with the name in its original case.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Header {
    /// Header name.
    pub name: String,
    /// Header value.
    pub value: String,
}

impl Header {
    /// A header from a name and a value.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Header {
            name: name.into(),
            value: value.into(),
        }
    }
}

impl<N: Into<String>, V: Into<String>> From<(N, V)> for Header {
    fn from((name, value): (N, V)) -> Self {
        Header::new(name, value)
    }
}

/// `RawHeaders.get`: all values of a header joined (`\n` for `set-cookie`).
pub(crate) fn header_get(headers: &[Header], name: &str) -> Option<String> {
    let values = header_get_all(headers, name);
    if values.is_empty() {
        return None;
    }
    let sep = if name.eq_ignore_ascii_case("set-cookie") {
        "\n"
    } else {
        ", "
    };
    Some(values.join(sep))
}

/// `RawHeaders.getAll`.
pub(crate) fn header_get_all(headers: &[Header], name: &str) -> Vec<String> {
    headers
        .iter()
        .filter(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.clone())
        .collect()
}

/// `RawHeaders.headers()`: lower-cased names, joined values.
pub(crate) fn headers_to_map(headers: &[Header]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for h in headers {
        let name = h.name.to_lowercase();
        if let std::collections::hash_map::Entry::Vacant(entry) = map.entry(name) {
            if let Some(value) = header_get(headers, entry.key()) {
                entry.insert(value);
            }
        }
    }
    map
}

/// `headersObjectToArray`: a CDP header object as a list. With `split`, values
/// containing `\n` (several headers of the same name) become several entries.
pub(crate) fn headers_from_cdp(
    headers: &chromiumoxide::cdp::browser_protocol::network::Headers,
    split: bool,
) -> Vec<Header> {
    let mut out = Vec::new();
    if let serde_json::Value::Object(obj) = headers.inner() {
        for (k, v) in obj {
            let value = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            if split {
                for part in value.split('\n') {
                    out.push(Header::new(k.clone(), part));
                }
            } else {
                out.push(Header::new(k.clone(), value));
            }
        }
    }
    out
}

/// `headersArrayToObject`.
pub(crate) fn headers_to_cdp(
    headers: &[Header],
) -> chromiumoxide::cdp::browser_protocol::network::Headers {
    let mut obj = serde_json::Map::new();
    for h in headers {
        obj.insert(h.name.clone(), serde_json::Value::String(h.value.clone()));
    }
    chromiumoxide::cdp::browser_protocol::network::Headers::new(serde_json::Value::Object(obj))
}

/// Timing of a request, in milliseconds relative to `start_time` (which is
/// a Unix timestamp in milliseconds); `-1` when not available.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceTiming {
    /// Request start, milliseconds since the Unix epoch.
    pub start_time: f64,
    /// Start of the DNS lookup.
    pub domain_lookup_start: f64,
    /// End of the DNS lookup.
    pub domain_lookup_end: f64,
    /// Start of the connection.
    pub connect_start: f64,
    /// Start of the TLS handshake.
    pub secure_connection_start: f64,
    /// Connection established.
    pub connect_end: f64,
    /// Request sent.
    pub request_start: f64,
    /// First byte of the response received.
    pub response_start: f64,
    /// Last byte of the response received.
    pub response_end: f64,
}

impl Default for ResourceTiming {
    fn default() -> Self {
        ResourceTiming {
            start_time: 0.0,
            domain_lookup_start: -1.0,
            domain_lookup_end: -1.0,
            connect_start: -1.0,
            secure_connection_start: -1.0,
            connect_end: -1.0,
            request_start: -1.0,
            response_start: -1.0,
            response_end: -1.0,
        }
    }
}

/// Sizes of a request and its response, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestSizes {
    /// Size of the request body (POST data).
    pub request_body_size: u64,
    /// Size of the request headers, including the request line.
    pub request_headers_size: u64,
    /// Size of the received (encoded) response body.
    pub response_body_size: u64,
    /// Size of the response headers, including the status line.
    pub response_headers_size: u64,
}

/// Remote address of a response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAddr {
    /// IPv4 or IPv6 address of the server.
    pub ip_address: String,
    /// Port of the server.
    pub port: u16,
}

/// TLS details of a response.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityDetails {
    /// Protocol, e.g. `TLS 1.3`.
    pub protocol: Option<String>,
    /// Subject name of the certificate.
    pub subject_name: Option<String>,
    /// Issuer of the certificate.
    pub issuer: Option<String>,
    /// Unix timestamp (seconds) of the start of validity.
    pub valid_from: Option<f64>,
    /// Unix timestamp (seconds) of the end of validity.
    pub valid_to: Option<f64>,
}

/// A network request issued by a page. Cheap to clone.
#[derive(Clone)]
pub struct Request {
    pub(crate) inner: Arc<RequestInner>,
}

impl PartialEq for Request {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

pub(crate) struct RequestInner {
    /// `Network.RequestId`.
    pub(crate) id: String,
    pub(crate) url: String,
    pub(crate) method: String,
    pub(crate) resource_type: String,
    /// Provisional headers.
    pub(crate) headers: Vec<Header>,
    pub(crate) post_data: Option<Vec<u8>>,
    pub(crate) frame: Option<Frame>,
    pub(crate) document_id: Option<String>,
    pub(crate) redirected_from: Option<Request>,
    pub(crate) page: Weak<PageInner>,
    pub(crate) is_favicon: bool,
    /// `requestWillBeSent.timestamp` (monotonic seconds).
    pub(crate) timestamp: f64,
    /// `requestWillBeSent.wallTime` (seconds since the epoch).
    pub(crate) wall_time: f64,
    /// Session to read the body from (OOPIF navigations move to the child).
    pub(crate) session: Mutex<Option<SessionId>>,
    /// The route of the first request of the redirect chain, if intercepted.
    pub(crate) route: Option<Arc<crate::route::RouteShared>>,
    pub(crate) state: Mutex<RequestState>,
    /// Bumped on every state change; waiters re-check the state.
    pub(crate) changed: watch::Sender<u64>,
}

/// Overrides applied by `route.continue_` / `route.fallback`.
#[derive(Default, Clone, Debug)]
pub(crate) struct Overrides {
    pub(crate) url: Option<String>,
    pub(crate) method: Option<String>,
    pub(crate) headers: Option<Vec<Header>>,
    pub(crate) post_data: Option<Vec<u8>>,
}

#[derive(Default)]
pub(crate) struct RequestState {
    /// Strong link (like Playwright); the cycle with `redirected_from` is
    /// broken when the page closes.
    pub(crate) redirected_to: Option<Request>,
    pub(crate) response: Option<ResponseData>,
    pub(crate) failure: Option<String>,
    /// `None` until known; then the raw headers (or the provisional ones).
    pub(crate) raw_headers: Option<Vec<Header>>,
    /// Finished or failed.
    pub(crate) done: bool,
    pub(crate) response_end: f64,
    pub(crate) overrides: Overrides,
    /// Body given to `route.fulfill` (served by `response.body()`).
    pub(crate) body_override: Option<Vec<u8>>,
    /// The route fulfilled the request (no network access to read a body).
    pub(crate) fulfilled: bool,
    /// Headers given to `route.fulfill` (the browser does not report
    /// `set-cookie` of fulfilled responses).
    pub(crate) fulfilled_headers: Option<Vec<Header>>,
    /// The page or context went away: waiters fail.
    pub(crate) closed: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ResponseData {
    pub(crate) url: String,
    pub(crate) status: i64,
    pub(crate) status_text: String,
    pub(crate) headers: Vec<Header>,
    pub(crate) from_cache: bool,
    pub(crate) from_service_worker: bool,
    pub(crate) timing: ResourceTiming,
    pub(crate) http_version: Option<String>,
    pub(crate) server_addr: Option<RemoteAddr>,
    pub(crate) security_details: Option<SecurityDetails>,
    /// `None` until known.
    pub(crate) raw_headers: Option<Vec<Header>>,
    /// `None` until known; `Some(None)` = not available.
    pub(crate) headers_size: Option<Option<i64>>,
    pub(crate) transfer_size: Option<Option<i64>>,
    pub(crate) encoded_body_size: Option<Option<i64>>,
    pub(crate) finished: bool,
}

impl Request {
    pub(crate) fn notify(&self) {
        self.inner.changed.send_modify(|v| *v += 1);
    }

    pub(crate) fn update<R>(&self, f: impl FnOnce(&mut RequestState) -> R) -> R {
        let r = f(&mut self.inner.state.lock().unwrap());
        self.notify();
        r
    }

    /// Wait until `f` returns `Some`, failing when the page is closed.
    pub(crate) async fn wait_state<T>(&self, f: impl Fn(&RequestState) -> Option<T>) -> Result<T> {
        let mut rx = self.inner.changed.subscribe();
        loop {
            {
                let state = self.inner.state.lock().unwrap();
                if let Some(v) = f(&state) {
                    return Ok(v);
                }
                // A finished request leaves the page's map, so `closed` is not
                // set for it on close: check the page too.
                if state.closed || self.inner.page.upgrade().is_none_or(|p| p.is_closed()) {
                    return Err(Error::target_closed());
                }
            }
            // The sender lives in the request itself, so `changed()` never
            // fails: re-check periodically in case the page goes away.
            tokio::select! {
                changed = rx.changed() => if changed.is_err() {
                    return Err(Error::target_closed());
                },
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
            }
        }
    }

    /// The request URL (with the overrides of a `route.continue_` /
    /// `route.fallback`, if any).
    pub fn url(&self) -> String {
        let state = self.inner.state.lock().unwrap();
        state
            .overrides
            .url
            .clone()
            .unwrap_or_else(|| self.inner.url.clone())
    }

    /// The HTTP method.
    pub fn method(&self) -> String {
        let state = self.inner.state.lock().unwrap();
        state
            .overrides
            .method
            .clone()
            .unwrap_or_else(|| self.inner.method.clone())
    }

    /// Resource type as rendered by the browser: `document`, `stylesheet`,
    /// `image`, `media`, `font`, `script`, `texttrack`, `xhr`, `fetch`,
    /// `eventsource`, `websocket`, `manifest`, `other`, ...
    pub fn resource_type(&self) -> &str {
        &self.inner.resource_type
    }

    /// The request body as text (lossy UTF-8).
    pub fn post_data(&self) -> Option<String> {
        self.post_data_buffer()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    /// The request body.
    pub fn post_data_buffer(&self) -> Option<Vec<u8>> {
        let state = self.inner.state.lock().unwrap();
        state
            .overrides
            .post_data
            .clone()
            .or_else(|| self.inner.post_data.clone())
    }

    /// The request body parsed as JSON (or as a form when the content type is
    /// `application/x-www-form-urlencoded`).
    pub fn post_data_json<T: serde::de::DeserializeOwned>(&self) -> Result<Option<T>> {
        let Some(data) = self.post_data() else {
            return Ok(None);
        };
        if data.is_empty() {
            return Ok(None);
        }
        let content_type = self
            .headers()
            .get("content-type")
            .cloned()
            .unwrap_or_default();
        if content_type.contains("application/x-www-form-urlencoded") {
            let mut map = serde_json::Map::new();
            for (k, v) in url::form_urlencoded::parse(data.as_bytes()) {
                map.insert(k.into_owned(), serde_json::Value::String(v.into_owned()));
            }
            return serde_json::from_value(serde_json::Value::Object(map))
                .map(Some)
                .map_err(Error::from);
        }
        serde_json::from_str(&data).map(Some).map_err(|_| {
            Error::invalid_argument(format!("POST data is not a valid JSON object: {data}"))
        })
    }

    pub(crate) fn headers_array_now(&self) -> Vec<Header> {
        let state = self.inner.state.lock().unwrap();
        state
            .overrides
            .headers
            .clone()
            .unwrap_or_else(|| self.inner.headers.clone())
    }

    /// Request headers with lower-cased names. These are the headers known
    /// when the request was issued (the browser may add more, see
    /// [`Request::all_headers`]).
    pub fn headers(&self) -> HashMap<String, String> {
        headers_to_map(&self.headers_array_now())
    }

    async fn actual_headers(&self) -> Result<Vec<Header>> {
        if let Some(h) = self.inner.state.lock().unwrap().overrides.headers.clone() {
            return Ok(h);
        }
        self.wait_state(|s| s.raw_headers.clone()).await
    }

    /// All request headers as sent over the network (including the ones
    /// added by the browser, like cookies), with lower-cased names.
    pub async fn all_headers(&self) -> Result<HashMap<String, String>> {
        Ok(headers_to_map(&self.actual_headers().await?))
    }

    /// All request headers as a list, with names in their original case.
    pub async fn headers_array(&self) -> Result<Vec<Header>> {
        self.actual_headers().await
    }

    /// The value of a header (case-insensitive); values of repeated headers
    /// are joined.
    pub async fn header_value(&self, name: &str) -> Result<Option<String>> {
        Ok(header_get(&self.actual_headers().await?, name))
    }

    /// Wait for the response. Resolves to `None` if the request failed.
    pub async fn response(&self) -> Result<Option<Response>> {
        self.wait_state(|s| {
            if s.response.is_some() {
                Some(true)
            } else if s.failure.is_some() || s.done {
                Some(false)
            } else {
                None
            }
        })
        .await
        .map(|has| has.then(|| Response::new(self.clone())))
        .api("request.response")
    }

    /// The response if it was already received.
    pub fn existing_response(&self) -> Option<Response> {
        if self.inner.state.lock().unwrap().response.is_some() {
            Some(Response::new(self.clone()))
        } else {
            None
        }
    }

    /// The frame that initiated the request.
    pub fn frame(&self) -> Option<Frame> {
        self.inner.frame.clone()
    }

    /// The page that issued the request.
    pub fn page(&self) -> Option<crate::Page> {
        self.inner.page.upgrade().map(crate::Page::from_inner)
    }

    /// Whether this request drives a frame navigation.
    pub fn is_navigation_request(&self) -> bool {
        self.inner.document_id.is_some()
    }

    /// The request that redirected to this one.
    pub fn redirected_from(&self) -> Option<Request> {
        self.inner.redirected_from.clone()
    }

    /// The request this one redirected to.
    pub fn redirected_to(&self) -> Option<Request> {
        self.inner.state.lock().unwrap().redirected_to.clone()
    }

    /// The failure text (e.g. `net::ERR_FAILED`) if the request failed.
    pub fn failure(&self) -> Option<String> {
        self.inner.state.lock().unwrap().failure.clone()
    }

    /// Resource timing; complete once the response finished.
    pub fn timing(&self) -> ResourceTiming {
        let state = self.inner.state.lock().unwrap();
        let mut timing = state
            .response
            .as_ref()
            .map(|r| r.timing)
            .unwrap_or_default();
        if state.response.is_none() {
            timing.start_time = self.inner.wall_time * 1000.0;
        }
        timing.response_end = state.response_end;
        if timing.response_start == -1.0 && state.response_end != -1.0 {
            timing.response_start = state.response_end;
        }
        timing
    }

    /// Request and response sizes. Fails for requests without a response.
    pub async fn sizes(&self) -> Result<RequestSizes> {
        async {
            let Some(response) = self.response().await? else {
                return Err(Error::invalid_argument(
                    "Unable to fetch sizes for failed request",
                ));
            };
            response.sizes().await
        }
        .await
        .api("request.sizes")
    }

    /// Service worker requests are not reported (always `false`): requests
    /// of pages are attributed to their frame.
    pub fn is_service_worker_request(&self) -> bool {
        false
    }

    /// The last request of the redirect chain starting at this request.
    pub(crate) fn final_request(&self) -> Request {
        let mut current = self.clone();
        while let Some(next) = current.redirected_to() {
            current = next;
        }
        current
    }

    pub(crate) fn body_size(&self) -> u64 {
        self.post_data_buffer().map(|b| b.len() as u64).unwrap_or(0)
    }

    async fn request_headers_size(&self) -> Result<u64> {
        let headers = self.actual_headers().await?;
        let path = url::Url::parse(&self.url())
            .map(|u| u.path().len())
            .unwrap_or(0);
        let mut result = 4 + self.method().len() + path + 8;
        result += headers_size(&headers);
        Ok(result as u64)
    }
}

fn headers_size(headers: &[Header]) -> usize {
    headers
        .iter()
        .map(|h| h.name.len() + h.value.len() + 4)
        .sum()
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("method", &self.inner.method)
            .field("url", &self.inner.url)
            .finish()
    }
}

/// A response received for a [`Request`]. Cheap to clone.
#[derive(Clone)]
pub struct Response {
    request: Request,
}

impl PartialEq for Response {
    fn eq(&self, other: &Self) -> bool {
        self.request == other.request
    }
}

impl Response {
    pub(crate) fn new(request: Request) -> Self {
        Response { request }
    }

    fn data(&self) -> ResponseData {
        self.request
            .inner
            .state
            .lock()
            .unwrap()
            .response
            .clone()
            .unwrap_or_default()
    }

    /// The response URL.
    pub fn url(&self) -> String {
        self.data().url
    }

    /// HTTP status code.
    pub fn status(&self) -> u16 {
        self.data().status as u16
    }

    /// HTTP status text, e.g. `OK`.
    pub fn status_text(&self) -> String {
        self.data().status_text
    }

    /// Whether the status is in the 200-299 range (or 0, e.g. for `file://`).
    pub fn ok(&self) -> bool {
        let status = self.data().status;
        status == 0 || (200..=299).contains(&status)
    }

    pub(crate) fn headers_array_now(&self) -> Vec<Header> {
        self.data().headers
    }

    /// Response headers with lower-cased names, as received by the renderer
    /// (see [`Response::all_headers`] for the raw ones).
    pub fn headers(&self) -> HashMap<String, String> {
        headers_to_map(&self.data().headers)
    }

    async fn actual_headers(&self) -> Result<Vec<Header>> {
        self.request
            .wait_state(|s| s.response.as_ref().and_then(|r| r.raw_headers.clone()))
            .await
    }

    /// All response headers as received over the network, lower-cased names.
    pub async fn all_headers(&self) -> Result<HashMap<String, String>> {
        Ok(headers_to_map(&self.actual_headers().await?))
    }

    /// All response headers as a list, names in their original case.
    pub async fn headers_array(&self) -> Result<Vec<Header>> {
        self.actual_headers().await
    }

    /// The value of a header; values of repeated headers are joined
    /// (`set-cookie` values with `\n`).
    pub async fn header_value(&self, name: &str) -> Result<Option<String>> {
        Ok(header_get(&self.actual_headers().await?, name))
    }

    /// All values of a header.
    pub async fn header_values(&self, name: &str) -> Result<Vec<String>> {
        Ok(header_get_all(&self.actual_headers().await?, name))
    }

    /// Whether the response was served from the browser cache.
    pub fn from_cache(&self) -> bool {
        self.data().from_cache
    }

    /// Whether the response was served by a service worker.
    pub fn from_service_worker(&self) -> bool {
        self.data().from_service_worker
    }

    /// The request that produced this response.
    pub fn request(&self) -> Request {
        self.request.clone()
    }

    /// The frame that initiated the request.
    pub fn frame(&self) -> Option<Frame> {
        self.request.frame()
    }

    /// Server IP address and port.
    pub async fn server_addr(&self) -> Result<Option<RemoteAddr>> {
        Ok(self.data().server_addr)
    }

    /// TLS details (HTTPS responses only).
    pub async fn security_details(&self) -> Result<Option<SecurityDetails>> {
        Ok(self.data().security_details)
    }

    /// The HTTP version, e.g. `HTTP/1.1` or `HTTP/2.0`.
    pub async fn http_version(&self) -> Result<String> {
        let version = self.data().http_version.unwrap_or_default();
        Ok(match version.as_str() {
            "" | "http/1.1" => "HTTP/1.1".to_string(),
            "h2" => "HTTP/2.0".to_string(),
            other => other.to_string(),
        })
    }

    /// Wait until the response body finished loading (or the request failed).
    pub async fn finished(&self) -> Result<()> {
        self.request
            .wait_state(|s| {
                (s.done || s.response.as_ref().is_some_and(|r| r.finished)).then_some(())
            })
            .await
            .api("response.finished")
    }

    /// Wait for the response body and return it.
    pub async fn body(&self) -> Result<Vec<u8>> {
        async {
            self.finished().await?;
            let data = self.data();
            if (300..=399).contains(&data.status) {
                return Err(Error::invalid_argument(
                    "Response body is unavailable for redirect responses",
                ));
            }
            let (body_override, fulfilled) = {
                let state = self.request.inner.state.lock().unwrap();
                (state.body_override.clone(), state.fulfilled)
            };
            if let Some(body) = body_override {
                return Ok(body);
            }
            let page = self
                .request
                .inner
                .page
                .upgrade()
                .ok_or_else(Error::target_closed)?;
            let session = self.request.inner.session.lock().unwrap().clone();
            let browser = page.browser.upgrade().ok_or_else(Error::target_closed)?;
            let params = GetResponseBodyParams::new(self.request.inner.id.clone());
            let body = match browser.execute_with_session(params, session).await {
                Ok(body) => body,
                Err(err) if fulfilled => {
                    tracing::debug!("body of a fulfilled response unavailable: {err}");
                    return Ok(Vec::new());
                }
                Err(Error::Protocol { source, .. }) => {
                    return Err(Error::evaluation(format!(
                        "{source}\nResponse body is not available for a response that was navigated away from. Read response.body() before triggering any navigation."
                    )));
                }
                Err(err) => return Err(err),
            };
            if body.base64_encoded {
                base64::engine::general_purpose::STANDARD
                    .decode(body.body)
                    .map_err(|e| Error::serialization(e.to_string()))
            } else {
                Ok(body.body.into_bytes())
            }
        }
        .await
        .api("response.body")
    }

    /// The response body as text (lossy UTF-8).
    pub async fn text(&self) -> Result<String> {
        let body = self.body().await?;
        Ok(String::from_utf8_lossy(&body).into_owned())
    }

    /// The response body parsed as JSON.
    pub async fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        let body = self.body().await?;
        serde_json::from_slice(&body).map_err(Error::from)
    }

    /// Request and response sizes.
    pub async fn sizes(&self) -> Result<RequestSizes> {
        let request_headers_size = self.request.request_headers_size().await?;
        let (headers_size, raw) = self
            .request
            .wait_state(|s| {
                let r = s.response.as_ref()?;
                let size = r.headers_size?;
                let raw = r.raw_headers.clone()?;
                Some((size, raw))
            })
            .await?;
        let data = self.data();
        let response_headers_size = match headers_size {
            Some(size) => size as u64,
            None => (4 + 8 + 3 + data.status_text.len() + headers_size_of(&raw) + 2) as u64,
        };
        let encoded = self
            .request
            .wait_state(|s| s.response.as_ref().and_then(|r| r.encoded_body_size))
            .await?;
        let response_body_size = match encoded {
            Some(size) => size.max(0) as u64,
            None => header_get(&raw, "content-length")
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or(0),
        };
        Ok(RequestSizes {
            request_body_size: self.request.body_size(),
            request_headers_size,
            response_body_size,
            response_headers_size,
        })
    }
}

fn headers_size_of(headers: &[Header]) -> usize {
    headers_size(headers)
}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let data = self.data();
        f.debug_struct("Response")
            .field("status", &data.status)
            .field("url", &data.url)
            .finish()
    }
}

/// Playwright's `STATUS_TEXTS`.
pub(crate) fn status_text(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        101 => "Switching Protocols",
        102 => "Processing",
        103 => "Early Hints",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        203 => "Non-Authoritative Information",
        204 => "No Content",
        205 => "Reset Content",
        206 => "Partial Content",
        207 => "Multi-Status",
        208 => "Already Reported",
        226 => "IM Used",
        300 => "Multiple Choices",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        305 => "Use Proxy",
        306 => "Switch Proxy",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Payload Too Large",
        414 => "URI Too Long",
        415 => "Unsupported Media Type",
        416 => "Range Not Satisfiable",
        417 => "Expectation Failed",
        418 => "I'm a teapot",
        421 => "Misdirected Request",
        422 => "Unprocessable Entity",
        423 => "Locked",
        424 => "Failed Dependency",
        425 => "Too Early",
        426 => "Upgrade Required",
        428 => "Precondition Required",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        451 => "Unavailable For Legal Reasons",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        506 => "Variant Also Negotiates",
        507 => "Insufficient Storage",
        508 => "Loop Detected",
        510 => "Not Extended",
        511 => "Network Authentication Required",
        _ => "Unknown",
    }
}

/// `mergeHeaders`: later lists override earlier ones (case-insensitive),
/// keeping the last spelling of each name.
pub(crate) fn merge_headers(lists: &[&[Header]]) -> Vec<Header> {
    let mut order: Vec<String> = Vec::new();
    let mut values: HashMap<String, (String, String)> = HashMap::new();
    for list in lists {
        for h in *list {
            let lower = h.name.to_lowercase();
            if !values.contains_key(&lower) {
                order.push(lower.clone());
            }
            values.insert(lower, (h.name.clone(), h.value.clone()));
        }
    }
    order
        .into_iter()
        .filter_map(|k| values.remove(&k))
        .map(|(name, value)| Header { name, value })
        .collect()
}

const FORBIDDEN_HEADER_NAMES: &[&str] = &[
    "accept-charset",
    "accept-encoding",
    "access-control-request-headers",
    "access-control-request-method",
    "connection",
    "content-length",
    "cookie",
    "date",
    "dnt",
    "expect",
    "host",
    "keep-alive",
    "origin",
    "referer",
    "set-cookie",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "via",
];

fn is_forbidden_header(name: &str, value: &str) -> bool {
    let lower = name.to_lowercase();
    if FORBIDDEN_HEADER_NAMES.contains(&lower.as_str()) {
        return true;
    }
    if lower.starts_with("proxy-") || lower.starts_with("sec-") {
        return true;
    }
    if lower == "x-http-method" || lower == "x-http-method-override" || lower == "x-method-override"
    {
        let v = value.to_uppercase();
        if v == "CONNECT" || v == "TRACE" || v == "TRACK" {
            return true;
        }
    }
    false
}

/// `applyHeadersOverrides`: forbidden headers can't be overridden and are
/// kept from the original request.
pub(crate) fn apply_headers_overrides(original: &[Header], overrides: &[Header]) -> Vec<Header> {
    let forbidden: Vec<Header> = original
        .iter()
        .filter(|h| is_forbidden_header(&h.name, &h.value))
        .cloned()
        .collect();
    let allowed: Vec<Header> = overrides
        .iter()
        .filter(|h| !is_forbidden_header(&h.name, &h.value))
        .cloned()
        .collect();
    merge_headers(&[&allowed, &forbidden])
}

/// A MIME type for a file path (`getMimeTypeForPath`).
pub(crate) fn mime_type_for_path(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?.to_lowercase();
    Some(match ext.as_str() {
        "html" | "htm" => "text/html",
        "js" | "mjs" => "application/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "txt" => "text/plain",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "zip" => "application/zip",
        "csv" => "text/csv",
        _ => return None,
    })
}
