//! HAR recording (`record_har` context option, `route_from_har` with
//! `update`) and replay (`route_from_har`), ports of Playwright's
//! `server/har/harTracer.ts`, `harRecorder.ts` and `server/harBackend.ts`.
//!
//! Zipped HAR archives (`.zip` paths) are not supported.

use std::collections::HashMap;
use std::future::{Future, IntoFuture};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use base64::Engine;
use serde_json::{Value, json};

use crate::browser_context::BrowserContext;
use crate::error::{Error, Result, ResultExt};
use crate::events::Event;
use crate::network::{Header, Request, header_get};
use crate::page::{Page, PageInner};
use crate::route::{Route, RouteHandler, RouteTarget, boxed_handler, validate_glob};
use crate::url_match::UrlMatch;

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// How response bodies are stored in a recorded HAR.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HarContentPolicy {
    /// Do not store bodies.
    Omit,
    /// Store bodies in the HAR (text, or base64 for binary content).
    #[default]
    Embed,
    /// Store bodies as separate files next to the HAR, named by their SHA-1.
    Attach,
}

/// How much detail a recorded HAR has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HarMode {
    /// Everything.
    #[default]
    Full,
    /// Only what is needed to replay with `route_from_har` (no cookies,
    /// timings, sizes, security details, server IPs or pages).
    Minimal,
}

/// The `record_har` context option.
#[derive(Clone, Debug)]
pub struct RecordHar {
    /// Where to write the HAR when the context closes.
    pub path: PathBuf,
    /// Body storage (default [`HarContentPolicy::Embed`]).
    pub content: HarContentPolicy,
    /// Detail level (default [`HarMode::Full`]).
    pub mode: HarMode,
    /// Only record requests whose URL matches.
    pub url_filter: Option<UrlMatch>,
}

impl RecordHar {
    /// Record into `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        RecordHar {
            path: path.into(),
            content: HarContentPolicy::Embed,
            mode: HarMode::Full,
            url_filter: None,
        }
    }

    /// Body storage.
    pub fn content(mut self, content: HarContentPolicy) -> Self {
        self.content = content;
        self
    }

    /// Detail level.
    pub fn mode(mut self, mode: HarMode) -> Self {
        self.mode = mode;
        self
    }

    /// Only record requests whose URL matches.
    pub fn url_filter(mut self, url: impl Into<UrlMatch>) -> Self {
        self.url_filter = Some(url.into());
        self
    }
}

static PAGE_SEQ: AtomicU64 = AtomicU64::new(1);

struct PageEntry {
    page: Weak<PageInner>,
    value: Value,
    started_ms: f64,
}

#[derive(Default)]
struct RecorderState {
    entries: Vec<Arc<Mutex<Value>>>,
    by_request: HashMap<usize, Arc<Mutex<Value>>>,
    pages: Vec<PageEntry>,
    page_ids: HashMap<usize, String>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    written: std::collections::HashSet<String>,
    flushed: bool,
    requests: Vec<Request>,
    finished: std::collections::HashSet<usize>,
}

/// Records the traffic of a context (or of one page) into a HAR file.
pub(crate) struct HarRecorder {
    options: RecordHar,
    base_url: Option<String>,
    page: Option<Weak<PageInner>>,
    browser_version: String,
    state: Mutex<RecorderState>,
}

fn slim(mode: HarMode) -> bool {
    mode == HarMode::Minimal
}

fn iso(ms: f64) -> String {
    let secs = (ms / 1000.0).floor();
    let millis = (ms - secs * 1000.0).round() as i64;
    let days = (secs / 86400.0).floor() as i64;
    let rem = secs as i64 - days * 86400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Civil date from days since epoch (Howard Hinnant's algorithm).
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}.{millis:03}Z")
}

fn now_ms() -> f64 {
    crate::network_manager::wall_now() * 1000.0
}

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn headers_json(headers: &[Header]) -> Value {
    Value::Array(
        headers
            .iter()
            .map(|h| json!({"name": h.name, "value": h.value}))
            .collect(),
    )
}

fn parse_cookie(c: &str) -> Value {
    let mut parts = c.split(';').map(str::trim);
    let name_value = parts.next().unwrap_or("");
    let (name, value) = match name_value.find('=') {
        Some(i) => (&name_value[..i], &name_value[i + 1..]),
        None => ("", name_value),
    };
    let mut cookie = json!({"name": name.trim(), "value": value.trim()});
    for pair in parts {
        let (n, v) = match pair.find('=') {
            Some(i) => (pair[..i].trim(), pair[i + 1..].trim()),
            None => (pair.trim(), ""),
        };
        match n.to_lowercase().as_str() {
            "domain" => cookie["domain"] = json!(v),
            "path" => cookie["path"] = json!(v),
            "httponly" => cookie["httpOnly"] = json!(true),
            "secure" => cookie["secure"] = json!(true),
            "samesite" => cookie["sameSite"] = json!(v),
            "expires" => cookie["expires"] = json!(v),
            _ => {}
        }
    }
    cookie
}

