//! Content Security Policy: Embedded Enforcement (<https://w3c.github.io/webappsec-cspee/>), as
//! Chrome implements it (services/network/public/cpp/content_security_policy, NavigationRequest).
//!
//! An HTML `<iframe csp="...">` requires a policy of the documents its frame loads:
//! - the attribute is a serialized policy (no CR/LF, at most 4096 bytes, ASCII directive names and
//!   values without `,`), else it is ignored ([`valid_attribute`]);
//! - the frame's required policy is the attribute's, when it has no reporting directive and the
//!   parent document's own required policy (if any) subsumes it, else the parent's
//!   ([`required_policy`]). Each navigation of the frame takes it once as it starts
//!   ([`requirement`]): the loader sends it as `Sec-Required-CSP` (`FetchRequest::required_csp`;
//!   the request keeps it on every redirect hop) and the commit judges the response against that
//!   same snapshot ([`judge`]);
//! - the response must either accept it (a local scheme, which about:blank and about:srcdoc
//!   documents are too, or `Allow-CSP-From` naming the parent's origin or `*`: the document then
//!   enforces it) or deliver policies that subsume it ([`decide`]); otherwise the navigation fails
//!   with `net::ERR_BLOCKED_BY_CSP` and the frame commits the opaque-origin error document, which
//!   loads;
//! - the committed document (the error one too) keeps the requirement, which bounds what its own
//!   frames may require (`DocumentState::required_csp`).
//!
//! Subsumption has its own small parser (below), made only for it, reading the response's
//! policies from the same `header_csp` the document is given. Enforcement is the CSP core's: an
//! accepted policy is appended to the document's CSP list as an enforced header policy
//! ([`enforce_on_document`]), after its own header policies and what a local-scheme document
//! inherited from its creator, so it is one more [`pyro_net::csp::Policy`] of the
//! [`pyro_net::csp::PolicyList`] every fetch, inline and eval check reads
//! (`pyro_webapi::csp::policy_list`). It is never written into `header_csp`, which would replace
//! the inherited policies.
//!
//! `Page.setBypassCSP`: the header is still sent, but nothing is judged or enforced (a page that
//! bypasses CSP records no policy at all).

use crate::frames::FrameKey;
use crate::Browser;
use std::collections::BTreeSet;

/// The error a navigation blocked by its frame's required policy fails with (Chrome's). A child
/// frame that fails with it commits Chrome's opaque error document, which loads, as for a
/// `frame-src` block (`Browser::document_fetch_failed`).
pub(crate) const BLOCKED_ERROR: &str = pyro_net::csp::BLOCKED_BY_CSP;

/// Blink's cap on the `csp` attribute's length.
const MAX_ATTRIBUTE_LENGTH: usize = 4096;

/// Whether subsumption looks at `trusted-types` and `require-trusted-types-for` (CSPEE PR #36).
/// Chrome ships `kCSPEmbeddedEnforcementTrustedTypes` disabled, so it does not.
const CHECK_TRUSTED_TYPES: bool = false;

const PORT_UNSPECIFIED: i32 = -1;

fn is_csp_whitespace(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0c' | '\r' | ' ')
}

fn trim(s: &str) -> &str {
    s.trim_matches(is_csp_whitespace)
}

fn is_directive_name(name: &str) -> bool {
    name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn is_directive_value(value: &str) -> bool {
    value.chars().all(|c| is_csp_whitespace(c) || (matches!(c, ' '..='~') && c != ',' && c != ';'))
}

/// Blink `HTMLIFrameElement::ParseAttribute` for `csp`: a value with CR or LF, one that is not a
/// serialized policy, or one longer than 4096 is ignored (the frame has no `csp` attribute).
pub(crate) fn valid_attribute(value: &str) -> bool {
    value.len() <= MAX_ATTRIBUTE_LENGTH && !value.contains(['\n', '\r']) && matches_serialized_policy(value)
}

/// CSPEE "serialized-policy": directives separated by `;`, each a name of ASCII alphanumerics and
/// `-`, then optionally whitespace and a value of visible ASCII other than `,` and `;`.
fn matches_serialized_policy(value: &str) -> bool {
    value.split(';').all(|token| {
        let token = trim(token);
        if token.is_empty() {
            return true;
        }
        let end = token.find(is_csp_whitespace).unwrap_or(token.len());
        let (name, rest) = token.split_at(end);
        !name.is_empty() && is_directive_name(name) && is_directive_value(rest)
    })
}

/// The policy text of a `csp` attribute that passed [`valid_attribute`]: `None` when it is empty
/// (Blink parses it to no policy).
fn attribute_policy(value: &str) -> Option<&str> {
    // The grammar admits no `,`: one policy at most.
    Some(trim(value)).filter(|v| !v.is_empty())
}

/// The policy a child frame's navigation to `url` requires (Chrome
/// `NavigationRequest::SetupCSPEmbeddedEnforcement`): its container's `csp` attribute when that is
/// valid, has no `report-uri`/`report-to` and is subsumed by `parent`, the required policy of the
/// parent document; else `parent`.
pub(crate) fn required_policy(attribute: Option<&str>, parent: Option<&str>, url: &str) -> Option<String> {
    if let Some(own) = attribute.filter(|v| valid_attribute(v)).and_then(attribute_policy) {
        let policy = Policy::parse(own, false);
        let reports = policy.raw.iter().any(|d| matches!(d, Directive::ReportUri | Directive::ReportTo));
        if !reports && parent.is_none_or(|p| subsumes(&Policy::parse(p, false), &[policy], &self_origin(url))) {
            return Some(own.to_owned());
        }
    }
    parent.map(str::to_owned)
}

/// The required policy of a child frame's navigation, snapshot once as the navigation starts
/// ([`requirement`]) and carried to its commit: the loader sends it (`FetchRequest::required_csp`)
/// and the commit checks the response against it ([`judge`]), whatever the attribute or the parent
/// became meanwhile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Requirement {
    /// The serialized policy (the `Sec-Required-CSP` value).
    pub policy: String,
    /// The serialized origin of the parent document (`Allow-CSP-From` must name it).
    pub parent_origin: String,
}

/// What a navigation's response does with its frame's required policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// The response's own policies subsume it.
    Allow,
    /// The response accepts it (a local scheme, `Allow-CSP-From`): the document enforces it.
    Enforce(String),
    /// Neither: the navigation is blocked.
    Block,
}

