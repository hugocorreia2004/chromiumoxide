//! Ports of `page-wait-for-function.spec.ts` and
//! `locator-wait-for-function.spec.ts`.
//!
//! Skipped:
//! - WA-006 / WA-012 test invalid `polling` values typed in JavaScript; the
//!   Rust `Polling` type only accepts `"raf"`, so `polling("mutation")` is
//!   checked instead (same error).
//! - WA-011 (`server.setCSP`, not in the test server).
//! - LO-191 / LO-192 (`AbortSignal`: in Rust the future is dropped).

use std::future::IntoFuture;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use oxidewright::EventKind;

use crate::common::*;

#[test]
fn wa_001_should_timeout() {
    run(|t| async move {
        let start = Instant::now();
        t.page.wait_for_timeout(42).await.unwrap();
        assert!(start.elapsed() >= Duration::from_millis(21));
    });
}

#[test]
fn wa_002_should_accept_a_string() {
    run(|t| async move {
        let watchdog = tokio::spawn(t.page.wait_for_function("window.__FOO === 1").into_future());
        t.page
            .evaluate::<()>("() => { window['__FOO'] = 1; }")
            .await
            .unwrap();
        watchdog.await.unwrap().unwrap();
    });
}

#[test]
fn wa_003_should_work_when_resolved_right_before_execution_context_disposal() {
    run(|t| async move {
        t.page
            .add_init_script("window['__RELOADED'] = true")
            .await
            .unwrap();
        t.page
            .wait_for_function(
                "() => { if (!window['__RELOADED']) window.location.reload(); return true; }",
            )
            .await
            .unwrap();
    });
}

#[test]
fn wa_004_should_poll_on_interval() {
    run(|t| async move {
        let polling = 100;
        let delta = t
            .page
            .wait_for_function(
                "() => { if (!window['__startTime']) { window['__startTime'] = Date.now(); return false; } return Date.now() - window['__startTime']; }",
            )
            .polling(polling)
            .await
            .unwrap();
        assert!(delta.json_value::<f64>().await.unwrap() >= polling as f64);
    });
}

#[test]
fn wa_005_no_side_effects_when_the_timeout_hits_while_starting() {
    run(|t| async move {
        // The first check runs while the loop is being started and takes
        // 300 ms, so the 100 ms timeout expires before the loop's handle
        // comes back: the loop must still be aborted.
        let _ = t
            .page
            .wait_for_function(
                "() => { const end = Date.now() + 300; while (Date.now() < end) {} window['counter'] = (window['counter'] || 0) + 1; return false; }",
            )
            .polling(1)
            .timeout(100)
            .await
            .unwrap_err();
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let saved: u64 = t
            .page
            .evaluate("() => window['counter'] || 0")
            .await
            .unwrap();
        t.page.wait_for_timeout(1000).await.unwrap();
        let now: u64 = t
            .page
            .evaluate("() => window['counter'] || 0")
            .await
            .unwrap();
        assert_eq!(now, saved);
    });
}

#[test]
fn wa_005_should_avoid_side_effects_after_timeout() {
    run(|t| async move {
        let context = t.browser.new_context().console_events(true).await.unwrap();
        let page = context.new_page().await.unwrap();
        let counter = Arc::new(AtomicUsize::new(0));
        let c = counter.clone();
        page.on(EventKind::Console, move |_| {
            c.fetch_add(1, Ordering::SeqCst);
        });
        let error = page
            .wait_for_function(
                "() => { window['counter'] = (window['counter'] || 0) + 1; console.log(window['counter']); }",
            )
            .polling(1)
            .timeout(1000)
            .await
            .unwrap_err();
        // Let the abort reach the page, then make sure nothing else is logged.
        tokio::time::sleep(Duration::from_millis(1000)).await;
        let saved = counter.load(Ordering::SeqCst);
        page.wait_for_timeout(2000).await.unwrap();
        assert!(
            error
                .to_string()
                .contains("page.waitForFunction: Timeout 1000ms exceeded"),
            "{error}"
        );
        assert_eq!(counter.load(Ordering::SeqCst), saved);
        context.close().await.unwrap();
    });
}

