//! Session history, `Location`, `History` and `Performance`.
//!
//! [`PageNav`] is the per-page source of truth for the current URL and the
//! joint session history. JS-facing classes read it synchronously; anything
//! that needs `&mut Browser` (cross-document navigation, events) is posted as
//! a task, as navigation is asynchronous in browsers too.
//!
//! History state is a structured serialization (`lp_webapi::WireMessage`),
//! deserialized once per activation in each realm that reads it. Known gap
//! (see docs/PARITY.md): traversal across documents re-fetches (no bfcache).
//! As in browsers, the first navigation replaces the initial about:blank
//! entry.

use crate::{
    Browser, FrameId, HistoryHandling, NavigateOpts, NavigationReason, ObserverCell, PageEvent, PageId,
    SameDocumentKind,
};
use lp_dom::parse::DomRef;
use lp_dom::NodeId;
use lp_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Persistent, Scope, Value};
use lp_runtime::{Scheduler, TaskSource};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;
use url::Url;

/// A history entry's serialized state (StructuredSerializeForStorage).
/// Entries compare by identity of their state, never by its content.
#[derive(Clone)]
struct HistoryState(Rc<lp_webapi::WireMessage>);

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
    sched: Scheduler<Browser>,
    state: RefCell<NavState>,
    /// Current document in the page's DOM (keeps `document.URL` in sync).
    document: RefCell<Option<(DomRef, NodeId)>>,
    /// The current document's bindings: the target of the `popstate` a
    /// fragment navigation fires synchronously.
    page_dom: RefCell<std::rc::Weak<lp_webapi::PageDom>>,
    time_origin: Cell<Instant>,
    /// When the current document first had a rendering opportunity
    /// (`now_ms`): its Paint Timing entries.
    first_paint: Cell<Option<f64>>,
    /// `PerformanceNavigationTiming.type` of the current document.
    nav_type: Cell<&'static str>,
    /// The realms with a PerformanceObserver (performance_observer.js) and
    /// the function each queues new timeline entries with. Empty unless a
    /// page observes, which is all the timeline costs otherwise.
    timeline_watchers: RefCell<Vec<(lp_js::RealmId, lp_js::WeakRealm, Rc<Persistent>)>>,
    /// The realms with a `navigation` object (navigation.js) and the hook
    /// each fires the Navigation API's events with. Empty unless a page
    /// reads `navigation`, which is all navigations cost otherwise.
    navigation_hooks: RefCell<Vec<(lp_js::RealmId, lp_js::WeakRealm, Rc<Persistent>)>>,
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
            sched,
            state: RefCell::new(NavState {
                url: "about:blank".into(),
                entries: Vec::new(),
                index: 0,
                doc_seq: 0,
                entry_seq: 0,
                state_version: 0,
                scroll_restoration_manual: false,
                initial: false,
                first_blank: false,
                stops: 0,
            }),
            document: RefCell::new(None),
            page_dom: RefCell::new(std::rc::Weak::new()),
            time_origin: Cell::new(crate::clock::now()),
            first_paint: Cell::new(None),
            nav_type: Cell::new("navigate"),
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
        self.pending_frame_url.borrow_mut().take();
        let mut st = self.state.borrow_mut();
        st.doc_seq += 1;
        let id = st.next_entry_id();
        // A push to the current entry's URL replaces it (HTML navigate step:
        // "url equals the active document's URL" -> replace).
        let same_url = st.entries.get(st.index).is_some_and(|e| e.url == url);
        let replaces = matches!(history, HistoryHandling::Replace) || same_url;
        let key = match st.entries.get(st.index) {
            Some(current) if replaces => current.key,
            _ => st.next_entry_id(),
        };
        let entry = Entry { url: url.to_string(), state: None, doc: st.doc_seq, id, key, nav_state: None };
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
                e.url = url.to_string();
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
        self.time_origin.set(crate::clock::now());
        self.first_paint.set(None);
        self.nav_type.set(match history {
            HistoryHandling::Reload => "reload",
            HistoryHandling::Traverse => "back_forward",
            _ => "navigate",
        });
    }

    pub(crate) fn set_document(&self, document: Option<(DomRef, NodeId)>) {
        *self.document.borrow_mut() = document;
    }

    /// The bindings of the document [`PageNav::commit_document`] recorded.
    pub(crate) fn set_page_dom(&self, page: &Rc<lp_webapi::PageDom>) {
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
                        lp_webapi::lifecycle::popstate(&page, None);
                    }
                });
                return;
            }
        }
        lp_webapi::lifecycle::popstate(&page, None);
    }

    fn set_url_sync(&self, url: &str) {
        self.state.borrow_mut().url = url.to_string();
        if let Some((dom, doc)) = self.document.borrow().as_ref() {
            let _ = dom.borrow_mut().set_document_url(*doc, url);
        }
    }

    /// Same-document navigation (pushState/replaceState/fragment).
    fn same_document(&self, url: &str, state: Option<HistoryState>, kind: SameDocumentKind) {
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

    fn navigate(&self, url: String, history: HistoryHandling) {
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
                st.doc_seq == document && st.stops == stops
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
            if nav.state.borrow().doc_seq == current_doc && nav.state.borrow().entries.get(index) == Some(&entry) {
                // Only a same-document traversal can be canceled or intercepted.
                let same = entry.doc == current_doc;
                if nav.navigate_event_to("traverse", &entry.url, same, Some(entry.key)) == NavigateDecision::Cancel
                    && same
                {
                    return;
                }
            }
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
    pub(crate) fn navigate_to_fragment(&self, url: &str) -> bool {
        let (Ok(current), Ok(target)) = (Url::parse(&self.url()), Url::parse(url)) else { return false };
        if target.fragment().is_none() || !same_document_except_fragment(&current, &target) {
            return false;
        }
        let kind = if current == target { SameDocumentKind::Replace } else { SameDocumentKind::Fragment };
        let navigation_type = if kind == SameDocumentKind::Replace { "replace" } else { "push" };
        match self.navigate_event(navigation_type, target.as_str(), true) {
            NavigateDecision::Cancel => return true,
            NavigateDecision::Intercept => {
                self.commit_intercepted(target.as_str(), navigation_type);
                return true;
            }
            NavigateDecision::Proceed => {}
        }
        self.same_document(target.as_str(), None, kind);
        self.navigation_committed(navigation_type);
        self.fire_fragment_popstate();
        self.queue_hashchange(current.as_str(), target.as_str());
        true
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
                lp_webapi::lifecycle::hashchange(&dom, &old_url, &new_url);
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
        let history = s.wrap::<History>(nav.clone(), Some(lp_js::IdentityKey(crate::window::KEY_HISTORY, 0)))?;
        if let (Some(v), Some(cached)) = (s.get_hidden(history, VERSION)?, s.get_hidden(history, VALUE)?) {
            if v.to_number().ok() == Some(version) {
                return Ok(cached);
            }
        }
        let value = match nav.active_state() {
            // A state that cannot be deserialized here reads as null.
            Some(state) => lp_webapi::deserialize_message(s, &state.0).unwrap_or_else(|_| s.null()),
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
        let ms = crate::clock::now().saturating_duration_since(self.time_origin.get()).as_secs_f64() * 1000.0;
        (ms * 10.0).floor() / 10.0
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
    fn navigation_hooks(&self) -> Vec<(lp_js::WeakRealm, Rc<Persistent>)> {
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
        self.navigate_event_to(navigation_type, url, cancelable, None)
    }

    /// [`PageNav::navigate_event`] for a traversal to the entry with `key`.
    fn navigate_event_to(
        &self,
        navigation_type: &'static str,
        url: &str,
        cancelable: bool,
        key: Option<u64>,
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
            let args = [
                s.string("navigate"),
                s.string(navigation_type),
                s.string(url),
                s.boolean(hash_change),
                s.boolean(cancelable),
                s.boolean(can_intercept),
                s.string(&key.map(|k| k.to_string()).unwrap_or_default()),
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
    fn watch_timeline(&self, s: &Scope<'_>, queue: Value<'_>) {
        let id = s.realm_id();
        let mut watchers = self.timeline_watchers.borrow_mut();
        watchers.retain(|(realm, ..)| *realm != id);
        if queue.as_object().is_some_and(|f| f.is_function()) {
            watchers.push((id, s.realm().downgrade(), Rc::new(Persistent::new(s, queue))));
        }
    }
}

impl Browser {
    /// Copies a synchronous same-document commit's URL to the frame.
    pub(crate) fn apply_same_document_url(&mut self, page: PageId) {
        // Runs at every checkpoint: take through the shared handle first so a
        // page with nothing pending is not marked dirty (`page_mut`).
        let Some(url) = self.page(page).and_then(|p| p.nav.take_pending_frame_url()) else { return };
        let Some(p) = self.page_mut(page) else { return };
        if !matches!(p.main_frame.url.as_str(), "about:blank" | "about:srcdoc") {
            p.main_frame.base_url = url.clone();
        }
        p.main_frame.url = url;
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
            (true, Some(scope)) => PageNav::state_value(&nav, &scope).ok().map(|v| lp_js::Persistent::new(&scope, v)),
            _ => None,
        };
        drop(p);
        lp_webapi::lifecycle::popstate(&dom, state);
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

/// Location assignment: fragment-only changes are same-document.
fn assign(nav: &PageNav, input: Value<'_>, replace: bool) -> JsResult<()> {
    navigate_to(nav, parse(nav, input)?, replace);
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
fn navigate_to(nav: &PageNav, target: Url, replace: bool) -> bool {
    let current = Url::parse(&nav.url()).ok();
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
        def.method("toString", 0, |s, n, _a| Ok(s.string(&n.url())));
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

fn change_state<'s>(s: &Scope<'s>, n: &PageNav, a: lp_js::Args<'s>, kind: SameDocumentKind) -> JsResult<Value<'s>> {
    a.require(2, if kind == SameDocumentKind::Push { "pushState" } else { "replaceState" })?;
    // StructuredSerializeForStorage: functions, DOM nodes and ports throw
    // DataCloneError; Date, Map, Set, typed arrays, cycles, Blob, ... survive.
    let state = match a.get(0) {
        v if v.is_nullish() => None,
        v => Some(HistoryState(Rc::new(lp_webapi::serialize_message(s, v, s.undefined())?))),
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
    target: lp_webapi::EventTargetData,
    /// User Timing: the marks and measures of the performance entry buffer,
    /// in the order they were added.
    user_entries: RefCell<Vec<UserEntry>>,
}

/// A `PerformanceMark` or `PerformanceMeasure` in the entry buffer.
struct UserEntry {
    name: String,
    mark: bool,
    start: f64,
    object: Persistent,
}

impl PerformanceData {
    pub(crate) fn new(nav: Rc<PageNav>, page: &Rc<lp_webapi::PageDom>) -> PerformanceData {
        let now_wall = crate::clock::wall_ms();
        let origin_wall_ms = now_wall - nav.now_ms();
        PerformanceData {
            nav,
            origin_wall_ms,
            target: lp_webapi::EventTargetData::new(page),
            user_entries: RefCell::new(Vec::new()),
        }
    }
    /// The identity its wrapper is made with (event dispatch looks it up).
    pub(crate) fn wrapper_identity(&self) -> Option<lp_js::IdentityKey> {
        self.target.wrapper_identity()
    }
    pub(crate) fn event_target(&self) -> &lp_webapi::EventTargetData {
        &self.target
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
    for entry in p.user_entries.borrow().iter() {
        let ty = if entry.mark { "mark" } else { "measure" };
        if name.is_none_or(|n| n == entry.name) && entry_type.is_none_or(|t| t == ty) {
            entries.push((entry.start, entry.object.get(s)));
        }
    }
    // Stable: entries with equal start times keep the order they were added.
    entries.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let values: Vec<Value<'s>> = entries.into_iter().map(|(_, v)| v).collect();
    Ok(s.array(&values)?.value())
}

pub struct PerformanceEntryData {
    name: String,
    entry_type: &'static str,
    start: f64,
    duration: f64,
    /// `detail` of a mark or measure (a structured clone), null when absent.
    detail: Option<Persistent>,
    /// The document's Navigation Timing (`navigation` entries only).
    navigation: Option<Box<NavigationValues>>,
}

impl PerformanceEntryData {
    fn new(name: &str, entry_type: &'static str, start: f64, duration: f64) -> PerformanceEntryData {
        PerformanceEntryData { name: name.to_owned(), entry_type, start, duration, detail: None, navigation: None }
    }
}

/// A document's Navigation Timing as its `navigation` entry reports it, in
/// milliseconds since the time origin; 0 for a step that has not run. The
/// time origin is when the document's response committed, so the fetch
/// phases (fetchStart .. responseEnd) and unloading come out as 0.
#[derive(Clone, Copy)]
struct NavigationValues {
    dom_interactive: f64,
    dom_content_loaded_start: f64,
    dom_content_loaded_end: f64,
    dom_complete: f64,
    load_start: f64,
    load_end: f64,
    nav_type: &'static str,
}

impl NavigationValues {
    const EMPTY: NavigationValues = NavigationValues {
        dom_interactive: 0.0,
        dom_content_loaded_start: 0.0,
        dom_content_loaded_end: 0.0,
        dom_complete: 0.0,
        load_start: 0.0,
        load_end: 0.0,
        nav_type: "navigate",
    };

    fn of(page: &lp_webapi::PageDom, nav: &PageNav) -> NavigationValues {
        let t = page.timing();
        NavigationValues {
            dom_interactive: t.dom_interactive.unwrap_or(0.0),
            dom_content_loaded_start: t.dom_content_loaded_start.unwrap_or(0.0),
            dom_content_loaded_end: t.dom_content_loaded_end.unwrap_or(0.0),
            dom_complete: t.dom_complete.unwrap_or(0.0),
            load_start: t.load_start.unwrap_or(0.0),
            load_end: t.load_end.unwrap_or(0.0),
            nav_type: nav.nav_type.get(),
        }
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
    let values = NavigationValues::of(&p.target.page, &p.nav);
    let mut entry = PerformanceEntryData::new(&url, "navigation", 0.0, values.load_end);
    entry.navigation = Some(Box::new(values));
    Some(entry)
}

fn entry_json<'s>(s: &Scope<'s>, e: &PerformanceEntryData, detail: bool) -> JsResult<Value<'s>> {
    let o = s.object();
    o.set("name", s.string(&e.name))?;
    o.set("entryType", s.string(e.entry_type))?;
    o.set("startTime", s.number(e.start))?;
    o.set("duration", s.number(e.duration))?;
    if detail {
        o.set("detail", e.detail.as_ref().map_or(s.null(), |d| d.get(s)))?;
    }
    if let Some(n) = &e.navigation {
        o.set("initiatorType", s.string("navigation"))?;
        o.set("deliveryType", s.string(""))?;
        o.set("nextHopProtocol", s.string(""))?;
        o.set("renderBlockingStatus", s.string("non-blocking"))?;
        o.set("contentType", s.string(""))?;
        for key in RESOURCE_TIMES.iter().chain(&RESOURCE_SIZES) {
            o.set(key, s.number(0.0))?;
        }
        o.set("serverTiming", s.array(&[])?.value())?;
        for (key, value) in navigation_fields(n) {
            o.set(key, s.number(value))?;
        }
        o.set("type", s.string(n.nav_type))?;
        o.set("redirectCount", s.number(0.0))?;
    }
    Ok(o.value())
}

/// PerformanceResourceTiming's times: all 0 for the navigation entry (see
/// [`NavigationValues`]).
const RESOURCE_TIMES: [&str; 14] = [
    "workerStart",
    "redirectStart",
    "redirectEnd",
    "fetchStart",
    "domainLookupStart",
    "domainLookupEnd",
    "connectStart",
    "connectEnd",
    "secureConnectionStart",
    "requestStart",
    "responseStart",
    "firstInterimResponseStart",
    "finalResponseHeadersStart",
    "responseEnd",
];
/// Sizes and status: not recorded (0).
const RESOURCE_SIZES: [&str; 4] = ["transferSize", "encodedBodySize", "decodedBodySize", "responseStatus"];

fn navigation_fields(n: &NavigationValues) -> [(&'static str, f64); 10] {
    [
        ("unloadEventStart", 0.0),
        ("unloadEventEnd", 0.0),
        ("domInteractive", n.dom_interactive),
        ("domContentLoadedEventStart", n.dom_content_loaded_start),
        ("domContentLoadedEventEnd", n.dom_content_loaded_end),
        ("domComplete", n.dom_complete),
        ("loadEventStart", n.load_start),
        ("loadEventEnd", n.load_end),
        ("activationStart", 0.0),
        ("criticalCHRestart", 0.0),
    ]
}

pub struct PerformanceEntry;

impl JsClass for PerformanceEntry {
    const NAME: &'static str = "PerformanceEntry";
    type Data = PerformanceEntryData;
    type Parent = NoParent;

    fn define(def: &mut ClassDef<Self>) {
        def.getter("name", |s, e| Ok(s.string(&e.name)));
        def.getter("entryType", |s, e| Ok(s.string(e.entry_type)));
        def.getter("startTime", |s, e| Ok(s.number(e.start)));
        def.getter("duration", |s, e| Ok(s.number(e.duration)));
        def.method("toJSON", 0, |s, e, _a| entry_json(s, e, false));
    }
}

/// Paint Timing: `first-paint` / `first-contentful-paint`.
pub struct PerformancePaintTiming;

impl JsClass for PerformancePaintTiming {
    const NAME: &'static str = "PerformancePaintTiming";
    type Data = PerformanceEntryData;
    type Parent = PerformanceEntry;

    fn define(_def: &mut ClassDef<Self>) {}
}

/// Resource Timing. Only the document's `navigation` entry is one: no
/// `resource` entries are recorded for sub-resources, fetch or XHR.
pub struct PerformanceResourceTiming;

impl JsClass for PerformanceResourceTiming {
    const NAME: &'static str = "PerformanceResourceTiming";
    type Data = PerformanceEntryData;
    type Parent = PerformanceEntry;

    fn define(def: &mut ClassDef<Self>) {
        for key in RESOURCE_TIMES.iter().chain(&RESOURCE_SIZES) {
            def.getter(key, |s, _e| Ok(s.number(0.0)));
        }
        def.getter("initiatorType", |s, e| Ok(s.string(if e.navigation.is_some() { "navigation" } else { "" })));
        def.getter("nextHopProtocol", |s, _e| Ok(s.string("")));
        def.getter("deliveryType", |s, _e| Ok(s.string("")));
        def.getter("renderBlockingStatus", |s, _e| Ok(s.string("non-blocking")));
        def.getter("contentType", |s, _e| Ok(s.string("")));
        def.getter("serverTiming", |s, _e| Ok(s.array(&[])?.value()));
        def.method("toJSON", 0, |s, e, _a| entry_json(s, e, false));
    }
}

/// Navigation Timing Level 2: the document's `navigation` entry.
pub struct PerformanceNavigationTiming;

impl JsClass for PerformanceNavigationTiming {
    const NAME: &'static str = "PerformanceNavigationTiming";
    type Data = PerformanceEntryData;
    type Parent = PerformanceResourceTiming;

    fn define(def: &mut ClassDef<Self>) {
        macro_rules! fields {
            ($($i:literal),*) => {$(
                def.getter(navigation_fields(&NavigationValues::EMPTY)[$i].0, |s, e| {
                    Ok(s.number(e.navigation.as_ref().map_or(0.0, |n| navigation_fields(n)[$i].1)))
                });
            )*};
        }
        fields!(0, 1, 2, 3, 4, 5, 6, 7, 8, 9);
        def.getter("type", |s, e| Ok(s.string(e.navigation.as_ref().map_or("navigate", |n| n.nav_type))));
        def.getter("redirectCount", |s, _e| Ok(s.number(0.0)));
        def.getter("notRestoredReasons", |s, _e| Ok(s.null()));
        def.method("toJSON", 0, |s, e, _a| entry_json(s, e, false));
    }
}

/// The legacy `performance.timing`: the document's Navigation Timing in
/// milliseconds since the epoch, 0 for a step that has not run (and for the
/// unload, redirect and secure-connection steps, which never do here).
pub struct PerformanceTiming;

pub struct TimingData {
    origin_wall_ms: f64,
    page: Rc<lp_webapi::PageDom>,
    nav: Rc<PageNav>,
}

/// `performance.timing[name]` (epoch milliseconds, 0 when not reached).
fn legacy_timing(origin: f64, n: &NavigationValues, name: &str) -> f64 {
    let at = |t: f64| if t > 0.0 { (origin + t).round() } else { 0.0 };
    match name {
        "navigationStart" | "fetchStart" | "domainLookupStart" | "domainLookupEnd" | "connectStart" | "connectEnd"
        | "requestStart" | "responseStart" | "responseEnd" | "domLoading" => origin.round(),
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
                    let n = NavigationValues::of(&t.page, &t.nav);
                    Ok(s.number(legacy_timing(t.origin_wall_ms, &n, TIMING_ATTRIBUTES[$i])))
                });
            )*};
        }
        attributes!(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20);
        def.method("toJSON", 0, |s, t, _a| {
            let o = s.object();
            let n = NavigationValues::of(&t.page, &t.nav);
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

/// Navigation Timing's `PerformanceTiming` attribute names: not usable as
/// mark names in a Window (User Timing).
const TIMING_ATTRIBUTES: &[&str] = &[
    "navigationStart",
    "unloadEventStart",
    "unloadEventEnd",
    "redirectStart",
    "redirectEnd",
    "fetchStart",
    "domainLookupStart",
    "domainLookupEnd",
    "connectStart",
    "connectEnd",
    "secureConnectionStart",
    "requestStart",
    "responseStart",
    "responseEnd",
    "domLoading",
    "domInteractive",
    "domContentLoadedEventStart",
    "domContentLoadedEventEnd",
    "domComplete",
    "loadEventStart",
    "loadEventEnd",
];

/// A member of an options dictionary, `None` when absent or undefined.
fn member<'s>(options: Option<lp_js::Object<'s>>, name: &str) -> JsResult<Option<Value<'s>>> {
    match options {
        Some(o) => Ok(Some(o.get(name)?).filter(|v| !v.is_undefined())),
        None => Ok(None),
    }
}

/// The structured clone of a `detail` member (null when absent).
fn clone_detail<'s>(s: &Scope<'s>, detail: Option<Value<'s>>) -> JsResult<Option<Persistent>> {
    let Some(detail) = detail.filter(|v| !v.is_null()) else { return Ok(None) };
    let clone = s.global().get("structuredClone")?;
    let clone = clone.as_object().ok_or_else(|| JsError::Type("structuredClone is unavailable".into()))?;
    Ok(Some(Persistent::new(s, clone.call(None, &[detail])?)))
}

/// User Timing "create a PerformanceMark": `now` is the current time.
fn mark_data<'s>(s: &Scope<'s>, name: &str, options: Value<'s>, now: f64) -> JsResult<PerformanceEntryData> {
    if TIMING_ATTRIBUTES.contains(&name) {
        return Err(JsError::dom(
            "SyntaxError",
            format!("'{name}' is part of the PerformanceTiming interface, and cannot be used as a mark name."),
        ));
    }
    if !options.is_nullish() && options.as_object().is_none() {
        return Err(JsError::Type("The mark options are not an object.".into()));
    }
    let options = options.as_object();
    let start = match member(options, "startTime")? {
        Some(v) => {
            let t = v.to_number()?;
            if !t.is_finite() {
                return Err(JsError::Type("startTime is not a finite number.".into()));
            }
            if t < 0.0 {
                return Err(JsError::Type(format!("'{name}' cannot have a negative start time.")));
            }
            t
        }
        None => now,
    };
    let detail = clone_detail(s, member(options, "detail")?)?;
    Ok(PerformanceEntryData {
        name: name.to_owned(),
        entry_type: "mark",
        start,
        duration: 0.0,
        detail,
        navigation: None,
    })
}

pub struct PerformanceMark;

impl JsClass for PerformanceMark {
    const NAME: &'static str = "PerformanceMark";
    type Data = PerformanceEntryData;
    type Parent = PerformanceEntry;

    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| {
            a.require(1, "PerformanceMark")?;
            let name = a.get(0).to_string()?;
            let now = s.global_data::<crate::window::Window>().map_or(0.0, |w| w.now_ms());
            s.wrap_new::<PerformanceMark>(mark_data(s, &name, a.get(1), now)?, nt, None)
        });
        def.getter("detail", |s, e| Ok(e.detail.as_ref().map_or(s.null(), |d| d.get(s))));
        def.method("toJSON", 0, |s, e, _a| entry_json(s, e, true));
    }
}

pub struct PerformanceMeasure;

impl JsClass for PerformanceMeasure {
    const NAME: &'static str = "PerformanceMeasure";
    type Data = PerformanceEntryData;
    type Parent = PerformanceEntry;

    fn define(def: &mut ClassDef<Self>) {
        def.getter("detail", |s, e| Ok(e.detail.as_ref().map_or(s.null(), |d| d.get(s))));
        def.method("toJSON", 0, |s, e, _a| entry_json(s, e, true));
    }
}

/// User Timing "convert a mark to a timestamp".
fn mark_timestamp(p: &PerformanceData, mark: Value<'_>) -> JsResult<f64> {
    if mark.is_number() {
        let t = mark.to_number()?;
        if t < 0.0 {
            return Err(JsError::Type("A measure cannot start or end at a negative time.".into()));
        }
        return Ok(t);
    }
    let name = mark.to_string()?;
    if name == "navigationStart" {
        return Ok(0.0);
    }
    // User Timing "convert a name to a timestamp": a PerformanceTiming
    // attribute is its time relative to navigationStart, an error while it
    // is 0 (the step has not run).
    if TIMING_ATTRIBUTES.contains(&name.as_str()) {
        let n = NavigationValues::of(&p.target.page, &p.nav);
        if legacy_timing(p.origin_wall_ms, &n, &name) == 0.0 {
            return Err(JsError::dom(
                "InvalidAccessError",
                format!("'{name}' is empty: either the event hasn't happened yet, or it would provide cross-origin timing information."),
            ));
        }
        // The fetch phases are at the time origin (see `NavigationValues`).
        return Ok(navigation_fields(&n).into_iter().find(|(key, _)| *key == name).map_or(0.0, |(_, t)| t));
    }
    let found = p.user_entries.borrow().iter().rev().find(|e| e.mark && e.name == name).map(|e| e.start);
    found.ok_or_else(|| JsError::dom("SyntaxError", format!("The mark '{name}' does not exist.")))
}

/// `performance.measure(name, startOrOptions, endMark)`.
fn measure<'s>(s: &Scope<'s>, p: &PerformanceData, a: &lp_js::Args<'s>) -> JsResult<Value<'s>> {
    a.require(1, "Performance.measure")?;
    let name = a.get(0).to_string()?;
    let end_mark = a.opt(2).filter(|v| !v.is_undefined());
    // A dictionary only when it is an object (a string or number is a mark).
    let options = a.get(1).as_object();
    let (start, end, duration) = (member(options, "start")?, member(options, "end")?, member(options, "duration")?);
    let detail = member(options, "detail")?;
    let is_options = start.is_some() || end.is_some() || duration.is_some() || detail.is_some();
    if is_options {
        if end_mark.is_some() {
            return Err(JsError::Type("If a non-empty measure options object is given, endMark must not be.".into()));
        }
        if start.is_none() && end.is_none() {
            return Err(JsError::Type("The measure options need at least one of start and end.".into()));
        }
        if start.is_some() && end.is_some() && duration.is_some() {
            return Err(JsError::Type("The measure options cannot have all of start, duration and end.".into()));
        }
    }
    let duration = duration.map(|d| d.to_number()).transpose()?;
    let end_time = match (end_mark, end, start, duration) {
        (Some(mark), ..) => mark_timestamp(p, mark)?,
        (None, Some(end), ..) => mark_timestamp(p, end)?,
        (None, None, Some(start), Some(duration)) => mark_timestamp(p, start)? + duration,
        _ => p.nav.now_ms(),
    };
    let start_time = match (start, duration, end) {
        (Some(start), ..) => mark_timestamp(p, start)?,
        (None, Some(duration), Some(end)) => mark_timestamp(p, end)? - duration,
        _ if !is_options && !a.get(1).is_nullish() => mark_timestamp(p, a.get(1))?,
        _ => 0.0,
    };
    let data = PerformanceEntryData {
        name: name.clone(),
        entry_type: "measure",
        start: start_time,
        duration: end_time - start_time,
        detail: clone_detail(s, detail)?,
        navigation: None,
    };
    let object = s.wrap::<PerformanceMeasure>(data, None)?.value();
    p.user_entries.borrow_mut().push(UserEntry {
        name,
        mark: false,
        start: start_time,
        object: Persistent::new(s, object),
    });
    p.nav.notify_entry(s, "measure", object)?;
    Ok(object)
}

/// `clearMarks(name)` / `clearMeasures(name)`.
fn clear_entries(p: &PerformanceData, marks: bool, name: Option<String>) {
    let removed: Vec<UserEntry> = {
        let mut entries = p.user_entries.borrow_mut();
        let (gone, kept) = std::mem::take(&mut *entries)
            .into_iter()
            .partition(|e| e.mark == marks && name.as_ref().is_none_or(|n| *n == e.name));
        *entries = kept;
        gone
    };
    drop(removed);
}

pub struct Performance;

impl JsClass for Performance {
    const NAME: &'static str = "Performance";
    type Data = PerformanceData;
    type Parent = lp_webapi::EventTarget;

    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|p: &PerformanceData| &p.target);
        def.method("now", 0, |s, p, _a| Ok(s.number(p.nav.now_ms())));
        def.getter("timeOrigin", |s, p| Ok(s.number(p.origin_wall_ms)));
        def.method("mark", 1, |s, p, a| {
            a.require(1, "Performance.mark")?;
            let name = a.get(0).to_string()?;
            let data = mark_data(s, &name, a.get(1), p.nav.now_ms())?;
            let start = data.start;
            let object = s.wrap::<PerformanceMark>(data, None)?.value();
            p.user_entries.borrow_mut().push(UserEntry { name, mark: true, start, object: Persistent::new(s, object) });
            p.nav.notify_entry(s, "mark", object)?;
            Ok(object)
        });
        def.method("measure", 1, |s, p, a| measure(s, p, &a));
        def.method("clearMarks", 0, |s, p, a| {
            let name = a.opt(0).filter(|v| !v.is_undefined()).map(|v| v.to_string()).transpose()?;
            clear_entries(p, true, name);
            Ok(s.undefined())
        });
        def.method("clearMeasures", 0, |s, p, a| {
            let name = a.opt(0).filter(|v| !v.is_undefined()).map(|v| v.to_string()).transpose()?;
            clear_entries(p, false, name);
            Ok(s.undefined())
        });
        def.method("getEntries", 0, |s, p, _a| timeline_entries(s, p, None, None));
        def.method("getEntriesByName", 1, |s, p, a| {
            a.require(1, "Performance.getEntriesByName")?;
            let name = a.get(0).to_string()?;
            let ty = a.opt(1).filter(|v| !v.is_undefined()).map(|v| v.to_string()).transpose()?;
            timeline_entries(s, p, Some(&name), ty.as_deref())
        });
        def.method("getEntriesByType", 1, |s, p, a| {
            a.require(1, "Performance.getEntriesByType")?;
            let ty = a.get(0).to_string()?;
            timeline_entries(s, p, None, Some(&ty))
        });
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
    let key = lp_js::IdentityKey(KEY_TIMING, 0);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o.value());
    }
    let data = TimingData { origin_wall_ms: p.origin_wall_ms, page: p.target.page.clone(), nav: p.nav.clone() };
    Ok(s.wrap::<PerformanceTiming>(data, Some(key))?.value())
}

