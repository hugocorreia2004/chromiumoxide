//! `waitForFunction` and `waitForTimeout` of [`Page`], [`Frame`] and
//! [`Locator`], ports of `Frame.waitForFunctionExpression`,
//! `Frame.waitForFunctionExpressionOnElement` and `Frame.waitForTimeout` of
//! Playwright's `server/frames.ts`.

use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};

use crate::actions::{Action, Target, kinds};
use crate::element_actions::wait_for_function_on_selector;
use crate::error::{Error, Result};
use crate::execution_context::World;
use crate::frame::Frame;
use crate::frame_selectors::{
    CallArg, SelectorTarget, call_injected, detached_or, handle_of, is_non_retriable,
};
use crate::js::JsValue;
use crate::js_handle::{JSHandle, serialize_arg};
use crate::locator::Locator;
use crate::page::{Page, PageEvent};
use crate::progress::{DEFAULT_TIMEOUT, Progress};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// The `polling` option of `wait_for_function`: `"raf"` (the default) runs
/// the predicate on every animation frame, a number of milliseconds runs it
/// at that interval.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Polling {
    /// Every `requestAnimationFrame` (`'raf'`).
    #[default]
    Raf,
    /// Every `n` milliseconds (must be positive).
    Interval(i64),
    /// An invalid string value (reported when awaited, like Playwright).
    Unknown(String),
}

impl From<&str> for Polling {
    fn from(v: &str) -> Self {
        if v == "raf" {
            Polling::Raf
        } else {
            Polling::Unknown(v.to_string())
        }
    }
}

macro_rules! polling_from_int {
    ($($t:ty),*) => {$(
        impl From<$t> for Polling {
            fn from(v: $t) -> Self {
                Polling::Interval(v as i64)
            }
        }
    )*};
}
polling_from_int!(i32, i64, u32, u64, usize);

/// The predicate loop run by the injected script (`waitForFunctionExpression`).
const WAIT_FOR_FUNCTION_JS: &str = r#"(injected, expression, polling, arg) => {
  let evaledExpression;
  const predicate = () => {
    let result = evaledExpression ?? globalThis.eval(expression);
    if (typeof result === 'function') {
      evaledExpression = result;
      result = result(arg);
    }
    return result;
  };
  let fulfill;
  let reject;
  let aborted = false;
  const result = new Promise((f, r) => { fulfill = f; reject = r; });
  const next = () => {
    if (aborted)
      return;
    try {
      const success = predicate();
      if (success) {
        fulfill(success);
        return;
      }
      if (typeof polling !== 'number')
        injected.utils.builtins.requestAnimationFrame(next);
      else
        injected.utils.builtins.setTimeout(next, polling);
    } catch (e) {
      reject(e);
    }
  };
  next();
  return { result, abort: () => aborted = true };
}"#;

/// Builder returned by `wait_for_function` on [`Page`] / [`Frame`];
/// resolves with a handle to the first truthy value.
#[must_use = "builders do nothing unless awaited"]
pub struct WaitForFunction {
    frame: Frame,
    expression: String,
    arg: Result<Option<(Value, Vec<JSHandle>)>>,
    polling: Polling,
    timeout: Option<u64>,
    world: World,
    api: &'static str,
}

impl WaitForFunction {
    /// The argument passed to the function (any `Serialize` value, which may
    /// contain [`JSHandle`]s).
    pub fn arg<A: Serialize>(mut self, arg: A) -> Self {
        self.arg = serialize_arg(&arg);
        self
    }

    /// `"raf"` (default) or an interval in milliseconds.
    pub fn polling(mut self, polling: impl Into<Polling>) -> Self {
        self.polling = polling.into();
        self
    }

    /// Maximum time in milliseconds (default: the page's default timeout,
    /// 30 s); `0` disables the timeout.
    pub fn timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout = Some(timeout_ms);
        self
    }

    /// Run in an isolated world (`true`, the default with `stealth`) or in
    /// the page's main world (`false`, needed to read page globals).
    pub fn isolated_context(mut self, isolated: bool) -> Self {
        self.world = if isolated {
            World::Utility
        } else {
            World::Main
        };
        self
    }
}

