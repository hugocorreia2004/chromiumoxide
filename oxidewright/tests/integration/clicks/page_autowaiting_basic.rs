//! Port of `page-autowaiting-basic.spec.ts`.
//!
//! Adapted: the specs order `click` after the `framenavigated` event; the
//! Rust port checks that when `click` resolves the navigation has already
//! committed (the request was served and `page.url()` changed).
//!
//! Skipped:
//! - AW-010 (needs the `__testHookAfterPointerAction` test hook).
//! - AW-012 (needs `expect(...).toBeVisible`).

use oxidewright::LoadState;

use crate::common::*;

fn init_server(t: &TestContext) {
    t.server.set_content(
        "/empty.html",
        "text/html",
        "<link rel='stylesheet' href='./one-style.css'>",
    );
}

async fn assert_navigated(t: &TestContext, path: &str, url: &str) {
    assert!(
        t.server.requested().iter().any(|p| p == path),
        "{path} not requested"
    );
    assert_eq!(t.page.url(), url);
}

#[test]
fn should_await_navigation_when_clicking_anchor() {
    run(|t| async move {
        init_server(&t);
        t.page
            .set_content(format!(
                "<a id=\"anchor\" href=\"{}\">empty.html</a>",
                t.server.empty_page
            ))
            .await
            .unwrap();
        t.page.click("a").await.unwrap();
        assert_navigated(&t, "/empty.html", &t.server.empty_page).await;
    });
}

#[test]
fn should_not_stall_on_js_navigation_link() {
    run(|t| async move {
        t.page
            .set_content("<a href=\"javascript:console.log(1)\">console.log</a>")
            .await
            .unwrap();
        t.page.click("a").await.unwrap();
    });
}

#[test]
fn should_await_cross_process_navigation_when_clicking_anchor() {
    run(|t| async move {
        init_server(&t);
        let url = format!("{}/empty.html", t.server.cross_process_prefix);
        t.page
            .set_content(format!("<a href=\"{url}\">empty.html</a>"))
            .await
            .unwrap();
        t.page.click("a").await.unwrap();
        assert_navigated(&t, "/empty.html", &url).await;
    });
}

#[test]
fn should_await_form_get_on_click() {
    run(|t| async move {
        init_server(&t);
        t.page
            .set_content(format!(
                r#"<form action="{}" method="get">
                  <input name="foo" value="bar">
                  <input type="submit" value="Submit">
                </form>"#,
                t.server.empty_page
            ))
            .await
            .unwrap();
        t.page.click("input[type=submit]").await.unwrap();
        assert_navigated(
            &t,
            "/empty.html",
            &format!("{}?foo=bar", t.server.empty_page),
        )
        .await;
    });
}

#[test]
fn should_await_form_post_on_click() {
    run(|t| async move {
        init_server(&t);
        t.page
            .set_content(format!(
                r#"<form action="{}" method="post">
                  <input name="foo" value="bar">
                  <input type="submit" value="Submit">
                </form>"#,
                t.server.empty_page
            ))
            .await
            .unwrap();
        t.page.click("input[type=submit]").await.unwrap();
        assert_navigated(&t, "/empty.html", &t.server.empty_page).await;
    });
}

#[test]
fn should_work_with_no_wait_after_true() {
    run(|t| async move {
        t.server.hang("/empty.html");
        t.page
            .set_content(format!(
                "<a id=\"anchor\" href=\"{}\">empty.html</a>",
                t.server.empty_page
            ))
            .await
            .unwrap();
        t.page.click("a").no_wait_after(true).await.unwrap();
    });
}

#[test]
fn should_work_with_dblclick_without_no_wait_after_when_navigation_is_stalled() {
    run(|t| async move {
        t.server.hang("/empty.html");
        t.page
            .set_content(format!(
                "<a id=\"anchor\" href=\"{}\">empty.html</a>",
                t.server.empty_page
            ))
            .await
            .unwrap();
        t.page.dblclick("a").await.unwrap();
    });
}

#[test]
fn should_work_with_wait_for_load_state_load() {
    run(|t| async move {
        init_server(&t);
        t.page
            .set_content(format!(
                "<a id=\"anchor\" href=\"{}\">empty.html</a>",
                t.server.empty_page
            ))
            .await
            .unwrap();
        t.page.click("a").await.unwrap();
        t.page
            .wait_for_load_state()
            .state(LoadState::Load)
            .await
            .unwrap();
        assert_navigated(&t, "/empty.html", &t.server.empty_page).await;
        assert!(t.page.main_frame().has_load_state(LoadState::Load));
    });
}

#[test]
fn should_work_with_goto_following_click() {
    run(|t| async move {
        t.server
            .set_content("/login.html", "text/html", "You are logged in");
        t.page
            .set_content(format!(
                r#"<form action="{}/login.html" method="get">
                  <input type="text">
                  <input type="submit" value="Submit">
                </form>"#,
                t.server.prefix
            ))
            .await
            .unwrap();
        t.page.fill("input[type=text]", "admin").await.unwrap();
        t.page.click("input[type=submit]").await.unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
    });
}

#[test]
fn should_report_and_collapse_log_in_action() {
    run(|t| async move {
        t.page
            .set_content(
                "<input id='checkbox' type='checkbox' style=\"visibility: hidden\"></input>",
            )
            .await
            .unwrap();
        let error = t
            .page
            .locator("input")
            .click()
            .timeout(5000)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("Call log:"), "{error}");
        assert!(
            regex::Regex::new(r"\d+ × waiting for")
                .unwrap()
                .is_match(&error),
            "{error}"
        );
        let lines = error[error.find("Call log:").unwrap()..].lines().count();
        assert!(lines < 30, "{error}");
    });
}
