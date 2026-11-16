//! Port of `elementhandle-select-text.spec.ts` (nothing skipped).

use std::time::Duration;

use super::spawn;
use crate::common::*;

async fn selection(t: &TestContext) -> String {
    t.page
        .evaluate("() => window.getSelection().toString()")
        .await
        .unwrap()
}

async fn goto_textarea(t: &TestContext) {
    t.page
        .goto(format!("{}/input/textarea.html", t.server.prefix))
        .await
        .unwrap();
}

#[test]
fn should_select_textarea() {
    run(|t| async move {
        goto_textarea(&t).await;
        let textarea = t.page.query_selector("textarea").await.unwrap().unwrap();
        textarea
            .evaluate::<()>("textarea => { textarea.value = 'some value'; }")
            .await
            .unwrap();
        textarea.select_text().await.unwrap();
        assert_eq!(selection(&t).await, "some value");
    });
}

#[test]
fn should_select_input() {
    run(|t| async move {
        goto_textarea(&t).await;
        let input = t.page.query_selector("input").await.unwrap().unwrap();
        input
            .evaluate::<()>("input => { input.value = 'some value'; }")
            .await
            .unwrap();
        input.select_text().await.unwrap();
        assert_eq!(selection(&t).await, "some value");
    });
}

#[test]
fn should_select_plain_div() {
    run(|t| async move {
        goto_textarea(&t).await;
        let div = t.page.query_selector("div.plain").await.unwrap().unwrap();
        div.select_text().await.unwrap();
        assert_eq!(selection(&t).await, "Plain div");
    });
}

#[test]
fn should_timeout_waiting_for_invisible_element() {
    run(|t| async move {
        goto_textarea(&t).await;
        let textarea = t.page.query_selector("textarea").await.unwrap().unwrap();
        textarea
            .evaluate::<()>("e => { e.style.display = 'none'; }")
            .await
            .unwrap();
        let error = textarea.select_text().timeout(3000).await.unwrap_err();
        assert!(
            error.to_string().contains("element is not visible"),
            "{error}"
        );
    });
}

#[test]
fn should_wait_for_visible() {
    run(|t| async move {
        goto_textarea(&t).await;
        let textarea = t.page.query_selector("textarea").await.unwrap().unwrap();
        textarea
            .evaluate::<()>("textarea => { textarea.value = 'some value'; }")
            .await
            .unwrap();
        textarea
            .evaluate::<()>("e => { e.style.display = 'none'; }")
            .await
            .unwrap();
        let select = spawn(textarea.select_text().timeout(3000));
        tokio::time::sleep(Duration::from_millis(1000)).await;
        assert!(!select.is_finished());
        textarea
            .evaluate::<()>("e => { e.style.display = 'block'; }")
            .await
            .unwrap();
        select.await.unwrap().unwrap();
    });
}
