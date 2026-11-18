//! ARIA snapshots of pages and locators (`ariaSnapshot()` /
//! `ariaSnapshotJSON()`), a port of `Frame.ariaSnapshot*` and
//! `ariaSnapshotJSONForFrame` of Playwright's server, plus the YAML renderer
//! of `isomorphic/ariaSnapshotRenderer.ts`.

use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::actions::{Action, Target, kinds, run_task};
use crate::element_actions::Task;
use crate::element_actions::TaskOutput;
use crate::error::{Error, Result};
use crate::frame::Frame;
use crate::frame_selectors::{
    CallArg, CallOptions, SelectorTarget, call_on_selector, is_non_retriable, value_of,
};
use crate::js::serde_impl::from_js_value;
use crate::page::Page;
use crate::progress::{DEFAULT_TIMEOUT, Progress};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Options shared by the snapshot builders.
#[derive(Clone, Default)]
struct SnapshotOptions {
    mode: Option<String>,
    depth: Option<u32>,
    boxes: Option<bool>,
}

impl SnapshotOptions {
    fn to_json(&self) -> Value {
        let mut options = json!({ "mode": self.mode.clone().unwrap_or_else(|| "default".into()) });
        if let Some(depth) = self.depth {
            options["depth"] = json!(depth);
        }
        if let Some(boxes) = self.boxes {
            options["boxes"] = json!(boxes);
        }
        options
    }

    fn is_ai(&self) -> bool {
        self.mode.as_deref() == Some("ai")
    }
}

/// `ariaSnapshotJSONForFrame`: the snapshot of `selector` (default the
/// body) with the content of rendered iframes merged in.
fn snapshot_json_for_frame(
    progress: Progress,
    frame: Frame,
    selector: Option<String>,
    options: SnapshotOptions,
    strict: Option<bool>,
) -> BoxFuture<Result<Value>> {
    Box::pin(async move {
        let target = SelectorTarget::new(
            selector
                .clone()
                .unwrap_or_else(|| "body,frameset".to_string()),
            strict.unwrap_or(selector.is_some()),
        );
        let delays = [0u64, 1000, 2000, 4000, 8000];
        let mut attempt = 0usize;
        let (snapshot, resolved_frame) = loop {
            let delay = delays[attempt.min(delays.len() - 1)];
            attempt += 1;
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            let result = call_on_selector(
                &frame,
                &target,
                "({ injected, elements }, options) => injected.ariaSnapshotJSON(elements[0], options)",
                CallArg::Json(options.to_json()),
                CallOptions {
                    by_value: true,
                    ..Default::default()
                },
            )
            .await;
            match result {
                Ok(Some(r)) => {
                    let value: Value =
                        from_js_value(value_of(r.result)).map_err(Error::serialization)?;
                    break (value, r.frame);
                }
                Ok(None) => {
                    if let Some(selector) = &selector {
                        return Err(Error::evaluation(format!(
                            "Selector \"{selector}\" does not match any element"
                        )));
                    }
                }
                Err(e) if is_non_retriable(&e, Some(&frame)) => return Err(e),
                Err(_) => {}
            }
        };
        let mut json = snapshot.get("json").cloned().unwrap_or(json!([]));
        let depths = snapshot
            .get("iframeDepths")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let refs: Vec<String> = snapshot
            .get("iframeRefs")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|r| r.as_str().map(str::to_string))
                    .filter(|r| depths.contains_key(r))
                    .collect()
            })
            .unwrap_or_default();
        let mut children = Map::new();
        for r in refs {
            // `options.depth ? options.depth - iframeDepths[ref] - 1 : undefined`,
            // where a non-positive depth means "no limit".
            let child_depth = options.depth.filter(|d| *d > 0).and_then(|d| {
                let at = depths.get(&r).and_then(Value::as_i64).unwrap_or(0);
                let depth = d as i64 - at - 1;
                (depth > 0).then_some(depth as u32)
            });
            let child_options = SnapshotOptions {
                depth: child_depth,
                ..options.clone()
            };
            let selector = format!("aria-ref={r} >> internal:control=enter-frame >> body,frameset");
            let child = snapshot_json_for_frame(
                progress.clone(),
                resolved_frame.clone(),
                Some(selector),
                child_options,
                Some(false),
            )
            .await
            .unwrap_or(json!([]));
            children.insert(r, child);
        }
        if let Some(nodes) = json.as_array_mut() {
            for node in nodes {
                merge_iframe_children(node, &children);
            }
        }
        Ok(json)
    })
}

