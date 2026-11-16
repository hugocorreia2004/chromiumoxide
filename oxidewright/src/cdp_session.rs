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
use crate::page::Page;

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
    /// Send a protocol command and return its JSON result.
    ///
    /// ```no_run
    /// # async fn run(context: oxidewright::BrowserContext, page: oxidewright::Page) -> oxidewright::Result<()> {
    /// let session = context.new_cdp_session(&page).await?;
    /// let metrics = session.send("Performance.getMetrics", serde_json::json!({})).await;
    /// # let _ = metrics; Ok(()) }
    /// ```
    pub async fn send(&self, method: &str, params: Value) -> Result<Value> {
        async {
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
        .await
        .api("cdpSession.send")
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

    /// Detach the session: later `send` calls fail.
    pub async fn detach(&self) -> Result<()> {
        if self.detached.swap(true, Ordering::SeqCst) {
            return Err(
                Error::target_closed_with("Session already detached").with_api("cdpSession.detach")
            );
        }
        Ok(())
    }
}

impl BrowserContext {
    /// A raw protocol session for a page, or for an out-of-process frame.
    pub async fn new_cdp_session<'a>(
        &self,
        target: impl Into<CdpSessionTarget<'a>>,
    ) -> Result<CDPSession> {
        async {
            let target = match target.into() {
                CdpSessionTarget::Page(page) => {
                    if page.is_closed() {
                        return Err(Error::target_closed());
                    }
                    SessionTarget::Page(page.inner.cdp.clone())
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
                        SessionTarget::Frame(cdp)
                    } else {
                        SessionTarget::Page(page.inner.cdp.clone())
                    }
                }
            };
            Ok(CDPSession {
                target,
                detached: Arc::new(AtomicBool::new(false)),
            })
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
        Ok(CDPSession {
            target: SessionTarget::Browser(self.clone()),
            detached: Arc::new(AtomicBool::new(false)),
        })
    }
}
