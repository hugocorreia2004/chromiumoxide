//! [`Page`]: a browser tab, with its frame tree and lifecycle tracking.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
use chromiumoxide::cdp::browser_protocol::network::{
    EnableParams as NetworkEnableParams, EventLoadingFailed, EventLoadingFinished,
    EventRequestWillBeSent, EventResponseReceived,
};
use chromiumoxide::cdp::browser_protocol::page::{
    AddScriptToEvaluateOnNewDocumentParams, BringToFrontParams, ClientNavigationDisposition,
    EventFrameAttached, EventFrameDetached, EventFrameNavigated, EventFrameRequestedNavigation,
    EventLifecycleEvent, EventNavigatedWithinDocument, FrameDetachedReason, FrameId,
    FrameTree as CdpFrameTree, GetFrameTreeParams, GetNavigationHistoryParams, NavigateParams,
    NavigateReturns, NavigateToHistoryEntryParams, NavigationType, ReferrerPolicy, ReloadParams,
    RemoveScriptToEvaluateOnNewDocumentParams, ScriptIdentifier, SetLifecycleEventsEnabledParams,
};
use chromiumoxide::cdp::browser_protocol::target::{CloseTargetParams, EventDetachedFromTarget};
use chromiumoxide::cdp::{Event, IntoEventKind};
use chromiumoxide::listeners::EventListenerRequest;
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedReceiver as FutReceiver, UnboundedSender as FutSender};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::browser::BrowserInner;
use crate::browser_context::{BrowserContext, ContextInner};
use crate::error::{Error, Result, ResultExt};
use crate::frame::{
    DocumentInfo, Frame, FrameEvaluate, FrameEvaluateHandle, Goto, LoadState, NavigationEvent,
    SetContent, WaitForLoadState, WaitForNavigation, WaitForUrl,
};
use crate::network::{Request, RequestInner, RequestState, ResponseData, headers_to_map};
use crate::progress::TimeoutSettings;
use crate::url_match::{UrlMatch, construct_url_based_on_base_url};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Width and height of the viewport, in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ViewportSize {
    /// Width in CSS pixels.
    pub width: u32,
    /// Height in CSS pixels.
    pub height: u32,
}

impl From<(u32, u32)> for ViewportSize {
    fn from((width, height): (u32, u32)) -> Self {
        ViewportSize { width, height }
    }
}

/// A single tab (or popup) of a [`BrowserContext`]. Cheap to clone.
#[derive(Clone)]
pub struct Page {
    pub(crate) inner: Arc<PageInner>,
}

impl fmt::Debug for Page {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Page")
            .field("target_id", &self.inner.cdp.target_id())
            .field("url", &self.url())
            .finish()
    }
}

impl PartialEq for Page {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

/// Internal notifications of a page, delivered in protocol order to every
/// subscriber (see [`PageInner::subscribe`]).
#[derive(Clone)]
pub(crate) enum PageEvent {
    Navigated(NavigationEvent),
    Lifecycle(String, LoadState),
    #[allow(dead_code)] // consumed by the events API
    FrameAttached(String),
    FrameDetached(String),
    Closed,
}

pub(crate) struct FrameData {
    pub(crate) frame: Frame,
    pub(crate) parent: Option<String>,
    pub(crate) children: Vec<String>,
    pub(crate) url: String,
    pub(crate) name: String,
    pub(crate) lifecycle: HashSet<LoadState>,
    fired_network_idle_self: bool,
    current_document: DocumentInfo,
    pending_document: Option<DocumentInfo>,
    inflight: HashSet<String>,
    idle_requests: HashSet<String>,
    idle_timer: Option<u64>,
}

#[derive(Default)]
struct Tree {
    frames: HashMap<String, FrameData>,
    main: Option<String>,
}

/// Side effects collected while the frame tree lock is held.
#[derive(Default)]
struct Effects {
    events: Vec<PageEvent>,
    timers: Vec<(String, u64)>,
    invalidate: Vec<Frame>,
}

thread_local! {
    /// Set while a protocol event is being applied on this thread.
    static HANDLING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) struct PageInner {
    pub(crate) cdp: chromiumoxide::Page,
    pub(crate) browser: Weak<BrowserInner>,
    pub(crate) context: Weak<ContextInner>,
    pub(crate) timeouts: Arc<TimeoutSettings>,
    pub(crate) utility_world_name: String,
    tree: Mutex<Tree>,
    subscribers: Mutex<Vec<UnboundedSender<PageEvent>>>,
    closed: AtomicBool,
    frame_seq: AtomicU64,
    timer_seq: AtomicU64,
    requests: Mutex<HashMap<String, Request>>,
    timer_tx: UnboundedSender<(String, u64)>,
    viewport: Mutex<Option<ViewportSize>>,
    /// Keyboard, mouse and touchscreen state (see [`crate::input`]).
    pub(crate) input: Arc<crate::input::InputState>,
    /// Protocol events not yet applied; drained by the pump task and, for
    /// accessors, synchronously (see [`PageInner::sync`]).
    events: Mutex<Option<FutReceiver<Arc<dyn Event>>>>,
    self_weak: Weak<PageInner>,
}

impl PageInner {
    /// Wrap a chromiumoxide page: subscribe to its events, build the frame
    /// tree and start the event pump.
    pub(crate) async fn attach(
        cdp: chromiumoxide::Page,
        browser: Weak<BrowserInner>,
        context: &BrowserContext,
    ) -> Result<Arc<PageInner>> {
        let (timer_tx, timer_rx) = unbounded_channel();
        let world_name: String = std::iter::repeat_with(fastrand::alphanumeric)
            .take(16)
            .collect();
        let input = crate::input::InputState::new(cdp.clone());
        let page = Arc::new_cyclic(|weak| PageInner {
            cdp,
            browser,
            context: Arc::downgrade(&context.inner),
            timeouts: Arc::new(TimeoutSettings::new(Some(context.inner.timeouts.clone()))),
            utility_world_name: world_name,
            tree: Mutex::new(Tree::default()),
            subscribers: Mutex::new(Vec::new()),
            closed: AtomicBool::new(false),
            frame_seq: AtomicU64::new(0),
            timer_seq: AtomicU64::new(0),
            requests: Mutex::new(HashMap::new()),
            timer_tx,
            viewport: Mutex::new(None),
            input,
            events: Mutex::new(None),
            self_weak: weak.clone(),
        });

        // One channel for every event keeps the protocol order across methods.
        let (tx, rx) = futures::channel::mpsc::unbounded();
        subscribe_events(&page.cdp, &tx).await?;
        drop(tx);

        let _ = page.cdp.execute(NetworkEnableParams::default()).await;
        let tree = page
            .cdp
            .execute(GetFrameTreeParams::default())
            .await?
            .result
            .frame_tree;
        page.handle_frame_tree(&tree);
        page.attach_out_of_process_frames().await;

        *page.events.lock().unwrap() = Some(rx);
        tokio::spawn(pump(page.clone(), timer_rx));

        // Re-enabling lifecycle events makes Blink re-emit the events already
        // fired for the current documents.
        page.refresh_lifecycle().await;
        Ok(page)
    }

