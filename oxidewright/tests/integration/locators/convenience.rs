//! Port of `locator-convenience.spec.ts`.
//!
//! All tests ported. LO-057 checks the text node handle preview loosely
//! (it is computed in the background).

use oxidewright::GetByRoleOptions;
use serde_json::Value;

use crate::common::*;

#[test]
fn should_have_a_nice_preview() {
    run(|t| async move {
        t.page
            .goto(format!("{}/dom.html", t.server.prefix))
            .await
            .unwrap();
        let outer = t.page.locator("#outer");
        let inner = outer.locator("#inner");
        let check = t.page.locator("#check");
        let text = inner.evaluate_handle("e => e.firstChild").await.unwrap();
        t.page.evaluate::<i32>("() => 1").await.unwrap();
        assert_eq!(outer.to_string(), "locator('#outer')");
        assert_eq!(inner.to_string(), "locator('#outer').locator('#inner')");
        assert_eq!(check.to_string(), "locator('#check')");
        // The preview of a node handle is computed asynchronously.
        for _ in 0..50 {
            if text.to_string() != "JSHandle@node" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(text.to_string(), "JSHandle@#text=Text,↵more text");
    });
}

#[test]
fn get_attribute_should_work() {
    run(|t| async move {
        t.page
            .goto(format!("{}/dom.html", t.server.prefix))
            .await
            .unwrap();
        let locator = t.page.locator("#outer");
        assert_eq!(
            locator.get_attribute("name").await.unwrap().as_deref(),
            Some("value")
        );
        assert_eq!(locator.get_attribute("foo").await.unwrap(), None);
        assert_eq!(
            t.page
                .get_attribute("#outer", "name")
                .await
                .unwrap()
                .as_deref(),
            Some("value")
        );
        assert_eq!(t.page.get_attribute("#outer", "foo").await.unwrap(), None);
    });
}

#[test]
fn input_value_should_work() {
    run(|t| async move {
        t.page
            .goto(format!("{}/dom.html", t.server.prefix))
            .await
            .unwrap();
        t.page.select_option("#select", "foo").await.unwrap();
        assert_eq!(t.page.input_value("#select").await.unwrap(), "foo");
        t.page.fill("#textarea", "text value").await.unwrap();
        assert_eq!(t.page.input_value("#textarea").await.unwrap(), "text value");
        t.page.fill("#input", "input value").await.unwrap();
        assert_eq!(t.page.input_value("#input").await.unwrap(), "input value");
        let locator = t.page.locator("#input");
        assert_eq!(locator.input_value().await.unwrap(), "input value");
        let err = t.page.input_value("#inner").await.unwrap_err();
        assert!(
            err.to_string()
                .contains("Node is not an <input>, <textarea> or <select> element")
        );
        let err = t.page.locator("#inner").input_value().await.unwrap_err();
        assert!(
            err.to_string()
                .contains("Node is not an <input>, <textarea> or <select> element")
        );
    });
}

#[test]
fn inner_html_should_work() {
    run(|t| async move {
        t.page
            .goto(format!("{}/dom.html", t.server.prefix))
            .await
            .unwrap();
        let locator = t.page.locator("#outer");
        assert_eq!(
            locator.inner_html().await.unwrap(),
            "<div id=\"inner\">Text,\nmore text</div>"
        );
        assert_eq!(
            t.page.inner_html("#outer").await.unwrap(),
            "<div id=\"inner\">Text,\nmore text</div>"
        );
    });
}

#[test]
fn inner_text_should_work() {
    run(|t| async move {
        t.page
            .goto(format!("{}/dom.html", t.server.prefix))
            .await
            .unwrap();
        let locator = t.page.locator("#inner");
        assert_eq!(locator.inner_text().await.unwrap(), "Text, more text");
        assert_eq!(
            t.page.inner_text("#inner").await.unwrap(),
            "Text, more text"
        );
    });
}

#[test]
fn inner_text_should_throw() {
    run(|t| async move {
        t.page.set_content("<svg>text</svg>").await.unwrap();
        let err = t.page.inner_text("svg").await.unwrap_err();
        assert!(err.to_string().contains("Node is not an HTMLElement"));
        let err = t.page.locator("svg").inner_text().await.unwrap_err();
        assert!(err.to_string().contains("Node is not an HTMLElement"));
    });
}

#[test]
fn inner_text_should_produce_log() {
    run(|t| async move {
        t.page.set_content("<div>Hello</div>").await.unwrap();
        let err = t
            .page
            .locator("span")
            .inner_text()
            .timeout(1000)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("waiting for locator('span')"));
    });
}

