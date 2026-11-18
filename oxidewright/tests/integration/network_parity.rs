//! Ports for the network parity additions: `route.fetch` options
//! (`page-request-intercept.spec.ts` RO-143/144), `APIResponse` timing and
//! server address (`browsercontext-fetch.spec.ts` FE-090,
//! `global-fetch.spec.ts` FE-020/023), `fileChooser.setFiles` options,
//! `CDPSession` timeout and `close`, `tracing.startHar`
//! (`har.spec.ts` HA-060/062), `context.credentials`
//! (`browsercontext-webauthn.spec.ts` AU-017/018/019/021/022) and service
//! workers (`chromium.spec.ts`).
//!
//! Skipped: FE-021/022 (need an HTTPS test server), AU-020/023
//! (`storageState({ credentials })`), AU-024 (JSON of the mock's
//! credentials), HA-061/063..065 (`APIRequestContext.tracing`), HA-066/067
//! (`resourcesDir`).

use std::time::Duration;

use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::Response;
use oxidewright::{FilePayload, HarMode, JsValue, Route};
use serde_json::{Value, json};

use crate::common::*;

// ------------------------------------------------------------ route.fetch

#[test]
fn ro_143_should_support_timeout_option_in_route_fetch() {
    run(|t| async move {
        t.server.set_route("/slow", |_| async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Response::new(Body::empty())
        });
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        t.page
            .route("**/*", move |route: Route| {
                let tx = tx.clone();
                async move {
                    let error = route.fetch().timeout(1000).await.unwrap_err();
                    let _ = tx.send(error.to_string());
                    let _ = route.abort().await;
                    Ok(())
                }
            })
            .await
            .unwrap();
        let _ = t
            .page
            .goto(format!("{}/slow", t.server.prefix))
            .timeout(5000)
            .await;
        let message = rx.recv().await.unwrap();
        assert!(
            message.contains("route.fetch: Timeout 1000ms exceeded"),
            "{message}"
        );
    });
}

#[test]
fn ro_144_should_not_follow_redirects_when_max_redirects_is_set_to_0_in_route_fetch() {
    run(|t| async move {
        t.server.set_redirect("/foo", "/empty.html");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(u16, Option<String>)>();
        t.page
            .route("**/*", move |route: Route| {
                let tx = tx.clone();
                async move {
                    let response = route.fetch().max_redirects(0).await.unwrap();
                    let location = response.headers().get("location").cloned();
                    let _ = tx.send((response.status(), location));
                    route.fulfill().body("hello").await
                }
            })
            .await
            .unwrap();
        t.page
            .goto(format!("{}/foo", t.server.prefix))
            .await
            .unwrap();
        assert!(t.page.content().await.unwrap().contains("hello"));
        let (status, location) = rx.recv().await.unwrap();
        assert_eq!(status, 302);
        assert_eq!(location.as_deref(), Some("/empty.html"));
    });
}

#[test]
fn ro_route_fetch_should_follow_redirects_by_default() {
    run(|t| async move {
        t.server.set_redirect("/foo", "/empty.html");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(u16, String)>();
        t.page
            .route("**/foo", move |route: Route| {
                let tx = tx.clone();
                async move {
                    let response = route.fetch().await.unwrap();
                    let _ = tx.send((response.status(), response.url().to_string()));
                    route.fulfill().response(&response).await
                }
            })
            .await
            .unwrap();
        t.page
            .goto(format!("{}/foo", t.server.prefix))
            .await
            .unwrap();
        let (status, url) = rx.recv().await.unwrap();
        assert_eq!(status, 200);
        assert_eq!(url, t.server.empty_page);
    });
}

// ------------------------------------------------------------ APIResponse

