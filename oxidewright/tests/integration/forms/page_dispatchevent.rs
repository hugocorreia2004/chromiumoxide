//! Port of `page-dispatchevent.spec.ts`.
//!
//! Skipped:
//! - DE-009 `should be atomic`: custom selector engines (`selectors.register`) are not
//!   supported.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::json;

use super::spawn;
use crate::common::*;

#[test]
fn should_dispatch_click_event() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/button.html", t.server.prefix))
            .await
            .unwrap();
        t.page.dispatch_event("button", "click").await.unwrap();
        let result: String = t.page.evaluate("() => window['result']").await.unwrap();
        assert_eq!(result, "Clicked");
    });
}

#[test]
fn should_dispatch_click_event_properties() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/button.html", t.server.prefix))
            .await
            .unwrap();
        t.page.dispatch_event("button", "click").await.unwrap();
        for name in ["bubbles", "cancelable", "composed"] {
            let value: bool = t.page.evaluate(name).await.unwrap();
            assert!(value, "{name}");
        }
    });
}

#[test]
fn should_dispatch_click_svg() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<svg height="100" width="100">
      <circle onclick="javascript:window.__CLICKED=42" cx="50" cy="50" r="40" stroke="black" stroke-width="3" fill="red" />
    </svg>"#,
            )
            .await
            .unwrap();
        t.page.dispatch_event("circle", "click").await.unwrap();
        let clicked: i32 = t.page.evaluate("() => window['__CLICKED']").await.unwrap();
        assert_eq!(clicked, 42);
    });
}

#[test]
fn should_dispatch_click_on_a_span_with_an_inline_element_inside() {
    run(|t| async move {
        t.page
            .set_content(
                "<style>
    span::before {
      content: 'q';
    }
    </style>
    <span onclick='javascript:window.CLICKED=42'></span>",
            )
            .await
            .unwrap();
        t.page.dispatch_event("span", "click").await.unwrap();
        let clicked: i32 = t.page.evaluate("() => window['CLICKED']").await.unwrap();
        assert_eq!(clicked, 42);
    });
}

#[test]
fn should_dispatch_click_after_navigation() {
    run(|t| async move {
        let url = format!("{}/input/button.html", t.server.prefix);
        t.page.goto(&url).await.unwrap();
        t.page.dispatch_event("button", "click").await.unwrap();
        t.page.goto(&url).await.unwrap();
        t.page.dispatch_event("button", "click").await.unwrap();
        let result: String = t.page.evaluate("() => window['result']").await.unwrap();
        assert_eq!(result, "Clicked");
    });
}

#[test]
fn should_dispatch_click_after_a_cross_origin_navigation() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/button.html", t.server.prefix))
            .await
            .unwrap();
        t.page.dispatch_event("button", "click").await.unwrap();
        t.page
            .goto(format!(
                "{}/input/button.html",
                t.server.cross_process_prefix
            ))
            .await
            .unwrap();
        t.page.dispatch_event("button", "click").await.unwrap();
        let result: String = t.page.evaluate("() => window['result']").await.unwrap();
        assert_eq!(result, "Clicked");
    });
}

#[test]
fn should_not_fail_when_element_is_blocked_on_hover() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<style>
    container { display: block; position: relative; width: 200px; height: 50px; }
    div, button { position: absolute; left: 0; top: 0; bottom: 0; right: 0; }
    div { pointer-events: none; }
    container:hover div { pointer-events: auto; background: red; }
  </style>
  <container>
    <button onclick="window.clicked=true">Click me</button>
    <div></div>
  </container>"#,
            )
            .await
            .unwrap();
        t.page.dispatch_event("button", "click").await.unwrap();
        let clicked: bool = t.page.evaluate("() => !!window['clicked']").await.unwrap();
        assert!(clicked);
    });
}

