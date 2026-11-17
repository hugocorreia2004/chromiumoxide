//! Storage state: cookies plus `localStorage` (and optionally IndexedDB) per
//! origin, saved with [`BrowserContext::storage_state`] and restored with the
//! `storage_state` context option or [`BrowserContext::set_storage_state`]
//! (Playwright's `BrowserContext.storageState/setStorageState`).

use std::future::{Future, IntoFuture};
use std::path::{Path, PathBuf};
use std::pin::Pin;

use chromiumoxide::cdp::browser_protocol::fetch::EventRequestPaused;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::browser_context::BrowserContext;
use crate::cdp_raw::RawCommand;
use crate::cookies::{Cookie, SetCookie};
use crate::error::{Error, Result, ResultExt};
use crate::execution_context::{Evaluated, World};
use crate::ext::http_origin;
use crate::js::{JsValue, sources};
use crate::page::Page;

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// A `name`/`value` pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameValue {
    /// Name.
    pub name: String,
    /// Value.
    pub value: String,
}

/// Storage of one origin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OriginState {
    /// The origin, e.g. `https://example.com`.
    pub origin: String,
    /// `localStorage` entries.
    #[serde(default)]
    pub local_storage: Vec<NameValue>,
    /// IndexedDB databases (Playwright's serialized format), when requested.
    #[serde(rename = "indexedDB", default, skip_serializing_if = "Option::is_none")]
    pub indexed_db: Option<Vec<Value>>,
    /// Origin private file system entries, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opfs: Option<Vec<Value>>,
}

/// Cookies and per-origin storage of a context; serializes to the same JSON
/// as Playwright's storage state files.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StorageState {
    /// Cookies.
    #[serde(default)]
    pub cookies: Vec<Cookie>,
    /// Per-origin storage.
    #[serde(default)]
    pub origins: Vec<OriginState>,
}

impl StorageState {
    /// Read a storage state file.
    pub async fn from_path(path: impl AsRef<Path>) -> Result<StorageState> {
        let path = path.as_ref();
        let text = tokio::fs::read_to_string(path).await.map_err(|e| {
            Error::invalid_argument(format!(
                "Error reading storage state from {}:\n{e}",
                path.display()
            ))
        })?;
        serde_json::from_str(&text).map_err(|e| {
            Error::invalid_argument(format!(
                "Error reading storage state from {}:\n{e}",
                path.display()
            ))
        })
    }
}

/// Where a storage state comes from: a value or a JSON file.
#[derive(Debug, Clone)]
pub enum StorageStateSource {
    /// An in-memory state.
    State(StorageState),
    /// A JSON file written by `storage_state().path(..)` (or Playwright).
    Path(PathBuf),
}

impl StorageStateSource {
    pub(crate) async fn load(&self) -> Result<StorageState> {
        match self {
            StorageStateSource::State(state) => Ok(state.clone()),
            StorageStateSource::Path(path) => StorageState::from_path(path).await,
        }
    }
}

impl From<StorageState> for StorageStateSource {
    fn from(state: StorageState) -> Self {
        StorageStateSource::State(state)
    }
}

impl From<&StorageState> for StorageStateSource {
    fn from(state: &StorageState) -> Self {
        StorageStateSource::State(state.clone())
    }
}

impl From<PathBuf> for StorageStateSource {
    fn from(path: PathBuf) -> Self {
        StorageStateSource::Path(path)
    }
}

impl From<&Path> for StorageStateSource {
    fn from(path: &Path) -> Self {
        StorageStateSource::Path(path.to_path_buf())
    }
}

impl From<&str> for StorageStateSource {
    fn from(path: &str) -> Self {
        StorageStateSource::Path(PathBuf::from(path))
    }
}

impl From<String> for StorageStateSource {
    fn from(path: String) -> Self {
        StorageStateSource::Path(PathBuf::from(path))
    }
}

fn storage_script(call: &str) -> String {
    format!(
        "(() => {{\nconst module = {{}};\n{}\nconst script = new (module.exports.StorageScript())(\"chromium\");\nreturn {call};\n}})()",
        sources::STORAGE_SCRIPT
    )
}

fn has_storage(origin: &OriginState) -> bool {
    !origin.local_storage.is_empty()
        || origin.indexed_db.as_ref().is_some_and(|v| !v.is_empty())
        || origin.opfs.as_ref().is_some_and(|v| !v.is_empty())
}

