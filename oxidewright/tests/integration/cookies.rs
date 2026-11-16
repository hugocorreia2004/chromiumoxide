//! Cookies, ported from Playwright's `tests/library/browsercontext-cookies.spec.ts`,
//! `browsercontext-add-cookies.spec.ts` and `browsercontext-clearcookies.spec.ts`.
//!
//! Not ported:
//! * CO-004, CO-014, CO-016..CO-018, CO-020..CO-022, CO-036, CO-038, CO-041:
//!   need `page.route` (another module), HTTPS servers or WebKit-only APIs.
//! * CO-049..CO-051: cookieStore events / partitioned cookies need HTTPS.

use oxidewright::{Cookie, SameSite, SetCookie};
use regex::Regex;
use serde_json::json;

use crate::common::*;

fn hostname(prefix: &str) -> String {
    url::Url::parse(prefix)
        .unwrap()
        .host_str()
        .unwrap()
        .to_string()
}

async fn document_cookie(page: &oxidewright::Page) -> String {
    page.evaluate("() => document.cookie").await.unwrap()
}

#[test]
fn co_001_should_return_no_cookies_in_pristine_browser_context() {
    run(|t| async move {
        assert_eq!(t.context.cookies().await.unwrap(), Vec::<Cookie>::new());
    });
}

#[test]
fn co_002_should_get_a_cookie() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let cookie: String = t
            .page
            .evaluate("() => { document.cookie = 'username=John Doe'; return document.cookie; }")
            .await
            .unwrap();
        assert_eq!(cookie, "username=John Doe");
        let cookies = t.context.cookies().await.unwrap();
        assert_eq!(
            serde_json::to_value(&cookies).unwrap(),
            json!([{
                "name": "username",
                "value": "John Doe",
                "domain": "localhost",
                "path": "/",
                "expires": -1,
                "httpOnly": false,
                "secure": false,
                "sameSite": "Lax",
            }])
        );
    });
}

#[test]
fn co_003_should_get_a_non_session_cookie() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let cookie: String = t
            .page
            .evaluate("() => { const date = new Date('1/1/2038'); document.cookie = `username=John Doe;expires=${date.toUTCString()}`; return document.cookie; }")
            .await
            .unwrap();
        assert_eq!(cookie, "username=John Doe");
        let cookies = t.context.cookies().await.unwrap();
        assert_eq!(cookies.len(), 1);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let four_hundred_days = 60.0 * 60.0 * 24.0 * 400.0;
        assert!(cookies[0].expires > now + four_hundred_days - 300.0);
        assert!(!cookies[0].http_only);
        assert_eq!(cookies[0].same_site, SameSite::Lax);
    });
}

#[test]
fn co_005_should_properly_report_http_only_cookie() {
    run(|t| async move {
        t.server.set_route("/empty.html", |_| async {
            axum::response::Response::builder()
                .header("Set-Cookie", "name=value;HttpOnly; Path=/")
                .body(axum::body::Body::empty())
                .unwrap()
        });
        t.page.goto(&t.server.empty_page).await.unwrap();
        let cookies = t.context.cookies().await.unwrap();
        assert_eq!(cookies.len(), 1);
        assert!(cookies[0].http_only);
    });
}

#[test]
fn co_006_should_properly_report_strict_same_site_cookie() {
    run(|t| async move {
        t.server.set_route("/empty.html", |_| async {
            axum::response::Response::builder()
                .header("Set-Cookie", "name=value;SameSite=Strict")
                .body(axum::body::Body::empty())
                .unwrap()
        });
        t.page.goto(&t.server.empty_page).await.unwrap();
        let cookies = t.context.cookies().await.unwrap();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].same_site, SameSite::Strict);
    });
}

#[test]
fn co_007_should_properly_report_lax_same_site_cookie() {
    run(|t| async move {
        t.server.set_route("/empty.html", |_| async {
            axum::response::Response::builder()
                .header("Set-Cookie", "name=value;SameSite=Lax")
                .body(axum::body::Body::empty())
                .unwrap()
        });
        t.page.goto(&t.server.empty_page).await.unwrap();
        let cookies = t.context.cookies().await.unwrap();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].same_site, SameSite::Lax);
    });
}

