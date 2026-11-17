//! Downloads (`download` event). Each context gets
//! `Browser.setDownloadBehavior` with a temporary directory of the browser
//! (removed when the browser object is dropped); files are named by their
//! download GUID until saved with [`Download::save_as`].

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use chromiumoxide::cdp::browser_protocol::browser::{
    BrowserContextId, CancelDownloadParams, DownloadProgressState, EventDownloadProgress,
    EventDownloadWillBegin, SetDownloadBehaviorBehavior, SetDownloadBehaviorParams,
};
use futures::StreamExt;
use tokio::sync::watch;

use crate::browser::BrowserInner;
use crate::browser_context::BrowserContext;
use crate::error::{Error, Result, ResultExt};
use crate::events::Event;
use crate::page::Page;

const DENIED: &str = "Pass { acceptDownloads: true } when you are creating your browser context.";

/// Download bookkeeping of a browser.
pub(crate) struct BrowserDownloads {
    dir: Mutex<Option<PathBuf>>,
    downloads: Mutex<HashMap<String, Download>>,
}

impl Default for BrowserDownloads {
    fn default() -> Self {
        BrowserDownloads {
            dir: Mutex::new(None),
            downloads: Mutex::new(HashMap::new()),
        }
    }
}

impl Drop for BrowserDownloads {
    fn drop(&mut self) {
        if let Some(dir) = self.dir.lock().unwrap().take() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

impl BrowserDownloads {
    pub(crate) fn dir(&self) -> PathBuf {
        let mut dir = self.dir.lock().unwrap();
        if dir.is_none() {
            let name: String = std::iter::repeat_with(fastrand::alphanumeric)
                .take(12)
                .collect();
            let path = std::env::temp_dir().join(format!("oxidewright-downloads-{name}"));
            let _ = std::fs::create_dir_all(&path);
            *dir = Some(path);
        }
        dir.clone().unwrap()
    }
}

#[derive(Clone, Debug, PartialEq)]
enum DownloadState {
    InProgress,
    Done(Option<String>),
}

/// A download started by a page. Cheap to clone.
#[derive(Clone)]
pub struct Download {
    inner: Arc<DownloadInner>,
}

struct DownloadInner {
    page: Page,
    url: String,
    suggested_filename: String,
    guid: String,
    path: PathBuf,
    context_id: Option<BrowserContextId>,
    accepted: bool,
    browser: Weak<BrowserInner>,
    state: watch::Sender<DownloadState>,
}

impl fmt::Debug for Download {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Download")
            .field("url", &self.inner.url)
            .field("suggested_filename", &self.inner.suggested_filename)
            .finish()
    }
}

impl Download {
    /// The downloaded URL.
    pub fn url(&self) -> &str {
        &self.inner.url
    }

    /// The file name suggested by the server or the `download` attribute.
    pub fn suggested_filename(&self) -> &str {
        &self.inner.suggested_filename
    }

    /// The page that started the download.
    pub fn page(&self) -> Page {
        self.inner.page.clone()
    }

    async fn finished(&self) -> Option<String> {
        let mut rx = self.inner.state.subscribe();
        loop {
            if let DownloadState::Done(failure) = &*rx.borrow_and_update() {
                return failure.clone();
            }
            // Chrome reports no progress once the context or browser is gone,
            // and the sender lives in the download itself: without this check
            // `path()` / `failure()` would wait forever.
            let gone = self.inner.browser.upgrade().is_none()
                || self.inner.page.context().is_none_or(|c| c.is_closed());
            if gone {
                return Some("Target page, context or browser has been closed".into());
            }
            tokio::select! {
                changed = rx.changed() => if changed.is_err() {
                    return Some("Download was canceled".into());
                },
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
            }
        }
    }

    /// Wait for the download to finish and return the path of the file
    /// (in a temporary directory deleted with the browser).
    pub async fn path(&self) -> Result<PathBuf> {
        async {
            if !self.inner.accepted {
                return Err(Error::invalid_argument(DENIED));
            }
            if let Some(failure) = self.finished().await {
                return Err(Error::navigation(failure));
            }
            Ok(self.inner.path.clone())
        }
        .await
        .api("download.path")
    }

    /// Wait for the download to finish and copy the file to `path`.
    pub async fn save_as(&self, path: impl AsRef<Path>) -> Result<()> {
        let target = path.as_ref().to_path_buf();
        async {
            if !self.inner.accepted {
                return Err(Error::invalid_argument(DENIED));
            }
            if let Some(failure) = self.finished().await {
                return Err(Error::navigation(failure));
            }
            if let Some(parent) = target.parent() {
                if !parent.as_os_str().is_empty() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(|e| Error::invalid_argument(e.to_string()))?;
                }
            }
            tokio::fs::copy(&self.inner.path, &target)
                .await
                .map(|_| ())
                .map_err(|e| Error::invalid_argument(e.to_string()))
        }
        .await
        .api("download.saveAs")
    }