fn is_textual(mime: &str) -> bool {
    regex::Regex::new(r"^(text/.*?|application/(json|(x-)?javascript|xml.*?|ecmascript|graphql|x-www-form-urlencoded)|image/svg(\+xml)?|application/.*?(\+json|\+xml))(;\s*charset=.*)?$")
        .map(|re| re.is_match(mime))
        .unwrap_or(false)
}

fn extension_for(mime: &str) -> &'static str {
    let mime = mime.split(';').next().unwrap_or("").trim();
    match mime {
        "text/html" => "html",
        "text/css" => "css",
        "text/plain" => "txt",
        "application/javascript" | "text/javascript" => "js",
        "application/json" => "json",
        "image/png" => "png",
        "image/jpeg" => "jpeg",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        "image/webp" => "webp",
        "font/woff2" => "woff2",
        "font/woff" => "woff",
        "application/x-www-form-urlencoded" => "dat",
        _ => "dat",
    }
}

fn request_key(request: &Request) -> usize {
    Arc::as_ptr(&request.inner) as usize
}

impl HarRecorder {
    pub(crate) fn new(
        options: RecordHar,
        base_url: Option<String>,
        page: Option<Weak<PageInner>>,
        browser_version: String,
    ) -> Result<Self> {
        if options.path.extension().is_some_and(|e| e == "zip") {
            return Err(Error::invalid_argument(
                "Zipped HAR files are not supported, use a .har path",
            ));
        }
        Ok(HarRecorder {
            options,
            base_url,
            page,
            browser_version,
            state: Mutex::new(RecorderState::default()),
        })
    }

    fn includes(&self, url: &str) -> bool {
        self.options
            .url_filter
            .as_ref()
            .is_none_or(|f| f.matches(self.base_url.as_deref(), url))
    }

    fn page_ref(&self, state: &mut RecorderState, page: &PageInner) -> Option<String> {
        if slim(self.options.mode) {
            return None;
        }
        let key = page as *const PageInner as usize;
        if let Some(id) = state.page_ids.get(&key) {
            return Some(id.clone());
        }
        let id = format!("page@{}", PAGE_SEQ.fetch_add(1, Ordering::SeqCst));
        let started = now_ms();
        state.pages.push(PageEntry {
            page: page.self_weak.clone(),
            value: json!({
                "startedDateTime": iso(started),
                "id": id,
                "title": "",
                "pageTimings": { "onContentLoad": -1, "onLoad": -1 },
            }),
            started_ms: started,
        });
        state.page_ids.insert(key, id.clone());
        Some(id)
    }

    fn content_blob(&self, state: &mut RecorderState, name: &str, data: &[u8]) -> String {
        if state.written.insert(name.to_string()) {
            let dir = self
                .options
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            let _ = std::fs::create_dir_all(&dir);
            let path = dir.join(name);
            if !path.exists() {
                let _ = std::fs::write(path, data);
            }
        }
        name.to_string()
    }

    fn post_data(&self, state: &mut RecorderState, request: &Request) -> Option<Value> {
        let data = request.post_data_buffer()?;
        let content_type = header_get(&request.headers_array_now(), "content-type")
            .unwrap_or_else(|| "application/octet-stream".into());
        let mut result = json!({"mimeType": content_type, "text": "", "params": []});
        if self.options.content == HarContentPolicy::Embed
            && content_type != "application/octet-stream"
        {
            result["text"] = json!(String::from_utf8_lossy(&data));
        }
        if self.options.content == HarContentPolicy::Attach {
            let name = format!(
                "{}.{}",
                sha1_smol::Sha1::from(&data).digest(),
                extension_for(&content_type)
            );
            result["_file"] = json!(self.content_blob(state, &name, &data));
        }
        if content_type == "application/x-www-form-urlencoded" {
            let params: Vec<Value> = url::form_urlencoded::parse(&data)
                .map(|(k, v)| json!({"name": k, "value": v}))
                .collect();
            result["params"] = Value::Array(params);
        }
        Some(result)
    }

    fn record_request_headers(&self, entry: &mut Value, headers: &[Header]) {
        if !slim(self.options.mode) {
            let cookies: Vec<Value> = headers
                .iter()
                .filter(|h| h.name.eq_ignore_ascii_case("cookie"))
                .flat_map(|h| h.value.split(';').map(parse_cookie).collect::<Vec<_>>())
                .collect();
            entry["request"]["cookies"] = Value::Array(cookies);
        }
        entry["request"]["headers"] = headers_json(headers);
    }

