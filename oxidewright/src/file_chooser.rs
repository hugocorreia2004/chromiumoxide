//! File choosers (`filechooser` event), intercepted with
//! `Page.setInterceptFileChooserDialog` while someone listens for them.

use std::fmt;
use std::sync::Arc;

use chromiumoxide::cdp::browser_protocol::dom::ResolveNodeParams;
use chromiumoxide::cdp::browser_protocol::page::{
    EventFileChooserOpened, FileChooserOpenedMode, SetInterceptFileChooserDialogParams,
};

use crate::actions::{InputFiles, SetInputFiles};
use crate::error::{Error, Result};
use crate::events::{Event, EventKind};
use crate::js_handle::{ElementHandle, JSHandle};
use crate::page::{Page, PageInner};

/// A file chooser about to be opened by the page. Cheap to clone.
#[derive(Clone)]
pub struct FileChooser {
    inner: Arc<FileChooserInner>,
}

struct FileChooserInner {
    page: Page,
    element: ElementHandle,
    multiple: bool,
}

impl fmt::Debug for FileChooser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileChooser")
            .field("multiple", &self.inner.multiple)
            .finish()
    }
}

impl FileChooser {
    /// The `<input type=file>` element.
    pub fn element(&self) -> ElementHandle {
        self.inner.element.clone()
    }

    /// Whether the input accepts several files.
    pub fn is_multiple(&self) -> bool {
        self.inner.multiple
    }

    /// The page the file chooser belongs to.
    pub fn page(&self) -> Page {
        self.inner.page.clone()
    }

    /// Select files: local paths or in-memory [`crate::FilePayload`]s (see
    /// [`InputFiles`]); an empty list clears the selection. Same as
    /// `chooser.element().set_input_files(files)`, with its options:
    /// `.timeout(ms)` and `.no_wait_after(bool)` before `.await`.
    pub fn set_files(&self, files: impl Into<InputFiles>) -> SetFiles {
        let files = files.into();
        let count = match &files {
            InputFiles::Paths(p) => p.len(),
            InputFiles::Payloads(p) => p.len(),
        };
        let error = (count > 1 && !self.inner.multiple).then(|| {
            Error::invalid_argument("Non-multiple file input can only accept single file")
        });
        let mut action = self.inner.element.set_input_files(files);
        action.api = "fileChooser.setFiles";
        SetFiles { action, error }
    }
}

/// Builder returned by [`FileChooser::set_files`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct SetFiles {
    action: SetInputFiles,
    error: Option<Error>,
}

impl SetFiles {
    /// Maximum time in milliseconds (default: the page's default timeout).
    pub fn timeout(mut self, timeout_ms: u64) -> Self {
        self.action = self.action.timeout(timeout_ms);
        self
    }

    /// Accepted for Playwright compatibility (deprecated there, no effect).
    pub fn no_wait_after(mut self, no_wait_after: bool) -> Self {
        self.action = self.action.no_wait_after(no_wait_after);
        self
    }
}

impl std::future::IntoFuture for SetFiles {
    type Output = Result<()>;
    type IntoFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            if let Some(error) = self.error {
                return Err(error.with_api("fileChooser.setFiles"));
            }
            self.action.await
        })
    }
}

/// Enable or disable file chooser interception according to the listeners.
pub(crate) async fn update_interception(page: &Arc<PageInner>) {
    let wanted = page.ext.listeners.count(EventKind::FileChooser) > 0;
    let previous = page
        .ext
        .file_chooser_intercepted
        .swap(wanted, std::sync::atomic::Ordering::SeqCst);
    if previous == wanted {
        return;
    }
    let _ = page
        .cdp
        .execute(SetInterceptFileChooserDialogParams::new(wanted))
        .await;
}

/// `Page.fileChooserOpened`.
pub(crate) fn on_file_chooser_opened(page: &PageInner, ev: &EventFileChooserOpened) {
    let Some(backend_node_id) = ev.backend_node_id else {
        return;
    };
    let Some(frame) = page.frame_by_id(ev.frame_id.as_ref()) else {
        return;
    };
    let Some(page_arc) = page.self_weak.upgrade() else {
        return;
    };
    let multiple = ev.mode == FileChooserOpenedMode::SelectMultiple;
    tokio::spawn(async move {
        let Ok(context) = frame.utility_context().await else {
            return;
        };
        let mut params = ResolveNodeParams::default();
        params.backend_node_id = Some(backend_node_id);
        params.execution_context_id = context.inner.context_id;
        let Ok(resolved) = context.send(params).await else {
            return;
        };
        let handle = JSHandle::from_remote(context, resolved.object);
        let Some(element) = handle.as_element() else {
            return;
        };
        let chooser = FileChooser {
            inner: Arc::new(FileChooserInner {
                page: Page::from_inner(page_arc.clone()),
                element,
                multiple,
            }),
        };
        page_arc.ext.listeners.emit(Event::FileChooser(chooser));
    });
}
