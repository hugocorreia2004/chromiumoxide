//! Fake timers: [`Clock`] (`page.clock()` / `context.clock()`), driven by
//! Playwright's injected clock (`clockSource.js`) exactly like
//! `server/clock.ts` as patched by Patchright.

use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::browser_context::BrowserContext;
use crate::error::{CONTEXT_DESTROYED_MESSAGE, Error, Result, ResultExt};
use crate::execution_context::{Evaluated, World};
use crate::js::sources;
use crate::page::{InitScript, Page};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Init scripts registered by the clock (`Clock._initScripts`).
#[derive(Default)]
pub(crate) struct ClockState {
    init_scripts: Vec<InitScript>,
    sources: Vec<String>,
}

/// A point in time for the clock: milliseconds since the epoch, a
/// [`SystemTime`] or a date string (`"2024-02-02T08:00:00Z"`).
#[derive(Debug, Clone, PartialEq)]
pub enum ClockTime {
    /// Milliseconds since the Unix epoch.
    Millis(f64),
    /// A date string, parsed like JavaScript's `new Date(string)`.
    Text(String),
}

impl From<i64> for ClockTime {
    fn from(v: i64) -> Self {
        ClockTime::Millis(v as f64)
    }
}

impl From<u64> for ClockTime {
    fn from(v: u64) -> Self {
        ClockTime::Millis(v as f64)
    }
}

impl From<i32> for ClockTime {
    fn from(v: i32) -> Self {
        ClockTime::Millis(v as f64)
    }
}

impl From<f64> for ClockTime {
    fn from(v: f64) -> Self {
        ClockTime::Millis(v)
    }
}

impl From<&str> for ClockTime {
    fn from(v: &str) -> Self {
        ClockTime::Text(v.to_string())
    }
}

impl From<String> for ClockTime {
    fn from(v: String) -> Self {
        ClockTime::Text(v)
    }
}

impl From<SystemTime> for ClockTime {
    fn from(v: SystemTime) -> Self {
        let millis = match v.duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_secs_f64() * 1000.0,
            Err(e) => -(e.duration().as_secs_f64() * 1000.0),
        };
        ClockTime::Millis(millis.trunc())
    }
}

/// A duration for the clock: milliseconds or `"mm:ss"` / `"hh:mm:ss"`.
#[derive(Debug, Clone, PartialEq)]
pub enum ClockTicks {
    /// Milliseconds.
    Millis(f64),
    /// `"mm:ss"` or `"hh:mm:ss"`.
    Text(String),
}

impl From<u64> for ClockTicks {
    fn from(v: u64) -> Self {
        ClockTicks::Millis(v as f64)
    }
}

impl From<i64> for ClockTicks {
    fn from(v: i64) -> Self {
        ClockTicks::Millis(v as f64)
    }
}

impl From<i32> for ClockTicks {
    fn from(v: i32) -> Self {
        ClockTicks::Millis(v as f64)
    }
}

impl From<f64> for ClockTicks {
    fn from(v: f64) -> Self {
        ClockTicks::Millis(v)
    }
}

impl From<std::time::Duration> for ClockTicks {
    fn from(v: std::time::Duration) -> Self {
        ClockTicks::Millis(v.as_millis() as f64)
    }
}

impl From<&str> for ClockTicks {
    fn from(v: &str) -> Self {
        ClockTicks::Text(v.to_string())
    }
}

impl From<String> for ClockTicks {
    fn from(v: String) -> Self {
        ClockTicks::Text(v)
    }
}

/// `parseTicks` of `server/clock.ts`.
pub(crate) fn parse_ticks(ticks: &ClockTicks) -> Result<f64> {
    let text = match ticks {
        ClockTicks::Millis(v) => {
            if !v.is_finite() {
                return Err(Error::invalid_argument(format!("Invalid ticks: {v}")));
            }
            return Ok(*v);
        }
        ClockTicks::Text(text) => text,
    };
    if text.is_empty() {
        return Ok(0.0);
    }
    let parts: Vec<&str> = text.split(':').collect();
    let valid = parts.len() <= 3
        && parts.iter().enumerate().all(|(i, p)| {
            let last = i == parts.len() - 1;
            let ok_len = if last {
                (1..=2).contains(&p.len())
            } else {
                p.len() == 2
            };
            ok_len && p.chars().all(|c| c.is_ascii_digit())
        });
    if !valid {
        return Err(Error::invalid_argument(
            "Clock only understands numbers, 'mm:ss' and 'hh:mm:ss'",
        ));
    }
    let mut ms = 0f64;
    let l = parts.len();
    for (i, part) in parts.iter().enumerate() {
        let parsed: f64 = part.parse().unwrap_or(0.0);
        if parsed >= 60.0 {
            return Err(Error::invalid_argument(format!("Invalid time {text}")));
        }
        ms += parsed * 60f64.powi((l - i - 1) as i32);
    }
    Ok(ms * 1000.0)
}

