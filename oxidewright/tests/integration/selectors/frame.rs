//! Port of `selectors-frame.spec.ts`. `page.route` is replaced by routes of
//! the test server. All tests are ported.

use std::future::IntoFuture;
use std::time::Duration;

use super::*;
use crate::common::*;

const HTML: &str = "text/html";

fn route_iframe(server: &TestServer) {
    server.set_content(
        "/empty.html",
        HTML,
        r#"<iframe src="iframe.html"></iframe>"#,
    );
    server.set_content(
        "/iframe.html",
        HTML,
        r#"
        <html>
          <div>
            <button>Hello iframe</button>
            <iframe src="iframe-2.html"></iframe>
          </div>
          <span>1</span>
          <span>2</span>
        </html>"#,
    );
    server.set_content(
        "/iframe-2.html",
        HTML,
        r#"<html><button tag="iframe2">Hello nested iframe</button></html>"#,
    );
}

const IN_FRAME: &str = "iframe >> internal:control=enter-frame >> ";

#[test]
fn should_work_for_iframe() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let button = t.page.locator(format!("{IN_FRAME}button"));
        button.wait_for().await.unwrap();
        assert_eq!(button.inner_text().await.unwrap(), "Hello iframe");
        expect_text(&button, "Hello iframe").await;
        button.click().await.unwrap();
    });
}

#[test]
fn should_work_for_iframe_handle() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let body = t.page.query_selector("body").await.unwrap().unwrap();
        let button = body
            .wait_for_selector(format!("{IN_FRAME}button"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(button.inner_text().await.unwrap(), "Hello iframe");
        assert_eq!(
            button.text_content().await.unwrap().as_deref(),
            Some("Hello iframe")
        );
        button.click().await.unwrap();
    });
}

#[test]
fn should_work_for_nested_iframe() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let button = t.page.locator(format!("{IN_FRAME}{IN_FRAME}button"));
        button.wait_for().await.unwrap();
        assert_eq!(button.inner_text().await.unwrap(), "Hello nested iframe");
        expect_text(&button, "Hello nested iframe").await;
        button.click().await.unwrap();
    });
}

#[test]
fn should_work_for_nested_iframe_handle() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let body = t.page.query_selector("body").await.unwrap().unwrap();
        let button = body
            .wait_for_selector(format!("{IN_FRAME}{IN_FRAME}button"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(button.inner_text().await.unwrap(), "Hello nested iframe");
        assert_eq!(
            button.text_content().await.unwrap().as_deref(),
            Some("Hello nested iframe")
        );
        button.click().await.unwrap();
    });
}

/// Wait until the routed frames are loaded (`page.$` does not wait).
async fn wait_frames(t: &TestContext) {
    t.page
        .locator(format!("{IN_FRAME}span"))
        .first()
        .wait_for()
        .await
        .unwrap();
}

#[test]
fn should_work_for_dollar_and_dollar_dollar() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        wait_frames(&t).await;
        let element = t
            .page
            .query_selector(&format!("{IN_FRAME}button"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            element.text_content().await.unwrap().as_deref(),
            Some("Hello iframe")
        );
        let elements = t
            .page
            .query_selector_all(&format!("{IN_FRAME}span"))
            .await
            .unwrap();
        assert_eq!(elements.len(), 2);
    });
}

#[test]
fn dollar_should_not_wait_for_frame() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let selector = format!("{IN_FRAME}canvas");
        assert!(t.page.query_selector(&selector).await.unwrap().is_none());
        let body = t.page.query_selector("body").await.unwrap().unwrap();
        assert!(body.query_selector(&selector).await.unwrap().is_none());
    });
}

#[test]
fn dollar_dollar_should_not_wait_for_frame() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let selector = format!("{IN_FRAME}canvas");
        assert!(
            t.page
                .query_selector_all(&selector)
                .await
                .unwrap()
                .is_empty()
        );
        let body = t.page.query_selector("body").await.unwrap().unwrap();
        assert!(body.query_selector_all(&selector).await.unwrap().is_empty());
    });
}