/// The requirement of a navigation of the child frame `key` to `url`, if any: the `csp` attribute
/// of its container when that is an HTML `<iframe>` (an `<object>`, `<embed>` or `<frame>` has
/// none), bounded by the required policy of the parent's current document.
///
/// The attribute is read with a plain `borrow()`, as the rest of a frame navigation's start reads
/// its container (`process_iframe_attributes`, `srcdoc_of`, the fullscreen container policy): a
/// navigation never starts inside a DOM mutation, and if one did it would fail loudly rather than
/// load the frame without the policy its container asks for.
pub(crate) fn requirement(browser: &Browser, key: FrameKey, url: &str) -> Option<Requirement> {
    key.child?;
    let host = browser.page(key.page)?.frames.clone();
    let (owner, parent) = {
        let doc = browser.doc(key)?;
        (doc.main_frame.owner, doc.main_frame.parent.clone())
    };
    let parent = parent.and_then(|id| host.key(&id));
    let (parent_required, parent_origin) = match parent {
        Some(parent) => {
            let doc = browser.doc(parent);
            (
                doc.as_ref().and_then(|d| d.required_csp.clone()),
                doc.as_ref().map_or_else(|| "null".to_owned(), |d| d.main_frame.origin.clone()),
            )
        }
        None => (None, "null".to_owned()),
    };
    let attribute = owner.and_then(|owner| {
        let dom = host.dom.borrow();
        let el = dom.element(owner).ok()?;
        if !el.is_html() || &*el.name.local != "iframe" {
            return None;
        }
        let value = el.attr("csp").map(str::to_owned);
        value
    });
    let policy = required_policy(attribute.as_deref(), parent_required.as_deref(), url)?;
    Some(Requirement { policy, parent_origin })
}

/// Chrome `NavigationRequest::CheckCSPEmbeddedEnforcement` for `resource`, the response a
/// navigation under `requirement` got (its URL after redirects, its policies as the document will
/// enforce them, its `Allow-CSP-From`). A document made locally (about:srcdoc, about:blank) is
/// decided the same way: its scheme accepts the policy.
pub(crate) fn decide(requirement: &Requirement, resource: &crate::loader::Resource) -> Verdict {
    // AllowsBlanketEnforcementOfRequiredCSP.
    let scheme = resource.url.split(':').next().unwrap_or("").to_ascii_lowercase();
    if matches!(scheme.as_str(), "about" | "data" | "file" | "filesystem" | "blob") {
        return Verdict::Enforce(requirement.policy.clone());
    }
    if allows_csp_from(resource.allow_csp_from.as_deref(), &requirement.parent_origin) {
        return Verdict::Enforce(requirement.policy.clone());
    }
    let policies = document_policies(resource.header_csp.as_deref());
    if subsumes(&Policy::parse(&requirement.policy, false), &policies, &self_origin(&resource.url)) {
        Verdict::Allow
    } else {
        Verdict::Block
    }
}

/// `resource`, the response of a navigation of `key` that took `requirement` as it started, as it
/// may commit ([`decide`], then [`apply`]): `Err` when blocked, else the resource, unchanged, and
/// the required policy its document must enforce on top of its own ([`Browser::prepare_required_csp`]).
/// No requirement, or a page that bypasses CSP (`Page.setBypassCSP`), judges nothing.
pub(crate) fn judge(
    browser: &Browser,
    key: FrameKey,
    requirement: Option<&Requirement>,
    resource: crate::loader::Resource,
) -> Result<(crate::loader::Resource, Option<String>), String> {
    let Some(requirement) = requirement else { return Ok((resource, None)) };
    if browser.page(key.page).is_some_and(|p| p.bypass_csp) {
        return Ok((resource, None));
    }
    let enforce = apply(Some(decide(requirement, &resource)))?;
    Ok((resource, enforce))
}

/// The policies of a document's `header_csp` (`Resource::header_csp`: its enforced policies joined
/// by `,`, a newline, its report-only ones), split as `Dom::set_trusted_types_policies` splits
/// them: subsumption judges exactly the policies the document is given.
fn document_policies(header_csp: Option<&str>) -> Vec<Policy> {
    let Some(csp) = header_csp else { return Vec::new() };
    let (enforce, report_only) = csp.split_once('\n').unwrap_or((csp, ""));
    let mut out = Vec::new();
    for (list, report_only) in [(enforce, false), (report_only, true)] {
        for piece in list.split(',').map(trim).filter(|p| !p.is_empty()) {
            out.push(Policy::parse(piece, report_only));
        }
    }
    out
}

/// `Allow-CSP-From` (its values joined by `, `; `Some("")` for one that was not ASCII) is `*` or
/// `parent_origin`. A value that is neither (empty, non-ASCII, not a URL) allows nothing.
fn allows_csp_from(value: Option<&str>, parent_origin: &str) -> bool {
    let Some(value) = value else { return false };
    let value = value.trim_matches(|c: char| c.is_ascii_whitespace());
    if value == "*" {
        return true;
    }
    let Ok(url) = url::Url::parse(value) else { return false };
    let origin = url.origin();
    origin.is_tuple() && parent_origin != "null" && origin.ascii_serialization() == parent_origin
}

/// What a navigation's commit does under `verdict`: a blocked response fails with
/// [`BLOCKED_ERROR`]; an accepted required policy is the one its document enforces besides its
/// own (`Ok(Some)`).
pub(crate) fn apply(verdict: Option<Verdict>) -> Result<Option<String>, String> {
    match verdict {
        Some(Verdict::Block) => Err(BLOCKED_ERROR.to_owned()),
        Some(Verdict::Enforce(policy)) => Ok(Some(policy)),
        Some(Verdict::Allow) | None => Ok(None),
    }
}

/// The CSP core's form of an accepted required policy: delivered by header and enforced (it never
/// reports: a required policy has no `report-uri` or `report-to`).
fn enforced_policy(policy: &str) -> pyro_dom::CspPolicy {
    let policy =
        pyro_net::csp::Policy::parse(policy, pyro_net::csp::PolicySource::Header, pyro_net::csp::Disposition::Enforce);
    pyro_dom::CspPolicy { text: policy.text.into(), report_only: false, meta: false }
}

/// `policies` (a document's CSP list) with the accepted required policy `policy` after them.
fn with_required(policies: &[pyro_dom::CspPolicy], policy: &str) -> Vec<pyro_dom::CspPolicy> {
    let mut all = policies.to_vec();
    all.push(enforced_policy(policy));
    all
}

impl Browser {
    /// Sets what the commit about to happen in the frame of `key` does with its required policy:
    /// `required`, the requirement the committed document keeps for its own frames
    /// (`DocumentState::committing_required_csp`, which `commit_document` takes), and `enforce`,
    /// the policy it accepted and enforces (`DocumentState::enforce_required_csp`, which
    /// `begin_document` takes). Set right before the commit, by every path that commits a
    /// document in a child frame.
    pub(crate) fn prepare_required_csp(&mut self, key: FrameKey, required: Option<String>, enforce: Option<String>) {
        if let Some(mut state) = self.doc_mut(key) {
            state.committing_required_csp = required;
            state.enforce_required_csp = enforce;
        }
    }
}

