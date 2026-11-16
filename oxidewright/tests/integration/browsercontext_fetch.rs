//! Ported from Playwright's `tests/library/browsercontext-fetch.spec.ts`.
//!
//! Skipped:
//! * FE-090/091 (timing), FE-117 (raw header case): `timing()` is not
//!   implemented and hyper reports lowercase header names.
//! * FE-092..095, FE-177 (socket-level failures): need raw socket control
//!   the axum test server does not offer.
//! * FE-096/097/104/105 (`__testHookLookup` for custom host names): no DNS
//!   test hook.
//! * FE-103/138/139, FE-170 (HTTPS server): the test server has no TLS.
//! * FE-106 (non-ASCII `Location` sent as raw bytes): axum refuses such
//!   headers.
//! * FE-111/116/171/173 (browser-specific cookie ordering/policies).
//! * FE-118..123 (context `httpCredentials` / `setHTTPCredentials`): context
//!   HTTP credentials are not part of this crate's context options yet.
//! * FE-135 (context `extraHTTPHeaders`): not a context option yet.
//! * FE-141..147, FE-151 (compressed bodies): decompression is done by
//!   `reqwest`; covered by the empty-body test of `global_fetch`.
//! * FE-154 (`fetch(request)`): fetching a `Request` object is not supported.
//! * FE-160/161/162 (`ReadStream`/`FormData` values): use
//!   `multipart_file`/repeated `multipart_field` instead.
//! * FE-164 (unsupported data type): prevented by the type system.
//! * FE-169 (connectOverCDP): covered elsewhere.

use std::future::IntoFuture;
use std::time::Duration;

use axum::body::Body;
use axum::http::header;
use axum::response::Response;
use oxidewright::SetCookie;
use serde_json::json;

use crate::common::*;
use crate::fetch_util::*;

