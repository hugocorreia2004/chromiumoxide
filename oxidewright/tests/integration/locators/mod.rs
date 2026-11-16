//! Ports of Playwright's `locator-*.spec.ts` and `page-strict.spec.ts`.
//!
//! `expect(locator).toX()` web-first assertions are emulated by the polling
//! helpers below (10 s; Playwright uses 5 s, the margin absorbs slow CI machines).

#![allow(dead_code)]

use std::time::{Duration, Instant};

use oxidewright::{Locator, Page};

mod any_frame;
mod convenience;
mod element_handle;
mod evaluate;
mod frame;
mod highlight;
mod is_visible;
mod list;
mod misc_1;
mod misc_2;
mod query;
mod strict;

const EXPECT_TIMEOUT: Duration = Duration::from_secs(10);

fn normalize(s: &str) -> String {
    s.replace(['\u{200b}', '\u{ad}'], "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Poll `check` until it returns `Ok(true)`; an `Err` stops polling.
async fn poll<F, Fut>(what: &str, mut check: F) -> Result<(), String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<bool, String>>,
{
    let deadline = Instant::now() + EXPECT_TIMEOUT;
    loop {
        match check().await {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(e) => return Err(e),
        }
        if Instant::now() > deadline {
            return Err(format!("expect timed out: {what}"));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn remaining_ms(start: Instant) -> u64 {
    EXPECT_TIMEOUT
        .saturating_sub(start.elapsed())
        .as_millis()
        .max(1) as u64
}

/// `expect(locator).toHaveText(text)` (single element, normalized whitespace).
pub async fn try_text(locator: &Locator, expected: &str) -> Result<(), String> {
    let start = Instant::now();
    let last = std::sync::Arc::new(std::sync::Mutex::new(None));
    let result = poll(&format!("{locator} to have text {expected:?}"), || {
        let locator = locator.clone();
        let timeout = remaining_ms(start);
        let last = last.clone();
        async move {
            let text = locator
                .text_content()
                .timeout(timeout)
                .await
                .map_err(|e| e.to_string())?
                .unwrap_or_default();
            let ok = normalize(&text) == normalize(expected);
            *last.lock().unwrap() = Some(text);
            Ok(ok)
        }
    })
    .await;
    let last = last.lock().unwrap().clone();
    result.map_err(|e| format!("{e} (last: {last:?})"))
}

pub async fn expect_text(locator: &Locator, expected: &str) {
    if let Err(e) = try_text(locator, expected).await {
        panic!("{e}");
    }
}

/// `expect(locator).toHaveText([..])`.
pub async fn try_texts(locator: &Locator, expected: &[&str]) -> Result<(), String> {
    let expected: Vec<String> = expected.iter().map(|s| normalize(s)).collect();
    let last = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let result = poll(&format!("{locator} to have texts {expected:?}"), || {
        let locator = locator.clone();
        let expected = expected.clone();
        let last = last.clone();
        async move {
            let texts = locator
                .all_text_contents()
                .await
                .map_err(|e| e.to_string())?;
            let texts: Vec<String> = texts.iter().map(|s| normalize(s)).collect();
            let ok = texts == expected;
            *last.lock().unwrap() = texts;
            Ok(ok)
        }
    })
    .await;
    let last = last.lock().unwrap().clone();
    result.map_err(|e| format!("{e} (last: {last:?})"))
}

pub async fn expect_texts(locator: &Locator, expected: &[&str]) {
    if let Err(e) = try_texts(locator, expected).await {
        panic!("{e}");
    }
}

/// `expect(locator).toHaveCount(n)`.
pub async fn try_count(locator: &Locator, expected: usize) -> Result<(), String> {
    poll(&format!("{locator} to have count {expected}"), || {
        let locator = locator.clone();
        async move {
            let count = locator.count().await.map_err(|e| e.to_string())?;
            Ok(count == expected)
        }
    })
    .await
}

pub async fn expect_count(locator: &Locator, expected: usize) {
    if let Err(e) = try_count(locator, expected).await {
        panic!("{e}");
    }
}

/// `expect(locator).toBeVisible()` / `toBeHidden()`.
pub async fn try_visible(locator: &Locator, visible: bool) -> Result<(), String> {
    poll(&format!("{locator} visible={visible}"), || {
        let locator = locator.clone();
        async move {
            let v = locator.is_visible().await.map_err(|e| e.to_string())?;
            Ok(v == visible)
        }
    })
    .await
}

pub async fn expect_visible(locator: &Locator) {
    if let Err(e) = try_visible(locator, true).await {
        panic!("{e}");
    }
}

pub async fn expect_hidden(locator: &Locator) {
    if let Err(e) = try_visible(locator, false).await {
        panic!("{e}");
    }
}

/// `expect(locator).toBeAttached()`.
pub async fn expect_attached(locator: &Locator) {
    expect_count_at_least(locator, 1).await;
}

async fn expect_count_at_least(locator: &Locator, n: usize) {
    let r = poll(&format!("{locator} attached"), || {
        let locator = locator.clone();
        async move { Ok(locator.count().await.map_err(|e| e.to_string())? >= n) }
    })
    .await;
    if let Err(e) = r {
        panic!("{e}");
    }
}

/// `expect(locator).toHaveValue(v)`.
pub async fn expect_value(locator: &Locator, expected: &str) {
    let start = Instant::now();
    let r = poll(&format!("{locator} to have value {expected:?}"), || {
        let locator = locator.clone();
        let timeout = remaining_ms(start);
        async move {
            let v = locator
                .input_value()
                .timeout(timeout)
                .await
                .map_err(|e| e.to_string())?;
            Ok(v == expected)
        }
    })
    .await;
    if let Err(e) = r {
        panic!("{e}");
    }
}

/// `expect(locator).toHaveAttribute(name, value)`.
pub async fn expect_attribute(locator: &Locator, name: &str, expected: &str) {
    let start = Instant::now();
    let r = poll(
        &format!("{locator} to have attribute {name}={expected:?}"),
        || {
            let locator = locator.clone();
            let timeout = remaining_ms(start);
            async move {
                let v = locator
                    .get_attribute(name)
                    .timeout(timeout)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(v.as_deref() == Some(expected))
            }
        },
    )
    .await;
    if let Err(e) = r {
        panic!("{e}");
    }
}

/// `expect(locator).toHaveClass(class)` (exact `class` attribute).
pub async fn expect_class(locator: &Locator, expected: &str) {
    expect_attribute(locator, "class", expected).await;
}

/// `expect.poll(() => page.frames().length).toBe(n)`.
pub async fn expect_frame_count(page: &Page, n: usize) {
    let r = poll(&format!("{n} frames"), || {
        let page = page.clone();
        async move { Ok(page.frames().len() == n) }
    })
    .await;
    if let Err(e) = r {
        panic!("{e}");
    }
}
