//! JavaScript dialogs (`alert`, `confirm`, `prompt`, `beforeunload`).
//!
//! Like Playwright, a dialog is dismissed automatically (accepted for
//! `beforeunload`) when neither the page nor its context listens for the
//! `dialog` event; otherwise a handler must accept or dismiss it, or the page
//! stays blocked.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use chromiumoxide::cdp::browser_protocol::page::{
    DialogType, EventJavascriptDialogOpening, HandleJavaScriptDialogParams,
};
use chromiumoxide::cdp::browser_protocol::target::SessionId;

use crate::error::{Error, Result, ResultExt};
use crate::events::{Event, EventKind};
use crate::network_manager::{emit_on_context, send};
use crate::page::{Page, PageInner};

/// A JavaScript dialog. Cheap to clone.
#[derive(Clone)]
pub struct Dialog {
    inner: Arc<DialogInner>,
}

struct DialogInner {
    page: Weak<PageInner>,
    session: SessionId,
    kind: String,
    message: String,
    default_value: String,
    handled: AtomicBool,
    closed: AtomicBool,
}

impl fmt::Debug for Dialog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dialog")
            .field("type", &self.inner.kind)
            .field("message", &self.inner.message)
            .finish()
    }
}

impl PartialEq for Dialog {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Dialog {
    /// `alert`, `beforeunload`, `confirm` or `prompt`.
    pub fn r#type(&self) -> &str {
        &self.inner.kind
    }

    /// The message shown in the dialog.
    pub fn message(&self) -> &str {
        &self.inner.message
    }

    /// The default value of a `prompt`, empty otherwise.
    pub fn default_value(&self) -> &str {
        &self.inner.default_value
    }

    /// The page that opened the dialog.
    pub fn page(&self) -> Option<Page> {
        self.inner.page.upgrade().map(Page::from_inner)
    }

    async fn handle(&self, accept: bool, prompt_text: Option<String>, api: &str) -> Result<()> {
        if self.inner.handled.swap(true, Ordering::SeqCst) {
            let verb = if accept { "accept" } else { "dismiss" };
            return Err(Error::invalid_argument(format!(
                "Cannot {verb} dialog which is already handled!"
            ))
            .with_api(api));
        }
        let page = self.inner.page.upgrade().ok_or_else(Error::target_closed)?;
        let mut params = HandleJavaScriptDialogParams::new(accept);
        params.prompt_text = prompt_text;
        send(&page, Some(self.inner.session.clone()), params)
            .await
            .map(|_| ())
            .api(api)?;
        self.did_close(&page);
        Ok(())
    }

    /// Emit `dialogclosed` once (after accept/dismiss, or when the browser
    /// closed the dialog).
    fn did_close(&self, page: &PageInner) {
        if self.inner.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        page.ext.dialogs.lock().unwrap().retain(|d| d != self);
        emit_on_context(page, Event::DialogClosed(self.clone()));
    }

    /// Accept the dialog, with the text to enter in a `prompt`.
    pub async fn accept(&self, prompt_text: Option<&str>) -> Result<()> {
        self.handle(true, prompt_text.map(str::to_string), "dialog.accept")
            .await
    }

    /// Dismiss the dialog.
    pub async fn dismiss(&self) -> Result<()> {
        self.handle(false, None, "dialog.dismiss").await
    }
}

/// `Page.javascriptDialogOpening`.
pub(crate) fn on_dialog_opening(page: &PageInner, ev: &EventJavascriptDialogOpening) {
    let kind = match ev.r#type {
        DialogType::Alert => "alert",
        DialogType::Confirm => "confirm",
        DialogType::Prompt => "prompt",
        DialogType::Beforeunload => "beforeunload",
    };
    let session = page
        .frame_by_id(ev.frame_id.as_ref())
        .and_then(|f| f.cdp_frame_now())
        .map(|f| f.session_id().clone())
        .unwrap_or_else(|| page.cdp.session_id().clone());
    let dialog = Dialog {
        inner: Arc::new(DialogInner {
            page: page.self_weak.clone(),
            session,
            kind: kind.to_string(),
            message: ev.message.clone(),
            default_value: ev.default_prompt.clone().unwrap_or_default(),
            handled: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        }),
    };
    let listeners = page.ext.listeners.count(EventKind::Dialog)
        + page
            .context
            .upgrade()
            .map(|c| c.ext.listeners.count(EventKind::Dialog))
            .unwrap_or(0);
    if listeners == 0 {
        let dialog = dialog.clone();
        tokio::spawn(async move {
            let result = if dialog.r#type() == "beforeunload" {
                dialog.accept(None).await
            } else {
                dialog.dismiss().await
            };
            if let Err(err) = result {
                tracing::debug!("failed to close a dialog: {err}");
            }
        });
        return;
    }
    page.ext.dialogs.lock().unwrap().push(dialog.clone());
    emit_on_context(page, Event::Dialog(dialog));
}

/// `Page.javascriptDialogClosed`.
pub(crate) fn on_dialog_closed(page: &PageInner) {
    let dialogs = std::mem::take(&mut *page.ext.dialogs.lock().unwrap());
    for dialog in dialogs {
        // Closed in the browser (possibly before `accept`/`dismiss` returned).
        dialog.did_close(page);
    }
}

impl Page {
    /// `dialogclosed`: a dialog of the page was accepted or dismissed
    /// (including the ones dismissed automatically).
    pub fn on_dialog_closed(
        &self,
        handler: impl Fn(Dialog) + Send + Sync + 'static,
    ) -> crate::events::ListenerId {
        self.on(EventKind::DialogClosed, move |event| {
            if let Event::DialogClosed(dialog) = event {
                handler(dialog)
            }
        })
    }

    /// Wait for the next `dialogclosed` event of the page.
    pub fn wait_for_dialog_closed(&self) -> crate::events::WaitForEvent<Dialog> {
        crate::events::WaitForEvent::new(
            crate::events::Emitter::Page(self.clone()),
            EventKind::DialogClosed,
            "page.waitForEvent",
            crate::events::as_dialog_closed,
        )
    }
}

impl crate::BrowserContext {
    /// `dialogclosed`: a dialog of a page of the context was accepted or
    /// dismissed (including the ones dismissed automatically).
    pub fn on_dialog_closed(
        &self,
        handler: impl Fn(Dialog) + Send + Sync + 'static,
    ) -> crate::events::ListenerId {
        self.on(EventKind::DialogClosed, move |event| {
            if let Event::DialogClosed(dialog) = event {
                handler(dialog)
            }
        })
    }

    /// Wait for the next `dialogclosed` event of the context.
    pub fn wait_for_dialog_closed(&self) -> crate::events::WaitForEvent<Dialog> {
        crate::events::WaitForEvent::new(
            crate::events::Emitter::Context(self.clone()),
            EventKind::DialogClosed,
            "browserContext.waitForEvent",
            crate::events::as_dialog_closed,
        )
    }
}
