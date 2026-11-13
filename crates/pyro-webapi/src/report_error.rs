//! HTML `reportError()` of `WindowOrWorkerGlobalScope`.
use pyro_js::{Args, JsError, JsResult, Object, Persistent, Scope, Value};

/// `self.reportError(e)`: "report an exception" `e` for the receiver's
/// global, as an uncaught exception of a callback would be: the `error`
/// event at the global (`onerror`), then, unless a listener canceled it,
/// the console and CDP `Runtime.exceptionThrown` (Playwright `pageerror`).
/// `window` selects the Window path, which places a primitive at the script
/// that called (`filename`/`lineno`/`colno` of the event).
pub fn report_error<'s>(
    s: &Scope<'s>,
    this: Object<'s>,
    a: &Args<'s>,
    interface: &str,
    window: bool,
) -> JsResult<Value<'s>> {
    if a.len() == 0 {
        return Err(JsError::Type(format!(
            "Failed to execute 'reportError' on '{interface}': 1 argument required, but only 0 present."
        ).into()));
    }
    let value = a.get(0);
    // `this`'s relevant global: `otherWindow.reportError` called with a
    // receiver of another realm reports there.
    if let Some(realm) = s.object_realm(this).filter(|r| r.id() != s.realm_id()) {
        let error = Persistent::new(s, value);
        let other = realm.scope()?;
        report(&other, error.get(&other), window);
    } else {
        report(s, value, window);
    }
    Ok(s.undefined())
}

fn report<'s>(s: &Scope<'s>, value: Value<'s>, window: bool) {
    if window {
        crate::observable::report_exception(s, value);
    } else {
        s.report_exception(&JsError::thrown(Persistent::new(s, value)));
    }
}
