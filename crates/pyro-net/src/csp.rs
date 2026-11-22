//! Content Security Policy Level 3 (https://w3c.github.io/webappsec-csp/): a document's (or
//! worker's) policy list, source-expression matching, and the checks the fetch paths run.
//!
//! No DOM or JS here: a caller builds a [`PolicyList`] from the delivered serializations
//! (`Content-Security-Policy` / `-Report-Only` headers, `<meta http-equiv>`), asks it whether a
//! request, an inline script or style, or a string compilation may go ahead, and gets back the
//! [`Violation`]s to report. Enforced policies block; report-only ones only report.
//!
//! Redirects: a request carries a [`RequestCsp`] (`Request::csp`) and the client runs the check
//! again on every redirect target before following it (CSP3 §4.1.2, the URL of each hop with the
//! redirect flag set, which turns path matching off). Its violations wait in the guard until the
//! initiator takes them ([`RequestCsp::take_violations`]).
//!
//! Where the spec and Chrome differ, this follows Chrome: `*` also matches `ws:`/`wss:`, a
//! host-source without a scheme also matches the WebSocket scheme of the page's own, a
//! violation of a redirected request reports the URL before its redirects, and IP-address hosts
//! match literally.

use std::sync::{Arc, Mutex};

use url::Url;

/// Whether a policy blocks what violates it (`Content-Security-Policy`) or only reports it
/// (`Content-Security-Policy-Report-Only`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Disposition {
    Enforce,
    Report,
}

impl Disposition {
    /// `SecurityPolicyViolationEvent.disposition`.
    pub fn as_str(self) -> &'static str {
        match self {
            Disposition::Enforce => "enforce",
            Disposition::Report => "report",
        }
    }
}

/// How a policy was delivered. A `<meta>` policy ignores `report-uri`, `frame-ancestors` and
/// `sandbox` (CSP3 §3.3).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum PolicySource {
    Header,
    Meta,
}

/// One directive: its lowercased name and its value tokens, as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Directive {
    pub name: String,
    pub value: Vec<String>,
}

/// One parsed policy (CSP3 §2.2.1 "parse a serialized CSP").
#[derive(Clone, Debug)]
pub struct Policy {
    pub directives: Vec<Directive>,
    pub disposition: Disposition,
    pub source: PolicySource,
    /// The serialization as delivered (stripped of surrounding whitespace): the violation's
    /// `originalPolicy`.
    pub text: String,
}

fn is_ascii_ws(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0c' | '\r' | ' ')
}

impl Policy {
    /// Parses one serialized policy (no commas: a header value is a list, see
    /// [`PolicyList::add_header`]). The first directive of a name wins; names are lowercased.
    pub fn parse(text: &str, source: PolicySource, disposition: Disposition) -> Policy {
        let mut directives: Vec<Directive> = Vec::new();
        for token in text.split(';') {
            let token = token.trim_matches(is_ascii_ws);
            if token.is_empty() {
                continue;
            }
            let (name, rest) = match token.find(is_ascii_ws) {
                Some(i) => (&token[..i], &token[i..]),
                None => (token, ""),
            };
            let name = name.to_ascii_lowercase();
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                continue;
            }
            if directives.iter().any(|d| d.name == name) {
                continue;
            }
            if source == PolicySource::Meta && matches!(name.as_str(), "report-uri" | "frame-ancestors" | "sandbox") {
                continue;
            }
            let value = rest.split(is_ascii_ws).filter(|v| !v.is_empty()).map(str::to_owned).collect();
            directives.push(Directive { name, value });
        }
        Policy { directives, disposition, source, text: text.trim_matches(is_ascii_ws).to_owned() }
    }

    /// The directive named `name` (lowercase).
    pub fn directive(&self, name: &str) -> Option<&Directive> {
        self.directives.iter().find(|d| d.name == name)
    }

    /// The first directive of `name`'s fallback list this policy has (CSP3 §6.8.4).
    pub fn directive_for(&self, effective: &str) -> Option<&Directive> {
        fallback_list(effective).iter().find_map(|name| self.directive(name))
    }

    /// The `report-uri` values.
    pub fn report_uris(&self) -> Vec<String> {
        self.directive("report-uri").map(|d| d.value.clone()).unwrap_or_default()
    }

    /// The `report-to` group.
    pub fn report_to(&self) -> Option<String> {
        self.directive("report-to").and_then(|d| d.value.first().cloned())
    }
}

/// The origin `'self'` matches: a tuple origin's scheme, host and port (the port is the
/// scheme's default when the URL names none).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelfOrigin {
    pub scheme: String,
    pub host: String,
    pub port: Option<u16>,
}

impl SelfOrigin {
    /// `url`'s origin, `None` when it is opaque (`data:`, `about:blank` of an opaque document).
    pub fn of(url: &Url) -> Option<SelfOrigin> {
        match url.origin() {
            url::Origin::Tuple(scheme, host, port) => Some(SelfOrigin { scheme, host: host.to_string(), port: Some(port) }),
            url::Origin::Opaque(_) => None,
        }
    }

