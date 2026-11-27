//! Trusted Types: `trustedTypes` (`TrustedTypePolicyFactory`), `TrustedTypePolicy` and the
//! `TrustedHTML` / `TrustedScript` / `TrustedScriptURL` values, made in JS (trusted_types.js) on a
//! realm's first use, on windows and workers. The document's CSP (headers, report-only headers
//! and `<meta>`; here the Trusted Types directives, the fetch directives are `crate::csp`'s,
//! which fires its violations through this module's `dispatchViolation`): `trusted-types` limits
//! policy names, `require-trusted-types-for 'script'` makes
//! the HTML and script sinks take Trusted Types values ([`compliant_string`]); a violation fires
//! `securitypolicyviolation` (SecurityPolicyViolationEvent) at the document, and blocks only
//! under an enforced policy.

use std::rc::Rc;

use pyro_js::{JsError, JsResult, Object, Scope, Value};

use crate::page::PageDom;

use crate::lazy_module::LazyModule;
use crate::sanitizer_options::{ParseMode, ParserOptions};

pub(crate) static MODULE: LazyModule = LazyModule {
    key: "lpTrustedTypes",
    source: include_str!("trusted_types.js"),
    url: "pyro://internal/trusted_types.js",
    natives: Some(natives),
};

// Engine-private properties are traced with their host object, including when
// the document that created it has gone. A retained policy holds its creation
// realm's callback; retained parse inputs hold only their immutable payload.
struct TrustedValue;
impl pyro_js::JsClass for TrustedValue {
    const NAME: &'static str = "TrustedValue";
    type Data = Option<ParserOptions>;
    type Parent = pyro_js::NoParent;
    fn define(def: &mut pyro_js::ClassDef<Self>) {
        def.not_exposed();
    }
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<TrustedValue>();
}

const HTML_DATA: &str = "lpTrustedHTMLData";
const POLICY_CALLBACK: &str = "lpTrustedParserPolicy";
const OPTIONS_PROTOTYPE: &str = "lpTrustedParserPrototype";

fn wrap_value<'s>(s: &Scope<'s>, prototype: Value<'_>, slot: &'static str, value: Value<'_>) -> JsResult<Object<'s>> {
    let object = s.wrap::<TrustedValue>(None, None)?;
    object.set_prototype(prototype);
    s.set_hidden(object, slot, value)?;
    Ok(object)
}

fn wrap_parser_options<'s>(s: &Scope<'s>, proto: Value<'_>, options: ParserOptions) -> JsResult<Value<'s>> {
    let object = s.wrap::<TrustedValue>(Some(options), None)?;
    object.set_prototype(proto);
    Ok(object.value())
}

/// The Trusted Types interfaces and `trustedTypes` on a window or worker global.
pub(crate) fn define_globals<W>(def: &mut pyro_js::ClassDef<W>)
where
    W: pyro_js::JsClass,
    W::Data: crate::window::WindowHost,
{
    crate::lazy_module::lazy_globals!(
        def,
        MODULE,
        "trustedTypes",
        "TrustedHTML",
        "TrustedScript",
        "TrustedScriptURL",
        "TrustedTypePolicy",
        "TrustedTypePolicyFactory",
        "SecurityPolicyViolationEvent",
    );
}

