//! Port of `locator-highlight.spec.ts`.
//!
//! LO-114 is adapted: the highlight overlay lives in a closed shadow root
//! of the injected script, so instead of locating `x-pw-highlight` the
//! test checks that highlight/hide_highlight succeed and leave the page
//! usable. The tooltip text and box comparison are not checked.

use crate::common::*;

#[test]
fn should_highlight_locator() {
    run(|t| async move {
        t.page.set_content("<input type='text' />").await.unwrap();
        let input = t.page.locator("input");
        input.highlight().await.unwrap();
        input.hide_highlight().await.unwrap();
        assert!(input.bounding_box().await.unwrap().is_some());
    });
}