#[test]
fn should_dispatch_click_when_node_is_added_in_shadow_dom() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let watchdog = spawn(t.page.dispatch_event("span", "click"));
        t.page
            .evaluate::<()>(
                "() => { const div = document.createElement('div'); div.attachShadow({ mode: 'open' }); document.body.appendChild(div); }",
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        t.page
            .evaluate::<()>(
                "() => { const span = document.createElement('span'); span.textContent = 'Hello from shadow'; span.addEventListener('click', () => window['clicked'] = true); document.querySelector('div').shadowRoot.appendChild(span); }",
            )
            .await
            .unwrap();
        watchdog.await.unwrap().unwrap();
        let clicked: bool = t.page.evaluate("() => window['clicked']").await.unwrap();
        assert!(clicked);
    });
}

#[test]
fn should_dispatch_drag_drop_events() {
    run(|t| async move {
        t.page
            .goto(format!("{}/drag-n-drop.html", t.server.prefix))
            .await
            .unwrap();
        let data_transfer = t
            .page
            .evaluate_handle("() => new DataTransfer()")
            .await
            .unwrap();
        let init = HashMap::from([("dataTransfer", data_transfer.clone())]);
        t.page
            .dispatch_event("#source", "dragstart")
            .event_init(&init)
            .await
            .unwrap();
        t.page
            .dispatch_event("#target", "drop")
            .event_init(&init)
            .await
            .unwrap();
        let source = t.page.query_selector("#source").await.unwrap().unwrap();
        let target = t.page.query_selector("#target").await.unwrap().unwrap();
        let moved: bool = t
            .page
            .evaluate("({ source, target }) => source.parentElement === target")
            .arg(HashMap::from([("source", source), ("target", target)]))
            .await
            .unwrap();
        assert!(moved);
    });
}

#[test]
fn should_dispatch_drag_drop_events_via_element_handles() {
    run(|t| async move {
        t.page
            .goto(format!("{}/drag-n-drop.html", t.server.prefix))
            .await
            .unwrap();
        let data_transfer = t
            .page
            .evaluate_handle("() => new DataTransfer()")
            .await
            .unwrap();
        let init = HashMap::from([("dataTransfer", data_transfer.clone())]);
        let source = t.page.query_selector("#source").await.unwrap().unwrap();
        source
            .dispatch_event("dragstart")
            .event_init(&init)
            .await
            .unwrap();
        let target = t.page.query_selector("#target").await.unwrap().unwrap();
        target
            .dispatch_event("drop")
            .event_init(&init)
            .await
            .unwrap();
        let moved: bool = t
            .page
            .evaluate("({ source, target }) => source.parentElement === target")
            .arg(HashMap::from([("source", source), ("target", target)]))
            .await
            .unwrap();
        assert!(moved);
    });
}

#[test]
fn should_dispatch_click_event_via_element_handles() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/button.html", t.server.prefix))
            .await
            .unwrap();
        let button = t.page.query_selector("button").await.unwrap().unwrap();
        button.dispatch_event("click").await.unwrap();
        let result: String = t.page.evaluate("() => window['result']").await.unwrap();
        assert_eq!(result, "Clicked");
    });
}

#[test]
fn should_dispatch_wheel_event() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/scrollable.html", t.server.prefix))
            .await
            .unwrap();
        let events = t
            .page
            .locator("body")
            .evaluate_handle(
                "e => { const events = []; e.addEventListener('wheel', event => events.push(event)); return events; }",
            )
            .await
            .unwrap();
        t.page
            .locator("body")
            .dispatch_event("wheel")
            .event_init(json!({ "deltaX": 100, "deltaY": 200 }))
            .await
            .unwrap();
        assert_eq!(events.evaluate::<usize>("e => e.length").await.unwrap(), 1);
        assert!(
            events
                .evaluate::<bool>("e => e[0] instanceof WheelEvent")
                .await
                .unwrap()
        );
        let deltas: serde_json::Value = events
            .evaluate("e => ({ deltaX: e[0].deltaX, deltaY: e[0].deltaY })")
            .await
            .unwrap();
        assert_eq!(deltas, json!({ "deltaX": 100, "deltaY": 200 }));
    });
}