    fn record_response_headers(&self, entry: &mut Value, headers: &[Header]) {
        if !slim(self.options.mode) {
            let cookies: Vec<Value> = headers
                .iter()
                .filter(|h| h.name.eq_ignore_ascii_case("set-cookie"))
                .map(|h| parse_cookie(&h.value))
                .collect();
            entry["response"]["cookies"] = Value::Array(cookies);
        }
        entry["response"]["headers"] = headers_json(headers);
        if let Some(ct) = header_get(headers, "content-type") {
            entry["response"]["content"]["mimeType"] = json!(ct);
        }
    }

    fn compute_time(entry: &mut Value) {
        let mut total = 0.0;
        for key in ["dns", "connect", "ssl", "wait", "receive"] {
            if let Some(v) = entry["timings"][key].as_f64() {
                if v > 0.0 {
                    total += v;
                }
            }
        }
        entry["time"] = json!(total);
    }

    fn spawn(
        self: &Arc<Self>,
        state: &mut RecorderState,
        fut: impl Future<Output = ()> + Send + 'static,
    ) {
        state.tasks.push(tokio::spawn(fut));
    }

    /// A network event of the context.
    pub(crate) fn on_event(self: &Arc<Self>, page: &PageInner, event: &Event) {
        if let Some(only) = &self.page {
            if !std::ptr::eq(only.as_ptr(), page) {
                return;
            }
        }
        let mut state = self.state.lock().unwrap();
        if state.flushed {
            return;
        }
        match event {
            Event::Request(request) => self.on_request(&mut state, page, request),
            Event::Response(response) => {
                let request = response.request();
                let Some(entry) = state.by_request.get(&request_key(&request)).cloned() else {
                    return;
                };
                self.on_response(&mut state, &entry, &request);
            }
            Event::RequestFinished(request) => {
                if !state.finished.insert(request_key(request)) {
                    return;
                }
                let Some(entry) = state.by_request.get(&request_key(request)).cloned() else {
                    return;
                };
                self.on_request_finished(&mut state, &entry, request);
            }
            Event::RequestFailed(request) => {
                let Some(entry) = state.by_request.get(&request_key(request)).cloned() else {
                    return;
                };
                let mut e = entry.lock().unwrap();
                if let Some(failure) = request.failure() {
                    e["response"]["_failureText"] = json!(failure);
                }
            }
            _ => {}
        }
    }

    fn on_request(
        self: &Arc<Self>,
        state: &mut RecorderState,
        page: &PageInner,
        request: &Request,
    ) {
        let url = request.url();
        if !self.includes(&url) {
            return;
        }
        let Ok(parsed) = url::Url::parse(&url) else {
            return;
        };
        let page_ref = self.page_ref(state, page);
        let started = request.inner.wall_time * 1000.0;
        let query: Vec<Value> = parsed
            .query_pairs()
            .map(|(k, v)| json!({"name": k, "value": v}))
            .collect();
        let mut entry = json!({
            "startedDateTime": iso(if started > 0.0 { started } else { now_ms() }),
            "time": -1,
            "request": {
                "method": request.method(),
                "url": parsed.to_string(),
                "httpVersion": "HTTP/1.1",
                "cookies": [],
                "headers": [],
                "queryString": query,
                "headersSize": -1,
                "bodySize": -1,
            },
            "response": {
                "status": -1,
                "statusText": "",
                "httpVersion": "HTTP/1.1",
                "cookies": [],
                "headers": [],
                "content": {"size": -1, "mimeType": "x-unknown"},
                "headersSize": -1,
                "bodySize": -1,
                "redirectURL": "",
            },
            "cache": {},
            "timings": {"send": -1, "wait": -1, "receive": -1},
            "_resourceType": request.resource_type(),
        });
        if let Some(page_ref) = page_ref {
            entry["pageref"] = json!(page_ref);
        }
        if let Some(frame) = request.frame() {
            entry["_frameref"] = json!(format!("frame@{}", frame.id()));
        }
        if !slim(self.options.mode) {
            entry["response"]["_transferSize"] = json!(-1);
        }
        self.record_request_headers(&mut entry, &request.headers_array_now());
        if let Some(post) = self.post_data(state, request) {
            entry["request"]["postData"] = post;
        }
        if !slim(self.options.mode) {
            entry["request"]["bodySize"] = json!(request.body_size());
        }
        if let Some(from) = request.redirected_from() {
            if let Some(from_entry) = state.by_request.get(&request_key(&from)) {
                from_entry.lock().unwrap()["response"]["redirectURL"] = json!(url);
            }
        }
        let entry = Arc::new(Mutex::new(entry));
        state.entries.push(entry.clone());
        state.by_request.insert(request_key(request), entry);
        state.requests.push(request.clone());
    }

