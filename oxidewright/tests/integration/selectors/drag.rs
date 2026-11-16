//! Port of `page-drag.spec.ts` (Chromium expectations). Uses
//! `drag-n-drop-positioned.html`, laid out so that the source and target
//! centers are where Playwright's `drag-n-drop.html` puts them.
//!
//! Skipped:
//! - DD-006, DD-007 (drag into/out of an iframe): `fixme` in Playwright.

use std::future::IntoFuture;

use oxidewright::{ElementHandle, JSHandle, Page, Position};
use serde_json::{Value, json};

use super::*;
use crate::common::*;

async fn track_events(target: &ElementHandle) -> JSHandle {
    target
        .evaluate_handle(
            r#"target => {
    const events = [];
    for (const event of [
      'mousedown', 'mousemove', 'mouseup',
      'dragstart', 'dragend', 'dragover', 'dragenter', 'dragleave', 'dragexit',
      'drop'
    ]) {
      target.addEventListener(event, e => {
        if (event === 'dragend')
          events.push('dragend');
        else
          events.push(`${event} at ${e.clientX};${e.clientY}`);
      }, false);
    }
    return events;
  }"#,
        )
        .await
        .unwrap()
}

async fn body(page: &Page) -> ElementHandle {
    page.query_selector("body").await.unwrap().unwrap()
}

fn dnd(t: &TestContext) -> String {
    format!("{}/drag-n-drop-positioned.html", t.server.prefix)
}

async fn source_in_target(page: &Page) -> bool {
    page.eval_on_selector::<bool>(
        "#target",
        "target => target.contains(document.querySelector('#source'))",
    )
    .await
    .unwrap()
}

async fn drag_manually(page: &Page) {
    page.hover("#source").await.unwrap();
    page.mouse().down().await.unwrap();
    page.hover("#target").await.unwrap();
    page.mouse().up().await.unwrap();
}

const FULL_DRAG: [&str; 8] = [
    "mousemove at 120;86",
    "mousedown at 120;86",
    "mousemove at 240;350",
    "dragstart at 120;86",
    "dragenter at 240;350",
    "dragover at 240;350",
    "drop at 240;350",
    "dragend",
];

#[test]
fn should_work() {
    run(|t| async move {
        t.page.goto(dnd(&t)).await.unwrap();
        drag_manually(&t.page).await;
        assert!(source_in_target(&t.page).await);
    });
}

#[test]
fn should_send_the_right_events() {
    run(|t| async move {
        t.page.goto(dnd(&t)).await.unwrap();
        let events = track_events(&body(&t.page).await).await;
        drag_manually(&t.page).await;
        assert_eq!(events.json_value::<Vec<String>>().await.unwrap(), FULL_DRAG);
    });
}

#[test]
fn should_not_send_dragover_on_the_first_mousemove() {
    run(|t| async move {
        t.page.goto(dnd(&t)).await.unwrap();
        let events = track_events(&body(&t.page).await).await;
        t.page.hover("#source").await.unwrap();
        t.page.mouse().down().await.unwrap();
        t.page.hover("#target").await.unwrap();
        assert_eq!(
            events.json_value::<Vec<String>>().await.unwrap(),
            FULL_DRAG[..5]
        );
    });
}

#[test]
fn should_work_inside_iframe() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let frame = attach_frame(&t.page, "myframe", &dnd(&t)).await;
        t.page
            .eval_on_selector::<()>(
                "iframe",
                "iframe => { iframe.style.width = '500px'; iframe.style.height = '600px'; iframe.style.marginLeft = '80px'; iframe.style.marginTop = '60px'; }",
            )
            .await
            .unwrap();
        let page_events = track_events(&body(&t.page).await).await;
        let frame_body = frame.query_selector("body").await.unwrap().unwrap();
        let frame_events = track_events(&frame_body).await;
        frame.hover("#source").await.unwrap();
        t.page.mouse().down().await.unwrap();
        frame.hover("#target").await.unwrap();
        t.page.mouse().up().await.unwrap();
        let inside: bool = frame
            .eval_on_selector(
                "#target",
                "target => target.contains(document.querySelector('#source'))",
            )
            .await
            .unwrap();
        assert!(inside);
        assert_eq!(
            frame_events.json_value::<Vec<String>>().await.unwrap(),
            FULL_DRAG
        );
        assert!(
            page_events
                .json_value::<Vec<String>>()
                .await
                .unwrap()
                .is_empty()
        );
    });
}

