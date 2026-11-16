//! Port of `locator-any-frame.spec.ts` (`frameLocator()` without a selector,
//! exposed as `frame_locator_any()`). `page.route(...)` fulfillments are
//! replaced by routes of the test server.
//!
//! Skipped:
//! - LO-047 `should resolve aria-ref selectors`: needs `page.ariaSnapshot({ mode: 'ai' })`.

use oxidewright::{GetByRoleOptions, LoadState, LocatorOptions, Page, WaitForSelectorState};
use serde_json::Value;

use super::*;
use crate::common::*;

fn route(t: &TestContext, path: &str, body: &'static str) {
    t.server.set_content(&format!("/{path}"), "text/html", body);
}

async fn wait_for_all_frames(page: &Page, count: usize, selector: &str) {
    expect_frame_count(page, count).await;
    for frame in page.frames() {
        if frame != page.main_frame() {
            frame
                .wait_for_selector(selector)
                .state(WaitForSelectorState::Attached)
                .await
                .unwrap();
        }
    }
}

fn frame_with(page: &Page, url_part: &str) -> oxidewright::Frame {
    page.frames()
        .into_iter()
        .find(|f| f.url().contains(url_part))
        .unwrap()
}

#[test]
fn should_click_a_button_inside_an_iframe() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(
            &t,
            "a.html",
            "<button onclick=\"window.__clicked = true\">Click me</button>",
        );
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .frame_locator_any()
            .get_by_role_with("button", GetByRoleOptions::new().name("Click me"))
            .click()
            .await
            .unwrap();
        let clicked: bool = t.page.frames()[1]
            .evaluate("() => window.__clicked")
            .await
            .unwrap();
        assert!(clicked);
    });
}

#[test]
fn should_click_a_button_in_the_main_frame() {
    run(|t| async move {
        route(
            &t,
            "empty.html",
            "<iframe src=\"a.html\"></iframe><button onclick=\"window.__clicked = true\">Click me</button>",
        );
        route(&t, "a.html", "<div>No buttons here</div>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .frame_locator_any()
            .locator("button")
            .click()
            .await
            .unwrap();
        let clicked: bool = t.page.evaluate("() => window.__clicked").await.unwrap();
        assert!(clicked);
    });
}

#[test]
fn should_fail_click_when_elements_match_in_multiple_frames() {
    run(|t| async move {
        route(
            &t,
            "empty.html",
            "<iframe src=\"a.html\"></iframe><iframe src=\"b.html\"></iframe>",
        );
        route(&t, "a.html", "<button>one</button>");
        route(&t, "b.html", "<button>two</button>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        wait_for_all_frames(&t.page, 3, "button").await;
        let error = t
            .page
            .frame_locator_any()
            .locator("button")
            .click()
            .timeout(10_000)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("frameLocator() matched elements in multiple frames"),
            "{error}"
        );
        assert!(
            error.contains("waiting for frameLocator().locator('button')"),
            "{error}"
        );
    });
}

#[test]
fn should_fail_click_upon_strict_mode_violation_inside_a_single_frame() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(&t, "a.html", "<button>one</button><button>two</button>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        wait_for_all_frames(&t.page, 2, "button").await;
        let error = t
            .page
            .frame_locator_any()
            .locator("button")
            .click()
            .timeout(10_000)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("strict mode violation"), "{error}");
        assert!(
            error.contains("waiting for frameLocator().locator('button')"),
            "{error}"
        );
    });
}

#[test]
fn should_time_out_on_click_when_there_are_no_matches() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(&t, "a.html", "<div>Nothing here</div>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        let error = t
            .page
            .frame_locator_any()
            .locator("button")
            .click()
            .timeout(1000)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("Timeout 1000ms exceeded"), "{error}");
        assert!(
            error.contains("waiting for frameLocator().locator('button')"),
            "{error}"
        );
    });
}

#[test]
fn should_count_elements_in_a_single_frame() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(&t, "a.html", "<div>1</div><div>2</div><div>3</div>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        wait_for_all_frames(&t.page, 2, "div").await;
        assert_eq!(
            t.page
                .frame_locator_any()
                .locator("div")
                .count()
                .await
                .unwrap(),
            3
        );
        assert_eq!(
            t.page
                .frame_locator_any()
                .locator("button")
                .count()
                .await
                .unwrap(),
            0
        );
    });
}

