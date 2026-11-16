//! Port of `locator-misc-2.spec.ts`.
//!
//! Skipped:
//! - LO-146 `should take screenshot`: element screenshots are not part of
//!   this crate yet.

use oxidewright::{BoundingBox, LocatorOptions, WaitForSelectorState};
use serde_json::Value;

use super::*;
use crate::common::*;

#[test]
fn should_press() {
    run(|t| async move {
        t.page.set_content("<input type='text' />").await.unwrap();
        t.page.locator("input").press("h").await.unwrap();
        let value: String = t
            .page
            .eval_on_selector("input", "input => input.value")
            .await
            .unwrap();
        assert_eq!(value, "h");
    });
}

#[test]
fn should_scroll_into_view() {
    run(|t| async move {
        t.page
            .goto(format!("{}/offscreenbuttons.html", t.server.prefix))
            .await
            .unwrap();
        for i in 0..11 {
            let button = t.page.locator(format!("#btn{i}"));
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
                .evaluate::<Value>("() => window.scrollTo(0, 0)")
                .await
                .unwrap();
        }
    });
}

#[test]
fn should_scroll_zero_sized_element_into_view() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<style>
      html,body { margin: 0; padding: 0; }
      ::-webkit-scrollbar { display: none; }
      * { scrollbar-width: none; }
    </style>
    <div style="height: 2000px; text-align: center; border: 10px solid blue;">
      <h1>SCROLL DOWN</h1>
    </div>
    <div id=lazyload style="font-size:75px; background-color: green;"></div>
    <script>
      const lazyLoadElement = document.querySelector('#lazyload');
      const observer = new IntersectionObserver((entries) => {
        if (entries.some(entry => entry.isIntersecting)) {
          lazyLoadElement.textContent = 'LAZY LOADED CONTENT';
          lazyLoadElement.style.height = '20px';
          observer.disconnect();
        }
      });
      observer.observe(lazyLoadElement);
    </script>"#,
            )
            .await
            .unwrap();
        let lazy = t.page.locator("#lazyload");
        assert_eq!(
            lazy.bounding_box().await.unwrap(),
            Some(BoundingBox {
                x: 0.0,
                y: 2020.0,
                width: 1280.0,
                height: 0.0
            })
        );
        lazy.scroll_into_view_if_needed().await.unwrap();
        expect_text(&lazy, "LAZY LOADED CONTENT").await;
        assert_eq!(
            lazy.bounding_box().await.unwrap(),
            Some(BoundingBox {
                x: 0.0,
                y: 720.0,
                width: 1280.0,
                height: 20.0
            })
        );
    });
}

#[test]
fn should_select_textarea() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        let textarea = t.page.locator("textarea");
        textarea
            .evaluate::<Value>("textarea => textarea.value = 'some value'")
            .await
            .unwrap();
        textarea.select_text().await.unwrap();
        let selection: String = t
            .page
            .evaluate("() => window.getSelection().toString()")
            .await
            .unwrap();
        assert_eq!(selection, "some value");
    });
}

#[test]
fn should_type() {
    run(|t| async move {
        t.page.set_content("<input type='text' />").await.unwrap();
        t.page.locator("input").r#type("hello").await.unwrap();
        let value: String = t
            .page
            .eval_on_selector("input", "input => input.value")
            .await
            .unwrap();
        assert_eq!(value, "hello");
    });
}

#[test]
fn should_press_sequentially() {
    run(|t| async move {
        t.page.set_content("<input type='text' />").await.unwrap();
        t.page
            .locator("input")
            .press_sequentially("hello")
            .await
            .unwrap();
        let value: String = t
            .page
            .eval_on_selector("input", "input => input.value")
            .await
            .unwrap();
        assert_eq!(value, "hello");
    });
}

#[test]
fn should_return_bounding_box() {
    run(|t| async move {
        t.page.set_viewport_size((500, 500)).await.unwrap();
        // Playwright's `grid.html`: 50px border-box boxes in a wrapping flexbox.
        t.page
            .set_content(
                "<style>body { margin: 0; display: flex; flex-wrap: wrap; } .box { width: 50px; height: 50px; box-sizing: border-box; border: 1px solid darkgray; }</style><script>for (let i = 0; i < 200; ++i) { const box = document.createElement('div'); box.classList.add('box'); box.textContent = i; document.write(box.outerHTML); }</script>",
            )
            .await
            .unwrap();
        let element = t.page.locator(".box:nth-of-type(13)");
        assert_eq!(
            element.bounding_box().await.unwrap(),
            Some(BoundingBox {
                x: 100.0,
                y: 50.0,
                width: 50.0,
                height: 50.0
            })
        );
    });
}

#[test]
fn should_wait_for() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let locator = t.page.locator("span");
        let waiting = tokio::spawn(locator.wait_for().into_future());
        t.page
            .eval_on_selector::<Value>("div", "div => div.innerHTML = '<span>target</span>'")
            .await
            .unwrap();
        waiting.await.unwrap().unwrap();
        expect_text(&locator, "target").await;
    });
}