#[test]
fn wa_006_should_throw_on_polling_mutation() {
    run(|t| async move {
        let r = t
            .page
            .wait_for_function("() => true")
            .polling("mutation")
            .await;
        assert_err_contains(r, "Unknown polling option: mutation");
    });
}

#[test]
fn wa_007_should_poll_on_raf() {
    run(|t| async move {
        let watchdog = tokio::spawn(
            t.page
                .wait_for_function("() => window['__FOO'] === 'hit'")
                .polling("raf")
                .into_future(),
        );
        t.page
            .evaluate::<()>("() => { window['__FOO'] = 'hit'; }")
            .await
            .unwrap();
        watchdog.await.unwrap().unwrap();
    });
}

#[test]
fn wa_008_should_fail_with_predicate_throwing_on_first_call() {
    run(|t| async move {
        let r = t
            .page
            .wait_for_function("() => { throw new Error('oh my'); }")
            .await;
        assert_err_contains(r, "oh my");
    });
}

#[test]
fn wa_009_should_fail_with_predicate_throwing_sometimes() {
    run(|t| async move {
        let r = t
            .page
            .wait_for_function(
                "() => { window['counter'] = (window['counter'] || 0) + 1; if (window['counter'] === 3) throw new Error('Bad counter!'); return window['counter'] === 5 ? 'result' : false; }",
            )
            .await;
        assert_err_contains(r, "Bad counter!");
    });
}

#[test]
fn wa_010_should_fail_with_reference_error_on_wrong_page() {
    run(|t| async move {
        let r = t.page.wait_for_function("() => globalVar === 123").await;
        assert_err_contains(r, "globalVar");
    });
}

#[test]
fn wa_013_should_throw_negative_polling_interval() {
    run(|t| async move {
        let r = t
            .page
            .wait_for_function("() => !!document.body")
            .polling(-10)
            .await;
        assert_err_contains(r, "Cannot poll with non-positive interval");
    });
}

#[test]
fn wa_014_should_return_the_success_value_as_a_js_handle() {
    run(|t| async move {
        let handle = t.page.wait_for_function("() => 5").await.unwrap();
        assert_eq!(handle.json_value::<i32>().await.unwrap(), 5);
    });
}

#[test]
fn wa_015_should_return_the_window_as_a_success_value() {
    run(|t| async move {
        let handle = t.page.wait_for_function("() => window").await.unwrap();
        assert!(!handle.is_element());
    });
}

#[test]
fn wa_016_should_accept_element_handle_arguments() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let div = t.page.query_selector("div").await.unwrap().unwrap();
        let wait = tokio::spawn(
            t.page
                .wait_for_function("element => !element.parentElement")
                .arg(&div)
                .into_future(),
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!wait.is_finished());
        t.page
            .evaluate::<()>("element => element.remove()")
            .arg(&div)
            .await
            .unwrap();
        wait.await.unwrap().unwrap();
    });
}

#[test]
fn wa_017_should_respect_timeout() {
    run(|t| async move {
        let r = t.page.wait_for_function("false").timeout(10).await;
        let e = assert_err_contains(r, "page.waitForFunction: Timeout 10ms exceeded");
        assert!(e.is_timeout());
    });
}

#[test]
fn wa_018_should_respect_default_timeout() {
    run(|t| async move {
        t.page.set_default_timeout(1);
        let r = t.page.wait_for_function("false").await;
        let e = assert_err_contains(r, "page.waitForFunction: Timeout 1ms exceeded");
        assert!(e.is_timeout());
    });
}

#[test]
fn wa_019_should_disable_timeout_when_its_set_to_0() {
    run(|t| async move {
        let watchdog = tokio::spawn(
            t.page
                .wait_for_function(
                    "() => { window['__counter'] = (window['__counter'] || 0) + 1; return window['__injected']; }",
                )
                .timeout(0)
                .polling(10)
                .into_future(),
        );
        t.page
            .wait_for_function("() => window['__counter'] > 10")
            .await
            .unwrap();
        t.page
            .evaluate::<()>("() => { window['__injected'] = true; }")
            .await
            .unwrap();
        watchdog.await.unwrap().unwrap();
    });
}