/// Days from the civil date to 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parse the ISO 8601 forms whose meaning does not depend on the local
/// timezone: `YYYY-MM-DD` (UTC, like JavaScript) and date-times with `Z` or
/// an offset.
pub(crate) fn parse_iso_date(text: &str) -> Option<f64> {
    let re = regex::Regex::new(
        r"^(\d{4})-(\d{2})-(\d{2})(?:[T ](\d{2}):(\d{2})(?::(\d{2})(?:\.(\d{1,9}))?)?(Z|[+-]\d{2}:?\d{2}))?$",
    )
    .ok()?;
    let caps = re.captures(text.trim())?;
    let num = |i: usize| caps.get(i).map(|m| m.as_str().parse::<i64>().unwrap_or(0));
    let (y, mo, d) = (num(1)?, num(2)?, num(3)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    let mut ms = days_from_civil(y, mo, d) * 86_400_000;
    if caps.get(4).is_some() {
        let (h, mi, s) = (num(4)?, num(5)?, num(6).unwrap_or(0));
        if h > 24 || mi > 59 || s > 59 {
            return None;
        }
        ms += (h * 3600 + mi * 60 + s) * 1000;
        if let Some(frac) = caps.get(7) {
            let digits = format!("{:0<3}", frac.as_str());
            ms += digits[..3].parse::<i64>().unwrap_or(0);
        }
        let zone = caps.get(8)?.as_str();
        if zone != "Z" {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let digits: String = zone[1..].chars().filter(|c| c.is_ascii_digit()).collect();
            let oh: i64 = digits[..2].parse().ok()?;
            let om: i64 = digits[2..].parse().ok()?;
            ms -= sign * (oh * 60 + om) * 60_000;
        }
    }
    Some(ms as f64)
}

/// Evaluate `expression` in every frame of every page of the context
/// (`safeNonStallingEvaluateInAllFrames`); JavaScript errors are returned
/// when `throw_on_js_errors`, other failures (navigations, detached frames)
/// are ignored.
pub(crate) async fn evaluate_in_all_frames(
    context: &BrowserContext,
    expression: &str,
    world: World,
    throw_on_js_errors: bool,
) -> Result<()> {
    for page in context.pages() {
        evaluate_in_page_frames(&page, expression, world, throw_on_js_errors).await?;
    }
    Ok(())
}

/// Like [`evaluate_in_all_frames`] for the frames of one page.
pub(crate) async fn evaluate_in_page_frames(
    page: &Page,
    expression: &str,
    world: World,
    throw_on_js_errors: bool,
) -> Result<()> {
    let frames = page.frames();
    let results = futures::future::join_all(
        frames
            .iter()
            .map(|frame| frame.evaluate_in(world, expression, None, true)),
    )
    .await;
    for result in results {
        if let Err(err) = result {
            let is_js_error = matches!(err, Error::Evaluation { .. })
                && !err.message().contains(CONTEXT_DESTROYED_MESSAGE);
            if throw_on_js_errors && is_js_error {
                return Err(err);
            }
        }
    }
    Ok(())
}

fn now_millis() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

fn js_number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 9.0e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// Fake timers of a browser context. Obtain it with [`Page::clock`] or
/// [`BrowserContext::clock`]; cheap to clone.
///
/// ```no_run
/// # async fn run(page: oxidewright::Page) -> oxidewright::Result<()> {
/// page.clock().install().time("2024-02-02T08:00:00Z").await?;
/// page.clock().pause_at("2024-02-02T10:00:00Z").await?;
/// page.clock().fast_forward("30:00").await?;
/// # Ok(()) }
/// ```
#[derive(Clone, Debug)]
pub struct Clock {
    context: Option<BrowserContext>,
}

impl BrowserContext {
    /// The context's [`Clock`].
    pub fn clock(&self) -> Clock {
        Clock {
            context: Some(self.clone()),
        }
    }
}

impl Page {
    /// The [`Clock`] of the page's context.
    pub fn clock(&self) -> Clock {
        Clock {
            context: self.context(),
        }
    }
}

impl Clock {
    fn context(&self) -> Result<&BrowserContext> {
        self.context.as_ref().ok_or_else(Error::target_closed)
    }

    async fn parse_time(&self, time: &ClockTime) -> Result<f64> {
        match time {
            ClockTime::Millis(v) => Ok(*v),
            ClockTime::Text(text) if text.is_empty() => Ok(0.0),
            ClockTime::Text(text) => {
                if let Some(ms) = parse_iso_date(text) {
                    return Ok(ms);
                }
                // Let the browser parse other forms (local times, RFC 2822...).
                if let Some(page) = self.context()?.pages().into_iter().next() {
                    let expression = format!(
                        "() => new Date({}).getTime()",
                        serde_json::to_string(text).unwrap_or_default()
                    );
                    if let Ok(Evaluated::Value(v)) = page
                        .main_frame()
                        .evaluate_in(World::Utility, &expression, None, true)
                        .await
                    {
                        if let Some(ms) = v.as_f64().filter(|v| v.is_finite()) {
                            return Ok(ms);
                        }
                    }
                }
                Err(Error::invalid_argument(format!("Invalid date: {text}")))
            }
        }
    }

    async fn install_if_needed(&self, state: &mut ClockState) -> Result<()> {
        if !state.init_scripts.is_empty() {
            // Patchright: frames that missed the init scripts get them now.
            let sources = serde_json::to_string(&state.sources)?;
            let script = format!(
                "(() => {{\n  if (globalThis.__pwClock?.controller)\n    return;\n  for (const source of {sources})\n    (0, eval)(source);\n}})();"
            );
            return evaluate_in_all_frames(self.context()?, &script, World::Main, true).await;
        }
        let script = format!(
            "(() => {{\nconst module = {{}};\n{}\nif (!globalThis.__pwClock)\n  globalThis.__pwClock = (module.exports.inject())(globalThis, \"chromium\");\n}})();",
            sources::CLOCK
        );
        let context = self.context()?;
        let init = context.add_init_script(script.clone()).await?;
        evaluate_in_all_frames(context, &script, World::Main, true).await?;
        state.init_scripts.push(init);
        state.sources.push(script);
        Ok(())
    }

    async fn run(&self, log: String, call: String) -> Result<()> {
        let context = self.context()?;
        let mut state = context.inner.ext.clock.lock().await;
        self.install_if_needed(&mut state).await?;
        let init = context.add_init_script(log.clone()).await?;
        state.init_scripts.push(init);
        state.sources.push(log);
        evaluate_in_all_frames(context, &call, World::Main, true).await
    }

    /// Install fake timers (`Date`, `setTimeout`, `performance.now`, ...);
    /// optionally starting at [`ClockInstall::time`].
    pub fn install(&self) -> ClockInstall {
        ClockInstall {
            clock: self.clone(),
            time: None,
        }
    }

    /// Advance the clock, firing due timers at most once.
    pub async fn fast_forward(&self, ticks: impl Into<ClockTicks>) -> Result<()> {
        async {
            let ticks = js_number(parse_ticks(&ticks.into())?);
            self.run(
                format!(
                    "globalThis.__pwClock.controller.log('fastForward', {}, {ticks})",
                    js_number(now_millis())
                ),
                format!("globalThis.__pwClock.controller.fastForward({ticks})"),
            )
            .await
        }
        .await
        .api("clock.fastForward")
    }

    /// Advance the clock to `time` and pause it there.
    pub async fn pause_at(&self, time: impl Into<ClockTime>) -> Result<()> {
        async {
            let time = js_number(self.parse_time(&time.into()).await?);
            self.run(
                format!(
                    "globalThis.__pwClock.controller.log('pauseAt', {}, {time})",
                    js_number(now_millis())
                ),
                format!("globalThis.__pwClock.controller.pauseAt({time})"),
            )
            .await
        }
        .await
        .api("clock.pauseAt")
    }

    /// Resume a paused clock.
    pub async fn resume(&self) -> Result<()> {
        async {
            self.run(
                format!(
                    "globalThis.__pwClock.controller.log('resume', {})",
                    js_number(now_millis())
                ),
                "globalThis.__pwClock.controller.resume()".to_string(),
            )
            .await
        }
        .await
        .api("clock.resume")
    }

    /// Advance the clock, firing all due timers.
    pub async fn run_for(&self, ticks: impl Into<ClockTicks>) -> Result<()> {
        async {
            let ticks = js_number(parse_ticks(&ticks.into())?);
            self.run(
                format!(
                    "globalThis.__pwClock.controller.log('runFor', {}, {ticks})",
                    js_number(now_millis())
                ),
                format!("globalThis.__pwClock.controller.runFor({ticks})"),
            )
            .await
        }
        .await
        .api("clock.runFor")
    }

    /// Make `Date.now()` and `new Date()` always return `time`; timers keep
    /// running.
    pub async fn set_fixed_time(&self, time: impl Into<ClockTime>) -> Result<()> {
        async {
            let time = js_number(self.parse_time(&time.into()).await?);
            self.run(
                format!(
                    "globalThis.__pwClock.controller.log('setFixedTime', {}, {time})",
                    js_number(now_millis())
                ),
                format!("globalThis.__pwClock.controller.setFixedTime({time})"),
            )
            .await
        }
        .await
        .api("clock.setFixedTime")
    }

    /// Change the system time without firing timers.
    pub async fn set_system_time(&self, time: impl Into<ClockTime>) -> Result<()> {
        async {
            let time = js_number(self.parse_time(&time.into()).await?);
            self.run(
                format!(
                    "globalThis.__pwClock.controller.log('setSystemTime', {}, {time})",
                    js_number(now_millis())
                ),
                format!("globalThis.__pwClock.controller.setSystemTime({time})"),
            )
            .await
        }
        .await
        .api("clock.setSystemTime")
    }
}

/// Builder returned by [`Clock::install`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct ClockInstall {
    clock: Clock,
    time: Option<ClockTime>,
}

