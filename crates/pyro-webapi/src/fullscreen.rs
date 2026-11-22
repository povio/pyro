//! Fullscreen API (https://fullscreen.spec.whatwg.org/): `Element.requestFullscreen()`,
//! `Document.exitFullscreen()`, `fullscreenElement`, `fullscreenEnabled`, and the
//! `fullscreenchange` / `fullscreenerror` events.
//!
//! pyro has no screen to give an element: the viewport keeps its size, the element is not
//! moved to the top layer and `:fullscreen` matches nothing. What a page can observe is the
//! document's state: a request made with transient activation by a connected element in a
//! document allowed to use fullscreen is granted (the element becomes the document's
//! fullscreen element, `fullscreenchange` fires at it, the promise resolves); anything else
//! is denied (`fullscreenerror`, a `TypeError`), as Chromium answers both.

use std::rc::Rc;

use pyro_dom::NodeId;
use pyro_js::{Deferred, JsError, JsResult, Scope, Value, WeakRealm};

use crate::event::{EventCore, EventInterface};
use crate::page::PageDom;

/// Whether the document may use fullscreen ("fullscreen enabled": its permissions policy
/// allows `fullscreen`), as the browser decided when it made the document
/// ([`allowed_in_frame`] for a child frame's; always for a top-level one).
pub(crate) fn enabled(page: &PageDom) -> bool {
    page.fullscreen_allowed()
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

/// A frame container's `allow` attribute on `feature` for a child document at `child_origin`:
/// `Some(allowed)` when the attribute declares the feature (an allowlist of `*`, `'self'` (the
/// parent's origin), `'src'` (the container's declared origin, also the default with no tokens),
/// `'none'` and origins), `None` when it does not mention it.
pub fn container_allows(
    feature: &str,
    allow: Option<&str>,
    parent_origin: Option<&str>,
    declared_origin: Option<&str>,
    child_origin: Option<&str>,
) -> Option<bool> {
    let same = |a: Option<&str>| a.is_some() && a == child_origin;
    let declared = allow.and_then(|allow| {
        allow.split(';').find_map(|directive| {
            let mut tokens = directive.split_ascii_whitespace();
            (tokens.next()? == feature).then(|| tokens.map(str::to_owned).collect::<Vec<_>>())
        })
    })?;
    Some(match declared {
        // `'src'`: the frame's document must be at the origin the container delegated to (its
        // `src` URL's), not wherever a redirect or a later navigation took it.
        list if list.is_empty() => same(declared_origin),
        list => list.iter().any(|t| match t.as_str() {
            "*" => true,
            "'self'" => same(parent_origin),
            "'src'" => same(declared_origin),
            "'none'" => false,
            origin => same(Some(origin.trim_end_matches('/'))),
        }),
    })
}

/// The document's fullscreen element: the topmost of its fullscreen elements that is still
/// connected to it (one removed from the document is no longer fullscreen).
pub(crate) fn element(page: &PageDom) -> Option<NodeId> {
    let dom = page.dom.borrow();
    let doc = page.document();
    let mut stack = page.fullscreen_stack.borrow_mut();
    stack.retain(|&el| dom.is_in_active_document(el) && dom.node_document(el).is_ok_and(|d| d == doc));
    stack.last().copied()
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
    let target = if page.dom.borrow().is_in_active_document(node) { node } else { page.document() };
    let Some(key) = crate::input::key_of(page, target) else { return };
    if crate::target::unheard(page, ty) {
        return;
    }
    let ev = EventCore::new(page, EventInterface::Event, ty, true, false, true);
    ev.state.borrow_mut().is_trusted = true;
    crate::target::fire(page, key, &ev);
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
    // The request is denied unless the element is ready and the window has transient
    // activation, which a granted request consumes.
    let granted = ready(page, el) && page.has_transient_activation();
    if granted {
        page.consume_transient_activation();
    }
    let weak = Rc::downgrade(page);
    // The rest runs "in parallel" and then in a task, as the spec has it (after a resize
    // pyro does not do): the state changes, the event fires, the promise settles.
    page.post_task(Box::new(move || {
        let Some(page) = weak.upgrade() else { return };
        if page.is_disposed() {
            return;
        }
        // The element may have been removed (or the document deactivated) in between.
        if granted && ready(&page, el) {
            {
                let mut stack = page.fullscreen_stack.borrow_mut();
                stack.retain(|&e| e != el);
                stack.push(el);
            }
            fire(&page, el, "fullscreenchange");
            settle(realm, d, None);
        } else {
            fire(&page, el, "fullscreenerror");
            settle(realm, d, Some("Permissions check failed"));
        }
    }));
    Ok(promise)
}

/// `Document.exitFullscreen()`: the document's fullscreen elements all leave fullscreen
/// ("fully exit fullscreen", there being no screen to restore), `fullscreenchange` fires at
/// each in a task, and the promise resolves; with no fullscreen element it rejects.
pub(crate) fn exit<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    let d = s.deferred()?;
    let promise = d.promise.get(s);
    if element(page).is_none() {
        d.reject(s, JsError::Type("Document not active or no fullscreen element".into()).to_value(s))?;
        return Ok(promise);
    }
    let realm = s.realm().downgrade();
    let left: Vec<NodeId> = std::mem::take(&mut *page.fullscreen_stack.borrow_mut());
    let weak = Rc::downgrade(page);
    page.post_task(Box::new(move || {
        let Some(page) = weak.upgrade() else { return };
        if page.is_disposed() {
            return;
        }
        for el in left.into_iter().rev() {
            fire(&page, el, "fullscreenchange");
        }
        settle(realm, d, None);
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
