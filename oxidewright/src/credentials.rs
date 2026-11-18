//! `context.credentials` (Playwright 1.65's `Credentials`): WebAuthn
//! passkeys.
//!
//! Playwright replaces `navigator.credentials` with an injected main-world
//! mock (`webAuthn.ts`, global `__pwWebAuthnInstalled`, binding
//! `__pwWebAuthnBinding`) and signs in Node. oxidewright uses Chromium's own
//! virtual authenticator instead (CDP `WebAuthn` domain, one authenticator per
//! page once [`Credentials::install`] was called): nothing is injected in the
//! page, `navigator.credentials` stays native, and the ceremonies (keys,
//! signatures, sign counters, attestation) are Chromium's. The credential
//! registry and its format (base64url ids, PKCS#8 private key, SPKI public
//! key, `signCount`) follow Playwright's.

use std::collections::HashMap;
use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::browser_context::BrowserContext;
use crate::cdp_raw::RawCommand;
use crate::error::{Error, Result, ResultExt};
use crate::page::Page;

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// A WebAuthn credential (Playwright's `VirtualCredential`). Binary fields
/// are base64url without padding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualCredential {
    /// Credential id.
    pub id: String,
    /// Relying party id (a domain).
    pub rp_id: String,
    /// User handle.
    pub user_handle: String,
    /// DER PKCS#8 private key (P-256).
    pub private_key: String,
    /// DER SPKI public key.
    pub public_key: String,
    /// Signature counter.
    #[serde(default)]
    pub sign_count: u32,
}

/// Context-level credential state.
#[derive(Default)]
pub(crate) struct CredentialsState {
    registry: Mutex<Vec<VirtualCredential>>,
    installed: AtomicBool,
    /// Page target id → authenticator id.
    authenticators: Mutex<HashMap<String, (Page, String)>>,
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn b64url_decode(s: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.trim_end_matches('='))
        .map_err(|e| Error::invalid_argument(format!("invalid base64url: {e}")))
}

fn b64_std(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn b64_std_decode(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .unwrap_or_default()
}

fn random_b64url(len: usize) -> String {
    let bytes: Vec<u8> = (0..len).map(|_| fastrand::u8(..)).collect();
    b64url(&bytes)
}

/// DER SPKI prefix of an uncompressed P-256 public key.
const P256_SPKI_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

fn spki_from_pkcs8(pkcs8: &[u8]) -> Option<Vec<u8>> {
    use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
    let rng = ring::rand::SystemRandom::new();
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8, &rng).ok()?;
    let mut out = P256_SPKI_PREFIX.to_vec();
    out.extend_from_slice(pair.public_key().as_ref());
    Some(out)
}

fn generate_key_pair() -> Result<(Vec<u8>, Vec<u8>)> {
    use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair};
    let rng = ring::rand::SystemRandom::new();
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng)
        .map_err(|_| Error::invalid_argument("failed to generate a P-256 key pair"))?;
    let pkcs8 = pkcs8.as_ref().to_vec();
    let spki = spki_from_pkcs8(&pkcs8)
        .ok_or_else(|| Error::invalid_argument("failed to generate a P-256 key pair"))?;
    Ok((pkcs8, spki))
}

/// CDP `WebAuthn.Credential` of a registry entry.
fn to_cdp(c: &VirtualCredential) -> Result<Value> {
    Ok(json!({
        "credentialId": b64_std(&b64url_decode(&c.id)?),
        "isResidentCredential": true,
        "rpId": c.rp_id,
        "privateKey": b64_std(&b64url_decode(&c.private_key)?),
        "userHandle": b64_std(&b64url_decode(&c.user_handle)?),
        "signCount": c.sign_count,
    }))
}

fn from_cdp(v: &Value) -> Option<VirtualCredential> {
    let private_key = b64_std_decode(v.get("privateKey")?.as_str()?);
    Some(VirtualCredential {
        id: b64url(&b64_std_decode(v.get("credentialId")?.as_str()?)),
        rp_id: v
            .get("rpId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        user_handle: b64url(&b64_std_decode(
            v.get("userHandle")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )),
        public_key: spki_from_pkcs8(&private_key)
            .map(|k| b64url(&k))
            .unwrap_or_default(),
        private_key: b64url(&private_key),
        sign_count: v.get("signCount").and_then(Value::as_u64).unwrap_or(0) as u32,
    })
}

async fn send(page: &Page, method: &str, params: Value) -> Result<Value> {
    Ok(page
        .inner
        .cdp
        .execute(RawCommand::new(method, params))
        .await?
        .result)
}

