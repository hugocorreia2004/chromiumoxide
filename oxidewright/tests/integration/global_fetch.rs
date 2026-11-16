//! Ported from Playwright's `tests/library/global-fetch.spec.ts` and
//! `global-fetch-cookie.spec.ts`.
//!
//! Skipped:
//! * FE-018/019/021/022/024 (HTTPS server, security details): the test
//!   server has no TLS; `securityDetails()`/`serverAddr()`/`timing()` are not
//!   implemented.
//! * FE-020/023 (server address / security details): not implemented.
//! * FE-027 (Playwright user agent): oxidewright sends a Chrome-like user
//!   agent instead (see `oxidewright::fetch`).
//! * FE-028 (instrumentation hooks): no equivalent.
//! * FE-036 (`util.inspect`): covered by `Display` in `fe_036`.
//! * FE-041 (negative `maxRedirects`): `u32` cannot be negative.
//! * FE-045 (header capitalization): hyper sends lowercase header names.
//! * FE-046 (`toJSON` on the client): serialization happens before the call.
//! * FE-049/050/051 (socket-level resets): need raw socket control the axum
//!   test server does not offer.

use std::future::IntoFuture;
use std::time::Duration;

use axum::body::Body;
use axum::http::header;
use axum::response::Response;
use oxidewright::{HttpCredentials, HttpCredentialsSend};
use serde_json::json;

use crate::common::*;
use crate::fetch_util::*;

#[test]
fn fe_001_should_work() {
    run(|t| async move {
        let request = oxidewright::request().new_context().await.unwrap();
        for method in ["fetch", "delete", "get", "head", "patch", "post", "put"] {
            let url = format!("{}/simple.json", t.server.prefix);
            let builder = match method {
                "fetch" => request.fetch(&url),
                "delete" => request.delete(&url),
                "get" => request.get(&url),
                "head" => request.head(&url),
                "patch" => request.patch(&url),
                "post" => request.post(&url),
                _ => request.put(&url),
            };
            let response = builder.await.unwrap();
            assert_eq!(response.url(), url);
            assert_eq!(response.status(), 200);
            assert_eq!(response.status_text(), "OK");
            assert!(response.ok());
            assert_eq!(response.headers()["content-type"], "application/json");
            assert!(
                response
                    .headers_array()
                    .iter()
                    .any(|h| h.name.eq_ignore_ascii_case("content-type"))
            );
            let expected = if method == "head" {
                ""
            } else {
                "{\"foo\": \"bar\"}\n"
            };
            assert_eq!(response.text().await.unwrap(), expected, "{method}");
        }
        request.dispose().await.unwrap();
    });
}

#[test]
fn fe_002_should_dispose_global_request() {
    run(|t| async move {
        let request = oxidewright::request().new_context().await.unwrap();
        let response = request
            .get(format!("{}/simple.json", t.server.prefix))
            .await
            .unwrap();
        let json: serde_json::Value = response.json().await.unwrap();
        assert_eq!(json, json!({ "foo": "bar" }));
        request.dispose().await.unwrap();
        assert_err_contains(response.body().await, "Response has been disposed");
        request.dispose().await.unwrap();
    });
}

#[test]
fn fe_003_should_support_global_user_agent_option() {
    run(|t| async move {
        let request = oxidewright::request()
            .new_context()
            .user_agent("My Agent")
            .await
            .unwrap();
        let (_, response) = tokio::join!(
            t.server.wait_for_request("/empty.html"),
            request.get(&t.server.empty_page).into_future()
        );
        let response = response.unwrap();
        assert!(response.ok());
        assert_eq!(response.url(), t.server.empty_page);
        let headers = t.server.request_headers("/empty.html").unwrap();
        assert_eq!(headers["user-agent"], "My Agent");
    });
}

#[test]
fn fe_004_should_support_global_timeout_option() {
    run(|t| async move {
        let request = oxidewright::request()
            .new_context()
            .timeout(100)
            .await
            .unwrap();
        t.server.hang("/empty.html");
        assert_err_contains(
            request.get(&t.server.empty_page).await,
            "apiRequestContext.get: Timeout 100ms exceeded",
        );
    });
}

