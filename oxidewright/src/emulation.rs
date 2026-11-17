//! Emulation: viewport/screen/device metrics, touch, user agent, locale,
//! timezone, geolocation, permissions, media features, JavaScript, CSP
//! bypass, service workers and downloads (Playwright's
//! `CRPage.FrameSession._initialize`, `_updateViewport`,
//! `_updateEmulateMedia`, `_updateUserAgent`, `_updateGeolocation` and
//! `CRBrowserContext.doGrantPermissions`).
//!
//! ## Stealth
//!
//! Every override below is observable by the page (`Emulation.*` changes
//! `navigator`, `matchMedia`, `Intl`, screen metrics...). oxidewright only
//! sends an override when the corresponding option is set:
//!
//! * contexts created with [`crate::Browser::new_context`] get Playwright's
//!   default 1280x720 viewport (pass `.viewport(None)` to keep the window
//!   size);
//! * the default context of a browser attached with
//!   [`crate::BrowserType::connect_over_cdp`] overrides nothing, like
//!   Playwright;
//! * media features (`prefers-color-scheme`, ...) are only emulated when set
//!   on the context or with [`Page::emulate_media`]; features left unset are
//!   not overridden (Playwright forces `light`/`no-preference`/`none`).

use std::future::{Future, IntoFuture};
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::browser_context::{BrowserContext, NewContextOptions};
use crate::cdp_raw::RawCommand;
use crate::error::{Error, Result, ResultExt};
use crate::page::{Page, PageInner, ViewportSize};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// A geographic position for `geolocation` emulation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Geolocation {
    /// Latitude between -90 and 90.
    pub latitude: f64,
    /// Longitude between -180 and 180.
    pub longitude: f64,
    /// Non-negative accuracy in meters (default 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accuracy: Option<f64>,
}

impl Geolocation {
    /// A position with accuracy 0.
    pub fn new(latitude: f64, longitude: f64) -> Self {
        Self {
            latitude,
            longitude,
            accuracy: None,
        }
    }

    /// Set the accuracy in meters.
    pub fn accuracy(mut self, accuracy: f64) -> Self {
        self.accuracy = Some(accuracy);
        self
    }

    /// `verifyGeolocation`.
    pub(crate) fn verify(&self) -> Result<()> {
        if !(-180.0..=180.0).contains(&self.longitude) {
            return Err(Error::invalid_argument(
                "geolocation.longitude: precondition -180 <= LONGITUDE <= 180 failed.",
            ));
        }
        if !(-90.0..=90.0).contains(&self.latitude) {
            return Err(Error::invalid_argument(
                "geolocation.latitude: precondition -90 <= LATITUDE <= 90 failed.",
            ));
        }
        if self.accuracy.unwrap_or(0.0) < 0.0 {
            return Err(Error::invalid_argument(
                "geolocation.accuracy: precondition 0 <= ACCURACY failed.",
            ));
        }
        Ok(())
    }
}

macro_rules! media_enum {
    ($(#[$m:meta])* $name:ident { $($(#[$vm:meta])* $variant:ident => $value:literal),* $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($(#[$vm])* $variant,)*
            /// Remove the override (Playwright's `null`).
            NoOverride,
        }

        impl $name {
            /// The protocol value (`""` for [`Self::NoOverride`]).
            pub fn as_str(&self) -> &'static str {
                match self {
                    $($name::$variant => $value,)*
                    $name::NoOverride => "",
                }
            }
        }
    };
}

media_enum!(
    /// `prefers-color-scheme`.
    ColorScheme {
        /// `light`
        Light => "light",
        /// `dark`
        Dark => "dark",
        /// `no-preference`
        NoPreference => "no-preference",
    }
);

media_enum!(
    /// `prefers-reduced-motion`.
    ReducedMotion {
        /// `reduce`
        Reduce => "reduce",
        /// `no-preference`
        NoPreference => "no-preference",
    }
);

media_enum!(
    /// `forced-colors`.
    ForcedColors {
        /// `active`
        Active => "active",
        /// `none`
        None => "none",
    }
);

media_enum!(
    /// `prefers-contrast`.
    Contrast {
        /// `no-preference`
        NoPreference => "no-preference",
        /// `more`
        More => "more",
    }
);

