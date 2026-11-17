//! `page.route_web_socket` / `context.route_web_socket` and
//! [`WebSocketRoute`], like Playwright's server
//! (`dispatchers/webSocketRouteDispatcher.ts`) as built by Patchright 1.64.3:
//!
//! * the context exposes the binding `__pwWebSocketBinding` (see
//!   [`crate::binding`]: `Runtime.addBinding` with that name, no
//!   `Runtime.enable`);
//! * the injected `webSocketMock` (Patchright's build) is added as a
//!   main-world init script and evaluated in the existing frames; it replaces
//!   `WebSocket` and defines `__pwWebSocketDispatch`, exactly what Patchright
//!   leaves in the main world while WebSockets are routed;
//! * requests to the page go through `__pwWebSocketDispatch(..)`, evaluated
//!   in the main world of the frame that created the socket.
//!
//! Nothing is installed until `route_web_socket` is called.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use base64::Engine;
use serde_json::{Value, json};

use crate::binding::{BindingError, BindingSource};
use crate::browser_context::BrowserContext;
use crate::error::{Error, Result, ResultExt};
use crate::execution_context::World;
use crate::frame::Frame;
use crate::js::{JsValue, sources};
use crate::page::{InitScript, Page, PageEvent};
use crate::url_match::UrlMatch;
use crate::websocket::WebSocketPayload;

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
type RouteFn = Arc<dyn Fn(WebSocketRoute) -> BoxFuture<()> + Send + Sync>;
type MessageFn = Arc<dyn Fn(WebSocketPayload) + Send + Sync>;
type CloseFn = Arc<dyn Fn(Option<u16>, Option<String>) + Send + Sync>;

const BINDING_NAME: &str = "__pwWebSocketBinding";

pub(crate) struct WsRouteHandler {
    url: UrlMatch,
    handler: RouteFn,
}

/// Web socket routing state of a page or a context.
#[derive(Default)]
pub(crate) struct WsRoutes {
    handlers: Mutex<Vec<Arc<WsRouteHandler>>>,
    init_script: tokio::sync::Mutex<Option<InitScript>>,
}

/// Context-wide state: the binding and the live routes.
#[derive(Default)]
pub(crate) struct WsContextState {
    binding: tokio::sync::Mutex<Option<crate::binding::ExposedBinding>>,
    routes: Mutex<HashMap<String, Weak<RouteInner>>>,
}

fn ws_base_url(base_url: Option<&str>) -> Option<String> {
    base_url.map(|b| {
        let lower = b.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            format!("ws{}", &b[4..])
        } else {
            b.to_string()
        }
    })
}

fn find_handler(
    routes: &WsRoutes,
    base_url: Option<&str>,
    url: &str,
) -> Option<Arc<WsRouteHandler>> {
    let base = ws_base_url(base_url);
    routes
        .handlers
        .lock()
        .unwrap()
        .iter()
        .find(|h| h.url.matches(base.as_deref(), url))
        .cloned()
}

// ----------------------------------------------------------------------
// WebSocketRoute
// ----------------------------------------------------------------------

struct RouteInner {
    id: String,
    url: String,
    protocols: Vec<String>,
    frame: Frame,
    on_page_message: Mutex<Option<MessageFn>>,
    on_page_close: Mutex<Option<CloseFn>>,
    on_server_message: Mutex<Option<MessageFn>>,
    on_server_close: Mutex<Option<CloseFn>>,
    connected: AtomicBool,
    gone: AtomicBool,
    /// Requests to the page, evaluated one after the other (in call order).
    queue: RequestQueue,
}

type RequestQueue =
    tokio::sync::mpsc::UnboundedSender<(Value, Option<tokio::sync::oneshot::Sender<()>>)>;

fn request_queue(frame: Frame) -> RequestQueue {
    let (tx, mut rx) =
        tokio::sync::mpsc::unbounded_channel::<(Value, Option<tokio::sync::oneshot::Sender<()>>)>();
    tokio::spawn(async move {
        while let Some((request, ack)) = rx.recv().await {
            evaluate_dispatch(&frame, request).await;
            if let Some(ack) = ack {
                let _ = ack.send(());
            }
        }
    });
    tx
}

/// A routed WebSocket, handed to the `route_web_socket` handler
/// (Playwright's `WebSocketRoute`). By default (no handler set) messages and
/// closes flow between the page and the server once
/// [`WebSocketRoute::connect_to_server`] is called; without it the socket is
/// fully mocked. Cheap to clone.
#[derive(Clone)]
pub struct WebSocketRoute {
    inner: Arc<RouteInner>,
}

