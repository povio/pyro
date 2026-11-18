//! CSS View Transitions (`document.startViewTransition`, `Element.startViewTransition`): the
//! lifecycle of a transition without its capture and animations (view_transition.js), made on
//! a realm's first call. Pages that wait on `finished` or `updateCallbackDone` go on.

use std::rc::Rc;

use pyro_js::{JsError, JsResult, Object, Scope, Value};

use crate::page::PageDom;

/// Kept on the realm's `navigator` (a hidden property), itself kept per realm.
const KEY: &str = "lpViewTransition";

fn unavailable() -> JsError {
    JsError::Type("ViewTransition is unavailable".into())
}

/// This realm's module (`{start, ViewTransition}`), made on first use.
fn module<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Object<'s>> {
    let navigator = crate::window::navigator(s, page)?.as_object().ok_or_else(unavailable)?;
    if let Some(module) = s.get_hidden(navigator, KEY)?.and_then(|v| v.as_object()) {
        return Ok(module);
    }
    let make = s.eval(include_str!("view_transition.js"), Some("pyro://internal/view_transition.js"), 1)?;
    let module = make.as_object().ok_or_else(unavailable)?.call(None, &[s.global().value()])?;
    s.set_hidden(navigator, KEY, module)?;
    module.as_object().ok_or_else(unavailable)
}

/// `startViewTransition(callbackOptions)` on `scope` (a document, or an element for a scoped
/// transition), whose `transitionRoot` is `root`.
pub(crate) fn start<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    scope: Value<'s>,
    root: Value<'s>,
    arg: Value<'s>,
) -> JsResult<Value<'s>> {
    let start = module(s, page)?.get("start")?;
    start.as_object().ok_or_else(unavailable)?.call(None, &[scope, root, arg])
}
