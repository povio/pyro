//! Session history, `Location`, `History` and `Performance`.
//!
//! [`PageNav`] is the per-page source of truth for the current URL and the
//! joint session history. JS-facing classes read it synchronously; anything
//! that needs `&mut Browser` (cross-document navigation, events) is posted as
//! a task, as navigation is asynchronous in browsers too.
//!
//! History state is a structured serialization (`pyro_webapi::WireMessage`),
//! deserialized once per activation in each realm that reads it. Known gap
//! (see docs/PARITY.md): traversal across documents re-fetches (no bfcache).
//! As in browsers, the first navigation replaces the initial about:blank
//! entry.

use crate::{
    Browser, FrameId, HistoryHandling, NavigateOpts, NavigationReason, ObserverCell, PageEvent, PageId,
    SameDocumentKind,
};
use pyro_dom::parse::DomRef;
use pyro_dom::NodeId;
use pyro_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Persistent, Scope, Value};
use pyro_runtime::{Scheduler, TaskSource};
use pyro_webapi::performance::{
    self as perf, navigation_fields, resource_object, NavigationValues, PerformanceEntryData,
    PerformanceNavigationTiming, PerformancePaintTiming, UserTiming, RESOURCE_TIMES, TIMING_ATTRIBUTES,
};
use pyro_webapi::resource_timing::ResourceEntry;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;
use url::Url;

/// A history entry's serialized state (StructuredSerializeForStorage).
/// Entries compare by identity of their state, never by its content.
#[derive(Clone)]
struct HistoryState(Rc<pyro_webapi::WireMessage>);

impl PartialEq for HistoryState {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for HistoryState {}
impl std::fmt::Debug for HistoryState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HistoryState")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    url: String,
    /// The serialized history state (`None` = null).
    state: Option<HistoryState>,
    /// Document sequence number: entries of the same document traverse
    /// without reloading.
    doc: u64,
    /// Unique within the page; identifies the entry to automation clients
    /// (CDP `NavigationEntry.id`).
    id: u64,
    /// Navigation API key: the history slot, kept when the entry is
    /// replaced.
    key: u64,
    /// Navigation API state (`JSON.stringify`; `None` = undefined).
    nav_state: Option<String>,
}

/// A session-history entry as reported to automation clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub id: u64,
    pub url: String,
}

struct NavState {
    url: String,
    entries: Vec<Entry>,
    index: usize,
    doc_seq: u64,
    entry_seq: u64,
    /// Bumped whenever the active entry's state is (re)activated: pushState,
    /// replaceState and same-document traversal. `history.state` returns one
    /// deserialized object per realm until it changes.
    state_version: u64,
    scroll_restoration_manual: bool,
    /// The viewport scroll position persisted for an entry of this document when it stopped
    /// being the current one (HTML "persist the history entry state"), by entry id.
    scroll_positions: std::collections::HashMap<u64, (i32, i32)>,
    /// The only entry is the page's initial about:blank document, which the
    /// next navigation replaces (HTML "is initial about:blank").
    initial: bool,
    /// The active document is that initial about:blank one, though
    /// `document.open()` or `pushState` may have cleared `initial` since: no
    /// document has been fetched for the frame yet.
    first_blank: bool,
    /// `document.open()` calls on the frame's documents (HTML "stop
    /// loading"): a navigation its scripts started before one is dropped.
    stops: u64,
}

impl NavState {
    fn next_entry_id(&mut self) -> u64 {
        self.entry_seq += 1;
        self.entry_seq
    }
}

/// Per-page navigation state shared by the page and its windows.
pub struct PageNav {
    page: PageId,
    frame_id: FrameId,
    observer: ObserverCell,
    /// URL of a synchronous same-document commit not yet copied to
    /// `Frame::url` (done at the next checkpoint, outside JavaScript).
    pending_frame_url: RefCell<Option<String>>,
    /// Latest queued script navigation; a later request supersedes earlier work.
    script_navigation: Cell<u64>,
    sched: Scheduler<Browser>,
    state: RefCell<NavState>,
    /// Current document in the page's DOM (keeps `document.URL` in sync).
    document: RefCell<Option<(DomRef, NodeId)>>,
    /// The current document's bindings: the target of the `popstate` a
    /// fragment navigation fires synchronously.
    page_dom: RefCell<std::rc::Weak<pyro_webapi::PageDom>>,
    time_origin: Cell<Instant>,
    /// When the current document first had a rendering opportunity
    /// (`now_ms`): its Paint Timing entries.
    first_paint: Cell<Option<f64>>,
    /// `PerformanceNavigationTiming.type` of the current document.
    nav_type: Cell<&'static str>,
    /// The fetch of the document about to commit (`begin_document`): its
    /// start becomes the time origin, its phases the Navigation Timing.
    incoming_fetch: RefCell<Option<Box<pyro_webapi::resource_timing::FetchReport>>>,
    /// The current document's fetch phases, in milliseconds since its time
    /// origin (`None`: no network fetch, as for `about:blank`).
    fetch_timing: RefCell<Option<Rc<pyro_webapi::resource_timing::ResourceEntry>>>,
    /// The realms with a PerformanceObserver (performance_observer.js) and
    /// the function each queues new timeline entries with. Empty unless a
    /// page observes, which is all the timeline costs otherwise.
    timeline_watchers: RefCell<Vec<(pyro_js::RealmId, pyro_js::WeakRealm, Rc<Persistent>)>>,
    /// The realms with a `navigation` object (navigation.js) and the hook
    /// each fires the Navigation API's events with. Empty unless a page
    /// reads `navigation`, which is all navigations cost otherwise.
    navigation_hooks: RefCell<Vec<(pyro_js::RealmId, pyro_js::WeakRealm, Rc<Persistent>)>>,
    /// A navigation that navigation.js started (and fired `navigate` for)
    /// is running: the native paths fire no event of their own.
    navigation_api_call: Cell<bool>,
}

/// What the listeners of a Navigation API `navigate` event decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NavigateDecision {
    Proceed,
    /// `preventDefault()`: the navigation does not happen.
    Cancel,
    /// `intercept()`: the navigation becomes a same-document one.
    Intercept,
}

/// Where a navigation comes from, for its `navigate` event.
#[derive(Default)]
pub(crate) struct NavigateSource {
    /// `NavigateEvent.sourceElement`: the link or the form (its submitter)
    /// and the bindings of the document it is in, when that document is
    /// same-origin with the one that navigates (the caller checks). The
    /// event of another page reads null.
    pub element: Option<(Rc<pyro_webapi::PageDom>, NodeId)>,
    /// `NavigateEvent.formData`: a POST form submission's entry list.
    pub form_entries: Option<pyro_webapi::formdata::FormEntries>,
    /// History handling "replace" (a form submitted while its document loads).
    pub replace: bool,
}

impl PageNav {
    pub(crate) fn new(
        page: PageId,
        frame_id: FrameId,
        sched: Scheduler<Browser>,
        observer: ObserverCell,
    ) -> Rc<PageNav> {
        Rc::new(PageNav {
            page,
            frame_id,
            observer,
            pending_frame_url: RefCell::new(None),
            script_navigation: Cell::new(0),
            sched,
            state: RefCell::new(NavState {
                url: "about:blank".into(),
                entries: Vec::new(),
                index: 0,
                doc_seq: 0,
                entry_seq: 0,
                state_version: 0,
                scroll_restoration_manual: false,
                scroll_positions: Default::default(),
                initial: false,
                first_blank: false,
                stops: 0,
            }),
            document: RefCell::new(None),
            page_dom: RefCell::new(std::rc::Weak::new()),
            time_origin: Cell::new(crate::clock::now()),
            first_paint: Cell::new(None),
            nav_type: Cell::new("navigate"),
            incoming_fetch: RefCell::new(None),
            fetch_timing: RefCell::new(None),
            timeline_watchers: RefCell::new(Vec::new()),
            navigation_hooks: RefCell::new(Vec::new()),
            navigation_api_call: Cell::new(false),
        })
    }

    pub fn url(&self) -> String {
        self.state.borrow().url.clone()
    }

    pub fn history_length(&self) -> usize {
        self.state.borrow().entries.len().max(1)
    }

    /// The session history and the index of the current entry.
    pub fn history_entries(&self) -> (usize, Vec<HistoryEntry>) {
        let st = self.state.borrow();
        let entries = st.entries.iter().map(|e| HistoryEntry { id: e.id, url: e.url.clone() }).collect();
        (st.index, entries)
    }

    /// Queues a traversal to the entry with `id`, like `history.go()` with the
    /// matching delta (the current entry reloads). Returns false when no entry
    /// has that id.
    pub fn traverse_to_entry(&self, id: u64) -> bool {
        let delta = {
            let st = self.state.borrow();
            match st.entries.iter().position(|e| e.id == id) {
                Some(i) => i as i64 - st.index as i64,
                None => return false,
            }
        };
        self.traverse(delta);
        true
    }

    /// The active document is the page's initial about:blank one (no
    /// navigation has committed since it).
    pub(crate) fn is_initial_blank(&self) -> bool {
        self.state.borrow().initial
    }

    /// The active document is the frame's initial about:blank one, whether
    /// or not a script has opened it since (no document has been fetched).
    pub(crate) fn is_first_blank(&self) -> bool {
        self.state.borrow().first_blank
    }

    /// HTML "document open steps" on the active document (`document.open()`,
    /// or a `write()` that opens):
    /// - "stop loading": a navigation its scripts started and that has not
    ///   committed is dropped ([`PageNav::stop_count`]), as Chrome does;
    /// - it is no longer the initial about:blank document: a navigation
    ///   started after it pushes an entry (`page.setContent`, then `goto`:
    ///   Chrome's history.length is 2), one started before it replaces.
    pub(crate) fn document_opened(&self) {
        let mut st = self.state.borrow_mut();
        st.initial = false;
        st.stops += 1;
    }

