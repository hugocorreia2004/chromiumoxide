//! Page screencast shared by tracing and video recording (Playwright's
//! `server/screencast.ts` and `CRPage.startScreencast`).
//!
//! Frames come from `Page.startScreencast` (JPEG) and are acknowledged with
//! `Page.screencastFrameAck`. The screencast is driven by the compositor
//! through the DevTools protocol; it is not observable by page scripts.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use chromiumoxide::cdp::browser_protocol::page::EventScreencastFrame;
use futures::StreamExt;
use serde_json::json;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::cdp_raw::RawCommand;
use crate::error::Result;
use crate::page::{Page, ViewportSize};

/// One JPEG frame of the screencast.
#[derive(Clone, Debug)]
pub(crate) struct ScreencastFrame {
    /// JPEG bytes.
    pub(crate) data: Arc<Vec<u8>>,
    /// Viewport size reported by the frame metadata (CSS pixels).
    pub(crate) viewport_width: u32,
    pub(crate) viewport_height: u32,
    /// Wall time of the frame swap, in milliseconds since the epoch.
    pub(crate) frame_swap_wall_time: f64,
}

/// Milliseconds since the Unix epoch.
pub(crate) fn wall_time() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Playwright's `monotonicTime()`: milliseconds on a monotonic clock, with
/// microsecond precision.
pub(crate) fn monotonic_time() -> f64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_micros() as f64 / 1000.0
}

/// Default screencast size: the viewport scaled down to fit 800x800, with
/// even dimensions (`Screencast._startScreencast`).
pub(crate) fn default_size(viewport: Option<ViewportSize>) -> ViewportSize {
    let viewport = viewport.unwrap_or(ViewportSize {
        width: 800,
        height: 600,
    });
    let scale = (800.0 / viewport.width.max(viewport.height).max(1) as f64).min(1.0);
    ViewportSize {
        width: ((viewport.width as f64 * scale).floor() as u32) & !1,
        height: ((viewport.height as f64 * scale).floor() as u32) & !1,
    }
}

/// Capture state of a page (`PageExt.capture`).
#[derive(Default)]
pub(crate) struct PageCapture {
    pub(crate) screencast: PageScreencast,
    pub(crate) video: Mutex<Option<crate::video::Video>>,
    pub(crate) tracing: Mutex<Option<(u64, tokio::task::JoinHandle<()>)>>,
}

struct Running {
    size: ViewportSize,
    task: tokio::task::JoinHandle<()>,
}

/// Screencast state of a page: subscribers and the running screencast.
#[derive(Default)]
pub(crate) struct PageScreencast {
    clients: Arc<Mutex<HashMap<u64, UnboundedSender<ScreencastFrame>>>>,
    running: Mutex<Option<Running>>,
    next_id: AtomicU64,
}

/// A subscription to the frames of a page; unsubscribe with
/// [`remove_client`].
pub(crate) struct ScreencastClient {
    pub(crate) id: u64,
    pub(crate) frames: UnboundedReceiver<ScreencastFrame>,
}

/// Subscribe to the page's frames, starting the screencast for the first
/// subscriber with `size` (default: the viewport fitted into 800x800).
pub(crate) async fn add_client(
    page: &Page,
    size: Option<ViewportSize>,
    quality: u32,
) -> Result<ScreencastClient> {
    let state = &page.inner.ext.capture.screencast;
    let (tx, rx) = unbounded_channel();
    let id = state.next_id.fetch_add(1, Ordering::SeqCst);
    state.clients.lock().unwrap().insert(id, tx);
    let existing = state.running.lock().unwrap().as_ref().map(|r| r.size);
    if existing.is_some() {
        return Ok(ScreencastClient { id, frames: rx });
    }
    let size = size.unwrap_or_else(|| default_size(page.viewport_size()));
    let size = ViewportSize {
        width: size.width & !1,
        height: size.height & !1,
    };
    let mut events = match page
        .inner
        .cdp
        .event_listener::<EventScreencastFrame>()
        .await
    {
        Ok(events) => events,
        Err(err) => {
            state.clients.lock().unwrap().remove(&id);
            return Err(err.into());
        }
    };
    let clients = state.clients.clone();
    let cdp = page.inner.cdp.clone();
    let task = tokio::spawn(async move {
        while let Some(event) = events.next().await {
            let data = base64::engine::general_purpose::STANDARD
                .decode(AsRef::<str>::as_ref(&event.data))
                .unwrap_or_default();
            let wall = event
                .metadata
                .timestamp
                .as_ref()
                .map(|t| *t.inner() * 1000.0)
                .unwrap_or_else(wall_time);
            let frame = ScreencastFrame {
                data: Arc::new(data),
                viewport_width: event.metadata.device_width.round() as u32,
                viewport_height: event.metadata.device_height.round() as u32,
                frame_swap_wall_time: wall,
            };
            clients
                .lock()
                .unwrap()
                .retain(|_, tx| tx.send(frame.clone()).is_ok());
            let _ = cdp
                .execute(RawCommand::new(
                    "Page.screencastFrameAck",
                    json!({ "sessionId": event.session_id }),
                ))
                .await;
        }
        // The page went away: subscribers see the end of their stream.
        clients.lock().unwrap().clear();
    });
    *state.running.lock().unwrap() = Some(Running { size, task });
    let started = page
        .inner
        .cdp
        .execute(RawCommand::new(
            "Page.startScreencast",
            json!({
                "format": "jpeg",
                "quality": quality,
                "maxWidth": size.width,
                "maxHeight": size.height,
                "maxFramesInFlight": 5,
            }),
        ))
        .await;
    if let Err(err) = started {
        remove_client(page, id).await;
        return Err(err.into());
    }
    Ok(ScreencastClient { id, frames: rx })
}

/// Unsubscribe; the screencast stops with the last subscriber.
pub(crate) async fn remove_client(page: &Page, id: u64) {
    let state = &page.inner.ext.capture.screencast;
    let empty = {
        let mut clients = state.clients.lock().unwrap();
        clients.remove(&id);
        clients.is_empty()
    };
    if !empty {
        return;
    }
    let running = state.running.lock().unwrap().take();
    if let Some(running) = running {
        running.task.abort();
        if !page.is_closed() {
            let _ = page
                .inner
                .cdp
                .execute(RawCommand::new("Page.stopScreencast", json!({})))
                .await;
        }
    }
}

/// Called for every new page of a context: starts video recording and
/// tracing screenshots when enabled.
pub(crate) async fn on_page_created(page: &Page) -> Result<()> {
    crate::video::start_for_page(page).await?;
    crate::trace::on_page_created(page).await;
    Ok(())
}

/// Called before a page or context closes: finalizes videos.
pub(crate) async fn on_page_closing(page: &Page) {
    crate::video::stop_for_page(page).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_size_fits_800() {
        assert_eq!(
            default_size(Some(ViewportSize {
                width: 1280,
                height: 720
            })),
            ViewportSize {
                width: 800,
                height: 450
            }
        );
        assert_eq!(
            default_size(None),
            ViewportSize {
                width: 800,
                height: 600
            }
        );
        assert_eq!(
            default_size(Some(ViewportSize {
                width: 333,
                height: 101
            })),
            ViewportSize {
                width: 332,
                height: 100
            }
        );
        let a = monotonic_time();
        assert!(monotonic_time() >= a);
    }
}