    fn handle_frame_tree(&self, tree: &CdpFrameTree) {
        let mut fx = Effects::default();
        {
            let mut state = self.tree.lock().unwrap();
            self.add_tree(&mut state, tree, &mut fx);
        }
        self.apply(fx);
    }

    fn add_tree(&self, state: &mut Tree, tree: &CdpFrameTree, fx: &mut Effects) {
        let frame = &tree.frame;
        let id = frame.id.as_ref().to_string();
        let parent = frame.parent_id.as_ref().map(|p| p.as_ref().to_string());
        self.frame_attached(state, &id, parent.as_deref(), fx);
        let url = format!(
            "{}{}",
            frame.url,
            frame.url_fragment.clone().unwrap_or_default()
        );
        self.commit_new_document(
            state,
            &id,
            url,
            frame.name.clone().unwrap_or_default(),
            frame.loader_id.as_ref().to_string(),
            fx,
        );
        for child in tree.child_frames.iter().flatten() {
            self.add_tree(state, child, fx);
        }
    }

    /// Frames living in child (OOPIF) sessions are not part of the main
    /// session's frame tree; chromiumoxide knows them.
    async fn attach_out_of_process_frames(&self) {
        let Ok(frames) = self.cdp.all_frames().await else {
            return;
        };
        let mut fx = Effects::default();
        {
            let mut state = self.tree.lock().unwrap();
            let mut pending: Vec<_> = frames
                .into_iter()
                .filter(|f| !state.frames.contains_key(f.id().as_ref()))
                .collect();
            loop {
                let before = pending.len();
                pending.retain(|f| {
                    let parent = f.parent_id().map(|p| p.as_ref().to_string());
                    let known = parent
                        .as_ref()
                        .is_none_or(|p| state.frames.contains_key(p.as_str()));
                    if !known {
                        return true;
                    }
                    let id = f.id().as_ref().to_string();
                    self.frame_attached(&mut state, &id, parent.as_deref(), &mut fx);
                    self.commit_new_document(
                        &mut state,
                        &id,
                        f.url().unwrap_or_default(),
                        String::new(),
                        String::new(),
                        &mut fx,
                    );
                    false
                });
                if pending.is_empty() || pending.len() == before {
                    break;
                }
            }
        }
        self.apply(fx);
    }