#[test]
fn fe_088_get_should_work() {
    run(|t| async move {
        let response = t
            .context
            .request()
            .get(format!("{}/simple.json", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(response.url(), format!("{}/simple.json", t.server.prefix));
        assert_eq!(response.status(), 200);
        assert_eq!(response.status_text(), "OK");
        assert!(response.ok());
        assert_eq!(response.headers()["content-type"], "application/json");
        assert_eq!(response.text().await.unwrap(), "{\"foo\": \"bar\"}\n");
    });
}

#[test]
fn fe_089_fetch_should_work() {
    run(|t| async move {
        let response = t
            .context
            .request()
            .fetch(format!("{}/simple.json", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let json: serde_json::Value = response.json().await.unwrap();
        assert_eq!(json, json!({ "foo": "bar" }));
    });
}

#[test]
fn fe_098_should_support_params_passed_as_object() {
    run(|t| async move {
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        let response = t
            .context
            .request()
            .get(&t.server.empty_page)
            .param("param1", "value1")
            .param("парам2", "знач2")
            .await
            .unwrap();
        let req = seen.recv().await.unwrap();
        let url = url::Url::parse(&format!("http://localhost{}", req.uri)).unwrap();
        let params: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        assert_eq!(
            params,
            vec![
                ("param1".to_string(), "value1".to_string()),
                ("парам2".to_string(), "знач2".to_string())
            ]
        );
        let response_url = url::Url::parse(response.url()).unwrap();
        assert_eq!(response_url.query_pairs().count(), 2);
    });
}

#[test]
fn fe_100_should_support_params_passed_as_string() {
    run(|t| async move {
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        t.context
            .request()
            .post(&t.server.empty_page)
            .encoded_params("?param1=value1&param1=value2&парам2=знач2")
            .await
            .unwrap();
        let req = seen.recv().await.unwrap();
        let url = url::Url::parse(&format!("http://localhost{}", req.uri)).unwrap();
        let values: Vec<String> = url
            .query_pairs()
            .filter(|(k, _)| k == "param1")
            .map(|(_, v)| v.into_owned())
            .collect();
        assert_eq!(values, vec!["value1", "value2"]);
        assert!(
            url.query_pairs()
                .any(|(k, v)| k == "парам2" && v == "знач2")
        );
    });
}

#[test]
fn fe_101_should_append_string_params_to_the_url_query() {
    run(|t| async move {
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        for params in ["q=cat", "?q=cat"] {
            t.context
                .request()
                .get(format!("{}?page=2", t.server.empty_page))
                .encoded_params(params)
                .await
                .unwrap();
            assert_eq!(seen.recv().await.unwrap().uri, "/empty.html?page=2&q=cat");
        }
        t.context
            .request()
            .get(format!("{}?page=2", t.server.empty_page))
            .param("q", "cat")
            .await
            .unwrap();
        assert_eq!(seen.recv().await.unwrap().uri, "/empty.html?page=2&q=cat");
    });
}

#[test]
fn fe_102_should_support_fail_on_status_code() {
    run(|t| async move {
        let request = t.context.request();
        for method in ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"] {
            let result = request
                .fetch(format!("{}/does-not-exist.html", t.server.prefix))
                .method(method)
                .fail_on_status_code(true)
                .await;
            let err = assert_err_contains(result, "404 Not Found");
            if method != "HEAD" {
                assert!(err.to_string().contains("Response text:\nFile not found:"));
            }
        }
    });
}

#[test]
fn fe_107_should_add_cookies_from_set_cookie_header() {
    run(|t| async move {
        t.server.set_route("/setcookie.html", |_| async {
            Response::builder()
                .header(header::SET_COOKIE, "session=value")
                .header(header::SET_COOKIE, "foo=bar; max-age=3600")
                .body(Body::empty())
                .unwrap()
        });
        t.context
            .request()
            .get(format!("{}/setcookie.html", t.server.prefix))
            .await
            .unwrap();
        let mut names: Vec<_> = t
            .context
            .cookies()
            .await
            .unwrap()
            .into_iter()
            .map(|c| format!("{}={}", c.name, c.value))
            .collect();
        names.sort();
        assert_eq!(names, vec!["foo=bar", "session=value"]);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let cookie: String = t.page.evaluate("() => document.cookie").await.unwrap();
        let mut cookies: Vec<_> = cookie.split(';').map(|s| s.trim().to_string()).collect();
        cookies.sort();
        assert_eq!(cookies, vec!["foo=bar", "session=value"]);
    });
}

#[test]
fn fe_108_page_request_add_cookies_should_add_cookies_to_the_browser_context() {
    run(|t| async move {
        let expires = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            + 3600.0;
        t.page
            .request()
            .add_cookies([
                SetCookie::new("a", "b").url(&t.server.empty_page),
                SetCookie::new("c", "d")
                    .domain("localhost")
                    .path("/")
                    .expires(expires),
            ])
            .await
            .unwrap();
        let mut names: Vec<_> = t
            .context
            .cookies()
            .await
            .unwrap()
            .into_iter()
            .map(|c| format!("{}={}", c.name, c.value))
            .collect();
        names.sort();
        assert_eq!(names, vec!["a=b", "c=d"]);
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        t.context.request().get(&t.server.empty_page).await.unwrap();
        let header = seen.recv().await.unwrap().header("cookie").unwrap();
        let mut sent: Vec<_> = header.split(';').map(|s| s.trim().to_string()).collect();
        sent.sort();
        assert_eq!(sent, vec!["a=b", "c=d"]);
    });
}

#[test]
fn fe_109_page_request_cookies_should_return_browser_context_cookies() {
    run(|t| async move {
        t.context
            .add_cookies([
                SetCookie::new("a", "b").url(&t.server.empty_page),
                SetCookie::new("c", "d").domain("example.com").path("/"),
            ])
            .await
            .unwrap();
        let mut names: Vec<_> = t
            .page
            .request()
            .cookies()
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        names.sort();
        assert_eq!(names, vec!["a", "c"]);
        let filtered = t
            .page
            .request()
            .cookies()
            .url(&t.server.empty_page)
            .await
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "a");
        assert_eq!(
            filtered,
            t.context.cookies().url(&t.server.empty_page).await.unwrap()
        );
    });
}

#[test]
fn fe_110_page_request_clear_cookies_should_clear_browser_context_cookies() {
    run(|t| async move {
        t.context
            .add_cookies([
                SetCookie::new("a", "b").url(&t.server.empty_page),
                SetCookie::new("c", "d").url(&t.server.empty_page),
            ])
            .await
            .unwrap();
        t.page.request().clear_cookies().name("a").await.unwrap();
        let names: Vec<_> = t
            .context
            .cookies()
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, vec!["c"]);
        t.page.request().clear_cookies().await.unwrap();
        assert!(t.context.cookies().await.unwrap().is_empty());
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        t.page.request().get(&t.server.empty_page).await.unwrap();
        assert_eq!(seen.recv().await.unwrap().header("cookie"), None);
    });
}

#[test]
fn fe_112_should_support_cookie_with_empty_value() {
    run(|t| async move {
        t.server.set_route("/setcookie.html", |_| async {
            Response::builder()
                .header(header::SET_COOKIE, "name=")
                .body(Body::empty())
                .unwrap()
        });
        t.context
            .request()
            .get(format!("{}/setcookie.html", t.server.prefix))
            .await
            .unwrap();
        let cookies = t.context.cookies().await.unwrap();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].name, "name");
        assert_eq!(cookies[0].value, "");
    });
}