#[test]
fn should_fail_count_when_elements_match_in_multiple_frames() {
    run(|t| async move {
        route(
            &t,
            "empty.html",
            "<div>main</div><iframe src=\"a.html\"></iframe>",
        );
        route(&t, "a.html", "<div>child</div>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        wait_for_all_frames(&t.page, 2, "div").await;
        let error = t
            .page
            .frame_locator_any()
            .locator("div")
            .count()
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("frameLocator() matched elements in multiple frames")
        );
    });
}

#[test]
fn should_support_to_have_count() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(&t, "a.html", "<span>one</span><span>two</span>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        expect_count(&t.page.frame_locator_any().locator("span"), 2).await;
        expect_count(&t.page.frame_locator_any().locator("button"), 0).await;
    });
}

#[test]
fn should_wait_for_a_frame_to_appear_with_to_have_count() {
    run(|t| async move {
        route(&t, "empty.html", "<div>No frames yet</div>");
        route(&t, "a.html", "<span>one</span><span>two</span>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .evaluate::<Value>(
                "() => { setTimeout(() => { const iframe = document.createElement('iframe'); iframe.src = 'a.html'; document.body.appendChild(iframe); }, 500); }",
            )
            .await
            .unwrap();
        expect_count(&t.page.frame_locator_any().locator("span"), 2).await;
    });
}

#[test]
fn should_fail_to_have_count_when_elements_match_in_multiple_frames() {
    run(|t| async move {
        route(
            &t,
            "empty.html",
            "<iframe src=\"a.html\"></iframe><iframe src=\"b.html\"></iframe>",
        );
        route(&t, "a.html", "<span>one</span>");
        route(&t, "b.html", "<span>two</span>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        wait_for_all_frames(&t.page, 3, "span").await;
        let error = try_count(&t.page.frame_locator_any().locator("span"), 2)
            .await
            .unwrap_err();
        assert!(
            error.contains("frameLocator() matched elements in multiple frames"),
            "{error}"
        );
    });
}

#[test]
fn should_support_to_have_text() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(&t, "a.html", "<div>Hello iframe</div>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        expect_text(&t.page.frame_locator_any().locator("div"), "Hello iframe").await;
    });
}

#[test]
fn should_support_to_have_text_with_an_array() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(&t, "a.html", "<span>one</span><span>two</span>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        expect_texts(&t.page.frame_locator_any().locator("span"), &["one", "two"]).await;
    });
}

#[test]
fn should_fail_to_have_text_when_elements_match_in_multiple_frames() {
    run(|t| async move {
        route(
            &t,
            "empty.html",
            "<iframe src=\"a.html\"></iframe><iframe src=\"b.html\"></iframe>",
        );
        route(&t, "a.html", "<div>one</div>");
        route(&t, "b.html", "<div>two</div>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        wait_for_all_frames(&t.page, 3, "div").await;
        let error = try_text(&t.page.frame_locator_any().locator("div"), "one")
            .await
            .unwrap_err();
        assert!(
            error.contains("frameLocator() matched elements in multiple frames"),
            "{error}"
        );
    });
}

#[test]
fn should_fail_to_have_text_with_an_array_when_elements_match_in_multiple_frames() {
    run(|t| async move {
        route(
            &t,
            "empty.html",
            "<iframe src=\"a.html\"></iframe><iframe src=\"b.html\"></iframe>",
        );
        route(&t, "a.html", "<span>one</span>");
        route(&t, "b.html", "<span>two</span>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        wait_for_all_frames(&t.page, 3, "span").await;
        let error = try_texts(&t.page.frame_locator_any().locator("span"), &["one", "two"])
            .await
            .unwrap_err();
        assert!(
            error.contains("frameLocator() matched elements in multiple frames"),
            "{error}"
        );
    });
}

#[test]
fn should_fail_to_have_text_upon_strict_mode_violation_inside_a_single_frame() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(&t, "a.html", "<div>one</div><div>two</div>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        wait_for_all_frames(&t.page, 2, "div").await;
        let error = try_text(&t.page.frame_locator_any().locator("div"), "one")
            .await
            .unwrap_err();
        assert!(error.contains("strict mode violation"), "{error}");
    });
}

#[test]
fn should_support_evaluate() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(&t, "a.html", "<div data-foo=\"bar\">Hello</div>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        let foo: String = t
            .page
            .frame_locator_any()
            .locator("div")
            .evaluate("e => e.getAttribute('data-foo')")
            .await
            .unwrap();
        assert_eq!(foo, "bar");
    });
}

