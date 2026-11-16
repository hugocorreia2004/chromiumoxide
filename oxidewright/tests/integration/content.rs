//! Ports of `page-set-content.spec.ts` and `page-add-init-script.spec.ts`.
//!
//! Skipped: PA-025 `includeShadow` option (not implemented yet), PA-034
//! (racy content during navigation), PA-036/037 (tamper with Playwright's
//! utility world internals), IS-004 (option validation of the JS API),
//! IS-007 (`setCSP`/`addScriptTag`), IS-011 (`console` event).

use std::future::IntoFuture;

use oxidewright::LoadState;

use crate::common::*;

const EXPECTED_OUTPUT: &str = "<html><head></head><body><div>hello</div></body></html>";

// ---- page-set-content.spec.ts ----

#[test]
fn set_content_should_work() {
    run(|t| async move {
        t.page.set_content("<div>hello</div>").await.unwrap();
        assert_eq!(t.page.content().await.unwrap(), EXPECTED_OUTPUT);
    });
}

#[test]
fn set_content_should_work_with_domcontentloaded_and_commit() {
    run(|t| async move {
        t.page
            .set_content("<div>hello</div>")
            .wait_until(LoadState::DomContentLoaded)
            .await
            .unwrap();
        assert_eq!(t.page.content().await.unwrap(), EXPECTED_OUTPUT);
        t.page
            .set_content("<div>hello</div>")
            .wait_until(LoadState::Commit)
            .await
            .unwrap();
        assert_eq!(t.page.content().await.unwrap(), EXPECTED_OUTPUT);
    });
}

#[test]
fn set_content_should_work_with_doctype() {
    run(|t| async move {
        let doctype = "<!DOCTYPE html>";
        t.page
            .set_content(format!("{doctype}<div>hello</div>"))
            .await
            .unwrap();
        assert_eq!(
            t.page.content().await.unwrap(),
            format!("{doctype}{EXPECTED_OUTPUT}")
        );
        let doctype = "<!DOCTYPE html PUBLIC \"-//W3C//DTD HTML 4.01//EN\" \"http://www.w3.org/TR/html4/strict.dtd\">";
        t.page
            .set_content(format!("{doctype}<div>hello</div>"))
            .await
            .unwrap();
        assert_eq!(
            t.page.content().await.unwrap(),
            format!("{doctype}{EXPECTED_OUTPUT}")
        );
    });
}

#[test]
fn set_content_should_respect_timeout() {
    run(|t| async move {
        t.server.hang("/img.png");
        let r = t
            .page
            .set_content(format!("<img src=\"{}/img.png\"></img>", t.server.prefix))
            .timeout(1)
            .await;
        assert!(r.unwrap_err().is_timeout());
    });
}

#[test]
fn set_content_should_respect_default_navigation_timeout() {
    run(|t| async move {
        t.page.set_default_navigation_timeout(1);
        t.server.hang("/img.png");
        let r = t
            .page
            .set_content(format!("<img src=\"{}/img.png\"></img>", t.server.prefix))
            .await;
        let e = assert_err_contains(r, "page.setContent: Timeout 1ms exceeded.");
        assert!(e.is_timeout());
    });
}

#[test]
fn set_content_should_await_resources_to_load() {
    run(|t| async move {
        let gate = t.server.stall("/img.png", "image/png", "", 200);
        let content = tokio::spawn(
            t.page
                .set_content(format!("<img src=\"{}/img.png\"></img>", t.server.prefix))
                .into_future(),
        );
        t.server.wait_for_request("/img.png").await;
        assert!(!content.is_finished());
        gate.release();
        content.await.unwrap().unwrap();
    });
}

#[test]
fn set_content_should_work_fast_enough() {
    run(|t| async move {
        for _ in 0..20 {
            t.page.set_content("<div>yo</div>").await.unwrap();
        }
    });
}

#[test]
fn set_content_should_work_with_tricky_content_accents_emojis_and_newlines() {
    run(|t| async move {
        for (html, text) in [
            ("<div>hello world</div>\x7F", "hello world"),
            ("<div>aberración</div>", "aberración"),
            ("<div>🐥</div>", "🐥"),
            ("<div>\n</div>", "\n"),
        ] {
            t.page.set_content(html).await.unwrap();
            let v: String = t
                .page
                .evaluate("() => document.querySelector('div').textContent")
                .await
                .unwrap();
            assert_eq!(v, text);
        }
    });
}

#[test]
fn set_content_should_return_empty_content_there_is_no_iframe_src() {
    run(|t| async move {
        t.page
            .set_content("<iframe src=\"javascript:console.log(1)\"></iframe>")
            .await
            .unwrap();
        assert_eq!(t.page.frames().len(), 2);
        assert_eq!(
            t.page.frames()[1].content().await.unwrap(),
            "<html><head></head><body></body></html>"
        );
    });
}

#[test]
fn set_content_on_a_child_frame() {
    run(|t| async move {
        t.page
            .goto(format!("{}/frames/one-frame.html", t.server.prefix))
            .await
            .unwrap();
        let frame = t.page.frames()[1].clone();
        frame.set_content("<div>in frame</div>").await.unwrap();
        assert_eq!(
            frame.content().await.unwrap(),
            "<html><head></head><body><div>in frame</div></body></html>"
        );
    });
}

