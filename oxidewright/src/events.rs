//! Page and context events: closure handlers (`page.on_request(..)`) and
//! one-shot waiters (`page.wait_for_request(..)`, `page.expect_popup(..)`),
//! mirroring Playwright's event emitters and `client/waiter.ts`.
//!
//! Handlers run on a dedicated task per page/context, in emission order, never
//! while internal locks are held, so they may call back into the API (spawn a
//! task for long `async` work, e.g. `dialog.accept()`).

use std::fmt;
use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::console::{ConsoleMessage, WebError};
use crate::dialog::Dialog;
use crate::download::Download;
use crate::error::{Error, Result};
use crate::file_chooser::FileChooser;
use crate::frame::Frame;
use crate::network::{Request, Response};
use crate::page::Page;
use crate::progress::TimeoutSettings;
use crate::websocket::WebSocket;
use crate::worker::Worker;
use crate::{BrowserContext, UrlMatch};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// An event emitted by a [`Page`] or a [`BrowserContext`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Event {
    /// The page closed (`close`).
    Close(Page),
    /// The context closed (`close` on a context).
    ContextClose(BrowserContext),
    /// `console.*` was called in the page (requires the
    /// `console_events` context option, see [`crate::NewContext::console_events`]).
    Console(ConsoleMessage),
    /// The page crashed (`crash`).
    Crash(Page),
    /// A JavaScript dialog opened (`dialog`).
    Dialog(Dialog),
    /// The `DOMContentLoaded` event fired in the main frame.
    DomContentLoaded(Page),
    /// A download started (`download`).
    Download(Download),
    /// A file chooser is about to open (`filechooser`).
    FileChooser(FileChooser),
    /// A frame got attached (`frameattached`).
    FrameAttached(Frame),
    /// A frame got detached (`framedetached`).
    FrameDetached(Frame),
    /// A frame navigated (`framenavigated`).
    FrameNavigated(Frame),
    /// The `load` event fired in the main frame.
    Load(Page),
    /// A new page in the context (`page` on a context).
    Page(Page),
    /// An uncaught exception in the page (`pageerror`; requires the
    /// `console_events` context option).
    PageError(WebError),
    /// The page opened a popup (`popup`).
    Popup(Page),
    /// A request was issued (`request`).
    Request(Request),
    /// A request failed (`requestfailed`).
    RequestFailed(Request),
    /// A request finished successfully (`requestfinished`).
    RequestFinished(Request),
    /// A response was received (`response`).
    Response(Response),
    /// A WebSocket was created (`websocket`).
    WebSocket(WebSocket),
    /// A dedicated worker was created (`worker`).
    Worker(Worker),
}

/// The kind of an [`Event`], used to register handlers and waiters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EventKind {
    /// `close`
    Close,
    /// `console`
    Console,
    /// `crash`
    Crash,
    /// `dialog`
    Dialog,
    /// `domcontentloaded`
    DomContentLoaded,
    /// `download`
    Download,
    /// `filechooser`
    FileChooser,
    /// `frameattached`
    FrameAttached,
    /// `framedetached`
    FrameDetached,
    /// `framenavigated`
    FrameNavigated,
    /// `load`
    Load,
    /// `page` (context)
    Page,
    /// `pageerror` (`weberror` on a context)
    PageError,
    /// `popup`
    Popup,
    /// `request`
    Request,
    /// `requestfailed`
    RequestFailed,
    /// `requestfinished`
    RequestFinished,
    /// `response`
    Response,
    /// `websocket`
    WebSocket,
    /// `worker`
    Worker,
}

impl EventKind {
    /// Playwright's event name.
    pub fn name(&self) -> &'static str {
        match self {
            EventKind::Close => "close",
            EventKind::Console => "console",
            EventKind::Crash => "crash",
            EventKind::Dialog => "dialog",
            EventKind::DomContentLoaded => "domcontentloaded",
            EventKind::Download => "download",
            EventKind::FileChooser => "filechooser",
            EventKind::FrameAttached => "frameattached",
            EventKind::FrameDetached => "framedetached",
            EventKind::FrameNavigated => "framenavigated",
            EventKind::Load => "load",
            EventKind::Page => "page",
            EventKind::PageError => "pageerror",
            EventKind::Popup => "popup",
            EventKind::Request => "request",
            EventKind::RequestFailed => "requestfailed",
            EventKind::RequestFinished => "requestfinished",
            EventKind::Response => "response",
            EventKind::WebSocket => "websocket",
            EventKind::Worker => "worker",
        }
    }
}

