//! Ported from Playwright's `tests/library/proxy.spec.ts` and
//! `browsercontext-proxy.spec.ts`: the test server acts as the HTTP proxy
//! (it receives absolute-form requests and serves them by path).

use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::Response;
use base64::Engine;
use oxidewright::{Browser, ProxySettings};

use crate::common::{TestServer, run};

async fn launch(proxy: Option<ProxySettings>) -> Browser {
    let mut launch = oxidewright::chromium().launch();
    if let Ok(path) = std::env::var("OXIDEWRIGHT_CHROME") {
        launch = launch.executable_path(path);
    }
    if let Some(proxy) = proxy {
        launch = launch.proxy(proxy);
    }
    launch.await.expect("failed to launch the browser")
}

fn serve_target(server: &TestServer) {
    server.set_route("/target.html", |_| async {
        Response::new(Body::from(
            "<html><title>Served by the proxy</title></html>",
        ))
    });
}

/// A route that demands proxy authentication and echoes the credentials.
fn serve_proxy_auth(server: &TestServer) {
    server.set_route("/target.html", |req| async move {
        match req.headers().get("proxy-authorization") {
            None => Response::builder()
                .status(StatusCode::PROXY_AUTHENTICATION_REQUIRED)
                .header(
                    "proxy-authenticate",
                    "Basic realm=\"Access to internal site\"",
                )
                .body(Body::empty())
                .unwrap(),
            Some(auth) => Response::new(Body::from(format!(
                "<html><title>{}</title></html>",
                auth.to_str().unwrap()
            ))),
        }
    });
}

fn basic(user: &str, pass: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"))
    )
}

fn with_auth(server: &str, user: &str, pass: &str) -> ProxySettings {
    let mut proxy = ProxySettings::new(server);
    proxy.username = Some(user.into());
    proxy.password = Some(pass.into());
    proxy
}

#[test]
fn pr_002_should_use_proxy() {
    run(|t| async move {
        serve_target(&t.server);
        let browser = launch(Some(ProxySettings::new(format!(
            "localhost:{}",
            t.server.port
        ))))
        .await;
        let page = browser.new_page().await.unwrap();
        let url = "http://staging.example.org/target.html";
        page.goto(url).await.unwrap();
        assert_eq!(page.title().await.unwrap(), "Served by the proxy");
        assert_eq!(page.url(), url);
        browser.close().await.unwrap();
    });
}

#[test]
fn pr_003_should_use_proxy_for_second_page() {
    run(|t| async move {
        serve_target(&t.server);
        let browser = launch(Some(ProxySettings::new(format!(
            "localhost:{}",
            t.server.port
        ))))
        .await;
        for _ in 0..2 {
            let page = browser.new_page().await.unwrap();
            page.goto("http://non-existent.test/target.html")
                .await
                .unwrap();
            assert_eq!(page.title().await.unwrap(), "Served by the proxy");
        }
        browser.close().await.unwrap();
    });
}

#[test]
fn pr_004_should_work_with_ip_port_notion() {
    run(|t| async move {
        serve_target(&t.server);
        let browser = launch(Some(ProxySettings::new(format!(
            "127.0.0.1:{}",
            t.server.port
        ))))
        .await;
        let page = browser.new_page().await.unwrap();
        page.goto("http://non-existent.test/target.html")
            .await
            .unwrap();
        assert_eq!(page.title().await.unwrap(), "Served by the proxy");
        browser.close().await.unwrap();
    });
}

/// `PR-005`: loopback goes through the proxy too unless bypassed (a page on
/// a port nothing listens on is still served by the proxy), with or without
/// an unrelated bypass rule.
fn proxy_local_network_requests(target: &'static str, additional_bypass: bool) {
    run(move |t| async move {
        serve_target(&t.server);
        let mut proxy = ProxySettings::new(format!("localhost:{}", t.server.port));
        if additional_bypass {
            proxy.bypass = Some("1.non.existent.domain.for.the.test".into());
        }
        let browser = launch(Some(proxy)).await;
        let page = browser.new_page().await.unwrap();
        page.goto(format!("http://{target}:55555/target.html"))
            .await
            .unwrap();
        assert_eq!(page.title().await.unwrap(), "Served by the proxy");
        let bypassed = page
            .goto("http://1.non.existent.domain.for.the.test/target.html")
            .await
            .is_err();
        assert_eq!(bypassed, additional_bypass);
        browser.close().await.unwrap();
    });
}