    /// Changes when the frame stops loading ([`PageNav::document_opened`]):
    /// a script navigation taken before a change is dropped after it.
    pub(crate) fn stop_count(&self) -> u64 {
        self.state.borrow().stops
    }

    /// Records a committed cross-document navigation.
    pub(crate) fn commit_document(&self, url: &str, history: HistoryHandling, document: Option<(DomRef, NodeId)>) {
        self.commit_document_at(url, url, history, document);
    }

    /// [`PageNav::commit_document`] of a document at `url` whose session
    /// history entry has `entry_url`: an error document's entry keeps the
    /// URL that could not be reached, as Chrome's does.
    pub(crate) fn commit_document_at(
        &self,
        url: &str,
        entry_url: &str,
        history: HistoryHandling,
        document: Option<(DomRef, NodeId)>,
    ) {
        self.pending_frame_url.borrow_mut().take();
        let mut st = self.state.borrow_mut();
        st.doc_seq += 1;
        let id = st.next_entry_id();
        // A push to the current entry's URL replaces it (HTML navigate step:
        // "url equals the active document's URL" -> replace).
        let same_url = st.entries.get(st.index).is_some_and(|e| e.url == entry_url);
        let replaces = matches!(history, HistoryHandling::Replace) || same_url;
        let key = match st.entries.get(st.index) {
            Some(current) if replaces => current.key,
            _ => st.next_entry_id(),
        };
        let entry = Entry { url: entry_url.to_string(), state: None, doc: st.doc_seq, id, key, nav_state: None };
        let initial = std::mem::take(&mut st.initial);
        st.first_blank = false;
        match history {
            _ if st.entries.is_empty() => {
                st.initial = url == "about:blank";
                st.first_blank = st.initial;
                st.entries.push(entry);
                st.index = 0;
            }
            HistoryHandling::Push if initial && st.entries.len() == 1 => {
                st.entries[0] = entry;
                st.index = 0;
            }
            HistoryHandling::Traverse | HistoryHandling::Reload => {
                // The entry at the index (set by traverse() for traversals)
                // keeps its id and state; a redirect updates its URL.
                let (i, doc) = (st.index, st.doc_seq);
                let e = &mut st.entries[i];
                e.doc = doc;
                e.url = entry_url.to_string();
            }
            HistoryHandling::Replace => {
                let i = st.index;
                st.entries[i] = entry;
            }
            HistoryHandling::Push if same_url => {
                let i = st.index;
                st.entries[i] = entry;
            }
            HistoryHandling::Push => {
                let i = st.index + 1;
                st.entries.truncate(i);
                st.entries.push(entry);
                st.index = i;
            }
        }
        st.url = url.to_string();
        drop(st);
        *self.document.borrow_mut() = document;
        // The previous document's realms are disposed with it: drop their
        // hooks now, so a frame whose later documents never fire an event
        // does not keep their globals protected.
        self.timeline_watchers.borrow_mut().clear();
        self.navigation_hooks.borrow_mut().clear();
        // The time origin is the start of the document's fetch (Chrome:
        // the navigation's start), so its phases have their own times.
        let now = crate::clock::now();
        let fetch = self.incoming_fetch.take();
        let origin = fetch
            .as_ref()
            .and_then(|f| now.checked_sub(Instant::now().saturating_duration_since(f.start)))
            .unwrap_or(now);
        self.time_origin.set(origin);
        *self.fetch_timing.borrow_mut() = fetch.map(|f| {
            let mut entry = pyro_webapi::resource_timing::entry_of(&f, f.start, 0.0);
            // Navigation Timing hides redirects through other origins.
            if f.response.as_ref().is_some_and(|r| r.timing.cross_origin_redirect) {
                entry.redirect_start = 0.0;
                entry.redirect_end = 0.0;
                entry.redirect_count = 0;
            }
            Rc::new(entry)
        });
        self.first_paint.set(None);
        self.nav_type.set(match history {
            HistoryHandling::Reload => "reload",
            HistoryHandling::Traverse => "back_forward",
            _ => "navigate",
        });
    }

    /// The fetch of the document that commits next (see `incoming_fetch`);
    /// `None` clears one a failed commit left.
    pub(crate) fn set_incoming_fetch(&self, fetch: Option<Box<pyro_webapi::resource_timing::FetchReport>>) {
        *self.incoming_fetch.borrow_mut() = fetch;
    }

    pub(crate) fn set_document(&self, document: Option<(DomRef, NodeId)>) {
        *self.document.borrow_mut() = document;
    }

    /// The bindings of the document [`PageNav::commit_document`] recorded.
    pub(crate) fn set_page_dom(&self, page: &Rc<pyro_webapi::PageDom>) {
        *self.page_dom.borrow_mut() = Rc::downgrade(page);
    }

    /// HTML "update document for history step application" for a fragment
    /// navigation: `popstate` (state null) at the window right away, before
    /// the queued `hashchange`. Dispatch runs script, so when the DOM is
    /// borrowed (never the case for a binding call) it moves to a task. A
    /// window without listeners pays one lookup.
    fn fire_fragment_popstate(&self) {
        let Some(page) = self.page_dom.borrow().upgrade() else { return };
        if page.is_disposed() || page.scripting_disabled() {
            return;
        }
        let Some(window) = page.window_target() else { return };
        match page.dom.try_borrow() {
            Ok(dom) if !dom.has_listeners(window) => return,
            Ok(_) => {}
            Err(_) => {
                let weak = Rc::downgrade(&page);
                let document = self.state.borrow().doc_seq;
                let frame_id = self.frame_id.clone();
                let page_id = self.page;
                self.sched.post(TaskSource::Navigation, move |b| {
                    let Some(key) = b.page(page_id).and_then(|p| p.frames.key(&frame_id)) else { return };
                    if !b.doc(key).is_some_and(|p| p.nav.state.borrow().doc_seq == document) {
                        return;
                    }
                    if let Some(page) = weak.upgrade() {
                        pyro_webapi::lifecycle::popstate(&page, None);
                    }
                });
                return;
            }
        }
        pyro_webapi::lifecycle::popstate(&page, None);
    }

    /// Initial iframe attribute processing updates the existing blank entry
    /// for about:blank URLs with a query or fragment, without a new Document.
    pub(crate) fn update_initial_blank_url(&self, url: &str) {
        let mut state = self.state.borrow_mut();
        let index = state.index;
        if let Some(entry) = state.entries.get_mut(index) { entry.url = url.to_owned(); }
        drop(state);
        self.set_url_sync(url);
    }

    fn set_url_sync(&self, url: &str) {
        self.state.borrow_mut().url = url.to_string();
        if let Some((dom, doc)) = self.document.borrow().as_ref() {
            let _ = dom.borrow_mut().set_document_url(*doc, url);
        }
    }

    /// Same-document navigation (pushState/replaceState/fragment).
    fn same_document(&self, url: &str, state: Option<HistoryState>, kind: SameDocumentKind) {
        self.persist_scroll();
        {
            let mut st = self.state.borrow_mut();
            st.state_version += 1;
            let doc = st.entries.get(st.index).map_or(st.doc_seq, |e| e.doc);
            let id = st.next_entry_id();
            // The Navigation API state carries over (HTML "URL and history
            // update steps"); a replace keeps the slot's key.
            let current = st.entries.get(st.index);
            let nav_state = current.and_then(|e| e.nav_state.clone());
            let key = match current {
                Some(e) if kind == SameDocumentKind::Replace => e.key,
                _ => st.next_entry_id(),
            };
            let entry = Entry { url: url.to_string(), state, doc, id, key, nav_state };
            st.initial = false;
            if st.entries.is_empty() {
                st.entries.push(entry);
            } else if kind == SameDocumentKind::Replace {
                let i = st.index;
                st.entries[i] = entry;
            } else {
                let i = st.index + 1;
                st.entries.truncate(i);
                st.entries.push(entry);
                st.index = i;
            }
        }
        self.set_url_sync(url);
        // Reported synchronously, like browsers: anything the client observes
        // after this JavaScript turn (a CDP reply, a network event) must come
        // after the URL change. Frame::url follows at the next checkpoint.
        *self.pending_frame_url.borrow_mut() = Some(url.to_string());
        let event =
            PageEvent::FrameNavigatedWithinDocument { frame_id: self.frame_id.clone(), url: url.to_string(), kind };
        crate::emit_to(&self.observer, self.page, &event);
    }

    pub(crate) fn has_pending_frame_url(&self) -> bool {
        self.pending_frame_url.borrow().is_some()
    }
    pub(crate) fn take_pending_frame_url(&self) -> Option<String> {
        self.pending_frame_url.borrow_mut().take()
    }

    /// HTML "navigate to a javascript: URL" in this frame's document: a task evaluates the
    /// URL's script there (the document stays: a string result does not replace it). A document
    /// that requires Trusted Types for script runs what its default policy makes of the script
    /// (sink "Location href"), or nothing.
    fn run_javascript_url(&self, url: &str) {
        let Some(source) = crate::bindings::javascript_url_source(url) else { return };
        let page = self.page;
        let frame_id = self.frame_id.clone();
        let document = self.state.borrow().doc_seq;
        self.sched.post(TaskSource::Navigation, move |b| {
            let Some(key) = b.page(page).and_then(|p| p.frames.key(&frame_id)) else { return };
            let Some(doc) = b.doc(key).filter(|p| p.nav.state.borrow().doc_seq == document) else { return };
            let (realm, page_dom) = (doc.main_frame.main_realm().cloned(), doc.main_frame.page_dom.clone());
            drop(doc);
            let (Some(realm), Some(page_dom)) = (realm, page_dom) else { return };
            let Ok(scope) = realm.scope() else { return };
            let Some(source) = pyro_webapi::trusted_types::javascript_url_script(&scope, &page_dom, &source) else { return };
            if let Err(e) = scope.eval(&source, None, 1) {
                scope.report_exception(&e);
            }
        });
    }

