//! `page.screencast()`, ported from Playwright's
//! `tests/library/screencast.spec.ts`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use oxidewright::{ScreencastFrame, ViewportSize};

use crate::common::*;

async fn ensure_some_frames(page: &oxidewright::Page) {
    for i in 0..10 {
        page.evaluate::<()>("i => { document.body.textContent = 'frame ' + i; }")
            .arg(i)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn size(width: u32, height: u32) -> ViewportSize {
    ViewportSize { width, height }
}

#[test]
fn sc_001_screencast_start_delivers_frames_via_on_frame() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .viewport(Some(size(1000, 400)))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        let frames: Arc<Mutex<Vec<ScreencastFrame>>> = Arc::default();
        let sink = frames.clone();
        page.screencast()
            .start()
            .on_frame(move |f| sink.lock().unwrap().push(f))
            .size(size(500, 400))
            .await
            .unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        page.evaluate::<()>("() => { document.body.style.backgroundColor = 'red'; }")
            .await
            .unwrap();
        ensure_some_frames(&page).await;
        // Slow runners deliver the first frames late: keep the page changing
        // until some arrive (10 s at most).
        for _ in 0..20 {
            if !frames.lock().unwrap().is_empty() {
                break;
            }
            ensure_some_frames(&page).await;
        }
        page.screencast().stop().await.unwrap();
        let frames = frames.lock().unwrap().clone();
        assert!(!frames.is_empty());
        for frame in &frames {
            // A JPEG starts with FF D8.
            assert_eq!(&frame.data[..2], &[0xff, 0xd8]);
            // SC-003: the viewport size of the page.
            assert_eq!((frame.viewport_width, frame.viewport_height), (1000, 400));
            assert!(frame.timestamp > 0.0);
        }
        context.close().await.unwrap();
    });
}

#[test]
fn sc_004_start_throws_if_screencast_is_already_started() {
    run(|t| async move {
        t.page.screencast().start().on_frame(|_| {}).await.unwrap();
        let error = t
            .page
            .screencast()
            .start()
            .on_frame(|_| {})
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("Screencast is already started"),
            "{error}"
        );
        t.page.screencast().stop().await.unwrap();
    });
}

#[test]
fn sc_005_start_allows_restart_with_different_options_after_stop() {
    run(|t| async move {
        let screencast = t.page.screencast();
        screencast
            .start()
            .on_frame(|_| {})
            .size(size(500, 400))
            .await
            .unwrap();
        screencast.stop().await.unwrap();
        screencast
            .start()
            .on_frame(|_| {})
            .size(size(320, 240))
            .await
            .unwrap();
        screencast.stop().await.unwrap();
    });
}

#[test]
fn sc_010_start_should_throw_on_invalid_fps() {
    run(|t| async move {
        let error = t
            .page
            .screencast()
            .start()
            .on_frame(|_| {})
            .fps(-1.0)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("\"fps\" must be a positive number, got -1"),
            "{error}"
        );
    });
}

#[test]
fn sc_012_stop_should_not_fail_when_no_recording_is_in_progress() {
    run(|t| async move {
        t.page.screencast().stop().await.unwrap();
    });
}

#[test]
fn sc_013_start_should_finish_when_page_is_closed() {
    run(|t| async move {
        if oxidewright::video::ffmpeg_path().is_none() {
            return;
        }
        let page = t.context.new_page().await.unwrap();
        let path = std::env::temp_dir().join(format!("oxw-sc013-{}.webm", std::process::id()));
        page.screencast()
            .start()
            .path(&path)
            .size(size(800, 800))
            .await
            .unwrap();
        page.evaluate::<()>("() => { document.body.style.backgroundColor = 'red'; }")
            .await
            .unwrap();
        ensure_some_frames(&page).await;
        page.close().await.unwrap();
        let error = page.screencast().stop().await.unwrap_err();
        assert!(error.is_target_closed(), "{error}");
        let _ = std::fs::remove_file(&path);
    });
}

#[test]
fn sc_015_stop_finalizes_the_recorded_video() {
    run(|t| async move {
        if oxidewright::video::ffmpeg_path().is_none() {
            return;
        }
        let path = std::env::temp_dir().join(format!("oxw-sc015-{}.webm", std::process::id()));
        t.page
            .screencast()
            .start()
            .path(&path)
            .size(size(800, 800))
            .await
            .unwrap();
        t.page
            .evaluate::<()>("() => { document.body.style.backgroundColor = 'red'; }")
            .await
            .unwrap();
        ensure_some_frames(&t.page).await;
        t.page.screencast().stop().await.unwrap();
        let bytes = std::fs::read(&path).unwrap();
        // A WebM (EBML) file starts with 1A 45 DF A3.
        assert_eq!(&bytes[..4], &[0x1a, 0x45, 0xdf, 0xa3]);
        let _ = std::fs::remove_file(&path);
    });
}
