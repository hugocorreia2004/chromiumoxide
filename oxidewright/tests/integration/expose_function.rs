//! Port of `tests/page/page-expose-function.spec.ts` and
//! `tests/library/browsercontext-expose-function.spec.ts`, plus a stealth
//! check (no global other than the exposed name).
//!
//! Ignored: EF-007 and EF-026 (an init script calling the binding at
//! document start): like Patchright, `Runtime.addBinding` without
//! `Runtime.enable` only reaches a new document when its main-world context
//! is created, which happens after the init scripts ran.
//!
//! Skipped: EF-005 checks that the stack contains the test file name (a JS
//! stack, no Rust equivalent; the message is checked); EF-028 is `fixme` for
//! Chromium in Playwright.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::header;
use axum::response::Response;
use oxidewright::{BindingError, BindingSource, JsValue};
use serde_json::{Value, json};

use crate::common::*;

fn num(v: &JsValue) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

async fn poll<F: Fn() -> bool>(f: F) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("condition not met");
}

#[test]
fn ef_001_expose_binding_should_work() {
    run(|t| async move {
        let source: Arc<Mutex<Option<BindingSource>>> = Arc::default();
        let s = source.clone();
        t.page
            .expose_binding("add", move |src, args| {
                *s.lock().unwrap() = Some(src);
                async move { Ok::<_, BindingError>(num(&args[0]) + num(&args[1])) }
            })
            .await
            .unwrap();
        let result: f64 = t
            .page
            .evaluate("async function() { return window['add'](5, 6); }")
            .await
            .unwrap();
        let src = source.lock().unwrap().clone().unwrap();
        assert_eq!(src.context, Some(t.context.clone()));
        assert_eq!(src.page, t.page);
        assert_eq!(src.frame, t.page.main_frame());
        assert_eq!(result, 11.0);
    });
}

#[test]
fn ef_002_should_work() {
    run(|t| async move {
        t.page
            .expose_function("compute", |args| async move {
                Ok::<_, BindingError>(num(&args[0]) * num(&args[1]))
            })
            .await
            .unwrap();
        let result: f64 = t
            .page
            .evaluate("async function() { return await window['compute'](9, 4); }")
            .await
            .unwrap();
        assert_eq!(result, 36.0);
    });
}

#[test]
fn ef_003_should_dispose() {
    run(|t| async move {
        let binding = t
            .page
            .expose_function("compute", |args| async move {
                Ok::<_, BindingError>(num(&args[0]) * num(&args[1]))
            })
            .await
            .unwrap();
        let result: f64 = t
            .page
            .evaluate("async function() { return await window['compute'](9, 4); }")
            .await
            .unwrap();
        assert_eq!(result, 36.0);
        binding.dispose().await.unwrap();
        let r = t
            .page
            .evaluate::<f64>("async function() { return await window['compute'](9, 4); }")
            .await;
        assert_err_contains(r, "is not a function");
    });
}

#[test]
fn ef_004_should_work_with_handles_and_complex_objects() {
    run(|t| async move {
        let foo = t
            .page
            .evaluate_handle(
                "() => { window['fooValue'] = { bar: 2 }; return window['fooValue']; }",
            )
            .await
            .unwrap();
        t.page
            .expose_function("handle", move |_args| {
                let foo = foo.clone();
                async move { Ok::<_, BindingError>(vec![json_with_handle(foo)]) }
            })
            .await
            .unwrap();
        let equals: bool = t
            .page
            .evaluate(
                "async function() { const value = await window['handle'](); const [{ foo }] = value; return foo === window['fooValue']; }",
            )
            .await
            .unwrap();
        assert!(equals);
    });
}

fn json_with_handle(
    handle: oxidewright::JSHandle,
) -> std::collections::BTreeMap<String, oxidewright::JSHandle> {
    std::collections::BTreeMap::from([("foo".to_string(), handle)])
}

#[test]
fn ef_005_should_throw_exception_in_page_context() {
    run(|t| async move {
        t.page
            .expose_function("woof", |_args| async move {
                Err::<(), _>(BindingError::new("WOOF WOOF"))
            })
            .await
            .unwrap();
        let r: Value = t
            .page
            .evaluate(
                "async () => { try { await window['woof'](); } catch (e) { return { message: e.message, stack: e.stack }; } }",
            )
            .await
            .unwrap();
        assert_eq!(r["message"], "WOOF WOOF");
        assert!(r["stack"].as_str().unwrap().contains("WOOF WOOF"));
    });
}

