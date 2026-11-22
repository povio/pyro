//! Document/window lifecycle events driven by the loader (pyro-browser).

use std::rc::Rc;

use pyro_dom::NodeId;
use pyro_js::{JsError, JsException, Persistent, RealmId, Scope};

use crate::event::{EventCore, EventData, EventInterface};
use crate::page::PageDom;
use crate::target::{fire, fire_with_target_override};

/// Sets `document.readyState` and fires `readystatechange` when it changes.
pub fn set_ready_state(page: &Rc<PageDom>, doc: NodeId, state: &'static str) {
    if page.ready_state(doc) == state {
        return;
    }
    page.set_ready_state(doc, state);
    if doc == page.document() {
        match state {
            "interactive" => page.note_timing(|t| &mut t.dom_interactive),
            "complete" => page.note_timing(|t| &mut t.dom_complete),
            _ => {}
        }
    }
    crate::input::fire_simple(page, doc, "readystatechange", false, false);
}

/// readyState=interactive, then `DOMContentLoaded` (bubbles to window).
pub fn dom_content_loaded(page: &Rc<PageDom>, doc: NodeId) {
    set_ready_state(page, doc, "interactive");
    flush_autofocus(page, doc);
    // pyro: the faces the styles use start loading (`document.fonts`, early measurements).
    page.start_font_loads();
    page.note_timing(|t| &mut t.dom_content_loaded_start);
    crate::input::fire_simple(page, doc, "DOMContentLoaded", true, false);
    page.note_timing(|t| &mut t.dom_content_loaded_end);
}

/// After parsing: the autofocus elements the parser inserted are flushed at the next rendering
/// update (HTML "flush autofocus candidates"), which the page may not have asked for yet.
fn flush_autofocus(page: &Rc<PageDom>, _doc: NodeId) {
    if page.dom.borrow().has_autofocus_insertions() {
        request_top_level_frames(page);
    }
}

/// Asks for a rendering update of the top-level page(s) sharing `page`'s DOM, whose flush
/// handles the autofocus candidates: a child frame's candidates join its top-level
/// document's, whose page may be idle (no animation frame of its own pending).
fn request_top_level_frames(page: &Rc<PageDom>) {
    if !page.is_nested_frame() {
        if let Some(hooks) = page.hooks() {
            hooks.request_animation_frame();
        }
        return;
    }
    let Some(realm) = page.main_world_realm() else { return };
    let Ok(scope) = realm.scope() else { return };
    for sibling in page.sibling_realms(&scope) {
        let Ok(s) = sibling.scope() else { continue };
        let Ok(other) = crate::target::page_of(&s) else { continue };
        if Rc::ptr_eq(&other.dom, &page.dom) && !other.is_nested_frame() && !other.is_disposed() {
            if let Some(hooks) = other.hooks() {
                hooks.request_animation_frame();
            }
        }
    }
}

/// The page whose top-level document's autofocus candidates `el` joins: its own document's
/// when that is a top-level one, the top-level `page`'s for a child frame of the same origin,
/// none in a cross-origin frame or a document without a browsing context.
fn autofocus_top_page(page: &Rc<PageDom>, el: NodeId) -> Option<Rc<PageDom>> {
    let doc = page.dom.borrow().node_document(el).ok()?;
    let owner = crate::custom::document_page(page, doc)?;
    if !owner.is_nested_frame() {
        return Some(owner);
    }
    (!page.is_nested_frame() && owner.origin_key() == page.origin_key()).then(|| page.clone())
}

/// HTML's autofocus insertion steps for the elements inserted since the last flush: each
/// moves to the end of its top-level document's candidates, unless that document already
/// processed autofocus.
fn collect_autofocus_candidates(page: &Rc<PageDom>) {
    let inserted = page.dom.borrow_mut().take_autofocus_insertions();
    for el in inserted {
        let Some(top) = autofocus_top_page(page, el) else { continue };
        if top.autofocus_processed.get() {
            continue;
        }
        let mut candidates = top.autofocus_candidates.borrow_mut();
        candidates.retain(|&c| c != el);
        candidates.push(el);
    }
}

