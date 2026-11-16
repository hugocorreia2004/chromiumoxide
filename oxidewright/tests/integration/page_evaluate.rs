//! Port of `tests/page/page-evaluate.spec.ts`.
//!
//! Skipped (not applicable to the Rust API or to parts not implemented yet):
//! EV-018 (JS function shorthands), EV-024/025 (`framenavigated` event),
//! EV-026 (`exposeFunction`), EV-038 (extra JS arguments), EV-045/046
//! (`window.builtins` test-only global), EV-050..053 (JS functions as
//! arguments), EV-060/061 (`page.$`), EV-067 (100MB transfer), EV-070
//! (popups), EV-071/076 (`server.setCSP`), EV-099 (`using` support of the
//! test runner).

use std::future::IntoFuture;

use oxidewright::{CONTEXT_DESTROYED_MESSAGE, JsValue};
use serde_json::{Value, json};

use crate::common::*;

#[test]
fn should_work() {
    run(|t| async move {
        let result: i32 = t.page.evaluate("() => 7 * 3").await.unwrap();
        assert_eq!(result, 21);
    });
}

#[test]
fn should_transfer_nan_zero_and_infinities() {
    run(|t| async move {
        let r: f64 = t.page.evaluate("a => a").arg(f64::NAN).await.unwrap();
        assert!(r.is_nan());
        let r: f64 = t.page.evaluate("a => a").arg(-0.0f64).await.unwrap();
        assert!(r == 0.0 && r.is_sign_negative());
        let r: f64 = t.page.evaluate("a => a").arg(f64::INFINITY).await.unwrap();
        assert_eq!(r, f64::INFINITY);
        let r: f64 = t
            .page
            .evaluate("a => a")
            .arg(f64::NEG_INFINITY)
            .await
            .unwrap();
        assert_eq!(r, f64::NEG_INFINITY);
        // The page sees the real values.
        let ok: bool = t
            .page
            .evaluate("a => Object.is(a.nan, NaN) && Object.is(a.nZero, -0) && a.inf === Infinity")
            .arg(std::collections::BTreeMap::from([
                ("nan", f64::NAN),
                ("nZero", -0.0),
                ("inf", f64::INFINITY),
            ]))
            .await
            .unwrap();
        assert!(ok);
    });
}

#[test]
fn should_roundtrip_unserializable_values() {
    run(|t| async move {
        #[derive(serde::Serialize, serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct V {
            infinity: f64,
            n_infinity: f64,
            n_zero: f64,
            nan: f64,
        }
        let v = V {
            infinity: f64::INFINITY,
            n_infinity: f64::NEG_INFINITY,
            n_zero: -0.0,
            nan: f64::NAN,
        };
        for expr in ["value => value", "value => Promise.resolve(value)"] {
            let r: V = t.page.evaluate(expr).arg(&v).await.unwrap();
            assert_eq!(r.infinity, f64::INFINITY);
            assert_eq!(r.n_infinity, f64::NEG_INFINITY);
            assert!(r.n_zero == 0.0 && r.n_zero.is_sign_negative());
            assert!(r.nan.is_nan());
        }
    });
}

#[test]
fn should_roundtrip_promise_to_value() {
    run(|t| async move {
        let r: JsValue = t
            .page
            .evaluate("value => Promise.resolve(value)")
            .arg(())
            .await
            .unwrap();
        assert!(r.is_null());
        let r: f64 = t
            .page
            .evaluate("value => Promise.resolve(value)")
            .arg(f64::INFINITY)
            .await
            .unwrap();
        assert_eq!(r, f64::INFINITY);
        let r: f64 = t
            .page
            .evaluate("value => Promise.resolve(value)")
            .arg(-0.0)
            .await
            .unwrap();
        assert!(r.is_sign_negative());
        let r: JsValue = t
            .page
            .evaluate("value => Promise.resolve(value)")
            .arg(JsValue::Undefined)
            .await
            .unwrap();
        assert!(r.is_undefined());
    });
}

#[test]
fn should_transfer_arrays() {
    run(|t| async move {
        let r: Vec<i32> = t.page.evaluate("a => a").arg([1, 2, 3]).await.unwrap();
        assert_eq!(r, vec![1, 2, 3]);
        let r: bool = t
            .page
            .evaluate("a => Array.isArray(a)")
            .arg([1, 2, 3])
            .await
            .unwrap();
        assert!(r);
    });
}

