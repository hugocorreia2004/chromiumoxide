//! Port of `locator-query.spec.ts`.
//!
//! Skipped:
//! - LO-182 `count() should not throw during navigation`: needs
//!   `selectors.register` (custom selector engines are not supported).

use oxidewright::{GetByRoleOptions, LocatorOptions, regex};
use serde_json::Value;

use super::*;
use crate::common::*;

const SECTIONS: &str = r#"
  <section>
    <div><p>A</p></div>
    <div><p>A</p><p>A</p></div>
    <div><p>A</p><p>A</p><p>A</p></div>
  </section>"#;

#[test]
fn should_respect_first_and_last() {
    run(|t| async move {
        t.page.set_content(SECTIONS).await.unwrap();
        assert_eq!(t.page.locator("div >> p").count().await.unwrap(), 6);
        assert_eq!(t.page.locator("div").locator("p").count().await.unwrap(), 6);
        assert_eq!(
            t.page
                .locator("div")
                .first()
                .locator("p")
                .count()
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            t.page
                .locator("div")
                .last()
                .locator("p")
                .count()
                .await
                .unwrap(),
            3
        );
    });
}

#[test]
fn should_respect_nth() {
    run(|t| async move {
        t.page.set_content(SECTIONS).await.unwrap();
        assert_eq!(t.page.locator("div >> p").nth(0).count().await.unwrap(), 1);
        assert_eq!(
            t.page
                .locator("div")
                .nth(1)
                .locator("p")
                .count()
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            t.page
                .locator("div")
                .nth(2)
                .locator("p")
                .count()
                .await
                .unwrap(),
            3
        );
    });
}

#[test]
fn should_throw_on_capture_w_nth() {
    run(|t| async move {
        t.page
            .set_content("<section><div><p>A</p></div></section>")
            .await
            .unwrap();
        let e = t
            .page
            .locator("*css=div >> p")
            .nth(1)
            .click()
            .await
            .unwrap_err();
        assert!(e.to_string().contains("Can't query n-th element"), "{e}");
    });
}

#[test]
fn should_throw_on_due_to_strictness() {
    run(|t| async move {
        t.page
            .set_content("<div>A</div><div>B</div>")
            .await
            .unwrap();
        let e = t.page.locator("div").is_visible().await.unwrap_err();
        assert!(e.to_string().contains("strict mode violation"), "{e}");
    });
}

#[test]
fn should_throw_on_due_to_strictness_2() {
    run(|t| async move {
        t.page
            .set_content("<select><option>One</option><option>Two</option></select>")
            .await
            .unwrap();
        let e = t
            .page
            .locator("option")
            .evaluate::<Value>("e => {}")
            .await
            .unwrap_err();
        assert!(e.to_string().contains("strict mode violation"), "{e}");
    });
}

#[test]
fn should_filter_by_text() {
    run(|t| async move {
        t.page
            .set_content("<div>Foobar</div><div>Bar</div>")
            .await
            .unwrap();
        expect_text(
            &t.page
                .locator_with("div", LocatorOptions::new().has_text("Foo")),
            "Foobar",
        )
        .await;
    });
}

#[test]
fn should_filter_by_text_2() {
    run(|t| async move {
        t.page
            .set_content("<div>foo <span>hello world</span> bar</div>")
            .await
            .unwrap();
        expect_text(
            &t.page
                .locator_with("div", LocatorOptions::new().has_text("hello world")),
            "foo hello world bar",
        )
        .await;
    });
}

#[test]
fn should_filter_by_regex() {
    run(|t| async move {
        t.page
            .set_content("<div>Foobar</div><div>Bar</div>")
            .await
            .unwrap();
        expect_text(
            &t.page
                .locator_with("div", LocatorOptions::new().has_text(regex("Foo.*", ""))),
            "Foobar",
        )
        .await;
    });
}

#[test]
fn should_filter_by_text_with_quotes() {
    run(|t| async move {
        t.page
            .set_content("<div>Hello \"world\"</div><div>Hello world</div>")
            .await
            .unwrap();
        expect_text(
            &t.page
                .locator_with("div", LocatorOptions::new().has_text("Hello \"world\"")),
            "Hello \"world\"",
        )
        .await;
    });
}

#[test]
fn should_filter_by_regex_with_quotes() {
    run(|t| async move {
        t.page
            .set_content("<div>Hello \"world\"</div><div>Hello world</div>")
            .await
            .unwrap();
        expect_text(
            &t.page.locator_with(
                "div",
                LocatorOptions::new().has_text(regex("Hello \"world\"", "")),
            ),
            "Hello \"world\"",
        )
        .await;
    });
}

