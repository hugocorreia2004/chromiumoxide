//! Port of `selectors-text.spec.ts`. All tests are ported (SE-036 only runs
//! the queries, like the original with `measure = false`).

use oxidewright::{GetByOptions, LocatorOptions, regex};

use super::*;
use crate::common::*;

async fn outer(p: &oxidewright::Page, selector: &str) -> String {
    eval::<String>(p, selector, "e => e.outerHTML").await
}

async fn id(p: &oxidewright::Page, selector: &str) -> String {
    eval::<String>(p, selector, "e => e.id").await
}

async fn count(p: &oxidewright::Page, selector: &str) -> usize {
    eval_all::<usize>(p, selector, "els => els.length").await
}

#[test]
fn should_work() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<div>yo</div><div>ya</div><div>\nye  </div>")
            .await
            .unwrap();
        assert_eq!(outer(p, "text=ya").await, "<div>ya</div>");
        assert_eq!(outer(p, r#"text="ya""#).await, "<div>ya</div>");
        assert_eq!(outer(p, "text=/^[ay]+$/").await, "<div>ya</div>");
        assert_eq!(outer(p, "text=/Ya/i").await, "<div>ya</div>");
        assert_eq!(outer(p, "text=ye").await, "<div>\nye  </div>");
        let ye: String = p
            .get_by_text("ye")
            .evaluate("e => e.outerHTML")
            .await
            .unwrap();
        assert!(ye.contains(">\nye  </div>"));

        p.set_content("<div> ye </div><div>ye</div>").await.unwrap();
        assert_eq!(outer(p, r#"text="ye""#).await, "<div> ye </div>");
        let ye: String = p
            .get_by_text_with("ye", GetByOptions::exact())
            .first()
            .evaluate("e => e.outerHTML")
            .await
            .unwrap();
        assert!(ye.contains("> ye </div>"));

        p.set_content(r#"<div>yo</div><div>"ya</div><div> hello world! </div>"#)
            .await
            .unwrap();
        assert_eq!(outer(p, r#"text="\"ya""#).await, r#"<div>"ya</div>"#);
        assert_eq!(outer(p, "text=/hello/").await, "<div> hello world! </div>");
        assert_eq!(
            outer(p, r"text=/^\s*heLLo/i").await,
            "<div> hello world! </div>"
        );

        p.set_content("<div>yo<div>ya</div>hey<div>hey</div></div>")
            .await
            .unwrap();
        assert_eq!(outer(p, "text=hey").await, "<div>hey</div>");
        for selector in [
            r#"text=yo>>text="ya""#,
            r#"text=yo>> text="ya""#,
            "text=yo >>text='ya'",
            "text=yo >> text='ya'",
            r#"'yo'>>"ya""#,
            r#""yo" >> 'ya'"#,
        ] {
            assert_eq!(outer(p, selector).await, "<div>ya</div>", "{selector}");
        }

        p.set_content(r#"<div>yo<span id="s1"></span></div><div>yo<span id="s2"></span><span id="s3"></span></div>"#)
            .await
            .unwrap();
        assert_eq!(
            eval_all::<String>(p, "text=yo", "es => es.map(e => e.outerHTML).join('\\n')").await,
            "<div>yo<span id=\"s1\"></span></div>\n<div>yo<span id=\"s2\"></span><span id=\"s3\"></span></div>"
        );

        p.set_content(r#"<div>'</div><div>"</div><div>\</div><div>x</div>"#)
            .await
            .unwrap();
        assert_eq!(outer(p, r"text='\''").await, "<div>'</div>");
        assert_eq!(outer(p, r#"text='"'"#).await, r#"<div>"</div>"#);
        assert_eq!(outer(p, r#"text="\"""#).await, r#"<div>"</div>"#);
        assert_eq!(outer(p, r#"text="'""#).await, "<div>'</div>");
        assert_eq!(outer(p, r#"text="\x""#).await, "<div>x</div>");
        assert_eq!(outer(p, r"text='\x'").await, "<div>x</div>");
        assert_eq!(outer(p, r"text='\\'").await, r"<div>\</div>");
        assert_eq!(outer(p, r#"text="\\""#).await, r"<div>\</div>");
        assert_eq!(outer(p, r#"text=""#).await, r#"<div>"</div>"#);
        assert_eq!(outer(p, "text='").await, "<div>'</div>");
        assert_eq!(outer(p, r#""x""#).await, "<div>x</div>");
        assert_eq!(outer(p, "'x'").await, "<div>x</div>");
        assert!(p.query_selector("\"").await.is_err());
        assert!(p.query_selector("'").await.is_err());

        p.set_content(r#"<div> ' </div><div> " </div>"#)
            .await
            .unwrap();
        assert_eq!(outer(p, r#"text=""#).await, r#"<div> " </div>"#);
        assert_eq!(outer(p, "text='").await, "<div> ' </div>");

        p.set_content("<div>Hi''&gt;&gt;foo=bar</div>")
            .await
            .unwrap();
        assert_eq!(
            outer(p, r#"text="Hi''>>foo=bar""#).await,
            "<div>Hi''&gt;&gt;foo=bar</div>"
        );
        p.set_content(r#"<div>Hi'"&gt;&gt;foo=bar</div>"#)
            .await
            .unwrap();
        assert_eq!(
            outer(p, r#"text="Hi'\">>foo=bar""#).await,
            r#"<div>Hi'"&gt;&gt;foo=bar</div>"#
        );

        p.set_content("<div>Hi&gt;&gt;<span></span></div>")
            .await
            .unwrap();
        assert_eq!(outer(p, r#"text="Hi>>">>span"#).await, "<span></span>");
        assert_eq!(outer(p, r"text=/Hi\>\>/ >> span").await, "<span></span>");

        p.set_content("<div>a<br>b</div><div>a</div>")
            .await
            .unwrap();
        assert_eq!(outer(p, "text=a").await, "<div>a<br>b</div>");
        assert_eq!(outer(p, "text=b").await, "<div>a<br>b</div>");
        assert_eq!(outer(p, "text=ab").await, "<div>a<br>b</div>");
        assert!(!exists(p, "text=abc").await);
        assert_eq!(count(p, "text=a").await, 2);
        assert_eq!(count(p, "text=b").await, 1);
        assert_eq!(count(p, "text=ab").await, 1);
        assert_eq!(count(p, "text=abc").await, 0);

        p.set_content("<div></div><span></span>").await.unwrap();
        for sel in ["div", "span"] {
            p.eval_on_selector::<()>(
                sel,
                "e => { e.appendChild(document.createTextNode('hello')); e.appendChild(document.createTextNode('world')); }",
            )
            .await
            .unwrap();
        }
        assert_eq!(outer(p, "text=lowo").await, "<div>helloworld</div>");
        assert_eq!(
            eval_all::<String>(p, "text=lowo", "els => els.map(e => e.outerHTML).join('')").await,
            "<div>helloworld</div><span>helloworld</span>"
        );

        p.set_content("<span>Sign&nbsp;in</span><span>Hello\n \nworld</span>")
            .await
            .unwrap();
        assert_eq!(outer(p, "text=Sign in").await, "<span>Sign&nbsp;in</span>");
        assert_eq!(
            p.query_selector_all("text=Sign \tin").await.unwrap().len(),
            1
        );
        assert_eq!(
            p.query_selector_all(r#"text="Sign in""#)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(outer(p, "text=lo wo").await, "<span>Hello\n \nworld</span>");
        assert_eq!(
            outer(p, r#"text="Hello world""#).await,
            "<span>Hello\n \nworld</span>"
        );
        assert!(!exists(p, r#"text="lo wo""#).await);
        assert_eq!(p.query_selector_all("text=lo \nwo").await.unwrap().len(), 1);
        assert_eq!(
            p.query_selector_all("text=\"lo \nwo\"")
                .await
                .unwrap()
                .len(),
            0
        );

        p.set_content("<div>let's<span>hello</span></div>")
            .await
            .unwrap();
        assert_eq!(
            outer(p, "text=/let's/i >> span").await,
            "<span>hello</span>"
        );
        assert_eq!(
            outer(p, r"text=/let\'s/i >> span").await,
            "<span>hello</span>"
        );
    });
}

#[test]
fn should_work_with_text() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<div>yo</div><div>ya</div><div>\nHELLO   \n world  </div>")
            .await
            .unwrap();
        let hello = "<div>\nHELLO   \n world  </div>";
        assert_eq!(outer(p, r#":text("ya")"#).await, "<div>ya</div>");
        assert_eq!(outer(p, r#":text-is("ya")"#).await, "<div>ya</div>");
        assert_eq!(outer(p, r#":text("y")"#).await, "<div>yo</div>");
        assert!(!exists(p, r#":text-is("Y")"#).await);
        assert_eq!(outer(p, r#":text("hello world")"#).await, hello);
        assert_eq!(outer(p, r#":text-is("HELLO world")"#).await, hello);
        assert_eq!(outer(p, r#":text("lo wo")"#).await, hello);
        assert!(!exists(p, r#":text-is("lo wo")"#).await);
        assert_eq!(
            outer(p, r#":text-matches("^[ay]+$")"#).await,
            "<div>ya</div>"
        );
        assert_eq!(
            outer(p, r#":text-matches("y", "g")"#).await,
            "<div>yo</div>"
        );
        assert_eq!(
            outer(p, r#":text-matches("Y", "i")"#).await,
            "<div>yo</div>"
        );
        assert!(!exists(p, r#":text-matches("^y$")"#).await);

        for selector in [r#":text("foo", "bar")"#, ":text(foo > bar)"] {
            let err = p.query_selector(selector).await.unwrap_err().to_string();
            assert!(
                err.contains(r#""text" engine expects a single string"#),
                "{err}"
            );
        }
    });
}

#[test]
fn should_support_empty_string() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<div></div><div>ya</div><div>\nHELLO   \n world  </div>")
            .await
            .unwrap();
        assert_eq!(outer(p, r#"div:text-is("")"#).await, "<div></div>");
        assert_eq!(count(p, r#"div:text-is("")"#).await, 1);
        assert_eq!(outer(p, r#"div:text("")"#).await, "<div></div>");
        assert_eq!(count(p, r#"div:text("")"#).await, 3);
        assert_eq!(outer(p, r#"div >> text="""#).await, "<div></div>");
        assert_eq!(count(p, r#"div >> text="""#).await, 1);
        assert_eq!(outer(p, "div >> text=/^$/").await, "<div></div>");
        assert_eq!(count(p, "div >> text=/^$/").await, 1);
        assert_eq!(outer(p, r#"div:text-matches("")"#).await, "<div></div>");
        assert_eq!(count(p, r#"div:text-matches("")"#).await, 3);
    });
}

#[test]
fn should_work_across_nodes() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"<div id=target1>Hello<i>,</i> <span id=target2>world</span><b>!</b></div>"#,
        )
        .await
        .unwrap();
        assert_eq!(id(p, r#":text("Hello, world!")"#).await, "target1");
        assert_eq!(id(p, r#":text("Hello")"#).await, "target1");
        assert_eq!(id(p, r#":text("world")"#).await, "target2");
        assert_eq!(count(p, r#":text("world")"#).await, 1);
        assert!(!exists(p, r#":text("hello world")"#).await);
        assert!(!exists(p, r#"div:text("world")"#).await);
        assert_eq!(id(p, "text=Hello, world!").await, "target1");
        assert_eq!(id(p, "text=Hello").await, "target1");
        assert_eq!(id(p, "text=world").await, "target2");
        assert_eq!(count(p, "text=world").await, 1);
        assert!(!exists(p, "text=hello world").await);

        assert!(!exists(p, r#":text-is("Hello, world!")"#).await);
        assert_eq!(id(p, r#":text-is("Hello")"#).await, "target1");
        assert_eq!(id(p, r#":text-is("world")"#).await, "target2");
        assert_eq!(count(p, r#":text-is("world")"#).await, 1);
        assert!(!exists(p, r#"text="Hello, world!""#).await);
        assert_eq!(id(p, r#"text="Hello""#).await, "target1");
        assert_eq!(id(p, r#"text="world""#).await, "target2");
        assert_eq!(count(p, r#"text="world""#).await, 1);

        assert_eq!(
            eval::<String>(p, r#":text-matches(".*")"#, "e => e.nodeName").await,
            "I"
        );
        assert_eq!(id(p, r#":text-matches("world?")"#).await, "target2");
        assert_eq!(count(p, r#":text-matches("world")"#).await, 1);
        assert!(!exists(p, r#"div:text(".*")"#).await);
        assert_eq!(eval::<String>(p, "text=/.*/", "e => e.nodeName").await, "I");
        assert_eq!(id(p, "text=/world?/").await, "target2");
        assert_eq!(count(p, "text=/world/").await, 1);
    });
}

#[test]
fn text_is_should_ignore_comments() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<div id=me>hel<!-- comment -->lo\n  <!-- comment -->\n  world</div>")
            .await
            .unwrap();
        assert_eq!(id(p, r#":text-is("hello world")"#).await, "me");
        assert_eq!(
            p.locator_with("div", LocatorOptions::new().has_text("hello world"))
                .get_attribute("id")
                .await
                .unwrap()
                .as_deref(),
            Some("me")
        );
        assert_eq!(
            p.get_by_text_with("hello world", GetByOptions::exact())
                .get_attribute("id")
                .await
                .unwrap()
                .as_deref(),
            Some("me")
        );
    });
}

#[test]
fn should_work_with_text_nodes_in_quoted_mode() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<div id=target1>Hello<span id=target2>wo  rld  </span>  Hi again  </div>")
            .await
            .unwrap();
        assert_eq!(id(p, r#"text="Hello""#).await, "target1");
        assert_eq!(id(p, r#"text="Hi again""#).await, "target1");
        assert_eq!(id(p, r#"text="wo rld""#).await, "target2");
        for selector in [
            r#"text="Hellowo rld Hi again""#,
            r#"text="Hellowo""#,
            r#"text="Hellowo rld""#,
            r#"text="wo rld Hi ag""#,
            r#"text="again""#,
            r#"text="hi again""#,
        ] {
            assert!(!exists(p, selector).await, "{selector}");
        }
        assert_eq!(id(p, "text=hi again").await, "target1");
    });
}

#[test]
fn should_clear_caches() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<div id=target1>text</div><div id=target2>text</div>")
            .await
            .unwrap();
        let div = p.query_selector("#target1").await.unwrap().unwrap();
        let set = |text: &'static str| {
            let div = div.clone();
            async move {
                div.evaluate::<()>("(div, text) => div.textContent = text")
                    .arg(text)
                    .await
                    .unwrap();
            }
        };
        set("text").await;
        assert_eq!(id(p, "text=text").await, "target1");
        set("foo").await;
        assert_eq!(id(p, "text=text").await, "target2");
        set("text").await;
        assert_eq!(id(p, r#":text("text")"#).await, "target1");
        set("foo").await;
        assert_eq!(id(p, r#":text("text")"#).await, "target2");
        set("text").await;
        assert_eq!(count(p, "text=text").await, 2);
        set("foo").await;
        assert_eq!(count(p, "text=text").await, 1);
        set("text").await;
        assert_eq!(count(p, r#":text("text")"#).await, 2);
        set("foo").await;
        assert_eq!(count(p, r#":text("text")"#).await, 1);
    });
}

#[test]
fn should_work_with_has_text() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <input id=input2>
    <div id=div1>
      <span>  Find me  </span>
      or
      <wrap><span id=span2>maybe me  </span></wrap>
      <div><input id=input1></div>
    </div>
  "#,
        )
        .await
        .unwrap();
        assert_eq!(
            eval::<String>(p, r#":has-text("find me")"#, "e => e.tagName").await,
            "HTML"
        );
        assert_eq!(
            outer(p, r#"span:has-text("find me")"#).await,
            "<span>  Find me  </span>"
        );
        assert_eq!(id(p, r#"div:has-text("find me")"#).await, "div1");
        assert_eq!(id(p, r#"div:has-text("find me") input"#).await, "input1");
        assert_eq!(id(p, r#":has-text("find me") input"#).await, "input2");
        assert_eq!(
            id(p, r#"div:has-text("find me or maybe me")"#).await,
            "div1"
        );
        assert!(!exists(p, r#"div:has-text("find noone")"#).await);
        assert_eq!(
            eval_all::<String>(
                p,
                r#":is(div,span):has-text("maybe")"#,
                "els => els.map(e => e.id).join(';')"
            )
            .await,
            "div1;span2"
        );
        assert_eq!(
            eval::<String>(
                p,
                r#"div:has-text("find me") :has-text("maybe me")"#,
                "e => e.tagName"
            )
            .await,
            "WRAP"
        );
        assert_eq!(
            id(p, r#"div:has-text("find me") span:has-text("maybe me")"#).await,
            "span2"
        );

        p.set_content("<div id=me>hello\n  wo\"r>>ld</div>")
            .await
            .unwrap();
        assert_eq!(id(p, r#"div:has-text("hello wo\"r>>ld")"#).await, "me");
        assert_eq!(id(p, r#"div:has-text("hello\a wo\"r>>ld")"#).await, "me");
        assert_eq!(
            p.locator_with("div", LocatorOptions::new().has_text("hello\nwo\"r>>ld"))
                .get_attribute("id")
                .await
                .unwrap()
                .as_deref(),
            Some("me")
        );

        for selector in [r#":has-text("foo", "bar")"#, ":has-text(foo > bar)"] {
            let err = p.query_selector(selector).await.unwrap_err().to_string();
            assert!(
                err.contains(r#""has-text" engine expects a single string"#),
                "{err}"
            );
        }
    });
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
      e.id = 'id' + eid;
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
            r#":has-text("id18")"#,
            r#":has-text("id12345")"#,
            r#":has-text("id")"#,
            r#":text("id18")"#,
            r#":text("id12345")"#,
            r#":text("id")"#,
            r#":text-matches("id12345", "i")"#,
            "text=id18",
            "text=id12345",
            "text=id",
            "#id18",
            "#id12345",
            "*",
        ] {
            count(p, selector).await;
        }
    });
}

#[test]
fn should_be_case_sensitive_if_quotes_are_specified() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<div>yo</div><div>ya</div><div>\nye  </div>")
            .await
            .unwrap();
        assert_eq!(outer(p, "text=yA").await, "<div>ya</div>");
        assert!(!exists(p, r#"text="yA""#).await);
        assert!(!exists(p, r#"text= "ya""#).await);
    });
}

#[test]
fn should_search_for_a_substring_without_quotes() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<div>textwithsubstring</div>").await.unwrap();
        assert_eq!(outer(p, "text=with").await, "<div>textwithsubstring</div>");
        assert!(!exists(p, r#"text="with""#).await);
    });
}

#[test]
fn should_skip_head_script_and_style() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <head>
      <title>title</title>
      <script>var script</script>
      <style>.style {}</style>
    </head>
    <body>
      <script>var script</script>
      <style>.style {}</style>
      <div>title script style</div>
    </body>"#,
        )
        .await
        .unwrap();
        let mut roots = Vec::new();
        for sel in ["head", "title", "body script", "body style"] {
            roots.push(p.query_selector(sel).await.unwrap().unwrap());
        }
        for text in ["title", "script", "style"] {
            let selector = format!("text={text}");
            assert_eq!(eval::<String>(p, &selector, "e => e.nodeName").await, "DIV");
            assert_eq!(
                eval_all::<String>(p, &selector, "els => els.map(e => e.nodeName).join('|')").await,
                "DIV"
            );
            for root in &roots {
                assert!(root.query_selector(&selector).await.unwrap().is_none());
                let n: usize = root
                    .eval_on_selector_all(selector.clone(), "els => els.length")
                    .await
                    .unwrap();
                assert_eq!(n, 0);
            }
        }
    });
}

#[test]
fn should_match_input_type_button_submit_reset() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(r#"<input type="submit" value="hello"><input type="button" value="world"><input type="reset" value="clear">"#)
            .await
            .unwrap();
        assert_eq!(
            outer(p, "text=hello").await,
            r#"<input type="submit" value="hello">"#
        );
        assert_eq!(
            outer(p, "text=world").await,
            r#"<input type="button" value="world">"#
        );
        assert_eq!(
            outer(p, "text=clear").await,
            r#"<input type="reset" value="clear">"#
        );
    });
}

#[test]
fn should_work_for_open_shadow_roots() {
    run(|t| async move {
        let p = &t.page;
        p.goto(deep_shadow(&t.server.prefix)).await.unwrap();
        let text = |sel: &'static str| eval::<String>(p, sel, "e => e.textContent");
        assert_eq!(text("text=root1").await, "Hello from root1");
        assert_eq!(text("text=root2").await, "Hello from root2");
        assert_eq!(text("text=root3").await, "Hello from root3");
        assert_eq!(text("#root1 >> text=from root3").await, "Hello from root3");
        assert_eq!(text("#target >> text=from root2").await, "Hello from root2");
        assert!(!exists(p, "text:light=root1").await);
        assert!(!exists(p, "text:light=root2").await);
        assert!(!exists(p, "text:light=root3").await);
    });
}

const LIGHT_AND_SHADOW: &str = r#"() => {
    const div = document.createElement('div');
    document.body.appendChild(div);
    div.attachShadow({ mode: 'open' });
    const shadowSpan = document.createElement('span');
    shadowSpan.textContent = 'Hello from shadow';
    div.shadowRoot.appendChild(shadowSpan);
    const lightSpan = document.createElement('span');
    lightSpan.textContent = 'Hello from light';
    div.appendChild(lightSpan);
  }"#;

#[test]
fn should_prioritize_light_dom_over_shadow_dom_in_the_same_parent() {
    run(|t| async move {
        let p = &t.page;
        p.evaluate::<()>(LIGHT_AND_SHADOW).await.unwrap();
        assert_eq!(
            eval::<String>(p, "div >> text=Hello", "e => e.textContent").await,
            "Hello from light"
        );
    });
}

#[test]
fn should_wait_for_selector_with_distributed_elements() {
    run(|t| async move {
        let p = t.page.clone();
        let waiter = tokio::spawn({
            let p = p.clone();
            async move { p.wait_for_selector("div >> text=Hello").await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        p.evaluate::<()>(
            r#"() => {
    const div = document.createElement('div');
    document.body.appendChild(div);
    div.attachShadow({ mode: 'open' });
    const shadowSpan = document.createElement('span');
    shadowSpan.textContent = 'Hello from shadow';
    div.shadowRoot.appendChild(shadowSpan);
    div.shadowRoot.appendChild(document.createElement('slot'));
    const lightSpan = document.createElement('span');
    lightSpan.textContent = 'Hello from light';
    div.appendChild(lightSpan);
  }"#,
        )
        .await
        .unwrap();
        let handle = waiter.await.unwrap().unwrap().unwrap();
        assert_eq!(
            handle.text_content().await.unwrap().as_deref(),
            Some("Hello from light")
        );
    });
}

#[test]
fn should_match_root_after() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<section>test</section>").await.unwrap();
        assert!(exists(p, "css=section >> text=test").await);
        assert!(exists(p, "text=test >> text=test").await);
    });
}

#[test]
fn should_match_root_after_with_star() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            "<button> hello world </button> <button> hellow <span> world </span> </button>",
        )
        .await
        .unwrap();
        assert_eq!(count(p, "*css=button >> text=hello >> text=world").await, 2);
    });
}

#[test]
fn should_work_with_leading_and_trailing_spaces() {
    run(|t| async move {
        let p = &t.page;
        p.set_content("<button> Add widget </button>")
            .await
            .unwrap();
        expect_visible(&p.locator("text=Add widget"), true).await;
        expect_visible(&p.locator("text= Add widget "), true).await;
    });
}

#[test]
fn should_work_with_unpaired_quotes_when_not_at_the_start() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(
            r#"
    <div>hello"world<span>yay</span></div>
    <div>hello'world<span>nay</span></div>
    <div>hello`world<span>oh</span></div>
    <div>hello`world<span>oh2</span></div>
  "#,
        )
        .await
        .unwrap();
        for selector in [
            r#"text=lo" >> span"#,
            r#"  text=lo" >> span"#,
            r#"text  =lo" >> span"#,
            r#"text=  lo" >> span"#,
            r#" text = lo" >> span"#,
            r#"text=o"wor >> span"#,
        ] {
            assert_eq!(outer(p, selector).await, "<span>yay</span>", "{selector}");
        }
        assert_eq!(outer(p, "text=lo'wor >> span").await, "<span>nay</span>");
        assert_eq!(outer(p, "text=o' >> span").await, "<span>nay</span>");
        assert_eq!(outer(p, "text=ello`wor >> span").await, "<span>oh</span>");
        expect_text(&p.locator("text=ello`wor").locator("span").first(), "oh").await;
        expect_text(&p.locator("text=ello`wor").locator("span").nth(1), "oh2").await;
        assert!(!exists(p, "text='wor >> span").await);
        assert!(!exists(p, r#"text=" >> span"#).await);
        assert!(!exists(p, "text=` >> span").await);
    });
}

#[test]
fn should_work_with_paired_quotes_in_the_middle_of_selector() {
    run(|t| async move {
        let p = &t.page;
        p.set_content(r#"<div>pattern "^-?\d+$"</div>"#)
            .await
            .unwrap();
        for selector in [
            r#"div >> text=pattern "^-?\d+$"#,
            r#"div >> text=pattern "^-?\d+$""#,
            r#"div >> text='pattern "^-?\\d+$"'"#,
        ] {
            expect_visible(&p.locator(selector), true).await;
        }
    });
}

#[test]
fn has_text_and_internal_text_should_match_full_node_text_in_strict_mode() {
    run(|t| async move {
        let p = &t.page;
        let has = |re: &str| p.locator_with("div", LocatorOptions::new().has_text(regex(re, "")));
        p.set_content("<div id=div1>hello<span>world</span></div>\n<div id=div2>hello</div>")
            .await
            .unwrap();
        expect_attr(
            &p.get_by_text_with("helloworld", GetByOptions::exact()),
            "id",
            "div1",
        )
        .await;
        expect_attr(
            &p.get_by_text_with("hello", GetByOptions::exact()),
            "id",
            "div2",
        )
        .await;
        expect_attr(&has("^helloworld$"), "id", "div1").await;
        expect_attr(&has("^hello$"), "id", "div2").await;

        p.set_content("<div id=div1><span id=span1>hello</span>world</div>\n<div id=div2><span id=span2>hello</span></div>")
            .await
            .unwrap();
        expect_attr(
            &p.get_by_text_with("helloworld", GetByOptions::exact()),
            "id",
            "div1",
        )
        .await;
        let ids: Vec<String> = p
            .get_by_text_with("hello", GetByOptions::exact())
            .evaluate_all("els => els.map(e => e.id)")
            .await
            .unwrap();
        assert_eq!(ids, vec!["span1", "span2"]);
        expect_attr(&has("^helloworld$"), "id", "div1").await;
        expect_attr(&has("^hello$"), "id", "div2").await;
    });
}
