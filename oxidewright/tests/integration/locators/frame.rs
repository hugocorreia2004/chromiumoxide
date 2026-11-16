//! Port of `locator-frame.spec.ts`. `page.route(...)` fulfillments are
//! replaced by routes of the test server.
//!
//! Skipped:
//! - LO-108 `should work with COEP/COOP/CORP isolated iframe`: needs an
//!   https cross-origin server.

use std::time::Duration;

use oxidewright::WaitForSelectorState;
use serde_json::Value;

use super::*;
use crate::common::*;

fn route_iframe(server: &TestServer) {
    server.set_content(
        "/empty.html",
        "text/html",
        "<iframe src=\"iframe.html\" name=\"frame1\"></iframe>",
    );
    server.set_content(
        "/iframe.html",
        "text/html",
        r#"
        <html>
          <div>
            <button data-testid="buttonId">Hello iframe</button>
            <iframe src="iframe-2.html"></iframe>
          </div>
          <span>1</span>
          <span>2</span>
          <label for=target>Name</label><input id=target type=text placeholder=Placeholder title=Title alt=Alternative>
        </html>"#,
    );
    server.set_content(
        "/iframe-2.html",
        "text/html",
        "<html><button>Hello nested iframe</button></html>",
    );
}

fn route_ambiguous(server: &TestServer) {
    server.set_content(
        "/empty.html",
        "text/html",
        r#"<iframe src="iframe-1.html"></iframe>
             <iframe src="iframe-2.html"></iframe>
             <iframe src="iframe-3.html"></iframe>"#,
    );
    server.set_content(
        "/iframe-1.html",
        "text/html",
        "<html><button>Hello from iframe-1.html</button></html>",
    );
    server.set_content(
        "/iframe-2.html",
        "text/html",
        "<html><button>Hello from iframe-2.html</button></html>",
    );
    server.set_content(
        "/iframe-3.html",
        "text/html",
        "<html><button>Hello from iframe-3.html</button></html>",
    );
}

#[test]
fn should_work_for_iframe() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let button = t.page.frame_locator("iframe").locator("button");
        button.wait_for().await.unwrap();
        assert_eq!(button.inner_text().await.unwrap(), "Hello iframe");
        expect_text(&button, "Hello iframe").await;
        button.click().await.unwrap();
    });
}

#[test]
fn should_work_for_nested_iframe() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let button = t
            .page
            .frame_locator("iframe")
            .frame_locator("iframe")
            .locator("button");
        button.wait_for().await.unwrap();
        assert_eq!(button.inner_text().await.unwrap(), "Hello nested iframe");
        expect_text(&button, "Hello nested iframe").await;
        button.click().await.unwrap();
    });
}

#[test]
fn should_work_for_dollar_and_dollar_dollar() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let locator = t.page.frame_locator("iframe").locator("button");
        expect_text(&locator, "Hello iframe").await;
        let spans = t.page.frame_locator("iframe").locator("span");
        expect_count(&spans, 2).await;
    });
}

#[test]
fn should_wait_for_frame() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let error = t
            .page
            .locator("body")
            .frame_locator("iframe")
            .locator("span")
            .click()
            .timeout(1000)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("waiting for locator('body').locator('iframe').contentFrame()"),
            "{error}"
        );
    });
}

#[test]
fn should_wait_for_frame_2() {
    run(|t| async move {
        route_iframe(&t.server);
        let page = t.page.clone();
        let url = t.server.empty_page.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let _ = page.goto(url).await;
        });
        t.page
            .frame_locator("iframe")
            .locator("button")
            .click()
            .await
            .unwrap();
    });
}

#[test]
fn should_wait_for_frame_to_go() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let page = t.page.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let _ = page
                .eval_on_selector::<Value>("iframe", "e => e.remove()")
                .await;
        });
        expect_hidden(&t.page.frame_locator("iframe").locator("button")).await;
    });
}

#[test]
fn should_not_wait_for_frame() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        expect_hidden(&t.page.frame_locator("iframe").locator("span")).await;
    });
}

#[test]
fn should_not_wait_for_frame_2() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        expect_hidden(&t.page.frame_locator("iframe").locator("span")).await;
    });
}

#[test]
fn should_not_wait_for_frame_3() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        expect_count(&t.page.frame_locator("iframe").locator("span"), 0).await;
    });
}