    /// Wait for the download to finish and open the file for reading
    /// (Playwright's `createReadStream`).
    pub async fn create_read_stream(&self) -> Result<tokio::fs::File> {
        async {
            if !self.inner.accepted {
                return Err(Error::invalid_argument(DENIED));
            }
            if let Some(failure) = self.finished().await {
                return Err(Error::navigation(failure));
            }
            tokio::fs::File::open(&self.inner.path)
                .await
                .map_err(|e| Error::invalid_argument(e.to_string()))
        }
        .await
        .api("download.createReadStream")
    }

    /// Wait for the download to finish; `Some(error)` if it failed (e.g.
    /// `canceled`).
    pub async fn failure(&self) -> Result<Option<String>> {
        if !self.inner.accepted {
            return Ok(Some(DENIED.to_string()));
        }
        Ok(self.finished().await)
    }

    /// Cancel the download (no-op if it already finished).
    pub async fn cancel(&self) -> Result<()> {
        async {
            if *self.inner.state.borrow() != DownloadState::InProgress {
                return Ok(());
            }
            let browser = self
                .inner
                .browser
                .upgrade()
                .ok_or_else(Error::target_closed)?;
            let mut params = CancelDownloadParams::new(self.inner.guid.clone());
            params.browser_context_id = self.inner.context_id.clone();
            browser.execute_with_session(params, None).await.map(|_| ())
        }
        .await
        .api("download.cancel")
    }

    /// Delete the downloaded file (waits for the download to finish).
    pub async fn delete(&self) -> Result<()> {
        if self.inner.accepted {
            let _ = self.finished().await;
            let _ = tokio::fs::remove_file(&self.inner.path).await;
        }
        Ok(())
    }
}

/// Apply the download behavior to a new context.
pub(crate) async fn configure_context(context: &BrowserContext) -> Result<()> {
    let Some(id) = context.inner.id.clone() else {
        return Ok(());
    };
    let browser = context
        .inner
        .browser
        .upgrade()
        .ok_or_else(Error::target_closed)?;
    let accept = context.inner.options.accept_downloads;
    let mut params = SetDownloadBehaviorParams::new(if accept {
        SetDownloadBehaviorBehavior::AllowAndName
    } else {
        SetDownloadBehaviorBehavior::Deny
    });
    params.browser_context_id = Some(id);
    params.download_path = Some(browser.ext.downloads.dir().to_string_lossy().into_owned());
    params.events_enabled = Some(true);
    browser.execute_with_session(params, None).await.map(|_| ())
}

/// Listen for download events of the browser.
pub(crate) async fn start(inner: &Arc<BrowserInner>, cdp: &chromiumoxide::Browser) {
    let (Ok(mut begin), Ok(mut progress)) = (
        cdp.event_listener::<EventDownloadWillBegin>().await,
        cdp.event_listener::<EventDownloadProgress>().await,
    ) else {
        return;
    };
    let weak = Arc::downgrade(inner);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                ev = begin.next() => {
                    let Some(ev) = ev else { break };
                    let Some(browser) = weak.upgrade() else { break };
                    on_will_begin(&browser, &ev);
                }
                ev = progress.next() => {
                    let Some(ev) = ev else { break };
                    let Some(browser) = weak.upgrade() else { break };
                    on_progress(&browser, &ev);
                }
            }
        }
    });
}

fn on_will_begin(browser: &Arc<BrowserInner>, ev: &EventDownloadWillBegin) {
    let Some(page) = browser
        .live_pages()
        .into_iter()
        .find(|p| p.frame_by_id(ev.frame_id.as_ref()).is_some())
    else {
        return;
    };
    let origin = match page.ext.opener() {
        // A download in a new window is reported on the opener when the
        // window has no document of its own.
        Some(opener) if page.main_frame().url().is_empty() => opener,
        _ => page,
    };
    let context = origin.context.upgrade();
    let accepted = context.as_ref().is_none_or(|c| c.options.accept_downloads);
    let download = Download {
        inner: Arc::new(DownloadInner {
            page: Page::from_inner(origin.clone()),
            url: ev.url.clone(),
            suggested_filename: ev.suggested_filename.clone(),
            guid: ev.guid.clone(),
            path: browser.ext.downloads.dir().join(&ev.guid),
            context_id: context.as_ref().and_then(|c| c.id.clone()),
            accepted,
            browser: Arc::downgrade(browser),
            state: watch::channel(DownloadState::InProgress).0,
        }),
    };
    browser
        .ext
        .downloads
        .downloads
        .lock()
        .unwrap()
        .insert(ev.guid.clone(), download.clone());
    origin.ext.listeners.emit(Event::Download(download.clone()));
    if let Some(context) = context {
        context.ext.listeners.emit(Event::Download(download));
    }
}

fn on_progress(browser: &Arc<BrowserInner>, ev: &EventDownloadProgress) {
    let state = match ev.state {
        DownloadProgressState::Completed => DownloadState::Done(None),
        DownloadProgressState::Canceled => DownloadState::Done(Some("canceled".into())),
        _ => return,
    };
    let download = browser
        .ext
        .downloads
        .downloads
        .lock()
        .unwrap()
        .remove(&ev.guid);
    if let Some(download) = download {
        download.inner.state.send_replace(state);
    }
}
