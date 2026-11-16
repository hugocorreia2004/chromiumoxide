//! Screenshots of pages and elements, mirroring Playwright's
//! `server/screenshotter.ts` and `CRPage.takeScreenshot`.
//!
//! The page is never brought to front: `Page.captureScreenshot` works on
//! background tabs too (call [`Page::bring_to_front`] yourself if needed).

use std::future::{Future, IntoFuture};
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cdp_raw::RawCommand;
use crate::error::{Error, Result};
use crate::execution_context::{Evaluated, World};
use crate::frame::Frame;
use crate::frame_selectors::{CallArg, CallOptions, SelectorTarget, call_on_selector};
use crate::js::serde_impl::from_js_value;
use crate::js_handle::{ElementHandle, serialize_arg};
use crate::locator::Locator;
use crate::page::{Page, PageInner, ViewportSize};
use crate::progress::Progress;

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Image format of a screenshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenshotType {
    /// PNG (default).
    Png,
    /// JPEG (supports `quality`, no transparency).
    Jpeg,
    /// WebP (supports `quality`).
    Webp,
}

impl ScreenshotType {
    fn as_str(&self) -> &'static str {
        match self {
            ScreenshotType::Png => "png",
            ScreenshotType::Jpeg => "jpeg",
            ScreenshotType::Webp => "webp",
        }
    }

    /// `determineScreenshotType`: the format implied by a file extension.
    fn from_path(path: &std::path::Path) -> Result<ScreenshotType> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        match ext.as_str() {
            "png" => Ok(ScreenshotType::Png),
            "jpg" | "jpeg" => Ok(ScreenshotType::Jpeg),
            "webp" => Ok(ScreenshotType::Webp),
            _ => {
                let mime = match ext.as_str() {
                    "" => "null".to_string(),
                    "gif" => "image/gif".to_string(),
                    "svg" => "image/svg+xml".to_string(),
                    "txt" => "text/plain".to_string(),
                    "html" | "htm" => "text/html".to_string(),
                    "pdf" => "application/pdf".to_string(),
                    other => format!("application/{other}"),
                };
                Err(Error::invalid_argument(format!(
                    "path: unsupported mime type \"{mime}\""
                )))
            }
        }
    }
}

/// A rectangle in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    /// Left.
    pub x: f64,
    /// Top.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

impl Rect {
    /// A rectangle.
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// `animations` option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Animations {
    /// Leave animations running (default).
    #[default]
    Allow,
    /// Finish finite animations, cancel infinite ones (restored afterwards).
    Disabled,
}

/// `caret` option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Caret {
    /// Hide the text caret (default).
    #[default]
    Hide,
    /// Keep the caret as is.
    Initial,
}

/// `scale` option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScreenshotScale {
    /// One image pixel per CSS pixel.
    Css,
    /// One image pixel per device pixel (default).
    #[default]
    Device,
}

/// Elements to cover with a box of `mask_color`: a selector resolved in a
/// frame (Playwright's locators are passed the same way).
#[derive(Debug, Clone)]
pub struct Mask {
    frame: Frame,
    selector: String,
}

impl Mask {
    /// Every element matching `selector` in `frame`.
    pub fn new(frame: &Frame, selector: impl Into<String>) -> Self {
        Self {
            frame: frame.clone(),
            selector: selector.into(),
        }
    }

    /// Every element matching `selector` in the page's main frame.
    pub fn page(page: &Page, selector: impl Into<String>) -> Self {
        Self::new(&page.main_frame(), selector)
    }
}

impl From<&Locator> for Mask {
    /// Every element the locator matches (in any of its frames).
    fn from(locator: &Locator) -> Self {
        Self::new(locator.frame(), locator.selector())
    }
}

impl From<Locator> for Mask {
    fn from(locator: Locator) -> Self {
        Mask::from(&locator)
    }
}

