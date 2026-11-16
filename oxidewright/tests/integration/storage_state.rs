//! Storage state, ported from Playwright's
//! `tests/library/browsercontext-storage-state.spec.ts`.
//!
//! The originals use `page.route` to serve `https://www.example.com`; these
//! ports use the two origins of the local test server instead.
//!
//! Not ported:
//! * ST-005 (OPFS), ST-007 (context events), ST-011..ST-015 (service workers,
//!   third-party frames, lone surrogates through routes), ST-016..ST-019 (need
//!   locators / the IndexedDB demo app), ST-020..ST-021 (WebAuthn credentials).

use oxidewright::{NameValue, OriginState, StorageState};
use serde_json::{Value, json};

use crate::common::*;

async fn set_local_storage(page: &oxidewright::Page, name: &str, value: &str) {
    page.evaluate::<()>("([name, value]) => { localStorage[name] = value; }")
        .arg((name, value))
        .await
        .unwrap();
}

async fn local_storage(page: &oxidewright::Page) -> Value {
    page.evaluate("() => Object.fromEntries(Object.entries(window.localStorage))")
        .await
        .unwrap()
}

fn origin_state(origin: &str, name: &str, value: &str) -> OriginState {
    OriginState {
        origin: origin.to_string(),
        local_storage: vec![NameValue {
            name: name.into(),
            value: value.into(),
        }],
        indexed_db: None,
        opfs: None,
    }
}

#[test]
fn st_001_should_capture_local_storage() {
    run(|t| async move {
        let page = &t.page;
        page.goto(&t.server.empty_page).await.unwrap();
        set_local_storage(page, "name1", "value1").await;
        page.goto(format!("{}/empty.html", t.server.cross_process_prefix))
            .await
            .unwrap();
        set_local_storage(page, "name2", "value2").await;
        let mut origins = t.context.storage_state().await.unwrap().origins;
        origins.sort_by(|a, b| a.origin.cmp(&b.origin));
        assert_eq!(
            origins,
            vec![
                origin_state(&t.server.cross_process_prefix, "name2", "value2"),
                origin_state(&t.server.prefix, "name1", "value1"),
            ]
        );
    });
}

#[test]
fn st_002_should_set_local_storage() {
    run(|t| async move {
        let state = StorageState {
            cookies: vec![],
            origins: vec![origin_state(&t.server.prefix, "name1", "value1")],
        };
        let context = t.browser.new_context().storage_state(state).await.unwrap();
        assert!(context.pages().is_empty());
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(local_storage(&page).await, json!({ "name1": "value1" }));

        context
            .set_storage_state(StorageState {
                cookies: vec![],
                origins: vec![origin_state(&t.server.prefix, "name2", "value2")],
            })
            .await
            .unwrap();
        assert_eq!(context.pages().len(), 1);
        page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(local_storage(&page).await, json!({ "name2": "value2" }));
        context.close().await.unwrap();
    });
}

#[test]
fn st_003_should_report_good_error_if_the_url_is_not_valid() {
    run(|t| async move {
        let state = StorageState {
            cookies: vec![],
            origins: vec![origin_state("foo", "name1", "value1")],
        };
        let err = assert_err_contains(
            t.browser.new_context().storage_state(state).await,
            "browser.newContext: Error setting storage state:",
        );
        assert!(err.to_string().contains("foo"), "{err}");
    });
}

