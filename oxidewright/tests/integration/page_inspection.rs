//! Ports of the Playwright 1.65 page inspection APIs:
//! - `page-event-console.spec.ts` (CM-016..019, `consoleMessages`),
//!   `page-event-pageerror.spec.ts` (ER-011, ER-012, `pageErrors`),
//!   `page-event-request.spec.ts` (NE-107, `requests`);
//! - `page-request-gc.spec.ts` (RO-149; RO-150 is `fixme` for Chromium);
//! - `page-localstorage.spec.ts` (ST-023..031);
//! - `page-set-content.spec.ts` (PA-025, `content({ includeShadow })`);
//! - `page-strict.spec.ts` (SM-004, `page.$` with `strict`);
//! - `page-aria-snapshot-json.spec.ts` (AR-096..106 except AR-101's
//!   `getByRef`) and the `boxes` tests of `page-aria-snapshot.spec.ts`
//!   (AR-042..045);
//! - `page-drop.spec.ts` (DD-021..027);
//! - `locator-highlight.spec.ts` (LO-193, LO-195..197, LO-199; the
//!   overlay lives in a closed shadow root, so it is inspected with
//!   `DOM.getDocument({ pierce: true })`; LO-194 is JS-only and LO-198
//!   needs `selectors.register`);
//! - `workers.spec.ts` tests about `evaluateHandle` and worker consoles,
//!   and `WebError.location()` / `page()`.
//!
//! Console messages and page errors need `console_events(true)`.

use std::future::IntoFuture;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use oxidewright::chromiumoxide::cdp::browser_protocol::dom::{GetDocumentParams, Node};
use oxidewright::{DropPayload, FilePayload, MessagesFilter, Page};
use serde_json::{Value, json};

use crate::common::*;

async fn console_page(t: &TestContext) -> (oxidewright::BrowserContext, Page) {
    let context = t.browser.new_context().console_events(true).await.unwrap();
    let page = context.new_page().await.unwrap();
    (context, page)
}

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64
}

// ---- console messages / page errors / requests ----

#[test]
fn cm_016_should_have_timestamp_in_console_messages() {
    run(|t| async move {
        let (context, page) = console_page(&t).await;
        let before = now_ms() - 100.0;
        page.evaluate::<()>("() => console.log('stored message')")
            .await
            .unwrap();
        let after = now_ms() + 100.0;
        page.wait_for_timeout(100).await.unwrap();
        let messages = page.console_messages().await.unwrap();
        let last = messages.last().expect("a message");
        assert_eq!(last.text(), "stored message");
        assert!(last.timestamp() >= before && last.timestamp() <= after);
        context.close().await.unwrap();
    });
}