    async fn refresh_lifecycle(&self) {
        let _ = self
            .cdp
            .execute(SetLifecycleEventsEnabledParams::new(false))
            .await;
        let _ = self
            .cdp
            .execute(SetLifecycleEventsEnabledParams::new(true))
            .await;
        let mut sessions = HashSet::new();
        sessions.insert(self.cdp.session_id().as_ref().to_string());
        for frame in self.frames_dfs() {
            if let Ok(cdp) = frame.cdp_frame().await {
                if sessions.insert(cdp.session_id().as_ref().to_string()) {
                    let _ = cdp
                        .execute(SetLifecycleEventsEnabledParams::new(false))
                        .await;
                    let _ = cdp
                        .execute(SetLifecycleEventsEnabledParams::new(true))
                        .await;
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Accessors
    // ------------------------------------------------------------------

    /// Apply protocol events that were already received. Command responses
    /// are delivered after the events preceding them, so calling this from
    /// accessors makes the frame tree consistent with completed commands.
    pub(crate) fn sync(&self) {
        // Never re-enter from event handling on this thread.
        if HANDLING.with(|h| h.get()) {
            return;
        }
        // The pump holds this lock only while applying one event (no await),
        // so blocking here is short.
        let mut guard = self.events.lock().unwrap();
        if let Some(rx) = guard.as_mut() {
            while let Ok(event) = rx.try_recv() {
                self.handle_event_guarded(event);
            }
        }
    }

    fn handle_event_guarded(&self, event: Arc<dyn Event>) {
        HANDLING.with(|h| h.set(true));
        self.handle_event(event);
        HANDLING.with(|h| h.set(false));
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub(crate) fn with_frame<R>(&self, id: &str, f: impl FnOnce(&FrameData) -> R) -> Option<R> {
        let state = self.tree.lock().unwrap();
        state.frames.get(id).map(f)
    }

    pub(crate) fn frame_by_id(&self, id: &str) -> Option<Frame> {
        self.with_frame(id, |d| d.frame.clone())
    }

    pub(crate) fn main_frame(&self) -> Frame {
        let state = self.tree.lock().unwrap();
        let main = state.main.as_ref().and_then(|id| state.frames.get(id));
        match main {
            Some(data) => data.frame.clone(),
            None => Frame::new(String::new(), self.self_weak.clone(), 0),
        }
    }

    /// All frames, depth first starting at the main frame.
    pub(crate) fn frames_dfs(&self) -> Vec<Frame> {
        let state = self.tree.lock().unwrap();
        let mut out = Vec::new();
        let mut stack: Vec<String> = state.main.iter().cloned().collect();
        while let Some(id) = stack.pop() {
            if let Some(data) = state.frames.get(&id) {
                out.push(data.frame.clone());
                for child in data.children.iter().rev() {
                    stack.push(child.clone());
                }
            }
        }
        out
    }

    pub(crate) fn base_url(&self) -> Option<String> {
        self.context
            .upgrade()
            .and_then(|c| c.options.base_url.clone())
    }

    pub(crate) fn test_id_attribute_name(&self) -> String {
        crate::selectors::test_id_attribute_name()
    }

    pub(crate) fn resolve_url(&self, url: &str) -> String {
        let url = construct_url_based_on_base_url(self.base_url().as_deref(), url);
        // `helper.completeUserURL`
        if url.starts_with("localhost") || url.starts_with("127.0.0.1") {
            format!("http://{url}")
        } else {
            url
        }
    }

    // ------------------------------------------------------------------
    // Subscriptions
    // ------------------------------------------------------------------

    pub(crate) fn subscribe(&self) -> UnboundedReceiver<PageEvent> {
        let (tx, rx) = unbounded_channel();
        if self.is_closed() {
            let _ = tx.send(PageEvent::Closed);
        } else {
            self.subscribers.lock().unwrap().push(tx);
        }
        rx
    }

    /// Next event, failing when the page closes.
    pub(crate) async fn next_event(
        &self,
        rx: &mut UnboundedReceiver<PageEvent>,
    ) -> Result<PageEvent> {
        match rx.recv().await {
            Some(PageEvent::Closed) | None => Err(Error::target_closed()),
            Some(event) => Ok(event),
        }
    }

    fn emit(&self, events: Vec<PageEvent>) {
        if events.is_empty() {
            return;
        }
        let mut subscribers = self.subscribers.lock().unwrap();
        subscribers.retain(|tx| {
            for event in &events {
                if tx.send(event.clone()).is_err() {
                    return false;
                }
            }
            true
        });
    }

    fn apply(&self, fx: Effects) {
        for frame in fx.invalidate {
            frame.invalidate_contexts();
        }
        for (frame_id, generation) in fx.timers {
            let tx = self.timer_tx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let _ = tx.send((frame_id, generation));
            });
        }
        self.emit(fx.events);
    }

    // ------------------------------------------------------------------
    // Frame tree (mirrors server/frames.ts FrameManager)
    // ------------------------------------------------------------------

    fn frame_attached(&self, state: &mut Tree, id: &str, parent: Option<&str>, fx: &mut Effects) {
        if state.frames.contains_key(id) {
            // Remote -> local transition: keep the frame, drop its children.
            self.remove_child_frames(state, id, fx);
            return;
        }
        if let Some(parent) = parent {
            if !state.frames.contains_key(parent) {
                return;
            }
        } else if let Some(main) = state.main.clone() {
            if main != id {
                // Main frame id changed (cross-process navigation of the root):
                // keep the frame object, rename it.
                if let Some(mut data) = state.frames.remove(&main) {
                    let frame =
                        Frame::new(id.to_string(), self.self_weak.clone(), data.frame.inner.seq);
                    data.frame = frame;
                    state.frames.insert(id.to_string(), data);
                    state.main = Some(id.to_string());
                }
                return;
            }
        }
        let seq = self.frame_seq.fetch_add(1, Ordering::SeqCst);
        let frame = Frame::new(id.to_string(), self.self_weak.clone(), seq);
        let mut lifecycle = HashSet::new();
        lifecycle.insert(LoadState::Commit);
        state.frames.insert(
            id.to_string(),
            FrameData {
                frame,
                parent: parent.map(str::to_string),
                children: Vec::new(),
                url: String::new(),
                name: String::new(),
                lifecycle,
                fired_network_idle_self: false,
                current_document: DocumentInfo::default(),
                pending_document: None,
                inflight: HashSet::new(),
                idle_requests: HashSet::new(),
                idle_timer: None,
            },
        );
        match parent {
            Some(parent) => {
                if let Some(p) = state.frames.get_mut(parent) {
                    p.children.push(id.to_string());
                }
            }
            None => state.main = Some(id.to_string()),
        }
        self.start_idle_timer(state, id, fx);
        fx.events.push(PageEvent::FrameAttached(id.to_string()));
    }

    fn remove_child_frames(&self, state: &mut Tree, id: &str, fx: &mut Effects) {
        let children = state
            .frames
            .get(id)
            .map(|d| d.children.clone())
            .unwrap_or_default();
        for child in children {
            self.remove_frame_recursively(state, &child, fx);
        }
    }

    fn remove_frame_recursively(&self, state: &mut Tree, id: &str, fx: &mut Effects) {
        self.remove_child_frames(state, id, fx);
        let Some(data) = state.frames.remove(id) else {
            return;
        };
        data.frame
            .mark_detached(data.url.clone(), data.name.clone());
        if let Some(parent) = &data.parent {
            if let Some(p) = state.frames.get_mut(parent) {
                p.children.retain(|c| c != id);
            }
        }
        if state.main.as_deref() == Some(id) {
            state.main = None;
        }
        fx.events.push(PageEvent::FrameDetached(id.to_string()));
    }

    fn frame_detached(&self, state: &mut Tree, id: &str, fx: &mut Effects) {
        if state.frames.contains_key(id) {
            self.remove_frame_recursively(state, id, fx);
            self.recalculate_network_idle(state, None, fx);
        }
    }

    fn commit_new_document(
        &self,
        state: &mut Tree,
        id: &str,
        url: String,
        name: String,
        document_id: String,
        fx: &mut Effects,
    ) {
        if !state.frames.contains_key(id) {
            return;
        }
        self.remove_child_frames(state, id, fx);
        let data = state.frames.get_mut(id).unwrap();
        data.url = url.clone();
        data.name = name;
        let mut keep_pending = None;
        match data.pending_document.take() {
            Some(mut pending) => {
                if pending.document_id.is_none() {
                    pending.document_id = Some(document_id.clone());
                }
                if pending.document_id.as_deref() == Some(document_id.as_str()) {
                    data.current_document = pending;
                } else {
                    keep_pending = Some(pending);
                    data.current_document = DocumentInfo {
                        document_id: Some(document_id),
                        request: None,
                    };
                }
            }
            None => {
                data.current_document = DocumentInfo {
                    document_id: Some(document_id),
                    request: None,
                };
            }
        }
        let current = data.current_document.clone();
        fx.invalidate.push(data.frame.clone());
        self.clear_lifecycle_locked(state, id, fx);
        fx.events.push(PageEvent::Navigated(NavigationEvent {
            frame_id: id.to_string(),
            url,
            new_document: Some(current),
            error: None,
        }));
        if let Some(data) = state.frames.get_mut(id) {
            data.pending_document = keep_pending;
        }
    }

    fn commit_same_document(&self, state: &mut Tree, id: &str, url: String, fx: &mut Effects) {
        let Some(data) = state.frames.get_mut(id) else {
            return;
        };
        if data
            .pending_document
            .as_ref()
            .is_some_and(|p| p.document_id.is_none() && p.request.is_none())
        {
            data.pending_document = None;
        }
        data.url = url.clone();
        fx.events.push(PageEvent::Navigated(NavigationEvent {
            frame_id: id.to_string(),
            url,
            new_document: None,
            error: None,
        }));
    }

    fn aborted_navigation(
        &self,
        state: &mut Tree,
        id: &str,
        error_text: String,
        document_id: Option<&str>,
        fx: &mut Effects,
    ) {
        let Some(data) = state.frames.get_mut(id) else {
            return;
        };
        let Some(pending) = &data.pending_document else {
            return;
        };
        if let Some(document_id) = document_id {
            if pending.document_id.as_deref() != Some(document_id) {
                return;
            }
        }
        let pending = data.pending_document.take();
        fx.events.push(PageEvent::Navigated(NavigationEvent {
            frame_id: id.to_string(),
            url: data.url.clone(),
            new_document: pending,
            error: Some(error_text),
        }));
    }

    fn requested_navigation(&self, state: &mut Tree, id: &str, document_id: Option<String>) {
        let Some(data) = state.frames.get_mut(id) else {
            return;
        };
        if let Some(pending) = &data.pending_document {
            if pending.document_id == document_id {
                return;
            }
        }
        let request = document_id.as_ref().and_then(|doc| {
            let requests = self.requests.lock().unwrap();
            data.inflight
                .iter()
                .filter_map(|r| requests.get(r))
                .find(|r| r.inner.document_id.as_ref() == Some(doc))
                .cloned()
        });
        data.pending_document = Some(DocumentInfo {
            document_id,
            request,
        });
    }

    fn lifecycle_event(&self, state: &mut Tree, id: &str, event: LoadState, fx: &mut Effects) {
        let Some(data) = state.frames.get_mut(id) else {
            return;
        };
        if !data.lifecycle.insert(event) {
            return;
        }
        fx.events.push(PageEvent::Lifecycle(id.to_string(), event));
        self.recalculate_network_idle(state, None, fx);
    }

    fn clear_lifecycle_locked(&self, state: &mut Tree, id: &str, fx: &mut Effects) {
        let Some(data) = state.frames.get_mut(id) else {
            return;
        };
        data.lifecycle.clear();
        let keep = data
            .current_document
            .request
            .as_ref()
            .map(|r| r.inner.id.clone());
        data.idle_requests.retain(|r| Some(r) == keep.as_ref());
        self.stop_idle_timer(state, id);
        if state
            .frames
            .get(id)
            .is_some_and(|d| d.idle_requests.is_empty())
        {
            self.start_idle_timer(state, id, fx);
        }
        self.recalculate_network_idle(state, Some(id), fx);
        self.lifecycle_event(state, id, LoadState::Commit, fx);
    }

    /// `Frame._onClearLifecycle` (used by `set_content`).
    pub(crate) fn clear_lifecycle(&self, id: &str) {
        let mut fx = Effects::default();
        {
            let mut state = self.tree.lock().unwrap();
            self.clear_lifecycle_locked(&mut state, id, &mut fx);
        }
        self.apply(fx);
    }

    fn start_idle_timer(&self, state: &mut Tree, id: &str, fx: &mut Effects) {
        let Some(data) = state.frames.get_mut(id) else {
            return;
        };
        if data.lifecycle.contains(&LoadState::NetworkIdle) || data.frame.is_detached() {
            return;
        }
        let generation = self.timer_seq.fetch_add(1, Ordering::SeqCst) + 1;
        data.idle_timer = Some(generation);
        fx.timers.push((id.to_string(), generation));
    }

    fn stop_idle_timer(&self, state: &mut Tree, id: &str) {
        if let Some(data) = state.frames.get_mut(id) {
            data.idle_timer = None;
            data.fired_network_idle_self = false;
        }
    }

    fn on_idle_timer(&self, id: &str, generation: u64) {
        let mut fx = Effects::default();
        {
            let mut state = self.tree.lock().unwrap();
            let Some(data) = state.frames.get_mut(id) else {
                return;
            };
            if data.idle_timer != Some(generation) {
                return;
            }
            data.idle_timer = None;
            data.fired_network_idle_self = true;
            self.recalculate_network_idle(&mut state, None, &mut fx);
        }
        self.apply(fx);
    }

    fn recalculate_network_idle(
        &self,
        state: &mut Tree,
        allow_removing: Option<&str>,
        fx: &mut Effects,
    ) {
        if let Some(main) = state.main.clone() {
            self.recalculate_frame(state, &main, allow_removing, fx);
        }
    }

    fn recalculate_frame(
        &self,
        state: &mut Tree,
        id: &str,
        allow_removing: Option<&str>,
        fx: &mut Effects,
    ) {
        let Some(data) = state.frames.get(id) else {
            return;
        };
        let children = data.children.clone();
        let mut idle = data.fired_network_idle_self;
        for child in &children {
            self.recalculate_frame(state, child, allow_removing, fx);
            if !state
                .frames
                .get(child)
                .is_some_and(|c| c.lifecycle.contains(&LoadState::NetworkIdle))
            {
                idle = false;
            }
        }
        let data = state.frames.get_mut(id).unwrap();
        if idle && !data.lifecycle.contains(&LoadState::NetworkIdle) {
            data.lifecycle.insert(LoadState::NetworkIdle);
            fx.events
                .push(PageEvent::Lifecycle(id.to_string(), LoadState::NetworkIdle));
        }
        if allow_removing != Some(id) && data.lifecycle.contains(&LoadState::NetworkIdle) && !idle {
            data.lifecycle.remove(&LoadState::NetworkIdle);
        }
    }

    // ------------------------------------------------------------------
    // Network bookkeeping (navigation requests and network idleness)
    // ------------------------------------------------------------------

    fn on_request_will_be_sent(&self, ev: &EventRequestWillBeSent) {
        if ev.request.url.starts_with("data:") {
            return;
        }
        let mut fx = Effects::default();
        let request_id = ev.request_id.as_ref().to_string();
        let mut redirected_from = None;
        if let Some(redirect) = &ev.redirect_response {
            let previous = self.requests.lock().unwrap().remove(&request_id);
            if let Some(previous) = previous {
                previous.inner.state.lock().unwrap().response = Some(response_data(redirect));
                let mut state = self.tree.lock().unwrap();
                self.request_finished(&mut state, &previous, &mut fx);
                previous.mark_finished();
                redirected_from = Some(previous);
            }
        }
        let frame_id = ev.frame_id.as_ref().map(|f| f.as_ref().to_string());
        let frame = frame_id.as_deref().and_then(|id| self.frame_by_id(id));
        let resource_type = ev
            .r#type
            .as_ref()
            .map(|t| t.as_ref().to_lowercase())
            .unwrap_or_else(|| "other".to_string());
        let is_navigation =
            ev.request_id.as_ref() == ev.loader_id.as_ref() && resource_type == "document";
        let document_id = is_navigation.then(|| ev.loader_id.as_ref().to_string());
        let request = Request {
            inner: Arc::new(RequestInner {
                id: request_id.clone(),
                url: format!(
                    "{}{}",
                    ev.request.url,
                    ev.request.url_fragment.clone().unwrap_or_default()
                ),
                method: ev.request.method.clone(),
                resource_type: resource_type.clone(),
                headers: headers_to_map(&ev.request.headers),
                post_data: None,
                frame,
                document_id: document_id.clone(),
                redirected_from: redirected_from.clone(),
                page: self.self_weak.clone(),
                state: Mutex::new(RequestState::default()),
                finished: tokio::sync::watch::channel(false).0,
            }),
        };
        if let Some(previous) = &redirected_from {
            previous.inner.state.lock().unwrap().redirected_to =
                Some(Arc::downgrade(&request.inner));
        }
        self.requests
            .lock()
            .unwrap()
            .insert(request_id.clone(), request.clone());
        {
            let mut state = self.tree.lock().unwrap();
            if let Some(frame_id) = &frame_id {
                if let Some(data) = state.frames.get_mut(frame_id) {
                    data.inflight.insert(request_id.clone());
                    if resource_type != "eventsource" {
                        data.idle_requests.insert(request_id.clone());
                        if data.idle_requests.len() == 1 {
                            self.stop_idle_timer(&mut state, frame_id);
                        }
                    }
                    if let Some(document_id) = document_id {
                        if let Some(data) = state.frames.get_mut(frame_id) {
                            data.pending_document = Some(DocumentInfo {
                                document_id: Some(document_id),
                                request: Some(request.clone()),
                            });
                        }
                    }
                }
            }
        }
        self.apply(fx);
    }

    fn request_finished(&self, state: &mut Tree, request: &Request, fx: &mut Effects) -> bool {
        let Some(frame) = &request.inner.frame else {
            return true;
        };
        let id = frame.id().to_string();
        let Some(data) = state.frames.get_mut(&id) else {
            return false;
        };
        if !data.inflight.remove(&request.inner.id) {
            return false;
        }
        if data.idle_requests.remove(&request.inner.id) && data.idle_requests.is_empty() {
            self.start_idle_timer(state, &id, fx);
        }
        true
    }

    fn on_response_received(&self, ev: &EventResponseReceived) {
        let request = self
            .requests
            .lock()
            .unwrap()
            .get(ev.request_id.as_ref())
            .cloned();
        if let Some(request) = request {
            request.inner.state.lock().unwrap().response = Some(response_data(&ev.response));
        }
    }

    fn on_loading_finished(&self, ev: &EventLoadingFinished) {
        let request = self.requests.lock().unwrap().remove(ev.request_id.as_ref());
        let Some(request) = request else {
            return;
        };
        let mut fx = Effects::default();
        {
            let mut state = self.tree.lock().unwrap();
            self.request_finished(&mut state, &request, &mut fx);
        }
        request.mark_finished();
        self.apply(fx);
    }

    fn on_loading_failed(&self, ev: &EventLoadingFailed) {
        let request = self.requests.lock().unwrap().remove(ev.request_id.as_ref());
        let Some(request) = request else {
            return;
        };
        request.inner.state.lock().unwrap().failure = Some(ev.error_text.clone());
        let mut fx = Effects::default();
        {
            let mut state = self.tree.lock().unwrap();
            if let Some(frame) = &request.inner.frame {
                let id = frame.id().to_string();
                let is_pending = state
                    .frames
                    .get(&id)
                    .and_then(|d| d.pending_document.as_ref())
                    .and_then(|p| p.request.as_ref())
                    .is_some_and(|r| Arc::ptr_eq(&r.inner, &request.inner));
                if is_pending {
                    let mut error_text = ev.error_text.clone();
                    if ev.canceled == Some(true) {
                        error_text.push_str("; maybe frame was detached?");
                    }
                    let document_id = state
                        .frames
                        .get(&id)
                        .and_then(|d| d.pending_document.as_ref())
                        .and_then(|p| p.document_id.clone());
                    self.aborted_navigation(
                        &mut state,
                        &id,
                        error_text,
                        document_id.as_deref(),
                        &mut fx,
                    );
                }
            }
            self.request_finished(&mut state, &request, &mut fx);
        }
        request.mark_finished();
        self.apply(fx);
    }

    // ------------------------------------------------------------------
    // Event dispatch
    // ------------------------------------------------------------------

    fn handle_event(&self, event: Arc<dyn Event>) {
        let any = event.into_any_arc();
        let any = match any.downcast::<EventLifecycleEvent>() {
            Ok(ev) => {
                let state = match ev.name.as_str() {
                    "load" => Some(LoadState::Load),
                    "DOMContentLoaded" => Some(LoadState::DomContentLoaded),
                    _ => None,
                };
                if let Some(load_state) = state {
                    self.with_effects(|this, state, fx| {
                        this.lifecycle_event(state, ev.frame_id.as_ref(), load_state, fx)
                    });
                }
                return;
            }
            Err(any) => any,
        };
        let any = match any.downcast::<EventFrameNavigated>() {
            Ok(ev) => {
                let frame = &ev.frame;
                let url = format!(
                    "{}{}",
                    frame.url,
                    frame.url_fragment.clone().unwrap_or_default()
                );
                let restored = ev.r#type == NavigationType::BackForwardCacheRestore;
                self.with_effects(|this, state, fx| {
                    this.commit_new_document(
                        state,
                        frame.id.as_ref(),
                        url,
                        frame.name.clone().unwrap_or_default(),
                        frame.loader_id.as_ref().to_string(),
                        fx,
                    );
                    if restored {
                        // A page restored from the back/forward cache is already
                        // loaded: no lifecycle events will follow.
                        let id = frame.id.as_ref();
                        this.lifecycle_event(state, id, LoadState::DomContentLoaded, fx);
                        this.lifecycle_event(state, id, LoadState::Load, fx);
                    }
                });
                return;
            }
            Err(any) => any,
        };
        let any = match any.downcast::<EventNavigatedWithinDocument>() {
            Ok(ev) => {
                self.with_effects(|this, state, fx| {
                    this.commit_same_document(state, ev.frame_id.as_ref(), ev.url.clone(), fx)
                });
                return;
            }
            Err(any) => any,
        };
        let any = match any.downcast::<EventFrameRequestedNavigation>() {
            Ok(ev) => {
                if ev.disposition == ClientNavigationDisposition::CurrentTab {
                    let mut state = self.tree.lock().unwrap();
                    self.requested_navigation(&mut state, ev.frame_id.as_ref(), None);
                }
                return;
            }
            Err(any) => any,
        };
        let any = match any.downcast::<EventFrameAttached>() {
            Ok(ev) => {
                self.with_effects(|this, state, fx| {
                    this.frame_attached(
                        state,
                        ev.frame_id.as_ref(),
                        Some(ev.parent_frame_id.as_ref()),
                        fx,
                    )
                });
                return;
            }
            Err(any) => any,
        };
        let any = match any.downcast::<EventFrameDetached>() {
            Ok(ev) => {
                let id = ev.frame_id.as_ref().to_string();
                if ev.reason == FrameDetachedReason::Swap {
                    // Local -> remote transition: keep the frame for the new target.
                    let frame = self.frame_by_id(&id);
                    self.with_effects(|this, state, fx| this.remove_child_frames(state, &id, fx));
                    if let Some(frame) = frame {
                        frame.invalidate_contexts();
                    }
                } else {
                    self.with_effects(|this, state, fx| this.frame_detached(state, &id, fx));
                }
                return;
            }
            Err(any) => any,
        };
        let any = match any.downcast::<EventRequestWillBeSent>() {
            Ok(ev) => {
                self.on_request_will_be_sent(&ev);
                return;
            }
            Err(any) => any,
        };
        let any = match any.downcast::<EventResponseReceived>() {
            Ok(ev) => {
                self.on_response_received(&ev);
                return;
            }
            Err(any) => any,
        };
        let any = match any.downcast::<EventLoadingFinished>() {
            Ok(ev) => {
                self.on_loading_finished(&ev);
                return;
            }
            Err(any) => any,
        };
        let any = match any.downcast::<EventLoadingFailed>() {
            Ok(ev) => {
                self.on_loading_failed(&ev);
                return;
            }
            Err(any) => any,
        };
        if let Ok(ev) = any.downcast::<EventDetachedFromTarget>() {
            self.on_session_detached(ev.session_id.as_ref().to_string());
        }
    }

    fn with_effects(&self, f: impl FnOnce(&Self, &mut Tree, &mut Effects)) {
        let mut fx = Effects::default();
        {
            let mut state = self.tree.lock().unwrap();
            f(self, &mut state, &mut fx);
        }
        self.apply(fx);
    }

    fn on_session_detached(&self, session_id: String) {
        // Frames whose (cached) CDP handle lives in that session.
        let mut candidates = Vec::new();
        for frame in self.frames_dfs() {
            if frame
                .cdp_frame_now()
                .is_some_and(|f| f.session_id().as_ref() == session_id)
            {
                candidates.push(frame.id().to_string());
            }
            frame.invalidate_contexts_in_session(&session_id);
        }
        if candidates.is_empty() {
            return;
        }
        // A detached OOPIF target is either a remote frame being removed or a
        // remote -> local swap (then `frameAttached` re-binds it). Check later.
        let weak = self.self_weak.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let Some(page) = weak.upgrade() else {
                return;
            };
            for frame_id in candidates {
                if let Ok(None) = page.cdp.frame_by_id(FrameId::new(frame_id.clone())).await {
                    page.with_effects(|this, state, fx| this.frame_detached(state, &frame_id, fx));
                }
            }
        });
    }

    /// Called when the target went away.
    pub(crate) fn did_close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        for frame in self.frames_dfs() {
            frame.invalidate_contexts();
        }
        let subscribers = std::mem::take(&mut *self.subscribers.lock().unwrap());
        for tx in subscribers {
            let _ = tx.send(PageEvent::Closed);
        }
        if let Some(context) = self.context.upgrade() {
            context.remove_page(self);
        }
        if let Some(browser) = self.browser.upgrade() {
            browser.forget_page(self.cdp.target_id().as_ref());
        }
    }

