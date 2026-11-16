//! Serde glue between Rust values and Playwright's wire format.
//!
//! * [`to_wire`] serializes any `T: Serialize` into the `SerializedValue`
//!   format understood by the utility script, collecting [`JSHandle`]s that
//!   appear in the value (they are passed as `{ h: index }` and sent as
//!   separate CDP call arguments).
//! * `JsValue` implements `Deserializer`, so evaluation results (parsed into a
//!   [`JsValue`]) can be turned into any `T: DeserializeOwned`, keeping `NaN`,
//!   `Infinity` and `-0` for floating point targets.

use std::cell::{Cell, RefCell};
use std::fmt;

use serde::de::{self, DeserializeSeed, IntoDeserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{self, Serialize};
use serde_json::{Value, json};

use super::value::{JsValue, number_to_wire};
use crate::js_handle::JSHandle;

/// Newtype-struct name used to splice pre-serialized wire JSON.
pub(crate) const RAW_WIRE: &str = "$__oxidewright_raw_wire";
/// Newtype-struct name used to hand a `JsValue` to its own `Deserialize` impl.
pub(crate) const RAW_JSVALUE: &str = "$__oxidewright_jsvalue";

thread_local! {
    static HANDLES: RefCell<Option<Vec<JSHandle>>> = const { RefCell::new(None) };
    static IDS: Cell<u64> = const { Cell::new(0) };
    static STASH: RefCell<Option<JsValue>> = const { RefCell::new(None) };
}

/// Serialize `value` into Playwright's wire format, returning the collected handles.
pub(crate) fn to_wire<T: Serialize + ?Sized>(value: &T) -> Result<(Value, Vec<JSHandle>), String> {
    let previous = HANDLES.with(|h| h.borrow_mut().replace(Vec::new()));
    let previous_ids = IDS.with(|c| c.replace(0));
    let result = value.serialize(WireSerializer);
    let handles = HANDLES.with(|h| {
        let mut slot = h.borrow_mut();
        let handles = slot.take().unwrap_or_default();
        *slot = previous;
        handles
    });
    IDS.with(|c| c.set(previous_ids));
    result.map(|v| (v, handles)).map_err(|e| e.0)
}

fn next_id() -> u64 {
    IDS.with(|c| {
        let id = c.get() + 1;
        c.set(id);
        id
    })
}

fn in_wire_context() -> bool {
    HANDLES.with(|h| h.borrow().is_some())
}

/// Register a handle for the current serialization; returns `{ h: index }`.
pub(crate) fn register_handle(handle: &JSHandle) -> Value {
    if let Some(value) = handle.inline_value() {
        let mut ids = IDS.with(|c| c.get());
        let wire = value.to_wire(&mut ids, &mut |h| register_handle(h));
        IDS.with(|c| c.set(ids));
        return wire;
    }
    HANDLES.with(|h| {
        let mut slot = h.borrow_mut();
        let handles = slot.get_or_insert_with(Vec::new);
        handles.push(handle.clone());
        json!({ "h": handles.len() - 1 })
    })
}

impl Serialize for JSHandle {
    fn serialize<S: ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if in_wire_context() {
            serializer.serialize_newtype_struct(RAW_WIRE, &register_handle(self))
        } else {
            serializer.serialize_str(&self.to_string())
        }
    }
}

impl Serialize for JsValue {
    fn serialize<S: ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if in_wire_context() {
            let mut ids = IDS.with(|c| c.get());
            let wire = self.to_wire(&mut ids, &mut |h| register_handle(h));
            IDS.with(|c| c.set(ids));
            serializer.serialize_newtype_struct(RAW_WIRE, &wire)
        } else {
            self.to_json().serialize(serializer)
        }
    }
}

/// Error converting between Rust values and [`JsValue`]s.
#[derive(Debug)]
pub struct ValueError(pub String);

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ValueError {}

impl ser::Error for ValueError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        ValueError(msg.to_string())
    }
}

