//! Port of `expect-misc.spec.ts`.
//!
//! Skipped:
//! * `EX-207`/`EX-210` checks that `toContainClass` rejects regular
//!   expressions: `to_contain_class` only accepts strings (type-checked).
//! * `EX-216 fail with invalid argument`: `to_have_url` only accepts strings
//!   and regular expressions (type-checked).
//! * `EX-222 support URLPattern`: `URLPattern` is not supported.
//! * `EX-235 should have good stack`: JavaScript stack traces.

use oxidewright::expect::expect;
use oxidewright::regex;
use serde_json::json;

use super::*;
use crate::common::*;

// ---------------------------------------------------------------- toHaveCount

#[test]
fn to_have_count_pass() {
    run(|t| async move {
        t.page
            .set_content("<select><option>One</option></select>")
            .await
            .unwrap();
        let locator = t.page.locator("option");
        let task = tokio::spawn(async move { expect(&locator).to_have_count(2).await });
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        assert!(!task.is_finished());
        t.page
            .set_content("<select><option>One</option><option>Two</option></select>")
            .await
            .unwrap();
        task.await.unwrap().unwrap();
    });
}

#[test]
fn to_have_count_pass_zero() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let locator = t.page.locator("span");
        expect(&locator).to_have_count(0).await.unwrap();
        expect(&locator).not().to_have_count(1).await.unwrap();
    });
}

#[test]
fn to_have_count_eventually_pass_zero() {
    run(|t| async move {
        t.page
            .set_content("<div><span>hello</span></div>")
            .await
            .unwrap();
        later(
            &t.page,
            200,
            "() => { document.querySelector('div').textContent = '' }",
        );
        let locator = t.page.locator("span");
        expect(&locator).to_have_count(0).await.unwrap();
        expect(&locator).not().to_have_count(1).await.unwrap();
    });
}

#[test]
fn to_have_count_eventually_pass_non_zero() {
    run(|t| async move {
        t.page.set_content("<ul></ul>").await.unwrap();
        later(
            &t.page,
            500,
            "() => { document.querySelector('ul').innerHTML = '<li>one</li><li>two</li>' }",
        );
        expect(&t.page.locator("li"))
            .to_have_count(2)
            .await
            .unwrap();
    });
}

#[test]
fn to_have_count_eventually_pass_not_non_zero() {
    run(|t| async move {
        t.page
            .set_content("<ul><li>one</li><li>two</li></ul>")
            .await
            .unwrap();
        later(
            &t.page,
            500,
            "() => { document.querySelector('ul').innerHTML = '' }",
        );
        expect(&t.page.locator("li"))
            .not()
            .to_have_count(2)
            .await
            .unwrap();
    });
}

#[test]
fn to_have_count_fail_zero() {
    run(|t| async move {
        t.page
            .set_content("<div><span></span></div>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("span"))
                .to_have_count(0)
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).toHaveCount(expected) failed\n\nLocator:  locator('span')\nExpected: 0\nReceived: 1\nTimeout:  1000ms",
        );
        assert_contains(
            &m,
            "- Expect \"toHaveCount\" locator('span') with timeout 1000ms",
        );
    });
}

#[test]
fn to_have_count_fail_zero_2() {
    run(|t| async move {
        t.page
            .set_content("<div><span></span></div>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("span"))
                .not()
                .to_have_count(1)
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).not.toHaveCount(expected) failed\n\nLocator:  locator('span')\nExpected: not 1\nReceived: 1\nTimeout:  1000ms",
        );
        assert_contains(
            &m,
            "- Expect \"not toHaveCount\" locator('span') with timeout 1000ms",
        );
    });
}

// ----------------------------------------------------------- toHaveJSProperty

const SET_FOO_OBJECT: &str = "e => { e.foo = { a: 1, b: 'string', c: new Date(1627503992000) } }";

