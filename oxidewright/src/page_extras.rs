//! Smaller page/context APIs: `add_script_tag`, `add_style_tag`, `opener`,
//! `close` with `run_before_unload`/`reason`.

use std::future::{Future, IntoFuture};
use std::path::PathBuf;
use std::pin::Pin;

use serde_json::{Value, json};

use crate::browser_context::BrowserContext;
use crate::cdp_raw::RawCommand;
use crate::error::{Error, Result, ResultExt};
use crate::frame::Frame;
use crate::js_handle::ElementHandle;
use crate::page::Page;

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

const ADD_SCRIPT_URL: &str = "async (params) => {
  const script = document.createElement('script');
  script.src = params.url;
  if (params.type)
    script.type = params.type;
  const promise = new Promise((res, rej) => {
    script.onload = res;
    script.onerror = e => rej(typeof e === 'string' ? new Error(e) : new Error(`Failed to load script at ${script.src}`));
  });
  document.head.appendChild(script);
  await promise;
  return script;
}";

const ADD_SCRIPT_CONTENT: &str = "(params) => {
  const script = document.createElement('script');
  script.type = params.type || 'text/javascript';
  script.text = params.content;
  let error = null;
  script.onerror = e => error = e;
  document.head.appendChild(script);
  if (error)
    throw error;
  return script;
}";

const ADD_STYLE_URL: &str = "async (url) => {
  const link = document.createElement('link');
  link.rel = 'stylesheet';
  link.href = url;
  const promise = new Promise((res, rej) => {
    link.onload = res;
    link.onerror = rej;
  });
  document.head.appendChild(link);
  await promise;
  return link;
}";

const ADD_STYLE_CONTENT: &str = "async (content) => {
  const style = document.createElement('style');
  style.type = 'text/css';
  style.appendChild(document.createTextNode(content));
  const promise = new Promise((res, rej) => {
    style.onload = res;
    style.onerror = rej;
  });
  document.head.appendChild(style);
  await promise;
  return style;
}";

/// Which kind of tag [`AddTag`] inserts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TagKind {
    Script,
    Style,
}

/// Builder returned by `add_script_tag` / `add_style_tag`; set one of
/// `url`, `path` or `content`, then `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct AddTag {
    frame: Frame,
    kind: TagKind,
    api: &'static str,
    url: Option<String>,
    path: Option<PathBuf>,
    content: Option<String>,
    script_type: Option<String>,
}

impl AddTag {
    /// URL of the script or stylesheet.
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    /// Local file whose content is injected.
    pub fn path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Raw content to inject.
    pub fn content(mut self, content: impl Into<String>) -> Self {
        self.content = Some(content.into());
        self
    }

    /// Script type (`"module"` for ES modules); scripts only.
    pub fn r#type(mut self, script_type: impl Into<String>) -> Self {
        self.script_type = Some(script_type.into());
        self
    }
}

impl IntoFuture for AddTag {
    type Output = Result<ElementHandle>;
    type IntoFuture = BoxFuture<Result<ElementHandle>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let api = self.api;
            async move {
                let mut content = self.content;
                if let Some(path) = &self.path {
                    let text = tokio::fs::read_to_string(path).await.map_err(|e| {
                        Error::invalid_argument(format!("{}: {e}", path.display()))
                    })?;
                    let source_url = path.display().to_string().replace('\n', "");
                    content = Some(match self.kind {
                        TagKind::Script => format!("{text}\n//# sourceURL={source_url}"),
                        TagKind::Style => format!("{text}/*# sourceURL={source_url}*/"),
                    });
                }
                if self.url.is_none() && content.is_none() {
                    return Err(Error::invalid_argument(
                        "Provide an object with a `url`, `path` or `content` property",
                    ));
                }
                let (function, arg): (&str, Value) = match (self.kind, &self.url) {
                    (TagKind::Script, Some(url)) => (
                        ADD_SCRIPT_URL,
                        json!({ "url": url, "type": self.script_type.clone().unwrap_or_default() }),
                    ),
                    (TagKind::Script, None) => (
                        ADD_SCRIPT_CONTENT,
                        json!({ "content": content, "type": self.script_type.clone().unwrap_or_default() }),
                    ),
                    (TagKind::Style, Some(url)) => (ADD_STYLE_URL, json!(url)),
                    (TagKind::Style, None) => (ADD_STYLE_CONTENT, json!(content)),
                };
                let handle = self.frame.evaluate_handle(function).arg(arg).await?;
                handle
                    .as_element()
                    .ok_or_else(|| Error::evaluation("Failed to insert the tag"))
            }
            .await
            .map_err(|e| e.with_api(api))
        })
    }
}

