//! `toHaveScreenshot` against a PNG file. Playwright's own tests live in
//! the test runner suite (`tests/playwright-test/to-have-screenshot.spec.ts`);
//! these cover the same behaviours with explicit file paths, and
//! `EX-180 toHaveScreenshot should populate matcherResult`.

use std::path::PathBuf;

use oxidewright::ViewportSize;
use oxidewright::expect::expect;

use super::*;
use crate::common::*;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "oxidewright-expect-{name}-{}-{}",
        std::process::id(),
        fastrand_suffix()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fastrand_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

#[test]
fn should_write_missing_snapshot_and_match_it_afterwards() {
    run(|t| async move {
        let dir = temp_dir("missing");
        let path = dir.join("page.png");
        t.page
            .set_content("<div style='width:50px;height:50px;background:red'></div>")
            .await
            .unwrap();
        let m = message(expect(&t.page).to_have_screenshot(&path).await);
        assert_contains(
            &m,
            &format!(
                "A snapshot doesn't exist at {}, writing actual.",
                path.display()
            ),
        );
        assert!(path.exists());
        expect(&t.page).to_have_screenshot(&path).await.unwrap();
        expect(&t.page.locator("div"))
            .to_have_screenshot(dir.join("div.png"))
            .await
            .unwrap_err();
        expect(&t.page.locator("div"))
            .to_have_screenshot(dir.join("div.png"))
            .await
            .unwrap();
        let _ = std::fs::remove_dir_all(dir);
    });
}

#[test]
fn should_populate_matcher_result_when_different() {
    run(|t| async move {
        let dir = temp_dir("different");
        let path = dir.join("screenshot-sanity.png");
        t.page
            .set_viewport_size(ViewportSize {
                width: 500,
                height: 500,
            })
            .await
            .unwrap();
        t.page
            .set_content("<div style='width:100px;height:100px;background:red'></div>")
            .await
            .unwrap();
        let _ = expect(&t.page).to_have_screenshot(&path).await;
        t.page
            .set_content("<div style='width:100px;height:100px;background:blue'></div>")
            .await
            .unwrap();
        let e = matcher_result(
            expect(&t.page)
                .to_have_screenshot(&path)
                .timeout(2000)
                .await,
        );
        assert_eq!(e.name, "toHaveScreenshot");
        assert!(!e.pass);
        assert!(!e.log.is_empty());
        assert_contains(
            &e.message,
            "expect(page).toHaveScreenshot(expected) failed\n\n  10000 pixels (ratio 0.04 of all image pixels) are different.",
        );
        assert!(dir.join("screenshot-sanity-actual.png").exists());
        assert!(dir.join("screenshot-sanity-diff.png").exists());
        // Thresholds.
        expect(&t.page)
            .to_have_screenshot(&path)
            .max_diff_pixels(10_000)
            .await
            .unwrap();
        expect(&t.page)
            .to_have_screenshot(&path)
            .max_diff_pixel_ratio(0.05)
            .await
            .unwrap();
        expect(&t.page)
            .not()
            .to_have_screenshot(&path)
            .await
            .unwrap();
        let _ = std::fs::remove_dir_all(dir);
    });
}

#[test]
fn should_fail_with_size_mismatch() {
    run(|t| async move {
        let dir = temp_dir("size");
        let path = dir.join("page.png");
        t.page
            .set_viewport_size(ViewportSize {
                width: 300,
                height: 200,
            })
            .await
            .unwrap();
        t.page
            .set_content("<body style='background:white'></body>")
            .await
            .unwrap();
        let _ = expect(&t.page).to_have_screenshot(&path).await;
        t.page
            .set_viewport_size(ViewportSize {
                width: 200,
                height: 200,
            })
            .await
            .unwrap();
        let m = message(
            expect(&t.page)
                .to_have_screenshot(&path)
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "Expected an image 300px by 200px, received 200px by 200px.",
        );
        let m = message(
            expect(&t.page)
                .to_have_screenshot(dir.join("page.jpg"))
                .await,
        );
        assert_contains(&m, "must have a '.png' extension");
        let _ = std::fs::remove_dir_all(dir);
    });
}
