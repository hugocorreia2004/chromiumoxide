//! The `stealth` defaults (like Patchright): automation code never runs in
//! the page's main world unless asked, and Chrome does not announce
//! automation. A separate test binary, so the integration suite's switch to
//! Playwright's main-world default does not apply here.
#![cfg(feature = "stealth")]

use oxidewright::{Browser, chromium};

async fn launch() -> Browser {
    let mut launch = chromium().launch();
    if let Ok(path) = std::env::var("OXIDEWRIGHT_CHROME") {
        launch = launch.executable_path(path);
    }
    launch.await.expect("failed to launch the browser")
}

/// The page counts every call to a few DOM methods made in its own world,
/// like bot-detector.rebrowser.net's `mainWorldExecution` check.
const TRAP: &str = r#"<div id="d" class="c">hello</div>
<script>
  window.secret = 42;
  window.calls = 0;
  for (const name of ['querySelector', 'querySelectorAll', 'getElementById', 'getElementsByClassName']) {
    const original = Document.prototype[name];
    Document.prototype[name] = function (...args) { window.calls++; return original.apply(this, args); };
    const elementOriginal = Element.prototype[name];
    if (elementOriginal)
      Element.prototype[name] = function (...args) { window.calls++; return elementOriginal.apply(this, args); };
  }
</script>"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automation_code_stays_out_of_the_main_world() {
    let browser = launch().await;
    let page = browser.new_page().await.unwrap();
    page.set_content(TRAP).await.unwrap();

    // `evaluate` runs in the isolated world: page globals are not visible...
    let secret: Option<i64> = page.evaluate("() => window.secret").await.unwrap();
    assert_eq!(secret, None);
    // ...unless asked for the main world.
    let secret: i64 = page
        .evaluate("() => window.secret")
        .isolated_context(false)
        .await
        .unwrap();
    assert_eq!(secret, 42);

    // DOM calls from evaluate, locators and element handles: none of them
    // may reach the page's hooks.
    let _: usize = page
        .evaluate("() => document.getElementsByClassName('c').length")
        .await
        .unwrap();
    let text: String = page
        .locator("#d")
        .evaluate("e => e.querySelector('*') === null ? e.textContent : ''")
        .await
        .unwrap();
    assert_eq!(text, "hello");
    let _: Vec<String> = page
        .locator("div")
        .evaluate_all("els => els.map(e => e.id)")
        .await
        .unwrap();
    let handle = page.query_selector("#d").await.unwrap().unwrap();
    let _: String = handle
        .evaluate("e => document.getElementById('d').textContent")
        .await
        .unwrap();
    page.locator("#d").click().await.unwrap();
    assert_eq!(
        page.locator("#d").text_content().await.unwrap().as_deref(),
        Some("hello")
    );

    let calls: i64 = page
        .evaluate("() => window.calls")
        .isolated_context(false)
        .await
        .unwrap();
    assert_eq!(calls, 0, "automation code ran in the page's main world");
    browser.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chrome_does_not_announce_automation() {
    let browser = launch().await;
    let page = browser.new_page().await.unwrap();
    page.set_content("<p>x</p>").await.unwrap();
    let webdriver: bool = page
        .evaluate("() => navigator.webdriver")
        .isolated_context(false)
        .await
        .unwrap();
    assert!(!webdriver, "navigator.webdriver is true");
    browser.close().await.unwrap();
}
