//! The Permissions Policy engine (https://w3c.github.io/webappsec-permissions-policy/): which
//! policy-controlled features a document may use, and the one question every gate asks,
//! [`feature_allowed`].
//!
//! A document's policy has two parts (Permissions Policy "is feature enabled in document for
//! origin"):
//! - the *inherited policy*: for a child frame, whether its parent document's declared policy
//!   allows the child's origin, and its container's policy (the iframe's `allow` attribute, or the
//!   legacy `allowfullscreen` / `allowpaymentrequest` attributes) allows it (else the feature's
//!   default allowlist). pyro-browser computes it when it makes the document ([`inherit`]);
//! - the *declared policy*: the `Permissions-Policy` response header (a structured-field
//!   dictionary, [`parse_header`]); `Permissions-Policy-Report-Only` declares a policy that is only
//!   reported. The loader hands both to [`PageDom::set_permissions_policy_headers`] when it commits
//!   the document.
//!
//! The legacy `Feature-Policy` header is not read: Chrome ignores it (WPT
//! permissions-policy/historical-feature-policy-header.html), and `document.featurePolicy` does not
//! exist. A gate that refuses (or that report-only would refuse) calls [`check_feature`], which
//! also queues the `permissions-policy-violation` report (`crate::reporting_store`) to the
//! `report-to` endpoint of the header's member for the feature, else `default`.

use std::collections::HashMap;
use std::rc::Rc;

use pyro_js::Scope;
use serde_json::{Map, Value as Json};

use crate::page::PageDom;
use crate::reporting_store::{queue_report, ReportInput};

/// A policy-controlled feature's default allowlist.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DefaultAllowlist {
    /// `*`: every origin.
    Star,
    /// `'self'`: the document's own origin; a child frame at another origin needs its container to
    /// delegate it.
    SelfOrigin,
    /// Enabled in a top-level document, disabled in a child frame unless its container's `allow`
    /// names the feature (Chrome's `unload`).
    Delegated,
}

/// The features pyro knows (those a gate or a test names), with their default allowlists as Chrome
/// has them. A feature not here is not policy-controlled: [`feature_allowed`] says yes.
const FEATURES: &[(&str, DefaultAllowlist)] = {
    use DefaultAllowlist::{Delegated, SelfOrigin, Star};
    &[
        ("accelerometer", SelfOrigin),
        ("ambient-light-sensor", SelfOrigin),
        ("autoplay", SelfOrigin),
        ("bluetooth", SelfOrigin),
        ("camera", SelfOrigin),
        // Client Hints: the low-entropy UA hints are open to every origin, the rest are the document's.
        ("ch-ua", Star),
        ("ch-ua-mobile", Star),
        ("ch-ua-platform", Star),
        ("ch-ua-arch", SelfOrigin),
        ("ch-ua-bitness", SelfOrigin),
        ("ch-ua-full-version", SelfOrigin),
        ("ch-ua-full-version-list", SelfOrigin),
        ("ch-ua-model", SelfOrigin),
        ("ch-ua-platform-version", SelfOrigin),
        ("ch-ua-wow64", SelfOrigin),
        ("ch-device-memory", SelfOrigin),
        ("ch-downlink", SelfOrigin),
        ("ch-dpr", SelfOrigin),
        ("ch-ect", SelfOrigin),
        ("ch-prefers-color-scheme", SelfOrigin),
        ("ch-prefers-reduced-motion", SelfOrigin),
        ("ch-prefers-reduced-transparency", SelfOrigin),
        ("ch-rtt", SelfOrigin),
        ("ch-viewport-height", SelfOrigin),
        ("ch-viewport-width", SelfOrigin),
        ("clipboard-read", SelfOrigin),
        ("clipboard-write", SelfOrigin),
        ("compute-pressure", SelfOrigin),
        ("cross-origin-isolated", SelfOrigin),
        ("deferred-fetch", SelfOrigin),
        ("display-capture", SelfOrigin),
        ("encrypted-media", SelfOrigin),
        ("focus-without-user-activation", Star),
        ("fullscreen", SelfOrigin),
        ("gamepad", SelfOrigin),
        ("geolocation", SelfOrigin),
        ("gyroscope", SelfOrigin),
        ("hid", SelfOrigin),
        ("idle-detection", SelfOrigin),
        ("keyboard-map", SelfOrigin),
        ("magnetometer", SelfOrigin),
        ("media-playback-while-not-visible", Star),
        ("microphone", SelfOrigin),
        ("midi", SelfOrigin),
        ("payment", SelfOrigin),
        ("picture-in-picture", Star),
        ("publickey-credentials-create", SelfOrigin),
        ("publickey-credentials-get", SelfOrigin),
        ("screen-wake-lock", SelfOrigin),
        ("serial", SelfOrigin),
        ("storage-access", Star),
        ("sync-xhr", Star),
        ("unload", Delegated),
        ("usb", SelfOrigin),
        ("web-share", SelfOrigin),
        ("webnn", SelfOrigin),
        ("xr-spatial-tracking", SelfOrigin),
    ]
};

