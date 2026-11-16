//! Cookies of a [`BrowserContext`]: `cookies`, `add_cookies`, `clear_cookies`
//! (Playwright's `server/network.ts` cookie helpers and
//! `CRBrowserContext.doGetCookies/addCookies/doClearCookies`).

use std::future::{Future, IntoFuture};
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::browser_context::BrowserContext;
use crate::cdp_raw::RawCommand;
use crate::error::{Error, Result, ResultExt};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// `253402300799` = Fri, 31 Dec 9999 23:59:59 +0000 (UTC).
pub(crate) const MAX_COOKIE_EXPIRES: f64 = 253_402_300_799.0;

/// The `SameSite` attribute of a cookie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SameSite {
    /// `Strict`
    Strict,
    /// `Lax` (the default when the attribute is missing)
    #[default]
    Lax,
    /// `None`
    None,
}

impl SameSite {
    /// `"Strict"`, `"Lax"` or `"None"`.
    pub fn as_str(&self) -> &'static str {
        match self {
            SameSite::Strict => "Strict",
            SameSite::Lax => "Lax",
            SameSite::None => "None",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<SameSite> {
        match value.to_ascii_lowercase().as_str() {
            "strict" => Some(SameSite::Strict),
            "lax" => Some(SameSite::Lax),
            "none" => Some(SameSite::None),
            _ => None,
        }
    }
}

fn default_expires() -> f64 {
    -1.0
}

/// Integral values are written without a fraction, like JavaScript does.
fn serialize_expires<S: serde::Serializer>(value: &f64, s: S) -> Result<S::Ok, S::Error> {
    if value.fract() == 0.0 && value.abs() < 9.0e15 {
        s.serialize_i64(*value as i64)
    } else {
        s.serialize_f64(*value)
    }
}

/// A cookie as returned by [`BrowserContext::cookies`] (Playwright's
/// `NetworkCookie`). Serializes like Playwright's storage state entries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cookie {
    /// Cookie name.
    pub name: String,
    /// Cookie value.
    pub value: String,
    /// Domain; a leading dot means the cookie applies to subdomains too.
    pub domain: String,
    /// Path.
    pub path: String,
    /// Unix time in seconds, `-1` for session cookies.
    #[serde(default = "default_expires", serialize_with = "serialize_expires")]
    pub expires: f64,
    /// `HttpOnly` attribute.
    #[serde(default)]
    pub http_only: bool,
    /// `Secure` attribute.
    #[serde(default)]
    pub secure: bool,
    /// `SameSite` attribute.
    #[serde(default)]
    pub same_site: SameSite,
    /// Top-level site of a partitioned (CHIPS) cookie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition_key: Option<String>,
    /// Chromium-specific: whether a partitioned cookie has a cross-site ancestor.
    #[serde(
        default,
        rename = "_crHasCrossSiteAncestor",
        skip_serializing_if = "Option::is_none"
    )]
    pub cr_has_cross_site_ancestor: Option<bool>,
}

/// A cookie to add with [`BrowserContext::add_cookies`]. Either `url` or
/// `domain` + `path` is required.
///
/// ```
/// use oxidewright::SetCookie;
/// let cookie = SetCookie::new("name", "value").url("https://example.com");
/// let other = SetCookie::new("name", "value").domain(".example.com").path("/");
/// # let _ = (cookie, other);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetCookie {
    /// Cookie name.
    pub name: String,
    /// Cookie value.
    pub value: String,
    /// URL the cookie applies to (sets domain, path and secure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Domain; a leading dot makes the cookie apply to subdomains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// Path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Unix time in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<f64>,
    /// `HttpOnly` attribute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_only: Option<bool>,
    /// `Secure` attribute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secure: Option<bool>,
    /// `SameSite` attribute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_site: Option<SameSite>,
    /// Top-level site for a partitioned (CHIPS) cookie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition_key: Option<String>,
    /// Chromium-specific, see [`Cookie::cr_has_cross_site_ancestor`].
    #[serde(
        default,
        rename = "_crHasCrossSiteAncestor",
        skip_serializing_if = "Option::is_none"
    )]
    pub cr_has_cross_site_ancestor: Option<bool>,
}

