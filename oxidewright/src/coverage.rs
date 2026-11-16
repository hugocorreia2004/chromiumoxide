//! JavaScript and CSS coverage (Playwright's `crCoverage.ts`, with
//! Patchright's reset on main-frame navigation instead of
//! `Runtime.executionContextsCleared`).
//!
//! **Not stealthy**: JS coverage enables the `Profiler` and `Debugger`
//! domains and CSS coverage the `DOM` and `CSS` domains while it runs. A page
//! can notice an attached debugger (e.g. through timing of `debugger`
//! statements or `Error.stack` getters). Only use coverage on pages you
//! control.

use std::collections::{HashMap, HashSet};
use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use chromiumoxide::cdp::browser_protocol::css::EventStyleSheetAdded;
use chromiumoxide::cdp::browser_protocol::page::EventFrameNavigated;
use chromiumoxide::cdp::js_protocol::debugger::{EventPaused, EventScriptParsed};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cdp_raw::RawCommand;
use crate::error::{Error, Result, ResultExt};
use crate::page::Page;

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// A covered range of a function (`Profiler.CoverageRange`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverageRange {
    /// Start offset (inclusive).
    pub start_offset: u64,
    /// End offset (exclusive).
    pub end_offset: u64,
    /// Execution count.
    pub count: u64,
}

/// Coverage of a function (`Profiler.FunctionCoverage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionCoverage {
    /// Function name (empty for anonymous functions).
    pub function_name: String,
    /// Ranges, the first one covering the whole function.
    pub ranges: Vec<CoverageRange>,
    /// Whether the coverage is block-level.
    pub is_block_coverage: bool,
}

/// Coverage of one script.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsCoverageEntry {
    /// Script URL.
    pub url: String,
    /// Script id.
    pub script_id: String,
    /// Script source, when it could be fetched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Per-function coverage.
    pub functions: Vec<FunctionCoverage>,
}

/// A used range of a stylesheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRange {
    /// Start offset (inclusive).
    pub start: u64,
    /// End offset (exclusive).
    pub end: u64,
}

/// Coverage of one stylesheet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CssCoverageEntry {
    /// Stylesheet URL.
    pub url: String,
    /// Stylesheet text.
    pub text: Option<String>,
    /// Used ranges.
    pub ranges: Vec<TextRange>,
}

#[derive(Default)]
struct JsData {
    script_ids: HashSet<String>,
    sources: HashMap<String, String>,
}

#[derive(Default)]
struct CssData {
    urls: HashMap<String, String>,
    sources: HashMap<String, String>,
    order: Vec<String>,
}

type Tasks = Vec<tokio::task::JoinHandle<()>>;

/// Coverage state of a page.
#[derive(Default)]
pub(crate) struct CoverageState {
    /// Collected data, listener tasks and `reportAnonymousScripts`.
    js: Option<(Arc<Mutex<JsData>>, Tasks, bool)>,
    css: Option<(Arc<Mutex<CssData>>, Tasks)>,
}

async fn send(page: &Page, method: &str, params: Value) -> Result<Value> {
    Ok(page
        .inner
        .cdp
        .execute(RawCommand::new(method, params))
        .await?
        .result)
}

/// Reset collected data when the main frame navigates (Patchright).
fn spawn_reset_on_navigation<F>(page: &Page, reset: F) -> Option<tokio::task::JoinHandle<()>>
where
    F: Fn() + Send + 'static,
{
    let cdp = page.inner.cdp.clone();
    Some(tokio::spawn(async move {
        let Ok(mut events) = cdp.event_listener::<EventFrameNavigated>().await else {
            return;
        };
        while let Some(event) = events.next().await {
            if event.frame.parent_id.is_none() {
                reset();
            }
        }
    }))
}

