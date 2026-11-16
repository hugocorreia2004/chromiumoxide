//! Port of `tests/page/page-route.spec.ts`.
//!
//! Skipped (reason):
//! - RO-008 should properly return navigation response when URL has cookies: needs `context.addCookies`.
//! - RO-011/RO-012 sync XHR redirect / pause: covered by RO-013 (fetch); sync XHR blocks evaluate.
//! - RO-029 should work with encoded server - 2: needs `setContent` with a link waiting for the request to be reported (flaky with stylesheets).
//! - RO-030 should not throw "Invalid Interception Id": needs `$eval` (locators are another module).
//! - RO-033 should not fulfill with redirect status: WebKit only.
//! - RO-034..RO-040 CORS/OPTIONS tests against `https://example.com`: need network access to a public host.
//! - Tests using `page.$eval`/`page.click`/`page.setContent` with element waits, service workers,
//!   `context.addCookies`, `route.fetch` with Node's network stack options (`maxRedirects`), and
//!   WebKit/Firefox specific tests.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::response::Response as HttpResponse;
use oxidewright::{EventKind, Route};

use crate::common::{assert_err_contains, run};

#[test]
fn ro_001_should_intercept() {
    run(|t| async move {
        let intercepted = Arc::new(Mutex::new(false));
        let flag = intercepted.clone();
        let main = t.page.main_frame();
        t.page
            .route("**/empty.html", move |route: Route| {
                let flag = flag.clone();
                let main = main.clone();
                async move {
                    let request = route.request();
                    assert!(request.url().contains("empty.html"));
                    assert!(request.headers().contains_key("user-agent"));
                    assert_eq!(request.method(), "GET");
                    assert_eq!(request.post_data(), None);
                    assert!(request.is_navigation_request());
                    assert_eq!(request.resource_type(), "document");
                    assert!(request.frame() == Some(main.clone()));
                    assert_eq!(request.frame().unwrap().url(), "about:blank");
                    *flag.lock().unwrap() = true;
                    route.continue_().await
                }
            })
            .await
            .unwrap();
        let response = t.page.goto(&t.server.empty_page).await.unwrap().unwrap();
        assert!(response.ok());
        assert!(*intercepted.lock().unwrap());
    });
}

#[test]
fn ro_002_should_unroute() {
    run(|t| async move {
        let intercepted = Arc::new(Mutex::new(Vec::<u32>::new()));
        let push = |n: u32| {
            let intercepted = intercepted.clone();
            move |route: Route| {
                intercepted.lock().unwrap().push(n);
                async move { route.fallback().await }
            }
        };
        t.page.route("**/*", push(1)).await.unwrap();
        t.page.route("**/empty.html", push(2)).await.unwrap();
        t.page.route("**/empty.html", push(3)).await.unwrap();
        let re = regex::Regex::new("empty.html").unwrap();
        let id4 = t.page.route(re.clone(), push(4)).await.unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(*intercepted.lock().unwrap(), vec![4, 3, 2, 1]);

        intercepted.lock().unwrap().clear();
        t.page.unroute(re).handler(id4).await.unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(*intercepted.lock().unwrap(), vec![3, 2, 1]);

        intercepted.lock().unwrap().clear();
        t.page.unroute("**/empty.html").await.unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(*intercepted.lock().unwrap(), vec![1]);
    });
}

#[test]
fn ro_003_should_not_support_question_mark_in_glob_pattern() {
    run(|t| async move {
        t.server
            .set_content("/index", "text/plain", "index-no-hello");
        t.server
            .set_content("/index123hello", "text/plain", "index123hello");
        t.server
            .set_content("/index?hello", "text/plain", "index?hello");
        t.server
            .set_content("/index1hello", "text/plain", "index1hello");
        t.page
            .route("**/index?hello", |route: Route| async move {
                route.fulfill().body("intercepted any character").await
            })
            .await
            .unwrap();
        t.page
            .route("**/index\\?hello", |route: Route| async move {
                route.fulfill().body("intercepted question mark").await
            })
            .await
            .unwrap();
        t.page
            .goto(format!("{}/index?hello", t.server.prefix))
            .await
            .unwrap();
        assert!(
            t.page
                .content()
                .await
                .unwrap()
                .contains("intercepted question mark")
        );
        t.page
            .goto(format!("{}/index", t.server.prefix))
            .await
            .unwrap();
        assert!(t.page.content().await.unwrap().contains("index-no-hello"));
        t.page
            .goto(format!("{}/index1hello", t.server.prefix))
            .await
            .unwrap();
        assert!(t.page.content().await.unwrap().contains("index1hello"));
        t.page
            .goto(format!("{}/index123hello", t.server.prefix))
            .await
            .unwrap();
        assert!(t.page.content().await.unwrap().contains("index123hello"));
    });
}

