//! Port of `page-click.spec.ts`.
//!
//! Skipped:
//! - CL-003 (needs the `popup` event).
//! - CL-035 (`fixme` for Chromium upstream).
//! - CL-067, CL-068 (need the `__testHookBeforeStable` test hook).
//! - CL-076 (needs `page.requestGC`).
//! - CL-081 (needs the `console` event and Playwright's `window.builtins`).
//! - CL-090..CL-093 (`AbortSignal` has no Rust equivalent; drop the future instead).
//!
//! Adapted: CL-017 and CL-083 collect `console.log` calls in the page
//! instead of listening to the `console` event; CL-023 checks the URL hash
//! (the local `wrappedlink.html` has no click handler); CL-069 detaches the
//! frame while the click waits for a stable position instead of using a test
//! hook; CL-084 polls the text instead of `toHaveText`.

use std::future::IntoFuture;
use std::time::Duration;

use oxidewright::{GetByRoleOptions, KeyboardModifier, MouseButton};
use serde_json::{Value, json};

use super::{eval, eval_sel, give_it_a_chance_to_click, rafraf};
use crate::common::*;

fn button_page(t: &TestContext) -> String {
    format!("{}/input/button.html", t.server.prefix)
}

#[test]
fn should_click_the_button() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        t.page.click("button").await.unwrap();
        assert_eq!(eval(&t.page, "result").await, "Clicked");
    });
}

#[test]
fn should_click_button_inside_frameset() {
    run(|t| async move {
        t.page
            .goto(format!("{}/frames/frameset.html", t.server.prefix))
            .await
            .unwrap();
        let frame_element = t.page.query_selector("frame").await.unwrap().unwrap();
        frame_element
            .evaluate::<Value>("frame => frame.src = '/input/button.html'")
            .await
            .unwrap();
        let frame = frame_element.content_frame().await.unwrap().unwrap();
        frame.wait_for_load_state().await.unwrap();
        frame.click("button").await.unwrap();
        assert_eq!(frame.evaluate::<Value>("result").await.unwrap(), "Clicked");
    });
}

#[test]
fn should_click_svg() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<svg height="100" width="100">
                  <circle onclick="window.__CLICKED=42" cx="50" cy="50" r="40" stroke="black" stroke-width="3" fill="red" />
                </svg>"#,
            )
            .await
            .unwrap();
        t.page.click("circle").await.unwrap();
        assert_eq!(eval(&t.page, "__CLICKED").await, 42);
    });
}

#[test]
fn should_click_the_button_if_window_node_is_removed() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        t.page
            .evaluate::<Value>("() => delete window.Node")
            .await
            .unwrap();
        t.page.click("button").await.unwrap();
        assert_eq!(eval(&t.page, "result").await, "Clicked");
    });
}

#[test]
fn should_click_on_a_span_with_an_inline_element_inside() {
    run(|t| async move {
        t.page
            .set_content(
                "<style>span::before { content: 'q'; }</style><span onclick='window.CLICKED=42'></span>",
            )
            .await
            .unwrap();
        t.page.click("span").await.unwrap();
        assert_eq!(eval(&t.page, "CLICKED").await, 42);
    });
}

async fn click_1x1(t: &TestContext, style: &str) {
    t.page
        .set_content(format!(
            r#"<div style="{style} width: 1px; height: 1px;" onclick="window.__clicked = true"></div>"#
        ))
        .await
        .unwrap();
    t.page.click("div").await.unwrap();
    assert_eq!(eval(&t.page, "window.__clicked").await, true);
}

#[test]
fn should_click_the_aligned_1x1_div() {
    run(|t| async move { click_1x1(&t, "").await });
}

#[test]
fn should_click_the_half_aligned_1x1_div() {
    run(|t| async move { click_1x1(&t, "margin-left: 20.5px; margin-top: 11.5px;").await });
}

#[test]
fn should_click_the_unaligned_1x1_div_v1() {
    run(|t| async move { click_1x1(&t, "margin-left: 20.23px; margin-top: 11.65px;").await });
}

#[test]
fn should_click_the_unaligned_1x1_div_v2() {
    run(|t| async move { click_1x1(&t, "margin-left: 20.68px; margin-top: 11.13px;").await });
}

#[test]
fn should_click_the_unaligned_1x1_div_v3() {
    run(|t| async move { click_1x1(&t, "margin-left: 20.68px; margin-top: 11.52px;").await });
}

#[test]
fn should_click_the_unaligned_1x1_div_v4() {
    run(|t| async move { click_1x1(&t, "margin-left: 20.15px; margin-top: 11.24px;").await });
}

#[test]
fn should_click_the_button_after_navigation() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        t.page.click("button").await.unwrap();
        t.page.goto(button_page(&t)).await.unwrap();
        t.page.click("button").await.unwrap();
        assert_eq!(eval(&t.page, "result").await, "Clicked");
    });
}

#[test]
fn should_click_the_button_after_a_cross_origin_navigation() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        t.page.click("button").await.unwrap();
        t.page
            .goto(format!(
                "{}/input/button.html",
                t.server.cross_process_prefix
            ))
            .await
            .unwrap();
        t.page.click("button").await.unwrap();
        assert_eq!(eval(&t.page, "result").await, "Clicked");
    });
}

#[test]
fn should_click_when_one_of_inline_box_children_is_outside_of_viewport() {
    run(|t| async move {
        t.page
            .set_content(
                "<style>i { position: absolute; top: -1000px; }</style><span onclick='window.CLICKED = 42;'><i>woof</i><b>doggo</b></span>",
            )
            .await
            .unwrap();
        t.page.click("span").await.unwrap();
        assert_eq!(eval(&t.page, "CLICKED").await, 42);
    });
}

