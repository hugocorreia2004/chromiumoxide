//! Port of `locator-list.spec.ts`. All tests ported.

use crate::common::*;

#[test]
fn locator_all_should_work() {
    run(|t| async move {
        t.page
            .set_content("<div><p>A</p><p>B</p><p>C</p></div>")
            .await
            .unwrap();
        let mut texts = Vec::new();
        for p in t.page.locator("div >> p").all().await.unwrap() {
            texts.push(p.text_content().await.unwrap().unwrap());
        }
        assert_eq!(texts, vec!["A", "B", "C"]);
    });
}