/// The server side of a [`WebSocketRoute`], returned by
/// [`WebSocketRoute::connect_to_server`].
#[derive(Clone)]
pub struct WebSocketRouteServer {
    inner: Arc<RouteInner>,
}

impl std::fmt::Debug for WebSocketRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocketRoute")
            .field("url", &self.inner.url)
            .finish()
    }
}

impl std::fmt::Debug for WebSocketRouteServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocketRouteServer")
            .field("url", &self.inner.url)
            .finish()
    }
}

fn payload_to_data(message: &WebSocketPayload) -> Value {
    match message {
        WebSocketPayload::Text(s) => json!({ "data": s, "isBase64": false }),
        WebSocketPayload::Binary(b) => json!({
            "data": base64::engine::general_purpose::STANDARD.encode(b),
            "isBase64": true,
        }),
    }
}

fn data_to_payload(data: &Value) -> WebSocketPayload {
    let text = data
        .get("data")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if data.get("isBase64").and_then(Value::as_bool) == Some(true) {
        WebSocketPayload::Binary(
            base64::engine::general_purpose::STANDARD
                .decode(text)
                .unwrap_or_default(),
        )
    } else {
        WebSocketPayload::Text(text)
    }
}

impl RouteInner {
    /// `_evaluateAPIRequest`: `globalThis.__pwWebSocketDispatch(request)`.
    async fn api_request(&self, request: Value) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if self.queue.send((request, Some(tx))).is_ok() {
            let _ = rx.await;
        }
    }

    fn spawn_request(self: &Arc<Self>, request: Value) {
        let _ = self.queue.send((request, None));
    }

    fn close_page(self: &Arc<Self>, code: Option<u16>, reason: Option<String>, was_clean: bool) {
        self.spawn_request(json!({
            "id": self.id, "type": "closePage", "code": code, "reason": reason, "wasClean": was_clean,
        }));
    }

    fn close_server(self: &Arc<Self>, code: Option<u16>, reason: Option<String>, was_clean: bool) {
        self.spawn_request(json!({
            "id": self.id, "type": "closeServer", "code": code, "reason": reason, "wasClean": was_clean,
        }));
    }

    fn send_to_page(self: &Arc<Self>, data: Value) {
        self.spawn_request(json!({ "id": self.id, "type": "sendToPage", "data": data }));
    }

    fn send_to_server(self: &Arc<Self>, data: Value) {
        self.spawn_request(json!({ "id": self.id, "type": "sendToServer", "data": data }));
    }

    fn message_from_page(self: &Arc<Self>, data: Value) {
        let handler = self.on_page_message.lock().unwrap().clone();
        if let Some(handler) = handler {
            handler(data_to_payload(&data));
        } else if self.connected.load(Ordering::SeqCst) {
            self.send_to_server(data);
        }
    }

    fn message_from_server(self: &Arc<Self>, data: Value) {
        let handler = self.on_server_message.lock().unwrap().clone();
        if let Some(handler) = handler {
            handler(data_to_payload(&data));
        } else {
            self.send_to_page(data);
        }
    }

    fn page_closed(self: &Arc<Self>, code: Option<u16>, reason: Option<String>, was_clean: bool) {
        let handler = self.on_page_close.lock().unwrap().clone();
        if let Some(handler) = handler {
            handler(code, reason);
        } else {
            self.close_server(code, reason, was_clean);
        }
    }

    fn server_closed(self: &Arc<Self>, code: Option<u16>, reason: Option<String>, was_clean: bool) {
        let handler = self.on_server_close.lock().unwrap().clone();
        if let Some(handler) = handler {
            handler(code, reason);
        } else {
            self.close_page(code, reason, was_clean);
        }
    }

    /// The frame navigated or detached, or the page closed: no more
    /// communication from the mock, so pretend it was closed.
    fn context_gone(self: &Arc<Self>) {
        if self.gone.swap(true, Ordering::SeqCst) {
            return;
        }
        self.page_closed(None, None, true);
        self.server_closed(None, None, true);
    }
}

impl WebSocketRoute {
    /// URL of the WebSocket created in the page.
    pub fn url(&self) -> &str {
        &self.inner.url
    }

