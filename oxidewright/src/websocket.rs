//! WebSockets opened by a page (`websocket` event), reported from the
//! `Network.webSocket*` events.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use chromiumoxide::cdp::browser_protocol::network::{
    EventWebSocketClosed, EventWebSocketCreated, EventWebSocketFrameError,
    EventWebSocketFrameReceived, EventWebSocketFrameSent,
};

use crate::error::{Error, Result};
use crate::events::Event;
use crate::page::PageInner;

/// The payload of a WebSocket frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebSocketPayload {
    /// A text frame.
    Text(String),
    /// A binary frame.
    Binary(Vec<u8>),
}

impl WebSocketPayload {
    /// The payload as text (binary frames are decoded lossily).
    pub fn as_text(&self) -> String {
        match self {
            WebSocketPayload::Text(s) => s.clone(),
            WebSocketPayload::Binary(b) => String::from_utf8_lossy(b).into_owned(),
        }
    }
}

/// An event of a [`WebSocket`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WebSocketEvent {
    /// A frame was sent (`framesent`).
    FrameSent(WebSocketPayload),
    /// A frame was received (`framereceived`).
    FrameReceived(WebSocketPayload),
    /// The socket errored (`socketerror`).
    SocketError(String),
    /// The socket closed (`close`).
    Close,
}

type SocketHandler = Arc<dyn Fn(WebSocketEvent) + Send + Sync>;

/// A WebSocket of a page. Cheap to clone.
#[derive(Clone)]
pub struct WebSocket {
    inner: Arc<WebSocketInner>,
}

struct WebSocketInner {
    url: String,
    closed: AtomicBool,
    handlers: Mutex<Vec<SocketHandler>>,
    waiters: Mutex<Vec<tokio::sync::mpsc::UnboundedSender<WebSocketEvent>>>,
}

impl fmt::Debug for WebSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebSocket")
            .field("url", &self.inner.url)
            .finish()
    }
}

impl PartialEq for WebSocket {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl WebSocket {
    /// The URL of the socket.
    pub fn url(&self) -> &str {
        &self.inner.url
    }

    /// Whether the socket closed.
    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst)
    }

    /// Call `handler` for every event of the socket.
    pub fn on(&self, handler: impl Fn(WebSocketEvent) + Send + Sync + 'static) {
        self.inner.handlers.lock().unwrap().push(Arc::new(handler));
    }

    /// Call `handler` for sent frames.
    pub fn on_frame_sent(&self, handler: impl Fn(WebSocketPayload) + Send + Sync + 'static) {
        self.on(move |e| {
            if let WebSocketEvent::FrameSent(p) = e {
                handler(p)
            }
        })
    }

    /// Call `handler` for received frames.
    pub fn on_frame_received(&self, handler: impl Fn(WebSocketPayload) + Send + Sync + 'static) {
        self.on(move |e| {
            if let WebSocketEvent::FrameReceived(p) = e {
                handler(p)
            }
        })
    }

    /// Call `handler` when the socket closes.
    pub fn on_close(&self, handler: impl Fn() + Send + Sync + 'static) {
        self.on(move |e| {
            if e == WebSocketEvent::Close {
                handler()
            }
        })
    }

    /// Call `handler` on socket errors.
    pub fn on_socket_error(&self, handler: impl Fn(String) + Send + Sync + 'static) {
        self.on(move |e| {
            if let WebSocketEvent::SocketError(m) = e {
                handler(m)
            }
        })
    }

    /// Wait for the next event matching `predicate` (Playwright's
    /// `webSocket.waitForEvent`): fails on socket close/error unless waited
    /// for, and after `timeout_ms` (0 = no timeout).
    /// The subscription starts when this method is called.
    pub fn wait_for_event(
        &self,
        predicate: impl Fn(&WebSocketEvent) -> bool + Send + Sync + 'static,
        timeout_ms: u64,
    ) -> impl std::future::Future<Output = Result<WebSocketEvent>> + Send + 'static {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        self.inner.waiters.lock().unwrap().push(tx);
        async move {
            let wait = async {
                loop {
                    let Some(event) = rx.recv().await else {
                        return Err(Error::target_closed());
                    };
                    if predicate(&event) {
                        return Ok(event);
                    }
                    match event {
                        WebSocketEvent::Close => {
                            return Err(Error::target_closed_with("Socket closed"));
                        }
                        WebSocketEvent::SocketError(_) => {
                            return Err(Error::target_closed_with("Socket error"));
                        }
                        _ => {}
                    }
                }
            };
            let result = if timeout_ms == 0 {
                wait.await
            } else {
                match tokio::time::timeout(Duration::from_millis(timeout_ms), wait).await {
                    Ok(r) => r,
                    Err(_) => Err(Error::timeout(format!(
                        "Timeout {timeout_ms}ms exceeded while waiting for event"
                    ))),
                }
            };
            result.map_err(|e| e.with_api("webSocket.waitForEvent"))
        }
    }

    fn emit(&self, event: WebSocketEvent) {
        if event == WebSocketEvent::Close {
            self.inner.closed.store(true, Ordering::SeqCst);
        }
        self.inner
            .waiters
            .lock()
            .unwrap()
            .retain(|tx| tx.send(event.clone()).is_ok());
        let handlers = self.inner.handlers.lock().unwrap().clone();
        for handler in handlers {
            handler(event.clone());
        }
    }
}

