//! Port of `page-set-input-files.spec.ts`.
//!
//! Skipped:
//! - IF-008 `should upload a file after popup`: needs the `popup` event.
//! - IF-009 / IF-011 `should upload large file (with relative path)`, IF-016 `should
//!   detect mime type`, IF-017 `should not trim big uploaded files`: need a multipart
//!   form parser on the test server (IF-017 does not use the upload API at all).
//!
//! IF-018 records the events in the page instead of using `exposeFunction`.

use std::path::PathBuf;

use axum::body::Body;
use axum::http::header;
use axum::response::Response;
use oxidewright::FilePayload;

use super::asset;
use crate::common::*;

/// A fresh `<tmp>/<unique>/file-upload-test` directory.
fn upload_dir() -> PathBuf {
    let unique: String = format!(
        "oxidewright-upload-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let dir = std::env::temp_dir().join(unique).join("file-upload-test");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const READ_FIRST_FILE: &str = "e => { const reader = new FileReader(); const promise = new Promise(fulfill => reader.onload = fulfill); reader.readAsText(e.files[0]); return promise.then(() => reader.result); }";

#[test]
fn should_upload_the_file() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/fileupload.html", t.server.prefix))
            .await
            .unwrap();
        let input = t.page.query_selector("input").await.unwrap().unwrap();
        input
            .set_input_files(asset("file-to-upload.txt"))
            .await
            .unwrap();
        let name: String = t
            .page
            .evaluate("e => e.files[0].name")
            .arg(&input)
            .await
            .unwrap();
        assert_eq!(name, "file-to-upload.txt");
        let content: String = t.page.evaluate(READ_FIRST_FILE).arg(&input).await.unwrap();
        assert_eq!(content, "contents of the file");
    });
}

#[test]
fn should_upload_a_folder() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/folderupload.html", t.server.prefix))
            .await
            .unwrap();
        let input = t.page.query_selector("input").await.unwrap().unwrap();
        let dir = upload_dir();
        std::fs::write(dir.join("file1.txt"), "file1 content").unwrap();
        std::fs::write(dir.join("file2"), "file2 content").unwrap();
        std::fs::create_dir(dir.join("sub-dir")).unwrap();
        std::fs::write(
            dir.join("sub-dir").join("really.txt"),
            "sub-dir file content",
        )
        .unwrap();
        input.set_input_files(dir.clone()).await.unwrap();
        let mut paths: Vec<String> = t
            .page
            .evaluate("e => [...e.files].map(f => f.webkitRelativePath)")
            .arg(&input)
            .await
            .unwrap();
        let relative = paths.clone();
        paths.sort();
        assert_eq!(
            paths,
            [
                "file-upload-test/file1.txt",
                "file-upload-test/file2",
                "file-upload-test/sub-dir/really.txt",
            ]
        );
        for (i, path) in relative.iter().enumerate() {
            let content: String = input
                .evaluate("(e, i) => { const reader = new FileReader(); const promise = new Promise(fulfill => reader.onload = fulfill); reader.readAsText(e.files[i]); return promise.then(() => reader.result); }")
                .arg(i)
                .await
                .unwrap();
            let expected = std::fs::read_to_string(dir.parent().unwrap().join(path)).unwrap();
            assert_eq!(content, expected);
        }
    });
}

#[test]
fn should_upload_a_folder_via_label() {
    run(|t| async move {
        t.page
            .set_content("<label for=\"folder\">Folder</label><input id=\"folder\" type=\"file\" webkitdirectory>")
            .await
            .unwrap();
        let dir = upload_dir();
        std::fs::write(dir.join("file1.txt"), "file1 content").unwrap();
        t.page
            .get_by_text("Folder")
            .set_input_files(dir)
            .await
            .unwrap();
        let paths: Vec<String> = t
            .page
            .locator("input")
            .evaluate("e => [...e.files].map(f => f.webkitRelativePath)")
            .await
            .unwrap();
        assert_eq!(paths, ["file-upload-test/file1.txt"]);
    });
}

#[test]
fn should_upload_a_folder_and_throw_for_multiple_directories() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/folderupload.html", t.server.prefix))
            .await
            .unwrap();
        let input = t.page.query_selector("input").await.unwrap().unwrap();
        let dir = upload_dir();
        for folder in ["folder1", "folder2"] {
            std::fs::create_dir_all(dir.join(folder)).unwrap();
            std::fs::write(dir.join(folder).join("file.txt"), "content").unwrap();
        }
        let error = input
            .set_input_files(vec![dir.join("folder1"), dir.join("folder2")])
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Multiple directories are not supported"),
            "{error}"
        );
    });
}

