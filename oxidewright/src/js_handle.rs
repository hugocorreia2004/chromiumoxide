//! [`JSHandle`] and [`ElementHandle`].

use std::collections::HashMap;
use std::fmt;
use std::future::{Future, IntoFuture};
use std::marker::PhantomData;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use chromiumoxide::cdp::browser_protocol::dom::DescribeNodeParams;
use chromiumoxide::cdp::js_protocol::runtime::{
    GetPropertiesParams, RemoteObject, RemoteObjectId, RemoteObjectSubtype, RemoteObjectType,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::{Error, Result, ResultExt};
use crate::execution_context::{Evaluated, ExecutionContext};
use crate::frame::Frame;
use crate::js::JsValue;
use crate::js::serde_impl::{from_js_value, to_wire};
use crate::js::value::format_js_number;

/// A handle to a JavaScript value living in the page.
///
/// Equivalent of Playwright's `JSHandle`. Cheap to clone; clones refer to the
/// same remote object. Remote objects are only released by [`JSHandle::dispose`].
#[derive(Clone)]
pub struct JSHandle {
    inner: Arc<HandleInner>,
}

struct HandleInner {
    context: ExecutionContext,
    object_id: Option<RemoteObjectId>,
    /// The value itself when the handle has no remote object (primitives).
    value: JsValue,
    is_node: bool,
    preview: Mutex<String>,
    disposed: AtomicBool,
}

impl JSHandle {
    pub(crate) fn from_remote(context: ExecutionContext, remote: RemoteObject) -> Self {
        let handle = Self::from_remote_quiet(context, remote);
        if handle.inner.is_node && handle.inner.object_id.is_some() {
            handle.initialize_node_preview();
        }
        handle
    }

    /// Like `from_remote`, without computing the node preview (temporary handles).
    pub(crate) fn from_remote_quiet(context: ExecutionContext, remote: RemoteObject) -> Self {
        let is_node = remote.subtype == Some(RemoteObjectSubtype::Node);
        let value = potentially_unserializable_value(&remote);
        let preview = if remote.object_id.is_some() {
            if is_node {
                "JSHandle@node".to_string()
            } else {
                render_preview(&remote)
                    .unwrap_or_else(|| format!("JSHandle@{}", object_type(&remote)))
            }
        } else {
            value.to_string()
        };
        JSHandle {
            inner: Arc::new(HandleInner {
                context,
                object_id: remote.object_id,
                value,
                is_node,
                preview: Mutex::new(preview),
                disposed: AtomicBool::new(false),
            }),
        }
    }

    /// Like Playwright's `ElementHandle._initializePreview`: compute
    /// `JSHandle@<tag ...>` in the background.
    pub(crate) fn initialize_node_preview(&self) {
        let handle = self.clone();
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        tokio::spawn(async move {
            let Ok(injected) = handle.context().injected_script().await else {
                return;
            };
            let preview = handle
                .context()
                .evaluate_expression(
                    "(injected, node) => 'JSHandle@' + injected.previewNode(node)",
                    Some(true),
                    true,
                    vec![serde_json::json!({ "h": 0 }), serde_json::json!({ "h": 1 })],
                    vec![injected, handle.clone()],
                )
                .await;
            if let Ok(Evaluated::Value(JsValue::String(preview))) = preview {
                handle.set_preview(&preview);
            }
        });
    }

    pub(crate) fn context(&self) -> &ExecutionContext {
        &self.inner.context
    }

    pub(crate) fn object_id(&self) -> Option<&RemoteObjectId> {
        self.inner.object_id.as_ref()
    }

    /// The value of a handle without a remote object (primitives).
    pub(crate) fn inline_value(&self) -> Option<&JsValue> {
        if self.inner.object_id.is_none() {
            Some(&self.inner.value)
        } else {
            None
        }
    }

    pub(crate) fn is_disposed(&self) -> bool {
        self.inner.disposed.load(Ordering::SeqCst)
    }

    pub(crate) fn set_preview(&self, preview: &str) {
        *self.inner.preview.lock().unwrap() = preview.to_string();
    }

    /// Whether two handles point to the same remote object.
    pub fn same_object(&self, other: &JSHandle) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
            || (self.inner.object_id.is_some() && self.inner.object_id == other.inner.object_id)
    }

    /// Evaluate `expression` with this handle as its first argument.
    ///
    /// `handle.evaluate("(node, arg) => node.textContent + arg").arg("!")`
    pub fn evaluate<T: DeserializeOwned>(&self, expression: impl Into<String>) -> EvaluateOn<T> {
        EvaluateOn {
            handle: self.clone(),
            expression: expression.into(),
            arg: Ok(None),
            api: "jsHandle.evaluate",
            _marker: PhantomData,
        }
    }

    /// Like [`JSHandle::evaluate`] but returns a handle to the result.
    pub fn evaluate_handle(&self, expression: impl Into<String>) -> EvaluateHandleOn {
        EvaluateHandleOn {
            handle: self.clone(),
            expression: expression.into(),
            arg: Ok(None),
            api: "jsHandle.evaluateHandle",
        }
    }

    pub(crate) async fn evaluate_inner(
        &self,
        expression: &str,
        arg: Option<(Value, Vec<JSHandle>)>,
        return_by_value: bool,
    ) -> Result<Evaluated> {
        let mut handles = vec![self.clone()];
        let mut args = vec![serde_json::json!({ "h": 0 })];
        match arg {
            Some((value, arg_handles)) => {
                args.push(rebase_handles(value, handles.len()));
                handles.extend(arg_handles);
            }
            None => args.push(serde_json::json!({ "v": "undefined" })),
        }
        self.context()
            .evaluate_expression(expression, None, return_by_value, args, handles)
            .await
    }

    /// Fetch a single property of the referenced object.
    pub async fn get_property(&self, name: &str) -> Result<JSHandle> {
        async {
            let object = self
                .evaluate_inner(
                    "(object, propertyName) => { const result = { __proto__: null }; result[propertyName] = object[propertyName]; return result; }",
                    Some((Value::String(name.to_string()), Vec::new())),
                    false,
                )
                .await?;
            let Evaluated::Handle(object) = object else {
                return Err(Error::evaluation("unexpected evaluation result"));
            };
            let mut properties = object.properties().await?;
            object.dispose().await;
            Ok(properties.remove(name).unwrap_or_else(|| {
                JSHandle::from_remote(
                    self.context().clone(),
                    RemoteObject::new(RemoteObjectType::Undefined),
                )
            }))
        }
        .await
        .api("jsHandle.getProperty")
    }

    /// Own enumerable properties of the referenced object.
    pub async fn get_properties(&self) -> Result<HashMap<String, JSHandle>> {
        self.properties().await.api("jsHandle.getProperties")
    }

    async fn properties(&self) -> Result<HashMap<String, JSHandle>> {
        let mut result = HashMap::new();
        let Some(object_id) = self.inner.object_id.clone() else {
            return Ok(result);
        };
        let mut params = GetPropertiesParams::new(object_id);
        params.own_properties = Some(true);
        let response = self.context().send(params).await?;
        for property in response.result {
            if !property.enumerable {
                continue;
            }
            if let Some(value) = property.value {
                result.insert(
                    property.name,
                    JSHandle::from_remote(self.context().clone(), value),
                );
            }
        }
        Ok(result)
    }

    /// JSON-like representation of the referenced value.
    pub async fn json_value<T: DeserializeOwned>(&self) -> Result<T> {
        async {
            let value = match &self.inner.object_id {
                None => self.inner.value.clone(),
                Some(_) => {
                    let evaluated = self
                        .context()
                        .call_utility(
                            "(utilityScript, ...args) => utilityScript.jsonValue(...args)",
                            true,
                            vec![Some(Value::Bool(true))],
                            vec![self.clone()],
                        )
                        .await?;
                    match evaluated {
                        Evaluated::Value(v) => v,
                        Evaluated::Handle(_) => JsValue::Undefined,
                    }
                }
            };
            from_js_value(value).map_err(Error::serialization)
        }
        .await
        .api("jsHandle.jsonValue")
    }

    /// Release the remote object. Further use of the handle fails.
    pub async fn dispose(&self) {
        if self.inner.disposed.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(object_id) = self.inner.object_id.clone() {
            self.context().release_object(object_id).await;
        }
    }

    /// This handle as an [`ElementHandle`] if it references a DOM node.
    pub fn as_element(&self) -> Option<ElementHandle> {
        if self.inner.is_node {
            Some(ElementHandle {
                handle: self.clone(),
            })
        } else {
            None
        }
    }

    /// Whether the handle references a DOM node.
    pub fn is_element(&self) -> bool {
        self.inner.is_node
    }
}

