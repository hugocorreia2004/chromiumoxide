//! Dedicated workers of a page (`worker` event).
//!
//! Workers are reported from the auto-attached `worker` targets. Evaluation
//! goes through the worker's `globalThis` (`Runtime.callFunctionOn`, no
//! `Runtime.enable`) with Playwright's serializer, so handles work too.

use std::fmt;
use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use chromiumoxide::cdp::browser_protocol::target::{EventAttachedToTarget, SessionId};
use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::console::ConsoleMessage;
use crate::error::{Error, Result, ResultExt};
use crate::events::Event;
use crate::execution_context::{ContextSession, ContextTarget, Evaluated, ExecutionContext, World};
use crate::js::serde_impl::from_js_value;
use crate::js_handle::{JSHandle, serialize_arg};
use crate::network_manager::send;
use crate::page::{Page, PageInner};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// A `console` handler of a worker.
type ConsoleHandler = Arc<dyn Fn(ConsoleMessage) + Send + Sync>;

/// A dedicated web worker. Cheap to clone.
#[derive(Clone)]
pub struct Worker {
    inner: Arc<WorkerInner>,
}

struct WorkerInner {
    url: String,
    session: SessionId,
    page: Weak<PageInner>,
    /// Commands go through the browser connection (service workers have
    /// no page).
    browser: Weak<crate::browser::BrowserInner>,
    closed: AtomicBool,
    attached_at: std::time::Instant,
    close_handlers: Mutex<Vec<Arc<dyn Fn() + Send + Sync>>>,
    close_tx: tokio::sync::watch::Sender<bool>,
    console_handlers: Mutex<Vec<ConsoleHandler>>,
    context: tokio::sync::OnceCell<ExecutionContext>,
}

impl fmt::Debug for Worker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Worker")
            .field("url", &self.inner.url)
            .finish()
    }
}

impl PartialEq for Worker {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Worker {
    /// The worker script URL.
    pub fn url(&self) -> &str {
        &self.inner.url
    }

    /// The page owning the worker.
    pub fn page(&self) -> Option<Page> {
        self.inner.page.upgrade().map(Page::from_inner)
    }