#[test]
fn cm_017_console_messages_should_work() {
    run(|t| async move {
        let (context, page) = console_page(&t).await;
        page.evaluate::<()>("() => { for (let i = 0; i < 301; i++) console.log('message' + i); }")
            .await
            .unwrap();
        page.wait_for_timeout(500).await.unwrap();
        let messages = page.console_messages().await.unwrap();
        assert!(messages.len() >= 100);
        let tail: Vec<String> = messages[messages.len() - 100..]
            .iter()
            .map(|m| m.text().to_string())
            .collect();
        let expected: Vec<String> = (201..301).map(|i| format!("message{i}")).collect();
        assert_eq!(tail, expected);
        assert!(messages.iter().all(|m| m.r#type() == "log"));
        assert!(messages.iter().all(|m| m.page().as_ref() == Some(&page)));
        context.close().await.unwrap();
    });
}

#[test]
fn cm_018_clear_console_messages_should_work() {
    run(|t| async move {
        let (context, page) = console_page(&t).await;
        page.evaluate::<()>("() => { console.log('message1'); console.log('message2'); }")
            .await
            .unwrap();
        page.wait_for_timeout(100).await.unwrap();
        let texts: Vec<String> = page
            .console_messages()
            .await
            .unwrap()
            .iter()
            .map(|m| m.text().to_string())
            .collect();
        assert!(texts.contains(&"message1".to_string()));
        assert!(texts.contains(&"message2".to_string()));
        page.clear_console_messages().await.unwrap();
        assert!(page.console_messages().await.unwrap().is_empty());
        page.evaluate::<()>("() => console.log('message3')")
            .await
            .unwrap();
        page.wait_for_timeout(100).await.unwrap();
        let messages = page.console_messages().await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text(), "message3");
        context.close().await.unwrap();
    });
}

#[test]
fn cm_019_console_messages_since_navigation_filter_should_work() {
    run(|t| async move {
        let (context, page) = console_page(&t).await;
        page.evaluate::<()>("() => console.log('before navigation')")
            .await
            .unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        page.evaluate::<()>("() => console.log('after navigation')")
            .await
            .unwrap();
        page.wait_for_timeout(100).await.unwrap();
        let texts = |m: Vec<oxidewright::ConsoleMessage>| -> Vec<String> {
            m.iter().map(|m| m.text().to_string()).collect()
        };
        let all = texts(
            page.console_messages_with(MessagesFilter::All)
                .await
                .unwrap(),
        );
        assert!(all.contains(&"before navigation".to_string()));
        assert!(all.contains(&"after navigation".to_string()));
        let since = texts(page.console_messages().await.unwrap());
        assert!(!since.contains(&"before navigation".to_string()));
        assert!(since.contains(&"after navigation".to_string()));
        context.close().await.unwrap();
    });
}

#[test]
fn er_011_page_errors_should_work() {
    run(|t| async move {
        let (context, page) = console_page(&t).await;
        page.evaluate::<()>(
            "async () => { for (let i = 0; i < 301; i++) setTimeout(() => { throw new Error('error' + i); }, 0); await new Promise(f => setTimeout(f, 2000)); }",
        )
        .await
        .unwrap();
        page.wait_for_timeout(200).await.unwrap();
        let errors = page.page_errors().await.unwrap();
        assert!(errors.len() >= 100);
        let tail: Vec<String> = errors[errors.len() - 100..]
            .iter()
            .map(|e| e.message.clone())
            .collect();
        let expected: Vec<String> = (201..301).map(|i| format!("error{i}")).collect();
        assert_eq!(tail, expected);
        context.close().await.unwrap();
    });
}

#[test]
fn er_012_clear_page_errors_should_work() {
    run(|t| async move {
        let (context, page) = console_page(&t).await;
        page.evaluate::<()>(
            "() => { setTimeout(() => { throw new Error('error1'); }, 0); setTimeout(() => { throw new Error('error2'); }, 0); }",
        )
        .await
        .unwrap();
        page.wait_for_timeout(1000).await.unwrap();
        let messages: Vec<String> = page
            .page_errors()
            .await
            .unwrap()
            .iter()
            .map(|e| e.message.clone())
            .collect();
        assert!(messages.contains(&"error1".to_string()));
        assert!(messages.contains(&"error2".to_string()));
        page.clear_page_errors().await.unwrap();
        assert!(page.page_errors().await.unwrap().is_empty());
        page.evaluate::<()>("() => { setTimeout(() => { throw new Error('error3'); }, 0); }")
            .await
            .unwrap();
        page.wait_for_timeout(1000).await.unwrap();
        let errors = page.page_errors().await.unwrap();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("error3"));
        context.close().await.unwrap();
    });
}

#[test]
fn web_error_location_and_page() {
    run(|t| async move {
        let (context, page) = console_page(&t).await;
        page.goto(&t.server.empty_page).await.unwrap();
        let error = page
            .wait_for_page_error()
            .run(
                page.add_script_tag()
                    .content("\n\nthrow new Error('boom');")
                    .into_future(),
            )
            .await
            .unwrap();
        assert_eq!(error.message, "boom");
        assert_eq!(error.page().as_ref(), Some(&page));
        assert_eq!(error.location().line_number, 2);
        context.close().await.unwrap();
    });
}

#[test]
fn ne_107_should_return_last_requests() {
    run(|t| async move {
        t.page
            .goto(format!("{}/title.html", t.server.prefix))
            .await
            .unwrap();
        for i in 0..200 {
            let prefix = t.server.prefix.clone();
            t.server.set_route(&format!("/fetch?{i}"), move |req| {
                let body = format!("url:{prefix}{}", req.uri());
                async move { html_response(body) }
            });
        }
        // #0 is the navigation request, so start with #1.
        for i in 0..99 {
            t.page
                .evaluate::<()>("async url => { await fetch(url); }")
                .arg(format!("{}/fetch?{i}", t.server.prefix))
                .await
                .unwrap();
        }
        let mut first = t.page.requests().await.unwrap();
        first.remove(0);
        for i in 99..199 {
            t.page
                .evaluate::<()>("async url => { await fetch(url); }")
                .arg(format!("{}/fetch?{i}", t.server.prefix))
                .await
                .unwrap();
        }
        let last = t.page.requests().await.unwrap();
        let all: Vec<_> = first.into_iter().chain(last).collect();
        assert_eq!(all.len(), 199);
        for (i, request) in all.iter().enumerate() {
            let url = format!("{}/fetch?{i}", t.server.prefix);
            assert_eq!(request.url(), url);
            let response = request.response().await.unwrap().unwrap();
            assert_eq!(response.text().await.unwrap(), format!("url:{url}"));
        }
    });
}

