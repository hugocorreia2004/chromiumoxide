//! Ports of `elementhandle-eval-on-selector.spec.ts`, `eval-on-selector.spec.ts`
//! and `eval-on-selector-all.spec.ts`.
//!
//! All tests ported.

use std::future::IntoFuture;

use serde_json::{Value, json};

use crate::common::*;

// ---- elementhandle-eval-on-selector.spec.ts ----

#[test]
fn element_handle_eval_on_selector_should_work() {
    run(|t| async move {
        t.page
            .set_content("<html><body><div class=\"tweet\"><div class=\"like\">100</div><div class=\"retweets\">10</div></div></body></html>")
            .await
            .unwrap();
        let tweet = t.page.query_selector(".tweet").await.unwrap().unwrap();
        let content: String = tweet
            .eval_on_selector(".like", "node => node.innerText")
            .await
            .unwrap();
        assert_eq!(content, "100");
    });
}

#[test]
fn should_retrieve_content_from_subtree() {
    run(|t| async move {
        t.page
            .set_content("<div class=\"a\">not-a-child-div</div><div id=\"myId\"><div class=\"a\">a-child-div</div></div>")
            .await
            .unwrap();
        let handle = t.page.query_selector("#myId").await.unwrap().unwrap();
        let content: String = handle
            .eval_on_selector(".a", "node => node.innerText")
            .await
            .unwrap();
        assert_eq!(content, "a-child-div");
    });
}

#[test]
fn should_throw_in_case_of_missing_selector() {
    run(|t| async move {
        t.page
            .set_content("<div class=\"a\">not-a-child-div</div><div id=\"myId\"></div>")
            .await
            .unwrap();
        let handle = t.page.query_selector("#myId").await.unwrap().unwrap();
        assert_err_contains(
            handle
                .eval_on_selector::<String>(".a", "node => node.innerText")
                .await,
            "elementHandle.$eval: Failed to find element matching selector \".a\"",
        );
    });
}

#[test]
fn should_work_for_all() {
    run(|t| async move {
        t.page
            .set_content("<html><body><div class=\"tweet\"><div class=\"like\">100</div><div class=\"like\">10</div></div></body></html>")
            .await
            .unwrap();
        let tweet = t.page.query_selector(".tweet").await.unwrap().unwrap();
        let content: Vec<String> = tweet
            .eval_on_selector_all(".like", "nodes => nodes.map(n => n.innerText)")
            .await
            .unwrap();
        assert_eq!(content, vec!["100", "10"]);
    });
}

#[test]
fn should_retrieve_content_from_subtree_for_all() {
    run(|t| async move {
        t.page
            .set_content("<div class=\"a\">not-a-child-div</div><div id=\"myId\"><div class=\"a\">a1-child-div</div><div class=\"a\">a2-child-div</div></div>")
            .await
            .unwrap();
        let handle = t.page.query_selector("#myId").await.unwrap().unwrap();
        let content: Vec<String> = handle
            .eval_on_selector_all(".a", "nodes => nodes.map(n => n.innerText)")
            .await
            .unwrap();
        assert_eq!(content, vec!["a1-child-div", "a2-child-div"]);
    });
}

#[test]
fn should_not_throw_in_case_of_missing_selector_for_all() {
    run(|t| async move {
        t.page
            .set_content("<div class=\"a\">not-a-child-div</div><div id=\"myId\"></div>")
            .await
            .unwrap();
        let handle = t.page.query_selector("#myId").await.unwrap().unwrap();
        let length: usize = handle
            .eval_on_selector_all(".a", "nodes => nodes.length")
            .await
            .unwrap();
        assert_eq!(length, 0);
    });
}

// ---- eval-on-selector.spec.ts ----

async fn id_of(page: &oxidewright::Page, selector: &str) -> String {
    page.eval_on_selector(selector, "e => e.id").await.unwrap()
}

