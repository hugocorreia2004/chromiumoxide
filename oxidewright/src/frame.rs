//! [`Frame`]: navigation, evaluation and the frame hierarchy.

use std::fmt;
use std::future::{Future, IntoFuture};
use std::marker::PhantomData;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::dom::{
    BackendNodeId, DescribeNodeParams, GetFrameOwnerParams, ResolveNodeParams,
};
use chromiumoxide::cdp::browser_protocol::page::{
    CreateIsolatedWorldParams, FrameId, SetDocumentContentParams,
};
use chromiumoxide::cdp::js_protocol::runtime::{CallFunctionOnParams, EvaluateParams};
use chromiumoxide::error::CdpError;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::execution_context::{ContextTarget, Evaluated, ExecutionContext, World};
use crate::js::serde_impl::from_js_value;
use crate::js_handle::{ElementHandle, JSHandle, serialize_arg};
use crate::network::Response;
use crate::page::{Page, PageEvent, PageInner};
use crate::progress::Progress;
use crate::url_match::UrlMatch;

/// Lifecycle states a frame goes through, used by `wait_until` and
/// `wait_for_load_state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LoadState {
    /// The navigation response was received and the document started loading.
    Commit,
    /// The `DOMContentLoaded` event fired.
    DomContentLoaded,
    /// The `load` event fired (default).
    #[default]
    Load,
    /// No network connections for at least 500 ms in the frame subtree.
    NetworkIdle,
}

impl LoadState {
    /// The Playwright name of the state.
    pub fn as_str(&self) -> &'static str {
        match self {
            LoadState::Commit => "commit",
            LoadState::DomContentLoaded => "domcontentloaded",
            LoadState::Load => "load",
            LoadState::NetworkIdle => "networkidle",
        }
    }
}

impl fmt::Display for LoadState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for LoadState {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "commit" => Ok(LoadState::Commit),
            "domcontentloaded" => Ok(LoadState::DomContentLoaded),
            "load" => Ok(LoadState::Load),
            "networkidle" | "networkidle0" => Ok(LoadState::NetworkIdle),
            _ => Err(Error::invalid_argument(
                "state: expected one of (load|domcontentloaded|networkidle|commit)",
            )),
        }
    }
}

impl TryFrom<&str> for LoadState {
    type Error = Error;
    fn try_from(s: &str) -> Result<Self> {
        s.parse()
    }
}

/// A frame of a page. Cheap to clone; clones refer to the same frame.
#[derive(Clone)]
pub struct Frame {
    pub(crate) inner: Arc<FrameInner>,
}

pub(crate) struct FrameInner {
    pub(crate) id: String,
    pub(crate) page: std::sync::Weak<PageInner>,
    pub(crate) seq: u64,
    contexts: Mutex<[Option<ExecutionContext>; 2]>,
    context_gen: AtomicU64,
    creating: [tokio::sync::Mutex<()>; 2],
    detached: AtomicBool,
    /// URL and name when the frame got detached.
    last_state: Mutex<(String, String)>,
    cdp_frame: Mutex<Option<chromiumoxide::Frame>>,
}

impl PartialEq for Frame {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for Frame {}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frame")
            .field("id", &self.inner.id)
            .field("url", &self.url())
            .finish()
    }
}

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

impl Frame {
    pub(crate) fn new(id: String, page: std::sync::Weak<PageInner>, seq: u64) -> Self {
        Frame {
            inner: Arc::new(FrameInner {
                id,
                page,
                seq,
                contexts: Mutex::new([None, None]),
                context_gen: AtomicU64::new(0),
                creating: [tokio::sync::Mutex::new(()), tokio::sync::Mutex::new(())],
                detached: AtomicBool::new(false),
                last_state: Mutex::new((String::new(), String::new())),
                cdp_frame: Mutex::new(None),
            }),
        }
    }

    pub(crate) fn from_inner(inner: Arc<FrameInner>) -> Self {
        Frame { inner }
    }

    fn sync(&self) {
        if let Some(page) = self.inner.page.upgrade() {
            page.sync();
        }
    }

    pub(crate) fn page_inner(&self) -> Option<Arc<PageInner>> {
        self.inner.page.upgrade()
    }

    fn page_or_closed(&self) -> Result<Arc<PageInner>> {
        self.page_inner().ok_or_else(Error::target_closed)
    }

    /// The CDP frame id.
    pub fn id(&self) -> &str {
        &self.inner.id
    }

    /// The page containing this frame.
    pub fn page(&self) -> Option<Page> {
        self.page_inner().map(Page::from_inner)
    }

    /// The frame's `name` attribute at the time it was created/navigated.
    pub fn name(&self) -> String {
        self.sync();
        if let Some(page) = self.page_inner() {
            if let Some(name) = page.with_frame(&self.inner.id, |data| data.name.clone()) {
                return name;
            }
        }
        self.inner.last_state.lock().unwrap().1.clone()
    }

