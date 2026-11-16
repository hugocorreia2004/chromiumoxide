//! [`JsValue`]: a faithful Rust representation of the values Playwright's
//! serializer (`packages/isomorphic/utilityScriptSerializers.ts`) can transfer
//! between the browser and the driver.
//!
//! Wire format (the `SerializedValue` of the utility script):
//!
//! | JavaScript                     | wire                                   |
//! |--------------------------------|----------------------------------------|
//! | `undefined`, symbols           | `{ "v": "undefined" }`                 |
//! | `null`                         | `{ "v": "null" }`                      |
//! | `NaN`, `±Infinity`, `-0`       | `{ "v": "NaN" }` ...                   |
//! | booleans, finite numbers, strings | the JSON value itself               |
//! | `BigInt`                       | `{ "bi": "123" }`                      |
//! | `Date`                         | `{ "d": "<ISO string>" }`              |
//! | `URL`                          | `{ "u": "<href>" }`                    |
//! | `RegExp`                       | `{ "r": { "p": source, "f": flags } }` |
//! | `Error`                        | `{ "e": { "n", "m", "s" } }`           |
//! | typed arrays / `ArrayBuffer`   | `{ "ta": { "b", "k" } }` / `{ "ab": { "b" } }` |
//! | arrays                         | `{ "a": [...], "id": n }`              |
//! | objects                        | `{ "o": [{ "k", "v" }], "id": n }`     |
//! | `Map` / `Set`                  | `{ "m": [...] }` / `{ "s": [...] }`    |
//! | handle argument                | `{ "h": index }`                       |
//! | back reference                 | `{ "ref": id }`                        |

use std::collections::HashMap;
use std::fmt;

use base64::Engine;
use serde_json::{Map, Value, json};

use crate::js_handle::JSHandle;

/// A JavaScript value as transferred by Playwright's serializer.
///
/// `JsValue` can be used both as an `evaluate` argument and as its result type
/// to keep JavaScript-only distinctions (`undefined` vs `null`, `NaN`, `Date`,
/// `BigInt`, ...) that `serde_json::Value` cannot represent.
#[derive(Debug, Clone, Default)]
pub enum JsValue {
    /// `undefined` (also used for symbols and unserializable values).
    #[default]
    Undefined,
    /// `null`.
    Null,
    /// A boolean.
    Bool(bool),
    /// A number, including `NaN`, `±Infinity` and `-0`.
    Number(f64),
    /// A string.
    String(String),
    /// A `BigInt`, as its decimal representation.
    BigInt(String),
    /// A `Date`, as its ISO-8601 representation (`Date.prototype.toJSON`).
    Date(String),
    /// A `URL`, as its `href`.
    Url(String),
    /// A `RegExp`.
    RegExp {
        /// `RegExp.prototype.source`.
        source: String,
        /// `RegExp.prototype.flags`.
        flags: String,
    },
    /// An `Error` (or subclass) instance.
    Error {
        /// `error.name`.
        name: String,
        /// `error.message`.
        message: String,
        /// `error.stack`.
        stack: String,
    },
    /// An array.
    Array(Vec<JsValue>),
    /// A plain object, keys in enumeration order.
    Object(Vec<(String, JsValue)>),
    /// A `Map` (only produced/consumed with extended serialization).
    Map(Vec<(JsValue, JsValue)>),
    /// A `Set` (only produced/consumed with extended serialization).
    Set(Vec<JsValue>),
    /// A typed array: `kind` is one of `i8 ui8 ui8c i16 ui16 i32 ui32 f32 f64 bi64 bui64`.
    TypedArray {
        /// Constructor kind, as used by Playwright.
        kind: String,
        /// Raw little-endian bytes of the array.
        bytes: Vec<u8>,
    },
    /// An `ArrayBuffer`.
    ArrayBuffer(Vec<u8>),
    /// A handle to a remote object (arguments only).
    Handle(JSHandle),
}

