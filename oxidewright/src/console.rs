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
use crate::execution_context::{ContextSession, ContextTarget, ExecutionContext, World};
use crate::js_handle::JSHandle;
use crate::network_manager::emit_on_context;
use crate::page::{Page, PageInner};
use crate::worker::Worker;

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
    worker: Option<Worker>,
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

    /// The worker that produced the message, if any.
    pub fn worker(&self) -> Option<Worker> {
        self.inner.worker.clone()
    }
}

/// An uncaught exception of a page (`pageerror`). Equality compares the
/// error (name, message and stack).
#[derive(Clone, Debug)]
pub struct WebError {
    /// The error name, e.g. `Error` or `TypeError`.
    pub name: String,
    /// The error message.
    pub message: String,
    /// The stack, as formatted by the browser.
    pub stack: String,
    location: ConsoleMessageLocation,
    page: Option<Page>,
}

impl PartialEq for WebError {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.message == other.message && self.stack == other.stack
    }
}

impl Eq for WebError {}

impl WebError {
    /// Where the error was thrown (the top frame of its stack).
    pub fn location(&self) -> &ConsoleMessageLocation {
        &self.location
    }

    /// The page that produced the error.
    pub fn page(&self) -> Option<Page> {
        self.page.clone()
    }
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
    if page
        .ext
        .state
        .skip_typed_runtime_event("Runtime.consoleAPICalled")
    {
        return;
    }
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
            worker: None,
            kind: ev.r#type.as_ref().to_string(),
            text,
            args,
            location: location_of(ev.stack_trace.as_ref()),
            timestamp: *ev.timestamp.inner(),
        }),
    };
    page.ext.state.add_console(message.clone());
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
        location: ConsoleMessageLocation::default(),
        page: None,
    }
}

/// `Runtime.exceptionThrown`.
pub(crate) fn on_exception(page: &PageInner, ev: &EventExceptionThrown) {
    if page
        .ext
        .state
        .skip_typed_runtime_event("Runtime.exceptionThrown")
    {
        return;
    }
    let mut error = exception_to_error(&ev.exception_details);
    error.location = location_of(ev.exception_details.stack_trace.as_ref());
    error.page = page.self_weak.upgrade().map(Page::from_inner);
    page.ext.state.add_error(error.clone());
    page.ext.listeners.emit(Event::PageError(error.clone()));
    if let Some(context) = page.context.upgrade() {
        context.ext.listeners.emit(Event::PageError(error));
    }
}

/// `Runtime` console/exception events of a child session, tagged with the
/// session by chromiumoxide (see
/// [`chromiumoxide::handler::target::CHILD_SESSION_RUNTIME_EVENT`]).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChildSessionRuntimeEvent {
    session_id: String,
    method: String,
    params: serde_json::Value,
}

impl chromiumoxide::types::MethodType for ChildSessionRuntimeEvent {
    fn method_id() -> chromiumoxide::types::MethodId {
        chromiumoxide::handler::target::CHILD_SESSION_RUNTIME_EVENT.into()
    }
}

impl chromiumoxide::cdp::CustomEvent for ChildSessionRuntimeEvent {}

/// A child-session `Runtime` event: worker consoles and errors are reported
/// here (with their worker); the typed event that follows is skipped.
/// Out-of-process frames keep going through the typed handlers.
pub(crate) fn on_child_session_event(page: &PageInner, ev: &ChildSessionRuntimeEvent) {
    let worker = page
        .ext
        .workers
        .lock()
        .unwrap()
        .get(&ev.session_id)
        .cloned();
    let Some(worker) = worker else {
        return;
    };
    page.ext.state.skip_next_typed_runtime_event(&ev.method);
    let page_handle = page.self_weak.upgrade().map(Page::from_inner);
    match ev.method.as_str() {
        "Runtime.consoleAPICalled" => {
            let Ok(event) = serde_json::from_value::<EventConsoleApiCalled>(ev.params.clone())
            else {
                return;
            };
            let context = ExecutionContext::new(
                std::sync::Weak::new(),
                World::Main,
                ContextSession::Worker {
                    page: page.self_weak.clone(),
                    session: worker.session_id().clone(),
                },
                ContextTarget::Id(event.execution_context_id),
            );
            let args: Vec<JSHandle> = event
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
                    page: page_handle,
                    worker: Some(worker.clone()),
                    kind: event.r#type.as_ref().to_string(),
                    text,
                    args,
                    location: location_of(event.stack_trace.as_ref()),
                    timestamp: *event.timestamp.inner(),
                }),
            };
            page.ext.state.add_console(message.clone());
            worker.emit_console(message.clone());
            emit_on_context(page, Event::Console(message));
        }
        "Runtime.exceptionThrown" => {
            let Ok(event) = serde_json::from_value::<EventExceptionThrown>(ev.params.clone())
            else {
                return;
            };
            let mut error = exception_to_error(&event.exception_details);
            error.location = location_of(event.exception_details.stack_trace.as_ref());
            error.page = page_handle;
            page.ext.state.add_error(error.clone());
            page.ext.listeners.emit(Event::PageError(error.clone()));
            if let Some(context) = page.context.upgrade() {
                context.ext.listeners.emit(Event::PageError(error));
            }
        }
        _ => {}
    }
}