    /// The frame's current URL.
    pub fn url(&self) -> String {
        self.sync();
        if let Some(page) = self.page_inner() {
            if let Some(url) = page.with_frame(&self.inner.id, |data| data.url.clone()) {
                return url;
            }
        }
        self.inner.last_state.lock().unwrap().0.clone()
    }

    /// The parent frame, `None` for the main frame and detached frames.
    pub fn parent_frame(&self) -> Option<Frame> {
        self.sync();
        let page = self.page_inner()?;
        let parent = page.with_frame(&self.inner.id, |data| data.parent.clone())??;
        page.frame_by_id(&parent)
    }

    /// Child frames, in attachment order.
    pub fn child_frames(&self) -> Vec<Frame> {
        self.sync();
        let Some(page) = self.page_inner() else {
            return Vec::new();
        };
        let children = page
            .with_frame(&self.inner.id, |data| data.children.clone())
            .unwrap_or_default();
        children
            .iter()
            .filter_map(|id| page.frame_by_id(id))
            .collect()
    }

    /// Whether the frame has been detached from the page.
    pub fn is_detached(&self) -> bool {
        self.sync();
        self.inner.detached.load(Ordering::SeqCst)
    }

    /// The detached flag without syncing events first: safe to call while
    /// holding page locks (`is_detached()` is not).
    pub(crate) fn detached_flag(&self) -> bool {
        self.inner.detached.load(Ordering::SeqCst)
    }

    /// Whether the given lifecycle state was reached for the current document.
    pub fn has_load_state(&self, state: LoadState) -> bool {
        self.sync();
        self.page_inner()
            .and_then(|p| p.with_frame(&self.inner.id, |d| d.lifecycle.contains(&state)))
            .unwrap_or(false)
    }

    pub(crate) fn mark_detached(&self, url: String, name: String) {
        *self.inner.last_state.lock().unwrap() = (url, name);
        self.inner.detached.store(true, Ordering::SeqCst);
        self.invalidate_contexts();
    }

    /// Drop the cached execution contexts (new document, detach, session gone).
    pub(crate) fn invalidate_contexts(&self) {
        self.inner.context_gen.fetch_add(1, Ordering::SeqCst);
        let mut contexts = self.inner.contexts.lock().unwrap();
        for slot in contexts.iter_mut() {
            if let Some(ctx) = slot.take() {
                ctx.mark_destroyed();
            }
        }
        *self.inner.cdp_frame.lock().unwrap() = None;
    }

