//! # oxidewright
//!
//! A Rust port of the [Playwright](https://playwright.dev) API for Chromium,
//! built on top of [`chromiumoxide`]. Class and method names follow
//! Playwright's (in `snake_case`); options are builder methods chained before
//! `.await`:
//!
//! ```no_run
//! # async fn run() -> oxidewright::Result<()> {
//! use oxidewright::LoadState;
//!
//! let browser = oxidewright::chromium().launch().await?;
//! let page = browser.new_context().await?.new_page().await?;
//! page.goto("https://example.com").wait_until(LoadState::DomContentLoaded).await?;
//! let title: String = page.evaluate("() => document.title").await?;
//! let sum: i32 = page.evaluate("([a, b]) => a + b").arg((1, 2)).await?;
//! # let _ = (title, sum);
//! browser.close().await?;
//! # Ok(()) }
//! ```
//!
//! ## Design
//!
//! * Execution contexts are created on demand with `Page.createIsolatedWorld`
//!   and the main world is reached through its `globalThis` object, so
//!   `Runtime.enable` is never needed (see [`execution_context`]).
//! * Values cross the boundary with Playwright's own serializer
//!   (`UtilityScript`), so `undefined`, `NaN`, `-0`, dates, regular
//!   expressions, `BigInt`s and handles round-trip like in Playwright
//!   (see [`JsValue`]).
//! * Frame lifecycle (`commit`, `domcontentloaded`, `load`, `networkidle`) is
//!   tracked like Playwright's `server/frames.ts`.
//! * The JavaScript bundles (`InjectedScript`, `UtilityScript`, ...) come
//!   from Patchright's build of Playwright, see [`js::sources`].

#![warn(missing_docs)]
// CDP parameter structs are built with `Default` and then filled in.
#![allow(clippy::field_reassign_with_default)]

mod browser;
mod browser_context;
mod browser_type;
mod console;
mod context_ext;
mod dialog;
mod download;
mod error;
mod events;
pub mod execution_context;
mod file_chooser;
mod frame;
mod har;
pub mod js;
mod js_handle;
mod network;
mod network_manager;
mod page;
mod page_ext;
mod progress;
mod route;
mod url_match;
mod websocket;
mod worker;

pub use browser::Browser;
pub use browser_context::{BrowserContext, NewContext, NewContextOptions};
pub use browser_type::{BrowserType, ConnectOverCdp, Launch, LaunchOptions, chromium};
pub use console::{ConsoleMessage, ConsoleMessageLocation, WebError};
pub use dialog::Dialog;
pub use download::Download;
pub use error::{CONTEXT_DESTROYED_MESSAGE, Error, Result, TARGET_CLOSED_MESSAGE};
pub use events::{Event, EventKind, ListenerId, WaitForEvent};
pub use execution_context::{ExecutionContext, World};
pub use file_chooser::FileChooser;
pub use frame::{
    Frame, FrameEvaluate, FrameEvaluateHandle, Goto, LoadState, SetContent, WaitForLoadState,
    WaitForNavigation, WaitForUrl,
};
pub use har::{HarContentPolicy, HarMode, HarNotFound, RecordHar, RouteFromHar};
pub use js::JsValue;
pub use js_handle::{ElementHandle, EvaluateHandleOn, EvaluateOn, JSHandle};
pub use network::{
    Header, RemoteAddr, Request, RequestSizes, ResourceTiming, Response, SecurityDetails,
};
pub use network_manager::HttpCredentials;
pub use page::{AddInitScript, GoHistory, InitScript, Page, Reload, ViewportSize};
pub use progress::DEFAULT_TIMEOUT;
pub use route::{
    APIResponse, Abort, Continue, Fallback, Fetch, Fulfill, Route, RouteBuilder, RouteHandlerFn,
    RouteId, Unroute, UnrouteAll, UnrouteBehavior,
};
pub use url_match::{UrlMatch, glob_to_regex_pattern};
pub use websocket::{WebSocket, WebSocketEvent, WebSocketPayload};
pub use worker::{Worker, WorkerEvaluate};

/// Re-export of the underlying CDP library.
pub use chromiumoxide;