fn merge_iframe_children(node: &mut Value, children: &Map<String, Value>) {
    let Some(object) = node.as_object_mut() else {
        return;
    };
    if object.get("role").and_then(Value::as_str) == Some("iframe") {
        if let Some(r) = object.get("ref").and_then(Value::as_str) {
            if let Some(child) = children.get(r) {
                if child.as_array().is_some_and(|a| !a.is_empty()) {
                    object.remove("text");
                    object.insert("children".into(), child.clone());
                }
            }
            return;
        }
    }
    if let Some(Value::Array(items)) = object.get_mut("children") {
        for item in items {
            merge_iframe_children(item, children);
        }
    }
}

// ----------------------------------------------------------------------
// YAML rendering (isomorphic/ariaSnapshotRenderer.ts, isomorphic/yaml.ts)
// ----------------------------------------------------------------------

fn js_is_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// `yamlStringNeedsQuotes`.
fn yaml_needs_quotes(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    if s.starts_with(js_is_space) || s.ends_with(js_is_space) {
        return true;
    }
    if s.chars().any(|c| {
        matches!(c, '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' | '\u{7f}'..='\u{9f}')
    }) {
        return true;
    }
    if s.starts_with('-') {
        return true;
    }
    let chars: Vec<char> = s.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if *c == '\n' || *c == ':' {
            match chars.get(i + 1) {
                None => return true,
                Some(n) if js_is_space(*n) => return true,
                _ => {}
            }
        }
        if js_is_space(*c) && chars.get(i + 1) == Some(&'#') {
            return true;
        }
    }
    if s.contains(['\n', '\r']) {
        return true;
    }
    if s.starts_with([
        '&', '*', ']', ',', '?', '!', '>', '|', '@', '"', '\'', '#', '%',
    ]) {
        return true;
    }
    if s.contains(['{', '}', '`']) {
        return true;
    }
    if s.starts_with('[') {
        return true;
    }
    if !crate::selector_parser::js_to_number(s).is_nan() {
        return true;
    }
    matches!(
        s.to_lowercase().as_str(),
        "y" | "n" | "yes" | "no" | "true" | "false" | "on" | "off" | "null"
    )
}

fn yaml_escape_key(s: &str) -> String {
    if !yaml_needs_quotes(s) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "''"))
}

