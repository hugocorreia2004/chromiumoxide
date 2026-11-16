//! Port of `elementhandle-convenience.spec.ts` (all tests ported).

use std::time::Duration;

use oxidewright::JSHandle;

use crate::common::*;

/// `expect.poll(() => String(handle)).toBe(expected)`.
async fn poll_preview(handle: &JSHandle, expected: &str) {
    for _ in 0..100 {
        if handle.to_string() == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(handle.to_string(), expected);
}

#[test]
fn should_have_a_nice_preview() {
    run(|t| async move {
        t.page
            .goto(format!("{}/dom.html", t.server.prefix))
            .await
            .unwrap();
        let outer = t.page.query_selector("#outer").await.unwrap().unwrap();
        let inner = t.page.query_selector("#inner").await.unwrap().unwrap();
        let check = t.page.query_selector("#check").await.unwrap().unwrap();
        let text = inner.evaluate_handle("e => e.firstChild").await.unwrap();
        t.page.evaluate::<i32>("() => 1").await.unwrap();
        poll_preview(&outer, "JSHandle@<div id=\"outer\" name=\"value\">…</div>").await;
        poll_preview(&inner, "JSHandle@<div id=\"inner\">Text,↵more text</div>").await;
        poll_preview(&text, "JSHandle@#text=Text,↵more text").await;
        poll_preview(
            &check,
            "JSHandle@<input checked id=\"check\" foo=\"bar\"\" type=\"checkbox\"/>",
        )
        .await;
    });
}

#[test]
fn should_have_a_nice_preview_for_non_ascii_attributes_children() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let emoji = "😛".repeat(100);
        t.page
            .set_content(format!("<div title=\"{emoji}\">{emoji}"))
            .await
            .unwrap();
        let handle = t.page.query_selector("div").await.unwrap().unwrap();
        let expected = format!(
            "JSHandle@<div title=\"{}\">{}…</div>",
            emoji,
            "😛".repeat(49)
        );
        poll_preview(&handle, &expected).await;
    });
}

#[test]
fn get_attribute_should_work() {
    run(|t| async move {
        t.page
            .goto(format!("{}/dom.html", t.server.prefix))
            .await
            .unwrap();
        let handle = t.page.query_selector("#outer").await.unwrap().unwrap();
        assert_eq!(
            handle.get_attribute("name").await.unwrap().as_deref(),
            Some("value")
        );
        assert_eq!(handle.get_attribute("foo").await.unwrap(), None);
        assert_eq!(
            t.page
                .get_attribute("#outer", "name")
                .await
                .unwrap()
                .as_deref(),
            Some("value")
        );
        assert_eq!(t.page.get_attribute("#outer", "foo").await.unwrap(), None);
    });
}

#[test]
fn input_value_should_work() {
    run(|t| async move {
        t.page
            .goto(format!("{}/dom.html", t.server.prefix))
            .await
            .unwrap();
        t.page.select_option("#select", "foo").await.unwrap();
        assert_eq!(t.page.input_value("#select").await.unwrap(), "foo");
        t.page.fill("#textarea", "text value").await.unwrap();
        assert_eq!(t.page.input_value("#textarea").await.unwrap(), "text value");
        t.page.fill("#input", "input value").await.unwrap();
        assert_eq!(t.page.input_value("#input").await.unwrap(), "input value");
        let handle = t.page.query_selector("#input").await.unwrap().unwrap();
        assert_eq!(handle.input_value().await.unwrap(), "input value");
        assert_err_contains(
            t.page.input_value("#inner").await,
            "Node is not an <input>, <textarea> or <select> element",
        );
        let handle2 = t.page.query_selector("#inner").await.unwrap().unwrap();
        assert_err_contains(
            handle2.input_value().await,
            "Node is not an <input>, <textarea> or <select> element",
        );
    });
}

#[test]
fn inner_html_should_work() {
    run(|t| async move {
        t.page
            .goto(format!("{}/dom.html", t.server.prefix))
            .await
            .unwrap();
        let handle = t.page.query_selector("#outer").await.unwrap().unwrap();
        assert_eq!(
            handle.inner_html().await.unwrap(),
            "<div id=\"inner\">Text,\nmore text</div>"
        );
        assert_eq!(
            t.page.inner_html("#outer").await.unwrap(),
            "<div id=\"inner\">Text,\nmore text</div>"
        );
    });
}

