//! Network Error Logging (https://w3c.github.io/network-error-logging/): the `NEL` response header
//! sets a policy for the origin of the response, and while the policy lives every request to that
//! origin may be reported, by the sampling fractions the policy names, with a `network-error` report
//! the Reporting API delivers (`ReportingSink::network_error`; `pyro-webapi`'s `reporting_store`
//! resolves the policy's `report_to` group through the `Report-To` groups of the origin and uploads).
//!
//! What a request's completion reports (one report per HTTP exchange of the network, so each
//! redirect hop and each conditional request answered `304` has its own; a response the HTTP cache
//! served without a request has none, and neither has a CORS preflight or a report upload):
//! `status_code`, `phase` (`application` for a response, `dns` / `connection` for a failure to get
//! one), `type` (`ok` for a status below 400, `http.error` from 400, `dns.name_not_resolved`,
//! `tcp.refused`, `tcp.timed_out`, `tls.failed` ... for a failure), `elapsed_time` (milliseconds
//! from the start of the hop), `method`, `protocol`, `referrer` and `sampling_fraction`. `server_ip`
//! is empty: the client does not record the address it connected to. A policy of a parent domain
//! with `include_subdomains` reports only a failure to resolve the subdomain's name, with a
//! downgraded body (no method, referrer, protocol or status); no other request of a subdomain is
//! reported.
//!
//! A response with a `Report-To` header is handed to the sink as it arrives, before the request is
//! reported, so a response that delivers both the groups and the policy is reported to them. The
//! cost for a client that never saw a policy is one atomic load per response, and a lookup of the
//! two headers only when the sink is set.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use http::{HeaderMap, Method};
use url::Url;

/// What the network layer tells the Reporting side. Called on the network thread: implementations
/// post the work to wherever the reporting state lives.
pub trait ReportingSink: Send + Sync {
    /// A response of `url` (any hop of any request) carried a `Report-To` header: the header's
    /// value (several header lines joined with `, `).
    fn report_to(&self, url: &Url, value: &str);
    /// A request completed and a policy asks for it to be reported.
    fn network_error(&self, report: NelReport);
}

/// A `network-error` report, before the Reporting API wraps it.
#[derive(Clone, Debug, PartialEq)]
pub struct NelReport {
    /// The URL of the response that delivered the policy: the origin whose `Report-To` groups
    /// `group` names, and the origin the upload is made from.
    pub policy_url: Url,
    /// The policy's `report_to`.
    pub group: String,
    /// The report's `url`: the request's, without fragment and credentials.
    pub url: String,
    pub body: NelBody,
}

/// The fields of a NEL report body (W3C NEL "network error report"). A downgraded report
/// (`downgraded`) leaves `method`, `referrer`, `protocol` and `status_code` empty.
#[derive(Clone, Debug, PartialEq)]
pub struct NelBody {
    pub sampling_fraction: f64,
    pub referrer: String,
    pub server_ip: String,
    pub protocol: String,
    pub method: String,
    pub status_code: u16,
    pub elapsed_time: u64,
    pub phase: &'static str,
    pub kind: String,
    pub downgraded: bool,
}

/// How one exchange ended.
pub(crate) enum Outcome {
    /// A response arrived with this status over this HTTP version.
    Response { status: u16, protocol: http::Version },
    /// No response: the failure, as NEL names it, and its phase.
    Failure { phase: &'static str, kind: &'static str },
}

struct Policy {
    /// The URL of the response that set it.
    base: Url,
    report_to: String,
    expires: Instant,
    include_subdomains: bool,
    success_fraction: f64,
    failure_fraction: f64,
}

/// The policies of one network client and where reports go.
#[derive(Default)]
pub(crate) struct Nel {
    /// Set while any policy was ever stored: until then a response costs this load only for the NEL
    /// header (the `Report-To` header goes to the sink whenever there is one).
    any: AtomicBool,
    policies: Mutex<Vec<Policy>>,
    sink: Mutex<Option<Arc<dyn ReportingSink>>>,
    has_sink: AtomicBool,
}

