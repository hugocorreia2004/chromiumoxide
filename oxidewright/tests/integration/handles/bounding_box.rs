//! Port of `elementhandle-bounding-box.spec.ts`.
//!
//! All tests ported. EH-001 uses Playwright's `grid.html` styles inline (the
//! local `grid.html` asset uses different box sizes).

use oxidewright::{BoundingBox, ViewportSize};
use serde::Deserialize;

use crate::common::*;

fn bbox(x: f64, y: f64, width: f64, height: f64) -> BoundingBox {
    BoundingBox {
        x,
        y,
        width,
        height,
    }
}

#[test]
fn should_work() {
    run(|t| async move {
        t.page
            .set_viewport_size(ViewportSize {
                width: 500,
                height: 500,
            })
            .await
            .unwrap();
        t.page
            .set_content(
                "<style>body { margin: 0; } .box { font-family: arial; text-align: center; display: inline-block; width: 50px; height: 50px; box-sizing: border-box; border: 1px solid darkgray; } ::-webkit-scrollbar { display: none; }</style>
                <script>for (let i = 0; i < 100; ++i) { const box = document.createElement('div'); box.classList.add('box'); box.textContent = i; document.write(box.outerHTML); }</script>",
            )
            .await
            .unwrap();
        let handle = t
            .page
            .query_selector(".box:nth-of-type(13)")
            .await
            .unwrap()
            .unwrap();
        let b = handle.bounding_box().await.unwrap().unwrap();
        assert_eq!(b, bbox(100.0, 50.0, 50.0, 50.0));
    });
}

#[test]
fn should_handle_nested_frames() {
    run(|t| async move {
        t.page
            .set_viewport_size(ViewportSize {
                width: 616,
                height: 500,
            })
            .await
            .unwrap();
        t.page
            .goto(format!("{}/frames/nested-frames.html", t.server.prefix))
            .await
            .unwrap();
        let nested = t
            .page
            .frame_locator("[name=\"2frames\"]")
            .frame_locator("[name=dos]");
        let handle = nested.locator("div").element_handle().await.unwrap();
        let b = handle.bounding_box().await.unwrap().unwrap();
        // Playwright expects { x: 24, y: 224, width: 268, height: 18 } with its
        // own frame assets; the local ones lay out differently, so compute the
        // expected box from the DOM (iframe boxes + borders + inner rect).
        let expected: Rect = t
            .page
            .evaluate(
                "() => {
                const f1 = document.querySelector('[name=\"2frames\"]');
                const r1 = f1.getBoundingClientRect();
                const f2 = f1.contentDocument.querySelector('[name=dos]');
                const r2 = f2.getBoundingClientRect();
                const r3 = f2.contentDocument.querySelector('div').getBoundingClientRect();
                return {
                  x: r1.x + f1.clientLeft + r2.x + f2.clientLeft + r3.x,
                  y: r1.y + f1.clientTop + r2.y + f2.clientTop + r3.y,
                  width: r3.width,
                  height: r3.height,
                };
              }",
            )
            .await
            .unwrap();
        assert_eq!(
            b,
            bbox(expected.x, expected.y, expected.width, expected.height)
        );
    });
}

#[test]
fn should_get_frame_box() {
    run(|t| async move {
        t.page
            .set_viewport_size(ViewportSize {
                width: 250,
                height: 250,
            })
            .await
            .unwrap();
        t.page
            .set_content(
                "<style>
                body { display: flex; height: 500px; margin: 0px; }
                body iframe { flex-shrink: 1; border: 0; background-color: green; }
                </style>
                <iframe></iframe>",
            )
            .await
            .unwrap();
        let handle = t.page.query_selector("iframe").await.unwrap().unwrap();
        let b = handle.bounding_box().await.unwrap().unwrap();
        assert_eq!(b, bbox(0.0, 0.0, 300.0, 500.0));
    });
}

#[test]
fn should_handle_scroll_offset_and_click() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<style>* { margin: 0; padding: 0; }</style>
                <div style="width:8000px; height:8000px;">
                  <div id=target style="width:20px; height:20px; margin-left:230px; margin-top:340px;"
                    onclick="window.__clicked = true">
                  </div>
                </div>"#,
            )
            .await
            .unwrap();
        let handle = t.page.query_selector("#target").await.unwrap().unwrap();
        let box1 = handle.bounding_box().await.unwrap().unwrap();
        t.page
            .evaluate::<()>("() => window.scrollBy(200, 300)")
            .await
            .unwrap();
        let box2 = handle.bounding_box().await.unwrap().unwrap();
        assert_eq!(box1, bbox(230.0, 340.0, 20.0, 20.0));
        assert_eq!(box2, bbox(30.0, 40.0, 20.0, 20.0));
        t.page
            .mouse()
            .click(box2.x + 10.0, box2.y + 10.0)
            .await
            .unwrap();
        let clicked: bool = t.page.evaluate("() => window.__clicked").await.unwrap();
        assert!(clicked);
    });
}

