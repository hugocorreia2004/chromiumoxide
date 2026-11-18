//! `browser.startTracing()` / `browser.stopTracing()`: Chromium performance
//! traces (open them in the DevTools Performance panel or chrome://tracing),
//! like Playwright's `CRBrowser.startTracing`.

use std::future::{Future, IntoFuture};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine;
use chromiumoxide::cdp::browser_protocol::target::SessionId;
use chromiumoxide::cdp::browser_protocol::tracing::EventTracingComplete;
use futures::StreamExt;
use serde_json::{Value, json};

use crate::browser::Browser;
use crate::cdp_raw::RawCommand;
use crate::error::{Error, Result};
use crate::page::Page;

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

const DEFAULT_CATEGORIES: &[&str] = &[
    "-*",
    "devtools.timeline",
    "v8.execute",
    "disabled-by-default-devtools.timeline",
    "disabled-by-default-devtools.timeline.frame",
    "toplevel",
    "blink.console",
    "blink.user_timing",
    "latencyInfo",
    "disabled-by-default-devtools.timeline.stack",
    "disabled-by-default-v8.cpu_profiler",
    "disabled-by-default-v8.cpu_profiler.hires",
];

/// The recording in progress, if any.
#[derive(Default)]
pub(crate) struct TracingState {
    recording: Mutex<Option<Recording>>,
}

struct Recording {
    /// The page's session, or `None` for the browser session.
    session: Option<SessionId>,
    page: Option<Page>,
    path: Option<PathBuf>,
}

impl Browser {
    /// Start a Chromium performance trace (Playwright's
    /// `browser.startTracing()`); only one at a time per browser. Stop it
    /// with [`Browser::stop_tracing`].
    pub fn start_tracing(&self) -> StartTracing {
        StartTracing {
            browser: self.clone(),
            page: None,
            path: None,
            screenshots: false,
            categories: None,
        }
    }

    /// Stop the trace started with [`Browser::start_tracing`] and return its
    /// JSON (also written to the `path` given at start, if any).
    pub async fn stop_tracing(&self) -> Result<Vec<u8>> {
        async {
            let recording = self
                .inner
                .ext
                .chromium_tracing
                .recording
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| Error::invalid_argument("Tracing was not started."))?;
            let mut complete = match &recording.page {
                Some(page) => page
                    .inner
                    .cdp
                    .event_listener::<EventTracingComplete>()
                    .await
                    .map_err(Error::from)?,
                None => {
                    let guard = self.inner.cdp_read().await;
                    let cdp = guard.as_ref().ok_or_else(Error::target_closed)?;
                    cdp.event_listener::<EventTracingComplete>()
                        .await
                        .map_err(Error::from)?
                }
            };
            self.inner
                .execute_with_session(
                    RawCommand::new("Tracing.end", json!({})),
                    recording.session.clone(),
                )
                .await?;
            let event = tokio::time::timeout(Duration::from_secs(60), complete.next())
                .await
                .map_err(|_| Error::timeout_exceeded(60_000))?
                .ok_or_else(Error::target_closed)?;
            let handle = event
                .stream
                .clone()
                .ok_or_else(|| Error::evaluation("The trace has no data stream"))?;
            let mut data = Vec::new();
            loop {
                let chunk: Value = self
                    .inner
                    .execute_with_session(
                        RawCommand::new("IO.read", json!({ "handle": handle, "size": 1 << 20 })),
                        recording.session.clone(),
                    )
                    .await?;
                let text = chunk["data"].as_str().unwrap_or_default();
                if chunk["base64Encoded"].as_bool().unwrap_or(false) {
                    data.extend(
                        base64::engine::general_purpose::STANDARD
                            .decode(text)
                            .map_err(|e| Error::evaluation(e.to_string()))?,
                    );
                } else {
                    data.extend_from_slice(text.as_bytes());
                }
                if chunk["eof"].as_bool().unwrap_or(true) {
                    break;
                }
            }
            let _ = self
                .inner
                .execute_with_session(
                    RawCommand::new("IO.close", json!({ "handle": handle })),
                    recording.session.clone(),
                )
                .await;
            if let Some(path) = &recording.path {
                if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(|e| Error::invalid_argument(e.to_string()))?;
                }
                tokio::fs::write(path, &data)
                    .await
                    .map_err(|e| Error::invalid_argument(e.to_string()))?;
            }
            Ok(data)
        }
        .await
        .map_err(|e: Error| e.with_api("browser.stopTracing"))
    }
}

/// Builder returned by [`Browser::start_tracing`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct StartTracing {
    browser: Browser,
    page: Option<Page>,
    path: Option<PathBuf>,
    screenshots: bool,
    categories: Option<Vec<String>>,
}

impl StartTracing {
    /// Trace only this page (default: the whole browser).
    pub fn page(mut self, page: &Page) -> Self {
        self.page = Some(page.clone());
        self
    }

    /// Also write the trace to this file when it stops.
    pub fn path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Capture screenshots in the trace.
    pub fn screenshots(mut self, screenshots: bool) -> Self {
        self.screenshots = screenshots;
        self
    }

    /// Trace categories (default: Playwright's DevTools timeline set).
    pub fn categories<I, S>(mut self, categories: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.categories = Some(categories.into_iter().map(Into::into).collect());
        self
    }
}

impl IntoFuture for StartTracing {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            async {
                let state = &self.browser.inner.ext.chromium_tracing;
                if state.recording.lock().unwrap().is_some() {
                    return Err(Error::invalid_argument(
                        "Cannot start recording trace while already recording trace.",
                    ));
                }
                let mut categories = self
                    .categories
                    .clone()
                    .unwrap_or_else(|| DEFAULT_CATEGORIES.iter().map(|c| c.to_string()).collect());
                if self.screenshots {
                    categories.push("disabled-by-default-devtools.screenshot".to_string());
                }
                let session = self.page.as_ref().map(|p| p.inner.cdp.session_id().clone());
                self.browser
                    .inner
                    .execute_with_session(
                        RawCommand::new(
                            "Tracing.start",
                            json!({
                                "transferMode": "ReturnAsStream",
                                "categories": categories.join(","),
                            }),
                        ),
                        session.clone(),
                    )
                    .await?;
                *state.recording.lock().unwrap() = Some(Recording {
                    session,
                    page: self.page.clone(),
                    path: self.path.clone(),
                });
                Ok(())
            }
            .await
            .map_err(|e: Error| e.with_api("browser.startTracing"))
        })
    }
}
