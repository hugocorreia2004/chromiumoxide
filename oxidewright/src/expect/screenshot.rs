//! `toHaveScreenshot` against a PNG file: a port of `Page.expectScreenshot`
//! (`server/page.ts`), of the `pixelmatch` comparator
//! (`utils/comparators.ts`) and of the snapshot handling of
//! `matchers/toMatchSnapshot.ts`, without the test runner's snapshot path
//! templates (the expected file is given explicitly).
//!
//! * Missing expected file: a stable screenshot (two consecutive identical
//!   captures) is written to the path and the assertion fails with
//!   `A snapshot doesn't exist at <path>, writing actual.` (Playwright's
//!   `--update-snapshots=missing` behaviour).
//! * Different: `<name>-actual.png` and `<name>-diff.png` are written next
//!   to the expected file and the message reports the pixel difference.
//! * Like Playwright, animations are disabled, the caret hidden and the CSS
//!   scale used by default.

use std::future::{Future, IntoFuture};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::{Duration, Instant};

use super::format::{MessageDetails, call_log_text, format_matcher_message};
use super::{Config, MatcherResult};
use crate::error::{Error, Result, compress_call_log};
use crate::frame_selectors::is_non_retriable;
use crate::js::JsValue;
use crate::locator::Locator;
use crate::page::Page;
use crate::screenshot::{Animations, Caret, Mask, ScreenshotScale};

#[derive(Debug, Clone)]
pub(super) enum Shot {
    Page(Page),
    Locator(Locator),
}

/// `expect(..).to_have_screenshot(path)`.
#[must_use = "assertions do nothing unless awaited"]
#[derive(Debug, Clone)]
pub struct ToHaveScreenshot {
    shot: Shot,
    cfg: Config,
    path: PathBuf,
    timeout: Option<u64>,
    threshold: f64,
    max_diff_pixels: Option<u64>,
    max_diff_pixel_ratio: Option<f64>,
    full_page: bool,
    omit_background: bool,
    animations: Animations,
    caret: Caret,
    scale: ScreenshotScale,
    mask: Vec<Mask>,
    mask_color: Option<String>,
    clip: Option<crate::screenshot::Rect>,
    style_paths: Vec<PathBuf>,
}

impl ToHaveScreenshot {
    pub(super) fn new(shot: Shot, cfg: Config, path: PathBuf) -> Self {
        ToHaveScreenshot {
            shot,
            cfg,
            path,
            timeout: None,
            threshold: 0.2,
            max_diff_pixels: None,
            max_diff_pixel_ratio: None,
            full_page: false,
            omit_background: false,
            animations: Animations::Disabled,
            caret: Caret::Hide,
            scale: ScreenshotScale::Css,
            mask: Vec::new(),
            mask_color: None,
            clip: None,
            style_paths: Vec::new(),
        }
    }