    /// Invalidate contexts living in the given CDP session.
    pub(crate) fn invalidate_contexts_in_session(&self, session_id: &str) {
        let matches = {
            let contexts = self.inner.contexts.lock().unwrap();
            contexts
                .iter()
                .flatten()
                .any(|c| c.session().session_id().as_ref() == session_id)
        };
        let cdp_matches = self
            .inner
            .cdp_frame
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|f| f.session_id().as_ref() == session_id);
        if matches || cdp_matches {
            self.invalidate_contexts();
        }
    }

    /// The chromiumoxide frame (session-pinned) if one was resolved already.
    pub(crate) fn cdp_frame_now(&self) -> Option<chromiumoxide::Frame> {
        self.inner.cdp_frame.lock().unwrap().clone()
    }

    /// Resolve the chromiumoxide frame handle bound to this frame's current
    /// CDP session (OOPIF frames live in child sessions).
    pub(crate) async fn cdp_frame(&self) -> Result<chromiumoxide::Frame> {
        if let Some(frame) = self.cdp_frame_now() {
            return Ok(frame);
        }
        let page = self.page_or_closed()?;
        let frame_id = FrameId::new(self.inner.id.clone());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if self.is_detached() {
                return Err(Error::frame_detached("Frame was detached"));
            }
            if page.is_closed() {
                return Err(Error::target_closed());
            }
            match page.cdp.frame_by_id(frame_id.clone()).await {
                Ok(Some(frame)) => {
                    *self.inner.cdp_frame.lock().unwrap() = Some(frame.clone());
                    return Ok(frame);
                }
                Ok(None) | Err(CdpError::FrameNotReady) => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(Error::frame_detached("Frame was detached"));
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// The execution context of `world`, created on demand.
    pub(crate) async fn context(&self, world: World) -> Result<ExecutionContext> {
        let index = world.index();
        let mut attempts = 0u32;
        loop {
            if self.is_detached() {
                return Err(Error::frame_detached("Frame was detached"));
            }
            if let Some(ctx) = self.inner.contexts.lock().unwrap()[index].clone() {
                if !ctx.is_destroyed() {
                    return Ok(ctx);
                }
            }
            let _guard = self.inner.creating[index].lock().await;
            if let Some(ctx) = self.inner.contexts.lock().unwrap()[index].clone() {
                if !ctx.is_destroyed() {
                    return Ok(ctx);
                }
            }
            let generation = self.inner.context_gen.load(Ordering::SeqCst);
            match self.create_context(world).await {
                Ok(ctx) => {
                    if generation == self.inner.context_gen.load(Ordering::SeqCst) {
                        self.inner.contexts.lock().unwrap()[index] = Some(ctx.clone());
                        return Ok(ctx);
                    }
                    ctx.mark_destroyed();
                }
                Err(err) => {
                    if self.is_detached() {
                        return Err(Error::frame_detached("Frame was detached"));
                    }
                    if err.is_target_closed() {
                        return Err(err);
                    }
                    attempts += 1;
                    if attempts > 50 {
                        return Err(err);
                    }
                    tracing::debug!("retrying execution context creation: {err}");
                    // The cached session may be stale (process swap).
                    *self.inner.cdp_frame.lock().unwrap() = None;
                    tokio::time::sleep(Duration::from_millis(20 * attempts.min(5) as u64)).await;
                }
            }
        }
    }

    async fn create_context(&self, world: World) -> Result<ExecutionContext> {
        let page = self.page_or_closed()?;
        let cdp = self.cdp_frame().await?;
        let weak = Arc::downgrade(&self.inner);
        match world {
            World::Utility => {
                let mut params =
                    CreateIsolatedWorldParams::new(FrameId::new(self.inner.id.clone()));
                params.world_name = Some(page.utility_world_name.clone());
                params.grant_univeral_access = Some(true);
                let response = cdp.execute(params).await?.result;
                Ok(ExecutionContext::new(
                    weak,
                    World::Utility,
                    cdp,
                    ContextTarget::Id(response.execution_context_id),
                ))
            }
            World::Main => {
                let is_session_root = match cdp.parent_id() {
                    None => true,
                    Some(parent) => match page.cdp.frame_by_id(parent.clone()).await {
                        Ok(Some(parent)) => parent.session_id() != cdp.session_id(),
                        _ => true,
                    },
                };
                let global = if is_session_root {
                    let response = cdp.execute(EvaluateParams::new("globalThis")).await?.result;
                    response
                        .result
                        .object_id
                        .ok_or_else(|| Error::evaluation("Failed to resolve the main world"))?
                } else {
                    self.child_frame_global(&cdp).await?
                };
                Ok(ExecutionContext::new(
                    weak,
                    World::Main,
                    cdp,
                    ContextTarget::Object(global),
                ))
            }
        }
    }

    /// Main world `globalThis` of an in-process child frame, reached through
    /// its document (Patchright's `_getFrameMainFrameContextId` technique).
    async fn child_frame_global(
        &self,
        cdp: &chromiumoxide::Frame,
    ) -> Result<chromiumoxide::cdp::js_protocol::runtime::RemoteObjectId> {
        let owner = cdp
            .execute(GetFrameOwnerParams::new(FrameId::new(
                self.inner.id.clone(),
            )))
            .await?
            .result;
        let mut describe = DescribeNodeParams::default();
        describe.backend_node_id = Some(owner.backend_node_id);
        let node = cdp.execute(describe).await?.result.node;
        let document: BackendNodeId = node
            .content_document
            .map(|d| d.backend_node_id)
            .ok_or_else(|| Error::evaluation("Frame has no content document"))?;
        let mut resolve = ResolveNodeParams::default();
        resolve.backend_node_id = Some(document);
        let document = cdp
            .execute(resolve)
            .await?
            .result
            .object
            .object_id
            .ok_or_else(|| Error::evaluation("Failed to resolve the frame document"))?;
        let mut call = CallFunctionOnParams::new("function() { return globalThis; }");
        call.object_id = Some(document.clone());
        let global = cdp.execute(call).await;
        let _ = cdp
            .execute(chromiumoxide::cdp::js_protocol::runtime::ReleaseObjectParams::new(document))
            .await;
        global?
            .result
            .result
            .object_id
            .ok_or_else(|| Error::evaluation("Failed to resolve the frame main world"))
    }

    /// The main-world execution context.
    pub async fn main_context(&self) -> Result<ExecutionContext> {
        self.context(World::Main).await
    }

    /// The isolated (utility) world execution context.
    pub async fn utility_context(&self) -> Result<ExecutionContext> {
        self.context(World::Utility).await
    }

    // ------------------------------------------------------------------
    // Evaluation
    // ------------------------------------------------------------------

    /// Evaluate a JavaScript expression or function in the frame.
    ///
    /// If `expression` evaluates to a function it is called with the argument
    /// (see [`FrameEvaluate::arg`]); a returned promise is awaited. The result
    /// is converted to `T` with Playwright's serializer.
    pub fn evaluate<T: DeserializeOwned>(&self, expression: impl Into<String>) -> FrameEvaluate<T> {
        FrameEvaluate {
            frame: self.clone(),
            expression: expression.into(),
            arg: Ok(None),
            world: World::default_evaluate(),
            api: "frame.evaluate",
            _marker: PhantomData,
        }
    }

    /// Like [`Frame::evaluate`] but returns a handle to the result.
    pub fn evaluate_handle(&self, expression: impl Into<String>) -> FrameEvaluateHandle {
        FrameEvaluateHandle {
            frame: self.clone(),
            expression: expression.into(),
            arg: Ok(None),
            world: World::default_evaluate(),
            api: "frame.evaluateHandle",
        }
    }

    pub(crate) async fn evaluate_in(
        &self,
        world: World,
        expression: &str,
        arg: Option<(Value, Vec<JSHandle>)>,
        return_by_value: bool,
    ) -> Result<Evaluated> {
        let ctx = self.context(world).await?;
        let (args, handles) = match arg {
            Some((value, handles)) => (vec![value], handles),
            None => (vec![serde_json::json!({ "v": "undefined" })], Vec::new()),
        };
        ctx.evaluate_expression(expression, None, return_by_value, args, handles)
            .await
    }

    /// Evaluate in the utility world and deserialize the result.
    pub(crate) async fn utility_evaluate<T: DeserializeOwned>(
        &self,
        expression: &str,
    ) -> Result<T> {
        match self
            .evaluate_in(World::Utility, expression, None, true)
            .await?
        {
            Evaluated::Value(v) => from_js_value(v).map_err(Error::serialization),
            Evaluated::Handle(_) => Err(Error::evaluation("unexpected handle")),
        }
    }

    /// `document.title`.
    pub async fn title(&self) -> Result<String> {
        self.utility_evaluate("() => document.title")
            .await
            .map_err(|e| e.with_api("frame.title"))
    }

    /// The full HTML contents of the frame, including the doctype.
    pub async fn content(&self) -> Result<String> {
        self.utility_evaluate(
            "() => { let retVal = ''; if (document.doctype) retVal = new XMLSerializer().serializeToString(document.doctype); if (document.documentElement) retVal += document.documentElement.outerHTML; return retVal; }",
        )
        .await
        .map_err(|e| e.with_api("frame.content"))
    }

    /// The `<iframe>`/`<frame>` element hosting this frame.
    pub async fn frame_element(&self) -> Result<ElementHandle> {
        async {
            let parent = self
                .parent_frame()
                .ok_or_else(|| Error::frame_detached("Frame has been detached."))?;
            let parent_cdp = parent.cdp_frame().await?;
            let owner = parent_cdp
                .execute(GetFrameOwnerParams::new(FrameId::new(
                    self.inner.id.clone(),
                )))
                .await
                .map_err(|e| {
                    if e.to_string()
                        .contains("Frame with the given id was not found")
                    {
                        Error::frame_detached("Frame has been detached.")
                    } else {
                        Error::from(e)
                    }
                })?
                .result;
            let ctx = parent.main_context().await?;
            let mut resolve = ResolveNodeParams::default();
            resolve.backend_node_id = Some(owner.backend_node_id);
            resolve.execution_context_id = ctx.inner.context_id;
            let object = ctx.send(resolve).await?.object;
            let handle = ctx.create_handle(object);
            handle
                .as_element()
                .ok_or_else(|| Error::evaluation("Frame owner is not an element"))
        }
        .await
        .map_err(|e| e.with_api("frame.frameElement"))
    }

    // ------------------------------------------------------------------
    // Navigation
    // ------------------------------------------------------------------

    /// Navigate the frame to `url`.
    ///
    /// Resolves with the main resource response (`None` for same-document
    /// navigations and `about:blank`), once `wait_until` is reached.
    pub fn goto(&self, url: impl Into<String>) -> Goto {
        Goto {
            frame: self.clone(),
            url: url.into(),
            wait_until: None,
            timeout: None,
            referer: None,
            api: "frame.goto",
        }
    }

    /// Replace the frame's document with `html`.
    pub fn set_content(&self, html: impl Into<String>) -> SetContent {
        SetContent {
            frame: self.clone(),
            html: html.into(),
            wait_until: None,
            timeout: None,
            api: "frame.setContent",
        }
    }

    /// Wait until the frame reaches the lifecycle `state` (default `load`).
    pub fn wait_for_load_state(&self) -> WaitForLoadState {
        WaitForLoadState {
            frame: self.clone(),
            state: None,
            timeout: None,
            api: "frame.waitForLoadState",
        }
    }

    /// Wait until the frame navigates to a URL matching `url`.
    pub fn wait_for_url(&self, url: impl Into<UrlMatch>) -> WaitForUrl {
        WaitForUrl {
            frame: self.clone(),
            url: url.into(),
            wait_until: None,
            timeout: None,
            api: "frame.waitForURL",
        }
    }

    /// Wait for the next navigation of the frame.
    ///
    /// Subscribe *before* triggering the navigation, e.g. with
    /// `tokio::join!(frame.wait_for_navigation().into_future(), click)`.
    pub fn wait_for_navigation(&self) -> WaitForNavigation {
        let rx = self.page_inner().map(|p| p.subscribe());
        WaitForNavigation {
            frame: self.clone(),
            url: None,
            wait_until: None,
            timeout: None,
            rx,
            api: "frame.waitForNavigation",
        }
    }

    pub(crate) fn timeout_settings_navigation(&self, explicit: Option<u64>) -> u64 {
        match self.page_inner() {
            Some(page) => page.timeouts.navigation_timeout(explicit),
            None => explicit.unwrap_or(crate::progress::DEFAULT_TIMEOUT),
        }
    }

    async fn goto_impl(
        &self,
        progress: &Progress,
        url: String,
        wait_until: LoadState,
        referer: Option<String>,
    ) -> Result<Option<Response>> {
        match self.goto_inner(progress, url, wait_until, referer).await {
            Ok(response) => Ok(response),
            Err(err) => Err(self.closed_or(err).await),
        }
    }

    /// A navigation failing because the page is closing reports the closure
    /// (Playwright races navigations against the page's open scope).
    pub(crate) async fn closed_or(&self, err: Error) -> Error {
        if err.is_target_closed() {
            return err;
        }
        let Some(page) = self.page_inner() else {
            return Error::target_closed();
        };
        for _ in 0..10 {
            if page.is_closed() {
                return Error::target_closed();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        err
    }

    async fn goto_inner(
        &self,
        progress: &Progress,
        url: String,
        wait_until: LoadState,
        referer: Option<String>,
    ) -> Result<Option<Response>> {
        let page = self.page_or_closed()?;
        let url = page.resolve_url(&url);
        progress.log(format!(
            "navigating to \"{url}\", waiting until \"{wait_until}\""
        ));
        // `subscribe()` applies the events already received, so a lagging
        // pump cannot replay an earlier commit as if it interrupted ours.
        let mut rx = page.subscribe();
        let frame_id = self.inner.id.clone();
        let mut detach_rx = page.subscribe();
        let navigate = page.navigate_frame(self, &url, referer);
        let detached = async {
            loop {
                match page.next_event(&mut detach_rx).await {
                    Ok(PageEvent::FrameDetached(id)) if id == frame_id => return,
                    Ok(_) => {}
                    Err(_) => futures::future::pending::<()>().await,
                }
            }
        };
        let result = tokio::select! {
            biased;
            _ = detached => return Err(Error::frame_detached("Frame was detached")),
            result = navigate => result,
        };
        let result = match result {
            Ok(result) => result,
            Err(err) => {
                return Err(self.detached_or(err).await);
            }
        };
        if result.error_text.as_deref().is_some_and(|e| !e.is_empty()) && self.detached_soon().await
        {
            return Err(Error::frame_detached("Frame was detached"));
        }
        if result.is_download == Some(true) {
            return Err(Error::navigation("Download is starting"));
        }
        if let Some(error_text) = result.error_text.filter(|e| !e.is_empty()) {
            // Closing the page aborts its navigation, and Chrome may answer
            // `Page.navigate` with ERR_ABORTED before the close is seen.
            return Err(self
                .closed_or(Error::navigation(format!("{error_text} at {url}")))
                .await);
        }
        let new_document_id = result.loader_id.map(|l| l.as_ref().to_string());
        // Commits received before the `Page.navigate` response belong to
        // earlier navigations (the one this replaces, whose event may still
        // have been in flight when we subscribed). Playwright only listens
        // after the response, so drop those instead of reporting them as an
        // interruption; every other queued event is kept.
        page.sync();
        let mut queued = std::collections::VecDeque::new();
        while let Ok(event) = rx.try_recv() {
            let stale = match (&event, &new_document_id) {
                (PageEvent::Navigated(nav), Some(expected)) if nav.frame_id == frame_id => nav
                    .new_document
                    .as_ref()
                    .is_some_and(|doc| doc.document_id.as_deref() != Some(expected.as_str())),
                _ => false,
            };
            if !stale {
                queued.push_back(event);
            }
        }
        let event = loop {
            let event = match queued.pop_front() {
                Some(PageEvent::Closed) => return Err(Error::target_closed()),
                Some(event) => event,
                None => page.next_event(&mut rx).await?,
            };
            match event {
                PageEvent::FrameDetached(id) if id == frame_id => {
                    return Err(Error::frame_detached("Frame was detached"));
                }
                PageEvent::Navigated(nav) if nav.frame_id == frame_id => match &new_document_id {
                    Some(expected) => {
                        let Some(doc) = &nav.new_document else {
                            continue;
                        };
                        if doc.document_id.as_deref() == Some(expected.as_str())
                            || nav.error.is_none()
                        {
                            if doc.document_id.as_deref() != Some(expected.as_str()) {
                                return Err(Error::navigation(format!(
                                    "Navigation to \"{url}\" is interrupted by another navigation to \"{}\"",
                                    nav.url
                                )));
                            }
                            if let Some(error) = nav.error {
                                // A page closing aborts its navigation
                                // (ERR_ABORTED) just before it is seen closed.
                                return Err(self.closed_or(Error::navigation(error)).await);
                            }
                            break nav;
                        }
                    }
                    None => {
                        if nav.new_document.is_none() {
                            break nav;
                        }
                    }
                },
                _ => {}
            }
        };
        // A detach drained along with the commit would otherwise be lost.
        if queued
            .iter()
            .any(|e| matches!(e, PageEvent::FrameDetached(id) if *id == frame_id))
        {
            return Err(Error::frame_detached("Navigating frame was detached!"));
        }
        self.wait_for_lifecycle(&page, &mut rx, wait_until, None)
            .await?;
        Ok(event.response())
    }

    /// Whether the frame gets detached within a short grace period (a failed
    /// navigation is often reported just before the frame detach event).
    async fn detached_soon(&self) -> bool {
        for _ in 0..10 {
            if self.is_detached() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        self.is_detached()
    }

    async fn detached_or(&self, err: Error) -> Error {
        if self.detached_soon().await {
            Error::frame_detached("Frame was detached")
        } else {
            err
        }
    }

    /// Wait until `state` is fired for this frame, consuming `rx`.
    pub(crate) async fn wait_for_lifecycle(
        &self,
        page: &Arc<PageInner>,
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<PageEvent>,
        state: LoadState,
        progress: Option<&Progress>,
    ) -> Result<()> {
        if self.has_load_state(state) {
            return Ok(());
        }
        loop {
            match page.next_event(rx).await? {
                PageEvent::Lifecycle(id, event) if id == self.inner.id => {
                    if let Some(progress) = progress {
                        progress.log(format!("  \"{event}\" event fired"));
                    }
                    if event == state {
                        return Ok(());
                    }
                }
                PageEvent::FrameDetached(id) if id == self.inner.id => {
                    return Err(Error::frame_detached("Navigating frame was detached!"));
                }
                _ => {
                    if self.has_load_state(state) {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn wait_for_navigation_impl(
        &self,
        progress: &Progress,
        rx: Option<tokio::sync::mpsc::UnboundedReceiver<PageEvent>>,
        url: Option<UrlMatch>,
        wait_until: LoadState,
        requires_new_document: bool,
    ) -> Result<Option<Response>> {
        let page = self.page_or_closed()?;
        let mut rx = rx.unwrap_or_else(|| page.subscribe());
        let to_url = url
            .as_ref()
            .and_then(|u| u.as_glob())
            .map(|g| format!(" to \"{g}\""))
            .unwrap_or_default();
        progress.log(format!(
            "waiting for navigation{to_url} until \"{wait_until}\""
        ));
        let base_url = page.base_url();
        let event = loop {
            match page.next_event(&mut rx).await? {
                PageEvent::FrameDetached(id) if id == self.inner.id => {
                    return Err(Error::frame_detached("Navigating frame was detached!"));
                }
                PageEvent::Navigated(nav) if nav.frame_id == self.inner.id => {
                    if let Some(error) = &nav.error {
                        return Err(Error::navigation(error.clone()));
                    }
                    if requires_new_document && nav.new_document.is_none() {
                        continue;
                    }
                    progress.log(format!("  navigated to \"{}\"", nav.url));
                    let matches = url
                        .as_ref()
                        .is_none_or(|u| u.matches(base_url.as_deref(), &nav.url));
                    if matches {
                        break nav;
                    }
                }
                _ => {}
            }
        };
        self.wait_for_lifecycle(&page, &mut rx, wait_until, Some(progress))
            .await?;
        Ok(event.response())
    }
}

/// Information about a document of a frame.
#[derive(Clone, Default)]
pub(crate) struct DocumentInfo {
    pub(crate) document_id: Option<String>,
    pub(crate) request: Option<crate::network::Request>,
}

/// An internal navigation notification (`Frame.Events.InternalNavigation`).
#[derive(Clone)]
pub(crate) struct NavigationEvent {
    pub(crate) frame_id: String,
    pub(crate) url: String,
    pub(crate) new_document: Option<DocumentInfo>,
    pub(crate) error: Option<String>,
}

impl NavigationEvent {
    fn response(&self) -> Option<Response> {
        self.new_document
            .as_ref()
            .and_then(|d| d.request.as_ref())
            .and_then(|r| r.final_request().existing_response())
    }
}

// ----------------------------------------------------------------------
// Builders
// ----------------------------------------------------------------------

/// Builder returned by [`Frame::evaluate`] / `Page::evaluate`; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct FrameEvaluate<T> {
    pub(crate) frame: Frame,
    pub(crate) expression: String,
    pub(crate) arg: Result<Option<(Value, Vec<JSHandle>)>>,
    pub(crate) world: World,
    pub(crate) api: &'static str,
    pub(crate) _marker: PhantomData<fn() -> T>,
}

impl<T> FrameEvaluate<T> {
    /// The argument passed to the function (any `Serialize` value, which may
    /// contain [`JSHandle`]s / [`crate::JsValue`]s).
    pub fn arg<A: Serialize>(mut self, arg: A) -> Self {
        self.arg = serialize_arg(&arg);
        self
    }

    /// Evaluate in an isolated world instead of the page's main world
    /// (Patchright's `isolated_context`). Page globals are not visible there.
    pub fn isolated_context(mut self, isolated: bool) -> Self {
        self.world = if isolated {
            World::Utility
        } else {
            World::Main
        };
        self
    }

    pub(crate) fn with_api(mut self, api: &'static str) -> Self {
        self.api = api;
        self
    }
}

impl<T: DeserializeOwned + Send + 'static> IntoFuture for FrameEvaluate<T> {
    type Output = Result<T>;
    type IntoFuture = BoxFuture<Result<T>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let api = self.api;
            async move {
                let arg = self.arg?;
                match self
                    .frame
                    .evaluate_in(self.world, &self.expression, arg, true)
                    .await?
                {
                    Evaluated::Value(v) => from_js_value(v).map_err(Error::serialization),
                    Evaluated::Handle(_) => Err(Error::evaluation("unexpected handle result")),
                }
            }
            .await
            .map_err(|e| e.with_api(api))
        })
    }
}

/// Builder returned by [`Frame::evaluate_handle`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct FrameEvaluateHandle {
    pub(crate) frame: Frame,
    pub(crate) expression: String,
    pub(crate) arg: Result<Option<(Value, Vec<JSHandle>)>>,
    pub(crate) world: World,
    pub(crate) api: &'static str,
}

impl FrameEvaluateHandle {
    /// The argument passed to the function.
    pub fn arg<A: Serialize>(mut self, arg: A) -> Self {
        self.arg = serialize_arg(&arg);
        self
    }

    /// Evaluate in an isolated world instead of the main world.
    pub fn isolated_context(mut self, isolated: bool) -> Self {
        self.world = if isolated {
            World::Utility
        } else {
            World::Main
        };
        self
    }

    pub(crate) fn with_api(mut self, api: &'static str) -> Self {
        self.api = api;
        self
    }
}

impl IntoFuture for FrameEvaluateHandle {
    type Output = Result<JSHandle>;
    type IntoFuture = BoxFuture<Result<JSHandle>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let api = self.api;
            async move {
                let arg = self.arg?;
                match self
                    .frame
                    .evaluate_in(self.world, &self.expression, arg, false)
                    .await?
                {
                    Evaluated::Handle(h) => Ok(h),
                    Evaluated::Value(_) => Err(Error::evaluation("unexpected value result")),
                }
            }
            .await
            .map_err(|e| e.with_api(api))
        })
    }
}

