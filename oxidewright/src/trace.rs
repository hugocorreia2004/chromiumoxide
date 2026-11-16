//! `context.tracing()`: traces in Playwright's trace format (a zip with
//! `trace.trace`, `trace.network` and resources) that the Playwright trace
//! viewer (`npx playwright show-trace`, trace.playwright.dev) can open.
//!
//! Recorded:
//! * the `context-options` header (browser, platform, viewport, user agent...);
//! * page screencast frames (`screenshots: true`), shown in the viewer's
//!   film strip;
//! * groups ([`Tracing::group`] / [`Tracing::group_end`]), shown as actions.
//!
//! **Not recorded (gap):** per-action `before`/`after` events with their
//! call logs, DOM snapshots (`snapshots: true` is accepted but ignored),
//! network entries (`trace.network` is empty), console messages and sources.
//! Playwright records those by instrumenting every protocol call of its
//! dispatcher layer; oxidewright has no such central instrumentation yet.

use std::future::{Future, IntoFuture};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::browser_context::BrowserContext;
use crate::error::{Error, Result};
use crate::page::Page;
use crate::screencast::{self, monotonic_time, wall_time};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Trace format version written (`trace.VERSION` of Playwright 1.65).
const TRACE_VERSION: u32 = 10;

#[derive(Default)]
struct ChunkData {
    lines: Vec<String>,
    files: Vec<(String, Arc<Vec<u8>>)>,
}

struct Chunk {
    data: Arc<Mutex<ChunkData>>,
    group_stack: Vec<String>,
}

#[derive(Default)]
struct Recording {
    started: bool,
    screenshots: bool,
    title: Option<String>,
    chunk: Option<Chunk>,
}

/// Tracing state of a context (`ContextExt.capture`).
#[derive(Default)]
pub(crate) struct ContextTracing {
    state: tokio::sync::Mutex<Recording>,
    /// Data of the running chunk, for pages created while recording.
    live: Mutex<Option<Arc<Mutex<ChunkData>>>>,
}

static CALL_ID: AtomicU64 = AtomicU64::new(1);

fn page_guid(page: &Page) -> String {
    format!("page@{}", page.inner.cdp.target_id().as_ref())
}

fn platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    }
}

/// Start appending the screencast frames of `page` to `data`.
async fn record_page(page: &Page, data: Arc<Mutex<ChunkData>>) {
    let Ok(mut client) = screencast::add_client(page, None, 90).await else {
        return;
    };
    let id = client.id;
    let page_id = page_guid(page);
    let task = tokio::spawn(async move {
        while let Some(frame) = client.frames.recv().await {
            let file = format!(
                "screencast/{}-{}.jpeg",
                page_id.replace(['@', ':'], "-"),
                frame.frame_swap_wall_time as u64
            );
            let event = json!({
                "type": "screencast-frame",
                "pageId": page_id,
                "file": file,
                "width": frame.viewport_width,
                "height": frame.viewport_height,
                "timestamp": monotonic_time(),
                "frameSwapWallTime": frame.frame_swap_wall_time,
            });
            let mut data = data.lock().unwrap();
            data.files.push((file, frame.data.clone()));
            data.lines.push(event.to_string());
        }
    });
    let previous = page
        .inner
        .ext
        .capture
        .tracing
        .lock()
        .unwrap()
        .replace((id, task));
    if let Some((old_id, old_task)) = previous {
        old_task.abort();
        screencast::remove_client(page, old_id).await;
    }
}

async fn stop_page(page: &Page) {
    let current = page.inner.ext.capture.tracing.lock().unwrap().take();
    if let Some((id, task)) = current {
        task.abort();
        screencast::remove_client(page, id).await;
    }
}

/// Called for each new page: record its screencast if a chunk with
/// screenshots is running.
pub(crate) async fn on_page_created(page: &Page) {
    let Some(context) = page.context() else {
        return;
    };
    let live = context.inner.ext.capture.live.lock().unwrap().clone();
    if let Some(data) = live {
        record_page(page, data).await;
    }
}

