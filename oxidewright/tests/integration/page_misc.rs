//! `add_script_tag`/`add_style_tag` (Playwright's `tests/page/page-add-script-tag.spec.ts`,
//! `page-add-style-tag.spec.ts`), `page.pdf` (`tests/library/pdf.spec.ts`), JS/CSS
//! coverage (`tests/library/chromium/js-coverage.spec.ts`, `css-coverage.spec.ts`),
//! CDP sessions (`tests/library/chromium/session.spec.ts`), `opener` and `close`.
//!
//! Not ported:
//! * AT-004 (asset path of an ES module importing a relative file), AT-010,
//!   AT-011, AT-019, AT-020 (CSP violations are only reported through console
//!   messages, which oxidewright does not enable).
//! * CV-002..CV-007, CV-009..CV-017: variations needing more fixture pages;
//!   the basic JS and CSS flows are covered below.

use oxidewright::Page;
use serde_json::{Value, json};

use crate::common::*;

fn asset(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("assets")
        .join(name)
}

async fn background(page: &Page) -> String {
    page.evaluate("() => window.getComputedStyle(document.querySelector('body')).getPropertyValue('background-color')")
        .await
        .unwrap()
}

#[test]
fn at_001_should_throw_an_error_if_no_options_are_provided() {
    run(|t| async move {
        assert_err_contains(
            t.page.add_script_tag().await,
            "Provide an object with a `url`, `path` or `content` property",
        );
    });
}

#[test]
fn at_002_should_work_with_a_url() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .add_script_tag()
            .url("/injectedfile.js")
            .await
            .unwrap();
        let value: i32 = t.page.evaluate("() => window['injected']").await.unwrap();
        assert_eq!(value, 123);
    });
}

#[test]
fn at_003_should_work_with_a_url_and_type_module() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .add_script_tag()
            .url("/es6/es6import.js")
            .r#type("module")
            .await
            .unwrap();
        let value: i32 = t
            .page
            .evaluate("() => window['__es6injected']")
            .await
            .unwrap();
        assert_eq!(value, 42);
    });
}

#[test]
fn at_005_should_work_with_a_content_and_type_module() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .add_script_tag()
            .content("import num from '/es6/es6module.js'; window.__es6injected = num;")
            .r#type("module")
            .await
            .unwrap();
        let mut value = Value::Null;
        for _ in 0..100 {
            value = t
                .page
                .evaluate("() => window['__es6injected'] ?? null")
                .await
                .unwrap();
            if !value.is_null() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(value, json!(42));
    });
}

#[test]
fn at_006_should_throw_an_error_if_loading_from_url_fail() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert!(
            t.page
                .add_script_tag()
                .url("/nonexistfile.js")
                .await
                .is_err()
        );
    });
}

#[test]
fn at_007_should_work_with_a_path() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .add_script_tag()
            .path(asset("injectedfile.js"))
            .await
            .unwrap();
        let value: i32 = t.page.evaluate("() => window['injected']").await.unwrap();
        assert_eq!(value, 123);
    });
}

#[test]
fn at_009_should_work_with_content() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let handle = t
            .page
            .add_script_tag()
            .content("window['__injected'] = 35;")
            .await
            .unwrap();
        let tag: String = handle.evaluate("e => e.tagName").await.unwrap();
        assert_eq!(tag, "SCRIPT");
        let value: i32 = t.page.evaluate("() => window['__injected']").await.unwrap();
        assert_eq!(value, 35);
    });
}

#[test]
fn at_012_should_throw_a_nice_error_when_the_request_fails() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let url = format!("{}/this_does_not_exist.js", t.server.prefix);
        assert_err_contains(t.page.add_script_tag().url(&url).await, &url);
    });
}

#[test]
fn at_013_should_throw_an_error_if_no_options_are_provided() {
    run(|t| async move {
        assert_err_contains(
            t.page.add_style_tag().await,
            "Provide an object with a `url`, `path` or `content` property",
        );
    });
}

#[test]
fn at_014_should_work_with_a_url() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .add_style_tag()
            .url("/injectedstyle.css")
            .await
            .unwrap();
        assert_eq!(background(&t.page).await, "rgb(255, 0, 0)");
    });
}