#[test]
fn should_select_the_text_by_triple_clicking() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        let text = "This is the text that we are going to try to select. Let's see how it goes.";
        t.page.fill("textarea", text).await.unwrap();
        t.page.click("textarea").click_count(3).await.unwrap();
        let selected = eval(
            &t.page,
            "() => { const textarea = document.querySelector('textarea'); return textarea.value.substring(textarea.selectionStart, textarea.selectionEnd); }",
        )
        .await;
        assert_eq!(selected, text);
    });
}

#[test]
fn should_click_offscreen_buttons() {
    run(|t| async move {
        t.page
            .goto(format!("{}/offscreenbuttons.html", t.server.prefix))
            .await
            .unwrap();
        eval(
            &t.page,
            "() => { window.__messages = []; console.log = m => window.__messages.push(m); }",
        )
        .await;
        for i in 0..11 {
            eval(&t.page, "() => window.scrollTo(0, 0)").await;
            t.page.click(format!("#btn{i}")).await.unwrap();
        }
        let expected: Vec<String> = (0..11).map(|i| format!("button #{i} clicked")).collect();
        assert_eq!(eval(&t.page, "window.__messages").await, json!(expected));
    });
}

#[test]
fn should_wait_for_visible_when_already_visible() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        t.page.click("button").await.unwrap();
        assert_eq!(eval(&t.page, "result").await, "Clicked");
    });
}

#[test]
fn should_not_wait_with_force() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        eval_sel(&t.page, "button", "b => b.style.display = 'none'").await;
        assert_err_contains(
            t.page.click("button").force(true).await,
            "Element is not visible",
        );
        assert_eq!(eval(&t.page, "result").await, "Was not clicked");
    });
}

async fn wait_for_style_to_be_gone(t: &TestContext, hide: &str, show: &str, check_result: bool) {
    t.page.goto(button_page(t)).await.unwrap();
    eval_sel(&t.page, "button", hide).await;
    let clicked = tokio::spawn(t.page.click("button").timeout(0).into_future());
    give_it_a_chance_to_click(&t.page).await;
    if check_result {
        assert_eq!(eval(&t.page, "result").await, "Was not clicked");
    }
    assert!(!clicked.is_finished());
    eval_sel(&t.page, "button", show).await;
    clicked.await.unwrap().unwrap();
    assert_eq!(eval(&t.page, "result").await, "Clicked");
}

#[test]
fn should_wait_for_display_none_to_be_gone() {
    run(|t| async move {
        wait_for_style_to_be_gone(
            &t,
            "b => b.style.display = 'none'",
            "b => b.style.display = 'block'",
            true,
        )
        .await
    });
}

#[test]
fn should_wait_for_visibility_hidden_to_be_gone() {
    run(|t| async move {
        wait_for_style_to_be_gone(
            &t,
            "b => b.style.visibility = 'hidden'",
            "b => b.style.visibility = 'visible'",
            true,
        )
        .await
    });
}

#[test]
fn should_wait_for_visible_when_parent_is_hidden() {
    run(|t| async move {
        wait_for_style_to_be_gone(
            &t,
            "b => b.parentElement.style.display = 'none'",
            "b => b.parentElement.style.display = 'block'",
            false,
        )
        .await
    });
}

#[test]
fn should_click_wrapped_links() {
    run(|t| async move {
        t.page
            .goto(format!("{}/wrappedlink.html", t.server.prefix))
            .await
            .unwrap();
        t.page.click("a").await.unwrap();
        assert_eq!(eval(&t.page, "location.hash").await, "#clicked");
    });
}

#[test]
fn should_click_on_checkbox_input_and_toggle() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/checkbox.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(
            eval(&t.page, "() => window['result'].check").await,
            Value::Null
        );
        t.page.click("input#agree").await.unwrap();
        assert_eq!(eval(&t.page, "() => window['result'].check").await, true);
        assert_eq!(
            eval(&t.page, "() => window['result'].events").await,
            json!([
                "mouseover",
                "mouseenter",
                "mousemove",
                "mousedown",
                "mouseup",
                "click",
                "input",
                "change"
            ])
        );
        t.page.click("input#agree").await.unwrap();
        assert_eq!(eval(&t.page, "() => window['result'].check").await, false);
    });
}

#[test]
fn should_click_on_checkbox_label_and_toggle() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/checkbox.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(
            eval(&t.page, "() => window['result'].check").await,
            Value::Null
        );
        t.page.click("label[for=\"agree\"]").await.unwrap();
        assert_eq!(eval(&t.page, "() => window['result'].check").await, true);
        assert_eq!(
            eval(&t.page, "() => window['result'].events").await,
            json!(["click", "input", "change"])
        );
        t.page.click("label[for=\"agree\"]").await.unwrap();
        assert_eq!(eval(&t.page, "() => window['result'].check").await, false);
    });
}

#[test]
fn should_scroll_and_click_the_button() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/scrollable.html", t.server.prefix))
            .await
            .unwrap();
        t.page.click("#button-5").await.unwrap();
        assert_eq!(
            eval(
                &t.page,
                "() => document.querySelector('#button-5').textContent"
            )
            .await,
            "clicked"
        );
        t.page.click("#button-80").await.unwrap();
        assert_eq!(
            eval(
                &t.page,
                "() => document.querySelector('#button-80').textContent"
            )
            .await,
            "clicked"
        );
    });
}

