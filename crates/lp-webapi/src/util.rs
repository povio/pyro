//! Conversions between lp-dom/lp-net results and JS values (WebIDL-ish).

use lp_dom::{DomError, NodeId};
use lp_js::{Args, IdentityKey, JsError, JsResult, Object, Scope, Value, WeakRealm};

/// A wrapper found again through its realm's identity map. A native payload
/// keeps JS values its wrapper owns in hidden properties on that wrapper,
/// which the GC traces, rather than in a `Persistent`, which is a root the GC
/// cannot see through: a protected value that reaches back to the wrapper
/// would keep both, its realm and its page alive after the page closes.
#[derive(Clone)]
pub(crate) struct WrapperRef {
    realm: WeakRealm,
    identity: IdentityKey,
}

impl WrapperRef {
    pub(crate) fn new(s: &Scope<'_>, identity: IdentityKey) -> WrapperRef {
        WrapperRef { realm: s.realm().downgrade(), identity }
    }

    /// The wrapper, from a call whose receiver (or argument) is that wrapper,
    /// which keeps it alive.
    pub(crate) fn get<'s>(&self, s: &Scope<'s>) -> JsResult<Object<'s>> {
        let realm = self.realm.upgrade().ok_or(JsError::ContextDestroyed)?;
        let owner = realm.scope()?;
        let wrapper = owner.lookup_identity(self.identity).ok_or(JsError::ContextDestroyed)?;
        // SAFETY: the caller's live receiver traces this same-VM wrapper.
        Ok(unsafe { Value::from_raw(s.ctx(), wrapper.value().as_raw()) }.as_object().expect("wrapper"))
    }

    pub(crate) fn hidden<'s>(&self, s: &Scope<'s>, key: &str) -> JsResult<Option<Value<'s>>> {
        s.get_hidden(self.get(s)?, key)
    }
}

/// Maps a DOM error to the JS exception the bindings throw.
pub fn dom_err(e: DomError) -> JsError {
    match e {
        DomError::Exception { kind, message } => JsError::dom(kind.name(), message),
        DomError::Type(m) => JsError::Type(m.to_owned()),
        DomError::StaleNode => JsError::dom("InvalidStateError", "The node is no longer available."),
    }
}

pub trait DomResultExt<T> {
    fn js(self) -> JsResult<T>;
}
impl<T> DomResultExt<T> for Result<T, DomError> {
    fn js(self) -> JsResult<T> {
        self.map_err(dom_err)
    }
}

/// WebIDL `DOMString` conversion (ToString; symbols throw).
pub fn dom_string(v: Value<'_>) -> JsResult<String> {
    v.to_string()
}

/// `DOMString?`: null/undefined -> None.
pub fn opt_dom_string(v: Value<'_>) -> JsResult<Option<String>> {
    if v.is_nullish() {
        Ok(None)
    } else {
        v.to_string().map(Some)
    }
}

/// Required string argument with a WebIDL-style arity error.
pub fn arg_string(a: &Args<'_>, i: usize, what: &str) -> JsResult<String> {
    a.require(i + 1, what)?;
    dom_string(a.get(i))
}

/// `[LegacyNullToEmptyString]` (innerHTML, textContent setter semantics).
pub fn null_to_empty(v: Value<'_>) -> JsResult<String> {
    if v.is_null() {
        Ok(String::new())
    } else {
        v.to_string()
    }
}

pub fn opt_node<'s>(s: &Scope<'s>, page: &crate::PageDom, id: Option<NodeId>) -> JsResult<Value<'s>> {
    match id {
        Some(id) => Ok(crate::node::wrap_node(s, page, id)?.value()),
        None => Ok(s.null()),
    }
}

/// ToNumber -> unsigned long (WebIDL modulo 2^32).
pub fn to_u32(v: Value<'_>) -> JsResult<u32> {
    let n = v.to_number()?;
    if !n.is_finite() {
        return Ok(0);
    }
    Ok((n.trunc() as i64).rem_euclid(1 << 32) as u32)
}

/// ToNumber -> long.
pub fn to_i32(v: Value<'_>) -> JsResult<i32> {
    Ok(to_u32(v)? as i32)
}

/// ToNumber -> a scroll offset in CSS px. CSSOM View takes an unrestricted
/// double; Chromium (device scale 1) rounds to the nearest pixel, half up,
/// treats non-finite values as 0 and clamps huge ones (no WebIDL wrapping).
pub fn to_scroll_offset(v: Value<'_>) -> JsResult<i32> {
    let n = v.to_number()?;
    if !n.is_finite() {
        return Ok(0);
    }
    Ok((n + 0.5).floor().clamp(i32::MIN as f64, i32::MAX as f64) as i32)
}
