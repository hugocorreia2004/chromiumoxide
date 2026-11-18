//! Execution contexts ("worlds") of a frame.
//!
//! Contexts are created on demand and never discovered through `Runtime`
//! events (`Runtime.enable` is never sent):
//!
//! * the **utility** world is an isolated world created with
//!   `Page.createIsolatedWorld` (random world name, universal access);
//! * the **main** world is reached through an object living in it: the
//!   `globalThis` returned by `Runtime.evaluate` on the frame's session (root
//!   frames) or the frame's document resolved via `DOM.getFrameOwner` /
//!   `DOM.describeNode` / `DOM.resolveNode` (in-process child frames). Every
//!   call then uses `Runtime.callFunctionOn` on objects of that world.
//!
//! The context object is cached per frame and invalidated when the frame
//! commits a new document, detaches, or its session goes away.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use chromiumoxide::cdp::browser_protocol::target::SessionId;
use chromiumoxide::cdp::js_protocol::runtime::{
    CallArgument, CallFunctionOnParams, EvaluateParams, ExceptionDetails, ExecutionContextId,
    ReleaseObjectParams, RemoteObject, RemoteObjectId, RemoteObjectSubtype, RemoteObjectType,
};
use chromiumoxide::error::CdpError;
use chromiumoxide::types::{Command, CommandResponse};
use serde_json::Value;

use crate::error::{CONTEXT_DESTROYED_MESSAGE, Error, Result, is_session_closed};
use crate::frame::{Frame, FrameInner};
use crate::js::{self, JsValue, sources};
use crate::js_handle::JSHandle;

static DEFAULT_ISOLATED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(cfg!(feature = "stealth"));

/// Change where `evaluate`, `evaluate_handle`, `locator.evaluate*` and
/// element handles run by default, for the whole process: `true` = an
/// isolated world the page cannot see (the default with `stealth`, like
/// Patchright's `isolatedContext`), `false` = the page's main world
/// (Playwright's behavior, needed to read page globals). Each call can still
/// choose with `.isolated_context(..)`.
pub fn set_default_isolated_context(isolated: bool) {
    DEFAULT_ISOLATED.store(isolated, std::sync::atomic::Ordering::Relaxed);
}

/// Which world of a frame a context belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum World {
    /// The page's own JavaScript world.
    Main,
    /// An isolated world private to the automation (Playwright's "utility" world).
    Utility,
}

impl World {
    /// Where `evaluate` / `evaluate_handle` run unless told otherwise. With
    /// `stealth`, like Patchright (`isolatedContext = true`): the isolated
    /// world, so page scripts never see the automation's calls (rebrowser's
    /// `mainWorldExecution` / `sourceUrlLeak`). Otherwise Playwright's main
    /// world. `.isolated_context(false)` reaches page globals either way.
    pub(crate) fn default_evaluate() -> World {
        if DEFAULT_ISOLATED.load(std::sync::atomic::Ordering::Relaxed) {
            World::Utility
        } else {
            World::Main
        }
    }

    pub(crate) fn index(self) -> usize {
        match self {
            World::Main => 0,
            World::Utility => 1,
        }
    }
}

/// How calls reach the context.
#[derive(Debug, Clone)]
pub(crate) enum ContextTarget {
    /// A context id (isolated worlds).
    Id(ExecutionContextId),
    /// An object living in the context (main worlds), usually `globalThis`.
    Object(RemoteObjectId),
}

/// The protocol session a context lives in: a frame's (page or OOPIF)
/// session, or a dedicated worker's session.
#[derive(Clone)]
pub(crate) enum ContextSession {
    Frame(chromiumoxide::Frame),
    Worker {
        browser: Weak<crate::browser::BrowserInner>,
        session: SessionId,
    },
}

impl From<chromiumoxide::Frame> for ContextSession {
    fn from(frame: chromiumoxide::Frame) -> Self {
        ContextSession::Frame(frame)
    }
}

impl ContextSession {
    /// The CDP session id.
    pub(crate) fn session_id(&self) -> &SessionId {
        match self {
            ContextSession::Frame(frame) => frame.session_id(),
            ContextSession::Worker { session, .. } => session,
        }
    }

    /// Execute a command on the session.
    pub(crate) async fn execute<C: Command>(
        &self,
        cmd: C,
    ) -> std::result::Result<CommandResponse<C::Response>, CdpError> {
        match self {
            ContextSession::Frame(frame) => frame.execute(cmd).await,
            ContextSession::Worker { browser, session } => {
                let browser = browser.upgrade().ok_or(CdpError::TargetClosed)?;
                let guard = browser.cdp_read().await;
                let cdp = guard.as_ref().ok_or(CdpError::TargetClosed)?;
                cdp.execute_with_session(cmd, Some(session.clone())).await
            }
        }
    }
}

