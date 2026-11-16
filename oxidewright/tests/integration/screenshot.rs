//! Screenshots, ported from Playwright's `tests/page/page-screenshot.spec.ts`,
//! `tests/page/elementhandle-screenshot.spec.ts` and
//! `tests/library/screenshot.spec.ts`.
//!
//! Golden images are not compared pixel by pixel: the ports check the image
//! format, dimensions and a few pixels (decoded by the browser itself).
//!
//! Not ported:
//! * SS-002..SS-004 (blinking caret: need keyboard input), SS-022, SS-024,
//!   SS-025, SS-095, SS-026, SS-028, SS-038, SS-066, SS-067 (pure golden
//!   comparisons of rendering details).
//! * SS-045 (parallel screenshots of one page with masks: golden comparison).
//! * SS-055, SS-058..SS-063 (test hooks / event ordering of animations).
//! * SS-065, SS-077 (huge pages, slow and platform dependent).
//! * EH-076 (needs `page.$`), EH-078/EH-079 (golden comparisons of huge elements).

use oxidewright::{Animations, ElementHandle, Mask, Page, Rect, ScreenshotScale, ScreenshotType};
use serde_json::Value;

use crate::common::*;

struct Decoded {
    width: u32,
    height: u32,
    pixels: Vec<[u8; 4]>,
}

fn mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        panic!("unknown image format")
    }
}

/// Decode an image in a scratch page and read some pixels.
async fn decode(
    context: &oxidewright::BrowserContext,
    bytes: &[u8],
    points: &[(u32, u32)],
) -> Decoded {
    use base64::Engine;
    let page = context.new_page().await.unwrap();
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    let result: Value = page
        .evaluate(
            "async ([src, points]) => {
              const img = new Image();
              img.src = src;
              await img.decode();
              const canvas = document.createElement('canvas');
              canvas.width = img.naturalWidth;
              canvas.height = img.naturalHeight;
              const ctx = canvas.getContext('2d');
              ctx.drawImage(img, 0, 0);
              return {
                width: img.naturalWidth,
                height: img.naturalHeight,
                pixels: points.map(([x, y]) => Array.from(ctx.getImageData(x, y, 1, 1).data)),
              };
            }",
        )
        .arg((format!("data:{};base64,{data}", mime(bytes)), points))
        .await
        .unwrap();
    page.close().await.unwrap();
    Decoded {
        width: result["width"].as_u64().unwrap() as u32,
        height: result["height"].as_u64().unwrap() as u32,
        pixels: serde_json::from_value(result["pixels"].clone()).unwrap(),
    }
}

async fn element(page: &Page, selector: &str) -> ElementHandle {
    page.evaluate_handle("s => document.querySelector(s)")
        .arg(selector)
        .await
        .unwrap()
        .as_element()
        .unwrap()
}

/// The default page background follows the OS color scheme when nothing is
/// emulated; make it white.
async fn light(page: &Page) {
    page.emulate_media()
        .color_scheme(oxidewright::ColorScheme::Light)
        .await
        .unwrap();
}

async fn grid(t: &TestContext) {
    t.page.set_viewport_size((500, 500)).await.unwrap();
    t.page
        .goto(format!("{}/grid.html", t.server.prefix))
        .await
        .unwrap();
}

fn temp_path(t: &TestContext, name: &str) -> std::path::PathBuf {
    std::env::temp_dir()
        .join(format!("oxidewright-screenshots-{}", t.server.port))
        .join(name)
}

#[test]
fn ss_001_should_work() {
    run(|t| async move {
        grid(&t).await;
        let png = t.page.screenshot().await.unwrap();
        assert_eq!(mime(&png), "image/png");
        let decoded = decode(&t.context, &png, &[(0, 0), (25, 25)]).await;
        assert_eq!((decoded.width, decoded.height), (500, 500));
        // The top-left box border is black.
        assert_eq!(decoded.pixels[0], [0, 0, 0, 255]);
    });
}

#[test]
fn ss_005_should_clip_rect() {
    run(|t| async move {
        grid(&t).await;
        let png = t
            .page
            .screenshot()
            .clip(Rect::new(50.0, 100.0, 150.0, 100.0))
            .await
            .unwrap();
        let decoded = decode(&t.context, &png, &[]).await;
        assert_eq!((decoded.width, decoded.height), (150, 100));
    });
}