/// Builder returned by [`Frame::goto`] / `Page::goto`; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct Goto {
    frame: Frame,
    url: String,
    wait_until: Option<LoadState>,
    timeout: Option<u64>,
    referer: Option<String>,
    api: &'static str,
}

impl Goto {
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

    /// `Referer` header value.
    pub fn referer(mut self, referer: impl Into<String>) -> Self {
        self.referer = Some(referer.into());
        self
    }

    pub(crate) fn with_api(mut self, api: &'static str) -> Self {
        self.api = api;
        self
    }
}

impl IntoFuture for Goto {
    type Output = Result<Option<Response>>;
    type IntoFuture = BoxFuture<Result<Option<Response>>>;
    fn into_future(self) -> Self::IntoFuture {
        let slow = self.frame.page_inner().and_then(|p| p.browser.upgrade());
        Box::pin(crate::browser::with_slow_mo(slow, async move {
            let timeout = self.frame.timeout_settings_navigation(self.timeout);
            let progress = Progress::new(self.api, timeout);
            let wait_until = self.wait_until.unwrap_or_default();
            progress
                .run(
                    self.frame
                        .goto_impl(&progress, self.url, wait_until, self.referer),
                )
                .await
        }))
    }
}

/// Builder returned by [`Frame::set_content`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct SetContent {
    frame: Frame,
    html: String,
    wait_until: Option<LoadState>,
    timeout: Option<u64>,
    api: &'static str,
}