#[test]
fn should_work_with_css_selector() {
    run(|t| async move {
        t.page
            .set_content("<section id=\"testAttribute\">43543</section>")
            .await
            .unwrap();
        assert_eq!(id_of(&t.page, "css=section").await, "testAttribute");
    });
}

#[test]
fn should_work_with_id_selector() {
    run(|t| async move {
        t.page
            .set_content("<section id=\"testAttribute\">43543</section>")
            .await
            .unwrap();
        assert_eq!(id_of(&t.page, "id=testAttribute").await, "testAttribute");
    });
}

#[test]
fn should_work_with_data_test_selector() {
    run(|t| async move {
        t.page
            .set_content("<section data-test=foo id=\"testAttribute\">43543</section>")
            .await
            .unwrap();
        assert_eq!(id_of(&t.page, "data-test=foo").await, "testAttribute");
    });
}

#[test]
fn should_work_with_data_testid_selector() {
    run(|t| async move {
        t.page
            .set_content("<section data-testid=foo id=\"testAttribute\">43543</section>")
            .await
            .unwrap();
        assert_eq!(id_of(&t.page, "data-testid=foo").await, "testAttribute");
    });
}

#[test]
fn should_work_with_data_test_id_selector() {
    run(|t| async move {
        t.page
            .set_content("<section data-test-id=foo id=\"testAttribute\">43543</section>")
            .await
            .unwrap();
        assert_eq!(id_of(&t.page, "data-test-id=foo").await, "testAttribute");
    });
}

#[test]
fn should_work_with_text_selector_in_quotes() {
    run(|t| async move {
        t.page
            .set_content("<section id=\"testAttribute\">43543</section>")
            .await
            .unwrap();
        assert_eq!(id_of(&t.page, "text=\"43543\"").await, "testAttribute");
    });
}

#[test]
fn should_work_with_xpath_selector() {
    run(|t| async move {
        t.page
            .set_content("<section id=\"testAttribute\">43543</section>")
            .await
            .unwrap();
        assert_eq!(
            id_of(&t.page, "xpath=/html/body/section").await,
            "testAttribute"
        );
    });
}

#[test]
fn should_work_with_text_selector() {
    run(|t| async move {
        t.page
            .set_content("<section id=\"testAttribute\">43543</section>")
            .await
            .unwrap();
        assert_eq!(id_of(&t.page, "text=43543").await, "testAttribute");
    });
}

#[test]
fn should_auto_detect_css_selector() {
    run(|t| async move {
        t.page
            .set_content("<section id=\"testAttribute\">43543</section>")
            .await
            .unwrap();
        assert_eq!(id_of(&t.page, "section").await, "testAttribute");
    });
}

#[test]
fn should_auto_detect_css_selector_with_attributes() {
    run(|t| async move {
        t.page
            .set_content("<section id=\"testAttribute\">43543</section>")
            .await
            .unwrap();
        assert_eq!(
            id_of(&t.page, "section[id=\"testAttribute\"]").await,
            "testAttribute"
        );
    });
}

#[test]
fn should_auto_detect_nested_selectors() {
    run(|t| async move {
        t.page
            .set_content("<div foo=bar><section>43543<span>Hello<div id=target></div></span></section></div>")
            .await
            .unwrap();
        assert_eq!(
            id_of(&t.page, "div[foo=bar] > section >> \"Hello\" >> div").await,
            "target"
        );
    });
}

#[test]
fn should_accept_arguments() {
    run(|t| async move {
        t.page
            .set_content("<section>hello</section>")
            .await
            .unwrap();
        let text: String = t
            .page
            .eval_on_selector("section", "(e, suffix) => e.textContent + suffix")
            .arg(" world!")
            .await
            .unwrap();
        assert_eq!(text, "hello world!");
    });
}

#[test]
fn should_accept_element_handles_as_arguments() {
    run(|t| async move {
        t.page
            .set_content("<section>hello</section><div> world</div>")
            .await
            .unwrap();
        let div = t.page.query_selector("div").await.unwrap().unwrap();
        let text: String = t
            .page
            .eval_on_selector("section", "(e, div) => e.textContent + div.textContent")
            .arg(&div)
            .await
            .unwrap();
        assert_eq!(text, "hello world");
    });
}

