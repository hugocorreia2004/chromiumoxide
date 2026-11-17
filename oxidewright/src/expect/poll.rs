//! `expect(fn).toPass()` and `expect.poll(fn)` (Playwright Test's
//! `matchers.ts::toPass` and `expect.ts::pollMatcher`): retry a closure
//! until it succeeds, with growing intervals.

use std::fmt::{Debug, Display};
use std::future::{Future, IntoFuture};
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::time::{Duration, Instant};

use futures::FutureExt;

use super::{MatcherResult, default_timeout};
use crate::error::{Error, Result};

const DEFAULT_INTERVALS: [u64; 4] = [100, 250, 500, 1000];

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Run `attempt` until it returns `Ok(true)` (keep going on `Ok(false)` or an
/// error) or the deadline passes. Returns the last failure message, or `None`
/// on success.
async fn poll_until<F>(
    timeout: u64,
    intervals: &[u64],
    mut attempt: F,
) -> std::result::Result<(), Option<String>>
where
    F: FnMut() -> BoxFuture<std::result::Result<bool, String>>,
{
    let deadline = (timeout > 0).then(|| Instant::now() + Duration::from_millis(timeout));
    let mut last = None;
    let mut index = 0;
    loop {
        // A deadline also bounds a single slow attempt, like Playwright's
        // `pollAgainstDeadline` racing each call.
        let outcome = match deadline {
            Some(deadline) => match tokio::time::timeout_at(deadline.into(), attempt()).await {
                Ok(outcome) => outcome,
                Err(_) => return Err(last),
            },
            None => attempt().await,
        };
        match outcome {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(message) => last = Some(message),
        }
        let wait = intervals
            .get(index)
            .or_else(|| intervals.last())
            .copied()
            .unwrap_or(1000);
        index += 1;
        let next = Instant::now() + Duration::from_millis(wait);
        if let Some(deadline) = deadline {
            if next >= deadline {
                tokio::time::sleep_until(deadline.into()).await;
                return Err(last);
            }
        }
        tokio::time::sleep_until(next.into()).await;
    }
}

/// The message of a panic payload (failed `assert!` inside the closure).
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "the closure panicked".to_string()
    }
}

fn failure(name: &str, message: String, timeout: u64) -> Error {
    Error::Expect(Box::new(MatcherResult {
        name: name.to_string(),
        pass: false,
        expected: None,
        actual: None,
        message,
        log: Vec::new(),
        timeout: Some(timeout),
        aria_snapshot: None,
    }))
}

// ----------------------------------------------------------------------
// toPass
// ----------------------------------------------------------------------

/// Retry a block of code until it passes: `expect(async () => {...}).toPass()`.
///
/// The closure returns a future of `Result<(), E>`; an `Err` or a panic (a
/// failed `assert!`) counts as a failure and the closure is retried after
/// 100, 250, 500, then every 1000 ms. Without [`ToPass::timeout`] it retries
/// forever, like Playwright.
///
/// ```no_run
/// # async fn run(page: oxidewright::Page) -> oxidewright::Result<()> {
/// use oxidewright::expect::to_pass;
/// to_pass(move || {
///     let page = page.clone();
///     async move {
///         let title = page.title().await?;
///         assert_eq!(title, "Ready");
///         Ok::<_, oxidewright::Error>(())
///     }
/// })
/// .timeout(10_000)
/// .await?;
/// # Ok(()) }
/// ```
#[must_use = "assertions do nothing unless awaited"]
pub fn to_pass<F, Fut, E>(callback: F) -> ToPass<F>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = std::result::Result<(), E>> + Send + 'static,
    E: Display,
{
    ToPass {
        callback,
        timeout: 0,
        intervals: DEFAULT_INTERVALS.to_vec(),
        is_not: false,
    }
}

/// Builder returned by [`to_pass`]; `.await` it.
#[must_use = "assertions do nothing unless awaited"]
pub struct ToPass<F> {
    callback: F,
    timeout: u64,
    intervals: Vec<u64>,
    is_not: bool,
}