    /// Timeout of this assertion in milliseconds.
    pub fn timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout = Some(timeout_ms);
        self
    }
    /// Per-pixel color distance tolerance in the YIQ space, `0..=1`
    /// (default `0.2`).
    pub fn threshold(mut self, threshold: f64) -> Self {
        self.threshold = threshold;
        self
    }
    /// Number of pixels allowed to differ (default `0`).
    pub fn max_diff_pixels(mut self, pixels: u64) -> Self {
        self.max_diff_pixels = Some(pixels);
        self
    }
    /// Ratio of pixels allowed to differ, `0..=1`.
    pub fn max_diff_pixel_ratio(mut self, ratio: f64) -> Self {
        self.max_diff_pixel_ratio = Some(ratio);
        self
    }
    /// Capture the full scrollable page (pages only).
    pub fn full_page(mut self, full_page: bool) -> Self {
        self.full_page = full_page;
        self
    }
    /// Transparent background instead of the default white.
    pub fn omit_background(mut self, omit: bool) -> Self {
        self.omit_background = omit;
        self
    }
    /// CSS animations and transitions (default: disabled).
    pub fn animations(mut self, animations: Animations) -> Self {
        self.animations = animations;
        self
    }
    /// Text caret (default: hidden).
    pub fn caret(mut self, caret: Caret) -> Self {
        self.caret = caret;
        self
    }
    /// Image scale (default: CSS pixels).
    pub fn scale(mut self, scale: ScreenshotScale) -> Self {
        self.scale = scale;
        self
    }
    /// Elements to cover with a box of `mask_color`.
    pub fn mask<I>(mut self, masks: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<Mask>,
    {
        self.mask.extend(masks.into_iter().map(Into::into));
        self
    }
    /// Color of the masks (default `#FF00FF`).
    pub fn mask_color(mut self, color: impl Into<String>) -> Self {
        self.mask_color = Some(color.into());
        self
    }

    /// Only compare this area of the page (pages only, CSS pixels).
    pub fn clip(mut self, clip: crate::screenshot::Rect) -> Self {
        self.clip = Some(clip);
        self
    }

    /// Stylesheet files applied while taking the screenshot (Playwright's
    /// `stylePath`, e.g. to hide dynamic content).
    pub fn style_path<I, P>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        self.style_paths = paths.into_iter().map(Into::into).collect();
        self
    }

    async fn capture(&self, timeout: u64) -> Result<Vec<u8>> {
        let mut style = Vec::new();
        for path in &self.style_paths {
            let text = tokio::fs::read_to_string(path).await.map_err(|e| {
                Error::invalid_argument(format!("Failed to read {}: {e}", path.display()))
            })?;
            style.push(text.trim().to_string());
        }
        let style = (!style.is_empty()).then(|| {
            style.join(
                "
",
            )
        });
        macro_rules! apply {
            ($b:expr) => {{
                let mut b = $b
                    .omit_background(self.omit_background)
                    .animations(self.animations)
                    .caret(self.caret)
                    .scale(self.scale)
                    .mask(self.mask.clone())
                    .timeout(timeout);
                if let Some(color) = &self.mask_color {
                    b = b.mask_color(color.clone());
                }
                if let Some(css) = &style {
                    b = b.style(css.clone());
                }
                b
            }};
        }
        match &self.shot {
            Shot::Page(p) => {
                let mut shot = p.screenshot().full_page(self.full_page);
                if let Some(clip) = self.clip {
                    shot = shot.clip(clip);
                }
                apply!(shot).await
            }
            Shot::Locator(l) => apply!(l.screenshot()).await,
        }
    }

    fn page(&self) -> Option<Page> {
        match &self.shot {
            Shot::Page(p) => Some(p.clone()),
            Shot::Locator(l) => l.page(),
        }
    }

    /// `Page.expectScreenshot`: polls screenshots until they match
    /// `expected` (or, without one, until two consecutive ones are equal).
    async fn expect_screenshot(&self, expected: Option<&Image>, timeout: u64) -> Outcome {
        let mut log = Vec::new();
        log.push(format!(
            "{}{}{}",
            self.cfg.title("toHaveScreenshot"),
            match &self.shot {
                Shot::Locator(l) => format!(" {l}"),
                Shot::Page(_) => String::new(),
            },
            if timeout > 0 {
                format!(" with timeout {timeout}ms")
            } else {
                String::new()
            }
        ));
        log.push(if expected.is_some() {
            "  verifying given screenshot expectation".to_string()
        } else {
            "  generating new stable screenshot expectation".to_string()
        });
        let start = Instant::now();
        let mut intervals = vec![0u64, 100, 250, 500].into_iter();
        let mut actual: Option<Vec<u8>> = None;
        let mut previous: Option<Vec<u8>>;
        let mut intermediate: Option<Comparison> = None;
        let mut first = true;
        let remaining = |start: Instant| -> Option<u64> {
            if timeout == 0 {
                return Some(0);
            }
            let elapsed = start.elapsed().as_millis() as u64;
            (elapsed < timeout).then(|| timeout - elapsed)
        };
        let timed_out = loop {
            if self.page().is_some_and(|p| p.is_closed()) {
                return Outcome::failed(log, "The page has closed", intermediate, false);
            }
            let wait = intervals.next().unwrap_or(1000);
            if wait > 0 {
                log.push(format!("waiting {wait}ms before taking screenshot"));
                let Some(left) = remaining(start) else {
                    break true;
                };
                let wait = if timeout > 0 { wait.min(left) } else { wait };
                tokio::time::sleep(Duration::from_millis(wait)).await;
            }
            let Some(left) = remaining(start) else {
                break true;
            };
            previous = actual.take();
            match self.capture(left).await {
                Ok(bytes) => actual = Some(bytes),
                Err(e) => {
                    if !matches!(e, Error::Timeout { .. }) && is_non_retriable(&e, None) {
                        return Outcome::error(e);
                    }
                    log.push(format!("failed to take screenshot - {}", e.message()));
                    actual = previous.take();
                    continue;
                }
            }
            let current = actual.as_deref().unwrap();
            let current_image = match Image::decode(current) {
                Ok(i) => i,
                Err(e) => return Outcome::error(e),
            };
            let expectation = if first && expected.is_some() {
                expected.cloned()
            } else {
                previous.as_deref().and_then(|p| Image::decode(p).ok())
            };
            if let Some(expectation) = expectation {
                match compare(&current_image, &expectation, self) {
                    None => break false,
                    Some(mut c) => {
                        log.push(c.message.clone());
                        c.actual = actual.clone();
                        c.previous = previous.clone();
                        intermediate = Some(c);
                    }
                }
            }
            first = false;
        };
        if timed_out {
            let message = match &intermediate {
                Some(i) if i.previous.is_some() && expected.is_none() => {
                    "Failed to take two consecutive stable screenshots.".to_string()
                }
                Some(i) if i.previous.is_some() => {
                    "Failed to take two consecutive stable screenshots.".to_string()
                }
                _ => format!("Timeout {timeout}ms exceeded."),
            };
            log.push(format!("Timeout {timeout}ms exceeded."));
            return Outcome::failed(log, &message, intermediate, true);
        }
        if !first {
            log.push("captured a stable screenshot".to_string());
        }
        let Some(expected) = expected else {
            return Outcome {
                actual,
                ..Default::default()
            };
        };
        if first {
            return Outcome::default();
        }
        let image = Image::decode(actual.as_deref().unwrap_or_default());
        match image.map(|i| compare(&i, expected, self)) {
            Ok(None) => Outcome::default(),
            Ok(Some(mut c)) => {
                let message = c.message.clone();
                c.actual = actual;
                Outcome::failed(log, &message, Some(c), false)
            }
            Err(e) => Outcome::error(e),
        }
    }

    async fn run(self) -> Result<()> {
        let cfg = &self.cfg;
        let timeout = cfg.timeout(self.timeout);
        let locator = match &self.shot {
            Shot::Locator(l) => Some(l.to_string()),
            Shot::Page(_) => None,
        };
        let expected_path = self.path.clone();
        let display = expected_path.display().to_string();
        let result = |message: String, pass: bool, log: Vec<String>| {
            cfg.fail(MatcherResult {
                name: "toHaveScreenshot".to_string(),
                pass,
                expected: Some(JsValue::String(display.clone())),
                actual: None,
                message,
                log,
                timeout: None,
                aria_snapshot: None,
            })
        };
        if expected_path.extension().and_then(|e| e.to_str()) != Some("png") {
            return Err(Error::invalid_argument(format!(
                "Screenshot name \"{}\" must have a '.png' extension",
                expected_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            )));
        }
        let header = |timed_out: bool| {
            format_matcher_message(MessageDetails {
                is_not: cfg.is_not,
                matcher_name: "toHaveScreenshot".to_string(),
                expectation: "expected".to_string(),
                locator: locator.clone(),
                timeout: Some(timeout),
                timed_out,
                ..Default::default()
            })
        };
        let expected_bytes = std::fs::read(&expected_path).ok();
        if cfg.is_not {
            let Some(bytes) = expected_bytes else {
                return Err(result(
                    format!(
                        "A snapshot doesn't exist at {display}, matchers using \".not\" won't write them automatically."
                    ),
                    true,
                    Vec::new(),
                ));
            };
            let expected = Image::decode(&bytes)?;
            let outcome = self.expect_screenshot(Some(&expected), timeout).await;
            if let Some(e) = outcome.error {
                return Err(e);
            }
            if outcome.error_message.is_some() {
                return Ok(());
            }
            let message = "Screenshot comparison failed:\n\n  Expected result should be different from the actual one.".to_string();
            return Err(result(message, true, outcome.log));
        }
        let expected = match &expected_bytes {
            Some(bytes) => Some(Image::decode(bytes)?),
            None => None,
        };
        let outcome = self.expect_screenshot(expected.as_ref(), timeout).await;
        if let Some(e) = outcome.error {
            return Err(e);
        }
        let Some(error_message) = outcome.error_message.clone() else {
            if expected.is_none() {
                if let Some(actual) = &outcome.actual {
                    write_file(&expected_path, actual)?;
                    write_file(&suffixed(&expected_path, "-actual"), actual)?;
                }
                return Err(result(
                    format!("A snapshot doesn't exist at {display}, writing actual."),
                    false,
                    Vec::new(),
                ));
            }
            return Ok(());
        };
        // handleDifferent
        let mut output = format!("{}{}", header(outcome.timed_out), indent(&error_message));
        if let Some(actual) = &outcome.actual {
            let path = suffixed(&expected_path, "-actual");
            write_file(&path, actual)?;
            output += &format!("\n\n  Expected: {display}\n  Received: {}", path.display());
        }
        if let Some(previous) = &outcome.previous {
            write_file(&suffixed(&expected_path, "-previous"), previous)?;
        }
        if let Some(diff) = &outcome.diff {
            let path = suffixed(&expected_path, "-diff");
            write_file(&path, diff)?;
            output += &format!("\n  Diff:     {}", path.display());
        }
        output.push('\n');
        if !outcome.log.is_empty() {
            output += &call_log_text(&outcome.log);
        }
        let mut err = result(output, false, outcome.log);
        if let Error::Expect(r) = &mut err {
            r.timeout = outcome.timed_out.then_some(timeout);
        }
        Err(err)
    }
}