    /// A serialized origin (`https://a.test:8443`), `None` for `"null"` or what does not parse.
    pub fn parse(origin: &str) -> Option<SelfOrigin> {
        Url::parse(origin).ok().as_ref().and_then(SelfOrigin::of)
    }
}

/// A context's policies (its policy container's CSP list) and the origin `'self'` means in it.
#[derive(Clone, Debug, Default)]
pub struct PolicyList {
    pub policies: Vec<Policy>,
    /// `None`: an opaque origin, which `'self'` never matches.
    pub self_origin: Option<SelfOrigin>,
}

/// A request's destination as CSP sees it (Fetch "destination", plus the initiator cases that
/// pick a directive).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum RequestDest {
    /// The empty destination: `fetch()`, XHR, `EventSource`, `sendBeacon`.
    Empty,
    /// A WebSocket connection (connect-src, as Chrome).
    WebSocket,
    Json,
    Script,
    Xslt,
    Style,
    Image,
    Font,
    /// `audio`, `video`, `track`.
    Media,
    Object,
    Embed,
    /// `frame` and `iframe` navigations.
    Frame,
    Worker,
    SharedWorker,
    ServiceWorker,
    AudioWorklet,
    PaintWorklet,
    Manifest,
    /// `<link rel=prefetch>` (Chrome: default-src).
    Prefetch,
    /// A top-level navigation, a report or anything no directive governs.
    Other,
}

impl RequestDest {
    /// Fetch "script-like" destinations: the nonce, hash and `'strict-dynamic'` rules apply.
    pub fn is_script_like(self) -> bool {
        matches!(
            self,
            RequestDest::Script
                | RequestDest::Xslt
                | RequestDest::AudioWorklet
                | RequestDest::PaintWorklet
                | RequestDest::Worker
                | RequestDest::SharedWorker
                | RequestDest::ServiceWorker
        )
    }
}

/// CSP3 §6.8.1 "Get the effective directive for request".
pub fn effective_directive(dest: RequestDest) -> Option<&'static str> {
    Some(match dest {
        RequestDest::Empty | RequestDest::WebSocket | RequestDest::Json => "connect-src",
        RequestDest::Manifest => "manifest-src",
        RequestDest::Object | RequestDest::Embed => "object-src",
        RequestDest::Frame => "frame-src",
        RequestDest::Media => "media-src",
        RequestDest::Font => "font-src",
        RequestDest::Image => "img-src",
        RequestDest::Style => "style-src-elem",
        RequestDest::Script | RequestDest::Xslt | RequestDest::AudioWorklet | RequestDest::PaintWorklet => {
            "script-src-elem"
        }
        RequestDest::Worker | RequestDest::SharedWorker | RequestDest::ServiceWorker => "worker-src",
        RequestDest::Prefetch => "default-src",
        RequestDest::Other => return None,
    })
}

/// CSP3 §6.8.3 "Get the fallback list": the directives that stand in for `effective`, in order.
pub fn fallback_list(effective: &str) -> &'static [&'static str] {
    match effective {
        "script-src-elem" => &["script-src-elem", "script-src", "default-src"],
        "script-src-attr" => &["script-src-attr", "script-src", "default-src"],
        "style-src-elem" => &["style-src-elem", "style-src", "default-src"],
        "style-src-attr" => &["style-src-attr", "style-src", "default-src"],
        "worker-src" => &["worker-src", "child-src", "script-src", "default-src"],
        "connect-src" => &["connect-src", "default-src"],
        "manifest-src" => &["manifest-src", "default-src"],
        "object-src" => &["object-src", "default-src"],
        "frame-src" => &["frame-src", "child-src", "default-src"],
        "fenced-frame-src" => &["fenced-frame-src", "frame-src", "child-src", "default-src"],
        "media-src" => &["media-src", "default-src"],
        "font-src" => &["font-src", "default-src"],
        "img-src" => &["img-src", "default-src"],
        // String compilation (eval, Function, WebAssembly).
        "script-src" => &["script-src", "default-src"],
        "style-src" => &["style-src", "default-src"],
        "child-src" => &["child-src", "default-src"],
        "default-src" => &["default-src"],
        "form-action" => &["form-action"],
        "base-uri" => &["base-uri"],
        "frame-ancestors" => &["frame-ancestors"],
        _ => &[],
    }
}

