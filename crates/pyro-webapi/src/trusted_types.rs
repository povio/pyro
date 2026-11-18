//! Trusted Types: `trustedTypes` (`TrustedTypePolicyFactory`), `TrustedTypePolicy` and the
//! `TrustedHTML` / `TrustedScript` / `TrustedScriptURL` values, made in JS (trusted_types.js) on a
//! realm's first use, on windows and workers. The document's CSP (headers and `<meta>`; only
//! the Trusted Types directives are read, no other directive is enforced): `trusted-types`
//! limits policy names, `require-trusted-types-for 'script'` makes the HTML and script sinks
//! take Trusted Types values ([`compliant_string`]).

use std::rc::Rc;

use pyro_js::{JsError, JsResult, Object, Scope, Value};

use crate::page::PageDom;

use crate::lazy_module::LazyModule;

pub(crate) static MODULE: LazyModule = LazyModule {
    key: "lpTrustedTypes",
    source: include_str!("trusted_types.js"),
    url: "pyro://internal/trusted_types.js",
    natives: Some(natives),
};

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
    );
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let natives = s.object();
    let blocked = s.function("policyBlocked", |s, _this, a| {
        let name = crate::util::dom_string(a.get(0))?;
        let duplicate = a.get(1).to_bool();
        let policies = match crate::target::page_of(s) {
            Ok(page) => policies_of(&page),
            Err(_) => Vec::new(),
        };
        Ok(s.boolean(policy_creation_blocked(&policies, &name, duplicate)))
    })?;
    natives.set_atom("policyBlocked", blocked.value())?;
    Ok(natives)
}

/// The Trusted Types directives of the document's policies: its `Content-Security-Policy`
/// headers and the `<meta http-equiv="Content-Security-Policy">` elements that joined its head.
fn policies_of(page: &PageDom) -> Vec<String> {
    page.dom.borrow().trusted_types_policies(page.document()).iter().map(|p| p.to_string()).collect()
}

/// Whether the document's policies have `require-trusted-types-for 'script'`: the script sinks
/// (HTML parsing, script text and URLs, `javascript:` navigation) then take Trusted Types values,
/// or what the default policy makes of a string.
pub(crate) fn requires_for_script(page: &PageDom) -> bool {
    let dom = page.dom.borrow();
    dom.trusted_types_policies(page.document()).iter().any(|policy| {
        policy.split(';').any(|directive| {
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
    let plain = |value: Value<'_>| if null_is_empty { crate::util::null_to_empty(value) } else { crate::util::dom_string(value) };
    if !requires_for_script(page) {
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
        return Err(JsError::Type(format!("Failed to set '{sink}': This document requires '{name}' assignment.").into()));
    }
    crate::util::dom_string(result)
}

/// Trusted Types "Should Trusted Type policy creation be blocked by Content Security Policy?":
/// each policy's `trusted-types` directive must list `name` (or `*`), and allow a name already
/// created only with `'allow-duplicates'`. A directive without names (or `'none'`) allows none.
pub(crate) fn policy_creation_blocked(policies: &[String], name: &str, duplicate: bool) -> bool {
    for policy in policies {
        for directive in policy.split(';') {
            let mut tokens = directive.split_ascii_whitespace();
            if !tokens.next().is_some_and(|d| d.eq_ignore_ascii_case("trusted-types")) {
                continue;
            }
            let values: Vec<&str> = tokens.collect();
            if duplicate && !values.iter().any(|v| v.eq_ignore_ascii_case("'allow-duplicates'")) {
                return true;
            }
            if !values.iter().any(|v| *v == "*" || *v == name) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::policy_creation_blocked;

    #[test]
    fn trusted_types_directive_names_policies() {
        let p = |s: &str| vec![s.to_owned()];
        assert!(!policy_creation_blocked(&[], "a", true));
        assert!(policy_creation_blocked(&p("trusted-types"), "a", false));
        assert!(policy_creation_blocked(&p("trusted-types 'none'"), "default", false));
        assert!(!policy_creation_blocked(&p("script-src 'self'; trusted-types a b"), "b", false));
        assert!(policy_creation_blocked(&p("trusted-types a"), "A", false));
        assert!(!policy_creation_blocked(&p("trusted-types *"), "x", false));
        assert!(policy_creation_blocked(&p("trusted-types *"), "x", true));
        assert!(!policy_creation_blocked(&p("trusted-types * 'allow-duplicates'"), "x", true));
    }
}