/// An execution context of a frame. Cheap to clone.
#[derive(Clone)]
pub struct ExecutionContext {
    pub(crate) inner: Arc<ContextInner>,
}

pub(crate) struct ContextInner {
    pub(crate) frame: Weak<FrameInner>,
    pub(crate) world: World,
    pub(crate) session: ContextSession,
    pub(crate) target: ContextTarget,
    pub(crate) context_id: Option<ExecutionContextId>,
    utility_script: tokio::sync::OnceCell<RemoteObjectId>,
    injected_script: tokio::sync::OnceCell<JSHandle>,
    destroyed: AtomicBool,
}

impl std::fmt::Debug for ExecutionContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutionContext")
            .field("world", &self.inner.world)
            .field("target", &self.inner.target)
            .finish()
    }
}

/// Outcome of an evaluation.
pub(crate) enum Evaluated {
    Value(JsValue),
    Handle(JSHandle),
}

impl ExecutionContext {
    pub(crate) fn new(
        frame: Weak<FrameInner>,
        world: World,
        session: impl Into<ContextSession>,
        target: ContextTarget,
    ) -> Self {
        let session = session.into();
        let context_id = match &target {
            ContextTarget::Id(id) => Some(*id),
            ContextTarget::Object(object_id) => context_id_from_object_id(object_id),
        };
        Self {
            inner: Arc::new(ContextInner {
                frame,
                world,
                session,
                target,
                context_id,
                utility_script: Default::default(),
                injected_script: Default::default(),
                destroyed: AtomicBool::new(false),
            }),
        }
    }

    /// The world of this context.
    pub fn world(&self) -> World {
        self.inner.world
    }

    /// The frame owning this context, if it still exists.
    pub fn frame(&self) -> Option<Frame> {
        self.inner.frame.upgrade().map(Frame::from_inner)
    }

    /// Whether both values refer to the same context.
    pub fn same_context(&self, other: &ExecutionContext) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// The CDP session this context lives in.
    pub(crate) fn session(&self) -> &ContextSession {
        &self.inner.session
    }

    pub(crate) fn is_destroyed(&self) -> bool {
        self.inner.destroyed.load(Ordering::SeqCst)
    }

    pub(crate) fn mark_destroyed(&self) {
        self.inner.destroyed.store(true, Ordering::SeqCst);
    }

    /// Execute a command on this context's session.
    pub(crate) async fn send<C: Command>(&self, cmd: C) -> Result<C::Response> {
        match self.inner.session.execute(cmd).await {
            Ok(response) => Ok(response.result),
            Err(err) => Err(self.rewrite_error(err)),
        }
    }

    /// Mirrors `rewriteError` of `crExecutionContext.ts`.
    fn rewrite_error(&self, err: CdpError) -> Error {
        let message = err.to_string();
        if message.contains("Object reference chain is too long")
            || message.contains("CBOR: stack limit exceeded")
        {
            return Error::evaluation(
                "Cannot serialize result: object reference chain is too long.",
            );
        }
        if message.contains("Promise was collected") {
            return Error::evaluation("Resulting promise was garbage collected.");
        }
        if is_session_closed(&err) {
            return Error::target_closed();
        }
        if matches!(err, CdpError::FrameNotReady) || self.is_destroyed() {
            return Error::evaluation(CONTEXT_DESTROYED_MESSAGE);
        }
        if let CdpError::Chrome(_) = err {
            return Error::evaluation(CONTEXT_DESTROYED_MESSAGE);
        }
        Error::from(err)
    }

    /// Evaluate a raw expression and return the remote object (no serializer).
    pub(crate) async fn raw_evaluate_handle(&self, expression: &str) -> Result<RemoteObject> {
        let (result, exception) = match &self.inner.target {
            ContextTarget::Id(id) => {
                let mut params = EvaluateParams::new(expression);
                params.context_id = Some(*id);
                let r = self.send(params).await?;
                (r.result, r.exception_details)
            }
            ContextTarget::Object(object_id) => {
                let mut params =
                    CallFunctionOnParams::new(format!("function() {{ return {expression}; }}"));
                params.object_id = Some(object_id.clone());
                let r = self.send(params).await?;
                (r.result, r.exception_details)
            }
        };
        if let Some(exception) = exception {
            return Err(Error::evaluation(exception_message(&exception)));
        }
        Ok(result)
    }

