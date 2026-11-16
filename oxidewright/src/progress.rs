//! Timeouts and call logs, mirroring Playwright's `TimeoutSettings` and
//! `Progress` (a timed operation that collects log lines which are appended
//! to the error message when the operation fails).

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::error::{Error, Result};

/// Default timeout of actions and navigations, in milliseconds.
pub const DEFAULT_TIMEOUT: u64 = 30_000;

/// Default timeouts of a context or page, inheriting from a parent.
#[derive(Debug, Default)]
pub(crate) struct TimeoutSettings {
    parent: Option<Arc<TimeoutSettings>>,
    default_timeout: Mutex<Option<u64>>,
    default_navigation_timeout: Mutex<Option<u64>>,
}

impl TimeoutSettings {
    pub(crate) fn new(parent: Option<Arc<TimeoutSettings>>) -> Self {
        Self {
            parent,
            ..Default::default()
        }
    }

    pub(crate) fn set_default_timeout(&self, timeout: u64) {
        *self.default_timeout.lock().unwrap() = Some(timeout);
    }

    pub(crate) fn set_default_navigation_timeout(&self, timeout: u64) {
        *self.default_navigation_timeout.lock().unwrap() = Some(timeout);
    }

    /// Timeout for actions: explicit > own default > parent > 30s.
    #[allow(dead_code)] // used by actions (locators, input)
    pub(crate) fn timeout(&self, explicit: Option<u64>) -> u64 {
        if let Some(t) = explicit {
            return t;
        }
        if let Some(t) = *self.default_timeout.lock().unwrap() {
            return t;
        }
        match &self.parent {
            Some(parent) => parent.timeout(None),
            None => DEFAULT_TIMEOUT,
        }
    }

    /// Timeout for navigations: explicit > own navigation default > own
    /// default > parent > 30s.
    pub(crate) fn navigation_timeout(&self, explicit: Option<u64>) -> u64 {
        if let Some(t) = explicit {
            return t;
        }
        if let Some(t) = *self.default_navigation_timeout.lock().unwrap() {
            return t;
        }
        if let Some(t) = *self.default_timeout.lock().unwrap() {
            return t;
        }
        match &self.parent {
            Some(parent) => parent.navigation_timeout(None),
            None => DEFAULT_TIMEOUT,
        }
    }
}

/// A timed API call collecting a call log.
#[derive(Debug, Clone)]
pub(crate) struct Progress {
    api: &'static str,
    timeout: u64,
    log: Arc<Mutex<Vec<String>>>,
}

impl Progress {
    pub(crate) fn new(api: &'static str, timeout: u64) -> Self {
        Self {
            api,
            timeout,
            log: Arc::default(),
        }
    }

    /// Add a line to the call log.
    pub(crate) fn log(&self, line: impl Into<String>) {
        let line = line.into();
        tracing::debug!(api = self.api, "{line}");
        self.log.lock().unwrap().push(line);
    }

    /// Run `fut` under this progress' timeout; errors get the API name and
    /// the call log attached.
    pub(crate) async fn run<T>(&self, fut: impl Future<Output = Result<T>>) -> Result<T> {
        let result = if self.timeout == 0 {
            fut.await
        } else {
            match tokio::time::timeout(Duration::from_millis(self.timeout), fut).await {
                Ok(result) => result,
                Err(_) => Err(Error::timeout_exceeded(self.timeout)),
            }
        };
        result.map_err(|e| {
            let log = self.log.lock().unwrap().clone();
            e.with_api(self.api).with_log(log)
        })
    }
}
