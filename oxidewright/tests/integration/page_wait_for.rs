//! Ports of `page-wait-for-load-state.spec.ts`, `page-wait-for-url.spec.ts`,
//! `page-wait-for-navigation.spec.ts` (subset) and `page-history.spec.ts`
//! (subset).
//!
//! Skipped: popup tests (WA-082..090, need the `popup` event), WA-091
//! (`page.route`), tests driven by `page.click` (replaced by DOM `click()`
//! where the click itself is not what is tested), NA-073 (file URLs history),
//! NA-077/083/084 (`request` event), bfcache test NA-074.

use std::future::IntoFuture;
use std::time::Duration;

use oxidewright::LoadState;
use regex::Regex;

use crate::common::*;

// ---- page-wait-for-load-state.spec.ts ----

#[test]
fn should_pick_up_ongoing_navigation() {
    run(|t| async move {
        let gate = t
            .server
            .stall("/one-style.css", "text/css", "Not found", 404);
        t.page
            .goto(format!("{}/one-style.html", t.server.prefix))
            .wait_until(LoadState::DomContentLoaded)
            .await
            .unwrap();
        let wait = tokio::spawn(t.page.wait_for_load_state().into_future());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!wait.is_finished());
        gate.release();
        wait.await.unwrap().unwrap();
    });
}

#[test]
fn should_respect_timeout() {
    run(|t| async move {
        t.server.hang("/one-style.css");
        t.page
            .goto(format!("{}/one-style.html", t.server.prefix))
            .wait_until(LoadState::DomContentLoaded)
            .await
            .unwrap();
        let r = t
            .page
            .wait_for_load_state()
            .state(LoadState::Load)
            .timeout(1)
            .await;
        let e = assert_err_contains(r, "page.waitForLoadState: Timeout 1ms exceeded.");
        assert!(e.is_timeout());
    });
}

#[test]
fn should_resolve_immediately_if_loaded() {
    run(|t| async move {
        t.page
            .goto(format!("{}/one-style.html", t.server.prefix))
            .await
            .unwrap();
        t.page.wait_for_load_state().timeout(100).await.unwrap();
    });
}

#[test]
fn should_throw_for_bad_state() {
    let err = "bad".parse::<LoadState>().unwrap_err();
    assert!(
        err.to_string()
            .contains("state: expected one of (load|domcontentloaded|networkidle|commit)")
    );
}

#[test]
fn should_resolve_immediately_if_load_state_matches() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.server.hang("/one-style.css");
        t.page
            .goto(format!("{}/one-style.html", t.server.prefix))
            .wait_until(LoadState::DomContentLoaded)
            .await
            .unwrap();
        t.page
            .wait_for_load_state()
            .state(LoadState::DomContentLoaded)
            .timeout(100)
            .await
            .unwrap();
    });
}

#[test]
fn should_wait_for_load_state_of_new_page() {
    run(|t| async move {
        let page = t.context.new_page().await.unwrap();
        page.wait_for_load_state().timeout(5000).await.unwrap();
        let state: String = page.evaluate("() => document.readyState").await.unwrap();
        assert_eq!(state, "complete");
    });
}

#[test]
fn should_work_for_frame() {
    run(|t| async move {
        t.page
            .goto(format!("{}/frames/one-frame.html", t.server.prefix))
            .await
            .unwrap();
        let frame = t.page.frames()[1].clone();
        let gate = t.server.stall("/one-style.css", "text/css", "", 200);
        frame
            .goto(format!("{}/one-style.html", t.server.prefix))
            .wait_until(LoadState::DomContentLoaded)
            .await
            .unwrap();
        let wait = tokio::spawn(frame.wait_for_load_state().into_future());
        t.page.evaluate::<i32>("1").await.unwrap();
        assert!(!wait.is_finished());
        gate.release();
        wait.await.unwrap().unwrap();
    });
}