impl SetCookie {
    /// A cookie with a name and a value; set `url` or `domain` + `path`.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            ..Default::default()
        }
    }

    /// URL the cookie applies to.
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    /// Cookie domain.
    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// Cookie path.
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Expiration as a Unix time in seconds.
    pub fn expires(mut self, expires: f64) -> Self {
        self.expires = Some(expires);
        self
    }

    /// `HttpOnly` attribute.
    pub fn http_only(mut self, http_only: bool) -> Self {
        self.http_only = Some(http_only);
        self
    }

    /// `Secure` attribute.
    pub fn secure(mut self, secure: bool) -> Self {
        self.secure = Some(secure);
        self
    }

    /// `SameSite` attribute.
    pub fn same_site(mut self, same_site: SameSite) -> Self {
        self.same_site = Some(same_site);
        self
    }

    /// Partition key (top-level site) of a partitioned cookie.
    pub fn partition_key(mut self, key: impl Into<String>) -> Self {
        self.partition_key = Some(key.into());
        self
    }
}

impl From<Cookie> for SetCookie {
    fn from(c: Cookie) -> Self {
        SetCookie {
            name: c.name,
            value: c.value,
            url: None,
            domain: Some(c.domain),
            path: Some(c.path),
            expires: Some(c.expires),
            http_only: Some(c.http_only),
            secure: Some(c.secure),
            same_site: Some(c.same_site),
            partition_key: c.partition_key,
            cr_has_cross_site_ancestor: c.cr_has_cross_site_ancestor,
        }
    }
}

impl From<&Cookie> for SetCookie {
    fn from(c: &Cookie) -> Self {
        c.clone().into()
    }
}

/// A string or regular expression used to filter values (`name`, `domain`
/// and `path` of [`BrowserContext::clear_cookies`]).
#[derive(Debug, Clone)]
pub enum TextMatch {
    /// Exact string equality.
    Exact(String),
    /// Regular expression search.
    Regex(regex::Regex),
}

impl TextMatch {
    /// Whether `value` matches.
    pub fn matches(&self, value: &str) -> bool {
        match self {
            TextMatch::Exact(s) => s == value,
            TextMatch::Regex(re) => re.is_match(value),
        }
    }
}

impl From<&str> for TextMatch {
    fn from(s: &str) -> Self {
        TextMatch::Exact(s.to_string())
    }
}

impl From<String> for TextMatch {
    fn from(s: String) -> Self {
        TextMatch::Exact(s)
    }
}

impl From<&String> for TextMatch {
    fn from(s: &String) -> Self {
        TextMatch::Exact(s.clone())
    }
}

impl From<regex::Regex> for TextMatch {
    fn from(re: regex::Regex) -> Self {
        TextMatch::Regex(re)
    }
}

impl From<&regex::Regex> for TextMatch {
    fn from(re: &regex::Regex) -> Self {
        TextMatch::Regex(re.clone())
    }
}

/// Filters of `clear_cookies` (`network.ClearCookiesOptions`).
#[derive(Debug, Clone, Default)]
pub(crate) struct ClearCookiesFilter {
    pub(crate) name: Option<TextMatch>,
    pub(crate) domain: Option<TextMatch>,
    pub(crate) path: Option<TextMatch>,
}

impl ClearCookiesFilter {
    pub(crate) fn is_empty(&self) -> bool {
        self.name.is_none() && self.domain.is_none() && self.path.is_none()
    }

    pub(crate) fn matches(&self, cookie: &Cookie) -> bool {
        let check = |filter: &Option<TextMatch>, value: &str| {
            filter.as_ref().is_none_or(|f| f.matches(value))
        };
        check(&self.name, &cookie.name)
            && check(&self.domain, &cookie.domain)
            && check(&self.path, &cookie.path)
    }
}

// ----------------------------------------------------------------------
// Helpers (server/network.ts)
// ----------------------------------------------------------------------

