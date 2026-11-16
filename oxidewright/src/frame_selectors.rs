//! Selector resolution across frames, a port of Playwright's
//! `server/frameSelectors.ts`.
//!
//! Selectors are parsed and validated in Rust ([`crate::selector_parser`])
//! and matched by the `InjectedScript` of the frame's utility world. A
//! selector that enters frames (`... >> internal:control=enter-frame >> ...`)
//! is split into per-frame chunks by the injected script's own parser, and
//! each chunk is resolved in the frame the previous chunk pointed to.

use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::dom::DescribeNodeParams;
use chromiumoxide::cdp::js_protocol::runtime::GetPropertiesParams;
use serde_json::{Value, json};

use crate::error::{CONTEXT_DESTROYED_MESSAGE, Error, Result};
use crate::execution_context::{Evaluated, ExecutionContext, World};
use crate::frame::Frame;
use crate::js::JsValue;
use crate::js::serde_impl::to_wire;
use crate::js_handle::{ElementHandle, JSHandle, rebase_handles};
use crate::locator_generators::as_locator;
use crate::selector_parser::{
    ParsedSelector, parse_selector, split_selector_by_frame, stringify_selector,
    visit_all_selector_parts,
};

/// Marker returned by the injected callback when nothing matched.
const NO_RESULT: &str = "--playwright--no--result--value--";

/// An argument of [`call_function`].
pub(crate) enum CallArg {
    /// A handle, passed as the object itself.
    Handle(JSHandle),
    /// A handle or `undefined`.
    OptHandle(Option<JSHandle>),
    /// A plain JSON value (no handles).
    Json(Value),
    /// A JavaScript value (may contain handles).
    Js(JsValue),
    /// An already serialized value and its handles.
    Wire(Value, Vec<JSHandle>),
}

/// Call the JavaScript function `function` in `ctx` with positional `args`,
/// using Playwright's serializer for arguments and results.
pub(crate) async fn call_function(
    ctx: &ExecutionContext,
    function: &str,
    args: Vec<CallArg>,
    by_value: bool,
) -> Result<Evaluated> {
    let mut handles: Vec<JSHandle> = Vec::new();
    let mut wires = Vec::with_capacity(args.len());
    for arg in args {
        match arg {
            CallArg::Handle(h) => {
                wires.push(json!({ "h": handles.len() }));
                handles.push(h);
            }
            CallArg::OptHandle(Some(h)) => {
                wires.push(json!({ "h": handles.len() }));
                handles.push(h);
            }
            CallArg::OptHandle(None) => wires.push(json!({ "v": "undefined" })),
            CallArg::Json(v) => {
                let (wire, _) = to_wire(&v).map_err(Error::serialization)?;
                wires.push(wire);
            }
            CallArg::Js(v) => {
                let (wire, hs) = to_wire(&v).map_err(Error::serialization)?;
                wires.push(rebase_handles(wire, handles.len()));
                handles.extend(hs);
            }
            CallArg::Wire(wire, hs) => {
                wires.push(rebase_handles(wire, handles.len()));
                handles.extend(hs);
            }
        }
    }
    ctx.evaluate_expression(function, Some(true), by_value, wires, handles)
        .await
}

/// Call `function` with the injected script of `ctx` as its first argument.
pub(crate) async fn call_injected(
    ctx: &ExecutionContext,
    function: &str,
    mut args: Vec<CallArg>,
    by_value: bool,
) -> Result<Evaluated> {
    let injected = ctx.injected_script().await?;
    args.insert(0, CallArg::Handle(injected));
    call_function(ctx, function, args, by_value).await
}

/// The value of an evaluation made with `by_value = true`.
pub(crate) fn value_of(evaluated: Evaluated) -> JsValue {
    match evaluated {
        Evaluated::Value(v) => v,
        Evaluated::Handle(h) => h.inline_value().cloned().unwrap_or(JsValue::Undefined),
    }
}

/// The handle of an evaluation made with `by_value = false`.
pub(crate) fn handle_of(evaluated: Evaluated) -> Result<JSHandle> {
    match evaluated {
        Evaluated::Handle(h) => Ok(h),
        Evaluated::Value(_) => Err(Error::evaluation("unexpected value result")),
    }
}

