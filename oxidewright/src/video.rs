//! Video recording (`recordVideo` context option, `page.video()`), like
//! Playwright's `server/videoRecorder.ts`: screencast JPEG frames are piped
//! to `ffmpeg`, which encodes a VP8 WebM file at 25 fps.
//!
//! Playwright bundles its own ffmpeg build; oxidewright uses the binary named
//! by the `OXIDEWRIGHT_FFMPEG_PATH` environment variable, or `ffmpeg` from
//! `PATH` (it must include the `libvpx` encoder). Creating a context with
//! `record_video_dir` fails when no ffmpeg is found.
//!
//! The file is finalized when the page (or its context) is closed.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use tokio::io::AsyncWriteExt;
use tokio::sync::{oneshot, watch};

use crate::browser_context::{NewContext, NewContextOptions};
use crate::error::{Error, Result};
use crate::page::{Page, ViewportSize};
use crate::screencast::{self, wall_time};

const FPS: f64 = 25.0;

/// Environment variable naming the ffmpeg binary.
pub const FFMPEG_ENV: &str = "OXIDEWRIGHT_FFMPEG_PATH";

/// Find the ffmpeg binary: `OXIDEWRIGHT_FFMPEG_PATH`, else `ffmpeg` on `PATH`.
pub fn ffmpeg_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(FFMPEG_ENV) {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let names: &[&str] = if cfg!(windows) {
        &["ffmpeg.exe", "ffmpeg.cmd", "ffmpeg.bat", "ffmpeg"]
    } else {
        &["ffmpeg"]
    };
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn require_ffmpeg() -> Result<PathBuf> {
    ffmpeg_path().ok_or_else(|| {
        Error::launch(format!(
            "ffmpeg not found: video recording needs an ffmpeg binary with the libvpx encoder; install it on PATH or set {FFMPEG_ENV}"
        ))
    })
}

/// Video options of a context.
pub(crate) fn validate_options(options: &NewContextOptions) -> Result<()> {
    if options.record_video_dir.is_some() {
        require_ffmpeg()?;
    }
    Ok(())
}

impl NewContext {
    /// Record a video of every page into `dir` (WebM files with random
    /// names). Requires ffmpeg, see [`crate::video`].
    pub fn record_video_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.options.record_video_dir = Some(dir.into());
        self
    }

    /// Size of the recorded videos (default: the viewport scaled down to
    /// fit 800x800). Frames are padded or cropped to this size.
    pub fn record_video_size(mut self, size: impl Into<ViewportSize>) -> Self {
        self.options.record_video_size = Some(size.into());
        self
    }
}

struct VideoInner {
    path: PathBuf,
    finished: watch::Receiver<Option<std::result::Result<(), String>>>,
    stop: Mutex<Option<oneshot::Sender<()>>>,
}

/// The video of a page (`page.video()`). Cheap to clone.
#[derive(Clone)]
pub struct Video {
    inner: Arc<VideoInner>,
}

impl std::fmt::Debug for Video {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Video")
            .field("path", &self.inner.path)
            .finish()
    }
}

impl Video {
    /// Path of the video file; it is complete once the page is closed.
    pub fn path(&self) -> PathBuf {
        self.inner.path.clone()
    }

    /// Wait until the recording is finalized (the page was closed).
    async fn wait_finished(&self) -> Result<()> {
        let mut rx = self.inner.finished.clone();
        let result = rx
            .wait_for(Option::is_some)
            .await
            .map_err(|_| Error::target_closed())?
            .clone();
        match result {
            Some(Err(message)) => Err(Error::launch(message)),
            _ => Ok(()),
        }
    }

    /// Copy the video to `path` once the page is closed (waits for it).
    pub async fn save_as(&self, path: impl AsRef<Path>) -> Result<()> {
        self.wait_finished().await?;
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
        }
        tokio::fs::copy(&self.inner.path, path)
            .await
            .map(|_| ())
            .map_err(|e| Error::invalid_argument(format!("{}: {e}", path.display())))
    }

    /// Delete the video file once the page is closed (waits for it).
    pub async fn delete(&self) -> Result<()> {
        self.wait_finished().await?;
        let _ = tokio::fs::remove_file(&self.inner.path).await;
        Ok(())
    }

    /// Finalize the recording now.
    pub(crate) async fn stop(&self) {
        if let Some(stop) = self.inner.stop.lock().unwrap().take() {
            let _ = stop.send(());
        }
        let _ = self.wait_finished().await;
    }
}

impl Page {
    /// The video being recorded for this page, when the context was created
    /// with `record_video_dir`.
    pub fn video(&self) -> Option<Video> {
        self.inner.emu.capture.video.lock().unwrap().clone()
    }
}

/// `-vf pad=W:H:0:0:gray,crop=W:H:0:0` and the other arguments of
/// Playwright's (VP8) ffmpeg recorder.
pub(crate) fn ffmpeg_args(size: ViewportSize, output: &Path) -> Vec<String> {
    let (w, h) = (size.width, size.height);
    let mut args: Vec<String> = format!(
        "-loglevel error -f image2pipe -avioflags direct -fpsprobesize 0 -probesize 32 -analyzeduration 0 -c:v mjpeg -i pipe:0 -y -an -r {FPS} -c:v vp8 -qmin 0 -qmax 50 -crf 8 -deadline realtime -speed 8 -b:v 1M -threads 1 -vf pad={w}:{h}:0:0:gray,crop={w}:{h}:0:0"
    )
    .split(' ')
    .map(str::to_string)
    .collect();
    args.push(output.to_string_lossy().into_owned());
    args
}

