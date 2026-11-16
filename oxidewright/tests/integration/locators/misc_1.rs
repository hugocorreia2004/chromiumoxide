//! Port of `locator-misc-1.spec.ts`.
//!
//! LO-136 records focus/blur in page globals instead of `exposeFunction`
//! (not available yet); everything else is ported as is.

use serde_json::Value;

use crate::common::*;

#[test]
fn should_hover() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/scrollable.html", t.server.prefix))
            .await
            .unwrap();
        t.page.locator("#button-6").hover().await.unwrap();
        let id: String = t
            .page
            .evaluate("() => document.querySelector('button:hover').id")
            .await
            .unwrap();
        assert_eq!(id, "button-6");
    });
}

#[test]
fn should_hover_when_node_is_removed() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/scrollable.html", t.server.prefix))
            .await
            .unwrap();
        t.page
            .evaluate::<Value>("() => delete window['Node']")
            .await
            .unwrap();
        t.page.locator("#button-6").hover().await.unwrap();
        let id: String = t
            .page
            .evaluate("() => document.querySelector('button:hover').id")
            .await
            .unwrap();
        assert_eq!(id, "button-6");
    });
}

#[test]
fn should_fill_input() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        t.page
            .evaluate::<Value>(
                "() => document.querySelector('input').addEventListener('input', e => window.result = e.target.value)",
            )
            .await
            .unwrap();
        t.page.locator("input").fill("some value").await.unwrap();
        let result: String = t.page.evaluate("() => window['result']").await.unwrap();
        assert_eq!(result, "some value");
    });
}

#[test]
fn should_fill_input_when_node_is_removed() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        t.page
            .evaluate::<Value>(
                "() => document.querySelector('input').addEventListener('input', e => window.result = e.target.value)",
            )
            .await
            .unwrap();
        t.page
            .evaluate::<Value>("() => delete window['Node']")
            .await
            .unwrap();
        t.page.locator("input").fill("some value").await.unwrap();
        let result: String = t.page.evaluate("() => window['result']").await.unwrap();
        assert_eq!(result, "some value");
    });
}

#[test]
fn should_clear_input() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        t.page
            .evaluate::<Value>(
                "() => document.querySelector('input').addEventListener('input', e => window.result = e.target.value)",
            )
            .await
            .unwrap();
        let handle = t.page.locator("input");
        handle.fill("some value").await.unwrap();
        let result: String = t.page.evaluate("() => window['result']").await.unwrap();
        assert_eq!(result, "some value");
        handle.clear().await.unwrap();
        let result: String = t.page.evaluate("() => window['result']").await.unwrap();
        assert_eq!(result, "");
    });
}

#[test]
fn should_check_the_box() {
    run(|t| async move {
        t.page
            .set_content("<input id='checkbox' type='checkbox'></input>")
            .await
            .unwrap();
        t.page.locator("input").check().await.unwrap();
        assert!(t.page.evaluate::<bool>("checkbox.checked").await.unwrap());
    });
}

#[test]
fn should_check_the_box_using_set_checked() {
    run(|t| async move {
        t.page
            .set_content("<input id='checkbox' type='checkbox'></input>")
            .await
            .unwrap();
        let input = t.page.locator("input");
        input.set_checked(true).await.unwrap();
        assert!(t.page.evaluate::<bool>("checkbox.checked").await.unwrap());
        input.set_checked(false).await.unwrap();
        assert!(!t.page.evaluate::<bool>("checkbox.checked").await.unwrap());
    });
}

#[test]
fn should_uncheck_the_box() {
    run(|t| async move {
        t.page
            .set_content("<input id='checkbox' type='checkbox' checked></input>")
            .await
            .unwrap();
        t.page.locator("input").uncheck().await.unwrap();
        assert!(!t.page.evaluate::<bool>("checkbox.checked").await.unwrap());
    });
}

#[test]
fn should_select_single_option() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/select.html", t.server.prefix))
            .await
            .unwrap();
        t.page
            .locator("select")
            .select_option("blue")
            .await
            .unwrap();
        let on_input: Vec<String> = t
            .page
            .evaluate("() => window['result'].onInput")
            .await
            .unwrap();
        assert_eq!(on_input, vec!["blue"]);
        let on_change: Vec<String> = t
            .page
            .evaluate("() => window['result'].onChange")
            .await
            .unwrap();
        assert_eq!(on_change, vec!["blue"]);
    });
}

#[test]
fn should_focus_and_blur_a_button() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/button.html", t.server.prefix))
            .await
            .unwrap();
        let button = t.page.locator("button");
        assert!(
            !button
                .evaluate::<bool>("button => document.activeElement === button")
                .await
                .unwrap()
        );
        button
            .evaluate::<Value>(
                "button => { window.focused = false; window.blurred = false; button.addEventListener('focus', () => window.focused = true); button.addEventListener('blur', () => window.blurred = true); }",
            )
            .await
            .unwrap();
        button.focus().await.unwrap();
        assert!(
            t.page
                .evaluate::<bool>("() => window.focused")
                .await
                .unwrap()
        );
        assert!(
            !t.page
                .evaluate::<bool>("() => window.blurred")
                .await
                .unwrap()
        );
        assert!(
            button
                .evaluate::<bool>("button => document.activeElement === button")
                .await
                .unwrap()
        );
        button.blur().await.unwrap();
        assert!(
            t.page
                .evaluate::<bool>("() => window.focused")
                .await
                .unwrap()
        );
        assert!(
            t.page
                .evaluate::<bool>("() => window.blurred")
                .await
                .unwrap()
        );
        assert!(
            !button
                .evaluate::<bool>("button => document.activeElement === button")
                .await
                .unwrap()
        );
    });
}

#[test]
fn focus_should_respect_strictness() {
    run(|t| async move {
        t.page
            .set_content("<div>A</div><div>B</div>")
            .await
            .unwrap();
        let error = t.page.locator("div").focus().await.unwrap_err();
        assert!(error.to_string().contains("strict mode violation"));
    });
}

#[test]
fn should_dispatch_click_event_via_element_handles() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/button.html", t.server.prefix))
            .await
            .unwrap();
        t.page
            .locator("button")
            .dispatch_event("click")
            .await
            .unwrap();
        let result: String = t.page.evaluate("() => window['result']").await.unwrap();
        assert_eq!(result, "Clicked");
    });
}

#[test]
fn should_upload_the_file() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/fileupload.html", t.server.prefix))
            .await
            .unwrap();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("assets")
            .join("file-to-upload.txt");
        let input = t.page.locator("input[type=file]");
        input.set_input_files(path).await.unwrap();
        let handle = input.element_handle().await.unwrap();
        let name: String = t
            .page
            .evaluate("e => e.files[0].name")
            .arg(&handle)
            .await
            .unwrap();
        assert_eq!(name, "file-to-upload.txt");
    });
}
