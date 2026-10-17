//! Bounded in-realm value cloning. JS owns temporary graph roots; native checks
//! reject proxies and host wrappers without consulting user properties.
use lp_js::{JsError, JsResult, Persistent, Scope};

pub(crate) fn install(s: &Scope<'_>, page: &crate::page::PageDom) -> JsResult<()> {
    let guard = s.function("cloneObjectGuard", |s, _, args| {
        let value = args.get(0);
        let check: JsResult<bool> = (|| Ok(s.is_host_object(value)? || value.has_proxy_target()?))();
        match check {
            Ok(reject) => Ok(s.boolean(!reject)),
            Err(JsError::Unsupported(_)) => {
                Err(JsError::dom("NotSupportedError", "Native clone classification is unavailable"))
            }
            Err(error) => Err(error),
        }
    })?;
    let guard_root = Persistent::new(s, guard.value());
    let fail = s.function("cloneFailure", |_, _, args| {
        let name = if args.get(1).to_bool() { "NotSupportedError" } else { "DataCloneError" };
        Err(JsError::dom(name, args.get(0).to_string()?))
    })?;
    let fail_root = Persistent::new(s, fail.value());
    let factory =
        s.eval(include_str!("structured_clone.js"), Some("lightningpanda://internal/structured-clone.js"), 1)?;
    let clone = factory.as_object().expect("clone factory").call(None, &[guard_root.get(s), fail_root.get(s)])?;
    // Native users (MessagePort) keep the original, unaffected by page
    // reassignment of `structuredClone`; released with the realm/page.
    page.structured_clones.borrow_mut().insert(s.realm_id(), Persistent::new(s, clone));
    s.global().set("structuredClone", clone)
}

/// StructuredSerialize + deserialize into the same realm, with `options`
/// passed as the `structuredClone` options argument. Transfer lists are
/// rejected with NotSupportedError by the clone itself.
pub(crate) fn clone_value<'s>(
    s: &Scope<'s>,
    value: lp_js::Value<'s>,
    options: lp_js::Value<'s>,
) -> JsResult<lp_js::Value<'s>> {
    let page = crate::target::page_of(s)?;
    let clone = page.structured_clones.borrow().get(&s.realm_id()).map(|p| p.get(s));
    let clone = clone
        .and_then(|v| v.as_object())
        .ok_or_else(|| JsError::dom("NotSupportedError", "structured clone is unavailable in this realm"))?;
    clone.call(None, &[value, options])
}