#[test]
fn ef_006_should_support_throwing_null() {
    run(|t| async move {
        t.page
            .expose_function("woof", |_args| async move {
                Err::<(), _>(BindingError::value(JsValue::Null))
            })
            .await
            .unwrap();
        let thrown: JsValue = t
            .page
            .evaluate("async () => { try { await window['woof'](); } catch (e) { return e; } }")
            .await
            .unwrap();
        assert!(thrown.is_null());
    });
}

#[test]
#[ignore = "Patchright behaviour: without Runtime.enable the binding reaches a new document only when its main-world context is created, after the init scripts ran"]
fn ef_007_should_be_callable_from_inside_add_init_script() {
    run(|t| async move {
        let called = Arc::new(Mutex::new(false));
        let c = called.clone();
        t.page
            .expose_function("woof", move |_args| {
                *c.lock().unwrap() = true;
                async move { Ok::<_, BindingError>(()) }
            })
            .await
            .unwrap();
        t.page
            .add_init_script("() => window['woof']()")
            .arg(())
            .await
            .unwrap();
        t.page.reload().await.unwrap();
        poll(|| *called.lock().unwrap()).await;
    });
}

async fn expose_compute(page: &oxidewright::Page) {
    page.expose_function("compute", |args| async move {
        Ok::<_, BindingError>(num(&args[0]) * num(&args[1]))
    })
    .await
    .unwrap();
}

#[test]
fn ef_008_should_survive_navigation() {
    run(|t| async move {
        expose_compute(&t.page).await;
        t.page.goto(&t.server.empty_page).await.unwrap();
        let result: f64 = t
            .page
            .evaluate("async function() { return await window['compute'](9, 4); }")
            .await
            .unwrap();
        assert_eq!(result, 36.0);
    });
}

#[test]
fn ef_009_should_await_returned_promise() {
    run(|t| async move {
        t.page
            .expose_function("compute", |args| async move {
                tokio::time::sleep(Duration::from_millis(10)).await;
                Ok::<_, BindingError>(num(&args[0]) * num(&args[1]))
            })
            .await
            .unwrap();
        let result: f64 = t
            .page
            .evaluate("async function() { return await window['compute'](3, 5); }")
            .await
            .unwrap();
        assert_eq!(result, 15.0);
    });
}

#[test]
fn ef_010_should_work_on_frames() {
    run(|t| async move {
        expose_compute(&t.page).await;
        t.page
            .goto(format!("{}/frames/nested-frames.html", t.server.prefix))
            .await
            .unwrap();
        let frame = t.page.frames()[1].clone();
        let result: f64 = frame
            .evaluate("async function() { return await window['compute'](3, 5); }")
            .await
            .unwrap();
        assert_eq!(result, 15.0);
    });
}

#[test]
fn ef_011_should_work_on_frames_before_navigation() {
    run(|t| async move {
        t.page
            .goto(format!("{}/frames/nested-frames.html", t.server.prefix))
            .await
            .unwrap();
        expose_compute(&t.page).await;
        let frame = t.page.frames()[1].clone();
        let result: f64 = frame
            .evaluate("async function() { return await window['compute'](3, 5); }")
            .await
            .unwrap();
        assert_eq!(result, 15.0);
    });
}

#[test]
fn ef_012_should_work_after_cross_origin_navigation() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        expose_compute(&t.page).await;
        t.page
            .goto(format!("{}/empty.html", t.server.cross_process_prefix))
            .await
            .unwrap();
        let result: f64 = t
            .page
            .evaluate("async function() { return await window['compute'](9, 4); }")
            .await
            .unwrap();
        assert_eq!(result, 36.0);
    });
}

#[test]
fn ef_013_should_work_with_complex_objects() {
    run(|t| async move {
        t.page
            .expose_function("complexObject", |args| async move {
                let x = num(args[0].get("x").unwrap()) + num(args[1].get("x").unwrap());
                Ok::<_, BindingError>(json!({ "x": x }))
            })
            .await
            .unwrap();
        let result: Value = t
            .page
            .evaluate("async () => window['complexObject']({ x: 5 }, { x: 2 })")
            .await
            .unwrap();
        assert_eq!(result["x"], 7.0);
    });
}

