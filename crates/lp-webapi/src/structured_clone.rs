//! Bounded in-realm value cloning. JS owns temporary graph roots; native checks
//! reject proxies and host wrappers without consulting user properties.
use lp_js::{JsError, JsResult, Persistent, Scope};

pub(crate) fn install(s: &Scope<'_>) -> JsResult<()> {
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
    s.global().set("structuredClone", clone)
}