    fn on_response(
        self: &Arc<Self>,
        state: &mut RecorderState,
        entry: &Arc<Mutex<Value>>,
        request: &Request,
    ) {
        let Some(response) = request.existing_response() else {
            return;
        };
        {
            let mut e = entry.lock().unwrap();
            e["response"] = json!({
                "status": response.status(),
                "statusText": response.status_text(),
                "httpVersion": "HTTP/1.1",
                "cookies": [],
                "headers": [],
                "content": {"size": -1, "mimeType": "x-unknown"},
                "headersSize": -1,
                "bodySize": -1,
                "redirectURL": "",
            });
            if !slim(self.options.mode) {
                e["response"]["_transferSize"] = json!(-1);
                let t = request.timing();
                let ms =
                    |a: f64, b: f64, cond: bool| if cond { json!(round(a - b)) } else { json!(-1) };
                e["timings"] = json!({
                    "dns": ms(t.domain_lookup_end, t.domain_lookup_start, t.domain_lookup_end != -1.0),
                    "connect": ms(t.connect_end, t.connect_start, t.connect_end != -1.0),
                    "ssl": ms(t.connect_end, t.secure_connection_start, t.connect_end != -1.0),
                    "send": 0,
                    "wait": ms(t.response_start, t.request_start, t.response_start != -1.0),
                    "receive": -1,
                });
                Self::compute_time(&mut e);
            }
            let overrides = request.inner.state.lock().unwrap().overrides.clone();
            if overrides.url.is_some() || overrides.method.is_some() || overrides.headers.is_some()
            {
                e["request"]["method"] = json!(request.method());
                e["request"]["url"] = json!(request.url());
            }
            let headers = response.headers_array_now();
            self.record_response_headers(&mut e, &headers);
        }
        let this = self.clone();
        let entry2 = entry.clone();
        let req = request.clone();
        self.spawn(state, async move {
            if let Ok(headers) = req.headers_array().await {
                this.record_request_headers(&mut entry2.lock().unwrap(), &headers);
            }
        });
        let this = self.clone();
        let entry2 = entry.clone();
        self.spawn(state, async move {
            if let Ok(headers) = response.headers_array().await {
                this.record_response_headers(&mut entry2.lock().unwrap(), &headers);
            }
        });
    }

    fn on_request_finished(
        self: &Arc<Self>,
        state: &mut RecorderState,
        entry: &Arc<Mutex<Value>>,
        request: &Request,
    ) {
        let Some(response) = request.existing_response() else {
            return;
        };
        let full = !slim(self.options.mode);
        {
            let mut e = entry.lock().unwrap();
            let t = request.timing();
            if full {
                e["timings"]["receive"] = if t.response_end != -1.0 {
                    json!(round(t.response_end - t.response_start))
                } else {
                    json!(-1)
                };
                Self::compute_time(&mut e);
            }
        }
        let this = self.clone();
        let entry2 = entry.clone();
        let content_policy = self.options.content;
        self.spawn(state, async move {
            if full {
                if let Ok(Some(addr)) = response.server_addr().await {
                    let mut e = entry2.lock().unwrap();
                    e["serverIPAddress"] = json!(addr.ip_address);
                    e["_serverPort"] = json!(addr.port);
                }
                if let Ok(Some(details)) = response.security_details().await {
                    entry2.lock().unwrap()["_securityDetails"] =
                        serde_json::to_value(details).unwrap_or(Value::Null);
                }
            }
            if let Ok(version) = response.http_version().await {
                let mut e = entry2.lock().unwrap();
                e["request"]["httpVersion"] = json!(version);
                e["response"]["httpVersion"] = json!(version);
            }
            let body = response.body().await.unwrap_or_default();
            let mime = entry2.lock().unwrap()["response"]["content"]["mimeType"]
                .as_str()
                .unwrap_or("x-unknown")
                .to_string();
            {
                let mut e = entry2.lock().unwrap();
                if full {
                    e["response"]["content"]["size"] = json!(body.len());
                }
                match content_policy {
                    HarContentPolicy::Embed => {
                        match (is_textual(&mime), std::str::from_utf8(&body)) {
                            (true, Ok(text)) => e["response"]["content"]["text"] = json!(text),
                            _ => {
                                e["response"]["content"]["text"] =
                                    json!(base64::engine::general_purpose::STANDARD.encode(&body));
                                e["response"]["content"]["encoding"] = json!("base64");
                            }
                        }
                    }
                    HarContentPolicy::Attach => {}
                    HarContentPolicy::Omit => {}
                }
            }
            if content_policy == HarContentPolicy::Attach {
                let name = format!(
                    "{}.{}",
                    sha1_smol::Sha1::from(&body).digest(),
                    extension_for(&mime)
                );
                let file = {
                    let mut state = this.state.lock().unwrap();
                    this.content_blob(&mut state, &name, &body)
                };
                entry2.lock().unwrap()["response"]["content"]["_file"] = json!(file);
            }
            if full {
                if let Ok(sizes) = response.sizes().await {
                    let mut e = entry2.lock().unwrap();
                    e["response"]["bodySize"] = json!(sizes.response_body_size);
                    e["response"]["headersSize"] = json!(sizes.response_headers_size);
                    e["response"]["_transferSize"] =
                        json!(sizes.response_headers_size + sizes.response_body_size);
                    e["request"]["headersSize"] = json!(sizes.request_headers_size);
                    let size = e["response"]["content"]["size"].as_i64().unwrap_or(0);
                    e["response"]["content"]["compression"] =
                        json!((size - sizes.response_body_size as i64).max(0));
                }
            }
        });
    }