impl IntoFuture for ToHaveScreenshot {
    type Output = Result<()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Result<()>> + Send>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

fn indent(text: &str) -> String {
    text.split('\n')
        .map(|l| format!("  {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    path.with_file_name(format!("{stem}{suffix}.png"))
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| Error::invalid_argument(format!("{}: {e}", dir.display())))?;
    }
    std::fs::write(path, bytes)
        .map_err(|e| Error::invalid_argument(format!("{}: {e}", path.display())))
}

#[derive(Default)]
struct Outcome {
    actual: Option<Vec<u8>>,
    previous: Option<Vec<u8>>,
    diff: Option<Vec<u8>>,
    error_message: Option<String>,
    timed_out: bool,
    log: Vec<String>,
    error: Option<Error>,
}

impl Outcome {
    fn error(e: Error) -> Self {
        Outcome {
            error: Some(e),
            ..Default::default()
        }
    }

    fn failed(
        log: Vec<String>,
        message: &str,
        comparison: Option<Comparison>,
        timed_out: bool,
    ) -> Self {
        let (actual, previous, diff) = match comparison {
            Some(c) => (c.actual, c.previous, Some(c.diff)),
            None => (None, None, None),
        };
        Outcome {
            actual,
            previous,
            diff,
            error_message: Some(message.to_string()),
            timed_out,
            log: compress_call_log(&log),
            error: None,
        }
    }
}

// ----------------------------------------------------------------------
// Images and pixelmatch
// ----------------------------------------------------------------------

/// An RGBA image.
#[derive(Debug, Clone)]
pub(crate) struct Image {
    width: usize,
    height: usize,
    data: Vec<u8>,
}

impl Image {
    pub(crate) fn decode(bytes: &[u8]) -> Result<Image> {
        const MAGIC: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
        if bytes.len() < 8 || bytes[..8] != MAGIC {
            return Err(Error::invalid_argument(
                "Could not decode expected image as PNG.",
            ));
        }
        let mut decoder = png::Decoder::new(bytes);
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder
            .read_info()
            .map_err(|e| Error::invalid_argument(format!("Could not decode image: {e}")))?;
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader
            .next_frame(&mut buf)
            .map_err(|e| Error::invalid_argument(format!("Could not decode image: {e}")))?;
        let (width, height) = (info.width as usize, info.height as usize);
        let pixels = &buf[..info.buffer_size()];
        let data = match info.color_type {
            png::ColorType::Rgba => pixels.to_vec(),
            png::ColorType::Rgb => pixels
                .chunks(3)
                .flat_map(|p| [p[0], p[1], p[2], 255])
                .collect(),
            png::ColorType::GrayscaleAlpha => pixels
                .chunks(2)
                .flat_map(|p| [p[0], p[0], p[0], p[1]])
                .collect(),
            png::ColorType::Grayscale => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            png::ColorType::Indexed => {
                return Err(Error::invalid_argument("Could not decode indexed PNG"));
            }
        };
        Ok(Image {
            width,
            height,
            data,
        })
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, self.width as u32, self.height as u32);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            if let Ok(mut writer) = encoder.write_header() {
                let _ = writer.write_image_data(&self.data);
            }
        }
        out
    }

    /// `padImageToSize`.
    fn padded(&self, width: usize, height: usize) -> Image {
        if self.width == width && self.height == height {
            return self.clone();
        }
        let mut data = vec![0u8; width * height * 4];
        for y in 0..self.height.min(height) {
            for x in 0..self.width.min(width) {
                let to = (y * width + x) * 4;
                let from = (y * self.width + x) * 4;
                data[to..to + 4].copy_from_slice(&self.data[from..from + 4]);
            }
        }
        Image {
            width,
            height,
            data,
        }
    }
}