/// `context.tracing()`. Cheap to clone.
#[derive(Clone, Debug)]
pub struct Tracing {
    context: BrowserContext,
}

impl BrowserContext {
    /// The context's [`Tracing`]; see [`crate::trace`] for what is recorded.
    pub fn tracing(&self) -> Tracing {
        Tracing {
            context: self.clone(),
        }
    }
}

impl Tracing {
    fn state(&self) -> &ContextTracing {
        &self.context.inner.ext.capture
    }

    /// Start tracing (and the first chunk).
    ///
    /// ```no_run
    /// # async fn run(context: oxidewright::BrowserContext) -> oxidewright::Result<()> {
    /// context.tracing().start().screenshots(true).await?;
    /// // ...
    /// context.tracing().stop().path("trace.zip").await?;
    /// # Ok(()) }
    /// ```
    pub fn start(&self) -> TracingStart {
        TracingStart {
            tracing: self.clone(),
            name: None,
            title: None,
            screenshots: false,
            snapshots: false,
            sources: false,
            chunk_only: false,
        }
    }

    /// Start a new chunk (after [`Tracing::start`] and a
    /// [`Tracing::stop_chunk`]).
    pub fn start_chunk(&self) -> TracingStart {
        TracingStart {
            chunk_only: true,
            ..self.start()
        }
    }

    /// Stop the current chunk; with `.path(..)` it is saved as a zip.
    pub fn stop_chunk(&self) -> TracingStop {
        TracingStop {
            tracing: self.clone(),
            path: None,
            stop_tracing: false,
            api: "tracing.stopChunk",
        }
    }

    /// Stop tracing; with `.path(..)` the current chunk is saved as a zip.
    pub fn stop(&self) -> TracingStop {
        TracingStop {
            tracing: self.clone(),
            path: None,
            stop_tracing: true,
            api: "tracing.stop",
        }
    }

    /// Open a named group of actions in the trace (shown as an action in
    /// the viewer); close it with [`Tracing::group_end`].
    pub async fn group(&self, name: impl Into<String>) -> Result<()> {
        let mut state = self.state().state.lock().await;
        let Some(chunk) = state.chunk.as_mut() else {
            return Ok(());
        };
        let call_id = format!("call@{}", CALL_ID.fetch_add(1, Ordering::SeqCst));
        let mut event = json!({
            "type": "before",
            "callId": call_id,
            "startTime": monotonic_time(),
            "title": name.into(),
            "class": "Tracing",
            "method": "tracingGroup",
            "params": {},
            "stack": [],
        });
        if let Some(parent) = chunk.group_stack.last() {
            event["parentId"] = json!(parent);
        }
        chunk.group_stack.push(call_id);
        chunk.data.lock().unwrap().lines.push(event.to_string());
        Ok(())
    }

    /// Close the last group opened with [`Tracing::group`].
    pub async fn group_end(&self) -> Result<()> {
        let mut state = self.state().state.lock().await;
        if let Some(chunk) = state.chunk.as_mut() {
            close_group(chunk);
        }
        Ok(())
    }

