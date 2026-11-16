//! Port of `selectors-misc.spec.ts`. All tests are ported.

use std::time::Duration;

use oxidewright::WaitForSelectorState;
use serde_json::json;

use super::*;
use crate::common::*;

async fn count(p: &oxidewright::Page, selector: &str) -> usize {
    eval_all::<usize>(p, selector, "els => els.length").await
}

async fn id(p: &oxidewright::Page, selector: &str) -> String {
    eval::<String>(p, selector, "e => e.id").await
}

async fn ids(p: &oxidewright::Page, selector: &str) -> String {
    eval_all::<String>(p, selector, "els => els.map(e => e.id).join(',')").await
}

async fn texts_of(p: &oxidewright::Page, selector: &str) -> Vec<String> {
    eval_all::<Vec<String>>(p, selector, "els => els.map(e => e.textContent)").await
}

#[test]
fn should_work_for_open_shadow_roots() {
    run(|t| async move {
        let p = &t.page;
        p.goto(deep_shadow(&t.server.prefix)).await.unwrap();
        assert_eq!(
            eval::<String>(p, "id=target", "e => e.textContent").await,
            "Hello from root2"
        );
        assert_eq!(
            eval::<String>(p, "data-testid=foo", "e => e.textContent").await,
            "Hello from root1"
        );
        assert_eq!(count(p, "data-testid=foo").await, 3);
        assert!(!exists(p, "id:light=target").await);
        assert!(!exists(p, "data-testid:light=foo").await);
        assert!(
            p.query_selector_all("data-testid:light=foo")
                .await
                .unwrap()
                .is_empty()
        );
    });
}

#[test]
fn should_click_on_links_in_shadow_dom() {
    run(|t| async move {
        let p = &t.page;
        p.goto(format!("{}/shadow-dom-link.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(
            p.evaluate::<i32>("() => window.clickCount").await.unwrap(),
            0
        );
        p.click("#inner-link").await.unwrap();
        assert_eq!(
            p.evaluate::<i32>("() => window.clickCount").await.unwrap(),
            1
        );
    });
}

const TWO_DIVS: &str = "<section><div id=target1></div><div id=target2></div></section>";

async fn wait_attached_then_make_visible(p: &oxidewright::Page, selector: &'static str) {
    let waiter = tokio::spawn({
        let p = p.clone();
        async move {
            p.wait_for_selector(selector)
                .state(WaitForSelectorState::Attached)
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    p.eval_on_selector::<()>("#target2", "div => div.textContent = 'Now visible'")
        .await
        .unwrap();
    let element = waiter.await.unwrap().unwrap().unwrap();
    let id: String = element.evaluate("e => e.id").await.unwrap();
    assert_eq!(id, "target2");
}

#[test]
fn should_work_with_visible() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(TWO_DIVS).await.unwrap();
        assert!(!exists(p, "div:visible").await);
        let err = p
            .wait_for_selector("div:visible")
            .timeout(1000)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("1000ms"), "{err}");
        wait_attached_then_make_visible(p, "div:visible").await;
        assert_eq!(id(p, "div:visible").await, "target2");
    });
}

#[test]
fn should_work_with_visible_equals() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(TWO_DIVS).await.unwrap();
        assert!(!exists(p, "div >> visible=true").await);
        let err = p
            .wait_for_selector("div >> visible=true")
            .timeout(1000)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("1000ms"), "{err}");
        wait_attached_then_make_visible(p, "div >> visible=true").await;
        assert_eq!(id(p, "div >> visible=true").await, "target2");
    });
}

#[test]
fn should_work_with_visible_false() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(TWO_DIVS).await.unwrap();
        expect_count(&p.locator("div >> visible=false"), 2).await;
        p.locator("#target2")
            .evaluate::<()>("div => div.textContent = 'Now visible'")
            .await
            .unwrap();
        expect_count(&p.locator("div >> visible=false"), 1).await;
    });
}

