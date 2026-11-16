//! Ports of `tests/library/browsercontext-route.spec.ts`,
//! `browsercontext-credentials.spec.ts`, `browsercontext-set-extra-http-headers.spec.ts`,
//! `page/page-set-extra-http-headers.spec.ts`, the offline tests of
//! `browsercontext-basic.spec.ts`, `har.spec.ts` and `browsercontext-har.spec.ts` (subsets).
//!
//! Skipped (reason):
//! - RO-169..RO-171 Set-Cookie with `context.cookies()` (cookies API not ported yet).
//! - RO-172 ignoreHTTPSErrors and `tests/library/ignorehttpserrors.spec.ts`: the test server
//!   has no TLS.
//! - RO-183 disk cache, RO-184 popup main request (popups are adopted after their first
//!   request), RO-185 URLPattern.
//! - AU-003/AU-007/AU-008/AU-010 several credentials (a single credential is supported).
//! - HA-068..HA-083 use Playwright's recorded `har-fulfill.har` asset and `toHaveCSS`; they are
//!   replaced by round-trip tests that record a HAR and replay it.
//! - HA-085..HA-087, HA-091, HA-095..HA-097 zipped HARs (not supported).
//! - `har.spec.ts` tests about cookies of `context.addCookies`, service workers, WebSocket frames
//!   in HARs, security details (no TLS server) and `page.click`-driven navigations.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::header;
use axum::response::Response as HttpResponse;
use oxidewright::{HarContentPolicy, HarMode, HarNotFound, HttpCredentials, RecordHar, Route};

use crate::common::{assert_err_contains, run};

fn temp_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "oxidewright-test-{}-{}",
        std::process::id(),
        fastrand::u64(..)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn ro_165_should_intercept() {
    run(|t| async move {
        let intercepted = Arc::new(Mutex::new(false));
        let i = intercepted.clone();
        let context = t.browser.new_context().await.unwrap();
        context
            .route("**/empty.html", move |route: Route| {
                let request = route.request();
                assert!(request.url().contains("empty.html"));
                assert!(request.headers().contains_key("user-agent"));
                assert_eq!(request.method(), "GET");
                assert!(request.is_navigation_request());
                assert_eq!(request.resource_type(), "document");
                *i.lock().unwrap() = true;
                async move { route.continue_().await }
            })
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        let response = page.goto(&t.server.empty_page).await.unwrap().unwrap();
        assert!(response.ok());
        assert!(*intercepted.lock().unwrap());
        context.close().await.unwrap();
    });
}

#[test]
fn ro_166_should_unroute() {
    run(|t| async move {
        let context = t.browser.new_context().await.unwrap();
        let page = context.new_page().await.unwrap();
        let intercepted = Arc::new(Mutex::new(Vec::<u32>::new()));
        let push = |n: u32| {
            let intercepted = intercepted.clone();
            move |route: Route| {
                intercepted.lock().unwrap().push(n);
                async move { route.fallback().await }
            }
        };
        context.route("**/*", push(1)).await.unwrap();
        context.route("**/empty.html", push(2)).await.unwrap();
        context.route("**/empty.html", push(3)).await.unwrap();
        let id4 = context.route("**/empty.html", push(4)).await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(*intercepted.lock().unwrap(), vec![4, 3, 2, 1]);
        intercepted.lock().unwrap().clear();
        context.unroute("**/empty.html").handler(id4).await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(*intercepted.lock().unwrap(), vec![3, 2, 1]);
        intercepted.lock().unwrap().clear();
        context.unroute("**/empty.html").await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(*intercepted.lock().unwrap(), vec![1]);
        context.close().await.unwrap();
    });
}

#[test]
fn ro_167_should_yield_to_page_route() {
    run(|t| async move {
        let context = t.browser.new_context().await.unwrap();
        context
            .route("**/empty.html", |route: Route| async move {
                route.fulfill().status(200).body("context").await
            })
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.route("**/empty.html", |route: Route| async move {
            route.fulfill().status(200).body("page").await
        })
        .await
        .unwrap();
        let response = page.goto(&t.server.empty_page).await.unwrap().unwrap();
        assert!(response.ok());
        assert_eq!(response.text().await.unwrap(), "page");
        context.close().await.unwrap();
    });
}

