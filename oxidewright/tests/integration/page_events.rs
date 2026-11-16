//! Ports of `tests/page/page-dialog.spec.ts`, `page-event-popup.spec.ts`,
//! `page-event-load.spec.ts`, `page-event-console.spec.ts`,
//! `page-event-pageerror.spec.ts`, `page-filechooser.spec.ts`,
//! `workers.spec.ts` and `tests/library/web-socket.spec.ts` (subsets).
//!
//! Skipped (reason):
//! - DI-002/DI-003/DI-004 `dialogclosed` event: not part of the stable API ported here.
//! - DI-010/DI-011/DI-013 need `page.textContent`/`page.click` (locators are another module).
//! - Popup tests that click links (`page.click`) are ported with `evaluate` clicks; tests about
//!   `noopener` features/user agents of popups need emulation (another module).
//! - Console tests are opt-in (`console_events`, see `src/console.rs`): only the basic ones are
//!   ported; tests about `console.table`, `%s` formatting and worker consoles are skipped.
//! - File chooser tests that use `page.click`, `setInputFiles` with buffers or drag-and-drop
//!   need the input module; ported with `element.click()` from `evaluate`.
//! - Worker tests with `evaluateHandle`, worker consoles, workers in OOPIFs and
//!   `page.workers()` after a cross-process navigation.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::header;
use axum::response::Response as HttpResponse;
use oxidewright::{EventKind, Page};

use crate::common::{assert_err_contains, attach_frame, detach_frame, run};

#[test]
fn di_001_should_fire() {
    run(|t| async move {
        t.page.on_dialog(|dialog| {
            assert_eq!(dialog.r#type(), "alert");
            assert_eq!(dialog.default_value(), "");
            assert_eq!(dialog.message(), "yo");
            tokio::spawn(async move { dialog.accept(None).await.unwrap() });
        });
        t.page.evaluate::<()>("() => alert('yo')").await.unwrap();
    });
}

#[test]
fn di_005_should_allow_accepting_prompts() {
    run(|t| async move {
        t.page.on_dialog(|dialog| {
            assert_eq!(dialog.r#type(), "prompt");
            assert_eq!(dialog.default_value(), "yes.");
            assert_eq!(dialog.message(), "question?");
            tokio::spawn(async move { dialog.accept(Some("answer!")).await.unwrap() });
        });
        let result: String = t
            .page
            .evaluate("() => prompt('question?', 'yes.')")
            .await
            .unwrap();
        assert_eq!(result, "answer!");
    });
}

#[test]
fn di_006_should_dismiss_the_prompt() {
    run(|t| async move {
        t.page.on_dialog(|dialog| {
            tokio::spawn(async move { dialog.dismiss().await.unwrap() });
        });
        let result: Option<String> = t.page.evaluate("() => prompt('question?')").await.unwrap();
        assert_eq!(result, None);
    });
}

#[test]
fn di_007_should_accept_the_confirm_prompt() {
    run(|t| async move {
        t.page.on_dialog(|dialog| {
            tokio::spawn(async move { dialog.accept(None).await.unwrap() });
        });
        let result: bool = t.page.evaluate("() => confirm('boolean?')").await.unwrap();
        assert!(result);
    });
}

#[test]
fn di_008_should_dismiss_the_confirm_prompt() {
    run(|t| async move {
        t.page.on_dialog(|dialog| {
            tokio::spawn(async move { dialog.dismiss().await.unwrap() });
        });
        let result: bool = t.page.evaluate("() => confirm('boolean?')").await.unwrap();
        assert!(!result);
    });
}

#[test]
fn di_009_should_be_able_to_close_context_with_open_alert() {
    run(|t| async move {
        let context = t.browser.new_context().await.unwrap();
        let page = context.new_page().await.unwrap();
        let alert = page.wait_for_dialog();
        page.evaluate::<()>("() => { setTimeout(() => alert('hello'), 0); }")
            .await
            .unwrap();
        alert.await.unwrap();
        context.close().await.unwrap();
    });
}

#[test]
fn di_012_should_auto_dismiss_the_prompt_without_listeners() {
    run(|t| async move {
        let result: Option<String> = t.page.evaluate("() => prompt('question?')").await.unwrap();
        assert_eq!(result, None);
    });
}

#[test]
fn should_reject_double_accept() {
    run(|t| async move {
        let dialog = t.page.wait_for_dialog();
        let page = t.page.clone();
        let eval = tokio::spawn(async move { page.evaluate::<()>("() => alert('x')").await });
        let dialog = dialog.await.unwrap();
        dialog.accept(None).await.unwrap();
        assert_err_contains(
            dialog.dismiss().await,
            "Cannot dismiss dialog which is already handled!",
        );
        eval.await.unwrap().unwrap();
    });
}

// ---------------------------------------------------------------- popups

#[test]
fn ev_popup_should_work() {
    run(|t| async move {
        let popup = t
            .page
            .expect_popup(
                t.page
                    .evaluate::<()>("() => { window.__popup = window.open('about:blank'); }"),
            )
            .await
            .unwrap();
        assert!(popup.opener() == Some(t.page.clone()));
        let opener: bool = popup.evaluate("() => !!window.opener").await.unwrap();
        assert!(opener);
    });
}

#[test]
fn ev_popup_should_work_with_window_features() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let popup = t
            .page
            .expect_popup(t.page.evaluate::<()>(
                "() => { window.__popup = window.open(window.location.href, 'Title', 'toolbar=no,location=no,directories=no,status=no,menubar=no,scrollbars=yes,resizable=yes,width=780,height=200,top=0,left=0'); }",
            ))
            .await
            .unwrap();
        popup.wait_for_load_state().await.unwrap();
        assert_eq!(popup.url(), t.server.empty_page);
    });
}

