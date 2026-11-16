//! Port of `expect-boolean.spec.ts`.
//!
//! Skipped:
//! * `EX-068 toBeOK fail with invalid argument`, `EX-069 toBeOK fail with
//!   promise`: `to_be_ok` only exists on API responses, the type system
//!   rejects other receivers.

use std::time::Duration;

use oxidewright::expect::expect;

use super::*;
use crate::common::*;

// ---------------------------------------------------------------- toBeChecked

#[test]
fn to_be_checked_default() {
    run(|t| async move {
        t.page
            .set_content("<input type=checkbox checked></input>")
            .await
            .unwrap();
        expect(&t.page.locator("input"))
            .to_be_checked()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_checked_with_checked_true() {
    run(|t| async move {
        t.page
            .set_content("<input type=checkbox checked></input>")
            .await
            .unwrap();
        expect(&t.page.locator("input"))
            .to_be_checked()
            .checked(true)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_checked_with_checked_false() {
    run(|t| async move {
        t.page
            .set_content("<input type=checkbox checked></input>")
            .await
            .unwrap();
        expect(&t.page.locator("input"))
            .not()
            .to_be_checked()
            .checked(false)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_checked_with_indeterminate_true() {
    run(|t| async move {
        t.page
            .set_content("<input type=checkbox></input>")
            .await
            .unwrap();
        t.page
            .locator("input")
            .evaluate::<()>("e => { e.indeterminate = true }")
            .await
            .unwrap();
        expect(&t.page.locator("input"))
            .to_be_checked()
            .indeterminate(true)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_checked_with_indeterminate_true_and_checked() {
    run(|t| async move {
        t.page
            .set_content("<input type=checkbox></input>")
            .await
            .unwrap();
        t.page
            .locator("input")
            .evaluate::<()>("e => { e.indeterminate = true }")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("input"))
                .to_be_checked()
                .indeterminate(true)
                .checked(false)
                .await,
        );
        assert_contains(
            &m,
            "Can't assert indeterminate and checked at the same time",
        );
    });
}

#[test]
fn to_be_checked_fail() {
    run(|t| async move {
        t.page
            .set_content("<input type=checkbox></input>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("input"))
                .to_be_checked()
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).toBeChecked() failed\n\nLocator:  locator('input')\nExpected: checked\nReceived: unchecked\nTimeout:  1000ms",
        );
        assert_contains(
            &m,
            "- Expect \"toBeChecked\" locator('input') with timeout 1000ms",
        );
    });
}

#[test]
fn to_be_checked_with_not() {
    run(|t| async move {
        t.page
            .set_content("<input type=checkbox></input>")
            .await
            .unwrap();
        expect(&t.page.locator("input"))
            .not()
            .to_be_checked()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_checked_with_not_and_checked_false() {
    run(|t| async move {
        t.page
            .set_content("<input type=checkbox></input>")
            .await
            .unwrap();
        expect(&t.page.locator("input"))
            .to_be_checked()
            .checked(false)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_checked_fail_with_not() {
    run(|t| async move {
        t.page
            .set_content("<input type=checkbox checked></input>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("input"))
                .not()
                .to_be_checked()
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).not.toBeChecked() failed\n\nLocator:  locator('input')\nExpected: not checked\nReceived: checked\nTimeout:  1000ms",
        );
        assert_contains(
            &m,
            "- Expect \"not toBeChecked\" locator('input') with timeout 1000ms",
        );
        assert_contains(&m, "locator resolved to <input checked type=\"checkbox\"/>");
    });
}

#[test]
fn to_be_checked_fail_with_checked_false() {
    run(|t| async move {
        t.page
            .set_content("<input type=checkbox checked></input>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("input"))
                .to_be_checked()
                .checked(false)
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).toBeChecked({ checked: false }) failed\n\nLocator:  locator('input')\nExpected: unchecked\nReceived: checked\nTimeout:  1000ms",
        );
        assert_contains(
            &m,
            "- Expect \"toBeChecked\" locator('input') with timeout 1000ms",
        );
        assert_contains(&m, "locator resolved to <input checked type=\"checkbox\"/>");
    });
}

#[test]
fn to_be_checked_fail_with_indeterminate_true() {
    run(|t| async move {
        t.page
            .set_content("<input type=checkbox></input>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("input"))
                .to_be_checked()
                .indeterminate(true)
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).toBeChecked({ indeterminate: true }) failed\n\nLocator:  locator('input')\nExpected: indeterminate\nReceived: unchecked\nTimeout:  1000ms",
        );
        assert_contains(
            &m,
            "- Expect \"toBeChecked\" locator('input') with timeout 1000ms",
        );
    });
}

#[test]
fn to_be_checked_fail_missing() {
    run(|t| async move {
        t.page
            .set_content("<div>no inputs here</div>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("input2"))
                .not()
                .to_be_checked()
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).not.toBeChecked() failed\n\nLocator: locator('input2')\nExpected: not checked\nTimeout: 1000ms\nError: element(s) not found\n\nCall log:\n  - Expect \"not toBeChecked\" locator('input2') with timeout 1000ms\n  - waiting for locator('input2')\n",
        );
    });
}

#[test]
fn to_be_checked_with_role() {
    run(|t| async move {
        for role in [
            "checkbox",
            "menuitemcheckbox",
            "option",
            "radio",
            "switch",
            "menuitemradio",
            "treeitem",
        ] {
            t.page
                .set_content(format!(
                    "<div role={role} aria-checked=true>I am checked</div>"
                ))
                .await
                .unwrap();
            expect(&t.page.locator("div"))
                .to_be_checked()
                .await
                .unwrap();
        }
    });
}

#[test]
fn to_be_checked_friendly_log() {
    run(|t| async move {
        t.page
            .set_content("<input type=checkbox></input>")
            .await
            .unwrap();
        let m1 = message(
            expect(&t.page.locator("input"))
                .to_be_checked()
                .timeout(1000)
                .await,
        );
        assert_contains(&m1, "unexpected value \"unchecked\"");
        t.page
            .set_content("<input type=checkbox checked></input>")
            .await
            .unwrap();
        let m2 = message(
            expect(&t.page.locator("input"))
                .to_be_checked()
                .checked(false)
                .timeout(1000)
                .await,
        );
        assert_contains(&m2, "unexpected value \"checked\"");
    });
}

#[test]
fn to_be_checked_with_impossible_timeout() {
    run(|t| async move {
        t.page
            .set_content("<input type=checkbox checked></input>")
            .await
            .unwrap();
        expect(&t.page.locator("input"))
            .to_be_checked()
            .timeout(1)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_checked_with_impossible_timeout_not() {
    run(|t| async move {
        t.page
            .set_content("<input type=checkbox></input>")
            .await
            .unwrap();
        expect(&t.page.locator("input"))
            .not()
            .to_be_checked()
            .timeout(1)
            .await
            .unwrap();
    });
}

// --------------------------------------------------------------- toBeEditable

#[test]
fn to_be_editable_default() {
    run(|t| async move {
        t.page.set_content("<input></input>").await.unwrap();
        expect(&t.page.locator("input"))
            .to_be_editable()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_editable_with_not() {
    run(|t| async move {
        t.page
            .set_content("<input readonly></input>")
            .await
            .unwrap();
        expect(&t.page.locator("input"))
            .not()
            .to_be_editable()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_editable_with_editable_true() {
    run(|t| async move {
        t.page.set_content("<input></input>").await.unwrap();
        expect(&t.page.locator("input"))
            .to_be_editable()
            .editable(true)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_editable_with_editable_false() {
    run(|t| async move {
        t.page
            .set_content("<input readonly></input>")
            .await
            .unwrap();
        expect(&t.page.locator("input"))
            .to_be_editable()
            .editable(false)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_editable_with_not_and_editable_false() {
    run(|t| async move {
        t.page.set_content("<input></input>").await.unwrap();
        expect(&t.page.locator("input"))
            .not()
            .to_be_editable()
            .editable(false)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_editable_throws() {
    run(|t| async move {
        t.page.set_content("<button>").await.unwrap();
        let m = message(expect(&t.page.locator("button")).to_be_editable().await);
        assert_contains(
            &m,
            "Element is not an <input>, <textarea>, <select> or [contenteditable] and does not have a role allowing [aria-readonly]",
        );
    });
}

// ---------------------------------------------------------------- toBeEnabled

#[test]
fn to_be_enabled_default() {
    run(|t| async move {
        t.page.set_content("<button>Text</button>").await.unwrap();
        expect(&t.page.locator("button"))
            .to_be_enabled()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_enabled_with_enabled_true() {
    run(|t| async move {
        t.page.set_content("<button>Text</button>").await.unwrap();
        expect(&t.page.locator("button"))
            .to_be_enabled()
            .enabled(true)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_enabled_with_enabled_false() {
    run(|t| async move {
        t.page
            .set_content("<button disabled>Text</button>")
            .await
            .unwrap();
        expect(&t.page.locator("button"))
            .to_be_enabled()
            .enabled(false)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_enabled_failed() {
    run(|t| async move {
        t.page
            .set_content("<button disabled>Text</button>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("button"))
                .to_be_enabled()
                .timeout(1000)
                .await,
        );
        assert_contains(&m, "locator resolved to <button disabled>Text</button>");
    });
}

#[test]
fn to_be_enabled_eventually() {
    run(|t| async move {
        t.page
            .set_content("<button disabled>Text</button>")
            .await
            .unwrap();
        later(
            &t.page,
            500,
            "() => document.querySelector('button').removeAttribute('disabled')",
        );
        expect(&t.page.locator("button"))
            .to_be_enabled()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_enabled_eventually_with_not() {
    run(|t| async move {
        t.page.set_content("<button>Text</button>").await.unwrap();
        later(
            &t.page,
            500,
            "() => document.querySelector('button').setAttribute('disabled', '')",
        );
        expect(&t.page.locator("button"))
            .not()
            .to_be_enabled()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_enabled_with_not_and_enabled_false() {
    run(|t| async move {
        t.page.set_content("<button>Text</button>").await.unwrap();
        expect(&t.page.locator("button"))
            .not()
            .to_be_enabled()
            .enabled(false)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_enabled_to_be_disabled() {
    run(|t| async move {
        t.page
            .set_content("<button disabled>Text</button>")
            .await
            .unwrap();
        expect(&t.page.locator("button"))
            .to_be_disabled()
            .await
            .unwrap();
    });
}

// ------------------------------------------------------------------- misc

#[test]
fn to_be_empty_input() {
    run(|t| async move {
        t.page.set_content("<input></input>").await.unwrap();
        expect(&t.page.locator("input"))
            .to_be_empty()
            .await
            .unwrap();
    });
}

#[test]
fn not_to_be_empty() {
    run(|t| async move {
        t.page
            .set_content("<input value=text></input>")
            .await
            .unwrap();
        expect(&t.page.locator("input"))
            .not()
            .to_be_empty()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_empty_div() {
    run(|t| async move {
        t.page
            .set_content("<div style=\"width: 50; height: 50px\"></div>")
            .await
            .unwrap();
        expect(&t.page.locator("div")).to_be_empty().await.unwrap();
    });
}

#[test]
fn to_be_disabled_with_value() {
    run(|t| async move {
        t.page
            .set_content("<button disabled=\"yes\">Text</button>")
            .await
            .unwrap();
        expect(&t.page.locator("button"))
            .to_be_disabled()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_checked_with_value() {
    run(|t| async move {
        t.page
            .set_content("<input type=checkbox checked=\"yes\"></input>")
            .await
            .unwrap();
        expect(&t.page.locator("input"))
            .to_be_checked()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_hidden_with_value() {
    run(|t| async move {
        t.page
            .set_content("<input type=checkbox hidden=\"of course\"></input>")
            .await
            .unwrap();
        expect(&t.page.locator("input"))
            .to_be_hidden()
            .await
            .unwrap();
    });
}

#[test]
fn not_to_be_disabled_div() {
    run(|t| async move {
        t.page
            .set_content("<div disabled=\"yes\"></div>")
            .await
            .unwrap();
        expect(&t.page.locator("div"))
            .not()
            .to_be_disabled()
            .await
            .unwrap();
    });
}

// ---------------------------------------------------------------- toBeVisible

#[test]
fn to_be_visible_default() {
    run(|t| async move {
        t.page.set_content("<input></input>").await.unwrap();
        expect(&t.page.locator("input"))
            .to_be_visible()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_visible_with_not() {
    run(|t| async move {
        t.page
            .set_content("<button style=\"display: none\">hello</button>")
            .await
            .unwrap();
        expect(&t.page.locator("button"))
            .not()
            .to_be_visible()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_visible_with_visible_true() {
    run(|t| async move {
        t.page.set_content("<button>hello</button>").await.unwrap();
        expect(&t.page.locator("button"))
            .to_be_visible()
            .visible(true)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_visible_with_visible_false() {
    run(|t| async move {
        t.page
            .set_content("<button hidden>hello</button>")
            .await
            .unwrap();
        expect(&t.page.locator("button"))
            .to_be_visible()
            .visible(false)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_visible_with_not_and_visible_false() {
    run(|t| async move {
        t.page.set_content("<button>hello</button>").await.unwrap();
        expect(&t.page.locator("button"))
            .not()
            .to_be_visible()
            .visible(false)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_visible_eventually() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        later(
            &t.page,
            0,
            "() => { document.querySelector('div').innerHTML = '<span>Hello</span>' }",
        );
        expect(&t.page.locator("span"))
            .to_be_visible()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_visible_eventually_with_not() {
    run(|t| async move {
        t.page
            .set_content("<div><span>Hello</span></div>")
            .await
            .unwrap();
        later(
            &t.page,
            0,
            "() => { document.querySelector('span').textContent = '' }",
        );
        expect(&t.page.locator("span"))
            .not()
            .to_be_visible()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_visible_fail() {
    run(|t| async move {
        t.page
            .set_content("<button style=\"display: none\"></button>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("button"))
                .to_be_visible()
                .timeout(1000)
                .await,
        );
        assert_contains(&m, "locator resolved to <button></button>");
    });
}

#[test]
fn to_be_visible_fail_with_not() {
    run(|t| async move {
        t.page.set_content("<input></input>").await.unwrap();
        let m = message(
            expect(&t.page.locator("input"))
                .not()
                .to_be_visible()
                .timeout(1000)
                .await,
        );
        assert_contains(&m, "locator resolved to <input/>");
    });
}

#[test]
fn to_be_visible_with_impossible_timeout() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        expect(&t.page.locator("#node"))
            .to_be_visible()
            .timeout(1)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_visible_with_impossible_timeout_not() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        expect(&t.page.locator("no-such-thing"))
            .not()
            .to_be_visible()
            .timeout(1)
            .await
            .unwrap();
    });
}

async fn not_done_after_1s(task: &tokio::task::JoinHandle<oxidewright::Result<()>>) {
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert!(!task.is_finished());
}

#[test]
fn to_be_visible_with_frame_locator() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let locator = t.page.frame_locator("iframe").locator("input");
        let task = tokio::spawn(async move { expect(&locator).to_be_visible().await });
        not_done_after_1s(&task).await;
        t.page
            .set_content("<iframe srcdoc=\"<input>\"></iframe>")
            .await
            .unwrap();
        task.await.unwrap().unwrap();
    });
}

#[test]
fn to_be_visible_with_frame_locator_2() {
    run(|t| async move {
        t.page.set_content("<iframe></iframe>").await.unwrap();
        let locator = t.page.frame_locator("iframe").locator("input");
        let task = tokio::spawn(async move { expect(&locator).to_be_visible().await });
        not_done_after_1s(&task).await;
        t.page
            .set_content("<iframe srcdoc=\"<input>\"></iframe>")
            .await
            .unwrap();
        task.await.unwrap().unwrap();
    });
}

#[test]
fn to_be_visible_over_navigation() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let locator = t.page.locator("input");
        let task = tokio::spawn(async move { expect(&locator).to_be_visible().await });
        not_done_after_1s(&task).await;
        t.page
            .goto(format!("{}/input/checkbox.html", t.server.prefix))
            .await
            .unwrap();
        task.await.unwrap().unwrap();
    });
}

// ----------------------------------------------------------------- toBeHidden

#[test]
fn to_be_hidden_default() {
    run(|t| async move {
        t.page
            .set_content("<button style=\"display: none\"></button>")
            .await
            .unwrap();
        expect(&t.page.locator("button"))
            .to_be_hidden()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_hidden_when_nothing_matches() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        expect(&t.page.locator("button"))
            .to_be_hidden()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_hidden_with_not() {
    run(|t| async move {
        t.page.set_content("<input></input>").await.unwrap();
        expect(&t.page.locator("input"))
            .not()
            .to_be_hidden()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_hidden_eventually_with_not() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        later(
            &t.page,
            0,
            "() => { document.querySelector('div').innerHTML = '<span>Hello</span>' }",
        );
        expect(&t.page.locator("span"))
            .not()
            .to_be_hidden()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_hidden_eventually() {
    run(|t| async move {
        t.page
            .set_content("<div><span>Hello</span></div>")
            .await
            .unwrap();
        later(
            &t.page,
            0,
            "() => { document.querySelector('span').textContent = '' }",
        );
        expect(&t.page.locator("span"))
            .to_be_hidden()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_hidden_fail() {
    run(|t| async move {
        t.page.set_content("<input></input>").await.unwrap();
        let m = message(
            expect(&t.page.locator("input"))
                .to_be_hidden()
                .timeout(3000)
                .await,
        );
        assert_contains(&m, "locator resolved to <input/>");
    });
}

#[test]
fn to_be_hidden_fail_with_not() {
    run(|t| async move {
        t.page
            .set_content("<button style=\"display: none\"></button>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page.locator("button"))
                .not()
                .to_be_hidden()
                .timeout(3000)
                .await,
        );
        assert_contains(&m, "locator resolved to <button></button>");
    });
}

#[test]
fn to_be_hidden_fail_with_not_when_nothing_matching() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let m = message(
            expect(&t.page.locator("button"))
                .not()
                .to_be_hidden()
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "expect(locator).not.toBeHidden() failed\n\nLocator: locator('button')\nExpected: not hidden\nTimeout: 1000ms\nError: element(s) not found\n\nCall log:\n  - Expect \"not toBeHidden\" locator('button') with timeout 1000ms\n",
        );
    });
}

#[test]
fn to_be_hidden_with_impossible_timeout_not() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        expect(&t.page.locator("#node"))
            .not()
            .to_be_hidden()
            .timeout(1)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_hidden_with_impossible_timeout() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        expect(&t.page.locator("no-such-thing"))
            .to_be_hidden()
            .timeout(1)
            .await
            .unwrap();
    });
}

// ---------------------------------------------------------------- toBeFocused

#[test]
fn to_be_focused() {
    run(|t| async move {
        t.page.set_content("<input></input>").await.unwrap();
        let locator = t.page.locator("input");
        locator.focus().await.unwrap();
        expect(&locator).to_be_focused().await.unwrap();
    });
}

#[test]
fn to_be_focused_with_shadow_elements() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<div id="app"></div>
    <script>
      const root = document.querySelector('div');
      const shadowRoot = root.attachShadow({ mode: 'open' });
      const input = document.createElement('input');
      input.id = "my-input"
      shadowRoot.appendChild(input);
    </script>"#,
            )
            .await
            .unwrap();
        t.page.locator("input").focus().await.unwrap();
        let id: String = t
            .page
            .evaluate("() => document.activeElement.shadowRoot.activeElement.id")
            .await
            .unwrap();
        assert_eq!(id, "my-input");
        expect(&t.page.locator("#app"))
            .to_be_focused()
            .await
            .unwrap();
        expect(&t.page.locator("input"))
            .to_be_focused()
            .await
            .unwrap();
    });
}

#[test]
fn should_print_unknown_engine_error() {
    run(|t| async move {
        let m = message(expect(&t.page.locator("row=\"row\"")).to_be_visible().await);
        assert_contains(
            &m,
            "Unknown engine \"row\" while parsing selector row=\"row\"",
        );
    });
}

#[test]
fn should_print_selector_syntax_error() {
    run(|t| async move {
        let m = message(expect(&t.page.locator("row]")).to_be_visible().await);
        assert_contains(
            &m,
            "Unexpected token \"]\" while parsing css selector \"row]\"",
        );
    });
}

// ------------------------------------------------------------------- toBeOK

#[test]
fn to_be_ok() {
    run(|t| async move {
        let res = t.page.request().get(&t.server.empty_page).await.unwrap();
        expect(&res).to_be_ok().await.unwrap();
    });
}

#[test]
fn not_to_be_ok() {
    run(|t| async move {
        let res = t
            .page
            .request()
            .get(format!("{}/unknown", t.server.prefix))
            .await
            .unwrap();
        expect(&res).not().to_be_ok().await.unwrap();
    });
}

fn set_error_routes(server: &TestServer) {
    use axum::response::Response;
    for (path, content_type, body) in [
        ("/text-content-type", Some("text/plain"), "Text error"),
        ("/no-content-type", None, "No content type error"),
        (
            "/binary-content-type",
            Some("image/bmp"),
            "Image content type error",
        ),
    ] {
        server.set_route(path, move |_| async move {
            let mut builder = Response::builder().status(404);
            if let Some(ct) = content_type {
                builder = builder.header("content-type", ct);
            }
            builder.body(axum::body::Body::from(body)).unwrap()
        });
    }
}

#[test]
fn to_be_ok_should_print_response_with_text_content_type_when_fails_text_content_type() {
    run(|t| async move {
        set_error_routes(&t.server);
        let url = format!("{}/text-content-type", t.server.prefix);
        let res = t.page.request().get(&url).await.unwrap();
        let m = message(expect(&res).to_be_ok().await);
        assert_contains(&m, "expect(response).toBeOK() failed");
        assert_contains(&m, &format!("→ GET {url}"));
        assert_contains(&m, "← 404 Not Found");
        assert_contains(&m, "Response text:\nText error");
    });
}

#[test]
fn to_be_ok_should_print_response_with_text_content_type_when_fails_no_content_type() {
    run(|t| async move {
        set_error_routes(&t.server);
        let url = format!("{}/no-content-type", t.server.prefix);
        let res = t.page.request().get(&url).await.unwrap();
        let m = message(expect(&res).to_be_ok().await);
        assert_contains(&m, &format!("→ GET {url}"));
        assert_contains(&m, "← 404 Not Found");
        assert_not_contains(&m, "No content type error");
    });
}

#[test]
fn to_be_ok_should_print_response_with_text_content_type_when_fails_image_content_type() {
    run(|t| async move {
        set_error_routes(&t.server);
        let url = format!("{}/image-content-type", t.server.prefix);
        let res = t.page.request().get(&url).await.unwrap();
        let m = message(expect(&res).to_be_ok().await);
        assert_contains(&m, &format!("→ GET {url}"));
        assert_contains(&m, "← 404 Not Found");
        assert_not_contains(&m, "Image content type error");
    });
}

// --------------------------------------------------------------- toBeAttached

#[test]
fn to_be_attached_default() {
    run(|t| async move {
        t.page.set_content("<input></input>").await.unwrap();
        expect(&t.page.locator("input"))
            .to_be_attached()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_attached_with_hidden_element() {
    run(|t| async move {
        t.page
            .set_content("<button style=\"display:none\">hello</button>")
            .await
            .unwrap();
        expect(&t.page.locator("button"))
            .to_be_attached()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_attached_with_not() {
    run(|t| async move {
        t.page.set_content("<button>hello</button>").await.unwrap();
        expect(&t.page.locator("input"))
            .not()
            .to_be_attached()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_attached_with_attached_true() {
    run(|t| async move {
        t.page.set_content("<button>hello</button>").await.unwrap();
        expect(&t.page.locator("button"))
            .to_be_attached()
            .attached(true)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_attached_with_attached_false() {
    run(|t| async move {
        t.page.set_content("<button>hello</button>").await.unwrap();
        expect(&t.page.locator("input"))
            .to_be_attached()
            .attached(false)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_attached_with_not_and_attached_false() {
    run(|t| async move {
        t.page.set_content("<button>hello</button>").await.unwrap();
        expect(&t.page.locator("button"))
            .not()
            .to_be_attached()
            .attached(false)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_attached_eventually() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        later(
            &t.page,
            0,
            "() => { document.querySelector('div').innerHTML = '<span>Hello</span>' }",
        );
        expect(&t.page.locator("span"))
            .to_be_attached()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_attached_eventually_with_not() {
    run(|t| async move {
        t.page
            .set_content("<div><span>Hello</span></div>")
            .await
            .unwrap();
        later(
            &t.page,
            0,
            "() => { document.querySelector('div').textContent = '' }",
        );
        expect(&t.page.locator("span"))
            .not()
            .to_be_attached()
            .await
            .unwrap();
    });
}

#[test]
fn to_be_attached_fail() {
    run(|t| async move {
        t.page.set_content("<button>Hello</button>").await.unwrap();
        let m = message(
            expect(&t.page.locator("input"))
                .to_be_attached()
                .timeout(1000)
                .await,
        );
        assert_not_contains(&m, "locator resolved to");
    });
}

#[test]
fn to_be_attached_fail_with_not() {
    run(|t| async move {
        t.page.set_content("<input></input>").await.unwrap();
        let m = message(
            expect(&t.page.locator("input"))
                .not()
                .to_be_attached()
                .timeout(3000)
                .await,
        );
        assert_contains(&m, "locator resolved to <input/>");
    });
}

#[test]
fn to_be_attached_with_impossible_timeout() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        expect(&t.page.locator("#node"))
            .to_be_attached()
            .timeout(1)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_attached_with_impossible_timeout_not() {
    run(|t| async move {
        t.page
            .set_content("<div id=node>Text content</div>")
            .await
            .unwrap();
        expect(&t.page.locator("no-such-thing"))
            .not()
            .to_be_attached()
            .timeout(1)
            .await
            .unwrap();
    });
}

#[test]
fn to_be_attached_with_frame_locator() {
    run(|t| async move {
        t.page.set_content("<div></div>").await.unwrap();
        let locator = t.page.frame_locator("iframe").locator("input");
        let task = tokio::spawn(async move { expect(&locator).to_be_attached().await });
        not_done_after_1s(&task).await;
        t.page
            .set_content("<iframe srcdoc=\"<input>\"></iframe>")
            .await
            .unwrap();
        task.await.unwrap().unwrap();
    });
}

#[test]
fn to_be_attached_over_navigation() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let locator = t.page.locator("input");
        let task = tokio::spawn(async move { expect(&locator).to_be_attached().await });
        not_done_after_1s(&task).await;
        t.page
            .goto(format!("{}/input/checkbox.html", t.server.prefix))
            .await
            .unwrap();
        task.await.unwrap().unwrap();
    });
}
