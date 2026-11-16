//! Port of `selectors-css.spec.ts`. All tests are ported (SE-001 only
//! compares counts, like the original with `measure = false`).

use super::*;
use crate::common::*;

async fn count(p: &oxidewright::Page, selector: &str) -> usize {
    eval_all::<usize>(p, selector, "els => els.length").await
}

async fn names(p: &oxidewright::Page, selector: &str) -> String {
    eval_all::<String>(p, selector, "els => els.map(e => e.nodeName).join(',')").await
}

async fn ids(p: &oxidewright::Page, selector: &str) -> Vec<String> {
    eval_all::<Vec<String>>(p, selector, "els => els.map(e => e.id)").await
}

async fn text(p: &oxidewright::Page, selector: &str) -> String {
    eval::<String>(p, selector, "e => e.textContent").await
}

#[test]
fn should_work_with_large_dom() {
    run(|t| async move {
        let p = &t.page;
        p.evaluate::<()>(
            r#"() => {
    let id = 0;
    const next = tag => {
      const e = document.createElement(tag);
      const eid = ++id;
      e.textContent = 'id' + eid;
      e.id = '' + eid;
      return e;
    };
    const generate = depth => {
      const div = next('div');
      const span1 = next('span');
      const span2 = next('span');
      div.appendChild(span1);
      div.appendChild(span2);
      if (depth > 0) {
        div.appendChild(generate(depth - 1));
        div.appendChild(generate(depth - 1));
      }
      return div;
    };
    document.body.appendChild(generate(12));
  }"#,
        )
        .await
        .unwrap();
        for selector in [
            "div div div span",
            "div > div div > span",
            "div + div div div span + span",
            "div ~ div div > span ~ span",
            "div > div > div + div > div + div > span ~ span",
            "div div div div div div div div div div span",
            "div > div > div > div > div > div > div > div > div > div > span",
            "div ~ div div ~ div div ~ div div ~ div div ~ div span",
            "span",
        ] {
            let ours = count(p, selector).await;
            let native: usize = p
                .evaluate("selector => document.querySelectorAll(selector).length")
                .arg(selector)
                .await
                .unwrap();
            assert_eq!(ours, native, "{selector}");
        }
    });
}

