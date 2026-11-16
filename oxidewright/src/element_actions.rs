//! Element actions with Playwright's actionability checks: a port of
//! `server/dom.ts` (`ElementHandle._retryAction`, `_performPointerAction`,
//! hit target checks, ...) and of the selector based action loop of
//! `server/frames.ts` (`_retryWithProgressIfNotConnected`,
//! `_waitForFunctionOnSelector`, `waitForSelector`).
//!
//! Every DOM operation runs in the frame's utility (isolated) world through
//! the injected script; input goes through the `Input` domain only.

use std::sync::Arc;
use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::dom::{
    GetBoxModelParams, GetContentQuadsParams, Rect as CdpRect, ScrollIntoViewIfNeededParams,
    SetFileInputFilesParams,
};
use chromiumoxide::cdp::browser_protocol::page::EnableParams as PageEnableParams;
use serde_json::{Value, json};

use crate::actions::{BoundingBox, Position};
use crate::error::{Error, Result};
use crate::execution_context::Evaluated;
use crate::frame::Frame;
use crate::frame_selectors::{
    Backoff, CallArg, CallOptions, SelectorTarget, call_function, call_injected, call_on_selector,
    describe_selector, handle_of, is_non_retriable, value_of,
};
use crate::input::{KeyboardModifier, MouseButton};
use crate::js::JsValue;
use crate::js_handle::{ElementHandle, JSHandle};
use crate::page::{PageEvent, PageInner};
use crate::progress::Progress;

/// Result of one attempt of an action (`PerformActionResult`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Outcome {
    Done,
    NotConnected,
    NotVisible,
    NotInViewport,
    OptionsNotFound,
    OptionNotEnabled,
    MissingState(String),
    HitTarget(String),
    /// `selectOption` succeeded with these values.
    Values(Vec<String>),
}

/// Parse the result of injected helpers (`checkElementStates`,
/// `expectHitTarget`, `fill`, `selectOptions`, ...). `undefined` means success.
pub(crate) fn parse_outcome(value: &JsValue) -> Outcome {
    match value {
        JsValue::Undefined | JsValue::Null => Outcome::Done,
        JsValue::String(s) => match s.as_str() {
            "error:notconnected" => Outcome::NotConnected,
            "error:notvisible" => Outcome::NotVisible,
            "error:notinviewport" => Outcome::NotInViewport,
            "error:optionsnotfound" => Outcome::OptionsNotFound,
            "error:optionnotenabled" => Outcome::OptionNotEnabled,
            "done" => Outcome::Done,
            other => Outcome::HitTarget(other.to_string()),
        },
        JsValue::Array(items) => Outcome::Values(
            items
                .iter()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect(),
        ),
        other => {
            if let Some(state) = other.get("missingState").and_then(|v| v.as_str()) {
                Outcome::MissingState(state.to_string())
            } else if let Some(d) = other.get("hitTargetDescription").and_then(|v| v.as_str()) {
                Outcome::HitTarget(d.to_string())
            } else {
                Outcome::Done
            }
        }
    }
}

/// `throwElementIsNotAttached`.
pub(crate) fn not_attached() -> Error {
    Error::evaluation("Element is not attached to the DOM")
}

/// `NonRecoverableDOMError`.
fn non_recoverable(message: &str) -> Error {
    Error::evaluation(message)
}

/// How an action waits for navigations it triggers (`waitAfter`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaitAfter {
    Yes,
    No,
    Disabled,
}

/// What a pointer action does at the action point.
#[derive(Debug, Clone)]
pub(crate) enum Perform {
    Click {
        button: MouseButton,
        click_count: u32,
        delay: Option<f64>,
        steps: Option<u32>,
    },
    Hover,
    Tap,
    MoveAndDown,
    MoveAndUp {
        steps: u32,
    },
}

/// Mirrors `PointerAction` of dom.ts.
#[derive(Debug, Clone)]
pub(crate) struct PointerAction {
    pub(crate) name: &'static str,
    pub(crate) wait_for_enabled: bool,
    pub(crate) wait_after: WaitAfter,
    pub(crate) skip_action_pre_checks: bool,
    pub(crate) perform: Perform,
}

/// Options shared by pointer actions.
#[derive(Debug, Clone, Default)]
pub(crate) struct PointerOptions {
    pub(crate) force: bool,
    pub(crate) position: Option<Position>,
    pub(crate) trial: bool,
    pub(crate) modifiers: Option<Vec<KeyboardModifier>>,
    pub(crate) scroll_none: bool,
}

/// Files for `setInputFiles` (`InputFilesItems`).
#[derive(Debug, Clone, Default)]
pub(crate) struct InputFilesItems {
    /// `{ name, mimeType, buffer (base64), lastModifiedMs }`
    pub(crate) payloads: Option<Vec<Value>>,
    pub(crate) local_paths: Option<Vec<String>>,
    pub(crate) local_directory: Option<String>,
}

/// An action run on a resolved element.
#[derive(Debug, Clone)]
pub(crate) enum Task {
    Pointer(PointerAction, PointerOptions),
    Fill {
        value: String,
        force: bool,
    },
    SelectOption {
        elements: Vec<ElementHandle>,
        values: Vec<Value>,
        force: bool,
    },
    SelectText {
        force: bool,
    },
    SetInputFiles(InputFilesItems),
    Focus,
    Blur,
    Type {
        text: String,
        delay: Option<f64>,
    },
    Press {
        key: String,
        delay: Option<f64>,
        no_wait_after: bool,
    },
    SetChecked {
        state: bool,
        options: PointerOptions,
    },
    ScrollIntoView,
    WaitForState(String),
    AriaSnapshot {
        depth: Option<u32>,
        mode: Option<String>,
    },
}

/// Output of a [`Task`].
#[derive(Debug, Clone)]
pub(crate) enum TaskOutput {
    Done,
    NotConnected,
    Values(Vec<String>),
    Text(String),
}

impl TaskOutput {
    fn from_outcome(outcome: Outcome) -> TaskOutput {
        match outcome {
            Outcome::NotConnected => TaskOutput::NotConnected,
            Outcome::Values(v) => TaskOutput::Values(v),
            _ => TaskOutput::Done,
        }
    }
}

const ROUND: f64 = 100.0;

/// `roundPoint`: truncate to 2 decimals like `(x * 100 | 0) / 100`.
fn round_point(p: Position) -> Position {
    Position {
        x: ((p.x * ROUND) as i32) as f64 / ROUND,
        y: ((p.y * ROUND) as i32) as f64 / ROUND,
    }
}

fn quad_to_rect(quad: &[Position]) -> BoundingBox {
    let min_x = quad.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
    let min_y = quad.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
    let max_x = quad.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
    let max_y = quad.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);
    BoundingBox {
        x: min_x,
        y: min_y,
        width: max_x - min_x,
        height: max_y - min_y,
    }
}

