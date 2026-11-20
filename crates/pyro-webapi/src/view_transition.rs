//! CSS View Transitions (`document.startViewTransition`, `Element.startViewTransition`): the
//! lifecycle of a transition without its capture and animations (view_transition.js), made on
//! a realm's first call. Pages that wait on `finished` or `updateCallbackDone` go on.

use std::rc::Rc;

use pyro_js::{JsError, JsResult, Scope, Value};

use crate::page::PageDom;

pub(crate) static MODULE: crate::lazy_module::LazyModule = crate::lazy_module::LazyModule {
    key: "lpViewTransition",
    source: include_str!("view_transition.js"),
    url: "pyro://internal/view_transition.js",
    natives: Some(natives),
};

fn natives<'s>(s: &Scope<'s>) -> JsResult<pyro_js::Object<'s>> {
    s.function("captureValid", |s, _, args| {
        let Some(root) = s.unwrap::<crate::node::Node>(args.get(0)) else { return Ok(s.boolean(false)) };
        let page = root.page();
        let layout = page.layout();
        let dom = page.dom.borrow();
        let valid = !page.is_hidden() && dom.is_connected(root.id).unwrap_or(false) && layout.synced_geometry_source(&dom)
            .is_none_or(|source| source.view_transition_names_valid(pyro_dom::snapshot::render_key(root.id)));
        Ok(s.boolean(valid))
    })
}

fn unavailable() -> JsError {
    JsError::Type("ViewTransition is unavailable".into())
}

pub(crate) fn active<'s>(s: &Scope<'s>, node: &crate::node::NodeData) -> JsResult<Value<'s>> {
    let Some(module) = MODULE.existing(s, node.page())? else { return Ok(s.null()) };
    let scope = crate::node::wrap_node(s, node.page(), node.id)?.value();
    module.get("active")?.as_object().ok_or_else(unavailable)?.call(None, &[scope])
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
    MODULE.helper(s, page, "start", &[scope, root, arg, s.boolean(page.is_hidden())])
}
