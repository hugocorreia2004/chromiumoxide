//! Port of `expect-timeout.spec.ts`.
//!
//! Skipped:
//! * `EX-248 should have timeout error name`: about `page.waitForSelector`.
//! * `EX-250`/`EX-251`: locator handlers (`page.addLocatorHandler`) are not
//!   implemented.
//! * `EX-253`..`EX-258`: the `signal` (`AbortSignal`) option; dropping the
//!   future cancels an assertion in Rust.

use oxidewright::expect::expect;

use super::*;
use crate::common::*;

#[test]
fn should_print_element_not_found() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("no-such-thing"))
                .to_have_text("hey")
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).toHaveText(expected) failed\n\nLocator: locator('no-such-thing')\nExpected: \"hey\"\nTimeout: 1000ms\nError: element(s) not found\n\nCall log:\n",
        );
    });
}

#[test]
fn should_print_timed_out_error_message_when_value_does_not_match() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("div"))
                .to_have_text("hey")
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).toHaveText(expected) failed\n\nLocator:  locator('div')\nExpected: \"hey\"\nReceived: \"Text content\"\nTimeout:  1000ms\n\nCall log:\n",
        );
    });
}

#[test]
fn should_print_timed_out_error_message_with_impossible_timeout() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("no-such-thing"))
                .to_have_text("hey")
                .timeout(1)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).toHaveText(expected) failed\n\nLocator: locator('no-such-thing')\nExpected: \"hey\"\nTimeout: 1ms\nError: element(s) not found\n\nCall log:",
        );
    });
}

#[test]
fn should_print_timed_out_error_message_when_value_does_not_match_with_impossible_timeout() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("div"))
                .to_have_text("hey")
                .timeout(1)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).toHaveText(expected) failed\n\nLocator:  locator('div')\nExpected: \"hey\"\nReceived: \"Text content\"\nTimeout:  1ms\n\nCall log:\n",
        );
    });
}

#[test]
fn should_not_throw_when_navigating_during_one_shot_check() {
    run(|t| async move {
        t.page.set_content("<div>hello</div>").await.unwrap();
        let locator = t.page.locator("div");
        let task = tokio::spawn(async move { expect(&locator).to_have_text("bye").await });
        // Let the assertion start before the navigation.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page.set_content("<div>bye</div>").await.unwrap();
        task.await.unwrap().unwrap();
    });
}

#[test]
fn should_not_miss_element_that_appears_between_retries_before_the_deadline() {
    run(|t| async move {
        t.page
            .set_content("<div id=\"target\" style=\"display:none\">content</div>")
            .await
            .unwrap();
        t.page
            .evaluate::<()>("() => { setTimeout(() => { document.getElementById('target').style.display = 'block'; }, 1500); }")
            .await
            .unwrap();
        // Shown after 1.5 s, between two retries (the interval grows to 1 s);
        // a 3 s deadline leaves slow CI runners room.
        expect(&t.page.locator("#target"))
            .to_be_visible()
            .timeout(3000)
            .await
            .unwrap();
    });
}
