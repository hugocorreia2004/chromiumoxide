//! Selector parsing, ported from Playwright's `isomorphic/selectorParser.ts`.
//!
//! The CSS parser is not ported: `css` parts keep their source text as body
//! (the injected script parses CSS when it queries). This parser is used to
//! render selectors as locators and to split selectors by frame.

#![allow(dead_code)]

use std::fmt;

use serde_json::Value;

use crate::locator_utils::JsRegex;

/// An invalid selector, with Playwright's message.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct InvalidSelectorError(pub(crate) String);

impl fmt::Display for InvalidSelectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidSelectorError {}

type Result<T> = std::result::Result<T, InvalidSelectorError>;

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(InvalidSelectorError(message.into()))
}

const NESTED_SELECTOR_NAMES: &[&str] = &[
    "internal:has",
    "internal:has-not",
    "internal:and",
    "internal:or",
    "internal:chain",
    "left-of",
    "right-of",
    "above",
    "below",
    "near",
];
const NESTED_SELECTOR_NAMES_WITH_DISTANCE: &[&str] =
    &["left-of", "right-of", "above", "below", "near"];

pub(crate) fn is_nested_selector_name(name: &str) -> bool {
    NESTED_SELECTOR_NAMES.contains(&name)
}

/// Body of a selector part.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PartBody {
    /// Engine body text (for `css`, the CSS source).
    Text(String),
    /// Nested selector of `internal:has`, `internal:and`, ...
    Nested {
        parsed: ParsedSelector,
        distance: Option<f64>,
    },
}

impl PartBody {
    pub(crate) fn as_text(&self) -> Option<&str> {
        match self {
            PartBody::Text(s) => Some(s),
            PartBody::Nested { .. } => None,
        }
    }
}

/// One `>>`-separated part of a selector.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedSelectorPart {
    pub(crate) name: String,
    pub(crate) body: PartBody,
    pub(crate) source: String,
}

impl ParsedSelectorPart {
    fn is_control(&self, body: &str) -> bool {
        self.name == "internal:control" && self.body.as_text() == Some(body)
    }
}

/// A parsed selector.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct ParsedSelector {
    pub(crate) parts: Vec<ParsedSelectorPart>,
    pub(crate) capture: Option<usize>,
}

struct StringPart {
    name: String,
    body: String,
}

/// `parseSelector`.
pub(crate) fn parse_selector(selector: &str) -> Result<ParsedSelector> {
    let (string_parts, capture) = parse_selector_string(selector)?;
    let mut parts: Vec<ParsedSelectorPart> = Vec::new();
    for part in string_parts {
        if part.name == "css" || part.name == "css:light" {
            let body = if part.name == "css:light" {
                format!(":light({})", part.body)
            } else {
                part.body
            };
            parts.push(ParsedSelectorPart {
                name: "css".to_string(),
                body: PartBody::Text(body.clone()),
                source: body,
            });
            continue;
        }
        if is_nested_selector_name(&part.name) {
            let malformed = || format!("Malformed selector: {}={}", part.name, part.body);
            let unescaped: Vec<Value> = match serde_json::from_str(&format!("[{}]", part.body)) {
                Ok(v) => v,
                Err(_) => return invalid(malformed()),
            };
            if unescaped.is_empty() || unescaped.len() > 2 {
                return invalid(malformed());
            }
            let inner_selector = match &unescaped[0] {
                Value::String(s) => s.clone(),
                _ => return invalid(malformed()),
            };
            let mut distance = None;
            if unescaped.len() == 2 {
                match &unescaped[1] {
                    Value::Number(n)
                        if NESTED_SELECTOR_NAMES_WITH_DISTANCE.contains(&part.name.as_str()) =>
                    {
                        distance = n.as_f64();
                    }
                    _ => return invalid(malformed()),
                }
            }
            let mut parsed = parse_selector(&inner_selector)?;
            let last_frame_index = parsed
                .parts
                .iter()
                .rposition(|p| p.is_control("enter-frame"));
            // The "any-frame" token applies to the whole selector.
            let outer_parts: &[ParsedSelectorPart] =
                if parts.first().is_some_and(|p| p.is_control("any-frame")) {
                    &parts[1..]
                } else {
                    &parts[..]
                };
            if let Some(index) = last_frame_index {
                let outer_end = (index + 1).min(outer_parts.len());
                if selector_parts_equal(&parsed.parts[..=index], &outer_parts[..outer_end]) {
                    parsed.parts.drain(..=index);
                    if let Some(capture) = parsed.capture {
                        if capture <= index {
                            return invalid(
                                "Can not capture the selector before diving into the frame. Only use * after the last frame has been selected",
                            );
                        }
                        parsed.capture = Some(capture - (index + 1));
                    }
                }
            }
            parts.push(ParsedSelectorPart {
                name: part.name,
                source: part.body,
                body: PartBody::Nested { parsed, distance },
            });
            continue;
        }
        parts.push(ParsedSelectorPart {
            name: part.name,
            source: part.body.clone(),
            body: PartBody::Text(part.body),
        });
    }
    if let Some(first) = parts.first() {
        if is_nested_selector_name(&first.name) {
            return invalid(format!("\"{}\" selector cannot be first", first.name));
        }
    }
    Ok(ParsedSelector { parts, capture })
}

