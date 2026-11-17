//! Ports of `page-wait-for-selector-1.spec.ts` and `page-wait-for-selector-2.spec.ts`.
//!
//! Skipped:
//! - WA-027 "should throw on waitFor", WA-028 "should tolerate waitFor=visible",
//!   WA-059 "should throw for visibility option", WA-060/WA-061 "should throw
//!   for true/false state option": these options do not exist in the typed
//!   Rust API (WA-058 checks the `state` parsing error instead).
//! - WA-057 "should have correct stack trace for timeout": JavaScript stacks.
//! - WA-072/WA-073: need Playwright's `__testHookBeforeAdoptNode`.

use std::future::IntoFuture;
use std::time::Duration;

use oxidewright::WaitForSelectorState;

use super::{ADD_ELEMENT, rafraf_frame};
use crate::common::*;

// ---- page-wait-for-selector-1.spec.ts ----

#[test]
fn should_immediately_resolve_promise_if_node_exists() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let frame = t.page.main_frame();
        frame.wait_for_selector("*").await.unwrap();
        frame.evaluate::<()>(ADD_ELEMENT).arg("div").await.unwrap();
        frame
            .wait_for_selector("div")
            .state(WaitForSelectorState::Attached)
            .await
            .unwrap();
    });
}

#[test]
fn element_handle_wait_for_selector_should_immediately_resolve_if_node_exists() {
    run(|t| async move {
        t.page
            .set_content("<span>extra</span><div><span>target</span></div>")
            .await
            .unwrap();
        let div = t.page.query_selector("div").await.unwrap().unwrap();
        let span = div
            .wait_for_selector("span")
            .state(WaitForSelectorState::Attached)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            span.evaluate::<String>("e => e.textContent").await.unwrap(),
            "target"
        );
    });
}

#[test]
fn element_handle_wait_for_selector_should_wait() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let div = t.page.query_selector("div").await.unwrap().unwrap();
        let task = tokio::spawn(
            div.wait_for_selector("span")
                .state(WaitForSelectorState::Attached)
                .into_future(),
        );
        div.evaluate::<()>("div => { div.innerHTML = '<span>target</span>'; }")
            .await
            .unwrap();
        let span = task.await.unwrap().unwrap().unwrap();
        assert_eq!(
            span.evaluate::<String>("e => e.textContent").await.unwrap(),
            "target"
        );
    });
}

#[test]
fn element_handle_wait_for_selector_should_timeout() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let div = t.page.query_selector("div").await.unwrap().unwrap();
        assert_err_contains(
            div.wait_for_selector("span").timeout(100).await,
            "Timeout 100ms exceeded.",
        );
    });
}

#[test]
fn element_handle_wait_for_selector_should_throw_on_navigation() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let div = t.page.query_selector("div").await.unwrap().unwrap();
        let task = tokio::spawn(div.wait_for_selector("span").into_future());
        for _ in 0..10 {
            t.page.evaluate::<i32>("() => 1").await.unwrap();
        }
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert_err_contains(
            task.await.unwrap(),
            "waiting for locator('span') to be visible",
        );
    });
}

#[test]
fn should_work_with_removed_mutation_observer() {
    run(|t| async move {
        t.page
            .evaluate::<()>("() => { delete window.MutationObserver; }")
            .await
            .unwrap();
        let task = tokio::spawn(t.page.wait_for_selector(".zombo").into_future());
        t.page
            .set_content("<div class='zombo'>anything</div>")
            .await
            .unwrap();
        let handle = task.await.unwrap().unwrap().unwrap();
        let text: String = t
            .page
            .evaluate("x => x.textContent")
            .arg(&handle)
            .await
            .unwrap();
        assert_eq!(text, "anything");
    });
}

#[test]
fn should_resolve_promise_when_node_is_added() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let frame = t.page.main_frame();
        let task = tokio::spawn(
            frame
                .wait_for_selector("div")
                .state(WaitForSelectorState::Attached)
                .into_future(),
        );
        frame.evaluate::<()>(ADD_ELEMENT).arg("br").await.unwrap();
        frame.evaluate::<()>(ADD_ELEMENT).arg("div").await.unwrap();
        let handle = task.await.unwrap().unwrap().unwrap();
        let tag: String = handle
            .get_property("tagName")
            .await
            .unwrap()
            .json_value()
            .await
            .unwrap();
        assert_eq!(tag, "DIV");
    });
}