impl fmt::Display for EventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl Event {
    /// The kind of this event.
    pub fn kind(&self) -> EventKind {
        match self {
            Event::Close(_) | Event::ContextClose(_) => EventKind::Close,
            Event::Console(_) => EventKind::Console,
            Event::Crash(_) => EventKind::Crash,
            Event::Dialog(_) => EventKind::Dialog,
            Event::DomContentLoaded(_) => EventKind::DomContentLoaded,
            Event::Download(_) => EventKind::Download,
            Event::FileChooser(_) => EventKind::FileChooser,
            Event::FrameAttached(_) => EventKind::FrameAttached,
            Event::FrameDetached(_) => EventKind::FrameDetached,
            Event::FrameNavigated(_) => EventKind::FrameNavigated,
            Event::Load(_) => EventKind::Load,
            Event::Page(_) => EventKind::Page,
            Event::PageError(_) => EventKind::PageError,
            Event::Popup(_) => EventKind::Popup,
            Event::Request(_) => EventKind::Request,
            Event::RequestFailed(_) => EventKind::RequestFailed,
            Event::RequestFinished(_) => EventKind::RequestFinished,
            Event::Response(_) => EventKind::Response,
            Event::WebSocket(_) => EventKind::WebSocket,
            Event::Worker(_) => EventKind::Worker,
        }
    }
}

/// Identifies a handler registered with `on`/`once`, for `remove_listener`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ListenerId(u64);

pub(crate) type Callback = Arc<dyn Fn(Event) + Send + Sync>;

struct HandlerEntry {
    id: u64,
    kind: EventKind,
    callback: Callback,
    once: bool,
}

struct WaiterEntry {
    id: u64,
    kinds: Vec<EventKind>,
    tx: UnboundedSender<Event>,
}

#[derive(Default)]
struct ListenersState {
    next_id: u64,
    handlers: Vec<HandlerEntry>,
    waiters: Vec<WaiterEntry>,
}

type Job = Box<dyn FnOnce() + Send>;
type ChangeCallback = Arc<dyn Fn(EventKind) + Send + Sync>;

/// Handlers and waiters of one emitter (a page or a context).
pub(crate) struct Listeners {
    state: Mutex<ListenersState>,
    jobs: Mutex<Option<UnboundedSender<Job>>>,
    /// Called when the number of listeners of a kind changes.
    on_change: Mutex<Option<ChangeCallback>>,
}

impl Default for Listeners {
    fn default() -> Self {
        Self::new()
    }
}

impl Listeners {
    pub(crate) fn new() -> Self {
        Listeners {
            state: Mutex::new(ListenersState::default()),
            jobs: Mutex::new(None),
            on_change: Mutex::new(None),
        }
    }

    pub(crate) fn set_on_change(&self, f: ChangeCallback) {
        *self.on_change.lock().unwrap() = Some(f);
    }

    fn changed(&self, kind: EventKind) {
        let f = self.on_change.lock().unwrap().clone();
        if let Some(f) = f {
            f(kind);
        }
    }

    fn sender(&self) -> Option<UnboundedSender<Job>> {
        let mut jobs = self.jobs.lock().unwrap();
        if jobs.is_none() {
            let Ok(handle) = tokio::runtime::Handle::try_current() else {
                return None;
            };
            let (tx, mut rx) = unbounded_channel::<Job>();
            handle.spawn(async move {
                while let Some(job) = rx.recv().await {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                    if result.is_err() {
                        tracing::warn!("an event handler panicked");
                    }
                }
            });
            *jobs = Some(tx);
        }
        jobs.clone()
    }

    pub(crate) fn add(&self, kind: EventKind, callback: Callback, once: bool) -> ListenerId {
        let id = {
            let mut state = self.state.lock().unwrap();
            state.next_id += 1;
            let id = state.next_id;
            state.handlers.push(HandlerEntry {
                id,
                kind,
                callback,
                once,
            });
            id
        };
        self.changed(kind);
        ListenerId(id)
    }