/// The WebAuthn credentials of a context (`context.credentials`).
#[derive(Clone, Debug)]
pub struct Credentials {
    context: BrowserContext,
}

impl BrowserContext {
    /// The context's WebAuthn credentials (Playwright 1.65's
    /// `context.credentials`).
    pub fn credentials(&self) -> Credentials {
        Credentials {
            context: self.clone(),
        }
    }
}

impl Credentials {
    fn state(&self) -> &CredentialsState {
        &self.context.inner.ext.credentials
    }

    /// Add a credential for `rp_id`; missing fields are generated (a new
    /// P-256 key pair, random id and user handle). `.from(&credential)`
    /// imports one returned by [`Credentials::get`].
    pub fn create(&self, rp_id: impl Into<String>) -> CreateCredential {
        CreateCredential {
            credentials: self.clone(),
            rp_id: rp_id.into(),
            id: None,
            user_handle: None,
            private_key: None,
            public_key: None,
            sign_count: None,
        }
    }

    /// The credentials, optionally filtered with `.rp_id(..)` / `.id(..)`.
    /// Includes the ones registered by pages (`navigator.credentials.create`)
    /// and the current sign counters.
    pub fn get(&self) -> GetCredentials {
        GetCredentials {
            credentials: self.clone(),
            rp_id: None,
            id: None,
        }
    }

    /// Delete a credential by id.
    pub async fn delete(&self, id: &str) -> Result<()> {
        self.sync().await;
        self.state().registry.lock().unwrap().retain(|c| c.id != id);
        let raw_id = b64_std(&b64url_decode(id).unwrap_or_default());
        for (page, authenticator) in self.authenticators() {
            let _ = send(
                &page,
                "WebAuthn.removeCredential",
                json!({ "authenticatorId": authenticator, "credentialId": raw_id }),
            )
            .await;
        }
        Ok(())
    }

    /// Start answering WebAuthn ceremonies of the context's pages (current
    /// and future) with the registered credentials.
    pub async fn install(&self) -> Result<()> {
        async {
            if self.state().installed.swap(true, Ordering::SeqCst) {
                return Ok(());
            }
            for page in self.context.pages() {
                setup_page(&self.context, &page).await?;
            }
            Ok(())
        }
        .await
        .api("credentials.install")
    }

    fn authenticators(&self) -> Vec<(Page, String)> {
        let mut map = self.state().authenticators.lock().unwrap();
        map.retain(|_, (page, _)| !page.is_closed());
        map.values().cloned().collect()
    }

    /// Merge what the authenticators know (sign counters, credentials
    /// created by pages) into the registry.
    async fn sync(&self) {
        for (page, authenticator) in self.authenticators() {
            let Ok(result) = send(
                &page,
                "WebAuthn.getCredentials",
                json!({ "authenticatorId": authenticator }),
            )
            .await
            else {
                continue;
            };
            let found: Vec<VirtualCredential> = result
                .get("credentials")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(from_cdp).collect())
                .unwrap_or_default();
            let mut registry = self.state().registry.lock().unwrap();
            for credential in found {
                match registry.iter_mut().find(|c| c.id == credential.id) {
                    Some(existing) => {
                        existing.sign_count = existing.sign_count.max(credential.sign_count)
                    }
                    None => registry.push(credential),
                }
            }
        }
    }

    async fn add(&self, credential: VirtualCredential) -> Result<()> {
        self.state()
            .registry
            .lock()
            .unwrap()
            .retain(|c| c.id != credential.id);
        self.state()
            .registry
            .lock()
            .unwrap()
            .push(credential.clone());
        let cdp = to_cdp(&credential)?;
        for (page, authenticator) in self.authenticators() {
            let _ = send(
                &page,
                "WebAuthn.addCredential",
                json!({ "authenticatorId": authenticator, "credential": cdp }),
            )
            .await;
        }
        Ok(())
    }
}

/// A page of the context: give it an authenticator if `install` was called.
pub(crate) async fn init_page(page: &Page, context: &BrowserContext) -> Result<()> {
    if context
        .inner
        .ext
        .credentials
        .installed
        .load(Ordering::SeqCst)
    {
        setup_page(context, page).await?;
    }
    Ok(())
}