#[test]
fn dollar_eval_should_throw_for_missing_frame() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let selector = format!("{IN_FRAME}canvas");
        let err = t
            .page
            .eval_on_selector::<i32>(selector.clone(), "e => 1")
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("page.$eval: Failed to find element matching selector"),
            "{err}"
        );
        let body = t.page.query_selector("body").await.unwrap().unwrap();
        let err = body
            .eval_on_selector::<i32>(selector, "e => 1")
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("elementHandle.$eval: Failed to find element matching selector"),
            "{err}"
        );
    });
}

#[test]
fn dollar_dollar_eval_should_not_throw_for_missing_frame() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let selector = format!("{IN_FRAME}canvas");
        let n: usize = t
            .page
            .eval_on_selector_all(selector.clone(), "e => e.length")
            .await
            .unwrap();
        assert_eq!(n, 0);
        let body = t.page.query_selector("body").await.unwrap().unwrap();
        let n: usize = body
            .eval_on_selector_all(selector, "e => e.length")
            .await
            .unwrap();
        assert_eq!(n, 0);
    });
}

#[test]
fn should_work_for_dollar_and_dollar_dollar_handle() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        wait_frames(&t).await;
        let body = t.page.query_selector("body").await.unwrap().unwrap();
        let element = body
            .query_selector(&format!("{IN_FRAME}button"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            element.text_content().await.unwrap().as_deref(),
            Some("Hello iframe")
        );
        let elements = body
            .query_selector_all(&format!("{IN_FRAME}span"))
            .await
            .unwrap();
        assert_eq!(elements.len(), 2);
    });
}

#[test]
fn should_work_for_dollar_eval() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        wait_frames(&t).await;
        let value: String = t
            .page
            .eval_on_selector(format!("{IN_FRAME}button"), "b => b.nodeName")
            .await
            .unwrap();
        assert_eq!(value, "BUTTON");
    });
}

#[test]
fn should_work_for_dollar_eval_handle() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        wait_frames(&t).await;
        let body = t.page.query_selector("body").await.unwrap().unwrap();
        let value: String = body
            .eval_on_selector(format!("{IN_FRAME}button"), "b => b.nodeName")
            .await
            .unwrap();
        assert_eq!(value, "BUTTON");
    });
}

#[test]
fn should_work_for_dollar_dollar_eval() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        wait_frames(&t).await;
        let value: Vec<String> = t
            .page
            .eval_on_selector_all(
                format!("{IN_FRAME}span"),
                "ss => ss.map(s => s.textContent)",
            )
            .await
            .unwrap();
        assert_eq!(value, vec!["1", "2"]);
    });
}

#[test]
fn should_work_for_dollar_dollar_eval_handle() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        wait_frames(&t).await;
        let body = t.page.query_selector("body").await.unwrap().unwrap();
        let value: Vec<String> = body
            .eval_on_selector_all(
                format!("{IN_FRAME}span"),
                "ss => ss.map(s => s.textContent)",
            )
            .await
            .unwrap();
        assert_eq!(value, vec!["1", "2"]);
    });
}

#[test]
fn should_not_allow_dangling_enter_frame() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let err = t
            .page
            .locator("iframe >> internal:control=enter-frame")
            .click()
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Selector cannot end with"), "{err}");
        assert!(
            err.contains("iframe >> internal:control=enter-frame"),
            "{err}"
        );
    });
}

#[test]
fn should_not_allow_leading_enter_frame() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let err = t
            .page
            .wait_for_selector("internal:control=enter-frame >> button")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Selector cannot start with"), "{err}");
    });
}

#[test]
fn should_not_allow_capturing_before_enter_frame() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let err = t
            .page
            .locator("*css=iframe >> internal:control=enter-frame >> div")
            .click()
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Can not capture the selector before diving into the frame"),
            "{err}"
        );
    });
}

#[test]
fn should_capture_after_the_enter_frame() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let div = t.page.locator(format!("{IN_FRAME}*css=div >> button"));
        assert!(div.inner_html().await.unwrap().contains("<button>"));
    });
}