const PREPARE_FOR_SCREENSHOTS: &str = r#"function inPagePrepareForScreenshots(screenshotStyle, hideCaret, disableAnimations, syncAnimations) {
  if (syncAnimations) {
    const style = document.createElement('style');
    style.textContent = 'body {}';
    document.head.appendChild(style);
    document.documentElement.getBoundingClientRect();
    style.remove();
  }
  if (!screenshotStyle && !hideCaret && !disableAnimations)
    return;
  const collectRoots = (root, roots = []) => {
    roots.push(root);
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT);
    do {
      const node = walker.currentNode;
      const shadowRoot = node instanceof Element ? node.shadowRoot : null;
      if (shadowRoot)
        collectRoots(shadowRoot, roots);
    } while (walker.nextNode());
    return roots;
  };
  const roots = collectRoots(document);
  const cleanupCallbacks = [];
  if (screenshotStyle) {
    for (const root of roots) {
      const styleTag = document.createElement('style');
      styleTag.textContent = screenshotStyle;
      if (root === document)
        document.documentElement.append(styleTag);
      else
        root.append(styleTag);
      cleanupCallbacks.push(() => {
        styleTag.remove();
      });
    }
  }
  if (hideCaret) {
    const elements = new Map();
    for (const root of roots) {
      root.querySelectorAll('input,textarea,[contenteditable]').forEach(element => {
        elements.set(element, {
          value: element.style.getPropertyValue('caret-color'),
          priority: element.style.getPropertyPriority('caret-color')
        });
        element.style.setProperty('caret-color', 'transparent', 'important');
      });
    }
    cleanupCallbacks.push(() => {
      for (const [element, value] of elements)
        element.style.setProperty('caret-color', value.value, value.priority);
    });
  }
  if (disableAnimations) {
    const infiniteAnimationsToResume = new Set();
    const handleAnimations = (root) => {
      for (const animation of root.getAnimations()) {
        if (!animation.effect || animation.playbackRate === 0 || infiniteAnimationsToResume.has(animation))
          continue;
        const endTime = animation.effect.getComputedTiming().endTime;
        if (Number.isFinite(endTime)) {
          try {
            animation.finish();
          } catch (e) {
          }
        } else {
          try {
            animation.cancel();
            infiniteAnimationsToResume.add(animation);
          } catch (e) {
          }
        }
      }
    };
    for (const root of roots) {
      const handleRootAnimations = handleAnimations.bind(null, root);
      handleRootAnimations();
      root.addEventListener('transitionrun', handleRootAnimations);
      root.addEventListener('animationstart', handleRootAnimations);
      cleanupCallbacks.push(() => {
        root.removeEventListener('transitionrun', handleRootAnimations);
        root.removeEventListener('animationstart', handleRootAnimations);
      });
    }
    cleanupCallbacks.push(() => {
      for (const animation of infiniteAnimationsToResume) {
        try {
          animation.play();
        } catch (e) {
        }
      }
    });
  }
  window.__pwCleanupScreenshot = () => {
    for (const cleanupCallback of cleanupCallbacks)
      cleanupCallback();
    delete window.__pwCleanupScreenshot;
  };
}"#;

const CLEANUP_SCREENSHOT: &str = "window.__pwCleanupScreenshot && window.__pwCleanupScreenshot()";

/// Options shared by page and element screenshots.
#[derive(Debug, Clone, Default)]
struct ScreenshotOptions {
    path: Option<PathBuf>,
    r#type: Option<ScreenshotType>,
    quality: Option<i64>,
    full_page: bool,
    clip: Option<Rect>,
    omit_background: bool,
    animations: Animations,
    caret: Caret,
    scale: ScreenshotScale,
    mask: Vec<Mask>,
    mask_color: Option<String>,
    style: Option<String>,
    timeout: Option<u64>,
}

impl ScreenshotOptions {
    /// `validateScreenshotOptions` (+ the client's `determineScreenshotType`).
    fn format(&self) -> Result<ScreenshotType> {
        let format = match (self.r#type, &self.path) {
            (Some(t), _) => t,
            (None, Some(path)) => ScreenshotType::from_path(path)?,
            (None, None) => ScreenshotType::Png,
        };
        if let Some(quality) = self.quality {
            if format == ScreenshotType::Png {
                return Err(Error::invalid_argument(
                    "options.quality is unsupported for the png screenshots",
                ));
            }
            if !(0..=100).contains(&quality) {
                return Err(Error::invalid_argument(format!(
                    "Expected options.quality to be between 0 and 100 (inclusive), got {quality}"
                )));
            }
        }
        if let Some(clip) = &self.clip {
            if clip.width <= 0.0 {
                return Err(Error::invalid_argument(
                    "Expected options.clip.width to be greater than 0.",
                ));
            }
            if clip.height <= 0.0 {
                return Err(Error::invalid_argument(
                    "Expected options.clip.height to be greater than 0.",
                ));
            }
        }
        Ok(format)
    }
}

