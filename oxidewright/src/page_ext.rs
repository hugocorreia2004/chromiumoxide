//! Page-level state and protocol plumbing of the events, network and routing
//! API, plus the public event methods of [`Page`]. Kept apart from `page.rs`,
//! which owns the frame tree.

use std::collections::HashMap;
use std::future::IntoFuture;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use chromiumoxide::cdp::browser_protocol::fetch::EventAuthRequired;
use chromiumoxide::cdp::browser_protocol::inspector::EventTargetCrashed;
use chromiumoxide::cdp::browser_protocol::network::{
    EventLoadingFailed, EventLoadingFinished, EventRequestServedFromCache, EventRequestWillBeSent,
    EventRequestWillBeSentExtraInfo, EventResponseReceived, EventResponseReceivedExtraInfo,
    EventWebSocketClosed, EventWebSocketCreated, EventWebSocketFrameError,
    EventWebSocketFrameReceived, EventWebSocketFrameSent,
};
use chromiumoxide::cdp::browser_protocol::page::{
    EventFileChooserOpened, EventJavascriptDialogClosed, EventJavascriptDialogOpening,
};
use chromiumoxide::cdp::browser_protocol::security::SetIgnoreCertificateErrorsParams;
use chromiumoxide::cdp::browser_protocol::target::{
    EventAttachedToTarget, EventDetachedFromTarget, EventTargetCreated,
};
use chromiumoxide::cdp::js_protocol::runtime::{
    EnableParams as RuntimeEnableParams, EventBindingCalled, EventConsoleApiCalled,
    EventExceptionThrown, EventExecutionContextCreated,
};
use chromiumoxide::cdp::{Event as CdpEvent, IntoEventKind};
use chromiumoxide::listeners::EventListenerRequest;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedSender as FutSender;

use crate::browser::BrowserInner;
use crate::browser_context::BrowserContext;
use crate::console::{ConsoleMessage, WebError};
use crate::dialog::{self, Dialog};
use crate::download::Download;
use crate::error::{Error, Result, ResultExt};
use crate::events::{self, Event, EventKind, ListenerId, Listeners, WaitForEvent, typed_handlers};
use crate::file_chooser::{self, FileChooser};
use crate::frame::{Frame, LoadState};
use crate::network::{Header, Request, Response};
use crate::network_manager::{self, NetworkManager};
use crate::page::{Page, PageEvent, PageInner};
use crate::route::RouteHandler;
use crate::url_match::UrlMatch;
use crate::websocket::{self, WebSocket};
use crate::worker::{self, Worker};

/// Extra state of a page (events, network, routes, dialogs, ...).
pub(crate) struct PageExt {
    pub(crate) listeners: Listeners,
    pub(crate) network: NetworkManager,
    pub(crate) routes: Mutex<Vec<Arc<RouteHandler>>>,
    pub(crate) extra_headers: Mutex<Vec<Header>>,
    pub(crate) opener: Mutex<Option<Weak<PageInner>>>,
    pub(crate) frames: Mutex<HashMap<String, Frame>>,
    pub(crate) dialogs: Mutex<Vec<Dialog>>,
    pub(crate) workers: Mutex<HashMap<String, Worker>>,
    pub(crate) websockets: Mutex<HashMap<String, WebSocket>>,
    pub(crate) console_contexts: Mutex<HashMap<i64, String>>,
    pub(crate) file_chooser_intercepted: AtomicBool,
    /// Requests linked to their redirect (cycles broken on close).
    pub(crate) redirects: Mutex<Vec<Weak<crate::network::RequestInner>>>,
    /// History, locator handlers and highlights (see `page_state.rs`).
    pub(crate) state: crate::page_state::PageState,
    initialized: AtomicBool,
    /// `expose_function` / `expose_binding` of the page.
    pub(crate) bindings: crate::binding::Bindings,
    /// `route_web_socket` handlers of the page.
    pub(crate) ws_routes: crate::web_socket_route::WsRoutes,
}