media_enum!(
    /// CSS media type.
    Media {
        /// `screen`
        Screen => "screen",
        /// `print`
        Print => "print",
    }
);

/// Whether pages may register service workers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ServiceWorkerPolicy {
    /// Service workers are allowed (default).
    #[default]
    Allow,
    /// `navigator.serviceWorker.register` is replaced by a no-op.
    Block,
}

/// Media overrides of a page (`page.emulateMedia`); `None` = not set.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct EmulatedMedia {
    pub(crate) media: Option<Media>,
    pub(crate) color_scheme: Option<ColorScheme>,
    pub(crate) reduced_motion: Option<ReducedMotion>,
    pub(crate) forced_colors: Option<ForcedColors>,
    pub(crate) contrast: Option<Contrast>,
}

impl EmulatedMedia {
    fn any(&self) -> bool {
        self.media.is_some()
            || self.color_scheme.is_some()
            || self.reduced_motion.is_some()
            || self.forced_colors.is_some()
            || self.contrast.is_some()
    }
}

/// Viewport and screen of a page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct EmulatedSize {
    pub(crate) viewport: ViewportSize,
    pub(crate) screen: ViewportSize,
}

/// The parameters of the last `Emulation.setDeviceMetricsOverride`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MetricsOverride {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) screen_width: u32,
    pub(crate) screen_height: u32,
    pub(crate) device_scale_factor: f64,
    pub(crate) mobile: bool,
    pub(crate) landscape: bool,
}

// ----------------------------------------------------------------------
// Context options helpers
// ----------------------------------------------------------------------

fn context_options(page: &PageInner) -> Option<NewContextOptions> {
    page.context.upgrade().map(|c| c.options.clone())
}

/// `validateBrowserContextOptions`.
pub(crate) fn validate_options(options: &NewContextOptions) -> Result<()> {
    if options.viewport.is_none() && options.device_scale_factor.is_some() {
        return Err(Error::invalid_argument(
            "\"deviceScaleFactor\" option is not supported with null \"viewport\"",
        ));
    }
    if options.viewport.is_none() && options.is_mobile == Some(true) {
        return Err(Error::invalid_argument(
            "\"isMobile\" option is not supported with null \"viewport\"",
        ));
    }
    if let Some(geolocation) = &options.geolocation {
        geolocation.verify()?;
    }
    Ok(())
}

async fn send(page: &PageInner, method: &str, params: Value) -> Result<Value> {
    Ok(page
        .cdp
        .execute(RawCommand::new(method, params))
        .await?
        .result)
}

/// `Page.emulatedSize()`: the page override, else the context viewport.
pub(crate) fn emulated_size(page: &PageInner) -> Option<EmulatedSize> {
    if let Some(size) = *page.emu.emulated_size.lock().unwrap() {
        return Some(size);
    }
    let options = context_options(page)?;
    let viewport = options.viewport?;
    Some(EmulatedSize {
        viewport,
        screen: options.screen.unwrap_or(viewport),
    })
}

/// `FrameSession._updateViewport`.
pub(crate) async fn update_viewport(page: &PageInner) -> Result<()> {
    let Some(size) = emulated_size(page) else {
        return Ok(());
    };
    let options = context_options(page).unwrap_or_default();
    let mobile = options.is_mobile.unwrap_or(false);
    let metrics = MetricsOverride {
        width: size.viewport.width,
        height: size.viewport.height,
        screen_width: size.screen.width,
        screen_height: size.screen.height,
        device_scale_factor: options.device_scale_factor.unwrap_or(1.0),
        mobile,
        landscape: size.screen.width > size.screen.height,
    };
    if *page.emu.metrics_override.lock().unwrap() == Some(metrics) {
        return Ok(());
    }
    let orientation = if mobile {
        if metrics.landscape {
            json!({ "angle": 90, "type": "landscapePrimary" })
        } else {
            json!({ "angle": 0, "type": "portraitPrimary" })
        }
    } else {
        json!({ "angle": 0, "type": "landscapePrimary" })
    };
    send(
        page,
        "Emulation.setDeviceMetricsOverride",
        json!({
            "mobile": mobile,
            "width": metrics.width,
            "height": metrics.height,
            "screenWidth": metrics.screen_width,
            "screenHeight": metrics.screen_height,
            "deviceScaleFactor": metrics.device_scale_factor,
            "screenOrientation": orientation,
        }),
    )
    .await?;
    *page.emu.metrics_override.lock().unwrap() = Some(metrics);
    *page.viewport.lock().unwrap() = Some(size.viewport);
    Ok(())
}