#[test]
fn should_wait_for_hidden() {
    run(|t| async move {
        t.page
            .set_content("<div><span>target</span></div>")
            .await
            .unwrap();
        let locator = t.page.locator("span");
        let waiting = tokio::spawn(
            locator
                .wait_for()
                .state(WaitForSelectorState::Hidden)
                .into_future(),
        );
        t.page
            .eval_on_selector::<Value>("div", "div => div.innerHTML = ''")
            .await
            .unwrap();
        waiting.await.unwrap().unwrap();
    });
}

const ITEMS: &str = r#"<div>
  <div class="item" style="display: none">Hidden data0</div>
  <div class="item">visible data1</div>
  <div class="item" style="display: none">Hidden data1</div>
  <div class="item">visible data2</div>
  <div class="item" style="display: none">Hidden data2</div>
  <div class="item">visible data3</div>
  </div>"#;

#[test]
fn should_combine_visible_with_other_selectors() {
    run(|t| async move {
        t.page
            .set_content(ITEMS.replace("Hidden data2", "Hidden data1"))
            .await
            .unwrap();
        let locator = t.page.locator(".item >> visible=true").nth(1);
        expect_text(&locator, "visible data2").await;
        expect_text(
            &t.page.locator(".item >> visible=true >> text=data3"),
            "visible data3",
        )
        .await;
    });
}

#[test]
fn should_support_filter_visible() {
    run(|t| async move {
        t.page.set_content(ITEMS).await.unwrap();
        let visible = LocatorOptions::new().visible(true);
        let locator = t.page.locator(".item").filter(visible.clone()).nth(1);
        expect_text(&locator, "visible data2").await;
        expect_text(
            &t.page.locator(".item").filter(visible).get_by_text("data3"),
            "visible data3",
        )
        .await;
        expect_text(
            &t.page
                .locator(".item")
                .filter(LocatorOptions::new().visible(false))
                .get_by_text("data1"),
            "Hidden data1",
        )
        .await;
    });
}

#[test]
fn should_support_visible() {
    run(|t| async move {
        t.page.set_content(ITEMS).await.unwrap();
        let locator = t.page.locator(".item").visible().nth(1);
        expect_text(&locator, "visible data2").await;
        expect_text(
            &t.page.locator(".item").visible().get_by_text("data3"),
            "visible data3",
        )
        .await;
        expect_count(&t.page.locator(".item").visible(), 3).await;
    });
}

#[test]
fn locator_count_should_work_with_deleted_map_in_main_world() {
    run(|t| async move {
        t.page.evaluate::<Value>("Map = 1").await.unwrap();
        t.page
            .locator("#searchResultTableDiv .x-grid3-row")
            .count()
            .await
            .unwrap();
        expect_count(&t.page.locator("#searchResultTableDiv .x-grid3-row"), 0).await;
    });
}

#[test]
fn locator_locator_and_frame_locator_locator_should_accept_locator() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"<div><input value=outer></div>
    <iframe srcdoc="<div><input value=inner></div>"></iframe>"#,
        )
        .await
        .unwrap();
        let input_locator = p.locator("input");
        assert_eq!(input_locator.input_value().await.unwrap(), "outer");
        assert_eq!(
            p.locator("div")
                .locator(&input_locator)
                .input_value()
                .await
                .unwrap(),
            "outer"
        );
        assert_eq!(
            p.frame_locator("iframe")
                .locator(&input_locator)
                .input_value()
                .await
                .unwrap(),
            "inner"
        );
        assert_eq!(
            p.frame_locator("iframe")
                .locator("div")
                .locator(&input_locator)
                .input_value()
                .await
                .unwrap(),
            "inner"
        );
        let div_locator = p.locator("div");
        assert_eq!(
            div_locator.locator("input").input_value().await.unwrap(),
            "outer"
        );
        assert_eq!(
            p.frame_locator("iframe")
                .locator(&div_locator)
                .locator("input")
                .input_value()
                .await
                .unwrap(),
            "inner"
        );
    });
}

#[test]
fn should_fill_programmatically_enabled_textarea() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<button>Enable</button>
    <form>
      <textarea id="text" disabled></textarea>
    </form>
    <script>
      document.querySelector('button').addEventListener('click', () => {
        document.querySelector('#text').disabled = false;
      });
    </script>"#,
            )
            .await
            .unwrap();
        t.page.locator("button").click().await.unwrap();
        t.page.locator("#text").fill("Hello").await.unwrap();
        expect_value(&t.page.locator("#text"), "Hello").await;
    });
}

#[test]
fn press_should_throw_on_unknown_keys() {
    run(|t| async move {
        t.page
            .set_content("<input type='text' value='hello' />")
            .await
            .unwrap();
        let locator = t.page.get_by_role("textbox");
        for key in ["NotARealKey", "ё", "😊"] {
            let error = locator.press(key).await.unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(&format!("Unknown key: \"{key}\"")),
                "{error}"
            );
        }
    });
}

use std::future::IntoFuture;
