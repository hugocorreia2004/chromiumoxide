//! `locator.drop()` (Playwright 1.65): an external drag-and-drop of files
//! and/or clipboard-like data, a port of `Frame.drop` / `ElementHandle._drop`.
//!
//! The `DataTransfer` and drag events are built in the isolated world (the
//! page only sees the dispatched events, like with a real drop).

use std::future::{Future, IntoFuture};
use std::pin::Pin;

use base64::Engine;
use serde_json::{Value, json};

use crate::actions::{
    Action, InputFiles, Position, kinds, mime_type_for, prepare_input_files, run_task,
};
use crate::element_actions::{Perform, PointerAction, PointerOptions, Task, WaitAfter};
use crate::error::{Error, Result};
use crate::locator::Locator;

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// What [`Locator::drop`] drops: files (paths or in-memory
/// [`crate::FilePayload`]s) and/or data entries keyed by mime type.
#[derive(Debug, Clone, Default)]
pub struct DropPayload {
    files: Option<InputFiles>,
    data: Vec<(String, String)>,
}

impl DropPayload {
    /// An empty payload (at least one of `files` / `data` is required).
    pub fn new() -> Self {
        Self::default()
    }

    /// The files to drop.
    pub fn files(mut self, files: impl Into<InputFiles>) -> Self {
        self.files = Some(files.into());
        self
    }

    /// A data entry, e.g. `("text/plain", "hello")` or
    /// `("text/uri-list", "https://example.com")`.
    pub fn data(mut self, mime_type: impl Into<String>, value: impl Into<String>) -> Self {
        self.data.push((mime_type.into(), value.into()));
        self
    }
}

/// The drop itself, run at the action point of the element.
pub(crate) const DROP_JS: &str = r#"(injected, node, { payloads, data, point }) => {
  if (!node.isConnected || node.nodeType !== 1)
    return 'error:notconnected';
  const element = node;
  const dt = new DataTransfer();
  for (const p of payloads) {
    const bytes = Uint8Array.from(atob(p.buffer), c => c.charCodeAt(0));
    const file = new File([bytes], p.name, { type: p.mimeType, lastModified: p.lastModifiedMs });
    dt.items.add(file);
  }
  for (const entry of data)
    dt.setData(entry.mimeType, entry.value);
  const makeEvent = type => new DragEvent(type, {
    bubbles: true,
    cancelable: true,
    composed: true,
    clientX: point.x,
    clientY: point.y,
    dataTransfer: dt,
  });
  element.dispatchEvent(makeEvent('dragenter'));
  const over = makeEvent('dragover');
  element.dispatchEvent(over);
  if (!over.defaultPrevented) {
    element.dispatchEvent(makeEvent('dragleave'));
    return 'not-accepted';
  }
  element.dispatchEvent(makeEvent('drop'));
  return 'accepted';
}"#;

/// The file payloads of a drop (local files are read here).
fn file_payloads(files: &InputFiles) -> Result<Vec<Value>> {
    let items = prepare_input_files(files)?;
    if items.local_directory.is_some() {
        return Err(Error::invalid_argument(
            "Dropping a directory is not supported \u{2014} pass individual files.",
        ));
    }
    if let Some(payloads) = items.payloads {
        return Ok(payloads);
    }
    let mut out = Vec::new();
    for path in items.local_paths.unwrap_or_default() {
        let bytes = std::fs::read(&path).map_err(|e| Error::invalid_argument(e.to_string()))?;
        let name = std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let modified = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as f64);
        out.push(json!({
            "name": name,
            "mimeType": mime_type_for(&name),
            "buffer": base64::engine::general_purpose::STANDARD.encode(bytes),
            "lastModifiedMs": modified,
        }));
    }
    Ok(out)
}

impl Locator {
    /// Simulate an external drag-and-drop of files and/or data onto the
    /// element: dispatches `dragenter`, `dragover` and `drop` with a
    /// `DataTransfer` carrying the payload. Fails when the element's
    /// `dragover` handler does not call `preventDefault()`. Builder:
    /// `.position(..)`, `.timeout(..)`, then `.await` it.
    ///
    /// ```no_run
    /// # async fn run(page: oxidewright::Page) -> oxidewright::Result<()> {
    /// use oxidewright::{DropPayload, FilePayload};
    /// page.locator("#dropzone")
    ///     .drop(DropPayload::new().files(FilePayload::new("note.txt", "text/plain", b"hello".to_vec())))
    ///     .await?;
    /// # Ok(()) }
    /// ```
    pub fn drop(&self, payload: DropPayload) -> Action<kinds::Drop> {
        let mut action: Action<kinds::Drop> = self.action_for("locator.drop");
        action.args.files = payload.files;
        action.args.other = Some(serde_json::to_string(&payload.data).unwrap_or_default());
        action
    }
}

impl Action<kinds::Drop> {
    /// The point to drop at, relative to the top-left corner of the
    /// element's padding box (default: a visible point of the element).
    pub fn position(mut self, position: impl Into<Position>) -> Self {
        self.opts.position = Some(position.into());
        self
    }
}

impl IntoFuture for Action<kinds::Drop> {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let api = self.api;
            let data: Vec<(String, String)> = self
                .args
                .other
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            let payloads = match &self.args.files {
                Some(files) => file_payloads(files).map_err(|e| e.with_api(api))?,
                None => Vec::new(),
            };
            if payloads.is_empty() && data.is_empty() {
                return Err(Error::invalid_argument(
                    "At least one of \"files\" or \"data\" must be provided.",
                )
                .with_api(api));
            }
            let data: Vec<Value> = data
                .into_iter()
                .map(|(mime_type, value)| json!({ "mimeType": mime_type, "value": value }))
                .collect();
            let pointer = PointerAction {
                name: "drop",
                wait_for_enabled: false,
                wait_after: WaitAfter::Disabled,
                skip_action_pre_checks: false,
                perform: Perform::Drop { payloads, data },
            };
            let options = PointerOptions {
                position: self.opts.position,
                ..Default::default()
            };
            run_task(self, Task::Pointer(pointer, options), false, true)
                .await
                .map(|_| ())
        })
    }
}
