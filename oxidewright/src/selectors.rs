//! Selector engine registry (`playwright.selectors`).

use std::path::PathBuf;
use std::sync::RwLock;

use crate::error::{Error, Result};

static TEST_ID_ATTRIBUTE: RwLock<Option<String>> = RwLock::new(None);
static ENGINES: RwLock<Vec<Engine>> = RwLock::new(Vec::new());

/// A custom selector engine registered with [`Selectors::register`].
#[derive(Debug, Clone)]
pub(crate) struct Engine {
    pub(crate) name: String,
    /// JavaScript evaluating to the engine object.
    pub(crate) source: String,
    /// Run in the isolated world instead of the page's main world.
    pub(crate) content_script: bool,
}

/// Built-in engines (keep in sync with the injected script), as in
/// Playwright's `server/selectors.ts`.
const BUILTIN_ENGINES: &[&str] = &[
    "css",
    "css:light",
    "xpath",
    "xpath:light",
    "_react",
    "_vue",
    "text",
    "text:light",
    "id",
    "id:light",
    "data-testid",
    "data-testid:light",
    "data-test-id",
    "data-test-id:light",
    "data-test",
    "data-test:light",
    "nth",
    "visible",
    "internal:control",
    "internal:has",
    "internal:has-not",
    "internal:has-text",
    "internal:has-not-text",
    "internal:and",
    "internal:or",
    "internal:chain",
    "role",
    "internal:attr",
    "internal:label",
    "internal:text",
    "internal:role",
    "internal:testid",
    "internal:describe",
    "aria-ref",
    "aria-template",
];

/// Built-in engines that only work in the page's main world.
const BUILTIN_ENGINES_IN_MAIN_WORLD: &[&str] = &["_react", "_vue"];

/// The attribute used by `get_by_test_id` (default `data-testid`).
pub(crate) fn test_id_attribute_name() -> String {
    TEST_ID_ATTRIBUTE
        .read()
        .unwrap()
        .clone()
        .unwrap_or_else(|| "data-testid".to_string())
}

/// The `customEngines` option of the injected script.
pub(crate) fn custom_engines_json() -> serde_json::Value {
    let engines = ENGINES.read().unwrap();
    serde_json::Value::Array(
        engines
            .iter()
            .map(|e| {
                // Like Playwright's `evaluationScript`: a function is called,
                // anything else is the engine object itself.
                let source = format!(
                    "(() => {{ const e = ({}\n); return typeof e === 'function' ? e() : e; }})()",
                    e.source
                );
                serde_json::json!({ "name": e.name, "source": format!("({source})") })
            })
            .collect(),
    )
}

/// Whether `selector` must run in the main world (`parseSelector`'s
/// `needsMainWorld`): a registered engine without `content_script`, or a
/// built-in main-world engine. Unparsable selectors report `false`: the
/// injected script then raises the parse error.
pub(crate) fn needs_main_world(selector: &str) -> bool {
    let Ok(parsed) = crate::selector_parser::parse_selector(selector) else {
        return false;
    };
    let engines = ENGINES.read().unwrap();
    let mut main = false;
    crate::selector_parser::visit_all_selector_parts(&parsed, &mut |part, _| {
        if BUILTIN_ENGINES_IN_MAIN_WORLD.contains(&part.name.as_str()) {
            main = true;
        } else if let Some(engine) = engines.iter().find(|e| e.name == part.name) {
            if !engine.content_script {
                main = true;
            }
        }
    });
    main
}

/// Global selector settings, like Playwright's `playwright.selectors`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Selectors;

/// The global [`Selectors`] instance.
pub fn selectors() -> Selectors {
    Selectors
}

impl Selectors {
    /// Change the attribute used by `get_by_test_id` (default `data-testid`).
    pub fn set_test_id_attribute(&self, attribute_name: impl Into<String>) {
        *TEST_ID_ATTRIBUTE.write().unwrap() = Some(attribute_name.into());
    }

    /// Register a custom selector engine, used as `name=body` in selectors.
    ///
    /// `script` is JavaScript evaluating to an object with
    /// `query(root, selector)` and `queryAll(root, selector)`, or to a
    /// function returning one. Engines run in the page's main world unless
    /// [`RegisterSelector::content_script`] is set (main-world code is visible
    /// to the page). Applies to pages created afterwards.
    ///
    /// ```no_run
    /// # async fn run() -> oxidewright::Result<()> {
    /// oxidewright::selectors()
    ///     .register(
    ///         "tag",
    ///         "() => ({ query: (root, s) => root.querySelector(s), queryAll: (root, s) => Array.from(root.querySelectorAll(s)) })",
    ///     )
    ///     .content_script(true)
    ///     .await?;
    /// # Ok(()) }
    /// ```
    pub fn register(&self, name: impl Into<String>, script: impl Into<String>) -> RegisterSelector {
        RegisterSelector {
            name: name.into(),
            source: Source::Script(script.into()),
            content_script: false,
        }
    }

    /// Like [`Selectors::register`], reading the engine script from a file.
    pub fn register_path(
        &self,
        name: impl Into<String>,
        path: impl Into<PathBuf>,
    ) -> RegisterSelector {
        RegisterSelector {
            name: name.into(),
            source: Source::Path(path.into()),
            content_script: false,
        }
    }
}

enum Source {
    Script(String),
    Path(PathBuf),
}

/// Builder returned by [`Selectors::register`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct RegisterSelector {
    name: String,
    source: Source,
    content_script: bool,
}

impl RegisterSelector {
    /// Run the engine in the isolated world, invisible to the page (default
    /// `false`: the main world, where page globals are reachable).
    pub fn content_script(mut self, content_script: bool) -> Self {
        self.content_script = content_script;
        self
    }
}

impl std::future::IntoFuture for RegisterSelector {
    type Output = Result<()>;
    type IntoFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let error = |m: String| Error::invalid_argument(format!("selectors.register: {m}"));
            let source = match self.source {
                Source::Script(s) => s,
                Source::Path(path) => tokio::fs::read_to_string(&path)
                    .await
                    .map_err(|e| error(format!("{}: {e}", path.display())))?,
            };
            if self.name.is_empty()
                || !self
                    .name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Err(error(
                    "Selector engine name may only contain [a-zA-Z0-9_] characters".into(),
                ));
            }
            if BUILTIN_ENGINES.contains(&self.name.as_str())
                || self.name == "zs"
                || self.name == "zs:light"
            {
                return Err(error(format!(
                    "\"{}\" is a predefined selector engine",
                    self.name
                )));
            }
            let mut engines = ENGINES.write().unwrap();
            if engines.iter().any(|e| e.name == self.name) {
                return Err(error(format!(
                    "\"{}\" selector engine has been already registered",
                    self.name
                )));
            }
            engines.push(Engine {
                name: self.name,
                source,
                content_script: self.content_script,
            });
            Ok(())
        })
    }
}
