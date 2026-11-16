//! Port of `selectors-get-by.spec.ts`.
//!
//! All tests are ported. `selectors().set_test_id_attribute` is global: the
//! tests changing it build their locators right after setting it (the
//! selector string captures the attribute name) and restore `data-testid`
//! immediately, so concurrently running tests are not affected.

use oxidewright::{GetByOptions, GetByRoleOptions, regex, selectors};

use super::*;
use crate::common::*;

#[test]
fn get_by_test_id_should_work() {
    run(|t| async move {
        t.page
            .set_content(r#"<div><div data-testid="Hello">Hello world</div></div>"#)
            .await
            .unwrap();
        expect_text(&t.page.get_by_test_id("Hello"), "Hello world").await;
        expect_text(&t.page.main_frame().get_by_test_id("Hello"), "Hello world").await;
        expect_text(
            &t.page.locator("div").get_by_test_id("Hello"),
            "Hello world",
        )
        .await;
    });
}

#[test]
fn get_by_test_id_with_custom_test_id_should_work() {
    run(|t| async move {
        t.page
            .set_content(r#"<div><div data-my-custom-testid="Hello">Hello world</div></div>"#)
            .await
            .unwrap();
        selectors().set_test_id_attribute("data-my-custom-testid");
        let a = t.page.get_by_test_id("Hello");
        let b = t.page.main_frame().get_by_test_id("Hello");
        let c = t.page.locator("div").get_by_test_id("Hello");
        selectors().set_test_id_attribute("data-testid");
        assert_eq!(
            a.selector(),
            r#"internal:testid=[data-my-custom-testid="Hello"s]"#
        );
        expect_text(&a, "Hello world").await;
        expect_text(&b, "Hello world").await;
        expect_text(&c, "Hello world").await;
    });
}

#[test]
fn get_by_test_id_with_comma_separated_test_id_attributes_should_match_any() {
    run(|t| async move {
        t.page
            .set_content(
                r#"
    <section>
      <div data-pw="Hello">first</div>
      <div data-ti="Hello">second</div>
      <div data-testid="Hello">third</div>
    </section>
  "#,
            )
            .await
            .unwrap();
        selectors().set_test_id_attribute("data-pw,data-ti");
        let a = t.page.get_by_test_id("Hello");
        let b = t.page.main_frame().get_by_test_id("Hello");
        let c = t.page.locator("section").get_by_test_id("Hello");
        selectors().set_test_id_attribute("data-testid");
        assert_eq!(
            a.selector(),
            r#"internal:testid=["data-pw,data-ti"="Hello"s]"#
        );
        expect_count(&a, 2).await;
        expect_texts(&a, &["first", "second"]).await;
        expect_count(&b, 2).await;
        expect_count(&c, 2).await;
    });
}

#[test]
fn get_by_test_id_should_escape_id() {
    run(|t| async move {
        t.page
            .set_content(r#"<div><div data-testid='He"llo'>Hello world</div></div>"#)
            .await
            .unwrap();
        expect_text(&t.page.get_by_test_id("He\"llo"), "Hello world").await;
    });
}

#[test]
fn get_by_test_id_should_work_for_regex() {
    run(|t| async move {
        t.page
            .set_content(r#"<div><div data-testid="Hello">Hello world</div></div>"#)
            .await
            .unwrap();
        expect_text(&t.page.get_by_test_id(regex("He[l]*o", "")), "Hello world").await;
        expect_text(&t.page.main_frame().get_by_test_id("Hello"), "Hello world").await;
        expect_text(
            &t.page.locator("div").get_by_test_id("Hello"),
            "Hello world",
        )
        .await;
    });
}

async fn outer(l: &oxidewright::Locator) -> String {
    l.evaluate::<String>("e => e.outerHTML").await.unwrap()
}

#[test]
fn get_by_text_should_work() {
    run(|t| async move {
        t.page
            .set_content("<div>yo</div><div>ya</div><div>\nye  </div>")
            .await
            .unwrap();
        assert!(
            outer(&t.page.get_by_text("ye"))
                .await
                .contains(">\nye  </div>")
        );
        assert!(
            outer(&t.page.get_by_text(regex("ye", "")))
                .await
                .contains(">\nye  </div>")
        );
        assert!(
            outer(&t.page.get_by_text(regex("e", "")))
                .await
                .contains(">\nye  </div>")
        );

        t.page
            .set_content("<div> ye </div><div>ye</div>")
            .await
            .unwrap();
        assert!(
            outer(&t.page.get_by_text_with("ye", GetByOptions::exact()).first())
                .await
                .contains("> ye </div>")
        );

        t.page
            .set_content("<div>Hello world</div><div>Hello</div>")
            .await
            .unwrap();
        assert!(
            outer(&t.page.get_by_text_with("Hello", GetByOptions::exact()))
                .await
                .contains(">Hello</div>")
        );
    });
}

async fn node_name(l: &oxidewright::Locator) -> String {
    l.evaluate::<String>("e => e.nodeName").await.unwrap()
}

async fn id(l: &oxidewright::Locator) -> String {
    l.evaluate::<String>("e => e.id").await.unwrap()
}

#[test]
fn get_by_label_should_work() {
    run(|t| async move {
        t.page
            .set_content("<div><label for=target>Name</label><input id=target type=text></div>")
            .await
            .unwrap();
        assert_eq!(node_name(&t.page.get_by_text("Name")).await, "LABEL");
        assert_eq!(node_name(&t.page.get_by_label("Name")).await, "INPUT");
        assert_eq!(
            node_name(&t.page.main_frame().get_by_label("Name")).await,
            "INPUT"
        );
        assert_eq!(
            node_name(&t.page.locator("div").get_by_label("Name")).await,
            "INPUT"
        );
    });
}

#[test]
fn get_by_label_should_work_with_nested_elements() {
    run(|t| async move {
        t.page
            .set_content(
                "<label for=target>Last <span>Name</span></label><input id=target type=text>",
            )
            .await
            .unwrap();
        let p = &t.page;
        expect_attr(&p.get_by_label("last name"), "id", "target").await;
        expect_attr(&p.get_by_label("st na"), "id", "target").await;
        expect_attr(&p.get_by_label("Name"), "id", "target").await;
        expect_attr(
            &p.get_by_label_with("Last Name", GetByOptions::exact()),
            "id",
            "target",
        )
        .await;
        expect_attr(&p.get_by_label(regex("Last\\s+name", "i")), "id", "target").await;

        for l in [
            p.get_by_label_with("Last", GetByOptions::exact()),
            p.get_by_label_with("last name", GetByOptions::exact()),
            p.get_by_label_with("Name", GetByOptions::exact()),
            p.get_by_label("what?"),
            p.get_by_label(regex("last name", "")),
        ] {
            assert!(l.element_handles().await.unwrap().is_empty(), "{l}");
        }
    });
}

#[test]
fn get_by_label_should_work_with_multiply_labelled_input() {
    run(|t| async move {
        t.page
            .set_content("<label for=target>Name</label><input id=target type=text><label for=target>First or Last</label>")
            .await
            .unwrap();
        assert_eq!(id(&t.page.get_by_label("Name")).await, "target");
        assert_eq!(id(&t.page.get_by_label("First or Last")).await, "target");
    });
}

#[test]
fn get_by_label_should_work_with_ancestor_label_and_multiple_controls() {
    run(|t| async move {
        t.page
            .set_content("<label>Name<button id=target>Click me</button><input type=text></label>")
            .await
            .unwrap();
        assert_eq!(id(&t.page.get_by_label("Name")).await, "target");
    });
}

#[test]
fn get_by_label_should_work_with_ancestor_label_and_for() {
    run(|t| async move {
        t.page
            .set_content(
                "<label for=target>Name<input type=text id=nontarget></label>\n<input type=text id=target>",
            )
            .await
            .unwrap();
        assert_eq!(id(&t.page.get_by_label("Name")).await, "target");
    });
}

#[test]
fn get_by_label_should_work_with_aria_labelledby() {
    run(|t| async move {
        t.page
            .set_content("<label id=name-label>Name</label><button aria-labelledby=name-label>Click me</button>")
            .await
            .unwrap();
        let text: String = t
            .page
            .get_by_label("Name")
            .evaluate("e => e.textContent")
            .await
            .unwrap();
        assert_eq!(text, "Click me");
    });
}

#[test]
fn get_by_label_should_prioritize_aria_labelledby_over_native_label() {
    run(|t| async move {
        t.page
            .set_content(
                "<label id=name-label>Name</label>\n<label>Wrong<button aria-labelledby=name-label>Click me</button></label>",
            )
            .await
            .unwrap();
        let text: String = t
            .page
            .get_by_label("Name")
            .evaluate("e => e.textContent")
            .await
            .unwrap();
        assert_eq!(text, "Click me");
    });
}

#[test]
fn get_by_label_should_work_with_aria_label() {
    run(|t| async move {
        t.page
            .set_content(r#"<input id=target aria-label="Name">"#)
            .await
            .unwrap();
        assert_eq!(id(&t.page.get_by_label("Name")).await, "target");
    });
}

#[test]
fn get_by_label_should_ignore_empty_aria_label() {
    run(|t| async move {
        t.page
            .set_content(
                "<label for=target>Last Name</label><input id=target type=text aria-label>",
            )
            .await
            .unwrap();
        assert_eq!(id(&t.page.get_by_label("Last Name")).await, "target");
    });
}

#[test]
fn get_by_label_should_prioritize_aria_labelledby_over_aria_label() {
    run(|t| async move {
        t.page
            .set_content(r#"<label id=other-label>Other</label><input id=target aria-label="Name" aria-labelledby=other-label>"#)
            .await
            .unwrap();
        assert_eq!(id(&t.page.get_by_label("Other")).await, "target");
    });
}

#[test]
fn get_by_placeholder_should_work() {
    run(|t| async move {
        t.page
            .set_content(
                "<div>\n<input placeholder='Hello'>\n<input placeholder='Hello World'>\n</div>",
            )
            .await
            .unwrap();
        let p = &t.page;
        expect_count(&p.get_by_placeholder("hello"), 2).await;
        expect_count(
            &p.get_by_placeholder_with("Hello", GetByOptions::exact()),
            1,
        )
        .await;
        expect_count(&p.get_by_placeholder(regex("wor", "i")), 1).await;
        expect_count(&p.main_frame().get_by_placeholder("hello"), 2).await;
        expect_count(&p.locator("div").get_by_placeholder("hello"), 2).await;
    });
}

#[test]
fn get_by_alt_text_should_work() {
    run(|t| async move {
        t.page
            .set_content("<div>\n<input alt='Hello'>\n<input alt='Hello World'>\n</div>")
            .await
            .unwrap();
        let p = &t.page;
        expect_count(&p.get_by_alt_text("hello"), 2).await;
        expect_count(&p.get_by_alt_text_with("Hello", GetByOptions::exact()), 1).await;
        expect_count(&p.get_by_alt_text(regex("wor", "i")), 1).await;
        expect_count(&p.main_frame().get_by_alt_text("hello"), 2).await;
        expect_count(&p.locator("div").get_by_alt_text("hello"), 2).await;
    });
}

#[test]
fn get_by_title_should_work() {
    run(|t| async move {
        t.page
            .set_content("<div>\n<input title='Hello'>\n<input title='Hello World'>\n</div>")
            .await
            .unwrap();
        let p = &t.page;
        expect_count(&p.get_by_title("hello"), 2).await;
        expect_count(&p.get_by_title_with("Hello", GetByOptions::exact()), 1).await;
        expect_count(&p.get_by_title(regex("wor", "i")), 1).await;
        expect_count(&p.main_frame().get_by_title("hello"), 2).await;
        expect_count(&p.locator("div").get_by_title("hello"), 2).await;
    });
}

#[test]
fn get_by_escaping() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<label id=label for=control>Hello my\nwo\"rld</label><input id=control />")
            .await
            .unwrap();
        p.eval_on_selector::<()>(
            "input",
            r#"input => {
    input.setAttribute('placeholder', 'hello my\nwo"rld');
    input.setAttribute('title', 'hello my\nwo"rld');
    input.setAttribute('alt', 'hello my\nwo"rld');
  }"#,
        )
        .await
        .unwrap();
        let s = "hello my\nwo\"rld";
        expect_attr(&p.get_by_text(s), "id", "label").await;
        expect_attr(&p.get_by_text("hello       my     wo\"rld"), "id", "label").await;
        expect_attr(&p.get_by_label(s), "id", "control").await;
        expect_attr(&p.get_by_placeholder(s), "id", "control").await;
        expect_attr(&p.get_by_alt_text(s), "id", "control").await;
        expect_attr(&p.get_by_title(s), "id", "control").await;

        p.set_content("<label id=label for=control>Hello my\nworld</label><input id=control />")
            .await
            .unwrap();
        p.eval_on_selector::<()>(
            "input",
            r#"input => {
    input.setAttribute('placeholder', 'hello my\nworld');
    input.setAttribute('title', 'hello my\nworld');
    input.setAttribute('alt', 'hello my\nworld');
  }"#,
        )
        .await
        .unwrap();
        let s = "hello my\nworld";
        expect_attr(&p.get_by_text(s), "id", "label").await;
        expect_attr(&p.get_by_text("hello        my    world"), "id", "label").await;
        expect_attr(&p.get_by_label(s), "id", "control").await;
        expect_attr(&p.get_by_placeholder(s), "id", "control").await;
        expect_attr(&p.get_by_alt_text(s), "id", "control").await;
        expect_attr(&p.get_by_title(s), "id", "control").await;

        p.set_content(r#"<div id=target title="my title">Text here</div>"#)
            .await
            .unwrap();
        let exact = GetByOptions::exact();
        expect_count(&p.get_by_title_with("my title", exact), 1).await;
        // JS 'my t\itle' is "my title".
        expect_count(&p.get_by_title_with("my title", exact), 1).await;
        expect_count(&p.get_by_title_with("my t\\itle", exact), 0).await;
        expect_count(&p.get_by_title_with("my t\\itle", exact), 0).await;
        expect_count(&p.get_by_title_with("my t\\\\itle", exact), 0).await;

        p.set_content("<label for=target>foo &gt;&gt; bar</label><input id=target>")
            .await
            .unwrap();
        p.eval_on_selector::<()>(
            "input",
            r#"input => {
    input.setAttribute('placeholder', 'foo >> bar');
    input.setAttribute('title', 'foo >> bar');
    input.setAttribute('alt', 'foo >> bar');
  }"#,
        )
        .await
        .unwrap();
        assert_eq!(
            p.get_by_text("foo >> bar")
                .text_content()
                .await
                .unwrap()
                .as_deref(),
            Some("foo >> bar")
        );
        expect_text(&p.locator("label"), "foo >> bar").await;
        expect_text(&p.get_by_text("foo >> bar"), "foo >> bar").await;
        assert_eq!(
            p.get_by_text(regex("foo >> bar", ""))
                .text_content()
                .await
                .unwrap()
                .as_deref(),
            Some("foo >> bar")
        );
        expect_attr(&p.get_by_label("foo >> bar"), "id", "target").await;
        expect_attr(&p.get_by_label(regex("foo >> bar", "")), "id", "target").await;
        expect_attr(&p.get_by_placeholder("foo >> bar"), "id", "target").await;
        expect_attr(&p.get_by_alt_text("foo >> bar"), "id", "target").await;
        expect_attr(&p.get_by_title("foo >> bar"), "id", "target").await;
        expect_attr(
            &p.get_by_placeholder(regex("foo >> bar", "")),
            "id",
            "target",
        )
        .await;
        expect_attr(&p.get_by_alt_text(regex("foo >> bar", "")), "id", "target").await;
        expect_attr(&p.get_by_title(regex("foo >> bar", "")), "id", "target").await;
    });
}

