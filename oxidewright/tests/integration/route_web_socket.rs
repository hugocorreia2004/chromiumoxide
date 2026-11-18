//! Port of `tests/library/route-web-socket.spec.ts` (the WebSocket server
//! is a small axum server of this file), plus a stealth check of the
//! main-world footprint.
//!
//! Skipped: the `no-mock`/`no-match`/`pass-through` matrix of WS-001..022
//! (WS-015 is covered by `ws_015_pass_through_text_message`), WS-024/025
//! (upstream handshake failures need a misbehaving server), WS-029
//! (frames), WS-031 (frame detach), WS-033 (page close race), WS-036..038
//! (host-only URLs and `baseURL` with the same host as the page server).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocketUpgrade};
use oxidewright::{Page, WebSocketPayload, WebSocketRoute};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use crate::common::*;

/// A WebSocket server logging what it receives (`message: x`,
/// `close: code=.. reason=..`); `send` pushes to the last client.
#[derive(Clone)]
struct WsServer {
    port: u16,
    log: Arc<Mutex<Vec<String>>>,
    client: Arc<Mutex<Option<UnboundedSender<String>>>>,
}

impl WsServer {
    async fn start() -> WsServer {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = WsServer {
            port,
            log: Arc::default(),
            client: Arc::default(),
        };
        let state = server.clone();
        let app = axum::Router::new().fallback(move |ws: WebSocketUpgrade| {
            let state = state.clone();
            async move {
                ws.protocols(["chat.v1", "chat.v2"])
                    .on_upgrade(move |mut socket| async move {
                        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
                        *state.client.lock().unwrap() = Some(tx);
                        loop {
                            tokio::select! {
                                msg = socket.recv() => match msg {
                                    Some(Ok(Message::Text(t))) => state.log.lock().unwrap().push(format!("message: {}", t.as_str())),
                                    Some(Ok(Message::Close(frame))) => {
                                        let (code, reason) = frame
                                            .map(|f| (f.code, f.reason.to_string()))
                                            .unwrap_or((1005, String::new()));
                                        state.log.lock().unwrap().push(format!("close: code={code} reason={reason}"));
                                        break;
                                    }
                                    Some(Ok(_)) => {}
                                    _ => break,
                                },
                                out = rx.recv() => match out {
                                    Some(text) => { let _ = socket.send(Message::Text(text.into())).await; }
                                    None => break,
                                },
                            }
                        }
                    })
            }
        });
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        server
    }