/// `page.setViewportSize`: the screen follows the viewport.
pub(crate) async fn set_page_viewport(page: &PageInner, viewport: ViewportSize) -> Result<()> {
    let old = *page.emu.emulated_size.lock().unwrap();
    *page.emu.emulated_size.lock().unwrap() = Some(EmulatedSize {
        viewport,
        screen: viewport,
    });
    if let Err(err) = update_viewport(page).await {
        *page.emu.emulated_size.lock().unwrap() = old;
        return Err(err);
    }
    Ok(())
}

/// `Page.emulatedMedia()` merged with the context options; `None` when no
/// media feature is emulated at all.
fn effective_media(page: &PageInner) -> Option<EmulatedMedia> {
    let own = *page.emu.emulated_media.lock().unwrap();
    let options = context_options(page).unwrap_or_default();
    let merged = EmulatedMedia {
        media: own.media,
        color_scheme: own.color_scheme.or(options.color_scheme),
        reduced_motion: own.reduced_motion.or(options.reduced_motion),
        forced_colors: own.forced_colors.or(options.forced_colors),
        contrast: own.contrast.or(options.contrast),
    };
    merged.any().then_some(merged)
}

/// `FrameSession._updateEmulateMedia`.
pub(crate) async fn update_emulate_media(page: &PageInner) -> Result<()> {
    let Some(media) = effective_media(page) else {
        return Ok(());
    };
    let value = |s: Option<&'static str>| s.unwrap_or("");
    send(
        page,
        "Emulation.setEmulatedMedia",
        json!({
            "media": value(media.media.map(|m| m.as_str())),
            "features": [
                { "name": "prefers-color-scheme", "value": value(media.color_scheme.map(|m| m.as_str())) },
                { "name": "prefers-reduced-motion", "value": value(media.reduced_motion.map(|m| m.as_str())) },
                { "name": "forced-colors", "value": value(media.forced_colors.map(|m| m.as_str())) },
                { "name": "prefers-contrast", "value": value(media.contrast.map(|m| m.as_str())) },
            ],
        }),
    )
    .await?;
    Ok(())
}

/// `FrameSession._updateGeolocation`.
pub(crate) async fn update_geolocation(page: &PageInner, initial: bool) -> Result<()> {
    let geolocation = page
        .context
        .upgrade()
        .and_then(|c| *c.emu.geolocation.lock().unwrap());
    if !initial || geolocation.is_some() {
        let params = match geolocation {
            Some(g) => json!({
                "latitude": g.latitude,
                "longitude": g.longitude,
                "accuracy": g.accuracy.unwrap_or(0.0),
            }),
            None => json!({}),
        };
        send(page, "Emulation.setGeolocationOverride", params).await?;
    }
    Ok(())
}

/// `calculateUserAgentMetadata` (Chromium's `user_agent_utils.cc`).
pub(crate) fn user_agent_metadata(user_agent: &str, is_mobile: bool) -> Value {
    let mut platform = "Windows".to_string();
    let mut platform_version = String::new();
    let mut architecture = "x86";
    let version_of = |pattern: &str| {
        regex::Regex::new(pattern)
            .ok()
            .and_then(|re| re.captures(user_agent))
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
    };
    if let Some(v) = version_of(r"Android (\d+(\.\d+)?(\.\d+)?)") {
        platform = "Android".into();
        platform_version = v;
        architecture = "arm";
    } else if let Some(v) = version_of(r"iPhone OS (\d+(_\d+)?)") {
        platform = "iOS".into();
        platform_version = v;
        architecture = "arm";
    } else if let Some(v) = version_of(r"iPad; CPU OS (\d+(_\d+)?)") {
        platform = "iOS".into();
        platform_version = v;
        architecture = "arm";
    } else if let Some(v) = version_of(r"Mac OS X (\d+(_\d+)?(_\d+)?)") {
        platform = "macOS".into();
        platform_version = v;
        if !user_agent.contains("Intel") {
            architecture = "arm";
        }
    } else if let Some(v) = version_of(r"Windows\D+(\d+(\.\d+)?(\.\d+)?)") {
        platform = "Windows".into();
        platform_version = v;
    } else if user_agent.to_lowercase().contains("linux") {
        platform = "Linux".into();
    }
    if user_agent.contains("ARM") {
        architecture = "arm";
    }
    json!({
        "mobile": is_mobile,
        "model": "",
        "architecture": architecture,
        "platform": platform,
        "platformVersion": platform_version,
    })
}

