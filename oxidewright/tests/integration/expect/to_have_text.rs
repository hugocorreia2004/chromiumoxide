//! Port of `expect-to-have-text.spec.ts`. All tests ported.

use oxidewright::expect::expect;
use oxidewright::{TextMatch, regex};

use super::*;
use crate::common::*;

#[test]
fn to_have_text_with_regex_pass() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text   content</div>")
            .await
            .unwrap();
        let locator = t.page.locator("#node");
        expect(&locator)
            .to_have_text(regex("Text", ""))
            .await
            .unwrap();
        // Should not normalize whitespace.
        expect(&locator)
            .to_have_text(regex("Text   content", ""))
            .await
            .unwrap();
        // Should respect ignoreCase.
        expect(&locator)
            .to_have_text(regex("text   content", ""))
            .ignore_case(true)
            .await
            .unwrap();
        // Should override regex flag with ignoreCase.
        expect(&locator)
            .not()
            .to_have_text(regex("text   content", "i"))
            .ignore_case(false)
            .await
            .unwrap();
    });
}

#[test]
fn to_have_text_with_regex_fail() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("#node"))
                .to_have_text(regex("Text 2", ""))
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).toHaveText(expected) failed\n\nLocator: locator('#node')\nExpected pattern: /Text 2/\nReceived string:  \"Text content\"\nTimeout: 1000ms\n\nCall log:\n",
        );
    });
}

#[test]
fn to_contain_text_with_regex_pass() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text   content</div>")
            .await
            .unwrap();
        let locator = t.page.locator("#node");
        expect(&locator)
            .to_contain_text(regex("ex", ""))
            .await
            .unwrap();
        // Should not normalize whitespace.
        expect(&locator)
            .to_contain_text(regex("ext   cont", ""))
            .await
            .unwrap();
    });
}

#[test]
fn to_contain_text_with_regex_fail() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("#node"))
                .to_contain_text(regex("ex2", ""))
                .timeout(1000)
                .await,
        );
        assert_contains(&m, "Expected pattern: /ex2/");
        assert_contains(&m, "Received string:  \"Text content\"");
    });
}

#[test]
fn to_contain_text_with_string_pass() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text   content</div>")
            .await
            .unwrap();
        expect(&t.page.locator("#node"))
            .to_contain_text("content")
            .await
            .unwrap();
    });
}

#[test]
fn to_contain_text_with_string_fail() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("#node"))
                .to_contain_text("foo")
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).toContainText(expected) failed\n\nLocator: locator('#node')\nExpected substring: \"foo\"\nReceived string:    \"Text content\"\nTimeout: 1000ms\n\nCall log:\n",
        );
    });
}

#[test]
fn to_have_text_with_text_pass() {
    run(|t| async move {
        t.page
            .set_content("<div id=node><span></span>Text \ncontent&nbsp;    </div>")
            .await
            .unwrap();
        let locator = t.page.locator("#node");
        // Should normalize whitespace.
        expect(&locator)
            .to_have_text("Text                        content")
            .await
            .unwrap();
        // Should normalize zero width whitespace.
        expect(&locator)
            .to_have_text("T\u{200b}e\u{200b}x\u{200b}t content")
            .await
            .unwrap();
        // Should support ignoreCase.
        expect(&locator)
            .to_have_text("text CONTENT")
            .ignore_case(true)
            .await
            .unwrap();
        // Should support falsy ignoreCase.
        expect(&locator)
            .not()
            .to_have_text("TEXT")
            .ignore_case(false)
            .await
            .unwrap();
        // Should normalize soft hyphens.
        expect(&locator)
            .to_have_text("T\u{ad}e\u{ad}xt content")
            .await
            .unwrap();
    });
}

#[test]
fn to_have_text_with_text_pass_contain() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        let locator = t.page.locator("#node");
        expect(&locator).to_contain_text("Text").await.unwrap();
        // Should normalize whitespace.
        expect(&locator)
            .to_contain_text("   ext        cont\n  ")
            .await
            .unwrap();
        // Should support ignoreCase.
        expect(&locator)
            .to_contain_text("EXT")
            .ignore_case(true)
            .await
            .unwrap();
        // Should support falsy ignoreCase.
        expect(&locator)
            .not()
            .to_contain_text("TEXT")
            .ignore_case(false)
            .await
            .unwrap();
    });
}

#[test]
fn to_have_text_with_text_fail() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("#node"))
                .to_have_text("Text")
                .timeout(1000)
                .await,
        );
        assert_contains(&m, "Expected: \"Text\"");
        assert_contains(&m, "Received: \"Text content\"");
    });
}