#[test]
fn ss_006_should_clip_rect_with_full_page() {
    run(|t| async move {
        grid(&t).await;
        t.page
            .evaluate::<()>("() => window.scrollBy(150, 200)")
            .await
            .unwrap();
        let png = t
            .page
            .screenshot()
            .full_page(true)
            .clip(Rect::new(50.0, 100.0, 150.0, 100.0))
            .await
            .unwrap();
        let decoded = decode(&t.context, &png, &[]).await;
        assert_eq!((decoded.width, decoded.height), (150, 100));
    });
}

#[test]
fn ss_007_should_clip_elements_to_the_viewport() {
    run(|t| async move {
        grid(&t).await;
        let png = t
            .page
            .screenshot()
            .clip(Rect::new(50.0, 450.0, 1000.0, 100.0))
            .await
            .unwrap();
        let decoded = decode(&t.context, &png, &[]).await;
        assert_eq!((decoded.width, decoded.height), (450, 50));
    });
}

#[test]
fn ss_008_should_throw_on_clip_outside_the_viewport() {
    run(|t| async move {
        grid(&t).await;
        assert_err_contains(
            t.page
                .screenshot()
                .clip(Rect::new(50.0, 650.0, 100.0, 100.0))
                .await,
            "Clipped area is either empty or outside the resulting image",
        );
    });
}

#[test]
fn ss_009_should_throw_on_a_negative_clip_size() {
    run(|t| async move {
        assert_err_contains(
            t.page
                .screenshot()
                .clip(Rect::new(0.0, 0.0, -1.0, -1.0))
                .await,
            "Expected options.clip.width to be greater than 0.",
        );
    });
}

#[test]
fn ss_010_should_run_in_parallel() {
    run(|t| async move {
        grid(&t).await;
        let shots = futures::future::join_all((0..3).map(|i| {
            t.page
                .screenshot()
                .clip(Rect::new(50.0 * i as f64, 0.0, 50.0, 50.0))
                .into_future()
        }))
        .await;
        for shot in shots {
            let decoded = decode(&t.context, &shot.unwrap(), &[]).await;
            assert_eq!((decoded.width, decoded.height), (50, 50));
        }
    });
}

#[test]
fn ss_011_should_take_full_page_screenshots() {
    run(|t| async move {
        grid(&t).await;
        let size: (u32, u32) = t
            .page
            .evaluate("() => [document.documentElement.scrollWidth, document.documentElement.scrollHeight]")
            .await
            .unwrap();
        assert!(size.1 > 500);
        let png = t.page.screenshot().full_page(true).await.unwrap();
        let decoded = decode(&t.context, &png, &[]).await;
        assert_eq!((decoded.width, decoded.height), size);
    });
}

#[test]
fn ss_013_should_restore_viewport_after_full_page_screenshot() {
    run(|t| async move {
        grid(&t).await;
        t.page.screenshot().full_page(true).await.unwrap();
        assert_eq!(t.page.viewport_size(), Some((500, 500).into()));
        let inner: (u32, u32) = t
            .page
            .evaluate("() => [window.innerWidth, window.innerHeight]")
            .await
            .unwrap();
        assert_eq!(inner, (500, 500));
    });
}

#[test]
fn ss_014_should_allow_transparency() {
    run(|t| async move {
        t.page.set_viewport_size((50, 150)).await.unwrap();
        light(&t.page).await;
        t.page
            .set_content("<style>body { margin: 0 }</style><div style=\"width:50px; height:50px; background:red;\"></div><div style=\"width:50px; height:50px; background:blue;\"></div>")
            .await
            .unwrap();
        let png = t.page.screenshot().omit_background(true).await.unwrap();
        let decoded = decode(&t.context, &png, &[(25, 25), (25, 75), (25, 125)]).await;
        assert_eq!(decoded.pixels[0], [255, 0, 0, 255]);
        assert_eq!(decoded.pixels[1], [0, 0, 255, 255]);
        assert_eq!(decoded.pixels[2][3], 0, "transparent background");
        // The background is restored afterwards.
        let png = t.page.screenshot().await.unwrap();
        let decoded = decode(&t.context, &png, &[(25, 125)]).await;
        assert_eq!(decoded.pixels[0], [255, 255, 255, 255]);
    });
}

