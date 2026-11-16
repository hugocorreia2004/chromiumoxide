//! In-memory cookie jar of global request contexts and `Set-Cookie`
//! parsing (Playwright's `server/cookieStore.ts`).

use std::time::{SystemTime, UNIX_EPOCH};

use crate::cookies::{Cookie, MAX_COOKIE_EXPIRES, SameSite};

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn expired(cookie: &Cookie) -> bool {
    if cookie.expires == -1.0 {
        return false;
    }
    cookie.expires * 1000.0 < now_secs() * 1000.0
}

/// A cookie jar keyed like RFC 6265 (name, domain, path).
#[derive(Debug, Default, Clone)]
pub(crate) struct CookieStore {
    cookies: Vec<Cookie>,
}

impl CookieStore {
    pub(crate) fn add_cookies(&mut self, cookies: impl IntoIterator<Item = Cookie>) {
        for cookie in cookies {
            self.cookies.retain(|other| {
                !(other.name == cookie.name
                    && other.domain == cookie.domain
                    && other.path == cookie.path)
            });
            self.cookies.push(cookie);
            self.cookies.retain(|c| !expired(c));
        }
    }

    pub(crate) fn all_cookies(&mut self) -> Vec<Cookie> {
        self.cookies.retain(|c| !expired(c));
        self.cookies.clone()
    }

    pub(crate) fn remove_cookies(&mut self, predicate: impl Fn(&Cookie) -> bool) {
        self.cookies.retain(|c| !predicate(c));
    }
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Milliseconds since the epoch of a cookie date (`Wdy, DD Mon YYYY
/// HH:MM:SS GMT`, RFC 850 and asctime forms); years before 1970 give
/// negative values like JavaScript's `Date`.
pub(crate) fn parse_cookie_date(value: &str) -> Option<f64> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let (mut day, mut month, mut year, mut time) = (None, None, None, None);
    for token in value.split([' ', ',', '-']).filter(|t| !t.is_empty()) {
        let lower = token.to_ascii_lowercase();
        if time.is_none() && token.contains(':') {
            let parts: Vec<i64> = token.split(':').filter_map(|p| p.parse().ok()).collect();
            if parts.len() == 3 {
                time = Some((parts[0], parts[1], parts[2]));
                continue;
            }
        }
        if month.is_none() {
            if let Some(index) = MONTHS.iter().position(|m| lower.starts_with(m)) {
                month = Some(index as i64 + 1);
                continue;
            }
        }
        if token.chars().all(|c| c.is_ascii_digit()) {
            let n: i64 = token.parse().ok()?;
            if day.is_none() && token.len() <= 2 {
                day = Some(n);
            } else if year.is_none() {
                year = Some(match token.len() {
                    2 if n < 70 => 2000 + n,
                    2 => 1900 + n,
                    _ => n,
                });
            }
        }
    }
    let (day, month, year) = (day?, month?, year?);
    let (h, mi, s) = time.unwrap_or((0, 0, 0));
    if !(1..=31).contains(&day) || h > 23 || mi > 59 || s > 59 {
        return None;
    }
    let secs = days_from_civil(year, month, day) * 86_400 + h * 3600 + mi * 60 + s;
    Some(secs as f64 * 1000.0)
}

/// `parseCookieNameValue`.
pub(crate) fn parse_cookie_name_value(pair: &str) -> (String, String) {
    match pair.find('=') {
        None => (String::new(), pair.trim().to_string()),
        Some(pos) => (
            pair[..pos].trim().to_string(),
            pair[pos + 1..].trim().to_string(),
        ),
    }
}

/// `serializeCookieNameValue`.
pub(crate) fn serialize_cookie_name_value(name: &str, value: &str) -> String {
    if name.is_empty() {
        value.to_string()
    } else {
        format!("{name}={value}")
    }
}

