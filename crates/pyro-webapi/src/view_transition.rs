//! CSS View Transitions (`document.startViewTransition`, `Element.startViewTransition`): the
//! per-scope lifecycle and captured rendering state, created on first use.

use std::{cell::Cell, rc::Rc};

use pyro_js::{JsError, JsResult, Scope, Value};

use crate::page::PageDom;

type Boxes = Vec<pyro_dom::geometry::TransitionBox>;
pub(crate) type Captures = rustc_hash::FxHashMap<u64, Capture>;

#[derive(Default)]
pub(crate) struct Quota {
    slots: Cell<usize>,
    pixels: Cell<u64>,
    work: Cell<u64>,
}

pub(crate) struct Capture {
    quota: Rc<Quota>,
    pixels: u64,
    work: u64,
    old: Boxes,
    new: Boxes,
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.quota.slots.set(self.quota.slots.get() - 1);
        self.quota.pixels.set(self.quota.pixels.get() - self.pixels);
        self.quota.work.set(self.quota.work.get() - self.work);
    }
}

fn admit(mut page: Rc<PageDom>) -> Option<Capture> {
    for _ in 0..64 {
        if page.is_detached() || page.is_disposed() { return None; }
        if page.hooks().is_some() {
            match page.container() {
                crate::page::FrameContainer::Frame(parent, _) => { page = parent; continue; }
                crate::page::FrameContainer::Detached => return None,
                crate::page::FrameContainer::Top => (),
            }
        }
        let quota = page.transition_quota.borrow_mut().get_or_insert_with(Default::default).clone();
        if quota.slots.get() >= 16 { return None; }
        quota.slots.set(quota.slots.get() + 1);
        return Some(Capture { quota, pixels: 0, work: 0, old: Vec::new(), new: Vec::new() });
    }
    None
}

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
        if args.get(4).to_bool() {
            if captures.as_ref().is_some_and(|map| map.contains_key(&scope_key)) { return Ok(s.boolean(false)); }
            let Some(record) = admit(page.clone()) else { return Ok(s.boolean(false)); };
            captures.get_or_insert_with(Default::default).insert(scope_key, record);
            return Ok(s.boolean(true));
        }
        let Some(record) = captures.as_mut().and_then(|map| map.get_mut(&scope_key)) else { return Ok(s.boolean(false)); };
        if record.quota.work.get() == 1 << 18 || (old && record.quota.pixels.get() == 1 << 24) { return Ok(s.boolean(false)); }
        let layout = page.layout();
        let dom = page.dom.borrow();
        if page.is_hidden() || !dom.is_connected(root.id).unwrap_or(false) { return Ok(s.boolean(false)); }
        let Some(source) = layout.synced_geometry_source(&dom) else { return Ok(s.boolean(true)); };
        let quota = &record.quota;
        let used_pixels = quota.pixels.replace(1 << 24);
        let used_work = quota.work.replace(1 << 18);
        let mut work = (1 << 18) - used_work;
        let result = source.capture_transition(key, old, (1 << 24) - used_pixels, &mut work);
        quota.pixels.set(used_pixels);
        quota.work.set(used_work);
        let Some(boxes) = result else { return Ok(s.boolean(false)); };
        let charged = (1 << 18) - used_work - work;
        record.work += charged;
        quota.work.set(used_work + charged);
        if old {
            record.pixels = boxes.iter().filter_map(|item| item.rgba.as_ref()).map(|img| img.len() as u64 / 4).sum();
            quota.pixels.set(used_pixels + record.pixels);
            record.old = boxes;
        } else { record.new = boxes; }
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