#[test]
fn should_report_logs_while_waiting_for_visible() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let frame = t.page.main_frame();
        // Make sure the injected script exists before the first poll.
        frame.query_selector("body").await.unwrap();
        let task = tokio::spawn(frame.wait_for_selector("div").timeout(5000).into_future());
        frame
            .evaluate::<()>(
                "() => {
                const div = document.createElement('div');
                div.className = 'foo bar';
                div.id = 'mydiv';
                div.setAttribute('style', 'display: none');
                div.setAttribute('foo', '123456789012345678901234567890123456789012345678901234567890');
                div.textContent = 'abcdefghijklmnopqrstuvwyxzabcdefghijklmnopqrstuvwyxzabcdefghijklmnopqrstuvwyxz';
                document.body.appendChild(div);
              }",
            )
            .await
            .unwrap();
        rafraf_frame(&frame, 2).await;
        // Hold each state across a full poll interval (up to 500ms): two
        // frames are not enough for a poll to see it on a loaded machine.
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        frame
            .evaluate::<()>("() => document.querySelector('div').remove()")
            .await
            .unwrap();
        rafraf_frame(&frame, 2).await;
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        frame
            .evaluate::<()>(
                "() => { const div = document.createElement('div'); div.className = 'another'; div.style.display = 'none'; document.body.appendChild(div); }",
            )
            .await
            .unwrap();
        rafraf_frame(&frame, 2).await;
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        let err = task.await.unwrap().unwrap_err().to_string();
        assert!(
            err.contains("frame.waitForSelector: Timeout 5000ms exceeded."),
            "{err}"
        );
        assert!(
            err.contains("waiting for locator('div') to be visible"),
            "{err}"
        );
        assert!(
            err.contains("locator resolved to hidden <div id=\"mydiv\" class=\"foo bar\" foo=\"123456789012345678901234567890123456789012345678901234567890\">abcdefghijklmnopqrstuvwyxzabcdefghijklmnopqrstuvw…</div>"),
            "{err}"
        );
        assert!(
            err.contains("locator resolved to hidden <div class=\"another\"></div>"),
            "{err}"
        );
    });
}

#[test]
fn should_report_logs_while_waiting_for_hidden() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let frame = t.page.main_frame();
        // Make sure the injected script exists before the first poll.
        frame.query_selector("body").await.unwrap();
        frame
            .evaluate::<()>(
                "() => { const div = document.createElement('div'); div.className = 'foo bar'; div.id = 'mydiv'; div.textContent = 'hello'; document.body.appendChild(div); }",
            )
            .await
            .unwrap();
        let task = tokio::spawn(
            frame
                .wait_for_selector("div")
                .state(WaitForSelectorState::Hidden)
                .timeout(5000)
                .into_future(),
        );
        rafraf_frame(&frame, 2).await;
        frame
            .evaluate::<()>(
                "() => { document.querySelector('div').remove(); const div = document.createElement('div'); div.className = 'another'; div.textContent = 'hello'; document.body.appendChild(div); }",
            )
            .await
            .unwrap();
        rafraf_frame(&frame, 2).await;
        let err = task.await.unwrap().unwrap_err().to_string();
        assert!(
            err.contains("frame.waitForSelector: Timeout 5000ms exceeded."),
            "{err}"
        );
        assert!(
            err.contains("waiting for locator('div') to be hidden"),
            "{err}"
        );
        assert!(
            err.contains(
                "locator resolved to visible <div id=\"mydiv\" class=\"foo bar\">hello</div>"
            ),
            "{err}"
        );
        assert!(
            err.contains("locator resolved to visible <div class=\"another\">hello</div>"),
            "{err}"
        );
    });
}

#[test]
fn should_report_logs_when_the_selector_resolves_to_multiple_elements() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .set_content(
                "<button style=\"display: none; position: absolute; top: 0px; left: 0px; width: 100%;\">Reset</button>
                <button>Reset</button>",
            )
            .await
            .unwrap();
        assert_err_contains(
            t.page.click("text=Reset").timeout(5000).await,
            "locator resolved to 2 elements. Proceeding with the first one: <button>Reset</button>",
        );
    });
}

#[test]
fn should_resolve_promise_when_node_is_added_in_shadow_dom() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let task = tokio::spawn(t.page.wait_for_selector("span").into_future());
        t.page
            .evaluate::<()>(
                "() => { const div = document.createElement('div'); div.attachShadow({ mode: 'open' }); document.body.appendChild(div); }",
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        t.page
            .evaluate::<()>(
                "() => { const span = document.createElement('span'); span.textContent = 'Hello from shadow'; document.querySelector('div').shadowRoot.appendChild(span); }",
            )
            .await
            .unwrap();
        let handle = task.await.unwrap().unwrap().unwrap();
        assert_eq!(
            handle
                .evaluate::<String>("e => e.textContent")
                .await
                .unwrap(),
            "Hello from shadow"
        );
    });
}

