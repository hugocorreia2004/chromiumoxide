//! Port of `expect-to-have-value.spec.ts`. All tests ported.

use oxidewright::expect::expect;
use oxidewright::regex;

use super::*;
use crate::common::*;

const SELECT_RGB: &str = r#"<select multiple>
        <option value="R">Red</option>
        <option value="G">Green</option>
        <option value="B">Blue</option>
      </select>"#;

#[test]
fn should_work() {
    run(|t| async move {
        t.page.set_content("<input id=node></input>").await.unwrap();
        let locator = t.page.locator("#node");
        locator.fill("Text content").await.unwrap();
        expect(&locator)
            .to_have_value("Text content")
            .await
            .unwrap();
    });
}

#[test]
fn should_work_with_label() {
    run(|t| async move {
        t.page
            .set_content("<label><input></input></label>")
            .await
            .unwrap();
        t.page
            .locator("label input")
            .fill("Text content")
            .await
            .unwrap();
        expect(&t.page.locator("label"))
            .to_have_value("Text content")
            .await
            .unwrap();
    });
}

#[test]
fn should_work_with_regex() {
    run(|t| async move {
        t.page.set_content("<input id=node></input>").await.unwrap();
        let locator = t.page.locator("#node");
        locator.fill("Text content").await.unwrap();
        expect(&locator)
            .to_have_value(regex("Text", ""))
            .await
            .unwrap();
    });
}

#[test]
fn should_support_failure() {
    run(|t| async move {
        t.page.set_content("<input id=node></input>").await.unwrap();
        let locator = t.page.locator("#node");
        locator.fill("Text content").await.unwrap();
        let m = message(
            expect(&locator)
                .to_have_value(regex("Text2", ""))
                .timeout(1000)
                .await,
        );
        assert_contains(&m, "\"Text content\"");
    });
}

#[test]
fn to_have_values_with_multi_select_works_with_text() {
    run(|t| async move {
        t.page.set_content(SELECT_RGB).await.unwrap();
        let locator = t.page.locator("select");
        locator.select_option(vec!["R", "G"]).await.unwrap();
        expect(&locator).to_have_values(["R", "G"]).await.unwrap();
    });
}

#[test]
fn to_have_values_with_multi_select_follows_labels() {
    run(|t| async move {
        t.page
            .set_content(format!(
                "<label for=\"colors\">Pick a Color</label>{}",
                SELECT_RGB.replace("<select multiple>", "<select id=\"colors\" multiple>")
            ))
            .await
            .unwrap();
        let locator = t.page.locator("text=Pick a Color");
        locator.select_option(vec!["R", "G"]).await.unwrap();
        expect(&locator).to_have_values(["R", "G"]).await.unwrap();
    });
}

#[test]
fn to_have_values_with_multi_select_exact_match_with_text_failure() {
    run(|t| async move {
        t.page
            .set_content(r#"<select multiple><option value="RR">Red</option><option value="GG">Green</option></select>"#)
            .await
            .unwrap();
        let locator = t.page.locator("select");
        locator.select_option(vec!["RR", "GG"]).await.unwrap();
        let m = message(
            expect(&locator)
                .to_have_values(["R", "G"])
                .timeout(1000)
                .await,
        );
        assert_contains(&m, "-   \"R\"");
        assert_contains(&m, "+   \"RR\"");
    });
}

#[test]
fn to_have_values_with_multi_select_works_with_regex() {
    run(|t| async move {
        t.page.set_content(SELECT_RGB).await.unwrap();
        let locator = t.page.locator("select");
        locator.select_option(vec!["R", "G"]).await.unwrap();
        expect(&locator)
            .to_have_values([regex("R", ""), regex("G", "")])
            .await
            .unwrap();
    });
}

#[test]
fn to_have_values_with_multi_select_fails_when_items_not_selected() {
    run(|t| async move {
        t.page.set_content(SELECT_RGB).await.unwrap();
        let locator = t.page.locator("select");
        locator.select_option(vec!["B"]).await.unwrap();
        let m = message(
            expect(&locator)
                .to_have_values([regex("R", ""), regex("G", "")])
                .timeout(1000)
                .await,
        );
        assert_contains(&m, "+   \"B\"");
    });
}

#[test]
fn to_have_values_with_multi_select_fails_when_multiple_not_specified() {
    run(|t| async move {
        t.page
            .set_content(SELECT_RGB.replace(" multiple", ""))
            .await
            .unwrap();
        let locator = t.page.locator("select");
        locator.select_option(vec!["B"]).await.unwrap();
        let m = message(
            expect(&locator)
                .to_have_values([regex("R", ""), regex("G", "")])
                .timeout(1000)
                .await,
        );
        assert_contains(&m, "Not a select element with a multiple attribute");
    });
}

#[test]
fn to_have_values_with_multi_select_fails_when_not_a_select_element() {
    run(|t| async move {
        t.page.set_content("<input value=\"foo\" />").await.unwrap();
        let m = message(
            expect(&t.page.locator("input"))
                .to_have_values([regex("R", ""), regex("G", "")])
                .timeout(1000)
                .await,
        );
        assert_contains(&m, "Not a select element with a multiple attribute");
    });
}
