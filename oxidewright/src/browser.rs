//! [`Browser`]: a launched or connected Chromium instance.

use std::collections::HashMap;
use std::fmt;
use std::future::{Future, IntoFuture};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::target::{
    CloseTargetParams, CreateBrowserContextParams, CreateTargetParams, EventTargetDestroyed,
    GetBrowserContextsParams, SessionId,
};
use chromiumoxide::types::Command;
use futures::StreamExt;
use tokio::task::JoinHandle;

use crate::browser_context::{BrowserContext, NewContext, NewContextOptions, NewPage};
use crate::error::{Error, Result, ResultExt};
use crate::events::{self, Event, EventKind, ListenerId, Listeners, WaitForEvent};
use crate::page::{Page, PageInner};

/// A browser instance. Cheap to clone.
#[derive(Clone)]
pub struct Browser {
    pub(crate) inner: Arc<BrowserInner>,
}

pub(crate) struct BrowserInner {
    cdp: tokio::sync::RwLock<Option<chromiumoxide::Browser>>,
    handler: Mutex<Option<JoinHandle<()>>>,
    handler_abort: Mutex<Option<tokio::task::AbortHandle>>,
    listener: Mutex<Option<JoinHandle<()>>>,
    contexts: Mutex<Vec<BrowserContext>>,
    default_context: Mutex<Option<BrowserContext>>,
    pages: Mutex<HashMap<String, Weak<PageInner>>>,
    /// Pages being adopted, so that a page is adopted once even when
    /// `new_page` and the target listener both see it.
    adoptions: Mutex<HashMap<String, Arc<tokio::sync::OnceCell<Page>>>>,
    connected: AtomicBool,
    launched: bool,
    version: String,
    profile_dir: Option<PathBuf>,
    /// Downloads and other extensions.
    pub(crate) ext: BrowserExt,
    self_weak: Weak<BrowserInner>,
    /// `disconnected` and `context` handlers.
    pub(crate) listeners: Listeners,
    /// Launched by `launch_persistent_context`: the default context is the
    /// persistent one, closing it closes the browser.
    pub(crate) persistent: AtomicBool,
    /// `slow_mo` of `launch`/`connect_over_cdp`, in ms.
    pub(crate) slow_mo: AtomicU64,
    /// `connect_over_cdp().no_defaults(true)`.
    pub(crate) no_defaults: AtomicBool,
}

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Run `fut`, then wait for the browser's `slow_mo`.
pub(crate) async fn with_slow_mo<T>(
    browser: Option<Arc<BrowserInner>>,
    fut: impl Future<Output = T>,
) -> T {
    let result = fut.await;
    if let Some(browser) = browser {
        browser.slow_mo().await;
    }
    result
}

/// Extra state of a browser.
#[derive(Default)]
pub(crate) struct BrowserExt {
    pub(crate) downloads: crate::download::BrowserDownloads,
    /// The launch proxy (its credentials answer contexts without their own).
    pub(crate) proxy: Mutex<Option<crate::ProxySettings>>,
}

impl fmt::Debug for Browser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Browser")
            .field("version", &self.inner.version)
            .field("connected", &self.is_connected())
            .finish()
    }
}

