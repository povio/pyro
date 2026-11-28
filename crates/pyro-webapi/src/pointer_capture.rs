//! Pointer capture of the mouse (Pointer Events §10): `setPointerCapture`,
//! `releasePointerCapture`, `hasPointerCapture` and "process pending pointer
//! capture", which `crate::input` runs before each mouse pointer event to
//! retarget it. Touch pointers keep their implicit capture (`crate::input`).
//!
//! The state is one `Cell` on the page: without capture, each mouse event
//! pays one read of it.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::NodeId;
use pyro_js::{Args, JsError, JsResult, Scope, Value};

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
    /// The `pointerId` of the mouse-like pointer driving the page (the mouse's, or a pen's); 0
    /// until the first mouse input.
    pub(crate) pointer_id: i32,
}

const MOUSE_POINTER_ID: i32 = 1;

/// The `pointerId` of the mouse-like pointer (mouse or pen) of `page`.
fn mouse_pointer_id(page: &PageDom) -> i32 {
    match page.mouse_capture.get().pointer_id {
        0 => MOUSE_POINTER_ID,
        id => id,
    }
}

/// The pending and the current pointer capture target of a touch pointer.
#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct TouchCapture {
    pub(crate) pending: Option<NodeId>,
    pub(crate) current: Option<NodeId>,
}

thread_local! {
    /// The touches down: (page address, pointerId, capture state). A touch sequence is a few
    /// entries long at most, so a list.
    static TOUCH_CAPTURE: RefCell<Vec<(usize, i32, TouchCapture)>> = const { RefCell::new(Vec::new()) };
}

/// This module's part of [`crate::thread_state::reset_thread`]: a touch sequence of a closed page
/// is keyed by an address a later page may reuse, so the list is emptied.
pub(crate) fn thread_leftover() -> Option<&'static str> {
    TOUCH_CAPTURE.with(|t| t.borrow_mut().clear());
    None
}

fn page_key(page: &PageDom) -> usize {
    page as *const PageDom as usize
}

/// A touch pointer comes down on `target`: implicit capture is pending for it.
pub(crate) fn touch_capture_begin(page: &PageDom, id: i32, target: NodeId) {
    let key = page_key(page);
    TOUCH_CAPTURE.with(|t| {
        let mut t = t.borrow_mut();
        t.retain(|(p, i, _)| !(*p == key && *i == id));
        t.push((key, id, TouchCapture { pending: Some(target), current: None }));
    });
}

/// The capture state of touch pointer `id` while it is down.
pub(crate) fn touch_capture(page: &PageDom, id: i32) -> Option<TouchCapture> {
    let key = page_key(page);
    TOUCH_CAPTURE.with(|t| t.borrow().iter().find(|(p, i, _)| *p == key && *i == id).map(|(_, _, c)| *c))
}

/// Stores the capture state of touch pointer `id` (only while it is down).
pub(crate) fn set_touch_capture(page: &PageDom, id: i32, state: TouchCapture) {
    let key = page_key(page);
    TOUCH_CAPTURE.with(|t| {
        if let Some(entry) = t.borrow_mut().iter_mut().find(|(p, i, _)| *p == key && *i == id) {
            entry.2 = state;
        }
    });
}

/// The touch pointer `id` went away: its capture state with it.
pub(crate) fn touch_capture_end(page: &PageDom, id: i32) {
    let key = page_key(page);
    TOUCH_CAPTURE.with(|t| t.borrow_mut().retain(|(p, i, _)| !(*p == key && *i == id)));
}

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

/// Pointer Events §4.1.3.2 "process pending pointer capture" for the mouse, with the boundary
/// events §4.1.1 puts around it (Chrome): a new capture target is entered (out/leave at what was
/// hovered, over/enter at the target) before it hears `gotpointercapture`; after
/// `lostpointercapture` the pointer is over whatever is under it again (out/leave at the old
/// capture target, over/enter there).
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
    // What the pending target was when this began: a `lostpointercapture` listener that sets
    // another one gets it processed with the next pointer event, not now.
    let pending = c.pending;
    let mut lost = false;
    if let Some(cur) = c.current.filter(|&cur| Some(cur) != pending) {
        page.mouse_capture.set(MouseCapture { current: None, ..c });
        // A capture target removed from the document: its document hears it.
        let at = if capture_connected(page, cur) { cur } else { page.document() };
        fire_capture_event(page, "lostpointercapture", at, m);
        lost = true;
    }
    // (That listener may have removed the pending target.)
    let c = page.mouse_capture.get();
    match pending.filter(|&p| Some(p) != c.current && capture_connected(page, p)) {
        Some(p) => {
            page.mouse_capture.set(MouseCapture { current: Some(p), ..c });
            crate::input::hover_capture_target(page, p, m);
            fire_capture_event(page, "gotpointercapture", p, m);
        }
        None if lost => crate::input::hover_under_pointer(page, m),
        None => {}
    }
}

