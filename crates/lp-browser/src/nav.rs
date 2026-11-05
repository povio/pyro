//! Session history, `Location`, `History` and `Performance`.
//!
//! [`PageNav`] is the per-page source of truth for the current URL and the
//! joint session history. JS-facing classes read it synchronously; anything
//! that needs `&mut Browser` (cross-document navigation, events) is posted as
//! a task, as navigation is asynchronous in browsers too.
//!
//! Known gaps (see docs/PARITY.md): history state is stored as JSON (not a
//! full structured clone); traversal across documents re-fetches (no bfcache).
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    url: String,
    /// `JSON.stringify` of the history state (`None` = null).
    state: Option<String>,
    /// Document sequence number: entries of the same document traverse
    /// without reloading.
    doc: u64,
    /// Unique within the page; identifies the entry to automation clients
    /// (CDP `NavigationEntry.id`).
    id: u64,
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
    /// next navigation replaces.
    initial: bool,
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
    time_origin: Cell<Instant>,
    /// When the current document first had a rendering opportunity
    /// (`now_ms`): its Paint Timing entries.
    first_paint: Cell<Option<f64>>,
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
            }),
            document: RefCell::new(None),
            time_origin: Cell::new(crate::clock::now()),
            first_paint: Cell::new(None),
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

    /// Records a committed cross-document navigation.
    pub(crate) fn commit_document(&self, url: &str, history: HistoryHandling, document: Option<(DomRef, NodeId)>) {
        self.pending_frame_url.borrow_mut().take();
        let mut st = self.state.borrow_mut();
        st.doc_seq += 1;
        let id = st.next_entry_id();
        let entry = Entry { url: url.to_string(), state: None, doc: st.doc_seq, id };
        // A push to the current entry's URL replaces it (HTML navigate step:
        // "url equals the active document's URL" -> replace).
        let same_url = st.entries.get(st.index).is_some_and(|e| e.url == url);
        let initial = std::mem::take(&mut st.initial);
        match history {
            _ if st.entries.is_empty() => {
                st.initial = url == "about:blank";
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
        self.time_origin.set(crate::clock::now());
        self.first_paint.set(None);
    }

    pub(crate) fn set_document(&self, document: Option<(DomRef, NodeId)>) {
        *self.document.borrow_mut() = document;
    }

    fn set_url_sync(&self, url: &str) {
        self.state.borrow_mut().url = url.to_string();
        if let Some((dom, doc)) = self.document.borrow().as_ref() {
            let _ = dom.borrow_mut().set_document_url(*doc, url);
        }
    }

    /// Same-document navigation (pushState/replaceState/fragment).
    fn same_document(&self, url: &str, state: Option<String>, kind: SameDocumentKind) {
        {
            let mut st = self.state.borrow_mut();
            st.state_version += 1;
            let doc = st.entries.get(st.index).map_or(st.doc_seq, |e| e.doc);
            let id = st.next_entry_id();
            let entry = Entry { url: url.to_string(), state, doc, id };
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
        let document = self.state.borrow().doc_seq;
        let frame_id = self.frame_id.clone();
        self.sched.post(TaskSource::Navigation, move |b| {
            let Some(page) = b.page(page).and_then(|p| p.frames.key(&frame_id)) else { return };
            if !b.doc(page).is_some_and(|p| p.nav.state.borrow().doc_seq == document) {
                return;
            }
            let opts = NavigateOpts {
                post: None,
                url,
                reason: NavigationReason::Script,
                cdp_request_id: None,
                referrer: None,
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
                b.fire_popstate(page, entry.state.as_deref(), &old_url, &entry.url);
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
        self.same_document(target.as_str(), None, kind);
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

    fn state_json(&self) -> Option<String> {
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
        let value = nav.state_json().and_then(|j| s.parse_json(&j)).unwrap_or_else(|| s.null());
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
    fn fire_popstate(&mut self, page: crate::frames::FrameKey, state: Option<&str>, old_url: &str, new_url: &str) {
        let Some(p) = self.doc(page) else { return };
        let Some(dom) = p.main_frame.page_dom.clone() else { return };
        // The event's state is the history state object itself (browsers:
        // `event.state === history.state`).
        let nav = p.nav.clone();
        let state = match (state, p.main_frame.main_realm().and_then(|r| r.scope().ok())) {
            (Some(_), Some(scope)) => {
                PageNav::state_value(&nav, &scope).ok().map(|v| lp_js::Persistent::new(&scope, v))
            }
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
    let target = parse(nav, input)?;
    let current = Url::parse(&nav.url()).ok();
    match current {
        Some(cur) if target.fragment().is_some() && same_document_except_fragment(&cur, &target) => {
            let kind = if replace { SameDocumentKind::Replace } else { SameDocumentKind::Fragment };
            nav.same_document(target.as_str(), None, kind);
            nav.queue_hashchange(cur.as_str(), target.as_str());
        }
        _ => nav.navigate(target.to_string(), if replace { HistoryHandling::Replace } else { HistoryHandling::Push }),
    }
    Ok(())
}

fn url_part(nav: &PageNav, f: impl Fn(&Url) -> String) -> String {
    Url::parse(&nav.url()).map(|u| f(&u)).unwrap_or_default()
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
            n.navigate(n.url(), HistoryHandling::Reload);
            Ok(s.undefined())
        });
        def.getter("origin", |s, n| {
            Ok(s.string(&url_part(n, |u| match u.origin() {
                o @ url::Origin::Tuple(..) => o.ascii_serialization(),
                url::Origin::Opaque(_) => "null".into(),
            })))
        });
        def.getter("protocol", |s, n| Ok(s.string(&url_part(n, |u| format!("{}:", u.scheme())))));
        def.getter("host", |s, n| {
            Ok(s.string(&url_part(n, |u| match (u.host_str(), u.port()) {
                (Some(h), Some(p)) => format!("{h}:{p}"),
                (Some(h), None) => h.to_string(),
                _ => String::new(),
            })))
        });
        def.getter("hostname", |s, n| Ok(s.string(&url_part(n, |u| u.host_str().unwrap_or("").to_string()))));
        def.getter("port", |s, n| Ok(s.string(&url_part(n, |u| u.port().map(|p| p.to_string()).unwrap_or_default()))));
        def.getter("pathname", |s, n| Ok(s.string(&url_part(n, |u| u.path().to_string()))));
        def.getter("search", |s, n| {
            Ok(s.string(&url_part(n, |u| match u.query() {
                Some(q) if !q.is_empty() => format!("?{q}"),
                _ => String::new(),
            })))
        });
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
                u.set_fragment(Some(h.strip_prefix('#').unwrap_or(&h)));
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
    // Approximates StructuredSerialize with JSON (functions/cycles throw).
    let state = match a.get(0) {
        v if v.is_nullish() => None,
        v => Some(v.to_json(0)?.ok_or_else(|| JsError::dom("DataCloneError", "state could not be cloned"))?),
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
    n.same_document(&url, state, kind);
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
}

impl PerformanceEntryData {
    fn new(name: &str, entry_type: &'static str, start: f64, duration: f64) -> PerformanceEntryData {
        PerformanceEntryData { name: name.to_owned(), entry_type, start, duration, detail: None }
    }
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
    Ok(o.value())
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
    Ok(PerformanceEntryData { name: name.to_owned(), entry_type: "mark", start, duration: 0.0, detail })
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
    };
    let object = s.wrap::<PerformanceMeasure>(data, None)?.value();
    p.user_entries.borrow_mut().push(UserEntry { name, mark: false, start: start_time, object: Persistent::new(s, object) });
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
        def.method("toJSON", 0, |s, p, _a| {
            let o = s.object();
            o.set("timeOrigin", s.number(p.origin_wall_ms))?;
            Ok(o.value())
        });
    }
}