#[test]
fn ss_015_should_render_white_background_on_jpeg_file() {
    run(|t| async move {
        t.page.set_viewport_size((100, 100)).await.unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        let jpeg = t
            .page
            .screenshot()
            .omit_background(true)
            .r#type(ScreenshotType::Jpeg)
            .await
            .unwrap();
        assert_eq!(mime(&jpeg), "image/jpeg");
        let decoded = decode(&t.context, &jpeg, &[(50, 50)]).await;
        let [r, g, b, a] = decoded.pixels[0];
        assert!(r > 250 && g > 250 && b > 250 && a == 255);
    });
}

#[test]
fn ss_016_should_produce_a_valid_webp_screenshot() {
    run(|t| async move {
        grid(&t).await;
        let webp = t
            .page
            .screenshot()
            .r#type(ScreenshotType::Webp)
            .await
            .unwrap();
        assert_eq!(mime(&webp), "image/webp");
        let decoded = decode(&t.context, &webp, &[]).await;
        assert_eq!((decoded.width, decoded.height), (500, 500));
    });
}

#[test]
fn ss_017_path_option_should_detect_webp() {
    run(|t| async move {
        grid(&t).await;
        let path = temp_path(&t, "screenshot.webp");
        t.page.screenshot().path(&path).await.unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(mime(&bytes), "image/webp");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    });
}

#[test]
fn ss_018_quality_option_should_work_for_webp() {
    run(|t| async move {
        grid(&t).await;
        let webp = t
            .page
            .screenshot()
            .r#type(ScreenshotType::Webp)
            .quality(50)
            .await
            .unwrap();
        assert_eq!(mime(&webp), "image/webp");
    });
}

#[test]
fn ss_021_quality_option_should_throw_for_webp_when_out_of_range() {
    run(|t| async move {
        assert_err_contains(
            t.page
                .screenshot()
                .r#type(ScreenshotType::Webp)
                .quality(101)
                .await,
            "Expected options.quality to be between 0 and 100 (inclusive), got 101",
        );
    });
}

#[test]
fn ss_023_should_work_for_canvas() {
    run(|t| async move {
        t.page.set_viewport_size((100, 100)).await.unwrap();
        t.page
            .set_content("<style>body { margin: 0 }</style><canvas width=100 height=100></canvas><script>const ctx = document.querySelector('canvas').getContext('2d'); ctx.fillStyle = 'rgb(0, 128, 0)'; ctx.fillRect(0, 0, 100, 100);</script>")
            .await
            .unwrap();
        let png = t.page.screenshot().await.unwrap();
        let decoded = decode(&t.context, &png, &[(50, 50)]).await;
        assert_eq!(decoded.pixels[0], [0, 128, 0, 255]);
    });
}

#[test]
fn ss_027_should_work_while_navigating() {
    run(|t| async move {
        t.page.set_viewport_size((500, 500)).await.unwrap();
        t.page
            .goto(format!("{}/redirectloop1.html", t.server.prefix))
            .await
            .ok();
        for _ in 0..5 {
            match t.page.screenshot().full_page(true).await {
                Ok(png) => assert_eq!(mime(&png), "image/png"),
                Err(err) => assert!(
                    err.to_string().contains("Execution context was destroyed")
                        || err.to_string().contains("Target"),
                    "{err}"
                ),
            }
        }
    });
}

#[test]
fn ss_029_path_option_should_work() {
    run(|t| async move {
        grid(&t).await;
        let path = temp_path(&t, "screenshot.png");
        let png = t.page.screenshot().path(&path).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), png);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    });
}

#[test]
fn ss_030_path_option_should_create_subdirectories() {
    run(|t| async move {
        grid(&t).await;
        let path = temp_path(&t, "these/are/directories/screenshot.png");
        t.page.screenshot().path(&path).await.unwrap();
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(temp_path(&t, ""));
    });
}

#[test]
fn ss_031_path_option_should_detect_jpeg() {
    run(|t| async move {
        grid(&t).await;
        let path = temp_path(&t, "screenshot.jpg");
        let bytes = t.page.screenshot().path(&path).await.unwrap();
        assert_eq!(mime(&bytes), "image/jpeg");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    });
}