    // ------------------------------------------------------------------
    // Commands
    // ------------------------------------------------------------------

    /// `Page.navigate` sent as-is on the frame's session (chromiumoxide's
    /// own navigation waiting is bypassed: lifecycle tracking is ours).
    pub(crate) async fn navigate_frame(
        &self,
        frame: &Frame,
        url: &str,
        referer: Option<String>,
    ) -> Result<NavigateReturns> {
        let cdp = frame.cdp_frame().await?;
        let browser = self.browser.upgrade().ok_or_else(Error::target_closed)?;
        let mut params = NavigateParams::new(url);
        params.frame_id = Some(FrameId::new(frame.id().to_string()));
        params.referrer = referer;
        params.referrer_policy = Some(ReferrerPolicy::UnsafeUrl);
        browser
            .execute_with_session(params, Some(cdp.session_id().clone()))
            .await
    }

    pub(crate) async fn apply_viewport(&self, viewport: ViewportSize) -> Result<()> {
        let mut params = SetDeviceMetricsOverrideParams::new(
            viewport.width as i64,
            viewport.height as i64,
            1.0,
            false,
        );
        params.screen_width = Some(viewport.width as i64);
        params.screen_height = Some(viewport.height as i64);
        self.cdp.execute(params).await?;
        *self.viewport.lock().unwrap() = Some(viewport);
        Ok(())
    }

