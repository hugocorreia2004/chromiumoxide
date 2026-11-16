//! Port of Playwright's `tests/library/video.spec.ts`. Tests that record a
//! video are skipped at runtime (with a message) when no ffmpeg binary is
//! available (`OXIDEWRIGHT_FFMPEG_PATH` or `ffmpeg` on `PATH`).
//!
//! Skipped: tests decoding video frames/pixels with Playwright's
//! `VideoPlayer` (VI-003, VI-004, VI-009, VI-012, VI-013, VI-015, VI-028,
//! VI-029) — only the WebM header is checked here; `recordVideo.fps` and
//! `showActions` (VI-018, VI-019) are not implemented; popups (VI-005,
//! VI-011, VI-014) and persistent contexts (VI-021) depend on APIs owned by
//! other modules; browser crash/close handling (VI-023 to VI-025), internal
//! pages (VI-027) and video+trace (VI-030) are not ported.

use std::path::PathBuf;

use crate::common::*;

fn has_ffmpeg() -> bool {
    if oxidewright::video::ffmpeg_path().is_some() {
        return true;
    }
    eprintln!("skipping: ffmpeg not found (set OXIDEWRIGHT_FFMPEG_PATH)");
    false
}

fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("oxidewright-video-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Matroska/WebM files start with the EBML magic number.
fn assert_webm(path: &std::path::Path) {
    let bytes = std::fs::read(path).expect("video file");
    assert!(bytes.len() > 100, "video too small: {} bytes", bytes.len());
    assert_eq!(&bytes[..4], &[0x1a, 0x45, 0xdf, 0xa3], "EBML header");
}

async fn ensure_some_frames(page: &oxidewright::Page) {
    for color in ["red", "green", "blue", "yellow"] {
        page.evaluate::<()>("c => document.body.style.backgroundColor = c")
            .arg(color)
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[test]
fn vi_001_should_not_have_video_by_default() {
    run(|t| async move {
        assert!(t.page.video().is_none());
    });
}

#[test]
fn vi_006_should_expose_video_path() {
    if !has_ffmpeg() {
        return;
    }
    run(|t| async move {
        let dir = out_dir("path");
        let context = t
            .browser
            .new_context()
            .record_video_dir(&dir)
            .record_video_size((320, 240))
            .viewport(Some((320, 240).into()))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.evaluate::<()>("() => document.body.style.backgroundColor = 'red'")
            .await
            .unwrap();
        let path = page.video().unwrap().path();
        assert!(path.starts_with(&dir));
        assert_eq!(path.extension().unwrap(), "webm");
        context.close().await.unwrap();
        assert!(path.exists());
        assert_webm(&path);
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn vi_007_should_delete_video() {
    if !has_ffmpeg() {
        return;
    }
    run(|t| async move {
        let dir = out_dir("delete");
        let context = t
            .browser
            .new_context()
            .record_video_dir(&dir)
            .record_video_size((320, 240))
            .viewport(Some((320, 240).into()))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        let video = page.video().unwrap();
        let delete = tokio::spawn({
            let video = video.clone();
            async move { video.delete().await }
        });
        ensure_some_frames(&page).await;
        context.close().await.unwrap();
        delete.await.unwrap().unwrap();
        assert!(!video.path().exists());
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn vi_008_should_expose_video_path_blank_page() {
    if !has_ffmpeg() {
        return;
    }
    run(|t| async move {
        let dir = out_dir("blank");
        let context = t
            .browser
            .new_context()
            .record_video_dir(&dir)
            .record_video_size((320, 240))
            .viewport(Some((320, 240).into()))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        let path = page.video().unwrap().path();
        assert!(path.starts_with(&dir));
        context.close().await.unwrap();
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn vi_031_should_save_as_video() {
    if !has_ffmpeg() {
        return;
    }
    run(|t| async move {
        let dir = out_dir("saveas");
        let context = t
            .browser
            .new_context()
            .record_video_dir(&dir)
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        ensure_some_frames(&page).await;
        let video = page.video().unwrap();
        page.close().await.unwrap();
        let saved = dir.join("my-video.webm");
        video.save_as(&saved).await.unwrap();
        assert_webm(&saved);
        context.close().await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn page_close_finalizes_the_video() {
    if !has_ffmpeg() {
        return;
    }
    run(|t| async move {
        let dir = out_dir("close");
        let context = t
            .browser
            .new_context()
            .record_video_dir(&dir)
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(format!("{}/grid.html", t.server.prefix))
            .await
            .unwrap();
        ensure_some_frames(&page).await;
        let path = page.video().unwrap().path();
        page.close().await.unwrap();
        assert_webm(&path);
        context.close().await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    });
}
