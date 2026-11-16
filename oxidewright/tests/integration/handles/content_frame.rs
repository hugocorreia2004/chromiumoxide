//! Port of `elementhandle-content-frame.spec.ts` (all tests ported).

use crate::common::*;

#[test]
fn should_work() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        let handle = t.page.query_selector("#frame1").await.unwrap().unwrap();
        let frame = handle.content_frame().await.unwrap();
        assert_eq!(frame, Some(t.page.frames()[1].clone()));
    });
}

#[test]
fn should_work_for_cross_process_iframes() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        attach_frame(
            &t.page,
            "frame1",
            &format!("{}/empty.html", t.server.cross_process_prefix),
        )
        .await;
        let handle = t.page.query_selector("#frame1").await.unwrap().unwrap();
        let frame = handle.content_frame().await.unwrap();
        assert_eq!(frame, Some(t.page.frames()[1].clone()));
    });
}

#[test]
fn should_work_for_cross_frame_evaluations() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        let frame = t.page.frames()[1].clone();
        let handle = frame
            .evaluate_handle("() => window.top.document.querySelector('#frame1')")
            .await
            .unwrap();
        let element = handle.as_element().unwrap();
        assert_eq!(element.content_frame().await.unwrap(), Some(frame));
    });
}

#[test]
fn should_return_null_for_non_iframes() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        let frame = t.page.frames()[1].clone();
        let handle = frame.evaluate_handle("() => document.body").await.unwrap();
        let element = handle.as_element().unwrap();
        assert_eq!(element.content_frame().await.unwrap(), None);
    });
}

#[test]
fn should_return_null_for_document_document_element() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        let frame = t.page.frames()[1].clone();
        let handle = frame
            .evaluate_handle("() => document.documentElement")
            .await
            .unwrap();
        let element = handle.as_element().unwrap();
        assert_eq!(element.content_frame().await.unwrap(), None);
    });
}
