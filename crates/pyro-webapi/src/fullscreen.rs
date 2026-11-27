//! Fullscreen API (https://fullscreen.spec.whatwg.org/): `Element.requestFullscreen()`,
//! `Document.exitFullscreen()`, `fullscreenElement`, `fullscreenEnabled`, and the
//! `fullscreenchange` / `fullscreenerror` events.
//!
//! pyro has no screen to give an element: the viewport keeps its size. What a page can observe is the
//! document's state: a request made with transient activation by a connected element in a
//! document allowed to use fullscreen is granted (the element becomes the document's
//! fullscreen element, `fullscreenchange` fires at it, the promise resolves); anything else
//! is denied (`fullscreenerror`, a `TypeError`), as Chromium answers both.

use std::rc::Rc;

use pyro_dom::NodeId;
use pyro_js::{Deferred, JsError, JsResult, Scope, Value, WeakRealm};

use crate::event::{EventCore, EventInterface};
use crate::page::{FrameContainer, PageDom, PageHooks};

/// Whether the document may use fullscreen ("fullscreen enabled": its permissions policy
/// allows `fullscreen`), as the browser decided when it made the document
/// ([`allowed_in_frame`] for a child frame's; always for a top-level one).
pub(crate) fn enabled(page: &PageDom) -> bool {
    crate::permissions_policy::feature_allowed(page, "fullscreen")
}

/// Permissions Policy "define an inherited policy for feature in container at origin" for
/// `fullscreen`: a child frame's document may use fullscreen when its parent document may, and
/// its container's policy allows the child's origin. The container policy is its `allow`
/// attribute's `fullscreen` directive (an allowlist of `*`, `'self'` (the parent's origin),
/// `'src'` (the container's declared origin: its `src` URL's, see pyro-browser's
/// `FullscreenContainer`; the default with no tokens), `'none'` and origins), else
/// `fullscreen *` when it has `allowfullscreen`. A container with neither admits the feature's
/// default allowlist, 'self': a child document at its parent's origin, not a cross-origin one
/// (Chrome; WPT fullscreen/api/document-fullscreen-enabled*, navigate-iframe). Origins are
/// ASCII serializations; `None` is opaque (a sandboxed frame), which matches only `*`.
pub fn allowed_in_frame(
    parent_allowed: bool,
    allowfullscreen: bool,
    allow: Option<&str>,
    parent_origin: Option<&str>,
    declared_origin: Option<&str>,
    child_origin: Option<&str>,
) -> bool {
    if !parent_allowed {
        return false;
    }
    // Without a declaration for it: `fullscreen *` with `allowfullscreen`, else the feature's default
    // allowlist, 'self': a child at its parent's origin may (Chrome; WPT document-fullscreen-enabled).
    let same_origin = parent_origin.is_some() && parent_origin == child_origin;
    container_allows("fullscreen", allow, parent_origin, declared_origin, child_origin).unwrap_or(allowfullscreen || same_origin)
}

/// A frame container's `allow` attribute on `feature` for a child document at `child_origin`
/// (the engine's [`crate::permissions_policy::container_allows`]).
pub fn container_allows(
    feature: &str,
    allow: Option<&str>,
    parent_origin: Option<&str>,
    declared_origin: Option<&str>,
    child_origin: Option<&str>,
) -> Option<bool> {
    crate::permissions_policy::container_allows(feature, allow, parent_origin, declared_origin, child_origin)
}

/// The document's fullscreen element: the topmost of its fullscreen elements that is still
/// connected to it (one removed from the document is no longer fullscreen).
pub(crate) fn element(page: &PageDom) -> Option<NodeId> {
    let doc = page.document();
    let (top, left) = {
        let dom = page.dom.borrow();
        let mut stack = page.fullscreen_stack.borrow_mut();
        let mut left = Vec::new();
        stack.retain(|&el| {
            // An element removed since (its flag cleared by the DOM's removing steps), or no
            // longer in this active document, has left fullscreen.
            let stays = dom.is_fullscreen(el)
                && dom.is_in_active_document(el)
                && dom.node_document(el).is_ok_and(|d| d == doc);
            if !stays {
                left.push(el);
            }
            stays
        });
        (stack.last().copied(), left)
    };
    // An element that left the document is no longer `:fullscreen`.
    if !left.is_empty() {
        if let Ok(mut dom) = page.dom.try_borrow_mut() {
            for el in left {
                let _ = dom.set_fullscreen(el, false);
            }
        }
    }
    top
}

