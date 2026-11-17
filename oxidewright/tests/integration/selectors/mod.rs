//! Ports of Playwright's selector specs (`selectors-*.spec.ts`,
//! `retarget.spec.ts`, `page-drag.spec.ts`) and small polling helpers that
//! stand in for Playwright's web-first `expect` matchers.

mod css;
mod drag;
mod frame;
mod get_by;
mod misc;
mod register;
mod retarget;
mod role;
mod text;

use std::future::Future;
use std::time::{Duration, Instant};

use oxidewright::{Locator, Page};
use serde::de::DeserializeOwned;

/// Poll `f` until it returns `expected` (Playwright's `expect.poll`), 5 s max.
pub async fn poll_eq<T, F, Fut>(expected: T, mut f: F)
where
    T: PartialEq + std::fmt::Debug,
    F: FnMut() -> Fut,
    Fut: Future<Output = T>,
{
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let value = f().await;
        if value == expected {
            return;
        }
        if Instant::now() > deadline {
            assert_eq!(value, expected);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// `expect(locator).toHaveCount(n)`.
pub async fn expect_count(locator: &Locator, n: usize) {
    poll_eq(n, move || async move { locator.count().await.unwrap() }).await;
}

/// `outerHTML` of every element matched by `locator`.
pub async fn outer_htmls(locator: &Locator) -> Vec<String> {
    locator
        .evaluate_all::<Vec<String>>("els => els.map(e => e.outerHTML)")
        .await
        .unwrap()
}

/// `textContent` of every element matched by `locator`.
pub async fn texts(locator: &Locator) -> Vec<String> {
    locator
        .evaluate_all::<Vec<String>>("els => els.map(e => e.textContent)")
        .await
        .unwrap()
}

/// `expect(locator).toHaveText(text)` (whitespace normalized, full match).
pub async fn expect_text(locator: &Locator, text: &str) {
    let expected = normalize(text);
    poll_eq(Some(expected), move || async move {
        locator
            .evaluate::<String>("e => e.textContent")
            .timeout(1000)
            .await
            .ok()
            .map(|t| normalize(&t))
    })
    .await;
}

/// `expect(locator).toHaveText([..])`.
pub async fn expect_texts(locator: &Locator, expected: &[&str]) {
    let expected: Vec<String> = expected.iter().map(|t| normalize(t)).collect();
    poll_eq(expected, move || async move {
        texts(locator).await.iter().map(|t| normalize(t)).collect()
    })
    .await;
}

/// `expect(locator).toHaveAttribute(name, value)`.
pub async fn expect_attr(locator: &Locator, name: &str, value: &str) {
    poll_eq(Some(value.to_string()), move || async move {
        locator
            .get_attribute(name)
            .timeout(1000)
            .await
            .ok()
            .flatten()
    })
    .await;
}

/// `expect(locator).toBeVisible()` / `.not.toBeVisible()`.
pub async fn expect_visible(locator: &Locator, visible: bool) {
    poll_eq(visible, move || async move {
        locator.is_visible().await.unwrap()
    })
    .await;
}

/// `expect(locator).toBeEnabled()` / `toBeDisabled()`.
pub async fn expect_enabled(locator: &Locator, enabled: bool) {
    poll_eq(Some(enabled), move || async move {
        locator.is_enabled().timeout(1000).await.ok()
    })
    .await;
}

fn normalize(text: &str) -> String {
    text.replace(['\u{200b}', '\u{ad}'], "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `page.$eval(selector, expression)`.
pub async fn eval<T: DeserializeOwned + Send + 'static>(
    page: &Page,
    selector: &str,
    expression: &str,
) -> T {
    match page.eval_on_selector::<T>(selector, expression).await {
        Ok(v) => v,
        Err(e) => panic!("$eval({selector}) failed: {e}"),
    }
}

/// `page.$$eval(selector, expression)`.
pub async fn eval_all<T: DeserializeOwned + Send + 'static>(
    page: &Page,
    selector: &str,
    expression: &str,
) -> T {
    match page.eval_on_selector_all::<T>(selector, expression).await {
        Ok(v) => v,
        Err(e) => panic!("$$eval({selector}) failed: {e}"),
    }
}

/// Whether `page.$(selector)` finds an element.
pub async fn exists(page: &Page, selector: &str) -> bool {
    page.query_selector(selector).await.unwrap().is_some()
}

/// The open shadow roots test page of Playwright (`deep-shadow.html`).
pub fn deep_shadow(prefix: &str) -> String {
    format!("{prefix}/deep-shadow-full.html")
}