/// What a request check looks at (Fetch request metadata).
#[derive(Clone, Debug)]
pub struct CspRequest {
    /// The URL being fetched (the current URL: a redirect's target on a redirect).
    pub url: Url,
    /// The URL the request started with: what a violation of a redirect hop reports.
    pub original_url: Url,
    pub dest: RequestDest,
    /// The initiating element's cryptographic nonce (empty: none).
    pub nonce: String,
    /// A module script's import whose root script's nonce the caller does not know (HTML passes
    /// it down the graph): any nonce-source of the directive counts as matched, as it would for
    /// an import of a nonced root.
    pub inherits_nonce: bool,
    /// The request's integrity metadata (empty: none).
    pub integrity: String,
    /// The initiating element was inserted by the parser (`'strict-dynamic'` does not trust it).
    pub parser_inserted: bool,
    /// The request went through a redirect: path parts of host-sources are not matched.
    pub redirected: bool,
}

impl CspRequest {
    /// A request of `url` for `dest` with no nonce, integrity or parser metadata.
    pub fn new(url: Url, dest: RequestDest) -> CspRequest {
        CspRequest {
            original_url: url.clone(),
            url,
            dest,
            nonce: String::new(),
            inherits_nonce: false,
            integrity: String::new(),
            parser_inserted: false,
            redirected: false,
        }
    }
}

/// A violation record (CSP3 §2.4), for `securitypolicyviolation` and for reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    /// The effective directive (`script-src-elem`, `worker-src`, `script-src` for eval...).
    pub effective_directive: String,
    /// The directive of the policy that decided (Chrome reports the effective directive as the
    /// violated one; this is the one the policy actually had, e.g. `default-src`).
    pub violated_directive: String,
    /// The policy's serialization.
    pub original_policy: String,
    pub disposition: Disposition,
    pub policy_source: PolicySource,
    /// What was blocked: a URL (before redirects, as Chrome), or `inline`, `eval`,
    /// `wasm-eval`.
    pub blocked_url: String,
    /// The URL that was checked (a redirect's target): reporting strips it as it must.
    pub checked_url: String,
    /// The first 40 characters of the inline source or compiled string when the directive has
    /// `'report-sample'`, else empty.
    pub sample: String,
    /// Where the violation happened, filled by the caller when it knows (script URL, 1-based
    /// line and column).
    pub source_file: String,
    pub line: u32,
    pub column: u32,
    /// The HTTP status of the document's (or worker's) response, filled by the caller.
    pub status_code: u16,
    /// The policy's `report-uri` endpoints and `report-to` group, for the reporting step.
    pub report_uris: Vec<String>,
    pub report_to: Option<String>,
}

/// The result of a check: whether an enforced policy blocks, and every violation (enforced and
/// report-only) to report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckOutcome {
    pub blocked: bool,
    pub violations: Vec<Violation>,
}

impl CheckOutcome {
    pub fn allowed(&self) -> bool {
        !self.blocked
    }
}

/// What an inline check is about (CSP3 §6.7.3.3 "type").
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum InlineKind {
    /// An inline `<script>`.
    Script,
    /// An event handler content attribute.
    ScriptAttribute,
    /// A `javascript:` URL navigation.
    Navigation,
    /// A `<style>` element (or `<link>`-less inline sheet).
    Style,
    /// A `style` attribute.
    StyleAttribute,
}

impl InlineKind {
    /// CSP3 §6.8.2 "Get the effective directive for inline checks".
    pub fn effective_directive(self) -> &'static str {
        match self {
            InlineKind::Script | InlineKind::Navigation => "script-src-elem",
            InlineKind::ScriptAttribute => "script-src-attr",
            InlineKind::Style => "style-src-elem",
            InlineKind::StyleAttribute => "style-src-attr",
        }
    }

    fn is_script(self) -> bool {
        matches!(self, InlineKind::Script | InlineKind::ScriptAttribute | InlineKind::Navigation)
    }
}

impl PolicyList {
    /// An empty list for a context of `self_url`'s origin.
    pub fn new(self_url: Option<&Url>) -> PolicyList {
        PolicyList { policies: Vec::new(), self_origin: self_url.and_then(SelfOrigin::of) }
    }

    /// An empty list whose `'self'` is `origin` (serialized; `"null"` or `None`: opaque).
    pub fn for_origin(origin: Option<&str>) -> PolicyList {
        PolicyList { policies: Vec::new(), self_origin: origin.and_then(SelfOrigin::parse) }
    }

    pub fn is_empty(&self) -> bool {
        self.policies.is_empty()
    }

    /// Adds the policies of a header value: a comma-separated list of serialized policies
    /// (several header fields joined by `,` are one value). Empty policies are dropped.
    pub fn add_header(&mut self, value: &str, disposition: Disposition) {
        for part in value.split(',') {
            let policy = Policy::parse(part, PolicySource::Header, disposition);
            if !policy.directives.is_empty() {
                self.policies.push(policy);
            }
        }
    }

    /// Adds a `<meta http-equiv="Content-Security-Policy">` policy (always enforced).
    pub fn add_meta(&mut self, content: &str) {
        let policy = Policy::parse(content, PolicySource::Meta, Disposition::Enforce);
        if !policy.directives.is_empty() {
            self.policies.push(policy);
        }
    }

