//! Compression Dictionary Transport (RFC 9842).
//!
//! A response with a `Use-As-Dictionary` header registers its decoded body
//! as a dictionary for the URLs its `match` pattern covers. A later request
//! to such a URL advertises the best match in `Available-Dictionary` (and its
//! `Dictionary-ID`), and accepts the `dcb` (brotli) and `dcz` (zstd) codings
//! that use it. As in Chromium:
//!
//! - dictionaries are partitioned by the request's top-level site and the
//!   dictionary's origin, and are used for same-origin and CORS requests to
//!   potentially trustworthy URLs only;
//! - a dictionary is used while its response is fresh, or stale inside its
//!   `stale-while-revalidate` window without `must-revalidate`; a
//!   `no-store` or `no-cache` response does not register;
//! - the best match is the one with a `match-dest` (that matches), then
//!   the longest `match` string, then the latest registered;
//! - `Clear-Site-Data` with "cache", "cookies" or "*" removes the origin's
//!   dictionaries.

use std::sync::Arc;

use http::{HeaderMap, HeaderName, HeaderValue};
use sha2::{Digest, Sha256};
use url::Url;

pub(crate) const USE_AS_DICTIONARY: HeaderName = HeaderName::from_static("use-as-dictionary");
pub(crate) const AVAILABLE_DICTIONARY: HeaderName = HeaderName::from_static("available-dictionary");
pub(crate) const DICTIONARY_ID: HeaderName = HeaderName::from_static("dictionary-id");
pub(crate) const CLEAR_SITE_DATA: HeaderName = HeaderName::from_static("clear-site-data");

/// The longest `id` a dictionary may carry.
const MAX_ID: usize = 1024;
/// The most dictionaries kept; the oldest go first.
const MAX_ENTRIES: usize = 1000;
/// The most dictionary bytes kept, and the largest dictionary.
pub(crate) const MAX_BYTES: usize = 200 << 20;

/// A registered dictionary's content and its SHA-256.
pub(crate) struct Dictionary {
    pub hash: [u8; 32],
    pub bytes: Vec<u8>,
}

/// A parsed `Use-As-Dictionary` header.
pub(crate) struct Registration {
    pattern: urlpattern::UrlPattern<regex::Regex>,
    /// The `match` string as sent: its length ranks matches.
    match_str: String,
    /// `match-dest`: empty matches every destination.
    dest: Vec<String>,
    id: Option<String>,
}

/// One item of a structured field (RFC 9651) as far as dictionaries read it.
#[derive(Debug, PartialEq)]
enum Bare {
    Str(String),
    Token(String),
    Other,
}

#[derive(Debug, PartialEq)]
enum Member {
    Item(Bare),
    List(Vec<Bare>),
}