#[test]
fn ef_014_should_throw_for_duplicate_registrations() {
    run(|t| async move {
        t.page
            .expose_function("foo", |_args| async move { Ok::<_, BindingError>(()) })
            .await
            .unwrap();
        let r = t
            .page
            .expose_function("foo", |_args| async move { Ok::<_, BindingError>(()) })
            .await;
        assert_err_contains(
            r,
            "page.exposeFunction: Function \"foo\" has been already registered",
        );
    });
}

#[test]
fn ef_015_should_work_with_set_content() {
    run(|t| async move {
        expose_compute(&t.page).await;
        t.page
            .set_content("<script>window.result = compute(3, 2)</script>")
            .await
            .unwrap();
        let result: f64 = t.page.evaluate("window.result").await.unwrap();
        assert_eq!(result, 6.0);
    });
}

#[test]
fn ef_016_should_alias_window_document_and_node() {
    run(|t| async move {
        let object: Arc<Mutex<Option<JsValue>>> = Arc::default();
        let o = object.clone();
        t.page
            .expose_binding("log", move |_src, args| {
                *o.lock().unwrap() = args.into_iter().next();
                async move { Ok::<_, BindingError>(()) }
            })
            .await
            .unwrap();
        let _: JsValue = t
            .page
            .evaluate("window.log([window, document, document.body])")
            .await
            .unwrap();
        let value = object.lock().unwrap().clone().unwrap();
        assert_eq!(
            value.to_json(),
            json!(["ref: <Window>", "ref: <Document>", "ref: <Node>"])
        );
    });
}

#[test]
fn ef_017_should_serialize_cycles() {
    run(|t| async move {
        let object: Arc<Mutex<Option<JsValue>>> = Arc::default();
        let o = object.clone();
        t.page
            .expose_binding("log", move |_src, args| {
                *o.lock().unwrap() = args.into_iter().next();
                async move { Ok::<_, BindingError>(()) }
            })
            .await
            .unwrap();
        let _: JsValue = t
            .page
            .evaluate("const a = {}; a.b = a; window.log(a)")
            .await
            .unwrap();
        let value = object.lock().unwrap().clone().unwrap();
        // `JsValue` is a tree: the back reference `a.b === a` cannot be
        // represented and comes back as `undefined` (the cycle is cut).
        assert!(
            value.get("b").is_some_and(|b| b.is_undefined()),
            "{value:?}"
        );
    });
}

#[test]
fn ef_018_should_work_with_overridden_console_object() {
    run(|t| async move {
        let _: JsValue = t
            .page
            .evaluate("() => window.console = null")
            .await
            .unwrap();
        let is_null: bool = t
            .page
            .evaluate("() => window.console === null")
            .await
            .unwrap();
        assert!(is_null);
        t.page
            .expose_function("add", |args| async move {
                Ok::<_, BindingError>(num(&args[0]) + num(&args[1]))
            })
            .await
            .unwrap();
        let r: f64 = t.page.evaluate("add(5, 6)").await.unwrap();
        assert_eq!(r, 11.0);
    });
}

#[test]
fn ef_019_should_work_with_busted_array_prototype_map_push() {
    run(|t| async move {
        t.server.set_route("/test", |_| async move {
            Response::builder()
                .header(header::CONTENT_TYPE, "text/html")
                .body(Body::from(
                    "<script>\n      Array.prototype.map = null;\n      Array.prototype.push = null;\n    </script>",
                ))
                .unwrap()
        });
        t.page
            .goto(format!("{}/test", t.server.prefix))
            .await
            .unwrap();
        t.page
            .expose_function("add", |args| async move {
                Ok::<_, BindingError>(num(&args[0]) + num(&args[1]))
            })
            .await
            .unwrap();
        let r: f64 = t.page.evaluate("add(5, 6)").await.unwrap();
        assert_eq!(r, 11.0);
    });
}

#[test]
fn ef_020_should_fail_with_busted_array_prototype_to_json() {
    run(|t| async move {
        let _ = t
            .page
            .evaluate_handle("() => Array.prototype.toJSON = () => '\"[]\"'")
            .await
            .unwrap();
        t.page
            .expose_function("add", |args| async move {
                Ok::<_, BindingError>(num(&args[0]) + num(&args[1]))
            })
            .await
            .unwrap();
        let r = t.page.evaluate::<f64>("add(5, 6)").await;
        assert_err_contains(
            r,
            "serializedArgs is not an array. This can happen when Array.prototype.toJSON is defined incorrectly",
        );
        let v: String = t.page.evaluate("() => ([]).toJSON()").await.unwrap();
        assert_eq!(v, "\"[]\"");
    });
}