#[test]
fn should_cancel_on_escape() {
    run(|t| async move {
        t.page.goto(dnd(&t)).await.unwrap();
        let events = track_events(&body(&t.page).await).await;
        t.page.hover("#source").await.unwrap();
        t.page.mouse().down().await.unwrap();
        t.page.hover("#target").await.unwrap();
        t.page.keyboard().press("Escape").await.unwrap();
        t.page.mouse().up().await.unwrap();
        assert!(!source_in_target(&t.page).await);
        assert_eq!(
            events.json_value::<Vec<String>>().await.unwrap(),
            [
                "mousemove at 120;86",
                "mousedown at 120;86",
                "mousemove at 240;350",
                "dragstart at 120;86",
                "dragenter at 240;350",
                "dragend",
                "mouseup at 240;350",
            ]
        );
    });
}

async fn test_if_dropped(page: &Page, effect_allowed: &str, drop_effect: &str) -> bool {
    page.set_content(
        "<div draggable=\"true\">drag target</div>\n<drop-target>this is the drop target</drop-target>",
    )
    .await
    .unwrap();
    page.evaluate::<()>(
        r#"({ effectAllowed, dropEffect }) => {
    window['dropped'] = false;
    document.querySelector('div').addEventListener('dragstart', event => {
      event.dataTransfer.effectAllowed = effectAllowed;
      event.dataTransfer.setData('text/plain', 'drag data');
    });
    const dropTarget = document.querySelector('drop-target');
    dropTarget.addEventListener('dragover', event => {
      event.dataTransfer.dropEffect = dropEffect;
      event.preventDefault();
    });
    dropTarget.addEventListener('drop', event => {
      window['dropped'] = true;
    });
  }"#,
    )
    .arg(json!({ "effectAllowed": effect_allowed, "dropEffect": drop_effect }))
    .await
    .unwrap();
    page.hover("div").await.unwrap();
    page.mouse().down().await.unwrap();
    page.hover("drop-target").await.unwrap();
    page.mouse().up().await.unwrap();
    page.evaluate("dropped").await.unwrap()
}

#[test]
fn should_respect_the_drop_effect() {
    run(|t| async move {
        let p = &t.page;
        for (allowed, effect, expected) in [
            ("copy", "copy", true),
            ("copy", "move", false),
            ("all", "link", true),
            ("all", "none", false),
            ("copyMove", "copy", true),
            ("copyLink", "copy", true),
            ("linkMove", "copy", false),
            ("copyMove", "link", false),
            ("copyLink", "link", true),
            ("linkMove", "link", true),
            ("copyMove", "move", true),
            ("copyLink", "move", false),
            ("linkMove", "move", true),
            ("uninitialized", "copy", true),
        ] {
            assert_eq!(
                test_if_dropped(p, allowed, effect).await,
                expected,
                "{allowed} / {effect}"
            );
        }
    });
}

#[test]
fn should_report_the_default_drop_effect_from_effect_allowed() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            "<div draggable=\"true\">drag target</div>\n<drop-target>this is the drop target</drop-target>",
        )
        .await
        .unwrap();
        p.evaluate::<()>(
            r#"() => {
    window['dropEffect'] = 'none';
    document.querySelector('div').addEventListener('dragstart', event => {
      event.dataTransfer.effectAllowed = 'copyMove';
      event.dataTransfer.setData('text/plain', 'drag data');
    });
    document.querySelector('div').addEventListener('dragend', event => {
      window['dropEffect'] = event.dataTransfer.dropEffect;
    });
    document.querySelector('drop-target').addEventListener('dragover', event => {
      event.preventDefault();
    });
  }"#,
        )
        .await
        .unwrap();
        p.hover("div").await.unwrap();
        p.mouse().down().await.unwrap();
        p.hover("drop-target").await.unwrap();
        p.mouse().up().await.unwrap();
        assert_eq!(p.evaluate::<String>("dropEffect").await.unwrap(), "move");
    });
}

#[test]
fn should_work_if_the_drag_is_canceled() {
    run(|t| async move {
        t.page.goto(dnd(&t)).await.unwrap();
        t.page
            .evaluate::<()>(
                "() => { document.body.addEventListener('dragstart', event => { event.preventDefault(); }, false); }",
            )
            .await
            .unwrap();
        drag_manually(&t.page).await;
        assert!(!source_in_target(&t.page).await);
    });
}

#[test]
fn should_work_if_the_drag_event_is_captured_but_not_canceled() {
    run(|t| async move {
        t.page.goto(dnd(&t)).await.unwrap();
        t.page
            .evaluate::<()>(
                "() => { document.body.addEventListener('dragstart', event => { event.stopImmediatePropagation(); }, false); }",
            )
            .await
            .unwrap();
        drag_manually(&t.page).await;
        assert!(source_in_target(&t.page).await);
    });
}

