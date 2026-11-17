//! Chromium performance traces, ported from Playwright's
//! `tests/library/chromium/tracing.spec.ts`.

use serde_json::Value;

use crate::common::*;

fn temp_file(name: &str) -> std::path::PathBuf {
    std::env::temp_dir()
        .join(format!("oxidewright-trace-{}-{name}", std::process::id()))
        .join("trace.json")
}

async fn rafraf(page: &oxidewright::Page, count: usize) {
    page.evaluate::<()>(
        "async count => { for (let i = 0; i < count; i++) await new Promise(f => requestAnimationFrame(() => requestAnimationFrame(f))); }",
    )
    .arg(count)
    .await
    .unwrap();
}

#[test]
fn tr_043_should_output_a_trace() {
    run(|t| async move {
        let path = temp_file("043");
        t.browser
            .start_tracing()
            .page(&t.page)
            .screenshots(true)
            .path(&path)
            .await
            .unwrap();
        for _ in 0..11 {
            t.page
                .goto(format!("{}/grid.html", t.server.prefix))
                .await
                .unwrap();
        }
        t.browser.stop_tracing().await.unwrap();
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    });
}

#[test]
fn tr_044_should_create_directories_as_needed() {
    run(|t| async move {
        let path = temp_file("044")
            .parent()
            .unwrap()
            .join("these")
            .join("are")
            .join("directories")
            .join("trace.json");
        t.browser
            .start_tracing()
            .page(&t.page)
            .screenshots(true)
            .path(&path)
            .await
            .unwrap();
        t.page
            .goto(format!("{}/grid.html", t.server.prefix))
            .await
            .unwrap();
        t.browser.stop_tracing().await.unwrap();
        assert!(path.exists());
    });
}

#[test]
fn tr_045_should_run_with_custom_categories_if_provided() {
    run(|t| async move {
        let path = temp_file("045");
        t.browser
            .start_tracing()
            .page(&t.page)
            .path(&path)
            .categories(["disabled-by-default-cc.debug"])
            .await
            .unwrap();
        rafraf(&t.page, 1).await;
        t.browser.stop_tracing().await.unwrap();
        let trace: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let in_config = trace["metadata"]["trace-config"]
            .as_str()
            .is_some_and(|c| c.contains("disabled-by-default-cc.debug"));
        let in_events = trace["traceEvents"].as_array().is_some_and(|events| {
            events
                .iter()
                .any(|e| e["cat"] == "disabled-by-default-cc.debug")
        });
        assert!(in_config || in_events);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    });
}

#[test]
fn tr_046_should_throw_if_tracing_on_two_pages() {
    run(|t| async move {
        t.browser.start_tracing().page(&t.page).await.unwrap();
        let other = t.context.new_page().await.unwrap();
        let error = t.browser.start_tracing().page(&other).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Cannot start recording trace while already recording trace."),
            "{error}"
        );
        t.browser.stop_tracing().await.unwrap();
    });
}

#[test]
fn tr_047_should_return_a_buffer() {
    run(|t| async move {
        let path = temp_file("047");
        t.browser
            .start_tracing()
            .page(&t.page)
            .screenshots(true)
            .path(&path)
            .await
            .unwrap();
        t.page
            .goto(format!("{}/grid.html", t.server.prefix))
            .await
            .unwrap();
        let trace = t.browser.stop_tracing().await.unwrap();
        assert_eq!(trace, std::fs::read(&path).unwrap());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    });
}

#[test]
fn tr_048_should_work_without_options() {
    run(|t| async move {
        t.browser.start_tracing().page(&t.page).await.unwrap();
        t.page
            .goto(format!("{}/grid.html", t.server.prefix))
            .await
            .unwrap();
        let trace = t.browser.stop_tracing().await.unwrap();
        assert!(!trace.is_empty());
    });
}

#[test]
fn tr_049_should_support_a_buffer_without_a_path() {
    run(|t| async move {
        t.browser
            .start_tracing()
            .page(&t.page)
            .screenshots(true)
            .await
            .unwrap();
        t.page
            .goto(format!("{}/grid.html", t.server.prefix))
            .await
            .unwrap();
        rafraf(&t.page, 100).await;
        let trace = t.browser.stop_tracing().await.unwrap();
        assert!(String::from_utf8_lossy(&trace).contains("screenshot"));
    });
}

#[test]
fn stop_tracing_without_start_fails() {
    run(|t| async move {
        let error = t.browser.stop_tracing().await.unwrap_err();
        assert!(
            error.to_string().contains("Tracing was not started."),
            "{error}"
        );
    });
}