#[test]
fn ro_149_request_gc_should_work() {
    run(|t| async move {
        t.page
            .evaluate::<()>("() => { globalThis.objectToDestroy = { hello: 'world' }; globalThis.weakRef = new WeakRef(globalThis.objectToDestroy); }")
            .await
            .unwrap();
        t.page.request_gc().await.unwrap();
        let v: Value = t
            .page
            .evaluate("() => globalThis.weakRef.deref()")
            .await
            .unwrap();
        assert_eq!(v, json!({ "hello": "world" }));
        t.page.request_gc().await.unwrap();
        let v: Value = t
            .page
            .evaluate("() => globalThis.weakRef.deref()")
            .await
            .unwrap();
        assert_eq!(v, json!({ "hello": "world" }));
        t.page
            .evaluate::<()>("() => { globalThis.objectToDestroy = null; }")
            .await
            .unwrap();
        t.page.request_gc().await.unwrap();
        let v: Option<Value> = t
            .page
            .evaluate("() => globalThis.weakRef.deref()")
            .await
            .unwrap();
        assert_eq!(v, None);
    });
}

// ---- localStorage / sessionStorage ----

fn nv(name: &str, value: &str) -> oxidewright::NameValue {
    serde_json::from_value(json!({ "name": name, "value": value })).unwrap()
}

fn sorted(mut items: Vec<oxidewright::NameValue>) -> Vec<oxidewright::NameValue> {
    items.sort_by(|a, b| a.name.cmp(&b.name));
    items
}

#[test]
fn st_023_local_storage_items_returns_empty_array_on_fresh_origin() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert!(t.page.local_storage().items().await.unwrap().is_empty());
    });
}

#[test]
fn st_024_local_storage_get_item_returns_null_for_missing_key() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(
            t.page.local_storage().get_item("absent").await.unwrap(),
            None
        );
    });
}

#[test]
fn st_025_local_storage_set_item_persists_and_surfaces_in_items_get_item() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let storage = t.page.local_storage();
        storage.set_item("alpha", "1").await.unwrap();
        storage.set_item("beta", "2").await.unwrap();
        assert_eq!(
            sorted(storage.items().await.unwrap()),
            vec![nv("alpha", "1"), nv("beta", "2")]
        );
        assert_eq!(
            storage.get_item("alpha").await.unwrap().as_deref(),
            Some("1")
        );
        let v: String = t
            .page
            .evaluate("() => localStorage.getItem('alpha')")
            .await
            .unwrap();
        assert_eq!(v, "1");
    });
}

#[test]
fn st_026_local_storage_set_item_overwrites_existing_value() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let storage = t.page.local_storage();
        storage.set_item("k", "first").await.unwrap();
        storage.set_item("k", "second").await.unwrap();
        assert_eq!(
            storage.get_item("k").await.unwrap().as_deref(),
            Some("second")
        );
    });
}

#[test]
fn st_027_local_storage_remove_item_removes_a_single_item() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let storage = t.page.local_storage();
        storage.set_item("a", "1").await.unwrap();
        storage.set_item("b", "2").await.unwrap();
        storage.remove_item("a").await.unwrap();
        assert_eq!(storage.items().await.unwrap(), vec![nv("b", "2")]);
    });
}

#[test]
fn st_028_local_storage_clear_empties_storage() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let storage = t.page.local_storage();
        storage.set_item("a", "1").await.unwrap();
        storage.set_item("b", "2").await.unwrap();
        storage.clear().await.unwrap();
        assert!(storage.items().await.unwrap().is_empty());
    });
}

#[test]
fn st_029_session_storage_round_trip() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let storage = t.page.session_storage();
        assert!(storage.items().await.unwrap().is_empty());
        storage.set_item("s1", "v1").await.unwrap();
        storage.set_item("s2", "v2").await.unwrap();
        assert_eq!(
            sorted(storage.items().await.unwrap()),
            vec![nv("s1", "v1"), nv("s2", "v2")]
        );
        assert_eq!(storage.get_item("s1").await.unwrap().as_deref(), Some("v1"));
        storage.remove_item("s1").await.unwrap();
        assert_eq!(storage.items().await.unwrap(), vec![nv("s2", "v2")]);
        storage.clear().await.unwrap();
        assert!(storage.items().await.unwrap().is_empty());
    });
}

#[test]
fn st_030_local_storage_and_session_storage_are_independent() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .local_storage()
            .set_item("shared", "local")
            .await
            .unwrap();
        t.page
            .session_storage()
            .set_item("shared", "session")
            .await
            .unwrap();
        assert_eq!(
            t.page
                .local_storage()
                .get_item("shared")
                .await
                .unwrap()
                .as_deref(),
            Some("local")
        );
        assert_eq!(
            t.page
                .session_storage()
                .get_item("shared")
                .await
                .unwrap()
                .as_deref(),
            Some("session")
        );
        t.page.local_storage().clear().await.unwrap();
        assert!(t.page.local_storage().items().await.unwrap().is_empty());
        assert_eq!(
            t.page
                .session_storage()
                .get_item("shared")
                .await
                .unwrap()
                .as_deref(),
            Some("session")
        );
    });
}

