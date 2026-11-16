//! Console messages and page errors.
//!
//! **Stealth note.** Chromium only reports `console.*` calls and uncaught
//! exceptions to clients that enabled the `Runtime` domain, and
//! `Runtime.enable` is detectable by page scripts. Patchright therefore
//! disables console events entirely. oxidewright makes them opt-in: create
//! the context with [`crate::NewContext::console_events`]`(true)` to receive
//! `console` and `pageerror` events, knowing that it enables `Runtime` on
//! every page of that context.

use std::fmt;
use std::sync::Arc;

use chromiumoxide::cdp::js_protocol::runtime::{
    EventConsoleApiCalled, EventExceptionThrown, EventExecutionContextCreated, ExceptionDetails,
    StackTrace,
};

use crate::events::Event;
use crate::execution_context::{ContextTarget, ExecutionContext, World};
use crate::js_handle::JSHandle;
use crate::network_manager::emit_on_context;
use crate::page::{Page, PageInner};

/// Location of a console message.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConsoleMessageLocation {
    /// Script URL.
    pub url: String,
    /// 0-based line number.
    pub line_number: u32,
    /// 0-based column number.
    pub column_number: u32,
}

/// A `console.*` call of a page. Cheap to clone.
#[derive(Clone)]
pub struct ConsoleMessage {
    inner: Arc<ConsoleInner>,
}

struct ConsoleInner {
    page: Option<Page>,
    kind: String,
    text: String,
    args: Vec<JSHandle>,
    location: ConsoleMessageLocation,
    timestamp: f64,
}

impl fmt::Debug for ConsoleMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConsoleMessage")
            .field("type", &self.inner.kind)
            .field("text", &self.inner.text)
            .finish()
    }
}

impl ConsoleMessage {
    /// `log`, `debug`, `info`, `error`, `warning`, `dir`, `dirxml`, `table`,
    /// `trace`, `clear`, `startGroup`, `startGroupCollapsed`, `endGroup`,
    /// `assert`, `profile`, `profileEnd`, `count`, `timeEnd`.
    pub fn r#type(&self) -> &str {
        &self.inner.kind
    }

    /// The text: the previews of the arguments joined with spaces.
    pub fn text(&self) -> &str {
        &self.inner.text
    }

    /// Handles to the arguments.
    pub fn args(&self) -> &[JSHandle] {
        &self.inner.args
    }

    /// Where `console.*` was called.
    pub fn location(&self) -> &ConsoleMessageLocation {
        &self.inner.location
    }

    /// The page that produced the message.
    pub fn page(&self) -> Option<Page> {
        self.inner.page.clone()
    }

    /// Milliseconds since the Unix epoch.
    pub fn timestamp(&self) -> f64 {
        self.inner.timestamp
    }
}

/// An uncaught exception of a page (`pageerror`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebError {
    /// The error name, e.g. `Error` or `TypeError`.
    pub name: String,
    /// The error message.
    pub message: String,
    /// The stack, as formatted by the browser.
    pub stack: String,
}

impl fmt::Display for WebError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.name.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{}: {}", self.name, self.message)
        }
    }
}

fn location_of(stack: Option<&StackTrace>) -> ConsoleMessageLocation {
    match stack.and_then(|s| s.call_frames.first()) {
        Some(frame) => ConsoleMessageLocation {
            url: frame.url.clone(),
            line_number: frame.line_number.max(0) as u32,
            column_number: frame.column_number.max(0) as u32,
        },
        None => ConsoleMessageLocation::default(),
    }
}

