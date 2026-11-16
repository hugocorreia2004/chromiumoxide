//! Web-first assertions: a port of Playwright Test's `expect` for locators,
//! pages and API responses (`packages/playwright/src/matchers/`), without
//! the test runner.
//!
//! ```no_run
//! # async fn run(page: oxidewright::Page) -> oxidewright::Result<()> {
//! use oxidewright::expect::expect;
//! use oxidewright::regex;
//!
//! let button = page.get_by_role("button");
//! expect(&button).to_be_visible().await?;
//! expect(&button).not().to_have_text("Loading").timeout(1_000).await?;
//! expect(&page.locator("li")).to_have_text(["one", "two"]).await?;
//! expect(&page).to_have_url(regex(r"/done$", "")).await?;
//! expect(&page).to_have_title("Done").await?;
//! # Ok(()) }
//! ```
//!
//! Assertions retry until they pass or their timeout (5 seconds by default,
//! see [`set_default_timeout`]) expires. Like Playwright, each check runs the
//! injected script's `expect()` in the page: the first check is made without
//! a deadline, the next ones with `retryWithProgressAndBackoff`'s delays
//! (`0, 20, 50, 100, 100, 500` ms, then every 500 ms).
//!
//! A failed assertion returns [`Error::Expect`](crate::Error::Expect) with a
//! [`MatcherResult`] whose message mirrors Playwright's:
//!
//! ```text
//! expect(locator).toHaveText(expected) failed
//!
//! Locator:  locator('#node')
//! Expected: "hey"
//! Received: "Text content"
//! Timeout:  1000ms
//!
//! Call log:
//!   - Expect "toHaveText" locator('#node') with timeout 1000ms
//!   - waiting for locator('#node')
//!   -   locator resolved to <div id="node">Text content</div>
//!   -   unexpected value "Text content"
//! ```
//!
//! Options follow the Playwright names: `.not()`, `.message("...")` and
//! `.timeout(ms)` on the value returned by [`expect`] (like `expect(x, msg)`
//! and `expect.configure({ timeout })`), and matcher options as builder
//! methods before `.await` (`.timeout(ms)`, `.ignore_case(true)`,
//! `.use_inner_text(true)`, `.checked(false)`, `.ratio(0.5)`, ...).
//!
//! Not ported: soft assertions, `expect.poll`, `expect.extend`, the
//! `AbortSignal` option (drop the future instead) and `URLPattern`
//! expectations. See also [`LocatorAssertions::to_match_aria_snapshot`] and
//! [`PageAssertions::to_have_screenshot`] for their limitations.

mod aria;
mod engine;
mod format;
mod screenshot;

use std::future::{Future, IntoFuture};
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::frame_selectors::Backoff;
use crate::js::JsValue;
use crate::locator::Locator;
use crate::locator_utils::{TextMatch, escape_with_quotes};
use crate::page::Page;

use engine::{ExpectOutcome, ExpectParams, frame_expect, js_string};
use format::{MessageDetails, format_matcher_message, print_diff_or_stringify, stringify};

pub use aria::ToMatchAriaSnapshot;
pub use screenshot::ToHaveScreenshot;

static DEFAULT_TIMEOUT: AtomicU64 = AtomicU64::new(5_000);

/// Default timeout of assertions in milliseconds (Playwright Test's
/// `expect.timeout`, 5000 by default). `0` disables the timeout.
pub fn set_default_timeout(timeout_ms: u64) {
    DEFAULT_TIMEOUT.store(timeout_ms, Ordering::SeqCst);
}

/// The current default timeout of assertions in milliseconds.
pub fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT.load(Ordering::SeqCst)
}

/// The result of a failed assertion, carried by
/// [`Error::Expect`](crate::Error::Expect) (Playwright's `matcherResult`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MatcherResult {
    /// Matcher name, e.g. `toHaveText`.
    pub name: String,
    /// What the check found: `true` when a `.not()` assertion failed.
    pub pass: bool,
    /// The expected value, if the matcher has one.
    pub expected: Option<JsValue>,
    /// The received value, if any.
    pub actual: Option<JsValue>,
    /// The full message (custom message first, when set).
    pub message: String,
    /// Call log lines.
    pub log: Vec<String>,
    /// The timeout, when the assertion timed out.
    pub timeout: Option<u64>,
    /// An ARIA snapshot of the element (or of the page) when the check failed.
    pub aria_snapshot: Option<String>,
}

// ----------------------------------------------------------------------
// expect()
// ----------------------------------------------------------------------

/// What [`expect`] accepts: locators, pages and API responses.
pub trait Expectable {
    /// The assertions available for this value.
    type Assertions;
    /// Create the assertions.
    fn assertions(&self) -> Self::Assertions;
}

/// Create assertions for a [`Locator`], a [`Page`] or an API response.
pub fn expect<T: Expectable + ?Sized>(value: &T) -> T::Assertions {
    value.assertions()
}

#[derive(Debug, Clone, Default)]
struct Config {
    is_not: bool,
    message: Option<String>,
    timeout: Option<u64>,
}

impl Config {
    fn timeout(&self, explicit: Option<u64>) -> u64 {
        explicit.or(self.timeout).unwrap_or_else(default_timeout)
    }

    fn title(&self, matcher_name: &str) -> String {
        match &self.message {
            Some(m) => m.clone(),
            None => {
                let title = format!("{}{matcher_name}", if self.is_not { "not " } else { "" });
                format!("Expect {}", escape_with_quotes(&title, '"'))
            }
        }
    }

    fn fail(&self, mut result: MatcherResult) -> Error {
        if let Some(custom) = &self.message {
            result.message = format!("{custom}\n\n{}", result.message);
        }
        Error::Expect(Box::new(result))
    }
}