#[test]
fn to_have_js_property_pass() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let locator = t.page.locator("div");
        locator.evaluate::<()>(SET_FOO_OBJECT).await.unwrap();
        let expected = oxidewright::JsValue::Object(vec![
            ("a".into(), oxidewright::JsValue::Number(1.0)),
            ("b".into(), oxidewright::JsValue::String("string".into())),
            (
                "c".into(),
                oxidewright::JsValue::Date("2021-07-28T20:26:32.000Z".into()),
            ),
        ]);
        expect(&locator)
            .to_have_js_property("foo", expected)
            .await
            .unwrap();
    });
}

#[test]
fn to_have_js_property_fail() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let locator = t.page.locator("div");
        locator.evaluate::<()>(SET_FOO_OBJECT).await.unwrap();
        let expected = oxidewright::JsValue::Object(vec![
            ("a".into(), oxidewright::JsValue::Number(1.0)),
            ("b".into(), oxidewright::JsValue::String("string".into())),
            (
                "c".into(),
                oxidewright::JsValue::Date("2021-07-28T20:26:32.001Z".into()),
            ),
        ]);
        let m = message(
            expect(&locator)
                .to_have_js_property("foo", expected)
                .timeout(1000)
                .await,
        );
        assert_contains(&m, "-   \"c\"");
    });
}

#[test]
fn to_have_js_property_pass_string() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let locator = t.page.locator("div");
        locator
            .evaluate::<()>("e => { e.foo = 'string' }")
            .await
            .unwrap();
        expect(&locator)
            .to_have_js_property("foo", "string")
            .await
            .unwrap();
    });
}

#[test]
fn to_have_js_property_fail_string() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let locator = t.page.locator("div");
        locator
            .evaluate::<()>("e => { e.foo = 'string' }")
            .await
            .unwrap();
        let m = message(
            expect(&locator)
                .to_have_js_property("foo", "error")
                .timeout(200)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).toHaveJSProperty(expected) failed\n\nLocator:  locator('div')\nExpected: \"error\"\nReceived: \"string\"\nTimeout:  200ms",
        );
        assert_contains(
            &m,
            "- Expect \"toHaveJSProperty\" locator('div') with timeout 200ms",
        );
    });
}

#[test]
fn to_have_js_property_pass_number() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let locator = t.page.locator("div");
        locator
            .evaluate::<()>("e => { e.foo = 2021 }")
            .await
            .unwrap();
        expect(&locator)
            .to_have_js_property("foo", 2021)
            .await
            .unwrap();
    });
}

#[test]
fn to_have_js_property_fail_number() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let locator = t.page.locator("div");
        locator
            .evaluate::<()>("e => { e.foo = 2021 }")
            .await
            .unwrap();
        let m = message(
            expect(&locator)
                .to_have_js_property("foo", 1)
                .timeout(200)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).toHaveJSProperty(expected) failed\n\nLocator:  locator('div')\nExpected: 1\nReceived: 2021\nTimeout:  200ms",
        );
        assert_contains(
            &m,
            "- Expect \"toHaveJSProperty\" locator('div') with timeout 200ms",
        );
    });
}

#[test]
fn to_have_js_property_pass_boolean() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let locator = t.page.locator("div");
        locator
            .evaluate::<()>("e => { e.foo = true }")
            .await
            .unwrap();
        expect(&locator)
            .to_have_js_property("foo", true)
            .await
            .unwrap();
    });
}

#[test]
fn to_have_js_property_fail_boolean() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let locator = t.page.locator("div");
        locator
            .evaluate::<()>("e => { e.foo = false }")
            .await
            .unwrap();
        let m = message(
            expect(&locator)
                .to_have_js_property("foo", true)
                .timeout(200)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).toHaveJSProperty(expected) failed\n\nLocator:  locator('div')\nExpected: true\nReceived: false\nTimeout:  200ms",
        );
        assert_contains(
            &m,
            "- Expect \"toHaveJSProperty\" locator('div') with timeout 200ms",
        );
    });
}

#[test]
fn to_have_js_property_pass_boolean_2() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let locator = t.page.locator("div");
        locator
            .evaluate::<()>("e => { e.foo = false }")
            .await
            .unwrap();
        expect(&locator)
            .to_have_js_property("foo", false)
            .await
            .unwrap();
    });
}