struct Comparison {
    message: String,
    diff: Vec<u8>,
    actual: Option<Vec<u8>>,
    previous: Option<Vec<u8>>,
}

/// `compareImages` with the `pixelmatch` comparator: `None` when equal.
fn compare(actual: &Image, expected: &Image, options: &ToHaveScreenshot) -> Option<Comparison> {
    let (width, height) = (
        expected.width.max(actual.width),
        expected.height.max(actual.height),
    );
    let mut sizes = String::new();
    let (a, e);
    let (actual, expected) = if expected.width != actual.width || expected.height != actual.height {
        sizes = format!(
            "Expected an image {}px by {}px, received {}px by {}px. ",
            expected.width, expected.height, actual.width, actual.height
        );
        a = actual.padded(width, height);
        e = expected.padded(width, height);
        (&a, &e)
    } else {
        (actual, expected)
    };
    let mut diff = vec![0u8; width * height * 4];
    let count = pixelmatch(
        &expected.data,
        &actual.data,
        &mut diff,
        width,
        height,
        options.threshold,
    );
    let total = (expected.width * expected.height).max(1) as f64;
    let max1 = options.max_diff_pixels.map(|p| p as f64);
    let max2 = options.max_diff_pixel_ratio.map(|r| total * r);
    let max = match (max1, max2) {
        (Some(a), Some(b)) => a.min(b),
        (a, b) => a.or(b).unwrap_or(0.0),
    };
    let ratio = (count as f64 / total * 100.0).ceil() / 100.0;
    let pixels = if count as f64 > max {
        format!("{count} pixels (ratio {ratio:.2} of all image pixels) are different.")
    } else {
        String::new()
    };
    if pixels.is_empty() && sizes.is_empty() {
        return None;
    }
    Some(Comparison {
        message: format!("{sizes}{pixels}"),
        diff: Image {
            width,
            height,
            data: diff,
        }
        .encode(),
        actual: None,
        previous: None,
    })
}