    /// Adds one policy of a known source and disposition (a list as a document stored it).
    pub fn add(&mut self, text: &str, source: PolicySource, disposition: Disposition) {
        match source {
            PolicySource::Header => self.add_header(text, disposition),
            PolicySource::Meta if disposition == Disposition::Enforce => self.add_meta(text),
            // A report-only policy cannot be delivered by `<meta>`.
            PolicySource::Meta => {}
        }
    }

    /// CSP3 §4.1.2 "Should request be blocked by Content Security Policy?" (the pre-request
    /// check of each policy's directive for the request's destination).
    pub fn pre_request(&self, request: &CspRequest) -> CheckOutcome {
        self.check_url(request, &request.url)
    }

    /// CSP3 §4.1.3 "Should response to request be blocked?": the post-request check, against
    /// the response's URL (after redirects; path matching is then off).
    pub fn post_request(&self, request: &CspRequest, response_url: &Url) -> CheckOutcome {
        if response_url == &request.url {
            return self.check_url(request, response_url);
        }
        let mut hop = request.clone();
        hop.redirected = true;
        self.check_url(&hop, response_url)
    }

    fn check_url(&self, request: &CspRequest, url: &Url) -> CheckOutcome {
        let mut outcome = CheckOutcome::default();
        if self.policies.is_empty() {
            return outcome;
        }
        let Some(effective) = effective_directive(request.dest) else { return outcome };
        for policy in &self.policies {
            let Some(directive) = policy.directive_for(effective) else { continue };
            if self.request_allowed_by(directive, request, url) {
                continue;
            }
            // Chrome reports a redirected request by the URL it started with.
            let blocked = if request.redirected { &request.original_url } else { url };
            outcome.violations.push(self.violation(policy, directive, effective, blocked.as_str(), url.as_str(), ""));
            if policy.disposition == Disposition::Enforce {
                outcome.blocked = true;
            }
        }
        outcome
    }

    /// One directive's pre-request check (CSP3 §6.1.x; the script-like rules of §6.7.1.1 for
    /// script and worker destinations, the nonce rule for styles too).
    fn request_allowed_by(&self, directive: &Directive, request: &CspRequest, url: &Url) -> bool {
        let list = &directive.value;
        let nonce_applies = request.dest.is_script_like() || request.dest == RequestDest::Style;
        if nonce_applies && !request.nonce.is_empty() && nonce_matches(list, &request.nonce) {
            return true;
        }
        if nonce_applies && request.inherits_nonce && list.iter().any(|e| is_nonce_source(e)) {
            return true;
        }
        if request.dest.is_script_like() {
            if integrity_matches(list, &request.integrity) {
                return true;
            }
            if has_keyword(list, "'strict-dynamic'") {
                return !request.parser_inserted;
            }
        }
        url_matches_source_list(list, url, self.self_origin.as_ref(), request.redirected)
    }

    /// CSP3 §4.2.3 "Should element's inline type behavior be blocked?": an inline script or
    /// style (`nonce`: the element's, empty for attributes and navigations; `source`: the text
    /// the hashes are of).
    pub fn inline_check(&self, kind: InlineKind, nonce: &str, source: &str) -> CheckOutcome {
        let mut outcome = CheckOutcome::default();
        let effective = kind.effective_directive();
        for policy in &self.policies {
            let Some(directive) = policy.directive_for(effective) else { continue };
            if element_matches_source_list(&directive.value, kind, nonce, source) {
                continue;
            }
            let sample = sample_of(directive, source);
            outcome.violations.push(self.violation(policy, directive, effective, "inline", "", &sample));
            if policy.disposition == Disposition::Enforce {
                outcome.blocked = true;
            }
        }
        outcome
    }

    /// CSP3 §4.4 "EnsureCSPDoesNotBlockStringCompilation": `eval`, `Function`, string timers.
    /// `source` is the compiled string (the sample).
    pub fn eval_check(&self, source: &str) -> CheckOutcome {
        self.compile_check(source, "eval", &["'unsafe-eval'"])
    }

    /// CSP3 §4.5 "EnsureCSPDoesNotBlockWasmByteCompilation".
    pub fn wasm_eval_check(&self) -> CheckOutcome {
        self.compile_check("", "wasm-eval", &["'unsafe-eval'", "'wasm-unsafe-eval'"])
    }

    fn compile_check(&self, source: &str, blocked: &str, allow: &[&str]) -> CheckOutcome {
        let mut outcome = CheckOutcome::default();
        for policy in &self.policies {
            let Some(directive) = policy.directive_for("script-src") else { continue };
            if allow.iter().any(|k| has_keyword(&directive.value, k)) {
                continue;
            }
            let sample = sample_of(directive, source);
            outcome.violations.push(self.violation(policy, directive, "script-src", blocked, "", &sample));
            if policy.disposition == Disposition::Enforce {
                outcome.blocked = true;
            }
        }
        outcome
    }