#[test]
fn should_work_when_node_is_added_through_inner_html() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let task = tokio::spawn(
            t.page
                .wait_for_selector("h3 div")
                .state(WaitForSelectorState::Attached)
                .into_future(),
        );
        t.page
            .evaluate::<()>(ADD_ELEMENT)
            .arg("span")
            .await
            .unwrap();
        t.page
            .evaluate::<()>(
                "() => { document.querySelector('span').innerHTML = '<h3><div></div></h3>'; }",
            )
            .await
            .unwrap();
        task.await.unwrap().unwrap();
    });
}

#[test]
fn page_wait_for_selector_is_shortcut_for_main_frame() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        let other = t.page.frames()[1].clone();
        let task = tokio::spawn(
            t.page
                .wait_for_selector("div")
                .state(WaitForSelectorState::Attached)
                .into_future(),
        );
        other.evaluate::<()>(ADD_ELEMENT).arg("div").await.unwrap();
        t.page.evaluate::<()>(ADD_ELEMENT).arg("div").await.unwrap();
        let handle = task.await.unwrap().unwrap().unwrap();
        assert_eq!(
            handle.owner_frame().await.unwrap(),
            Some(t.page.main_frame())
        );
    });
}

#[test]
fn should_run_in_specified_frame() {
    run(|t| async move {
        attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        attach_frame(&t.page, "frame2", &t.server.empty_page).await;
        let frame1 = t.page.frames()[1].clone();
        let frame2 = t.page.frames()[2].clone();
        let task = tokio::spawn(
            frame2
                .wait_for_selector("div")
                .state(WaitForSelectorState::Attached)
                .into_future(),
        );
        frame1.evaluate::<()>(ADD_ELEMENT).arg("div").await.unwrap();
        frame2.evaluate::<()>(ADD_ELEMENT).arg("div").await.unwrap();
        let handle = task.await.unwrap().unwrap().unwrap();
        assert_eq!(handle.owner_frame().await.unwrap(), Some(frame2));
    });
}

#[test]
fn should_throw_when_frame_is_detached() {
    run(|t| async move {
        attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        let frame = t.page.frames()[1].clone();
        let task = tokio::spawn(frame.wait_for_selector(".box").into_future());
        detach_frame(&t.page, "frame1").await;
        assert_err_contains(
            task.await.unwrap(),
            "frame.waitForSelector: Frame was detached",
        );
    });
}

// ---- page-wait-for-selector-2.spec.ts ----

#[test]
fn should_survive_cross_process_navigation() {
    run(|t| async move {
        let task = tokio::spawn(t.page.wait_for_selector(".box").into_future());
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert!(!task.is_finished());
        t.page.reload().await.unwrap();
        assert!(!task.is_finished());
        t.page
            .goto(format!("{}/grid.html", t.server.cross_process_prefix))
            .await
            .unwrap();
        task.await.unwrap().unwrap();
    });
}

#[test]
fn should_wait_for_visible() {
    run(|t| async move {
        let task = tokio::spawn(t.page.wait_for_selector("div").into_future());
        t.page
            .set_content("<div style='display: none; visibility: hidden;'>1</div>")
            .await
            .unwrap();
        assert!(!task.is_finished());
        t.page
            .evaluate::<()>("() => document.querySelector('div').style.removeProperty('display')")
            .await
            .unwrap();
        assert!(!task.is_finished());
        t.page
            .evaluate::<()>(
                "() => document.querySelector('div').style.removeProperty('visibility')",
            )
            .await
            .unwrap();
        assert!(task.await.unwrap().unwrap().is_some());
    });
}

#[test]
fn should_not_consider_visible_when_zero_sized() {
    run(|t| async move {
        t.page
            .set_content("<div style='width: 0; height: 0;'>1</div>")
            .await
            .unwrap();
        assert_err_contains(
            t.page.wait_for_selector("div").timeout(1000).await,
            "page.waitForSelector: Timeout 1000ms exceeded",
        );
        t.page
            .evaluate::<()>("() => { document.querySelector('div').style.width = '10px'; }")
            .await
            .unwrap();
        assert_err_contains(
            t.page.wait_for_selector("div").timeout(1000).await,
            "page.waitForSelector: Timeout 1000ms exceeded",
        );
        t.page
            .evaluate::<()>("() => { document.querySelector('div').style.height = '10px'; }")
            .await
            .unwrap();
        assert!(
            t.page
                .wait_for_selector("div")
                .timeout(1000)
                .await
                .unwrap()
                .is_some()
        );
    });
}

