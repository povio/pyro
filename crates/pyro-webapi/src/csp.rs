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
//! bubbling and composed. In the same task, after the event, a document's violation goes to
//! `crate::csp_report` for its `report-uri` / `report-to` reports ([`report_parts`]); a worker's
//! sends none yet.
//!
//! Provenance: a request carries a nonce only from its initiating element's own
//! \[\[CryptographicNonce\]\] slot ([`element_initiator`]), or, for a module import, from the
//! nonce its referring script's element passed down (the browser loader's module options,
//! keyed by the referring script the engine names); everything else is unattributed and
//! matches no nonce-source.
//!
//! WebAssembly compilation ([`wasm_allowed`]) is gated at the JS level (install.js
//! `wasmStreaming`, armed by [`sync_wasm_gate`] in realms whose policies need it).
//!
//! Not covered (explicitly): `report-uri` / `report-to` reports from workers; a local-scheme document
//! (`about:blank`, `about:srcdoc`, `blob:`, `data:`, a `javascript:` result) inherits its
//! policies ([`InheritedCsp`]) from its parent, else its popup's creator (a `javascript:` result
//! from the document it replaces), not from the document that started its navigation;
//! `frame-src` is checked against the parent's policies (a `javascript:` navigation against its
//! initiator's, [`NavigationInitiator`]); `Page.setBypassCSP` applies from
//! the next document committed; a blocked `sendBeacon` sends nothing and still returns true
//! (as Chrome). `setTimeout`/`setInterval` strings are checked when the timer is set
//! (`trusted_types::timer_handler_source`), as Chrome.
//!
//! Inline styles ([`resolve_inline_styles`]): an HTML `<style>` is checked (`style-src-elem`,
//! its nonce and text) at each update of its style block, a `style` attribute
//! (`style-src-attr`, its hashes with `'unsafe-hashes'`) at each write by the parser or a script
//! and when its element joins the document. pyro-dom records what waits; it is checked before
//! anything reads styles (layout, `el.style`, `document.styleSheets`, the `sheet`) and after each
//! task. A blocked `<style>` has no sheet and fires `error`; a blocked attribute declares
//! nothing. CSSOM writes (`el.style.x = …`), constructed sheets and `insertRule` are not checked
//! (as Chrome). Violations carry no source position.

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
/// (CSP3 "is element nonceable": no attribute name or value holds `<script` or `<style`, nor
/// `<link` as in Chrome, the dangling-markup case), else empty.
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
    !attrs.iter().any(|a| holds_markup(&a.name.local) || holds_markup(&a.value))
}

