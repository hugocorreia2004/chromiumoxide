//! Page-side probes for CDP traces, run against a local server.
//!
//! The page records what it can observe of the automation on its own
//! `<html>` element:
//! - `runtimeEnable`: `Runtime.enable` makes the browser serialize console
//!   arguments, which historically read the `stack` of an `Error` passed to
//!   `console.debug` (the classic probe; recent Chrome no longer runs that
//!   getter, so the tests also check on the protocol side that no
//!   `Runtime.consoleAPICalled` event is produced, which only happens once
//!   `Runtime.enable` was sent);
//! - `mainWorld`: a call of a DOM method the page patched, i.e. automation code
//!   running in the page's main world (like bot-detector.rebrowser.net's
//!   `mainWorldExecution` check, which patches `document.getElementsByClassName`).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use chromiumoxide::cdp::js_protocol::runtime::EventConsoleApiCalled;
use chromiumoxide::listeners::EventStream;
use chromiumoxide::{BrowserConfig, Page};
use futures::StreamExt;

use crate::test_config;

struct ProbeServer {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ProbeServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind probe server");
        listener
            .set_nonblocking(true)
            .expect("non-blocking listener");
        let port = listener.local_addr().expect("local address").port();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        std::thread::spawn(move || serve(stream, port));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            port,
            stop,
            thread: Some(thread),
        }
    }

    fn url(&self, host: &str, path: &str) -> String {
        format!("http://{host}:{}{path}", self.port)
    }
}

impl Drop for ProbeServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Installed first in every document; records what the page can observe.
const PROBES: &str = r#"<script>
(() => {
  const mark = name => { document.documentElement.dataset[name] = '1'; };
  const probe = () => {
    const error = new Error('probe');
    Object.defineProperty(error, 'stack', {
      configurable: true,
      get() { mark('runtimeEnable'); return ''; },
    });
    console.debug(error);
  };
  probe();
  setInterval(probe, 25);

  const getElementsByClassName = document.getElementsByClassName;
  document.getElementsByClassName = function (...args) {
    mark('mainWorld');
    return getElementsByClassName.apply(this, args);
  };
  for (const [proto, name] of [
    [Document.prototype, 'querySelector'],
    [Document.prototype, 'querySelectorAll'],
    [Element.prototype, 'querySelector'],
    [Element.prototype, 'getBoundingClientRect'],
    [Element.prototype, 'getClientRects'],
    [Element.prototype, 'scrollIntoView'],
    [Element.prototype, 'scrollIntoViewIfNeeded'],
    [HTMLElement.prototype, 'focus'],
  ]) {
    const original = proto[name];
    if (!original) continue;
    proto[name] = function (...args) {
      mark('mainWorld');
      return original.apply(this, args);
    };
  }
})();
</script>"#;