#[test]
fn wa_020_should_survive_cross_process_navigation() {
    run(|t| async move {
        let wait = tokio::spawn(t.page.wait_for_function("window.__FOO === 1").into_future());
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert!(!wait.is_finished());
        t.page.reload().await.unwrap();
        assert!(!wait.is_finished());
        t.page
            .goto(format!("{}/grid.html", t.server.cross_process_prefix))
            .await
            .unwrap();
        assert!(!wait.is_finished());
        t.page
            .evaluate::<()>("() => { window['__FOO'] = 1; }")
            .await
            .unwrap();
        wait.await.unwrap().unwrap();
    });
}

#[test]
fn wa_021_should_survive_navigations() {
    run(|t| async move {
        let watchdog = tokio::spawn(
            t.page
                .wait_for_function("() => window['__done']")
                .into_future(),
        );
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .goto(format!("{}/title.html", t.server.prefix))
            .await
            .unwrap();
        t.page
            .evaluate::<()>("() => { window['__done'] = true; }")
            .await
            .unwrap();
        watchdog.await.unwrap().unwrap();
    });
}

#[test]
fn wa_022_should_work_with_multiline_body() {
    run(|t| async move {
        let result = t
            .page
            .wait_for_function("\n    (() => true)()\n  ")
            .await
            .unwrap();
        assert!(result.json_value::<bool>().await.unwrap());
    });
}

#[test]
fn wa_023_should_wait_for_predicate_with_arguments() {
    run(|t| async move {
        t.page
            .wait_for_function("({ arg1, arg2 }) => arg1 + arg2 === 3")
            .arg(serde_json::json!({ "arg1": 1, "arg2": 2 }))
            .await
            .unwrap();
    });
}

fn wait_messages(page: &oxidewright::Page) -> Arc<Mutex<Vec<String>>> {
    let messages = Arc::new(Mutex::new(Vec::new()));
    let m = messages.clone();
    page.on(EventKind::Console, move |event| {
        if let oxidewright::Event::Console(msg) = event {
            if msg.text().starts_with("waitForFunction") {
                m.lock().unwrap().push(msg.text().to_string());
            }
        }
    });
    messages
}

#[test]
fn wa_024_should_not_be_called_after_finishing_successfully() {
    run(|t| async move {
        let context = t.browser.new_context().console_events(true).await.unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let messages = wait_messages(&page);
        for i in 1..=3 {
            page.wait_for_function(format!(
                "() => {{ console.log('waitForFunction{i}'); return true; }}"
            ))
            .await
            .unwrap();
            if i < 3 {
                page.reload().await.unwrap();
            }
        }
        page.wait_for_timeout(500).await.unwrap();
        assert_eq!(
            messages.lock().unwrap().join("|"),
            "waitForFunction1|waitForFunction2|waitForFunction3"
        );
        context.close().await.unwrap();
    });
}

#[test]
fn wa_025_should_not_be_called_after_finishing_unsuccessfully() {
    run(|t| async move {
        let context = t.browser.new_context().console_events(true).await.unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let messages = wait_messages(&page);
        for i in 1..=3 {
            let _ = page
                .wait_for_function(format!(
                    "() => {{ console.log('waitForFunction{i}'); throw new Error('waitForFunction{i}'); }}"
                ))
                .await;
            if i < 3 {
                page.reload().await.unwrap();
            }
        }
        page.wait_for_timeout(500).await.unwrap();
        assert_eq!(
            messages.lock().unwrap().join("|"),
            "waitForFunction1|waitForFunction2|waitForFunction3"
        );
        context.close().await.unwrap();
    });
}

#[test]
fn wa_026_should_throw_when_frame_is_detached() {
    run(|t| async move {
        attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        let frame = t.page.frames()[1].clone();
        let wait = tokio::spawn(frame.wait_for_function("() => false").into_future());
        tokio::time::sleep(Duration::from_millis(100)).await;
        detach_frame(&t.page, "frame1").await;
        let error = wait.await.unwrap().unwrap_err().to_string();
        assert!(
            error.contains("frame.waitForFunction: Frame was detached")
                || error.contains("frame.waitForFunction: Execution context was destroyed"),
            "{error}"
        );
    });
}

