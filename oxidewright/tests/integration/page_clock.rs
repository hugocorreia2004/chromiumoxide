//! Fake timers, ported from Playwright's `tests/library/page-clock.spec.ts`.
//! The `calls` fixture (`page.exposeFunction('stub')`) is replaced by an
//! in-page `window.stub` that records its arguments.
//!
//! Not ported: CK-007 (error propagation of a throwing timer depends on page
//! error events), CK-027..CK-030 (popups), CK-045/CK-046/CK-220 (init-script
//! ordering on new pages), CK-049..CK-052 (Temporal, not shipped by default in
//! Chromium), CK-053/CK-054 (timing sensitive), CK-015 (fixed after the bundled
//! Patchright 1.64.3 clock: fast-forwarding more than 2^32 ms overflows).
//!
//! Fake time starts at -10s where the originals use 0, so that the real time
//! elapsing before `pause_at(1000)` on a loaded CI machine never exceeds it.

use oxidewright::Page;
use serde_json::{Value, json};

use crate::common::*;

async fn setup(page: &Page) {
    page.evaluate::<()>(
        "() => { window.__calls = []; window.stub = (...params) => window.__calls.push(params); }",
    )
    .await
    .unwrap();
}

async fn calls(page: &Page) -> Vec<Value> {
    page.evaluate("() => window.__calls").await.unwrap()
}

async fn install_paused(page: &Page) {
    page.clock().install().time(-10_000).await.unwrap();
    page.clock().pause_at(1000).await.unwrap();
    setup(page).await;
}

#[test]
fn ck_001_triggers_immediately_without_specified_delay() {
    run(|t| async move {
        install_paused(&t.page).await;
        t.page
            .evaluate::<()>("() => { setTimeout(window.stub); }")
            .await
            .unwrap();
        t.page.clock().run_for(0).await.unwrap();
        assert_eq!(calls(&t.page).await.len(), 1);
    });
}

#[test]
fn ck_002_does_not_trigger_without_sufficient_delay() {
    run(|t| async move {
        install_paused(&t.page).await;
        t.page
            .evaluate::<()>("() => { setTimeout(window.stub, 100); }")
            .await
            .unwrap();
        t.page.clock().run_for(10).await.unwrap();
        assert!(calls(&t.page).await.is_empty());
    });
}

#[test]
fn ck_003_triggers_after_sufficient_delay() {
    run(|t| async move {
        install_paused(&t.page).await;
        t.page
            .evaluate::<()>("() => { setTimeout(window.stub, 100); }")
            .await
            .unwrap();
        t.page.clock().run_for(100).await.unwrap();
        assert_eq!(calls(&t.page).await.len(), 1);
    });
}

#[test]
fn ck_005_triggers_multiple_simultaneous_timers() {
    run(|t| async move {
        install_paused(&t.page).await;
        t.page
            .evaluate::<()>("() => { setTimeout(window.stub, 100); setTimeout(window.stub, 100); setTimeout(window.stub, 99); setTimeout(window.stub, 100); }")
            .await
            .unwrap();
        t.page.clock().run_for(100).await.unwrap();
        assert_eq!(calls(&t.page).await.len(), 4);
    });
}

#[test]
fn ck_006_waits_after_set_timeout_was_called() {
    run(|t| async move {
        install_paused(&t.page).await;
        t.page
            .evaluate::<()>("() => { setTimeout(window.stub, 150); }")
            .await
            .unwrap();
        t.page.clock().run_for(50).await.unwrap();
        assert!(calls(&t.page).await.is_empty());
        t.page.clock().run_for(100).await.unwrap();
        assert_eq!(calls(&t.page).await.len(), 1);
    });
}

#[test]
fn ck_008_creates_updated_date_while_ticking() {
    run(|t| async move {
        install_paused(&t.page).await;
        t.page.clock().set_system_time(0).await.unwrap();
        t.page
            .evaluate::<()>("() => { setInterval(() => window.stub(new Date().getTime()), 10); }")
            .await
            .unwrap();
        t.page.clock().run_for(100).await.unwrap();
        let expected: Vec<Value> = (1..=10).map(|i| json!([i * 10])).collect();
        assert_eq!(calls(&t.page).await, expected);
    });
}

#[test]
fn ck_009_passes_8_seconds() {
    run(|t| async move {
        install_paused(&t.page).await;
        t.page
            .evaluate::<()>("() => { setInterval(window.stub, 4000); }")
            .await
            .unwrap();
        t.page.clock().run_for("08").await.unwrap();
        assert_eq!(calls(&t.page).await.len(), 2);
    });
}

#[test]
fn ck_010_passes_1_minute() {
    run(|t| async move {
        install_paused(&t.page).await;
        t.page
            .evaluate::<()>("() => { setInterval(window.stub, 6000); }")
            .await
            .unwrap();
        t.page.clock().run_for("01:00").await.unwrap();
        assert_eq!(calls(&t.page).await.len(), 10);
    });
}