#[test]
fn should_throw_error_if_no_element_is_found() {
    run(|t| async move {
        assert_err_contains(
            t.page
                .eval_on_selector::<String>("section", "e => e.id")
                .await,
            "Failed to find element matching selector \"section\"",
        );
    });
}

#[test]
fn should_support_gt_gt_syntax() {
    run(|t| async move {
        t.page
            .set_content("<section><div>hello</div></section>")
            .await
            .unwrap();
        let text: String = t
            .page
            .eval_on_selector(
                "css=section >> css=div",
                "(e, suffix) => e.textContent + suffix",
            )
            .arg(" world!")
            .await
            .unwrap();
        assert_eq!(text, "hello world!");
    });
}

#[test]
fn should_support_gt_gt_syntax_with_different_engines() {
    run(|t| async move {
        t.page
            .set_content("<section><div><span>hello</span></div></section>")
            .await
            .unwrap();
        let text: String = t
            .page
            .eval_on_selector(
                "xpath=/html/body/section >> css=div >> text=\"hello\"",
                "(e, suffix) => e.textContent + suffix",
            )
            .arg(" world!")
            .await
            .unwrap();
        assert_eq!(text, "hello world!");
    });
}

#[test]
fn should_support_spaces_with_gt_gt_syntax() {
    run(|t| async move {
        t.page
            .goto(format!("{}/deep-shadow.html", t.server.prefix))
            .await
            .unwrap();
        let text: String = t
            .page
            .eval_on_selector(" css = div >>css=div>>css   = span  ", "e => e.textContent")
            .await
            .unwrap();
        assert_eq!(text, "Hello from root2");
    });
}

#[test]
fn should_not_stop_at_first_failure_with_gt_gt_syntax() {
    run(|t| async move {
        t.page
            .set_content(
                "<div><span>Next</span><button>Previous</button><button>Next</button></div>",
            )
            .await
            .unwrap();
        let html: String = t
            .page
            .eval_on_selector("button >> \"Next\"", "e => e.outerHTML")
            .await
            .unwrap();
        assert_eq!(html, "<button>Next</button>");
    });
}

#[test]
fn should_support_capture() {
    run(|t| async move {
        t.page
            .set_content("<section><div><span>a</span></div></section><section><div><span>b</span></div></section>")
            .await
            .unwrap();
        let outer = |s: &'static str| {
            let page = t.page.clone();
            async move {
                page.eval_on_selector::<String>(s, "e => e.outerHTML")
                    .await
                    .unwrap()
            }
        };
        assert_eq!(
            outer("*css=div >> \"b\"").await,
            "<div><span>b</span></div>"
        );
        assert_eq!(
            outer("section >> *css=div >> \"b\"").await,
            "<div><span>b</span></div>"
        );
        assert_eq!(outer("css=div >> *text=\"b\"").await, "<span>b</span>");
        assert!(t.page.query_selector("*").await.unwrap().is_some());
    });
}

#[test]
fn should_throw_on_multiple_captures() {
    run(|t| async move {
        assert_err_contains(
            t.page
                .eval_on_selector::<String>("*css=div >> *css=span", "e => e.outerHTML")
                .await,
            "Only one of the selectors can capture using * modifier",
        );
    });
}

#[test]
fn should_throw_on_malformed_capture() {
    run(|t| async move {
        assert_err_contains(
            t.page
                .eval_on_selector::<String>("*=div", "e => e.outerHTML")
                .await,
            "Unknown engine \"\" while parsing selector *=div",
        );
    });
}

