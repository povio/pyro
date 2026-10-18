//! Document/window lifecycle events driven by the loader (lp-browser).

use std::rc::Rc;

use lp_dom::NodeId;
use lp_js::{Persistent, RealmId};

use crate::event::{EventCore, EventInterface};
use crate::page::PageDom;
use crate::target::fire;

/// Sets `document.readyState` and fires `readystatechange` when it changes.
pub fn set_ready_state(page: &Rc<PageDom>, doc: NodeId, state: &'static str) {
    if page.ready_state(doc) == state {
        return;
    }
    page.set_ready_state(doc, state);
    crate::input::fire_simple(page, doc, "readystatechange", false, false);
}

/// readyState=interactive, then `DOMContentLoaded` (bubbles to window).
pub fn dom_content_loaded(page: &Rc<PageDom>, doc: NodeId) {
    set_ready_state(page, doc, "interactive");
    crate::input::fire_simple(page, doc, "DOMContentLoaded", true, false);
}

/// readyState=complete, then `load` and `pageshow` at the window.
pub fn load(page: &Rc<PageDom>, doc: NodeId) {
    set_ready_state(page, doc, "complete");
    if let Some(w) = window_key(page, doc) {
        let ev = EventCore::trusted(page, EventInterface::Event, "load", false, false);
        fire(page, w, &ev);
        let ev = EventCore::trusted(page, EventInterface::PageTransitionEvent, "pageshow", false, false);
        fire(page, w, &ev);
    }
}

fn window_key(page: &PageDom, _doc: NodeId) -> Option<lp_dom::events::TargetKey> {
    page.window_target()
}

/// `popstate` at the window (history traversal / back-forward).
pub fn popstate(page: &Rc<PageDom>, state: Option<Persistent>) {
    let Some(w) = page.window_target() else { return };
    let ev = EventCore::trusted(page, EventInterface::PopStateEvent, "popstate", false, false);
    ev.fields.borrow_mut().state = state;
    fire(page, w, &ev);
}

/// `hashchange` at the window.
pub fn hashchange(page: &Rc<PageDom>, old_url: &str, new_url: &str) {
    let Some(w) = page.window_target() else { return };
    let ev = EventCore::trusted(page, EventInterface::HashChangeEvent, "hashchange", false, false);
    {
        let mut f = ev.fields.borrow_mut();
        f.old_url = old_url.to_owned();
        f.new_url = new_url.to_owned();
    }
    fire(page, w, &ev);
}

/// `load` / `error` of a script, stylesheet, image or iframe element. Call
/// from a separate task (the loader decides scheduling).
pub fn resource_loaded(page: &Rc<PageDom>, el: NodeId, ok: bool) {
    crate::input::fire_simple(page, el, if ok { "load" } else { "error" }, false, false);
}

/// Forgets per-realm state of a disposed realm (navigation, world removal):
/// window listener holder and pinned wrappers.
pub fn realm_disposed(page: &PageDom, realm: RealmId) {
    crate::fetch::cancel_realm(page, realm);
    crate::xhr::cancel_realm(page, realm);
    crate::file_reader::cancel_realm(page, realm);
    crate::mutation_observer::cancel_realm(page, realm);
    crate::intersection_observer::cancel_realm(page, realm);
    crate::resize_observer::cancel_realm(page, realm);
    page.realms.borrow_mut().remove(&realm);
    page.data_view_readers.borrow_mut().remove(&realm);
    page.header_helpers.borrow_mut().remove(&realm);
    page.url_params_helpers.borrow_mut().remove(&realm);
    page.window_holders.borrow_mut().remove(&realm);
    drop(page.same_objects.borrow_mut().remove(&realm));
    drop(page.structured_clones.borrow_mut().remove(&realm));
    drop(page.define_properties.borrow_mut().remove(&realm));
    page.pinned.borrow_mut().retain(|(r, _), _| *r != realm);
}