macro_rules! config_methods {
    () => {
        /// Negate the assertions (`expect(x).not`).
        #[allow(clippy::should_implement_trait)]
        pub fn not(mut self) -> Self {
            self.cfg.is_not = !self.cfg.is_not;
            self
        }

        /// A custom message, printed before the failure message and used as
        /// the call log title (`expect(x, message)`).
        pub fn message(mut self, message: impl Into<String>) -> Self {
            self.cfg.message = Some(message.into());
            self
        }

        /// Timeout of the assertions in milliseconds
        /// (`expect.configure({ timeout })`).
        pub fn timeout(mut self, timeout_ms: u64) -> Self {
            self.cfg.timeout = Some(timeout_ms);
            self
        }
    };
}

/// Assertions on a [`Locator`] (Playwright's `LocatorAssertions`).
#[derive(Debug, Clone)]
pub struct LocatorAssertions {
    locator: Locator,
    cfg: Config,
}

/// Assertions on a [`Page`] (Playwright's `PageAssertions`).
#[derive(Clone)]
pub struct PageAssertions {
    page: Page,
    cfg: Config,
}

/// Assertions on an API response (Playwright's `APIResponseAssertions`).
pub struct APIResponseAssertions {
    response: Box<dyn ResponseLike>,
    cfg: Config,
}

impl Expectable for Locator {
    type Assertions = LocatorAssertions;
    fn assertions(&self) -> LocatorAssertions {
        LocatorAssertions {
            locator: self.clone(),
            cfg: Config::default(),
        }
    }
}

impl Expectable for Page {
    type Assertions = PageAssertions;
    fn assertions(&self) -> PageAssertions {
        PageAssertions {
            page: self.clone(),
            cfg: Config::default(),
        }
    }
}

/// What `to_be_ok` needs from a response.
pub trait ResponseLike: Send + Sync {
    /// Whether the status is in `200..=299`.
    fn is_ok(&self) -> bool;
    /// The `content-type` header.
    fn content_type(&self) -> Option<String>;
    /// The body as text.
    fn body_text(&self) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>>;
    /// The request log (`→ GET url`, headers, `← 200 OK`, headers).
    fn request_log(&self) -> Vec<String>;
}

impl ResponseLike for crate::fetch::APIResponse {
    fn is_ok(&self) -> bool {
        self.ok()
    }
    fn content_type(&self) -> Option<String> {
        self.header_value("content-type")
    }
    fn body_text(&self) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>> {
        Box::pin(async move { self.text().await.ok() })
    }
    fn request_log(&self) -> Vec<String> {
        self.fetch_log()
    }
}

impl ResponseLike for crate::route::APIResponse {
    fn is_ok(&self) -> bool {
        self.ok()
    }
    fn content_type(&self) -> Option<String> {
        self.headers().get("content-type").cloned()
    }
    fn body_text(&self) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>> {
        Box::pin(async move { self.text().await.ok() })
    }
    fn request_log(&self) -> Vec<String> {
        Vec::new()
    }
}

impl Expectable for crate::fetch::APIResponse {
    type Assertions = APIResponseAssertions;
    fn assertions(&self) -> APIResponseAssertions {
        APIResponseAssertions {
            response: Box::new(self.clone()),
            cfg: Config::default(),
        }
    }
}

impl Expectable for crate::route::APIResponse {
    type Assertions = APIResponseAssertions;
    fn assertions(&self) -> APIResponseAssertions {
        APIResponseAssertions {
            response: Box::new(self.clone()),
            cfg: Config::default(),
        }
    }
}

// ----------------------------------------------------------------------
// Expected values
// ----------------------------------------------------------------------

/// Expected text of `to_have_text` / `to_contain_text` / `to_have_class`:
/// one string or regular expression, or a list (one per element).
#[derive(Debug, Clone)]
pub enum ExpectedText {
    /// One value, for a single element.
    One(TextMatch),
    /// One value per element, in order.
    Many(Vec<TextMatch>),
}

impl<T: Into<TextMatch>> From<T> for ExpectedText {
    fn from(v: T) -> Self {
        ExpectedText::One(v.into())
    }
}

impl<T: Into<TextMatch>> From<Vec<T>> for ExpectedText {
    fn from(v: Vec<T>) -> Self {
        ExpectedText::Many(v.into_iter().map(Into::into).collect())
    }
}

impl<T: Into<TextMatch>, const N: usize> From<[T; N]> for ExpectedText {
    fn from(v: [T; N]) -> Self {
        ExpectedText::Many(v.into_iter().map(Into::into).collect())
    }
}

impl<T: Into<TextMatch> + Clone> From<&[T]> for ExpectedText {
    fn from(v: &[T]) -> Self {
        ExpectedText::Many(v.iter().cloned().map(Into::into).collect())
    }
}

/// Expected classes of `to_contain_class`: a space separated list of
/// classes, or a list of them (one per element). Regular expressions are
/// not accepted, like in Playwright.
#[derive(Debug, Clone)]
pub enum ExpectedClasses {
    /// For a single element.
    One(String),
    /// One entry per element, in order.
    Many(Vec<String>),
}

impl From<&str> for ExpectedClasses {
    fn from(v: &str) -> Self {
        ExpectedClasses::One(v.to_string())
    }
}

impl From<String> for ExpectedClasses {
    fn from(v: String) -> Self {
        ExpectedClasses::One(v)
    }
}

impl<T: Into<String>> From<Vec<T>> for ExpectedClasses {
    fn from(v: Vec<T>) -> Self {
        ExpectedClasses::Many(v.into_iter().map(Into::into).collect())
    }
}

impl<T: Into<String>, const N: usize> From<[T; N]> for ExpectedClasses {
    fn from(v: [T; N]) -> Self {
        ExpectedClasses::Many(v.into_iter().map(Into::into).collect())
    }
}