#[test]
fn fe_113_should_not_lose_body_while_handling_set_cookie_header() {
    run(|t| async move {
        t.server.set_route("/setcookie.html", |_| async {
            Response::builder()
                .header(header::SET_COOKIE, "session=value")
                .body(Body::from("text content"))
                .unwrap()
        });
        let response = t
            .context
            .request()
            .get(format!("{}/setcookie.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(response.text().await.unwrap(), "text content");
    });
}

#[test]
fn fe_114_should_remove_cookie_with_negative_max_age() {
    run(|t| async move {
        t.server.set_route("/setcookie.html", |_| async {
            Response::builder()
                .header(header::SET_COOKIE, "a=v")
                .header(header::SET_COOKIE, "b=v")
                .body(Body::empty())
                .unwrap()
        });
        t.server.set_route("/removecookie.html", |_| async {
            Response::builder()
                .header(header::SET_COOKIE, "a=v; max-age=-1")
                .body(Body::empty())
                .unwrap()
        });
        let request = t.page.request();
        request
            .get(format!("{}/setcookie.html", t.server.prefix))
            .await
            .unwrap();
        request
            .get(format!("{}/removecookie.html", t.server.prefix))
            .await
            .unwrap();
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        request.get(&t.server.empty_page).await.unwrap();
        assert_eq!(
            seen.recv().await.unwrap().header("cookie").as_deref(),
            Some("b=v")
        );
    });
}

#[test]
fn fe_115_should_remove_cookie_with_expires_far_in_the_past() {
    run(|t| async move {
        t.server.set_route("/setcookie.html", |_| async {
            Response::builder()
                .header(header::SET_COOKIE, "a=v")
                .header(header::SET_COOKIE, "b=v")
                .body(Body::empty())
                .unwrap()
        });
        t.server.set_route("/removecookie.html", |_| async {
            Response::builder()
                .header(
                    header::SET_COOKIE,
                    "a=v; expires=Wed, 01 Jan 1000 00:00:00 GMT",
                )
                .body(Body::empty())
                .unwrap()
        });
        let request = t.page.request();
        request
            .get(format!("{}/setcookie.html", t.server.prefix))
            .await
            .unwrap();
        request
            .get(format!("{}/removecookie.html", t.server.prefix))
            .await
            .unwrap();
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        request.get(&t.server.empty_page).await.unwrap();
        assert_eq!(
            seen.recv().await.unwrap().header("cookie").as_deref(),
            Some("b=v")
        );
    });
}

#[test]
fn fe_124_to_129_methods_should_support_post_data() {
    run(|t| async move {
        let request = t.context.request();
        let mut seen = capture(&t.server, "/simple.json", 200, "{}");
        for method in ["DELETE", "GET", "HEAD", "PATCH", "POST", "PUT"] {
            let response = request
                .fetch(format!("{}/simple.json", t.server.prefix))
                .method(method)
                .data("My request")
                .await
                .unwrap();
            let req = seen.recv().await.unwrap();
            assert_eq!(req.method, method);
            assert_eq!(req.body, b"My request");
            assert_eq!(response.status(), 200);
        }
    });
}

#[test]
fn fe_130_should_add_default_headers() {
    run(|t| async move {
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        t.context.request().get(&t.server.empty_page).await.unwrap();
        let req = seen.recv().await.unwrap();
        assert_eq!(req.header("accept").as_deref(), Some("*/*"));
        let user_agent: String = t.page.evaluate("() => navigator.userAgent").await.unwrap();
        assert_eq!(req.header("user-agent"), Some(user_agent));
        assert_eq!(
            req.header("accept-encoding").as_deref(),
            Some("gzip,deflate,br")
        );
    });
}

#[test]
fn fe_131_should_send_content_length() {
    run(|t| async move {
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        let bytes: Vec<u8> = (0..=255).collect();
        t.context
            .request()
            .post(&t.server.empty_page)
            .data(bytes)
            .await
            .unwrap();
        let req = seen.recv().await.unwrap();
        assert_eq!(req.header("content-length").as_deref(), Some("256"));
        assert_eq!(
            req.header("content-type").as_deref(),
            Some("application/octet-stream")
        );
    });
}

#[test]
fn fe_132_should_add_default_headers_to_redirects() {
    run(|t| async move {
        t.server.set_redirect("/redirect", "/empty.html");
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        t.context
            .request()
            .get(format!("{}/redirect", t.server.prefix))
            .await
            .unwrap();
        let req = seen.recv().await.unwrap();
        assert_eq!(req.header("accept").as_deref(), Some("*/*"));
        assert_eq!(
            req.header("accept-encoding").as_deref(),
            Some("gzip,deflate,br")
        );
    });
}

#[test]
fn fe_133_should_allow_to_override_default_headers() {
    run(|t| async move {
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        t.context
            .request()
            .get(&t.server.empty_page)
            .header("User-Agent", "Playwright")
            .header("Accept", "text/html")
            .header("Accept-Encoding", "br")
            .await
            .unwrap();
        let req = seen.recv().await.unwrap();
        assert_eq!(req.header("accept").as_deref(), Some("text/html"));
        assert_eq!(req.header("user-agent").as_deref(), Some("Playwright"));
        assert_eq!(req.header("accept-encoding").as_deref(), Some("br"));
    });
}

#[test]
fn fe_134_should_propagate_custom_headers_with_redirects() {
    run(|t| async move {
        t.server.set_redirect("/a/redirect1", "/b/c/redirect2");
        t.server.set_redirect("/b/c/redirect2", "/simple.json");
        t.context
            .request()
            .get(format!("{}/a/redirect1", t.server.prefix))
            .header("foo", "bar")
            .await
            .unwrap();
        for path in ["/a/redirect1", "/b/c/redirect2", "/simple.json"] {
            assert_eq!(t.server.request_headers(path).unwrap()["foo"], "bar");
        }
    });
}

#[test]
fn fe_137_should_throw_on_non_http_s_protocol() {
    run(|t| async move {
        let err = t
            .context
            .request()
            .get("data:text/plain,test")
            .await
            .unwrap_err();
        assert!(
            err.to_string().starts_with("apiRequestContext.get:"),
            "{err}"
        );
    });
}

#[test]
fn fe_140_should_resolve_url_relative_to_base_url() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .base_url(format!("{}/", t.server.prefix))
            .await
            .unwrap();
        let response = context.request().get("./empty.html").await.unwrap();
        assert_eq!(response.url(), t.server.empty_page);
    });
}

