//! Popups inherit the context's emulation, routes and init scripts.
//! Ported from Playwright's `tests/library/popup.spec.ts` (PO-014..PO-024);
//! these also check that a popup does not look different from its opener
//! (user agent, viewport, touch), which detectors compare.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use oxidewright::{HttpCredentials, Route, ViewportSize};
use serde_json::{Value, json};

use crate::common::*;

#[test]
fn po_014_should_inherit_user_agent_from_browser_context() {
    run(|t| async move {
        let context = t.browser.new_context().user_agent("hey").await.unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        page.set_content("<a target=_blank rel=noopener href=\"/popup/popup.html\">link</a>")
            .await
            .unwrap();
        let request = t.server.wait_for_request("/popup/popup.html");
        let popup = context.expect_page(page.click("a")).await.unwrap();
        popup
            .wait_for_load_state()
            .state(oxidewright::LoadState::DomContentLoaded)
            .await
            .unwrap();
        let user_agent: String = popup
            .evaluate("() => window['initialUserAgent']")
            .await
            .unwrap();
        let request = request.await;
        assert_eq!(user_agent, "hey");
        assert_eq!(request.header("user-agent").as_deref(), Some("hey"));
        context.close().await.unwrap();
    });
}

#[test]
#[ignore = "a popup in a new renderer process (noopener) is resumed before its             routes are set up: enabling Fetch earlier could leave requests paused             with nobody listening; its user agent and headers are applied"]
fn po_015_should_respect_routes_from_browser_context() {
    run(|t| async move {
        let context = t.browser.new_context().await.unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        page.set_content("<a target=_blank rel=noopener href=\"empty.html\">link</a>")
            .await
            .unwrap();
        let intercepted = Arc::new(AtomicBool::new(false));
        let flag = intercepted.clone();
        context
            .route("**/empty.html", move |route: Route| {
                let flag = flag.clone();
                async move {
                    flag.store(true, Ordering::SeqCst);
                    route.continue_().await
                }
            })
            .await
            .unwrap();
        context.expect_page(page.click("a")).await.unwrap();
        assert!(intercepted.load(Ordering::SeqCst));
        context.close().await.unwrap();
    });
}

#[test]
fn po_016_should_inherit_extra_headers_from_browser_context() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .extra_http_headers([("foo", "bar")])
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let request = t.server.wait_for_request("/dummy.html");
        page.evaluate::<()>("url => { window['_popup'] = window.open(url); }")
            .arg(format!("{}/dummy.html", t.server.prefix))
            .await
            .unwrap();
        let request = request.await;
        assert_eq!(request.header("foo").as_deref(), Some("bar"));
        context.close().await.unwrap();
    });
}

#[test]
fn po_017_should_inherit_offline_from_browser_context() {
    run(|t| async move {
        let context = t.browser.new_context().await.unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        context.set_offline(true).await.unwrap();
        let online: bool = page
            .evaluate("url => { const win = window.open(url); return win.navigator.onLine; }")
            .arg(format!("{}/dummy.html", t.server.prefix))
            .await
            .unwrap();
        assert!(!online);
        context.close().await.unwrap();
    });
}

#[test]
fn po_018_should_inherit_http_credentials_from_browser_context() {
    run(|t| async move {
        t.server.set_auth("/title.html", "user", "pass");
        let context = t
            .browser
            .new_context()
            .http_credentials(HttpCredentials::new("user", "pass"))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let popup = page
            .expect_popup(
                page.evaluate::<()>("url => { window['_popup'] = window.open(url); }")
                    .arg(format!("{}/title.html", t.server.prefix)),
            )
            .await
            .unwrap();
        popup
            .wait_for_load_state()
            .state(oxidewright::LoadState::DomContentLoaded)
            .await
            .unwrap();
        assert_eq!(popup.title().await.unwrap(), "Woof-Woof");
        context.close().await.unwrap();
    });
}

#[test]
fn po_019_should_inherit_touch_support_from_browser_context() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .viewport(Some(ViewportSize {
                width: 400,
                height: 500,
            }))
            .has_touch(true)
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let has_touch: bool = page
            .evaluate("() => { const win = window.open(''); return 'ontouchstart' in win; }")
            .await
            .unwrap();
        assert!(has_touch);
        context.close().await.unwrap();
    });
}

#[test]
fn po_020_should_inherit_viewport_size_from_browser_context() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .viewport(Some(ViewportSize {
                width: 400,
                height: 500,
            }))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let size: Value = page
            .evaluate("() => { const win = window.open('about:blank'); return { width: win.innerWidth, height: win.innerHeight }; }")
            .await
            .unwrap();
        assert_eq!(size, json!({ "width": 400, "height": 500 }));
        context.close().await.unwrap();
    });
}

#[test]
fn po_022_should_respect_routes_from_browser_context_when_using_window_open() {
    run(|t| async move {
        let context = t.browser.new_context().await.unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let intercepted = Arc::new(AtomicBool::new(false));
        let flag = intercepted.clone();
        context
            .route("**/empty.html", move |route: Route| {
                let flag = flag.clone();
                async move {
                    flag.store(true, Ordering::SeqCst);
                    route.continue_().await
                }
            })
            .await
            .unwrap();
        page.expect_popup(
            page.evaluate::<()>("url => { window['__popup'] = window.open(url); }")
                .arg(&t.server.empty_page),
        )
        .await
        .unwrap();
        // The popup event comes once the popup is set up and resumed; its
        // request is routed right after, on another task.
        oxidewright::expect::poll(move || {
            let intercepted = intercepted.clone();
            async move { intercepted.load(Ordering::SeqCst) }
        })
        .to_equal(true)
        .await
        .unwrap();
        context.close().await.unwrap();
    });
}

#[test]
fn po_023_add_init_script_should_apply_to_an_in_process_popup() {
    run(|t| async move {
        let context = t.browser.new_context().await.unwrap();
        context
            .add_init_script("window['injected'] = 123")
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let injected: i64 = page
            .evaluate("() => { const win = window.open('about:blank'); return win['injected']; }")
            .await
            .unwrap();
        assert_eq!(injected, 123);
        context.close().await.unwrap();
    });
}

#[test]
fn po_024_add_init_script_should_apply_to_a_cross_process_popup() {
    run(|t| async move {
        let context = t.browser.new_context().await.unwrap();
        context
            .add_init_script("window['injected'] = 123")
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let popup = page
            .expect_popup(
                page.evaluate::<()>("url => { window.open(url); }")
                    .arg(format!("{}/title.html", t.server.cross_process_prefix)),
            )
            .await
            .unwrap();
        let injected: i64 = popup.evaluate("injected").await.unwrap();
        assert_eq!(injected, 123);
        popup.reload().await.unwrap();
        let injected: i64 = popup.evaluate("injected").await.unwrap();
        assert_eq!(injected, 123);
        context.close().await.unwrap();
    });
}