    pub(crate) fn remove(&self, id: ListenerId) -> bool {
        let kind = {
            let mut state = self.state.lock().unwrap();
            let Some(index) = state.handlers.iter().position(|h| h.id == id.0) else {
                return false;
            };
            state.handlers.remove(index).kind
        };
        self.changed(kind);
        true
    }

    pub(crate) fn remove_all(&self, kind: Option<EventKind>) {
        let kinds: Vec<EventKind> = {
            let mut state = self.state.lock().unwrap();
            let removed: Vec<EventKind> = state
                .handlers
                .iter()
                .filter(|h| kind.is_none_or(|k| k == h.kind))
                .map(|h| h.kind)
                .collect();
            state.handlers.retain(|h| kind.is_some_and(|k| k != h.kind));
            removed
        };
        for kind in kinds {
            self.changed(kind);
        }
    }

    fn add_waiter(&self, kinds: Vec<EventKind>) -> (u64, UnboundedReceiver<Event>) {
        let (tx, rx) = unbounded_channel();
        let id = {
            let mut state = self.state.lock().unwrap();
            state.next_id += 1;
            let id = state.next_id;
            state.waiters.push(WaiterEntry {
                id,
                kinds: kinds.clone(),
                tx,
            });
            id
        };
        if let Some(kind) = kinds.first() {
            self.changed(*kind);
        }
        (id, rx)
    }

    fn remove_waiter(&self, id: u64) {
        let kind = {
            let mut state = self.state.lock().unwrap();
            let Some(index) = state.waiters.iter().position(|w| w.id == id) else {
                return;
            };
            state.waiters.remove(index).kinds.first().copied()
        };
        if let Some(kind) = kind {
            self.changed(kind);
        }
    }

    /// Handlers and waiters interested in `kind` (a waiter counts for the
    /// first kind it waits for, like Playwright's `waitForEvent` listener).
    pub(crate) fn count(&self, kind: EventKind) -> usize {
        let state = self.state.lock().unwrap();
        state.handlers.iter().filter(|h| h.kind == kind).count()
            + state
                .waiters
                .iter()
                .filter(|w| w.kinds.first() == Some(&kind))
                .count()
    }

    /// Run a job on the handler task (after the handlers already queued).
    pub(crate) fn run(&self, job: Box<dyn FnOnce() + Send>) {
        if let Some(tx) = self.sender() {
            let _ = tx.send(job);
        }
    }

    /// Deliver an event to waiters right away and queue it for handlers.
    pub(crate) fn emit(&self, event: Event) {
        let kind = event.kind();
        let callbacks: Vec<Callback> = {
            let mut state = self.state.lock().unwrap();
            state.waiters.retain(|w| {
                if w.kinds.contains(&kind) {
                    w.tx.send(event.clone()).is_ok()
                } else {
                    !w.tx.is_closed()
                }
            });
            let callbacks = state
                .handlers
                .iter()
                .filter(|h| h.kind == kind)
                .map(|h| h.callback.clone())
                .collect();
            state.handlers.retain(|h| !(h.once && h.kind == kind));
            callbacks
        };
        if callbacks.is_empty() {
            return;
        }
        if let Some(tx) = self.sender() {
            for callback in callbacks {
                let event = event.clone();
                let _ = tx.send(Box::new(move || callback(event)));
            }
        }
    }
}

/// Something that emits events: a page or a context.
#[derive(Clone)]
pub(crate) enum Emitter {
    Page(Page),
    Context(BrowserContext),
}

impl Emitter {
    pub(crate) fn listeners(&self) -> &Listeners {
        match self {
            Emitter::Page(page) => &page.inner.ext.listeners,
            Emitter::Context(context) => &context.inner.ext.listeners,
        }
    }

    fn is_closed(&self) -> bool {
        match self {
            Emitter::Page(page) => page.is_closed(),
            Emitter::Context(context) => context.is_closed(),
        }
    }

    fn timeouts(&self) -> Arc<TimeoutSettings> {
        match self {
            Emitter::Page(page) => page.inner.timeouts.clone(),
            Emitter::Context(context) => context.inner.timeouts.clone(),
        }
    }
}

type Predicate<T> = Box<dyn Fn(&T) -> bool + Send + Sync>;

