//! Pointer Lock API (https://w3c.github.io/pointerlock/): `Element.requestPointerLock()`,
//! `Document.exitPointerLock()`, `pointerLockElement` (on Document and ShadowRoot, retargeted) and the
//! `pointerlockchange` / `pointerlockerror` events.
//!
//! While an element holds the lock, the mouse input of its document (`crate::input::dispatch_mouse_inner`) is
//! delivered to it, whatever is under the pointer: the pointer stays where it was when the lock was granted
//! (`clientX/Y`, `screenX/Y` do not move), `movementX/Y` keep reporting the motion of the input, no boundary events are
//! fired, and pointer capture does not apply. The lock ends by `exitPointerLock()`, the document losing focus,
//! or the lock target leaving the document (the Escape key is browser UI, which CDP key events bypass).
//!
//! The input pyro receives is a CDP stream of positions, not a device: it has no raw (unadjusted) movement to give, so
//! `requestPointerLock({unadjustedMovement: true})` is refused (`NotSupportedError`), as a platform without raw input does.

use std::rc::Rc;

use pyro_dom::NodeId;
use pyro_js::{Deferred, JsError, JsResult, Scope, Value, WeakRealm};

use crate::event::{EventCore, EventInterface};
use crate::page::PageDom;

/// The lock of a document: its target and the pointer position the target's events report while it holds.
#[derive(Default)]
pub(crate) struct State {
    element: Option<NodeId>,
    x: f64,
    y: f64,
}

impl State {
    /// Whether an element holds the lock (it may have left the document since: [`check_removed`]).
    pub(crate) fn held(&self) -> bool {
        self.element.is_some()
    }
}

/// The lock target while it is locked: the element when it is still in the document, and the frozen pointer position.
pub(crate) fn target(page: &PageDom) -> Option<(NodeId, f64, f64)> {
    let (el, x, y) = {
        let state = page.pointer_lock.borrow();
        (state.element?, state.x, state.y)
    };
    let dom = page.dom.borrow();
    (dom.is_connected(el).unwrap_or(false) && dom.node_document(el).ok() == Some(page.document())).then_some((el, x, y))
}

/// `DocumentOrShadowRoot.pointerLockElement`: the lock target retargeted against `root` (the document or a shadow
/// root), or null when it is not in that tree.
pub(crate) fn element_for<'s>(s: &Scope<'s>, page: &Rc<PageDom>, root: NodeId) -> JsResult<Value<'s>> {
    let Some((el, _, _)) = target(page) else { return Ok(s.null()) };
    let found = {
        let dom = page.dom.borrow();
        dom.retarget_node(el, root).filter(|&c| dom.root_node(c, false).is_ok_and(|r| r == root))
    };
    match found {
        Some(id) => crate::node::node_value(s, page, Some(id)),
        None => Ok(s.null()),
    }
}

/// Fires the trusted `pointerlockchange` or `pointerlockerror` (plain `Event`, not bubbling, not cancelable) at the document.
fn fire(page: &Rc<PageDom>, ty: &str) {
    if page.is_disposed() || crate::target::unheard(page, ty) {
        return;
    }
    let Some(key) = crate::input::key_of(page, page.document()) else { return };
    let ev = EventCore::trusted(page, EventInterface::Event, ty, false, false);
    crate::target::fire(page, key, &ev);
}

fn fire_later(page: &Rc<PageDom>, ty: &'static str) {
    let weak = Rc::downgrade(page);
    page.post_task(Box::new(move || {
        if let Some(page) = weak.upgrade() {
            fire(&page, ty);
        }
    }));
}

fn settle(realm: WeakRealm, deferred: Deferred, error: Option<(&'static str, &'static str)>) {
    let Some(realm) = realm.upgrade() else { return };
    let Ok(s) = realm.scope() else { return };
    let _ = match error {
        Some((name, message)) => deferred.reject(&s, JsError::dom(name, message).to_value(&s)),
        None => deferred.resolve(&s, s.undefined()),
    };
}