    fn navigate(&self, url: String, history: HistoryHandling) {
        // The frame's latest navigation from now on: it supersedes those
        // started before it (a planned form submission too), and a later
        // one supersedes it (`PageDom::navigation_started`).
        let started = self.page_dom.borrow().upgrade().map(|page| {
            page.note_navigation_request();
            page.navigation_started()
        });
        let sequence = self.script_navigation.get().wrapping_add(1);
        self.script_navigation.set(sequence);
        let page = self.page;
        let (document, stops) = {
            let st = self.state.borrow();
            (st.doc_seq, st.stops)
        };
        let frame_id = self.frame_id.clone();
        self.sched.post(TaskSource::Navigation, move |b| {
            let Some(page) = b.page(page).and_then(|p| p.frames.key(&frame_id)) else { return };
            // Another document, or this one was opened since (stop loading).
            if !b.doc(page).is_some_and(|p| {
                let st = p.nav.state.borrow();
                st.doc_seq == document
                    && st.stops == stops
                    && p.nav.script_navigation.get() == sequence
                    && started.is_none_or(|n| p.main_frame.page_dom.as_ref().is_none_or(|d| d.navigation_started() == n))
            }) {
                return;
            }
            let opts = NavigateOpts {
                post: None,
                url,
                reason: NavigationReason::Script,
                cdp_request_id: None,
                // The document that navigates (`location`, `window.open`).
                referrer: b.doc(page).map(|p| p.main_frame.url.clone()),
                referrer_policy: None,
            };
            if let Err(e) = b.navigate_frame_with_handling(page, opts, history) {
                b.report_navigation_error(page, &e);
            }
        });
    }

    fn traverse(&self, delta: i64) {
        let target = {
            let st = self.state.borrow();
            // The JS number conversion saturates at i64's limits. Add in a
            // wider type so out-of-range deltas remain no-ops, not panics.
            let t = st.index as i128 + i128::from(delta);
            if delta == 0 || t < 0 || t >= st.entries.len() as i128 {
                None
            } else {
                Some((t as usize, st.entries[t as usize].clone(), st.doc_seq))
            }
        };
        let Some((index, entry, current_doc)) = target else {
            if delta == 0 {
                let url = self.url();
                self.navigate(url, HistoryHandling::Reload);
            }
            return;
        };
        // Traversal is queued as a task, like browsers do.
        let page = self.page;
        let frame_id = self.frame_id.clone();
        self.sched.post(TaskSource::Navigation, move |b| {
            let Some(page) = b.page(page).and_then(|p| p.frames.key(&frame_id)) else { return };
            let Some(nav) = b.doc(page).map(|p| p.nav.clone()) else { return };
            // A traversal to another document does not take the place of
            // the frame's planned form navigation that has begun.
            if entry.doc != current_doc
                && b.doc(page).is_some_and(|p| p.pending_planned.is_some() && p.pending_planned.as_ref() == p.pending_loader())
            {
                return;
            }
            if nav.state.borrow().doc_seq == current_doc && nav.state.borrow().entries.get(index) == Some(&entry) {
                // Only a same-document traversal can be canceled or intercepted.
                let same = entry.doc == current_doc;
                let source = NavigateSource::default();
                if nav.navigate_event_to("traverse", &entry.url, same, Some(entry.key), &source)
                    == NavigateDecision::Cancel
                    && same
                {
                    return;
                }
            }
            nav.persist_scroll();
            {
                let mut state = nav.state.borrow_mut();
                // Navigation can replace the document, or pushState can replace
                // a forward entry, while this task waits. Never apply an obsolete
                // entry or write an index that is no longer in the history list.
                if state.doc_seq != current_doc || state.entries.get(index) != Some(&entry) {
                    return;
                }
                state.index = index;
                state.state_version += 1;
            }
            if entry.doc == current_doc {
                let old_url = nav.url();
                nav.set_url_sync(&entry.url);
                nav.restore_scroll(&entry);
                b.same_document_committed(page, entry.url.clone(), SameDocumentKind::Push);
                nav.navigation_committed("traverse");
                b.fire_popstate(page, entry.state.is_some(), &old_url, &entry.url);
            } else {
                let opts = NavigateOpts {
                    post: None,
                    url: entry.url.clone(),
                    reason: NavigationReason::History,
                    cdp_request_id: None,
                    referrer: None,
                    referrer_policy: None,
                };
                if let Err(e) = b.navigate_frame_with_handling(page, opts, HistoryHandling::Traverse) {
                    b.report_navigation_error(page, &e);
                }
            }
        });
    }

    /// HTML "navigate to a fragment", for a navigation the document requested
    /// (a followed hyperlink, a submitted GET form): when `url` is the
    /// document's URL but for a fragment it has, the URL changes at once, in
    /// place when it is the same URL, and `hashchange` is queued. Returns
    /// false for any other URL, which is a navigation to load.
    pub(crate) fn navigate_to_fragment_from(&self, url: &str, source: &NavigateSource) -> bool {
        let (Ok(current), Ok(mut target)) = (Url::parse(&self.url()), Url::parse(url)) else { return false };
        if target.fragment().is_none() || !same_document_except_fragment(&current, &target) {
            return false;
        }
        self.take_fragment_directive(&mut target);
        let kind =
            if current == target || source.replace { SameDocumentKind::Replace } else { SameDocumentKind::Fragment };
        let navigation_type = if kind == SameDocumentKind::Replace { "replace" } else { "push" };
        match self.navigate_event_from(navigation_type, target.as_str(), source) {
            NavigateDecision::Cancel => return true,
            NavigateDecision::Intercept => {
                self.commit_intercepted(target.as_str(), navigation_type);
                return true;
            }
            NavigateDecision::Proceed => {}
        }
        self.same_document(target.as_str(), None, kind);
        self.scroll_to_fragment(&target);
        self.navigation_committed(navigation_type);
        self.fire_fragment_popstate();
        self.queue_hashchange(current.as_str(), target.as_str());
        true
    }

    /// URL Fragment Text Directives: a same-document navigation's directive (`:~:text=...`)
    /// leaves its URL and selects the document's target text.
    fn take_fragment_directive(&self, target: &mut Url) {
        let Some((fragment, directives)) = target
            .fragment()
            .and_then(pyro_webapi::highlights::split_fragment_directive)
            .map(|(fragment, directives)| (fragment.to_owned(), directives.to_owned()))
        else {
            return;
        };
        target.set_fragment(Some(&fragment));
        if let Some(page) = self.page_dom.borrow().upgrade() {
            pyro_webapi::highlights::apply_fragment_directive(&page, &directives);
        }
    }

    /// HTML "persist the history entry state": the viewport's scroll position, kept for the
    /// current entry as it stops being current.
    fn persist_scroll(&self) {
        let Some(page) = self.page_dom.borrow().upgrade() else { return };
        let position = pyro_webapi::layout_api::viewport_scroll(&page);
        let mut st = self.state.borrow_mut();
        if let Some(id) = st.entries.get(st.index).map(|e| e.id) {
            st.scroll_positions.insert(id, position);
        }
    }

    /// HTML "restore persisted state" of a same-document traversal to `entry`: with
    /// `history.scrollRestoration` `auto`, its persisted scroll position; else, without one,
    /// the scroll to its fragment.
    fn restore_scroll(&self, entry: &Entry) {
        let (manual, saved) = {
            let st = self.state.borrow();
            (st.scroll_restoration_manual, st.scroll_positions.get(&entry.id).copied())
        };
        let Some(page) = self.page_dom.borrow().upgrade() else { return };
        match saved {
            Some(position) if !manual => pyro_webapi::layout_api::restore_viewport_scroll(&page, position),
            Some(_) => {}
            None => {
                if let Ok(url) = Url::parse(&entry.url) {
                    self.scroll_to_fragment(&url);
                }
            }
        }
    }

        /// HTML "scroll to the fragment" of a same-document navigation to `target`.
    fn scroll_to_fragment(&self, target: &Url) {
        let Some(fragment) = target.fragment() else { return };
        if let Some(page) = self.page_dom.borrow().upgrade() {
            pyro_webapi::layout_api::scroll_to_fragment(&page, fragment);
        }
    }

        /// Queues `hashchange` at the window when a same-document navigation
    /// changed only the fragment.
    fn queue_hashchange(&self, old_url: &str, new_url: &str) {
        let (Ok(old), Ok(new)) = (Url::parse(old_url), Url::parse(new_url)) else { return };
        if old.fragment() == new.fragment() || !same_document_except_fragment(&old, &new) {
            return;
        }
        let (page, document) = (self.page, self.state.borrow().doc_seq);
        let (old_url, new_url) = (old_url.to_string(), new_url.to_string());
        let frame_id = self.frame_id.clone();
        self.sched.post(TaskSource::Navigation, move |b| {
            let Some(page) = b.page(page).and_then(|p| p.frames.key(&frame_id)) else { return };
            let Some(p) = b.doc(page) else { return };
            // Only for the document that navigated.
            if p.nav.state.borrow().doc_seq != document {
                return;
            }
            if let Some(dom) = p.main_frame.page_dom.clone() {
                pyro_webapi::lifecycle::hashchange(&dom, &old_url, &new_url);
            }
        });
    }

    fn active_state(&self) -> Option<HistoryState> {
        let st = self.state.borrow();
        st.entries.get(st.index).and_then(|e| e.state.clone())
    }

