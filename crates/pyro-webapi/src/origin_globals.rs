//! Origin extraction from globals. Keep already accessible WindowProxy
//! targets behind their normal access check, including after navigation.

use crate::page::PageDom;
use pyro_js::{Scope, Value};

pub(crate) fn global_origin_key(s: &Scope<'_>, value: Value<'_>) -> Option<String> {
    let object = value.as_object()?;
    let caller = s.embedder::<PageDom>()?;
    // A retained frame WindowProxy moves to the new document's realm on
    // navigation. Inspect its current realm even when an old global still
    // has a reference to that same proxy.
    let owner = s.object_realm(object).or_else(|| {
        (!s.realm().is_disposed() && value.strict_equals(s.global().value())).then(|| s.realm())
    })?;
    let home = owner.scope().ok()?;
    let peer = if value.strict_equals(home.global().value()) {
        home.embedder::<PageDom>()?
    } else {
        // Check only actual UA-issued proxies. The lookup ignores arbitrary
        // objects and proxies without inspecting their properties.
        let global = home.accessible_popup_window(object).ok()??;
        let target = home.object_realm(global)?;
        let target_scope = target.scope().ok()?;
        if !global.value().strict_equals(target_scope.global().value()) {
            return None;
        }
        target_scope.embedder::<PageDom>()?
    };
    let key = peer.origin_key();
    (key == caller.origin_key()).then_some(key)
}
