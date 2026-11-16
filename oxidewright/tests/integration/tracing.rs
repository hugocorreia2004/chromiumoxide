//! Port of Playwright's `tests/library/tracing.spec.ts` (the parts that do
//! not depend on per-action events).
//!
//! Skipped (oxidewright does not record per-action `before`/`after` events,
//! DOM snapshots, network entries, console messages or sources, see
//! `oxidewright::trace`): TR-001, TR-002, TR-004 to TR-007, TR-009 to TR-011,
//! TR-013, TR-014, TR-016, TR-017, TR-019, TR-021, TR-022, TR-024, TR-031 to
//! TR-042. Disposables returned by `start` (TR-028, TR-029) do not exist in
//! the Rust API. TR-030 is ported without its action/snapshot assertions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::common::*;

/// Entries of a zip with stored (uncompressed) entries.
fn read_zip(path: &Path) -> HashMap<String, Vec<u8>> {
    let bytes = std::fs::read(path).expect("trace zip");
    let mut out = HashMap::new();
    let mut i = 0;
    let u16_at = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]) as usize;
    let u32_at =
        |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
    while i + 30 <= bytes.len() && bytes[i..i + 4] == [0x50, 0x4b, 0x03, 0x04] {
        assert_eq!(u16_at(&bytes, i + 8), 0, "stored entry");
        let size = u32_at(&bytes, i + 18);
        let name_len = u16_at(&bytes, i + 26);
        let extra_len = u16_at(&bytes, i + 28);
        let name = String::from_utf8(bytes[i + 30..i + 30 + name_len].to_vec()).unwrap();
        let start = i + 30 + name_len + extra_len;
        out.insert(name, bytes[start..start + size].to_vec());
        i = start + size;
    }
    out
}

fn events(zip: &HashMap<String, Vec<u8>>) -> Vec<Value> {
    String::from_utf8(zip["trace.trace"].clone())
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn out_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("oxidewright-tracing-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn should_collect_screencast_frames_and_context_options() {
    run(|t| async move {
        let dir = out_dir("frames");
        let trace = dir.join("trace.zip");
        t.context
            .tracing()
            .start()
            .screenshots(true)
            .title("my trace")
            .await
            .unwrap();
        t.page
            .goto(format!("{}/grid.html", t.server.prefix))
            .await
            .unwrap();
        for color in ["red", "green", "blue"] {
            t.page
                .evaluate::<()>("c => document.body.style.backgroundColor = c")
                .arg(color)
                .await
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }
        t.context.tracing().stop().path(&trace).await.unwrap();

        let zip = read_zip(&trace);
        assert!(zip.contains_key("trace.network"));
        let events = events(&zip);
        assert_eq!(events[0]["type"], "context-options");
        assert_eq!(events[0]["browserName"], "chromium");
        assert_eq!(events[0]["title"], "my trace");
        assert_eq!(events[0]["options"]["viewport"]["width"], 1280);
        let frames: Vec<&Value> = events
            .iter()
            .filter(|e| e["type"] == "screencast-frame")
            .collect();
        assert!(!frames.is_empty(), "no screencast frames");
        for frame in frames {
            let file = frame["file"].as_str().unwrap();
            let data = zip.get(file).unwrap_or_else(|| panic!("missing {file}"));
            assert_eq!(&data[..2], &[0xff, 0xd8], "jpeg");
            assert!(frame["width"].as_u64().unwrap() > 0);
        }
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn tr_008_can_call_tracing_group_group_end_at_any_time_and_auto_close() {
    run(|t| async move {
        let dir = out_dir("groups");
        let tracing = t.context.tracing();
        tracing.group("ignored").await.unwrap();
        tracing.group_end().await.unwrap();
        tracing.group("ignored2").await.unwrap();

        tracing.start().await.unwrap();
        tracing.group("actual").await.unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        tracing
            .stop_chunk()
            .path(dir.join("trace.zip"))
            .await
            .unwrap();

        tracing.group("ignored3").await.unwrap();
        tracing.group_end().await.unwrap();
        tracing.group_end().await.unwrap();
        tracing.group_end().await.unwrap();

        let events = events(&read_zip(&dir.join("trace.zip")));
        let groups: Vec<&Value> = events
            .iter()
            .filter(|e| e["method"] == "tracingGroup")
            .collect();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["title"], "actual");
        let call_id = &groups[0]["callId"];
        assert!(
            events
                .iter()
                .any(|e| e["type"] == "after" && &e["callId"] == call_id)
        );
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn tr_012_should_collect_two_traces() {
    run(|t| async move {
        let dir = out_dir("two");
        let tracing = t.context.tracing();
        tracing
            .start()
            .screenshots(true)
            .snapshots(true)
            .await
            .unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        tracing.stop().path(dir.join("trace1.zip")).await.unwrap();
        tracing.start().screenshots(true).await.unwrap();
        t.page.close().await.unwrap();
        tracing.stop().path(dir.join("trace2.zip")).await.unwrap();
        for name in ["trace1.zip", "trace2.zip"] {
            let events = events(&read_zip(&dir.join(name)));
            assert_eq!(events[0]["type"], "context-options");
        }
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn tr_015_should_overwrite_existing_file() {
    run(|t| async move {
        let dir = out_dir("overwrite");
        let path = dir.join("trace1.zip");
        let tracing = t.context.tracing();
        tracing.start().screenshots(true).await.unwrap();
        t.page
            .goto(format!("{}/grid.html", t.server.prefix))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        tracing.stop().path(&path).await.unwrap();
        let first = read_zip(&path).len();
        tracing.start().await.unwrap();
        tracing.stop().path(&path).await.unwrap();
        let second = read_zip(&path);
        assert!(second.len() <= first);
        assert!(!second.keys().any(|k| k.ends_with(".jpeg")));
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn tr_025_should_throw_when_starting_with_different_options() {
    run(|t| async move {
        t.context
            .tracing()
            .start()
            .screenshots(true)
            .snapshots(true)
            .await
            .unwrap();
        let result = t.context.tracing().start().await;
        assert_err_contains(result, "Tracing has been already started");
    });
}

#[test]
fn tr_026_should_throw_when_stopping_without_start() {
    run(|t| async move {
        let dir = out_dir("nostart");
        let result = t.context.tracing().stop().path(dir.join("trace.zip")).await;
        assert_err_contains(result, "Must start tracing before stopping");
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn tr_027_should_not_throw_when_stopping_without_start_but_not_exporting() {
    run(|t| async move {
        t.context.tracing().stop().await.unwrap();
    });
}

#[test]
fn tr_030_should_work_with_multiple_chunks() {
    run(|t| async move {
        let dir = out_dir("chunks");
        let tracing = t.context.tracing();
        tracing.start().screenshots(true).await.unwrap();
        t.page
            .goto(format!("{}/frames/frame.html", t.server.prefix))
            .await
            .unwrap();
        tracing.stop_chunk().await.unwrap();
        tracing.start_chunk().await.unwrap();
        t.page.set_content("<button>Click</button>").await.unwrap();
        tracing
            .stop_chunk()
            .path(dir.join("trace.zip"))
            .await
            .unwrap();
        tracing.start_chunk().await.unwrap();
        tracing
            .stop_chunk()
            .path(dir.join("trace2.zip"))
            .await
            .unwrap();
        tracing.start_chunk().await.unwrap();
        tracing.stop_chunk().await.unwrap();
        for name in ["trace.zip", "trace2.zip"] {
            let events = events(&read_zip(&dir.join(name)));
            assert_eq!(events[0]["type"], "context-options");
        }
        let _ = std::fs::remove_dir_all(&dir);
    });
}