/// Adds `policy`, a required policy the document of `key` accepted, to that document's CSP list
/// as an enforced header policy after what it already holds: its own header policies and those
/// it inherited as a local-scheme document (nothing is replaced, and its own `<meta>` ones,
/// which come later, add on top). Called once the document is made and before its parser
/// starts. A page that bypasses CSP adds nothing.
pub(crate) fn enforce_on_document(browser: &Browser, key: FrameKey, policy: &str) {
    if browser.page(key.page).is_some_and(|p| p.bypass_csp) {
        return;
    }
    let Some(page_dom) = browser.doc(key).and_then(|d| d.main_frame.page_dom.clone()) else { return };
    let doc = page_dom.document();
    let mut dom = page_dom.dom.borrow_mut();
    let policies = with_required(dom.csp_policies(doc), policy);
    let _ = dom.inherit_csp_policies(doc, &policies);
    // A document with an enforced header-delivered policy hides its nonces.
    let _ = dom.set_header_csp(doc, true);
}

// ----- policies ------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Directive {
    BaseUri,
    BlockAllMixedContent,
    ChildSrc,
    ConnectSrc,
    DefaultSrc,
    FencedFrameSrc,
    FrameAncestors,
    FrameSrc,
    FontSrc,
    FormAction,
    ImgSrc,
    ManifestSrc,
    MediaSrc,
    ObjectSrc,
    ReportUri,
    RequireTrustedTypesFor,
    Sandbox,
    ScriptSrc,
    ScriptSrcAttr,
    ScriptSrcElem,
    StyleSrc,
    StyleSrcAttr,
    StyleSrcElem,
    TrustedTypes,
    UpgradeInsecureRequests,
    WorkerSrc,
    ReportTo,
}

impl Directive {
    const NAMES: [(&'static str, Directive); 27] = [
        ("base-uri", Directive::BaseUri),
        ("block-all-mixed-content", Directive::BlockAllMixedContent),
        ("child-src", Directive::ChildSrc),
        ("connect-src", Directive::ConnectSrc),
        ("default-src", Directive::DefaultSrc),
        ("fenced-frame-src", Directive::FencedFrameSrc),
        ("frame-ancestors", Directive::FrameAncestors),
        ("frame-src", Directive::FrameSrc),
        ("font-src", Directive::FontSrc),
        ("form-action", Directive::FormAction),
        ("img-src", Directive::ImgSrc),
        ("manifest-src", Directive::ManifestSrc),
        ("media-src", Directive::MediaSrc),
        ("object-src", Directive::ObjectSrc),
        ("report-uri", Directive::ReportUri),
        ("require-trusted-types-for", Directive::RequireTrustedTypesFor),
        ("sandbox", Directive::Sandbox),
        ("script-src", Directive::ScriptSrc),
        ("script-src-attr", Directive::ScriptSrcAttr),
        ("script-src-elem", Directive::ScriptSrcElem),
        ("style-src", Directive::StyleSrc),
        ("style-src-attr", Directive::StyleSrcAttr),
        ("style-src-elem", Directive::StyleSrcElem),
        ("trusted-types", Directive::TrustedTypes),
        ("upgrade-insecure-requests", Directive::UpgradeInsecureRequests),
        ("worker-src", Directive::WorkerSrc),
        ("report-to", Directive::ReportTo),
    ];

    fn from_name(name: &str) -> Option<Directive> {
        Self::NAMES.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, d)| *d)
    }

    fn is_source_list(self) -> bool {
        use Directive::*;
        matches!(
            self,
            BaseUri
                | ChildSrc
                | ConnectSrc
                | DefaultSrc
                | FencedFrameSrc
                | FontSrc
                | FormAction
                | FrameAncestors
                | FrameSrc
                | ImgSrc
                | ManifestSrc
                | MediaSrc
                | ObjectSrc
                | ScriptSrc
                | ScriptSrcAttr
                | ScriptSrcElem
                | StyleSrc
                | StyleSrcAttr
                | StyleSrcElem
                | WorkerSrc
        )
    }

    /// Chrome `CSPFallbackDirective`.
    fn fallback(self, original: Directive) -> Option<Directive> {
        use Directive::*;
        match self {
            ConnectSrc | FontSrc | ImgSrc | ManifestSrc | MediaSrc | ObjectSrc | ScriptSrc | StyleSrc => {
                Some(DefaultSrc)
            }
            ScriptSrcAttr | ScriptSrcElem => Some(ScriptSrc),
            StyleSrcAttr | StyleSrcElem => Some(StyleSrc),
            FencedFrameSrc => Some(FrameSrc),
            FrameSrc | WorkerSrc => Some(ChildSrc),
            ChildSrc => Some(if original == WorkerSrc { ScriptSrc } else { DefaultSrc }),
            _ => None,
        }
    }

    fn is_script(self) -> bool {
        use Directive::*;
        matches!(self, ScriptSrc | ScriptSrcAttr | ScriptSrcElem | DefaultSrc)
    }

    fn is_style(self) -> bool {
        use Directive::*;
        matches!(self, StyleSrc | StyleSrcAttr | StyleSrcElem | DefaultSrc)
    }
}

/// The directives subsumption compares (Chrome `Subsumes`).
const SUBSUMPTION_DIRECTIVES: [Directive; 19] = [
    Directive::ChildSrc,
    Directive::ConnectSrc,
    Directive::FontSrc,
    Directive::FrameSrc,
    Directive::ImgSrc,
    Directive::ManifestSrc,
    Directive::MediaSrc,
    Directive::ObjectSrc,
    Directive::ScriptSrc,
    Directive::ScriptSrcAttr,
    Directive::ScriptSrcElem,
    Directive::StyleSrc,
    Directive::StyleSrcAttr,
    Directive::StyleSrcElem,
    Directive::WorkerSrc,
    Directive::BaseUri,
    Directive::FrameAncestors,
    Directive::FormAction,
    Directive::FencedFrameSrc,
];

/// A source expression with a host, or a scheme-only one (`host` empty, not a wildcard). An empty
/// `scheme`: none was given (`self`'s applies).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Source {
    scheme: String,
    host: String,
    host_wildcard: bool,
    port: i32,
    port_wildcard: bool,
    path: String,
}

impl Source {
    fn new() -> Source {
        Source {
            scheme: String::new(),
            host: String::new(),
            host_wildcard: false,
            port: PORT_UNSPECIFIED,
            port_wildcard: false,
            path: String::new(),
        }
    }

    fn scheme_only(scheme: &str) -> Source {
        Source { scheme: scheme.to_owned(), ..Source::new() }
    }

