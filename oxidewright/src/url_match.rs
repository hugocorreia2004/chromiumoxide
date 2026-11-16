//! URL matching as in Playwright's `packages/isomorphic/urlMatch.ts`: glob
//! patterns (resolved against the context `baseURL`), regular expressions and
//! predicates.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use regex::Regex;
use url::Url;

/// Something a URL can be matched against.
#[derive(Clone)]
pub enum UrlMatch {
    /// A glob pattern: `*` matches anything but `/`, `**` anything,
    /// `{a,b}` alternatives, `\` escapes. Relative globs are resolved against
    /// the context's `base_url`.
    Glob(String),
    /// A regular expression tested against the full URL.
    Regex(Regex),
    /// A predicate on the parsed URL.
    Predicate(Arc<dyn Fn(&Url) -> bool + Send + Sync>),
}

impl fmt::Debug for UrlMatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UrlMatch::Glob(g) => f.debug_tuple("Glob").field(g).finish(),
            UrlMatch::Regex(r) => f.debug_tuple("Regex").field(&r.as_str()).finish(),
            UrlMatch::Predicate(_) => f.write_str("Predicate(..)"),
        }
    }
}

impl From<&str> for UrlMatch {
    fn from(s: &str) -> Self {
        UrlMatch::Glob(s.to_string())
    }
}

impl From<String> for UrlMatch {
    fn from(s: String) -> Self {
        UrlMatch::Glob(s)
    }
}

impl From<&String> for UrlMatch {
    fn from(s: &String) -> Self {
        UrlMatch::Glob(s.clone())
    }
}

impl From<Regex> for UrlMatch {
    fn from(r: Regex) -> Self {
        UrlMatch::Regex(r)
    }
}

impl UrlMatch {
    /// Match by predicate.
    pub fn predicate(f: impl Fn(&Url) -> bool + Send + Sync + 'static) -> Self {
        UrlMatch::Predicate(Arc::new(f))
    }

    /// `urlMatches(baseURL, url, match)`.
    pub fn matches(&self, base_url: Option<&str>, url: &str) -> bool {
        match self {
            UrlMatch::Glob(glob) => {
                if glob.is_empty() {
                    return true;
                }
                let pattern = resolve_glob_to_regex_pattern(base_url, glob);
                match Regex::new(&pattern) {
                    Ok(re) => re.is_match(url),
                    Err(_) => false,
                }
            }
            UrlMatch::Regex(re) => re.is_match(url),
            UrlMatch::Predicate(f) => match Url::parse(url) {
                Ok(parsed) => f(&parsed),
                Err(_) => false,
            },
        }
    }

    /// The glob, if this matcher is a glob (used for log lines).
    pub(crate) fn as_glob(&self) -> Option<&str> {
        match self {
            UrlMatch::Glob(g) => Some(g),
            _ => None,
        }
    }
}

const ESCAPED: &[char] = &[
    '$', '^', '+', '.', '*', '(', ')', '|', '\\', '?', '{', '}', '[', ']',
];

/// `globToRegexPattern`.
pub fn glob_to_regex_pattern(glob: &str) -> String {
    let chars: Vec<char> = glob.chars().collect();
    let mut tokens = vec!["^".to_string()];
    let mut in_group = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            i += 1;
            let ch = chars[i];
            if ESCAPED.contains(&ch) {
                tokens.push(format!("\\{ch}"));
            } else {
                tokens.push(ch.to_string());
            }
            i += 1;
            continue;
        }
        if c == '*' {
            let char_before = if i > 0 { Some(chars[i - 1]) } else { None };
            let mut star_count = 1;
            while i + 1 < chars.len() && chars[i + 1] == '*' {
                star_count += 1;
                i += 1;
            }
            if star_count > 1 {
                let char_after = chars.get(i + 1).copied();
                if char_after == Some('/') {
                    if char_before == Some('/') {
                        tokens.push("((.+/)|)".to_string());
                    } else {
                        tokens.push("(.*/)".to_string());
                    }
                    i += 1;
                } else {
                    tokens.push("(.*)".to_string());
                }
            } else {
                tokens.push("([^/]*)".to_string());
            }
            i += 1;
            continue;
        }
        match c {
            '{' => {
                in_group = true;
                tokens.push("(".to_string());
            }
            '}' => {
                in_group = false;
                tokens.push(")".to_string());
            }
            ',' => {
                if in_group {
                    tokens.push("|".to_string());
                } else {
                    tokens.push(format!("\\{c}"));
                }
            }
            _ => {
                if ESCAPED.contains(&c) {
                    tokens.push(format!("\\{c}"));
                } else {
                    tokens.push(c.to_string());
                }
            }
        }
        i += 1;
    }
    tokens.push("$".to_string());
    tokens.concat()
}