#[test]
fn co_008_should_get_multiple_cookies() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let cookie: String = t
            .page
            .evaluate("() => { document.cookie = 'username=John Doe'; document.cookie = 'password=1234'; return document.cookie.split('; ').sort().join('; '); }")
            .await
            .unwrap();
        assert_eq!(cookie, "password=1234; username=John Doe");
        let mut names: Vec<String> = t
            .context
            .cookies()
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        names.sort();
        assert_eq!(names, ["password", "username"]);
    });
}

#[test]
fn co_009_should_get_cookies_from_multiple_urls() {
    run(|t| async move {
        t.context
            .add_cookies([
                SetCookie::new("doggo", "woofs")
                    .url("https://foo.com")
                    .same_site(SameSite::None),
                SetCookie::new("catto", "purrs")
                    .url("https://bar.com")
                    .same_site(SameSite::Lax),
                SetCookie::new("birdo", "tweets")
                    .url("https://baz.com")
                    .same_site(SameSite::Lax),
            ])
            .await
            .unwrap();
        let mut cookies = t
            .context
            .cookies()
            .urls(["https://foo.com", "https://baz.com"])
            .await
            .unwrap();
        cookies.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(cookies.len(), 2);
        assert_eq!(cookies[0].name, "birdo");
        assert_eq!(cookies[0].domain, "baz.com");
        assert!(cookies[0].secure);
        assert_eq!(cookies[0].same_site, SameSite::Lax);
        assert_eq!(cookies[1].name, "doggo");
        assert_eq!(cookies[1].same_site, SameSite::None);
    });
}

#[test]
fn co_010_should_work_with_subdomain_cookie() {
    run(|t| async move {
        t.context
            .add_cookies([SetCookie::new("doggo", "woofs")
                .domain(".foo.com")
                .path("/")
                .same_site(SameSite::Lax)
                .secure(true)])
            .await
            .unwrap();
        for url in ["https://foo.com", "https://sub.foo.com"] {
            let cookies = t.context.cookies().url(url).await.unwrap();
            assert_eq!(cookies.len(), 1, "{url}");
            assert_eq!(cookies[0].domain, ".foo.com");
            assert!(cookies[0].secure);
        }
    });
}

#[test]
fn co_011_should_only_return_cookies_that_apply_to_the_url() {
    run(|t| async move {
        t.context
            .add_cookies([
                SetCookie::new("hostonly", "v")
                    .domain("foo.com")
                    .path("/api"),
                SetCookie::new("subdomains", "v")
                    .domain(".foo.com")
                    .path("/api"),
                SetCookie::new("slash", "v").domain("foo.com").path("/foo/"),
            ])
            .await
            .unwrap();
        let names = |url: &'static str| {
            let context = t.context.clone();
            async move {
                let mut names: Vec<String> = context
                    .cookies()
                    .url(url)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|c| c.name)
                    .collect();
                names.sort();
                names
            }
        };
        assert_eq!(
            names("http://foo.com/api").await,
            ["hostonly", "subdomains"]
        );
        assert_eq!(
            names("http://foo.com/api/x").await,
            ["hostonly", "subdomains"]
        );
        assert!(names("http://foo.com/apiv2").await.is_empty());
        assert_eq!(names("http://sub.foo.com/api").await, ["subdomains"]);
        assert!(names("http://notfoo.com/api").await.is_empty());
        assert!(names("http://foo.com/foo").await.is_empty());
        assert_eq!(names("http://foo.com/foo/").await, ["slash"]);
        assert_eq!(names("http://foo.com/foo/bar").await, ["slash"]);
    });
}

#[test]
fn co_012_should_return_cookies_with_empty_value() {
    run(|t| async move {
        t.server.set_route("/empty.html", |_| async {
            axum::response::Response::builder()
                .header("Set-Cookie", "name=;Path=/")
                .body(axum::body::Body::empty())
                .unwrap()
        });
        t.page.goto(&t.server.empty_page).await.unwrap();
        let cookies = t.context.cookies().await.unwrap();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].name, "name");
        assert_eq!(cookies[0].value, "");
    });
}

#[test]
fn co_013_should_return_secure_cookies_based_on_http_s_protocol() {
    run(|t| async move {
        t.context
            .add_cookies([
                SetCookie::new("doggo", "woofs")
                    .url("https://foo.com")
                    .same_site(SameSite::Lax),
                SetCookie::new("catto", "purrs")
                    .url("http://foo.com")
                    .same_site(SameSite::Lax),
            ])
            .await
            .unwrap();
        let http = t.context.cookies().url("http://foo.com").await.unwrap();
        assert_eq!(http.len(), 1);
        assert_eq!(http[0].name, "catto");
        let https = t.context.cookies().url("https://foo.com").await.unwrap();
        assert_eq!(https.len(), 2);
    });
}