/// An RFC 9651 dictionary; `None` when malformed. Parameters are skipped.
fn parse_sf_dictionary(input: &str) -> Option<Vec<(String, Member)>> {
    let mut p = Parser { s: input.as_bytes(), i: 0 };
    let mut out: Vec<(String, Member)> = Vec::new();
    p.sp();
    if p.i == p.s.len() {
        return Some(out);
    }
    loop {
        let key = p.key()?;
        let member = if p.eat(b'=') {
            if p.peek() == Some(b'(') {
                p.i += 1;
                let mut list = Vec::new();
                loop {
                    while p.peek() == Some(b' ') {
                        p.i += 1;
                    }
                    if p.eat(b')') {
                        break;
                    }
                    list.push(p.bare()?);
                    p.params()?;
                    if !matches!(p.peek(), Some(b' ') | Some(b')')) {
                        return None;
                    }
                }
                p.params()?;
                Member::List(list)
            } else {
                let item = p.bare()?;
                p.params()?;
                Member::Item(item)
            }
        } else {
            p.params()?;
            Member::Item(Bare::Other)
        };
        out.retain(|(k, _)| *k != key);
        out.push((key, member));
        p.ows();
        if p.i == p.s.len() {
            return Some(out);
        }
        if !p.eat(b',') {
            return None;
        }
        p.ows();
        if p.i == p.s.len() {
            return None;
        }
    }
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn sp(&mut self) {
        while self.peek() == Some(b' ') {
            self.i += 1;
        }
    }
    fn ows(&mut self) {
        while matches!(self.peek(), Some(b' ') | Some(b'\t')) {
            self.i += 1;
        }
    }
    fn key(&mut self) -> Option<String> {
        let start = self.i;
        match self.peek()? {
            b'a'..=b'z' | b'*' => self.i += 1,
            _ => return None,
        }
        while matches!(self.peek(), Some(b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.' | b'*')) {
            self.i += 1;
        }
        Some(String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
    }
    fn params(&mut self) -> Option<()> {
        while self.eat(b';') {
            self.sp();
            self.key()?;
            if self.eat(b'=') {
                self.bare()?;
            }
        }
        Some(())
    }
    fn bare(&mut self) -> Option<Bare> {
        match self.peek()? {
            b'"' => {
                self.i += 1;
                let mut out = String::new();
                loop {
                    let c = self.peek()?;
                    self.i += 1;
                    match c {
                        b'"' => return Some(Bare::Str(out)),
                        b'\\' => {
                            let c = self.peek()?;
                            if c != b'"' && c != b'\\' {
                                return None;
                            }
                            self.i += 1;
                            out.push(c as char);
                        }
                        0x20..=0x7e => out.push(c as char),
                        _ => return None,
                    }
                }
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'*' => {
                let start = self.i;
                self.i += 1;
                while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~:/".contains(&c)) {
                    self.i += 1;
                }
                Some(Bare::Token(String::from_utf8_lossy(&self.s[start..self.i]).into_owned()))
            }
            b'?' => {
                self.i += 1;
                match self.peek()? {
                    b'0' | b'1' => {
                        self.i += 1;
                        Some(Bare::Other)
                    }
                    _ => None,
                }
            }
            b'-' | b'0'..=b'9' => {
                self.i += 1;
                while matches!(self.peek(), Some(b'0'..=b'9' | b'.')) {
                    self.i += 1;
                }
                Some(Bare::Other)
            }
            b':' => {
                self.i += 1;
                while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || b"+/=".contains(&c)) {
                    self.i += 1;
                }
                self.eat(b':').then_some(Bare::Other)
            }
            _ => None,
        }
    }
}

/// The `Use-As-Dictionary` header of a response from `url`: `None` when
/// absent, malformed, or not usable (a `match` that does not parse, has
/// regular expression groups or names another origin; an `id` above 1,024
/// characters; a `type` other than "raw").
pub(crate) fn parse_use_as_dictionary(headers: &HeaderMap, url: &Url) -> Option<Registration> {
    let value = headers.get(USE_AS_DICTIONARY)?.to_str().ok()?;
    let members = parse_sf_dictionary(value)?;
    let mut match_str = None;
    let mut dest = Vec::new();
    let mut id = None;
    for (key, member) in members {
        match (key.as_str(), member) {
            ("match", Member::Item(Bare::Str(s))) => match_str = Some(s),
            ("match", _) => return None,
            ("match-dest", Member::List(list)) => {
                for item in list {
                    match item {
                        Bare::Str(s) => dest.push(s),
                        _ => return None,
                    }
                }
            }
            ("match-dest", _) => return None,
            ("id", Member::Item(Bare::Str(s))) if s.len() <= MAX_ID => id = Some(s),
            ("id", _) => return None,
            ("type", Member::Item(Bare::Token(t))) if t == "raw" => {}
            ("type", _) => return None,
            _ => {}
        }
    }
    let match_str = match_str?;
    let init = urlpattern::UrlPatternInit::parse_constructor_string::<regex::Regex>(&match_str, Some(url.clone())).ok()?;
    let pattern = urlpattern::UrlPattern::<regex::Regex>::parse(init, Default::default()).ok()?;
    if pattern.has_regexp_groups() {
        return None;
    }
    // The pattern stays on the dictionary's origin.
    let port = url.port().map(|p| p.to_string()).unwrap_or_default();
    // Pattern strings escape what a pattern would read as syntax (an IPv6 host's colons, `[`, `]`).
    let hostname = pattern.hostname().replace('\\', "");
    if pattern.protocol() != url.scheme() || hostname != url.host_str().unwrap_or("") || pattern.port() != port {
        return None;
    }
    Some(Registration { pattern, match_str, dest, id })
}

/// Until when (wall-clock seconds) a response may serve as a dictionary,
/// from its caching headers (RFC 9111 §4.2); `None` when it never may.
pub(crate) fn usable_until(headers: &HeaderMap, request_time: f64, response_time: f64) -> Option<f64> {
    let d = crate::cache::Directives::parse(headers, false);
    if d.no_store || d.no_cache {
        return None;
    }
    let date = headers
        .get(http::header::DATE)
        .and_then(|v| v.to_str().ok())
        .and_then(crate::cache::parse_http_date);
    let lifetime = match d.max_age {
        Some(Some(s)) => s as f64,
        Some(None) => 0.0,
        None => {
            let expires =
                headers.get(http::header::EXPIRES).and_then(|v| v.to_str().ok()).and_then(crate::cache::parse_http_date);
            match expires {
                Some(e) => (e - date.unwrap_or(response_time)).max(0.0),
                None => 0.0,
            }
        }
    };
    let age_value = headers
        .get(http::header::AGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(0) as f64;
    let apparent = date.map_or(0.0, |date| (response_time - date).max(0.0));
    let initial_age = apparent.max(age_value + (response_time - request_time).max(0.0));
    let stale = if d.must_revalidate { 0.0 } else { d.stale_while_revalidate.unwrap_or(0) as f64 };
    let until = response_time - initial_age + lifetime + stale;
    (until > response_time).then_some(until)
}

struct Entry {
    top: Option<String>,
    origin: String,
    reg: Registration,
    dict: Arc<Dictionary>,
    until: f64,
    seq: u64,
}

/// The registered dictionaries of one network client.
#[derive(Default)]
pub(crate) struct Dictionaries {
    entries: Vec<Entry>,
    seq: u64,
    bytes: usize,
}

impl Dictionaries {
    /// Registers `body`, the decoded response to `url`, replacing a
    /// dictionary with the same `match` and `match-dest` there.
    pub fn register(&mut self, top: Option<String>, url: &Url, reg: Registration, body: Vec<u8>, until: f64) {
        let origin = url.origin().ascii_serialization();
        let hash: [u8; 32] = Sha256::digest(&body).into();
        self.remove(|e| e.top == top && e.origin == origin && e.reg.match_str == reg.match_str && e.reg.dest == reg.dest);
        self.seq += 1;
        self.bytes += body.len();
        let dict = Arc::new(Dictionary { hash, bytes: body });
        self.entries.push(Entry { top, origin, reg, dict, until, seq: self.seq });
        while self.entries.len() > MAX_ENTRIES || self.bytes > MAX_BYTES {
            let gone = self.entries.remove(0);
            self.bytes -= gone.dict.bytes.len();
        }
    }

    fn remove(&mut self, mut f: impl FnMut(&Entry) -> bool) {
        let bytes = &mut self.bytes;
        self.entries.retain(|e| {
            let gone = f(e);
            if gone {
                *bytes -= e.dict.bytes.len();
            }
            !gone
        });
    }

    /// The best dictionary for a request to `url` of destination `dest`
    /// (Fetch's string, "" for none) under `top`, and its `id`.
    pub fn best(&self, top: Option<&str>, url: &Url, dest: &str, now: f64) -> Option<(&Arc<Dictionary>, Option<&str>)> {
        let origin = url.origin().ascii_serialization();
        self.entries
            .iter()
            .filter(|e| e.top.as_deref() == top && e.origin == origin && e.until > now)
            .filter(|e| e.reg.dest.is_empty() || e.reg.dest.iter().any(|d| d == dest))
            .filter(|e| e.reg.pattern.test(urlpattern::UrlPatternMatchInput::Url(url.clone())).unwrap_or(false))
            .max_by_key(|e| (!e.reg.dest.is_empty(), e.reg.match_str.len(), e.seq))
            .map(|e| (&e.dict, e.reg.id.as_deref()))
    }

    /// The dictionary an `Available-Dictionary` value names.
    pub fn by_header(&self, value: &HeaderValue) -> Option<Arc<Dictionary>> {
        self.entries.iter().rev().find(|e| hash_header(&e.dict.hash).as_bytes() == value.as_bytes()).map(|e| e.dict.clone())
    }

    /// `Clear-Site-Data` from `url` naming "cache" or "cookies" (or "*", both):
    /// the origin's dictionaries go.
    pub fn clear_origin(&mut self, url: &Url) {
        let origin = url.origin().ascii_serialization();
        self.remove(|e| e.origin == origin);
    }
}

/// The types a `Clear-Site-Data` header names (quoted strings; "*" is all of them).
#[derive(Default, Debug, PartialEq)]
pub(crate) struct ClearSiteData {
    pub cache: bool,
    pub cookies: bool,
}

pub(crate) fn clear_site_data(headers: &HeaderMap) -> ClearSiteData {
    let mut out = ClearSiteData::default();
    for t in headers.get_all(CLEAR_SITE_DATA).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(',')) {
        match t.trim() {
            "\"cache\"" => out.cache = true,
            "\"cookies\"" => out.cookies = true,
            "\"*\"" => (out.cache, out.cookies) = (true, true),
            _ => {}
        }
    }
    out
}

/// The `Available-Dictionary` value of a hash: an sf-binary.
pub(crate) fn hash_header(hash: &[u8; 32]) -> String {
    use base64::Engine as _;
    format!(":{}:", base64::engine::general_purpose::STANDARD.encode(hash))
}

/// A `Dictionary-ID` value: the id as an sf-string.
pub(crate) fn id_header(id: &str) -> Option<HeaderValue> {
    let mut s = String::with_capacity(id.len() + 2);
    s.push('"');
    for c in id.chars() {
        if c == '"' || c == '\\' {
            s.push('\\');
        }
        s.push(c);
    }
    s.push('"');
    HeaderValue::from_str(&s).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(USE_AS_DICTIONARY, HeaderValue::from_str(value).unwrap());
        h
    }

    #[test]
    fn use_as_dictionary_parses_and_rejects() {
        let url = Url::parse("https://a.test/fetch/compression-dictionary/resources/register.py").unwrap();
        let ok = |v: &str| parse_use_as_dictionary(&headers(v), &url).is_some();
        assert!(ok(r#"match="/fetch/compression-dictionary/resources/*""#));
        let v6 = Url::parse("https://[::1]:8443/d/r.py").unwrap();
        assert!(parse_use_as_dictionary(&headers(r#"match="/d/*""#), &v6).is_some());
        assert!(ok(r#"match="/x/*", id="a\"b\\c", type=raw, match-dest=("" "asdf")"#));
        assert!(ok(r#"match="/x/*", unknown-option="some-value""#));
        assert!(!ok("?0"));
        assert!(!ok(r#"match="{""#));
        assert!(!ok(r#"match="https://b.test/x/*""#));
        assert!(!ok(r#"match="/x/*", type=bogus"#));
        assert!(!ok(r#"match="/x/(\\d+)""#));
        assert!(!ok(&format!(r#"match="/x/*", id="{}""#, "a".repeat(1025))));
        assert!(ok(&format!(r#"match="/x/*", id="{}""#, "a".repeat(1024))));
        let reg = parse_use_as_dictionary(&headers(r#"match="/x/*", id="a\"b""#), &url).unwrap();
        assert_eq!(reg.id.as_deref(), Some("a\"b"));
        assert_eq!(id_header("a\"b").unwrap(), r#""a\"b""#);
    }

    #[test]
    fn best_match_prefers_dest_then_length_then_latest() {
        let url = Url::parse("https://a.test/d/r/echo.py").unwrap();
        let mut d = Dictionaries::default();
        let reg = |v: &str| parse_use_as_dictionary(&headers(v), &url).unwrap();
        d.register(None, &url, reg(r#"match="/d/*""#), b"general".to_vec(), 1e12);
        d.register(None, &url, reg(r#"match="/d/r/*""#), b"medium".to_vec(), 1e12);
        let best = |d: &Dictionaries| d.best(None, &url, "", 0.0).map(|(x, _)| x.bytes.clone());
        assert_eq!(best(&d).unwrap(), b"medium");
        d.register(None, &url, reg(r#"match="/*", match-dest=("")"#), b"dest".to_vec(), 1e12);
        assert_eq!(best(&d).unwrap(), b"dest");
        assert!(d.best(None, &url, "script", 0.0).is_some_and(|(x, _)| x.bytes == b"medium"));
        assert!(d.best(Some("https://other.test"), &url, "", 0.0).is_none());
        let mut h = HeaderMap::new();
        h.insert(CLEAR_SITE_DATA, HeaderValue::from_static("\"storage\""));
        assert_eq!(clear_site_data(&h), ClearSiteData::default());
        h.insert(CLEAR_SITE_DATA, HeaderValue::from_static("\"storage\", \"cookies\""));
        assert_eq!(clear_site_data(&h), ClearSiteData { cache: false, cookies: true });
        h.insert(CLEAR_SITE_DATA, HeaderValue::from_static("\"*\""));
        assert_eq!(clear_site_data(&h), ClearSiteData { cache: true, cookies: true });
        d.clear_origin(&url);
        assert!(best(&d).is_none());
    }

    #[test]
    fn freshness_windows() {
        let h = |cc: &str, age: Option<&str>| {
            let mut h = HeaderMap::new();
            h.insert(http::header::CACHE_CONTROL, HeaderValue::from_str(cc).unwrap());
            if let Some(a) = age {
                h.insert(http::header::AGE, HeaderValue::from_str(a).unwrap());
            }
            h
        };
        assert!(usable_until(&h("max-age=3600", None), 100.0, 100.0).is_some());
        assert!(usable_until(&h("max-age=3600", Some("7200")), 100.0, 100.0).is_none());
        assert!(usable_until(&h("max-age=0", None), 100.0, 100.0).is_none());
        assert!(usable_until(&h("max-age=0, stale-while-revalidate=3600", None), 100.0, 100.0).is_some());
        assert!(usable_until(&h("max-age=0, stale-while-revalidate=5", Some("10")), 100.0, 100.0).is_none());
        assert!(usable_until(&h("max-age=0, must-revalidate, stale-while-revalidate=3600", None), 100.0, 100.0).is_none());
        assert!(usable_until(&h("max-age=3600, no-cache", None), 100.0, 100.0).is_none());
        assert!(usable_until(&h("max-age=3600, no-store", None), 100.0, 100.0).is_none());
    }
}