#[test]
fn should_work_with_javascript_data_and_blob_iframes() {
    run(|t| async move {
        for src in ["javascript:false", "data:text/html", "blob:"] {
            t.page.goto(&t.server.empty_page).await.unwrap();
            t.page
                .set_content(format!("<iframe src=\"{src}\"></iframe>"))
                .wait_until(LoadState::Commit)
                .await
                .unwrap();
            for state in [
                LoadState::DomContentLoaded,
                LoadState::Load,
                LoadState::NetworkIdle,
            ] {
                t.page
                    .wait_for_load_state()
                    .state(state)
                    .timeout(10_000)
                    .await
                    .unwrap();
            }
        }
    });
}

// ---- page-wait-for-url.spec.ts ----

#[test]
fn wait_for_url_should_work() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .evaluate::<()>("url => { window.location.href = url; }")
            .arg(format!("{}/grid.html", t.server.prefix))
            .await
            .unwrap();
        t.page.wait_for_url("**/grid.html").await.unwrap();
    });
}

#[test]
fn wait_for_url_should_respect_timeout() {
    run(|t| async move {
        let wait = tokio::spawn(
            t.page
                .wait_for_url("**/frame.html")
                .timeout(2500)
                .into_future(),
        );
        t.page.goto(&t.server.empty_page).await.unwrap();
        let e = assert_err_contains(
            wait.await.unwrap(),
            "page.waitForURL: Timeout 2500ms exceeded.",
        );
        assert!(
            e.to_string()
                .contains("waiting for navigation to \"**/frame.html\" until \"load\""),
            "{e}"
        );
    });
}

#[test]
fn wait_for_url_should_work_with_both_domcontentloaded_and_load() {
    run(|t| async move {
        let gate = t.server.stall("/one-style.css", "text/css", "", 200);
        let navigation = tokio::spawn(
            t.page
                .goto(format!("{}/one-style.html", t.server.prefix))
                .into_future(),
        );
        let dcl = tokio::spawn(
            t.page
                .wait_for_url("**/one-style.html")
                .wait_until(LoadState::DomContentLoaded)
                .into_future(),
        );
        let load = tokio::spawn(
            t.page
                .wait_for_url("**/one-style.html")
                .wait_until(LoadState::Load)
                .into_future(),
        );
        t.server.wait_for_request("/one-style.css").await;
        dcl.await.unwrap().unwrap();
        assert!(!load.is_finished());
        gate.release();
        load.await.unwrap().unwrap();
        navigation.await.unwrap().unwrap();
    });
}

#[test]
fn wait_for_url_should_work_with_commit() {
    run(|t| async move {
        t.server.hang("/script.js");
        t.server.set_content(
            "/empty.html",
            "text/html",
            "<title>Hello</title><script src=\"script.js\"></script>",
        );
        let page = t.page.clone();
        let url = t.server.empty_page.clone();
        tokio::spawn(async move {
            let _ = page.goto(url).await;
        });
        t.page
            .wait_for_url("**/empty.html")
            .wait_until(LoadState::Commit)
            .await
            .unwrap();
        assert_eq!(t.page.title().await.unwrap(), "Hello");
    });
}

#[test]
fn wait_for_url_should_work_with_commit_and_about_blank() {
    run(|t| async move {
        t.page
            .wait_for_url("about:blank")
            .wait_until(LoadState::Commit)
            .await
            .unwrap();
    });
}

#[test]
fn wait_for_url_should_work_with_clicking_on_anchor_links() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .set_content("<a href='#foobar'>foobar</a>")
            .await
            .unwrap();
        t.page
            .evaluate::<()>("() => document.querySelector('a').click()")
            .await
            .unwrap();
        t.page.wait_for_url("**/*#foobar").await.unwrap();
    });
}

#[test]
fn wait_for_url_should_work_with_history_push_and_replace_state() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .set_content("<a onclick='javascript:pushState()'>SPA</a><script>function pushState() { history.pushState({}, '', 'wow.html') }</script>")
            .await
            .unwrap();
        t.page
            .evaluate::<()>("() => document.querySelector('a').click()")
            .await
            .unwrap();
        t.page.wait_for_url("**/wow.html").await.unwrap();
        assert_eq!(t.page.url(), format!("{}/wow.html", t.server.prefix));
        t.page
            .evaluate::<()>("() => history.replaceState({}, '', '/replaced.html')")
            .await
            .unwrap();
        t.page.wait_for_url("**/replaced.html").await.unwrap();
        assert_eq!(t.page.url(), format!("{}/replaced.html", t.server.prefix));
    });
}