impl Default for PageExt {
    fn default() -> Self {
        PageExt {
            listeners: Listeners::new(),
            network: NetworkManager::default(),
            routes: Mutex::new(Vec::new()),
            extra_headers: Mutex::new(Vec::new()),
            opener: Mutex::new(None),
            frames: Mutex::new(HashMap::new()),
            dialogs: Mutex::new(Vec::new()),
            workers: Mutex::new(HashMap::new()),
            websockets: Mutex::new(HashMap::new()),
            console_contexts: Mutex::new(HashMap::new()),
            file_chooser_intercepted: AtomicBool::new(false),
            redirects: Mutex::new(Vec::new()),
            state: Default::default(),
            initialized: AtomicBool::new(false),
            bindings: Default::default(),
            ws_routes: Default::default(),
        }
    }
}

impl PageExt {
    pub(crate) fn opener(&self) -> Option<Arc<PageInner>> {
        self.opener.lock().unwrap().as_ref().and_then(Weak::upgrade)
    }
}

/// Subscribe to the protocol events handled here (same channel as the
/// frame tree events, so the protocol order is kept).
pub(crate) async fn subscribe(
    cdp: &chromiumoxide::Page,
    tx: &FutSender<Arc<dyn CdpEvent>>,
) -> Result<()> {
    async fn one<T: IntoEventKind>(
        cdp: &chromiumoxide::Page,
        tx: &FutSender<Arc<dyn CdpEvent>>,
    ) -> Result<()> {
        cdp.add_event_listener(EventListenerRequest::new::<T>(tx.clone()))
            .await
            .map_err(Error::from)
    }
    one::<EventRequestWillBeSentExtraInfo>(cdp, tx).await?;
    one::<EventResponseReceivedExtraInfo>(cdp, tx).await?;
    one::<EventRequestServedFromCache>(cdp, tx).await?;
    one::<EventAuthRequired>(cdp, tx).await?;
    one::<EventWebSocketCreated>(cdp, tx).await?;
    one::<EventWebSocketFrameSent>(cdp, tx).await?;
    one::<EventWebSocketFrameReceived>(cdp, tx).await?;
    one::<EventWebSocketFrameError>(cdp, tx).await?;
    one::<EventWebSocketClosed>(cdp, tx).await?;
    one::<EventJavascriptDialogOpening>(cdp, tx).await?;
    one::<EventJavascriptDialogClosed>(cdp, tx).await?;
    one::<EventFileChooserOpened>(cdp, tx).await?;
    one::<EventTargetCrashed>(cdp, tx).await?;
    one::<EventConsoleApiCalled>(cdp, tx).await?;
    one::<EventExceptionThrown>(cdp, tx).await?;
    one::<EventExecutionContextCreated>(cdp, tx).await?;
    one::<EventAttachedToTarget>(cdp, tx).await?;
    one::<crate::console::ChildSessionRuntimeEvent>(cdp, tx).await?;
    one::<EventBindingCalled>(cdp, tx).await?;
    Ok(())
}

type AnyArc = Arc<dyn std::any::Any + Send + Sync>;

macro_rules! dispatch {
    ($any:ident, $page:ident, $( $ty:ty => $handler:expr ),* $(,)?) => {
        $(
            let $any = match $any.downcast::<$ty>() {
                Ok(ev) => {
                    #[allow(clippy::redundant_closure_call)]
                    ($handler)($page, ev);
                    return;
                }
                Err(any) => any,
            };
        )*
        let _ = $any;
    };
}

