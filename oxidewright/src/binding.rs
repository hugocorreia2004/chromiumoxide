//! `expose_function` / `expose_binding` (page and context), the way
//! Patchright 1.64.3 does it:
//!
//! * `Runtime.addBinding` only with the user's name, on every session of the
//!   page (main and out-of-process frames). No `Runtime.enable`:
//!   `Runtime.bindingCalled` is reported without it.
//! * The wrapper (`createPageBindingScript` of `server/pageBinding.ts`) is
//!   installed with `Page.addScriptToEvaluateOnNewDocument` in the main world
//!   (`runImmediately`), before later init scripts, and evaluated in the main
//!   world of the frames that already exist.
//! * No Playwright-named global (`__playwright__binding__`) is ever created.
//! * The result goes back with `deliverBindingResult`, evaluated in the main
//!   world of the calling frame. That frame is found by the context id of the
//!   `Runtime.bindingCalled` event, matched against the main-world context of
//!   each frame (created on demand, like Patchright's fallback when the
//!   context is not known).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};

use chromiumoxide::cdp::browser_protocol::page::{
    AddScriptToEvaluateOnNewDocumentParams, RemoveScriptToEvaluateOnNewDocumentParams,
    ScriptIdentifier,
};
use chromiumoxide::cdp::browser_protocol::target::SessionId;
use chromiumoxide::cdp::js_protocol::runtime::{
    AddBindingParams, EventBindingCalled, RemoveBindingParams,
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::browser_context::BrowserContext;
use crate::error::{Error, Result};
use crate::execution_context::World;
use crate::frame::Frame;
use crate::js::JsValue;
use crate::js_handle::JSHandle;
use crate::page::{Page, PageInner};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Serialized result of a binding callback: wire value + handles.
pub(crate) type WireResult = std::result::Result<(Value, Vec<JSHandle>), BindingError>;

/// Type-erased binding callback.
pub(crate) type BindingCallback =
    Arc<dyn Fn(BindingSource, Vec<JsValue>) -> BoxFuture<WireResult> + Send + Sync>;

/// Where a binding was called from (Playwright's `source` argument of
/// `exposeBinding` callbacks).
#[derive(Clone, Debug)]
pub struct BindingSource {
    /// The context of the page, if it still exists.
    pub context: Option<BrowserContext>,
    /// The page that called the binding.
    pub page: Page,
    /// The frame that called the binding.
    pub frame: Frame,
}

/// What a binding callback throws in the page: an `Error` with a message
/// (from `String`, `&str` or [`Error`]) or any value
/// ([`BindingError::value`], e.g. `null`).
#[derive(Clone, Debug)]
pub struct BindingError(pub(crate) JsValue);

impl BindingError {
    /// Throw an `Error` with this message in the page.
    pub fn new(message: impl Into<String>) -> Self {
        let message = message.into();
        BindingError(JsValue::Error {
            name: "Error".to_string(),
            stack: format!("Error: {message}"),
            message,
        })
    }

    /// Throw an arbitrary value in the page (`throw null`, `throw 42`, ...).
    pub fn value(value: JsValue) -> Self {
        BindingError(value)
    }
}

impl From<Error> for BindingError {
    fn from(error: Error) -> Self {
        BindingError::new(error.to_string())
    }
}

impl From<String> for BindingError {
    fn from(message: String) -> Self {
        BindingError::new(message)
    }
}

impl From<&str> for BindingError {
    fn from(message: &str) -> Self {
        BindingError::new(message)
    }
}

/// Wrap a typed callback into a [`BindingCallback`].
pub(crate) fn erase<F, Fut, R, E>(callback: F) -> BindingCallback
where
    F: Fn(BindingSource, Vec<JsValue>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = std::result::Result<R, E>> + Send + 'static,
    R: Serialize + Send + 'static,
    E: Into<BindingError> + Send + 'static,
{
    Arc::new(move |source, args| {
        let fut = callback(source, args);
        Box::pin(async move {
            match fut.await {
                Ok(value) => crate::js::serde_impl::to_wire(&value)
                    .map_err(|e| BindingError::new(format!("Cannot serialize the result: {e}"))),
                Err(e) => Err(e.into()),
            }
        })
    })
}

/// An exposed binding.
pub(crate) struct PageBinding {
    pub(crate) name: String,
    pub(crate) callback: BindingCallback,
    /// `createPageBindingScript(name, false)`.
    pub(crate) source: String,
    /// Init script identifiers, per page.
    scripts: Mutex<Vec<(Weak<PageInner>, ScriptIdentifier)>>,
}

impl PageBinding {
    pub(crate) fn new(name: String, callback: BindingCallback) -> Arc<Self> {
        let source = page_binding_script(&name);
        Arc::new(PageBinding {
            name,
            callback,
            source,
            scripts: Mutex::new(Vec::new()),
        })
    }
}

/// The bindings of a page or a context.
#[derive(Default)]
pub(crate) struct Bindings {
    map: Mutex<HashMap<String, Arc<PageBinding>>>,
}

impl Bindings {
    pub(crate) fn get(&self, name: &str) -> Option<Arc<PageBinding>> {
        self.map.lock().unwrap().get(name).cloned()
    }

    pub(crate) fn has(&self, name: &str) -> bool {
        self.map.lock().unwrap().contains_key(name)
    }

    pub(crate) fn all(&self) -> Vec<Arc<PageBinding>> {
        self.map.lock().unwrap().values().cloned().collect()
    }

    fn insert(&self, binding: Arc<PageBinding>) {
        self.map
            .lock()
            .unwrap()
            .insert(binding.name.clone(), binding);
    }

    fn remove(&self, binding: &Arc<PageBinding>) -> bool {
        let mut map = self.map.lock().unwrap();
        match map.get(&binding.name) {
            Some(existing) if Arc::ptr_eq(existing, binding) => {
                map.remove(&binding.name);
                true
            }
            _ => false,
        }
    }
}

/// The serializers of `utils/isomorphic/oldUtilityScriptSerializers.ts`
/// (`source()`), as built in patchright-core 1.64.3.
const OLD_SERIALIZERS: &str = include_str!("js/page_binding_serializers.js");

/// `addPageBinding` of Patchright's `server/pageBinding.ts`.
const ADD_PAGE_BINDING: &str = r#"function addPageBinding(bindingName, needsHandle, utilityScriptSerializersFactory) {
  const { serializeAsCallArgument: serializeAsCallArgument2 } = utilityScriptSerializersFactory;
  const binding = globalThis[bindingName];
  if (!binding || binding.toString().startsWith("(...args) => {")) return;
  globalThis[bindingName] = (...args) => {
    const me = globalThis[bindingName];
    if (needsHandle && args.slice(1).some((arg2) => arg2 !== void 0))
      throw new Error(`exposeBindingHandle supports a single argument, ${args.length} received`);
    let callbacks = me["callbacks"];
    if (!callbacks) {
      callbacks = /* @__PURE__ */ new Map();
      me["callbacks"] = callbacks;
    }
    const seq = (me["lastSeq"] || 0) + 1;
    me["lastSeq"] = seq;
    let handles = me["handles"];
    if (!handles) {
      handles = /* @__PURE__ */ new Map();
      me["handles"] = handles;
    }
    const promise = new Promise((resolve, reject) => callbacks.set(seq, { resolve, reject }));
    let payload;
    if (needsHandle) {
      handles.set(seq, args[0]);
      payload = { name: bindingName, seq };
    } else {
      const serializedArgs = [];
      for (let i = 0; i < args.length; i++) {
        serializedArgs[i] = serializeAsCallArgument2(args[i], (v) => {
          return { fallThrough: v };
        });
      }
      payload = { name: bindingName, seq, serializedArgs };
    }
    binding(JSON.stringify(payload));
    return promise;
  };
}"#;

/// `deliverBindingResult` of `server/pageBinding.ts`.
const DELIVER_BINDING_RESULT: &str = r#"function deliverBindingResult(arg2) {
  const callbacks = globalThis[arg2.name]["callbacks"];
  if ("error" in arg2) callbacks.get(arg2.seq).reject(arg2.error);
  else callbacks.get(arg2.seq).resolve(arg2.result);
  callbacks.delete(arg2.seq);
}"#;