fn quad_middle_point(quad: &[Position]) -> Position {
    let mut result = Position { x: 0.0, y: 0.0 };
    for p in quad {
        result.x += p.x / 4.0;
        result.y += p.y / 4.0;
    }
    result
}

fn compute_quad_area(quad: &[Position]) -> f64 {
    let mut area = 0.0f64;
    for i in 0..quad.len() {
        let p1 = quad[i];
        let p2 = quad[(i + 1) % quad.len()];
        area += (p1.x * p2.y - p2.x * p1.y) / 2.0;
    }
    area.abs()
}

/// What a pointer target resolves to.
enum PointResult {
    Point(Position, BoundingBox),
    Outcome(Outcome),
}

/// `_checkFrameIsHitTarget` result.
enum FrameHit {
    Point(Option<Position>),
    Outcome(Outcome),
}

impl ElementHandle {
    /// The frame owning this element.
    pub(crate) fn owner_frame_sync(&self) -> Result<Frame> {
        self.context()
            .frame()
            .ok_or_else(|| Error::frame_detached("Frame was detached"))
    }

    pub(crate) fn page_or_closed(&self) -> Result<Arc<PageInner>> {
        self.owner_frame_sync()?
            .page_inner()
            .ok_or_else(Error::target_closed)
    }

    /// `evaluateInUtility`: `None` means the element is not connected.
    pub(crate) async fn eval_in_utility(
        &self,
        function: &str,
        arg: Value,
    ) -> Result<Option<JsValue>> {
        let frame = self.owner_frame_sync()?;
        let result = async {
            let ctx = frame.utility_context().await?;
            call_injected(
                &ctx,
                function,
                vec![
                    CallArg::Handle(self.as_js_handle().clone()),
                    CallArg::Json(arg),
                ],
                true,
            )
            .await
        }
        .await;
        match result {
            Ok(evaluated) => Ok(Some(value_of(evaluated))),
            Err(e) if is_non_retriable(&e, Some(&frame)) => Err(e),
            Err(e) => {
                tracing::debug!("evaluateInUtility failed: {e}");
                Ok(None)
            }
        }
    }

    /// `_evaluateHandleInUtility`.
    async fn eval_handle_in_utility(&self, function: &str, arg: Value) -> Result<Option<JSHandle>> {
        let frame = self.owner_frame_sync()?;
        let result = async {
            let ctx = frame.utility_context().await?;
            handle_of(
                call_injected(
                    &ctx,
                    function,
                    vec![
                        CallArg::Handle(self.as_js_handle().clone()),
                        CallArg::Json(arg),
                    ],
                    false,
                )
                .await?,
            )
        }
        .await;
        match result {
            Ok(handle) => Ok(Some(handle)),
            Err(e) if is_non_retriable(&e, Some(&frame)) => Err(e),
            Err(_) => Ok(None),
        }
    }

    /// Position of the root frame of this element's CDP session in the
    /// main frame viewport (`_framePosition`).
    async fn frame_position(&self) -> Result<Option<Position>> {
        let frame = self.owner_frame_sync()?;
        let session = self.context().session().session_id().clone();
        let mut root = frame;
        loop {
            let Some(parent) = root.parent_frame() else {
                break;
            };
            let parent_session = match parent.cdp_frame().await {
                Ok(f) => f.session_id().clone(),
                Err(_) => break,
            };
            if parent_session != session {
                break;
            }
            root = parent;
        }
        if root.parent_frame().is_none() {
            return Ok(Some(Position { x: 0.0, y: 0.0 }));
        }
        let element = match root.frame_element().await {
            Ok(e) => e,
            Err(_) => return Ok(None),
        };
        let result = Box::pin(element.bounding_box_impl()).await;
        element.dispose().await;
        Ok(result?.map(|b| Position { x: b.x, y: b.y }))
    }

    /// `getBoundingBox` of crPage.ts.
    pub(crate) async fn bounding_box_impl(&self) -> Result<Option<BoundingBox>> {
        let Some(object_id) = self.object_id().cloned() else {
            return Ok(None);
        };
        let mut params = GetBoxModelParams::default();
        params.object_id = Some(object_id);
        let model = match self.context().send(params).await {
            Ok(r) => r.model,
            Err(e) if e.is_target_closed() => return Err(e),
            Err(_) => return Ok(None),
        };
        let q = model.border.inner();
        if q.len() < 8 {
            return Ok(None);
        }
        let x = q[0].min(q[2]).min(q[4]).min(q[6]);
        let y = q[1].min(q[3]).min(q[5]).min(q[7]);
        let width = q[0].max(q[2]).max(q[4]).max(q[6]) - x;
        let height = q[1].max(q[3]).max(q[5]).max(q[7]) - y;
        let Some(position) = self.frame_position().await? else {
            return Ok(None);
        };
        Ok(Some(BoundingBox {
            x: x + position.x,
            y: y + position.y,
            width,
            height,
        }))
    }

    /// `getContentQuads` of crPage.ts.
    async fn content_quads(&self) -> Result<Option<Vec<Vec<Position>>>> {
        let Some(object_id) = self.object_id().cloned() else {
            return Ok(None);
        };
        let mut params = GetContentQuadsParams::default();
        params.object_id = Some(object_id);
        let quads = match self.context().send(params).await {
            Ok(r) => r.quads,
            Err(e) if e.is_target_closed() => return Err(e),
            Err(_) => return Ok(None),
        };
        let Some(position) = self.frame_position().await? else {
            return Ok(None);
        };
        Ok(Some(
            quads
                .iter()
                .map(|q| {
                    let q = q.inner();
                    (0..4)
                        .map(|i| Position {
                            x: q.get(2 * i).copied().unwrap_or_default() + position.x,
                            y: q.get(2 * i + 1).copied().unwrap_or_default() + position.y,
                        })
                        .collect()
                })
                .collect(),
        ))
    }

    /// `scrollRectIntoViewIfNeeded` of crPage.ts.
    async fn scroll_rect_into_view_if_needed(&self, rect: Option<BoundingBox>) -> Result<Outcome> {
        let Some(object_id) = self.object_id().cloned() else {
            return Ok(Outcome::NotConnected);
        };
        let mut params = ScrollIntoViewIfNeededParams::default();
        params.object_id = Some(object_id);
        params.rect = rect.map(|r| CdpRect::new(r.x, r.y, r.width, r.height));
        match self.context().session().execute(params).await {
            Ok(_) => Ok(Outcome::Done),
            Err(e) => {
                let message = e.to_string();
                if message.contains("Node does not have a layout object") {
                    Ok(Outcome::NotVisible)
                } else if message.contains("Node is detached from document") {
                    Ok(Outcome::NotConnected)
                } else {
                    Err(Error::from(e))
                }
            }
        }
    }