#[test]
fn should_work_for_open_shadow_roots() {
    run(|t| async move {
        let p = &t.page;
        p.goto(deep_shadow(&t.server.prefix)).await.unwrap();
        assert_eq!(text(p, "css=span").await, "Hello from root1");
        assert_eq!(
            text(p, r#"css=[attr="value\ space"]"#).await,
            "Hello from root3 #2"
        );
        assert_eq!(
            text(p, r"css=[attr='value\ \space']").await,
            "Hello from root3 #2"
        );
        assert_eq!(text(p, "css=div div span").await, "Hello from root2");
        assert_eq!(text(p, "css=div span + span").await, "Hello from root3 #2");
        assert_eq!(
            text(p, r#"css=span + [attr*="value"]"#).await,
            "Hello from root3 #2"
        );
        assert_eq!(
            text(p, r#"css=[data-testid="foo"] + [attr*="value"]"#).await,
            "Hello from root3 #2"
        );
        assert_eq!(text(p, "css=#target").await, "Hello from root2");
        assert_eq!(text(p, "css=div #target").await, "Hello from root2");
        assert_eq!(text(p, "css=div div #target").await, "Hello from root2");
        assert!(!exists(p, "css=div div div #target").await);
        assert_eq!(
            text(p, "css=section > div div span").await,
            "Hello from root2"
        );
        assert_eq!(
            text(p, "css=section > div div span:nth-child(2)").await,
            "Hello from root3 #2"
        );
        assert!(!exists(p, "css=section div div div div").await);

        let root2 = p.query_selector("css=div div").await.unwrap().unwrap();
        let v: String = root2
            .eval_on_selector("css=#target", "e => e.textContent")
            .await
            .unwrap();
        assert_eq!(v, "Hello from root2");
        assert!(
            root2
                .query_selector("css:light=#target")
                .await
                .unwrap()
                .is_none()
        );
        let shadow = root2
            .evaluate_handle("r => r.shadowRoot")
            .await
            .unwrap()
            .as_element()
            .unwrap();
        let v: String = shadow
            .eval_on_selector("css:light=#target", "e => e.textContent")
            .await
            .unwrap();
        assert_eq!(v, "Hello from root2");
        let root3 = p.query_selector_all("css=div div").await.unwrap().remove(1);
        let v: String = root3
            .eval_on_selector("text=root3", "e => e.textContent")
            .await
            .unwrap();
        assert_eq!(v, "Hello from root3");
        let v: String = root3
            .eval_on_selector(r#"css=[attr*="value"]"#, "e => e.textContent")
            .await
            .unwrap();
        assert_eq!(v, "Hello from root3 #2");
        assert!(
            root3
                .query_selector(r#"css:light=[attr*="value"]"#)
                .await
                .unwrap()
                .is_none()
        );
    });
}

#[test]
fn should_work_with_combinator_and_spaces() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(r#"<div foo="bar" bar="baz"><span></span></div>"#)
            .await
            .unwrap();
        for prefix in [r#"div[foo="bar"]"#, r#"div[foo="bar"][bar="baz"]"#] {
            for comb in [" > ", "> ", " >", ">", "   >    ", ">    ", "     >"] {
                let selector = format!("{prefix}{comb}span");
                assert_eq!(
                    eval::<String>(p, &selector, "e => e.outerHTML").await,
                    "<span></span>",
                    "{selector}"
                );
            }
        }
    });
}

#[test]
fn should_work_with_comma_separated_list() {
    run(|t| async move {
        let p = &t.page;
        p.goto(deep_shadow(&t.server.prefix)).await.unwrap();
        assert_eq!(count(p, "css=span,section #root1").await, 5);
        assert_eq!(count(p, "css=section #root1, div span").await, 5);
        assert_eq!(
            eval::<String>(p, "css=doesnotexist , section #root1", "e => e.id").await,
            "root1"
        );
        assert_eq!(count(p, "css=doesnotexist ,section #root1").await, 1);
        assert_eq!(count(p, "css=span,div span").await, 4);
        assert_eq!(count(p, "css=span,div span,div div span").await, 4);
        assert_eq!(count(p, r#"css=#target,[attr="value\ space"]"#).await, 2);
        assert_eq!(
            count(
                p,
                r#"css=#target,[data-testid="foo"],[attr="value\ space"]"#
            )
            .await,
            4
        );
        assert_eq!(
            count(
                p,
                r#"css=#target,[data-testid="foo"],[attr="value\ space"],span"#
            )
            .await,
            4
        );
    });
}

#[test]
fn should_keep_dom_order_with_comma_separated_list() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<section><span><div><x></x><y></y></div></span></section>")
            .await
            .unwrap();
        assert_eq!(names(p, "css=span,div").await, "SPAN,DIV");
        assert_eq!(names(p, "css=div,span").await, "SPAN,DIV");
        assert_eq!(names(p, "css=span div, div").await, "DIV");
        assert_eq!(names(p, "*css=section >> css=div,span").await, "SECTION");
        assert_eq!(names(p, "css=section >> *css=div >> css=x,y").await, "DIV");
        assert_eq!(
            names(p, "css=section >> *css=div,span >> css=x,y").await,
            "SPAN,DIV"
        );
        assert_eq!(
            names(p, "css=section >> *css=div,span >> css=y").await,
            "SPAN,DIV"
        );
    });
}

#[test]
fn should_return_multiple_captures_for_the_same_node() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<div><div><div><span></span></div></div></div>")
            .await
            .unwrap();
        assert_eq!(names(p, "*css=div >> span").await, "DIV,DIV,DIV");
    });
}

#[test]
fn should_return_multiple_captures_when_going_up_the_hierarchy() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<section>Hello<ul><li></li><li></li></ul></section>")
            .await
            .unwrap();
        assert_eq!(names(p, "*css=li >> ../.. >> text=Hello").await, "LI,LI");
    });
}

#[test]
fn should_work_with_comma_separated_list_in_various_positions() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<section><span><div><x></x><y></y></div></span></section>")
            .await
            .unwrap();
        for (selector, expected) in [
            ("css=span,div >> css=x,y", "X,Y"),
            ("css=span,div >> css=x", "X"),
            ("css=div >> css=x,y", "X,Y"),
            ("css=div >> css=x", "X"),
            ("css=section >> css=div >> css=x", "X"),
            ("css=section >> css=span >> css=div >> css=y", "Y"),
            ("css=section >> css=div >> css=x,y", "X,Y"),
            ("css=section >> css=div,span >> css=x,y", "X,Y"),
            ("css=section >> css=span >> css=x,y", "X,Y"),
        ] {
            assert_eq!(names(p, selector).await, expected, "{selector}");
        }
    });
}