fn yaml_escape_value(s: &str) -> String {
    if !yaml_needs_quotes(s) {
        return s.to_string();
    }
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A JSON number the way JavaScript prints it.
fn js_number(v: &Value) -> String {
    match v.as_f64() {
        Some(f) if f.fract() == 0.0 && f.abs() < 1e21 => format!("{}", f as i64),
        Some(f) => format!("{f}"),
        None => v.to_string(),
    }
}

fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

fn create_key(node: &Map<String, Value>) -> String {
    let mut key = node
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if let Some(name) = node.get("name").and_then(Value::as_str) {
        if !name.is_empty() && name.encode_utf16().count() <= 900 {
            key.push(' ');
            key.push_str(&serde_json::to_string(name).unwrap_or_default());
        }
    }
    let flag = |name: &str| node.get(name);
    match flag("checked") {
        Some(Value::String(s)) if s == "mixed" => key.push_str(" [checked=mixed]"),
        Some(Value::Bool(true)) => key.push_str(" [checked]"),
        _ => {}
    }
    let truthy =
        |name: &str| flag(name).is_some_and(|v| !matches!(v, Value::Null | Value::Bool(false)));
    if truthy("disabled") {
        key.push_str(" [disabled]");
    }
    if truthy("expanded") {
        key.push_str(" [expanded]");
    }
    if truthy("active") {
        key.push_str(" [active]");
    }
    match flag("invalid") {
        Some(Value::String(s)) if s == "grammar" || s == "spelling" => {
            key.push_str(&format!(" [invalid={s}]"))
        }
        Some(Value::Bool(true)) => key.push_str(" [invalid]"),
        _ => {}
    }
    if let Some(level) = flag("level").filter(|v| v.as_f64().is_some_and(|f| f != 0.0)) {
        key.push_str(&format!(" [level={}]", js_number(level)));
    }
    match flag("pressed") {
        Some(Value::String(s)) if s == "mixed" => key.push_str(" [pressed=mixed]"),
        Some(Value::Bool(true)) => key.push_str(" [pressed]"),
        _ => {}
    }
    if flag("selected") == Some(&Value::Bool(true)) {
        key.push_str(" [selected]");
    }
    if truthy("ariaHidden") {
        key.push_str(" [aria-hidden]");
    }
    if let Some(r) = flag("ref")
        .and_then(Value::as_str)
        .filter(|r| !r.is_empty())
    {
        key.push_str(&format!(" [ref={r}]"));
        if flag("cursor").and_then(Value::as_str) == Some("pointer") {
            key.push_str(" [cursor=pointer]");
        }
    }
    if let Some(b) = flag("box").and_then(Value::as_object) {
        let n = |k: &str| b.get(k).map(js_number).unwrap_or_default();
        key.push_str(&format!(
            " [box={},{},{},{}]",
            n("x"),
            n("y"),
            n("width"),
            n("height")
        ));
    }
    key
}

fn visit_text(lines: &mut Vec<String>, text: &str, depth: usize) {
    let escaped = yaml_escape_value(text);
    if !escaped.is_empty() {
        lines.push(format!("{}- text: {escaped}", indent(depth)));
    }
}

fn visit(lines: &mut Vec<String>, node: &Value, depth: usize) {
    let Some(node) = node.as_object() else {
        return;
    };
    if node.get("role").and_then(Value::as_str) == Some("text") {
        visit_text(
            lines,
            node.get("text").and_then(Value::as_str).unwrap_or_default(),
            depth,
        );
        return;
    }
    let escaped_key = format!("{}- {}", indent(depth), yaml_escape_key(&create_key(node)));
    let mut props = Vec::new();
    for name in ["url", "placeholder"] {
        if let Some(v) = node.get(name).and_then(Value::as_str) {
            props.push((name, v.to_string()));
        }
    }
    let text = node.get("text").and_then(Value::as_str);
    let children = node.get("children").and_then(Value::as_array);
    if text.is_none() && props.is_empty() && children.is_none_or(|c| c.is_empty()) {
        lines.push(escaped_key);
    } else if let (Some(text), true) = (text, props.is_empty()) {
        lines.push(format!("{escaped_key}: {}", yaml_escape_value(text)));
    } else {
        lines.push(format!("{escaped_key}:"));
        for (name, value) in props {
            lines.push(format!(
                "{}- /{name}: {}",
                indent(depth + 1),
                yaml_escape_value(&value)
            ));
        }
        if let Some(text) = text {
            visit_text(lines, text, depth + 1);
        } else {
            for child in children.into_iter().flatten() {
                match child {
                    Value::String(s) => visit_text(lines, s, depth + 1),
                    other => visit(lines, other, depth + 1),
                }
            }
        }
    }
}

/// `renderAriaSnapshotAsYaml`.
pub(crate) fn render_yaml(snapshot: &Value) -> String {
    let mut lines = Vec::new();
    for node in snapshot.as_array().into_iter().flatten() {
        visit(&mut lines, node, 0);
    }
    lines.join("\n")
}

// ----------------------------------------------------------------------
// Builders
// ----------------------------------------------------------------------

/// Builder returned by [`Page::aria_snapshot`] (YAML, `T = String`) and
/// [`Page::aria_snapshot_json`] (JSON tree, `T = serde_json::Value`);
/// `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct PageAriaSnapshot<T> {
    page: Page,
    options: SnapshotOptions,
    timeout: Option<u64>,
    api: &'static str,
    _marker: std::marker::PhantomData<fn() -> T>,
}