    pub(crate) async fn add_init_script_source(
        &self,
        source: String,
    ) -> Result<(Weak<PageInner>, ScriptIdentifier)> {
        let params = AddScriptToEvaluateOnNewDocumentParams::new(source);
        let id = self.cdp.evaluate_on_new_document(params).await?;
        Ok((self.self_weak.clone(), id))
    }
}

fn response_data(
    response: &chromiumoxide::cdp::browser_protocol::network::Response,
) -> ResponseData {
    ResponseData {
        url: response.url.clone(),
        status: response.status,
        status_text: response.status_text.clone(),
        headers: headers_to_map(&response.headers),
        from_cache: response.from_disk_cache == Some(true)
            || response.from_prefetch_cache == Some(true),
    }
}

async fn subscribe_events(cdp: &chromiumoxide::Page, tx: &FutSender<Arc<dyn Event>>) -> Result<()> {
    async fn one<T: IntoEventKind>(
        cdp: &chromiumoxide::Page,
        tx: &FutSender<Arc<dyn Event>>,
    ) -> Result<()> {
        cdp.add_event_listener(EventListenerRequest::new::<T>(tx.clone()))
            .await
            .map_err(Error::from)
    }
    one::<EventFrameAttached>(cdp, tx).await?;
    one::<EventFrameDetached>(cdp, tx).await?;
    one::<EventFrameNavigated>(cdp, tx).await?;
    one::<EventNavigatedWithinDocument>(cdp, tx).await?;
    one::<EventFrameRequestedNavigation>(cdp, tx).await?;
    one::<EventLifecycleEvent>(cdp, tx).await?;
    one::<EventRequestWillBeSent>(cdp, tx).await?;
    one::<EventResponseReceived>(cdp, tx).await?;
    one::<EventLoadingFinished>(cdp, tx).await?;
    one::<EventLoadingFailed>(cdp, tx).await?;
    one::<EventDetachedFromTarget>(cdp, tx).await?;
    Ok(())
}

async fn pump(page: Arc<PageInner>, mut timers: UnboundedReceiver<(String, u64)>) {
    let next_event = || {
        futures::future::poll_fn(|cx| {
            let mut guard = page.events.lock().unwrap();
            let Some(rx) = guard.as_mut() else {
                return std::task::Poll::Ready(None);
            };
            match rx.poll_next_unpin(cx) {
                std::task::Poll::Ready(Some(event)) => {
                    // Handled under the lock so `sync()` keeps the order.
                    page.handle_event_guarded(event);
                    std::task::Poll::Ready(Some(()))
                }
                std::task::Poll::Ready(None) => std::task::Poll::Ready(None),
                std::task::Poll::Pending => std::task::Poll::Pending,
            }
        })
    };
    loop {
        tokio::select! {
            event = next_event() => if event.is_none() {
                break;
            },
            Some((frame_id, generation)) = timers.recv() => page.on_idle_timer(&frame_id, generation),
        }
    }
    page.did_close();
}

// ----------------------------------------------------------------------
// Public API
// ----------------------------------------------------------------------

impl Page {
    pub(crate) fn from_inner(inner: Arc<PageInner>) -> Self {
        Page { inner }
    }

