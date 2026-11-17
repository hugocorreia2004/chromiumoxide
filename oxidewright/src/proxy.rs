//! Browser and context proxies, like Playwright's `normalizeProxySettings`,
//! `chromium.ts` (launch switches) and `crBrowser.ts` (per-context proxy).

use crate::error::{Error, Result};
use crate::fetch::ProxySettings;
use crate::network_manager::HttpCredentials;

/// `normalizeProxySettings`: `host:port` becomes `http://host:port`, the
/// server keeps only scheme and host, bypass entries are trimmed. SOCKS
/// proxies cannot authenticate in Chromium.
pub(crate) fn normalize(proxy: &ProxySettings) -> Result<ProxySettings> {
    let parsed = url::Url::parse(&proxy.server)
        .ok()
        .filter(|u| u.has_host())
        .or_else(|| url::Url::parse(&format!("http://{}", proxy.server)).ok())
        .ok_or_else(|| {
            Error::invalid_argument(format!("Invalid proxy server: {}", proxy.server))
        })?;
    let has_auth = proxy.username.is_some() || proxy.password.is_some();
    match parsed.scheme() {
        "socks4" if has_auth => {
            return Err(Error::invalid_argument(
                "Socks4 proxy protocol does not support authentication",
            ));
        }
        "socks5" if has_auth => {
            return Err(Error::invalid_argument(
                "Browser does not support socks5 proxy authentication",
            ));
        }
        _ => {}
    }
    let host = match parsed.port() {
        Some(port) => format!("{}:{port}", parsed.host_str().unwrap_or_default()),
        None => parsed.host_str().unwrap_or_default().to_string(),
    };
    Ok(ProxySettings {
        server: format!("{}://{host}", parsed.scheme()),
        bypass: proxy
            .bypass
            .as_ref()
            .map(|b| b.split(',').map(str::trim).collect::<Vec<_>>().join(",")),
        username: proxy.username.clone(),
        password: proxy.password.clone(),
    })
}

/// `shouldProxyLoopback`: Chromium bypasses loopback addresses unless told
/// otherwise; Playwright proxies them too unless the bypass list names one.
fn should_proxy_loopback(bypass: Option<&str>) -> bool {
    let hosts: Vec<&str> = bypass
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .collect();
    ![
        "localhost",
        "127.0.0.1",
        "::1",
        "[::]",
        "[::1]",
        "<loopback>",
        "<-loopback>",
    ]
    .iter()
    .any(|h| hosts.contains(h))
}

/// Command line switches of a browser-wide proxy (`chromium.ts`).
pub(crate) fn launch_switches(proxy: &ProxySettings) -> Vec<(String, Option<String>)> {
    let mut switches = Vec::new();
    if let Ok(url) = url::Url::parse(&proxy.server) {
        if url.scheme() == "socks5" {
            // Resolve every host through the proxy, except the proxy itself.
            switches.push((
                "host-resolver-rules".to_string(),
                Some(format!(
                    "MAP * ~NOTFOUND , EXCLUDE {}",
                    url.host_str().unwrap_or_default()
                )),
            ));
        }
    }
    switches.push(("proxy-server".to_string(), Some(proxy.server.clone())));
    let mut rules: Vec<String> = proxy
        .bypass
        .as_deref()
        .map(|b| {
            b.split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(|t| {
                    if t.starts_with('.') {
                        format!("*{t}")
                    } else {
                        t.to_string()
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    if should_proxy_loopback(proxy.bypass.as_deref()) {
        rules.push("<-loopback>".to_string());
    }
    if !rules.is_empty() {
        switches.push(("proxy-bypass-list".to_string(), Some(rules.join(";"))));
    }
    switches
}

/// `Target.createBrowserContext` proxy parameters (`crBrowser.ts`).
pub(crate) fn context_params(proxy: &ProxySettings) -> (String, Option<String>) {
    let bypass = if should_proxy_loopback(proxy.bypass.as_deref()) {
        Some(match &proxy.bypass {
            Some(b) => format!("<-loopback>,{b}"),
            None => "<-loopback>".to_string(),
        })
    } else {
        proxy.bypass.clone()
    };
    (proxy.server.clone(), bypass)
}

/// `BrowserContext.proxyCredentials()`: they only answer the proxy's
/// challenge, never the website's.
pub(crate) fn credentials(proxy: Option<&ProxySettings>) -> Option<HttpCredentials> {
    let proxy = proxy?;
    let username = proxy.username.clone()?;
    Some(HttpCredentials::new(
        username,
        proxy.password.clone().unwrap_or_default(),
    ))
}

/// The proxy credentials of a context: its own proxy's, else the launch
/// proxy's (`this._options.proxy || this._browser.options.proxy`).
pub(crate) fn context_credentials(
    context: &crate::browser_context::ContextInner,
) -> Option<HttpCredentials> {
    if let Some(proxy) = &context.options.proxy {
        return credentials(Some(proxy));
    }
    let browser = context.browser.upgrade()?;
    let proxy = browser.ext.proxy.lock().unwrap().clone();
    credentials(proxy.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_like_playwright() {
        let p = normalize(&ProxySettings::new("127.0.0.1:8080")).unwrap();
        assert_eq!(p.server, "http://127.0.0.1:8080");
        let p = normalize(&ProxySettings::new("localhost:3128")).unwrap();
        assert_eq!(p.server, "http://localhost:3128");
        let p = normalize(&ProxySettings::new("socks5://proxy.example:1080")).unwrap();
        assert_eq!(p.server, "socks5://proxy.example:1080");
        let mut auth = ProxySettings::new("socks5://proxy.example:1080");
        auth.username = Some("u".into());
        assert!(normalize(&auth).is_err());
        let mut bypass = ProxySettings::new("http://p:1");
        bypass.bypass = Some(" a.com , .b.com".into());
        assert_eq!(
            normalize(&bypass).unwrap().bypass.as_deref(),
            Some("a.com,.b.com")
        );
    }

    #[test]
    fn launch_switches_proxy_loopback_unless_bypassed() {
        let mut p = ProxySettings::new("http://p:1");
        p.bypass = Some("a.com,.b.com".into());
        let s = launch_switches(&p);
        assert!(s.contains(&("proxy-server".into(), Some("http://p:1".into()))));
        assert!(s.contains(&(
            "proxy-bypass-list".into(),
            Some("a.com;*.b.com;<-loopback>".into())
        )));
        p.bypass = Some("localhost".into());
        let s = launch_switches(&p);
        assert!(s.contains(&("proxy-bypass-list".into(), Some("localhost".into()))));
        assert_eq!(context_params(&p).1.as_deref(), Some("localhost"));
        p.bypass = None;
        assert_eq!(context_params(&p).1.as_deref(), Some("<-loopback>"));
    }
}
