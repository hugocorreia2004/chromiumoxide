//! `toMatchAriaSnapshot` (`matchers/toMatchAriaSnapshot.ts`).
//!
//! Playwright parses the YAML template on the server with the `yaml`
//! package and hands the resulting `AriaTemplateNode` to the injected
//! script. Here the YAML is parsed in Rust (`yaml-rust2`, with source
//! positions) and the tree is given to the injected script's own
//! `parseAriaSnapshot` through a small stand-in for the `yaml` API
//! (`Scalar`, `YAMLMap`, `YAMLSeq`, `LineCounter`, `parseDocument`), so the
//! template semantics and error messages of the key parser are Playwright's.
//! YAML syntax errors use `yaml-rust2`'s wording.

use std::future::{Future, IntoFuture};
use std::pin::Pin;

use serde_json::{Value, json};
use yaml_rust2::Yaml;
use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser};
use yaml_rust2::scanner::{Marker, TScalarStyle};

use super::engine::{ExpectParams, frame_expect};
use super::format::{MessageDetails, format_matcher_message, print_diff_or_stringify, stringify};
use super::{Config, MatcherResult};
use crate::error::Result;
use crate::frame::Frame;
use crate::frame_selectors::{CallArg, call_injected, value_of};
use crate::js::JsValue;
use crate::locator::Locator;
use crate::page::Page;

/// `unshift`: remove the common indentation and blank lines.
fn unshift(snapshot: &str) -> String {
    let lines: Vec<&str> = snapshot.split('\n').collect();
    let mut prefix = 100;
    for line in &lines {
        if line.trim().is_empty() {
            continue;
        }
        let ws = line.len() - line.trim_start().len();
        prefix = prefix.min(ws);
    }
    lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.get(prefix..).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Builds a JSON tree of the YAML document with UTF-16 offsets.
struct TreeBuilder {
    /// UTF-16 offset of each char index.
    utf16: Vec<usize>,
    stack: Vec<(Value, Option<Value>)>,
    root: Option<Value>,
    documents: usize,
}

impl TreeBuilder {
    fn offset(&self, mark: &Marker) -> usize {
        let i = mark.index();
        self.utf16
            .get(i)
            .copied()
            .unwrap_or_else(|| *self.utf16.last().unwrap_or(&0))
    }

    fn push_node(&mut self, node: Value) {
        match self.stack.last_mut() {
            None => {
                if self.root.is_none() {
                    self.root = Some(node);
                }
            }
            Some((container, pending_key)) => {
                if container["t"] == "seq" {
                    container["items"].as_array_mut().unwrap().push(node);
                } else if let Some(key) = pending_key.take() {
                    container["pairs"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({ "k": key, "v": node }));
                } else {
                    *pending_key = Some(node);
                }
            }
        }
    }
}

impl MarkedEventReceiver for TreeBuilder {
    fn on_event(&mut self, ev: Event, mark: Marker) {
        let pos = self.offset(&mark);
        match ev {
            Event::DocumentStart => self.documents += 1,
            Event::Scalar(value, style, _, _) => {
                let end = pos + value.encode_utf16().count();
                let v = if style == TScalarStyle::Plain {
                    match Yaml::from_str(&value) {
                        Yaml::Integer(i) => json!(i),
                        Yaml::Real(r) => r.parse::<f64>().map(|f| json!(f)).unwrap_or(json!(r)),
                        Yaml::Boolean(b) => json!(b),
                        Yaml::Null => Value::Null,
                        _ => Value::String(value),
                    }
                } else {
                    Value::String(value)
                };
                self.push_node(json!({ "t": "scalar", "v": v, "r": [pos, end, end] }));
            }
            Event::SequenceStart(..) => self.stack.push((
                json!({ "t": "seq", "items": [], "r": [pos, pos, pos] }),
                None,
            )),
            Event::MappingStart(..) => self.stack.push((
                json!({ "t": "map", "pairs": [], "r": [pos, pos, pos] }),
                None,
            )),
            Event::SequenceEnd | Event::MappingEnd => {
                if let Some((mut node, pending)) = self.stack.pop() {
                    if let Some(key) = pending {
                        node["pairs"]
                            .as_array_mut()
                            .unwrap()
                            .push(json!({ "k": key, "v": { "t": "scalar", "v": null, "r": [pos, pos, pos] } }));
                    }
                    node["r"][1] = json!(pos);
                    node["r"][2] = json!(pos);
                    self.push_node(node);
                }
            }
            Event::Alias(_) => {
                self.push_node(json!({ "t": "alias", "r": [pos, pos, pos] }));
            }
            _ => {}
        }
    }
}

/// Parses `text` into the JSON document handed to the JavaScript stand-in.
fn parse_yaml(text: &str) -> Value {
    let mut utf16 = Vec::with_capacity(text.len() + 1);
    let mut offset = 0;
    for c in text.chars() {
        utf16.push(offset);
        offset += c.len_utf16();
    }
    utf16.push(offset);
    let mut builder = TreeBuilder {
        utf16,
        stack: Vec::new(),
        root: None,
        documents: 0,
    };
    let mut parser = Parser::new_from_str(text);
    let result = parser.load(&mut builder, true);
    let mut errors = Vec::new();
    if let Err(e) = result {
        let pos = builder.offset(e.marker());
        errors.push(json!({ "message": e.to_string(), "pos": [pos, pos + 1] }));
    } else if builder.documents > 1 {
        errors.push(json!({
            "message": "Source contains multiple documents; please use YAML.parseAllDocuments()",
            "pos": [0, 0],
        }));
    }
    json!({ "errors": errors, "contents": builder.root })
}

const PARSE_JS: &str = r#"(injected, doc, text) => {
  class Scalar { constructor(n) { this.value = n.v; this.range = n.r; } }
  class YAMLMap { constructor(n) { this.items = n.pairs.map(p => ({ key: build(p.k), value: build(p.v) })); this.range = n.r; } }
  class YAMLSeq { constructor(n) { this.items = n.items.map(build); this.range = n.r; } }
  class Alias { constructor(n) { this.range = n.r; } }
  function build(n) {
    if (!n) return null;
    if (n.t === 'scalar') return new Scalar(n);
    if (n.t === 'map') return new YAMLMap(n);
    if (n.t === 'seq') return new YAMLSeq(n);
    return new Alias(n);
  }
  class LineCounter {
    constructor() { this.lineStarts = []; }
    addNewLine(offset) { this.lineStarts.push(offset); }
    linePos(offset) {
      let line = 0;
      while (line + 1 < this.lineStarts.length && this.lineStarts[line + 1] <= offset) line++;
      return { line: line + 1, col: offset - this.lineStarts[line] + 1 };
    }
  }
  const yaml = {
    Scalar, YAMLMap, YAMLSeq, LineCounter,
    parseDocument(source, options) {
      const counter = options.lineCounter;
      counter.addNewLine(0);
      for (let i = 0; i < source.length; i++) {
        if (source[i] === '\n') counter.addNewLine(i + 1);
      }
      return { errors: doc.errors, contents: build(doc.contents) };
    },
  };
  const result = injected.utils.parseAriaSnapshot(yaml, text);
  if (result.errors.length)
    return { error: result.errors[0].message };
  return { fragment: result.fragment };
}"#;