#[test]
fn should_be_able_to_drag_the_mouse_in_a_frame() {
    run(|t| async move {
        t.page
            .goto(format!("{}/frames/one-frame.html", t.server.prefix))
            .await
            .unwrap();
        let frame = t.page.frames()[1].clone();
        let html = frame.query_selector("html").await.unwrap().unwrap();
        let events = track_events(&html).await;
        let mouse = t.page.mouse();
        mouse.r#move(30, 30).await.unwrap();
        mouse.down().await.unwrap();
        mouse.r#move(60, 60).await.unwrap();
        mouse.up().await.unwrap();
        assert_eq!(
            events.json_value::<Vec<String>>().await.unwrap(),
            [
                "mousemove at 20;20",
                "mousedown at 20;20",
                "mousemove at 50;50",
                "mouseup at 50;50"
            ]
        );
    });
}

#[test]
fn should_work_if_a_frame_is_stalled() {
    run(|t| async move {
        t.page.goto(dnd(&t)).await.unwrap();
        t.server.hang("/stalled.html");
        t.page
            .evaluate::<()>(
                "() => { const frame = document.createElement('iframe'); frame.src = '/stalled.html'; document.body.appendChild(frame); }",
            )
            .await
            .unwrap();
        t.server.wait_for_request("/stalled.html").await;
        drag_manually(&t.page).await;
        assert!(source_in_target(&t.page).await);
    });
}

#[test]
fn should_work_with_the_helper_method() {
    run(|t| async move {
        t.page.goto(dnd(&t)).await.unwrap();
        t.page.drag_and_drop("#source", "#target").await.unwrap();
        assert!(source_in_target(&t.page).await);
    });
}

const TWEEN_PAGE: &str = r#"
        <body style="margin: 0; padding: 0;">
          <div style="width:100px;height:100px;background:red;" id="red"></div>
          <div style="width:300px;height:100px;background:blue;" id="blue"></div>
        </body>
      "#;

const TRACK_PAGE_MOUSE: &str = r#"() => {
    const events = [];
    for (const type of ['mousedown', 'mouseup', 'mousemove'])
      document.addEventListener(type, event => events.push({ type, x: event.pageX, y: event.pageY }));
    return events;
  }"#;

fn tween_expected() -> Value {
    json!([
        { "type": "mousemove", "x": 50, "y": 50 },
        { "type": "mousedown", "x": 50, "y": 50 },
        { "type": "mousemove", "x": 75, "y": 75 },
        { "type": "mousemove", "x": 100, "y": 100 },
        { "type": "mousemove", "x": 125, "y": 125 },
        { "type": "mousemove", "x": 150, "y": 150 },
        { "type": "mouseup", "x": 150, "y": 150 },
    ])
}

async fn tween(page: &Page, use_locator: bool) {
    page.set_content(TWEEN_PAGE).await.unwrap();
    let events = page.evaluate_handle(TRACK_PAGE_MOUSE).await.unwrap();
    if use_locator {
        page.locator("#red")
            .drag_to(&page.locator("#blue"))
            .steps(4)
            .await
            .unwrap();
    } else {
        page.drag_and_drop("#red", "#blue").steps(4).await.unwrap();
    }
    poll_eq(tween_expected(), move || {
        let events = events.clone();
        async move { events.json_value::<Value>().await.unwrap() }
    })
    .await;
}

#[test]
fn should_drag_and_drop_with_tweened_mouse_movement() {
    run(|t| async move {
        tween(&t.page, false).await;
    });
}

#[test]
fn should_drag_to_with_tweened_mouse_movement() {
    run(|t| async move {
        tween(&t.page, true).await;
    });
}

#[test]
fn should_allow_specifying_the_position() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"<div style="width:100px;height:100px;background:red;" id="red"></div>
      <div style="width:100px;height:100px;background:blue;" id="blue"></div>"#,
        )
        .await
        .unwrap();
        let events = p
            .evaluate_handle(
                r#"() => {
    const events = [];
    document.getElementById('red').addEventListener('mousedown', event => {
      events.push({ type: 'mousedown', x: event.offsetX, y: event.offsetY });
    });
    document.getElementById('blue').addEventListener('mouseup', event => {
      events.push({ type: 'mouseup', x: event.offsetX, y: event.offsetY });
    });
    return events;
  }"#,
            )
            .await
            .unwrap();
        p.drag_and_drop("#red", "#blue")
            .source_position(Position::new(34.0, 7.0))
            .target_position(Position::new(10.0, 20.0))
            .await
            .unwrap();
        assert_eq!(
            events.json_value::<Value>().await.unwrap(),
            json!([
                { "type": "mousedown", "x": 34, "y": 7 },
                { "type": "mouseup", "x": 10, "y": 20 },
            ])
        );
    });
}