/// Matches in any frame of the subtree. Only allowed as the first token.
pub(crate) const ANY_FRAME_SELECTOR: &str = "internal:control=any-frame";

/// `splitSelectorByFrame`: per-frame chunks separated by `enter-frame`
/// boundaries, and whether the selector starts with `any-frame`.
pub(crate) fn split_selector_by_frame(selector_text: &str) -> Result<(bool, Vec<ParsedSelector>)> {
    let selector = parse_selector(selector_text)?;
    let mut chunks = Vec::new();
    let mut chunk = ParsedSelector::default();
    let mut any_frame = false;
    let mut chunk_start_index = 0;
    for (i, part) in selector.parts.iter().enumerate() {
        if part.is_control("any-frame") {
            if i != 0 {
                return invalid(format!(
                    "\"any-frame\" is only allowed as the first selector token, while parsing selector {selector_text}"
                ));
            }
            any_frame = true;
            chunk_start_index = i + 1;
            continue;
        }
        if part.is_control("enter-frame") {
            match chunk.parts.last() {
                None => {
                    return invalid(
                        "Selector cannot start with entering frame, select the iframe first",
                    );
                }
                Some(last) if last.is_control("enter-frame") => {
                    return invalid(
                        "Selector cannot start with entering frame, select the iframe first",
                    );
                }
                _ => {}
            }
            chunks.push(std::mem::take(&mut chunk));
            chunk_start_index = i + 1;
            continue;
        }
        if selector.capture == Some(i) {
            chunk.capture = Some(i - chunk_start_index);
        }
        chunk.parts.push(part.clone());
    }
    if chunk.parts.is_empty() {
        if any_frame && chunks.is_empty() {
            return invalid(format!(
                "Selector cannot be empty after frameLocator(), while parsing selector {selector_text}"
            ));
        }
        return invalid(format!(
            "Selector cannot end with entering frame, while parsing selector {selector_text}"
        ));
    }
    if chunk
        .parts
        .last()
        .is_some_and(|p| p.is_control("enter-frame"))
    {
        return invalid(format!(
            "Selector cannot end with entering frame, while parsing selector {selector_text}"
        ));
    }
    let last_has_capture = chunk.capture.is_some();
    chunks.push(chunk);
    if selector.capture.is_some() && !last_has_capture {
        return invalid(
            "Can not capture the selector before diving into the frame. Only use * after the last frame has been selected",
        );
    }
    Ok((any_frame, chunks))
}

fn selector_parts_equal(list1: &[ParsedSelectorPart], list2: &[ParsedSelectorPart]) -> bool {
    let a = ParsedSelector {
        parts: list1.to_vec(),
        capture: None,
    };
    let b = ParsedSelector {
        parts: list2.to_vec(),
        capture: None,
    };
    stringify_selector(&a, false) == stringify_selector(&b, false)
}

/// `stringifySelector`.
pub(crate) fn stringify_selector(selector: &ParsedSelector, force_engine_name: bool) -> String {
    selector
        .parts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let implicit_engine = p.name == "css"
                || (p.name == "xpath"
                    && (p.source.starts_with("//") || p.source.starts_with("..")));
            let include_engine =
                force_engine_name || Some(i) == selector.capture || !implicit_engine;
            let prefix = if include_engine {
                format!("{}=", p.name)
            } else {
                String::new()
            };
            let capture = if Some(i) == selector.capture { "*" } else { "" };
            format!("{capture}{prefix}{}", p.source)
        })
        .collect::<Vec<_>>()
        .join(" >> ")
}