    /// `_clickablePoint`.
    async fn clickable_point(&self) -> Result<PointResult> {
        let page = self.page_or_closed()?;
        let quads = self.content_quads().await?;
        let metrics = async {
            let ctx = page.main_frame().utility_context().await?;
            Ok::<_, Error>(value_of(
                call_function(
                    &ctx,
                    "() => ({ width: innerWidth, height: innerHeight })",
                    vec![],
                    true,
                )
                .await?,
            ))
        }
        .await?;
        let width = metrics.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let height = metrics
            .get("height")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let Some(quads) = quads.filter(|q| !q.is_empty()) else {
            return Ok(PointResult::Outcome(Outcome::NotVisible));
        };
        let filtered: Vec<Vec<Position>> = quads
            .into_iter()
            .map(|quad| {
                quad.into_iter()
                    .map(|p| Position {
                        x: p.x.max(0.0).min(width),
                        y: p.y.max(0.0).min(height),
                    })
                    .collect::<Vec<_>>()
            })
            .filter(|quad| compute_quad_area(quad) > 0.99)
            .collect();
        let Some(quad) = filtered.first() else {
            return Ok(PointResult::Outcome(Outcome::NotInViewport));
        };
        Ok(PointResult::Point(
            quad_middle_point(quad),
            quad_to_rect(quad),
        ))
    }

    /// `_offsetPoint`.
    async fn offset_point(&self, offset: Position) -> Result<PointResult> {
        let bounding = self.bounding_box_impl().await?;
        let border = self
            .eval_in_utility(
                "(injected, node) => injected.getElementBorderWidth(node)",
                Value::Null,
            )
            .await
            .unwrap_or(None);
        let (Some(bounding), Some(border)) = (bounding, border) else {
            return Ok(PointResult::Outcome(Outcome::NotVisible));
        };
        if border.as_str() == Some("error:notconnected") {
            return Ok(PointResult::Outcome(Outcome::NotConnected));
        }
        let left = border.get("left").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let top = border.get("top").and_then(|v| v.as_f64()).unwrap_or(0.0);
        Ok(PointResult::Point(
            Position {
                x: bounding.x + left + offset.x,
                y: bounding.y + top + offset.y,
            },
            bounding,
        ))
    }

    /// `_retryAction`.
    async fn retry_action(
        &self,
        progress: &Progress,
        action_name: &str,
        trial: bool,
        force: bool,
        kind: &RetryKind<'_>,
    ) -> Result<Outcome> {
        const WAIT_TIME: [u64; 5] = [0, 20, 100, 100, 500];
        let trial_suffix = if trial { " (trial run)" } else { "" };
        let mut retry: usize = 0;
        loop {
            if retry > 0 {
                progress.log(format!("retrying {action_name} action{trial_suffix}"));
                let timeout = WAIT_TIME[(retry - 1).min(WAIT_TIME.len() - 1)];
                if timeout > 0 {
                    progress.log(format!("  waiting {timeout}ms"));
                    tokio::time::sleep(Duration::from_millis(timeout)).await;
                }
            } else {
                progress.log(format!("attempting {action_name} action{trial_suffix}"));
            }
            let result = self.attempt(progress, kind, retry).await?;
            retry += 1;
            match result {
                Outcome::NotVisible => {
                    if force {
                        return Err(non_recoverable("Element is not visible"));
                    }
                    progress.log("  element is not visible");
                }
                Outcome::NotInViewport => {
                    if force {
                        return Err(non_recoverable("Element is outside of the viewport"));
                    }
                    progress.log("  element is outside of the viewport");
                }
                Outcome::OptionsNotFound => progress.log("  did not find some options"),
                Outcome::OptionNotEnabled => progress.log("  option being selected is not enabled"),
                Outcome::HitTarget(description) => {
                    progress.log(format!("  {description} intercepts pointer events"))
                }
                Outcome::MissingState(state) => {
                    if state == "non-inert" {
                        progress.log("  element is inert");
                    } else {
                        progress.log(format!("  element is not {state}"));
                    }
                }
                other => return Ok(other),
            }
        }
    }