/// Port of `pixelmatch` (v6) with its default options (`includeAA: false`,
/// `alpha: 0.1`, red diff color, yellow anti-aliasing color).
pub(crate) fn pixelmatch(
    img1: &[u8],
    img2: &[u8],
    output: &mut [u8],
    width: usize,
    height: usize,
    threshold: f64,
) -> usize {
    let len = width * height;
    if img1[..len * 4] == img2[..len * 4] {
        for i in 0..len {
            draw_gray_pixel(img1, i * 4, 0.1, output);
        }
        return 0;
    }
    let max_delta = 35215.0 * threshold * threshold;
    let mut diff = 0;
    for y in 0..height {
        for x in 0..width {
            let pos = (y * width + x) * 4;
            let delta = color_delta(img1, img2, pos, pos, false);
            if delta.abs() > max_delta {
                if antialiased(img1, x, y, width, height, img2)
                    || antialiased(img2, x, y, width, height, img1)
                {
                    draw_pixel(output, pos, 255.0, 255.0, 0.0);
                } else {
                    draw_pixel(output, pos, 255.0, 0.0, 0.0);
                    diff += 1;
                }
            } else {
                draw_gray_pixel(img1, pos, 0.1, output);
            }
        }
    }
    diff
}

fn antialiased(img: &[u8], x1: usize, y1: usize, width: usize, height: usize, img2: &[u8]) -> bool {
    let x0 = x1.saturating_sub(1);
    let y0 = y1.saturating_sub(1);
    let x2 = (x1 + 1).min(width - 1);
    let y2 = (y1 + 1).min(height - 1);
    let pos = (y1 * width + x1) * 4;
    let mut zeroes = if x1 == x0 || x1 == x2 || y1 == y0 || y1 == y2 {
        1
    } else {
        0
    };
    let (mut min, mut max) = (0.0f64, 0.0f64);
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (0, 0, 0, 0);
    for x in x0..=x2 {
        for y in y0..=y2 {
            if x == x1 && y == y1 {
                continue;
            }
            let delta = color_delta(img, img, pos, (y * width + x) * 4, true);
            if delta == 0.0 {
                zeroes += 1;
                if zeroes > 2 {
                    return false;
                }
            } else if delta < min {
                min = delta;
                min_x = x;
                min_y = y;
            } else if delta > max {
                max = delta;
                max_x = x;
                max_y = y;
            }
        }
    }
    if min == 0.0 || max == 0.0 {
        return false;
    }
    (has_many_siblings(img, min_x, min_y, width, height)
        && has_many_siblings(img2, min_x, min_y, width, height))
        || (has_many_siblings(img, max_x, max_y, width, height)
            && has_many_siblings(img2, max_x, max_y, width, height))
}