#[test]
fn should_transfer_typed_arrays() {
    run(|t| async move {
        for ctor in [
            "Int8Array",
            "Uint8Array",
            "Uint8ClampedArray",
            "Int16Array",
            "Uint16Array",
            "Int32Array",
            "Uint32Array",
        ] {
            let v: JsValue = t
                .page
                .evaluate(format!("() => new {ctor}([1, 2, 3])"))
                .await
                .unwrap();
            let back: String = t
                .page
                .evaluate("a => a.constructor.name + ':' + Array.from(a).join(',')")
                .arg(&v)
                .await
                .unwrap();
            assert_eq!(back, format!("{ctor}:1,2,3"));
            let numbers: Vec<f64> = t
                .page
                .evaluate(format!("() => new {ctor}([1, 2, 3])"))
                .await
                .unwrap();
            assert_eq!(numbers, vec![1.0, 2.0, 3.0]);
        }
        let floats: Vec<f64> = t
            .page
            .evaluate("() => new Float64Array([1.1, 2.2, 3.3])")
            .await
            .unwrap();
        assert_eq!(floats, vec![1.1, 2.2, 3.3]);
        let big: JsValue = t
            .page
            .evaluate("() => new BigInt64Array([1n, 2n, 3n])")
            .await
            .unwrap();
        let back: String = t
            .page
            .evaluate("a => a.constructor.name + ':' + a.join(',')")
            .arg(&big)
            .await
            .unwrap();
        assert_eq!(back, "BigInt64Array:1,2,3");
    });
}

#[test]
fn should_transfer_bigint() {
    run(|t| async move {
        let v: JsValue = t.page.evaluate("() => 42n").await.unwrap();
        assert_eq!(v, JsValue::BigInt("42".into()));
        let v: JsValue = t
            .page
            .evaluate("a => a")
            .arg(JsValue::BigInt("17".into()))
            .await
            .unwrap();
        assert_eq!(v, JsValue::BigInt("17".into()));
        let is_bigint: bool = t
            .page
            .evaluate("a => typeof a === 'bigint'")
            .arg(JsValue::BigInt("17".into()))
            .await
            .unwrap();
        assert!(is_bigint);
    });
}

#[test]
fn should_transfer_maps_and_sets_as_empty_objects() {
    run(|t| async move {
        let v: Value = t.page.evaluate("() => new Map([[1, 2]])").await.unwrap();
        assert_eq!(v, json!({}));
        let v: Value = t.page.evaluate("() => new Set([1, 2])").await.unwrap();
        assert_eq!(v, json!({}));
    });
}

#[test]
fn should_modify_global_environment() {
    run(|t| async move {
        t.page
            .evaluate::<()>("() => window['globalVar'] = 123")
            .await
            .unwrap();
        let v: i32 = t.page.evaluate("globalVar").await.unwrap();
        assert_eq!(v, 123);
    });
}

#[test]
fn should_evaluate_in_the_page_context() {
    run(|t| async move {
        t.page
            .goto(format!("{}/global-var.html", t.server.prefix))
            .await
            .unwrap();
        let v: i32 = t.page.evaluate("globalVar").await.unwrap();
        assert_eq!(v, 123);
    });
}

#[test]
fn should_return_undefined_for_objects_with_symbols() {
    run(|t| async move {
        let v: JsValue = t.page.evaluate("() => [Symbol('foo4')]").await.unwrap();
        assert_eq!(v, JsValue::Array(vec![JsValue::Undefined]));
        let v: Value = t
            .page
            .evaluate("() => { const a = {}; a[Symbol('foo4')] = 42; return a; }")
            .await
            .unwrap();
        assert_eq!(v, json!({}));
    });
}

#[test]
fn should_work_with_unicode_chars() {
    run(|t| async move {
        let v: i32 = t
            .page
            .evaluate("a => a['中文字符']")
            .arg(json!({ "中文字符": 42 }))
            .await
            .unwrap();
        assert_eq!(v, 42);
    });
}

#[test]
fn should_work_with_large_strings() {
    run(|t| async move {
        let expected = "x".repeat(40000);
        let v: String = t
            .page
            .evaluate("data => data")
            .arg(&expected)
            .await
            .unwrap();
        assert_eq!(v, expected);
        let expected = "🎭".repeat(10000);
        let v: String = t
            .page
            .evaluate("data => data")
            .arg(&expected)
            .await
            .unwrap();
        assert_eq!(v, expected);
    });
}