async fn add_target3(p: &oxidewright::Page, selector: &'static str) {
    let waiter = tokio::spawn({
        let p = p.clone();
        async move {
            p.wait_for_selector(selector)
                .state(WaitForSelectorState::Attached)
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    p.eval_on_selector::<()>(
        "section",
        "section => { const div = document.createElement('div'); div.setAttribute('id', 'target3'); section.appendChild(div); }",
    )
    .await
    .unwrap();
    let element = waiter.await.unwrap().unwrap().unwrap();
    let id: String = element.evaluate("e => e.id").await.unwrap();
    assert_eq!(id, "target3");
}

#[test]
fn should_work_with_nth_match() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(TWO_DIVS).await.unwrap();
        assert!(!exists(p, ":nth-match(div, 3)").await);
        assert_eq!(id(p, ":nth-match(div, 1)").await, "target1");
        assert_eq!(id(p, ":nth-match(div, 2)").await, "target2");
        assert_eq!(id(p, ":nth-match(section > div, 2)").await, "target2");
        assert_eq!(id(p, ":nth-match(section, div, 2)").await, "target1");
        assert_eq!(id(p, ":nth-match(div, section, 3)").await, "target2");
        assert_eq!(
            count(p, ":is(:nth-match(div, 1), :nth-match(div, 2))").await,
            2
        );

        for (selector, message) in [
            (
                ":nth-match(div, bar, 0)",
                r#""nth-match" engine expects a one-based index as the last argument"#,
            ),
            (
                ":nth-match(2)",
                r#""nth-match" engine expects non-empty selector list and an index argument"#,
            ),
            (
                ":nth-match(div, bar, foo)",
                r#""nth-match" engine expects a one-based index as the last argument"#,
            ),
        ] {
            let err = p.query_selector(selector).await.unwrap_err().to_string();
            assert!(err.contains(message), "{err}");
        }
        add_target3(p, ":nth-match(div, 3)").await;
    });
}

#[test]
fn should_work_with_nth_equals() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(TWO_DIVS).await.unwrap();
        assert!(!exists(p, "div >> nth=2").await);
        assert_eq!(id(p, "div >> nth=0").await, "target1");
        assert_eq!(id(p, "div >> nth=1").await, "target2");
        assert_eq!(id(p, "section > div >> nth=1").await, "target2");
        assert_eq!(id(p, "section, div >> nth=1").await, "target1");
        assert_eq!(id(p, "div, section >> nth=2").await, "target2");
        add_target3(p, "div >> nth=2").await;

        p.set_content("<div><div><div><span>hi</span><span>hello</span></div></div></div>")
            .await
            .unwrap();
        assert_eq!(
            p.locator("div >> div >> span >> nth=1")
                .text_content()
                .await
                .unwrap()
                .as_deref(),
            Some("hello")
        );
    });
}

#[test]
fn should_work_with_strict_mode_and_chaining() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<div><div><div><span>hi</span></div></div></div>")
            .await
            .unwrap();
        assert_eq!(
            p.locator("div >> div >> span")
                .text_content()
                .await
                .unwrap()
                .as_deref(),
            Some("hi")
        );
    });
}