#[test]
fn ev_popup_should_be_reported_on_the_context() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let page = t
            .context
            .expect_page(
                t.page
                    .evaluate::<()>("() => { window.open('about:blank'); }"),
            )
            .await;
        let page = page.unwrap();
        assert!(t.context.pages().contains(&page));
    });
}

#[test]
fn ev_popup_should_work_with_target_blank_link() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .set_content(format!(
                "<a target=_blank rel=opener href='{}/one-style.html'>yo</a>",
                t.server.prefix
            ))
            .await
            .unwrap();
        let popup = t
            .page
            .expect_popup(
                t.page
                    .evaluate::<()>("() => document.querySelector('a').click()"),
            )
            .await
            .unwrap();
        popup.wait_for_load_state().await.unwrap();
        assert_eq!(popup.url(), format!("{}/one-style.html", t.server.prefix));
    });
}

// ----------------------------------------------------------- page events

#[test]
fn should_fire_load_and_dom_content_loaded() {
    run(|t| async move {
        let events = Arc::new(Mutex::new(Vec::new()));
        let e = events.clone();
        t.page
            .on_dom_content_loaded(move |_| e.lock().unwrap().push("domcontentloaded"));
        let e = events.clone();
        t.page.on_load(move |_| e.lock().unwrap().push("load"));
        let load = t.page.wait_for_event(EventKind::Load);
        t.page.goto(&t.server.empty_page).await.unwrap();
        load.await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(*events.lock().unwrap(), vec!["domcontentloaded", "load"]);
    });
}

#[test]
fn should_fire_frame_events() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let attached = t.page.wait_for_frame_event(EventKind::FrameAttached);
        let frame = attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        assert!(attached.await.unwrap() == frame);
        let navigated = t.page.wait_for_frame_event(EventKind::FrameNavigated);
        frame
            .goto(format!("{}/title.html", t.server.prefix))
            .await
            .unwrap();
        assert!(navigated.await.unwrap() == frame);
        let detached = t.page.wait_for_frame_event(EventKind::FrameDetached);
        detach_frame(&t.page, "frame1").await;
        assert!(detached.await.unwrap() == frame);
    });
}

#[test]
fn should_fire_close_event() {
    run(|t| async move {
        let page = t.context.new_page().await.unwrap();
        let closed = Arc::new(Mutex::new(false));
        let c = closed.clone();
        page.on_close(move |_| *c.lock().unwrap() = true);
        let close = page.wait_for_close();
        page.close().await.unwrap();
        close.await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(*closed.lock().unwrap());
    });
}