#[test]
fn should_throw_when_evaluation_triggers_reload() {
    run(|t| async move {
        let r = t
            .page
            .evaluate::<()>("() => { location.reload(); return new Promise(() => {}); }")
            .await;
        assert_err_contains(r, "navigation");
    });
}

#[test]
fn should_await_promise() {
    run(|t| async move {
        let v: i32 = t
            .page
            .evaluate("() => Promise.resolve(8 * 7)")
            .await
            .unwrap();
        assert_eq!(v, 56);
    });
}

#[test]
fn should_reject_promise_with_exception() {
    run(|t| async move {
        let r = t
            .page
            .evaluate::<()>("() => not_existing_object.property")
            .await;
        let e = assert_err_contains(r, "not_existing_object");
        assert!(e.to_string().starts_with("page.evaluate: "), "{e}");
    });
}

#[test]
fn should_support_thrown_strings_and_numbers_as_error_messages() {
    run(|t| async move {
        assert_err_contains(
            t.page.evaluate::<()>("() => { throw 'qwerty'; }").await,
            "qwerty",
        );
        assert_err_contains(
            t.page.evaluate::<()>("() => { throw 100500; }").await,
            "100500",
        );
    });
}

#[test]
fn should_reject_when_a_falsy_value_is_thrown() {
    run(|t| async move {
        for value in ["null", "undefined", "0", "''", "false", "NaN", "-0", "0n"] {
            let r = t
                .page
                .evaluate::<JsValue>(format!("(() => {{ throw {value}; }})()"))
                .await;
            assert!(r.is_err(), "throw {value}");
            let r = t
                .page
                .evaluate::<JsValue>(format!("Promise.reject({value})"))
                .await;
            assert!(r.is_err(), "Promise.reject({value})");
        }
        assert_err_contains(t.page.evaluate::<()>("() => Promise.reject(7)").await, "7");
        assert_err_contains(t.page.evaluate::<()>("() => { throw null; }").await, "null");
    });
}

#[test]
fn should_return_complex_objects() {
    run(|t| async move {
        let v: Value = t
            .page
            .evaluate("a => a")
            .arg(json!({ "foo": "bar!" }))
            .await
            .unwrap();
        assert_eq!(v, json!({ "foo": "bar!" }));
    });
}

#[test]
fn should_return_nan_zero_and_infinities() {
    run(|t| async move {
        let v: f64 = t.page.evaluate("() => NaN").await.unwrap();
        assert!(v.is_nan());
        let v: f64 = t.page.evaluate("() => -0").await.unwrap();
        assert!(v == 0.0 && v.is_sign_negative());
        let v: f64 = t.page.evaluate("() => Infinity").await.unwrap();
        assert_eq!(v, f64::INFINITY);
        let v: f64 = t.page.evaluate("() => -Infinity").await.unwrap();
        assert_eq!(v, f64::NEG_INFINITY);
    });
}

#[test]
fn should_work_with_overwritten_promise() {
    run(|t| async move {
        t.page
            .evaluate::<()>(
                "() => {
                const originalPromise = window.Promise;
                class Promise2 {
                    static all(arg) { return wrap(originalPromise.all(arg)); }
                    static race(arg) { return wrap(originalPromise.race(arg)); }
                    static resolve(arg) { return wrap(originalPromise.resolve(arg)); }
                    constructor(f) { this._promise = new originalPromise(f); }
                    then(f, r) { return wrap(this._promise.then(f, r)); }
                    catch(f) { return wrap(this._promise.catch(f)); }
                    finally(f) { return wrap(this._promise.finally(f)); }
                }
                const wrap = p => { const result = new Promise2(() => {}); result._promise = p; return result; };
                window.Promise = Promise2;
                window.__Promise2 = Promise2;
            }",
            )
            .await
            .unwrap();
        let ok: bool = t
            .page
            .evaluate("() => { const p = Promise.all([Promise.race([]), new Promise(() => {}).then(() => {})]); return p instanceof window.__Promise2; }")
            .await
            .unwrap();
        assert!(ok);
        let v: i32 = t.page.evaluate("() => Promise.resolve(42)").await.unwrap();
        assert_eq!(v, 42);
    });
}