#[test]
fn should_work_with_layout_selectors() {
    run(|t| async move {
        let p = &t.page;
        let boxes = json!([
            [0, 0, 150, 150],
            [100, 200, 50, 50],
            [200, 200, 50, 50],
            [100, 150, 50, 50],
            [201, 150, 50, 50],
            [200, 100, 50, 50],
            [50, 50, 50, 50],
            [150, 50, 50, 50],
            [150, -51, 50, 50],
            [201, -101, 50, 50],
        ]);
        p.set_content(
            r#"<container style="width: 500px; height: 500px; position: relative;"></container>"#,
        )
        .await
        .unwrap();
        p.eval_on_selector::<()>(
            "container",
            r#"(container, boxes) => {
    for (let i = 0; i < boxes.length; i++) {
      const div = document.createElement('div');
      div.style.position = 'absolute';
      div.style.overflow = 'hidden';
      div.style.boxSizing = 'border-box';
      div.style.border = '1px solid black';
      div.id = 'id' + i;
      div.textContent = 'id' + i;
      const box = boxes[i];
      div.style.left = box[0] + 'px';
      div.style.top = (250 - box[1] - box[3]) + 'px';
      div.style.width = box[2] + 'px';
      div.style.height = box[3] + 'px';
      container.appendChild(div);
      const span = document.createElement('span');
      span.textContent = '' + i;
      div.appendChild(span);
    }
  }"#,
        )
        .arg(boxes)
        .await
        .unwrap();

        for (selector, expected) in [
            ("div:right-of(#id6)", "id7"),
            ("div:right-of(#id1)", "id2"),
            ("div:right-of(#id3)", "id4"),
            ("div:right-of(#id0)", "id7"),
            ("div:right-of(#id8)", "id9"),
            ("div:left-of(#id2)", "id1"),
            ("div:left-of(#id5)", "id0"),
            ("div:left-of(#id9)", "id8"),
            ("div:left-of(#id4)", "id3"),
            ("div:above(#id0)", "id3"),
            ("div:above(#id5)", "id4"),
            ("div:above(#id7)", "id5"),
            ("div:above(#id8)", "id0"),
            ("div:above(#id9)", "id8"),
            ("div:below(#id4)", "id5"),
            ("div:below(#id3)", "id0"),
            ("div:below(#id2)", "id4"),
            ("div:below(#id6)", "id8"),
            ("div:below(#id7)", "id8"),
            ("div:below(#id8)", "id9"),
            ("div:near(#id0)", "id3"),
            ("div:below(#id5):above(#id8)", "id7"),
        ] {
            assert_eq!(id(p, selector).await, expected, "{selector}");
        }
        for selector in [
            "div:right-of(#id4)",
            "div:left-of(#id0)",
            "div:above(#id2)",
            "div:below(#id9)",
        ] {
            assert!(!exists(p, selector).await, "{selector}");
        }
        for (selector, expected) in [
            ("div:right-of(#id3)", "id4,id2,id5,id7,id8,id9"),
            ("div:right-of(#id3, 50)", "id2,id5,id7,id8"),
            ("div:right-of(#id3, 49)", "id7,id8"),
            ("div:left-of(#id5)", "id0,id7,id3,id1,id6,id8"),
            ("div:left-of(#id5, 3)", "id7,id8"),
            ("div:above(#id5)", "id4,id2,id3,id1"),
            ("div:above(#id5, 20)", "id4,id3"),
            ("div:below(#id3)", "id0,id5,id6,id7,id8,id9"),
            ("div:below(#id3, 105)", "id0,id5,id6,id7"),
            ("div:near(#id7)", "id0,id5,id3,id6"),
            ("div:near(#id0)", "id3,id6,id7,id8,id1,id5"),
            ("div:near(#id6)", "id0,id3,id7"),
            ("div:near(#id6, 10)", "id0"),
            ("div:near(#id0, 100)", "id3,id6,id7,id8,id1,id5,id4,id2"),
            ("div:below(#id5):above(#id8)", "id7,id6"),
            ("div:right-of(#id0) + div:above(#id8)", "id5,id6,id3"),
        ] {
            assert_eq!(ids(p, selector).await, expected, "{selector}");
        }
        let err = p.query_selector(":near(50)").await.unwrap_err().to_string();
        assert!(
            err.contains(
                r#""near" engine expects a selector list and optional maximum distance in pixels"#
            ),
            "{err}"
        );
    });
}

#[test]
fn should_escape_the_scope_with() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<div><label>Test</label><input id='myinput'></div>")
            .await
            .unwrap();
        assert_eq!(id(p, "label >> xpath=.. >> input").await, "myinput");
    });
}

const FIND_ME: &str = r#"
    <span class="find-me" id=target1>1</span>
    <div>
      <span class="find-me" id=target2>2</span>
    </div>
  "#;

#[test]
fn xpath_should_be_relative() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(FIND_ME).await.unwrap();
        assert_eq!(id(p, r#"//*[@class="find-me"]"#).await, "target1");
        let div = p.query_selector("div").await.unwrap().unwrap();
        for selector in [
            r#"xpath=./*[@class="find-me"]"#,
            r#"xpath=.//*[@class="find-me"]"#,
            r#"//*[@class="find-me"]"#,
            r#"xpath=/*[@class="find-me"]"#,
        ] {
            let v: String = div.eval_on_selector(selector, "e => e.id").await.unwrap();
            assert_eq!(v, "target2", "{selector}");
            assert_eq!(
                id(p, &format!("div >> {selector}")).await,
                "target2",
                "{selector}"
            );
        }
    });
}

