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
    /// The context's `Connection-Allowlist` headers, checked with every request alongside the
    /// policies ([`crate::connection_allowlist`]).
    pub allowlist: Option<Arc<crate::connection_allowlist::ConnectionPolicy>>,
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

/// The fetch directives that also allow a `<link rel=prefetch>` (Chrome), besides
/// `default-src`: those that govern URLs (not the `-attr` ones, which govern inline code).
const PREFETCH_DIRECTIVES: &[&str] = &[
    "child-src",
    "connect-src",
    "fenced-frame-src",
    "font-src",
    "frame-src",
    "img-src",
    "manifest-src",
    "media-src",
    "object-src",
    "script-src",
    "script-src-elem",
    "style-src",
    "style-src-elem",
    "worker-src",
];

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

/// Who started a request, as far as its caller can vouch for it: the request's cryptographic
/// nonce and parser metadata (Fetch) come only from here.
///
/// A nonce comes only from a script element: [`ScriptInitiator::Element`], built by a caller
/// that read it from the initiating element's own cryptographic nonce slot (a nonceable
/// element: HTML "is element nonceable"), or [`ScriptInitiator::Descendant`], a module import
/// carrying the nonce its referring script's element passed down (HTML "descendant script fetch
/// options": the engine names the referring module, whose options the caller recorded from its
/// element). What no script element vouches for — an `import()` of an event handler or a
/// `javascript:` URL, a preload, an engine-internal fetch — is
/// [`ScriptInitiator::Unattributed`]: no nonce, so it never matches a nonce-source of any
/// policy, and HTML's default script fetch options' "not-parser-inserted".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ScriptInitiator {
    #[default]
    Unattributed,
    /// A `<script>` (or `<link>`, `<style>`) element of the policy's document.
    Element {
        /// Its cryptographic nonce (empty: none).
        nonce: String,
        /// It was inserted by the parser (`'strict-dynamic'` does not trust it).
        parser_inserted: bool,
    },
    /// A module import (static, or a script's `import()`) with its referring script's options:
    /// that script element's nonce, and never parser-inserted.
    Descendant {
        /// The root element's cryptographic nonce (empty: none).
        nonce: String,
    },
}

impl ScriptInitiator {
    /// The nonce a nonce-source may match: an element's non-empty one, else none.
    pub fn nonce(&self) -> Option<&str> {
        match self {
            ScriptInitiator::Element { nonce, .. } | ScriptInitiator::Descendant { nonce } if !nonce.is_empty() => {
                Some(nonce)
            }
            _ => None,
        }
    }