    async fn attempt(
        &self,
        progress: &Progress,
        kind: &RetryKind<'_>,
        retry: usize,
    ) -> Result<Outcome> {
        match kind {
            RetryKind::Pointer {
                action,
                options,
                frame_visible,
            } => {
                let scroll_options = [
                    None,
                    Some(json!({ "block": "end", "inline": "end" })),
                    Some(json!({ "block": "center", "inline": "center" })),
                    Some(json!({ "block": "start", "inline": "start" })),
                ];
                let force_scroll = scroll_options[retry % scroll_options.len()].clone();
                self.perform_pointer_action(progress, action, force_scroll, options, *frame_visible)
                    .await
            }
            RetryKind::ScrollIntoView {
                wait_for_visible,
                frame_visible,
            } => {
                progress.log("  waiting for element to be stable");
                let states = if *wait_for_visible {
                    json!(["visible", "stable"])
                } else {
                    json!(["stable"])
                };
                let result = self
                    .eval_in_utility(
                        "async (injected, node, { states, frameVisible }) => await injected.checkElementStates(node, states, frameVisible)",
                        json!({ "states": states, "frameVisible": frame_visible }),
                    )
                    .await?;
                match result {
                    None => Ok(Outcome::NotConnected),
                    Some(v) if !v.is_undefined() => Ok(parse_outcome(&v)),
                    Some(_) => self.scroll_rect_into_view_if_needed(None).await,
                }
            }
            RetryKind::SelectOption {
                elements,
                values,
                force,
                frame_visible,
            } => {
                if !force {
                    progress.log("  waiting for element to be visible and enabled");
                }
                let frame = self.owner_frame_sync()?;
                let result = async {
                    let ctx = frame.utility_context().await?;
                    let mut options: Vec<JsValue> = elements
                        .iter()
                        .map(|e| JsValue::Handle(e.as_js_handle().clone()))
                        .collect();
                    options.extend(values.iter().map(|v| JsValue::from(v.clone())));
                    call_injected(
                        &ctx,
                        r#"async (injected, node, { optionsToSelect, force, frameVisible }) => {
  if (!force) {
    const checkResult = await injected.checkElementStates(node, ['visible', 'enabled'], frameVisible);
    if (checkResult)
      return checkResult;
  }
  return injected.selectOptions(node, optionsToSelect);
}"#,
                        vec![
                            CallArg::Handle(self.as_js_handle().clone()),
                            CallArg::Js(JsValue::Object(vec![
                                ("optionsToSelect".into(), JsValue::Array(options)),
                                ("force".into(), JsValue::Bool(*force)),
                                ("frameVisible".into(), JsValue::Bool(*frame_visible)),
                            ])),
                        ],
                        true,
                    )
                    .await
                }
                .await;
                let value = match result {
                    Ok(e) => value_of(e),
                    Err(e) if is_non_retriable(&e, Some(&frame)) => return Err(e),
                    Err(_) => return Ok(Outcome::NotConnected),
                };
                let outcome = parse_outcome(&value);
                if let Outcome::Values(_) = &outcome {
                    progress.log("  selected specified option(s)");
                }
                Ok(outcome)
            }
            RetryKind::Fill {
                value,
                force,
                frame_visible,
            } => {
                if !force {
                    progress.log("  waiting for element to be visible, enabled and editable");
                }
                let result = self
                    .eval_in_utility(
                        r#"async (injected, node, { value, force, frameVisible }) => {
  if (!force) {
    const checkResult = await injected.checkElementStates(node, ['visible', 'enabled', 'editable'], frameVisible);
    if (checkResult)
      return checkResult;
  }
  return injected.fill(node, value);
}"#,
                        json!({ "value": value, "force": force, "frameVisible": frame_visible }),
                    )
                    .await?;
                match result {
                    None => Ok(Outcome::NotConnected),
                    Some(JsValue::String(s)) if s == "needsinput" => {
                        let page = self.page_or_closed()?;
                        if !value.is_empty() {
                            page.input.insert_text(value).await?;
                        } else {
                            page.input.press("Delete", None).await?;
                        }
                        Ok(Outcome::Done)
                    }
                    Some(v) => Ok(parse_outcome(&v)),
                }
            }
            RetryKind::SelectText { force } => {
                if !force {
                    progress.log("  waiting for element to be visible");
                }
                let result = self
                    .eval_in_utility(
                        r#"async (injected, node, { force, frameVisible }) => {
  if (!force) {
    const checkResult = await injected.checkElementStates(node, ['visible'], frameVisible);
    if (checkResult)
      return checkResult;
  }
  return injected.selectText(node);
}"#,
                        json!({ "force": force, "frameVisible": true }),
                    )
                    .await?;
                Ok(match result {
                    None => Outcome::NotConnected,
                    Some(v) => parse_outcome(&v),
                })
            }
            RetryKind::WaitForState(state) => {
                let result = self
                    .eval_in_utility(
                        "async (injected, node, state) => (await injected.checkElementStates(node, [state], true)) || 'done'",
                        Value::String(state.to_string()),
                    )
                    .await?;
                Ok(match result {
                    None => Outcome::NotConnected,
                    Some(v) => parse_outcome(&v),
                })
            }
        }
    }

    /// `_performPointerAction`.
    async fn perform_pointer_action(
        &self,
        progress: &Progress,
        action: &PointerAction,
        force_scroll: Option<Value>,
        options: &PointerOptions,
        frame_visible: bool,
    ) -> Result<Outcome> {
        let force = options.force;
        let frame = self.owner_frame_sync()?;
        let page = self.page_or_closed()?;

        if frame.parent_frame().is_some() {
            // Best-effort scroll to make sure the containing iframes are visible.
            let _ = self
                .do_scroll_into_view(options, force_scroll.as_ref())
                .await;
        }

        if !force {
            let (states, description) = if action.wait_for_enabled {
                (
                    json!(["visible", "enabled", "stable"]),
                    "visible, enabled and stable",
                )
            } else {
                (json!(["visible", "stable"]), "visible and stable")
            };
            progress.log(format!("  waiting for element to be {description}"));
            let result = self
                .eval_in_utility(
                    "async (injected, node, { elementStates, frameVisible }) => await injected.checkElementStates(node, elementStates, frameVisible)",
                    json!({ "elementStates": states, "frameVisible": frame_visible }),
                )
                .await?;
            match result {
                None => return Ok(Outcome::NotConnected),
                Some(v) if !v.is_undefined() && !v.is_null() => return Ok(parse_outcome(&v)),
                Some(_) => {}
            }
            progress.log(format!("  element is {description}"));
        }

        progress.log("  scrolling into view if needed");
        let scrolled = self
            .do_scroll_into_view(options, force_scroll.as_ref())
            .await?;
        if scrolled != Outcome::Done {
            return Ok(scrolled);
        }
        progress.log("  done scrolling");

        let point = match options.position {
            Some(position) => self.offset_point(position).await?,
            None => self.clickable_point().await?,
        };
        let point = match point {
            PointResult::Point(point, _) => round_point(point),
            PointResult::Outcome(outcome) => return Ok(outcome),
        };

        let mut interceptor: Option<JSHandle> = None;
        if force {
            progress.log("  forcing action");
        } else {
            let hit_point = match self.check_frame_is_hit_target(point).await? {
                FrameHit::Outcome(outcome) => return Ok(outcome),
                FrameHit::Point(p) => p,
            };
            let action_type = match action.name {
                "move and up" => "drag",
                "hover" => "hover",
                "tap" => "tap",
                _ => "mouse",
            };
            let hit_point_json = match hit_point {
                Some(p) => json!({ "x": p.x, "y": p.y }),
                None => Value::Null,
            };
            let handle = self
                .eval_handle_in_utility(
                    "(injected, node, { actionType, hitPoint, trial }) => injected.setupHitTargetInterceptor(node, actionType, hitPoint === null ? undefined : hitPoint, trial)",
                    json!({ "actionType": action_type, "hitPoint": hit_point_json, "trial": options.trial }),
                )
                .await?;
            let Some(handle) = handle else {
                return Ok(Outcome::NotConnected);
            };
            if handle.object_id().is_none() {
                return Ok(match handle.inline_value() {
                    Some(JsValue::String(s)) if s == "error:notconnected" => Outcome::NotConnected,
                    Some(JsValue::String(s)) => Outcome::HitTarget(s.clone()),
                    _ => Outcome::NotConnected,
                });
            }
            interceptor = Some(handle);
        }

        let wait_after = action.wait_after == WaitAfter::Yes;
        let result = wait_for_signals_created_by(&page, progress, wait_after, async {
            let mut restore: Option<Vec<KeyboardModifier>> = None;
            if let Some(modifiers) = &options.modifiers {
                restore = Some(page.input.ensure_modifiers(modifiers).await?);
            }
            progress.log(format!("  performing {} action", action.name));
            match &action.perform {
                Perform::Click {
                    button,
                    click_count,
                    delay,
                    steps,
                } => {
                    page.input
                        .mouse_click_with_steps(
                            point.x,
                            point.y,
                            *button,
                            *click_count,
                            *delay,
                            *steps,
                        )
                        .await?
                }
                Perform::Hover => page.input.mouse_move(point.x, point.y, 1, false).await?,
                Perform::Tap => page.input.tap(point.x, point.y).await?,
                Perform::MoveAndDown => {
                    page.input.mouse_move(point.x, point.y, 1, false).await?;
                    page.input.mouse_down(MouseButton::Left, 1).await?;
                }
                Perform::MoveAndUp { steps } => {
                    page.input
                        .mouse_move(point.x, point.y, *steps, false)
                        .await?;
                    page.input.mouse_up(MouseButton::Left, 1).await?;
                }
            }
            if let Some(restore) = restore {
                page.input.ensure_modifiers(&restore).await?;
            }
            if let Some(handle) = &interceptor {
                if action.wait_after != WaitAfter::No {
                    let stopped = handle.evaluate_inner("h => h.stop()", None, true).await;
                    if let Ok(Evaluated::Value(v)) = stopped {
                        let outcome = parse_outcome(&v);
                        if outcome != Outcome::Done {
                            return Ok(outcome);
                        }
                    }
                }
            }
            progress.log(format!(
                "  {}{} action done",
                if options.trial { "trial " } else { "" },
                action.name
            ));
            progress.log("  waiting for scheduled navigations to finish");
            Ok(Outcome::Done)
        })
        .await;
        if let Some(handle) = interceptor {
            // Do not wait: the renderer may be stuck (e.g. on a dialog).
            tokio::spawn(async move {
                let _ = handle.evaluate_inner("h => h.stop()", None, true).await;
                handle.dispose().await;
            });
        }
        let result = result?;
        if result != Outcome::Done {
            return Ok(result);
        }
        progress.log("  navigations have finished");
        Ok(Outcome::Done)
    }

    async fn do_scroll_into_view(
        &self,
        options: &PointerOptions,
        force_scroll: Option<&Value>,
    ) -> Result<Outcome> {
        if options.scroll_none {
            return Ok(Outcome::Done);
        }
        if let Some(scroll) = force_scroll {
            let result = self
                .eval_in_utility(
                    "(injected, node, options) => { if (node.nodeType === 1) node.scrollIntoView({ ...options, behavior: 'instant' }); return 'done'; }",
                    scroll.clone(),
                )
                .await?;
            return Ok(match result {
                None => Outcome::NotConnected,
                Some(_) => Outcome::Done,
            });
        }
        let rect = options.position.map(|p| BoundingBox {
            x: p.x,
            y: p.y,
            width: 0.0,
            height: 0.0,
        });
        self.scroll_rect_into_view_if_needed(rect).await
    }

    /// `_checkFrameIsHitTarget`.
    async fn check_frame_is_hit_target(&self, point: Position) -> Result<FrameHit> {
        let mut frame = self.owner_frame_sync()?;
        let mut data: Vec<(Option<ElementHandle>, Position)> = Vec::new();
        let mut temporary: Vec<ElementHandle> = Vec::new();
        let result = async {
            while let Some(parent) = frame.parent_frame() {
                let frame_element = frame.frame_element().await?;
                temporary.push(frame_element.clone());
                let bounding = frame_element.bounding_box_impl().await?;
                let style = frame_element
                    .eval_in_utility(
                        "(injected, iframe) => injected.describeIFrameStyle(iframe)",
                        Value::Null,
                    )
                    .await
                    .unwrap_or(None);
                let (Some(bounding), Some(style)) = (bounding, style) else {
                    return Ok(FrameHit::Outcome(Outcome::NotConnected));
                };
                match style.as_str() {
                    Some("error:notconnected") => {
                        return Ok(FrameHit::Outcome(Outcome::NotConnected));
                    }
                    Some("transformed") => return Ok(FrameHit::Point(None)),
                    _ => {}
                }
                let left = style.get("left").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let top = style.get("top").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let point_in_frame = Position {
                    x: point.x - bounding.x - left,
                    y: point.y - bounding.y - top,
                };
                data.push((Some(frame_element), point_in_frame));
                frame = parent;
            }
            data.push((None, point));
            for i in (1..data.len()).rev() {
                let element = data[i - 1].0.clone().expect("frame element");
                let p = data[i].1;
                let result = element
                    .eval_in_utility(
                        "(injected, element, hitPoint) => injected.expectHitTarget(hitPoint, element)",
                        json!({ "x": p.x, "y": p.y }),
                    )
                    .await?;
                let outcome = match result {
                    None => Outcome::NotConnected,
                    Some(v) => parse_outcome(&v),
                };
                if outcome != Outcome::Done {
                    return Ok(FrameHit::Outcome(outcome));
                }
            }
            Ok(FrameHit::Point(Some(data[0].1)))
        }
        .await;
        for element in temporary {
            element.dispose().await;
        }
        result
    }

    /// `_setChecked`.
    async fn set_checked_impl(
        &self,
        progress: &Progress,
        state: bool,
        options: &PointerOptions,
        frame_visible: bool,
    ) -> Result<Outcome> {
        let is_checked = || async {
            let result = self
                .eval_in_utility(
                    "(injected, node) => injected.elementState(node, 'checked')",
                    Value::Null,
                )
                .await?;
            let Some(result) = result else {
                return Err(not_attached());
            };
            if result.get("received").and_then(|v| v.as_str()) == Some("error:notconnected") {
                return Err(not_attached());
            }
            Ok((
                result.get("matches").and_then(|v| v.as_bool()) == Some(true),
                result.get("isRadio").and_then(|v| v.as_bool()) == Some(true),
            ))
        };
        let (matches, is_radio) = is_checked().await?;
        if matches == state {
            return Ok(Outcome::Done);
        }
        if !state && is_radio {
            return Err(non_recoverable(
                "Cannot uncheck radio button. Radio buttons can only be unchecked by selecting another radio button in the same group.",
            ));
        }
        let action = PointerAction {
            name: "click",
            wait_for_enabled: true,
            wait_after: WaitAfter::Disabled,
            skip_action_pre_checks: false,
            perform: Perform::Click {
                button: MouseButton::Left,
                click_count: 1,
                delay: None,
                steps: None,
            },
        };
        let result = self
            .retry_action(
                progress,
                action.name,
                options.trial,
                options.force,
                &RetryKind::Pointer {
                    action: &action,
                    options,
                    frame_visible,
                },
            )
            .await?;
        if result != Outcome::Done {
            return Ok(result);
        }
        if options.trial {
            return Ok(Outcome::Done);
        }
        let (matches, _) = is_checked().await?;
        if matches != state {
            return Err(non_recoverable(
                "Clicking the checkbox did not change its state",
            ));
        }
        Ok(Outcome::Done)
    }

    /// `_setInputFiles`.
    async fn set_input_files_impl(&self, items: &InputFilesItems) -> Result<Outcome> {
        let multiple = items.payloads.as_ref().is_some_and(|p| p.len() > 1)
            || items.local_paths.as_ref().is_some_and(|p| p.len() > 1);
        let directory_upload = items.local_directory.is_some();
        let retargeted = self
            .eval_handle_in_utility(
                r#"(injected, node, { multiple, directoryUpload }) => {
  const element = injected.retarget(node, 'follow-label');
  if (!element)
    return;
  if (element.tagName !== 'INPUT')
    throw injected.createStacklessError('Node is not an HTMLInputElement');
  const inputElement = element;
  if (multiple && !inputElement.multiple && !inputElement.webkitdirectory)
    throw injected.createStacklessError('Non-multiple file input can only accept single file');
  if (directoryUpload && !inputElement.webkitdirectory)
    throw injected.createStacklessError('File input does not support directories, pass individual files instead');
  if (!directoryUpload && inputElement.webkitdirectory)
    throw injected.createStacklessError('[webkitdirectory] input requires passing a path to a directory');
  return inputElement;
}"#,
                json!({ "multiple": multiple, "directoryUpload": directory_upload }),
            )
            .await?;
        let Some(retargeted) = retargeted else {
            return Ok(Outcome::NotConnected);
        };
        let Some(element) = retargeted.as_element() else {
            return Ok(Outcome::NotConnected);
        };
        let result = async {
            if items.local_paths.is_some() || items.local_directory.is_some() {
                let paths: Vec<String> = match &items.local_directory {
                    Some(dir) => vec![dir.clone()],
                    None => items.local_paths.clone().unwrap_or_default(),
                };
                for path in &paths {
                    if !std::path::Path::new(path).exists() {
                        return Err(Error::invalid_argument(format!(
                            "ENOENT: no such file or directory, access '{path}'"
                        )));
                    }
                }
                let input_event = if directory_upload {
                    element
                        .evaluate_inner(
                            "node => ({ promise: new Promise(fulfill => node.addEventListener('input', () => fulfill(), { once: true })) })",
                            None,
                            false,
                        )
                        .await
                        .ok()
                        .and_then(|e| match e {
                            Evaluated::Handle(h) => Some(h),
                            Evaluated::Value(_) => None,
                        })
                } else {
                    None
                };
                let mut params = SetFileInputFilesParams::new(paths);
                params.object_id = element.object_id().cloned();
                let sent = element.context().send(params).await;
                if let Some(event) = input_event {
                    if sent.is_ok() {
                        let _ = event.evaluate_inner("event => event.promise", None, true).await;
                    }
                    event.dispose().await;
                }
                sent?;
            } else {
                let payloads = items.payloads.clone().unwrap_or_default();
                let result = element
                    .eval_in_utility(
                        "(injected, node, files) => injected.setInputFiles(node, files)",
                        Value::Array(payloads),
                    )
                    .await?;
                if let Some(JsValue::String(s)) = result {
                    if s == "error:notconnected" {
                        return Ok(Outcome::NotConnected);
                    }
                    return Err(Error::evaluation(s));
                }
            }
            Ok(Outcome::Done)
        }
        .await;
        element.dispose().await;
        result
    }

    /// `_focus`.
    async fn focus_impl(&self, reset_selection_if_not_focused: bool) -> Result<Outcome> {
        let result = self
            .eval_in_utility(
                "(injected, node, resetSelectionIfNotFocused) => injected.focusNode(node, resetSelectionIfNotFocused)",
                Value::Bool(reset_selection_if_not_focused),
            )
            .await?;
        Ok(match result {
            None => Outcome::NotConnected,
            Some(v) => parse_outcome(&v),
        })
    }

    /// Run `task` on this element (the `handle._xxx` call of frames.ts).
    pub(crate) async fn run_task(
        &self,
        progress: &Progress,
        task: &Task,
        frame_visible: bool,
    ) -> Result<TaskOutput> {
        let outcome = match task {
            Task::Pointer(action, options) => {
                self.retry_action(
                    progress,
                    action.name,
                    options.trial,
                    options.force,
                    &RetryKind::Pointer {
                        action,
                        options,
                        frame_visible,
                    },
                )
                .await?
            }
            Task::Fill { value, force } => {
                progress.log(format!("  fill(\"{value}\")"));
                self.retry_action(
                    progress,
                    "fill",
                    false,
                    *force,
                    &RetryKind::Fill {
                        value,
                        force: *force,
                        frame_visible,
                    },
                )
                .await?
            }
            Task::SelectOption {
                elements,
                values,
                force,
            } => {
                self.retry_action(
                    progress,
                    "select option",
                    false,
                    *force,
                    &RetryKind::SelectOption {
                        elements,
                        values,
                        force: *force,
                        frame_visible,
                    },
                )
                .await?
            }
            Task::SelectText { force } => {
                self.retry_action(
                    progress,
                    "selectText",
                    false,
                    *force,
                    &RetryKind::SelectText { force: *force },
                )
                .await?
            }
            Task::SetInputFiles(items) => self.set_input_files_impl(items).await?,
            Task::Focus => self.focus_impl(false).await?,
            Task::Blur => {
                let result = self
                    .eval_in_utility("(injected, node) => injected.blurNode(node)", Value::Null)
                    .await?;
                match result {
                    None => Outcome::NotConnected,
                    Some(v) => parse_outcome(&v),
                }
            }
            Task::Type { text, delay } => {
                progress.log(format!("elementHandle.type(\"{text}\")"));
                let result = self.focus_impl(true).await?;
                if result != Outcome::Done {
                    result
                } else {
                    self.page_or_closed()?.input.type_text(text, *delay).await?;
                    Outcome::Done
                }
            }
            Task::Press {
                key,
                delay,
                no_wait_after,
            } => {
                progress.log(format!("elementHandle.press(\"{key}\")"));
                let page = self.page_or_closed()?;
                wait_for_signals_created_by(&page, progress, !no_wait_after, async {
                    let result = self.focus_impl(true).await?;
                    if result != Outcome::Done {
                        return Ok(result);
                    }
                    page.input.press(key, *delay).await?;
                    Ok(Outcome::Done)
                })
                .await?
            }
            Task::SetChecked { state, options } => {
                self.set_checked_impl(progress, *state, options, frame_visible)
                    .await?
            }
            Task::ScrollIntoView => {
                self.retry_action(
                    progress,
                    "scroll into view",
                    false,
                    false,
                    &RetryKind::ScrollIntoView {
                        wait_for_visible: false,
                        frame_visible: true,
                    },
                )
                .await?
            }
            Task::WaitForState(state) => {
                self.retry_action(
                    progress,
                    &format!("wait for {state}"),
                    false,
                    false,
                    &RetryKind::WaitForState(state),
                )
                .await?
            }
            Task::AriaSnapshot { depth, mode } => {
                let mut options =
                    json!({ "mode": mode.clone().unwrap_or_else(|| "default".into()) });
                if let Some(depth) = depth {
                    options["depth"] = json!(depth);
                }
                let result = self
                    .eval_in_utility(
                        "(injected, element, options) => injected.ariaSnapshot(element, options)",
                        options,
                    )
                    .await?;
                return Ok(match result {
                    None => TaskOutput::NotConnected,
                    Some(v) => TaskOutput::Text(v.as_str().unwrap_or_default().to_string()),
                });
            }
        };
        Ok(TaskOutput::from_outcome(outcome))
    }

    /// Run `task` directly on this handle (ElementHandle API): a detached
    /// element fails with "Element is not attached to the DOM".
    pub(crate) async fn run_handle_task(
        &self,
        progress: &Progress,
        task: &Task,
    ) -> Result<TaskOutput> {
        match self.run_task(progress, task, true).await? {
            TaskOutput::NotConnected => Err(not_attached()),
            other => Ok(other),
        }
    }
}