#[test]
fn should_wait_for_visible_recursively() {
    run(|t| async move {
        let task = tokio::spawn(t.page.wait_for_selector("div#inner").into_future());
        t.page
            .set_content(
                "<div style='display: none; visibility: hidden;'><div id=\"inner\">hi</div></div>",
            )
            .await
            .unwrap();
        assert!(!task.is_finished());
        t.page
            .evaluate::<()>("() => document.querySelector('div').style.removeProperty('display')")
            .await
            .unwrap();
        assert!(!task.is_finished());
        t.page
            .evaluate::<()>(
                "() => document.querySelector('div').style.removeProperty('visibility')",
            )
            .await
            .unwrap();
        assert!(task.await.unwrap().unwrap().is_some());
    });
}

#[test]
fn should_consider_outside_of_viewport_visible() {
    run(|t| async move {
        t.page
            .set_content(
                "<style>
                  .cover { position: fixed; left: 0; top: 0; width: 100px; height: 100px; background-color: red; transform: translateX(-200px); }
                </style>
                <div class=\"cover\">cover</div>",
            )
            .await
            .unwrap();
        let cover = t.page.locator(".cover");
        cover
            .wait_for()
            .state(WaitForSelectorState::Visible)
            .await
            .unwrap();
        assert!(cover.is_visible().await.unwrap());
    });
}

async fn wait_hidden_after(page: &oxidewright::Page, mutation: &str) {
    let task = tokio::spawn(
        page.wait_for_selector("div")
            .state(WaitForSelectorState::Hidden)
            .into_future(),
    );
    page.wait_for_selector("div").await.unwrap(); // do a round trip
    assert!(!task.is_finished());
    page.evaluate::<()>(mutation).await.unwrap();
    assert!(task.await.unwrap().unwrap().is_none());
}

#[test]
fn hidden_should_wait_for_hidden() {
    run(|t| async move {
        t.page
            .set_content("<div style='display: block;'>content</div>")
            .await
            .unwrap();
        wait_hidden_after(
            &t.page,
            "() => document.querySelector('div').style.setProperty('visibility', 'hidden')",
        )
        .await;
    });
}

#[test]
fn hidden_should_wait_for_display_none() {
    run(|t| async move {
        t.page
            .set_content("<div style='display: block;'>content</div>")
            .await
            .unwrap();
        wait_hidden_after(
            &t.page,
            "() => document.querySelector('div').style.setProperty('display', 'none')",
        )
        .await;
    });
}

#[test]
fn hidden_should_wait_for_removal() {
    run(|t| async move {
        t.page.set_content("<div>content</div>").await.unwrap();
        wait_hidden_after(&t.page, "() => document.querySelector('div').remove()").await;
    });
}

#[test]
fn should_return_null_if_waiting_to_hide_non_existing_element() {
    run(|t| async move {
        let handle = t
            .page
            .wait_for_selector("non-existing")
            .state(WaitForSelectorState::Hidden)
            .await
            .unwrap();
        assert!(handle.is_none());
    });
}

#[test]
fn should_respect_timeout() {
    run(|t| async move {
        let err = assert_err_contains(
            t.page
                .wait_for_selector("div")
                .timeout(3000)
                .state(WaitForSelectorState::Attached)
                .await,
            "page.waitForSelector: Timeout 3000ms exceeded",
        );
        assert!(err.to_string().contains("waiting for locator('div')"));
        assert!(err.is_timeout());
    });
}

#[test]
fn should_have_an_error_message_specifically_for_awaiting_an_element_to_be_hidden() {
    run(|t| async move {
        t.page.set_content("<div>content</div>").await.unwrap();
        let err = assert_err_contains(
            t.page
                .wait_for_selector("div")
                .state(WaitForSelectorState::Hidden)
                .timeout(1000)
                .await,
            "page.waitForSelector: Timeout 1000ms exceeded",
        );
        assert!(
            err.to_string()
                .contains("waiting for locator('div') to be hidden")
        );
    });
}