macro_rules! screenshot_builder_methods {
    () => {
        /// Also save the image to this file (its extension sets the
        /// format unless [`Self::r#type`] is given).
        pub fn path(mut self, path: impl Into<PathBuf>) -> Self {
            self.options.path = Some(path.into());
            self
        }

        /// Image format (default PNG).
        pub fn r#type(mut self, format: ScreenshotType) -> Self {
            self.options.r#type = Some(format);
            self
        }

        /// JPEG/WebP quality, 0-100.
        pub fn quality(mut self, quality: i64) -> Self {
            self.options.quality = Some(quality);
            self
        }

        /// Make the default white background transparent (PNG/WebP).
        pub fn omit_background(mut self, omit: bool) -> Self {
            self.options.omit_background = omit;
            self
        }

        /// Stop CSS animations and transitions during the screenshot.
        pub fn animations(mut self, animations: Animations) -> Self {
            self.options.animations = animations;
            self
        }

        /// Hide the text caret (default) or keep it.
        pub fn caret(mut self, caret: Caret) -> Self {
            self.options.caret = caret;
            self
        }

        /// CSS or device pixels (default device).
        pub fn scale(mut self, scale: ScreenshotScale) -> Self {
            self.options.scale = scale;
            self
        }

        /// Cover elements with a box of [`Self::mask_color`].
        pub fn mask<I>(mut self, masks: I) -> Self
        where
            I: IntoIterator,
            I::Item: Into<Mask>,
        {
            self.options.mask.extend(masks.into_iter().map(Into::into));
            self
        }

        /// Color of masks (CSS color, default `#FF00FF`).
        pub fn mask_color(mut self, color: impl Into<String>) -> Self {
            self.options.mask_color = Some(color.into());
            self
        }

        /// Stylesheet applied while taking the screenshot (also inside
        /// shadow roots and frames).
        pub fn style(mut self, css: impl Into<String>) -> Self {
            self.options.style = Some(css.into());
            self
        }

        /// Maximum time in milliseconds, `0` disables the timeout.
        pub fn timeout(mut self, timeout_ms: u64) -> Self {
            self.options.timeout = Some(timeout_ms);
            self
        }
    };
}

/// Builder returned by [`Page::screenshot`]; `.await` it for the image bytes.
#[must_use = "builders do nothing unless awaited"]
pub struct PageScreenshot {
    page: Page,
    options: ScreenshotOptions,
}

impl PageScreenshot {
    screenshot_builder_methods!();

    /// Capture the whole scrollable page instead of the viewport.
    pub fn full_page(mut self, full_page: bool) -> Self {
        self.options.full_page = full_page;
        self
    }

    /// Capture only this area (CSS pixels, relative to the viewport or to
    /// the page with [`Self::full_page`]).
    pub fn clip(mut self, clip: Rect) -> Self {
        self.options.clip = Some(clip);
        self
    }
}

/// Builder returned by [`ElementHandle::screenshot`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct ElementScreenshot {
    target: ScreenshotTarget,
    api: &'static str,
    options: ScreenshotOptions,
}

impl ElementScreenshot {
    screenshot_builder_methods!();
}

/// What an [`ElementScreenshot`] captures.
enum ScreenshotTarget {
    Handle(ElementHandle),
    /// Resolved when awaited (`Locator._withElement`).
    Locator(Locator),
}

impl Page {
    /// Take a screenshot of the viewport (or the full page / a clip).
    ///
    /// ```no_run
    /// # async fn run(page: oxidewright::Page) -> oxidewright::Result<()> {
    /// let png = page.screenshot().full_page(true).await?;
    /// page.screenshot().path("shot.jpg").quality(80).await?;
    /// # let _ = png; Ok(()) }
    /// ```
    pub fn screenshot(&self) -> PageScreenshot {
        PageScreenshot {
            page: self.clone(),
            options: ScreenshotOptions::default(),
        }
    }
}

impl ElementHandle {
    /// Take a screenshot of the element, scrolling it into view first.
    pub fn screenshot(&self) -> ElementScreenshot {
        ElementScreenshot {
            target: ScreenshotTarget::Handle(self.clone()),
            api: "elementHandle.screenshot",
            options: ScreenshotOptions::default(),
        }
    }
}

impl Locator {
    /// Take a screenshot of the element this locator resolves to (waiting
    /// for it, strict), like [`ElementHandle::screenshot`].
    pub fn screenshot(&self) -> ElementScreenshot {
        ElementScreenshot {
            target: ScreenshotTarget::Locator(self.clone()),
            api: "locator.screenshot",
            options: ScreenshotOptions::default(),
        }
    }
}