/// What `_retryAction` retries.
enum RetryKind<'a> {
    Pointer {
        action: &'a PointerAction,
        options: &'a PointerOptions,
        frame_visible: bool,
    },
    ScrollIntoView {
        wait_for_visible: bool,
        frame_visible: bool,
    },
    SelectOption {
        elements: &'a [ElementHandle],
        values: &'a [Value],
        force: bool,
        frame_visible: bool,
    },
    Fill {
        value: &'a str,
        force: bool,
        frame_visible: bool,
    },
    SelectText {
        force: bool,
    },
    WaitForState(&'a str),
}

/// `FrameManager.waitForSignalsCreatedBy`: after the action, wait for the
/// top-level navigations it requested to commit.
pub(crate) async fn wait_for_signals_created_by<T>(
    page: &Arc<PageInner>,
    progress: &Progress,
    wait_after: bool,
    action: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    if !wait_after {
        return action.await;
    }
    let mut rx = page.subscribe();
    let result = action.await?;
    // `inputActionEpilogue`: a round trip that flushes pending events.
    let _ = page.cdp.execute(PageEnableParams::default()).await;
    page.sync();
    let main_id = page.main_frame().id().to_string();
    let mut pending: usize = 0;
    loop {
        let event = if pending == 0 {
            match rx.try_recv() {
                Ok(event) => event,
                Err(_) => break,
            }
        } else {
            page.next_event(&mut rx).await?
        };
        match event {
            PageEvent::NavigationRequested(id) if id == main_id => pending += 1,
            PageEvent::Navigated(nav) if nav.frame_id == main_id && pending > 0 => {
                pending -= 1;
                if nav.error.is_none() {
                    progress.log(format!("  navigated to \"{}\"", nav.url));
                }
            }
            PageEvent::FrameDetached(id) if id == main_id => pending = 0,
            PageEvent::Closed => break,
            _ => {}
        }
    }
    Ok(result)
}