    /// Subprotocols requested by the page (the `protocols` argument of the
    /// `WebSocket` constructor).
    pub fn protocols(&self) -> Vec<String> {
        self.inner.protocols.clone()
    }

    /// Close the page side (default code 1000).
    pub async fn close(&self) -> Result<()> {
        self.close_with(None, None).await
    }

    /// Close the page side with a `code` and/or `reason`.
    pub async fn close_with(&self, code: Option<u16>, reason: Option<String>) -> Result<()> {
        self.inner
            .api_request(json!({
                "id": self.inner.id, "type": "closePage", "code": code, "reason": reason, "wasClean": true,
            }))
            .await;
        Ok(())
    }

    /// Connect to the actual server. Returns the server side, whose messages
    /// are forwarded to the page unless [`WebSocketRouteServer::on_message`]
    /// is set; page messages go to the server unless
    /// [`WebSocketRoute::on_message`] is set.
    pub fn connect_to_server(&self) -> Result<WebSocketRouteServer> {
        if self.inner.connected.swap(true, Ordering::SeqCst) {
            return Err(Error::invalid_argument("Already connected to the server"));
        }
        self.inner
            .spawn_request(json!({ "id": self.inner.id, "type": "connect" }));
        Ok(WebSocketRouteServer {
            inner: self.inner.clone(),
        })
    }

    /// Send a message to the page.
    pub fn send(&self, message: impl Into<WebSocketPayload>) {
        self.inner.send_to_page(payload_to_data(&message.into()));
    }

    /// Handle the messages sent by the page (they are no longer forwarded
    /// to the server).
    pub fn on_message(&self, handler: impl Fn(WebSocketPayload) + Send + Sync + 'static) {
        *self.inner.on_page_message.lock().unwrap() = Some(Arc::new(handler));
    }

    /// Handle `WebSocket.close()` in the page (the server is then no longer
    /// closed automatically). The handler gets the code and reason.
    pub fn on_close(&self, handler: impl Fn(Option<u16>, Option<String>) + Send + Sync + 'static) {
        *self.inner.on_page_close.lock().unwrap() = Some(Arc::new(handler));
    }

    /// `_afterHandle`: without a server connection, make the socket "open".
    async fn after_handle(&self) {
        if self.inner.connected.load(Ordering::SeqCst) {
            return;
        }
        self.inner
            .api_request(json!({ "id": self.inner.id, "type": "ensureOpened" }))
            .await;
    }
}

impl WebSocketRouteServer {
    /// URL of the WebSocket.
    pub fn url(&self) -> &str {
        &self.inner.url
    }

    /// Subprotocols requested by the page.
    pub fn protocols(&self) -> Vec<String> {
        self.inner.protocols.clone()
    }

    /// Close the server connection (default code 1000).
    pub async fn close(&self) -> Result<()> {
        self.close_with(None, None).await
    }

    /// Close the server connection with a `code` and/or `reason`.
    pub async fn close_with(&self, code: Option<u16>, reason: Option<String>) -> Result<()> {
        self.inner
            .api_request(json!({
                "id": self.inner.id, "type": "closeServer", "code": code, "reason": reason, "wasClean": true,
            }))
            .await;
        Ok(())
    }

    /// Send a message to the server.
    pub fn send(&self, message: impl Into<WebSocketPayload>) {
        self.inner.send_to_server(payload_to_data(&message.into()));
    }

    /// Handle the messages sent by the server (they are no longer forwarded
    /// to the page).
    pub fn on_message(&self, handler: impl Fn(WebSocketPayload) + Send + Sync + 'static) {
        *self.inner.on_server_message.lock().unwrap() = Some(Arc::new(handler));
    }

    /// Handle the server closing the connection (the page is then no longer
    /// closed automatically).
    pub fn on_close(&self, handler: impl Fn(Option<u16>, Option<String>) + Send + Sync + 'static) {
        *self.inner.on_server_close.lock().unwrap() = Some(Arc::new(handler));
    }
}

impl From<&str> for WebSocketPayload {
    fn from(s: &str) -> Self {
        WebSocketPayload::Text(s.to_string())
    }
}

impl From<String> for WebSocketPayload {
    fn from(s: String) -> Self {
        WebSocketPayload::Text(s)
    }
}

impl From<Vec<u8>> for WebSocketPayload {
    fn from(b: Vec<u8>) -> Self {
        WebSocketPayload::Binary(b)
    }
}

// ----------------------------------------------------------------------
// Installation
// ----------------------------------------------------------------------