#[test]
fn ro_004_should_work_when_post_is_redirected_with_302() {
    run(|t| async move {
        t.server.set_redirect("/rredirect", "/empty.html");
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .route(
                "**/*",
                |route: Route| async move { route.continue_().await },
            )
            .await
            .unwrap();
        t.page
            .set_content(
                "<form action='/rredirect' method='post'>\
                 <input type='hidden' id='foo' name='foo' value='FOOBAR'></form>",
            )
            .await
            .unwrap();
        let nav = t.page.wait_for_navigation();
        t.page
            .evaluate::<()>("() => document.querySelector('form').submit()")
            .await
            .unwrap();
        nav.await.unwrap();
    });
}

#[test]
fn ro_005_should_work_when_header_manipulation_headers_with_redirect() {
    run(|t| async move {
        t.server.set_redirect("/rrredirect", "/empty.html");
        t.page
            .route("**/*", |route: Route| async move {
                let mut headers = route.request().headers();
                headers.insert("foo".into(), "bar".into());
                route.continue_().headers(headers).await
            })
            .await
            .unwrap();
        t.page
            .goto(format!("{}/rrredirect", t.server.prefix))
            .await
            .unwrap();
    });
}

#[test]
fn ro_006_should_be_able_to_remove_headers() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .route("**/*", |route: Route| async move {
                let mut headers = route.request().headers();
                headers.remove("foo");
                route.continue_().headers(headers).await
            })
            .await
            .unwrap();
        let server_request = t.server.wait_for_request("/title.html");
        t.page
            .evaluate::<serde_json::Value>(
                "url => fetch(url, { headers: { foo: 'bar' } }).then(() => null)",
            )
            .arg(format!("{}/title.html", t.server.prefix))
            .await
            .unwrap();
        let request = server_request.await;
        assert_eq!(request.header("foo"), None);
    });
}

#[test]
fn ro_007_should_contain_referer_header() {
    run(|t| async move {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let r = requests.clone();
        t.page
            .route("**/*", move |route: Route| {
                r.lock().unwrap().push(route.request());
                async move { route.continue_().await }
            })
            .await
            .unwrap();
        t.page
            .goto(format!("{}/one-style.html", t.server.prefix))
            .await
            .unwrap();
        let requests = requests.lock().unwrap().clone();
        assert!(requests[1].url().contains("/one-style.css"));
        assert!(requests[1].headers()["referer"].contains("/one-style.html"));
    });
}

#[test]
fn ro_009_should_not_override_cookie_header() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .evaluate::<String>("() => document.cookie = 'original=value'")
            .await
            .unwrap();
        let seen = Arc::new(Mutex::new(None));
        let s = seen.clone();
        t.page
            .route("**", move |route: Route| {
                let s = s.clone();
                async move {
                    let mut headers = route.request().all_headers().await?;
                    *s.lock().unwrap() = headers.get("cookie").cloned();
                    headers.insert("cookie".into(), "overridden=value".into());
                    route.continue_().headers(headers).await
                }
            })
            .await
            .unwrap();
        let server_request = t.server.wait_for_request("/empty.html");
        t.page.goto(&t.server.empty_page).await.unwrap();
        let request = server_request.await;
        assert_eq!(seen.lock().unwrap().as_deref(), Some("original=value"));
        assert_eq!(request.header("cookie").as_deref(), Some("original=value"));
    });
}