/// Fullscreen "removing steps": `gone` left fullscreen because a removal (or a move into
/// another document) took them out of this document. They leave the fullscreen stack at once
/// (`fullscreenElement` reads the next one down, or null, synchronously). When the document's
/// fullscreen element (the stack's top) is what left, the document exits fullscreen in a task: what
/// is still on the stack leaves too and one `fullscreenchange` is fired at the document, as the
/// element is no longer connected (Chrome; WPT fullscreen/model/remove-*, move-*). Removing a
/// lower entry only drops it, with no event.
pub(crate) fn removed(page: &Rc<PageDom>, gone: Vec<NodeId>) {
    if gone.is_empty() {
        return;
    }
    // The removing steps fire `fullscreenchange` only when the document's fullscreen element
    // (the stack's top) is what left; removing a lower entry just drops it from the top layer.
    let top_left = page.fullscreen_stack.borrow().last().is_some_and(|top| gone.contains(top));
    // Drop the departed elements from the stack now (their flags are already clear).
    let _ = element(page);
    if !top_left {
        return;
    }
    // What is left of the stack exits with the element, when the task runs (not what a request
    // pushes in between).
    let remaining: Vec<NodeId> = page.fullscreen_stack.borrow().clone();
    let weak = Rc::downgrade(page);
    page.post_task(Box::new(move || {
        let Some(page) = weak.upgrade() else { return };
        if page.is_disposed() {
            return;
        }
        let leaving: Vec<NodeId> = {
            let mut stack = page.fullscreen_stack.borrow_mut();
            let mut leaving = Vec::new();
            stack.retain(|&el| {
                let leaves = remaining.contains(&el);
                if leaves {
                    leaving.push(el);
                }
                !leaves
            });
            leaving
        };
        if !leaving.is_empty() {
            let mut dom = page.dom.borrow_mut();
            for el in leaving {
                let _ = dom.set_fullscreen(el, false);
            }
        }
        // A frame whose document left fullscreen entirely takes its container with it.
        let mut events: Vec<(Rc<PageDom>, NodeId)> = leave_containers(&page);
        events.reverse();
        let document = page.document();
        events.push((page, document));
        for (page, node) in events {
            fire(&page, node, "fullscreenchange");
        }
    }));
}

/// `DocumentOrShadowRoot.fullscreenElement`: the fullscreen element retargeted against
/// `root` (the document or a shadow root), or null when it is not in that tree.
pub(crate) fn element_for<'s>(s: &Scope<'s>, page: &Rc<PageDom>, root: NodeId) -> JsResult<Value<'s>> {
    let Some(el) = element(page) else { return Ok(s.null()) };
    let found = {
        let dom = page.dom.borrow();
        dom.retarget_node(el, root).filter(|&c| dom.root_node(c, false).is_ok_and(|r| r == root))
    };
    match found {
        Some(id) => crate::node::node_value(s, page, Some(id)),
        None => Ok(s.null()),
    }
}

/// Fires a trusted `fullscreenchange` or `fullscreenerror` (bubbles, composed) at `node`, or
/// at the document when `node` is no longer connected.
fn fire(page: &Rc<PageDom>, node: NodeId, ty: &str) {
    // An element that moved to another document (or left this one) is no longer a target here.
    let target = {
        let dom = page.dom.borrow();
        if dom.is_in_active_document(node) && dom.node_document(node).ok() == Some(page.document()) { node } else { page.document() }
    };
    let Some(key) = crate::input::key_of(page, target) else { return };
    if crate::target::unheard(page, ty) {
        return;
    }
    let ev = EventCore::new(page, EventInterface::Event, ty, true, false, true);
    ev.state.borrow_mut().is_trusted = true;
    crate::target::fire(page, key, &ev);
}

/// Fires `ty` in a task of its own, after the one that changed the state and settled the
/// promise: the spec appends the event to the document's pending events, which the next rendering
/// update fires, so the promise's continuations run first and can listen for it
/// (`await el.requestFullscreen(); await fullScreenChange()`, WPT fullscreen/rendering/fullscreen-pseudo-class).
/// `events` are (document, target) pairs in the frame tree's order, the outermost document first.
fn fire_later(events: Vec<(Rc<PageDom>, NodeId)>, ty: &'static str) {
    let Some((first, _)) = events.first() else { return };
    let first = first.clone();
    first.post_task(Box::new(move || {
        for (page, node) in events {
            if !page.is_disposed() {
                fire(&page, node, ty);
            }
        }
    }));
}