    /// The underlying chromiumoxide page (raw CDP access).
    pub fn cdp(&self) -> &chromiumoxide::Page {
        &self.inner.cdp
    }

    /// The context this page belongs to.
    pub fn context(&self) -> Option<BrowserContext> {
        self.inner
            .context
            .upgrade()
            .map(|inner| BrowserContext { inner })
    }

    /// The page's main frame.
    pub fn main_frame(&self) -> Frame {
        self.inner.sync();
        self.inner.main_frame()
    }

    /// All frames attached to the page (main frame first, depth first).
    pub fn frames(&self) -> Vec<Frame> {
        self.inner.sync();
        self.inner.frames_dfs()
    }

    /// The first frame whose `name` attribute equals `name`.
    pub fn frame(&self, name: &str) -> Option<Frame> {
        self.frames().into_iter().find(|f| f.name() == name)
    }

    /// The first frame whose URL matches.
    pub fn frame_by_url(&self, url: impl Into<UrlMatch>) -> Option<Frame> {
        let url = url.into();
        let base = self.inner.base_url();
        self.frames()
            .into_iter()
            .find(|f| url.matches(base.as_deref(), &f.url()))
    }

    /// The main frame's URL.
    pub fn url(&self) -> String {
        self.main_frame().url()
    }

    /// The page title.
    pub async fn title(&self) -> Result<String> {
        self.main_frame()
            .title()
            .await
            .map_err(|e| rename_api(e, "page.title"))
    }