impl PartialEq for JsValue {
    fn eq(&self, other: &Self) -> bool {
        use JsValue::*;
        match (self, other) {
            (Undefined, Undefined) | (Null, Null) => true,
            (Bool(a), Bool(b)) => a == b,
            (Number(a), Number(b)) => {
                (a.is_nan() && b.is_nan())
                    || (a == b && a.is_sign_negative() == b.is_sign_negative())
            }
            (String(a), String(b))
            | (BigInt(a), BigInt(b))
            | (Date(a), Date(b))
            | (Url(a), Url(b)) => a == b,
            (
                RegExp {
                    source: a,
                    flags: f,
                },
                RegExp {
                    source: b,
                    flags: g,
                },
            ) => a == b && f == g,
            (
                Error {
                    name: a,
                    message: m,
                    ..
                },
                Error {
                    name: b,
                    message: n,
                    ..
                },
            ) => a == b && m == n,
            (Array(a), Array(b)) | (Set(a), Set(b)) => a == b,
            (Object(a), Object(b)) => a == b,
            (Map(a), Map(b)) => a == b,
            (TypedArray { kind: a, bytes: x }, TypedArray { kind: b, bytes: y }) => {
                a == b && x == y
            }
            (ArrayBuffer(a), ArrayBuffer(b)) => a == b,
            (Handle(a), Handle(b)) => a.same_object(b),
            _ => false,
        }
    }
}

impl JsValue {
    /// Whether the value is `undefined`.
    pub fn is_undefined(&self) -> bool {
        matches!(self, JsValue::Undefined)
    }

    /// Whether the value is `null`.
    pub fn is_null(&self) -> bool {
        matches!(self, JsValue::Null)
    }

