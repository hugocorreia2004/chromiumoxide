//! Message formatting of the assertions, without colors: Playwright's
//! `formatMatcherMessage` (`matchers/matcherHint.ts`) and the parts of
//! `jest-matcher-utils` / `jest-diff` / `pretty-format` it relies on
//! (`stringify`, `printDiffOrStringify`, `getLabelPrinter`, unified line
//! diffs with change counts).

use crate::js::JsValue;
use crate::js::value::format_js_number;

/// Strings longer than this are not diffed (`MAX_DIFF_STRING_LENGTH`).
const MAX_DIFF_STRING_LENGTH: usize = 20_000;

/// `pretty-format` options.
#[derive(Clone, Copy)]
struct PrettyOptions {
    min: bool,
    max_depth: usize,
    max_width: usize,
}

fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn format_number(n: f64) -> String {
    if n == 0.0 && n.is_sign_negative() {
        "-0".to_string()
    } else {
        format_js_number(n)
    }
}

fn pretty(value: &JsValue, options: PrettyOptions, indent: &str, depth: usize) -> String {
    let inner_indent = format!("{indent}  ");
    let list =
        |open_min: &str, open: &str, close: &str, items: Vec<String>, truncated: bool| -> String {
            let mut items = items;
            if truncated {
                items.push("…".to_string());
            }
            if items.is_empty() {
                return if options.min {
                    format!("{open_min}{close}")
                } else {
                    format!("{open}{close}")
                };
            }
            if options.min {
                format!("{open_min}{}{close}", items.join(", "))
            } else {
                let mut out = format!("{open}\n");
                for item in items {
                    out.push_str(&inner_indent);
                    out.push_str(&item);
                    out.push_str(",\n");
                }
                out.push_str(indent);
                out.push_str(close);
                out
            }
        };
    match value {
        JsValue::Undefined => "undefined".to_string(),
        JsValue::Null => "null".to_string(),
        JsValue::Bool(b) => b.to_string(),
        JsValue::Number(n) => format_number(*n),
        JsValue::String(s) => format!("\"{}\"", escape_string(s)),
        JsValue::BigInt(b) => format!("{b}n"),
        JsValue::Date(d) => d.clone(),
        JsValue::Url(u) => format!("\"{}\"", escape_string(u)),
        JsValue::RegExp { source, flags } => format!("/{source}/{flags}"),
        JsValue::Error { name, message, .. } => format!("[{name}: {message}]"),
        JsValue::Handle(_) => "JSHandle".to_string(),
        JsValue::TypedArray { kind, .. } => format!("{kind} []"),
        JsValue::ArrayBuffer(_) => "ArrayBuffer []".to_string(),
        JsValue::Array(items) => {
            if depth >= options.max_depth {
                return "[Array]".to_string();
            }
            let shown: Vec<String> = items
                .iter()
                .take(options.max_width)
                .map(|v| pretty(v, options, &inner_indent, depth + 1))
                .collect();
            let prefix = if options.min { "[" } else { "Array [" };
            list("[", prefix, "]", shown, items.len() > options.max_width)
        }
        JsValue::Set(items) => {
            if depth >= options.max_depth {
                return "[Set]".to_string();
            }
            let shown: Vec<String> = items
                .iter()
                .take(options.max_width)
                .map(|v| pretty(v, options, &inner_indent, depth + 1))
                .collect();
            list(
                "Set {",
                "Set {",
                "}",
                shown,
                items.len() > options.max_width,
            )
        }
        JsValue::Map(entries) => {
            if depth >= options.max_depth {
                return "[Map]".to_string();
            }
            let shown: Vec<String> = entries
                .iter()
                .take(options.max_width)
                .map(|(k, v)| {
                    format!(
                        "{} => {}",
                        pretty(k, options, &inner_indent, depth + 1),
                        pretty(v, options, &inner_indent, depth + 1)
                    )
                })
                .collect();
            list(
                "Map {",
                "Map {",
                "}",
                shown,
                entries.len() > options.max_width,
            )
        }
        JsValue::Object(entries) => {
            if depth >= options.max_depth {
                return "[Object]".to_string();
            }
            let mut sorted: Vec<&(String, JsValue)> = entries.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            let shown: Vec<String> = sorted
                .iter()
                .take(options.max_width)
                .map(|(k, v)| {
                    format!(
                        "\"{}\": {}",
                        escape_string(k),
                        pretty(v, options, &inner_indent, depth + 1)
                    )
                })
                .collect();
            let prefix = if options.min { "{" } else { "Object {" };
            list("{", prefix, "}", shown, entries.len() > options.max_width)
        }
    }
}

