//! Launch Chromium, render a page and save screenshots.
//!
//! ```sh
//! cargo run -p oxidewright --example screenshot
//! ```
//!
//! Playwright equivalent:
//!
//! ```js
//! const browser = await chromium.launch();
//! const page = await browser.newPage();
//! await page.setContent(html);
//! await page.screenshot({ path: 'page.png', fullPage: true });
//! await page.locator('h1').screenshot({ path: 'title.png' });
//! ```

use oxidewright::{ScreenshotType, ViewportSize, chromium};

const HTML: &str = r#"<!doctype html>
<style>body { font-family: sans-serif; } section { height: 1500px; }</style>
<h1>Hello from oxidewright</h1>
<section>A tall section, so the full-page screenshot is taller than the viewport.</section>"#;

#[tokio::main]
async fn main() -> oxidewright::Result<()> {
    let browser = chromium().launch().headless(true).await?;
    let context = browser
        .new_context()
        .viewport(Some(ViewportSize {
            width: 800,
            height: 600,
        }))
        .await?;
    let page = context.new_page().await?;
    page.set_content(HTML).await?;

    let out = std::env::temp_dir();
    page.screenshot()
        .path(out.join("oxidewright-page.png"))
        .full_page(true)
        .await?;
    let jpeg = page
        .screenshot()
        .r#type(ScreenshotType::Jpeg)
        .quality(80)
        .await?;
    page.locator("h1")
        .screenshot()
        .path(out.join("oxidewright-title.png"))
        .await?;

    println!(
        "saved screenshots to {} (viewport JPEG: {} bytes)",
        out.display(),
        jpeg.len()
    );
    browser.close().await?;
    Ok(())
}