impl de::Error for ValueError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        ValueError(msg.to_string())
    }
}

// ---------------------------------------------------------------------------
// Serializer
// ---------------------------------------------------------------------------

struct WireSerializer;

fn wire_array(items: Vec<Value>) -> Value {
    json!({ "a": items, "id": next_id() })
}

fn wire_object(entries: Vec<(String, Value)>) -> Value {
    let o: Vec<Value> = entries
        .into_iter()
        .map(|(k, v)| json!({ "k": k, "v": v }))
        .collect();
    json!({ "o": o, "id": next_id() })
}

fn int_to_wire(n: i128) -> Value {
    if n.unsigned_abs() <= 9_007_199_254_740_991 {
        Value::from(n as i64)
    } else {
        Value::from(n as f64)
    }
}

impl ser::Serializer for WireSerializer {
    type Ok = Value;
    type Error = ValueError;
    type SerializeSeq = SeqSer;
    type SerializeTuple = SeqSer;
    type SerializeTupleStruct = SeqSer;
    type SerializeTupleVariant = VariantSeqSer;
    type SerializeMap = MapSer;
    type SerializeStruct = MapSer;
    type SerializeStructVariant = VariantMapSer;

    fn serialize_bool(self, v: bool) -> Result<Value, ValueError> {
        Ok(Value::Bool(v))
    }
    fn serialize_i8(self, v: i8) -> Result<Value, ValueError> {
        Ok(Value::from(v))
    }
    fn serialize_i16(self, v: i16) -> Result<Value, ValueError> {
        Ok(Value::from(v))
    }
    fn serialize_i32(self, v: i32) -> Result<Value, ValueError> {
        Ok(Value::from(v))
    }
    fn serialize_i64(self, v: i64) -> Result<Value, ValueError> {
        Ok(int_to_wire(v as i128))
    }
    fn serialize_i128(self, v: i128) -> Result<Value, ValueError> {
        Ok(int_to_wire(v))
    }
    fn serialize_u8(self, v: u8) -> Result<Value, ValueError> {
        Ok(Value::from(v))
    }
    fn serialize_u16(self, v: u16) -> Result<Value, ValueError> {
        Ok(Value::from(v))
    }
    fn serialize_u32(self, v: u32) -> Result<Value, ValueError> {
        Ok(Value::from(v))
    }
    fn serialize_u64(self, v: u64) -> Result<Value, ValueError> {
        Ok(int_to_wire(v as i128))
    }
    fn serialize_u128(self, v: u128) -> Result<Value, ValueError> {
        Ok(if v <= i128::MAX as u128 {
            int_to_wire(v as i128)
        } else {
            Value::from(v as f64)
        })
    }
    fn serialize_f32(self, v: f32) -> Result<Value, ValueError> {
        Ok(number_to_wire(v as f64))
    }
    fn serialize_f64(self, v: f64) -> Result<Value, ValueError> {
        Ok(number_to_wire(v))
    }
    fn serialize_char(self, v: char) -> Result<Value, ValueError> {
        Ok(Value::String(v.to_string()))
    }
    fn serialize_str(self, v: &str) -> Result<Value, ValueError> {
        Ok(Value::String(v.to_string()))
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<Value, ValueError> {
        Ok(wire_array(v.iter().map(|b| Value::from(*b)).collect()))
    }
    fn serialize_none(self) -> Result<Value, ValueError> {
        Ok(json!({ "v": "null" }))
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Value, ValueError> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<Value, ValueError> {
        Ok(json!({ "v": "null" }))
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<Value, ValueError> {
        Ok(json!({ "v": "null" }))
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<Value, ValueError> {
        Ok(Value::String(variant.to_string()))
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<Value, ValueError> {
        if name == RAW_WIRE {
            return value
                .serialize(serde_json::value::Serializer)
                .map_err(|e| ValueError(e.to_string()));
        }
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Value, ValueError> {
        let v = value.serialize(WireSerializer)?;
        Ok(wire_object(vec![(variant.to_string(), v)]))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<SeqSer, ValueError> {
        Ok(SeqSer {
            items: Vec::with_capacity(len.unwrap_or(0)),
        })
    }
    fn serialize_tuple(self, len: usize) -> Result<SeqSer, ValueError> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_struct(self, _name: &'static str, len: usize) -> Result<SeqSer, ValueError> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<VariantSeqSer, ValueError> {
        Ok(VariantSeqSer {
            variant,
            items: Vec::with_capacity(len),
        })
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<MapSer, ValueError> {
        Ok(MapSer {
            entries: Vec::new(),
            next_key: None,
        })
    }
    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<MapSer, ValueError> {
        Ok(MapSer {
            entries: Vec::with_capacity(len),
            next_key: None,
        })
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<VariantMapSer, ValueError> {
        Ok(VariantMapSer {
            variant,
            entries: Vec::with_capacity(len),
        })
    }
}

struct SeqSer {
    items: Vec<Value>,
}

impl ser::SerializeSeq for SeqSer {
    type Ok = Value;
    type Error = ValueError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), ValueError> {
        self.items.push(value.serialize(WireSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<Value, ValueError> {
        Ok(wire_array(self.items))
    }
}

impl ser::SerializeTuple for SeqSer {
    type Ok = Value;
    type Error = ValueError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), ValueError> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<Value, ValueError> {
        ser::SerializeSeq::end(self)
    }
}

impl ser::SerializeTupleStruct for SeqSer {
    type Ok = Value;
    type Error = ValueError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), ValueError> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<Value, ValueError> {
        ser::SerializeSeq::end(self)
    }
}

struct VariantSeqSer {
    variant: &'static str,
    items: Vec<Value>,
}

impl ser::SerializeTupleVariant for VariantSeqSer {
    type Ok = Value;
    type Error = ValueError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), ValueError> {
        self.items.push(value.serialize(WireSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<Value, ValueError> {
        let inner = wire_array(self.items);
        Ok(wire_object(vec![(self.variant.to_string(), inner)]))
    }
}

struct MapSer {
    entries: Vec<(String, Value)>,
    next_key: Option<String>,
}

fn key_to_string(key: Value) -> Result<String, ValueError> {
    match key {
        Value::String(s) => Ok(s),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        other => Err(ValueError(format!(
            "object keys must be strings, got {other}"
        ))),
    }
}

impl ser::SerializeMap for MapSer {
    type Ok = Value;
    type Error = ValueError;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), ValueError> {
        let key = key
            .serialize(serde_json::value::Serializer)
            .map_err(|e| ValueError(e.to_string()))?;
        self.next_key = Some(key_to_string(key)?);
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), ValueError> {
        let key = self
            .next_key
            .take()
            .ok_or_else(|| ValueError("serialize_value called before serialize_key".into()))?;
        self.entries.push((key, value.serialize(WireSerializer)?));
        Ok(())
    }
    fn end(self) -> Result<Value, ValueError> {
        Ok(wire_object(self.entries))
    }
}

impl ser::SerializeStruct for MapSer {
    type Ok = Value;
    type Error = ValueError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), ValueError> {
        self.entries
            .push((key.to_string(), value.serialize(WireSerializer)?));
        Ok(())
    }
    fn end(self) -> Result<Value, ValueError> {
        Ok(wire_object(self.entries))
    }
}

struct VariantMapSer {
    variant: &'static str,
    entries: Vec<(String, Value)>,
}

impl ser::SerializeStructVariant for VariantMapSer {
    type Ok = Value;
    type Error = ValueError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), ValueError> {
        self.entries
            .push((key.to_string(), value.serialize(WireSerializer)?));
        Ok(())
    }
    fn end(self) -> Result<Value, ValueError> {
        let inner = wire_object(self.entries);
        Ok(wire_object(vec![(self.variant.to_string(), inner)]))
    }
}

// ---------------------------------------------------------------------------
// Deserializer
// ---------------------------------------------------------------------------

/// Deserialize a `T` out of a parsed evaluation result.
pub(crate) fn from_js_value<T: de::DeserializeOwned>(value: JsValue) -> Result<T, String> {
    T::deserialize(value).map_err(|e| e.0)
}

fn integral(n: f64) -> bool {
    n.is_finite() && n.fract() == 0.0 && !(n == 0.0 && n.is_sign_negative()) && n.abs() < 9.2e18
}

impl<'de> de::Deserializer<'de> for JsValue {
    type Error = ValueError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ValueError> {
        match self {
            JsValue::Undefined | JsValue::Null => visitor.visit_unit(),
            JsValue::Bool(b) => visitor.visit_bool(b),
            JsValue::Number(n) => {
                if integral(n) {
                    if n >= 0.0 {
                        visitor.visit_u64(n as u64)
                    } else {
                        visitor.visit_i64(n as i64)
                    }
                } else {
                    visitor.visit_f64(n)
                }
            }
            JsValue::String(s) | JsValue::BigInt(s) | JsValue::Date(s) | JsValue::Url(s) => {
                visitor.visit_string(s)
            }
            JsValue::RegExp { source, flags } => visitor.visit_string(format!("/{source}/{flags}")),
            JsValue::Error {
                name,
                message,
                stack,
            } => visitor.visit_map(ObjectAccess::new(vec![
                ("name".to_string(), JsValue::String(name)),
                ("message".to_string(), JsValue::String(message)),
                ("stack".to_string(), JsValue::String(stack)),
            ])),
            JsValue::Array(items) | JsValue::Set(items) => visitor.visit_seq(ArrayAccess {
                iter: items.into_iter(),
            }),
            JsValue::Object(entries) => visitor.visit_map(ObjectAccess::new(entries)),
            JsValue::Map(entries) => visitor.visit_map(MapEntriesAccess {
                iter: entries.into_iter(),
                value: None,
            }),
            v @ (JsValue::TypedArray { .. } | JsValue::ArrayBuffer(_)) => {
                visitor.visit_seq(ArrayAccess {
                    iter: v.typed_elements().into_iter(),
                })
            }
            JsValue::Handle(h) => Err(ValueError(format!(
                "cannot deserialize a handle ({h}) into a Rust value"
            ))),
        }
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ValueError> {
        match self {
            JsValue::Number(n) => visitor.visit_f64(n),
            other => other.deserialize_any(visitor),
        }
    }

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ValueError> {
        self.deserialize_f64(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ValueError> {
        match self {
            JsValue::Undefined | JsValue::Null => visitor.visit_none(),
            other => visitor.visit_some(other),
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ValueError> {
        // Lenient: `()` accepts any result, so `evaluate::<()>` ignores the value.
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, ValueError> {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, ValueError> {
        if name == RAW_JSVALUE {
            STASH.with(|s| *s.borrow_mut() = Some(self));
            return visitor.visit_newtype_struct(de::value::UnitDeserializer::<ValueError>::new());
        }
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ValueError> {
        match self {
            JsValue::ArrayBuffer(bytes) => visitor.visit_byte_buf(bytes),
            JsValue::TypedArray { kind, bytes }
                if kind == "ui8" || kind == "ui8c" || kind == "i8" =>
            {
                visitor.visit_byte_buf(bytes)
            }
            other => other.deserialize_any(visitor),
        }
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ValueError> {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, ValueError> {
        match self {
            JsValue::String(s) => visitor.visit_enum(s.into_deserializer()),
            other => de::Deserializer::deserialize_enum(other.to_json(), name, variants, visitor)
                .map_err(|e| ValueError(e.to_string())),
        }
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 char str string
        seq tuple tuple_struct map struct identifier ignored_any
    }
}

impl<'de> IntoDeserializer<'de, ValueError> for JsValue {
    type Deserializer = JsValue;
    fn into_deserializer(self) -> JsValue {
        self
    }
}

struct ArrayAccess {
    iter: std::vec::IntoIter<JsValue>,
}

impl<'de> SeqAccess<'de> for ArrayAccess {
    type Error = ValueError;
    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, ValueError> {
        match self.iter.next() {
            Some(v) => seed.deserialize(v).map(Some),
            None => Ok(None),
        }
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.iter.len())
    }
}

struct ObjectAccess {
    iter: std::vec::IntoIter<(String, JsValue)>,
    value: Option<JsValue>,
}

impl ObjectAccess {
    fn new(entries: Vec<(String, JsValue)>) -> Self {
        Self {
            iter: entries.into_iter(),
            value: None,
        }
    }
}

impl<'de> MapAccess<'de> for ObjectAccess {
    type Error = ValueError;
    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, ValueError> {
        match self.iter.next() {
            Some((k, v)) => {
                self.value = Some(v);
                let key: de::value::StringDeserializer<ValueError> = k.into_deserializer();
                seed.deserialize(key).map(Some)
            }
            None => Ok(None),
        }
    }
    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, ValueError> {
        seed.deserialize(self.value.take().unwrap_or_default())
    }
}

struct MapEntriesAccess {
    iter: std::vec::IntoIter<(JsValue, JsValue)>,
    value: Option<JsValue>,
}

impl<'de> MapAccess<'de> for MapEntriesAccess {
    type Error = ValueError;
    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, ValueError> {
        match self.iter.next() {
            Some((k, v)) => {
                self.value = Some(v);
                seed.deserialize(k).map(Some)
            }
            None => Ok(None),
        }
    }
    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, ValueError> {
        seed.deserialize(self.value.take().unwrap_or_default())
    }
}

// ---------------------------------------------------------------------------
// Deserialize for JsValue
// ---------------------------------------------------------------------------

impl<'de> de::Deserialize<'de> for JsValue {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_newtype_struct(RAW_JSVALUE, JsValueVisitor)
    }
}

struct JsValueVisitor;

impl<'de> Visitor<'de> for JsValueVisitor {
    type Value = JsValue;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JavaScript value")
    }

    fn visit_newtype_struct<D: de::Deserializer<'de>>(self, d: D) -> Result<JsValue, D::Error> {
        if let Some(value) = STASH.with(|s| s.borrow_mut().take()) {
            return Ok(value);
        }
        d.deserialize_any(JsValueVisitor)
    }
    fn visit_bool<E>(self, v: bool) -> Result<JsValue, E> {
        Ok(JsValue::Bool(v))
    }
    fn visit_i64<E>(self, v: i64) -> Result<JsValue, E> {
        Ok(JsValue::Number(v as f64))
    }
    fn visit_u64<E>(self, v: u64) -> Result<JsValue, E> {
        Ok(JsValue::Number(v as f64))
    }
    fn visit_f64<E>(self, v: f64) -> Result<JsValue, E> {
        Ok(JsValue::Number(v))
    }
    fn visit_str<E>(self, v: &str) -> Result<JsValue, E> {
        Ok(JsValue::String(v.to_string()))
    }
    fn visit_string<E>(self, v: String) -> Result<JsValue, E> {
        Ok(JsValue::String(v))
    }
    fn visit_bytes<E>(self, v: &[u8]) -> Result<JsValue, E> {
        Ok(JsValue::ArrayBuffer(v.to_vec()))
    }
    fn visit_unit<E>(self) -> Result<JsValue, E> {
        Ok(JsValue::Null)
    }
    fn visit_none<E>(self) -> Result<JsValue, E> {
        Ok(JsValue::Null)
    }
    fn visit_some<D: de::Deserializer<'de>>(self, d: D) -> Result<JsValue, D::Error> {
        de::Deserialize::deserialize(d)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<JsValue, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element::<JsValue>()? {
            items.push(item);
        }
        Ok(JsValue::Array(items))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<JsValue, A::Error> {
        let mut entries = Vec::new();
        while let Some((k, v)) = map.next_entry::<String, JsValue>()? {
            entries.push((k, v));
        }
        Ok(JsValue::Object(entries))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[test]
    fn serializes_primitives_like_playwright() {
        assert_eq!(to_wire(&1).unwrap().0, json!(1));
        assert_eq!(to_wire(&f64::NAN).unwrap().0, json!({ "v": "NaN" }));
        assert_eq!(to_wire(&-0.0f64).unwrap().0, json!({ "v": "-0" }));
        assert_eq!(
            to_wire(&Option::<i32>::None).unwrap().0,
            json!({ "v": "null" })
        );
        assert_eq!(to_wire(&"x").unwrap().0, json!("x"));
        assert_eq!(
            to_wire(&JsValue::Undefined).unwrap().0,
            json!({ "v": "undefined" })
        );
    }

    #[test]
    fn serializes_structs_and_arrays() {
        #[derive(Serialize)]
        struct S {
            a: i32,
            b: Vec<bool>,
        }
        let (v, handles) = to_wire(&S {
            a: 1,
            b: vec![true],
        })
        .unwrap();
        assert!(handles.is_empty());
        assert_eq!(
            v,
            json!({ "o": [ { "k": "a", "v": 1 }, { "k": "b", "v": { "a": [true], "id": 1 } } ], "id": 2 })
        );
    }

    #[test]
    fn deserializes_special_numbers() {
        let nan: f64 = from_js_value(JsValue::from_wire(&json!({ "v": "NaN" }))).unwrap();
        assert!(nan.is_nan());
        let neg_zero: f64 = from_js_value(JsValue::from_wire(&json!({ "v": "-0" }))).unwrap();
        assert!(neg_zero == 0.0 && neg_zero.is_sign_negative());
        let inf: f64 = from_js_value(JsValue::from_wire(&json!({ "v": "Infinity" }))).unwrap();
        assert_eq!(inf, f64::INFINITY);
        let none: Option<i32> =
            from_js_value(JsValue::from_wire(&json!({ "v": "undefined" }))).unwrap();
        assert_eq!(none, None);
    }

    #[test]
    fn deserializes_structs() {
        #[derive(Deserialize, Debug, PartialEq)]
        struct S {
            a: i32,
            b: String,
            c: Option<f64>,
        }
        let wire = json!({ "o": [
            { "k": "a", "v": 1 },
            { "k": "b", "v": "x" },
            { "k": "c", "v": { "v": "undefined" } },
        ], "id": 1 });
        let s: S = from_js_value(JsValue::from_wire(&wire)).unwrap();
        assert_eq!(
            s,
            S {
                a: 1,
                b: "x".into(),
                c: None
            }
        );
    }

    #[test]
    fn js_value_round_trips_through_its_own_deserializer() {
        let wire = json!({ "a": [ { "d": "2020-01-01T00:00:00.000Z" }, { "bi": "12" }, { "v": "undefined" } ], "id": 1 });
        let v: JsValue = from_js_value(JsValue::from_wire(&wire)).unwrap();
        assert_eq!(
            v,
            JsValue::Array(vec![
                JsValue::Date("2020-01-01T00:00:00.000Z".into()),
                JsValue::BigInt("12".into()),
                JsValue::Undefined
            ])
        );
        let nested: Vec<JsValue> = from_js_value(JsValue::from_wire(&wire)).unwrap();
        assert_eq!(nested[0], JsValue::Date("2020-01-01T00:00:00.000Z".into()));
        let json: Value = from_js_value(JsValue::from_wire(&wire)).unwrap();
        assert_eq!(json, json!(["2020-01-01T00:00:00.000Z", "12", null]));
    }

    #[test]
    fn js_value_deserializes_from_serde_json() {
        let v: JsValue = serde_json::from_value(json!({ "a": [1, "x", null] })).unwrap();
        assert_eq!(
            v,
            JsValue::Object(vec![(
                "a".into(),
                JsValue::Array(vec![
                    JsValue::Number(1.0),
                    JsValue::String("x".into()),
                    JsValue::Null
                ])
            )])
        );
    }
}