#[test]
fn st_031_storage_methods_are_scoped_to_the_current_origin() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .local_storage()
            .set_item("k", "origin-1")
            .await
            .unwrap();
        t.page
            .goto(format!("{}/empty.html", t.server.cross_process_prefix))
            .await
            .unwrap();
        assert!(t.page.local_storage().items().await.unwrap().is_empty());
        t.page
            .local_storage()
            .set_item("k", "origin-2")
            .await
            .unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(
            t.page
                .local_storage()
                .get_item("k")
                .await
                .unwrap()
                .as_deref(),
            Some("origin-1")
        );
    });
}

#[test]
fn storage_calls_are_not_observed_by_the_page() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .evaluate::<()>("() => { window.calls = 0; const original = Storage.prototype.setItem; Storage.prototype.setItem = function(...args) { window.calls++; return original.apply(this, args); }; }")
            .await
            .unwrap();
        t.page.local_storage().set_item("k", "v").await.unwrap();
        let calls: i32 = t.page.evaluate("() => window.calls").await.unwrap();
        assert_eq!(calls, 0);
        let v: String = t
            .page
            .evaluate("() => localStorage.getItem('k')")
            .await
            .unwrap();
        assert_eq!(v, "v");
    });
}

// ---- content / strict ----

#[test]
fn pa_025_should_include_shadow_roots() {
    run(|t| async move {
        let html = "<!DOCTYPE html><html lang=\"en\"><head></head><body><div id=\"host\"><template shadowrootmode=\"open\"><div id=\"nested\"><template shadowrootmode=\"open\"><span>nested</span></template><slot></slot></div></template><span>light</span></div><div id=\"closed\"></div></body></html>";
        t.page
            .set_content(html.replace(
                "<div id=\"closed\">",
                "<div id=\"closed\"><template shadowrootmode=\"closed\"><span>closed</span></template>",
            ))
            .await
            .unwrap();
        assert_eq!(
            t.page.content().await.unwrap(),
            "<!DOCTYPE html><html lang=\"en\"><head></head><body><div id=\"host\"><span>light</span></div><div id=\"closed\"></div></body></html>"
        );
        assert_eq!(t.page.content().include_shadow(true).await.unwrap(), html);
    });
}

#[test]
fn sm_004_should_fail_page_query_selector_in_strict_mode() {
    run(|t| async move {
        t.page
            .set_content("<span>span1</span><div><span>target</span></div>")
            .await
            .unwrap();
        let r = t.page.query_selector("span").strict(true).await;
        assert_err_contains(r, "strict mode violation");
        assert!(t.page.query_selector("span").await.unwrap().is_some());
    });
}

// ---- aria snapshots ----

fn find_node<'a>(nodes: &'a Value, predicate: &dyn Fn(&Value) -> bool) -> Option<&'a Value> {
    for node in nodes.as_array()? {
        if !node.is_object() {
            continue;
        }
        if predicate(node) {
            return Some(node);
        }
        if let Some(found) = node.get("children").and_then(|c| find_node(c, predicate)) {
            return Some(found);
        }
    }
    None
}

#[test]
fn ar_096_should_snapshot_roles_names_and_text() {
    run(|t| async move {
        t.page
            .set_content("<h1>title</h1><ul aria-label=\"my list\"><li>one</li><li>two</li></ul>")
            .await
            .unwrap();
        assert_eq!(
            t.page.aria_snapshot_json().await.unwrap(),
            json!([
                { "role": "heading", "name": "title", "level": 1 },
                { "role": "list", "name": "my list", "children": [
                    { "role": "listitem", "text": "one" },
                    { "role": "listitem", "text": "two" },
                ]},
            ])
        );
    });
}

#[test]
fn ar_097_should_snapshot_flags_as_properties() {
    run(|t| async move {
        t.page
            .set_content(
                "<input type=\"checkbox\" title=\"Check\" checked><button disabled>Click</button>",
            )
            .await
            .unwrap();
        assert_eq!(
            t.page.aria_snapshot_json().await.unwrap(),
            json!([
                { "role": "checkbox", "name": "Check", "checked": true },
                { "role": "button", "name": "Click", "disabled": true },
            ])
        );
    });
}

