//! Dedicated workers of a page (`worker` event).
//!
//! Workers are reported from the auto-attached `worker` targets. Evaluation
//! uses `Runtime.evaluate` in the worker's session (no `Runtime.enable`), so
//! values cross the boundary as JSON: `undefined` becomes `null`, and handles
//! are not supported.

use std::fmt;
use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use chromiumoxide::cdp::browser_protocol::target::{EventAttachedToTarget, SessionId};
use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{Error, Result, ResultExt};
use crate::events::Event;
use crate::network_manager::send;
use crate::page::{Page, PageInner};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// A dedicated web worker. Cheap to clone.
#[derive(Clone)]
pub struct Worker {
    inner: Arc<WorkerInner>,
}

struct WorkerInner {
    url: String,
    session: SessionId,
    page: Weak<PageInner>,
    closed: AtomicBool,
    attached_at: std::time::Instant,
    close_handlers: Mutex<Vec<Arc<dyn Fn() + Send + Sync>>>,
    close_tx: tokio::sync::watch::Sender<bool>,
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
    /// argument and awaited. Builder; `.await` it.
    pub fn evaluate<T: DeserializeOwned>(
        &self,
        expression: impl Into<String>,
    ) -> WorkerEvaluate<T> {
        WorkerEvaluate {
            worker: self.clone(),
            expression: expression.into(),
            arg: Ok(serde_json::Value::Null),
            _marker: std::marker::PhantomData,
        }
    }

    fn mark_closed(&self) {
        if self.inner.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.inner.close_tx.send_replace(true);
        let handlers = self.inner.close_handlers.lock().unwrap().clone();
        for handler in handlers {
            handler();
        }
    }
}

/// Builder returned by [`Worker::evaluate`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct WorkerEvaluate<T> {
    worker: Worker,
    expression: String,
    arg: Result<serde_json::Value>,
    _marker: std::marker::PhantomData<fn() -> T>,
}

impl<T> WorkerEvaluate<T> {
    /// The argument passed to the function (JSON-serialized).
    pub fn arg<A: Serialize>(mut self, arg: A) -> Self {
        self.arg = serde_json::to_value(arg).map_err(Error::from);
        self
    }
}

impl<T: DeserializeOwned + Send + 'static> IntoFuture for WorkerEvaluate<T> {
    type Output = Result<T>;
    type IntoFuture = BoxFuture<Result<T>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            async {
                let arg = self.arg?;
                let page = self
                    .worker
                    .inner
                    .page
                    .upgrade()
                    .ok_or_else(Error::target_closed)?;
                // The CDP layer registers the worker session right after the
                // attach event; a command sent before that is rejected (and
                // the session dropped), so give it a moment.
                let ready = self.worker.inner.attached_at + std::time::Duration::from_millis(250);
                tokio::time::sleep_until(ready.into()).await;
                if self.worker.is_closed() {
                    return Err(Error::target_closed());
                }
                let expression = format!(
                    "(async () => {{ const __f = ({}\n); const __a = {}; return typeof __f === 'function' ? await __f(__a) : await __f; }})()",
                    self.expression, arg
                );
                let mut params = EvaluateParams::new(expression);
                params.return_by_value = Some(true);
                params.await_promise = Some(true);
                let result = send(&page, Some(self.worker.inner.session.clone()), params).await?;
                if let Some(details) = result.exception_details {
                    return Err(Error::evaluation(
                        crate::execution_context::exception_message(&details),
                    ));
                }
                let value = result.result.value.unwrap_or(serde_json::Value::Null);
                serde_json::from_value(value).map_err(Error::from)
            }
            .await
            .api("worker.evaluate")
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
            closed: AtomicBool::new(false),
            attached_at: std::time::Instant::now(),
            close_handlers: Mutex::new(Vec::new()),
            close_tx: tokio::sync::watch::channel(false).0,
        }),
    };
    page.ext
        .workers
        .lock()
        .unwrap()
        .insert(ev.session_id.as_ref().to_string(), worker.clone());
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
