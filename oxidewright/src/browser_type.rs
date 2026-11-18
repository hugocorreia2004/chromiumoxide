//! [`BrowserType`]: launching and connecting to Chromium.

use std::future::{Future, IntoFuture};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::time::Duration;

use chromiumoxide::BrowserConfig;
use chromiumoxide::handler::HandlerConfig;

use crate::browser::{Browser, BrowserInner};
use crate::browser_context::{BrowserContext, NewContext, NewContextOptions};
use crate::error::{Error, Result, ResultExt};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Entry point to launch or connect to a browser of a given type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    /// The browser executable `launch` uses when neither `executable_path`
    /// nor `channel` is given: the `CHROME` environment variable, then a
    /// Chrome/Chromium/Edge found on the `PATH` or in the usual install
    /// locations (oxidewright does not download browsers).
    pub fn executable_path(&self) -> Result<PathBuf> {
        chromiumoxide::detection::default_executable(Default::default()).map_err(|e| {
            Error::launch(format!("Browser is not supported on current platform: {e}"))
        })
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

    /// Launch a browser with the profile `user_data_dir` (created if
    /// missing; a relative path is resolved against the current directory)
    /// and return its default context, which takes every launch option and
    /// every context option. Closing the context closes the browser; the
    /// startup `about:blank` page is in [`BrowserContext::pages`].
    ///
    /// ```no_run
    /// # async fn run() -> oxidewright::Result<()> {
    /// let context = oxidewright::chromium()
    ///     .launch_persistent_context("profile")
    ///     .locale("de-DE")
    ///     .await?;
    /// let page = context.pages()[0].clone();
    /// # let _ = page; Ok(()) }
    /// ```
    pub fn launch_persistent_context(
        &self,
        user_data_dir: impl Into<PathBuf>,
    ) -> LaunchPersistentContext {
        LaunchPersistentContext {
            user_data_dir: user_data_dir.into(),
            options: LaunchOptions::default(),
            context: NewContext {
                browser: None,
                options: NewContextOptions::default(),
            },
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
            headers: Vec::new(),
            slow_mo: 0,
            is_local: false,
            no_defaults: false,
        }
    }
}

/// `ignoreDefaultArgs` of [`Launch`]: `true` drops every default switch,
/// a list drops only those switches (written like `--mute-audio`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum IgnoreDefaultArgs {
    /// Keep the default switches.
    #[default]
    None,
    /// Drop every default switch: only `args` are passed (plus the
    /// `--remote-debugging-port` and `--user-data-dir` the connection needs).
    All,
    /// Drop these default switches.
    List(Vec<String>),
}

impl From<bool> for IgnoreDefaultArgs {
    fn from(all: bool) -> Self {
        if all {
            IgnoreDefaultArgs::All
        } else {
            IgnoreDefaultArgs::None
        }
    }
}

impl<S: Into<String>> From<Vec<S>> for IgnoreDefaultArgs {
    fn from(list: Vec<S>) -> Self {
        IgnoreDefaultArgs::List(list.into_iter().map(Into::into).collect())
    }
}

impl<S: Into<String>, const N: usize> From<[S; N]> for IgnoreDefaultArgs {
    fn from(list: [S; N]) -> Self {
        IgnoreDefaultArgs::List(list.into_iter().map(Into::into).collect())
    }
}

/// Options of [`BrowserType::launch`].
#[derive(Debug, Clone)]
pub struct LaunchOptions {
    /// Run without a visible window (default `true`).
    pub headless: bool,
    /// Path of the Chromium/Chrome executable; detected when `None`.
    pub executable_path: Option<PathBuf>,
    /// Browser distribution: `chrome`, `chrome-beta`, `chrome-dev`,
    /// `chrome-canary`, `msedge`, `msedge-beta`, `msedge-dev`,
    /// `msedge-canary`, or `chromium` (the detected executable).
    pub channel: Option<String>,
    /// Extra command line arguments, e.g. `--lang=en-US`.
    pub args: Vec<String>,
    /// Default switches to leave out.
    pub ignore_default_args: IgnoreDefaultArgs,
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
    /// Slow down input actions, navigations and history calls by this many
    /// milliseconds (default `0`).
    pub slow_mo: u64,
    /// Directory of accepted downloads (default: a temporary directory
    /// removed when the browser closes).
    pub downloads_path: Option<PathBuf>,
    /// Accepted for Playwright compatibility; traces are buffered in memory
    /// until `tracing.stop()`, so nothing is written here.
    pub traces_dir: Option<PathBuf>,
    /// Accepted for Playwright compatibility (default `true`). The browser
    /// process is killed when the [`Browser`] is closed or dropped; no signal
    /// handler is installed.
    pub handle_sigint: bool,
    /// See [`LaunchOptions::handle_sigint`].
    pub handle_sigterm: bool,
    /// See [`LaunchOptions::handle_sigint`].
    pub handle_sighup: bool,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            headless: true,
            executable_path: None,
            channel: None,
            args: Vec::new(),
            ignore_default_args: IgnoreDefaultArgs::None,
            chromium_sandbox: false,
            timeout: 30_000,
            env: Vec::new(),
            user_data_dir: None,
            proxy: None,
            slow_mo: 0,
            downloads_path: None,
            traces_dir: None,
            handle_sigint: true,
            handle_sigterm: true,
            handle_sighup: true,
        }
    }
}