#[test]
fn ro_168_should_fall_back_to_context_route() {
    run(|t| async move {
        let context = t.browser.new_context().await.unwrap();
        context
            .route("**/empty.html", |route: Route| async move {
                route.fulfill().status(200).body("context").await
            })
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.route("**/non-empty.html", |route: Route| async move {
            route.fulfill().status(200).body("page").await
        })
        .await
        .unwrap();
        let response = page.goto(&t.server.empty_page).await.unwrap().unwrap();
        assert_eq!(response.text().await.unwrap(), "context");
        context.close().await.unwrap();
    });
}

#[test]
fn ro_176_should_overwrite_post_body_with_empty_string() {
    run(|t| async move {
        t.context
            .route("**/empty.html", |route: Route| async move {
                route.continue_().post_data("").await
            })
            .await
            .unwrap();
        let server_request = t.server.wait_for_request("/empty.html");
        t.page
            .set_content(format!(
                "<script>(async () => {{ await fetch('{}', {{ method: 'POST', body: 'original' }}); }})()</script>",
                t.server.empty_page
            ))
            .await
            .unwrap();
        assert_eq!(server_request.await.text(), "");
    });
}

#[test]
fn ro_177_should_chain_fallback() {
    run(|t| async move {
        let intercepted = Arc::new(Mutex::new(Vec::<u32>::new()));
        for n in 1..=3u32 {
            let i = intercepted.clone();
            t.context
                .route("**/empty.html", move |route: Route| {
                    i.lock().unwrap().push(n);
                    async move { route.fallback().await }
                })
                .await
                .unwrap();
        }
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(*intercepted.lock().unwrap(), vec![3, 2, 1]);
    });
}

#[test]
fn ro_179_should_not_chain_fulfill() {
    run(|t| async move {
        let failed = Arc::new(Mutex::new(false));
        let f = failed.clone();
        t.context
            .route("**/empty.html", move |_route: Route| {
                *f.lock().unwrap() = true;
                async { Ok(()) }
            })
            .await
            .unwrap();
        t.context
            .route("**/empty.html", |route: Route| async move {
                route.fulfill().status(200).body("fulfilled").await
            })
            .await
            .unwrap();
        t.context
            .route("**/empty.html", |route: Route| async move {
                route.fallback().await
            })
            .await
            .unwrap();
        let response = t.page.goto(&t.server.empty_page).await.unwrap().unwrap();
        assert_eq!(response.text().await.unwrap(), "fulfilled");
        assert!(!*failed.lock().unwrap());
    });
}

#[test]
fn ro_180_should_not_chain_abort() {
    run(|t| async move {
        let failed = Arc::new(Mutex::new(false));
        let f = failed.clone();
        t.context
            .route("**/empty.html", move |_route: Route| {
                *f.lock().unwrap() = true;
                async { Ok(()) }
            })
            .await
            .unwrap();
        t.context
            .route("**/empty.html", |route: Route| async move {
                route.abort().await
            })
            .await
            .unwrap();
        t.context
            .route("**/empty.html", |route: Route| async move {
                route.fallback().await
            })
            .await
            .unwrap();
        assert!(t.page.goto(&t.server.empty_page).await.is_err());
        assert!(!*failed.lock().unwrap());
    });
}

#[test]
fn ro_181_should_chain_fallback_into_page() {
    run(|t| async move {
        let intercepted = Arc::new(Mutex::new(Vec::<u32>::new()));
        for n in 1..=3u32 {
            let i = intercepted.clone();
            t.context
                .route("**/empty.html", move |route: Route| {
                    i.lock().unwrap().push(n);
                    async move { route.fallback().await }
                })
                .await
                .unwrap();
        }
        for n in 4..=6u32 {
            let i = intercepted.clone();
            t.page
                .route("**/empty.html", move |route: Route| {
                    i.lock().unwrap().push(n);
                    async move { route.fallback().await }
                })
                .await
                .unwrap();
        }
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(*intercepted.lock().unwrap(), vec![6, 5, 4, 3, 2, 1]);
    });
}

#[test]
fn ro_182_should_fall_back_async() {
    run(|t| async move {
        let intercepted = Arc::new(Mutex::new(Vec::<u32>::new()));
        for n in 1..=3u32 {
            let i = intercepted.clone();
            t.context
                .route("**/empty.html", move |route: Route| {
                    let i = i.clone();
                    async move {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        i.lock().unwrap().push(n);
                        route.fallback().await
                    }
                })
                .await
                .unwrap();
        }
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(*intercepted.lock().unwrap(), vec![3, 2, 1]);
    });
}

// ------------------------------------------------------------ credentials