#[test]
fn to_have_js_property_fail_boolean_2() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let locator = t.page.locator("div");
        locator
            .evaluate::<()>("e => { e.foo = false }")
            .await
            .unwrap();
        let m = message(
            expect(&locator)
                .to_have_js_property("foo", true)
                .timeout(200)
                .await,
        );
        assert_contains(&m, "Expected: true\nReceived: false\nTimeout:  200ms");
    });
}

#[test]
fn to_have_js_property_pass_undefined() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        expect(&t.page.locator("div"))
            .to_have_js_property("foo", oxidewright::JsValue::Undefined)
            .await
            .unwrap();
    });
}

#[test]
fn to_have_js_property_pass_null() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let locator = t.page.locator("div");
        locator
            .evaluate::<()>("e => { e.foo = null }")
            .await
            .unwrap();
        expect(&locator)
            .to_have_js_property("foo", json!(null))
            .await
            .unwrap();
    });
}

#[test]
fn to_have_js_property_pass_nested() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let locator = t.page.locator("div");
        locator
            .evaluate::<()>(
                "e => { e.foo = { nested: { a: 1, b: 'string', c: new Date(1627503992000) } } }",
            )
            .await
            .unwrap();
        let date = oxidewright::JsValue::Date("2021-07-28T20:26:32.000Z".into());
        let nested = oxidewright::JsValue::Object(vec![
            ("a".into(), oxidewright::JsValue::Number(1.0)),
            ("b".into(), oxidewright::JsValue::String("string".into())),
            ("c".into(), date.clone()),
        ]);
        expect(&locator)
            .to_have_js_property("foo.nested", nested)
            .await
            .unwrap();
        expect(&locator)
            .to_have_js_property("foo.nested.a", 1)
            .await
            .unwrap();
        expect(&locator)
            .to_have_js_property("foo.nested.b", "string")
            .await
            .unwrap();
        expect(&locator)
            .to_have_js_property("foo.nested.c", date)
            .await
            .unwrap();
    });
}

#[test]
fn to_have_js_property_fail_nested() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let locator = t.page.locator("div");
        locator
            .evaluate::<()>(
                "e => { e.foo = { nested: { a: 1, b: 'string', c: new Date(1627503992000) } } }",
            )
            .await
            .unwrap();
        let m1 = message(
            expect(&locator)
                .to_have_js_property("foo.bar", json!({ "a": 1 }))
                .timeout(1000)
                .await,
        );
        assert_contains(&m1, "Received: undefined");
        let m2 = message(
            expect(&locator)
                .to_have_js_property("foo.nested.a", 2)
                .timeout(1000)
                .await,
        );
        assert_contains(&m2, "Received: 1");
    });
}

// ---------------------------------------------------------------- toHaveClass

#[test]
fn to_have_class_pass() {
    run(|t| async move {
        t.page
            .set_content("<div class=\"foo bar baz\"></div>")
            .await
            .unwrap();
        expect(&t.page.locator("div"))
            .to_have_class("foo bar baz")
            .await
            .unwrap();
    });
}

#[test]
fn to_have_class_pass_with_svgs() {
    run(|t| async move {
        t.page
            .set_content(r#"<svg class="c1 c2" role="img" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512"></svg>"#)
            .await
            .unwrap();
        expect(&t.page.locator("svg"))
            .to_have_class(regex("c1", ""))
            .await
            .unwrap();
    });
}

#[test]
fn to_have_class_fail() {
    run(|t| async move {
        t.page
            .set_content("<div class=\"bar baz\"></div>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("div"))
                .to_have_class("foo bar baz")
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).toHaveClass(expected) failed\n\nLocator:  locator('div')\nExpected: \"foo bar baz\"\nReceived: \"bar baz\"\nTimeout:  1000ms\n\nCall log:\n",
        );
        assert_contains(
            &m,
            "- Expect \"toHaveClass\" locator('div') with timeout 1000ms",
        );
    });
}