#[test]
fn should_click_in_lazy_iframe() {
    run(|t| async move {
        t.server.set_content(
            "/iframe.html",
            "text/html",
            "<html><button>Hello iframe</button></html>",
        );
        t.page.goto(&t.server.empty_page).await.unwrap();
        let page = t.page.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let _ = page
                .evaluate::<Value>(
                    "() => { const iframe = document.createElement('iframe'); document.body.appendChild(iframe); }",
                )
                .await;
            tokio::time::sleep(Duration::from_millis(500)).await;
            let _ = page
                .evaluate::<Value>("() => document.querySelector('iframe').src = 'iframe.html'")
                .await;
        });
        let button = t.page.frame_locator("iframe").locator("button");
        let (click, text, ()) = tokio::join!(
            button.click().into_future(),
            button.inner_text().into_future(),
            expect_text(&button, "Hello iframe")
        );
        click.unwrap();
        assert_eq!(text.unwrap(), "Hello iframe");
    });
}

#[test]
fn wait_for_should_survive_frame_reattach() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let button = t
            .page
            .frame_locator("iframe")
            .locator("button:has-text(\"Hello nested iframe\")");
        let waiting = tokio::spawn(button.wait_for().into_future());
        t.page
            .locator("iframe")
            .evaluate::<Value>("e => e.remove()")
            .await
            .unwrap();
        t.page
            .evaluate::<Value>(
                "() => { const iframe = document.createElement('iframe'); iframe.src = 'iframe-2.html'; document.body.appendChild(iframe); }",
            )
            .await
            .unwrap();
        waiting.await.unwrap().unwrap();
    });
}

#[test]
fn click_should_survive_frame_reattach() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let button = t
            .page
            .frame_locator("iframe")
            .locator("button:has-text(\"Hello nested iframe\")");
        let clicking = tokio::spawn(button.click().into_future());
        t.page
            .locator("iframe")
            .evaluate::<Value>("e => e.remove()")
            .await
            .unwrap();
        t.page
            .evaluate::<Value>(
                "() => { const iframe = document.createElement('iframe'); iframe.src = 'iframe-2.html'; document.body.appendChild(iframe); }",
            )
            .await
            .unwrap();
        clicking.await.unwrap().unwrap();
    });
}

#[test]
fn click_should_survive_iframe_navigation() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let button = t
            .page
            .frame_locator("iframe")
            .locator("button:has-text(\"Hello nested iframe\")");
        let clicking = tokio::spawn(button.click().into_future());
        let page = t.page.clone();
        tokio::spawn(async move {
            let _ = page
                .locator("iframe")
                .evaluate::<Value>("e => e.src = 'iframe-2.html'")
                .await;
        });
        clicking.await.unwrap().unwrap();
    });
}

#[test]
fn should_non_work_for_non_frame() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.set_content("<div></div>").await.unwrap();
        let button = t.page.frame_locator("div").locator("button");
        let error = button.wait_for().await.unwrap_err().to_string();
        assert!(error.contains("<div></div>"), "{error}");
        assert!(error.contains("<iframe> was expected"), "{error}");
    });
}

#[test]
fn locator_frame_locator_should_work_for_iframe() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let button = t
            .page
            .locator("body")
            .frame_locator("iframe")
            .locator("button");
        button.wait_for().await.unwrap();
        assert_eq!(button.inner_text().await.unwrap(), "Hello iframe");
        expect_text(&button, "Hello iframe").await;
        button.click().await.unwrap();
    });
}

#[test]
fn locator_frame_locator_should_throw_on_ambiguity() {
    run(|t| async move {
        route_ambiguous(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let button = t
            .page
            .locator("body")
            .frame_locator("iframe")
            .locator("button");
        let error = button.wait_for().await.unwrap_err().to_string();
        assert!(
            error.contains(
                "Error: strict mode violation: locator('body').locator('iframe') resolved to 3 elements"
            ),
            "{error}"
        );
    });
}

#[test]
fn locator_frame_locator_should_not_throw_on_first_last_nth() {
    run(|t| async move {
        route_ambiguous(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let fl = || t.page.locator("body").frame_locator("iframe");
        expect_text(&fl().first().locator("button"), "Hello from iframe-1.html").await;
        expect_text(&fl().nth(1).locator("button"), "Hello from iframe-2.html").await;
        expect_text(&fl().last().locator("button"), "Hello from iframe-3.html").await;
    });
}

#[test]
fn get_by_coverage() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let fl = || t.page.frame_locator("iframe");
        expect_text(&fl().get_by_role("button"), "Hello iframe").await;
        expect_text(&fl().get_by_text("Hello"), "Hello iframe").await;
        expect_text(&fl().get_by_test_id("buttonId"), "Hello iframe").await;
        expect_value(&fl().get_by_label("Name"), "").await;
        expect_value(&fl().get_by_placeholder("Placeholder"), "").await;
        expect_value(&fl().get_by_alt_text("Alternative"), "").await;
        expect_value(&fl().get_by_title("Title"), "").await;
    });
}

