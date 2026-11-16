//! Port of `tests/page/page-goto.spec.ts` (and `frame-goto.spec.ts`).
//!
//! Skipped: tests relying on `page.on('request'|'response'|...)` events or
//! `page.route` (network events/routing come with the network API), on an
//! HTTPS test server (NA-024..026, NA-050), on service workers (NA-051),
//! `setExtraHTTPHeaders` (NA-054), `page.waitForEvent` (NA-065), timing
//! sensitive js-redirect races (NA-038, NA-039, NA-062) and Node.js
//! listener-leak checks (NA-044..046, which are about EventEmitters).

use std::future::IntoFuture;
use std::time::Duration;

use axum::body::Body;
use axum::response::Response;
use oxidewright::LoadState;

use crate::common::*;

#[test]
fn should_work() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(t.page.url(), t.server.empty_page);
    });
}

#[test]
fn should_work_with_file_url() {
    run(|t| async move {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/assets/frames/two-frames.html");
        let file_url = url::Url::from_file_path(&path).unwrap().to_string();
        t.page.goto(&file_url).await.unwrap();
        assert_eq!(t.page.url().to_lowercase(), file_url.to_lowercase());
        assert_eq!(t.page.frames().len(), 3);
        t.page.goto("about:blank").await.unwrap();
    });
}

#[test]
fn should_use_http_for_no_protocol() {
    run(|t| async move {
        t.page
            .goto(t.server.empty_page.trim_start_matches("http://"))
            .await
            .unwrap();
        assert_eq!(t.page.url(), t.server.empty_page);
    });
}

#[test]
fn should_work_cross_process() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let url = format!("{}/empty.html", t.server.cross_process_prefix);
        let response = t.page.goto(&url).await.unwrap().unwrap();
        assert_eq!(t.page.url(), url);
        assert_eq!(response.frame().unwrap(), t.page.main_frame());
        assert_eq!(response.url(), url);
    });
}

#[test]
fn should_work_with_cross_process_that_fails_before_committing() {
    run(|t| async move {
        t.server.set_route("/empty.html", |_| async {
            Response::builder()
                .status(500)
                .header("connection", "close")
                .body(Body::empty())
                .unwrap()
        });
        let response = t
            .page
            .goto(format!("{}/title.html", t.server.cross_process_prefix))
            .await
            .unwrap()
            .unwrap();
        response.finished().await.unwrap();
        let _ = t.page.goto(&t.server.empty_page).await;
    });
}

#[test]
fn should_work_with_cross_origin_opener_policy() {
    run(|t| async move {
        t.server.set_route("/empty.html", |_| async {
            Response::builder()
                .header("content-type", "text/html")
                .header("Cross-Origin-Opener-Policy", "same-origin")
                .body(Body::from("<div>Hello there!</div><script>window.onload = () => console.log('onload')</script>"))
                .unwrap()
        });
        let response = t.page.goto(&t.server.empty_page).await.unwrap().unwrap();
        assert_eq!(t.page.url(), t.server.empty_page);
        response.finished().await.unwrap();
        assert_eq!(response.request().failure(), None);
    });
}

#[test]
fn should_work_with_cross_origin_opener_policy_after_redirect() {
    run(|t| async move {
        t.server.set_redirect("/redirect", "/empty.html");
        t.server.set_route("/empty.html", |_| async {
            Response::builder()
                .header("content-type", "text/html")
                .header("Cross-Origin-Opener-Policy", "same-origin")
                .body(Body::from("<div>Hello there!</div>"))
                .unwrap()
        });
        let response = t
            .page
            .goto(format!("{}/redirect", t.server.prefix))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(t.page.url(), t.server.empty_page);
        assert_eq!(response.status(), 200);
        let redirected_from = response.request().redirected_from().unwrap();
        assert_eq!(
            redirected_from.url(),
            format!("{}/redirect", t.server.prefix)
        );
    });
}

#[test]
fn should_capture_iframe_navigation_request() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let response = t
            .page
            .goto(format!("{}/frames/one-frame.html", t.server.prefix))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            t.page.url(),
            format!("{}/frames/one-frame.html", t.server.prefix)
        );
        assert_eq!(response.frame().unwrap(), t.page.main_frame());
        assert_eq!(
            response.url(),
            format!("{}/frames/one-frame.html", t.server.prefix)
        );
        assert_eq!(t.page.frames().len(), 2);
    });
}

