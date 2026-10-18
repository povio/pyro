//! Private HTTP cache (RFC 9111) owned by one `NetClient`, i.e. one browser
//! context. Its entries never cross contexts and die with the client. The only
//! cross-context storage is the separate, explicit opt-in [`SharedAssetCache`]
//! (off by default), whose eligibility contract is documented there.
//!
//! Supported:
//! - GET only. Keyed by the URL without its fragment, with up to
//!   `MAX_VARIANTS` stored responses per URL, selected by `Vary`.
//! - Freshness from `max-age`, else `Expires` - `Date` (RFC 9111 §4.2.1), and
//!   the age calculation of §4.2.3.
//! - Response directives `no-store` and `no-cache`. `private` is accepted
//!   (this is a private cache).
//! - Request directives `no-store`, `no-cache`, `max-age`, `min-fresh` and
//!   `only-if-cached`, `Pragma: no-cache` (§5.4), and the Fetch cache modes.
//! - Revalidation with `ETag`/`Last-Modified` and a 304 header merge (§4.3).
//! - Invalidation by unsafe methods (§4.4).
//! - Byte-bounded with LRU eviction.
//!
//! Deliberately unsupported (such responses are not stored, or are always
//! revalidated, never served unsafely):
//! - heuristic freshness: no explicit lifetime means revalidate when there
//!   are validators, otherwise do not store;
//! - serving stale content (`max-stale`, `stale-while-revalidate`,
//!   `stale-if-error`);
//! - partial content and `Range`;
//! - storing redirects;
//! - `Vary: *`;
//! - responses carrying `Set-Cookie`;
//! - partitioning within a context by top-level site;
//! - persistence across processes.

use std::collections::HashMap;

use bytes::Bytes;
use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use http::StatusCode;
use url::Url;

use crate::CacheMode;

const MAX_VARIANTS: usize = 4;
/// Per-entry bookkeeping charged on top of body and header bytes.
const ENTRY_OVERHEAD: usize = 256;

/// Parsed `Cache-Control` (plus `Pragma: no-cache` for requests).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Directives {
    pub no_store: bool,
    pub no_cache: bool,
    /// `None`: absent. `Some(None)`: present but invalid or duplicated, which
    /// means stale (RFC 9111 §4.2.1).
    pub max_age: Option<Option<u64>>,
    pub min_fresh: Option<u64>,
    pub only_if_cached: bool,
    /// Response-only, for the shared asset cache (RFC 9111 §5.2.2).
    pub public: bool,
    pub private: bool,
    pub s_maxage: Option<Option<u64>>,
}

impl Directives {
    pub fn parse(headers: &HeaderMap, request: bool) -> Directives {
        let mut d = Directives::default();
        let mut any = false;
        for value in headers.get_all(header::CACHE_CONTROL) {
            let Ok(text) = value.to_str() else { continue };
            for part in text.split(',') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                any = true;
                let (name, arg) = match part.split_once('=') {
                    Some((n, a)) => (n.trim().to_ascii_lowercase(), Some(a.trim().trim_matches('"'))),
                    None => (part.to_ascii_lowercase(), None),
                };
                let seconds = || arg.and_then(|a| a.parse::<u64>().ok());
                match name.as_str() {
                    "no-store" => d.no_store = true,
                    // Field-qualified no-cache is treated as unqualified: conservative.
                    "no-cache" => d.no_cache = true,
                    "max-age" => d.max_age = Some(if d.max_age.is_some() { None } else { seconds() }),
                    "s-maxage" if !request => d.s_maxage = Some(if d.s_maxage.is_some() { None } else { seconds() }),
                    "public" if !request => d.public = true,
                    "private" if !request => d.private = true,
                    "min-fresh" if request => d.min_fresh = seconds(),
                    "only-if-cached" if request => d.only_if_cached = true,
                    _ => {}
                }
            }
        }
        // RFC 9111 §5.4: Pragma: no-cache only counts when no Cache-Control is present.
        if request && !any {
            let pragma = headers.get_all(header::PRAGMA).iter().filter_map(|v| v.to_str().ok());
            if pragma.flat_map(|v| v.split(',')).any(|p| p.trim().eq_ignore_ascii_case("no-cache")) {
                d.no_cache = true;
            }
        }
        d
    }
}