impl IntoFuture for PageScreenshot {
    type Output = Result<Vec<u8>>;
    type IntoFuture = BoxFuture<Result<Vec<u8>>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let timeout = self.page.inner.timeouts.timeout(self.options.timeout);
            let progress = Progress::new("page.screenshot", timeout);
            let page = self.page.clone();
            let options = self.options;
            let bytes = progress
                .run(screenshot_page(&page, &options, &progress))
                .await?;
            if let Some(path) = &options.path {
                crate::storage_state::write_file(path, &bytes).await?;
            }
            Ok(bytes)
        })
    }
}

impl IntoFuture for ElementScreenshot {
    type Output = Result<Vec<u8>>;
    type IntoFuture = BoxFuture<Result<Vec<u8>>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let api = self.api;
            let started = std::time::Instant::now();
            let (handle, owned, timeout) = match self.target {
                ScreenshotTarget::Handle(handle) => (handle, false, None),
                ScreenshotTarget::Locator(locator) => {
                    let page = locator
                        .page()
                        .ok_or_else(|| Error::target_closed().with_api(api))?;
                    let timeout = page.inner.timeouts.timeout(self.options.timeout);
                    let handle = locator
                        .element_handle_as(api)
                        .timeout(timeout)
                        .await
                        .map_err(|e| e.with_api(api))?;
                    (handle, true, Some(timeout))
                }
            };
            let frame = handle.context().frame();
            let page = frame
                .as_ref()
                .and_then(|f| f.page())
                .ok_or_else(|| Error::target_closed().with_api(api))?;
            // A locator shares one timeout between resolving and capturing.
            let timeout = match timeout {
                Some(0) => 0,
                Some(t) => t
                    .saturating_sub(started.elapsed().as_millis() as u64)
                    .max(1),
                None => page.inner.timeouts.timeout(self.options.timeout),
            };
            let progress = Progress::new(api, timeout);
            let options = self.options;
            let result = progress
                .run(screenshot_element(&page, &handle, &options, &progress))
                .await;
            if owned {
                handle.dispose().await;
            }
            let bytes = result?;
            if let Some(path) = &options.path {
                crate::storage_state::write_file(path, &bytes).await?;
            }
            Ok(bytes)
        })
    }
}

// ----------------------------------------------------------------------
// Screenshotter
// ----------------------------------------------------------------------

async fn utility_value<T: serde::de::DeserializeOwned>(frame: &Frame, function: &str) -> Result<T> {
    match frame
        .evaluate_in(World::Utility, function, None, true)
        .await?
    {
        Evaluated::Value(v) => from_js_value(v).map_err(Error::serialization),
        Evaluated::Handle(_) => Err(Error::evaluation("unexpected handle")),
    }
}

/// `waitForFunctionValueInUtility`: poll until the function returns a
/// non-null value.
async fn wait_for_value<T: serde::de::DeserializeOwned>(
    frame: &Frame,
    function: &str,
) -> Result<T> {
    loop {
        match utility_value::<Option<T>>(frame, function).await {
            Ok(Some(value)) => return Ok(value),
            Ok(None) => {}
            Err(err) if err.message().contains(crate::CONTEXT_DESTROYED_MESSAGE) => {}
            Err(err) => return Err(err),
        }
        tokio::time::sleep(Duration::from_millis(16)).await;
    }
}

async fn original_viewport_size(page: &Page) -> Result<ViewportSize> {
    if let Some(size) = crate::emulation::emulated_size(&page.inner) {
        return Ok(size.viewport);
    }
    wait_for_value(
        &page.main_frame(),
        "() => ({ width: window.innerWidth, height: window.innerHeight })",
    )
    .await
}

async fn full_page_size(page: &Page) -> Result<ViewportSize> {
    let size: (f64, f64) = wait_for_value(
        &page.main_frame(),
        "() => {
          if (!document.body || !document.documentElement)
            return null;
          return [
            Math.max(
              document.body.scrollWidth, document.documentElement.scrollWidth,
              document.body.offsetWidth, document.documentElement.offsetWidth,
              document.body.clientWidth, document.documentElement.clientWidth
            ),
            Math.max(
              document.body.scrollHeight, document.documentElement.scrollHeight,
              document.body.offsetHeight, document.documentElement.offsetHeight,
              document.body.clientHeight, document.documentElement.clientHeight
            ),
          ];
        }",
    )
    .await?;
    Ok(ViewportSize {
        width: size.0 as u32,
        height: size.1 as u32,
    })
}