impl BrowserInner {
    /// Wrap a chromiumoxide browser and drive its handler.
    pub(crate) async fn start(
        cdp: chromiumoxide::Browser,
        mut handler: chromiumoxide::Handler,
        launched: bool,
        profile_dir: Option<PathBuf>,
    ) -> Result<Arc<BrowserInner>> {
        let handler_task = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if let Err(err) = event {
                    tracing::debug!("chromiumoxide handler error: {err}");
                }
            }
        });
        let version = match cdp.version().await {
            Ok(v) => v.product.rsplit('/').next().unwrap_or_default().to_string(),
            Err(err) => {
                handler_task.abort();
                return Err(Error::launch(format!(
                    "Failed to query the browser version: {err}"
                )));
            }
        };
        let destroyed = cdp.event_listener::<EventTargetDestroyed>().await.ok();
        let inner = Arc::new_cyclic(|weak| BrowserInner {
            cdp: tokio::sync::RwLock::new(Some(cdp)),
            handler: Mutex::new(None),
            handler_abort: Mutex::new(Some(handler_task.abort_handle())),
            listener: Mutex::new(None),
            contexts: Mutex::new(Vec::new()),
            default_context: Mutex::new(None),
            pages: Mutex::new(HashMap::new()),
            adoptions: Mutex::new(HashMap::new()),
            connected: AtomicBool::new(true),
            launched,
            version,
            profile_dir,
            ext: BrowserExt::default(),
            self_weak: weak.clone(),
            listeners: Listeners::new(),
            persistent: AtomicBool::new(false),
            slow_mo: AtomicU64::new(0),
            no_defaults: AtomicBool::new(false),
        });
        // Watch the handler: when it ends, the connection is gone.
        let weak = Arc::downgrade(&inner);
        let watcher = tokio::spawn(async move {
            let _ = handler_task.await;
            if let Some(inner) = weak.upgrade() {
                inner.on_disconnected();
            }
        });
        *inner.handler.lock().unwrap() = Some(watcher);
        if let Some(mut destroyed) = destroyed {
            let weak = Arc::downgrade(&inner);
            let listener = tokio::spawn(async move {
                while let Some(event) = destroyed.next().await {
                    let Some(inner) = weak.upgrade() else {
                        break;
                    };
                    let page = inner
                        .pages
                        .lock()
                        .unwrap()
                        .remove(event.target_id.as_ref())
                        .and_then(|p| p.upgrade());
                    if let Some(page) = page {
                        page.did_close();
                    }
                }
            });
            *inner.listener.lock().unwrap() = Some(listener);
        }
        if let Some(cdp) = inner.cdp.read().await.as_ref() {
            crate::download::start(&inner, cdp).await;
            crate::page_ext::start_target_listener(&inner, cdp).await;
        }
        Ok(inner)
    }

    pub(crate) fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    fn on_disconnected(&self) {
        if !self.connected.swap(false, Ordering::SeqCst) {
            return;
        }
        let pages: Vec<_> = self.pages.lock().unwrap().drain().collect();
        for (_, page) in pages {
            if let Some(page) = page.upgrade() {
                page.did_close();
            }
        }
        // Like Playwright, every context closes with the browser.
        let mut contexts: Vec<BrowserContext> = self.contexts.lock().unwrap().clone();
        contexts.extend(self.default_context.lock().unwrap().iter().cloned());
        for context in contexts {
            context.did_close_with_browser();
        }
        if let Some(inner) = self.self_weak.upgrade() {
            self.listeners.emit(Event::Disconnected(Browser { inner }));
        }
    }

    /// Sleep for `slow_mo` after an operation that Playwright slows down.
    pub(crate) async fn slow_mo(&self) {
        let ms = self.slow_mo.load(Ordering::Relaxed);
        if ms > 0 {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
    }

    /// Send a raw command on a session (`None` = browser session).
    pub(crate) async fn execute_with_session<C: Command>(
        &self,
        cmd: C,
        session: Option<SessionId>,
    ) -> Result<C::Response> {
        let guard = self.cdp.read().await;
        let browser = guard.as_ref().ok_or_else(Error::target_closed)?;
        Ok(browser.execute_with_session(cmd, session).await?.result)
    }

    pub(crate) async fn new_cdp_page(
        &self,
        params: CreateTargetParams,
    ) -> Result<chromiumoxide::Page> {
        let guard = self.cdp.read().await;
        let browser = guard.as_ref().ok_or_else(Error::target_closed)?;
        Ok(browser.new_page(params).await?)
    }

    /// The default context (if any) and the created contexts.
    pub(crate) fn all_contexts(&self) -> Vec<BrowserContext> {
        let mut out: Vec<_> = self
            .default_context
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect();
        out.extend(self.contexts.lock().unwrap().iter().cloned());
        out
    }

    pub(crate) fn remember_page(&self, page: &Page) {
        self.pages.lock().unwrap().insert(
            page.inner.cdp.target_id().as_ref().to_string(),
            Arc::downgrade(&page.inner),
        );
    }

    /// The open pages of the browser.
    pub(crate) fn live_pages(&self) -> Vec<Arc<PageInner>> {
        self.pages
            .lock()
            .unwrap()
            .values()
            .filter_map(Weak::upgrade)
            .collect()
    }

    /// The page of a target.
    /// The adoption slot of a target (see `adoptions`).
    pub(crate) fn adoption_cell(&self, target_id: &str) -> Arc<tokio::sync::OnceCell<Page>> {
        self.adoptions
            .lock()
            .unwrap()
            .entry(target_id.to_string())
            .or_default()
            .clone()
    }

    /// The adoption of a target is over (the page is in `pages` now).
    pub(crate) fn finish_adoption(&self, target_id: &str) {
        self.adoptions.lock().unwrap().remove(target_id);
    }

    /// The context owning pages of a CDP browser context: one created by
    /// `new_context`, else the default context when it is exposed
    /// (persistent or connected browsers).
    pub(crate) fn context_for_cdp_context(
        &self,
        id: Option<&chromiumoxide::cdp::browser_protocol::browser::BrowserContextId>,
    ) -> Option<BrowserContext> {
        if let Some(id) = id {
            let owned = self
                .contexts
                .lock()
                .unwrap()
                .iter()
                .find(|c| c.inner.id.as_ref() == Some(id))
                .cloned();
            if owned.is_some() {
                return owned;
            }
        }
        self.default_context.lock().unwrap().clone()
    }

    pub(crate) fn page_by_target(&self, target_id: &str) -> Option<Arc<PageInner>> {
        self.pages
            .lock()
            .unwrap()
            .get(target_id)
            .and_then(Weak::upgrade)
    }

    /// The chromiumoxide page of a target, if attached.
    pub(crate) async fn get_cdp_page(
        &self,
        target_id: chromiumoxide::cdp::browser_protocol::target::TargetId,
    ) -> Result<chromiumoxide::Page> {
        let guard = self.cdp.read().await;
        let browser = guard.as_ref().ok_or_else(Error::target_closed)?;
        Ok(browser.get_page(target_id).await?)
    }

    /// Read access to the chromiumoxide browser (`None` once closed).
    pub(crate) async fn cdp_read(
        &self,
    ) -> tokio::sync::RwLockReadGuard<'_, Option<chromiumoxide::Browser>> {
        self.cdp.read().await
    }

    pub(crate) fn forget_page(&self, target_id: &str) {
        self.pages.lock().unwrap().remove(target_id);
    }

    pub(crate) fn forget_context(&self, context: &BrowserContext) {
        self.contexts.lock().unwrap().retain(|c| c != context);
    }

    pub(crate) async fn create_context(
        &self,
        options: NewContextOptions,
    ) -> Result<BrowserContext> {
        if !self.is_connected() {
            return Err(Error::target_closed());
        }
        let mut params = CreateBrowserContextParams::default();
        params.dispose_on_detach = Some(true);
        let mut options = options;
        if let Some(proxy) = &options.proxy {
            let proxy = crate::proxy::normalize(proxy)?;
            let (server, bypass) = crate::proxy::context_params(&proxy);
            params.proxy_server = Some(server);
            params.proxy_bypass_list = bypass;
            options.proxy = Some(proxy);
        }
        let id = self
            .execute_with_session(params, None)
            .await?
            .browser_context_id;
        let context = BrowserContext::new(Some(id), self.self_weak.clone(), options);
        crate::download::configure_context(&context).await?;
        crate::har::start_context_recording(&context)?;
        self.contexts.lock().unwrap().push(context.clone());
        Ok(context)
    }

    /// Expose the default context (connected browsers) with its existing pages.
    pub(crate) async fn init_default_context(&self) -> Result<()> {
        self.adopt_default_context(NewContextOptions::no_overrides(), None)
            .await
            .map(|_| ())
    }

    /// Wrap the default browser context with `options` and adopt its
    /// pages. With `wait`, wait up to that long for its first page (the
    /// `about:blank` page of a persistent launch).
    pub(crate) async fn adopt_default_context(
        &self,
        options: NewContextOptions,
        wait: Option<Duration>,
    ) -> Result<BrowserContext> {
        let context = BrowserContext::new(None, self.self_weak.clone(), options);
        *self.default_context.lock().unwrap() = Some(context.clone());
        if !self.no_defaults.load(Ordering::SeqCst) {
            crate::download::configure_context(&context).await?;
        }
        if wait.is_some() {
            crate::har::start_context_recording(&context)?;
            crate::emulation::init_context(&context).await?;
        }
        let default_id = self
            .execute_with_session(GetBrowserContextsParams::default(), None)
            .await
            .ok()
            .and_then(|r| r.default_browser_context_id);
        let deadline = wait.map(|w| tokio::time::Instant::now() + w);
        let targets = loop {
            let targets = {
                let mut guard = self.cdp.write().await;
                let Some(browser) = guard.as_mut() else {
                    return Err(Error::target_closed());
                };
                browser.fetch_targets().await.unwrap_or_default()
            };
            let has_page = targets.iter().any(|t| {
                t.r#type == "page" && (default_id.is_none() || t.browser_context_id == default_id)
            });
            match deadline {
                Some(deadline) if !has_page && tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                _ => break targets,
            }
        };
        let mut pages = Vec::new();
        for target in targets {
            if target.r#type != "page" {
                continue;
            }
            if default_id.is_some() && target.browser_context_id != default_id {
                continue;
            }
            // Targets are attached asynchronously after `fetch_targets`
            // (wait longer for the startup page of a persistent launch).
            let attach_deadline = tokio::time::Instant::now()
                + deadline
                    .map(|d| d.saturating_duration_since(tokio::time::Instant::now()))
                    .unwrap_or_default()
                    .max(Duration::from_secs(1));
            loop {
                let page = {
                    let guard = self.cdp.read().await;
                    let Some(browser) = guard.as_ref() else {
                        return Err(Error::target_closed());
                    };
                    browser.get_page(target.target_id.clone()).await
                };
                if let Ok(page) = page {
                    pages.push(page);
                    break;
                }
                if tokio::time::Instant::now() >= attach_deadline {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        let mut stuck = Vec::new();
        for page in pages {
            let target_id = page.target_id().clone();
            // A startup page occasionally never finishes chromiumoxide's
            // attach; it is replaced below rather than blocking the launch.
            let adopt_timeout = Duration::from_secs(if wait.is_some() { 5 } else { 10 });
            match tokio::time::timeout(adopt_timeout, context.adopt_page(page)).await {
                Ok(Ok(_)) => {}
                Ok(Err(err)) => {
                    tracing::debug!("failed to adopt an existing page: {err}");
                    stuck.push(target_id);
                }
                Err(_) => {
                    tracing::debug!("timed out adopting an existing page");
                    stuck.push(target_id);
                }
            }
        }
        if wait.is_some() && !stuck.is_empty() {
            if context.pages().is_empty() {
                context.new_page().await?;
            }
            for target_id in stuck {
                let _ = self
                    .execute_with_session(CloseTargetParams::new(target_id), None)
                    .await;
            }
        }
        Ok(context)
    }
}

impl Browser {
    /// Browser contexts: the default context (connected browsers only) and
    /// the ones created with [`Browser::new_context`].
    pub fn contexts(&self) -> Vec<BrowserContext> {
        let mut out: Vec<_> = self
            .inner
            .default_context
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect();
        out.extend(self.inner.contexts.lock().unwrap().iter().cloned());
        out.retain(|c| !c.is_closed());
        out
    }

    /// Create a new isolated context.
    pub fn new_context(&self) -> NewContext {
        NewContext {
            browser: Some(self.clone()),
            options: NewContextOptions::default(),
        }
    }

    /// Create a page in a new context owned by the page: closing the page
    /// closes the context, and the context cannot open other pages. Accepts
    /// every option of [`Browser::new_context`]:
    ///
    /// ```no_run
    /// # async fn run(browser: oxidewright::Browser) -> oxidewright::Result<()> {
    /// let page = browser.new_page().locale("de-DE").await?;
    /// # let _ = page; Ok(()) }
    /// ```
    pub fn new_page(&self) -> NewPage {
        NewPage {
            context: self.new_context(),
        }
    }

    /// The [`crate::BrowserType`] the browser was launched or connected with.
    pub fn browser_type(&self) -> crate::BrowserType {
        crate::chromium()
    }

    /// Call `handler` for every event of `kind` (browser events:
    /// `context` and `disconnected`).
    pub fn on(
        &self,
        kind: EventKind,
        handler: impl Fn(Event) + Send + Sync + 'static,
    ) -> ListenerId {
        self.inner.listeners.add(kind, Arc::new(handler), false)
    }

    /// Like [`Browser::on`], for the next event only.
    pub fn once(
        &self,
        kind: EventKind,
        handler: impl Fn(Event) + Send + Sync + 'static,
    ) -> ListenerId {
        self.inner.listeners.add(kind, Arc::new(handler), true)
    }

    /// Remove a handler.
    pub fn remove_listener(&self, id: ListenerId) -> bool {
        self.inner.listeners.remove(id)
    }

    /// Remove every handler of `kind` (all kinds when `None`).
    pub fn remove_all_listeners(&self, kind: Option<EventKind>) {
        self.inner.listeners.remove_all(kind)
    }

    /// `context`: a context was created with [`Browser::new_context`] or
    /// [`Browser::new_page`].
    pub fn on_context(
        &self,
        handler: impl Fn(BrowserContext) + Send + Sync + 'static,
    ) -> ListenerId {
        self.on(EventKind::Context, move |event| {
            if let Event::Context(context) = event {
                handler(context)
            }
        })
    }

    /// `disconnected`: the browser closed, crashed, or the connection was
    /// lost (also after [`Browser::close`]).
    pub fn on_disconnected(&self, handler: impl Fn(Browser) + Send + Sync + 'static) -> ListenerId {
        self.on(EventKind::Disconnected, move |event| {
            if let Event::Disconnected(browser) = event {
                handler(browser)
            }
        })
    }

    /// Wait for the next event of `kind` (`context` or `disconnected`).
    pub fn wait_for_event(&self, kind: EventKind) -> WaitForEvent<Event> {
        WaitForEvent::new(
            events::Emitter::Browser(self.clone()),
            kind,
            "browser.waitForEvent",
            events::as_any,
        )
        .with_log(format!("waiting for event \"{}\"", kind.name()))
    }

    /// Wait for the next context.
    pub fn wait_for_context(&self) -> WaitForEvent<BrowserContext> {
        WaitForEvent::new(
            events::Emitter::Browser(self.clone()),
            EventKind::Context,
            "browser.waitForEvent",
            events::as_context,
        )
    }

    /// The browser version, e.g. `157.0.8103.52`.
    pub fn version(&self) -> &str {
        &self.inner.version
    }

    /// Whether the connection to the browser is alive.
    pub fn is_connected(&self) -> bool {
        self.inner.is_connected()
    }

    /// Run a closure with the underlying chromiumoxide browser.
    pub async fn with_cdp<R>(&self, f: impl FnOnce(&chromiumoxide::Browser) -> R) -> Result<R> {
        let guard = self.inner.cdp.read().await;
        let browser = guard.as_ref().ok_or_else(Error::target_closed)?;
        Ok(f(browser))
    }

    /// Close the browser (launched) or disconnect from it (connected);
    /// `.reason(..)` sets the message of errors of operations interrupted
    /// by the closure.
    pub fn close(&self) -> CloseBrowser {
        CloseBrowser {
            browser: self.clone(),
            reason: None,
        }
    }

    async fn close_impl(&self) -> Result<()> {
        async {
            if !self.inner.is_connected() && self.inner.cdp.read().await.is_none() {
                return Ok(());
            }
            let contexts: Vec<_> = self.inner.contexts.lock().unwrap().clone();
            if !self.inner.launched {
                for context in contexts {
                    let _ = context.close().await;
                }
            }
            let browser = self.inner.cdp.write().await.take();
            if let Some(mut browser) = browser {
                if self.inner.launched {
                    let _ = tokio::time::timeout(Duration::from_secs(5), browser.close()).await;
                    let waited =
                        tokio::time::timeout(Duration::from_secs(10), browser.wait()).await;
                    if waited.is_err() {
                        let _ = browser.kill().await;
                    }
                }
                drop(browser);
            }
            if let Some(task) = self.inner.handler_abort.lock().unwrap().take() {
                task.abort();
            }
            if let Some(task) = self.inner.listener.lock().unwrap().take() {
                task.abort();
            }
            self.inner.on_disconnected();
            if let Some(dir) = &self.inner.profile_dir {
                let _ = std::fs::remove_dir_all(dir);
            }
            Ok(())
        }
        .await
        .api("browser.close")
    }
}

/// Builder returned by [`Browser::close`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct CloseBrowser {
    browser: Browser,
    reason: Option<String>,
}

impl CloseBrowser {
    /// Reason reported to operations interrupted by the closure.
    pub fn reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}

impl IntoFuture for CloseBrowser {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            if let Some(reason) = &self.reason {
                for context in self.browser.contexts() {
                    let mut current = context.inner.emu.close_reason.lock().unwrap();
                    if current.is_none() {
                        *current = Some(reason.clone());
                    }
                }
            }
            self.browser.close_impl().await
        })
    }
}