async fn evaluate_dispatch(frame: &Frame, request: Value) {
    let expression = format!("globalThis.__pwWebSocketDispatch({request})");
    let run = async {
        let ctx = frame.context(World::Main).await?;
        let object = ctx.raw_evaluate_handle(&expression).await?;
        if let Some(id) = object.object_id {
            ctx.release_object(id).await;
        }
        Ok::<_, Error>(())
    };
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), run).await;
}

/// The binding of the mock, resolved lazily: without `Runtime.enable` the
/// binding only reaches a new document once its main-world context is
/// created (after the init scripts ran), so the mock waits for the wrapper
/// when it is called (Patchright's `credentialsPatch.ts` technique for its
/// WebAuthn mock, applied to the WebSocket mock).
const LAZY_BINDING: &str = "const binding = async (payload) => {
    while (!globalThis.__pwWebSocketBinding?.toString().startsWith('(...args) => {'))
      await new Promise(resolve => globalThis.setTimeout(resolve, 0));
    return globalThis.__pwWebSocketBinding(payload);
  };";

fn mock_script() -> String {
    let source = sources::WEB_SOCKET_MOCK.replacen(
        "const binding = globalThis.__pwWebSocketBinding;",
        LAZY_BINDING,
        1,
    );
    format!(
        "\n      (() => {{\n        const module = {{}};\n        {source}\n        (module.exports.inject())(globalThis);\n      }})();\n    "
    )
}

enum Target {
    Page(Page),
    Context(BrowserContext),
}

impl Target {
    fn context(&self) -> Option<BrowserContext> {
        match self {
            Target::Page(p) => p.context(),
            Target::Context(c) => Some(c.clone()),
        }
    }

    fn routes(&self) -> &WsRoutes {
        match self {
            Target::Page(p) => &p.inner.ext.ws_routes,
            Target::Context(c) => &c.inner.ext.ws_routes,
        }
    }
}

/// `WebSocketRouteDispatcher.install`.
async fn install(target: &Target) -> Result<()> {
    let context = target.context().ok_or_else(Error::target_closed)?;
    {
        let mut binding = context.inner.ext.ws_state.binding.lock().await;
        if binding.is_none() {
            let weak = Arc::downgrade(&context.inner);
            let exposed = crate::binding::expose_on_context(
                &context,
                BINDING_NAME.to_string(),
                crate::binding::erase(move |source: BindingSource, args: Vec<JsValue>| {
                    let weak = weak.clone();
                    async move {
                        if let Some(inner) = weak.upgrade() {
                            on_binding(BrowserContext { inner }, source, args);
                        }
                        Ok::<_, BindingError>(())
                    }
                }),
            )
            .await?;
            *binding = Some(exposed);
        }
    }
    let routes = target.routes();
    let mut script = routes.init_script.lock().await;
    if script.is_none() {
        let source = mock_script();
        let init = match target {
            Target::Page(p) => p.add_init_script(source.clone()).await?,
            Target::Context(c) => c.add_init_script(source.clone()).await?,
        };
        *script = Some(init);
        let pages = match target {
            Target::Page(p) => vec![p.clone()],
            Target::Context(c) => c.pages(),
        };
        for page in pages {
            for frame in page.frames() {
                let run = async {
                    let ctx = frame.context(World::Main).await?;
                    let object = ctx.raw_evaluate_handle(&source).await?;
                    if let Some(id) = object.object_id {
                        ctx.release_object(id).await;
                    }
                    Ok::<_, Error>(())
                };
                let _ = tokio::time::timeout(std::time::Duration::from_secs(5), run).await;
            }
        }
    }
    Ok(())
}