/// "Process pending pointer capture" for touch pointer `id` (Pointer Events §4.1.3.2): returns
/// where `lostpointercapture` fires (the old capture target, its document when that left the
/// document) and where `gotpointercapture` does (the new one); the caller fires them, lost
/// first. A pending target that left the document is no longer pending.
pub(crate) fn touch_process_pending(page: &PageDom, id: i32) -> (Option<NodeId>, Option<NodeId>) {
    let Some(c) = touch_capture(page, id) else { return (None, None) };
    let pending = c.pending.filter(|&p| capture_connected(page, p));
    let lost = c.current.filter(|&cur| Some(cur) != pending).map(|cur| if capture_connected(page, cur) { cur } else { page.document() });
    let mut current = if lost.is_some() { None } else { c.current };
    let got = pending.filter(|&p| Some(p) != current);
    if got.is_some() {
        current = got;
    }
    set_touch_capture(page, id, TouchCapture { pending, current });
    (lost, got)
}

/// The touch pointer's implicit release after pointerup or pointercancel: no capture is pending
/// any more, so [`touch_process_pending`] loses the current one (the returned lost target).
pub(crate) fn touch_release_pending(page: &PageDom, id: i32) -> Option<NodeId> {
    if let Some(c) = touch_capture(page, id) {
        set_touch_capture(page, id, TouchCapture { pending: None, ..c });
    }
    touch_process_pending(page, id).0
}

/// The `pointerId` argument (a WebIDL `long`, wrapping modulo 2^32); ids
/// below 1 are never active pointers.
fn pointer_id_arg(a: &Args<'_>, method: &str) -> JsResult<i32> {
    a.require(1, method)?;
    let id = crate::util::to_i32(a.get(0))?;
    if id < 1 {
        return Err(JsError::dom("NotFoundError", "No active pointer with the given id is found."));
    }
    Ok(id)
}

/// `Element.setPointerCapture(pointerId)` for the mouse and the touches down; other ids
/// are ignored.
pub(crate) fn set_pointer_capture<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    let id = pointer_id_arg(&a, "Element.setPointerCapture")?;
    let page = n.page();
    let mouse = mouse_pointer_id(page);
    // (An id that is no active pointer: the mouse, or a touch that is down.)
    if id != mouse && touch_capture(page, id).is_none() {
        return Err(JsError::dom("NotFoundError", "No active pointer with the given id is found."));
    }
    if !page.dom.borrow().is_connected(n.id).unwrap_or(false) {
        return Err(JsError::dom("InvalidStateError", "The element is not connected."));
    }
    // Pointer Events: no capture while the pointer is locked.
    if crate::pointer_lock::target(page).is_some() {
        return Err(JsError::dom("InvalidStateError", "The pointer is locked."));
    }
    if id != mouse {
        if let Some(c) = touch_capture(page, id).filter(|_| capture_connected(page, n.id)) {
            set_touch_capture(page, id, TouchCapture { pending: Some(n.id), ..c });
        }
        return Ok(s.undefined());
    }
    let c = page.mouse_capture.get();
    if id == mouse && c.buttons_down && capture_connected(page, n.id) {
        page.mouse_capture.set(MouseCapture { pending: Some(n.id), ..c });
    }
    Ok(s.undefined())
}

pub(crate) fn release_pointer_capture<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    let id = pointer_id_arg(&a, "Element.releasePointerCapture")?;
    let page = n.page();
    let mouse = mouse_pointer_id(page);
    if id != mouse && touch_capture(page, id).is_none() {
        return Err(JsError::dom("NotFoundError", "No active pointer with the given id is found."));
    }
    if id != mouse {
        if let Some(c) = touch_capture(page, id).filter(|c| c.pending == Some(n.id)) {
            set_touch_capture(page, id, TouchCapture { pending: None, ..c });
        }
        return Ok(s.undefined());
    }
    let c = page.mouse_capture.get();
    if id == mouse && c.pending == Some(n.id) {
        page.mouse_capture.set(MouseCapture { pending: None, ..c });
    }
    Ok(s.undefined())
}

pub(crate) fn has_pointer_capture<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    a.require(1, "Element.hasPointerCapture")?;
    let id = crate::util::to_i32(a.get(0))?;
    let page = n.page();
    let has = if id == mouse_pointer_id(page) {
        page.mouse_capture.get().pending == Some(n.id)
    } else {
        touch_capture(page, id).is_some_and(|c| c.pending == Some(n.id))
    };
    Ok(s.boolean(has))
}
