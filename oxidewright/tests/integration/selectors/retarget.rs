//! Port of `retarget.spec.ts`. All tests are ported; web-first `expect`
//! matchers are replaced by the equivalent queries (`toHaveValues` by
//! reading the selected options, `toBeFocused` by `document.activeElement`).
//!
//! RE-007: the disabled `<option>`/`<optgroup>` cases are left out, they
//! need Playwright 1.65's injected script (issue 43634); oxidewright bundles
//! Patchright's 1.64 build.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use oxidewright::ElementState;

use super::*;
use crate::common::*;

#[test]
fn element_state_checks_should_work_as_expected_for_label_with_zero_sized_input() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"<label>
      Click me
      <input disabled style="width:0;height:0;padding:0;margin:0;border:0;">
    </label>"#,
        )
        .await
        .unwrap();
        assert!(p.is_visible("text=Click me").await.unwrap());
        assert!(!p.is_hidden("text=Click me").await.unwrap());
        assert!(!p.is_enabled("text=Click me").await.unwrap());
        assert!(p.is_disabled("text=Click me").await.unwrap());
    });
}

async fn wait_state_after(p: &oxidewright::Page, content: &str, state: ElementState, change: &str) {
    p.set_content(content).await.unwrap();
    let span = p.query_selector("text=Target").await.unwrap().unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let task = tokio::spawn({
        let span = span.clone();
        let done = done.clone();
        async move {
            let result = span.wait_for_element_state(state).await;
            done.store(true, Ordering::SeqCst);
            result
        }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!done.load(Ordering::SeqCst));
    span.evaluate::<()>(change).await.unwrap();
    task.await.unwrap().unwrap();
}

#[test]
fn should_wait_for_enclosing_disabled_button() {
    run(|t| async move {
        wait_state_after(
            &t.page,
            "<button><span>Target</span></button>",
            ElementState::Disabled,
            "span => { span.parentElement.disabled = true; }",
        )
        .await;
    });
}

#[test]
fn should_wait_for_enclosing_button_with_a_disabled_fieldset() {
    run(|t| async move {
        wait_state_after(
            &t.page,
            "<fieldset disabled=true><button><span>Target</span></button></div>",
            ElementState::Enabled,
            "span => { span.parentElement.parentElement.disabled = false; }",
        )
        .await;
    });
}

#[test]
fn should_wait_for_enclosing_enabled_button() {
    run(|t| async move {
        wait_state_after(
            &t.page,
            "<button disabled><span>Target</span></button>",
            ElementState::Enabled,
            "span => { span.parentElement.disabled = false; }",
        )
        .await;
    });
}

#[test]
fn should_check_the_box_outside_shadow_dom_label() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<div></div>").await.unwrap();
        p.eval_on_selector::<()>(
            "div",
            r#"div => {
    const root = div.attachShadow({ mode: 'open' });
    const label = document.createElement('label');
    label.setAttribute('for', 'target');
    label.textContent = 'Click me';
    root.appendChild(label);
    const input = document.createElement('input');
    input.setAttribute('type', 'checkbox');
    input.setAttribute('id', 'target');
    root.appendChild(input);
  }"#,
        )
        .await
        .unwrap();
        p.check("label").await.unwrap();
        assert!(eval::<bool>(p, "input", "input => input.checked").await);
    });
}

#[test]
fn set_input_files_should_work_with_label() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<label for=target>Choose a file</label><input id=target type=file>")
            .await
            .unwrap();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("assets")
            .join("file-to-upload.txt");
        p.set_input_files("text=Choose a file", path).await.unwrap();
        assert_eq!(
            eval::<usize>(p, "input", "input => input.files.length").await,
            1
        );
        assert_eq!(
            eval::<String>(p, "input", "input => input.files[0].name").await,
            "file-to-upload.txt"
        );
    });
}

#[derive(Default, Clone, Copy)]
struct Opts {
    disabled: bool,
    hidden: bool,
    readonly: bool,
}

fn attrs(o: Opts) -> String {
    format!(
        " {} {} {} ",
        if o.disabled { "disabled" } else { "" },
        if o.hidden { "hidden" } else { "" },
        if o.readonly { "readonly" } else { "" }
    )
}

fn in_label(dom: &str, o: Opts) -> String {
    format!("<label {}>Text {dom}</label>", attrs(o))
}

fn label_for(dom: &str, o: Opts) -> String {
    format!(
        "<label {} for=\"target\"><h1>Text</h1></label>{dom}",
        attrs(o)
    )
}

