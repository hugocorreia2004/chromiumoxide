//! Shared test harness: one browser for the whole test binary, a fresh
//! context + page per test and a local HTTP server per test serving
//! `tests/assets` (with per-test custom routes), like Playwright's
//! `server`/`page` fixtures.

#![allow(dead_code)]

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use futures::future::BoxFuture;
use oxidewright::{Browser, BrowserContext, Frame, Page};
use tokio::sync::Notify;

pub type Handler = Arc<dyn Fn(Request) -> BoxFuture<'static, Response> + Send + Sync>;

async fn launch_browser() -> Browser {
    let mut launch = oxidewright::chromium().launch();
    if let Ok(path) = std::env::var("OXIDEWRIGHT_CHROME") {
        launch = launch.executable_path(path);
    }
    launch.await.expect("failed to launch the browser")
}

/// Everything a test gets.
pub struct TestContext {
    pub browser: Browser,
    pub context: BrowserContext,
    pub page: Page,
    pub server: TestServer,
}

/// Run an async test with a fresh context/page and server.
pub fn run<F, Fut>(f: F)
where
    F: FnOnce(TestContext) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    // One browser per test: the test binary has no global teardown, and a
    // browser left running would keep the process' output pipes open.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("tokio runtime");
    runtime.block_on(async move {
        let browser = launch_browser().await;
        let server = TestServer::start().await;
        let setup = async {
            let context = browser.new_context().await?;
            let page = context.new_page().await?;
            Ok::<_, oxidewright::Error>((context, page))
        };
        let (context, page) = match setup.await {
            Ok(v) => v,
            Err(e) => {
                let _ = browser.close().await;
                panic!("test setup failed: {e}");
            }
        };
        let ctx = TestContext {
            browser: browser.clone(),
            context,
            page,
            server,
        };
        let result = tokio::time::timeout(Duration::from_secs(60), tokio::spawn(f(ctx))).await;
        let _ = browser.close().await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(join_error)) => std::panic::resume_unwind(join_error.into_panic()),
            Err(_) => panic!("test timed out after 60s"),
        }
    });
}

#[derive(Clone)]
pub struct TestServer {
    pub port: u16,
    /// `http://localhost:<port>`
    pub prefix: String,
    /// `http://127.0.0.1:<port>` (another site: frames from it are OOPIFs)
    pub cross_process_prefix: String,
    /// `<prefix>/empty.html`
    pub empty_page: String,
    routes: Arc<Mutex<HashMap<String, Handler>>>,
    requests: Arc<Mutex<HashMap<String, Arc<Notify>>>>,
    seen: Arc<Mutex<Vec<String>>>,
    headers: Arc<Mutex<HashMap<String, axum::http::HeaderMap>>>,
}

fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("assets")
}

fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

impl TestServer {
    pub async fn start() -> TestServer {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = TestServer {
            port,
            prefix: format!("http://localhost:{port}"),
            cross_process_prefix: format!("http://127.0.0.1:{port}"),
            empty_page: format!("http://localhost:{port}/empty.html"),
            routes: Arc::default(),
            requests: Arc::default(),
            seen: Arc::default(),
            headers: Arc::default(),
        };
        let state = server.clone();
        let app = axum::Router::new().fallback(move |req: Request| {
            let state = state.clone();
            async move { state.handle(req).await }
        });
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        server
    }

