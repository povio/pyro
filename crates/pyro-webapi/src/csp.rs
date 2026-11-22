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
//! \[\[CryptographicNonce\]\] slot ([`element_initiator`]); everything else is unattributed and
//! matches no nonce-source. A module graph's imports are unattributed: the engine's module
//! loader does not say which module or root asked for one.
//!
//! Not covered (explicitly): WebAssembly compilation (`wasm_eval_check` exists, nothing calls
//! it); `report-uri` / `report-to` delivery (violation records only); local-scheme documents
//! (`about:blank`, `about:srcdoc`, `data:` frames) do not inherit their creator's policies;
//! `frame-src` is checked against the parent's policies, not the navigation initiator's, and
//! a `javascript:` navigation against the target document's; `Page.setBypassCSP` applies from
//! the next document committed; a blocked `sendBeacon` still returns true (it is queued and
//! fails, as Chrome); inline `<style>` and `style` attributes (CSS code) and `setTimeout`
//! strings are not checked.

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

/// The policies of `page`'s document as delivered (each with its delivery, header or `<meta>`,
/// and disposition), for a worker that inherits them (a `data:` or `blob:` script).
pub(crate) fn inherited_policies(page: &PageDom) -> Vec<pyro_dom::CspPolicy> {
    page.dom.borrow().csp_policies(page.document()).to_vec()
}

/// Whether `page`'s document has a CSP policy (a cheap test before capturing a source location).
pub(crate) fn has_policies(page: &PageDom) -> bool {
    !page.dom.borrow().csp_policies(page.document()).is_empty()
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
    inline_allowed_at(page, kind, nonce, source, target, None)
}

/// [`inline_allowed`] with the violations located at `location` (the script that set the
/// handler attribute or navigated to the `javascript:` URL, captured when it did).
pub fn inline_allowed_at(
    page: &Rc<PageDom>,
    kind: InlineKind,
    nonce: &str,
    source: &str,
    target: Option<NodeId>,
    location: Option<&SourceLocation>,
) -> bool {
    let Some(list) = policy_list(page) else { return true };
    let mut outcome = list.inline_check(kind, nonce, source);
    locate(&mut outcome.violations, location);
    report(page, outcome.violations, target);
    !outcome.blocked
}

/// CSP3 "EnsureCSPDoesNotBlockStringCompilation" for `page`: `eval`, `Function` (`source`: the
/// compiled string). Violations at the document. Whether it may compile.
pub fn eval_allowed(page: &Rc<PageDom>, source: &str) -> bool {
    eval_allowed_at(page, source, None)
}

/// [`eval_allowed`] with the violations located at `location` (the script that called `eval`
/// or `Function`).
pub fn eval_allowed_at(page: &Rc<PageDom>, source: &str, location: Option<&SourceLocation>) -> bool {
    let Some(list) = policy_list(page) else { return true };
    let mut outcome = list.eval_check(source);
    locate(&mut outcome.violations, location);
    report(page, outcome.violations, None);
    !outcome.blocked
}

/// Where a violation happened (CSP3 violation's source file, line number and column number):
/// the innermost frame of page script on the stack when the violating operation ran. The URL is
/// already stripped for reports ([`strip_source_file`]). Only ever read off a real stack: a
/// violation with no script on the stack (a parser-inserted handler or script) has none, and
/// reports an empty `sourceFile` with line and column 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceLocation {
    pub source_file: String,
    /// 1-based, as the engine's stack reports them.
    pub line: u32,
    pub column: u32,
}

impl SourceLocation {
    /// The location of the page script running in `s`, `None` when none is on the stack (only
    /// engine-internal frames, or a stack the engine could not give).
    pub fn running(s: &Scope<'_>) -> Option<SourceLocation> {
        let (url, line, column) = running_script_location(s)?;
        Some(SourceLocation { source_file: strip_source_file(&url), line, column })
    }

    /// Fills `violation`'s source file, line and column.
    pub fn apply(&self, violation: &mut Violation) {
        violation.source_file = self.source_file.clone();
        violation.line = self.line;
        violation.column = self.column;
    }
}

/// Sets the location of each of `violations` (fresh from a check, without one).
pub(crate) fn locate(violations: &mut [Violation], location: Option<&SourceLocation>) {
    let Some(location) = location else { return };
    for violation in violations {
        location.apply(violation);
    }
}

/// URL prefixes of the engine's own scripts in a stack trace (pyro-webapi's modules and
/// pyro-js's helpers): never a violation's location.
const INTERNAL_URLS: [&str; 2] = ["pyro://", "lightpanda://"];

/// `(url, line, column)` of the innermost frame of page script on the stack of `s`, as the
/// engine reports it (unstripped).
pub(crate) fn running_script_location(s: &Scope<'_>) -> Option<(String, u32, u32)> {
    let stack = s.current_stack().ok()?;
    stack.lines().find_map(parse_stack_frame)
}

/// One JavaScriptCore stack line, `function@url:line:column` or `url:line:column`: its page
/// script location. `None` for native frames (`[native code]`), frames without a URL and the
/// engine's own scripts.
fn parse_stack_frame(frame: &str) -> Option<(String, u32, u32)> {
    let location = frame.split_once('@').map_or(frame, |(_, location)| location);
    let (rest, column) = location.rsplit_once(':')?;
    let (url, line) = rest.rsplit_once(':')?;
    if url.is_empty() || INTERNAL_URLS.iter().any(|prefix| url.starts_with(prefix)) {
        return None;
    }
    Some((url.to_owned(), line.parse().ok()?, column.parse().ok()?))
}