    /// Whether the worker was terminated.
    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst)
    }

    /// Call `handler` when the worker is terminated.
    pub fn on_close(&self, handler: impl Fn() + Send + Sync + 'static) {
        self.inner
            .close_handlers
            .lock()
            .unwrap()
            .push(Arc::new(handler));
    }

    /// Wait until the worker is terminated.
    pub async fn wait_for_close(&self) {
        let mut rx = self.inner.close_tx.subscribe();
        let _ = rx.wait_for(|c| *c).await;
    }

    /// Evaluate JavaScript in the worker; a function is called with the
    /// argument and a returned promise is awaited. Values cross the boundary
    /// with Playwright's serializer. Builder; `.await` it.
    pub fn evaluate<T: DeserializeOwned>(
        &self,
        expression: impl Into<String>,
    ) -> WorkerEvaluate<T> {
        WorkerEvaluate {
            worker: self.clone(),
            expression: expression.into(),
            arg: Ok(None),
            api: "worker.evaluate",
            _marker: std::marker::PhantomData,
        }
    }

    /// Like [`Worker::evaluate`], returning a handle to the result.
    pub fn evaluate_handle(&self, expression: impl Into<String>) -> WorkerEvaluateHandle {
        WorkerEvaluateHandle(WorkerEvaluate {
            worker: self.clone(),
            expression: expression.into(),
            arg: Ok(None),
            api: "worker.evaluateHandle",
            _marker: std::marker::PhantomData,
        })
    }

    /// Call `handler` for every `console.*` call of the worker. Requires
    /// console events on the context (see [`crate::NewContext::console_events`]).
    pub fn on_console(&self, handler: impl Fn(ConsoleMessage) + Send + Sync + 'static) {
        self.inner
            .console_handlers
            .lock()
            .unwrap()
            .push(Arc::new(handler));
    }

    pub(crate) fn session_id(&self) -> &SessionId {
        &self.inner.session
    }

    pub(crate) fn emit_console(&self, message: ConsoleMessage) {
        let handlers = self.inner.console_handlers.lock().unwrap().clone();
        for handler in handlers {
            handler(message.clone());
        }
    }

    /// The worker's global scope, reached through its `globalThis`
    /// (`Runtime.enable` is never needed).
    async fn context(&self) -> Result<ExecutionContext> {
        self.inner
            .context
            .get_or_try_init(|| async {
                // The CDP layer registers the worker session right after the
                // attach event; a command sent before that is rejected (and
                // the session dropped), so give it a moment.
                let ready = self.inner.attached_at + std::time::Duration::from_millis(250);
                tokio::time::sleep_until(ready.into()).await;
                if self.is_closed() {
                    return Err(Error::target_closed());
                }
                let browser = self
                    .inner
                    .browser
                    .upgrade()
                    .ok_or_else(Error::target_closed)?;
                let result = browser
                    .execute_with_session(
                        EvaluateParams::new("globalThis"),
                        Some(self.inner.session.clone()),
                    )
                    .await?;
                let object_id = result
                    .result
                    .object_id
                    .ok_or_else(|| Error::evaluation("Failed to resolve the worker global"))?;
                Ok(ExecutionContext::new(
                    Weak::new(),
                    World::Main,
                    ContextSession::Worker {
                        browser: self.inner.browser.clone(),
                        session: self.inner.session.clone(),
                    },
                    ContextTarget::Object(object_id),
                ))
            })
            .await
            .cloned()
    }

    /// A service worker attached by the browser (see `service_worker.rs`).
    pub(crate) fn new_service_worker(
        url: String,
        session: SessionId,
        browser: Weak<crate::browser::BrowserInner>,
    ) -> Worker {
        Worker {
            inner: Arc::new(WorkerInner {
                url,
                session,
                page: Weak::new(),
                browser,
                closed: AtomicBool::new(false),
                attached_at: std::time::Instant::now(),
                close_handlers: Mutex::new(Vec::new()),
                close_tx: tokio::sync::watch::channel(false).0,
                console_handlers: Mutex::new(Vec::new()),
                context: tokio::sync::OnceCell::new(),
            }),
        }
    }

    pub(crate) fn mark_closed(&self) {
        if self.inner.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(ctx) = self.inner.context.get() {
            ctx.mark_destroyed();
        }
        self.inner.close_tx.send_replace(true);
        let handlers = self.inner.close_handlers.lock().unwrap().clone();
        for handler in handlers {
            handler();
        }
    }
}

/// Builder returned by [`Worker::evaluate`] / [`Worker::evaluate_handle`];
/// `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct WorkerEvaluate<T> {
    worker: Worker,
    expression: String,
    arg: Result<Option<(serde_json::Value, Vec<JSHandle>)>>,
    api: &'static str,
    _marker: std::marker::PhantomData<fn() -> T>,
}

impl<T> WorkerEvaluate<T> {
    /// The argument passed to the function (any `Serialize` value, which may
    /// contain handles of this worker).
    pub fn arg<A: Serialize>(mut self, arg: A) -> Self {
        self.arg = serialize_arg(&arg);
        self
    }

    async fn run(self, by_value: bool) -> Result<Evaluated> {
        let api = self.api;
        let worker = self.worker.clone();
        async move {
            let arg = self.arg?;
            let ctx = self.worker.context().await?;
            let (args, handles) = match arg {
                Some((value, handles)) => (vec![value], handles),
                None => (vec![serde_json::json!({ "v": "undefined" })], Vec::new()),
            };
            ctx.evaluate_expression(&self.expression, None, by_value, args, handles)
                .await
        }
        .await
        .map_err(|e| {
            if worker.is_closed() && !e.is_target_closed() {
                Error::target_closed()
            } else {
                e
            }
        })
        .api(api)
    }
}