#[test]
fn should_work_with_spaces_in_css_attributes() {
    run(|t| async move {
        t.page
            .set_content("<div><input placeholder=\"Select date\"></div>")
            .await
            .unwrap();
        for s in [
            "[placeholder=\"Select date\"]",
            "[placeholder='Select date']",
            "input[placeholder=\"Select date\"]",
            "input[placeholder='Select date']",
        ] {
            assert!(t.page.wait_for_selector(s).await.unwrap().is_some());
            assert!(t.page.query_selector(s).await.unwrap().is_some());
        }
        for s in [
            "[placeholder=\"Select date\"]",
            "[placeholder='Select date']",
            "input[placeholder=\"Select date\"]",
            "input[placeholder='Select date']",
            "css=[placeholder=\"Select date\"]",
            "css=[placeholder='Select date']",
            "css=input[placeholder=\"Select date\"]",
            "css=input[placeholder='Select date']",
            "div >> [placeholder=\"Select date\"]",
            "div >> [placeholder='Select date']",
        ] {
            let html: String = t
                .page
                .eval_on_selector(s, "e => e.outerHTML")
                .await
                .unwrap();
            assert_eq!(html, "<input placeholder=\"Select date\">", "{s}");
        }
    });
}

#[test]
fn should_work_with_quotes_in_css_attributes() {
    run(|t| async move {
        let cases: [(&str, [&str; 2]); 4] = [
            (
                "<div><input placeholder=\"Select&quot;date\"></div>",
                [
                    "[placeholder=\"Select\\\"date\"]",
                    "[placeholder='Select\"date']",
                ],
            ),
            (
                "<div><input placeholder=\"Select &quot; date\"></div>",
                [
                    "[placeholder=\"Select \\\" date\"]",
                    "[placeholder='Select \" date']",
                ],
            ),
            (
                "<div><input placeholder=\"Select&apos;date\"></div>",
                [
                    "[placeholder=\"Select'date\"]",
                    "[placeholder='Select\\'date']",
                ],
            ),
            (
                "<div><input placeholder=\"Select &apos; date\"></div>",
                [
                    "[placeholder=\"Select ' date\"]",
                    "[placeholder='Select \\' date']",
                ],
            ),
        ];
        for (content, selectors) in cases {
            t.page.set_content(content).await.unwrap();
            for s in selectors {
                assert!(t.page.query_selector(s).await.unwrap().is_some(), "{s}");
            }
        }
    });
}

#[test]
fn should_work_with_spaces_in_css_attributes_when_missing() {
    run(|t| async move {
        let input = tokio::spawn(
            t.page
                .wait_for_selector("[placeholder=\"Select date\"]")
                .into_future(),
        );
        assert!(
            t.page
                .query_selector("[placeholder=\"Select date\"]")
                .await
                .unwrap()
                .is_none()
        );
        t.page
            .set_content("<div><input placeholder=\"Select date\"></div>")
            .await
            .unwrap();
        input.await.unwrap().unwrap();
    });
}

#[test]
fn should_work_with_quotes_in_css_attributes_when_missing() {
    run(|t| async move {
        let input = tokio::spawn(
            t.page
                .wait_for_selector("[placeholder=\"Select\\\"date\"]")
                .into_future(),
        );
        assert!(
            t.page
                .query_selector("[placeholder=\"Select\\\"date\"]")
                .await
                .unwrap()
                .is_none()
        );
        t.page
            .set_content("<div><input placeholder=\"Select&quot;date\"></div>")
            .await
            .unwrap();
        input.await.unwrap().unwrap();
    });
}

#[test]
fn should_return_complex_values() {
    run(|t| async move {
        t.page
            .set_content("<section id=\"testAttribute\">43543</section>")
            .await
            .unwrap();
        let value: Value = t
            .page
            .eval_on_selector("css=section", "e => [{ id: e.id }]")
            .await
            .unwrap();
        assert_eq!(value, json!([{ "id": "testAttribute" }]));
    });
}

// ---- eval-on-selector-all.spec.ts ----

async fn count(page: &oxidewright::Page, selector: &str) -> usize {
    page.eval_on_selector_all(selector, "els => els.length")
        .await
        .unwrap()
}