#[test]
fn fe_148_should_support_timeout_option() {
    run(|t| async move {
        t.server.hang("/slow");
        assert_err_contains(
            t.context
                .request()
                .get(format!("{}/slow", t.server.prefix))
                .timeout(100)
                .await,
            "Timeout 100ms exceeded",
        );
    });
}

#[test]
fn fe_149_should_support_a_timeout_of_0() {
    run(|t| async move {
        t.server.set_route("/slow", |_| async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            Response::builder().body(Body::from("done")).unwrap()
        });
        let response = t
            .context
            .request()
            .get(format!("{}/slow", t.server.prefix))
            .timeout(0)
            .await
            .unwrap();
        assert_eq!(response.text().await.unwrap(), "done");
    });
}

#[test]
fn fe_150_should_respect_timeout_after_redirects() {
    run(|t| async move {
        t.server.set_redirect("/redirect", "/slow");
        t.server.hang("/slow");
        t.context.set_default_timeout(100);
        assert_err_contains(
            t.context
                .request()
                .get(format!("{}/redirect", t.server.prefix))
                .await,
            "Timeout 100ms exceeded",
        );
    });
}

#[test]
fn fe_152_should_dispose() {
    run(|t| async move {
        let response = t
            .context
            .request()
            .get(format!("{}/simple.json", t.server.prefix))
            .await
            .unwrap();
        let json: serde_json::Value = response.json().await.unwrap();
        assert_eq!(json, json!({ "foo": "bar" }));
        response.dispose().await.unwrap();
        assert_err_contains(response.body().await, "Response has been disposed");
    });
}

