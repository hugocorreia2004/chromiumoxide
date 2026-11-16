//! Ports of Playwright's form specs: `page-fill`, `page-check`,
//! `page-select-option`, `page-set-input-files`, `page-focus`,
//! `page-dispatchevent`, `elementhandle-select-text`, `elementhandle-type`
//! and `elementhandle-press`.

mod elementhandle_press;
mod elementhandle_select_text;
mod elementhandle_type;
mod page_check;
mod page_dispatchevent;
mod page_fill;
mod page_focus;
mod page_select_option;
mod page_set_input_files;

use std::future::IntoFuture;
use std::time::Duration;

/// Run an action in the background (like an un-awaited promise in the specs).
pub fn spawn<F>(action: F) -> tokio::task::JoinHandle<F::Output>
where
    F: IntoFuture,
    F::IntoFuture: Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(action.into_future())
}

/// `rafraf(page, 5)` / "give it a chance": let the page run a few frames.
pub async fn give_it_a_chance() {
    tokio::time::sleep(Duration::from_millis(200)).await;
}

/// Absolute path of a test asset.
pub fn asset(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("assets")
        .join(name)
}
