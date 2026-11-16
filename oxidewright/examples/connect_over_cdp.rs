//! Attach to a Chrome that is already running with remote debugging.
//!
//! Start Chrome yourself, for example:
//!
//! ```sh
//! chrome --remote-debugging-port=9222 --user-data-dir=/tmp/profile
//! cargo run -p oxidewright --example connect_over_cdp -- http://127.0.0.1:9222
//! ```
//!
//! The endpoint is either `http://host:port` (its `/json/version` is queried)
//! or the browser's `ws://` URL. Playwright equivalent:
//!
//! ```js
//! const browser = await chromium.connectOverCDP('http://127.0.0.1:9222');
//! const context = browser.contexts()[0];
//! const page = await context.newPage();
//! ```
//!
//! The default context of a connected browser overrides nothing (no
//! viewport, user agent or media emulation), so pages keep the browser's
//! real values.

use oxidewright::chromium;

#[tokio::main]
async fn main() -> oxidewright::Result<()> {
    let endpoint = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "http://127.0.0.1:9222".to_string());
    let browser = chromium()
        .connect_over_cdp(endpoint)
        .timeout(10_000)
        .await?;
    println!("connected to Chrome {}", browser.version());

    // The browser's default context, with the tabs that are already open.
    let context = browser
        .contexts()
        .into_iter()
        .next()
        .expect("a connected browser has a default context");
    for page in context.pages() {
        println!("open tab: {}", page.url());
    }

    // Open a tab, use it, and close it again.
    let page = context.new_page().await?;
    page.set_content("<title>oxidewright</title><p>attached</p>")
        .await?;
    println!("new tab title: {}", page.title().await?);
    page.close().await?;

    // For a connected browser, `close` only disconnects: Chrome keeps running.
    browser.close().await?;
    Ok(())
}