impl<T> PageAriaSnapshot<T> {
    /// Snapshot mode, `"default"` or `"ai"` (adds element refs, includes
    /// iframes, marks clickable elements).
    pub fn mode(mut self, mode: impl Into<String>) -> Self {
        self.options.mode = Some(mode.into());
        self
    }

    /// Maximum depth of the snapshot.
    pub fn depth(mut self, depth: u32) -> Self {
        self.options.depth = Some(depth);
        self
    }

    /// Include each element's bounding box (`[box=x,y,width,height]`).
    pub fn boxes(mut self, boxes: bool) -> Self {
        self.options.boxes = Some(boxes);
        self
    }

    /// Maximum time in milliseconds (default: the page's default timeout).
    pub fn timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout = Some(timeout_ms);
        self
    }

    async fn json(self) -> Result<Value> {
        let timeout = self.page.inner.timeouts.timeout(self.timeout);
        let progress = Progress::new(self.api, timeout);
        let frame = self.page.main_frame();
        progress
            .run(snapshot_json_for_frame(
                progress.clone(),
                frame,
                None,
                self.options,
                None,
            ))
            .await
    }
}

impl IntoFuture for PageAriaSnapshot<String> {
    type Output = Result<String>;
    type IntoFuture = BoxFuture<Result<String>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { self.json().await.map(|json| render_yaml(&json)) })
    }
}

impl IntoFuture for PageAriaSnapshot<Value> {
    type Output = Result<Value>;
    type IntoFuture = BoxFuture<Result<Value>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.json())
    }
}

impl Page {
    /// The ARIA snapshot of the page as YAML (including the content of
    /// iframes in `"ai"` mode). Builder: `.mode(..)`, `.depth(..)`,
    /// `.boxes(..)`, `.timeout(..)`, then `.await` it.
    pub fn aria_snapshot(&self) -> PageAriaSnapshot<String> {
        self.snapshot_builder("page.ariaSnapshot")
    }

    /// The ARIA snapshot of the page as a JSON tree (an array of nodes such
    /// as `{ "role": "button", "name": "OK" }` and text strings).
    pub fn aria_snapshot_json(&self) -> PageAriaSnapshot<Value> {
        self.snapshot_builder("page.ariaSnapshotJSON")
    }

    /// The element with this `ref` in the last `"ai"`-mode ARIA snapshot
    /// (e.g. `e3`), for agents acting on a snapshot (Playwright's
    /// `page.getByRef()`).
    pub fn get_by_ref(&self, aria_ref: &str) -> crate::Locator {
        self.locator(format!("aria-ref={aria_ref}"))
    }

    fn snapshot_builder<T>(&self, api: &'static str) -> PageAriaSnapshot<T> {
        PageAriaSnapshot {
            page: self.clone(),
            options: SnapshotOptions::default(),
            timeout: None,
            api,
            _marker: std::marker::PhantomData,
        }
    }
}

macro_rules! snapshot_options {
    ($kind:ty) => {
        impl Action<$kind> {
            /// Maximum depth of the snapshot.
            pub fn depth(mut self, depth: u32) -> Self {
                self.opts.depth = Some(depth);
                self
            }

            /// Snapshot mode, `"default"` or `"ai"`.
            pub fn mode(mut self, mode: impl Into<String>) -> Self {
                self.opts.mode = Some(mode.into());
                self
            }

            /// Include each element's bounding box.
            pub fn boxes(mut self, boxes: bool) -> Self {
                self.opts.boxes = Some(boxes);
                self
            }

            fn snapshot_options(&self) -> SnapshotOptions {
                SnapshotOptions {
                    mode: self.opts.mode.clone(),
                    depth: self.opts.depth,
                    boxes: self.opts.boxes,
                }
            }
        }
    };
}