    fn is_scheme_only(&self) -> bool {
        self.host.is_empty() && !self.host_wildcard
    }

    /// The host as a pattern another source's host is matched against (`*.` for a wildcard).
    fn host_pattern(&self) -> String {
        if self.host_wildcard {
            format!("*.{}", self.host)
        } else {
            self.host.clone()
        }
    }
}

#[derive(Clone, Debug, Default)]
struct SourceList {
    sources: Vec<Source>,
    allow_self: bool,
    allow_star: bool,
    allow_inline: bool,
    allow_eval: bool,
    allow_dynamic: bool,
    allow_unsafe_hashes: bool,
    nonces: Vec<String>,
    /// (algorithm, base64url digest without padding).
    hashes: Vec<(u8, String)>,
}

#[derive(Clone, Debug, Default)]
struct TrustedTypes {
    allow_any: bool,
    allow_duplicates: bool,
    list: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct Policy {
    /// The source-list directives.
    directives: Vec<(Directive, SourceList)>,
    /// Every recognized directive name, in order (the first of a name wins).
    raw: Vec<Directive>,
    report_only: bool,
    trusted_types: Option<TrustedTypes>,
    require_trusted_types: bool,
}

impl Policy {
    /// Chrome `AddContentSecurityPolicyFromHeader` for one policy.
    fn parse(text: &str, report_only: bool) -> Policy {
        let mut out = Policy { report_only, ..Policy::default() };
        for token in text.split(';') {
            let token = trim(token);
            if token.is_empty() {
                continue;
            }
            let (name, value) = match token.find(is_csp_whitespace) {
                Some(at) => (&token[..at], trim(&token[at + 1..])),
                None => (token, ""),
            };
            if !is_directive_name(name) {
                continue;
            }
            let Some(directive) = Directive::from_name(name) else { continue };
            if out.raw.contains(&directive) {
                continue;
            }
            out.raw.push(directive);
            if !is_directive_value(value) {
                continue;
            }
            if directive.is_source_list() {
                out.directives.push((directive, parse_source_list(directive, value)));
            } else if directive == Directive::RequireTrustedTypesFor {
                out.require_trusted_types = value.split(is_csp_whitespace).any(|e| e == "'script'");
            } else if directive == Directive::TrustedTypes {
                out.trusted_types = Some(parse_trusted_types(value));
            }
        }
        out
    }

    /// Chrome `GetSourceList`: `directive`'s list, or that of the first directive of its fallback
    /// chain the policy has, with that directive.
    fn source_list(&self, directive: Directive) -> Option<(Directive, &SourceList)> {
        let mut current = Some(directive);
        while let Some(d) = current {
            if let Some((_, list)) = self.directives.iter().find(|(name, _)| *name == d) {
                return Some((d, list));
            }
            current = d.fallback(directive);
        }
        None
    }
}

fn parse_trusted_types(value: &str) -> TrustedTypes {
    let mut out = TrustedTypes::default();
    let pieces: Vec<&str> = value.split(is_csp_whitespace).filter(|p| !p.is_empty()).collect();
    if pieces.len() == 1 && pieces[0] == "'none'" {
        return out;
    }
    for expression in pieces {
        if expression == "*" {
            out.allow_any = true;
        } else if expression.eq_ignore_ascii_case("'allow-duplicates'") {
            out.allow_duplicates = true;
        } else if expression.eq_ignore_ascii_case("'none'") {
        } else if expression.bytes().all(|b| b.is_ascii_alphanumeric() || b"-#=_/@.%".contains(&b)) {
            out.list.push(expression.to_owned());
        }
    }
    out
}

/// Chrome `ParseSourceList`.
fn parse_source_list(directive: Directive, value: &str) -> SourceList {
    let value = trim(value);
    let mut list = SourceList::default();
    if value.eq_ignore_ascii_case("'none'") {
        return list;
    }
    for expression in value.split(is_csp_whitespace).filter(|e| !e.is_empty()) {
        if expression.eq_ignore_ascii_case("'none'") {
            continue;
        }
        if expression.eq_ignore_ascii_case("'self'") {
            list.allow_self = true;
            continue;
        }
        if expression == "*" {
            list.allow_star = true;
            continue;
        }
        if let Some(source) = parse_source(expression) {
            list.sources.push(source);
            continue;
        }
        if directive == Directive::FrameAncestors {
            continue;
        }
        if expression.eq_ignore_ascii_case("'unsafe-inline'") {
            list.allow_inline = true;
        } else if expression.eq_ignore_ascii_case("'unsafe-eval'") {
            list.allow_eval = true;
        } else if expression.eq_ignore_ascii_case("'strict-dynamic'") {
            list.allow_dynamic = true;
        } else if expression.eq_ignore_ascii_case("'unsafe-hashes'") {
            list.allow_unsafe_hashes = true;
        } else if let Some(nonce) = parse_nonce(expression) {
            list.nonces.push(nonce);
        } else if let Some(hash) = parse_hash(expression) {
            list.hashes.push(hash);
        }
        // Other keywords ('wasm-unsafe-eval', 'report-sample', ...) and invalid expressions take
        // no part in subsumption.
    }
    list
}

/// Chrome `ParseSource`: `scheme:`, or `[scheme://]host[:port][/path]`.
fn parse_source(expression: &str) -> Option<Source> {
    let mut source = Source::new();
    let mut expression = expression;
    let separator = |c: char| c == ':' || c == '/';
    let mut position = expression.find(separator);
    if let Some(at) = position.filter(|&at| expression.as_bytes()[at] == b':') {
        if at + 1 == expression.len() {
            source.scheme = parse_scheme(&expression[..at])?;
            return Some(source);
        }
        if expression.as_bytes()[at + 1] == b'/' {
            if at + 2 >= expression.len() || expression.as_bytes()[at + 2] != b'/' {
                return None;
            }
            source.scheme = parse_scheme(&expression[..at])?;
            expression = &expression[at + 3..];
            position = expression.find(separator);
        }
    }
    parse_host(&expression[..position.unwrap_or(expression.len())], &mut source)?;
    let Some(at) = position else { return Some(source) };
    expression = &expression[at..];
    if let Some(rest) = expression.strip_prefix(':') {
        let end = rest.find('/');
        parse_port(&rest[..end.unwrap_or(rest.len())], &mut source)?;
        let Some(end) = end else { return Some(source) };
        expression = &rest[end..];
    }
    if expression.is_empty() {
        return Some(source);
    }
    // A query or fragment is ignored.
    let end = expression.find(['?', '#']).unwrap_or(expression.len());
    let path = &expression[..end];
    if !path.starts_with('/') {
        return None;
    }
    source.path = percent_decode(path);
    Some(source)
}

fn parse_scheme(scheme: &str) -> Option<String> {
    let mut bytes = scheme.bytes();
    let first = bytes.next()?;
    if !first.is_ascii_alphabetic() || !bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')) {
        return None;
    }
    Some(scheme.to_ascii_lowercase())
}

