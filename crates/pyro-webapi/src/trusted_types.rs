//! Trusted Types: `trustedTypes` (`TrustedTypePolicyFactory`), `TrustedTypePolicy` and the
//! `TrustedHTML` / `TrustedScript` / `TrustedScriptURL` values, made in JS (trusted_types.js) on a
//! realm's first use, on windows and workers. The CSP `trusted-types` directive of the
//! document's `<meta>` policies limits policy names. Not yet: header-delivered policies, and
//! `require-trusted-types-for` (no sink requires a trusted value).

use pyro_js::{JsResult, Object, Scope};

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
            Ok(page) => meta_policies(&page.dom.borrow(), page.document()),
            Err(_) => Vec::new(),
        };
        Ok(s.boolean(policy_creation_blocked(&policies, &name, duplicate)))
    })?;
    natives.set_atom("policyBlocked", blocked.value())?;
    Ok(natives)
}

/// The Content Security Policies `<meta http-equiv="Content-Security-Policy">` elements of the
/// document's head deliver (enforced). Not yet: policies of response headers.
fn meta_policies(dom: &pyro_dom::Dom, doc: pyro_dom::NodeId) -> Vec<String> {
    let Ok(Some(head)) = dom.head(doc) else { return Vec::new() };
    let Ok(children) = dom.children(head) else { return Vec::new() };
    children
        .into_iter()
        .filter_map(|n| {
            let e = dom.element(n).ok()?;
            if !e.is_html() || &*e.name.local != "meta" {
                return None;
            }
            let equiv = e.attr("http-equiv")?;
            equiv.eq_ignore_ascii_case("content-security-policy").then(|| e.attr("content").map(str::to_owned)).flatten()
        })
        .flat_map(|text| text.split(',').map(str::to_owned).collect::<Vec<_>>())
        .collect()
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
