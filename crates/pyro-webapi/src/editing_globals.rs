//! `ClipboardEvent`, `CaretPosition`, `document.caretRangeFromPoint()` and
//! `document.caretPositionFromPoint()`. The two interfaces are made in JS
//! (editing_globals.js) on a realm's first use, as Observable is: a realm
//! that never names them or sees a clipboard event pays for nothing more
//! than the two lazy global accessors.

use std::rc::Rc;

use pyro_dom::{NodeId, NodeType};
use pyro_js::{Args, ClassDef, IdentityKey, JsClass, JsError, JsResult, Object, Persistent, Scope, Value};

use crate::event::{EventData, EventInterface};
use crate::node::NodeData;
use crate::page::PageDom;
use crate::window::WindowHost;

/// Kept on the realm's `navigator` (a hidden property), itself kept per realm.
const KEY: &str = "lpEditingGlobals";

/// Adds the lazy interface names to pyro-browser's `Window` class.
pub(crate) fn define_globals<W>(def: &mut ClassDef<W>)
where
    W: JsClass,
    W::Data: WindowHost,
{
    def.global_value_accessor(
        "ClipboardEvent",
        |s, w| global(s, w.page(), "ClipboardEvent"),
        |s, w, value| replace(s, w.page(), "ClipboardEvent", value),
    );
    def.global_value_accessor(
        "CaretPosition",
        |s, w| global(s, w.page(), "CaretPosition"),
        |s, w, value| replace(s, w.page(), "CaretPosition", value),
    );
}

fn global<'s>(s: &Scope<'s>, page: &Rc<PageDom>, name: &str) -> JsResult<Value<'s>> {
    let value = module(s, page)?.get(name)?;
    replace(s, page, name, value)?;
    Ok(value)
}

fn replace(s: &Scope<'_>, page: &Rc<PageDom>, name: &str, value: Value<'_>) -> JsResult<()> {
    let define = page.define_properties.borrow().get(&s.realm_id()).map(|p| p.get(s));
    let define = define.and_then(|v| v.as_object()).ok_or_else(unavailable)?;
    let desc = s.object();
    desc.set_atom("value", value)?;
    desc.set_atom("writable", s.boolean(true))?;
    desc.set_atom("enumerable", s.boolean(false))?;
    desc.set_atom("configurable", s.boolean(true))?;
    define.call(None, &[s.global().value(), s.string(name), desc.value()])?;
    Ok(())
}

fn unavailable() -> JsError {
    JsError::Type("ClipboardEvent is unavailable".into())
}

/// This realm's module, made on first use.
fn module<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Object<'s>> {
    let navigator = crate::window::navigator(s, page)?.as_object().ok_or_else(unavailable)?;
    if let Some(module) = s.get_hidden(navigator, KEY)?.and_then(|v| v.as_object()) {
        return Ok(module);
    }
    let natives = natives(s)?;
    let root = Persistent::new(s, natives.value());
    let make = s.eval(include_str!("editing_globals.js"), Some("pyro://internal/editing-globals.js"), 1)?;
    let module = make.as_object().ok_or_else(unavailable)?.call(None, &[root.get(s)])?;
    s.set_hidden(navigator, KEY, module)?;
    module.as_object().ok_or_else(unavailable)
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let natives = s.object();
    // A trusted ClipboardEvent's DataTransfer (null when it has none);
    // undefined for any other object.
    let data = s.function("clipboardData", |s, _this, a| {
        let Some(event) = s.unwrap::<crate::event::Event>(a.get(0)) else { return Ok(s.undefined()) };
        if event.iface != EventInterface::ClipboardEvent {
            return Ok(s.undefined());
        }
        let reference = event.fields.borrow().data_transfer.clone();
        Ok(match reference {
            Some(r) => r.get(s)?.value(),
            None => s.null(),
        })
    })?;
    natives.set_atom("clipboardData", data.value())?;
    let is_transfer = s.function("isDataTransfer", |s, _this, a| {
        Ok(s.boolean(s.unwrap::<crate::data_transfer::DataTransfer>(a.get(0)).is_some()))
    })?;
    natives.set_atom("isDataTransfer", is_transfer.value())?;
    let rect = s.function("caretRect", |s, _this, a| {
        let page = crate::target::page_of(s)?;
        let node = crate::node::node_arg(s, a.get(0), "CaretPosition.getClientRect")?;
        let offset = a.get(1).to_number()? as u32;
        crate::layout_api::range_rect(s, &page, (node, offset, node, offset))
    })?;
    natives.set_atom("caretRect", rect.value())?;
    Ok(natives)
}

