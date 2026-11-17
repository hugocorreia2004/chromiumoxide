//! Actions and queries on elements: the builders returned by
//! `locator.click()`, `page.fill(selector, value)`, `element_handle.hover()`,
//! ... and the value types they use.
//!
//! Every builder is an [`Action`]; option methods are only available on the
//! actions that accept them, and `.await` runs the action:
//!
//! ```no_run
//! # async fn run(page: oxidewright::Page) -> oxidewright::Result<()> {
//! use oxidewright::{KeyboardModifier, MouseButton};
//!
//! page.locator("button").click().button(MouseButton::Right).await?;
//! page.click("#submit").modifiers([KeyboardModifier::Shift]).timeout(5_000).await?;
//! let text = page.locator("h1").text_content().await?;
//! # let _ = text; Ok(()) }
//! ```

use std::future::{Future, IntoFuture};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use base64::Engine;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::element_actions::{
    InputFilesItems, Perform, PointerAction, PointerOptions, Task, TaskOutput, WaitAfter,
    WaitForSelectorState, element_state, is_visible_internal, retry_with_progress_if_not_connected,
    wait_for_function_on_selector, wait_for_selector,
};
use crate::error::{Error, Result};
use crate::execution_context::Evaluated;
use crate::frame::Frame;
use crate::frame_selectors::{
    CallArg, SelectorTarget, describe_selector, handle_of, query, query_all,
    query_array_in_main_world,
};
use crate::input::{KeyboardModifier, MouseButton};
use crate::js::JsValue;
use crate::js::serde_impl::from_js_value;
use crate::js_handle::{ElementHandle, JSHandle, serialize_arg};
use crate::page::Page;
use crate::progress::{DEFAULT_TIMEOUT, Progress};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

// ----------------------------------------------------------------------
// Value types
// ----------------------------------------------------------------------

/// A point, in CSS pixels (`{ x, y }`).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Position {
    /// Horizontal coordinate.
    pub x: f64,
    /// Vertical coordinate.
    pub y: f64,
}

impl Position {
    /// A new point.
    pub fn new(x: f64, y: f64) -> Self {
        Position { x, y }
    }
}

impl From<(f64, f64)> for Position {
    fn from((x, y): (f64, f64)) -> Self {
        Position { x, y }
    }
}

impl From<(i32, i32)> for Position {
    fn from((x, y): (i32, i32)) -> Self {
        Position {
            x: x as f64,
            y: y as f64,
        }
    }
}

/// An element's bounding box relative to the main frame viewport.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct BoundingBox {
    /// The x coordinate of the top-left corner, in pixels.
    pub x: f64,
    /// The y coordinate of the top-left corner, in pixels.
    pub y: f64,
    /// The width, in pixels.
    pub width: f64,
    /// The height, in pixels.
    pub height: f64,
}

/// States accepted by `element_handle.wait_for_element_state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementState {
    /// Visible (non-empty box, no `visibility: hidden`).
    Visible,
    /// Not visible or not attached.
    Hidden,
    /// Visible and not animating.
    Stable,
    /// Not disabled.
    Enabled,
    /// Disabled.
    Disabled,
    /// Enabled and not read-only.
    Editable,
}

impl ElementState {
    /// The Playwright name of the state.
    pub fn as_str(&self) -> &'static str {
        match self {
            ElementState::Visible => "visible",
            ElementState::Hidden => "hidden",
            ElementState::Stable => "stable",
            ElementState::Enabled => "enabled",
            ElementState::Disabled => "disabled",
            ElementState::Editable => "editable",
        }
    }
}

impl std::str::FromStr for ElementState {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "visible" => ElementState::Visible,
            "hidden" => ElementState::Hidden,
            "stable" => ElementState::Stable,
            "enabled" => ElementState::Enabled,
            "disabled" => ElementState::Disabled,
            "editable" => ElementState::Editable,
            _ => {
                return Err(Error::invalid_argument(
                    "state: expected one of (visible|hidden|stable|enabled|disabled|editable)",
                ));
            }
        })
    }
}

/// An `<option>` to select, matched by value, label and/or index.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SelectOption {
    /// Matches by `option.value`.
    pub value: Option<String>,
    /// Matches by `option.label`.
    pub label: Option<String>,
    /// Matches by index.
    pub index: Option<usize>,
}

impl SelectOption {
    /// Match by value.
    pub fn value(value: impl Into<String>) -> Self {
        SelectOption {
            value: Some(value.into()),
            ..Default::default()
        }
    }

    /// Match by label.
    pub fn label(label: impl Into<String>) -> Self {
        SelectOption {
            label: Some(label.into()),
            ..Default::default()
        }
    }

    /// Match by index.
    pub fn index(index: usize) -> Self {
        SelectOption {
            index: Some(index),
            ..Default::default()
        }
    }
}

/// One value of `select_option`.
#[derive(Debug, Clone)]
pub enum SelectValue {
    /// Matches by value or label.
    Text(String),
    /// Matches by value, label and/or index.
    Option(SelectOption),
    /// An `<option>` element.
    Element(ElementHandle),
}

/// The values of `select_option` (a string, an option, an element or a
/// list of them; an empty list deselects everything).
#[derive(Debug, Clone, Default)]
pub struct SelectValues(pub Vec<SelectValue>);

impl SelectValues {
    /// No value: deselect everything (`selectOption(null)`).
    pub fn none() -> Self {
        SelectValues(Vec::new())
    }
}

impl From<&str> for SelectValues {
    fn from(v: &str) -> Self {
        SelectValues(vec![SelectValue::Text(v.to_string())])
    }
}
impl From<String> for SelectValues {
    fn from(v: String) -> Self {
        SelectValues(vec![SelectValue::Text(v)])
    }
}
impl From<&String> for SelectValues {
    fn from(v: &String) -> Self {
        SelectValues(vec![SelectValue::Text(v.clone())])
    }
}
impl From<SelectOption> for SelectValues {
    fn from(v: SelectOption) -> Self {
        SelectValues(vec![SelectValue::Option(v)])
    }
}
impl From<ElementHandle> for SelectValues {
    fn from(v: ElementHandle) -> Self {
        SelectValues(vec![SelectValue::Element(v)])
    }
}
impl From<&ElementHandle> for SelectValues {
    fn from(v: &ElementHandle) -> Self {
        SelectValues(vec![SelectValue::Element(v.clone())])
    }
}
impl From<Vec<&str>> for SelectValues {
    fn from(v: Vec<&str>) -> Self {
        SelectValues(v.into_iter().map(|s| SelectValue::Text(s.into())).collect())
    }
}
impl<const N: usize> From<[&str; N]> for SelectValues {
    fn from(v: [&str; N]) -> Self {
        SelectValues(v.into_iter().map(|s| SelectValue::Text(s.into())).collect())
    }
}
impl From<&[&str]> for SelectValues {
    fn from(v: &[&str]) -> Self {
        SelectValues(v.iter().map(|s| SelectValue::Text((*s).into())).collect())
    }
}
impl From<Vec<String>> for SelectValues {
    fn from(v: Vec<String>) -> Self {
        SelectValues(v.into_iter().map(SelectValue::Text).collect())
    }
}
impl From<Vec<SelectOption>> for SelectValues {
    fn from(v: Vec<SelectOption>) -> Self {
        SelectValues(v.into_iter().map(SelectValue::Option).collect())
    }
}
impl From<Vec<ElementHandle>> for SelectValues {
    fn from(v: Vec<ElementHandle>) -> Self {
        SelectValues(v.into_iter().map(SelectValue::Element).collect())
    }
}
impl From<Vec<SelectValue>> for SelectValues {
    fn from(v: Vec<SelectValue>) -> Self {
        SelectValues(v)
    }
}
impl From<Option<&str>> for SelectValues {
    fn from(v: Option<&str>) -> Self {
        match v {
            Some(v) => v.into(),
            None => SelectValues::none(),
        }
    }
}

pub use crate::fetch::FilePayload;

/// Files for `set_input_files`: paths (files or a single directory) or
/// in-memory payloads. An empty list clears the input.
#[derive(Debug, Clone)]
pub enum InputFiles {
    /// Local paths.
    Paths(Vec<PathBuf>),
    /// In-memory files.
    Payloads(Vec<FilePayload>),
}

impl InputFiles {
    /// No files: clears the selection.
    pub fn none() -> Self {
        InputFiles::Payloads(Vec::new())
    }
}

