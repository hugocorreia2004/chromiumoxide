//! File choosers (`filechooser` event), intercepted with
//! `Page.setInterceptFileChooserDialog` while someone listens for them.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use chromiumoxide::cdp::browser_protocol::dom::{
    BackendNodeId, ResolveNodeParams, SetFileInputFilesParams,
};
use chromiumoxide::cdp::browser_protocol::page::{
    EventFileChooserOpened, FileChooserOpenedMode, SetInterceptFileChooserDialogParams,
};

use crate::error::{Error, Result, ResultExt};
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
    backend_node_id: BackendNodeId,
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

    /// Select files (paths are made absolute). An empty list clears the
    /// selection.
    pub async fn set_files(
        &self,
        files: impl IntoIterator<Item = impl Into<PathBuf>>,
    ) -> Result<()> {
        async {
            let files: Vec<PathBuf> = files.into_iter().map(Into::into).collect();
            if files.len() > 1 && !self.inner.multiple {
                return Err(Error::invalid_argument(
                    "Non-multiple file input can only accept single file",
                ));
            }
            let mut paths = Vec::new();
            for file in files {
                let absolute = if file.is_absolute() {
                    file
                } else {
                    std::env::current_dir()
                        .map_err(|e| Error::invalid_argument(e.to_string()))?
                        .join(file)
                };
                paths.push(absolute.to_string_lossy().into_owned());
            }
            let mut params = SetFileInputFilesParams::new(paths);
            params.backend_node_id = Some(self.inner.backend_node_id);
            self.inner
                .element
                .as_js_handle()
                .context()
                .send(params)
                .await
                .map(|_| ())
        }
        .await
        .api("fileChooser.setFiles")
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
                backend_node_id,
                multiple,
            }),
        };
        page_arc.ext.listeners.emit(Event::FileChooser(chooser));
    });
}