#[test]
fn should_throw_if_a_directory_and_files_are_passed() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/folderupload.html", t.server.prefix))
            .await
            .unwrap();
        let input = t.page.query_selector("input").await.unwrap().unwrap();
        let dir = upload_dir();
        std::fs::create_dir_all(dir.join("folder1")).unwrap();
        std::fs::write(dir.join("folder1").join("file1.txt"), "file1 content").unwrap();
        let error = input
            .set_input_files(vec![
                dir.join("folder1"),
                dir.join("folder1").join("file1.txt"),
            ])
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("File paths must be all files or a single directory"),
            "{error}"
        );
    });
}

#[test]
fn should_throw_when_uploading_a_folder_in_a_normal_file_upload_input() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/fileupload.html", t.server.prefix))
            .await
            .unwrap();
        let input = t.page.query_selector("input").await.unwrap().unwrap();
        let dir = upload_dir();
        std::fs::write(dir.join("file1.txt"), "file1 content").unwrap();
        let error = input.set_input_files(dir).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("File input does not support directories, pass individual files instead"),
            "{error}"
        );
    });
}

#[test]
fn should_throw_when_uploading_a_file_in_a_directory_upload_input() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/folderupload.html", t.server.prefix))
            .await
            .unwrap();
        let input = t.page.query_selector("input").await.unwrap().unwrap();
        let error = input
            .set_input_files(asset("file to upload.txt"))
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("[webkitdirectory] input requires passing a path to a directory"),
            "{error}"
        );
    });
}

#[test]
fn should_throw_an_error_if_the_file_does_not_exist() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/fileupload.html", t.server.prefix))
            .await
            .unwrap();
        let input = t.page.query_selector("input").await.unwrap().unwrap();
        let error = input
            .set_input_files("i actually do not exist.txt")
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("ENOENT: no such file or directory"),
            "{message}"
        );
        assert!(message.contains("i actually do not exist.txt"), "{message}");
    });
}

#[test]
fn should_upload_the_file_with_spaces_in_name() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/fileupload.html", t.server.prefix))
            .await
            .unwrap();
        let input = t.page.query_selector("input").await.unwrap().unwrap();
        input
            .set_input_files(asset("file to upload.txt"))
            .await
            .unwrap();
        let name: String = t
            .page
            .evaluate("e => e.files[0].name")
            .arg(&input)
            .await
            .unwrap();
        assert_eq!(name, "file to upload.txt");
        let content: String = t.page.evaluate(READ_FIRST_FILE).arg(&input).await.unwrap();
        assert_eq!(content, "contents of the file");
    });
}

async fn assert_single_file(t: &TestContext, name: &str) {
    let len: usize = t
        .page
        .eval_on_selector("input", "input => input.files.length")
        .await
        .unwrap();
    assert_eq!(len, 1);
    let actual: String = t
        .page
        .eval_on_selector("input", "input => input.files[0].name")
        .await
        .unwrap();
    assert_eq!(actual, name);
}

#[test]
fn should_work() {
    run(|t| async move {
        t.page.set_content("<input type=file>").await.unwrap();
        t.page
            .set_input_files("input", asset("file-to-upload.txt"))
            .await
            .unwrap();
        assert_single_file(&t, "file-to-upload.txt").await;
    });
}

#[test]
fn should_set_from_memory() {
    run(|t| async move {
        t.page.set_content("<input type=file>").await.unwrap();
        t.page
            .set_input_files(
                "input",
                FilePayload::new("test.txt", "text/plain", b"this is a test".to_vec()),
            )
            .await
            .unwrap();
        assert_single_file(&t, "test.txt").await;
    });
}

#[test]
fn should_work_with_csp() {
    run(|t| async move {
        t.server.set_route("/empty.html", |_| async {
            Response::builder()
                .header(header::CONTENT_TYPE, "text/html")
                .header("content-security-policy", "default-src \"none\"")
                .body(Body::empty())
                .unwrap()
        });
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page.set_content("<input type=file>").await.unwrap();
        t.page
            .set_input_files("input", asset("file-to-upload.txt"))
            .await
            .unwrap();
        assert_single_file(&t, "file-to-upload.txt").await;
    });
}

#[test]
fn should_emit_input_and_change_events() {
    run(|t| async move {
        t.page
            .set_content(
                "<input id=input type=file></input>
  <script>
    window.events = [];
    input.addEventListener('input', e => window.events.push({ type: e.type }));
    input.addEventListener('change', e => window.events.push({ type: e.type }));
  </script>",
            )
            .await
            .unwrap();
        t.page
            .query_selector("input")
            .await
            .unwrap()
            .unwrap()
            .set_input_files(asset("file-to-upload.txt"))
            .await
            .unwrap();
        let events: Vec<String> = t
            .page
            .evaluate("() => window.events.map(e => e.type)")
            .await
            .unwrap();
        assert_eq!(events, ["input", "change"]);
    });
}