#[test]
fn ef_021_expose_binding_should_work_in_parallel() {
    run(|t| async move {
        let (a, b) = tokio::join!(
            t.page
                .expose_binding("foo", |_s, _a| async move { Ok::<_, BindingError>(42) }),
            t.page
                .expose_binding("bar", |_s, _a| async move { Ok::<_, BindingError>(42) }),
        );
        a.unwrap();
        b.unwrap();
        let _: JsValue = t
            .page
            .evaluate("() => { window.foo(); window.bar(); }")
            .await
            .unwrap();
    });
}

// --- browsercontext-expose-function.spec.ts ---

#[test]
fn ef_022_expose_binding_should_work() {
    run(|t| async move {
        let source: Arc<Mutex<Option<BindingSource>>> = Arc::default();
        let s = source.clone();
        t.context
            .expose_binding("add", move |src, args| {
                *s.lock().unwrap() = Some(src);
                async move { Ok::<_, BindingError>(num(&args[0]) + num(&args[1])) }
            })
            .await
            .unwrap();
        let page = t.context.new_page().await.unwrap();
        let result: f64 = page.evaluate("add(5, 6)").await.unwrap();
        let src = source.lock().unwrap().clone().unwrap();
        assert_eq!(src.context, Some(t.context.clone()));
        assert_eq!(src.page, page);
        assert_eq!(src.frame, page.main_frame());
        assert_eq!(result, 11.0);
    });
}

#[test]
fn ef_023_should_work() {
    run(|t| async move {
        t.context
            .expose_function("add", |a| async move {
                Ok::<_, BindingError>(num(&a[0]) + num(&a[1]))
            })
            .await
            .unwrap();
        let page = t.context.new_page().await.unwrap();
        page.expose_function("mul", |a| async move {
            Ok::<_, BindingError>(num(&a[0]) * num(&a[1]))
        })
        .await
        .unwrap();
        t.context
            .expose_function("sub", |a| async move {
                Ok::<_, BindingError>(num(&a[0]) - num(&a[1]))
            })
            .await
            .unwrap();
        t.context
            .expose_binding("addHandle", |src, a| async move {
                let handle = src
                    .frame
                    .evaluate_handle("([a, b]) => a + b")
                    .arg((num(&a[0]), num(&a[1])))
                    .await?;
                Ok::<_, BindingError>(handle)
            })
            .await
            .unwrap();
        let result: Value = page
            .evaluate("(async () => ({ mul: await mul(9, 4), add: await add(9, 4), sub: await sub(9, 4), addHandle: await addHandle(5, 6) }))()")
            .await
            .unwrap();
        assert_eq!(
            result,
            json!({ "mul": 36, "add": 13, "sub": 5, "addHandle": 11 })
        );
    });
}

#[test]
fn ef_024_should_dispose() {
    run(|t| async move {
        let binding = t
            .context
            .expose_function("compute", |a| async move {
                Ok::<_, BindingError>(num(&a[0]) * num(&a[1]))
            })
            .await
            .unwrap();
        let page = t.context.new_page().await.unwrap();
        let result: f64 = page
            .evaluate("async function() { return await window['compute'](9, 4); }")
            .await
            .unwrap();
        assert_eq!(result, 36.0);
        binding.dispose().await.unwrap();
        let r = page
            .evaluate::<f64>("async function() { return await window['compute'](9, 4); }")
            .await;
        assert_err_contains(r, "is not a function");
    });
}

#[test]
fn ef_025_should_throw_for_duplicate_registrations() {
    run(|t| async move {
        let noop = |_a: Vec<JsValue>| async move { Ok::<_, BindingError>(()) };
        t.context.expose_function("foo", noop).await.unwrap();
        t.context.expose_function("bar", noop).await.unwrap();
        let r = t.context.expose_function("foo", noop).await;
        assert_err_contains(r, "Function \"foo\" has been already registered");
        let page = t.context.new_page().await.unwrap();
        let r = page.expose_function("foo", noop).await;
        assert_err_contains(
            r,
            "Function \"foo\" has been already registered in the browser context",
        );
        page.expose_function("baz", noop).await.unwrap();
        let r = t.context.expose_function("baz", noop).await;
        assert_err_contains(
            r,
            "Function \"baz\" has been already registered in one of the pages",
        );
    });
}