/// The launch option methods shared by [`Launch`] and
/// [`LaunchPersistentContext`].
macro_rules! launch_option_methods {
    () => {
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

        /// Browser distribution: `chrome`, `chrome-beta`, `chrome-dev`,
        /// `chrome-canary`, `msedge`, `msedge-beta`, `msedge-dev`,
        /// `msedge-canary` (found in their standard install location, like
        /// Playwright) or `chromium` (the detected executable).
        pub fn channel(mut self, channel: impl Into<String>) -> Self {
            self.options.channel = Some(channel.into());
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

        /// `true` leaves out every default switch; a list leaves out only
        /// those (e.g. `["--mute-audio"]`).
        pub fn ignore_default_args(mut self, ignore: impl Into<IgnoreDefaultArgs>) -> Self {
            self.options.ignore_default_args = ignore.into();
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

        /// Slow down input actions, navigations and history calls by
        /// `slow_mo_ms` milliseconds each.
        pub fn slow_mo(mut self, slow_mo_ms: u64) -> Self {
            self.options.slow_mo = slow_mo_ms;
            self
        }

        /// Directory of accepted downloads (created if missing; kept when the
        /// browser closes).
        pub fn downloads_path(mut self, dir: impl Into<PathBuf>) -> Self {
            self.options.downloads_path = Some(dir.into());
            self
        }

        /// Accepted for Playwright compatibility: traces are buffered in
        /// memory until `tracing.stop()`, nothing is written to this directory.
        pub fn traces_dir(mut self, dir: impl Into<PathBuf>) -> Self {
            self.options.traces_dir = Some(dir.into());
            self
        }

        /// Accepted for Playwright compatibility: no signal handler is
        /// installed (the browser process is killed when the [`Browser`] is
        /// closed or dropped).
        pub fn handle_sigint(mut self, handle: bool) -> Self {
            self.options.handle_sigint = handle;
            self
        }

        /// See [`Launch::handle_sigint`].
        pub fn handle_sigterm(mut self, handle: bool) -> Self {
            self.options.handle_sigterm = handle;
            self
        }

        /// See [`Launch::handle_sigint`].
        pub fn handle_sighup(mut self, handle: bool) -> Self {
            self.options.handle_sighup = handle;
            self
        }
    };
}

/// Builder returned by [`BrowserType::launch`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct Launch {
    options: LaunchOptions,
}

impl Launch {
    launch_option_methods!();

    /// Profile directory (a fresh temporary one when not set). Prefer
    /// [`BrowserType::launch_persistent_context`], which returns the
    /// profile's default context like Playwright.
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

/// Builder returned by [`BrowserType::launch_persistent_context`]; takes
/// the launch options and the context options. `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct LaunchPersistentContext {
    user_data_dir: PathBuf,
    options: LaunchOptions,
    context: NewContext,
}

impl LaunchPersistentContext {
    launch_option_methods!();
    crate::browser_context::forward_context_options!(context);

    /// Replace all launch options at once (`user_data_dir` is ignored).
    pub fn launch_options(mut self, options: LaunchOptions) -> Self {
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

/// Install locations of the Chromium channels (Playwright's registry).
fn channel_location(channel: &str) -> Option<&'static str> {
    #[cfg(windows)]
    let table: &[(&str, &str)] = &[
        ("chrome", r"Google\Chrome\Application\chrome.exe"),
        ("chrome-beta", r"Google\Chrome Beta\Application\chrome.exe"),
        ("chrome-dev", r"Google\Chrome Dev\Application\chrome.exe"),
        ("chrome-canary", r"Google\Chrome SxS\Application\chrome.exe"),
        ("msedge", r"Microsoft\Edge\Application\msedge.exe"),
        ("msedge-beta", r"Microsoft\Edge Beta\Application\msedge.exe"),
        ("msedge-dev", r"Microsoft\Edge Dev\Application\msedge.exe"),
        (
            "msedge-canary",
            r"Microsoft\Edge SxS\Application\msedge.exe",
        ),
    ];
    #[cfg(target_os = "macos")]
    let table: &[(&str, &str)] = &[
        (
            "chrome",
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ),
        (
            "chrome-beta",
            "/Applications/Google Chrome Beta.app/Contents/MacOS/Google Chrome Beta",
        ),
        (
            "chrome-dev",
            "/Applications/Google Chrome Dev.app/Contents/MacOS/Google Chrome Dev",
        ),
        (
            "chrome-canary",
            "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
        ),
        (
            "msedge",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ),
        (
            "msedge-beta",
            "/Applications/Microsoft Edge Beta.app/Contents/MacOS/Microsoft Edge Beta",
        ),
        (
            "msedge-dev",
            "/Applications/Microsoft Edge Dev.app/Contents/MacOS/Microsoft Edge Dev",
        ),
        (
            "msedge-canary",
            "/Applications/Microsoft Edge Canary.app/Contents/MacOS/Microsoft Edge Canary",
        ),
    ];
    #[cfg(not(any(windows, target_os = "macos")))]
    let table: &[(&str, &str)] = &[
        ("chrome", "/opt/google/chrome/chrome"),
        ("chrome-beta", "/opt/google/chrome-beta/chrome"),
        ("chrome-dev", "/opt/google/chrome-unstable/chrome"),
        ("chrome-canary", "/opt/google/chrome-canary/chrome"),
        ("msedge", "/opt/microsoft/msedge/msedge"),
        ("msedge-beta", "/opt/microsoft/msedge-beta/msedge"),
        ("msedge-dev", "/opt/microsoft/msedge-dev/msedge"),
    ];
    table
        .iter()
        .find(|(name, _)| *name == channel)
        .map(|(_, path)| *path)
}

/// The executable of a channel, looked up like Playwright's registry
/// (`%LOCALAPPDATA%`, `%PROGRAMFILES%`, `%PROGRAMFILES(X86)%` on Windows).
/// `None` for the `chromium` aliases (the detected executable is used).
fn channel_executable(channel: &str) -> Result<Option<PathBuf>> {
    if channel.starts_with("chromium") {
        return Ok(None);
    }
    let known = [
        "chrome",
        "chrome-beta",
        "chrome-dev",
        "chrome-canary",
        "msedge",
        "msedge-beta",
        "msedge-dev",
        "msedge-canary",
    ];
    if !known.contains(&channel) {
        return Err(Error::launch(format!(
            "Unsupported chromium channel \"{channel}\""
        )));
    }
    let Some(suffix) = channel_location(channel) else {
        return Err(Error::launch(format!(
            "Chromium distribution '{channel}' is not supported on {}",
            std::env::consts::OS
        )));
    };
    let prefixes: Vec<PathBuf> = if cfg!(windows) {
        let mut prefixes: Vec<PathBuf> = ["LOCALAPPDATA", "PROGRAMFILES", "PROGRAMFILES(X86)"]
            .iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .collect();
        if let Some(drive) = std::env::var_os("HOMEDRIVE") {
            let drive = PathBuf::from(format!("{}\\", drive.to_string_lossy()));
            prefixes.push(drive.join("Program Files"));
            prefixes.push(drive.join("Program Files (x86)"));
        }
        prefixes
    } else {
        vec![PathBuf::new()]
    };
    for prefix in &prefixes {
        let path = prefix.join(suffix);
        if path.is_file() {
            return Ok(Some(path));
        }
    }
    let location = prefixes
        .first()
        .map(|p| format!(" at {}", p.join(suffix).display()))
        .unwrap_or_default();
    Err(Error::launch(format!(
        "Chromium distribution '{channel}' is not found{location}"
    )))
}

/// The command line of a launch (`_innerDefaultArgs` + `defaultArgs`), as
/// full switches (`--name` or `--name=value`) before `ignore_default_args`
/// is applied. `--user-data-dir` and `--remote-debugging-port` are added by
/// chromiumoxide.
fn default_args(options: &LaunchOptions, proxy: Option<&crate::ProxySettings>) -> Vec<String> {
    let mut args: Vec<String> = chromium_switches()
        .into_iter()
        .map(|s| format!("--{s}"))
        .collect();
    if options.headless {
        args.extend(["--headless", "--hide-scrollbars", "--mute-audio"].map(String::from));
    }
    if !options.chromium_sandbox {
        args.push("--no-sandbox".to_string());
    }
    if let Some(proxy) = proxy {
        for (key, value) in crate::proxy::launch_switches(proxy) {
            args.push(match value {
                Some(value) => format!("--{key}={value}"),
                None => format!("--{key}"),
            });
        }
    }
    args
}

/// Playwright's checks of `args` (skipped with `ignore_default_args(true)`).
fn validate_args(args: &[String]) -> Result<()> {
    if args.iter().any(|a| a.starts_with("--user-data-dir")) {
        return Err(Error::invalid_argument(
            "Pass userDataDir parameter to 'browserType.launchPersistentContext(userDataDir, options)' instead of specifying '--user-data-dir' argument",
        ));
    }
    if args
        .iter()
        .any(|a| a.starts_with("--remote-debugging-pipe"))
    {
        return Err(Error::invalid_argument(
            "Playwright manages remote debugging connection itself.",
        ));
    }
    if args.iter().any(|a| !a.starts_with('-')) {
        return Err(Error::invalid_argument(
            "Arguments can not specify page to be opened",
        ));
    }
    Ok(())
}

/// Launch the browser process; `persistent` opens `about:blank` in the
/// profile's default context instead of starting without a page.
async fn launch_browser(options: LaunchOptions, persistent: bool) -> Result<Browser> {
    let ignore_all = options.ignore_default_args == IgnoreDefaultArgs::All;
    if !ignore_all {
        validate_args(&options.args)?;
    }
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
    let proxy = options
        .proxy
        .as_ref()
        .map(crate::proxy::normalize)
        .transpose()?;
    let mut args: Vec<String> = match &options.ignore_default_args {
        IgnoreDefaultArgs::All => Vec::new(),
        IgnoreDefaultArgs::None => default_args(&options, proxy.as_ref()),
        IgnoreDefaultArgs::List(list) => default_args(&options, proxy.as_ref())
            .into_iter()
            .filter(|a| !list.contains(a))
            .collect(),
    };
    args.extend(options.args.iter().cloned());
    let mut builder = BrowserConfig::builder()
        .disable_default_args()
        // New pages and popups stay paused until adopted and configured, so
        // their first document already gets the context's emulation.
        .wait_for_debugger_on_start()
        .viewport(None)
        .user_data_dir(&profile_dir)
        .request_timeout(Duration::from_secs(600))
        // Headless and sandbox switches are part of `args` (so they can be
        // ignored): chromiumoxide adds none.
        .with_head();
    if options.timeout > 0 {
        builder = builder.launch_timeout(Duration::from_millis(options.timeout));
    }
    let executable = match (&options.executable_path, &options.channel) {
        (Some(path), _) => Some(path.clone()),
        (None, Some(channel)) => channel_executable(channel)?,
        (None, None) => None,
    };
    if let Some(path) = &executable {
        builder = builder.chrome_executable(path);
    }
    for raw in &args {
        if !raw.starts_with('-') {
            // Only reachable with `ignore_default_args(true)`: a URL to open.
            builder = builder.start_url(raw.clone());
            continue;
        }
        let arg = parse_arg(raw);
        builder = match arg.split_once('=') {
            Some((key, value)) => builder.arg((key, value)),
            None => builder.arg(arg.as_str()),
        };
    }
    if persistent && !ignore_all && !args.iter().any(|a| a == "about:blank") {
        builder = builder.start_url("about:blank");
    }
    for (key, value) in &options.env {
        builder = builder.env(key.clone(), value.clone());
    }
    let config = builder.build().map_err(Error::launch)?;
    let (cdp, handler) = chromiumoxide::Browser::launch(config)
        .await
        .map_err(|e| Error::launch(format!("Failed to launch the browser: {e}")))?;
    let inner =
        BrowserInner::start(cdp, handler, true, owned_profile.then_some(profile_dir)).await?;
    *inner.ext.proxy.lock().unwrap() = proxy;
    inner.slow_mo.store(options.slow_mo, Ordering::Relaxed);
    if let Some(dir) = &options.downloads_path {
        inner.ext.downloads.set_user_dir(absolute(dir));
    }
    Ok(Browser { inner })
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

impl IntoFuture for Launch {
    type Output = Result<Browser>;
    type IntoFuture = BoxFuture<Result<Browser>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            launch_browser(self.options, false)
                .await
                .api("browserType.launch")
        })
    }
}

