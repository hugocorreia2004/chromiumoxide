//! Ports of `elementhandle-query-selector.spec.ts` and `queryselector.spec.ts`.
//!
//! Skipped:
//! - EH-069 "should work for adopted elements": needs the `popup` event.
//! - QS-001 "should throw for non-string selector": not expressible, the
//!   Rust API only takes string selectors.
//!
//! EH-067/EH-072 navigate to `empty.html` instead of `playground.html`
//! (the content is replaced right after anyway).

use crate::common::*;

// ---- elementhandle-query-selector.spec.ts ----

#[test]
fn should_query_existing_element() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .set_content("<html><body><div class=\"second\"><div class=\"inner\">A</div></div></body></html>")
            .await
            .unwrap();
        let html = t.page.query_selector("html").await.unwrap().unwrap();
        let second = html.query_selector(".second").await.unwrap().unwrap();
        let inner = second.query_selector(".inner").await.unwrap().unwrap();
        let content: String = t
            .page
            .evaluate("e => e.textContent")
            .arg(&inner)
            .await
            .unwrap();
        assert_eq!(content, "A");
    });
}

#[test]
fn should_return_null_for_non_existing_element() {
    run(|t| async move {
        t.page
            .set_content("<html><body><div class=\"second\"><div class=\"inner\">B</div></div></body></html>")
            .await
            .unwrap();
        let html = t.page.query_selector("html").await.unwrap().unwrap();
        assert!(html.query_selector(".third").await.unwrap().is_none());
    });
}

#[test]
fn element_handle_should_query_existing_elements() {
    run(|t| async move {
        t.page
            .set_content("<html><body><div>A</div><br/><div>B</div></body></html>")
            .await
            .unwrap();
        let html = t.page.query_selector("html").await.unwrap().unwrap();
        let elements = html.query_selector_all("div").await.unwrap();
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
        let html = t.page.query_selector("html").await.unwrap().unwrap();
        assert!(html.query_selector_all("div").await.unwrap().is_empty());
    });
}

#[test]
fn element_handle_xpath_should_query_existing_element() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .set_content("<html><body><div class=\"second\"><div class=\"inner\">A</div></div></body></html>")
            .await
            .unwrap();
        let html = t.page.query_selector("html").await.unwrap().unwrap();
        let second = html
            .query_selector_all("xpath=./body/div[contains(@class, 'second')]")
            .await
            .unwrap();
        let inner = second[0]
            .query_selector_all("xpath=./div[contains(@class, 'inner')]")
            .await
            .unwrap();
        let content: String = t
            .page
            .evaluate("e => e.textContent")
            .arg(&inner[0])
            .await
            .unwrap();
        assert_eq!(content, "A");
    });
}

#[test]
fn element_handle_xpath_should_return_null_for_non_existing_element() {
    run(|t| async move {
        t.page
            .set_content("<html><body><div class=\"second\"><div class=\"inner\">B</div></div></body></html>")
            .await
            .unwrap();
        let html = t.page.query_selector("html").await.unwrap().unwrap();
        let second = html
            .query_selector_all("xpath=/div[contains(@class, 'third')]")
            .await
            .unwrap();
        assert!(second.is_empty());
    });
}

// ---- queryselector.spec.ts ----

#[test]
fn should_query_existing_element_with_css_selector() {
    run(|t| async move {
        t.page.set_content("<section>test</section>").await.unwrap();
        assert!(
            t.page
                .query_selector("css=section")
                .await
                .unwrap()
                .is_some()
        );
    });
}

#[test]
fn should_query_existing_element_with_text_selector() {
    run(|t| async move {
        t.page.set_content("<section>test</section>").await.unwrap();
        assert!(
            t.page
                .query_selector("text=\"test\"")
                .await
                .unwrap()
                .is_some()
        );
    });
}

#[test]
fn should_query_existing_element_with_xpath_selector() {
    run(|t| async move {
        t.page.set_content("<section>test</section>").await.unwrap();
        assert!(
            t.page
                .query_selector("xpath=/html/body/section")
                .await
                .unwrap()
                .is_some()
        );
    });
}