#[test]
fn should_capture_cross_process_iframe_navigation_request() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let url = format!("{}/frames/one-frame.html", t.server.cross_process_prefix);
        let response = t.page.goto(&url).await.unwrap().unwrap();
        assert_eq!(t.page.url(), url);
        assert_eq!(response.frame().unwrap(), t.page.main_frame());
        assert_eq!(t.page.frames().len(), 2);
    });
}

#[test]
fn should_work_with_anchor_navigation() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(t.page.url(), t.server.empty_page);
        let response = t
            .page
            .goto(format!("{}#foo", t.server.empty_page))
            .await
            .unwrap();
        assert!(response.is_none());
        assert_eq!(t.page.url(), format!("{}#foo", t.server.empty_page));
        t.page
            .goto(format!("{}#bar", t.server.empty_page))
            .await
            .unwrap();
        assert_eq!(t.page.url(), format!("{}#bar", t.server.empty_page));
    });
}

#[test]
fn should_work_with_redirects() {
    run(|t| async move {
        t.server
            .set_redirect("/redirect/1.html", "/redirect/2.html");
        t.server.set_redirect("/redirect/2.html", "/empty.html");
        let response = t
            .page
            .goto(format!("{}/redirect/1.html", t.server.prefix))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(t.page.url(), t.server.empty_page);
    });
}

#[test]
fn should_navigate_to_about_blank() {
    run(|t| async move {
        let response = t.page.goto("about:blank").await.unwrap();
        assert!(response.is_none());
    });
}

#[test]
fn should_return_response_when_page_changes_its_url_after_load() {
    run(|t| async move {
        let response = t
            .page
            .goto(format!("{}/historyapi.html", t.server.prefix))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), 200);
    });
}

#[test]
fn should_work_with_subframes_return_204() {
    run(|t| async move {
        t.server.set_route("/frames/frame.html", |_| async {
            Response::builder().status(204).body(Body::empty()).unwrap()
        });
        t.page
            .goto(format!("{}/frames/one-frame.html", t.server.prefix))
            .await
            .unwrap();
        t.page
            .goto(format!("{}/frames/one-frame.html", t.server.prefix))
            .wait_until(LoadState::DomContentLoaded)
            .await
            .unwrap();
    });
}

#[test]
fn should_fail_when_server_returns_204() {
    run(|t| async move {
        t.server.set_route("/empty.html", |_| async {
            Response::builder().status(204).body(Body::empty()).unwrap()
        });
        assert_err_contains(t.page.goto(&t.server.empty_page).await, "net::ERR_ABORTED");
    });
}

#[test]
fn should_navigate_to_empty_page_with_domcontentloaded() {
    run(|t| async move {
        let response = t
            .page
            .goto(&t.server.empty_page)
            .wait_until(LoadState::DomContentLoaded)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), 200);
    });
}

#[test]
fn should_work_when_page_calls_history_api_in_beforeunload() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .evaluate::<()>("() => { window.addEventListener('beforeunload', () => history.replaceState(null, 'initial', window.location.href), false); }")
            .await
            .unwrap();
        let response = t
            .page
            .goto(format!("{}/grid.html", t.server.prefix))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), 200);
    });
}

#[test]
fn should_fail_when_navigating_to_bad_url() {
    run(|t| async move {
        assert_err_contains(
            t.page.goto("asdfasdf").await,
            "Cannot navigate to invalid URL",
        );
    });
}

#[test]
fn should_not_throw_if_networkidle0_is_passed_as_an_option() {
    run(|t| async move {
        let state: LoadState = "networkidle0".parse().unwrap();
        t.page
            .goto(&t.server.empty_page)
            .wait_until(state)
            .await
            .unwrap();
        let err = "networkidle2".parse::<LoadState>().unwrap_err();
        assert!(
            err.to_string()
                .contains("expected one of (load|domcontentloaded|networkidle|commit)")
        );
    });
}

#[test]
fn should_fail_when_main_resources_failed_to_load() {
    run(|t| async move {
        let e = assert_err_contains(
            t.page.goto("http://localhost:44123/non-existing-url").await,
            "net::ERR_CONNECTION_REFUSED",
        );
        let text = e.to_string();
        assert!(
            text.starts_with(
                "page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:44123/non-existing-url"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "navigating to \"http://localhost:44123/non-existing-url\", waiting until \"load\""
            ),
            "{text}"
        );
    });
}

#[test]
fn should_fail_when_exceeding_maximum_navigation_timeout() {
    run(|t| async move {
        t.server.hang("/empty.html");
        let e = assert_err_contains(
            t.page.goto(&t.server.empty_page).timeout(1).await,
            "page.goto: Timeout 1ms exceeded.",
        );
        assert!(e.is_timeout());
        assert!(e.to_string().contains(&t.server.empty_page));
    });
}