async fn prepare_page(
    page: &Page,
    frame: &Frame,
    options: &ScreenshotOptions,
    progress: &Progress,
) -> Result<()> {
    let disable_animations = options.animations == Animations::Disabled;
    if disable_animations {
        progress.log("  disabled all CSS animations");
    }
    let style = serde_json::to_string(&options.style)?;
    let expression = format!(
        "({PREPARE_FOR_SCREENSHOTS})({style}, {}, {disable_animations}, false)",
        options.caret != Caret::Initial
    );
    crate::clock::evaluate_in_page_frames(page, &expression, World::Utility, false).await?;
    progress.log("waiting for fonts to load...");
    let _ = frame
        .evaluate_in(World::Utility, "document.fonts.ready", None, true)
        .await;
    progress.log("fonts loaded");
    Ok(())
}

async fn restore_page(page: &Page) {
    let _ = crate::clock::evaluate_in_page_frames(page, CLEANUP_SCREENSHOT, World::Utility, false)
        .await;
}

/// `trimClipToSize`.
fn trim_clip_to_size(clip: &Rect, width: f64, height: f64) -> Result<Rect> {
    let x1 = clip.x.min(width).max(0.0);
    let y1 = clip.y.min(height).max(0.0);
    let x2 = (clip.x + clip.width).min(width).max(0.0);
    let y2 = (clip.y + clip.height).min(height).max(0.0);
    let result = Rect::new(x1, y1, x2 - x1, y2 - y1);
    if !(result.width > 0.0 && result.height > 0.0) {
        return Err(Error::invalid_argument(
            "Clipped area is either empty or outside the resulting image",
        ));
    }
    Ok(result)
}

/// `helper.enclosingIntRect`.
fn enclosing_int_rect(rect: &Rect) -> Rect {
    let x = (rect.x + 1e-3).floor();
    let y = (rect.y + 1e-3).floor();
    let x2 = (rect.x + rect.width - 1e-3).ceil();
    let y2 = (rect.y + rect.height - 1e-3).ceil();
    Rect::new(x, y, x2 - x, y2 - y)
}

async fn screenshot_page(
    page: &Page,
    options: &ScreenshotOptions,
    progress: &Progress,
) -> Result<Vec<u8>> {
    let format = options.format()?;
    let _queue = page.inner.emu.screenshot_queue.lock().await;
    progress.log("taking page screenshot");
    let viewport = original_viewport_size(page).await?;
    prepare_page(page, &page.main_frame(), options, progress).await?;
    let result = async {
        if options.full_page {
            let full = full_page_size(page).await?;
            let mut document_rect = Rect::new(0.0, 0.0, full.width as f64, full.height as f64);
            let fits = full.width <= viewport.width && full.height <= viewport.height;
            if let Some(clip) = &options.clip {
                document_rect = trim_clip_to_size(clip, document_rect.width, document_rect.height)?;
            }
            return screenshot(page, format, Some(document_rect), None, fits, options).await;
        }
        let viewport_rect = match &options.clip {
            Some(clip) => trim_clip_to_size(clip, viewport.width as f64, viewport.height as f64)?,
            None => Rect::new(0.0, 0.0, viewport.width as f64, viewport.height as f64),
        };
        screenshot(page, format, None, Some(viewport_rect), true, options).await
    }
    .await;
    restore_page(page).await;
    result
}

async fn screenshot_element(
    page: &Page,
    handle: &ElementHandle,
    options: &ScreenshotOptions,
    progress: &Progress,
) -> Result<Vec<u8>> {
    let format = options.format()?;
    let _queue = page.inner.emu.screenshot_queue.lock().await;
    progress.log("taking element screenshot");
    let viewport = original_viewport_size(page).await?;
    let frame = handle
        .context()
        .frame()
        .ok_or_else(|| Error::frame_detached("Frame was detached"))?;
    prepare_page(page, &frame, options, progress).await?;
    let result = async {
        wait_and_scroll_into_view(handle, progress).await?;
        let bbox = bounding_box(handle)
            .await?
            .ok_or_else(|| Error::evaluation("Node is either not visible or not an HTMLElement"))?;
        if bbox.width == 0.0 {
            return Err(Error::evaluation("Node has 0 width."));
        }
        if bbox.height == 0.0 {
            return Err(Error::evaluation("Node has 0 height."));
        }
        let fits = bbox.width <= viewport.width as f64 && bbox.height <= viewport.height as f64;
        let scroll: (f64, f64) =
            utility_value(&page.main_frame(), "() => [window.scrollX, window.scrollY]").await?;
        let document_rect = Rect::new(
            bbox.x + scroll.0,
            bbox.y + scroll.1,
            bbox.width,
            bbox.height,
        );
        screenshot(
            page,
            format,
            Some(enclosing_int_rect(&document_rect)),
            None,
            fits,
            options,
        )
        .await
    }
    .await;
    restore_page(page).await;
    result
}