#[test]
fn ar_098_should_snapshot_link_url_and_textbox_value() {
    run(|t| async move {
        t.page
            .set_content(
                "<a href=\"https://example.com/\">Link</a><input title=\"Input\" value=\"hello\">",
            )
            .await
            .unwrap();
        assert_eq!(
            t.page.aria_snapshot_json().await.unwrap(),
            json!([
                { "role": "link", "name": "Link", "url": "https://example.com/" },
                { "role": "textbox", "name": "Input", "text": "hello" },
            ])
        );
    });
}

#[test]
fn ar_099_should_snapshot_text_fragments_in_children() {
    run(|t| async move {
        t.page
            .set_content("<p>Hello <a href=\"/link\">world</a> again</p>")
            .await
            .unwrap();
        assert_eq!(
            t.page.aria_snapshot_json().await.unwrap(),
            json!([
                { "role": "paragraph", "children": [
                    "Hello",
                    { "role": "link", "name": "world", "url": "/link" },
                    "again",
                ]},
            ])
        );
    });
}

#[test]
fn ar_100_should_snapshot_top_level_text_fragments_as_text_nodes() {
    run(|t| async move {
        t.page
            .set_content("Hello <button>One</button> again")
            .await
            .unwrap();
        assert_eq!(
            t.page.aria_snapshot_json().await.unwrap(),
            json!([
                { "role": "text", "text": "Hello" },
                { "role": "button", "name": "One" },
                { "role": "text", "text": "again" },
            ])
        );
    });
}

#[test]
fn ar_101_should_generate_refs_in_ai_mode() {
    run(|t| async move {
        t.page
            .set_content("<button>One</button><button>Two</button>")
            .await
            .unwrap();
        assert_eq!(
            t.page.aria_snapshot_json().mode("ai").await.unwrap(),
            json!([
                { "role": "generic", "active": true, "ref": "e1", "children": [
                    { "role": "button", "name": "One", "ref": "e2" },
                    { "role": "button", "name": "Two", "ref": "e3" },
                ]},
            ])
        );
    });
}

#[test]
fn ar_102_should_mark_clickable_elements_with_cursor_in_ai_mode() {
    run(|t| async move {
        t.page
            .set_content("<button style=\"cursor: pointer\">One</button>")
            .await
            .unwrap();
        let json = t.page.aria_snapshot_json().mode("ai").await.unwrap();
        let button = find_node(&json, &|n| n["role"] == "button").unwrap();
        assert_eq!(button["cursor"], "pointer");
    });
}

#[test]
fn ar_103_should_snapshot_iframes_in_ai_mode() {
    run(|t| async move {
        t.page
            .set_content("<h1>Hello</h1><iframe srcdoc=\"<button>In frame</button>\"></iframe>")
            .await
            .unwrap();
        // Let the iframe load.
        t.page
            .wait_for_function(
                "() => document.querySelector('iframe').contentDocument?.querySelector('button')",
            )
            .await
            .unwrap();
        let json = t.page.aria_snapshot_json().mode("ai").await.unwrap();
        let iframe = find_node(&json, &|n| n["role"] == "iframe").unwrap();
        assert!(iframe["ref"].as_str().is_some_and(|r| !r.is_empty()));
        let button = find_node(&iframe["children"], &|n| n["role"] == "button").unwrap();
        assert_eq!(button["name"], "In frame");
        let r = button["ref"].as_str().unwrap();
        assert!(regex::Regex::new(r"^f\d+e\d+$").unwrap().is_match(r), "{r}");
        // The YAML rendering includes the frame content too.
        let yaml = t.page.aria_snapshot().mode("ai").await.unwrap();
        assert!(yaml.contains("button \"In frame\""), "{yaml}");
    });
}

#[test]
fn ar_104_should_limit_depth() {
    run(|t| async move {
        t.page
            .set_content("<ul><li><button>One</button></li></ul>")
            .await
            .unwrap();
        assert_eq!(
            t.page.aria_snapshot_json().depth(1).await.unwrap(),
            json!([{ "role": "list", "children": [{ "role": "listitem" }] }])
        );
    });
}

#[test]
fn ar_105_should_include_boxes_when_requested() {
    run(|t| async move {
        t.page.set_content("<button>One</button>").await.unwrap();
        let json = t.page.aria_snapshot_json().boxes(true).await.unwrap();
        let button = find_node(&json, &|n| n["role"] == "button").unwrap();
        let b = &button["box"];
        assert!(b["x"].is_number() && b["y"].is_number());
        assert!(b["width"].as_f64().unwrap() > 0.0);
        assert!(b["height"].as_f64().unwrap() > 0.0);
    });
}

#[test]
fn ar_106_should_snapshot_a_locator() {
    run(|t| async move {
        t.page
            .set_content("<h1>title</h1><ul><li>one</li><li>two</li></ul>")
            .await
            .unwrap();
        assert_eq!(
            t.page.locator("ul").aria_snapshot_json().await.unwrap(),
            json!([{ "role": "list", "children": [
                { "role": "listitem", "text": "one" },
                { "role": "listitem", "text": "two" },
            ]}])
        );
    });
}