    fn host(&self) -> String {
        format!("localhost:{}", self.port)
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    async fn send(&self, text: &str) {
        for _ in 0..200 {
            if let Some(tx) = self.client.lock().unwrap().as_ref() {
                let _ = tx.send(text.to_string());
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("no WebSocket client");
    }
}

#[allow(dead_code)]
fn _close_frame_type(_: CloseFrame) {}

async fn setup_ws(page: &Page, empty_page: &str, ws: &WsServer, protocols: Value) {
    page.goto(empty_page).await.unwrap();
    let _: Value = page
        .evaluate(
            r#"({ wsUrl, protocols }) => {
    window.log = [];
    window.ws = protocols === null ? new WebSocket(wsUrl) : new WebSocket(wsUrl, protocols);
    window.ws.binaryType = 'blob';
    window.ws.addEventListener('open', () => window.log.push('open'));
    window.ws.addEventListener('close', event => window.log.push(`close code=${event.code} reason=${event.reason} wasClean=${event.wasClean}`));
    window.ws.addEventListener('error', event => window.log.push(`error`));
    window.ws.addEventListener('message', async event => {
      let data;
      if (typeof event.data === 'string')
        data = event.data;
      else if (event.data instanceof Blob)
        data = 'blob:' + await event.data.text();
      else
        data = 'arraybuffer:' + await (new Blob([event.data])).text();
      window.log.push(`message: data=${data} origin=${event.origin} lastEventId=${event.lastEventId}`);
    });
    window.wsOpened = new Promise(f => window.ws.addEventListener('open', () => f()));
  }"#,
        )
        .arg(serde_json::json!({ "wsUrl": format!("ws://{}/ws", ws.host()), "protocols": protocols }))
        .await
        .unwrap();
}

async fn page_log(page: &Page) -> Vec<String> {
    page.evaluate("() => window.log").await.unwrap()
}

async fn poll_page(page: &Page, expected: &[String]) {
    let mut last = Vec::new();
    for _ in 0..200 {
        last = page_log(page).await;
        if last == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("page log {last:?} != {expected:?}");
}

async fn poll_server(ws: &WsServer, expected: &[&str]) {
    for _ in 0..200 {
        if ws.log() == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("server log {:?} != {expected:?}", ws.log());
}

fn msg(ws: &WsServer, data: &str) -> String {
    format!(
        "message: data={data} origin=ws://{} lastEventId=",
        ws.host()
    )
}

fn text(p: &WebSocketPayload) -> String {
    p.as_text()
}

#[test]
fn ws_015_pass_through_text_message() {
    run(|t| async move {
        let ws = WsServer::start().await;
        t.page
            .route_web_socket(
                oxidewright::regex(".*", ""),
                |route: WebSocketRoute| async move {
                    let server = route.connect_to_server().unwrap();
                    let s = server.clone();
                    route.on_message(move |m| s.send(m));
                    let r = route.clone();
                    server.on_message(move |m| r.send(m));
                },
            )
            .await
            .unwrap();
        setup_ws(&t.page, &t.server.empty_page, &ws, Value::Null).await;
        poll_page(&t.page, &["open".to_string()]).await;
        ws.send("hello").await;
        poll_page(&t.page, &["open".to_string(), msg(&ws, "hello")]).await;
        let _: Value = t.page.evaluate("() => window.ws.send('hi')").await.unwrap();
        poll_server(&ws, &["message: hi"]).await;
    });
}

#[test]
fn ws_026_should_pattern_match() {
    run(|t| async move {
        let ws = WsServer::start().await;
        t.page
            .route_web_socket(
                oxidewright::regex(r".*/ws$", ""),
                |route: WebSocketRoute| async move {
                    route.connect_to_server().unwrap();
                },
            )
            .await
            .unwrap();
        t.page
            .route_web_socket("**/mock-ws", |route: WebSocketRoute| async move {
                let r = route.clone();
                route.on_message(move |_| r.send("mock-response"));
            })
            .await
            .unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        let _: Value = t
            .page
            .evaluate(
                r#"async ({ host }) => {
    window.log = [];
    window.ws1 = new WebSocket('ws://' + host + '/ws');
    window.ws1.addEventListener('message', event => window.log.push(`ws1:${event.data}`));
    window.ws2 = new WebSocket('ws://' + host + '/something/something/mock-ws');
    window.ws2.addEventListener('message', event => window.log.push(`ws2:${event.data}`));
    await Promise.all([
      new Promise(f => window.ws1.addEventListener('open', f)),
      new Promise(f => window.ws2.addEventListener('open', f)),
    ]);
  }"#,
            )
            .arg(serde_json::json!({ "host": ws.host() }))
            .await
            .unwrap();
        let _: Value = t
            .page
            .evaluate("() => window.ws1.send('request')")
            .await
            .unwrap();
        poll_server(&ws, &["message: request"]).await;
        ws.send("response").await;
        poll_page(&t.page, &["ws1:response".to_string()]).await;
        let _: Value = t
            .page
            .evaluate("() => window.ws2.send('request')")
            .await
            .unwrap();
        poll_page(
            &t.page,
            &["ws1:response".to_string(), "ws2:mock-response".to_string()],
        )
        .await;
    });
}

#[test]
fn ws_027_should_work_with_server() {
    run(|t| async move {
        let ws = WsServer::start().await;
        let saved: Arc<Mutex<Option<WebSocketRoute>>> = Arc::default();
        let s = saved.clone();
        t.page
            .route_web_socket(
                oxidewright::regex(".*", ""),
                move |route: WebSocketRoute| {
                    let s = s.clone();
                    async move {
                        let server = route.connect_to_server().unwrap();
                        let (r, sv) = (route.clone(), server.clone());
                        route.on_message(move |m| match text(&m).as_str() {
                            "to-respond" => r.send("response"),
                            "to-block" => {}
                            "to-modify" => sv.send("modified"),
                            _ => sv.send(m),
                        });
                        let r = route.clone();
                        server.on_message(move |m| match text(&m).as_str() {
                            "to-block" => {}
                            "to-modify" => r.send("modified"),
                            _ => r.send(m),
                        });
                        server.send("fake");
                        *s.lock().unwrap() = Some(route);
                    }
                },
            )
            .await
            .unwrap();
        setup_ws(&t.page, &t.server.empty_page, &ws, Value::Null).await;
        poll_server(&ws, &["message: fake"]).await;
        ws.send("to-modify").await;
        ws.send("to-block").await;
        ws.send("pass-server").await;
        let mut expected = vec![
            "open".to_string(),
            msg(&ws, "modified"),
            msg(&ws, "pass-server"),
        ];
        poll_page(&t.page, &expected).await;
        let _: Value = t
            .page
            .evaluate(
                "() => { window.ws.send('to-respond'); window.ws.send('to-modify'); window.ws.send('to-block'); window.ws.send('pass-client'); }",
            )
            .await
            .unwrap();
        poll_server(
            &ws,
            &["message: fake", "message: modified", "message: pass-client"],
        )
        .await;
        expected.push(msg(&ws, "response"));
        poll_page(&t.page, &expected).await;
        let route = saved.lock().unwrap().clone().unwrap();
        route.send("another");
        expected.push(msg(&ws, "another"));
        poll_page(&t.page, &expected).await;
        let _: Value = t
            .page
            .evaluate("() => window.ws.close(3009, 'problem')")
            .await
            .unwrap();
        poll_server(
            &ws,
            &[
                "message: fake",
                "message: modified",
                "message: pass-client",
                "close: code=3009 reason=problem",
            ],
        )
        .await;
    });
}

#[test]
fn ws_028_should_work_without_server() {
    run(|t| async move {
        let ws = WsServer::start().await;
        let saved: Arc<Mutex<Option<WebSocketRoute>>> = Arc::default();
        let s = saved.clone();
        t.page
            .route_web_socket(
                oxidewright::regex(".*", ""),
                move |route: WebSocketRoute| {
                    let s = s.clone();
                    async move {
                        let r = route.clone();
                        route.on_message(move |m| {
                            if text(&m) == "to-respond" {
                                r.send("response");
                            }
                        });
                        *s.lock().unwrap() = Some(route);
                    }
                },
            )
            .await
            .unwrap();
        setup_ws(&t.page, &t.server.empty_page, &ws, Value::Null).await;
        let _: Value = t
            .page
            .evaluate(
                "async () => { await window.wsOpened; window.ws.send('to-respond'); window.ws.send('to-block'); window.ws.send('to-respond'); }",
            )
            .await
            .unwrap();
        let mut expected = vec![
            "open".to_string(),
            msg(&ws, "response"),
            msg(&ws, "response"),
        ];
        poll_page(&t.page, &expected).await;
        let route = saved.lock().unwrap().clone().unwrap();
        route.send("another");
        route
            .close_with(Some(3008), Some("oops".to_string()))
            .await
            .unwrap();
        expected.push(msg(&ws, "another"));
        expected.push("close code=3008 reason=oops wasClean=true".to_string());
        poll_page(&t.page, &expected).await;
        // Never connected to the real server.
        assert!(ws.log().is_empty());
    });
}

#[test]
fn ws_030_should_emit_close_upon_frame_navigation() {
    run(|t| async move {
        let ws = WsServer::start().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<WebSocketRoute>();
        t.page
            .route_web_socket(
                oxidewright::regex(".*", ""),
                move |route: WebSocketRoute| {
                    let tx = tx.clone();
                    async move {
                        route.connect_to_server().unwrap();
                        let _ = tx.send(route);
                    }
                },
            )
            .await
            .unwrap();
        setup_ws(&t.page, &t.server.empty_page, &ws, Value::Null).await;
        let route = rx.recv().await.unwrap();
        route.send("hello");
        poll_page(&t.page, &["open".to_string(), msg(&ws, "hello")]).await;
        let (ctx, mut crx) = tokio::sync::mpsc::unbounded_channel::<()>();
        route.on_close(move |_, _| {
            let _ = ctx.send(());
        });
        t.page.goto(&t.server.empty_page).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), crx.recv())
            .await
            .unwrap();
    });
}

#[test]
fn ws_032_should_route_on_context() {
    run(|t| async move {
        let ws = WsServer::start().await;
        for reply in ["page-mock-1", "page-mock-2"] {
            t.page
                .route_web_socket(
                    oxidewright::regex("ws1", ""),
                    move |route: WebSocketRoute| async move {
                        let r = route.clone();
                        route.on_message(move |_| r.send(reply));
                    },
                )
                .await
                .unwrap();
        }
        t.context
            .route_web_socket(
                oxidewright::regex(".*", ""),
                |route: WebSocketRoute| async move {
                    let r = route.clone();
                    route.on_message(move |_| r.send("context-mock-1"));
                    let r = route.clone();
                    route.on_message(move |_| r.send("context-mock-2"));
                },
            )
            .await
            .unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        let _: Value = t
            .page
            .evaluate(
                r#"({ host }) => {
    window.log = [];
    window.ws1 = new WebSocket('ws://' + host + '/ws1');
    window.ws1.addEventListener('message', event => window.log.push(`ws1:${event.data}`));
    window.ws2 = new WebSocket('ws://' + host + '/ws2');
    window.ws2.addEventListener('message', event => window.log.push(`ws2:${event.data}`));
  }"#,
            )
            .arg(serde_json::json!({ "host": ws.host() }))
            .await
            .unwrap();
        let _: Value = t
            .page
            .evaluate("async () => { await new Promise(f => window.ws1.readyState === 1 ? f() : window.ws1.addEventListener('open', f)); window.ws1.send('request'); }")
            .await
            .unwrap();
        poll_page(&t.page, &["ws1:page-mock-2".to_string()]).await;
        let _: Value = t
            .page
            .evaluate("async () => { await new Promise(f => window.ws2.readyState === 1 ? f() : window.ws2.addEventListener('open', f)); window.ws2.send('request'); }")
            .await
            .unwrap();
        poll_page(
            &t.page,
            &[
                "ws1:page-mock-2".to_string(),
                "ws2:context-mock-2".to_string(),
            ],
        )
        .await;
    });
}

