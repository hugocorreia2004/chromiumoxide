//! `page.pdf()` (Playwright's `crPdf.ts`): `Page.printToPDF` streamed back
//! with `IO.read`. Chromium only prints PDFs in headless mode.

use std::future::{Future, IntoFuture};
use std::path::PathBuf;
use std::pin::Pin;

use base64::Engine;
use serde_json::{Value, json};

use crate::cdp_raw::RawCommand;
use crate::error::{Error, Result, ResultExt};
use crate::page::Page;

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// A length for `page.pdf` options: pixels or a string with a unit
/// (`px`, `in`, `cm`, `mm`), e.g. `"10cm"`.
#[derive(Debug, Clone, PartialEq)]
pub enum PdfLength {
    /// CSS pixels.
    Pixels(f64),
    /// A value with a unit, e.g. `"1in"`.
    Text(String),
}

impl From<f64> for PdfLength {
    fn from(v: f64) -> Self {
        PdfLength::Pixels(v)
    }
}

impl From<u32> for PdfLength {
    fn from(v: u32) -> Self {
        PdfLength::Pixels(v as f64)
    }
}

impl From<i32> for PdfLength {
    fn from(v: i32) -> Self {
        PdfLength::Pixels(v as f64)
    }
}

impl From<&str> for PdfLength {
    fn from(v: &str) -> Self {
        PdfLength::Text(v.to_string())
    }
}

impl From<String> for PdfLength {
    fn from(v: String) -> Self {
        PdfLength::Text(v)
    }
}

impl PdfLength {
    fn to_text(&self) -> String {
        match self {
            PdfLength::Pixels(v) => format!("{v}px"),
            PdfLength::Text(t) => t.clone(),
        }
    }
}

/// Page margins of `page.pdf`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PdfMargin {
    /// Top margin.
    pub top: Option<PdfLength>,
    /// Right margin.
    pub right: Option<PdfLength>,
    /// Bottom margin.
    pub bottom: Option<PdfLength>,
    /// Left margin.
    pub left: Option<PdfLength>,
}

impl PdfMargin {
    /// The same margin on every side.
    pub fn all(value: impl Into<PdfLength>) -> Self {
        let value = value.into();
        Self {
            top: Some(value.clone()),
            right: Some(value.clone()),
            bottom: Some(value.clone()),
            left: Some(value),
        }
    }
}

/// `convertPrintParameterToInches`.
pub(crate) fn to_inches(text: Option<&str>) -> Result<Option<f64>> {
    let Some(text) = text else {
        return Ok(None);
    };
    let units = [("px", 1.0), ("in", 96.0), ("cm", 37.8), ("mm", 3.78)];
    let lower = text.to_lowercase();
    let (value_text, factor) = match units
        .iter()
        .find(|(unit, _)| text.len() >= 2 && lower.ends_with(unit))
    {
        Some((_, factor)) => (&text[..text.len() - 2], *factor),
        None => (text, 1.0),
    };
    let value: f64 = if value_text.trim().is_empty() {
        0.0
    } else {
        value_text.trim().parse().map_err(|_| {
            Error::invalid_argument(format!("Failed to parse parameter value: {text}"))
        })?
    };
    Ok(Some(value * factor / 96.0))
}

fn paper_format(format: &str) -> Option<(f64, f64)> {
    Some(match format.to_lowercase().as_str() {
        "letter" => (8.5, 11.0),
        "legal" => (8.5, 14.0),
        "tabloid" => (11.0, 17.0),
        "ledger" => (17.0, 11.0),
        "a0" => (33.1, 46.8),
        "a1" => (23.4, 33.1),
        "a2" => (16.54, 23.4),
        "a3" => (11.7, 16.54),
        "a4" => (8.27, 11.7),
        "a5" => (5.83, 8.27),
        "a6" => (4.13, 5.83),
        _ => return None,
    })
}