#[test]
fn should_work_with_comma_inside_text() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(r#"<span></span><div attr="hello,world!"></div>"#)
            .await
            .unwrap();
        let div = r#"<div attr="hello,world!"></div>"#;
        for selector in [
            r#"css=div[attr="hello,world!"]"#,
            r#"css=[attr="hello,world!"]"#,
            "css=div[attr='hello,world!']",
            "css=[attr='hello,world!']",
        ] {
            assert_eq!(eval::<String>(p, selector, "e => e.outerHTML").await, div);
        }
        assert_eq!(
            eval::<String>(
                p,
                r#"css=div[attr="hello,world!"],span"#,
                "e => e.outerHTML"
            )
            .await,
            "<span></span>"
        );
    });
}

#[test]
fn should_work_with_attribute_selectors() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(r#"<div attr="hello world" attr2="hello-''>>foo=bar[]" attr3="] span"><span></span></div>"#)
            .await
            .unwrap();
        p.evaluate::<()>("() => window['div'] = document.querySelector('div')")
            .await
            .unwrap();
        for selector in [
            r#"[attr="hello world"]"#,
            r#"[attr = "hello world"]"#,
            "[attr ~= world]",
            "[attr ^=hello ]",
            "[attr $= world ]",
            r#"[attr *= "llo wor" ]"#,
            "[attr2 |= hello]",
            r#"[attr = "Hello World" i ]"#,
            r#"[attr *= "llo WOR"i]"#,
            "[attr $= woRLD i]",
            r#"[attr2 = "hello-''>>foo=bar[]"]"#,
            r#"[attr2 $="foo=bar[]"]"#,
        ] {
            assert!(
                eval::<bool>(p, selector, "e => e === window['div']").await,
                "{selector}"
            );
        }
        assert!(
            eval::<bool>(
                p,
                "[attr*=hello] span",
                "e => e.parentNode === window['div']"
            )
            .await
        );
        assert!(
            eval::<bool>(
                p,
                "[attr*=hello] >> span",
                "e => e.parentNode === window['div']"
            )
            .await
        );
        assert!(
            eval::<bool>(
                p,
                r#"[attr3="] span"] >> span"#,
                "e => e.parentNode === window['div']"
            )
            .await
        );
    });
}

#[test]
fn should_not_match_root_after() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<section><div>test</div></section>")
            .await
            .unwrap();
        assert!(!exists(p, "css=section >> css=section").await);
    });
}

#[test]
fn should_work_with_numerical_id() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(r#"<section id="123"></section>"#)
            .await
            .unwrap();
        assert!(exists(p, r"#\31\32\33").await);
    });
}

#[test]
fn should_work_with_wrong_case_id() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(r#"<section id="Hello"></section>"#)
            .await
            .unwrap();
        for selector in ["#Hello", "#hello", "#HELLO", "#helLO"] {
            assert_eq!(
                eval::<String>(p, selector, "e => e.tagName").await,
                "SECTION"
            );
        }
    });
}

#[test]
fn should_work_with_star() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<div id=div1></div><div id=div2><span><span></span></span></div>")
            .await
            .unwrap();
        for (selector, n) in [
            ("*", 7),
            ("*#div1", 1),
            ("*:not(#div1)", 6),
            ("*:not(div)", 5),
            ("*:not(span)", 5),
            ("*:not(*)", 0),
            ("*:is(*)", 7),
            ("* *", 6),
            ("* *:not(span)", 4),
            ("div > *", 1),
            ("div *", 2),
            ("* > *", 6),
        ] {
            assert_eq!(count(p, selector).await, n, "{selector}");
        }
        let body = p.query_selector("body").await.unwrap().unwrap();
        for (selector, n) in [
            ("*", 4),
            ("*#div1", 1),
            ("*:not(#div1)", 3),
            ("*:not(div)", 2),
            ("*:not(span)", 2),
            ("*:not(*)", 0),
            ("*:is(*)", 4),
            ("div > *", 1),
            ("div *", 2),
            ("* > *", 2),
            (":scope * > *", 2),
            ("* *", 2),
            ("* *:not(span)", 0),
        ] {
            let got: usize = body
                .eval_on_selector_all(selector, "els => els.length")
                .await
                .unwrap();
            assert_eq!(got, n, "body {selector}");
        }
    });
}