/// Error messages that only mean "try again" (Playwright throws plain
/// `Error`s for them, which its retry loops swallow).
const RETRIABLE_MESSAGES: &[&str] = &[
    CONTEXT_DESTROYED_MESSAGE,
    "Execution context was destroyed",
    "Cannot find context with specified id",
    "Unable to adopt element handle from a different document",
    "JSHandles can be evaluated only in the context they were created!",
    "Element is not attached to the DOM",
    "Failed to create the utility script",
];

/// Mirrors `Frame.isNonRetriableError`.
pub(crate) fn is_non_retriable(err: &Error, frame: Option<&Frame>) -> bool {
    if frame.is_some_and(|f| f.is_detached()) {
        return true;
    }
    match err {
        Error::Evaluation { message, .. } => {
            !RETRIABLE_MESSAGES.iter().any(|m| message.contains(m))
        }
        Error::Protocol { .. } => false,
        _ => true,
    }
}

/// Elements of a JavaScript array handle, in index order.
pub(crate) async fn array_elements(array: &JSHandle) -> Result<Vec<JSHandle>> {
    let Some(object_id) = array.object_id().cloned() else {
        return Ok(Vec::new());
    };
    let mut params = GetPropertiesParams::new(object_id);
    params.own_properties = Some(true);
    let response = array.context().send(params).await?;
    let mut items: Vec<(usize, JSHandle)> = Vec::new();
    for property in response.result {
        let Ok(index) = property.name.parse::<usize>() else {
            continue;
        };
        if let Some(value) = property.value {
            items.push((index, array.context().create_handle(value)));
        }
    }
    items.sort_by_key(|(i, _)| *i);
    Ok(items.into_iter().map(|(_, h)| h).collect())
}

/// The selector of one frame, as understood by the injected script.
#[derive(Clone)]
pub(crate) enum SelectorSpec {
    /// The selector string, parsed by `injected.parseSelector`.
    Raw(String),
    /// A `ParsedSelector` produced by the injected script.
    Parsed(JsValue),
}

impl SelectorSpec {
    fn to_arg(&self) -> CallArg {
        match self {
            SelectorSpec::Raw(s) => CallArg::Json(Value::String(s.clone())),
            SelectorSpec::Parsed(v) => CallArg::Js(v.clone()),
        }
    }
}

/// A selector resolved to the frame it applies to.
pub(crate) struct Resolved {
    pub(crate) frame: Frame,
    pub(crate) spec: SelectorSpec,
    pub(crate) scope: Option<ElementHandle>,
    pub(crate) frame_visible: bool,
}

/// What to resolve.
#[derive(Clone)]
pub(crate) struct SelectorTarget {
    pub(crate) selector: String,
    pub(crate) strict: bool,
    pub(crate) scope: Option<ElementHandle>,
}

impl SelectorTarget {
    pub(crate) fn new(selector: impl Into<String>, strict: bool) -> Self {
        SelectorTarget {
            selector: selector.into(),
            strict,
            scope: None,
        }
    }

    pub(crate) fn scoped(selector: impl Into<String>, scope: ElementHandle) -> Self {
        SelectorTarget {
            selector: selector.into(),
            strict: false,
            scope: Some(scope),
        }
    }
}

fn invalid_selector(message: impl Into<String>) -> Error {
    Error::invalid_argument(message)
}

/// Splits `selector` (in JavaScript) into per-frame `ParsedSelector`s.
const SPLIT_JS: &str = r#"(injected, selector) => {
  const parsed = injected.parseSelector(selector);
  const chunks = [];
  let chunk = { parts: [] };
  let chunkStartIndex = 0;
  for (let i = 0; i < parsed.parts.length; ++i) {
    const part = parsed.parts[i];
    if (part.name === 'internal:control' && part.body === 'any-frame') {
      chunkStartIndex = i + 1;
      continue;
    }
    if (part.name === 'internal:control' && part.body === 'enter-frame') {
      chunks.push(chunk);
      chunk = { parts: [] };
      chunkStartIndex = i + 1;
      continue;
    }
    if (parsed.capture === i)
      chunk.capture = i - chunkStartIndex;
    chunk.parts.push(part);
  }
  chunks.push(chunk);
  return chunks;
}"#;

