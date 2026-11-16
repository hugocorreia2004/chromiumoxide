//! Save cookies and `localStorage` of a context and restore them in another
//! one (for example to reuse a signed-in session).
//!
//! ```sh
//! cargo run -p oxidewright --example storage_state
//! ```
//!
//! The file uses Playwright's `storageState` JSON format, so it can be shared
//! with Playwright scripts. Playwright equivalent:
//!
//! ```js
//! await context.storageState({ path: 'state.json' });
//! const restored = await browser.newContext({ storageState: 'state.json' });
//! ```

use oxidewright::{Browser, BrowserContext, Route, SetCookie, chromium};

/// A context whose pages on `http://app.test` are served by a route.
async fn app_context(
    browser: &Browser,
    state: Option<&str>,
) -> oxidewright::Result<BrowserContext> {
    let mut builder = browser.new_context();
    if let Some(path) = state {
        builder = builder.storage_state(path);
    }
    let context = builder.await?;
    context
        .route("http://app.test/**", |route: Route| async move {
            route
                .fulfill()
                .content_type("text/html")
                .body("<!doctype html><title>app</title>")
                .await
        })
        .await?;
    Ok(context)
}

#[tokio::main]
async fn main() -> oxidewright::Result<()> {
    let path = std::env::temp_dir().join("oxidewright-state.json");
    let path_str = path.to_string_lossy().into_owned();
    let browser = chromium().launch().await?;

    // "Sign in": set a cookie and some localStorage, then save the state.
    let context = app_context(&browser, None).await?;
    context
        .add_cookies([SetCookie::new("session", "abc123").url("http://app.test/")])
        .await?;
    let page = context.new_page().await?;
    page.goto("http://app.test/").await?;
    page.evaluate::<()>("() => localStorage.setItem('theme', 'dark')")
        .await?;
    let state = context.storage_state().path(&path).await?;
    println!(
        "saved {} cookie(s) and {} origin(s) to {}",
        state.cookies.len(),
        state.origins.len(),
        path.display()
    );
    context.close().await?;

    // Restore it in a fresh context.
    let restored = app_context(&browser, Some(&path_str)).await?;
    let page = restored.new_page().await?;
    page.goto("http://app.test/").await?;
    let cookie: String = page.evaluate("() => document.cookie").await?;
    let theme: Option<String> = page.evaluate("() => localStorage.getItem('theme')").await?;
    println!("restored cookie {cookie:?}, theme {theme:?}");

    // `set_storage_state` replaces the state of an existing context.
    restored.set_storage_state(path_str.as_str()).await?;

    browser.close().await?;
    Ok(())
}