#[test]
fn ck_011_passes_2_hours_34_minutes_and_10_seconds() {
    run(|t| async move {
        install_paused(&t.page).await;
        t.page
            .evaluate::<()>("() => { setInterval(window.stub, 10000); }")
            .await
            .unwrap();
        t.page.clock().run_for("02:34:10").await.unwrap();
        assert_eq!(calls(&t.page).await.len(), 925);
    });
}

#[test]
fn ck_012_throws_for_invalid_format() {
    run(|t| async move {
        install_paused(&t.page).await;
        t.page
            .evaluate::<()>("() => { setInterval(window.stub, 10000); }")
            .await
            .unwrap();
        assert_err_contains(
            t.page.clock().run_for("12:02:34:10").await,
            "Clock only understands numbers, 'mm:ss' and 'hh:mm:ss'",
        );
        assert!(calls(&t.page).await.is_empty());
    });
}

#[test]
fn ck_013_returns_the_current_now_value() {
    run(|t| async move {
        install_paused(&t.page).await;
        t.page.clock().set_system_time(0).await.unwrap();
        t.page.clock().run_for(200).await.unwrap();
        let now: f64 = t.page.evaluate("() => Date.now()").await.unwrap();
        assert_eq!(now, 200.0);
    });
}

#[test]
fn ck_014_ignores_timers_which_wouldnt_be_run() {
    run(|t| async move {
        install_paused(&t.page).await;
        t.page
            .evaluate::<()>(
                "() => { setTimeout(() => window.stub('should not be logged'), 1000); }",
            )
            .await
            .unwrap();
        t.page.clock().fast_forward(500).await.unwrap();
        assert!(calls(&t.page).await.is_empty());
    });
}

#[test]
fn ck_016_supports_string_time_arguments() {
    run(|t| async move {
        install_paused(&t.page).await;
        t.page
            .evaluate::<()>("() => { setTimeout(() => window.stub(Date.now()), 100000); }")
            .await
            .unwrap();
        t.page.clock().fast_forward("01:50").await.unwrap();
        assert_eq!(calls(&t.page).await, vec![json!([1000 + 110000])]);
    });
}

#[test]
fn ck_017_sets_initial_timestamp() {
    run(|t| async move {
        install_paused(&t.page).await;
        t.page.clock().set_system_time(1400).await.unwrap();
        let now: f64 = t.page.evaluate("() => Date.now()").await.unwrap();
        assert_eq!(now, 1400.0);
    });
}

#[test]
fn ck_018_should_throw_for_invalid_date() {
    run(|t| async move {
        install_paused(&t.page).await;
        assert_err_contains(
            t.page.clock().set_system_time("invalid").await,
            "clock.setSystemTime: Invalid date: invalid",
        );
    });
}

#[test]
fn ck_020_global_fake_set_timeout_should_return_id() {
    run(|t| async move {
        install_paused(&t.page).await;
        let id: Value = t
            .page
            .evaluate("() => setTimeout(window.stub, 1000)")
            .await
            .unwrap();
        assert!(id.is_number());
    });
}

#[test]
fn ck_021_replaces_global_clear_timeout() {
    run(|t| async move {
        install_paused(&t.page).await;
        t.page
            .evaluate::<()>("() => { const to = setTimeout(window.stub, 1000); clearTimeout(to); }")
            .await
            .unwrap();
        t.page.clock().run_for(1000).await.unwrap();
        assert!(calls(&t.page).await.is_empty());
    });
}

#[test]
fn ck_023_replaces_global_clear_interval() {
    run(|t| async move {
        install_paused(&t.page).await;
        t.page
            .evaluate::<()>(
                "() => { const to = setInterval(window.stub, 500); clearInterval(to); }",
            )
            .await
            .unwrap();
        t.page.clock().run_for(1000).await.unwrap();
        assert!(calls(&t.page).await.is_empty());
    });
}

#[test]
fn ck_024_replaces_global_performance_now() {
    run(|t| async move {
        install_paused(&t.page).await;
        let promise = t
            .page
            .evaluate_handle("async () => { const prev = performance.now(); await new Promise(f => setTimeout(f, 1000)); const next = performance.now(); return { prev, next }; }")
            .into_future();
        let page = t.page.clone();
        let runner = async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            page.clock().run_for(1000).await.unwrap();
        };
        let (handle, ()) = tokio::join!(promise, runner);
        let value: Value = handle.unwrap().json_value().await.unwrap();
        // `performance.now()` counts from the install time (-10s).
        assert_eq!(value, json!({ "prev": 11000, "next": 12000 }));
    });
}

#[test]
fn ck_025_fakes_date_constructor() {
    run(|t| async move {
        install_paused(&t.page).await;
        let now: f64 = t.page.evaluate("() => new Date().getTime()").await.unwrap();
        assert_eq!(now, 1000.0);
    });
}

