//! Port of `locator-click.spec.ts`.
//!
//! Skipped: none.

use super::{eval, eval_sel};
use crate::common::*;

fn button_page(t: &TestContext) -> String {
    format!("{}/input/button.html", t.server.prefix)
}

#[test]
fn should_work() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        t.page.locator("button").click().await.unwrap();
        assert_eq!(eval(&t.page, "() => window['result']").await, "Clicked");
    });
}

#[test]
fn should_work_with_node_removed() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        eval(&t.page, "() => delete window['Node']").await;
        t.page.locator("button").click().await.unwrap();
        assert_eq!(eval(&t.page, "() => window['result']").await, "Clicked");
    });
}

#[test]
fn should_double_click_the_button() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        eval(
            &t.page,
            "() => { window['double'] = false; const button = document.querySelector('button'); button.addEventListener('dblclick', event => { window['double'] = true; }); }",
        )
        .await;
        t.page.locator("button").dblclick().await.unwrap();
        assert_eq!(eval(&t.page, "double").await, true);
        assert_eq!(eval(&t.page, "result").await, "Clicked");
    });
}

#[test]
fn should_click_if_the_target_element_is_removed_in_pointerup_event() {
    run(|t| async move {
        t.page
            .set_content("<button id=clickme>Clickable</button>")
            .await
            .unwrap();
        eval_sel(
            &t.page,
            "#clickme",
            "element => element.addEventListener('pointerup', () => element.remove(), false)",
        )
        .await;
        t.page.locator("#clickme").click().await.unwrap();
    });
}

#[test]
fn should_click_if_the_target_element_is_removed_in_pointerdown_event() {
    run(|t| async move {
        t.page
            .set_content("<button id=clickme>Clickable</button>")
            .await
            .unwrap();
        eval_sel(
            &t.page,
            "#clickme",
            "element => element.addEventListener('pointerdown', () => element.remove(), false)",
        )
        .await;
        t.page.locator("#clickme").click().await.unwrap();
    });
}