#[test]
fn au_001_should_fail_without_credentials() {
    run(|t| async move {
        t.server.set_auth("/empty.html", "user", "pass");
        assert_unauthorized(t.page.goto(&t.server.empty_page).await);
    });
}

/// Without credentials (and so without `Fetch`), headless Chromium fails the
/// navigation instead of rendering the 401 response like Playwright's
/// `chrome-headless-shell` does.
fn assert_unauthorized(result: oxidewright::Result<Option<oxidewright::Response>>) {
    match result {
        Ok(Some(response)) => assert_eq!(response.status(), 401),
        Ok(None) => panic!("no response"),
        Err(err) => assert!(
            err.to_string().contains("ERR_INVALID_AUTH_CREDENTIALS"),
            "{err}"
        ),
    }
}

#[test]
fn au_002_should_work_with_set_http_credentials() {
    run(|t| async move {
        t.server.set_auth("/empty.html", "user", "pass");
        assert_unauthorized(t.page.goto(&t.server.empty_page).await);
        t.context
            .set_http_credentials(Some(HttpCredentials::new("user", "pass")))
            .await
            .unwrap();
        let response = t.page.goto(&t.server.empty_page).await.unwrap().unwrap();
        assert_eq!(response.status(), 200);
    });
}

#[test]
fn au_004_should_work_with_correct_credentials() {
    run(|t| async move {
        t.server.set_auth("/empty.html", "user", "pass");
        let context = t
            .browser
            .new_context()
            .http_credentials(HttpCredentials::new("user", "pass"))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        let response = page.goto(&t.server.empty_page).await.unwrap().unwrap();
        assert_eq!(response.status(), 200);
        context.close().await.unwrap();
    });
}

#[test]
fn au_005_should_fail_with_wrong_credentials() {
    run(|t| async move {
        t.server.set_auth("/empty.html", "user", "pass");
        let context = t
            .browser
            .new_context()
            .http_credentials(HttpCredentials::new("foo", "bar"))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        let response = page.goto(&t.server.empty_page).await.unwrap().unwrap();
        assert_eq!(response.status(), 401);
        context.close().await.unwrap();
    });
}

#[test]
fn au_006_should_return_resource_body() {
    run(|t| async move {
        t.server.set_auth("/title.html", "user", "pass");
        let context = t
            .browser
            .new_context()
            .http_credentials(HttpCredentials::new("user", "pass"))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        let response = page
            .goto(format!("{}/title.html", t.server.prefix))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(page.title().await.unwrap(), "Woof-Woof");
        assert!(response.text().await.unwrap().contains("Woof-Woof"));
        context.close().await.unwrap();
    });
}

#[test]
fn au_011_should_work_with_correct_credentials_and_matching_origin() {
    run(|t| async move {
        t.server.set_auth("/empty.html", "user", "pass");
        let context = t
            .browser
            .new_context()
            .http_credentials(HttpCredentials::new("user", "pass").origin(t.server.prefix.clone()))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        let response = page.goto(&t.server.empty_page).await.unwrap().unwrap();
        assert_eq!(response.status(), 200);
        context.close().await.unwrap();
    });
}

#[test]
fn au_012_should_work_with_correct_credentials_and_matching_origin_case_insensitive() {
    run(|t| async move {
        t.server.set_auth("/empty.html", "user", "pass");
        let context = t
            .browser
            .new_context()
            .http_credentials(
                HttpCredentials::new("user", "pass").origin(t.server.prefix.to_uppercase()),
            )
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        let response = page.goto(&t.server.empty_page).await.unwrap().unwrap();
        assert_eq!(response.status(), 200);
        context.close().await.unwrap();
    });
}

#[test]
fn au_014_should_fail_with_correct_credentials_and_mismatching_hostname() {
    run(|t| async move {
        t.server.set_auth("/empty.html", "user", "pass");
        let origin = t.server.prefix.replace("localhost", "mismatching-hostname");
        let context = t
            .browser
            .new_context()
            .http_credentials(HttpCredentials::new("user", "pass").origin(origin))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        let response = page.goto(&t.server.empty_page).await.unwrap().unwrap();
        assert_eq!(response.status(), 401);
        context.close().await.unwrap();
    });
}

#[test]
fn au_015_should_fail_with_correct_credentials_and_mismatching_port() {
    run(|t| async move {
        t.server.set_auth("/empty.html", "user", "pass");
        let origin = t
            .server
            .prefix
            .replace(&t.server.port.to_string(), &(t.server.port + 1).to_string());
        let context = t
            .browser
            .new_context()
            .http_credentials(HttpCredentials::new("user", "pass").origin(origin))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        let response = page.goto(&t.server.empty_page).await.unwrap().unwrap();
        assert_eq!(response.status(), 401);
        context.close().await.unwrap();
    });
}

