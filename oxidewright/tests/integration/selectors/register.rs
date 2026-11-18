//! Custom selector engines, ported from Playwright's
//! `tests/library/selectors-register.spec.ts`. The registry is global to the
//! process (like `playwright.selectors`), so every test uses its own names.

use crate::common::*;

const TAG_ENGINE: &str = "() => ({
  query(root, selector) { return root.querySelector(selector); },
  queryAll(root, selector) { return Array.from(root.querySelectorAll(selector)); }
})";

const DUMMY_ENGINE: &str = "() => ({
  query(root, selector) { return window['__answer']; },
  queryAll(root, selector) { return window['__answer'] ? [window['__answer'], document.body, document.documentElement] : []; }
})";

#[test]
fn se_160_should_work() {
    run(|t| async move {
        oxidewright::selectors()
            .register("se160tag", TAG_ENGINE)
            .await
            .unwrap();
        let context = t.browser.new_context().await.unwrap();
        oxidewright::selectors()
            .register("se160tag2", format!("({TAG_ENGINE})()"))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.set_content("<div><span></span></div><div></div>")
            .await
            .unwrap();
        for engine in ["se160tag", "se160tag2"] {
            let name: String = page
                .eval_on_selector(format!("{engine}=DIV"), "e => e.nodeName")
                .await
                .unwrap();
            assert_eq!(name, "DIV");
            let name: String = page
                .eval_on_selector(format!("{engine}=SPAN"), "e => e.nodeName")
                .await
                .unwrap();
            assert_eq!(name, "SPAN");
            let count: usize = page
                .eval_on_selector_all(format!("{engine}=DIV"), "es => es.length")
                .await
                .unwrap();
            assert_eq!(count, 2);
        }
        // Selector names are case-sensitive.
        let error = page.query_selector("se160TAG=DIV").await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Unknown engine \"se160TAG\" while parsing selector se160TAG=DIV"),
            "{error}"
        );
        context.close().await.unwrap();
    });
}

#[test]
fn se_162_should_work_with_path() {
    run(|t| async move {
        let path = std::env::temp_dir().join(format!("se162-{}.js", std::process::id()));
        std::fs::write(
            &path,
            "({ query(root, selector) { return root.querySelector('section'); }, queryAll(root, selector) { return Array.from(root.querySelectorAll('section')); } })",
        )
        .unwrap();
        oxidewright::selectors()
            .register_path("se162foo", &path)
            .await
            .unwrap();
        t.page.set_content("<section></section>").await.unwrap();
        let name: String = t
            .page
            .eval_on_selector("se162foo=whatever", "e => e.nodeName")
            .await
            .unwrap();
        assert_eq!(name, "SECTION");
        let _ = std::fs::remove_file(&path);
    });
}

#[test]
fn se_163_should_work_in_main_and_isolated_world() {
    run(|t| async move {
        oxidewright::selectors()
            .register("se163main", DUMMY_ENGINE)
            .await
            .unwrap();
        oxidewright::selectors()
            .register("se163isolated", DUMMY_ENGINE)
            .content_script(true)
            .await
            .unwrap();
        let page = t.page.clone();
        page.set_content("<div><span><section></section></span></div>")
            .await
            .unwrap();
        page.evaluate::<()>("() => { window['__answer'] = document.querySelector('span'); }")
            .await
            .unwrap();
        async fn name(page: &oxidewright::Page, selector: &str) -> String {
            page.eval_on_selector(selector, "e => e.nodeName")
                .await
                .unwrap()
        }
        // Works in main if asked.
        assert_eq!(name(&page, "se163main=ignored").await, "SPAN");
        assert_eq!(name(&page, "css=div >> se163main=ignored").await, "SPAN");
        let defined: bool = page
            .eval_on_selector_all(
                "se163main=ignored",
                "es => window['__answer'] !== undefined",
            )
            .await
            .unwrap();
        assert!(defined);
        let count: usize = page
            .eval_on_selector_all("se163main=ignored", "es => es.filter(e => e).length")
            .await
            .unwrap();
        assert_eq!(count, 3);
        // Works in isolated by default.
        assert!(
            page.query_selector("se163isolated=ignored")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            page.query_selector("css=div >> se163isolated=ignored")
                .await
                .unwrap()
                .is_none()
        );
        // At least one engine in main forces all to be in main.
        assert_eq!(
            name(&page, "se163main=ignored >> se163isolated=ignored").await,
            "SPAN"
        );
        assert_eq!(
            name(&page, "se163isolated=ignored >> se163main=ignored").await,
            "SPAN"
        );
        // Can be chained to css.
        assert_eq!(
            name(&page, "se163main=ignored >> css=section").await,
            "SECTION"
        );
    });
}

#[test]
fn se_164_should_handle_errors() {
    run(|t| async move {
        let error = t
            .page
            .query_selector("se164never=ignored")
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains(
                "Unknown engine \"se164never\" while parsing selector se164never=ignored"
            ),
            "{error}"
        );
        let error = oxidewright::selectors()
            .register("$", DUMMY_ENGINE)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains(
                "selectors.register: Selector engine name may only contain [a-zA-Z0-9_] characters"
            ),
            "{error}"
        );
        // Selector names are case-sensitive.
        oxidewright::selectors()
            .register("se164dummy", DUMMY_ENGINE)
            .await
            .unwrap();
        oxidewright::selectors()
            .register("se164duMMy", DUMMY_ENGINE)
            .await
            .unwrap();
        let error = oxidewright::selectors()
            .register("se164dummy", DUMMY_ENGINE)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains(
                "selectors.register: \"se164dummy\" selector engine has been already registered"
            ),
            "{error}"
        );
        let error = oxidewright::selectors()
            .register("css", DUMMY_ENGINE)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("selectors.register: \"css\" is a predefined selector engine"),
            "{error}"
        );
    });
}

#[test]
fn se_166_should_not_rely_on_engines_working_from_the_root() {
    run(|t| async move {
        oxidewright::selectors()
            .register(
                "__se166value",
                "() => ({
                  query(root, selector) { return root && root.value.includes(selector) ? root : undefined; },
                  queryAll(root, selector) { return root && root.value.includes(selector) ? [root] : []; },
                })",
            )
            .await
            .unwrap();
        t.page
            .set_content("<input id=input1 value=value1><input id=input2 value=value2>")
            .await
            .unwrap();
        let id: String = t
            .page
            .eval_on_selector("input >> __se166value=value2", "e => e.id")
            .await
            .unwrap();
        assert_eq!(id, "input2");
    });
}

#[test]
fn se_167_should_throw_a_nice_error_if_the_selector_returns_a_bad_value() {
    run(|t| async move {
        oxidewright::selectors()
            .register(
                "__se167fake",
                "() => ({
                  query(root, selector) { return [document.body]; },
                  queryAll(root, selector) { return [[document.body]]; },
                })",
            )
            .await
            .unwrap();
        let error = t
            .page
            .query_selector("__se167fake=value2")
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Expected a Node but got [object Array]"),
            "{error}"
        );
    });
}
