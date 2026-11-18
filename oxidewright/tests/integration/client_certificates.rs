//! TLS client certificates of `APIRequestContext`, ported from Playwright's
//! `tests/library/client-certificates.spec.ts` (CC-001..CC-006). The
//! certificates in `tests/assets/client-certificates` are test-only.

use std::path::PathBuf;
use std::sync::Arc;

use oxidewright::ClientCertificate;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::common::*;

fn asset(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("assets")
        .join("client-certificates")
        .join(name)
}

/// An HTTPS server that requires a client certificate signed by the test
/// CA and answers with the certificate's subject. Returns its origin.
async fn start_cc_server() -> String {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let certs: Vec<CertificateDer<'static>> =
        CertificateDer::pem_file_iter(asset("server_cert.pem"))
            .unwrap()
            .map(|c| c.unwrap())
            .collect();
    let key = PrivateKeyDer::from_pem_file(asset("server_key.pem")).unwrap();
    let mut roots = rustls::RootCertStore::empty();
    for ca in CertificateDer::pem_file_iter(asset("ca_cert.pem")).unwrap() {
        roots.add(ca.unwrap()).unwrap();
    }
    let verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .unwrap();
    let config = rustls::ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(certs, key)
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut buf = vec![0u8; 4096];
                let mut read = 0;
                while !buf[..read].windows(4).any(|w| w == b"\r\n\r\n") {
                    match tls.read(&mut buf[read..]).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => read += n,
                    }
                }
                let body = "Hello Alice, your certificate was issued by the test CA!";
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = tls.write_all(response.as_bytes()).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    format!("https://localhost:{port}")
}

fn trusted(origin: &str) -> ClientCertificate {
    ClientCertificate::new(origin)
        .cert_path(asset("client_cert.pem"))
        .key_path(asset("client_key.pem"))
}

#[test]
fn cc_001_validate_input() {
    run(|_t| async move {
        let error = oxidewright::request()
            .new_context()
            .client_certificates([ClientCertificate::new("test")])
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("clientCertificates.origin is required")
                || error
                    .to_string()
                    .contains("None of cert, key, passphrase or pfx is specified"),
            "{error}"
        );
        let error = oxidewright::request()
            .new_context()
            .client_certificates([ClientCertificate::new("https://example.com")])
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("None of cert, key, passphrase or pfx is specified"),
            "{error}"
        );
    });
}

#[test]
fn cc_002_should_fail_with_no_client_certificates_provided() {
    run(|_t| async move {
        let origin = start_cc_server().await;
        let request = oxidewright::request()
            .new_context()
            .ignore_https_errors(true)
            .await
            .unwrap();
        assert!(request.get(&origin).await.is_err());
        request.dispose().await.unwrap();
    });
}

#[test]
fn cc_003_should_keep_supporting_http() {
    run(|t| async move {
        let request = oxidewright::request()
            .new_context()
            .ignore_https_errors(true)
            .client_certificates([trusted(&t.server.prefix)])
            .await
            .unwrap();
        let response = request.get(&t.server.empty_page).await.unwrap();
        assert_eq!(response.status(), 200);
        request.dispose().await.unwrap();
    });
}

#[test]
fn cc_004_should_throw_with_untrusted_client_certs() {
    run(|_t| async move {
        let origin = start_cc_server().await;
        let request = oxidewright::request()
            .new_context()
            .ignore_https_errors(true)
            .client_certificates([ClientCertificate::new(&origin)
                .cert_path(asset("self_signed_cert.pem"))
                .key_path(asset("self_signed_key.pem"))])
            .await
            .unwrap();
        assert!(request.get(&origin).await.is_err());
        request.dispose().await.unwrap();
    });
}

#[test]
fn cc_005_pass_with_trusted_client_certificates() {
    run(|_t| async move {
        let origin = start_cc_server().await;
        let request = oxidewright::request()
            .new_context()
            .ignore_https_errors(true)
            .client_certificates([trusted(&origin)])
            .await
            .unwrap();
        let response = request.get(&origin).await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.text().await.unwrap(),
            "Hello Alice, your certificate was issued by the test CA!"
        );
        request.dispose().await.unwrap();
    });
}

#[test]
fn cc_certificate_is_only_used_for_its_origin() {
    run(|_t| async move {
        let origin = start_cc_server().await;
        // A certificate for another origin is not presented.
        let request = oxidewright::request()
            .new_context()
            .ignore_https_errors(true)
            .client_certificates([trusted("https://example.com")])
            .await
            .unwrap();
        assert!(request.get(&origin).await.is_err());
        request.dispose().await.unwrap();
    });
}
