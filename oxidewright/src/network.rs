//! Network [`Request`] and [`Response`] objects.
//!
//! This is the foundation used by navigation tracking (the response of
//! `page.goto`, network idleness). Routing, request/response events and the
//! rest of the network API build on top of it.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, Weak};

use base64::Engine;
use chromiumoxide::cdp::browser_protocol::network::GetResponseBodyParams;
use tokio::sync::watch;

use crate::error::{Error, Result, ResultExt};
use crate::frame::Frame;
use crate::page::PageInner;

/// A network request issued by a page.
#[derive(Clone)]
pub struct Request {
    pub(crate) inner: Arc<RequestInner>,
}

pub(crate) struct RequestInner {
    pub(crate) id: String,
    pub(crate) url: String,
    pub(crate) method: String,
    pub(crate) resource_type: String,
    pub(crate) headers: HashMap<String, String>,
    pub(crate) post_data: Option<String>,
    pub(crate) frame: Option<Frame>,
    pub(crate) document_id: Option<String>,
    pub(crate) redirected_from: Option<Request>,
    pub(crate) page: Weak<PageInner>,
    pub(crate) state: Mutex<RequestState>,
    pub(crate) finished: watch::Sender<bool>,
}

#[derive(Default)]
pub(crate) struct RequestState {
    pub(crate) redirected_to: Option<Weak<RequestInner>>,
    pub(crate) response: Option<ResponseData>,
    pub(crate) failure: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ResponseData {
    pub(crate) url: String,
    pub(crate) status: i64,
    pub(crate) status_text: String,
    pub(crate) headers: HashMap<String, String>,
    pub(crate) from_cache: bool,
}

impl Request {
    /// The request URL.
    pub fn url(&self) -> &str {
        &self.inner.url
    }

    /// The HTTP method.
    pub fn method(&self) -> &str {
        &self.inner.method
    }

    /// Resource type as reported by the browser, lower-cased (`document`, `script`, ...).
    pub fn resource_type(&self) -> &str {
        &self.inner.resource_type
    }

    /// Request headers (names lower-cased).
    pub fn headers(&self) -> &HashMap<String, String> {
        &self.inner.headers
    }

    /// Request body, if any.
    pub fn post_data(&self) -> Option<&str> {
        self.inner.post_data.as_deref()
    }

    /// The frame that initiated the request.
    pub fn frame(&self) -> Option<Frame> {
        self.inner.frame.clone()
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
        self.inner
            .state
            .lock()
            .unwrap()
            .redirected_to
            .as_ref()
            .and_then(Weak::upgrade)
            .map(|inner| Request { inner })
    }

    /// The failure text if the request failed.
    pub fn failure(&self) -> Option<String> {
        self.inner.state.lock().unwrap().failure.clone()
    }

    /// The response, if one was received.
    pub fn response(&self) -> Option<Response> {
        if self.inner.state.lock().unwrap().response.is_some() {
            Some(Response {
                request: self.clone(),
            })
        } else {
            None
        }
    }

    /// The last request of the redirect chain starting at this request.
    pub(crate) fn final_request(&self) -> Request {
        let mut current = self.clone();
        while let Some(next) = current.redirected_to() {
            current = next;
        }
        current
    }

    /// Resolves once the request finished or failed.
    pub(crate) async fn wait_finished(&self) {
        let mut rx = self.inner.finished.subscribe();
        let _ = rx.wait_for(|finished| *finished).await;
    }

    pub(crate) fn mark_finished(&self) {
        self.inner.finished.send_replace(true);
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("method", &self.inner.method)
            .field("url", &self.inner.url)
            .finish()
    }
}

/// A response received for a [`Request`].
#[derive(Clone)]
pub struct Response {
    request: Request,
}

impl Response {
    fn data(&self) -> ResponseData {
        self.request
            .inner
            .state
            .lock()
            .unwrap()
            .response
            .clone()
            .expect("response data")
    }

    /// The response URL.
    pub fn url(&self) -> String {
        self.data().url
    }

    /// HTTP status code.
    pub fn status(&self) -> u16 {
        self.data().status as u16
    }

    /// HTTP status text.
    pub fn status_text(&self) -> String {
        self.data().status_text
    }

    /// Whether the status is in the 200-299 range (or 0, e.g. for `file://`).
    pub fn ok(&self) -> bool {
        let status = self.data().status;
        status == 0 || (200..=299).contains(&status)
    }

    /// Response headers (names lower-cased).
    pub fn headers(&self) -> HashMap<String, String> {
        self.data().headers
    }

    /// Whether the response was served from the browser cache.
    pub fn from_cache(&self) -> bool {
        self.data().from_cache
    }

    /// The request that produced this response.
    pub fn request(&self) -> Request {
        self.request.clone()
    }

    /// The frame that initiated the request.
    pub fn frame(&self) -> Option<Frame> {
        self.request.frame()
    }

    /// Wait for the response body to be received and return it.
    pub async fn body(&self) -> Result<Vec<u8>> {
        async {
            self.request.wait_finished().await;
            if let Some(failure) = self.request.failure() {
                return Err(Error::navigation(failure));
            }
            let page = self
                .request
                .inner
                .page
                .upgrade()
                .ok_or_else(Error::target_closed)?;
            let params = GetResponseBodyParams::new(self.request.inner.id.clone());
            let body = match self.request.frame().and_then(|f| f.cdp_frame_now()) {
                Some(frame) => frame.execute(params).await?.result,
                None => page.cdp.execute(params).await?.result,
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

    /// The response body as text.
    pub async fn text(&self) -> Result<String> {
        let body = self.body().await?;
        Ok(String::from_utf8_lossy(&body).into_owned())
    }

    /// The response body parsed as JSON.
    pub async fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        let body = self.body().await?;
        serde_json::from_slice(&body).map_err(Error::from)
    }

    /// Resolves once the response finished loading; returns the failure, if any.
    pub async fn finished(&self) -> Option<String> {
        self.request.wait_finished().await;
        self.request.failure()
    }
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

pub(crate) fn headers_to_map(
    headers: &chromiumoxide::cdp::browser_protocol::network::Headers,
) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let serde_json::Value::Object(obj) = headers.inner() {
        for (k, v) in obj {
            let value = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            map.insert(k.to_lowercase(), value);
        }
    }
    map
}
