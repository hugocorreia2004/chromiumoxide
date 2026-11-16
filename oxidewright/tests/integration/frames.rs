//! Ports of `frame-evaluate.spec.ts`, `frame-hierarchy.spec.ts` and
//! `frame-frame-element.spec.ts`.
//!
//! Skipped: FR-003/004 (inspect Playwright server internals), FR-025, FR-026,
//! FR-028..030 and FR-034 assertions on `frameattached`/`framenavigated`/
//! `framedetached` events (the hierarchy part is checked), FR-035 (`console`
//! event). `page.$(..)` is replaced by `evaluate_handle(querySelector)`.

use std::time::Duration;

use serde_json::{Value, json};

use crate::common::*;

// ---- frame-evaluate.spec.ts ----

#[test]
fn should_have_different_execution_contexts() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        let frames = t.page.frames();
        assert_eq!(frames.len(), 2);
        frames[0]
            .evaluate::<()>("() => window['FOO'] = 'foo'")
            .await
            .unwrap();
        frames[1]
            .evaluate::<()>("() => window['FOO'] = 'bar'")
            .await
            .unwrap();
        assert_eq!(
            frames[0]
                .evaluate::<String>("() => window['FOO']")
                .await
                .unwrap(),
            "foo"
        );
        assert_eq!(
            frames[1]
                .evaluate::<String>("() => window['FOO']")
                .await
                .unwrap(),
            "bar"
        );
    });
}

#[test]
fn should_have_correct_execution_contexts() {
    run(|t| async move {
        t.page
            .goto(format!("{}/frames/one-frame.html", t.server.prefix))
            .await
            .unwrap();
        let frames = t.page.frames();
        assert_eq!(frames.len(), 2);
        assert_eq!(
            frames[0]
                .evaluate::<String>("() => document.body.textContent.trim()")
                .await
                .unwrap(),
            ""
        );
        assert_eq!(
            frames[1]
                .evaluate::<String>("() => document.body.textContent.trim()")
                .await
                .unwrap(),
            "Hi, I'm frame"
        );
    });
}

#[test]
fn should_execute_after_cross_site_navigation() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let main_frame = t.page.main_frame();
        let href: String = main_frame
            .evaluate("() => window.location.href")
            .await
            .unwrap();
        assert!(href.contains(&t.server.empty_page));
        t.page
            .goto(format!("{}/empty.html", t.server.cross_process_prefix))
            .await
            .unwrap();
        let href: String = main_frame
            .evaluate("() => window.location.href")
            .await
            .unwrap();
        assert!(href.contains(&t.server.cross_process_prefix));
        // FR-027: the main frame object persists.
        assert_eq!(t.page.main_frame(), main_frame);
    });
}

#[test]
fn should_not_allow_cross_frame_js_handles() {
    run(|t| async move {
        t.page
            .goto(format!("{}/frames/one-frame.html", t.server.prefix))
            .await
            .unwrap();
        let handle = t
            .page
            .evaluate_handle("() => { const iframe = document.querySelector('iframe'); const foo = { bar: 'baz' }; iframe.contentWindow['__foo'] = foo; return foo; }")
            .await
            .unwrap();
        let child = t.page.main_frame().child_frames()[0].clone();
        let v: Value = child.evaluate("() => window['__foo']").await.unwrap();
        assert_eq!(v, json!({ "bar": "baz" }));
        let r = child
            .evaluate::<String>("foo => foo.bar")
            .arg(&handle)
            .await;
        assert_err_contains(
            r,
            "JSHandles can be evaluated only in the context they were created!",
        );
    });
}

#[test]
fn should_allow_cross_frame_element_handles() {
    run(|t| async move {
        t.page
            .goto(format!("{}/frames/one-frame.html", t.server.prefix))
            .await
            .unwrap();
        let body = t.page.main_frame().child_frames()[0]
            .evaluate_handle("() => document.body")
            .await
            .unwrap();
        let html: String = t
            .page
            .evaluate("body => body.innerHTML")
            .arg(&body)
            .await
            .unwrap();
        assert_eq!(html.trim(), "<div>Hi, I'm frame</div>");
    });
}