/// `_waitAndScrollIntoViewIfNeeded(waitForVisible = true)`.
async fn wait_and_scroll_into_view(handle: &ElementHandle, progress: &Progress) -> Result<()> {
    let wait_time = [0u64, 20, 100, 100, 500];
    let mut retry = 0usize;
    loop {
        if retry > 0 {
            progress.log("retrying scroll into view action");
            let timeout = wait_time[(retry - 1).min(wait_time.len() - 1)];
            if timeout > 0 {
                progress.log(format!("  waiting {timeout}ms"));
                tokio::time::sleep(Duration::from_millis(timeout)).await;
            }
        } else {
            progress.log("attempting scroll into view action");
        }
        retry += 1;
        progress.log("  waiting for element to be stable");
        let state = check_visible_and_stable(handle).await?;
        match state {
            Value::Null => {}
            Value::String(s) if s == "error:notconnected" => {
                return Err(Error::evaluation("Element is not attached to the DOM"));
            }
            Value::Object(map) => {
                if let Some(missing) = map.get("missingState").and_then(Value::as_str) {
                    progress.log(format!("  element is not {missing}"));
                }
                continue;
            }
            _ => {}
        }
        match scroll_into_view_if_needed(handle).await? {
            ScrollResult::Done => return Ok(()),
            ScrollResult::NotVisible => {
                progress.log("  element is not visible");
                continue;
            }
            ScrollResult::NotConnected => {
                return Err(Error::evaluation("Element is not attached to the DOM"));
            }
        }
    }
}

async fn check_visible_and_stable(handle: &ElementHandle) -> Result<Value> {
    let frame = handle
        .context()
        .frame()
        .ok_or_else(|| Error::frame_detached("Frame was detached"))?;
    let context = frame.context(World::Utility).await?;
    let injected = context.injected_script().await?;
    let (arg, handles) = serialize_arg(&(injected, handle.clone()))?.unwrap_or_default();
    let result = context
        .evaluate_expression(
            "async ([injected, node]) => (await injected.checkElementStates(node, ['visible', 'stable'], true)) ?? null",
            Some(true),
            true,
            vec![arg],
            handles,
        )
        .await?;
    match result {
        Evaluated::Value(v) => Ok(v.to_json()),
        Evaluated::Handle(_) => Ok(Value::Null),
    }
}

enum ScrollResult {
    Done,
    NotVisible,
    NotConnected,
}

async fn scroll_into_view_if_needed(handle: &ElementHandle) -> Result<ScrollResult> {
    let object_id = handle
        .object_id()
        .cloned()
        .ok_or_else(|| Error::evaluation("JSHandle is disposed!"))?;
    let result = handle
        .context()
        .session()
        .execute(RawCommand::new(
            "DOM.scrollIntoViewIfNeeded",
            json!({ "objectId": object_id.as_ref() }),
        ))
        .await;
    match result {
        Ok(_) => Ok(ScrollResult::Done),
        Err(err) => {
            let message = err.to_string();
            if message.contains("Node does not have a layout object") {
                Ok(ScrollResult::NotVisible)
            } else if message.contains("Node is detached from document") {
                Ok(ScrollResult::NotConnected)
            } else {
                Err(Error::from(err))
            }
        }
    }
}

/// `CRPage._getBoundingBox`: the border box in main-frame coordinates.
pub(crate) async fn bounding_box(handle: &ElementHandle) -> Result<Option<Rect>> {
    let Some(object_id) = handle.object_id().cloned() else {
        return Ok(None);
    };
    let result = handle
        .context()
        .session()
        .execute(RawCommand::new(
            "DOM.getBoxModel",
            json!({ "objectId": object_id.as_ref() }),
        ))
        .await;
    let Ok(result) = result else {
        return Ok(None);
    };
    let quad: Vec<f64> = result
        .result
        .get("model")
        .and_then(|m| m.get("border"))
        .and_then(|b| serde_json::from_value(b.clone()).ok())
        .unwrap_or_default();
    if quad.len() < 8 {
        return Ok(None);
    }
    let xs = [quad[0], quad[2], quad[4], quad[6]];
    let ys = [quad[1], quad[3], quad[5], quad[7]];
    let x = xs.iter().cloned().fold(f64::INFINITY, f64::min);
    let y = ys.iter().cloned().fold(f64::INFINITY, f64::min);
    let width = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max) - x;
    let height = ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max) - y;
    let Some(position) = frame_position(handle).await? else {
        return Ok(None);
    };
    Ok(Some(Rect::new(
        x + position.0,
        y + position.1,
        width,
        height,
    )))
}