impl<F> ToPass<F> {
    /// Give up after this many milliseconds (default `0` = never).
    pub fn timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout = timeout_ms;
        self
    }

    /// Waits between attempts in ms; the last one repeats.
    pub fn intervals(mut self, intervals: impl Into<Vec<u64>>) -> Self {
        self.intervals = intervals.into();
        self
    }

    /// Expect the block to keep failing until the timeout.
    #[allow(clippy::should_implement_trait)]
    pub fn not(mut self) -> Self {
        self.is_not = !self.is_not;
        self
    }
}

impl<F, Fut, E> IntoFuture for ToPass<F>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = std::result::Result<(), E>> + Send + 'static,
    E: Display,
{
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;

    fn into_future(self) -> Self::IntoFuture {
        let ToPass {
            mut callback,
            timeout,
            intervals,
            is_not,
        } = self;
        Box::pin(async move {
            let result = poll_until(timeout, &intervals, move || {
                let fut = AssertUnwindSafe(callback()).catch_unwind();
                Box::pin(async move {
                    let outcome = match fut.await {
                        Ok(Ok(())) => Ok(()),
                        Ok(Err(e)) => Err(e.to_string()),
                        Err(payload) => Err(panic_message(payload)),
                    };
                    match (outcome, is_not) {
                        (Ok(()), false) => Ok(true),
                        (Ok(()), true) => Ok(false),
                        (Err(m), false) => Err(m),
                        (Err(_), true) => Ok(true),
                    }
                })
            })
            .await;
            match result {
                Ok(()) => Ok(()),
                Err(last) => {
                    let timeout_message =
                        format!("Timeout {timeout}ms exceeded while waiting on the predicate");
                    let message = match last {
                        Some(last) => format!("{last}\n\nCall Log:\n- {timeout_message}"),
                        None => timeout_message,
                    };
                    Err(failure("toPass", message, timeout))
                }
            }
        })
    }
}

// ----------------------------------------------------------------------
// expect.poll
// ----------------------------------------------------------------------

/// Poll a value until a matcher passes: `expect.poll(async () => value)`.
///
/// The closure is called after 100, 250, 500, then every 1000 ms until the
/// matcher passes or the timeout (the assertion default, 5 s) runs out.
///
/// ```no_run
/// # async fn run(page: oxidewright::Page) -> oxidewright::Result<()> {
/// use oxidewright::expect::poll;
/// poll(move || {
///     let page = page.clone();
///     async move { page.evaluate::<i64>("() => window.counter").await.unwrap_or(0) }
/// })
/// .timeout(10_000)
/// .to_equal(3)
/// .await?;
/// # Ok(()) }
/// ```
#[must_use = "assertions do nothing unless awaited"]
pub fn poll<F, Fut, T>(callback: F) -> Poll<F>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = T> + Send + 'static,
    T: Debug + Send + 'static,
{
    Poll {
        callback,
        timeout: None,
        intervals: DEFAULT_INTERVALS.to_vec(),
        is_not: false,
        message: None,
    }
}

/// Builder returned by [`poll`]: set options, then pick a matcher.
#[must_use = "assertions do nothing unless awaited"]
pub struct Poll<F> {
    callback: F,
    timeout: Option<u64>,
    intervals: Vec<u64>,
    is_not: bool,
    message: Option<String>,
}

impl<F> Poll<F> {
    /// Maximum time in ms (default: the assertion timeout, 5000).
    pub fn timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout = Some(timeout_ms);
        self
    }

    /// Waits between attempts in ms; the last one repeats.
    pub fn intervals(mut self, intervals: impl Into<Vec<u64>>) -> Self {
        self.intervals = intervals.into();
        self
    }

    /// Negate the matcher.
    #[allow(clippy::should_implement_trait)]
    pub fn not(mut self) -> Self {
        self.is_not = !self.is_not;
        self
    }

    /// A custom message shown first when the assertion fails.
    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
}