fn has_many_siblings(img: &[u8], x1: usize, y1: usize, width: usize, height: usize) -> bool {
    let x0 = x1.saturating_sub(1);
    let y0 = y1.saturating_sub(1);
    let x2 = (x1 + 1).min(width - 1);
    let y2 = (y1 + 1).min(height - 1);
    let pos = (y1 * width + x1) * 4;
    let mut zeroes = if x1 == x0 || x1 == x2 || y1 == y0 || y1 == y2 {
        1
    } else {
        0
    };
    for x in x0..=x2 {
        for y in y0..=y2 {
            if x == x1 && y == y1 {
                continue;
            }
            let pos2 = (y * width + x) * 4;
            if img[pos..pos + 4] == img[pos2..pos2 + 4] {
                zeroes += 1;
            }
            if zeroes > 2 {
                return true;
            }
        }
    }
    false
}

fn color_delta(img1: &[u8], img2: &[u8], k: usize, m: usize, y_only: bool) -> f64 {
    let (mut r1, mut g1, mut b1, a1) = (
        img1[k] as f64,
        img1[k + 1] as f64,
        img1[k + 2] as f64,
        img1[k + 3] as f64,
    );
    let (mut r2, mut g2, mut b2, a2) = (
        img2[m] as f64,
        img2[m + 1] as f64,
        img2[m + 2] as f64,
        img2[m + 3] as f64,
    );
    if a1 == a2 && r1 == r2 && g1 == g2 && b1 == b2 {
        return 0.0;
    }
    if a1 < 255.0 {
        let a = a1 / 255.0;
        r1 = blend(r1, a);
        g1 = blend(g1, a);
        b1 = blend(b1, a);
    }
    if a2 < 255.0 {
        let a = a2 / 255.0;
        r2 = blend(r2, a);
        g2 = blend(g2, a);
        b2 = blend(b2, a);
    }
    let y1 = rgb2y(r1, g1, b1);
    let y2 = rgb2y(r2, g2, b2);
    let y = y1 - y2;
    if y_only {
        return y;
    }
    let i = rgb2i(r1, g1, b1) - rgb2i(r2, g2, b2);
    let q = rgb2q(r1, g1, b1) - rgb2q(r2, g2, b2);
    let delta = 0.5053 * y * y + 0.299 * i * i + 0.1957 * q * q;
    if y1 > y2 { -delta } else { delta }
}

