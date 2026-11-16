//! Ports of `jshandle-as-element.spec.ts`, `jshandle-evaluate.spec.ts`,
//! `jshandle-json-value.spec.ts`, `jshandle-properties.spec.ts`,
//! `jshandle-to-string.spec.ts` and `page-evaluate-handle.spec.ts`.
//!
//! Skipped: JH-020 (`console` event).

use std::time::Duration;

use oxidewright::{JSHandle, JsValue};
use serde::Serialize;
use serde_json::{Value, json};

use crate::common::*;

// ---- jshandle-as-element.spec.ts ----

#[test]
fn as_element_should_work() {
    run(|t| async move {
        let handle = t.page.evaluate_handle("() => document.body").await.unwrap();
        assert!(handle.as_element().is_some());
        let handle = t.page.evaluate_handle("() => 2").await.unwrap();
        assert!(handle.as_element().is_none());
    });
}

#[test]
fn as_element_should_return_element_handle_for_text_nodes() {
    run(|t| async move {
        t.page.set_content("<div>ee!</div>").await.unwrap();
        let handle = t
            .page
            .evaluate_handle("() => document.querySelector('div').firstChild")
            .await
            .unwrap();
        let element = handle.as_element().unwrap();
        let is_text: bool = t
            .page
            .evaluate("e => e.nodeType === Node.TEXT_NODE")
            .arg(&*element)
            .await
            .unwrap();
        assert!(is_text);
    });
}

#[test]
fn as_element_should_work_with_nullified_node() {
    run(|t| async move {
        t.page.set_content("<section>test</section>").await.unwrap();
        t.page.evaluate::<()>("delete Node").await.unwrap();
        let handle = t
            .page
            .evaluate_handle("() => document.querySelector('section')")
            .await
            .unwrap();
        assert!(handle.as_element().is_some());
    });
}

// ---- jshandle-evaluate.spec.ts ----

#[test]
fn evaluate_should_work_with_function_and_expression() {
    run(|t| async move {
        let window = t
            .page
            .evaluate_handle("() => { window['foo'] = [1, 2]; return window; }")
            .await
            .unwrap();
        let v: Vec<i32> = window.evaluate("w => w['foo']").await.unwrap();
        assert_eq!(v, vec![1, 2]);
        let v: Vec<i32> = window.evaluate("window.foo").await.unwrap();
        assert_eq!(v, vec![1, 2]);
    });
}

// ---- jshandle-json-value.spec.ts ----

#[test]
fn json_value_should_work() {
    run(|t| async move {
        let handle = t
            .page
            .evaluate_handle("() => ({ foo: 'bar' })")
            .await
            .unwrap();
        let v: Value = handle.json_value().await.unwrap();
        assert_eq!(v, json!({ "foo": "bar" }));
    });
}

#[test]
fn json_value_should_work_with_dates() {
    run(|t| async move {
        let handle = t
            .page
            .evaluate_handle("() => new Date('2017-09-26T00:00:00.000Z')")
            .await
            .unwrap();
        let v: JsValue = handle.json_value().await.unwrap();
        assert_eq!(v, JsValue::Date("2017-09-26T00:00:00.000Z".into()));
    });
}

#[test]
fn json_value_should_handle_circular_objects() {
    run(|t| async move {
        let handle = t
            .page
            .evaluate_handle("const a = {}; a.b = a; a")
            .await
            .unwrap();
        let v: JsValue = handle.json_value().await.unwrap();
        assert!(matches!(v, JsValue::Object(_)));
    });
}

// ---- jshandle-properties.spec.ts ----

#[test]
fn get_property_should_work() {
    run(|t| async move {
        let handle = t
            .page
            .evaluate_handle("() => ({ one: 1, two: 2, three: 3 })")
            .await
            .unwrap();
        let two = handle.get_property("two").await.unwrap();
        assert_eq!(two.json_value::<i32>().await.unwrap(), 2);
    });
}