/// `FrameSession._updateUserAgent`.
async fn update_user_agent(page: &PageInner, options: &NewContextOptions) -> Result<()> {
    let mut params = serde_json::Map::new();
    params.insert(
        "userAgent".into(),
        json!(options.user_agent.clone().unwrap_or_default()),
    );
    if let Some(locale) = &options.locale {
        params.insert("acceptLanguage".into(), json!(locale));
    }
    if let Some(ua) = &options.user_agent {
        params.insert(
            "userAgentMetadata".into(),
            user_agent_metadata(ua, options.is_mobile.unwrap_or(false)),
        );
    }
    send(
        page,
        "Emulation.setUserAgentOverride",
        Value::Object(params),
    )
    .await?;
    Ok(())
}

/// Settings a new page's first request and document need, sent before the
/// page is resumed (it starts paused, see `wait_for_debugger_on_start`) and
/// before anything waits on its renderer: a new window that needs a new
/// renderer process answers renderer commands only once resumed, like
/// Playwright sending its whole setup together with
/// `Runtime.runIfWaitingForDebugger`. Init scripts are only queued (they
/// reach the renderer when it starts); browser-side commands are awaited
/// briefly. Returns whether the init scripts were installed.
pub(crate) async fn preconfigure_new_page(
    cdp: &chromiumoxide::Page,
    context: &crate::BrowserContext,
    init_scripts: &[String],
) -> bool {
    use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;
    use chromiumoxide::cdp::browser_protocol::page::GetFrameTreeParams;
    use futures::FutureExt;
    use std::time::Duration;
    // A page whose renderer runs (in-process popups, new pages) answers right
    // away: it gets the full setup and is resumed at the end of adoption.
    if tokio::time::timeout(
        Duration::from_millis(500),
        cdp.execute(GetFrameTreeParams::default()),
    )
    .await
    .is_ok()
    {
        return false;
    }
    let options = &context.inner.options;
    let quick = |method: &'static str, params: Value| async move {
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            cdp.execute(RawCommand::new(method, params)),
        )
        .await;
    };
    // Queue the init scripts first, without waiting for the renderer.
    // Polling once queues each command on the page's channel ahead of the
    // resume below; the replies only come once the renderer starts.
    for script in init_scripts {
        let mut params = AddScriptToEvaluateOnNewDocumentParams::new(script.clone());
        params.run_immediately = Some(true);
        let _ = cdp.execute(params).now_or_never();
    }
    if options.user_agent.is_some() || options.locale.is_some() {
        let mut params = serde_json::Map::new();
        params.insert(
            "userAgent".into(),
            json!(options.user_agent.clone().unwrap_or_default()),
        );
        if let Some(locale) = &options.locale {
            params.insert("acceptLanguage".into(), json!(locale));
        }
        if let Some(ua) = &options.user_agent {
            params.insert(
                "userAgentMetadata".into(),
                user_agent_metadata(ua, options.is_mobile.unwrap_or(false)),
            );
        }
        quick("Emulation.setUserAgentOverride", Value::Object(params)).await;
    }
    let headers = context.inner.ext.extra_headers.lock().unwrap().clone();
    let offline = context
        .inner
        .ext
        .offline
        .load(std::sync::atomic::Ordering::SeqCst);
    if !headers.is_empty() || offline {
        quick("Network.enable", json!({})).await;
    }
    if !headers.is_empty() {
        let map: serde_json::Map<String, Value> = headers
            .iter()
            .map(|h| (h.name.clone(), json!(h.value)))
            .collect();
        quick("Network.setExtraHTTPHeaders", json!({ "headers": map })).await;
    }
    if offline {
        quick(
            "Network.emulateNetworkConditions",
            json!({ "offline": true, "latency": 0, "downloadThroughput": -1, "uploadThroughput": -1 }),
        )
        .await;
    }
    let _ = cdp
        .execute(chromiumoxide::cdp::js_protocol::runtime::RunIfWaitingForDebuggerParams::default())
        .await;
    true
}