/// How many times a frame shown from `from` to `to` (milliseconds) is
/// written at [`FPS`] (at least once).
pub(crate) fn repeat_count(from_ms: f64, to_ms: f64) -> usize {
    ((FPS * (to_ms - from_ms) / 1000.0).round() as i64).max(1) as usize
}

/// Start recording a new page when its context records videos.
pub(crate) async fn start_for_page(page: &Page) -> Result<()> {
    let Some(context) = page.context() else {
        return Ok(());
    };
    let Some(dir) = context.inner.options.record_video_dir.clone() else {
        return Ok(());
    };
    require_ffmpeg()?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| Error::invalid_argument(format!("{}: {e}", dir.display())))?;
    let name: String = std::iter::repeat_with(fastrand::alphanumeric)
        .take(32)
        .collect();
    let path = dir.join(format!("{}.webm", name.to_lowercase()));
    let size = context
        .inner
        .options
        .record_video_size
        .unwrap_or_else(|| screencast::default_size(page.viewport_size()));
    let size = ViewportSize {
        width: size.width & !1,
        height: size.height & !1,
    };
    let video = record(page, size, path).await?;
    *page.inner.emu.capture.video.lock().unwrap() = Some(video);
    Ok(())
}

/// Record the page's screencast to a webm file at `path` with ffmpeg until
/// the returned video is stopped (`Video::stop`) or the page closes.
pub(crate) async fn record(page: &Page, size: ViewportSize, path: PathBuf) -> Result<Video> {
    let ffmpeg = require_ffmpeg()?;
    let size = ViewportSize {
        width: size.width & !1,
        height: size.height & !1,
    };
    let mut command = tokio::process::Command::new(&ffmpeg);
    command
        .args(ffmpeg_args(size, &path))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = command
        .spawn()
        .map_err(|e| Error::launch(format!("Failed to launch ffmpeg: {e}")))?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let mut client = match screencast::add_client(page, Some(size), 90).await {
        Ok(client) => client,
        Err(err) => {
            let _ = child.kill().await;
            return Err(err);
        }
    };
    let (stop_tx, mut stop_rx) = oneshot::channel();
    let (done_tx, done_rx) = watch::channel(None);
    let page_for_task = page.clone();
    tokio::spawn(async move {
        let mut last: Option<(Arc<Vec<u8>>, f64)> = None;
        let mut write_error = None;
        loop {
            let frame = tokio::select! {
                frame = client.frames.recv() => frame,
                _ = &mut stop_rx => None,
            };
            let Some(frame) = frame else {
                break;
            };
            if let Some((buffer, at)) = &last {
                for _ in 0..repeat_count(*at, frame.frame_swap_wall_time) {
                    if let Err(e) = stdin.write_all(buffer).await {
                        write_error = Some(e.to_string());
                    }
                }
            }
            last = Some((frame.data.clone(), frame.frame_swap_wall_time));
        }
        screencast::remove_client(&page_for_task, client.id).await;
        if last.is_none() && !page_for_task.is_closed() {
            // No frame was produced (e.g. a static blank page): use a
            // screenshot so the video is never empty.
            if let Ok(shot) = page_for_task
                .inner
                .cdp
                .execute(crate::cdp_raw::RawCommand::new(
                    "Page.captureScreenshot",
                    serde_json::json!({ "format": "jpeg", "quality": 90 }),
                ))
                .await
            {
                use base64::Engine;
                if let Some(data) = shot.result.get("data").and_then(|d| d.as_str()) {
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) {
                        last = Some((Arc::new(bytes), wall_time()));
                    }
                }
            }
        }
        let had_frames = last.is_some();
        if let Some((buffer, at)) = &last {
            for _ in 0..repeat_count(*at, wall_time()) {
                if let Err(e) = stdin.write_all(buffer).await {
                    write_error = Some(e.to_string());
                }
            }
        }
        let _ = stdin.flush().await;
        drop(stdin);
        let output = child.wait_with_output().await;
        let result = match output {
            Ok(out) if out.status.success() || !had_frames => Ok(()),
            Ok(out) => Err(format!(
                "ffmpeg failed ({}): {}{}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim(),
                write_error.map(|e| format!(" ({e})")).unwrap_or_default()
            )),
            Err(e) => Err(format!("ffmpeg failed: {e}")),
        };
        if let Err(message) = &result {
            tracing::debug!("video recording failed: {message}");
        }
        let _ = done_tx.send(Some(result));
    });
    Ok(Video {
        inner: Arc::new(VideoInner {
            path,
            finished: done_rx,
            stop: Mutex::new(Some(stop_tx)),
        }),
    })
}

/// Finalize the page's video (page or context closing).
pub(crate) async fn stop_for_page(page: &Page) {
    let video = page.video();
    if let Some(video) = video {
        video.stop().await;
    }
}

/// Finalize the videos of every page of a closing context.
pub(crate) async fn on_context_closing(context: &crate::BrowserContext) {
    for page in context.pages() {
        crate::screencast::on_page_closing(&page).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_arguments() {
        let args = ffmpeg_args(
            ViewportSize {
                width: 320,
                height: 240,
            },
            Path::new("out.webm"),
        );
        assert!(args.contains(&"pad=320:240:0:0:gray,crop=320:240:0:0".to_string()));
        assert_eq!(args.last().unwrap(), "out.webm");
        assert_eq!(args[args.iter().position(|a| a == "-r").unwrap() + 1], "25");
    }

    #[test]
    fn frame_repetition() {
        assert_eq!(repeat_count(0.0, 0.0), 1);
        assert_eq!(repeat_count(0.0, 1000.0), 25);
        assert_eq!(repeat_count(0.0, 100.0), 3);
    }
}