impl fmt::Display for JSHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.inner.preview.lock().unwrap())
    }
}

impl fmt::Debug for JSHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JSHandle")
            .field("object_id", &self.inner.object_id)
            .field("preview", &*self.inner.preview.lock().unwrap())
            .finish()
    }
}

/// Shift `{ h: n }` handle references of a serialized argument by `offset`.
pub(crate) fn rebase_handles(value: Value, offset: usize) -> Value {
    if offset == 0 {
        return value;
    }
    match value {
        Value::Object(mut map) => {
            if map.len() == 1 {
                if let Some(h) = map.get("h").and_then(Value::as_u64) {
                    map.insert("h".into(), Value::from(h + offset as u64));
                    return Value::Object(map);
                }
            }
            Value::Object(
                map.into_iter()
                    .map(|(k, v)| (k, rebase_handles(v, offset)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|v| rebase_handles(v, offset))
                .collect(),
        ),
        other => other,
    }
}

/// Serialize an `evaluate` argument.
pub(crate) fn serialize_arg<A: Serialize + ?Sized>(
    arg: &A,
) -> Result<Option<(Value, Vec<JSHandle>)>> {
    to_wire(arg).map(Some).map_err(Error::serialization)
}

fn object_type(remote: &RemoteObject) -> String {
    match &remote.subtype {
        Some(subtype) => subtype.as_ref().to_string(),
        None => remote.r#type.as_ref().to_string(),
    }
}

/// Mirrors `potentiallyUnserializableValue` of `crExecutionContext.ts`.
fn potentially_unserializable_value(remote: &RemoteObject) -> JsValue {
    if let Some(unserializable) = &remote.unserializable_value {
        let s: &str = unserializable.as_ref();
        return match s {
            "NaN" => JsValue::Number(f64::NAN),
            "Infinity" => JsValue::Number(f64::INFINITY),
            "-Infinity" => JsValue::Number(f64::NEG_INFINITY),
            "-0" => JsValue::Number(-0.0),
            other => match other.strip_suffix('n') {
                Some(bigint) => JsValue::BigInt(bigint.to_string()),
                None => JsValue::Undefined,
            },
        };
    }
    if remote.r#type == RemoteObjectType::Undefined {
        return JsValue::Undefined;
    }
    match &remote.value {
        Some(value) => JsValue::from(value.clone()),
        None if remote.subtype == Some(RemoteObjectSubtype::Null) => JsValue::Null,
        None => JsValue::Undefined,
    }
}

/// Mirrors `renderPreview` of `crExecutionContext.ts`.
fn render_preview(remote: &RemoteObject) -> Option<String> {
    if remote.r#type == RemoteObjectType::Undefined {
        return Some("undefined".to_string());
    }
    if let Some(value) = &remote.value {
        return Some(match value {
            Value::String(s) => s.clone(),
            Value::Number(n) => format_js_number(n.as_f64().unwrap_or(f64::NAN)),
            other => other.to_string(),
        });
    }
    if let Some(unserializable) = &remote.unserializable_value {
        return Some(unserializable.as_ref().to_string());
    }
    if remote.description.as_deref() == Some("Object") {
        if let Some(preview) = &remote.preview {
            let tokens: Vec<String> = preview
                .properties
                .iter()
                .map(|p| format!("{}: {}", p.name, p.value.clone().unwrap_or_default()))
                .collect();
            return Some(format!("{{{}}}", tokens.join(", ")));
        }
    }
    remote.description.clone()
}

