//! `page.screencast` (Playwright 1.65's `Screencast`): stream the page's
//! frames to a callback and/or record them to a video file. Frames come
//! from the compositor through the DevTools protocol, invisible to the page.

use std::future::{Future, IntoFuture};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::page::{Page, ViewportSize};
use crate::screencast;

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
type FrameHandler = Arc<dyn Fn(ScreencastFrame) + Send + Sync>;

/// A frame of [`Screencast`].
#[derive(Clone, Debug)]
pub struct ScreencastFrame {
    /// JPEG image.
    pub data: Arc<Vec<u8>>,
    /// Wall time of the frame, in milliseconds since the epoch.
    pub timestamp: f64,
    /// Viewport width in CSS pixels.
    pub viewport_width: u32,
    /// Viewport height in CSS pixels.
    pub viewport_height: u32,
}

/// The screencast running for a page.
pub(crate) struct Running {
    client: Option<(u64, tokio::task::JoinHandle<()>)>,
    video: Option<crate::video::Video>,
}

/// Screencast of a page (`page.screencast`). Cheap to clone.
#[derive(Clone, Debug)]
pub struct Screencast {
    page: Page,
}

impl Page {
    /// The page's [`Screencast`].
    pub fn screencast(&self) -> Screencast {
        Screencast { page: self.clone() }
    }
}

impl Screencast {
    /// Start streaming frames ([`StartScreencast::on_frame`]) and/or
    /// recording a video ([`StartScreencast::path`], needs ffmpeg).
    pub fn start(&self) -> StartScreencast {
        StartScreencast {
            page: self.page.clone(),
            on_frame: None,
            path: None,
            size: None,
            quality: None,
            fps: None,
        }
    }

    /// Stop the screencast; a recorded video is finalized when this returns.
    pub async fn stop(&self) -> Result<()> {
        let running = self.page.inner.emu.capture.public.lock().unwrap().take();
        let Some(running) = running else {
            return Ok(());
        };
        if let Some((id, task)) = running.client {
            task.abort();
            screencast::remove_client(&self.page, id).await;
        }
        if let Some(video) = running.video {
            video.stop().await;
        }
        if self.page.is_closed() {
            return Err(Error::target_closed().with_api("screencast.stop"));
        }
        Ok(())
    }
}

/// Builder returned by [`Screencast::start`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct StartScreencast {
    page: Page,
    on_frame: Option<FrameHandler>,
    path: Option<PathBuf>,
    size: Option<ViewportSize>,
    quality: Option<u32>,
    fps: Option<f64>,
}

impl StartScreencast {
    /// Call `handler` with every frame.
    pub fn on_frame(mut self, handler: impl Fn(ScreencastFrame) + Send + Sync + 'static) -> Self {
        self.on_frame = Some(Arc::new(handler));
        self
    }

    /// Record a webm video to this file (needs ffmpeg).
    pub fn path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Frame size (default: the viewport fitted into 800x800).
    pub fn size(mut self, size: ViewportSize) -> Self {
        self.size = Some(size);
        self
    }

    /// JPEG quality of the frames, 0-100 (default 90).
    pub fn quality(mut self, quality: u32) -> Self {
        self.quality = Some(quality.min(100));
        self
    }

    /// Maximum frames per second passed to `on_frame`.
    pub fn fps(mut self, fps: f64) -> Self {
        self.fps = Some(fps);
        self
    }
}

impl IntoFuture for StartScreencast {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            if let Some(fps) = self.fps {
                if fps <= 0.0 || !fps.is_finite() {
                    return Err(Error::invalid_argument(format!(
                        "\"fps\" must be a positive number, got {fps}"
                    ))
                    .with_api("screencast.start"));
                }
            }
            let state = &self.page.inner.emu.capture.public;
            if state.lock().unwrap().is_some() {
                return Err(Error::invalid_argument("Screencast is already started")
                    .with_api("screencast.start"));
            }
            let size = self
                .size
                .unwrap_or_else(|| screencast::default_size(self.page.viewport_size()));
            let mut running = Running {
                client: None,
                video: None,
            };
            if let Some(handler) = self.on_frame.clone() {
                let mut client =
                    screencast::add_client(&self.page, Some(size), self.quality.unwrap_or(90))
                        .await
                        .map_err(|e| e.with_api("screencast.start"))?;
                let min_gap = self.fps.filter(|f| *f > 0.0).map(|f| 1000.0 / f);
                let task = tokio::spawn(async move {
                    let mut last: Option<f64> = None;
                    while let Some(frame) = client.frames.recv().await {
                        if let (Some(gap), Some(last)) = (min_gap, last) {
                            if frame.frame_swap_wall_time - last < gap {
                                continue;
                            }
                        }
                        last = Some(frame.frame_swap_wall_time);
                        handler(ScreencastFrame {
                            data: frame.data.clone(),
                            timestamp: frame.frame_swap_wall_time,
                            viewport_width: frame.viewport_width,
                            viewport_height: frame.viewport_height,
                        });
                    }
                });
                running.client = Some((client.id, task));
            }
            if let Some(path) = self.path.clone() {
                if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(|e| Error::invalid_argument(e.to_string()))?;
                }
                match crate::video::record(&self.page, size, path).await {
                    Ok(video) => running.video = Some(video),
                    Err(err) => {
                        if let Some((id, task)) = running.client.take() {
                            task.abort();
                            screencast::remove_client(&self.page, id).await;
                        }
                        return Err(err.with_api("screencast.start"));
                    }
                }
            }
            *state.lock().unwrap() = Some(running);
            Ok(())
        })
    }
}