impl From<&str> for InputFiles {
    fn from(v: &str) -> Self {
        InputFiles::Paths(vec![PathBuf::from(v)])
    }
}
impl From<String> for InputFiles {
    fn from(v: String) -> Self {
        InputFiles::Paths(vec![PathBuf::from(v)])
    }
}
impl From<&Path> for InputFiles {
    fn from(v: &Path) -> Self {
        InputFiles::Paths(vec![v.to_path_buf()])
    }
}
impl From<PathBuf> for InputFiles {
    fn from(v: PathBuf) -> Self {
        InputFiles::Paths(vec![v])
    }
}
impl From<&PathBuf> for InputFiles {
    fn from(v: &PathBuf) -> Self {
        InputFiles::Paths(vec![v.clone()])
    }
}
impl From<Vec<PathBuf>> for InputFiles {
    fn from(v: Vec<PathBuf>) -> Self {
        InputFiles::Paths(v)
    }
}
impl From<Vec<&str>> for InputFiles {
    fn from(v: Vec<&str>) -> Self {
        if v.is_empty() {
            return InputFiles::none();
        }
        InputFiles::Paths(v.into_iter().map(PathBuf::from).collect())
    }
}
impl<const N: usize> From<[&str; N]> for InputFiles {
    fn from(v: [&str; N]) -> Self {
        Vec::from(v).into()
    }
}
impl From<FilePayload> for InputFiles {
    fn from(v: FilePayload) -> Self {
        InputFiles::Payloads(vec![v])
    }
}
impl From<Vec<FilePayload>> for InputFiles {
    fn from(v: Vec<FilePayload>) -> Self {
        InputFiles::Payloads(v)
    }
}

/// A MIME type for a file name (subset of the `mime` package used by Playwright).
pub(crate) fn mime_type_for(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "txt" | "text" | "log" => "text/plain",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "csv" => "text/csv",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "wasm" => "application/wasm",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/vnd.microsoft.icon",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        _ => "application/octet-stream",
    }
}

/// `convertInputFiles` + `prepareFilesForUpload` (browser on this machine).
pub(crate) fn prepare_input_files(files: &InputFiles) -> Result<InputFilesItems> {
    match files {
        InputFiles::Payloads(payloads) => {
            let total: usize = payloads.iter().map(|p| p.buffer.len()).sum();
            if total >= 50 * 1024 * 1024 {
                return Err(Error::invalid_argument(
                    "Cannot set buffer larger than 50Mb, please write it to a file and pass its path instead.",
                ));
            }
            Ok(InputFilesItems {
                payloads: Some(
                    payloads
                        .iter()
                        .map(|p| {
                            json!({
                                "name": p.name,
                                "mimeType": p.mime_type.clone().filter(|m| !m.is_empty()).unwrap_or_else(|| mime_type_for(&p.name).to_string()),
                                "buffer": base64::engine::general_purpose::STANDARD.encode(&p.buffer),
                            })
                        })
                        .collect(),
                ),
                ..Default::default()
            })
        }
        InputFiles::Paths(paths) => {
            let mut local_paths: Vec<String> = Vec::new();
            let mut local_directory: Option<String> = None;
            for path in paths {
                let metadata = std::fs::metadata(path).map_err(|_| {
                    Error::invalid_argument(format!(
                        "ENOENT: no such file or directory, stat '{}'",
                        path.display()
                    ))
                })?;
                let absolute = std::path::absolute(path)
                    .unwrap_or_else(|_| path.clone())
                    .to_string_lossy()
                    .to_string();
                if metadata.is_dir() {
                    if local_directory.is_some() {
                        return Err(Error::invalid_argument(
                            "Multiple directories are not supported",
                        ));
                    }
                    local_directory = Some(absolute);
                } else {
                    local_paths.push(absolute);
                }
            }
            if !local_paths.is_empty() && local_directory.is_some() {
                return Err(Error::invalid_argument(
                    "File paths must be all files or a single directory",
                ));
            }
            if local_directory.is_some() {
                return Ok(InputFilesItems {
                    local_directory,
                    ..Default::default()
                });
            }
            if local_paths.is_empty() {
                return Ok(InputFilesItems {
                    payloads: Some(Vec::new()),
                    ..Default::default()
                });
            }
            Ok(InputFilesItems {
                local_paths: Some(local_paths),
                ..Default::default()
            })
        }
    }
}

/// `convertSelectOptionValues`: split into elements and option objects.
fn convert_select_values(values: &SelectValues) -> (Vec<ElementHandle>, Vec<Value>) {
    let mut elements = Vec::new();
    let mut options = Vec::new();
    for value in &values.0 {
        match value {
            SelectValue::Element(e) => elements.push(e.clone()),
            SelectValue::Text(s) => options.push(json!({ "valueOrLabel": s })),
            SelectValue::Option(o) => {
                let mut obj = serde_json::Map::new();
                if let Some(v) = &o.value {
                    obj.insert("value".into(), json!(v));
                }
                if let Some(l) = &o.label {
                    obj.insert("label".into(), json!(l));
                }
                if let Some(i) = o.index {
                    obj.insert("index".into(), json!(i));
                }
                options.push(Value::Object(obj));
            }
        }
    }
    (elements, options)
}

// ----------------------------------------------------------------------
// The generic action builder
// ----------------------------------------------------------------------

/// What an action applies to.
#[derive(Clone)]
pub(crate) enum Target {
    /// An element handle (ElementHandle API).
    Handle(ElementHandle),
    /// A selector in a frame (Page/Frame/Locator API).
    Selector {
        frame: Frame,
        selector: String,
        strict: bool,
    },
    /// A locator that could not be built (reported when awaited).
    Invalid(String),
}

impl Target {
    pub(crate) fn selector(frame: &Frame, selector: impl Into<String>, strict: bool) -> Target {
        Target::Selector {
            frame: frame.clone(),
            selector: selector.into(),
            strict,
        }
    }

    fn timeout(&self, explicit: Option<u64>) -> u64 {
        let page = match self {
            Target::Handle(h) => h.page_or_closed().ok(),
            Target::Selector { frame, .. } => frame.page_inner(),
            Target::Invalid(_) => None,
        };
        match page {
            Some(page) => page.timeouts.timeout(explicit),
            None => explicit.unwrap_or(DEFAULT_TIMEOUT),
        }
    }

    /// The browser of the target's page, for `slow_mo`.
    fn browser(&self) -> Option<std::sync::Arc<crate::browser::BrowserInner>> {
        let page = match self {
            Target::Handle(h) => h.page_or_closed().ok(),
            Target::Selector { frame, .. } => frame.page_inner(),
            Target::Invalid(_) => None,
        };
        page.and_then(|p| p.browser.upgrade())
    }
}

/// Options of every action (only the relevant ones are settable).
#[derive(Default)]
pub(crate) struct Opts {
    pub(crate) button: Option<MouseButton>,
    pub(crate) click_count: Option<u32>,
    pub(crate) delay: Option<f64>,
    pub(crate) force: bool,
    pub(crate) modifiers: Option<Vec<KeyboardModifier>>,
    pub(crate) no_wait_after: bool,
    pub(crate) position: Option<Position>,
    pub(crate) source_position: Option<Position>,
    pub(crate) target_position: Option<Position>,
    pub(crate) timeout: Option<u64>,
    pub(crate) trial: bool,
    pub(crate) strict: Option<bool>,
    pub(crate) steps: Option<u32>,
    pub(crate) scroll_none: bool,
    pub(crate) state: Option<WaitForSelectorState>,
    pub(crate) depth: Option<u32>,
    pub(crate) mode: Option<String>,
    pub(crate) boxes: Option<bool>,
    pub(crate) isolated: Option<bool>,
}

/// A serialized evaluation argument and the handles it references.
pub(crate) type SerializedArg = Option<(Value, Vec<JSHandle>)>;

/// Positional arguments of actions.
#[derive(Default)]
pub(crate) struct Args {
    pub(crate) text: String,
    pub(crate) flag: bool,
    pub(crate) values: SelectValues,
    pub(crate) files: Option<InputFiles>,
    pub(crate) arg: Option<Result<SerializedArg>>,
    pub(crate) other: Option<String>,
}

/// A pending action or query; configure it with its option methods and
/// `.await` it. See the [module documentation](self).
#[must_use = "actions do nothing unless awaited"]
pub struct Action<K> {
    pub(crate) target: Target,
    pub(crate) api: &'static str,
    pub(crate) opts: Opts,
    pub(crate) args: Args,
    pub(crate) _kind: PhantomData<fn() -> K>,
}