/// `visitAllSelectorParts`: `visitor(part, nested)`.
pub(crate) fn visit_all_selector_parts(
    selector: &ParsedSelector,
    visitor: &mut dyn FnMut(&ParsedSelectorPart, bool),
) {
    fn visit(
        selector: &ParsedSelector,
        nested: bool,
        visitor: &mut dyn FnMut(&ParsedSelectorPart, bool),
    ) {
        for part in &selector.parts {
            visitor(part, nested);
            if let PartBody::Nested { parsed, .. } = &part.body {
                visit(parsed, true, visitor);
            }
        }
    }
    visit(selector, false, visitor);
}

fn is_engine_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | ':' | '*'))
}

fn parse_selector_string(selector: &str) -> Result<(Vec<StringPart>, Option<usize>)> {
    let chars: Vec<char> = selector.chars().collect();
    let mut parts: Vec<StringPart> = Vec::new();
    let mut capture: Option<usize> = None;

    let mut append = |start: usize, index: usize| -> Result<()> {
        let part: String = chars[start..index].iter().collect();
        let part = part.trim();
        let part_chars: Vec<char> = part.chars().collect();
        let eq_index = part.find('=');
        let (mut name, body) = match eq_index {
            Some(eq) if is_engine_name(part[..eq].trim()) => {
                (part[..eq].trim().to_string(), part[eq + 1..].to_string())
            }
            _ => {
                let quoted = |q: char| {
                    part_chars.len() > 1
                        && part_chars[0] == q
                        && part_chars[part_chars.len() - 1] == q
                };
                if quoted('"') || quoted('\'') {
                    ("text".to_string(), part.to_string())
                } else if is_xpath_start(part) {
                    ("xpath".to_string(), part.to_string())
                } else {
                    ("css".to_string(), part.to_string())
                }
            }
        };
        let mut is_capture = false;
        if let Some(rest) = name.strip_prefix('*') {
            is_capture = true;
            name = rest.to_string();
        }
        parts.push(StringPart { name, body });
        if is_capture {
            if capture.is_some() {
                return invalid("Only one of the selectors can capture using * modifier");
            }
            capture = Some(parts.len() - 1);
        }
        Ok(())
    };

    if !selector.contains(">>") {
        append(0, chars.len())?;
        return Ok((parts, capture));
    }

    let should_ignore_text_selector_quote = |start: usize, index: usize| -> bool {
        let prefix: String = chars[start..index].iter().collect();
        // /^\s*text\s*=(.*)$/ with some text after "=" (no line terminators).
        let Some(rest) = prefix.trim_start().strip_prefix("text") else {
            return false;
        };
        let Some(rest) = rest.trim_start().strip_prefix('=') else {
            return false;
        };
        !rest.is_empty() && !rest.contains(['\n', '\r', '\u{2028}', '\u{2029}'])
    };

    let mut index = 0;
    let mut start = 0;
    let mut quote: Option<char> = None;
    while index < chars.len() {
        let c = chars[index];
        if c == '\\' && index + 1 < chars.len() {
            index += 2;
        } else if Some(c) == quote {
            quote = None;
            index += 1;
        } else if quote.is_none()
            && (c == '"' || c == '\'' || c == '`')
            && !should_ignore_text_selector_quote(start, index)
        {
            quote = Some(c);
            index += 1;
        } else if quote.is_none() && c == '>' && chars.get(index + 1) == Some(&'>') {
            append(start, index)?;
            index += 2;
            start = index;
        } else {
            index += 1;
        }
    }
    append(start, chars.len())?;
    Ok((parts, capture))
}

/// `/^\(*\/\//` or starts with `..`.
fn is_xpath_start(part: &str) -> bool {
    part.trim_start_matches('(').starts_with("//") || part.starts_with("..")
}

// ----------------------------------------------------------------------
// Attribute selectors
// ----------------------------------------------------------------------

/// Operator of an attribute selector part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttributeSelectorOperator {
    Truthy,
    Equal,
    Contains,
    DashMatch,
    StartsWith,
    EndsWith,
    Includes,
}