    /// `domcontentloaded`/`load` of a page: title and page timings.
    pub(crate) fn on_page_lifecycle(self: &Arc<Self>, page: &Page, load: bool) {
        if slim(self.options.mode) {
            return;
        }
        if let Some(only) = &self.page {
            if !std::ptr::eq(only.as_ptr(), Arc::as_ptr(&page.inner)) {
                return;
            }
        }
        let mut state = self.state.lock().unwrap();
        if state.flushed {
            return;
        }
        self.page_ref(&mut state, &page.inner);
        let this = self.clone();
        let page = page.clone();
        self.spawn(&mut state, async move {
            let script = if load {
                "() => ({ title: document.title, time: performance.timing.loadEventStart })"
            } else {
                "() => ({ title: document.title, time: performance.timing.domContentLoadedEventStart })"
            };
            let result: Result<Value> = page
                .main_frame()
                .evaluate(script)
                .isolated_context(true)
                .await;
            let Ok(result) = result else {
                return;
            };
            let mut state = this.state.lock().unwrap();
            let key = Arc::as_ptr(&page.inner) as usize;
            let Some(id) = state.page_ids.get(&key).cloned() else {
                return;
            };
            if let Some(entry) = state.pages.iter_mut().find(|p| p.value["id"] == json!(id)) {
                if let Some(title) = result["title"].as_str() {
                    entry.value["title"] = json!(title);
                }
                if let Some(time) = result["time"].as_f64() {
                    let key = if load { "onLoad" } else { "onContentLoad" };
                    entry.value["pageTimings"][key] = json!(round(time - entry.started_ms).max(-1.0));
                }
            }
        });
    }

    /// Wait for pending work and write the HAR file.
    pub(crate) async fn flush(self: &Arc<Self>) -> Result<()> {
        // Requests that got a response but whose `requestfinished` was not
        // processed yet: wait a little for them, then record their content.
        let pending: Vec<Request> = {
            let state = self.state.lock().unwrap();
            state
                .requests
                .iter()
                .filter(|r| !state.finished.contains(&request_key(r)))
                .filter(|r| r.existing_response().is_some())
                .cloned()
                .collect()
        };
        for request in pending {
            if let Some(response) = request.existing_response() {
                let _ =
                    tokio::time::timeout(std::time::Duration::from_secs(2), response.finished())
                        .await;
            }
            let mut state = self.state.lock().unwrap();
            if request.failure().is_some() || !state.finished.insert(request_key(&request)) {
                continue;
            }
            if let Some(entry) = state.by_request.get(&request_key(&request)).cloned() {
                self.on_request_finished(&mut state, &entry, &request);
            }
        }
        // Page titles and timings of the pages that are still open (their
        // lifecycle events may not have been processed yet).
        let pages: Vec<(String, Weak<PageInner>, f64)> = {
            let state = self.state.lock().unwrap();
            state
                .pages
                .iter()
                .map(|p| {
                    (
                        p.value["id"].as_str().unwrap_or_default().to_string(),
                        p.page.clone(),
                        p.started_ms,
                    )
                })
                .collect()
        };
        for (id, page, started) in pages {
            let Some(page) = page.upgrade().filter(|p| !p.is_closed()) else {
                continue;
            };
            let page = Page::from_inner(page);
            let eval = page
                .main_frame()
                .evaluate::<Value>(
                    "() => ({ title: document.title, dcl: performance.timing.domContentLoadedEventStart, load: performance.timing.loadEventStart })",
                )
                .isolated_context(true);
            let Ok(Ok(result)) =
                tokio::time::timeout(std::time::Duration::from_secs(2), eval.into_future()).await
            else {
                continue;
            };
            let mut state = self.state.lock().unwrap();
            if let Some(entry) = state.pages.iter_mut().find(|p| p.value["id"] == json!(id)) {
                if entry.value["title"] == json!("") {
                    if let Some(title) = result["title"].as_str() {
                        entry.value["title"] = json!(title);
                    }
                }
                for (key, field) in [("onContentLoad", "dcl"), ("onLoad", "load")] {
                    let current = entry.value["pageTimings"][key].as_f64().unwrap_or(-1.0);
                    if let Some(time) = result[field].as_f64().filter(|t| *t > 0.0) {
                        if current < 0.0 {
                            entry.value["pageTimings"][key] = json!(round(time - started).max(0.0));
                        }
                    }
                }
            }
        }
        let tasks = {
            let mut state = self.state.lock().unwrap();
            if state.flushed {
                return Ok(());
            }
            std::mem::take(&mut state.tasks)
        };
        for task in tasks {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(10), task).await;
        }
        let (entries, pages) = {
            let mut state = self.state.lock().unwrap();
            state.flushed = true;
            let entries: Vec<Value> = state
                .entries
                .iter()
                .map(|e| e.lock().unwrap().clone())
                .collect();
            let pages: Vec<Value> = state
                .pages
                .iter()
                .map(|p| {
                    let _ = &p.page;
                    p.value.clone()
                })
                .collect();
            (entries, pages)
        };
        let mut log = json!({
            "version": "1.2",
            "creator": {"name": "oxidewright", "version": env!("CARGO_PKG_VERSION")},
            "browser": {"name": "chromium", "version": self.browser_version},
        });
        if !pages.is_empty() {
            log["pages"] = Value::Array(pages);
        }
        log["entries"] = Value::Array(entries);
        let har = json!({ "log": log });
        if let Some(parent) = self.options.path.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| Error::invalid_argument(e.to_string()))?;
            }
        }
        let text = serde_json::to_string_pretty(&har)?;
        tokio::fs::write(&self.options.path, text)
            .await
            .map_err(|e| Error::invalid_argument(format!("{}: {e}", self.options.path.display())))
    }
}

