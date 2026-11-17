//! Smaller page APIs: `content({ includeShadow })`, `requestGC()` and the
//! `localStorage` / `sessionStorage` accessors ([`WebStorage`]).

use std::future::{Future, IntoFuture};
use std::pin::Pin;

use chromiumoxide::cdp::js_protocol::heap_profiler::CollectGarbageParams;
use serde_json::Value;

use crate::error::{Error, Result, ResultExt};
use crate::frame::Frame;
use crate::frame_selectors::{CallArg, call_injected, value_of};
use crate::page::Page;
use crate::storage_state::NameValue;

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Builder returned by `content()` of [`Page`] / [`Frame`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct Content {
    frame: Frame,
    include_shadow: bool,
    api: &'static str,
}

impl Content {
    pub(crate) fn new(frame: Frame, api: &'static str) -> Self {
        Content {
            frame,
            include_shadow: false,
            api,
        }
    }

    /// Also serialize open shadow roots, as declarative
    /// `<template shadowrootmode="open">` elements (closed ones never are).
    pub fn include_shadow(mut self, include_shadow: bool) -> Self {
        self.include_shadow = include_shadow;
        self
    }
}

impl IntoFuture for Content {
    type Output = Result<String>;
    type IntoFuture = BoxFuture<Result<String>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            async {
                let ctx = self.frame.utility_context().await?;
                let value = call_injected(
                    &ctx,
                    "(injected, includeShadow) => injected.documentContent(includeShadow)",
                    vec![CallArg::Json(Value::Bool(self.include_shadow))],
                    true,
                )
                .await?;
                Ok(value_of(value).as_str().unwrap_or_default().to_string())
            }
            .await
            .api(self.api)
        })
    }
}

/// `page.localStorage` / `page.sessionStorage`: the Web Storage of the main
/// frame's current origin. Calls run in an isolated world, so page scripts
/// do not observe them (patched `Storage` methods are not called).
#[derive(Clone, Debug)]
pub struct WebStorage {
    page: Page,
    kind: &'static str,
}

impl WebStorage {
    async fn eval<T: serde::de::DeserializeOwned>(
        &self,
        expression: String,
        api: &'static str,
    ) -> Result<T> {
        self.page
            .main_frame()
            .utility_evaluate(&expression)
            .await
            .map_err(|e| e.with_api(api))
    }

    fn storage(&self) -> String {
        format!("{}Storage", self.kind)
    }

    /// Every item, in key order.
    pub async fn items(&self) -> Result<Vec<NameValue>> {
        let storage = self.storage();
        self.eval(
            format!(
                "(() => {{ const result = []; for (let i = 0; i < {storage}.length; i++) {{ const name = {storage}.key(i); if (name !== null) result.push({{ name, value: {storage}.getItem(name) }}); }} return result; }})()"
            ),
            "webStorage.items",
        )
        .await
    }

    /// The value of `name`, `None` when absent.
    pub async fn get_item(&self, name: &str) -> Result<Option<String>> {
        self.eval(
            format!("{}.getItem({})", self.storage(), json_string(name)),
            "webStorage.getItem",
        )
        .await
    }

    /// Set `name` to `value`.
    pub async fn set_item(&self, name: &str, value: &str) -> Result<()> {
        let _: Value = self
            .eval(
                format!(
                    "{}.setItem({}, {})",
                    self.storage(),
                    json_string(name),
                    json_string(value)
                ),
                "webStorage.setItem",
            )
            .await?;
        Ok(())
    }

    /// Remove `name`.
    pub async fn remove_item(&self, name: &str) -> Result<()> {
        let _: Value = self
            .eval(
                format!("{}.removeItem({})", self.storage(), json_string(name)),
                "webStorage.removeItem",
            )
            .await?;
        Ok(())
    }

    /// Remove every item.
    pub async fn clear(&self) -> Result<()> {
        let _: Value = self
            .eval(format!("{}.clear()", self.storage()), "webStorage.clear")
            .await?;
        Ok(())
    }
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

impl Page {
    /// The `localStorage` of the main frame's current origin.
    pub fn local_storage(&self) -> WebStorage {
        WebStorage {
            page: self.clone(),
            kind: "local",
        }
    }

    /// The `sessionStorage` of the main frame's current origin.
    pub fn session_storage(&self) -> WebStorage {
        WebStorage {
            page: self.clone(),
            kind: "session",
        }
    }

    /// Ask the browser to run garbage collection
    /// (`HeapProfiler.collectGarbage`), e.g. to check for leaks with a
    /// `WeakRef`.
    pub async fn request_gc(&self) -> Result<()> {
        if self.is_closed() {
            return Err(Error::target_closed().with_api("page.requestGC"));
        }
        self.inner
            .cdp
            .execute(CollectGarbageParams::default())
            .await
            .map(|_| ())
            .map_err(|e| Error::from(e).with_api("page.requestGC"))
    }
}