#[test]
fn should_click_in_lazy_iframe() {
    run(|t| async move {
        t.server.set_content(
            "/iframe.html",
            HTML,
            "<html><button>Hello iframe</button></html>",
        );
        t.page.goto(&t.server.empty_page).await.unwrap();
        let page = t.page.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let _ = page
                .evaluate::<()>(
                    "() => { document.body.appendChild(document.createElement('iframe')); }",
                )
                .await;
            tokio::time::sleep(Duration::from_millis(500)).await;
            let _ = page
                .evaluate::<()>("() => { document.querySelector('iframe').src = 'iframe.html'; }")
                .await;
        });
        let button = t.page.locator(format!("{IN_FRAME}button"));
        let (click, text, ()) = tokio::join!(
            button.click().into_future(),
            button.inner_text().into_future(),
            expect_text(&button, "Hello iframe"),
        );
        click.unwrap();
        assert_eq!(text.unwrap(), "Hello iframe");
    });
}

const NESTED_BUTTON: &str =
    r#"iframe >> internal:control=enter-frame >> button:has-text("Hello nested iframe")"#;

async fn reattach(page: &oxidewright::Page) {
    page.locator("iframe")
        .evaluate::<()>("e => e.remove()")
        .await
        .unwrap();
    page.evaluate::<()>(
        "() => { const iframe = document.createElement('iframe'); iframe.src = 'iframe-2.html'; document.body.appendChild(iframe); }",
    )
    .await
    .unwrap();
}

#[test]
fn wait_for_should_survive_frame_reattach() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let button = t.page.locator(NESTED_BUTTON);
        let (result, ()) = tokio::join!(button.wait_for().into_future(), reattach(&t.page));
        result.unwrap();
    });
}

#[test]
fn wait_for_selector_should_survive_frame_reattach_handle() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let body = t.page.query_selector("body").await.unwrap().unwrap();
        let (result, ()) = tokio::join!(
            body.wait_for_selector(NESTED_BUTTON).into_future(),
            reattach(&t.page)
        );
        result.unwrap().unwrap();
    });
}

#[test]
fn wait_for_selector_should_survive_iframe_navigation_handle() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let body = t.page.query_selector("body").await.unwrap().unwrap();
        let navigate = async {
            let _ = t
                .page
                .locator("iframe")
                .evaluate::<()>("e => { e.src = 'iframe-2.html'; }")
                .await;
        };
        let (result, ()) = tokio::join!(
            body.wait_for_selector(NESTED_BUTTON).into_future(),
            navigate
        );
        result.unwrap().unwrap();
    });
}

#[test]
fn click_should_survive_frame_reattach() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let button = t.page.locator(NESTED_BUTTON);
        let (result, ()) = tokio::join!(button.click().into_future(), reattach(&t.page));
        result.unwrap();
    });
}

#[test]
fn click_should_survive_iframe_navigation() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let button = t.page.locator(NESTED_BUTTON);
        let navigate = async {
            let _ = t
                .page
                .locator("iframe")
                .evaluate::<()>("e => { e.src = 'iframe-2.html'; }")
                .await;
        };
        let (result, ()) = tokio::join!(button.click().into_future(), navigate);
        result.unwrap();
    });
}

#[test]
fn click_should_survive_navigation() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page
            .goto(format!("{}/iframe.html", t.server.prefix))
            .await
            .unwrap();
        let navigate = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            t.page
                .goto(format!("{}/iframe-2.html", t.server.prefix))
                .await
                .unwrap();
        };
        let (result, ()) = tokio::join!(
            t.page
                .click(r#"button:has-text("Hello nested iframe")"#)
                .into_future(),
            navigate
        );
        result.unwrap();
    });
}

#[test]
fn should_non_work_for_non_frame() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.set_content("<div></div>").await.unwrap();
        let err = t
            .page
            .locator("div >> internal:control=enter-frame >> button")
            .wait_for()
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("<div></div>"), "{err}");
        assert!(err.contains("<iframe> was expected"), "{err}");
    });
}