#[test]
fn should_fail_evaluate_when_elements_match_in_multiple_frames() {
    run(|t| async move {
        route(
            &t,
            "empty.html",
            "<iframe src=\"a.html\"></iframe><iframe src=\"b.html\"></iframe>",
        );
        route(&t, "a.html", "<div>one</div>");
        route(&t, "b.html", "<div>two</div>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        wait_for_all_frames(&t.page, 3, "div").await;
        let error = t
            .page
            .frame_locator_any()
            .locator("div")
            .evaluate::<Value>("e => e.textContent")
            .timeout(10_000)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("frameLocator() matched elements in multiple frames")
        );
    });
}

#[test]
fn should_time_out_on_evaluate_when_there_are_no_matches() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(&t, "a.html", "<div>Nothing here</div>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        let error = t
            .page
            .frame_locator_any()
            .locator("button")
            .evaluate::<Value>("e => e.textContent")
            .timeout(1000)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("Timeout 1000ms exceeded"), "{error}");
        assert!(
            error.contains("waiting for frameLocator().locator('button')"),
            "{error}"
        );
    });
}

#[test]
fn should_support_evaluate_all() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(&t, "a.html", "<span>one</span><span>two</span>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        wait_for_all_frames(&t.page, 2, "span").await;
        let texts: Vec<String> = t
            .page
            .frame_locator_any()
            .locator("span")
            .evaluate_all("els => els.map(e => e.textContent)")
            .await
            .unwrap();
        assert_eq!(texts, vec!["one", "two"]);
        let n: usize = t
            .page
            .frame_locator_any()
            .locator("button")
            .evaluate_all("els => els.length")
            .await
            .unwrap();
        assert_eq!(n, 0);
    });
}

#[test]
fn should_fail_evaluate_all_when_elements_match_in_multiple_frames() {
    run(|t| async move {
        route(
            &t,
            "empty.html",
            "<iframe src=\"a.html\"></iframe><iframe src=\"b.html\"></iframe>",
        );
        route(&t, "a.html", "<span>one</span>");
        route(&t, "b.html", "<span>two</span>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        wait_for_all_frames(&t.page, 3, "span").await;
        let error = t
            .page
            .frame_locator_any()
            .locator("span")
            .evaluate_all::<usize>("els => els.length")
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("frameLocator() matched elements in multiple frames")
        );
    });
}

#[test]
fn should_support_has_text_filter() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(&t, "a.html", "<div>foo</div><div>bar</div>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        expect_text(
            &t.page
                .frame_locator_any()
                .locator_with("div", LocatorOptions::new().has_text("bar")),
            "bar",
        )
        .await;
    });
}

#[test]
fn should_support_first_last_nth() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(
            &t,
            "a.html",
            "<span>one</span><span>two</span><span>three</span>",
        );
        t.page.goto(&t.server.empty_page).await.unwrap();
        wait_for_all_frames(&t.page, 2, "span").await;
        let spans = || t.page.frame_locator_any().locator("span");
        expect_text(&spans().first(), "one").await;
        expect_text(&spans().last(), "three").await;
        expect_text(&spans().nth(1), "two").await;
    });
}

#[test]
fn should_support_nth_in_the_middle_of_the_chain() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(
            &t,
            "a.html",
            "<div><span>one</span></div><div><span>two</span></div>",
        );
        t.page.goto(&t.server.empty_page).await.unwrap();
        expect_text(
            &t.page
                .frame_locator_any()
                .locator("div")
                .nth(1)
                .locator("span"),
            "two",
        )
        .await;
    });
}

#[test]
fn should_support_composite_locators() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(
            &t,
            "a.html",
            "<div><span>foo</span></div><div><i>bar</i></div>",
        );
        t.page.goto(&t.server.empty_page).await.unwrap();
        expect_text(
            &t.page
                .frame_locator_any()
                .locator_with("div", LocatorOptions::new().has(&t.page.locator("span"))),
            "foo",
        )
        .await;
    });
}

#[test]
fn should_support_capture() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(&t, "a.html", "<div id=\"target\"><span>hello</span></div>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        expect_attribute(
            &t.page.frame_locator_any().locator("*css=div >> span"),
            "id",
            "target",
        )
        .await;
    });
}

