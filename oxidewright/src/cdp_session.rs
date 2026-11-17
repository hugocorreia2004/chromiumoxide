//! [`CDPSession`]: raw DevTools protocol access to a page, an out-of-process
//! frame or the browser (`context.new_cdp_session`,
//! `browser.new_browser_cdp_session`).
//!
//! Unlike Playwright, which attaches a new protocol session, the session
//! shares the target's existing connection: commands and events are those of
//! the page (or frame/browser) session oxidewright already uses. Domains you
//! enable (`Runtime.enable`, `Debugger.enable`, ...) are therefore visible to
//! the page like with any other client, so keep stealth in mind.

use chromiumoxide::cdp::IntoEventKind;
use chromiumoxide::listeners::EventStream;
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::browser::Browser;
use crate::browser_context::BrowserContext;
use crate::cdp_raw::RawCommand;
use crate::error::{Error, Result, ResultExt};
use crate::frame::Frame;
use crate::page::{Page, PageEvent};

#[derive(Clone)]
enum SessionTarget {
    Page(chromiumoxide::Page),
    Frame(chromiumoxide::Frame),
    Browser(Browser),
}

/// A raw protocol session. Cheap to clone.
#[derive(Clone)]
pub struct CDPSession {
    target: SessionTarget,
    detached: Arc<AtomicBool>,
    /// `true` once the session is gone (Playwright's `close` event).
    closed: Arc<tokio::sync::watch::Sender<bool>>,
}

impl std::fmt::Debug for CDPSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match &self.target {
            SessionTarget::Page(_) => "page",
            SessionTarget::Frame(_) => "frame",
            SessionTarget::Browser(_) => "browser",
        };
        f.debug_struct("CDPSession").field("target", &kind).finish()
    }
}

/// What a CDP session can be created for.
pub enum CdpSessionTarget<'a> {
    /// A page.
    Page(&'a Page),
    /// An out-of-process frame (it must have its own session).
    Frame(&'a Frame),
}

impl<'a> From<&'a Page> for CdpSessionTarget<'a> {
    fn from(page: &'a Page) -> Self {
        CdpSessionTarget::Page(page)
    }
}

impl<'a> From<&'a Frame> for CdpSessionTarget<'a> {
    fn from(frame: &'a Frame) -> Self {
        CdpSessionTarget::Frame(frame)
    }
}

impl CDPSession {
    /// Send a protocol command and return its JSON result; `.timeout(ms)`
    /// bounds the wait (Playwright's `send(method, params, { timeout })`).
    ///
    /// ```no_run
    /// # async fn run(context: oxidewright::BrowserContext, page: oxidewright::Page) -> oxidewright::Result<()> {
    /// let session = context.new_cdp_session(&page).await?;
    /// let metrics = session.send("Performance.getMetrics", serde_json::json!({})).await;
    /// let version = session
    ///     .send("Browser.getVersion", serde_json::json!({}))
    ///     .timeout(5_000)
    ///     .await?;
    /// # let _ = (metrics, version); Ok(()) }
    /// ```
    pub fn send(&self, method: impl Into<String>, params: Value) -> CdpSend {
        CdpSend {
            session: self.clone(),
            method: method.into(),
            params,
            timeout: None,
        }
    }

    async fn send_impl(&self, method: &str, params: Value) -> Result<Value> {
        if self.detached.load(Ordering::SeqCst) {
            return Err(Error::target_closed_with(
                "Target page, context or browser has been closed",
            ));
        }
        let command = RawCommand::new(method, params);
        match &self.target {
            SessionTarget::Page(page) => Ok(page.execute(command).await?.result),
            SessionTarget::Frame(frame) => Ok(frame.execute(command).await?.result),
            SessionTarget::Browser(browser) => {
                browser.inner.execute_with_session(command, None).await
            }
        }
    }

    /// Whether the session is gone (detached, or its page/frame closed).
    pub fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    /// Resolve when the session closes (Playwright's `close` event).
    pub async fn wait_for_close(&self) {
        let mut rx = self.closed.subscribe();
        let _ = rx.wait_for(|closed| *closed).await;
    }

    /// Call `handler` once when the session closes (Playwright's
    /// `session.on('close')`).
    pub fn on_close(&self, handler: impl FnOnce() + Send + 'static) {
        let session = self.clone();
        tokio::spawn(async move {
            session.wait_for_close().await;
            handler();
        });
    }

    fn mark_closed(&self) {
        self.closed.send_replace(true);
    }

    fn new(target: SessionTarget) -> Self {
        CDPSession {
            target,
            detached: Arc::new(AtomicBool::new(false)),
            closed: Arc::new(tokio::sync::watch::channel(false).0),
        }
    }