fn payload(opcode: f64, data: &str) -> Option<WebSocketPayload> {
    if opcode == 1.0 {
        Some(WebSocketPayload::Text(data.to_string()))
    } else if opcode == 2.0 {
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .ok()
            .map(WebSocketPayload::Binary)
    } else {
        None
    }
}

fn find(page: &PageInner, id: &str) -> Option<WebSocket> {
    page.ext.websockets.lock().unwrap().get(id).cloned()
}

/// Dispatch a socket event through the page's handler task, so that socket
/// handlers run in order with page handlers.
fn dispatch(page: &PageInner, socket: WebSocket, event: WebSocketEvent) {
    page.ext.listeners.run(Box::new(move || socket.emit(event)));
}

pub(crate) fn on_created(page: &PageInner, ev: &EventWebSocketCreated) {
    let url = match ev.url.find('#') {
        Some(i) => ev.url[..i].to_string(),
        None => ev.url.clone(),
    };
    let socket = WebSocket {
        inner: Arc::new(WebSocketInner {
            url,
            closed: AtomicBool::new(false),
            handlers: Mutex::new(Vec::new()),
            waiters: Mutex::new(Vec::new()),
        }),
    };
    page.ext
        .websockets
        .lock()
        .unwrap()
        .insert(ev.request_id.as_ref().to_string(), socket.clone());
    page.ext.listeners.emit(Event::WebSocket(socket));
}

pub(crate) fn on_frame_sent(page: &PageInner, ev: &EventWebSocketFrameSent) {
    if let (Some(socket), Some(p)) = (
        find(page, ev.request_id.as_ref()),
        payload(ev.response.opcode, &ev.response.payload_data),
    ) {
        dispatch(page, socket, WebSocketEvent::FrameSent(p));
    }
}

pub(crate) fn on_frame_received(page: &PageInner, ev: &EventWebSocketFrameReceived) {
    if let (Some(socket), Some(p)) = (
        find(page, ev.request_id.as_ref()),
        payload(ev.response.opcode, &ev.response.payload_data),
    ) {
        dispatch(page, socket, WebSocketEvent::FrameReceived(p));
    }
}

pub(crate) fn on_frame_error(page: &PageInner, ev: &EventWebSocketFrameError) {
    if let Some(socket) = find(page, ev.request_id.as_ref()) {
        dispatch(
            page,
            socket,
            WebSocketEvent::SocketError(ev.error_message.clone()),
        );
    }
}

pub(crate) fn on_closed(page: &PageInner, ev: &EventWebSocketClosed) {
    let socket = page
        .ext
        .websockets
        .lock()
        .unwrap()
        .remove(ev.request_id.as_ref());
    if let Some(socket) = socket {
        dispatch(page, socket, WebSocketEvent::Close);
    }
}

/// The page closed: close its sockets.
pub(crate) fn close_all(page: &PageInner) {
    let sockets: Vec<WebSocket> = page
        .ext
        .websockets
        .lock()
        .unwrap()
        .drain()
        .map(|(_, s)| s)
        .collect();
    for socket in sockets {
        socket.emit(WebSocketEvent::Close);
    }
}