// ----------------------------------------------------------- extra headers

#[test]
fn ne_114_should_work_page_extra_headers() {
    run(|t| async move {
        t.page
            .set_extra_http_headers([("foo", "bar")])
            .await
            .unwrap();
        let request = t.server.wait_for_request("/empty.html");
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(request.await.header("foo").as_deref(), Some("bar"));
    });
}

#[test]
fn ne_119_should_override_extra_headers_from_browser_context() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .extra_http_headers([("fOo", "bAr"), ("baR", "foO")])
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.set_extra_http_headers([("Foo", "Bar")]).await.unwrap();
        let request = t.server.wait_for_request("/empty.html");
        page.goto(&t.server.empty_page).await.unwrap();
        let request = request.await;
        assert_eq!(request.header("foo").as_deref(), Some("Bar"));
        assert_eq!(request.header("bar").as_deref(), Some("foO"));
        context.close().await.unwrap();
    });
}

#[test]
fn context_set_extra_http_headers_should_work() {
    run(|t| async move {
        t.context
            .set_extra_http_headers([("foo", "bar")])
            .await
            .unwrap();
        let request = t.server.wait_for_request("/empty.html");
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(request.await.header("foo").as_deref(), Some("bar"));
    });
}

// ----------------------------------------------------------------- offline

#[test]
fn should_work_with_offline_option() {
    run(|t| async move {
        let context = t.browser.new_context().offline(true).await.unwrap();
        let page = context.new_page().await.unwrap();
        assert!(page.goto(&t.server.empty_page).await.is_err());
        context.set_offline(false).await.unwrap();
        let response = page.goto(&t.server.empty_page).await.unwrap().unwrap();
        assert_eq!(response.status(), 200);
        context.close().await.unwrap();
    });
}

#[test]
fn should_emulate_navigator_online() {
    run(|t| async move {
        let online: bool = t
            .page
            .evaluate("() => window.navigator.onLine")
            .await
            .unwrap();
        assert!(online);
        t.context.set_offline(true).await.unwrap();
        let online: bool = t
            .page
            .evaluate("() => window.navigator.onLine")
            .await
            .unwrap();
        assert!(!online);
        t.context.set_offline(false).await.unwrap();
        let online: bool = t
            .page
            .evaluate("() => window.navigator.onLine")
            .await
            .unwrap();
        assert!(online);
    });
}

// --------------------------------------------------------------------- HAR

async fn record(
    t: &crate::common::TestContext,
    options: RecordHar,
    path: &str,
) -> serde_json::Value {
    let har_path = options.path.clone();
    let context = t.browser.new_context().record_har(options).await.unwrap();
    let page = context.new_page().await.unwrap();
    page.goto(format!("{}{path}", t.server.prefix))
        .await
        .unwrap();
    context.close().await.unwrap();
    serde_json::from_str(&std::fs::read_to_string(har_path).unwrap()).unwrap()
}

#[test]
fn ha_should_have_version_and_creator() {
    run(|t| async move {
        let har = record(&t, RecordHar::new(temp_path("test.har")), "/empty.html").await;
        assert_eq!(har["log"]["version"], "1.2");
        assert_eq!(har["log"]["creator"]["name"], "oxidewright");
        assert_eq!(har["log"]["browser"]["name"], "chromium");
    });
}

#[test]
fn ha_should_have_pages() {
    run(|t| async move {
        let har = record(&t, RecordHar::new(temp_path("test.har")), "/title.html").await;
        let pages = har["log"]["pages"].as_array().unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0]["title"], "Woof-Woof");
        assert!(pages[0]["pageTimings"]["onContentLoad"].as_f64().unwrap() >= 0.0);
        assert!(pages[0]["pageTimings"]["onLoad"].as_f64().unwrap() >= 0.0);
        let entries = har["log"]["entries"].as_array().unwrap();
        assert_eq!(entries[0]["pageref"], pages[0]["id"]);
    });
}

