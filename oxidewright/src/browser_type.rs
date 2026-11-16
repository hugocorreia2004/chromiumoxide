//! [`BrowserType`]: launching and connecting to Chromium.

use std::future::{Future, IntoFuture};
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

use chromiumoxide::BrowserConfig;
use chromiumoxide::browser::HeadlessMode;

use crate::browser::{Browser, BrowserInner};
use crate::error::{Error, Result, ResultExt};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Entry point to launch or connect to a browser of a given type.
#[derive(Debug, Clone, Copy)]
pub struct BrowserType {
    name: &'static str,
}

/// The Chromium [`BrowserType`].
pub fn chromium() -> BrowserType {
    BrowserType { name: "chromium" }
}

impl BrowserType {
    /// `"chromium"`.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Launch a new browser process.
    ///
    /// ```no_run
    /// # async fn run() -> oxidewright::Result<()> {
    /// let browser = oxidewright::chromium().launch().headless(true).await?;
    /// # Ok(()) }
    /// ```
    pub fn launch(&self) -> Launch {
        Launch {
            options: LaunchOptions::default(),
        }
    }

    /// Attach to an existing browser through the Chrome DevTools Protocol.
    ///
    /// `endpoint` is either the browser websocket URL or an `http://host:port`
    /// endpoint (its `/json/version` is queried).
    pub fn connect_over_cdp(&self, endpoint: impl Into<String>) -> ConnectOverCdp {
        ConnectOverCdp {
            endpoint: endpoint.into(),
            timeout: None,
        }
    }
}

/// Options of [`BrowserType::launch`].
#[derive(Debug, Clone)]
pub struct LaunchOptions {
    /// Run without a visible window (default `true`).
    pub headless: bool,
    /// Path of the Chromium/Chrome executable; detected when `None`.
    pub executable_path: Option<PathBuf>,
    /// Extra command line arguments, e.g. `--lang=en-US`.
    pub args: Vec<String>,
    /// Keep Chromium's sandbox (default `false`, like Playwright).
    pub chromium_sandbox: bool,
    /// Maximum time to wait for the browser to start, in ms (default 30000).
    pub timeout: u64,
    /// Environment variables of the browser process.
    pub env: Vec<(String, String)>,
    /// Profile directory; a fresh temporary one is used (and removed on
    /// close) when `None`.
    pub user_data_dir: Option<PathBuf>,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            headless: true,
            executable_path: None,
            args: Vec::new(),
            chromium_sandbox: false,
            timeout: 30_000,
            env: Vec::new(),
            user_data_dir: None,
        }
    }
}

/// Builder returned by [`BrowserType::launch`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct Launch {
    options: LaunchOptions,
}

impl Launch {
    /// Run headless (default `true`).
    pub fn headless(mut self, headless: bool) -> Self {
        self.options.headless = headless;
        self
    }

    /// Path of the browser executable.
    pub fn executable_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.options.executable_path = Some(path.into());
        self
    }

    /// Add command line arguments.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.options.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Enable Chromium's sandbox.
    pub fn chromium_sandbox(mut self, sandbox: bool) -> Self {
        self.options.chromium_sandbox = sandbox;
        self
    }

    /// Launch timeout in milliseconds (`0` = default of chromiumoxide).
    pub fn timeout(mut self, timeout_ms: u64) -> Self {
        self.options.timeout = timeout_ms;
        self
    }

    /// Set an environment variable of the browser process.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.options.env.push((key.into(), value.into()));
        self
    }

    /// Use a specific profile directory.
    pub fn user_data_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.options.user_data_dir = Some(dir.into());
        self
    }

    /// Replace all options at once.
    pub fn options(mut self, options: LaunchOptions) -> Self {
        self.options = options;
        self
    }
}

fn parse_arg(raw: &str) -> String {
    raw.trim_start_matches('-').to_string()
}

impl IntoFuture for Launch {
    type Output = Result<Browser>;
    type IntoFuture = BoxFuture<Result<Browser>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let options = self.options;
            let (profile_dir, owned_profile) = match &options.user_data_dir {
                Some(dir) => (dir.clone(), false),
                None => {
                    let suffix: String = std::iter::repeat_with(fastrand::alphanumeric)
                        .take(12)
                        .collect();
                    (
                        std::env::temp_dir().join(format!("oxidewright-profile-{suffix}")),
                        true,
                    )
                }
            };
            let mut builder = BrowserConfig::builder()
                .viewport(None)
                .user_data_dir(&profile_dir)
                .request_timeout(Duration::from_secs(600));
            if options.timeout > 0 {
                builder = builder.launch_timeout(Duration::from_millis(options.timeout));
            }
            builder = if options.headless {
                builder.headless_mode(HeadlessMode::True)
            } else {
                builder.with_head()
            };
            if !options.chromium_sandbox {
                builder = builder.no_sandbox();
            }
            if let Some(path) = &options.executable_path {
                builder = builder.chrome_executable(path);
            }
            // Like Playwright: navigations must not be restored from the
            // back/forward cache (they would not fire lifecycle events).
            builder = builder.arg("disable-back-forward-cache");
            for raw in &options.args {
                let arg = parse_arg(raw);
                builder = match arg.split_once('=') {
                    Some((key, value)) => builder.arg((key, value)),
                    None => builder.arg(arg.as_str()),
                };
            }
            for (key, value) in &options.env {
                builder = builder.env(key.clone(), value.clone());
            }
            let config = builder.build().map_err(Error::launch)?;
            let (cdp, handler) = chromiumoxide::Browser::launch(config)
                .await
                .map_err(|e| Error::launch(format!("Failed to launch the browser: {e}")))?;
            let inner =
                BrowserInner::start(cdp, handler, true, owned_profile.then_some(profile_dir))
                    .await?;
            Ok(Browser { inner })
        })
    }
}

/// Builder returned by [`BrowserType::connect_over_cdp`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct ConnectOverCdp {
    endpoint: String,
    timeout: Option<u64>,
}

impl ConnectOverCdp {
    /// Connection timeout in milliseconds (default 30000, `0` = none).
    pub fn timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout = Some(timeout_ms);
        self
    }
}

impl IntoFuture for ConnectOverCdp {
    type Output = Result<Browser>;
    type IntoFuture = BoxFuture<Result<Browser>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let timeout = self.timeout.unwrap_or(30_000);
            let connect = async {
                let (cdp, handler) = chromiumoxide::Browser::connect(self.endpoint.clone())
                    .await
                    .map_err(|e| Error::launch(format!("Failed to connect: {e}")))?;
                let inner = BrowserInner::start(cdp, handler, false, None).await?;
                inner.init_default_context().await?;
                Ok(Browser { inner })
            };
            let result = if timeout == 0 {
                connect.await
            } else {
                match tokio::time::timeout(Duration::from_millis(timeout), connect).await {
                    Ok(result) => result,
                    Err(_) => Err(Error::timeout_exceeded(timeout)),
                }
            };
            result.api("browserType.connectOverCDP")
        })
    }
}