#[test]
fn should_work_with_nth_child() {
    run(|t| async move {
        let p = &t.page;
        p.goto(deep_shadow(&t.server.prefix)).await.unwrap();
        for (selector, n) in [
            ("css=span:nth-child(odd)", 3),
            ("css=span:nth-child(even)", 1),
            ("css=span:nth-child(n+1)", 4),
            ("css=span:nth-child(n+2)", 1),
            ("css=span:nth-child(2n)", 1),
            ("css=span:nth-child(2n+1)", 3),
            ("css=span:nth-child(-n)", 0),
            ("css=span:nth-child(-n+1)", 3),
            ("css=span:nth-child(-n+2)", 4),
            ("css=span:nth-child(23n+2)", 1),
        ] {
            assert_eq!(count(p, selector).await, n, "{selector}");
        }
    });
}

#[test]
fn should_work_with_nth_child_of_notation_with_nested_functions() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <div>
      <span>span1</span>
      <span class=foo>span2<dd></dd></span>
      <span class=foo>span3<dd class=marker></dd></span>
      <span class=foo>span4<dd class=marker></dd></span>
      <span class=foo>span5<dd></dd></span>
      <span>span6</span>
    </div>
  "#,
        )
        .await
        .unwrap();
        let texts = |sel: &'static str| {
            eval_all::<Vec<String>>(p, sel, "els => els.map(e => e.textContent)")
        };
        assert_eq!(texts("css=span:nth-child(1)").await, vec!["span1"]);
        assert_eq!(texts("css=span:nth-child(1 of .foo)").await, vec!["span2"]);
        assert_eq!(
            texts("css=span:nth-child(1 of .foo:has(dd.marker))").await,
            vec!["span3"]
        );
        assert_eq!(
            texts("css=span:nth-last-child(1 of .foo:has(dd.marker))").await,
            vec!["span4"]
        );
        assert_eq!(
            texts("css=span:nth-last-child(1 of .foo)").await,
            vec!["span5"]
        );
        assert_eq!(texts("css=span:nth-last-child(  1  )").await, vec!["span6"]);
        assert_eq!(
            texts("css=span:nth-child(1 of .foo:nth-child(3))").await,
            vec!["span3"]
        );
        assert!(
            texts("css=span:nth-child(1 of .foo:nth-child(6))")
                .await
                .is_empty()
        );
    });
}

#[test]
fn should_work_with_not() {
    run(|t| async move {
        let p = &t.page;
        p.goto(deep_shadow(&t.server.prefix)).await.unwrap();
        assert_eq!(count(p, "css=div:not(#root1)").await, 2);
        assert_eq!(count(p, "css=body :not(span)").await, 4);
        assert_eq!(count(p, "css=div > :not(span):not(div)").await, 0);
    });
}

#[test]
fn should_work_with_tilde() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            "<div id=div1></div><div id=div2></div><div id=div3></div><div id=div4></div><div id=div5></div><div id=div6></div>",
        )
        .await
        .unwrap();
        let after = vec!["div4", "div5", "div6"];
        assert_eq!(ids(p, "#div3 >> :scope ~ div").await, after);
        assert_eq!(ids(p, "#div3 >> :scope ~ *").await, after);
        assert_eq!(ids(p, "#div3 >> ~ div").await, after);
        assert_eq!(ids(p, "#div3 >> ~ *").await, after);
        assert_eq!(ids(p, "#div3 >> #div1 ~ :scope").await, vec!["div3"]);
        assert!(ids(p, "#div3 >> #div4 ~ :scope").await.is_empty());
        for (selector, n) in [
            ("css=#div1 ~ div ~ #div6", 1),
            ("css=#div1 ~ div ~ div", 4),
            ("css=#div3 ~ div ~ div", 2),
            ("css=#div4 ~ div ~ div", 1),
            ("css=#div5 ~ div ~ div", 0),
            ("css=#div3 ~ #div2 ~ #div6", 0),
            ("css=#div3 ~ #div4 ~ #div5", 1),
        ] {
            assert_eq!(count(p, selector).await, n, "{selector}");
        }
    });
}

