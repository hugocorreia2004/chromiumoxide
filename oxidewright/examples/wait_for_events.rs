//! Waiting for responses, popups, dialogs and other events.
//!
//! ```sh
//! cargo run -p oxidewright --example wait_for_events
//! ```
//!
//! Waiters (`wait_for_*`) subscribe when they are *called*, not when they
//! are awaited, so create them before triggering the event. `expect_*`
//! helpers run an action and wait for the event at the same time, like
//! `Promise.all([page.waitForEvent(..), action])` in Playwright.

use oxidewright::{EventKind, Route, chromium};

const PAGE: &str = r#"<!doctype html>
<button id="load" onclick="fetch('/api/items?page=2')">Load more</button>
<button id="open" onclick="window.open('about:blank')">Open popup</button>
<button id="ask" onclick="document.body.dataset.answer = confirm('Sure?')">Ask</button>"#;

#[tokio::main]
async fn main() -> oxidewright::Result<()> {
    let browser = chromium().launch().await?;
    let context = browser.new_context().await?;
    context
        .route("http://app.test/**", |route: Route| async move {
            let url = route.request().url();
            if url.contains("/api/") {
                route.fulfill().json(&vec!["c", "d"]).await
            } else {
                route.fulfill().content_type("text/html").body(PAGE).await
            }
        })
        .await?;
    let page = context.new_page().await?;
    page.goto("http://app.test/").await?;

    // const responsePromise = page.waitForResponse(r => r.url().includes('page=2'));
    // await page.click('#load');
    // const response = await responsePromise;
    let response = page
        .wait_for_response("**/api/items*")
        .predicate(|r| r.url().contains("page=2"))
        .timeout(10_000);
    page.click("#load").await?;
    let response = response.await?;
    let items: Vec<String> = response.json().await?;
    println!("items: {items:?}");

    // const [popup] = await Promise.all([page.waitForEvent('popup'), page.click('#open')]);
    // (The very first request of a popup is not routed yet, so this one
    // opens `about:blank` and is filled afterwards.)
    let popup = page.expect_popup(page.click("#open")).await?;
    popup.set_content("<title>Popup</title>").await?;
    println!("popup opened: {:?}", popup.title().await?);
    popup.close().await?;

    // page.once('dialog', d => d.accept());
    // Without a listener dialogs are dismissed automatically.
    page.on_dialog(|dialog| {
        println!("dialog: {} {:?}", dialog.r#type(), dialog.message());
        tokio::spawn(async move { dialog.accept(None).await });
    });
    page.click("#ask").await?;
    let answer = page.locator("body").get_attribute("data-answer").await?;
    println!("confirm() returned {answer:?}");

    // Generic events: page.waitForEvent('framenavigated')
    let navigated = page.wait_for_event(EventKind::FrameNavigated);
    page.reload().await?;
    println!("event: {:?}", navigated.await?.kind());

    // A URL that never loads times out with Playwright's message.
    let err = page
        .wait_for_url("**/never")
        .timeout(500)
        .await
        .unwrap_err();
    println!("expected timeout: {err} (is_timeout: {})", err.is_timeout());

    browser.close().await?;
    Ok(())
}