/// TrustedHTMLParserOptions is exposed only in Window realms.
pub(crate) fn define_parser_options_global<W>(def: &mut pyro_js::ClassDef<W>)
where
    W: pyro_js::JsClass,
    W::Data: crate::window::WindowHost,
{
    crate::lazy_module::lazy_globals!(def, MODULE, "TrustedHTMLParserOptions");
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let natives = s.object();
    natives.set_atom("windowRealm", s.boolean(!crate::worker_scope::is_worker(s)))?;
    let make_value = s.function("makeValue", |s, _this, a| {
        let key = match crate::util::dom_string(a.get(0))?.as_str() {
            "html" => HTML_DATA,
            "policy" => POLICY_CALLBACK,
            _ => return Err(JsError::Type("Invalid Trusted Types value".into())),
        };
        let object = wrap_value(s, a.get(1), key, a.get(2))?;
        if key == POLICY_CALLBACK {
            s.set_hidden(object, OPTIONS_PROTOTYPE, a.get(3))?;
        }
        Ok(object.value())
    })?;
    natives.set_atom("makeValue", make_value.value())?;
    let create_options = s.function("createParserOptions", |s, _this, a| {
        let invalid_receiver = || JsError::Type("Expected a TrustedTypePolicy receiver".into());
        let policy = a.get(0).as_object().ok_or_else(invalid_receiver)?;
        let callback = s.get_hidden(policy, POLICY_CALLBACK)?.and_then(|v| v.as_object()).ok_or_else(invalid_receiver)?;
        // The policy callback's input: the options as a `TrustedHTMLParserOptions` dictionary,
        // `runScripts` and, when given, `sanitizer` as a new Sanitizer (the caller's stays untouched).
        let (given, sanitizer) = ParserOptions::convert_full(s, a.get(1), ParseMode::Unsafe)?;
        let input = s.object();
        input.set_atom("runScripts", s.boolean(given.run_scripts))?;
        if let Some(config) = sanitizer {
            let page = crate::target::page_of(s)?;
            input.set_atom("sanitizer", crate::sanitizer::instance(s, &page, config)?)?;
        }
        let result = callback.call(None, &[input.value(), a.get(2)])?;
        let options = ParserOptions::convert(s, result)?;
        let proto = s.get_hidden(policy, OPTIONS_PROTOTYPE)?.ok_or_else(invalid_receiver)?;
        if let Some(realm) = s.object_realm(policy).filter(|r| r.id() != s.realm_id() && !r.is_disposed()) {
            return Ok(s.rehome(wrap_parser_options(&realm.scope()?, proto, options)?));
        }
        // A removed frame can retain its policy in another window. The
        // callback and public prototype still belong to its original realm.
        wrap_parser_options(s, proto, options)
    })?;
    natives.set_atom("createParserOptions", create_options.value())?;
    // The policies of the realm's document that have Trusted Types directives:
    // [text, report-only, from a `<meta>`].
    let policies = s.function("policies", |s, _this, _a| {
        let mut entries = Vec::new();
        if let Ok(page) = crate::target::page_of(s) {
            let policies: Vec<pyro_dom::CspPolicy> = page.dom.borrow().trusted_types_policies(page.document()).to_vec();
            for policy in policies {
                entries.push(
                    s.array(&[s.string(&policy.text), s.boolean(policy.report_only), s.boolean(policy.meta)])?.value(),
                );
            }
        }
        Ok(s.array(&entries)?.value())
    })?;
    natives.set_atom("policies", policies.value())?;
    // A violation's report delivery (csp_report), after its event: the violated policy's
    // `report-uri` (not a `<meta>` policy's) or `report-to`. Args: policy text, report-only,
    // from a `<meta>`, directive, blocked, sample, document URL, referrer. Workers send none yet
    // (their report delivery belongs with the worker CSP hooks).
    let deliver = s.function("deliverReport", |s, _this, a| {
        if crate::worker_scope::is_worker(s) {
            return Ok(s.undefined());
        }
        if let Ok(page) = crate::target::page_of(s) {
            let text = |i: usize| crate::util::dom_string(a.get(i));
            let violation = crate::csp_report::Violation {
                policy: text(0)?,
                report_only: a.get(1).to_bool(),
                effective_directive: text(3)?,
                blocked: text(4)?,
                sample: text(5)?,
                url: text(6)?,
                referrer: text(7)?,
                ..Default::default()
            };
            let targets = crate::csp_report::ReportTargets::from_policy(&violation.policy, a.get(2).to_bool());
            crate::csp_report::report_violation(&page, &violation, &targets);
        }
        Ok(s.undefined())
    })?;
    natives.set_atom("deliverReport", deliver.value())?;
    // eval and Function through the module's hooks (patched JSC 0035), and the document's
    // enforcement for them.
    let install = s.function("installCompileHooks", |s, _this, a| {
        if let (Some(code_for_eval), Some(can_compile)) = (a.get(0).as_object(), a.get(1).as_object()) {
            s.set_compile_strings_hooks(code_for_eval, can_compile);
            if let Ok(page) = crate::target::page_of(s) {
                let level = eval_level(&page);
                page.tt_eval_level.set(level);
                s.set_trusted_types_enforcement(level as i32);
            }
        }
        Ok(s.undefined())
    })?;
    natives.set_atom("installCompileHooks", install.value())?;
    // isEventHandler(name) -> whether `name` (lowercase, `onclick`) is an event handler content
    // attribute of some element: GlobalEventHandlers, WindowEventHandlers (`<body>`), the
    // Fullscreen and SVG animation handlers and the elements' own, for `getAttributeType`.
    let is_handler = s.function("isEventHandler", |s, _this, a| {
        let name = crate::util::dom_string(a.get(0))?;
        Ok(s.boolean(is_event_handler_name(&name)))
    })?;
    natives.set_atom("isEventHandler", is_handler.value())?;
    // ttLevel() -> Trusted Types' own eval level (the hooks also run for CSP's).
    let tt = s.function("ttLevel", |s, _this, _a| {
        let level = crate::target::page_of(s).map_or(0, |page| tt_level(&page));
        Ok(s.number(f64::from(level)))
    })?;
    natives.set_atom("ttLevel", tt.value())?;
    // cspEval(code) -> whether the realm's CSP lets `code` compile (`'unsafe-eval'`); a
    // violation is reported otherwise, located at the script that called eval or Function.
    // Asked after Trusted Types passed `code` (canCompile): with them enforced, CSP3
    // `'trusted-types-eval'` lets it compile, as for a timer's string handler.
    let csp_eval = s.function("cspEval", |s, _this, a| {
        let code = crate::util::dom_string(a.get(0))?;
        let allowed = crate::target::page_of(s).map_or(true, |page| {
            if tt_level(&page) == 2 && crate::csp::trusted_types_eval_allows(&page) {
                return true;
            }
            let location = if crate::csp::has_policies(&page) { crate::csp::SourceLocation::running(s) } else { None };
            crate::csp::eval_allowed_at(&page, &code, location.as_ref())
        });
        Ok(s.boolean(allowed))
    })?;
    natives.set_atom("cspEval", csp_eval.value())?;
    // sourceLocation() -> [sourceFile, lineNumber, columnNumber] of the page script on the
    // stack (the file stripped for reports), or null when there is none: a Trusted Types
    // violation's location, read when it happens.
    let location = s.function("sourceLocation", |s, _this, _a| match crate::csp::SourceLocation::running(s) {
        Some(at) => Ok(s
            .array(&[s.string(&at.source_file), s.number(f64::from(at.line)), s.number(f64::from(at.column))])?
            .value()),
        None => Ok(s.null()),
    })?;
    natives.set_atom("sourceLocation", location.value())?;
    Ok(natives)
}