/// Mirrors `Frame._retryWithProgressIfNotConnected`.
pub(crate) async fn retry_with_progress_if_not_connected(
    frame: &Frame,
    progress: &Progress,
    target: &SelectorTarget,
    force: bool,
    wait_for_frame_visible: bool,
    task: &Task,
) -> Result<TaskOutput> {
    progress.log(format!(
        "waiting for {}",
        describe_selector(&target.selector)
    ));
    let wait_for_frame_visible = wait_for_frame_visible && !force;
    let mut backoff = Backoff::new(progress.timeout());
    loop {
        backoff.wait().await;
        let attempt = async {
            let resolved = call_on_selector(
                frame,
                target,
                r#"({ injected, elements }) => {
  const element = elements[0];
  let log = '';
  if (elements.length > 1)
    log = `  locator resolved to ${elements.length} elements. Proceeding with the first one: ${injected.previewNode(elements[0])}`;
  else if (element)
    log = `  locator resolved to ${injected.previewNode(element)}`;
  return { log, success: !!element, element };
}"#,
                CallArg::OptHandle(None),
                CallOptions::default(),
            )
            .await?;
            let Some(resolved) = resolved else {
                return Ok(None);
            };
            let result = handle_of(resolved.result)?;
            let info = result
                .evaluate_inner("r => ({ log: r.log, success: r.success })", None, true)
                .await;
            let info = match info {
                Ok(e) => value_of(e),
                Err(e) => {
                    result.dispose().await;
                    return Err(e);
                }
            };
            if let Some(log) = info.get("log").and_then(|v| v.as_str()) {
                if !log.is_empty() {
                    progress.log(log);
                }
            }
            if info.get("success").and_then(|v| v.as_bool()) != Some(true) {
                result.dispose().await;
                return Ok(None);
            }
            if wait_for_frame_visible && !resolved.frame_visible {
                progress.log("  element is inside a hidden frame, retrying");
                result.dispose().await;
                return Ok(None);
            }
            let element = result.evaluate_inner("r => r.element", None, false).await;
            result.dispose().await;
            let element = handle_of(element?)?;
            let Some(element) = element.as_element() else {
                element.dispose().await;
                return Ok(None);
            };
            let output = element.run_task(progress, task, resolved.frame_visible).await;
            element.dispose().await;
            match output? {
                TaskOutput::NotConnected => {
                    progress.log("element was detached from the DOM, retrying");
                    Ok(None)
                }
                other => Ok(Some(other)),
            }
        }
        .await;
        match attempt {
            Ok(Some(output)) => return Ok(output),
            Ok(None) => continue,
            Err(e) if is_non_retriable(&e, Some(frame)) => return Err(e),
            Err(e) => {
                tracing::debug!("retrying after error: {e}");
                continue;
            }
        }
    }
}

