//! Port of `page-focus.spec.ts`.
//!
//! Skipped:
//! - FO-006 `should traverse only form elements`: WebKit on macOS only.
//! - FO-008 / FO-009 `tab should cycle between ... and browser`: Playwright skips them
//!   on Chromium unless it runs the headless shell (the tests use new headless Chrome).
//!
//! FO-002/003/004 record the events in the page instead of using `exposeFunction`.

use crate::common::*;

async fn active_id(t: &TestContext) -> String {
    t.page
        .evaluate("() => document.activeElement.id")
        .await
        .unwrap()
}

#[test]
fn should_work() {
    run(|t| async move {
        t.page
            .set_content("<div id=d1 tabIndex=0></div>")
            .await
            .unwrap();
        let name: String = t
            .page
            .evaluate("() => document.activeElement.nodeName")
            .await
            .unwrap();
        assert_eq!(name, "BODY");
        t.page.focus("#d1").await.unwrap();
        assert_eq!(active_id(&t).await, "d1");
    });
}

#[test]
fn should_emit_focus_event() {
    run(|t| async move {
        t.page
            .set_content("<div id=d1 tabIndex=0></div>")
            .await
            .unwrap();
        t.page
            .eval_on_selector::<()>(
                "#d1",
                "d1 => d1.addEventListener('focus', () => window.focused = true)",
            )
            .await
            .unwrap();
        t.page.focus("#d1").await.unwrap();
        let focused: bool = t.page.evaluate("() => !!window.focused").await.unwrap();
        assert!(focused);
    });
}

#[test]
fn should_emit_blur_event() {
    run(|t| async move {
        t.page
            .set_content("<div id=d1 tabIndex=0>DIV1</div><div id=d2 tabIndex=0>DIV2</div>")
            .await
            .unwrap();
        t.page.focus("#d1").await.unwrap();
        t.page
            .eval_on_selector::<()>(
                "#d1",
                "d1 => d1.addEventListener('blur', () => window.blurred = true)",
            )
            .await
            .unwrap();
        t.page
            .eval_on_selector::<()>(
                "#d2",
                "d2 => d2.addEventListener('focus', () => window.focused = true)",
            )
            .await
            .unwrap();
        t.page.focus("#d2").await.unwrap();
        let (focused, blurred): (bool, bool) = t
            .page
            .evaluate("() => [!!window.focused, !!window.blurred]")
            .await
            .unwrap();
        assert!(focused);
        assert!(blurred);
    });
}

#[test]
fn should_traverse_focus() {
    run(|t| async move {
        t.page
            .set_content("<input id=\"i1\"><input id=\"i2\">")
            .await
            .unwrap();
        t.page
            .eval_on_selector::<()>(
                "#i2",
                "i2 => i2.addEventListener('focus', () => window.focused = true)",
            )
            .await
            .unwrap();
        t.page.focus("#i1").await.unwrap();
        t.page.keyboard().r#type("First").await.unwrap();
        t.page.keyboard().press("Tab").await.unwrap();
        t.page.keyboard().r#type("Last").await.unwrap();
        let focused: bool = t.page.evaluate("() => !!window.focused").await.unwrap();
        assert!(focused);
        let i1: String = t
            .page
            .eval_on_selector("#i1", "e => e.value")
            .await
            .unwrap();
        let i2: String = t
            .page
            .eval_on_selector("#i2", "e => e.value")
            .await
            .unwrap();
        assert_eq!(i1, "First");
        assert_eq!(i2, "Last");
    });
}

#[test]
fn should_traverse_focus_in_all_directions() {
    run(|t| async move {
        t.page
            .set_content("<input value=\"1\"><input value=\"2\"><input value=\"3\">")
            .await
            .unwrap();
        for (key, expected) in [
            ("Tab", "1"),
            ("Tab", "2"),
            ("Tab", "3"),
            ("Shift+Tab", "2"),
            ("Shift+Tab", "1"),
        ] {
            t.page.keyboard().press(key).await.unwrap();
            let value: String = t
                .page
                .evaluate("() => document.activeElement.value")
                .await
                .unwrap();
            assert_eq!(value, expected);
        }
    });
}