/// Whether an attribute name or value holds the start of a `<script`, `<style` or (Chrome's
/// `ContentSecurityPolicy::IsNonceableElement` addition) `<link` tag, ASCII case-insensitively.
fn holds_markup(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("<script") || lower.contains("<style") || lower.contains("<link")
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

/// Whether an enforced (not report-only) policy of `page`'s document has the
/// `upgrade-insecure-requests` directive (Upgrade Insecure Requests §3.1), by header, `<meta>` or inheritance.
pub(crate) fn upgrades_insecure_requests(page: &PageDom) -> bool {
    let dom = page.dom.borrow();
    dom.csp_policies(page.document()).iter().any(|policy| {
        !policy.report_only
            && policy.text.split([',', ';']).any(|d| {
                d.trim_matches(|c: char| c.is_ascii_whitespace())
                    .split(|c: char| c.is_ascii_whitespace())
                    .next()
                    .is_some_and(|name| name.eq_ignore_ascii_case("upgrade-insecure-requests"))
            })
    })
}

/// The policies of `page`'s document as delivered (each with its delivery, header or `<meta>`,
/// and disposition), for a worker that inherits them (a `data:` or `blob:` script).
pub fn inherited_policies(page: &PageDom) -> Vec<pyro_dom::CspPolicy> {
    page.dom.borrow().csp_policies(page.document()).to_vec()
}

/// What a local-scheme document (`about:blank`, `about:srcdoc`, `blob:`, `data:`, a
/// `javascript:` result) takes from the document it inherits from (CSP3 § 4.2.1, HTML policy
/// containers): a copy of its policies, so a `<meta>` of its own adds to its list only, and the
/// origin its `'self'` means.
#[derive(Clone, Debug, Default)]
pub struct InheritedCsp {
    pub policies: Vec<pyro_dom::CspPolicy>,
    pub self_origin: Option<String>,
    /// The cross-origin embedder policy of the same policy container (HTML § 7.1.4 "embedder policy"
    /// is inherited with the CSP list by local-scheme documents).
    pub embedder_policy: pyro_net::EmbedderPolicy,
}

impl InheritedCsp {
    /// `page`'s, now.
    pub fn of(page: &PageDom) -> InheritedCsp {
        InheritedCsp { policies: inherited_policies(page), self_origin: self_origin(page), embedder_policy: page.embedder_policy() }
    }

    /// Gives `page`'s document, new and its parser not started, these policies. When its own
    /// origin is opaque (`data:`, a sandboxed frame) and nothing set its precursor origin yet,
    /// `'self'` keeps meaning the creator's origin (as Chrome).
    pub fn apply(&self, page: &PageDom) {
        page.set_embedder_policy(self.embedder_policy);
        if !self.policies.is_empty() {
            let _ = page.dom.borrow_mut().inherit_csp_policies(page.document(), &self.policies);
        }
        if self.self_origin.is_some() && self_origin(page).is_none() {
            page.set_csp_precursor_origin(self.self_origin.clone());
        }
    }
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
    inline_allowed_in_at(page, list, kind, nonce, source, target, None)
}

/// [`inline_allowed_in`] with the violations located at `location` (the script that navigated
/// to the `javascript:` URL, captured when it did).
pub fn inline_allowed_in_at(
    page: &Rc<PageDom>,
    list: Option<&PolicyList>,
    kind: InlineKind,
    nonce: &str,
    source: &str,
    target: Option<NodeId>,
    location: Option<&SourceLocation>,
) -> bool {
    let Some(list) = list else { return true };
    let mut outcome = list.inline_check(kind, nonce, source);
    locate(&mut outcome.violations, location);
    report(page, outcome.violations, target);
    !outcome.blocked
}

/// [`inline_allowed`] for an inline `<script>` a script inserted ([`PolicyList::inserted_script_inline_check`]:
/// `'strict-dynamic'` trusts it).
pub fn inserted_script_allowed(page: &Rc<PageDom>, nonce: &str, source: &str, target: NodeId) -> bool {
    let Some(list) = policy_list(page) else { return true };
    let outcome = list.inserted_script_inline_check(nonce, source);
    report(page, outcome.violations, Some(target));
    !outcome.blocked
}

/// Runs the inline style checks waiting in `page`'s document (pyro-dom records them: a `style`
/// attribute set, a `<style>` updated, in a document with policies), each reporting its
/// violations at its element. Nothing to do (one `RefCell` check) without them; nothing either
/// while the DOM is borrowed (the next read or task runs them).
pub fn resolve_inline_styles(page: &Rc<PageDom>) {
    let pending = {
        let Ok(mut dom) = page.dom.try_borrow_mut() else { return };
        if !dom.has_pending_style_checks() {
            return;
        }
        dom.take_pending_style_checks(page.document())
    };
    for el in pending.elements {
        style_element_allowed(page, el);
    }
    if pending.attributes.is_empty() {
        return;
    }
    let list = policy_list(page);
    for el in pending.attributes {
        let text = page.dom.borrow().get_attribute(el, "style").ok().flatten().map(str::to_owned);
        let Some(text) = text else {
            page.dom.borrow_mut().set_style_attribute_blocked(el, None);
            continue;
        };
        let allowed = inline_allowed_in(page, list.as_deref(), InlineKind::StyleAttribute, "", &text, Some(el));
        page.dom.borrow_mut().set_style_attribute_blocked(el, (!allowed).then_some(text.as_str()));
    }
}

/// CSP3 "Should element's inline type behavior be blocked?" for the HTML `<style>` `el` of
/// `page`'s document (`style-src-elem`: its nonce, its text), once per update of its style block
/// (its style sheet generation): another update checks, and reports, again. Whether its sheet
/// applies. Without a policy, true at once.
pub fn style_element_allowed(page: &Rc<PageDom>, el: NodeId) -> bool {
    let text = {
        let dom = page.dom.borrow();
        if dom.csp_policies(page.document()).is_empty() || dom.node_document(el).ok() != Some(page.document()) {
            return true;
        }
        let Ok(e) = dom.element(el) else { return true };
        // A `<style>` whose type is not CSS (`text/plain`, `module`) has no sheet: never checked
        // (Chrome reads the type first).
        if !pyro_dom::Dom::style_type_is_css(e.attr("type")) {
            return true;
        }
        let parsing = e.flags & pyro_dom::node::element_flags::STYLE_PARSING != 0;
        if !e.is_html() || &*e.name.local != "style" || parsing {
            return !dom.style_element_blocked(el);
        }
        let generation = dom.style_sheet_generation(el);
        if let Some((checked, blocked)) = dom.style_element_check(el) {
            if checked == generation {
                return !blocked;
            }
        }
        // A disconnected one has no sheet: checked when it is connected (a new update).
        if !dom.is_connected(el).unwrap_or(false) {
            return true;
        }
        let text = dom.child_text_content(el).into_owned();
        text
    };
    let nonce = element_nonce(page, el);
    let location = parser_location(page, el);
    let allowed = inline_allowed_in_at(page, policy_list(page).as_deref(), InlineKind::Style, &nonce, &text, Some(el), location.as_ref());
    page.dom.borrow_mut().set_style_element_blocked(el, !allowed);
    allowed
}

/// A navigation initiator's CSP, snapshotted when it starts a navigation: its document (where
/// violations go) and its policy list (`None`: no policy).
#[derive(Clone)]
pub struct NavigationInitiator {
    pub page: Rc<PageDom>,
    pub list: Option<Arc<PolicyList>>,
    /// Where the script that navigated was when it did ([`NavigationInitiator::at`]); `None`
    /// without one: a blocked URL's violation is then unlocated.
    pub location: Option<SourceLocation>,
    /// The element a violation is reported at (an `<iframe>` navigating to its own `src`:
    /// Chrome fires `securitypolicyviolation` at the container); `None`: the document.
    pub target: Option<NodeId>,
}

impl NavigationInitiator {
    /// `page`'s policies, now.
    pub fn of(page: &Rc<PageDom>) -> NavigationInitiator {
        NavigationInitiator { page: page.clone(), list: policy_list(page), location: None, target: None }
    }

    /// This initiator, its violations located at the page script running in `s` now (read only
    /// when it has a policy).
    pub fn at(mut self, s: &Scope<'_>) -> NavigationInitiator {
        if self.list.is_some() {
            self.location = SourceLocation::running(s);
        }
        self
    }

    /// This initiator, its violations reported at the element `target`.
    pub fn on(mut self, target: NodeId) -> NavigationInitiator {
        self.target = Some(target);
        self
    }

    /// CSP3 for a `javascript:` navigation (`url`, the whole URL, is what a hash-source with
    /// 'unsafe-hashes' matches): `script-src-elem` of the initiator's policies. Whether its
    /// script may run.
    pub fn allows_javascript_url(&self, url: &str) -> bool {
        let (list, location) = (self.list.as_deref(), self.location.as_ref());
        inline_allowed_in_at(&self.page, list, InlineKind::Navigation, "", url, self.target, location)
    }
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

/// CSP3 "EnsureCSPDoesNotBlockWasmByteCompilation" for `page` (a window's document or a worker's):
/// `script-src` (or `default-src`) without `'wasm-unsafe-eval'` or `'unsafe-eval'` blocks
/// WebAssembly compilation, reported with blockedURI `wasm-eval` at the document (a worker's
/// global). Whether it may compile.
pub fn wasm_allowed(page: &Rc<PageDom>) -> bool {
    let Some(list) = policy_list(page) else { return true };
    let outcome = list.wasm_eval_check();
    report(page, outcome.violations, None);
    !outcome.blocked
}

/// The hidden slot on a realm's module holder (its navigator) holding install.js `wasmStreaming`'s `arm` until it ran.
pub(crate) const WASM_GATE: &str = "lpWasmGateArm";

/// Before `page`'s scripts run in its realm `s` (a window's classic scripts, a worker's main
/// script): once a policy would check WebAssembly compilation (any `script-src` without
/// `'wasm-unsafe-eval'` or `'unsafe-eval'`, enforced or report-only), WebAssembly.compile,
/// instantiate and the Module constructor are replaced with ones that ask
/// [`wasm_allowed`] (install.js `wasmStreaming`; the streaming ones ask already). A realm
/// without such a policy keeps the natives untouched. Once armed it stays so (policies only
/// accumulate).
pub fn sync_wasm_gate(s: &Scope<'_>, page: &PageDom) {
    if page.dom.borrow().csp_policies(page.document()).is_empty() {
        return;
    }
    let Ok(holder) = crate::target::page_of(s).and_then(|page| crate::worker_scope::module_holder(s, &page)) else {
        return;
    };
    let Ok(Some(arm)) = s.get_hidden(holder, WASM_GATE) else { return };
    let Some(arm) = arm.as_object() else { return };
    let checked = policy_list(page).is_some_and(|list| !list.wasm_eval_check().violations.is_empty());
    if !checked {
        return;
    }
    let _ = s.set_hidden(holder, WASM_GATE, s.undefined());
    if let Err(e) = arm.call(None, &[]) {
        s.report_exception(&e);
    }
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

/// Where the inline text of the parser-created `<script>` or `<style>` `node` of `page`'s document
/// is: where its start tag ends (Chrome locates an inline violation there). `None` for an element
/// no parser made (its violation is located by the script that made it, when one runs).
pub fn parser_location(page: &Rc<PageDom>, node: NodeId) -> Option<SourceLocation> {
    let (line, column) = page.dom.borrow().parser_start_position(node)?;
    Some(SourceLocation { source_file: strip_source_file(&page.document_url()), line, column })
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
pub fn running_script_location(s: &Scope<'_>) -> Option<(String, u32, u32)> {
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
            // CSP3 "report a violation": the reports follow the event in the same task. A
            // worker sends none to endpoints yet (that belongs with the worker hooks), but its
            // observers get theirs.
            let (report, targets) = report_parts(&violation);
            if !crate::worker_scope::is_worker(&s) {
                crate::csp_report::report_violation(&page, &report, &targets);
            }
            crate::reporting::queue_csp_violation(&s, &page, &report);
        }));
    }
}