#[test]
fn fe_090_should_return_timing() {
    run(|t| async move {
        let response = t.context.request().get(&t.server.empty_page).await.unwrap();
        assert!(response.ok());
        let timing = response.timing();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as f64;
        assert!((timing.start_time - now).abs() < 10_000.0);
        assert!(timing.response_start >= timing.request_start);
        assert!(timing.response_end >= timing.response_start);
        assert!(timing.response_end < 60_000.0);
        assert_eq!(timing.secure_connection_start, -1.0);
    });
}

#[test]
fn fe_020_should_return_server_address_from_response() {
    run(|t| async move {
        let request = oxidewright::request().new_context().await.unwrap();
        for _ in 0..2 {
            let response = request.get(&t.server.empty_page).await.unwrap();
            let addr = response.server_addr().await.unwrap();
            assert!(
                addr.ip_address == "127.0.0.1" || addr.ip_address == "::1",
                "{}",
                addr.ip_address
            );
            assert_eq!(addr.port, t.server.port);
        }
    });
}

#[test]
fn fe_023_should_return_null_security_details_for_http_response() {
    run(|t| async move {
        let request = oxidewright::request().new_context().await.unwrap();
        let response = request.get(&t.server.empty_page).await.unwrap();
        assert!(response.security_details().await.is_none());
    });
}

// ------------------------------------------------------------ FileChooser

#[test]
fn fc_set_files_should_accept_file_payloads_and_options() {
    run(|t| async move {
        t.page
            .set_content("<input type=file multiple>")
            .await
            .unwrap();
        let chooser = t
            .page
            .expect_file_chooser(
                t.page
                    .evaluate::<()>("() => document.querySelector('input').click()"),
            )
            .await
            .unwrap();
        chooser
            .set_files(vec![
                FilePayload::new("a.txt", "text/plain", b"hello".to_vec()),
                FilePayload::new("b.txt", "text/plain", b"world!".to_vec()),
            ])
            .timeout(5000)
            .no_wait_after(true)
            .await
            .unwrap();
        let files: Value = t
            .page
            .evaluate("async () => Promise.all([...document.querySelector('input').files].map(async f => [f.name, await f.text()]))")
            .await
            .unwrap();
        assert_eq!(files, json!([["a.txt", "hello"], ["b.txt", "world!"]]));
    });
}

// ------------------------------------------------------------ CDPSession

#[test]
fn cdp_session_send_should_time_out() {
    run(|t| async move {
        let session = t.context.new_cdp_session(&t.page).await.unwrap();
        let error = session
            .send(
                "Runtime.evaluate",
                json!({ "expression": "new Promise(() => {})", "awaitPromise": true }),
            )
            .timeout(500)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cdpSession.send: Timeout 500ms exceeded"),
            "{error}"
        );
    });
}

#[test]
fn cdp_session_should_emit_close() {
    run(|t| async move {
        let session = t.context.new_cdp_session(&t.page).await.unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        session.on_close(move || {
            let _ = tx.send(());
        });
        assert!(!session.is_closed());
        session.detach().await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap();
        assert!(session.is_closed());

        let page = t.context.new_page().await.unwrap();
        let other = t.context.new_cdp_session(&page).await.unwrap();
        page.close().await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), other.wait_for_close())
            .await
            .unwrap();
    });
}

// ------------------------------------------------------------ tracing.startHar

#[test]
fn ha_060_should_record_a_har_with_options() {
    run(|t| async move {
        let dir = std::env::temp_dir().join(format!("ow-har-{}", fastrand_suffix()));
        std::fs::create_dir_all(&dir).unwrap();
        let har_path = dir.join("tracing.har");
        t.context
            .tracing()
            .start_har(&har_path)
            .mode(HarMode::Minimal)
            .url_filter("**/one-style.css")
            .await
            .unwrap();
        let page = t.context.new_page().await.unwrap();
        page.goto(format!("{}/one-style.html", t.server.prefix))
            .await
            .unwrap();
        t.context.tracing().stop_har().await.unwrap();
        let log: Value =
            serde_json::from_str(&std::fs::read_to_string(&har_path).unwrap()).unwrap();
        let urls: Vec<&str> = log["log"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["request"]["url"].as_str().unwrap())
            .collect();
        assert_eq!(urls, vec![format!("{}/one-style.css", t.server.prefix)]);
        assert_eq!(log["log"]["entries"][0]["request"]["bodySize"], -1);
        let again = t.context.tracing().stop_har().await;
        assert_err_contains(again, "HAR recording has not been started");
        let _ = std::fs::remove_dir_all(dir);
    });
}

