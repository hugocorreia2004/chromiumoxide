//! Port of `page-check.spec.ts` (nothing skipped).

use crate::common::*;

async fn checked(t: &TestContext) -> bool {
    t.page
        .evaluate("() => window['checkbox'].checked")
        .await
        .unwrap()
}

async fn aria_checked(t: &TestContext) -> String {
    t.page
        .evaluate("() => window['checkbox'].getAttribute('aria-checked')")
        .await
        .unwrap()
}

#[test]
fn should_check_the_box() {
    run(|t| async move {
        t.page
            .set_content("<input id='checkbox' type='checkbox'></input>")
            .await
            .unwrap();
        t.page.check("input").await.unwrap();
        assert!(checked(&t).await);
    });
}

#[test]
fn should_not_check_the_checked_box() {
    run(|t| async move {
        t.page
            .set_content("<input id='checkbox' type='checkbox' checked></input>")
            .await
            .unwrap();
        t.page.check("input").await.unwrap();
        assert!(checked(&t).await);
    });
}

#[test]
fn should_uncheck_the_box() {
    run(|t| async move {
        t.page
            .set_content("<input id='checkbox' type='checkbox' checked></input>")
            .await
            .unwrap();
        t.page.uncheck("input").await.unwrap();
        assert!(!checked(&t).await);
    });
}

#[test]
fn should_not_uncheck_the_unchecked_box() {
    run(|t| async move {
        t.page
            .set_content("<input id='checkbox' type='checkbox'></input>")
            .await
            .unwrap();
        t.page.uncheck("input").await.unwrap();
        assert!(!checked(&t).await);
    });
}

#[test]
fn should_check_radio() {
    run(|t| async move {
        t.page
            .set_content(
                "<input type='radio'>one</input>
    <input id='two' type='radio'>two</input>
    <input type='radio'>three</input>",
            )
            .await
            .unwrap();
        t.page.check("#two").await.unwrap();
        let two: bool = t
            .page
            .evaluate("() => window['two'].checked")
            .await
            .unwrap();
        assert!(two);
    });
}

#[test]
fn should_check_radio_by_aria_role() {
    run(|t| async move {
        t.page
            .set_content(
                "<div role='radio' id='checkbox'>CHECKBOX</div>
    <script>
      checkbox.addEventListener('click', () => checkbox.setAttribute('aria-checked', 'true'));
    </script>",
            )
            .await
            .unwrap();
        t.page.check("div").await.unwrap();
        assert_eq!(aria_checked(&t).await, "true");
    });
}

#[test]
fn should_uncheck_radio_by_aria_role() {
    run(|t| async move {
        t.page
            .set_content(
                "<div role='radio' id='checkbox' aria-checked=\"true\">CHECKBOX</div>
    <script>
      checkbox.addEventListener('click', () => checkbox.setAttribute('aria-checked', 'false'));
    </script>",
            )
            .await
            .unwrap();
        t.page.uncheck("div").await.unwrap();
        assert_eq!(aria_checked(&t).await, "false");
    });
}

const ROLES: [&str; 7] = [
    "checkbox",
    "menuitemcheckbox",
    "option",
    "radio",
    "switch",
    "menuitemradio",
    "treeitem",
];

#[test]
fn should_check_the_box_by_aria_role() {
    run(|t| async move {
        for role in ROLES {
            t.page
                .set_content(format!(
                    "<div role='{role}' id='checkbox'>CHECKBOX</div>
        <script>
          checkbox.addEventListener('click', () => checkbox.setAttribute('aria-checked', 'true'));
        </script>"
                ))
                .await
                .unwrap();
            t.page.check("div").await.unwrap();
            assert_eq!(aria_checked(&t).await, "true", "role={role}");
        }
    });
}