#[test]
fn ro_010_should_show_custom_http_headers() {
    run(|t| async move {
        t.page
            .set_extra_http_headers([("foo", "bar")])
            .await
            .unwrap();
        t.page
            .route("**/*", |route: Route| async move {
                assert_eq!(route.request().headers()["foo"], "bar");
                route.continue_().await
            })
            .await
            .unwrap();
        let response = t.page.goto(&t.server.empty_page).await.unwrap().unwrap();
        assert!(response.ok());
    });
}

#[test]
fn ro_013_should_pause_intercepted_fetch_request_until_continue() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<Route>();
        let tx = Arc::new(Mutex::new(Some(tx)));
        t.page
            .route("**/global-var.html", move |route: Route| {
                if let Some(tx) = tx.lock().unwrap().take() {
                    let _ = tx.send(route);
                }
                async { Ok(()) }
            })
            .await
            .unwrap();
        t.page
            .evaluate::<()>(
                "() => {
                    window.__fetchFinished = false;
                    window.__statusPromise = fetch('/global-var.html').then(response => {
                        window.__fetchFinished = true;
                        return response.status;
                    });
                }",
            )
            .await
            .unwrap();
        let route = rx.await.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        let finished: bool = t
            .page
            .evaluate("() => window.__fetchFinished")
            .await
            .unwrap();
        assert!(!finished);
        route.continue_().await.unwrap();
        let status: u16 = t
            .page
            .evaluate("() => window.__statusPromise")
            .await
            .unwrap();
        assert_eq!(status, 200);
    });
}

#[test]
fn ro_014_should_work_with_custom_referer_headers() {
    run(|t| async move {
        let empty = t.server.empty_page.clone();
        t.page
            .set_extra_http_headers([("referer", empty.clone())])
            .await
            .unwrap();
        t.page
            .route("**/*", move |route: Route| {
                let empty = empty.clone();
                async move {
                    assert_eq!(route.request().headers()["referer"], empty);
                    route.continue_().await
                }
            })
            .await
            .unwrap();
        let response = t.page.goto(&t.server.empty_page).await.unwrap().unwrap();
        assert!(response.ok());
    });
}

#[test]
fn ro_015_should_be_abortable() {
    run(|t| async move {
        t.page
            .route(
                regex::Regex::new(r"\.css$").unwrap(),
                |route: Route| async move { route.abort().await },
            )
            .await
            .unwrap();
        let failed = Arc::new(Mutex::new(false));
        let f = failed.clone();
        t.page.on_request_failed(move |request| {
            if request.url().contains(".css") {
                *f.lock().unwrap() = true;
            }
        });
        let response = t
            .page
            .goto(format!("{}/one-style.html", t.server.prefix))
            .await
            .unwrap()
            .unwrap();
        assert!(response.ok());
        assert_eq!(response.request().failure(), None);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(*failed.lock().unwrap());
    });
}

#[test]
fn ro_016_should_be_abortable_with_custom_error_codes() {
    run(|t| async move {
        t.page
            .route("**/*", |route: Route| async move {
                route.abort().error_code("internetdisconnected").await
            })
            .await
            .unwrap();
        let failed = t.page.wait_for_request_failed();
        let _ = t.page.goto(&t.server.empty_page).await;
        let request = failed.await.unwrap();
        assert_eq!(
            request.failure().as_deref(),
            Some("net::ERR_INTERNET_DISCONNECTED")
        );
    });
}

#[test]
fn ro_017_should_not_throw_if_request_was_cancelled_by_the_page() {
    run(|t| async move {
        let (tx, rx) = tokio::sync::oneshot::channel::<Route>();
        let tx = Arc::new(Mutex::new(Some(tx)));
        t.page
            .route("**/data.json", move |route: Route| {
                if let Some(tx) = tx.lock().unwrap().take() {
                    let _ = tx.send(route);
                }
                async { Ok(()) }
            })
            .await
            .unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .evaluate::<()>(
                "url => { globalThis.controller = new AbortController(); fetch(url, { signal: globalThis.controller.signal }).catch(() => {}); }",
            )
            .arg(format!("{}/data.json", t.server.prefix))
            .await
            .unwrap();
        let route = rx.await.unwrap();
        let failure = t.page.wait_for_request_failed();
        t.page
            .evaluate::<()>("() => globalThis.controller.abort()")
            .await
            .unwrap();
        let cancelled = failure.await.unwrap();
        let text = cancelled.failure().unwrap().to_lowercase();
        assert!(
            text.contains("cancelled") || text.contains("aborted"),
            "{text}"
        );
        route.abort().await.unwrap();
    });
}