fn standalone(dom: &str) -> String {
    dom.to_string()
}

fn in_button(dom: &str, o: Opts) -> String {
    format!("<button {}>Button {dom}</button>", attrs(o))
}

/// Same as `in_button`, like the original `domInLink`.
fn in_link(dom: &str, o: Opts) -> String {
    in_button(dom, o)
}

const N: Opts = Opts {
    disabled: false,
    hidden: false,
    readonly: false,
};
const DISABLED: Opts = Opts {
    disabled: true,
    hidden: false,
    readonly: false,
};
const HIDDEN: Opts = Opts {
    disabled: false,
    hidden: true,
    readonly: false,
};
const READONLY: Opts = Opts {
    disabled: false,
    hidden: false,
    readonly: true,
};

#[test]
fn enabled_disabled_retargeting() {
    run(|t| async move {
        let p = &t.page;
        let cases = vec![
            (in_label("<input id=target>", N), true, "label"),
            (label_for("<input id=target>", N), true, "label"),
            (standalone("<input id=target>"), true, "input"),
            (in_button("<input id=target>", N), true, "input"),
            (in_link("<input id=target>", N), true, "input"),
            (in_button("<input id=target>", DISABLED), true, "input"),
            (
                in_label("<li role=menuitem id=target aria-disabled=false></li>", N),
                true,
                "li",
            ),
            (in_label("<input id=target disabled>", N), false, "label"),
            (label_for("<input id=target disabled>", N), false, "label"),
            (standalone("<input id=target disabled>"), false, "input"),
            (in_button("<input id=target disabled>", N), false, "input"),
            (in_link("<input id=target disabled>", N), false, "input"),
            (
                in_button("<input id=target disabled>", DISABLED),
                false,
                "input",
            ),
            (
                in_label("<li role=menuitem id=target aria-disabled=true></li>", N),
                false,
                "li",
            ),
            // The disabled <option>/<optgroup> cases (issue 43634) need the
            // Playwright 1.65 injected script; the bundled one is 1.64.
        ];
        for (dom, enabled, locator) in cases {
            p.set_content(&dom).await.unwrap();
            let target = p.locator(locator);
            let handle = p.query_selector(locator).await.unwrap().unwrap();
            assert_eq!(
                target.is_enabled().await.unwrap(),
                enabled,
                "{locator} in {dom}"
            );
            assert_eq!(
                target.is_disabled().await.unwrap(),
                !enabled,
                "{locator} in {dom}"
            );
            expect_enabled(&target, enabled).await;
            let state = if enabled {
                ElementState::Enabled
            } else {
                ElementState::Disabled
            };
            handle
                .wait_for_element_state(state)
                .timeout(5000)
                .await
                .unwrap_or_else(|e| panic!("{locator} in {dom}: {e}"));
        }
    });
}

#[test]
fn visible_hidden_retargeting() {
    run(|t| async move {
        let p = &t.page;
        let cases = vec![
            (in_label("<span id=target>content</span>", N), true, "label"),
            (
                in_label("<span id=target hidden>content</span>", N),
                true,
                "label",
            ),
            (
                label_for("<span id=target>content</span>", N),
                true,
                "label",
            ),
            (
                label_for("<span id=target hidden>content</span>", N),
                true,
                "label",
            ),
            (standalone("<span id=target>content</span>"), true, "span"),
            (in_button("<span id=target>content</span>", N), true, "span"),
            (in_link("<span id=target>content</span>", N), true, "span"),
            (
                in_label("<span id=target>content</span>", HIDDEN),
                false,
                "label",
            ),
            (
                label_for("<span id=target>content</span>", HIDDEN),
                false,
                "label",
            ),
            (
                standalone("<span id=target hidden>content</span>"),
                false,
                "span",
            ),
            (
                in_button("<span id=target hidden>content</span>", N),
                false,
                "span",
            ),
            (
                in_button("<span id=target>content</span>", HIDDEN),
                false,
                "span",
            ),
            (
                in_link("<span id=target hidden>content</span>", N),
                false,
                "span",
            ),
            (
                in_link("<span id=target>content</span>", HIDDEN),
                false,
                "span",
            ),
        ];
        for (dom, visible, locator) in cases {
            p.set_content(&dom).await.unwrap();
            let target = p.locator(locator);
            let handle = p.query_selector(locator).await.unwrap().unwrap();
            assert_eq!(
                target.is_visible().await.unwrap(),
                visible,
                "{locator} in {dom}"
            );
            assert_eq!(
                target.is_hidden().await.unwrap(),
                !visible,
                "{locator} in {dom}"
            );
            expect_visible(&target, visible).await;
            let state = if visible {
                ElementState::Visible
            } else {
                ElementState::Hidden
            };
            handle
                .wait_for_element_state(state)
                .timeout(5000)
                .await
                .unwrap_or_else(|e| panic!("{locator} in {dom}: {e}"));
        }
    });
}

