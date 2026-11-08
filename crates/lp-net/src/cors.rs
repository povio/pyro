//! CORS (Fetch §3.2): safelisting, preflight construction/validation and the
//! response CORS check. The IO thread applies these; a small preflight cache
//! avoids repeated OPTIONS round-trips.

use std::time::{Duration, Instant};

use http::{header, HeaderMap, HeaderValue, Method};
use rustc_hash::FxHashMap;
use url::Url;

use crate::request::{Credentials, Request, RequestMode};

pub fn same_origin(a: &Url, b: &Url) -> bool {
    // A tuple origin compared in place: `Url::origin` allocates its host,
    // and every request asks (CORS, referrer, `Sec-Fetch-Site`).
    let tuple = |u: &Url| matches!(u.scheme(), "http" | "https" | "ws" | "wss" | "ftp");
    if tuple(a) && tuple(b) {
        return a.scheme() == b.scheme()
            && a.host() == b.host()
            && a.port_or_known_default() == b.port_or_known_default();
    }
    a.origin() == b.origin()
}

fn safelisted_content_type(v: &[u8]) -> bool {
    let Ok(s) = std::str::from_utf8(v) else { return false };
    let essence = s.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    matches!(&*essence, "application/x-www-form-urlencoded" | "multipart/form-data" | "text/plain")
}

fn has_unsafe_byte(v: &[u8]) -> bool {
    v.iter().any(|&b| {
        (b < 0x20 && b != b'\t')
            || matches!(
                b,
                b'"' | b'(' | b')' | b':' | b'<' | b'>' | b'?' | b'@' | b'[' | b'\\' | b']' | b'{' | b'}' | 0x7f
            )
    })
}

/// CORS-safelisted request-header check.
pub fn is_safelisted_header(name: &str, value: &HeaderValue) -> bool {
    let v = value.as_bytes();
    if v.len() > 128 {
        return false;
    }
    match name {
        "accept" => !has_unsafe_byte(v),
        "accept-language" | "content-language" => {
            v.iter().all(|&b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'*' | b',' | b'-' | b'.' | b';' | b'='))
        }
        "content-type" => !has_unsafe_byte(v) && safelisted_content_type(v),
        _ => false,
    }
}

/// Headers that make a request non-simple, lowercased and sorted.
pub fn unsafe_header_names(headers: &HeaderMap) -> Vec<String> {
    let mut names: Vec<String> = headers
        .iter()
        .filter(|(n, v)| !is_safelisted_header(n.as_str(), v))
        .map(|(n, _)| n.as_str().to_owned())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Requests without an initiator are browser-owned. An opaque or invalid
/// serialized initiator never becomes same-origin merely because URL parsing failed.
pub fn is_same_origin_request(req: &Request) -> bool {
    req.origin.as_deref().is_none_or(|o| Url::parse(o).is_ok_and(|o| same_origin(&o, &req.url)))
}

/// Whether a request needs CORS processing at all.
pub fn is_cors_request(req: &Request, _origin_url: Option<&Url>) -> bool {
    req.mode == RequestMode::Cors && !is_same_origin_request(req)
}

pub fn needs_preflight(req: &Request) -> bool {
    let simple_method = matches!(req.method, Method::GET | Method::HEAD | Method::POST);
    !simple_method || !unsafe_header_names(&req.headers).is_empty()
}

pub fn preflight_request_headers(req: &Request, origin: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(origin) {
        h.insert(header::ORIGIN, v);
    }
    if let Ok(v) = HeaderValue::from_str(req.method.as_str()) {
        h.insert(header::ACCESS_CONTROL_REQUEST_METHOD, v);
    }
    let names = unsafe_header_names(&req.headers);
    if !names.is_empty() {
        if let Ok(v) = HeaderValue::from_str(&names.join(",")) {
            h.insert(header::ACCESS_CONTROL_REQUEST_HEADERS, v);
        }
    }
    h.insert(header::ACCEPT, HeaderValue::from_static("*/*"));
    h
}

/// Response CORS check.
pub fn cors_check(headers: &HeaderMap, origin: &str, credentials: Credentials) -> Result<(), String> {
    let acao = headers
        .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        .and_then(|v| v.to_str().ok())
        .ok_or("No 'Access-Control-Allow-Origin' header is present on the requested resource")?;
    if credentials != Credentials::Include && acao == "*" {
        return Ok(());
    }
    if acao != origin {
        return Err(format!(
            "The 'Access-Control-Allow-Origin' header has a value '{acao}' that is not equal to the supplied origin"
        ));
    }
    if credentials == Credentials::Include {
        let acac = headers.get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS).and_then(|v| v.to_str().ok());
        if acac != Some("true") {
            return Err(
                "The value of 'Access-Control-Allow-Credentials' must be 'true' when credentials mode is 'include'"
                    .into(),
            );
        }
    }
    Ok(())
}

