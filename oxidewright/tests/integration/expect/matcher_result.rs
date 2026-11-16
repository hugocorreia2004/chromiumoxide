//! Port of `expect-matcher-result.spec.ts`.
//!
//! `EX-180 toHaveScreenshot should populate matcherResult` is ported in
//! `screenshot.rs` (without the baseline image of Playwright's repository).

use oxidewright::JsValue;
use oxidewright::expect::expect;
use oxidewright::regex;

use super::*;
use crate::common::*;

fn s(v: &str) -> JsValue {
    JsValue::String(v.to_string())
}

#[test]
fn to_match_text_based_assertions_should_have_matcher_result() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        let locator = t.page.locator("#node");

        let e = matcher_result(
            expect(&locator)
                .to_have_text(regex("Text2", ""))
                .timeout(1)
                .await,
        );
        assert_eq!(e.name, "toHaveText");
        assert!(!e.pass);
        assert_eq!(e.actual, Some(s("Text content")));
        assert_eq!(
            e.expected,
            Some(JsValue::RegExp {
                source: "Text2".into(),
                flags: String::new()
            })
        );
        assert_eq!(e.timeout, Some(1));
        assert!(!e.log.is_empty());
        assert!(e.aria_snapshot.is_some());
        assert_contains(
            &e.message,
            "expect(locator).toHaveText(expected) failed\n\nLocator: locator('#node')\nExpected pattern: /Text2/\nReceived string:  \"Text content\"\nTimeout: 1ms\n\nCall log",
        );

        let e = matcher_result(
            expect(&locator)
                .not()
                .to_have_text(regex("Text", ""))
                .timeout(1)
                .await,
        );
        assert_eq!(e.name, "toHaveText");
        assert!(e.pass);
        assert_eq!(e.actual, Some(s("Text content")));
        assert_eq!(e.timeout, Some(1));
        assert!(e.aria_snapshot.is_some());
        assert_contains(
            &e.message,
            "expect(locator).not.toHaveText(expected) failed\n\nLocator: locator('#node')\nExpected pattern: not /Text/\nReceived string: \"Text content\"\nTimeout: 1ms\n\nCall log",
        );
    });
}

#[test]
fn to_be_truthy_based_assertions_should_have_matcher_result() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();

        let e = matcher_result(
            expect(&t.page.locator("#node2"))
                .to_be_visible()
                .timeout(1)
                .await,
        );
        assert_eq!(e.name, "toBeVisible");
        assert!(!e.pass);
        assert_eq!(e.expected, Some(s("visible")));
        assert_eq!(e.actual, None);
        assert_eq!(e.timeout, Some(1));
        assert!(e.aria_snapshot.is_some());
        assert_contains(
            &e.message,
            "expect(locator).toBeVisible() failed\n\nLocator: locator('#node2')\nExpected: visible\nTimeout: 1ms\nError: element(s) not found\n\nCall log:\n",
        );

        let e = matcher_result(
            expect(&t.page.locator("#node"))
                .not()
                .to_be_visible()
                .timeout(1)
                .await,
        );
        assert!(e.pass);
        assert_eq!(e.actual, Some(s("visible")));
        assert_eq!(e.expected, Some(s("visible")));
        assert!(e.aria_snapshot.is_some());
        assert_contains(
            &e.message,
            "expect(locator).not.toBeVisible() failed\n\nLocator:  locator('#node')\nExpected: not visible\nReceived: visible\nTimeout:  1ms\n\nCall log:\n",
        );
    });
}

#[test]
fn to_equal_based_assertions_should_have_matcher_result() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();

        let e = matcher_result(
            expect(&t.page.locator("#node2"))
                .to_have_count(1)
                .timeout(1)
                .await,
        );
        assert_eq!(e.name, "toHaveCount");
        assert!(!e.pass);
        assert_eq!(e.actual, Some(JsValue::Number(0.0)));
        assert_eq!(e.expected, Some(JsValue::Number(1.0)));
        assert_eq!(e.timeout, Some(1));
        assert_contains(
            &e.message,
            "expect(locator).toHaveCount(expected) failed\n\nLocator:  locator('#node2')\nExpected: 1\nReceived: 0\nTimeout:  1ms\n\nCall log",
        );

        let e = matcher_result(
            expect(&t.page.locator("#node"))
                .not()
                .to_have_count(1)
                .timeout(1)
                .await,
        );
        assert!(e.pass);
        assert_eq!(e.actual, Some(JsValue::Number(1.0)));
        assert_contains(
            &e.message,
            "expect(locator).not.toHaveCount(expected) failed\n\nLocator:  locator('#node')\nExpected: not 1\nReceived: 1\nTimeout:  1ms\n\nCall log",
        );
    });
}