/// `stringify` of jest-matcher-utils (`printExpected` / `printReceived`
/// without colors).
pub(crate) fn stringify(value: &JsValue) -> String {
    let mut max_depth = 10;
    loop {
        let result = pretty(
            value,
            PrettyOptions {
                min: true,
                max_depth,
                max_width: 10,
            },
            "",
            0,
        );
        if result.len() <= 10_000 || max_depth <= 1 {
            return result;
        }
        max_depth /= 2;
    }
}

/// `pretty-format` with jest-diff's options (indented, no limits).
fn pretty_for_diff(value: &JsValue) -> String {
    pretty(
        value,
        PrettyOptions {
            min: false,
            max_depth: usize::MAX,
            max_width: usize::MAX,
        },
        "",
        0,
    )
}

/// `getLabelPrinter`: pads labels to the same width.
pub(crate) fn label_printer(labels: &[&str]) -> impl Fn(&str) -> String + use<> {
    let max = labels.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    move |label: &str| format!("{label}: {}", " ".repeat(max - label.chars().count()))
}

fn type_name(value: &JsValue) -> &'static str {
    match value {
        JsValue::Undefined => "undefined",
        JsValue::Null => "null",
        JsValue::Bool(_) => "boolean",
        JsValue::Number(_) => "number",
        JsValue::String(_) => "string",
        JsValue::BigInt(_) => "bigint",
        JsValue::Date(_) => "date",
        JsValue::RegExp { .. } => "regexp",
        JsValue::Array(_) => "array",
        JsValue::Map(_) => "map",
        JsValue::Set(_) => "set",
        _ => "object",
    }
}

/// `isLineDiffable` of jest-matcher-utils.
fn is_line_diffable(expected: &JsValue, received: &JsValue) -> bool {
    let (et, rt) = (type_name(expected), type_name(received));
    if et != rt {
        return false;
    }
    match (expected, received) {
        (JsValue::String(e), JsValue::String(r)) => {
            !e.is_empty() && !r.is_empty() && (e.contains('\n') || r.contains('\n'))
        }
        (JsValue::Error { .. }, _) => false,
        _ => !matches!(
            et,
            "undefined" | "null" | "boolean" | "number" | "bigint" | "date" | "regexp"
        ),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Op {
    Equal,
    Delete,
    Insert,
}

/// Line diff (LCS), deletions before insertions in each changed block.
fn diff_lines<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<(Op, &'a str)> {
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    let mut deletes = Vec::new();
    let mut inserts = Vec::new();
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            out.append(&mut deletes);
            out.append(&mut inserts);
            out.push((Op::Equal, a[i]));
            i += 1;
            j += 1;
        } else if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) {
            inserts.push((Op::Insert, b[j]));
            j += 1;
        } else {
            deletes.push((Op::Delete, a[i]));
            i += 1;
        }
    }
    out.append(&mut deletes);
    out.append(&mut inserts);
    out
}

fn print_line(indicator: char, line: &str) -> String {
    if !line.is_empty() {
        format!("{indicator} {line}")
    } else if indicator != ' ' {
        indicator.to_string()
    } else {
        String::new()
    }
}