#[test]
fn pr_005_should_proxy_local_network_requests_localhost() {
    proxy_local_network_requests("localhost", false);
}

#[test]
fn pr_005_should_proxy_local_network_requests_localhost_with_bypass() {
    proxy_local_network_requests("localhost", true);
}

#[test]
fn pr_005_should_proxy_local_network_requests_loopback_ip() {
    proxy_local_network_requests("127.0.0.1", false);
}

#[test]
fn pr_005_should_proxy_local_network_requests_loopback_ip_with_bypass() {
    proxy_local_network_requests("127.0.0.1", true);
}

#[test]
fn pr_006_should_allow_bypassing_localhost_requests() {
    run(|t| async move {
        serve_target(&t.server);
        let mut proxy = ProxySettings::new("127.0.0.1:1");
        proxy.bypass = Some("localhost".into());
        let browser = launch(Some(proxy)).await;
        let page = browser.new_page().await.unwrap();
        // Bypassed: reaches the server directly (the proxy port is dead).
        page.goto(format!("{}/target.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(page.title().await.unwrap(), "Served by the proxy");
        browser.close().await.unwrap();
    });
}

#[test]
fn pr_007_should_authenticate() {
    run(|t| async move {
        serve_proxy_auth(&t.server);
        t.server.set_route("/protected.html", |req| async move {
            if req.headers().get("proxy-authorization").is_none() {
                return Response::builder()
                    .status(StatusCode::PROXY_AUTHENTICATION_REQUIRED)
                    .header(
                        "proxy-authenticate",
                        "Basic realm=\"Access to internal site\"",
                    )
                    .body(Body::empty())
                    .unwrap();
            }
            let auth = req
                .headers()
                .get(header::AUTHORIZATION)
                .map(|v| v.to_str().unwrap().to_string())
                .unwrap_or_else(|| "no credentials".into());
            Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header(header::WWW_AUTHENTICATE, "Basic realm=\"site\"")
                .body(Body::from(auth))
                .unwrap()
        });
        let browser = launch(Some(with_auth(
            &format!("localhost:{}", t.server.port),
            "user",
            "secret",
        )))
        .await;
        let page = browser.new_page().await.unwrap();
        page.goto("http://non-existent.test/target.html")
            .await
            .unwrap();
        assert_eq!(page.title().await.unwrap(), basic("user", "secret"));
        // Proxy credentials are never sent to the website's own challenge.
        let response = page
            .goto("http://non-existent.test/protected.html")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), 401);
        assert_eq!(
            page.locator("body")
                .text_content()
                .await
                .unwrap()
                .as_deref(),
            Some("no credentials")
        );
        browser.close().await.unwrap();
    });
}

#[test]
fn pr_010_should_exclude_patterns() {
    run(|t| async move {
        serve_target(&t.server);
        let mut proxy = ProxySettings::new(format!("localhost:{}", t.server.port));
        proxy.bypass = Some(
            "1.non.existent.domain.for.the.test, 2.non.existent.domain.for.the.test, .another.test"
                .into(),
        );
        let browser = launch(Some(proxy)).await;
        let page = browser.new_page().await.unwrap();
        page.goto("http://0.non.existent.domain.for.the.test/target.html")
            .await
            .unwrap();
        assert_eq!(page.title().await.unwrap(), "Served by the proxy");
        for url in [
            "http://1.non.existent.domain.for.the.test/target.html",
            "http://2.non.existent.domain.for.the.test/target.html",
            "http://foo.another.test/target.html",
        ] {
            let page = browser.new_page().await.unwrap();
            assert!(
                page.goto(url).await.is_err(),
                "{url} went through the proxy"
            );
            page.close().await.unwrap();
        }
        let page = browser.new_page().await.unwrap();
        page.goto("http://3.non.existent.domain.for.the.test/target.html")
            .await
            .unwrap();
        assert_eq!(page.title().await.unwrap(), "Served by the proxy");
        browser.close().await.unwrap();
    });
}