/// A handle to a DOM node. Dereferences to [`JSHandle`].
#[derive(Clone)]
pub struct ElementHandle {
    handle: JSHandle,
}

impl Deref for ElementHandle {
    type Target = JSHandle;
    fn deref(&self) -> &JSHandle {
        &self.handle
    }
}

impl fmt::Display for ElementHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.handle, f)
    }
}

impl fmt::Debug for ElementHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ElementHandle").field(&self.handle).finish()
    }
}

impl Serialize for ElementHandle {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.handle.serialize(serializer)
    }
}

impl From<ElementHandle> for JSHandle {
    fn from(e: ElementHandle) -> Self {
        e.handle
    }
}

impl ElementHandle {
    /// The underlying [`JSHandle`].
    pub fn as_js_handle(&self) -> &JSHandle {
        &self.handle
    }

    async fn describe(&self) -> Result<chromiumoxide::cdp::browser_protocol::dom::Node> {
        let object_id = self
            .object_id()
            .cloned()
            .ok_or_else(|| Error::evaluation("Element handle has no remote object"))?;
        let mut params = DescribeNodeParams::default();
        params.object_id = Some(object_id);
        Ok(self.context().send(params).await?.node)
    }

    /// The value of an attribute, `None` if the attribute is absent.
    pub async fn get_attribute(&self, name: &str) -> Result<Option<String>> {
        async {
            match self
                .evaluate_inner(
                    "(element, name) => element.nodeType === Node.ELEMENT_NODE ? element.getAttribute(name) : null",
                    Some((Value::String(name.to_string()), Vec::new())),
                    true,
                )
                .await?
            {
                Evaluated::Value(v) => from_js_value(v).map_err(Error::serialization),
                Evaluated::Handle(_) => Ok(None),
            }
        }
        .await
        .api("elementHandle.getAttribute")
    }