fn serve(mut stream: TcpStream, port: u16) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => request.extend_from_slice(&buffer[..read]),
        }
    }
    let request = String::from_utf8_lossy(&request);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_owned();
    let body = match path.as_str() {
        "/child" => format!(
            r#"<!doctype html><html><head>{PROBES}</head><body>
            <span class="item" id="child-item">child</span></body></html>"#
        ),
        _ => format!(
            r#"<!doctype html><html><head><title>probe</title>{PROBES}</head><body>
            <button id="button" onclick="document.body.dataset.clicked = '1'">button</button>
            <span class="item">item</span>
            <iframe id="frame" src="http://127.0.0.1:{port}/child"></iframe>
            </body></html>"#
        ),
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

fn config() -> BrowserConfig {
    BrowserConfig::builder()
        .new_headless_mode()
        .request_timeout(Duration::from_secs(20))
        .args(["--site-per-process", "--no-proxy-server"])
        .build()
        .expect("browser config is valid")
}

/// What the page observed, read from its DOM through an isolated world.
async fn observed(page: &Page) -> (bool, bool) {
    let flags: serde_json::Value = page
        .evaluate("JSON.stringify(Object.assign({}, document.documentElement.dataset))")
        .await
        .expect("read the probes")
        .into_value::<String>()
        .map(|json| serde_json::from_str(&json).expect("probe json"))
        .expect("probe string");
    (
        flags.get("runtimeEnable").is_some(),
        flags.get("mainWorld").is_some(),
    )
}

/// How many `Runtime.consoleAPICalled` events arrived (the page logs every
/// 25 ms): the browser only reports them after `Runtime.enable`.
async fn console_events(events: &mut EventStream<EventConsoleApiCalled>) -> usize {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
    let mut count = 0;
    while let Ok(Some(_)) = tokio::time::timeout_at(deadline, events.next()).await {
        count += 1;
    }
    count
}

/// Drive the usual APIs: evaluation, element lookup and interaction, frames.
async fn exercise(page: &Page) {
    let items: usize = page
        .evaluate("document.getElementsByClassName('item').length")
        .await
        .expect("evaluate an expression")
        .into_value()
        .expect("a number");
    assert_eq!(items, 1);
    let title: String = page
        .evaluate_function("() => document.querySelector('title').textContent")
        .await
        .expect("evaluate a function")
        .into_value()
        .expect("a string");
    assert_eq!(title, "probe");

    let button = page.find_element("#button").await.expect("find the button");
    button.click().await.expect("click the button");
    assert_eq!(
        button
            .attribute("id")
            .await
            .expect("read an attribute")
            .as_deref(),
        Some("button")
    );
    let _ = button.inner_text().await.expect("read the text");
    let _ = page.find_elements("span").await.expect("find spans");
    let _ = page.content().await.expect("read the content");

    let clicked: String = page
        .evaluate("document.body.dataset.clicked || ''")
        .await
        .expect("read the click")
        .into_value()
        .expect("a string");
    assert_eq!(clicked, "1", "the click reached the page");

    // The cross-site iframe, through its own session.
    let child = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Some(frame) = page
                .all_frames()
                .await
                .expect("list frames")
                .into_iter()
                .find(|frame| frame.url().is_some_and(|url| url.ends_with("/child")))
            {
                return frame;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the iframe attaches");
    let count: usize = child
        .eval("document.getElementsByClassName('item').length")
        .await
        .expect("evaluate in the iframe")
        .into_value()
        .expect("a number");
    assert_eq!(count, 1);
    let item = child
        .query_selector("#child-item")
        .await
        .expect("query the iframe")
        .expect("the item exists");
    let _ = item.inner_text().await.expect("read the iframe text");
    let child_flags: String = child
        .eval("JSON.stringify(Object.assign({}, document.documentElement.dataset))")
        .await
        .expect("read the iframe probes")
        .into_value()
        .expect("a string");
    if cfg!(feature = "stealth") {
        assert_eq!(child_flags, "{}", "the iframe observed the automation");
    }
}

/// With `stealth`, the page sees neither `Runtime.enable` nor main-world
/// execution while the usual APIs are used, including across a cross-site
/// navigation (process swap) and in an out-of-process iframe.
#[cfg(feature = "stealth")]
#[tokio::test]
async fn stealth_leaves_no_trace_the_page_can_observe() {
    let server = ProbeServer::start();
    test_config(config(), async |browser| {
        let page = browser.new_page("about:blank").await.unwrap();
        let mut console = page
            .event_listener::<EventConsoleApiCalled>()
            .await
            .unwrap();
        page.goto(server.url("localhost", "/")).await.unwrap();
        exercise(&page).await;
        // Let the console probe run a few more times.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(observed(&page).await, (false, false));
        assert_eq!(console_events(&mut console).await, 0, "Runtime is enabled");

        // A cross-site navigation swaps the renderer process; the cached
        // isolated world must not be reused for the new document.
        page.goto(server.url("127.0.0.1", "/")).await.unwrap();
        let host: String = page
            .evaluate("location.hostname")
            .await
            .expect("evaluate after the swap")
            .into_value()
            .expect("a string");
        assert_eq!(host, "127.0.0.1");
        exercise(&page).await;
        page.goto(server.url("localhost", "/")).await.unwrap();
        exercise(&page).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(observed(&page).await, (false, false));
        assert_eq!(console_events(&mut console).await, 0, "Runtime is enabled");

        // The main world stays reachable on purpose.
        let main_world_items: usize = page
            .evaluate_main_world("document.getElementsByClassName('item').length")
            .await
            .expect("evaluate in the main world")
            .into_value()
            .expect("a number");
        assert_eq!(main_world_items, 1);
        assert_eq!(observed(&page).await, (false, true));

        assert!(
            page.expose_function("exposed", "() => 1").await.is_err(),
            "bindings are refused under stealth"
        );
    })
    .await;
}

/// Without `stealth` the same checks do see the automation, which shows that
/// they work: the page observes main-world execution and the browser
/// reports console calls because `Runtime.enable` was sent.
#[cfg(not(feature = "stealth"))]
#[tokio::test]
async fn probes_detect_the_default_protocol_usage() {
    let server = ProbeServer::start();
    test_config(config(), async |browser| {
        let page = browser.new_page("about:blank").await.unwrap();
        let mut console = page
            .event_listener::<EventConsoleApiCalled>()
            .await
            .unwrap();
        page.goto(server.url("localhost", "/")).await.unwrap();
        exercise(&page).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let (_stack_probe, main_world) = observed(&page).await;
        assert!(main_world, "main-world execution is observed");
        assert!(console_events(&mut console).await > 0, "Runtime is enabled");
    })
    .await;
}

/// `Page::evaluate_main_world` reaches the page's own globals, in both
/// builds, for functions and expressions, and in subframes.
#[tokio::test]
async fn evaluate_main_world_reaches_page_globals() {
    let server = ProbeServer::start();
    test_config(config(), async |browser| {
        let page = browser.new_page("about:blank").await.unwrap();
        page.goto(server.url("localhost", "/")).await.unwrap();
        page.evaluate_main_world("window.pageGlobal = 41")
            .await
            .expect("set a page global");
        let value: i64 = page
            .evaluate_main_world("() => window.pageGlobal + 1")
            .await
            .expect("call a function in the main world")
            .into_value()
            .expect("a number");
        assert_eq!(value, 42);
        let main = page.main_frame().await.unwrap().expect("main frame");
        let value: i64 = main
            .evaluate_main_world("window.pageGlobal")
            .await
            .expect("frame main world")
            .into_value()
            .expect("a number");
        assert_eq!(value, 41);
    })
    .await;
}