#[test]
fn should_match_in_a_descendant_frame() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let div = t.page.locator("internal:control=any-frame >> div");
        div.wait_for().await.unwrap();
        expect_count(&div, 1).await;
        assert!(
            div.inner_html()
                .await
                .unwrap()
                .contains("<button>Hello iframe</button>")
        );
    });
}

#[test]
fn should_match_in_a_deeply_nested_frame() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let button = t
            .page
            .locator(r#"internal:control=any-frame >> button[tag="iframe2"]"#);
        button.wait_for().await.unwrap();
        expect_count(&button, 1).await;
        assert_eq!(
            button.text_content().await.unwrap().as_deref(),
            Some("Hello nested iframe")
        );
    });
}

#[test]
fn should_not_match_a_chain_across_frames() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .locator(r#"internal:control=any-frame >> button[tag="iframe2"]"#)
            .wait_for()
            .await
            .unwrap();
        expect_count(
            &t.page
                .locator(r#"internal:control=any-frame >> div >> button[tag="iframe2"]"#),
            0,
        )
        .await;
        expect_text(
            &t.page
                .locator("internal:control=any-frame >> div >> button"),
            "Hello iframe",
        )
        .await;
    });
}

fn route_two_frames(server: &TestServer, b: &'static str) {
    server.set_content(
        "/empty.html",
        HTML,
        r#"<iframe src="a.html"></iframe><iframe src="b.html"></iframe>"#,
    );
    server.set_content("/a.html", HTML, "<div>one</div>");
    server.set_content("/b.html", HTML, b);
}

#[test]
fn should_match_multiple_elements() {
    run(|t| async move {
        route_two_frames(&t.server, "<span>two</span><span>three</span>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        let texts: Vec<String> = t
            .page
            .eval_on_selector_all(
                "internal:control=any-frame >> span",
                "els => els.map(e => e.textContent)",
            )
            .await
            .unwrap();
        assert_eq!(texts, vec!["two", "three"]);
    });
}

#[test]
fn should_throw_when_matching_elements_in_multiple_frames() {
    run(|t| async move {
        route_two_frames(&t.server, "<div>two</div>");
        t.page.goto(&t.server.empty_page).await.unwrap();
        let page = t.page.clone();
        poll_eq(3, move || {
            let page = page.clone();
            async move { page.frames().len() }
        })
        .await;
        for frame in t.page.frames() {
            if frame != t.page.main_frame() {
                frame.wait_for_selector("div").await.unwrap();
            }
        }
        let err = t
            .page
            .locator("internal:control=any-frame >> div")
            .inner_html()
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("frameLocator() matched elements in multiple frames"),
            "{err}"
        );
    });
}

#[test]
fn should_not_allow_any_frame_in_the_middle_of_a_selector() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let err = t
            .page
            .locator("iframe >> internal:control=any-frame >> div")
            .wait_for()
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(r#""any-frame" is only allowed as the first selector token"#),
            "{err}"
        );
    });
}

#[test]
fn should_allow_entering_frames_from_any_frame() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let button = t.page.locator(
            r#"internal:control=any-frame >> iframe[src="iframe-2.html"] >> internal:control=enter-frame >> button"#,
        );
        button.wait_for().await.unwrap();
        assert_eq!(button.inner_text().await.unwrap(), "Hello nested iframe");
    });
}

#[test]
fn should_not_allow_any_frame_after_entering_a_frame() {
    run(|t| async move {
        let err = t
            .page
            .locator(
                "iframe >> internal:control=enter-frame >> internal:control=any-frame >> button",
            )
            .count()
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(r#""any-frame" is only allowed as the first selector token"#),
            "{err}"
        );
    });
}

#[test]
fn should_not_allow_dangling_enter_frame_after_any_frame() {
    run(|t| async move {
        let err = t
            .page
            .locator("internal:control=any-frame >> iframe >> internal:control=enter-frame")
            .count()
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Selector cannot end with entering frame"),
            "{err}"
        );
    });
}
