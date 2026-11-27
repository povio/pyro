//! Client Hints (WICG Client Hints Infrastructure, UA Client Hints), the part the network layer owns.
//!
//! - The low-entropy UA hints (`Sec-CH-UA`, `-Mobile`, `-Platform`) go to every potentially
//!   trustworthy request of a browser whose user agent is not overridden (an override without
//!   metadata sends none, as Chrome; one with metadata brings its own, see `RequestDefaults`).
//! - `Accept-CH` on a top-level document response (a redirect's included) replaces the hints the
//!   response's origin asked for, kept for the session. They go to later requests to that origin
//!   from a same-origin context, and to top-level navigations. As Chrome, a response that is not
//!   from a potentially trustworthy URL, and a subframe's, do not set them. An empty header, or any
//!   empty one among several, clears the set; so does a malformed one.
//! - `Clear-Site-Data` ("cache", "cookies", "storage", "clientHints", "*") from a top-level
//!   document or a same-origin response clears the origin's set after the same response's `Accept-CH`.
//!
//! Not done: `<meta http-equiv="Accept-CH">`/`Delegate-CH`, `Critical-CH` restarts, delegation to
//! other origins through Permissions-Policy, `Sec-CH-Width`, and the real viewport/DPR values.

use std::collections::{HashMap, HashSet};

use http::{HeaderMap, HeaderName, HeaderValue};
use url::Url;

pub(crate) const ACCEPT_CH: HeaderName = HeaderName::from_static("accept-ch");

/// The hint headers a page can ask for, in the lowercase names `Accept-CH` lists them by.
const KNOWN: &[&str] = &[
    "device-memory",
    "sec-ch-device-memory",
    "dpr",
    "sec-ch-dpr",
    "viewport-width",
    "sec-ch-viewport-width",
    "sec-ch-viewport-height",
    "rtt",
    "downlink",
    "ect",
    "sec-ch-prefers-color-scheme",
    "sec-ch-prefers-reduced-motion",
    "sec-ch-prefers-reduced-transparency",
    "sec-ch-ua",
    "sec-ch-ua-arch",
    "sec-ch-ua-platform",
    "sec-ch-ua-model",
    "sec-ch-ua-mobile",
    "sec-ch-ua-full-version",
    "sec-ch-ua-full-version-list",
    "sec-ch-ua-platform-version",
    "sec-ch-ua-bitness",
    "sec-ch-ua-wow64",
];

/// Brand lists of the default (not overridden) agent: Chromium's GREASE brand and Chromium.
const BRANDS: &str = "\"Not.A/Brand\";v=\"99\", \"Chromium\";v=\"141\"";
const FULL_BRANDS: &str = "\"Not.A/Brand\";v=\"99.0.0.0\", \"Chromium\";v=\"141.0.0.0\"";

fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "\"macOS\""
    } else {
        "\"Linux\""
    }
}

/// The three hints sent without an `Accept-CH` opt-in.
pub(crate) fn low_entropy() -> [(HeaderName, HeaderValue); 3] {
    [
        (HeaderName::from_static("sec-ch-ua"), HeaderValue::from_static(BRANDS)),
        (HeaderName::from_static("sec-ch-ua-mobile"), HeaderValue::from_static("?0")),
        (HeaderName::from_static("sec-ch-ua-platform"), HeaderValue::from_str(platform()).unwrap_or(HeaderValue::from_static("\"\""))),
    ]
}

/// The value of one hint of [`KNOWN`]; `ua` is false when the agent is overridden (its own
/// identity is not the one these values describe: the UA hints are left out).
fn value(name: &str, ua: bool) -> Option<HeaderValue> {
    let v = match name {
        "device-memory" | "sec-ch-device-memory" => "8",
        "dpr" | "sec-ch-dpr" => "1",
        "viewport-width" | "sec-ch-viewport-width" => "800",
        "sec-ch-viewport-height" => "600",
        "rtt" => "50",
        "downlink" => "10",
        "ect" => "4g",
        "sec-ch-prefers-color-scheme" => "\"light\"",
        "sec-ch-prefers-reduced-motion" | "sec-ch-prefers-reduced-transparency" => "\"no-preference\"",
        _ if !ua => return None,
        "sec-ch-ua" => BRANDS,
        "sec-ch-ua-full-version-list" => FULL_BRANDS,
        "sec-ch-ua-platform" => platform(),
        "sec-ch-ua-mobile" => "?0",
        "sec-ch-ua-arch" => {
            if cfg!(target_arch = "aarch64") {
                "\"arm\""
            } else {
                "\"x86\""
            }
        }
        "sec-ch-ua-model" => "\"\"",
        "sec-ch-ua-full-version" => "\"141.0.0.0\"",
        "sec-ch-ua-platform-version" => "\"15.0.0\"",
        "sec-ch-ua-bitness" => "\"64\"",
        "sec-ch-ua-wow64" => "?0",
        _ => return None,
    };
    Some(HeaderValue::from_static(v))
}