impl<K> Action<K> {
    pub(crate) fn new(target: Target, api: &'static str) -> Self {
        Action {
            target,
            api,
            opts: Opts::default(),
            args: Args::default(),
            _kind: PhantomData,
        }
    }

    pub(crate) fn with_text(mut self, text: impl Into<String>) -> Self {
        self.args.text = text.into();
        self
    }

    pub(crate) fn with_flag(mut self, flag: bool) -> Self {
        self.args.flag = flag;
        self
    }

    /// Maximum time in milliseconds (default: the page's default timeout,
    /// 30 s); `0` disables the timeout.
    pub fn timeout(mut self, timeout_ms: u64) -> Self {
        self.opts.timeout = Some(timeout_ms);
        self
    }

    fn progress(&self) -> Progress {
        Progress::new(self.api, self.target.timeout(self.opts.timeout))
    }

    fn strict_target(&self) -> Result<(Frame, SelectorTarget)> {
        match &self.target {
            Target::Selector {
                frame,
                selector,
                strict,
            } => Ok((
                frame.clone(),
                SelectorTarget::new(selector.clone(), self.opts.strict.unwrap_or(*strict)),
            )),
            Target::Handle(h) => Ok((
                h.owner_frame_sync()?,
                SelectorTarget::scoped(":scope", h.clone()),
            )),
            Target::Invalid(message) => Err(Error::invalid_argument(message.clone())),
        }
    }
}

/// Marker traits enabling option methods on [`Action`]s.
#[doc(hidden)]
pub mod opt {
    /// `force`
    pub trait Force {}
    /// `trial`
    pub trait Trial {}
    /// `position`, `modifiers`, `scroll`
    pub trait Pointer {}
    /// `no_wait_after`
    pub trait NoWaitAfter {}
    /// `strict`
    pub trait Strict {}
    /// `delay`
    pub trait Delay {}
    /// `button`
    pub trait Button {}
    /// `click_count`
    pub trait ClickCount {}
    /// `steps`
    pub trait Steps {}
    /// `state`
    pub trait State {}
    /// `arg`
    pub trait Arg {}
}

impl<K: opt::Force> Action<K> {
    /// Bypass the actionability checks.
    pub fn force(mut self, force: bool) -> Self {
        self.opts.force = force;
        self
    }
}

impl<K: opt::Trial> Action<K> {
    /// Only perform the actionability checks, skip the action.
    pub fn trial(mut self, trial: bool) -> Self {
        self.opts.trial = trial;
        self
    }
}

impl<K: opt::Pointer> Action<K> {
    /// The point to use, relative to the top-left corner of the padding box.
    pub fn position(mut self, position: impl Into<Position>) -> Self {
        self.opts.position = Some(position.into());
        self
    }

    /// Modifier keys to hold during the action.
    pub fn modifiers(mut self, modifiers: impl IntoIterator<Item = KeyboardModifier>) -> Self {
        self.opts.modifiers = Some(modifiers.into_iter().collect());
        self
    }

    /// `scroll: "none"` (`false`) skips scrolling the element into view.
    pub fn scroll(mut self, scroll: bool) -> Self {
        self.opts.scroll_none = !scroll;
        self
    }
}

impl<K: opt::NoWaitAfter> Action<K> {
    /// Do not wait for navigations initiated by the action.
    pub fn no_wait_after(mut self, no_wait_after: bool) -> Self {
        self.opts.no_wait_after = no_wait_after;
        self
    }
}

impl<K: opt::Strict> Action<K> {
    /// Fail when the selector matches more than one element.
    pub fn strict(mut self, strict: bool) -> Self {
        self.opts.strict = Some(strict);
        self
    }
}

impl<K: opt::Delay> Action<K> {
    /// Time to wait between key presses / mouse down and up, in milliseconds.
    pub fn delay(mut self, delay_ms: u64) -> Self {
        self.opts.delay = Some(delay_ms as f64);
        self
    }
}

impl<K: opt::Button> Action<K> {
    /// Mouse button (default left).
    pub fn button(mut self, button: MouseButton) -> Self {
        self.opts.button = Some(button);
        self
    }
}

impl<K: opt::ClickCount> Action<K> {
    /// Number of clicks (default 1).
    pub fn click_count(mut self, click_count: u32) -> Self {
        self.opts.click_count = Some(click_count);
        self
    }
}

impl<K: opt::Steps> Action<K> {
    /// Number of intermediate mouse moves (default 1).
    pub fn steps(mut self, steps: u32) -> Self {
        self.opts.steps = Some(steps);
        self
    }
}

impl<K: opt::State> Action<K> {
    /// The state to wait for (default visible).
    pub fn state(mut self, state: WaitForSelectorState) -> Self {
        self.opts.state = Some(state);
        self
    }
}

impl<K: opt::Arg> Action<K> {
    /// The argument passed to the function (any `Serialize` value, which may
    /// contain [`JSHandle`]s).
    pub fn arg<A: Serialize>(mut self, arg: A) -> Self {
        self.args.arg = Some(serialize_arg(&arg));
        self
    }
}

/// Kinds of [`Action`]s (type-level tags).
pub mod kinds {
    use std::marker::PhantomData;