#[test]
fn to_have_class_pass_with_array() {
    run(|t| async move {
        t.page
            .set_content(
                "<div class=\"foo\"></div><div class=\"bar\"></div><div class=\"baz\"></div>",
            )
            .await
            .unwrap();
        let expected: Vec<oxidewright::TextMatch> =
            vec!["foo".into(), "bar".into(), regex("[a-z]az", "").into()];
        expect(&t.page.locator("div"))
            .to_have_class(expected)
            .await
            .unwrap();
    });
}

#[test]
fn to_have_class_fail_with_array() {
    run(|t| async move {
        t.page
            .set_content(
                "<div class=\"foo\"></div><div class=\"bar\"></div><div class=\"bar\"></div>",
            )
            .await
            .unwrap();
        let expected: Vec<oxidewright::TextMatch> =
            vec!["foo".into(), "bar".into(), regex("[a-z]az", "").into()];
        let m = message(
            expect(&t.page.locator("div"))
                .to_have_class(expected)
                .timeout(1000)
                .await,
        );
        assert_contains(&m, "expect(locator).toHaveClass(expected) failed");
        assert_contains(&m, "Timeout: 1000ms");
        assert_contains(
            &m,
            "- Expect \"toHaveClass\" locator('div') with timeout 1000ms",
        );
    });
}

// ------------------------------------------------------------- toContainClass

#[test]
fn to_contain_class_pass() {
    run(|t| async move {
        t.page
            .set_content("<div class=\"foo bar baz\"></div>")
            .await
            .unwrap();
        let locator = t.page.locator("div");
        expect(&locator).to_contain_class("").await.unwrap();
        expect(&locator).to_contain_class("bar").await.unwrap();
        expect(&locator).to_contain_class("baz bar").await.unwrap();
        expect(&locator)
            .to_contain_class("  bar   foo ")
            .await
            .unwrap();
        expect(&locator)
            .not()
            .to_contain_class("  baz   not-matching ")
            .await
            .unwrap();
    });
}

#[test]
fn to_contain_class_pass_with_svgs() {
    run(|t| async move {
        t.page
            .set_content(r#"<svg class="c1 c2" role="img" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512"></svg>"#)
            .await
            .unwrap();
        expect(&t.page.locator("svg"))
            .to_contain_class("c1")
            .await
            .unwrap();
    });
}

#[test]
fn to_contain_class_fail() {
    run(|t| async move {
        t.page
            .set_content("<div class=\"bar baz\"></div>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("div"))
                .to_contain_class("does-not-exist")
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).toContainClass(expected) failed\n\nLocator:  locator('div')\nExpected: \"does-not-exist\"\nReceived: \"bar baz\"\nTimeout:  1000ms\n\nCall log:\n",
        );
        assert_contains(
            &m,
            "- Expect \"toContainClass\" locator('div') with timeout 1000ms",
        );
    });
}

#[test]
fn to_contain_class_pass_with_array() {
    run(|t| async move {
        t.page
            .set_content(
                "<div class=\"foo\"></div><div class=\"hello bar\"></div><div class=\"baz\"></div>",
            )
            .await
            .unwrap();
        let locator = t.page.locator("div");
        expect(&locator)
            .to_contain_class(["foo", "hello", "baz"])
            .await
            .unwrap();
        expect(&locator)
            .not()
            .to_have_class(["not-there", "hello", "baz"])
            .await
            .unwrap();
        expect(&locator)
            .not()
            .to_have_class(["foo", "hello"])
            .await
            .unwrap();
    });
}

#[test]
fn to_contain_class_fail_with_array() {
    run(|t| async move {
        t.page
            .set_content(
                "<div class=\"foo\"></div><div class=\"bar\"></div><div class=\"bar\"></div>",
            )
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("div"))
                .to_contain_class(["foo", "bar", "baz"])
                .timeout(1000)
                .await,
        );
        assert_contains(&m, "expect(locator).toContainClass(expected) failed");
        assert_contains(&m, "Timeout: 1000ms");
        assert_contains(
            &m,
            "- Expect \"toContainClass\" locator('div') with timeout 1000ms",
        );
    });
}