/// `resolveGlobToRegexPattern` (without the WebSocket variant).
pub(crate) fn resolve_glob_to_regex_pattern(base_url: Option<&str>, glob: &str) -> String {
    let glob = resolve_glob_base(base_url, glob);
    glob_to_regex_pattern(&glob)
}

fn resolve_base_url(base_url: Option<&str>, given: &str) -> (String, Option<String>) {
    let parsed = match base_url.and_then(|b| Url::parse(b).ok()) {
        Some(base) => base.join(given),
        None => Url::parse(given),
    };
    match parsed {
        Ok(url) => {
            let origin = url.origin().ascii_serialization();
            (url.to_string(), Some(origin))
        }
        Err(_) => (given.to_string(), None),
    }
}

/// `resolveGlobBase`.
fn resolve_glob_base(base_url: Option<&str>, glob: &str) -> String {
    if glob.starts_with('*') {
        return glob.to_string();
    }
    let glob = glob.replace("\\\\?", "?");
    for scheme in ["about:", "data:", "chrome:", "edge:", "file:"] {
        if glob.starts_with(scheme) {
            return glob;
        }
    }
    let mut token_map: Vec<(String, String)> = Vec::new();
    let mut map_token = |original: &str, replacement: String| -> String {
        if original.is_empty() {
            return String::new();
        }
        token_map.push((replacement.clone(), original.to_string()));
        replacement
    };
    let relative_path: Vec<String> = glob
        .split('/')
        .enumerate()
        .map(|(index, token)| {
            if token == "." || token == ".." || token.is_empty() {
                return token.to_string();
            }
            if index == 0 && token.ends_with(':') {
                if token.contains('*') || token.contains('{') {
                    return map_token(token, "http:".to_string());
                }
                return token.to_string();
            }
            if !token.contains(['*', '?', '{', '}', '\\']) {
                return token.to_string();
            }
            match token.find('?') {
                None => map_token(token, format!("$_{index}_$")),
                Some(q) => {
                    let prefix = map_token(&token[..q], format!("$_{index}_$"));
                    let suffix = map_token(&token[q..], format!("?$_{index}_$"));
                    prefix + &suffix
                }
            }
        })
        .collect();
    let relative_path = relative_path.join("/");
    let (mut resolved, case_insensitive_part) = resolve_base_url(base_url, &relative_path);
    // Keep first-registered replacement for duplicated tokens, like a JS Map.
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut ordered = Vec::new();
    for (token, original) in token_map {
        if seen.contains_key(&token) {
            seen.insert(token.clone(), original);
            continue;
        }
        seen.insert(token.clone(), original);
        ordered.push(token);
    }
    for token in ordered {
        let original = &seen[&token];
        let normalize = case_insensitive_part
            .as_deref()
            .is_some_and(|part| part.contains(&token));
        let replacement = if normalize {
            original.to_lowercase()
        } else {
            original.clone()
        };
        if let Some(pos) = resolved.find(&token) {
            resolved.replace_range(pos..pos + token.len(), &replacement);
        }
    }
    resolved
}