#[test]
fn ws_034_should_not_throw_with_empty_handler() {
    run(|t| async move {
        let ws = WsServer::start().await;
        t.page
            .route_web_socket(
                oxidewright::regex(".*", ""),
                |_route: WebSocketRoute| async move {},
            )
            .await
            .unwrap();
        setup_ws(&t.page, &t.server.empty_page, &ws, Value::Null).await;
        poll_page(&t.page, &["open".to_string()]).await;
        let _: Value = t.page.evaluate("() => window.ws.send('hi')").await.unwrap();
        let _: Value = t
            .page
            .evaluate("() => window.ws.send('hi2')")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(page_log(&t.page).await, vec!["open".to_string()]);
    });
}

#[test]
fn ws_035_should_throw_when_connecting_twice() {
    run(|t| async move {
        let ws = WsServer::start().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        t.page
            .route_web_socket(
                oxidewright::regex(".*", ""),
                move |route: WebSocketRoute| {
                    let tx = tx.clone();
                    async move {
                        route.connect_to_server().unwrap();
                        if let Err(e) = route.connect_to_server() {
                            let _ = tx.send(e.to_string());
                        }
                    }
                },
            )
            .await
            .unwrap();
        setup_ws(&t.page, &t.server.empty_page, &ws, Value::Null).await;
        let error = rx.recv().await.unwrap();
        assert!(error.contains("Already connected to the server"), "{error}");
    });
}