async fn evaluate_storage(page: &Page, expression: &str) -> Result<Value> {
    let frame = page.main_frame();
    match frame
        .evaluate_in(World::Utility, expression, None, true)
        .await?
    {
        Evaluated::Value(v) => Ok(js_value_to_json(v)),
        Evaluated::Handle(_) => Ok(Value::Null),
    }
}

fn js_value_to_json(value: JsValue) -> Value {
    crate::js::serde_impl::from_js_value::<Value>(value).unwrap_or(Value::Null)
}

fn parse_origin(origin: &str, storage: Value) -> Option<OriginState> {
    let mut object = match storage {
        Value::Object(map) => map,
        _ => return None,
    };
    object.insert("origin".into(), json!(origin));
    serde_json::from_value(Value::Object(object)).ok()
}

/// A page whose every request is answered with an empty document, used to
/// visit origins (`visitOrigins`). Closed when dropped... explicitly.
struct BlankOriginsPage {
    page: Page,
    task: tokio::task::JoinHandle<()>,
}

impl BlankOriginsPage {
    async fn open(context: &BrowserContext) -> Result<Self> {
        let page = context.new_page().await?;
        let mut paused = page
            .inner
            .cdp
            .event_listener::<EventRequestPaused>()
            .await?;
        let cdp = page.inner.cdp.clone();
        let task = tokio::spawn(async move {
            use base64::Engine;
            let body = base64::engine::general_purpose::STANDARD.encode("<html></html>");
            while let Some(event) = paused.next().await {
                let _ = cdp
                    .execute(RawCommand::new(
                        "Fetch.fulfillRequest",
                        json!({
                            "requestId": event.request_id.as_ref(),
                            "responseCode": 200,
                            "responseHeaders": [{ "name": "Content-Type", "value": "text/html" }],
                            "body": body,
                        }),
                    ))
                    .await;
            }
        });
        page.inner
            .cdp
            .execute(RawCommand::new(
                "Fetch.enable",
                json!({ "patterns": [{ "urlPattern": "*" }] }),
            ))
            .await?;
        Ok(Self { page, task })
    }

    async fn close(self) {
        self.task.abort();
        let _ = self.page.close().await;
    }
}

impl BrowserContext {
    /// Origins committed in this context (`BrowserContext._origins`).
    fn visited_origins(&self) -> Vec<String> {
        let mut origins: Vec<String> = self
            .inner
            .emu
            .origins
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect();
        origins.sort();
        origins
    }

    pub(crate) async fn collect_storage_state(
        &self,
        indexed_db: bool,
        opfs: bool,
    ) -> Result<StorageState> {
        let mut result = StorageState {
            cookies: self.get_cookies(&[]).await?,
            origins: Vec::new(),
        };
        let mut to_save = self.visited_origins();
        let collect = storage_script(&format!(
            "script.collect({{ indexedDB: {indexed_db}, opfs: {opfs} }})"
        ));
        // First try collecting from the existing pages.
        for page in self.pages() {
            let Some(origin) = http_origin(&page.url()) else {
                continue;
            };
            let Some(position) = to_save.iter().position(|o| *o == origin) else {
                continue;
            };
            if let Ok(storage) = evaluate_storage(&page, &collect).await {
                if let Some(state) = parse_origin(&origin, storage) {
                    if has_storage(&state) {
                        result.origins.push(state);
                    }
                    to_save.remove(position);
                }
            }
        }
        // Visit the remaining origins on a blank page.
        if !to_save.is_empty() {
            let helper = BlankOriginsPage::open(self).await?;
            let outcome: Result<()> = async {
                for origin in &to_save {
                    helper.page.goto(origin.clone()).await?;
                    let storage = evaluate_storage(&helper.page, &collect).await?;
                    if let Some(state) = parse_origin(origin, storage) {
                        if has_storage(&state) {
                            result.origins.push(state);
                        }
                    }
                }
                Ok(())
            }
            .await;
            helper.close().await;
            outcome?;
        }
        Ok(result)
    }

    /// The context's cookies and local storage (and IndexedDB with
    /// [`StorageStateBuilder::indexed_db`]); optionally saved to a file.
    ///
    /// ```no_run
    /// # async fn run(context: oxidewright::BrowserContext) -> oxidewright::Result<()> {
    /// let state = context.storage_state().path("state.json").await?;
    /// # let _ = state; Ok(()) }
    /// ```
    pub fn storage_state(&self) -> StorageStateBuilder {
        StorageStateBuilder {
            context: self.clone(),
            path: None,
            indexed_db: false,
            opfs: false,
        }
    }

