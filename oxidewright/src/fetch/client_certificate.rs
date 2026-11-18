//! TLS client certificates (Playwright's `clientCertificates` option), for
//! [`super::APIRequestContext`] requests.

use std::path::PathBuf;

use crate::error::{Error, Result};

/// A TLS client certificate presented to one origin (Playwright's
/// `ClientCertificate`). Give the certificate and its private key in PEM
/// (bytes or files); PKCS#12 (`pfx`) and encrypted keys are not supported
/// yet.
#[derive(Debug, Clone, Default)]
pub struct ClientCertificate {
    /// Exact origin the certificate is valid for, e.g. `https://example.com:8443`.
    pub origin: String,
    /// The certificate chain, PEM.
    pub cert: Option<Vec<u8>>,
    /// Path of the certificate chain, PEM.
    pub cert_path: Option<PathBuf>,
    /// The private key, PEM.
    pub key: Option<Vec<u8>>,
    /// Path of the private key, PEM.
    pub key_path: Option<PathBuf>,
    /// PKCS#12 bundle (not supported yet).
    pub pfx: Option<Vec<u8>>,
    /// Path of a PKCS#12 bundle (not supported yet).
    pub pfx_path: Option<PathBuf>,
    /// Passphrase of the private key or bundle (not supported yet).
    pub passphrase: Option<String>,
}

impl ClientCertificate {
    /// A certificate for `origin`; add `cert`/`key` (or their paths).
    pub fn new(origin: impl Into<String>) -> Self {
        Self {
            origin: origin.into(),
            ..Default::default()
        }
    }

    /// The certificate chain file (PEM).
    pub fn cert_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.cert_path = Some(path.into());
        self
    }

    /// The private key file (PEM).
    pub fn key_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.key_path = Some(path.into());
        self
    }

    /// The certificate chain (PEM bytes).
    pub fn cert(mut self, pem: impl Into<Vec<u8>>) -> Self {
        self.cert = Some(pem.into());
        self
    }

    /// The private key (PEM bytes).
    pub fn key(mut self, pem: impl Into<Vec<u8>>) -> Self {
        self.key = Some(pem.into());
        self
    }

    /// The normalized origin (`scheme://host[:port]`) this certificate is for.
    pub(crate) fn normalized_origin(&self) -> Option<String> {
        url::Url::parse(&self.origin)
            .ok()
            .map(|u| u.origin().ascii_serialization())
    }

    /// Whether it applies to a request to `url`.
    pub(crate) fn matches(&self, url: &str) -> bool {
        let Some(origin) = self.normalized_origin() else {
            return false;
        };
        url::Url::parse(url).is_ok_and(|u| u.origin().ascii_serialization() == origin)
    }

    /// The PEM bundle (chain + key) for the HTTP client.
    pub(crate) async fn identity_pem(&self) -> Result<Vec<u8>> {
        async fn read(bytes: &Option<Vec<u8>>, path: &Option<PathBuf>) -> Result<Option<Vec<u8>>> {
            if let Some(bytes) = bytes {
                return Ok(Some(bytes.clone()));
            }
            match path {
                Some(path) => tokio::fs::read(path)
                    .await
                    .map(Some)
                    .map_err(|e| Error::invalid_argument(format!("{}: {e}", path.display()))),
                None => Ok(None),
            }
        }
        if self.pfx.is_some() || self.pfx_path.is_some() {
            return Err(Error::invalid_argument(
                "PKCS#12 (pfx) client certificates are not supported yet: use cert and key (PEM)",
            ));
        }
        if self.passphrase.is_some() {
            return Err(Error::invalid_argument(
                "Encrypted client certificate keys (passphrase) are not supported yet",
            ));
        }
        let cert = read(&self.cert, &self.cert_path).await?;
        let key = read(&self.key, &self.key_path).await?;
        match (cert, key) {
            (Some(mut cert), Some(key)) => {
                cert.push(b'\n');
                cert.extend(key);
                Ok(cert)
            }
            _ => Err(Error::invalid_argument(
                "None of cert, key, passphrase or pfx is specified",
            )),
        }
    }
}

/// Playwright's `verifyClientCertificates`.
pub(crate) fn validate(certificates: &[ClientCertificate]) -> Result<()> {
    for cert in certificates {
        if cert.origin.is_empty() || cert.normalized_origin().is_none() {
            return Err(Error::invalid_argument(
                "clientCertificates.origin is required",
            ));
        }
        let pem = cert.cert.is_some()
            || cert.cert_path.is_some()
            || cert.key.is_some()
            || cert.key_path.is_some();
        let pfx = cert.pfx.is_some() || cert.pfx_path.is_some();
        if !pem && !pfx && cert.passphrase.is_none() {
            return Err(Error::invalid_argument(
                "None of cert, key, passphrase or pfx is specified",
            ));
        }
        if pem && pfx {
            return Err(Error::invalid_argument(
                "pfx is specified together with cert, key or passphrase",
            ));
        }
    }
    Ok(())
}