#[test]
fn ws_039_should_expose_protocols_to_the_route_handler() {
    run(|t| async move {
        let ws = WsServer::start().await;
        let routes: Arc<Mutex<Vec<WebSocketRoute>>> = Arc::default();
        let r = routes.clone();
        t.page
            .route_web_socket(
                oxidewright::regex(".*", ""),
                move |route: WebSocketRoute| {
                    r.lock().unwrap().push(route);
                    async move {}
                },
            )
            .await
            .unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        let _: Value = t
            .page
            .evaluate(
                r#"({ host }) => {
    window.wsNone = new WebSocket('ws://' + host + '/ws-none');
    window.wsString = new WebSocket('ws://' + host + '/ws-string', 'chat.v1');
    window.wsArray = new WebSocket('ws://' + host + '/ws-array', ['chat.v2', 'chat.v1']);
  }"#,
            )
            .arg(serde_json::json!({ "host": ws.host() }))
            .await
            .unwrap();
        for _ in 0..200 {
            if routes.lock().unwrap().len() == 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let routes = routes.lock().unwrap().clone();
        assert_eq!(routes.len(), 3);
        let by = |path: &str| {
            routes
                .iter()
                .find(|r| r.url().ends_with(path))
                .unwrap()
                .protocols()
        };
        assert!(by("/ws-none").is_empty());
        assert_eq!(by("/ws-string"), vec!["chat.v1"]);
        assert_eq!(by("/ws-array"), vec!["chat.v2", "chat.v1"]);
    });
}