    macro_rules! kinds {
        ($($(#[$m:meta])* $name:ident),* $(,)?) => {
            $( $(#[$m])* #[derive(Debug)] pub struct $name; )*
        };
    }
    kinds!(
        /// `click`
        Click,
        /// `dblclick`
        Dblclick,
        /// `tap`
        Tap,
        /// `hover`
        Hover,
        /// `fill` / `clear`
        Fill,
        /// `type` / `press_sequentially`
        Type,
        /// `press`
        Press,
        /// `check` / `uncheck` / `set_checked`
        SetChecked,
        /// `select_option`
        SelectOption,
        /// `select_text`
        SelectText,
        /// `set_input_files`
        SetInputFiles,
        /// `focus`
        Focus,
        /// `blur`
        Blur,
        /// `dispatch_event`
        DispatchEvent,
        /// `scroll_into_view_if_needed`
        ScrollIntoView,
        /// `text_content`
        TextContent,
        /// `inner_text`
        InnerText,
        /// `inner_html`
        InnerHtml,
        /// `get_attribute`
        GetAttribute,
        /// `input_value`
        InputValue,
        /// `is_checked`, `is_enabled`, ...
        IsState,
        /// `is_visible` / `is_hidden`
        IsVisible,
        /// `wait_for_selector`
        WaitForSelector,
        /// `locator.wait_for`
        WaitFor,
        /// `wait_for_element_state`
        WaitForElementState,
        /// `bounding_box`
        BoundingBox,
        /// `drag_and_drop` / `drag_to`
        DragAndDrop,
        /// `locator.element_handle`
        ElementHandle,
        /// `evaluate_handle` on a locator
        EvaluateHandle,
        /// `aria_snapshot`
        AriaSnapshot,
        /// `aria_snapshot_json`
        AriaSnapshotJson,
        /// `locator.wait_for_function`
        WaitForFunction,
        /// `locator.drop`
        Drop,
    );

    /// `evaluate` on a locator / `eval_on_selector`.
    #[derive(Debug)]
    pub struct Evaluate<T>(PhantomData<fn() -> T>);
    /// `evaluate_all` / `eval_on_selector_all`.
    #[derive(Debug)]
    pub struct EvaluateAll<T>(PhantomData<fn() -> T>);
    /// `eval_on_selector`.
    #[derive(Debug)]
    pub struct EvalOnSelector<T>(PhantomData<fn() -> T>);
}

macro_rules! opts {
    ($kind:ty: $($trait:ident),*) => {
        $( impl opt::$trait for $kind {} )*
    };
}

opts!(kinds::Click: Force, Trial, Pointer, NoWaitAfter, Strict, Delay, Button, ClickCount, Steps);
opts!(kinds::Dblclick: Force, Trial, Pointer, NoWaitAfter, Strict, Delay, Button, Steps);
opts!(kinds::Tap: Force, Trial, Pointer, NoWaitAfter, Strict);
opts!(kinds::Hover: Force, Trial, Pointer, NoWaitAfter, Strict);
opts!(kinds::Fill: Force, NoWaitAfter, Strict);
opts!(kinds::Type: Delay, NoWaitAfter, Strict);
opts!(kinds::Press: Delay, NoWaitAfter, Strict);
opts!(kinds::SetChecked: Force, Trial, Pointer, NoWaitAfter, Strict);
opts!(kinds::SelectOption: Force, NoWaitAfter, Strict);
opts!(kinds::SelectText: Force);
opts!(kinds::SetInputFiles: NoWaitAfter, Strict);
opts!(kinds::Focus: Strict);
opts!(kinds::Blur: Strict);
opts!(kinds::DispatchEvent: Strict);
opts!(kinds::TextContent: Strict);
opts!(kinds::InnerText: Strict);
opts!(kinds::InnerHtml: Strict);
opts!(kinds::GetAttribute: Strict);
opts!(kinds::InputValue: Strict);
opts!(kinds::IsState: Strict);
opts!(kinds::IsVisible: Strict);
opts!(kinds::WaitForSelector: Strict, State);
opts!(kinds::WaitFor: State);
opts!(kinds::DragAndDrop: Force, Trial, NoWaitAfter, Strict, Steps);
opts!(kinds::EvaluateHandle: Arg);
opts!(kinds::WaitForFunction: Arg);
impl<T> opt::Arg for kinds::Evaluate<T> {}
impl<T> opt::Arg for kinds::EvaluateAll<T> {}
impl<T> opt::Arg for kinds::EvalOnSelector<T> {}
impl<T> opt::Strict for kinds::EvalOnSelector<T> {}

impl Action<kinds::DispatchEvent> {
    /// The event init dictionary (`eventInit`), any `Serialize` value
    /// (may contain [`JSHandle`]s, e.g. a `DataTransfer`).
    pub fn event_init<A: Serialize>(mut self, event_init: A) -> Self {
        self.args.arg = Some(serialize_arg(&event_init));
        self
    }
}

impl Action<kinds::DragAndDrop> {
    /// Point on the source element, relative to its padding box.
    pub fn source_position(mut self, position: impl Into<Position>) -> Self {
        self.opts.source_position = Some(position.into());
        self
    }

    /// Point on the target element, relative to its padding box.
    pub fn target_position(mut self, position: impl Into<Position>) -> Self {
        self.opts.target_position = Some(position.into());
        self
    }

    /// `scroll: "none"` (`false`) skips scrolling the elements into view.
    pub fn scroll(mut self, scroll: bool) -> Self {
        self.opts.scroll_none = !scroll;
        self
    }
}

/// Builders returned by the action methods.
pub type Click = Action<kinds::Click>;
/// Builder of `dblclick`.
pub type Dblclick = Action<kinds::Dblclick>;
/// Builder of `tap`.
pub type Tap = Action<kinds::Tap>;
/// Builder of `hover`.
pub type Hover = Action<kinds::Hover>;
/// Builder of `fill` / `clear`.
pub type Fill = Action<kinds::Fill>;
/// Builder of `type` / `press_sequentially`.
pub type Type = Action<kinds::Type>;
/// Builder of `press`.
pub type Press = Action<kinds::Press>;
/// Builder of `check` / `uncheck` / `set_checked`.
pub type SetChecked = Action<kinds::SetChecked>;
/// Builder of `select_option`.
pub type SelectOptionAction = Action<kinds::SelectOption>;
/// Builder of `set_input_files`.
pub type SetInputFiles = Action<kinds::SetInputFiles>;
/// Builder of `wait_for_selector`.
pub type WaitForSelector = Action<kinds::WaitForSelector>;

// ----------------------------------------------------------------------
// Running actions
// ----------------------------------------------------------------------

macro_rules! into_future {
    // Actions Playwright slows down with `slowMo`.
    (slow $kind:ty => $out:ty, |$a:ident| $body:expr) => {
        impl IntoFuture for Action<$kind> {
            type Output = Result<$out>;
            type IntoFuture = BoxFuture<Result<$out>>;
            fn into_future(self) -> Self::IntoFuture {
                let $a = self;
                let browser = $a.target.browser();
                Box::pin(async move {
                    let result = $body.await;
                    if let Some(browser) = browser {
                        browser.slow_mo().await;
                    }
                    result
                })
            }
        }
    };
    ($kind:ty => $out:ty, |$a:ident| $body:expr) => {
        impl IntoFuture for Action<$kind> {
            type Output = Result<$out>;
            type IntoFuture = BoxFuture<Result<$out>>;
            fn into_future(self) -> Self::IntoFuture {
                let $a = self;
                Box::pin(async move { $body.await })
            }
        }
    };
}

/// Run a task on the target with Playwright's progress semantics.
pub(crate) async fn run_task<K>(
    action: Action<K>,
    task: Task,
    force: bool,
    wait_for_frame_visible: bool,
) -> Result<TaskOutput> {
    let progress = action.progress();
    let p = progress.clone();
    match action.target.clone() {
        Target::Handle(handle) => {
            progress
                .run(async move { handle.run_handle_task(&p, &task).await })
                .await
        }
        Target::Selector {
            frame,
            selector,
            strict,
        } => {
            let target = SelectorTarget::new(selector, action.opts.strict.unwrap_or(strict));
            progress
                .run(async move {
                    retry_with_progress_if_not_connected(
                        &frame,
                        &p,
                        &target,
                        force,
                        wait_for_frame_visible,
                        &task,
                    )
                    .await
                })
                .await
        }
        Target::Invalid(message) => Err(Error::invalid_argument(message).with_api(action.api)),
    }
}

fn pointer_options(opts: &Opts) -> PointerOptions {
    PointerOptions {
        force: opts.force,
        position: opts.position,
        trial: opts.trial,
        modifiers: opts.modifiers.clone(),
        scroll_none: opts.scroll_none,
    }
}

fn check_tap_target(action: &Action<kinds::Tap>) -> Result<()> {
    let _ = action;
    Ok(())
}

into_future!(slow kinds::Click => (), |a| async move {
    let options = pointer_options(&a.opts);
    let pointer = PointerAction {
        name: "click",
        wait_for_enabled: true,
        wait_after: if a.opts.no_wait_after { WaitAfter::No } else { WaitAfter::Yes },
        skip_action_pre_checks: false,
        perform: Perform::Click {
            button: a.opts.button.unwrap_or_default(),
            click_count: a.opts.click_count.unwrap_or(1),
            delay: a.opts.delay,
            steps: a.opts.steps,
        },
    };
    let force = a.opts.force;
    run_task(a, Task::Pointer(pointer, options), force, true).await.map(|_| ())
});

into_future!(slow kinds::Dblclick => (), |a| async move {
    let options = pointer_options(&a.opts);
    let pointer = PointerAction {
        name: "dblclick",
        wait_for_enabled: true,
        wait_after: WaitAfter::Disabled,
        skip_action_pre_checks: false,
        perform: Perform::Click {
            button: a.opts.button.unwrap_or_default(),
            click_count: 2,
            delay: a.opts.delay,
            steps: a.opts.steps,
        },
    };
    let force = a.opts.force;
    run_task(a, Task::Pointer(pointer, options), force, true).await.map(|_| ())
});

into_future!(slow kinds::Tap => (), |a| async move {
    check_tap_target(&a)?;
    let options = pointer_options(&a.opts);
    let pointer = PointerAction {
        name: "tap",
        wait_for_enabled: true,
        wait_after: WaitAfter::Disabled,
        skip_action_pre_checks: false,
        perform: Perform::Tap,
    };
    let force = a.opts.force;
    run_task(a, Task::Pointer(pointer, options), force, true).await.map(|_| ())
});

into_future!(slow kinds::Hover => (), |a| async move {
    let options = pointer_options(&a.opts);
    let pointer = PointerAction {
        name: "hover",
        wait_for_enabled: false,
        wait_after: WaitAfter::Disabled,
        skip_action_pre_checks: false,
        perform: Perform::Hover,
    };
    let force = a.opts.force;
    run_task(a, Task::Pointer(pointer, options), force, true).await.map(|_| ())
});

into_future!(slow kinds::Fill => (), |a| async move {
    let task = Task::Fill { value: a.args.text.clone(), force: a.opts.force };
    let force = a.opts.force;
    run_task(a, task, force, true).await.map(|_| ())
});

into_future!(slow kinds::Type => (), |a| async move {
    let task = Task::Type { text: a.args.text.clone(), delay: a.opts.delay };
    run_task(a, task, false, false).await.map(|_| ())
});

into_future!(slow kinds::Press => (), |a| async move {
    let task = Task::Press {
        key: a.args.text.clone(),
        delay: a.opts.delay,
        no_wait_after: a.opts.no_wait_after,
    };
    run_task(a, task, false, false).await.map(|_| ())
});

into_future!(slow kinds::SetChecked => (), |a| async move {
    let task = Task::SetChecked { state: a.args.flag, options: pointer_options(&a.opts) };
    let force = a.opts.force;
    run_task(a, task, force, true).await.map(|_| ())
});

into_future!(slow kinds::SelectOption => Vec<String>, |a| async move {
    let (elements, values) = convert_select_values(&a.args.values);
    let task = Task::SelectOption { elements, values, force: a.opts.force };
    let force = a.opts.force;
    match run_task(a, task, force, true).await? {
        TaskOutput::Values(v) => Ok(v),
        _ => Ok(Vec::new()),
    }
});

into_future!(slow kinds::SelectText => (), |a| async move {
    let task = Task::SelectText { force: a.opts.force };
    let force = a.opts.force;
    with_element(a, move |handle, progress| async move {
        handle.run_handle_task(&progress, &task).await.map(|_| ())
    }, force)
    .await
});

into_future!(slow kinds::SetInputFiles => (), |a| async move {
    let files = a.args.files.clone().unwrap_or_else(InputFiles::none);
    let items = prepare_input_files(&files).map_err(|e| e.with_api(a.api))?;
    run_task(a, Task::SetInputFiles(items), false, false).await.map(|_| ())
});

into_future!(slow kinds::Focus => (), |a| async move {
    run_task(a, Task::Focus, false, false).await.map(|_| ())
});

into_future!(slow kinds::Blur => (), |a| async move {
    run_task(a, Task::Blur, false, false).await.map(|_| ())
});

into_future!(slow kinds::ScrollIntoView => (), |a| async move {
    with_element(a, |handle, progress| async move {
        handle.run_handle_task(&progress, &Task::ScrollIntoView).await.map(|_| ())
    }, false)
    .await
});

/// `Locator._withElement`: resolve the element (strict, attached), run
/// `task` with the remaining time, dispose the handle.
async fn with_element<K, F, Fut, T>(action: Action<K>, task: F, _force: bool) -> Result<T>
where
    F: FnOnce(ElementHandle, Progress) -> Fut + Send,
    Fut: Future<Output = Result<T>> + Send,
{
    let progress = action.progress();
    match action.target.clone() {
        Target::Handle(handle) => {
            let p = progress.clone();
            progress.run(async move { task(handle, p).await }).await
        }
        Target::Selector {
            frame,
            selector,
            strict,
        } => {
            let p = progress.clone();
            let target = SelectorTarget::new(selector.clone(), strict);
            progress
                .run(async move {
                    let handle = wait_for_selector(
                        &frame,
                        &p,
                        &target,
                        true,
                        WaitForSelectorState::Attached,
                        false,
                    )
                    .await?
                    .ok_or_else(|| {
                        Error::evaluation(format!("Could not resolve {selector} to DOM Element"))
                    })?;
                    let result = task(handle.clone(), p).await;
                    handle.dispose().await;
                    result
                })
                .await
        }
        Target::Invalid(message) => Err(Error::invalid_argument(message).with_api(action.api)),
    }
}

async fn wait_for_function<K>(
    action: Action<K>,
    callback: &'static str,
    data: CallArg,
    main_world: bool,
) -> Result<JsValue> {
    let progress = action.progress();
    let (frame, target) = match action.strict_target() {
        Ok(v) => v,
        Err(e) => return Err(e.with_api(action.api)),
    };
    let p = progress.clone();
    progress
        .run(async move {
            wait_for_function_on_selector(&frame, &p, &target, callback, data, main_world).await
        })
        .await
}

fn result_field(value: JsValue) -> JsValue {
    value.get("result").cloned().unwrap_or(JsValue::Undefined)
}

fn to_opt_string(value: JsValue) -> Option<String> {
    match value {
        JsValue::String(s) => Some(s),
        _ => None,
    }
}

into_future!(slow kinds::DispatchEvent => (), |a| async move {
    let mut a = a;
    let api = a.api;
    let init = a.args.arg.take_arg().map_err(|e| e.with_api(api))?;
    let (init_wire, init_handles) =
        init.unwrap_or_else(|| (json!({ "v": "undefined" }), Vec::new()));
    // `eventInit` travels as the task data so that it may hold handles.
    let callback = format!(
        "(injectedScript, element, eventInit) => {{ injectedScript.dispatchEvent(element, {}, eventInit === undefined ? {{}} : eventInit); return {{ result: undefined }}; }}",
        serde_json::to_string(&a.args.text).unwrap_or_default()
    );
    let progress = a.progress();
    let (frame, target) = a.strict_target().map_err(|e| e.with_api(api))?;
    let p = progress.clone();
    progress
        .run(async move {
            wait_for_function_on_selector(
                &frame,
                &p,
                &target,
                &callback,
                CallArg::Wire(init_wire, init_handles),
                true,
            )
            .await
            .map(|_| ())
        })
        .await
});

/// Helper to take the serialized `arg` of an action.
trait TakeArg {
    fn take_arg(&mut self) -> Result<Option<(Value, Vec<JSHandle>)>>;
}

impl TakeArg for Option<Result<Option<(Value, Vec<JSHandle>)>>> {
    fn take_arg(&mut self) -> Result<Option<(Value, Vec<JSHandle>)>> {
        match self.take() {
            None => Ok(None),
            Some(r) => r,
        }
    }
}

into_future!(kinds::TextContent => Option<String>, |a| async move {
    let v = wait_for_function(a, "(injected, element) => ({ result: element.textContent })", CallArg::OptHandle(None), false).await?;
    Ok(to_opt_string(result_field(v)))
});

into_future!(kinds::InnerText => String, |a| async move {
    let v = wait_for_function(a, "(injectedScript, element) => { if (element.namespaceURI !== 'http://www.w3.org/1999/xhtml') throw injectedScript.createStacklessError('Node is not an HTMLElement'); return { result: element.innerText }; }", CallArg::OptHandle(None), false).await?;
    Ok(to_opt_string(result_field(v)).unwrap_or_default())
});

into_future!(kinds::InnerHtml => String, |a| async move {
    let v = wait_for_function(a, "(injected, element) => ({ result: element.innerHTML })", CallArg::OptHandle(None), false).await?;
    Ok(to_opt_string(result_field(v)).unwrap_or_default())
});

into_future!(kinds::GetAttribute => Option<String>, |a| async move {
    let name = a.args.text.clone();
    let v = wait_for_function(a, "(injected, element, data) => ({ result: element.getAttribute(data.name) })", CallArg::Json(json!({ "name": name })), false).await?;
    Ok(to_opt_string(result_field(v)))
});

into_future!(kinds::InputValue => String, |a| async move {
    let v = wait_for_function(a, "(injectedScript, node) => { const element = injectedScript.retarget(node, 'follow-label'); if (!element || (element.nodeName !== 'INPUT' && element.nodeName !== 'TEXTAREA' && element.nodeName !== 'SELECT')) throw injectedScript.createStacklessError('Node is not an <input>, <textarea> or <select> element'); return { result: element.value }; }", CallArg::OptHandle(None), false).await?;
    Ok(to_opt_string(result_field(v)).unwrap_or_default())
});

into_future!(kinds::IsState => bool, |a| async move {
    let state = a.args.text.clone();
    let progress = a.progress();
    let (frame, target) = a.strict_target().map_err(|e| e.with_api(a.api))?;
    let p = progress.clone();
    progress.run(async move { element_state(&frame, &p, &target, &state).await }).await
});

into_future!(kinds::IsVisible => bool, |a| async move {
    let hidden = a.args.flag;
    let progress = a.progress();
    let (frame, target) = a.strict_target().map_err(|e| e.with_api(a.api))?;
    let p = progress.clone();
    progress.run(async move {
        p.log(format!("  checking visibility of {}", describe_selector(&target.selector)));
        let visible = is_visible_internal(&frame, &target).await?;
        Ok(if hidden { !visible } else { visible })
    }).await
});

into_future!(kinds::WaitForSelector => Option<ElementHandle>, |a| async move {
    let state = a.opts.state.unwrap_or_default();
    let progress = a.progress();
    let (frame, mut target) = a.strict_target().map_err(|e| e.with_api(a.api))?;
    if let Target::Handle(h) = &a.target {
        target = SelectorTarget::scoped(a.args.text.clone(), h.clone());
    }
    let p = progress.clone();
    progress.run(async move { wait_for_selector(&frame, &p, &target, true, state, false).await }).await
});

into_future!(kinds::WaitFor => (), |a| async move {
    let state = a.opts.state.unwrap_or_default();
    let progress = a.progress();
    let (frame, target) = a.strict_target().map_err(|e| e.with_api(a.api))?;
    let p = progress.clone();
    progress.run(async move { wait_for_selector(&frame, &p, &target, true, state, true).await.map(|_| ()) }).await
});

into_future!(kinds::WaitForElementState => (), |a| async move {
    let state = a.args.text.clone();
    let Target::Handle(handle) = a.target.clone() else {
        return Err(Error::invalid_argument("wait_for_element_state requires an element handle"));
    };
    let progress = a.progress();
    let p = progress.clone();
    progress.run(async move {
        handle.run_handle_task(&p, &Task::WaitForState(state)).await.map(|_| ())
    }).await
});

into_future!(kinds::BoundingBox => Option<BoundingBox>, |a| async move {
    with_element(a, |handle, _| async move { handle.bounding_box_impl().await }, false).await
});

into_future!(kinds::ElementHandle => ElementHandle, |a| async move {
    let progress = a.progress();
    let (frame, target) = a.strict_target().map_err(|e| e.with_api(a.api))?;
    let p = progress.clone();
    progress.run(async move {
        let selector = target.selector.clone();
        wait_for_selector(&frame, &p, &target, true, WaitForSelectorState::Attached, false)
            .await?
            .ok_or_else(|| Error::evaluation(format!("Could not resolve {selector} to DOM Element")))
    }).await
});

into_future!(slow kinds::DragAndDrop => (), |a| async move {
    let Target::Selector { frame, selector, strict } = a.target.clone() else {
        return Err(Error::invalid_argument("drag_and_drop requires selectors").with_api(a.api));
    };
    let strict = a.opts.strict.unwrap_or(strict);
    let target_selector = a.args.other.clone().unwrap_or_default();
    let progress = a.progress();
    let p = progress.clone();
    let force = a.opts.force;
    let trial = a.opts.trial;
    let steps = a.opts.steps.unwrap_or(1);
    let source_options = PointerOptions {
        force,
        position: a.opts.source_position,
        trial,
        modifiers: None,
        scroll_none: a.opts.scroll_none,
    };
    let target_options = PointerOptions {
        position: a.opts.target_position,
        ..source_options.clone()
    };
    progress.run(async move {
        let down = Task::Pointer(
            PointerAction {
                name: "move and down",
                wait_for_enabled: false,
                wait_after: WaitAfter::Disabled,
                skip_action_pre_checks: false,
                perform: Perform::MoveAndDown,
            },
            source_options,
        );
        retry_with_progress_if_not_connected(&frame, &p, &SelectorTarget::new(selector, strict), force, true, &down).await?;
        let up = Task::Pointer(
            PointerAction {
                name: "move and up",
                wait_for_enabled: false,
                wait_after: WaitAfter::Disabled,
                skip_action_pre_checks: true,
                perform: Perform::MoveAndUp { steps },
            },
            target_options,
        );
        retry_with_progress_if_not_connected(&frame, &p, &SelectorTarget::new(target_selector, strict), force, true, &up).await?;
        Ok(())
    }).await
});

/// Evaluate on the element a locator resolves to (`_withElement`).
impl<T: DeserializeOwned + Send + 'static> IntoFuture for Action<kinds::Evaluate<T>> {
    type Output = Result<T>;
    type IntoFuture = BoxFuture<Result<T>>;
    fn into_future(mut self) -> Self::IntoFuture {
        Box::pin(async move {
            let arg = self.args.arg.take_arg().map_err(|e| e.with_api(self.api))?;
            let expression = self.args.text.clone();
            let api = self.api;
            with_element(
                self,
                move |handle, _| async move {
                    match handle.evaluate_inner(&expression, arg, true).await? {
                        Evaluated::Value(v) => from_js_value(v).map_err(Error::serialization),
                        Evaluated::Handle(_) => Err(Error::evaluation("unexpected handle result")),
                    }
                },
                false,
            )
            .await
            .map_err(|e| e.with_api(api))
        })
    }
}

impl IntoFuture for Action<kinds::EvaluateHandle> {
    type Output = Result<JSHandle>;
    type IntoFuture = BoxFuture<Result<JSHandle>>;
    fn into_future(mut self) -> Self::IntoFuture {
        Box::pin(async move {
            let arg = self.args.arg.take_arg().map_err(|e| e.with_api(self.api))?;
            let expression = self.args.text.clone();
            with_element(
                self,
                move |handle, _| async move {
                    handle_of(handle.evaluate_inner(&expression, arg, false).await?)
                },
                false,
            )
            .await
        })
    }
}

/// `$eval`: evaluate on the first element matching the selector.
impl<T: DeserializeOwned + Send + 'static> IntoFuture for Action<kinds::EvalOnSelector<T>> {
    type Output = Result<T>;
    type IntoFuture = BoxFuture<Result<T>>;
    fn into_future(mut self) -> Self::IntoFuture {
        Box::pin(async move {
            let api = self.api;
            let arg = self.args.arg.take_arg().map_err(|e| e.with_api(api))?;
            let expression = self.args.text.clone();
            let selector = self.args.other.clone().unwrap_or_default();
            let (frame, target) = match &self.target {
                Target::Handle(h) => (
                    h.owner_frame_sync().map_err(|e| e.with_api(api))?,
                    SelectorTarget {
                        selector: selector.clone(),
                        strict: false,
                        scope: Some(h.clone()),
                    },
                ),
                Target::Selector {
                    frame,
                    selector,
                    strict,
                } => (
                    frame.clone(),
                    SelectorTarget::new(selector.clone(), self.opts.strict.unwrap_or(*strict)),
                ),
                Target::Invalid(m) => return Err(Error::invalid_argument(m.clone()).with_api(api)),
            };
            async move {
                let handle = query(&frame, &target).await?.ok_or_else(|| {
                    Error::evaluation(format!(
                        "Failed to find element matching selector \"{}\"",
                        target.selector
                    ))
                })?;
                let result = handle.evaluate_inner(&expression, arg, true).await;
                handle.dispose().await;
                match result? {
                    Evaluated::Value(v) => from_js_value(v).map_err(Error::serialization),
                    Evaluated::Handle(_) => Err(Error::evaluation("unexpected handle result")),
                }
            }
            .await
            .map_err(|e| e.with_api(api))
        })
    }
}

/// `$$eval` / `locator.evaluate_all`.
impl<T: DeserializeOwned + Send + 'static> IntoFuture for Action<kinds::EvaluateAll<T>> {
    type Output = Result<T>;
    type IntoFuture = BoxFuture<Result<T>>;
    fn into_future(mut self) -> Self::IntoFuture {
        Box::pin(async move {
            let api = self.api;
            let arg = self.args.arg.take_arg().map_err(|e| e.with_api(api))?;
            let expression = self.args.text.clone();
            let (frame, selector, scope) = match &self.target {
                Target::Handle(h) => (
                    h.owner_frame_sync().map_err(|e| e.with_api(api))?,
                    self.args.other.clone().unwrap_or_default(),
                    Some(h.clone()),
                ),
                Target::Selector {
                    frame, selector, ..
                } => (frame.clone(), selector.clone(), None),
                Target::Invalid(m) => return Err(Error::invalid_argument(m.clone()).with_api(api)),
            };
            async move {
                let array = query_array_in_main_world(&frame, &selector, scope).await?;
                let result = array.evaluate_inner(&expression, arg, true).await;
                array.dispose().await;
                match result? {
                    Evaluated::Value(v) => from_js_value(v).map_err(Error::serialization),
                    Evaluated::Handle(_) => Err(Error::evaluation("unexpected handle result")),
                }
            }
            .await
            .map_err(|e| e.with_api(api))
        })
    }
}

// ----------------------------------------------------------------------
// ElementHandle API
// ----------------------------------------------------------------------

impl ElementHandle {
    fn action<K>(&self, api: &'static str) -> Action<K> {
        Action::new(Target::Handle(self.clone()), api)
    }