fn role(name: impl Into<oxidewright::TextMatch>) -> GetByRoleOptions {
    GetByRoleOptions::new().name(name)
}

#[test]
fn get_by_role_escaping() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <a href="https://playwright.dev">issues 123</a>
    <a href="https://playwright.dev">he llo 56</a>
    <button>Click me</button>
  "#,
        )
        .await
        .unwrap();
        let link1 = r#"<a href="https://playwright.dev">issues 123</a>"#;
        let link2 = r#"<a href="https://playwright.dev">he llo 56</a>"#;
        let button = "<button>Click me</button>";
        assert_eq!(outer_htmls(&p.get_by_role("button")).await, vec![button]);
        assert_eq!(
            outer_htmls(&p.get_by_role("link")).await,
            vec![link1, link2]
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("link", role("issues 123"))).await,
            vec![link1]
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("link", role("sues"))).await,
            vec![link1]
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("link", role("  he    \n  llo "))).await,
            vec![link2]
        );
        assert!(
            outer_htmls(&p.get_by_role_with("button", role("issues")))
                .await
                .is_empty()
        );
        assert!(
            outer_htmls(&p.get_by_role_with("link", role("sues").exact(true)))
                .await
                .is_empty()
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("link", role("   he \n llo 56 ").exact(true))).await,
            vec![link2]
        );
        assert_eq!(
            outer_htmls(&p.get_by_role_with("button", role("Click me").exact(true))).await,
            vec![button]
        );
        // JS 'Click \me' is "Click me".
        assert_eq!(
            outer_htmls(&p.get_by_role_with("button", role("Click me").exact(true))).await,
            vec![button]
        );
        for name in ["Click \\me", "Click \\me", "Click \\\\me"] {
            assert!(
                outer_htmls(&p.get_by_role_with("button", role(name).exact(true)))
                    .await
                    .is_empty(),
                "{name}"
            );
        }
    });
}