#[test]
fn fe_005_should_propagate_extra_http_headers_with_redirects() {
    run(|t| async move {
        t.server.set_redirect("/a/redirect1", "/b/c/redirect2");
        t.server.set_redirect("/b/c/redirect2", "/simple.json");
        let request = oxidewright::request()
            .new_context()
            .extra_http_header("My-Secret", "Value")
            .await
            .unwrap();
        request
            .get(format!("{}/a/redirect1", t.server.prefix))
            .await
            .unwrap();
        for path in ["/a/redirect1", "/b/c/redirect2", "/simple.json"] {
            let headers = t.server.request_headers(path).unwrap();
            assert_eq!(headers["my-secret"], "Value", "{path}");
        }
    });
}

#[test]
fn fe_006_should_preserve_authorization_on_same_origin_redirect_but_strip_on_cross_origin() {
    run(|t| async move {
        t.server.set_redirect("/same/redirect", "/same/dest");
        t.server.set_redirect(
            "/cross/redirect",
            &format!("{}/cross/dest", t.server.cross_process_prefix),
        );
        let request = oxidewright::request()
            .new_context()
            .extra_http_header("Authorization", "Bearer secret")
            .await
            .unwrap();
        request
            .get(format!("{}/same/redirect", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(
            t.server.request_headers("/same/dest").unwrap()["authorization"],
            "Bearer secret"
        );
        request
            .get(format!("{}/cross/redirect", t.server.prefix))
            .await
            .unwrap();
        assert!(
            t.server
                .request_headers("/cross/dest")
                .unwrap()
                .get("authorization")
                .is_none()
        );
    });
}

#[test]
fn fe_007_should_support_global_http_credentials_option() {
    run(|t| async move {
        set_auth(&t.server, "/empty.html", "user", "pass");
        let request1 = oxidewright::request().new_context().await.unwrap();
        let response1 = request1.get(&t.server.empty_page).await.unwrap();
        assert_eq!(response1.status(), 401);
        let request2 = oxidewright::request()
            .new_context()
            .http_credentials(HttpCredentials::new("user", "pass"))
            .await
            .unwrap();
        let response2 = request2.get(&t.server.empty_page).await.unwrap();
        assert_eq!(response2.status(), 200);
    });
}

#[test]
fn fe_008_should_return_error_with_wrong_credentials() {
    run(|t| async move {
        set_auth(&t.server, "/empty.html", "user", "pass");
        let request = oxidewright::request()
            .new_context()
            .http_credentials(HttpCredentials::new("user", "wrong"))
            .await
            .unwrap();
        assert_eq!(
            request.get(&t.server.empty_page).await.unwrap().status(),
            401
        );
    });
}

#[test]
fn fe_009_to_013_credentials_and_origins() {
    run(|t| async move {
        set_auth(&t.server, "/empty.html", "user", "pass");
        let port = t.server.port;
        let cases = [
            (t.server.prefix.clone(), 200),
            (t.server.prefix.to_uppercase(), 200),
            (t.server.prefix.replace("http://", "https://"), 401),
            (format!("http://mismatching-hostname:{port}"), 401),
            (format!("http://localhost:{}", port + 1), 401),
        ];
        for (origin, status) in cases {
            let request = oxidewright::request()
                .new_context()
                .http_credentials(HttpCredentials::new("user", "pass").origin(origin.clone()))
                .await
                .unwrap();
            assert_eq!(
                request.get(&t.server.empty_page).await.unwrap().status(),
                status,
                "{origin}"
            );
        }
    });
}

#[test]
fn fe_014_should_support_www_authenticate_basic() {
    run(|t| async move {
        t.server
            .set_route("/empty.html", |req: axum::extract::Request| async move {
                match req.headers().get(header::AUTHORIZATION) {
                    None => Response::builder()
                        .status(401)
                        .header(header::WWW_AUTHENTICATE, "Basic")
                        .body(Body::from("HTTP Error 401 Unauthorized: Access is denied"))
                        .unwrap(),
                    Some(value) => {
                        use base64::Engine;
                        let encoded = value.to_str().unwrap().split(' ').nth(1).unwrap_or("");
                        let decoded = base64::engine::general_purpose::STANDARD
                            .decode(encoded)
                            .unwrap();
                        Response::builder()
                            .header(header::CONTENT_TYPE, "text/plain")
                            .body(Body::from(decoded))
                            .unwrap()
                    }
                }
            });
        let request = oxidewright::request()
            .new_context()
            .http_credentials(HttpCredentials::new("user", "pass"))
            .await
            .unwrap();
        let response = request.get(&t.server.empty_page).await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.text().await.unwrap(), "user:pass");
    });
}

#[test]
fn fe_015_should_support_http_credentials_send() {
    run(|t| async move {
        let request = oxidewright::request()
            .new_context()
            .http_credentials(
                HttpCredentials::new("user", "pass")
                    .origin(t.server.prefix.to_uppercase())
                    .send(HttpCredentialsSend::Always),
            )
            .await
            .unwrap();
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        let response = request.get(&t.server.empty_page).await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            seen.recv()
                .await
                .unwrap()
                .header("authorization")
                .as_deref(),
            Some("Basic dXNlcjpwYXNz")
        );
        let response = request
            .get(format!("{}/empty.html", t.server.cross_process_prefix))
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(seen.recv().await.unwrap().header("authorization"), None);
    });
}