    /// Mark the session closed when `page` closes or, for a frame session,
    /// when `frame_id` detaches.
    fn watch(&self, page: &Page, frame_id: Option<String>) {
        let weak = Arc::downgrade(&self.closed);
        let mut events = page.inner.subscribe();
        tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                let gone = match &event {
                    PageEvent::Closed => true,
                    PageEvent::FrameDetached(id) => frame_id.as_deref() == Some(id.as_str()),
                    _ => false,
                };
                let Some(closed) = weak.upgrade() else {
                    return;
                };
                if *closed.borrow() {
                    return;
                }
                if gone {
                    closed.send_replace(true);
                    return;
                }
            }
            if let Some(closed) = weak.upgrade() {
                closed.send_replace(true);
            }
        });
    }

    /// Stream of a protocol event (generated types of
    /// [`chromiumoxide::cdp`], or a custom [`chromiumoxide::cdp::CustomEvent`]).
    /// Not available for frame sessions.
    pub async fn event_listener<T: IntoEventKind>(&self) -> Result<EventStream<T>> {
        match &self.target {
            SessionTarget::Page(page) => Ok(page.event_listener::<T>().await?),
            SessionTarget::Browser(browser) => {
                let guard = browser.inner.cdp_read().await;
                let cdp = guard.as_ref().ok_or_else(Error::target_closed)?;
                Ok(cdp.event_listener::<T>().await?)
            }
            SessionTarget::Frame(_) => Err(Error::invalid_argument(
                "Events of out-of-process frame sessions are not supported; listen on the page session",
            )),
        }
    }

    /// Detach the session: later `send` calls fail and the `close` event
    /// fires.
    pub async fn detach(&self) -> Result<()> {
        if self.detached.swap(true, Ordering::SeqCst) {
            return Err(
                Error::target_closed_with("Session already detached").with_api("cdpSession.detach")
            );
        }
        self.mark_closed();
        Ok(())
    }
}

/// Builder returned by [`CDPSession::send`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct CdpSend {
    session: CDPSession,
    method: String,
    params: Value,
    timeout: Option<u64>,
}

impl CdpSend {
    /// Maximum time in milliseconds to wait for the response (no limit by
    /// default; `0` also means no limit).
    pub fn timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout = Some(timeout_ms);
        self
    }
}

impl std::future::IntoFuture for CdpSend {
    type Output = Result<Value>;
    type IntoFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value>> + Send>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let run = self.session.send_impl(&self.method, self.params);
            match self.timeout {
                Some(ms) if ms > 0 => {
                    match tokio::time::timeout(std::time::Duration::from_millis(ms), run).await {
                        Ok(result) => result,
                        Err(_) => Err(Error::timeout_exceeded(ms)),
                    }
                }
                _ => run.await,
            }
            .api("cdpSession.send")
        })
    }
}

impl BrowserContext {
    /// A raw protocol session for a page, or for an out-of-process frame.
    pub async fn new_cdp_session<'a>(
        &self,
        target: impl Into<CdpSessionTarget<'a>>,
    ) -> Result<CDPSession> {
        async {
            let (target, page, frame_id) = match target.into() {
                CdpSessionTarget::Page(page) => {
                    if page.is_closed() {
                        return Err(Error::target_closed());
                    }
                    (SessionTarget::Page(page.inner.cdp.clone()), page.clone(), None)
                }
                CdpSessionTarget::Frame(frame) => {
                    let page = frame.page().ok_or_else(Error::target_closed)?;
                    let cdp = frame.cdp_frame().await?;
                    let is_oopif = cdp.session_id() != page.inner.cdp.session_id();
                    if !is_oopif && frame.parent_frame().is_some() {
                        return Err(Error::invalid_argument(
                            "This frame does not have a separate CDP session, it is a part of the parent frame's session",
                        ));
                    }
                    if is_oopif {
                        (
                            SessionTarget::Frame(cdp),
                            page,
                            Some(frame.id().to_string()),
                        )
                    } else {
                        (SessionTarget::Page(page.inner.cdp.clone()), page, None)
                    }
                }
            };
            let session = CDPSession::new(target);
            session.watch(&page, frame_id);
            Ok(session)
        }
        .await
        .api("browserContext.newCDPSession")
    }
}

impl Browser {
    /// A raw protocol session for the browser target.
    pub async fn new_browser_cdp_session(&self) -> Result<CDPSession> {
        if !self.is_connected() {
            return Err(Error::target_closed().with_api("browser.newBrowserCDPSession"));
        }
        Ok(CDPSession::new(SessionTarget::Browser(self.clone())))
    }
}
