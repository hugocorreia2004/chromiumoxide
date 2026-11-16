//! Port of `page-click-scroll.spec.ts`.
//!
//! Skipped: CL-097 (`fixme` for Chromium upstream: `DOM.getBoxModel` does not
//! work for `display: contents`).

use serde_json::Value;

use super::eval;
use crate::common::*;

#[test]
fn should_not_hit_scroll_bar() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<style>
                  .categories { width: 180px; display: flex; overflow-x: scroll; }
                  button { flex: none; height: 28px; }
                </style>
                <div class="categories">
                  <button>One</button>
                  <button>Two</button>
                  <button>Three</button>
                  <button>Story</button>
                  <button>More</button>
                  <button>Items</button>
                  <button>Here</button>
                </div>"#,
            )
            .await
            .unwrap();
        t.page.click("text=Story").timeout(2000).await.unwrap();
    });
}

#[test]
fn should_scroll_into_view_display_contents() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<div style="background:red;height:2000px">filler</div>
                <div>
                  Example text, and button here:
                  <button style="display: contents" onclick="window._clicked=true;">click me</button>
                </div>"#,
            )
            .await
            .unwrap();
        t.page.click("text=click me").timeout(5000).await.unwrap();
        assert_eq!(eval(&t.page, "window._clicked").await, true);
    });
}

#[test]
fn should_scroll_into_view_display_contents_with_a_child() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<div style="background:red;height:2000px">filler</div>
                Example text, and button here:
                <button style="display: contents" onclick="window._clicked=true;"><div>click me</div></button>"#,
            )
            .await
            .unwrap();
        t.page.click("text=click me").timeout(5000).await.unwrap();
        assert_eq!(eval(&t.page, "window._clicked").await, true);
    });
}

#[test]
fn should_not_crash_when_force_clicking_hidden_input() {
    run(|t| async move {
        t.page.set_content("<input type=hidden>").await.unwrap();
        assert_err_contains(
            t.page
                .locator("input")
                .click()
                .force(true)
                .timeout(2000)
                .await,
            "Element is not visible",
        );
    });
}

#[test]
fn should_scroll_into_view_span_element() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<div id=big style="height: 10000px;"></div>
                <span id=small>foo</span>"#,
            )
            .await
            .unwrap();
        t.page
            .locator("#small")
            .scroll_into_view_if_needed()
            .await
            .unwrap();
        let scroll_y = eval(&t.page, "() => window.scrollY").await;
        assert!(scroll_y.as_f64().unwrap() > 9000.0, "{scroll_y}");
    });
}

#[test]
fn should_scroll_into_view_element_in_iframe() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .set_content(format!(
                r#"<div id=big style="height: 10000px;"></div>
                <iframe src='{}/input/button.html'></iframe>"#,
                t.server.cross_process_prefix
            ))
            .await
            .unwrap();
        t.page
            .frame_locator("iframe")
            .get_by_role("button")
            .click()
            .timeout(5000)
            .await
            .unwrap();
    });
}

#[test]
fn should_not_scroll_the_page_when_scroll_is_none() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<div style="height: 2000px;"></div>
                <button onclick="window._clicked=true">click me</button>"#,
            )
            .await
            .unwrap();
        assert_err_contains(
            t.page
                .locator("button")
                .click()
                .scroll(false)
                .timeout(2000)
                .await,
            "element is outside of the viewport",
        );
        assert_eq!(eval(&t.page, "window._clicked").await, Value::Null);
        assert_eq!(eval(&t.page, "() => window.scrollY").await, 0);
    });
}

#[test]
fn should_click_in_viewport_element_when_scroll_is_none() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<button onclick="window._clicked=true">click me</button>
                <div style="height: 2000px;"></div>"#,
            )
            .await
            .unwrap();
        t.page
            .locator("button")
            .click()
            .scroll(false)
            .timeout(2000)
            .await
            .unwrap();
        assert_eq!(eval(&t.page, "window._clicked").await, true);
        assert_eq!(eval(&t.page, "() => window.scrollY").await, 0);
    });
}

#[test]
fn should_not_scroll_nested_container_when_scroll_is_none() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<div style="height: 100px; width: 100px; overflow-y: scroll;">
                  <div style="height: 50px;">A</div>
                  <div style="height: 50px;">B</div>
                  <button style="height: 50px; width: 100px;" onclick="window._clicked=true">C</button>
                </div>"#,
            )
            .await
            .unwrap();
        let button = t.page.locator("button");
        assert!(button.click().scroll(false).timeout(2000).await.is_err());
        assert_eq!(eval(&t.page, "window._clicked").await, Value::Null);
        button.click().timeout(2000).await.unwrap();
        assert_eq!(eval(&t.page, "window._clicked").await, true);
    });
}

#[test]
fn should_not_scroll_on_hover_when_scroll_is_none() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<div style="height: 2000px;"></div>
                <div onmouseover="window._hovered=true" style="width: 50px; height: 50px;">hover me</div>"#,
            )
            .await
            .unwrap();
        assert_err_contains(
            t.page
                .locator("div >> text=hover me")
                .hover()
                .scroll(false)
                .timeout(2000)
                .await,
            "element is outside of the viewport",
        );
        assert_eq!(eval(&t.page, "window._hovered").await, Value::Null);
        assert_eq!(eval(&t.page, "() => window.scrollY").await, 0);
    });
}

#[test]
fn should_scroll_instantly_on_retry_when_scroll_behavior_is_smooth() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<style>
                  html { scroll-behavior: smooth; }
                  body { margin: 0; }
                  .spacer { height: 2000px; }
                  #header { position: fixed; top: 0; left: 0; right: 0; bottom: 50px; background: rgba(0, 0, 0, 0.1); }
                  button { height: 30px; }
                </style>
                <div id="header"></div>
                <div class="spacer"></div>
                <button onclick="window.clicked = true">Target</button>
                <div class="spacer"></div>
                <script>
                  window.scrolls = [];
                  addEventListener('scroll', () => window.scrolls.push(window.scrollY));
                </script>"#,
            )
            .await
            .unwrap();
        t.page.click("button").await.unwrap();
        assert_eq!(eval(&t.page, "window.clicked").await, true);
        let scrolls = eval(&t.page, "() => window['scrolls']").await;
        assert!(scrolls.as_array().unwrap().len() <= 2, "{scrolls}");
    });
}