    /// Click the element (scrolls it into view, waits for actionability).
    pub fn click(&self) -> Click {
        self.action("elementHandle.click")
    }

    /// Double click the element.
    pub fn dblclick(&self) -> Dblclick {
        self.action("elementHandle.dblclick")
    }

    /// Tap the element (requires touch support).
    pub fn tap(&self) -> Tap {
        self.action("elementHandle.tap")
    }

    /// Hover over the element.
    pub fn hover(&self) -> Hover {
        self.action("elementHandle.hover")
    }

    /// Fill an `<input>`, `<textarea>` or `[contenteditable]` with `value`.
    pub fn fill(&self, value: impl Into<String>) -> Fill {
        self.action("elementHandle.fill").with_text(value)
    }

    /// Focus the element and type `text` key by key.
    pub fn r#type(&self, text: impl Into<String>) -> Type {
        self.action("elementHandle.type").with_text(text)
    }

    /// Alias of `ElementHandle::type` (`r#type`).
    pub fn type_text(&self, text: impl Into<String>) -> Type {
        self.r#type(text)
    }

    /// Focus the element and press `key` (e.g. `"Enter"`, `"Control+a"`).
    pub fn press(&self, key: impl Into<String>) -> Press {
        self.action("elementHandle.press").with_text(key)
    }

