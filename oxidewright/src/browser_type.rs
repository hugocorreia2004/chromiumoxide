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
    /// Proxy of every request of the browser.
    pub proxy: Option<crate::ProxySettings>,
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
            proxy: None,
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

    /// Proxy of every request of the browser (each context can override it
    /// with [`crate::NewContext::proxy`]). `server` is `http://host:port`,
    /// `socks5://host:port` or `host:port`; `bypass` is a comma-separated
    /// list of hosts (`.example.com` matches subdomains). Loopback addresses
    /// go through the proxy too unless the bypass list names one.
    pub fn proxy(mut self, proxy: crate::ProxySettings) -> Self {
        self.options.proxy = Some(proxy);
        self
    }

    /// Profile directory (a fresh temporary one when not set).
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

/// Playwright's `chromiumSwitches`. With `stealth`, Patchright's changes:
/// no `--enable-automation` and none of the switches it drops, plus
/// `--disable-blink-features=AutomationControlled` (no
/// `navigator.webdriver = true`). `--disable-back-forward-cache` stays: the
/// navigation tracking needs lifecycle events for history navigations.
fn chromium_switches() -> Vec<String> {
    let disabled_features = [
        "AvoidUnnecessaryBeforeUnloadCheckSync",
        "DestroyProfileOnBrowserClose",
        "HttpsUpgrades",
        "HttpsFirstBalancedModeAutoEnable",
        "LensOverlay",
        // Patchright 1.64 keeps the media router (cast) like a normal Chrome.
        if cfg!(feature = "stealth") {
            ""
        } else {
            "MediaRouter"
        },
        "PaintHolding",
        // Not with `stealth`: an iframe could then read another site's
        // storage, which detectors check (Patchright still disables it).
        if cfg!(feature = "stealth") {
            ""
        } else {
            "ThirdPartyStoragePartitioning"
        },
        "AvoidCorsURLLoaderRestartOnRedirect",
        "TcpPortRandomizationWin",
        "OptimizationHints",
        "NetworkTimeServiceQuerying",
        "AimEnabled",
        "PreconnectToSearch",
    ]
    .into_iter()
    .filter(|f| !f.is_empty())
    .collect::<Vec<_>>()
    .join(",");
    let mut switches: Vec<String> = [
        "disable-field-trial-config",
        "disable-background-networking",
        "disable-background-timer-throttling",
        "disable-backgrounding-occluded-windows",
        "disable-back-forward-cache",
        "disable-breakpad",
        "no-default-browser-check",
        "disable-dev-shm-usage",
        "disable-hang-monitor",
        "disable-prompt-on-repost",
        "disable-renderer-backgrounding",
        "force-color-profile=srgb",
        "no-first-run",
        "password-store=basic",
        "use-mock-keychain",
        "no-service-autorun",
        "disable-search-engine-choice-screen",
        "enable-unsafe-swiftshader",
        "disable-stack-profiler",
        "disable-infobars",
        "disable-sync",
        "do-not-de-elevate",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    switches.push(format!("disable-features={disabled_features}"));
    switches.push(
        "enable-features=CDPScreenshotNewSurface,SkipListAccountsWithoutGaiaCookies".to_string(),
    );
    if cfg!(feature = "stealth") {
        switches.push("disable-blink-features=AutomationControlled".to_string());
    } else {
        // Switches Patchright drops, as they hint at automation.
        switches.extend(
            [
                "enable-automation",
                "disable-component-extensions-with-background-pages",
                "disable-component-update",
                "disable-default-apps",
                "allow-pre-commit-input",
                "disable-ipc-flooding-protection",
                "disable-popup-blocking",
                "metrics-recording-only",
                "unsafely-disable-devtools-self-xss-warnings",
            ]
            .map(String::from),
        );
    }
    switches
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
                .disable_default_args()
                .args(chromium_switches())
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
            for raw in &options.args {
                let arg = parse_arg(raw);
                builder = match arg.split_once('=') {
                    Some((key, value)) => builder.arg((key, value)),
                    None => builder.arg(arg.as_str()),
                };
            }
            let proxy = options
                .proxy
                .as_ref()
                .map(crate::proxy::normalize)
                .transpose()?;
            if let Some(proxy) = &proxy {
                for (key, value) in crate::proxy::launch_switches(proxy) {
                    builder = match value {
                        Some(value) => builder.arg((key.as_str(), value.as_str())),
                        None => builder.arg(key.as_str()),
                    };
                }
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
            *inner.ext.proxy.lock().unwrap() = proxy;
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