impl<T: DeserializeOwned + Send + 'static> IntoFuture for WorkerEvaluate<T> {
    type Output = Result<T>;
    type IntoFuture = BoxFuture<Result<T>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let api = self.api;
            match self.run(true).await? {
                Evaluated::Value(v) => {
                    from_js_value(v).map_err(|e| Error::serialization(e).with_api(api))
                }
                Evaluated::Handle(_) => {
                    Err(Error::evaluation("unexpected handle result").with_api(api))
                }
            }
        })
    }
}

/// Builder returned by [`Worker::evaluate_handle`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct WorkerEvaluateHandle(WorkerEvaluate<()>);

impl WorkerEvaluateHandle {
    /// The argument passed to the function.
    pub fn arg<A: Serialize>(self, arg: A) -> Self {
        WorkerEvaluateHandle(self.0.arg(arg))
    }
}

impl IntoFuture for WorkerEvaluateHandle {
    type Output = Result<JSHandle>;
    type IntoFuture = BoxFuture<Result<JSHandle>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let api = self.0.api;
            match self.0.run(false).await? {
                Evaluated::Handle(h) => Ok(h),
                Evaluated::Value(_) => {
                    Err(Error::evaluation("unexpected value result").with_api(api))
                }
            }
        })
    }
}

/// `Target.attachedToTarget` of a page session.
pub(crate) fn on_attached(page: &PageInner, ev: &EventAttachedToTarget) {
    if ev.target_info.r#type != "worker" {
        return;
    }
    let worker = Worker {
        inner: Arc::new(WorkerInner {
            url: ev.target_info.url.clone(),
            session: ev.session_id.clone(),
            page: page.self_weak.clone(),
            browser: page.browser.clone(),
            closed: AtomicBool::new(false),
            attached_at: std::time::Instant::now(),
            close_handlers: Mutex::new(Vec::new()),
            close_tx: tokio::sync::watch::channel(false).0,
            console_handlers: Mutex::new(Vec::new()),
            context: tokio::sync::OnceCell::new(),
        }),
    };
    page.ext
        .workers
        .lock()
        .unwrap()
        .insert(ev.session_id.as_ref().to_string(), worker.clone());
    let console = page
        .context
        .upgrade()
        .is_some_and(|c| c.options.console_events);
    if console {
        // Console events of the worker (opt-in, like the page's).
        let page_weak = page.self_weak.clone();
        let w = worker.clone();
        tokio::spawn(async move {
            let ready = w.inner.attached_at + std::time::Duration::from_millis(250);
            tokio::time::sleep_until(ready.into()).await;
            if let Some(page) = page_weak.upgrade() {
                let _ = send(
                    &page,
                    Some(w.inner.session.clone()),
                    chromiumoxide::cdp::js_protocol::runtime::EnableParams::default(),
                )
                .await;
            }
        });
    }
    page.ext.listeners.emit(Event::Worker(worker));
}

/// `Target.detachedFromTarget` of a page session.
pub(crate) fn on_detached(page: &PageInner, session_id: &str) {
    let worker = page.ext.workers.lock().unwrap().remove(session_id);
    if let Some(worker) = worker {
        worker.mark_closed();
    }
}

/// The page closed: terminate its workers.
pub(crate) fn close_all(page: &PageInner) {
    let workers: Vec<Worker> = page
        .ext
        .workers
        .lock()
        .unwrap()
        .drain()
        .map(|(_, w)| w)
        .collect();
    for worker in workers {
        worker.mark_closed();
    }
}

impl Page {
    /// The dedicated workers of the page.
    pub fn workers(&self) -> Vec<Worker> {
        self.inner.sync();
        self.inner
            .ext
            .workers
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect()
    }
}