/// Flush the HAR recorders of a closing context.
pub(crate) async fn flush_context(context: &BrowserContext) -> Result<()> {
    let recorders: Vec<Arc<HarRecorder>> =
        std::mem::take(&mut *context.inner.ext.har_recorders.lock().unwrap());
    for recorder in recorders {
        recorder.flush().await?;
    }
    Ok(())
}

/// Start recording for a new context with the `record_har` option.
pub(crate) fn start_context_recording(context: &BrowserContext) -> Result<()> {
    if let Some(options) = context.inner.options.record_har.clone() {
        let version = context
            .browser()
            .map(|b| b.version().to_string())
            .unwrap_or_default();
        let recorder = HarRecorder::new(
            options,
            context.inner.options.base_url.clone(),
            None,
            version,
        )?;
        context
            .inner
            .ext
            .har_recorders
            .lock()
            .unwrap()
            .push(Arc::new(recorder));
    }
    Ok(())
}

// ----------------------------------------------------------------------
// Replay
// ----------------------------------------------------------------------

/// What to do with requests not found in the HAR.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HarNotFound {
    /// Abort them (default).
    #[default]
    Abort,
    /// Pass them to the next handler / the network.
    Fallback,
}

struct HarBackend {
    entries: Vec<Value>,
    base_dir: PathBuf,
}

enum Lookup {
    Fulfill {
        status: i64,
        headers: Vec<Header>,
        body: Vec<u8>,
    },
    Redirect(String),
    NotFound,
    Error(String),
}

fn har_headers(value: &Value) -> Vec<Header> {
    value
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|h| Some(Header::new(h["name"].as_str()?, h["value"].as_str()?)))
                .collect()
        })
        .unwrap_or_default()
}

fn multipart_boundary(headers: &[Header]) -> Option<String> {
    let ct = header_get(headers, "content-type")?;
    if !ct.contains("multipart/form-data") {
        return None;
    }
    let re = regex::Regex::new(r"boundary=(\S+)").ok()?;
    re.captures(&ct).map(|c| c[1].to_string())
}

impl HarBackend {
    fn load_content(&self, content: &Value) -> Result<Vec<u8>> {
        if let Some(file) = content["_file"].as_str() {
            let resolved = self.base_dir.join(file);
            let canonical_base = self
                .base_dir
                .canonicalize()
                .unwrap_or(self.base_dir.clone());
            let canonical = resolved
                .canonicalize()
                .map_err(|e| Error::invalid_argument(e.to_string()))?;
            if !canonical.starts_with(&canonical_base) {
                return Err(Error::invalid_argument(format!(
                    "HAR entry _file escapes base directory: {file}"
                )));
            }
            return std::fs::read(canonical).map_err(|e| Error::invalid_argument(e.to_string()));
        }
        let text = content["text"].as_str().unwrap_or("");
        if content["encoding"].as_str() == Some("base64") {
            base64::engine::general_purpose::STANDARD
                .decode(text)
                .map_err(|e| Error::invalid_argument(e.to_string()))
        } else {
            Ok(text.as_bytes().to_vec())
        }
    }