// ---------------------------------------------------------------- toHaveTitle

#[test]
fn to_have_title_pass() {
    run(|t| async move {
        t.page
            .set_content("<title>  Hello     world</title>")
            .await
            .unwrap();
        expect(&t.page).to_have_title("Hello  world").await.unwrap();
    });
}

#[test]
fn to_have_title_fail() {
    run(|t| async move {
        t.page.set_content("<title>Bye</title>").await.unwrap();
        let m = message(expect(&t.page).to_have_title("Hello").timeout(1000).await);
        assert_contains(
            &m,
            "expect(page).toHaveTitle(expected) failed\n\nExpected: \"Hello\"\nReceived: \"Bye\"\nTimeout:  1000ms\n\nCall log:\n",
        );
        assert_contains(&m, "- Expect \"toHaveTitle\" with timeout 1000ms");
    });
}

// ------------------------------------------------------------------ toHaveURL

#[test]
fn to_have_url_pass() {
    run(|t| async move {
        t.page.goto("data:text/html,<div>A</div>").await.unwrap();
        expect(&t.page)
            .to_have_url("data:text/html,<div>A</div>")
            .await
            .unwrap();
    });
}

#[test]
fn to_have_url_fail_string() {
    run(|t| async move {
        t.page.goto("data:text/html,<div>A</div>").await.unwrap();
        let m = message(expect(&t.page).to_have_url("wrong").timeout(1000).await);
        assert_contains(
            &m,
            "expect(page).toHaveURL(expected) failed\n\nExpected: \"wrong\"\nReceived: \"data:text/html,<div>A</div>\"\nTimeout:  1000ms\n\nCall log:\n",
        );
    });
}

#[test]
fn to_have_url_fail_with_positive_predicate() {
    run(|t| async move {
        t.page.goto("data:text/html,<div>A</div>").await.unwrap();
        let m = message(
            expect(&t.page)
                .to_have_url_matching(|_| false)
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "expect(page).toHaveURL(expected) failed\n\nExpected: predicate to succeed\nReceived: \"data:text/html,<div>A</div>\"\nTimeout:  1000ms\n",
        );
    });
}

#[test]
fn to_have_url_fail_with_negative_predicate() {
    run(|t| async move {
        t.page.goto("data:text/html,<div>A</div>").await.unwrap();
        let m = message(
            expect(&t.page)
                .not()
                .to_have_url_matching(|_| true)
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "expect(page).not.toHaveURL(expected) failed\n\nExpected: predicate to fail\nReceived: \"data:text/html,<div>A</div>\"\nTimeout:  1000ms\n",
        );
    });
}

#[test]
fn to_have_url_resolve_predicate_on_initial_call() {
    run(|t| async move {
        t.page.goto("data:text/html,<div>A</div>").await.unwrap();
        expect(&t.page)
            .to_have_url_matching(|url| url == "data:text/html,<div>A</div>")
            .timeout(1000)
            .await
            .unwrap();
    });
}

#[test]
fn to_have_url_resolve_predicate_after_retries() {
    run(|t| async move {
        t.page.goto("data:text/html,<div>A</div>").await.unwrap();
        let page = t.page.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let _ = page.goto("data:text/html,<div>B</div>").await;
        });
        expect(&t.page)
            .to_have_url_matching(|url| url == "data:text/html,<div>B</div>")
            .timeout(1000)
            .await
            .unwrap();
    });
}

#[test]
fn to_have_url_support_ignore_case() {
    run(|t| async move {
        t.page.goto("data:text/html,<div>A</div>").await.unwrap();
        expect(&t.page)
            .to_have_url("DATA:teXT/HTml,<div>a</div>")
            .ignore_case(true)
            .await
            .unwrap();
    });
}

// ------------------------------------------------------------ toHaveAttribute

#[test]
fn to_have_attribute_pass() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        expect(&t.page.locator("#node"))
            .to_have_attribute("id", "node")
            .await
            .unwrap();
    });
}