async fn orientation(t: &TestContext, event: &str, init: serde_json::Value, absolute: bool) {
    t.page
        .goto(format!("{}/device-orientation.html", t.server.prefix))
        .await
        .unwrap();
    t.page
        .locator("html")
        .dispatch_event(event)
        .event_init(init)
        .await
        .unwrap();
    let result: String = t.page.evaluate("result").await.unwrap();
    assert_eq!(result, "Oriented");
    assert_eq!(t.page.evaluate::<f64>("alpha").await.unwrap(), 10.0);
    assert_eq!(t.page.evaluate::<f64>("beta").await.unwrap(), 20.0);
    assert_eq!(t.page.evaluate::<f64>("gamma").await.unwrap(), 30.0);
    assert_eq!(t.page.evaluate::<bool>("absolute").await.unwrap(), absolute);
}

#[test]
fn should_dispatch_device_orientation_event() {
    run(|t| async move {
        orientation(
            &t,
            "deviceorientation",
            json!({ "alpha": 10, "beta": 20, "gamma": 30 }),
            false,
        )
        .await;
    });
}

#[test]
fn should_dispatch_absolute_device_orientation_event() {
    run(|t| async move {
        orientation(
            &t,
            "deviceorientationabsolute",
            json!({ "alpha": 10, "beta": 20, "gamma": 30, "absolute": true }),
            true,
        )
        .await;
    });
}

#[test]
fn should_dispatch_device_motion_event() {
    run(|t| async move {
        t.page
            .goto(format!("{}/device-motion.html", t.server.prefix))
            .await
            .unwrap();
        t.page
            .locator("html")
            .dispatch_event("devicemotion")
            .event_init(json!({
                "acceleration": { "x": 10, "y": 20, "z": 30 },
                "accelerationIncludingGravity": { "x": 15, "y": 25, "z": 35 },
                "rotationRate": { "alpha": 5, "beta": 10, "gamma": 15 },
                "interval": 16,
            }))
            .await
            .unwrap();
        let result: String = t.page.evaluate("result").await.unwrap();
        assert_eq!(result, "Moved");
        let dump: String = t
            .page
            .evaluate("() => JSON.stringify([acceleration, accelerationIncludingGravity, rotationRate, interval])")
            .await
            .unwrap();
        for (expr, expected) in [
            ("acceleration.x", 10.0),
            ("acceleration.y", 20.0),
            ("acceleration.z", 30.0),
            ("accelerationIncludingGravity.x", 15.0),
            ("accelerationIncludingGravity.y", 25.0),
            ("accelerationIncludingGravity.z", 35.0),
            ("rotationRate.alpha", 5.0),
            ("rotationRate.beta", 10.0),
            ("rotationRate.gamma", 15.0),
            ("interval", 16.0),
        ] {
            let actual: f64 = t
                .page
                .evaluate(expr)
                .await
                .unwrap_or_else(|e| panic!("{expr}: {e}: {}", dump));
            assert_eq!(actual, expected, "{expr}");
        }
    });
}

#[test]
fn should_throw_if_argument_is_from_different_frame() {
    run(|t| async move {
        t.page
            .goto(format!("{}/frames/one-frame.html", t.server.prefix))
            .await
            .unwrap();
        let frame = t.page.frames()[1].clone();
        let data_transfer = frame
            .evaluate_handle("() => new DataTransfer()")
            .await
            .unwrap();
        t.page
            .frame_locator("iframe")
            .locator("div")
            .dispatch_event("drop")
            .event_init(HashMap::from([("dataTransfer", data_transfer)]))
            .await
            .unwrap();
        let data_transfer = t
            .page
            .evaluate_handle("() => new DataTransfer()")
            .await
            .unwrap();
        let error = t
            .page
            .frame_locator("iframe")
            .locator("div")
            .dispatch_event("drop")
            .event_init(HashMap::from([("dataTransfer", data_transfer)]))
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("JSHandles can be evaluated only in the context they were created!"),
            "{error}"
        );
    });
}
