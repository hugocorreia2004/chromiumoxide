//! Navigation, cookie and teardown behavior against a local test server.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chromiumoxide::cdp::browser_protocol::fetch::EventRequestPaused;
use chromiumoxide::cdp::browser_protocol::network::{CookieParam, ErrorReason};
use chromiumoxide::cdp::browser_protocol::page::EventFrameNavigated;
use chromiumoxide::error::CdpError;
use chromiumoxide::{BrowserConfig, LifecycleEvent};
use futures::StreamExt;
use tokio::time::timeout;

use crate::test_config;

/// Short enough that a hang shows up as a failed test, long enough for CI.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

fn config() -> BrowserConfig {
    BrowserConfig::builder()
        .new_headless_mode()
        .request_timeout(REQUEST_TIMEOUT)
        .args(["--no-proxy-server"])
        .build()
        .expect("browser config is valid")
}

/// A tiny HTTP server, one thread per connection.
pub(crate) struct TestServer {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl TestServer {
    pub(crate) fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
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
                        let stop = Arc::clone(&thread_stop);
                        std::thread::spawn(move || serve(stream, &stop));
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

    pub(crate) fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(mut stream: TcpStream, stop: &AtomicBool) {
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
        "/hang" => {
            // Never answer: the request stays pending until the test ends.
            while !stop.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(20));
            }
            return;
        }
        "/slow" => {
            std::thread::sleep(Duration::from_millis(1500));
            "slow".to_owned()
        }
        "/late-subframe" => r#"<!doctype html><title>late subframe</title>
<script>
  window.addEventListener('load', () => {
    const frame = document.createElement('iframe');
    frame.src = '/hang';
    document.body.appendChild(frame);
  });
</script><body>main</body>"#
            .to_owned(),
        "/idle" => r#"<!doctype html><title>waiting</title>
<script>
  window.addEventListener('load', () => {
    fetch('/slow').then(r => r.text()).then(() => { document.title = 'done'; });
  });
</script><body>idle</body>"#
            .to_owned(),
        _ => "<!doctype html><title>ok</title><body><button id=b>b</button></body>".to_owned(),
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

/// mattsse/chromiumoxide#191: `goto` resolves on the main frame's `load`
/// even when a subframe attached afterwards never finishes loading.
#[tokio::test]
async fn goto_returns_when_a_late_subframe_never_loads() {
    let server = TestServer::start();
    test_config(config(), async |browser| {
        let page = browser.new_page("about:blank").await.unwrap();
        let started = Instant::now();
        page.goto(server.url("/late-subframe"))
            .await
            .expect("goto resolves on the main frame load");
        assert!(started.elapsed() < REQUEST_TIMEOUT);
        assert_eq!(
            page.get_title().await.unwrap().as_deref(),
            Some("late subframe")
        );
    })
    .await;
}

/// mattsse/chromiumoxide#125: aborting the navigation request makes `goto`
/// fail promptly instead of never returning.
#[tokio::test]
async fn goto_fails_when_the_navigation_request_is_aborted() {
    let server = TestServer::start();
    test_config(config(), async |browser| {
        let page = browser.new_page("about:blank").await.unwrap();
        let mut paused = page
            .event_listener::<EventRequestPaused>()
            .await
            .expect("listen for paused requests");
        page.set_request_interception(true).await.unwrap();

        let aborter = {
            let page = page.clone();
            tokio::spawn(async move {
                let event = paused.next().await.expect("the navigation request pauses");
                page.execute(
                    chromiumoxide::cdp::browser_protocol::fetch::FailRequestParams::new(
                        event.request_id.clone(),
                        ErrorReason::Aborted,
                    ),
                )
                .await
                .expect("fail the paused request");
            })
        };

        let started = Instant::now();
        let result = page.goto(server.url("/")).await;
        assert!(result.is_err(), "an aborted navigation is an error");
        assert!(started.elapsed() < REQUEST_TIMEOUT);
        aborter.await.unwrap();
    })
    .await;
}

/// mattsse/chromiumoxide#36: wait until the network is idle.
#[tokio::test]
async fn wait_for_navigation_until_network_idle() {
    let server = TestServer::start();
    test_config(config(), async |browser| {
        let page = browser.new_page("about:blank").await.unwrap();
        page.goto(server.url("/idle")).await.unwrap();
        page.wait_for_navigation_until(LifecycleEvent::NetworkIdle)
            .await
            .expect("network becomes idle");
        assert_eq!(page.get_title().await.unwrap().as_deref(), Some("done"));
    })
    .await;
}

/// mattsse/chromiumoxide#14: after `Page::close`, handles of the page fail
/// fast with `TargetClosed` and its event streams end.
#[tokio::test]
async fn closed_page_fails_fast_and_ends_its_streams() {
    let server = TestServer::start();
    test_config(config(), async |browser| {
        let page = browser.new_page(server.url("/")).await.unwrap();
        let element = page.find_element("#b").await.unwrap();
        let mut navigations = page.event_listener::<EventFrameNavigated>().await.unwrap();
        let clone = page.clone();

        page.close().await.expect("page closes");

        assert!(matches!(element.click().await, Err(CdpError::TargetClosed)));
        assert!(matches!(
            clone.evaluate("1 + 1").await,
            Err(CdpError::TargetClosed)
        ));
        assert!(matches!(
            clone.wait_for_navigation().await,
            Err(CdpError::TargetClosed)
        ));
        let end = timeout(Duration::from_secs(5), navigations.next())
            .await
            .expect("the event stream ends");
        assert!(end.is_none());
    })
    .await;
}

/// mattsse/chromiumoxide#293: clearing and setting cookies on the browser
/// resolves.
#[tokio::test]
async fn browser_cookies_round_trip() {
    let server = TestServer::start();
    test_config(config(), async |browser| {
        browser.set_cookies(Vec::new()).await.expect("empty set");
        browser.clear_cookies().await.expect("clear");

        let mut cookie = CookieParam::new("name", "value");
        cookie.url = Some(server.url("/"));
        browser.set_cookies(vec![cookie]).await.expect("set");
        let cookies = browser.get_cookies().await.expect("get");
        assert!(cookies.iter().any(|cookie| cookie.name == "name"));

        browser.clear_cookies().await.expect("clear");
        let cookies = browser.get_cookies().await.expect("get");
        assert!(cookies.iter().all(|cookie| cookie.name != "name"));
    })
    .await;
}