#[test]
fn ha_should_include_request_and_response() {
    run(|t| async move {
        let har = record(&t, RecordHar::new(temp_path("test.har")), "/empty.html").await;
        let entry = &har["log"]["entries"][0];
        assert_eq!(entry["request"]["url"], t.server.empty_page);
        assert_eq!(entry["request"]["method"], "GET");
        assert_eq!(entry["request"]["httpVersion"], "HTTP/1.1");
        assert!(entry["request"]["headers"].as_array().unwrap().len() > 1);
        assert_eq!(entry["response"]["status"], 200);
        assert_eq!(entry["response"]["statusText"], "OK");
        assert!(entry["response"]["headers"].as_array().unwrap().len() > 1);
        assert!(
            entry["response"]["content"]["mimeType"]
                .as_str()
                .unwrap()
                .contains("text/html")
        );
        assert!(entry["serverIPAddress"].as_str().is_some());
    });
}

#[test]
fn ha_should_include_content() {
    run(|t| async move {
        let har = record(&t, RecordHar::new(temp_path("test.har")), "/title.html").await;
        let content = &har["log"]["entries"][0]["response"]["content"];
        assert!(content["text"].as_str().unwrap().contains("Woof-Woof"));
        assert!(content["size"].as_i64().unwrap() > 0);
    });
}

#[test]
fn ha_should_omit_content() {
    run(|t| async move {
        let har = record(
            &t,
            RecordHar::new(temp_path("test.har")).content(HarContentPolicy::Omit),
            "/title.html",
        )
        .await;
        let content = &har["log"]["entries"][0]["response"]["content"];
        assert!(content.get("text").is_none());
    });
}

#[test]
fn ha_should_attach_content() {
    run(|t| async move {
        let path = temp_path("test.har");
        let har = record(
            &t,
            RecordHar::new(path.clone()).content(HarContentPolicy::Attach),
            "/title.html",
        )
        .await;
        let content = &har["log"]["entries"][0]["response"]["content"];
        let file = content["_file"].as_str().unwrap();
        assert!(file.ends_with(".html"));
        let text = std::fs::read_to_string(path.parent().unwrap().join(file)).unwrap();
        assert!(text.contains("Woof-Woof"));
    });
}

#[test]
fn ha_should_filter_by_url() {
    run(|t| async move {
        let har = record(
            &t,
            RecordHar::new(temp_path("test.har")).url_filter("**/*.css"),
            "/one-style.html",
        )
        .await;
        let entries = har["log"]["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(
            entries[0]["request"]["url"]
                .as_str()
                .unwrap()
                .ends_with("one-style.css")
        );
    });
}

#[test]
fn ha_should_record_minimal_mode() {
    run(|t| async move {
        let har = record(
            &t,
            RecordHar::new(temp_path("test.har")).mode(HarMode::Minimal),
            "/empty.html",
        )
        .await;
        assert!(har["log"].get("pages").is_none());
        let entry = &har["log"]["entries"][0];
        assert!(entry.get("serverIPAddress").is_none());
    });
}

#[test]
fn ha_should_include_post_data() {
    run(|t| async move {
        let path = temp_path("test.har");
        let context = t
            .browser
            .new_context()
            .record_har(RecordHar::new(path.clone()))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        page.evaluate::<()>(
            "() => fetch('./post', { method: 'POST', body: 'Hello' }).then(() => {})",
        )
        .await
        .unwrap();
        context.close().await.unwrap();
        let har: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let entry = har["log"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["request"]["method"] == "POST")
            .unwrap();
        assert_eq!(entry["request"]["postData"]["text"], "Hello");
    });
}

