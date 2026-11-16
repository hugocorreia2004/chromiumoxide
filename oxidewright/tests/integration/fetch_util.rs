//! Helpers shared by the `APIRequestContext` tests.

#![allow(dead_code)]

use axum::body::Body;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use base64::Engine;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use crate::common::TestServer;

/// A request received by the test server.
#[derive(Debug, Clone)]
pub struct Captured {
    pub method: String,
    pub uri: String,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Captured {
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .map(|v| v.to_str().unwrap_or_default().to_string())
    }
}

/// Serve `path` with `status`/`body` and report every request to it.
pub fn capture(
    server: &TestServer,
    path: &str,
    status: u16,
    body: &'static str,
) -> UnboundedReceiver<Captured> {
    let (tx, rx) = unbounded_channel();
    server.set_route(path, move |req: axum::extract::Request| {
        let tx = tx.clone();
        async move {
            let method = req.method().to_string();
            let uri = req.uri().to_string();
            let headers = req.headers().clone();
            let bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
                .await
                .unwrap_or_default()
                .to_vec();
            let _ = tx.send(Captured {
                method,
                uri,
                headers,
                body: bytes,
            });
            Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, "text/plain")
                .body(Body::from(body))
                .unwrap()
        }
    });
    rx
}

/// Playwright's `server.setAuth`.
pub fn set_auth(server: &TestServer, path: &str, username: &str, password: &str) {
    let expected = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"))
    );
    server.set_route(path, move |req: axum::extract::Request| {
        let ok = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            == Some(expected.as_str());
        async move {
            if ok {
                Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::empty())
                    .unwrap()
            } else {
                Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .header(header::WWW_AUTHENTICATE, "Basic realm=\"Secure Area\"")
                    .body(Body::from("HTTP Error 401 Unauthorized: Access is denied"))
                    .unwrap()
            }
        }
    });
}