/// `parseAriaSnapshotUnsafe`: the template, or the parse error message.
async fn parse_template(frame: &Frame, text: &str) -> Result<std::result::Result<JsValue, String>> {
    let doc = parse_yaml(text);
    let ctx = frame.utility_context().await?;
    let result = value_of(
        call_injected(
            &ctx,
            PARSE_JS,
            vec![
                CallArg::Json(doc),
                CallArg::Json(Value::String(text.to_string())),
            ],
            true,
        )
        .await?,
    );
    if let Some(error) = result.get("error").and_then(|e| e.as_str()) {
        return Ok(Err(error.to_string()));
    }
    Ok(Ok(result.get("fragment").cloned().unwrap_or_default()))
}

#[derive(Debug, Clone)]
pub(super) enum Receiver {
    Locator(Locator),
    Page(Page),
}

/// `expect(..).to_match_aria_snapshot(..)`.
#[must_use = "assertions do nothing unless awaited"]
#[derive(Debug, Clone)]
pub struct ToMatchAriaSnapshot {
    receiver: Receiver,
    cfg: Config,
    expected: String,
    timeout: Option<u64>,
    children: Option<String>,
}

impl ToMatchAriaSnapshot {
    pub(super) fn new(receiver: Receiver, cfg: Config, expected: String) -> Self {
        ToMatchAriaSnapshot {
            receiver,
            cfg,
            expected,
            timeout: None,
            children: None,
        }
    }