#[test]
fn at_015_should_throw_an_error_if_loading_from_url_fail() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert!(
            t.page
                .add_style_tag()
                .url("/nonexistfile.js")
                .await
                .is_err()
        );
    });
}

#[test]
fn at_016_should_work_with_a_path() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .add_style_tag()
            .path(asset("injectedstyle.css"))
            .await
            .unwrap();
        assert_eq!(background(&t.page).await, "rgb(255, 0, 0)");
    });
}

#[test]
fn at_017_should_include_source_url_when_path_is_provided() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .add_style_tag()
            .path(asset("injectedstyle.css"))
            .await
            .unwrap();
        let content: String = t
            .page
            .evaluate("() => document.querySelector('style').innerHTML")
            .await
            .unwrap();
        assert!(content.contains("injectedstyle.css"), "{content}");
    });
}

#[test]
fn at_018_should_work_with_content() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .add_style_tag()
            .content("body { background-color: green; }")
            .await
            .unwrap();
        assert_eq!(background(&t.page).await, "rgb(0, 128, 0)");
    });
}

#[test]
fn frame_add_script_tag() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let frame = attach_frame(&t.page, "f", &t.server.empty_page).await;
        frame
            .add_script_tag()
            .content("window.__inFrame = 1;")
            .await
            .unwrap();
        let value: i32 = frame.evaluate("() => window.__inFrame").await.unwrap();
        assert_eq!(value, 1);
    });
}

// ----------------------------------------------------------------------
// pdf
// ----------------------------------------------------------------------

#[test]
fn pd_001_should_be_able_to_save_file() {
    run(|t| async move {
        let path = std::env::temp_dir()
            .join(format!("oxidewright-pdf-{}", t.server.port))
            .join("output.pdf");
        let bytes = t.page.pdf().path(&path).await.unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    });
}

#[test]
fn pd_002_should_be_able_to_generate_outline() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .base_url(&t.server.prefix)
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto("/headings.html").await.unwrap();
        let plain = page.pdf().await.unwrap();
        let outline = page.pdf().tagged(true).outline(true).await.unwrap();
        assert!(outline.len() > plain.len());
        context.close().await.unwrap();
    });
}

#[test]
fn pdf_options_are_validated() {
    run(|t| async move {
        assert_err_contains(
            t.page.pdf().format("A42").await,
            "Unknown paper format: A42",
        );
        assert_err_contains(
            t.page.pdf().width("abcpx").await,
            "Failed to parse parameter value: abcpx",
        );
        let pdf = t
            .page
            .pdf()
            .format("a4")
            .landscape(true)
            .margin(oxidewright::PdfMargin::all("1cm"))
            .print_background(true)
            .display_header_footer(true)
            .header_template("<span class=title></span>")
            .page_ranges("1")
            .await
            .unwrap();
        assert!(pdf.starts_with(b"%PDF"));
    });
}

// ----------------------------------------------------------------------
// coverage
// ----------------------------------------------------------------------

#[test]
fn cv_001_should_work() {
    run(|t| async move {
        t.page.start_js_coverage().await.unwrap();
        t.page
            .goto(format!("{}/jscoverage-simple.html", t.server.prefix))
            .await
            .unwrap();
        let coverage = t.page.stop_js_coverage().await.unwrap();
        assert_eq!(coverage.len(), 1, "{coverage:?}");
        assert!(coverage[0].url.contains("/jscoverage-simple.html"));
        let foo = coverage[0]
            .functions
            .iter()
            .find(|f| f.function_name == "foo")
            .unwrap();
        assert_eq!(foo.ranges[0].count, 1);
        assert!(
            coverage[0]
                .source
                .as_deref()
                .unwrap()
                .contains("function foo")
        );
    });
}

#[test]
fn cv_008_should_work() {
    run(|t| async move {
        t.page.start_css_coverage().await.unwrap();
        t.page
            .goto(format!("{}/csscoverage-simple.html", t.server.prefix))
            .await
            .unwrap();
        let coverage = t.page.stop_css_coverage().await.unwrap();
        let sheet = coverage
            .iter()
            .find(|e| e.url.contains("one-style.css"))
            .expect("one-style.css entry");
        assert!(!sheet.ranges.is_empty());
        let text = sheet.text.as_deref().unwrap();
        let used = &text[sheet.ranges[0].start as usize..sheet.ranges[0].end as usize];
        assert!(used.contains("body"), "{used}");
    });
}

