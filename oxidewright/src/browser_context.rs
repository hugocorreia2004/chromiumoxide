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

/// Options of [`crate::Browser::new_context`] (Playwright's
/// `BrowserContextOptions`). Options left unset are not emulated, see
/// [`crate::emulation`] about stealth. Builder methods live on
/// [`NewContext`].
#[derive(Debug, Clone)]
pub struct NewContextOptions {
    /// Viewport of every page (default 1280x720); `None` keeps the window size.
    pub viewport: Option<ViewportSize>,
    /// Base URL used to resolve relative URLs of `goto`, `wait_for_url`, ...
    pub base_url: Option<String>,
    /// Screen size exposed to pages (`window.screen`); defaults to the viewport.
    pub screen: Option<ViewportSize>,
    /// Device scale factor (`window.devicePixelRatio`), default 1.
    pub device_scale_factor: Option<f64>,
    /// Emulate a mobile device (meta viewport, overlay scrollbars...).
    pub is_mobile: Option<bool>,
    /// Emulate touch support.
    pub has_touch: Option<bool>,
    /// User agent override.
    pub user_agent: Option<String>,
    /// Locale (`navigator.language`, `Accept-Language`, `Intl`), e.g. `de-DE`.
    pub locale: Option<String>,
    /// Timezone, e.g. `Europe/Lisbon`.
    pub timezone_id: Option<String>,
    /// Emulated geolocation.
    pub geolocation: Option<crate::emulation::Geolocation>,
    /// Permissions granted to every origin.
    pub permissions: Option<Vec<String>>,
    /// `prefers-color-scheme`.
    pub color_scheme: Option<crate::emulation::ColorScheme>,
    /// `prefers-reduced-motion`.
    pub reduced_motion: Option<crate::emulation::ReducedMotion>,
    /// `forced-colors`.
    pub forced_colors: Option<crate::emulation::ForcedColors>,
    /// `prefers-contrast`.
    pub contrast: Option<crate::emulation::Contrast>,
    /// `Some(false)` disables JavaScript.
    pub java_script_enabled: Option<bool>,
    /// Bypass the page's Content-Security-Policy.
    pub bypass_csp: bool,
    /// Ignore HTTPS errors (pages and `context.request()`).
    pub ignore_https_errors: bool,
    /// Accept downloads (default `true`); `None` keeps the browser's behavior.
    pub accept_downloads: Option<bool>,
    /// Make locators strict by default.
    pub strict_selectors: bool,
    /// Whether pages may register service workers.
    pub service_workers: crate::emulation::ServiceWorkerPolicy,
    /// Initial cookies and storage.
    pub storage_state: Option<crate::storage_state::StorageStateSource>,
    /// Record a video of every page into this directory (requires ffmpeg).
    pub record_video_dir: Option<std::path::PathBuf>,
    /// Size of the recorded videos.
    pub record_video_size: Option<ViewportSize>,
}

impl Default for NewContextOptions {
    fn default() -> Self {
        Self {
            viewport: Some(ViewportSize {
                width: 1280,
                height: 720,
            }),
            accept_downloads: Some(true),
            ..Self::no_overrides()
        }
    }
}

impl NewContextOptions {
    /// Options that emulate nothing, used for the default context of a
    /// browser attached with `connect_over_cdp`.
    pub fn no_overrides() -> Self {
        Self {
            viewport: None,
            base_url: None,
            screen: None,
            device_scale_factor: None,
            is_mobile: None,
            has_touch: None,
            user_agent: None,
            locale: None,
            timezone_id: None,
            geolocation: None,
            permissions: None,
            color_scheme: None,
            reduced_motion: None,
            forced_colors: None,
            contrast: None,
            java_script_enabled: None,
            bypass_csp: false,
            ignore_https_errors: false,
            accept_downloads: None,
            strict_selectors: false,
            service_workers: Default::default(),
            storage_state: None,
            record_video_dir: None,
            record_video_size: None,
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
    pages: Mutex<Vec<Page>>,
    init_scripts: Mutex<Vec<String>>,
    closed: AtomicBool,
    pub(crate) ext: crate::ext::ContextExt,
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
                options,
                timeouts: Arc::new(TimeoutSettings::new(None)),
                pages: Mutex::new(Vec::new()),
                init_scripts: Mutex::new(Vec::new()),
                closed: AtomicBool::new(false),
                ext: Default::default(),
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
        let browser = self.browser_inner()?;
        let inner = PageInner::attach(cdp, Arc::downgrade(&browser), self).await?;
        let page = Page::from_inner(inner);
        crate::emulation::init_page(&page).await?;
        crate::screencast::on_page_created(&page).await?;
        let scripts = self.inner.init_scripts.lock().unwrap().clone();
        for script in scripts {
            page.inner.add_init_script_source(script).await?;
        }
        browser.remember_page(&page);
        self.inner.pages.lock().unwrap().push(page.clone());
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
            // Each page gets its own window (like Playwright's pages): a background
            // tab of a shared window would not render, so screenshots would stall.
            params.new_window = Some(true);
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

    /// Close the context and all its pages; `.reason(..)` sets the message
    /// of errors of operations interrupted by the closure.
    pub fn close(&self) -> crate::page_extras::CloseContext {
        crate::page_extras::CloseContext {
            context: self.clone(),
            reason: None,
        }
    }

    pub(crate) async fn close_impl(&self) -> Result<()> {
        crate::video::on_context_closing(self).await;
        async {
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
            Ok(())
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
            async {
                crate::emulation::validate_options(&self.options)?;
                crate::video::validate_options(&self.options)?;
                let context = self.browser.inner.create_context(self.options).await?;
                if let Err(err) = crate::emulation::init_context(&context).await {
                    let _ = context.close().await;
                    return Err(err);
                }
                Ok(context)
            }
            .await
            .api("browser.newContext")
        })
    }
}