/// Mirrors `FrameSelectors.resolveFramesForSelector`.
pub(crate) async fn resolve_frames_for_selector(
    frame: &Frame,
    target: &SelectorTarget,
    any_frame_override: bool,
) -> Result<Vec<Resolved>> {
    let selector = target.selector.as_str();
    let (any_frame, chunks) =
        split_selector_by_frame(selector).map_err(|e| invalid_selector(e.0))?;
    for chunk in &chunks {
        let mut error = None;
        visit_all_selector_parts(chunk, &mut |part, nested| {
            if error.is_some() || !nested || part.name != "internal:control" {
                return;
            }
            match part.body.as_text() {
                Some("enter-frame") => {
                    error = Some(format!(
                        "Frame locators are not allowed inside composite locators, while querying \"{}\"",
                        as_locator(selector, false)
                    ))
                }
                Some("any-frame") => {
                    error = Some(format!(
                        "frameLocator() is not allowed inside composite locators, while querying \"{}\"",
                        as_locator(selector, false)
                    ))
                }
                _ => {}
            }
        });
        if let Some(error) = error {
            return Err(invalid_selector(error));
        }
    }

    if !any_frame && !any_frame_override && chunks.len() == 1 {
        return Ok(vec![Resolved {
            frame: frame.clone(),
            spec: SelectorSpec::Raw(selector.to_string()),
            scope: target.scope.clone(),
            frame_visible: true,
        }]);
    }

    // Let the injected script parse the chunks, so that CSS and nested
    // selectors are represented exactly like Playwright does.
    let ctx = frame.utility_context().await?;
    let js_chunks = value_of(
        call_injected(
            &ctx,
            SPLIT_JS,
            vec![CallArg::Json(Value::String(selector.to_string()))],
            true,
        )
        .await?,
    );
    let js_chunks = match js_chunks {
        JsValue::Array(items) if items.len() == chunks.len() => items,
        _ => return Err(Error::evaluation("Failed to split selector by frame")),
    };
    let chunk_specs: Vec<(String, SelectorSpec)> = chunks
        .iter()
        .zip(js_chunks)
        .map(|(c, js)| (stringify_selector(c, false), SelectorSpec::Parsed(js)))
        .collect();

    if !any_frame && !any_frame_override {
        let resolved = resolve_chained(
            frame,
            target.scope.clone(),
            &chunk_specs,
            target.strict,
            true,
        )
        .await?;
        return Ok(resolved.into_iter().collect());
    }

    let mut result: Vec<Resolved> = Vec::new();
    for (start, frame_visible) in any_frame_candidates(frame, target.scope.clone()).await {
        let scope = if &start == frame {
            target.scope.clone()
        } else {
            None
        };
        let resolved = match resolve_chained(
            &start,
            scope,
            &chunk_specs,
            target.strict,
            frame_visible,
        )
        .await
        {
            Ok(Some(r)) => r,
            Ok(None) => continue,
            Err(e) => {
                if &start == frame {
                    return Err(e);
                }
                continue;
            }
        };
        if result.iter().any(|r| r.frame == resolved.frame) {
            continue;
        }
        result.push(resolved);
    }
    Ok(result)
}

/// The frame subtree of `frame` (restricted to `scope`), with visibility.
async fn any_frame_candidates(frame: &Frame, scope: Option<ElementHandle>) -> Vec<(Frame, bool)> {
    let mut result = Vec::new();
    let mut stack = vec![(frame.clone(), true, scope)];
    while let Some((frame, visible, scope)) = stack.pop() {
        result.push((frame.clone(), visible));
        let children = query_frames(
            &frame,
            &SelectorSpec::Raw("css=frame,iframe".to_string()),
            "frame,iframe",
            scope.as_ref(),
            false,
            visible,
        )
        .await
        .unwrap_or_default();
        for (child, child_visible) in children.into_iter().rev() {
            stack.push((child, child_visible, None));
        }
    }
    result
}

async fn resolve_chained(
    start: &Frame,
    scope: Option<ElementHandle>,
    chunks: &[(String, SelectorSpec)],
    strict: bool,
    start_visible: bool,
) -> Result<Option<Resolved>> {
    let mut frame = start.clone();
    let mut frame_visible = start_visible;
    for (i, (selector_string, spec)) in chunks[..chunks.len() - 1].iter().enumerate() {
        let frames = query_frames(
            &frame,
            spec,
            selector_string,
            if i == 0 { scope.as_ref() } else { None },
            strict,
            frame_visible,
        )
        .await?;
        let Some((next, visible)) = frames.into_iter().next() else {
            return Ok(None);
        };
        frame = next;
        frame_visible = visible;
    }
    let scope = if &frame != start { None } else { scope };
    Ok(Some(Resolved {
        frame,
        spec: chunks[chunks.len() - 1].1.clone(),
        scope,
        frame_visible,
    }))
}

