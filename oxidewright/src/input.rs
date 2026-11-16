//! Keyboard, mouse and touchscreen input (Playwright's `server/input.ts`
//! and `server/chromium/crInput.ts`).

use std::sync::Arc;

/// Input state of a page (pressed keys/modifiers, mouse position/buttons).
pub(crate) struct InputState {
    #[allow(dead_code)]
    session: chromiumoxide::Page,
}

impl InputState {
    pub(crate) fn new(session: chromiumoxide::Page) -> Arc<Self> {
        Arc::new(InputState { session })
    }
}