    /// The `UtilityScript` instance of this context.
    pub(crate) async fn utility_script(&self) -> Result<RemoteObjectId> {
        self.inner
            .utility_script
            .get_or_try_init(|| async {
                let source =
                    js::instantiate_source(sources::UTILITY_SCRIPT, "UtilityScript", "false");
                let object = self.raw_evaluate_handle(&source).await?;
                object
                    .object_id
                    .ok_or_else(|| Error::evaluation("Failed to create the utility script"))
            })
            .await
            .cloned()
    }

    /// The `InjectedScript` instance of this context, created with the same
    /// options Playwright's server uses for Chromium.
    pub async fn injected_script(&self) -> Result<JSHandle> {
        self.inner
            .injected_script
            .get_or_try_init(|| async {
                let frame_seq = self
                    .inner
                    .frame
                    .upgrade()
                    .map(|f| f.seq)
                    .unwrap_or_default();
                let test_id_attribute = self
                    .inner
                    .frame
                    .upgrade()
                    .and_then(|f| f.page.upgrade())
                    .map(|p| p.test_id_attribute_name())
                    .unwrap_or_else(|| "data-testid".to_string());
                let options = serde_json::json!({
                    "isUnderTest": false,
                    "sdkLanguage": "javascript",
                    "frameSeq": frame_seq,
                    "testIdAttributeName": test_id_attribute,
                    "stableRafCount": 1,
                    "browserName": "chromium",
                    "shouldPrependErrorPrefix": false,
                    "isUtilityWorld": self.inner.world == World::Utility,
                    "customEngines": crate::selectors::custom_engines_json(),
                });
                let source = js::instantiate_source(
                    sources::INJECTED_SCRIPT,
                    "InjectedScript",
                    &options.to_string(),
                );
                let object = self.raw_evaluate_handle(&source).await?;
                let handle = self.create_handle(object);
                handle.set_preview("InjectedScript");
                Ok(handle)
            })
            .await
            .cloned()
    }

    /// `UtilityScript.evaluate` with Playwright's argument serialization:
    /// `args` are wire values whose `{ h: n }` entries index into `handles`.
    pub(crate) async fn evaluate_expression(
        &self,
        expression: &str,
        is_function: Option<bool>,
        return_by_value: bool,
        args: Vec<Value>,
        handles: Vec<JSHandle>,
    ) -> Result<Evaluated> {
        let expression = js::normalize_expression(expression);
        let mut adopted = Vec::new();
        let mut handles = handles;
        for handle in handles.iter_mut() {
            if handle.is_disposed() {
                return Err(Error::evaluation("JSHandle is disposed!"));
            }
            if handle.context().same_context(self) || handle.inline_value().is_some() {
                continue;
            }
            if !handle.is_element() {
                return Err(Error::evaluation(
                    "JSHandles can be evaluated only in the context they were created!",
                ));
            }
            // `adoptIfNeeded`: element handles move between worlds/frames.
            let moved = self.adopt_element(handle).await?;
            adopted.push(moved.clone());
            *handle = moved;
        }
        let result = self
            .evaluate_with_handles(expression, is_function, return_by_value, args, handles)
            .await;
        for handle in adopted {
            handle.dispose().await;
        }
        result
    }

    async fn evaluate_with_handles(
        &self,
        expression: String,
        is_function: Option<bool>,
        return_by_value: bool,
        args: Vec<Value>,
        handles: Vec<JSHandle>,
    ) -> Result<Evaluated> {
        let mut values = Vec::with_capacity(4 + args.len());
        values.push(is_function.map(Value::Bool));
        values.push(Some(Value::Bool(return_by_value)));
        values.push(Some(Value::String(expression)));
        values.push(Some(Value::from(args.len())));
        values.extend(args.into_iter().map(Some));
        self.call_utility(
            "(utilityScript, ...args) => utilityScript.evaluate(...args)",
            return_by_value,
            values,
            handles,
        )
        .await
    }

