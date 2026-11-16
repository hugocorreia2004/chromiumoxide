//! Ports of `page-click-timeout-1.spec.ts` .. `page-click-timeout-4.spec.ts`
//! and `page-click-during-navigation.spec.ts`.
//!
//! Skipped (need Playwright's `__testHook*` action hooks):
//! - CL-110 should avoid side effects after timeout
//! - CL-116 should fail when element jumps during hit testing
//! - CL-121 should click for the second time after first timeout
//! - CL-109 should not fail with internal error upon navigation

use serde_json::Value;

use super::{eval, eval_sel, rafraf};
use crate::common::*;

fn button_page(t: &TestContext) -> String {
    format!("{}/input/button.html", t.server.prefix)
}

// ---- page-click-timeout-1.spec.ts ----

#[test]
fn should_timeout_waiting_for_button_to_be_enabled() {
    run(|t| async move {
        t.page
            .set_content("<button onclick=\"javascript:window.__CLICKED=true;\" disabled><span>Click target</span></button>")
            .await
            .unwrap();
        let result = t.page.click("text=Click target").timeout(3000).await;
        assert_eq!(eval(&t.page, "window.__CLICKED").await, Value::Null);
        let error = assert_err_contains(result, "page.click: Timeout 3000ms exceeded.").to_string();
        assert!(error.contains("element is not enabled"), "{error}");
        assert!(error.contains("retrying click action"), "{error}");
    });
}

#[test]
#[ignore = "the bundled injected script (1.64) has no 'non-inert' element state"]
fn should_timeout_waiting_for_inert_element_to_become_non_inert() {
    run(|t| async move {
        t.page
            .set_content("<div inert><button onclick=\"javascript:window.__CLICKED=true;\">Click target</button></div>")
            .await
            .unwrap();
        let result = t.page.click("text=Click target").timeout(3000).await;
        assert_eq!(eval(&t.page, "window.__CLICKED").await, Value::Null);
        let error = assert_err_contains(result, "page.click: Timeout 3000ms exceeded.").to_string();
        assert!(error.contains("element is inert"), "{error}");
        assert!(error.contains("retrying click action"), "{error}");
    });
}

// ---- page-click-timeout-2.spec.ts ----

async fn timeout_waiting_for(t: &TestContext, hide: &str) {
    t.page.goto(button_page(t)).await.unwrap();
    eval_sel(&t.page, "button", hide).await;
    let error = assert_err_contains(
        t.page.click("button").timeout(5000).await,
        "page.click: Timeout 5000ms exceeded.",
    )
    .to_string();
    assert!(
        error.contains("waiting for element to be visible, enabled and stable"),
        "{error}"
    );
    assert!(error.contains("element is not visible"), "{error}");
    assert!(error.contains("retrying click action"), "{error}");
}

#[test]
fn should_timeout_waiting_for_display_none_to_be_gone() {
    run(|t| async move { timeout_waiting_for(&t, "b => b.style.display = 'none'").await });
}

#[test]
fn should_timeout_waiting_for_visibility_hidden_to_be_gone() {
    run(|t| async move { timeout_waiting_for(&t, "b => b.style.visibility = 'hidden'").await });
}

// ---- page-click-timeout-3.spec.ts ----

const BLOCKER: &str = "() => { document.body.style.position = 'relative'; const blocker = document.createElement('div'); blocker.id = 'blocker'; blocker.style.position = 'absolute'; blocker.style.width = '400px'; blocker.style.height = 'HEIGHT'; blocker.style.left = '0'; blocker.style.top = '0'; document.body.appendChild(blocker); }";

#[test]
fn should_timeout_waiting_for_hit_target() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        let button = t.page.query_selector("button").await.unwrap().unwrap();
        eval(&t.page, &BLOCKER.replace("HEIGHT", "20px")).await;
        let error = assert_err_contains(
            button.click().timeout(5000).await,
            "elementHandle.click: Timeout 5000ms exceeded.",
        )
        .to_string();
        assert!(
            error.contains("<div id=\"blocker\"></div> intercepts pointer events"),
            "{error}"
        );
        assert!(error.contains("retrying click action"), "{error}");
        assert!(error.contains("waiting 500ms"), "{error}");
    });
}