#[test]
fn to_have_text_with_text_pass_eventually() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        later(
            &t.page,
            1000,
            "() => { document.querySelector('#node').textContent = 'Text 2 content' }",
        );
        expect(&t.page.locator("#node"))
            .to_have_text(regex("Text 2", ""))
            .await
            .unwrap();
    });
}

#[test]
fn to_have_text_with_text_with_user_inner_text() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text <span hidden>garbage</span> content</div>")
            .await
            .unwrap();
        expect(&t.page.locator("#node"))
            .to_have_text("Text content")
            .use_inner_text(true)
            .await
            .unwrap();
    });
}

#[test]
fn to_have_text_with_text_in_shadow_dom() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<div></div>
      <script>
        const div = document.querySelector('div');
        const span = document.createElement('span');
        span.textContent = 'some text';
        div.attachShadow({ mode: 'open' }).appendChild(span);
      </script>"#,
            )
            .await
            .unwrap();
        let span = t.page.locator("span");
        let div = t.page.locator("div");
        expect(&span).to_have_text("some text").await.unwrap();
        expect(&span).to_contain_text("text").await.unwrap();
        expect(&div).to_have_text("some text").await.unwrap();
        expect(&div).to_contain_text("text").await.unwrap();
        expect(&span)
            .to_have_text("some text")
            .use_inner_text(true)
            .await
            .unwrap();
        expect(&span)
            .to_contain_text("text")
            .use_inner_text(true)
            .await
            .unwrap();
        // Playwright intentionally does not perform innerText piercing on shadow dom.
        expect(&div)
            .not()
            .to_have_text("some text")
            .use_inner_text(true)
            .await
            .unwrap();
        expect(&div)
            .not()
            .to_contain_text("text")
            .use_inner_text(true)
            .await
            .unwrap();
    });
}

#[test]
fn to_have_text_with_text_fail_with_impossible_timeout() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("#node"))
                .to_have_text("Text")
                .timeout(1)
                .await,
        );
        assert_contains(&m, "Expected: \"Text\"");
        assert_contains(&m, "Received: \"Text content\"");
    });
}

#[test]
fn to_have_text_with_text_do_not_show_element_s_not_found_when_the_real_failure_is_a_string_mismatch()
 {
    run(|t| async move {
        t.page
            .set_content(
                r#"<div>Initial</div>
      <script>
        const field = document.querySelector('div');
        setTimeout(() => {
          field.id = 'field';
          field.textContent = 'Final value';
        }, 1000);
      </script>"#,
            )
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("#field"))
                .to_have_text("Something")
                .timeout(3000)
                .await,
        );
        assert_contains(&m, "Expected: \"Something\"");
        assert_contains(&m, "Received: \"Final value\"");
    });
}

#[test]
fn not_to_have_text_pass() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        let locator = t.page.locator("#node");
        expect(&locator).not().to_have_text("Text2").await.unwrap();
        // Should be case-sensitive by default.
        expect(&locator).not().to_have_text("TEXT").await.unwrap();
    });
}

#[test]
fn not_to_have_text_fail() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("#node"))
                .not()
                .to_have_text("Text content")
                .timeout(1000)
                .await,
        );
        assert_contains(&m, "Expected: not \"Text content\"");
        assert_contains(&m, "Received: \"Text content");
    });
}

#[test]
fn not_to_have_text_should_work_when_selector_does_not_match() {
    run(|t| async move {
        t.page.set_content("<div>hello</div>").await.unwrap();
        let m = message(
            expect(&t.page.locator("span"))
                .not()
                .to_have_text("hello")
                .timeout(1000)
                .await,
        );
        assert_contains(&m, "Expected: not \"hello\"");
        assert_contains(&m, "Error: element(s) not found");
        assert_contains(&m, "waiting for locator('span')");
    });
}

#[test]
fn to_have_text_with_array_pass() {
    run(|t| async move {
        t.page
            .set_content("<div>Text    \n1</div><div>Text   2a</div>")
            .await
            .unwrap();
        let locator = t.page.locator("div");
        // Should only normalize whitespace in the first item.
        let expected: Vec<TextMatch> = vec!["Text  1".into(), regex(r"Text   \d+a", "").into()];
        expect(&locator).to_have_text(expected).await.unwrap();
        // Should support ignoreCase.
        expect(&locator)
            .to_have_text(["tEXT 1", "TExt 2A"])
            .ignore_case(true)
            .await
            .unwrap();
    });
}