    /// Check a checkbox or radio button.
    pub fn check(&self) -> SetChecked {
        self.action("elementHandle.check").with_flag(true)
    }

    /// Uncheck a checkbox.
    pub fn uncheck(&self) -> SetChecked {
        self.action("elementHandle.uncheck").with_flag(false)
    }

    /// Check or uncheck a checkbox or radio button.
    pub fn set_checked(&self, checked: bool) -> SetChecked {
        self.action("elementHandle.setChecked").with_flag(checked)
    }

    /// Select options of a `<select>`; resolves with the selected values.
    pub fn select_option(&self, values: impl Into<SelectValues>) -> SelectOptionAction {
        let mut action = self.action("elementHandle.selectOption");
        action.args.values = values.into();
        action
    }

    /// Select the text of the element.
    pub fn select_text(&self) -> Action<kinds::SelectText> {
        self.action("elementHandle.selectText")
    }

    /// Set the files of an `<input type=file>`.
    pub fn set_input_files(&self, files: impl Into<InputFiles>) -> SetInputFiles {
        let mut action = self.action("elementHandle.setInputFiles");
        action.args.files = Some(files.into());
        action
    }

    /// Focus the element.
    pub fn focus(&self) -> Action<kinds::Focus> {
        self.action("elementHandle.focus")
    }

