//! The error type of the crate.
//!
//! Messages mirror the ones produced by Playwright so that code (and humans)
//! used to Playwright recognise them, e.g. `page.goto: Timeout 30000ms exceeded.`
//! followed by a `Call log:` section.

use chromiumoxide::error::CdpError;

/// `Result` alias used throughout the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Message used when the page, context or browser is gone.
pub const TARGET_CLOSED_MESSAGE: &str = "Target page, context or browser has been closed";

/// Message used when an evaluation fails because its execution context went away.
pub const CONTEXT_DESTROYED_MESSAGE: &str =
    "Execution context was destroyed, most likely because of a navigation.";

/// Everything that can go wrong while driving the browser.
///
/// Every variant renders as `"<api>: <message>"` (when the API name is known)
/// followed by an optional Playwright-style call log.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An operation did not complete within its timeout.
    #[error("{}", render(api, message, log))]
    Timeout {
        /// Public API that timed out, e.g. `page.goto`.
        api: Option<String>,
        /// `Timeout 30000ms exceeded.`
        message: String,
        /// Call log lines collected while waiting.
        log: Vec<String>,
    },
    /// The page, context or browser has been closed.
    #[error("{}", render(api, message, log))]
    TargetClosed {
        /// Public API that failed.
        api: Option<String>,
        /// Message, usually [`TARGET_CLOSED_MESSAGE`].
        message: String,
        /// Call log lines.
        log: Vec<String>,
    },
    /// A navigation failed (network error, aborted, interrupted, ...).
    #[error("{}", render(api, message, log))]
    Navigation {
        /// Public API that failed.
        api: Option<String>,
        /// For example `net::ERR_CONNECTION_REFUSED at http://localhost:1/`.
        message: String,
        /// Call log lines.
        log: Vec<String>,
    },
    /// JavaScript threw while evaluating, or the result could not be used.
    #[error("{}", render(api, message, log))]
    Evaluation {
        /// Public API that failed.
        api: Option<String>,
        /// The JavaScript error message (with stack when available).
        message: String,
        /// Call log lines.
        log: Vec<String>,
    },
    /// The frame was detached while (or before) running the operation.
    #[error("{}", render(api, message, log))]
    FrameDetached {
        /// Public API that failed.
        api: Option<String>,
        /// Message, for example `Frame was detached`.
        message: String,
        /// Call log lines.
        log: Vec<String>,
    },
    /// More than one element matched a strict locator.
    #[error("{}", render(api, message, log))]
    StrictModeViolation {
        /// Public API that failed.
        api: Option<String>,
        /// The full strict mode violation message.
        message: String,
        /// Call log lines.
        log: Vec<String>,
    },
    /// An argument or option was invalid.
    #[error("{}", render(api, message, log))]
    InvalidArgument {
        /// Public API that failed.
        api: Option<String>,
        /// What was wrong.
        message: String,
        /// Call log lines.
        log: Vec<String>,
    },
    /// Converting a value from/to JavaScript failed.
    #[error("{}", render(api, message, log))]
    Serialization {
        /// Public API that failed.
        api: Option<String>,
        /// What could not be converted.
        message: String,
        /// Call log lines.
        log: Vec<String>,
    },
    /// Launching or connecting to the browser failed.
    #[error("{}", render(api, message, log))]
    Launch {
        /// Public API that failed.
        api: Option<String>,
        /// What went wrong.
        message: String,
        /// Call log lines.
        log: Vec<String>,
    },
    /// An error reported by the DevTools protocol layer.
    #[error("{}{source}", api.as_ref().map(|a| format!("{a}: ")).unwrap_or_default())]
    Protocol {
        /// Public API that failed.
        api: Option<String>,
        /// The underlying chromiumoxide error.
        #[source]
        source: CdpError,
    },
}

fn render(api: &Option<String>, message: &str, log: &[String]) -> String {
    let mut out = String::new();
    if let Some(api) = api {
        out.push_str(api);
        out.push_str(": ");
    }
    out.push_str(message);
    if !log.is_empty() {
        out.push_str("\nCall log:\n");
        for (i, line) in log.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str("  - ");
            out.push_str(line.trim_start());
        }
    }
    out
}