    /// `form-action` (CSP3 §6.3.1, no fallback): a form submission to `url`.
    pub fn form_action_check(&self, url: &Url, redirected: bool) -> CheckOutcome {
        self.navigation_directive_check("form-action", url, redirected)
    }

    /// `base-uri` (CSP3 §6.3.2): a `<base href>` of `url`.
    pub fn base_uri_check(&self, url: &Url) -> CheckOutcome {
        self.navigation_directive_check("base-uri", url, false)
    }

    fn navigation_directive_check(&self, name: &str, url: &Url, redirected: bool) -> CheckOutcome {
        let mut outcome = CheckOutcome::default();
        for policy in &self.policies {
            let Some(directive) = policy.directive(name) else { continue };
            if url_matches_source_list(&directive.value, url, self.self_origin.as_ref(), redirected) {
                continue;
            }
            outcome.violations.push(self.violation(policy, directive, name, url.as_str(), url.as_str(), ""));
            if policy.disposition == Disposition::Enforce {
                outcome.blocked = true;
            }
        }
        outcome
    }

    fn violation(
        &self,
        policy: &Policy,
        directive: &Directive,
        effective: &str,
        blocked_url: &str,
        checked_url: &str,
        sample: &str,
    ) -> Violation {
        Violation {
            effective_directive: effective.to_owned(),
            violated_directive: directive.name.clone(),
            original_policy: policy.text.clone(),
            disposition: policy.disposition,
            policy_source: policy.source,
            blocked_url: blocked_url.to_owned(),
            checked_url: checked_url.to_owned(),
            sample: sample.to_owned(),
            source_file: String::new(),
            line: 0,
            column: 0,
            status_code: 0,
            report_uris: policy.report_uris(),
            report_to: policy.report_to(),
        }
    }
}

/// The 40-character sample of `source` a directive with `'report-sample'` reports.
fn sample_of(directive: &Directive, source: &str) -> String {
    if has_keyword(&directive.value, "'report-sample'") {
        source.chars().take(40).collect()
    } else {
        String::new()
    }
}

fn has_keyword(list: &[String], keyword: &str) -> bool {
    list.iter().any(|e| e.eq_ignore_ascii_case(keyword))
}

/// CSP3 §6.7.2.5 "Does url match source list in origin with redirect count?".
pub fn url_matches_source_list(list: &[String], url: &Url, self_origin: Option<&SelfOrigin>, redirected: bool) -> bool {
    if list.is_empty() || (list.len() == 1 && list[0].eq_ignore_ascii_case("'none'")) {
        return false;
    }
    list.iter().any(|expression| url_matches_expression(expression, url, self_origin, redirected))
}

fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        "ftp" => Some(21),
        _ => None,
    }
}

fn is_scheme(s: &str) -> bool {
    let mut bytes = s.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_alphabetic())
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
}

/// CSP3 §6.7.2.8 "scheme-part matching" (`expression` lowercase), with Chrome's
/// http(s)-to-ws(s) mapping.
fn scheme_part_matches(expression: &str, scheme: &str) -> bool {
    expression == scheme
        || match expression {
            "http" => matches!(scheme, "https" | "ws" | "wss"),
            "https" => scheme == "wss",
            "ws" => matches!(scheme, "wss" | "http" | "https"),
            "wss" => scheme == "https",
            _ => false,
        }
}

/// CSP3 §6.7.2.9 "host-part matching" (both lowercase).
fn host_part_matches(pattern: &str, host: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(rest) = pattern.strip_prefix("*.") {
        return host.len() > rest.len() + 1 && host.ends_with(rest) && host.as_bytes()[host.len() - rest.len() - 1] == b'.';
    }
    pattern == host
}

/// CSP3 §6.7.2.10 "port-part matching", with Chrome's upgrade of port 80 to 443.
fn port_part_matches(port: Option<&str>, url: &Url) -> bool {
    match port {
        None => url.port().is_none(),
        Some("*") => true,
        Some(p) => {
            let Ok(n) = p.parse::<u16>() else { return false };
            if url.port() == Some(n) {
                return true;
            }
            let effective = url.port().or_else(|| default_port(url.scheme()));
            if url.port().is_none() && effective == Some(n) {
                return true;
            }
            n == 80 && matches!(url.scheme(), "https" | "wss") && effective == Some(443)
        }
    }
}

/// CSP3 §6.7.2.11 "path-part matching": percent-decoded, exact unless the expression ends in `/`.
fn path_part_matches(path: &str, url: &Url) -> bool {
    if path.is_empty() {
        return true;
    }
    let url_path = url.path();
    if path == "/" && url_path.is_empty() {
        return true;
    }
    let decode = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned();
    let (pattern, target) = (decode(path), decode(url_path));
    if pattern.ends_with('/') {
        target.starts_with(&pattern)
    } else {
        target == pattern
    }
}