    /// Dispatch a DOM event of type `type_` on the element.
    pub fn dispatch_event(&self, type_: impl Into<String>) -> Action<kinds::DispatchEvent> {
        self.action("elementHandle.dispatchEvent").with_text(type_)
    }

    /// Scroll the element into view if needed.
    pub fn scroll_into_view_if_needed(&self) -> Action<kinds::ScrollIntoView> {
        self.action("elementHandle.scrollIntoViewIfNeeded")
    }

    /// Wait until the element reaches `state`.
    pub fn wait_for_element_state(
        &self,
        state: ElementState,
    ) -> Action<kinds::WaitForElementState> {
        self.action("elementHandle.waitForElementState")
            .with_text(state.as_str())
    }

    /// Wait for a selector relative to this element.
    pub fn wait_for_selector(&self, selector: impl Into<String>) -> WaitForSelector {
        self.action("elementHandle.waitForSelector")
            .with_text(selector)
    }

    /// `textContent` of the element.
    pub async fn text_content(&self) -> Result<Option<String>> {
        self.action::<kinds::TextContent>("elementHandle.textContent")
            .await
    }

    /// `innerText` of the element.
    pub async fn inner_text(&self) -> Result<String> {
        self.action::<kinds::InnerText>("elementHandle.innerText")
            .await
    }

    /// `innerHTML` of the element.
    pub async fn inner_html(&self) -> Result<String> {
        self.action::<kinds::InnerHtml>("elementHandle.innerHTML")
            .await
    }

    /// `value` of an `<input>`, `<textarea>` or `<select>`.
    pub async fn input_value(&self) -> Result<String> {
        self.action::<kinds::InputValue>("elementHandle.inputValue")
            .await
    }

    async fn state(&self, api: &'static str, state: &str) -> Result<bool> {
        self.action::<kinds::IsState>(api).with_text(state).await
    }

    /// Whether a checkbox or radio is checked.
    pub async fn is_checked(&self) -> Result<bool> {
        self.state("elementHandle.isChecked", "checked").await
    }

    /// Whether the element is disabled.
    pub async fn is_disabled(&self) -> Result<bool> {
        self.state("elementHandle.isDisabled", "disabled").await
    }

    /// Whether the element is editable.
    pub async fn is_editable(&self) -> Result<bool> {
        self.state("elementHandle.isEditable", "editable").await
    }

    /// Whether the element is enabled.
    pub async fn is_enabled(&self) -> Result<bool> {
        self.state("elementHandle.isEnabled", "enabled").await
    }

    /// Whether the element is hidden.
    pub async fn is_hidden(&self) -> Result<bool> {
        self.action::<kinds::IsVisible>("elementHandle.isHidden")
            .with_flag(true)
            .await
    }

    /// Whether the element is visible.
    pub async fn is_visible(&self) -> Result<bool> {
        self.action::<kinds::IsVisible>("elementHandle.isVisible")
            .await
    }

    /// The bounding box of the element, `None` if it is not rendered.
    pub async fn bounding_box(&self) -> Result<Option<BoundingBox>> {
        self.bounding_box_impl()
            .await
            .map_err(|e| e.with_api("elementHandle.boundingBox"))
    }

    /// The first element matching `selector` inside this element.
    pub async fn query_selector(&self, selector: &str) -> Result<Option<ElementHandle>> {
        async {
            let frame = self.owner_frame_sync()?;
            query(
                &frame,
                &SelectorTarget {
                    selector: selector.to_string(),
                    strict: false,
                    scope: Some(self.clone()),
                },
            )
            .await
        }
        .await
        .map_err(|e| e.with_api("elementHandle.$"))
    }

    /// All elements matching `selector` inside this element.
    pub async fn query_selector_all(&self, selector: &str) -> Result<Vec<ElementHandle>> {
        async {
            let frame = self.owner_frame_sync()?;
            query_all(&frame, selector, Some(self.clone())).await
        }
        .await
        .map_err(|e| e.with_api("elementHandle.$$"))
    }

    /// Evaluate `expression` on the first element matching `selector`
    /// inside this element (`$eval`).
    pub fn eval_on_selector<T: DeserializeOwned>(
        &self,
        selector: impl Into<String>,
        expression: impl Into<String>,
    ) -> Action<kinds::EvalOnSelector<T>> {
        let mut action = self.action("elementHandle.$eval").with_text(expression);
        action.args.other = Some(selector.into());
        action
    }

    /// Evaluate `expression` on all elements matching `selector` inside this
    /// element (`$$eval`).
    pub fn eval_on_selector_all<T: DeserializeOwned>(
        &self,
        selector: impl Into<String>,
        expression: impl Into<String>,
    ) -> Action<kinds::EvaluateAll<T>> {
        let mut action = self.action("elementHandle.$$eval").with_text(expression);
        action.args.other = Some(selector.into());
        action
    }
}

// ----------------------------------------------------------------------
// Frame / Page selector API
// ----------------------------------------------------------------------

/// Builder returned by query_selector of [Page](crate::Page) /
/// [Frame]; .await it.
#[must_use = "builders do nothing unless awaited"]
pub struct QuerySelector {
    frame: Frame,
    selector: String,
    strict: bool,
    api: &'static str,
}

impl QuerySelector {
    /// Fail when the selector matches more than one element.
    pub fn strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }
}

impl IntoFuture for QuerySelector {
    type Output = Result<Option<ElementHandle>>;
    type IntoFuture = BoxFuture<Result<Option<ElementHandle>>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            query(
                &self.frame,
                &SelectorTarget::new(self.selector, self.strict),
            )
            .await
            .map_err(|e| e.with_api(self.api))
        })
    }
}