#[test]
fn should_accept_undefined_as_one_of_multiple_parameters() {
    run(|t| async move {
        let arg = JsValue::Object(vec![
            ("a".into(), JsValue::Undefined),
            ("b".into(), JsValue::from("foo")),
        ]);
        let v: bool = t
            .page
            .evaluate("({ a, b }) => Object.is(a, undefined) && Object.is(b, 'foo')")
            .arg(arg)
            .await
            .unwrap();
        assert!(v);
    });
}

#[test]
fn should_properly_serialize_undefined_and_null() {
    run(|t| async move {
        let v: Value = t
            .page
            .evaluate("x => ({ a: x })")
            .arg(JsValue::Undefined)
            .await
            .unwrap();
        assert_eq!(v, json!({ "a": null }));
        let v: JsValue = t.page.evaluate("() => ({ a: undefined })").await.unwrap();
        assert_eq!(v.get("a"), Some(&JsValue::Undefined));
        let v: Value = t.page.evaluate("x => x").arg(Value::Null).await.unwrap();
        assert_eq!(v, Value::Null);
        let v: JsValue = t.page.evaluate("() => ({ a: null })").await.unwrap();
        assert_eq!(v.get("a"), Some(&JsValue::Null));
    });
}

#[test]
fn should_return_undefined_for_non_serializable_objects() {
    run(|t| async move {
        let v: JsValue = t.page.evaluate("() => function() {}").await.unwrap();
        assert!(v.is_undefined());
    });
}

#[test]
fn should_throw_for_too_deep_reference_chain() {
    run(|t| async move {
        let r = t
            .page
            .evaluate::<Value>(
                "depth => { const obj = {}; let temp = obj; for (let i = 0; i < depth; i++) { temp[i] = {}; temp = temp[i]; } return obj; }",
            )
            .arg(1000)
            .await;
        assert_err_contains(
            r,
            "Cannot serialize result: object reference chain is too long.",
        );
        let r = t
            .page
            .evaluate::<Value>("depth => { let node = {}; for (let i = 0; i < depth; i++) node = { child: node }; return node; }")
            .arg(200)
            .await;
        assert_err_contains(
            r,
            "Cannot serialize result: object reference chain is too long.",
        );
    });
}

#[test]
fn should_alias_window_document_and_node() {
    run(|t| async move {
        let v: Vec<String> = t
            .page
            .evaluate("[window, document, document.body]")
            .await
            .unwrap();
        assert_eq!(v, vec!["ref: <Window>", "ref: <Document>", "ref: <Node>"]);
    });
}

#[test]
fn should_work_for_circular_object() {
    run(|t| async move {
        let v: JsValue = t
            .page
            .evaluate("() => { const a = {}; a.b = a; return a; }")
            .await
            .unwrap();
        assert!(matches!(v, JsValue::Object(_)));
    });
}

#[test]
fn should_be_able_to_throw_a_tricky_error() {
    run(|t| async move {
        let window = t.page.evaluate_handle("() => window").await.unwrap();
        let error_text = window
            .json_value::<Value>()
            .await
            .map(|v| v.to_string())
            .unwrap_or_else(|e| e.message());
        let r = t
            .page
            .evaluate::<()>("errorText => { throw new Error(errorText); }")
            .arg(&error_text)
            .await;
        assert_err_contains(r, &error_text);
    });
}

#[test]
fn should_accept_a_string() {
    run(|t| async move {
        let v: i32 = t.page.evaluate("1 + 2").await.unwrap();
        assert_eq!(v, 3);
        let v: i32 = t.page.evaluate("1 + 5;").await.unwrap();
        assert_eq!(v, 6);
        let v: i32 = t.page.evaluate("2 + 5;\n// do some math!").await.unwrap();
        assert_eq!(v, 7);
    });
}

#[test]
fn should_accept_element_handle_as_an_argument() {
    run(|t| async move {
        t.page.set_content("<section>42</section>").await.unwrap();
        let element = t
            .page
            .evaluate_handle("() => document.querySelector('section')")
            .await
            .unwrap();
        let text: String = t
            .page
            .evaluate("e => e.textContent")
            .arg(&element)
            .await
            .unwrap();
        assert_eq!(text, "42");
        element.dispose().await;
        let r = t
            .page
            .evaluate::<String>("e => e.textContent")
            .arg(&element)
            .await;
        assert_err_contains(r, "JSHandle is disposed");
    });
}