/// Handle a protocol event not consumed by the frame tree.
pub(crate) fn handle_event(page: &PageInner, any: AnyArc) {
    dispatch!(any, page,
        EventRequestWillBeSent => |p: &PageInner, ev: Arc<EventRequestWillBeSent>| network_manager::on_request_will_be_sent(p, ev),
        EventResponseReceived => |p: &PageInner, ev: Arc<EventResponseReceived>| network_manager::on_response_received(p, &ev),
        EventLoadingFinished => |p: &PageInner, ev: Arc<EventLoadingFinished>| network_manager::on_loading_finished(p, &ev),
        EventLoadingFailed => |p: &PageInner, ev: Arc<EventLoadingFailed>| network_manager::on_loading_failed(p, &ev),
        EventRequestWillBeSentExtraInfo => |p: &PageInner, ev: Arc<EventRequestWillBeSentExtraInfo>| network_manager::on_request_will_be_sent_extra_info(p, &ev),
        EventResponseReceivedExtraInfo => |p: &PageInner, ev: Arc<EventResponseReceivedExtraInfo>| network_manager::on_response_received_extra_info(p, &ev),
        EventRequestServedFromCache => |p: &PageInner, ev: Arc<EventRequestServedFromCache>| network_manager::on_request_served_from_cache(p, &ev),
        EventAuthRequired => |p: &PageInner, ev: Arc<EventAuthRequired>| network_manager::on_auth_required(p, &ev),
        EventWebSocketCreated => |p: &PageInner, ev: Arc<EventWebSocketCreated>| websocket::on_created(p, &ev),
        EventWebSocketFrameSent => |p: &PageInner, ev: Arc<EventWebSocketFrameSent>| websocket::on_frame_sent(p, &ev),
        EventWebSocketFrameReceived => |p: &PageInner, ev: Arc<EventWebSocketFrameReceived>| websocket::on_frame_received(p, &ev),
        EventWebSocketFrameError => |p: &PageInner, ev: Arc<EventWebSocketFrameError>| websocket::on_frame_error(p, &ev),
        EventWebSocketClosed => |p: &PageInner, ev: Arc<EventWebSocketClosed>| websocket::on_closed(p, &ev),
        EventJavascriptDialogOpening => |p: &PageInner, ev: Arc<EventJavascriptDialogOpening>| dialog::on_dialog_opening(p, &ev),
        EventJavascriptDialogClosed => |p: &PageInner, _ev: Arc<EventJavascriptDialogClosed>| dialog::on_dialog_closed(p),
        EventFileChooserOpened => |p: &PageInner, ev: Arc<EventFileChooserOpened>| file_chooser::on_file_chooser_opened(p, &ev),
        EventTargetCrashed => |p: &PageInner, _ev: Arc<EventTargetCrashed>| on_crashed(p),
        crate::console::ChildSessionRuntimeEvent => |p: &PageInner, ev: Arc<crate::console::ChildSessionRuntimeEvent>| crate::console::on_child_session_event(p, &ev),
        EventConsoleApiCalled => |p: &PageInner, ev: Arc<EventConsoleApiCalled>| crate::console::on_console_api(p, &ev),
        EventExceptionThrown => |p: &PageInner, ev: Arc<EventExceptionThrown>| crate::console::on_exception(p, &ev),
        EventExecutionContextCreated => |p: &PageInner, ev: Arc<EventExecutionContextCreated>| crate::console::on_context_created(p, &ev),
        EventAttachedToTarget => |p: &PageInner, ev: Arc<EventAttachedToTarget>| worker::on_attached(p, &ev),
        EventBindingCalled => |p: &PageInner, ev: Arc<EventBindingCalled>| crate::binding::on_binding_called(p, &ev),
        EventDetachedFromTarget => |p: &PageInner, ev: Arc<EventDetachedFromTarget>| worker::on_detached(p, ev.session_id.as_ref()),
    );
}

fn on_crashed(page: &PageInner) {
    if let Some(page) = page.self_weak.upgrade() {
        page.ext
            .listeners
            .emit(Event::Crash(Page::from_inner(page.clone())));
    }
}

/// The page went away.
pub(crate) fn did_close(page: &PageInner) {
    page.ext.network.close(page);
    websocket::close_all(page);
    worker::close_all(page);
    page.ext.state.did_close();
    if let Some(inner) = page.self_weak.upgrade() {
        let page = Page::from_inner(inner);
        page.inner.ext.listeners.emit(Event::Close(page.clone()));
        if let Some(context) = page.inner.context.upgrade() {
            context.ext.listeners.emit(Event::PageClose(page.clone()));
        }
    }
}