#[test]
fn should_filter_by_regex_with_a_single_quote() {
    run(|t| async move {
        let page = &t.page;
        let by_text = |re: &str, flags: &str| {
            page.locator_with("button", LocatorOptions::new().has_text(regex(re, flags)))
        };
        let by_role = |re: &str, flags: &str| {
            page.get_by_role_with("button", GetByRoleOptions::new().name(regex(re, flags)))
        };
        page.set_content("<button>let's let's<span>hello</span></button>")
            .await
            .unwrap();
        for re in [
            r"let's",
            r"let\'s",
            r"'s",
            r"\'s",
            r"let['abc]s",
            r"let's let\'s",
            r"let\'s let's",
        ] {
            expect_text(&by_text(re, "i").locator("span"), "hello").await;
            expect_text(&by_role(re, "i").locator("span"), "hello").await;
        }
        expect_hidden(&by_text(r"let\\'s", "i")).await;
        expect_hidden(&by_role(r"let\\'s", "i")).await;

        page.set_content("<button>let\\'s let\\'s<span>hello</span></button>")
            .await
            .unwrap();
        expect_hidden(&by_text(r"let\'s", "i")).await;
        expect_hidden(&by_role(r"let\'s", "i")).await;
        for re in [
            r"let\\'s",
            r"let\\\'s",
            r"let\\'s let\\\'s",
            r"let\\\'s let\\'s",
        ] {
            expect_text(&by_text(re, "i").locator("span"), "hello").await;
            expect_text(&by_role(re, "i").locator("span"), "hello").await;
        }

        page.set_content("<button>let's hello</button>")
            .await
            .unwrap();
        expect_text(&by_text("let's", "iu"), "let's hello").await;
        expect_text(&by_role("let's", "iu"), "let's hello").await;
    });
}

#[test]
fn should_filter_by_regex_and_regexp_flags() {
    run(|t| async move {
        t.page
            .set_content("<div>Hello \"world\"</div><div>Hello world</div>")
            .await
            .unwrap();
        expect_text(
            &t.page.locator_with(
                "div",
                LocatorOptions::new().has_text(regex("hElLo \"world\"", "i")),
            ),
            "Hello \"world\"",
        )
        .await;
    });
}

#[test]
fn should_filter_by_case_insensitive_regex_in_a_child() {
    run(|t| async move {
        t.page
            .set_content("<div class=\"test\"><h5>Title Text</h5></div>")
            .await
            .unwrap();
        expect_text(
            &t.page.locator_with(
                "div",
                LocatorOptions::new().has_text(regex("^title text$", "i")),
            ),
            "Title Text",
        )
        .await;
    });
}

#[test]
fn should_filter_by_case_insensitive_regex_in_multiple_children() {
    run(|t| async move {
        t.page
            .set_content("<div class=\"test\"><h5>Title</h5> <h2><i>Text</i></h2></div>")
            .await
            .unwrap();
        expect_class(
            &t.page.locator_with(
                "div",
                LocatorOptions::new().has_text(regex("^title text$", "i")),
            ),
            "test",
        )
        .await;
    });
}

#[test]
fn should_filter_by_regex_with_special_symbols() {
    run(|t| async move {
        t.page
            .set_content("<div class=\"test\"><h5>First/\"and\"</h5><h2><i>Second\\</i></h2></div>")
            .await
            .unwrap();
        expect_class(
            &t.page.locator_with(
                "div",
                LocatorOptions::new().has_text(regex(r#"^first\/".*"second\\$"#, "si")),
            ),
            "test",
        )
        .await;
    });
}

#[test]
fn should_support_has_locator() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<div><span>hello</span></div><div><span>world</span></div>")
            .await
            .unwrap();
        let has =
            |inner: oxidewright::Locator| p.locator_with("div", LocatorOptions::new().has(&inner));
        expect_count(&has(p.locator("text=world")), 1).await;
        let html: String = has(p.locator("text=world"))
            .evaluate("e => e.outerHTML")
            .await
            .unwrap();
        assert_eq!(html, "<div><span>world</span></div>");
        expect_count(&has(p.locator("text=\"hello\"")), 1).await;
        let html: String = has(p.locator("text=\"hello\""))
            .evaluate("e => e.outerHTML")
            .await
            .unwrap();
        assert_eq!(html, "<div><span>hello</span></div>");
        expect_count(&has(p.locator("xpath=./span")), 2).await;
        expect_count(&has(p.locator("span")), 2).await;
        let wor = p.locator_with("span", LocatorOptions::new().has_text("wor"));
        expect_count(&has(wor.clone()), 1).await;
        let html: String = has(wor).evaluate("e => e.outerHTML").await.unwrap();
        assert_eq!(html, "<div><span>world</span></div>");
        expect_count(
            &p.locator_with(
                "div",
                LocatorOptions::new()
                    .has(&p.locator("span"))
                    .has_text("wor"),
            ),
            1,
        )
        .await;
    });
}