#[test]
fn fe_153_should_dispose_when_context_closes() {
    run(|t| async move {
        let response = t
            .context
            .request()
            .get(format!("{}/simple.json", t.server.prefix))
            .await
            .unwrap();
        let json: serde_json::Value = response.json().await.unwrap();
        assert_eq!(json, json!({ "foo": "bar" }));
        t.context.close().await.unwrap();
        assert_err_contains(response.body().await, "Response has been disposed");
    });
}

#[test]
fn fe_155_should_support_application_x_www_form_urlencoded() {
    run(|t| async move {
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        t.context
            .request()
            .post(&t.server.empty_page)
            .form([("firstName", "John"), ("lastName", "Doe"), ("file", "f.js")])
            .await
            .unwrap();
        let req = seen.recv().await.unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(
            req.header("content-type").as_deref(),
            Some("application/x-www-form-urlencoded")
        );
        let body = String::from_utf8(req.body.clone()).unwrap();
        assert_eq!(
            req.header("content-length").as_deref(),
            Some(body.len().to_string().as_str())
        );
        assert_eq!(body, "firstName=John&lastName=Doe&file=f.js");
    });
}

#[test]
fn fe_157_should_encode_to_application_json_by_default() {
    run(|t| async move {
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        let data = json!({ "firstName": "John", "lastName": "Doe", "file": { "name": "f.js" } });
        t.context
            .request()
            .post(&t.server.empty_page)
            .json(&data)
            .await
            .unwrap();
        let req = seen.recv().await.unwrap();
        assert_eq!(
            req.header("content-type").as_deref(),
            Some("application/json")
        );
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body, data);
    });
}

#[test]
fn fe_159_should_escape_quotes_and_newlines_in_multipart_field_and_file_names() {
    run(|t| async move {
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        t.context
            .request()
            .post(&t.server.empty_page)
            .multipart_field("say\"hi\nthere", "v1")
            .multipart_field(
                "file",
                oxidewright::FilePayload::new(
                    "report \"final\"\r\n.txt",
                    "text/plain",
                    b"hello".to_vec(),
                ),
            )
            .await
            .unwrap();
        let body = String::from_utf8(seen.recv().await.unwrap().body).unwrap();
        let headers: Vec<_> = body
            .split("\r\n")
            .filter(|l| l.starts_with("content-disposition"))
            .collect();
        assert_eq!(
            headers,
            vec![
                "content-disposition: form-data; name=\"say%22hi%0D%0Athere\"",
                "content-disposition: form-data; name=\"file\"; filename=\"report %22final%22%0D%0A.txt\"",
            ]
        );
    });
}