impl IntoFuture for LaunchPersistentContext {
    type Output = Result<BrowserContext>;
    type IntoFuture = BoxFuture<Result<BrowserContext>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            async {
                let mut options = self.options;
                let context_options = self.context.options;
                crate::emulation::validate_options(&context_options)?;
                crate::video::validate_options(&context_options)?;
                let dir = absolute(&self.user_data_dir);
                std::fs::create_dir_all(&dir).map_err(|e| {
                    Error::launch(format!(
                        "Failed to create the user data dir {}: {e}",
                        dir.display()
                    ))
                })?;
                options.user_data_dir = Some(dir);
                let ignore_all = options.ignore_default_args == IgnoreDefaultArgs::All;
                let timeout = options.timeout;
                let browser = launch_browser(options, true).await?;
                browser.inner.persistent.store(true, Ordering::SeqCst);
                // Playwright waits for the startup page; with custom
                // arguments it assumes no control and only takes what is there.
                let wait = if ignore_all {
                    Duration::from_secs(1)
                } else {
                    Duration::from_millis(if timeout == 0 { 30_000 } else { timeout })
                };
                match browser
                    .inner
                    .adopt_default_context(context_options, Some(wait))
                    .await
                {
                    Ok(context) => {
                        context.keep_browser(browser);
                        Ok(context)
                    }
                    Err(err) => {
                        let _ = browser.close().await;
                        Err(err)
                    }
                }
            }
            .await
            .api("browserType.launchPersistentContext")
        })
    }
}