#[test]
fn clicking_checkbox_should_activate_it() {
    run(|t| async move {
        t.page
            .set_content("<input type=checkbox></input>")
            .await
            .unwrap();
        t.page.click("input").await.unwrap();
        let name: String = t
            .page
            .evaluate("() => document.activeElement.nodeName")
            .await
            .unwrap();
        assert_eq!(name, "INPUT");
    });
}

#[test]
fn keeps_focus_on_element_when_attempting_to_focus_a_non_focusable_element() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<div id="focusable" tabindex="0">focusable</div>
      <div id="non-focusable">not focusable</div>
      <script>
        window.eventLog = [];
        const focusable = document.getElementById("focusable");
        focusable.addEventListener('blur', () => window.eventLog.push('blur focusable'));
        focusable.addEventListener('focus', () => window.eventLog.push('focus focusable'));
        const nonFocusable = document.getElementById("non-focusable");
        nonFocusable.addEventListener('blur', () => window.eventLog.push('blur non-focusable'));
        nonFocusable.addEventListener('focus', () => window.eventLog.push('focus non-focusable'));
      </script>"#,
            )
            .await
            .unwrap();
        t.page.locator("#focusable").click().await.unwrap();
        assert_eq!(active_id(&t).await, "focusable");
        t.page.locator("#non-focusable").focus().await.unwrap();
        assert_eq!(active_id(&t).await, "focusable");
        let log: Vec<String> = t.page.evaluate("() => window['eventLog']").await.unwrap();
        assert_eq!(log, ["focus focusable"]);
    });
}

const IS_ACTIVE: &str = "e => e === document.activeElement";

async fn frame_page(t: &TestContext) {
    t.server
        .set_content("/frame.html", "text/html", "<input id=inner>");
    t.page.goto(&t.server.empty_page).await.unwrap();
    t.page
        .set_content(format!(
            "<iframe src=\"{}/frame.html\"></iframe><input id=outer>",
            t.server.prefix
        ))
        .await
        .unwrap();
}

#[test]
fn should_move_focus_from_an_iframe_to_its_parent() {
    run(|t| async move {
        frame_page(&t).await;
        let inner = t.page.frame_locator("iframe").locator("#inner");
        inner.focus().await.unwrap();
        assert!(inner.evaluate::<bool>(IS_ACTIVE).await.unwrap());
        t.page
            .evaluate::<()>("() => document.getElementById('outer').focus()")
            .await
            .unwrap();
        assert!(
            t.page
                .locator("#outer")
                .evaluate::<bool>(IS_ACTIVE)
                .await
                .unwrap()
        );
        assert!(!inner.evaluate::<bool>(IS_ACTIVE).await.unwrap());
    });
}

#[test]
fn should_tab_from_an_iframe_to_its_parent() {
    run(|t| async move {
        frame_page(&t).await;
        let inner = t.page.frame_locator("iframe").locator("#inner");
        inner.click().await.unwrap();
        assert!(inner.evaluate::<bool>(IS_ACTIVE).await.unwrap());
        t.page.keyboard().press("Tab").await.unwrap();
        assert!(
            t.page
                .locator("#outer")
                .evaluate::<bool>(IS_ACTIVE)
                .await
                .unwrap()
        );
    });
}

#[test]
fn should_move_focus_from_the_parent_into_its_iframe() {
    run(|t| async move {
        frame_page(&t).await;
        t.page.locator("#outer").focus().await.unwrap();
        assert!(
            t.page
                .locator("#outer")
                .evaluate::<bool>(IS_ACTIVE)
                .await
                .unwrap()
        );
        t.page
            .evaluate::<()>(
                "() => document.querySelector('iframe').contentDocument.querySelector('input').focus()",
            )
            .await
            .unwrap();
        assert!(
            t.page
                .locator("iframe")
                .evaluate::<bool>(IS_ACTIVE)
                .await
                .unwrap()
        );
        // `toBeFocused` on the inner input also requires the document to have focus.
        let inner = t.page.frame_locator("iframe").locator("#inner");
        assert!(inner.evaluate::<bool>(IS_ACTIVE).await.unwrap());
    });
}