/// A stored response returned by a lookup.
#[derive(Clone, Debug)]
pub(crate) struct Cached {
    pub id: u64,
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Cached {
    /// Conditional request headers for revalidation (RFC 9111 §4.3.1).
    pub fn validators(&self) -> Vec<(HeaderName, HeaderValue)> {
        let mut v = Vec::new();
        if let Some(etag) = self.headers.get(header::ETAG) {
            v.push((header::IF_NONE_MATCH, etag.clone()));
        }
        if let Some(lm) = self.headers.get(header::LAST_MODIFIED) {
            v.push((header::IF_MODIFIED_SINCE, lm.clone()));
        }
        v
    }
}

pub(crate) enum Lookup {
    Miss,
    /// Usable as-is. Headers carry the current `Age`.
    Hit(Cached),
    /// Stored but must be revalidated with these validators first.
    Revalidate(Cached),
}

struct Entry {
    id: u64,
    vary: Vec<(HeaderName, Option<String>)>,
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
    /// Wall-clock seconds (RFC 9111 §4.2.3 request_time / response_time).
    request_time: f64,
    response_time: f64,
    used: u64,
    size: usize,
}

impl Entry {
    fn current_age(&self, now: f64) -> f64 {
        let date = header_date(&self.headers, header::DATE).unwrap_or(self.response_time);
        let age_value = self
            .headers
            .get(header::AGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(0) as f64;
        let apparent_age = (self.response_time - date).max(0.0);
        let response_delay = (self.response_time - self.request_time).max(0.0);
        let corrected_initial_age = apparent_age.max(age_value + response_delay);
        corrected_initial_age + (now - self.response_time).max(0.0)
    }

    fn freshness_lifetime(&self, shared: bool) -> f64 {
        let d = Directives::parse(&self.headers, false);
        // A shared cache uses s-maxage in preference to max-age (RFC 9111 §5.2.2.10).
        let max_age = if shared && d.s_maxage.is_some() { d.s_maxage } else { d.max_age };
        match max_age {
            Some(Some(s)) => s as f64,
            Some(None) => 0.0,
            None => match self.headers.get(header::EXPIRES) {
                // An invalid Expires (e.g. "0") means already expired (RFC 9111 §5.3).
                Some(v) => {
                    let expires = v.to_str().ok().and_then(parse_http_date);
                    let date = header_date(&self.headers, header::DATE).unwrap_or(self.response_time);
                    expires.map_or(0.0, |e| (e - date).max(0.0))
                }
                None => 0.0,
            },
        }
    }

    fn has_validators(&self) -> bool {
        self.headers.contains_key(header::ETAG) || self.headers.contains_key(header::LAST_MODIFIED)
    }

    fn matches(&self, request: &HeaderMap) -> bool {
        self.vary.iter().all(|(name, value)| header_value(request, name) == *value)
    }

    fn cached(&self, age: f64) -> Cached {
        let mut headers = self.headers.clone();
        if let Ok(v) = HeaderValue::from_str(&(age.floor() as u64).to_string()) {
            headers.insert(header::AGE, v);
        }
        Cached { id: self.id, status: self.status, headers, body: self.body.clone() }
    }
}

pub(crate) struct HttpCache {
    /// Shared (cross-context) asset store: stricter storability, s-maxage.
    shared: bool,
    entries: HashMap<String, Vec<Entry>>,
    bytes: usize,
    max_bytes: usize,
    max_entry_bytes: usize,
    tick: u64,
    next_id: u64,
}

impl HttpCache {
    pub fn new(max_bytes: usize, max_entry_bytes: usize) -> Self {
        HttpCache {
            shared: false,
            entries: HashMap::new(),
            bytes: 0,
            max_bytes,
            max_entry_bytes: max_entry_bytes.min(max_bytes),
            tick: 0,
            next_id: 0,
        }
    }

    pub fn enabled(&self) -> bool {
        self.max_bytes > 0
    }

    /// Largest body this cache will store.
    pub fn max_entry_bytes(&self) -> usize {
        self.max_entry_bytes
    }