#[test]
fn should_not_allow_cross_frame_element_handles_when_frames_do_not_script_each_other() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let frame = attach_frame(
            &t.page,
            "frame1",
            &format!("{}/empty.html", t.server.cross_process_prefix),
        )
        .await;
        let body = frame.evaluate_handle("() => document.body").await.unwrap();
        let r = t
            .page
            .evaluate::<String>("body => body.innerHTML")
            .arg(&body)
            .await;
        assert_err_contains(
            r,
            "Unable to adopt element handle from a different document",
        );
    });
}

#[test]
fn should_throw_for_detached_frames() {
    run(|t| async move {
        let frame1 = attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        detach_frame(&t.page, "frame1").await;
        let r = frame1.evaluate::<i32>("() => 7 * 8").await;
        assert_err_contains(r, "frame.evaluate: Frame was detached");
    });
}

#[test]
fn should_be_isolated_between_frames() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        let frames = t.page.frames();
        assert_eq!(frames.len(), 2);
        assert_ne!(frames[0], frames[1]);
        let (a, b) = tokio::join!(
            frames[0]
                .evaluate::<()>("() => window['a'] = 1")
                .into_future(),
            frames[1]
                .evaluate::<()>("() => window['a'] = 2")
                .into_future()
        );
        a.unwrap();
        b.unwrap();
        let (a1, a2) = tokio::join!(
            frames[0].evaluate::<i32>("() => window['a']").into_future(),
            frames[1].evaluate::<i32>("() => window['a']").into_future()
        );
        assert_eq!((a1.unwrap(), a2.unwrap()), (1, 2));
    });
}

#[test]
fn should_work_in_iframes_that_failed_initial_navigation() {
    run(|t| async move {
        t.page
            .set_content("<meta http-equiv=\"Content-Security-Policy\" content=\"script-src 'none';\"><iframe src='javascript:\"\"'></iframe>")
            .wait_until(oxidewright::LoadState::DomContentLoaded)
            .await
            .unwrap();
        t.page
            .evaluate::<()>("() => { const iframe = document.querySelector('iframe'); const div = iframe.contentDocument.createElement('div'); iframe.contentDocument.body.appendChild(div); }")
            .await
            .unwrap();
        let frame = t.page.frames()[1].clone();
        assert_eq!(frame.url(), "");
        // Main world should work.
        let href: String = frame.evaluate("() => window.location.href").await.unwrap();
        assert_eq!(href, "about:blank");
        // Utility world should work.
        let has_div: bool = frame
            .evaluate("() => !!document.querySelector('div')")
            .isolated_context(true)
            .await
            .unwrap();
        assert!(has_div);
    });
}

#[test]
fn should_work_in_iframes_that_interrupted_initial_javascript_url_navigation() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .evaluate::<()>("() => { const iframe = document.createElement('iframe'); iframe.src = 'javascript:\"\"'; document.body.appendChild(iframe); iframe.contentDocument.open(); iframe.contentDocument.write('<div>hello</div>'); iframe.contentDocument.close(); }")
            .await
            .unwrap();
        let frame = t.page.frames()[1].clone();
        let href: String = frame
            .evaluate("() => window.top.location.href")
            .await
            .unwrap();
        assert_eq!(href, t.server.empty_page);
        let has_div: bool = frame
            .evaluate("() => !!document.querySelector('div')")
            .isolated_context(true)
            .await
            .unwrap();
        assert!(has_div);
    });
}

#[test]
fn evaluate_handle_should_work() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let window = t
            .page
            .main_frame()
            .evaluate_handle("() => window")
            .await
            .unwrap();
        assert_eq!(window.to_string(), "Window");
    });
}

#[test]
fn should_evaluate_in_out_of_process_iframes() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let frame = attach_frame(
            &t.page,
            "oopif",
            &format!("{}/frames/frame.html", t.server.cross_process_prefix),
        )
        .await;
        let text: String = frame
            .evaluate("() => document.body.textContent.trim()")
            .await
            .unwrap();
        assert_eq!(text, "Hi, I'm frame");
        let origin: String = frame
            .evaluate("() => location.origin")
            .isolated_context(true)
            .await
            .unwrap();
        assert_eq!(origin, t.server.cross_process_prefix);
        assert_eq!(frame.title().await.unwrap(), "");
        frame
            .goto(format!("{}/title.html", t.server.cross_process_prefix))
            .await
            .unwrap();
        assert_eq!(frame.title().await.unwrap(), "Woof-Woof");
        let element = frame.frame_element().await.unwrap();
        assert_eq!(
            element.get_attribute("id").await.unwrap().as_deref(),
            Some("oopif")
        );
    });
}