#[test]
fn fe_016_should_support_multiple_http_credentials() {
    run(|t| async move {
        set_auth(&t.server, "/empty.html", "user1", "pass1");
        let request = oxidewright::request()
            .new_context()
            .http_credentials(HttpCredentials::new("user1", "pass1").origin(&t.server.prefix))
            .http_credentials(
                HttpCredentials::new("user2", "pass2").origin(&t.server.cross_process_prefix),
            )
            .await
            .unwrap();
        assert_eq!(
            request.get(&t.server.empty_page).await.unwrap().status(),
            200
        );
        assert_eq!(
            request
                .get(format!("{}/empty.html", t.server.cross_process_prefix))
                .await
                .unwrap()
                .status(),
            401
        );
    });
}

#[test]
fn fe_025_should_resolve_url_relative_to_global_base_url_option() {
    run(|t| async move {
        let request = oxidewright::request()
            .new_context()
            .base_url(&t.server.prefix)
            .await
            .unwrap();
        let response = request.get("/empty.html").await.unwrap();
        assert_eq!(response.url(), t.server.empty_page);
    });
}

#[test]
fn fe_026_should_fallback_to_given_url_if_base_url_is_bogus() {
    run(|t| async move {
        let request = oxidewright::request()
            .new_context()
            .base_url("bogus")
            .await
            .unwrap();
        let response = request
            .get(format!("{}/empty.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(response.url(), t.server.empty_page);
    });
}

#[test]
fn fe_029_should_return_empty_body() {
    run(|t| async move {
        let request = oxidewright::request().new_context().await.unwrap();
        let response = request.get(&t.server.empty_page).await.unwrap();
        assert!(response.body().await.unwrap().is_empty());
        assert_eq!(response.text().await.unwrap(), "");
    });
}

#[test]
fn fe_030_should_abort_requests_when_context_is_disposed() {
    run(|t| async move {
        t.server.hang("/empty.html");
        let request = oxidewright::request().new_context().await.unwrap();
        let disposer = request.clone();
        let server = t.server.clone();
        let (get, post, delete, _) = tokio::join!(
            request.get(&t.server.empty_page).into_future(),
            request.post(&t.server.empty_page).into_future(),
            request.delete(&t.server.empty_page).into_future(),
            async move {
                server.wait_for_request("/empty.html").await;
                tokio::time::sleep(Duration::from_millis(100)).await;
                disposer.dispose().await.unwrap();
            }
        );
        for result in [get, post, delete] {
            assert_err_contains(result, "Request context disposed.");
        }
    });
}

#[test]
fn fe_032_should_remove_content_length_from_redirected_post_requests() {
    run(|t| async move {
        t.server.set_redirect("/redirect", "/empty.html");
        let mut target = capture(&t.server, "/empty.html", 200, "");
        let request = oxidewright::request().new_context().await.unwrap();
        let response = request
            .post(format!("{}/redirect", t.server.prefix))
            .json(&json!({ "foo": "bar" }))
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            t.server.request_headers("/redirect").unwrap()["content-length"],
            "13"
        );
        let second = target.recv().await.unwrap();
        assert_eq!(second.method, "GET");
        assert_eq!(second.header("content-length"), None);
    });
}

#[test]
fn fe_033_should_json_stringify_body_when_content_type_is_application_json() {
    run(|t| async move {
        let request = oxidewright::request().new_context().await.unwrap();
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        let values = [
            json!({ "foo": "bar" }),
            json!(["foo", "bar", 2021]),
            json!("foo"),
            json!(""),
            json!(true),
            json!(false),
            json!(2021),
            json!(0),
            json!(null),
            json!("undefined"),
        ];
        for value in values {
            let expected = serde_json::to_string(&value).unwrap();
            for content_type in ["application/json", "Application/JSON; charset=utf-8"] {
                // Strings go through the `data` text path, other values are JSON.
                let builder = request
                    .post(&t.server.empty_page)
                    .header("content-type", content_type);
                let builder = match &value {
                    serde_json::Value::String(s) => builder.data(s.as_str()),
                    other => builder.json(other),
                };
                builder.await.unwrap();
                let body = seen.recv().await.unwrap().body;
                assert_eq!(String::from_utf8(body).unwrap(), expected, "{value}");
            }
        }
    });
}

#[test]
fn fe_034_should_not_double_stringify_body_when_content_type_is_application_json() {
    run(|t| async move {
        let request = oxidewright::request().new_context().await.unwrap();
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        for value in [
            json!({ "foo": "bar" }),
            json!(["foo", "bar", 2021]),
            json!("foo"),
            json!(""),
            json!(true),
            json!(0),
            json!(null),
        ] {
            let stringified = serde_json::to_string(&value).unwrap();
            request
                .post(&t.server.empty_page)
                .header("content-type", "application/json")
                .data(stringified.as_str())
                .await
                .unwrap();
            let body = seen.recv().await.unwrap().body;
            assert_eq!(String::from_utf8(body).unwrap(), stringified);
        }
    });
}

#[test]
fn fe_035_should_accept_already_serialized_data_as_buffer_when_content_type_is_application_json() {
    run(|t| async move {
        let request = oxidewright::request().new_context().await.unwrap();
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        let value =
            serde_json::to_string(&serde_json::to_string(&json!({ "foo": "bar" })).unwrap())
                .unwrap();
        request
            .post(&t.server.empty_page)
            .header("content-type", "application/json")
            .data(value.clone().into_bytes())
            .await
            .unwrap();
        assert_eq!(seen.recv().await.unwrap().body, value.into_bytes());
    });
}

#[test]
fn fe_036_should_have_nice_to_string() {
    run(|t| async move {
        let request = oxidewright::request().new_context().await.unwrap();
        let response = request
            .post(&t.server.empty_page)
            .header("content-type", "application/json")
            .data("My post data")
            .await
            .unwrap();
        let text = response.to_string();
        assert!(text.contains("APIResponse: 200 OK"), "{text}");
        for header in response.headers_array() {
            assert!(text.contains(&format!("  {}: {}", header.name, header.value)));
        }
    });
}

#[test]
fn fe_037_should_not_fail_on_empty_body_with_encoding() {
    run(|t| async move {
        let request = oxidewright::request().new_context().await.unwrap();
        for method in ["HEAD", "PUT"] {
            for encoding in ["br", "gzip", "deflate"] {
                t.server.set_route("/empty.html", move |_| async move {
                    Response::builder()
                        .header("Content-Encoding", encoding)
                        .header(header::CONTENT_TYPE, "text/plain")
                        .body(Body::empty())
                        .unwrap()
                });
                let response = request
                    .fetch(&t.server.empty_page)
                    .method(method)
                    .await
                    .unwrap();
                assert_eq!(response.status(), 200);
                assert!(response.body().await.unwrap().is_empty());
            }
        }
    });
}

#[test]
fn fe_038_should_return_body_for_failing_requests() {
    run(|t| async move {
        let request = oxidewright::request().new_context().await.unwrap();
        t.server.set_route("/empty.html", |_| async {
            Response::builder()
                .status(404)
                .header(header::CONTENT_TYPE, "text/plain")
                .body(Body::from("Not found."))
                .unwrap()
        });
        for method in ["HEAD", "PUT", "TRACE"] {
            let response = request
                .fetch(&t.server.empty_page)
                .method(method)
                .await
                .unwrap();
            assert_eq!(response.status(), 404);
            let expected = if method == "HEAD" { "" } else { "Not found." };
            assert_eq!(response.text().await.unwrap(), expected);
        }
    });
}

const HTTP_METHODS: [&str; 6] = ["GET", "PUT", "POST", "OPTIONS", "HEAD", "PATCH"];

fn four_redirects(server: &TestServer) {
    server.set_redirect("/a/redirect1", "/b/c/redirect2");
    server.set_redirect("/b/c/redirect2", "/b/c/redirect3");
    server.set_redirect("/b/c/redirect3", "/b/c/redirect4");
    server.set_redirect("/b/c/redirect4", "/simple.json");
}

#[test]
fn fe_039_should_throw_an_error_when_max_redirects_is_exceeded() {
    run(|t| async move {
        four_redirects(&t.server);
        let request = oxidewright::request().new_context().await.unwrap();
        for method in HTTP_METHODS {
            for max in [1, 2, 3] {
                assert_err_contains(
                    request
                        .fetch(format!("{}/a/redirect1", t.server.prefix))
                        .method(method)
                        .max_redirects(max)
                        .await,
                    "Max redirect count exceeded",
                );
            }
        }
    });
}

#[test]
fn fe_040_should_not_follow_redirects_when_max_redirects_is_set_to_0() {
    run(|t| async move {
        t.server.set_redirect("/a/redirect1", "/b/c/redirect2");
        t.server.set_redirect("/b/c/redirect2", "/simple.json");
        let request = oxidewright::request().new_context().await.unwrap();
        for method in HTTP_METHODS {
            let response = request
                .fetch(format!("{}/a/redirect1", t.server.prefix))
                .method(method)
                .max_redirects(0)
                .await
                .unwrap();
            assert_eq!(response.headers()["location"], "/b/c/redirect2");
            assert_eq!(response.status(), 302);
        }
    });
}

#[test]
fn fe_042_should_not_follow_redirects_when_max_redirects_is_set_to_0_in_new_context() {
    run(|t| async move {
        t.server.set_redirect("/a/redirect1", "/b/c/redirect2");
        t.server.set_redirect("/b/c/redirect2", "/simple.json");
        let request = oxidewright::request()
            .new_context()
            .max_redirects(0)
            .await
            .unwrap();
        for method in HTTP_METHODS {
            let response = request
                .fetch(format!("{}/a/redirect1", t.server.prefix))
                .method(method)
                .await
                .unwrap();
            assert_eq!(response.headers()["location"], "/b/c/redirect2");
            assert_eq!(response.status(), 302);
        }
    });
}

#[test]
fn fe_043_should_follow_redirects_up_to_max_redirects_limit_set_in_new_context() {
    run(|t| async move {
        four_redirects(&t.server);
        for max in [1, 2, 3, 4] {
            let request = oxidewright::request()
                .new_context()
                .max_redirects(max)
                .await
                .unwrap();
            for method in HTTP_METHODS {
                let result = request
                    .fetch(format!("{}/a/redirect1", t.server.prefix))
                    .method(method)
                    .await;
                if max < 4 {
                    assert_err_contains(result, "Max redirect count exceeded");
                } else {
                    assert_eq!(result.unwrap().status(), 200);
                }
            }
        }
    });
}

#[test]
fn fe_044_should_use_max_redirects_from_fetch_when_provided_overriding_new_context() {
    run(|t| async move {
        four_redirects(&t.server);
        let request = oxidewright::request()
            .new_context()
            .max_redirects(1)
            .await
            .unwrap();
        for method in HTTP_METHODS {
            let response = request
                .fetch(format!("{}/a/redirect1", t.server.prefix))
                .method(method)
                .max_redirects(4)
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
        }
    });
}

#[test]
fn fe_047_should_throw_after_dispose() {
    run(|t| async move {
        let request = oxidewright::request().new_context().await.unwrap();
        request.dispose().await.unwrap();
        assert_err_contains(
            request.get(&t.server.empty_page).await,
            "Target page, context or browser has been closed",
        );
    });
}

#[test]
fn fe_052_should_throw_when_fail_on_status_code_is_set_to_true_inside_api_request_context_options()
{
    run(|t| async move {
        let request = oxidewright::request()
            .new_context()
            .fail_on_status_code(true)
            .await
            .unwrap();
        t.server.set_route("/empty.html", |_| async {
            Response::builder()
                .status(404)
                .header(header::CONTENT_TYPE, "text/plain")
                .body(Body::from("Not found."))
                .unwrap()
        });
        assert_err_contains(
            request.fetch(&t.server.empty_page).await,
            "404 Not Found\nResponse text:\nNot found.",
        );
    });
}

#[test]
fn fe_053_should_not_throw_when_fail_on_status_code_is_set_to_false_inside_api_request_context_options()
 {
    run(|t| async move {
        let request = oxidewright::request()
            .new_context()
            .fail_on_status_code(false)
            .await
            .unwrap();
        t.server.set_route("/empty.html", |_| async {
            Response::builder()
                .status(404)
                .body(Body::from("Not found."))
                .unwrap()
        });
        assert_eq!(
            request.fetch(&t.server.empty_page).await.unwrap().status(),
            404
        );
    });
}

// ----------------------------------------------------------------------
// global-fetch-cookie.spec.ts
// ----------------------------------------------------------------------

#[test]
fn should_store_cookie_from_set_cookie_header() {
    run(|t| async move {
        t.server.set_route("/setcookie.html", |_| async {
            Response::builder()
                .header(header::SET_COOKIE, "a=b")
                .header(header::SET_COOKIE, "c=d; max-age=3600; domain=b.one.com")
                .header(
                    header::SET_COOKIE,
                    "e=f; domain=b.one.com; path=/empty.html",
                )
                .body(Body::empty())
                .unwrap()
        });
        let request = oxidewright::request().new_context().await.unwrap();
        request
            .get(format!("{}/setcookie.html", t.server.prefix))
            .await
            .unwrap();
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        request.get(&t.server.empty_page).await.unwrap();
        assert_eq!(
            seen.recv().await.unwrap().header("cookie").as_deref(),
            Some("a=b")
        );
        let state = request.storage_state().await.unwrap();
        assert_eq!(state.cookies.len(), 1);
        assert_eq!(state.cookies[0].domain, "localhost");
        assert_eq!(state.cookies[0].path, "/");
    });
}

#[test]
fn should_remove_expired_cookies() {
    run(|t| async move {
        t.server.set_route("/setcookie.html", |_| async {
            Response::builder()
                .header(header::SET_COOKIE, "a=v; max-age=1000")
                .header(header::SET_COOKIE, "b=v; max-age=1000")
                .body(Body::empty())
                .unwrap()
        });
        t.server.set_route("/removecookie.html", |_| async {
            Response::builder()
                .header(header::SET_COOKIE, "a=v; max-age=0")
                .header(
                    header::SET_COOKIE,
                    "b=v; expires=Thu, 01 Jan 1970 00:00:00 GMT",
                )
                .body(Body::empty())
                .unwrap()
        });
        let request = oxidewright::request().new_context().await.unwrap();
        request
            .get(format!("{}/setcookie.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(request.cookies().await.unwrap().len(), 2);
        request
            .get(format!("{}/removecookie.html", t.server.prefix))
            .await
            .unwrap();
        assert!(request.cookies().await.unwrap().is_empty());
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        request.get(&t.server.empty_page).await.unwrap();
        assert_eq!(seen.recv().await.unwrap().header("cookie"), None);
    });
}

#[test]
fn should_preserve_local_storage_on_import_export_of_storage_state() {
    run(|t| async move {
        let state: oxidewright::StorageState = serde_json::from_value(json!({
            "cookies": [{
                "name": "a", "value": "b", "domain": "a.b.one.com", "path": "/",
                "expires": -1, "httpOnly": false, "secure": false, "sameSite": "Lax"
            }],
            "origins": [{
                "origin": "https://www.example.com",
                "localStorage": [{ "name": "name1", "value": "value1" }]
            }]
        }))
        .unwrap();
        let request = oxidewright::request()
            .new_context()
            .storage_state(state.clone())
            .await
            .unwrap();
        let exported = request.storage_state().await.unwrap();
        assert_eq!(exported.cookies, state.cookies);
        assert_eq!(
            exported.origins[0].local_storage,
            state.origins[0].local_storage
        );
        let _ = t;
    });
}

#[test]
fn should_add_and_clear_cookies() {
    run(|t| async move {
        let request = oxidewright::request().new_context().await.unwrap();
        request
            .add_cookies([
                oxidewright::SetCookie::new("a", "1").url(&t.server.empty_page),
                oxidewright::SetCookie::new("b", "2").url(&t.server.empty_page),
            ])
            .await
            .unwrap();
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        request.get(&t.server.empty_page).await.unwrap();
        assert_eq!(
            seen.recv().await.unwrap().header("cookie").as_deref(),
            Some("a=1; b=2")
        );
        request.clear_cookies().name("a").await.unwrap();
        let names: Vec<_> = request
            .cookies()
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, vec!["b"]);
        request.clear_cookies().await.unwrap();
        assert!(request.cookies().await.unwrap().is_empty());
    });
}

#[test]
fn should_send_form_and_multipart_data() {
    run(|t| async move {
        let request = oxidewright::request().new_context().await.unwrap();
        let mut seen = capture(&t.server, "/empty.html", 200, "");
        request
            .post(&t.server.empty_page)
            .form_field("foo", 1)
            .form_field("foo", 2)
            .form_field("name", "John Doe")
            .await
            .unwrap();
        let req = seen.recv().await.unwrap();
        assert_eq!(
            req.header("content-type").as_deref(),
            Some("application/x-www-form-urlencoded")
        );
        assert_eq!(req.body, b"foo=1&foo=2&name=John+Doe");
        request
            .post(&t.server.empty_page)
            .multipart_field("firstName", "John")
            .multipart_field(
                "file",
                oxidewright::FilePayload::new("f.js", "text/javascript", b"var x = 10;".to_vec()),
            )
            .await
            .unwrap();
        let req = seen.recv().await.unwrap();
        let content_type = req.header("content-type").unwrap();
        assert!(content_type.starts_with("multipart/form-data; boundary="));
        let body = String::from_utf8(req.body).unwrap();
        assert!(body.contains("name=\"firstName\"\r\n\r\nJohn\r\n"));
        assert!(body.contains(
            "name=\"file\"; filename=\"f.js\"\r\ncontent-type: text/javascript\r\n\r\nvar x = 10;\r\n"
        ));
        assert_err_contains(
            request
                .post(&t.server.empty_page)
                .data("x")
                .form_field("a", "b")
                .await,
            "Only one of 'data', 'form' or 'multipart' can be specified",
        );
    });
}