#[test]
#[ignore = "same as EF-007 (bindings are not there when init scripts run at document start)"]
fn ef_026_should_be_callable_from_inside_add_init_script() {
    run(|t| async move {
        let args: Arc<Mutex<Vec<String>>> = Arc::default();
        let a = args.clone();
        t.context
            .expose_function("woof", move |v| {
                a.lock()
                    .unwrap()
                    .push(v[0].as_str().unwrap_or_default().to_string());
                async move { Ok::<_, BindingError>(()) }
            })
            .await
            .unwrap();
        t.context
            .add_init_script("window[\"woof\"](\"context\")")
            .await
            .unwrap();
        let page = t.context.new_page().await.unwrap();
        let _: JsValue = page.evaluate("undefined").await.unwrap();
        poll(|| *args.lock().unwrap() == ["context"]).await;
        args.lock().unwrap().clear();
        page.add_init_script("window[\"woof\"](\"page\")")
            .await
            .unwrap();
        page.reload().await.unwrap();
        poll(|| *args.lock().unwrap() == ["context", "page"]).await;
    });
}

#[test]
fn ef_027_should_work_with_csp() {
    run(|t| async move {
        t.server.set_route("/empty.html", |_| async move {
            Response::builder()
                .header(header::CONTENT_TYPE, "text/html")
                .header("Content-Security-Policy", "default-src \"self\"")
                .body(Body::from(""))
                .unwrap()
        });
        t.page.goto(&t.server.empty_page).await.unwrap();
        let called = Arc::new(Mutex::new(false));
        let c = called.clone();
        t.context
            .expose_binding("hi", move |_s, _a| {
                *c.lock().unwrap() = true;
                async move { Ok::<_, BindingError>(true) }
            })
            .await
            .unwrap();
        let _: JsValue = t.page.evaluate("() => window.hi()").await.unwrap();
        assert!(*called.lock().unwrap());
    });
}

// --- stealth ---

/// Exposing a function adds exactly one own property to `window` (the
/// function), in the main frame and in an out-of-process frame.
#[test]
fn ef_stealth_no_extra_globals() {
    run(|t| async move {
        t.page
            .goto(format!("{}/empty.html", t.server.prefix))
            .await
            .unwrap();
        let names = "() => Object.getOwnPropertyNames(window).sort()";
        let before: Vec<String> = t.page.evaluate(names).await.unwrap();
        expose_compute(&t.page).await;
        let after: Vec<String> = t.page.evaluate(names).await.unwrap();
        let added: Vec<&String> = after.iter().filter(|n| !before.contains(n)).collect();
        assert_eq!(added, vec!["compute"]);
        let removed: Vec<&String> = before.iter().filter(|n| !after.contains(n)).collect();
        assert!(removed.is_empty(), "{removed:?}");
        // After a navigation (init script path) and in a cross-process frame.
        t.page.goto(&t.server.empty_page).await.unwrap();
        let after_nav: Vec<String> = t.page.evaluate(names).await.unwrap();
        let added: Vec<&String> = after_nav.iter().filter(|n| !before.contains(n)).collect();
        assert_eq!(added, vec!["compute"]);
        let frame = attach_frame(
            &t.page,
            "oop",
            &format!("{}/empty.html", t.server.cross_process_prefix),
        )
        .await;
        let in_frame: Vec<String> = frame.evaluate(names).await.unwrap();
        assert!(in_frame.contains(&"compute".to_string()));
        let extra: Vec<&String> = in_frame
            .iter()
            .filter(|n| !before.contains(n) && n.as_str() != "compute")
            .collect();
        assert!(extra.is_empty(), "{extra:?}");
        let r: f64 = frame
            .evaluate("async () => await window.compute(2, 3)")
            .await
            .unwrap();
        assert_eq!(r, 6.0);
        // The wrapper hides nothing Playwright-specific.
        let src: String = t
            .page
            .evaluate("() => window.compute.toString()")
            .await
            .unwrap();
        assert!(!src.contains("playwright"), "{src}");
    });
}