#[test]
fn ss_032_path_option_should_throw_for_unsupported_mime_type() {
    run(|t| async move {
        assert_err_contains(
            t.page.screenshot().path("file.txt").await,
            "path: unsupported mime type \"text/plain\"",
        );
    });
}

#[test]
fn ss_033_quality_option_should_throw_for_png() {
    run(|t| async move {
        assert_err_contains(
            t.page.screenshot().quality(10).await,
            "options.quality is unsupported for the png",
        );
    });
}

#[test]
fn ss_034_zero_quality_option_should_throw_for_png() {
    run(|t| async move {
        assert_err_contains(
            t.page
                .screenshot()
                .quality(0)
                .r#type(ScreenshotType::Png)
                .await,
            "options.quality is unsupported for the png",
        );
    });
}

#[test]
fn ss_035_quality_option_should_work_for_jpeg() {
    run(|t| async move {
        grid(&t).await;
        let low = t
            .page
            .screenshot()
            .r#type(ScreenshotType::Jpeg)
            .quality(10)
            .await
            .unwrap();
        let high = t
            .page
            .screenshot()
            .r#type(ScreenshotType::Jpeg)
            .quality(100)
            .await
            .unwrap();
        assert_eq!(mime(&low), "image/jpeg");
        assert!(low.len() < high.len());
    });
}

#[test]
fn ss_036_should_prefer_type_over_extension() {
    run(|t| async move {
        grid(&t).await;
        let path = temp_path(&t, "file.png");
        let bytes = t
            .page
            .screenshot()
            .path(&path)
            .r#type(ScreenshotType::Jpeg)
            .await
            .unwrap();
        assert_eq!(mime(&bytes), "image/jpeg");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    });
}

#[test]
fn ss_037_should_not_issue_resize_event() {
    run(|t| async move {
        grid(&t).await;
        t.page
            .evaluate::<()>("() => { window.__resized = false; window.addEventListener('resize', () => window.__resized = true); }")
            .await
            .unwrap();
        t.page.screenshot().await.unwrap();
        let resized: bool = t.page.evaluate("() => window.__resized").await.unwrap();
        assert!(!resized);
    });
}

#[test]
fn ss_040_mask_should_work() {
    run(|t| async move {
        grid(&t).await;
        let png = t
            .page
            .screenshot()
            .mask([Mask::page(&t.page, "div >> nth=5")])
            .await
            .unwrap();
        // Box 5 is the sixth inline box of the first row, at x = 5 * 52.
        let decoded = decode(&t.context, &png, &[(5 * 52 + 26, 26)]).await;
        assert_eq!(decoded.pixels[0], [255, 0, 255, 255]);
        // The mask is removed afterwards.
        let png = t.page.screenshot().await.unwrap();
        let decoded = decode(&t.context, &png, &[(5 * 52 + 26, 26)]).await;
        assert_ne!(decoded.pixels[0], [255, 0, 255, 255]);
    });
}

#[test]
fn ss_046_should_work_with_mask_color() {
    run(|t| async move {
        grid(&t).await;
        let png = t
            .page
            .screenshot()
            .mask([Mask::page(&t.page, "div >> nth=5")])
            .mask_color("#00FF00")
            .await
            .unwrap();
        let decoded = decode(&t.context, &png, &[(5 * 52 + 26, 26)]).await;
        assert_eq!(decoded.pixels[0], [0, 255, 0, 255]);
    });
}

#[test]
fn ss_049_should_hide_elements_based_on_attr_with_style() {
    run(|t| async move {
        t.page.set_viewport_size((100, 100)).await.unwrap();
        light(&t.page).await;
        t.page
            .set_content("<style>body { margin: 0 }</style><div style=\"width:100px; height:100px; background:red;\"></div>")
            .await
            .unwrap();
        let png = t
            .page
            .screenshot()
            .style("div { visibility: hidden; }")
            .await
            .unwrap();
        let decoded = decode(&t.context, &png, &[(50, 50)]).await;
        assert_eq!(decoded.pixels[0], [255, 255, 255, 255]);
        let visible: String = t
            .page
            .evaluate("() => getComputedStyle(document.querySelector('div')).visibility")
            .await
            .unwrap();
        assert_eq!(visible, "visible");
    });
}