impl SetContent {
    /// When to consider the operation done (default [`LoadState::Load`]).
    pub fn wait_until(mut self, state: LoadState) -> Self {
        self.wait_until = Some(state);
        self
    }

    /// Maximum time in milliseconds, `0` disables the timeout.
    pub fn timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout = Some(timeout_ms);
        self
    }

    pub(crate) fn with_api(mut self, api: &'static str) -> Self {
        self.api = api;
        self
    }
}

impl IntoFuture for SetContent {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let timeout = self.frame.timeout_settings_navigation(self.timeout);
            let progress = Progress::new(self.api, timeout);
            let wait_until = self.wait_until.unwrap_or_default();
            let frame = self.frame.clone();
            let html = self.html;
            let p = progress.clone();
            progress
                .run(async move {
                    let page = frame.page_or_closed()?;
                    p.log(format!(
                        "setting frame content, waiting until \"{wait_until}\""
                    ));
                    let cdp = frame.cdp_frame().await?;
                    let mut rx = page.subscribe();
                    page.clear_lifecycle(&frame.inner.id);
                    let params =
                        SetDocumentContentParams::new(FrameId::new(frame.inner.id.clone()), html);
                    let set = async {
                        cdp.execute(params).await.map_err(Error::from)?;
                        Ok::<_, Error>(())
                    };
                    let wait = frame.wait_for_lifecycle(&page, &mut rx, wait_until, None);
                    let (set, wait) = tokio::join!(set, wait);
                    set?;
                    wait
                })
                .await
        })
    }
}

