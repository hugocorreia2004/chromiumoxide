//! Per-page state of the inspection and interception APIs, a port of the
//! matching parts of Playwright's `server/page.ts`:
//!
//! * the kept history of console messages, page errors and requests
//!   (`page.consoleMessages()`, `page.pageErrors()`, `page.requests()`);
//! * locator handlers (`page.addLocatorHandler()`) and the checkpoint actions
//!   run before acting (`performActionPreChecks`);
//! * locator highlights (`highlightController.ts`).

use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures::FutureExt;
use tokio::sync::watch;

use crate::console::{ConsoleMessage, WebError};
use crate::element_actions::{WaitForSelectorState, is_visible_internal, wait_for_selector};
use crate::error::{Error, Result};
use crate::frame::Frame;
use crate::frame_selectors::{
    CallArg, SelectorSpec, SelectorTarget, call_injected, describe_selector,
    resolve_frames_for_selector,
};
use crate::js::JsValue;
use crate::locator::Locator;
use crate::network::Request;
use crate::page::{Page, PageEvent, PageInner};
use crate::progress::Progress;

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Maximum number of kept console messages / page errors.
const MESSAGES_LIMIT: usize = 200;
/// Maximum number of kept requests.
const REQUESTS_LIMIT: usize = 100;

/// `ensureArrayLimit`: drop the oldest tenth once over the limit.
fn ensure_limit<T>(items: &mut Vec<T>, limit: usize) {
    if items.len() > limit {
        items.drain(0..limit / 10);
    }
}

/// Which messages `console_messages` / `page_errors` return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MessagesFilter {
    /// Only the ones since the last main-frame navigation (the default).
    #[default]
    SinceNavigation,
    /// Every kept entry.
    All,
}

#[derive(Default)]
struct History {
    /// Entries and whether a main-frame navigation happened right after them.
    console: Vec<(ConsoleMessage, bool)>,
    errors: Vec<(WebError, bool)>,
    requests: Vec<Request>,
}

fn filtered<T: Clone>(items: &[(T, bool)], filter: MessagesFilter) -> Vec<T> {
    let start = match filter {
        MessagesFilter::All => 0,
        MessagesFilter::SinceNavigation => items
            .iter()
            .rposition(|(_, marked)| *marked)
            .map(|i| i + 1)
            .unwrap_or(0),
    };
    items[start..].iter().map(|(v, _)| v.clone()).collect()
}

/// The handler of a locator handler.
pub(crate) type HandlerFn = Arc<dyn Fn(Locator) -> BoxFuture<()> + Send + Sync>;

struct HandlerEntry {
    uid: u64,
    locator: Locator,
    handler: HandlerFn,
    no_wait_after: bool,
    times: Option<u32>,
    /// Set while the handler runs (`handler.resolved` of page.ts).
    resolved: Option<watch::Receiver<bool>>,
}

struct HighlightEntry {
    frame: Frame,
    selector: String,
    style: Option<String>,
}

/// See the [module documentation](self).
#[derive(Default)]
pub(crate) struct PageState {
    history: Mutex<History>,
    handlers: Mutex<Vec<HandlerEntry>>,
    last_uid: AtomicU64,
    running: AtomicUsize,
    highlights: Mutex<Vec<HighlightEntry>>,
    highlight_timer: AtomicBool,
    /// Typed `Runtime` events to skip (already reported for a worker).
    skip_typed: Mutex<Vec<String>>,
}

impl PageState {
    /// The typed `method` event that follows was handled as a worker event.
    pub(crate) fn skip_next_typed_runtime_event(&self, method: &str) {
        self.skip_typed.lock().unwrap().push(method.to_string());
    }

    /// Whether a typed `method` event must be skipped (see above).
    pub(crate) fn skip_typed_runtime_event(&self, method: &str) -> bool {
        let mut skip = self.skip_typed.lock().unwrap();
        match skip.iter().position(|m| m == method) {
            Some(i) => {
                skip.remove(i);
                true
            }
            None => false,
        }
    }

    // ------------------------------------------------------------------
    // History
    // ------------------------------------------------------------------

    pub(crate) fn add_console(&self, message: ConsoleMessage) {
        let mut history = self.history.lock().unwrap();
        history.console.push((message, false));
        ensure_limit(&mut history.console, MESSAGES_LIMIT);
    }

    pub(crate) fn add_error(&self, error: WebError) {
        let mut history = self.history.lock().unwrap();
        history.errors.push((error, false));
        ensure_limit(&mut history.errors, MESSAGES_LIMIT);
    }