    /// The full HTML of the page.
    pub async fn content(&self) -> Result<String> {
        self.main_frame()
            .content()
            .await
            .map_err(|e| rename_api(e, "page.content"))
    }

    /// Replace the page's document with `html`.
    pub fn set_content(&self, html: impl Into<String>) -> SetContent {
        self.main_frame()
            .set_content(html)
            .with_api("page.setContent")
    }

    /// Navigate the main frame to `url`.
    pub fn goto(&self, url: impl Into<String>) -> Goto {
        self.main_frame().goto(url).with_api("page.goto")
    }

    /// Reload the page.
    pub fn reload(&self) -> Reload {
        Reload {
            page: self.clone(),
            wait_until: None,
            timeout: None,
        }
    }

    /// Navigate to the previous history entry. Resolves to `None` when there
    /// is no previous entry.
    pub fn go_back(&self) -> GoHistory {
        GoHistory {
            page: self.clone(),
            delta: -1,
            wait_until: None,
            timeout: None,
        }
    }

    /// Navigate to the next history entry.
    pub fn go_forward(&self) -> GoHistory {
        GoHistory {
            page: self.clone(),
            delta: 1,
            wait_until: None,
            timeout: None,
        }
    }

    /// Wait until the main frame reaches a lifecycle state.
    pub fn wait_for_load_state(&self) -> WaitForLoadState {
        self.main_frame()
            .wait_for_load_state()
            .with_api("page.waitForLoadState")
    }

    /// Wait until the main frame navigates to a matching URL.
    pub fn wait_for_url(&self, url: impl Into<UrlMatch>) -> WaitForUrl {
        self.main_frame()
            .wait_for_url(url)
            .with_api("page.waitForURL")
    }

    /// Wait for the next navigation of the main frame. The subscription
    /// starts when this method is called.
    pub fn wait_for_navigation(&self) -> WaitForNavigation {
        self.main_frame()
            .wait_for_navigation()
            .with_api("page.waitForNavigation")
    }

    /// Evaluate JavaScript in the main frame (main world).
    pub fn evaluate<T: DeserializeOwned>(&self, expression: impl Into<String>) -> FrameEvaluate<T> {
        self.main_frame()
            .evaluate(expression)
            .with_api("page.evaluate")
    }

    /// Evaluate JavaScript and return a handle to the result.
    pub fn evaluate_handle(&self, expression: impl Into<String>) -> FrameEvaluateHandle {
        self.main_frame()
            .evaluate_handle(expression)
            .with_api("page.evaluateHandle")
    }

    /// Add a script evaluated in every new document of the page (before any
    /// page script). With [`AddInitScript::arg`] the script must be a function.
    pub fn add_init_script(&self, script: impl Into<String>) -> AddInitScript {
        AddInitScript {
            target: InitScriptTarget::Page(self.clone()),
            script: script.into(),
            arg: None,
        }
    }