/// Mirrors `Frame._waitForFunctionOnSelector`: `callback` is
/// `(injected, element, data) => value`; resolves with the first truthy value.
pub(crate) async fn wait_for_function_on_selector(
    frame: &Frame,
    progress: &Progress,
    target: &SelectorTarget,
    callback: &str,
    data: CallArg,
    main_world: bool,
) -> Result<JsValue> {
    progress.log(format!(
        "waiting for {}",
        describe_selector(&target.selector)
    ));
    let function = format!(
        r#"({{ injected, elements }}, taskData) => {{
  const callback = ({callback});
  const element = elements[0];
  const value = callback(injected, element, taskData);
  if (!value)
    return {{ success: false }};
  const log = `  locator resolved to ${{injected.previewNode(element)}}`;
  return {{ log, success: true, value }};
}}"#
    );
    let mut backoff = Backoff::new(progress.timeout());
    loop {
        backoff.wait().await;
        if let Some(scope) = &target.scope {
            if scope.context().is_destroyed() {
                return Err(Error::evaluation(crate::error::CONTEXT_DESTROYED_MESSAGE));
            }
        }
        let data = clone_arg(&data);
        let attempt = call_on_selector(
            frame,
            target,
            &function,
            data,
            CallOptions {
                main_world,
                by_value: true,
                ..Default::default()
            },
        )
        .await;
        let resolved = match attempt {
            Ok(r) => r,
            Err(e) if is_non_retriable(&e, Some(frame)) => return Err(e),
            Err(_) => continue,
        };
        let Some(resolved) = resolved else {
            continue;
        };
        let value = value_of(resolved.result);
        if let Some(log) = value.get("log").and_then(|v| v.as_str()) {
            progress.log(log);
        }
        if value.get("success").and_then(|v| v.as_bool()) != Some(true) {
            continue;
        }
        return Ok(value.get("value").cloned().unwrap_or(JsValue::Undefined));
    }
}

pub(crate) fn clone_arg(arg: &CallArg) -> CallArg {
    match arg {
        CallArg::Handle(h) => CallArg::Handle(h.clone()),
        CallArg::OptHandle(h) => CallArg::OptHandle(h.clone()),
        CallArg::Json(v) => CallArg::Json(v.clone()),
        CallArg::Js(v) => CallArg::Js(v.clone()),
        CallArg::Wire(v, h) => CallArg::Wire(v.clone(), h.clone()),
    }
}