/// `joinAlignedDiffsNoExpand` with 5 context lines.
fn join_no_expand(diffs: &[(Op, &str)]) -> String {
    const N: usize = 5;
    const N2: usize = 10;
    let len = diffs.len();
    let mut has_excess_at_ends = false;
    let mut excesses_between = 0;
    let mut i = 0;
    while i != len {
        let start = i;
        while i != len && diffs[i].0 == Op::Equal {
            i += 1;
        }
        if start != i {
            let count = i - start;
            if start == 0 || i == len {
                if count > N {
                    has_excess_at_ends = true;
                }
            } else if count > N2 {
                excesses_between += 1;
            }
        }
        while i != len && diffs[i].0 != Op::Equal {
            i += 1;
        }
    }
    let has_patch = excesses_between != 0 || has_excess_at_ends;
    let mut lines: Vec<String> = Vec::new();
    let mut patch_mark = 0;
    if has_patch {
        lines.push(String::new());
    }
    let (mut a_start, mut b_start, mut a_end, mut b_end) = (0usize, 0usize, 0usize, 0usize);
    let mark = |a_start: usize, a_end: usize, b_start: usize, b_end: usize| {
        format!(
            "@@ -{},{} +{},{} @@",
            a_start + 1,
            a_end - a_start,
            b_start + 1,
            b_end - b_start
        )
    };
    i = 0;
    while i != len {
        let mut start = i;
        while i != len && diffs[i].0 == Op::Equal {
            i += 1;
        }
        if start != i {
            if start == 0 {
                if i > N {
                    start = i - N;
                    a_start = start;
                    b_start = start;
                    a_end = a_start;
                    b_end = b_start;
                }
                for d in &diffs[start..i] {
                    lines.push(print_line(' ', d.1));
                    a_end += 1;
                    b_end += 1;
                }
            } else if i == len {
                let end = if i - start > N { start + N } else { i };
                for d in &diffs[start..end] {
                    lines.push(print_line(' ', d.1));
                    a_end += 1;
                    b_end += 1;
                }
            } else {
                let count = i - start;
                if count > N2 {
                    for d in &diffs[start..start + N] {
                        lines.push(print_line(' ', d.1));
                        a_end += 1;
                        b_end += 1;
                    }
                    lines[patch_mark] = mark(a_start, a_end, b_start, b_end);
                    patch_mark = lines.len();
                    lines.push(String::new());
                    let omit = count - N2;
                    a_start = a_end + omit;
                    b_start = b_end + omit;
                    a_end = a_start;
                    b_end = b_start;
                    for d in &diffs[i - N..i] {
                        lines.push(print_line(' ', d.1));
                        a_end += 1;
                        b_end += 1;
                    }
                } else {
                    for d in &diffs[start..i] {
                        lines.push(print_line(' ', d.1));
                        a_end += 1;
                        b_end += 1;
                    }
                }
            }
        }
        while i != len && diffs[i].0 == Op::Delete {
            lines.push(print_line('-', diffs[i].1));
            a_end += 1;
            i += 1;
        }
        while i != len && diffs[i].0 == Op::Insert {
            lines.push(print_line('+', diffs[i].1));
            b_end += 1;
            i += 1;
        }
    }
    if has_patch {
        lines[patch_mark] = mark(a_start, a_end, b_start, b_end);
    }
    lines.join("\n")
}