#[test]
fn should_scroll_and_click_the_button_with_smooth_scroll_behavior() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/scrollable.html", t.server.prefix))
            .await
            .unwrap();
        eval(
            &t.page,
            "() => { const s = document.createElement('style'); s.textContent = 'html { scroll-behavior: smooth; }'; document.head.appendChild(s); }",
        )
        .await;
        for _ in 0..5 {
            t.page.click("#button-80").await.unwrap();
            assert_eq!(
                eval(
                    &t.page,
                    "() => document.querySelector('#button-80').textContent"
                )
                .await,
                "clicked"
            );
            t.page.click("#button-20").await.unwrap();
            assert_eq!(
                eval(
                    &t.page,
                    "() => document.querySelector('#button-20').textContent"
                )
                .await,
                "clicked"
            );
        }
    });
}

const DOUBLE_LISTENER: &str = "() => { window['double'] = false; const button = document.querySelector('button'); button.addEventListener('dblclick', event => { window['double'] = true; }); }";

#[test]
fn should_double_click_the_button() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        eval(&t.page, DOUBLE_LISTENER).await;
        t.page.dblclick("button").await.unwrap();
        assert_eq!(eval(&t.page, "double").await, true);
        assert_eq!(eval(&t.page, "result").await, "Clicked");
    });
}

#[test]
fn should_click_a_partially_obscured_button() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        eval(
            &t.page,
            "() => { const button = document.querySelector('button'); button.textContent = 'Some really long text that will go offscreen'; button.style.position = 'absolute'; button.style.left = '368px'; }",
        )
        .await;
        t.page.click("button").await.unwrap();
        assert_eq!(eval(&t.page, "() => window['result']").await, "Clicked");
    });
}

#[test]
fn should_click_a_rotated_button() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/rotatedButton.html", t.server.prefix))
            .await
            .unwrap();
        t.page.click("button").await.unwrap();
        assert_eq!(eval(&t.page, "result").await, "Clicked");
    });
}

#[test]
fn should_fire_contextmenu_event_on_right_click() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/scrollable.html", t.server.prefix))
            .await
            .unwrap();
        t.page
            .click("#button-8")
            .button(MouseButton::Right)
            .await
            .unwrap();
        assert_eq!(
            eval(
                &t.page,
                "() => document.querySelector('#button-8').textContent"
            )
            .await,
            "context menu"
        );
    });
}

#[test]
fn should_click_links_which_cause_navigation() {
    run(|t| async move {
        t.page
            .set_content(format!(
                "<a href=\"{}\">empty.html</a>",
                t.server.empty_page
            ))
            .await
            .unwrap();
        t.page.click("a").await.unwrap();
    });
}

#[test]
fn should_not_wait_for_navigation_after_clicking_a_link_answered_with_204() {
    run(|t| async move {
        t.server.set_route("/204", |_| async {
            axum::response::Response::builder()
                .status(204)
                .body(axum::body::Body::empty())
                .unwrap()
        });
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .set_content(format!("<a href=\"{}/204\">link</a>", t.server.prefix))
            .await
            .unwrap();
        t.page.click("a").timeout(5000).await.unwrap();
        assert_eq!(t.page.url(), t.server.empty_page);
    });
}

#[test]
fn should_click_the_button_inside_an_iframe() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .set_content("<div style=\"width:100px;height:100px\">spacer</div>")
            .await
            .unwrap();
        attach_frame(&t.page, "button-test", &button_page(&t)).await;
        let frame = t.page.frames()[1].clone();
        let button = frame.query_selector("button").await.unwrap().unwrap();
        button.click().await.unwrap();
        assert_eq!(
            frame
                .evaluate::<Value>("() => window['result']")
                .await
                .unwrap(),
            "Clicked"
        );
    });
}

const STICKY_LIST: &str = r#"
    <ol>
    <li>hi1</li><li>hi2</li><li>hi3</li><li>hi4</li><li>hi5</li><li>hi6</li><li>hi7</li><li>hi8</li>
    <li id=target onclick="window.__clicked = true">hi9</li>
    <li>hi10</li><li>hi11</li><li>hi12</li><li>hi13</li><li id=li14>hi14</li>
    </ol>"#;

#[test]
fn should_click_the_button_behind_sticky_header() {
    run(|t| async move {
        t.page.set_viewport_size((500, 240)).await.unwrap();
        t.page
            .set_content(format!(
                r#"<style>
                * {{ padding: 0; margin: 0; }}
                li {{ height: 80px; border: 1px solid black; }}
                ol {{ padding-top: 160px; }}
                div.fixed {{ position: fixed; z-index: 1001; width: 100%; background: red; height: 160px; }}
                </style>
                <div class=fixed></div>{STICKY_LIST}"#
            ))
            .await
            .unwrap();
        eval_sel(&t.page, "#li14", "e => e.scrollIntoView()").await;
        t.page.click("#target").await.unwrap();
        assert_eq!(eval(&t.page, "() => window['__clicked']").await, true);
    });
}

#[test]
fn should_click_the_button_behind_position_absolute_header() {
    run(|t| async move {
        t.page.set_viewport_size((500, 240)).await.unwrap();
        t.page
            .set_content(format!(
                r#"<style>
                * {{ padding: 0; margin: 0; }}
                li {{ height: 80px; border: 1px solid black; }}
                ol {{ height: 100vh; overflow: scroll; padding-top: 160px; }}
                body {{ position: relative; }}
                div.fixed {{ position: absolute; top: 0; z-index: 1001; width: 100%; background: red; height: 160px; }}
                </style>{STICKY_LIST}
                <div class=fixed>Overlay</div>"#
            ))
            .await
            .unwrap();
        eval_sel(
            &t.page,
            "ol",
            "e => { const target = document.querySelector('#target'); e.scrollTo({ top: target.offsetTop, behavior: 'instant' }); }",
        )
        .await;
        t.page.click("#target").await.unwrap();
        assert_eq!(eval(&t.page, "() => window['__clicked']").await, true);
    });
}