fn feature(name: &str) -> Option<(&'static str, DefaultAllowlist)> {
    FEATURES.iter().copied().find(|(n, _)| *n == name)
}

/// An allowlist: `*`, `self`, and the origins (ASCII serializations) a header member names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Allowlist {
    star: bool,
    self_origin: bool,
    origins: Vec<String>,
}

impl Allowlist {
    /// Whether the allowlist matches `origin` (`None`: opaque, which only `*` and, for the document
    /// itself (`own`), `self` match) for a document at `doc_origin`.
    fn matches(&self, origin: Option<&str>, doc_origin: Option<&str>, own: bool) -> bool {
        if self.star || (self.self_origin && (own || (origin.is_some() && origin == doc_origin))) {
            return true;
        }
        origin.is_some_and(|o| self.origins.iter().any(|x| x == o))
    }
}

/// One member of a policy header.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Declared {
    allowlist: Allowlist,
    /// The member's `report-to` parameter: the endpoint its violations go to.
    report_to: Option<String>,
}

/// A document's permissions policy.
#[derive(Clone, Debug, Default)]
pub struct PermissionsPolicy {
    /// The document's origin (ASCII serialization); `None` when opaque.
    origin: Option<String>,
    /// Features the inherited policy disables.
    disabled: Vec<&'static str>,
    /// The `Permissions-Policy` header's members.
    declared: HashMap<String, Declared>,
    /// The `Permissions-Policy-Report-Only` header's members.
    report_only: HashMap<String, Declared>,
}

/// What a gate learned about a use of `feature`.
struct Verdict {
    allowed: bool,
    /// `Some((disposition, report-to))`: the use violates the enforced (`enforce`) or the
    /// report-only (`report`) policy.
    violation: Option<(&'static str, Option<String>)>,
}

impl PermissionsPolicy {
    /// The policy of a top-level document at `origin`: nothing inherited.
    pub fn top_level(origin: Option<String>) -> Self {
        Self { origin, ..Self::default() }
    }

    /// "Is feature enabled in document for origin" for the document's own origin.
    fn enabled(&self, name: &str) -> bool {
        let Some((name, _)) = feature(name) else { return true };
        if self.disabled.contains(&name) {
            return false;
        }
        self.declared.get(name).map_or(true, |d| d.allowlist.matches(self.origin.as_deref(), self.origin.as_deref(), true))
    }

    fn verdict(&self, name: &str) -> Verdict {
        if !self.enabled(name) {
            let report_to = self.declared.get(name).and_then(|d| d.report_to.clone());
            return Verdict { allowed: false, violation: Some(("enforce", report_to)) };
        }
        // Report-only: reported only for a use the enforced policy allows (Chrome).
        let violation = self
            .report_only
            .get(name)
            .filter(|d| !d.allowlist.matches(self.origin.as_deref(), self.origin.as_deref(), true))
            .map(|d| ("report", d.report_to.clone()));
        Verdict { allowed: true, violation }
    }
}

/// A frame container's policy inputs, as pyro-browser captured them when the navigation began.
pub struct Container<'a> {
    /// The `allowfullscreen` attribute (alias of `allow="fullscreen *"`).
    pub allowfullscreen: bool,
    /// The `allowpaymentrequest` attribute (alias of `allow="payment *"`).
    pub allowpaymentrequest: bool,
    pub allow: Option<&'a str>,
    /// The container document's origin, `None` when opaque.
    pub parent_origin: Option<&'a str>,
    /// Permissions Policy "declared origin" of the container: what `'src'` stands for.
    pub declared_origin: Option<&'a str>,
    /// The container is sandboxed without `allow-same-origin`: its documents have opaque origins,
    /// and `'src'` matches only one that was loaded from the `src` URL's origin
    /// ([`Container::sandbox_src_origin`] against [`Container::child_precursor`]), which Chrome
    /// compares through the opaque origin's precursor.
    pub sandboxed: bool,
    /// For a sandboxed container: the origin of its `src` URL (a `srcdoc` frame's: its parent's).
    pub sandbox_src_origin: Option<&'a str>,
    /// The origin of the URL the child document was loaded from, whatever its own (opaque) origin.
    pub child_precursor: Option<&'a str>,
}

