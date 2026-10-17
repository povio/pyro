//! Engine-independent `Fetch` domain state: `Fetch.enable` patterns, the
//! wildcard matcher and the paused-request table. Ported from upstream
//! `src/server/cdp/domains/fetch.zig` (`InterceptState`).
//!
//! Playwright 1.59 enables `{urlPattern:"*", requestStage:"Request"}` with
//! `handleAuthRequests:true` whenever a route is installed and expects a
//! `Fetch.requestPaused` for every request (including the Document), answered
//! by continue/fulfill/fail/continueWithAuth.

use std::collections::HashMap;

use serde::Deserialize;

use crate::msg::{CdpError, CdpResult};

/// Resource types as the Fetch domain reports them. Chrome's Fetch domain
/// does not tell XHR, fetch() and EventSource apart: all pause as `XHR`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ResourceType {
    Document,
    Stylesheet,
    Image,
    Media,
    Font,
    Script,
    TextTrack,
    XHR,
    Fetch,
    Prefetch,
    EventSource,
    WebSocket,
    Manifest,
    SignedExchange,
    Ping,
    CSPViolationReport,
    Preflight,
    FedCM,
    Other,
}

impl ResourceType {
    /// Type reported in `Fetch.requestPaused` for a request of this kind.
    pub fn fetch_reported(self) -> Self {
        match self {
            ResourceType::Fetch | ResourceType::EventSource => ResourceType::XHR,
            other => other,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ResourceType::Document => "Document",
            ResourceType::Stylesheet => "Stylesheet",
            ResourceType::Image => "Image",
            ResourceType::Media => "Media",
            ResourceType::Font => "Font",
            ResourceType::Script => "Script",
            ResourceType::TextTrack => "TextTrack",
            ResourceType::XHR => "XHR",
            ResourceType::Fetch => "Fetch",
            ResourceType::Prefetch => "Prefetch",
            ResourceType::EventSource => "EventSource",
            ResourceType::WebSocket => "WebSocket",
            ResourceType::Manifest => "Manifest",
            ResourceType::SignedExchange => "SignedExchange",
            ResourceType::Ping => "Ping",
            ResourceType::CSPViolationReport => "CSPViolationReport",
            ResourceType::Preflight => "Preflight",
            ResourceType::FedCM => "FedCM",
            ResourceType::Other => "Other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub enum RequestStage {
    #[default]
    Request,
    Response,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPattern {
    #[serde(default = "star")]
    pub url_pattern: String,
    #[serde(default)]
    pub resource_type: Option<ResourceType>,
    #[serde(default)]
    pub request_stage: RequestStage,
}

fn star() -> String {
    "*".to_owned()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnableParams {
    /// Omitted means every request; an explicit empty list means none.
    #[serde(default)]
    pub patterns: Option<Vec<RequestPattern>>,
    #[serde(default)]
    pub handle_auth_requests: bool,
}

/// Interception state for the session that enabled `Fetch`.
pub struct InterceptState<T> {
    patterns: Vec<RequestPattern>,
    pub handle_auth_requests: bool,
    waiting: HashMap<u64, T>,
    next_id: u64,
}

impl<T> Default for InterceptState<T> {
    fn default() -> Self {
        Self { patterns: Vec::new(), handle_auth_requests: false, waiting: HashMap::new(), next_id: 1 }
    }
}

impl<T> InterceptState<T> {
    pub fn enable(&mut self, params: EnableParams) -> CdpResult<()> {
        if matches!(&params.patterns, Some(p) if p.is_empty()) && params.handle_auth_requests {
            return Err(CdpError::invalid_params("Can't specify empty patterns with handleAuth set"));
        }
        self.patterns = params.patterns.unwrap_or_else(|| {
            vec![RequestPattern { url_pattern: star(), resource_type: None, request_stage: RequestStage::Request }]
        });
        self.handle_auth_requests = params.handle_auth_requests;
        Ok(())
    }

    /// Disables interception and returns every paused request so the caller
    /// can resume them unmodified (as Chrome does on `Fetch.disable`).
    pub fn disable(&mut self) -> Vec<T> {
        self.patterns.clear();
        self.handle_auth_requests = false;
        self.waiting.drain().map(|(_, t)| t).collect()
    }

    pub fn is_enabled(&self) -> bool {
        !self.patterns.is_empty() || self.handle_auth_requests
    }

    pub fn matches(&self, stage: RequestStage, url: &str, resource_type: ResourceType) -> bool {
        let reported = resource_type.fetch_reported();
        self.patterns.iter().any(|p| {
            p.request_stage == stage
                && p.resource_type.is_none_or(|rt| rt.fetch_reported() == reported)
                && wildcard_match(&p.url_pattern, url)
        })
    }

    /// Parks a request and returns its `Fetch` request id (`INT-<n>`).
    pub fn pause(&mut self, request: T) -> String {
        let id = self.next_id;
        self.next_id += 1;
        self.waiting.insert(id, request);
        format!("INT-{id}")
    }

    /// Takes the paused request for a continue/fulfill/fail decision.
    pub fn take(&mut self, request_id: &str) -> CdpResult<T> {
        let id = request_id
            .strip_prefix("INT-")
            .and_then(|n| n.parse::<u64>().ok())
            .ok_or_else(|| CdpError::invalid_params("Invalid InterceptionId."))?;
        self.waiting.remove(&id).ok_or_else(|| CdpError::invalid_params("Invalid InterceptionId."))
    }

    pub fn paused(&self) -> usize {
        self.waiting.len()
    }

    /// Drops paused entries the engine already abandoned (e.g. frame teardown).
    pub fn retain(&mut self, keep: impl FnMut(&T) -> bool) {
        let mut keep = keep;
        self.waiting.retain(|_, t| keep(t));
    }
}

/// `RequestPattern.urlPattern`: `*` matches zero or more characters, `?`
/// exactly one, `\` escapes the next character. Linear-time backtracking glob.
pub fn wildcard_match(pattern: &str, value: &str) -> bool {
    let (p, v) = (pattern.as_bytes(), value.as_bytes());
    let (mut pi, mut vi) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while vi < v.len() {
        if pi < p.len() {
            match p[pi] {
                b'*' => {
                    pi += 1;
                    star = Some((pi, vi));
                    continue;
                }
                b'?' => {
                    // '?' consumes one UTF-8 character, not one byte.
                    pi += 1;
                    vi += utf8_len(v[vi]);
                    continue;
                }
                c => {
                    let (literal, advance) = if c == b'\\' && pi + 1 < p.len() { (p[pi + 1], 2) } else { (c, 1) };
                    if literal == v[vi] {
                        pi += advance;
                        vi += 1;
                        continue;
                    }
                }
            }
        }
        match star {
            Some((sp, sv)) => {
                // The star absorbs one more whole character and we retry.
                let next = sv + utf8_len(v[sv]);
                pi = sp;
                vi = next;
                star = Some((sp, next));
            }
            None => return false,
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

fn utf8_len(first: u8) -> usize {
    match first {
        0xF0.. => 4,
        0xE0.. => 3,
        0xC0.. => 2,
        _ => 1,
    }
}

/// Standard base64 decoding (padding optional) for `Fetch.fulfillRequest`
/// bodies and `postData`. Rejects invalid characters.
pub fn base64_decode(input: &str) -> CdpResult<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some(u32::from(c - b'A')),
            b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes = input.trim_end_matches('=').as_bytes();
    if bytes.len() % 4 == 1 {
        return Err(CdpError::invalid_params("Invalid base64 data"));
    }
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= val(c).ok_or_else(|| CdpError::invalid_params("Invalid base64 data"))? << (18 - 6 * i);
        }
        let produced = chunk.len() - 1;
        out.extend_from_slice(&n.to_be_bytes()[1..1 + produced]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_semantics_match_upstream() {
        assert!(wildcard_match("*", ""));
        assert!(wildcard_match("*", "http://h/a"));
        assert!(wildcard_match("*/api/*", "http://h/api/x?y=1"));
        assert!(!wildcard_match("*/api/*", "http://h/apix"));
        assert!(wildcard_match("http://h/exact", "http://h/exact"));
        assert!(!wildcard_match("http://h/exact", "http://h/exact2"));
        assert!(wildcard_match("*.css", "http://h/a.b.css"));
        assert!(wildcard_match("http://h/?", "http://h/é"));
        assert!(wildcard_match("*?x", "éx"), "a star never splits a character");
        assert!(!wildcard_match("http://h/?", "http://h/"));
        assert!(wildcard_match(r"http://h/\*", "http://h/*"));
        assert!(!wildcard_match(r"http://h/\*", "http://h/x"));
        assert!(wildcard_match("a*b*c", "aXXbYYc"));
        assert!(!wildcard_match("a*b*c", "aXXbYY"));
        // Pathological input stays linear-ish rather than exponential.
        let long = "a".repeat(10_000);
        assert!(!wildcard_match("*a*a*a*a*a*b", &long));
    }

    #[test]
    fn enable_defaults_and_resource_type_aliasing() {
        let mut s: InterceptState<u32> = InterceptState::default();
        let params: EnableParams = serde_json::from_str(
            r#"{"handleAuthRequests":true,"patterns":[{"urlPattern":"*","requestStage":"Request"}]}"#,
        )
        .unwrap();
        s.enable(params).unwrap();
        assert!(s.matches(RequestStage::Request, "http://h/", ResourceType::Document));
        assert!(!s.matches(RequestStage::Response, "http://h/", ResourceType::Document));

        let params: EnableParams =
            serde_json::from_str(r#"{"patterns":[{"urlPattern":"*/json/*","resourceType":"Fetch"}]}"#).unwrap();
        s.enable(params).unwrap();
        assert!(s.matches(RequestStage::Request, "http://h/json/a", ResourceType::XHR), "Fetch pattern matches XHR");
        assert!(s.matches(RequestStage::Request, "http://h/json/a", ResourceType::EventSource));
        assert!(!s.matches(RequestStage::Request, "http://h/json/a", ResourceType::Script));

        let params: EnableParams = serde_json::from_str("{}").unwrap();
        s.enable(params).unwrap();
        assert!(s.matches(RequestStage::Request, "x", ResourceType::Image), "omitted patterns = everything");

        let params: EnableParams = serde_json::from_str(r#"{"patterns":[],"handleAuthRequests":true}"#).unwrap();
        assert!(s.enable(params).is_err());
    }

    #[test]
    fn paused_requests_round_trip_and_disable_releases_them() {
        let mut s: InterceptState<&str> = InterceptState::default();
        let a = s.pause("a");
        let b = s.pause("b");
        assert_eq!(a, "INT-1");
        assert_eq!(s.take(&a).unwrap(), "a");
        assert!(s.take(&a).is_err(), "decision applies once");
        assert!(s.take("REQ-1").is_err());
        assert_eq!(s.disable(), vec!["b"]);
        assert!(s.take(&b).is_err());
    }

    #[test]
    fn base64_round_trips() {
        assert_eq!(base64_decode("").unwrap(), b"");
        assert_eq!(base64_decode("Zg==").unwrap(), b"f");
        assert_eq!(base64_decode("Zm8").unwrap(), b"fo");
        assert_eq!(base64_decode("Zm9vYmFy").unwrap(), b"foobar");
        assert_eq!(base64_decode("eyJhIjoxfQ==").unwrap(), br#"{"a":1}"#);
        assert!(base64_decode("Z").is_err());
        assert!(base64_decode("Zm9v!").is_err());
    }
}