/// The hints an `Accept-CH` header set lists, `None` when the response has no such header.
/// An empty value, or a malformed one (a structured-header list of tokens: whitespace inside an
/// item, an empty item) gives the empty list, which clears the origin's set as every other
/// empty header does. Unknown tokens are valid and ignored.
pub(crate) fn parse_accept_ch(headers: &HeaderMap) -> Option<Vec<&'static str>> {
    if !headers.contains_key(&ACCEPT_CH) {
        return None;
    }
    let mut hints = Vec::new();
    for v in headers.get_all(&ACCEPT_CH) {
        let Ok(text) = v.to_str() else { return Some(Vec::new()) };
        let text = text.trim();
        if text.is_empty() {
            // Chrome: one empty header among several drops them all.
            return Some(Vec::new());
        }
        for item in text.split(',') {
            let token = item.split(';').next().unwrap_or("").trim();
            let valid = !token.is_empty()
                && token.chars().all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~:/".contains(c))
                && token.starts_with(|c: char| c.is_ascii_alphabetic() || c == '*');
            if !valid {
                return Some(Vec::new());
            }
            if let Some(known) = KNOWN.iter().find(|k| k.eq_ignore_ascii_case(token)) {
                if !hints.contains(known) {
                    hints.push(*known);
                }
            }
        }
    }
    Some(hints)
}

/// Whether a `Clear-Site-Data` header names a type that drops the hints.
pub(crate) fn clears_hints(headers: &HeaderMap) -> bool {
    headers
        .get_all(crate::dictionary::CLEAR_SITE_DATA)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|t| matches!(t.trim().trim_matches('"'), "cache" | "cookies" | "storage" | "clientHints" | "*"))
}

/// The hints each origin (serialized) asked for through `Accept-CH`.
#[derive(Default)]
pub(crate) struct Store {
    by_origin: HashMap<String, HashSet<&'static str>>,
}

impl Store {
    /// A response from `url`; `top_level` for a top-level document's, `same_origin` for one a
    /// same-origin context fetched.
    pub(crate) fn response(&mut self, url: &Url, headers: &HeaderMap, top_level: bool, same_origin: bool) {
        if !crate::cookies::is_trustworthy(url) {
            return;
        }
        let origin = url.origin().ascii_serialization();
        if top_level {
            if let Some(hints) = parse_accept_ch(headers) {
                if hints.is_empty() {
                    self.by_origin.remove(&origin);
                } else {
                    self.by_origin.insert(origin.clone(), hints.into_iter().collect());
                }
            }
        }
        if (top_level || same_origin) && clears_hints(headers) {
            self.by_origin.remove(&origin);
        }
    }

    /// Adds the hints `url`'s origin asked for to `h` (a header the request has keeps its value).
    pub(crate) fn add_to(&self, url: &Url, ua: bool, h: &mut HeaderMap) {
        let Some(hints) = self.by_origin.get(&url.origin().ascii_serialization()) else { return };
        for &name in hints {
            if let Some(v) = value(name, ua) {
                h.entry(HeaderName::from_static(name)).or_insert(v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(values: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for v in values {
            h.append(ACCEPT_CH, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn accept_ch_parses_split_empty_and_malformed_headers() {
        assert_eq!(parse_accept_ch(&HeaderMap::new()), None);
        assert_eq!(parse_accept_ch(&headers(&["Sec-CH-DPR, DPR"])), Some(vec!["sec-ch-dpr", "dpr"]));
        assert_eq!(parse_accept_ch(&headers(&["rtt", "ect"])), Some(vec!["rtt", "ect"]));
        assert_eq!(parse_accept_ch(&headers(&["", "rtt", ""])), Some(vec![]));
        assert_eq!(parse_accept_ch(&headers(&["device memory"])), Some(vec![]));
        assert_eq!(parse_accept_ch(&headers(&["unknown-hint, rtt"])), Some(vec!["rtt"]));
    }

    #[test]
    fn hints_are_kept_per_origin_until_cleared() {
        let url = Url::parse("https://a.test/x").unwrap();
        let mut store = Store::default();
        store.response(&url, &headers(&["sec-ch-device-memory"]), true, true);
        let mut h = HeaderMap::new();
        store.add_to(&url, true, &mut h);
        assert_eq!(h.get("sec-ch-device-memory").unwrap().to_str().unwrap(), "8");
        // A subframe does not set them.
        let other = Url::parse("https://b.test/").unwrap();
        store.response(&other, &headers(&["rtt"]), false, false);
        let mut h = HeaderMap::new();
        store.add_to(&other, true, &mut h);
        assert!(h.is_empty());
        // Clear-Site-Data after the same response's Accept-CH wins.
        let mut cleared = headers(&["rtt"]);
        cleared.insert(crate::dictionary::CLEAR_SITE_DATA, HeaderValue::from_static("\"cache\""));
        store.response(&url, &cleared, true, true);
        let mut h = HeaderMap::new();
        store.add_to(&url, true, &mut h);
        assert!(h.is_empty());
    }
}