/// Read a protocol stream (`readProtocolStream`).
pub(crate) async fn read_stream(page: &Page, handle: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let chunk = page
            .inner
            .cdp
            .execute(RawCommand::new("IO.read", json!({ "handle": handle })))
            .await?
            .result;
        let data = chunk
            .get("data")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if chunk.get("base64Encoded").and_then(Value::as_bool) == Some(true) {
            out.extend(
                base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|e| Error::serialization(e.to_string()))?,
            );
        } else {
            out.extend_from_slice(data.as_bytes());
        }
        if chunk.get("eof").and_then(Value::as_bool) == Some(true) {
            break;
        }
    }
    let _ = page
        .inner
        .cdp
        .execute(RawCommand::new("IO.close", json!({ "handle": handle })))
        .await;
    Ok(out)
}

impl Page {
    /// Print the page to PDF (headless Chromium only); the `print` media
    /// type is used unless [`Page::emulate_media`] sets `screen`.
    ///
    /// ```no_run
    /// # async fn run(page: oxidewright::Page) -> oxidewright::Result<()> {
    /// let bytes = page.pdf().format("A4").print_background(true).await?;
    /// # let _ = bytes; Ok(()) }
    /// ```
    pub fn pdf(&self) -> Pdf {
        Pdf {
            page: self.clone(),
            path: None,
            scale: None,
            display_header_footer: false,
            header_template: None,
            footer_template: None,
            print_background: false,
            landscape: false,
            page_ranges: None,
            format: None,
            width: None,
            height: None,
            prefer_css_page_size: false,
            margin: PdfMargin::default(),
            tagged: false,
            outline: false,
        }
    }
}

/// Builder returned by [`Page::pdf`]; `.await` it for the PDF bytes.
#[must_use = "builders do nothing unless awaited"]
pub struct Pdf {
    page: Page,
    path: Option<PathBuf>,
    scale: Option<f64>,
    display_header_footer: bool,
    header_template: Option<String>,
    footer_template: Option<String>,
    print_background: bool,
    landscape: bool,
    page_ranges: Option<String>,
    format: Option<String>,
    width: Option<PdfLength>,
    height: Option<PdfLength>,
    prefer_css_page_size: bool,
    margin: PdfMargin,
    tagged: bool,
    outline: bool,
}

impl Pdf {
    /// Also save the PDF to this file.
    pub fn path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Rendering scale (default 1, between 0.1 and 2).
    pub fn scale(mut self, scale: f64) -> Self {
        self.scale = Some(scale);
        self
    }

    /// Display header and footer.
    pub fn display_header_footer(mut self, display: bool) -> Self {
        self.display_header_footer = display;
        self
    }

    /// HTML template of the header (classes `date`, `title`, `url`,
    /// `pageNumber`, `totalPages` are filled in).
    pub fn header_template(mut self, template: impl Into<String>) -> Self {
        self.header_template = Some(template.into());
        self
    }

    /// HTML template of the footer.
    pub fn footer_template(mut self, template: impl Into<String>) -> Self {
        self.footer_template = Some(template.into());
        self
    }

    /// Print background graphics.
    pub fn print_background(mut self, print: bool) -> Self {
        self.print_background = print;
        self
    }

    /// Landscape orientation.
    pub fn landscape(mut self, landscape: bool) -> Self {
        self.landscape = landscape;
        self
    }

    /// Pages to print, e.g. `"1-5, 8, 11-13"`.
    pub fn page_ranges(mut self, ranges: impl Into<String>) -> Self {
        self.page_ranges = Some(ranges.into());
        self
    }

    /// Paper format (`Letter`, `Legal`, `Tabloid`, `Ledger`, `A0`-`A6`);
    /// takes priority over width and height.
    pub fn format(mut self, format: impl Into<String>) -> Self {
        self.format = Some(format.into());
        self
    }

    /// Paper width.
    pub fn width(mut self, width: impl Into<PdfLength>) -> Self {
        self.width = Some(width.into());
        self
    }

    /// Paper height.
    pub fn height(mut self, height: impl Into<PdfLength>) -> Self {
        self.height = Some(height.into());
        self
    }