#[test]
fn to_have_attribute_should_not_match_missing_attribute() {
    run(|t| async move {
        t.page
            .set_content("<div checked id=node>Text content</div>")
            .await
            .unwrap();
        let locator = t.page.locator("#node");
        let m = message(
            expect(&locator)
                .to_have_attribute("disabled", "")
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).toHaveAttribute(expected) failed\n\nLocator:  locator('#node')\nExpected: \"\"\nReceived: serializes to the same string\nTimeout:  1000ms\n\nCall log:\n",
        );
        assert_contains(
            &m,
            "- Expect \"toHaveAttribute\" locator('#node') with timeout 1000ms",
        );
        let m = message(
            expect(&locator)
                .to_have_attribute("disabled", regex(".*", ""))
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).toHaveAttribute(expected) failed\n\nLocator: locator('#node')\nExpected pattern: /.*/\nReceived string:  \"\"\nTimeout: 1000ms\n\nCall log:\n",
        );
        expect(&locator)
            .not()
            .to_have_attribute("disabled", "")
            .await
            .unwrap();
        expect(&locator)
            .not()
            .to_have_attribute("disabled", regex(".*", ""))
            .await
            .unwrap();
    });
}

#[test]
fn to_have_attribute_should_match_boolean_attribute() {
    run(|t| async move {
        t.page
            .set_content("<div checked id=node>Text content</div>")
            .await
            .unwrap();
        let locator = t.page.locator("#node");
        expect(&locator)
            .to_have_attribute("checked", "")
            .await
            .unwrap();
        expect(&locator)
            .to_have_attribute("checked", regex(".*", ""))
            .await
            .unwrap();
        let m = message(
            expect(&locator)
                .not()
                .to_have_attribute("checked", "")
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).not.toHaveAttribute(expected) failed\n\nLocator:  locator('#node')\nExpected: not \"\"\nReceived: \"\"\nTimeout:  1000ms\n\nCall log:\n",
        );
        assert_contains(
            &m,
            "- Expect \"not toHaveAttribute\" locator('#node') with timeout 1000ms",
        );
        let m = message(
            expect(&locator)
                .not()
                .to_have_attribute("checked", regex(".*", ""))
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).not.toHaveAttribute(expected) failed\n\nLocator: locator('#node')\nExpected pattern: not /.*/\nReceived string: \"\"\nTimeout: 1000ms",
        );
    });
}

#[test]
fn to_have_attribute_should_match_attribute_without_value() {
    run(|t| async move {
        t.page
            .set_content("<div checked id=node>Text content</div>")
            .await
            .unwrap();
        let locator = t.page.locator("#node");
        expect(&locator)
            .to_have_attribute_present("id")
            .await
            .unwrap();
        expect(&locator)
            .to_have_attribute_present("checked")
            .await
            .unwrap();
        expect(&locator)
            .not()
            .to_have_attribute_present("open")
            .await
            .unwrap();
    });
}

#[test]
fn to_have_attribute_should_support_boolean_attribute_with_options() {
    run(|t| async move {
        t.page
            .set_content("<div checked id=node>Text content</div>")
            .await
            .unwrap();
        let locator = t.page.locator("#node");
        expect(&locator)
            .to_have_attribute_present("id")
            .timeout(5000)
            .await
            .unwrap();
        expect(&locator)
            .to_have_attribute_present("checked")
            .timeout(5000)
            .await
            .unwrap();
        expect(&locator)
            .not()
            .to_have_attribute_present("open")
            .timeout(5000)
            .await
            .unwrap();
    });
}

#[test]
fn to_have_attribute_support_ignore_case() {
    run(|t| async move {
        t.page
            .set_content("<div id=NoDe>Text content</div>")
            .await
            .unwrap();
        let locator = t.page.locator("#NoDe");
        expect(&locator)
            .to_have_attribute("id", "node")
            .ignore_case(true)
            .await
            .unwrap();
        expect(&locator)
            .not()
            .to_have_attribute("id", "node")
            .await
            .unwrap();
    });
}

// ------------------------------------------------------------------ toHaveCSS