/// `state` option of `waitForSelector` / `locator.waitFor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WaitForSelectorState {
    /// Present in the DOM.
    Attached,
    /// Not present in the DOM.
    Detached,
    /// Present and visible (default).
    #[default]
    Visible,
    /// Absent or not visible.
    Hidden,
}

impl WaitForSelectorState {
    /// The Playwright name of the state.
    pub fn as_str(&self) -> &'static str {
        match self {
            WaitForSelectorState::Attached => "attached",
            WaitForSelectorState::Detached => "detached",
            WaitForSelectorState::Visible => "visible",
            WaitForSelectorState::Hidden => "hidden",
        }
    }
}

impl std::str::FromStr for WaitForSelectorState {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "attached" => Ok(Self::Attached),
            "detached" => Ok(Self::Detached),
            "visible" => Ok(Self::Visible),
            "hidden" => Ok(Self::Hidden),
            _ => Err(Error::invalid_argument(
                "state: expected one of (attached|detached|visible|hidden)",
            )),
        }
    }
}

/// Mirrors `Frame.waitForSelector`.
pub(crate) async fn wait_for_selector(
    frame: &Frame,
    progress: &Progress,
    target: &SelectorTarget,
    log: bool,
    state: WaitForSelectorState,
    omit_return_value: bool,
) -> Result<Option<ElementHandle>> {
    if log {
        let suffix = if state == WaitForSelectorState::Attached {
            String::new()
        } else {
            format!(" to be {}", state.as_str())
        };
        progress.log(format!(
            "waiting for {}{suffix}",
            describe_selector(&target.selector)
        ));
    }
    let mut backoff = Backoff::new(progress.timeout());
    loop {
        backoff.wait().await;
        let attempt = async {
            if let Some(scope) = &target.scope {
                let connected = scope
                    .eval_in_utility("(injected, node) => node.isConnected", Value::Null)
                    .await?;
                if connected.and_then(|v| v.as_bool()) != Some(true) {
                    return Err(non_recoverable("Element is not attached to the DOM (scope)"));
                }
            }
            let resolved = call_on_selector(
                frame,
                target,
                r#"({ injected, elements, frameVisible }) => {
  const element = elements[0];
  const visible = element ? injected.elementState(element, 'visible', frameVisible).matches : false;
  let log = '';
  if (elements.length > 1)
    log = `  locator resolved to ${elements.length} elements. Proceeding with the first one: ${injected.previewNode(elements[0])}`;
  else if (element)
    log = `  locator resolved to ${visible ? 'visible' : 'hidden'} ${injected.previewNode(element)}`;
  return { log, element, visible, attached: !!element };
}"#,
                CallArg::OptHandle(None),
                CallOptions::default(),
            )
            .await?;
            let Some(resolved) = resolved else {
                return Ok(match state {
                    WaitForSelectorState::Hidden | WaitForSelectorState::Detached => {
                        Some(None)
                    }
                    _ => None,
                });
            };
            let result = handle_of(resolved.result)?;
            let info = result
                .evaluate_inner(
                    "r => ({ log: r.log, visible: r.visible, attached: r.attached })",
                    None,
                    true,
                )
                .await;
            let info = match info {
                Ok(e) => value_of(e),
                Err(e) => {
                    result.dispose().await;
                    return Err(e);
                }
            };
            if let Some(log) = info.get("log").and_then(|v| v.as_str()) {
                if !log.is_empty() {
                    progress.log(log);
                }
            }
            let visible = info.get("visible").and_then(|v| v.as_bool()) == Some(true);
            let attached = info.get("attached").and_then(|v| v.as_bool()) == Some(true);
            let success = match state {
                WaitForSelectorState::Attached => attached,
                WaitForSelectorState::Detached => !attached,
                WaitForSelectorState::Visible => visible,
                WaitForSelectorState::Hidden => !visible,
            };
            if !success {
                result.dispose().await;
                return Ok(None);
            }
            if omit_return_value
                || matches!(
                    state,
                    WaitForSelectorState::Hidden | WaitForSelectorState::Detached
                )
            {
                result.dispose().await;
                return Ok(Some(None));
            }
            let element = result.evaluate_inner("r => r.element", None, false).await;
            result.dispose().await;
            let element = handle_of(element?)?;
            let Some(element) = element.as_element() else {
                return Ok(Some(None));
            };
            match crate::frame_selectors::adopt_to_main(element).await {
                Ok(adopted) => Ok(Some(Some(adopted))),
                Err(_) => Ok(None),
            }
        }
        .await;
        match attempt {
            Ok(Some(result)) => return Ok(result),
            Ok(None) => continue,
            Err(e) if is_non_retriable(&e, Some(frame)) => return Err(e),
            Err(_) => continue,
        }
    }
}

/// Mirrors `Frame._elementState` for enabled/disabled/editable/checked.
pub(crate) async fn element_state(
    frame: &Frame,
    progress: &Progress,
    target: &SelectorTarget,
    state: &str,
) -> Result<bool> {
    let value = wait_for_function_on_selector(
        frame,
        progress,
        target,
        "(injected, element, data) => ({ result: injected.elementState(element, data.state) })",
        CallArg::Json(json!({ "state": state })),
        false,
    )
    .await?;
    let result = value.get("result").cloned().unwrap_or(JsValue::Undefined);
    if result.get("received").and_then(|v| v.as_str()) == Some("error:notconnected") {
        return Err(not_attached());
    }
    Ok(result.get("matches").and_then(|v| v.as_bool()) == Some(true))
}

/// Mirrors `Frame.isVisibleInternal`.
pub(crate) async fn is_visible_internal(frame: &Frame, target: &SelectorTarget) -> Result<bool> {
    let result = call_on_selector(
        frame,
        target,
        "({ injected, elements, frameVisible }) => injected.elementState(elements[0], 'visible', frameVisible).matches",
        CallArg::OptHandle(None),
        CallOptions {
            by_value: true,
            ..Default::default()
        },
    )
    .await;
    match result {
        Ok(Some(r)) => Ok(value_of(r.result).as_bool() == Some(true)),
        Ok(None) => Ok(false),
        Err(e) if is_non_retriable(&e, Some(frame)) => Err(e),
        Err(_) => Ok(false),
    }
}

/// Re-exported for `frame_selectors` users.
#[allow(dead_code)]
pub(crate) async fn evaluate_on_element(
    element: &ElementHandle,
    expression: &str,
    arg: Option<(Value, Vec<JSHandle>)>,
    by_value: bool,
) -> Result<Evaluated> {
    element.evaluate_inner(expression, arg, by_value).await
}

#[allow(dead_code)]
fn _assert_send() {
    fn is_send<T: Send>(_: T) {}
    let _ = is_send::<Option<SelectorTarget>>;
    let _ = call_function;
}