/// The eval/Function enforcement the document's policies ask for: Trusted Types' level
/// ([`tt_level`]) or CSP's (`'unsafe-eval'` missing, `crate::csp::eval_level`), the higher.
fn eval_level(page: &PageDom) -> u8 {
    tt_level(page).max(crate::csp::eval_level(page))
}

/// Trusted Types' part of [`eval_level`]: 2 when an enforced policy has
/// `require-trusted-types-for 'script'`, 1 when only report-only ones do, else 0.
fn tt_level(page: &PageDom) -> u8 {
    let dom = page.dom.borrow();
    let mut level = 0;
    for policy in dom.trusted_types_policies(page.document()) {
        let requires = policy.text.split(';').any(|directive| {
            let mut tokens = directive.split_ascii_whitespace();
            tokens.next().is_some_and(|d| d.eq_ignore_ascii_case("require-trusted-types-for"))
                && tokens.any(|v| v == "'script'")
        });
        if requires {
            level = level.max(if policy.report_only { 1 } else { 2 });
        }
    }
    level
}

/// Before a classic script of the page runs in its main realm `s`: eval and Function follow the
/// document's `require-trusted-types-for` (a `<meta>` CSP can add it while the document parses).
/// Makes the Trusted Types module (which installs the hooks) when a policy first asks.
pub fn sync_compile_strings(s: &Scope<'_>, page: &Rc<PageDom>) {
    if page.dom.borrow().csp_policies(page.document()).is_empty() && page.tt_eval_level.get() == 0 {
        return;
    }
    // WebAssembly compilation follows the same policies (`'wasm-unsafe-eval'`).
    crate::csp::sync_wasm_gate(s, page);
    let level = eval_level(page);
    if level == page.tt_eval_level.get() {
        return;
    }
    if MODULE.existing(s, page).ok().flatten().is_none() {
        // Loading it installs the hooks with the current level.
        let _ = MODULE.module(s, page);
        return;
    }
    page.tt_eval_level.set(level);
    s.set_trusted_types_enforcement(level as i32);
}