#[test]
fn get_by_role_should_accept_regexp_with_v_flag() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<button>Click me</button><button>Submit</button>")
            .await
            .unwrap();
        expect_count(
            &p.get_by_role_with("button", role(regex("Click me", "v"))),
            1,
        )
        .await;
        expect_count(
            &p.get_by_role_with("button", role(regex("click me", "iv"))),
            1,
        )
        .await;
        expect_count(
            &p.get_by_role_with("button", role(regex("Missing", "v"))),
            0,
        )
        .await;
    });
}

fn desc(d: impl Into<oxidewright::TextMatch>) -> GetByRoleOptions {
    GetByRoleOptions::new().description(d)
}

#[test]
fn get_by_role_with_description() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <div role="alert" aria-label="Upload successful" aria-description="File doc-2025.pdf was uploaded successfully">Alert 1</div>
    <div role="alert" aria-label="Upload successful" aria-description="File report-2026.pdf was uploaded successfully">Alert 2</div>
    <div role="alert" aria-label="Invalid file" aria-description="File demo.doc has an invalid file format">Alert 3</div>
  "#,
        )
        .await
        .unwrap();
        let q = |o: GetByRoleOptions| p.get_by_role_with("alert", o);
        assert_eq!(texts(&q(desc("doc-2025"))).await, vec!["Alert 1"]);
        assert_eq!(texts(&q(desc("report-2026"))).await, vec!["Alert 2"]);
        assert_eq!(
            texts(&q(desc("doc-2025").name("Upload successful"))).await,
            vec!["Alert 1"]
        );
        assert_eq!(
            texts(&q(desc("report-2026").name("Upload successful"))).await,
            vec!["Alert 2"]
        );
        assert!(
            texts(&q(desc("doc-2025").name("Invalid file")))
                .await
                .is_empty()
        );
        assert!(texts(&q(desc("doc-2025").exact(true))).await.is_empty());
        assert_eq!(
            texts(&q(
                desc("File doc-2025.pdf was uploaded successfully").exact(true)
            ))
            .await,
            vec!["Alert 1"]
        );
        assert_eq!(
            texts(&q(desc(regex("report-\\d+", "")))).await,
            vec!["Alert 2"]
        );
        assert_eq!(
            texts(&q(desc(regex("uploaded successfully$", "")))).await,
            vec!["Alert 1", "Alert 2"]
        );
    });
}