async fn click_with_offset(t: &TestContext, setup: &[&str], x: f64, y: f64) {
    t.page.goto(button_page(t)).await.unwrap();
    for s in setup {
        eval_sel(&t.page, "button", s).await;
    }
    t.page.click("button").position((x, y)).await.unwrap();
    assert_eq!(eval(&t.page, "result").await, "Clicked");
    assert_eq!(eval(&t.page, "offsetX").await, json!(x as i64));
    assert_eq!(eval(&t.page, "offsetY").await, json!(y as i64));
}

#[test]
fn should_click_the_button_with_px_border_with_offset() {
    run(|t| async move {
        click_with_offset(
            &t,
            &["button => button.style.borderWidth = '8px'"],
            20.0,
            10.0,
        )
        .await
    });
}

#[test]
fn should_click_the_button_with_em_border_with_offset() {
    run(|t| async move {
        click_with_offset(
            &t,
            &[
                "button => button.style.borderWidth = '2em'",
                "button => button.style.fontSize = '12px'",
            ],
            20.0,
            10.0,
        )
        .await
    });
}

#[test]
fn should_click_a_very_large_button_with_offset() {
    run(|t| async move {
        click_with_offset(
            &t,
            &[
                "button => button.style.borderWidth = '8px'",
                "button => button.style.height = button.style.width = '2000px'",
            ],
            1900.0,
            1910.0,
        )
        .await
    });
}

#[test]
fn should_click_a_button_in_scrolling_container_with_offset() {
    run(|t| async move {
        click_with_offset(
            &t,
            &["button => { const container = document.createElement('div'); container.style.overflow = 'auto'; container.style.width = '200px'; container.style.height = '200px'; button.parentElement.insertBefore(container, button); container.appendChild(button); button.style.height = '2000px'; button.style.width = '2000px'; button.style.borderWidth = '8px'; }"],
            1900.0,
            1910.0,
        )
        .await
    });
}

#[test]
fn should_wait_for_stable_position() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        eval_sel(
            &t.page,
            "button",
            "button => { button.style.transition = 'margin 500ms linear 0s'; button.style.marginLeft = '200px'; button.style.borderWidth = '0'; button.style.width = '200px'; button.style.height = '20px'; button.style.display = 'block'; document.body.style.margin = '0'; }",
        )
        .await;
        rafraf(&t.page, 1).await;
        t.page.click("button").await.unwrap();
        assert_eq!(eval(&t.page, "() => window['result']").await, "Clicked");
        assert_eq!(eval(&t.page, "pageX").await, 300);
        assert_eq!(eval(&t.page, "pageY").await, 10);
    });
}

const FLYOVER: &str = "button => { button.style.borderWidth = '0'; button.style.width = '200px'; button.style.height = '20px'; document.body.style.margin = '0'; document.body.style.position = 'relative'; const flyOver = document.createElement('div'); flyOver.className = 'flyover'; flyOver.style.position = 'absolute'; flyOver.style.width = '400px'; flyOver.style.height = '20px'; flyOver.style.left = '-200px'; flyOver.style.top = '0'; flyOver.style.background = 'red'; document.body.appendChild(flyOver); }";

async fn wait_for_hit_target(t: &TestContext, trial: bool) {
    t.page.goto(button_page(t)).await.unwrap();
    eval_sel(&t.page, "button", FLYOVER).await;
    let click = tokio::spawn(t.page.click("button").trial(trial).into_future());
    assert!(!click.is_finished());
    eval_sel(&t.page, ".flyover", "flyOver => flyOver.style.left = '0'").await;
    give_it_a_chance_to_click(&t.page).await;
    assert!(!click.is_finished());
    eval_sel(
        &t.page,
        ".flyover",
        "flyOver => flyOver.style.left = '200px'",
    )
    .await;
    click.await.unwrap().unwrap();
}

#[test]
fn should_wait_for_becoming_hit_target() {
    run(|t| async move {
        wait_for_hit_target(&t, false).await;
        assert_eq!(eval(&t.page, "() => window['result']").await, "Clicked");
    });
}

#[test]
fn should_wait_for_becoming_hit_target_with_trial_run() {
    run(|t| async move {
        wait_for_hit_target(&t, true).await;
        assert_eq!(
            eval(&t.page, "() => window['result']").await,
            "Was not clicked"
        );
    });
}

#[test]
fn trial_run_should_work_with_short_timeout() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        eval_sel(&t.page, "button", "button => button.disabled = true").await;
        assert_err_contains(
            t.page.click("button").trial(true).timeout(2000).await,
            "click action (trial run)",
        );
        assert_eq!(
            eval(&t.page, "() => window['result']").await,
            "Was not clicked"
        );
    });
}

#[test]
fn trial_run_should_not_click() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        t.page.click("button").trial(true).await.unwrap();
        assert_eq!(
            eval(&t.page, "() => window['result']").await,
            "Was not clicked"
        );
    });
}

#[test]
fn trial_run_should_not_double_click() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        eval(&t.page, DOUBLE_LISTENER).await;
        t.page.dblclick("button").trial(true).await.unwrap();
        assert_eq!(eval(&t.page, "double").await, false);
        assert_eq!(eval(&t.page, "result").await, "Was not clicked");
    });
}

#[test]
fn should_fail_when_obscured_and_not_waiting_for_hit_target() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        let button = t.page.query_selector("button").await.unwrap().unwrap();
        eval(
            &t.page,
            "() => { document.body.style.position = 'relative'; const blocker = document.createElement('div'); blocker.style.position = 'absolute'; blocker.style.width = '400px'; blocker.style.height = '20px'; blocker.style.left = '0'; blocker.style.top = '0'; document.body.appendChild(blocker); }",
        )
        .await;
        button.click().force(true).await.unwrap();
        assert_eq!(
            eval(&t.page, "() => window['result']").await,
            "Was not clicked"
        );
    });
}