/// HTML "flush autofocus candidates" for the top-level document of `page`, at its rendering
/// update (before the scroll steps and the animation frame callbacks). The first candidate
/// that is (or delegates to) a focusable area in a fully active document of this top-level
/// one gets focus, unless something is focused already or a URL fragment targets an element;
/// either way autofocus is then processed for good. An element in a `<dialog>` is left to
/// the dialog's own focusing steps.
pub fn flush_autofocus_candidates(page: &Rc<PageDom>) {
    if page.is_disposed() {
        return;
    }
    // A child frame's rendering update hands its document's new candidates to the top-level
    // page's next one.
    if page.is_nested_frame() {
        if page.dom.borrow().has_autofocus_insertions() {
            request_top_level_frames(page);
        }
        return;
    }
    collect_autofocus_candidates(page);
    if page.autofocus_processed.get() || page.autofocus_candidates.borrow().is_empty() {
        return;
    }
    let doc = page.document();
    let settled = {
        let dom = page.dom.borrow();
        dom.focused(doc).ok().flatten().is_some() || dom.url_target_element(doc).is_some()
    };
    if settled {
        page.autofocus_candidates.borrow_mut().clear();
        page.autofocus_processed.set(true);
        return;
    }
    loop {
        let el = {
            let mut candidates = page.autofocus_candidates.borrow_mut();
            if candidates.is_empty() {
                return;
            }
            candidates.remove(0)
        };
        let Some((owner, target)) = autofocus_target(page, el) else { continue };
        page.autofocus_candidates.borrow_mut().clear();
        page.autofocus_processed.set(true);
        crate::input::focus_element(&owner, target);
        return;
    }
}

/// The candidate `el`'s page and what takes focus for it, if anything.
fn autofocus_target(page: &Rc<PageDom>, el: NodeId) -> Option<(Rc<PageDom>, NodeId)> {
    let doc = page.dom.borrow().node_document(el).ok()?;
    let owner = crate::custom::document_page(page, doc)?;
    if !owner.is_fully_active() {
        return None;
    }
    let target = {
        let layout = owner.layout();
        let dom = page.dom.borrow();
        if !dom.is_connected(el).unwrap_or(false) || dom.url_target_element(doc).is_some() {
            return None;
        }
        let mut ancestor = dom.parent(el).ok().flatten();
        while let Some(a) = ancestor {
            if dom.element(a).is_ok_and(|e| e.is_html() && &*e.name.local == "dialog") {
                return None;
            }
            ancestor = dom.parent(a).ok().flatten();
        }
        let area = |n: NodeId| crate::input::is_focusable_area(&layout, &dom, n);
        if area(el) {
            Some(el)
        } else if dom.is_delegates_focus_host(el) {
            dom.focus_delegate(el, &area)
        } else {
            None
        }
    }?;
    Some((owner, target))
}

/// readyState=complete, then `load` and `pageshow` at the window (with
/// `doc` as their target).
pub fn load(page: &Rc<PageDom>, doc: NodeId) {
    set_ready_state(page, doc, "complete");
    page.start_font_loads();
    if let Some(w) = window_key(page, doc) {
        page.note_timing(|t| &mut t.load_start);
        let ev = EventCore::trusted(page, EventInterface::Event, "load", false, false);
        // While the window's `load` runs: a frame of this document's own iframe re-inserted now
        // stays on its initial document (pyro-browser `FrameHost::connected`, as in Chrome).
        let outer = page.dispatching_load.replace(true);
        fire_for_document(page, w, doc, &ev);
        page.dispatching_load.set(outer);
        page.note_timing(|t| &mut t.load_end);
        fire_for_document(page, w, doc, &page_transition_event(page, "pageshow"));
    }
    if doc == page.document() {
        page.set_completely_loaded();
    }
}

/// HTML fires a document's `load`, `pageshow`, `pagehide`, `unload` and
/// `beforeunload` at its Window with the legacy target override flag: the
/// Window's listeners run (it is the whole path, at the target phase) and
/// the event's `target` is the document, as in Chrome (`event.target ===
/// document`, `event.currentTarget === window`). Returns false if canceled.
fn fire_for_document(page: &PageDom, window: pyro_dom::events::TargetKey, doc: NodeId, event: &EventData) -> bool {
    let doc_key = page.dom.borrow().node_key(doc);
    match doc_key {
        Some(doc_key) => fire_with_target_override(page, window, doc_key, event),
        None => fire(page, window, event),
    }
}

/// HTML "fire a page transition event": `pageshow` or `pagehide`, a
/// PageTransitionEvent that bubbles and is cancelable (`persisted` false:
/// there is no back/forward cache).
fn page_transition_event(page: &Rc<PageDom>, ty: &str) -> EventData {
    EventCore::trusted(page, EventInterface::PageTransitionEvent, ty, true, true)
}