#[test]
fn inner_text_should_work() {
    run(|t| async move {
        t.page
            .goto(format!("{}/dom.html", t.server.prefix))
            .await
            .unwrap();
        let handle = t.page.query_selector("#inner").await.unwrap().unwrap();
        assert_eq!(handle.inner_text().await.unwrap(), "Text, more text");
        assert_eq!(
            t.page.inner_text("#inner").await.unwrap(),
            "Text, more text"
        );
    });
}

#[test]
fn inner_text_should_throw() {
    run(|t| async move {
        t.page.set_content("<svg>text</svg>").await.unwrap();
        assert_err_contains(t.page.inner_text("svg").await, "Node is not an HTMLElement");
        let handle = t.page.query_selector("svg").await.unwrap().unwrap();
        assert_err_contains(handle.inner_text().await, "Node is not an HTMLElement");
    });
}

#[test]
fn text_content_should_work() {
    run(|t| async move {
        t.page
            .goto(format!("{}/dom.html", t.server.prefix))
            .await
            .unwrap();
        let handle = t.page.query_selector("#inner").await.unwrap().unwrap();
        assert_eq!(
            handle.text_content().await.unwrap().as_deref(),
            Some("Text,\nmore text")
        );
        assert_eq!(
            t.page.text_content("#inner").await.unwrap().as_deref(),
            Some("Text,\nmore text")
        );
    });
}

#[test]
fn text_content_should_work_on_shadow_root() {
    run(|t| async move {
        t.page
            .set_content(
                "<div></div>
                <script>
                  document.querySelector('div').attachShadow({ mode: 'open' }).innerHTML = '<div>hello</div>';
                </script>",
            )
            .await
            .unwrap();
        let div = t.page.query_selector("div").await.unwrap().unwrap();
        let root = div.evaluate_handle("div => div.shadowRoot").await.unwrap();
        let root = root.as_element().unwrap();
        assert_eq!(root.text_content().await.unwrap().as_deref(), Some("hello"));
        // We do not match ShadowRoot as ":scope".
        assert!(
            root.query_selector_all(":scope div")
                .await
                .unwrap()
                .is_empty()
        );
    });
}

#[test]
fn is_visible_and_is_hidden_should_work() {
    run(|t| async move {
        t.page
            .set_content("<div>Hi</div><span></span>")
            .await
            .unwrap();
        let div = t.page.query_selector("div").await.unwrap().unwrap();
        assert!(div.is_visible().await.unwrap());
        assert!(!div.is_hidden().await.unwrap());
        assert!(t.page.is_visible("div").await.unwrap());
        assert!(!t.page.is_hidden("div").await.unwrap());
        let span = t.page.query_selector("span").await.unwrap().unwrap();
        assert!(!span.is_visible().await.unwrap());
        assert!(span.is_hidden().await.unwrap());
        assert!(!t.page.is_visible("span").await.unwrap());
        assert!(t.page.is_hidden("span").await.unwrap());
        assert!(!t.page.is_visible("no-such-element").await.unwrap());
        assert!(t.page.is_hidden("no-such-element").await.unwrap());
    });
}

#[test]
fn is_visible_should_not_throw_when_the_dom_element_is_not_connected() {
    run(|t| async move {
        t.page.set_content("<div id=\"root\"></div>").await.unwrap();
        t.page
            .evaluate::<()>(
                "() => {
                function insert() {
                  document.getElementById('root').innerHTML = '<div id=\"problem\">Problem</div>';
                  window.requestAnimationFrame(remove);
                }
                function remove() {
                  const node = document.getElementById('problem');
                  node?.parentNode?.removeChild(node);
                  window.requestAnimationFrame(insert);
                }
                window.requestAnimationFrame(insert);
              }",
            )
            .await
            .unwrap();
        for _ in 0..10 {
            t.page.is_visible("#problem").await.unwrap();
        }
    });
}