    /// (entries, logical bytes).
    pub fn usage(&self) -> (usize, usize) {
        (self.entries.values().map(Vec::len).sum(), self.bytes)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    pub fn lookup(&mut self, url: &Url, request: &HeaderMap, mode: CacheMode, now: f64) -> Lookup {
        if matches!(mode, CacheMode::NoStore | CacheMode::Reload) || !self.enabled() {
            return Lookup::Miss;
        }
        let req = Directives::parse(request, true);
        if req.no_store {
            return Lookup::Miss;
        }
        self.tick += 1;
        let tick = self.tick;
        let Some(entry) = self.entries.get_mut(&key(url)).and_then(|v| v.iter_mut().find(|e| e.matches(request)))
        else {
            return Lookup::Miss;
        };
        entry.used = tick;
        let age = entry.current_age(now);
        let lifetime = entry.freshness_lifetime(self.shared);
        if matches!(mode, CacheMode::ForceCache | CacheMode::OnlyIfCached) {
            return Lookup::Hit(entry.cached(age));
        }
        let response_no_cache = Directives::parse(&entry.headers, false).no_cache;
        let fresh_enough = lifetime > age
            && !response_no_cache
            && !req.no_cache
            && mode != CacheMode::NoCache
            && req.max_age.is_none_or(|m| m.is_some_and(|m| age <= m as f64))
            && req.min_fresh.is_none_or(|m| lifetime - age >= m as f64);
        if fresh_enough {
            Lookup::Hit(entry.cached(age))
        } else if entry.has_validators() {
            Lookup::Revalidate(entry.cached(age))
        } else {
            Lookup::Miss
        }
    }

    /// Whether a final response may be stored (RFC 9111 §3, plus the scope above).
    pub fn storable(&self, status: StatusCode, response: &HeaderMap, request: &HeaderMap, mode: CacheMode) -> bool {
        if !self.enabled() || mode == CacheMode::NoStore {
            return false;
        }
        if self.shared {
            return shared_storable(status, response, request);
        }
        if !matches!(status.as_u16(), 200 | 203 | 204 | 404 | 410) {
            return false;
        }
        let (req, resp) = (Directives::parse(request, true), Directives::parse(response, false));
        if req.no_store || resp.no_store {
            return false;
        }
        if response.contains_key(header::SET_COOKIE) || response.contains_key(header::CONTENT_RANGE) {
            return false;
        }
        if vary_names(response).is_none() {
            return false; // Vary: *
        }
        let explicit = resp.max_age.is_some() || response.contains_key(header::EXPIRES);
        explicit || response.contains_key(header::ETAG) || response.contains_key(header::LAST_MODIFIED)
    }

    /// Stores a complete response. `request` is the header map actually sent,
    /// so that `Vary` is matched against what the server saw.
    #[allow(clippy::too_many_arguments)]
    pub fn insert(
        &mut self,
        url: &Url,
        request: &HeaderMap,
        status: StatusCode,
        response: &HeaderMap,
        body: Bytes,
        request_time: f64,
        response_time: f64,
    ) {
        let Some(names) = vary_names(response) else { return };
        if body.len() > self.max_entry_bytes {
            return;
        }
        let vary: Vec<_> = names.into_iter().map(|n| (n.clone(), header_value(request, &n))).collect();
        let headers = storable_headers(response);
        let k = key(url);
        let size = body.len() + header_bytes(&headers) + k.len() + ENTRY_OVERHEAD;
        if size > self.max_bytes {
            return;
        }
        self.tick += 1;
        self.next_id += 1;
        let entry =
            Entry { id: self.next_id, vary, status, headers, body, request_time, response_time, used: self.tick, size };
        let variants = self.entries.entry(k).or_default();
        if let Some(i) = variants.iter().position(|e| e.vary == entry.vary) {
            self.bytes -= variants.remove(i).size;
        }
        if variants.len() >= MAX_VARIANTS {
            let lru = variants.iter().enumerate().min_by_key(|(_, e)| e.used).map(|(i, _)| i).unwrap();
            self.bytes -= variants.remove(lru).size;
        }
        self.bytes += entry.size;
        variants.push(entry);
        self.evict();
    }

    /// Applies a 304 to the entry it revalidated (RFC 9111 §4.3.4). Returns
    /// the updated response, or `None` if it no longer matches: the entry is
    /// gone, or a strong ETag differs. The caller must then fetch unconditionally.
    pub fn update_from_304(
        &mut self,
        url: &Url,
        id: u64,
        not_modified: &HeaderMap,
        request_time: f64,
        response_time: f64,
    ) -> Option<Cached> {
        let k = key(url);
        let variants = self.entries.get_mut(&k)?;
        let i = variants.iter().position(|e| e.id == id)?;
        let stored_etag = variants[i].headers.get(header::ETAG).cloned();
        if let (Some(new), Some(old)) = (not_modified.get(header::ETAG), stored_etag.as_ref()) {
            if new != old {
                let removed = variants.remove(i);
                self.bytes -= removed.size;
                return None;
            }
        }
        let entry = &mut variants[i];
        for name in not_modified.keys() {
            if is_hop_by_hop(name) || name == header::CONTENT_LENGTH || name == header::CONTENT_ENCODING {
                continue;
            }
            entry.headers.remove(name);
            for v in not_modified.get_all(name) {
                entry.headers.append(name.clone(), v.clone());
            }
        }
        entry.request_time = request_time;
        entry.response_time = response_time;
        self.tick += 1;
        entry.used = self.tick;
        let old = entry.size;
        entry.size = entry.body.len() + header_bytes(&entry.headers) + k.len() + ENTRY_OVERHEAD;
        let age = entry.current_age(response_time);
        let cached = entry.cached(age);
        self.bytes = self.bytes - old + entry.size;
        self.evict();
        Some(cached)
    }

    /// Drops every stored response for `url` (RFC 9111 §4.4).
    pub fn invalidate(&mut self, url: &Url) {
        if let Some(v) = self.entries.remove(&key(url)) {
            self.bytes -= v.iter().map(|e| e.size).sum::<usize>();
        }
    }

    fn evict(&mut self) {
        while self.bytes > self.max_bytes {
            let Some((k, i)) = self
                .entries
                .iter()
                .flat_map(|(k, v)| v.iter().enumerate().map(move |(i, e)| (e.used, k, i)))
                .min_by_key(|(used, _, _)| *used)
                .map(|(_, k, i)| (k.clone(), i))
            else {
                break;
            };
            let variants = self.entries.get_mut(&k).unwrap();
            self.bytes -= variants.remove(i).size;
            if variants.is_empty() {
                self.entries.remove(&k);
            }
        }
    }
}

/// Requests that may use or fill the shared asset cache: no credential-bearing
/// headers, so nothing personalized can be keyed or served by URL alone.
pub(crate) fn shared_request_eligible(request: &HeaderMap) -> bool {
    ![header::AUTHORIZATION, header::PROXY_AUTHORIZATION, header::COOKIE].iter().any(|h| request.contains_key(h))
}

/// Shared asset storability: a 200 explicitly marked `public` with an explicit
/// positive lifetime (`s-maxage` or `max-age`). Never `private`, `no-store` or
/// `no-cache`; no `Set-Cookie`; no `Vary` (one representation per URL); no
/// `Content-Range`. The request must carry no credential-bearing headers.
fn shared_storable(status: StatusCode, response: &HeaderMap, request: &HeaderMap) -> bool {
    if status != StatusCode::OK || !shared_request_eligible(request) {
        return false;
    }
    let (req, resp) = (Directives::parse(request, true), Directives::parse(response, false));
    if req.no_store || !resp.public || resp.private || resp.no_store || resp.no_cache {
        return false;
    }
    let lifetime = match (resp.s_maxage, resp.max_age) {
        (Some(s), _) => s,
        (None, m) => m.flatten(),
    };
    lifetime.is_some_and(|l| l > 0)
        && !response.contains_key(header::SET_COOKIE)
        && !response.contains_key(header::VARY)
        && !response.contains_key(header::CONTENT_RANGE)
}

/// Opt-in, process-wide shared cache used by every `NetClient` (browser context)
/// configured with the same handle; off unless explicitly enabled.
///
/// The contract is header- and credential-based, not MIME-, path- or
/// destination-based. ANY response is shared across those contexts when it is a
/// GET 200 explicitly marked `public` with a positive `s-maxage`/`max-age`; is
/// not `private`/`no-store`/`no-cache`; has no `Set-Cookie`, `Vary` or
/// `Content-Range`; and the request carried no `Authorization`,
/// `Proxy-Authorization` or `Cookie` ([`shared_storable`],
/// [`shared_request_eligible`]). Only fresh entries are served; there is no
/// revalidation.
/// - Never shared: cookie jars, credentialed or non-`public` responses, and
///   page state.
/// - A server that marks a response `public` and long-lived is trusted to mean
///   it for every client of that origin, as with any shared HTTP cache.
/// - In the TOS workload that motivated it, only content-hashed `/assets/` files
///   qualify. Its documents and API responses carry no such headers and are
///   requested with credentials. That is an observation about that workload,
///   not a guarantee for other sites.
#[derive(Clone)]
pub struct SharedAssetCache(std::sync::Arc<std::sync::Mutex<HttpCache>>);

impl std::fmt::Debug for SharedAssetCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (entries, bytes) = self.usage();
        f.debug_struct("SharedAssetCache").field("entries", &entries).field("bytes", &bytes).finish()
    }
}

/// Environment opt-in: whole MiB for the process-wide shared asset cache.
pub const SHARED_ASSET_CACHE_ENV: &str = "LIGHTNINGPANDA_SHARED_ASSET_CACHE_MB";

impl SharedAssetCache {
    pub fn new(max_bytes: usize, max_entry_bytes: usize) -> Self {
        let mut cache = HttpCache::new(max_bytes, max_entry_bytes);
        cache.shared = true;
        SharedAssetCache(std::sync::Arc::new(std::sync::Mutex::new(cache)))
    }