/// Configure a page adopted by a context (before it is reported).
pub(crate) async fn initialize(page: &Page, context: &BrowserContext) -> Result<()> {
    let inner = &page.inner;
    if inner.ext.initialized.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let options = &context.inner.options;
    if options.ignore_https_errors {
        inner
            .cdp
            .execute(SetIgnoreCertificateErrorsParams::new(true))
            .await?;
    }
    if options.console_events {
        inner.cdp.execute(RuntimeEnableParams::default()).await?;
    }
    NetworkManager::add_session(inner, inner.cdp.session_id().clone()).await;
    NetworkManager::update_interception(inner).await?;
    for frame in inner.frames_dfs() {
        if let Some(cdp) = frame.cdp_frame_now() {
            if cdp.session_id() != inner.cdp.session_id() {
                configure_child_session(inner, cdp.session_id().clone(), options.console_events)
                    .await;
            }
        }
    }
    let weak = Arc::downgrade(inner);
    inner
        .ext
        .listeners
        .set_on_change(Arc::new(move |kind: EventKind| {
            if kind == EventKind::FileChooser {
                if let Some(page) = weak.upgrade() {
                    tokio::spawn(async move { file_chooser::update_interception(&page).await });
                }
            }
        }));
    for frame in inner.frames_dfs() {
        inner
            .ext
            .frames
            .lock()
            .unwrap()
            .insert(frame.id().to_string(), frame);
    }
    tokio::spawn(forward_page_events(
        Arc::downgrade(inner),
        inner.subscribe(),
    ));
    Ok(())
}

async fn configure_child_session(
    page: &Arc<PageInner>,
    session: chromiumoxide::cdp::browser_protocol::target::SessionId,
    console: bool,
) {
    NetworkManager::add_session(page, session.clone()).await;
    if console {
        let _ = network_manager::send(page, Some(session.clone()), RuntimeEnableParams::default())
            .await;
    }
    crate::binding::on_child_session(page, session).await;
}

/// Turn the frame tree notifications into public events.
async fn forward_page_events(
    weak: Weak<PageInner>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<PageEvent>,
) {
    while let Some(event) = rx.recv().await {
        let Some(page) = weak.upgrade() else {
            break;
        };
        match event {
            PageEvent::Closed => break,
            PageEvent::FrameAttached(id) => {
                if let Some(frame) = page.frame_by_id(&id) {
                    page.ext
                        .frames
                        .lock()
                        .unwrap()
                        .insert(id.clone(), frame.clone());
                    watch_session(&page, &frame);
                    emit_frame(&page, Event::FrameAttached(frame));
                }
            }
            PageEvent::FrameDetached(id) => {
                let frame = page.ext.frames.lock().unwrap().remove(&id);
                if let Some(frame) = frame {
                    emit_frame(&page, Event::FrameDetached(frame));
                }
            }
            PageEvent::Navigated(nav) => {
                if nav.error.is_some() {
                    continue;
                }
                if let Some(frame) = page.frame_by_id(&nav.frame_id) {
                    if nav.new_document.is_some() {
                        watch_session(&page, &frame);
                        crate::binding::on_new_document(&page, &frame);
                    }
                    emit_frame(&page, Event::FrameNavigated(frame));
                }
            }
            PageEvent::Lifecycle(id, state) => {
                let is_main = page.main_frame().id() == id;
                if !is_main {
                    continue;
                }
                let p = Page::from_inner(page.clone());
                match state {
                    LoadState::Load => {
                        page.ext.listeners.emit(Event::Load(p.clone()));
                        if let Some(context) = page.context.upgrade() {
                            context.ext.listeners.emit(Event::PageLoad(p.clone()));
                            context.ext.har_page_event(&p, true);
                        }
                    }
                    LoadState::DomContentLoaded => {
                        page.ext.listeners.emit(Event::DomContentLoaded(p.clone()));
                        if let Some(context) = page.context.upgrade() {
                            context.ext.har_page_event(&p, false);
                        }
                    }
                    _ => {}
                }
            }
            // Only used internally (actions waiting for navigations).
            PageEvent::NavigationRequested(_) => {}
        }
    }
}

fn emit_frame(page: &Arc<PageInner>, event: Event) {
    page.ext.listeners.emit(event.clone());
    if let Some(context) = page.context.upgrade() {
        context.ext.listeners.emit(event);
    }
}