async fn scoped_buttons(page: &Page) -> Vec<String> {
    let scope = page.query_selector("section").await.unwrap().unwrap();
    let buttons = scope
        .query_selector_all("internal:control=any-frame >> button")
        .await
        .unwrap();
    let mut texts = Vec::new();
    for b in buttons {
        texts.push(b.text_content().await.unwrap().unwrap_or_default());
    }
    texts
}

#[test]
fn should_find_a_frame_inside_the_scope() {
    run(|t| async move {
        route(
            &t,
            "empty.html",
            "<section><iframe src=\"a.html\"></iframe></section><button>outside</button>",
        );
        route(&t, "a.html", "<button>inside</button>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        wait_for_all_frames(&t.page, 2, "button").await;
        assert_eq!(scoped_buttons(&t.page).await, vec!["inside"]);
    });
}

#[test]
fn should_find_a_nested_frame_inside_the_scope() {
    run(|t| async move {
        route(
            &t,
            "empty.html",
            "<section><iframe src=\"a.html\"></iframe></section><iframe src=\"b.html\"></iframe>",
        );
        route(&t, "a.html", "<iframe src=\"c.html\"></iframe>");
        route(&t, "b.html", "<button>outside</button>");
        route(&t, "c.html", "<button>deep</button>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        expect_frame_count(&t.page, 4).await;
        for frame in t.page.frames() {
            if frame.url().contains("b.html") || frame.url().contains("c.html") {
                frame
                    .wait_for_selector("button")
                    .state(WaitForSelectorState::Attached)
                    .await
                    .unwrap();
            }
        }
        assert_eq!(scoped_buttons(&t.page).await, vec!["deep"]);
    });
}

#[test]
fn should_find_a_frame_inside_the_scope_while_another_iframe_is_stalled() {
    run(|t| async move {
        route(
            &t,
            "empty.html",
            "<section><iframe src=\"a.html\"></iframe><iframe src=\"stall.html\"></iframe></section>",
        );
        route(&t, "a.html", "<button>inside</button>");
        t.server.hang("/stall.html");
        t.page
            .goto(&t.server.empty_page)
            .wait_until(LoadState::DomContentLoaded)
            .await
            .unwrap();
        expect_frame_count(&t.page, 3).await;
        // Unlike Playwright's test, wait for a.html to render its button.
        let mut buttons = Vec::new();
        for _ in 0..50 {
            buttons = scoped_buttons(&t.page).await;
            if !buttons.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert_eq!(buttons, vec!["inside"]);
    });
}

#[test]
fn should_respect_the_scope_without_a_frame_inside_the_scope() {
    run(|t| async move {
        route(
            &t,
            "empty.html",
            "<section><button>target</button></section><iframe src=\"a.html\"></iframe>",
        );
        route(&t, "a.html", "<button>in-frame</button>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        wait_for_all_frames(&t.page, 2, "button").await;
        assert_eq!(scoped_buttons(&t.page).await, vec!["target"]);
    });
}

#[test]
fn should_not_match_a_chain_across_a_frame_boundary() {
    run(|t| async move {
        route(
            &t,
            "empty.html",
            "<section><iframe src=\"a.html\"></iframe></section>",
        );
        route(&t, "a.html", "<iframe src=\"b.html\"></iframe>");
        route(&t, "b.html", "<button>deep</button>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        expect_frame_count(&t.page, 3).await;
        frame_with(&t.page, "b.html")
            .wait_for_selector("button")
            .state(WaitForSelectorState::Attached)
            .await
            .unwrap();
        expect_count(
            &t.page
                .frame_locator_any()
                .locator("section")
                .locator("button"),
            0,
        )
        .await;
        expect_text(&t.page.frame_locator_any().locator("button"), "deep").await;
    });
}

#[test]
fn should_only_search_frames_inside_the_starting_frame() {
    run(|t| async move {
        route(
            &t,
            "empty.html",
            "<iframe src=\"a.html\"></iframe><button>main</button>",
        );
        route(&t, "a.html", "<iframe src=\"b.html\"></iframe>");
        route(&t, "b.html", "<button>deep</button>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        expect_frame_count(&t.page, 3).await;
        frame_with(&t.page, "b.html")
            .wait_for_selector("button")
            .state(WaitForSelectorState::Attached)
            .await
            .unwrap();
        let middle = frame_with(&t.page, "a.html");
        expect_text(&middle.frame_locator_any().locator("button"), "deep").await;
    });
}

#[test]
fn should_enter_a_frame_found_in_a_nested_frame() {
    run(|t| async move {
        route(
            &t,
            "empty.html",
            "<iframe src=\"a.html\"></iframe><button>main</button>",
        );
        route(
            &t,
            "a.html",
            "<iframe id=\"target\" src=\"b.html\"></iframe><button>decoy</button>",
        );
        route(&t, "b.html", "<button>inside</button>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        expect_text(
            &t.page
                .frame_locator_any()
                .frame_locator("#target")
                .locator("button"),
            "inside",
        )
        .await;
    });
}

#[test]
fn should_click_inside_an_entered_frame() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(
            &t,
            "a.html",
            "<iframe id=\"target\" src=\"b.html\"></iframe><button>Click me</button>",
        );
        route(
            &t,
            "b.html",
            "<button onclick=\"window.__clicked = true\">Click me</button>",
        );
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .frame_locator_any()
            .frame_locator("#target")
            .get_by_role_with("button", GetByRoleOptions::new().name("Click me"))
            .click()
            .await
            .unwrap();
        let clicked: bool = frame_with(&t.page, "b.html")
            .evaluate("() => window.__clicked")
            .await
            .unwrap();
        assert!(clicked);
    });
}

#[test]
fn should_not_search_nested_frames_after_entering_a_frame() {
    run(|t| async move {
        route(
            &t,
            "empty.html",
            "<iframe id=\"target\" src=\"a.html\"></iframe><button>main</button>",
        );
        route(&t, "a.html", "<iframe src=\"b.html\"></iframe>");
        route(&t, "b.html", "<button>deep</button>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        expect_frame_count(&t.page, 3).await;
        expect_count(
            &t.page
                .frame_locator_any()
                .frame_locator("#target")
                .locator("button"),
            0,
        )
        .await;
    });
}

#[test]
fn should_support_two_frame_locators() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(&t, "a.html", "<iframe id=\"x\" src=\"b.html\"></iframe>");
        route(
            &t,
            "b.html",
            "<iframe id=\"y\" src=\"c.html\"></iframe><button>decoy</button>",
        );
        route(&t, "c.html", "<button>bottom</button>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        expect_text(
            &t.page
                .frame_locator_any()
                .frame_locator("#x")
                .frame_locator("#y")
                .locator("button"),
            "bottom",
        )
        .await;
    });
}

#[test]
fn should_support_locator_before_frame_locator() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(
            &t,
            "a.html",
            "<section><iframe src=\"b.html\"></iframe></section><iframe src=\"c.html\"></iframe>",
        );
        route(&t, "b.html", "<button>in-section</button>");
        route(&t, "c.html", "<button>outside</button>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        expect_text(
            &t.page
                .frame_locator_any()
                .locator("section")
                .frame_locator("iframe")
                .locator("button"),
            "in-section",
        )
        .await;
    });
}

