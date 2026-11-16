//! The polling engine of the assertions: a port of Playwright's server-side
//! `Frame.expect` / `Frame._expectInternal` (`server/frames.ts`) and of the
//! client's `Frame._expect` (`client/frame.ts`).
//!
//! Every check calls the injected script's `expect()` (bundled from
//! Patchright 1.64.3, which supports every expression Playwright 1.65 uses).
//! Like Playwright, the first check runs without a deadline (so
//! `to_be_visible().timeout(1)` passes on a visible element), then the check
//! is retried with `retryWithProgressAndBackoff`'s delays
//! (`[0, 20, 50, 100, 100, 500]` ms, capped to a fifth of the timeout) until
//! the deadline.

use std::time::{Duration, Instant};

use crate::error::{Error, TARGET_CLOSED_MESSAGE, compress_call_log};
use crate::frame::Frame;
use crate::frame_selectors::{
    Backoff, CallArg, CallOptions, SelectorTarget, call_injected, call_on_selector,
    describe_selector, is_non_retriable, value_of,
};
use crate::js::JsValue;

/// `FrameExpectParams`.
#[derive(Debug, Clone, Default)]
pub(crate) struct ExpectParams {
    pub(crate) expression: &'static str,
    pub(crate) selector: Option<String>,
    pub(crate) expression_arg: Option<String>,
    pub(crate) expected_text: Option<Vec<JsValue>>,
    pub(crate) expected_number: Option<f64>,
    pub(crate) expected_value: Option<JsValue>,
    pub(crate) use_inner_text: Option<bool>,
    pub(crate) is_not: bool,
    pub(crate) pseudo: Option<String>,
}

impl ExpectParams {
    pub(crate) fn new(expression: &'static str) -> Self {
        ExpectParams {
            expression,
            ..Default::default()
        }
    }

    fn is_array(&self) -> bool {
        self.expression == "to.have.count" || self.expression.ends_with(".array")
    }

    /// The `options` object given to `injected.expect()`.
    fn to_js(&self) -> JsValue {
        let mut entries = vec![
            (
                "expression".to_string(),
                JsValue::String(self.expression.to_string()),
            ),
            ("isNot".to_string(), JsValue::Bool(self.is_not)),
        ];
        if let Some(arg) = &self.expression_arg {
            entries.push(("expressionArg".to_string(), JsValue::String(arg.clone())));
        }
        if let Some(text) = &self.expected_text {
            entries.push(("expectedText".to_string(), JsValue::Array(text.clone())));
        }
        if let Some(n) = self.expected_number {
            entries.push(("expectedNumber".to_string(), JsValue::Number(n)));
        }
        if let Some(v) = &self.expected_value {
            entries.push(("expectedValue".to_string(), v.clone()));
        }
        if let Some(u) = self.use_inner_text {
            entries.push(("useInnerText".to_string(), JsValue::Bool(u)));
        }
        if let Some(p) = &self.pseudo {
            entries.push(("pseudo".to_string(), JsValue::String(p.clone())));
        }
        JsValue::Object(entries)
    }
}

/// `ExpectResult` of the client.
#[derive(Debug, Clone, Default)]
pub(crate) struct ExpectOutcome {
    pub(crate) matches: bool,
    /// `received.value`; `None` when the server sent no received value.
    pub(crate) received: Option<JsValue>,
    pub(crate) aria_snapshot: Option<String>,
    pub(crate) log: Vec<String>,
    pub(crate) timed_out: bool,
    /// `Error: ...` lines of the message.
    pub(crate) error_message: Option<String>,
    /// The selector could not be parsed (Playwright then prints it as is).
    pub(crate) invalid_selector: bool,
}

/// The message of a JavaScript error thrown by the injected script, the
/// way Playwright reports it: without the stack and the `Error: ` /
/// `InvalidSelectorError: ` prefix. Returns whether it is a selector error.
pub(crate) fn clean_js_error(message: &str) -> (String, bool) {
    let mut lines = Vec::new();
    for line in message.lines() {
        if line.trim_start().starts_with("at ") && !lines.is_empty() {
            break;
        }
        lines.push(line);
    }
    let text = lines.join("\n");
    if let Some(rest) = text.strip_prefix("InvalidSelectorError: ") {
        return (rest.to_string(), true);
    }
    (
        text.strip_prefix("Error: ").unwrap_or(&text).to_string(),
        false,
    )
}

/// `{ value, ariaSnapshot }` of the server.
#[derive(Debug, Clone, Default)]
struct Received {
    value: Option<JsValue>,
    aria_snapshot: Option<String>,
}

#[derive(Default)]
struct Intermediate {
    set: bool,
    received: Option<Received>,
    error_message: Option<String>,
}