fn list(headers: &HeaderMap, name: header::HeaderName) -> Vec<String> {
    headers
        .get_all(name)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

#[derive(Clone, Debug)]
struct PreflightEntry {
    methods: Vec<String>,
    headers: Vec<String>,
    any_method: bool,
    any_header: bool,
    expires: Instant,
}

/// Validates a preflight response and returns the cache entry.
fn validate_preflight(status: u16, resp: &HeaderMap, req: &Request, origin: &str) -> Result<PreflightEntry, String> {
    if !(200..300).contains(&status) {
        return Err(format!("Response to preflight request doesn't pass access control check: status {status}"));
    }
    cors_check(resp, origin, req.credentials)?;
    let methods = list(resp, header::ACCESS_CONTROL_ALLOW_METHODS);
    let headers: Vec<String> =
        list(resp, header::ACCESS_CONTROL_ALLOW_HEADERS).into_iter().map(|h| h.to_ascii_lowercase()).collect();
    let wildcard_ok = req.credentials != Credentials::Include;
    let any_method = wildcard_ok && methods.iter().any(|m| m == "*");
    let any_header = wildcard_ok && headers.iter().any(|h| h == "*");
    let m = req.method.as_str();
    if !matches!(req.method, Method::GET | Method::HEAD | Method::POST)
        && !any_method
        && !methods.iter().any(|x| x == m)
    {
        return Err(format!("Method {m} is not allowed by Access-Control-Allow-Methods in preflight response"));
    }
    for h in unsafe_header_names(&req.headers) {
        // Authorization is never covered by the wildcard.
        let wild = any_header && h != "authorization";
        if !wild && !headers.contains(&h) {
            return Err(format!(
                "Request header field {h} is not allowed by Access-Control-Allow-Headers in preflight response"
            ));
        }
    }
    let max_age = resp
        .get(header::ACCESS_CONTROL_MAX_AGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(5)
        .min(7200);
    Ok(PreflightEntry {
        methods,
        headers,
        any_method,
        any_header,
        expires: Instant::now() + Duration::from_secs(max_age),
    })
}

/// Preflight cache keyed by (origin, url, credentials).
#[derive(Default)]
pub struct PreflightCache {
    map: FxHashMap<(String, String, bool), PreflightEntry>,
}

const PREFLIGHT_CACHE_CAP: usize = 1024;

impl PreflightCache {
    fn key(req: &Request, origin: &str) -> (String, String, bool) {
        (origin.to_owned(), req.url.as_str().to_owned(), req.credentials == Credentials::Include)
    }
    /// True if a fresh cache entry covers the request's method and headers.
    pub fn covers(&mut self, req: &Request, origin: &str) -> bool {
        let k = Self::key(req, origin);
        let Some(e) = self.map.get(&k) else { return false };
        if e.expires <= Instant::now() {
            self.map.remove(&k);
            return false;
        }
        let m = req.method.as_str();
        let method_ok = matches!(req.method, Method::GET | Method::HEAD | Method::POST)
            || e.any_method
            || e.methods.iter().any(|x| x == m);
        method_ok
            && unsafe_header_names(&req.headers)
                .iter()
                .all(|h| (e.any_header && h != "authorization") || e.headers.contains(h))
    }
    pub fn record(&mut self, status: u16, resp: &HeaderMap, req: &Request, origin: &str) -> Result<(), String> {
        let entry = validate_preflight(status, resp, req, origin)?;
        if self.map.len() >= PREFLIGHT_CACHE_CAP {
            let now = Instant::now();
            self.map.retain(|_, e| e.expires > now);
            if self.map.len() >= PREFLIGHT_CACHE_CAP {
                self.map.clear();
            }
        }
        self.map.insert(Self::key(req, origin), entry);
        Ok(())
    }
}

/// CORS-safelisted response header names plus Access-Control-Expose-Headers,
/// for filtering what a `cors` response exposes to script.
pub fn exposed_response_headers(headers: &HeaderMap, credentials: Credentials) -> Vec<String> {
    let mut out: Vec<String> =
        ["cache-control", "content-language", "content-length", "content-type", "expires", "last-modified", "pragma"]
            .iter()
            .map(|s| s.to_string())
            .collect();
    let expose = list(headers, header::ACCESS_CONTROL_EXPOSE_HEADERS);
    if credentials != Credentials::Include && expose.iter().any(|h| h == "*") {
        out.extend(headers.keys().map(|k| k.as_str().to_owned()));
    } else {
        out.extend(expose.into_iter().map(|h| h.to_ascii_lowercase()));
    }
    out.sort();
    out.dedup();
    out
}