#[test]
fn editable_retargeting() {
    run(|t| async move {
        let p = &t.page;
        let cases = vec![
            (in_label("<input id=target>", N), true, "label"),
            (label_for("<input id=target>", N), true, "label"),
            (standalone("<input id=target>"), true, "input"),
            (in_button("<input id=target>", N), true, "input"),
            (in_link("<input id=target>", N), true, "input"),
            (in_button("<input id=target>", READONLY), true, "input"),
            (in_label("<input id=target readonly>", N), false, "label"),
            (label_for("<input id=target readonly>", N), false, "label"),
            (standalone("<input id=target readonly>"), false, "input"),
            (in_button("<input id=target readonly>", N), false, "input"),
            (in_link("<input id=target readonly>", N), false, "input"),
            (
                in_button("<input id=target readonly>", READONLY),
                false,
                "input",
            ),
        ];
        for (dom, editable, locator) in cases {
            p.set_content(&dom).await.unwrap();
            let target = p.locator(locator);
            let handle = p.query_selector(locator).await.unwrap().unwrap();
            assert_eq!(
                target.is_editable().await.unwrap(),
                editable,
                "{locator} in {dom}"
            );
            if editable {
                handle
                    .wait_for_element_state(ElementState::Editable)
                    .timeout(5000)
                    .await
                    .unwrap();
            }
        }
    });
}

async fn selection(p: &oxidewright::Page) -> String {
    p.evaluate("() => window.getSelection().toString()")
        .await
        .unwrap()
}

#[test]
fn input_value_retargeting() {
    run(|t| async move {
        let p = &t.page;
        let cases = vec![
            (in_label("<input id=target>", N), "label"),
            (label_for("<input id=target>", N), "label"),
            (standalone("<input id=target>"), "input"),
            (in_button("<input id=target>", N), "input"),
            (in_link("<input id=target>", N), "input"),
            (in_button("<input id=target>", N), "input"),
        ];
        for (dom, locator) in cases {
            p.set_content(&dom).await.unwrap();
            let target = p.locator(locator);
            let handle = p.query_selector(locator).await.unwrap().unwrap();
            assert_eq!(target.input_value().await.unwrap(), "");
            assert_eq!(handle.input_value().await.unwrap(), "");
            target.fill("foo").await.unwrap();
            assert_eq!(target.input_value().await.unwrap(), "foo");
            assert_eq!(handle.input_value().await.unwrap(), "foo");
            p.eval_on_selector::<()>("#target", "input => { input.value = 'bar'; }")
                .await
                .unwrap();
            assert_eq!(target.input_value().await.unwrap(), "bar");
            assert_eq!(handle.input_value().await.unwrap(), "bar");
            target.select_text().await.unwrap();
            assert_eq!(selection(p).await, "bar", "{locator} in {dom}");
        }
    });
}

#[test]
fn selection_retargeting() {
    run(|t| async move {
        let p = &t.page;
        let cases = vec![
            standalone("<div contenteditable id=target>content</div>"),
            in_button("<div contenteditable id=target>content</div>", N),
            in_link("<div contenteditable id=target>content</div>", N),
            in_button("<div contenteditable id=target>content</div>", N),
        ];
        for dom in cases {
            p.set_content(&dom).await.unwrap();
            let target = p.locator("div");
            let handle = p.query_selector("div").await.unwrap().unwrap();
            assert!(target.is_editable().await.unwrap());
            assert!(handle.is_editable().await.unwrap());
            expect_text(&p.locator("#target"), "content").await;
            target.fill("foo").await.unwrap();
            expect_text(&p.locator("#target"), "foo").await;
            target.select_text().await.unwrap();
            assert_eq!(selection(p).await, "foo", "{dom}");
        }
    });
}

