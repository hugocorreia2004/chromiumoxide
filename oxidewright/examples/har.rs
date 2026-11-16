//! Record the traffic of a context into a HAR file, then replay it.
//!
//! ```sh
//! cargo run -p oxidewright --example har
//! ```
//!
//! Playwright equivalent:
//!
//! ```js
//! const context = await browser.newContext({ recordHar: { path: 'app.har' } });
//! // ... browse ...
//! await context.close(); // the HAR is written on close
//! const replay = await browser.newContext();
//! await replay.routeFromHAR('app.har', { url: '**/api/**' });
//! ```

use oxidewright::{HarContentPolicy, HarNotFound, RecordHar, Route, chromium};
use serde_json::json;

const PAGE: &str = r#"<!doctype html>
<pre id="out"></pre>
<script>
  fetch('/api/data').then(r => r.json())
    .then(d => document.querySelector('#out').textContent = d.message);
</script>"#;

#[tokio::main]
async fn main() -> oxidewright::Result<()> {
    let har = std::env::temp_dir().join("oxidewright-example.har");
    let browser = chromium().launch().await?;

    // 1. Record. Responses are mocked here so the example runs offline; on a
    //    real site you would just browse.
    let context = browser
        .new_context()
        .record_har(
            RecordHar::new(&har)
                .content(HarContentPolicy::Embed)
                .url_filter("**/api/**"),
        )
        .await?;
    context
        .route("http://app.test/**", |route: Route| async move {
            if route.request().url().ends_with("/api/data") {
                route
                    .fulfill()
                    .json(&json!({ "message": "recorded response" }))
                    .await
            } else {
                route.fulfill().content_type("text/html").body(PAGE).await
            }
        })
        .await?;
    let page = context.new_page().await?;
    page.expect_response("**/api/data", page.goto("http://app.test/"))
        .await?;
    context.close().await?; // writes the HAR
    println!("recorded {}", har.display());

    // 2. Replay: API calls are answered from the HAR, the page itself by a
    //    route that falls through when the HAR has no entry.
    let replay = browser.new_context().await?;
    replay
        .route("http://app.test/**", |route: Route| async move {
            route.fulfill().content_type("text/html").body(PAGE).await
        })
        .await?;
    replay
        .route_from_har(&har)
        .url("**/api/**")
        .not_found(HarNotFound::Fallback)
        .await?;
    let page = replay.new_page().await?;
    page.goto("http://app.test/").await?;
    let out = page.locator("#out");
    out.wait_for().await?;
    // Wait until the script has filled the element.
    page.locator("#out:not(:empty)").wait_for().await?;
    println!("replayed: {}", out.inner_text().await?);

    browser.close().await?;
    Ok(())
}