/// `createPageBindingScript(name, false)`.
pub(crate) fn page_binding_script(name: &str) -> String {
    format!(
        "({ADD_PAGE_BINDING})({}, false, ({OLD_SERIALIZERS})())",
        serde_json::to_string(name).unwrap_or_default()
    )
}

// ----------------------------------------------------------------------
// Installation
// ----------------------------------------------------------------------

/// All CDP sessions of the page (main + out-of-process frames).
fn sessions(page: &PageInner) -> Vec<SessionId> {
    let mut out = vec![page.cdp.session_id().clone()];
    for frame in page.frames_dfs() {
        if let Some(cdp) = frame.cdp_frame_now() {
            let id = cdp.session_id().clone();
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

async fn add_binding(page: &PageInner, session: SessionId, name: &str) {
    let params = AddBindingParams::new(name);
    let result = if &session == page.cdp.session_id() {
        page.cdp
            .execute(params)
            .await
            .map(|_| ())
            .map_err(Error::from)
    } else {
        crate::network_manager::send(page, Some(session), params)
            .await
            .map(|_| ())
    };
    if let Err(err) = result {
        tracing::debug!("Runtime.addBinding failed: {err}");
    }
}

/// Evaluate `expression` in the main world of every frame, ignoring errors
/// (`safeNonStallingEvaluateInAllFrames`).
async fn evaluate_in_all_frames(frames: Vec<Frame>, expression: &str) {
    let futures = frames.into_iter().map(|frame| {
        let expression = expression.to_string();
        async move {
            let run = async {
                let ctx = frame.context(World::Main).await?;
                let object = ctx.raw_evaluate_handle(&expression).await?;
                if let Some(id) = object.object_id {
                    ctx.release_object(id).await;
                }
                Ok::<_, Error>(())
            };
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), run).await;
        }
    });
    futures::future::join_all(futures).await;
}

/// `CRPage.exposeBinding`: the binding in every session, the wrapper as an
/// init script (main world, `runImmediately`) and in the existing frames.
pub(crate) async fn install_on_page(
    page: &Arc<PageInner>,
    binding: &Arc<PageBinding>,
) -> Result<()> {
    for session in sessions(page) {
        add_binding(page, session, &binding.name).await;
    }
    let mut params = AddScriptToEvaluateOnNewDocumentParams::new(binding.source.clone());
    params.run_immediately = Some(true);
    let id = page.cdp.evaluate_on_new_document(params).await?;
    binding
        .scripts
        .lock()
        .unwrap()
        .push((Arc::downgrade(page), id));
    evaluate_in_all_frames(page.frames_dfs(), &binding.source).await;
    Ok(())
}

/// A new page of a context: install the context's bindings (before its init
/// scripts, like Patchright's `_initialize`).
pub(crate) async fn init_page(page: &Page, context: &BrowserContext) -> Result<()> {
    for binding in context.inner.ext.bindings.all() {
        install_on_page(&page.inner, &binding).await?;
    }
    Ok(())
}

/// The session of an out-of-process frame became known: add the bindings
/// there and install the wrappers in its frames (its first document ran the
/// init scripts before the binding existed).
pub(crate) async fn on_child_session(page: &Arc<PageInner>, session: SessionId) {
    let bindings = all_bindings(page);
    if bindings.is_empty() {
        return;
    }
    for binding in &bindings {
        add_binding(page, session.clone(), &binding.name).await;
    }
    let frames: Vec<Frame> = page
        .frames_dfs()
        .into_iter()
        .filter(|f| {
            f.cdp_frame_now()
                .is_some_and(|c| c.session_id() == &session)
        })
        .collect();
    for binding in &bindings {
        evaluate_in_all_frames(frames.clone(), &binding.source).await;
    }
}

/// A main-world context was created (new document): add the bindings to
/// it and install their wrappers (Patchright's `_onExecutionContextCreated`,
/// reached here through the on-demand context creation).
pub(crate) async fn on_main_context(
    frame: &Frame,
    ctx: &crate::execution_context::ExecutionContext,
) {
    let Some(page) = frame.page_inner() else {
        return;
    };
    let bindings = all_bindings(&page);
    if bindings.is_empty() {
        return;
    }
    let Some(context_id) = ctx.inner.context_id else {
        return;
    };
    for binding in &bindings {
        let params = json!({ "name": binding.name, "executionContextId": context_id });
        let _ = ctx
            .send(crate::cdp_raw::RawCommand::new(
                "Runtime.addBinding",
                params,
            ))
            .await;
    }
    for binding in &bindings {
        if let Ok(object) = ctx.raw_evaluate_handle(&binding.source).await {
            if let Some(id) = object.object_id {
                ctx.release_object(id).await;
            }
        }
    }
}

/// A frame committed a new document: create its main-world context now so
/// the bindings are installed (Patchright creates the contexts on commit).
pub(crate) fn on_new_document(page: &Arc<PageInner>, frame: &Frame) {
    if all_bindings(page).is_empty() {
        return;
    }
    let frame = frame.clone();
    tokio::spawn(async move {
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            frame.context(World::Main),
        )
        .await;
    });
}