#[test]
fn wait_for_hidden_should_succeed_when_frame_is_not_in_dom() {
    run(|t| async move {
        t.page.goto("about:blank").await.unwrap();
        let button = t.page.frame_locator("iframe1").locator("button");
        assert!(button.is_hidden().await.unwrap());
        button
            .wait_for()
            .state(WaitForSelectorState::Hidden)
            .timeout(1000)
            .await
            .unwrap();
        button
            .wait_for()
            .state(WaitForSelectorState::Detached)
            .timeout(1000)
            .await
            .unwrap();
        let error = button
            .wait_for()
            .state(WaitForSelectorState::Attached)
            .timeout(1000)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Timeout 1000ms exceeded"));
    });
}

#[test]
fn locator_content_frame_should_work() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let locator = t.page.locator("iframe");
        let button = locator.content_frame().locator("button");
        assert_eq!(button.inner_text().await.unwrap(), "Hello iframe");
        expect_text(&button, "Hello iframe").await;
        button.click().await.unwrap();
    });
}

#[test]
fn frame_locator_owner_should_work() {
    run(|t| async move {
        route_iframe(&t.server);
        t.page.goto(&t.server.empty_page).await.unwrap();
        let locator = t.page.frame_locator("iframe").owner();
        expect_visible(&locator).await;
        assert_eq!(
            locator.get_attribute("name").await.unwrap().as_deref(),
            Some("frame1")
        );
    });
}

#[test]
fn should_treat_elements_inside_hidden_iframe_as_hidden() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<iframe name="hidden" style="visibility: hidden" srcdoc="<button>Hidden</button>"></iframe>
    <div style="visibility: hidden">
      <iframe name="hidden-parent" srcdoc="<button>Hidden parent</button>"></iframe>
      <iframe name="visible-override" style="visibility: visible" srcdoc="<button>Visible override</button>"></iframe>
    </div>
    <iframe name="outer" style="visibility: hidden" srcdoc="<iframe name='inner' srcdoc='<button>Nested</button>'></iframe>"></iframe>"#,
            )
            .await
            .unwrap();
        for button in [
            t.page.frame_locator("[name=hidden]").locator("button"),
            t.page
                .frame_locator("[name=hidden-parent]")
                .locator("button"),
            t.page
                .frame_locator("[name=outer]")
                .frame_locator("[name=inner]")
                .locator("button"),
        ] {
            expect_attached(&button).await;
            assert!(!button.is_visible().await.unwrap());
            assert!(button.is_hidden().await.unwrap());
            expect_hidden(&button).await;
            button
                .wait_for()
                .state(WaitForSelectorState::Hidden)
                .await
                .unwrap();
        }
        let button = t
            .page
            .frame_locator("[name=visible-override]")
            .locator("button");
        expect_visible(&button).await;
        assert!(button.is_visible().await.unwrap());
        button
            .wait_for()
            .state(WaitForSelectorState::Visible)
            .await
            .unwrap();
    });
}

#[test]
fn should_wait_for_hidden_iframe_to_become_visible() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<iframe name="frame" style="visibility: hidden" srcdoc="<button onclick='window.__clicked = true'>Button</button>"></iframe>"#,
            )
            .await
            .unwrap();
        let button = t.page.frame_locator("[name=frame]").locator("button");
        expect_hidden(&button).await;
        let error = button.click().timeout(1000).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("element is inside a hidden frame, retrying"),
            "{error}"
        );
        let clicked: Option<bool> = button.evaluate("() => window.__clicked").await.unwrap();
        assert_eq!(clicked, None);
        let clicking = tokio::spawn(button.click().into_future());
        t.page
            .evaluate::<Value>(
                "() => document.querySelector('iframe').style.visibility = 'visible'",
            )
            .await
            .unwrap();
        expect_visible(&button).await;
        clicking.await.unwrap().unwrap();
        let clicked: Option<bool> = button.evaluate("() => window.__clicked").await.unwrap();
        assert_eq!(clicked, Some(true));
        t.page
            .evaluate::<Value>("() => document.querySelector('iframe').style.visibility = 'hidden'")
            .await
            .unwrap();
        expect_hidden(&button).await;
        assert!(!button.is_visible().await.unwrap());
    });
}

#[test]
fn should_treat_elements_inside_hidden_cross_origin_iframe_as_hidden() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .set_content(format!(
                "<iframe style=\"visibility: hidden\" src=\"{}/frames/frame.html\"></iframe>",
                t.server.cross_process_prefix
            ))
            .await
            .unwrap();
        let div = t.page.frame_locator("iframe").locator("div");
        expect_text(&div, "Hi, I'm frame").await;
        assert!(!div.is_visible().await.unwrap());
        expect_hidden(&div).await;
        t.page
            .evaluate::<Value>(
                "() => document.querySelector('iframe').style.visibility = 'visible'",
            )
            .await
            .unwrap();
        expect_visible(&div).await;
        assert!(div.is_visible().await.unwrap());
    });
}

use std::future::IntoFuture;