/// `CRPage._framePosition`: offset of the handle's frame in the page.
/// Frames sharing the main frame's session report main-frame coordinates.
async fn frame_position(handle: &ElementHandle) -> Result<Option<(f64, f64)>> {
    let Some(frame) = handle.context().frame() else {
        return Ok(None);
    };
    let session = handle.context().session().session_id().clone();
    // The root frame of the handle's session: the main frame or an OOPIF.
    let mut root = frame;
    while let Some(parent) = root.parent_frame() {
        if parent.cdp_frame().await?.session_id() != &session {
            break;
        }
        root = parent;
    }
    if root.parent_frame().is_none() {
        return Ok(Some((0.0, 0.0)));
    }
    let element = root.frame_element().await?;
    let bbox = Box::pin(bounding_box(&element)).await?;
    element.dispose().await;
    Ok(bbox.map(|b| (b.x, b.y)))
}

async fn send(page: &PageInner, method: &str, params: Value) -> Result<Value> {
    Ok(page
        .cdp
        .execute(RawCommand::new(method, params))
        .await?
        .result)
}

async fn set_background(page: &Page, transparent: bool) -> Result<()> {
    let params = if transparent {
        json!({ "color": { "r": 0, "g": 0, "b": 0, "a": 0 } })
    } else {
        json!({})
    };
    send(
        &page.inner,
        "Emulation.setDefaultBackgroundColorOverride",
        params,
    )
    .await?;
    Ok(())
}

/// `Screenshotter._maskElements`: masks are resolved like any selector
/// (`frameSelectors.callOnSelector`, non-strict), so locators inside frames
/// work too. Returns the frames to clean up (all of the page's, like
/// `highlightController.hideHighlights`).
async fn mask_elements(page: &Page, options: &ScreenshotOptions) -> Result<Vec<Frame>> {
    if options.mask.is_empty() {
        return Ok(Vec::new());
    }
    let color = options
        .mask_color
        .clone()
        .unwrap_or_else(|| "#F0F".to_string());
    for mask in &options.mask {
        call_on_selector(
            &mask.frame,
            &SelectorTarget::new(mask.selector.clone(), false),
            "({ injected, elements }, color) => { injected.addMaskedElements(elements, color); }",
            CallArg::Json(Value::String(color.clone())),
            CallOptions::default(),
        )
        .await?;
    }
    Ok(page.frames())
}

async fn hide_masks(frames: &[Frame]) {
    for frame in frames {
        if let Ok(context) = frame.context(World::Utility).await {
            if let Ok(injected) = context.injected_script().await {
                let _ = injected
                    .evaluate::<()>("injected => injected.hideHighlight()")
                    .await;
            }
        }
    }
}

/// `Screenshotter._screenshot` + `CRPage.takeScreenshot`.
async fn screenshot(
    page: &Page,
    format: ScreenshotType,
    document_rect: Option<Rect>,
    viewport_rect: Option<Rect>,
    fits_viewport: bool,
    options: &ScreenshotOptions,
) -> Result<Vec<u8>> {
    let transparent = options.omit_background && format != ScreenshotType::Jpeg;
    if transparent {
        set_background(page, true).await?;
    }
    let masked = match mask_elements(page, options).await {
        Ok(frames) => frames,
        Err(err) => {
            if transparent {
                let _ = set_background(page, false).await;
            }
            return Err(err);
        }
    };
    let quality = match format {
        ScreenshotType::Jpeg => Some(options.quality.unwrap_or(80)),
        ScreenshotType::Webp => Some(options.quality.unwrap_or(100)),
        ScreenshotType::Png => None,
    };
    let result = take_screenshot(
        page,
        format,
        document_rect,
        viewport_rect,
        quality,
        fits_viewport,
        options.scale,
    )
    .await;
    hide_masks(&masked).await;
    if transparent {
        let _ = set_background(page, false).await;
    }
    result
}

