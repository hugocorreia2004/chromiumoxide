//! Network routing: mock an API, block resources, modify requests and
//! observe traffic.
//!
//! Everything is served by routes on a made-up origin, so the example needs
//! no network:
//!
//! ```sh
//! cargo run -p oxidewright --example network_mocking
//! ```

use oxidewright::{Route, chromium};
use serde_json::json;

const PAGE: &str = r#"<!doctype html>
<img src="/logo.png">
<pre id="out"></pre>
<script>
  fetch('/api/user', { headers: { 'x-from': 'page' } })
    .then(r => r.json())
    .then(user => document.querySelector('#out').textContent = user.name);
</script>"#;

#[tokio::main]
async fn main() -> oxidewright::Result<()> {
    let browser = chromium().launch().await?;
    let page = browser.new_page().await?;

    // page.on('request', r => console.log(r.method(), r.url()))
    page.on_request(|request| println!("-> {} {}", request.method(), request.url()));
    // page.on('requestfailed', r => console.log('failed', r.url(), r.failure()))
    page.on_request_failed(|request| println!("x  {} ({:?})", request.url(), request.failure()));

    // Routes added later run first; `fallback()` hands the request to the
    // previous one. This catch-all serves the HTML page.
    page.route("http://app.test/**", |route: Route| async move {
        route.fulfill().content_type("text/html").body(PAGE).await
    })
    .await?;

    // await page.route('**/*.png', route => route.abort());
    page.route(
        "**/*.png",
        |route: Route| async move { route.abort().await },
    )
    .await?;

    // await page.route('**/api/user', route => route.fulfill({ json: {...} }));
    page.route("**/api/user", |route: Route| async move {
        let from = route.request().header_value("x-from").await?;
        println!("   API called (x-from: {from:?})");
        route
            .fulfill()
            .json(&json!({ "id": 1, "name": "Mocked user" }))
            .await
    })
    .await?;

    // Modify outgoing requests and fall through to the other routes:
    // await page.route('**/*', route => route.fallback({ headers: {...} }));
    page.route("**/*", |route: Route| async move {
        let mut headers = route.request().all_headers().await?;
        headers.insert("x-example".into(), "1".into());
        route.fallback().headers(headers).await
    })
    .await?;

    let response = page
        .expect_response("**/api/user", page.goto("http://app.test/"))
        .await?;
    println!(
        "API status {}: {}",
        response.status(),
        response.text().await?
    );

    page.locator("#out").wait_for().await?;
    println!("page shows: {}", page.locator("#out").inner_text().await?);

    // Remove every route (page.unrouteAll()).
    page.unroute_all().await?;
    browser.close().await?;
    Ok(())
}