#[test]
fn wait_for_event_should_fail_when_the_page_closes() {
    run(|t| async move {
        let page = t.context.new_page().await.unwrap();
        let wait = page.wait_for_popup();
        page.close().await.unwrap();
        let err = wait.await.unwrap_err();
        assert!(err.is_target_closed(), "{err}");
    });
}

#[test]
fn context_page_event_should_report_new_pages() {
    run(|t| async move {
        let page = t
            .context
            .expect_page(async { t.context.new_page().await })
            .await
            .unwrap();
        assert!(t.context.pages().contains(&page));
    });
}

#[test]
fn remove_listener_should_work() {
    run(|t| async move {
        let count = Arc::new(Mutex::new(0));
        let c = count.clone();
        let id = t.page.on_request(move |_| *c.lock().unwrap() += 1);
        t.page.goto(&t.server.empty_page).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(t.page.remove_listener(id));
        t.page.goto(&t.server.empty_page).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(*count.lock().unwrap(), 1);
    });
}

// ----------------------------------------------------------- file chooser

#[test]
fn fc_should_emit_event_and_set_files() {
    run(|t| async move {
        t.page.set_content("<input type=file>").await.unwrap();
        let chooser = t
            .page
            .expect_file_chooser(
                t.page
                    .evaluate::<()>("() => document.querySelector('input').click()"),
            )
            .await
            .unwrap();
        assert!(!chooser.is_multiple());
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/assets/file-to-upload.txt");
        chooser.set_files([file]).await.unwrap();
        let name: String = t
            .page
            .evaluate("() => document.querySelector('input').files[0].name")
            .await
            .unwrap();
        assert_eq!(name, "file-to-upload.txt");
    });
}

#[test]
fn fc_should_work_for_multiple() {
    run(|t| async move {
        t.page
            .set_content("<input multiple type=file>")
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
        assert!(chooser.is_multiple());
        let one: Vec<String> = Vec::new();
        chooser.set_files(one).await.unwrap();
    });
}

#[test]
fn fc_should_fail_for_non_multiple_with_several_files() {
    run(|t| async move {
        t.page.set_content("<input type=file>").await.unwrap();
        let chooser = t
            .page
            .expect_file_chooser(
                t.page
                    .evaluate::<()>("() => document.querySelector('input').click()"),
            )
            .await
            .unwrap();
        assert_err_contains(
            chooser.set_files(["a.txt", "b.txt"]).await,
            "Non-multiple file input can only accept single file",
        );
    });
}

// --------------------------------------------------------------- download

fn serve_download(t: &crate::common::TestContext) {
    t.server.set_route("/download", |_| async {
        HttpResponse::builder()
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::CONTENT_DISPOSITION, "attachment")
            .body(Body::from("Hello world"))
            .unwrap()
    });
    t.server.set_route("/downloadWithFilename", |_| async {
        HttpResponse::builder()
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::CONTENT_DISPOSITION, "attachment; filename=file.txt")
            .body(Body::from("Hello world"))
            .unwrap()
    });
}

#[test]
fn do_should_report_downloads_with_accept_downloads() {
    run(|t| async move {
        serve_download(&t);
        t.page
            .set_content(format!(
                "<a href='{}/downloadWithFilename'>download</a>",
                t.server.prefix
            ))
            .await
            .unwrap();
        let download = t
            .page
            .expect_download(
                t.page
                    .evaluate::<()>("() => document.querySelector('a').click()"),
            )
            .await
            .unwrap();
        assert_eq!(
            download.url(),
            format!("{}/downloadWithFilename", t.server.prefix)
        );
        assert_eq!(download.suggested_filename(), "file.txt");
        let path = download.path().await.unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "Hello world");
        assert_eq!(download.failure().await.unwrap(), None);
    });
}