/// Value of an attribute selector part.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AttributeValue {
    Null,
    String(String),
    Bool(bool),
    Number(f64),
    Regex(JsRegex),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AttributeSelectorPart {
    pub(crate) name: String,
    pub(crate) json_path: Vec<String>,
    pub(crate) op: AttributeSelectorOperator,
    pub(crate) value: AttributeValue,
    pub(crate) case_sensitive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AttributeSelector {
    pub(crate) name: String,
    pub(crate) attributes: Vec<AttributeSelectorPart>,
}

/// JavaScript's `+string` for the characters an unquoted value may contain.
pub(crate) fn js_to_number(s: &str) -> f64 {
    let t = s.trim();
    if t.is_empty() {
        return 0.0;
    }
    let lower = t.to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix("0x") {
        return i64::from_str_radix(hex, 16)
            .map(|v| v as f64)
            .unwrap_or(f64::NAN);
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    if t.chars()
        .any(|c| c.is_ascii_alphabetic() && c != 'e' && c != 'E')
    {
        return f64::NAN;
    }
    t.parse::<f64>().unwrap_or(f64::NAN)
}

struct AttrParser<'a> {
    selector: &'a str,
    chars: Vec<char>,
    wp: usize,
}

impl AttrParser<'_> {
    fn eol(&self) -> bool {
        self.wp >= self.chars.len()
    }

    fn next(&self) -> Option<char> {
        self.chars.get(self.wp).copied()
    }

    fn next_is(&self, c: char) -> bool {
        self.next() == Some(c)
    }

    fn eat1(&mut self) -> String {
        let c = self.next().map(String::from).unwrap_or_default();
        self.wp += 1;
        c
    }

    fn syntax_error<T>(&self, stage: Option<&str>) -> Result<T> {
        if self.eol() {
            return invalid(format!(
                "Unexpected end of selector while parsing selector `{}`",
                self.selector
            ));
        }
        invalid(format!(
            "Error while parsing selector `{}` - unexpected symbol \"{}\" at position {}{}",
            self.selector,
            self.next().map(String::from).unwrap_or_default(),
            self.wp,
            stage.map(|s| format!(" during {s}")).unwrap_or_default()
        ))
    }

    fn skip_spaces(&mut self) {
        while !self.eol() && self.next().is_some_and(char::is_whitespace) {
            self.wp += 1;
        }
    }

    fn is_css_name_char(c: char) -> bool {
        c >= '\u{0080}' || c.is_ascii_alphanumeric() || c == '_' || c == '-'
    }

    fn read_identifier(&mut self) -> String {
        let mut result = String::new();
        self.skip_spaces();
        while !self.eol() && self.next().is_some_and(Self::is_css_name_char) {
            result.push_str(&self.eat1());
        }
        result
    }

    fn read_quoted_string(&mut self, quote: char) -> Result<String> {
        let mut result = self.eat1();
        if result != quote.to_string() {
            return self.syntax_error(Some("parsing quoted string"));
        }
        while !self.eol() && !self.next_is(quote) {
            if self.next_is('\\') {
                self.eat1();
            }
            result.push_str(&self.eat1());
        }
        if !self.next_is(quote) {
            return self.syntax_error(Some("parsing quoted string"));
        }
        result.push_str(&self.eat1());
        Ok(result)
    }

    fn read_regular_expression(&mut self) -> Result<JsRegex> {
        if self.eat1() != "/" {
            return self.syntax_error(Some("parsing regular expression"));
        }
        let mut source = String::new();
        let mut in_class = false;
        while !self.eol() {
            if self.next_is('\\') {
                source.push_str(&self.eat1());
                if self.eol() {
                    return self.syntax_error(Some("parsing regular expression"));
                }
            } else if in_class && self.next_is(']') {
                in_class = false;
            } else if !in_class && self.next_is('[') {
                in_class = true;
            } else if !in_class && self.next_is('/') {
                break;
            }
            source.push_str(&self.eat1());
        }
        if self.eat1() != "/" {
            return self.syntax_error(Some("parsing regular expression"));
        }
        let mut flags = String::new();
        while !self.eol() && self.next().is_some_and(|c| "dgimsuvy".contains(c)) {
            flags.push_str(&self.eat1());
        }
        Ok(JsRegex::new(source, flags))
    }

    fn read_attribute_token(&mut self) -> Result<String> {
        self.skip_spaces();
        let token = match self.next() {
            Some(q @ ('\'' | '"')) => {
                let s = self.read_quoted_string(q)?;
                let chars: Vec<char> = s.chars().collect();
                chars[1..chars.len() - 1].iter().collect()
            }
            _ => self.read_identifier(),
        };
        if token.is_empty() {
            return self.syntax_error(Some("parsing property path"));
        }
        Ok(token)
    }

    fn read_operator(&mut self) -> Result<AttributeSelectorOperator> {
        self.skip_spaces();
        let mut op = String::new();
        if !self.eol() {
            op.push_str(&self.eat1());
        }
        if !self.eol() && op != "=" {
            op.push_str(&self.eat1());
        }
        Ok(match op.as_str() {
            "=" => AttributeSelectorOperator::Equal,
            "*=" => AttributeSelectorOperator::Contains,
            "^=" => AttributeSelectorOperator::StartsWith,
            "$=" => AttributeSelectorOperator::EndsWith,
            "|=" => AttributeSelectorOperator::DashMatch,
            "~=" => AttributeSelectorOperator::Includes,
            _ => return self.syntax_error(Some("parsing operator")),
        })
    }

    fn read_attribute(&mut self, allow_unquoted: bool) -> Result<AttributeSelectorPart> {
        // skip leading [
        self.eat1();
        let mut json_path = vec![self.read_attribute_token()?];
        self.skip_spaces();
        while self.next_is('.') {
            self.eat1();
            json_path.push(self.read_attribute_token()?);
            self.skip_spaces();
        }
        if self.next_is(']') {
            self.eat1();
            return Ok(AttributeSelectorPart {
                name: json_path.join("."),
                json_path,
                op: AttributeSelectorOperator::Truthy,
                value: AttributeValue::Null,
                case_sensitive: false,
            });
        }
        let operator = self.read_operator()?;
        let value;
        let mut case_sensitive = true;
        self.skip_spaces();
        if self.next_is('/') {
            if operator != AttributeSelectorOperator::Equal {
                return invalid(format!(
                    "Error while parsing selector `{}` - cannot use {} in attribute with regular expression",
                    self.selector,
                    operator_str(operator)
                ));
            }
            value = AttributeValue::Regex(self.read_regular_expression()?);
        } else if let Some(q @ ('\'' | '"')) = self.next() {
            let s = self.read_quoted_string(q)?;
            let chars: Vec<char> = s.chars().collect();
            value = AttributeValue::String(chars[1..chars.len() - 1].iter().collect());
            self.skip_spaces();
            if self.next_is('i') || self.next_is('I') {
                case_sensitive = false;
                self.eat1();
            } else if self.next_is('s') || self.next_is('S') {
                case_sensitive = true;
                self.eat1();
            }
        } else {
            let mut raw = String::new();
            while !self.eol()
                && self
                    .next()
                    .is_some_and(|c| Self::is_css_name_char(c) || c == '+' || c == '.')
            {
                raw.push_str(&self.eat1());
            }
            value = if raw == "true" {
                AttributeValue::Bool(true)
            } else if raw == "false" {
                AttributeValue::Bool(false)
            } else if !allow_unquoted {
                let n = js_to_number(&raw);
                if n.is_nan() {
                    return self.syntax_error(Some("parsing attribute value"));
                }
                AttributeValue::Number(n)
            } else {
                AttributeValue::String(raw)
            };
        }
        self.skip_spaces();
        if !self.next_is(']') {
            return self.syntax_error(Some("parsing attribute value"));
        }
        self.eat1();
        if operator != AttributeSelectorOperator::Equal
            && !matches!(value, AttributeValue::String(_))
        {
            let rendered = match &value {
                AttributeValue::Bool(b) => b.to_string(),
                AttributeValue::Number(n) => crate::js::value::format_js_number(*n),
                AttributeValue::Regex(re) => re.to_string(),
                AttributeValue::Null => "null".to_string(),
                AttributeValue::String(s) => s.clone(),
            };
            return invalid(format!(
                "Error while parsing selector `{}` - cannot use {} in attribute with non-string matching value - {rendered}",
                self.selector,
                operator_str(operator)
            ));
        }
        Ok(AttributeSelectorPart {
            name: json_path.join("."),
            json_path,
            op: operator,
            value,
            case_sensitive,
        })
    }
}