#[test]
fn co_015_should_add_cookies_with_an_expiration() {
    run(|t| async move {
        // Chromium caps expiration at 400 days from now.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let expires = (now + 100.0 * 86_400.0).floor();
        t.context
            .add_cookies([SetCookie::new("username", "John Doe")
                .url("https://foo.com")
                .expires(expires)])
            .await
            .unwrap();
        let cookies = t.context.cookies().await.unwrap();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].expires.floor(), expires);
        assert_err_contains(
            t.context
                .add_cookies([SetCookie::new("a", "b")
                    .url("https://foo.com")
                    .expires(-42.0)])
                .await,
            "Cookie should have a valid expires",
        );
    });
}

#[test]
fn co_019_should_work() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.context
            .add_cookies([SetCookie::new("password", "123456").url(&t.server.empty_page)])
            .await
            .unwrap();
        assert_eq!(document_cookie(&t.page).await, "password=123456");
    });
}

#[test]
fn co_023_should_roundtrip_cookie() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .evaluate::<()>("() => { const date = new Date('1/1/2038'); document.cookie = `username=John Doe;expires=${date.toUTCString()}`; }")
            .await
            .unwrap();
        let cookies = t.context.cookies().await.unwrap();
        t.context.clear_cookies().await.unwrap();
        assert!(t.context.cookies().await.unwrap().is_empty());
        t.context.add_cookies(cookies.clone()).await.unwrap();
        let normalize = |cookies: Vec<Cookie>| {
            cookies
                .into_iter()
                .map(|mut c| {
                    c.expires = c.expires.floor();
                    c
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            normalize(t.context.cookies().await.unwrap()),
            normalize(cookies)
        );
    });
}

#[test]
fn co_024_should_send_cookie_header() {
    run(|t| async move {
        t.context
            .add_cookies([SetCookie::new("cookie", "value").url(&t.server.empty_page)])
            .await
            .unwrap();
        let page = t.context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let headers = t.server.request_headers("/empty.html").unwrap();
        assert_eq!(headers.get("cookie").unwrap(), "cookie=value");
    });
}

#[test]
fn co_025_should_isolate_cookies_in_browser_contexts() {
    run(|t| async move {
        let another = t.browser.new_context().await.unwrap();
        t.context
            .add_cookies([SetCookie::new("isolatecookie", "page1value").url(&t.server.empty_page)])
            .await
            .unwrap();
        another
            .add_cookies([SetCookie::new("isolatecookie", "page2value").url(&t.server.empty_page)])
            .await
            .unwrap();
        let cookies1 = t.context.cookies().await.unwrap();
        let cookies2 = another.cookies().await.unwrap();
        assert_eq!(cookies1.len(), 1);
        assert_eq!(cookies2.len(), 1);
        assert_eq!(cookies1[0].value, "page1value");
        assert_eq!(cookies2[0].value, "page2value");
        another.close().await.unwrap();
    });
}

#[test]
fn co_026_should_isolate_session_cookies() {
    run(|t| async move {
        t.server.set_route("/setcookie.html", |_| async {
            axum::response::Response::builder()
                .header("Set-Cookie", "session=value")
                .body(axum::body::Body::empty())
                .unwrap()
        });
        t.page
            .goto(format!("{}/setcookie.html", t.server.prefix))
            .await
            .unwrap();
        let page = t.context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(t.context.cookies().await.unwrap().len(), 1);
        let another = t.browser.new_context().await.unwrap();
        let page = another.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        assert!(another.cookies().await.unwrap().is_empty());
        another.close().await.unwrap();
    });
}

#[test]
fn co_030_should_set_multiple_cookies() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.context
            .add_cookies([
                SetCookie::new("multiple-1", "123456").url(&t.server.empty_page),
                SetCookie::new("multiple-2", "bar").url(&t.server.empty_page),
            ])
            .await
            .unwrap();
        let cookies: Vec<String> = t
            .page
            .evaluate("() => { const cookies = document.cookie.split(';'); return cookies.map(cookie => cookie.trim()).sort(); }")
            .await
            .unwrap();
        assert_eq!(cookies, ["multiple-1=123456", "multiple-2=bar"]);
    });
}