impl Page {
    /// Start collecting JavaScript coverage (V8 precise coverage).
    /// Not stealthy, see [`crate::coverage`].
    pub fn start_js_coverage(&self) -> StartJsCoverage {
        StartJsCoverage {
            page: self.clone(),
            reset_on_navigation: true,
            report_anonymous_scripts: false,
        }
    }

    /// Stop JavaScript coverage and return the entries.
    pub async fn stop_js_coverage(&self) -> Result<Vec<JsCoverageEntry>> {
        async {
            let mut state = self.inner.ext.coverage.lock().await;
            let Some((data, tasks, report_anonymous)) = state.js.take() else {
                return Ok(Vec::new());
            };
            let profile = send(self, "Profiler.takePreciseCoverage", Value::Null).await;
            let _ = send(self, "Profiler.stopPreciseCoverage", Value::Null).await;
            let _ = send(self, "Profiler.disable", Value::Null).await;
            let _ = send(self, "Debugger.disable", Value::Null).await;
            for task in tasks {
                task.abort();
            }
            let profile = profile?;
            let data = data.lock().unwrap();
            let mut entries = Vec::new();
            for entry in profile
                .get("result")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let Ok(mut parsed) = serde_json::from_value::<JsCoverageEntry>(entry) else {
                    continue;
                };
                if !data.script_ids.contains(&parsed.script_id) {
                    continue;
                }
                if parsed.url.is_empty() && !report_anonymous {
                    continue;
                }
                parsed.source = data.sources.get(&parsed.script_id).cloned();
                entries.push(parsed);
            }
            Ok(entries)
        }
        .await
        .api("coverage.stopJSCoverage")
    }

    /// Start collecting CSS rule usage.
    pub fn start_css_coverage(&self) -> StartCssCoverage {
        StartCssCoverage {
            page: self.clone(),
            reset_on_navigation: true,
        }
    }

    /// Stop CSS coverage and return the entries.
    pub async fn stop_css_coverage(&self) -> Result<Vec<CssCoverageEntry>> {
        async {
            let mut state = self.inner.ext.coverage.lock().await;
            let Some((data, tasks)) = state.css.take() else {
                return Ok(Vec::new());
            };
            let usage = send(self, "CSS.stopRuleUsageTracking", Value::Null).await;
            let _ = send(self, "CSS.disable", Value::Null).await;
            let _ = send(self, "DOM.disable", Value::Null).await;
            for task in tasks {
                task.abort();
            }
            let usage = usage?;
            let mut by_sheet: HashMap<String, Vec<(u64, u64, u64)>> = HashMap::new();
            for entry in usage
                .get("ruleUsage")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let Some(id) = entry.get("styleSheetId").and_then(Value::as_str) else {
                    continue;
                };
                let start = entry
                    .get("startOffset")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0) as u64;
                let end = entry
                    .get("endOffset")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0) as u64;
                let used = entry.get("used").and_then(Value::as_bool).unwrap_or(false);
                by_sheet
                    .entry(id.to_string())
                    .or_default()
                    .push((start, end, used as u64));
            }
            let data = data.lock().unwrap();
            Ok(data
                .order
                .iter()
                .filter_map(|id| {
                    let url = data.urls.get(id)?.clone();
                    let ranges = convert_to_disjoint_ranges(
                        by_sheet.get(id).map(Vec::as_slice).unwrap_or_default(),
                    );
                    Some(CssCoverageEntry {
                        url,
                        text: data.sources.get(id).cloned(),
                        ranges,
                    })
                })
                .collect())
        }
        .await
        .api("coverage.stopCSSCoverage")
    }
}

/// Builder returned by [`Page::start_js_coverage`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct StartJsCoverage {
    page: Page,
    reset_on_navigation: bool,
    report_anonymous_scripts: bool,
}

impl StartJsCoverage {
    /// Reset coverage on every navigation (default `true`).
    pub fn reset_on_navigation(mut self, reset: bool) -> Self {
        self.reset_on_navigation = reset;
        self
    }

