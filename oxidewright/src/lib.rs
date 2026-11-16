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
mod error;
pub mod execution_context;
mod element_actions;
mod frame;
pub mod input;
pub mod js;
mod js_handle;
mod locator;
mod locator_generators;
mod locator_utils;
mod network;
mod page;
mod progress;
mod selector_parser;
mod selectors;
mod url_match;

pub use browser::Browser;
pub use browser_context::{BrowserContext, NewContext, NewContextOptions};
pub use browser_type::{BrowserType, ConnectOverCdp, Launch, LaunchOptions, chromium};
pub use error::{CONTEXT_DESTROYED_MESSAGE, Error, Result, TARGET_CLOSED_MESSAGE};
pub use execution_context::{ExecutionContext, World};
pub use frame::{
    Frame, FrameEvaluate, FrameEvaluateHandle, Goto, LoadState, SetContent, WaitForLoadState,
    WaitForNavigation, WaitForUrl,
};
pub use js::JsValue;
pub use js_handle::{ElementHandle, EvaluateHandleOn, EvaluateOn, JSHandle};
pub use network::{Request, Response};
pub use page::{AddInitScript, GoHistory, InitScript, Page, Reload, ViewportSize};
pub use progress::DEFAULT_TIMEOUT;
pub use selectors::{Selectors, selectors};
pub use url_match::{UrlMatch, glob_to_regex_pattern};

/// Re-export of the underlying CDP library.
pub use chromiumoxide;