impl<F, Fut, T> Poll<F>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = T> + Send + 'static,
    T: Debug + Send + 'static,
{
    /// Pass when the value equals `expected` (`toBe` / `toEqual`).
    pub fn to_equal<U>(self, expected: U) -> BoxFuture<Result<()>>
    where
        T: PartialEq<U>,
        U: Debug + Send + Sync + 'static,
    {
        let shown = format!("{expected:?}");
        self.matcher("toEqual", Some(shown), move |v: &T| *v == expected)
    }

    /// Alias of [`Poll::to_equal`] (`toBe`).
    pub fn to_be<U>(self, expected: U) -> BoxFuture<Result<()>>
    where
        T: PartialEq<U>,
        U: Debug + Send + Sync + 'static,
    {
        let shown = format!("{expected:?}");
        self.matcher("toBe", Some(shown), move |v: &T| *v == expected)
    }

    /// Pass when the predicate returns `true` for the value.
    pub fn to_satisfy(
        self,
        predicate: impl Fn(&T) -> bool + Send + Sync + 'static,
    ) -> BoxFuture<Result<()>> {
        self.matcher("toSatisfy", None, predicate)
    }

    fn matcher(
        self,
        name: &'static str,
        expected: Option<String>,
        check: impl Fn(&T) -> bool + Send + Sync + 'static,
    ) -> BoxFuture<Result<()>> {
        let Poll {
            mut callback,
            timeout,
            intervals,
            is_not,
            message,
        } = self;
        let timeout = timeout.unwrap_or_else(default_timeout);
        Box::pin(async move {
            let check = std::sync::Arc::new(check);
            let last_value = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
            let last = last_value.clone();
            let result = poll_until(timeout, &intervals, move || {
                let fut = callback();
                let check = check.clone();
                let last = last.clone();
                Box::pin(async move {
                    let value = fut.await;
                    *last.lock().unwrap() = Some(format!("{value:?}"));
                    Ok(check(&value) != is_not)
                })
            })
            .await;
            if result.is_ok() {
                return Ok(());
            }
            let received = last_value
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| "<no value>".into());
            let not = if is_not { "not." } else { "" };
            let mut text = String::new();
            if let Some(message) = message {
                text.push_str(&message);
                text.push_str("\n\n");
            }
            text.push_str(&format!("expect.poll(received).{not}{name}(expected)\n\n"));
            if let Some(expected) = &expected {
                text.push_str(&format!(
                    "Expected: {}{expected}\n",
                    if is_not { "not " } else { "" }
                ));
            }
            text.push_str(&format!("Received: {received}\n\nCall Log:\n- Timeout {timeout}ms exceeded while waiting on the predicate"));
            Err(failure(name, text, timeout))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn to_pass_retries_until_ok() {
        let n = Arc::new(AtomicU32::new(0));
        let c = n.clone();
        to_pass(move || {
            let c = c.clone();
            async move {
                let v = c.fetch_add(1, Ordering::SeqCst);
                assert!(v >= 2, "not yet");
                Ok::<_, String>(())
            }
        })
        .intervals(vec![1])
        .await
        .unwrap();
        assert_eq!(n.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn to_pass_times_out_with_the_last_error() {
        let err = to_pass(|| async { Err::<(), _>("boom") })
            .intervals(vec![10])
            .timeout(100)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("boom"), "{msg}");
        assert!(
            msg.contains("Timeout 100ms exceeded while waiting on the predicate"),
            "{msg}"
        );
    }

    #[tokio::test]
    async fn not_to_pass_passes_when_it_keeps_failing() {
        to_pass(|| async { Err::<(), _>("never") })
            .not()
            .intervals(vec![1])
            .timeout(50)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn poll_to_equal() {
        let n = Arc::new(AtomicU32::new(0));
        let c = n.clone();
        poll(move || {
            let c = c.clone();
            async move { c.fetch_add(1, Ordering::SeqCst) }
        })
        .intervals(vec![1])
        .to_equal(3u32)
        .await
        .unwrap();
        let err = poll(|| async { 1 })
            .intervals(vec![5])
            .timeout(50)
            .to_equal(2)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Expected: 2") && err.contains("Received: 1"),
            "{err}"
        );
    }
}
