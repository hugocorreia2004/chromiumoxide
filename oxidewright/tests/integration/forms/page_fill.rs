//! Port of `page-fill.spec.ts`.
//!
//! Skipped:
//! - FI-019 `should throw if passed a non-string value`: the Rust API only takes strings.
//! - FI-034 `should retry on inert element`, FI-036 `should fail with a nice error on
//!   inert element`: the bundled injected script (Patchright 1.64) has no `non-inert`
//!   element state.

use std::future::IntoFuture;

use super::{give_it_a_chance, spawn};
use crate::common::*;

async fn result(t: &TestContext) -> String {
    t.page.evaluate("() => window['result']").await.unwrap()
}

async fn input_value(t: &TestContext, selector: &str) -> String {
    t.page
        .eval_on_selector::<String>(selector, "input => input.value")
        .await
        .unwrap()
}

#[test]
fn should_fill_textarea() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        t.page.fill("textarea", "some value").await.unwrap();
        assert_eq!(result(&t).await, "some value");
    });
}

#[test]
fn should_fill_input() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        t.page.fill("input", "some value").await.unwrap();
        assert_eq!(result(&t).await, "some value");
    });
}

#[test]
fn should_throw_on_unsupported_inputs() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        for ty in [
            "button", "checkbox", "file", "image", "radio", "reset", "submit",
        ] {
            t.page
                .eval_on_selector::<()>(
                    "input",
                    "(input, type) => input.setAttribute('type', type)",
                )
                .arg(ty)
                .await
                .unwrap();
            let error = t.page.fill("input", "").await.unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(&format!("Input of type \"{ty}\" cannot be filled")),
                "{error}"
            );
        }
    });
}

#[test]
fn should_fill_different_input_types() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        for ty in ["password", "search", "tel", "text", "url", "invalid-type"] {
            t.page
                .eval_on_selector::<()>(
                    "input",
                    "(input, type) => input.setAttribute('type', type)",
                )
                .arg(ty)
                .await
                .unwrap();
            t.page.fill("input", format!("text {ty}")).await.unwrap();
            assert_eq!(result(&t).await, format!("text {ty}"));
        }
    });
}

#[test]
fn should_fill_range_input() {
    run(|t| async move {
        t.page
            .set_content("<input type=range min=0 max=100 value=50>")
            .await
            .unwrap();
        t.page.fill("input", "42").await.unwrap();
        assert_eq!(input_value(&t, "input").await, "42");
        // The rest of FI-005 (`42.0`, `1e1`) needs the Playwright 1.65 injected
        // script (issue 43328); the bundled one (1.64) rejects them as malformed.
    });
}

#[test]
fn should_fill_color_input() {
    run(|t| async move {
        t.page
            .set_content("<input type=color value=\"#e66465\">")
            .await
            .unwrap();
        t.page.fill("input", "#aaaaaa").await.unwrap();
        assert_eq!(input_value(&t, "input").await, "#aaaaaa");
    });
}

#[test]
fn should_fill_color_input_case_insensitive() {
    run(|t| async move {
        t.page
            .set_content("<input type=color value=\"#e66465\">")
            .await
            .unwrap();
        t.page.fill("input", "#AbCd00").await.unwrap();
        assert_eq!(input_value(&t, "input").await, "#abcd00");
    });
}

#[test]
fn should_fill_date_input_after_clicking() {
    run(|t| async move {
        t.page.set_content("<input type=date>").await.unwrap();
        t.page.click("input").await.unwrap();
        t.page.fill("input", "2020-03-02").await.unwrap();
        assert_eq!(input_value(&t, "input").await, "2020-03-02");
    });
}