snapshot_options!(kinds::AriaSnapshot);
snapshot_options!(kinds::AriaSnapshotJson);

/// `Frame.ariaSnapshotJSON` with a selector: auto-waiting for the element
/// unless in `"ai"` mode, which snapshots iframes too.
async fn locator_json<K>(action: Action<K>, options: SnapshotOptions) -> Result<Value> {
    if options.is_ai() {
        let api = action.api;
        let Target::Selector {
            frame, selector, ..
        } = action.target.clone()
        else {
            let message = match &action.target {
                Target::Invalid(m) => m.clone(),
                _ => "aria snapshots require a locator".to_string(),
            };
            return Err(Error::invalid_argument(message).with_api(api));
        };
        let timeout = match frame.page_inner() {
            Some(page) => page.timeouts.timeout(action.opts.timeout),
            None => action.opts.timeout.unwrap_or(DEFAULT_TIMEOUT),
        };
        let progress = Progress::new(api, timeout);
        return progress
            .run(snapshot_json_for_frame(
                progress.clone(),
                frame,
                Some(selector),
                options,
                None,
            ))
            .await;
    }
    let task = Task::AriaSnapshot {
        depth: options.depth,
        mode: Some("default".into()),
        boxes: options.boxes,
        json: true,
    };
    match run_task(action, task, false, false).await? {
        TaskOutput::Value(v) => from_js_value(v).map_err(Error::serialization),
        _ => Ok(json!([])),
    }
}

impl IntoFuture for Action<kinds::AriaSnapshot> {
    type Output = Result<String>;
    type IntoFuture = BoxFuture<Result<String>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let options = self.snapshot_options();
            if options.is_ai() {
                return locator_json(self, options).await.map(|j| render_yaml(&j));
            }
            let task = Task::AriaSnapshot {
                depth: options.depth,
                mode: options.mode.clone(),
                boxes: options.boxes,
                json: false,
            };
            match run_task(self, task, false, false).await? {
                TaskOutput::Text(s) => Ok(s),
                _ => Ok(String::new()),
            }
        })
    }
}

impl IntoFuture for Action<kinds::AriaSnapshotJson> {
    type Output = Result<Value>;
    type IntoFuture = BoxFuture<Result<Value>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let options = self.snapshot_options();
            locator_json(self, options).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_like_playwright() {
        let json = json!([
            { "role": "heading", "name": "title", "level": 1 },
            { "role": "list", "name": "my list", "children": [
                { "role": "listitem", "text": "one" },
                { "role": "listitem", "text": "two: three" },
            ]},
            { "role": "link", "name": "Link", "url": "https://example.com/" },
            { "role": "paragraph", "children": ["Hello", { "role": "button", "name": "b", "box": { "x": 1, "y": 2, "width": 3, "height": 4 } }] },
            { "role": "text", "text": "123" },
            { "role": "checkbox", "name": "c", "checked": "mixed", "ref": "e2", "cursor": "pointer" },
        ]);
        assert_eq!(
            render_yaml(&json),
            [
                "- heading \"title\" [level=1]",
                "- list \"my list\":",
                "  - listitem: one",
                "  - listitem: \"two: three\"",
                "- link \"Link\":",
                "  - /url: https://example.com/",
                "- paragraph:",
                "  - text: Hello",
                "  - button \"b\" [box=1,2,3,4]",
                "- text: \"123\"",
                "- checkbox \"c\" [checked=mixed] [ref=e2] [cursor=pointer]",
            ]
            .join("\n")
        );
    }
}