/// HTML "timer initialization steps" for the string handler `handler` of `setTimeout` /
/// `setInterval` (`sink`: "Window setTimeout", "WorkerGlobalScope setInterval", ...), checked
/// when the timer is set, as Chrome: Trusted Types first (a TrustedScript, else what the default
/// policy makes of the string; a TypeError when an enforced policy rejects it), then CSP's
/// string compilation check (`crate::csp::eval_allowed`, its violation reported now even when
/// the timer is cleared right away). `None`: blocked, the timer is not set (it returns 0).
pub fn timer_handler_source(s: &Scope<'_>, page: &Rc<PageDom>, handler: Value<'_>, sink: &str) -> JsResult<Option<String>> {
    let code = compliant_string(s, page, Kind::Script, handler, sink, false)?;
    // CSP3 'trusted-types-eval': with Trusted Types enforced, what passed them compiles.
    if tt_level(page) == 2 && crate::csp::trusted_types_eval_allows(page) {
        return Ok(Some(code));
    }
    // Its violation is located at the script that set the timer.
    let location = if crate::csp::has_policies(page) { crate::csp::SourceLocation::running(s) } else { None };
    Ok(crate::csp::eval_allowed_at(page, &code, location.as_ref()).then_some(code))
}

/// Whether the document's policies have `require-trusted-types-for 'script'`: the script sinks
/// (HTML parsing, script text and URLs, `javascript:` navigation) then take Trusted Types values,
/// or what the default policy makes of a string.
pub(crate) fn requires_for_script(page: &PageDom) -> bool {
    requires_for_script_in(page, page.document())
}

/// [`requires_for_script`] for the document `doc` (an element's node document, which an
/// adopted element changed).
pub(crate) fn requires_for_script_in(page: &PageDom, doc: pyro_dom::NodeId) -> bool {
    let dom = page.dom.borrow();
    dom.trusted_types_policies(doc).iter().any(|policy| {
        policy.text.split(';').any(|directive| {
            let mut tokens = directive.split_ascii_whitespace();
            tokens.next().is_some_and(|d| d.eq_ignore_ascii_case("require-trusted-types-for"))
                && tokens.any(|v| v == "'script'")
        })
    })
}

/// The script a `javascript:` URL runs (`source`: its percent-decoded script), through the
/// document's Trusted Types: as it is unless the document requires them, else what its default
/// policy makes of it at the "Location href" sink; `None` (the navigation is not run) when that
/// fails or the policy throws.
pub fn javascript_url_script(s: &Scope<'_>, page: &Rc<PageDom>, source: &str) -> Option<String> {
    if !requires_for_script(page) {
        return Some(source.to_owned());
    }
    match compliant_string(s, page, Kind::Script, s.string(source), "Location href", false) {
        Ok(script) => Some(script),
        Err(e) => {
            // A policy that throws reports its exception; a missing one is a violation only.
            if !matches!(e, JsError::Type(_)) {
                s.report_exception(&e);
            }
            None
        }
    }
}