#[test]
fn to_have_css_pass() {
    run(|t| async move {
        t.page
            .set_content("<div id=node style=\"color: rgb(255, 0, 0)\">Text content</div>")
            .await
            .unwrap();
        expect(&t.page.locator("#node"))
            .to_have_css("color", "rgb(255, 0, 0)")
            .await
            .unwrap();
    });
}

#[test]
fn to_have_css_should_support_pseudo_element() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<style>
        #node::before {
          color: rgb(255, 0, 0);
          content: "Text content";
        }
      </style>
      <div id=node></div>"#,
            )
            .await
            .unwrap();
        expect(&t.page.locator("#node"))
            .to_have_css("color", "rgb(255, 0, 0)")
            .pseudo("before")
            .await
            .unwrap();
    });
}

#[test]
fn to_have_css_custom_css_properties() {
    run(|t| async move {
        t.page
            .set_content(
                "<div id=node style=\"--custom-color-property:#FF00FF;\">Text content</div>",
            )
            .await
            .unwrap();
        expect(&t.page.locator("#node"))
            .to_have_css("--custom-color-property", "#FF00FF")
            .await
            .unwrap();
    });
}

#[test]
fn to_have_id_pass() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        expect(&t.page.locator("#node"))
            .to_have_id("node")
            .await
            .unwrap();
    });
}

// ------------------------------------------------------------- toBeInViewport