#[test]
fn pr_014_does_launch_without_a_port() {
    run(|_t| async move {
        let browser = launch(Some(ProxySettings::new("http://localhost"))).await;
        browser.close().await.unwrap();
    });
}

#[test]
fn pr_018_should_work_when_passing_the_proxy_only_on_the_context_level() {
    run(|t| async move {
        serve_target(&t.server);
        let browser = launch(None).await;
        let context = browser
            .new_context()
            .proxy(ProxySettings::new(format!("localhost:{}", t.server.port)))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto("http://non-existent.test/target.html")
            .await
            .unwrap();
        assert_eq!(page.title().await.unwrap(), "Served by the proxy");
        browser.close().await.unwrap();
    });
}

#[test]
fn pr_026_should_use_proxy_twice() {
    run(|t| async move {
        serve_target(&t.server);
        let context = t
            .browser
            .new_context()
            .proxy(ProxySettings::new(format!("localhost:{}", t.server.port)))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto("http://non-existent.test/target.html")
            .await
            .unwrap();
        page.goto("http://non-existent-2.test/target.html")
            .await
            .unwrap();
        assert_eq!(page.title().await.unwrap(), "Served by the proxy");
        context.close().await.unwrap();
    });
}

#[test]
fn pr_030_should_throw_for_socks5_authentication() {
    run(|t| async move {
        let error = t
            .browser
            .new_context()
            .proxy(with_auth("socks5://localhost:1234", "user", "secret"))
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Browser does not support socks5 proxy authentication"),
            "{error}"
        );
    });
}

#[test]
fn pr_031_should_throw_for_socks4_authentication() {
    run(|t| async move {
        let error = t
            .browser
            .new_context()
            .proxy(with_auth("socks4://localhost:1234", "user", "secret"))
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Socks4 proxy protocol does not support authentication"),
            "{error}"
        );
    });
}

#[test]
fn pr_032_should_authenticate_on_the_context_level() {
    run(|t| async move {
        serve_proxy_auth(&t.server);
        let context = t
            .browser
            .new_context()
            .proxy(with_auth(
                &format!("localhost:{}", t.server.port),
                "user",
                "secret",
            ))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto("http://non-existent.test/target.html")
            .await
            .unwrap();
        assert_eq!(page.title().await.unwrap(), basic("user", "secret"));
        context.close().await.unwrap();
    });
}

#[test]
fn pr_033_should_authenticate_with_empty_password() {
    run(|t| async move {
        serve_proxy_auth(&t.server);
        let context = t
            .browser
            .new_context()
            .proxy(with_auth(
                &format!("localhost:{}", t.server.port),
                "user",
                "",
            ))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto("http://non-existent.test/target.html")
            .await
            .unwrap();
        assert_eq!(page.title().await.unwrap(), basic("user", ""));
        context.close().await.unwrap();
    });
}

#[test]
fn pr_034_should_isolate_proxy_credentials_between_contexts() {
    run(|t| async move {
        serve_proxy_auth(&t.server);
        for (user, pass) in [("user1", "secret1"), ("user2", "secret2")] {
            let context = t
                .browser
                .new_context()
                .proxy(with_auth(
                    &format!("localhost:{}", t.server.port),
                    user,
                    pass,
                ))
                .await
                .unwrap();
            let page = context.new_page().await.unwrap();
            page.goto("http://non-existent.test/target.html")
                .await
                .unwrap();
            assert_eq!(page.title().await.unwrap(), basic(user, pass));
            context.close().await.unwrap();
        }
    });
}
