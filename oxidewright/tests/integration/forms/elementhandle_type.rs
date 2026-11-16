//! Port of `elementhandle-type.spec.ts` (nothing skipped).

use crate::common::*;

async fn value(t: &TestContext) -> String {
    t.page
        .eval_on_selector("input", "input => input.value")
        .await
        .unwrap()
}

#[test]
fn should_work() {
    run(|t| async move {
        t.page.set_content("<input type='text' />").await.unwrap();
        t.page.r#type("input", "hello").await.unwrap();
        assert_eq!(value(&t).await, "hello");
    });
}

#[test]
fn should_not_select_existing_value() {
    run(|t| async move {
        t.page
            .set_content("<input type='text' value='hello' />")
            .await
            .unwrap();
        t.page.r#type("input", "world").await.unwrap();
        assert_eq!(value(&t).await, "worldhello");
    });
}

#[test]
fn should_reset_selection_when_not_focused() {
    run(|t| async move {
        t.page
            .set_content("<input type='text' value='hello' /><div tabIndex=2>text</div>")
            .await
            .unwrap();
        t.page
            .eval_on_selector::<()>(
                "input",
                "input => { input.selectionStart = 2; input.selectionEnd = 4; document.querySelector('div').focus(); }",
            )
            .await
            .unwrap();
        t.page.r#type("input", "world").await.unwrap();
        assert_eq!(value(&t).await, "worldhello");
    });
}

#[test]
fn should_not_modify_selection_when_focused() {
    run(|t| async move {
        t.page
            .set_content("<input type='text' value='hello' />")
            .await
            .unwrap();
        t.page
            .eval_on_selector::<()>(
                "input",
                "input => { input.focus(); input.selectionStart = 2; input.selectionEnd = 4; }",
            )
            .await
            .unwrap();
        t.page.r#type("input", "world").await.unwrap();
        assert_eq!(value(&t).await, "heworldo");
    });
}

#[test]
fn should_work_with_number_input() {
    run(|t| async move {
        t.page
            .set_content("<input type='number' value=2 />")
            .await
            .unwrap();
        t.page.r#type("input", "13").await.unwrap();
        assert_eq!(value(&t).await, "132");
    });
}