macro_rules! ctor {
    ($(#[$m:meta])* $fn:ident => $variant:ident) => {
        $(#[$m])*
        pub fn $fn(message: impl Into<String>) -> Self {
            Error::$variant { api: None, message: message.into(), log: Vec::new() }
        }
    };
}

impl Error {
    ctor!(
        /// A [`Error::Timeout`] with the given message.
        timeout => Timeout
    );
    ctor!(
        /// A [`Error::Navigation`] with the given message.
        navigation => Navigation
    );
    ctor!(
        /// A [`Error::Evaluation`] with the given message.
        evaluation => Evaluation
    );
    ctor!(
        /// A [`Error::FrameDetached`] with the given message.
        frame_detached => FrameDetached
    );
    ctor!(
        /// A [`Error::InvalidArgument`] with the given message.
        invalid_argument => InvalidArgument
    );
    ctor!(
        /// A [`Error::Serialization`] with the given message.
        serialization => Serialization
    );
    ctor!(
        /// A [`Error::Launch`] with the given message.
        launch => Launch
    );
    ctor!(
        /// A [`Error::StrictModeViolation`] with the given message.
        strict_mode_violation => StrictModeViolation
    );

    /// `Timeout {ms}ms exceeded.`
    pub fn timeout_exceeded(timeout_ms: u64) -> Self {
        Self::timeout(format!("Timeout {timeout_ms}ms exceeded."))
    }

    /// A [`Error::TargetClosed`] with Playwright's default message.
    pub fn target_closed() -> Self {
        Error::TargetClosed {
            api: None,
            message: TARGET_CLOSED_MESSAGE.to_string(),
            log: Vec::new(),
        }
    }

    /// A [`Error::TargetClosed`] with a custom message.
    pub fn target_closed_with(message: impl Into<String>) -> Self {
        Error::TargetClosed {
            api: None,
            message: message.into(),
            log: Vec::new(),
        }
    }

    /// The error message without the API prefix and call log.
    pub fn message(&self) -> String {
        match self {
            Error::Timeout { message, .. }
            | Error::TargetClosed { message, .. }
            | Error::Navigation { message, .. }
            | Error::Evaluation { message, .. }
            | Error::FrameDetached { message, .. }
            | Error::StrictModeViolation { message, .. }
            | Error::InvalidArgument { message, .. }
            | Error::Serialization { message, .. }
            | Error::Launch { message, .. } => message.clone(),
            Error::Protocol { source, .. } => source.to_string(),
        }
    }

    /// The public API name attached to this error, if any.
    pub fn api(&self) -> Option<&str> {
        match self {
            Error::Timeout { api, .. }
            | Error::TargetClosed { api, .. }
            | Error::Navigation { api, .. }
            | Error::Evaluation { api, .. }
            | Error::FrameDetached { api, .. }
            | Error::StrictModeViolation { api, .. }
            | Error::InvalidArgument { api, .. }
            | Error::Serialization { api, .. }
            | Error::Launch { api, .. }
            | Error::Protocol { api, .. } => api.as_deref(),
        }
    }

    /// The call log attached to this error.
    pub fn call_log(&self) -> &[String] {
        match self {
            Error::Timeout { log, .. }
            | Error::TargetClosed { log, .. }
            | Error::Navigation { log, .. }
            | Error::Evaluation { log, .. }
            | Error::FrameDetached { log, .. }
            | Error::StrictModeViolation { log, .. }
            | Error::InvalidArgument { log, .. }
            | Error::Serialization { log, .. }
            | Error::Launch { log, .. } => log,
            Error::Protocol { .. } => &[],
        }
    }

    /// Whether this is a [`Error::Timeout`].
    pub fn is_timeout(&self) -> bool {
        matches!(self, Error::Timeout { .. })
    }

    /// Whether this is a [`Error::TargetClosed`].
    pub fn is_target_closed(&self) -> bool {
        matches!(self, Error::TargetClosed { .. })
    }

    /// Attach the public API name (only if none is set yet).
    pub(crate) fn with_api(mut self, name: &str) -> Self {
        match &mut self {
            Error::Timeout { api, .. }
            | Error::TargetClosed { api, .. }
            | Error::Navigation { api, .. }
            | Error::Evaluation { api, .. }
            | Error::FrameDetached { api, .. }
            | Error::StrictModeViolation { api, .. }
            | Error::InvalidArgument { api, .. }
            | Error::Serialization { api, .. }
            | Error::Launch { api, .. }
            | Error::Protocol { api, .. } => {
                if api.is_none() {
                    *api = Some(name.to_string());
                }
            }
        }
        self
    }

    /// Attach call log lines (only if none are set yet).
    pub(crate) fn with_log(mut self, lines: Vec<String>) -> Self {
        match &mut self {
            Error::Timeout { log, .. }
            | Error::TargetClosed { log, .. }
            | Error::Navigation { log, .. }
            | Error::Evaluation { log, .. }
            | Error::FrameDetached { log, .. }
            | Error::StrictModeViolation { log, .. }
            | Error::InvalidArgument { log, .. }
            | Error::Serialization { log, .. }
            | Error::Launch { log, .. } => {
                if log.is_empty() {
                    *log = lines;
                }
            }
            Error::Protocol { .. } => {}
        }
        self
    }
}

impl From<CdpError> for Error {
    fn from(err: CdpError) -> Self {
        if is_session_closed(&err) {
            return Error::target_closed();
        }
        Error::Protocol {
            api: None,
            source: err,
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Error::serialization(err.to_string())
    }
}

/// Whether a protocol error means that the session/target is gone.
pub(crate) fn is_session_closed(err: &CdpError) -> bool {
    match err {
        CdpError::ChannelSendError(_) | CdpError::NoResponse | CdpError::TargetClosed => true,
        CdpError::Chrome(e) => {
            let m = e.message.as_str();
            m.contains("Session with given id not found")
                || m.contains("Target closed")
                || m.contains("No target with given id")
                || m.contains("Not attached to an active page")
        }
        other => {
            let m = other.to_string();
            m.contains("Session with given id not found")
                || m.contains("Target closed")
                || m.contains("receiver is gone")
        }
    }
}

/// Helper to attach an API name to a `Result`.
pub(crate) trait ResultExt<T> {
    fn api(self, name: &str) -> Result<T>;
}

impl<T> ResultExt<T> for Result<T> {
    fn api(self, name: &str) -> Result<T> {
        self.map_err(|e| e.with_api(name))
    }
}