const ROTATE_Z: &str = "<style>
  @keyframes z-rotate { from { transform: rotateZ(0deg); } to { transform: rotateZ(359deg); } }
  body { margin: 0; }
  div { width: 50px; height: 50px; margin: 25px; background: green; animation: 1s linear infinite z-rotate; }
</style><div></div>";

#[test]
fn ss_052_should_not_capture_infinite_css_animation() {
    run(|t| async move {
        t.page.set_viewport_size((100, 100)).await.unwrap();
        t.page.set_content(ROTATE_Z).await.unwrap();
        let first = t
            .page
            .screenshot()
            .animations(Animations::Disabled)
            .await
            .unwrap();
        for _ in 0..3 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let next = t
                .page
                .screenshot()
                .animations(Animations::Disabled)
                .await
                .unwrap();
            assert_eq!(next, first);
        }
    });
}

#[test]
fn ss_056_should_resume_infinite_animations() {
    run(|t| async move {
        t.page.set_viewport_size((100, 100)).await.unwrap();
        t.page.set_content(ROTATE_Z).await.unwrap();
        t.page
            .screenshot()
            .animations(Animations::Disabled)
            .await
            .unwrap();
        let running: usize = t
            .page
            .evaluate(
                "() => document.getAnimations().filter(a => a.playState === 'running').length",
            )
            .await
            .unwrap();
        assert_eq!(running, 1);
    });
}

#[test]
fn ss_064_should_wait_for_fonts_to_load() {
    run(|t| async move {
        t.page
            .set_content("<style>@font-face { font-family: 'x'; src: url('/does-not-exist.woff2'); } body { font-family: 'x'; }</style>hello")
            .await
            .unwrap();
        let png = t.page.screenshot().await.unwrap();
        assert_eq!(mime(&png), "image/png");
    });
}

#[test]
fn ss_074_should_work_with_device_scale_factor() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .viewport(Some((320, 480).into()))
            .device_scale_factor(2.0)
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(format!("{}/grid.html", t.server.prefix))
            .await
            .unwrap();
        let png = page.screenshot().await.unwrap();
        let decoded = decode(&t.context, &png, &[]).await;
        assert_eq!((decoded.width, decoded.height), (640, 960));
        let css = page.screenshot().scale(ScreenshotScale::Css).await.unwrap();
        let decoded = decode(&t.context, &css, &[]).await;
        assert_eq!((decoded.width, decoded.height), (320, 480));
        context.close().await.unwrap();
    });
}

#[test]
fn ss_075_should_produce_screenshot_of_correct_size_with_scale_css_and_null_viewport() {
    run(|t| async move {
        let context = t.browser.new_context().viewport(None).await.unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(format!("{}/grid.html", t.server.prefix))
            .await
            .unwrap();
        let inner: (u32, u32) = page
            .evaluate("() => [window.innerWidth, window.innerHeight]")
            .await
            .unwrap();
        let png = page.screenshot().scale(ScreenshotScale::Css).await.unwrap();
        let decoded = decode(&t.context, &png, &[]).await;
        assert_eq!((decoded.width, decoded.height), inner);
        context.close().await.unwrap();
    });
}

#[test]
fn ss_076_should_work_with_device_scale_factor_clip_and_scale_css() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .viewport(Some((500, 500).into()))
            .device_scale_factor(3.0)
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(format!("{}/grid.html", t.server.prefix))
            .await
            .unwrap();
        let png = page
            .screenshot()
            .clip(Rect::new(50.0, 100.0, 150.0, 100.0))
            .scale(ScreenshotScale::Css)
            .await
            .unwrap();
        let decoded = decode(&t.context, &png, &[]).await;
        assert_eq!((decoded.width, decoded.height), (150, 100));
        context.close().await.unwrap();
    });
}

#[test]
fn should_not_bring_the_page_to_front() {
    run(|t| async move {
        grid(&t).await;
        let other = t.context.new_page().await.unwrap();
        other.goto(&t.server.empty_page).await.unwrap();
        let png = t.page.screenshot().await.unwrap();
        let decoded = decode(&t.context, &png, &[]).await;
        assert_eq!((decoded.width, decoded.height), (500, 500));
    });
}