fn rgb2y(r: f64, g: f64, b: f64) -> f64 {
    r * 0.29889531 + g * 0.58662247 + b * 0.11448223
}
fn rgb2i(r: f64, g: f64, b: f64) -> f64 {
    r * 0.59597799 - g * 0.2741761 - b * 0.32180189
}
fn rgb2q(r: f64, g: f64, b: f64) -> f64 {
    r * 0.21147017 - g * 0.52261711 + b * 0.31114694
}
fn blend(c: f64, a: f64) -> f64 {
    255.0 + (c - 255.0) * a
}

fn draw_pixel(output: &mut [u8], pos: usize, r: f64, g: f64, b: f64) {
    output[pos] = r as u8;
    output[pos + 1] = g as u8;
    output[pos + 2] = b as u8;
    output[pos + 3] = 255;
}

fn draw_gray_pixel(img: &[u8], i: usize, alpha: f64, output: &mut [u8]) {
    let val = blend(
        rgb2y(img[i] as f64, img[i + 1] as f64, img[i + 2] as f64),
        alpha * img[i + 3] as f64 / 255.0,
    );
    draw_pixel(output, i, val, val, val);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: usize, height: usize, rgba: [u8; 4]) -> Image {
        Image {
            width,
            height,
            data: rgba.repeat(width * height),
        }
    }

    #[test]
    fn pixelmatch_counts_different_pixels() {
        let a = solid(4, 4, [255, 255, 255, 255]);
        let mut b = a.clone();
        b.data[0..4].copy_from_slice(&[0, 0, 0, 255]);
        let mut out = vec![0; 64];
        assert_eq!(pixelmatch(&a.data, &a.data, &mut out, 4, 4, 0.2), 0);
        assert_eq!(pixelmatch(&a.data, &b.data, &mut out, 4, 4, 0.2), 1);
        assert_eq!(&out[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn png_round_trip() {
        let a = solid(3, 2, [10, 20, 30, 255]);
        let decoded = Image::decode(&a.encode()).unwrap();
        assert_eq!(decoded.width, 3);
        assert_eq!(decoded.data, a.data);
    }
}
