//! Port of `selectors-role.spec.ts`.
//!
//! Partially skipped assertions:
//! - SE-052: the `toMatchAriaSnapshot` and `toBeEditable` assertions (no
//!   `expect` matchers; `is_editable` is checked instead).
//! - SE-052: the case-insensitive ARIA attribute part (issue 43662) and
//!   SE-060: the `ariaLabelledByElements` part (issue 43549) are fixed in
//!   Playwright 1.65's injected script; oxidewright bundles Patchright's
//!   1.64 build.
//! - SE-060: `toHaveAccessibleName` (no `expect` matchers).

use oxidewright::{GetByRoleOptions, regex};

use super::*;
use crate::common::*;

fn opts() -> GetByRoleOptions {
    GetByRoleOptions::new()
}

#[test]
fn should_detect_roles() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <button>Hello</button>
    <select multiple="" size="2"></select>
    <select></select>
    <h3>Heading</h3>
    <details><summary>Hello</summary></details>
    <div role="dialog">I am a dialog</div>
  "#,
        )
        .await
        .unwrap();
        assert_eq!(
            outer_htmls(&p.locator("role=button")).await,
            vec!["<button>Hello</button>"]
        );
        assert_eq!(
            outer_htmls(&p.locator("role=listbox")).await,
            vec![r#"<select multiple="" size="2"></select>"#]
        );
        assert_eq!(
            outer_htmls(&p.locator("role=combobox")).await,
            vec!["<select></select>"]
        );
        assert_eq!(
            outer_htmls(&p.locator("role=heading")).await,
            vec!["<h3>Heading</h3>"]
        );
        assert_eq!(
            outer_htmls(&p.locator("role=group")).await,
            vec!["<details><summary>Hello</summary></details>"]
        );
        assert_eq!(
            outer_htmls(&p.locator("role=dialog")).await,
            vec![r#"<div role="dialog">I am a dialog</div>"#]
        );
        assert!(outer_htmls(&p.locator("role=menuitem")).await.is_empty());
        assert!(outer_htmls(&p.get_by_role("menuitem")).await.is_empty());
    });
}

#[test]
fn should_support_selected() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <select>
      <option>Hi</option>
      <option selected>Hello</option>
    </select>
    <div>
      <div role="option" aria-selected="true">Hi</div>
      <div role="option" aria-selected="false">Hello</div>
    </div>
  "#,
        )
        .await
        .unwrap();
        let selected = vec![
            r#"<option selected="">Hello</option>"#,
            r#"<div role="option" aria-selected="true">Hi</div>"#,
        ];
        let not_selected = vec![
            "<option>Hi</option>",
            r#"<div role="option" aria-selected="false">Hello</div>"#,
        ];
        assert_eq!(
            outer_htmls(&p.locator("role=option[selected]")).await,
            selected
        );
        assert_eq!(
            outer_htmls(&p.locator("role=option[selected=true]")).await,
            selected
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("option", opts().selected(true))).await,
            selected
        );
        assert_eq!(
            outer_htmls(&p.locator("role=option[selected=false]")).await,
            not_selected
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("option", opts().selected(false))).await,
            not_selected
        );
    });
}

#[test]
fn should_support_checked() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <input type=checkbox>
    <input type=checkbox checked>
    <input type=checkbox indeterminate>
    <div role=checkbox aria-checked="true">Hi</div>
    <div role=checkbox aria-checked="false">Hello</div>
    <div role=checkbox>Unknown</div>
  "#,
        )
        .await
        .unwrap();
        p.eval_on_selector::<()>("[indeterminate]", "input => input.indeterminate = true")
            .await
            .unwrap();
        let checked = vec![
            r#"<input type="checkbox" checked="">"#,
            r#"<div role="checkbox" aria-checked="true">Hi</div>"#,
        ];
        let unchecked = vec![
            r#"<input type="checkbox">"#,
            r#"<div role="checkbox" aria-checked="false">Hello</div>"#,
            r#"<div role="checkbox">Unknown</div>"#,
        ];
        assert_eq!(
            outer_htmls(&p.locator("role=checkbox[checked]")).await,
            checked
        );
        assert_eq!(
            outer_htmls(&p.locator("role=checkbox[checked=true]")).await,
            checked
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("checkbox", opts().checked(true))).await,
            checked
        );
        assert_eq!(
            outer_htmls(&p.locator("role=checkbox[checked=false]")).await,
            unchecked
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("checkbox", opts().checked(false))).await,
            unchecked
        );
        assert_eq!(
            outer_htmls(&p.locator(r#"role=checkbox[checked="mixed"]"#)).await,
            vec![r#"<input type="checkbox" indeterminate="">"#]
        );
        assert_eq!(outer_htmls(&p.locator("role=checkbox")).await.len(), 6);

        // The case-insensitive ARIA values part (issue 43662) needs the
        // Playwright 1.65 injected script; the bundled one is 1.64.
    });
}