/// Builder returned by [`Frame::wait_for_load_state`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct WaitForLoadState {
    frame: Frame,
    state: Option<LoadState>,
    timeout: Option<u64>,
    api: &'static str,
}

impl WaitForLoadState {
    /// The state to wait for (default [`LoadState::Load`]).
    pub fn state(mut self, state: LoadState) -> Self {
        self.state = Some(state);
        self
    }

    /// Maximum time in milliseconds, `0` disables the timeout.
    pub fn timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout = Some(timeout_ms);
        self
    }

    pub(crate) fn with_api(mut self, api: &'static str) -> Self {
        self.api = api;
        self
    }
}

impl IntoFuture for WaitForLoadState {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let timeout = self.frame.timeout_settings_navigation(self.timeout);
            let progress = Progress::new(self.api, timeout);
            let state = self.state.unwrap_or_default();
            let frame = self.frame;
            let p = progress.clone();
            progress
                .run(async move {
                    let page = frame.page_or_closed()?;
                    let mut rx = page.subscribe();
                    if frame.has_load_state(state) {
                        p.log(format!("  not waiting, \"{state}\" event already fired"));
                        return Ok(());
                    }
                    frame
                        .wait_for_lifecycle(&page, &mut rx, state, Some(&p))
                        .await
                })
                .await
        })
    }
}