    /// Replace the cookies and storage of the context with `state` (a
    /// [`StorageState`] or the path of a storage state file).
    pub async fn set_storage_state(&self, state: impl Into<StorageStateSource>) -> Result<()> {
        async {
            let state = state.into().load().await?;
            set_storage_state(self, &state, false).await
        }
        .await
        .api("browserContext.setStorageState")
    }
}

/// `BrowserContext.setStorageState` (`initial` = from the context options).
pub(crate) async fn set_storage_state(
    context: &BrowserContext,
    state: &StorageState,
    initial: bool,
) -> Result<()> {
    let outcome: Result<()> = async {
        if !initial {
            context.clear_all_cookies().await?;
        }
        if !state.cookies.is_empty() {
            context
                .set_cookies(state.cookies.iter().map(SetCookie::from).collect())
                .await?;
        }
        let mut origins = context.visited_origins();
        for origin in &state.origins {
            if !origins.contains(&origin.origin) {
                origins.push(origin.origin.clone());
            }
        }
        if !origins.is_empty() {
            let helper = BlankOriginsPage::open(context).await?;
            let restored: Result<()> = async {
                for origin in &origins {
                    helper.page.goto(origin.clone()).await.map_err(|e| {
                        Error::navigation(format!("{} (origin: {origin})", e.message()))
                    })?;
                    let data = state
                        .origins
                        .iter()
                        .find(|o| &o.origin == origin)
                        .map(|o| serde_json::to_value(o).unwrap_or(Value::Null))
                        .unwrap_or(Value::Null);
                    let restore = storage_script(&format!("script.restore({data})"));
                    evaluate_storage(&helper.page, &restore).await?;
                }
                Ok(())
            }
            .await;
            helper.close().await;
            restored?;
        }
        let mut visited = context.inner.emu.origins.lock().unwrap();
        visited.clear();
        visited.extend(state.origins.iter().map(|o| o.origin.clone()));
        Ok(())
    }
    .await;
    outcome.map_err(|e| {
        let message = format!("Error setting storage state:\n{}", e.message());
        match e {
            Error::Timeout { .. } => e,
            _ => Error::invalid_argument(message),
        }
    })
}

/// Builder returned by [`BrowserContext::storage_state`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct StorageStateBuilder {
    context: BrowserContext,
    path: Option<PathBuf>,
    indexed_db: bool,
    opfs: bool,
}

impl StorageStateBuilder {
    /// Include the origin private file system (OPFS) of each origin.
    pub fn opfs(mut self, opfs: bool) -> Self {
        self.opfs = opfs;
        self
    }

    /// Also write the state as JSON to this file.
    pub fn path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Include IndexedDB databases.
    pub fn indexed_db(mut self, indexed_db: bool) -> Self {
        self.indexed_db = indexed_db;
        self
    }
}

impl IntoFuture for StorageStateBuilder {
    type Output = Result<StorageState>;
    type IntoFuture = BoxFuture<Result<StorageState>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            async {
                let state = self
                    .context
                    .collect_storage_state(self.indexed_db, self.opfs)
                    .await?;
                if let Some(path) = &self.path {
                    write_json(path, &state).await?;
                }
                Ok(state)
            }
            .await
            .api("browserContext.storageState")
        })
    }
}

/// Write `value` as pretty JSON, creating parent directories.
pub(crate) async fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let text = serde_json::to_string_pretty(value)?;
    write_file(path, text.as_bytes()).await
}

/// Write a file, creating its parent directories (`mkdirIfNeeded`).
pub(crate) async fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| Error::invalid_argument(format!("{}: {e}", parent.display())))?;
        }
    }
    tokio::fs::write(path, bytes)
        .await
        .map_err(|e| Error::invalid_argument(format!("{}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_state_json_matches_playwright() {
        let json = r#"{
            "cookies": [{"name":"a","value":"b","domain":"example.com","path":"/","expires":-1,"httpOnly":false,"secure":false,"sameSite":"Lax"}],
            "origins": [{"origin":"https://example.com","localStorage":[{"name":"k","value":"v"}]}]
        }"#;
        let state: StorageState = serde_json::from_str(json).unwrap();
        assert_eq!(state.cookies[0].name, "a");
        assert_eq!(state.origins[0].local_storage[0].value, "v");
        let out = serde_json::to_value(&state).unwrap();
        assert_eq!(out["origins"][0]["localStorage"][0]["name"], "k");
        assert!(out["origins"][0].get("indexedDB").is_none());
    }
}