#[test]
fn ck_031_does_not_fake_methods() {
    run(|t| async move {
        t.page.clock().set_fixed_time(0).await.unwrap();
        // Timers are real: this resolves without the clock being advanced.
        t.page
            .evaluate::<()>("() => new Promise(f => setTimeout(f, 1))")
            .await
            .unwrap();
    });
}

#[test]
fn ck_032_allows_setting_time_multiple_times() {
    run(|t| async move {
        t.page.clock().set_fixed_time(100).await.unwrap();
        let now: f64 = t.page.evaluate("() => Date.now()").await.unwrap();
        assert_eq!(now, 100.0);
        t.page.clock().set_fixed_time(200).await.unwrap();
        let now: f64 = t.page.evaluate("() => Date.now()").await.unwrap();
        assert_eq!(now, 200.0);
    });
}

#[test]
fn ck_033_fixed_time_is_not_affected_by_clock_manipulation() {
    run(|t| async move {
        t.page.clock().set_fixed_time(100).await.unwrap();
        let now: f64 = t.page.evaluate("() => Date.now()").await.unwrap();
        assert_eq!(now, 100.0);
        t.page.clock().fast_forward(20).await.unwrap();
        let now: f64 = t.page.evaluate("() => Date.now()").await.unwrap();
        assert_eq!(now, 100.0);
    });
}

#[test]
fn ck_034_allows_installing_fake_timers_after_settings_time() {
    run(|t| async move {
        t.page.clock().set_fixed_time(100).await.unwrap();
        let now: f64 = t.page.evaluate("() => Date.now()").await.unwrap();
        assert_eq!(now, 100.0);
        t.page.clock().set_fixed_time(200).await.unwrap();
        setup(&t.page).await;
        t.page
            .evaluate::<()>("() => { setTimeout(() => window.stub(Date.now()), 0); }")
            .await
            .unwrap();
        t.page.clock().run_for(0).await.unwrap();
        assert_eq!(calls(&t.page).await, vec![json!([200])]);
    });
}

#[test]
fn ck_035_should_progress_time() {
    run(|t| async move {
        t.page.clock().install().time(0).await.unwrap();
        t.page.goto("data:text/html,").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        let now: f64 = t.page.evaluate("() => Date.now()").await.unwrap();
        assert!((1000.0..=40000.0).contains(&now), "{now}");
    });
}

#[test]
fn ck_037_should_fast_forward() {
    run(|t| async move {
        t.page.clock().install().time(0).await.unwrap();
        t.page.goto("data:text/html,").await.unwrap();
        t.page.clock().fast_forward(10000).await.unwrap();
        let now: f64 = t.page.evaluate("() => Date.now()").await.unwrap();
        assert!((10000.0..=40000.0).contains(&now), "{now}");
    });
}

#[test]
fn ck_038_should_pause() {
    run(|t| async move {
        t.page.clock().install().time(-10_000).await.unwrap();
        t.page.goto("data:text/html,").await.unwrap();
        t.page.clock().pause_at(1000).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        t.page.clock().resume().await.unwrap();
        let now: f64 = t.page.evaluate("() => Date.now()").await.unwrap();
        assert!((0.0..=1500.0).contains(&now), "{now}");
    });
}

#[test]
fn ck_041_should_set_system_time_on_pause() {
    run(|t| async move {
        t.page.clock().install().time(-10_000).await.unwrap();
        t.page.goto("data:text/html,").await.unwrap();
        t.page.clock().pause_at(1000).await.unwrap();
        let now: f64 = t.page.evaluate("() => Date.now()").await.unwrap();
        assert_eq!(now, 1000.0);
    });
}

#[test]
fn ck_047_check_date_now_is_an_integer() {
    run(|t| async move {
        t.page.clock().install().await.unwrap();
        t.page.goto("data:text/html,").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let integer: bool = t
            .page
            .evaluate("() => Number.isInteger(Date.now())")
            .await
            .unwrap();
        assert!(integer);
    });
}

#[test]
fn install_accepts_date_strings() {
    run(|t| async move {
        t.page
            .clock()
            .install()
            .time("2024-02-02T08:00:00Z")
            .await
            .unwrap();
        t.page
            .clock()
            .pause_at("2024-02-02T10:00:00Z")
            .await
            .unwrap();
        let iso: String = t
            .page
            .evaluate("() => new Date().toISOString()")
            .await
            .unwrap();
        assert_eq!(iso, "2024-02-02T10:00:00.000Z");
    });
}

#[test]
fn clock_applies_to_new_documents_and_frames() {
    run(|t| async move {
        t.context.clock().set_fixed_time(42_000).await.unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        let now: f64 = t.page.evaluate("() => Date.now()").await.unwrap();
        assert_eq!(now, 42_000.0);
        let frame = attach_frame(&t.page, "f", &t.server.empty_page).await;
        let now: f64 = frame.evaluate("() => Date.now()").await.unwrap();
        assert_eq!(now, 42_000.0);
        let page = t.context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let now: f64 = page.evaluate("() => Date.now()").await.unwrap();
        assert_eq!(now, 42_000.0);
    });
}

use std::future::IntoFuture;