async fn take_screenshot(
    page: &Page,
    format: ScreenshotType,
    document_rect: Option<Rect>,
    viewport_rect: Option<Rect>,
    quality: Option<i64>,
    fits_viewport: bool,
    scale: ScreenshotScale,
) -> Result<Vec<u8>> {
    let inner = &page.inner;
    let metrics = send(inner, "Page.getLayoutMetrics", Value::Null).await?;
    let visual = metrics
        .get("visualViewport")
        .cloned()
        .unwrap_or(Value::Null);
    let num = |v: &Value, key: &str| v.get(key).and_then(Value::as_f64).unwrap_or(0.0);
    let page_x = num(&visual, "pageX");
    let page_y = num(&visual, "pageY");
    let visual_scale = visual.get("scale").and_then(Value::as_f64).unwrap_or(1.0);
    let document_rect = match document_rect {
        Some(rect) => rect,
        None => {
            let vr = viewport_rect.unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0));
            Rect::new(
                page_x + vr.x,
                page_y + vr.y,
                (vr.width / visual_scale + 1e-3).floor(),
                (vr.height / visual_scale + 1e-3).floor(),
            )
        }
    };
    let mut clip_scale = if viewport_rect.is_some() {
        visual_scale
    } else {
        1.0
    };
    if scale == ScreenshotScale::Css {
        let content = metrics.get("contentSize").cloned().unwrap_or(Value::Null);
        let css_content = metrics
            .get("cssContentSize")
            .cloned()
            .unwrap_or(Value::Null);
        let override_dsf = inner
            .emu
            .metrics_override
            .lock()
            .unwrap()
            .map(|m| m.device_scale_factor)
            .filter(|v| *v != 0.0);
        let ratio = {
            let css_width = num(&css_content, "width");
            if css_width > 0.0 {
                num(&content, "width") / css_width
            } else {
                0.0
            }
        };
        let dsf = override_dsf
            .or(Some(ratio).filter(|v| *v != 0.0 && v.is_finite()))
            .unwrap_or(1.0);
        clip_scale /= dsf;
    }
    let mut params = json!({
        "format": format.as_str(),
        "clip": {
            "x": document_rect.x,
            "y": document_rect.y,
            "width": document_rect.width,
            "height": document_rect.height,
            "scale": clip_scale,
        },
        "captureBeyondViewport": !fits_viewport,
    });
    if let Some(quality) = quality {
        params["quality"] = json!(quality);
    }
    let result = send(inner, "Page.captureScreenshot", params).await?;
    if !fits_viewport {
        let has_touch = inner
            .context
            .upgrade()
            .is_some_and(|c| c.options.has_touch == Some(true));
        if has_touch {
            // Capturing beyond the viewport resets touch emulation.
            send(
                inner,
                "Emulation.setTouchEmulationEnabled",
                json!({ "enabled": true }),
            )
            .await?;
        }
    }
    let data = result
        .get("data")
        .and_then(Value::as_str)
        .unwrap_or_default();
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| Error::serialization(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_options_like_playwright() {
        let mut options = ScreenshotOptions {
            quality: Some(10),
            ..Default::default()
        };
        let err = options.format().unwrap_err();
        assert!(
            err.to_string()
                .contains("options.quality is unsupported for the png")
        );
        options.r#type = Some(ScreenshotType::Jpeg);
        assert_eq!(options.format().unwrap(), ScreenshotType::Jpeg);
        options.quality = Some(101);
        assert!(options.format().is_err());
        let options = ScreenshotOptions {
            path: Some("a/b.jpg".into()),
            ..Default::default()
        };
        assert_eq!(options.format().unwrap(), ScreenshotType::Jpeg);
        let options = ScreenshotOptions {
            path: Some("a/b.txt".into()),
            ..Default::default()
        };
        assert!(
            options
                .format()
                .unwrap_err()
                .to_string()
                .contains("unsupported mime type \"text/plain\"")
        );
        let options = ScreenshotOptions {
            clip: Some(Rect::new(0.0, 0.0, 0.0, 10.0)),
            ..Default::default()
        };
        assert!(
            options
                .format()
                .unwrap_err()
                .to_string()
                .contains("clip.width")
        );
    }

    #[test]
    fn trims_clips() {
        let r = trim_clip_to_size(&Rect::new(-10.0, -10.0, 50.0, 50.0), 100.0, 100.0).unwrap();
        assert_eq!(r, Rect::new(0.0, 0.0, 40.0, 40.0));
        assert!(trim_clip_to_size(&Rect::new(200.0, 0.0, 50.0, 50.0), 100.0, 100.0).is_err());
        let r = enclosing_int_rect(&Rect::new(0.5, 0.5, 10.2, 10.0));
        assert_eq!(r, Rect::new(0.0, 0.0, 11.0, 11.0));
    }
}
