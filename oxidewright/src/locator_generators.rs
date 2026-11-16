//! Rendering selectors as locator code (`locator('div').getByText('x')`),
//! ported from Playwright's `isomorphic/locatorGenerators.ts` (JavaScript
//! only). Used for call logs and `Display` of locators.

#![allow(dead_code)]

use crate::js::value::format_js_number;
use crate::locator_utils::{
    JsRegex, TextMatch, escape_with_quotes, normalize_escaped_regex_quotes,
};
use crate::selector_parser::{
    AttributeValue, ParsedSelector, PartBody, parse_attribute_selector, parse_selector,
    stringify_selector,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocatorType {
    Default,
    Role,
    Text,
    Label,
    Placeholder,
    Alt,
    Title,
    TestId,
    Nth,
    First,
    Last,
    Visible,
    FilterVisible,
    HasText,
    HasNotText,
    Has,
    HasNot,
    Frame,
    FrameLocator,
    AnyFrame,
    And,
    Or,
    Chain,
}

#[derive(Debug, Clone)]
enum AttrOut {
    Str(String),
    Bool(bool),
    Number(f64),
}

#[derive(Debug, Clone, Default)]
struct LocatorOptions {
    attrs: Vec<(String, AttrOut)>,
    exact: Option<bool>,
    name: Option<TextMatch>,
    description: Option<TextMatch>,
    has_text: Option<TextMatch>,
    has_not_text: Option<TextMatch>,
}

type GenResult<T> = Result<T, String>;

/// `JavaScriptLocatorFactory` with the default `'` quote.
struct JavaScriptLocatorFactory;

impl JavaScriptLocatorFactory {
    fn generate_locator(
        &self,
        kind: LocatorType,
        body: &TextMatch,
        options: &LocatorOptions,
    ) -> String {
        let body_str = || match body {
            TextMatch::Text(s) => s.clone(),
            TextMatch::Regex(re) => re.to_string(),
        };
        match kind {
            LocatorType::Default => {
                if let Some(has_text) = &options.has_text {
                    return format!(
                        "locator({}, {{ hasText: {} }})",
                        self.quote(&body_str()),
                        self.to_has_text(has_text)
                    );
                }
                if let Some(has_not_text) = &options.has_not_text {
                    return format!(
                        "locator({}, {{ hasNotText: {} }})",
                        self.quote(&body_str()),
                        self.to_has_text(has_not_text)
                    );
                }
                format!("locator({})", self.quote(&body_str()))
            }
            LocatorType::FrameLocator => format!("frameLocator({})", self.quote(&body_str())),
            LocatorType::Frame => "contentFrame()".to_string(),
            LocatorType::AnyFrame => "frameLocator()".to_string(),
            LocatorType::Nth => format!("nth({})", body_str()),
            LocatorType::First => "first()".to_string(),
            LocatorType::Last => "last()".to_string(),
            LocatorType::Visible => "visible()".to_string(),
            LocatorType::FilterVisible => format!(
                "filter({{ visible: {} }})",
                if body_str() == "true" {
                    "true"
                } else {
                    "false"
                }
            ),
            LocatorType::Role => {
                let mut attrs: Vec<String> = Vec::new();
                match &options.name {
                    Some(TextMatch::Regex(re)) => {
                        attrs.push(format!("name: {}", self.regex_to_source_string(re)))
                    }
                    Some(TextMatch::Text(s)) => attrs.push(format!("name: {}", self.quote(s))),
                    None => {}
                }
                match &options.description {
                    Some(TextMatch::Regex(re)) => {
                        attrs.push(format!("description: {}", self.regex_to_source_string(re)))
                    }
                    Some(TextMatch::Text(s)) => {
                        attrs.push(format!("description: {}", self.quote(s)))
                    }
                    None => {}
                }
                let has_string = matches!(options.name, Some(TextMatch::Text(_)))
                    || matches!(options.description, Some(TextMatch::Text(_)));
                if options.exact == Some(true) && has_string {
                    attrs.push("exact: true".to_string());
                }
                for (name, value) in &options.attrs {
                    let value = match value {
                        AttrOut::Str(s) => self.quote(s),
                        AttrOut::Bool(b) => b.to_string(),
                        AttrOut::Number(n) => format_js_number(*n),
                    };
                    attrs.push(format!("{name}: {value}"));
                }
                let attr_string = if attrs.is_empty() {
                    String::new()
                } else {
                    format!(", {{ {} }}", attrs.join(", "))
                };
                format!("getByRole({}{attr_string})", self.quote(&body_str()))
            }
            LocatorType::HasText => format!("filter({{ hasText: {} }})", self.to_has_text(body)),
            LocatorType::HasNotText => {
                format!("filter({{ hasNotText: {} }})", self.to_has_text(body))
            }
            LocatorType::Has => format!("filter({{ has: {} }})", body_str()),
            LocatorType::HasNot => format!("filter({{ hasNot: {} }})", body_str()),
            LocatorType::And => format!("and({})", body_str()),
            LocatorType::Or => format!("or({})", body_str()),
            LocatorType::Chain => format!("locator({})", body_str()),
            LocatorType::TestId => format!("getByTestId({})", self.to_has_text(body)),
            LocatorType::Text => self.to_call_with_exact("getByText", body, options.exact),
            LocatorType::Alt => self.to_call_with_exact("getByAltText", body, options.exact),
            LocatorType::Placeholder => {
                self.to_call_with_exact("getByPlaceholder", body, options.exact)
            }
            LocatorType::Label => self.to_call_with_exact("getByLabel", body, options.exact),
            LocatorType::Title => self.to_call_with_exact("getByTitle", body, options.exact),
        }
    }

    fn chain_locators(&self, locators: &[String]) -> String {
        locators.join(".")
    }

    fn regex_to_source_string(&self, re: &JsRegex) -> String {
        normalize_escaped_regex_quotes(&re.to_string())
    }

    fn to_call_with_exact(&self, method: &str, body: &TextMatch, exact: Option<bool>) -> String {
        match body {
            TextMatch::Regex(re) => format!("{method}({})", self.regex_to_source_string(re)),
            TextMatch::Text(s) => {
                if exact == Some(true) {
                    format!("{method}({}, {{ exact: true }})", self.quote(s))
                } else {
                    format!("{method}({})", self.quote(s))
                }
            }
        }
    }

    fn to_has_text(&self, body: &TextMatch) -> String {
        match body {
            TextMatch::Regex(re) => self.regex_to_source_string(re),
            TextMatch::Text(s) => self.quote(s),
        }
    }

    fn quote(&self, text: &str) -> String {
        escape_with_quotes(text, '\'')
    }
}

fn text(s: impl Into<String>) -> TextMatch {
    TextMatch::Text(s.into())
}

/// `asLocatorDescription('javascript', selector)`.
pub(crate) fn as_locator_description(selector: &str) -> String {
    let Ok(parsed) = parse_selector(selector) else {
        return selector.to_string();
    };
    if let Some(description) = parse_custom_description(&parsed) {
        return description;
    }
    match inner_as_locators(&JavaScriptLocatorFactory, &parsed, false, 1) {
        Ok(mut v) if !v.is_empty() => v.swap_remove(0),
        _ => selector.to_string(),
    }
}

/// `locatorCustomDescription`: the `internal:describe` text, if any.
pub(crate) fn locator_custom_description(selector: &str) -> Option<String> {
    parse_selector(selector)
        .ok()
        .and_then(|parsed| parse_custom_description(&parsed))
}

fn parse_custom_description(parsed: &ParsedSelector) -> Option<String> {
    let last = parsed.parts.last()?;
    if last.name != "internal:describe" {
        return None;
    }
    match serde_json::from_str::<serde_json::Value>(last.body.as_text()?) {
        Ok(serde_json::Value::String(s)) => Some(s),
        _ => None,
    }
}

/// `asLocator('javascript', selector, isFrameLocator)`.
pub(crate) fn as_locator(selector: &str, is_frame_locator: bool) -> String {
    as_locators(selector, is_frame_locator, 1)
        .into_iter()
        .next()
        .unwrap_or_else(|| selector.to_string())
}

/// `asLocators('javascript', selector, isFrameLocator, maxOutputSize)`.
pub(crate) fn as_locators(
    selector: &str,
    is_frame_locator: bool,
    max_output_size: usize,
) -> Vec<String> {
    let result = parse_selector(selector)
        .map_err(|e| e.0)
        .and_then(|parsed| {
            inner_as_locators(
                &JavaScriptLocatorFactory,
                &parsed,
                is_frame_locator,
                max_output_size,
            )
        });
    match result {
        Ok(v) => v,
        Err(_) => vec![selector.to_string()],
    }
}

fn nested_parsed(body: &PartBody) -> GenResult<&ParsedSelector> {
    match body {
        PartBody::Nested { parsed, .. } => Ok(parsed),
        PartBody::Text(_) => Err("expected a nested selector".to_string()),
    }
}

fn body_text(body: &PartBody) -> GenResult<&str> {
    body.as_text()
        .ok_or_else(|| "expected a text body".to_string())
}

fn attr_value_to_text(value: &AttributeValue) -> Option<TextMatch> {
    match value {
        AttributeValue::String(s) => Some(text(s.clone())),
        AttributeValue::Regex(re) => Some(TextMatch::Regex(re.clone())),
        _ => None,
    }
}

fn inner_as_locators(
    factory: &JavaScriptLocatorFactory,
    parsed: &ParsedSelector,
    is_frame_locator: bool,
    max_output_size: usize,
) -> GenResult<Vec<String>> {
    // The JavaScript factory renders `page`, `locator` and `frame-locator`
    // bases the same way, so the starting base does not matter.
    let _ = is_frame_locator;
    let parts = &parsed.parts;
    let mut tokens: Vec<Vec<String>> = Vec::new();
    let no_options = LocatorOptions::default();
    let empty = text("");
    let make = |kind: LocatorType, body: &TextMatch, options: &LocatorOptions| {
        factory.generate_locator(kind, body, options)
    };
    let mut index = 0;
    while index < parts.len() {
        let part = &parts[index];
        let name = part.name.as_str();

        if name == "internal:describe" {
            index += 1;
            continue;
        }
        if name == "nth" {
            let body = body_text(&part.body)?;
            if body == "0" {
                tokens.push(vec![
                    make(LocatorType::First, &empty, &no_options),
                    make(LocatorType::Nth, &text("0"), &no_options),
                ]);
            } else if body == "-1" {
                tokens.push(vec![
                    make(LocatorType::Last, &empty, &no_options),
                    make(LocatorType::Nth, &text("-1"), &no_options),
                ]);
            } else {
                tokens.push(vec![make(LocatorType::Nth, &text(body), &no_options)]);
            }
            index += 1;
            continue;
        }
        if name == "visible" {
            let body = body_text(&part.body)?;
            let mut list = Vec::new();
            if body == "true" {
                list.push(make(LocatorType::Visible, &empty, &no_options));
            }
            list.push(make(LocatorType::FilterVisible, &text(body), &no_options));
            list.push(make(
                LocatorType::Default,
                &text(format!("visible={body}")),
                &no_options,
            ));
            tokens.push(list);
            index += 1;
            continue;
        }
        if name == "internal:text" {
            let (exact, t) = detect_exact(body_text(&part.body)?)?;
            let options = LocatorOptions {
                exact: Some(exact),
                ..Default::default()
            };
            tokens.push(vec![make(LocatorType::Text, &t, &options)]);
            index += 1;
            continue;
        }
        if name == "internal:has-text" || name == "internal:has-not-text" {
            let (exact, t) = detect_exact(body_text(&part.body)?)?;
            // There is no locator equivalent for strict has-text, leave it as is.
            if !exact {
                let kind = if name == "internal:has-text" {
                    LocatorType::HasText
                } else {
                    LocatorType::HasNotText
                };
                let options = LocatorOptions {
                    exact: Some(exact),
                    ..Default::default()
                };
                tokens.push(vec![make(kind, &t, &options)]);
                index += 1;
                continue;
            }
        }
        let nested_kind = match name {
            "internal:has" => Some(LocatorType::Has),
            "internal:has-not" => Some(LocatorType::HasNot),
            "internal:and" => Some(LocatorType::And),
            "internal:or" => Some(LocatorType::Or),
            "internal:chain" => Some(LocatorType::Chain),
            _ => None,
        };
        if let Some(kind) = nested_kind {
            let inners =
                inner_as_locators(factory, nested_parsed(&part.body)?, false, max_output_size)?;
            tokens.push(
                inners
                    .into_iter()
                    .map(|inner| make(kind, &text(inner), &no_options))
                    .collect(),
            );
            index += 1;
            continue;
        }
        if name == "internal:label" {
            let (exact, t) = detect_exact(body_text(&part.body)?)?;
            let options = LocatorOptions {
                exact: Some(exact),
                ..Default::default()
            };
            tokens.push(vec![make(LocatorType::Label, &t, &options)]);
            index += 1;
            continue;
        }
        if name == "internal:role" {
            let attr_selector =
                parse_attribute_selector(body_text(&part.body)?, true).map_err(|e| e.0)?;
            let mut options = LocatorOptions::default();
            for attr in &attr_selector.attributes {
                if attr.name == "name" || attr.name == "description" {
                    // Only string values carry exactness, regular expressions are used as is.
                    if matches!(attr.value, AttributeValue::String(_)) {
                        if options.exact.is_some() && options.exact != Some(attr.case_sensitive) {
                            return Err(format!(
                                "Conflicting exactness in internal:role selector: {}",
                                stringify_selector(
                                    &ParsedSelector {
                                        parts: vec![part.clone()],
                                        capture: None
                                    },
                                    false
                                )
                            ));
                        }
                        options.exact = Some(attr.case_sensitive);
                    }
                    let value = attr_value_to_text(&attr.value);
                    if attr.name == "name" {
                        options.name = value;
                    } else {
                        options.description = value;
                    }
                } else {
                    let value = match &attr.value {
                        AttributeValue::String(s) if attr.name == "level" => {
                            AttrOut::Number(crate::selector_parser::js_to_number(s))
                        }
                        AttributeValue::String(s) => AttrOut::Str(s.clone()),
                        AttributeValue::Bool(b) => AttrOut::Bool(*b),
                        AttributeValue::Number(n) => AttrOut::Number(*n),
                        AttributeValue::Regex(re) => AttrOut::Str(re.to_string()),
                        AttributeValue::Null => AttrOut::Str("null".to_string()),
                    };
                    let attr_name = if attr.name == "include-hidden" {
                        "includeHidden".to_string()
                    } else {
                        attr.name.clone()
                    };
                    options.attrs.push((attr_name, value));
                }
            }
            tokens.push(vec![make(
                LocatorType::Role,
                &text(attr_selector.name),
                &options,
            )]);
            index += 1;
            continue;
        }
        if name == "internal:testid" {
            let attr_selector =
                parse_attribute_selector(body_text(&part.body)?, true).map_err(|e| e.0)?;
            let first = attr_selector
                .attributes
                .first()
                .ok_or_else(|| "missing test id".to_string())?;
            let value = attr_value_to_text(&first.value).unwrap_or_else(|| text(""));
            tokens.push(vec![make(LocatorType::TestId, &value, &no_options)]);
            index += 1;
            continue;
        }
        if name == "internal:attr" {
            let attr_selector =
                parse_attribute_selector(body_text(&part.body)?, true).map_err(|e| e.0)?;
            let first = attr_selector
                .attributes
                .first()
                .ok_or_else(|| "missing attribute".to_string())?;
            let kind = match first.name.as_str() {
                "placeholder" => Some(LocatorType::Placeholder),
                "alt" => Some(LocatorType::Alt),
                "title" => Some(LocatorType::Title),
                _ => None,
            };
            if let Some(kind) = kind {
                let value = attr_value_to_text(&first.value).unwrap_or_else(|| text(""));
                let options = LocatorOptions {
                    exact: Some(first.case_sensitive),
                    ..Default::default()
                };
                tokens.push(vec![make(kind, &value, &options)]);
                index += 1;
                continue;
            }
        }
        if name == "internal:control" && part.body.as_text() == Some("any-frame") {
            tokens.push(vec![make(LocatorType::AnyFrame, &empty, &no_options)]);
            index += 1;
            continue;
        }
        if name == "internal:control" && part.body.as_text() == Some("enter-frame") {
            // Transform the last tokens from `${selector}` into
            // `${selector}.contentFrame()` and `frameLocator(${selector})`.
            if index == 0 || tokens.is_empty() {
                return Err("enter-frame without a frame selector".to_string());
            }
            let last_part = &parts[index - 1];
            let frame = make(LocatorType::Frame, &empty, &no_options);
            let last_tokens = tokens.last_mut().unwrap();
            let mut transformed: Vec<String> = last_tokens
                .iter()
                .map(|token| factory.chain_locators(&[token.clone(), frame.clone()]))
                .collect();
            if last_part.name == "xpath" || last_part.name == "css" {
                let single = ParsedSelector {
                    parts: vec![last_part.clone()],
                    capture: None,
                };
                transformed.push(make(
                    LocatorType::FrameLocator,
                    &text(stringify_selector(&single, false)),
                    &no_options,
                ));
                transformed.push(make(
                    LocatorType::FrameLocator,
                    &text(stringify_selector(&single, true)),
                    &no_options,
                ));
            }
            *last_tokens = transformed;
            index += 1;
            continue;
        }

        let next_part = parts.get(index + 1);
        let single = ParsedSelector {
            parts: vec![part.clone()],
            capture: None,
        };
        let selector_part = stringify_selector(&single, false);
        let locator_part = make(LocatorType::Default, &text(&selector_part), &no_options);

        if let Some(next) = next_part {
            if next.name == "internal:has-text" || next.name == "internal:has-not-text" {
                let (exact, t) = detect_exact(body_text(&next.body)?)?;
                // There is no locator equivalent for strict has-text and has-not-text.
                if !exact {
                    let is_has = next.name == "internal:has-text";
                    let next_locator_part = make(
                        if is_has {
                            LocatorType::HasText
                        } else {
                            LocatorType::HasNotText
                        },
                        &t,
                        &LocatorOptions {
                            exact: Some(exact),
                            ..Default::default()
                        },
                    );
                    let mut options = LocatorOptions::default();
                    if is_has {
                        options.has_text = Some(t);
                    } else {
                        options.has_not_text = Some(t);
                    }
                    let combined_part = make(LocatorType::Default, &text(&selector_part), &options);
                    tokens.push(vec![
                        factory.chain_locators(&[locator_part, next_locator_part]),
                        combined_part,
                    ]);
                    index += 2;
                    continue;
                }
            }
        }

        // Selectors can be prefixed with engine name, e.g. xpath=//foo
        let mut list = vec![locator_part];
        if part.name == "xpath" || part.name == "css" {
            let with_engine = stringify_selector(&single, true);
            list.push(make(LocatorType::Default, &text(with_engine), &no_options));
        }
        tokens.push(list);
        index += 1;
    }
    Ok(combine_tokens(factory, &tokens, max_output_size))
}

fn combine_tokens(
    factory: &JavaScriptLocatorFactory,
    tokens: &[Vec<String>],
    max_output_size: usize,
) -> Vec<String> {
    fn visit(
        factory: &JavaScriptLocatorFactory,
        tokens: &[Vec<String>],
        index: usize,
        current: &mut Vec<String>,
        result: &mut Vec<String>,
        max: usize,
    ) -> bool {
        if index == tokens.len() {
            result.push(factory.chain_locators(current));
            return result.len() < max;
        }
        for taken in &tokens[index] {
            current[index] = taken.clone();
            if !visit(factory, tokens, index + 1, current, result, max) {
                return false;
            }
        }
        true
    }
    let mut current = vec![String::new(); tokens.len()];
    let mut result = Vec::new();
    visit(
        factory,
        tokens,
        0,
        &mut current,
        &mut result,
        max_output_size,
    );
    result
}

/// `detectExact`: `"foo"s`/`"foo"` are exact, `"foo"i` is not, `/re/f` is a regex.
fn detect_exact(t: &str) -> GenResult<(bool, TextMatch)> {
    if let Some(rest) = t.strip_prefix('/') {
        if let Some(slash) = rest.rfind('/') {
            let (source, flags) = (&rest[..slash], &rest[slash + 1..]);
            let no_line_terminators = !source.contains(['\n', '\r', '\u{2028}', '\u{2029}']);
            if no_line_terminators && flags.chars().all(|c| "dgimsuvy".contains(c)) {
                return Ok((false, TextMatch::Regex(JsRegex::new(source, flags))));
            }
        }
    }
    let parse = |s: &str| -> GenResult<String> {
        serde_json::from_str::<String>(s).map_err(|e| e.to_string())
    };
    if t.ends_with('"') {
        return Ok((true, text(parse(t)?)));
    }
    if t.ends_with("\"s") {
        return Ok((true, text(parse(&t[..t.len() - 1])?)));
    }
    if t.ends_with("\"i") {
        return Ok((false, text(parse(&t[..t.len() - 1])?)));
    }
    Ok((false, text(t)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locator_utils::{
        ByRoleOptions, get_by_alt_text_selector, get_by_label_selector,
        get_by_placeholder_selector, get_by_role_selector, get_by_test_id_selector,
        get_by_text_selector, get_by_title_selector, json_string, regex,
    };

    // Helpers mirroring the client `Locator` selector building.
    fn t(s: &str) -> TextMatch {
        s.into()
    }
    fn re(s: &str, f: &str) -> TextMatch {
        regex(s, f).into()
    }
    fn by_test_id(v: TextMatch) -> String {
        get_by_test_id_selector("data-testid", &v)
    }
    fn chain(a: &str, b: &str) -> String {
        format!("{a} >> {b}")
    }
    fn has(outer: &str, inner: &str) -> String {
        format!("{outer} >> internal:has={}", json_string(inner))
    }
    fn has_not(outer: &str, inner: &str) -> String {
        format!("{outer} >> internal:has-not={}", json_string(inner))
    }
    fn has_text(outer: &str, v: TextMatch) -> String {
        format!(
            "{outer} >> internal:has-text={}",
            crate::locator_utils::escape_for_text_selector(&v, false)
        )
    }
    fn has_not_text(outer: &str, v: TextMatch) -> String {
        format!(
            "{outer} >> internal:has-not-text={}",
            crate::locator_utils::escape_for_text_selector(&v, false)
        )
    }
    fn js(selector: &str) -> String {
        as_locator(selector, false)
    }

    #[test]
    fn gn_001_reverse_engineer_locators() {
        assert_eq!(js(&by_test_id(t("Hello"))), "getByTestId('Hello')");
        assert_eq!(js(&by_test_id(t("He\"llo"))), "getByTestId('He\"llo')");
        assert_eq!(js(&by_test_id(re("He\"llo", ""))), "getByTestId(/He\"llo/)");
        assert_eq!(
            js(&by_test_id(re("He\\\\\"llo", ""))),
            "getByTestId(/He\\\\\"llo/)"
        );
        assert_eq!(
            js(&get_by_text_selector(&t("Hello"), true)),
            "getByText('Hello', { exact: true })"
        );
        assert_eq!(
            js(&get_by_text_selector(&t("Hello"), false)),
            "getByText('Hello')"
        );
        assert_eq!(
            js(&get_by_text_selector(&re("Hello", ""), false)),
            "getByText(/Hello/)"
        );
        assert_eq!(
            js(&get_by_label_selector(&t("Name"), false)),
            "getByLabel('Name')"
        );
        assert_eq!(
            js(&get_by_label_selector(&t("Last Name"), true)),
            "getByLabel('Last Name', { exact: true })"
        );
        assert_eq!(
            js(&get_by_label_selector(&re("Last\\s+name", "i"), false)),
            "getByLabel(/Last\\s+name/i)"
        );
        for flags in ["u", "s", "y", "d", "v", "gm"] {
            for (engine, method) in [("text", "getByText"), ("label", "getByLabel")] {
                let selector = format!("internal:{engine}=/Hello/{flags}");
                assert_eq!(js(&selector), format!("{method}(/Hello/{flags})"));
            }
        }
        assert_eq!(
            js(&get_by_placeholder_selector(&t("hello"), false)),
            "getByPlaceholder('hello')"
        );
        assert_eq!(
            js(&get_by_placeholder_selector(&t("Hello"), true)),
            "getByPlaceholder('Hello', { exact: true })"
        );
        assert_eq!(
            js(&get_by_placeholder_selector(&re("wor", "i"), false)),
            "getByPlaceholder(/wor/i)"
        );
        assert_eq!(
            js(&get_by_alt_text_selector(&t("hello"), false)),
            "getByAltText('hello')"
        );
        assert_eq!(
            js(&get_by_alt_text_selector(&t("Hello"), true)),
            "getByAltText('Hello', { exact: true })"
        );
        assert_eq!(
            js(&get_by_alt_text_selector(&re("wor", "i"), false)),
            "getByAltText(/wor/i)"
        );
        assert_eq!(
            js(&get_by_title_selector(&t("hello"), false)),
            "getByTitle('hello')"
        );
        assert_eq!(
            js(&get_by_title_selector(&t("Hello"), true)),
            "getByTitle('Hello', { exact: true })"
        );
        assert_eq!(
            js(&get_by_title_selector(&re("wor", "i"), false)),
            "getByTitle(/wor/i)"
        );
        assert_eq!(
            js(&get_by_placeholder_selector(&t("hello my\nwo\"rld"), false)),
            "getByPlaceholder('hello my\\nwo\"rld')"
        );
        assert_eq!(
            js(&get_by_alt_text_selector(&t("hello my\nwo\"rld"), false)),
            "getByAltText('hello my\\nwo\"rld')"
        );
        assert_eq!(
            js(&get_by_title_selector(&t("hello my\nwo\"rld"), false)),
            "getByTitle('hello my\\nwo\"rld')"
        );
    }

    fn role(role: &str, options: ByRoleOptions) -> String {
        js(&get_by_role_selector(role, &options))
    }

    #[test]
    fn gn_002_reverse_engineer_get_by_role() {
        assert_eq!(role("button", Default::default()), "getByRole('button')");
        assert_eq!(role("heading", Default::default()), "getByRole('heading')");
        assert_eq!(
            role(
                "button",
                ByRoleOptions {
                    name: Some(t("Hello")),
                    ..Default::default()
                }
            ),
            "getByRole('button', { name: 'Hello' })"
        );
        assert_eq!(
            role(
                "button",
                ByRoleOptions {
                    name: Some(re("Hello", "")),
                    ..Default::default()
                }
            ),
            "getByRole('button', { name: /Hello/ })"
        );
        assert_eq!(
            role(
                "button",
                ByRoleOptions {
                    name: Some(t("He\"llo")),
                    exact: Some(true),
                    ..Default::default()
                }
            ),
            "getByRole('button', { name: 'He\"llo', exact: true })"
        );
        assert_eq!(
            role(
                "button",
                ByRoleOptions {
                    checked: Some(true),
                    pressed: Some(false),
                    level: Some(3),
                    ..Default::default()
                }
            ),
            "getByRole('button', { checked: true, level: 3, pressed: false })"
        );
        assert_eq!(
            role(
                "alert",
                ByRoleOptions {
                    name: Some(t("Upload")),
                    description: Some(t("doc.pdf")),
                    ..Default::default()
                }
            ),
            "getByRole('alert', { name: 'Upload', description: 'doc.pdf' })"
        );
        assert_eq!(
            role(
                "alert",
                ByRoleOptions {
                    description: Some(t("doc.pdf")),
                    ..Default::default()
                }
            ),
            "getByRole('alert', { description: 'doc.pdf' })"
        );
        assert_eq!(
            role(
                "alert",
                ByRoleOptions {
                    description: Some(re("doc\\.pdf", "")),
                    ..Default::default()
                }
            ),
            "getByRole('alert', { description: /doc\\.pdf/ })"
        );
        assert_eq!(
            role(
                "alert",
                ByRoleOptions {
                    name: Some(t("Upload")),
                    description: Some(t("doc.pdf")),
                    exact: Some(true),
                    ..Default::default()
                }
            ),
            "getByRole('alert', { name: 'Upload', description: 'doc.pdf', exact: true })"
        );
        assert_eq!(
            role(
                "checkbox",
                ByRoleOptions {
                    include_hidden: Some(true),
                    ..Default::default()
                }
            ),
            "getByRole('checkbox', { includeHidden: true })"
        );
    }

    #[test]
    fn gn_003_conflicting_exactness() {
        let conflicting = "internal:role=row[name=\"abc\"i][description=\"d\"s]";
        let reversed = "internal:role=row[name=\"abc\"s][description=\"d\"i]";
        assert_eq!(js(conflicting), conflicting);
        assert_eq!(js(reversed), reversed);
    }

    #[test]
    fn gn_004_role_with_regex_and_string() {
        assert_eq!(
            js("internal:role=alert[name=/Upload/][description=\"doc.pdf\"i]"),
            "getByRole('alert', { name: /Upload/, description: 'doc.pdf' })"
        );
        assert_eq!(
            js("internal:role=alert[name=\"Upload\"i][description=/doc\\.pdf/]"),
            "getByRole('alert', { name: 'Upload', description: /doc\\.pdf/ })"
        );
    }

    #[test]
    fn gn_005_ignore_case_locators() {
        assert_eq!(
            js(&get_by_text_selector(&t("hello my\nwo\"rld"), false)),
            "getByText('hello my\\nwo\"rld')"
        );
        assert_eq!(
            js(&get_by_text_selector(
                &t("hello       my     wo\"rld"),
                false
            )),
            "getByText('hello       my     wo\"rld')"
        );
        assert_eq!(
            js(&get_by_label_selector(&t("hello my\nwo\"rld"), false)),
            "getByLabel('hello my\\nwo\"rld')"
        );
    }

    #[test]
    fn gn_006_ordered_locators() {
        assert_eq!(
            js("div >> nth=3 >> nth=0 >> nth=-1"),
            "locator('div').nth(3).first().last()"
        );
    }

    #[test]
    fn gn_007_locators_with_regex() {
        assert_eq!(
            js(&get_by_text_selector(&re("he\\/\\sl\\nlo", ""), false)),
            "getByText(/he\\/\\sl\\nlo/)"
        );
        assert_eq!(
            js(&get_by_placeholder_selector(
                &re("he\\/\\sl\\nlo", ""),
                false
            )),
            "getByPlaceholder(/he\\/\\sl\\nlo/)"
        );
        assert_eq!(
            js(&get_by_text_selector(&re("hel\"lo", ""), false)),
            "getByText(/hel\"lo/)"
        );
        assert_eq!(
            js(&get_by_placeholder_selector(&re("hel\"lo", ""), false)),
            "getByPlaceholder(/hel\"lo/)"
        );
    }

    #[test]
    fn gn_008_has_text() {
        let base = get_by_text_selector(&t("Hello"), false);
        assert_eq!(
            js(&has_text(&base, t("wo\"rld\n"))),
            "getByText('Hello').filter({ hasText: 'wo\"rld\\n' })"
        );
        assert_eq!(
            js(&has_text(&base, re("wo\\/\\srld\\n", ""))),
            "getByText('Hello').filter({ hasText: /wo\\/\\srld\\n/ })"
        );
        assert_eq!(
            js(&has_text(&base, re("wor\"ld", ""))),
            "getByText('Hello').filter({ hasText: /wor\"ld/ })"
        );
    }

    #[test]
    fn gn_009_has_not_text() {
        let base = get_by_text_selector(&t("Hello"), false);
        assert_eq!(
            js(&has_not_text(&base, t("wo\"rld\n"))),
            "getByText('Hello').filter({ hasNotText: 'wo\"rld\\n' })"
        );
    }

    #[test]
    fn gn_010_visible() {
        let base = get_by_text_selector(&t("Hello"), false);
        assert_eq!(
            js(&format!("{base} >> visible=true >> div")),
            "getByText('Hello').visible().locator('div')"
        );
        assert_eq!(
            js(&format!("{base} >> visible=false >> div")),
            "getByText('Hello').filter({ visible: false }).locator('div')"
        );
    }

    #[test]
    fn gn_011_has() {
        let base = get_by_text_selector(&t("Hello"), false);
        let inner = chain("div", &get_by_text_selector(&t("bye"), false));
        assert_eq!(
            js(&has(&base, &inner)),
            "getByText('Hello').filter({ has: locator('div').getByText('bye') })"
        );
        let s = has(
            &has_text(&has("section", &has("div", "span")), t("foo")),
            "a",
        );
        assert_eq!(
            js(&s),
            "locator('section').filter({ has: locator('div').filter({ has: locator('span') }) }).filter({ hasText: 'foo' }).filter({ has: locator('a') })"
        );
    }

    #[test]
    fn gn_012_has_not() {
        let base = get_by_text_selector(&t("Hello"), false);
        let inner = chain("div", &get_by_text_selector(&t("bye"), false));
        assert_eq!(
            js(&has_not(&base, &inner)),
            "getByText('Hello').filter({ hasNot: locator('div').getByText('bye') })"
        );
        let s = has_not(
            &has_text(&has("section", &has_not("div", "span")), t("foo")),
            "a",
        );
        assert_eq!(
            js(&s),
            "locator('section').filter({ has: locator('div').filter({ hasNot: locator('span') }) }).filter({ hasText: 'foo' }).filter({ hasNot: locator('a') })"
        );
    }

    #[test]
    fn gn_013_has_and_has_text() {
        let s = chain(&has(&has_text("section", t("foo")), "div"), "a");
        assert_eq!(
            js(&s),
            "locator('section').filter({ hasText: 'foo' }).filter({ has: locator('div') }).locator('a')"
        );
    }

    #[test]
    fn gn_014_frame_locator() {
        let s = "iframe >> internal:control=enter-frame >> internal:text=\"foo\"s >> frame >> nth=0 >> internal:control=enter-frame >> iframe >> internal:control=enter-frame >> span";
        assert_eq!(
            js(s),
            "locator('iframe').contentFrame().getByText('foo', { exact: true }).locator('frame').first().contentFrame().locator('iframe').contentFrame().locator('span')"
        );
        assert_eq!(
            js("div >> iframe >> internal:control=enter-frame >> span"),
            "locator('div').locator('iframe').contentFrame().locator('span')"
        );
        assert_eq!(
            as_locators(
                "internal:attr=[title=\"iframe title\"i] >> internal:control=enter-frame",
                false,
                20
            ),
            vec!["getByTitle('iframe title').contentFrame()".to_string()]
        );
    }

    #[test]
    fn gn_015_frame_locator_without_selector() {
        assert_eq!(
            js("internal:control=any-frame >> internal:text=\"foo\"i >> span"),
            "frameLocator().getByText('foo').locator('span')"
        );
        assert_eq!(js("internal:control=any-frame"), "frameLocator()");
    }

    #[test]
    fn gn_016_generate_multiple_locators() {
        let inner = format!("{} >> nth=-1", has_not_text("span", t("bar")));
        let selector = has(&format!("{} >> nth=0", has_text("div", t("foo"))), &inner);
        let expected = [
            "locator('div').filter({ hasText: 'foo' }).first().filter({ has: locator('span').filter({ hasNotText: 'bar' }).last() })",
            "locator('div').filter({ hasText: 'foo' }).first().filter({ has: locator('span').filter({ hasNotText: 'bar' }).nth(-1) })",
            "locator('div').filter({ hasText: 'foo' }).first().filter({ has: locator('span', { hasNotText: 'bar' }).last() })",
            "locator('div').filter({ hasText: 'foo' }).first().filter({ has: locator('span', { hasNotText: 'bar' }).nth(-1) })",
            "locator('div').filter({ hasText: 'foo' }).nth(0).filter({ has: locator('span').filter({ hasNotText: 'bar' }).last() })",
            "locator('div').filter({ hasText: 'foo' }).nth(0).filter({ has: locator('span').filter({ hasNotText: 'bar' }).nth(-1) })",
            "locator('div').filter({ hasText: 'foo' }).nth(0).filter({ has: locator('span', { hasNotText: 'bar' }).last() })",
            "locator('div').filter({ hasText: 'foo' }).nth(0).filter({ has: locator('span', { hasNotText: 'bar' }).nth(-1) })",
            "locator('div', { hasText: 'foo' }).first().filter({ has: locator('span').filter({ hasNotText: 'bar' }).last() })",
            "locator('div', { hasText: 'foo' }).first().filter({ has: locator('span').filter({ hasNotText: 'bar' }).nth(-1) })",
            "locator('div', { hasText: 'foo' }).first().filter({ has: locator('span', { hasNotText: 'bar' }).last() })",
            "locator('div', { hasText: 'foo' }).first().filter({ has: locator('span', { hasNotText: 'bar' }).nth(-1) })",
            "locator('div', { hasText: 'foo' }).nth(0).filter({ has: locator('span').filter({ hasNotText: 'bar' }).last() })",
            "locator('div', { hasText: 'foo' }).nth(0).filter({ has: locator('span').filter({ hasNotText: 'bar' }).nth(-1) })",
            "locator('div', { hasText: 'foo' }).nth(0).filter({ has: locator('span', { hasNotText: 'bar' }).last() })",
            "locator('div', { hasText: 'foo' }).nth(0).filter({ has: locator('span', { hasNotText: 'bar' }).nth(-1) })",
        ];
        assert_eq!(as_locators(&selector, false, 20), expected);
    }

    #[test]
    fn gn_017_strict_has_text_kept() {
        assert_eq!(
            js("div >> internal:has-text=\"foo\"s"),
            "locator('div').locator('internal:has-text=\"foo\"s')"
        );
        assert_eq!(
            js("div >> internal:has-not-text=\"foo\"s"),
            "locator('div').locator('internal:has-not-text=\"foo\"s')"
        );
    }

    #[test]
    fn gn_018_020_and_or_chain() {
        assert_eq!(
            js("div >> internal:and=\"span >> article\""),
            "locator('div').and(locator('span').locator('article'))"
        );
        assert_eq!(
            js("div >> internal:or=\"span >> article\""),
            "locator('div').or(locator('span').locator('article'))"
        );
        assert_eq!(
            js("div >> internal:chain=\"span >> article\""),
            "locator('div').locator(locator('span').locator('article'))"
        );
    }

    #[test]
    fn gn_021_nested_locators() {
        let row = get_by_role_selector("row", &Default::default());
        let cell = format!(
            "{} >> nth=2",
            get_by_role_selector("cell", &Default::default())
        );
        let s = format!("{row} >> internal:chain={}", json_string(&cell));
        assert_eq!(js(&s), "getByRole('row').locator(getByRole('cell').nth(2))");

        let and = format!(
            "div >> internal:and={}",
            json_string(&get_by_text_selector(&t("foo"), false))
        );
        let inner_or = format!("span >> internal:chain={}", json_string("a"));
        let s = format!("{and} >> internal:or={} >> nth=0", json_string(&inner_or));
        assert_eq!(
            js(&s),
            "locator('div').and(getByText('foo')).or(locator('span').locator(locator('a'))).first()"
        );
    }

    #[test]
    fn gn_022_xpath() {
        let selector = "//*[contains(normalizer-text(), 'foo']";
        assert_eq!(
            js(selector),
            "locator('//*[contains(normalizer-text(), \\'foo\\']')"
        );
    }

    #[test]
    fn gn_023_regex_flags() {
        assert_eq!(js("internal:text=/a.b/ims"), "getByText(/a.b/ims)");
        assert_eq!(
            js("internal:role=button[name=/a.b/s]"),
            "getByRole('button', { name: /a.b/s })"
        );
    }

    #[test]
    fn gn_025_as_locator_description() {
        assert_eq!(
            as_locator_description("body >> internal:describe=\"desc\""),
            "desc"
        );
        assert_eq!(
            as_locator_description("body >> internal:describe=12"),
            "locator('body')"
        );
        // Not ported: `following-sibling::*[1]` is rejected by Playwright's CSS
        // parser (not ported), here it renders as `locator(...)`.
        assert_eq!(
            as_locator_description("body >> internal:describe=\"desc\" >> div"),
            "locator('body').locator('div')"
        );
        assert_eq!(
            locator_custom_description("body >> internal:describe=\"desc\""),
            Some("desc".to_string())
        );
    }

    #[test]
    fn frame_locator_flag_and_test_id() {
        assert_eq!(as_locator("iframe", true), "locator('iframe')");
        assert_eq!(
            js("internal:testid=[data-testid=/He\\\\\\\"llo/]"),
            "getByTestId(/He\\\\\"llo/)"
        );
    }
}
