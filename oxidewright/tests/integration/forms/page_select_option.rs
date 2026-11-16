//! Port of `page-select-option.spec.ts`.
//!
//! Skipped:
//! - SO-020 `should not allow null items`, SO-024 `should throw if passed wrong types`:
//!   the Rust types make these values unrepresentable.
//! - SO-034 `should wait for select to become non-inert`: the bundled injected script
//!   (Patchright 1.64) has no `non-inert` element state.
//!
//! `select_option(null)` maps to [`SelectValues::none`].

use std::future::IntoFuture;
use std::time::Duration;

use oxidewright::{SelectOption, SelectValues};

use super::{give_it_a_chance, spawn};
use crate::common::*;

async fn goto_select(t: &TestContext) {
    t.page
        .goto(format!("{}/input/select.html", t.server.prefix))
        .await
        .unwrap();
}

async fn on_input(t: &TestContext) -> Vec<String> {
    t.page
        .evaluate("() => window['result'].onInput")
        .await
        .unwrap()
}

async fn on_change(t: &TestContext) -> Vec<String> {
    t.page
        .evaluate("() => window['result'].onChange")
        .await
        .unwrap()
}

async fn make_multiple(t: &TestContext) {
    t.page
        .evaluate::<()>("() => window['makeMultiple']()")
        .await
        .unwrap();
}

async fn none_selected(t: &TestContext) -> bool {
    t.page
        .eval_on_selector(
            "select",
            "select => Array.from(select.options).every(option => !option.selected)",
        )
        .await
        .unwrap()
}

#[test]
fn should_select_single_option() {
    run(|t| async move {
        goto_select(&t).await;
        t.page.select_option("select", "blue").await.unwrap();
        assert_eq!(on_input(&t).await, ["blue"]);
        assert_eq!(on_change(&t).await, ["blue"]);
    });
}

#[test]
fn should_select_single_option_by_value() {
    run(|t| async move {
        goto_select(&t).await;
        t.page
            .select_option("select", SelectOption::value("blue"))
            .await
            .unwrap();
        assert_eq!(on_input(&t).await, ["blue"]);
        assert_eq!(on_change(&t).await, ["blue"]);
    });
}

#[test]
fn should_fall_back_to_selecting_by_label() {
    run(|t| async move {
        goto_select(&t).await;
        t.page.select_option("select", "Blue").await.unwrap();
        assert_eq!(on_input(&t).await, ["blue"]);
        assert_eq!(on_change(&t).await, ["blue"]);
    });
}

#[test]
fn should_select_single_option_by_label() {
    run(|t| async move {
        goto_select(&t).await;
        t.page
            .select_option("select", SelectOption::label("Indigo"))
            .await
            .unwrap();
        assert_eq!(on_input(&t).await, ["indigo"]);
        assert_eq!(on_change(&t).await, ["indigo"]);
    });
}

#[test]
fn should_select_single_option_by_label_with_html_whitespace() {
    run(|t| async move {
        goto_select(&t).await;
        t.page
            .select_option("select", SelectOption::label("HTML"))
            .await
            .unwrap();
        assert_eq!(on_input(&t).await, ["html"]);
        assert_eq!(on_change(&t).await, ["html"]);
    });
}

#[test]
fn should_select_single_option_by_handle() {
    run(|t| async move {
        goto_select(&t).await;
        let option = t
            .page
            .query_selector("[id=whiteOption]")
            .await
            .unwrap()
            .unwrap();
        t.page.select_option("select", option).await.unwrap();
        assert_eq!(on_input(&t).await, ["white"]);
        assert_eq!(on_change(&t).await, ["white"]);
    });
}

#[test]
fn should_select_single_option_by_index() {
    run(|t| async move {
        goto_select(&t).await;
        t.page
            .select_option("select", SelectOption::index(2))
            .await
            .unwrap();
        assert_eq!(on_input(&t).await, ["brown"]);
        assert_eq!(on_change(&t).await, ["brown"]);
    });
}

#[test]
fn should_select_single_option_by_multiple_attributes() {
    run(|t| async move {
        goto_select(&t).await;
        let option = SelectOption {
            value: Some("green".into()),
            label: Some("Green".into()),
            index: None,
        };
        t.page.select_option("select", option).await.unwrap();
        assert_eq!(on_input(&t).await, ["green"]);
        assert_eq!(on_change(&t).await, ["green"]);
    });
}