    /// `history.state` in `s`'s realm: the active entry's state deserialized
    /// once per (re)activation and then the same object on every read, as in
    /// browsers (`history.state === history.state`; mutations are visible
    /// until the next pushState, replaceState or traversal). Cached on the
    /// realm's History wrapper (one per realm), so each world has its own
    /// object.
    fn state_value<'s>(nav: &Rc<PageNav>, s: &Scope<'s>) -> JsResult<Value<'s>> {
        const VALUE: &str = "lpHistoryState";
        const VERSION: &str = "lpHistoryStateVersion";
        let version = nav.state.borrow().state_version as f64;
        let history = s.wrap::<History>(nav.clone(), Some(pyro_js::IdentityKey(crate::window::KEY_HISTORY, 0)))?;
        if let (Some(v), Some(cached)) = (s.get_hidden(history, VERSION)?, s.get_hidden(history, VALUE)?) {
            if v.to_number().ok() == Some(version) {
                return Ok(cached);
            }
        }
        let value = match nav.active_state() {
            // A state that cannot be deserialized here reads as null.
            Some(state) => pyro_webapi::deserialize_message(s, &state.0).unwrap_or_else(|_| s.null()),
            None => s.null(),
        };
        s.set_hidden(history, VALUE, value)?;
        s.set_hidden(history, VERSION, s.number(version))?;
        Ok(value)
    }

    /// Milliseconds since the current document's time origin, in steps of
    /// 100 microseconds: the resolution of `performance.now()`, animation
    /// frame times and `Event.timeStamp` in a document that is not
    /// cross-origin isolated (as in Chrome).
    pub fn now_ms(&self) -> f64 {
        self.time_at(Instant::now())
    }

    pub(crate) fn time_at(&self, real: Instant) -> f64 {
        let elapsed = crate::clock::at(real).saturating_duration_since(self.time_origin.get());
        (elapsed.as_micros() / 100) as f64 / 10.0
    }

    /// Time of the current document's first paint, once it had one.
    pub(crate) fn first_paint(&self) -> Option<f64> {
        self.first_paint.get()
    }
    /// Records the first paint (now) unless one was recorded.
    pub(crate) fn note_first_paint(&self) {
        if self.first_paint.get().is_none() {
            self.first_paint.set(Some(self.now_ms()));
            self.notify_timeline("paint");
        }
    }

    /// The document's entry of `entry_type` (`paint`, `navigation`) is in
    /// the timeline now: every observing realm looks for it. Nothing runs
    /// unless a realm observes.
    pub(crate) fn notify_timeline(&self, entry_type: &'static str) {
        if self.timeline_watchers.borrow().is_empty() {
            return;
        }
        let watchers: Vec<_> = {
            let mut watchers = self.timeline_watchers.borrow_mut();
            watchers.retain(|(_, realm, _)| realm.upgrade().is_some_and(|r| !r.is_disposed()));
            watchers.iter().map(|(_, realm, f)| (realm.clone(), f.clone())).collect()
        };
        for (realm, f) in watchers {
            let Some(realm) = realm.upgrade() else { continue };
            let Ok(s) = realm.scope() else { continue };
            if let Some(f) = f.get(&s).as_object() {
                if let Err(e) = f.call(None, &[s.string(entry_type)]) {
                    s.report_exception(&e);
                }
            }
        }
    }

    /// A mark or measure was added in `s`'s realm: its observers queue it.
    fn notify_entry<'s>(&self, s: &Scope<'s>, entry_type: &'static str, entry: Value<'s>) -> JsResult<()> {
        let f = {
            let watchers = self.timeline_watchers.borrow();
            if watchers.is_empty() {
                return Ok(());
            }
            let id = s.realm_id();
            watchers.iter().find(|(realm, ..)| *realm == id).map(|(.., f)| f.clone())
        };
        if let Some(f) = f.and_then(|f| f.get(s).as_object()) {
            f.call(None, &[s.string(entry_type), entry])?;
        }
        Ok(())
    }

    /// The live realms' Navigation API hooks.
    fn navigation_hooks(&self) -> Vec<(pyro_js::WeakRealm, Rc<Persistent>)> {
        let mut hooks = self.navigation_hooks.borrow_mut();
        hooks.retain(|(_, realm, _)| realm.upgrade().is_some_and(|r| !r.is_disposed()));
        hooks.iter().map(|(_, realm, f)| (realm.clone(), f.clone())).collect()
    }

    /// Navigation API "fire a navigate event" for a navigation to `url` of
    /// `navigation_type` (push, replace, reload, traverse), at each realm's
    /// `navigation`. Free unless a realm made one; skipped while the DOM is
    /// borrowed (listeners run script) and for a navigation navigation.js
    /// started itself (it fired the event).
    pub(crate) fn navigate_event(
        &self,
        navigation_type: &'static str,
        url: &str,
        cancelable: bool,
    ) -> NavigateDecision {
        self.navigate_event_to(navigation_type, url, cancelable, None, &NavigateSource::default())
    }

    /// [`PageNav::navigate_event`] for a followed link or a submitted form.
    pub(crate) fn navigate_event_from(
        &self,
        navigation_type: &'static str,
        url: &str,
        source: &NavigateSource,
    ) -> NavigateDecision {
        self.navigate_event_to(navigation_type, url, true, None, source)
    }

    /// [`PageNav::navigate_event`] for a traversal to the entry with `key`.
    fn navigate_event_to(
        &self,
        navigation_type: &'static str,
        url: &str,
        cancelable: bool,
        key: Option<u64>,
        source: &NavigateSource,
    ) -> NavigateDecision {
        if self.navigation_hooks.borrow().is_empty() || self.navigation_api_call.get() {
            return NavigateDecision::Proceed;
        }
        let Some(page) = self.page_dom.borrow().upgrade() else { return NavigateDecision::Proceed };
        if page.is_disposed() || page.dom.try_borrow_mut().is_err() {
            return NavigateDecision::Proceed;
        }
        let (current, target) = (Url::parse(&self.url()).ok(), Url::parse(url).ok());
        let (hash_change, can_intercept) = match (&current, &target) {
            (Some(c), Some(t)) => (
                t.fragment().is_some() && c.fragment() != t.fragment() && same_document_except_fragment(c, t),
                // A traversal to another document cannot be intercepted.
                can_rewrite_url(c, t) && (navigation_type != "traverse" || cancelable),
            ),
            _ => (false, false),
        };
        let mut decision = NavigateDecision::Proceed;
        for (realm, hook) in self.navigation_hooks() {
            let Some(realm) = realm.upgrade() else { continue };
            let Ok(s) = realm.scope() else { continue };
            let Some(f) = hook.get(&s).as_object() else { continue };
            // The entry list becomes a FormData of the event's realm; the
            // element keeps the wrapper of its own document's realm.
            let form_data = source
                .form_entries
                .as_ref()
                .and_then(|entries| pyro_webapi::formdata::form_data_object(&s, entries).ok())
                .map_or(s.null(), |o| o.value());
            let element = source
                .element
                .as_ref()
                .filter(|(from, _)| Rc::ptr_eq(&from.dom, &page.dom))
                .and_then(|(from, id)| pyro_webapi::wrap_node(&s, from, *id).ok())
                .map_or(s.null(), |o| o.value());
            let args = [
                s.string("navigate"),
                s.string(navigation_type),
                s.string(url),
                s.boolean(hash_change),
                s.boolean(cancelable),
                s.boolean(can_intercept),
                s.string(&key.map(|k| k.to_string()).unwrap_or_default()),
                form_data,
                element,
            ];
            match f.call(None, &args).and_then(|v| v.to_string()) {
                Ok(answer) if answer == "cancel" => decision = NavigateDecision::Cancel,
                Ok(answer) if answer == "intercept" && decision != NavigateDecision::Cancel => {
                    decision = NavigateDecision::Intercept
                }
                Ok(_) => {}
                Err(e) => s.report_exception(&e),
            }
        }
        decision
    }

    /// The navigation the last `navigate` event was for committed in this
    /// document (its URL and entry changed, or an intercepted reload):
    /// `currententrychange`, then the intercept handlers or
    /// `navigatesuccess`.
    pub(crate) fn navigation_committed(&self, navigation_type: &'static str) {
        if self.navigation_hooks.borrow().is_empty() || self.navigation_api_call.get() {
            return;
        }
        for (realm, hook) in self.navigation_hooks() {
            let Some(realm) = realm.upgrade() else { continue };
            let Ok(s) = realm.scope() else { continue };
            if let Some(f) = hook.get(&s).as_object() {
                if let Err(e) = f.call(None, &[s.string("commit"), s.string(navigation_type)]) {
                    s.report_exception(&e);
                }
            }
        }
    }

    /// An intercepted navigation to `url` (HTML "navigate", Navigation API
    /// intercept): the document stays and its URL changes, as pushState
    /// does, then the handlers run.
    pub(crate) fn commit_intercepted(&self, url: &str, navigation_type: &'static str) {
        let kind = if navigation_type == "replace" { SameDocumentKind::Replace } else { SameDocumentKind::Push };
        if navigation_type != "reload" {
            self.same_document(url, None, kind);
        }
        self.navigation_committed(navigation_type);
    }

    /// Sets the current entry's Navigation API state (`None`: undefined).
    fn set_navigation_state(&self, state: Option<String>) {
        let mut st = self.state.borrow_mut();
        let i = st.index;
        if let Some(entry) = st.entries.get_mut(i) {
            entry.nav_state = state;
        }
    }

    /// `natives.watch(queue)` of performance_observer.js: `queue` (or null
    /// to stop) gets the entries of `s`'s realm.
    fn watch_timeline(self: &Rc<Self>, s: &Scope<'_>, queue: Value<'_>) {
        let id = s.realm_id();
        let watched = {
            let mut watchers = self.timeline_watchers.borrow_mut();
            watchers.retain(|(realm, ..)| *realm != id);
            if queue.as_object().is_some_and(|f| f.is_function()) {
                watchers.push((id, s.realm().downgrade(), Rc::new(Persistent::new(s, queue))));
            }
            !watchers.is_empty()
        };
        // New resource entries reach the observers only while some watch:
        // otherwise adding one is a buffer push.
        let Some(page) = self.page_dom.borrow().upgrade() else { return };
        let listener = watched.then(|| {
            let nav = Rc::downgrade(self);
            Rc::new(move |entry: &Rc<ResourceEntry>| {
                if let Some(nav) = nav.upgrade() {
                    nav.notify_resource(entry);
                }
            }) as pyro_webapi::resource_timing::EntryListener
        });
        page.resource_timings().set_listener(listener);
    }

    /// A `resource` entry was added: every observing realm queues its
    /// object for that entry.
    fn notify_resource(&self, entry: &Rc<ResourceEntry>) {
        let watchers: Vec<_> = {
            let mut watchers = self.timeline_watchers.borrow_mut();
            watchers.retain(|(_, realm, _)| realm.upgrade().is_some_and(|r| !r.is_disposed()));
            watchers.iter().map(|(_, realm, f)| (realm.clone(), f.clone())).collect()
        };
        for (realm, f) in watchers {
            let Some(realm) = realm.upgrade() else { continue };
            let Ok(s) = realm.scope() else { continue };
            let Some(f) = f.get(&s).as_object() else { continue };
            let queued = resource_object(&s, entry.clone()).and_then(|o| f.call(None, &[s.string("resource"), o]));
            if let Err(e) = queued {
                s.report_exception(&e);
            }
        }
    }
}