    pub(crate) fn add_request(&self, request: &Request) {
        let mut history = self.history.lock().unwrap();
        history.requests.push(request.clone());
        ensure_limit(&mut history.requests, REQUESTS_LIMIT);
    }

    /// The main frame committed a new document.
    pub(crate) fn mark_navigation(&self) {
        let mut history = self.history.lock().unwrap();
        if let Some(last) = history.console.last_mut() {
            last.1 = true;
        }
        if let Some(last) = history.errors.last_mut() {
            last.1 = true;
        }
    }

    pub(crate) fn console_messages(&self, filter: MessagesFilter) -> Vec<ConsoleMessage> {
        filtered(&self.history.lock().unwrap().console, filter)
    }

    pub(crate) fn page_errors(&self, filter: MessagesFilter) -> Vec<WebError> {
        filtered(&self.history.lock().unwrap().errors, filter)
    }

    pub(crate) fn requests(&self) -> Vec<Request> {
        self.history.lock().unwrap().requests.clone()
    }

    pub(crate) fn clear_console_messages(&self) {
        self.history.lock().unwrap().console.clear();
    }

    pub(crate) fn clear_page_errors(&self) {
        self.history.lock().unwrap().errors.clear();
    }

    /// The page closed: drop what may hold the page alive.
    pub(crate) fn did_close(&self) {
        self.handlers.lock().unwrap().clear();
        self.highlights.lock().unwrap().clear();
    }

    // ------------------------------------------------------------------
    // Locator handlers
    // ------------------------------------------------------------------

    fn register_handler(
        &self,
        locator: Locator,
        handler: HandlerFn,
        no_wait_after: bool,
        times: Option<u32>,
    ) {
        let uid = self.last_uid.fetch_add(1, Ordering::SeqCst) + 1;
        self.handlers.lock().unwrap().push(HandlerEntry {
            uid,
            locator,
            handler,
            no_wait_after,
            times,
            resolved: None,
        });
    }

    fn unregister_handlers(&self, locator: &Locator) {
        self.handlers
            .lock()
            .unwrap()
            .retain(|h| h.locator != *locator);
    }

    /// `handler.resolved` of `uid`, and whether the handler still exists.
    fn resolved(&self, uid: u64) -> (bool, Option<watch::Receiver<bool>>) {
        let handlers = self.handlers.lock().unwrap();
        match handlers.iter().find(|h| h.uid == uid) {
            Some(h) => (true, h.resolved.clone()),
            None => (false, None),
        }
    }

    /// Emit `locatorHandlerTriggered`: run the handler (on its own task,
    /// like the client does) and return the promise it resolves.
    fn trigger(&self, uid: u64, page: &Arc<PageInner>) -> Option<watch::Receiver<bool>> {
        let mut handlers = self.handlers.lock().unwrap();
        let entry = handlers.iter_mut().find(|h| h.uid == uid)?;
        let (tx, rx) = watch::channel(false);
        entry.resolved = Some(rx.clone());
        tokio::spawn(run_handler(Arc::downgrade(page), uid, tx));
        Some(rx)
    }
}

/// Clears `resolved` (and removes an exhausted handler) when the handler
/// task ends, even if it panicked.
struct ResolveOnDrop {
    page: Weak<PageInner>,
    uid: u64,
    tx: Option<watch::Sender<bool>>,
}

impl Drop for ResolveOnDrop {
    fn drop(&mut self) {
        if let Some(page) = self.page.upgrade() {
            let mut handlers = page.ext.state.handlers.lock().unwrap();
            if let Some(pos) = handlers.iter().position(|h| h.uid == self.uid) {
                handlers[pos].resolved = None;
                if handlers[pos].times == Some(0) {
                    handlers.remove(pos);
                }
            }
        }
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(true);
        }
    }
}

/// `Page._onLocatorHandlerTriggered` of the client.
async fn run_handler(page: Weak<PageInner>, uid: u64, tx: watch::Sender<bool>) {
    let _resolve = ResolveOnDrop {
        page: page.clone(),
        uid,
        tx: Some(tx),
    };
    let call = page.upgrade().and_then(|page| {
        let mut handlers = page.ext.state.handlers.lock().unwrap();
        let entry = handlers.iter_mut().find(|h| h.uid == uid)?;
        if entry.times == Some(0) {
            return None;
        }
        if let Some(times) = &mut entry.times {
            *times -= 1;
        }
        Some((entry.handler.clone(), entry.locator.clone()))
    });
    if let Some((handler, locator)) = call {
        if std::panic::AssertUnwindSafe(handler(locator))
            .catch_unwind()
            .await
            .is_err()
        {
            tracing::debug!("locator handler panicked");
        }
    }
}