#[test]
fn should_fail_when_exceeding_default_maximum_navigation_timeout() {
    run(|t| async move {
        t.server.hang("/empty.html");
        t.context.set_default_navigation_timeout(2);
        t.page.set_default_navigation_timeout(1);
        let e = assert_err_contains(
            t.page.goto(&t.server.empty_page).await,
            "page.goto: Timeout 1ms exceeded.",
        );
        assert!(e.is_timeout());
    });
}

#[test]
fn should_fail_when_exceeding_browser_context_navigation_timeout() {
    run(|t| async move {
        t.server.hang("/empty.html");
        t.context.set_default_navigation_timeout(2);
        assert_err_contains(
            t.page.goto(&t.server.empty_page).await,
            "page.goto: Timeout 2ms exceeded.",
        );
    });
}

#[test]
fn should_fail_when_exceeding_default_maximum_timeout() {
    run(|t| async move {
        t.server.hang("/empty.html");
        t.context.set_default_timeout(2);
        t.page.set_default_timeout(1);
        assert_err_contains(
            t.page.goto(&t.server.empty_page).await,
            "page.goto: Timeout 1ms exceeded.",
        );
    });
}

#[test]
fn should_fail_when_exceeding_browser_context_timeout() {
    run(|t| async move {
        t.server.hang("/empty.html");
        t.context.set_default_timeout(2);
        assert_err_contains(
            t.page.goto(&t.server.empty_page).await,
            "page.goto: Timeout 2ms exceeded.",
        );
    });
}

#[test]
fn should_prioritize_default_navigation_timeout_over_default_timeout() {
    run(|t| async move {
        t.server.hang("/empty.html");
        t.page.set_default_timeout(0);
        t.page.set_default_navigation_timeout(1);
        assert_err_contains(
            t.page.goto(&t.server.empty_page).await,
            "page.goto: Timeout 1ms exceeded.",
        );
    });
}

#[test]
fn should_disable_timeout_when_its_set_to_0() {
    run(|t| async move {
        t.page
            .goto(format!("{}/grid.html", t.server.prefix))
            .timeout(0)
            .wait_until(LoadState::Load)
            .await
            .unwrap();
        let ready: String = t.page.evaluate("document.readyState").await.unwrap();
        assert_eq!(ready, "complete");
    });
}

#[test]
fn should_fail_when_replaced_by_another_navigation() {
    run(|t| async move {
        t.server.hang("/empty.html");
        let page = t.page.clone();
        let first = tokio::spawn(page.goto(&t.server.empty_page).into_future());
        t.server.wait_for_request("/empty.html").await;
        t.page
            .goto(format!("{}/one-style.html", t.server.prefix))
            .await
            .unwrap();
        let error = first.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("net::ERR_ABORTED"), "{error}");
    });
}

#[test]
fn should_work_when_navigating_to_valid_url_data_url_and_404() {
    run(|t| async move {
        let response = t.page.goto(&t.server.empty_page).await.unwrap().unwrap();
        assert!(response.ok());
        let response = t.page.goto("data:text/html,hello").await.unwrap();
        assert!(response.is_none());
        let response = t
            .page
            .goto(format!("{}/not-found", t.server.prefix))
            .await
            .unwrap()
            .unwrap();
        assert!(!response.ok());
        assert_eq!(response.status(), 404);
    });
}

#[test]
fn should_return_last_response_in_redirect_chain() {
    run(|t| async move {
        t.server
            .set_redirect("/redirect/1.html", "/redirect/2.html");
        t.server
            .set_redirect("/redirect/2.html", "/redirect/3.html");
        t.server
            .set_redirect("/redirect/3.html", &t.server.empty_page);
        let response = t
            .page
            .goto(format!("{}/redirect/1.html", t.server.prefix))
            .await
            .unwrap()
            .unwrap();
        assert!(response.ok());
        assert_eq!(response.url(), t.server.empty_page);
        // The chain is reachable from the final request.
        let mut chain = vec![];
        let mut request = Some(response.request());
        while let Some(r) = request {
            chain.push(r.url().to_string());
            request = r.redirected_from();
        }
        assert_eq!(chain.len(), 4);
    });
}