impl Browser {
    /// Apply the URL already committed by history APIs before a consumer
    /// reads the frame's URL or fallback base. A parser-triggered resource
    /// drain can run before the next event-loop checkpoint.
    pub(crate) fn apply_same_document_url(&mut self, key: crate::frames::FrameKey) {
        let Some(url) = self.doc(key).and_then(|doc| doc.nav.take_pending_frame_url()) else { return };
        if let Some(mut doc) = self.doc_mut(key) {
            // The DOM already committed the URL. An initial blank or srcdoc
            // Document can retain a creator-provided fallback base.
            let frame = &mut doc.main_frame;
            frame.base_url = frame.page_dom.as_ref().and_then(|page| {
                page.dom.borrow().document_fallback_base_url(page.document()).ok().map(str::to_owned)
            }).unwrap_or_else(|| url.clone());
            frame.url = url;
        }
        if key.child.is_none() { self.sync_root(key.page); }
    }

    pub(crate) fn same_document_committed(
        &mut self,
        page: crate::frames::FrameKey,
        url: String,
        kind: SameDocumentKind,
    ) {
        let Some(mut p) = self.doc_mut(page) else { return };
        p.main_frame.url = url.clone();
        let frame_id = p.main_frame.id.clone();
        drop(p);
        self.sync_root(page.page);
        self.emit(page.page, PageEvent::FrameNavigatedWithinDocument { frame_id, url, kind });
    }

    /// Same-document traversal: `popstate` with the entry's state, then a
    /// queued `hashchange` when only the fragment changed (HTML "update
    /// document for history step application").
    fn fire_popstate(&mut self, page: crate::frames::FrameKey, state: bool, old_url: &str, new_url: &str) {
        let Some(p) = self.doc(page) else { return };
        let Some(dom) = p.main_frame.page_dom.clone() else { return };
        // The event's state is the history state object itself (browsers:
        // `event.state === history.state`).
        let nav = p.nav.clone();
        let state = match (state, p.main_frame.main_realm().and_then(|r| r.scope().ok())) {
            (true, Some(scope)) => PageNav::state_value(&nav, &scope).ok().map(|v| pyro_js::Persistent::new(&scope, v)),
            _ => None,
        };
        drop(p);
        pyro_webapi::lifecycle::popstate(&dom, state);
        if let Some(nav) = self.doc(page).map(|p| p.nav.clone()) {
            nav.queue_hashchange(old_url, new_url);
        }
    }

    fn report_navigation_error(&mut self, page: crate::frames::FrameKey, e: &JsError) {
        if let Some(realm) = self.doc(page).and_then(|p| p.main_frame.main_realm().cloned()) {
            if let Ok(scope) = realm.scope() {
                scope.report_exception(e);
            }
        }
    }
}

fn parse(nav: &PageNav, input: Value<'_>) -> JsResult<Url> {
    let href = input.to_string()?;
    let base_url = nav
        .document
        .borrow()
        .as_ref()
        .and_then(|(dom, doc)| dom.borrow().document_base_url(*doc).ok().map(str::to_owned))
        .unwrap_or_else(|| nav.url());
    let base = Url::parse(&base_url).ok();
    Url::options()
        .base_url(base.as_ref())
        .parse(&href)
        .map_err(|_| JsError::dom("SyntaxError", format!("Failed to resolve URL '{href}'")))
}

/// HTML "can have its URL rewritten", as Chrome applies it: scheme,
/// credentials, host and port must match; outside HTTP(S) (`about:`,
/// `data:`, `file:`) the path must match too, while the query and fragment may
/// change. Compared by components, so documents with opaque origins (such as
/// `about:blank`) can still change their own fragment.
fn can_rewrite_url(document: &Url, target: &Url) -> bool {
    if document.scheme() != target.scheme()
        || document.username() != target.username()
        || document.password() != target.password()
        || document.host_str() != target.host_str()
        || document.port() != target.port()
    {
        return false;
    }
    matches!(target.scheme(), "http" | "https") || document.path() == target.path()
}

fn same_document_except_fragment(a: &Url, b: &Url) -> bool {
    let mut a = a.clone();
    let mut b = b.clone();
    a.set_fragment(None);
    b.set_fragment(None);
    a == b
}

/// Location assignment: fragment-only changes are same-document; a `javascript:` URL runs
/// its script in this document.
fn assign(nav: &PageNav, input: Value<'_>, replace: bool) -> JsResult<()> {
    let target = parse(nav, input)?;
    if target.scheme() == "javascript" {
        nav.run_javascript_url(target.as_str());
        return Ok(());
    }
    // HTML "Location-object navigate": a document that has not completely
    // loaded, with no user gesture, replaces its entry.
    let loading = nav.page_dom.borrow().upgrade().is_some_and(|page| page.navigates_by_replacing());
    navigate_to(nav, target, replace || loading);
    Ok(())
}

/// A cross-origin `location.href = url` or `location.replace(url)` on the
/// frame of `nav`: `url` is resolved against `base`, the base URL of the
/// document doing it (HTML: relative to the entry settings object). A
/// `javascript:` URL is ignored, as it would run in the other origin.
pub(crate) fn navigate_cross_origin(nav: &PageNav, base: &str, input: Value<'_>, replace: bool) -> JsResult<()> {
    let href = input.to_string()?;
    let base = Url::parse(base).ok();
    let target = Url::options()
        .base_url(base.as_ref())
        .parse(&href)
        .map_err(|_| JsError::dom("SyntaxError", format!("Failed to resolve URL '{href}'")))?;
    if target.scheme() != "javascript" {
        navigate_to(nav, target, replace);
    }
    Ok(())
}

/// HTML "navigate" from script (Location, `navigation.navigate`): a
/// fragment change is same-document, anything else loads. Returns whether
/// it stayed in the document.
fn navigate_to(nav: &PageNav, mut target: Url, replace: bool) -> bool {
    let current = Url::parse(&nav.url()).ok();
    if current.as_ref().is_some_and(|current| same_document_except_fragment(current, &target)) {
        nav.take_fragment_directive(&mut target);
    }
    let same_url = current.as_ref() == Some(&target);
    let navigation_type = if replace || same_url { "replace" } else { "push" };
    match nav.navigate_event(navigation_type, target.as_str(), true) {
        NavigateDecision::Cancel => return false,
        NavigateDecision::Intercept => {
            nav.commit_intercepted(target.as_str(), navigation_type);
            return true;
        }
        NavigateDecision::Proceed => {}
    }
    match current {
        Some(cur) if target.fragment().is_some() && same_document_except_fragment(&cur, &target) => {
            // HTML "navigate": the document's own URL is a replace.
            let kind = if replace || same_url { SameDocumentKind::Replace } else { SameDocumentKind::Fragment };
            nav.same_document(target.as_str(), None, kind);
            nav.scroll_to_fragment(&target);
            nav.navigation_committed(navigation_type);
            nav.fire_fragment_popstate();
            nav.queue_hashchange(cur.as_str(), target.as_str());
            true
        }
        _ => {
            nav.navigate(target.to_string(), if replace { HistoryHandling::Replace } else { HistoryHandling::Push });
            false
        }
    }
}

fn url_part(nav: &PageNav, f: impl Fn(&Url) -> String) -> String {
    Url::parse(&nav.url()).map(|u| f(&u)).unwrap_or_default()
}

/// A Location setter: `change` edits a copy of the URL and says whether to
/// navigate to it (HTML "Location-object navigate").
fn set_url_part(s: &Scope<'_>, nav: &PageNav, change: impl FnOnce(&mut Url) -> JsResult<bool>) -> JsResult<()> {
    let mut url = Url::parse(&nav.url()).map_err(|_| JsError::Type("invalid current URL".into()))?;
    if change(&mut url)? {
        assign(nav, s.string(url.as_str()), false)?;
    }
    Ok(())
}

pub struct Location;

impl JsClass for Location {
    const NAME: &'static str = "Location";
    type Data = Rc<PageNav>;
    type Parent = NoParent;