/// Apply the context options to a newly attached page
/// (`FrameSession._initialize`), only for options that are set.
pub(crate) async fn init_page(page: &Page) -> Result<()> {
    let inner = &page.inner;
    let Some(options) = context_options(inner) else {
        return Ok(());
    };
    if options.bypass_csp {
        send(inner, "Page.setBypassCSP", json!({ "enabled": true })).await?;
    }
    if options.ignore_https_errors {
        send(
            inner,
            "Security.setIgnoreCertificateErrors",
            json!({ "ignore": true }),
        )
        .await?;
    }
    update_viewport(inner).await?;
    if options.has_touch == Some(true) {
        send(
            inner,
            "Emulation.setTouchEmulationEnabled",
            json!({ "enabled": true }),
        )
        .await?;
    }
    if options.java_script_enabled == Some(false) {
        send(
            inner,
            "Emulation.setScriptExecutionDisabled",
            json!({ "value": true }),
        )
        .await?;
    }
    if options.user_agent.is_some() || options.locale.is_some() {
        update_user_agent(inner, &options).await?;
    }
    if let Some(locale) = &options.locale {
        if let Err(err) = send(
            inner,
            "Emulation.setLocaleOverride",
            json!({ "locale": locale }),
        )
        .await
        {
            if !err
                .to_string()
                .contains("Another locale override is already in effect")
            {
                return Err(err);
            }
        }
    }
    if let Some(timezone) = &options.timezone_id {
        if let Err(err) = send(
            inner,
            "Emulation.setTimezoneOverride",
            json!({ "timezoneId": timezone }),
        )
        .await
        {
            let text = err.to_string();
            if text.contains("Invalid timezone") {
                return Err(Error::invalid_argument(format!(
                    "Invalid timezone ID: {timezone}"
                )));
            }
            if !text.contains("Timezone override is already in effect") {
                return Err(err);
            }
        }
    }
    update_geolocation(inner, true).await?;
    update_emulate_media(inner).await?;
    Ok(())
}

/// Context-level initialization (`BrowserContext._initialize` and
/// `CRBrowserContext.initialize`): service workers, permissions
/// and the initial storage state.
pub(crate) async fn init_context(context: &BrowserContext) -> Result<()> {
    let options = context.inner.options.clone();
    *context.inner.emu.geolocation.lock().unwrap() = options.geolocation;
    if options.service_workers == ServiceWorkerPolicy::Block {
        context
            .add_init_script(if cfg!(feature = "stealth") {
                // Patchright: no console message naming the automation.
                "\nif (self.ServiceWorkerContainer) ServiceWorkerContainer.prototype.register = async () => {};\n"
            } else {
                "\nif (self.ServiceWorkerContainer) ServiceWorkerContainer.prototype.register = async () => { console.warn('Service Worker registration blocked by Playwright'); };\n"
            })
            .await?;
    }
    if let Some(permissions) = &options.permissions {
        context.grant(permissions.clone(), None).await?;
    }
    if let Some(state) = &options.storage_state {
        let state = state.load().await?;
        crate::storage_state::set_storage_state(context, &state, true).await?;
    }
    Ok(())
}

/// Web permission names to `Browser.PermissionType`s.
fn protocol_permissions(permission: &str, fallback: bool) -> Option<Vec<&'static str>> {
    Some(match permission {
        "geolocation" => vec!["geolocation"],
        "midi" => vec!["midi"],
        "notifications" => vec!["notifications"],
        "camera" => vec!["videoCapture"],
        "microphone" => vec!["audioCapture"],
        "background-sync" => vec!["backgroundSync"],
        "ambient-light-sensor" | "accelerometer" | "gyroscope" | "magnetometer" => {
            vec!["sensors"]
        }
        "clipboard-read" => vec!["clipboardReadWrite"],
        "clipboard-write" => vec!["clipboardSanitizedWrite"],
        "payment-handler" => vec!["paymentHandler"],
        "midi-sysex" => vec!["midiSysex"],
        "storage-access" => vec!["storageAccess"],
        "local-fonts" => vec!["localFonts"],
        "local-network-access" if fallback => vec!["localNetworkAccess"],
        "local-network-access" => vec!["localNetworkAccess", "localNetwork", "loopbackNetwork"],
        "screen-wake-lock" => vec!["wakeLockScreen"],
        _ => return None,
    })
}