/// Start clicking `selector`, check it waits, run `unblock`, check it clicked.
async fn waits_until(t: &TestContext, content: &str, selector: &str, unblock: &str) {
    t.page.set_content(content).await.unwrap();
    let click = tokio::spawn(t.page.click(selector).into_future());
    give_it_a_chance_to_click(&t.page).await;
    assert_eq!(eval(&t.page, "window.__CLICKED").await, Value::Null);
    assert!(!click.is_finished());
    eval(&t.page, unblock).await;
    click.await.unwrap().unwrap();
    assert_eq!(eval(&t.page, "__CLICKED").await, true);
}

#[test]
fn should_wait_for_button_to_be_enabled() {
    run(|t| async move {
        waits_until(
            &t,
            "<button onclick=\"window.__CLICKED=true;\" disabled><span>Click target</span></button>",
            "text=Click target",
            "() => document.querySelector('button').removeAttribute('disabled')",
        )
        .await
    });
}

#[test]
fn should_wait_for_element_to_become_non_inert() {
    run(|t| async move {
        waits_until(
            &t,
            "<div inert id=container><button onclick=\"window.__CLICKED=true;\"><span>Click target</span></button></div>",
            "text=Click target",
            "() => document.getElementById('container').removeAttribute('inert')",
        )
        .await
    });
}

#[test]
fn should_wait_for_input_to_be_enabled() {
    run(|t| async move {
        waits_until(
            &t,
            "<input onclick=\"window.__CLICKED=true;\" disabled>",
            "input",
            "() => document.querySelector('input').removeAttribute('disabled')",
        )
        .await
    });
}

#[test]
fn should_wait_for_select_to_be_enabled() {
    run(|t| async move {
        waits_until(
            &t,
            "<select disabled><option selected>Hello</option></select><script>document.querySelector('select').addEventListener('mousedown', event => { window.__CLICKED=true; event.preventDefault(); });</script>",
            "select",
            "() => document.querySelector('select').removeAttribute('disabled')",
        )
        .await
    });
}

#[test]
fn should_click_disabled_div() {
    run(|t| async move {
        t.page
            .set_content("<div onclick=\"window.__CLICKED=true\" disabled>Click target</div>")
            .await
            .unwrap();
        t.page.click("text=Click target").await.unwrap();
        assert_eq!(eval(&t.page, "__CLICKED").await, true);
    });
}

#[test]
fn should_wait_for_button_to_be_clickable_when_it_has_pointer_events_none() {
    run(|t| async move {
        waits_until(
            &t,
            "<button onclick=\"window.__CLICKED=true\" style=\"pointer-events:none\"><span>Click target</span></button>",
            "text=Click target",
            "() => document.querySelector('button').style.removeProperty('pointer-events')",
        )
        .await
    });
}

#[test]
fn should_wait_for_label_to_be_clickable_when_it_has_pointer_events_none() {
    run(|t| async move {
        t.page
            .set_content("<label onclick=\"window.__CLICKED=true\" style=\"pointer-events:none\"><span>Click target</span></label>")
            .await
            .unwrap();
        let click = tokio::spawn(t.page.click("text=Click target").into_future());
        for _ in 0..5 {
            assert_eq!(eval(&t.page, "window.__CLICKED").await, Value::Null);
        }
        eval(
            &t.page,
            "() => document.querySelector('label').style.removeProperty('pointer-events')",
        )
        .await;
        click.await.unwrap().unwrap();
        assert_eq!(eval(&t.page, "__CLICKED").await, true);
    });
}

#[test]
fn should_update_modifiers_correctly() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        t.page
            .click("button")
            .modifiers([KeyboardModifier::Shift])
            .await
            .unwrap();
        assert_eq!(eval(&t.page, "shiftKey").await, true);
        t.page.click("button").modifiers([]).await.unwrap();
        assert_eq!(eval(&t.page, "shiftKey").await, false);

        t.page.keyboard().down("Shift").await.unwrap();
        t.page.click("button").modifiers([]).await.unwrap();
        assert_eq!(eval(&t.page, "shiftKey").await, false);
        t.page.click("button").await.unwrap();
        assert_eq!(eval(&t.page, "shiftKey").await, true);
        t.page.keyboard().up("Shift").await.unwrap();
        t.page.click("button").await.unwrap();
        assert_eq!(eval(&t.page, "shiftKey").await, false);
    });
}

#[test]
fn should_click_an_offscreen_element_when_scroll_behavior_is_smooth() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<div style="border: 1px solid black; height: 500px; overflow: auto; width: 500px; scroll-behavior: smooth">
                <button style="margin-top: 2000px" onClick="window.clicked = true">hi</button>
                </div>"#,
            )
            .await
            .unwrap();
        t.page.click("button").await.unwrap();
        assert_eq!(eval(&t.page, "window.clicked").await, true);
    });
}

#[test]
fn should_report_nice_error_when_element_is_detached_and_force_clicked() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/animating-button.html", t.server.prefix))
            .await
            .unwrap();
        eval(&t.page, "addButton()").await;
        let handle = t.page.query_selector("button").await.unwrap().unwrap();
        eval(&t.page, "stopButton(true)").await;
        let result = handle.click().force(true).await;
        assert_eq!(eval(&t.page, "window.clicked").await, Value::Null);
        assert_err_contains(result, "Element is not attached to the DOM");
    });
}

#[test]
fn should_fail_when_element_detaches_after_animation() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/animating-button.html", t.server.prefix))
            .await
            .unwrap();
        eval(&t.page, "addButton()").await;
        let handle = t.page.query_selector("button").await.unwrap().unwrap();
        let click = tokio::spawn(handle.click().into_future());
        eval(&t.page, "stopButton(true)").await;
        let result = click.await.unwrap();
        assert_eq!(eval(&t.page, "window.clicked").await, Value::Null);
        assert_err_contains(result, "Element is not attached to the DOM");
    });
}