#[test]
fn wa_isolated_context_does_not_see_page_globals() {
    run(|t| async move {
        t.page
            .evaluate::<()>("() => { window['__FOO'] = 1; }")
            .await
            .unwrap();
        let r = t
            .page
            .wait_for_function("() => window['__FOO'] === 1")
            .isolated_context(true)
            .timeout(300)
            .await;
        assert!(r.unwrap_err().is_timeout());
        t.page
            .wait_for_function("() => window['__FOO'] === 1")
            .isolated_context(false)
            .await
            .unwrap();
    });
}

// ---- locator-wait-for-function.spec.ts ----

#[test]
fn lo_183_should_wait_for_an_attribute_to_appear() {
    run(|t| async move {
        t.page
            .set_content("<button id=toggle>Menu</button>")
            .await
            .unwrap();
        t.page
            .evaluate::<()>("() => { setTimeout(() => document.querySelector('#toggle').setAttribute('aria-expanded', 'true'), 1000); }")
            .await
            .unwrap();
        t.page
            .locator("#toggle")
            .wait_for_function("element => element.hasAttribute('aria-expanded')")
            .await
            .unwrap();
    });
}

#[test]
fn lo_184_should_return_immediately_when_already_truthy() {
    run(|t| async move {
        t.page
            .set_content("<div id=target>yes</div>")
            .await
            .unwrap();
        let () = t
            .page
            .locator("#target")
            .wait_for_function("element => element.textContent === 'yes'")
            .timeout(5000)
            .await
            .unwrap();
    });
}

#[test]
fn lo_185_should_accept_element_handle_arguments() {
    run(|t| async move {
        t.page
            .set_content("<div id=a></div><div id=b>value</div>")
            .await
            .unwrap();
        let handle = t.page.query_selector("#b").await.unwrap().unwrap();
        t.page
            .locator("#a")
            .wait_for_function("(element, other) => other.textContent === 'value'")
            .arg(&handle)
            .await
            .unwrap();
    });
}

#[test]
fn lo_186_should_accept_string_expression() {
    run(|t| async move {
        t.page
            .set_content("<div id=target>yes</div>")
            .await
            .unwrap();
        t.page
            .locator("#target")
            .wait_for_function("element => element.textContent === 'yes'")
            .await
            .unwrap();
    });
}

#[test]
fn lo_187_should_resolve_a_promise_returned_by_the_predicate() {
    run(|t| async move {
        t.page
            .set_content("<div id=target>yes</div>")
            .await
            .unwrap();
        t.page
            .locator("#target")
            .wait_for_function("async element => element.textContent === 'yes'")
            .await
            .unwrap();
    });
}

#[test]
fn lo_188_should_wait_for_element_to_appear_and_survive_rerender() {
    run(|t| async move {
        t.page
            .set_content("<span>nothing here</span>")
            .await
            .unwrap();
        t.page
            .evaluate::<()>(
                "() => { let count = 0; let prev = null; const tick = () => { ++count; const next = document.createElement('div'); next.id = 'target'; next.textContent = String(count); if (prev) prev.remove(); document.body.appendChild(next); prev = next; if (count < 3) setTimeout(tick, 500); }; setTimeout(tick, 500); }",
            )
            .await
            .unwrap();
        t.page
            .locator("#target")
            .wait_for_function("element => element.textContent === '3'")
            .await
            .unwrap();
    });
}

#[test]
fn lo_189_should_throw_when_predicate_throws() {
    run(|t| async move {
        t.page.set_content("<div id=target>no</div>").await.unwrap();
        let r = t
            .page
            .locator("#target")
            .wait_for_function("() => { throw new Error('oh my'); }")
            .await;
        assert_err_contains(r, "oh my");
    });
}

#[test]
fn lo_190_should_throw_on_strict_mode_violation() {
    run(|t| async move {
        t.page
            .set_content("<div class=x>1</div><div class=x>2</div>")
            .await
            .unwrap();
        let r = t
            .page
            .locator("div.x")
            .wait_for_function("() => true")
            .await;
        assert_err_contains(r, "strict mode violation");
    });
}