fn text_to_js(t: &TextMatch) -> JsValue {
    match t {
        TextMatch::Text(s) => JsValue::String(s.clone()),
        TextMatch::Regex(re) => JsValue::RegExp {
            source: re.source().to_string(),
            flags: re.flags().to_string(),
        },
    }
}

/// `serializeExpectedTextValues`.
fn serialize_expected(
    items: &[TextMatch],
    match_substring: bool,
    normalize_white_space: bool,
    ignore_case: Option<bool>,
) -> Vec<JsValue> {
    items
        .iter()
        .map(|item| {
            let mut entries = Vec::new();
            match item {
                TextMatch::Text(s) => {
                    entries.push(("string".to_string(), JsValue::String(s.clone())))
                }
                TextMatch::Regex(re) => {
                    entries.push((
                        "regexSource".to_string(),
                        JsValue::String(re.source().to_string()),
                    ));
                    entries.push((
                        "regexFlags".to_string(),
                        JsValue::String(re.flags().to_string()),
                    ));
                }
            }
            if match_substring {
                entries.push(("matchSubstring".to_string(), JsValue::Bool(true)));
            }
            if let Some(i) = ignore_case {
                entries.push(("ignoreCase".to_string(), JsValue::Bool(i)));
            }
            if normalize_white_space {
                entries.push(("normalizeWhiteSpace".to_string(), JsValue::Bool(true)));
            }
            JsValue::Object(entries)
        })
        .collect()
}

// ----------------------------------------------------------------------
// Matcher builders
// ----------------------------------------------------------------------