/// CSP3 §6.7.2.6 "Does url match expression in origin with redirect count?".
pub fn url_matches_expression(expression: &str, url: &Url, self_origin: Option<&SelfOrigin>, redirected: bool) -> bool {
    let scheme = url.scheme();
    if expression == "*" {
        return matches!(scheme, "http" | "https" | "ws" | "wss") || self_origin.is_some_and(|o| o.scheme == scheme);
    }
    if expression.starts_with('\'') {
        if !expression.eq_ignore_ascii_case("'self'") {
            // 'none', 'unsafe-inline', nonces, hashes...: no URL matches a keyword.
            return false;
        }
        let Some(origin) = self_origin else { return false };
        let Some(host) = url.host_str() else { return false };
        if !origin.host.eq_ignore_ascii_case(host) {
            return false;
        }
        let url_port = url.port_or_known_default();
        if origin.scheme == scheme && origin.port == url_port {
            return true;
        }
        let ports = origin.port == url_port
            || (origin.port == default_port(&origin.scheme) && url.port().is_none());
        let upgrade = matches!(scheme, "https" | "wss") || (origin.scheme == "http" && matches!(scheme, "http" | "ws"));
        return ports && upgrade;
    }
    // scheme-source: "scheme:".
    if let Some(name) = expression.strip_suffix(':') {
        if is_scheme(name) {
            return scheme_part_matches(&name.to_ascii_lowercase(), scheme);
        }
        return false;
    }
    // host-source: [ scheme "://" ] host [ ":" port ] [ path ].
    let (expression_scheme, rest) = match expression.find("://") {
        Some(i) if is_scheme(&expression[..i]) => (Some(expression[..i].to_ascii_lowercase()), &expression[i + 3..]),
        Some(_) => return false,
        None => (None, expression),
    };
    let host_end = rest.find([':', '/']).unwrap_or(rest.len());
    let host_pattern = rest[..host_end].to_ascii_lowercase();
    if host_pattern.is_empty()
        || !host_pattern.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'*'))
    {
        return false;
    }
    let mut tail = &rest[host_end..];
    let port = match tail.strip_prefix(':') {
        Some(after) => {
            let end = after.find('/').unwrap_or(after.len());
            let port = &after[..end];
            tail = &after[end..];
            Some(port)
        }
        None => None,
    };
    let path = tail;
    let Some(host) = url.host_str() else { return false };
    let scheme_ok = match (&expression_scheme, self_origin) {
        (Some(s), _) => scheme_part_matches(s, scheme),
        (None, Some(origin)) => scheme_part_matches(&origin.scheme, scheme),
        // An opaque origin: only the HTTP(S) schemes.
        (None, None) => matches!(scheme, "http" | "https"),
    };
    if !scheme_ok {
        return false;
    }
    if !host_part_matches(&host_pattern, &host.to_ascii_lowercase()) {
        return false;
    }
    if !port_part_matches(port, url) {
        return false;
    }
    redirected || path_part_matches(path, url)
}

/// A nonce-source of `list` is `'nonce-<nonce>'` (the value compared case-sensitively).
fn nonce_matches(list: &[String], nonce: &str) -> bool {
    list.iter().any(|e| nonce_value(e) == Some(nonce))
}

/// The value of a nonce-source (`'nonce-<value>'`).
fn nonce_value(expression: &str) -> Option<&str> {
    let prefix = expression.get(..7)?;
    if !prefix.eq_ignore_ascii_case("'nonce-") || expression.len() <= 8 || !expression.ends_with('\'') {
        return None;
    }
    expression.get(7..expression.len() - 1)
}

/// The hash algorithms CSP knows.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum HashAlgorithm {
    Sha256,
    Sha384,
    Sha512,
}

impl HashAlgorithm {
    fn parse(name: &str) -> Option<HashAlgorithm> {
        match name.to_ascii_lowercase().as_str() {
            "sha256" => Some(HashAlgorithm::Sha256),
            "sha384" => Some(HashAlgorithm::Sha384),
            "sha512" => Some(HashAlgorithm::Sha512),
            _ => None,
        }
    }

    fn digest(self, data: &[u8]) -> String {
        use base64::Engine as _;
        use sha2::Digest as _;
        let bytes: Vec<u8> = match self {
            HashAlgorithm::Sha256 => sha2::Sha256::digest(data).to_vec(),
            HashAlgorithm::Sha384 => sha2::Sha384::digest(data).to_vec(),
            HashAlgorithm::Sha512 => sha2::Sha512::digest(data).to_vec(),
        };
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }
}

/// A hash-source (`'sha256-<base64>'`): its algorithm and value.
fn hash_source(expression: &str) -> Option<(HashAlgorithm, &str)> {
    let inner = expression.strip_prefix('\'')?.strip_suffix('\'')?;
    let (algorithm, value) = inner.split_once('-')?;
    Some((HashAlgorithm::parse(algorithm)?, value))
}