impl ClockInstall {
    /// Initial time (default: now).
    pub fn time(mut self, time: impl Into<ClockTime>) -> Self {
        self.time = Some(time.into());
        self
    }
}

impl IntoFuture for ClockInstall {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            async {
                let time = match &self.time {
                    Some(time) => self.clock.parse_time(time).await?,
                    None => now_millis(),
                };
                let time = js_number(time);
                self.clock
                    .run(
                        format!(
                            "globalThis.__pwClock.controller.log('install', {}, {time})",
                            js_number(now_millis())
                        ),
                        format!("globalThis.__pwClock.controller.install({time})"),
                    )
                    .await
            }
            .await
            .api("clock.install")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ticks_like_playwright() {
        assert_eq!(parse_ticks(&"".into()).unwrap(), 0.0);
        assert_eq!(parse_ticks(&"01:10".into()).unwrap(), 70_000.0);
        assert_eq!(parse_ticks(&"01:10:00".into()).unwrap(), 4_200_000.0);
        assert_eq!(parse_ticks(&1000u64.into()).unwrap(), 1000.0);
        assert!(parse_ticks(&"1:2:3:4".into()).is_err());
        assert!(parse_ticks(&"00:60".into()).is_err());
        assert!(parse_ticks(&"abc".into()).is_err());
    }

    #[test]
    fn parses_iso_dates() {
        assert_eq!(parse_iso_date("1970-01-01"), Some(0.0));
        assert_eq!(
            parse_iso_date("2024-02-02T08:00:00Z"),
            Some(1_706_860_800_000.0)
        );
        assert_eq!(
            parse_iso_date("2024-02-02T10:00:00+02:00"),
            Some(1_706_860_800_000.0)
        );
        assert_eq!(
            parse_iso_date("2024-02-02T08:00:00.5Z"),
            Some(1_706_860_800_500.0)
        );
        assert_eq!(parse_iso_date("2024-02-02T08:00:00"), None);
        assert_eq!(parse_iso_date("nope"), None);
    }
}