/// Option sets of the matcher builders.
pub mod kinds {
    macro_rules! kind {
        ($($(#[$m:meta])* $name:ident),* $(,)?) => {
            $($(#[$m])* #[derive(Debug, Clone, Copy)] pub struct $name;)*
        };
    }
    kind!(
        /// Only `timeout`.
        Plain,
        /// `timeout`, `ignore_case`.
        IgnoreCase,
        /// `timeout`, `ignore_case`, `use_inner_text`.
        Text,
        /// `timeout`, `checked`, `indeterminate`.
        Checked,
        /// `timeout`, `visible`.
        Visible,
        /// `timeout`, `attached`.
        Attached,
        /// `timeout`, `editable`.
        Editable,
        /// `timeout`, `enabled`.
        Enabled,
        /// `timeout`, `ratio`.
        InViewport,
        /// `timeout`, `pseudo`.
        Css,
    );
}

#[derive(Debug, Clone)]
enum Subject {
    Locator(Locator),
    Page(Page),
}

#[derive(Debug, Clone)]
enum Spec {
    /// toBeTruthy-based.
    State(&'static str),
    AttributePresent(String),
    Text {
        name: &'static str,
        expected: ExpectedText,
    },
    Classes {
        expected: ExpectedClasses,
    },
    Attribute {
        name: String,
        expected: TextMatch,
    },
    Css {
        name: String,
        expected: TextMatch,
    },
    Count(usize),
    Property {
        name: String,
        expected: JsValue,
    },
    Values(Vec<TextMatch>),
    Title(TextMatch),
    Url(TextMatch),
}

#[derive(Debug, Clone, Default)]
struct Options {
    timeout: Option<u64>,
    ignore_case: Option<bool>,
    use_inner_text: Option<bool>,
    flag: Option<bool>,
    indeterminate: Option<bool>,
    ratio: Option<f64>,
    pseudo: Option<String>,
}

/// A pending assertion; set options, then `.await` it.
#[must_use = "assertions do nothing unless awaited"]
#[derive(Debug, Clone)]
pub struct Assertion<K> {
    subject: Subject,
    cfg: Config,
    spec: Spec,
    opts: Options,
    _kind: PhantomData<K>,
}

impl<K> Assertion<K> {
    fn new(subject: Subject, cfg: Config, spec: Spec) -> Self {
        Assertion {
            subject,
            cfg,
            spec,
            opts: Options::default(),
            _kind: PhantomData,
        }
    }

    /// Timeout of this assertion in milliseconds.
    pub fn timeout(mut self, timeout_ms: u64) -> Self {
        self.opts.timeout = Some(timeout_ms);
        self
    }
}

macro_rules! option {
    ($kind:ident: $($(#[$m:meta])* $name:ident($ty:ty) => $field:ident),* $(,)?) => {
        impl Assertion<kinds::$kind> {
            $(
                $(#[$m])*
                pub fn $name(mut self, value: $ty) -> Self {
                    self.opts.$field = Some(value.into());
                    self
                }
            )*
        }
    };
}

option!(IgnoreCase:
    /// Case-insensitive match (overrides the `i` flag of a regular expression).
    ignore_case(bool) => ignore_case,
);
option!(Text:
    /// Case-insensitive match (overrides the `i` flag of a regular expression).
    ignore_case(bool) => ignore_case,
    /// Use `element.innerText` instead of `element.textContent`.
    use_inner_text(bool) => use_inner_text,
);
option!(Checked:
    /// `false` asserts that the element is unchecked.
    checked(bool) => flag,
    /// Assert the indeterminate (mixed) state.
    indeterminate(bool) => indeterminate,
);
option!(Visible:
    /// `false` asserts that the element is hidden.
    visible(bool) => flag,
);
option!(Attached:
    /// `false` asserts that the element is detached.
    attached(bool) => flag,
);
option!(Editable:
    /// `false` asserts that the element is read-only.
    editable(bool) => flag,
);
option!(Enabled:
    /// `false` asserts that the element is disabled.
    enabled(bool) => flag,
);
option!(InViewport:
    /// Minimal ratio of the element that must intersect the viewport.
    ratio(f64) => ratio,
);
option!(Css:
    /// Pseudo element: `"before"` or `"after"`.
    pseudo(&str) => pseudo,
);

impl<K: Send + 'static> IntoFuture for Assertion<K> {
    type Output = Result<()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Result<()>> + Send>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { self.run().await })
    }
}

impl LocatorAssertions {
    config_methods!();

    fn assertion<K>(&self, spec: Spec) -> Assertion<K> {
        Assertion::new(
            Subject::Locator(self.locator.clone()),
            self.cfg.clone(),
            spec,
        )
    }

    /// The element is attached to the DOM.
    pub fn to_be_attached(&self) -> Assertion<kinds::Attached> {
        self.assertion(Spec::State("toBeAttached"))
    }
    /// The checkbox or radio is checked (see `checked` and `indeterminate`).
    pub fn to_be_checked(&self) -> Assertion<kinds::Checked> {
        self.assertion(Spec::State("toBeChecked"))
    }
    /// The element is disabled.
    pub fn to_be_disabled(&self) -> Assertion<kinds::Plain> {
        self.assertion(Spec::State("toBeDisabled"))
    }
    /// The element is editable.
    pub fn to_be_editable(&self) -> Assertion<kinds::Editable> {
        self.assertion(Spec::State("toBeEditable"))
    }
    /// The element is an empty editable element or has no text.
    pub fn to_be_empty(&self) -> Assertion<kinds::Plain> {
        self.assertion(Spec::State("toBeEmpty"))
    }
    /// The element is enabled.
    pub fn to_be_enabled(&self) -> Assertion<kinds::Enabled> {
        self.assertion(Spec::State("toBeEnabled"))
    }
    /// The element is focused.
    pub fn to_be_focused(&self) -> Assertion<kinds::Plain> {
        self.assertion(Spec::State("toBeFocused"))
    }
    /// The element is hidden or does not exist.
    pub fn to_be_hidden(&self) -> Assertion<kinds::Plain> {
        self.assertion(Spec::State("toBeHidden"))
    }
    /// The element intersects the viewport.
    pub fn to_be_in_viewport(&self) -> Assertion<kinds::InViewport> {
        self.assertion(Spec::State("toBeInViewport"))
    }
    /// The element is visible.
    pub fn to_be_visible(&self) -> Assertion<kinds::Visible> {
        self.assertion(Spec::State("toBeVisible"))
    }

    /// The element contains `expected` text (substring, whitespace
    /// normalized), or the elements contain the texts of a list in order.
    pub fn to_contain_text(&self, expected: impl Into<ExpectedText>) -> Assertion<kinds::Text> {
        self.assertion(Spec::Text {
            name: "toContainText",
            expected: expected.into(),
        })
    }
    /// The element has exactly `expected` text (whitespace normalized), or
    /// the elements have the texts of a list.
    pub fn to_have_text(&self, expected: impl Into<ExpectedText>) -> Assertion<kinds::Text> {
        self.assertion(Spec::Text {
            name: "toHaveText",
            expected: expected.into(),
        })
    }
    /// The element has the accessible description.
    pub fn to_have_accessible_description(
        &self,
        expected: impl Into<TextMatch>,
    ) -> Assertion<kinds::IgnoreCase> {
        self.assertion(Spec::Text {
            name: "toHaveAccessibleDescription",
            expected: ExpectedText::One(expected.into()),
        })
    }
    /// The element has the accessible error message.
    pub fn to_have_accessible_error_message(
        &self,
        expected: impl Into<TextMatch>,
    ) -> Assertion<kinds::IgnoreCase> {
        self.assertion(Spec::Text {
            name: "toHaveAccessibleErrorMessage",
            expected: ExpectedText::One(expected.into()),
        })
    }
    /// The element has the accessible name.
    pub fn to_have_accessible_name(
        &self,
        expected: impl Into<TextMatch>,
    ) -> Assertion<kinds::IgnoreCase> {
        self.assertion(Spec::Text {
            name: "toHaveAccessibleName",
            expected: ExpectedText::One(expected.into()),
        })
    }
    /// The element has the attribute `name` with the value `expected`.
    pub fn to_have_attribute(
        &self,
        name: impl Into<String>,
        expected: impl Into<TextMatch>,
    ) -> Assertion<kinds::IgnoreCase> {
        self.assertion(Spec::Attribute {
            name: name.into(),
            expected: expected.into(),
        })
    }
    /// The element has the attribute `name`, whatever its value
    /// (`toHaveAttribute(name)` without a value).
    pub fn to_have_attribute_present(&self, name: impl Into<String>) -> Assertion<kinds::Plain> {
        self.assertion(Spec::AttributePresent(name.into()))
    }
    /// The element's `class` attribute matches (full match for strings), or
    /// the elements' classes match the list.
    pub fn to_have_class(&self, expected: impl Into<ExpectedText>) -> Assertion<kinds::Plain> {
        self.assertion(Spec::Text {
            name: "toHaveClass",
            expected: expected.into(),
        })
    }
    /// The element has all the given classes, or the elements have the
    /// classes of the list.
    pub fn to_contain_class(
        &self,
        expected: impl Into<ExpectedClasses>,
    ) -> Assertion<kinds::Plain> {
        self.assertion(Spec::Classes {
            expected: expected.into(),
        })
    }
    /// The locator resolves to exactly `count` elements.
    pub fn to_have_count(&self, count: usize) -> Assertion<kinds::Plain> {
        self.assertion(Spec::Count(count))
    }
    /// The computed style property `name` has the value `expected`.
    pub fn to_have_css(
        &self,
        name: impl Into<String>,
        expected: impl Into<TextMatch>,
    ) -> Assertion<kinds::Css> {
        self.assertion(Spec::Css {
            name: name.into(),
            expected: expected.into(),
        })
    }
    /// The element has the id.
    pub fn to_have_id(&self, expected: impl Into<TextMatch>) -> Assertion<kinds::Plain> {
        self.assertion(Spec::Text {
            name: "toHaveId",
            expected: ExpectedText::One(expected.into()),
        })
    }
    /// The JavaScript property `name` (dotted paths allowed) of the element
    /// deep-equals `expected`.
    pub fn to_have_js_property(
        &self,
        name: impl Into<String>,
        expected: impl Serialize,
    ) -> Assertion<kinds::Plain> {
        let expected = match crate::js::serde_impl::to_wire(&expected) {
            Ok((wire, _)) => JsValue::from_wire(&wire),
            Err(_) => JsValue::Undefined,
        };
        self.assertion(Spec::Property {
            name: name.into(),
            expected,
        })
    }
    /// The element has the ARIA role.
    pub fn to_have_role(&self, role: &str) -> Assertion<kinds::Plain> {
        self.assertion(Spec::Text {
            name: "toHaveRole",
            expected: ExpectedText::One(TextMatch::Text(role.to_string())),
        })
    }
    /// The input (or the control of a label) has the value.
    pub fn to_have_value(&self, expected: impl Into<TextMatch>) -> Assertion<kinds::Plain> {
        self.assertion(Spec::Text {
            name: "toHaveValue",
            expected: ExpectedText::One(expected.into()),
        })
    }
    /// The `<select multiple>` has exactly these options selected.
    pub fn to_have_values<T: Into<TextMatch>>(
        &self,
        expected: impl IntoIterator<Item = T>,
    ) -> Assertion<kinds::Plain> {
        self.assertion(Spec::Values(expected.into_iter().map(Into::into).collect()))
    }
    /// The element matches the ARIA snapshot template (YAML).
    pub fn to_match_aria_snapshot(&self, expected: impl Into<String>) -> ToMatchAriaSnapshot {
        ToMatchAriaSnapshot::new(
            aria::Receiver::Locator(self.locator.clone()),
            self.cfg.clone(),
            expected.into(),
        )
    }
    /// The element looks like the PNG stored at `path`.
    pub fn to_have_screenshot(&self, path: impl Into<std::path::PathBuf>) -> ToHaveScreenshot {
        ToHaveScreenshot::new(
            screenshot::Shot::Locator(self.locator.clone()),
            self.cfg.clone(),
            path.into(),
        )
    }
}

/// Expected URL of [`PageAssertions::to_have_url`].
pub type ExpectedUrl = TextMatch;

impl PageAssertions {
    config_methods!();

    fn assertion<K>(&self, spec: Spec) -> Assertion<K> {
        Assertion::new(Subject::Page(self.page.clone()), self.cfg.clone(), spec)
    }

    /// The page title matches (whitespace normalized).
    pub fn to_have_title(&self, expected: impl Into<TextMatch>) -> Assertion<kinds::Plain> {
        self.assertion(Spec::Title(expected.into()))
    }

    /// The page URL matches; strings are resolved against the context's
    /// `base_url`.
    pub fn to_have_url(&self, expected: impl Into<ExpectedUrl>) -> Assertion<kinds::IgnoreCase> {
        self.assertion(Spec::Url(expected.into()))
    }

    /// The page URL satisfies `predicate` (`toHaveURL(url => ...)`).
    pub fn to_have_url_matching<F>(&self, predicate: F) -> ToHaveUrlPredicate
    where
        F: Fn(&str) -> bool + Send + Sync + 'static,
    {
        ToHaveUrlPredicate {
            page: self.page.clone(),
            cfg: self.cfg.clone(),
            predicate: Box::new(predicate),
            timeout: None,
        }
    }

    /// The page body matches the ARIA snapshot template (YAML).
    pub fn to_match_aria_snapshot(&self, expected: impl Into<String>) -> ToMatchAriaSnapshot {
        ToMatchAriaSnapshot::new(
            aria::Receiver::Page(self.page.clone()),
            self.cfg.clone(),
            expected.into(),
        )
    }

    /// The page looks like the PNG stored at `path`.
    pub fn to_have_screenshot(&self, path: impl Into<std::path::PathBuf>) -> ToHaveScreenshot {
        ToHaveScreenshot::new(
            screenshot::Shot::Page(self.page.clone()),
            self.cfg.clone(),
            path.into(),
        )
    }
}

impl APIResponseAssertions {
    /// Negate the assertion.
    #[allow(clippy::should_implement_trait)]
    pub fn not(mut self) -> Self {
        self.cfg.is_not = !self.cfg.is_not;
        self
    }

    /// A custom message, printed before the failure message.
    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.cfg.message = Some(message.into());
        self
    }

    /// The status is in `200..=299`. Not retried. On failure the message
    /// contains the request log and, for textual content types, the first
    /// 1000 characters of the body.
    pub async fn to_be_ok(&self) -> Result<()> {
        let ok = self.response.is_ok();
        if ok != self.cfg.is_not {
            return Ok(());
        }
        let log = self.response.request_log();
        let text = match self.response.content_type() {
            Some(ct) if is_textual_mime_type(&ct) => self.response.body_text().await,
            _ => None,
        };
        let mut message = format_matcher_message(MessageDetails {
            is_not: self.cfg.is_not,
            receiver: Some("response".to_string()),
            matcher_name: "toBeOK".to_string(),
            expectation: String::new(),
            log: log.clone(),
            ..Default::default()
        });
        if let Some(text) = text {
            message += &format!(
                "\nResponse text:\n{}",
                text.chars().take(1000).collect::<String>()
            );
        }
        Err(self.cfg.fail(MatcherResult {
            name: "toBeOK".to_string(),
            pass: ok,
            expected: None,
            actual: None,
            message,
            log,
            timeout: None,
            aria_snapshot: None,
        }))
    }
}

/// `isTextualMimeType`.
fn is_textual_mime_type(content_type: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r"^(text/.*?|application/(json|(x-)?javascript|xml.*?|ecmascript|graphql|x-www-form-urlencoded)|image/svg(\+xml)?|application/.*?(\+json|\+xml))(;\s*charset=.*)?$",
        )
        .unwrap()
    })
    .is_match(content_type)
}

// ----------------------------------------------------------------------
// Running the matchers
// ----------------------------------------------------------------------

/// How the failure message is built.
enum Style {
    /// `toBeTruthy.ts`: expected state, arguments of the matcher.
    Truthy { expected: String, arg: String },
    /// `toMatchText.ts`.
    Text {
        expected: TextMatch,
        match_substring: bool,
    },
    /// `toEqual.ts`.
    Equal { expected: JsValue },
}

impl<K> Assertion<K> {
    async fn run(self) -> Result<()> {
        let cfg = &self.cfg;
        let opts = &self.opts;
        let (frame, selector, locator_text) = match &self.subject {
            Subject::Locator(l) => {
                if let Some(e) = l.construction_error() {
                    return Err(Error::invalid_argument(e));
                }
                (
                    l.frame().clone(),
                    Some(l.selector().to_string()),
                    Some(l.to_string()),
                )
            }
            Subject::Page(p) => (p.main_frame(), None, None),
        };
        let ignore_case = opts.ignore_case;
        let mut params = ExpectParams::new("");
        params.selector = selector;
        params.is_not = cfg.is_not;
        params.use_inner_text = opts.use_inner_text;
        let (name, style): (&'static str, Style) = match &self.spec {
            Spec::State(name) => {
                let flag = opts.flag.unwrap_or(true);
                let (expression, expected, arg): (&'static str, &str, String) = match *name {
                    "toBeAttached" => (
                        if flag {
                            "to.be.attached"
                        } else {
                            "to.be.detached"
                        },
                        if flag { "attached" } else { "detached" },
                        if flag { "" } else { "{ attached: false }" }.to_string(),
                    ),
                    "toBeChecked" => {
                        let mut value = Vec::new();
                        if let Some(c) = opts.flag {
                            value.push(("checked".to_string(), JsValue::Bool(c)));
                        }
                        if let Some(i) = opts.indeterminate {
                            value.push(("indeterminate".to_string(), JsValue::Bool(i)));
                        }
                        params.expected_value = Some(JsValue::Object(value));
                        if opts.indeterminate == Some(true) {
                            (
                                "to.be.checked",
                                "indeterminate",
                                "{ indeterminate: true }".into(),
                            )
                        } else if opts.flag == Some(false) {
                            ("to.be.checked", "unchecked", "{ checked: false }".into())
                        } else {
                            ("to.be.checked", "checked", String::new())
                        }
                    }
                    "toBeDisabled" => ("to.be.disabled", "disabled", String::new()),
                    "toBeEditable" => (
                        if flag {
                            "to.be.editable"
                        } else {
                            "to.be.readonly"
                        },
                        if flag { "editable" } else { "readOnly" },
                        if flag { "" } else { "{ editable: false }" }.to_string(),
                    ),
                    "toBeEmpty" => ("to.be.empty", "empty", String::new()),
                    "toBeEnabled" => (
                        if flag {
                            "to.be.enabled"
                        } else {
                            "to.be.disabled"
                        },
                        if flag { "enabled" } else { "disabled" },
                        if flag { "" } else { "{ enabled: false }" }.to_string(),
                    ),
                    "toBeFocused" => ("to.be.focused", "focused", String::new()),
                    "toBeHidden" => ("to.be.hidden", "hidden", String::new()),
                    "toBeVisible" => (
                        if flag {
                            "to.be.visible"
                        } else {
                            "to.be.hidden"
                        },
                        if flag { "visible" } else { "hidden" },
                        if flag { "" } else { "{ visible: false }" }.to_string(),
                    ),
                    "toBeInViewport" => {
                        params.expected_number = opts.ratio;
                        ("to.be.in.viewport", "in viewport", String::new())
                    }
                    other => unreachable!("unknown matcher {other}"),
                };
                params.expression = expression;
                (
                    name,
                    Style::Truthy {
                        expected: expected.to_string(),
                        arg,
                    },
                )
            }
            Spec::AttributePresent(attr) => {
                params.expression = "to.have.attribute";
                params.expression_arg = Some(attr.clone());
                (
                    "toHaveAttribute",
                    Style::Truthy {
                        expected: "have attribute".to_string(),
                        arg: String::new(),
                    },
                )
            }
            Spec::Text { name, expected } => {
                let (normalize, substring) = match *name {
                    "toHaveText" => (true, false),
                    "toContainText" => (true, true),
                    "toHaveAccessibleName"
                    | "toHaveAccessibleDescription"
                    | "toHaveAccessibleErrorMessage" => (true, false),
                    _ => (false, false),
                };
                let ignore_case = if normalize { ignore_case } else { None };
                match expected {
                    ExpectedText::One(t) => {
                        params.expression = match *name {
                            "toHaveText" | "toContainText" => "to.have.text",
                            "toHaveAccessibleName" => "to.have.accessible.name",
                            "toHaveAccessibleDescription" => "to.have.accessible.description",
                            "toHaveAccessibleErrorMessage" => "to.have.accessible.error.message",
                            "toHaveClass" => "to.have.class",
                            "toHaveId" => "to.have.id",
                            "toHaveRole" => "to.have.role",
                            "toHaveValue" => "to.have.value",
                            other => unreachable!("unknown matcher {other}"),
                        };
                        params.expected_text = Some(serialize_expected(
                            std::slice::from_ref(t),
                            substring,
                            normalize,
                            ignore_case,
                        ));
                        (
                            *name,
                            Style::Text {
                                expected: t.clone(),
                                match_substring: substring,
                            },
                        )
                    }
                    ExpectedText::Many(items) => {
                        params.expression = match *name {
                            "toHaveText" => "to.have.text.array",
                            "toContainText" => "to.contain.text.array",
                            "toHaveClass" => "to.have.class.array",
                            other => {
                                return Err(Error::invalid_argument(format!(
                                    "{other} does not accept a list of expected values"
                                )));
                            }
                        };
                        params.expected_text =
                            Some(serialize_expected(items, substring, normalize, ignore_case));
                        (
                            *name,
                            Style::Equal {
                                expected: JsValue::Array(items.iter().map(text_to_js).collect()),
                            },
                        )
                    }
                }
            }
            Spec::Classes { expected } => match expected {
                ExpectedClasses::One(c) => {
                    params.expression = "to.contain.class";
                    let t = TextMatch::Text(c.clone());
                    params.expected_text = Some(serialize_expected(
                        std::slice::from_ref(&t),
                        false,
                        false,
                        None,
                    ));
                    (
                        "toContainClass",
                        Style::Text {
                            expected: t,
                            match_substring: false,
                        },
                    )
                }
                ExpectedClasses::Many(items) => {
                    params.expression = "to.contain.class.array";
                    let items: Vec<TextMatch> =
                        items.iter().map(|c| TextMatch::Text(c.clone())).collect();
                    params.expected_text = Some(serialize_expected(&items, false, false, None));
                    (
                        "toContainClass",
                        Style::Equal {
                            expected: JsValue::Array(items.iter().map(text_to_js).collect()),
                        },
                    )
                }
            },
            Spec::Attribute { name, expected } => {
                params.expression = "to.have.attribute.value";
                params.expression_arg = Some(name.clone());
                params.expected_text = Some(serialize_expected(
                    std::slice::from_ref(expected),
                    false,
                    false,
                    ignore_case,
                ));
                (
                    "toHaveAttribute",
                    Style::Text {
                        expected: expected.clone(),
                        match_substring: false,
                    },
                )
            }
            Spec::Css { name, expected } => {
                params.expression = "to.have.css";
                params.expression_arg = Some(name.clone());
                params.pseudo = opts.pseudo.clone();
                params.expected_text = Some(serialize_expected(
                    std::slice::from_ref(expected),
                    false,
                    false,
                    None,
                ));
                (
                    "toHaveCSS",
                    Style::Text {
                        expected: expected.clone(),
                        match_substring: false,
                    },
                )
            }
            Spec::Count(n) => {
                params.expression = "to.have.count";
                params.expected_number = Some(*n as f64);
                (
                    "toHaveCount",
                    Style::Equal {
                        expected: JsValue::Number(*n as f64),
                    },
                )
            }
            Spec::Property { name, expected } => {
                params.expression = "to.have.property";
                params.expression_arg = Some(name.clone());
                params.expected_value = Some(expected.clone());
                (
                    "toHaveJSProperty",
                    Style::Equal {
                        expected: expected.clone(),
                    },
                )
            }
            Spec::Values(items) => {
                params.expression = "to.have.values";
                params.expected_text = Some(serialize_expected(items, false, false, None));
                (
                    "toHaveValues",
                    Style::Equal {
                        expected: JsValue::Array(items.iter().map(text_to_js).collect()),
                    },
                )
            }
            Spec::Title(t) => {
                params.expression = "to.have.title";
                params.expected_text = Some(serialize_expected(
                    std::slice::from_ref(t),
                    false,
                    true,
                    None,
                ));
                (
                    "toHaveTitle",
                    Style::Text {
                        expected: t.clone(),
                        match_substring: false,
                    },
                )
            }
            Spec::Url(t) => {
                let t = match (t, &self.subject) {
                    (TextMatch::Text(s), Subject::Page(p)) => {
                        TextMatch::Text(crate::url_match::construct_url_based_on_base_url(
                            p.inner.base_url().as_deref(),
                            s,
                        ))
                    }
                    _ => t.clone(),
                };
                params.expression = "to.have.url";
                params.expected_text = Some(serialize_expected(
                    std::slice::from_ref(&t),
                    false,
                    false,
                    ignore_case,
                ));
                (
                    "toHaveURL",
                    Style::Text {
                        expected: t,
                        match_substring: false,
                    },
                )
            }
        };
        let timeout = cfg.timeout(opts.timeout);
        let outcome = frame_expect(&frame, params, timeout, &cfg.title(name)).await;
        let locator_text = match &self.subject {
            Subject::Locator(l) if outcome.invalid_selector => Some(l.selector().to_string()),
            _ => locator_text,
        };
        finish(cfg, name, locator_text, style, outcome, timeout)
    }
}

/// The client-side part of `toBeTruthy` / `toMatchText` / `toEqual`.
fn finish(
    cfg: &Config,
    name: &str,
    locator: Option<String>,
    style: Style,
    outcome: ExpectOutcome,
    timeout: u64,
) -> Result<()> {
    let pass = outcome.matches;
    if pass != cfg.is_not {
        return Ok(());
    }
    let received = outcome.received.clone();
    let has_error = outcome.error_message.is_some();
    let mut details = MessageDetails {
        is_not: cfg.is_not,
        matcher_name: name.to_string(),
        locator,
        timed_out: outcome.timed_out,
        timeout: Some(timeout),
        error_message: outcome.error_message.clone(),
        log: outcome.log.clone(),
        ..Default::default()
    };
    let expected_value = match style {
        Style::Truthy { expected, arg } => {
            details.expectation = arg;
            if pass {
                details.printed_expected = Some(format!("Expected: not {expected}"));
                details.printed_received = Some(if has_error {
                    String::new()
                } else {
                    format!("Received: {expected}")
                });
            } else {
                details.printed_expected = Some(format!("Expected: {expected}"));
                details.printed_received = Some(if has_error {
                    String::new()
                } else {
                    format!("Received: {}", js_string(received.as_ref()))
                });
            }
            JsValue::String(expected)
        }
        Style::Text {
            expected,
            match_substring,
        } => {
            details.expectation = "expected".to_string();
            let is_string = matches!(expected, TextMatch::Text(_));
            let expected_suffix = match (is_string, match_substring) {
                (true, true) => " substring",
                (true, false) => "",
                (false, _) => " pattern",
            };
            let received_suffix = if is_string && !match_substring {
                ""
            } else {
                " string"
            };
            let received_string = match &received {
                Some(JsValue::String(s)) => s.clone(),
                _ => String::new(),
            };
            let expected_js = text_to_js(&expected);
            if pass {
                details.printed_expected = Some(format!(
                    "Expected{expected_suffix}: not {}",
                    stringify(&expected_js)
                ));
                if !has_error {
                    details.printed_received = Some(format!(
                        "Received{received_suffix}: {}",
                        stringify(&JsValue::String(received_string))
                    ));
                }
            } else if has_error {
                details.printed_expected = Some(format!(
                    "Expected{expected_suffix}: {}",
                    stringify(&expected_js)
                ));
            } else {
                details.printed_diff = Some(print_diff_or_stringify(
                    &expected_js,
                    &JsValue::String(received_string),
                    &format!("Expected{expected_suffix}"),
                    &format!("Received{received_suffix}"),
                ));
            }
            expected_js
        }
        Style::Equal { expected } => {
            details.expectation = "expected".to_string();
            let received_value = received.clone().unwrap_or(JsValue::Undefined);
            if pass {
                details.printed_expected = Some(format!("Expected: not {}", stringify(&expected)));
                details.printed_received = Some(if has_error {
                    String::new()
                } else {
                    format!("Received: {}", stringify(&received_value))
                });
            } else if has_error {
                details.printed_expected = Some(format!("Expected: {}", stringify(&expected)));
            } else if let (JsValue::Array(exp), JsValue::Array(rec)) = (&expected, &received_value)
            {
                // Regular expressions that match are shown as the received text.
                let normalized: Vec<JsValue> = exp
                    .iter()
                    .enumerate()
                    .map(|(i, e)| match (e, rec.get(i)) {
                        (JsValue::RegExp { source, flags }, Some(JsValue::String(r))) => {
                            let re = crate::locator_utils::JsRegex::new(source, flags);
                            if re.to_regex().is_some_and(|re| re.is_match(r)) {
                                JsValue::String(r.clone())
                            } else {
                                e.clone()
                            }
                        }
                        _ => e.clone(),
                    })
                    .collect();
                details.printed_diff = Some(print_diff_or_stringify(
                    &JsValue::Array(normalized),
                    &received_value,
                    "Expected",
                    "Received",
                ));
            } else {
                details.printed_diff = Some(print_diff_or_stringify(
                    &expected,
                    &received_value,
                    "Expected",
                    "Received",
                ));
            }
            expected
        }
    };
    let message = format_matcher_message(details);
    Err(cfg.fail(MatcherResult {
        name: name.to_string(),
        pass,
        expected: Some(expected_value),
        actual: received,
        message,
        log: outcome.log,
        timeout: outcome.timed_out.then_some(timeout),
        aria_snapshot: outcome.aria_snapshot,
    }))
}

// ----------------------------------------------------------------------
// toHaveURL with a predicate
// ----------------------------------------------------------------------

/// `expect(page).to_have_url_matching(predicate)`.
#[must_use = "assertions do nothing unless awaited"]
pub struct ToHaveUrlPredicate {
    page: Page,
    cfg: Config,
    predicate: Box<dyn Fn(&str) -> bool + Send + Sync>,
    timeout: Option<u64>,
}

impl ToHaveUrlPredicate {
    /// Timeout of this assertion in milliseconds.
    pub fn timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout = Some(timeout_ms);
        self
    }