const EXPECT_JS: &str = r#"async ({ injected, elements, frameVisible }, options) => {
  const isArray = options.expression === 'to.have.count' || options.expression.endsWith('.array');
  const log = isArray
    ? `  locator resolved to ${elements.length} element${elements.length === 1 ? '' : 's'}`
    : `  locator resolved to ${injected.previewNode(elements[0])}`;
  return { log, ...await injected.expect(elements[0], options, elements, frameVisible) };
}"#;

const ARIA_SNAPSHOT_FOR_FAILURE_JS: &str = r#"(injected) => {
  if (injected.document.body)
    return injected.ariaSnapshotForExpectFailure(injected.document.body, { mode: 'default' });
}"#;

/// `renderUnexpectedValue` / `String(value)` for log lines.
pub(crate) fn js_string(v: Option<&JsValue>) -> String {
    match v {
        None | Some(JsValue::Undefined) => "undefined".into(),
        Some(JsValue::Null) => "null".into(),
        Some(JsValue::Bool(b)) => b.to_string(),
        Some(JsValue::Number(n)) => crate::js::value::format_js_number(*n),
        Some(JsValue::String(s)) | Some(JsValue::Date(s)) | Some(JsValue::Url(s)) => s.clone(),
        Some(JsValue::BigInt(b)) => b.clone(),
        Some(JsValue::RegExp { source, flags }) => format!("/{source}/{flags}"),
        Some(JsValue::Array(items)) => items
            .iter()
            .map(|i| match i {
                JsValue::Undefined | JsValue::Null => String::new(),
                other => js_string(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(_) => "[object Object]".into(),
    }
}

fn render_unexpected_value(expression: &str, received: Option<&JsValue>) -> String {
    if expression == "to.match.aria" {
        return match received {
            Some(v) => js_string(v.get("raw")),
            None => "undefined".to_string(),
        };
    }
    js_string(received)
}

/// One `_expectInternal` round: `(matches, received value)`.
async fn expect_internal(
    frame: &Frame,
    params: &ExpectParams,
    log: &mut Vec<String>,
    last: &mut Intermediate,
) -> crate::Result<bool> {
    let main_world = params.expression == "to.have.property";
    let is_array = params.is_array();
    let effective = params.selector.clone().unwrap_or_else(|| {
        if params.expression == "to.match.aria" {
            "body".to_string()
        } else {
            ":root".to_string()
        }
    });
    let target = SelectorTarget::new(effective, !is_array);
    let resolved = call_on_selector(
        frame,
        &target,
        EXPECT_JS,
        CallArg::Js(params.to_js()),
        CallOptions {
            main_world,
            by_value: true,
            ..Default::default()
        },
    )
    .await?;
    let mut missing = false;
    let mut received: Option<Received> = None;
    let matches = match resolved {
        Some(r) => {
            let v = value_of(r.result);
            if let Some(l) = v.get("log").and_then(|l| l.as_str()) {
                if !l.is_empty() {
                    log.push(l.to_string());
                }
            }
            if let Some(r) = v.get("received") {
                received = Some(Received {
                    value: r.get("value").cloned(),
                    aria_snapshot: r
                        .get("ariaSnapshot")
                        .and_then(|a| a.as_str())
                        .map(str::to_string),
                });
            }
            v.get("matches").and_then(|m| m.as_bool()).unwrap_or(false)
        }
        None => {
            let e = params.expression;
            let matches = if e == "to.have.count" {
                log.push("  locator resolved to 0 elements".into());
                received = Some(Received {
                    value: Some(JsValue::Number(0.0)),
                    aria_snapshot: None,
                });
                params.expected_number == Some(0.0)
            } else if e.ends_with(".array") {
                log.push("  locator resolved to 0 elements".into());
                received = Some(Received {
                    value: Some(JsValue::Array(vec![])),
                    aria_snapshot: None,
                });
                params.expected_text.as_ref().is_none_or(|t| t.is_empty())
            } else if !params.is_not && (e == "to.be.hidden" || e == "to.be.detached") {
                true
            } else if params.is_not
                && (e == "to.be.visible" || e == "to.be.attached" || e == "to.be.in.viewport")
            {
                false
            } else {
                missing = true;
                params.is_not
            };
            if matches == params.is_not && !is_array {
                if let Some(snapshot) = aria_snapshot_for_failure(frame, main_world).await {
                    received.get_or_insert_with(Received::default).aria_snapshot = Some(snapshot);
                }
            }
            matches
        }
    };
    if matches == params.is_not {
        last.error_message = missing.then(|| "element(s) not found".to_string());
        last.received = received.clone();
        last.set = true;
        let value = received.as_ref().and_then(|r| r.value.as_ref());
        if !missing && !matches!(value, Some(JsValue::Array(_))) {
            log.push(format!(
                "  unexpected value \"{}\"",
                render_unexpected_value(params.expression, value)
            ));
        }
    }
    Ok(matches)
}

async fn aria_snapshot_for_failure(frame: &Frame, main_world: bool) -> Option<String> {
    let ctx = if main_world {
        frame.main_context().await.ok()?
    } else {
        frame.utility_context().await.ok()?
    };
    let value = call_injected(&ctx, ARIA_SNAPSHOT_FOR_FAILURE_JS, vec![], true)
        .await
        .ok()?;
    value_of(value).as_str().map(str::to_string)
}

/// Port of `Frame.expect` (server) and `Frame._expect` (client).
pub(crate) async fn frame_expect(
    frame: &Frame,
    params: ExpectParams,
    timeout: u64,
    title: &str,
) -> ExpectOutcome {
    let mut log = Vec::new();
    let subtitle = params
        .selector
        .as_deref()
        .map(|s| format!(" {}", describe_selector(s)))
        .unwrap_or_default();
    let with_timeout = if timeout > 0 {
        format!(" with timeout {timeout}ms")
    } else {
        String::new()
    };
    log.push(format!("{title}{subtitle}{with_timeout}"));
    if let Some(s) = &params.selector {
        log.push(format!("waiting for {}", describe_selector(s)));
    }
    let mut last = Intermediate::default();
    let start = Instant::now();

    let error = 'run: {
        // Step 2: one-shot check, not bound by the timeout.
        match expect_internal(frame, &params, &mut log, &mut last).await {
            Ok(matches) if matches != params.is_not => {
                return ExpectOutcome {
                    matches,
                    ..Default::default()
                };
            }
            Ok(_) => {}
            Err(e) if is_non_retriable(&e, Some(frame)) => break 'run Some(e),
            Err(_) => {}
        }
        // Step 3: retry with backoff until the deadline.
        let mut backoff = Backoff::new(timeout);
        let deadline = Duration::from_millis(timeout);
        loop {
            let remaining = if timeout > 0 {
                match deadline.checked_sub(start.elapsed()) {
                    Some(r) if !r.is_zero() => Some(r),
                    _ => break 'run None,
                }
            } else {
                None
            };
            let round = async {
                backoff.wait().await;
                expect_internal(frame, &params, &mut log, &mut last).await
            };
            let result = match remaining {
                Some(remaining) => tokio::time::timeout(remaining, round).await.ok(),
                None => Some(round.await),
            };
            match result {
                None => break 'run None,
                Some(Ok(matches)) if matches != params.is_not => {
                    return ExpectOutcome {
                        matches,
                        ..Default::default()
                    };
                }
                Some(Ok(_)) => {}
                Some(Err(e)) if is_non_retriable(&e, Some(frame)) => break 'run Some(e),
                Some(Err(_)) => {}
            }
        }
    };

    // The catch block of `Frame.expect`.
    let timed_out = error.is_none();
    let mut outcome = ExpectOutcome {
        matches: params.is_not,
        timed_out,
        ..Default::default()
    };
    let closed = match &error {
        Some(Error::TargetClosed { message, .. }) => {
            let reason = frame.page().and_then(|p| p.close_reason());
            Some(reason.unwrap_or_else(|| {
                if message.is_empty() {
                    TARGET_CLOSED_MESSAGE.to_string()
                } else {
                    message.clone()
                }
            }))
        }
        _ => None,
    };
    if let Some(closed) = &closed {
        log.push(closed.clone());
    }
    let custom = match &error {
        Some(
            Error::InvalidArgument { message, .. }
            | Error::Evaluation { message, .. }
            | Error::StrictModeViolation { message, .. },
        ) => {
            let (message, invalid_selector) = clean_js_error(message);
            outcome.invalid_selector = invalid_selector;
            Some(message)
        }
        _ => None,
    };
    if let Some(custom) = custom {
        outcome.error_message = Some(format!("Error: {custom}"));
    } else if last.set {
        if let Some(received) = last.received.take() {
            outcome.received = received.value;
            outcome.aria_snapshot = received.aria_snapshot;
        }
        outcome.error_message = last.error_message.take().map(|m| format!("Error: {m}"));
    } else if let Some(closed) = closed {
        outcome.error_message = Some(format!("Error: {closed}"));
    } else if let Some(e) = &error {
        outcome.error_message = Some(format!("Error: {}", e.message()));
    }
    outcome.log = compress_call_log(&log);
    outcome
}