#[test]
fn should_not_leak_during_many_navigations() {
    run(|t| async move {
        for _ in 0..20 {
            t.page.goto(&t.server.empty_page).await.unwrap();
        }
        for _ in 0..20 {
            let _ = t.page.goto("asdf").await;
        }
        let waits: Vec<_> = (0..20)
            .map(|_| t.page.wait_for_navigation().into_future())
            .collect();
        t.page.goto(&t.server.empty_page).await.unwrap();
        for w in futures::future::join_all(waits).await {
            w.unwrap();
        }
    });
}

#[test]
fn should_navigate_to_url_with_hash_and_fire_requests_without_hash() {
    run(|t| async move {
        let response = t
            .page
            .goto(format!("{}#hash", t.server.empty_page))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.url(), t.server.empty_page);
        let requested: Vec<String> = t
            .server
            .requested()
            .into_iter()
            .filter(|p| p != "/favicon.ico")
            .collect();
        assert_eq!(requested, vec!["/empty.html".to_string()]);
    });
}

#[test]
fn should_work_with_self_requesting_page() {
    run(|t| async move {
        let response = t
            .page
            .goto(format!("{}/self-request.html", t.server.prefix))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), 200);
        assert!(response.url().contains("self-request.html"));
    });
}

#[test]
fn should_send_referer() {
    run(|t| async move {
        t.page
            .goto(format!("{}/grid.html", t.server.prefix))
            .referer("http://google.com/")
            .await
            .unwrap();
        let headers = t.server.request_headers("/grid.html").unwrap();
        assert_eq!(headers.get("referer").unwrap(), "http://google.com/");
        assert_eq!(t.page.url(), format!("{}/grid.html", t.server.prefix));
        t.page
            .goto(format!("{}/title.html", t.server.prefix))
            .referer("https://microsoft.com/xbox/")
            .await
            .unwrap();
        let headers = t.server.request_headers("/title.html").unwrap();
        assert_eq!(
            headers.get("referer").unwrap(),
            "https://microsoft.com/xbox/"
        );
    });
}

#[test]
fn should_fail_when_canceled_by_another_navigation() {
    run(|t| async move {
        t.server.hang("/one-style.html");
        let failed = tokio::spawn(
            t.page
                .goto(format!("{}/one-style.html", t.server.prefix))
                .into_future(),
        );
        t.server.wait_for_request("/one-style.html").await;
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert!(failed.await.unwrap().is_err());
    });
}

#[test]
fn should_report_main_resource_body() {
    run(|t| async move {
        t.server
            .set_content("/empty.html", "text/html; charset=utf-8", "Ü (lowercase ü)");
        let response = t.page.goto(&t.server.empty_page).await.unwrap().unwrap();
        assert_eq!(response.text().await.unwrap(), "Ü (lowercase ü)");
    });
}

#[test]
fn should_not_throw_unhandled_rejections_on_invalid_url() {
    run(|t| async move {
        assert!(
            t.page
                .goto("https://www.youtube Panel Title.com/")
                .await
                .is_err()
        );
    });
}

#[test]
fn should_return_from_goto_if_new_navigation_is_started() {
    run(|t| async move {
        t.server.hang("/slow.js");
        let navigation = tokio::spawn(
            t.page
                .goto(format!("{}/load-event/load-event.html", t.server.prefix))
                .into_future(),
        );
        // The document has committed once it requests the hanging script;
        // a fixed sleep raced with slow CI runners (ERR_ABORTED before commit).
        t.server.wait_for_request("/slow.js").await;
        assert!(!navigation.is_finished());
        t.page.goto(&t.server.empty_page).await.unwrap();
        let response = navigation.await.unwrap().unwrap().unwrap();
        assert_eq!(response.status(), 200);
    });
}

#[test]
fn should_return_when_navigation_is_committed_if_commit_is_specified() {
    run(|t| async move {
        t.server.hang("/script.js");
        t.server.set_content(
            "/empty.html",
            "text/html",
            "<title>Hello</title><script src=\"script.js\"></script>",
        );
        let response = t
            .page
            .goto(&t.server.empty_page)
            .wait_until(LoadState::Commit)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(t.page.title().await.unwrap(), "Hello");
    });
}

#[test]
fn should_wait_for_load_when_iframe_attaches_and_detaches() {
    run(|t| async move {
        t.server.set_content(
            "/empty.html",
            "text/html",
            "<body><script>const iframe = document.createElement('iframe'); iframe.src = './iframe.html'; document.body.appendChild(iframe); setTimeout(() => iframe.remove(), 1000);</script></body>",
        );
        t.server.set_content(
            "/iframe.html",
            "text/html",
            "<link rel=\"stylesheet\" href=\"./style2.css\">",
        );
        t.server.hang("/style2.css");
        t.page
            .goto(&t.server.empty_page)
            .wait_until(LoadState::Load)
            .await
            .unwrap();
        let count: i32 = t
            .page
            .evaluate("document.querySelectorAll('iframe').length")
            .await
            .unwrap();
        assert_eq!(count, 0);
    });
}