#[test]
fn ha_062_should_include_pages() {
    run(|t| async move {
        let dir = std::env::temp_dir().join(format!("ow-har-{}", fastrand_suffix()));
        std::fs::create_dir_all(&dir).unwrap();
        let har_path = dir.join("tracing.har");
        t.context.tracing().start_har(&har_path).await.unwrap();
        assert_err_contains(
            t.context.tracing().start_har(&har_path).await,
            "HAR recording has already been started",
        );
        let page = t.context.new_page().await.unwrap();
        page.goto(format!("{}/title.html", t.server.prefix))
            .await
            .unwrap();
        let page2 = t.context.new_page().await.unwrap();
        page2.goto(&t.server.empty_page).await.unwrap();
        page.close().await.unwrap();
        t.context.tracing().stop_har().await.unwrap();
        let log: Value =
            serde_json::from_str(&std::fs::read_to_string(&har_path).unwrap()).unwrap();
        let pages = log["log"]["pages"].as_array().unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0]["title"], "Woof-Woof");
        assert_eq!(pages[1]["title"], "");
        let _ = std::fs::remove_dir_all(dir);
    });
}

fn fastrand_suffix() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

// ------------------------------------------------------------ credentials

const ASSERT_SIGN_COUNT: &str = r#"async ({ rpId }) => {
    const challenge = crypto.getRandomValues(new Uint8Array(32));
    const cred = await navigator.credentials.get({
      publicKey: { challenge, rpId, userVerification: 'preferred' },
    });
    return new DataView(cred.response.authenticatorData).getUint32(33);
  }"#;

const CREATE_CREDENTIAL: &str = r#"async ({ rpId }) => {
    const challenge = crypto.getRandomValues(new Uint8Array(32));
    const created = await navigator.credentials.create({
      publicKey: {
        challenge,
        rp: { id: rpId, name: 'Test RP' },
        user: { id: new Uint8Array([1, 2, 3, 4]), name: 'u', displayName: 'User' },
        pubKeyCredParams: [{ type: 'public-key', alg: -7 }],
        authenticatorSelection: { residentKey: 'required', userVerification: 'preferred' },
      },
    });
    return created.id;
  }"#;

#[test]
fn au_017_should_not_intercept_navigator_credentials_without_install() {
    run(|t| async move {
        t.context.credentials().create("localhost").await.unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        let intercepted: bool = t
            .page
            .evaluate("() => globalThis.__pwWebAuthnInstalled === true")
            .await
            .unwrap();
        assert!(!intercepted);
        // No main-world trace after install either.
        t.context.credentials().install().await.unwrap();
        let native: bool = t
            .page
            .evaluate("() => navigator.credentials.get.toString().includes('[native code]')")
            .await
            .unwrap();
        assert!(native);
    });
}