#[test]
fn ar_042_should_snapshot_with_box_from_page() {
    run(|t| async move {
        t.page
            .set_content("<button style=\"position:absolute;left:100px;top:50px;width:80px;height:40px;margin:0;padding:0;border:0;\">click</button>")
            .await
            .unwrap();
        assert_eq!(
            t.page.aria_snapshot().boxes(true).await.unwrap(),
            "- button \"click\" [box=100,50,80,40]"
        );
    });
}

#[test]
fn ar_043_should_snapshot_with_box_from_locator() {
    run(|t| async move {
        t.page
            .set_content("<div style=\"position:absolute;left:10px;top:20px;width:200px;height:100px;\"><button style=\"position:absolute;left:5px;top:5px;width:60px;height:30px;margin:0;padding:0;border:0;\">ok</button></div>")
            .await
            .unwrap();
        assert_eq!(
            t.page
                .locator("div")
                .aria_snapshot()
                .boxes(true)
                .await
                .unwrap(),
            "- button \"ok\" [box=15,25,60,30]"
        );
    });
}

#[test]
fn ar_044_should_not_include_box_when_option_is_omitted() {
    run(|t| async move {
        t.page.set_content("<button>click</button>").await.unwrap();
        let snapshot = t.page.aria_snapshot().await.unwrap();
        assert_eq!(snapshot, "- button \"click\"");
    });
}

#[test]
fn ar_045_should_not_include_hidden_iframes() {
    run(|t| async move {
        t.page
            .set_content("<iframe name=\"visible\" srcdoc=\"<button>Visible</button>\"></iframe><iframe name=\"hidden\" style=\"visibility: hidden\" srcdoc=\"<button>Hidden</button>\"></iframe><iframe name=\"none\" style=\"display: none\" srcdoc=\"<button>None</button>\"></iframe>")
            .await
            .unwrap();
        assert_eq!(t.page.aria_snapshot().await.unwrap(), "- iframe");
    });
}

// ---- drop ----

const DROPZONE: &str = r#"
<style>#dropzone { width: 300px; height: 200px; border: 2px dashed #888; }</style>
<div id="dropzone"></div>
<script>
  window.__dropInfo = null;
  const zone = document.getElementById('dropzone');
  zone.addEventListener('dragenter', e => e.preventDefault());
  zone.addEventListener('dragover', e => e.preventDefault());
  zone.addEventListener('drop', async e => {
    e.preventDefault();
    const files = [];
    for (const file of e.dataTransfer.files)
      files.push({ name: file.name, type: file.type, size: file.size, text: await file.text() });
    const data = {};
    for (const t of e.dataTransfer.types) {
      if (t !== 'Files')
        data[t] = e.dataTransfer.getData(t);
    }
    window.__dropInfo = { files, data };
  });
</script>"#;

async fn drop_info(page: &Page) -> Value {
    page.wait_for_function("() => window.__dropInfo")
        .await
        .unwrap()
        .json_value()
        .await
        .unwrap()
}

#[test]
fn dd_021_should_drop_a_file_payload() {
    run(|t| async move {
        t.page.set_content(DROPZONE).await.unwrap();
        t.page
            .locator("#dropzone")
            .drop(DropPayload::new().files(FilePayload::new(
                "note.txt",
                "text/plain",
                b"hello".to_vec(),
            )))
            .await
            .unwrap();
        assert_eq!(
            drop_info(&t.page).await,
            json!({ "files": [{ "name": "note.txt", "type": "text/plain", "size": 5, "text": "hello" }], "data": {} })
        );
    });
}

#[test]
fn dd_022_should_drop_multiple_file_payloads() {
    run(|t| async move {
        t.page.set_content(DROPZONE).await.unwrap();
        t.page
            .locator("#dropzone")
            .drop(DropPayload::new().files(vec![
                FilePayload::new("a.txt", "text/plain", b"AAA".to_vec()),
                FilePayload::new("b.txt", "text/plain", b"BB".to_vec()),
            ]))
            .await
            .unwrap();
        let info = drop_info(&t.page).await;
        let files: Vec<(String, String)> = info["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                (
                    f["name"].as_str().unwrap().into(),
                    f["text"].as_str().unwrap().into(),
                )
            })
            .collect();
        assert_eq!(
            files,
            vec![
                ("a.txt".into(), "AAA".into()),
                ("b.txt".into(), "BB".into())
            ]
        );
    });
}