/// Permissions Policy "define an inherited policy for feature in container at origin", for every
/// known feature: the policy a child document at `child_origin` starts with, in `container`, whose
/// document is `parent`.
pub fn inherit(parent: &PageDom, container: &Container<'_>, child_origin: Option<&str>) -> PermissionsPolicy {
    let parent_policy = parent.permissions_policy.borrow();
    let src_in_sandbox = container.sandboxed.then(|| {
        child_origin.is_none() && container.sandbox_src_origin.is_some() && container.sandbox_src_origin == container.child_precursor
    });
    let mut disabled = Vec::new();
    for &(name, default) in FEATURES {
        // The parent document's own inherited policy and, when its header declares the feature,
        // that allowlist for the child's origin ('self' is the parent's origin).
        let parent_allows = !parent_policy.disabled.contains(&name)
            && parent_policy
                .declared
                .get(name)
                .map_or(true, |d| d.allowlist.matches(child_origin, parent_policy.origin.as_deref(), false));
        let container_allows = match container_allows_in(
            name,
            container.allow,
            container.parent_origin,
            container.declared_origin,
            child_origin,
            src_in_sandbox,
        ) {
            Some(allowed) => allowed,
            None => match name {
                "fullscreen" if container.allowfullscreen => true,
                "payment" if container.allowpaymentrequest => true,
                _ => match default {
                    DefaultAllowlist::Star => true,
                    DefaultAllowlist::SelfOrigin => container.parent_origin.is_some() && container.parent_origin == child_origin,
                    DefaultAllowlist::Delegated => false,
                },
            },
        };
        if !(parent_allows && container_allows) {
            disabled.push(name);
        }
    }
    PermissionsPolicy { origin: child_origin.map(str::to_owned), disabled, ..PermissionsPolicy::default() }
}

/// A frame container's `allow` attribute on `feature` for a child document at `child_origin`:
/// `Some(allowed)` when the attribute declares the feature (an allowlist of `*`, `'self'` (the
/// parent's origin), `'src'` (the container's declared origin, also the default with no tokens),
/// `'none'` and origins), `None` when it does not mention it.
pub fn container_allows(
    feature: &str,
    allow: Option<&str>,
    parent_origin: Option<&str>,
    declared_origin: Option<&str>,
    child_origin: Option<&str>,
) -> Option<bool> {
    container_allows_in(feature, allow, parent_origin, declared_origin, child_origin, None)
}

fn container_allows_in(
    feature: &str,
    allow: Option<&str>,
    parent_origin: Option<&str>,
    declared_origin: Option<&str>,
    child_origin: Option<&str>,
    src_in_sandbox: Option<bool>,
) -> Option<bool> {
    let same = |a: Option<&str>| a.is_some() && a == child_origin;
    // `'src'`: the frame's document must be at the origin the container delegated to (its `src`
    // URL's), not wherever a redirect or a later navigation took it. In a sandboxed container
    // the caller decided: its documents' origins are opaque (`src_in_sandbox`).
    let src = || src_in_sandbox.unwrap_or_else(|| same(declared_origin));
    let declared = allow.and_then(|allow| {
        allow.split(';').find_map(|directive| {
            let mut tokens = directive.split_ascii_whitespace();
            (tokens.next()? == feature).then(|| tokens.map(str::to_owned).collect::<Vec<_>>())
        })
    })?;
    Some(match declared {
        list if list.is_empty() => src(),
        list => list.iter().any(|t| match t.as_str() {
            "*" => true,
            "'self'" => same(parent_origin),
            "'src'" => src(),
            "'none'" => false,
            origin => same(Some(origin.trim_end_matches('/'))),
        }),
    })
}