    /// Use the page's CSS `@page` size.
    pub fn prefer_css_page_size(mut self, prefer: bool) -> Self {
        self.prefer_css_page_size = prefer;
        self
    }

    /// Paper margins.
    pub fn margin(mut self, margin: PdfMargin) -> Self {
        self.margin = margin;
        self
    }

    /// Generate a tagged (accessible) PDF.
    pub fn tagged(mut self, tagged: bool) -> Self {
        self.tagged = tagged;
        self
    }

    /// Embed the document outline.
    pub fn outline(mut self, outline: bool) -> Self {
        self.outline = outline;
        self
    }

    fn params(&self) -> Result<Value> {
        let (mut paper_width, mut paper_height) = (8.5, 11.0);
        if let Some(format) = &self.format {
            let (w, h) = paper_format(format).ok_or_else(|| {
                Error::invalid_argument(format!("Unknown paper format: {format}"))
            })?;
            paper_width = w;
            paper_height = h;
        } else {
            let width = self.width.as_ref().map(PdfLength::to_text);
            let height = self.height.as_ref().map(PdfLength::to_text);
            if let Some(w) = to_inches(width.as_deref())?.filter(|v| *v != 0.0) {
                paper_width = w;
            }
            if let Some(h) = to_inches(height.as_deref())?.filter(|v| *v != 0.0) {
                paper_height = h;
            }
        }
        let margin = |m: &Option<PdfLength>| -> Result<f64> {
            let text = m.as_ref().map(PdfLength::to_text);
            Ok(to_inches(text.as_deref())?.unwrap_or(0.0))
        };
        Ok(json!({
            "transferMode": "ReturnAsStream",
            "landscape": self.landscape,
            "displayHeaderFooter": self.display_header_footer,
            "headerTemplate": self.header_template.clone().unwrap_or_default(),
            "footerTemplate": self.footer_template.clone().unwrap_or_default(),
            "printBackground": self.print_background,
            "scale": self.scale.unwrap_or(1.0),
            "paperWidth": paper_width,
            "paperHeight": paper_height,
            "marginTop": margin(&self.margin.top)?,
            "marginBottom": margin(&self.margin.bottom)?,
            "marginLeft": margin(&self.margin.left)?,
            "marginRight": margin(&self.margin.right)?,
            "pageRanges": self.page_ranges.clone().unwrap_or_default(),
            "preferCSSPageSize": self.prefer_css_page_size,
            "generateTaggedPDF": self.tagged,
            "generateDocumentOutline": self.outline,
        }))
    }
}

impl IntoFuture for Pdf {
    type Output = Result<Vec<u8>>;
    type IntoFuture = BoxFuture<Result<Vec<u8>>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            async {
                let params = self.params()?;
                let result = self
                    .page
                    .inner
                    .cdp
                    .execute(RawCommand::new("Page.printToPDF", params))
                    .await?
                    .result;
                let bytes = match result.get("stream").and_then(Value::as_str) {
                    Some(stream) => read_stream(&self.page, stream).await?,
                    None => base64::engine::general_purpose::STANDARD
                        .decode(
                            result
                                .get("data")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                        )
                        .map_err(|e| Error::serialization(e.to_string()))?,
                };
                if let Some(path) = &self.path {
                    crate::storage_state::write_file(path, &bytes).await?;
                }
                Ok(bytes)
            }
            .await
            .api("page.pdf")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_units_like_playwright() {
        assert_eq!(to_inches(Some("96px")).unwrap(), Some(1.0));
        assert_eq!(to_inches(Some("1in")).unwrap(), Some(1.0));
        assert_eq!(to_inches(Some("96")).unwrap(), Some(1.0));
        assert!((to_inches(Some("2.54cm")).unwrap().unwrap() - 1.000125).abs() < 1e-6);
        assert!(to_inches(Some("abcpx")).is_err());
        assert_eq!(to_inches(None).unwrap(), None);
        assert_eq!(paper_format("A4"), Some((8.27, 11.7)));
    }
}