#[test]
fn should_return_null_for_invisible_elements() {
    run(|t| async move {
        t.page
            .set_content("<div style=\"display:none\">hi</div>")
            .await
            .unwrap();
        let element = t.page.query_selector("div").await.unwrap().unwrap();
        assert_eq!(element.bounding_box().await.unwrap(), None);
    });
}

#[derive(Deserialize)]
struct Point {
    x: f64,
    y: f64,
}

#[test]
fn should_get_bounding_box_of_element_inside_a_cross_origin_iframe() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .set_content(format!(
                r#"<div style="height:120px"></div>
                <iframe style="border:0;margin-left:30px;width:300px;height:200px" src="{}/input/button.html"></iframe>"#,
                t.server.cross_process_prefix
            ))
            .await
            .unwrap();
        let frame = t.page.frames()[1].clone();
        let button = frame.wait_for_selector("button").await.unwrap().unwrap();
        let iframe_box = t
            .page
            .query_selector("iframe")
            .await
            .unwrap()
            .unwrap()
            .bounding_box()
            .await
            .unwrap()
            .unwrap();
        let inner: Point = button
            .evaluate(
                "b => { const r = b.getBoundingClientRect(); return { x: r.left, y: r.top }; }",
            )
            .await
            .unwrap();
        let b = button.bounding_box().await.unwrap().unwrap();
        assert_eq!(b.x.round(), (iframe_box.x + inner.x).round());
        assert_eq!(b.y.round(), (iframe_box.y + inner.y).round());
    });
}

#[test]
fn should_force_a_layout() {
    run(|t| async move {
        t.page
            .set_viewport_size(ViewportSize {
                width: 500,
                height: 500,
            })
            .await
            .unwrap();
        t.page
            .set_content("<div style=\"width: 100px; height: 100px\">hello</div>")
            .await
            .unwrap();
        let handle = t.page.query_selector("div").await.unwrap().unwrap();
        t.page
            .evaluate::<()>("element => { element.style.height = '200px'; }")
            .arg(&handle)
            .await
            .unwrap();
        let b = handle.bounding_box().await.unwrap().unwrap();
        assert_eq!(b, bbox(8.0, 8.0, 100.0, 200.0));
    });
}

#[derive(Deserialize, Debug, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

const WEB_BOX: &str = "e => { const rect = e.getBoundingClientRect(); return { x: rect.x, y: rect.y, width: rect.width, height: rect.height }; }";

#[test]
fn should_work_with_svg_nodes() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="500" height="500">
                <rect id="theRect" x="30" y="50" width="200" height="300"></rect>
                </svg>"#,
            )
            .await
            .unwrap();
        let element = t.page.query_selector("#therect").await.unwrap().unwrap();
        let pw = element.bounding_box().await.unwrap().unwrap();
        let web: Rect = t.page.evaluate(WEB_BOX).arg(&element).await.unwrap();
        assert_eq!(
            Rect {
                x: pw.x,
                y: pw.y,
                width: pw.width,
                height: pw.height
            },
            web
        );
    });
}

#[test]
fn should_work_when_inline_box_child_is_outside_of_viewport() {
    run(|t| async move {
        t.page
            .set_content(
                "<style>
                i { position: absolute; top: -1000px; }
                body { margin: 0; font-size: 12px; }
                </style>
                <span><i>woof</i><b>doggo</b></span>",
            )
            .await
            .unwrap();
        let handle = t.page.query_selector("span").await.unwrap().unwrap();
        let b = handle.bounding_box().await.unwrap().unwrap();
        let web: Rect = handle.evaluate(WEB_BOX).await.unwrap();
        let round = |v: f64| (v * 100.0).round();
        assert_eq!(round(b.x), round(web.x));
        assert_eq!(round(b.y), round(web.y));
        assert_eq!(round(b.width), round(web.width));
        assert_eq!(round(b.height), round(web.height));
    });
}