#[test]
fn wait_for_url_should_work_with_dom_history_back_forward() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .set_content("<script>history.pushState({}, '', '/first.html'); history.pushState({}, '', '/second.html');</script>")
            .await
            .unwrap();
        assert_eq!(t.page.url(), format!("{}/second.html", t.server.prefix));
        t.page.evaluate::<()>("() => history.back()").await.unwrap();
        t.page.wait_for_url("**/first.html").await.unwrap();
        assert_eq!(t.page.url(), format!("{}/first.html", t.server.prefix));
        t.page
            .evaluate::<()>("() => history.forward()")
            .await
            .unwrap();
        t.page.wait_for_url("**/second.html").await.unwrap();
        assert_eq!(t.page.url(), format!("{}/second.html", t.server.prefix));
    });
}

#[test]
fn wait_for_url_should_work_with_url_match_for_same_document_navigations() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let wait = tokio::spawn(
            t.page
                .wait_for_url(Regex::new(r"third\.html").unwrap())
                .into_future(),
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        for path in ["/first.html", "/second.html"] {
            t.page
                .evaluate::<()>("p => history.pushState({}, '', p)")
                .arg(path)
                .await
                .unwrap();
            assert!(!wait.is_finished());
        }
        t.page
            .evaluate::<()>("() => history.pushState({}, '', '/third.html')")
            .await
            .unwrap();
        wait.await.unwrap().unwrap();
    });
}

#[test]
fn wait_for_url_should_work_on_frame() {
    run(|t| async move {
        t.page
            .goto(format!("{}/frames/one-frame.html", t.server.prefix))
            .await
            .unwrap();
        let frame = t.page.frames()[1].clone();
        frame
            .evaluate::<()>("url => { window.location.href = url; }")
            .arg(format!("{}/grid.html", t.server.prefix))
            .await
            .unwrap();
        frame.wait_for_url("**/grid.html").await.unwrap();
    });
}

#[test]
fn wait_for_url_with_predicate() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let wait = tokio::spawn(
            t.page
                .wait_for_url(oxidewright::UrlMatch::predicate(|u| {
                    u.path() == "/grid.html"
                }))
                .into_future(),
        );
        t.page
            .goto(format!("{}/grid.html", t.server.prefix))
            .await
            .unwrap();
        wait.await.unwrap().unwrap();
    });
}

// ---- page-wait-for-navigation.spec.ts (subset) ----

#[test]
fn wait_for_navigation_should_work() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let wait = t.page.wait_for_navigation();
        t.page
            .evaluate::<()>("url => { window.location.href = url; }")
            .arg(format!("{}/grid.html", t.server.prefix))
            .await
            .unwrap();
        let response = wait.await.unwrap().unwrap();
        assert!(response.ok());
        assert!(response.url().contains("grid.html"));
    });
}

#[test]
fn wait_for_navigation_should_work_with_url_option() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let wait = tokio::spawn(
            t.page
                .wait_for_navigation()
                .url("**/frame.html")
                .into_future(),
        );
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert!(!wait.is_finished());
        t.page
            .goto(format!("{}/frames/frame.html", t.server.prefix))
            .await
            .unwrap();
        let response = wait.await.unwrap().unwrap().unwrap();
        assert!(response.url().ends_with("/frames/frame.html"));
    });
}

#[test]
fn wait_for_navigation_should_fail_when_frame_detaches() {
    run(|t| async move {
        t.page
            .goto(format!("{}/frames/one-frame.html", t.server.prefix))
            .await
            .unwrap();
        let frame = t.page.frames()[1].clone();
        let wait = tokio::spawn(frame.wait_for_navigation().into_future());
        t.page
            .evaluate::<()>("() => document.querySelector('iframe').remove()")
            .await
            .unwrap();
        assert_err_contains(
            wait.await.unwrap(),
            "frame.waitForNavigation: Navigating frame was detached!",
        );
    });
}