// ----------------------------------------------------------------------
// element screenshots
// ----------------------------------------------------------------------

#[test]
fn eh_074_should_work() {
    run(|t| async move {
        grid(&t).await;
        t.page
            .evaluate::<()>("() => window.scrollBy(50, 100)")
            .await
            .unwrap();
        let handle = element(&t.page, ".box:nth-of-type(3)").await;
        let png = handle.screenshot().await.unwrap();
        let decoded = decode(&t.context, &png, &[(0, 0), (26, 26)]).await;
        assert_eq!((decoded.width, decoded.height), (52, 52));
        assert_eq!(decoded.pixels[0], [0, 0, 0, 255]);
    });
}

#[test]
fn eh_075_should_work_with_webp() {
    run(|t| async move {
        grid(&t).await;
        let handle = element(&t.page, ".box:nth-of-type(3)").await;
        let webp = handle
            .screenshot()
            .r#type(ScreenshotType::Webp)
            .await
            .unwrap();
        let png = handle.screenshot().await.unwrap();
        let a = decode(&t.context, &webp, &[(0, 0), (10, 40)]).await;
        let b = decode(&t.context, &png, &[(0, 0), (10, 40)]).await;
        assert_eq!((a.width, a.height), (b.width, b.height));
        assert_eq!(a.pixels, b.pixels);
    });
}

#[test]
fn eh_077_should_take_into_account_padding_and_border() {
    run(|t| async move {
        t.page.set_viewport_size((500, 500)).await.unwrap();
        t.page
            .set_content("<div style=\"height: 14px\">oooo</div><style>div { border: 2px solid blue; background: green; width: 50px; height: 50px; }</style><div id=\"d\"></div>")
            .await
            .unwrap();
        let handle = element(&t.page, "div#d").await;
        let png = handle.screenshot().await.unwrap();
        let decoded = decode(&t.context, &png, &[(0, 0), (27, 27)]).await;
        assert_eq!((decoded.width, decoded.height), (54, 54));
        assert_eq!(decoded.pixels[0], [0, 0, 255, 255]);
        assert_eq!(decoded.pixels[1], [0, 128, 0, 255]);
    });
}

#[test]
fn eh_080_should_scroll_element_into_view() {
    run(|t| async move {
        t.page.set_viewport_size((500, 500)).await.unwrap();
        t.page
            .set_content("<div style=\"height: 14px\">oooo</div><style>div.above { border: 2px solid blue; background: red; height: 1500px; } div.to-screenshot { border: 2px solid blue; background: green; width: 50px; height: 50px; }</style><div class=\"above\"></div><div class=\"to-screenshot\"></div>")
            .await
            .unwrap();
        let handle = element(&t.page, "div.to-screenshot").await;
        let png = handle.screenshot().await.unwrap();
        let decoded = decode(&t.context, &png, &[(27, 27)]).await;
        assert_eq!((decoded.width, decoded.height), (54, 54));
        assert_eq!(decoded.pixels[0], [0, 128, 0, 255]);
    });
}

#[test]
fn eh_081_should_scroll_15000px_into_view() {
    run(|t| async move {
        t.page.set_viewport_size((500, 500)).await.unwrap();
        t.page
            .set_content("<div style=\"height: 14px\">oooo</div><style>div.above { border: 2px solid blue; background: red; height: 15000px; } div.to-screenshot { border: 2px solid blue; background: green; width: 50px; height: 50px; }</style><div class=\"above\"></div><div class=\"to-screenshot\"></div>")
            .await
            .unwrap();
        let handle = element(&t.page, "div.to-screenshot").await;
        let png = handle.screenshot().await.unwrap();
        let decoded = decode(&t.context, &png, &[(27, 27)]).await;
        assert_eq!((decoded.width, decoded.height), (54, 54));
        assert_eq!(decoded.pixels[0], [0, 128, 0, 255]);
    });
}

#[test]
fn eh_082_should_work_with_a_rotated_element() {
    run(|t| async move {
        t.page.set_viewport_size((500, 500)).await.unwrap();
        t.page
            .set_content("<div style=\"position:absolute; top: 100px; left: 100px; width: 100px; height: 100px; background: green; transform: rotateZ(200deg);\">&nbsp;</div>")
            .await
            .unwrap();
        let handle = element(&t.page, "div").await;
        let png = handle.screenshot().await.unwrap();
        let decoded = decode(&t.context, &png, &[]).await;
        // The bounding box of the rotated square is larger than 100x100.
        assert!(decoded.width > 100 && decoded.height > 100);
    });
}