#[test]
fn fe_163_should_serialize_data_to_json_regardless_of_content_type() {
    run(|t| async move {
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        let data = json!({ "firstName": "John", "lastName": "Doe" });
        t.context
            .request()
            .post(&t.server.empty_page)
            .header("content-type", "unknown")
            .json(&data)
            .await
            .unwrap();
        let req = seen.recv().await.unwrap();
        assert_eq!(req.header("content-type").as_deref(), Some("unknown"));
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body, data);
    });
}

#[test]
fn fe_165_context_request_should_export_same_storage_state_as_context() {
    run(|t| async move {
        t.server.set_route("/setcookie.html", |_| async {
            Response::builder()
                .header(header::SET_COOKIE, "a=b")
                .header(header::SET_COOKIE, "c=d")
                .body(Body::empty())
                .unwrap()
        });
        t.context
            .request()
            .get(format!("{}/setcookie.html", t.server.prefix))
            .await
            .unwrap();
        let context_state = t.context.storage_state().await.unwrap();
        assert_eq!(context_state.cookies.len(), 2);
        assert_eq!(
            t.context.request().storage_state().await.unwrap(),
            context_state
        );
        assert_eq!(
            t.page.request().storage_state().await.unwrap(),
            context_state
        );
    });
}

#[test]
fn fe_166_should_send_secure_cookie_over_http_for_localhost() {
    run(|t| async move {
        t.server.set_route("/setcookie.html", |_| async {
            Response::builder()
                .header(header::SET_COOKIE, "a=v; secure")
                .body(Body::empty())
                .unwrap()
        });
        t.page
            .request()
            .get(format!("{}/setcookie.html", t.server.prefix))
            .await
            .unwrap();
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        t.page.request().get(&t.server.empty_page).await.unwrap();
        assert_eq!(
            seen.recv().await.unwrap().header("cookie").as_deref(),
            Some("a=v")
        );
    });
}

#[test]
fn fe_167_should_accept_bool_and_numeric_params() {
    run(|t| async move {
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        t.page
            .request()
            .get(&t.server.empty_page)
            .param("str", "s")
            .param("num", 10)
            .param("bool", true)
            .param("bool2", false)
            .await
            .unwrap();
        assert_eq!(
            seen.recv().await.unwrap().uri,
            "/empty.html?str=s&num=10&bool=true&bool2=false"
        );
    });
}

#[test]
fn fe_168_should_abort_requests_when_browser_context_closes() {
    run(|t| async move {
        t.server.hang("/empty.html");
        let context = t.browser.new_context().await.unwrap();
        let request = context.request();
        let server = t.server.clone();
        let closer = context.clone();
        let (get, post, _) = tokio::join!(
            request.get(&t.server.empty_page).into_future(),
            request.post(&t.server.empty_page).into_future(),
            async move {
                server.wait_for_request("/empty.html").await;
                closer.close().await.unwrap();
            }
        );
        for result in [get, post] {
            assert_err_contains(result, "Target page, context or browser has been closed");
        }
    });
}

#[test]
fn fe_172_fetch_should_not_throw_on_long_set_cookie_value() {
    run(|t| async move {
        let long = "a".repeat(4097);
        t.server.set_route("/setcookie.html", move |_| {
            let long = long.clone();
            async move {
                Response::builder()
                    .header(header::SET_COOKIE, format!("loooong={long}"))
                    .header(header::SET_COOKIE, "bar=val")
                    .body(Body::empty())
                    .unwrap()
            }
        });
        t.context
            .request()
            .get(format!("{}/setcookie.html", t.server.prefix))
            .await
            .unwrap();
        let names: Vec<_> = t
            .context
            .cookies()
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, vec!["bar"]);
    });
}

#[test]
fn fe_175_should_not_work_after_dispose() {
    run(|t| async move {
        t.context.request().dispose().await.unwrap();
        assert_err_contains(
            t.context.request().get(&t.server.empty_page).await,
            "Target page, context or browser has been closed",
        );
    });
}

#[test]
fn fe_176_should_not_work_after_context_dispose() {
    run(|t| async move {
        t.context.close().reason("Test ended.").await.unwrap();
        assert_err_contains(
            t.context.request().get(&t.server.empty_page).await,
            "Test ended.",
        );
    });
}
