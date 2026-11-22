//! Content Security Policy enforcement for a page's (or a worker's) fetches: its CSP list as a
//! [`pyro_net::csp::PolicyList`], the pre-request checks of the fetch paths with their
//! violations reported, and `securitypolicyviolation`.
//!
//! The list of a document is what `pyro_dom::Dom::csp_policies` recorded for it (headers, then
//! `<meta>`); a worker's is what its own document there holds (its script's response headers,
//! or its owner's when the script was `data:` or `blob:`). `'self'` is the context's origin.
//!
//! A check gives back a [`RequestCsp`] for the request: the client runs the check again on each
//! redirect hop, and the initiator reports what those found once the request ended
//! ([`report_guard`]). Violations fire as `securitypolicyviolation`
//! (SecurityPolicyViolationEvent, the Trusted Types module's class) at the element that made the
//! request, else the document (a worker's global in a worker), each in a task of its own,
//! bubbling and composed. The violation records go to [`report`] unchanged, for a reporting step
//! (`report-uri`, `report-to`) to send.
//!
//! Provenance: a request carries a nonce only from its initiating element's own
//! \[\[CryptographicNonce\]\] slot ([`element_initiator`]), or, for a module import, from the
//! nonce its referring script's element passed down (the browser loader's module options,
//! keyed by the referring script the engine names); everything else is unattributed and
//! matches no nonce-source.
//!
//! Not covered (explicitly): WebAssembly compilation (`wasm_eval_check` exists, nothing calls
//! it); `report-uri` / `report-to` delivery (violation records only); local-scheme documents
//! (`about:blank`, `about:srcdoc`, `data:` frames) do not inherit their creator's policies;
//! `frame-src` is checked against the parent's policies (a `javascript:` navigation against its
//! initiator's, [`NavigationInitiator`]); `Page.setBypassCSP` applies from
//! the next document committed; a blocked `sendBeacon` sends nothing and still returns true
//! (as Chrome); inline `<style>` and `style` attributes (CSS code) are not checked.
//! `setTimeout`/`setInterval` strings are checked when the timer is set
//! (`trusted_types::timer_handler_source`), as Chrome.

use std::rc::Rc;
use std::sync::Arc;

use pyro_dom::NodeId;
use pyro_js::{JsError, JsResult, Scope, Value};
use pyro_net::csp::{CspRequest, Disposition, PolicyList, PolicySource, RequestCsp, RequestDest, Violation};

use crate::page::PageDom;

pub use pyro_net::csp::{InlineKind, ScriptInitiator, BLOCKED_BY_CSP};

/// An enforced policy blocked the request (its violations were reported).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Blocked;

/// The nonce CSP may match for `el`: its \[\[CryptographicNonce\]\] slot when `el` is nonceable
/// (HTML "is element nonceable": no attribute name or value holds `<script` or `<style`, the
/// dangling-markup case), else empty.
pub fn element_nonce(page: &PageDom, el: NodeId) -> String {
    let nonce = page.cryptographic_nonce(el);
    if nonce.is_empty() || !nonceable(page, el) {
        return String::new();
    }
    nonce
}

fn nonceable(page: &PageDom, el: NodeId) -> bool {
    let dom = page.dom.borrow();
    let Ok(attrs) = dom.attributes(el) else { return false };
    let markup = |text: &str| {
        let lower = text.to_ascii_lowercase();
        lower.contains("<script") || lower.contains("<style")
    };
    !attrs.iter().any(|a| markup(&a.name.local) || markup(&a.value))
}

/// The initiator of a request `el` makes (`parser_inserted`: its parser metadata), with its
/// nonce read from the element itself ([`element_nonce`]): the only way a request carries a
/// nonce.
pub fn element_initiator(page: &PageDom, el: NodeId, parser_inserted: bool) -> ScriptInitiator {
    ScriptInitiator::Element { nonce: element_nonce(page, el), parser_inserted }
}

/// The serialized origin `'self'` means for `page` (`None`: opaque).
/// The origin CSP's `'self'` means in `page`: its document's, or for a sandboxed document,
/// whose own origin is opaque, the precursor origin it would have without the sandbox (as
/// Chrome; [`PageDom::set_csp_precursor_origin`]). `None`: no origin `'self'` matches.
pub fn self_origin(page: &PageDom) -> Option<String> {
    crate::storage::document_origin(page).or_else(|| page.csp_precursor_origin())
}

/// `page`'s CSP list (its document's), `None` when it has no policy.
pub fn policy_list(page: &PageDom) -> Option<Arc<PolicyList>> {
    if page.dom.borrow().csp_policies(page.document()).is_empty() {
        return None;
    }
    let origin = self_origin(page);
    let mut list = PolicyList::for_origin(origin.as_deref());
    for policy in page.dom.borrow().csp_policies(page.document()) {
        let disposition = if policy.report_only { Disposition::Report } else { Disposition::Enforce };
        let source = if policy.meta { PolicySource::Meta } else { PolicySource::Header };
        list.add(&policy.text, source, disposition);
    }
    (!list.is_empty()).then(|| Arc::new(list))
}