#[test]
fn eh_083_should_fail_to_screenshot_a_detached_element() {
    run(|t| async move {
        t.page.set_content("<h1>remove this</h1>").await.unwrap();
        let handle = element(&t.page, "h1").await;
        t.page
            .evaluate::<()>("element => element.remove()")
            .arg(&handle)
            .await
            .unwrap();
        assert_err_contains(
            handle.screenshot().await,
            "Element is not attached to the DOM",
        );
    });
}

#[test]
fn eh_084_should_timeout_waiting_for_visible() {
    run(|t| async move {
        t.page
            .set_content("<div style=\"width: 50px; height: 0\"></div>")
            .await
            .unwrap();
        let div = element(&t.page, "div").await;
        let err = assert_err_contains(
            div.screenshot().timeout(3000).await,
            "elementHandle.screenshot: Timeout 3000ms exceeded",
        );
        assert!(err.to_string().contains("element is not visible"), "{err}");
    });
}

#[test]
fn eh_085_should_wait_for_visible() {
    run(|t| async move {
        grid(&t).await;
        let handle = element(&t.page, ".box:nth-of-type(3)").await;
        handle
            .evaluate::<()>("e => e.style.visibility = 'hidden'")
            .await
            .unwrap();
        let shot = tokio::spawn(handle.screenshot().into_future());
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(!shot.is_finished());
        handle
            .evaluate::<()>("e => e.style.visibility = 'visible'")
            .await
            .unwrap();
        let png = shot.await.unwrap().unwrap();
        let decoded = decode(&t.context, &png, &[]).await;
        assert_eq!((decoded.width, decoded.height), (52, 52));
    });
}

#[test]
fn eh_086_should_work_for_an_element_with_fractional_dimensions() {
    run(|t| async move {
        t.page
            .set_content(
                "<div style=\"width:48.51px;height:19.8px;border:1px solid black;\"></div>",
            )
            .await
            .unwrap();
        let handle = element(&t.page, "div").await;
        let png = handle.screenshot().await.unwrap();
        let decoded = decode(&t.context, &png, &[]).await;
        assert_eq!((decoded.width, decoded.height), (51, 22));
    });
}

#[test]
fn eh_087_should_work_for_an_element_with_an_offset() {
    run(|t| async move {
        t.page
            .set_content("<div style=\"position:absolute; top: 10.3px; left: 20.4px;width:50.3px;height:20.2px;border:1px solid black;\"></div>")
            .await
            .unwrap();
        let handle = element(&t.page, "div").await;
        let png = handle.screenshot().await.unwrap();
        let decoded = decode(&t.context, &png, &[]).await;
        assert_eq!((decoded.width, decoded.height), (53, 23));
    });
}

#[test]
fn eh_089_should_take_screenshot_of_disabled_button() {
    run(|t| async move {
        t.page.set_viewport_size((500, 500)).await.unwrap();
        t.page
            .set_content("<button disabled>Click me</button>")
            .await
            .unwrap();
        let button = element(&t.page, "button").await;
        let png = button.screenshot().await.unwrap();
        assert_eq!(mime(&png), "image/png");
    });
}

#[test]
fn eh_090_path_option_should_create_subdirectories() {
    run(|t| async move {
        grid(&t).await;
        let handle = element(&t.page, ".box:nth-of-type(3)").await;
        let path = temp_path(&t, "these/are/directories/screenshot.png");
        handle.screenshot().path(&path).await.unwrap();
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(temp_path(&t, ""));
    });
}

#[test]
fn eh_092_should_not_issue_resize_event() {
    run(|t| async move {
        grid(&t).await;
        t.page
            .evaluate::<()>("() => { window.__resized = false; window.addEventListener('resize', () => window.__resized = true); }")
            .await
            .unwrap();
        let handle = element(&t.page, ".box:nth-of-type(3)").await;
        handle.screenshot().await.unwrap();
        let resized: bool = t.page.evaluate("() => window.__resized").await.unwrap();
        assert!(!resized);
    });
}