#[test]
fn to_be_in_viewport_should_work() {
    run(|t| async move {
        t.page
            .set_content("<div id=big style=\"height: 10000px;\"></div><div id=small>foo</div>")
            .await
            .unwrap();
        expect(&t.page.locator("#big"))
            .to_be_in_viewport()
            .await
            .unwrap();
        expect(&t.page.locator("#small"))
            .not()
            .to_be_in_viewport()
            .await
            .unwrap();
        t.page
            .locator("#small")
            .scroll_into_view_if_needed()
            .await
            .unwrap();
        expect(&t.page.locator("#small"))
            .to_be_in_viewport()
            .await
            .unwrap();
        expect(&t.page.locator("#small"))
            .to_be_in_viewport()
            .ratio(1.0)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_in_viewport_should_respect_ratio_option() {
    run(|t| async move {
        t.page
            .set_content("<style>body, div, html { padding: 0; margin: 0; }</style><div id=big style=\"height: 400vh;\"></div>")
            .await
            .unwrap();
        let div = t.page.locator("div");
        expect(&div).to_be_in_viewport().await.unwrap();
        expect(&div).to_be_in_viewport().ratio(0.1).await.unwrap();
        expect(&div).to_be_in_viewport().ratio(0.2).await.unwrap();
        expect(&div).to_be_in_viewport().ratio(0.24).await.unwrap();
        expect(&div).to_be_in_viewport().ratio(0.25).await.unwrap();
        expect(&div)
            .not()
            .to_be_in_viewport()
            .ratio(0.26)
            .await
            .unwrap();
        expect(&div)
            .not()
            .to_be_in_viewport()
            .ratio(0.3)
            .await
            .unwrap();
        expect(&div)
            .not()
            .to_be_in_viewport()
            .ratio(0.7)
            .await
            .unwrap();
        expect(&div)
            .not()
            .to_be_in_viewport()
            .ratio(0.8)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_in_viewport_should_report_intersection_even_if_fully_covered_by_other_element() {
    run(|t| async move {
        t.page
            .set_content("<h1>hello</h1><div style=\"position: relative; height: 10000px; top: -5000px;></div>")
            .await
            .unwrap();
        expect(&t.page.locator("h1"))
            .to_be_in_viewport()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_in_viewport_unexpected_value_in_log() {
    // The log part of `EX-235 should have good stack`.
    run(|t| async move {
        let m = message(
            expect(&t.page.locator("body"))
                .not()
                .to_be_in_viewport()
                .timeout(3000)
                .await,
        );
        assert!(
            regex::Regex::new(r#"unexpected value "viewport ratio \d+"#)
                .unwrap()
                .is_match(&m),
            "{m}"
        );
    });
}

// ----------------------------------------------------------------- call logs

#[test]
fn to_have_count_should_not_produce_logs_twice() {
    run(|t| async move {
        t.page
            .set_content("<select><option>One</option></select>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("option"))
                .to_have_count(2)
                .timeout(2000)
                .await,
        );
        let waiting = "waiting for locator('option')";
        assert_contains(&m, waiting);
        assert_contains(&m, "locator resolved to 1 element");
        assert_contains(&m, "unexpected value \"1\"");
        assert_not_contains(&m.replacen(waiting, "<redacted>", 1), waiting);
    });
}

#[test]
fn to_have_text_should_not_produce_logs_twice() {
    run(|t| async move {
        t.page.set_content("<div>hello</div>").await.unwrap();
        let m = message(
            expect(&t.page.locator("div"))
                .to_have_text("world")
                .timeout(2000)
                .await,
        );
        let waiting = "waiting for locator('div')";
        assert_contains(&m, waiting);
        assert_contains(&m, "locator resolved to <div>hello</div>");
        assert_contains(&m, "unexpected value \"hello\"");
        assert_not_contains(&m.replacen(waiting, "<redacted>", 1), waiting);
    });
}

#[test]
fn to_have_text_that_does_not_match_should_not_produce_logs_twice() {
    run(|t| async move {
        t.page.set_content("<div>hello</div>").await.unwrap();
        let m = message(
            expect(&t.page.locator("span"))
                .to_have_text("world")
                .timeout(2000)
                .await,
        );
        let waiting = "waiting for locator('span')";
        assert_contains(&m, waiting);
        assert_not_contains(&m, "locator resolved to");
        assert_not_contains(&m.replacen(waiting, "<redacted>", 1), waiting);
    });
}

#[test]
fn strict_mode_violation_error_format() {
    run(|t| async move {
        t.page
            .set_content("<div>a</div><div>b</div>")
            .await
            .unwrap();
        let m = message(expect(&t.page.locator("div")).to_have_text("foo").await);
        assert_contains(
            &m,
            "expect(locator).toHaveText(expected) failed\n\nLocator: locator('div')\nExpected: \"foo\"\nError: strict mode violation: locator('div') resolved to 2 elements:\n    1) <div>a</div> aka getByText('a')\n    2) <div>b</div> aka getByText('b')\n\nCall log:\n",
        );
    });
}

#[test]
fn invalid_selector_error_format() {
    run(|t| async move {
        t.page
            .set_content("<div>a</div><div>b</div>")
            .await
            .unwrap();
        let m = message(expect(&t.page.locator("##")).to_be_visible().await);
        assert_contains(
            &m,
            "expect(locator).toBeVisible() failed\n\nLocator: ##\nExpected: visible\nError: Unexpected token \"#\" while parsing css selector \"##\". Did you mean to CSS.escape it?\n\nCall log:\n",
        );
    });
}

#[test]
fn should_report_expect_error_details_when_page_closes_during_expect() {
    run(|t| async move {
        t.page.set_content("<div>hello</div>").await.unwrap();
        let locator = t.page.locator("div");
        let task =
            tokio::spawn(
                async move { expect(&locator).to_have_text("world").timeout(10_000).await },
            );
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        t.page.close().await.unwrap();
        let m = message(task.await.unwrap());
        assert_contains(&m, "expect(locator).toHaveText(expected) failed");
        assert_contains(&m, "Expected: \"world\"");
        assert_contains(&m, "Received: \"hello\"");
    });
}

#[test]
fn should_report_page_close_reason_when_page_closes_during_expect() {
    run(|t| async move {
        t.page.set_content("<div>hello</div>").await.unwrap();
        let locator = t.page.locator("non-existent");
        let task =
            tokio::spawn(
                async move { expect(&locator).to_have_text("world").timeout(10_000).await },
            );
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        t.page.close().reason("Bye bye.").await.unwrap();
        let m = message(task.await.unwrap());
        assert_contains(&m, "expect(locator).toHaveText(expected) failed");
        assert_contains(&m, "Bye bye.");
        assert_not_contains(&m, "Internal server error");
    });
}
