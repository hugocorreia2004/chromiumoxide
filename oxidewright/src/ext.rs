//! Per-context and per-page state of the emulation, storage, capture, clock
//! and related features. Kept in one place so `ContextInner`/`PageInner` only
//! need a single extra field each.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use crate::emulation::{EmulatedMedia, EmulatedSize, Geolocation};

/// Mutable state of a browser context.
#[derive(Default)]
pub(crate) struct ContextExt {
    /// Current geolocation override (`setGeolocation`).
    pub(crate) geolocation: Mutex<Option<Geolocation>>,
    /// Granted permissions per origin (`*` = every origin).
    pub(crate) permissions: Mutex<HashMap<String, Vec<String>>>,
    /// Origins whose documents were committed in this context
    /// (`BrowserContext._origins`).
    pub(crate) origins: Mutex<HashSet<String>>,
    /// Reason given to `close({ reason })`.
    pub(crate) close_reason: Mutex<Option<String>>,
    /// Fake timers state (`context.clock`).
    pub(crate) clock: tokio::sync::Mutex<crate::clock::ClockState>,
    /// Tracing state (`context.tracing()`).
    pub(crate) capture: crate::trace::ContextTracing,
    /// `context.request()`.
    pub(crate) request: std::sync::OnceLock<crate::fetch::APIRequestContext>,
}

impl ContextExt {
    /// `addVisitedOrigin`, called when a frame commits a new document.
    pub(crate) fn add_visited_url(&self, url: &str) {
        if let Some(origin) = http_origin(url) {
            self.origins.lock().unwrap().insert(origin);
        }
    }
}

/// `Frame.origin()`: the origin of `http(s)` URLs only.
pub(crate) fn http_origin(url: &str) -> Option<String> {
    if !url.starts_with("http") {
        return None;
    }
    let parsed = url::Url::parse(url).ok()?;
    match parsed.origin() {
        origin @ url::Origin::Tuple(..) => Some(origin.ascii_serialization()),
        url::Origin::Opaque(_) => None,
    }
}

/// Mutable state of a page.
#[derive(Default)]
pub(crate) struct PageExt {
    /// `page.emulateMedia` overrides.
    pub(crate) emulated_media: Mutex<EmulatedMedia>,
    /// `page.setViewportSize` override of the context viewport.
    pub(crate) emulated_size: Mutex<Option<EmulatedSize>>,
    /// Last `Emulation.setDeviceMetricsOverride` sent (its device scale factor
    /// is needed by screenshots).
    pub(crate) metrics_override: Mutex<Option<crate::emulation::MetricsOverride>>,
    /// Screenshots of a page are taken one at a time (`Screenshotter._queue`).
    #[allow(dead_code)] // used by screenshots
    pub(crate) screenshot_queue: tokio::sync::Mutex<()>,
    /// Reason given to `close({ reason })`.
    pub(crate) close_reason: Mutex<Option<String>>,
    /// JS/CSS coverage state.
    pub(crate) coverage: tokio::sync::Mutex<crate::coverage::CoverageState>,
    /// Screencast, video and tracing state.
    pub(crate) capture: crate::screencast::PageCapture,
}