/// Whether the request is allowed now ("pointer lock request" checks, in the order Chrome reports them): the element must be
/// in the document, the document focused and visible, and, unless the document is locked already (a new target is just
/// taken), the window must have transient activation.
fn check(page: &Rc<PageDom>, el: NodeId, unadjusted: bool) -> Result<(), (&'static str, &'static str)> {
    let connected = {
        let dom = page.dom.borrow();
        dom.is_in_active_document(el) && dom.node_document(el).ok() == Some(page.document())
    };
    if !connected || page.is_detached() {
        return Err(("InvalidStateError", "The root document of this element is not valid for pointer lock."));
    }
    if unadjusted {
        return Err(("NotSupportedError", "Unadjusted movement is not supported: the input has no raw device."));
    }
    let focused = page.hooks().is_none_or(|h| h.is_focused()) && !page.is_hidden();
    if !focused {
        return Err(("NotAllowedError", "The root document of this element does not have focus."));
    }
    if target(page).is_none() && !page.has_transient_activation() {
        return Err(("NotAllowedError", "A user gesture is required to request pointer lock."));
    }
    Ok(())
}

/// `PointerLockOptions.unadjustedMovement` of `requestPointerLock(options)`; a non-object, non-nullish `options` is a `TypeError`.
pub(crate) fn unadjusted_option(options: Value<'_>) -> JsResult<bool> {
    if options.is_nullish() {
        return Ok(false);
    }
    let Some(dict) = options.as_object() else {
        return Err(JsError::Type("Failed to execute 'requestPointerLock' on 'Element': The provided value is not of type 'PointerLockOptions'.".into()));
    };
    Ok(dict.get_atom("unadjustedMovement")?.to_bool())
}

/// `Element.requestPointerLock(options)`: the checks run now, the lock is taken in a task (which checks the element again),
/// `pointerlockchange` fires at the document and the promise resolves; a refusal fires `pointerlockerror` and rejects.
pub(crate) fn request<'s>(s: &Scope<'s>, page: &Rc<PageDom>, el: NodeId, unadjusted: bool) -> JsResult<Value<'s>> {
    let d = s.deferred()?;
    let promise = d.promise.get(s);
    let realm = s.realm().downgrade();
    let early = check(page, el, unadjusted).err();
    let weak = Rc::downgrade(page);
    page.post_task(Box::new(move || {
        let Some(page) = weak.upgrade().filter(|p| !p.is_disposed()) else {
            settle(realm, d, Some(("InvalidStateError", "The document is no longer active.")));
            return;
        };
        // The element may have been removed (or the lock refused) before the task ran.
        let refused = early.or_else(|| check(&page, el, false).err());
        if let Some(error) = refused {
            fire(&page, "pointerlockerror");
            settle(realm, d, Some(error));
            return;
        }
        // Already the target: a success with no change, so no event.
        if target(&page).is_some_and(|(locked, _, _)| locked == el) {
            settle(realm, d, None);
            return;
        }
        // Pointer capture ends when the lock is granted: a pending target is dropped without events, a capture
        // that took effect hears `lostpointercapture`. (Before the lock is set, so those events and the boundary
        // events of the release are the unlocked pointer's.)
        let pointer = page.pointer.borrow().clone();
        let mut capture = page.mouse_capture.get();
        capture.pending = None;
        page.mouse_capture.set(capture);
        if capture.current.is_some() {
            crate::pointer_capture::process_pending_capture(&page, &pointer.clone().unwrap_or_default());
        }
        // Retargeting an existing lock keeps the frozen position; a new lock freezes the pointer where it is.
        {
            let keep = target(&page).is_some();
            let mut state = page.pointer_lock.borrow_mut();
            state.element = Some(el);
            if !keep {
                (state.x, state.y) = pointer.map_or((0.0, 0.0), |p| (p.x, p.y));
            }
        }
        fire(&page, "pointerlockchange");
        settle(realm, d, None);
    }));
    Ok(promise)
}

/// The lock ends (`exitPointerLock()`, blur, or its target left the document): `pointerLockElement` is null at
/// once and `pointerlockchange` fires in a task, once. Nothing when the document holds no lock.
pub(crate) fn release(page: &Rc<PageDom>) {
    let held = page.pointer_lock.borrow_mut().element.take().is_some();
    if held {
        fire_later(page, "pointerlockchange");
    }
}

/// `Document.exitPointerLock()`.
pub(crate) fn exit(page: &Rc<PageDom>) {
    release(page);
}

/// Called after mutations (`PageDom::flush_mutation_events`): a lock target that left the document releases the lock.
pub(crate) fn check_removed(page: &Rc<PageDom>) {
    let Some(el) = page.pointer_lock.try_borrow().ok().and_then(|state| state.element) else { return };
    let Ok(dom) = page.dom.try_borrow() else { return };
    let present = dom.is_connected(el).unwrap_or(false) && dom.node_document(el).ok() == Some(page.document());
    drop(dom);
    if !present {
        release(page);
    }
}
