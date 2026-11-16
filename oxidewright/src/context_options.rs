//! Builder methods of [`NewContext`] for every context option, and the
//! device registry ([`devices`]).

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::browser_context::{NewContext, NewContextOptions};
use crate::emulation::{
    ColorScheme, Contrast, ForcedColors, Geolocation, ReducedMotion, ServiceWorkerPolicy,
};
use crate::page::ViewportSize;
use crate::storage_state::StorageStateSource;

/// A device descriptor (Playwright's `devices['iPhone 13']`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    /// User agent of the device's browser.
    pub user_agent: String,
    /// Viewport size.
    pub viewport: ViewportSize,
    /// Screen size, when different from the viewport.
    #[serde(default)]
    pub screen: Option<ViewportSize>,
    /// Device scale factor.
    pub device_scale_factor: f64,
    /// Mobile device.
    pub is_mobile: bool,
    /// Touch support.
    pub has_touch: bool,
    /// `chromium`, `firefox` or `webkit`.
    pub default_browser_type: String,
}

/// The device registry, keyed by name (`"iPhone 13"`, `"Pixel 7"`,
/// `"Desktop Chrome"`, ...). Ported from Playwright's
/// `deviceDescriptorsSource.json`.
///
/// ```
/// let iphone = &oxidewright::devices()["iPhone 13"];
/// assert!(iphone.is_mobile);
/// ```
pub fn devices() -> &'static HashMap<String, Device> {
    static DEVICES: OnceLock<HashMap<String, Device>> = OnceLock::new();
    DEVICES.get_or_init(|| {
        serde_json::from_str(include_str!("devices.json")).expect("valid device descriptors")
    })
}

impl NewContextOptions {
    /// Apply a device descriptor: user agent, viewport, screen, device scale
    /// factor, `is_mobile` and `has_touch`.
    pub fn apply_device(&mut self, device: &Device) {
        self.user_agent = Some(device.user_agent.clone());
        self.viewport = Some(device.viewport);
        self.screen = device.screen;
        self.device_scale_factor = Some(device.device_scale_factor);
        self.is_mobile = Some(device.is_mobile);
        self.has_touch = Some(device.has_touch);
    }
}

impl From<&Device> for NewContextOptions {
    fn from(device: &Device) -> Self {
        let mut options = NewContextOptions::default();
        options.apply_device(device);
        options
    }
}

impl NewContext {
    /// Emulate a device (see [`devices`]); later builder calls override
    /// individual options.
    ///
    /// ```no_run
    /// # async fn run(browser: oxidewright::Browser) -> oxidewright::Result<()> {
    /// let context = browser
    ///     .new_context()
    ///     .device(&oxidewright::devices()["iPhone 13"])
    ///     .locale("pt-PT")
    ///     .await?;
    /// # let _ = context; Ok(()) }
    /// ```
    pub fn device(mut self, device: &Device) -> Self {
        self.options.apply_device(device);
        self
    }

    /// Screen size (`window.screen`); requires a viewport.
    pub fn screen(mut self, screen: impl Into<ViewportSize>) -> Self {
        self.options.screen = Some(screen.into());
        self
    }

    /// Device scale factor (`window.devicePixelRatio`).
    pub fn device_scale_factor(mut self, factor: f64) -> Self {
        self.options.device_scale_factor = Some(factor);
        self
    }

    /// Emulate a mobile device.
    pub fn is_mobile(mut self, is_mobile: bool) -> Self {
        self.options.is_mobile = Some(is_mobile);
        self
    }

    /// Emulate touch support.
    pub fn has_touch(mut self, has_touch: bool) -> Self {
        self.options.has_touch = Some(has_touch);
        self
    }

    /// User agent override.
    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.options.user_agent = Some(user_agent.into());
        self
    }

    /// Locale, e.g. `en-GB`.
    pub fn locale(mut self, locale: impl Into<String>) -> Self {
        self.options.locale = Some(locale.into());
        self
    }

    /// Timezone, e.g. `America/New_York`.
    pub fn timezone_id(mut self, timezone_id: impl Into<String>) -> Self {
        self.options.timezone_id = Some(timezone_id.into());
        self
    }

    /// Emulated geolocation.
    pub fn geolocation(mut self, geolocation: Geolocation) -> Self {
        self.options.geolocation = Some(geolocation);
        self
    }

    /// Permissions granted to every origin.
    pub fn permissions<I, S>(mut self, permissions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.options.permissions = Some(permissions.into_iter().map(Into::into).collect());
        self
    }

    /// `prefers-color-scheme`.
    pub fn color_scheme(mut self, value: ColorScheme) -> Self {
        self.options.color_scheme = Some(value);
        self
    }

    /// `prefers-reduced-motion`.
    pub fn reduced_motion(mut self, value: ReducedMotion) -> Self {
        self.options.reduced_motion = Some(value);
        self
    }

    /// `forced-colors`.
    pub fn forced_colors(mut self, value: ForcedColors) -> Self {
        self.options.forced_colors = Some(value);
        self
    }

    /// `prefers-contrast`.
    pub fn contrast(mut self, value: Contrast) -> Self {
        self.options.contrast = Some(value);
        self
    }

    /// Enable or disable JavaScript.
    pub fn java_script_enabled(mut self, enabled: bool) -> Self {
        self.options.java_script_enabled = Some(enabled);
        self
    }

    /// Bypass Content-Security-Policy.
    pub fn bypass_csp(mut self, bypass: bool) -> Self {
        self.options.bypass_csp = bypass;
        self
    }

    /// Make locators strict by default.
    pub fn strict_selectors(mut self, strict: bool) -> Self {
        self.options.strict_selectors = strict;
        self
    }

    /// Allow or block service workers.
    pub fn service_workers(mut self, policy: ServiceWorkerPolicy) -> Self {
        self.options.service_workers = policy;
        self
    }

    /// Initial cookies and storage: a [`crate::StorageState`] or the path of
    /// a storage state file.
    pub fn storage_state(mut self, state: impl Into<StorageStateSource>) -> Self {
        self.options.storage_state = Some(state.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_registry() {
        let devices = devices();
        assert!(devices.len() > 100);
        let iphone = &devices["iPhone 13"];
        assert_eq!(iphone.viewport, ViewportSize::from((390, 664)));
        assert_eq!(iphone.device_scale_factor, 3.0);
        assert!(iphone.is_mobile && iphone.has_touch);
        assert_eq!(iphone.default_browser_type, "webkit");
        let desktop = &devices["Desktop Chrome"];
        assert!(!desktop.is_mobile);
        assert!(desktop.screen.is_some());
        let options = NewContextOptions::from(iphone);
        assert_eq!(
            options.user_agent.as_deref(),
            Some(iphone.user_agent.as_str())
        );
    }
}