/// Sends the reports of `violations` found in the navigation response `response_url` (status
/// `status`) that never became a document: no event is fired anywhere (`frame-ancestors`). `page`
/// is the document that was in the frame, whose network sends them from the response's origin.
pub fn report_response_violations(page: &Rc<PageDom>, violations: &[Violation], response_url: &str, status: u16) {
    let origin = url::Url::parse(response_url).ok().map(|u| u.origin().ascii_serialization());
    for violation in violations {
        let (mut report, targets) = report_parts(violation);
        report.url = response_url.to_owned();
        report.status_code = status;
        crate::csp_report::report_violation_from(page, &report, &targets, origin.as_deref());
    }
}

/// What `crate::csp_report` sends for `violation`: the report's fields, and where the policy
/// sends it. Its URL and referrer are left empty, so they come from the document. The policy
/// parse already left a `<meta>` policy's `report-uri` out of `report_uris`.
pub(crate) fn report_parts(
    violation: &Violation,
) -> (crate::csp_report::Violation, crate::csp_report::ReportTargets) {
    let report = crate::csp_report::Violation {
        blocked: violation.blocked_url.clone(),
        effective_directive: violation.effective_directive.clone(),
        policy: violation.original_policy.clone(),
        report_only: violation.disposition == Disposition::Report,
        source_file: violation.source_file.clone(),
        sample: violation.sample.clone(),
        status_code: violation.status_code,
        line: violation.line,
        column: violation.column,
        ..Default::default()
    };
    let targets =
        crate::csp_report::ReportTargets { uris: violation.report_uris.clone(), group: violation.report_to.clone() };
    (report, targets)
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

    fn violation(source: PolicySource, report_uris: &[&str], report_to: Option<&str>) -> Violation {
        Violation {
            effective_directive: "img-src".into(),
            violated_directive: "default-src".into(),
            original_policy: "default-src 'none'; report-uri /r".into(),
            disposition: Disposition::Report,
            policy_source: source,
            blocked_url: "https://c.test/before-redirect.png".into(),
            checked_url: "https://d.test/after-redirect.png".into(),
            sample: String::new(),
            source_file: "https://a.test/s.js".into(),
            line: 4,
            column: 9,
            status_code: 200,
            report_uris: report_uris.iter().map(|u| u.to_string()).collect(),
            report_to: report_to.map(str::to_owned),
        }
    }

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
    fn report_parts_map_the_core_violation() {
        let (report, targets) = report_parts(&violation(PolicySource::Header, &["/r"], None));
        assert_eq!(report.blocked, "https://c.test/before-redirect.png");
        assert_eq!(report.effective_directive, "img-src");
        assert_eq!(report.policy, "default-src 'none'; report-uri /r");
        assert!(report.report_only);
        assert_eq!((report.source_file.as_str(), report.line, report.column), ("https://a.test/s.js", 4, 9));
        assert_eq!(report.status_code, 200);
        // The document fills these in at delivery.
        assert!(report.url.is_empty() && report.referrer.is_empty());
        assert_eq!(targets.uris, ["/r"]);
        assert_eq!(targets.group, None);
    }

    #[test]
    fn report_parts_keep_report_to_and_core_meta_filtering() {
        let (_, targets) = report_parts(&violation(PolicySource::Header, &["/r"], Some("g")));
        assert_eq!(targets.group.as_deref(), Some("g"));
        // A <meta> policy reaches here with no report-uri (Policy::parse dropped it).
        let (_, targets) = report_parts(&violation(PolicySource::Meta, &[], Some("g")));
        assert!(targets.uris.is_empty());
        assert_eq!(targets.group.as_deref(), Some("g"));
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

#[cfg(test)]
mod nonceable_tests {
    use super::holds_markup;

    /// content-security-policy/script-src/non-nonceable-elements{,-attr-name}.html: `<script`,
    /// `<style` and `<link` anywhere in a name or value, any ASCII case.
    #[test]
    fn markup_in_attribute_text() {
        for text in ["<script", "<sCRIpt", "<stYlE", "<LinK", "aaa<sCripTbb", "aa<sTylEbbb", "cc<lInKddd"] {
            assert!(holds_markup(text), "{text}");
        }
        for text in ["", "secret-script", "a < b", "<scrip", "<lin k", "script", "link"] {
            assert!(!holds_markup(text), "{text}");
        }
    }
}

/// [`crate::thread_state::reset_thread`]'s part: [`with_handler_location`] restores the slot when
/// it returns, so outside of a call it is empty.
pub(crate) fn thread_leftover() -> Option<&'static str> {
    HANDLER_LOCATION.with(|l| l.borrow().is_some()).then_some("csp::HANDLER_LOCATION")
}