/// `filterCookies`: keep the cookies sent with a request to any of `urls`
/// (all cookies when `urls` is empty).
pub(crate) fn filter_cookies(cookies: Vec<Cookie>, urls: &[String]) -> Result<Vec<Cookie>> {
    if urls.is_empty() {
        return Ok(cookies);
    }
    let parsed = urls
        .iter()
        .map(|u| {
            url::Url::parse(u).map_err(|_| Error::invalid_argument(format!("Invalid URL: {u}")))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(cookies
        .into_iter()
        .filter(|c| parsed.iter().any(|u| cookie_matches_url(c, u)))
        .collect())
}

/// Whether the cookie is sent with a request to `url` (RFC 6265 5.4).
pub(crate) fn cookie_matches_url(cookie: &Cookie, url: &url::Url) -> bool {
    let host = url_hostname(url);
    if !domain_matches(&host, &cookie.domain) {
        return false;
    }
    if !path_matches(url.path(), &cookie.path) {
        return false;
    }
    if cookie.secure && url.scheme() != "https" && !is_local_hostname(&host) {
        return false;
    }
    true
}

/// `URL.hostname` (IPv6 hosts keep their brackets).
pub(crate) fn url_hostname(url: &url::Url) -> String {
    match url.host() {
        Some(url::Host::Ipv6(addr)) => format!("[{addr}]"),
        Some(host) => host.to_string(),
        None => String::new(),
    }
}

/// `domainMatches`: a domain without a leading dot is host-only.
pub(crate) fn domain_matches(hostname: &str, cookie_domain: &str) -> bool {
    if hostname == cookie_domain {
        return true;
    }
    if !cookie_domain.starts_with('.') {
        return false;
    }
    format!(".{hostname}").ends_with(cookie_domain)
}

/// `pathMatches` (RFC 6265 5.1.4).
pub(crate) fn path_matches(request_path: &str, cookie_path: &str) -> bool {
    if request_path == cookie_path {
        return true;
    }
    if !request_path.starts_with(cookie_path) {
        return false;
    }
    cookie_path.ends_with('/') || request_path[cookie_path.len()..].starts_with('/')
}

/// Mirrors Chromium's `net::IsLocalhost()`.
pub(crate) fn is_local_hostname(hostname: &str) -> bool {
    if hostname == "localhost" || hostname.ends_with(".localhost") || hostname == "[::1]" {
        return true;
    }
    let parts: Vec<&str> = hostname.split('.').collect();
    parts.len() == 4
        && parts[0] == "127"
        && parts[1..]
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// `rewriteCookies`: validate and resolve `url` into domain/path/secure.
pub(crate) fn rewrite_cookies(cookies: Vec<SetCookie>) -> Result<Vec<SetCookie>> {
    cookies
        .into_iter()
        .map(|c| {
            if c.url.is_none() && (c.domain.is_none() || c.path.is_none()) {
                return Err(Error::invalid_argument(
                    "Cookie should have a url or a domain/path pair",
                ));
            }
            if c.url.is_some() && c.domain.is_some() {
                return Err(Error::invalid_argument(
                    "Cookie should have either url or domain",
                ));
            }
            if c.url.is_some() && c.path.is_some() {
                return Err(Error::invalid_argument(
                    "Cookie should have either url or path",
                ));
            }
            if let Some(expires) = c.expires {
                if (expires < 0.0 && expires != -1.0) || expires > MAX_COOKIE_EXPIRES {
                    return Err(Error::invalid_argument(
                        "Cookie should have a valid expires, only -1 or a positive number for the unix timestamp in seconds is allowed",
                    ));
                }
            }
            let mut copy = c;
            if let Some(raw) = copy.url.clone() {
                if raw == "about:blank" {
                    return Err(Error::invalid_argument(format!(
                        "Blank page can not have cookie \"{}\"",
                        copy.name
                    )));
                }
                if raw.starts_with("data:") {
                    return Err(Error::invalid_argument(format!(
                        "Data URL page can not have cookie \"{}\"",
                        copy.name
                    )));
                }
                let url = url::Url::parse(&raw)
                    .map_err(|_| Error::invalid_argument(format!("Invalid URL: {raw}")))?;
                copy.domain = Some(url_hostname(&url));
                let path = url.path();
                copy.path = Some(path[..path.rfind('/').map(|i| i + 1).unwrap_or(0)].to_string());
                copy.secure = Some(url.scheme() == "https");
            }
            Ok(copy)
        })
        .collect()
}

/// A cookie of `Storage.getCookies` converted like `doGetCookies`.
fn from_protocol(c: &Value) -> Option<Cookie> {
    let partition = c.get("partitionKey");
    Some(Cookie {
        name: c.get("name")?.as_str()?.to_string(),
        value: c.get("value")?.as_str()?.to_string(),
        domain: c.get("domain")?.as_str()?.to_string(),
        path: c.get("path")?.as_str()?.to_string(),
        expires: c.get("expires").and_then(Value::as_f64).unwrap_or(-1.0),
        http_only: c.get("httpOnly").and_then(Value::as_bool).unwrap_or(false),
        secure: c.get("secure").and_then(Value::as_bool).unwrap_or(false),
        same_site: c
            .get("sameSite")
            .and_then(Value::as_str)
            .and_then(SameSite::parse)
            .unwrap_or(SameSite::Lax),
        partition_key: partition
            .and_then(|p| p.get("topLevelSite"))
            .and_then(Value::as_str)
            .map(str::to_string),
        cr_has_cross_site_ancestor: partition
            .and_then(|p| p.get("hasCrossSiteAncestor"))
            .and_then(Value::as_bool),
    })
}

/// `toChromiumCookie`.
fn to_protocol(c: &SetCookie) -> Value {
    let mut out = serde_json::Map::new();
    out.insert("name".into(), json!(c.name));
    out.insert("value".into(), json!(c.value));
    let mut put = |key: &str, value: Option<Value>| {
        if let Some(value) = value {
            out.insert(key.into(), value);
        }
    };
    put("url", c.url.as_ref().map(|v| json!(v)));
    put("domain", c.domain.as_ref().map(|v| json!(v)));
    put("path", c.path.as_ref().map(|v| json!(v)));
    put("expires", c.expires.map(|v| json!(v)));
    put("httpOnly", c.http_only.map(|v| json!(v)));
    put("secure", c.secure.map(|v| json!(v)));
    put("sameSite", c.same_site.map(|v| json!(v.as_str())));
    if let Some(key) = &c.partition_key {
        put(
            "partitionKey",
            Some(json!({
                "topLevelSite": key,
                "hasCrossSiteAncestor": c.cr_has_cross_site_ancestor.unwrap_or(true),
            })),
        );
    }
    Value::Object(out)
}

// ----------------------------------------------------------------------
// BrowserContext API
// ----------------------------------------------------------------------

impl BrowserContext {
    fn cookie_params(&self, mut params: serde_json::Map<String, Value>) -> Value {
        if let Some(id) = &self.inner.id {
            params.insert("browserContextId".into(), json!(id.as_ref()));
        }
        Value::Object(params)
    }

    pub(crate) async fn browser_command(&self, method: &str, params: Value) -> Result<Value> {
        let browser = self
            .inner
            .browser
            .upgrade()
            .ok_or_else(Error::target_closed)?;
        browser
            .execute_with_session(RawCommand::new(method, params), None)
            .await
    }

    /// All cookies of the context (`urls` empty) or the ones sent to `urls`.
    pub(crate) async fn get_cookies(&self, urls: &[String]) -> Result<Vec<Cookie>> {
        let result = self
            .browser_command("Storage.getCookies", self.cookie_params(Default::default()))
            .await?;
        let cookies = result
            .get("cookies")
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(from_protocol).collect())
            .unwrap_or_default();
        filter_cookies(cookies, urls)
    }

    pub(crate) async fn set_cookies(&self, cookies: Vec<SetCookie>) -> Result<()> {
        let cookies = rewrite_cookies(cookies)?;
        let mut params = serde_json::Map::new();
        params.insert(
            "cookies".into(),
            Value::Array(cookies.iter().map(to_protocol).collect()),
        );
        self.browser_command("Storage.setCookies", self.cookie_params(params))
            .await?;
        Ok(())
    }

    pub(crate) async fn clear_all_cookies(&self) -> Result<()> {
        self.browser_command(
            "Storage.clearCookies",
            self.cookie_params(Default::default()),
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn clear_cookies_filtered(&self, filter: &ClearCookiesFilter) -> Result<()> {
        if filter.is_empty() {
            return self.clear_all_cookies().await;
        }
        let expire: Vec<SetCookie> = self
            .get_cookies(&[])
            .await?
            .into_iter()
            .filter(|c| filter.matches(c))
            .map(|c| {
                let mut c = SetCookie::from(c);
                c.expires = Some(0.0);
                c
            })
            .collect();
        if expire.is_empty() {
            return Ok(());
        }
        self.set_cookies(expire).await
    }

    /// Cookies of the context. With no URL, all cookies are returned;
    /// otherwise only the cookies that would be sent to one of the URLs.
    ///
    /// ```no_run
    /// # async fn run(context: oxidewright::BrowserContext) -> oxidewright::Result<()> {
    /// let all = context.cookies().await?;
    /// let some = context.cookies().url("https://example.com").await?;
    /// # let _ = (all, some); Ok(()) }
    /// ```
    pub fn cookies(&self) -> Cookies {
        Cookies {
            context: self.clone(),
            urls: Vec::new(),
        }
    }

    /// Add cookies to the context. Each cookie needs either `url` or
    /// `domain` and `path`.
    pub async fn add_cookies<I, C>(&self, cookies: I) -> Result<()>
    where
        I: IntoIterator<Item = C>,
        C: Into<SetCookie>,
    {
        let cookies: Vec<SetCookie> = cookies.into_iter().map(Into::into).collect();
        self.set_cookies(cookies)
            .await
            .api("browserContext.addCookies")
    }

    /// Remove cookies, all of them or the ones matching the filters.
    ///
    /// ```no_run
    /// # async fn run(context: oxidewright::BrowserContext) -> oxidewright::Result<()> {
    /// context.clear_cookies().await?;
    /// context.clear_cookies().name("session-id").await?;
    /// context.clear_cookies().domain(regex::Regex::new("example").unwrap()).await?;
    /// # Ok(()) }
    /// ```
    pub fn clear_cookies(&self) -> ClearCookies {
        ClearCookies {
            context: self.clone(),
            filter: ClearCookiesFilter::default(),
        }
    }
}

/// Builder returned by [`BrowserContext::cookies`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct Cookies {
    context: BrowserContext,
    urls: Vec<String>,
}

impl Cookies {
    /// Only cookies sent to this URL (can be repeated).
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.urls.push(url.into());
        self
    }

    /// Only cookies sent to one of these URLs.
    pub fn urls<I, S>(mut self, urls: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.urls.extend(urls.into_iter().map(Into::into));
        self
    }
}

impl IntoFuture for Cookies {
    type Output = Result<Vec<Cookie>>;
    type IntoFuture = BoxFuture<Result<Vec<Cookie>>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            self.context
                .get_cookies(&self.urls)
                .await
                .api("browserContext.cookies")
        })
    }
}