#[test]
fn dd_023_should_drop_a_file_by_local_path() {
    run(|t| async move {
        t.page.set_content(DROPZONE).await.unwrap();
        let dir = std::env::temp_dir().join(format!("oxidewright-drop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hello.txt");
        std::fs::write(&path, "path-content").unwrap();
        t.page
            .locator("#dropzone")
            .drop(DropPayload::new().files(path.clone()))
            .await
            .unwrap();
        let info = drop_info(&t.page).await;
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(info["files"].as_array().unwrap().len(), 1);
        assert_eq!(info["files"][0]["name"], "hello.txt");
        assert_eq!(info["files"][0]["text"], "path-content");
    });
}

#[test]
fn dd_024_should_drop_clipboard_like_data() {
    run(|t| async move {
        t.page.set_content(DROPZONE).await.unwrap();
        t.page
            .locator("#dropzone")
            .drop(
                DropPayload::new()
                    .data("text/plain", "hello world")
                    .data("text/uri-list", "https://example.com"),
            )
            .await
            .unwrap();
        let info = drop_info(&t.page).await;
        assert_eq!(info["files"], json!([]));
        assert_eq!(info["data"]["text/plain"], "hello world");
        assert_eq!(info["data"]["text/uri-list"], "https://example.com");
    });
}

#[test]
fn dd_025_should_drop_files_and_data_together() {
    run(|t| async move {
        t.page.set_content(DROPZONE).await.unwrap();
        t.page
            .locator("#dropzone")
            .drop(
                DropPayload::new()
                    .files(FilePayload::new("mix.txt", "text/plain", b"mix".to_vec()))
                    .data("text/plain", "label"),
            )
            .await
            .unwrap();
        let info = drop_info(&t.page).await;
        assert_eq!(info["files"][0]["text"], "mix");
        assert_eq!(info["data"]["text/plain"], "label");
    });
}

#[test]
fn dd_026_should_throw_when_target_does_not_accept_drop() {
    run(|t| async move {
        t.page
            .set_content("<div id=\"dropzone\" style=\"width: 200px; height: 100px;\"></div>")
            .await
            .unwrap();
        let r = t
            .page
            .locator("#dropzone")
            .drop(DropPayload::new().data("text/plain", "nope"))
            .timeout(5000)
            .await;
        assert_err_contains(r, "Drop target did not accept the drop");
    });
}

#[test]
fn dd_027_should_throw_when_neither_files_nor_data_provided() {
    run(|t| async move {
        t.page.set_content(DROPZONE).await.unwrap();
        let r = t.page.locator("#dropzone").drop(DropPayload::new()).await;
        assert_err_contains(r, "At least one of \"files\" or \"data\"");
    });
}

// ---- highlight ----

fn collect_nodes<'a>(node: &'a Node, name: &str, out: &mut Vec<&'a Node>) {
    if node.node_name.eq_ignore_ascii_case(name) {
        out.push(node);
    }
    for child in node.children.iter().flatten() {
        collect_nodes(child, name, out);
    }
    for root in node.shadow_roots.iter().flatten() {
        collect_nodes(root, name, out);
    }
    if let Some(doc) = &node.content_document {
        collect_nodes(doc, name, out);
    }
}

/// The `style` attributes of the `x-pw-highlight` boxes of the page
/// (they live in a closed shadow root).
async fn highlights(page: &Page) -> Vec<String> {
    let params = GetDocumentParams {
        depth: Some(-1),
        pierce: Some(true),
    };
    let root = page.cdp().execute(params).await.unwrap().result.root;
    let mut nodes = Vec::new();
    collect_nodes(&root, "x-pw-highlight", &mut nodes);
    nodes
        .iter()
        .map(|n| {
            let attrs = n.attributes.clone().unwrap_or_default();
            attrs
                .chunks(2)
                .find(|kv| kv[0] == "style")
                .map(|kv| kv[1].clone())
                .unwrap_or_default()
        })
        .collect()
}

async fn wait_highlights(page: &Page, count: usize) -> Vec<String> {
    for _ in 0..50 {
        let found = highlights(page).await;
        if found.len() == count {
            return found;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let found = highlights(page).await;
    assert_eq!(found.len(), count, "{found:?}");
    found
}

#[test]
fn lo_193_highlight_should_accept_a_css_string_style() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/button.html", t.server.prefix))
            .await
            .unwrap();
        t.page
            .get_by_role("button")
            .highlight()
            .style("outline: 3px solid rgb(255, 0, 0); background-color: rgba(0, 255, 0, 0.25)")
            .await
            .unwrap();
        let styles = wait_highlights(&t.page, 1).await;
        assert!(
            styles[0].contains("outline: rgb(255, 0, 0) solid 3px")
                || styles[0].contains("3px solid"),
            "{styles:?}"
        );
        assert!(styles[0].contains("rgba(0, 255, 0, 0.25)"), "{styles:?}");
    });
}