#[test]
fn should_retry_when_element_detaches_after_animation() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/animating-button.html", t.server.prefix))
            .await
            .unwrap();
        eval(&t.page, "addButton()").await;
        let click = tokio::spawn(t.page.click("button").into_future());
        assert!(!click.is_finished());
        assert_eq!(eval(&t.page, "window.clicked").await, Value::Null);
        eval(&t.page, "stopButton(true)").await;
        eval(&t.page, "addButton()").await;
        assert!(!click.is_finished());
        assert_eq!(eval(&t.page, "window.clicked").await, Value::Null);
        eval(&t.page, "stopButton(true)").await;
        eval(&t.page, "addButton()").await;
        assert!(!click.is_finished());
        assert_eq!(eval(&t.page, "window.clicked").await, Value::Null);
        eval(&t.page, "stopButton(false)").await;
        click.await.unwrap().unwrap();
        assert_eq!(eval(&t.page, "clicked").await, true);
    });
}

const ANIMATING_FROM_OUTSIDE: &str = r#"<style>
    @keyframes move { from { left: -300px; } to { left: 0; } }
    button { position: absolute; left: -300px; top: 0; bottom: 0; width: 200px; }
    button.animated { animation: 1s linear 1s move forwards; }
    </style>
    <div style="position: relative; width: 300px; height: 300px;">
      <button onclick="window.clicked=true"></button>
    </div>"#;

#[test]
fn should_retry_when_element_is_animating_from_outside_the_viewport() {
    run(|t| async move {
        t.page.set_content(ANIMATING_FROM_OUTSIDE).await.unwrap();
        let handle = t.page.query_selector("button").await.unwrap().unwrap();
        let click = tokio::spawn(handle.click().into_future());
        handle
            .evaluate::<Value>("button => button.className = 'animated'")
            .await
            .unwrap();
        click.await.unwrap().unwrap();
        assert_eq!(eval(&t.page, "clicked").await, true);
    });
}

#[test]
fn should_fail_when_element_is_animating_from_outside_the_viewport_with_force() {
    run(|t| async move {
        t.page.set_content(ANIMATING_FROM_OUTSIDE).await.unwrap();
        let handle = t.page.query_selector("button").await.unwrap().unwrap();
        let click = tokio::spawn(handle.click().force(true).into_future());
        handle
            .evaluate::<Value>("button => button.className = 'animated'")
            .await
            .unwrap();
        let result = click.await.unwrap();
        assert_eq!(eval(&t.page, "window.clicked").await, Value::Null);
        assert_err_contains(result, "Element is outside of the viewport");
    });
}

#[test]
fn should_dispatch_microtasks_in_order() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<button id=button>Click me</button>
                <script>
                  let mutationCount = 0;
                  const observer = new MutationObserver((mutationsList, observer) => {
                    for(let mutation of mutationsList)
                      ++mutationCount;
                  });
                  observer.observe(document.body, { attributes: true, childList: true, subtree: true });
                  button.addEventListener('mousedown', () => {
                    mutationCount = 0;
                    document.body.appendChild(document.createElement('div'));
                  });
                  button.addEventListener('mouseup', () => {
                    window['result'] = mutationCount;
                  });
                </script>"#,
            )
            .await
            .unwrap();
        t.page.click("button").await.unwrap();
        assert_eq!(eval(&t.page, "() => window['result']").await, 1);
    });
}

#[test]
fn should_click_the_button_when_window_inner_width_is_corrupted() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        eval(
            &t.page,
            "() => Object.defineProperty(window, 'innerWidth', { value: 0 })",
        )
        .await;
        t.page.click("button").await.unwrap();
        assert_eq!(eval(&t.page, "result").await, "Clicked");
    });
}

#[test]
fn should_click_zero_sized_input_by_label() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<label>
                  Click me
                  <input onclick="window.__clicked=true" style="width:0;height:0;padding:0;margin:0;border:0;">
                </label>"#,
            )
            .await
            .unwrap();
        t.page.click("text=Click me").await.unwrap();
        assert_eq!(eval(&t.page, "window.__clicked").await, true);
    });
}

#[test]
fn should_not_hang_when_frame_is_detached() {
    run(|t| async move {
        attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        let frame = t.page.frames()[1].clone();
        frame.goto(button_page(&t)).await.unwrap();
        frame
            .eval_on_selector::<Value>(
                "button",
                "button => { button.style.transition = 'margin 5s linear 0s'; button.style.marginLeft = '200px'; }",
            )
            .await
            .unwrap();
        let click = tokio::spawn(frame.click("button").into_future());
        tokio::time::sleep(Duration::from_millis(1000)).await;
        detach_frame(&t.page, "frame1").await;
        let error = click.await.unwrap().unwrap_err().to_string();
        assert!(
            error.contains("frame got detached") || error.contains("Frame was detached"),
            "{error}"
        );
    });
}

#[test]
fn should_climb_dom_for_inner_label_with_pointer_events_none() {
    run(|t| async move {
        t.page
            .set_content("<button onclick=\"window.__CLICKED=true;\"><label style=\"pointer-events:none\">Click target</label></button>")
            .await
            .unwrap();
        t.page.click("text=Click target").await.unwrap();
        assert_eq!(eval(&t.page, "__CLICKED").await, true);
    });
}