    async fn start_impl(&self, start: &TracingStart) -> Result<()> {
        let mut state = self.state().state.lock().await;
        if !start.chunk_only {
            if state.started {
                return Err(Error::invalid_argument("Tracing has been already started"));
            }
            state.started = true;
            state.screenshots = start.screenshots;
            state.title = start.title.clone();
        } else if !state.started {
            return Err(Error::invalid_argument(
                "Must start tracing before starting a new chunk",
            ));
        }
        // A running chunk is discarded (`startChunk` while recording).
        state.chunk = None;
        let options = &self.context.inner.options;
        let user_agent = options.user_agent.clone();
        let header = json!({
            "version": TRACE_VERSION,
            "type": "context-options",
            "origin": "library",
            "browserName": "chromium",
            "playwrightVersion": "1.65.1",
            "options": {
                "baseURL": options.base_url,
                "viewport": options.viewport,
                "deviceScaleFactor": options.device_scale_factor,
                "isMobile": options.is_mobile,
                "userAgent": user_agent,
            },
            "platform": platform(),
            "wallTime": wall_time(),
            "monotonicTime": monotonic_time(),
            "sdkLanguage": "javascript",
            "title": start.title.clone().or_else(|| state.title.clone()),
        });
        let data = Arc::new(Mutex::new(ChunkData {
            lines: vec![strip_nulls(header).to_string()],
            files: Vec::new(),
        }));
        state.chunk = Some(Chunk {
            data: data.clone(),
            group_stack: Vec::new(),
        });
        if state.screenshots {
            *self.state().live.lock().unwrap() = Some(data.clone());
            for page in self.context.pages() {
                record_page(&page, data.clone()).await;
            }
        }
        Ok(())
    }

    async fn stop_impl(&self, path: Option<PathBuf>, stop_tracing: bool) -> Result<()> {
        let mut state = self.state().state.lock().await;
        if !state.started {
            // Stopping without exporting is a no-op (TR-027).
            if path.is_none() {
                return Ok(());
            }
            return Err(Error::invalid_argument(
                "Must start tracing before stopping",
            ));
        }
        let chunk = state.chunk.take();
        if stop_tracing {
            state.started = false;
        }
        drop(state);
        *self.state().live.lock().unwrap() = None;
        for page in self.context.pages() {
            stop_page(&page).await;
        }
        let Some(mut chunk) = chunk else {
            if stop_tracing {
                return Ok(());
            }
            return Err(Error::invalid_argument(
                "Must start tracing before stopping",
            ));
        };
        while !chunk.group_stack.is_empty() {
            close_group(&mut chunk);
        }
        let Some(path) = path else {
            return Ok(());
        };
        let data = std::mem::take(&mut *chunk.data.lock().unwrap());
        let mut trace = data.lines.join("\n");
        trace.push('\n');
        let mut entries: Vec<(String, Arc<Vec<u8>>)> = vec![
            ("trace.trace".into(), Arc::new(trace.into_bytes())),
            ("trace.network".into(), Arc::new(Vec::new())),
        ];
        entries.extend(data.files);
        let zip = zip_store(&entries);
        crate::storage_state::write_file(&path, &zip).await
    }
}

fn close_group(chunk: &mut Chunk) {
    if let Some(call_id) = chunk.group_stack.pop() {
        let event = json!({ "type": "after", "callId": call_id, "endTime": monotonic_time() });
        chunk.data.lock().unwrap().lines.push(event.to_string());
    }
}

fn strip_nulls(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, strip_nulls(v)))
                .collect(),
        ),
        other => other,
    }
}

/// Builder returned by [`Tracing::start`] / [`Tracing::start_chunk`].
#[must_use = "builders do nothing unless awaited"]
pub struct TracingStart {
    tracing: Tracing,
    name: Option<String>,
    title: Option<String>,
    screenshots: bool,
    snapshots: bool,
    sources: bool,
    chunk_only: bool,
}

impl TracingStart {
    /// Trace name (kept for API compatibility; the file name comes from
    /// `stop().path(..)`).
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Title shown in the trace viewer.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Record screencast frames (film strip).
    pub fn screenshots(mut self, screenshots: bool) -> Self {
        self.screenshots = screenshots;
        self
    }

    /// DOM snapshots: accepted, not recorded (see [`crate::trace`]).
    pub fn snapshots(mut self, snapshots: bool) -> Self {
        self.snapshots = snapshots;
        self
    }

    /// Source files: accepted, not recorded (see [`crate::trace`]).
    pub fn sources(mut self, sources: bool) -> Self {
        self.sources = sources;
        self
    }
}