#[test]
fn lo_195_hide_highlight_removes_a_styled_highlight() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/button.html", t.server.prefix))
            .await
            .unwrap();
        let button = t.page.get_by_role("button");
        button
            .highlight()
            .style("outline: 2px solid red")
            .await
            .unwrap();
        wait_highlights(&t.page, 1).await;
        button.hide_highlight().await.unwrap();
        wait_highlights(&t.page, 0).await;
    });
}

#[test]
fn lo_196_highlight_should_survive_navigation() {
    run(|t| async move {
        t.page
            .set_content("<button onclick=\"console.log(1)\">Clicker</button>")
            .await
            .unwrap();
        t.page.get_by_role("button").highlight().await.unwrap();
        wait_highlights(&t.page, 1).await;
        t.page
            .goto(format!("{}/input/button.html", t.server.prefix))
            .await
            .unwrap();
        wait_highlights(&t.page, 1).await;
        t.page.hide_highlight().await.unwrap();
        wait_highlights(&t.page, 0).await;
    });
}

#[test]
fn lo_197_page_hide_highlight_clears_all_locator_highlights() {
    run(|t| async move {
        t.page
            .set_content("<button>One</button><button>Two</button>")
            .await
            .unwrap();
        t.page
            .get_by_role_with("button", oxidewright::GetByRoleOptions::new().name("One"))
            .highlight()
            .await
            .unwrap();
        t.page
            .get_by_role_with("button", oxidewright::GetByRoleOptions::new().name("Two"))
            .highlight()
            .await
            .unwrap();
        wait_highlights(&t.page, 2).await;
        t.page.hide_highlight().await.unwrap();
        wait_highlights(&t.page, 0).await;
    });
}

#[test]
fn lo_199_highlight_should_resolve_relative_to_the_frame_of_the_locator() {
    run(|t| async move {
        t.page
            .set_content("<iframe name=\"frame\" srcdoc=\"<button>foo</button>\"></iframe><button>bar</button>")
            .await
            .unwrap();
        t.page
            .wait_for_function(
                "() => document.querySelector('iframe').contentDocument?.querySelector('button')",
            )
            .await
            .unwrap();
        let frame = t.page.frame("frame").unwrap();
        t.page.locator("button").highlight().await.unwrap();
        frame.locator("button").highlight().await.unwrap();
        wait_highlights(&t.page, 2).await;
        t.page.locator("button").hide_highlight().await.unwrap();
        wait_highlights(&t.page, 1).await;
        frame.locator("button").hide_highlight().await.unwrap();
        wait_highlights(&t.page, 0).await;
    });
}

// ---- workers ----

#[test]
fn wo_should_evaluate_handle_in_worker() {
    run(|t| async move {
        let worker = t
            .page
            .expect_worker(t.page.evaluate::<()>(
                "() => { window.w = new Worker(URL.createObjectURL(new Blob(['self.data = { answer: 42 }'], { type: 'application/javascript' }))); }",
            ))
            .await
            .unwrap();
        let handle = worker.evaluate_handle("() => self.data").await.unwrap();
        assert_eq!(
            handle.json_value::<Value>().await.unwrap(),
            json!({ "answer": 42 })
        );
        let answer: i32 = worker
            .evaluate("data => data.answer")
            .arg(&handle)
            .await
            .unwrap();
        assert_eq!(answer, 42);
        let undefined: Option<i32> = worker.evaluate("() => undefined").await.unwrap();
        assert_eq!(undefined, None);
    });
}

#[test]
fn wo_should_report_console_logs() {
    run(|t| async move {
        let (context, page) = console_page(&t).await;
        let worker = page
            .expect_worker(page.evaluate::<()>(
                "() => { window.w = new Worker(URL.createObjectURL(new Blob(['onmessage = e => console.log(e.data)'], { type: 'application/javascript' }))); }",
            ))
            .await
            .unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = std::sync::Mutex::new(Some(tx));
        worker.on_console(move |message| {
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(message);
            }
        });
        // Give the worker session time to enable its console.
        page.wait_for_timeout(500).await.unwrap();
        let page_message = page
            .expect_console_message(page.evaluate::<()>("() => w.postMessage('from worker')"))
            .await
            .unwrap();
        assert_eq!(page_message.text(), "from worker");
        assert_eq!(page_message.worker().as_ref(), Some(&worker));
        let message = tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(message.text(), "from worker");
        assert_eq!(message.r#type(), "log");
        let page_logs: Vec<_> = page
            .console_messages()
            .await
            .unwrap()
            .into_iter()
            .filter(|m| m.text() == "from worker")
            .collect();
        assert_eq!(page_logs.len(), 1);
        context.close().await.unwrap();
    });
}