    fn define(def: &mut ClassDef<Self>) {
        def.accessor("href", |s, n| Ok(s.string(&n.url())), |_s, n, v| assign(n, v, false));
        def.pure_method("toString", 0, |s, n, _a| Ok(s.string(&n.url())));
        def.method("assign", 1, |s, n, a| {
            a.require(1, "assign")?;
            assign(n, a.get(0), false)?;
            Ok(s.undefined())
        });
        def.method("replace", 1, |s, n, a| {
            a.require(1, "replace")?;
            assign(n, a.get(0), true)?;
            Ok(s.undefined())
        });
        def.method("reload", 0, |s, n, _a| {
            let url = n.url();
            match n.navigate_event("reload", &url, true) {
                NavigateDecision::Cancel => {}
                NavigateDecision::Intercept => n.commit_intercepted(&url, "reload"),
                NavigateDecision::Proceed => n.navigate(url, HistoryHandling::Reload),
            }
            Ok(s.undefined())
        });
        def.getter("origin", |s, n| {
            Ok(s.string(&url_part(n, |u| match u.origin() {
                o @ url::Origin::Tuple(..) => o.ascii_serialization(),
                url::Origin::Opaque(_) => "null".into(),
            })))
        });
        def.accessor(
            "protocol",
            |s, n| Ok(s.string(&url_part(n, |u| format!("{}:", u.scheme())))),
            |s, n, v| {
                let value = v.to_string()?;
                set_url_part(s, n, |u| {
                    // URL "scheme state" with a state override: tabs and
                    // newlines are dropped, everything from the first `:` on
                    // is ignored, and anything else that is no scheme fails.
                    let value: String = value.chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect();
                    let scheme = value.split(':').next().unwrap_or("");
                    let mut chars = scheme.chars();
                    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
                        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
                    if !valid {
                        return Err(JsError::dom("SyntaxError", format!("'{value}' is an invalid protocol.")));
                    }
                    // A switch between special and other schemes leaves the
                    // URL as it was, as the URL parser does.
                    let _ = u.set_scheme(&scheme.to_ascii_lowercase());
                    // Only HTTP(S) navigates.
                    Ok(matches!(u.scheme(), "http" | "https"))
                })
            },
        );
        def.accessor(
            "host",
            |s, n| {
                Ok(s.string(&url_part(n, |u| match (u.host_str(), u.port()) {
                    (Some(h), Some(p)) => format!("{h}:{p}"),
                    (Some(h), None) => h.to_string(),
                    _ => String::new(),
                })))
            },
            |s, n, v| {
                let value = v.to_string()?;
                set_url_part(s, n, |u| Ok(!u.cannot_be_a_base() && url::quirks::set_host(u, &value).is_ok()))
            },
        );
        def.accessor(
            "hostname",
            |s, n| Ok(s.string(&url_part(n, |u| u.host_str().unwrap_or("").to_string()))),
            |s, n, v| {
                let value = v.to_string()?;
                set_url_part(s, n, |u| Ok(!u.cannot_be_a_base() && url::quirks::set_hostname(u, &value).is_ok()))
            },
        );
        def.accessor(
            "port",
            |s, n| Ok(s.string(&url_part(n, |u| u.port().map(|p| p.to_string()).unwrap_or_default()))),
            |s, n, v| {
                let value = v.to_string()?;
                set_url_part(s, n, |u| {
                    // A URL that cannot have a port (no host, `file:`) is left alone.
                    if u.host_str().is_none_or(str::is_empty) || u.scheme() == "file" {
                        return Ok(false);
                    }
                    Ok(url::quirks::set_port(u, &value).is_ok())
                })
            },
        );
        def.accessor(
            "pathname",
            |s, n| Ok(s.string(&url_part(n, |u| u.path().to_string()))),
            |s, n, v| {
                let value = v.to_string()?;
                set_url_part(s, n, |u| {
                    if u.cannot_be_a_base() {
                        return Ok(false);
                    }
                    url::quirks::set_pathname(u, &value);
                    Ok(true)
                })
            },
        );
        def.accessor(
            "search",
            |s, n| {
                Ok(s.string(&url_part(n, |u| match u.query() {
                    Some(q) if !q.is_empty() => format!("?{q}"),
                    _ => String::new(),
                })))
            },
            |s, n, v| {
                let value = v.to_string()?;
                set_url_part(s, n, |u| {
                    url::quirks::set_search(u, &value);
                    Ok(true)
                })
            },
        );
        def.accessor(
            "hash",
            |s, n| {
                Ok(s.string(&url_part(n, |u| match u.fragment() {
                    Some(f) if !f.is_empty() => format!("#{f}"),
                    _ => String::new(),
                })))
            },
            |s, n, v| {
                let mut u = Url::parse(&n.url()).map_err(|_| JsError::Type("invalid current URL".into()))?;
                let h = v.to_string()?;
                let current = u.fragment().unwrap_or("").to_owned();
                u.set_fragment(Some(h.strip_prefix('#').unwrap_or(&h)));
                // The same fragment (no fragment counts as an empty one, as
                // in Chrome) navigates nowhere: no event, no history entry.
                if u.fragment() == Some(current.as_str()) {
                    return Ok(());
                }
                assign(n, s.string(u.as_str()), false)
            },
        );
    }
}

pub struct History;

impl JsClass for History {
    const NAME: &'static str = "History";
    type Data = Rc<PageNav>;
    type Parent = NoParent;

    fn define(def: &mut ClassDef<Self>) {
        // Browsers accept an unbound receiver here; TanStack History calls
        // `(push ? history.pushState : history.replaceState)(state, "", url)`.
        def.global_receiver(|s| s.global_data::<crate::Window>().map(|w| w.nav()));
        def.getter("length", |s, n| Ok(s.number(n.history_length() as f64)));
        def.getter("state", |s, n| PageNav::state_value(n, s));
        def.accessor(
            "scrollRestoration",
            |s, n| Ok(s.string(if n.state.borrow().scroll_restoration_manual { "manual" } else { "auto" })),
            |_s, n, v| {
                match v.to_string()?.as_str() {
                    "manual" => n.state.borrow_mut().scroll_restoration_manual = true,
                    "auto" => n.state.borrow_mut().scroll_restoration_manual = false,
                    _ => {} // invalid enum values are ignored
                }
                Ok(())
            },
        );
        def.method("pushState", 2, |s, n, a| change_state(s, n, a, SameDocumentKind::Push));
        def.method("replaceState", 2, |s, n, a| change_state(s, n, a, SameDocumentKind::Replace));
        def.method("back", 0, |s, n, _a| {
            n.traverse(-1);
            Ok(s.undefined())
        });
        def.method("forward", 0, |s, n, _a| {
            n.traverse(1);
            Ok(s.undefined())
        });
        def.method("go", 0, |s, n, a| {
            let d = a.get(0).to_number()?;
            n.traverse(if d.is_finite() { d.trunc() as i64 } else { 0 });
            Ok(s.undefined())
        });
    }
}

fn change_state<'s>(s: &Scope<'s>, n: &PageNav, a: pyro_js::Args<'s>, kind: SameDocumentKind) -> JsResult<Value<'s>> {
    a.require(2, if kind == SameDocumentKind::Push { "pushState" } else { "replaceState" })?;
    // StructuredSerializeForStorage: functions, DOM nodes and ports throw
    // DataCloneError; Date, Map, Set, typed arrays, cycles, Blob, ... survive.
    let state = match a.get(0) {
        v if v.is_nullish() => None,
        v => Some(HistoryState(Rc::new(pyro_webapi::serialize_message(s, v, s.undefined())?))),
    };
    let url = match a.opt(2).filter(|v| !v.is_nullish()) {
        Some(v) => {
            let document = n.url();
            let current = Url::parse(&document).ok();
            let rejected = |target: &str| {
                let origin = current.as_ref().map_or("null".into(), |c| c.origin().ascii_serialization());
                JsError::dom(
                    "SecurityError",
                    format!(
                        "A history state object with URL '{target}' cannot be created in a document with origin '{origin}' and URL '{document}'"
                    ),
                )
            };
            // HTML: a URL that fails to parse is a SecurityError here.
            let target = parse(n, v).map_err(|_| rejected(&v.to_string().unwrap_or_default()))?;
            if !current.as_ref().is_some_and(|c| can_rewrite_url(c, &target)) {
                return Err(rejected(target.as_str()));
            }
            target.to_string()
        }
        None => n.url(),
    };
    let navigation_type = if kind == SameDocumentKind::Push { "push" } else { "replace" };
    if n.navigate_event(navigation_type, &url, true) == NavigateDecision::Cancel {
        return Ok(s.undefined());
    }
    n.same_document(&url, state, kind);
    n.navigation_committed(navigation_type);
    Ok(s.undefined())
}

pub struct PerformanceData {
    nav: Rc<PageNav>,
    /// Wall-clock time of the time origin (ms since the Unix epoch).
    origin_wall_ms: f64,
    /// `Performance` is an EventTarget (`resourcetimingbufferfull`).
    target: pyro_webapi::EventTargetData,
    /// User Timing: the marks and measures of the performance entry buffer.
    user: UserTiming,
}

impl PerformanceData {
    pub(crate) fn new(nav: Rc<PageNav>, page: &Rc<pyro_webapi::PageDom>) -> PerformanceData {
        let now_wall = crate::clock::wall_ms();
        let origin_wall_ms = now_wall - nav.now_ms();
        let target = pyro_webapi::EventTargetData::new(page);
        page.resource_timings().add_target(target.key);
        PerformanceData { nav, origin_wall_ms, target, user: UserTiming::default() }
    }
    /// The identity its wrapper is made with (event dispatch looks it up).
    pub(crate) fn wrapper_identity(&self) -> Option<pyro_js::IdentityKey> {
        self.target.wrapper_identity()
    }
    pub(crate) fn event_target(&self) -> &pyro_webapi::EventTargetData {
        &self.target
    }
}

impl Drop for PerformanceData {
    fn drop(&mut self) {
        self.target.page.resource_timings().remove_target(self.target.key);
    }
}