/// The documents above `page`, innermost first, each with the owner element in it of the frame
/// that shows the one below (the "container" chain Fullscreen walks).
fn ancestors(page: &Rc<PageDom>) -> Vec<(Rc<PageDom>, NodeId)> {
    let mut out = Vec::new();
    let mut cur = page.clone();
    // (Bounded: a frame tree is never this deep.)
    for _ in 0..64 {
        match cur.container() {
            FrameContainer::Frame(parent, owner) => {
                out.push((parent.clone(), owner));
                cur = parent;
            }
            _ => break,
        }
    }
    out
}

/// Fullscreen "exit fullscreen" for the documents a document leaving fullscreen drags along: when
/// `page`'s stack just became empty and its frame's owner is the only fullscreen element of the
/// parent document (a "simple fullscreen document"), the owner leaves fullscreen there too, and so
/// on up. Returns the (document, element) pairs that left, innermost first.
fn leave_containers(page: &Rc<PageDom>) -> Vec<(Rc<PageDom>, NodeId)> {
    let mut left = Vec::new();
    let mut cur = page.clone();
    while element(&cur).is_none() {
        let FrameContainer::Frame(parent, owner) = cur.container() else { break };
        if element(&parent) != Some(owner) || parent.fullscreen_stack.borrow().len() != 1 {
            break;
        }
        parent.fullscreen_stack.borrow_mut().pop();
        let _ = parent.dom.borrow_mut().set_fullscreen(owner, false);
        left.push((parent.clone(), owner));
        cur = parent;
    }
    left
}

fn settle(realm: WeakRealm, deferred: Deferred, error: Option<&'static str>) {
    let Some(realm) = realm.upgrade() else { return };
    let Ok(s) = realm.scope() else { return };
    let _ = match error {
        Some(message) => {
            let e = JsError::Type(message.into()).to_value(&s);
            deferred.reject(&s, e)
        }
        None => deferred.resolve(&s, s.undefined()),
    };
}

/// Whether `el` may be made fullscreen ("fullscreen element ready check" and the request's
/// own checks, without the activation): an HTML element other than `<dialog>`, or an SVG or
/// MathML root, that is not a showing popover, connected to a document that is fully active
/// and allowed to use fullscreen.
fn ready(page: &PageDom, el: NodeId) -> bool {
    if !enabled(page) {
        return false;
    }
    let dom = page.dom.borrow();
    if !dom.is_in_active_document(el) || dom.node_document(el).ok() != Some(page.document()) {
        return false;
    }
    let Ok(data) = dom.element(el) else { return false };
    // Fullscreen "requestFullscreen()": a showing popover is refused (TypeError), as is a dialog.
    if dom.is_popover_showing(el) {
        return false;
    }
    let ns = data.namespace();
    if *ns == markup5ever::ns!(html) {
        return *data.local_name() != markup5ever::local_name!("dialog");
    }
    // An SVG or MathML element whose parent is not of its namespace (its root).
    if *ns == markup5ever::ns!(svg) || *ns == markup5ever::ns!(mathml) {
        let parent_ns = dom.parent_element(el).ok().flatten().and_then(|p| dom.element(p).ok().map(|d| d.namespace().clone()));
        return parent_ns.as_ref() != Some(ns);
    }
    false
}