/// Trusted Types "prepare the script text" (slot value verification) for the script element
/// `el` whose child text content is `text`, as "prepare the script element" runs it: the text as
/// it is while it is the script's [[ScriptText]] (`Dom::script_text`: what its parser or a
/// trusted sink gave it) or the document does not require Trusted Types; else what the default
/// policy makes of it at the "HTMLScriptElement text" or "SVGScriptElement text" sink. `None`
/// (the script is not prepared) when that fails or the policy throws.
pub fn script_source(s: &Scope<'_>, page: &Rc<PageDom>, el: pyro_dom::NodeId, text: &str) -> Option<String> {
    let (doc, svg) = {
        let dom = page.dom.borrow();
        let doc = dom.node_document(el).unwrap_or_else(|_| page.document());
        if !requires_for_script_in(page, doc) || dom.script_text(el) == text {
            return Some(text.to_owned());
        }
        (doc, dom.element(el).is_ok_and(|e| !e.is_html()))
    };
    let sink = if svg { "SVGScriptElement text" } else { "HTMLScriptElement text" };
    match compliant_string_in(s, page, doc, Kind::Script, s.string(text), sink, false) {
        Ok(script) => Some(script),
        Err(e) => {
            // A policy that throws reports its exception; a missing one is a violation only.
            if !matches!(e, JsError::Type(_)) {
                s.report_exception(&e);
            }
            None
        }
    }
}

/// Whether the script element `el` needs [`script_source`] before it is prepared with `text`:
/// its document requires Trusted Types and `text` is not its [[ScriptText]]. No JavaScript runs.
pub fn script_source_unverified(page: &PageDom, el: pyro_dom::NodeId, text: &str) -> bool {
    let dom = page.dom.borrow();
    let doc = dom.node_document(el).unwrap_or_else(|_| page.document());
    drop(dom);
    requires_for_script_in(page, doc) && page.dom.borrow().script_text(el) != text
}

/// Whether `local` (case-sensitive: `onclick`) names an event handler content attribute of some
/// element: GlobalEventHandlers, WindowEventHandlers (not only on `<body>`), the Fullscreen,
/// SVG animation, permission element and canvas handlers. Trusted Types decides by the name alone
/// (`getAttributeType` and `setAttribute` agree), not by the element it is set on.
pub(crate) fn is_event_handler_name(local: &str) -> bool {
    let Some(ty) = local.strip_prefix("on") else { return false };
    crate::handlers::GLOBAL.contains(&ty)
        || crate::handlers::WINDOW.contains(&ty)
        || crate::handlers::FULLSCREEN.contains(&ty)
        || crate::handlers::PERMISSION_ELEMENT.contains(&ty)
        || crate::handlers::CANVAS_ELEMENT.contains(&ty)
        || crate::handlers::SVG_ANIMATION_ATTRIBUTES.iter().any(|(name, _)| *name == ty)
}

/// Trusted Types "get Trusted Types-compliant attribute value": the kind an attribute of `el`
/// takes and its sink name (an event handler content attribute of the element: TrustedScript,
/// "Element onclick"; `iframe srcdoc`: TrustedHTML; HTML `script src` and SVG `script href`:
/// TrustedScriptURL), `None` for any other. `local` is compared as given (setAttribute lowercases
/// it first for an HTML element; setAttributeNS does not).
pub(crate) fn attribute_kind(page: &PageDom, el: pyro_dom::NodeId, local: &str, ns: Option<&str>) -> Option<(Kind, String)> {
    let dom = page.dom.borrow();
    let e = dom.element(el).ok()?;
    let tag = &*e.name.local;
    let html = e.is_html();
    let svg = e.namespace() == &markup5ever::ns!(svg);
    let mathml = e.namespace() == &markup5ever::ns!(mathml);
    match ns.filter(|ns| !ns.is_empty()) {
        // An event handler content attribute of any HTML, SVG or MathML element: the name alone
        // decides, whatever the element (Chrome, and `getAttributeType("dummy", "onunload")`).
        None if (html || svg || mathml) && is_event_handler_name(local) => {
            Some((Kind::Script, format!("Element {local}")))
        }
        None if html && tag == "iframe" && local == "srcdoc" => Some((Kind::Html, "HTMLIFrameElement srcdoc".into())),
        None if html && tag == "script" && local == "src" => Some((Kind::ScriptUrl, "HTMLScriptElement src".into())),
        None if svg && tag == "script" && local == "href" => Some((Kind::ScriptUrl, "SVGScriptElement href".into())),
        Some("http://www.w3.org/1999/xlink") if svg && tag == "script" && local == "href" => {
            Some((Kind::ScriptUrl, "SVGScriptElement href".into()))
        }
        _ => None,
    }
}

