//! Ports of Playwright's `tests/page/expect-*.spec.ts` and
//! `to-match-aria-snapshot.spec.ts` for `oxidewright::expect`.
//! Messages are compared without colors (Playwright's tests strip them).

#![allow(dead_code)]

use std::time::Duration;

use oxidewright::expect::MatcherResult;
use oxidewright::{Error, Page};

mod accessible;
mod aria_snapshot;
mod boolean;
mod matcher_result;
mod misc;
mod screenshot;
mod timeout;
mod to_have_text;
mod to_have_value;

/// The message of a failed assertion.
fn message(result: oxidewright::Result<()>) -> String {
    match result {
        Ok(()) => panic!("the assertion passed"),
        Err(e) => e.to_string(),
    }
}

/// The matcher result of a failed assertion.
fn matcher_result(result: oxidewright::Result<()>) -> MatcherResult {
    match result {
        Ok(()) => panic!("the assertion passed"),
        Err(Error::Expect(r)) => *r,
        Err(e) => panic!("not an expect error: {e}"),
    }
}

#[track_caller]
fn assert_contains(haystack: &str, needle: &str) {
    assert!(
        haystack.contains(needle),
        "expected to contain:\n{needle}\n--- in:\n{haystack}"
    );
}

#[track_caller]
fn assert_not_contains(haystack: &str, needle: &str) {
    assert!(
        !haystack.contains(needle),
        "expected not to contain:\n{needle}\n--- in:\n{haystack}"
    );
}

/// `setTimeout(() => page.evaluate(script), ms)`.
fn later(page: &Page, ms: u64, script: &'static str) {
    let page = page.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        let _ = page.evaluate::<()>(script).await;
    });
}
