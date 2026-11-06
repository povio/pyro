//! Pointer capture of the mouse (Pointer Events §10): `setPointerCapture`,
//! `releasePointerCapture`, `hasPointerCapture` and "process pending pointer
//! capture", which `crate::input` runs before each mouse pointer event to
//! retarget it. Touch pointers keep their implicit capture (`crate::input`).
//!
//! The state is one `Cell` on the page: without capture, each mouse event
//! pays one read of it.

use std::rc::Rc;

use lp_dom::NodeId;
use lp_js::{Args, JsError, JsResult, Scope, Value};

use crate::event::EventInterface;
use crate::input::{key_of, mouse_event, MouseInput};
use crate::node::NodeData;
use crate::page::PageDom;
use crate::target::fire;

/// Pointer Events capture state of the mouse pointer (pointerId 1): the
/// pending and the current pointer capture target override, and whether a
/// button is down (the "active buttons state" capture needs).
#[derive(Copy, Clone, Debug, Default)]
pub struct MouseCapture {
    pub(crate) pending: Option<NodeId>,
    pub(crate) current: Option<NodeId>,
    pub(crate) buttons_down: bool,
}

const MOUSE_POINTER_ID: i32 = 1;

fn capture_connected(page: &PageDom, el: NodeId) -> bool {
    let dom = page.dom.borrow();
    dom.is_connected(el).unwrap_or(false) && dom.node_document(el).ok() == Some(page.document())
}

/// Pointer Events "process pending pointer capture" for the mouse, then
/// its capture target. Without capture (nearly always) this is one read.
pub(crate) fn capture_target(page: &Rc<PageDom>, m: &MouseInput) -> Option<NodeId> {
    process_pending_capture(page, m);
    page.mouse_capture.get().current
}

fn fire_capture_event(page: &Rc<PageDom>, ty: &str, el: NodeId, m: &MouseInput) {
    if let Some(k) = key_of(page, el) {
        fire(page, k, &mouse_event(page, EventInterface::PointerEvent, ty, m, false));
    }
}

pub(crate) fn process_pending_capture(page: &Rc<PageDom>, m: &MouseInput) {
    let c = page.mouse_capture.get();
    if c.pending.is_none() && c.current.is_none() {
        return;
    }
    // A pending target that left the document is no longer pending.
    if c.pending.is_some_and(|p| !capture_connected(page, p)) {
        page.mouse_capture.set(MouseCapture { pending: None, ..c });
    }
    let c = page.mouse_capture.get();
    if let Some(cur) = c.current.filter(|&cur| Some(cur) != c.pending) {
        page.mouse_capture.set(MouseCapture { current: None, ..c });
        // A capture target removed from the document: its document hears it.
        let at = if capture_connected(page, cur) { cur } else { page.document() };
        fire_capture_event(page, "lostpointercapture", at, m);
    }
    let c = page.mouse_capture.get();
    if let Some(p) = c.pending.filter(|&p| Some(p) != c.current) {
        page.mouse_capture.set(MouseCapture { current: Some(p), ..c });
        fire_capture_event(page, "gotpointercapture", p, m);
    }
}

/// The `pointerId` argument; ids below 1 are never active pointers.
fn pointer_id_arg(a: &Args<'_>, method: &str) -> JsResult<i32> {
    a.require(1, method)?;
    let id = a.get(0).to_number()?;
    if !(id >= 1.0) {
        return Err(JsError::dom("NotFoundError", "No active pointer with the given id is found."));
    }
    Ok(id.min(i32::MAX as f64) as i32)
}

/// `Element.setPointerCapture(pointerId)` for the mouse; other (touch)
/// pointers keep their implicit capture and ignore it.
pub(crate) fn set_pointer_capture<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    let id = pointer_id_arg(&a, "Element.setPointerCapture")?;
    let page = n.page();
    if !page.dom.borrow().is_connected(n.id).unwrap_or(false) {
        return Err(JsError::dom("InvalidStateError", "The element is not connected."));
    }
    let c = page.mouse_capture.get();
    if id == MOUSE_POINTER_ID && c.buttons_down && capture_connected(page, n.id) {
        page.mouse_capture.set(MouseCapture { pending: Some(n.id), ..c });
    }
    Ok(s.undefined())
}

pub(crate) fn release_pointer_capture<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    let id = pointer_id_arg(&a, "Element.releasePointerCapture")?;
    let page = n.page();
    let c = page.mouse_capture.get();
    if id == MOUSE_POINTER_ID && c.pending == Some(n.id) {
        page.mouse_capture.set(MouseCapture { pending: None, ..c });
    }
    Ok(s.undefined())
}

pub(crate) fn has_pointer_capture<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    a.require(1, "Element.hasPointerCapture")?;
    let id = a.get(0).to_number()?;
    Ok(s.boolean(id == MOUSE_POINTER_ID as f64 && n.page().mouse_capture.get().pending == Some(n.id)))
}