/// HTML "prompt to unload" for a document about to be navigated away from:
/// fires `beforeunload` (cancelable) at its Window, the document as its
/// target. Returns whether a listener asked for the prompt (canceled the
/// event or set `returnValue`). There is nobody to answer one, and Chrome
/// shows none without a user gesture, so the embedder navigates either way.
pub fn before_unload(page: &Rc<PageDom>) -> bool {
    if page.is_disposed() || page.scripting_disabled() {
        return false;
    }
    let Some(window) = page.window_target() else { return false };
    if !page.dom.borrow().has_listeners(window) {
        return false;
    }
    let ev = EventCore::trusted(page, EventInterface::BeforeUnloadEvent, "beforeunload", false, true);
    let canceled = !fire_for_document(page, window, page.document(), &ev);
    let asked = ev.fields.borrow().data.as_deref().is_some_and(|text| !text.is_empty());
    canceled || asked
}

/// HTML "unload" of a document being navigated away from or closed, as
/// Chrome runs it (no back/forward cache): `pagehide` (persisted false) at
/// the Window, the visibility state becomes "hidden" with
/// `visibilitychange` at the document (bubbling to the Window), then
/// `unload` at the Window (`pagehide` and `unload` with the document as
/// their target). Runs once per document. Documents without listeners for
/// any of them only get the flag set: no event is made.
/// The embedder calls it for a navigable's descendants first, and keeps
/// the network running afterwards, so keepalive requests made by the
/// listeners (`sendBeacon`, `fetch(.., {keepalive})`) go out.
pub fn unload(page: &Rc<PageDom>) {
    unload_document(page, true);
}

/// The events of a document whose frame was removed from its parent (its iframe left the document),
/// as Chrome fires them, synchronously after the removal: `pagehide` and `visibilitychange` (now
/// hidden), and `unload` only when `fire_unload` (the frame being removed, when its container's
/// permissions policy allows `unload`; never its descendants). HTML's "destroy a child navigable"
/// fires none of them; pyro follows Chrome.
pub fn removed(page: &Rc<PageDom>, fire_unload: bool) {
    unload_document(page, fire_unload);
}

fn unload_document(page: &Rc<PageDom>, fire_unload: bool) {
    // Once per document, marked before any listener runs: one that removes an ancestor frame unloads
    // that frame's documents, this one among them, which must not get its events again.
    if page.is_disposed() || page.is_hidden() || !page.start_unloading() {
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
        fire_for_document(page, window, doc, &page_transition_event(page, "pagehide"));
    }
    page.set_hidden(true);
    let (window_listens, document_listens) = listens(page);
    if document_listens || window_listens {
        crate::input::fire_simple(page, doc, "visibilitychange", true, false);
    }
    // A listener may have added an `unload` one: look again.
    if fire_unload && page.dom.try_borrow().is_ok_and(|dom| dom.has_listeners(window)) {
        let ev = EventCore::trusted(page, EventInterface::Event, "unload", false, false);
        fire_for_document(page, window, doc, &ev);
    }
    // The document is no longer fully active: its deferred fetches go out now, those its
    // `pagehide` and `unload` listeners queued included (Fetch "process deferred fetches").
    crate::fetch_later::flush(page);
}

fn window_key(page: &PageDom, _doc: NodeId) -> Option<pyro_dom::events::TargetKey> {
    page.window_target()
}

