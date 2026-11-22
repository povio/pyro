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
use crate::sanitizer_options::ParserOptions;

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
        let result = callback.call(None, &[a.get(1), a.get(2)])?;
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
    // The policies of the realm's document that have Trusted Types directives: [text, report-only].
    let policies = s.function("policies", |s, _this, _a| {
        let mut entries = Vec::new();
        if let Ok(page) = crate::target::page_of(s) {
            let policies: Vec<pyro_dom::CspPolicy> = page.dom.borrow().trusted_types_policies(page.document()).to_vec();
            for policy in policies {
                entries.push(s.array(&[s.string(&policy.text), s.boolean(policy.report_only)])?.value());
            }
        }
        Ok(s.array(&entries)?.value())
    })?;
    natives.set_atom("policies", policies.value())?;
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
    // ttLevel() -> Trusted Types' own eval level (the hooks also run for CSP's).
    let tt = s.function("ttLevel", |s, _this, _a| {
        let level = crate::target::page_of(s).map_or(0, |page| tt_level(&page));
        Ok(s.number(f64::from(level)))
    })?;
    natives.set_atom("ttLevel", tt.value())?;
    // cspEval(code) -> whether the realm's CSP lets `code` compile (`'unsafe-eval'`); a
    // violation is reported otherwise.
    let csp_eval = s.function("cspEval", |s, _this, a| {
        let code = crate::util::dom_string(a.get(0))?;
        let allowed = crate::target::page_of(s).map_or(true, |page| crate::csp::eval_allowed(&page, &code));
        Ok(s.boolean(allowed))
    })?;
    natives.set_atom("cspEval", csp_eval.value())?;
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

/// Whether `el` is an HTML `<script>` (its text and `src` are Trusted Types sinks).
pub(crate) fn is_html_script(page: &PageDom, el: pyro_dom::NodeId) -> bool {
    page.dom.borrow().element(el).is_ok_and(|e| e.is_html() && &*e.name.local == "script")
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
        None if (html || svg || mathml) && !local.bytes().any(|b| b.is_ascii_uppercase()) && {
            // The element's event handler content attributes: GlobalEventHandlers, and
            // WindowEventHandlers on `<body>` and `<frameset>`, and a permission element's own.
            let ty = local.strip_prefix("on").unwrap_or("");
            crate::handlers::GLOBAL.contains(&ty)
                || (html && crate::handlers::element_handler(tag, ty))
                || (html && matches!(tag, "body" | "frameset") && crate::handlers::WINDOW.contains(&ty))
        } =>
        {
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
        let (options, vetted) = parser_options(s, page, options, sink, false)?;
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
    if let Some(wrapper) = s.unwrap::<TrustedValue>(value) {
        if let Some(snapshot) = wrapper.as_ref() {
            return Ok((snapshot.clone(), true));
        }
    }
    let options = ParserOptions::convert(s, value)?;
    if requires_for_script(page) {
        let module = MODULE.module(s, page)?;
        let callback = module.get("defaultParserOptions")?.as_object()
            .ok_or_else(|| JsError::Type("Trusted Types are unavailable".into()))?;
        let result = callback.call(None, &[s.boolean(options.run_scripts), s.string(sink), s.boolean(options.filter.is_some())])?;
        if !result.is_null() && !result.is_undefined() {
            return Ok((ParserOptions::convert(s, result)?, true));
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