/// Builder returned by [`Frame::wait_for_url`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct WaitForUrl {
    frame: Frame,
    url: UrlMatch,
    wait_until: Option<LoadState>,
    timeout: Option<u64>,
    api: &'static str,
}

impl WaitForUrl {
    /// The state to wait for after the URL matched (default [`LoadState::Load`]).
    pub fn wait_until(mut self, state: LoadState) -> Self {
        self.wait_until = Some(state);
        self
    }

    /// Maximum time in milliseconds, `0` disables the timeout.
    pub fn timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout = Some(timeout_ms);
        self
    }

    pub(crate) fn with_api(mut self, api: &'static str) -> Self {
        self.api = api;
        self
    }
}

impl IntoFuture for WaitForUrl {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let timeout = self.frame.timeout_settings_navigation(self.timeout);
            let progress = Progress::new(self.api, timeout);
            let wait_until = self.wait_until.unwrap_or_default();
            let frame = self.frame;
            let url = self.url;
            let p = progress.clone();
            progress
                .run(async move {
                    let page = frame.page_or_closed()?;
                    let mut rx = page.subscribe();
                    if url.matches(page.base_url().as_deref(), &frame.url()) {
                        if frame.has_load_state(wait_until) {
                            p.log(format!(
                                "  not waiting, \"{wait_until}\" event already fired"
                            ));
                            return Ok(());
                        }
                        return frame
                            .wait_for_lifecycle(&page, &mut rx, wait_until, Some(&p))
                            .await;
                    }
                    frame
                        .wait_for_navigation_impl(&p, Some(rx), Some(url), wait_until, false)
                        .await
                        .map(|_| ())
                })
                .await
        })
    }
}