fn all_bindings(page: &PageInner) -> Vec<Arc<PageBinding>> {
    let mut out = page.ext.bindings.all();
    if let Some(context) = page.context.upgrade() {
        out.extend(context.ext.bindings.all());
    }
    out
}

fn find_binding(page: &PageInner, name: &str) -> Option<Arc<PageBinding>> {
    page.ext.bindings.get(name).or_else(|| {
        page.context
            .upgrade()
            .and_then(|context| context.ext.bindings.get(name))
    })
}

async fn uninstall_from_page(page: &Arc<PageInner>, binding: &Arc<PageBinding>) {
    let ids: Vec<ScriptIdentifier> = {
        let mut scripts = binding.scripts.lock().unwrap();
        let mut ids = Vec::new();
        scripts.retain(|(p, id)| {
            let same = p.upgrade().is_some_and(|p| Arc::ptr_eq(&p, page));
            if same {
                ids.push(id.clone());
            }
            !same && p.strong_count() > 0
        });
        ids
    };
    for id in ids {
        let _ = page
            .cdp
            .execute(RemoveScriptToEvaluateOnNewDocumentParams::new(id))
            .await;
    }
    // New documents must not get the raw binding either.
    for session in sessions(page) {
        let params = RemoveBindingParams::new(binding.name.clone());
        let _ = if &session == page.cdp.session_id() {
            page.cdp
                .execute(params)
                .await
                .map(|_| ())
                .map_err(Error::from)
        } else {
            crate::network_manager::send(page, Some(session), params)
                .await
                .map(|_| ())
        };
    }
    let cleanup = format!(
        "delete globalThis[{}]",
        serde_json::to_string(&binding.name).unwrap_or_default()
    );
    evaluate_in_all_frames(page.frames_dfs(), &cleanup).await;
}