#[test]
fn do_should_save_to_user_specified_path() {
    run(|t| async move {
        serve_download(&t);
        t.page
            .set_content(format!(
                "<a href='{}/download'>download</a>",
                t.server.prefix
            ))
            .await
            .unwrap();
        let download = t
            .page
            .expect_download(
                t.page
                    .evaluate::<()>("() => document.querySelector('a').click()"),
            )
            .await
            .unwrap();
        let dir = std::env::temp_dir().join(format!("ow-dl-{}", std::process::id()));
        let target = dir.join("download.txt");
        download.save_as(&target).await.unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "Hello world");
        let _ = std::fs::remove_dir_all(dir);
    });
}

#[test]
fn do_should_report_download_when_navigation_turns_into_download() {
    run(|t| async move {
        serve_download(&t);
        let download = t.page.wait_for_download();
        let result = t.page.goto(format!("{}/download", t.server.prefix)).await;
        assert_err_contains(result, "Download is starting");
        let download = download.await.unwrap();
        assert_eq!(download.url(), format!("{}/download", t.server.prefix));
        let path = download.path().await.unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "Hello world");
    });
}

#[test]
fn do_should_throw_if_downloads_were_not_accepted() {
    run(|t| async move {
        serve_download(&t);
        let context = t
            .browser
            .new_context()
            .accept_downloads(false)
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.set_content(format!(
            "<a href='{}/download'>download</a>",
            t.server.prefix
        ))
        .await
        .unwrap();
        let download = page
            .expect_download(page.evaluate::<()>("() => document.querySelector('a').click()"))
            .await
            .unwrap();
        assert_err_contains(download.path().await, "acceptDownloads");
        context.close().await.unwrap();
    });
}

// ---------------------------------------------------------------- console

#[test]
fn cm_001_should_work_when_console_events_are_enabled() {
    run(|t| async move {
        let context = t.browser.new_context().console_events(true).await.unwrap();
        let page = context.new_page().await.unwrap();
        let message = page
            .expect_console_message(
                page.evaluate::<()>("() => console.log('hello', 5, {foo: 'bar'})"),
            )
            .await
            .unwrap();
        assert_eq!(message.text(), "hello 5 {foo: bar}");
        assert_eq!(message.r#type(), "log");
        assert_eq!(message.args().len(), 3);
        let value: String = message.args()[0].json_value().await.unwrap();
        assert_eq!(value, "hello");
        context.close().await.unwrap();
    });
}

#[test]
fn co_should_report_warning_type() {
    run(|t| async move {
        let context = t.browser.new_context().console_events(true).await.unwrap();
        let page = context.new_page().await.unwrap();
        let message = page
            .expect_console_message(page.evaluate::<()>("() => console.warn('w')"))
            .await
            .unwrap();
        assert_eq!(message.r#type(), "warning");
        context.close().await.unwrap();
    });
}

#[test]
fn pe_should_fire_pageerror() {
    run(|t| async move {
        let context = t.browser.new_context().console_events(true).await.unwrap();
        let page = context.new_page().await.unwrap();
        let error = page.wait_for_page_error();
        page.evaluate::<()>("() => { setTimeout(() => { throw new Error('Fancy error!'); }, 0); }")
            .await
            .unwrap();
        let error = error.await.unwrap();
        assert_eq!(error.name, "Error");
        assert_eq!(error.message, "Fancy error!");
        assert!(error.stack.contains("Fancy error!"));
        context.close().await.unwrap();
    });
}

#[test]
fn console_events_are_off_by_default() {
    run(|t| async move {
        let messages = Arc::new(Mutex::new(0));
        let m = messages.clone();
        t.page.on_console(move |_| *m.lock().unwrap() += 1);
        t.page
            .evaluate::<()>("() => console.log('x')")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(*messages.lock().unwrap(), 0);
    });
}

// ---------------------------------------------------------------- workers

#[test]
fn wo_page_workers_should_work() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let worker = t
            .page
            .expect_worker(t.page.evaluate::<()>(
                "() => { window.w = new Worker(URL.createObjectURL(new Blob(['self.workerFunction = () => \"worker function result\"; console.log(\"hello\")'], { type: 'application/javascript' }))); }",
            ))
            .await
            .unwrap();
        assert!(worker.url().contains("blob"));
        assert_eq!(t.page.workers().len(), 1);
        let mut result = String::new();
        for _ in 0..50 {
            match worker
                .evaluate::<String>("() => self.workerFunction()")
                .await
            {
                Ok(r) => {
                    result = r;
                    break;
                }
                Err(e) => eprintln!("worker evaluate: {e}"),
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(result, "worker function result");
        let sum: i32 = worker
            .evaluate("([a, b]) => a + b")
            .arg((1, 2))
            .await
            .unwrap();
        assert_eq!(sum, 3);
        t.page
            .evaluate::<()>("() => window.w.terminate()")
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), worker.wait_for_close())
            .await
            .unwrap();
        assert!(worker.is_closed());
    });
}