#[test]
fn text_content_should_work() {
    run(|t| async move {
        t.page
            .goto(format!("{}/dom.html", t.server.prefix))
            .await
            .unwrap();
        let locator = t.page.locator("#inner");
        assert_eq!(
            locator.text_content().await.unwrap().as_deref(),
            Some("Text,\nmore text")
        );
        assert_eq!(
            t.page.text_content("#inner").await.unwrap().as_deref(),
            Some("Text,\nmore text")
        );
    });
}

#[test]
fn is_enabled_and_is_disabled_should_work() {
    run(|t| async move {
        t.page
            .set_content(
                "<button disabled>button1</button>\n<button>button2</button>\n<div>div</div>",
            )
            .await
            .unwrap();
        let div = t.page.locator("div");
        assert!(div.is_enabled().await.unwrap());
        assert!(!div.is_disabled().await.unwrap());
        assert!(t.page.is_enabled("div").await.unwrap());
        assert!(!t.page.is_disabled("div").await.unwrap());
        let button1 = t.page.locator(":text(\"button1\")");
        assert!(!button1.is_enabled().await.unwrap());
        assert!(button1.is_disabled().await.unwrap());
        assert!(!t.page.is_enabled(":text(\"button1\")").await.unwrap());
        assert!(t.page.is_disabled(":text(\"button1\")").await.unwrap());
        let button2 = t.page.locator(":text(\"button2\")");
        assert!(button2.is_enabled().await.unwrap());
        assert!(!button2.is_disabled().await.unwrap());
        assert!(t.page.is_enabled(":text(\"button2\")").await.unwrap());
        assert!(!t.page.is_disabled(":text(\"button2\")").await.unwrap());
    });
}

#[test]
fn is_editable_should_work() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<input id=input1 disabled>
    <textarea></textarea>
    <input id=input2>
    <div contenteditable="true"></div>
    <span id=span1 role=textbox aria-readonly=true></span>
    <span id=span2 role=textbox></span>
    <button>button</button>"#,
            )
            .await
            .unwrap();
        t.page
            .eval_on_selector::<Value>("textarea", "t => t.readOnly = true")
            .await
            .unwrap();
        let input1 = t.page.locator("#input1");
        assert!(!input1.is_editable().await.unwrap());
        assert!(!t.page.is_editable("#input1").await.unwrap());
        let input2 = t.page.locator("#input2");
        assert!(input2.is_editable().await.unwrap());
        assert!(t.page.is_editable("#input2").await.unwrap());
        let textarea = t.page.locator("textarea");
        assert!(!textarea.is_editable().await.unwrap());
        assert!(!t.page.is_editable("textarea").await.unwrap());
        assert!(t.page.locator("div").is_editable().await.unwrap());
        assert!(!t.page.locator("#span1").is_editable().await.unwrap());
        assert!(t.page.locator("#span2").is_editable().await.unwrap());
        let err = t.page.locator("button").is_editable().await.unwrap_err();
        assert!(err.to_string().contains("Element is not an <input>, <textarea>, <select> or [contenteditable] and does not have a role allowing [aria-readonly]"), "{err}");
    });
}

#[test]
fn is_checked_should_work() {
    run(|t| async move {
        t.page
            .set_content("<input type='checkbox' checked><div>Not a checkbox</div>")
            .await
            .unwrap();
        let element = t.page.locator("input");
        assert!(element.is_checked().await.unwrap());
        assert!(t.page.is_checked("input").await.unwrap());
        element
            .evaluate::<Value>("input => input.checked = false")
            .await
            .unwrap();
        assert!(!element.is_checked().await.unwrap());
        assert!(!t.page.is_checked("input").await.unwrap());
        let err = t.page.is_checked("div").await.unwrap_err();
        assert!(err.to_string().contains("Not a checkbox or radio button"));
    });
}