impl IntoFuture for WaitForFunction {
    type Output = Result<JSHandle>;
    type IntoFuture = BoxFuture<Result<JSHandle>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let api = self.api;
            let polling = match &self.polling {
                Polling::Raf => None,
                Polling::Interval(n) if *n > 0 => Some(*n),
                Polling::Interval(n) => {
                    return Err(Error::invalid_argument(format!(
                        "Cannot poll with non-positive interval: {n}"
                    ))
                    .with_api(api));
                }
                Polling::Unknown(s) => {
                    return Err(
                        Error::invalid_argument(format!("Unknown polling option: {s}"))
                            .with_api(api),
                    );
                }
            };
            let arg = self.arg.map_err(|e| e.with_api(api))?;
            let timeout = match self.frame.page_inner() {
                Some(page) => page.timeouts.timeout(self.timeout),
                None => self.timeout.unwrap_or(DEFAULT_TIMEOUT),
            };
            let progress = Progress::new(api, timeout);
            let expression = crate::js::normalize_expression(&self.expression);
            let frame = self.frame;
            let world = self.world;
            progress
                .run(async move {
                    let mut first = true;
                    loop {
                        if !first {
                            tokio::select! {
                                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                                _ = frame_gone(&frame) => {}
                            }
                        }
                        first = false;
                        match attempt(&frame, world, &expression, polling, &arg).await {
                            Ok(handle) => return Ok(handle),
                            Err(e) if is_non_retriable(&e, Some(&frame)) => {
                                return Err(detached_or(e, &frame));
                            }
                            Err(e) => tracing::debug!("retrying waitForFunction: {e}"),
                        }
                    }
                })
                .await
        })
    }
}

/// Aborts the predicate loop when dropped (timeout, error, cancellation),
/// so that it has no side effects after the call returns.
struct AbortOnDrop(Option<JSHandle>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        let Some(handle) = self.0.take() else {
            return;
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = handle.evaluate_inner("h => h.abort()", None, true).await;
                handle.dispose().await;
            });
        }
    }
}

async fn attempt(
    frame: &Frame,
    world: World,
    expression: &str,
    polling: Option<i64>,
    arg: &Option<(Value, Vec<JSHandle>)>,
) -> Result<JSHandle> {
    let ctx = frame.context(world).await?;
    let (wire, handles) = arg
        .clone()
        .unwrap_or_else(|| (json!({ "v": "undefined" }), Vec::new()));
    let state = call_injected(
        &ctx,
        WAIT_FOR_FUNCTION_JS,
        vec![
            CallArg::Json(Value::String(expression.to_string())),
            CallArg::Js(match polling {
                Some(n) => JsValue::Number(n as f64),
                None => JsValue::Undefined,
            }),
            CallArg::Wire(wire, handles),
        ],
        false,
    )
    .await?;
    let state = handle_of(state)?;
    let mut guard = AbortOnDrop(Some(state.clone()));
    let result = tokio::select! {
        r = state.evaluate_inner("h => h.result", None, false) => r,
        _ = frame_gone(frame) => Err(Error::frame_detached("Frame was detached")),
    };
    match result {
        Ok(evaluated) => {
            guard.0 = None;
            state.dispose().await;
            handle_of(evaluated)
        }
        Err(e) => Err(e),
    }
}

/// Resolves when the frame detaches or its page closes.
async fn frame_gone(frame: &Frame) {
    let Some(page) = frame.page_inner() else {
        return;
    };
    let mut rx = page.subscribe();
    if frame.is_detached() {
        return;
    }
    let id = frame.id().to_string();
    loop {
        match rx.recv().await {
            Some(PageEvent::FrameDetached(f)) if f == id => return,
            Some(PageEvent::Closed) | None => return,
            Some(_) => {}
        }
    }
}

/// `Frame.waitForTimeout`: sleep, failing early when the page closes or the
/// frame detaches.
async fn wait_for_timeout(frame: &Frame, timeout_ms: u64, api: &'static str) -> Result<()> {
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_millis(timeout_ms)) => Ok(()),
        _ = frame_gone(frame) => {
            let err = if frame.page_inner().is_some_and(|p| !p.is_closed()) {
                Error::frame_detached("Frame was detached")
            } else {
                Error::target_closed()
            };
            Err(err.with_api(api))
        }
    }
}