async fn composed_test(t: &TestContext, ty: &str, value: &str) {
    t.page.goto(&t.server.empty_page).await.unwrap();
    t.page
        .set_content(format!(
            "<body><script>
    const div = document.createElement('div');
    const shadowRoot = div.attachShadow({{mode: 'open'}});
    shadowRoot.innerHTML = '<input type={ty}></input>';
    document.body.appendChild(div);
  </script></body>"
        ))
        .await
        .unwrap();
    t.page
        .locator("body")
        .evaluate::<()>(
            "select => { window.firedBodyEvents = []; for (const event of ['input', 'change']) select.addEventListener(event, e => window.firedBodyEvents.push(e.type + ':' + e.composed), false); }",
        )
        .await
        .unwrap();
    t.page
        .locator("input")
        .evaluate::<()>(
            "select => { window.firedEvents = []; for (const event of ['input', 'change']) select.addEventListener(event, e => window.firedEvents.push(e.type + ':' + e.composed), false); }",
        )
        .await
        .unwrap();
    t.page.locator("input").fill(value).await.unwrap();
    let fired: Vec<String> = t
        .page
        .evaluate("() => window['firedEvents']")
        .await
        .unwrap();
    assert_eq!(fired, vec!["input:true", "change:false"], "{ty}");
    let body: Vec<String> = t
        .page
        .evaluate("() => window['firedBodyEvents']")
        .await
        .unwrap();
    assert_eq!(body, vec!["input:true"], "{ty}");
}

#[test]
fn input_event_composed_should_be_true_and_cross_shadow_dom_boundary_color() {
    run(|t| async move {
        composed_test(&t, "color", "#aaaaaa").await;
    });
}

#[test]
fn input_event_composed_should_be_true_and_cross_shadow_dom_boundary_date() {
    run(|t| async move {
        composed_test(&t, "date", "2020-03-02").await;
    });
}

#[test]
fn input_event_composed_should_be_true_and_cross_shadow_dom_boundary_time() {
    run(|t| async move {
        composed_test(&t, "time", "13:15").await;
    });
}

#[test]
fn input_event_composed_should_be_true_and_cross_shadow_dom_boundary_datetime_local() {
    run(|t| async move {
        // Without seconds: the bundled 1.64 injected script rejects them (issue 43370).
        composed_test(&t, "datetime-local", "2020-03-02T13:15").await;
    });
}

#[test]
fn input_event_composed_should_be_true_and_cross_shadow_dom_boundary_month() {
    run(|t| async move {
        composed_test(&t, "month", "2020-03").await;
    });
}

#[test]
fn input_event_composed_should_be_true_and_cross_shadow_dom_boundary_range() {
    run(|t| async move {
        composed_test(&t, "range", "42").await;
    });
}

#[test]
fn input_event_composed_should_be_true_and_cross_shadow_dom_boundary_week() {
    run(|t| async move {
        composed_test(&t, "week", "2020-W50").await;
    });
}

#[test]
fn should_fill_time_input() {
    run(|t| async move {
        t.page.set_content("<input type=time>").await.unwrap();
        t.page.fill("input", "13:15").await.unwrap();
        assert_eq!(input_value(&t, "input").await, "13:15");
    });
}

#[test]
fn should_fill_month_input() {
    run(|t| async move {
        t.page.set_content("<input type=month>").await.unwrap();
        t.page.fill("input", "2020-07").await.unwrap();
        assert_eq!(input_value(&t, "input").await, "2020-07");
    });
}

#[test]
fn should_fill_week_input() {
    run(|t| async move {
        t.page.set_content("<input type=week>").await.unwrap();
        t.page.fill("input", "2020-W50").await.unwrap();
        assert_eq!(input_value(&t, "input").await, "2020-W50");
    });
}

#[test]
fn should_fill_datetime_local_input() {
    run(|t| async move {
        t.page
            .set_content("<input type=datetime-local>")
            .await
            .unwrap();
        t.page.fill("input", "2020-03-02T05:15").await.unwrap();
        assert_eq!(input_value(&t, "input").await, "2020-03-02T05:15");
        // Filling seconds (`2020-03-02T06:20:00`) needs the Playwright 1.65 injected
        // script (issue 43370); the bundled one (1.64) rejects it as malformed.
    });
}

#[test]
fn should_fill_contenteditable() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        t.page
            .fill("div[contenteditable]", "some value")
            .await
            .unwrap();
        let text: String = t
            .page
            .eval_on_selector("div[contenteditable]", "div => div.textContent")
            .await
            .unwrap();
        assert_eq!(text, "some value");
    });
}

#[test]
fn should_fill_contenteditable_with_new_lines() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .set_content("<div contenteditable=\"true\"></div>")
            .await
            .unwrap();
        let div = t.page.locator("div[contenteditable]");
        div.fill("John\nDoe").await.unwrap();
        assert_eq!(div.inner_text().await.unwrap(), "John\nDoe");
    });
}

