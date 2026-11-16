//! Port of `expect-to-have-accessible.spec.ts`.
//!
//! Skipped: the second half of `EX-273 toHaveRole` (a regular expression
//! must be rejected): `to_have_role` only accepts a string (type-checked).

use oxidewright::expect::expect;
use oxidewright::regex;

use crate::common::*;

#[test]
fn to_have_accessible_name() {
    run(|t| async move {
        t.page
            .set_content(r#"<div role="button" aria-label="Hello"></div>"#)
            .await
            .unwrap();
        let div = t.page.locator("div");
        expect(&div).to_have_accessible_name("Hello").await.unwrap();
        expect(&div)
            .not()
            .to_have_accessible_name("hello")
            .await
            .unwrap();
        expect(&div)
            .to_have_accessible_name("hello")
            .ignore_case(true)
            .await
            .unwrap();
        expect(&div)
            .to_have_accessible_name(regex(r"ell\w", ""))
            .await
            .unwrap();
        expect(&div)
            .not()
            .to_have_accessible_name(regex("hello", ""))
            .await
            .unwrap();
        expect(&div)
            .to_have_accessible_name(regex("hello", ""))
            .ignore_case(true)
            .await
            .unwrap();

        t.page
            .set_content("<button>foo&nbsp;bar\nbaz</button>")
            .await
            .unwrap();
        expect(&t.page.locator("button"))
            .to_have_accessible_name("foo bar baz")
            .await
            .unwrap();

        t.page
            .set_content("<select><option>&nbsp;HTML</option></select>")
            .await
            .unwrap();
        expect(&t.page.locator("option"))
            .to_have_accessible_name("HTML")
            .await
            .unwrap();
    });
}

#[test]
fn to_have_accessible_description() {
    run(|t| async move {
        t.page
            .set_content(r#"<div role="button" aria-description="Hello"></div>"#)
            .await
            .unwrap();
        let div = t.page.locator("div");
        expect(&div)
            .to_have_accessible_description("Hello")
            .await
            .unwrap();
        expect(&div)
            .not()
            .to_have_accessible_description("hello")
            .await
            .unwrap();
        expect(&div)
            .to_have_accessible_description("hello")
            .ignore_case(true)
            .await
            .unwrap();
        expect(&div)
            .to_have_accessible_description(regex(r"ell\w", ""))
            .await
            .unwrap();
        expect(&div)
            .not()
            .to_have_accessible_description(regex("hello", ""))
            .await
            .unwrap();
        expect(&div)
            .to_have_accessible_description(regex("hello", ""))
            .ignore_case(true)
            .await
            .unwrap();

        t.page
            .set_content(
                r#"<div role="button" aria-describedby="desc"></div>
    <span id="desc">foo&nbsp;bar
baz</span>"#,
            )
            .await
            .unwrap();
        expect(&t.page.locator("div"))
            .to_have_accessible_description("foo bar baz")
            .await
            .unwrap();
    });
}

#[test]
fn to_have_accessible_error_message() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<form>
      <input role="textbox" aria-invalid="true" aria-errormessage="error-message" />
      <div id="error-message">Hello</div>
      <div id="irrelevant-error">This should not be considered.</div>
    </form>"#,
            )
            .await
            .unwrap();
        let locator = t.page.locator("input[role=\"textbox\"]");
        expect(&locator)
            .to_have_accessible_error_message("Hello")
            .await
            .unwrap();
        expect(&locator)
            .not()
            .to_have_accessible_error_message("hello")
            .await
            .unwrap();
        expect(&locator)
            .to_have_accessible_error_message("hello")
            .ignore_case(true)
            .await
            .unwrap();
        expect(&locator)
            .to_have_accessible_error_message(regex(r"ell\w", ""))
            .await
            .unwrap();
        expect(&locator)
            .not()
            .to_have_accessible_error_message(regex("hello", ""))
            .await
            .unwrap();
        expect(&locator)
            .to_have_accessible_error_message(regex("hello", ""))
            .ignore_case(true)
            .await
            .unwrap();
        expect(&locator)
            .not()
            .to_have_accessible_error_message("This should not be considered.")
            .await
            .unwrap();
    });
}

#[test]
fn to_have_accessible_error_message_should_handle_multiple_aria_errormessage_references() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<form>
      <input role="textbox" aria-invalid="true" aria-errormessage="error1 error2" />
      <div id="error1">First error message.</div>
      <div id="error2">Second error message.</div>
      <div id="irrelevant-error">This should not be considered.</div>
    </form>"#,
            )
            .await
            .unwrap();
        let locator = t.page.locator("input[role=\"textbox\"]");
        expect(&locator)
            .to_have_accessible_error_message("First error message. Second error message.")
            .await
            .unwrap();
        expect(&locator)
            .to_have_accessible_error_message(regex("first error message.", "i"))
            .await
            .unwrap();
        expect(&locator)
            .to_have_accessible_error_message(regex("second error message.", "i"))
            .await
            .unwrap();
        expect(&locator)
            .not()
            .to_have_accessible_error_message(regex("This should not be considered.", "i"))
            .await
            .unwrap();
    });
}