#[test]
fn should_respond_to_node_attribute_mutation() {
    run(|t| async move {
        let task = tokio::spawn(
            t.page
                .wait_for_selector(".zombo")
                .state(WaitForSelectorState::Attached)
                .into_future(),
        );
        t.page
            .set_content("<div class='notZombo'></div>")
            .await
            .unwrap();
        assert!(!task.is_finished());
        t.page
            .evaluate::<()>("() => { document.querySelector('div').className = 'zombo'; }")
            .await
            .unwrap();
        assert!(task.await.unwrap().unwrap().is_some());
    });
}

#[test]
fn should_return_the_element_handle() {
    run(|t| async move {
        let task = tokio::spawn(t.page.wait_for_selector(".zombo").into_future());
        t.page
            .set_content("<div class='zombo'>anything</div>")
            .await
            .unwrap();
        let handle = task.await.unwrap().unwrap().unwrap();
        let text: String = t
            .page
            .evaluate("x => x.textContent")
            .arg(&handle)
            .await
            .unwrap();
        assert_eq!(text, "anything");
    });
}

#[test]
fn should_throw_for_unknown_state_option() {
    let err = "foo".parse::<WaitForSelectorState>().unwrap_err();
    assert!(
        err.to_string()
            .contains("state: expected one of (attached|detached|visible|hidden)")
    );
}

#[test]
fn should_support_gt_gt_selector_syntax() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let frame = t.page.main_frame();
        let task = tokio::spawn(
            frame
                .wait_for_selector("css=div >> css=span")
                .state(WaitForSelectorState::Attached)
                .into_future(),
        );
        frame.evaluate::<()>(ADD_ELEMENT).arg("br").await.unwrap();
        frame.evaluate::<()>(ADD_ELEMENT).arg("div").await.unwrap();
        frame
            .evaluate::<()>(
                "() => { document.querySelector('div').appendChild(document.createElement('span')); }",
            )
            .await
            .unwrap();
        let handle = task.await.unwrap().unwrap().unwrap();
        let tag: String = handle
            .get_property("tagName")
            .await
            .unwrap()
            .json_value()
            .await
            .unwrap();
        assert_eq!(tag, "SPAN");
    });
}

#[test]
fn should_wait_for_detached_if_already_detached() {
    run(|t| async move {
        t.page
            .set_content("<section id=\"testAttribute\">43543</section>")
            .await
            .unwrap();
        assert!(
            t.page
                .wait_for_selector("css=div")
                .state(WaitForSelectorState::Detached)
                .await
                .unwrap()
                .is_none()
        );
    });
}

#[test]
fn should_wait_for_detached() {
    run(|t| async move {
        t.page
            .set_content("<section id=\"testAttribute\"><div>43543</div></section>")
            .await
            .unwrap();
        let task = tokio::spawn(
            t.page
                .wait_for_selector("css=div")
                .state(WaitForSelectorState::Detached)
                .into_future(),
        );
        assert!(!task.is_finished());
        t.page.wait_for_selector("css=section").await.unwrap();
        assert!(!task.is_finished());
        t.page
            .eval_on_selector::<()>("div", "div => div.remove()")
            .await
            .unwrap();
        assert!(task.await.unwrap().unwrap().is_none());
    });
}

#[test]
fn should_support_some_fancy_xpath() {
    run(|t| async move {
        t.page
            .set_content("<p>red herring</p><p>hello  world  </p>")
            .await
            .unwrap();
        let handle = t
            .page
            .wait_for_selector("//p[normalize-space(.)=\"hello world\"]")
            .await
            .unwrap()
            .unwrap();
        let text: String = t
            .page
            .evaluate("x => x.textContent")
            .arg(&handle)
            .await
            .unwrap();
        assert_eq!(text, "hello  world  ");
    });
}

#[test]
fn should_respect_timeout_xpath() {
    run(|t| async move {
        let err = assert_err_contains(
            t.page
                .wait_for_selector("//div")
                .state(WaitForSelectorState::Attached)
                .timeout(3000)
                .await,
            "page.waitForSelector: Timeout 3000ms exceeded",
        );
        assert!(err.to_string().contains("waiting for locator('//div')"));
        assert!(err.is_timeout());
    });
}

#[test]
fn should_run_in_specified_frame_xpath() {
    run(|t| async move {
        attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        attach_frame(&t.page, "frame2", &t.server.empty_page).await;
        let frame1 = t.page.frames()[1].clone();
        let frame2 = t.page.frames()[2].clone();
        let task = tokio::spawn(
            frame2
                .wait_for_selector("//div")
                .state(WaitForSelectorState::Attached)
                .into_future(),
        );
        frame1.evaluate::<()>(ADD_ELEMENT).arg("div").await.unwrap();
        frame2.evaluate::<()>(ADD_ELEMENT).arg("div").await.unwrap();
        let handle = task.await.unwrap().unwrap().unwrap();
        assert_eq!(handle.owner_frame().await.unwrap(), Some(frame2));
    });
}