fn parse_host(host: &str, source: &mut Source) -> Option<()> {
    if host.is_empty() {
        return None;
    }
    let mut host = host;
    if host.starts_with('*') {
        if host.len() == 1 {
            source.host_wildcard = true;
            return Some(());
        }
        if host.as_bytes()[1] != b'.' {
            return None;
        }
        source.host_wildcard = true;
        host = &host[2..];
    }
    if host.is_empty() {
        return None;
    }
    let pieces: Vec<&str> = host.split('.').collect();
    for (i, piece) in pieces.iter().enumerate() {
        // Only a trailing dot is allowed.
        let label = piece.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
        if (piece.is_empty() && i + 1 < pieces.len()) || !label {
            return None;
        }
    }
    source.host = host.to_ascii_lowercase();
    Some(())
}

fn parse_port(port: &str, source: &mut Source) -> Option<()> {
    if port.is_empty() {
        return None;
    }
    if port == "*" {
        source.port_wildcard = true;
        return Some(());
    }
    if !port.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    source.port = port.parse().ok()?;
    Some(())
}

/// `%XX` escapes decoded: as UTF-8 when the result is, else one character per byte.
fn percent_decode(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    match String::from_utf8(out) {
        Ok(s) => s,
        Err(e) => e.into_bytes().into_iter().map(char::from).collect(),
    }
}

fn is_base64(value: &str) -> bool {
    let body = value.trim_end_matches('=');
    !body.is_empty()
        && value.len() - body.len() <= 2
        && body.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'_' | b'/'))
}

fn parse_nonce(expression: &str) -> Option<String> {
    let prefix = expression.get(..7)?;
    if expression.len() < 8 || !prefix.eq_ignore_ascii_case("'nonce-") || !expression.ends_with('\'') {
        return None;
    }
    let nonce = &expression[7..expression.len() - 1];
    is_base64(nonce).then(|| nonce.to_owned())
}

fn parse_hash(expression: &str) -> Option<(u8, String)> {
    let inner = expression.strip_prefix('\'')?.strip_suffix('\'')?;
    const PREFIXES: [(&str, u8); 7] = [
        ("sha256-", 1),
        ("sha384-", 2),
        ("sha512-", 3),
        ("sha-256-", 1),
        ("sha-384-", 2),
        ("sha-512-", 3),
        ("ed25519-", 4),
    ];
    let (algorithm, digest) = PREFIXES.iter().find_map(|(prefix, algorithm)| {
        let head = inner.get(..prefix.len())?;
        head.eq_ignore_ascii_case(prefix).then(|| (*algorithm, &inner[prefix.len()..]))
    })?;
    if !is_base64(digest) {
        return None;
    }
    // base64 and base64url alike; padding ignored.
    let normalized: String =
        digest.trim_end_matches('=').chars().map(|c| match c { '+' => '-', '/' => '_', c => c }).collect();
    if normalized.len() % 4 == 1 {
        return None;
    }
    Some((algorithm, normalized))
}

/// Chrome `ComputeSelfOrigin`: `url`'s scheme, host and (effective) port.
fn self_origin(url: &str) -> Source {
    match url::Url::parse(url) {
        Ok(url) if url.scheme() == "file" => Source::scheme_only("file"),
        Ok(url) => Source {
            scheme: url.scheme().to_owned(),
            host: url.host_str().unwrap_or("").to_owned(),
            port: url.port_or_known_default().map_or(PORT_UNSPECIFIED, i32::from),
            ..Source::new()
        },
        Err(_) => Source::new(),
    }
}

// ----- subsumption ---------------------------------------------------------------------------

/// Chrome `Subsumes`: whether the policies `b` (a response's, whose `self` is `origin_b`) are at
/// least as strict as `a`.
fn subsumes(a: &Policy, b: &[Policy], origin_b: &Source) -> bool {
    if a.report_only {
        return true;
    }
    let a_trusted_types = CHECK_TRUSTED_TYPES && (a.trusted_types.is_some() || a.require_trusted_types);
    if a.directives.is_empty() && !a_trusted_types {
        return true;
    }
    if b.is_empty() {
        return false;
    }
    if CHECK_TRUSTED_TYPES {
        if a.require_trusted_types && !b.iter().any(|p| !p.report_only && p.require_trusted_types) {
            return false;
        }
        if let Some(required) = &a.trusted_types {
            match combined_trusted_types(b) {
                Some(combined) if trusted_types_match(required, &combined) => {}
                _ => return false,
            }
        }
    }
    SUBSUMPTION_DIRECTIVES.iter().all(|&directive| {
        let Some((effective, required)) = a.source_list(directive) else { return true };
        let returned: Vec<&SourceList> = b
            .iter()
            .filter(|p| !p.report_only)
            .filter_map(|p| p.source_list(directive).map(|(_, list)| list))
            .collect();
        source_list_subsumes(required, &returned, effective, origin_b)
    })
}

fn normalized_allow_duplicates(tt: &TrustedTypes) -> bool {
    tt.allow_duplicates && (tt.allow_any || !tt.list.is_empty())
}

/// Chrome `CombinedTrustedTypes`.
fn combined_trusted_types(policies: &[Policy]) -> Option<TrustedTypes> {
    let mut found = false;
    let mut allow_any = true;
    let mut allow_duplicates = true;
    let mut restricted = false;
    let mut list: BTreeSet<&str> = BTreeSet::new();
    for tt in policies.iter().filter(|p| !p.report_only).filter_map(|p| p.trusted_types.as_ref()) {
        found = true;
        allow_any &= tt.allow_any;
        allow_duplicates &= tt.allow_duplicates;
        if !tt.allow_any {
            if !restricted {
                list.extend(tt.list.iter().map(String::as_str));
            } else {
                list.retain(|name| tt.list.iter().any(|n| n.as_str() == *name));
            }
            restricted = true;
        }
    }
    if !found {
        return None;
    }
    let list: Vec<String> = if allow_any { Vec::new() } else { list.into_iter().map(str::to_owned).collect() };
    let allow_duplicates = allow_duplicates && (allow_any || !list.is_empty());
    Some(TrustedTypes { allow_any, allow_duplicates, list })
}

/// Chrome `TrustedTypesNormalizedExactMatch`.
fn trusted_types_match(required: &TrustedTypes, combined: &TrustedTypes) -> bool {
    if required.allow_any != combined.allow_any
        || normalized_allow_duplicates(required) != normalized_allow_duplicates(combined)
    {
        return false;
    }
    if required.allow_any {
        return true;
    }
    let a: BTreeSet<&str> = required.list.iter().map(String::as_str).collect();
    let b: BTreeSet<&str> = combined.list.iter().map(String::as_str).collect();
    a == b
}