#[test]
fn get_by_role_with_description_via_aria_describedby() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <button aria-describedby="desc1">Submit</button>
    <span id="desc1">Submits the form data</span>
    <button aria-describedby="desc2">Submit</button>
    <span id="desc2">Saves as draft</span>
  "#,
        )
        .await
        .unwrap();
        assert_eq!(
            texts(&p.get_by_role_with("button", desc("form data").name("Submit"))).await,
            vec!["Submit"]
        );
        assert_eq!(
            texts(&p.get_by_role_with("button", desc("draft").name("Submit"))).await,
            vec!["Submit"]
        );
    });
}

#[test]
fn get_by_role_with_description_via_title_fallback() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <button title="Submits the form">Submit</button>
    <button title="Resets the form">Reset</button>
  "#,
        )
        .await
        .unwrap();
        assert_eq!(
            texts(&p.get_by_role_with("button", desc("Submits"))).await,
            vec!["Submit"]
        );
        assert_eq!(
            texts(&p.get_by_role_with("button", desc("Resets"))).await,
            vec!["Reset"]
        );
    });
}

#[test]
fn get_by_role_with_description_whitespace_normalization() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"<div role="alert" aria-description="File  doc-2025.pdf   was uploaded   successfully">Alert</div>"#,
        )
        .await
        .unwrap();
        assert_eq!(
            texts(&p.get_by_role_with("alert", desc("  doc-2025.pdf \n was  uploaded "))).await,
            vec!["Alert"]
        );
    });
}

#[test]
fn locators_should_not_be_affected_by_global_and_sticky_regex_flags() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <div>foo</div><div>foo</div><div>foo</div>
    <label>foo<input></label><label>foo<input></label><label>foo<input></label>
    <button>foo</button><button>foo</button><button>foo</button>
    <input placeholder=foo><input placeholder=foo><input placeholder=foo>
  "#,
        )
        .await
        .unwrap();
        for flags in ["", "g", "y"] {
            expect_count(&p.get_by_text(regex("foo", flags)), 9).await;
            expect_count(&p.get_by_label(regex("foo", flags)), 3).await;
            expect_count(&p.get_by_role_with("button", role(regex("foo", flags))), 3).await;
            expect_count(&p.get_by_placeholder(regex("foo", flags)), 3).await;
        }
    });
}