#[test]
fn should_not_select_single_option_when_some_attributes_do_not_match() {
    run(|t| async move {
        goto_select(&t).await;
        t.page
            .eval_on_selector::<()>("select", "s => { s.value = undefined; }")
            .await
            .unwrap();
        let option = SelectOption {
            value: Some("green".into()),
            label: Some("Brown".into()),
            index: None,
        };
        let error = t
            .page
            .select_option("select", option)
            .timeout(1000)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Timeout"), "{error}");
        let value: String = t
            .page
            .evaluate("() => document.querySelector('select').value")
            .await
            .unwrap();
        assert_eq!(value, "");
    });
}

#[test]
fn should_select_only_first_option() {
    run(|t| async move {
        goto_select(&t).await;
        t.page
            .select_option("select", ["blue", "green", "red"])
            .await
            .unwrap();
        assert_eq!(on_input(&t).await, ["blue"]);
        assert_eq!(on_change(&t).await, ["blue"]);
    });
}

#[test]
fn should_not_throw_when_select_causes_navigation() {
    run(|t| async move {
        goto_select(&t).await;
        t.page
            .eval_on_selector::<()>(
                "select",
                "select => select.addEventListener('input', () => window.location.href = '/empty.html')",
            )
            .await
            .unwrap();
        let navigation = t.page.wait_for_navigation();
        let (select, nav) = tokio::join!(
            t.page.select_option("select", "blue").into_future(),
            navigation.into_future()
        );
        select.unwrap();
        nav.unwrap();
        assert!(t.page.url().contains("empty.html"));
    });
}

#[test]
fn should_select_multiple_options() {
    run(|t| async move {
        goto_select(&t).await;
        make_multiple(&t).await;
        t.page
            .select_option("select", ["blue", "green", "red"])
            .await
            .unwrap();
        assert_eq!(on_input(&t).await, ["blue", "green", "red"]);
        assert_eq!(on_change(&t).await, ["blue", "green", "red"]);
    });
}

#[test]
fn should_select_multiple_options_with_attributes() {
    run(|t| async move {
        goto_select(&t).await;
        make_multiple(&t).await;
        t.page
            .select_option(
                "select",
                vec![
                    SelectOption::value("blue"),
                    SelectOption::label("Green"),
                    SelectOption::index(4),
                ],
            )
            .await
            .unwrap();
        assert_eq!(on_input(&t).await, ["blue", "gray", "green"]);
        assert_eq!(on_change(&t).await, ["blue", "gray", "green"]);
    });
}

#[test]
fn should_respect_event_bubbling() {
    run(|t| async move {
        goto_select(&t).await;
        t.page.select_option("select", "blue").await.unwrap();
        let input: Vec<String> = t
            .page
            .evaluate("() => window['result'].onBubblingInput")
            .await
            .unwrap();
        let change: Vec<String> = t
            .page
            .evaluate("() => window['result'].onBubblingChange")
            .await
            .unwrap();
        assert_eq!(input, ["blue"]);
        assert_eq!(change, ["blue"]);
    });
}

#[test]
fn should_throw_when_element_is_not_a_select() {
    run(|t| async move {
        goto_select(&t).await;
        let error = t.page.select_option("body", "").await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Element is not a <select> element"),
            "{error}"
        );
    });
}

#[test]
fn should_return_empty_on_no_matched_values() {
    run(|t| async move {
        goto_select(&t).await;
        let result = t
            .page
            .select_option("select", SelectValues::none())
            .await
            .unwrap();
        assert!(result.is_empty());
    });
}

#[test]
fn should_return_an_array_of_matched_values() {
    run(|t| async move {
        goto_select(&t).await;
        make_multiple(&t).await;
        let result = t
            .page
            .select_option("select", ["blue", "black", "magenta"])
            .await
            .unwrap();
        assert!(
            result
                .iter()
                .all(|v| ["blue", "black", "magenta"].contains(&v.as_str()))
        );
    });
}

#[test]
fn should_return_an_array_of_one_element_when_multiple_is_not_set() {
    run(|t| async move {
        goto_select(&t).await;
        let result = t
            .page
            .select_option("select", ["42", "blue", "black", "magenta"])
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
    });
}