// ---- frame-hierarchy.spec.ts ----

#[test]
fn should_handle_nested_frames() {
    run(|t| async move {
        t.page
            .goto(format!("{}/frames/nested-frames.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(
            dump_frames(&t.page.main_frame(), ""),
            vec![
                "http://localhost:<PORT>/frames/nested-frames.html",
                "    http://localhost:<PORT>/frames/frame.html (aframe)",
                "    http://localhost:<PORT>/frames/two-frames.html (2frames)",
                "        http://localhost:<PORT>/frames/frame.html (dos)",
                "        http://localhost:<PORT>/frames/frame.html (uno)",
            ]
        );
    });
}

#[test]
fn should_track_frames_manipulated_dynamically() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let frame = attach_frame(&t.page, "frame1", "./assets/frame.html").await;
        assert!(frame.url().contains("/assets/frame.html"));
        t.page
            .evaluate::<()>("() => { const frame = document.getElementById('frame1'); frame.src = './empty.html'; return new Promise(x => frame.onload = x); }")
            .await
            .unwrap();
        assert_eq!(frame.url(), t.server.empty_page);
        detach_frame(&t.page, "frame1").await;
        assert!(frame.is_detached());
        assert_eq!(t.page.frames().len(), 1);
    });
}

#[test]
fn should_detach_child_frames_on_navigation() {
    run(|t| async move {
        t.page
            .goto(format!("{}/frames/nested-frames.html", t.server.prefix))
            .await
            .unwrap();
        let frames = t.page.frames();
        assert_eq!(frames.len(), 5);
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(t.page.frames().len(), 1);
        assert!(frames[1..].iter().all(|f| f.is_detached()));
        assert!(!frames[0].is_detached());
    });
}

#[test]
fn should_support_framesets() {
    run(|t| async move {
        t.page
            .goto(format!("{}/frames/frameset.html", t.server.prefix))
            .await
            .unwrap();
        let frames = t.page.frames();
        assert_eq!(frames.len(), 5);
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(t.page.frames().len(), 1);
        assert_eq!(frames.iter().filter(|f| f.is_detached()).count(), 4);
    });
}

#[test]
fn should_report_frame_from_inside_shadow_dom() {
    run(|t| async move {
        t.page
            .goto(format!("{}/shadow.html", t.server.prefix))
            .await
            .unwrap();
        t.page
            .evaluate::<()>("async url => { const frame = document.createElement('iframe'); frame.src = url; document.body.shadowRoot.appendChild(frame); await new Promise(x => frame.onload = x); }")
            .arg(&t.server.empty_page)
            .await
            .unwrap();
        assert_eq!(t.page.frames().len(), 2);
        assert_eq!(t.page.frames()[1].url(), t.server.empty_page);
    });
}

#[test]
fn should_report_frame_name() {
    run(|t| async move {
        attach_frame(&t.page, "theFrameId", &t.server.empty_page).await;
        t.page
            .evaluate::<()>("url => { const frame = document.createElement('iframe'); frame.name = 'theFrameName'; frame.src = url; document.body.appendChild(frame); return new Promise(x => frame.onload = x); }")
            .arg(&t.server.empty_page)
            .await
            .unwrap();
        let frames = t.page.frames();
        assert_eq!(frames[0].name(), "");
        assert_eq!(frames[1].name(), "theFrameId");
        assert_eq!(frames[2].name(), "theFrameName");
        assert_eq!(t.page.frame("theFrameName").unwrap(), frames[2]);
    });
}

#[test]
fn should_report_frame_parent() {
    run(|t| async move {
        attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        attach_frame(&t.page, "frame2", &t.server.empty_page).await;
        let frames = t.page.frames();
        assert!(frames[0].parent_frame().is_none());
        assert_eq!(frames[1].parent_frame().unwrap(), t.page.main_frame());
        assert_eq!(frames[2].parent_frame().unwrap(), t.page.main_frame());
    });
}