#[test]
fn should_support_owner_of_a_frame_locator() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(
            &t,
            "a.html",
            "<iframe id=\"target\" src=\"b.html\"></iframe>",
        );
        route(&t, "b.html", "<button>inside</button>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        let id = t
            .page
            .frame_locator_any()
            .frame_locator("#target")
            .owner()
            .get_attribute("id")
            .await
            .unwrap();
        assert_eq!(id.as_deref(), Some("target"));
    });
}

#[test]
fn should_wait_for_the_frame_to_enter_to_appear() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(&t, "a.html", "<div>Nothing yet</div>");
        route(&t, "b.html", "<button>late</button>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        expect_frame_count(&t.page, 2).await;
        t.page.frames()[1]
            .evaluate::<Value>(
                "() => { setTimeout(() => { const iframe = document.createElement('iframe'); iframe.id = 'late'; iframe.src = 'b.html'; document.body.appendChild(iframe); }, 3000); }",
            )
            .await
            .unwrap();
        let button = t
            .page
            .frame_locator_any()
            .frame_locator("#late")
            .locator("button");
        // The frame appears after 3 s; wait for it before the 5 s text assertion.
        button.wait_for().await.unwrap();
        expect_text(&button, "late").await;
    });
}

#[test]
fn should_fail_when_the_frame_to_enter_matches_in_multiple_frames() {
    run(|t| async move {
        route(
            &t,
            "empty.html",
            "<iframe src=\"a.html\"></iframe><iframe src=\"b.html\"></iframe>",
        );
        route(
            &t,
            "a.html",
            "<iframe class=\"inner\" src=\"c.html\"></iframe>",
        );
        route(
            &t,
            "b.html",
            "<iframe class=\"inner\" src=\"c.html\"></iframe>",
        );
        route(&t, "c.html", "<button>Click me</button>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        expect_frame_count(&t.page, 5).await;
        for frame in t.page.frames() {
            if frame.url().contains("c.html") {
                frame
                    .wait_for_selector("button")
                    .state(WaitForSelectorState::Attached)
                    .await
                    .unwrap();
            }
        }
        let error = t
            .page
            .frame_locator_any()
            .frame_locator(".inner")
            .locator("button")
            .click()
            .timeout(10_000)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("frameLocator() matched elements in multiple frames")
        );
    });
}