impl Frame {
    /// Insert a `<script>` tag (from `url`, `path` or `content`) and wait
    /// for it to load. Note: Content-Security-Policy violations are not
    /// detected (they are only reported on the console, which oxidewright
    /// does not enable).
    pub fn add_script_tag(&self) -> AddTag {
        AddTag {
            frame: self.clone(),
            kind: TagKind::Script,
            api: "frame.addScriptTag",
            url: None,
            path: None,
            content: None,
            script_type: None,
        }
    }

    /// Insert a `<link rel="stylesheet">` (`url`) or `<style>` (`path`,
    /// `content`) tag and wait for it to load.
    pub fn add_style_tag(&self) -> AddTag {
        AddTag {
            frame: self.clone(),
            kind: TagKind::Style,
            api: "frame.addStyleTag",
            url: None,
            path: None,
            content: None,
            script_type: None,
        }
    }
}

impl Page {
    /// See [`Frame::add_script_tag`]; runs in the main frame.
    pub fn add_script_tag(&self) -> AddTag {
        let mut tag = self.main_frame().add_script_tag();
        tag.api = "page.addScriptTag";
        tag
    }

    /// See [`Frame::add_style_tag`]; runs in the main frame.
    pub fn add_style_tag(&self) -> AddTag {
        let mut tag = self.main_frame().add_style_tag();
        tag.api = "page.addStyleTag";
        tag
    }

    /// The page that opened this one (popups), if it is still open.
    pub async fn opener(&self) -> Result<Option<Page>> {
        async {
            let browser = self
                .inner
                .browser
                .upgrade()
                .ok_or_else(Error::target_closed)?;
            let info = browser
                .execute_with_session(
                    RawCommand::new(
                        "Target.getTargetInfo",
                        json!({ "targetId": self.inner.cdp.target_id().as_ref() }),
                    ),
                    None,
                )
                .await?;
            let Some(opener) = info
                .get("targetInfo")
                .and_then(|t| t.get("openerId"))
                .and_then(Value::as_str)
            else {
                return Ok(None);
            };
            Ok(browser
                .page_by_target_id(opener)
                .filter(|page| !page.is_closed()))
        }
        .await
        .api("page.opener")
    }
}

/// Builder returned by [`Page::close`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct ClosePage {
    pub(crate) page: Page,
    pub(crate) run_before_unload: bool,
    pub(crate) reason: Option<String>,
}

impl ClosePage {
    /// Run the page's `beforeunload` handlers (default `false`). The page
    /// may then stay open, e.g. when a handler shows a dialog; the call does
    /// not wait for the page to close.
    pub fn run_before_unload(mut self, run: bool) -> Self {
        self.run_before_unload = run;
        self
    }

    /// Reason reported to operations interrupted by the closure.
    pub fn reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}

impl IntoFuture for ClosePage {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            if self.run_before_unload {
                if self.page.is_closed() {
                    return Err(Error::target_closed().with_api("page.close"));
                }
                return self
                    .page
                    .inner
                    .cdp
                    .execute(RawCommand::new("Page.close", Value::Null))
                    .await
                    .map(|_| ())
                    .map_err(Error::from)
                    .api("page.close");
            }
            if let Some(reason) = &self.reason {
                *self.page.inner.ext.close_reason.lock().unwrap() = Some(reason.clone());
            }
            self.page.close_impl().await
        })
    }
}

/// Builder returned by [`BrowserContext::close`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct CloseContext {
    pub(crate) context: BrowserContext,
    pub(crate) reason: Option<String>,
}

impl CloseContext {
    /// Reason reported to operations interrupted by the closure.
    pub fn reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}

impl IntoFuture for CloseContext {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            if let Some(reason) = &self.reason {
                *self.context.inner.ext.close_reason.lock().unwrap() = Some(reason.clone());
            }
            self.context.close_impl().await
        })
    }
}

impl BrowserContext {
    /// The reason passed to `close().reason(..)`, if any.
    pub fn close_reason(&self) -> Option<String> {
        self.inner.ext.close_reason.lock().unwrap().clone()
    }
}

impl Page {
    /// The reason passed to `close().reason(..)` (of the page or its
    /// context), if any.
    pub fn close_reason(&self) -> Option<String> {
        self.inner
            .ext
            .close_reason
            .lock()
            .unwrap()
            .clone()
            .or_else(|| self.context().and_then(|c| c.close_reason()))
    }
}