/// The policies of `page`'s document as delivered (each with its delivery, header or `<meta>`,
/// and disposition), for a worker that inherits them (a `data:` or `blob:` script).
pub(crate) fn inherited_policies(page: &PageDom) -> Vec<pyro_dom::CspPolicy> {
    page.dom.borrow().csp_policies(page.document()).to_vec()
}

/// The pre-request check of `request` against `page`'s CSP list (CSP3 "Should request be
/// blocked by Content Security Policy?"). Its violations are reported at `target` (an element of
/// the document), else the document. `Ok` carries the guard the request should carry for its
/// redirects (`None`: the page has no policy).
pub fn check_request(page: &Rc<PageDom>, request: CspRequest, target: Option<NodeId>) -> Result<Option<Arc<RequestCsp>>, Blocked> {
    match policy_list(page) {
        Some(list) => check_request_with(page, list, request, target),
        None => Ok(None),
    }
}

/// [`check_request`] against `list` (another context's, as a worker's owner's).
pub fn check_request_with(
    page: &Rc<PageDom>,
    list: Arc<PolicyList>,
    request: CspRequest,
    target: Option<NodeId>,
) -> Result<Option<Arc<RequestCsp>>, Blocked> {
    let outcome = list.pre_request(&request);
    report(page, outcome.violations, target);
    if outcome.blocked {
        return Err(Blocked);
    }
    Ok(Some(RequestCsp::new(list, request)))
}

/// [`check_request`] for an initiator that hands a blocked request to the client anyway, to
/// fail through its own network-error path: the guard to set as `Request::csp` (one that blocks
/// the request when an enforced policy did), `None` without a policy.
pub fn guard_request(page: &Rc<PageDom>, request: CspRequest, target: Option<NodeId>) -> Option<Arc<RequestCsp>> {
    let list = policy_list(page)?;
    let outcome = list.pre_request(&request);
    report(page, outcome.violations, target);
    Some(if outcome.blocked { RequestCsp::blocked(list, request) } else { RequestCsp::new(list, request) })
}

/// CSP3 "Should element's inline type behavior be blocked?" for `page`: an inline `<script>`
/// (its `nonce`), an event handler attribute, a `javascript:` URL (`source`: the text whose
/// hash may be listed). Violations at `target`, else the document. Whether it may run.
pub fn inline_allowed(page: &Rc<PageDom>, kind: InlineKind, nonce: &str, source: &str, target: Option<NodeId>) -> bool {
    inline_allowed_in(page, policy_list(page).as_deref(), kind, nonce, source, target)
}

/// [`inline_allowed`] against `list`: another context's policies, as a navigation's initiator's
/// snapshotted when the navigation started (a `javascript:` URL is checked against the
/// document that navigated, not the one it runs in). Violations are reported at `page` (that
/// initiator). `None`: no policy, allowed.
pub fn inline_allowed_in(
    page: &Rc<PageDom>,
    list: Option<&PolicyList>,
    kind: InlineKind,
    nonce: &str,
    source: &str,
    target: Option<NodeId>,
) -> bool {
    let Some(list) = list else { return true };
    let outcome = list.inline_check(kind, nonce, source);
    report(page, outcome.violations, target);
    !outcome.blocked
}

/// A navigation initiator's CSP, snapshotted when it starts a navigation: its document (where
/// violations go) and its policy list (`None`: no policy).
#[derive(Clone)]
pub struct NavigationInitiator {
    pub page: Rc<PageDom>,
    pub list: Option<Arc<PolicyList>>,
}

impl NavigationInitiator {
    /// `page`'s policies, now.
    pub fn of(page: &Rc<PageDom>) -> NavigationInitiator {
        NavigationInitiator { page: page.clone(), list: policy_list(page) }
    }

    /// CSP3 for a `javascript:` navigation (`url`, the whole URL, is what a hash-source with
    /// 'unsafe-hashes' matches): `script-src-elem` of the initiator's policies. Whether its
    /// script may run.
    pub fn allows_javascript_url(&self, url: &str) -> bool {
        inline_allowed_in(&self.page, self.list.as_deref(), InlineKind::Navigation, "", url, None)
    }
}

/// CSP3 "EnsureCSPDoesNotBlockStringCompilation" for `page`: `eval`, `Function` (`source`: the
/// compiled string). Violations at the document. Whether it may compile.
pub fn eval_allowed(page: &Rc<PageDom>, source: &str) -> bool {
    let Some(list) = policy_list(page) else { return true };
    let outcome = list.eval_check(source);
    report(page, outcome.violations, None);
    !outcome.blocked
}

