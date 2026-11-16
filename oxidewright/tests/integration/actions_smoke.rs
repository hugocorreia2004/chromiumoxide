//! Basic end-to-end checks of locators and actions (one per feature); the
//! detailed ports of Playwright's specs live in the other modules.

use oxidewright::{GetByRoleOptions, LocatorOptions, SelectOption};

use crate::common::*;

#[test]
fn smoke_click_button() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/button.html", t.server.prefix))
            .await
            .unwrap();
        t.page.click("button").await.unwrap();
        let result: String = t.page.evaluate("() => result").await.unwrap();
        assert_eq!(result, "Clicked");
    });
}

#[test]
fn smoke_locator_fill_and_value() {
    run(|t| async move {
        t.page
            .set_content("<input id=a><textarea></textarea>")
            .await
            .unwrap();
        t.page.locator("#a").fill("hello").await.unwrap();
        assert_eq!(t.page.locator("#a").input_value().await.unwrap(), "hello");
        t.page.fill("textarea", "world").await.unwrap();
        assert_eq!(t.page.input_value("textarea").await.unwrap(), "world");
    });
}

#[test]
fn smoke_get_by_role_and_strict() {
    run(|t| async move {
        t.page
            .set_content("<button>One</button><button>Two</button>")
            .await
            .unwrap();
        let two = t
            .page
            .get_by_role_with("button", GetByRoleOptions::new().name("Two"));
        assert_eq!(two.text_content().await.unwrap().as_deref(), Some("Two"));
        assert_eq!(t.page.get_by_role("button").count().await.unwrap(), 2);
        let err = t
            .page
            .get_by_role("button")
            .click()
            .timeout(1000)
            .await
            .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("strict mode violation"), "{message}");
        assert!(message.starts_with("locator.click:"), "{message}");
        let filtered = t
            .page
            .locator("button")
            .filter(LocatorOptions::new().has_text("one"));
        assert_eq!(filtered.inner_text().await.unwrap(), "One");
        assert_eq!(
            t.page.get_by_role("button").to_string(),
            "getByRole('button')"
        );
    });
}

#[test]
fn smoke_timeout_log() {
    run(|t| async move {
        t.page.set_content("<div>x</div>").await.unwrap();
        let err = t
            .page
            .locator("button")
            .click()
            .timeout(500)
            .await
            .unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("locator.click: Timeout 500ms exceeded."),
            "{message}"
        );
        assert!(
            message.contains("waiting for locator('button')"),
            "{message}"
        );
    });
}

#[test]
fn smoke_check_select_files() {
    run(|t| async move {
        t.page
            .set_content(
                "<input type=checkbox id=c><select><option value=a>A</option><option value=b>B</option></select><input type=file>",
            )
            .await
            .unwrap();
        t.page.check("#c").await.unwrap();
        assert!(t.page.is_checked("#c").await.unwrap());
        let selected = t
            .page
            .select_option("select", SelectOption::label("B"))
            .await
            .unwrap();
        assert_eq!(selected, vec!["b".to_string()]);
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/assets/file-to-upload.txt");
        t.page
            .locator("input[type=file]")
            .set_input_files(path)
            .await
            .unwrap();
        let name: String = t
            .page
            .eval_on_selector("input[type=file]", "e => e.files[0].name")
            .await
            .unwrap();
        assert_eq!(name, "file-to-upload.txt");
    });
}

#[test]
fn smoke_frame_locator_and_hover() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .set_content("<iframe srcdoc='<button onclick=\"window.c=1\">in frame</button>'></iframe><div id=h onmouseover=\"window.hovered=1\">hover</div>")
            .await
            .unwrap();
        let button = t.page.frame_locator("iframe").locator("button");
        button.click().await.unwrap();
        assert_eq!(
            button.text_content().await.unwrap().as_deref(),
            Some("in frame")
        );
        t.page.hover("#h").await.unwrap();
        let hovered: i32 = t.page.evaluate("() => window.hovered").await.unwrap();
        assert_eq!(hovered, 1);
        let handle = t.page.query_selector("#h").await.unwrap().unwrap();
        assert_eq!(handle.inner_text().await.unwrap(), "hover");
        let bbox = handle.bounding_box().await.unwrap().unwrap();
        assert!(bbox.width > 0.0);
    });
}
