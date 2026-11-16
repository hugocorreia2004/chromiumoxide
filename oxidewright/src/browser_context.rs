//! [`BrowserContext`]: an isolated browser session (cookies, storage, pages).

use std::fmt;
use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use chromiumoxide::cdp::browser_protocol::browser::BrowserContextId;
use chromiumoxide::cdp::browser_protocol::page::ScriptIdentifier;
use chromiumoxide::cdp::browser_protocol::target::{
    CreateTargetParams, DisposeBrowserContextParams,
};

use crate::browser::{Browser, BrowserInner};
use crate::error::{Error, Result, ResultExt};
use crate::page::{AddInitScript, InitScriptTarget, Page, PageInner, ViewportSize};
use crate::progress::TimeoutSettings;

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Options of [`crate::Browser::new_context`].
#[derive(Debug, Clone)]
pub struct NewContextOptions {
    /// Viewport of every page (default 1280x720); `None` keeps the window size.
    pub viewport: Option<ViewportSize>,
    /// Base URL used to resolve relative URLs of `goto`, `wait_for_url`, ...
    pub base_url: Option<String>,
    /// Extra HTTP headers sent with every request.
    pub extra_http_headers: Vec<crate::Header>,
    /// Emulate a disconnected network.
    pub offline: bool,
    /// Credentials for HTTP authentication.
    pub http_credentials: Option<crate::HttpCredentials>,
    /// Ignore HTTPS errors.
    pub ignore_https_errors: bool,
    /// Accept downloads (default `true`).
    pub accept_downloads: bool,
    /// Record a HAR file.
    pub record_har: Option<crate::RecordHar>,
    /// Report `console`/`pageerror` events (enables `Runtime`, not stealthy).
    pub console_events: bool,
}

impl Default for NewContextOptions {
    fn default() -> Self {
        Self {
            viewport: Some(ViewportSize {
                width: 1280,
                height: 720,
            }),
            base_url: None,
            extra_http_headers: Vec::new(),
            offline: false,
            http_credentials: None,
            ignore_https_errors: false,
            accept_downloads: true,
            record_har: None,
            console_events: false,
        }
    }
}

/// An isolated browser context. Cheap to clone.
#[derive(Clone)]
pub struct BrowserContext {
    pub(crate) inner: Arc<ContextInner>,
}

pub(crate) struct ContextInner {
    pub(crate) id: Option<BrowserContextId>,
    pub(crate) browser: Weak<BrowserInner>,
    pub(crate) options: NewContextOptions,
    pub(crate) timeouts: Arc<TimeoutSettings>,
    /// Events, routes, network options (see `context_ext.rs`).
    pub(crate) ext: crate::context_ext::ContextExt,
    pages: Mutex<Vec<Page>>,
    init_scripts: Mutex<Vec<String>>,
    closed: AtomicBool,
}

impl fmt::Debug for BrowserContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BrowserContext")
            .field("id", &self.inner.id)
            .finish()
    }
}

impl PartialEq for BrowserContext {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl ContextInner {
    pub(crate) fn remove_page(&self, page: &PageInner) {
        self.pages
            .lock()
            .unwrap()
            .retain(|p| !std::ptr::eq(Arc::as_ptr(&p.inner), page));
    }
}

impl BrowserContext {
    pub(crate) fn new(
        id: Option<BrowserContextId>,
        browser: Weak<BrowserInner>,
        options: NewContextOptions,
    ) -> Self {
        BrowserContext {
            inner: Arc::new(ContextInner {
                id,
                browser,
                ext: crate::context_ext::ContextExt::new(&options),
                options,
                timeouts: Arc::new(TimeoutSettings::new(None)),
                pages: Mutex::new(Vec::new()),
                init_scripts: Mutex::new(Vec::new()),
                closed: AtomicBool::new(false),
            }),
        }
    }

    fn browser_inner(&self) -> Result<Arc<BrowserInner>> {
        self.inner
            .browser
            .upgrade()
            .ok_or_else(Error::target_closed)
    }

    /// The browser owning this context.
    pub fn browser(&self) -> Option<Browser> {
        self.inner.browser.upgrade().map(|inner| Browser { inner })
    }

    /// Open pages of the context.
    pub fn pages(&self) -> Vec<Page> {
        self.inner
            .pages
            .lock()
            .unwrap()
            .iter()
            .filter(|p| !p.is_closed())
            .cloned()
            .collect()
    }

    /// Wrap a chromiumoxide page of this context and track it.
    pub(crate) async fn adopt_page(&self, cdp: chromiumoxide::Page) -> Result<Page> {
        self.adopt_page_with_opener(cdp, None).await
    }