/// Builder returned by [`BrowserContext::clear_cookies`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct ClearCookies {
    context: BrowserContext,
    filter: ClearCookiesFilter,
}

impl ClearCookies {
    /// Only cookies with this name (string or regex).
    pub fn name(mut self, name: impl Into<TextMatch>) -> Self {
        self.filter.name = Some(name.into());
        self
    }

    /// Only cookies with this domain (string or regex).
    pub fn domain(mut self, domain: impl Into<TextMatch>) -> Self {
        self.filter.domain = Some(domain.into());
        self
    }

    /// Only cookies with this path (string or regex).
    pub fn path(mut self, path: impl Into<TextMatch>) -> Self {
        self.filter.path = Some(path.into());
        self
    }
}

impl IntoFuture for ClearCookies {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            self.context
                .clear_cookies_filtered(&self.filter)
                .await
                .api("browserContext.clearCookies")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie(domain: &str, path: &str, secure: bool) -> Cookie {
        Cookie {
            name: "n".into(),
            value: "v".into(),
            domain: domain.into(),
            path: path.into(),
            expires: -1.0,
            http_only: false,
            secure,
            same_site: SameSite::Lax,
            partition_key: None,
            cr_has_cross_site_ancestor: None,
        }
    }

    #[test]
    fn domain_and_path_matching() {
        assert!(domain_matches("example.com", "example.com"));
        assert!(domain_matches("www.example.com", ".example.com"));
        assert!(domain_matches("example.com", ".example.com"));
        assert!(!domain_matches("www.example.com", "example.com"));
        assert!(path_matches("/a/b", "/a"));
        assert!(path_matches("/a/b", "/a/"));
        assert!(!path_matches("/ab", "/a"));
        assert!(is_local_hostname("127.0.0.1"));
        assert!(is_local_hostname("foo.localhost"));
        assert!(!is_local_hostname("128.0.0.1"));
    }