#[test]
fn should_return_empty_on_no_values() {
    run(|t| async move {
        goto_select(&t).await;
        let result = t
            .page
            .select_option("select", Vec::<&str>::new())
            .await
            .unwrap();
        assert!(result.is_empty());
    });
}

#[test]
fn should_unselect_with_null() {
    run(|t| async move {
        goto_select(&t).await;
        make_multiple(&t).await;
        let result = t
            .page
            .select_option("select", ["blue", "black", "magenta"])
            .await
            .unwrap();
        assert!(
            result
                .iter()
                .all(|v| ["blue", "black", "magenta"].contains(&v.as_str()))
        );
        t.page
            .select_option("select", SelectValues::none())
            .await
            .unwrap();
        assert!(none_selected(&t).await);
    });
}

#[test]
fn should_deselect_all_options_when_passed_no_values_for_a_multiple_select() {
    run(|t| async move {
        goto_select(&t).await;
        make_multiple(&t).await;
        t.page
            .select_option("select", ["blue", "black", "magenta"])
            .await
            .unwrap();
        t.page
            .select_option("select", SelectValues::none())
            .await
            .unwrap();
        assert!(none_selected(&t).await);
    });
}

#[test]
fn should_deselect_all_options_when_passed_no_values_for_a_select_without_multiple() {
    run(|t| async move {
        goto_select(&t).await;
        t.page
            .select_option("select", ["blue", "black", "magenta"])
            .await
            .unwrap();
        t.page
            .select_option("select", SelectValues::none())
            .await
            .unwrap();
        assert!(none_selected(&t).await);
    });
}

#[test]
fn should_work_when_re_defining_top_level_event_class() {
    run(|t| async move {
        goto_select(&t).await;
        t.page
            .evaluate::<()>("() => { window.Event = null; }")
            .await
            .unwrap();
        t.page.select_option("select", "blue").await.unwrap();
        assert_eq!(on_input(&t).await, ["blue"]);
        assert_eq!(on_change(&t).await, ["blue"]);
    });
}

const ADD_SCARLET: &str = "select => { const option = document.createElement('option'); option.value = 'scarlet'; option.textContent = 'Scarlet'; select.appendChild(option); }";

#[test]
fn should_wait_for_option_to_be_present() {
    run(|t| async move {
        goto_select(&t).await;
        let select = spawn(t.page.select_option("select", "scarlet"));
        give_it_a_chance().await;
        assert!(!select.is_finished());
        t.page
            .eval_on_selector::<()>("select", ADD_SCARLET)
            .await
            .unwrap();
        assert_eq!(select.await.unwrap().unwrap(), ["scarlet"]);
    });
}

#[test]
fn should_wait_for_option_index_to_be_present() {
    run(|t| async move {
        goto_select(&t).await;
        let len: usize = t
            .page
            .eval_on_selector("select", "select => select.options.length")
            .await
            .unwrap();
        let select = spawn(t.page.select_option("select", SelectOption::index(len)));
        give_it_a_chance().await;
        assert!(!select.is_finished());
        t.page
            .eval_on_selector::<()>("select", ADD_SCARLET)
            .await
            .unwrap();
        assert_eq!(select.await.unwrap().unwrap(), ["scarlet"]);
    });
}

#[test]
fn should_wait_for_multiple_options_to_be_present() {
    run(|t| async move {
        goto_select(&t).await;
        make_multiple(&t).await;
        let select = spawn(t.page.select_option("select", ["green", "scarlet"]));
        give_it_a_chance().await;
        assert!(!select.is_finished());
        t.page
            .eval_on_selector::<()>("select", ADD_SCARLET)
            .await
            .unwrap();
        assert_eq!(select.await.unwrap().unwrap(), ["green", "scarlet"]);
    });
}

