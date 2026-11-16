//! Record a trace that opens in Playwright's trace viewer.
//!
//! ```sh
//! cargo run -p oxidewright --example tracing
//! npx playwright show-trace "$TMPDIR/oxidewright-trace.zip"
//! ```
//!
//! Traces currently contain the screencast frames and the `group` /
//! `group_end` entries; per-action events, DOM snapshots and network entries
//! are not recorded yet (see the `oxidewright::trace` module docs).
//! Playwright equivalent:
//!
//! ```js
//! await context.tracing.start({ screenshots: true });
//! await context.tracing.group('fill the form');
//! // ...
//! await context.tracing.groupEnd();
//! await context.tracing.stop({ path: 'trace.zip' });
//! ```

use oxidewright::chromium;

#[tokio::main]
async fn main() -> oxidewright::Result<()> {
    let path = std::env::temp_dir().join("oxidewright-trace.zip");
    let browser = chromium().launch().await?;
    let context = browser.new_context().await?;
    let tracing = context.tracing();
    tracing
        .start()
        .name("example")
        .title("oxidewright tracing example")
        .screenshots(true)
        .await?;

    let page = context.new_page().await?;
    page.set_content(
        r#"<input id="q" placeholder="Search"><button onclick="this.textContent = 'Done'">Go</button>"#,
    )
    .await?;

    tracing.group("fill the form").await?;
    page.get_by_placeholder("Search")
        .fill("oxidewright")
        .await?;
    page.get_by_role("button").click().await?;
    tracing.group_end().await?;

    page.get_by_text("Done").wait_for().await?;
    tracing.stop().path(&path).await?;
    println!("trace saved to {}", path.display());

    browser.close().await?;
    Ok(())
}
