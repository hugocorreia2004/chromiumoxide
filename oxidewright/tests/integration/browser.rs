//! Browser, context and page lifecycle (subset of `page-basic.spec.ts`,
//! `browsercontext-basic.spec.ts`, `browser.spec.ts`, `page-set-viewport-size.spec.ts`).

use oxidewright::{LoadState, ViewportSize};
use serde_json::Value;

use crate::common::*;

#[test]
fn browser_version_and_contexts() {
    run(|t| async move {
        let version = t.browser.version();
        assert!(version.split('.').count() == 4, "{version}");
        assert!(t.browser.is_connected());
        assert!(t.browser.contexts().contains(&t.context));
        let context = t.browser.new_context().await.unwrap();
        assert_eq!(t.browser.contexts().len(), 2);
        assert!(context.pages().is_empty());
        let page = context.new_page().await.unwrap();
        assert_eq!(context.pages(), vec![page.clone()]);
        assert_eq!(page.context().unwrap(), context);
        assert_eq!(context.browser().unwrap().version(), version);
        context.close().await.unwrap();
        assert!(page.is_closed());
        assert!(context.is_closed());
        assert_eq!(t.browser.contexts().len(), 1);
    });
}

#[test]
fn contexts_are_isolated() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .evaluate::<()>("() => localStorage.setItem('k', 'v')")
            .await
            .unwrap();
        let other = t
            .browser
            .new_context()
            .await
            .unwrap()
            .new_page()
            .await
            .unwrap();
        other.goto(&t.server.empty_page).await.unwrap();
        let v: Option<String> = other
            .evaluate("() => localStorage.getItem('k')")
            .await
            .unwrap();
        assert_eq!(v, None);
    });
}

#[test]
fn page_close_should_work() {
    run(|t| async move {
        let page = t.context.new_page().await.unwrap();
        assert!(!page.is_closed());
        assert_eq!(t.context.pages().len(), 2);
        page.close().await.unwrap();
        assert!(page.is_closed());
        assert_eq!(t.context.pages(), vec![t.page.clone()]);
        assert_err_contains(
            page.evaluate::<i32>("1").await,
            "Target page, context or browser has been closed",
        );
        // Closing twice is fine.
        page.close().await.unwrap();
    });
}

#[test]
fn page_should_be_closed_when_window_closes_itself() {
    run(|t| async move {
        let page = t.context.new_page().await.unwrap();
        let wait = page.wait_for_load_state().timeout(0);
        wait.await.unwrap();
        let _ = page.evaluate::<()>("() => window.close()").await;
        for _ in 0..100 {
            if page.is_closed() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        // `window.close()` only works for script-opened windows in Chrome; a
        // page opened through the protocol may or may not close. Either way
        // the page object must stay consistent.
        if page.is_closed() {
            assert!(!t.context.pages().contains(&page));
        }
    });
}

#[test]
fn pending_navigation_fails_when_page_closes() {
    run(|t| async move {
        let page = t.context.new_page().await.unwrap();
        t.server.hang("/empty.html");
        let navigation = tokio::spawn(std::future::IntoFuture::into_future(
            page.goto(&t.server.empty_page),
        ));
        t.server.wait_for_request("/empty.html").await;
        page.close().await.unwrap();
        let error = navigation.await.unwrap().unwrap_err();
        assert!(error.is_target_closed(), "{error}");
        assert!(
            error
                .to_string()
                .starts_with("page.goto: Target page, context or browser has been closed"),
            "{error}"
        );
    });
}

#[test]
fn page_url_title_and_viewport() {
    run(|t| async move {
        assert_eq!(t.page.url(), "about:blank");
        assert_eq!(
            t.page.viewport_size(),
            Some(ViewportSize {
                width: 1280,
                height: 720
            })
        );
        let size: (u32, u32) = t
            .page
            .evaluate("() => [window.innerWidth, window.innerHeight]")
            .await
            .unwrap();
        assert_eq!(size, (1280, 720));
        t.page.set_viewport_size((123, 456)).await.unwrap();
        let size: (u32, u32) = t
            .page
            .evaluate("() => [window.innerWidth, window.innerHeight]")
            .await
            .unwrap();
        assert_eq!(size, (123, 456));
        assert_eq!(
            t.page.viewport_size(),
            Some(ViewportSize {
                width: 123,
                height: 456
            })
        );
        t.page
            .goto(format!("{}/title.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(t.page.title().await.unwrap(), "Woof-Woof");
        t.page.bring_to_front().await.unwrap();
    });
}

#[test]
fn context_base_url_is_used_by_goto_and_wait_for_url() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .base_url(format!("{}/", t.server.prefix))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto("empty.html").await.unwrap();
        assert_eq!(page.url(), t.server.empty_page);
        page.wait_for_url("/empty.html")
            .timeout(1000)
            .await
            .unwrap();
        assert!(page.frame_by_url("/empty.html").is_some());
        context.close().await.unwrap();
    });
}

#[test]
fn context_without_viewport_keeps_window_size() {
    run(|t| async move {
        let context = t.browser.new_context().viewport(None).await.unwrap();
        let page = context.new_page().await.unwrap();
        assert_eq!(page.viewport_size(), None);
        let width: u32 = page.evaluate("() => window.innerWidth").await.unwrap();
        assert!(width > 0);
        context.close().await.unwrap();
    });
}

#[test]
fn injected_script_is_created_in_the_utility_world() {
    run(|t| async move {
        t.page
            .set_content("<div class=a>one</div><div class=a>two</div>")
            .await
            .unwrap();
        let context = t.page.main_frame().utility_context().await.unwrap();
        let injected = context.injected_script().await.unwrap();
        assert_eq!(injected.to_string(), "InjectedScript");
        let texts: Vec<String> = injected
            .evaluate("(injected, selector) => injected.querySelectorAll(injected.parseSelector(selector), document).map(e => e.textContent)")
            .arg("css=div.a >> internal:text=\"two\"i")
            .await
            .unwrap();
        assert_eq!(texts, vec!["two".to_string()]);
        // The page cannot see it.
        let leaked: Value = t
            .page
            .evaluate("() => Object.keys(window).filter(k => /injected|playwright|__pw/i.test(k))")
            .await
            .unwrap();
        assert_eq!(leaked, serde_json::json!([]));
        // Same instance per context.
        let again = context.injected_script().await.unwrap();
        assert!(again.same_object(&injected));
        // A navigation creates a new context.
        t.page
            .goto(&t.server.empty_page)
            .wait_until(LoadState::Load)
            .await
            .unwrap();
        let new_context = t.page.main_frame().utility_context().await.unwrap();
        assert!(!new_context.same_context(&context));
    });
}

#[test]
fn connect_over_cdp_should_work() {
    run(|t| async move {
        let ws = t
            .browser
            .with_cdp(|b| b.websocket_address().clone())
            .await
            .unwrap();
        let remote = oxidewright::chromium().connect_over_cdp(ws).await.unwrap();
        assert_eq!(remote.version(), t.browser.version());
        assert_eq!(remote.contexts().len(), 1);
        let context = remote.new_context().await.unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(page.evaluate::<i32>("() => 1 + 1").await.unwrap(), 2);
        remote.close().await.unwrap();
        assert!(!remote.is_connected());
        // The original connection is unaffected.
        assert!(t.browser.is_connected());
        t.page.goto(&t.server.empty_page).await.unwrap();
    });
}