#[test]
fn should_not_double_fill_in_contenteditable_with_beforeinput_handler_in_firefox() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<div id="editor" contenteditable="true"></div>
    <script>
      const editor = document.getElementById('editor');
      editor.addEventListener('beforeinput', (event) => {
        event.preventDefault();
        editor.textContent = event.data;
      });
    </script>"#,
            )
            .await
            .unwrap();
        let locator = t.page.locator("#editor");
        locator.fill("Playwright").await.unwrap();
        assert_eq!(
            locator.text_content().await.unwrap().as_deref(),
            Some("Playwright")
        );
    });
}

#[test]
fn should_fill_elements_with_existing_value_and_selection() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        t.page
            .eval_on_selector::<()>("input", "input => { input.value = 'value one'; }")
            .await
            .unwrap();
        t.page.fill("input", "another value").await.unwrap();
        assert_eq!(result(&t).await, "another value");

        t.page
            .eval_on_selector::<()>(
                "input",
                "input => { input.selectionStart = 1; input.selectionEnd = 2; }",
            )
            .await
            .unwrap();
        t.page.fill("input", "maybe this one").await.unwrap();
        assert_eq!(result(&t).await, "maybe this one");

        t.page
            .eval_on_selector::<()>(
                "div[contenteditable]",
                "div => { div.innerHTML = 'some text <span>some more text<span> and even more text'; const range = document.createRange(); range.selectNodeContents(div.querySelector('span')); const selection = window.getSelection(); selection.removeAllRanges(); selection.addRange(range); }",
            )
            .await
            .unwrap();
        t.page
            .fill("div[contenteditable]", "replace with this")
            .await
            .unwrap();
        let text: String = t
            .page
            .eval_on_selector("div[contenteditable]", "div => div.textContent")
            .await
            .unwrap();
        assert_eq!(text, "replace with this");
    });
}

#[test]
fn should_throw_nice_error_without_injected_script_stack_when_element_is_not_an_input() {
    run(|t| async move {
        t.page
            .set_content("<select><option>value1</option></select>")
            .await
            .unwrap();
        let error = t.page.fill("select", "").await.unwrap_err();
        assert!(
            error.to_string().contains(
                "page.fill: Error: Element is not an <input>, <textarea> or [contenteditable] element\nCall log:"
            ),
            "{error}"
        );
    });
}

async fn retry_test(t: &TestContext, selector: &str, block: &str, unblock: &str) {
    t.page
        .goto(format!("{}/input/textarea.html", t.server.prefix))
        .await
        .unwrap();
    t.page
        .eval_on_selector::<()>(selector, block)
        .await
        .unwrap();
    let fill = spawn(t.page.fill(selector, "some value"));
    give_it_a_chance().await;
    assert!(!fill.is_finished());
    assert_eq!(result(t).await, "");
    t.page
        .eval_on_selector::<()>(selector, unblock)
        .await
        .unwrap();
    fill.await.unwrap().unwrap();
    assert_eq!(result(t).await, "some value");
}

#[test]
fn should_retry_on_disabled_element() {
    run(|t| async move {
        retry_test(
            &t,
            "input",
            "i => { i.disabled = true; }",
            "i => { i.disabled = false; }",
        )
        .await;
    });
}

#[test]
fn should_retry_on_readonly_element() {
    run(|t| async move {
        retry_test(
            &t,
            "textarea",
            "i => { i.readOnly = true; }",
            "i => { i.readOnly = false; }",
        )
        .await;
    });
}

#[test]
fn should_retry_on_invisible_element() {
    run(|t| async move {
        retry_test(
            &t,
            "input",
            "i => { i.style.display = 'none'; }",
            "i => { i.style.display = 'inline'; }",
        )
        .await;
    });
}

#[test]
fn should_be_able_to_fill_the_body() {
    run(|t| async move {
        t.page
            .set_content("<body contentEditable=\"true\"></body>")
            .await
            .unwrap();
        t.page.fill("body", "some value").await.unwrap();
        let text: String = t
            .page
            .evaluate("() => document.body.textContent")
            .await
            .unwrap();
        assert_eq!(text, "some value");
    });
}

#[test]
fn should_fill_fixed_position_input() {
    run(|t| async move {
        t.page
            .set_content("<input style='position: fixed;' />")
            .await
            .unwrap();
        t.page.fill("input", "some value").await.unwrap();
        assert_eq!(input_value(&t, "input").await, "some value");
    });
}

