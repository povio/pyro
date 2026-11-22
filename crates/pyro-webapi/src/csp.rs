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

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use pyro_dom::NodeId;
use pyro_js::{JsError, JsResult, Scope, Value};
use pyro_net::csp::{CspRequest, Disposition, PolicyList, PolicySource, RequestCsp, RequestDest, Violation};

use crate::page::PageDom;

pub use pyro_net::csp::BLOCKED_BY_CSP;

/// An enforced policy blocked the request (its violations were reported).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Blocked;

/// The serialized origin `'self'` means for `page` (`None`: opaque).
fn self_origin(page: &PageDom) -> Option<String> {
    crate::storage::document_origin(page)
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

/// The policies of `page`'s document as delivered, for a worker that inherits them (a `data:`
/// or `blob:` script): the enforced ones joined by `,`, and the report-only ones.
pub(crate) fn inherited_policies(page: &PageDom) -> (String, String) {
    let dom = page.dom.borrow();
    let mut enforce = Vec::new();
    let mut report_only = Vec::new();
    for policy in dom.csp_policies(page.document()) {
        if policy.report_only {
            report_only.push(policy.text.to_string());
        } else {
            enforce.push(policy.text.to_string());
        }
    }
    (enforce.join(","), report_only.join(","))
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

thread_local! {
    /// The destination the next `fetch()` of this thread is made for, when an engine-internal
    /// caller (a worklet's `addModule`) uses `fetch` for a request that is not of the empty
    /// destination: [`set_next_fetch_dest`], taken by `fetch` at its start.
    static NEXT_FETCH_DEST: Cell<Option<RequestDest>> = const { Cell::new(None) };
}

/// The next `fetch()` call of this thread is a request for `dest` (CSP picks its directive by
/// it). Only engine-internal script calls this, right before its `fetch`.
pub(crate) fn set_next_fetch_dest(dest: RequestDest) {
    NEXT_FETCH_DEST.with(|d| d.set(Some(dest)));
}

/// What [`set_next_fetch_dest`] left for this `fetch()` call, if anything.
pub(crate) fn take_next_fetch_dest() -> Option<RequestDest> {
    NEXT_FETCH_DEST.with(|d| d.take())
}

/// The CSP destination of a script fetch made by the engine for a module of `ty`.
pub fn module_dest(ty: pyro_js::ModuleType) -> RequestDest {
    match ty {
        pyro_js::ModuleType::Json => RequestDest::Json,
        pyro_js::ModuleType::Css => RequestDest::Style,
        _ => RequestDest::Script,
    }
}