#[test]
fn should_work_with_pipe_in_xpath() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <span class="find-me" id=t1>1</span>
    <div>
      <span class="find-me" id=t2>2</span>
    </div>
    <div id=t3>3</span>
  "#,
        )
        .await
        .unwrap();
        assert_eq!(count(p, r#"//*[@id="t1"]|//*[@id="t3"]"#).await, 2);
        let e1 = p
            .wait_for_selector(r#"//*[@id="t1"]|//*[@id="t3"]"#)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(e1.evaluate::<String>("e => e.id").await.unwrap(), "t1");
        let e2 = p
            .wait_for_selector(r#"//*[@id="unknown"]|//*[@id="t2"]"#)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(e2.evaluate::<String>("e => e.id").await.unwrap(), "t2");
        p.click(r#"//code|//span[@id="t2"]"#).await.unwrap();
    });
}

#[test]
fn should_print_original_xpath_in_error() {
    run(|t| async move {
        let err = t
            .page
            .locator("//*[contains(@Class, 'foo']")
            .is_visible()
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains(r"//*[contains(@Class, \'foo\']"), "{err}");
        assert!(!err.contains(".//*[contains(@Class, 'foo']"), "{err}");
    });
}

#[test]
fn data_testid_on_the_handle_should_be_relative() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <span data-testid="find-me" id=target1>1</span>
    <div>
      <span data-testid="find-me" id=target2>2</span>
    </div>
  "#,
        )
        .await
        .unwrap();
        assert_eq!(id(p, "data-testid=find-me").await, "target1");
        let div = p.query_selector("div").await.unwrap().unwrap();
        let v: String = div
            .eval_on_selector("data-testid=find-me", "e => e.id")
            .await
            .unwrap();
        assert_eq!(v, "target2");
        assert_eq!(id(p, "div >> data-testid=find-me").await, "target2");
    });
}

#[test]
fn should_properly_determine_visibility_of_display_contents_elements() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(r#"<div><p style="display:contents">DISPLAY CONTENTS</p></div>"#)
            .await
            .unwrap();
        p.wait_for_selector(r#""DISPLAY CONTENTS""#)
            .timeout(5000)
            .await
            .unwrap();
        for content in [
            r#"<div><article style="display:contents"><div>DISPLAY CONTENTS</div></article></div>"#,
            r#"<div><article style="display:contents"><div style="display:contents">DISPLAY CONTENTS</div></article></div>"#,
            r#"<div><article style="display:contents"><div></div>DISPLAY CONTENTS<span></span></article></div>"#,
        ] {
            p.set_content(content).await.unwrap();
            p.wait_for_selector("article").timeout(5000).await.unwrap();
        }
        p.set_content(r#"<div><article style="display:contents"><div></div></article></div>"#)
            .await
            .unwrap();
        p.wait_for_selector("article")
            .state(WaitForSelectorState::Hidden)
            .timeout(5000)
            .await
            .unwrap();
    });
}