const QUERY_FRAMES_JS: &str = r#"(injected, sel, strict, selectorString, scope) => {
  const parsed = typeof sel === 'string' ? injected.parseSelector(sel) : sel;
  const elements = injected.querySelectorAll(parsed, scope || document);
  if (strict && elements.length > 1)
    throw injected.strictModeViolationError(parsed, elements);
  for (const element of elements) {
    if (element.nodeName !== 'IFRAME' && element.nodeName !== 'FRAME')
      throw injected.createStacklessError(`Selector "${selectorString}" resolved to ${injected.previewNode(element)}, <iframe> was expected`);
  }
  return elements;
}"#;

/// Mirrors `FrameSelectors._queryFrames`: the content frames of the
/// `<iframe>`s matched by `spec` in `frame`.
async fn query_frames(
    frame: &Frame,
    spec: &SelectorSpec,
    selector_string: &str,
    scope: Option<&ElementHandle>,
    strict: bool,
    parent_visible: bool,
) -> Result<Vec<(Frame, bool)>> {
    let ctx = frame.utility_context().await?;
    let array = handle_of(
        call_injected(
            &ctx,
            QUERY_FRAMES_JS,
            vec![
                spec.to_arg(),
                CallArg::Json(Value::Bool(strict)),
                CallArg::Json(Value::String(selector_string.to_string())),
                CallArg::OptHandle(scope.map(|s| s.as_js_handle().clone())),
            ],
            false,
        )
        .await
        .map_err(rewrite_strict)?,
    )?;
    let visible: Vec<bool> = if parent_visible {
        match value_of(
            call_injected(
                &ctx,
                "(injected, elements) => elements.map(e => injected.utils.isElementVisible(e))",
                vec![CallArg::Handle(array.clone())],
                true,
            )
            .await?,
        ) {
            JsValue::Array(items) => items.iter().map(|v| v.as_bool() == Some(true)).collect(),
            _ => Vec::new(),
        }
    } else {
        Vec::new()
    };
    let elements = array_elements(&array).await;
    array.dispose().await;
    let mut result = Vec::new();
    for (index, element) in elements?.into_iter().enumerate() {
        let content = content_frame_of(&element).await;
        element.dispose().await;
        if let Some(content) = content? {
            result.push((
                content,
                parent_visible && visible.get(index).copied().unwrap_or(false),
            ));
        }
    }
    Ok(result)
}