#[test]
fn select_options_retargeting() {
    run(|t| async move {
        let p = &t.page;
        let select = "<select id=target multiple><option value=dog selected>Dog</option><option value=cat>Cat</option></select>";
        let cases = vec![
            (in_label(select, N), "label"),
            (label_for(select, N), "label"),
            (standalone(select), "select"),
            (in_button(select, N), "select"),
            (in_link(select, N), "select"),
            (in_button(select, N), "select"),
        ];
        let values = |p: &oxidewright::Page| {
            let p = p.clone();
            async move {
                p.eval_on_selector::<Vec<String>>(
                    "#target",
                    "s => [...s.selectedOptions].map(o => o.value)",
                )
                .await
                .unwrap()
            }
        };
        for (dom, locator) in cases {
            p.set_content(&dom).await.unwrap();
            let target = p.locator(locator);
            let handle = p.query_selector(locator).await.unwrap().unwrap();
            assert_eq!(target.input_value().await.unwrap(), "dog");
            assert_eq!(handle.input_value().await.unwrap(), "dog");
            assert_eq!(values(p).await, vec!["dog"]);
            target.select_option("cat").await.unwrap();
            assert_eq!(target.input_value().await.unwrap(), "cat");
            assert_eq!(handle.input_value().await.unwrap(), "cat");
            assert_eq!(values(p).await, vec!["cat"]);
        }
    });
}

#[test]
fn direct_actions_retargeting() {
    run(|t| async move {
        let p = &t.page;
        let cases = vec![
            in_label("<div>content</div><input id=target value=oh>", N),
            label_for("<div>content</div><input id=target value=oh>", N),
            standalone("<div>content</div>"),
            in_button("<div>content</div>", N),
            in_link("<div>content</div>", N),
            in_button("<div>content</div>", N),
        ];
        for dom in cases {
            p.set_content(&dom).await.unwrap();
            let target = p.locator("div");
            assert_eq!(target.inner_text().await.unwrap(), "content");
            assert_eq!(
                target.text_content().await.unwrap().as_deref(),
                Some("content")
            );
            expect_text(&target, "content").await;
            let focused: bool = target
                .evaluate("e => document.activeElement === e")
                .await
                .unwrap();
            assert!(!focused);
            expect_count(&target, 1).await;

            p.eval_on_selector::<()>("div", "div => { div.foo = 'bar'; }")
                .await
                .unwrap();
            let foo: String = target.evaluate("e => e.foo").await.unwrap();
            assert_eq!(foo, "bar");
            p.eval_on_selector::<()>("div", "div => div.classList.add('cls')")
                .await
                .unwrap();
            expect_attr(&target, "class", "cls").await;
            p.eval_on_selector::<()>("div", "div => { div.id = 'myid'; }")
                .await
                .unwrap();
            expect_attr(&target, "id", "myid").await;
            assert_eq!(
                target.get_attribute("id").await.unwrap().as_deref(),
                Some("myid")
            );
        }
    });
}

#[test]
fn check_retargeting() {
    run(|t| async move {
        let p = &t.page;
        let checkbox = "<input type=checkbox id=target>";
        let cases = vec![
            (in_label(checkbox, N), "label"),
            (label_for(checkbox, N), "label"),
            (standalone(checkbox), "input"),
            (in_button(checkbox, N), "input"),
            (in_link(checkbox, N), "input"),
            (in_button(checkbox, N), "input"),
        ];
        let checked = |p: &oxidewright::Page| {
            let p = p.clone();
            async move {
                p.eval_on_selector::<bool>("input", "input => input.checked")
                    .await
                    .unwrap()
            }
        };
        for (dom, locator) in cases {
            p.set_content(&dom).await.unwrap();
            let target = p.locator(locator);
            assert!(!target.is_checked().await.unwrap());
            p.eval_on_selector::<()>("input", "input => { input.checked = true; }")
                .await
                .unwrap();
            assert!(target.is_checked().await.unwrap());
            target.uncheck().await.unwrap();
            assert!(!checked(p).await, "{locator} in {dom}");
            target.check().await.unwrap();
            assert!(checked(p).await, "{locator} in {dom}");
            target.set_checked(false).await.unwrap();
            assert!(!checked(p).await, "{locator} in {dom}");
        }
    });
}

#[test]
fn should_not_retarget_anchor_into_parent_label() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            "<label disabled>Text<a href='#' onclick='window.__clicked=1'>Target</a></label>",
        )
        .await
        .unwrap();
        p.locator("a").click().await.unwrap();
        assert_eq!(p.evaluate::<i32>("window.__clicked").await.unwrap(), 1);

        p.set_content(
            r#"<input type="radio" id="input-id" checked disabled />
    <label for="input-id">Text<a href='#' onclick='window.__clicked=2'>Target</a></label>"#,
        )
        .await
        .unwrap();
        p.locator("a").click().await.unwrap();
        assert_eq!(p.evaluate::<i32>("window.__clicked").await.unwrap(), 2);
    });
}
