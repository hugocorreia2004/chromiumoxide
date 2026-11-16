//! Port of `elementhandle-wait-for-element-state.spec.ts` (all tests ported).

use std::future::IntoFuture;

use oxidewright::{ElementHandle, ElementState, Page};

use super::rafraf;
use crate::common::*;

/// `giveItAChanceToResolve`: five double animation frames.
async fn give_it_a_chance_to_resolve(page: &Page) {
    rafraf(page, 5).await;
}

/// Start waiting for `state`, check it does not resolve early, run `after`
/// on the element and wait for the result.
async fn wait_then(page: &Page, element: &ElementHandle, state: ElementState, after: &str) {
    let task = tokio::spawn(element.wait_for_element_state(state).into_future());
    give_it_a_chance_to_resolve(page).await;
    assert!(!task.is_finished());
    element.evaluate::<()>(after).await.unwrap();
    task.await.unwrap().unwrap();
}

#[test]
fn should_wait_for_visible() {
    run(|t| async move {
        t.page
            .set_content("<div style='display:none'>content</div>")
            .await
            .unwrap();
        let div = t.page.query_selector("div").await.unwrap().unwrap();
        wait_then(
            &t.page,
            &div,
            ElementState::Visible,
            "div => { div.style.display = 'block'; }",
        )
        .await;
    });
}

#[test]
fn should_wait_for_already_visible() {
    run(|t| async move {
        t.page.set_content("<div>content</div>").await.unwrap();
        let div = t.page.query_selector("div").await.unwrap().unwrap();
        div.wait_for_element_state(ElementState::Visible)
            .await
            .unwrap();
    });
}

#[test]
fn should_timeout_waiting_for_visible() {
    run(|t| async move {
        t.page
            .set_content("<div style='display:none'>content</div>")
            .await
            .unwrap();
        let div = t.page.query_selector("div").await.unwrap().unwrap();
        assert_err_contains(
            div.wait_for_element_state(ElementState::Visible)
                .timeout(1000)
                .await,
            "Timeout 1000ms exceeded",
        );
    });
}

#[test]
fn should_throw_waiting_for_visible_when_detached() {
    run(|t| async move {
        t.page
            .set_content("<div style='display:none'>content</div>")
            .await
            .unwrap();
        let div = t.page.query_selector("div").await.unwrap().unwrap();
        let task = tokio::spawn(
            div.wait_for_element_state(ElementState::Visible)
                .into_future(),
        );
        div.evaluate::<()>("div => div.remove()").await.unwrap();
        assert_err_contains(task.await.unwrap(), "Element is not attached to the DOM");
    });
}

#[test]
fn should_wait_for_hidden() {
    run(|t| async move {
        t.page.set_content("<div>content</div>").await.unwrap();
        let div = t.page.query_selector("div").await.unwrap().unwrap();
        wait_then(
            &t.page,
            &div,
            ElementState::Hidden,
            "div => { div.style.display = 'none'; }",
        )
        .await;
    });
}

#[test]
fn should_wait_for_already_hidden() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let div = t.page.query_selector("div").await.unwrap().unwrap();
        div.wait_for_element_state(ElementState::Hidden)
            .await
            .unwrap();
    });
}

#[test]
fn should_wait_for_hidden_when_detached() {
    run(|t| async move {
        t.page.set_content("<div>content</div>").await.unwrap();
        let div = t.page.query_selector("div").await.unwrap().unwrap();
        wait_then(&t.page, &div, ElementState::Hidden, "div => div.remove()").await;
    });
}

#[test]
fn should_throw_waiting_for_enabled_when_detached() {
    run(|t| async move {
        t.page
            .set_content("<button disabled>Target</button>")
            .await
            .unwrap();
        let button = t.page.query_selector("button").await.unwrap().unwrap();
        let task = tokio::spawn(
            button
                .wait_for_element_state(ElementState::Enabled)
                .into_future(),
        );
        button
            .evaluate::<()>("button => button.remove()")
            .await
            .unwrap();
        assert_err_contains(task.await.unwrap(), "Element is not attached to the DOM");
    });
}

#[test]
fn should_wait_for_aria_enabled_button() {
    run(|t| async move {
        t.page
            .set_content("<button aria-disabled=true><span>Target</span></button>")
            .await
            .unwrap();
        let span = t.page.query_selector("text=Target").await.unwrap().unwrap();
        wait_then(
            &t.page,
            &span,
            ElementState::Enabled,
            "span => span.parentElement.setAttribute('aria-disabled', 'false')",
        )
        .await;
    });
}

#[test]
fn should_wait_for_button_with_an_aria_disabled_parent() {
    run(|t| async move {
        t.page
            .set_content(
                "<div role=\"group\" aria-disabled=true><button><span>Target</span></button></div>",
            )
            .await
            .unwrap();
        let span = t.page.query_selector("text=Target").await.unwrap().unwrap();
        wait_then(
            &t.page,
            &span,
            ElementState::Enabled,
            "span => span.parentElement.parentElement.setAttribute('aria-disabled', 'false')",
        )
        .await;
    });
}

#[test]
fn should_wait_for_stable_position() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/button.html", t.server.prefix))
            .await
            .unwrap();
        let button = t.page.query_selector("button").await.unwrap().unwrap();
        t.page
            .eval_on_selector::<()>(
                "button",
                "button => { button.style.transition = 'margin 10000ms linear 0s'; button.style.marginLeft = '20000px'; }",
            )
            .await
            .unwrap();
        rafraf(&t.page, 1).await;
        wait_then(
            &t.page,
            &button,
            ElementState::Stable,
            "button => { button.style.transition = ''; }",
        )
        .await;
    });
}

#[test]
fn should_wait_for_editable_input() {
    run(|t| async move {
        t.page.set_content("<input readonly>").await.unwrap();
        let input = t.page.query_selector("input").await.unwrap().unwrap();
        wait_then(
            &t.page,
            &input,
            ElementState::Editable,
            "input => { input.readOnly = false; }",
        )
        .await;
    });
}