/// A violation's source file as `SecurityPolicyViolationEvent.sourceFile` shows it, as Chrome
/// does (WPT securitypolicyviolation/source-file.html): an http(s), ws(s) or ftp URL without
/// its fragment, query, username and password; any other URL is only its scheme (`data`,
/// `blob`, `file`, `about`, `javascript`, a custom one); a string that is not a URL (a relative
/// `sourceURL`) is empty.
pub fn strip_source_file(value: &str) -> String {
    let Ok(mut url) = url::Url::parse(value) else { return String::new() };
    if !matches!(url.scheme(), "http" | "https" | "ws" | "wss" | "ftp") {
        return url.scheme().to_owned();
    }
    url.set_fragment(None);
    url.set_query(None);
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.into()
}

thread_local! {
    /// The location event handler content attributes compiled now are set at, while
    /// [`with_handler_location`] runs (`Some(None)`: no script set them, the parser did).
    static HANDLER_LOCATION: std::cell::RefCell<Option<Option<SourceLocation>>> = const { std::cell::RefCell::new(None) };
}

/// Runs `f` with handler attributes compiled in it located at `location` instead of the script
/// on the stack: `drain_inline` compiles attributes the parser or a fragment parse made, whose
/// location is not that of whatever script triggered the drain.
pub(crate) fn with_handler_location<R>(location: Option<SourceLocation>, f: impl FnOnce() -> R) -> R {
    let previous = HANDLER_LOCATION.with(|l| l.borrow_mut().replace(location));
    let result = f();
    HANDLER_LOCATION.with(|l| *l.borrow_mut() = previous);
    result
}

/// The location of an event handler content attribute set now in `s`: the one
/// [`with_handler_location`] gives, else the running script's (`setAttribute`).
pub(crate) fn handler_location(s: &Scope<'_>) -> Option<SourceLocation> {
    match HANDLER_LOCATION.with(|l| l.borrow().clone()) {
        Some(location) => location,
        None => SourceLocation::running(s),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_frames_give_page_script_locations_only() {
        assert_eq!(
            parse_stack_frame("f@https://a.test/s.js:3:17"),
            Some(("https://a.test/s.js".to_owned(), 3, 17))
        );
        assert_eq!(parse_stack_frame("https://a.test/s.js:10:7"), Some(("https://a.test/s.js".to_owned(), 10, 7)));
        assert_eq!(
            parse_stack_frame("global code@data:text/javascript,%0Aeval(1):3:17"),
            Some(("data:text/javascript,%0Aeval(1)".to_owned(), 3, 17))
        );
        assert_eq!(parse_stack_frame("eval@[native code]"), None);
        assert_eq!(parse_stack_frame("eval code@"), None);
        assert_eq!(parse_stack_frame("canCompile@pyro://internal/trusted_types.js:333:7"), None);
        assert_eq!(parse_stack_frame("stack@lightpanda://internal/helpers.js:1:1"), None);
    }

    #[test]
    fn source_files_strip_as_chrome() {
        let cases = [
            ("http://dummy.test/script1.js", "http://dummy.test/script1.js"),
            ("https://dummy.test/script1.js#frag", "https://dummy.test/script1.js"),
            ("https://dummy.test/script1.js?query", "https://dummy.test/script1.js"),
            ("https://dummy.test:8080/script1.js", "https://dummy.test:8080/script1.js"),
            ("https://user:password@dummy.test/script1.js", "https://dummy.test/script1.js"),
            ("https://user@dummy.test/script1.js", "https://dummy.test/script1.js"),
            ("wss://dummy.test/script1.js", "wss://dummy.test/script1.js"),
            ("ws://dummy.test/script1.js", "ws://dummy.test/script1.js"),
            ("script2.js", ""),
            ("file:///temp/script3.js", "file"),
            ("webpack://node_modules/sample/script4.js", "webpack"),
            ("about:blank", "about"),
            ("about:custom", "about"),
            ("data:text/html;charset=utf8,<html></html>", "data"),
            ("blob:http://test.test/012345-6789-abcd-efab-0123456789", "blob"),
            ("javascript:void(0)", "javascript"),
        ];
        for (input, expected) in cases {
            assert_eq!(strip_source_file(input), expected, "{input}");
        }
    }

    #[test]
    fn handler_location_override_nests_and_restores() {
        let at = SourceLocation { source_file: "https://a.test/".into(), line: 10, column: 5 };
        assert_eq!(HANDLER_LOCATION.with(|l| l.borrow().clone()), None);
        with_handler_location(Some(at.clone()), || {
            assert_eq!(HANDLER_LOCATION.with(|l| l.borrow().clone()), Some(Some(at.clone())));
            with_handler_location(None, || {
                assert_eq!(HANDLER_LOCATION.with(|l| l.borrow().clone()), Some(None));
            });
            assert_eq!(HANDLER_LOCATION.with(|l| l.borrow().clone()), Some(Some(at.clone())));
        });
        assert_eq!(HANDLER_LOCATION.with(|l| l.borrow().clone()), None);
    }

    #[test]
    fn located_violations_carry_the_location() {
        let mut list = PolicyList::for_origin(Some("https://a.test"));
        list.add("script-src 'none'", PolicySource::Header, Disposition::Enforce);
        let mut outcome = list.eval_check("1");
        assert_eq!(outcome.violations.len(), 1);
        let at = SourceLocation { source_file: "https://a.test/s.js".into(), line: 3, column: 17 };
        locate(&mut outcome.violations, Some(&at));
        let v = &outcome.violations[0];
        assert_eq!((v.source_file.as_str(), v.line, v.column), ("https://a.test/s.js", 3, 17));
        let mut unlocated = list.eval_check("1");
        locate(&mut unlocated.violations, None);
        let v = &unlocated.violations[0];
        assert_eq!((v.source_file.as_str(), v.line, v.column), ("", 0, 0));
    }
}