#[test]
fn to_have_text_with_array_pass_lazy() {
    run(|t| async move {
        t.page.set_content("<div id=div></div>").await.unwrap();
        later(
            &t.page,
            500,
            "() => { document.querySelector('div').innerHTML = '<p>Text 1</p><p>Text 2</p>' }",
        );
        expect(&t.page.locator("p"))
            .to_have_text(["Text 1", "Text 2"])
            .await
            .unwrap();
    });
}

#[test]
fn to_have_text_with_array_pass_empty() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        expect(&t.page.locator("p"))
            .to_have_text(Vec::<&str>::new())
            .await
            .unwrap();
    });
}

#[test]
fn to_have_text_with_array_pass_not_empty() {
    run(|t| async move {
        t.page.set_content("<div><p>Test</p></div>").await.unwrap();
        expect(&t.page.locator("p"))
            .not()
            .to_have_text(Vec::<&str>::new())
            .await
            .unwrap();
    });
}

#[test]
fn to_have_text_with_array_pass_on_empty() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        expect(&t.page.locator("p"))
            .not()
            .to_have_text(["Test"])
            .await
            .unwrap();
    });
}

#[test]
fn to_have_text_with_array_fail_on_not_empty() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let m = message(
            expect(&t.page.locator("p"))
                .not()
                .to_have_text(Vec::<&str>::new())
                .timeout(1000)
                .await,
        );
        assert_contains(&m, "expect(locator).not.toHaveText(expected)");
        assert_contains(&m, "Timeout:  1000ms");
        assert_contains(
            &m,
            "- Expect \"not toHaveText\" locator('p') with timeout 1000ms",
        );
    });
}

#[test]
fn to_have_text_with_array_pass_eventually_empty() {
    run(|t| async move {
        t.page
            .set_content("<div id=div><p>Text</p></div>")
            .await
            .unwrap();
        later(
            &t.page,
            500,
            "() => { document.querySelector('div').innerHTML = '' }",
        );
        expect(&t.page.locator("p"))
            .not()
            .to_have_text(Vec::<&str>::new())
            .await
            .unwrap();
    });
}

#[test]
fn to_have_text_with_array_fail() {
    run(|t| async move {
        t.page
            .set_content("<div>Text 1</div><div>Text 3</div>")
            .await
            .unwrap();
        let expected: Vec<TextMatch> = vec![
            "Text 1".into(),
            regex(r"Text \d", "").into(),
            "Extra".into(),
        ];
        let m = message(
            expect(&t.page.locator("div"))
                .to_have_text(expected)
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).toHaveText(expected) failed\n\nLocator: locator('div')\nTimeout: 1000ms\n- Expected  - 1\n+ Received  + 0\n\n  Array [\n    \"Text 1\",\n    \"Text 3\",\n-   \"Extra\",\n  ]\n\nCall log:\n  - Expect \"toHaveText\" locator('div') with timeout 1000ms\n  - waiting for locator('div')\n",
        );
        assert_contains(&m, "locator resolved to 2 elements");
    });
}

#[test]
fn to_have_text_with_array_fail_on_repeating_array_matchers() {
    run(|t| async move {
        t.page.set_content("<div>KekFoo</div>").await.unwrap();
        let m = message(
            expect(&t.page.locator("div"))
                .to_contain_text(["KekFoo", "KekFoo", "KekFoo"])
                .timeout(1000)
                .await,
        );
        assert_contains(&m, "locator resolved to 1 element");
    });
}

#[test]
fn to_contain_text_with_array_pass() {
    run(|t| async move {
        t.page
            .set_content("<div>Text \n1</div><div>Text2</div><div>Text3</div>")
            .await
            .unwrap();
        let locator = t.page.locator("div");
        let expected: Vec<TextMatch> = vec!["ext     1".into(), regex("ext3", "").into()];
        expect(&locator).to_contain_text(expected).await.unwrap();
        // Should support ignoreCase.
        expect(&locator)
            .to_contain_text(["EXT 1", "eXt3"])
            .ignore_case(true)
            .await
            .unwrap();
    });
}

#[test]
fn to_contain_text_with_array_fail() {
    run(|t| async move {
        t.page
            .set_content("<div>Text 1</div><div>Text 3</div>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("div"))
                .to_contain_text(["Text 2"])
                .timeout(1000)
                .await,
        );
        assert_contains(&m, "-   \"Text 2\"");
    });
}