/// An attribute value set through `setAttribute`/`setAttributeNS`, made compliant when the
/// attribute is a Trusted Types sink and the document requires Trusted Types.
pub(crate) fn attribute_value(
    s: &Scope<'_>,
    page: &Rc<PageDom>,
    el: pyro_dom::NodeId,
    local: &str,
    ns: Option<&str>,
    value: Value<'_>,
) -> JsResult<Option<String>> {
    // The element's node document's policies (an adopted element follows its new document).
    let Ok(doc) = page.dom.borrow().node_document(el) else { return Ok(None) };
    if !requires_for_script_in(page, doc) {
        return Ok(None);
    }
    match attribute_kind(page, el, local, ns) {
        Some((kind, sink)) => compliant_string_in(s, page, doc, kind, value, &sink, false).map(Some),
        None => Ok(None),
    }
}

/// Converted (TrustedHTML or DOMString) argument, before a positional HTML
/// method reads its following options dictionary or invokes the default policy.
pub(crate) enum HtmlInput {
    Trusted(String),
    String(String),
}

impl HtmlInput {
    pub(crate) fn convert(s: &Scope<'_>, value: Value<'_>) -> JsResult<Self> {
        if let Some(object) = value.as_object() {
            if let Some(data) = s.get_hidden(object, HTML_DATA)? {
                return Ok(Self::Trusted(crate::util::dom_string(data)?));
            }
        }
        Ok(Self::String(crate::util::dom_string(value)?))
    }

    /// Resolve parser options before applying the HTML policy. A branded
    /// options object or a successful options policy approves this parse.
    pub(crate) fn for_html_insertion(
        self,
        s: &Scope<'_>,
        page: &Rc<PageDom>,
        options: Value<'_>,
        sink: &str,
    ) -> JsResult<(String, ParserOptions)> {
        self.for_html_insertion_as(s, page, options, sink, ParseMode::Unsafe)
    }

    /// [`Self::for_html_insertion`] for the method `mode` names: a safe method's (`setHTML`,
    /// `appendHTML`, ...) options carry no `runScripts` and always filter (the default sanitizer,
    /// the built-in safe baseline).
    pub(crate) fn for_html_insertion_as(
        self,
        s: &Scope<'_>,
        page: &Rc<PageDom>,
        options: Value<'_>,
        sink: &str,
        mode: ParseMode,
    ) -> JsResult<(String, ParserOptions)> {
        // The safe methods are not Trusted Types sinks: the sanitizer makes their markup safe, so
        // neither the default policy's `createHTML` nor its `createParserOptions` is consulted.
        if mode == ParseMode::Safe {
            let options = ParserOptions::convert_with(s, options, mode)?;
            return Ok((match self { Self::Trusted(html) | Self::String(html) => html }, options));
        }
        let (options, vetted) = parser_options_with(s, page, options, sink, false, mode)?;
        if vetted {
            return Ok((match self { Self::Trusted(html) | Self::String(html) => html }, options));
        }
        Ok((self.compliant(s, page, sink)?, options))
    }

    pub(crate) fn compliant(self, s: &Scope<'_>, page: &Rc<PageDom>, sink: &str) -> JsResult<String> {
        match self {
            Self::Trusted(html) => Ok(html),
            Self::String(html) if requires_for_script(page) => {
                compliant_string(s, page, Kind::Html, s.string(&html), sink, false)
            }
            Self::String(html) => Ok(html),
        }
    }
}