#[test]
fn should_simulate_a_user_gesture() {
    run(|t| async move {
        let v: bool = t
            .page
            .evaluate("() => { document.body.appendChild(document.createTextNode('test')); document.execCommand('selectAll'); return document.execCommand('copy'); }")
            .await
            .unwrap();
        assert!(v);
    });
}

#[test]
fn should_throw_a_nice_error_after_a_navigation() {
    run(|t| async move {
        let navigation = t.page.wait_for_navigation();
        let r = t
            .page
            .evaluate::<i32>("() => new Promise(f => setTimeout(() => { window.location.reload(); setTimeout(() => f(42), 1000); }, 0))")
            .await;
        navigation.await.unwrap();
        let e = assert_err_contains(r, "navigation");
        assert!(e.to_string().contains(CONTEXT_DESTROYED_MESSAGE));
    });
}

#[test]
fn should_not_throw_an_error_when_evaluation_does_a_navigation() {
    run(|t| async move {
        t.page
            .goto(format!("{}/one-style.html", t.server.prefix))
            .await
            .unwrap();
        let v: Vec<i32> = t
            .page
            .evaluate("() => { window.location.href = '/empty.html'; return [42]; }")
            .await
            .unwrap();
        assert_eq!(v, vec![42]);
    });
}

#[test]
fn should_not_throw_an_error_when_evaluation_does_a_synchronous_navigation() {
    run(|t| async move {
        let v: Value = t
            .page
            .evaluate("() => { window.location.reload(); return { a: 42 }; }")
            .await
            .unwrap();
        assert_eq!(v, json!({ "a": 42 }));
    });
}

#[test]
fn should_not_throw_an_error_when_evaluation_does_a_synchronous_navigation_and_returns_undefined() {
    run(|t| async move {
        let v: JsValue = t
            .page
            .evaluate("() => { window.location.reload(); return undefined; }")
            .await
            .unwrap();
        assert!(v.is_undefined());
    });
}

#[test]
fn should_throw_error_with_detailed_information_on_exception_inside_promise() {
    run(|t| async move {
        let r = t
            .page
            .evaluate::<()>("() => new Promise(() => { throw new Error('Error in promise'); })")
            .await;
        assert_err_contains(r, "Error in promise");
    });
}

#[test]
fn should_work_even_when_json_is_set_to_null() {
    run(|t| async move {
        t.page
            .evaluate::<()>("() => { window.JSON.stringify = null; window.JSON = null; }")
            .await
            .unwrap();
        let v: Value = t.page.evaluate("() => ({ abc: 123 })").await.unwrap();
        assert_eq!(v, json!({ "abc": 123 }));
    });
}

#[test]
fn should_work_with_non_strict_expressions() {
    run(|t| async move {
        let v: f64 = t
            .page
            .evaluate("() => { y = 2.5; return y; }")
            .await
            .unwrap();
        assert_eq!(v, 2.5);
        let r = t
            .page
            .evaluate::<f64>("() => { 'use strict'; variableY = 3.14; return variableY; }")
            .await;
        assert_err_contains(r, "variableY");
    });
}

#[test]
fn should_not_leak_utility_script_or_handles() {
    run(|t| async move {
        let v: bool = t.page.evaluate("this === window").await.unwrap();
        assert!(v);
        let r = t.page.evaluate::<i32>("handles.length").await;
        assert_err_contains(r, " handles");
        let v: bool = t
            .page
            .evaluate("typeof pwExport === 'undefined'")
            .await
            .unwrap();
        assert!(v);
    });
}

#[test]
fn should_evaluate_exception() {
    run(|t| async move {
        let e: JsValue = t
            .page
            .evaluate("() => (function functionOnStack() { return new Error('error message'); })()")
            .await
            .unwrap();
        let JsValue::Error { message, stack, .. } = e else {
            panic!("{e:?}")
        };
        assert_eq!(message, "error message");
        assert!(stack.contains("functionOnStack"));
        let e: JsValue = t
            .page
            .evaluate("() => { function innerFunction() { const e = new Error('error message'); e.name = 'foobar'; return e; } return innerFunction(); }")
            .await
            .unwrap();
        let JsValue::Error {
            name,
            message,
            stack,
        } = e
        else {
            panic!()
        };
        assert_eq!(
            (name.as_str(), message.as_str()),
            ("foobar", "error message")
        );
        assert!(stack.contains("innerFunction"));
    });
}