#[test]
fn co_031_should_have_expires_set_to_minus_1_for_session_cookies() {
    run(|t| async move {
        t.context
            .add_cookies([SetCookie::new("expires", "123456").url(&t.server.empty_page)])
            .await
            .unwrap();
        assert_eq!(t.context.cookies().await.unwrap()[0].expires, -1.0);
    });
}

#[test]
fn co_032_should_set_cookie_with_reasonable_defaults() {
    run(|t| async move {
        t.context
            .add_cookies([SetCookie::new("defaults", "123456").url(&t.server.empty_page)])
            .await
            .unwrap();
        let cookies = t.context.cookies().await.unwrap();
        assert_eq!(
            serde_json::to_value(&cookies).unwrap(),
            json!([{
                "name": "defaults",
                "value": "123456",
                "domain": "localhost",
                "path": "/",
                "expires": -1,
                "httpOnly": false,
                "secure": false,
                "sameSite": "Lax",
            }])
        );
    });
}

#[test]
fn co_033_should_set_a_cookie_with_a_path() {
    run(|t| async move {
        let page = &t.page;
        page.goto(format!("{}/grid.html", t.server.prefix))
            .await
            .unwrap();
        t.context
            .add_cookies([SetCookie::new("gridcookie", "GRID")
                .domain("localhost")
                .path("/grid.html")
                .same_site(SameSite::Lax)])
            .await
            .unwrap();
        let cookies = t.context.cookies().await.unwrap();
        assert_eq!(cookies[0].path, "/grid.html");
        assert_eq!(document_cookie(page).await, "gridcookie=GRID");
        page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(document_cookie(page).await, "");
        page.goto(format!("{}/grid.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(document_cookie(page).await, "gridcookie=GRID");
    });
}

#[test]
fn co_034_should_not_set_a_cookie_with_blank_page_url() {
    run(|t| async move {
        assert_err_contains(
            t.context
                .add_cookies([
                    SetCookie::new("example-cookie", "best").url(&t.server.empty_page),
                    SetCookie::new("example-cookie-blank", "best").url("about:blank"),
                ])
                .await,
            "Blank page can not have cookie \"example-cookie-blank\"",
        );
    });
}

#[test]
fn co_035_should_not_set_a_cookie_on_a_data_url_page() {
    run(|t| async move {
        assert_err_contains(
            t.context
                .add_cookies([
                    SetCookie::new("example-cookie", "best").url("data:,Hello%2C%20World!")
                ])
                .await,
            "Data URL page can not have cookie \"example-cookie\"",
        );
    });
}

#[test]
fn co_037_should_be_able_to_set_unsecure_cookie_for_http_website() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.context
            .add_cookies([SetCookie::new("foo", "bar").url(&t.server.empty_page)])
            .await
            .unwrap();
        let cookies = t.context.cookies().url(&t.server.empty_page).await.unwrap();
        assert!(!cookies[0].secure);
    });
}

#[test]
fn co_039_should_set_cookies_for_a_frame() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.context
            .add_cookies([SetCookie::new("frame-cookie", "value").url(&t.server.prefix)])
            .await
            .unwrap();
        let value: String = t
            .page
            .evaluate(
                "src => { let fulfill; const promise = new Promise(x => fulfill = x); const iframe = document.createElement('iframe'); document.body.appendChild(iframe); iframe.onload = fulfill; iframe.src = src; return promise.then(() => iframe.contentDocument.cookie); }",
            )
            .arg(format!("{}/grid.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(value, "frame-cookie=value");
    });
}

#[test]
fn co_042_should_clear_cookies() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.context
            .add_cookies([SetCookie::new("cookie1", "1").url(&t.server.empty_page)])
            .await
            .unwrap();
        assert_eq!(document_cookie(&t.page).await, "cookie1=1");
        t.context.clear_cookies().await.unwrap();
        assert!(t.context.cookies().await.unwrap().is_empty());
        t.page.reload().await.unwrap();
        assert_eq!(document_cookie(&t.page).await, "");
    });
}

#[test]
fn co_043_should_isolate_cookies_when_clearing() {
    run(|t| async move {
        let another = t.browser.new_context().await.unwrap();
        t.context
            .add_cookies([SetCookie::new("page1cookie", "page1value").url(&t.server.empty_page)])
            .await
            .unwrap();
        another
            .add_cookies([SetCookie::new("page2cookie", "page2value").url(&t.server.empty_page)])
            .await
            .unwrap();
        assert_eq!(t.context.cookies().await.unwrap().len(), 1);
        assert_eq!(another.cookies().await.unwrap().len(), 1);
        t.context.clear_cookies().await.unwrap();
        assert_eq!(t.context.cookies().await.unwrap().len(), 0);
        assert_eq!(another.cookies().await.unwrap().len(), 1);
        another.clear_cookies().await.unwrap();
        assert_eq!(another.cookies().await.unwrap().len(), 0);
        another.close().await.unwrap();
    });
}