/// The frame hosted by an `<iframe>`/`<frame>` element (`getContentFrame`).
pub(crate) async fn content_frame_of(element: &JSHandle) -> Result<Option<Frame>> {
    let Some(object_id) = element.object_id().cloned() else {
        return Ok(None);
    };
    let mut params = DescribeNodeParams::default();
    params.object_id = Some(object_id);
    let node = element.context().send(params).await?.node;
    let Some(frame_id) = node.frame_id else {
        return Ok(None);
    };
    let Some(page) = element.context().frame().and_then(|f| f.page_inner()) else {
        return Ok(None);
    };
    // A frame attached a moment ago may not be known yet.
    for _ in 0..50 {
        if let Some(frame) = page.frame_by_id(frame_id.as_ref()) {
            return Ok(Some(frame));
        }
        page.sync();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Ok(page.frame_by_id(frame_id.as_ref()))
}

/// Strict mode violations become [`Error::StrictModeViolation`] (the
/// message is kept as Playwright reports it, `Error: strict mode violation: ...`).
pub(crate) fn rewrite_strict(err: Error) -> Error {
    match err {
        Error::Evaluation { api, message, log } if message.contains("strict mode violation: ") => {
            Error::StrictModeViolation {
                api,
                message: message.trim_end().to_string(),
                log,
            }
        }
        other => other,
    }
}

/// Options of [`call_on_selector`].
#[derive(Clone, Copy, Default)]
pub(crate) struct CallOptions {
    pub(crate) main_world: bool,
    pub(crate) call_without_matches: bool,
    pub(crate) by_value: bool,
}

/// Result of [`call_on_selector`].
pub(crate) struct CallResult {
    pub(crate) frame: Frame,
    pub(crate) frame_visible: bool,
    pub(crate) result: Evaluated,
}

/// Mirrors `FrameSelectors._callOnSelectorInternal`: runs `function`
/// (`({ injected, elements, info, frameVisible }, arg) => ...`) on the
/// elements matched by the selector. `None` when nothing matched.
pub(crate) async fn call_on_selector(
    frame: &Frame,
    target: &SelectorTarget,
    function: &str,
    arg: CallArg,
    options: CallOptions,
) -> Result<Option<CallResult>> {
    let resolved = resolve_frames_for_selector(frame, target, false).await?;
    let no_stall = resolved.len() > 1;
    let mut aggregated: Option<CallResult> = None;
    let source = format!(
        r#"(injected, params, scope, arg) => {{
  const parsed = typeof params.sel === 'string' ? injected.parseSelector(params.sel) : params.sel;
  const elements = injected.querySelectorAll(parsed, scope || document);
  injected.checkDeprecatedSelectorUsage(parsed, elements);
  if (params.strict && elements.length > 1)
    throw injected.strictModeViolationError(parsed, elements);
  if (!elements.length && !params.callWithoutMatches)
    return '{NO_RESULT}';
  const func = ({function});
  return func({{ injected, elements, info: {{ parsed, strict: params.strict }}, frameVisible: params.frameVisible }}, arg);
}}"#
    );
    let mut arg = Some(arg);
    for Resolved {
        frame: resolved_frame,
        spec,
        scope,
        frame_visible,
    } in resolved
    {
        let world = if options.main_world {
            World::Main
        } else {
            World::Utility
        };
        let attempt = async {
            let ctx = resolved_frame.context(world).await?;
            let params = json!({
                "strict": target.strict,
                "frameVisible": frame_visible,
                "callWithoutMatches": options.call_without_matches,
            });
            let params_arg = match &spec {
                SelectorSpec::Raw(s) => {
                    let mut p = params;
                    p["sel"] = Value::String(s.clone());
                    CallArg::Json(p)
                }
                SelectorSpec::Parsed(v) => {
                    let mut entries = vec![
                        ("strict".to_string(), JsValue::Bool(target.strict)),
                        ("frameVisible".to_string(), JsValue::Bool(frame_visible)),
                        (
                            "callWithoutMatches".to_string(),
                            JsValue::Bool(options.call_without_matches),
                        ),
                    ];
                    entries.push(("sel".to_string(), v.clone()));
                    CallArg::Js(JsValue::Object(entries))
                }
            };
            let arg = match &arg {
                Some(CallArg::Json(v)) => CallArg::Json(v.clone()),
                Some(CallArg::Js(v)) => CallArg::Js(v.clone()),
                Some(CallArg::Wire(v, h)) => CallArg::Wire(v.clone(), h.clone()),
                Some(CallArg::Handle(h)) => CallArg::Handle(h.clone()),
                Some(CallArg::OptHandle(h)) => CallArg::OptHandle(h.clone()),
                None => CallArg::OptHandle(None),
            };
            call_injected(
                &ctx,
                &source,
                vec![
                    params_arg,
                    CallArg::OptHandle(scope.as_ref().map(|s| s.as_js_handle().clone())),
                    arg,
                ],
                options.by_value,
            )
            .await
            .map_err(rewrite_strict)
        };
        let evaluated = match attempt.await {
            Ok(e) => e,
            Err(e) if no_stall && !e.is_target_closed() => {
                tracing::debug!("skipping frame while resolving selector: {e}");
                continue;
            }
            Err(e) => return Err(e),
        };
        let matched = match &evaluated {
            Evaluated::Value(JsValue::String(s)) => s != NO_RESULT,
            Evaluated::Handle(h) => {
                !matches!(h.inline_value(), Some(JsValue::String(s)) if s == NO_RESULT)
            }
            _ => true,
        };
        if !matched {
            continue;
        }
        if aggregated.is_some() {
            return Err(Error::evaluation(
                "frameLocator() matched elements in multiple frames.",
            ));
        }
        aggregated = Some(CallResult {
            frame: resolved_frame,
            frame_visible,
            result: evaluated,
        });
    }
    let _ = arg.take();
    Ok(aggregated)
}

/// Mirrors `FrameSelectors.query`: the first matching element, adopted to
/// the main world of its frame.
pub(crate) async fn query(frame: &Frame, target: &SelectorTarget) -> Result<Option<ElementHandle>> {
    let Some(resolved) = call_on_selector(
        frame,
        target,
        "({ elements }) => elements[0]",
        CallArg::OptHandle(None),
        CallOptions::default(),
    )
    .await?
    else {
        return Ok(None);
    };
    let handle = handle_of(resolved.result)?;
    let Some(element) = handle.as_element() else {
        handle.dispose().await;
        return Ok(None);
    };
    adopt_to_main(element).await.map(Some)
}