#[test]
fn element_screenshot_in_iframe() {
    run(|t| async move {
        t.page.set_viewport_size((500, 500)).await.unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        let frame = attach_frame(&t.page, "f", &format!("{}/grid.html", t.server.prefix)).await;
        let handle = frame
            .evaluate_handle("() => document.querySelector('.box:nth-of-type(3)')")
            .await
            .unwrap()
            .as_element()
            .unwrap();
        let png = handle.screenshot().await.unwrap();
        let decoded = decode(&t.context, &png, &[(0, 0)]).await;
        assert_eq!((decoded.width, decoded.height), (52, 52));
        assert_eq!(decoded.pixels[0], [0, 0, 0, 255]);
    });
}

use std::future::IntoFuture;

/// Center of the `index`-th `div` of `frame`, in its viewport coordinates.
async fn div_center(frame: &oxidewright::Frame, index: usize) -> (u32, u32) {
    let (x, y): (f64, f64) = frame
        .evaluate(
            "i => { const r = document.querySelectorAll('div')[i].getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; }",
        )
        .arg(index)
        .await
        .unwrap();
    (x as u32, y as u32)
}

const MAGENTA: [u8; 4] = [255, 0, 255, 255];

#[test]
fn ss_041_mask_should_work_with_locator() {
    run(|t| async move {
        grid(&t).await;
        let body = t.page.locator("body");
        let png = body
            .screenshot()
            .mask([t.page.locator("div").nth(5)])
            .await
            .unwrap();
        // The body starts at the page origin (margin: 0).
        let point = div_center(&t.page.main_frame(), 5).await;
        let decoded = decode(&t.context, &png, &[point, (2, 2)]).await;
        assert_eq!(decoded.pixels[0], MAGENTA);
        assert_ne!(decoded.pixels[1], MAGENTA);
    });
}

#[test]
fn ss_042_mask_should_work_with_elementhandle() {
    run(|t| async move {
        grid(&t).await;
        let body = element(&t.page, "body").await;
        let png = body
            .screenshot()
            .mask([&t.page.locator("div").nth(5)])
            .await
            .unwrap();
        let point = div_center(&t.page.main_frame(), 5).await;
        let decoded = decode(&t.context, &png, &[point]).await;
        assert_eq!(decoded.pixels[0], MAGENTA);
    });
}

#[test]
fn ss_043_should_mask_multiple_elements() {
    run(|t| async move {
        grid(&t).await;
        let png = t
            .page
            .screenshot()
            .mask([t.page.locator("div").nth(5), t.page.locator("div").nth(12)])
            .await
            .unwrap();
        let a = div_center(&t.page.main_frame(), 5).await;
        let b = div_center(&t.page.main_frame(), 12).await;
        let c = div_center(&t.page.main_frame(), 6).await;
        let decoded = decode(&t.context, &png, &[a, b, c]).await;
        assert_eq!(decoded.pixels[0], MAGENTA);
        assert_eq!(decoded.pixels[1], MAGENTA);
        assert_ne!(decoded.pixels[2], MAGENTA);
    });
}

#[test]
fn ss_044_should_mask_inside_iframe() {
    run(|t| async move {
        grid(&t).await;
        let frame =
            attach_frame(&t.page, "frame1", &format!("{}/grid.html", t.server.prefix)).await;
        t.page
            .evaluate::<()>(
                "() => { const s = document.createElement('style'); s.textContent = 'iframe { border: none; }'; document.head.appendChild(s); document.querySelector('#frame1').scrollIntoView(); }",
            )
            .await
            .unwrap();
        let png = t
            .page
            .screenshot()
            .mask([t.page.frame_locator("#frame1").locator("div").nth(12)])
            .await
            .unwrap();
        let (fx, fy): (f64, f64) = t
            .page
            .evaluate("() => { const r = document.querySelector('#frame1').getBoundingClientRect(); return [r.x, r.y]; }")
            .await
            .unwrap();
        let (x, y) = div_center(&frame, 12).await;
        let inside = (fx as u32 + x, fy as u32 + y);
        let decoded = decode(&t.context, &png, &[inside]).await;
        assert_eq!(decoded.pixels[0], MAGENTA);
    });
}