/// The `Permissions-Policy` header value `value` (a structured-field dictionary of feature to
/// allowlist: `camera=()`, `geolocation=(self "https://a.test")`, `fullscreen=*`,
/// `camera=self;report-to=endpoint`). A member that does not parse is skipped; `src` and
/// unknown tokens in an allowlist are ignored; a feature named twice keeps the last.
fn parse_header(value: &str) -> HashMap<String, Declared> {
    let mut out = HashMap::new();
    for member in split_outside_strings(value, ',') {
        let member = member.trim();
        let Some((key, rest)) = member.split_once('=') else { continue };
        let key = key.trim();
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-' | b'.' | b'*')) {
            continue;
        }
        let rest = rest.trim_start();
        // The member's value (a parenthesized inner list or a bare item), then its parameters.
        let (items, params) = if let Some(inner) = rest.strip_prefix('(') {
            let Some(end) = find_list_end(inner) else { continue };
            (split_whitespace_outside_strings(&inner[..end]), &inner[end + 1..])
        } else {
            let end = rest.find(';').unwrap_or(rest.len());
            (vec![rest[..end].trim().to_owned()], &rest[end..])
        };
        let mut allowlist = Allowlist::default();
        for item in items {
            // An item's own parameters are not read.
            let item = split_outside_strings(&item, ';').into_iter().next().unwrap_or("").trim();
            if let Some(quoted) = item.strip_prefix('"') {
                let Some(end) = quoted.find('"') else { continue };
                if let Ok(url) = url::Url::parse(&quoted[..end]) {
                    let origin = url.origin();
                    if origin.is_tuple() {
                        allowlist.origins.push(origin.ascii_serialization());
                    }
                }
            } else if item == "*" {
                allowlist.star = true;
            } else if item == "self" {
                allowlist.self_origin = true;
            }
        }
        let mut report_to = None;
        for param in split_outside_strings(params, ';').into_iter().skip(1) {
            if let Some((name, v)) = param.split_once('=') {
                if name.trim() == "report-to" {
                    let v = v.trim();
                    report_to = Some(v.strip_prefix('"').and_then(|q| q.strip_suffix('"')).unwrap_or(v).to_owned());
                }
            }
        }
        out.insert(key.to_owned(), Declared { allowlist, report_to });
    }
    out
}

/// The index of the `)` that ends an inner list whose text after `(` is `s`.
fn find_list_end(s: &str) -> Option<usize> {
    let mut in_string = false;
    for (i, c) in s.char_indices() {
        match c {
            '"' => in_string = !in_string,
            ')' if !in_string => return Some(i),
            _ => {}
        }
    }
    None
}

fn split_outside_strings(s: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut start, mut in_string) = (0, false);
    for (i, c) in s.char_indices() {
        if c == '"' {
            in_string = !in_string;
        } else if c == separator && !in_string {
            parts.push(&s[start..i]);
            start = i + c.len_utf8();
        }
    }
    parts.push(&s[start..]);
    parts
}