    /// Like [`Page::add_init_script`] with the content of a file.
    pub async fn add_init_script_path(
        &self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<InitScript> {
        let source = tokio::fs::read_to_string(path.as_ref())
            .await
            .map_err(|e| {
                Error::invalid_argument(format!("{}: {e}", path.as_ref().display()))
                    .with_api("page.addInitScript")
            })?;
        self.add_init_script(source).await
    }

    /// Resize the viewport.
    pub async fn set_viewport_size(&self, viewport: impl Into<ViewportSize>) -> Result<()> {
        self.inner
            .apply_viewport(viewport.into())
            .await
            .api("page.setViewportSize")
    }

    /// The current viewport size, if emulated.
    pub fn viewport_size(&self) -> Option<ViewportSize> {
        *self.inner.viewport.lock().unwrap()
    }

    /// Bring the page to front (activate the tab).
    pub async fn bring_to_front(&self) -> Result<()> {
        self.inner
            .cdp
            .execute(BringToFrontParams::default())
            .await
            .map(|_| ())
            .map_err(Error::from)
            .api("page.bringToFront")
    }

    /// Default timeout (ms) of actions of this page.
    pub fn set_default_timeout(&self, timeout_ms: u64) {
        self.inner.timeouts.set_default_timeout(timeout_ms);
    }

    /// Default timeout (ms) of navigations of this page.
    pub fn set_default_navigation_timeout(&self, timeout_ms: u64) {
        self.inner
            .timeouts
            .set_default_navigation_timeout(timeout_ms);
    }

    /// Whether the page has been closed.
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    /// Close the page.
    pub async fn close(&self) -> Result<()> {
        async {
            if self.inner.is_closed() {
                return Ok(());
            }
            let mut rx = self.inner.subscribe();
            let browser = self
                .inner
                .browser
                .upgrade()
                .ok_or_else(Error::target_closed)?;
            let result = browser
                .execute_with_session(
                    CloseTargetParams::new(self.inner.cdp.target_id().clone()),
                    None,
                )
                .await;
            if let Err(err) = result {
                if !err.is_target_closed() {
                    return Err(err);
                }
            }
            let wait = async { while self.inner.next_event(&mut rx).await.is_ok() {} };
            let _ = tokio::time::timeout(Duration::from_secs(10), wait).await;
            self.inner.did_close();
            Ok(())
        }
        .await
        .api("page.close")
    }
}

fn rename_api(err: Error, api: &str) -> Error {
    // Errors from frame helpers already carry `frame.*`; report the page API.
    match err {
        Error::Timeout { message, log, .. } => Error::Timeout {
            api: Some(api.into()),
            message,
            log,
        },
        Error::TargetClosed { message, log, .. } => Error::TargetClosed {
            api: Some(api.into()),
            message,
            log,
        },
        Error::Evaluation { message, log, .. } => Error::Evaluation {
            api: Some(api.into()),
            message,
            log,
        },
        Error::FrameDetached { message, log, .. } => Error::FrameDetached {
            api: Some(api.into()),
            message,
            log,
        },
        other => other,
    }
}

/// Builder returned by [`Page::reload`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct Reload {
    page: Page,
    wait_until: Option<LoadState>,
    timeout: Option<u64>,
}

impl Reload {
    /// When to consider the navigation done (default [`LoadState::Load`]).
    pub fn wait_until(mut self, state: LoadState) -> Self {
        self.wait_until = Some(state);
        self
    }

    /// Maximum time in milliseconds, `0` disables the timeout.
    pub fn timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout = Some(timeout_ms);
        self
    }
}

impl IntoFuture for Reload {
    type Output = Result<Option<crate::Response>>;
    type IntoFuture = BoxFuture<Result<Option<crate::Response>>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let frame = self.page.main_frame();
            let mut wait = frame.wait_for_navigation().with_api("page.reload");
            if let Some(state) = self.wait_until {
                wait = wait.wait_until(state);
            }
            if let Some(timeout) = self.timeout {
                wait = wait.timeout(timeout);
            }
            let wait = wait.requires_new_document().into_future();
            self.page
                .inner
                .cdp
                .execute(ReloadParams::default())
                .await
                .map_err(|e| Error::from(e).with_api("page.reload"))?;
            wait.await
        })
    }
}

/// Builder returned by [`Page::go_back`] / [`Page::go_forward`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct GoHistory {
    page: Page,
    delta: i64,
    wait_until: Option<LoadState>,
    timeout: Option<u64>,
}

impl GoHistory {
    /// When to consider the navigation done (default [`LoadState::Load`]).
    pub fn wait_until(mut self, state: LoadState) -> Self {
        self.wait_until = Some(state);
        self
    }

    /// Maximum time in milliseconds, `0` disables the timeout.
    pub fn timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout = Some(timeout_ms);
        self
    }
}

impl IntoFuture for GoHistory {
    type Output = Result<Option<crate::Response>>;
    type IntoFuture = BoxFuture<Result<Option<crate::Response>>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let api = if self.delta < 0 {
                "page.goBack"
            } else {
                "page.goForward"
            };
            let frame = self.page.main_frame();
            let mut wait = frame.wait_for_navigation().with_api(api);
            if let Some(state) = self.wait_until {
                wait = wait.wait_until(state);
            }
            if let Some(timeout) = self.timeout {
                wait = wait.timeout(timeout);
            }
            let cdp = &self.page.inner.cdp;
            let history = cdp
                .execute(GetNavigationHistoryParams::default())
                .await
                .map_err(|e| Error::from(e).with_api(api))?
                .result;
            let index = history.current_index + self.delta;
            if index < 0 || index as usize >= history.entries.len() {
                return Ok(None);
            }
            let entry = &history.entries[index as usize];
            let wait = wait.into_future();
            cdp.execute(NavigateToHistoryEntryParams::new(entry.id))
                .await
                .map_err(|e| Error::from(e).with_api(api))?;
            wait.await
        })
    }
}

pub(crate) enum InitScriptTarget {
    Page(Page),
    Context(BrowserContext),
}

/// Builder returned by `add_init_script`; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct AddInitScript {
    pub(crate) target: InitScriptTarget,
    pub(crate) script: String,
    pub(crate) arg: Option<Result<serde_json::Value>>,
}

impl AddInitScript {
    /// Treat the script as a function and call it with `arg` (JSON-serialized).
    pub fn arg<A: Serialize>(mut self, arg: A) -> Self {
        self.arg = Some(serde_json::to_value(arg).map_err(Error::from));
        self
    }

    fn source(&self) -> Result<String> {
        match &self.arg {
            None => Ok(self.script.clone()),
            Some(Ok(arg)) => Ok(format!("({})({})", self.script, arg)),
            Some(Err(e)) => Err(Error::serialization(e.to_string())),
        }
    }
}

impl IntoFuture for AddInitScript {
    type Output = Result<InitScript>;
    type IntoFuture = BoxFuture<Result<InitScript>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let source = self.source();
            match self.target {
                InitScriptTarget::Page(page) => async {
                    let id = page.inner.add_init_script_source(source?).await?;
                    Ok(InitScript {
                        target: InitScriptTarget::Page(page),
                        ids: vec![id],
                        source: String::new(),
                    })
                }
                .await
                .api("page.addInitScript"),
                InitScriptTarget::Context(context) => async {
                    let source = source?;
                    let ids = context.add_init_script_source(source.clone()).await?;
                    Ok(InitScript {
                        target: InitScriptTarget::Context(context),
                        ids,
                        source,
                    })
                }
                .await
                .api("browserContext.addInitScript"),
            }
        })
    }
}

/// A script registered with `add_init_script`; [`InitScript::dispose`]
/// unregisters it (Playwright's `Disposable`).
pub struct InitScript {
    target: InitScriptTarget,
    ids: Vec<(Weak<PageInner>, ScriptIdentifier)>,
    source: String,
}

impl InitScript {
    /// Stop evaluating the script in new documents.
    pub async fn dispose(self) -> Result<()> {
        if let InitScriptTarget::Context(context) = &self.target {
            context.remove_init_script_source(&self.source);
        }
        for (page, id) in self.ids {
            if let Some(page) = page.upgrade() {
                let _ = page
                    .cdp
                    .execute(RemoveScriptToEvaluateOnNewDocumentParams::new(id))
                    .await;
            }
        }
        Ok(())
    }
}
