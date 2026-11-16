//! Port of `locator-is-visible.spec.ts`. All tests ported.

use oxidewright::{GetByRoleOptions, LocatorOptions, WaitForSelectorState};

use super::*;
use crate::common::*;

#[test]
fn is_visible_and_is_hidden_should_work() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<div>Hi</div><span></span>").await.unwrap();
        let div = p.locator("div");
        assert!(div.is_visible().await.unwrap());
        assert!(!div.is_hidden().await.unwrap());
        assert!(p.is_visible("div").await.unwrap());
        assert!(!p.is_hidden("div").await.unwrap());
        let span = p.locator("span");
        assert!(!span.is_visible().await.unwrap());
        assert!(span.is_hidden().await.unwrap());
        assert!(!p.is_visible("span").await.unwrap());
        assert!(p.is_hidden("span").await.unwrap());
        assert!(!p.is_visible("no-such-element").await.unwrap());
        assert!(p.is_hidden("no-such-element").await.unwrap());
    });
}

#[test]
fn is_visible_should_be_true_for_opacity_0() {
    run(|t| async move {
        t.page
            .set_content("<div style=\"opacity:0\">Hi</div>")
            .await
            .unwrap();
        expect_visible(&t.page.locator("div")).await;
    });
}

#[test]
fn is_visible_should_be_true_for_element_outside_view() {
    run(|t| async move {
        t.page
            .set_content("<div style=\"position: absolute; left: -1000px\">Hi</div>")
            .await
            .unwrap();
        expect_visible(&t.page.locator("div")).await;
    });
}

#[test]
fn is_visible_and_is_hidden_should_work_with_details() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<details>
    <summary>click to open</summary>
      <ul>
        <li>hidden item 1</li>
        <li>hidden item 2</li>
        <li>hidden item 3</li>
      </ul
  </details>"#,
            )
            .await
            .unwrap();
        expect_hidden(&t.page.locator("ul")).await;
    });
}

#[test]
fn is_visible_and_is_hidden_should_work_with_nested_details() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"<details>
    <summary>outer</summary>
    <details open>
      <summary>inner</summary>
      <button>hidden button</button>
    </details>
  </details>"#,
        )
        .await
        .unwrap();
        expect_visible(&p.locator_with("summary", LocatorOptions::new().has_text("outer"))).await;
        expect_hidden(&p.locator_with("summary", LocatorOptions::new().has_text("inner"))).await;
        expect_hidden(&p.locator("button")).await;
        expect_count(&p.get_by_role("button"), 0).await;
    });
}

#[test]
fn is_visible_and_is_hidden_should_work_with_unslotted_content() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"<div id=unslotted><span>hidden 1</span><div><span>hidden 2</span></div></div>
    <div id=slotted><span>visible</span></div>
    <script>
      unslotted.attachShadow({ mode: 'open' }).innerHTML = 'nothing to see here';
      slotted.attachShadow({ mode: 'open' }).innerHTML = '<slot></slot>';
    </script>"#,
        )
        .await
        .unwrap();
        let span = |text: &str| p.locator_with("span", LocatorOptions::new().has_text(text));
        expect_hidden(&span("hidden 1")).await;
        expect_hidden(&span("hidden 2")).await;
        expect_visible(&span("visible")).await;
    });
}

#[test]
fn is_visible_and_is_hidden_should_work_with_list_box_options() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"<select multiple>
      <option>visible 1</option>
      <optgroup label="group"><option>visible 2</option></optgroup>
      <option hidden>hidden 1</option>
      <optgroup label="hidden group" style="display:none"><option>hidden 2</option></optgroup>
    </select>
    <select size=3><option>visible 3</option></select>
    <select multiple style="display:none"><option>hidden 3</option></select>"#,
        )
        .await
        .unwrap();
        for name in ["visible 1", "visible 2", "visible 3"] {
            expect_visible(
                &p.get_by_role_with("option", GetByRoleOptions::new().name(name).exact(true)),
            )
            .await;
        }
        for text in ["hidden 1", "hidden 2", "hidden 3"] {
            expect_hidden(&p.locator_with("option", LocatorOptions::new().has_text(text))).await;
        }
    });
}

async fn check_invisible_span(t: &TestContext, parent: &str) {
    let p = &t.page;
    let span = p.locator("span");
    assert!(!span.is_visible().await.unwrap());
    assert!(span.is_hidden().await.unwrap());
    assert!(!p.is_visible("span").await.unwrap());
    assert!(p.is_hidden("span").await.unwrap());
    expect_hidden(&span).await;
    span.wait_for()
        .state(WaitForSelectorState::Hidden)
        .await
        .unwrap();
    p.locator(parent)
        .wait_for()
        .state(WaitForSelectorState::Visible)
        .await
        .unwrap();
}

#[test]
fn is_visible_inside_a_button() {
    run(|t| async move {
        t.page
            .set_content("<button><span></span>a button</button>")
            .await
            .unwrap();
        check_invisible_span(&t, "button").await;
    });
}

#[test]
fn is_visible_inside_a_role_button() {
    run(|t| async move {
        t.page
            .set_content("<div role=button><span></span>a button</div>")
            .await
            .unwrap();
        check_invisible_span(&t, "[role=button]").await;
    });
}

#[test]
fn is_visible_during_navigation_should_not_throw() {
    run(|t| async move {
        for _ in 0..20 {
            let _ = t
                .page
                .set_content(format!(
                    "<script>setTimeout(() => {{ window.location.href = {:?}; }}, Math.random(50));</script>",
                    t.server.empty_page
                ))
                .await;
            assert!(!t.page.locator("div").is_visible().await.unwrap());
        }
    });
}

#[test]
fn is_visible_with_invalid_selector_should_throw() {
    run(|t| async move {
        let error = t.page.locator("hey=what").is_visible().await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Unknown engine \"hey\" while parsing selector hey=what"),
            "{error}"
        );
    });
}