#[test]
fn ro_018_should_send_referer() {
    run(|t| async move {
        t.page
            .set_extra_http_headers([("referer", "http://google.com/")])
            .await
            .unwrap();
        t.page
            .route(
                "**/*",
                |route: Route| async move { route.continue_().await },
            )
            .await
            .unwrap();
        let request = t.server.wait_for_request("/grid.html");
        t.page
            .goto(format!("{}/grid.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(
            request.await.header("referer").as_deref(),
            Some("http://google.com/")
        );
    });
}

#[test]
fn ro_019_should_fail_navigation_when_aborting_main_resource() {
    run(|t| async move {
        t.page
            .route("**/*", |route: Route| async move { route.abort().await })
            .await
            .unwrap();
        assert_err_contains(t.page.goto(&t.server.empty_page).await, "net::ERR_FAILED");
    });
}

#[test]
fn ro_020_should_not_work_with_redirects() {
    run(|t| async move {
        let intercepted = Arc::new(Mutex::new(Vec::new()));
        let i = intercepted.clone();
        t.page
            .route("**/*", move |route: Route| {
                i.lock().unwrap().push(route.request());
                async move { route.continue_().await }
            })
            .await
            .unwrap();
        t.server
            .set_redirect("/non-existing-page.html", "/non-existing-page-2.html");
        t.server
            .set_redirect("/non-existing-page-2.html", "/non-existing-page-3.html");
        t.server
            .set_redirect("/non-existing-page-3.html", "/non-existing-page-4.html");
        t.server
            .set_redirect("/non-existing-page-4.html", "/empty.html");
        let response = t
            .page
            .goto(format!("{}/non-existing-page.html", t.server.prefix))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), 200);
        assert!(response.url().contains("empty.html"));
        let intercepted = intercepted.lock().unwrap().clone();
        assert_eq!(intercepted.len(), 1);
        assert_eq!(intercepted[0].resource_type(), "document");
        assert!(intercepted[0].is_navigation_request());
        assert!(intercepted[0].url().contains("/non-existing-page.html"));
        let mut chain = Vec::new();
        let mut r = Some(response.request());
        while let Some(req) = r {
            assert!(req.is_navigation_request());
            r = req.redirected_from();
            chain.push(req);
        }
        assert_eq!(chain.len(), 5);
        assert!(chain[0].url().contains("/empty.html"));
        assert!(chain[1].url().contains("/non-existing-page-4.html"));
        assert!(chain[2].url().contains("/non-existing-page-3.html"));
        assert!(chain[3].url().contains("/non-existing-page-2.html"));
        assert!(chain[4].url().contains("/non-existing-page.html"));
        for (i, req) in chain.iter().enumerate() {
            let expected = if i == 0 {
                None
            } else {
                Some(chain[i - 1].clone())
            };
            assert!(req.redirected_to() == expected);
        }
    });
}

#[test]
fn ro_021_should_chain_fallback_with_dynamic_url() {
    run(|t| async move {
        let intercepted = Arc::new(Mutex::new(Vec::new()));
        let i1 = intercepted.clone();
        let empty = t.server.empty_page.clone();
        t.page
            .route("**/bar", move |route: Route| {
                i1.lock().unwrap().push(1);
                let empty = empty.clone();
                async move { route.fallback().url(empty).await }
            })
            .await
            .unwrap();
        let i2 = intercepted.clone();
        t.page
            .route("**/foo", move |route: Route| {
                i2.lock().unwrap().push(2);
                async move { route.fallback().url("http://localhost/bar").await }
            })
            .await
            .unwrap();
        let i3 = intercepted.clone();
        t.page
            .route("**/empty.html", move |route: Route| {
                i3.lock().unwrap().push(3);
                async move { route.fallback().url("http://localhost/foo").await }
            })
            .await
            .unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(*intercepted.lock().unwrap(), vec![3, 2, 1]);
    });
}