#[test]
fn should_support_pressed() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <button>Hi</button>
    <button aria-pressed="true">Hello</button>
    <button aria-pressed="false">Bye</button>
    <button aria-pressed="mixed">Mixed</button>
  "#,
        )
        .await
        .unwrap();
        let pressed = vec![r#"<button aria-pressed="true">Hello</button>"#];
        let not_pressed = vec![
            "<button>Hi</button>",
            r#"<button aria-pressed="false">Bye</button>"#,
        ];
        assert_eq!(
            outer_htmls(&p.locator("role=button[pressed]")).await,
            pressed
        );
        assert_eq!(
            outer_htmls(&p.locator("role=button[pressed=true]")).await,
            pressed
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("button", opts().pressed(true))).await,
            pressed
        );
        assert_eq!(
            outer_htmls(&p.locator("role=button[pressed=false]")).await,
            not_pressed
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("button", opts().pressed(false))).await,
            not_pressed
        );
        assert_eq!(
            outer_htmls(&p.locator(r#"role=button[pressed="mixed"]"#)).await,
            vec![r#"<button aria-pressed="mixed">Mixed</button>"#]
        );
        assert_eq!(outer_htmls(&p.locator("role=button")).await.len(), 4);
    });
}

#[test]
fn should_support_expanded() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <div role="treeitem">Hi</div>
    <div role="treeitem" aria-expanded="true">Hello</div>
    <div role="treeitem" aria-expanded="false">Bye</div>
  "#,
        )
        .await
        .unwrap();
        let all = vec![
            r#"<div role="treeitem">Hi</div>"#,
            r#"<div role="treeitem" aria-expanded="true">Hello</div>"#,
            r#"<div role="treeitem" aria-expanded="false">Bye</div>"#,
        ];
        assert_eq!(outer_htmls(&p.locator("role=treeitem")).await, all);
        assert_eq!(outer_htmls(&p.get_by_role("treeitem")).await, all);
        assert_eq!(
            outer_htmls(&p.locator("role=treeitem[expanded]")).await,
            vec![all[1]]
        );
        assert_eq!(
            outer_htmls(&p.locator("role=treeitem[expanded=true]")).await,
            vec![all[1]]
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("treeitem", opts().expanded(true))).await,
            vec![all[1]]
        );
        assert_eq!(
            outer_htmls(&p.locator("role=treeitem[expanded=false]")).await,
            vec![all[2]]
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("treeitem", opts().expanded(false))).await,
            vec![all[2]]
        );
        assert_eq!(
            outer_htmls(&p.locator("[role=treeitem]:not([aria-expanded])")).await,
            vec![all[0]]
        );
    });
}