/// Adopt an element handle to the main world of its frame.
pub(crate) async fn adopt_to_main(element: ElementHandle) -> Result<ElementHandle> {
    let frame = element
        .context()
        .frame()
        .ok_or_else(|| Error::frame_detached("Frame was detached"))?;
    let main = frame.main_context().await?;
    if element.context().same_context(&main) {
        return Ok(element);
    }
    let adopted = main.adopt_element(element.as_js_handle()).await;
    element.dispose().await;
    adopted?.as_element().ok_or_else(|| {
        Error::evaluation("Unable to adopt element handle from a different document")
    })
}

/// Mirrors `FrameSelectors.queryAll`.
pub(crate) async fn query_all(
    frame: &Frame,
    selector: &str,
    scope: Option<ElementHandle>,
) -> Result<Vec<ElementHandle>> {
    let target = SelectorTarget {
        selector: selector.to_string(),
        strict: false,
        scope,
    };
    let Some(resolved) = call_on_selector(
        frame,
        &target,
        "({ elements }) => elements",
        CallArg::OptHandle(None),
        CallOptions::default(),
    )
    .await?
    else {
        return Ok(Vec::new());
    };
    let array = handle_of(resolved.result)?;
    let items = array_elements(&array).await;
    array.dispose().await;
    let mut result = Vec::new();
    for item in items? {
        match item.as_element() {
            Some(element) => result.push(adopt_to_main(element).await?),
            None => item.dispose().await,
        }
    }
    Ok(result)
}

/// Mirrors `FrameSelectors.queryCount`.
pub(crate) async fn query_count(frame: &Frame, selector: &str) -> Result<usize> {
    let target = SelectorTarget::new(selector, false);
    let resolved = call_on_selector(
        frame,
        &target,
        "({ elements }) => elements.length",
        CallArg::OptHandle(None),
        CallOptions {
            by_value: true,
            ..Default::default()
        },
    )
    .await?;
    Ok(match resolved {
        Some(r) => value_of(r.result).as_f64().unwrap_or(0.0) as usize,
        None => 0,
    })
}

/// Mirrors `FrameSelectors.queryArrayInMainWorld`.
pub(crate) async fn query_array_in_main_world(
    frame: &Frame,
    selector: &str,
    scope: Option<ElementHandle>,
) -> Result<JSHandle> {
    let target = SelectorTarget {
        selector: selector.to_string(),
        strict: false,
        scope,
    };
    let resolved = call_on_selector(
        frame,
        &target,
        "({ elements }) => elements",
        CallArg::OptHandle(None),
        CallOptions {
            main_world: true,
            ..Default::default()
        },
    )
    .await?;
    match resolved {
        Some(r) => handle_of(r.result),
        None => {
            let ctx = frame.main_context().await?;
            handle_of(call_function(&ctx, "() => []", vec![], false).await?)
        }
    }
}

/// Parse a selector (validation only), like `Selectors.parseSelector`.
pub(crate) fn validate_selector(selector: &str) -> Result<ParsedSelector> {
    parse_selector(selector).map_err(|e| invalid_selector(e.0))
}

/// `asLocator('javascript', selector)` for log lines.
pub(crate) fn describe_selector(selector: &str) -> String {
    as_locator(selector, false)
}

/// Polling delays of `retryWithProgressAndBackoff`.
pub(crate) struct Backoff {
    timeouts: Vec<u64>,
    index: usize,
}

impl Backoff {
    /// Backoff for a progress with the given timeout (ms, `0` = none).
    pub(crate) fn new(timeout: u64) -> Self {
        let mut scale = vec![20u64, 50, 100, 100, 500];
        if timeout > 0 {
            while scale
                .last()
                .is_some_and(|&last| last as f64 > timeout as f64 / 5.0)
            {
                scale.pop();
            }
        }
        let mut timeouts = vec![0];
        timeouts.extend(scale);
        Backoff { timeouts, index: 0 }
    }

    /// Wait before the next attempt.
    pub(crate) async fn wait(&mut self) {
        let timeout = self.timeouts[self.index.min(self.timeouts.len() - 1)];
        self.index += 1;
        if timeout > 0 {
            tokio::time::sleep(Duration::from_millis(timeout)).await;
        }
    }
}