    /// Like `adopt_page`, for a popup of `opener`.
    pub(crate) async fn adopt_page_with_opener(
        &self,
        cdp: chromiumoxide::Page,
        opener: Option<Weak<PageInner>>,
    ) -> Result<Page> {
        let browser = self.browser_inner()?;
        let inner = PageInner::attach(cdp, Arc::downgrade(&browser), self).await?;
        *inner.ext.opener.lock().unwrap() = opener;
        let page = Page::from_inner(inner);
        if let Some(viewport) = self.inner.options.viewport {
            page.inner.apply_viewport(viewport).await?;
        }
        let scripts = self.inner.init_scripts.lock().unwrap().clone();
        for script in scripts {
            page.inner.add_init_script_source(script).await?;
        }
        crate::page_ext::initialize(&page, self).await?;
        browser.remember_page(&page);
        self.inner.pages.lock().unwrap().push(page.clone());
        self.inner
            .ext
            .listeners
            .emit(crate::events::Event::Page(page.clone()));
        Ok(page)
    }

    /// Open a new page in this context.
    pub async fn new_page(&self) -> Result<Page> {
        async {
            if self.inner.closed.load(Ordering::SeqCst) {
                return Err(Error::target_closed());
            }
            let browser = self.browser_inner()?;
            let mut params = CreateTargetParams::new("about:blank");
            params.browser_context_id = self.inner.id.clone();
            let cdp = browser.new_cdp_page(params).await?;
            self.adopt_page(cdp).await
        }
        .await
        .api("browserContext.newPage")
    }

    /// Default timeout (ms) of actions of every page of the context.
    pub fn set_default_timeout(&self, timeout_ms: u64) {
        self.inner.timeouts.set_default_timeout(timeout_ms);
    }

    /// Default timeout (ms) of navigations of every page of the context.
    pub fn set_default_navigation_timeout(&self, timeout_ms: u64) {
        self.inner
            .timeouts
            .set_default_navigation_timeout(timeout_ms);
    }

    /// Add a script evaluated in every new document of every page.
    pub fn add_init_script(&self, script: impl Into<String>) -> AddInitScript {
        AddInitScript {
            target: InitScriptTarget::Context(self.clone()),
            script: script.into(),
            arg: None,
        }
    }

    pub(crate) async fn add_init_script_source(
        &self,
        source: String,
    ) -> Result<Vec<(Weak<PageInner>, ScriptIdentifier)>> {
        self.inner.init_scripts.lock().unwrap().push(source.clone());
        let mut ids = Vec::new();
        for page in self.pages() {
            ids.push(page.inner.add_init_script_source(source.clone()).await?);
        }
        Ok(ids)
    }

    pub(crate) fn remove_init_script_source(&self, source: &str) {
        let mut scripts = self.inner.init_scripts.lock().unwrap();
        if let Some(index) = scripts.iter().position(|s| s == source) {
            scripts.remove(index);
        }
    }

    /// Whether the context has been closed.
    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst)
    }

    /// Close the context and all its pages.
    pub async fn close(&self) -> Result<()> {
        async {
            if self.inner.closed.load(Ordering::SeqCst) {
                return Ok(());
            }
            let har = crate::har::flush_context(self).await;
            if self.inner.closed.swap(true, Ordering::SeqCst) {
                return Ok(());
            }
            let pages = std::mem::take(&mut *self.inner.pages.lock().unwrap());
            let browser = self.inner.browser.upgrade();
            match (&self.inner.id, &browser) {
                (Some(id), Some(browser)) if browser.is_connected() => {
                    let _ = browser
                        .execute_with_session(DisposeBrowserContextParams::new(id.clone()), None)
                        .await;
                }
                _ => {
                    for page in &pages {
                        let _ = page.close().await;
                    }
                }
            }
            for page in &pages {
                page.inner.did_close();
            }
            if let Some(browser) = browser {
                browser.forget_context(self);
            }
            self.emit_closed();
            har
        }
        .await
        .api("browserContext.close")
    }
}

/// Builder returned by [`crate::Browser::new_context`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct NewContext {
    pub(crate) browser: Browser,
    pub(crate) options: NewContextOptions,
}

impl NewContext {
    /// Viewport of every page; `None` disables viewport emulation.
    pub fn viewport(mut self, viewport: Option<ViewportSize>) -> Self {
        self.options.viewport = viewport;
        self
    }

    /// Base URL used to resolve relative URLs.
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.options.base_url = Some(base_url.into());
        self
    }

    /// Replace all options at once.
    pub fn options(mut self, options: NewContextOptions) -> Self {
        self.options = options;
        self
    }
}

impl IntoFuture for NewContext {
    type Output = Result<BrowserContext>;
    type IntoFuture = BoxFuture<Result<BrowserContext>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            self.browser
                .inner
                .create_context(self.options)
                .await
                .api("browser.newContext")
        })
    }
}
