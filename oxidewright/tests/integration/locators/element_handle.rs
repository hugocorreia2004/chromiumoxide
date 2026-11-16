//! Port of `locator-element-handle.spec.ts`. All tests ported
//! (`/playground.html` is not needed: the content is replaced right away).

use crate::common::*;

#[test]
fn should_query_existing_element() {
    run(|t| async move {
        t.page
            .set_content(
                "<html><body><div class=\"second\"><div class=\"inner\">A</div></div></body></html>",
            )
            .await
            .unwrap();
        let html = t.page.locator("html");
        let second = html.locator(".second");
        let inner = second.locator(".inner");
        let handle = inner.element_handle().await.unwrap();
        let content: String = t
            .page
            .evaluate("e => e.textContent")
            .arg(&handle)
            .await
            .unwrap();
        assert_eq!(content, "A");
    });
}

#[test]
fn should_query_existing_elements() {
    run(|t| async move {
        t.page
            .set_content("<html><body><div>A</div><br/><div>B</div></body></html>")
            .await
            .unwrap();
        let html = t.page.locator("html");
        let elements = html.locator("div").element_handles().await.unwrap();
        assert_eq!(elements.len(), 2);
        let mut texts = Vec::new();
        for element in &elements {
            let text: String = t
                .page
                .evaluate("e => e.textContent")
                .arg(element)
                .await
                .unwrap();
            texts.push(text);
        }
        assert_eq!(texts, vec!["A", "B"]);
    });
}

#[test]
fn should_return_empty_array_for_non_existing_elements() {
    run(|t| async move {
        t.page
            .set_content("<html><body><span>A</span><br/><span>B</span></body></html>")
            .await
            .unwrap();
        let html = t.page.locator("html");
        let elements = html.locator("div").element_handles().await.unwrap();
        assert!(elements.is_empty());
    });
}

#[test]
fn xpath_should_query_existing_element() {
    run(|t| async move {
        t.page
            .set_content(
                "<html><body><div class=\"second\"><div class=\"inner\">A</div></div></body></html>",
            )
            .await
            .unwrap();
        let html = t.page.locator("html");
        let second = html.locator("xpath=./body/div[contains(@class, 'second')]");
        let inner = second.locator("xpath=./div[contains(@class, 'inner')]");
        let handle = inner.element_handle().await.unwrap();
        let content: String = t
            .page
            .evaluate("e => e.textContent")
            .arg(&handle)
            .await
            .unwrap();
        assert_eq!(content, "A");
    });
}

#[test]
fn xpath_should_return_null_for_non_existing_element() {
    run(|t| async move {
        t.page
            .set_content(
                "<html><body><div class=\"second\"><div class=\"inner\">B</div></div></body></html>",
            )
            .await
            .unwrap();
        let html = t.page.locator("html");
        let second = html
            .locator("xpath=/div[contains(@class, 'third')]")
            .element_handles()
            .await
            .unwrap();
        assert!(second.is_empty());
    });
}