#[test]
fn should_pass_exception_argument() {
    run(|t| async move {
        let error = JsValue::Error {
            name: "foobar".into(),
            message: "error message".into(),
            stack: "foobar: error message\n    at innerFunction".into(),
        };
        let v: Value = t
            .page
            .evaluate("e => ({ message: e.message, name: e.name, stack: e.stack, isError: e instanceof Error })")
            .arg(error)
            .await
            .unwrap();
        assert_eq!(v["message"], "error message");
        assert_eq!(v["name"], "foobar");
        assert!(v["stack"].as_str().unwrap().contains("innerFunction"));
        assert_eq!(v["isError"], true);
    });
}

#[test]
fn should_evaluate_and_roundtrip_date() {
    run(|t| async move {
        let v: JsValue = t
            .page
            .evaluate("() => ({ date: new Date('2020-05-27T01:31:38.506Z') })")
            .await
            .unwrap();
        assert_eq!(
            v.get("date"),
            Some(&JsValue::Date("2020-05-27T01:31:38.506Z".into()))
        );
        let v: String = t
            .page
            .evaluate("date => date.toUTCString()")
            .arg(JsValue::Date("2020-05-27T01:31:38.506Z".into()))
            .await
            .unwrap();
        assert_eq!(v, "Wed, 27 May 2020 01:31:38 GMT");
        let handle = t
            .page
            .evaluate_handle("() => ({ date: new Date('2020-05-27T01:31:38.506Z') })")
            .await
            .unwrap();
        let v: JsValue = handle.json_value().await.unwrap();
        assert_eq!(
            v.get("date"),
            Some(&JsValue::Date("2020-05-27T01:31:38.506Z".into()))
        );
    });
}

#[test]
fn should_roundtrip_regex() {
    run(|t| async move {
        let regex = JsValue::RegExp {
            source: "hello".into(),
            flags: "im".into(),
        };
        let v: JsValue = t.page.evaluate("regex => regex").arg(&regex).await.unwrap();
        assert_eq!(v, regex);
        let s: String = t
            .page
            .evaluate("regex => regex.toString()")
            .arg(&regex)
            .await
            .unwrap();
        assert_eq!(s, "/hello/im");
    });
}

#[test]
fn should_evaluate_and_roundtrip_url() {
    run(|t| async move {
        let v: JsValue = t
            .page
            .evaluate("() => ({ url: new URL('https://example.com') })")
            .await
            .unwrap();
        assert_eq!(
            v.get("url"),
            Some(&JsValue::Url("https://example.com/".into()))
        );
        let v: String = t
            .page
            .evaluate("url => url instanceof URL && url.toString()")
            .arg(JsValue::Url("https://example.com/".into()))
            .await
            .unwrap();
        assert_eq!(v, "https://example.com/");
    });
}

#[test]
fn should_not_use_to_json_when_evaluating() {
    run(|t| async move {
        let v: Value = t
            .page
            .evaluate("() => ({ toJSON: () => 'string', data: 'data' })")
            .await
            .unwrap();
        assert_eq!(v, json!({ "data": "data", "toJSON": {} }));
        let v: Value = t
            .page
            .evaluate("() => { Array.prototype.toJSON = () => 'busted'; return [1, 2, 3]; }")
            .await
            .unwrap();
        assert_eq!(v, json!([1, 2, 3]));
        let handle = t
            .page
            .evaluate_handle("() => ({ toJSON: () => 'string', data: 'data' })")
            .await
            .unwrap();
        let v: Value = handle.json_value().await.unwrap();
        assert_eq!(v, json!({ "data": "data", "toJSON": {} }));
    });
}

#[test]
fn should_not_add_a_to_json_property_to_newly_created_arrays_after_evaluation() {
    run(|t| async move {
        t.page.evaluate::<Value>("() => []").await.unwrap();
        let v: bool = t.page.evaluate("() => 'toJSON' in []").await.unwrap();
        assert!(!v);
    });
}

#[test]
fn should_ignore_buggy_to_json() {
    run(|t| async move {
        let v: Value = t
            .page
            .evaluate("() => { class Foo { toJSON() { throw new Error('Bad'); } } class Bar { get toJSON() { throw new Error('Also bad'); } } return { foo: new Foo(), bar: new Bar() }; }")
            .await
            .unwrap();
        assert_eq!(v, json!({ "foo": {}, "bar": {} }));
    });
}

