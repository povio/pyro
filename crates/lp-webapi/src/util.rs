//! Conversions between lp-dom/lp-net results and JS values (WebIDL-ish).

use lp_dom::{DomError, NodeId};
use lp_js::{Args, JsError, JsResult, Scope, Value};

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
