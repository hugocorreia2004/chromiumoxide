//! JavaScript worlds used for evaluation.
//!
//! With the `stealth` feature no `Runtime.enable` is sent, so the browser
//! never reports execution contexts. Evaluation then runs in an isolated world
//! created on demand per frame with `Page.createIsolatedWorld` (a random name,
//! universal access), cached per frame document by the target and dropped when
//! the frame navigates, detaches or changes session. DOM objects handed out
//! by element APIs are resolved into that world, so page scripts never observe
//! calls made by this crate.
//!
//! Code that has to touch the page's own globals runs in the main world with
//! the technique Patchright uses: `Runtime.evaluate("globalThis")` without a
//! context id in the frame's session (or the frame's document resolved through
//! its owner element for a same-process subframe) yields an object of the main
//! world, and `Runtime.callFunctionOn` on that object runs the function there.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

use chromiumoxide_cdp::cdp::browser_protocol::dom::{
    DescribeNodeParams, GetFrameOwnerParams, ResolveNodeParams,
};
use chromiumoxide_cdp::cdp::browser_protocol::page::FrameId;
use chromiumoxide_cdp::cdp::browser_protocol::target::SessionId;
use chromiumoxide_cdp::cdp::js_protocol::runtime::{
    CallFunctionOnParams, EvaluateParams, ExecutionContextId, ReleaseObjectParams, RemoteObjectId,
};

use crate::error::{CdpError, Result};
use crate::handler::PageInner;
use crate::js::{Evaluation, EvaluationResult};

/// An isolated world of one frame document.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(feature = "stealth"), allow(dead_code))]
pub(crate) struct IsolatedWorld {
    pub(crate) frame_id: FrameId,
    pub(crate) session_id: SessionId,
    pub(crate) context_id: ExecutionContextId,
}

/// A world name made of random lowercase letters: it must not identify the
/// automation library to the page.
#[cfg_attr(not(feature = "stealth"), allow(dead_code))]
pub(crate) fn random_world_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    let mut bits = hasher.finish();
    (0..10)
        .map(|_| {
            let letter = (b'a' + (bits % 26) as u8) as char;
            bits /= 26;
            letter
        })
        .collect()
}

/// Whether the browser rejected a command because the execution context it
/// names no longer exists (the document went away). Such a command never
/// ran, so it can be retried in a fresh world.
#[cfg_attr(not(feature = "stealth"), allow(dead_code))]
pub(crate) fn is_stale_context_error(error: &CdpError) -> bool {
    match error {
        CdpError::Chrome(error) => {
            let message = error.message.to_ascii_lowercase();
            message.contains("context")
                && (message.contains("cannot find") || message.contains("not found"))
        }
        _ => false,
    }
}

/// The execution context an object id belongs to. Remote object ids are
/// `<isolate>.<context>.<object>`.
pub(crate) fn context_id_from_object_id(object_id: &RemoteObjectId) -> Option<ExecutionContextId> {
    let mut parts = object_id.inner().split('.');
    let _isolate = parts.next()?;
    let context = parts.next()?.parse::<i64>().ok()?;
    parts.next()?;
    Some(ExecutionContextId::new(context))
}

/// Evaluate in the main world of `frame_id`, a frame owned by `session_id`.
///
/// `session_root` tells whether the frame is the root frame of its session
/// (the page's main frame, or an out-of-process iframe); other frames are
/// same-process subframes reached through their owner element.
pub(crate) async fn evaluate_main_world(
    tab: &PageInner,
    frame_id: &FrameId,
    session_id: &SessionId,
    session_root: bool,
    evaluation: Evaluation,
) -> Result<EvaluationResult> {
    match (evaluation, session_root) {
        (Evaluation::Expression(mut expression), true) => {
            // Without a context id the session's root frame main world is used.
            expression.context_id = None;
            expression.unique_context_id = None;
            evaluate(tab, session_id, expression).await
        }
        (evaluation, _) => {
            let (anchor, this_is_document) = if session_root {
                (global_object(tab, session_id).await?, false)
            } else {
                (subframe_document(tab, frame_id, session_id).await?, true)
            };
            let result = match evaluation {
                Evaluation::Function(mut function) => {
                    if this_is_document {
                        function.function_declaration = format!(
                            "function(...args) {{ return ({}).apply(this.defaultView, args); }}",
                            function.function_declaration
                        );
                    }
                    function.object_id = Some(anchor.clone());
                    function.execution_context_id = None;
                    function.unique_context_id = None;
                    call_function(tab, session_id, function).await
                }
                Evaluation::Expression(mut expression) => {
                    expression.context_id =
                        Some(context_id_from_object_id(&anchor).ok_or_else(|| {
                            CdpError::msg("Could not locate the frame's main world")
                        })?);
                    expression.unique_context_id = None;
                    evaluate(tab, session_id, expression).await
                }
            };
            let _ = tab
                .execute_with_session(ReleaseObjectParams::new(anchor), session_id)
                .await;
            result
        }
    }
}