/// Calls of `__pwWebSocketBinding` from the mock.
fn on_binding(context: BrowserContext, source: BindingSource, args: Vec<JsValue>) {
    let payload = args
        .into_iter()
        .next()
        .map(|v| v.to_json())
        .unwrap_or_default();
    let kind = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let id = payload
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let state = &context.inner.ext.ws_state;
    if kind == "onCreate" {
        let url = payload
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let protocols: Vec<String> = payload
            .get("protocols")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|p| p.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let base_url = context.inner.options.base_url.clone();
        let handler = find_handler(&source.page.inner.ext.ws_routes, base_url.as_deref(), &url)
            .or_else(|| find_handler(&context.inner.ext.ws_routes, base_url.as_deref(), &url));
        let Some(handler) = handler else {
            let frame = source.frame.clone();
            tokio::spawn(async move {
                evaluate_dispatch(&frame, json!({ "id": id, "type": "passthrough" })).await;
            });
            return;
        };
        let inner = Arc::new(RouteInner {
            id: id.clone(),
            url,
            protocols,
            frame: source.frame.clone(),
            on_page_message: Mutex::new(None),
            on_page_close: Mutex::new(None),
            on_server_message: Mutex::new(None),
            on_server_close: Mutex::new(None),
            connected: AtomicBool::new(false),
            gone: AtomicBool::new(false),
            queue: request_queue(source.frame.clone()),
        });
        state
            .routes
            .lock()
            .unwrap()
            .insert(id, Arc::downgrade(&inner));
        watch_frame(&source.page, &inner);
        let route = WebSocketRoute { inner };
        tokio::spawn(async move {
            (handler.handler)(route.clone()).await;
            route.after_handle().await;
        });
        return;
    }
    let route = state
        .routes
        .lock()
        .unwrap()
        .get(&id)
        .and_then(Weak::upgrade);
    let Some(route) = route else {
        return;
    };
    let code = payload
        .get("code")
        .and_then(Value::as_u64)
        .map(|c| c as u16);
    let reason = payload
        .get("reason")
        .and_then(Value::as_str)
        .map(str::to_string);
    let was_clean = payload
        .get("wasClean")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let data = payload.get("data").cloned().unwrap_or_default();
    match kind {
        "onMessageFromPage" => route.message_from_page(data),
        "onMessageFromServer" => route.message_from_server(data),
        "onClosePage" => route.page_closed(code, reason, was_clean),
        "onCloseServer" => route.server_closed(code, reason, was_clean),
        _ => {}
    }
}

/// Close the route when its frame navigates or detaches, or the page closes.
fn watch_frame(page: &Page, route: &Arc<RouteInner>) {
    let mut events = page.inner.subscribe();
    // The route lives as long as its frame's document (the handler may
    // keep no reference to it).
    let route = route.clone();
    let frame_id = route.frame.id().to_string();
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            let gone = match &event {
                PageEvent::Closed => true,
                PageEvent::FrameDetached(id) => *id == frame_id,
                PageEvent::Navigated(nav) => {
                    nav.frame_id == frame_id && nav.error.is_none() && nav.new_document.is_some()
                }
                _ => false,
            };
            if gone || route.gone.load(Ordering::SeqCst) {
                route.context_gone();
                return;
            }
        }
        route.context_gone();
    });
}

fn wrap_handler<F, Fut>(handler: F) -> RouteFn
where
    F: Fn(WebSocketRoute) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    Arc::new(move |route| Box::pin(handler(route)))
}

impl Page {
    /// Route the WebSockets created by the page whose URL matches `url`
    /// (Playwright's `page.routeWebSocket`). The handler gets a
    /// [`WebSocketRoute`]; the socket is mocked unless it calls
    /// [`WebSocketRoute::connect_to_server`]. Page handlers take precedence
    /// over context ones.
    ///
    /// Stealth: same main-world footprint as Patchright while routing
    /// (`WebSocket` replaced by the mock, `__pwWebSocketDispatch` and the
    /// `__pwWebSocketBinding` binding).
    pub async fn route_web_socket<F, Fut>(&self, url: impl Into<UrlMatch>, handler: F) -> Result<()>
    where
        F: Fn(WebSocketRoute) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let routes = &self.inner.ext.ws_routes;
        routes.handlers.lock().unwrap().insert(
            0,
            Arc::new(WsRouteHandler {
                url: url.into(),
                handler: wrap_handler(handler),
            }),
        );
        install(&Target::Page(self.clone()))
            .await
            .api("page.routeWebSocket")
    }
}

impl BrowserContext {
    /// [`Page::route_web_socket`] for every page of the context
    /// (Playwright's `browserContext.routeWebSocket`).
    pub async fn route_web_socket<F, Fut>(&self, url: impl Into<UrlMatch>, handler: F) -> Result<()>
    where
        F: Fn(WebSocketRoute) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let routes = &self.inner.ext.ws_routes;
        routes.handlers.lock().unwrap().insert(
            0,
            Arc::new(WsRouteHandler {
                url: url.into(),
                handler: wrap_handler(handler),
            }),
        );
        install(&Target::Context(self.clone()))
            .await
            .api("browserContext.routeWebSocket")
    }
}