#[test]
fn should_climb_up_to_role_button() {
    run(|t| async move {
        t.page
            .set_content("<div role=button onclick=\"window.__CLICKED=true;\"><div style=\"pointer-events:none\"><span><div>Click target</div></span></div>")
            .await
            .unwrap();
        t.page.click("text=Click target").await.unwrap();
        assert_eq!(eval(&t.page, "__CLICKED").await, true);
    });
}

#[test]
fn should_climb_up_to_a_anchor() {
    run(|t| async move {
        t.page
            .set_content("<a href=\"#\" onclick=\"window.__CLICKED=true\" id=\"outer\"><div id=\"intermediate\"><div id=\"inner\" style=\"pointer-events: none\">Inner</div></div></a>")
            .await
            .unwrap();
        t.page.click("#inner").await.unwrap();
        assert_eq!(eval(&t.page, "__CLICKED").await, true);
    });
}

#[test]
fn should_climb_up_to_a_role_link() {
    run(|t| async move {
        t.page
            .set_content("<div role=link onclick=\"window.__CLICKED=true\" id=\"outer\"><div id=\"inner\" style=\"pointer-events: none\">Inner</div></div>")
            .await
            .unwrap();
        t.page.click("#inner").await.unwrap();
        assert_eq!(eval(&t.page, "__CLICKED").await, true);
    });
}

fn iframe_page(outer_style: &str) -> String {
    format!(
        r#"<style>
          {outer_style}
        </style>
        <iframe srcdoc="
          <style>
            body, html {{ margin: 0; padding: 0; }}
            div {{ margin-left: 10px; margin-top: 20px; width: 2px; height: 2px; }}
          </style>
          <div>Target</div>
          <script>
            document.querySelector('div').addEventListener('click', () => window.top._clicked = true);
          </script>
        "></iframe>"#
    )
}

async fn click_in_iframe(t: &TestContext, outer_style: &str, force: bool) {
    t.page.set_content(iframe_page(outer_style)).await.unwrap();
    let locator = t.page.frame_locator("iframe").locator("div");
    locator.click().force(force).await.unwrap();
    assert_eq!(eval(&t.page, "window._clicked").await, true);
}

#[test]
fn should_click_in_an_iframe_with_border() {
    run(|t| async move {
        click_in_iframe(
            &t,
            "body, html, iframe { margin: 0; padding: 0; border: none; box-sizing: border-box; }
             iframe { border: 4px solid black; background: gray; margin-left: 33px; margin-top: 24px; width: 400px; height: 400px; }",
            false,
        )
        .await
    });
}

#[test]
fn should_click_in_an_iframe_with_border_2() {
    run(|t| async move {
        click_in_iframe(
            &t,
            "body, html, iframe { margin: 0; padding: 0; border: none; }
             iframe { border: 4px solid black; background: gray; margin-left: 33px; margin-top: 24px; width: 400px; height: 400px; }",
            false,
        )
        .await
    });
}

#[test]
fn should_click_in_a_transformed_iframe() {
    run(|t| async move {
        click_in_iframe(
            &t,
            "body, html, iframe { margin: 0; padding: 0; border: none; }
             iframe { border: 4px solid black; background: gray; margin-left: 33px; margin-top: 24px; width: 400px; height: 400px; transform: translate(100px, 100px) scale(1.2) rotate3d(1, 1, 1, 25deg); }",
            false,
        )
        .await
    });
}

#[test]
fn should_click_a_button_that_is_overlaid_by_a_permission_popup() {
    run(|t| async move {
        t.page.set_viewport_size((500, 500)).await.unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .set_content(
                r#"<style>body, html { padding: 0; margin: 0; }</style>
                <script type='text/javascript'>
                  window.addEventListener('DOMContentLoaded', () => {
                    for (let i = 0; i < 100; ++i) {
                      const button = document.createElement('button');
                      button.textContent = i;
                      button.style.setProperty('width', '50px');
                      button.style.setProperty('height', '50px');
                      document.body.append(button);
                    }
                  }, false);
                </script>"#,
            )
            .await
            .unwrap();
        eval(
            &t.page,
            "() => { navigator.geolocation.getCurrentPosition(position => { }); }",
        )
        .await;
        for i in 0..30 {
            t.page.click(format!("text={i}")).await.unwrap();
        }
    });
}

#[test]
fn should_click_in_a_transformed_iframe_with_force() {
    run(|t| async move {
        click_in_iframe(
            &t,
            "body, html, iframe { margin: 0; padding: 0; border: none; }
             iframe { background: gray; margin-left: 33px; margin-top: 24px; width: 400px; height: 400px; transform: translate(-40px, -40px) scale(0.8); }",
            true,
        )
        .await
    });
}

#[test]
fn should_click_in_a_nested_transformed_iframe() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<style>
                  body, html, iframe { margin: 0; padding: 0; box-sizing: border-box; }
                  iframe { border: 1px solid black; background: gray; margin-left: 33px; margin-top: 24px; width: 400px; height: 400px; transform: scale(0.8); }
                </style>
                <iframe srcdoc="
                  <style>
                    body, html, iframe { margin: 0; padding: 0; box-sizing: border-box; }
                    iframe { border: 3px solid black; background: gray; margin-left: 18px; margin-top: 14px; width: 200px; height: 200px; transform: scale(0.7); }
                  </style>
                  <iframe srcdoc='
                    <style>
                      div { margin-left: 10px; margin-top: 20px; width: 2px; height: 2px; }
                    </style>
                    <div>Target</div>
                  '></iframe>
                "></iframe>"#,
            )
            .await
            .unwrap();
        let locator = t
            .page
            .frame_locator("iframe")
            .frame_locator("iframe")
            .locator("div");
        locator
            .evaluate::<Value>(
                "div => { div.addEventListener('click', () => window.top['_clicked'] = true); }",
            )
            .await
            .unwrap();
        locator.click().await.unwrap();
        assert_eq!(eval(&t.page, "window._clicked").await, true);
    });
}

