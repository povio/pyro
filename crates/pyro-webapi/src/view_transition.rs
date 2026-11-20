//! CSS View Transitions (`document.startViewTransition`, `Element.startViewTransition`): the
//! per-scope lifecycle and captured rendering state, created on first use.

use std::rc::Rc;

use pyro_js::{JsError, JsResult, Scope, Value};

use crate::page::PageDom;

type Boxes = Vec<pyro_dom::geometry::TransitionBox>;
pub(crate) type Captures = rustc_hash::FxHashMap<u64, (Boxes, Boxes)>;

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
        let key = pyro_dom::snapshot::render_key(root.id);
        let Some(scope) = s.unwrap::<crate::node::Node>(args.get(3)) else { return Ok(s.boolean(false)) };
        let scope_key = pyro_dom::snapshot::render_key(scope.id);
        let mut captures = page.transition_captures.borrow_mut();
        if args.get(2).to_bool() {
            if let Some(map) = captures.as_mut() { map.remove(&scope_key); if map.is_empty() { *captures = None; } }
            return Ok(s.boolean(true));
        }
        let old = args.get(1).to_bool();
        let layout = page.layout();
        let dom = page.dom.borrow();
        if page.is_hidden() || !dom.is_connected(root.id).unwrap_or(false) { return Ok(s.boolean(false)); }
        let Some(source) = layout.synced_geometry_source(&dom) else { return Ok(s.boolean(true)); };
        let map = captures.get_or_insert_with(Default::default);
        if old && map.len() >= 16 { return Ok(s.boolean(false)); }
        let used = map.values().flat_map(|(old, _)| old).filter_map(|item| item.rgba.as_ref()).map(|img| img.len() as u64 / 4).sum::<u64>();
        let Some(boxes) = source.capture_transition(key, old, (1u64 << 24).saturating_sub(used)) else { return Ok(s.boolean(false)); };
        if old { map.insert(scope_key, (boxes, Vec::new())); }
        else if let Some(record) = map.get_mut(&scope_key) { record.1 = boxes; }
        Ok(s.boolean(true))
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