impl Nel {
    pub(crate) fn set_sink(&self, sink: Option<Arc<dyn ReportingSink>>) {
        self.has_sink.store(sink.is_some(), Ordering::Release);
        *self.sink.lock().unwrap_or_else(|e| e.into_inner()) = sink;
    }

    fn sink(&self) -> Option<Arc<dyn ReportingSink>> {
        if !self.has_sink.load(Ordering::Acquire) {
            return None;
        }
        self.sink.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// An exchange for `url` ended. `headers` are the response's (empty without one). `referrer`
    /// and `method` are the hop's, `started` when it began.
    pub(crate) fn record(
        &self,
        url: &Url,
        method: &Method,
        referrer: Option<&Url>,
        headers: &HeaderMap,
        started: Instant,
        outcome: Outcome,
    ) {
        let Some(sink) = self.sink() else { return };
        if let Some(value) = joined(headers, "report-to") {
            sink.report_to(url, &value);
        }
        // The policy needs a secure transport; nothing else is looked at before one was stored.
        if url.scheme() == "https" {
            if let Some(value) = joined(headers, "nel") {
                self.set_policy(url, &value);
            }
        }
        if !self.any.load(Ordering::Relaxed) {
            return;
        }
        let elapsed_time = started.elapsed().as_millis() as u64;
        let (policy, same_origin) = match self.policy_for(url) {
            Some(found) => found,
            None => return,
        };
        let (status_code, protocol, phase, kind, success) = match outcome {
            Outcome::Response { status, protocol } => {
                let success = status < 400;
                (status, protocol_name(protocol), "application", (if success { "ok" } else { "http.error" }).to_owned(), success)
            }
            Outcome::Failure { phase, kind } => (0, "", phase, kind.to_owned(), false),
        };
        let fraction = if success { policy.success_fraction } else { policy.failure_fraction };
        // A parent domain's policy covers a subdomain's failure to resolve its name, downgraded, and nothing else.
        if !same_origin && kind != "dns.name_not_resolved" {
            return;
        }
        if !sampled(fraction) {
            return;
        }
        let url_text = strip(url);
        let body = if same_origin {
            NelBody {
                sampling_fraction: fraction,
                referrer: referrer.map(strip).unwrap_or_default(),
                server_ip: String::new(),
                protocol: protocol.to_owned(),
                method: method.as_str().to_owned(),
                status_code,
                elapsed_time,
                phase,
                kind,
                downgraded: false,
            }
        } else {
            NelBody {
                sampling_fraction: fraction,
                referrer: String::new(),
                server_ip: String::new(),
                protocol: String::new(),
                method: String::new(),
                status_code: 0,
                elapsed_time,
                phase: "dns",
                kind,
                downgraded: true,
            }
        };
        sink.network_error(NelReport { policy_url: policy.base, group: policy.report_to, url: url_text, body });
    }

    /// The `NEL` header `value` of a response from `url`: a policy replaces the origin's earlier
    /// one, `max_age: 0` removes it, an invalid value changes nothing.
    fn set_policy(&self, url: &Url, value: &str) {
        let Some(members) = parse_object(value) else { return };
        let get = |name: &str| members.iter().find(|(k, _)| k == name).map(|(_, v)| v);
        let Some(Json::Num(max_age)) = get("max_age") else { return };
        if !max_age.is_finite() || *max_age < 0.0 {
            return;
        }
        let origin = url.origin();
        let mut policies = self.policies.lock().unwrap_or_else(|e| e.into_inner());
        policies.retain(|p| p.base.origin() != origin);
        if *max_age == 0.0 {
            return;
        }
        let Some(Json::Str(report_to)) = get("report_to") else { return };
        let fraction = |name: &str, default: f64| match get(name) {
            Some(Json::Num(n)) if (0.0..=1.0).contains(n) => *n,
            _ => default,
        };
        policies.push(Policy {
            base: url.clone(),
            report_to: report_to.clone(),
            // (Two years at most: keeps the sum an `Instant` can hold.)
            expires: Instant::now() + Duration::from_secs_f64(max_age.min(63_072_000.0)),
            include_subdomains: matches!(get("include_subdomains"), Some(Json::Bool(true))),
            success_fraction: fraction("success_fraction", 0.0),
            failure_fraction: fraction("failure_fraction", 1.0),
        });
        self.any.store(true, Ordering::Relaxed);
    }

    /// The live policy that covers a request to `url`, and whether it is the origin's own (else a
    /// parent domain's `include_subdomains` policy).
    fn policy_for(&self, url: &Url) -> Option<(PolicyView, bool)> {
        let now = Instant::now();
        let mut policies = self.policies.lock().unwrap_or_else(|e| e.into_inner());
        policies.retain(|p| p.expires > now);
        let origin = url.origin();
        if let Some(p) = policies.iter().find(|p| p.base.origin() == origin) {
            return Some((PolicyView::from(p), true));
        }
        let host = match url.host() {
            Some(url::Host::Domain(host)) => host,
            _ => return None,
        };
        policies
            .iter()
            .find(|p| {
                p.include_subdomains
                    && p.base.scheme() == url.scheme()
                    && p.base.port_or_known_default() == url.port_or_known_default()
                    && matches!(p.base.host(), Some(url::Host::Domain(parent)) if host.len() > parent.len()
                        && host.ends_with(parent) && host.as_bytes()[host.len() - parent.len() - 1] == b'.')
            })
            .map(|p| (PolicyView::from(p), false))
    }
}

/// A policy as `record` needs it.
struct PolicyView {
    base: Url,
    report_to: String,
    success_fraction: f64,
    failure_fraction: f64,
}

impl From<&Policy> for PolicyView {
    fn from(p: &Policy) -> Self {
        PolicyView {
            base: p.base.clone(),
            report_to: p.report_to.clone(),
            success_fraction: p.success_fraction,
            failure_fraction: p.failure_fraction,
        }
    }
}

fn joined(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut values = headers.get_all(name).iter().filter_map(|v| std::str::from_utf8(v.as_bytes()).ok());
    let first = values.next()?.to_owned();
    Some(values.fold(first, |acc, v| format!("{acc}, {v}")))
}

fn protocol_name(version: http::Version) -> &'static str {
    match version {
        http::Version::HTTP_09 => "http/0.9",
        http::Version::HTTP_10 => "http/1.0",
        http::Version::HTTP_11 => "http/1.1",
        http::Version::HTTP_2 => "h2",
        http::Version::HTTP_3 => "h3",
        _ => "",
    }
}