// ----------------------------------------------------------------------
// Public registration
// ----------------------------------------------------------------------

pub(crate) async fn expose_on_page(
    page: &Page,
    name: String,
    callback: BindingCallback,
) -> Result<ExposedBinding> {
    if page.inner.ext.bindings.has(&name) {
        return Err(Error::invalid_argument(format!(
            "Function \"{name}\" has been already registered"
        )));
    }
    if let Some(context) = page.inner.context.upgrade() {
        if context.ext.bindings.has(&name) {
            return Err(Error::invalid_argument(format!(
                "Function \"{name}\" has been already registered in the browser context"
            )));
        }
    }
    let binding = PageBinding::new(name, callback);
    page.inner.ext.bindings.insert(binding.clone());
    if let Err(err) = install_on_page(&page.inner, &binding).await {
        page.inner.ext.bindings.remove(&binding);
        return Err(err);
    }
    Ok(ExposedBinding {
        owner: Owner::Page(Arc::downgrade(&page.inner)),
        binding,
    })
}

pub(crate) async fn expose_on_context(
    context: &BrowserContext,
    name: String,
    callback: BindingCallback,
) -> Result<ExposedBinding> {
    let bindings = &context.inner.ext.bindings;
    if bindings.has(&name) {
        return Err(Error::invalid_argument(format!(
            "Function \"{name}\" has been already registered"
        )));
    }
    for page in context.pages() {
        if page.inner.ext.bindings.has(&name) {
            return Err(Error::invalid_argument(format!(
                "Function \"{name}\" has been already registered in one of the pages"
            )));
        }
    }
    let binding = PageBinding::new(name, callback);
    bindings.insert(binding.clone());
    for page in context.pages() {
        if let Err(err) = install_on_page(&page.inner, &binding).await {
            if !page.is_closed() {
                bindings.remove(&binding);
                return Err(err);
            }
        }
    }
    Ok(ExposedBinding {
        owner: Owner::Context(Arc::downgrade(&context.inner)),
        binding,
    })
}

enum Owner {
    Page(Weak<PageInner>),
    Context(Weak<crate::browser_context::ContextInner>),
}