#[test]
fn should_work_with_plus() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            "<section><div id=div1></div><div id=div2></div><div id=div3></div><div id=div4></div><div id=div5></div><div id=div6></div></section>",
        )
        .await
        .unwrap();
        assert_eq!(ids(p, "#div1 >> :scope+div").await, vec!["div2"]);
        assert_eq!(ids(p, "#div1 >> :scope+*").await, vec!["div2"]);
        assert_eq!(ids(p, "#div1 >> + div").await, vec!["div2"]);
        assert_eq!(ids(p, "#div1 >> + *").await, vec!["div2"]);
        assert_eq!(ids(p, "#div3 >> div + :scope").await, vec!["div3"]);
        assert!(ids(p, "#div3 >> #div1 + :scope").await.is_empty());
        for (selector, n) in [
            ("css=#div1 ~ div + #div6", 1),
            ("css=#div1 ~ div + div", 4),
            ("css=#div3 + div + div", 1),
            ("css=#div4 ~ #div5 + div", 1),
            ("css=#div5 + div + div", 0),
            ("css=#div3 ~ #div2 + #div6", 0),
            ("css=#div3 + #div4 + #div5", 1),
            ("css=div + #div1", 0),
            ("css=section > div + div ~ div", 4),
            ("css=section > div + #div4 ~ div", 2),
            ("css=section:has(:scope > div + #div2)", 1),
            ("css=section:has(:scope > div + #div1)", 0),
        ] {
            assert_eq!(count(p, selector).await, n, "{selector}");
        }
        assert_eq!(
            eval::<String>(p, "css=div:has(:scope + #div5)", "e => e.id").await,
            "div4"
        );
    });
}

#[test]
fn should_work_with_spaces_in_nth_child_and_not() {
    run(|t| async move {
        let p = &t.page;
        p.goto(deep_shadow(&t.server.prefix)).await.unwrap();
        for (selector, n) in [
            ("css=span:nth-child(23n +2)", 1),
            ("css=span:nth-child(23n+ 2)", 1),
            ("css=span:nth-child( 23n + 2 )", 1),
            ("css=span:not(#root1 #target)", 3),
            ("css=span:not(:not(#root1 #target))", 1),
            ("css=span:not(span:not(#root1 #target))", 1),
            ("css=div > :not(span)", 2),
            ("css=body :not(span, div)", 1),
            ("css=span, section:not(span, div)", 5),
            ("span:nth-child(23n+ 2) >> xpath=.", 1),
        ] {
            assert_eq!(count(p, selector).await, n, "{selector}");
        }
    });
}

#[test]
fn should_work_with_is() {
    run(|t| async move {
        let p = &t.page;
        p.goto(deep_shadow(&t.server.prefix)).await.unwrap();
        for (selector, n) in [
            ("css=div:is(#root1)", 1),
            ("css=div:is(#root1, #target)", 1),
            ("css=div:is(span, #target)", 0),
            ("css=div:is(span, #root1 > *)", 2),
            ("css=div:is(section div)", 3),
            ("css=:is(div, span)", 7),
            ("css=section:is(section) div:is(section div)", 3),
            ("css=:is(div, span) > *", 6),
            ("css=#root1:has(:is(#root1))", 0),
            ("css=#root1:has(:is(:scope, #root1))", 1),
        ] {
            assert_eq!(count(p, selector).await, n, "{selector}");
        }
    });
}

#[test]
fn should_work_with_has() {
    run(|t| async move {
        let p = &t.page;
        p.goto(deep_shadow(&t.server.prefix)).await.unwrap();
        assert_eq!(count(p, "css=div:has(#target)").await, 2);
        assert_eq!(count(p, "css=div:has([data-testid=foo])").await, 3);
        assert_eq!(count(p, "css=div:has([attr*=value])").await, 2);
        p.set_content("<section><span></span><div></div></section><section><br></section>")
            .await
            .unwrap();
        assert_eq!(count(p, "section:has(span, div)").await, 1);
        assert_eq!(count(p, "section:has(br)").await, 1);
        assert_eq!(count(p, "section:has(span, br)").await, 2);
        assert_eq!(count(p, "section:has(span, br, div)").await, 2);
    });
}