#[test]
fn input_event_composed_should_be_true_and_cross_shadow_dom_boundary() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .set_content(
                "<body><script>
  const div = document.createElement('div');
  const shadowRoot = div.attachShadow({mode: 'open'});
  shadowRoot.innerHTML = '<input type=file></input>';
  document.body.appendChild(div);
</script></body>",
            )
            .await
            .unwrap();
        t.page
            .locator("body")
            .evaluate::<()>(
                "select => { window.firedBodyEvents = []; for (const event of ['input', 'change']) select.addEventListener(event, e => window.firedBodyEvents.push(e.type + ':' + e.composed), false); }",
            )
            .await
            .unwrap();
        t.page
            .locator("input")
            .evaluate::<()>(
                "select => { window.firedEvents = []; for (const event of ['input', 'change']) select.addEventListener(event, e => window.firedEvents.push(e.type + ':' + e.composed), false); }",
            )
            .await
            .unwrap();
        t.page
            .locator("input")
            .set_input_files(FilePayload::new(
                "test.txt",
                "text/plain",
                b"this is a test".to_vec(),
            ))
            .await
            .unwrap();
        let fired: Vec<String> = t
            .page
            .evaluate("() => window['firedEvents']")
            .await
            .unwrap();
        assert_eq!(fired, ["input:true", "change:false"]);
        let body: Vec<String> = t
            .page
            .evaluate("() => window['firedBodyEvents']")
            .await
            .unwrap();
        assert_eq!(body, ["input:true"]);
    });
}

#[test]
fn input_should_trigger_events_when_files_changed_second_time() {
    run(|t| async move {
        t.page
            .set_content("<input type=file multiple=true/>")
            .await
            .unwrap();
        let input = t.page.locator("input");
        let events = input
            .evaluate_handle(
                "e => { const events = []; e.addEventListener('input', () => events.push('input')); e.addEventListener('change', () => events.push('change')); return events; }",
            )
            .await
            .unwrap();
        input
            .set_input_files(asset("file-to-upload.txt"))
            .await
            .unwrap();
        let name: String = input.evaluate("e => e.files[0].name").await.unwrap();
        assert_eq!(name, "file-to-upload.txt");
        let list: Vec<String> = events.evaluate("e => e").await.unwrap();
        assert_eq!(list, ["input", "change"]);
        events
            .evaluate::<()>("e => { e.length = 0; }")
            .await
            .unwrap();
        input.set_input_files(asset("pptr.png")).await.unwrap();
        let name: String = input.evaluate("e => e.files[0].name").await.unwrap();
        assert_eq!(name, "pptr.png");
        let list: Vec<String> = events.evaluate("e => e").await.unwrap();
        assert_eq!(list, ["input", "change"]);
    });
}

#[test]
fn should_preserve_last_modified_timestamp() {
    run(|t| async move {
        t.page
            .set_content("<input type=file multiple=true/>")
            .await
            .unwrap();
        let input = t.page.locator("input");
        let files = ["file-to-upload.txt", "file-to-upload-2.txt"];
        input
            .set_input_files(files.iter().map(|f| asset(f)).collect::<Vec<_>>())
            .await
            .unwrap();
        let names: Vec<String> = input
            .evaluate("e => [...e.files].map(f => f.name)")
            .await
            .unwrap();
        assert_eq!(names, files);
        let timestamps: Vec<f64> = input
            .evaluate("e => [...e.files].map(f => f.lastModified)")
            .await
            .unwrap();
        for (i, file) in files.iter().enumerate() {
            let mtime = std::fs::metadata(asset(file))
                .unwrap()
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as f64;
            assert!(
                (timestamps[i] - mtime).abs() <= 1000.0,
                "{timestamps:?} vs {mtime}"
            );
        }
    });
}

#[test]
fn should_store_an_uploaded_file_in_indexed_db() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/fileupload.html", t.server.prefix))
            .await
            .unwrap();
        let input = t.page.locator("input[type=file]");
        input
            .set_input_files(asset("file-to-upload.txt"))
            .await
            .unwrap();
        let stored: serde_json::Value = input
            .evaluate(
                "async input => {
    const db = await new Promise((resolve, reject) => {
      const request = indexedDB.open('file-upload');
      request.onupgradeneeded = () => request.result.createObjectStore('files');
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    const file = await new Promise((resolve, reject) => {
      const transaction = db.transaction('files', 'readwrite');
      const store = transaction.objectStore('files');
      store.put(input.files[0], 'file');
      const request = store.get('file');
      transaction.oncomplete = () => resolve(request.result);
      transaction.onabort = () => reject(transaction.error);
    });
    db.close();
    return { name: file.name, text: await file.text() };
  }",
            )
            .await
            .unwrap();
        assert_eq!(
            stored,
            serde_json::json!({ "name": "file-to-upload.txt", "text": "contents of the file" })
        );
    });
}