#[test]
fn wait_for_navigation_should_work_with_history_api() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let wait = t.page.wait_for_navigation();
        t.page
            .evaluate::<()>("() => history.pushState({}, '', 'wow.html')")
            .await
            .unwrap();
        let response = wait.await.unwrap();
        assert!(response.is_none());
        assert_eq!(t.page.url(), format!("{}/wow.html", t.server.prefix));
    });
}

// ---- page-history.spec.ts (subset) ----

#[test]
fn go_back_should_work() {
    run(|t| async move {
        assert!(t.page.go_back().await.unwrap().is_none());
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .goto(format!("{}/grid.html", t.server.prefix))
            .await
            .unwrap();
        let response = t.page.go_back().await.unwrap().unwrap();
        assert!(response.ok());
        assert!(response.url().contains(&t.server.empty_page));
        let response = t.page.go_forward().await.unwrap().unwrap();
        assert!(response.ok());
        assert!(response.url().contains("/grid.html"));
        assert!(t.page.go_forward().await.unwrap().is_none());
    });
}

#[test]
fn go_back_should_work_with_history_api() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .evaluate::<()>("() => { history.pushState({}, '', '/first.html'); history.pushState({}, '', '/second.html'); }")
            .await
            .unwrap();
        assert_eq!(t.page.url(), format!("{}/second.html", t.server.prefix));
        t.page.go_back().await.unwrap();
        assert_eq!(t.page.url(), format!("{}/first.html", t.server.prefix));
        t.page.go_back().await.unwrap();
        assert_eq!(t.page.url(), t.server.empty_page);
        t.page.go_forward().await.unwrap();
        assert_eq!(t.page.url(), format!("{}/first.html", t.server.prefix));
    });
}

#[test]
fn reload_should_work() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .evaluate::<()>("() => window['_foo'] = 10")
            .await
            .unwrap();
        t.page.reload().await.unwrap();
        let v: Option<i32> = t.page.evaluate("() => window['_foo']").await.unwrap();
        assert_eq!(v, None);
    });
}

#[test]
fn reload_should_work_with_data_url() {
    run(|t| async move {
        t.page.goto("data:text/html,hello").await.unwrap();
        assert!(t.page.content().await.unwrap().contains("hello"));
        assert!(t.page.reload().await.unwrap().is_none());
        assert!(t.page.content().await.unwrap().contains("hello"));
    });
}

#[test]
fn reload_should_not_resolve_with_same_document_navigation() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page.evaluate::<i32>("1").await.unwrap();
        let gate = t.server.stall("/empty.html", "text/html", "hello", 200);
        let _ = t
            .page
            .evaluate::<()>("() => { window.history.pushState({}, ''); }")
            .await;
        let reload = tokio::spawn(t.page.reload().into_future());
        tokio::time::sleep(Duration::from_millis(50)).await;
        // Like the original test, this evaluation is not awaited: it stalls
        // while the reload request is pending.
        tokio::spawn(
            t.page
                .evaluate::<()>("() => { window.history.pushState({}, ''); }")
                .into_future(),
        );
        t.server.wait_for_request("/empty.html").await;
        if reload.is_finished() {
            panic!("reload finished early: {:?}", reload.await.unwrap());
        }
        gate.release();
        let response = reload.await.unwrap().unwrap().unwrap();
        assert_eq!(response.text().await.unwrap(), "hello");
    });
}

#[test]
fn reload_should_work_with_redirects_and_hashes() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.server
            .set_redirect("/empty.html", &format!("{}/title.html", t.server.prefix));
        t.page.reload().await.unwrap();
        assert_eq!(t.page.url(), format!("{}/title.html", t.server.prefix));
        t.server.set_redirect(
            "/title.html",
            &format!("{}/grid.html", t.server.cross_process_prefix),
        );
        t.page.reload().await.unwrap();
        assert_eq!(
            t.page.url(),
            format!("{}/grid.html", t.server.cross_process_prefix)
        );
        for suffix in ["#hash", "#"] {
            let url = format!("{}/one-style.html{suffix}", t.server.prefix);
            t.page.goto(&url).await.unwrap();
            t.page.reload().await.unwrap();
            assert_eq!(t.page.url(), url);
        }
    });
}