/// The URL as a report states it: no fragment, no credentials.
fn strip(url: &Url) -> String {
    let mut url = url.clone();
    url.set_fragment(None);
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.to_string()
}

/// Whether a report with `fraction` of probability is made now.
fn sampled(fraction: f64) -> bool {
    if fraction >= 1.0 {
        return true;
    }
    if fraction <= 0.0 {
        return false;
    }
    let mut bytes = [0u8; 8];
    if getrandom::fill(&mut bytes).is_err() {
        return false;
    }
    (u64::from_le_bytes(bytes) >> 11) as f64 / (1u64 << 53) as f64 <= fraction
}

/// A JSON value as far as a NEL policy needs: nested values are skipped.
enum Json {
    Str(String),
    Num(f64),
    Bool(bool),
    Other,
}

/// The members of the JSON object `text` (it must be the whole text).
fn parse_object(text: &str) -> Option<Vec<(String, Json)>> {
    let mut p = Parser { b: text.as_bytes(), i: 0 };
    p.ws();
    p.eat(b'{')?;
    let mut members = Vec::new();
    p.ws();
    if p.peek() == Some(b'}') {
        p.i += 1;
    } else {
        loop {
            let key = p.string()?;
            p.eat(b':')?;
            let value = p.value()?;
            members.push((key, value));
            p.ws();
            match p.next()? {
                b',' => {}
                b'}' => break,
                _ => return None,
            }
        }
    }
    p.ws();
    (p.i == p.b.len()).then_some(members)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while matches!(self.b.get(self.i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.i += 1;
        Some(c)
    }

    fn eat(&mut self, c: u8) -> Option<()> {
        self.ws();
        (self.peek() == Some(c)).then(|| self.i += 1)
    }

    fn literal(&mut self, word: &str) -> Option<()> {
        self.b[self.i..].starts_with(word.as_bytes()).then(|| self.i += word.len())
    }

    fn string(&mut self) -> Option<String> {
        self.eat(b'"')?;
        let mut out = Vec::new();
        loop {
            match self.next()? {
                b'"' => break,
                b'\\' => match self.next()? {
                    e @ (b'"' | b'\\' | b'/') => out.push(e),
                    b'n' => out.push(b'\n'),
                    b't' => out.push(b'\t'),
                    b'r' => out.push(b'\r'),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'u' => {
                        let hex = std::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
                        self.i += 4;
                        let c = char::from_u32(u32::from_str_radix(hex, 16).ok()?).unwrap_or('\u{fffd}');
                        out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
                    }
                    _ => return None,
                },
                c => out.push(c),
            }
        }
        String::from_utf8(out).ok()
    }

    fn value(&mut self) -> Option<Json> {
        self.ws();
        match self.peek()? {
            b'"' => Some(Json::Str(self.string()?)),
            b'{' => {
                self.i += 1;
                self.ws();
                if self.peek() == Some(b'}') {
                    self.i += 1;
                    return Some(Json::Other);
                }
                loop {
                    self.string()?;
                    self.eat(b':')?;
                    self.value()?;
                    self.ws();
                    match self.next()? {
                        b',' => {}
                        b'}' => return Some(Json::Other),
                        _ => return None,
                    }
                }
            }
            b'[' => {
                self.i += 1;
                self.ws();
                if self.peek() == Some(b']') {
                    self.i += 1;
                    return Some(Json::Other);
                }
                loop {
                    self.value()?;
                    self.ws();
                    match self.next()? {
                        b',' => {}
                        b']' => return Some(Json::Other),
                        _ => return None,
                    }
                }
            }
            b't' => self.literal("true").map(|_| Json::Bool(true)),
            b'f' => self.literal("false").map(|_| Json::Bool(false)),
            b'n' => self.literal("null").map(|_| Json::Other),
            _ => {
                let start = self.i;
                while matches!(self.peek(), Some(b'+' | b'-' | b'.' | b'e' | b'E' | b'0'..=b'9')) {
                    self.i += 1;
                }
                std::str::from_utf8(&self.b[start..self.i]).ok()?.parse::<f64>().ok().map(Json::Num)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Collect {
        groups: Mutex<Vec<(String, String)>>,
        reports: Mutex<Vec<NelReport>>,
    }

    impl ReportingSink for Collect {
        fn report_to(&self, url: &Url, value: &str) {
            self.groups.lock().unwrap().push((url.to_string(), value.to_owned()));
        }
        fn network_error(&self, report: NelReport) {
            self.reports.lock().unwrap().push(report);
        }
    }

    fn nel_with(sink: &Arc<Collect>) -> Nel {
        let nel = Nel::default();
        nel.set_sink(Some(sink.clone()));
        nel
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            map.append(*k, v.parse().unwrap());
        }
        map
    }

    fn ok(nel: &Nel, url: &str, status: u16, h: &HeaderMap) {
        nel.record(
            &Url::parse(url).unwrap(),
            &Method::GET,
            None,
            h,
            Instant::now(),
            Outcome::Response { status, protocol: http::Version::HTTP_11 },
        );
    }

    #[test]
    fn policy_reports_the_request_that_delivered_it_and_later_ones() {
        let sink = Arc::new(Collect::default());
        let nel = nel_with(&sink);
        let policy = headers(&[("nel", r#"{"report_to":"g","max_age":60,"success_fraction":1.0}"#)]);
        ok(&nel, "https://a.test/pass.png", 200, &policy);
        ok(&nel, "https://a.test/missing.png", 404, &HeaderMap::new());
        let reports = sink.reports.lock().unwrap();
        assert_eq!(reports.len(), 2);
        assert_eq!((reports[0].body.status_code, reports[0].body.kind.as_str(), reports[0].body.phase), (200, "ok", "application"));
        assert_eq!((reports[1].body.status_code, reports[1].body.kind.as_str()), (404, "http.error"));
        assert_eq!(reports[1].group, "g");
        assert_eq!(reports[1].url, "https://a.test/missing.png");
    }

    #[test]
    fn default_fractions_report_failures_only_and_zero_age_clears() {
        let sink = Arc::new(Collect::default());
        let nel = nel_with(&sink);
        ok(&nel, "https://a.test/x", 200, &headers(&[("nel", r#"{"report_to":"g","max_age":60}"#)]));
        ok(&nel, "https://a.test/y", 200, &HeaderMap::new());
        assert!(sink.reports.lock().unwrap().is_empty());
        ok(&nel, "https://a.test/z", 500, &HeaderMap::new());
        assert_eq!(sink.reports.lock().unwrap().len(), 1);
        ok(&nel, "https://a.test/clear", 200, &headers(&[("nel", r#"{"max_age":0}"#)]));
        ok(&nel, "https://a.test/w", 500, &HeaderMap::new());
        assert_eq!(sink.reports.lock().unwrap().len(), 1);
    }

    #[test]
    fn subdomains_are_covered_for_name_failures_only() {
        let sink = Arc::new(Collect::default());
        let nel = nel_with(&sink);
        let policy = headers(&[("nel", r#"{"report_to":"g","max_age":60,"include_subdomains":true,"success_fraction":1.0}"#)]);
        ok(&nel, "https://a.test/p", 200, &policy);
        sink.reports.lock().unwrap().clear();
        ok(&nel, "https://www.a.test/q", 200, &HeaderMap::new());
        ok(&nel, "https://www.a.test/q", 404, &HeaderMap::new());
        assert!(sink.reports.lock().unwrap().is_empty());
        nel.record(
            &Url::parse("https://www.a.test/q").unwrap(),
            &Method::GET,
            None,
            &HeaderMap::new(),
            Instant::now(),
            Outcome::Failure { phase: "dns", kind: "dns.name_not_resolved" },
        );
        let reports = sink.reports.lock().unwrap();
        assert_eq!(reports.len(), 1);
        assert!(reports[0].body.downgraded);
        assert_eq!(reports[0].policy_url.as_str(), "https://a.test/p");
    }

    #[test]
    fn report_to_headers_reach_the_sink_and_insecure_policies_are_ignored() {
        let sink = Arc::new(Collect::default());
        let nel = nel_with(&sink);
        let h = headers(&[("report-to", r#"{"group":"g","max_age":1}"#), ("report-to", r#"{"group":"h","max_age":1}"#)]);
        ok(&nel, "http://a.test/p", 200, &h);
        assert_eq!(sink.groups.lock().unwrap()[0].1, r#"{"group":"g","max_age":1}, {"group":"h","max_age":1}"#);
        ok(&nel, "http://a.test/p", 200, &headers(&[("nel", r#"{"report_to":"g","max_age":60,"success_fraction":1.0}"#)]));
        ok(&nel, "http://a.test/q", 200, &HeaderMap::new());
        assert!(sink.reports.lock().unwrap().is_empty());
    }

    #[test]
    fn json_objects() {
        let members = parse_object(r#" {"a": 1.5, "b": "xA", "c": [1, {"d": null}], "e": true} "#).unwrap();
        assert_eq!(members.len(), 4);
        assert!(matches!(&members[1].1, Json::Str(s) if s == "xA"));
        assert!(parse_object("{").is_none());
        assert!(parse_object("[]").is_none());
        assert!(parse_object(r#"{"a":1} x"#).is_none());
    }
}
