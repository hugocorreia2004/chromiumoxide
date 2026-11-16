//! [`Browser`]: a launched or connected Chromium instance.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::target::{
    CreateBrowserContextParams, CreateTargetParams, EventTargetDestroyed, GetBrowserContextsParams,
    SessionId,
};
use chromiumoxide::types::Command;
use futures::StreamExt;
use tokio::task::JoinHandle;

use crate::browser_context::{BrowserContext, NewContext, NewContextOptions};
use crate::error::{Error, Result, ResultExt};
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
    connected: AtomicBool,
    launched: bool,
    version: String,
    profile_dir: Option<PathBuf>,
    self_weak: Weak<BrowserInner>,
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
            connected: AtomicBool::new(true),
            launched,
            version,
            profile_dir,
            self_weak: weak.clone(),
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

    pub(crate) fn remember_page(&self, page: &Page) {
        self.pages.lock().unwrap().insert(
            page.inner.cdp.target_id().as_ref().to_string(),
            Arc::downgrade(&page.inner),
        );
    }

    /// The page of a target, if it is known.
    pub(crate) fn page_by_target_id(&self, target_id: &str) -> Option<Page> {
        self.pages
            .lock()
            .unwrap()
            .get(target_id)
            .and_then(Weak::upgrade)
            .map(Page::from_inner)
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
        let id = self
            .execute_with_session(params, None)
            .await?
            .browser_context_id;
        let context = BrowserContext::new(Some(id), self.self_weak.clone(), options);
        self.contexts.lock().unwrap().push(context.clone());
        Ok(context)
    }

    /// Expose the default context (connected browsers) with its existing pages.
    pub(crate) async fn init_default_context(&self) -> Result<()> {
        let context = BrowserContext::new(
            None,
            self.self_weak.clone(),
            NewContextOptions::no_overrides(),
        );
        *self.default_context.lock().unwrap() = Some(context.clone());
        let default_id = self
            .execute_with_session(GetBrowserContextsParams::default(), None)
            .await
            .ok()
            .and_then(|r| r.default_browser_context_id);
        let targets = {
            let mut guard = self.cdp.write().await;
            let Some(browser) = guard.as_mut() else {
                return Err(Error::target_closed());
            };
            browser.fetch_targets().await.unwrap_or_default()
        };
        let mut pages = Vec::new();
        for target in targets {
            if target.r#type != "page" {
                continue;
            }
            if default_id.is_some() && target.browser_context_id != default_id {
                continue;
            }
            // Targets are attached asynchronously after `fetch_targets`.
            for _ in 0..50 {
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
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        for page in pages {
            match tokio::time::timeout(Duration::from_secs(10), context.adopt_page(page)).await {
                Ok(Ok(_)) => {}
                Ok(Err(err)) => tracing::debug!("failed to adopt an existing page: {err}"),
                Err(_) => tracing::debug!("timed out adopting an existing page"),
            }
        }
        Ok(())
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
        out
    }

    /// Create a new isolated context.
    pub fn new_context(&self) -> NewContext {
        NewContext {
            browser: self.clone(),
            options: NewContextOptions::default(),
        }
    }

    /// Create a page in a new context owned by the page (closing the page
    /// does not close the context; closing the browser does).
    pub async fn new_page(&self) -> Result<Page> {
        async {
            let context = self
                .inner
                .create_context(NewContextOptions::default())
                .await?;
            context.new_page().await
        }
        .await
        .api("browser.newPage")
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

    /// Close the browser (launched) or disconnect from it (connected).
    pub async fn close(&self) -> Result<()> {
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