impl Frame {
    /// Wait until `expression` (a function or an expression) returns a
    /// truthy value; resolves with a handle to it. Builder: set `.arg(..)`,
    /// `.polling(..)`, `.timeout(..)`, `.isolated_context(..)`, then
    /// `.await` it.
    ///
    /// ```no_run
    /// # async fn run(frame: oxidewright::Frame) -> oxidewright::Result<()> {
    /// frame
    ///     .wait_for_function("n => document.querySelectorAll('li').length >= n")
    ///     .arg(3)
    ///     .polling(100)
    ///     .await?;
    /// # Ok(()) }
    /// ```
    pub fn wait_for_function(&self, expression: impl Into<String>) -> WaitForFunction {
        WaitForFunction {
            frame: self.clone(),
            expression: expression.into(),
            arg: Ok(None),
            polling: Polling::Raf,
            timeout: None,
            world: World::default_evaluate(),
            api: "frame.waitForFunction",
        }
    }

    /// Wait for `timeout_ms` milliseconds (discouraged: prefer waiting for a
    /// condition).
    pub async fn wait_for_timeout(&self, timeout_ms: u64) -> Result<()> {
        wait_for_timeout(self, timeout_ms, "frame.waitForTimeout").await
    }
}

impl Page {
    /// Wait until `expression` returns a truthy value in the main frame;
    /// see [`Frame::wait_for_function`].
    pub fn wait_for_function(&self, expression: impl Into<String>) -> WaitForFunction {
        let mut builder = self.main_frame().wait_for_function(expression);
        builder.api = "page.waitForFunction";
        builder
    }

    /// Wait for `timeout_ms` milliseconds (discouraged: prefer waiting for a
    /// condition).
    pub async fn wait_for_timeout(&self, timeout_ms: u64) -> Result<()> {
        wait_for_timeout(&self.main_frame(), timeout_ms, "page.waitForTimeout").await
    }
}

impl Locator {
    /// Wait until `expression`, called with the matching element (and the
    /// argument), returns a truthy value. Strict, auto-waits for the
    /// element. Builder: set `.arg(..)`, `.timeout(..)`,
    /// `.isolated_context(..)`, then `.await` it.
    pub fn wait_for_function(
        &self,
        expression: impl Into<String>,
    ) -> Action<kinds::WaitForFunction> {
        let target = match self.construction_error() {
            Some(e) => Target::Invalid(e.to_string()),
            None => Target::selector(self.frame(), self.selector(), true),
        };
        Action::new(target, "locator.waitForFunction").with_text(expression)
    }
}

impl Action<kinds::WaitForFunction> {
    /// Run in an isolated world (`true`, the default with `stealth`) or in
    /// the page's main world (`false`).
    pub fn isolated_context(mut self, isolated: bool) -> Self {
        self.opts.isolated = Some(isolated);
        self
    }
}

impl IntoFuture for Action<kinds::WaitForFunction> {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(mut self) -> Self::IntoFuture {
        Box::pin(async move {
            let api = self.api;
            let arg = match self.args.arg.take() {
                None => None,
                Some(r) => r.map_err(|e| e.with_api(api))?,
            };
            let (wire, handles) = arg.unwrap_or_else(|| (json!({ "v": "undefined" }), Vec::new()));
            let expression = crate::js::normalize_expression(&self.args.text);
            let callback = format!(
                "(injected, element, arg) => {{ let result = globalThis.eval({}); if (typeof result === 'function') result = result(element, arg); return result; }}",
                serde_json::to_string(&expression).unwrap_or_default()
            );
            let main_world = match self.opts.isolated {
                Some(isolated) => !isolated,
                None => World::default_evaluate() == World::Main,
            };
            let Target::Selector {
                frame,
                selector,
                strict,
            } = self.target.clone()
            else {
                let message = match &self.target {
                    Target::Invalid(m) => m.clone(),
                    _ => "wait_for_function requires a locator".to_string(),
                };
                return Err(Error::invalid_argument(message).with_api(api));
            };
            let timeout = match frame.page_inner() {
                Some(page) => page.timeouts.timeout(self.opts.timeout),
                None => self.opts.timeout.unwrap_or(DEFAULT_TIMEOUT),
            };
            let progress = Progress::new(api, timeout);
            let p = progress.clone();
            progress
                .run(async move {
                    wait_for_function_on_selector(
                        &frame,
                        &p,
                        &SelectorTarget::new(selector, strict),
                        &callback,
                        CallArg::Wire(wire, handles),
                        main_world,
                    )
                    .await
                    .map(|_| ())
                })
                .await
        })
    }
}