#[test]
fn should_click_if_opened_select_covers_the_button() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<div>
                  <select>
                    <option>very long text #1</option>
                    <option>very long text #2</option>
                    <option>very long text #3</option>
                    <option>very long text #4</option>
                    <option>very long text #5</option>
                    <option>very long text #6</option>
                  </select>
                </div>
                <div>
                  <button onclick="window.__CLICKED=42">clickme</button>
                </div>"#,
            )
            .await
            .unwrap();
        t.page.click("select").await.unwrap();
        t.page.click("button").await.unwrap();
        assert_eq!(eval(&t.page, "window.__CLICKED").await, 42);
    });
}

#[test]
fn should_fire_contextmenu_event_on_right_click_in_correct_order() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .set_content("<button id=\"target\">Click me</button>")
            .await
            .unwrap();
        eval(
            &t.page,
            "() => { window.__entries = []; const logEvent = e => window.__entries.push(e.type); document.addEventListener('mousedown', logEvent); document.addEventListener('mouseup', logEvent); document.addEventListener('contextmenu', logEvent); }",
        )
        .await;
        t.page
            .get_by_role_with("button", GetByRoleOptions::new().name("Click me"))
            .click()
            .button(MouseButton::Right)
            .await
            .unwrap();
        let expected = if cfg!(windows) {
            json!(["mousedown", "mouseup", "contextmenu"])
        } else {
            json!(["mousedown", "contextmenu", "mouseup"])
        };
        let mut entries = Value::Null;
        for _ in 0..50 {
            entries = eval(&t.page, "window.__entries").await;
            if entries == expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(entries, expected);
    });
}

#[test]
fn should_click_after_a_right_click() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<button>Click me</button>
                <script>
                  const button = document.querySelector('button');
                  button.addEventListener('click', () => button.textContent = 'Clicked!');
                </script>"#,
            )
            .await
            .unwrap();
        t.page
            .get_by_role("button")
            .click()
            .button(MouseButton::Right)
            .await
            .unwrap();
        t.page.get_by_role("button").click().await.unwrap();
        let mut text = None;
        for _ in 0..50 {
            text = t.page.get_by_role("button").text_content().await.unwrap();
            if text.as_deref() == Some("Clicked!") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(text.as_deref(), Some("Clicked!"));
    });
}

#[test]
fn should_set_pointer_event_pressure_on_pointerdown() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<button id="target">Click me</button>
                <script>
                  window['pressures'] = [];
                  document.addEventListener('pointerdown', e => window['pressures'].push(['pointerdown', e.pressure]));
                  document.addEventListener('pointerup', e => window['pressures'].push(['pointerup', e.pressure]));
                </script>"#,
            )
            .await
            .unwrap();
        t.page.click("button").await.unwrap();
        let pressures = eval(&t.page, "() => window['pressures']").await;
        let list = pressures.as_array().unwrap();
        assert!(list.contains(&json!(["pointerdown", 0.5])), "{pressures}");
        assert!(list.contains(&json!(["pointerup", 0])), "{pressures}");
    });
}

#[test]
fn should_set_pointer_event_pressure_on_pointermove() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<body style="margin: 0; padding: 0;">
                  <div id="target" style="width: 500px; height: 500px; background-color: red;"></div>
                  <script>
                    window['pressures'] = [];
                    document.addEventListener('pointermove', e => window['pressures'].push([e.pressure, e.clientX, e.clientY]));
                  </script>
                </body>"#,
            )
            .await
            .unwrap();
        t.page.click("div#target").await.unwrap();
        let mouse = t.page.mouse();
        mouse.r#move(10, 10).await.unwrap();
        mouse.down().await.unwrap();
        mouse.r#move(250, 250).await.unwrap();
        mouse.up().await.unwrap();
        mouse.r#move(50, 50).await.unwrap();
        let pressures = eval(&t.page, "() => window['pressures']").await;
        let list = pressures.as_array().unwrap();
        for expected in [
            json!([0, 250, 250]),
            json!([0, 10, 10]),
            json!([0.5, 250, 250]),
            json!([0, 50, 50]),
        ] {
            assert!(list.contains(&expected), "{expected} not in {pressures}");
        }
    });
}

const SLOTTED: &str = r#"<my-button>
  <template shadowrootmode="open">
    <button><slot></slot></button>
  </template>
  <div>Foo</div>
</my-button>"#;

#[test]
fn should_click_into_shadow_root_with_slotted_div() {
    run(|t| async move {
        t.page.set_content(SLOTTED).await.unwrap();
        t.page
            .get_by_role_with("button", GetByRoleOptions::new().name("Foo"))
            .click()
            .await
            .unwrap();
    });
}

#[test]
fn should_click_shadow_root_button() {
    run(|t| async move {
        t.page.set_content(SLOTTED).await.unwrap();
        t.page.locator("my-button").click().await.unwrap();
    });
}

#[test]
fn should_click_with_tweened_mouse_movement() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<body style="margin: 0; padding: 0; height: 500px; width: 500px;">
                  <div style="position: relative; top: 280px; left: 150px; width: 100px; height: 40px">Click me</div>
                </body>"#,
            )
            .await
            .unwrap();
        t.page.mouse().r#move(100, 100).await.unwrap();
        eval(
            &t.page,
            "() => { window['result'] = []; document.addEventListener('mousemove', event => { window['result'].push([event.clientX, event.clientY]); }); }",
        )
        .await;
        t.page.locator("div").click().steps(5).await.unwrap();
        assert_eq!(
            eval(&t.page, "result").await,
            json!([[120, 140], [140, 180], [160, 220], [180, 260], [200, 300]])
        );
    });
}