/// `diffLinesUnified` with `includeChangeCounts`.
fn diff_lines_unified(a: &str, b: &str, a_annotation: &str, b_annotation: &str) -> String {
    let a_lines: Vec<&str> = a.split('\n').collect();
    let b_lines: Vec<&str> = b.split('\n').collect();
    let diffs = diff_lines(&a_lines, &b_lines);
    let a_count = diffs
        .iter()
        .filter(|d| d.0 == Op::Delete)
        .count()
        .to_string();
    let b_count = diffs
        .iter()
        .filter(|d| d.0 == Op::Insert)
        .count()
        .to_string();
    let ann_diff = b_annotation.chars().count() as isize - a_annotation.chars().count() as isize;
    let a_ann_pad = " ".repeat(ann_diff.max(0) as usize);
    let b_ann_pad = " ".repeat((-ann_diff).max(0) as usize);
    let count_diff = b_count.len() as isize - a_count.len() as isize;
    let a_count_pad = " ".repeat(count_diff.max(0) as usize);
    let b_count_pad = " ".repeat((-count_diff).max(0) as usize);
    format!(
        "- {a_annotation}{a_ann_pad}  - {a_count_pad}{a_count}\n+ {b_annotation}{b_ann_pad}  + {b_count_pad}{b_count}\n\n{}",
        join_no_expand(&diffs)
    )
}

/// `printDiffOrStringify` of jest-matcher-utils (without colors).
pub(crate) fn print_diff_or_stringify(
    expected: &JsValue,
    received: &JsValue,
    expected_label: &str,
    received_label: &str,
) -> String {
    if let (JsValue::String(e), JsValue::String(r)) = (expected, received) {
        if !e.is_empty()
            && !r.is_empty()
            && e.len() <= MAX_DIFF_STRING_LENGTH
            && r.len() <= MAX_DIFF_STRING_LENGTH
            && e != r
        {
            if e.contains('\n') || r.contains('\n') {
                return diff_lines_unified(e, r, expected_label, received_label);
            }
            let print_label = label_printer(&[expected_label, received_label]);
            return format!(
                "{}{}\n{}{}",
                print_label(expected_label),
                stringify(expected),
                print_label(received_label),
                stringify(received)
            );
        }
    }
    if is_line_diffable(expected, received) {
        let a = pretty_for_diff(expected);
        let b = pretty_for_diff(received);
        if a != b {
            return diff_lines_unified(&a, &b, expected_label, received_label);
        }
    }
    let print_label = label_printer(&[expected_label, received_label]);
    let e = stringify(expected);
    let r = stringify(received);
    let received_text = if e == r {
        "serializes to the same string".to_string()
    } else {
        r
    };
    format!(
        "{}{}\n{}{}",
        print_label(expected_label),
        e,
        print_label(received_label),
        received_text
    )
}

/// What `formatMatcherMessage` prints.
#[derive(Debug, Default, Clone)]
pub(crate) struct MessageDetails {
    pub(crate) is_not: bool,
    /// Defaults to `locator` when `locator` is set, `page` otherwise.
    pub(crate) receiver: Option<String>,
    pub(crate) matcher_name: String,
    pub(crate) expectation: String,
    pub(crate) locator: Option<String>,
    pub(crate) printed_expected: Option<String>,
    pub(crate) printed_received: Option<String>,
    pub(crate) printed_diff: Option<String>,
    pub(crate) timed_out: bool,
    pub(crate) timeout: Option<u64>,
    pub(crate) error_message: Option<String>,
    pub(crate) log: Vec<String>,
}