    /// The number, if this is a number.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            JsValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// The string, if this is a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsValue::String(s) => Some(s),
            _ => None,
        }
    }

    /// The boolean, if this is a boolean.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            JsValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Look up a property of an object value.
    pub fn get(&self, key: &str) -> Option<&JsValue> {
        match self {
            JsValue::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Convert to a JSON value, the way `JSON.stringify` would see it as closely
    /// as possible (`undefined` and non-finite numbers become `null`, dates and
    /// URLs become strings, ...).
    pub fn to_json(&self) -> Value {
        match self {
            JsValue::Undefined | JsValue::Null | JsValue::Handle(_) => Value::Null,
            JsValue::Bool(b) => Value::Bool(*b),
            JsValue::Number(n) => number_to_json(*n).unwrap_or(Value::Null),
            JsValue::String(s) | JsValue::BigInt(s) | JsValue::Date(s) | JsValue::Url(s) => {
                Value::String(s.clone())
            }
            JsValue::RegExp { source, flags } => Value::String(format!("/{source}/{flags}")),
            JsValue::Error {
                name,
                message,
                stack,
            } => json!({ "name": name, "message": message, "stack": stack }),
            JsValue::Array(items) | JsValue::Set(items) => {
                Value::Array(items.iter().map(JsValue::to_json).collect())
            }
            JsValue::Object(entries) => Value::Object(
                entries
                    .iter()
                    .map(|(k, v)| (k.clone(), v.to_json()))
                    .collect(),
            ),
            JsValue::Map(entries) => Value::Array(
                entries
                    .iter()
                    .map(|(k, v)| Value::Array(vec![k.to_json(), v.to_json()]))
                    .collect(),
            ),
            JsValue::TypedArray { .. } | JsValue::ArrayBuffer(_) => Value::Array(
                self.typed_elements()
                    .into_iter()
                    .map(|e| e.to_json())
                    .collect(),
            ),
        }
    }

    /// Elements of a typed array / array buffer, as JS values.
    pub(crate) fn typed_elements(&self) -> Vec<JsValue> {
        fn chunks<const N: usize>(bytes: &[u8]) -> impl Iterator<Item = [u8; N]> + '_ {
            bytes.chunks_exact(N).map(|c| {
                let mut a = [0u8; N];
                a.copy_from_slice(c);
                a
            })
        }
        match self {
            JsValue::ArrayBuffer(bytes) => {
                bytes.iter().map(|b| JsValue::Number(*b as f64)).collect()
            }
            JsValue::TypedArray { kind, bytes } => match kind.as_str() {
                "i8" => bytes
                    .iter()
                    .map(|b| JsValue::Number(*b as i8 as f64))
                    .collect(),
                "ui8" | "ui8c" => bytes.iter().map(|b| JsValue::Number(*b as f64)).collect(),
                "i16" => chunks::<2>(bytes)
                    .map(|c| JsValue::Number(i16::from_le_bytes(c) as f64))
                    .collect(),
                "ui16" => chunks::<2>(bytes)
                    .map(|c| JsValue::Number(u16::from_le_bytes(c) as f64))
                    .collect(),
                "i32" => chunks::<4>(bytes)
                    .map(|c| JsValue::Number(i32::from_le_bytes(c) as f64))
                    .collect(),
                "ui32" => chunks::<4>(bytes)
                    .map(|c| JsValue::Number(u32::from_le_bytes(c) as f64))
                    .collect(),
                "f32" => chunks::<4>(bytes)
                    .map(|c| JsValue::Number(f32::from_le_bytes(c) as f64))
                    .collect(),
                "f64" => chunks::<8>(bytes)
                    .map(|c| JsValue::Number(f64::from_le_bytes(c)))
                    .collect(),
                "bi64" => chunks::<8>(bytes)
                    .map(|c| JsValue::BigInt(i64::from_le_bytes(c).to_string()))
                    .collect(),
                "bui64" => chunks::<8>(bytes)
                    .map(|c| JsValue::BigInt(u64::from_le_bytes(c).to_string()))
                    .collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// Parse a value in Playwright's wire format (`parseEvaluationResultValue`).
    pub fn from_wire(value: &Value) -> JsValue {
        let mut refs = HashMap::new();
        parse_wire(value, &mut refs)
    }

    /// Serialize to Playwright's wire format. Handles are registered in
    /// `handles` and referenced by index.
    pub(crate) fn to_wire(
        &self,
        ids: &mut u64,
        handles: &mut dyn FnMut(&JSHandle) -> Value,
    ) -> Value {
        match self {
            JsValue::Undefined => json!({ "v": "undefined" }),
            JsValue::Null => json!({ "v": "null" }),
            JsValue::Bool(b) => Value::Bool(*b),
            JsValue::Number(n) => number_to_wire(*n),
            JsValue::String(s) => Value::String(s.clone()),
            JsValue::BigInt(s) => json!({ "bi": s }),
            JsValue::Date(s) => json!({ "d": s }),
            JsValue::Url(s) => json!({ "u": s }),
            JsValue::RegExp { source, flags } => json!({ "r": { "p": source, "f": flags } }),
            JsValue::Error {
                name,
                message,
                stack,
            } => json!({ "e": { "n": name, "m": message, "s": stack } }),
            JsValue::Array(items) => {
                *ids += 1;
                let id = *ids;
                let a: Vec<Value> = items.iter().map(|v| v.to_wire(ids, handles)).collect();
                json!({ "a": a, "id": id })
            }
            JsValue::Set(items) => {
                *ids += 1;
                let id = *ids;
                let s: Vec<Value> = items.iter().map(|v| v.to_wire(ids, handles)).collect();
                json!({ "s": s, "id": id })
            }
            JsValue::Object(entries) => {
                *ids += 1;
                let id = *ids;
                let o: Vec<Value> = entries
                    .iter()
                    .map(|(k, v)| json!({ "k": k, "v": v.to_wire(ids, handles) }))
                    .collect();
                json!({ "o": o, "id": id })
            }
            JsValue::Map(entries) => {
                *ids += 1;
                let id = *ids;
                let m: Vec<Value> = entries
                    .iter()
                    .map(|(k, v)| json!({ "k": k.to_wire(ids, handles), "v": v.to_wire(ids, handles) }))
                    .collect();
                json!({ "m": m, "id": id })
            }
            JsValue::TypedArray { kind, bytes } => {
                json!({ "ta": { "b": base64::engine::general_purpose::STANDARD.encode(bytes), "k": kind } })
            }
            JsValue::ArrayBuffer(bytes) => {
                json!({ "ab": { "b": base64::engine::general_purpose::STANDARD.encode(bytes) } })
            }
            JsValue::Handle(handle) => handles(handle),
        }
    }
}

pub(crate) fn number_to_wire(n: f64) -> Value {
    if n.is_nan() {
        json!({ "v": "NaN" })
    } else if n == f64::INFINITY {
        json!({ "v": "Infinity" })
    } else if n == f64::NEG_INFINITY {
        json!({ "v": "-Infinity" })
    } else if n == 0.0 && n.is_sign_negative() {
        json!({ "v": "-0" })
    } else {
        number_to_json(n).unwrap_or(Value::Null)
    }
}

fn number_to_json(n: f64) -> Option<Value> {
    if !n.is_finite() {
        return None;
    }
    if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
        return Some(Value::from(n as i64));
    }
    serde_json::Number::from_f64(n).map(Value::Number)
}

fn parse_wire(value: &Value, refs: &mut HashMap<u64, JsValue>) -> JsValue {
    match value {
        Value::Null => JsValue::Null,
        Value::Bool(b) => JsValue::Bool(*b),
        Value::Number(n) => JsValue::Number(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => JsValue::String(s.clone()),
        Value::Array(items) => JsValue::Array(items.iter().map(|v| parse_wire(v, refs)).collect()),
        Value::Object(obj) => parse_wire_object(obj, refs),
    }
}

fn parse_wire_object(obj: &Map<String, Value>, refs: &mut HashMap<u64, JsValue>) -> JsValue {
    let id = obj.get("id").and_then(Value::as_u64);
    if let Some(r) = obj.get("ref").and_then(Value::as_u64) {
        // Back references to fully parsed values are cloned; references to a
        // value that is still being parsed (cycles) cannot be represented.
        return refs.get(&r).cloned().unwrap_or(JsValue::Undefined);
    }
    if let Some(v) = obj.get("v").and_then(Value::as_str) {
        return match v {
            "null" => JsValue::Null,
            "NaN" => JsValue::Number(f64::NAN),
            "Infinity" => JsValue::Number(f64::INFINITY),
            "-Infinity" => JsValue::Number(f64::NEG_INFINITY),
            "-0" => JsValue::Number(-0.0),
            _ => JsValue::Undefined,
        };
    }
    if let Some(d) = obj.get("d").and_then(Value::as_str) {
        return JsValue::Date(d.to_string());
    }
    if let Some(u) = obj.get("u").and_then(Value::as_str) {
        return JsValue::Url(u.to_string());
    }
    if let Some(bi) = obj.get("bi").and_then(Value::as_str) {
        return JsValue::BigInt(bi.to_string());
    }
    if let Some(e) = obj.get("e") {
        let s = |k: &str| {
            e.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        return JsValue::Error {
            name: s("n"),
            message: s("m"),
            stack: s("s"),
        };
    }
    if let Some(r) = obj.get("r") {
        let s = |k: &str| {
            r.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        return JsValue::RegExp {
            source: s("p"),
            flags: s("f"),
        };
    }
    if let Some(Value::Array(items)) = obj.get("a") {
        let v = JsValue::Array(items.iter().map(|v| parse_wire(v, refs)).collect());
        if let Some(id) = id {
            refs.insert(id, v.clone());
        }
        return v;
    }
    if let Some(Value::Array(items)) = obj.get("o") {
        let mut entries = Vec::with_capacity(items.len());
        for item in items {
            let Some(k) = item.get("k").and_then(Value::as_str) else {
                continue;
            };
            if k == "__proto__" {
                continue;
            }
            let v = item
                .get("v")
                .map(|v| parse_wire(v, refs))
                .unwrap_or_default();
            entries.push((k.to_string(), v));
        }
        let v = JsValue::Object(entries);
        if let Some(id) = id {
            refs.insert(id, v.clone());
        }
        return v;
    }
    if let Some(Value::Array(items)) = obj.get("m") {
        let entries = items
            .iter()
            .map(|item| {
                (
                    item.get("k")
                        .map(|v| parse_wire(v, refs))
                        .unwrap_or_default(),
                    item.get("v")
                        .map(|v| parse_wire(v, refs))
                        .unwrap_or_default(),
                )
            })
            .collect();
        let v = JsValue::Map(entries);
        if let Some(id) = id {
            refs.insert(id, v.clone());
        }
        return v;
    }
    if let Some(Value::Array(items)) = obj.get("s") {
        let v = JsValue::Set(items.iter().map(|v| parse_wire(v, refs)).collect());
        if let Some(id) = id {
            refs.insert(id, v.clone());
        }
        return v;
    }
    if let Some(ta) = obj.get("ta") {
        let bytes = ta
            .get("b")
            .and_then(Value::as_str)
            .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
            .unwrap_or_default();
        let kind = ta
            .get("k")
            .and_then(Value::as_str)
            .unwrap_or("ui8")
            .to_string();
        return JsValue::TypedArray { kind, bytes };
    }
    if let Some(ab) = obj.get("ab") {
        let bytes = ab
            .get("b")
            .and_then(Value::as_str)
            .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
            .unwrap_or_default();
        return JsValue::ArrayBuffer(bytes);
    }
    JsValue::Undefined
}

impl fmt::Display for JsValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JsValue::Undefined => f.write_str("undefined"),
            JsValue::Null => f.write_str("null"),
            JsValue::Bool(b) => write!(f, "{b}"),
            JsValue::Number(n) => f.write_str(&format_js_number(*n)),
            JsValue::String(s) => f.write_str(s),
            JsValue::BigInt(s) => write!(f, "{s}n"),
            JsValue::Date(s) | JsValue::Url(s) => f.write_str(s),
            JsValue::RegExp { source, flags } => write!(f, "/{source}/{flags}"),
            JsValue::Error { name, message, .. } => write!(f, "{name}: {message}"),
            JsValue::Handle(h) => write!(f, "{h}"),
            other => f.write_str(&other.to_json().to_string()),
        }
    }
}

/// Format a number the way JavaScript's `String(n)` does for common values.
pub(crate) fn format_js_number(n: f64) -> String {
    if n.is_nan() {
        "NaN".to_string()
    } else if n == f64::INFINITY {
        "Infinity".to_string()
    } else if n == f64::NEG_INFINITY {
        "-Infinity".to_string()
    } else if n == 0.0 {
        "0".to_string()
    } else if n.fract() == 0.0 && n.abs() < 1e21 {
        format!("{}", n as i128)
    } else {
        format!("{n}")
    }
}

impl From<bool> for JsValue {
    fn from(v: bool) -> Self {
        JsValue::Bool(v)
    }
}

impl From<f64> for JsValue {
    fn from(v: f64) -> Self {
        JsValue::Number(v)
    }
}

impl From<i32> for JsValue {
    fn from(v: i32) -> Self {
        JsValue::Number(v as f64)
    }
}

impl From<i64> for JsValue {
    fn from(v: i64) -> Self {
        JsValue::Number(v as f64)
    }
}

impl From<&str> for JsValue {
    fn from(v: &str) -> Self {
        JsValue::String(v.to_string())
    }
}

impl From<String> for JsValue {
    fn from(v: String) -> Self {
        JsValue::String(v)
    }
}

impl From<JSHandle> for JsValue {
    fn from(v: JSHandle) -> Self {
        JsValue::Handle(v)
    }
}

impl From<Value> for JsValue {
    fn from(v: Value) -> Self {
        match v {
            Value::Null => JsValue::Null,
            Value::Bool(b) => JsValue::Bool(b),
            Value::Number(n) => JsValue::Number(n.as_f64().unwrap_or(f64::NAN)),
            Value::String(s) => JsValue::String(s),
            Value::Array(items) => JsValue::Array(items.into_iter().map(JsValue::from).collect()),
            Value::Object(obj) => JsValue::Object(
                obj.into_iter()
                    .map(|(k, v)| (k, JsValue::from(v)))
                    .collect(),
            ),
        }
    }
}

impl<T: Into<JsValue>> From<Vec<T>> for JsValue {
    fn from(v: Vec<T>) -> Self {
        JsValue::Array(v.into_iter().map(Into::into).collect())
    }
}

impl<T: Into<JsValue>> From<Option<T>> for JsValue {
    fn from(v: Option<T>) -> Self {
        v.map(Into::into).unwrap_or(JsValue::Null)
    }
}