/// Builder returned by the `wait_for_*` methods; `.await` it. The
/// subscription starts when the builder is created, so it can be created
/// before the action that triggers the event:
///
/// ```no_run
/// # async fn run(page: oxidewright::Page) -> oxidewright::Result<()> {
/// let popup = page.wait_for_popup();
/// page.evaluate::<()>("() => { window.open('about:blank') }").await?;
/// let popup = popup.await?;
/// # let _ = popup; Ok(()) }
/// ```
#[must_use = "builders do nothing unless awaited"]
pub struct WaitForEvent<T> {
    emitter: Emitter,
    kind: EventKind,
    api: &'static str,
    log: Option<String>,
    waiter: Option<(u64, UnboundedReceiver<Event>)>,
    map: fn(Event) -> Option<T>,
    predicate: Option<Predicate<T>>,
    timeout: Option<u64>,
}

impl<T> Drop for WaitForEvent<T> {
    fn drop(&mut self) {
        if let Some((id, _)) = self.waiter.take() {
            self.emitter.listeners().remove_waiter(id);
        }
    }
}

impl<T: Send + 'static> WaitForEvent<T> {
    pub(crate) fn new(
        emitter: Emitter,
        kind: EventKind,
        api: &'static str,
        map: fn(Event) -> Option<T>,
    ) -> Self {
        let mut kinds = vec![kind];
        if kind != EventKind::Close {
            kinds.push(EventKind::Close);
        }
        if matches!(emitter, Emitter::Page(_)) && kind != EventKind::Crash {
            kinds.push(EventKind::Crash);
        }
        // Apply events already received, so an event from an earlier action
        // does not resolve this wait.
        match &emitter {
            Emitter::Page(page) => page.inner.sync(),
            Emitter::Context(context) => {
                for page in context.pages() {
                    page.inner.sync();
                }
            }
        }
        let waiter = emitter.listeners().add_waiter(kinds);
        WaitForEvent {
            emitter,
            kind,
            api,
            log: None,
            waiter: Some(waiter),
            map,
            predicate: None,
            timeout: None,
        }
    }

    pub(crate) fn with_log(mut self, line: String) -> Self {
        self.log = Some(line);
        self
    }

    /// Only resolve for events matching the predicate.
    pub fn predicate(mut self, predicate: impl Fn(&T) -> bool + Send + Sync + 'static) -> Self {
        let previous = self.predicate.take();
        self.predicate = Some(match previous {
            Some(prev) => Box::new(move |v: &T| prev(v) && predicate(v)),
            None => Box::new(predicate),
        });
        self
    }

    /// Maximum time in milliseconds (default: the page's default timeout),
    /// `0` disables the timeout.
    pub fn timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout = Some(timeout_ms);
        self
    }

    /// Run `action` while waiting (Playwright's `expect_*` helpers) and
    /// return the event. Fails if the action fails.
    pub async fn run<R>(self, action: impl IntoFuture<Output = Result<R>>) -> Result<T> {
        let wait = self.into_future();
        let (value, _) = futures::future::try_join(wait, action.into_future()).await?;
        Ok(value)
    }
}

impl<T: Send + 'static> IntoFuture for WaitForEvent<T> {
    type Output = Result<T>;
    type IntoFuture = BoxFuture<Result<T>>;

    fn into_future(mut self) -> Self::IntoFuture {
        let waiter = self.waiter.take();
        let emitter = self.emitter.clone();
        let kind = self.kind;
        let api = self.api;
        let map = self.map;
        let predicate = self.predicate.take();
        let timeout = self.emitter.timeouts().timeout(self.timeout);
        let log: Vec<String> = self.log.take().into_iter().collect();
        Box::pin(async move {
            let Some((id, mut rx)) = waiter else {
                return Err(Error::target_closed().with_api(api));
            };
            let closed_before = emitter.is_closed() && kind != EventKind::Close;
            let wait = async {
                if closed_before {
                    return Err(Error::target_closed());
                }
                loop {
                    let Some(event) = rx.recv().await else {
                        return Err(Error::target_closed());
                    };
                    let event_kind = event.kind();
                    if event_kind == kind {
                        if let Some(value) = map(event) {
                            if predicate.as_ref().is_none_or(|p| p(&value)) {
                                return Ok(value);
                            }
                        }
                        continue;
                    }
                    match event_kind {
                        EventKind::Close => return Err(Error::target_closed()),
                        EventKind::Crash => {
                            return Err(Error::target_closed_with("Page crashed"));
                        }
                        _ => {}
                    }
                }
            };
            let result = if timeout == 0 {
                wait.await
            } else {
                match tokio::time::timeout(Duration::from_millis(timeout), wait).await {
                    Ok(r) => r,
                    Err(_) => Err(Error::timeout(format!(
                        "Timeout {timeout}ms exceeded while waiting for event \"{}\"",
                        kind.name()
                    ))),
                }
            };
            emitter.listeners().remove_waiter(id);
            result.map_err(|e| e.with_api(api).with_log(log))
        })
    }
}