/// The hash-sources of `list`.
fn hash_sources(list: &[String]) -> impl Iterator<Item = (HashAlgorithm, &str)> + '_ {
    list.iter().filter_map(|e| hash_source(e))
}

fn is_hash_source(expression: &str) -> bool {
    hash_source(expression).is_some()
}

fn is_nonce_source(expression: &str) -> bool {
    nonce_value(expression).is_some()
}

/// Base64 and base64url spell the same digest (Chrome compares them as one), padding aside.
fn normalize_base64(value: &str) -> String {
    value.trim_end_matches('=').chars().map(|c| match c {
        '-' => '+',
        '_' => '/',
        c => c,
    }).collect()
}

/// CSP3 §6.7.2.3 "Does integrity metadata match source list?": every hash of `integrity` is in
/// `list` (and there is at least one).
fn integrity_matches(list: &[String], integrity: &str) -> bool {
    let sources: Vec<(HashAlgorithm, String)> =
        hash_sources(list).map(|(a, v)| (a, normalize_base64(v))).collect();
    if sources.is_empty() {
        return false;
    }
    let mut any = false;
    for item in integrity.split(is_ascii_ws).filter(|t| !t.is_empty()) {
        let item = item.split('?').next().unwrap_or(item);
        let Some((algorithm, value)) = item.split_once('-') else { return false };
        let Some(algorithm) = HashAlgorithm::parse(algorithm) else { return false };
        let value = normalize_base64(value);
        if !sources.iter().any(|(a, v)| *a == algorithm && *v == value) {
            return false;
        }
        any = true;
    }
    any
}

/// CSP3 §6.7.3.2 "Does a source list allow all inline behavior for type?".
fn allows_all_inline(list: &[String], kind: InlineKind) -> bool {
    let mut allow = false;
    for expression in list {
        if is_nonce_source(expression) || is_hash_source(expression) {
            return false;
        }
        if kind.is_script() && expression.eq_ignore_ascii_case("'strict-dynamic'") {
            return false;
        }
        if expression.eq_ignore_ascii_case("'unsafe-inline'") {
            allow = true;
        }
    }
    allow
}

/// CSP3 §6.7.3.3 "Does element match source list for type and source?".
fn element_matches_source_list(list: &[String], kind: InlineKind, nonce: &str, source: &str) -> bool {
    if allows_all_inline(list, kind) {
        return true;
    }
    let element = matches!(kind, InlineKind::Script | InlineKind::Style);
    if element && !nonce.is_empty() && nonce_matches(list, nonce) {
        return true;
    }
    if element || has_keyword(list, "'unsafe-hashes'") {
        for (algorithm, value) in hash_sources(list) {
            if normalize_base64(&algorithm.digest(source.as_bytes())) == normalize_base64(value) {
                return true;
            }
        }
    }
    false
}

/// A request's CSP state while it is in flight: its context's policies and metadata, checked
/// again by the client at every redirect hop (`Request::csp`), and the violations those checks
/// found, until the initiator takes them.
#[derive(Debug)]
pub struct RequestCsp {
    pub list: Arc<PolicyList>,
    pub request: CspRequest,
    /// The pre-request check blocked the request: the client fails it before sending, with
    /// [`BLOCKED_BY_CSP`] (an initiator that reuses its network-error path this way).
    blocked: bool,
    violations: Mutex<Vec<Violation>>,
}

impl RequestCsp {
    pub fn new(list: Arc<PolicyList>, request: CspRequest) -> Arc<RequestCsp> {
        Arc::new(RequestCsp { list, request, blocked: false, violations: Mutex::new(Vec::new()) })
    }

    /// The guard of a request its pre-request check blocked: the client fails it unsent.
    pub fn blocked(list: Arc<PolicyList>, request: CspRequest) -> Arc<RequestCsp> {
        Arc::new(RequestCsp { list, request, blocked: true, violations: Mutex::new(Vec::new()) })
    }

    /// Whether the client must fail the request before sending it.
    pub fn blocks_request(&self) -> bool {
        self.blocked
    }

    /// The check of a redirect to `target` (the pre-request check of the next hop, redirect
    /// flag set): whether it may be followed. Its violations are kept.
    pub fn allows_redirect(&self, target: &Url) -> bool {
        let mut hop = self.request.clone();
        hop.url = target.clone();
        hop.redirected = true;
        let outcome = self.list.pre_request(&hop);
        if !outcome.violations.is_empty() {
            self.violations.lock().unwrap_or_else(|e| e.into_inner()).extend(outcome.violations);
        }
        !outcome.blocked
    }