/// Configure the session of an out-of-process frame once it is known.
fn watch_session(page: &Arc<PageInner>, frame: &Frame) {
    let page = page.clone();
    let frame = frame.clone();
    tokio::spawn(async move {
        let Ok(cdp) = frame.cdp_frame().await else {
            return;
        };
        if cdp.session_id() == page.cdp.session_id() {
            return;
        }
        let console = page
            .context
            .upgrade()
            .is_some_and(|c| c.options.console_events);
        configure_child_session(&page, cdp.session_id().clone(), console).await;
    });
}

// ----------------------------------------------------------------------
// Popups
// ----------------------------------------------------------------------

/// Adopt popups (targets with an opener) into the opener's context.
pub(crate) async fn start_target_listener(inner: &Arc<BrowserInner>, cdp: &chromiumoxide::Browser) {
    crate::service_worker::start_listener(inner, cdp).await;
    let Ok(mut created) = cdp.event_listener::<EventTargetCreated>().await else {
        return;
    };
    let weak = Arc::downgrade(inner);
    tokio::spawn(async move {
        while let Some(ev) = created.next().await {
            let info = &ev.target_info;
            if info.r#type != "page" {
                continue;
            }
            let Some(browser) = weak.upgrade() else {
                break;
            };
            // New pages start paused until adopted and configured; one that
            // nobody adopts (a context we do not own, a failed adoption) must
            // not stay paused forever.
            tokio::spawn(resume_if_not_adopted(
                Arc::downgrade(&browser),
                info.target_id.clone(),
            ));
            let Some(opener_id) = info.opener_id.clone() else {
                // Like Playwright, every page of a known context is reported
                // (e.g. `target=_blank rel=noopener` links).
                let target_id = info.target_id.clone();
                let context_id = info.browser_context_id.clone();
                tokio::spawn(adopt_new_page(browser, target_id, context_id));
                continue;
            };
            let target_id = info.target_id.clone();
            tokio::spawn(async move {
                adopt_popup(browser, target_id, opener_id).await;
            });
        }
    });
}