    /// The process-wide instance when `LIGHTNINGPANDA_SHARED_ASSET_CACHE_MB` is
    /// a positive integer (read once; the first size wins), else `None`.
    pub fn from_env() -> Option<Self> {
        static GLOBAL: std::sync::OnceLock<Option<SharedAssetCache>> = std::sync::OnceLock::new();
        GLOBAL
            .get_or_init(|| {
                let mb = std::env::var(SHARED_ASSET_CACHE_ENV).ok()?.trim().parse::<usize>().ok()?;
                (mb > 0).then(|| SharedAssetCache::new(mb * 1024 * 1024, (mb * 1024 * 1024).min(16 * 1024 * 1024)))
            })
            .clone()
    }

    /// (entries, logical bytes).
    pub fn usage(&self) -> (usize, usize) {
        self.0.lock().unwrap().usage()
    }

    pub fn clear(&self) {
        self.0.lock().unwrap().clear();
    }

    pub(crate) fn with<R>(&self, f: impl FnOnce(&mut HttpCache) -> R) -> R {
        f(&mut self.0.lock().unwrap())
    }
}

fn key(url: &Url) -> String {
    let mut u = url.clone();
    u.set_fragment(None);
    u.into()
}

/// `Vary` field names, `None` for `Vary: *`.
fn vary_names(headers: &HeaderMap) -> Option<Vec<HeaderName>> {
    let mut out = Vec::new();
    for v in headers.get_all(header::VARY) {
        for name in v.to_str().unwrap_or("*").split(',').map(str::trim).filter(|n| !n.is_empty()) {
            if name == "*" {
                return None;
            }
            if let Ok(n) = HeaderName::from_bytes(name.to_ascii_lowercase().as_bytes()) {
                if !out.contains(&n) {
                    out.push(n);
                }
            }
        }
    }
    Some(out)
}

/// A request header's combined value, whitespace-normalized for Vary matching.
fn header_value(headers: &HeaderMap, name: &HeaderName) -> Option<String> {
    let values: Vec<String> = headers
        .get_all(name)
        .iter()
        .map(|v| String::from_utf8_lossy(v.as_bytes()).split(',').map(str::trim).collect::<Vec<_>>().join(","))
        .collect();
    (!values.is_empty()).then(|| values.join(","))
}

fn is_hop_by_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

/// RFC 9111 §3.1: do not store connection-specific fields.
fn storable_headers(headers: &HeaderMap) -> HeaderMap {
    let connection: Vec<String> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(',').map(|s| s.trim().to_ascii_lowercase()))
        .collect();
    let mut out = HeaderMap::new();
    for (name, value) in headers {
        if !is_hop_by_hop(name) && !connection.iter().any(|c| c == name.as_str()) {
            out.append(name.clone(), value.clone());
        }
    }
    out
}