fn allow_all_inline(directive: Directive, list: &SourceList) -> bool {
    list.allow_inline
        && list.hashes.is_empty()
        && list.nonces.is_empty()
        && (!directive.is_script() || !list.allow_dynamic)
}

/// Chrome `CSPSourceListSubsumes`: whether the intersection of the lists `b` is subsumed by `a`
/// (for `directive`, the directive `a` was found under).
fn source_list_subsumes(a: &SourceList, b: &[&SourceList], directive: Directive, origin_b: &Source) -> bool {
    let Some((first, rest)) = b.split_first() else { return false };
    let mut allow_inline_b = first.allow_inline;
    let mut allow_eval_b = first.allow_eval;
    let mut allow_dynamic_b = first.allow_dynamic;
    let mut allow_unsafe_hashes_b = first.allow_unsafe_hashes;
    let mut hash_or_nonce_b = !first.nonces.is_empty() || !first.hashes.is_empty();
    let mut nonces_b: BTreeSet<&str> = first.nonces.iter().map(String::as_str).collect();
    let mut hashes_b: BTreeSet<&(u8, String)> = first.hashes.iter().collect();
    let mut normalized_b = expand_scheme_star_and_self(first, origin_b);
    for item in rest {
        // 'unsafe-inline' is ignored when hashes or nonces are present, or 'strict-dynamic'.
        allow_inline_b = allow_inline_b && item.allow_inline;
        allow_eval_b = allow_eval_b && item.allow_eval;
        allow_dynamic_b = allow_dynamic_b && item.allow_dynamic;
        allow_unsafe_hashes_b = allow_unsafe_hashes_b && item.allow_unsafe_hashes;
        hash_or_nonce_b = hash_or_nonce_b && (!item.nonces.is_empty() || !item.hashes.is_empty());
        nonces_b.retain(|nonce| item.nonces.iter().any(|n| n.as_str() == *nonce));
        hashes_b.retain(|hash| item.hashes.iter().any(|h| h == *hash));
        normalized_b = intersect_sources(item, &normalized_b, origin_b);
    }

    // A nonce `b` enforces needs some nonce in `a`, not the same one.
    if !nonces_b.is_empty() && a.nonces.is_empty() {
        return false;
    }
    // Every hash `b` enforces must be in `a`.
    if hashes_b.iter().any(|hash| !a.hashes.iter().any(|h| h == *hash)) {
        return false;
    }

    if directive.is_script() || directive.is_style() {
        if !a.allow_eval && allow_eval_b {
            return false;
        }
        if !a.allow_unsafe_hashes && allow_unsafe_hashes_b {
            return false;
        }
        let allow_all_inline_b = allow_inline_b && !hash_or_nonce_b && (!directive.is_script() || !allow_dynamic_b);
        if !allow_all_inline(directive, a) && allow_all_inline_b {
            return false;
        }
    }

    if directive.is_script() && (a.allow_dynamic || allow_dynamic_b) {
        // 'strict-dynamic' ignores the sources: `b` must allow it too, or allow no source.
        if !a.allow_dynamic {
            return false;
        }
        return allow_dynamic_b || normalized_b.is_empty();
    }

    // `self` in the embedder's policy is the embedded document's origin.
    let normalized_a = expand_scheme_star_and_self(a, origin_b);
    url_source_list_subsumes(&normalized_a, &normalized_b)
}

/// Chrome `ExpandSchemeStarAndSelf`: the sources of `list` with `self`'s scheme for those without
/// one, `*` as `ftp:`, `ws:`, `http:` and `self`'s scheme, and `'self'` as `self`.
fn expand_scheme_star_and_self(list: &SourceList, origin: &Source) -> Vec<Source> {
    let mut out: Vec<Source> = list
        .sources
        .iter()
        .map(|source| {
            let mut source = source.clone();
            if source.scheme.is_empty() && !origin.scheme.is_empty() {
                source.scheme = origin.scheme.clone();
            }
            source
        })
        .collect();
    if list.allow_star {
        for scheme in ["ftp", "ws", "http"] {
            out.push(Source::scheme_only(scheme));
        }
        if !origin.scheme.is_empty() {
            out.push(Source::scheme_only(&origin.scheme));
        }
    }
    if list.allow_self && !origin.scheme.is_empty() && !origin.host.is_empty() {
        out.push(origin.clone());
    }
    out
}

fn add_source_schemes(set: &mut BTreeSet<String>, scheme: &str) {
    set.insert(scheme.to_owned());
    match scheme {
        "http" => {
            set.insert("https".to_owned());
        }
        "ws" => {
            set.insert("wss".to_owned());
        }
        _ => {}
    }
}

/// Chrome `IntersectSchemesOnly`.
fn intersect_schemes_only(list_a: &[Source], list_b: &[Source]) -> BTreeSet<String> {
    let mut schemes_a = BTreeSet::new();
    for source in list_a.iter().filter(|s| s.is_scheme_only()) {
        add_source_schemes(&mut schemes_a, &source.scheme);
    }
    let mut out = BTreeSet::new();
    for source in list_b.iter().filter(|s| s.is_scheme_only()) {
        if schemes_a.contains(&source.scheme) {
            add_source_schemes(&mut out, &source.scheme);
        } else if source.scheme == "http" && schemes_a.contains("https") {
            out.insert("https".to_owned());
        } else if source.scheme == "ws" && schemes_a.contains("wss") {
            out.insert("wss".to_owned());
        }
    }
    out
}

/// Chrome `IntersectSources`: what both `list_a` and the (normalized) sources `list_b` allow.
fn intersect_sources(list_a: &SourceList, list_b: &[Source], origin: &Source) -> Vec<Source> {
    let schemes = intersect_schemes_only(&list_a.sources, list_b);
    let mut out = Vec::new();
    for scheme in &schemes {
        // No secure variant when the insecure scheme is there.
        if (scheme != "https" || !schemes.contains("http")) && (scheme != "wss" || !schemes.contains("ws")) {
            out.push(Source::scheme_only(scheme));
        }
    }
    for source_a in expand_scheme_star_and_self(list_a, origin) {
        if schemes.contains(&source_a.scheme) {
            continue;
        }
        for source_b in list_b {
            // Subsumed by the matching scheme source.
            if schemes.contains(&source_b.scheme) {
                continue;
            }
            if let Some(source) = sources_intersect(&source_a, source_b) {
                out.push(source);
            }
        }
    }
    out
}

/// Whether `a` matches `b` as a scheme: the same, or `b` its secure variant.
fn match_scheme(a: &str, b: &str) -> bool {
    a == b || (a == "http" && b == "https") || (a == "ws" && b == "wss")
}