#[test]
fn should_report_different_frame_instance_when_frame_re_attaches() {
    run(|t| async move {
        let frame1 = attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        t.page
            .evaluate::<()>("() => { window['frame'] = document.querySelector('#frame1'); window['frame'].remove(); }")
            .await
            .unwrap();
        assert!(frame1.is_detached());
        t.page
            .evaluate::<()>("() => { document.body.appendChild(window['frame']); }")
            .await
            .unwrap();
        let mut frame2 = None;
        for _ in 0..100 {
            if let Some(f) = t.page.frames().get(1) {
                frame2 = Some(f.clone());
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let frame2 = frame2.expect("re-attached frame");
        assert!(!frame2.is_detached());
        assert_ne!(frame1, frame2);
    });
}

#[test]
fn should_return_frame_page() {
    run(|t| async move {
        t.page
            .goto(format!("{}/frames/one-frame.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(t.page.main_frame().page().unwrap(), t.page);
        assert_eq!(
            t.page.main_frame().child_frames()[0].page().unwrap(),
            t.page
        );
    });
}

// ---- frame-frame-element.spec.ts ----

#[test]
fn frame_element_should_work() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let frame1 = attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        attach_frame(&t.page, "frame2", &t.server.empty_page).await;
        let frame3 = attach_frame(&t.page, "frame3", &t.server.empty_page).await;
        let frame1_handle1 = t
            .page
            .evaluate_handle("() => document.querySelector('#frame1')")
            .await
            .unwrap();
        let frame1_handle2 = frame1.frame_element().await.unwrap();
        let frame3_handle1 = t
            .page
            .evaluate_handle("() => document.querySelector('#frame3')")
            .await
            .unwrap();
        let frame3_handle2 = frame3.frame_element().await.unwrap();
        assert!(
            frame1_handle1
                .evaluate::<bool>("(a, b) => a === b")
                .arg(&frame1_handle2)
                .await
                .unwrap()
        );
        assert!(
            frame3_handle1
                .evaluate::<bool>("(a, b) => a === b")
                .arg(&frame3_handle2)
                .await
                .unwrap()
        );
        assert!(
            !frame1_handle1
                .evaluate::<bool>("(a, b) => a === b")
                .arg(&frame3_handle1)
                .await
                .unwrap()
        );
    });
}

#[test]
fn frame_element_should_work_with_content_frame() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let frame = attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        let handle = frame.frame_element().await.unwrap();
        assert_eq!(handle.content_frame().await.unwrap().unwrap(), frame);
        assert_eq!(
            handle.owner_frame().await.unwrap().unwrap(),
            t.page.main_frame()
        );
    });
}

#[test]
fn frame_element_should_work_with_frameset() {
    run(|t| async move {
        t.page
            .goto(format!("{}/frames/frameset.html", t.server.prefix))
            .await
            .unwrap();
        let element1 = t
            .page
            .evaluate_handle("() => document.querySelector('frame')")
            .await
            .unwrap();
        let frame = element1
            .as_element()
            .unwrap()
            .content_frame()
            .await
            .unwrap()
            .unwrap();
        let element2 = frame.frame_element().await.unwrap();
        assert!(
            element1
                .evaluate::<bool>("(a, b) => a === b")
                .arg(&element2)
                .await
                .unwrap()
        );
    });
}

#[test]
fn frame_element_should_throw_when_detached() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let frame1 = attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        t.page
            .evaluate::<()>("() => document.querySelector('#frame1').remove()")
            .await
            .unwrap();
        assert_err_contains(frame1.frame_element().await, "Frame has been detached.");
    });
}

#[test]
fn frame_element_should_work_inside_closed_and_declarative_shadow_roots() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .set_content("<div id=framecontainer></div><script>const iframe = document.createElement('iframe'); iframe.setAttribute('name', 'myframe'); iframe.setAttribute('srcdoc', 'find me'); const div = document.getElementById('framecontainer'); const host = div.attachShadow({ mode: 'closed' }); host.appendChild(iframe);</script>")
            .await
            .unwrap();
        let element = t.page.frames()[1].frame_element().await.unwrap();
        assert_eq!(
            element.get_attribute("name").await.unwrap().as_deref(),
            Some("myframe")
        );
        t.page
            .set_content("<div><template shadowrootmode=\"open\"><iframe name=\"myframe2\" srcdoc=\"<h1>Hi!</h1>\"></iframe><slot></slot></template><span>footer</span></div>")
            .await
            .unwrap();
        let element = t.page.frames()[1].frame_element().await.unwrap();
        assert_eq!(
            element.get_attribute("name").await.unwrap().as_deref(),
            Some("myframe2")
        );
    });
}

use std::future::IntoFuture;