#[test]
fn get_property_should_work_with_undefined_null_and_empty() {
    run(|t| async move {
        let handle = t
            .page
            .evaluate_handle("() => ({ undefined: undefined, null: null })")
            .await
            .unwrap();
        let v: JsValue = handle
            .get_property("undefined")
            .await
            .unwrap()
            .json_value()
            .await
            .unwrap();
        assert!(v.is_undefined());
        let v: JsValue = handle
            .get_property("null")
            .await
            .unwrap()
            .json_value()
            .await
            .unwrap();
        assert!(v.is_null());
        let v: JsValue = handle
            .get_property("empty")
            .await
            .unwrap()
            .json_value()
            .await
            .unwrap();
        assert!(v.is_undefined());
    });
}

#[test]
fn get_property_should_work_with_unserializable_values() {
    run(|t| async move {
        let handle = t
            .page
            .evaluate_handle(
                "() => ({ infinity: Infinity, nInfinity: -Infinity, nan: NaN, nzero: -0 })",
            )
            .await
            .unwrap();
        let v: f64 = handle
            .get_property("infinity")
            .await
            .unwrap()
            .json_value()
            .await
            .unwrap();
        assert_eq!(v, f64::INFINITY);
        let v: f64 = handle
            .get_property("nInfinity")
            .await
            .unwrap()
            .json_value()
            .await
            .unwrap();
        assert_eq!(v, f64::NEG_INFINITY);
        let v: f64 = handle
            .get_property("nan")
            .await
            .unwrap()
            .json_value()
            .await
            .unwrap();
        assert!(v.is_nan());
        let v: f64 = handle
            .get_property("nzero")
            .await
            .unwrap()
            .json_value()
            .await
            .unwrap();
        assert!(v == 0.0 && v.is_sign_negative());
    });
}

#[test]
fn get_properties_should_work() {
    run(|t| async move {
        let handle = t
            .page
            .evaluate_handle("() => ({ foo: 'bar' })")
            .await
            .unwrap();
        let properties = handle.get_properties().await.unwrap();
        assert_eq!(
            properties["foo"].json_value::<String>().await.unwrap(),
            "bar"
        );
        let handle = t.page.evaluate_handle("() => 123").await.unwrap();
        assert!(handle.get_properties().await.unwrap().is_empty());
    });
}

#[test]
fn get_properties_should_return_properties_inherited_from_super_class() {
    run(|t| async move {
        let handle = t
            .page
            .evaluate_handle("() => { class A { constructor() { this.a = '1'; } } class B extends A { constructor() { super(); this.b = '2'; } } return new B(); }")
            .await
            .unwrap();
        let properties = handle.get_properties().await.unwrap();
        assert_eq!(properties["a"].json_value::<String>().await.unwrap(), "1");
        assert_eq!(properties["b"].json_value::<String>().await.unwrap(), "2");
    });
}

#[test]
fn get_properties_should_return_own_enumerable_properties_only() {
    run(|t| async move {
        let handle = t
            .page
            .evaluate_handle("() => { class A { constructor() { this.own = 1; Object.defineProperty(this, 'ownHidden', { value: 2, enumerable: false }); } } A.prototype.inherited = 3; return new A(); }")
            .await
            .unwrap();
        let keys: Vec<String> = handle.get_properties().await.unwrap().into_keys().collect();
        assert_eq!(keys, vec!["own".to_string()]);
        t.page
            .set_content("<div id=d><span>a</span><span>b</span></div>")
            .await
            .unwrap();
        let collection = t
            .page
            .evaluate_handle("() => document.querySelector('#d').children")
            .await
            .unwrap();
        let mut keys: Vec<String> = collection
            .get_properties()
            .await
            .unwrap()
            .into_keys()
            .collect();
        keys.sort();
        assert_eq!(keys, vec!["0".to_string(), "1".to_string()]);
    });
}

#[test]
fn get_properties_should_work_with_elements() {
    run(|t| async move {
        t.page.set_content("<div>Hello</div>").await.unwrap();
        let handle = t
            .page
            .evaluate_handle("() => ({ body: document.body })")
            .await
            .unwrap();
        let properties = handle.get_properties().await.unwrap();
        let body = properties["body"].as_element().unwrap();
        let text: String = body.evaluate("e => e.textContent").await.unwrap();
        assert_eq!(text, "Hello");
    });
}

// ---- jshandle-to-string.spec.ts ----