#[test]
fn au_018_should_seed_a_known_credential_and_authenticate() {
    run(|t| async move {
        let source = t.browser.new_context().await.unwrap();
        let known = source.credentials().create("localhost").await.unwrap();
        let context = t.browser.new_context().await.unwrap();
        context
            .credentials()
            .create(known.rp_id.clone())
            .from(&known)
            .await
            .unwrap();
        context.credentials().install().await.unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let script = r#"async ({ rpId, credentialId }) => {
    const b64UrlToBytes = s => {
      let str = s.replace(/-/g, '+').replace(/_/g, '/');
      while (str.length % 4) str += '=';
      const bin = atob(str);
      const u8 = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) u8[i] = bin.charCodeAt(i);
      return u8;
    };
    const challenge = crypto.getRandomValues(new Uint8Array(32));
    try {
      const cred = await navigator.credentials.get({
        publicKey: { challenge, rpId, allowCredentials: [{ type: 'public-key', id: b64UrlToBytes(credentialId) }], userVerification: 'preferred' },
      });
      return { id: cred.id, type: cred.type, hasSignature: cred.response.signature.byteLength > 0, flags: new Uint8Array(cred.response.authenticatorData)[32] };
    } catch (e) {
      return { error: e.name };
    }
  }"#;
        let arg = json!({ "rpId": "localhost", "credentialId": known.id });
        let result: Value = page.evaluate(script).arg(&arg).await.unwrap();
        assert_eq!(result["id"], known.id.as_str());
        assert_eq!(result["type"], "public-key");
        assert_eq!(result["hasSignature"], true);
        assert_eq!(result["flags"].as_u64().unwrap() & 0x05, 0x05);
        context.credentials().delete(&known.id).await.unwrap();
        assert!(context.credentials().get().await.unwrap().is_empty());
        let result: Value = page.evaluate(script).arg(&arg).await.unwrap();
        assert_eq!(result["error"], "NotAllowedError");
    });
}

#[test]
fn au_019_should_capture_a_page_created_credential_and_reuse_it_in_another_context() {
    run(|t| async move {
        t.context.credentials().install().await.unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        let created_id: String = t
            .page
            .evaluate(CREATE_CREDENTIAL)
            .arg(json!({ "rpId": "localhost" }))
            .await
            .unwrap();
        let captured = t
            .context
            .credentials()
            .get()
            .rp_id("localhost")
            .await
            .unwrap();
        assert_eq!(captured.len(), 1);
        let captured = captured[0].clone();
        assert_eq!(captured.id, created_id);
        let b64url = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        };
        assert!(b64url(&captured.private_key));
        assert!(b64url(&captured.public_key));

        let context = t.browser.new_context().await.unwrap();
        context
            .credentials()
            .create(captured.rp_id.clone())
            .from(&captured)
            .await
            .unwrap();
        context.credentials().install().await.unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let got: String = page
            .evaluate(
                "async ({ rpId }) => (await navigator.credentials.get({ publicKey: { challenge: crypto.getRandomValues(new Uint8Array(32)), rpId, userVerification: 'preferred' } })).id",
            )
            .arg(json!({ "rpId": "localhost" }))
            .await
            .unwrap();
        assert_eq!(got, created_id);
    });
}

#[test]
fn au_021_should_seed_and_report_sign_count() {
    run(|t| async move {
        let credentials = t.context.credentials();
        let fresh = credentials.create("fresh.example.com").await.unwrap();
        assert_eq!(fresh.sign_count, 0);
        let seeded = credentials
            .create("localhost")
            .sign_count(41)
            .await
            .unwrap();
        assert_eq!(seeded.sign_count, 41);
        credentials.install().await.unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        let arg = json!({ "rpId": "localhost" });
        let n: u32 = t.page.evaluate(ASSERT_SIGN_COUNT).arg(&arg).await.unwrap();
        assert_eq!(n, 42);
        let n: u32 = t.page.evaluate(ASSERT_SIGN_COUNT).arg(&arg).await.unwrap();
        assert_eq!(n, 43);
        let mut expected = seeded.clone();
        expected.sign_count = 43;
        assert_eq!(
            credentials.get().id(seeded.id.clone()).await.unwrap(),
            vec![expected.clone()]
        );
        assert_eq!(
            credentials.get().id(fresh.id.clone()).await.unwrap(),
            vec![fresh]
        );
        let context2 = t.browser.new_context().await.unwrap();
        context2
            .credentials()
            .create(expected.rp_id.clone())
            .from(&expected)
            .await
            .unwrap();
        context2.credentials().install().await.unwrap();
        let page2 = context2.new_page().await.unwrap();
        page2.goto(&t.server.empty_page).await.unwrap();
        let n: u32 = page2.evaluate(ASSERT_SIGN_COUNT).arg(&arg).await.unwrap();
        assert_eq!(n, 44);
    });
}