#[test]
fn is_checked_should_work_for_indeterminate_input() {
    run(|t| async move {
        t.page
            .set_content("<input type=\"checkbox\" checked>")
            .await
            .unwrap();
        t.page
            .locator("input")
            .evaluate::<Value>("e => e.indeterminate = true")
            .await
            .unwrap();
        assert!(t.page.locator("input").is_checked().await.unwrap());
        t.page.locator("input").uncheck().await.unwrap();
        assert!(!t.page.locator("input").is_checked().await.unwrap());
    });
}

#[test]
fn all_text_contents_should_work() {
    run(|t| async move {
        t.page
            .set_content("<div>A</div><div>B</div><div>C</div>")
            .await
            .unwrap();
        assert_eq!(
            t.page.locator("div").all_text_contents().await.unwrap(),
            vec!["A", "B", "C"]
        );
    });
}

#[test]
fn all_inner_texts_should_work() {
    run(|t| async move {
        t.page
            .set_content("<div>A</div><div>B</div><div>C</div>")
            .await
            .unwrap();
        assert_eq!(
            t.page.locator("div").all_inner_texts().await.unwrap(),
            vec!["A", "B", "C"]
        );
    });
}

#[test]
fn should_return_page() {
    run(|t| async move {
        t.page
            .goto(format!("{}/frames/two-frames.html", t.server.prefix))
            .await
            .unwrap();
        let outer = t.page.locator("#outer");
        assert!(outer.page().unwrap() == t.page);
        let inner = outer.locator("#inner");
        assert!(inner.page().unwrap() == t.page);
        let in_frame = t.page.frames()[1].locator("div");
        assert!(in_frame.page().unwrap() == t.page);
    });
}

#[test]
fn description_should_return_null_for_locator_without_description() {
    run(|t| async move {
        assert_eq!(t.page.locator("button").description(), None);
    });
}

#[test]
fn description_should_return_description_for_locator_with_simple_description() {
    run(|t| async move {
        let locator = t.page.locator("button").describe("Submit button");
        assert_eq!(locator.description().as_deref(), Some("Submit button"));
    });
}

#[test]
fn description_should_return_description_with_special_characters() {
    run(|t| async move {
        let locator = t
            .page
            .locator("div")
            .describe("Button with \"quotes\" and 'apostrophes'");
        assert_eq!(
            locator.description().as_deref(),
            Some("Button with \"quotes\" and 'apostrophes'")
        );
    });
}

#[test]
fn description_should_return_description_for_chained_locators() {
    run(|t| async move {
        let locator = t
            .page
            .locator("form")
            .locator("input")
            .describe("Form input field");
        assert_eq!(locator.description().as_deref(), Some("Form input field"));
    });
}

#[test]
fn description_should_return_description_for_locator_with_multiple_describe_calls() {
    run(|t| async move {
        let locator1 = t.page.locator("foo").describe("First description");
        assert_eq!(locator1.description().as_deref(), Some("First description"));
        let locator2 = locator1.locator("button").describe("Second description");
        assert_eq!(
            locator2.description().as_deref(),
            Some("Second description")
        );
        let locator3 = locator2.locator("button");
        assert_eq!(locator3.description(), None);
    });
}

#[test]
fn to_string_returns_formatted_locator() {
    run(|t| async move {
        let locator = t
            .page
            .get_by_role_with("button", GetByRoleOptions::new().name("Submit"));
        assert_eq!(
            locator.to_string(),
            "getByRole('button', { name: 'Submit' })"
        );
        assert_eq!(locator.description(), None);
    });
}

#[test]
fn to_string_prefers_description() {
    run(|t| async move {
        let locator = t
            .page
            .get_by_role_with("button", GetByRoleOptions::new().name("Submit"))
            .describe("Submit button");
        assert_eq!(locator.to_string(), "Submit button");
        assert_eq!(Some(locator.to_string()), locator.description());
    });
}