#[test]
fn should_work_with_locators() {
    run(|t| async move {
        t.page.goto(dnd(&t)).await.unwrap();
        t.page
            .locator("#source")
            .drag_to(&t.page.locator("#target"))
            .await
            .unwrap();
        assert!(source_in_target(&t.page).await);
    });
}

#[test]
fn should_work_if_not_doing_a_drag() {
    run(|t| async move {
        let html = t.page.query_selector("html").await.unwrap().unwrap();
        let events = track_events(&html).await;
        let mouse = t.page.mouse();
        mouse.r#move(50, 50).await.unwrap();
        mouse.down().await.unwrap();
        mouse.r#move(100, 100).await.unwrap();
        mouse.up().await.unwrap();
        assert_eq!(
            events.json_value::<Vec<String>>().await.unwrap(),
            [
                "mousemove at 50;50",
                "mousedown at 50;50",
                "mousemove at 100;100",
                "mouseup at 100;100"
            ]
        );
    });
}

#[test]
fn should_report_event_buttons() {
    run(|t| async move {
        let logs = t
            .page
            .evaluate_handle(
                r#"async () => {
    const div = document.createElement('div');
    document.body.appendChild(div);
    div.style.width = '200px';
    div.style.height = '200px';
    div.style.backgroundColor = 'blue';
    div.addEventListener('mousedown', onEvent);
    div.addEventListener('mousemove', onEvent, { passive: false });
    div.addEventListener('mouseup', onEvent);
    const logs = [];
    function onEvent(event) {
      logs.push({ type: event.type, buttons: event.buttons });
    }
    await new Promise(requestAnimationFrame);
    return logs;
  }"#,
            )
            .await
            .unwrap();
        let mouse = t.page.mouse();
        mouse.r#move(20, 20).await.unwrap();
        mouse.down().await.unwrap();
        mouse.r#move(40, 40).await.unwrap();
        mouse.up().await.unwrap();
        assert_eq!(
            logs.json_value::<Value>().await.unwrap(),
            json!([
                { "type": "mousemove", "buttons": 0 },
                { "type": "mousedown", "buttons": 1 },
                { "type": "mousemove", "buttons": 1 },
                { "type": "mouseup", "buttons": 0 },
            ])
        );
    });
}

#[test]
fn should_handle_custom_data_transfer() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(r#"<button draggable="true">Draggable</button>"#)
            .await
            .unwrap();
        p.evaluate::<()>(
            r#"() => {
    window.__result = new Promise(resolve => {
      document.addEventListener('dragstart', event => {
        event.dataTransfer.setData('custom-type', 'Hello World');
      }, false);
      document.addEventListener('dragenter', event => {
        event.preventDefault();
      }, false);
      document.addEventListener('dragover', event => {
        event.preventDefault();
      }, false);
      document.addEventListener('drop', event => {
        event.preventDefault();
        resolve({
          types: event.dataTransfer.types,
          data: event.dataTransfer.getData('custom-type'),
        });
      }, false);
    });
  }"#,
        )
        .await
        .unwrap();
        p.hover(r#"[draggable="true"]"#).await.unwrap();
        p.mouse().down().await.unwrap();
        p.mouse().r#move(100, 100).await.unwrap();
        p.mouse().up().await.unwrap();
        assert_eq!(
            p.evaluate::<Value>("() => window.__result").await.unwrap(),
            json!({ "types": ["custom-type"], "data": "Hello World" })
        );
    });
}

#[test]
fn what_happens_when_dragging_element_is_destroyed() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"<button draggable="true">Draggable</button>
    <div id=target>drop here</div>"#,
        )
        .await
        .unwrap();
        p.evaluate::<()>(
            r#"() => {
    document.querySelector('#target').addEventListener('dragover', event => {
      document.querySelector('button')?.remove();
    }, false);
    document.querySelector('#target').addEventListener('drop', event => {
      document.querySelector('#target').textContent = 'dropped';
    }, false);
  }"#,
        )
        .await
        .unwrap();
        p.locator("button")
            .drag_to(&p.locator("div"))
            .into_future()
            .await
            .unwrap();
        expect_text(&p.locator("div"), "drop here").await;
    });
}