#[test]
fn ws_040_should_expose_protocols_on_server_side_route() {
    run(|t| async move {
        let ws = WsServer::start().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        t.page
            .route_web_socket(
                oxidewright::regex(".*", ""),
                move |route: WebSocketRoute| {
                    let tx = tx.clone();
                    async move {
                        let server = route.connect_to_server().unwrap();
                        let _ = tx.send((route.protocols(), server.protocols()));
                    }
                },
            )
            .await
            .unwrap();
        setup_ws(
            &t.page,
            &t.server.empty_page,
            &ws,
            serde_json::json!(["chat.v2", "chat.v1"]),
        )
        .await;
        let (page_side, server_side) = rx.recv().await.unwrap();
        assert_eq!(page_side, vec!["chat.v2", "chat.v1"]);
        assert_eq!(server_side, vec!["chat.v2", "chat.v1"]);
    });
}

/// While routing, the main world gets exactly what Patchright leaves:
/// `__pwWebSocketBinding` and `__pwWebSocketDispatch` (and `WebSocket`
/// replaced); nothing before `route_web_socket` is called.
#[test]
fn ws_stealth_main_world_footprint() {
    run(|t| async move {
        let names = "() => Object.getOwnPropertyNames(window).sort()";
        t.page.goto(&t.server.empty_page).await.unwrap();
        let before: Vec<String> = t.page.evaluate(names).await.unwrap();
        assert!(!before.iter().any(|n| n.starts_with("__pw")));
        t.page
            .route_web_socket("**/never", |_r: WebSocketRoute| async move {})
            .await
            .unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        let after: Vec<String> = t.page.evaluate(names).await.unwrap();
        let mut added: Vec<&String> = after.iter().filter(|n| !before.contains(n)).collect();
        added.sort();
        assert_eq!(added, vec!["__pwWebSocketBinding", "__pwWebSocketDispatch"]);
    });
}