fn default_port(scheme: &str) -> i32 {
    match scheme {
        "http" | "ws" => 80,
        "https" | "wss" => 443,
        "ftp" => 21,
        _ => PORT_UNSPECIFIED,
    }
}

/// Chrome `SourceAllowHost`.
fn allow_host(source: &Source, host: &str) -> bool {
    if source.host_wildcard {
        if source.host.is_empty() {
            return true;
        }
        let Some(prefix) = host.strip_suffix(source.host.as_str()) else { return false };
        prefix.ends_with('.')
    } else {
        host == source.host
    }
}

/// Chrome `SourceAllowPort` (whether it matches at all).
fn allow_port(source: &Source, port: i32, scheme: &str) -> bool {
    if source.port_wildcard || source.port == port {
        return true;
    }
    if source.port == PORT_UNSPECIFIED && default_port(scheme) == port {
        return true;
    }
    if port == PORT_UNSPECIFIED && source.port == default_port(scheme) {
        return true;
    }
    let source_port = if source.port == PORT_UNSPECIFIED { default_port(&source.scheme) } else { source.port };
    let port = if port == PORT_UNSPECIFIED { default_port(scheme) } else { port };
    source_port == 80 && port == 443
}

/// Chrome `SourceAllowPath`.
fn allow_path(source: &Source, path: &str) -> bool {
    if source.path.is_empty() || (source.path == "/" && path.is_empty()) {
        return true;
    }
    if source.path.ends_with('/') {
        return path.starts_with(source.path.as_str());
    }
    source.path == path
}

/// Chrome `CSPSourcesIntersect`.
fn sources_intersect(a: &Source, b: &Source) -> Option<Source> {
    let scheme = if match_scheme(&a.scheme, &b.scheme) {
        b.scheme.clone()
    } else if match_scheme(&b.scheme, &a.scheme) {
        a.scheme.clone()
    } else {
        return None;
    };
    if a.is_scheme_only() {
        return Some(Source { scheme, ..b.clone() });
    }
    if b.is_scheme_only() {
        return Some(Source { scheme, ..a.clone() });
    }
    let mut out = Source { scheme, ..Source::new() };
    if allow_host(a, &b.host_pattern()) {
        out.host = b.host.clone();
        out.host_wildcard = b.host_wildcard;
    } else if allow_host(b, &a.host_pattern()) {
        out.host = a.host.clone();
        out.host_wildcard = a.host_wildcard;
    } else {
        return None;
    }
    if b.port_wildcard {
        out.port = a.port;
        out.port_wildcard = a.port_wildcard;
    } else if a.port_wildcard {
        out.port = b.port;
    } else if allow_port(a, b.port, &b.scheme) && !(a.port != PORT_UNSPECIFIED && b.port == PORT_UNSPECIFIED) {
        // An omitted port in `b` takes `a`'s explicit one, which is stricter.
        out.port = b.port;
    } else if allow_port(b, a.port, &a.scheme) {
        out.port = a.port;
    } else {
        return None;
    }
    if allow_path(a, &b.path) {
        out.path = b.path.clone();
    } else if allow_path(b, &a.path) {
        out.path = a.path.clone();
    } else {
        return None;
    }
    Some(out)
}

/// Chrome `CSPSourceSubsumes`.
fn source_subsumes(a: &Source, b: &Source) -> bool {
    if !match_scheme(&a.scheme, &b.scheme) {
        return false;
    }
    if a.is_scheme_only() {
        return true;
    }
    if b.is_scheme_only() {
        return false;
    }
    if !allow_host(a, &b.host_pattern()) {
        return false;
    }
    if b.port_wildcard && !a.port_wildcard {
        return false;
    }
    allow_port(a, b.port, &b.scheme) && allow_path(a, &b.path)
}

