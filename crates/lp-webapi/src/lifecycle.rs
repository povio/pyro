//! Document/window lifecycle events driven by the loader (lp-browser).

use std::rc::Rc;

use lp_dom::NodeId;
use lp_js::{JsError, JsException, Persistent, RealmId, Scope};

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
    flush_autofocus(page, doc);
    crate::input::fire_simple(page, doc, "DOMContentLoaded", true, false);
}

/// HTML "flush autofocus candidates" (simplified): the first `autofocus`
/// element of the top-level document gets focus when nothing has it.
fn flush_autofocus(page: &Rc<PageDom>, doc: NodeId) {
    if page.scripting_disabled() {
        return;
    }
    let candidate = {
        let dom = page.dom.borrow();
        if dom.focused(doc).ok().flatten().is_some() {
            return;
        }
        let Ok(mut desc) = dom.descendants(doc) else { return };
        desc.find(|&id| dom.element(id).is_ok_and(|e| e.is_html() && e.attr("autofocus").is_some()))
            .filter(|&id| dom.is_focusable(id).unwrap_or(false))
    };
    if let Some(el) = candidate {
        crate::input::focus_element(page, el);
    }
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

/// HTML "prompt to unload" for a document about to be navigated away from:
/// fires `beforeunload` (cancelable) at its Window. Returns whether a
/// listener asked for the prompt (canceled the event or set `returnValue`).
/// There is nobody to answer one, and Chrome shows none without a user
/// gesture, so the embedder navigates either way.
pub fn before_unload(page: &Rc<PageDom>) -> bool {
    if page.is_disposed() || page.scripting_disabled() {
        return false;
    }
    let Some(window) = page.window_target() else { return false };
    if !page.dom.borrow().has_listeners(window) {
        return false;
    }
    let ev = EventCore::trusted(page, EventInterface::BeforeUnloadEvent, "beforeunload", false, true);
    let canceled = !fire(page, window, &ev);
    let asked = ev.fields.borrow().data.as_deref().is_some_and(|text| !text.is_empty());
    canceled || asked
}

/// HTML "unload" of a document being navigated away from or closed, as
/// Chrome runs it (no back/forward cache): `pagehide` (persisted false) at
/// the Window, the visibility state becomes "hidden" with
/// `visibilitychange` at the document (bubbling to the Window), then
/// `unload` at the Window. Runs once per document. Documents without
/// listeners for any of them only get the flag set: no event is made.
/// The embedder calls it for a navigable's descendants first, and keeps
/// the network running afterwards, so keepalive requests made by the
/// listeners (`sendBeacon`, `fetch(.., {keepalive})`) go out.
pub fn unload(page: &Rc<PageDom>) {
    if page.is_disposed() || page.is_hidden() {
        return;
    }
    let window = page.window_target().filter(|_| !page.scripting_disabled());
    let Some(window) = window else { return page.set_hidden(true) };
    let doc = page.document();
    let listens = |page: &PageDom| match page.dom.try_borrow() {
        Ok(dom) => (dom.has_listeners(window), dom.node_key(doc).is_some_and(|key| dom.has_listeners(key))),
        Err(_) => (false, false),
    };
    if listens(page).0 {
        let ev = EventCore::trusted(page, EventInterface::PageTransitionEvent, "pagehide", false, false);
        fire(page, window, &ev);
    }
    page.set_hidden(true);
    let (window_listens, document_listens) = listens(page);
    if document_listens || window_listens {
        crate::input::fire_simple(page, doc, "visibilitychange", true, false);
    }
    // A listener may have added an `unload` one: look again.
    if page.dom.try_borrow().is_ok_and(|dom| dom.has_listeners(window)) {
        let ev = EventCore::trusted(page, EventInterface::Event, "unload", false, false);
        fire(page, window, &ev);
    }
}

fn window_key(page: &PageDom, _doc: NodeId) -> Option<lp_dom::events::TargetKey> {
    page.window_target()
}

/// HTML "report an exception" for a Window, installed on page realms as
/// their `lp_js::ExceptionReporter`: an uncaught exception of a callback
/// the platform invoked (listener, timer, observer, script evaluation) fires
/// a trusted, cancelable `error` ErrorEvent at the Window right away, with
/// `message`/`filename`/`lineno`/`colno`/`error` from the thrown value.
/// Returns true when a listener canceled the event (`window.onerror`
/// returning true, `preventDefault()`): the caller then skips the console
/// report. While the event is dispatched the page is "in error reporting
/// mode": an exception of one of its listeners is reported but fires no
/// second event. A disposed page, a page without scripting and the error
/// of a realm that has no Window get no event. As in Chrome, only the
/// listeners of the world that threw see the event: an isolated world's
/// exception never reaches the page's `onerror`, and a page exception stays
/// out of content scripts. Dispatch runs script, so a report made while the
/// DOM is borrowed is replayed from a task.
pub fn report_exception(s: &Scope<'_>, exception: &JsException) -> bool {
    let Ok(page) = crate::target::page_of(s) else { return false };
    if page.is_disposed() || page.scripting_disabled() || s.realm().is_disposed() || page.error_reporting.get() {
        return false;
    }
    let Some(target) = page.window_target() else { return false };
    if page.dom.try_borrow_mut().is_err() {
        // The caller holds the DOM: dispatch (and the console report an
        // unhandled event leads to) from a task of its own. Without an
        // embedder to post it the exception is only reported.
        let (Some(value), Some(hooks)) = (exception.value.clone(), page.hooks()) else { return false };
        let realm = s.realm().downgrade();
        hooks.post_task(Box::new(move || {
            let Some(realm) = realm.upgrade() else { return };
            let Ok(s) = realm.scope() else { return };
            s.report_exception(&JsError::Thrown(value));
        }));
        return true;
    }
    let ev = EventCore::trusted(&page, EventInterface::ErrorEvent, "error", false, true);
    {
        let mut fields = ev.fields.borrow_mut();
        fields.message = format!("Uncaught {}", exception.text());
        fields.filename = match &exception.url {
            Some(url) => url.clone(),
            None => page.dom.borrow().document_data(page.document()).map(|d| d.url.to_string()).unwrap_or_default(),
        };
        fields.lineno = exception.line.unwrap_or(0);
        fields.colno = exception.column.unwrap_or(0);
        fields.error = exception.value.clone();
    }
    page.error_reporting.set(true);
    page.error_event_world.set(Some((ev.id, s.realm_id())));
    let not_canceled = fire(&page, target, &ev);
    page.error_event_world.set(None);
    page.error_reporting.set(false);
    !not_canceled
}

/// Reports an engine diagnostic (a script that failed to load) to the
/// console/CDP hooks only. It is no script exception, so no `error` event
/// fires at the Window: Chrome fires `error` at the element and logs.
pub fn report_diagnostic(s: &Scope<'_>, error: &JsError) {
    let Ok(page) = crate::target::page_of(s) else { return s.report_exception(error) };
    let was = page.error_reporting.replace(true);
    s.report_exception(error);
    page.error_reporting.set(was);
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
    page.unregister_realm(realm);
    page.data_view_readers.borrow_mut().remove(&realm);
    page.header_helpers.borrow_mut().remove(&realm);
    page.url_params_helpers.borrow_mut().remove(&realm);
    crate::custom::realm_disposed(page, realm);
    page.window_holders.borrow_mut().remove(&realm);
    drop(page.same_objects.borrow_mut().remove(&realm));
    drop(page.structured_clones.borrow_mut().remove(&realm));
    drop(page.define_properties.borrow_mut().remove(&realm));
    drop(page.event_unforgeables.borrow_mut().remove(&realm));
    page.pinned.borrow_mut().retain(|(r, _), _| *r != realm);
    page.forget_realm_wrappers(realm);
}

/// The loader fetched the parser-blocking script `node` written to the
/// script-created parser (`PageHooks::written_script_queued`).
pub fn written_script_fetched(page: &Rc<PageDom>, node: NodeId, url: &str, source: Result<String, String>) {
    crate::html::written_script_fetched(page, node, url, source);
}
