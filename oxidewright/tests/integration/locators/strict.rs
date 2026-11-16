//! Port of `page-strict.spec.ts`.
//!
//! Skipped:
//! - SM-004 `should fail page.$ in strict mode`: `query_selector` has no
//!   `strict` option in this crate.

use super::*;
use crate::common::*;

#[test]
fn should_fail_page_text_content_in_strict_mode() {
    run(|t| async move {
        t.page
            .set_content("<span>span1</span><div><span>target</span></div>")
            .await
            .unwrap();
        let error = t
            .page
            .text_content("span")
            .strict(true)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("strict mode violation"), "{error}");
        assert!(
            error.contains("1) <span>span1</span> aka getByText('span1')"),
            "{error}"
        );
        assert!(
            error.contains("2) <span>target</span> aka getByText('target')"),
            "{error}"
        );
    });
}

#[test]
fn should_fail_page_get_attribute_in_strict_mode() {
    run(|t| async move {
        t.page
            .set_content("<span>span1</span><div><span>target</span></div>")
            .await
            .unwrap();
        let error = t
            .page
            .get_attribute("span", "id")
            .strict(true)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("strict mode violation"));
    });
}

#[test]
fn should_fail_page_fill_in_strict_mode() {
    run(|t| async move {
        t.page
            .set_content("<input></input><div><input></input></div>")
            .await
            .unwrap();
        let error = t
            .page
            .fill("input", "text")
            .strict(true)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("strict mode violation"), "{error}");
        assert!(
            error.contains("1) <input/> aka getByRole('textbox').first()"),
            "{error}"
        );
        assert!(
            error.contains("2) <input/> aka locator('div').getByRole('textbox')"),
            "{error}"
        );
    });
}

#[test]
fn should_fail_page_wait_for_selector_in_strict_mode() {
    run(|t| async move {
        t.page
            .set_content("<span>span1</span><div><span>target</span></div>")
            .await
            .unwrap();
        let error = t
            .page
            .wait_for_selector("span")
            .strict(true)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("strict mode violation"));
    });
}

#[test]
fn should_fail_page_dispatch_event_in_strict_mode() {
    run(|t| async move {
        t.page
            .set_content("<span></span><div><span></span></div>")
            .await
            .unwrap();
        let error = t
            .page
            .dispatch_event("span", "click")
            .event_init(serde_json::json!({}))
            .strict(true)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("strict mode violation"), "{error}");
        assert!(
            error.contains("1) <span></span> aka locator('span').first()"),
            "{error}"
        );
        assert!(
            error.contains("2) <span></span> aka locator('div span')"),
            "{error}"
        );
    });
}

#[test]
fn should_properly_format_nth_child_in_strict_mode_message() {
    run(|t| async move {
        t.page
            .set_content(
                r#"
  <div>
    <div>
    </div>
    <div>
      <div>
      </div>
      <div>
      </div>
    </div>
  </div>
  <div>
    <div class='foo'>
    </div>
    <div class='foo'>
    </div>
  </div>"#,
            )
            .await
            .unwrap();
        let error = t
            .page
            .locator(".foo")
            .hover()
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("strict mode violation"), "{error}");
        assert!(
            error.contains("body > div:nth-child(2) > div:nth-child(2)"),
            "{error}"
        );
    });
}

#[test]
fn should_escape_class_names() {
    run(|t| async move {
        t.page
            .set_content(
                r#"
  <div>
    <div></div>
    <div>
      <div></div>
      <div></div>
    </div>
  </div>
  <div>
    <div class='foo bar:0'>
    </div>
    <div class='foo bar:1'>
    </div>
  </div>"#,
            )
            .await
            .unwrap();
        let error = t
            .page
            .locator(".foo")
            .hover()
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("strict mode violation"), "{error}");
        assert!(error.contains("<div class=\"foo bar:0"), "{error}");
        assert!(error.contains("<div class=\"foo bar:1"), "{error}");
    });
}

#[test]
fn should_escape_tag_names() {
    run(|t| async move {
        t.page
            .set_content(
                r#"
    <q:template> </q:template>
    <span>special test description</span>
    <q:template hidden="" aria-hidden="true">
      <span>special test description</span>
    </q:template>"#,
            )
            .await
            .unwrap();
        let error = try_visible(&t.page.get_by_text("special test description"), true)
            .await
            .unwrap_err();
        assert!(error.contains("strict mode violation"), "{error}");
        assert!(
            error.contains("getByText('special test description').first()"),
            "{error}"
        );
        assert!(
            error.contains(
                "locator('q\\\\:template').filter({ hasText: 'special test description' })"
            ),
            "{error}"
        );
    });
}

#[test]
fn should_keep_the_engine_name_for_a_text_selector_in_strict_mode() {
    run(|t| async move {
        t.page
            .set_content("<div>..loading</div><div>..loading</div>")
            .await
            .unwrap();
        let error = t
            .page
            .locator("text=..loading")
            .hover()
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("strict mode violation"), "{error}");
        assert!(error.contains("locator('text=..loading')"), "{error}");
    });
}