fn header_bytes(headers: &HeaderMap) -> usize {
    headers.iter().map(|(n, v)| n.as_str().len() + v.len() + 32).sum()
}

fn header_date(headers: &HeaderMap, name: HeaderName) -> Option<f64> {
    headers.get(name).and_then(|v| v.to_str().ok()).and_then(parse_http_date)
}

/// HTTP-date (RFC 9110 §5.6.7): IMF-fixdate, plus the obsolete RFC 850 and
/// asctime forms recipients must accept. Returns Unix seconds.
pub(crate) fn parse_http_date(s: &str) -> Option<f64> {
    let s = s.trim();
    let month = |m: &str| {
        ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"]
            .iter()
            .position(|x| x.eq_ignore_ascii_case(m))
            .map(|i| i as u32 + 1)
    };
    let hms = |t: &str| -> Option<(u32, u32, u32)> {
        let mut it = t.split(':').map(|x| x.parse::<u32>().ok());
        let (h, m, s) = (it.next()??, it.next()??, it.next()??);
        (it.next().is_none() && h < 24 && m < 60 && s < 61).then_some((h, m, s))
    };
    let (y, mo, d, (h, mi, se)) = if let Some((_, rest)) = s.split_once(", ") {
        let p: Vec<&str> = rest.split_whitespace().collect();
        if p.len() == 5 && p[4] == "GMT" {
            // IMF-fixdate: Sun, 06 Nov 1994 08:49:37 GMT
            (p[2].parse::<i64>().ok()?, month(p[1])?, p[0].parse::<u32>().ok()?, hms(p[3])?)
        } else if p.len() == 3 && p[2] == "GMT" {
            // RFC 850: Sunday, 06-Nov-94 08:49:37 GMT. A two-digit year more than
            // 50 years in the future means the most recent past year with those
            // digits (RFC 9110 §5.6.7).
            let dmy: Vec<&str> = p[0].split('-').collect();
            if dmy.len() != 3 {
                return None;
            }
            let yy = dmy[2].parse::<i64>().ok()?;
            let y = if dmy[2].len() == 2 {
                let now = (crate::cookies::now_secs() / 86400.0 / 365.2425) as i64 + 1970;
                let candidate = now - now.rem_euclid(100) + yy;
                if candidate > now + 50 {
                    candidate - 100
                } else {
                    candidate
                }
            } else {
                yy
            };
            (y, month(dmy[1])?, dmy[0].parse::<u32>().ok()?, hms(p[1])?)
        } else {
            return None;
        }
    } else {
        // asctime: Sun Nov  6 08:49:37 1994
        let p: Vec<&str> = s.split_whitespace().collect();
        if p.len() != 5 {
            return None;
        }
        (p[4].parse::<i64>().ok()?, month(p[1])?, p[2].parse::<u32>().ok()?, hms(p[3])?)
    };
    if !(1..=31).contains(&d) {
        return None;
    }
    Some((days_from_civil(y, mo, d) * 86400 + (h * 3600 + mi * 60 + se) as i64) as f64)
}