#[test]
fn should_support_disabled() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <button>Hi</button>
    <button disabled>Bye</button>
    <button aria-disabled="true">Hello</button>
    <button aria-disabled="false">Oh</button>
    <fieldset disabled>
      <button>Yay</button>
    </fieldset>
    <select>
      <optgroup disabled>
        <option>one</option>
      </optgroup>
      <optgroup>
        <option>two</option>
      </optgroup>
      <option disabled>three</option>
    </select>
  "#,
        )
        .await
        .unwrap();
        let disabled = vec![
            r#"<button disabled="">Bye</button>"#,
            r#"<button aria-disabled="true">Hello</button>"#,
            "<button>Yay</button>",
        ];
        let enabled = vec![
            "<button>Hi</button>",
            r#"<button aria-disabled="false">Oh</button>"#,
        ];
        assert_eq!(
            outer_htmls(&p.locator("role=button[disabled]")).await,
            disabled
        );
        assert_eq!(
            outer_htmls(&p.locator("role=button[disabled=true]")).await,
            disabled
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("button", opts().disabled(true))).await,
            disabled
        );
        assert_eq!(
            outer_htmls(&p.locator("role=button[disabled=false]")).await,
            enabled
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("button", opts().disabled(false))).await,
            enabled
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("option", opts().disabled(true))).await,
            vec![
                "<option>one</option>",
                r#"<option disabled="">three</option>"#
            ]
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("option", opts().disabled(false))).await,
            vec!["<option>two</option>"]
        );
        assert_eq!(outer_htmls(&p.get_by_role("option")).await.len(), 3);
    });
}

#[test]
fn should_inherit_disabled_from_the_ancestor() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(r#"<span aria-disabled="true"><button>Click me!</button></span>"#)
            .await
            .unwrap();
        expect_enabled(&p.locator("button"), false).await;
        p.set_content(r#"<span aria-disabled="true"><h1>Heading</h1></span>"#)
            .await
            .unwrap();
        assert!(!p.locator("h1").is_disabled().await.unwrap());
    });
}

#[test]
fn should_support_disabled_fieldset() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <fieldset disabled>
      <input></input>
      <button data-testid="inside-fieldset-element">x</button>
      <legend>
        <button data-testid="inside-legend-element">legend</button>
      </legend>
    </fieldset>

    <fieldset disabled>
      <legend>
        <div>
          <button data-testid="nested-inside-legend-element">x</button>
        </div>
      </legend>
    </fieldset>

    <fieldset disabled>
      <div></div>
      <legend>
        <button data-testid="first-legend-element">x</button>
      </legend>
      <legend>
        <button data-testid="second-legend-element">x</button>
      </legend>
    </fieldset>

    <fieldset disabled>
      <fieldset>
        <button data-testid="deep-button">x</button>
      </fieldset>
    </fieldset>
  "#,
        )
        .await
        .unwrap();
        // CSS selectors instead of getByTestId: the test id attribute is a
        // global setting other tests may change.
        let by = |id: &str| p.locator(format!("[data-testid={id}]"));
        expect_enabled(&by("inside-legend-element"), true).await;
        expect_enabled(&by("nested-inside-legend-element"), true).await;
        expect_enabled(&by("first-legend-element"), true).await;
        expect_enabled(&by("second-legend-element"), false).await;
        expect_enabled(&by("deep-button"), false).await;
    });
}

#[test]
fn should_support_level() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <h1>Hello</h1>
    <h3>Hi</h3>
    <div role="heading" aria-level="5">Bye</div>
  "#,
        )
        .await
        .unwrap();
        assert_eq!(
            outer_htmls(&p.locator("role=heading[level=1]")).await,
            vec!["<h1>Hello</h1>"]
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("heading", opts().level(1))).await,
            vec!["<h1>Hello</h1>"]
        );
        assert_eq!(
            outer_htmls(&p.locator("role=heading[level=3]")).await,
            vec!["<h3>Hi</h3>"]
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("heading", opts().level(3))).await,
            vec!["<h3>Hi</h3>"]
        );
        assert_eq!(
            outer_htmls(&p.locator("role=heading[level=5]")).await,
            vec![r#"<div role="heading" aria-level="5">Bye</div>"#]
        );
    });
}