    /// The violations the redirect checks found so far.
    pub fn take_violations(&self) -> Vec<Violation> {
        std::mem::take(&mut *self.violations.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// The network error text of a request CSP blocked (CDP `blockedReason` "csp").
pub const BLOCKED_BY_CSP: &str = "net::ERR_BLOCKED_BY_CSP";

#[cfg(test)]
mod tests {
    use super::*;

    fn list(policy: &str, page: &str) -> PolicyList {
        let mut list = PolicyList::new(Some(&Url::parse(page).unwrap()));
        list.add_header(policy, Disposition::Enforce);
        list
    }

    fn allowed(policy: &str, url: &str, dest: RequestDest) -> bool {
        list(policy, "https://a.test/page.html").pre_request(&CspRequest::new(Url::parse(url).unwrap(), dest)).allowed()
    }

    #[test]
    fn source_expressions() {
        assert!(allowed("script-src 'self'", "https://a.test/x.js", RequestDest::Script));
        assert!(!allowed("script-src 'self'", "https://b.test/x.js", RequestDest::Script));
        assert!(!allowed("script-src 'self'", "data:text/javascript,1", RequestDest::Script));
        assert!(allowed("script-src *", "https://b.test/x.js", RequestDest::Script));
        assert!(!allowed("script-src *", "data:text/javascript,1", RequestDest::Script));
        assert!(allowed("script-src data:", "data:text/javascript,1", RequestDest::Script));
        assert!(allowed("img-src *.b.test", "https://c.b.test/i.png", RequestDest::Image));
        assert!(!allowed("img-src *.b.test", "https://b.test/i.png", RequestDest::Image));
        assert!(allowed("img-src https://b.test:*", "https://b.test:8443/i.png", RequestDest::Image));
        assert!(!allowed("img-src https://b.test", "https://b.test:8443/i.png", RequestDest::Image));
        assert!(allowed("img-src http://b.test", "https://b.test/i.png", RequestDest::Image));
        assert!(allowed("img-src b.test/dir/", "https://b.test/dir/i.png", RequestDest::Image));
        assert!(!allowed("img-src b.test/dir/i.png", "https://b.test/dir/j.png", RequestDest::Image));
        assert!(!allowed("img-src 'none'", "https://a.test/i.png", RequestDest::Image));
        assert!(allowed("default-src 'none'; img-src 'self'", "https://a.test/i.png", RequestDest::Image));
        assert!(!allowed("default-src 'none'", "https://a.test/i.png", RequestDest::Image));
        assert!(allowed("connect-src 'self'", "wss://a.test/s", RequestDest::WebSocket));
    }

    #[test]
    fn worker_fallback_and_redirects() {
        assert!(!allowed("script-src 'self'", "https://b.test/w.js", RequestDest::Worker));
        assert!(allowed("script-src 'self'; worker-src *", "https://b.test/w.js", RequestDest::Worker));
        assert!(allowed("worker-src 'none'", "https://b.test/x.js", RequestDest::PaintWorklet));
        let list = Arc::new(list("script-src 'self' https://b.test/only.js", "https://a.test/"));
        let guard = RequestCsp::new(list, CspRequest::new(Url::parse("https://b.test/only.js").unwrap(), RequestDest::Script));
        assert!(guard.allows_redirect(&Url::parse("https://b.test/elsewhere.js").unwrap()));
        assert!(!guard.allows_redirect(&Url::parse("https://c.test/only.js").unwrap()));
        let violations = guard.take_violations();
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].blocked_url, "https://b.test/only.js");
    }

    #[test]
    fn nonces_hashes_and_strict_dynamic() {
        let list = list("script-src 'nonce-abc' 'strict-dynamic' 'sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU='", "https://a.test/");
        let mut request = CspRequest::new(Url::parse("https://b.test/x.js").unwrap(), RequestDest::Script);
        request.parser_inserted = true;
        assert!(list.pre_request(&request).blocked);
        request.nonce = "abc".into();
        assert!(!list.pre_request(&request).blocked);
        request.nonce.clear();
        request.parser_inserted = false;
        assert!(!list.pre_request(&request).blocked);
        assert!(!list.inline_check(InlineKind::Script, "", "").blocked);
        assert!(list.inline_check(InlineKind::Script, "", "alert(1)").blocked);
        assert!(!list.inline_check(InlineKind::Script, "abc", "alert(1)").blocked);
        assert!(list.inline_check(InlineKind::ScriptAttribute, "", "").blocked);
        assert!(list.eval_check("1").blocked);
    }

    #[test]
    fn report_only_reports_without_blocking() {
        let mut list = PolicyList::new(Some(&Url::parse("https://a.test/").unwrap()));
        list.add_header("img-src 'none'", Disposition::Report);
        let outcome = list.pre_request(&CspRequest::new(Url::parse("https://a.test/i.png").unwrap(), RequestDest::Image));
        assert!(!outcome.blocked);
        assert_eq!(outcome.violations.len(), 1);
        assert_eq!(outcome.violations[0].disposition, Disposition::Report);
        assert_eq!(outcome.violations[0].effective_directive, "img-src");
    }
}