/// `formatMatcherMessage` of `matcherHint.ts`.
pub(crate) fn format_matcher_message(details: MessageDetails) -> String {
    let mut d = details;
    let receiver = d.receiver.clone().unwrap_or_else(|| {
        if d.locator.is_some() {
            "locator".to_string()
        } else {
            "page".to_string()
        }
    });
    let mut message = format!(
        "expect({receiver}){}.{}({}) failed\n\n",
        if d.is_not { ".not" } else { "" },
        d.matcher_name,
        d.expectation
    );
    if let Some(diff) = &d.printed_diff {
        let lines: Vec<&str> = diff.split('\n').collect();
        if lines.len() == 2 {
            d.printed_expected = Some(lines[0].to_string());
            d.printed_received = Some(lines[1].to_string());
            d.printed_diff = None;
        }
    }
    let align = d.error_message.is_none()
        && d.printed_expected
            .as_deref()
            .is_some_and(|e| e.starts_with("Expected:"))
        && d.printed_received
            .as_deref()
            .is_none_or(|r| r.is_empty() || r.starts_with("Received:"));
    let pad = if align { " " } else { "" };
    if let Some(locator) = &d.locator {
        message += &format!("Locator: {pad}{locator}\n");
    }
    if let Some(e) = d.printed_expected.as_deref().filter(|e| !e.is_empty()) {
        message += e;
        message.push('\n');
    }
    if let Some(r) = d.printed_received.as_deref().filter(|r| !r.is_empty()) {
        message += r;
        message.push('\n');
    }
    if d.timed_out {
        if let Some(timeout) = d.timeout.filter(|t| *t > 0) {
            message += &format!("Timeout: {pad}{timeout}ms\n");
        }
    }
    if let Some(diff) = &d.printed_diff {
        message += diff;
        message.push('\n');
    }
    if let Some(error) = &d.error_message {
        message += error;
        if !error.ends_with('\n') {
            message.push('\n');
        }
    }
    message += &call_log_text(&d.log);
    message
}

/// `callLogText`.
pub(crate) fn call_log_text(log: &[String]) -> String {
    if !log.iter().any(|l| !l.is_empty()) {
        return String::new();
    }
    format!("\nCall log:\n{}\n", log.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> JsValue {
        JsValue::String(v.to_string())
    }

    #[test]
    fn stringify_values() {
        assert_eq!(stringify(&s("a\"b")), "\"a\\\"b\"");
        assert_eq!(
            stringify(&JsValue::Array(vec![s("a"), JsValue::Number(1.0)])),
            "[\"a\", 1]"
        );
        assert_eq!(
            stringify(&JsValue::Object(vec![
                ("b".into(), JsValue::Bool(true)),
                ("a".into(), JsValue::Null)
            ])),
            "{\"a\": null, \"b\": true}"
        );
        assert_eq!(stringify(&JsValue::Number(-0.0)), "-0");
    }

    #[test]
    fn diff_or_stringify_strings() {
        assert_eq!(
            print_diff_or_stringify(&s("hey"), &s("Text content"), "Expected", "Received"),
            "Expected: \"hey\"\nReceived: \"Text content\""
        );
        assert_eq!(
            print_diff_or_stringify(&s(""), &s(""), "Expected", "Received"),
            "Expected: \"\"\nReceived: serializes to the same string"
        );
        assert_eq!(
            print_diff_or_stringify(
                &JsValue::RegExp {
                    source: ".*".into(),
                    flags: String::new()
                },
                &s(""),
                "Expected pattern",
                "Received string"
            ),
            "Expected pattern: /.*/\nReceived string:  \"\""
        );
    }

    #[test]
    fn diff_arrays() {
        let expected = JsValue::Array(vec![s("Text 1"), s("Text 3"), s("Extra")]);
        let received = JsValue::Array(vec![s("Text 1"), s("Text 3")]);
        assert_eq!(
            print_diff_or_stringify(&expected, &received, "Expected", "Received"),
            "- Expected  - 1\n+ Received  + 0\n\n  Array [\n    \"Text 1\",\n    \"Text 3\",\n-   \"Extra\",\n  ]"
        );
    }

    #[test]
    fn diff_with_patch_marks() {
        let expected = JsValue::Array((0..30).map(|i| JsValue::Number(i as f64)).collect());
        let mut items: Vec<JsValue> = (0..30).map(|i| JsValue::Number(i as f64)).collect();
        items[15] = JsValue::Number(100.0);
        let received = JsValue::Array(items);
        let diff = print_diff_or_stringify(&expected, &received, "Expected", "Received");
        assert!(
            diff.starts_with("- Expected  - 1\n+ Received  + 1\n\n@@ -12,11 +12,11 @@\n"),
            "{diff}"
        );
        assert!(diff.contains("-   15,\n+   100,"), "{diff}");
    }
}
