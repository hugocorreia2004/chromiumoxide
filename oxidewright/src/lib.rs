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

pub mod actions;
mod browser;
mod browser_context;
mod browser_type;
mod cdp_raw;
mod cdp_session;
mod clock;
mod console;
mod context_ext;
mod context_options;
mod cookies;
pub mod coverage;
mod dialog;
mod download;
mod element_actions;
pub mod emulation;
mod error;
mod events;
pub mod execution_context;
mod ext;
pub mod fetch;
mod file_chooser;
mod frame;
mod frame_selectors;
mod har;
pub mod input;
pub mod js;
mod js_handle;
mod locator;
mod locator_generators;
mod locator_utils;
mod network;
mod network_manager;
mod page;
mod page_ext;
mod page_extras;
mod pdf;
mod progress;
mod route;
mod screenshot;
mod selector_parser;
mod selectors;
mod storage_state;
mod url_match;
mod websocket;
mod worker;

pub use actions::{
    Action, BoundingBox, Click, Dblclick, ElementState, Fill, Hover, InputFiles, Position, Press,
    SelectOption, SelectOptionAction, SelectValue, SelectValues, SetChecked, SetInputFiles, Tap,
    Type, WaitForSelector, kinds,
};
pub use browser::Browser;
pub use browser_context::{BrowserContext, NewContext, NewContextOptions};
pub use browser_type::{BrowserType, ConnectOverCdp, Launch, LaunchOptions, chromium};
pub use cdp_session::{CDPSession, CdpSessionTarget};
pub use clock::{Clock, ClockInstall, ClockTicks, ClockTime};
pub use console::{ConsoleMessage, ConsoleMessageLocation, WebError};
pub use context_options::{Device, devices};
pub use cookies::{ClearCookies, Cookie, Cookies, SameSite, SetCookie};
pub use coverage::{
    CoverageRange, CssCoverageEntry, FunctionCoverage, JsCoverageEntry, StartCssCoverage,
    StartJsCoverage, TextRange,
};
pub use dialog::Dialog;
pub use download::Download;
pub use element_actions::WaitForSelectorState;
pub use emulation::{
    ColorScheme, Contrast, EmulateMedia, ForcedColors, Geolocation, GrantPermissions, Media,
    ReducedMotion, ServiceWorkerPolicy,
};
pub use error::{CONTEXT_DESTROYED_MESSAGE, Error, Result, TARGET_CLOSED_MESSAGE};
pub use events::{Event, EventKind, ListenerId, WaitForEvent};
pub use execution_context::{ExecutionContext, World};
pub use fetch::{
    APIRequest, APIRequestContext, FilePayload, HttpCredentialsSend, MultipartValue, ProxySettings,
    RequestData, request,
};
pub use file_chooser::FileChooser;
pub use frame::{
    Frame, FrameEvaluate, FrameEvaluateHandle, Goto, LoadState, SetContent, WaitForLoadState,
    WaitForNavigation, WaitForUrl,
};
pub use har::{HarContentPolicy, HarMode, HarNotFound, RecordHar, RouteFromHar};
pub use input::{
    Keyboard, KeyboardModifier, KeyboardPress, KeyboardType, Mouse, MouseButton, MouseButtonAction,
    MouseClick, MouseMove, Touchscreen,
};
pub use js::JsValue;
pub use js_handle::{ElementHandle, EvaluateHandleOn, EvaluateOn, JSHandle};
pub use locator::{
    FrameLocator, GetByOptions, GetByRoleOptions, Locator, LocatorOptions, SelectorOrLocator,
};
pub use locator_utils::{JsRegex, TextMatch, regex};
pub use network::{
    Header, RemoteAddr, Request, RequestSizes, ResourceTiming, Response, SecurityDetails,
};
pub use network_manager::HttpCredentials;
pub use page::{AddInitScript, GoHistory, InitScript, Page, Reload, ViewportSize};
pub use page_extras::{AddTag, CloseContext, ClosePage};
pub use pdf::{Pdf, PdfLength, PdfMargin};
pub use progress::DEFAULT_TIMEOUT;
pub use route::{
    APIResponse, Abort, Continue, Fallback, Fetch, Fulfill, Route, RouteBuilder, RouteHandlerFn,
    RouteId, Unroute, UnrouteAll, UnrouteBehavior,
};
pub use screenshot::{
    Animations, Caret, ElementScreenshot, Mask, PageScreenshot, Rect, ScreenshotScale,
    ScreenshotType,
};
pub use selectors::{Selectors, selectors};
pub use storage_state::{
    NameValue, OriginState, StorageState, StorageStateBuilder, StorageStateSource,
};
pub use url_match::{UrlMatch, glob_to_regex_pattern};
pub use websocket::{WebSocket, WebSocketEvent, WebSocketPayload};
pub use worker::{Worker, WorkerEvaluate};

/// Re-export of the underlying CDP library.
pub use chromiumoxide;