#[test]
fn eval_on_selector_all_should_work_with_css_selector() {
    run(|t| async move {
        t.page
            .set_content("<div>hello</div><div>beautiful</div><div>world!</div>")
            .await
            .unwrap();
        assert_eq!(count(&t.page, "css=div").await, 3);
    });
}

#[test]
fn eval_on_selector_all_should_work_with_text_selector() {
    run(|t| async move {
        t.page
            .set_content(
                "<div>hello</div><div>beautiful</div><div>beautiful</div><div>world!</div>",
            )
            .await
            .unwrap();
        assert_eq!(count(&t.page, "text=\"beautiful\"").await, 2);
    });
}

#[test]
fn eval_on_selector_all_should_work_with_xpath_selector() {
    run(|t| async move {
        t.page
            .set_content("<div>hello</div><div>beautiful</div><div>world!</div>")
            .await
            .unwrap();
        assert_eq!(count(&t.page, "xpath=/html/body/div").await, 3);
    });
}

#[test]
fn eval_on_selector_all_should_auto_detect_css_selector() {
    run(|t| async move {
        t.page
            .set_content("<div>hello</div><div>beautiful</div><div>world!</div>")
            .await
            .unwrap();
        assert_eq!(count(&t.page, "div").await, 3);
    });
}

#[test]
fn eval_on_selector_all_should_support_gt_gt_syntax() {
    run(|t| async move {
        t.page
            .set_content("<div><span>hello</span></div><div>beautiful</div><div><span>wo</span><span>rld!</span></div><span>Not this one</span>")
            .await
            .unwrap();
        assert_eq!(count(&t.page, "css=div >> css=span").await, 3);
    });
}

#[test]
fn eval_on_selector_all_should_support_capture() {
    run(|t| async move {
        t.page
            .set_content("<section><div><span>a</span></div></section><section><div><span>b</span></div></section>")
            .await
            .unwrap();
        assert_eq!(count(&t.page, "*css=div >> \"b\"").await, 1);
        assert_eq!(count(&t.page, "section >> *css=div >> \"b\"").await, 1);
        assert_eq!(count(&t.page, "section >> *").await, 4);
        t.page
            .set_content("<section><div><span>a</span><span>a</span></div></section>")
            .await
            .unwrap();
        assert_eq!(count(&t.page, "*css=div >> \"a\"").await, 1);
        assert_eq!(count(&t.page, "section >> *css=div >> \"a\"").await, 1);
        t.page
            .set_content("<div><span>a</span></div><div><span>a</span></div><section><div><span>a</span></div></section>")
            .await
            .unwrap();
        assert_eq!(count(&t.page, "*css=div >> \"a\"").await, 3);
        assert_eq!(count(&t.page, "section >> *css=div >> \"a\"").await, 1);
    });
}

#[test]
fn should_support_capture_when_multiple_paths_match() {
    run(|t| async move {
        t.page
            .set_content("<div><div><span></span></div></div><div></div>")
            .await
            .unwrap();
        assert_eq!(count(&t.page, "*css=div >> span").await, 2);
        t.page
            .set_content("<div><div><span></span></div><span></span><span></span></div><div></div>")
            .await
            .unwrap();
        assert_eq!(count(&t.page, "*css=div >> span").await, 2);
    });
}

#[test]
fn eval_on_selector_all_should_return_complex_values() {
    run(|t| async move {
        t.page
            .set_content("<div>hello</div><div>beautiful</div><div>world!</div>")
            .await
            .unwrap();
        let texts: Vec<String> = t
            .page
            .eval_on_selector_all("css=div", "divs => divs.map(div => div.textContent)")
            .await
            .unwrap();
        assert_eq!(texts, vec!["hello", "beautiful", "world!"]);
    });
}

#[test]
fn should_work_with_bogus_array_from() {
    run(|t| async move {
        t.page
            .set_content("<div>hello</div><div>beautiful</div><div>world!</div>")
            .await
            .unwrap();
        t.page
            .evaluate::<()>("() => { Array.from = () => []; }")
            .await
            .unwrap();
        assert_eq!(count(&t.page, "css=div").await, 3);
    });
}