#[test]
fn should_return_url_with_basic_auth_info() {
    run(|t| async move {
        let url = format!("http://admin:admin@localhost:{}/empty.html", t.server.port);
        t.page.goto(&url).await.unwrap();
        assert_eq!(t.page.url(), url);
    });
}

#[test]
fn should_wait_for_networkidle() {
    run(|t| async move {
        t.server.set_route("/slow.css", |_| async {
            tokio::time::sleep(Duration::from_millis(600)).await;
            Response::builder()
                .header("content-type", "text/css")
                .body(Body::from(""))
                .unwrap()
        });
        t.server.set_content(
            "/networkidle.html",
            "text/html",
            "<script>window.addEventListener('load', () => setTimeout(() => { const l = document.createElement('link'); l.rel = 'stylesheet'; l.href = '/slow.css'; document.head.appendChild(l); }, 50));</script>",
        );
        let start = std::time::Instant::now();
        t.page
            .goto(format!("{}/networkidle.html", t.server.prefix))
            .wait_until(LoadState::NetworkIdle)
            .await
            .unwrap();
        assert!(
            start.elapsed() >= Duration::from_millis(1000),
            "{:?}",
            start.elapsed()
        );
        assert!(t.page.main_frame().has_load_state(LoadState::NetworkIdle));
    });
}

// ---- frame-goto.spec.ts ----

#[test]
fn frame_goto_should_navigate_subframes() {
    run(|t| async move {
        t.page
            .goto(format!("{}/frames/one-frame.html", t.server.prefix))
            .await
            .unwrap();
        assert!(t.page.frames()[0].url().contains("/frames/one-frame.html"));
        assert!(t.page.frames()[1].url().contains("/frames/frame.html"));
        let response = t.page.frames()[1]
            .goto(&t.server.empty_page)
            .await
            .unwrap()
            .unwrap();
        assert!(response.ok());
        assert_eq!(response.frame().unwrap(), t.page.frames()[1]);
    });
}

#[test]
fn frame_goto_should_reject_when_frame_detaches() {
    run(|t| async move {
        t.page
            .goto(format!("{}/frames/one-frame.html", t.server.prefix))
            .await
            .unwrap();
        t.server.hang("/empty.html");
        let frame = t.page.frames()[1].clone();
        let navigation = tokio::spawn(frame.goto(&t.server.empty_page).into_future());
        t.server.wait_for_request("/empty.html").await;
        t.page
            .evaluate::<()>("() => document.querySelector('iframe').remove()")
            .await
            .unwrap();
        let error = navigation.await.unwrap().unwrap_err().to_string();
        assert!(
            error.contains("frame was detached") || error.contains("Frame was detached"),
            "{error}"
        );
    });
}

#[test]
fn frame_goto_should_continue_after_client_redirect() {
    run(|t| async move {
        t.server.hang("/frames/script.js");
        let url = format!("{}/frames/child-redirect.html", t.server.prefix);
        t.server.set_content(
            "/frames/child-redirect.html",
            "text/html",
            "<iframe src='./redirect-target.html'></iframe>",
        );
        t.server.set_content(
            "/frames/redirect-target.html",
            "text/html",
            "<script>location.href = './frame.html';</script>",
        );
        let error = t
            .page
            .goto(&url)
            .timeout(5000)
            .wait_until(LoadState::NetworkIdle)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("page.goto: Timeout 5000ms exceeded."),
            "{error}"
        );
        assert!(
            error.to_string().contains(&format!(
                "navigating to \"{url}\", waiting until \"networkidle\""
            )),
            "{error}"
        );
    });
}

#[test]
fn frame_goto_should_return_matching_responses() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let mut frames = vec![];
        for id in ["frame1", "frame2", "frame3"] {
            frames.push(attach_frame(&t.page, id, &t.server.empty_page).await);
        }
        let texts = ["AAA", "BBB", "CCC"];
        for (i, text) in texts.iter().enumerate() {
            let path = format!("/{i}.html");
            t.server.set_content(&path, "text/html", texts[i]);
            let response = frames[i]
                .goto(format!("{}{}", t.server.prefix, path))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(response.frame().unwrap(), frames[i]);
            assert_eq!(response.text().await.unwrap(), *text);
        }
    });
}