impl BrowserContext {
    async fn grant(&self, permissions: Vec<String>, origin: Option<String>) -> Result<()> {
        let resolved = match origin {
            Some(origin) => match url::Url::parse(&origin) {
                Ok(url) => url.origin().ascii_serialization(),
                Err(_) => return Err(Error::invalid_argument(format!("Invalid URL: {origin}"))),
            },
            None => "*".to_string(),
        };
        let list = {
            let mut map = self.inner.emu.permissions.lock().unwrap();
            let entry = map.entry(resolved.clone()).or_default();
            for p in permissions {
                if !entry.contains(&p) {
                    entry.push(p);
                }
            }
            entry.clone()
        };
        let mut last = None;
        for fallback in [false, true] {
            let mut protocol = Vec::new();
            for p in &list {
                match protocol_permissions(p, fallback) {
                    Some(names) => protocol.extend(names),
                    None => {
                        return Err(Error::invalid_argument(format!("Unknown permission: {p}")));
                    }
                }
            }
            let mut params = serde_json::Map::new();
            if resolved != "*" {
                params.insert("origin".into(), json!(resolved));
            }
            if let Some(id) = &self.inner.id {
                params.insert("browserContextId".into(), json!(id.as_ref()));
            }
            params.insert("permissions".into(), json!(protocol));
            match self
                .browser_command("Browser.grantPermissions", Value::Object(params))
                .await
            {
                Ok(_) => return Ok(()),
                Err(err) => last = Some(err),
            }
        }
        Err(last.unwrap_or_else(Error::target_closed))
    }

    /// Grant permissions (`"geolocation"`, `"notifications"`,
    /// `"clipboard-read"`, ...) to every origin, or to one origin with
    /// [`GrantPermissions::origin`].
    pub fn grant_permissions<I, S>(&self, permissions: I) -> GrantPermissions
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        GrantPermissions {
            context: self.clone(),
            permissions: permissions.into_iter().map(Into::into).collect(),
            origin: None,
        }
    }

    /// Remove all granted permissions.
    pub async fn clear_permissions(&self) -> Result<()> {
        async {
            self.inner.emu.permissions.lock().unwrap().clear();
            let mut params = serde_json::Map::new();
            if let Some(id) = &self.inner.id {
                params.insert("browserContextId".into(), json!(id.as_ref()));
            }
            self.browser_command("Browser.resetPermissions", Value::Object(params))
                .await?;
            Ok(())
        }
        .await
        .api("browserContext.clearPermissions")
    }

    /// Change (or with `None`, remove) the emulated geolocation of every page.
    /// Grant the `"geolocation"` permission for pages to read it.
    pub async fn set_geolocation(&self, geolocation: Option<Geolocation>) -> Result<()> {
        async {
            if let Some(g) = &geolocation {
                g.verify()?;
            }
            *self.inner.emu.geolocation.lock().unwrap() = geolocation;
            for page in self.pages() {
                update_geolocation(&page.inner, false).await?;
            }
            Ok(())
        }
        .await
        .api("browserContext.setGeolocation")
    }
}

/// Builder returned by [`BrowserContext::grant_permissions`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct GrantPermissions {
    context: BrowserContext,
    permissions: Vec<String>,
    origin: Option<String>,
}

impl GrantPermissions {
    /// Grant only to this origin, e.g. `https://example.com`.
    pub fn origin(mut self, origin: impl Into<String>) -> Self {
        self.origin = Some(origin.into());
        self
    }
}

impl IntoFuture for GrantPermissions {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            self.context
                .grant(self.permissions, self.origin)
                .await
                .api("browserContext.grantPermissions")
        })
    }
}