#[test]
fn to_be_checked_checked_should_have_expected() {
    run(|t| async move {
        t.page
            .set_content("<input id=checked type=checkbox checked></input><input id=unchecked type=checkbox></input>")
            .await
            .unwrap();
        let checked = t.page.locator("#checked");
        let unchecked = t.page.locator("#unchecked");

        let e = matcher_result(expect(&unchecked).to_be_checked().timeout(1).await);
        assert_eq!(
            (e.actual, e.expected, e.pass),
            (Some(s("unchecked")), Some(s("checked")), false)
        );
        assert_contains(
            &e.message,
            "expect(locator).toBeChecked() failed\n\nLocator:  locator('#unchecked')\nExpected: checked\nReceived: unchecked\nTimeout:  1ms\n\nCall log",
        );

        let e = matcher_result(expect(&checked).not().to_be_checked().timeout(1).await);
        assert_eq!(
            (e.actual, e.expected, e.pass),
            (Some(s("checked")), Some(s("checked")), true)
        );
        assert_contains(
            &e.message,
            "expect(locator).not.toBeChecked() failed\n\nLocator:  locator('#checked')\nExpected: not checked\nReceived: checked\nTimeout:  1ms\n\nCall log",
        );

        let e = matcher_result(
            expect(&checked)
                .to_be_checked()
                .checked(false)
                .timeout(1)
                .await,
        );
        assert_eq!(
            (e.actual, e.expected, e.pass),
            (Some(s("checked")), Some(s("unchecked")), false)
        );
        assert_contains(
            &e.message,
            "expect(locator).toBeChecked({ checked: false }) failed\n\nLocator:  locator('#checked')\nExpected: unchecked\nReceived: checked\nTimeout:  1ms\n\nCall log",
        );

        let e = matcher_result(
            expect(&unchecked)
                .not()
                .to_be_checked()
                .checked(false)
                .timeout(1)
                .await,
        );
        assert_eq!(
            (e.actual, e.expected, e.pass),
            (Some(s("unchecked")), Some(s("unchecked")), true)
        );
        assert_contains(
            &e.message,
            "expect(locator).not.toBeChecked({ checked: false }) failed\n\nLocator:  locator('#unchecked')\nExpected: not unchecked\nReceived: unchecked\nTimeout:  1ms\n\nCall log",
        );

        let e = matcher_result(
            expect(&unchecked)
                .to_be_checked()
                .indeterminate(true)
                .timeout(1)
                .await,
        );
        assert_eq!(
            (e.actual, e.expected, e.pass),
            (Some(s("unchecked")), Some(s("indeterminate")), false)
        );
        assert_contains(
            &e.message,
            "expect(locator).toBeChecked({ indeterminate: true }) failed\n\nLocator:  locator('#unchecked')\nExpected: indeterminate\nReceived: unchecked\nTimeout:  1ms\n\nCall log",
        );
    });
}

#[test]
fn custom_message_is_printed_first_and_used_as_title() {
    // Not in Playwright's file: `expect(locator, 'message')`.
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("#node"))
                .message("node should say hi")
                .to_have_text("hi")
                .timeout(100)
                .await,
        );
        assert!(
            m.starts_with("node should say hi\n\nexpect(locator).toHaveText(expected) failed"),
            "{m}"
        );
        assert_contains(
            &m,
            "  - node should say hi locator('#node') with timeout 100ms",
        );
    });
}

#[test]
fn default_and_configured_timeouts() {
    // Not in Playwright's file: `expect.configure({ timeout })`.
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        let start = std::time::Instant::now();
        let e = matcher_result(
            expect(&t.page.locator("#node"))
                .timeout(300)
                .to_have_text("hi")
                .await,
        );
        assert_eq!(e.timeout, Some(300));
        assert!(start.elapsed() < std::time::Duration::from_millis(3000));
        assert_eq!(oxidewright::expect::default_timeout(), 5000);
    });
}