/// Days since 1970-01-01 (Howard Hinnant's civil-from-days inverse).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.append(*k, v.parse().unwrap());
        }
        m
    }
    fn url(p: &str) -> Url {
        Url::parse(&format!("http://a.test{p}")).unwrap()
    }
    const T0: f64 = 784111777.0; // Sun, 06 Nov 1994 08:49:37 GMT

    #[test]
    fn http_dates_in_all_three_forms() {
        assert_eq!(parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"), Some(T0));
        assert_eq!(parse_http_date("Sunday, 06-Nov-94 08:49:37 GMT"), Some(T0));
        assert_eq!(parse_http_date("Sun Nov  6 08:49:37 1994"), Some(T0));
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0.0));
        assert_eq!(parse_http_date("Wed, 29 Feb 2024 12:00:00 GMT"), Some(1709208000.0));
        for bad in ["0", "", "Sun, 06 Nov 1994 25:00:00 GMT", "Sun, 06 Foo 1994 08:49:37 GMT", "-1"] {
            assert_eq!(parse_http_date(bad), None, "{bad}");
        }
    }

    #[test]
    fn directives_parse_requests_and_responses() {
        let d = Directives::parse(
            &h(&[("cache-control", "No-Store, max-age=\"60\""), ("cache-control", "min-fresh=5")]),
            true,
        );
        assert_eq!((d.no_store, d.max_age, d.min_fresh), (true, Some(Some(60)), Some(5)));
        assert_eq!(Directives::parse(&h(&[("cache-control", "max-age=1, max-age=2")]), false).max_age, Some(None));
        assert_eq!(Directives::parse(&h(&[("cache-control", "max-age=abc")]), false).max_age, Some(None));
        assert!(Directives::parse(&h(&[("pragma", "no-cache")]), true).no_cache);
        assert!(!Directives::parse(&h(&[("pragma", "no-cache"), ("cache-control", "max-age=5")]), true).no_cache);
        assert!(!Directives::parse(&h(&[("pragma", "no-cache")]), false).no_cache, "Pragma is request-only");
        assert!(Directives::parse(&h(&[("cache-control", "no-cache=\"set-cookie\"")]), false).no_cache);
    }

    #[test]
    fn freshness_and_age_follow_rfc9111() {
        let mut c = HttpCache::new(1 << 20, 1 << 20);
        let date = "Sun, 06 Nov 1994 08:49:37 GMT";
        let req = HeaderMap::new();
        // max-age=100, Age: 10, received 2 s after Date, 1 s response delay.
        c.insert(
            &url("/a"),
            &req,
            StatusCode::OK,
            &h(&[("cache-control", "max-age=100"), ("date", date), ("age", "10")]),
            Bytes::from_static(b"x"),
            T0 + 1.0,
            T0 + 2.0,
        );
        // corrected initial age = max(2, 10 + 1) = 11; at T0 + 2 + 88 the age is 99 < 100: fresh.
        assert!(
            matches!(c.lookup(&url("/a"), &req, CacheMode::Default, T0 + 90.0), Lookup::Hit(ref x) if x.headers[header::AGE] == "99")
        );
        // At age 101 it is stale, and without validators it is a miss.
        assert!(matches!(c.lookup(&url("/a"), &req, CacheMode::Default, T0 + 92.0), Lookup::Miss));
        // Expires - Date gives the lifetime when there is no max-age; an invalid Expires is stale.
        c.insert(
            &url("/e"),
            &req,
            StatusCode::OK,
            &h(&[("date", date), ("expires", "Sun, 06 Nov 1994 08:50:37 GMT"), ("etag", "\"1\"")]),
            Bytes::new(),
            T0,
            T0,
        );
        assert!(matches!(c.lookup(&url("/e"), &req, CacheMode::Default, T0 + 59.0), Lookup::Hit(_)));
        assert!(matches!(c.lookup(&url("/e"), &req, CacheMode::Default, T0 + 61.0), Lookup::Revalidate(_)));
        c.insert(
            &url("/z"),
            &req,
            StatusCode::OK,
            &h(&[("date", date), ("expires", "0"), ("etag", "\"1\"")]),
            Bytes::new(),
            T0,
            T0,
        );
        assert!(matches!(c.lookup(&url("/z"), &req, CacheMode::Default, T0), Lookup::Revalidate(_)));
        // Request max-age / min-fresh constrain a fresh entry.
        let r = h(&[("cache-control", "max-age=50")]);
        assert!(matches!(c.lookup(&url("/a"), &r, CacheMode::Default, T0 + 60.0), Lookup::Miss));
        let r = h(&[("cache-control", "min-fresh=30")]);
        assert!(matches!(c.lookup(&url("/e"), &r, CacheMode::Default, T0 + 40.0), Lookup::Revalidate(_)));
        // force-cache / only-if-cached use stale entries.
        assert!(matches!(c.lookup(&url("/a"), &req, CacheMode::ForceCache, T0 + 1000.0), Lookup::Hit(_)));
    }

    #[test]
    fn storability_rules() {
        let c = HttpCache::new(1 << 20, 1 << 20);
        let none = HeaderMap::new();
        let ok = |resp: &[(&'static str, &str)]| c.storable(StatusCode::OK, &h(resp), &none, CacheMode::Default);
        assert!(ok(&[("cache-control", "max-age=60")]));
        assert!(ok(&[("etag", "\"v\"")]), "validators allow storing for revalidation");
        assert!(ok(&[("cache-control", "private, max-age=60")]), "private cache");
        assert!(!ok(&[]), "no heuristic freshness");
        assert!(!ok(&[("cache-control", "no-store, max-age=60")]));
        assert!(!ok(&[("cache-control", "max-age=60"), ("vary", "*")]));
        assert!(!ok(&[("cache-control", "max-age=60"), ("set-cookie", "a=b")]));
        assert!(!c.storable(
            StatusCode::PARTIAL_CONTENT,
            &h(&[("cache-control", "max-age=60")]),
            &none,
            CacheMode::Default
        ));
        assert!(!c.storable(
            StatusCode::MOVED_PERMANENTLY,
            &h(&[("cache-control", "max-age=60")]),
            &none,
            CacheMode::Default
        ));
        assert!(!c.storable(
            StatusCode::OK,
            &h(&[("cache-control", "max-age=60")]),
            &h(&[("cache-control", "no-store")]),
            CacheMode::Default
        ));
        assert!(!c.storable(StatusCode::OK, &h(&[("cache-control", "max-age=60")]), &none, CacheMode::NoStore));
        assert!(!HttpCache::new(0, 0).storable(
            StatusCode::OK,
            &h(&[("cache-control", "max-age=60")]),
            &none,
            CacheMode::Default
        ));
    }

    #[test]
    fn vary_selects_variants_and_304_merges_headers() {
        let mut c = HttpCache::new(1 << 20, 1 << 20);
        let resp = h(&[
            ("cache-control", "max-age=60"),
            ("vary", "Accept-Language"),
            ("etag", "\"v1\""),
            ("connection", "x-hop"),
            ("x-hop", "1"),
        ]);
        let en = h(&[("accept-language", "en")]);
        let de = h(&[("accept-language", "de")]);
        c.insert(&url("/v"), &en, StatusCode::OK, &resp, Bytes::from_static(b"en"), T0, T0);
        assert!(matches!(c.lookup(&url("/v"), &de, CacheMode::Default, T0), Lookup::Miss));
        c.insert(&url("/v"), &de, StatusCode::OK, &resp, Bytes::from_static(b"de"), T0, T0);
        let Lookup::Hit(hit) = c.lookup(&url("/v#frag"), &en, CacheMode::Default, T0) else { panic!() };
        assert_eq!(hit.body, "en");
        assert!(
            !hit.headers.contains_key("x-hop") && !hit.headers.contains_key(header::CONNECTION),
            "connection-listed fields are not stored"
        );
        assert_eq!(c.usage().0, 2);
        // 304: headers replaced, except Content-Length; the body is kept.
        let merged = c
            .update_from_304(
                &url("/v"),
                hit.id,
                &h(&[("etag", "\"v1\""), ("cache-control", "max-age=120"), ("x-new", "1"), ("content-length", "0")]),
                T0 + 5.0,
                T0 + 5.0,
            )
            .unwrap();
        assert_eq!(merged.body.as_ref(), b"en");
        assert_eq!(merged.headers["cache-control"], "max-age=120");
        assert_eq!(merged.headers["x-new"], "1");
        assert!(!merged.headers.contains_key(header::CONTENT_LENGTH));
        // A 304 with a different strong ETag does not match: the entry is dropped.
        assert!(c.update_from_304(&url("/v"), hit.id, &h(&[("etag", "\"v2\"")]), T0, T0).is_none());
        assert_eq!(c.usage().0, 1);
        c.invalidate(&url("/v"));
        assert_eq!(c.usage(), (0, 0));
    }

    #[test]
    fn bounded_bytes_evict_least_recently_used() {
        let body = Bytes::from(vec![0u8; 1000]);
        let fresh = h(&[("cache-control", "max-age=60")]);
        let none = HeaderMap::new();
        let mut c = HttpCache::new(3000, 1500);
        c.insert(&url("/1"), &none, StatusCode::OK, &fresh, body.clone(), T0, T0);
        c.insert(&url("/2"), &none, StatusCode::OK, &fresh, body.clone(), T0, T0);
        assert!(matches!(c.lookup(&url("/1"), &none, CacheMode::Default, T0), Lookup::Hit(_))); // /1 now more recent
        c.insert(&url("/3"), &none, StatusCode::OK, &fresh, body.clone(), T0, T0);
        assert!(c.usage().1 <= 3000);
        assert!(matches!(c.lookup(&url("/2"), &none, CacheMode::Default, T0), Lookup::Miss), "LRU evicted");
        assert!(matches!(c.lookup(&url("/1"), &none, CacheMode::Default, T0), Lookup::Hit(_)));
        c.insert(&url("/big"), &none, StatusCode::OK, &fresh, Bytes::from(vec![0u8; 2000]), T0, T0);
        assert!(
            matches!(c.lookup(&url("/big"), &none, CacheMode::Default, T0), Lookup::Miss),
            "over the per-entry cap"
        );
        c.clear();
        assert_eq!(c.usage(), (0, 0));
    }
}