/// The timeline entries matching the filters, sorted by start time: the
/// User Timing marks and measures, and `first-paint` /
/// `first-contentful-paint` once the document had its first rendering
/// opportunity (`Browser::note_first_paint`). Nothing is rasterized, so both
/// paint entries carry the time of that opportunity.
fn timeline_entries<'s>(
    s: &Scope<'s>,
    p: &PerformanceData,
    name: Option<&str>,
    entry_type: Option<&str>,
) -> JsResult<Value<'s>> {
    let mut entries: Vec<(f64, Value<'s>)> = Vec::new();
    if entry_type.is_none_or(|t| t == "navigation") {
        if let Some(entry) = navigation_entry(p) {
            if name.is_none_or(|n| n == entry.name) {
                entries.push((0.0, s.wrap::<PerformanceNavigationTiming>(entry, None)?.value()));
            }
        }
    }
    if let Some(start) = p.nav.first_paint().filter(|_| entry_type.is_none_or(|t| t == "paint")) {
        for entry_name in ["first-paint", "first-contentful-paint"] {
            if name.is_none_or(|n| n == entry_name) {
                let data = PerformanceEntryData::new(entry_name, "paint", start, 0.0);
                entries.push((start, s.wrap::<PerformancePaintTiming>(data, None)?.value()));
            }
        }
    }
    p.user.collect(s, name, entry_type, &mut entries);
    if entry_type.is_none_or(|t| t == "resource") {
        perf::collect_resources(s, &p.target.page, name, &mut entries)?;
    }
    perf::sorted_entries(s, entries)
}

fn navigation_values(page: &pyro_webapi::PageDom, nav: &PageNav) -> NavigationValues {
    let t = page.timing();
    NavigationValues {
        dom_interactive: t.dom_interactive.unwrap_or(0.0),
        dom_content_loaded_start: t.dom_content_loaded_start.unwrap_or(0.0),
        dom_content_loaded_end: t.dom_content_loaded_end.unwrap_or(0.0),
        dom_complete: t.dom_complete.unwrap_or(0.0),
        load_start: t.load_start.unwrap_or(0.0),
        load_end: t.load_end.unwrap_or(0.0),
        nav_type: nav.nav_type.get(),
        fetch: nav.fetch_timing.borrow().clone(),
    }
}

/// The document's `navigation` entry. Chrome adds it to the timeline as the
/// document starts loading and fills it in as it goes; an `about:` document
/// (the initial one, `setContent`) has none.
fn navigation_entry(p: &PerformanceData) -> Option<PerformanceEntryData> {
    let url = p.nav.url();
    if !(url.starts_with("http:") || url.starts_with("https:") || url.starts_with("file:")) {
        return None;
    }
    let mut values = navigation_values(&p.target.page, &p.nav);
    if let Some(fetch) = &mut values.fetch {
        if !fetch.server_timing.is_empty() && !p.target.page.exposes_server_timing() {
            Rc::make_mut(fetch).server_timing.clear();
        }
    }
    let mut entry = PerformanceEntryData::new(&url, "navigation", 0.0, values.load_end);
    entry.navigation = Some(Box::new(values));
    Some(entry)
}

/// User Timing "convert a name to a timestamp" in a Window: a
/// PerformanceTiming attribute is its time relative to navigationStart, an
/// error while it is 0 (the step has not run).
fn timing_attribute_timestamp(p: &PerformanceData, name: &str) -> Option<JsResult<f64>> {
    if name == "navigationStart" {
        return Some(Ok(0.0));
    }
    if !TIMING_ATTRIBUTES.contains(&name) {
        return None;
    }
    let n = navigation_values(&p.target.page, &p.nav);
    if legacy_timing(p.origin_wall_ms, &n, name) == 0.0 {
        return Some(Err(JsError::dom(
            "InvalidAccessError",
            format!("'{name}' is empty: either the event hasn't happened yet, or it would provide cross-origin timing information."),
        )));
    }
    // Relative to navigationStart, the time origin.
    let fetch_phase = if name == "domLoading" { "responseStart" } else { name };
    if let Some((_, value)) = RESOURCE_TIMES.iter().find(|(key, _)| *key == fetch_phase) {
        return Some(Ok(n.fetch.as_deref().map_or(0.0, value)));
    }
    Some(Ok(navigation_fields(&n).into_iter().find(|(key, _)| *key == name).map_or(0.0, |(_, t)| t)))
}

/// The legacy `performance.timing`: the document's Navigation Timing in
/// milliseconds since the epoch, 0 for a step that has not run (and for the
/// unload, redirect and secure-connection steps, which never do here).
pub struct PerformanceTiming;

pub struct TimingData {
    origin_wall_ms: f64,
    page: Rc<pyro_webapi::PageDom>,
    nav: Rc<PageNav>,
}

/// `performance.timing[name]` (epoch milliseconds, 0 when not reached). A
/// document without a fetch reports its fetch phases at `navigationStart`.
fn legacy_timing(origin: f64, n: &NavigationValues, name: &str) -> f64 {
    let at = |t: f64| if t > 0.0 { (origin + t).round() } else { 0.0 };
    // A phase of the fetch: at or after the time origin, so never 0 itself.
    let phase = |f: fn(&ResourceEntry) -> f64| (origin + n.fetch.as_deref().map_or(0.0, f)).round();
    match name {
        "navigationStart" => origin.round(),
        "fetchStart" => phase(|f| f.fetch_start),
        "domainLookupStart" => phase(|f| f.domain_lookup_start),
        "domainLookupEnd" => phase(|f| f.domain_lookup_end),
        "connectStart" => phase(|f| f.connect_start),
        "connectEnd" => phase(|f| f.connect_end),
        "requestStart" => phase(|f| f.request_start),
        "responseStart" | "domLoading" => phase(|f| f.response_start),
        "responseEnd" => phase(|f| f.response_end),
        "secureConnectionStart" => n.fetch.as_ref().map_or(0.0, |f| at(f.secure_connection_start)),
        "redirectStart" => n.fetch.as_ref().filter(|f| f.redirect_count > 0).map_or(0.0, |_| origin.round()),
        "redirectEnd" => n.fetch.as_ref().map_or(0.0, |f| at(f.redirect_end)),
        "domInteractive" => at(n.dom_interactive),
        "domContentLoadedEventStart" => at(n.dom_content_loaded_start),
        "domContentLoadedEventEnd" => at(n.dom_content_loaded_end),
        "domComplete" => at(n.dom_complete),
        "loadEventStart" => at(n.load_start),
        "loadEventEnd" => at(n.load_end),
        _ => 0.0,
    }
}

impl JsClass for PerformanceTiming {
    const NAME: &'static str = "PerformanceTiming";
    type Data = TimingData;
    type Parent = NoParent;

    fn define(def: &mut ClassDef<Self>) {
        macro_rules! attributes {
            ($($i:literal),*) => {$(
                def.getter(TIMING_ATTRIBUTES[$i], |s, t| {
                    let n = navigation_values(&t.page, &t.nav);
                    Ok(s.number(legacy_timing(t.origin_wall_ms, &n, TIMING_ATTRIBUTES[$i])))
                });
            )*};
        }
        attributes!(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20);
        def.method("toJSON", 0, |s, t, _a| {
            let o = s.object();
            let n = navigation_values(&t.page, &t.nav);
            for name in TIMING_ATTRIBUTES {
                o.set(name, s.number(legacy_timing(t.origin_wall_ms, &n, name)))?;
            }
            Ok(o.value())
        });
    }
}

/// The legacy `performance.navigation`.
pub struct PerformanceNavigation;

impl JsClass for PerformanceNavigation {
    const NAME: &'static str = "PerformanceNavigation";
    type Data = Rc<PageNav>;
    type Parent = NoParent;

    fn define(def: &mut ClassDef<Self>) {
        def.getter("type", |s, n| Ok(s.number(legacy_navigation_type(n))));
        def.getter("redirectCount", |s, _n| Ok(s.number(0.0)));
        def.method("toJSON", 0, |s, n, _a| {
            let o = s.object();
            o.set("type", s.number(legacy_navigation_type(n)))?;
            o.set("redirectCount", s.number(0.0))?;
            Ok(o.value())
        });
    }
}

fn legacy_navigation_type(n: &PageNav) -> f64 {
    match n.nav_type.get() {
        "reload" => 1.0,
        "back_forward" => 2.0,
        _ => 0.0,
    }
}

pub struct Performance;

impl JsClass for Performance {
    const NAME: &'static str = "Performance";
    type Data = PerformanceData;
    type Parent = pyro_webapi::EventTarget;

    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|p: &PerformanceData| &p.target);
        def.pure_method("now", 0, |s, p, _a| Ok(s.number(p.nav.now_ms())));
        def.getter("timeOrigin", |s, p| Ok(s.number(p.origin_wall_ms)));
        def.method("mark", 1, |s, p, a| {
            let object = p.user.mark(s, &a, p.nav.now_ms())?;
            p.nav.notify_entry(s, "mark", object)?;
            Ok(object)
        });
        def.method("measure", 1, |s, p, a| {
            let object = p.user.measure(s, &a, p.nav.now_ms(), &|name| timing_attribute_timestamp(p, name))?;
            p.nav.notify_entry(s, "measure", object)?;
            Ok(object)
        });
        def.method("clearMarks", 0, |s, p, a| p.user.clear(true, &a).map(|()| s.undefined()));
        def.method("clearMeasures", 0, |s, p, a| p.user.clear(false, &a).map(|()| s.undefined()));
        def.pure_method("getEntries", 0, |s, p, _a| timeline_entries(s, p, None, None));
        def.pure_method("getEntriesByName", 1, |s, p, a| {
            a.require(1, "Performance.getEntriesByName")?;
            let name = a.get(0).to_string()?;
            let ty = a.opt(1).filter(|v| !v.is_undefined()).map(|v| v.to_string()).transpose()?;
            timeline_entries(s, p, Some(&name), ty.as_deref())
        });
        def.pure_method("getEntriesByType", 1, |s, p, a| {
            a.require(1, "Performance.getEntriesByType")?;
            let ty = a.get(0).to_string()?;
            timeline_entries(s, p, None, Some(&ty))
        });
        def.method("setResourceTimingBufferSize", 1, |s, p, a| {
            perf::set_buffer_size(&p.target.page, &a).map(|()| s.undefined())
        });
        def.method("clearResourceTimings", 0, |s, p, _a| {
            perf::clear_resources(&p.target.page);
            Ok(s.undefined())
        });
        def.accessor(
            "onresourcetimingbufferfull",
            |s, p| pyro_webapi::resource_timing::buffer_full_handler(s, &p.target),
            |s, p, v| pyro_webapi::resource_timing::set_buffer_full_handler(s, &p.target, v),
        );
        def.getter("timing", timing_object);
        def.getter("navigation", navigation_object);
        def.method("toJSON", 0, |s, p, _a| {
            let o = s.object();
            o.set("timeOrigin", s.number(p.origin_wall_ms))?;
            o.set("timing", timing_object(s, p)?)?;
            o.set("navigation", navigation_object(s, p)?)?;
            Ok(o.value())
        });
    }
}