/// A function exposed with `expose_function` / `expose_binding`
/// (Playwright's `Disposable`); [`ExposedBinding::dispose`] removes it.
pub struct ExposedBinding {
    owner: Owner,
    binding: Arc<PageBinding>,
}

impl std::fmt::Debug for ExposedBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExposedBinding")
            .field("name", &self.binding.name)
            .finish()
    }
}

impl ExposedBinding {
    /// The exposed name.
    pub fn name(&self) -> &str {
        &self.binding.name
    }

    /// Remove the function: its wrapper stops being installed in new
    /// documents and `globalThis[name]` is deleted in the existing ones.
    pub async fn dispose(self) -> Result<()> {
        match &self.owner {
            Owner::Page(page) => {
                if let Some(page) = page.upgrade() {
                    if page.ext.bindings.remove(&self.binding) {
                        uninstall_from_page(&page, &self.binding).await;
                    }
                }
            }
            Owner::Context(context) => {
                if let Some(context) = context.upgrade() {
                    if context.ext.bindings.remove(&self.binding) {
                        let pages = context.pages.lock().unwrap().clone();
                        for page in pages {
                            if !page.is_closed() {
                                uninstall_from_page(&page.inner, &self.binding).await;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

// ----------------------------------------------------------------------
// Dispatch
// ----------------------------------------------------------------------

/// `Runtime.bindingCalled` (any session of the page).
pub(crate) fn on_binding_called(page: &PageInner, event: &EventBindingCalled) {
    let Some(page) = page.self_weak.upgrade() else {
        return;
    };
    let name = event.name.clone();
    let payload = event.payload.clone();
    let context_id = *event.execution_context_id.inner();
    tokio::spawn(async move {
        dispatch(page, name, payload, context_id).await;
    });
}

/// The frame whose main world has `context_id`.
async fn frame_for_context(page: &Arc<PageInner>, context_id: i64) -> Option<Frame> {
    let frames = page.frames_dfs();
    let matches = |frame: &Frame| {
        frame
            .cached_context(World::Main)
            .and_then(|c| c.inner.context_id)
            .is_some_and(|id| *id.inner() == context_id)
    };
    if let Some(frame) = frames.iter().find(|f| matches(f)) {
        return Some(frame.clone());
    }
    // Patchright: "Ensure execution contexts for all frames are resolved".
    let contexts = futures::future::join_all(frames.iter().map(|f| async move {
        tokio::time::timeout(std::time::Duration::from_secs(5), f.context(World::Main))
            .await
            .ok()
            .and_then(|r| r.ok())
    }))
    .await;
    for (frame, ctx) in frames.iter().zip(contexts) {
        if ctx
            .and_then(|c| c.inner.context_id)
            .is_some_and(|id| *id.inner() == context_id)
        {
            return Some(frame.clone());
        }
    }
    None
}

async fn dispatch(page: Arc<PageInner>, name: String, payload: String, context_id: i64) {
    let Ok(parsed) = serde_json::from_str::<Value>(&payload) else {
        return;
    };
    let Some(seq) = parsed.get("seq").cloned() else {
        return;
    };
    let payload_name = parsed
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(&name)
        .to_string();
    let frame = match frame_for_context(&page, context_id).await {
        Some(frame) => frame,
        // Last resort fallback (Patchright): the main frame.
        None => page.main_frame(),
    };
    let result: WireResult = async {
        let binding = find_binding(&page, &payload_name).ok_or_else(|| {
            BindingError::new(format!("Function \"{payload_name}\" is not exposed"))
        })?;
        let Some(args) = parsed.get("serializedArgs").and_then(Value::as_array) else {
            return Err(BindingError::new(
                "serializedArgs is not an array. This can happen when Array.prototype.toJSON is defined incorrectly",
            ));
        };
        let args: Vec<JsValue> = args.iter().map(JsValue::from_wire).collect();
        let source = BindingSource {
            context: page.context.upgrade().map(|inner| BrowserContext { inner }),
            page: Page::from_inner(page.clone()),
            frame: frame.clone(),
        };
        (binding.callback)(source, args).await
    }
    .await;
    deliver(&frame, &payload_name, seq, result).await;
}

async fn deliver(frame: &Frame, name: &str, seq: Value, result: WireResult) {
    let (key, (value, handles)) = match result {
        Ok(wire) => ("result", wire),
        Err(BindingError(thrown)) => {
            let mut ids = 0u64;
            let wire = thrown.to_wire(&mut ids, &mut |_| json!({ "v": "undefined" }));
            ("error", (wire, Vec::new()))
        }
    };
    let arg = json!({
        "o": [
            { "k": "name", "v": name },
            { "k": "seq", "v": seq },
            { "k": key, "v": value },
        ],
        "id": 0,
    });
    let Ok(ctx) = frame.context(World::Main).await else {
        return;
    };
    if let Err(err) = ctx
        .evaluate_expression(DELIVER_BINDING_RESULT, Some(true), true, vec![arg], handles)
        .await
    {
        tracing::debug!("failed to deliver a binding result: {err}");
    }
}

// ----------------------------------------------------------------------
// Public API
// ----------------------------------------------------------------------

impl Page {
    /// Expose `callback` to the page as `window[name]` (Playwright's
    /// `page.exposeFunction`). Calls from the page resolve with the value the
    /// callback returns (serialized like `evaluate` arguments, handles
    /// included) or reject with its [`BindingError`]. The function survives
    /// navigations and is installed in every frame.
    ///
    /// Stealth: like Patchright, only `Runtime.addBinding` with `name` and a
    /// main-world wrapper; no `Runtime.enable` and no other global.
    ///
    /// ```no_run
    /// # async fn run(page: oxidewright::Page) -> oxidewright::Result<()> {
    /// page.expose_function("compute", |args| async move {
    ///     let a = args[0].as_f64().unwrap_or_default();
    ///     let b = args[1].as_f64().unwrap_or_default();
    ///     Ok::<_, oxidewright::BindingError>(a * b)
    /// })
    /// .await?;
    /// let r: f64 = page.evaluate("() => window.compute(9, 4)").await?;
    /// # let _ = r; Ok(()) }
    /// ```
    pub async fn expose_function<F, Fut, R, E>(
        &self,
        name: impl Into<String>,
        callback: F,
    ) -> Result<ExposedBinding>
    where
        F: Fn(Vec<JsValue>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = std::result::Result<R, E>> + Send + 'static,
        R: Serialize + Send + 'static,
        E: Into<BindingError> + Send + 'static,
    {
        use crate::error::ResultExt;
        let callback = erase(move |_source, args| callback(args));
        expose_on_page(self, name.into(), callback)
            .await
            .api("page.exposeFunction")
    }

    /// Like [`Page::expose_function`], the callback also gets the
    /// [`BindingSource`] (page, frame and context of the call), like
    /// Playwright's `page.exposeBinding`.
    pub async fn expose_binding<F, Fut, R, E>(
        &self,
        name: impl Into<String>,
        callback: F,
    ) -> Result<ExposedBinding>
    where
        F: Fn(BindingSource, Vec<JsValue>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = std::result::Result<R, E>> + Send + 'static,
        R: Serialize + Send + 'static,
        E: Into<BindingError> + Send + 'static,
    {
        use crate::error::ResultExt;
        expose_on_page(self, name.into(), erase(callback))
            .await
            .api("page.exposeBinding")
    }
}

impl BrowserContext {
    /// [`Page::expose_function`] for every page of the context, current and
    /// future (Playwright's `browserContext.exposeFunction`).
    pub async fn expose_function<F, Fut, R, E>(
        &self,
        name: impl Into<String>,
        callback: F,
    ) -> Result<ExposedBinding>
    where
        F: Fn(Vec<JsValue>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = std::result::Result<R, E>> + Send + 'static,
        R: Serialize + Send + 'static,
        E: Into<BindingError> + Send + 'static,
    {
        use crate::error::ResultExt;
        let callback = erase(move |_source, args| callback(args));
        expose_on_context(self, name.into(), callback)
            .await
            .api("browserContext.exposeFunction")
    }

    /// [`Page::expose_binding`] for every page of the context, current and
    /// future (Playwright's `browserContext.exposeBinding`).
    pub async fn expose_binding<F, Fut, R, E>(
        &self,
        name: impl Into<String>,
        callback: F,
    ) -> Result<ExposedBinding>
    where
        F: Fn(BindingSource, Vec<JsValue>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = std::result::Result<R, E>> + Send + 'static,
        R: Serialize + Send + 'static,
        E: Into<BindingError> + Send + 'static,
    {
        use crate::error::ResultExt;
        expose_on_context(self, name.into(), erase(callback))
            .await
            .api("browserContext.exposeBinding")
    }
}