async fn setup_page(context: &BrowserContext, page: &Page) -> Result<()> {
    let credentials = context.credentials();
    credentials.sync().await;
    send(page, "WebAuthn.enable", json!({ "enableUI": false })).await?;
    let result = send(
        page,
        "WebAuthn.addVirtualAuthenticator",
        json!({ "options": {
            "protocol": "ctap2",
            "transport": "internal",
            "hasResidentKey": true,
            "hasUserVerification": true,
            "isUserVerified": true,
            "automaticPresenceSimulation": true,
        } }),
    )
    .await?;
    let authenticator = result
        .get("authenticatorId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let registry = context
        .inner
        .ext
        .credentials
        .registry
        .lock()
        .unwrap()
        .clone();
    for credential in &registry {
        send(
            page,
            "WebAuthn.addCredential",
            json!({ "authenticatorId": authenticator, "credential": to_cdp(credential)? }),
        )
        .await?;
    }
    context
        .inner
        .ext
        .credentials
        .authenticators
        .lock()
        .unwrap()
        .insert(
            page.inner.cdp.target_id().as_ref().to_string(),
            (page.clone(), authenticator),
        );
    Ok(())
}

/// Builder returned by [`Credentials::create`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct CreateCredential {
    credentials: Credentials,
    rp_id: String,
    id: Option<String>,
    user_handle: Option<String>,
    private_key: Option<String>,
    public_key: Option<String>,
    sign_count: Option<i64>,
}

impl CreateCredential {
    /// Credential id (base64url); random by default.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// User handle (base64url); random by default.
    pub fn user_handle(mut self, user_handle: impl Into<String>) -> Self {
        self.user_handle = Some(user_handle.into());
        self
    }

    /// Private key (base64url DER PKCS#8, P-256); generated by default.
    pub fn private_key(mut self, private_key: impl Into<String>) -> Self {
        self.private_key = Some(private_key.into());
        self
    }

    /// Public key (base64url DER SPKI); generated with the private key.
    pub fn public_key(mut self, public_key: impl Into<String>) -> Self {
        self.public_key = Some(public_key.into());
        self
    }

    /// Initial signature counter (0 to 4294967295, default 0).
    pub fn sign_count(mut self, sign_count: i64) -> Self {
        self.sign_count = Some(sign_count);
        self
    }

    /// Take id, user handle, keys and counter from an existing credential
    /// (Playwright's `credentials.create(rpId, credential)`).
    pub fn from(mut self, credential: &VirtualCredential) -> Self {
        self.id = Some(credential.id.clone());
        self.user_handle = Some(credential.user_handle.clone());
        self.private_key = Some(credential.private_key.clone());
        self.public_key = Some(credential.public_key.clone());
        self.sign_count = Some(credential.sign_count as i64);
        self
    }
}

impl IntoFuture for CreateCredential {
    type Output = Result<VirtualCredential>;
    type IntoFuture = BoxFuture<Result<VirtualCredential>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            async {
                let sign_count = self.sign_count.unwrap_or(0);
                if !(0..=0xffff_ffff).contains(&sign_count) {
                    return Err(Error::invalid_argument(format!(
                        "signCount must be between 0 and 4294967295, got {sign_count}"
                    )));
                }
                let (private_key, public_key) = match (self.private_key, self.public_key) {
                    (Some(private_key), Some(public_key)) => (private_key, public_key),
                    _ => {
                        let (pkcs8, spki) = generate_key_pair()?;
                        (b64url(&pkcs8), b64url(&spki))
                    }
                };
                let credential = VirtualCredential {
                    id: self.id.unwrap_or_else(|| random_b64url(16)),
                    rp_id: self.rp_id,
                    user_handle: self.user_handle.unwrap_or_else(|| random_b64url(16)),
                    private_key,
                    public_key,
                    sign_count: sign_count as u32,
                };
                self.credentials.add(credential.clone()).await?;
                Ok(credential)
            }
            .await
            .api("credentials.create")
        })
    }
}

/// Builder returned by [`Credentials::get`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct GetCredentials {
    credentials: Credentials,
    rp_id: Option<String>,
    id: Option<String>,
}

impl GetCredentials {
    /// Only credentials of this relying party.
    pub fn rp_id(mut self, rp_id: impl Into<String>) -> Self {
        self.rp_id = Some(rp_id.into());
        self
    }

    /// Only the credential with this id.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }
}

impl IntoFuture for GetCredentials {
    type Output = Result<Vec<VirtualCredential>>;
    type IntoFuture = BoxFuture<Result<Vec<VirtualCredential>>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            self.credentials.sync().await;
            let registry = self.credentials.state().registry.lock().unwrap().clone();
            Ok(registry
                .into_iter()
                .filter(|c| self.rp_id.as_ref().is_none_or(|r| &c.rp_id == r))
                .filter(|c| self.id.as_ref().is_none_or(|i| &c.id == i))
                .collect())
        })
    }
}