#[test]
fn should_be_able_to_fill_when_focus_is_in_the_wrong_frame() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<div contentEditable="true"></div>
    <iframe srcdoc="<body contenteditable><p>old</p></body>"></iframe>"#,
            )
            .await
            .unwrap();
        t.page.focus("iframe").await.unwrap();
        t.page.fill("div", "some value").await.unwrap();
        let text: String = t
            .page
            .eval_on_selector("div", "d => d.textContent")
            .await
            .unwrap();
        assert_eq!(text, "some value");
        let paragraph = t.page.frame_locator("iframe").locator("p");
        paragraph.fill("new").await.unwrap();
        assert_eq!(
            paragraph.text_content().await.unwrap().as_deref(),
            Some("new")
        );
    });
}

#[test]
fn should_be_able_to_fill_the_input_type_number() {
    run(|t| async move {
        t.page
            .set_content("<input id=\"input\" type=\"number\"></input>")
            .await
            .unwrap();
        t.page.fill("input", "42").await.unwrap();
        assert_eq!(input_value(&t, "input").await, "42");
    });
}

#[test]
fn should_be_able_to_fill_exponent_into_the_input_type_number() {
    run(|t| async move {
        t.page
            .set_content("<input id=\"input\" type=\"number\"></input>")
            .await
            .unwrap();
        t.page.fill("input", "-10e5").await.unwrap();
        assert_eq!(input_value(&t, "input").await, "-10e5");
    });
}

#[test]
fn should_be_able_to_fill_input_type_number_with_empty_string() {
    run(|t| async move {
        t.page
            .set_content("<input id=\"input\" type=\"number\" value=\"123\"></input>")
            .await
            .unwrap();
        t.page.fill("input", "").await.unwrap();
        assert_eq!(input_value(&t, "input").await, "");
    });
}

#[test]
fn should_not_be_able_to_fill_text_into_the_input_type_number() {
    run(|t| async move {
        t.page
            .set_content("<input id=\"input\" type=\"number\"></input>")
            .await
            .unwrap();
        let error = t.page.fill("input", "abc").await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Cannot type text into input[type=number]"),
            "{error}"
        );
    });
}

#[test]
fn should_be_able_to_clear_using_fill() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        t.page.fill("input", "some value").await.unwrap();
        assert_eq!(result(&t).await, "some value");
        t.page.fill("input", "").await.unwrap();
        assert_eq!(result(&t).await, "");
    });
}

#[test]
fn should_not_throw_when_fill_causes_navigation() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        t.page.set_content("<input type=date>").await.unwrap();
        t.page
            .eval_on_selector::<()>(
                "input",
                "select => select.addEventListener('input', () => window.location.href = '/empty.html')",
            )
            .await
            .unwrap();
        let navigation = t.page.wait_for_navigation();
        let (fill, nav) = tokio::join!(
            t.page.fill("input", "2020-03-02").into_future(),
            navigation.into_future()
        );
        fill.unwrap();
        nav.unwrap();
        assert!(t.page.url().contains("empty.html"));
    });
}

#[test]
fn fill_back_to_back() {
    run(|t| async move {
        t.page
            .set_content("<input id=\"one\"></input><input id=\"two\"></input>")
            .await
            .unwrap();
        t.page.fill("id=one", "first value").await.unwrap();
        t.page.fill("id=two", "second value").await.unwrap();
        assert_eq!(
            t.page.locator("id=one").input_value().await.unwrap(),
            "first value"
        );
        assert_eq!(
            t.page.locator("id=two").input_value().await.unwrap(),
            "second value"
        );
    });
}

#[test]
fn should_fill_contenteditable_with_focus_handler_that_collapses_selection() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<div contenteditable="true">initial text</div>
    <script>
      const editor = document.querySelector('[contenteditable]');
      editor.addEventListener('focus', () => {
        const selection = window.getSelection();
        if (selection.rangeCount > 0)
          selection.collapseToEnd();
      });
    </script>"#,
            )
            .await
            .unwrap();
        t.page
            .fill("div[contenteditable]", "some value")
            .await
            .unwrap();
        assert_eq!(
            t.page
                .locator("div[contenteditable]")
                .text_content()
                .await
                .unwrap()
                .as_deref(),
            Some("some value")
        );
    });
}