    /// `Runtime.callFunctionOn` on the utility script, the way
    /// `CRExecutionContext.evaluateWithArguments` does it.
    pub(crate) async fn call_utility(
        &self,
        function: &str,
        return_by_value: bool,
        values: Vec<Option<Value>>,
        handles: Vec<JSHandle>,
    ) -> Result<Evaluated> {
        let utility = self.utility_script().await?;
        let mut arguments = Vec::with_capacity(1 + values.len() + handles.len());
        arguments.push(object_argument(utility.clone()));
        for value in values {
            arguments.push(CallArgument {
                value,
                unserializable_value: None,
                object_id: None,
            });
        }
        for handle in &handles {
            match handle.object_id() {
                Some(id) => arguments.push(object_argument(id.clone())),
                None => arguments.push(CallArgument {
                    value: None,
                    unserializable_value: None,
                    object_id: None,
                }),
            }
        }
        let mut params = CallFunctionOnParams::new(function);
        params.object_id = Some(utility);
        params.arguments = Some(arguments);
        params.return_by_value = Some(return_by_value);
        params.await_promise = Some(true);
        params.user_gesture = Some(true);
        let response = match self.inner.session.execute(params).await {
            Ok(response) => response.result,
            Err(err) => {
                if return_by_value
                    && err
                        .to_string()
                        .contains("Object couldn't be returned by value")
                {
                    return Ok(Evaluated::Value(JsValue::Undefined));
                }
                return Err(self.rewrite_error(err));
            }
        };
        if let Some(exception) = response.exception_details {
            return Err(Error::evaluation(exception_message(&exception)));
        }
        if return_by_value {
            let value = match response.result.value {
                Some(value) => JsValue::from_wire(&value),
                None => JsValue::Undefined,
            };
            Ok(Evaluated::Value(value))
        } else {
            Ok(Evaluated::Handle(self.create_handle(response.result)))
        }
    }

    /// Resolve the DOM node of an element handle from another context in
    /// this context (`CRPage.adoptElementHandle`).
    pub(crate) async fn adopt_element(&self, handle: &JSHandle) -> Result<JSHandle> {
        let foreign = handle.context();
        if foreign.session().session_id() != self.session().session_id() {
            return Err(Error::evaluation(
                "Unable to adopt element handle from a different document",
            ));
        }
        let Some(object_id) = handle.object_id().cloned() else {
            return Err(Error::evaluation("JSHandle is disposed!"));
        };
        let mut describe = chromiumoxide::cdp::browser_protocol::dom::DescribeNodeParams::default();
        describe.object_id = Some(object_id);
        let node = self.send(describe).await?.node;
        let mut resolve = chromiumoxide::cdp::browser_protocol::dom::ResolveNodeParams::default();
        resolve.backend_node_id = Some(node.backend_node_id);
        resolve.execution_context_id = self.inner.context_id;
        let object = self
            .send(resolve)
            .await
            .map_err(|_| {
                Error::evaluation("Unable to adopt element handle from a different document")
            })?
            .object;
        Ok(JSHandle::from_remote_quiet(self.clone(), object))
    }

    /// Wrap a remote object into a handle of this context.
    pub(crate) fn create_handle(&self, remote: RemoteObject) -> JSHandle {
        JSHandle::from_remote(self.clone(), remote)
    }

    /// Release a remote object of this context (errors are ignored).
    pub(crate) async fn release_object(&self, object_id: RemoteObjectId) {
        let _ = self
            .inner
            .session
            .execute(ReleaseObjectParams::new(object_id))
            .await;
    }
}

fn object_argument(object_id: RemoteObjectId) -> CallArgument {
    CallArgument {
        value: None,
        unserializable_value: None,
        object_id: Some(object_id),
    }
}

/// Remote object ids look like `<isolate>.<context>.<object>`; Patchright
/// relies on that to recover the context id of the main world.
fn context_id_from_object_id(object_id: &RemoteObjectId) -> Option<ExecutionContextId> {
    let raw: &str = object_id.as_ref();
    let mut parts = raw.split('.');
    let _isolate = parts.next()?;
    let context = parts.next()?;
    parts.next()?;
    context.parse::<i64>().ok().map(ExecutionContextId::new)
}

/// Mirrors `getExceptionMessage` of `crProtocolHelper.ts`.
pub(crate) fn exception_message(details: &ExceptionDetails) -> String {
    if let Some(exception) = &details.exception {
        if let Some(description) = &exception.description {
            return description.clone();
        }
        return match &exception.value {
            Some(Value::String(s)) => s.clone(),
            Some(value) => value.to_string(),
            None => match (&exception.r#type, &exception.unserializable_value) {
                (_, Some(v)) => v.as_ref().to_string(),
                (RemoteObjectType::Undefined, _) => "undefined".to_string(),
                _ => match exception.subtype {
                    Some(RemoteObjectSubtype::Null) => "null".to_string(),
                    _ => "undefined".to_string(),
                },
            },
        };
    }
    let mut message = details.text.clone();
    if let Some(stack) = &details.stack_trace {
        for frame in &stack.call_frames {
            let location = format!(
                "{}:{}:{}",
                frame.url,
                frame.line_number + 1,
                frame.column_number + 1
            );
            let name = if frame.function_name.is_empty() {
                "<anonymous>"
            } else {
                frame.function_name.as_str()
            };
            message.push_str(&format!("\n    at {name} ({location})"));
        }
    }
    message
}