#[test]
fn to_string_should_work_for_primitives() {
    run(|t| async move {
        assert_eq!(
            t.page.evaluate_handle("() => 2").await.unwrap().to_string(),
            "2"
        );
        assert_eq!(
            t.page
                .evaluate_handle("() => 'a'")
                .await
                .unwrap()
                .to_string(),
            "a"
        );
    });
}

#[test]
fn to_string_should_work_for_complicated_objects() {
    run(|t| async move {
        assert_eq!(
            t.page
                .evaluate_handle("() => window")
                .await
                .unwrap()
                .to_string(),
            "Window"
        );
    });
}

#[test]
fn to_string_should_work_for_promises() {
    run(|t| async move {
        let wrapper = t
            .page
            .evaluate_handle("() => ({ b: Promise.resolve(123) })")
            .await
            .unwrap();
        let b = wrapper.get_property("b").await.unwrap();
        assert_eq!(b.to_string(), "Promise");
    });
}

#[test]
fn to_string_should_work_with_different_subtypes() {
    run(|t| async move {
        let s = |h: JSHandle| h.to_string();
        let p = &t.page;
        // String expressions evaluating to a function are called (like the Python
        // client), so return the function from an arrow function instead.
        assert!(s(p.evaluate_handle("() => (function(){})").await.unwrap()).contains("function"));
        assert_eq!(s(p.evaluate_handle("12").await.unwrap()), "12");
        assert_eq!(s(p.evaluate_handle("true").await.unwrap()), "true");
        assert_eq!(
            s(p.evaluate_handle("undefined").await.unwrap()),
            "undefined"
        );
        assert_eq!(s(p.evaluate_handle("\"foo\"").await.unwrap()), "foo");
        assert_eq!(s(p.evaluate_handle("Symbol()").await.unwrap()), "Symbol()");
        assert!(s(p.evaluate_handle("new Map()").await.unwrap()).contains("Map"));
        assert!(s(p.evaluate_handle("new Set()").await.unwrap()).contains("Set"));
        assert!(s(p.evaluate_handle("[]").await.unwrap()).contains("Array"));
        assert_eq!(s(p.evaluate_handle("null").await.unwrap()), "null");
        let body = p.evaluate_handle("document.body").await.unwrap();
        let mut preview = body.to_string();
        for _ in 0..100 {
            if preview == "JSHandle@<body></body>" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
            preview = body.to_string();
        }
        assert_eq!(preview, "JSHandle@<body></body>");
        assert_eq!(
            s(p.evaluate_handle("new WeakMap()").await.unwrap()),
            "WeakMap"
        );
        assert_eq!(
            s(p.evaluate_handle("new WeakSet()").await.unwrap()),
            "WeakSet"
        );
        assert!(s(p.evaluate_handle("new Error()").await.unwrap()).contains("Error"));
        assert_eq!(
            s(p.evaluate_handle("new Proxy({}, {})").await.unwrap()),
            "Proxy(Object)"
        );
        assert_eq!(s(p.evaluate_handle("/foo/").await.unwrap()), "/foo/");
        assert!(s(p.evaluate_handle("new Date(0)").await.unwrap()).contains("GMT"));
        assert!(s(p.evaluate_handle("new Int32Array()").await.unwrap()).contains("Int32Array"));
    });
}

// ---- page-evaluate-handle.spec.ts ----

#[test]
fn evaluate_handle_should_accept_object_handles_as_arguments() {
    run(|t| async move {
        let navigator = t.page.evaluate_handle("() => navigator").await.unwrap();
        let ua: String = t
            .page
            .evaluate("e => e.userAgent")
            .arg(&navigator)
            .await
            .unwrap();
        assert!(ua.contains("Mozilla"));
        let five = t.page.evaluate_handle("() => 5").await.unwrap();
        assert!(
            t.page
                .evaluate::<bool>("e => Object.is(e, 5)")
                .arg(&five)
                .await
                .unwrap()
        );
        let inf = t.page.evaluate_handle("() => Infinity").await.unwrap();
        assert!(
            t.page
                .evaluate::<bool>("e => Object.is(e, Infinity)")
                .arg(&inf)
                .await
                .unwrap()
        );
        let window = t
            .page
            .evaluate_handle("() => { window['FOO'] = 123; return window; }")
            .await
            .unwrap();
        assert_eq!(
            t.page
                .evaluate::<i32>("e => e['FOO']")
                .arg(&window)
                .await
                .unwrap(),
            123
        );
    });
}