    #[test]
    fn filters_by_url() {
        let cookies = vec![
            cookie("example.com", "/", false),
            cookie("other.com", "/", false),
            cookie("example.com", "/", true),
        ];
        let out = filter_cookies(cookies.clone(), &["http://example.com/x".into()]).unwrap();
        assert_eq!(out.len(), 1);
        let out = filter_cookies(cookies, &["https://example.com/x".into()]).unwrap();
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn rewrites_url_cookies() {
        let out = rewrite_cookies(vec![
            SetCookie::new("a", "b").url("https://www.example.com/foo/bar"),
        ])
        .unwrap();
        assert_eq!(out[0].domain.as_deref(), Some("www.example.com"));
        assert_eq!(out[0].path.as_deref(), Some("/foo/"));
        assert_eq!(out[0].secure, Some(true));
        let err = rewrite_cookies(vec![SetCookie::new("a", "b")]).unwrap_err();
        assert!(err.to_string().contains("url or a domain/path pair"));
        let err = rewrite_cookies(vec![SetCookie::new("a", "b").url("about:blank")]).unwrap_err();
        assert!(
            err.to_string()
                .contains("Blank page can not have cookie \"a\"")
        );
    }

    #[test]
    fn cookie_json_round_trip() {
        let c = cookie("example.com", "/", false);
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["sameSite"], "Lax");
        assert_eq!(json["httpOnly"], false);
        let back: Cookie = serde_json::from_value(json).unwrap();
        assert_eq!(back, c);
    }
}