#[test]
fn is_enabled_and_is_disabled_should_work() {
    run(|t| async move {
        t.page
            .set_content(
                "<button disabled>button1</button>
                <button>button2</button>
                <div>div</div>",
            )
            .await
            .unwrap();
        let div = t.page.query_selector("div").await.unwrap().unwrap();
        assert!(div.is_enabled().await.unwrap());
        assert!(!div.is_disabled().await.unwrap());
        assert!(t.page.is_enabled("div").await.unwrap());
        assert!(!t.page.is_disabled("div").await.unwrap());
        let button1 = t
            .page
            .query_selector(":text(\"button1\")")
            .await
            .unwrap()
            .unwrap();
        assert!(!button1.is_enabled().await.unwrap());
        assert!(button1.is_disabled().await.unwrap());
        assert!(!t.page.is_enabled(":text(\"button1\")").await.unwrap());
        assert!(t.page.is_disabled(":text(\"button1\")").await.unwrap());
        let button2 = t
            .page
            .query_selector(":text(\"button2\")")
            .await
            .unwrap()
            .unwrap();
        assert!(button2.is_enabled().await.unwrap());
        assert!(!button2.is_disabled().await.unwrap());
        assert!(t.page.is_enabled(":text(\"button2\")").await.unwrap());
        assert!(!t.page.is_disabled(":text(\"button2\")").await.unwrap());
    });
}

#[test]
fn is_enabled_and_is_disabled_should_work_with_select_option_optgroup_correctly() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<select name="select">
                  <option id="enabled1" value="1">Enabled</option>
                  <option id="disabled1" value="2" disabled>Disabled</option>
                  <optgroup label="Foo1">
                    <option value="mercedes">Mercedes</option>
                  </optgroup>
                  <optgroup label="Foo2" disabled>
                    <option value="mercedes">Mercedes</option>
                  </optgroup>
                </select>"#,
            )
            .await
            .unwrap();
        let get = |s: &'static str| {
            let page = t.page.clone();
            async move { page.query_selector(s).await.unwrap().unwrap() }
        };
        assert!(get("#enabled1").await.is_enabled().await.unwrap());
        assert!(!get("#enabled1").await.is_disabled().await.unwrap());
        assert!(!get("#disabled1").await.is_enabled().await.unwrap());
        assert!(get("#disabled1").await.is_disabled().await.unwrap());
        assert!(get("optgroup >> nth=0").await.is_enabled().await.unwrap());
        assert!(!get("optgroup >> nth=0").await.is_disabled().await.unwrap());
        assert!(!get("optgroup >> nth=1").await.is_enabled().await.unwrap());
        assert!(get("optgroup >> nth=1").await.is_disabled().await.unwrap());
    });
}

#[test]
fn is_editable_should_work() {
    run(|t| async move {
        t.page
            .set_content("<input id=input1 disabled><textarea></textarea><input id=input2>")
            .await
            .unwrap();
        t.page
            .eval_on_selector::<bool>("textarea", "t => t.readOnly = true")
            .await
            .unwrap();
        let input1 = t.page.query_selector("#input1").await.unwrap().unwrap();
        assert!(!input1.is_editable().await.unwrap());
        assert!(!t.page.is_editable("#input1").await.unwrap());
        let input2 = t.page.query_selector("#input2").await.unwrap().unwrap();
        assert!(input2.is_editable().await.unwrap());
        assert!(t.page.is_editable("#input2").await.unwrap());
        let textarea = t.page.query_selector("textarea").await.unwrap().unwrap();
        assert!(!textarea.is_editable().await.unwrap());
        assert!(!t.page.is_editable("textarea").await.unwrap());
    });
}

#[test]
fn is_checked_should_work() {
    run(|t| async move {
        t.page
            .set_content("<input type='checkbox' checked><div>Not a checkbox</div>")
            .await
            .unwrap();
        let handle = t.page.query_selector("input").await.unwrap().unwrap();
        assert!(handle.is_checked().await.unwrap());
        assert!(t.page.is_checked("input").await.unwrap());
        handle
            .evaluate::<bool>("input => input.checked = false")
            .await
            .unwrap();
        assert!(!handle.is_checked().await.unwrap());
        assert!(!t.page.is_checked("input").await.unwrap());
        assert_err_contains(
            t.page.is_checked("div").await,
            "Not a checkbox or radio button",
        );
    });
}