#[test]
fn should_support_content_frame() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(
            &t,
            "a.html",
            "<iframe id=\"target\" src=\"b.html\"></iframe><button>decoy</button>",
        );
        route(&t, "b.html", "<button>inside</button>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        expect_text(
            &t.page
                .frame_locator_any()
                .locator("#target")
                .content_frame()
                .locator("button"),
            "inside",
        )
        .await;
    });
}

#[test]
fn should_render_frame_locator_in_the_locator_description() {
    run(|t| async move {
        assert_eq!(
            t.page
                .frame_locator_any()
                .frame_locator("#x")
                .locator("button")
                .to_string(),
            "frameLocator().locator('#x').contentFrame().locator('button')"
        );
        assert_eq!(
            t.page
                .frame_locator_any()
                .locator("section")
                .frame_locator("iframe")
                .get_by_text("foo")
                .to_string(),
            "frameLocator().locator('section').locator('iframe').contentFrame().getByText('foo')"
        );
    });
}

#[test]
fn should_not_allow_frame_locator_inside_a_composite_locator() {
    run(|t| async move {
        let p = &t.page;
        route(
            &t,
            "empty.html",
            "<button>main</button><iframe src=\"a.html\"></iframe>",
        );
        route(&t, "a.html", "<a href=\"#\">link</a>");
        p.goto(&t.server.empty_page).await.unwrap();
        wait_for_all_frames(p, 2, "a").await;
        let any = || p.frame_locator_any();

        let error = p
            .locator("button")
            .or_(&any().locator("a"))
            .count()
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("frameLocator() is not allowed inside composite locators, while querying \"locator('button').or(frameLocator().locator('a'))\""), "{error}");

        let error2 = p
            .locator("button")
            .filter(LocatorOptions::new().has(&any().locator("a")))
            .count()
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error2.contains("frameLocator() is not allowed inside composite locators"),
            "{error2}"
        );

        let error3 = any()
            .locator("button")
            .or_(&any().locator("a"))
            .count()
            .await
            .unwrap_err()
            .to_string();
        assert!(error3.contains("frameLocator() is not allowed inside composite locators, while querying \"frameLocator().locator('button').or(frameLocator().locator('a'))\""), "{error3}");

        let error4 = any()
            .frame_locator("#f")
            .locator("a")
            .or_(&any().frame_locator("#f").locator("button"))
            .count()
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error4.contains("frameLocator() is not allowed inside composite locators"),
            "{error4}"
        );

        let error5 = any()
            .locator("a")
            .or_(&p.locator("button"))
            .count()
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error5.contains("frameLocator() matched elements in multiple frames"),
            "{error5}"
        );
    });
}

#[test]
fn should_support_a_composite_locator_under_frame_locator() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(
            &t,
            "a.html",
            "<div class=\"classname\">first</div><button>second</button>",
        );
        t.page.goto(&t.server.empty_page).await.unwrap();
        wait_for_all_frames(&t.page, 2, "button").await;
        expect_texts(
            &t.page
                .frame_locator_any()
                .locator(".classname")
                .or_(&t.page.get_by_role("button")),
            &["first", "second"],
        )
        .await;
    });
}

#[test]
fn should_support_a_composite_locator_under_frame_locator_and_a_frame_locator() {
    run(|t| async move {
        route(&t, "empty.html", "<iframe src=\"a.html\"></iframe>");
        route(&t, "a.html", "<iframe id=\"f\" src=\"b.html\"></iframe>");
        route(
            &t,
            "b.html",
            "<div class=\"classname\">first</div><button>second</button>",
        );
        t.page.goto(&t.server.empty_page).await.unwrap();
        expect_frame_count(&t.page, 3).await;
        expect_texts(
            &t.page
                .frame_locator_any()
                .frame_locator("#f")
                .locator(".classname")
                .or_(&t.page.frame_locator("#f").get_by_role("button")),
            &["first", "second"],
        )
        .await;
    });
}