/// `Element.requestFullscreen()`.
pub(crate) fn request<'s>(s: &Scope<'s>, page: &Rc<PageDom>, el: NodeId) -> JsResult<Value<'s>> {
    let d = s.deferred()?;
    let promise = d.promise.get(s);
    let realm = s.realm().downgrade();
    // A document the policy refuses (or report-only would) reports the violation, whatever else
    // the request lacks (`ready` refuses it below).
    crate::permissions_policy::check_feature(s, page, "fullscreen");
    // The request is denied unless the element is ready and the window has transient
    // activation, which a granted request consumes.
    let granted = ready(page, el) && page.has_transient_activation();
    if granted {
        page.consume_transient_activation();
    }
    let weak = Rc::downgrade(page);
    // The rest runs "in parallel" and then in a task, as the spec has it (after a resize
    // pyro does not do): the state changes, the promise settles, the event fires.
    page.post_task(Box::new(move || {
        // A document that is gone (its frame was removed or navigated) is not fully active: the
        // request is refused with no event, there being no document to fire it at.
        let Some(page) = weak.upgrade().filter(|p| !p.is_disposed() && !p.is_detached()) else {
            settle(realm, d, Some("Document not active"));
            return;
        };
        // The element may have been removed (or the document deactivated) in between.
        if granted && ready(&page, el) {
            // "If element is doc's fullscreen element, terminate these subsubsteps": already
            // fullscreen is a success without a change, so no event.
            if element(&page) == Some(el) {
                settle(realm, d, None);
                return;
            }
            // The popovers that do not contain the element close first.
            crate::popover::hide_popovers_for_fullscreen(&page, el);
            {
                let mut stack = page.fullscreen_stack.borrow_mut();
                stack.retain(|&e| e != el);
                stack.push(el);
            }
            let _ = page.dom.borrow_mut().set_fullscreen(el, true);
            // Each ancestor document goes fullscreen with its frame: the container is pushed on
            // its document's stack, unless it is already that document's fullscreen element. The
            // events fire outermost document first (animation frame tasks run in frame tree order).
            let mut events: Vec<(Rc<PageDom>, NodeId)> = Vec::new();
            for (parent, owner) in ancestors(&page) {
                if element(&parent) == Some(owner) {
                    continue;
                }
                {
                    let mut stack = parent.fullscreen_stack.borrow_mut();
                    stack.retain(|&e| e != owner);
                    stack.push(owner);
                }
                let _ = parent.dom.borrow_mut().set_fullscreen(owner, true);
                events.push((parent, owner));
            }
            events.reverse();
            events.push((page.clone(), el));
            // The promise resolves before the events are dispatched (Chrome).
            settle(realm, d, None);
            fire_later(events, "fullscreenchange");
        } else {
            settle(realm, d, Some("Permissions check failed"));
            fire_later(vec![(page, el)], "fullscreenerror");
        }
    }));
    Ok(promise)
}

/// The `FullscreenOptions` dictionary of `requestFullscreen(options)`: null or undefined is the
/// empty dictionary, anything else must be an object whose `navigationUI` (when present) is one of
/// the `FullscreenNavigationUI` values, else a `TypeError` (which the operation turns into a
/// rejection). Members are read in lexicographic order, so a `screen` getter runs too.
pub(crate) fn check_options(options: Value<'_>) -> JsResult<()> {
    if options.is_nullish() {
        return Ok(());
    }
    let Some(dict) = options.as_object() else {
        return Err(JsError::Type("Failed to execute 'requestFullscreen' on 'Element': The provided value is not of type 'FullscreenOptions'.".into()));
    };
    let navigation_ui = dict.get_atom("navigationUI")?;
    if !navigation_ui.is_undefined() {
        let keyword = navigation_ui.to_string()?;
        if !matches!(keyword.as_str(), "auto" | "hide" | "show") {
            return Err(JsError::Type(format!("Failed to execute 'requestFullscreen' on 'Element': The provided value '{keyword}' is not a valid enum value of type FullscreenNavigationUI.").into()));
        }
    }
    dict.get_atom("screen")?;
    Ok(())
}

