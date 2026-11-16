//! Port of `elementhandle-owner-frame.spec.ts`.
//!
//! Skipped: EH-061 "should work for adopted elements" (needs the `popup`
//! event / `wait_for_event`).

use crate::common::*;

#[test]
fn should_work() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        let frame = t.page.frames()[1].clone();
        let handle = frame.evaluate_handle("() => document.body").await.unwrap();
        let element = handle.as_element().unwrap();
        assert_eq!(element.owner_frame().await.unwrap(), Some(frame));
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
        let frame = t.page.frames()[1].clone();
        let handle = frame.evaluate_handle("() => document.body").await.unwrap();
        let element = handle.as_element().unwrap();
        assert_eq!(element.owner_frame().await.unwrap(), Some(frame));
    });
}

#[test]
fn should_work_for_document() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        let frame = t.page.frames()[1].clone();
        let handle = frame.evaluate_handle("() => document").await.unwrap();
        let element = handle.as_element().unwrap();
        assert_eq!(element.owner_frame().await.unwrap(), Some(frame));
    });
}

#[test]
fn should_work_for_iframe_elements() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        let frame = t.page.main_frame();
        let handle = frame
            .evaluate_handle("() => document.querySelector('#frame1')")
            .await
            .unwrap();
        let element = handle.as_element().unwrap();
        assert_eq!(element.owner_frame().await.unwrap(), Some(frame));
    });
}

#[test]
fn should_work_for_cross_frame_evaluations() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        let frame = t.page.main_frame();
        let handle = frame
            .evaluate_handle("() => document.querySelector('iframe').contentWindow.document.body")
            .await
            .unwrap();
        let element = handle.as_element().unwrap();
        assert_eq!(
            element.owner_frame().await.unwrap(),
            Some(frame.child_frames()[0].clone())
        );
    });
}

#[test]
fn should_work_for_detached_elements() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let div = t
            .page
            .evaluate_handle(
                "() => { const div = document.createElement('div'); document.body.appendChild(div); return div; }",
            )
            .await
            .unwrap();
        let div = div.as_element().unwrap();
        assert_eq!(div.owner_frame().await.unwrap(), Some(t.page.main_frame()));
        t.page
            .evaluate::<()>(
                "() => { const div = document.querySelector('div'); document.body.removeChild(div); }",
            )
            .await
            .unwrap();
        assert_eq!(div.owner_frame().await.unwrap(), Some(t.page.main_frame()));
    });
}
