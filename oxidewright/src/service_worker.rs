//! Service workers of a context (`context.service_workers()`, the
//! `serviceworker` event), like Playwright's `crServiceWorker.ts` as patched
//! by Patchright 1.64.3: the worker target is attached with a flat session
//! and reached with `Runtime.evaluate` only (no `Runtime.enable`).
//!
//! Service worker targets are discovered from the browser's
//! `Target.targetCreated` events (chromiumoxide's page sessions detach from
//! them), then attached with `Target.attachToTarget`. Network events of the
//! service worker session are not tracked, so `request.service_worker()` is
//! not available.

use std::sync::{Arc, Mutex, Weak};

use chromiumoxide::cdp::browser_protocol::target::{
    AttachToTargetParams, EventTargetCreated, EventTargetDestroyed, TargetInfo,
};
use chromiumoxide::cdp::js_protocol::runtime::RunIfWaitingForDebuggerParams;
use futures::StreamExt;

use crate::browser::BrowserInner;
use crate::browser_context::BrowserContext;
use crate::worker::Worker;

type Handler = Arc<dyn Fn(Worker) + Send + Sync>;

/// Context-level service worker state.
#[derive(Default)]
pub(crate) struct ServiceWorkers {
    /// Target id → worker.
    workers: Mutex<Vec<(String, Worker)>>,
    handlers: Mutex<Vec<Handler>>,
}

fn context_for(browser: &BrowserInner, info: &TargetInfo) -> Option<BrowserContext> {
    let contexts = browser.all_contexts();
    match &info.browser_context_id {
        Some(id) => contexts
            .iter()
            .find(|c| c.inner.id.as_ref() == Some(id))
            .or_else(|| contexts.iter().find(|c| c.inner.id.is_none()))
            .cloned(),
        None => contexts.into_iter().find(|c| c.inner.id.is_none()),
    }
}

/// Watch the browser's targets for service workers.
pub(crate) async fn start_listener(inner: &Arc<BrowserInner>, cdp: &chromiumoxide::Browser) {
    let Ok(mut created) = cdp.event_listener::<EventTargetCreated>().await else {
        return;
    };
    let Ok(mut destroyed) = cdp.event_listener::<EventTargetDestroyed>().await else {
        return;
    };
    let weak = Arc::downgrade(inner);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                ev = created.next() => {
                    let Some(ev) = ev else { break };
                    if ev.target_info.r#type != "service_worker" {
                        continue;
                    }
                    let Some(browser) = weak.upgrade() else { break };
                    let info = ev.target_info.clone();
                    tokio::spawn(async move { attach(browser, info).await });
                }
                ev = destroyed.next() => {
                    let Some(ev) = ev else { break };
                    let Some(browser) = weak.upgrade() else { break };
                    on_destroyed(&browser, ev.target_id.as_ref());
                }
            }
        }
    });
}

async fn attach(browser: Arc<BrowserInner>, info: TargetInfo) {
    let Some(context) = context_for(&browser, &info) else {
        return;
    };
    let params = AttachToTargetParams::builder()
        .target_id(info.target_id.clone())
        .flatten(true)
        .build();
    let Ok(params) = params else {
        return;
    };
    let session = match browser.execute_with_session(params, None).await {
        Ok(result) => result.session_id,
        Err(err) => {
            tracing::debug!("failed to attach to a service worker: {err}");
            return;
        }
    };
    let _ = browser
        .execute_with_session(
            RunIfWaitingForDebuggerParams::default(),
            Some(session.clone()),
        )
        .await;
    let worker = Worker::new_service_worker(info.url.clone(), session, Arc::downgrade(&browser));
    let state = &context.inner.ext.service_workers;
    state
        .workers
        .lock()
        .unwrap()
        .push((info.target_id.as_ref().to_string(), worker.clone()));
    let handlers = state.handlers.lock().unwrap().clone();
    for handler in handlers {
        handler(worker.clone());
    }
}

fn on_destroyed(browser: &BrowserInner, target_id: &str) {
    for context in browser.all_contexts() {
        let removed: Vec<Worker> = {
            let mut workers = context.inner.ext.service_workers.workers.lock().unwrap();
            let (gone, kept): (Vec<_>, Vec<_>) =
                workers.drain(..).partition(|(id, _)| id == target_id);
            *workers = kept;
            gone.into_iter().map(|(_, w)| w).collect()
        };
        for worker in removed {
            worker.mark_closed();
        }
    }
}

/// Close the service workers of a closing context.
#[allow(dead_code)]
pub(crate) fn close_all(context: &BrowserContext) {
    let workers: Vec<Worker> = context
        .inner
        .ext
        .service_workers
        .workers
        .lock()
        .unwrap()
        .drain(..)
        .map(|(_, w)| w)
        .collect();
    for worker in workers {
        worker.mark_closed();
    }
}

impl BrowserContext {
    /// The service workers of the context (Playwright's
    /// `browserContext.serviceWorkers()`, Chromium only).
    pub fn service_workers(&self) -> Vec<Worker> {
        self.inner
            .ext
            .service_workers
            .workers
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, w)| !w.is_closed())
            .map(|(_, w)| w.clone())
            .collect()
    }

    /// Call `handler` for every new service worker (Playwright's
    /// `serviceworker` event).
    pub fn on_service_worker(&self, handler: impl Fn(Worker) + Send + Sync + 'static) {
        self.inner
            .ext
            .service_workers
            .handlers
            .lock()
            .unwrap()
            .push(Arc::new(handler));
    }

    /// Wait for the next service worker
    /// (`context.waitForEvent('serviceworker')`); `.timeout(ms)` before
    /// `.await` (default: the context's default timeout).
    pub fn wait_for_service_worker(&self) -> WaitForServiceWorker {
        let (tx, rx) = tokio::sync::oneshot::channel::<Worker>();
        let tx = Arc::new(Mutex::new(Some(tx)));
        let weak: Weak<_> = Arc::downgrade(&tx);
        self.on_service_worker(move |worker| {
            if let Some(tx) = weak.upgrade() {
                if let Some(sender) = tx.lock().unwrap().take() {
                    let _ = sender.send(worker);
                }
            }
        });
        WaitForServiceWorker {
            context: self.clone(),
            rx,
            _tx: tx,
            timeout: None,
        }
    }
}

/// Builder returned by [`BrowserContext::wait_for_service_worker`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct WaitForServiceWorker {
    context: BrowserContext,
    rx: tokio::sync::oneshot::Receiver<Worker>,
    _tx: Arc<Mutex<Option<tokio::sync::oneshot::Sender<Worker>>>>,
    timeout: Option<u64>,
}

impl WaitForServiceWorker {
    /// Maximum time in milliseconds (`0` = no limit).
    pub fn timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout = Some(timeout_ms);
        self
    }
}

impl std::future::IntoFuture for WaitForServiceWorker {
    type Output = crate::Result<Worker>;
    type IntoFuture =
        std::pin::Pin<Box<dyn std::future::Future<Output = crate::Result<Worker>> + Send>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let timeout = self
                .timeout
                .unwrap_or_else(|| self.context.inner.timeouts.timeout(None));
            let keep = self._tx;
            let wait = async { self.rx.await.map_err(|_| crate::Error::target_closed()) };
            let result = if timeout == 0 {
                wait.await
            } else {
                match tokio::time::timeout(std::time::Duration::from_millis(timeout), wait).await {
                    Ok(r) => r,
                    Err(_) => Err(crate::Error::timeout_exceeded(timeout)),
                }
            };
            drop(keep);
            result.map_err(|e| e.with_api("browserContext.waitForEvent"))
        })
    }
}