/// A parsed `Set-Cookie` header (`parseRawCookie` + `parseCookie`): missing
/// domain and path are empty strings, `expires` is `-1` for session cookies.
pub(crate) fn parse_set_cookie(header: &str) -> Option<Cookie> {
    let mut parts = header.split(';');
    let (name, value) = parse_cookie_name_value(parts.next().unwrap_or_default());
    if name.is_empty() && value.is_empty() {
        return None;
    }
    let mut cookie = Cookie {
        name,
        value,
        domain: String::new(),
        path: String::new(),
        expires: -1.0,
        http_only: false,
        secure: false,
        same_site: SameSite::Lax,
        partition_key: None,
        cr_has_cross_site_ancestor: None,
    };
    let mut max_age_expires = None;
    for attribute in parts.filter(|s| !s.trim().is_empty()) {
        let (key, value) = match attribute.find('=') {
            None => (attribute.trim(), ""),
            Some(pos) => (attribute[..pos].trim(), attribute[pos + 1..].trim()),
        };
        match key.to_lowercase().as_str() {
            "expires" => {
                if let Some(ms) = parse_cookie_date(value) {
                    cookie.expires = if ms <= 0.0 {
                        0.0
                    } else {
                        (ms / 1000.0).min(MAX_COOKIE_EXPIRES)
                    };
                }
            }
            "max-age" => {
                let digits = value.strip_prefix('-').unwrap_or(value);
                if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                    let seconds: f64 = value.parse().unwrap_or(0.0);
                    max_age_expires = Some(if seconds <= 0.0 {
                        0.0
                    } else {
                        (now_secs() + seconds).min(MAX_COOKIE_EXPIRES)
                    });
                }
            }
            "domain" => {
                let mut domain = value.to_lowercase();
                if !domain.is_empty() && !domain.starts_with('.') && domain.contains('.') {
                    domain = format!(".{domain}");
                }
                cookie.domain = domain;
            }
            "path" => cookie.path = value.to_string(),
            "secure" => cookie.secure = true,
            "httponly" => cookie.http_only = true,
            "samesite" => {
                if let Some(same_site) = SameSite::parse(value) {
                    cookie.same_site = same_site;
                }
            }
            _ => {}
        }
    }
    if let Some(expires) = max_age_expires {
        cookie.expires = expires;
    }
    Some(cookie)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_set_cookie_headers() {
        let c =
            parse_set_cookie("a=b; Domain=Example.com; Path=/p; Secure; HttpOnly; SameSite=Strict")
                .unwrap();
        assert_eq!((c.name.as_str(), c.value.as_str()), ("a", "b"));
        assert_eq!(c.domain, ".example.com");
        assert_eq!(c.path, "/p");
        assert!(c.secure && c.http_only);
        assert_eq!(c.same_site, SameSite::Strict);
        assert_eq!(c.expires, -1.0);
        let c = parse_set_cookie("x=y; Expires=Wed, 21 Oct 2015 07:28:00 GMT").unwrap();
        assert_eq!(c.expires, 1_445_412_480.0);
        let c = parse_set_cookie("x=y; Expires=Wed, 21 Oct 2015 07:28:00 GMT; Max-Age=0").unwrap();
        assert_eq!(c.expires, 0.0);
        let c = parse_set_cookie("x=y; expires=Wed, 01 Jan 1000 00:00:00 GMT").unwrap();
        assert_eq!(c.expires, 0.0);
        assert_eq!(
            parse_cookie_date("Thu, 01-Jan-1970 00:00:10 GMT"),
            Some(10_000.0)
        );
        let c = parse_set_cookie("x=y; Max-Age=abc").unwrap();
        assert_eq!(c.expires, -1.0);
        assert!(parse_set_cookie("=").is_none());
        let c = parse_set_cookie("novalue").unwrap();
        assert_eq!((c.name.as_str(), c.value.as_str()), ("", "novalue"));
        assert_eq!(serialize_cookie_name_value("", "v"), "v");
        assert_eq!(serialize_cookie_name_value("n", "v"), "n=v");
    }

    #[test]
    fn store_replaces_and_prunes() {
        let mut store = CookieStore::default();
        let mut a = parse_set_cookie("a=1").unwrap();
        a.domain = "example.com".into();
        a.path = "/".into();
        let mut a2 = a.clone();
        a2.value = "2".into();
        store.add_cookies([a, a2]);
        assert_eq!(store.all_cookies().len(), 1);
        assert_eq!(store.all_cookies()[0].value, "2");
        let mut old = store.all_cookies()[0].clone();
        old.name = "old".into();
        old.expires = 1.0;
        store.add_cookies([old]);
        assert_eq!(store.all_cookies().len(), 1);
        store.remove_cookies(|c| c.name == "a");
        assert!(store.all_cookies().is_empty());
    }
}