/// Resolves when the page closes.
async fn page_closed(page: &PageInner) {
    let mut rx = page.subscribe();
    loop {
        match rx.recv().await {
            Some(PageEvent::Closed) | None => return,
            Some(_) => {}
        }
    }
}

struct RunningGuard<'a>(&'a AtomicUsize);

impl<'a> RunningGuard<'a> {
    fn new(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        RunningGuard(counter)
    }
}

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// `Page.performActionPreChecks`.
pub(crate) fn perform_action_pre_checks<'a>(
    page: &'a Arc<PageInner>,
    progress: &'a Progress,
) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async move {
        wait_for_navigation_check(page, progress).await?;
        locator_handlers_checkpoint(page, progress).await?;
        // Wait once again, just in case a locator handler caused a navigation.
        wait_for_navigation_check(page, progress).await
    })
}

/// `Page._performWaitForNavigationCheck`.
async fn wait_for_navigation_check(page: &Arc<PageInner>, progress: &Progress) -> Result<()> {
    if std::env::var_os("PLAYWRIGHT_SKIP_NAVIGATION_CHECK").is_some() {
        return Ok(());
    }
    let mut rx = page.subscribe();
    let Some(url) = page.main_pending_navigation() else {
        return Ok(());
    };
    let to_url = url
        .map(|u| format!(" \"{}\"", trim_with_ellipsis(&u, 200)))
        .unwrap_or_default();
    progress.log(format!("  waiting for{to_url} navigation to finish..."));
    let main_id = page.main_frame().id().to_string();
    loop {
        match page.next_event(&mut rx).await? {
            PageEvent::Navigated(nav) if nav.frame_id == main_id => {
                if nav.error.is_none() {
                    progress.log(format!(
                        "  navigated to \"{}\"",
                        trim_with_ellipsis(&page.main_frame().url(), 200)
                    ));
                }
                return Ok(());
            }
            PageEvent::FrameDetached(id) if id == main_id => return Ok(()),
            _ => {}
        }
    }
}

fn trim_with_ellipsis(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('\u{2026}');
    out
}

/// `Page._performLocatorHandlersCheckpoint`.
async fn locator_handlers_checkpoint(page: &Arc<PageInner>, progress: &Progress) -> Result<()> {
    let state = &page.ext.state;
    let snapshot: Vec<(u64, String, bool)> = {
        let handlers = state.handlers.lock().unwrap();
        // Do not run locator handlers from inside locator handler callbacks
        // to avoid deadlocks.
        if state.running.load(Ordering::SeqCst) > 0 || handlers.iter().any(|h| h.resolved.is_some())
        {
            return Ok(());
        }
        handlers
            .iter()
            .map(|h| (h.uid, h.locator.selector().to_string(), h.no_wait_after))
            .collect()
    };
    for (uid, selector, no_wait_after) in snapshot {
        let (exists, mut resolved) = state.resolved(uid);
        if exists && resolved.is_none() {
            let main = page.main_frame();
            if is_visible_internal(&main, &SelectorTarget::new(selector.clone(), true)).await? {
                resolved = state.trigger(uid, page);
            }
        }
        let Some(mut resolved) = resolved else {
            continue;
        };
        let _running = RunningGuard::new(&state.running);
        let description = describe_selector(&selector);
        progress.log(format!(
            "  found {description}, intercepting action to run the handler"
        ));
        let wait = async {
            // A dropped sender (the page went away) also ends the wait.
            let _ = resolved.wait_for(|done| *done).await;
            if !no_wait_after {
                progress.log(format!(
                    "  locator handler has finished, waiting for {description} to be hidden"
                ));
                wait_for_selector(
                    &page.main_frame(),
                    progress,
                    &SelectorTarget::new(selector.clone(), false),
                    false,
                    WaitForSelectorState::Hidden,
                    true,
                )
                .await?;
            } else {
                progress.log("  locator handler has finished");
            }
            Ok::<(), Error>(())
        };
        tokio::select! {
            result = wait => result?,
            _ = page_closed(page) => return Err(Error::target_closed()),
        }
        progress.log("  interception handler has finished, continuing");
    }
    Ok(())
}

// ----------------------------------------------------------------------
// Highlights (highlightController.ts)
// ----------------------------------------------------------------------