    /// Report anonymous scripts (default `false`).
    pub fn report_anonymous_scripts(mut self, report: bool) -> Self {
        self.report_anonymous_scripts = report;
        self
    }
}

impl IntoFuture for StartJsCoverage {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            async {
                let page = self.page;
                let mut state = page.inner.ext.coverage.lock().await;
                if state.js.is_some() {
                    return Err(Error::invalid_argument("JSCoverage is already enabled"));
                }
                let data = Arc::new(Mutex::new(JsData::default()));
                let mut tasks = Vec::new();
                let report_anonymous = self.report_anonymous_scripts;
                let mut parsed = page.inner.cdp.event_listener::<EventScriptParsed>().await?;
                let cdp = page.inner.cdp.clone();
                let sink = data.clone();
                tasks.push(tokio::spawn(async move {
                    while let Some(event) = parsed.next().await {
                        let id = event.script_id.as_ref().to_string();
                        sink.lock().unwrap().script_ids.insert(id.clone());
                        if event.url.is_empty() && !report_anonymous {
                            continue;
                        }
                        let cdp = cdp.clone();
                        let sink = sink.clone();
                        tokio::spawn(async move {
                            if let Ok(response) = cdp
                                .execute(RawCommand::new(
                                    "Debugger.getScriptSource",
                                    json!({ "scriptId": id }),
                                ))
                                .await
                            {
                                if let Some(source) =
                                    response.result.get("scriptSource").and_then(Value::as_str)
                                {
                                    sink.lock().unwrap().sources.insert(id, source.to_string());
                                }
                            }
                        });
                    }
                }));
                let mut paused = page.inner.cdp.event_listener::<EventPaused>().await?;
                let cdp = page.inner.cdp.clone();
                tasks.push(tokio::spawn(async move {
                    while paused.next().await.is_some() {
                        let _ = cdp
                            .execute(RawCommand::new("Debugger.resume", Value::Null))
                            .await;
                    }
                }));
                if self.reset_on_navigation {
                    let sink = data.clone();
                    tasks.extend(spawn_reset_on_navigation(&page, move || {
                        let mut d = sink.lock().unwrap();
                        d.script_ids.clear();
                        d.sources.clear();
                    }));
                }
                let result: Result<()> = async {
                    send(&page, "Profiler.enable", Value::Null).await?;
                    send(
                        &page,
                        "Profiler.startPreciseCoverage",
                        json!({ "callCount": true, "detailed": true }),
                    )
                    .await?;
                    send(&page, "Debugger.enable", Value::Null).await?;
                    send(&page, "Debugger.setSkipAllPauses", json!({ "skip": true })).await?;
                    Ok(())
                }
                .await;
                if let Err(err) = result {
                    for task in tasks {
                        task.abort();
                    }
                    return Err(err);
                }
                state.js = Some((data, tasks, report_anonymous));
                Ok(())
            }
            .await
            .api("coverage.startJSCoverage")
        })
    }
}

/// Builder returned by [`Page::start_css_coverage`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct StartCssCoverage {
    page: Page,
    reset_on_navigation: bool,
}

impl StartCssCoverage {
    /// Reset coverage on every navigation (default `true`).
    pub fn reset_on_navigation(mut self, reset: bool) -> Self {
        self.reset_on_navigation = reset;
        self
    }
}