#[test]
fn page_should_return_null_for_non_existing_element() {
    run(|t| async move {
        assert!(
            t.page
                .query_selector("non-existing-element")
                .await
                .unwrap()
                .is_none()
        );
    });
}

#[test]
fn should_auto_detect_xpath_selector() {
    run(|t| async move {
        t.page.set_content("<section>test</section>").await.unwrap();
        assert!(
            t.page
                .query_selector("//html/body/section")
                .await
                .unwrap()
                .is_some()
        );
    });
}

#[test]
fn should_auto_detect_xpath_selector_with_starting_parenthesis() {
    run(|t| async move {
        t.page.set_content("<section>test</section>").await.unwrap();
        assert!(
            t.page
                .query_selector("(//section)[1]")
                .await
                .unwrap()
                .is_some()
        );
    });
}

#[test]
fn should_auto_detect_xpath_selector_starting_with_dot_dot() {
    run(|t| async move {
        t.page
            .set_content("<div><section>test</section><span></span></div>")
            .await
            .unwrap();
        let span = t
            .page
            .query_selector("\"test\" >> ../span")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            span.evaluate::<String>("e => e.nodeName").await.unwrap(),
            "SPAN"
        );
        let div = t
            .page
            .query_selector("\"test\" >> ..")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            div.evaluate::<String>("e => e.nodeName").await.unwrap(),
            "DIV"
        );
    });
}

#[test]
fn should_auto_detect_text_selector() {
    run(|t| async move {
        t.page.set_content("<section>test</section>").await.unwrap();
        assert!(t.page.query_selector("\"test\"").await.unwrap().is_some());
    });
}

#[test]
fn should_auto_detect_css_selector() {
    run(|t| async move {
        t.page.set_content("<section>test</section>").await.unwrap();
        assert!(t.page.query_selector("section").await.unwrap().is_some());
    });
}

#[test]
fn should_support_gt_gt_syntax() {
    run(|t| async move {
        t.page
            .set_content("<section><div>test</div></section>")
            .await
            .unwrap();
        assert!(
            t.page
                .query_selector("css=section >> css=div")
                .await
                .unwrap()
                .is_some()
        );
    });
}

#[test]
fn should_query_existing_elements() {
    run(|t| async move {
        t.page
            .set_content("<div>A</div><br/><div>B</div>")
            .await
            .unwrap();
        let elements = t.page.query_selector_all("div").await.unwrap();
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
fn should_return_empty_array_if_nothing_is_found() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert!(t.page.query_selector_all("div").await.unwrap().is_empty());
    });
}

#[test]
fn xpath_should_query_existing_element() {
    run(|t| async move {
        t.page.set_content("<section>test</section>").await.unwrap();
        let elements = t
            .page
            .query_selector_all("xpath=/html/body/section")
            .await
            .unwrap();
        assert_eq!(elements.len(), 1);
    });
}

#[test]
fn xpath_should_return_empty_array_for_non_existing_element() {
    run(|t| async move {
        assert!(
            t.page
                .query_selector_all("//html/body/non-existing-element")
                .await
                .unwrap()
                .is_empty()
        );
    });
}

#[test]
fn xpath_should_return_multiple_elements() {
    run(|t| async move {
        t.page.set_content("<div></div><div></div>").await.unwrap();
        let elements = t
            .page
            .query_selector_all("xpath=/html/body/div")
            .await
            .unwrap();
        assert_eq!(elements.len(), 2);
    });
}

#[test]
fn query_selector_all_should_work_with_bogus_array_from() {
    run(|t| async move {
        t.page
            .set_content("<div>hello</div><div></div>")
            .await
            .unwrap();
        let div1 = t
            .page
            .evaluate_handle(
                "() => { Array.from = () => []; return document.querySelector('div'); }",
            )
            .await
            .unwrap();
        let elements = t.page.query_selector_all("div").await.unwrap();
        assert_eq!(elements.len(), 2);
        // Check that element handle is functional and belongs to the main world.
        let same: bool = elements[0]
            .evaluate("(div, div1) => div === div1")
            .arg(&div1)
            .await
            .unwrap();
        assert!(same);
    });
}