    async fn handle(&self, req: Request) -> Response {
        let path = req.uri().path().to_string();
        self.seen.lock().unwrap().push(path.clone());
        self.headers
            .lock()
            .unwrap()
            .insert(path.clone(), req.headers().clone());
        let notify = self
            .requests
            .lock()
            .unwrap()
            .entry(path.clone())
            .or_insert_with(|| Arc::new(Notify::new()))
            .clone();
        notify.notify_waiters();
        notify.notify_one();
        let route = self.routes.lock().unwrap().get(&path).cloned();
        if let Some(route) = route {
            return route(req).await;
        }
        let relative = path.trim_start_matches('/');
        let file = assets_dir().join(relative);
        match tokio::fs::read(&file).await {
            Ok(bytes) => Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime_for(relative))
                .header(header::CACHE_CONTROL, "no-cache, no-store")
                .body(Body::from(bytes))
                .unwrap(),
            Err(_) => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from(format!("File not found: {path}")))
                .unwrap(),
        }
    }

    /// Serve `path` with a custom handler.
    pub fn set_route<F, Fut>(&self, path: &str, f: F)
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        let handler: Handler = Arc::new(move |req| Box::pin(f(req)));
        self.routes
            .lock()
            .unwrap()
            .insert(path.to_string(), handler);
    }

    /// Serve `path` with a fixed body and content type.
    pub fn set_content(&self, path: &str, content_type: &'static str, body: &'static str) {
        self.set_route(path, move |_| async move {
            Response::builder()
                .header(header::CONTENT_TYPE, content_type)
                .body(Body::from(body))
                .unwrap()
        });
    }

    /// Redirect `from` to `to` with a 302.
    pub fn set_redirect(&self, from: &str, to: &str) {
        let to = to.to_string();
        self.set_route(from, move |_| {
            let to = to.clone();
            async move {
                Response::builder()
                    .status(StatusCode::FOUND)
                    .header(header::LOCATION, HeaderValue::from_str(&to).unwrap())
                    .body(Body::empty())
                    .unwrap()
            }
        });
    }

    /// Never respond to `path`.
    pub fn hang(&self, path: &str) {
        self.set_route(path, |_| async {
            futures::future::pending::<()>().await;
            unreachable!()
        });
    }

    /// Serve `path` only once the returned gate is released.
    pub fn stall(
        &self,
        path: &str,
        content_type: &'static str,
        body: &'static str,
        status: u16,
    ) -> Gate {
        let (tx, rx) = tokio::sync::watch::channel(false);
        self.set_route(path, move |_| {
            let mut rx = rx.clone();
            async move {
                let _ = rx.wait_for(|released| *released).await;
                Response::builder()
                    .status(status)
                    .header(header::CONTENT_TYPE, content_type)
                    .body(Body::from(body))
                    .unwrap()
            }
        });
        Gate(Arc::new(tx))
    }

    /// Resolves when the server receives a request for `path`.
    pub async fn wait_for_request(&self, path: &str) {
        let notify = self
            .requests
            .lock()
            .unwrap()
            .entry(path.to_string())
            .or_insert_with(|| Arc::new(Notify::new()))
            .clone();
        notify.notified().await;
    }

    /// Headers of the last request for `path`.
    pub fn request_headers(&self, path: &str) -> Option<axum::http::HeaderMap> {
        self.headers.lock().unwrap().get(path).cloned()
    }

    /// Paths requested so far.
    pub fn requested(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

#[derive(Clone)]
pub struct Gate(Arc<tokio::sync::watch::Sender<bool>>);

impl Gate {
    pub fn release(&self) {
        self.0.send_replace(true);
    }
}

pub fn html_response(body: impl Into<String>) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(body.into()))
        .unwrap()
}

/// Playwright's `attachFrame` test util.
pub async fn attach_frame(page: &Page, frame_id: &str, url: &str) -> Frame {
    let handle = page
        .evaluate_handle(
            "async ({ frameId, url }) => {
                const frame = document.createElement('iframe');
                frame.src = url;
                frame.id = frameId;
                document.body.appendChild(frame);
                await new Promise(x => frame.onload = x);
                return frame;
            }",
        )
        .arg(serde_json::json!({ "frameId": frame_id, "url": url }))
        .await
        .expect("attach frame");
    handle
        .as_element()
        .expect("iframe element")
        .content_frame()
        .await
        .expect("content frame")
        .expect("frame")
}

/// Playwright's `detachFrame` test util.
pub async fn detach_frame(page: &Page, frame_id: &str) {
    page.evaluate::<()>("frameId => document.getElementById(frameId).remove()")
        .arg(frame_id)
        .await
        .expect("detach frame");
}

/// Playwright's `dumpFrames` test util.
pub fn dump_frames(frame: &Frame, indentation: &str) -> Vec<String> {
    let re = regex::Regex::new(r":\d{4,5}/").unwrap();
    let mut description = re.replace(&frame.url(), ":<PORT>/").to_string();
    if !frame.name().is_empty() {
        description = format!("{description} ({})", frame.name());
    }
    let mut result = vec![format!("{indentation}{description}")];
    let mut children = frame.child_frames();
    children.sort_by(|a, b| {
        if a.url() != b.url() {
            a.url().cmp(&b.url())
        } else {
            a.name().cmp(&b.name())
        }
    });
    for child in children {
        result.extend(dump_frames(&child, &format!("    {indentation}")));
    }
    result
}

/// Assert `err`'s rendered message contains `needle`.
#[track_caller]
pub fn assert_err_contains<T: std::fmt::Debug>(
    result: oxidewright::Result<T>,
    needle: &str,
) -> oxidewright::Error {
    match result {
        Ok(v) => panic!("expected an error containing {needle:?}, got Ok({v:?})"),
        Err(e) => {
            let text = e.to_string();
            assert!(
                text.contains(needle),
                "error {text:?} does not contain {needle:?}"
            );
            e
        }
    }
}