/// `constructURLBasedOnBaseURL`.
pub(crate) fn construct_url_based_on_base_url(base_url: Option<&str>, given: &str) -> String {
    match base_url.and_then(|b| Url::parse(b).ok()) {
        Some(base) => base
            .join(given)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| given.to_string()),
        None => match Url::parse(given) {
            Ok(u) => u.to_string(),
            Err(_) => given.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn re(glob: &str) -> Regex {
        Regex::new(&glob_to_regex_pattern(glob)).unwrap()
    }

    fn um(base: Option<&str>, url: &str, glob: &str) -> bool {
        UrlMatch::Glob(glob.into()).matches(base, url)
    }

    /// Port of Playwright's `interception.spec.ts` "RO-153 should work with glob".
    #[test]
    fn should_work_with_glob() {
        assert!(re("**/*.js").is_match("https://localhost:8080/foo.js"));
        assert!(!re("**/*.css").is_match("https://localhost:8080/foo.js"));
        assert!(!re("*.js").is_match("https://localhost:8080/foo.js"));
        assert!(re("https://**/*.js").is_match("https://localhost:8080/foo.js"));
        assert!(
            re("http://localhost:8080/simple/path.js")
                .is_match("http://localhost:8080/simple/path.js")
        );
        assert!(re("**/{a,b}.js").is_match("https://localhost:8080/a.js"));
        assert!(re("**/{a,b}.js").is_match("https://localhost:8080/b.js"));
        assert!(!re("**/{a,b}.js").is_match("https://localhost:8080/c.js"));
        assert!(re("**/*.{png,jpg,jpeg}").is_match("https://localhost:8080/c.jpg"));
        assert!(re("**/*.{png,jpg,jpeg}").is_match("https://localhost:8080/c.jpeg"));
        assert!(re("**/*.{png,jpg,jpeg}").is_match("https://localhost:8080/c.png"));
        assert!(!re("**/*.{png,jpg,jpeg}").is_match("https://localhost:8080/c.css"));
        assert!(re("foo*").is_match("foo.js"));
        assert!(!re("foo*").is_match("foo/bar.js"));
        assert!(
            !re("http://localhost:3000/signin-oidc*")
                .is_match("http://localhost:3000/signin-oidc/foo")
        );
        assert!(
            re("http://localhost:3000/signin-oidc*")
                .is_match("http://localhost:3000/signin-oidcnice")
        );
        assert!(re("**/*.js").is_match("/foo.js"));
        assert!(!re("asd/**.js").is_match("/foo.js"));
        assert!(!re("**/*.js").is_match("bar_foo.js"));
        assert!(re("**/api/v[0-9]").is_match("http://example.com/api/v[0-9]"));
        assert!(!re("**/api/v[0-9]").is_match("http://example.com/api/version"));
        assert!(re(r"**/api\?param").is_match("http://example.com/api?param"));
        assert!(!re(r"**/api\?param").is_match("http://example.com/api-param"));
        assert!(re(r"**/three-columns/settings.html\?**id=settings-**").is_match(
            "http://mydomain:8080/blah/blah/three-columns/settings.html?id=settings-e3c58efe-02e9-44b0-97ac-dd138100cf7c&blah"
        ));
        assert_eq!(glob_to_regex_pattern(r"\?"), r"^\?$");
        assert_eq!(glob_to_regex_pattern(r"\"), r"^\\$");
        assert_eq!(glob_to_regex_pattern(r"\\"), r"^\\$");
        assert_eq!(glob_to_regex_pattern(r"\["), r"^\[$");
        assert_eq!(glob_to_regex_pattern("[a-z]"), r"^\[a-z\]$");
        assert_eq!(
            glob_to_regex_pattern(r"$^+.\*()|\?\{\}\[\]"),
            r"^\$\^\+\.\*\(\)\|\?\{\}\[\]$"
        );

        assert!(um(None, "http://playwright.dev/", "http://playwright.dev"));
        assert!(um(
            None,
            "http://playwright.dev/?a=b",
            "http://playwright.dev?a=b"
        ));
        assert!(um(None, "http://playwright.dev/", "h*://playwright.dev"));
        assert!(um(
            None,
            "http://api.playwright.dev/?x=y",
            "http://*.playwright.dev?x=y"
        ));
        assert!(um(None, "http://playwright.dev/foo/bar", "**/foo/**"));
        assert!(um(
            Some("http://playwright.dev"),
            "http://playwright.dev/?x=y",
            "?x=y"
        ));
        assert!(um(
            Some("http://playwright.dev/foo/"),
            "http://playwright.dev/foo/bar?x=y",
            "./bar?x=y"
        ));
        assert!(um(
            None,
            "http://playwright.dev/foo$$bar",
            "http://playwright.dev/foo$$bar"
        ));
        assert!(um(
            None,
            "http://playwright.dev/a$&b",
            "http://playwright.dev/a$&b"
        ));
        assert!(um(
            Some("http://playwright.dev"),
            "http://playwright.dev/p$$q",
            "./p$$q"
        ));
        assert!(um(
            None,
            "https://playwright.dev/fooBAR",
            "HtTpS://pLaYwRiGhT.dEv/fooBAR"
        ));
        assert!(um(
            Some("http://ignored"),
            "https://playwright.dev/fooBAR",
            "HtTpS://pLaYwRiGhT.dEv/fooBAR"
        ));
        assert!(!um(
            None,
            "https://playwright.dev/foobar",
            "https://playwright.dev/fooBAR"
        ));
        assert!(!um(
            None,
            "https://playwright.dev/foobar?a=b",
            "https://playwright.dev/foobar?A=B"
        ));
        assert!(um(
            None,
            "http://example.com/path",
            "http://example.com:80/path"
        ));
        assert!(um(
            None,
            "https://example.com/path",
            "https://example.com:443/path"
        ));
        assert!(um(
            None,
            "http://example.com:8080/path",
            "http://example.com:8080/path"
        ));
        assert!(um(None, "http://localhost/", "http://localhost:80/**"));
        assert!(um(
            None,
            "http://example.com/foo%20bar",
            "http://example.com/foo bar"
        ));
        assert!(um(None, "http://xn--mnchen-3ya.de/", "http://münchen.de/"));
        assert!(um(None, "https://localhost:3000/?a=b", "**/?a=b"));
        assert!(um(None, "https://localhost:3000/?a=b", "**?a=b"));
        assert!(um(None, "https://localhost:3000/?a=b", "**=b"));
        assert!(um(
            None,
            "my.custom.protocol://foo",
            "my.custom.protocol://**"
        ));
        assert!(!um(None, "my.p://foo", "my.{p,y}://**"));
        assert!(um(None, "my.p://foo/", "my.{p,y}://**"));
        assert!(um(None, "file:///foo/", "f*e://**"));
        assert!(
            !re("http://localhost:8080/?imple/path.js")
                .is_match("http://localhost:8080/Simple/path.js")
        );
        assert!(!um(None, "http://playwright.dev/", "http://playwright.?ev"));
        assert!(um(None, "http://playwright./?ev", "http://playwright.?ev"));
    }

    #[test]
    fn resolves_against_base_url() {
        let m = UrlMatch::Glob("/foo/*".into());
        assert!(m.matches(
            Some("http://localhost:8080/"),
            "http://localhost:8080/foo/bar"
        ));
        assert!(!m.matches(
            Some("http://localhost:8080/"),
            "http://localhost:8080/baz/bar"
        ));
    }

    #[test]
    fn regex_and_predicate() {
        assert!(UrlMatch::from(Regex::new("frame").unwrap()).matches(None, "http://x/frame.html"));
        assert!(UrlMatch::predicate(|u| u.path() == "/a").matches(None, "http://x/a"));
    }
}