const ERROR_MESSAGE_TEXT: &str = "Error message";

async fn setup_page(page: &oxidewright::Page, aria_invalid: Option<&str>) -> oxidewright::Locator {
    let attr = match aria_invalid {
        None => String::new(),
        Some(v) => format!("aria-invalid=\"{v}\""),
    };
    page.set_content(format!(
        r#"<form>
          <input id="node" role="textbox" {attr} aria-errormessage="error-msg" />
          <div id="error-msg">{ERROR_MESSAGE_TEXT}</div>
        </form>"#
    ))
    .await
    .unwrap();
    page.locator("#node")
}

#[test]
fn aria_invalid_evaluated_in_false_no_aria_invalid_attribute() {
    run(|t| async move {
        let locator = setup_page(&t.page, None).await;
        expect(&locator)
            .not()
            .to_have_accessible_error_message(ERROR_MESSAGE_TEXT)
            .await
            .unwrap();
    });
}

#[test]
fn aria_invalid_evaluated_in_false_aria_invalid_false() {
    run(|t| async move {
        let locator = setup_page(&t.page, Some("false")).await;
        expect(&locator)
            .not()
            .to_have_accessible_error_message(ERROR_MESSAGE_TEXT)
            .await
            .unwrap();
    });
}

#[test]
fn aria_invalid_evaluated_in_false_aria_invalid_empty_string() {
    run(|t| async move {
        let locator = setup_page(&t.page, Some("")).await;
        expect(&locator)
            .not()
            .to_have_accessible_error_message(ERROR_MESSAGE_TEXT)
            .await
            .unwrap();
    });
}

#[test]
fn aria_invalid_evaluated_in_true_aria_invalid_true() {
    run(|t| async move {
        let locator = setup_page(&t.page, Some("true")).await;
        expect(&locator)
            .to_have_accessible_error_message(ERROR_MESSAGE_TEXT)
            .await
            .unwrap();
    });
}

#[test]
fn aria_invalid_evaluated_in_true_aria_invalid_foo_unrecognized_value() {
    run(|t| async move {
        let locator = setup_page(&t.page, Some("foo")).await;
        expect(&locator)
            .to_have_accessible_error_message(ERROR_MESSAGE_TEXT)
            .await
            .unwrap();
    });
}

async fn number_input(page: &oxidewright::Page, aria_invalid: &str) -> oxidewright::Locator {
    page.set_content(format!(
        r#"<form>
      <input id="node" role="textbox" type="number" min="1" max="100" aria-invalid="{aria_invalid}" aria-errormessage="error-msg" />
      <div id="error-msg">{ERROR_MESSAGE_TEXT}</div>
    </form>"#
    ))
    .await
    .unwrap();
    page.locator("#node")
}

#[test]
fn should_show_error_message_when_validity_is_false_and_aria_invalid_is_true() {
    run(|t| async move {
        let locator = number_input(&t.page, "true").await;
        locator.fill("101").await.unwrap();
        expect(&locator)
            .to_have_accessible_error_message(ERROR_MESSAGE_TEXT)
            .await
            .unwrap();
    });
}

#[test]
fn should_show_error_message_when_validity_is_true_and_aria_invalid_is_true() {
    run(|t| async move {
        let locator = number_input(&t.page, "true").await;
        locator.fill("99").await.unwrap();
        expect(&locator)
            .to_have_accessible_error_message(ERROR_MESSAGE_TEXT)
            .await
            .unwrap();
    });
}

#[test]
fn should_show_error_message_when_validity_is_false_and_aria_invalid_is_false() {
    run(|t| async move {
        let locator = number_input(&t.page, "false").await;
        locator.fill("101").await.unwrap();
        expect(&locator)
            .to_have_accessible_error_message(ERROR_MESSAGE_TEXT)
            .await
            .unwrap();
    });
}

#[test]
fn should_not_show_error_message_when_validity_is_true_and_aria_invalid_is_false() {
    run(|t| async move {
        let locator = number_input(&t.page, "false").await;
        locator.fill("99").await.unwrap();
        expect(&locator)
            .not()
            .to_have_accessible_error_message(ERROR_MESSAGE_TEXT)
            .await
            .unwrap();
    });
}

#[test]
fn should_show_error_message_for_all_roles() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<label for="file">File input</label>
    <input id="file" aria-invalid="true" aria-errormessage="file-error-id" type="file" />
    <p id="file-error-id" role="alert">File is incorrect</p>"#,
            )
            .await
            .unwrap();
        expect(&t.page.get_by_label("File input"))
            .to_have_accessible_error_message("File is incorrect")
            .await
            .unwrap();
    });
}

#[test]
fn to_have_role() {
    run(|t| async move {
        t.page
            .set_content(r#"<div role="button">Button!</div>"#)
            .await
            .unwrap();
        expect(&t.page.locator("div"))
            .to_have_role("button")
            .await
            .unwrap();
        expect(&t.page.locator("div"))
            .not()
            .to_have_role("checkbox")
            .await
            .unwrap();
    });
}