#[test]
fn ro_022_should_work_with_redirects_for_subresources() {
    run(|t| async move {
        let intercepted = Arc::new(Mutex::new(Vec::new()));
        let i = intercepted.clone();
        t.page
            .route("**/*", move |route: Route| {
                i.lock().unwrap().push(route.request());
                async move { route.continue_().await }
            })
            .await
            .unwrap();
        t.server.set_redirect("/one-style.css", "/two-style.css");
        t.server.set_redirect("/two-style.css", "/three-style.css");
        t.server.set_redirect("/three-style.css", "/four-style.css");
        t.server.set_content(
            "/four-style.css",
            "text/css",
            "body {box-sizing: border-box; }",
        );
        let response = t
            .page
            .goto(format!("{}/one-style.html", t.server.prefix))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), 200);
        assert!(response.url().contains("one-style.html"));
        let intercepted = intercepted.lock().unwrap().clone();
        assert_eq!(intercepted.len(), 2);
        assert_eq!(intercepted[0].resource_type(), "document");
        assert!(intercepted[0].url().contains("one-style.html"));
        let mut r = Some(intercepted[1].clone());
        for url in [
            "/one-style.css",
            "/two-style.css",
            "/three-style.css",
            "/four-style.css",
        ] {
            let req = r.unwrap();
            assert_eq!(req.resource_type(), "stylesheet");
            assert!(req.url().contains(url));
            r = req.redirected_to();
        }
        assert!(r.is_none());
    });
}

#[test]
fn ro_023_should_work_with_equal_requests() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let count = Arc::new(Mutex::new(1));
        let c = count.clone();
        t.server.set_route("/zzz", move |_| {
            let c = c.clone();
            async move {
                let mut n = c.lock().unwrap();
                let body = (*n * 11).to_string();
                *n += 1;
                HttpResponse::builder().body(Body::from(body)).unwrap()
            }
        });
        let spinner = Arc::new(Mutex::new(false));
        t.page
            .route("**/*", move |route: Route| {
                let abort = {
                    let mut s = spinner.lock().unwrap();
                    let v = *s;
                    *s = !*s;
                    v
                };
                async move {
                    if abort {
                        route.abort().await
                    } else {
                        route.continue_().await
                    }
                }
            })
            .await
            .unwrap();
        let mut results = Vec::new();
        for _ in 0..3 {
            let r: String = t
                .page
                .evaluate(
                    "() => fetch('/zzz').then(response => response.text()).catch(e => 'FAILED')",
                )
                .await
                .unwrap();
            results.push(r);
        }
        assert_eq!(results, vec!["11", "FAILED", "22"]);
    });
}

#[test]
fn ro_024_should_navigate_to_data_url_and_not_fire_data_url_requests() {
    run(|t| async move {
        let requests = Arc::new(Mutex::new(0));
        let r = requests.clone();
        t.page
            .route("**/*", move |route: Route| {
                *r.lock().unwrap() += 1;
                async move { route.continue_().await }
            })
            .await
            .unwrap();
        let response = t.page.goto("data:text/html,<div>yo</div>").await.unwrap();
        assert!(response.is_none());
        assert_eq!(*requests.lock().unwrap(), 0);
    });
}

#[test]
fn ro_025_should_be_able_to_fetch_data_url_and_not_fire_data_url_requests() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let requests = Arc::new(Mutex::new(0));
        let r = requests.clone();
        t.page
            .route("**/*", move |route: Route| {
                *r.lock().unwrap() += 1;
                async move { route.continue_().await }
            })
            .await
            .unwrap();
        let text: String = t
            .page
            .evaluate("url => fetch(url).then(r => r.text())")
            .arg("data:text/html,<div>yo</div>")
            .await
            .unwrap();
        assert_eq!(text, "<div>yo</div>");
        assert_eq!(*requests.lock().unwrap(), 0);
    });
}