pub(crate) fn as_request(e: Event) -> Option<Request> {
    match e {
        Event::Request(r) => Some(r),
        _ => None,
    }
}
pub(crate) fn as_request_finished(e: Event) -> Option<Request> {
    match e {
        Event::RequestFinished(r) => Some(r),
        _ => None,
    }
}
pub(crate) fn as_request_failed(e: Event) -> Option<Request> {
    match e {
        Event::RequestFailed(r) => Some(r),
        _ => None,
    }
}
pub(crate) fn as_response(e: Event) -> Option<Response> {
    match e {
        Event::Response(r) => Some(r),
        _ => None,
    }
}
pub(crate) fn as_popup(e: Event) -> Option<Page> {
    match e {
        Event::Popup(p) => Some(p),
        _ => None,
    }
}
pub(crate) fn as_page(e: Event) -> Option<Page> {
    match e {
        Event::Page(p) => Some(p),
        _ => None,
    }
}
pub(crate) fn as_close(e: Event) -> Option<()> {
    match e {
        Event::Close(_) | Event::ContextClose(_) => Some(()),
        _ => None,
    }
}
pub(crate) fn as_dialog(e: Event) -> Option<Dialog> {
    match e {
        Event::Dialog(d) => Some(d),
        _ => None,
    }
}
pub(crate) fn as_download(e: Event) -> Option<Download> {
    match e {
        Event::Download(d) => Some(d),
        _ => None,
    }
}
pub(crate) fn as_file_chooser(e: Event) -> Option<FileChooser> {
    match e {
        Event::FileChooser(f) => Some(f),
        _ => None,
    }
}
pub(crate) fn as_console(e: Event) -> Option<ConsoleMessage> {
    match e {
        Event::Console(c) => Some(c),
        _ => None,
    }
}
pub(crate) fn as_page_error(e: Event) -> Option<WebError> {
    match e {
        Event::PageError(c) => Some(c),
        _ => None,
    }
}
pub(crate) fn as_websocket(e: Event) -> Option<WebSocket> {
    match e {
        Event::WebSocket(w) => Some(w),
        _ => None,
    }
}
pub(crate) fn as_worker(e: Event) -> Option<Worker> {
    match e {
        Event::Worker(w) => Some(w),
        _ => None,
    }
}
pub(crate) fn as_frame(e: Event) -> Option<Frame> {
    match e {
        Event::FrameAttached(f) | Event::FrameDetached(f) | Event::FrameNavigated(f) => Some(f),
        _ => None,
    }
}
pub(crate) fn as_any(e: Event) -> Option<Event> {
    Some(e)
}

/// `trimUrl` of Playwright's client (for log lines).
pub(crate) fn trim_url(url: &UrlMatch) -> Option<String> {
    match url {
        UrlMatch::Glob(g) => {
            let s = if g.chars().count() > 50 {
                format!("{}\u{2026}", g.chars().take(49).collect::<String>())
            } else {
                g.clone()
            };
            Some(format!("\"{s}\""))
        }
        UrlMatch::Regex(r) => Some(format!("/{}/", r.as_str())),
        UrlMatch::Predicate(_) => None,
    }
}

/// Typed `on_*` methods and waiters shared by [`Page`] and [`BrowserContext`].
macro_rules! typed_handlers {
    ($($(#[$m:meta])* $on:ident, $kind:ident, $variant:ident, $ty:ty;)*) => {
        $(
            $(#[$m])*
            pub fn $on(&self, handler: impl Fn($ty) + Send + Sync + 'static) -> $crate::events::ListenerId {
                self.on($crate::events::EventKind::$kind, move |event| {
                    if let $crate::events::Event::$variant(value) = event {
                        handler(value)
                    }
                })
            }
        )*
    };
}
pub(crate) use typed_handlers;