/// `HighlightController.addHighlight`.
pub(crate) async fn add_highlight(
    page: &Arc<PageInner>,
    frame: &Frame,
    selector: &str,
    style: Option<String>,
) -> Result<()> {
    crate::selector_parser::parse_selector(selector)
        .map_err(|e| Error::invalid_argument(e.to_string()))?;
    {
        let mut entries = page.ext.state.highlights.lock().unwrap();
        entries.retain(|e| !(e.frame == *frame && e.selector == selector));
        entries.push(HighlightEntry {
            frame: frame.clone(),
            selector: selector.to_string(),
            style,
        });
    }
    resolve_highlights(page).await;
    Ok(())
}

/// `HighlightController.removeHighlight`.
pub(crate) async fn remove_highlight(page: &Arc<PageInner>, frame: &Frame, selector: &str) {
    page.ext
        .state
        .highlights
        .lock()
        .unwrap()
        .retain(|e| !(e.frame == *frame && e.selector == selector));
    resolve_highlights(page).await;
}

/// `HighlightController.hideHighlights`.
pub(crate) async fn hide_highlights(page: &Arc<PageInner>) {
    page.ext.state.highlights.lock().unwrap().clear();
    for frame in page.frames_dfs() {
        if let Some(ctx) = frame.existing_context(crate::execution_context::World::Utility) {
            let _ = call_injected(&ctx, "injected => injected.hideHighlight()", vec![], true).await;
        }
    }
}

/// `HighlightController._resolve`: render every entry in the frames its
/// selector resolves to, and re-resolve every second while there are
/// entries (so highlights survive navigations).
fn resolve_highlights(page: &Arc<PageInner>) -> BoxFuture<()> {
    let page = page.clone();
    Box::pin(async move {
        if page.is_closed() {
            return;
        }
        let entries: Vec<(Frame, String, Option<String>)> = page
            .ext
            .state
            .highlights
            .lock()
            .unwrap()
            .iter()
            .map(|e| (e.frame.clone(), e.selector.clone(), e.style.clone()))
            .collect();
        let mut per_frame: Vec<(Frame, Vec<JsValue>)> = Vec::new();
        for (frame, selector, style) in &entries {
            let target = SelectorTarget::new(selector.clone(), false);
            let Ok(resolved) = resolve_frames_for_selector(frame, &target, false).await else {
                continue;
            };
            for r in resolved {
                let sel = match r.spec {
                    SelectorSpec::Raw(s) => JsValue::String(s),
                    SelectorSpec::Parsed(v) => v,
                };
                let item = JsValue::Object(vec![
                    ("sel".to_string(), sel),
                    (
                        "cssStyle".to_string(),
                        style
                            .clone()
                            .map(JsValue::String)
                            .unwrap_or(JsValue::Undefined),
                    ),
                ]);
                match per_frame.iter_mut().find(|(f, _)| *f == r.frame) {
                    Some((_, list)) => list.push(item),
                    None => per_frame.push((r.frame.clone(), vec![item])),
                }
            }
        }
        for frame in page.frames_dfs() {
            let list = per_frame
                .iter()
                .find(|(f, _)| *f == frame)
                .map(|(_, l)| l.clone());
            let ctx = match &list {
                Some(_) => frame.utility_context().await.ok(),
                None => frame.existing_context(crate::execution_context::World::Utility),
            };
            let Some(ctx) = ctx else {
                continue;
            };
            let _ = call_injected(
                &ctx,
                "(injected, list) => injected.setHighlights(list.map(e => ({ selector: typeof e.sel === 'string' ? injected.parseSelector(e.sel) : e.sel, cssStyle: e.cssStyle })))",
                vec![CallArg::Js(JsValue::Array(list.unwrap_or_default()))],
                true,
            )
            .await;
        }
        let has_entries = !page.ext.state.highlights.lock().unwrap().is_empty();
        if has_entries && !page.ext.state.highlight_timer.swap(true, Ordering::SeqCst) {
            let weak = Arc::downgrade(&page);
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    let Some(page) = weak.upgrade() else {
                        return;
                    };
                    let empty = page.ext.state.highlights.lock().unwrap().is_empty();
                    if empty || page.is_closed() {
                        page.ext
                            .state
                            .highlight_timer
                            .store(false, Ordering::SeqCst);
                        return;
                    }
                    resolve_highlights(&page).await;
                }
            });
        }
    })
}

// ----------------------------------------------------------------------
// Public API
// ----------------------------------------------------------------------

/// Builder returned by [`Page::add_locator_handler`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct AddLocatorHandler {
    page: Page,
    locator: Locator,
    handler: HandlerFn,
    no_wait_after: bool,
    times: Option<u32>,
}