impl IntoFuture for TracingStart {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let api = if self.chunk_only {
                "tracing.startChunk"
            } else {
                "tracing.start"
            };
            let _ = (&self.name, self.snapshots, self.sources);
            self.tracing
                .start_impl(&self)
                .await
                .map_err(|e| e.with_api(api))
        })
    }
}

/// Builder returned by [`Tracing::stop`] / [`Tracing::stop_chunk`].
#[must_use = "builders do nothing unless awaited"]
pub struct TracingStop {
    tracing: Tracing,
    path: Option<PathBuf>,
    stop_tracing: bool,
    api: &'static str,
}

impl TracingStop {
    /// Save the trace zip to this file.
    pub fn path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }
}

impl IntoFuture for TracingStop {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            self.tracing
                .stop_impl(self.path, self.stop_tracing)
                .await
                .map_err(|e| e.with_api(self.api))
        })
    }
}

// ----------------------------------------------------------------------
// Minimal zip writer (stored entries; JPEG frames do not compress anyway)
// ----------------------------------------------------------------------

fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (i, entry) in table.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *entry = c;
        }
        table
    });
    let mut crc = 0xFFFF_FFFFu32;
    for b in data {
        crc = table[((crc ^ *b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

/// A zip archive with the given entries stored uncompressed.
pub(crate) fn zip_store(entries: &[(String, Arc<Vec<u8>>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    let put16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
    let put32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
    // 1980-01-01 00:00 in DOS format.
    let (dos_time, dos_date) = (0u16, (1 << 5) | 1);
    for (name, data) in entries {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let size = data.len() as u32;
        let name = name.as_bytes();
        put32(&mut out, 0x0403_4b50);
        put16(&mut out, 20);
        put16(&mut out, 0x0800); // UTF-8 names
        put16(&mut out, 0); // stored
        put16(&mut out, dos_time);
        put16(&mut out, dos_date);
        put32(&mut out, crc);
        put32(&mut out, size);
        put32(&mut out, size);
        put16(&mut out, name.len() as u16);
        put16(&mut out, 0);
        out.extend_from_slice(name);
        out.extend_from_slice(data);

        put32(&mut central, 0x0201_4b50);
        put16(&mut central, 20);
        put16(&mut central, 20);
        put16(&mut central, 0x0800);
        put16(&mut central, 0);
        put16(&mut central, dos_time);
        put16(&mut central, dos_date);
        put32(&mut central, crc);
        put32(&mut central, size);
        put32(&mut central, size);
        put16(&mut central, name.len() as u16);
        put16(&mut central, 0);
        put16(&mut central, 0);
        put16(&mut central, 0);
        put16(&mut central, 0);
        put32(&mut central, 0);
        put32(&mut central, offset);
        central.extend_from_slice(name);
    }
    let central_offset = out.len() as u32;
    let central_size = central.len() as u32;
    out.extend_from_slice(&central);
    put32(&mut out, 0x0605_4b50);
    put16(&mut out, 0);
    put16(&mut out, 0);
    put16(&mut out, entries.len() as u16);
    put16(&mut out, entries.len() as u16);
    put32(&mut out, central_size);
    put32(&mut out, central_offset);
    put16(&mut out, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_reference() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn zip_layout() {
        let zip = zip_store(&[
            ("a.txt".into(), Arc::new(b"hello".to_vec())),
            ("b/c.bin".into(), Arc::new(vec![1, 2, 3])),
        ]);
        assert_eq!(&zip[..4], &[0x50, 0x4b, 0x03, 0x04]);
        let eocd = zip.len() - 22;
        assert_eq!(&zip[eocd..eocd + 4], &[0x50, 0x4b, 0x05, 0x06]);
        assert_eq!(u16::from_le_bytes([zip[eocd + 10], zip[eocd + 11]]), 2);
        assert!(zip.windows(5).any(|w| w == b"hello"));
    }

    #[test]
    fn strips_null_options() {
        let v = strip_nulls(json!({ "a": null, "b": { "c": null, "d": 1 } }));
        assert_eq!(v, json!({ "b": { "d": 1 } }));
    }
}