#[test]
fn should_still_click_when_force_but_hit_target_is_obscured() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        let button = t.page.query_selector("button").await.unwrap().unwrap();
        eval(&t.page, &BLOCKER.replace("HEIGHT", "200px")).await;
        button.click().force(true).await.unwrap();
    });
}

#[test]
fn should_report_wrong_hit_target_subtree() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        let button = t.page.query_selector("button").await.unwrap().unwrap();
        eval(&t.page, &BLOCKER.replace("HEIGHT", "20px")).await;
        eval(
            &t.page,
            "() => { const inner = document.createElement('div'); inner.id = 'inner'; inner.style.position = 'absolute'; inner.style.left = '0'; inner.style.top = '0'; inner.style.right = '0'; inner.style.bottom = '0'; document.getElementById('blocker').appendChild(inner); }",
        )
        .await;
        let error = assert_err_contains(
            button.click().timeout(5000).await,
            "elementHandle.click: Timeout 5000ms exceeded.",
        )
        .to_string();
        assert!(
            error.contains(
                "<div id=\"inner\"></div> from <div id=\"blocker\">…</div> subtree intercepts pointer events"
            ),
            "{error}"
        );
        assert!(error.contains("retrying click action"), "{error}");
    });
}

// ---- page-click-timeout-4.spec.ts ----

#[test]
fn should_timeout_waiting_for_stable_position() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        let button = t.page.query_selector("button").await.unwrap().unwrap();
        button
            .evaluate::<Value>(
                "button => { button.style.transition = 'margin 5s linear 0s'; button.style.marginLeft = '200px'; }",
            )
            .await
            .unwrap();
        rafraf(&t.page, 1).await;
        let error = assert_err_contains(
            button.click().timeout(3000).await,
            "elementHandle.click: Timeout 3000ms exceeded.",
        )
        .to_string();
        assert!(
            error.contains("waiting for element to be visible, enabled and stable"),
            "{error}"
        );
        assert!(error.contains("element is not stable"), "{error}");
        assert!(error.contains("retrying click action"), "{error}");
    });
}

#[test]
fn should_fail_to_click_the_button_behind_a_large_header_after_scrolling_around() {
    run(|t| async move {
        t.page.set_viewport_size((500, 240)).await.unwrap();
        t.page
            .set_content(
                r#"<style>
                * { padding: 0; margin: 0; }
                li { height: 80px; border: 1px solid black; }
                ol { padding-top: 160px; }
                div.fixed { position: fixed; z-index: 1001; width: 100%; background: rgba(255, 0, 0, 0.2); height: 2000px; }
                </style>
                <div class=fixed></div>
                <ol>
                <li>hi1</li><li>hi2</li><li>hi3</li><li>hi4</li><li>hi5</li><li>hi6</li><li>hi7</li><li>hi8</li>
                <li id=target onclick="window.__clicked = true">hi9</li>
                <li>hi10</li><li>hi11</li><li>hi12</li><li>hi13</li><li id=li14>hi14</li>
                </ol>
                <script>
                  window.scrollTops = [];
                  window.addEventListener('scroll', () => {
                    window.scrollTops.push(window.scrollY);
                  });
                </script>"#,
            )
            .await
            .unwrap();
        eval_sel(&t.page, "#li14", "e => e.scrollIntoView()").await;
        assert_err_contains(
            t.page.click("#target").timeout(3500).await,
            "<div class=\"fixed\"></div> intercepts pointer events",
        );
        assert_eq!(
            eval(&t.page, "() => window['__clicked']").await,
            Value::Null
        );
        let scroll_tops = eval(&t.page, "() => window['scrollTops']").await;
        let mut distinct: Vec<String> = scroll_tops
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.to_string())
            .collect();
        distinct.sort();
        distinct.dedup();
        assert!(distinct.len() > 2, "{scroll_tops}");
    });
}