/// Builder returned by [`BrowserType::connect_over_cdp`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct ConnectOverCdp {
    endpoint: String,
    timeout: Option<u64>,
    headers: Vec<(String, String)>,
    slow_mo: u64,
    is_local: bool,
    no_defaults: bool,
}

impl ConnectOverCdp {
    /// Connection timeout in milliseconds (default 30000, `0` = none).
    pub fn timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout = Some(timeout_ms);
        self
    }

    /// Extra HTTP headers sent with the connection request (the
    /// `/json/version` request and the websocket handshake).
    pub fn headers<K: Into<String>, V: Into<String>>(
        mut self,
        headers: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        self.headers
            .extend(headers.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    /// Slow down input actions, navigations and history calls by
    /// `slow_mo_ms` milliseconds each.
    pub fn slow_mo(mut self, slow_mo_ms: u64) -> Self {
        self.slow_mo = slow_mo_ms;
        self
    }

    /// Accepted for Playwright compatibility: the browser and oxidewright
    /// are assumed to share the file system either way.
    pub fn is_local(mut self, is_local: bool) -> Self {
        self.is_local = is_local;
        self
    }

    /// Leave the default context as the browser has it: its download
    /// behavior is not changed (downloads are then not reported).
    pub fn no_defaults(mut self, no_defaults: bool) -> Self {
        self.no_defaults = no_defaults;
        self
    }
}

impl IntoFuture for ConnectOverCdp {
    type Output = Result<Browser>;
    type IntoFuture = BoxFuture<Result<Browser>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let timeout = self.timeout.unwrap_or(30_000);
            let _ = self.is_local;
            let connect = async {
                let (cdp, handler) = chromiumoxide::Browser::connect_with_headers(
                    self.endpoint.clone(),
                    HandlerConfig {
                        wait_for_debugger_on_start: true,
                        ..HandlerConfig::default()
                    },
                    self.headers.clone(),
                )
                .await
                .map_err(|e| Error::launch(format!("Failed to connect: {e}")))?;
                let inner = BrowserInner::start(cdp, handler, false, None).await?;
                inner.slow_mo.store(self.slow_mo, Ordering::Relaxed);
                inner.no_defaults.store(self.no_defaults, Ordering::SeqCst);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_default_args_filters_switches() {
        let options = LaunchOptions::default();
        let args = default_args(&options, None);
        assert!(args.contains(&"--headless".to_string()));
        assert!(args.contains(&"--no-sandbox".to_string()));
        assert!(validate_args(&["http://example.com".to_string()]).is_err());
        assert!(validate_args(&["--user-data-dir=x".to_string()]).is_err());
        assert!(validate_args(&["--lang=en".to_string()]).is_ok());
        assert_eq!(IgnoreDefaultArgs::from(true), IgnoreDefaultArgs::All);
        assert_eq!(
            IgnoreDefaultArgs::from(["--mute-audio"]),
            IgnoreDefaultArgs::List(vec!["--mute-audio".into()])
        );
    }

    #[test]
    fn unknown_channels_are_rejected() {
        assert!(channel_executable("chromium").unwrap().is_none());
        assert!(channel_executable("firefox").is_err());
    }
}