/// `Runtime.executionContextCreated` (only received with console events):
/// remember which frame a context id belongs to.
pub(crate) fn on_context_created(page: &PageInner, ev: &EventExecutionContextCreated) {
    let frame_id = ev
        .context
        .aux_data
        .as_ref()
        .and_then(|d| d.get("frameId"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let is_default = ev
        .context
        .aux_data
        .as_ref()
        .and_then(|d| d.get("isDefault"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if let (Some(frame_id), true) = (frame_id, is_default) {
        page.ext
            .console_contexts
            .lock()
            .unwrap()
            .insert(*ev.context.id.inner(), frame_id);
    }
}

/// `Runtime.consoleAPICalled`.
pub(crate) fn on_console_api(page: &PageInner, ev: &EventConsoleApiCalled) {
    if *ev.execution_context_id.inner() == 0 {
        // Messages replayed for contexts that no longer exist.
        return;
    }
    let frame_id = page
        .ext
        .console_contexts
        .lock()
        .unwrap()
        .get(ev.execution_context_id.inner())
        .cloned();
    let frame = frame_id
        .as_deref()
        .and_then(|id| page.frame_by_id(id))
        .unwrap_or_else(|| page.main_frame());
    let Some(session) = frame.cdp_frame_now() else {
        return;
    };
    let context = ExecutionContext::new(
        Arc::downgrade(&frame.inner),
        World::Main,
        session,
        ContextTarget::Id(ev.execution_context_id),
    );
    let args: Vec<JSHandle> = ev
        .args
        .iter()
        .map(|arg| JSHandle::from_remote_quiet(context.clone(), arg.clone()))
        .collect();
    let text = args
        .iter()
        .map(|a| a.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    let message = ConsoleMessage {
        inner: Arc::new(ConsoleInner {
            page: page.self_weak.upgrade().map(Page::from_inner),
            kind: ev.r#type.as_ref().to_string(),
            text,
            args,
            location: location_of(ev.stack_trace.as_ref()),
            timestamp: *ev.timestamp.inner(),
        }),
    };
    emit_on_context(page, Event::Console(message));
}

/// `exceptionToError` of `crProtocolHelper.ts`.
pub(crate) fn exception_to_error(details: &ExceptionDetails) -> WebError {
    // `getExceptionMessage`
    let message_with_stack = match &details.exception {
        Some(e) if e.description.is_some() => e.description.clone().unwrap_or_default(),
        Some(e) if e.value.is_some() => match e.value.clone().unwrap_or_default() {
            serde_json::Value::String(s) => s,
            other => other.to_string(),
        },
        _ => {
            let mut message = details
                .text
                .strip_prefix("Uncaught ")
                .unwrap_or(&details.text)
                .to_string();
            if let Some(stack) = &details.stack_trace {
                for frame in &stack.call_frames {
                    let name = if frame.function_name.is_empty() {
                        "<anonymous>"
                    } else {
                        frame.function_name.as_str()
                    };
                    message.push_str(&format!(
                        "\n    at {name} ({}:{}:{})",
                        frame.url, frame.line_number, frame.column_number
                    ));
                }
            }
            message
        }
    };
    let lines: Vec<&str> = message_with_stack.split('\n').collect();
    let (message_with_name, stack) = match lines.iter().position(|l| l.starts_with("    at")) {
        None => (message_with_stack.clone(), String::new()),
        Some(i) => (lines[..i].join("\n"), message_with_stack.clone()),
    };
    // `splitErrorMessage`
    let (name, message) = match message_with_name.find(':') {
        Some(i) => {
            let message = if i + 2 <= message_with_name.len() {
                message_with_name[i + 2..].to_string()
            } else {
                message_with_name.clone()
            };
            (message_with_name[..i].to_string(), message)
        }
        None => (String::new(), message_with_name.clone()),
    };
    WebError {
        name,
        message,
        stack,
    }
}

/// `Runtime.exceptionThrown`.
pub(crate) fn on_exception(page: &PageInner, ev: &EventExceptionThrown) {
    let error = exception_to_error(&ev.exception_details);
    page.ext.listeners.emit(Event::PageError(error.clone()));
    if let Some(context) = page.context.upgrade() {
        context.ext.listeners.emit(Event::PageError(error));
    }
}