#[test]
fn should_throw_when_frame_is_detached() {
    run(|t| async move {
        attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        let frame = t.page.frames()[1].clone();
        let pending = tokio::spawn(
            frame
                .evaluate::<()>("() => new Promise(() => {})")
                .into_future(),
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        detach_frame(&t.page, "frame1").await;
        let error = pending.await.unwrap().unwrap_err().to_string();
        assert!(
            error.starts_with("frame.evaluate: Frame was detached")
                || error.starts_with("frame.evaluate: Execution context was destroyed"),
            "{error}"
        );
    });
}

#[test]
fn should_work_with_overridden_object_define_property_and_array_prototype() {
    run(|t| async move {
        t.server.set_content(
            "/test",
            "text/html",
            "<script>Object.create = null; Object.defineProperty = null; Object.getOwnPropertyDescriptor = null; Object.getOwnPropertyNames = null; Object.getPrototypeOf = null; Object.prototype.hasOwnProperty = null; Array.prototype.map = null; Array.prototype.push = null;</script>",
        );
        t.page
            .goto(format!("{}/test", t.server.prefix))
            .await
            .unwrap();
        let v: i32 = t.page.evaluate("1+2").await.unwrap();
        assert_eq!(v, 3);
        let v: i32 = t
            .page
            .evaluate_handle("1+2")
            .await
            .unwrap()
            .json_value()
            .await
            .unwrap();
        assert_eq!(v, 3);
    });
}

#[test]
fn should_work_with_overridden_globals() {
    run(|t| async move {
        for case in [
            "() => globalThis.Window = {}",
            "() => globalThis.Document = {}",
            "() => globalThis.Node = {}",
            "() => globalThis.Window = null",
            "() => globalThis.Document = null",
            "() => globalThis.Node = null",
            "() => globalThis.URL = 'foo'",
            "() => globalThis.RegExp = 'foo'",
            "() => globalThis.Date = 'foo'",
        ] {
            t.page.goto(&t.server.empty_page).await.unwrap();
            t.page.evaluate::<()>(case).await.unwrap();
            let v: i32 = t.page.evaluate("1+2").await.unwrap();
            assert_eq!(v, 3, "{case}");
            let v: Value = t
                .page
                .evaluate("() => ({ 'a': 2023, b: ['foo'] })")
                .await
                .unwrap();
            assert_eq!(v, json!({ "a": 2023, "b": ["foo"] }), "{case}");
        }
    });
}

#[test]
fn should_work_with_array_from_map() {
    run(|t| async move {
        let v: String = t
            .page
            .evaluate("() => { const r = (str, amount) => Array.from(Array(amount)).map(() => str).join(''); return r('([a-f0-9]{2})', 3); }")
            .await
            .unwrap();
        assert_eq!(v, "([a-f0-9]{2})([a-f0-9]{2})([a-f0-9]{2})");
    });
}

#[test]
fn should_ignore_dangerous_object_keys() {
    run(|t| async move {
        let arg = JsValue::Object(vec![
            (
                "__proto__".into(),
                JsValue::Object(vec![("polluted".into(), JsValue::Bool(true))]),
            ),
            ("safeKey".into(), JsValue::from("safeValue")),
        ]);
        let v: Value = t.page.evaluate("arg => arg").arg(arg).await.unwrap();
        assert_eq!(v, json!({ "safeKey": "safeValue" }));
    });
}

#[test]
fn should_evaluate_in_isolated_context() {
    // Patchright extension: `isolated_context(true)` runs in the utility world.
    run(|t| async move {
        t.page
            .evaluate::<()>("() => window.mainWorldOnly = 1")
            .await
            .unwrap();
        let main: String = t
            .page
            .evaluate("() => typeof window.mainWorldOnly")
            .await
            .unwrap();
        assert_eq!(main, "number");
        let isolated: String = t
            .page
            .evaluate("() => typeof window.mainWorldOnly")
            .isolated_context(true)
            .await
            .unwrap();
        assert_eq!(isolated, "undefined");
        let title: String = t
            .page
            .evaluate("() => document.title = 'shared DOM'")
            .isolated_context(true)
            .await
            .unwrap();
        assert_eq!(title, t.page.title().await.unwrap());
    });
}