/// Wraps a native ClipboardEvent in `s`'s realm: an Event wrapper with
/// ClipboardEvent.prototype.
pub(crate) fn wrap_clipboard_event<'s>(s: &Scope<'s>, core: EventData, key: IdentityKey) -> JsResult<Object<'s>> {
    let page = core.page.clone();
    let wrapper = s.wrap::<crate::event::Event>(core, Some(key))?;
    let root = Persistent::new(s, wrapper.value());
    let adopt = module(s, &page)?.get_atom("adopt")?.as_object().ok_or_else(unavailable)?;
    adopt.call(None, &[root.get(s)])?;
    Ok(wrapper)
}

/// The caret position under viewport point (x, y): the character boundary
/// nearest to it in the text of the element there; in an element without
/// laid-out text its start, and beside a replaced element (image, control)
/// the position before or after it.
fn caret_at(page: &PageDom, x: f64, y: f64) -> Option<(NodeId, u32)> {
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    let hit = crate::layout_api::hit_test(page, x, y)?;
    let dom = page.dom.borrow();
    if !crate::editing::is_container(&dom, hit)
        || matches!(crate::editing::local(&dom, hit).as_str(), "textarea" | "select" | "button" | "audio")
    {
        let parent = dom.parent(hit).ok().flatten()?;
        let index = dom.index_in_parent(hit).ok()?;
        let rect = page.layout().bounding_client_rect(&dom, hit);
        let after = x > rect.x + rect.width / 2.0;
        return Some((parent, index + after as u32));
    }
    let layout = page.layout();
    Some(match layout.caret_from_point(&dom, hit, x, y, &|_| true) {
        Some((node, offset, _)) => (node, offset),
        None => (hit, 0),
    })
}

/// A position inside a shadow tree not in `allowed` moves out to its host
/// (the position before it), repeatedly.
fn retarget(page: &PageDom, (mut node, mut offset): (NodeId, u32), allowed: &[NodeId]) -> (NodeId, u32) {
    let dom = page.dom.borrow();
    loop {
        let Ok(root) = dom.root_node(node, false) else { break };
        if dom.node_type(root).ok() != Some(NodeType::DocumentFragment) || allowed.contains(&root) {
            break;
        }
        let Ok(Some(host)) = dom.fragment_host(root) else { break };
        let Ok(Some(parent)) = dom.parent(host) else { break };
        let Ok(index) = dom.index_in_parent(host) else { break };
        (node, offset) = (parent, index);
    }
    (node, offset)
}

/// `document.caretRangeFromPoint(x, y)`: a collapsed Range at the caret
/// position, or null outside the viewport's content.
pub(crate) fn caret_range_from_point<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    let page = n.page();
    if n.id != page.document() || a.len() < 2 {
        return Ok(s.null());
    }
    let (x, y) = (a.get(0).to_number()?, a.get(1).to_number()?);
    let Some(p) = caret_at(page, x, y) else { return Ok(s.null()) };
    let p = retarget(page, p, &[]);
    crate::range::range_value(s, page, p, p)
}

/// `document.caretPositionFromPoint(x, y, options)`: a CaretPosition, or
/// null; positions in shadow trees not listed in `options.shadowRoots`
/// move out to their hosts.
pub(crate) fn caret_position_from_point<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    a.require(2, "Document.caretPositionFromPoint")?;
    let page = n.page();
    let (x, y) = (a.get(0).to_number()?, a.get(1).to_number()?);
    let mut allowed = Vec::new();
    if let Some(options) = a.get(2).as_object() {
        let roots = options.get_atom("shadowRoots")?;
        if let Some(list) = roots.as_object() {
            for i in 0..list.get_atom("length")?.to_number()? as u32 {
                let root = crate::node::node_arg(s, list.get_index(i)?, "Document.caretPositionFromPoint")?;
                allowed.push(root);
            }
        }
    }
    if n.id != page.document() {
        return Ok(s.null());
    }
    let Some(p) = caret_at(page, x, y) else { return Ok(s.null()) };
    let (node, offset) = retarget(page, p, &allowed);
    let make = module(s, page)?.get_atom("caretPosition")?.as_object().ok_or_else(unavailable)?;
    let node = crate::node::wrap_node(s, page, node)?.value();
    make.call(None, &[node, s.number(offset as f64)])
}