#[test]
fn st_004_should_round_trip_through_the_file() {
    run(|t| async move {
        let page = &t.page;
        page.goto(&t.server.empty_page).await.unwrap();
        let cookie: String = page
            .evaluate(
                "async () => {
                  localStorage['name1'] = 'value1';
                  document.cookie = 'username=John Doe';
                  await new Promise((resolve, reject) => {
                    const openRequest = indexedDB.open('db', 42);
                    openRequest.onupgradeneeded = () => {
                      openRequest.result.createObjectStore('store', { keyPath: 'name' });
                      openRequest.result.createObjectStore('store2');
                    };
                    openRequest.onsuccess = () => {
                      const transaction = openRequest.result.transaction(['store', 'store2'], 'readwrite');
                      transaction.objectStore('store').put({ name: 'foo', date: new Date(0), null: null });
                      transaction.objectStore('store2').put(new TextEncoder().encode('bar'), 'foo');
                      transaction.addEventListener('complete', resolve);
                      transaction.addEventListener('error', reject);
                    };
                  });
                  return document.cookie;
                }",
            )
            .await
            .unwrap();
        assert_eq!(cookie, "username=John Doe");
        let dir = std::env::temp_dir().join(format!("oxidewright-st004-{}", t.server.port));
        let path = dir.join("storage-state.json");
        let state = t
            .context
            .storage_state()
            .path(&path)
            .indexed_db(true)
            .await
            .unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(serde_json::to_string_pretty(&state).unwrap(), written);

        let check = |context: oxidewright::BrowserContext| {
            let empty = t.server.empty_page.clone();
            async move {
                let page = context.new_page().await.unwrap();
                page.goto(&empty).await.unwrap();
                assert_eq!(local_storage(&page).await, json!({ "name1": "value1" }));
                let cookie: String = page.evaluate("() => document.cookie").await.unwrap();
                assert_eq!(cookie, "username=John Doe");
                let values: Value = page
                    .evaluate(
                        "() => new Promise((resolve, reject) => {
                          const openRequest = indexedDB.open('db', 42);
                          openRequest.addEventListener('success', async () => {
                            const db = openRequest.result;
                            const transaction = db.transaction(['store', 'store2'], 'readonly');
                            const request1 = transaction.objectStore('store').get('foo');
                            const request2 = transaction.objectStore('store2').get('foo');
                            const [result1, result2] = await Promise.all([request1, request2].map(request => new Promise((resolve, reject) => {
                              request.addEventListener('success', () => resolve(request.result));
                              request.addEventListener('error', () => reject(request.error));
                            })));
                            resolve([result1.name, result1.date.getTime(), result1.null, new TextDecoder().decode(result2)]);
                          });
                          openRequest.addEventListener('error', () => reject(openRequest.error));
                        })",
                    )
                    .await
                    .unwrap();
                assert_eq!(values, json!(["foo", 0, null, "bar"]));
                context.close().await.unwrap();
            }
        };
        let context2 = t
            .browser
            .new_context()
            .storage_state(path.clone())
            .await
            .unwrap();
        check(context2).await;
        let context3 = t.browser.new_context().await.unwrap();
        context3.set_storage_state(path.as_path()).await.unwrap();
        assert!(context3.pages().is_empty());
        check(context3).await;
        let _ = std::fs::remove_dir_all(dir);
    });
}

#[test]
fn st_006_should_capture_cookies() {
    run(|t| async move {
        t.server.set_route("/setcookie.html", |_| async {
            axum::response::Response::builder()
                .header("Set-Cookie", "a=b")
                .header("Set-Cookie", "empty=")
                .body(axum::body::Body::empty())
                .unwrap()
        });
        t.page
            .goto(format!("{}/setcookie.html", t.server.prefix))
            .await
            .unwrap();
        let read = "() => document.cookie.split(';').map(cookie => cookie.trim()).sort()";
        let cookies: Vec<String> = t.page.evaluate(read).await.unwrap();
        assert_eq!(cookies, ["a=b", "empty="]);
        let state = t.context.storage_state().await.unwrap();
        let mut pairs: Vec<(String, String)> = state
            .cookies
            .iter()
            .map(|c| (c.name.clone(), c.value.clone()))
            .collect();
        pairs.sort();
        assert_eq!(
            pairs,
            [("a".into(), "b".into()), ("empty".into(), String::new())]
        );
        let context2 = t.browser.new_context().storage_state(state).await.unwrap();
        let page2 = context2.new_page().await.unwrap();
        page2.goto(&t.server.empty_page).await.unwrap();
        let cookies: Vec<String> = page2.evaluate(read).await.unwrap();
        assert_eq!(cookies, ["a=b", "empty="]);
        context2.close().await.unwrap();
    });
}

#[test]
fn st_008_should_not_restore_local_storage_twice() {
    run(|t| async move {
        let state = StorageState {
            cookies: vec![],
            origins: vec![origin_state(&t.server.prefix, "name1", "value1")],
        };
        let context = t.browser.new_context().storage_state(state).await.unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(local_storage(&page).await, json!({ "name1": "value1" }));
        set_local_storage(&page, "name1", "value2").await;
        page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(local_storage(&page).await, json!({ "name1": "value2" }));
        context.close().await.unwrap();
    });
}

#[test]
fn st_009_should_handle_missing_file() {
    run(|t| async move {
        let file = std::env::temp_dir().join("oxidewright-does-not-exist.json");
        assert_err_contains(
            t.browser.new_context().storage_state(file.clone()).await,
            &format!("Error reading storage state from {}:\n", file.display()),
        );
    });
}

#[test]
fn st_010_should_handle_malformed_file() {
    run(|t| async move {
        let file = std::env::temp_dir().join(format!("oxidewright-state-{}.json", t.server.port));
        std::fs::write(&file, "not-json").unwrap();
        assert_err_contains(
            t.browser.new_context().storage_state(file.clone()).await,
            &format!("Error reading storage state from {}:\n", file.display()),
        );
        let _ = std::fs::remove_file(file);
    });
}

#[test]
fn st_022_set_storage_state_should_handle_missing_file() {
    run(|t| async move {
        let file = std::env::temp_dir().join("oxidewright-does-not-exist-2.json");
        assert_err_contains(
            t.context.set_storage_state(file.as_path()).await,
            "Error reading storage state from",
        );
    });
}