#[test]
fn ro_026_should_navigate_to_url_with_hash_and_fire_requests_without_hash() {
    run(|t| async move {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let r = requests.clone();
        t.page
            .route("**/*", move |route: Route| {
                r.lock().unwrap().push(route.request());
                async move { route.continue_().await }
            })
            .await
            .unwrap();
        let response = t
            .page
            .goto(format!("{}#hash", t.server.empty_page))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.url(), t.server.empty_page);
        let requests = requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].url(), t.server.empty_page);
    });
}

#[test]
fn ro_027_should_work_with_encoded_server() {
    run(|t| async move {
        t.page
            .route(
                "**/*",
                |route: Route| async move { route.continue_().await },
            )
            .await
            .unwrap();
        let response = t
            .page
            .goto(format!("{}/some nonexisting page", t.server.prefix))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), 404);
    });
}

#[test]
fn ro_028_should_work_with_badly_encoded_server() {
    run(|t| async move {
        t.server
            .set_content("/malformed?rnd=%911", "text/plain", "");
        t.page
            .route(
                "**/*",
                |route: Route| async move { route.continue_().await },
            )
            .await
            .unwrap();
        let response = t
            .page
            .goto(format!("{}/malformed?rnd=%911", t.server.prefix))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), 200);
    });
}

#[test]
fn ro_031_should_intercept_main_resource_during_cross_process_navigation() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let intercepted = Arc::new(Mutex::new(false));
        let i = intercepted.clone();
        t.page
            .route(
                format!("{}/empty.html", t.server.cross_process_prefix),
                move |route: Route| {
                    *i.lock().unwrap() = true;
                    async move { route.continue_().await }
                },
            )
            .await
            .unwrap();
        let response = t
            .page
            .goto(format!("{}/empty.html", t.server.cross_process_prefix))
            .await
            .unwrap()
            .unwrap();
        assert!(response.ok());
        assert!(*intercepted.lock().unwrap());
    });
}

