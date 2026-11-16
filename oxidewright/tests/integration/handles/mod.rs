//! Ports of Playwright's element handle, `$`/`$$`/`$eval`/`$$eval` and
//! `waitForSelector` specs (see the submodules).

mod bounding_box;
mod content_frame;
mod convenience;
mod eval_on_selector;
mod misc;
mod owner_frame;
mod query_selector;
mod scroll_into_view;
mod wait_for_element_state;
mod wait_for_selector;

use oxidewright::{Frame, Page};

/// Playwright's `rafraf(page, count)` test util: wait for `count` double
/// animation frames.
pub async fn rafraf(page: &Page, count: usize) {
    rafraf_frame(&page.main_frame(), count).await;
}

/// `rafraf` on a frame.
pub async fn rafraf_frame(frame: &Frame, count: usize) {
    for _ in 0..count {
        frame
            .evaluate::<()>(
                "() => new Promise(f => requestAnimationFrame(() => requestAnimationFrame(f)))",
            )
            .await
            .unwrap();
    }
}

/// `const addElement = tag => document.body.appendChild(document.createElement(tag))`.
pub const ADD_ELEMENT: &str = "tag => { document.body.appendChild(document.createElement(tag)); }";