#[test]
fn should_support_locator_filter() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            "<section><div><span>hello</span></div><div><span>world</span></div></section>",
        )
        .await
        .unwrap();
        let f = LocatorOptions::new;
        expect_count(&p.locator("div").filter(f().has_text("hello")), 1).await;
        expect_count(
            &p.locator_with("div", f().has_text("hello"))
                .filter(f().has_text("hello")),
            1,
        )
        .await;
        expect_count(
            &p.locator_with("div", f().has_text("hello"))
                .filter(f().has_text("world")),
            0,
        )
        .await;
        expect_count(
            &p.locator_with("section", f().has_text("hello"))
                .filter(f().has_text("world")),
            1,
        )
        .await;
        expect_count(
            &p.locator("div")
                .filter(f().has_text("hello"))
                .locator("span"),
            1,
        )
        .await;
        expect_count(
            &p.locator("div")
                .filter(f().has(&p.locator_with("span", f().has_text("world")))),
            1,
        )
        .await;
        expect_count(&p.locator("div").filter(f().has(&p.locator("span"))), 2).await;
        expect_count(
            &p.locator("div")
                .filter(f().has(&p.locator("span")).has_text("world")),
            1,
        )
        .await;
        expect_count(
            &p.locator("div")
                .filter(f().has_not(&p.locator_with("span", f().has_text("world")))),
            1,
        )
        .await;
        expect_count(
            &p.locator("div").filter(f().has_not(&p.locator("section"))),
            2,
        )
        .await;
        expect_count(&p.locator("div").filter(f().has_not(&p.locator("span"))), 0).await;
        expect_count(&p.locator("div").filter(f().has_not_text("hello")), 1).await;
        expect_count(&p.locator("div").filter(f().has_not_text("foo")), 2).await;
    });
}

#[test]
fn should_support_locator_and() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"<div data-testid=foo>hello</div><div data-testid=bar>world</div>
    <span data-testid=foo>hello2</span><span data-testid=bar>world2</span>"#,
        )
        .await
        .unwrap();
        expect_count(&p.locator("div").and_(&p.locator("div")), 2).await;
        expect_texts(&p.locator("div").and_(&p.get_by_test_id("foo")), &["hello"]).await;
        expect_texts(&p.locator("div").and_(&p.get_by_test_id("bar")), &["world"]).await;
        expect_texts(&p.get_by_test_id("foo").and_(&p.locator("div")), &["hello"]).await;
        expect_texts(
            &p.get_by_test_id("bar").and_(&p.locator("span")),
            &["world2"],
        )
        .await;
        expect_count(
            &p.locator("span")
                .and_(&p.get_by_test_id(regex("bar|foo", ""))),
            2,
        )
        .await;
    });
}

#[test]
fn should_support_locator_or() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<div>hello</div><span>world</span>")
            .await
            .unwrap();
        expect_count(&p.locator("div").or_(&p.locator("span")), 2).await;
        expect_texts(
            &p.locator("div").or_(&p.locator("span")),
            &["hello", "world"],
        )
        .await;
        expect_texts(
            &p.locator("span")
                .or_(&p.locator("article"))
                .or_(&p.locator("div")),
            &["hello", "world"],
        )
        .await;
        expect_count(&p.locator("article").or_(&p.locator("something")), 0).await;
        expect_text(&p.locator("article").or_(&p.locator("div")), "hello").await;
        expect_text(&p.locator("article").or_(&p.locator("span")), "world").await;
        expect_text(&p.locator("div").or_(&p.locator("article")), "hello").await;
        expect_text(&p.locator("span").or_(&p.locator("article")), "world").await;
    });
}