/// Convert and vet the complete options snapshot before parsing. A streaming
/// sink has no separate HTML argument on which to enforce a missing policy.
pub(crate) fn parser_options(
    s: &Scope<'_>, page: &Rc<PageDom>, value: Value<'_>, sink: &str, streaming: bool,
) -> JsResult<(ParserOptions, bool)> {
    parser_options_with(s, page, value, sink, streaming, ParseMode::Unsafe)
}

/// [`parser_options`] for the method `mode` names: a safe method does not read `runScripts` and
/// the built-in safe baseline applies to every sanitizer.
pub(crate) fn parser_options_with(
    s: &Scope<'_>, page: &Rc<PageDom>, value: Value<'_>, sink: &str, streaming: bool, mode: ParseMode,
) -> JsResult<(ParserOptions, bool)> {
    parser_options_inner(s, page, value, sink, streaming, mode, None)
}

/// The markup and parser options of a legacy HTML sink (`innerHTML`, `outerHTML`,
/// `insertAdjacentHTML`, `createContextualFragment`), which take no options argument: a document
/// that requires Trusted Types asks the default policy's `createParserOptions` for them, with the
/// sink's name and `{runScripts: initial_run_scripts}` as input (false for the setters, true for
/// `createContextualFragment`, whose scripts run once inserted); what it returns (a sanitizer, or
/// `runScripts`) applies even to a `TrustedHTML` value. A policy that does not return options (none
/// at all, or no `createParserOptions`) leaves the sink as it was; one that returns nothing is a
/// violation, reported before the markup's own. `null_is_empty`: a `null` value is the empty string.
pub(crate) fn legacy_html(
    s: &Scope<'_>,
    page: &Rc<PageDom>,
    value: Value<'_>,
    sink: &str,
    null_is_empty: bool,
    initial_run_scripts: bool,
) -> JsResult<(String, ParserOptions)> {
    let defaults = ParserOptions { run_scripts: initial_run_scripts, ..ParserOptions::default() };
    if !requires_for_script(page) {
        let html = if null_is_empty { crate::util::null_to_empty(value) } else { crate::util::dom_string(value) }?;
        return Ok((html, defaults));
    }
    let input = if null_is_empty && value.is_null() { HtmlInput::String(String::new()) } else { HtmlInput::convert(s, value)? };
    let (options, vetted) = parser_options_inner(s, page, s.undefined(), sink, false, ParseMode::Unsafe, Some(initial_run_scripts))?;
    if vetted {
        return Ok((match input { HtmlInput::Trusted(html) | HtmlInput::String(html) => html }, options));
    }
    Ok((input.compliant(s, page, sink)?, options))
}

/// [`parser_options_with`]; `initial_run_scripts` is the `runScripts` the default policy's callback
/// is given and a policy-less parse uses when `value` has none to say (a legacy sink's).
fn parser_options_inner(
    s: &Scope<'_>,
    page: &Rc<PageDom>,
    value: Value<'_>,
    sink: &str,
    streaming: bool,
    mode: ParseMode,
    initial_run_scripts: Option<bool>,
) -> JsResult<(ParserOptions, bool)> {
    if mode == ParseMode::Safe {
        // Not a Trusted Types sink (see `for_html_insertion_as`): a plain `SetHTMLOptions` conversion.
        return Ok((ParserOptions::convert_with(s, value, mode)?, false));
    }
    if let Some(wrapper) = s.unwrap::<TrustedValue>(value) {
        if let Some(snapshot) = wrapper.as_ref() {
            return Ok((snapshot.clone(), true));
        }
    }
    let (mut options, sanitizer) = ParserOptions::convert_full(s, value, mode)?;
    if let Some(run_scripts) = initial_run_scripts {
        options.run_scripts = run_scripts;
    }
    if requires_for_script(page) {
        let module = MODULE.module(s, page)?;
        let callback = module.get("defaultParserOptions")?.as_object()
            .ok_or_else(|| JsError::Type("Trusted Types are unavailable".into()))?;
        // The callback's input holds the sanitizer as a new Sanitizer (a copy it may change).
        let sanitizer = match sanitizer {
            Some(config) => crate::sanitizer::instance(s, page, config)?,
            None => s.undefined(),
        };
        let result = callback.call(None, &[s.boolean(options.run_scripts), s.string(sink), sanitizer])?;
        if !result.is_null() && !result.is_undefined() {
            return Ok((ParserOptions::convert_with(s, result, mode)?, true));
        }
        if (streaming || options.needs_vetting || result.is_null()) && sink_violation(s, page, sink, "")? {
            return Err(JsError::Type("This document requires trusted HTML parser options".into()));
        }
    }
    Ok((options, false))
}

