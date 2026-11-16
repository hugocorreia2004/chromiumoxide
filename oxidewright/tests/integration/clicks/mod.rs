//! Ports of Playwright's click related specs (see the submodules).

mod elementhandle_click;
mod locator_click;
mod page_autowaiting_basic;
mod page_click;
mod page_click_scroll;
mod page_click_timeout;

use oxidewright::Page;
use serde_json::Value;

/// Playwright's `rafraf(page, count)` test util.
pub async fn rafraf(page: &Page, count: usize) {
    for _ in 0..count {
        page.evaluate::<()>(
            "() => new Promise(f => requestAnimationFrame(() => requestAnimationFrame(f)))",
        )
        .await
        .unwrap();
    }
}

/// `giveItAChanceToClick`.
pub async fn give_it_a_chance_to_click(page: &Page) {
    rafraf(page, 5).await;
}

/// `page.$eval(selector, expression)` ignoring the result.
pub async fn eval_sel(page: &Page, selector: &str, expression: &str) {
    page.eval_on_selector::<Value>(selector, expression)
        .await
        .unwrap();
}

/// `page.evaluate(expression)` as JSON.
pub async fn eval(page: &Page, expression: &str) -> Value {
    page.evaluate::<Value>(expression).await.unwrap()
}