// ---- page-add-init-script.spec.ts ----

#[test]
fn add_init_script_should_evaluate_before_anything_else_on_the_page() {
    run(|t| async move {
        t.page
            .add_init_script("() => { window['injected'] = 123; }")
            .arg(())
            .await
            .unwrap();
        t.page
            .goto(format!("{}/tamperable.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(
            t.page
                .evaluate::<i32>("() => window['result']")
                .await
                .unwrap(),
            123
        );
    });
}

#[test]
fn add_init_script_should_work_with_a_path() {
    run(|t| async move {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/assets/injectedfile.js");
        t.page.add_init_script_path(path).await.unwrap();
        t.page
            .goto(format!("{}/tamperable.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(
            t.page
                .evaluate::<i32>("() => window['result']")
                .await
                .unwrap(),
            123
        );
    });
}

#[test]
fn add_init_script_should_work_with_content() {
    run(|t| async move {
        t.page
            .add_init_script("window[\"injected\"] = 123")
            .await
            .unwrap();
        t.page
            .goto(format!("{}/tamperable.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(
            t.page
                .evaluate::<i32>("() => window['result']")
                .await
                .unwrap(),
            123
        );
    });
}

#[test]
fn add_init_script_should_work_with_trailing_comments() {
    run(|t| async move {
        t.page.add_init_script("// comment").await.unwrap();
        t.page.add_init_script("window.secret = 42;").await.unwrap();
        t.page.goto("data:text/html,<html></html>").await.unwrap();
        assert_eq!(t.page.evaluate::<i32>("secret").await.unwrap(), 42);
    });
}

#[test]
fn add_init_script_should_support_multiple_scripts() {
    run(|t| async move {
        t.page
            .add_init_script("window['script1'] = 1")
            .await
            .unwrap();
        t.page
            .add_init_script("window['script2'] = 2")
            .await
            .unwrap();
        t.page
            .goto(format!("{}/tamperable.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(
            t.page
                .evaluate::<i32>("() => window['script1']")
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            t.page
                .evaluate::<i32>("() => window['script2']")
                .await
                .unwrap(),
            2
        );
    });
}

#[test]
fn add_init_script_should_work_after_a_cross_origin_navigation() {
    run(|t| async move {
        t.page
            .goto(format!("{}/empty.html", t.server.cross_process_prefix))
            .await
            .unwrap();
        t.page
            .add_init_script("window['injected'] = 123")
            .await
            .unwrap();
        t.page
            .goto(format!("{}/tamperable.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(
            t.page
                .evaluate::<i32>("() => window['result']")
                .await
                .unwrap(),
            123
        );
    });
}

#[test]
fn add_init_script_should_remove_one_of_multiple_init_scripts_after_dispose() {
    run(|t| async move {
        let script1 = t
            .page
            .add_init_script("window['script1'] = 1")
            .await
            .unwrap();
        t.page
            .add_init_script("window['script2'] = 2")
            .await
            .unwrap();
        t.page
            .goto(format!("{}/tamperable.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(
            t.page
                .evaluate::<i32>("() => window['script1']")
                .await
                .unwrap(),
            1
        );
        script1.dispose().await.unwrap();
        t.page
            .goto(format!("{}/tamperable.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(
            t.page
                .evaluate::<Option<i32>>("() => window['script1']")
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            t.page
                .evaluate::<i32>("() => window['script2']")
                .await
                .unwrap(),
            2
        );
    });
}

#[test]
fn add_init_script_with_argument() {
    run(|t| async move {
        t.page
            .add_init_script("value => { window['injected'] = value.n; }")
            .arg(serde_json::json!({ "n": 7 }))
            .await
            .unwrap();
        t.page
            .goto(format!("{}/tamperable.html", t.server.prefix))
            .await
            .unwrap();
        assert_eq!(
            t.page
                .evaluate::<i32>("() => window['result']")
                .await
                .unwrap(),
            7
        );
    });
}

#[test]
fn add_init_script_runs_in_iframes_and_context_scripts_apply_to_new_pages() {
    run(|t| async move {
        t.context
            .add_init_script("window['fromContext'] = location.pathname")
            .await
            .unwrap();
        let page = t.context.new_page().await.unwrap();
        page.goto(format!("{}/frames/one-frame.html", t.server.prefix))
            .await
            .unwrap();
        let frames = page.frames();
        assert_eq!(
            frames[0]
                .evaluate::<String>("() => window['fromContext']")
                .await
                .unwrap(),
            "/frames/one-frame.html"
        );
        assert_eq!(
            frames[1]
                .evaluate::<String>("() => window['fromContext']")
                .await
                .unwrap(),
            "/frames/frame.html"
        );
        // Existing pages get it too.
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(
            t.page
                .evaluate::<String>("() => window['fromContext']")
                .await
                .unwrap(),
            "/empty.html"
        );
    });
}