async fn add_two(t: &TestContext, name2: &str, domain2: &str, path1: &str, path2: &str) {
    let host = hostname(&t.server.prefix);
    t.context
        .add_cookies([
            SetCookie::new("cookie1", "1").domain(&host).path(path1),
            SetCookie::new(name2, "2").domain(domain2).path(path2),
        ])
        .await
        .unwrap();
}

#[test]
fn co_044_should_remove_cookies_by_name() {
    run(|t| async move {
        add_two(&t, "cookie2", "localhost", "/", "/").await;
        t.page.goto(&t.server.prefix).await.unwrap();
        assert_eq!(document_cookie(&t.page).await, "cookie1=1; cookie2=2");
        t.context.clear_cookies().name("cookie1").await.unwrap();
        assert_eq!(document_cookie(&t.page).await, "cookie2=2");
    });
}

#[test]
fn co_045_should_remove_cookies_by_name_regex() {
    run(|t| async move {
        add_two(&t, "cookie2", "localhost", "/", "/").await;
        t.page.goto(&t.server.prefix).await.unwrap();
        assert_eq!(document_cookie(&t.page).await, "cookie1=1; cookie2=2");
        t.context
            .clear_cookies()
            .name(Regex::new("coo.*1").unwrap())
            .await
            .unwrap();
        assert_eq!(document_cookie(&t.page).await, "cookie2=2");
    });
}

#[test]
fn co_046_should_remove_cookies_by_domain() {
    run(|t| async move {
        let cross = hostname(&t.server.cross_process_prefix);
        add_two(&t, "cookie2", &cross, "/", "/").await;
        t.page.goto(&t.server.prefix).await.unwrap();
        assert_eq!(document_cookie(&t.page).await, "cookie1=1");
        t.page.goto(&t.server.cross_process_prefix).await.unwrap();
        assert_eq!(document_cookie(&t.page).await, "cookie2=2");
        t.context.clear_cookies().domain(cross).await.unwrap();
        assert_eq!(document_cookie(&t.page).await, "");
        t.page.goto(&t.server.prefix).await.unwrap();
        assert_eq!(document_cookie(&t.page).await, "cookie1=1");
    });
}

#[test]
fn co_047_should_remove_cookies_by_path() {
    run(|t| async move {
        let host = hostname(&t.server.prefix);
        t.context
            .add_cookies([
                SetCookie::new("cookie1", "1").domain(&host).path("/api/v1"),
                SetCookie::new("cookie2", "2").domain(&host).path("/api/v2"),
                SetCookie::new("cookie3", "3").domain(&host).path("/"),
            ])
            .await
            .unwrap();
        let page = &t.page;
        page.goto(format!("{}/api/v1", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(document_cookie(page).await, "cookie1=1; cookie3=3");
        t.context.clear_cookies().path("/api/v1").await.unwrap();
        assert_eq!(document_cookie(page).await, "cookie3=3");
        page.goto(format!("{}/api/v2", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(document_cookie(page).await, "cookie2=2; cookie3=3");
        page.goto(format!("{}/", t.server.prefix)).await.unwrap();
        assert_eq!(document_cookie(page).await, "cookie3=3");
    });
}

#[test]
fn co_048_should_remove_cookies_by_name_and_domain() {
    run(|t| async move {
        let host = hostname(&t.server.prefix);
        let cross = hostname(&t.server.cross_process_prefix);
        t.context
            .add_cookies([
                SetCookie::new("cookie1", "1").domain(&host).path("/"),
                SetCookie::new("cookie1", "1").domain(&cross).path("/"),
            ])
            .await
            .unwrap();
        t.page.goto(&t.server.prefix).await.unwrap();
        assert_eq!(document_cookie(&t.page).await, "cookie1=1");
        t.context
            .clear_cookies()
            .name("cookie1")
            .domain(host)
            .await
            .unwrap();
        assert_eq!(document_cookie(&t.page).await, "");
        t.page.goto(&t.server.cross_process_prefix).await.unwrap();
        assert_eq!(document_cookie(&t.page).await, "cookie1=1");
    });
}