#[test]
fn should_support_locator_locator_with_and_or() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"<div>one <span>two</span> <button>three</button> </div>
    <span>four</span>
    <button>five</button>"#,
        )
        .await
        .unwrap();
        expect_texts(&p.locator("div").locator(p.locator("button")), &["three"]).await;
        expect_texts(
            &p.locator("div")
                .locator(p.locator("button").or_(&p.locator("span"))),
            &["two", "three"],
        )
        .await;
        expect_texts(
            &p.locator("button").or_(&p.locator("span")),
            &["two", "three", "four", "five"],
        )
        .await;
        expect_texts(
            &p.locator("div")
                .locator(p.locator("button").and_(&p.get_by_role("button"))),
            &["three"],
        )
        .await;
        expect_texts(
            &p.locator("button").and_(&p.get_by_role("button")),
            &["three", "five"],
        )
        .await;
    });
}

#[test]
fn should_support_locator_within() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"<table>
      <tr><td>a1</td><td>a2</td><td>a3</td></tr>
      <tr><td>b1</td><td>b2</td><td>b3</td></tr>
      <tr><td>c1</td><td>c2</td><td>c3</td></tr>
    </table>
    <span>outside</span>"#,
        )
        .await
        .unwrap();
        let row = p.get_by_role("row");
        expect_texts(
            &p.get_by_role("cell").within(&row),
            &["a1", "a2", "a3", "b1", "b2", "b3", "c1", "c2", "c3"],
        )
        .await;
        expect_texts(
            &p.get_by_role("cell").nth(1).within(&row),
            &["a2", "b2", "c2"],
        )
        .await;
        expect_texts(
            &p.get_by_role("cell").last().within(&row),
            &["a3", "b3", "c3"],
        )
        .await;
        expect_texts(&p.get_by_role("cell").nth(1).within(&row.nth(2)), &["c2"]).await;
        expect_count(&p.locator("span").within(&row), 0).await;
        expect_text(&p.get_by_role("cell").nth(1).within(&row).nth(1), "b2").await;
    });
}

#[test]
fn should_allow_some_but_not_all_nested_frame_locators() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            "<iframe srcdoc=\"<span id=target>world</span>\"></iframe><span>hello</span>",
        )
        .await
        .unwrap();
        let fl = || p.frame_locator("iframe");
        expect_text(&fl().locator("span").or_(&fl().locator("article")), "world").await;
        expect_text(&fl().locator("article").or_(&fl().locator("span")), "world").await;
        expect_text(
            &fl().locator("span").and_(&fl().locator("#target")),
            "world",
        )
        .await;
        let error1 = try_text(
            &fl()
                .locator("div")
                .or_(&p.frame_locator("#iframe").locator("span")),
            "world",
        )
        .await
        .unwrap_err();
        assert!(error1.contains("Frame locators are not allowed inside composite locators, while querying \"locator('iframe').contentFrame().locator('div').or(locator('#iframe').contentFrame().locator('span'))"), "{error1}");
        let error2 = try_text(
            &fl()
                .locator("div")
                .and_(&p.frame_locator("#iframe").locator("span")),
            "world",
        )
        .await
        .unwrap_err();
        assert!(error2.contains("Frame locators are not allowed inside composite locators, while querying \"locator('iframe').contentFrame().locator('div').and(locator('#iframe').contentFrame().locator('span'))"), "{error2}");
    });
}

#[test]
fn should_keep_the_capture_when_removing_a_common_frame_prefix() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            "<iframe id=f srcdoc=\"<section id=target><span>hi</span></section>\"></iframe>",
        )
        .await
        .unwrap();
        let inner = p.frame_locator("#f").locator("*css=section >> span");
        expect_attribute(
            &p.frame_locator("#f").locator("body").locator(inner),
            "id",
            "target",
        )
        .await;
        let capture_frame = p.locator("*css=#f").content_frame().locator("span");
        let error = p
            .frame_locator("#f")
            .locator("body")
            .locator(capture_frame)
            .count()
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Can not capture the selector before diving into the frame"),
            "{error}"
        );
    });
}

#[test]
fn should_enforce_same_frame_for_has_left_of_right_of_above_below_near() {
    run(|t| async move {
        t.page
            .goto(format!("{}/frames/two-frames.html", t.server.prefix))
            .await
            .unwrap();
        let child = t.page.frames()[1].clone();
        let locator = t
            .page
            .locator_with("div", LocatorOptions::new().has(&child.locator("span")));
        let error = locator.count().await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Inner \"has\" locator must belong to the same frame."),
            "{error}"
        );
    });
}

#[test]
fn alias_methods_coverage() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<div><button>Submit</button></div>")
            .await
            .unwrap();
        expect_count(&p.locator("button"), 1).await;
        expect_count(&p.locator("div").locator("button"), 1).await;
        expect_count(&p.locator("div").get_by_role("button"), 1).await;
        expect_count(&p.main_frame().locator("button"), 1).await;
    });
}