    /// The frame this `<iframe>`/`<frame>` element hosts, if any.
    pub async fn content_frame(&self) -> Result<Option<Frame>> {
        async {
            // `_isIframeElement`: only <iframe>/<frame> elements host frames.
            let is_frame = self
                .evaluate_inner(
                    "node => !!node && (node.nodeName === 'IFRAME' || node.nodeName === 'FRAME')",
                    None,
                    true,
                )
                .await?;
            if !matches!(is_frame, Evaluated::Value(JsValue::Bool(true))) {
                return Ok(None);
            }
            let node = self.describe().await?;
            let Some(frame_id) = node.frame_id else {
                return Ok(None);
            };
            let Some(page) = self.context().frame().and_then(|f| f.page_inner()) else {
                return Ok(None);
            };
            Ok(page.frame_by_id(frame_id.as_ref()))
        }
        .await
        .api("elementHandle.contentFrame")
    }

    /// The frame containing this element.
    pub async fn owner_frame(&self) -> Result<Option<Frame>> {
        async {
            let document_element = self
                .evaluate_inner(
                    "(node) => { const doc = node; if (doc.documentElement && doc.documentElement.ownerDocument === doc) return doc.documentElement; return node.ownerDocument ? node.ownerDocument.documentElement : null; }",
                    None,
                    false,
                )
                .await?;
            let Evaluated::Handle(document_element) = document_element else {
                return Ok(None);
            };
            let Some(element) = document_element.as_element() else {
                document_element.dispose().await;
                return Ok(None);
            };
            let node = element.describe().await;
            document_element.dispose().await;
            let node = node?;
            let Some(frame_id) = node.frame_id else {
                return Ok(None);
            };
            let Some(page) = self.context().frame().and_then(|f| f.page_inner()) else {
                return Ok(None);
            };
            Ok(page.frame_by_id(frame_id.as_ref()))
        }
        .await
        .api("elementHandle.ownerFrame")
    }
}

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Builder returned by [`JSHandle::evaluate`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct EvaluateOn<T> {
    handle: JSHandle,
    expression: String,
    arg: Result<Option<(Value, Vec<JSHandle>)>>,
    api: &'static str,
    _marker: PhantomData<fn() -> T>,
}

impl<T> EvaluateOn<T> {
    /// The argument passed after the handle.
    pub fn arg<A: Serialize>(mut self, arg: A) -> Self {
        self.arg = serialize_arg(&arg);
        self
    }
}

impl<T: DeserializeOwned + Send + 'static> IntoFuture for EvaluateOn<T> {
    type Output = Result<T>;
    type IntoFuture = BoxFuture<Result<T>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let api = self.api;
            async move {
                let arg = self.arg?;
                match self
                    .handle
                    .evaluate_inner(&self.expression, arg, true)
                    .await?
                {
                    Evaluated::Value(v) => from_js_value(v).map_err(Error::serialization),
                    Evaluated::Handle(_) => Err(Error::evaluation("unexpected handle result")),
                }
            }
            .await
            .api(api)
        })
    }
}

/// Builder returned by [`JSHandle::evaluate_handle`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct EvaluateHandleOn {
    handle: JSHandle,
    expression: String,
    arg: Result<Option<(Value, Vec<JSHandle>)>>,
    api: &'static str,
}

impl EvaluateHandleOn {
    /// The argument passed after the handle.
    pub fn arg<A: Serialize>(mut self, arg: A) -> Self {
        self.arg = serialize_arg(&arg);
        self
    }
}

impl IntoFuture for EvaluateHandleOn {
    type Output = Result<JSHandle>;
    type IntoFuture = BoxFuture<Result<JSHandle>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let api = self.api;
            async move {
                let arg = self.arg?;
                match self
                    .handle
                    .evaluate_inner(&self.expression, arg, false)
                    .await?
                {
                    Evaluated::Handle(h) => Ok(h),
                    Evaluated::Value(_) => Err(Error::evaluation("unexpected value result")),
                }
            }
            .await
            .api(api)
        })
    }
}