fn operator_str(op: AttributeSelectorOperator) -> &'static str {
    match op {
        AttributeSelectorOperator::Truthy => "<truthy>",
        AttributeSelectorOperator::Equal => "=",
        AttributeSelectorOperator::Contains => "*=",
        AttributeSelectorOperator::DashMatch => "|=",
        AttributeSelectorOperator::StartsWith => "^=",
        AttributeSelectorOperator::EndsWith => "$=",
        AttributeSelectorOperator::Includes => "~=",
    }
}

/// `parseAttributeSelector`.
pub(crate) fn parse_attribute_selector(
    selector: &str,
    allow_unquoted_strings: bool,
) -> Result<AttributeSelector> {
    let mut p = AttrParser {
        selector,
        chars: selector.chars().collect(),
        wp: 0,
    };
    let name = p.read_identifier();
    let mut attributes = Vec::new();
    p.skip_spaces();
    while p.next_is('[') {
        attributes.push(p.read_attribute(allow_unquoted_strings)?);
        p.skip_spaces();
    }
    if !p.eol() {
        return p.syntax_error(None);
    }
    if name.is_empty() && attributes.is_empty() {
        return invalid(format!(
            "Error while parsing selector `{selector}` - selector cannot be empty"
        ));
    }
    Ok(AttributeSelector { name, attributes })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(s: &str) -> Vec<(String, String)> {
        parse_selector(s)
            .unwrap()
            .parts
            .into_iter()
            .map(|p| (p.name, p.source))
            .collect()
    }

    fn pair(a: &str, b: &str) -> (String, String) {
        (a.to_string(), b.to_string())
    }

    #[test]
    fn parses_engines() {
        assert_eq!(names("div"), vec![pair("css", "div")]);
        assert_eq!(names("//div"), vec![pair("xpath", "//div")]);
        assert_eq!(names("((//div"), vec![pair("xpath", "((//div")]);
        assert_eq!(names(".."), vec![pair("xpath", "..")]);
        assert_eq!(names("\"foo\""), vec![pair("text", "\"foo\"")]);
        assert_eq!(names("'foo'"), vec![pair("text", "'foo'")]);
        assert_eq!(names("text=foo"), vec![pair("text", "foo")]);
        assert_eq!(names("css:light=div"), vec![pair("css", ":light(div)")]);
        assert_eq!(
            names("div >> text=\"a >> b\" >> span"),
            vec![
                pair("css", "div"),
                pair("text", "\"a >> b\""),
                pair("css", "span")
            ]
        );
        assert_eq!(
            names("text=it's >> span"),
            vec![pair("text", "it's"), pair("css", "span")]
        );
        assert_eq!(
            names("a[title=\"x\\\" >> y\"] >> b"),
            vec![pair("css", "a[title=\"x\\\" >> y\"]"), pair("css", "b")]
        );
    }

    #[test]
    fn capture() {
        let p = parse_selector("div >> *css=span >> a").unwrap();
        assert_eq!(p.capture, Some(1));
        assert_eq!(stringify_selector(&p, false), "div >> *css=span >> a");
        assert_eq!(
            parse_selector("*css=div >> *css=span").unwrap_err().0,
            "Only one of the selectors can capture using * modifier"
        );
    }

    #[test]
    fn nested() {
        let p = parse_selector("div >> internal:has=\"span >> a\"").unwrap();
        match &p.parts[1].body {
            PartBody::Nested { parsed, distance } => {
                assert_eq!(parsed.parts.len(), 2);
                assert!(distance.is_none());
            }
            _ => panic!(),
        }
        assert_eq!(
            stringify_selector(&p, false),
            "div >> internal:has=\"span >> a\""
        );
        assert_eq!(
            parse_selector("internal:has=\"a\"").unwrap_err().0,
            "\"internal:has\" selector cannot be first"
        );
        assert_eq!(
            parse_selector("div >> internal:has=foo").unwrap_err().0,
            "Malformed selector: internal:has=foo"
        );
        assert_eq!(
            parse_selector("div >> internal:has=\"a\", 3")
                .unwrap_err()
                .0,
            "Malformed selector: internal:has=\"a\", 3"
        );
        let p = parse_selector("div >> left-of=\"a\", 3").unwrap();
        assert!(matches!(
            p.parts[1].body,
            PartBody::Nested {
                distance: Some(3.0),
                ..
            }
        ));
    }

    #[test]
    fn nested_same_frame_prefix_is_removed() {
        let p = parse_selector(
            "iframe >> internal:control=enter-frame >> div >> internal:has=\"iframe >> internal:control=enter-frame >> span\"",
        )
        .unwrap();
        match &p.parts[3].body {
            PartBody::Nested { parsed, .. } => {
                assert_eq!(parsed.parts.len(), 1);
                assert_eq!(parsed.parts[0].source, "span");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn split_by_frame() {
        let (any, chunks) =
            split_selector_by_frame("iframe >> internal:control=enter-frame >> div").unwrap();
        assert!(!any);
        assert_eq!(chunks.len(), 2);
        assert_eq!(stringify_selector(&chunks[0], false), "iframe");
        assert_eq!(stringify_selector(&chunks[1], false), "div");
        let (any, chunks) = split_selector_by_frame("internal:control=any-frame >> div").unwrap();
        assert!(any);
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            split_selector_by_frame("internal:control=enter-frame >> div")
                .unwrap_err()
                .0,
            "Selector cannot start with entering frame, select the iframe first"
        );
        assert_eq!(
            split_selector_by_frame("iframe >> internal:control=enter-frame")
                .unwrap_err()
                .0,
            "Selector cannot end with entering frame, while parsing selector iframe >> internal:control=enter-frame"
        );
        assert_eq!(
            split_selector_by_frame("div >> internal:control=any-frame")
                .unwrap_err()
                .0,
            "\"any-frame\" is only allowed as the first selector token, while parsing selector div >> internal:control=any-frame"
        );
        assert_eq!(
            split_selector_by_frame("*css=iframe >> internal:control=enter-frame >> div")
                .unwrap_err()
                .0,
            "Can not capture the selector before diving into the frame. Only use * after the last frame has been selected"
        );
        let (_, chunks) =
            split_selector_by_frame("iframe >> internal:control=enter-frame >> div >> *css=span")
                .unwrap();
        assert_eq!(chunks[1].capture, Some(1));
    }

    #[test]
    fn attribute_selectors() {
        let a =
            parse_attribute_selector("button[name=\"Hello\"i][level=3][checked]", true).unwrap();
        assert_eq!(a.name, "button");
        assert_eq!(a.attributes.len(), 3);
        assert_eq!(
            a.attributes[0].value,
            AttributeValue::String("Hello".into())
        );
        assert!(!a.attributes[0].case_sensitive);
        assert_eq!(a.attributes[1].value, AttributeValue::String("3".into()));
        assert_eq!(a.attributes[2].op, AttributeSelectorOperator::Truthy);

        let a = parse_attribute_selector("[level=3][x=true]", false).unwrap();
        assert_eq!(a.attributes[0].value, AttributeValue::Number(3.0));
        assert_eq!(a.attributes[1].value, AttributeValue::Bool(true));

        let a = parse_attribute_selector("[name=/a\\/b[/]/i]", true).unwrap();
        assert_eq!(
            a.attributes[0].value,
            AttributeValue::Regex(JsRegex::new("a\\/b[/]", "i"))
        );

        let a = parse_attribute_selector("[foo.\"bar baz\"*=\"x\"s]", false).unwrap();
        assert_eq!(a.attributes[0].json_path, vec!["foo", "bar baz"]);
        assert_eq!(a.attributes[0].op, AttributeSelectorOperator::Contains);

        assert_eq!(
            parse_attribute_selector("[a=foo]", false).unwrap_err().0,
            "Error while parsing selector `[a=foo]` - unexpected symbol \"]\" at position 6 during parsing attribute value"
        );
        assert_eq!(
            parse_attribute_selector("[a=\"foo", false).unwrap_err().0,
            "Unexpected end of selector while parsing selector `[a=\"foo`"
        );
        assert_eq!(
            parse_attribute_selector("", false).unwrap_err().0,
            "Error while parsing selector `` - selector cannot be empty"
        );
        assert_eq!(
            parse_attribute_selector("[a*=/x/]", false).unwrap_err().0,
            "Error while parsing selector `[a*=/x/]` - cannot use *= in attribute with regular expression"
        );
    }
}