/// `Document.exitFullscreen()`: with no fullscreen element it rejects; otherwise, in a task, the
/// topmost fullscreen element leaves fullscreen ("exit fullscreen" with nested fullscreen
/// elements pops one level), `fullscreenchange` fires at it and the promise resolves. The state
/// still holds when the call returns. A call made while another exit is pending joins it: it
/// resolves after it, with no second change. A document left with no fullscreen element whose
/// frame is the only fullscreen element of its parent takes the frame out of fullscreen too (and
/// so on up); the documents of frames below it leave fullscreen entirely.
pub(crate) fn exit<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    let d = s.deferred()?;
    let promise = d.promise.get(s);
    if element(page).is_none() || page.is_detached() {
        d.reject(s, JsError::Type("Document not active or no fullscreen element".into()).to_value(s))?;
        return Ok(promise);
    }
    let realm = s.realm().downgrade();
    let joined = page.fullscreen_exit_pending.replace(true);
    let weak = Rc::downgrade(page);
    page.post_task(Box::new(move || {
        let Some(page) = weak.upgrade().filter(|p| !p.is_disposed()) else {
            settle(realm, d, Some("Document not active"));
            return;
        };
        let mut events: Vec<(Rc<PageDom>, NodeId)> = Vec::new();
        if !joined {
            page.fullscreen_exit_pending.set(false);
            // The element may have left (a removal) before the task ran: then there is nothing to pop.
            if let Some(top) = element(&page) {
                page.fullscreen_stack.borrow_mut().pop();
                let _ = page.dom.borrow_mut().set_fullscreen(top, false);
                // The frame tree below leaves fullscreen entirely, with the one event per document.
                if let Some(hooks) = page.hooks() {
                    for below in hooks.frame_tree() {
                        if Rc::ptr_eq(&below, &page) || !ancestors(&below).iter().any(|(p, _)| Rc::ptr_eq(p, &page)) {
                            continue;
                        }
                        let Some(top) = element(&below) else { continue };
                        let all: Vec<NodeId> = std::mem::take(&mut *below.fullscreen_stack.borrow_mut());
                        {
                            let mut dom = below.dom.borrow_mut();
                            for el in all {
                                let _ = dom.set_fullscreen(el, false);
                            }
                        }
                        events.push((below, top));
                    }
                }
                // Then the ancestors that only were fullscreen for this document's sake.
                let mut up = leave_containers(&page);
                up.reverse();
                events.extend(up);
                events.push((page, top));
                // Outermost document first: the frame tree's order.
                events.sort_by_key(|(p, _)| ancestors(p).len());
            }
        }
        settle(realm, d, None);
        fire_later(events, "fullscreenchange");
    }));
    Ok(promise)
}

#[cfg(test)]
mod tests {
    use super::allowed_in_frame as allowed;

    const A: Option<&str> = Some("http://a.test");
    const B: Option<&str> = Some("http://b.test");

    #[test]
    fn no_container_policy_admits_same_origin_only() {
        // Neither allowfullscreen nor an `allow` declaration: the default allowlist, 'self' (Chrome).
        assert!(allowed(true, false, None, A, A, A));
        assert!(!allowed(true, false, None, A, B, B));
        assert!(!allowed(true, false, None, A, A, B));
        assert!(allowed(true, false, Some("camera *"), A, A, A));
        assert!(!allowed(true, false, None, A, None, None));
        assert!(!allowed(true, false, None, None, None, None));
        // A parent that may not use fullscreen allows no frame, whatever the container says.
        assert!(!allowed(false, true, Some("fullscreen *"), A, A, A));
    }

    #[test]
    fn allowfullscreen_is_fullscreen_star() {
        assert!(allowed(true, true, None, A, B, B));
        assert!(allowed(true, true, None, A, None, None));
        // An `allow` that declares fullscreen wins over allowfullscreen.
        assert!(!allowed(true, true, Some("fullscreen 'none'"), A, B, B));
        // One that declares something else does not.
        assert!(allowed(true, true, Some("camera *"), A, B, B));
    }

    #[test]
    fn src_is_the_declared_origin() {
        // `allow="fullscreen"` delegates to the `src` URL's origin: a frame that redirected or
        // navigated elsewhere (child B, declared A) does not get it.
        assert!(allowed(true, false, Some("fullscreen"), A, A, A));
        assert!(!allowed(true, false, Some("fullscreen"), A, A, B));
        assert!(!allowed(true, false, Some("fullscreen 'src'"), A, A, B));
        assert!(allowed(true, false, Some("fullscreen 'src'"), A, B, B));
        // A sandboxed container's declared origin is opaque: 'src' matches nothing.
        assert!(!allowed(true, false, Some("fullscreen"), A, None, None));
        // allowfullscreen is `*`: any destination.
        assert!(allowed(true, true, None, A, A, B));
    }

    #[test]
    fn allow_attribute_allowlists() {
        assert!(allowed(true, false, Some("fullscreen"), A, B, B)); // 'src'
        assert!(allowed(true, false, Some("fullscreen 'src'"), A, B, B));
        assert!(!allowed(true, false, Some("fullscreen 'self'"), A, B, B));
        assert!(allowed(true, false, Some("fullscreen 'self'"), A, A, A));
        assert!(allowed(true, false, Some("geolocation *; fullscreen http://b.test"), A, B, B));
        assert!(allowed(true, false, Some("fullscreen http://b.test/"), A, B, B));
        assert!(!allowed(true, false, Some("fullscreen http://c.test"), A, B, B));
        assert!(allowed(true, false, Some("fullscreen *"), A, None, None));
        assert!(!allowed(true, false, Some("fullscreen 'none'"), A, A, A));
    }
}