impl AddLocatorHandler {
    /// Do not wait for the locator to be hidden after the handler ran
    /// (default `false`).
    pub fn no_wait_after(mut self, no_wait_after: bool) -> Self {
        self.no_wait_after = no_wait_after;
        self
    }

    /// Maximum number of times the handler is called (unlimited by default).
    pub fn times(mut self, times: u32) -> Self {
        self.times = Some(times);
        self
    }
}

impl IntoFuture for AddLocatorHandler {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            if *self.locator.frame() != self.page.main_frame() {
                return Err(Error::invalid_argument(
                    "Locator must belong to the main frame of this page",
                )
                .with_api("page.addLocatorHandler"));
            }
            if self.times == Some(0) {
                return Ok(());
            }
            self.page.inner.ext.state.register_handler(
                self.locator,
                self.handler,
                self.no_wait_after,
                self.times,
            );
            Ok(())
        })
    }
}

impl Page {
    /// Register a handler run when `locator` becomes visible, before
    /// actions and auto-waiting assertions (to dismiss overlays such as
    /// cookie banners). The handler receives the locator. Builder: set
    /// `.no_wait_after(..)` / `.times(..)`, then `.await` it.
    ///
    /// ```no_run
    /// # async fn run(page: oxidewright::Page) -> oxidewright::Result<()> {
    /// let p = page.clone();
    /// page.add_locator_handler(&page.get_by_text("Accept cookies"), move |_| {
    ///     let p = p.clone();
    ///     async move { p.locator("#accept").click().await }
    /// })
    /// .await?;
    /// # Ok(()) }
    /// ```
    pub fn add_locator_handler<F, Fut>(&self, locator: &Locator, handler: F) -> AddLocatorHandler
    where
        F: Fn(Locator) -> Fut + Send + Sync + 'static,
        Fut: Future + Send + 'static,
    {
        let handler: HandlerFn = Arc::new(move |locator| {
            let fut = handler(locator);
            Box::pin(async move {
                let _ = fut.await;
            })
        });
        AddLocatorHandler {
            page: self.clone(),
            locator: locator.clone(),
            handler,
            no_wait_after: false,
            times: None,
        }
    }

    /// Remove every locator handler added for `locator`.
    pub async fn remove_locator_handler(&self, locator: &Locator) -> Result<()> {
        self.inner.ext.state.unregister_handlers(locator);
        Ok(())
    }

    /// Console messages kept for this page (up to the last 200), since the
    /// last navigation of the main frame. Only filled when console events
    /// are enabled (see [`crate::NewContext::console_events`]).
    pub async fn console_messages(&self) -> Result<Vec<ConsoleMessage>> {
        self.console_messages_with(MessagesFilter::SinceNavigation)
            .await
    }

    /// Like [`Page::console_messages`] with a filter
    /// (`{ filter: 'all' | 'since-navigation' }`).
    pub async fn console_messages_with(
        &self,
        filter: MessagesFilter,
    ) -> Result<Vec<ConsoleMessage>> {
        self.inner.sync();
        Ok(self.inner.ext.state.console_messages(filter))
    }

    /// Forget the kept console messages.
    pub async fn clear_console_messages(&self) -> Result<()> {
        self.inner.sync();
        self.inner.ext.state.clear_console_messages();
        Ok(())
    }

    /// Uncaught errors kept for this page (up to the last 200), since the
    /// last navigation of the main frame. Only filled when console events
    /// are enabled (see [`crate::NewContext::console_events`]).
    pub async fn page_errors(&self) -> Result<Vec<WebError>> {
        self.page_errors_with(MessagesFilter::SinceNavigation).await
    }

    /// Like [`Page::page_errors`] with a filter.
    pub async fn page_errors_with(&self, filter: MessagesFilter) -> Result<Vec<WebError>> {
        self.inner.sync();
        Ok(self.inner.ext.state.page_errors(filter))
    }

    /// Forget the kept page errors.
    pub async fn clear_page_errors(&self) -> Result<()> {
        self.inner.sync();
        self.inner.ext.state.clear_page_errors();
        Ok(())
    }

    /// The last (up to 100) requests issued by the page.
    pub async fn requests(&self) -> Result<Vec<Request>> {
        self.inner.sync();
        Ok(self.inner.ext.state.requests())
    }

    /// Hide every highlight added with [`Locator::highlight`] on this page.
    pub async fn hide_highlight(&self) -> Result<()> {
        hide_highlights(&self.inner).await;
        Ok(())
    }
}