    fn find(
        &self,
        url: &str,
        method: &str,
        headers: &[Header],
        post_data: Option<&[u8]>,
    ) -> Result<Option<&Value>> {
        let mut url = url.to_string();
        let mut method = method.to_string();
        let mut visited: Vec<*const Value> = Vec::new();
        loop {
            let mut candidates: Vec<&Value> = Vec::new();
            for candidate in &self.entries {
                if candidate["request"]["url"].as_str() != Some(url.as_str())
                    || candidate["request"]["method"].as_str() != Some(method.as_str())
                {
                    continue;
                }
                if method == "POST" {
                    if let (Some(post), Some(candidate_post)) =
                        (post_data, candidate["request"].get("postData"))
                    {
                        let buffer = self.load_content(candidate_post)?;
                        if buffer != post {
                            let Some(boundary) = multipart_boundary(headers) else {
                                continue;
                            };
                            let Some(candidate_boundary) =
                                multipart_boundary(&har_headers(&candidate["request"]["headers"]))
                            else {
                                continue;
                            };
                            let a = String::from_utf8_lossy(post).replace(&boundary, "");
                            let b =
                                String::from_utf8_lossy(&buffer).replace(&candidate_boundary, "");
                            if a != b {
                                continue;
                            }
                        }
                    }
                }
                candidates.push(candidate);
            }
            if candidates.is_empty() {
                return Ok(None);
            }
            let mut entry = candidates[0];
            if candidates.len() > 1 {
                let set: std::collections::HashSet<String> = headers
                    .iter()
                    .map(|h| format!("{}:{}", h.name.to_lowercase(), h.value))
                    .collect();
                let mut best = 0usize;
                for (i, c) in candidates.iter().enumerate() {
                    let count = har_headers(&c["request"]["headers"])
                        .iter()
                        .filter(|h| set.contains(&format!("{}:{}", h.name.to_lowercase(), h.value)))
                        .count();
                    if i == 0 || count > best {
                        best = count;
                        entry = c;
                    }
                }
            }
            let ptr = entry as *const Value;
            if visited.contains(&ptr) {
                return Err(Error::invalid_argument(format!(
                    "Found redirect cycle for {url}"
                )));
            }
            visited.push(ptr);
            let status = entry["response"]["status"].as_i64().unwrap_or(0);
            let location = header_get(&har_headers(&entry["response"]["headers"]), "location");
            if [301, 302, 303, 307, 308].contains(&status) {
                if let Some(location) = location {
                    if let Ok(next) = url::Url::parse(&url).and_then(|u| u.join(&location)) {
                        url = next.to_string();
                        if ((status == 301 || status == 302) && method == "POST")
                            || (status == 303 && method != "GET" && method != "HEAD")
                        {
                            method = "GET".into();
                        }
                        continue;
                    }
                }
            }
            return Ok(Some(entry));
        }
    }

    fn lookup(
        &self,
        url: &str,
        method: &str,
        headers: &[Header],
        post_data: Option<&[u8]>,
        is_navigation: bool,
    ) -> Lookup {
        let entry = match self.find(url, method, headers, post_data) {
            Ok(Some(e)) => e,
            Ok(None) => return Lookup::NotFound,
            Err(e) => return Lookup::Error(format!("HAR error: {}", e.message())),
        };
        let entry_url = entry["request"]["url"].as_str().unwrap_or("");
        if entry_url != url && is_navigation {
            return Lookup::Redirect(entry_url.to_string());
        }
        match self.load_content(&entry["response"]["content"]) {
            Ok(body) => Lookup::Fulfill {
                status: entry["response"]["status"].as_i64().unwrap_or(200),
                headers: har_headers(&entry["response"]["headers"]),
                body,
            },
            Err(e) => Lookup::Error(e.message()),
        }
    }
}

async fn har_handle(backend: Arc<HarBackend>, not_found: HarNotFound, route: Route) -> Result<()> {
    let request = route.request();
    let headers = request.headers_array().await.unwrap_or_default();
    let lookup = backend.lookup(
        &request.url(),
        &request.method(),
        &headers,
        request.post_data_buffer().as_deref(),
        request.is_navigation_request(),
    );
    match lookup {
        Lookup::Redirect(url) => {
            // Restart the navigation at the final URL so that the document
            // URL changes, like a real redirect.
            return route
                .fulfill()
                .status(302)
                .headers([("location", url)])
                .await;
        }
        Lookup::Fulfill {
            status,
            headers,
            body,
        } => {
            if status == -1 {
                // Canceled or stalled when recorded: stall it.
                return Ok(());
            }
            let mut merged: Vec<(String, String)> = Vec::new();
            for h in headers {
                if h.name.eq_ignore_ascii_case("set-cookie") {
                    if let Some(existing) = merged.iter_mut().find(|(n, _)| n == "set-cookie") {
                        existing.1.push('\n');
                        existing.1.push_str(&h.value);
                        continue;
                    }
                    merged.push(("set-cookie".into(), h.value));
                } else {
                    merged.retain(|(n, _)| n != &h.name);
                    merged.push((h.name, h.value));
                }
            }
            return route
                .fulfill()
                .status(status as u16)
                .headers(merged)
                .body(body)
                .await;
        }
        Lookup::Error(message) => tracing::debug!("{message}"),
        Lookup::NotFound => {}
    }
    match not_found {
        HarNotFound::Abort => route.abort().await,
        HarNotFound::Fallback => route.fallback().await,
    }
}