#[test]
fn evaluate_handle_should_accept_nested_handles() {
    run(|t| async move {
        #[derive(Serialize)]
        struct Foo<'a> {
            foo: &'a JSHandle,
        }
        let foo = t
            .page
            .evaluate_handle("() => ({ x: 1, y: 'foo' })")
            .await
            .unwrap();
        let v: Value = t
            .page
            .evaluate("({ foo }) => foo")
            .arg(Foo { foo: &foo })
            .await
            .unwrap();
        assert_eq!(v, json!({ "x": 1, "y": "foo" }));
        let window = t.page.evaluate_handle("() => window").await.unwrap();
        let v: bool = t
            .page
            .evaluate("({ foo }) => foo === window")
            .arg(Foo { foo: &window })
            .await
            .unwrap();
        assert!(v);
    });
}

#[test]
fn evaluate_handle_should_accept_multiple_nested_handles() {
    run(|t| async move {
        let foo = t
            .page
            .evaluate_handle("() => ({ x: 1, y: 'foo' })")
            .await
            .unwrap();
        let bar = t.page.evaluate_handle("() => 5").await.unwrap();
        let baz = t.page.evaluate_handle("() => (['baz'])").await.unwrap();
        let arg = JsValue::Object(vec![
            (
                "a1".into(),
                JsValue::Object(vec![("foo".into(), JsValue::Handle(foo.clone()))]),
            ),
            (
                "a2".into(),
                JsValue::Object(vec![
                    ("bar".into(), JsValue::Handle(bar)),
                    (
                        "arr".into(),
                        JsValue::Array(vec![JsValue::Object(vec![(
                            "baz".into(),
                            JsValue::Handle(baz),
                        )])]),
                    ),
                ]),
            ),
        ]);
        let result: String = t
            .page
            .evaluate("x => JSON.stringify(x)")
            .arg(&arg)
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            parsed,
            json!({ "a1": { "foo": { "x": 1, "y": "foo" } }, "a2": { "bar": 5, "arr": [{ "baz": ["baz"] }] } })
        );
    });
}

#[test]
fn evaluate_handle_should_accept_same_handle_multiple_times() {
    run(|t| async move {
        #[derive(Serialize)]
        struct Arg<'a> {
            foo: &'a JSHandle,
            bar: Vec<&'a JSHandle>,
            baz: std::collections::BTreeMap<&'static str, &'a JSHandle>,
        }
        let foo = t.page.evaluate_handle("() => 1").await.unwrap();
        let v: Value = t
            .page
            .evaluate("x => x")
            .arg(Arg {
                foo: &foo,
                bar: vec![&foo],
                baz: [("foo", &foo)].into(),
            })
            .await
            .unwrap();
        assert_eq!(v, json!({ "foo": 1, "bar": [1], "baz": { "foo": 1 } }));
        let obj = json!({ "x": 1 });
        let v: Value = t
            .page
            .evaluate("x => x")
            .arg(json!({ "foo": obj, "bar": [obj], "baz": { "foo": obj } }))
            .await
            .unwrap();
        assert_eq!(
            v,
            json!({ "foo": { "x": 1 }, "bar": [{ "x": 1 }], "baz": { "foo": { "x": 1 } } })
        );
    });
}

#[test]
fn evaluate_handle_should_pass_configurable_args() {
    run(|t| async move {
        let v: Value = t
            .page
            .evaluate("arg => { if (arg.foo !== 42) throw new Error('Not a 42'); arg.foo = 17; if (arg.foo !== 17) throw new Error('Not 17'); delete arg.foo; if (arg.foo === 17) throw new Error('Still 17'); return arg; }")
            .arg(json!({ "foo": 42 }))
            .await
            .unwrap();
        assert_eq!(v, json!({}));
    });
}

#[test]
fn handle_dispose_releases_the_object() {
    run(|t| async move {
        let handle = t.page.evaluate_handle("() => ({ a: 1 })").await.unwrap();
        handle.dispose().await;
        assert_err_contains(
            handle.evaluate::<i32>("o => o.a").await,
            "JSHandle is disposed!",
        );
    });
}