#[test]
fn input_event_composed_should_be_true_and_cross_shadow_dom_boundary() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .set_content(
                "<body><script>
  const div = document.createElement('div');
  const shadowRoot = div.attachShadow({mode: 'open'});
  shadowRoot.innerHTML = `<select>
    <option value=\"black\">Black</option>
    <option value=\"blue\">Blue</option>
  </select>`;
  document.body.appendChild(div);
</script></body>",
            )
            .await
            .unwrap();
        t.page
            .locator("body")
            .evaluate::<()>(
                "select => { window.firedBodyEvents = []; for (const event of ['input', 'change']) select.addEventListener(event, e => window.firedBodyEvents.push(e.type + ':' + e.composed), false); }",
            )
            .await
            .unwrap();
        t.page
            .locator("select")
            .evaluate::<()>(
                "select => { window.firedEvents = []; for (const event of ['input', 'change']) select.addEventListener(event, e => window.firedEvents.push(e.type + ':' + e.composed), false); }",
            )
            .await
            .unwrap();
        t.page.select_option("select", "blue").await.unwrap();
        let fired: Vec<String> = t
            .page
            .evaluate("() => window['firedEvents']")
            .await
            .unwrap();
        assert_eq!(fired, ["input:true", "change:false"]);
        let body: Vec<String> = t
            .page
            .evaluate("() => window['firedBodyEvents']")
            .await
            .unwrap();
        assert_eq!(body, ["input:true"]);
    });
}

async fn wait_and_hydrate(t: &TestContext) {
    let select = spawn(t.page.locator("select").select_option("two"));
    tokio::time::sleep(Duration::from_millis(1000)).await;
    t.page
        .evaluate::<()>("() => window.hydrate()")
        .await
        .unwrap();
    select.await.unwrap().unwrap();
    let result: String = t.page.evaluate("() => window['result']").await.unwrap();
    assert_eq!(result, "two");
    assert_eq!(t.page.locator("select").input_value().await.unwrap(), "two");
}

#[test]
fn should_wait_for_select_to_be_enabled() {
    run(|t| async move {
        t.page
            .set_content(
                "<select disabled>
      <option>one</option>
      <option>two</option>
    </select>
    <script>
    function hydrate() {
      const select = document.querySelector('select');
      select.removeAttribute('disabled');
      select.addEventListener('change', () => {
        window['result'] = select.value;
      });
    }
    </script>",
            )
            .await
            .unwrap();
        wait_and_hydrate(&t).await;
    });
}

#[test]
// Upstream uses 1000ms; more headroom for loaded CI runners.
fn should_wait_for_option_to_be_enabled() {
    run(|t| async move {
        t.page
            .set_content(
                "<select>
      <option>one</option>
      <option disabled id=myoption>two</option>
    </select>
    <script>
    function hydrate() {
      const option = document.querySelector('#myoption');
      option.removeAttribute('disabled');
      const select = document.querySelector('select');
      select.addEventListener('change', () => {
        window['result'] = select.value;
      });
    }
    </script>",
            )
            .await
            .unwrap();
        let error = t
            .page
            .locator("select")
            .select_option("two")
            .timeout(3000)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("option being selected is not enabled"),
            "{error}"
        );
        wait_and_hydrate(&t).await;
    });
}

#[test]
fn should_wait_for_optgroup_to_be_enabled() {
    run(|t| async move {
        t.page
            .set_content(
                "<select>
      <option>one</option>
      <optgroup label=\"Group\" disabled id=mygroup>
        <option>two</option>
      </optgroup>
    </select>
    <script>
    function hydrate() {
      const group = document.querySelector('#mygroup');
      group.removeAttribute('disabled');
      const select = document.querySelector('select');
      select.addEventListener('change', () => {
        window['result'] = select.value;
      });
    }
    </script>",
            )
            .await
            .unwrap();
        let error = t
            .page
            .locator("select")
            .select_option("two")
            .timeout(3000)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("option being selected is not enabled"),
            "{error}"
        );
        wait_and_hydrate(&t).await;
    });
}

#[test]
fn should_wait_for_select_to_be_swapped() {
    run(|t| async move {
        t.page
            .set_content(
                "<select disabled>
      <option>one</option>
      <option>two</option>
    </select>
    <script>
    function hydrate() {
      const select = document.querySelector('select');
      select.remove();
      const newSelect = document.createElement('select');
      const option1 = document.createElement('option');
      option1.textContent = 'one';
      newSelect.appendChild(option1);
      const option2 = document.createElement('option');
      option2.textContent = 'two';
      newSelect.appendChild(option2);
      document.body.appendChild(newSelect);
      newSelect.addEventListener('change', () => {
        window['result'] = newSelect.value;
      });
    }
    </script>",
            )
            .await
            .unwrap();
        wait_and_hydrate(&t).await;
    });
}