/// Builder returned by `route_from_har`; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct RouteFromHar {
    target: RouteTarget,
    path: PathBuf,
    url: Option<UrlMatch>,
    not_found: HarNotFound,
    update: bool,
    update_content: HarContentPolicy,
    update_mode: HarMode,
}

impl RouteFromHar {
    /// Only serve requests whose URL matches from the HAR.
    pub fn url(mut self, url: impl Into<UrlMatch>) -> Self {
        self.url = Some(url.into());
        self
    }

    /// What to do with requests not found in the HAR (default abort).
    pub fn not_found(mut self, not_found: HarNotFound) -> Self {
        self.not_found = not_found;
        self
    }

    /// Record the HAR instead of replaying it (written on context close).
    pub fn update(mut self, update: bool) -> Self {
        self.update = update;
        self
    }

    /// Body storage when updating (default attach in Playwright; embed here
    /// unless set).
    pub fn update_content(mut self, content: HarContentPolicy) -> Self {
        self.update_content = content;
        self
    }

    /// Detail level when updating (default minimal).
    pub fn update_mode(mut self, mode: HarMode) -> Self {
        self.update_mode = mode;
        self
    }
}

impl IntoFuture for RouteFromHar {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let api = match self.target {
                RouteTarget::Page(_) => "page.routeFromHAR",
                RouteTarget::Context(_) => "browserContext.routeFromHAR",
            };
            async {
                let (context, page) = match &self.target {
                    RouteTarget::Page(p) => (p.context(), Some(Arc::downgrade(&p.inner))),
                    RouteTarget::Context(c) => (Some(c.clone()), None),
                };
                if self.update {
                    let context = context.ok_or_else(Error::target_closed)?;
                    let mut options = RecordHar::new(self.path.clone())
                        .content(self.update_content)
                        .mode(self.update_mode);
                    options.url_filter = self.url.clone();
                    let version = context
                        .browser()
                        .map(|b| b.version().to_string())
                        .unwrap_or_default();
                    let recorder = HarRecorder::new(
                        options,
                        context.inner.options.base_url.clone(),
                        page,
                        version,
                    )?;
                    context
                        .inner
                        .ext
                        .har_recorders
                        .lock()
                        .unwrap()
                        .push(Arc::new(recorder));
                    return Ok(());
                }
                let text = tokio::fs::read_to_string(&self.path).await.map_err(|e| {
                    Error::invalid_argument(format!(
                        "Error reading HAR file {}: {e}",
                        self.path.display()
                    ))
                })?;
                let har: Value = serde_json::from_str(&text)?;
                let entries = har["log"]["entries"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let base_dir = self
                    .path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default();
                let backend = Arc::new(HarBackend { entries, base_dir });
                let not_found = self.not_found;
                let url = self.url.unwrap_or_else(|| UrlMatch::Glob("**/*".into()));
                let base_url = match &self.target {
                    RouteTarget::Page(p) => p.inner.base_url(),
                    RouteTarget::Context(c) => c.inner.options.base_url.clone(),
                };
                validate_glob(base_url.as_deref(), &url)?;
                let handler =
                    boxed_handler(move |route| har_handle(backend.clone(), not_found, route));
                self.target
                    .add(RouteHandler::new(base_url, url, handler, None))
                    .await
                    .map(|_| ())
            }
            .await
            .api(api)
        })
    }
}

impl Page {
    /// Serve requests from a HAR file (or record one with
    /// [`RouteFromHar::update`]).
    pub fn route_from_har(&self, path: impl Into<PathBuf>) -> RouteFromHar {
        RouteFromHar {
            target: RouteTarget::Page(self.clone()),
            path: path.into(),
            url: None,
            not_found: HarNotFound::Abort,
            update: false,
            update_content: HarContentPolicy::Embed,
            update_mode: HarMode::Minimal,
        }
    }
}

impl BrowserContext {
    /// Serve requests of every page from a HAR file (or record one with
    /// [`RouteFromHar::update`]).
    pub fn route_from_har(&self, path: impl Into<PathBuf>) -> RouteFromHar {
        RouteFromHar {
            target: RouteTarget::Context(self.clone()),
            path: path.into(),
            url: None,
            not_found: HarNotFound::Abort,
            update: false,
            update_content: HarContentPolicy::Embed,
            update_mode: HarMode::Minimal,
        }
    }
}