#[test]
fn should_filter_hidden_unless_explicitly_asked_for() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <button>Hi</button>
    <button hidden>Hello</button>
    <button aria-hidden="true">Yay</button>
    <button aria-hidden="false">Nay</button>
    <button style="visibility:hidden">Bye</button>
    <div style="visibility:hidden">
      <button>Oh</button>
    </div>
    <div style="visibility:hidden">
      <button style="visibility:visible">Still here</button>
    </div>
    <button style="display:none">Never</button>
    <div id=host1></div>
    <div id=host2 style="display:none"></div>

    <input name="one">
    <details>
      <summary>Open form</summary>
      <label>
         Label
         <input name="two">
      </label>
    </details>

    <select>
      <option style="visibility:hidden">One</option>
      <option style="display:none">Two</option>
      <option>Three</option>
    </select>

    <script>
      function addButton(host, text) {
        const root = host.attachShadow({ mode: 'open' });
        const button = document.createElement('button');
        button.textContent = text;
        root.appendChild(button);
      }
      addButton(document.getElementById('host1'), 'Shadow1');
      addButton(document.getElementById('host2'), 'Shadow2');
    </script>
  "#,
        )
        .await
        .unwrap();
        let visible = vec![
            "<button>Hi</button>",
            r#"<button aria-hidden="false">Nay</button>"#,
            r#"<button style="visibility:visible">Still here</button>"#,
            "<button>Shadow1</button>",
        ];
        let all = vec![
            "<button>Hi</button>",
            r#"<button hidden="">Hello</button>"#,
            r#"<button aria-hidden="true">Yay</button>"#,
            r#"<button aria-hidden="false">Nay</button>"#,
            r#"<button style="visibility:hidden">Bye</button>"#,
            "<button>Oh</button>",
            r#"<button style="visibility:visible">Still here</button>"#,
            r#"<button style="display:none">Never</button>"#,
            "<button>Shadow1</button>",
            "<button>Shadow2</button>",
        ];
        assert_eq!(outer_htmls(&p.locator("role=button")).await, visible);
        assert_eq!(
            outer_htmls(&p.locator("role=button[include-hidden]")).await,
            all
        );
        assert_eq!(
            outer_htmls(&p.locator("role=button[include-hidden=true]")).await,
            all
        );
        assert_eq!(
            outer_htmls(&p.locator("role=button[include-hidden=false]")).await,
            visible
        );
        assert_eq!(
            outer_htmls(&p.locator("role=textbox")).await,
            vec![r#"<input name="one">"#]
        );
        assert_eq!(
            outer_htmls(&p.locator("role=option")).await,
            vec![
                r#"<option style="visibility:hidden">One</option>"#,
                "<option>Three</option>"
            ]
        );
    });
}

#[test]
fn should_support_name() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <div role="button" aria-label=" Hello "></div>
    <div role="button" aria-label="Hallo"></div>
    <div role="button" aria-label="Hello" aria-hidden="true"></div>
    <div role="button" aria-label="123" aria-hidden="true"></div>
    <div role="button" aria-label='foo"bar' aria-hidden="true"></div>

    <span id="title">Chosen name</span>
    <div role="group" aria-labelledby="title"></div>
  "#,
        )
        .await
        .unwrap();
        let hello = r#"<div role="button" aria-label=" Hello "></div>"#;
        let hallo = r#"<div role="button" aria-label="Hallo"></div>"#;
        let hidden_hello = r#"<div role="button" aria-label="Hello" aria-hidden="true"></div>"#;
        let hidden_123 = r#"<div role="button" aria-label="123" aria-hidden="true"></div>"#;
        assert_eq!(
            outer_htmls(&p.locator(r#"role=button[name="Hello"]"#)).await,
            vec![hello]
        );
        assert_eq!(
            outer_htmls(&p.locator("role=button[name=\" \n Hello \"]")).await,
            vec![hello]
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("button", opts().name("Hello"))).await,
            vec![hello]
        );
        assert_eq!(
            outer_htmls(&p.locator(r#"role=button[name*="all"]"#)).await,
            vec![hallo]
        );
        assert_eq!(
            outer_htmls(&p.locator("role=button[name=/^H[ae]llo$/]")).await,
            vec![hello, hallo]
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("button", opts().name(regex("^H[ae]llo$", "")))).await,
            vec![hello, hallo]
        );
        assert_eq!(
            outer_htmls(&p.locator("role=button[name=/h.*o/i]")).await,
            vec![hello, hallo]
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("button", opts().name(regex("h.*o", "i")))).await,
            vec![hello, hallo]
        );
        assert_eq!(
            outer_htmls(&p.locator(r#"role=button[name="Hello"][include-hidden]"#)).await,
            vec![hello, hidden_hello]
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("button", opts().name("Hello").include_hidden(true)))
                .await,
            vec![hello, hidden_hello]
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("button", opts().name("hello").include_hidden(true)))
                .await,
            vec![hello, hidden_hello]
        );
        assert_eq!(
            outer_htmls(&p.locator("role=button[name=Hello]")).await,
            vec![hello]
        );
        assert_eq!(
            outer_htmls(&p.locator("role=button[name=123][include-hidden]")).await,
            vec![hidden_123]
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("button", opts().name("123").include_hidden(true)))
                .await,
            vec![hidden_123]
        );

        // `ariaLabelledByElements` (issue 43549) needs the Playwright 1.65
        // injected script; the bundled one is 1.64.
    });
}