/// Whether `page`'s policies make string compilation need a check: 2 when an enforced policy
/// lacks `'unsafe-eval'` for `script-src` (or `default-src`), 1 when only report-only ones do,
/// else 0 (the Trusted Types compile hooks' levels).
pub(crate) fn eval_level(page: &PageDom) -> u8 {
    let Some(list) = policy_list(page) else { return 0 };
    let mut level = 0;
    for policy in &list.policies {
        let Some(directive) = policy.directive_for("script-src") else { continue };
        if directive.value.iter().any(|v| v.eq_ignore_ascii_case("'unsafe-eval'")) {
            continue;
        }
        level = level.max(if policy.disposition == Disposition::Report { 1 } else { 2 });
    }
    level
}

/// CSP3 `'trusted-types-eval'`: whether every policy of `page` whose `script-src` (or
/// `default-src`) lacks `'unsafe-eval'` has `'trusted-types-eval'`, so that string compilation
/// needs only enforced Trusted Types (which the caller checked).
pub(crate) fn trusted_types_eval_allows(page: &PageDom) -> bool {
    let Some(list) = policy_list(page) else { return true };
    list.policies.iter().all(|policy| {
        policy.directive_for("script-src").map_or(true, |directive| {
            directive
                .value
                .iter()
                .any(|v| v.eq_ignore_ascii_case("'unsafe-eval'") || v.eq_ignore_ascii_case("'trusted-types-eval'"))
        })
    })
}

/// After a request that carried `guard` ended (or failed): reports the violations its redirect
/// checks found.
pub fn report_guard(page: &Rc<PageDom>, guard: Option<&Arc<RequestCsp>>, target: Option<NodeId>) {
    if let Some(guard) = guard {
        let violations = guard.take_violations();
        if !violations.is_empty() {
            report(page, violations, target);
        }
    }
}

/// Fires `securitypolicyviolation` for each of `violations` at `target` (while it is connected),
/// else at the document (a worker's global in a worker), each in a task of its own (CSP3
/// "report a violation" queues a task).
pub fn report(page: &Rc<PageDom>, violations: Vec<Violation>, target: Option<NodeId>) {
    for violation in violations {
        let weak = Rc::downgrade(page);
        page.post_task(Box::new(move || {
            let Some(page) = weak.upgrade().filter(|p| !p.is_disposed()) else { return };
            let Some(realm) = page.main_world_realm() else { return };
            let Ok(s) = realm.scope() else { return };
            if let Err(e) = dispatch(&s, &page, &violation, target) {
                s.report_exception(&e);
            }
        }));
    }
}

/// Fires one violation now, in `s` (the realm of `page`'s document or worker).
pub fn dispatch(s: &Scope<'_>, page: &Rc<PageDom>, violation: &Violation, target: Option<NodeId>) -> JsResult<()> {
    let module = crate::trusted_types::MODULE.module(s, page)?;
    let fire = module
        .get_atom("dispatchViolation")?
        .as_object()
        .ok_or_else(|| JsError::Type("SecurityPolicyViolationEvent is unavailable".into()))?;
    let connected = target.filter(|node| page.dom.borrow().is_connected(*node).unwrap_or(false));
    let at: Value<'_> = match connected {
        Some(node) => crate::node::wrap_node(s, page, node)?.value(),
        None => s.null(),
    };
    let init = s.object();
    init.set_atom("blockedURI", s.string(&violation.blocked_url))?;
    init.set_atom("effectiveDirective", s.string(&violation.effective_directive))?;
    // Chrome reports the effective directive as the violated one.
    init.set_atom("violatedDirective", s.string(&violation.effective_directive))?;
    init.set_atom("originalPolicy", s.string(&violation.original_policy))?;
    init.set_atom("sourceFile", s.string(&violation.source_file))?;
    init.set_atom("sample", s.string(&violation.sample))?;
    init.set_atom("disposition", s.string(violation.disposition.as_str()))?;
    init.set_atom("statusCode", s.number(f64::from(violation.status_code)))?;
    init.set_atom("lineNumber", s.number(f64::from(violation.line)))?;
    init.set_atom("columnNumber", s.number(f64::from(violation.column)))?;
    fire.call(None, &[at, init.value()])?;
    Ok(())
}

/// The CSP destination of a script fetch made by the engine for a module of `ty`.
pub fn module_dest(ty: pyro_js::ModuleType) -> RequestDest {
    match ty {
        // JSON and text modules: connect-src.
        pyro_js::ModuleType::Json | pyro_js::ModuleType::Text => RequestDest::Json,
        pyro_js::ModuleType::Css => RequestDest::Style,
        _ => RequestDest::Script,
    }
}