    /// Fetch "parser metadata" is "parser-inserted".
    pub fn parser_inserted(&self) -> bool {
        matches!(self, ScriptInitiator::Element { parser_inserted: true, .. })
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
    /// Its initiator: the nonce and parser metadata, when the caller can attribute them.
    pub initiator: ScriptInitiator,
    /// The request's integrity metadata (empty: none).
    pub integrity: String,
    /// The request went through a redirect: path parts of host-sources are not matched.
    pub redirected: bool,
}

impl CspRequest {
    /// A request of `url` for `dest`, unattributed, with no integrity metadata.
    pub fn new(url: Url, dest: RequestDest) -> CspRequest {
        CspRequest {
            original_url: url.clone(),
            url,
            dest,
            initiator: ScriptInitiator::Unattributed,
            integrity: String::new(),
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
        PolicyList { policies: Vec::new(), self_origin: self_url.and_then(SelfOrigin::of), allowlist: None }
    }

    /// An empty list whose `'self'` is `origin` (serialized; `"null"` or `None`: opaque).
    pub fn for_origin(origin: Option<&str>) -> PolicyList {
        PolicyList { policies: Vec::new(), self_origin: origin.and_then(SelfOrigin::parse), allowlist: None }
    }

    /// Whether the list has no policy (a connection allowlist is not one: [`PolicyList::has_allowlist`]).
    pub fn is_empty(&self) -> bool {
        self.policies.is_empty()
    }

    /// Whether the list carries a `Connection-Allowlist`.
    pub fn has_allowlist(&self) -> bool {
        self.allowlist.is_some()
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

    /// CSP Integrity: the policies whose `script-src-elem` (or its fallback) holds
    /// `'report-sha256'`, `'report-sha384'` or `'report-sha512'` source expressions, each with its
    /// `report-to` group and those algorithms (`"sha256"`...), in order. Disposition does not
    /// matter: a report-only policy asks for the hashes too, and nothing is blocked either way.
    pub fn script_hash_reports(&self) -> Vec<(Option<String>, Vec<&'static str>)> {
        let mut out = Vec::new();
        for policy in &self.policies {
            let Some(directive) = policy.directive_for("script-src-elem") else { continue };
            let algorithms: Vec<&'static str> = [("'report-sha256'", "sha256"), ("'report-sha384'", "sha384"), ("'report-sha512'", "sha512")]
                .into_iter()
                .filter(|(keyword, _)| has_keyword(&directive.value, keyword))
                .map(|(_, algorithm)| algorithm)
                .collect();
            if !algorithms.is_empty() {
                out.push((policy.report_to(), algorithms));
            }
        }
        out
    }

    /// CSP3 §4.1.2 "Should request be blocked by Content Security Policy?" (the pre-request
    /// check of each policy's directive for the request's destination).
    pub fn pre_request(&self, request: &CspRequest) -> CheckOutcome {
        // The connection allowlist first: a request it refuses is not matched against the policies.
        let mut outcome = match &self.allowlist {
            Some(allowlist) => allowlist.check(&request.url, &request.original_url, request.redirected),
            None => CheckOutcome::default(),
        };
        if outcome.blocked {
            return outcome;
        }
        let csp = self.check_url(request, &request.url);
        outcome.blocked = csp.blocked;
        outcome.violations.extend(csp.violations);
        outcome
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
            if request.dest == RequestDest::Prefetch && self.prefetch_allowed_by_any(policy, request, url) {
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
    /// script and worker destinations, the nonce rule for styles too). Each policy's directive
    /// is checked on its own (`check_url`): what one policy's nonce-source allows says nothing
    /// for another. A nonce matches only the request's attributed one
    /// ([`ScriptInitiator::nonce`]).
    fn request_allowed_by(&self, directive: &Directive, request: &CspRequest, url: &Url) -> bool {
        let list = &directive.value;
        // A `<link rel=prefetch>` matches its own nonce too (Chrome).
        let nonce_applies =
            request.dest.is_script_like() || matches!(request.dest, RequestDest::Style | RequestDest::Prefetch);
        if nonce_applies && request.initiator.nonce().is_some_and(|nonce| nonce_matches(list, nonce)) {
            return true;
        }
        if request.dest.is_script_like() {
            if integrity_matches(list, &request.integrity) || ed25519_matches(list, &request.integrity) {
                return true;
            }
            if has_keyword(list, "'strict-dynamic'") {
                return !request.initiator.parser_inserted();
            }
        }
        url_matches_source_list(list, url, self.self_origin.as_ref(), request.redirected)
    }

    /// A `<link rel=prefetch>` that the policy's `default-src` does not allow: Chrome allows it
    /// when any other fetch directive the policy has allows the URL. Each directive is checked
    /// on its own, with no fallback, so a permissive `script-src` allows it even when
    /// `script-src-elem` is stricter. `prefetch-src` is not a directive (CSP3 dropped it). A
    /// nonce-source matches the link's nonce.
    fn prefetch_allowed_by_any(&self, policy: &Policy, request: &CspRequest, url: &Url) -> bool {
        PREFETCH_DIRECTIVES.iter().filter_map(|name| policy.directive(name)).any(|directive| {
            request.initiator.nonce().is_some_and(|nonce| nonce_matches(&directive.value, nonce))
                || url_matches_source_list(&directive.value, url, self.self_origin.as_ref(), request.redirected)
        })
    }

    /// CSP3 §4.2.3 "Should element's inline type behavior be blocked?": an inline script or
    /// style (`nonce`: the element's, empty for attributes and navigations; `source`: the text
    /// the hashes are of).
    pub fn inline_check(&self, kind: InlineKind, nonce: &str, source: &str) -> CheckOutcome {
        self.inline_check_with(kind, nonce, source, false)
    }

    /// [`PolicyList::inline_check`] for an inline `<script>` a script inserted (not the
    /// parser): a policy with `'strict-dynamic'` trusts it, as it trusts the fetches of
    /// non-parser-inserted scripts (Chrome; WPT script-src-strict_dynamic_non_parser_inserted).
    pub fn inserted_script_inline_check(&self, nonce: &str, source: &str) -> CheckOutcome {
        self.inline_check_with(InlineKind::Script, nonce, source, true)
    }

    fn inline_check_with(&self, kind: InlineKind, nonce: &str, source: &str, script_inserted: bool) -> CheckOutcome {
        let mut outcome = CheckOutcome::default();
        let effective = kind.effective_directive();
        for policy in &self.policies {
            let Some(directive) = policy.directive_for(effective) else { continue };
            if script_inserted && has_keyword(&directive.value, "'strict-dynamic'") {
                continue;
            }
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

    /// `frame-ancestors` (CSP3 §6.4.2.1, no fallback): this list is a navigation response's
    /// (header policies only; `<meta>` drops the directive), and `ancestors` the origins of
    /// the frames above the one it would commit in, parent first, up to the top-level one
    /// (`None`: an opaque origin, which nothing matches). Each policy reports the first
    /// ancestor its directive does not match (blocked URI: that ancestor's origin). An origin
    /// has no path, so a source with a path never matches one (Chrome, and §6.4.2.1 6.2-6.3).
    pub fn frame_ancestors_check(&self, ancestors: &[Option<Url>]) -> CheckOutcome {
        let mut outcome = CheckOutcome::default();
        for policy in &self.policies {
            let Some(directive) = policy.directive("frame-ancestors") else { continue };
            let refused = ancestors.iter().find(|ancestor| match ancestor {
                Some(url) => !url_matches_source_list(&directive.value, url, self.self_origin.as_ref(), false),
                None => true,
            });
            let Some(ancestor) = refused else { continue };
            let blocked = ancestor.as_ref().map_or_else(|| "null".to_owned(), |url| url.origin().ascii_serialization());
            outcome.violations.push(self.violation(policy, directive, "frame-ancestors", &blocked, &blocked, ""));
            if policy.disposition == Disposition::Enforce {
                outcome.blocked = true;
            }
        }
        outcome
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

/// The hash-source value of `data` for `algorithm` (`"sha256"`, `"sha384"`, `"sha512"`):
/// `sha256-<base64>`; `None` for another name.
pub fn hash_expression(algorithm: &str, data: &[u8]) -> Option<String> {
    let algorithm = HashAlgorithm::parse(algorithm)?;
    let name = match algorithm {
        HashAlgorithm::Sha256 => "sha256",
        HashAlgorithm::Sha384 => "sha384",
        HashAlgorithm::Sha512 => "sha512",
    };
    Some(format!("{name}-{}", algorithm.digest(data)))
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

/// The unpadded standard-base64 form of the Ed25519 public keys (32 bytes) of `integrity`
/// metadata (`ed25519-<base64>` tokens; others and malformed keys are not keys).
fn ed25519_keys(integrity: &str) -> Vec<String> {
    integrity
        .split(is_ascii_ws)
        .filter_map(|item| {
            let (algorithm, value) = item.split('?').next()?.split_once('-')?;
            algorithm.eq_ignore_ascii_case("ed25519").then(|| normalize_base64(value))
        })
        // 32 bytes are 43 characters of unpadded base64.
        .filter(|key| key.len() == 43 && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/'))
        .collect()
}

/// Signature-based SRI's `'ed25519-<key>'` source expressions: the request's integrity metadata has
/// at least one Ed25519 key, and every one of them is in `list`.
fn ed25519_matches(list: &[String], integrity: &str) -> bool {
    let keys = ed25519_keys(integrity);
    if keys.is_empty() {
        return false;
    }
    let sources: Vec<String> = list
        .iter()
        .filter_map(|e| {
            let value = e.strip_prefix('\'')?.strip_suffix('\'')?;
            let (algorithm, key) = value.split_once('-')?;
            algorithm.eq_ignore_ascii_case("ed25519").then(|| normalize_base64(key))
        })
        .collect();
    keys.iter().all(|key| sources.contains(key))
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
    /// The URL the last check covered (the request's, then each redirect target's): a response
    /// of that URL is not checked again (its report-only violations were reported once).
    last_checked: Mutex<Url>,
    /// The pre-request check blocked the request: the client fails it before sending, with
    /// [`BLOCKED_BY_CSP`] (an initiator that reuses its network-error path this way).
    blocked: bool,
    violations: Mutex<Vec<Violation>>,
}

impl RequestCsp {
    /// The guard of `request`, whose pre-request check against `list` its initiator ran.
    pub fn new(list: Arc<PolicyList>, request: CspRequest) -> Arc<RequestCsp> {
        let last_checked = Mutex::new(request.url.clone());
        Arc::new(RequestCsp { list, request, last_checked, blocked: false, violations: Mutex::new(Vec::new()) })
    }

    /// The guard of a request its pre-request check blocked: the client fails it unsent.
    pub fn blocked(list: Arc<PolicyList>, request: CspRequest) -> Arc<RequestCsp> {
        let last_checked = Mutex::new(request.url.clone());
        Arc::new(RequestCsp { list, request, last_checked, blocked: true, violations: Mutex::new(Vec::new()) })
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
        *self.last_checked.lock().unwrap_or_else(|e| e.into_inner()) = target.clone();
        if !outcome.violations.is_empty() {
            self.violations.lock().unwrap_or_else(|e| e.into_inner()).extend(outcome.violations);
        }
        !outcome.blocked
    }

    /// CSP3 "Should response to request be blocked?": the post-request check of the URL of
    /// the response the request ends with, which a service worker's or a cache's answer can
    /// make another than its last hop's (one equal to it was checked already). Its violations
    /// are kept. Whether it may be used.
    pub fn allows_response(&self, response_url: &Url) -> bool {
        if *self.last_checked.lock().unwrap_or_else(|e| e.into_inner()) == *response_url {
            return true;
        }
        let outcome = self.list.post_request(&self.request, response_url);
        if !outcome.violations.is_empty() {
            self.violations.lock().unwrap_or_else(|e| e.into_inner()).extend(outcome.violations);
        }
        !outcome.blocked
    }

    /// The violations the redirect and response checks found so far.
    pub fn take_violations(&self) -> Vec<Violation> {
        std::mem::take(&mut *self.violations.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// The network error text of a request CSP blocked (CDP `blockedReason` "csp").
pub const BLOCKED_BY_CSP: &str = "net::ERR_BLOCKED_BY_CSP";

/// `sink` behind `guard`'s response check ([`RequestCsp::allows_response`]): a response whose
/// URL an enforced policy blocks reaches `sink` as a network error ([`BLOCKED_BY_CSP`]), and
/// nothing after it does. The client puts every guarded request's sink behind one; a path
/// that answers without the client (a service worker's `fetch` event) does too.
pub fn guard_sink(guard: Arc<RequestCsp>, mut sink: crate::client::ResponseSink) -> crate::client::ResponseSink {
    let mut blocked = false;
    Box::new(move |event| {
        if blocked {
            return;
        }
        if let crate::request::ResponseEvent::Response(head) = &event {
            if !guard.allows_response(&head.url) {
                blocked = true;
                sink(crate::request::ResponseEvent::Failed(crate::request::NetError::Blocked(BLOCKED_BY_CSP.into())));
                return;
            }
        }
        sink(event);
    })
}

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
    fn script_hash_reports_follow_the_fallback_list() {
        let reports = |policy: &str| list(policy, "https://a.test/page.html").script_hash_reports();
        assert_eq!(
            reports("script-src 'self' 'report-sha256' 'report-SHA512'; report-to g"),
            [(Some("g".to_owned()), vec!["sha256", "sha512"])]
        );
        assert_eq!(reports("default-src 'report-sha384'"), [(None, vec!["sha384"])]);
        assert!(reports("script-src-elem 'self'; script-src 'report-sha256'").is_empty());
        assert!(reports("img-src 'report-sha256'").is_empty());
        assert!(hash_expression("sha256", b"").unwrap().starts_with("sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="));
    }

    #[test]
    fn ed25519_source_expressions() {
        let key = "JrQLj5P/89iXES9+vFgrIy29clF9CC/oPPsw3c5D0bs=";
        let other = "xDnP380zcL4rJ76rXYjeHlfMyPZEOqpJYjsjEppbuXE=";
        let list = list(&format!("script-src 'ed25519-{key}'"), "https://a.test/page.html");
        let check = |integrity: &str| {
            let mut request = CspRequest::new(Url::parse("https://a.test/x.js").unwrap(), RequestDest::Script);
            request.integrity = integrity.to_owned();
            list.pre_request(&request).allowed()
        };
        assert!(check(&format!("ed25519-{key}")));
        assert!(check(&format!("ed25519-{key} malformed-thing")));
        // No key, a malformed one, or one the policy does not list: blocked.
        assert!(!check(""));
        assert!(!check("ed25519-???"));
        assert!(!check(&format!("ed25519-{other}")));
        assert!(!check(&format!("ed25519-{key} ed25519-{other}")));
    }

    #[test]
    fn prefetch_is_allowed_by_default_src_or_any_fetch_directive() {
        let p = RequestDest::Prefetch;
        assert!(allowed("default-src 'self'", "https://a.test/d.xml", p));
        assert!(!allowed("default-src 'none'; script-src 'unsafe-inline'", "https://a.test/d.xml", p));
        // No default-src: nothing governs it.
        assert!(allowed("script-src 'self'; img-src 'self'", "https://b.test/d.xml", p));
        // Any other fetch directive of the policy allows it, each without its fallback.
        assert!(allowed("default-src 'none'; img-src https://b.test", "https://b.test/d.xml", p));
        assert!(!allowed("default-src 'none'; img-src https://b.test", "https://c.test/d.xml", p));
        assert!(allowed("default-src 'none'; script-src-elem 'unsafe-inline'; script-src *", "https://b.test/d.xml", p));
        assert!(allowed("default-src 'none'; manifest-src *", "https://b.test/d.xml", p));
        // Not base-uri, nor the dropped prefetch-src.
        assert!(!allowed("default-src 'none'; base-uri *", "https://b.test/d.xml", p));
        assert!(!allowed("default-src 'none'; prefetch-src *", "https://b.test/d.xml", p));
        // Each policy on its own: the first one's default-src blocks whatever the second allows.
        assert!(!allowed("default-src 'unsafe-inline', default-src 'self'; script-src 'unsafe-inline'", "https://a.test/d.xml", p));
        assert!(allowed("default-src 'none'; img-src *; connect-src 'none', default-src 'none'; img-src 'none'; connect-src *", "https://a.test/d.xml", p));
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
        request.initiator = element("", true);
        assert!(list.pre_request(&request).blocked);
        request.initiator = element("abc", true);
        assert!(!list.pre_request(&request).blocked);
        request.initiator = element("", false);
        assert!(!list.pre_request(&request).blocked);
        assert!(!list.inline_check(InlineKind::Script, "", "").blocked);
        assert!(list.inline_check(InlineKind::Script, "", "alert(1)").blocked);
        assert!(!list.inline_check(InlineKind::Script, "abc", "alert(1)").blocked);
        assert!(list.inline_check(InlineKind::ScriptAttribute, "", "").blocked);
        assert!(list.eval_check("1").blocked);
    }

    fn element(nonce: &str, parser_inserted: bool) -> ScriptInitiator {
        ScriptInitiator::Element { nonce: nonce.into(), parser_inserted }
    }

    fn script(policies: &[(&str, Disposition)], initiator: ScriptInitiator) -> CheckOutcome {
        let mut list = PolicyList::new(Some(&Url::parse("https://a.test/").unwrap()));
        for (text, disposition) in policies {
            list.add_header(text, *disposition);
        }
        let mut request = CspRequest::new(Url::parse("https://b.test/m.js").unwrap(), RequestDest::Script);
        request.initiator = initiator;
        list.pre_request(&request)
    }

    #[test]
    fn an_attributed_element_nonce_allows() {
        let outcome = script(&[("script-src 'nonce-abc'", Disposition::Enforce)], element("abc", true));
        assert!(!outcome.blocked);
        assert!(outcome.violations.is_empty());
    }

    #[test]
    fn an_unattributed_request_never_matches_a_nonce() {
        // A module graph's import: the policy has a nonce-source, the request no attributed nonce.
        let outcome = script(&[("script-src 'nonce-abc'", Disposition::Enforce)], ScriptInitiator::Unattributed);
        assert!(outcome.blocked);
        assert_eq!(outcome.violations.len(), 1);
        assert_eq!(outcome.violations[0].effective_directive, "script-src-elem");
        assert_eq!(outcome.violations[0].violated_directive, "script-src");
        assert_eq!(ScriptInitiator::Unattributed.nonce(), None);
    }

    #[test]
    fn an_empty_or_different_nonce_does_not_match() {
        let policy = [("script-src 'nonce-abc'", Disposition::Enforce)];
        assert!(script(&policy, element("", false)).blocked);
        assert!(script(&policy, element("abd", false)).blocked);
        // Nonce values compare case-sensitively; the keyword itself does not.
        assert!(script(&policy, element("ABC", false)).blocked);
        assert!(!script(&[("script-src 'NONCE-abc'", Disposition::Enforce)], element("abc", false)).blocked);
    }

    #[test]
    fn each_policy_is_checked_on_its_own() {
        // The first policy allows the nonce, the second has no nonce-source and lists another
        // host: the request is blocked by the second alone.
        let policies =
            [("script-src 'nonce-abc'", Disposition::Enforce), ("script-src https://c.test", Disposition::Enforce)];
        let outcome = script(&policies, element("abc", false));
        assert!(outcome.blocked);
        assert_eq!(outcome.violations.len(), 1);
        assert_eq!(outcome.violations[0].original_policy, "script-src https://c.test");
        // Allowed by both: the nonce for one, the host for the other.
        let policies =
            [("script-src 'nonce-abc'", Disposition::Enforce), ("script-src https://b.test", Disposition::Enforce)];
        assert!(!script(&policies, element("abc", false)).blocked);
        // A nonce in one policy grants nothing for an unattributed request in either.
        let policies =
            [("script-src 'nonce-abc'", Disposition::Enforce), ("script-src 'nonce-abc'", Disposition::Report)];
        let outcome = script(&policies, ScriptInitiator::Unattributed);
        assert!(outcome.blocked);
        assert_eq!(outcome.violations.len(), 2);
    }

    #[test]
    fn an_import_inherits_its_root_elements_nonce() {
        // `<script type=module nonce=abc>` importing `m.js` under a nonce-only policy.
        let policy = [("script-src 'nonce-abc'", Disposition::Enforce)];
        let descendant = |nonce: &str| ScriptInitiator::Descendant { nonce: nonce.into() };
        assert!(!script(&policy, descendant("abc")).blocked);
        assert!(script(&policy, descendant("other")).blocked);
        assert!(script(&policy, descendant("")).blocked);
        assert!(!descendant("abc").parser_inserted());
        // Strict-dynamic trusts an import whatever its nonce: it is never parser-inserted.
        let strict = [("script-src 'strict-dynamic' 'nonce-abc'", Disposition::Enforce)];
        assert!(!script(&strict, descendant("")).blocked);
    }

    #[test]
    fn a_report_only_nonce_miss_reports_without_blocking() {
        let policies =
            [("script-src https://b.test", Disposition::Enforce), ("script-src 'nonce-xyz'", Disposition::Report)];
        let outcome = script(&policies, element("abc", false));
        assert!(!outcome.blocked);
        assert_eq!(outcome.violations.len(), 1);
        assert_eq!(outcome.violations[0].disposition, Disposition::Report);
    }

    #[test]
    fn strict_dynamic_trusts_script_inserted_inline_scripts() {
        let strict = list("script-src 'strict-dynamic' 'nonce-abc'", "https://a.test/");
        assert!(strict.inline_check(InlineKind::Script, "", "x()").blocked);
        assert!(!strict.inserted_script_inline_check("", "x()").blocked);
        assert!(!strict.inserted_script_inline_check("wrong", "x()").blocked);
        let plain = list("script-src 'nonce-abc'", "https://a.test/");
        assert!(plain.inserted_script_inline_check("", "x()").blocked);
        assert!(!plain.inserted_script_inline_check("abc", "x()").blocked);
    }

    #[test]
    fn strict_dynamic_trusts_only_non_parser_inserted_or_nonced() {
        let policy = [("script-src 'strict-dynamic' 'nonce-abc'", Disposition::Enforce)];
        assert!(script(&policy, element("", true)).blocked);
        assert!(!script(&policy, element("abc", true)).blocked);
        assert!(!script(&policy, element("", false)).blocked);
        // Unattributed: HTML's default fetch options, not parser-inserted.
        assert!(!script(&policy, ScriptInitiator::Unattributed).blocked);
    }

    #[test]
    fn a_redirect_keeps_the_initiator() {
        let mut list = PolicyList::new(Some(&Url::parse("https://a.test/").unwrap()));
        list.add_header("script-src 'nonce-abc' https://b.test/m.js", Disposition::Enforce);
        let list = Arc::new(list);
        let mut request = CspRequest::new(Url::parse("https://b.test/m.js").unwrap(), RequestDest::Script);
        request.initiator = element("abc", false);
        let guard = RequestCsp::new(list.clone(), request.clone());
        assert!(guard.allows_redirect(&Url::parse("https://c.test/other.js").unwrap()));
        request.initiator = ScriptInitiator::Unattributed;
        let guard = RequestCsp::new(list, request);
        assert!(!guard.allows_redirect(&Url::parse("https://c.test/other.js").unwrap()));
        assert_eq!(guard.take_violations().len(), 1);
    }

    #[test]
    fn a_response_of_another_url_is_checked() {
        let mut list = PolicyList::new(Some(&Url::parse("https://a.test/").unwrap()));
        list.add_header("script-src https://a.test/m.js", Disposition::Enforce);
        let request = CspRequest::new(Url::parse("https://a.test/m.js").unwrap(), RequestDest::Script);
        let guard = RequestCsp::new(Arc::new(list), request);
        // The request's own URL (checked before it was sent): not checked again.
        assert!(guard.allows_response(&Url::parse("https://a.test/m.js").unwrap()));
        assert!(guard.take_violations().is_empty());
        // A service worker's answer with another URL's response.
        assert!(!guard.allows_response(&Url::parse("https://evil.test/m.js").unwrap()));
        let violations = guard.take_violations();
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].blocked_url, "https://a.test/m.js");
        assert_eq!(violations[0].checked_url, "https://evil.test/m.js");
    }

    #[test]
    fn frame_ancestors_checks_every_ancestor_origin() {
        let child = Url::parse("http://a.test:8000/frame.html").unwrap();
        let same = Some(Url::parse("http://a.test:8000").unwrap());
        let cross = Some(Url::parse("http://b.test:8001").unwrap());
        let check = |policy: &str, disposition, ancestors: &[Option<Url>]| {
            let mut list = PolicyList::new(Some(&child));
            list.add_header(policy, disposition);
            list.frame_ancestors_check(ancestors)
        };
        assert!(check("frame-ancestors 'none'", Disposition::Enforce, &[same.clone()]).blocked);
        assert!(check("frame-ancestors 'self'", Disposition::Enforce, &[same.clone()]).allowed());
        // Every ancestor, not only the parent.
        let outcome = check("frame-ancestors 'self'", Disposition::Enforce, &[same.clone(), cross.clone()]);
        assert!(outcome.blocked);
        assert_eq!(outcome.violations[0].blocked_url, "http://b.test:8001");
        assert_eq!(outcome.violations[0].effective_directive, "frame-ancestors");
        assert!(check("frame-ancestors http://b.test:8001 'self'", Disposition::Enforce, &[same.clone(), cross.clone()]).allowed());
        assert!(check("frame-ancestors *", Disposition::Enforce, &[cross.clone()]).allowed());
        // A source with a path matches no origin.
        assert!(check("frame-ancestors http://a.test:8000/test", Disposition::Enforce, &[same.clone()]).blocked);
        assert!(check("frame-ancestors http://a.test:8000", Disposition::Enforce, &[same.clone()]).allowed());
        // An opaque ancestor matches nothing, `*` included.
        assert!(check("frame-ancestors *", Disposition::Enforce, &[None]).blocked);
        // No fallback to default-src; report-only reports without blocking.
        assert!(check("default-src 'none'", Disposition::Enforce, &[cross.clone()]).violations.is_empty());
        let outcome = check("frame-ancestors 'none'", Disposition::Report, &[same]);
        assert!(!outcome.blocked);
        assert_eq!(outcome.violations.len(), 1);
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