    /// Timeout of this assertion in milliseconds.
    pub fn timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout = Some(timeout_ms);
        self
    }

    /// Default matching mode of children when the template has no
    /// `/children` property: `"contain"`, `"equal"` or `"deep-equal"`
    /// (the `toMatchAriaSnapshot.children` configuration).
    pub fn children(mut self, mode: &str) -> Self {
        self.children = Some(mode.to_string());
        self
    }

    async fn run(self) -> Result<()> {
        let cfg = &self.cfg;
        let mut expected = unshift(&self.expected);
        if let Some(children) = &self.children {
            let has_children = expected.lines().any(|l| l.starts_with("- /children:"));
            if !has_children {
                expected = format!("- /children: {children}\n{expected}");
            }
        }
        let (frame, selector, locator_text) = match &self.receiver {
            Receiver::Locator(l) => {
                if let Some(e) = l.construction_error() {
                    return Err(crate::Error::invalid_argument(e));
                }
                (
                    l.frame().clone(),
                    Some(l.selector().to_string()),
                    Some(l.to_string()),
                )
            }
            Receiver::Page(p) => (p.main_frame(), None, None),
        };
        let timeout = cfg.timeout(self.timeout);
        let name = "toMatchAriaSnapshot";
        let outcome = match parse_template(&frame, &expected).await? {
            Err(message) => {
                let subtitle = selector
                    .as_deref()
                    .map(|s| format!(" {}", crate::frame_selectors::describe_selector(s)))
                    .unwrap_or_default();
                let with_timeout = if timeout > 0 {
                    format!(" with timeout {timeout}ms")
                } else {
                    String::new()
                };
                super::engine::ExpectOutcome {
                    matches: cfg.is_not,
                    error_message: Some(format!("Error: {message}")),
                    log: crate::error::compress_call_log(&[format!(
                        "{}{subtitle}{with_timeout}",
                        cfg.title(name)
                    )]),
                    ..Default::default()
                }
            }
            Ok(template) => {
                let mut params = ExpectParams::new("to.match.aria");
                params.selector = selector;
                params.is_not = cfg.is_not;
                params.expected_value = Some(template);
                frame_expect(&frame, params, timeout, &cfg.title(name)).await
            }
        };
        let pass = outcome.matches;
        if pass != cfg.is_not {
            return Ok(());
        }
        let raw = outcome
            .received
            .as_ref()
            .and_then(|r| r.get("raw"))
            .and_then(|r| r.as_str())
            .map(str::to_string);
        let expected_js = JsValue::String(expected.clone());
        let mut details = MessageDetails {
            is_not: cfg.is_not,
            matcher_name: name.to_string(),
            expectation: "expected".to_string(),
            locator: locator_text,
            timed_out: outcome.timed_out,
            timeout: Some(timeout),
            error_message: outcome.error_message.clone(),
            log: outcome.log.clone(),
            ..Default::default()
        };
        if outcome.error_message.is_some() {
            details.printed_expected = Some(format!(
                "Expected: {}{}",
                if cfg.is_not { "not " } else { "" },
                stringify(&expected_js)
            ));
        } else if pass {
            details.printed_expected = Some(format!("Expected: not {}", stringify(&expected_js)));
            details.printed_received = Some(format!(
                "Received: {}",
                stringify(&JsValue::String(raw.clone().unwrap_or_default()))
            ));
        } else {
            details.printed_diff = Some(print_diff_or_stringify(
                &expected_js,
                &JsValue::String(raw.clone().unwrap_or_default()),
                "Expected",
                "Received",
            ));
        }
        let message = format_matcher_message(details);
        Err(cfg.fail(MatcherResult {
            name: name.to_string(),
            pass,
            expected: Some(expected_js),
            actual: raw.map(JsValue::String),
            message,
            log: outcome.log,
            timeout: outcome.timed_out.then_some(timeout),
            aria_snapshot: None,
        }))
    }
}

impl IntoFuture for ToMatchAriaSnapshot {
    type Output = Result<()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Result<()>> + Send>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unshift_removes_common_indentation() {
        assert_eq!(
            unshift("\n    - list\n      - item\n\n"),
            "- list\n  - item"
        );
    }

    #[test]
    fn yaml_tree() {
        let doc = parse_yaml("- heading \"title\" [level=1]\n- list:\n  - listitem: one\n");
        assert_eq!(doc["errors"], json!([]));
        let contents = &doc["contents"];
        assert_eq!(contents["t"], "seq");
        assert_eq!(contents["items"][0]["v"], "heading \"title\" [level=1]");
        assert_eq!(contents["items"][1]["t"], "map");
        assert_eq!(contents["items"][1]["pairs"][0]["k"]["v"], "list");
        assert_eq!(contents["items"][1]["pairs"][0]["v"]["t"], "seq");
    }
}