    /// Mirrors `toHaveURLWithPredicate` (polls the main frame URL).
    async fn run(self) -> Result<()> {
        let timeout = self.cfg.timeout(self.timeout);
        let start = std::time::Instant::now();
        let mut backoff = Backoff::new(timeout);
        let mut last;
        loop {
            backoff.wait().await;
            last = self.page.main_frame().url();
            if (self.predicate)(&last) != self.cfg.is_not {
                return Ok(());
            }
            if self.page.is_closed()
                || (timeout > 0 && start.elapsed() >= Duration::from_millis(timeout))
            {
                break;
            }
        }
        let message = format_matcher_message(MessageDetails {
            is_not: self.cfg.is_not,
            matcher_name: "toHaveURL".to_string(),
            expectation: "expected".to_string(),
            printed_expected: Some(format!(
                "Expected: predicate to {}",
                if self.cfg.is_not { "fail" } else { "succeed" }
            )),
            printed_received: Some(format!(
                "Received: {}",
                stringify(&JsValue::String(last.clone()))
            )),
            timed_out: true,
            timeout: Some(timeout),
            ..Default::default()
        });
        Err(self.cfg.fail(MatcherResult {
            name: "toHaveURL".to_string(),
            pass: self.cfg.is_not,
            expected: None,
            actual: Some(JsValue::String(last)),
            message,
            log: Vec::new(),
            timeout: Some(timeout),
            aria_snapshot: None,
        }))
    }
}

impl IntoFuture for ToHaveUrlPredicate {
    type Output = Result<()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Result<()>> + Send>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}