#[test]
fn should_uncheck_the_box_by_aria_role() {
    run(|t| async move {
        for role in ROLES {
            t.page
                .set_content(format!(
                    "<div role='{role}' id='checkbox' aria-checked=\"true\">CHECKBOX</div>
        <script>
          checkbox.addEventListener('click', () => checkbox.setAttribute('aria-checked', 'false'));
        </script>"
                ))
                .await
                .unwrap();
            t.page.uncheck("div").await.unwrap();
            assert_eq!(aria_checked(&t).await, "false", "role={role}");
        }
    });
}

#[test]
fn should_throw_when_not_a_checkbox() {
    run(|t| async move {
        t.page.set_content("<div>Check me</div>").await.unwrap();
        let error = t.page.check("div").await.unwrap_err();
        assert!(
            error.to_string().contains("Not a checkbox or radio button"),
            "{error}"
        );
    });
}

#[test]
fn should_throw_when_not_a_checkbox_2() {
    run(|t| async move {
        t.page
            .set_content("<div role=button>Check me</div>")
            .await
            .unwrap();
        let error = t.page.check("div").await.unwrap_err();
        assert!(
            error.to_string().contains("Not a checkbox or radio button"),
            "{error}"
        );
    });
}

#[test]
fn should_check_the_box_inside_a_button() {
    run(|t| async move {
        t.page
            .set_content("<div role='button'><input type='checkbox'></div>")
            .await
            .unwrap();
        t.page.check("input").await.unwrap();
        let c: bool = t
            .page
            .eval_on_selector("input", "input => input.checked")
            .await
            .unwrap();
        assert!(c);
        assert!(t.page.is_checked("input").await.unwrap());
        let handle = t.page.query_selector("input").await.unwrap().unwrap();
        assert!(handle.is_checked().await.unwrap());
    });
}

#[test]
fn should_check_the_label_with_position() {
    run(|t| async move {
        t.page
            .set_content(format!(
                "<input id='checkbox' type='checkbox' style='width: 5px; height: 5px;'>
    <label for='checkbox'>
      <a href={}>I am a long link that goes away so that nothing good will happen if you click on me</a>
      Click me
    </label>",
                serde_json::to_string(&t.server.empty_page).unwrap()
            ))
            .await
            .unwrap();
        let bbox = t
            .page
            .query_selector("text=Click me")
            .await
            .unwrap()
            .unwrap()
            .bounding_box()
            .await
            .unwrap()
            .unwrap();
        t.page
            .check("text=Click me")
            .position((bbox.width - 10.0, 2.0))
            .await
            .unwrap();
        let c: bool = t
            .page
            .eval_on_selector("input", "input => input.checked")
            .await
            .unwrap();
        assert!(c);
    });
}

#[test]
fn trial_run_should_not_check() {
    run(|t| async move {
        t.page
            .set_content("<input id='checkbox' type='checkbox'></input>")
            .await
            .unwrap();
        t.page.check("input").trial(true).await.unwrap();
        assert!(!checked(&t).await);
    });
}

#[test]
fn trial_run_should_not_uncheck() {
    run(|t| async move {
        t.page
            .set_content("<input id='checkbox' type='checkbox' checked></input>")
            .await
            .unwrap();
        t.page.uncheck("input").trial(true).await.unwrap();
        assert!(checked(&t).await);
    });
}

#[test]
fn should_check_the_box_using_set_checked() {
    run(|t| async move {
        t.page
            .set_content("<input id='checkbox' type='checkbox'></input>")
            .await
            .unwrap();
        t.page.set_checked("input", true).await.unwrap();
        assert!(checked(&t).await);
        t.page.set_checked("input", false).await.unwrap();
        assert!(!checked(&t).await);
    });
}

#[test]
fn should_throw_when_trying_to_uncheck_radio_button() {
    run(|t| async move {
        t.page
            .set_content("<input type='radio' name='test' checked id='radio'>")
            .await
            .unwrap();
        let error = t.page.uncheck("#radio").await.unwrap_err();
        assert!(
            error.to_string().contains("Cannot uncheck radio button"),
            "{error}"
        );
    });
}