/// `globalThis` of the main world of the session's root frame.
async fn global_object(tab: &PageInner, session_id: &SessionId) -> Result<RemoteObjectId> {
    let response = tab
        .execute_with_session(
            EvaluateParams::builder()
                .expression("globalThis")
                .return_by_value(false)
                .build()
                .map_err(CdpError::msg)?,
            session_id,
        )
        .await?
        .result;
    if let Some(exception) = response.exception_details {
        return Err(CdpError::JavascriptException(Box::new(exception)));
    }
    response
        .result
        .object_id
        .ok_or_else(|| CdpError::msg("globalThis did not yield an object id"))
}

/// The document of a same-process subframe, resolved in its main world.
async fn subframe_document(
    tab: &PageInner,
    frame_id: &FrameId,
    session_id: &SessionId,
) -> Result<RemoteObjectId> {
    let owner = tab
        .execute_with_session(GetFrameOwnerParams::new(frame_id.clone()), session_id)
        .await?
        .result
        .backend_node_id;
    let owner = tab
        .execute_with_session(
            DescribeNodeParams::builder().backend_node_id(owner).build(),
            session_id,
        )
        .await?
        .result
        .node;
    let document = owner
        .content_document
        .ok_or_else(|| CdpError::msg("The frame has no document yet"))?
        .backend_node_id;
    tab.execute_with_session(
        ResolveNodeParams::builder()
            .backend_node_id(document)
            .build(),
        session_id,
    )
    .await?
    .result
    .object
    .object_id
    .ok_or_else(|| CdpError::msg("The frame document did not yield an object id"))
}

pub(crate) async fn evaluate(
    tab: &PageInner,
    session_id: &SessionId,
    mut expression: EvaluateParams,
) -> Result<EvaluationResult> {
    if expression.await_promise.is_none() {
        expression.await_promise = Some(true);
    }
    if expression.return_by_value.is_none() {
        expression.return_by_value = Some(true);
    }
    let response = tab
        .execute_with_session(expression, session_id)
        .await?
        .result;
    if let Some(exception) = response.exception_details {
        return Err(CdpError::JavascriptException(Box::new(exception)));
    }
    Ok(EvaluationResult::new(response.result))
}

pub(crate) async fn call_function(
    tab: &PageInner,
    session_id: &SessionId,
    mut function: CallFunctionOnParams,
) -> Result<EvaluationResult> {
    if function.await_promise.is_none() {
        function.await_promise = Some(true);
    }
    if function.return_by_value.is_none() {
        function.return_by_value = Some(true);
    }
    let response = tab.execute_with_session(function, session_id).await?.result;
    if let Some(exception) = response.exception_details {
        return Err(CdpError::JavascriptException(Box::new(exception)));
    }
    Ok(EvaluationResult::new(response.result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_names_are_random_letters() {
        let first = random_world_name();
        let second = random_world_name();
        assert_eq!(first.len(), 10);
        assert!(first.chars().all(|c| c.is_ascii_lowercase()));
        assert_ne!(first, second);
        assert!(!first.contains("chromiumoxide"));
    }

    #[test]
    fn context_id_is_read_from_the_object_id() {
        let id = RemoteObjectId::new("-4129346453263578923.7.1");
        assert_eq!(
            context_id_from_object_id(&id),
            Some(ExecutionContextId::new(7))
        );
        assert_eq!(
            context_id_from_object_id(&RemoteObjectId::new("garbage")),
            None
        );
    }

    #[test]
    fn stale_context_errors_are_recognized() {
        let stale = CdpError::Chrome(chromiumoxide_types::Error {
            code: -32000,
            message: "Cannot find context with specified id".to_owned(),
        });
        assert!(is_stale_context_error(&stale));
        let other = CdpError::Chrome(chromiumoxide_types::Error {
            code: -32000,
            message: "Object reference chain is too long".to_owned(),
        });
        assert!(!is_stale_context_error(&other));
        assert!(!is_stale_context_error(&CdpError::Timeout));
    }
}