/// HTML "report an exception" for a Window, installed on page realms as
/// their `pyro_js::ExceptionReporter`: an uncaught exception of a callback
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
            s.report_exception(&JsError::thrown(value));
        }));
        return true;
    }
    let ev = EventCore::trusted(&page, EventInterface::ErrorEvent, "error", false, true);
    // HTML "muted errors": an exception from a classic script fetched CORS-cross-origin reports
    // "Script error." with no location and a null error (the console report keeps the details).
    if exception.url.as_deref().is_some_and(|url| page.script_errors_muted(url)) {
        let mut fields = ev.fields.borrow_mut();
        fields.message = "Script error.".to_owned();
        fields.filename = String::new();
        fields.lineno = 0;
        fields.colno = 0;
        fields.error = None;
    } else {
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
    // Every listener runs before the microtasks they queue (the error is
    // reported inside the script's run, before its clean-up checkpoint).
    let not_canceled = {
        let (p, e) = (page.clone(), ev.clone());
        s.in_script_frame(move |_s| fire(&p, target, &e)).unwrap_or_else(|_| fire(&page, target, &ev))
    };
    page.error_event_world.set(None);
    page.error_reporting.set(false);
    if not_canceled {
        page.reported_errors.set(page.reported_errors.get().saturating_add(1));
    }
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

/// HTML "reveal" (update the rendering, before the animation frame callbacks): the first
/// rendering update of a document fires `pagereveal` at its Window, once: a PageRevealEvent whose
/// `viewTransition` is null (there are no view transitions here). Gap: a document that never
/// requests an animation frame never gets it.
pub fn reveal(page: &Rc<PageDom>) {
    if page.revealed.replace(true) {
        return;
    }
    let Some(w) = page.window_target() else { return };
    let ev = EventCore::trusted(page, EventInterface::PageRevealEvent, "pagereveal", false, false);
    fire(page, w, &ev);
}

/// HTML `pageswap` at the Window of a document that a cross-document navigation is about to
/// replace, before it unloads. `activation` makes the event's NavigationActivation (null for a
/// navigation to another origin); it runs only when the Window has listeners, which is all the
/// event costs otherwise.
pub fn page_swap(page: &Rc<PageDom>, activation: impl FnOnce() -> Option<pyro_js::Persistent>) {
    if page.is_disposed() || page.scripting_disabled() {
        return;
    }
    let Some(window) = page.window_target() else { return };
    if !page.dom.try_borrow().is_ok_and(|dom| dom.has_listeners(window)) {
        return;
    }
    let ev = EventCore::trusted(page, EventInterface::PageSwapEvent, "pageswap", false, false);
    ev.fields.borrow_mut().activation = activation();
    fire(page, window, &ev);
}

/// `popstate` at the window (history traversal / back-forward).
pub fn popstate(page: &Rc<PageDom>, state: Option<Persistent>) {
    let Some(w) = page.window_target() else { return };
    let ev = EventCore::trusted(page, EventInterface::PopStateEvent, "popstate", false, false);
    ev.fields.borrow_mut().state = state;
    fire(page, w, &ev);
}

/// HTML "focus update steps" for a document entering (`focus`) or leaving (`blur`) the page's
/// focus chain: a FocusEvent at its Window, which does not bubble.
pub fn window_focus(page: &Rc<PageDom>, focused: bool) {
    let Some(w) = page.window_target() else { return };
    let ev = EventCore::trusted(page, EventInterface::FocusEvent, if focused { "focus" } else { "blur" }, false, false);
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
    realm_detached(page, realm);
    page.unregister_realm(realm);
    page.forget_realm_wrappers(realm);
}

/// `realm_disposed` for a realm that lingers after its document was
/// detached ([`PageDom::detach`]): its loads and observers end and the
/// bindings drop their roots into it, but its wrappers stay registered, so
/// a node keeps its wrapper while script still reaches the document.
pub fn realm_detached(page: &PageDom, realm: RealmId) {
    page.web_tasks.cancel_realm(realm);
    crate::crypto_subtle::public_key::cancel(page.id(), Some(realm));
    crate::fetch::cancel_realm(page, realm);
    crate::websocket::cancel_realm(realm);
    crate::cookie_store::events::detach(page, realm);
    crate::xhr::cancel_realm(page, realm);
    crate::file_reader::cancel_realm(page, realm);
    crate::mutation_observer::cancel_realm(page, realm);
    crate::intersection_observer::cancel_realm(page, realm);
    crate::resize_observer::cancel_realm(page, realm);
    page.encoding_helpers.borrow_mut().remove(&realm);
    page.blob_parts.borrow_mut().remove(&realm);
    page.header_helpers.borrow_mut().remove(&realm);
    page.url_params_helpers.borrow_mut().remove(&realm);
    crate::custom::realm_disposed(page, realm);
    page.window_holders.borrow_mut().remove(&realm);
    drop(page.same_objects.borrow_mut().remove(&realm));
    drop(page.structured_clones.borrow_mut().remove(&realm));
    drop(page.define_properties.borrow_mut().remove(&realm));
    drop(page.event_unforgeables.borrow_mut().remove(&realm));
    page.pinned.borrow_mut().retain(|(r, _), _| *r != realm);
}

/// The loader fetched the parser-blocking script `node` written to the
/// script-created parser (`PageHooks::written_script_queued`).
pub fn written_script_fetched(page: &Rc<PageDom>, node: NodeId, url: &str, source: Result<String, String>) {
    crate::html::written_script_fetched(page, node, url, source);
}
