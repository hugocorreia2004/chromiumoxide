//! Selector engine registry (`playwright.selectors`).

use std::sync::RwLock;

static TEST_ID_ATTRIBUTE: RwLock<Option<String>> = RwLock::new(None);

/// The attribute used by `get_by_test_id` (default `data-testid`).
pub(crate) fn test_id_attribute_name() -> String {
    TEST_ID_ATTRIBUTE
        .read()
        .unwrap()
        .clone()
        .unwrap_or_else(|| "data-testid".to_string())
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
}
