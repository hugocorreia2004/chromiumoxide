//! Locators and form filling.
//!
//! The page is served by a route on a made-up origin, so the example needs
//! no network:
//!
//! ```sh
//! cargo run -p oxidewright --example locators_forms
//! ```

use oxidewright::{GetByRoleOptions, LocatorOptions, Route, chromium, regex};

const FORM: &str = r#"<!doctype html>
<title>Sign up</title>
<form onsubmit="event.preventDefault(); document.querySelector('#result').textContent =
    JSON.stringify(Object.fromEntries(new FormData(this)))">
  <label>Name <input name="name"></label>
  <label>Email <input name="email" type="email" placeholder="you@example.test"></label>
  <label><input type="checkbox" name="terms"> I accept the terms</label>
  <select name="plan" aria-label="Plan">
    <option value="free">Free</option>
    <option value="pro">Pro</option>
  </select>
  <button type="submit">Create account</button>
</form>
<ul>
  <li>Apple <button>Buy</button></li>
  <li>Banana <button>Buy</button></li>
</ul>
<output id="result"></output>"#;

#[tokio::main]
async fn main() -> oxidewright::Result<()> {
    let browser = chromium().launch().await?;
    let context = browser.new_context().await?;
    context
        .route("http://app.test/**", |route: Route| async move {
            route.fulfill().content_type("text/html").body(FORM).await
        })
        .await?;
    let page = context.new_page().await?;
    page.goto("http://app.test/signup").await?;

    // await page.getByLabel('Name').fill('Ada');
    page.get_by_label("Name").fill("Ada").await?;
    // await page.getByPlaceholder('you@example.test').fill(...)
    page.get_by_placeholder("you@example.test")
        .fill("ada@example.test")
        .await?;
    // await page.getByRole('checkbox', { name: /terms/i }).check();
    page.get_by_role_with(
        "checkbox",
        GetByRoleOptions::new().name(regex("terms", "i")),
    )
    .check()
    .await?;
    // await page.getByLabel('Plan').selectOption('pro');
    page.get_by_label("Plan").select_option("pro").await?;
    // await page.getByRole('button', { name: 'Create account' }).click();
    page.get_by_role_with("button", GetByRoleOptions::new().name("Create account"))
        .click()
        .await?;

    let result = page.locator("#result").text_content().await?;
    println!("submitted: {}", result.unwrap_or_default());

    // Filtering: the "Buy" button of the list item that contains "Banana".
    // await page.getByRole('listitem').filter({ hasText: 'Banana' }).getByRole('button').click();
    let banana = page
        .get_by_role("listitem")
        .filter(LocatorOptions::new().has_text("Banana"));
    banana.get_by_role("button").click().await?;
    println!(
        "list items: {:?}",
        page.get_by_role("listitem").all_inner_texts().await?
    );
    println!("buttons: {}", page.get_by_role("button").count().await?);

    browser.close().await?;
    Ok(())
}