/// `performance.timing`: one object per realm.
fn timing_object<'s>(s: &Scope<'s>, p: &PerformanceData) -> JsResult<Value<'s>> {
    let key = pyro_js::IdentityKey(KEY_TIMING, 0);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o.value());
    }
    let data = TimingData { origin_wall_ms: p.origin_wall_ms, page: p.target.page.clone(), nav: p.nav.clone() };
    Ok(s.wrap::<PerformanceTiming>(data, Some(key))?.value())
}

/// `performance.navigation`: one object per realm.
fn navigation_object<'s>(s: &Scope<'s>, p: &PerformanceData) -> JsResult<Value<'s>> {
    Ok(s.wrap::<PerformanceNavigation>(p.nav.clone(), Some(pyro_js::IdentityKey(KEY_NAVIGATION, 0)))?.value())
}

/// Identity keys of `performance.timing` and `performance.navigation` (one
/// per realm).
const KEY_TIMING: u64 = 0x5065_7254_696d_6e00;
const KEY_NAVIGATION: u64 = 0x5065_724e_6176_6900;

/// Entry types a Window's observers take.
const WINDOW_ENTRY_TYPES: &[&str] = &["mark", "measure", "navigation", "paint", "resource"];

/// This realm's performance_observer.js module (`PerformanceObserver`,
/// `PerformanceObserverEntryList`), made on first use and kept on the
/// realm's `performance` object.
pub(crate) fn observer_module<'s>(
    s: &Scope<'s>,
    performance: pyro_js::Object<'s>,
    nav: &Rc<PageNav>,
) -> JsResult<pyro_js::Object<'s>> {
    let page = s
        .unwrap::<Performance>(performance.value())
        .map(|p| p.target.page.clone())
        .ok_or_else(|| JsError::Type("PerformanceObserver is unavailable".into()))?;
    let watched = Rc::downgrade(nav);
    let watch = move |s: &Scope<'_>, queue: Value<'_>| {
        if let Some(nav) = watched.upgrade() {
            nav.watch_timeline(s, queue);
        }
    };
    perf::observer_module(s, performance, &page, watch, WINDOW_ENTRY_TYPES)
}

/// The realm's Navigation API module (navigation.js): `navigation` and
/// its interfaces, made on the realm's first use and kept on the realm's
/// History object (one per realm).
pub(crate) fn navigation_module<'s>(s: &Scope<'s>, nav: &Rc<PageNav>) -> JsResult<pyro_js::Object<'s>> {
    const KEY: &str = "lpNavigation";
    let history = s.wrap::<History>(nav.clone(), Some(pyro_js::IdentityKey(crate::window::KEY_HISTORY, 0)))?;
    if let Some(module) = s.get_hidden(history, KEY)?.and_then(|v| v.as_object()) {
        return Ok(module);
    }
    let unavailable = || JsError::Type("navigation is unavailable".into());
    let natives = s.object();
    let weak = Rc::downgrade(nav);
    let with_nav = move || weak.upgrade().ok_or(JsError::ContextDestroyed);
    macro_rules! native {
        ($name:literal, |$s:ident, $nav:ident, $a:ident| $body:expr) => {{
            let with_nav = with_nav.clone();
            let f = s.function($name, move |$s, _this, $a| {
                let $nav = with_nav()?;
                $body
            })?;
            natives.set_atom($name, f.value())?;
        }};
    }
    // watch(hook | null): hook(kind, ...) fires the realm's events.
    native!("watch", |s, nav, a| {
        let id = s.realm_id();
        let mut hooks = nav.navigation_hooks.borrow_mut();
        hooks.retain(|(realm, ..)| *realm != id);
        if a.get(0).is_function() {
            hooks.push((id, s.realm().downgrade(), Rc::new(Persistent::new(s, a.get(0)))));
        }
        Ok(s.undefined())
    });
    // entries() -> [index, [key, id, url, sameDocument, stateJSON]...]: the
    // same-origin entries around the current one.
    native!("entries", |s, nav, _a| {
        let st = nav.state.borrow();
        let origin = |url: &str| Url::parse(url).ok().map(|u| u.origin());
        let Some(current) = st.entries.get(st.index) else { return Ok(s.array(&[s.number(-1.0)])?.value()) };
        let here = origin(&current.url);
        let same =
            |e: &Entry| (here.as_ref().is_some_and(|o| o.is_tuple()) && origin(&e.url) == here) || e.doc == current.doc;
        let mut start = st.index;
        while start > 0 && same(&st.entries[start - 1]) {
            start -= 1;
        }
        let mut end = st.index + 1;
        while end < st.entries.len() && same(&st.entries[end]) {
            end += 1;
        }
        let mut values = vec![s.number((st.index - start) as f64)];
        for e in &st.entries[start..end] {
            let state = e.nav_state.as_deref().map_or(s.undefined(), |j| s.string(j));
            let row = [
                s.string(&e.key.to_string()),
                s.string(&e.id.to_string()),
                s.string(&e.url),
                s.boolean(e.doc == current.doc),
                state,
            ];
            values.push(s.array(&row)?.value());
        }
        Ok(s.array(&values)?.value())
    });
    // active() -> whether the document is fully active (its frame not
    // removed, not navigated away): the methods reject InvalidStateError
    // otherwise.
    {
        let with_nav = with_nav.clone();
        let f = s.function("active", move |s, _this, _a| {
            let active =
                with_nav().ok().is_some_and(|nav| nav.page_dom.borrow().upgrade().is_some_and(|p| p.is_fully_active()));
            Ok(s.boolean(active))
        })?;
        natives.set_atom("active", f.value())?;
    }
    // resolve(url) -> the absolute URL, or SyntaxError.
    native!("resolve", |s, nav, a| Ok(s.string(parse(&nav, a.get(0))?.as_str())));
    // navigate(url, replace, stateJSON?) -> whether it stayed in the
    // document. Its `navigate` event was fired by the caller.
    native!("navigate", |s, nav, a| {
        let target = Url::parse(&a.get(0).to_string()?).map_err(|_| JsError::dom("SyntaxError", "Invalid URL"))?;
        let replace = a.get(1).to_bool();
        let state = a.opt(2).filter(|v| !v.is_undefined()).map(|v| v.to_string()).transpose()?;
        let outer = nav.navigation_api_call.replace(true);
        let same = navigate_to(&nav, target, replace);
        nav.navigation_api_call.set(outer);
        if same {
            nav.set_navigation_state(state);
        }
        Ok(s.boolean(same))
    });
    // commit(url, type, stateJSON?): an intercepted navigation's
    // same-document commit.
    native!("commit", |s, nav, a| {
        let url = a.get(0).to_string()?;
        let navigation_type = match a.get(1).to_string()?.as_str() {
            "replace" => "replace",
            "reload" => "reload",
            _ => "push",
        };
        let state = a.opt(2).filter(|v| !v.is_undefined()).map(|v| v.to_string()).transpose()?;
        let outer = nav.navigation_api_call.replace(true);
        nav.commit_intercepted(&url, navigation_type);
        nav.navigation_api_call.set(outer);
        if navigation_type != "reload" || state.is_some() {
            nav.set_navigation_state(state);
        }
        Ok(s.undefined())
    });
    native!("reload", |s, nav, _a| {
        nav.navigate(nav.url(), HistoryHandling::Reload);
        Ok(s.undefined())
    });
    // traverse(key) -> 'missing', 'current' or 'queued' (its events come
    // with the traversal task).
    native!("traverse", |s, nav, a| {
        let key = a.get(0).to_string()?;
        let delta = {
            let st = nav.state.borrow();
            st.entries.iter().position(|e| e.key.to_string() == key).map(|i| i as i64 - st.index as i64)
        };
        Ok(s.string(match delta {
            None => "missing",
            Some(0) => "current",
            Some(delta) => {
                nav.traverse(delta);
                "queued"
            }
        }))
    });
    native!("updateState", |s, nav, a| {
        let state = a.opt(0).filter(|v| !v.is_undefined()).map(|v| v.to_string()).transpose()?;
        nav.set_navigation_state(state);
        Ok(s.undefined())
    });
    let report = s.function("reportException", |s, _this, a| {
        s.report_exception(&JsError::thrown(Persistent::new(s, a.get(0))));
        Ok(s.undefined())
    })?;
    natives.set_atom("reportException", report.value())?;
    let root = Persistent::new(s, natives.value());
    let make = s.eval(include_str!("navigation.js"), Some("pyro://internal/navigation.js"), 1)?;
    let module = make.as_object().ok_or_else(unavailable)?.call(None, &[root.get(s)])?;
    s.set_hidden(history, KEY, module)?;
    module.as_object().ok_or_else(unavailable)
}