#[test]
fn should_work_with_scope() {
    run(|t| async move {
        let p = &t.page;
        p.goto(deep_shadow(&t.server.prefix)).await.unwrap();
        assert_eq!(count(p, "css=div:is(:scope#root1)").await, 0);
        assert_eq!(count(p, "css=div:is(:scope #root1)").await, 1);
        assert_eq!(count(p, "css=div:has(:scope > #target)").await, 1);

        let handle = p.query_selector("css=span").await.unwrap().unwrap();
        for (selector, n) in [
            ("css=:scope", 1),
            ("css=* :scope", 0),
            ("css=* + :scope", 0),
            ("css=* > :scope", 0),
            ("css=* ~ :scope", 0),
        ] {
            assert_eq!(count(p, selector).await, n, "page {selector}");
            let got: usize = handle
                .eval_on_selector_all(selector, "els => els.length")
                .await
                .unwrap();
            assert_eq!(got, n, "handle {selector}");
        }

        p.set_content("<article><div class=target>hello<span></span></div></article>")
            .await
            .unwrap();
        assert_eq!(
            eval::<String>(p, "div >> :scope.target", "e => e.textContent").await,
            "hello"
        );
        assert_eq!(
            eval::<String>(p, "div >> :scope:nth-child(1)", "e => e.textContent").await,
            "hello"
        );
        assert_eq!(
            eval::<String>(p, "div >> :scope.target:has(span)", "e => e.textContent").await,
            "hello"
        );
        assert_eq!(
            eval::<String>(p, "html:scope", "e => e.nodeName").await,
            "HTML"
        );

        p.set_content(
            "<section><span id=span1><span id=inner></span></span><span id=span2></span></section>",
        )
        .await
        .unwrap();
        assert_eq!(
            ids(p, "#span1 >> span:not(:has(:scope > div))").await,
            vec!["inner"]
        );
        assert_eq!(
            ids(p, "#span1 >> #inner,:scope").await,
            vec!["span1", "inner"]
        );
        assert_eq!(
            ids(p, "#span1 >> span,:scope").await,
            vec!["span1", "inner"]
        );
        assert_eq!(ids(p, "#span1 >> span:not(:scope)").await, vec!["inner"]);
    });
}

#[test]
fn should_work_with_scope_and_class() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<div class=\"apple\"></div>\n<div class=\"apple selected\"></div>")
            .await
            .unwrap();
        let selected = p.locator(".apple").locator(":scope.selected");
        expect_count(&selected, 1).await;
    });
}

#[test]
fn should_absolutize_relative_selectors() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<div><span>Hi</span></div>").await.unwrap();
        assert_eq!(
            eval::<String>(p, "div >> >span", "e => e.textContent").await,
            "Hi"
        );
        assert_eq!(
            p.locator("div")
                .locator(">span")
                .text_content()
                .await
                .unwrap()
                .as_deref(),
            Some("Hi")
        );
        assert_eq!(
            eval::<String>(p, "div:has(> span)", "e => e.outerHTML").await,
            "<div><span>Hi</span></div>"
        );
        assert!(!exists(p, "div:has(> div)").await);
    });
}

#[test]
fn css_on_the_handle_should_be_relative() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"<span class="find-me" id=target1>1</span>
    <div>
      <span class="find-me" id=target2>2</span>
    </div>"#,
        )
        .await
        .unwrap();
        assert_eq!(eval::<String>(p, ".find-me", "e => e.id").await, "target1");
        let div = p.query_selector("div").await.unwrap().unwrap();
        let v: String = div.eval_on_selector(".find-me", "e => e.id").await.unwrap();
        assert_eq!(v, "target2");
        assert_eq!(
            eval::<String>(p, "div >> .find-me", "e => e.id").await,
            "target2"
        );
    });
}

#[test]
fn should_use_light_dom_structure_for_child_combinator_with_slotted_content() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <my-button>
      <template shadowrootmode="open">
        <button><slot></slot></button>
      </template>
      <div class="content">Foo</div>
    </my-button>
  "#,
        )
        .await
        .unwrap();
        assert_eq!(
            eval::<String>(p, "my-button > div", "e => e.className").await,
            "content"
        );
        assert_eq!(
            eval::<String>(p, "my-button > .content", "e => e.textContent").await,
            "Foo"
        );
    });
}