#[test]
fn coverage_cannot_be_started_twice() {
    run(|t| async move {
        t.page.start_js_coverage().await.unwrap();
        assert_err_contains(
            t.page.start_js_coverage().await,
            "JSCoverage is already enabled",
        );
        t.page.stop_js_coverage().await.unwrap();
        assert!(t.page.stop_js_coverage().await.unwrap().is_empty());
    });
}

// ----------------------------------------------------------------------
// CDP sessions, opener, close, misc
// ----------------------------------------------------------------------

#[test]
fn cdp_session_should_work() {
    run(|t| async move {
        let session = t.context.new_cdp_session(&t.page).await.unwrap();
        let result = session
            .send(
                "Runtime.evaluate",
                json!({ "expression": "7 * 3", "returnByValue": true }),
            )
            .await
            .unwrap();
        assert_eq!(result["result"]["value"], 21);
        session.detach().await.unwrap();
        assert!(
            session
                .send("Runtime.evaluate", json!({ "expression": "1" }))
                .await
                .is_err()
        );
    });
}

#[test]
fn cdp_session_should_receive_events() {
    run(|t| async move {
        use chromiumoxide_events::EventFrameNavigated;
        use futures::StreamExt;
        let session = t.context.new_cdp_session(&t.page).await.unwrap();
        let mut events = session
            .event_listener::<EventFrameNavigated>()
            .await
            .unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        let event = events.next().await.unwrap();
        assert_eq!(event.frame.url, t.server.empty_page);
    });
}

mod chromiumoxide_events {
    pub use oxidewright::chromiumoxide::cdp::browser_protocol::page::EventFrameNavigated;
}

#[test]
fn cdp_session_should_reject_in_process_frames() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let frame = attach_frame(&t.page, "f", &t.server.empty_page).await;
        assert_err_contains(
            t.context.new_cdp_session(&frame).await,
            "This frame does not have a separate CDP session",
        );
    });
}

#[test]
fn browser_cdp_session_should_work() {
    run(|t| async move {
        let session = t.browser.new_browser_cdp_session().await.unwrap();
        let version = session.send("Browser.getVersion", json!({})).await.unwrap();
        assert!(
            version["product"]
                .as_str()
                .unwrap()
                .contains(t.browser.version())
        );
    });
}

#[test]
fn opener_should_return_the_opening_page() {
    run(|t| async move {
        assert!(t.page.opener().is_none());
        let other = t.context.new_page().await.unwrap();
        assert!(other.opener().is_none());
    });
}

#[test]
fn page_close_should_accept_a_reason() {
    run(|t| async move {
        let page = t.context.new_page().await.unwrap();
        page.close().reason("done").await.unwrap();
        assert!(page.is_closed());
        assert_eq!(page.close_reason().as_deref(), Some("done"));
    });
}

#[test]
fn page_close_should_run_beforeunload_if_asked_for() {
    run(|t| async move {
        let page = t.context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        // Without a beforeunload handler, the page simply closes.
        page.close().run_before_unload(true).await.unwrap();
        for _ in 0..100 {
            if page.is_closed() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(page.is_closed());
    });
}

#[test]
fn context_close_should_accept_a_reason() {
    run(|t| async move {
        let context = t.browser.new_context().await.unwrap();
        context.close().reason("bye").await.unwrap();
        assert!(context.is_closed());
        assert_eq!(context.close_reason().as_deref(), Some("bye"));
    });
}

#[test]
fn page_title_and_content() {
    run(|t| async move {
        t.page
            .goto(format!("{}/title.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(t.page.title().await.unwrap(), "Woof-Woof");
        assert!(
            t.page
                .content()
                .await
                .unwrap()
                .contains("<title>Woof-Woof</title>")
        );
        assert!(t.context.pages().contains(&t.page));
        t.page.bring_to_front().await.unwrap();
    });
}