#[test]
fn should_support_option_name_with_html_whitespace() {
    run(|t| async move {
        t.page
            .set_content(r#"<select><option value="html">&nbsp;HTML</option></select>"#)
            .await
            .unwrap();
        expect_count(&t.page.get_by_role_with("option", opts().name("HTML")), 1).await;
    });
}

#[test]
fn errors() {
    run(|t| async move {
        let p = &t.page;
        let cases = [
            ("role=[bar]", "Role must not be empty"),
            (
                "role=foo[sElected]",
                r#"Unknown attribute "sElected", must be one of "checked", "description", "disabled", "expanded", "include-hidden", "level", "name", "pressed", "selected""#,
            ),
            ("role=foo[bar . qux=true]", r#"Unknown attribute "bar.qux""#),
            (
                r#"role=heading[level="bar"]"#,
                r#""level" attribute must be compared to a number"#,
            ),
            (
                r#"role=checkbox[checked="bar"]"#,
                r#""checked" must be one of true, false, "mixed""#,
            ),
            (
                "role=checkbox[checked~=true]",
                "cannot use ~= in attribute with non-string matching value",
            ),
            (
                "role=button[level=3]",
                r#""level" attribute is only supported for roles: "heading", "listitem", "row", "treeitem""#,
            ),
            ("role=button[name]", r#""name" attribute must have a value"#),
            (
                r#"role=treeitem[expanded="none"]"#,
                r#""expanded" must be one of true, false"#,
            ),
        ];
        for (selector, message) in cases {
            let err = p.query_selector(selector).await.unwrap_err().to_string();
            assert!(err.contains(message), "{selector}: {err}");
        }
    });
}

#[test]
fn hidden_with_shadow_dom_slots() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <div make-hidden>
      <button>hidden1</button>
    </div>
    <div make-hidden>
      <span><button>hidden2</button></v>
    </div>
    <div>
      <button>visible1</button>
    </div>
    <div>
      <span><button>visible2</button></span>
    </div>
    <script>
      for (const div of document.querySelectorAll('div')) {
        const hidden = div.hasAttribute('make-hidden');
        div.attachShadow({ mode: 'open' }).innerHTML = hidden ? 'nothing to see here' : '<slot></slot>';
      }
    </script>
  "#,
        )
        .await
        .unwrap();
        assert_eq!(
            outer_htmls(&p.locator("role=button")).await,
            vec!["<button>visible1</button>", "<button>visible2</button>"]
        );
        assert_eq!(
            outer_htmls(&p.locator("role=button[include-hidden]")).await,
            vec![
                "<button>hidden1</button>",
                "<button>hidden2</button>",
                "<button>visible1</button>",
                "<button>visible2</button>"
            ]
        );
    });
}

#[test]
fn should_support_output_accessible_name() {
    run(|t| async move {
        t.page
            .set_content("<label>Output1<output>output</output></label>")
            .await
            .unwrap();
        expect_visible(
            &t.page.get_by_role_with("status", opts().name("Output1")),
            true,
        )
        .await;
    });
}

#[test]
fn should_not_match_scope_by_default() {
    run(|t| async move {
        t.page
            .set_content(
                r#"
    <ul>
      <li aria-label="Parent list">
        Parent list
        <ul>
          <li>child 1</li>
          <li>child 2</li>
        </ul>
      </li>
    </ul>
  "#,
            )
            .await
            .unwrap();
        let children = t
            .page
            .get_by_role_with("listitem", opts().name("Parent list"))
            .get_by_role("listitem");
        expect_count(&children, 2).await;
        expect_texts(&children, &["child 1", "child 2"]).await;
    });
}
