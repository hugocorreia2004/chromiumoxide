//! Port of `elementhandle-scroll-into-view.spec.ts` (all tests ported).

use std::future::IntoFuture;
use std::time::Duration;

use oxidewright::Page;

use crate::common::*;

#[test]
fn should_work() {
    run(|t| async move {
        t.page
            .goto(format!("{}/offscreenbuttons.html", t.server.prefix))
            .await
            .unwrap();
        for i in 0..11 {
            let button = t
                .page
                .query_selector(&format!("#btn{i}"))
                .await
                .unwrap()
                .unwrap();
            let before: f64 = button
                .evaluate("button => button.getBoundingClientRect().right - window.innerWidth")
                .await
                .unwrap();
            assert_eq!(before, (10 * i) as f64);
            button.scroll_into_view_if_needed().await.unwrap();
            let after: f64 = button
                .evaluate("button => button.getBoundingClientRect().right - window.innerWidth")
                .await
                .unwrap();
            assert!(after <= 0.0);
            t.page
                .evaluate::<()>("() => window.scrollTo(0, 0)")
                .await
                .unwrap();
        }
    });
}

#[test]
fn should_throw_for_detached_element() {
    run(|t| async move {
        t.page.set_content("<div>Hello</div>").await.unwrap();
        let div = t.page.query_selector("div").await.unwrap().unwrap();
        div.evaluate::<()>("div => div.remove()").await.unwrap();
        assert_err_contains(
            div.scroll_into_view_if_needed().await,
            "Element is not attached to the DOM",
        );
    });
}

async fn test_waiting(page: &Page, after: &str) {
    let div = page.query_selector("div").await.unwrap().unwrap();
    let task = tokio::spawn(div.scroll_into_view_if_needed().into_future());
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert!(!task.is_finished());
    div.evaluate::<()>(after).await.unwrap();
    task.await.unwrap().unwrap();
}

#[test]
fn should_wait_for_display_none_to_become_visible() {
    run(|t| async move {
        t.page
            .set_content("<div style=\"display:none\">Hello</div>")
            .await
            .unwrap();
        test_waiting(&t.page, "div => { div.style.display = 'block'; }").await;
    });
}

#[test]
fn should_scroll_display_contents_into_view() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<style>
                  html, body { margin: 0; padding: 0; width: 100%; height: 100%; }
                  ::-webkit-scrollbar { display: none; }
                  * { scrollbar-width: none; }
                </style>
                <div id=container style="width:200px;height:200px;overflow:scroll;border:1px solid black;">
                  <div style="margin-top:500px;background:red;">
                    <div style="height:50px;width:100px;background:cyan;">
                      <div id=target style="display:contents">Hello</div>
                    </div>
                  <div>
                </div>"#,
            )
            .await
            .unwrap();
        let div = t.page.query_selector("#target").await.unwrap().unwrap();
        div.scroll_into_view_if_needed().await.unwrap();
        let scroll_top: f64 = t
            .page
            .eval_on_selector("#container", "e => e.scrollTop")
            .await
            .unwrap();
        assert!((scroll_top - 350.0).abs() < 1.0, "{scroll_top}");
    });
}

#[test]
fn should_scroll_nested_display_contents_into_view() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<style>
                  html, body { margin: 0; padding: 0; width: 100%; height: 100%; }
                  ::-webkit-scrollbar { display: none; }
                  * { scrollbar-width: none; }
                </style>
                <div id=container style="width:200px;height:200px;overflow:scroll;border:1px solid black;">
                  <div style="margin-top:500px;background:red;">
                    <div id=outer style="display:contents"><div style="display:contents"><div id=target style="height:50px;width:100px;background:cyan;">Hello</div></div><div style="height:2000px;"></div><div>Sibling</div></div>
                  </div>
                </div>"#,
            )
            .await
            .unwrap();
        let div = t.page.query_selector("#outer").await.unwrap().unwrap();
        div.scroll_into_view_if_needed().await.unwrap();
        let visible: bool = t
            .page
            .eval_on_selector(
                "#target",
                "target => { const container = document.getElementById('container').getBoundingClientRect(); const rect = target.getBoundingClientRect(); return rect.top >= container.top && rect.bottom <= container.bottom; }",
            )
            .await
            .unwrap();
        assert!(visible);
    });
}

#[test]
fn should_work_for_visibility_hidden_element() {
    run(|t| async move {
        t.page
            .set_content("<div style=\"visibility:hidden\">Hello</div>")
            .await
            .unwrap();
        let div = t.page.query_selector("div").await.unwrap().unwrap();
        div.scroll_into_view_if_needed().await.unwrap();
    });
}

#[test]
fn should_work_for_zero_sized_element() {
    run(|t| async move {
        t.page
            .set_content("<div style=\"height:0\">Hello</div>")
            .await
            .unwrap();
        let div = t.page.query_selector("div").await.unwrap().unwrap();
        div.scroll_into_view_if_needed().await.unwrap();
    });
}

#[test]
fn should_wait_for_nested_display_none_to_become_visible() {
    run(|t| async move {
        t.page
            .set_content("<span style=\"display:none\"><div>Hello</div></span>")
            .await
            .unwrap();
        test_waiting(
            &t.page,
            "div => { div.parentElement.style.display = 'block'; }",
        )
        .await;
    });
}

#[test]
fn should_wait_for_element_to_stop_moving() {
    run(|t| async move {
        t.page
            .set_content(
                "<style>
                  @keyframes move { from { margin-left: 0; } to { margin-left: 200px; } }
                  div.animated { animation: 2s linear 0s infinite alternate move; }
                </style>
                <div class=animated>moving</div>",
            )
            .await
            .unwrap();
        test_waiting(&t.page, "div => div.classList.remove('animated')").await;
    });
}

#[test]
fn should_timeout_waiting_for_visible() {
    run(|t| async move {
        t.page
            .set_content("<div style=\"display:none\">Hello</div>")
            .await
            .unwrap();
        let div = t.page.query_selector("div").await.unwrap().unwrap();
        let err = assert_err_contains(
            div.scroll_into_view_if_needed().timeout(3000).await,
            "element is not visible",
        );
        assert!(
            err.to_string().contains("retrying scroll into view action"),
            "{err}"
        );
    });
}