#[test]
fn ha_should_record_multiple_set_cookie_headers() {
    run(|t| async move {
        t.server.set_route("/empty.html", |_| async {
            HttpResponse::builder()
                .header(header::CONTENT_TYPE, "text/html")
                .header(header::SET_COOKIE, "name1=value1; HttpOnly")
                .header(header::SET_COOKIE, "name2=value2")
                .body(Body::from(""))
                .unwrap()
        });
        let har = record(&t, RecordHar::new(temp_path("test.har")), "/empty.html").await;
        let cookies = har["log"]["entries"][0]["response"]["cookies"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(cookies.len(), 2);
        assert_eq!(cookies[0]["name"], "name1");
        assert_eq!(cookies[0]["httpOnly"], true);
        assert_eq!(cookies[1]["name"], "name2");
    });
}

#[test]
fn ha_should_round_trip_har_with_route_from_har() {
    run(|t| async move {
        let path = temp_path("roundtrip.har");
        record(
            &t,
            RecordHar::new(path.clone()).mode(HarMode::Minimal),
            "/one-style.html",
        )
        .await;
        let context = t.browser.new_context().await.unwrap();
        context.route_from_har(&path).await.unwrap();
        let page = context.new_page().await.unwrap();
        // The server answers differently now: the HAR must be used.
        t.server
            .set_content("/one-style.html", "text/html", "<div>changed</div>");
        page.goto(format!("{}/one-style.html", t.server.prefix))
            .await
            .unwrap();
        let text: String = page
            .evaluate("() => document.body.textContent.trim()")
            .await
            .unwrap();
        assert_eq!(text, "hello, world!");
        let color: String = page
            .evaluate("() => getComputedStyle(document.body).backgroundColor")
            .await
            .unwrap();
        assert_eq!(color, "rgb(255, 192, 203)");
        context.close().await.unwrap();
    });
}

#[test]
fn ha_071_by_default_should_abort_requests_not_found_in_har() {
    run(|t| async move {
        let path = temp_path("abort.har");
        record(&t, RecordHar::new(path.clone()), "/title.html").await;
        let context = t.browser.new_context().await.unwrap();
        context.route_from_har(&path).await.unwrap();
        let page = context.new_page().await.unwrap();
        assert!(page.goto(&t.server.empty_page).await.is_err());
        context.close().await.unwrap();
    });
}

#[test]
fn ha_070_fallback_continue_should_continue_when_not_found_in_har() {
    run(|t| async move {
        let path = temp_path("fallback.har");
        record(&t, RecordHar::new(path.clone()), "/title.html").await;
        let page = t.context.new_page().await.unwrap();
        page.route_from_har(&path)
            .not_found(HarNotFound::Fallback)
            .await
            .unwrap();
        let response = page.goto(&t.server.empty_page).await.unwrap().unwrap();
        assert_eq!(response.status(), 200);
    });
}

#[test]
fn ha_072_should_report_bad_har() {
    run(|t| async move {
        let path = temp_path("bad.har");
        std::fs::write(&path, "not json").unwrap();
        assert!(t.context.route_from_har(&path).await.is_err());
        let missing = temp_path("missing.har");
        assert_err_contains(
            t.context.route_from_har(&missing).await,
            "Error reading HAR file",
        );
    });
}

#[test]
fn ha_073_should_only_handle_requests_matching_url_filter() {
    run(|t| async move {
        let path = temp_path("filter.har");
        record(&t, RecordHar::new(path.clone()), "/one-style.html").await;
        t.server
            .set_content("/one-style.html", "text/html", "<div>live</div>");
        let page = t.context.new_page().await.unwrap();
        page.route_from_har(&path)
            .url("**/*.css")
            .not_found(HarNotFound::Fallback)
            .await
            .unwrap();
        page.goto(format!("{}/one-style.html", t.server.prefix))
            .await
            .unwrap();
        let text: String = page
            .evaluate("() => document.body.textContent.trim()")
            .await
            .unwrap();
        assert_eq!(text, "live");
    });
}

#[test]
fn ha_079_should_change_document_url_after_redirected_navigation() {
    run(|t| async move {
        let path = temp_path("redirect.har");
        t.server.set_redirect("/redirect-me", "/title.html");
        record(&t, RecordHar::new(path.clone()), "/redirect-me").await;
        let context = t.browser.new_context().await.unwrap();
        context.route_from_har(&path).await.unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(format!("{}/redirect-me", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(page.url(), format!("{}/title.html", t.server.prefix));
        assert_eq!(page.title().await.unwrap(), "Woof-Woof");
        context.close().await.unwrap();
    });
}

#[test]
fn ha_098_page_unroute_all_should_stop_page_route_from_har() {
    run(|t| async move {
        let path = temp_path("unroute.har");
        record(&t, RecordHar::new(path.clone()), "/title.html").await;
        t.server
            .set_content("/title.html", "text/html", "<title>live</title>");
        let page = t.context.new_page().await.unwrap();
        page.route_from_har(&path).await.unwrap();
        page.unroute_all().await.unwrap();
        page.goto(format!("{}/title.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(page.title().await.unwrap(), "live");
    });
}

#[test]
fn ha_should_update_har_with_route_from_har_update() {
    run(|t| async move {
        let path = temp_path("update.har");
        let context = t.browser.new_context().await.unwrap();
        context
            .route_from_har(&path)
            .update(true)
            .update_content(HarContentPolicy::Embed)
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(format!("{}/title.html", t.server.prefix))
            .await
            .unwrap();
        context.close().await.unwrap();
        let har: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(
            har["log"]["entries"][0]["response"]["content"]["text"]
                .as_str()
                .unwrap()
                .contains("Woof-Woof")
        );
    });
}