/// Builder returned by [`Frame::wait_for_navigation`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct WaitForNavigation {
    frame: Frame,
    url: Option<UrlMatch>,
    wait_until: Option<LoadState>,
    timeout: Option<u64>,
    rx: Option<tokio::sync::mpsc::UnboundedReceiver<PageEvent>>,
    api: &'static str,
}

impl WaitForNavigation {
    /// Only resolve for navigations to a matching URL.
    pub fn url(mut self, url: impl Into<UrlMatch>) -> Self {
        self.url = Some(url.into());
        self
    }

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

    pub(crate) fn with_api(mut self, api: &'static str) -> Self {
        self.api = api;
        self
    }

    pub(crate) fn requires_new_document(self) -> NewDocumentNavigation {
        NewDocumentNavigation(self)
    }
}

impl IntoFuture for WaitForNavigation {
    type Output = Result<Option<Response>>;
    type IntoFuture = BoxFuture<Result<Option<Response>>>;
    fn into_future(self) -> Self::IntoFuture {
        run_wait_for_navigation(self, false)
    }
}

/// [`WaitForNavigation`] that ignores same-document navigations (reload).
pub(crate) struct NewDocumentNavigation(WaitForNavigation);

impl IntoFuture for NewDocumentNavigation {
    type Output = Result<Option<Response>>;
    type IntoFuture = BoxFuture<Result<Option<Response>>>;
    fn into_future(self) -> Self::IntoFuture {
        run_wait_for_navigation(self.0, true)
    }
}

fn run_wait_for_navigation(
    builder: WaitForNavigation,
    requires_new_document: bool,
) -> BoxFuture<Result<Option<Response>>> {
    Box::pin(async move {
        let timeout = builder.frame.timeout_settings_navigation(builder.timeout);
        let progress = Progress::new(builder.api, timeout);
        let wait_until = builder.wait_until.unwrap_or_default();
        let frame = builder.frame;
        let p = progress.clone();
        progress
            .run(async move {
                frame
                    .wait_for_navigation_impl(
                        &p,
                        builder.rx,
                        builder.url,
                        wait_until,
                        requires_new_document,
                    )
                    .await
            })
            .await
    })
}