#[test]
fn should_not_allow_first_last_nth_on_frame_locator() {
    run(|t| async move {
        for fl in [
            t.page.frame_locator_any().first(),
            t.page.frame_locator_any().last(),
            t.page.frame_locator_any().nth(1),
        ] {
            let error = fl.locator("div").count().await.unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("Selecting the nth frame is not allowed on frameLocator()"),
                "{error}"
            );
        }
    });
}

#[test]
fn should_not_allow_owner_on_frame_locator() {
    run(|t| async move {
        let error = t
            .page
            .frame_locator_any()
            .owner()
            .count()
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Selector cannot be empty after frameLocator()"),
            "{error}"
        );
    });
}

#[test]
fn should_click_while_another_iframe_is_stalled() {
    run(|t| async move {
        route(
            &t,
            "empty.html",
            "<iframe src=\"a.html\"></iframe><iframe src=\"stall.html\"></iframe>",
        );
        route(
            &t,
            "a.html",
            "<button onclick=\"window.__clicked = true\">Click me</button>",
        );
        t.server.hang("/stall.html");
        t.page
            .goto(&t.server.empty_page)
            .wait_until(LoadState::DomContentLoaded)
            .await
            .unwrap();
        expect_frame_count(&t.page, 3).await;
        t.page
            .frame_locator_any()
            .locator("button")
            .click()
            .await
            .unwrap();
        let clicked: bool = frame_with(&t.page, "a.html")
            .evaluate("() => window.__clicked")
            .await
            .unwrap();
        assert!(clicked);
    });
}

#[test]
fn should_support_to_be_visible_while_another_iframe_is_stalled() {
    run(|t| async move {
        route(
            &t,
            "empty.html",
            "<iframe src=\"a.html\"></iframe><iframe src=\"stall.html\"></iframe>",
        );
        route(&t, "a.html", "<button>Click me</button>");
        t.server.hang("/stall.html");
        t.page
            .goto(&t.server.empty_page)
            .wait_until(LoadState::DomContentLoaded)
            .await
            .unwrap();
        expect_frame_count(&t.page, 3).await;
        expect_visible(&t.page.frame_locator_any().locator("button")).await;
    });
}

#[test]
fn should_support_to_have_count_while_another_iframe_is_stalled() {
    run(|t| async move {
        route(
            &t,
            "empty.html",
            "<iframe src=\"a.html\"></iframe><iframe src=\"stall.html\"></iframe>",
        );
        route(&t, "a.html", "<button>Click me</button>");
        t.server.hang("/stall.html");
        t.page
            .goto(&t.server.empty_page)
            .wait_until(LoadState::DomContentLoaded)
            .await
            .unwrap();
        expect_frame_count(&t.page, 3).await;
        expect_count(&t.page.frame_locator_any().locator("button"), 1).await;
    });
}

#[test]
fn should_treat_elements_inside_hidden_iframe_as_hidden() {
    run(|t| async move {
        route(
            &t,
            "empty.html",
            "<iframe src=\"a.html\" style=\"visibility: hidden\"></iframe>",
        );
        route(
            &t,
            "a.html",
            "<button onclick=\"window.__clicked = true\">Click me</button>",
        );
        t.page.goto(&t.server.empty_page).await.unwrap();
        let button = t.page.frame_locator_any().locator("button");
        expect_attached(&button).await;
        assert!(!button.is_visible().await.unwrap());
        expect_hidden(&button).await;
        let error = button.click().timeout(1000).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("element is inside a hidden frame, retrying"),
            "{error}"
        );
        let clicked: Option<bool> = t.page.frames()[1]
            .evaluate("() => window.__clicked")
            .await
            .unwrap();
        assert_eq!(clicked, None);
        t.page
            .evaluate::<Value>(
                "() => document.querySelector('iframe').style.visibility = 'visible'",
            )
            .await
            .unwrap();
        expect_visible(&button).await;
        button.click().await.unwrap();
        let clicked: Option<bool> = t.page.frames()[1]
            .evaluate("() => window.__clicked")
            .await
            .unwrap();
        assert_eq!(clicked, Some(true));
    });
}