macro_rules! selector_api {
    ($ty:ty, $prefix:literal, $frame:ident => $get_frame:expr) => {
        impl $ty {
            fn selector_action<K>(
                &self,
                api: &'static str,
                selector: impl Into<String>,
            ) -> Action<K> {
                let $frame = self;
                let frame: Frame = $get_frame;
                Action::new(Target::selector(&frame, selector, false), api)
            }

            /// Click the element matching `selector`.
            pub fn click(&self, selector: impl Into<String>) -> Click {
                self.selector_action(concat!($prefix, ".click"), selector)
            }

            /// Double click the element matching `selector`.
            pub fn dblclick(&self, selector: impl Into<String>) -> Dblclick {
                self.selector_action(concat!($prefix, ".dblclick"), selector)
            }

            /// Tap the element matching `selector`.
            pub fn tap(&self, selector: impl Into<String>) -> Tap {
                self.selector_action(concat!($prefix, ".tap"), selector)
            }

            /// Hover over the element matching `selector`.
            pub fn hover(&self, selector: impl Into<String>) -> Hover {
                self.selector_action(concat!($prefix, ".hover"), selector)
            }

            /// Fill the element matching `selector` with `value`.
            pub fn fill(&self, selector: impl Into<String>, value: impl Into<String>) -> Fill {
                self.selector_action(concat!($prefix, ".fill"), selector)
                    .with_text(value)
            }

            /// Focus the element matching `selector` and type `text`.
            pub fn r#type(&self, selector: impl Into<String>, text: impl Into<String>) -> Type {
                self.selector_action(concat!($prefix, ".type"), selector)
                    .with_text(text)
            }

            /// Alias of `type`.
            pub fn type_text(&self, selector: impl Into<String>, text: impl Into<String>) -> Type {
                self.r#type(selector, text)
            }

            /// Focus the element matching `selector` and press `key`.
            pub fn press(&self, selector: impl Into<String>, key: impl Into<String>) -> Press {
                self.selector_action(concat!($prefix, ".press"), selector)
                    .with_text(key)
            }

            /// Check the checkbox/radio matching `selector`.
            pub fn check(&self, selector: impl Into<String>) -> SetChecked {
                self.selector_action(concat!($prefix, ".check"), selector)
                    .with_flag(true)
            }

            /// Uncheck the checkbox matching `selector`.
            pub fn uncheck(&self, selector: impl Into<String>) -> SetChecked {
                self.selector_action(concat!($prefix, ".uncheck"), selector)
                    .with_flag(false)
            }

            /// Check or uncheck the element matching `selector`.
            pub fn set_checked(&self, selector: impl Into<String>, checked: bool) -> SetChecked {
                self.selector_action(concat!($prefix, ".setChecked"), selector)
                    .with_flag(checked)
            }

            /// Select options of the `<select>` matching `selector`.
            pub fn select_option(
                &self,
                selector: impl Into<String>,
                values: impl Into<SelectValues>,
            ) -> SelectOptionAction {
                let mut action = self.selector_action(concat!($prefix, ".selectOption"), selector);
                action.args.values = values.into();
                action
            }

            /// Set the files of the file input matching `selector`.
            pub fn set_input_files(
                &self,
                selector: impl Into<String>,
                files: impl Into<InputFiles>,
            ) -> SetInputFiles {
                let mut action = self.selector_action(concat!($prefix, ".setInputFiles"), selector);
                action.args.files = Some(files.into());
                action
            }

            /// Focus the element matching `selector`.
            pub fn focus(&self, selector: impl Into<String>) -> Action<kinds::Focus> {
                self.selector_action(concat!($prefix, ".focus"), selector)
            }

            /// Dispatch an event on the element matching `selector`.
            pub fn dispatch_event(
                &self,
                selector: impl Into<String>,
                type_: impl Into<String>,
            ) -> Action<kinds::DispatchEvent> {
                self.selector_action(concat!($prefix, ".dispatchEvent"), selector)
                    .with_text(type_)
            }

            /// `textContent` of the element matching `selector`.
            pub fn text_content(&self, selector: impl Into<String>) -> Action<kinds::TextContent> {
                self.selector_action(concat!($prefix, ".textContent"), selector)
            }

            /// `innerText` of the element matching `selector`.
            pub fn inner_text(&self, selector: impl Into<String>) -> Action<kinds::InnerText> {
                self.selector_action(concat!($prefix, ".innerText"), selector)
            }

            /// `innerHTML` of the element matching `selector`.
            pub fn inner_html(&self, selector: impl Into<String>) -> Action<kinds::InnerHtml> {
                self.selector_action(concat!($prefix, ".innerHTML"), selector)
            }

            /// An attribute of the element matching `selector`.
            pub fn get_attribute(
                &self,
                selector: impl Into<String>,
                name: impl Into<String>,
            ) -> Action<kinds::GetAttribute> {
                self.selector_action(concat!($prefix, ".getAttribute"), selector)
                    .with_text(name)
            }

            /// `value` of the input matching `selector`.
            pub fn input_value(&self, selector: impl Into<String>) -> Action<kinds::InputValue> {
                self.selector_action(concat!($prefix, ".inputValue"), selector)
            }

            /// Whether the element matching `selector` is checked.
            pub fn is_checked(&self, selector: impl Into<String>) -> Action<kinds::IsState> {
                self.selector_action(concat!($prefix, ".isChecked"), selector)
                    .with_text("checked")
            }

            /// Whether the element matching `selector` is disabled.
            pub fn is_disabled(&self, selector: impl Into<String>) -> Action<kinds::IsState> {
                self.selector_action(concat!($prefix, ".isDisabled"), selector)
                    .with_text("disabled")
            }

            /// Whether the element matching `selector` is editable.
            pub fn is_editable(&self, selector: impl Into<String>) -> Action<kinds::IsState> {
                self.selector_action(concat!($prefix, ".isEditable"), selector)
                    .with_text("editable")
            }

            /// Whether the element matching `selector` is enabled.
            pub fn is_enabled(&self, selector: impl Into<String>) -> Action<kinds::IsState> {
                self.selector_action(concat!($prefix, ".isEnabled"), selector)
                    .with_text("enabled")
            }

            /// Whether the element matching `selector` is hidden (does not wait).
            pub fn is_hidden(&self, selector: impl Into<String>) -> Action<kinds::IsVisible> {
                self.selector_action(concat!($prefix, ".isHidden"), selector)
                    .with_flag(true)
            }

            /// Whether the element matching `selector` is visible (does not wait).
            pub fn is_visible(&self, selector: impl Into<String>) -> Action<kinds::IsVisible> {
                self.selector_action(concat!($prefix, ".isVisible"), selector)
            }

            /// Wait for an element matching `selector` to reach a state
            /// (default visible); resolves with the element (`None` for
            /// hidden/detached).
            pub fn wait_for_selector(&self, selector: impl Into<String>) -> WaitForSelector {
                self.selector_action(concat!($prefix, ".waitForSelector"), selector)
            }

            /// Drag the element matching `source` onto the one matching `target`.
            pub fn drag_and_drop(
                &self,
                source: impl Into<String>,
                target: impl Into<String>,
            ) -> Action<kinds::DragAndDrop> {
                let mut action = self.selector_action(concat!($prefix, ".dragAndDrop"), source);
                action.args.other = Some(target.into());
                action
            }

            /// Evaluate `expression` on the first element matching `selector` (`$eval`).
            pub fn eval_on_selector<T: DeserializeOwned>(
                &self,
                selector: impl Into<String>,
                expression: impl Into<String>,
            ) -> Action<kinds::EvalOnSelector<T>> {
                let selector = selector.into();
                let mut action = self
                    .selector_action(concat!($prefix, ".$eval"), selector.clone())
                    .with_text(expression);
                action.args.other = Some(selector);
                action
            }

            /// Evaluate `expression` on all elements matching `selector` (`$$eval`).
            pub fn eval_on_selector_all<T: DeserializeOwned>(
                &self,
                selector: impl Into<String>,
                expression: impl Into<String>,
            ) -> Action<kinds::EvaluateAll<T>> {
                self.selector_action(concat!($prefix, ".$$eval"), selector)
                    .with_text(expression)
            }

            /// The first element matching selector ($), without waiting.
            /// Builder: .strict(true) fails when several elements match;
            /// .await it.
            pub fn query_selector(&self, selector: &str) -> QuerySelector {
                let $frame = self;
                let frame: Frame = $get_frame;
                QuerySelector {
                    frame,
                    selector: selector.to_string(),
                    strict: false,
                    api: concat!($prefix, ".$"),
                }
            }

            /// All elements matching `selector` (`$$`).
            pub async fn query_selector_all(&self, selector: &str) -> Result<Vec<ElementHandle>> {
                let $frame = self;
                let frame: Frame = $get_frame;
                query_all(&frame, selector, None)
                    .await
                    .map_err(|e| e.with_api(concat!($prefix, ".$$")))
            }
        }
    };
}

selector_api!(Frame, "frame", f => f.clone());
selector_api!(Page, "page", p => p.main_frame());