#[test]
fn should_work_with_internal_has() {
    run(|t| async move {
        let p = &t.page;
        p.goto(deep_shadow(&t.server.prefix)).await.unwrap();
        assert_eq!(count(p, r##"div >> internal:has="#target""##).await, 2);
        assert_eq!(
            count(p, r#"div >> internal:has="[data-testid=foo]""#).await,
            3
        );
        assert_eq!(count(p, r#"div >> internal:has="[attr*=value]""#).await, 2);

        p.set_content("<section><span></span><div></div></section><section><br></section>")
            .await
            .unwrap();
        assert_eq!(count(p, r#"section >> internal:has="span, div""#).await, 1);
        assert_eq!(count(p, r#"section >> internal:has="br""#).await, 1);
        assert_eq!(count(p, r#"section >> internal:has="span, br""#).await, 2);
        assert_eq!(
            count(p, r#"section >> internal:has="span, br, div""#).await,
            2
        );

        p.set_content("<div><span>hello</span></div><div><span>world</span></div>")
            .await
            .unwrap();
        let outer = |sel: &'static str| eval::<String>(p, sel, "e => e.outerHTML");
        assert_eq!(count(p, r#"div >> internal:has="text=world""#).await, 1);
        assert_eq!(
            outer(r#"div >> internal:has="text=world""#).await,
            "<div><span>world</span></div>"
        );
        assert_eq!(count(p, r#"div >> internal:has="text=\"hello\"""#).await, 1);
        assert_eq!(
            outer(r#"div >> internal:has="text=\"hello\"""#).await,
            "<div><span>hello</span></div>"
        );
        assert_eq!(count(p, r#"div >> internal:has="xpath=./span""#).await, 2);
        assert_eq!(count(p, r#"div >> internal:has="span""#).await, 2);
        assert_eq!(
            count(p, r#"div >> internal:has="span >> text=wor""#).await,
            1
        );
        assert_eq!(
            outer(r#"div >> internal:has="span >> text=wor""#).await,
            "<div><span>world</span></div>"
        );
        assert_eq!(
            outer(r#"div >> internal:has="span >> text=wor" >> span"#).await,
            "<span>world</span>"
        );

        for (selector, message) in [
            (
                "div >> internal:has=abc",
                "Malformed selector: internal:has=abc",
            ),
            (
                r#"internal:has="div""#,
                r#""internal:has" selector cannot be first"#,
            ),
            (
                "div >> internal:has=33",
                "Malformed selector: internal:has=33",
            ),
            (
                r#"div >> internal:has="span!""#,
                r#"Unexpected token "!" while parsing css selector "span!""#,
            ),
        ] {
            let err = p.query_selector(selector).await.unwrap_err().to_string();
            assert!(err.contains(message), "{selector}: {err}");
        }
    });
}

#[test]
fn should_work_with_internal_has_not() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<section><span></span><div></div></section><section><br></section>")
            .await
            .unwrap();
        for (selector, n) in [
            (r#"section >> internal:has-not="span""#, 1),
            (r#"section >> internal:has-not="span, div, br""#, 0),
            (r#"section >> internal:has-not="br""#, 1),
            (r#"section >> internal:has-not="span, div""#, 1),
            (r#"section >> internal:has-not="article""#, 2),
        ] {
            assert_eq!(count(p, selector).await, n, "{selector}");
        }
    });
}

#[test]
fn should_work_with_internal_and() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <div class=foo>hello</div><div class=bar>world</div>
    <span class=foo>hello2</span><span class=bar>world2</span>
  "#,
        )
        .await
        .unwrap();
        assert!(
            texts_of(p, r#"div >> internal:and="span""#)
                .await
                .is_empty()
        );
        assert_eq!(
            texts_of(p, r#"div >> internal:and=".foo""#).await,
            vec!["hello"]
        );
        assert_eq!(
            texts_of(p, r#"div >> internal:and=".bar""#).await,
            vec!["world"]
        );
        assert_eq!(
            texts_of(p, r#"span >> internal:and="span""#).await,
            vec!["hello2", "world2"]
        );
        assert_eq!(
            texts_of(p, r#".foo >> internal:and="div""#).await,
            vec!["hello"]
        );
        assert_eq!(
            texts_of(p, r#".bar >> internal:and="span""#).await,
            vec!["world2"]
        );
    });
}

#[test]
fn should_work_with_internal_or() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<div>hello</div>\n<span>world</span>")
            .await
            .unwrap();
        assert_eq!(
            texts_of(p, r#"div >> internal:or="span""#).await,
            vec!["hello", "world"]
        );
        assert_eq!(
            texts_of(p, r#"span >> internal:or="div""#).await,
            vec!["hello", "world"]
        );
        assert_eq!(count(p, r#"article >> internal:or="something""#).await, 0);
        for (selector, expected) in [
            (r#"article >> internal:or="div""#, "hello"),
            (r#"article >> internal:or="span""#, "world"),
            (r#"div >> internal:or="article""#, "hello"),
            (r#"span >> internal:or="article""#, "world"),
        ] {
            assert_eq!(
                p.locator(selector).text_content().await.unwrap().as_deref(),
                Some(expected)
            );
        }
    });
}

#[test]
fn should_work_with_internal_chain() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            "<div>one <span>two</span> <button>three</button> </div>\n<span>four</span>\n<button>five</button>",
        )
        .await
        .unwrap();
        assert_eq!(
            texts_of(p, r#"div >> internal:chain="button""#).await,
            vec!["three"]
        );
        assert_eq!(
            texts_of(
                p,
                r#"div >> internal:chain="span >> internal:or=\"button\"""#
            )
            .await,
            vec!["two", "three"]
        );
    });
}

#[test]
fn chaining_should_work_with_large_dom() {
    run(|t| async move {
        let p = &t.page;
        p.evaluate::<()>(
            r#"() => {
    let last = document.body;
    for (let i = 0; i < 100; i++) {
      const e = document.createElement('div');
      last.appendChild(e);
      last = e;
    }
    const target = document.createElement('span');
    target.textContent = 'Found me!';
    last.appendChild(target);
  }"#,
        )
        .await
        .unwrap();
        for selector in [
            "div >> div >> div >> div >> div >> div >> div >> div >> span",
            "div div div div div div div div span",
            "div div >> div div >> div div >> div div >> span",
        ] {
            assert_eq!(count(p, selector).await, 1, "{selector}");
        }
    });
}