async fn adopt_new_page(
    browser: Arc<BrowserInner>,
    target_id: chromiumoxide::cdp::browser_protocol::target::TargetId,
    context_id: Option<chromiumoxide::cdp::browser_protocol::browser::BrowserContextId>,
) {
    let Some(context) = browser.context_for_cdp_context(context_id.as_ref()) else {
        return;
    };
    let mut cdp_page = None;
    for _ in 0..250 {
        if browser.page_by_target(target_id.as_ref()).is_some() {
            return;
        }
        if let Ok(page) = browser.get_cdp_page(target_id.clone()).await {
            cdp_page = Some(page);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let Some(cdp_page) = cdp_page else {
        return;
    };
    if let Err(err) = context.adopt_page(cdp_page).await {
        tracing::debug!("failed to adopt a new page: {err}");
    }
}

async fn resume_if_not_adopted(
    browser: std::sync::Weak<BrowserInner>,
    target_id: chromiumoxide::cdp::browser_protocol::target::TargetId,
) {
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    let Some(browser) = browser.upgrade() else {
        return;
    };
    if browser.page_by_target(target_id.as_ref()).is_some() {
        return;
    }
    if let Ok(page) = browser.get_cdp_page(target_id).await {
        let _ = page
            .execute(
                chromiumoxide::cdp::js_protocol::runtime::RunIfWaitingForDebuggerParams::default(),
            )
            .await;
    }
}

async fn adopt_popup(
    browser: Arc<BrowserInner>,
    target_id: chromiumoxide::cdp::browser_protocol::target::TargetId,
    opener_id: chromiumoxide::cdp::browser_protocol::target::TargetId,
) {
    let Some(opener) = browser.page_by_target(opener_id.as_ref()) else {
        return;
    };
    let Some(context) = opener.context.upgrade() else {
        return;
    };
    let context = BrowserContext { inner: context };
    let mut cdp_page = None;
    for _ in 0..250 {
        if let Some(existing) = browser.page_by_target(target_id.as_ref()) {
            let _ = existing;
            return;
        }
        if let Ok(page) = browser.get_cdp_page(target_id.clone()).await {
            cdp_page = Some(page);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let Some(cdp_page) = cdp_page else {
        return;
    };
    match context
        .adopt_page_with_opener(cdp_page, Some(Arc::downgrade(&opener)))
        .await
    {
        Ok(page) => {
            opener.ext.listeners.emit(Event::Popup(page));
        }
        Err(err) => tracing::debug!("failed to adopt a popup: {err}"),
    }
}

// ----------------------------------------------------------------------
// Public API
// ----------------------------------------------------------------------

/// Parsed `set_extra_http_headers` argument.
pub(crate) fn to_headers<K: Into<String>, V: Into<String>>(
    headers: impl IntoIterator<Item = (K, V)>,
) -> Vec<Header> {
    headers
        .into_iter()
        .map(|(k, v)| Header::new(k, v))
        .collect()
}

impl Page {
    /// Call `handler` for every event of `kind`; returns an id for
    /// [`Page::remove_listener`]. Handlers run in order on a dedicated task.
    pub fn on(
        &self,
        kind: EventKind,
        handler: impl Fn(Event) + Send + Sync + 'static,
    ) -> ListenerId {
        self.inner.ext.listeners.add(kind, Arc::new(handler), false)
    }

    /// Like [`Page::on`], for the next event only.
    pub fn once(
        &self,
        kind: EventKind,
        handler: impl Fn(Event) + Send + Sync + 'static,
    ) -> ListenerId {
        self.inner.ext.listeners.add(kind, Arc::new(handler), true)
    }

    /// Remove a handler added with `on`/`once`/`on_*`.
    pub fn remove_listener(&self, id: ListenerId) -> bool {
        self.inner.ext.listeners.remove(id)
    }

    /// Remove all handlers (of one kind, or all).
    pub fn remove_all_listeners(&self, kind: Option<EventKind>) {
        self.inner.ext.listeners.remove_all(kind)
    }

    typed_handlers! {
        /// `close`: the page closed.
        on_close, Close, Close, Page;
        /// `console` (requires the `console_events` context option).
        on_console, Console, Console, ConsoleMessage;
        /// `crash`: the page crashed.
        on_crash, Crash, Crash, Page;
        /// `dialog`: a JavaScript dialog opened; the handler must accept or
        /// dismiss it.
        on_dialog, Dialog, Dialog, Dialog;
        /// `domcontentloaded` of the main frame.
        on_dom_content_loaded, DomContentLoaded, DomContentLoaded, Page;
        /// `download`: a download started.
        on_download, Download, Download, Download;
        /// `filechooser`: a file chooser is about to open.
        on_file_chooser, FileChooser, FileChooser, FileChooser;
        /// `frameattached`.
        on_frame_attached, FrameAttached, FrameAttached, Frame;
        /// `framedetached`.
        on_frame_detached, FrameDetached, FrameDetached, Frame;
        /// `framenavigated`.
        on_frame_navigated, FrameNavigated, FrameNavigated, Frame;
        /// `load` of the main frame.
        on_load, Load, Load, Page;
        /// `pageerror` (requires the `console_events` context option).
        on_page_error, PageError, PageError, WebError;
        /// `popup`: the page opened a new page.
        on_popup, Popup, Popup, Page;
        /// `request`: a request was issued.
        on_request, Request, Request, Request;
        /// `requestfailed`.
        on_request_failed, RequestFailed, RequestFailed, Request;
        /// `requestfinished`.
        on_request_finished, RequestFinished, RequestFinished, Request;
        /// `response`.
        on_response, Response, Response, Response;
        /// `websocket`.
        on_websocket, WebSocket, WebSocket, WebSocket;
        /// `worker`.
        on_worker, Worker, Worker, Worker;
    }

    fn emitter(&self) -> events::Emitter {
        events::Emitter::Page(self.clone())
    }

    /// Wait for the next event of `kind` (Playwright's `waitForEvent`). The
    /// subscription starts when this method is called.
    pub fn wait_for_event(&self, kind: EventKind) -> WaitForEvent<Event> {
        WaitForEvent::new(self.emitter(), kind, "page.waitForEvent", events::as_any)
            .with_log(format!("waiting for event \"{}\"", kind.name()))
    }

    /// Wait for a request whose URL matches.
    pub fn wait_for_request(&self, url: impl Into<UrlMatch>) -> WaitForEvent<Request> {
        let url = url.into();
        let base = self.inner.base_url();
        let log = events::trim_url(&url).map(|u| format!("waiting for request {u}"));
        let mut w = WaitForEvent::new(
            self.emitter(),
            EventKind::Request,
            "page.waitForRequest",
            events::as_request,
        )
        .predicate(move |r: &Request| url.matches(base.as_deref(), &r.url()));
        if let Some(log) = log {
            w = w.with_log(log);
        }
        w
    }

    /// Wait for a response whose URL matches.
    pub fn wait_for_response(&self, url: impl Into<UrlMatch>) -> WaitForEvent<Response> {
        let url = url.into();
        let base = self.inner.base_url();
        let log = events::trim_url(&url).map(|u| format!("waiting for response {u}"));
        let mut w = WaitForEvent::new(
            self.emitter(),
            EventKind::Response,
            "page.waitForResponse",
            events::as_response,
        )
        .predicate(move |r: &Response| url.matches(base.as_deref(), &r.url()));
        if let Some(log) = log {
            w = w.with_log(log);
        }
        w
    }

    /// Wait for the next popup.
    pub fn wait_for_popup(&self) -> WaitForEvent<Page> {
        WaitForEvent::new(
            self.emitter(),
            EventKind::Popup,
            "page.waitForEvent",
            events::as_popup,
        )
    }

    /// Wait for the next download.
    pub fn wait_for_download(&self) -> WaitForEvent<Download> {
        WaitForEvent::new(
            self.emitter(),
            EventKind::Download,
            "page.waitForEvent",
            events::as_download,
        )
    }

    /// Wait for the next file chooser.
    pub fn wait_for_file_chooser(&self) -> WaitForEvent<FileChooser> {
        WaitForEvent::new(
            self.emitter(),
            EventKind::FileChooser,
            "page.waitForEvent",
            events::as_file_chooser,
        )
    }

    /// Wait for the next dialog.
    pub fn wait_for_dialog(&self) -> WaitForEvent<Dialog> {
        WaitForEvent::new(
            self.emitter(),
            EventKind::Dialog,
            "page.waitForEvent",
            events::as_dialog,
        )
    }

    /// Wait for the next console message (requires `console_events`).
    pub fn wait_for_console_message(&self) -> WaitForEvent<ConsoleMessage> {
        WaitForEvent::new(
            self.emitter(),
            EventKind::Console,
            "page.waitForConsoleMessage",
            events::as_console,
        )
    }

    /// Wait for the next page error (requires `console_events`).
    pub fn wait_for_page_error(&self) -> WaitForEvent<WebError> {
        WaitForEvent::new(
            self.emitter(),
            EventKind::PageError,
            "page.waitForEvent",
            events::as_page_error,
        )
    }

    /// Wait for the next finished request.
    pub fn wait_for_request_finished(&self) -> WaitForEvent<Request> {
        WaitForEvent::new(
            self.emitter(),
            EventKind::RequestFinished,
            "page.waitForEvent",
            events::as_request_finished,
        )
    }

    /// Wait for the next failed request.
    pub fn wait_for_request_failed(&self) -> WaitForEvent<Request> {
        WaitForEvent::new(
            self.emitter(),
            EventKind::RequestFailed,
            "page.waitForEvent",
            events::as_request_failed,
        )
    }

    /// Wait for the next WebSocket.
    pub fn wait_for_websocket(&self) -> WaitForEvent<WebSocket> {
        WaitForEvent::new(
            self.emitter(),
            EventKind::WebSocket,
            "page.waitForEvent",
            events::as_websocket,
        )
    }

    /// Wait for the next worker.
    pub fn wait_for_worker(&self) -> WaitForEvent<Worker> {
        WaitForEvent::new(
            self.emitter(),
            EventKind::Worker,
            "page.waitForEvent",
            events::as_worker,
        )
    }

    /// Wait for the page to close.
    pub fn wait_for_close(&self) -> WaitForEvent<()> {
        WaitForEvent::new(
            self.emitter(),
            EventKind::Close,
            "page.waitForEvent",
            events::as_close,
        )
    }

    /// Wait for the next frame event of `kind` (`FrameAttached`,
    /// `FrameDetached` or `FrameNavigated`).
    pub fn wait_for_frame_event(&self, kind: EventKind) -> WaitForEvent<Frame> {
        WaitForEvent::new(self.emitter(), kind, "page.waitForEvent", events::as_frame)
    }

    /// Run `action` and return the next event of `kind`.
    pub async fn expect_event<R>(
        &self,
        kind: EventKind,
        action: impl IntoFuture<Output = Result<R>>,
    ) -> Result<Event> {
        self.wait_for_event(kind).run(action).await
    }

    /// Run `action` and return the popup it opened.
    pub async fn expect_popup<R>(
        &self,
        action: impl IntoFuture<Output = Result<R>>,
    ) -> Result<Page> {
        self.wait_for_popup().run(action).await
    }

    /// Run `action` and return the download it started.
    pub async fn expect_download<R>(
        &self,
        action: impl IntoFuture<Output = Result<R>>,
    ) -> Result<Download> {
        self.wait_for_download().run(action).await
    }

    /// Run `action` and return the file chooser it opened.
    pub async fn expect_file_chooser<R>(
        &self,
        action: impl IntoFuture<Output = Result<R>>,
    ) -> Result<FileChooser> {
        self.wait_for_file_chooser().run(action).await
    }

    /// Run `action` and return the next console message.
    pub async fn expect_console_message<R>(
        &self,
        action: impl IntoFuture<Output = Result<R>>,
    ) -> Result<ConsoleMessage> {
        self.wait_for_console_message().run(action).await
    }

    /// Run `action` and return the next request matching `url`.
    pub async fn expect_request<R>(
        &self,
        url: impl Into<UrlMatch>,
        action: impl IntoFuture<Output = Result<R>>,
    ) -> Result<Request> {
        self.wait_for_request(url).run(action).await
    }

    /// Run `action` and return the next response matching `url`.
    pub async fn expect_response<R>(
        &self,
        url: impl Into<UrlMatch>,
        action: impl IntoFuture<Output = Result<R>>,
    ) -> Result<Response> {
        self.wait_for_response(url).run(action).await
    }

    /// Run `action` and return the next finished request.
    pub async fn expect_request_finished<R>(
        &self,
        action: impl IntoFuture<Output = Result<R>>,
    ) -> Result<Request> {
        self.wait_for_request_finished().run(action).await
    }

    /// Run `action` and return the next WebSocket.
    pub async fn expect_websocket<R>(
        &self,
        action: impl IntoFuture<Output = Result<R>>,
    ) -> Result<WebSocket> {
        self.wait_for_websocket().run(action).await
    }

    /// Run `action` and return the next worker.
    pub async fn expect_worker<R>(
        &self,
        action: impl IntoFuture<Output = Result<R>>,
    ) -> Result<Worker> {
        self.wait_for_worker().run(action).await
    }

    /// The page that opened this one (popups), if still open.
    pub fn opener(&self) -> Option<Page> {
        self.inner
            .ext
            .opener()
            .filter(|p| !p.is_closed())
            .map(Page::from_inner)
    }

    /// Extra HTTP headers sent with every request of the page (merged over
    /// the context's).
    pub async fn set_extra_http_headers<K: Into<String>, V: Into<String>>(
        &self,
        headers: impl IntoIterator<Item = (K, V)>,
    ) -> Result<()> {
        *self.inner.ext.extra_headers.lock().unwrap() = to_headers(headers);
        NetworkManager::update_extra_headers(&self.inner)
            .await
            .api("page.setExtraHTTPHeaders")
    }
}
