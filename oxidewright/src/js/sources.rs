//! The JavaScript bundles injected into pages.
//!
//! They are extracted from the `patchright-core` npm package (see
//! `generated/VERSION`) by `scripts/update-injected.mjs` and are licensed
//! under the Apache License 2.0 (Playwright and Patchright authors, see the
//! crate's `NOTICE` file). Each one is a CommonJS-style module body: wrap it
//! in a function that declares `const module = {}` and read
//! `module.exports.<Name>()` to get the exported class.

/// Version of the package the scripts were extracted from.
pub const VERSION: &str = include_str!("generated/VERSION");

/// `InjectedScript`: selector engines, actionability checks, aria snapshots...
pub const INJECTED_SCRIPT: &str = include_str!("generated/injectedScriptSource.js");
/// `UtilityScript`: evaluation with Playwright's value serializer.
pub const UTILITY_SCRIPT: &str = include_str!("generated/utilityScriptSource.js");
/// Fake timers (`page.clock`).
pub const CLOCK: &str = include_str!("generated/clockSource.js");
/// JS/CSS coverage helpers.
pub const COVERAGE_SCRIPT: &str = include_str!("generated/coverageScriptSource.js");
/// Storage state collection (`context.storageState`).
pub const STORAGE_SCRIPT: &str = include_str!("generated/storageScriptSource.js");
/// WebSocket routing (`page.routeWebSocket`).
pub const WEB_SOCKET_MOCK: &str = include_str!("generated/webSocketMockSource.js");
/// Recorder (codegen) overlay.
pub const POLLING_RECORDER: &str = include_str!("generated/pollingRecorderSource.js");
/// Virtual WebAuthn authenticator support.
pub const WEB_AUTHN: &str = include_str!("generated/webAuthnSource.js");
/// WebMCP support.
pub const WEB_MCP: &str = include_str!("generated/webMCPSource.js");
/// `insertText` helper used by the BiDi backend.
pub const BIDI_INSERT_TEXT: &str = include_str!("generated/bidiInsertTextSource.js");
/// Dialog helper used by WebView backends.
pub const WEB_VIEW_DIALOG: &str = include_str!("generated/webViewDialogSource.js");
/// Input helper used by WebView backends.
pub const WEB_VIEW_INPUT: &str = include_str!("generated/webViewInputSource.js");