#[test]
fn ro_032_should_fulfill_with_redirect_status() {
    run(|t| async move {
        t.page
            .goto(format!("{}/title.html", t.server.prefix))
            .await
            .unwrap();
        t.server.set_content("/final", "text/plain", "foo");
        let target = format!("{}/redirect_this", t.server.prefix);
        t.page
            .route("**/*", move |route: Route| {
                let target = target.clone();
                async move {
                    if route.request().url() != target {
                        return route.continue_().await;
                    }
                    route
                        .fulfill()
                        .status(301)
                        .headers([("location", "/final")])
                        .await
                }
            })
            .await
            .unwrap();
        let text: String = t
            .page
            .evaluate("async url => { const data = await fetch(url); return data.text(); }")
            .arg(format!("{}/redirect_this", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(text, "foo");
    });
}

#[test]
fn ro_045_should_support_the_times_parameter_with_route_matching() {
    run(|t| async move {
        let intercepted = Arc::new(Mutex::new(Vec::new()));
        let i = intercepted.clone();
        t.page
            .route("**/empty.html", move |route: Route| {
                i.lock().unwrap().push(1);
                async move { route.continue_().await }
            })
            .times(1)
            .await
            .unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(intercepted.lock().unwrap().len(), 1);
    });
}

#[test]
fn ro_046_should_work_if_handler_with_times_parameter_was_removed_from_another_handler() {
    run(|t| async move {
        let intercepted = Arc::new(Mutex::new(Vec::new()));
        let i = intercepted.clone();
        t.page
            .route("**/*", move |route: Route| {
                i.lock().unwrap().push("first");
                async move { route.continue_().await }
            })
            .times(1)
            .await
            .unwrap();
        let i = intercepted.clone();
        let page = t.page.clone();
        t.page
            .route("**/*", move |route: Route| {
                i.lock().unwrap().push("second");
                let page = page.clone();
                async move {
                    page.unroute("**/*").await?;
                    route.fallback().await
                }
            })
            .await
            .unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(*intercepted.lock().unwrap(), vec!["second"]);
        intercepted.lock().unwrap().clear();
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert!(intercepted.lock().unwrap().is_empty());
    });
}

#[test]
fn ro_047_should_support_async_handler_with_times() {
    run(|t| async move {
        t.page
            .route("**/empty.html", |route: Route| async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                route
                    .fulfill()
                    .body("<html>intercepted</html>")
                    .content_type("text/html")
                    .await
            })
            .times(1)
            .await
            .unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        let text: String = t
            .page
            .evaluate("() => document.body.textContent")
            .await
            .unwrap();
        assert_eq!(text, "intercepted");
        t.page.goto(&t.server.empty_page).await.unwrap();
        let text: String = t
            .page
            .evaluate("() => document.body.textContent")
            .await
            .unwrap();
        assert_eq!(text, "");
    });
}

#[test]
fn ro_049_should_contain_raw_request_header() {
    run(|t| async move {
        let headers = Arc::new(Mutex::new(None));
        let h = headers.clone();
        t.page
            .route("**/*", move |route: Route| {
                let h = h.clone();
                async move {
                    *h.lock().unwrap() = Some(route.request().all_headers().await?);
                    route.continue_().await
                }
            })
            .await
            .unwrap();
        t.page.goto(&t.server.prefix).await.unwrap();
        assert!(
            headers
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .contains_key("accept")
        );
    });
}

#[test]
fn ro_051_should_contain_raw_response_header() {
    run(|t| async move {
        let request = Arc::new(Mutex::new(None));
        let r = request.clone();
        t.page
            .route("**/*", move |route: Route| {
                *r.lock().unwrap() = Some(route.request());
                async move { route.continue_().await }
            })
            .await
            .unwrap();
        t.page.goto(&t.server.prefix).await.unwrap();
        let request = request.lock().unwrap().clone().unwrap();
        let response = request.response().await.unwrap().unwrap();
        let headers = response.all_headers().await.unwrap();
        assert!(headers.contains_key("date"));
    });
}

#[test]
fn ro_052_should_contain_raw_response_header_after_fulfill() {
    run(|t| async move {
        t.page
            .route("**/*", |route: Route| async move {
                route
                    .fulfill()
                    .status(200)
                    .body("Hello")
                    .content_type("text/html")
                    .await
            })
            .await
            .unwrap();
        let response = t.wait_and_goto().await;
        let headers = response.all_headers().await.unwrap();
        assert_eq!(headers["content-type"], "text/html");
    });
}

#[test]
fn unroute_all_with_wait_behavior_waits_for_running_handlers() {
    run(|t| async move {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let tx = Arc::new(Mutex::new(Some(tx)));
        let gate = Arc::new(tokio::sync::Notify::new());
        let g = gate.clone();
        t.page
            .route("**/*", move |route: Route| {
                if let Some(tx) = tx.lock().unwrap().take() {
                    let _ = tx.send(());
                }
                let g = g.clone();
                async move {
                    g.notified().await;
                    route.fallback().await
                }
            })
            .await
            .unwrap();
        let page = t.page.clone();
        let empty = t.server.empty_page.clone();
        let nav = tokio::spawn(async move { page.goto(empty).await });
        rx.await.unwrap();
        let unroute = tokio::spawn({
            let page = t.page.clone();
            async move {
                page.unroute_all()
                    .behavior(oxidewright::UnrouteBehavior::Wait)
                    .await
            }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!unroute.is_finished());
        gate.notify_one();
        unroute.await.unwrap().unwrap();
        nav.await.unwrap().unwrap();
    });
}

#[test]
fn wait_for_event_close_resolves_on_page_close() {
    run(|t| async move {
        let page = t.context.new_page().await.unwrap();
        let close = page.wait_for_event(EventKind::Close);
        page.close().await.unwrap();
        close.await.unwrap();
    });
}

impl crate::common::TestContext {
    async fn wait_and_goto(&self) -> oxidewright::Response {
        self.page
            .goto(&self.server.empty_page)
            .await
            .unwrap()
            .unwrap()
    }
}