#[test]
fn should_throw_when_frame_is_detached_xpath() {
    run(|t| async move {
        attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        let frame = t.page.frames()[1].clone();
        let task = tokio::spawn(frame.wait_for_selector("//*[@class=\"box\"]").into_future());
        detach_frame(&t.page, "frame1").await;
        assert_err_contains(
            task.await.unwrap(),
            "frame.waitForSelector: Frame was detached",
        );
    });
}

#[test]
fn should_return_the_element_handle_xpath() {
    run(|t| async move {
        let task = tokio::spawn(
            t.page
                .wait_for_selector("//*[@class=\"zombo\"]")
                .into_future(),
        );
        t.page
            .set_content("<div class='zombo'>anything</div>")
            .await
            .unwrap();
        let handle = task.await.unwrap().unwrap().unwrap();
        let text: String = t
            .page
            .evaluate("x => x.textContent")
            .arg(&handle)
            .await
            .unwrap();
        assert_eq!(text, "anything");
    });
}

#[test]
fn should_allow_you_to_select_an_element_with_single_slash_xpath() {
    run(|t| async move {
        t.page.set_content("<div>some text</div>").await.unwrap();
        let handle = t
            .page
            .wait_for_selector("//html/body/div")
            .await
            .unwrap()
            .unwrap();
        let text: String = t
            .page
            .evaluate("x => x.textContent")
            .arg(&handle)
            .await
            .unwrap();
        assert_eq!(text, "some text");
    });
}

#[test]
fn should_correctly_handle_hidden_shadow_host() {
    run(|t| async move {
        t.page
            .set_content(
                "<x-host hidden></x-host>
                <script>
                  const host = document.querySelector('x-host');
                  const root = host.attachShadow({ mode: 'open' });
                  const style = document.createElement('style');
                  style.textContent = ':host([hidden]) { display: none; }';
                  root.appendChild(style);
                  const child = document.createElement('div');
                  child.textContent = 'Find me';
                  root.appendChild(child);
                </script>",
            )
            .await
            .unwrap();
        assert_eq!(
            t.page.text_content("div").await.unwrap().as_deref(),
            Some("Find me")
        );
        t.page
            .wait_for_selector("div")
            .state(WaitForSelectorState::Hidden)
            .await
            .unwrap();
    });
}

#[test]
fn should_fail_if_element_handle_was_detached_while_waiting() {
    run(|t| async move {
        t.page.set_content("<button>hello</button>").await.unwrap();
        let button = t.page.query_selector("button").await.unwrap().unwrap();
        let task = tokio::spawn(button.wait_for_selector("something").into_future());
        tokio::time::sleep(Duration::from_millis(100)).await;
        t.page
            .evaluate::<()>("() => { document.body.innerText = ''; }")
            .await
            .unwrap();
        assert_err_contains(task.await.unwrap(), "Element is not attached to the DOM");
    });
}

#[test]
fn should_succeed_if_element_handle_was_detached_while_waiting_for_hidden() {
    run(|t| async move {
        t.page.set_content("<button>hello</button>").await.unwrap();
        let button = t.page.query_selector("button").await.unwrap().unwrap();
        let task = tokio::spawn(
            button
                .wait_for_selector("something")
                .state(WaitForSelectorState::Hidden)
                .into_future(),
        );
        tokio::time::sleep(Duration::from_millis(1000)).await;
        t.page
            .evaluate::<()>("() => { document.body.innerText = ''; }")
            .await
            .unwrap();
        task.await.unwrap().unwrap();
    });
}

#[test]
fn should_succeed_if_element_handle_was_detached_while_waiting_for_detached() {
    run(|t| async move {
        t.page.set_content("<button>hello</button>").await.unwrap();
        let button = t.page.query_selector("button").await.unwrap().unwrap();
        let task = tokio::spawn(
            button
                .wait_for_selector("something")
                .state(WaitForSelectorState::Detached)
                .into_future(),
        );
        tokio::time::sleep(Duration::from_millis(1000)).await;
        t.page
            .evaluate::<()>("() => { document.body.innerText = ''; }")
            .await
            .unwrap();
        task.await.unwrap().unwrap();
    });
}