#[test]
fn au_022_should_reject_invalid_sign_count() {
    run(|t| async move {
        let r = t
            .context
            .credentials()
            .create("example.com")
            .sign_count(-1)
            .await;
        assert_err_contains(r, "signCount must be between 0 and 4294967295, got -1");
        assert!(t.context.credentials().get().await.unwrap().is_empty());
    });
}

// ------------------------------------------------------------ service workers

fn serve_service_worker(t: &TestContext) {
    t.server.set_route("/sw.js", |_| async move {
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/javascript")
            .body(Body::from(
                "self.addEventListener('install', () => self.skipWaiting());",
            ))
            .unwrap()
    });
    t.server.set_route("/sw.html", |_| async move {
        Response::builder()
            .header(header::CONTENT_TYPE, "text/html")
            .body(Body::from(
                "<script>window.registrationPromise = navigator.serviceWorker.register('sw.js');</script>",
            ))
            .unwrap()
    });
}

/// Chromium may run component extension service workers
/// (`chrome-extension://...`) in the context too: only the page's count here.
fn page_workers(t: &TestContext) -> Vec<oxidewright::Worker> {
    t.context
        .service_workers()
        .into_iter()
        .filter(|w| w.url().starts_with("http"))
        .collect()
}

async fn wait_page_worker(t: &TestContext) -> oxidewright::Worker {
    for _ in 0..400 {
        if let Some(w) = page_workers(t).into_iter().next() {
            return w;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("no service worker");
}

#[test]
fn sw_should_create_a_worker_from_a_service_worker() {
    run(|t| async move {
        serve_service_worker(&t);
        t.page
            .goto(format!("{}/sw.html", t.server.prefix))
            .await
            .unwrap();
        let worker = wait_page_worker(&t).await;
        assert!(worker.url().ends_with("/sw.js"), "{}", worker.url());
        let scope: String = worker.evaluate("() => self.toString()").await.unwrap();
        assert_eq!(scope, "[object ServiceWorkerGlobalScope]");
    });
}

#[test]
fn sw_service_workers_should_return_current_workers() {
    run(|t| async move {
        serve_service_worker(&t);
        assert!(page_workers(&t).is_empty());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        t.context.on_service_worker(move |w| {
            if w.url().starts_with("http") {
                let _ = tx.send(w);
            }
        });
        t.page
            .goto(format!("{}/sw.html", t.server.prefix))
            .await
            .unwrap();
        let worker = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(page_workers(&t), vec![worker]);
        // Nothing leaks into the page.
        let leaked: JsValue = t
            .page
            .evaluate("() => Object.getOwnPropertyNames(window).filter(n => n.startsWith('__pw'))")
            .await
            .unwrap();
        assert_eq!(leaked.to_json(), json!([]));
    });
}

#[test]
fn cd_049_should_emit_event_for_each_cdp_event() {
    run(|t| async move {
        use futures::StreamExt;
        let session = t.context.new_cdp_session(&t.page).await.unwrap();
        session.send("Network.enable", json!({})).await.unwrap();
        let mut events = session.all_events().await.unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        let mut seen = 0;
        let request = loop {
            let event = tokio::time::timeout(Duration::from_secs(10), events.next())
                .await
                .unwrap()
                .unwrap();
            seen += 1;
            if event.method == "Network.requestWillBeSent" {
                break event;
            }
        };
        assert!(seen > 0);
        assert_eq!(request.params["request"]["url"], t.server.empty_page);
    });
}