/// Chrome `UrlSourceListSubsumes`: no sources is `'none'`; else every source of `b` is subsumed
/// by one of `a`.
fn url_source_list_subsumes(a: &[Source], b: &[Source]) -> bool {
    if a.is_empty() || b.is_empty() {
        return b.is_empty();
    }
    b.iter().all(|source_b| a.iter().any(|source_a| source_subsumes(source_a, source_b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: &str = "http://xn--n8j6ds53lwwkrqhv28a.web-platform.test:8000/support/x.py";

    /// Whether a response with the policies `returned` loads in a frame that requires `required`.
    fn loads(required: &str, returned: &[&str]) -> bool {
        let required = Policy::parse(required, false);
        let returned: Vec<Policy> = returned.iter().map(|p| Policy::parse(p, false)).collect();
        subsumes(&required, &returned, &self_origin(ORIGIN))
    }

    #[test]
    fn attribute_grammar() {
        assert!(valid_attribute("script-src 'unsafe-inline';"));
        assert!(valid_attribute("completely wrong csp"));
        assert!(valid_attribute("script-src 'unsafe-inline' 127.0.0.1:8000/path?query=string"));
        assert!(!valid_attribute("script-src 'unsafe-inline' 'self', object-src 'none'"));
        assert!(!valid_attribute("media-src%20127.0.0.1%3A8000"));
        assert!(!valid_attribute("script-src 'unsafe-inline' &#x31;&#x32;"));
        assert!(!valid_attribute("style-src\r\n'unsafe-inline'"));
        assert!(!valid_attribute("default-src http://example.com; i//nvalid-policy-name http://example.com"));
        let long = format!("style-src {}", vec!["a"; 2044].join(" "));
        assert!(!valid_attribute(&long));
    }

    #[test]
    fn required_policy_cascades() {
        let url = "http://web-platform.test:8000/x";
        let parent = "script-src 'unsafe-inline'; style-src 'self';";
        assert_eq!(required_policy(Some("script-src 'unsafe-inline';"), Some(parent), url).as_deref(), Some(parent));
        assert_eq!(required_policy(None, Some(parent), url).as_deref(), Some(parent));
        let stricter = "script-src 'unsafe-inline'; style-src 'self';";
        let loose = "script-src 'unsafe-inline';";
        assert_eq!(required_policy(Some(stricter), Some(loose), url).as_deref(), Some(stricter));
        assert_eq!(required_policy(Some("script-src 'unsafe-inline'; report-uri x"), None, url), None);
        assert_eq!(required_policy(Some(""), None, url), None);
    }

    #[test]
    fn subsumption() {
        let both = "style-src 'none'; script-src 'unsafe-inline'";
        assert!(loads(both, &[both]));
        assert!(!loads("style-src 'none';", &[]));
        assert!(loads("frame-src http://c.com:443 http://b.com", &["frame-src http://b.com:80 http://c.com:443"]));
        assert!(!loads("frame-src http://*.a.com", &["frame-src http://a.com"]));
        assert!(loads("frame-src http://b.com:80", &["child-src https://b.com:443"]));
        assert!(!loads(
            "style-src http://*.example.com:*",
            &["style-src http://*.com:*", "style-src http://*.com http://*.example.com:*"]
        ));
        assert!(loads(
            "style-src http://*.example.com",
            &["style-src http://*.example.com:*", "style-src http://*.com"]
        ));
        assert!(!loads("img-src *", &["img-src data: blob:", "img-src data://a.com ws://b.com ftp://c.com"]));
        assert!(loads("img-src http: ftp: ws:", &["img-src *", "img-src https: http: ftp: ws: wss:"]));
        assert!(!loads("style-src 'none'", &["style-src 'nonce-def'", "style-src 'nonce-def' 'nonce-xyz'"]));
        assert!(loads("style-src 'none'", &["style-src 'nonce-def'", "style-src 'nonce-xyz'"]));
        assert!(!loads("default-src 'none'", &["img-src 'none'", "script-src 'unsafe-inline'"]));
        assert!(loads(
            "img-src 'self' http://b.com:*",
            &["img-src http://xn--n8j6ds53lwwkrqhv28a.web-platform.test:8000/"]
        ));
        assert!(!loads(
            "img-src https://xn--n8j6ds53lwwkrqhv28a.web-platform.test:8000/ http://b.com:*",
            &["img-src 'self'"]
        ));
        assert!(loads(
            "script-src 'strict-dynamic' 'nonce-abc'",
            &["script-src http://example.com 'strict-dynamic' 'nonce-abc'"]
        ));
        assert!(!loads("script-src 'nonce-abc'", &["script-src 'strict-dynamic' 'nonce-abc'"]));
        assert!(!loads("img-src http://b.com/page1.html http://b.com/page2.html", &["img-src http://b.com/"]));
        assert!(loads("img-src http://b.com/", &["img-src http://b.com"]));
        assert!(loads("frame-sr 'none'", &["frame-src http://a.com"]));
    }

    fn resource(url: &str, header_csp: Option<&str>, allow_csp_from: Option<&str>) -> crate::loader::Resource {
        crate::loader::Resource {
            url: url.to_owned(),
            status: 200,
            mime: "text/html".into(),
            charset: None,
            body: Default::default(),
            immutable_local: false,
            content_disposition: None,
            header_csp: header_csp.map(str::to_owned),
            allow_csp_from: allow_csp_from.map(str::to_owned),
            refresh: None,
            content_language: None,
            last_modified: None,
            reporting_endpoints: None,
            connection_allowlist: None,
            link_fetches: Vec::new(),
            integrity_headers: None,
            dictionary_links: Vec::new(),
            speculation_rules: Vec::new(),
            no_vary_search: None,
            referrer_policy: None,
            origin_agent_cluster: false, force_load_at_top: false,
            embedder_policy: pyro_net::EmbedderPolicy::UnsafeNone,
            coop: Vec::new(),
            sent_referrer: None,
            timing: None,
            partial: None,
            nosniff: false,
            opaque: false,
        }
    }

    #[test]
    fn response_decisions() {
        let required = Requirement { policy: "script-src 'none'".into(), parent_origin: "http://site.test".into() };
        let enforce = Verdict::Enforce("script-src 'none'".into());
        // Local schemes accept it.
        for url in ["about:srcdoc", "about:blank", "data:text/html,x", "blob:http://site.test/1"] {
            assert_eq!(decide(&required, &resource(url, None, None)), enforce, "{url}");
        }
        // Allow-CSP-From: `*` or the parent's origin; nothing else.
        let url = "http://other.test/x";
        assert_eq!(decide(&required, &resource(url, None, Some("*"))), enforce);
        assert_eq!(decide(&required, &resource(url, None, Some(" http://site.test "))), enforce);
        assert_eq!(decide(&required, &resource(url, None, Some("http://site.test:81"))), Verdict::Block);
        assert_eq!(decide(&required, &resource(url, None, Some(""))), Verdict::Block);
        assert_eq!(decide(&required, &resource(url, None, None)), Verdict::Block);
        let opaque = Requirement { parent_origin: "null".into(), ..required.clone() };
        assert_eq!(decide(&opaque, &resource(url, None, Some("null"))), Verdict::Block);
        // Subsumption over the policies the document gets: enforced ones count, report-only not.
        assert_eq!(decide(&required, &resource(url, Some("script-src 'none'\n"), None)), Verdict::Allow);
        assert_eq!(decide(&required, &resource(url, Some("img-src *,script-src 'none'\n"), None)), Verdict::Allow);
        assert_eq!(decide(&required, &resource(url, Some("script-src 'none'"), None)), Verdict::Allow);
        assert_eq!(decide(&required, &resource(url, Some("\nscript-src 'none'"), None)), Verdict::Block);
        assert_eq!(decide(&required, &resource(url, Some("script-src *\n"), None)), Verdict::Block);
    }

    #[test]
    fn an_accepted_policy_is_enforced_by_the_csp_core() {
        assert_eq!(apply(Some(Verdict::Block)).err().as_deref(), Some(BLOCKED_ERROR));
        assert_eq!(apply(Some(Verdict::Allow)), Ok(None));
        assert_eq!(apply(None), Ok(None));
        let enforce = apply(Some(Verdict::Enforce(" script-src 'nonce-123' ".into()))).unwrap().unwrap();
        // A document's own (or inherited) list: an enforced and a report-only policy. The
        // required policy goes after them, an enforced header policy; nothing is replaced.
        let own = [
            pyro_dom::CspPolicy { text: "img-src *".into(), report_only: false, meta: false },
            pyro_dom::CspPolicy { text: "style-src 'none'".into(), report_only: true, meta: false },
        ];
        let all = with_required(&own, &enforce);
        assert_eq!(all.len(), 3);
        assert_eq!(&*all[2].text, "script-src 'nonce-123'");
        assert!(!all[2].report_only && !all[2].meta);
        // The PolicyList built from it as `pyro_webapi::csp::policy_list` builds a document's.
        let url = url::Url::parse("http://other.test/x").unwrap();
        let mut list = pyro_net::csp::PolicyList::new(Some(&url));
        use pyro_net::csp::{Disposition, PolicySource};
        for policy in &all {
            let disposition = if policy.report_only { Disposition::Report } else { Disposition::Enforce };
            list.add(&policy.text, PolicySource::Header, disposition);
        }
        assert_eq!(list.policies.len(), 3);
        use pyro_net::csp::InlineKind;
        assert!(list.inline_check(InlineKind::Script, "", "navigator.userAgent").blocked);
        assert!(list.inline_check(InlineKind::Script, "123", "navigator.userAgent").allowed());
    }
}