/// The Trusted Types a sink takes.
#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Html,
    Script,
    ScriptUrl,
}

/// Trusted Types "get Trusted Type compliant string" for `value` at `sink` (`"Element innerHTML"`):
/// the value as it is when the document does not require Trusted Types; else the data of a
/// Trusted* value of the kind, else what the default policy makes of the string; a TypeError
/// when there is no default policy or it gives nothing.
/// `null_is_empty`: the sink's IDL type treats `null` as the empty string (`[LegacyNullToEmptyString]`).
pub(crate) fn compliant_string(
    s: &Scope<'_>,
    page: &Rc<PageDom>,
    kind: Kind,
    value: Value<'_>,
    sink: &str,
    null_is_empty: bool,
) -> JsResult<String> {
    compliant_string_in(s, page, page.document(), kind, value, sink, null_is_empty)
}

/// [`compliant_string`] under the policies of the document `doc`.
pub(crate) fn compliant_string_in(
    s: &Scope<'_>,
    page: &Rc<PageDom>,
    doc: pyro_dom::NodeId,
    kind: Kind,
    value: Value<'_>,
    sink: &str,
    null_is_empty: bool,
) -> JsResult<String> {
    let plain = |value: Value<'_>| if null_is_empty { crate::util::null_to_empty(value) } else { crate::util::dom_string(value) };
    if !requires_for_script_in(page, doc) {
        return plain(value);
    }
    let module = MODULE.module(s, page)?;
    let (tag, method, name) = match kind {
        Kind::Html => ("html", "createHTML", "TrustedHTML"),
        Kind::Script => ("script", "createScript", "TrustedScript"),
        Kind::ScriptUrl => ("scriptURL", "createScriptURL", "TrustedScriptURL"),
    };
    let call = |name: &str, args: &[Value<'_>]| -> JsResult<Value<'_>> {
        let f = module.get(name)?.as_object().ok_or_else(|| JsError::Type("Trusted Types are unavailable".into()))?;
        f.call(None, args)
    };
    let data = call("trustedData", &[s.string(tag), value])?;
    if !data.is_null() {
        return crate::util::dom_string(data);
    }
    let input = plain(value)?;
    let result = call("defaultPolicyValue", &[s.string(method), s.string(&input), s.string(name), s.string(sink)])?;
    if result.is_null() || result.is_undefined() {
        // A violation, reported to every policy that requires Trusted Types; blocked when one
        // of them is enforced (report-only ones let the string through).
        let enforced = sink_violation(s, page, sink, &input)?;
        if enforced {
            return Err(JsError::Type(format!("Failed to set '{sink}': This document requires '{name}' assignment.").into()));
        }
        return Ok(input);
    }
    crate::util::dom_string(result)
}

/// Report an unvetted value to applicable CSP policies and return whether any blocks it.
pub(crate) fn sink_violation(s: &Scope<'_>, page: &Rc<PageDom>, sink: &str, input: &str) -> JsResult<bool> {
    let module = MODULE.module(s, page)?;
    let report = module.get("sinkViolation")?.as_object()
        .ok_or_else(|| JsError::Type("Trusted Types are unavailable".into()))?;
    Ok(report.call(None, &[s.string(sink), s.string(input)])?.to_bool())
}