#[test]
fn wo_should_report_errors() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let worker = t
            .page
            .expect_worker(t.page.evaluate::<()>(
                "() => { window.w = new Worker(URL.createObjectURL(new Blob(['1'], { type: 'application/javascript' }))); }",
            ))
            .await
            .unwrap();
        let result = worker
            .evaluate::<()>("() => { throw new Error('boom') }")
            .await;
        assert_err_contains(result, "boom");
    });
}

// --------------------------------------------------------------- websocket

#[test]
fn ws_should_work() {
    run(|t| async move {
        let port = crate::common::start_ws_echo_server().await;
        t.page.goto(&t.server.empty_page).await.unwrap();
        let ws = t
            .page
            .expect_websocket(
                t.page
                    .evaluate::<()>(
                        "port => { window.ws = new WebSocket('ws://localhost:' + port + '/ws'); window.opened = new Promise(f => window.ws.addEventListener('open', f)); }",
                    )
                    .arg(port),
            )
            .await
            .unwrap();
        let received = ws.wait_for_event(
            |e| matches!(e, oxidewright::WebSocketEvent::FrameReceived(_)),
            10_000,
        );
        t.page
            .evaluate::<()>("async () => { await window.opened; window.ws.send('text'); }")
            .await
            .unwrap();
        assert_eq!(ws.url(), format!("ws://localhost:{port}/ws"));
        let received = received.await.unwrap();
        assert_eq!(
            received,
            oxidewright::WebSocketEvent::FrameReceived(oxidewright::WebSocketPayload::Text(
                "text".into()
            ))
        );
        let close = ws.wait_for_event(|e| *e == oxidewright::WebSocketEvent::Close, 10_000);
        t.page
            .evaluate::<()>("() => window.ws.close()")
            .await
            .unwrap();
        close.await.unwrap();
        assert!(ws.is_closed());
    });
}

#[test]
fn ws_should_report_frames() {
    run(|t| async move {
        let port = crate::common::start_ws_echo_server().await;
        t.page.goto(&t.server.empty_page).await.unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let l = log.clone();
        t.page.on_websocket(move |ws| {
            let l1 = l.clone();
            ws.on_frame_sent(move |p| l1.lock().unwrap().push(format!("sent<{}>", p.as_text())));
            let l2 = l.clone();
            ws.on_frame_received(move |p| {
                l2.lock()
                    .unwrap()
                    .push(format!("received<{}>", p.as_text()))
            });
            let l3 = l.clone();
            ws.on_close(move || l3.lock().unwrap().push("close".into()));
        });
        let _: () = t
            .page
            .evaluate(
                "port => new Promise(resolve => { const ws = new WebSocket('ws://localhost:' + port + '/ws'); ws.addEventListener('open', () => ws.send('outgoing')); ws.addEventListener('message', () => { ws.close(); }); ws.addEventListener('close', () => resolve()); })",
            )
            .arg(port)
            .await
            .unwrap();
        for _ in 0..50 {
            if log.lock().unwrap().len() >= 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(
            *log.lock().unwrap(),
            vec!["sent<outgoing>", "received<outgoing>", "close"]
        );
    });
}

// ----------------------------------------------------------- helper check

#[allow(dead_code)]
fn _assert_send(page: Page) -> impl Send {
    page
}