impl IntoFuture for StartCssCoverage {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            async {
                let page = self.page;
                let mut state = page.inner.ext.coverage.lock().await;
                if state.css.is_some() {
                    return Err(Error::invalid_argument("CSSCoverage is already enabled"));
                }
                let data = Arc::new(Mutex::new(CssData::default()));
                let mut tasks = Vec::new();
                let mut added = page
                    .inner
                    .cdp
                    .event_listener::<EventStyleSheetAdded>()
                    .await?;
                let cdp = page.inner.cdp.clone();
                let sink = data.clone();
                tasks.push(tokio::spawn(async move {
                    while let Some(event) = added.next().await {
                        let header = &event.header;
                        if header.source_url.is_empty() {
                            continue;
                        }
                        let id = header.style_sheet_id.as_ref().to_string();
                        let url = header.source_url.clone();
                        if let Ok(response) = cdp
                            .execute(RawCommand::new(
                                "CSS.getStyleSheetText",
                                json!({ "styleSheetId": id }),
                            ))
                            .await
                        {
                            let text = response
                                .result
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            let mut d = sink.lock().unwrap();
                            if !d.urls.contains_key(&id) {
                                d.order.push(id.clone());
                            }
                            d.urls.insert(id.clone(), url);
                            d.sources.insert(id, text);
                        }
                    }
                }));
                if self.reset_on_navigation {
                    let sink = data.clone();
                    tasks.extend(spawn_reset_on_navigation(&page, move || {
                        let mut d = sink.lock().unwrap();
                        d.urls.clear();
                        d.sources.clear();
                        d.order.clear();
                    }));
                }
                let result: Result<()> = async {
                    send(&page, "DOM.enable", Value::Null).await?;
                    send(&page, "CSS.enable", Value::Null).await?;
                    send(&page, "CSS.startRuleUsageTracking", Value::Null).await?;
                    Ok(())
                }
                .await;
                if let Err(err) = result {
                    for task in tasks {
                        task.abort();
                    }
                    return Err(err);
                }
                state.css = Some((data, tasks));
                Ok(())
            }
            .await
            .api("coverage.startCSSCoverage")
        })
    }
}

/// `convertToDisjointRanges`: flatten nested `(start, end, count)` ranges
/// into the disjoint ranges with a positive count.
pub(crate) fn convert_to_disjoint_ranges(nested: &[(u64, u64, u64)]) -> Vec<TextRange> {
    // (offset, type (0 = start, 1 = end), range index)
    let mut points: Vec<(u64, u8, usize)> = Vec::with_capacity(nested.len() * 2);
    for (i, (start, end, _)) in nested.iter().enumerate() {
        points.push((*start, 0, i));
        points.push((*end, 1, i));
    }
    let len = |i: usize| nested[i].1 as i64 - nested[i].0 as i64;
    points.sort_by(|a, b| {
        if a.0 != b.0 {
            return a.0.cmp(&b.0);
        }
        if a.1 != b.1 {
            return b.1.cmp(&a.1);
        }
        if a.1 == 0 {
            len(b.2).cmp(&len(a.2))
        } else {
            len(a.2).cmp(&len(b.2))
        }
    });
    let mut stack: Vec<u64> = Vec::new();
    let mut results: Vec<TextRange> = Vec::new();
    let mut last_offset = 0;
    for (offset, kind, index) in points {
        if !stack.is_empty() && last_offset < offset && *stack.last().unwrap() > 0 {
            match results.last_mut() {
                Some(last) if last.end == last_offset => last.end = offset,
                _ => results.push(TextRange {
                    start: last_offset,
                    end: offset,
                }),
            }
        }
        last_offset = offset;
        if kind == 0 {
            stack.push(nested[index].2);
        } else {
            stack.pop();
        }
    }
    results.retain(|r| r.end - r.start > 1);
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disjoint_ranges() {
        let ranges = convert_to_disjoint_ranges(&[(0, 10, 1), (2, 5, 0)]);
        assert_eq!(
            ranges,
            vec![
                TextRange { start: 0, end: 2 },
                TextRange { start: 5, end: 10 }
            ]
        );
        let ranges = convert_to_disjoint_ranges(&[(0, 4, 1), (4, 8, 1)]);
        assert_eq!(ranges, vec![TextRange { start: 0, end: 8 }]);
        assert!(convert_to_disjoint_ranges(&[(0, 4, 0)]).is_empty());
    }
}