impl Page {
    /// Emulate media type and media features of the page. Options left unset
    /// keep their current value; `NoOverride` removes an override.
    ///
    /// ```no_run
    /// # async fn run(page: oxidewright::Page) -> oxidewright::Result<()> {
    /// use oxidewright::{ColorScheme, Media};
    /// page.emulate_media().media(Media::Print).color_scheme(ColorScheme::Dark).await?;
    /// # Ok(()) }
    /// ```
    pub fn emulate_media(&self) -> EmulateMedia {
        EmulateMedia {
            page: self.clone(),
            media: EmulatedMedia::default(),
        }
    }
}

/// Builder returned by [`Page::emulate_media`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct EmulateMedia {
    page: Page,
    media: EmulatedMedia,
}

impl EmulateMedia {
    /// CSS media type (`screen`, `print`).
    pub fn media(mut self, media: Media) -> Self {
        self.media.media = Some(media);
        self
    }

    /// `prefers-color-scheme`.
    pub fn color_scheme(mut self, value: ColorScheme) -> Self {
        self.media.color_scheme = Some(value);
        self
    }

    /// `prefers-reduced-motion`.
    pub fn reduced_motion(mut self, value: ReducedMotion) -> Self {
        self.media.reduced_motion = Some(value);
        self
    }

    /// `forced-colors`.
    pub fn forced_colors(mut self, value: ForcedColors) -> Self {
        self.media.forced_colors = Some(value);
        self
    }

    /// `prefers-contrast`.
    pub fn contrast(mut self, value: Contrast) -> Self {
        self.media.contrast = Some(value);
        self
    }
}

impl IntoFuture for EmulateMedia {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let inner = &self.page.inner;
            let old = *inner.emu.emulated_media.lock().unwrap();
            {
                let mut current = inner.emu.emulated_media.lock().unwrap();
                let new = self.media;
                if new.media.is_some() {
                    current.media = new.media;
                }
                if new.color_scheme.is_some() {
                    current.color_scheme = new.color_scheme;
                }
                if new.reduced_motion.is_some() {
                    current.reduced_motion = new.reduced_motion;
                }
                if new.forced_colors.is_some() {
                    current.forced_colors = new.forced_colors;
                }
                if new.contrast.is_some() {
                    current.contrast = new.contrast;
                }
            }
            let result = update_emulate_media(inner).await;
            if result.is_err() {
                *inner.emu.emulated_media.lock().unwrap() = old;
            }
            result.api("page.emulateMedia")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_metadata_matches_playwright() {
        let m = user_agent_metadata(
            "Mozilla/5.0 (iPhone; CPU iPhone OS 15_0 like Mac OS X) AppleWebKit/605.1.15",
            true,
        );
        assert_eq!(m["platform"], "iOS");
        assert_eq!(m["platformVersion"], "15_0");
        assert_eq!(m["architecture"], "arm");
        assert_eq!(m["mobile"], true);
        let m = user_agent_metadata(
            "Mozilla/5.0 (Linux; Android 12; Pixel 5) AppleWebKit/537.36",
            true,
        );
        assert_eq!(m["platform"], "Android");
        assert_eq!(m["platformVersion"], "12");
        let m = user_agent_metadata(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            false,
        );
        assert_eq!(m["platform"], "Windows");
        assert_eq!(m["platformVersion"], "10.0");
        assert_eq!(m["architecture"], "x86");
        let m = user_agent_metadata("Mozilla/5.0 (X11; Linux x86_64)", false);
        assert_eq!(m["platform"], "Linux");
    }

    #[test]
    fn geolocation_is_verified() {
        assert!(Geolocation::new(10.0, 10.0).verify().is_ok());
        let err = Geolocation::new(10.0, 200.0).verify().unwrap_err();
        assert!(err.to_string().contains("geolocation.longitude"));
        let err = Geolocation::new(100.0, 10.0).verify().unwrap_err();
        assert!(err.to_string().contains("geolocation.latitude"));
        let err = Geolocation::new(1.0, 1.0)
            .accuracy(-1.0)
            .verify()
            .unwrap_err();
        assert!(err.to_string().contains("geolocation.accuracy"));
    }
}