fn split_whitespace_outside_strings(s: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    for c in s.chars() {
        if c == '"' {
            in_string = !in_string;
        }
        if c.is_ascii_whitespace() && !in_string {
            if !current.is_empty() {
                items.push(std::mem::take(&mut current));
            }
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        items.push(current);
    }
    items
}

impl PageDom {
    /// The permissions policy the document starts with, as pyro-browser computed it when it made
    /// the document ([`inherit`], or [`PermissionsPolicy::top_level`]). Replaces any header policy.
    pub fn set_permissions_policy(&self, policy: PermissionsPolicy) {
        *self.permissions_policy.borrow_mut() = policy;
    }

    /// The document committed with these policy headers: the `Permissions-Policy` value, and after
    /// a newline the `Permissions-Policy-Report-Only` one (`None`: it had none). The inherited
    /// policy stays.
    pub fn set_permissions_policy_headers(&self, value: Option<&str>) {
        let (enforced, report_only) = match value {
            Some(v) => v.split_once('\n').unwrap_or((v, "")),
            None => ("", ""),
        };
        let mut policy = self.permissions_policy.borrow_mut();
        policy.declared = parse_header(enforced);
        policy.report_only = parse_header(report_only);
    }
}

/// Packs the two header values for [`PageDom::set_permissions_policy_headers`]; `None` when the
/// response had neither.
pub fn pack_headers(enforced: Option<String>, report_only: Option<String>) -> Option<String> {
    match (enforced, report_only) {
        (None, None) => None,
        (enforced, report_only) => Some(format!("{}\n{}", enforced.unwrap_or_default(), report_only.unwrap_or_default())),
    }
}

/// Whether `feature` is enabled in the document: its inherited policy and its `Permissions-Policy`
/// header both allow its own origin. The question every gate asks; it reports nothing (use
/// [`check_feature`] where the document is about to use the feature). A feature that is not
/// policy-controlled is enabled.
pub fn feature_allowed(page: &PageDom, feature: &str) -> bool {
    page.permissions_policy.borrow().enabled(feature)
}

/// The `ch-*` features of Client Hints this document's policy refuses its own origin, for the
/// `Accept-CH` hints its requests would carry (`None`: none refused).
pub(crate) fn denied_client_hints(page: &PageDom) -> Option<std::sync::Arc<Vec<String>>> {
    let policy = page.permissions_policy.borrow();
    let denied: Vec<String> =
        FEATURES.iter().map(|(name, _)| *name).filter(|name| name.starts_with("ch-") && !policy.enabled(name)).map(str::to_owned).collect();
    (!denied.is_empty()).then(|| std::sync::Arc::new(denied))
}

/// [`feature_allowed`] for a use of `feature` by script of the realm `s` of `page`: also queues
/// the `permissions-policy-violation` report when the use is refused (`disposition: "enforce"`),
/// or would be by the report-only policy (`"report"`).
pub fn check_feature(s: &Scope<'_>, page: &Rc<PageDom>, feature: &str) -> bool {
    let verdict = page.permissions_policy.borrow().verdict(feature);
    if let Some((disposition, report_to)) = verdict.violation {
        report_violation(s, page, feature, disposition, report_to);
    }
    verdict.allowed
}

fn report_violation(s: &Scope<'_>, page: &Rc<PageDom>, feature: &str, disposition: &str, report_to: Option<String>) {
    let url = crate::csp_report::strip_url_for_report(&page.document_url());
    // The calling script's location, from the innermost page frame of the stack (reporting.js).
    let location = crate::reporting::MODULE
        .helper(s, page, "callerSource", &[])
        .ok()
        .and_then(|v| v.as_string())
        .and_then(|text| serde_json::from_str::<Json>(&text).ok())
        .and_then(|v| match v {
            Json::Object(o) => Some(o),
            _ => None,
        });
    let mut body = Map::new();
    body.insert("featureId".into(), feature.into());
    body.insert("sourceFile".into(), location.as_ref().and_then(|l| l.get("sourceFile").cloned()).unwrap_or_else(|| url.clone().into()));
    body.insert("lineNumber".into(), location.as_ref().and_then(|l| l.get("lineNumber").cloned()).unwrap_or(Json::Null));
    body.insert("columnNumber".into(), location.as_ref().and_then(|l| l.get("columnNumber").cloned()).unwrap_or(Json::Null));
    body.insert("disposition".into(), disposition.into());
    queue_report(
        page,
        Some(s),
        ReportInput {
            report_type: "permissions-policy-violation",
            url,
            body,
            destination: Some(report_to.unwrap_or_else(|| "default".to_owned())),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Option<&str> = Some("https://a.test");
    const B: Option<&str> = Some("https://b.test");

    fn declared(value: &str, feature: &str) -> Declared {
        parse_header(value).remove(feature).unwrap()
    }

    #[test]
    fn header_members() {
        let d = declared("camera=(), fullscreen=*, geolocation=(self \"https://b.test\");report-to=ep", "geolocation");
        assert!(d.allowlist.self_origin && !d.allowlist.star);
        assert_eq!(d.allowlist.origins, vec!["https://b.test".to_owned()]);
        assert_eq!(d.report_to.as_deref(), Some("ep"));
        assert_eq!(declared("camera=()", "camera").allowlist, Allowlist::default());
        assert!(declared("fullscreen=*", "fullscreen").allowlist.star);
        assert!(declared("fullscreen=self", "fullscreen").allowlist.self_origin);
        assert!(parse_header("not a dictionary").is_empty());
    }

    #[test]
    fn allowlist_matching() {
        let list = declared("camera=(self \"https://b.test\")", "camera").allowlist;
        assert!(list.matches(A, A, true));
        assert!(list.matches(A, A, false));
        assert!(list.matches(B, A, false));
        assert!(!list.matches(Some("https://c.test"), A, false));
        assert!(!Allowlist::default().matches(A, A, true));
    }

    #[test]
    fn container_policy() {
        assert_eq!(container_allows("camera", Some("camera"), A, B, B), Some(true));
        assert_eq!(container_allows("camera", Some("camera"), A, B, A), Some(false));
        assert_eq!(container_allows("camera", Some("camera 'none'"), A, B, B), Some(false));
        assert_eq!(container_allows("camera", Some("microphone"), A, B, B), None);
        assert_eq!(container_allows_in("camera", Some("camera"), A, None, None, Some(true)), Some(true));
        assert_eq!(container_allows_in("camera", Some("camera"), A, None, None, Some(false)), Some(false));
        assert_eq!(container_allows_in("camera", Some("camera"), A, None, None, None), Some(false));
    }

    #[test]
    fn enabled_follows_inheritance_and_header() {
        let mut policy = PermissionsPolicy::top_level(A.map(str::to_owned));
        assert!(policy.enabled("camera"));
        assert!(policy.enabled("not-a-feature"));
        policy.declared = parse_header("camera=()");
        assert!(!policy.enabled("camera"));
        assert!(policy.enabled("microphone"));
        policy.disabled.push("microphone");
        assert!(!policy.enabled("microphone"));
        assert!(feature("fullscreen").is_some());
    }
}