/// `performance.navigation`: one object per realm.
fn navigation_object<'s>(s: &Scope<'s>, p: &PerformanceData) -> JsResult<Value<'s>> {
    Ok(s.wrap::<PerformanceNavigation>(p.nav.clone(), Some(lp_js::IdentityKey(KEY_NAVIGATION, 0)))?.value())
}

/// Identity keys of `performance.timing` and `performance.navigation` (one
/// per realm).
const KEY_TIMING: u64 = 0x5065_7254_696d_6e00;
const KEY_NAVIGATION: u64 = 0x5065_724e_6176_6900;

/// This realm's performance_observer.js module (`PerformanceObserver`,
/// `PerformanceObserverEntryList`), made on first use and kept on the
/// realm's `performance` object.
pub(crate) fn observer_module<'s>(
    s: &Scope<'s>,
    performance: lp_js::Object<'s>,
    nav: &Rc<PageNav>,
) -> JsResult<lp_js::Object<'s>> {
    const KEY: &str = "lpPerformanceObserver";
    if let Some(module) = s.get_hidden(performance, KEY)?.and_then(|v| v.as_object()) {
        return Ok(module);
    }
    let unavailable = || JsError::Type("PerformanceObserver is unavailable".into());
    let natives = s.object();
    let watched = Rc::downgrade(nav);
    let watch = s.function("watch", move |s, _this, a| {
        if let Some(nav) = watched.upgrade() {
            nav.watch_timeline(s, a.get(0));
        }
        Ok(s.undefined())
    })?;
    natives.set_atom("watch", watch.value())?;
    let report = s.function("reportException", |s, _this, a| {
        s.report_exception(&JsError::Thrown(Persistent::new(s, a.get(0))));
        Ok(s.undefined())
    })?;
    natives.set_atom("reportException", report.value())?;
    let root = Persistent::new(s, natives.value());
    let make =
        s.eval(include_str!("performance_observer.js"), Some("lightningpanda://internal/performance-observer.js"), 1)?;
    let module = make.as_object().ok_or_else(unavailable)?.call(None, &[performance.value(), root.get(s)])?;
    s.set_hidden(performance, KEY, module)?;
    module.as_object().ok_or_else(unavailable)
}

/// The realm's Navigation API module (navigation.js): `navigation` and
/// its interfaces, made on the realm's first use and kept on the realm's
/// History object (one per realm).
pub(crate) fn navigation_module<'s>(s: &Scope<'s>, nav: &Rc<PageNav>) -> JsResult<lp_js::Object<'s>> {
    const KEY: &str = "lpNavigation";
    let history = s.wrap::<History>(nav.clone(), Some(lp_js::IdentityKey(crate::window::KEY_HISTORY, 0)))?;
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
        s.report_exception(&JsError::Thrown(Persistent::new(s, a.get(0))));
        Ok(s.undefined())
    })?;
    natives.set_atom("reportException", report.value())?;
    let root = Persistent::new(s, natives.value());
    let make = s.eval(include_str!("navigation.js"), Some("lightningpanda://internal/navigation.js"), 1)?;
    let module = make.as_object().ok_or_else(unavailable)?.call(None, &[root.get(s)])?;
    s.set_hidden(history, KEY, module)?;
    module.as_object().ok_or_else(unavailable)
}
