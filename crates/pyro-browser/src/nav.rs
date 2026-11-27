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
    PerformanceNavigationTiming, UserTiming, RESOURCE_TIMES, TIMING_ATTRIBUTES,
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

/// What the Navigation API needs to know of an entry's document once it is not the active one:
/// its origin (an `about:blank`, `srcdoc` or `blob:` document inherits one its URL does not show)
/// and whether its referrer policy was `no-referrer` when it was left (`NavigationHistoryEntry.url`
/// reads null then). Recorded while the document is alive; never part of an entry's identity.
#[derive(Debug, Clone, Default)]
struct DocFacts {
    origin: Option<String>,
    no_referrer: bool,
}
impl PartialEq for DocFacts {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
impl Eq for DocFacts {}

/// A 64-bit finalizer (splitmix64's): a bijection, so distinct inputs stay distinct.
fn mix64(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// The Navigation API's key or id of an entry: a version 4 (variant 1) UUID shaped string made of the
/// frame's random `salt` and the entry's counter value `n`. Unique per counter value within a frame
/// (the first half is a bijection of `salt + n`), different between frames and not guessable from
/// the counter, as Chrome's random keys; the same `n` always gives the same string.
fn uuid_of(salt: u64, n: u64) -> String {
    let a = mix64(salt.wrapping_add(n));
    let b = mix64(a ^ salt.rotate_left(32) ^ 0x7079_7230_4e41_0000);
    format!(
        "{:08x}-{:04x}-4{:03x}-{:x}{:03x}-{:012x}",
        a >> 32,
        (a >> 16) & 0xffff,
        a & 0xfff,
        8 | ((b >> 60) & 3),
        (b >> 48) & 0xfff,
        b & 0xffff_ffff_ffff
    )
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
    /// HTML "step": where the entry sits on its page's joint session history
    /// ([`JointHistory`]), shared by the entries of all its frames.
    step: u64,
    /// HTML "scroll restoration mode" of the entry (`history.scrollRestoration`): "manual"
    /// when true. A new document's entry starts "auto"; pushState and replaceState keep the
    /// current entry's.
    scroll_manual: bool,
    /// See [`DocFacts`].
    facts: DocFacts,
}

/// The ids [`PageNav::history_entries`] gives the steps only a child frame
/// made: above any entry id, below 2^53 (a JSON number stays exact).
const STEP_ENTRY_ID: u64 = 1 << 52;

/// HTML session history of a traversable (a page and its frames): each frame
/// keeps its own entry list ([`NavState`]), and an entry's `step` places it
/// on the page's timeline. `history.length` counts the steps of all the
/// page's frames, a push in any frame clears the forward steps of all of
/// them, and a traversal moves every frame whose entry at the target step
/// differs (an iframe's `history.back()` from the top document).
#[derive(Default)]
pub(crate) struct JointHistory {
    /// The last step given out.
    last_step: Cell<u64>,
    /// The traversable's current session history step.
    current: Cell<u64>,
    /// The navigation states of the page's frames.
    members: RefCell<Vec<std::rc::Weak<PageNav>>>,
    /// Counts the traversals applied: of several asked for in one task the
    /// first wins.
    traversals: Cell<u64>,
}

impl JointHistory {
    /// A push: the step after every other, now the current one, and the
    /// step it was pushed from.
    fn begin_push(&self) -> (u64, u64) {
        let from = self.current.get();
        let step = self.last_step.get() + 1;
        self.last_step.set(step);
        self.current.set(step);
        (from, step)
    }

    /// The frames whose documents are fully active (a removed frame's
    /// entries, or those of a document navigated away from, are gone).
    fn members(&self) -> Vec<Rc<PageNav>> {
        let mut members = self.members.borrow_mut();
        members.retain(|m| m.strong_count() > 0);
        members.iter().filter_map(std::rc::Weak::upgrade).filter(|m| m.fully_active()).collect()
    }

    /// The steps of `members`' entries, in order.
    fn steps(members: &[Rc<PageNav>]) -> Vec<u64> {
        let mut steps: Vec<u64> = Vec::new();
        for m in members {
            steps.extend(m.state.borrow().entries.iter().map(|e| e.step));
        }
        steps.sort_unstable();
        steps.dedup();
        steps
    }

    /// HTML "clear the forward session history" after a push from step
    /// `from` in frame `except` (which dropped its own forward entries).
    ///
    /// Every live member, not only the fully active ones: dropping forward entries of a frame that
    /// is gone changes nothing, and asking whether a frame is active borrows the frame host, which
    /// a commit in progress (`Browser::commit_document`) holds.
    fn prune_forward(&self, except: &PageNav, from: u64) {
        let members: Vec<Rc<PageNav>> = {
            let mut members = self.members.borrow_mut();
            members.retain(|m| m.strong_count() > 0);
            members.iter().filter_map(std::rc::Weak::upgrade).collect()
        };
        for m in members {
            if std::ptr::eq(Rc::as_ptr(&m), except) {
                continue;
            }
            let mut st = m.state.borrow_mut();
            let keep = st.entries.iter().position(|e| e.step > from).unwrap_or(st.entries.len()).max(st.index + 1);
            st.entries.truncate(keep);
        }
    }
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
    /// `window.stop()` calls (HTML "stop loading" aborts the document): a
    /// document load begun before one fires no DOMContentLoaded or load.
    aborts: u64,
    /// HTML `navigation.activation` of the active document: how its navigation came to be
    /// (`None` for the initial about:blank document). Fixed when the document commits;
    /// same-document navigations leave it alone.
    activation: Option<Activation>,
}

/// A document's NavigationActivation data: its navigation type, the document's first session
/// history entry as (key, id, url), and the entry the navigation left as (key, id, url, Navigation
/// API state) when the Navigation API exposes it (same origin as the new document), else `None`.
#[derive(Clone)]
struct Activation {
    ty: &'static str,
    entry: (u64, u64, String),
    /// The url is `None` when the left document's referrer policy was `no-referrer`.
    from: Option<(u64, u64, Option<String>, Option<String>)>,
}

impl NavState {
    fn next_entry_id(&mut self) -> u64 {
        self.entry_seq += 1;
        self.entry_seq
    }
}

/// Per-page navigation state shared by the page and its windows.
pub struct PageNav {
    /// Random per frame: mixed into the Navigation API entry keys and ids (`uuid_of`).
    salt: u64,
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
    /// The URL the current document was created with (its navigation's final URL): the
    /// navigation entry's `name`, which `document.open()` and fragment navigations leave alone.
    creation_url: RefCell<String>,
    /// When the previous document's unload ran (set by `Browser::commit_document` for a
    /// same-origin previous document, taken by the commit).
    pending_unload: Cell<Option<(Instant, Instant)>>,
    /// The current document's `unloadEventStart`/`unloadEventEnd` (time-origin ms; 0: not exposed).
    unload_times: Cell<(f64, f64)>,
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
    /// Whether the navigation being requested has a user activation of its initiator (URL
    /// Fragment Text Directives: a directive activates only for one that has). `None`: nothing
    /// said, as for a browser-initiated navigation. Taken by the commit or the fragment
    /// navigation it is for.
    pub(crate) directive_gesture: Cell<Option<bool>>,
    /// Chrome's navigation rate limiter (crbug.com/1038223): the start of
    /// the current period and the history updates and navigations counted
    /// in it.
    rate_limit: Cell<(Instant, u32)>,
    /// The joint session history of the page this frame belongs to.
    joint: Rc<JointHistory>,
    /// Script navigations queued ([`PageNav::navigate`]) whose task has not
    /// run yet.
    queued_navigations: Rc<Cell<u32>>,
    /// Iframe attribute processings queued for this frame, and run
    /// (`FrameHost::queue_attributes`).
    pub(crate) attributes_queued: Cell<u64>,
    pub(crate) attributes_run: Cell<u64>,
    /// While a script navigation's task navigates: the processings queued
    /// when it was asked for. HTML "navigate" clears the lazy load
    /// resumption steps then, so a `src` set after it still lazy loads.
    pub(crate) lazy_cancel_limit: Cell<Option<u64>>,
    /// A traversal to another document's entry under way: the index it left,
    /// the document it left and the page's step before it, restored if no
    /// document commits.
    traversal_from: Cell<Option<(usize, u64, u64)>>,
    /// The container's `referrerpolicy` when the frame's next navigation
    /// began (HTML "beginning navigation" snapshot), and that of the
    /// navigation of the current document; `None`: not known (a navigation
    /// the container did not start), the attribute as it is now is used.
    container_policy_pending: Cell<Option<Option<pyro_net::ReferrerPolicy>>>,
    container_policy: Cell<Option<Option<pyro_net::ReferrerPolicy>>>,
    /// The current document's ancestor origins list, made when it committed
    /// (`FrameHost::store_ancestor_origins`): `location.ancestorOrigins`.
    ancestor_origins: RefCell<Vec<String>>,
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
    /// The initiator is not same origin with the navigated document: a navigation to that
    /// document's own URL is not made a "replace" (HTML navigate: the URL comparison is only
    /// for a same-origin initiator, which can see it).
    pub cross_origin: bool,
    /// `NavigateEvent.userInitiated`: the activation behavior of a trusted click (a followed
    /// link, a submitted form).
    pub user_initiated: bool,
    /// `history.pushState` / `replaceState`: HTML's "URL and history update steps" fire a
    /// `navigate` event that is never a `hashChange`.
    pub history_api: bool,
}

impl PageNav {
    /// The navigation state of a page's top-level frame, with a joint
    /// session history of its own.
    pub(crate) fn new(
        page: PageId,
        frame_id: FrameId,
        sched: Scheduler<Browser>,
        observer: ObserverCell,
    ) -> Rc<PageNav> {
        Self::with_joint(page, frame_id, sched, observer, Rc::default())
    }

    /// The navigation state of a child frame of the page whose top-level
    /// frame's state is `top`: their session histories are joint.
    pub(crate) fn new_child(
        top: &PageNav,
        page: PageId,
        frame_id: FrameId,
        sched: Scheduler<Browser>,
        observer: ObserverCell,
    ) -> Rc<PageNav> {
        Self::with_joint(page, frame_id, sched, observer, top.joint.clone())
    }

    fn with_joint(
        page: PageId,
        frame_id: FrameId,
        sched: Scheduler<Browser>,
        observer: ObserverCell,
        joint: Rc<JointHistory>,
    ) -> Rc<PageNav> {
        let nav = Rc::new(PageNav {
            salt: pyro_webapi::crypto::random_u64(),
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
                scroll_positions: Default::default(),
                initial: false,
                first_blank: false,
                stops: 0,
                aborts: 0,
                activation: None,
            }),
            document: RefCell::new(None),
            page_dom: RefCell::new(std::rc::Weak::new()),
            time_origin: Cell::new(crate::clock::now()),
            first_paint: Cell::new(None),
            nav_type: Cell::new("navigate"),
            creation_url: RefCell::new(String::new()),
            pending_unload: Cell::new(None),
            unload_times: Cell::new((0.0, 0.0)),
            incoming_fetch: RefCell::new(None),
            fetch_timing: RefCell::new(None),
            timeline_watchers: RefCell::new(Vec::new()),
            navigation_hooks: RefCell::new(Vec::new()),
            navigation_api_call: Cell::new(false),
            directive_gesture: Cell::new(None),
            rate_limit: Cell::new((Instant::now(), 0)),
            joint,
            queued_navigations: Rc::new(Cell::new(0)),
            attributes_queued: Cell::new(0),
            attributes_run: Cell::new(0),
            lazy_cancel_limit: Cell::new(None),
            traversal_from: Cell::new(None),
            container_policy_pending: Cell::new(None),
            container_policy: Cell::new(None),
            ancestor_origins: RefCell::new(Vec::new()),
        });
        nav.joint.members.borrow_mut().push(Rc::downgrade(&nav));
        nav
    }

    pub fn url(&self) -> String {
        self.state.borrow().url.clone()
    }

    /// The step of the page's first (top-level) frame's entry that covers `current`: its last
    /// entry at or before it. `current` when that frame has none, or is this one.
    fn covering_step(&self, current: u64) -> u64 {
        let members = self.joint.members.borrow();
        let top = members.first().and_then(std::rc::Weak::upgrade);
        let Some(top) = top.filter(|top| !std::ptr::eq(Rc::as_ptr(top), self)) else { return current };
        let step = top.state.try_borrow().ok().and_then(|st| st.entries.iter().rev().find(|e| e.step <= current).map(|e| e.step));
        step.unwrap_or(current)
    }

    /// `history.length`: the steps of the page's joint session history.
    pub fn history_length(&self) -> usize {
        let members = self.joint.members();
        if members.is_empty() {
            return self.state.borrow().entries.len().max(1);
        }
        JointHistory::steps(&members).len().max(1)
    }

    /// The session history as Chrome's top-level navigation entries list
    /// it (CDP `Page.getNavigationHistory`), and the index of the current
    /// one: one entry per step of the page's joint session history, with
    /// this frame's URL at that step. A step of this frame's own entry has
    /// that entry's id; a step only a child frame made has an id of its own
    /// ([`STEP_ENTRY_ID`] plus the step).
    pub fn history_entries(&self) -> (usize, Vec<HistoryEntry>) {
        let st = self.state.borrow();
        let members = self.joint.members();
        let steps = JointHistory::steps(&members);
        if steps.is_empty() || st.entries.is_empty() {
            let entries = st.entries.iter().map(|e| HistoryEntry { id: e.id, url: e.url.clone() }).collect();
            return (st.index, entries);
        }
        let current = self.joint.current.get();
        let index = steps.iter().rposition(|s| *s <= current).unwrap_or(0);
        let entries = steps
            .iter()
            .map(|&step| {
                let entry = st.entries.iter().rev().find(|e| e.step <= step).unwrap_or(&st.entries[0]);
                let id = if entry.step == step { entry.id } else { STEP_ENTRY_ID + step };
                HistoryEntry { id, url: entry.url.clone() }
            })
            .collect();
        (index, entries)
    }

    /// Queues a traversal to the entry of [`PageNav::history_entries`] with
    /// `id`, like `history.go()` with the matching delta (the current entry
    /// reloads). Returns false when no entry has that id.
    pub fn traverse_to_entry(&self, id: u64) -> bool {
        let (index, entries) = self.history_entries();
        let Some(target) = entries.iter().position(|e| e.id == id) else { return false };
        let delta = target as i64 - index as i64;
        if delta == 0 {
            self.traverse(0);
        } else {
            self.traverse_joint(delta);
        }
        true
    }

    /// The active document is the page's initial about:blank one (no
    /// navigation has committed since it).
    pub(crate) fn is_initial_blank(&self) -> bool {
        self.state.borrow().initial
    }

    /// HTML "has entries and events disabled": the document is the initial about:blank one or
    /// has an opaque origin. The Navigation API fires no events for it (not even for a fragment
    /// navigation or `history.pushState`) and its methods' promises never settle.
    pub(crate) fn navigation_disabled(&self) -> bool {
        self.is_initial_blank() || self.has_opaque_origin()
    }

    /// The active document has an opaque origin (a sandbox, a data: URL, about:blank of those):
    /// the Navigation API lists no entries for it.
    pub(crate) fn has_opaque_origin(&self) -> bool {
        self.page_dom
            .borrow()
            .upgrade()
            .is_some_and(|page| page.has_opaque_origin() || (page.dom.try_borrow().is_ok() && page.origin_key().starts_with("opaque")))
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

    /// `window.stop()` aborted the active document (HTML "stop loading"),
    /// at once: the frame's document load sees it before it parses further
    /// or fires DOMContentLoaded or load. A navigation its scripts queued and
    /// that has not started is dropped too ([`PageNav::stop_count`]), as one
    /// fetching is canceled.
    pub(crate) fn abort_document(&self) {
        {
            let mut st = self.state.borrow_mut();
            st.aborts += 1;
            st.stops += 1;
        }
        self.navigation_stopped();
    }

    /// `window.stop()` ran: the Navigation API aborts a navigation still waiting for its
    /// `precommitHandler`s (navigation.js `hook('stop')`).
    fn navigation_stopped(&self) {
        if self.navigation_hooks.borrow().is_empty() {
            return;
        }
        for (realm, hook) in self.navigation_hooks() {
            let Some(realm) = realm.upgrade() else { continue };
            let Ok(s) = realm.scope() else { continue };
            if let Some(f) = hook.get(&s).as_object() {
                if let Err(e) = f.call(None, &[s.string("stop")]) {
                    s.report_exception(&e);
                }
            }
        }
    }

    /// Changes when `window.stop()` aborts the frame's document.
    pub(crate) fn abort_count(&self) -> u64 {
        self.state.borrow().aborts
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
        let traversal_from = self.traversal_from.take();
        self.container_policy.set(self.container_policy_pending.take());
        self.ancestor_origins.borrow_mut().clear();
        // The document this navigation leaves is still the frame's: what its entries need later.
        let leaving = self.page_dom.borrow().upgrade().filter(|p| p.dom.try_borrow().is_ok()).map(|p| {
            let origin = Some(p.origin_key()).filter(|k| !k.starts_with("opaque"));
            (origin, matches!(p.referrer_policy(), pyro_net::ReferrerPolicy::NoReferrer))
        });
        let mut st = self.state.borrow_mut();
        if let Some((origin, no_referrer)) = leaving {
            let doc = st.doc_seq;
            for e in st.entries.iter_mut().filter(|e| e.doc == doc) {
                e.facts = DocFacts { origin: origin.clone().or_else(|| e.facts.origin.take()), no_referrer };
            }
        }
        // The entry this navigation leaves (HTML `navigation.activation.from`): a traversal's is
        // the one it started from (the index is already the target's), any other the current one.
        // The initial about:blank document has none.
        let left = if st.entries.is_empty() {
            None
        } else {
            let i = match (history, traversal_from) {
                (HistoryHandling::Traverse, Some((i, ..))) => i,
                _ => st.index,
            };
            st.entries.get(i).cloned()
        };
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
        let current_step = match st.entries.get(st.index) {
            Some(e) => e.step,
            // A frame's first (initial about:blank) entry joins the step of the page's top-level
            // document that covers the current step, not a step a removed frame pushed (the
            // current step stays at it, and a frame inserted since would otherwise add a step
            // to `history.length`).
            None => self.covering_step(self.joint.current.get()),
        };
        let mut entry = Entry {
            url: entry_url.to_string(),
            state: None,
            doc: st.doc_seq,
            id,
            key,
            nav_state: None,
            step: current_step,
            scroll_manual: false,
            facts: DocFacts::default(),
        };
        let initial = std::mem::take(&mut st.initial);
        st.first_blank = false;
        let mut pushed_from = None;
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
                // The entries that shared the restored entry's document share the new one
                // (`entry.sameDocument` of the entries around it holds again).
                let (i, doc) = (st.index, st.doc_seq);
                let shared = st.entries[i].doc;
                for e in st.entries.iter_mut().filter(|e| e.doc == shared) {
                    e.doc = doc;
                    e.facts = DocFacts::default();
                }
                st.entries[i].url = entry_url.to_string();
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
                let (from, step) = self.joint.begin_push();
                entry.step = step;
                pushed_from = Some(from);
                let i = st.index + 1;
                st.entries.truncate(i);
                st.entries.push(entry);
                st.index = i;
            }
        }
        st.url = url.to_string();
        // `from` is null across origins (and for an opaque origin such as about:blank's): the
        // Navigation API shows only the same-origin entries around the current one.
        let activation = st.entries.get(st.index).and_then(|entry| {
            // A push the initial about:blank document or a same-URL entry takes is a replace.
            let ty = match history {
                HistoryHandling::Push if initial || same_url => "replace",
                HistoryHandling::Push => "push",
                HistoryHandling::Replace => "replace",
                HistoryHandling::Traverse => "traverse",
                HistoryHandling::Reload => "reload",
            };
            let origin = |url: &str| Url::parse(url).ok().map(|u| u.origin());
            let here = origin(&entry.url);
            let from = left.as_ref().filter(|left| {
                here.as_ref().is_some_and(|o| o.is_tuple()) && origin(&left.url) == here
            });
            // An entry of a no-referrer document shows no URL once that document was left.
            // The frame's own initial about:blank document is not a navigation's result.
            if left.is_none() && url == "about:blank" {
                return None;
            }
            Some(Activation {
                ty,
                entry: (entry.key, entry.id, entry.url.clone()),
                from: from.map(|left| {
                    (left.key, left.id, Some(left.url.clone()).filter(|_| !left.facts.no_referrer), left.nav_state.clone())
                }),
            })
        });
        st.activation = activation;
        drop(st);
        if let Some(from) = pushed_from {
            self.joint.prune_forward(self, from);
        }
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
        let cross_origin_redirect = fetch.as_ref().and_then(|f| f.response.as_ref()).is_some_and(|r| r.timing.cross_origin_redirect);
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
        // Exposed only without a cross-origin redirect (the unload says what the previous document did).
        let hidden = self.fetch_timing.borrow().as_ref().is_none() || cross_origin_redirect;
        self.unload_times.set(match self.pending_unload.take() {
            Some((start, end)) if !hidden => {
                let ms = |t: Instant| (t.saturating_duration_since(origin).as_micros() / 100) as f64 / 10.0;
                (ms(start), ms(end).max(ms(start)))
            }
            _ => (0.0, 0.0),
        });
        *self.creation_url.borrow_mut() = url.to_owned();
        self.nav_type.set(match history {
            HistoryHandling::Reload => "reload",
            HistoryHandling::Traverse => "back_forward",
            _ => "navigate",
        });
    }

    /// Records when the previous document's unload ran, for the commit that follows
    /// (`None`: not exposed to the next document).
    pub(crate) fn set_unload_span(&self, span: Option<(Instant, Instant)>) {
        self.pending_unload.set(span);
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

    /// `document.open()` gave the document its entry document's URL: the
    /// current entry takes it, without a new entry or a navigation.
    pub(crate) fn document_opened_url(&self, url: &str) {
        self.update_initial_blank_url(url);
        // Frame::url (what CDP reports) follows at the next checkpoint
        // (`Browser::apply_same_document_url`), as for a same-document change.
        *self.pending_frame_url.borrow_mut() = Some(url.to_string());
    }

    fn set_url_sync(&self, url: &str) {
        self.state.borrow_mut().url = url.to_string();
        if let Some((dom, doc)) = self.document.borrow().as_ref() {
            let _ = dom.borrow_mut().set_document_url(*doc, url);
        }
    }

    /// Same-document navigation (pushState/replaceState/fragment).
    fn same_document(&self, url: &str, state: Option<HistoryState>, kind: SameDocumentKind) {
        // HTML "navigate": a navigation of the initial about:blank document replaces, a fragment
        // navigation included; that leaves the document initial (the next navigation replaces it
        // too).
        // pushState too (HTML "URL and history update steps": the initial about:blank document
        // makes the history handling "replace").
        let kind = if kind != SameDocumentKind::Replace && self.state.borrow().initial {
            SameDocumentKind::Replace
        } else {
            kind
        };
        self.persist_scroll();
        let mut pushed_from = None;
        {
            let mut st = self.state.borrow_mut();
            st.state_version += 1;
            let doc = st.entries.get(st.index).map_or(st.doc_seq, |e| e.doc);
            let id = st.next_entry_id();
            // The Navigation API state carries over (HTML "URL and history
            // update steps"); a replace keeps the slot's key.
            let current = st.entries.get(st.index);
            let nav_state = current.and_then(|e| e.nav_state.clone());
            let replace_key = current.filter(|_| kind == SameDocumentKind::Replace).map(|e| e.key);
            let step = current.map_or(self.joint.current.get(), |e| e.step);
            let scroll_manual = current.is_some_and(|e| e.scroll_manual);
            let facts = current.map(|e| e.facts.clone()).unwrap_or_default();
            let key = match replace_key {
                Some(key) => key,
                None => st.next_entry_id(),
            };
            let mut entry = Entry { url: url.to_string(), state, doc, id, key, nav_state, step, scroll_manual, facts };
            if kind != SameDocumentKind::Replace {
                st.initial = false;
            }
            if st.entries.is_empty() {
                st.entries.push(entry);
            } else if kind == SameDocumentKind::Replace {
                let i = st.index;
                st.entries[i] = entry;
            } else {
                let (from, step) = self.joint.begin_push();
                entry.step = step;
                pushed_from = Some(from);
                let i = st.index + 1;
                st.entries.truncate(i);
                st.entries.push(entry);
                st.index = i;
            }
        }
        if let Some(from) = pushed_from {
            self.joint.prune_forward(self, from);
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
    /// `initiator`: the CSP of the document that navigated (its realm's, snapshotted now), which
    /// the URL is checked against; `None` (no realm to name one): this frame's document's.
    fn run_javascript_url(&self, url: &str, initiator: Option<pyro_webapi::csp::NavigationInitiator>) {
        let Some(source) = crate::bindings::javascript_url_source(url) else { return };
        let page = self.page;
        let frame_id = self.frame_id.clone();
        let document = self.state.borrow().doc_seq;
        let url = url.to_owned();
        // HTML "navigate to a javascript: URL" makes this the navigable's ongoing navigation as it is asked
        // for: a navigation started before the task runs (a link's default action after its onclick set
        // `location = "javascript:..."`) supersedes it, and nothing is evaluated.
        let asked = self.page_dom.borrow().upgrade().map(|dom| dom.navigation_started());
        self.sched.post(TaskSource::Navigation, move |b| {
            let Some(key) = b.page(page).and_then(|p| p.frames.key(&frame_id)) else { return };
            let Some(doc) = b.doc(key).filter(|p| p.nav.state.borrow().doc_seq == document) else { return };
            let (realm, page_dom) = (doc.main_frame.main_realm().cloned(), doc.main_frame.page_dom.clone());
            drop(doc);
            let (Some(realm), Some(page_dom)) = (realm, page_dom) else { return };
            if asked.is_some_and(|asked| page_dom.navigation_started() != asked) {
                return;
            }
            // CSP `script-src-elem` of the initiator for a navigation ('unsafe-inline', or with
            // 'unsafe-hashes' a hash of the whole URL): blocked, nothing runs.
            let initiator = initiator.unwrap_or_else(|| pyro_webapi::csp::NavigationInitiator::of(&page_dom));
            if !initiator.allows_javascript_url(&url) {
                return;
            }
            // The script then runs in the target document: its own policies apply too, after the
            // initiator's (Chrome checks the parent's first; its violation fires in the target).
            if !Rc::ptr_eq(&initiator.page, &page_dom) && !pyro_webapi::csp::NavigationInitiator::of(&page_dom).allows_javascript_url(&url) {
                return;
            }
            let Ok(scope) = realm.scope() else { return };
            let Some(source) = pyro_webapi::trusted_types::javascript_url_script(&scope, &page_dom, &source) else { return };
            // HTML "evaluate a javascript: URL": a string result replaces the document
            // (a load the container sees, as for an iframe's src).
            let started = page_dom.navigation_started();
            let Some(html) = crate::frames::javascript_url_result(&scope, &source) else { return };
            drop(scope);
            // The script may have navigated or removed the frame meanwhile: a navigation it
            // started (a form's `submit()`, `location`) wins over the result (HTML "navigate to a
            // javascript: URL": the navigable's ongoing navigation changed).
            if page_dom.navigation_started() == started && b.doc(key).is_some_and(|p| p.nav.state.borrow().doc_seq == document) {
                let _ = b.begin_javascript_result(key, html);
            }
        });
    }

    fn navigate(&self, url: String, history: HistoryHandling) {
        self.navigate_from(url, history, None);
    }

    /// `navigate` with the referrer of the source document (HTML "navigate": the document of
    /// the script that navigates, e.g. a parent calling a child frame's `location.assign`);
    /// `None`: the navigated frame's own document.
    fn navigate_from(&self, url: String, history: HistoryHandling, source_referrer: Option<String>) {
        let current = self.page_dom.borrow().upgrade();
        let mut started = None;
        if let Some(page) = &current {
            // HTML "navigate": a navigable whose document's unload counter is above 0 (its
            // `beforeunload`, `pagehide` or `unload` listeners run) does not navigate; the
            // navigation unloading it goes on.
            if page.in_unload() {
                return;
            }
            page.note_navigation_request();
            // The frame's latest navigation from now on: it supersedes those
            // started before it (a planned form submission too), and a later
            // one supersedes it (`PageDom::navigation_started`).
            started = Some(page.navigation_started());
            // "Checking if unloading is canceled" runs synchronously, before `assign()` (or the
            // `href` setter) returns; the navigation task does not fire it again.
            pyro_webapi::lifecycle::before_unload(page);
            page.mark_before_unload_fired(!page.is_disposed());
        }
        let sequence = self.script_navigation.get().wrapping_add(1);
        self.script_navigation.set(sequence);
        let page = self.page;
        let (document, stops) = {
            let st = self.state.borrow();
            (st.doc_seq, st.stops)
        };
        let frame_id = self.frame_id.clone();
        let attributes_queued = self.attributes_queued.get();
        let queued = self.queued_navigations.clone();
        queued.set(queued.get().saturating_add(1));
        self.sched.post(TaskSource::Navigation, move |b| {
            queued.set(queued.get().saturating_sub(1));
            let Some(page) = b.page(page).and_then(|p| p.frames.key(&frame_id)) else { return };
            // Another document, or this one was opened since (stop loading).
            if !b.doc(page).is_some_and(|p| {
                let st = p.nav.state.borrow();
                st.doc_seq == document
                    && st.stops == stops
                    && p.nav.script_navigation.get() == sequence
                    && started.is_none_or(|n| p.main_frame.page_dom.as_ref().is_none_or(|d| d.navigation_started() == n))
            }) {
                // Dropped by `document.open()` (not superseded by a later script navigation, which
                // owns the mark): the document's next navigation fires `beforeunload` itself.
                // Superseded by a newer navigation (`navigation_started`), the mark is that one's;
                // stopped (`document.open()` also takes a number), it is nobody's.
                let own = b.doc(page).filter(|p| {
                    let st = p.nav.state.borrow();
                    st.doc_seq == document
                        && p.nav.script_navigation.get() == sequence
                        && (st.stops != stops
                            || started
                                .is_none_or(|n| p.main_frame.page_dom.as_ref().is_none_or(|d| d.navigation_started() == n)))
                });
                if let Some(dom) = own.and_then(|p| p.main_frame.page_dom.clone()) {
                    dom.take_before_unload_fired();
                }
                return;
            }
            // HTML "reload" of a lazy iframe still on its deferred initial about:blank: the lazy
            // load resumption steps are kept, so the check's navigation to `src` still happens
            // (WPT iframe-loading-lazy-reload-location-reload, -navigation-reload).
            if history == HistoryHandling::Reload
                && b.page(page.page).is_some_and(|p| p.frames.lazy_frames.borrow().iter().any(|(k, _)| *k == page))
            {
                return;
            }
            let opts = NavigateOpts {
                post: None,
                url,
                reason: NavigationReason::Script,
                cdp_request_id: None,
                // The document that navigates (`location`, `window.open`).
                referrer: source_referrer.or_else(|| b.doc(page).map(|p| p.main_frame.url.clone())),
                referrer_policy: None,
            };
            let nav = b.doc(page).map(|p| p.nav.clone());
            if let Some(nav) = &nav {
                nav.lazy_cancel_limit.set(Some(attributes_queued));
            }
            let result = b.navigate_frame_with_handling(page, opts, history);
            if let Some(nav) = &nav {
                nav.lazy_cancel_limit.set(None);
            }
            if let Err(e) = result {
                b.report_navigation_error(page, &e);
            }
        });
    }

    /// The frame is about to leave its document for another one: a script
    /// navigation is queued (its fetch, once started, is the frame's
    /// pending loader). Not for the frame's initial about:blank document.
    /// A link, form or `window.open` navigation of this frame to another document is queued
    /// ([`PageNav::leaving_document`]); `leaving_navigation_ran` when its task starts.
    pub(crate) fn queue_leaving_navigation(&self) {
        self.queued_navigations.set(self.queued_navigations.get().saturating_add(1));
    }

    pub(crate) fn leaving_navigation_ran(&self) {
        self.queued_navigations.set(self.queued_navigations.get().saturating_sub(1));
    }

    pub(crate) fn leaving_document(&self) -> bool {
        self.queued_navigations.get() > 0 && !self.is_first_blank()
    }

    /// The container's referrer policy snapshotted for the current document
    /// (`None`: its navigation was not one the container started).
    pub(crate) fn committed_container_policy(&self) -> Option<Option<pyro_net::ReferrerPolicy>> {
        self.container_policy.get()
    }

    /// The current document's ancestor origins list.
    pub(crate) fn ancestor_origins(&self) -> Vec<String> {
        self.ancestor_origins.borrow().clone()
    }

    pub(crate) fn set_ancestor_origins(&self, list: Vec<String>) {
        *self.ancestor_origins.borrow_mut() = list;
    }

    /// Records the container's referrer policy as the frame's navigation
    /// begins (its `location.ancestorOrigins` masking).
    pub(crate) fn snapshot_container_policy(&self, policy: Option<pyro_net::ReferrerPolicy>) {
        self.container_policy_pending.set(Some(policy));
    }

    /// A traversal to another document's entry ended without a document
    /// committing (its load failed, was a download or a 204): the frame's
    /// current entry is again the one it left, and the page's current step
    /// that entry's.
    pub(crate) fn abandon_traversal(&self) {
        let Some((index, doc, step)) = self.traversal_from.take() else { return };
        let mut st = self.state.borrow_mut();
        if st.doc_seq != doc || index >= st.entries.len() {
            return;
        }
        st.index = index;
        st.state_version += 1;
        drop(st);
        self.joint.current.set(step);
    }

    /// A traversal of document `doc` to another document's entry has made
    /// that entry the active one, and the document has not changed yet.
    fn traversal_pending(&self, doc: u64) -> bool {
        let st = self.state.borrow();
        st.doc_seq == doc && st.entries.get(st.index).is_some_and(|e| e.doc != doc)
    }

    /// Counts a history update or navigation of the frame: false when it is
    /// to be ignored, past 200 in 10 seconds, as Chrome's
    /// NavigationRateLimiter does (HTML lets user agents limit them).
    fn rate_limit_allows(&self) -> bool {
        if !NAVIGATION_RATE_LIMIT.load(std::sync::atomic::Ordering::Relaxed) {
            return true;
        }
        const LIMIT: u32 = 200;
        const PERIOD: std::time::Duration = std::time::Duration::from_secs(10);
        let (start, count) = self.rate_limit.get();
        let count = count.saturating_add(1);
        if count <= LIMIT {
            self.rate_limit.set((start, count));
            return true;
        }
        let now = Instant::now();
        if now.saturating_duration_since(start) > PERIOD {
            self.rate_limit.set((now, 1));
            return true;
        }
        self.rate_limit.set((start, count));
        false
    }

    /// Whether the active document is fully active (HTML): its frame not
    /// removed, the document not navigated away from. A navigation state
    /// never bound to a document counts as active.
    pub(crate) fn fully_active(&self) -> bool {
        let page = self.page_dom.borrow();
        match page.upgrade() {
            Some(page) => page.is_fully_active(),
            None => page.ptr_eq(&std::rc::Weak::new()),
        }
    }

    /// A traversal of this frame alone by `delta` of its own entries
    /// (`navigation.traverseTo()`; 0 reloads).
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
        let joint = self.joint.clone();
        self.sched.post(TaskSource::Navigation, move |b| {
            let step = entry.step;
            if apply_traversal(b, page, &frame_id, index, entry, current_doc, joint.current.get()) {
                joint.current.set(step);
                // A `history.go()` asked for before this applied loses (first wins).
                joint.traversals.set(joint.traversals.get() + 1);
            }
        });
    }

    /// `history.go(delta)` (and back, forward) on any frame of the page: HTML
    /// "traverse the history by a delta" over the page's joint session
    /// history. Every frame whose entry at the target step differs from its
    /// current one traverses to it, in a task.
    fn traverse_joint(&self, delta: i64) {
        if delta == 0 {
            self.traverse(0);
            return;
        }
        let joint = self.joint.clone();
        let from = joint.current.get();
        let target = {
            let members = joint.members();
            let steps = JointHistory::steps(&members);
            // The current step, or the last one before it (a step only a
            // removed frame had).
            steps.iter().rposition(|s| *s <= from).and_then(|p| {
                let t = p as i128 + i128::from(delta);
                (t >= 0 && t < steps.len() as i128).then(|| steps[t as usize])
            })
        };
        let Some(target) = target else { return };
        let traversals = joint.traversals.get();
        let source = (self.page, self.frame_id.clone());
        self.sched.post(TaskSource::Navigation, move |b| {
            // The target was computed when the traversal was asked for (HTML: from the original
            // placement): a same-document navigation since does not cancel it, as long as the
            // target step is still in the history (a push may have pruned it). A traversal
            // applied since, or one still loading another document, wins: of `go(-3); go(-2)`
            // (or `back(); forward()`) in one task the first wins, as in Chrome.
            if joint.traversals.get() != traversals {
                return;
            }
            let members = joint.members();
            if members.iter().any(|m| m.traversal_pending(m.state.borrow().doc_seq)) {
                return;
            }
            if !JointHistory::steps(&members).contains(&target) {
                return;
            }
            let changes: Vec<_> = members
                .into_iter()
                .filter_map(|nav| {
                    let change = {
                        let st = nav.state.borrow();
                        st.entries
                            .iter()
                            .rposition(|e| e.step <= target)
                            .filter(|i| *i != st.index)
                            .map(|i| (i, st.entries[i].clone(), st.doc_seq))
                    };
                    change.map(|change| (nav, change))
                })
                .collect();
            // HTML "apply the traverse history step": an initiator not allowed by sandboxing to
            // navigate a frame the traversal changes aborts it (a sandboxed iframe's
            // `history.back()` cannot traverse its parent: WPT the-iframe-element/
            // iframe_sandbox_navigate_history_go_back, _go_forward).
            if let Some(host) = b.page(source.0).map(|p| p.frames.clone()) {
                if let Some(from_key) = host.key(&source.1) {
                    let disallowed = changes.iter().any(|(nav, _)| {
                        host.key(&nav.frame_id).is_some_and(|to| !host.allowed_by_sandboxing(from_key, to, false))
                    });
                    if disallowed {
                        return;
                    }
                }
            }
            // A traversal that takes a frame to another document does not take the place of
            // that frame's planned form navigation that has begun: the joint step stays.
            if changes.iter().any(|(nav, (_, entry, current_doc))| {
                entry.doc != *current_doc && planned_navigation_begun(b, nav.page, &nav.frame_id)
            }) {
                return;
            }
            let prev = joint.current.replace(target);
            joint.traversals.set(traversals + 1);
            // The document whose `history.go()` this is: the initiator a speculation-rules prefetch
            // must have been made by to answer a navigation it causes (`prefetch`); an iframe's
            // call does not use its parent's prefetch.
            b.prefetch_navigation_source = b
                .page(source.0)
                .and_then(|p| p.frames.key(&source.1))
                .and_then(|key| b.doc(key).map(|d| d.main_frame.url.clone()));
            for (nav, (index, entry, current_doc)) in changes {
                apply_traversal(b, nav.page, &nav.frame_id, index, entry, current_doc, prev);
            }
            b.prefetch_navigation_source = None;
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
        // URL Fragment Text Directives: a same-document navigation activates one when it has a
        // user activation or its initiator is same-origin with the document.
        let activated = self.directive_gesture.take() == Some(true);
        let directives = self.take_fragment_directive(&mut target).filter(|_| !source.cross_origin || activated);
        let kind = if (current == target && !source.cross_origin) || source.replace {
            SameDocumentKind::Replace
        } else {
            SameDocumentKind::Fragment
        };
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
        self.scroll_to_fragment_or_text(&target, directives.as_deref());
        self.navigation_committed(navigation_type);
        self.fire_fragment_popstate();
        self.queue_hashchange(current.as_str(), target.as_str());
        true
    }

    /// URL Fragment Text Directives: a same-document navigation's directive (`:~:text=...`)
    /// leaves its URL, whether or not it may activate. Returns the directives it held.
    fn take_fragment_directive(&self, target: &mut Url) -> Option<String> {
        let (fragment, directives) = target
            .fragment()
            .and_then(pyro_webapi::highlights::split_fragment_directive)
            .map(|(fragment, directives)| (fragment.to_owned(), directives.to_owned()))?;
        target.set_fragment(Some(&fragment));
        Some(directives)
    }

    /// HTML "scroll to the fragment" of a same-document navigation to `target`, which first
    /// invokes its text `directives` (the first matching one is scrolled to, centered, and the
    /// text is painted as `::target-text`); the element the fragment names is the fallback.
    fn scroll_to_fragment_or_text(&self, target: &Url, directives: Option<&str>) {
        if let Some(directives) = directives {
            let page = self.page_dom.borrow().upgrade();
            if page.is_some_and(|page| pyro_webapi::highlights::invoke_text_directives(&page, directives)) {
                return;
            }
        }
        self.scroll_to_fragment(target);
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

    /// `history.scrollRestoration = mode` on the current entry.
    fn set_scroll_manual(&self, manual: bool) {
        let mut st = self.state.borrow_mut();
        let index = st.index;
        if let Some(entry) = st.entries.get_mut(index) {
            entry.scroll_manual = manual;
        }
    }

    /// HTML "restore persisted state" of a same-document traversal to `entry`: with
    /// `history.scrollRestoration` `auto`, its persisted scroll position; else, without one,
    /// the scroll to its fragment.
    fn restore_scroll(&self, entry: &Entry) {
        let (manual, saved) = {
            let st = self.state.borrow();
            (entry.scroll_manual, st.scroll_positions.get(&entry.id).copied())
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

    /// HTML "process scroll behavior" of a push, replace or reload the Navigation API
    /// intercepted (as Chrome does it): the scroll to `target`'s fragment, or to the top of the
    /// document when the URL has none or no element is indicated.
    fn scroll_to_fragment_or_top(&self, target: &Url) {
        if let Some(page) = self.page_dom.borrow().upgrade() {
            pyro_webapi::layout_api::scroll_to_fragment_or_top(&page, target.fragment().unwrap_or(""));
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
        if self.navigation_hooks.borrow().is_empty() || self.navigation_api_call.get() || self.navigation_disabled() {
            return NavigateDecision::Proceed;
        }
        let Some(page) = self.page_dom.borrow().upgrade() else { return NavigateDecision::Proceed };
        if page.is_disposed() || page.dom.try_borrow_mut().is_err() {
            return NavigateDecision::Proceed;
        }
        let (current, target) = (Url::parse(&self.url()).ok(), Url::parse(url).ok());
        // Whether a traversal to the entry `key` stays in the current document.
        let traversal_stays = key.is_some_and(|k| {
            let st = self.state.borrow();
            st.entries.iter().any(|e| e.key == k && e.doc == st.doc_seq)
        });
        // `destination.sameDocument` of a push or replace: the history API, or a fragment
        // navigation (a traversal's comes from its entry).
        let same_document = match (&current, &target) {
            (Some(c), Some(t)) => source.history_api || (t.fragment().is_some() && same_document_except_fragment(c, t)),
            _ => source.history_api,
        };
        let (hash_change, can_intercept) = match (&current, &target) {
            (Some(c), Some(t)) => (
                // A traversal is a hash change whenever only the fragment differs (a back to the
                // URL without one included), a push or replace only when the target has one.
                !source.history_api
                    && c.fragment() != t.fragment()
                    && same_document_except_fragment(c, t)
                    && if navigation_type == "traverse" { traversal_stays } else { t.fragment().is_some() },
                // A traversal to another document cannot be intercepted (a same-document one of a
                // child frame can, though its event is not cancelable).
                can_rewrite_url(c, t) && (navigation_type != "traverse" || traversal_stays),
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
                s.string(&key.map(|k| uuid_of(self.salt, k)).unwrap_or_default()),
                form_data,
                element,
                s.boolean(source.user_initiated),
                s.boolean(same_document),
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
        if self.navigation_hooks.borrow().is_empty() || self.navigation_api_call.get() || self.navigation_disabled() {
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

/// Whether the frame's planned form navigation has begun and is still its loading navigation.
fn planned_navigation_begun_at(b: &Browser, page: crate::frames::FrameKey) -> bool {
    b.doc(page).is_some_and(|p| p.pending_planned.is_some() && p.pending_planned.as_ref() == p.pending_loader())
}

/// [`planned_navigation_begun_at`] for a frame named by its page and id.
fn planned_navigation_begun(b: &Browser, page: PageId, frame_id: &FrameId) -> bool {
    b.page(page).and_then(|p| p.frames.key(frame_id)).is_some_and(|key| planned_navigation_begun_at(b, key))
}

/// Makes entry `index` (`entry`) of the frame `frame_id` of `page` its
/// current one, as the traversal task of document `current_doc` asked:
/// same-document at once (`popstate`), otherwise by loading it. False when
/// the frame or its history changed since, or a same-document traversal was
/// canceled.
fn apply_traversal(
    b: &mut Browser,
    page: PageId,
    frame_id: &FrameId,
    index: usize,
    entry: Entry,
    current_doc: u64,
    prev_step: u64,
) -> bool {
    let Some(page) = b.page(page).and_then(|p| p.frames.key(frame_id)) else { return false };
    let Some(nav) = b.doc(page).map(|p| p.nav.clone()) else { return false };
    // A traversal to another document does not take the place of
    // the frame's planned form navigation that has begun.
    if entry.doc != current_doc && planned_navigation_begun_at(b, page) {
        return false;
    }
    // While a traversal to another document is under way, later traversals
    // are ignored, as in Chrome (the active entry already belongs to the next
    // document).
    if nav.traversal_pending(current_doc) {
        return false;
    }
    let mut intercepted = false;
    if nav.state.borrow().doc_seq == current_doc && nav.state.borrow().entries.get(index) == Some(&entry) {
        // Only a same-document traversal can be canceled or intercepted.
        let same = entry.doc == current_doc;
        let source = NavigateSource::default();
        // HTML: a traversal's `navigate` event is cancelable only in a top-level traversable.
        let cancelable = same && page.child.is_none();
        match nav.navigate_event_to("traverse", &entry.url, cancelable, Some(entry.key), &source) {
            NavigateDecision::Cancel if same => return false,
            NavigateDecision::Intercept => intercepted = same,
            _ => {}
        }
    }
    nav.persist_scroll();
    {
        let mut state = nav.state.borrow_mut();
        // Navigation can replace the document, or pushState can replace a
        // forward entry, while this task waits. Never apply an obsolete entry
        // or write an index that is no longer in the history list.
        if state.doc_seq != current_doc || state.entries.get(index) != Some(&entry) {
            return false;
        }
        if state.entries.get(state.index).is_some_and(|e| e.doc != current_doc) {
            return false;
        }
        if entry.doc != current_doc {
            nav.traversal_from.set(Some((state.index, current_doc, prev_step)));
        }
        state.index = index;
        state.state_version += 1;
    }
    if entry.doc == current_doc {
        let old_url = nav.url();
        nav.set_url_sync(&entry.url);
        b.same_document_committed(page, entry.url.clone(), SameDocumentKind::Push);
        nav.navigation_committed("traverse");
        // HTML "update document for history step application": popstate fires, then the
        // persisted scroll position is restored, then hashchange (queued) fires. A traversal
        // the Navigation API intercepted suppresses that restore: navigation.js restores after
        // its handlers ("process scroll behavior", `scroll: "after-transition"`) or on
        // `event.scroll()`.
        b.fire_popstate(page, entry.state.is_some(), &old_url, &entry.url);
        if !intercepted {
            nav.restore_scroll(&entry);
        }
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
            nav.abandon_traversal();
            b.report_navigation_error(page, &e);
        }
    }
    true
}

/// Parses `href` against the document's base URL.
fn parse_str(nav: &PageNav, href: &str) -> JsResult<Url> {
    let base_url = nav
        .document
        .borrow()
        .as_ref()
        .and_then(|(dom, doc)| dom.borrow().document_base_url(*doc).ok().map(str::to_owned))
        .unwrap_or_else(|| nav.url());
    let base = Url::parse(&base_url).ok();
    Url::options()
        .base_url(base.as_ref())
        .parse(href)
        .map_err(|_| JsError::dom("SyntaxError", format!("Failed to resolve URL '{href}'")))
}

/// HTML "encoding-parse a URL" for Location (`href`, `assign()`, `replace()`), relative to the
/// entry settings object: against the document of the script doing it (`pyro_js::entry_realm`;
/// the Location's own document when there is none), with the query in that document's encoding
/// (`?ÿ` is `?%FF` from a windows-1252 document, an unmappable character an escaped `&#...;`).
fn parse_location(nav: &PageNav, input: Value<'_>) -> JsResult<Url> {
    let href = input.to_string()?;
    let entry = pyro_js::entry_realm()
        .and_then(|realm| realm.scope().ok().and_then(|scope| pyro_webapi::target::page_of(&scope).ok()));
    let resolved = match entry {
        Some(page) => Some(pyro_webapi::html::resolve_url(&page.dom.borrow(), page.document(), &href)),
        None => {
            nav.document.borrow().as_ref().map(|(dom, doc)| pyro_webapi::html::resolve_url(&dom.borrow(), *doc, &href))
        }
    };
    match resolved.and_then(|resolved| Url::parse(&resolved).ok()) {
        Some(url) => Ok(url),
        // No document to resolve against, or one without a base URL (its relative href came back
        // as it was): against the Location's document's base, else the navigable's URL.
        None => parse_str(nav, &href),
    }
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
/// `s`: the calling realm, whose document is the navigation's initiator.
/// The document of HTML's incumbent settings object for a call into a Location member, as far as
/// JavaScriptCore lets it be told: the entry realm (`pyro_js::entry_realm`, the script that led to
/// the call), unless the innermost page script on the stack belongs to the Location's own document
/// and not to the entry's: then a function of that document, called from the entry realm, is the
/// caller (a parent calling its child frame's `go()`, which sets `location.href`: WPT
/// navigate-child-function). Scripts are told apart by their URL, the document's for an inline
/// script; when both documents have that URL, or the Location's is an `about:` document
/// (about:srcdoc, about:blank: its scripts' URL tells nothing, and its referrer would be its
/// container's anyway), the entry realm stays.
fn incumbent_page(s: &Scope<'_>) -> Option<Rc<pyro_webapi::PageDom>> {
    let own = pyro_webapi::target::page_of(s).ok();
    let entry = pyro_js::entry_realm()
        .and_then(|realm| realm.scope().ok().and_then(|scope| pyro_webapi::target::page_of(&scope).ok()));
    let (Some(own), Some(entry)) = (own.clone(), entry.clone()) else { return entry.or(own) };
    if Rc::ptr_eq(&own, &entry) {
        return Some(entry);
    }
    let document_url = |page: &pyro_webapi::PageDom| {
        let dom = page.dom.borrow();
        dom.document_data(page.document()).ok().map(|d| d.url.split('#').next().unwrap_or_default().to_owned())
    };
    let script = pyro_webapi::csp::running_script_location(s).map(|(url, ..)| url);
    let script = script.as_deref().map(|url| url.split('#').next().unwrap_or_default());
    let is_own = script.is_some_and(|url| {
        !url.starts_with("about:")
            && document_url(&own).as_deref() == Some(url)
            && document_url(&entry).as_deref() != Some(url)
    });
    if is_own {
        return Some(own);
    }
    // A third document of the frame tree, whose function runs: neither the Location's nor the
    // entry's, but the one whose URL the running script has (an inline script has its
    // document's), as in WPT multiple-globals (the incumbent is a sibling of the relevant frame).
    if let Some(url) = script.filter(|url| !url.starts_with("about:") && document_url(&entry).as_deref() != Some(url)) {
        let tree = s.global_data::<crate::Window>().and_then(|w| Some((w.frame_host()?, w.frame_key()?)));
        if let Some((host, key)) = tree {
            for realm in host.sibling_realms(key, s.realm_id()) {
                let Some(page) = realm.scope().ok().and_then(|scope| pyro_webapi::target::page_of(&scope).ok()) else {
                    continue;
                };
                if !Rc::ptr_eq(&page, &own) && !page.is_disposed() && document_url(&page).as_deref() == Some(url) {
                    return Some(page);
                }
            }
        }
    }
    Some(entry)
}

fn assign(s: &Scope<'_>, nav: &PageNav, input: Value<'_>, replace: bool) -> JsResult<()> {
    // HTML: a Location whose relevant Document is gone (its browsing context removed) does
    // nothing, before the URL is even parsed.
    if !nav.fully_active() {
        return Ok(());
    }
    let target = parse_location(nav, input)?;
    // HTML "navigate": a navigable whose active document's unload counter is above 0 (its
    // `beforeunload`, `pagehide` or `unload` listeners are running) ignores the navigation, so
    // a `beforeunload` listener cannot start one of its own (WPT navigation-within-beforeunload).
    if nav.page_dom.borrow().upgrade().is_some_and(|page| page.in_unload()) {
        return Ok(());
    }
    if target.scheme() == "javascript" {
        // Located at the calling script, for a blocked URL's violation. The initiator is the
        // incumbent document (a parent assigning its child's `location.href` is checked against
        // the parent's policies, not the child's), as for the referrer below.
        let initiator = incumbent_page(s).map(|page| pyro_webapi::csp::NavigationInitiator::of(&page).at(s));
        nav.run_javascript_url(target.as_str(), initiator);
        return Ok(());
    }
    // HTML "Location-object navigate": a document that has not completely
    // loaded, with no user gesture, replaces its entry.
    let loading = nav.page_dom.borrow().upgrade().is_some_and(|page| page.navigates_by_replacing());
    // HTML "Location-object navigate": the source document is the incumbent's (the calling
    // script's document), whose URL is the request's referrer: a parent navigating its child
    // frame sends the parent's URL, not the child's (initial about:blank) one.
    let source_referrer = incumbent_page(s).and_then(|page| {
        let dom = page.dom.borrow();
        dom.document_data(page.document()).ok().map(|d| d.url.to_string())
    });
    if nav.rate_limit_allows() {
        navigate_to_from(nav, target, replace || loading, source_referrer, true);
    }
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
    if target.scheme() != "javascript" && nav.rate_limit_allows() {
        // A text directive of a cross-origin navigation needs a user activation.
        navigate_to_from(nav, target, replace, None, false);
    }
    Ok(())
}

/// HTML "navigate" from script (Location, `navigation.navigate`): a
/// fragment change is same-document, anything else loads. Returns whether
/// it stayed in the document.
fn navigate_to(nav: &PageNav, target: Url, replace: bool) -> bool {
    navigate_to_from(nav, target, replace, None, true)
}

/// Whether the script now running, the initiator of the navigation it asks for, has a user
/// activation (URL Fragment Text Directives).
fn initiator_activated() -> bool {
    pyro_js::entry_realm()
        .and_then(|realm| realm.scope().ok().and_then(|scope| pyro_webapi::target::page_of(&scope).ok()))
        .is_some_and(|page| page.window_open_user_gesture())
}

/// `navigate_to` with the source document's URL as the referrer of a loading navigation.
/// `same_origin`: the initiator is same-origin with the document, which lets a same-document
/// navigation invoke a text directive without a user activation.
fn navigate_to_from(nav: &PageNav, mut target: Url, replace: bool, source_referrer: Option<String>, same_origin: bool) -> bool {
    let current = Url::parse(&nav.url()).ok();
    let activated = initiator_activated();
    let mut directives = None;
    if current.as_ref().is_some_and(|current| same_document_except_fragment(current, &target)) {
        directives = nav.take_fragment_directive(&mut target).filter(|_| same_origin || activated);
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
            nav.scroll_to_fragment_or_text(&target, directives.as_deref());
            nav.navigation_committed(navigation_type);
            nav.fire_fragment_popstate();
            nav.queue_hashchange(cur.as_str(), target.as_str());
            true
        }
        _ => {
            // A client redirect while the document loads keeps the activation the navigation
            // that made the document had (Chrome: a text directive per user gesture).
            let loading = nav.page_dom.borrow().upgrade().is_some_and(|page| !page.is_completely_loaded());
            nav.directive_gesture.set(Some(activated || (loading && nav.directive_gesture.get() == Some(true))));
            nav.navigate_from(
                target.to_string(),
                if replace { HistoryHandling::Replace } else { HistoryHandling::Push },
                source_referrer,
            );
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
    if !nav.fully_active() {
        return Ok(());
    }
    let mut url = Url::parse(&nav.url()).map_err(|_| JsError::Type("invalid current URL".into()))?;
    if change(&mut url)? {
        assign(s, nav, s.string(url.as_str()), false)?;
    }
    Ok(())
}

/// `location.ancestorOrigins`: the origins of the frame's ancestors,
/// parent first, while the Location's document is the frame's fully active
/// one; empty once it is not (the frame removed, or navigated away). The
/// same list on every read until then, a new empty one after. Chrome's list
/// is a DOMStringList (an array here: the length, the indices, iteration).
fn ancestor_origins<'s>(s: &Scope<'s>, nav: &Rc<PageNav>, this: pyro_js::Object<'s>) -> JsResult<Value<'s>> {
    let window = s.global_data::<crate::Window>();
    let active = window.is_some_and(|w| {
        let page = pyro_webapi::WindowHost::page(w);
        page.is_fully_active() && nav.page_dom.borrow().upgrade().is_some_and(|p| Rc::ptr_eq(&p, page))
    });
    let key = if active { "lpAncestorOrigins" } else { "lpAncestorOriginsInactive" };
    if let Some(list) = s.get_hidden(this, key)? {
        return Ok(list);
    }
    // The list the document got when it committed (`FrameHost::store_ancestor_origins`).
    let stored = if active { nav.ancestor_origins.borrow().clone() } else { Vec::new() };
    let origins: Vec<Value<'s>> = stored.iter().map(|o| s.string(o)).collect();
    let list = s.array(&origins)?.value();
    s.set_hidden(this, key, list)?;
    Ok(list)
}

/// Whether the `referrerpolicy` of a frame's container (as snapshotted when
/// the navigation began) hides its embedder (of origin `embedder`) from the
/// frame's document (of origin `origin`) in `location.ancestorOrigins`:
/// `no-referrer` always, `same-origin` across origins (WPT
/// location-ancestor-origins*). Other policies hide nothing.
pub(crate) fn masks_ancestor_origin(policy: pyro_net::ReferrerPolicy, embedder: &str, origin: &str) -> bool {
    use pyro_net::ReferrerPolicy as P;
    match policy {
        P::NoReferrer => true,
        P::SameOrigin => origin == "null" || embedder != origin,
        _ => false,
    }
}

pub struct Location;

impl JsClass for Location {
    const NAME: &'static str = "Location";
    type Data = Rc<PageNav>;
    type Parent = NoParent;

    fn define(def: &mut ClassDef<Self>) {
        // HTML: Location's [[SetPrototypeOf]] is SetImmutablePrototype and its [[PreventExtensions]]
        // fails (WPT the-location-interface/location-prototype-setting-same-origin.html).
        def.immutable_prototype();
        def.accessor("href", |s, n| Ok(s.string(&n.url())), |s, n, v| assign(s, n, v, false));
        def.pure_method("toString", 0, |s, n, _a| Ok(s.string(&n.url())));
        def.method("assign", 1, |s, n, a| {
            a.require(1, "assign")?;
            assign(s, n, a.get(0), false)?;
            Ok(s.undefined())
        });
        def.method("replace", 1, |s, n, a| {
            a.require(1, "replace")?;
            assign(s, n, a.get(0), true)?;
            Ok(s.undefined())
        });
        def.method("reload", 0, |s, n, _a| {
            if !n.fully_active() {
                return Ok(s.undefined());
            }
            let url = n.url();
            match n.navigate_event("reload", &url, true) {
                NavigateDecision::Cancel => {}
                NavigateDecision::Intercept => n.commit_intercepted(&url, "reload"),
                NavigateDecision::Proceed => n.navigate(url, HistoryHandling::Reload),
            }
            Ok(s.undefined())
        });
        def.getter_this("ancestorOrigins", ancestor_origins);
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
                let h = v.to_string()?;
                if !n.fully_active() {
                    return Ok(());
                }
                let mut u = Url::parse(&n.url()).map_err(|_| JsError::Type("invalid current URL".into()))?;
                let current = u.fragment().unwrap_or("").to_owned();
                u.set_fragment(Some(h.strip_prefix('#').unwrap_or(&h)));
                // The same fragment (no fragment counts as an empty one, as
                // in Chrome) navigates nowhere: no event, no history entry.
                if u.fragment() == Some(current.as_str()) {
                    return Ok(());
                }
                assign(s, n, s.string(u.as_str()), false)
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
        def.getter("length", |s, n| {
            require_fully_active(n, "length")?;
            Ok(s.number(n.history_length() as f64))
        });
        def.getter("state", |s, n| {
            require_fully_active(n, "state")?;
            PageNav::state_value(n, s)
        });
        def.accessor(
            "scrollRestoration",
            |s, n| {
                require_fully_active(n, "scrollRestoration")?;
                let st = n.state.borrow();
                let manual = st.entries.get(st.index).is_some_and(|e| e.scroll_manual);
                Ok(s.string(if manual { "manual" } else { "auto" }))
            },
            |_s, n, v| {
                require_fully_active(n, "scrollRestoration")?;
                match v.to_string()?.as_str() {
                    "manual" => n.set_scroll_manual(true),
                    "auto" => n.set_scroll_manual(false),
                    _ => {} // invalid enum values are ignored
                }
                Ok(())
            },
        );
        def.method("pushState", 2, |s, n, a| change_state(s, n, a, SameDocumentKind::Push));
        def.method("replaceState", 2, |s, n, a| change_state(s, n, a, SameDocumentKind::Replace));
        def.method("back", 0, |s, n, _a| {
            require_fully_active(n, "back")?;
            if !in_unload(s) && n.rate_limit_allows() {
                n.traverse_joint(-1);
            }
            Ok(s.undefined())
        });
        def.method("forward", 0, |s, n, _a| {
            require_fully_active(n, "forward")?;
            if !in_unload(s) && n.rate_limit_allows() {
                n.traverse_joint(1);
            }
            Ok(s.undefined())
        });
        def.method("go", 0, |s, n, a| {
            let d = a.get(0).to_number()?;
            require_fully_active(n, "go")?;
            if !in_unload(s) && n.rate_limit_allows() {
                n.traverse_joint(if d.is_finite() { d.trunc() as i64 } else { 0 });
            }
            Ok(s.undefined())
        });
    }
}

static NAVIGATION_RATE_LIMIT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Whether history updates and navigations past 200 in 10 seconds are
/// ignored, as stock Chrome does (on by default; `serve
/// --navigation-rate-limit off`, as Playwright's Chromium runs with
/// `--disable-ipc-flooding-protection`). For the whole process.
pub fn set_navigation_rate_limit(on: bool) {
    NAVIGATION_RATE_LIMIT.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// HTML "traverse the history by a delta" from a document whose unload counter is above 0 (its
/// `beforeunload`, `pagehide` or `unload` listeners run) does nothing: the navigation that is
/// unloading it wins (WPT the-history-interface/traverse-during-beforeunload, -unload).
fn in_unload(s: &Scope<'_>) -> bool {
    s.global_data::<crate::Window>().is_some_and(|w| pyro_webapi::WindowHost::page(w).in_unload())
}

/// HTML: History's methods throw a SecurityError for a document that is not
/// fully active (checked before the rate limit).
fn require_fully_active(n: &PageNav, method: &str) -> JsResult<()> {
    if n.fully_active() {
        return Ok(());
    }
    Err(JsError::dom(
        "SecurityError",
        format!(
            "Failed to execute '{method}' on 'History': May not use a History object associated with a Document that is not fully active"
        ),
    ))
}

fn change_state<'s>(s: &Scope<'s>, n: &PageNav, a: pyro_js::Args<'s>, kind: SameDocumentKind) -> JsResult<Value<'s>> {
    let method = if kind == SameDocumentKind::Push { "pushState" } else { "replaceState" };
    a.require(2, method)?;
    require_fully_active(n, method)?;
    // StructuredSerializeForStorage: functions, DOM nodes and ports throw
    // DataCloneError; Date, Map, Set, typed arrays, cycles, Blob, ... survive.
    let state = match a.get(0) {
        v if v.is_nullish() => None,
        v => Some(HistoryState(Rc::new(pyro_webapi::serialize_for_storage(s, v)?))),
    };
    // HTML (whatwg/html#9343): a null or empty-string URL keeps the
    // document's URL, fragment included, whatever the base URL is.
    let href = match a.opt(2).filter(|v| !v.is_nullish()) {
        Some(v) => Some(v.to_string()?).filter(|h| !h.is_empty()),
        None => None,
    };
    let url = match href {
        Some(href) => {
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
            let target = parse_str(n, &href).map_err(|_| rejected(&href))?;
            if !current.as_ref().is_some_and(|c| can_rewrite_url(c, &target)) {
                return Err(rejected(target.as_str()));
            }
            target.to_string()
        }
        None => n.url(),
    };
    // After the state, the URL and whether it may be set were checked.
    if !n.rate_limit_allows() {
        return Ok(s.undefined());
    }
    let navigation_type = if kind == SameDocumentKind::Push { "push" } else { "replace" };
    let source = NavigateSource { history_api: true, ..Default::default() };
    if n.navigate_event_to(navigation_type, &url, true, None, &source) == NavigateDecision::Cancel {
        return Ok(s.undefined());
    }
    n.same_document(&url, state, kind);
    // The Navigation API state is separate from `history.state`: the entry a pushState or
    // replaceState makes has none (`entry.getState()` is undefined), whatever the entry had.
    n.set_navigation_state(None);
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
    if entry_type.is_none_or(|t| t == "paint") {
        use pyro_webapi::perf::timeline::{wrap_paint, PaintRecord};
        // With the paint module the entries were reported from frames; without it
        // both come from the first rendering opportunity (no presentation time).
        let records: Vec<PaintRecord> = if pyro_webapi::perf::paint::ENABLED {
            p.target.page.timeline().paint_records()
        } else {
            p.nav
                .first_paint()
                .into_iter()
                .flat_map(|start| {
                    ["first-paint", "first-contentful-paint"].map(|name| PaintRecord {
                        name,
                        start,
                        paint_time: start,
                        presentation_time: None,
                    })
                })
                .collect()
        };
        for (slot, record) in records.iter().enumerate() {
            if name.is_none_or(|n| n == record.name) {
                entries.push((record.start, wrap_paint(s, slot as u64, record)?));
            }
        }
    }
    p.user.collect(s, name, entry_type, &mut entries);
    // Event Timing's `first-input` entry and the long animation frames are on the timeline (the
    // other types of `perf/` are for observers only).
    if pyro_webapi::perf::event::ENABLED && entry_type.is_none_or(|t| t == "first-input") {
        entries.extend(pyro_webapi::perf::event::first_input_entries(s, &p.target.page, name)?);
    }
    // Long Animation Frames: the frames are on the timeline too.
    if pyro_webapi::perf::loaf::ENABLED && entry_type.is_none_or(|t| t == "long-animation-frame") {
        entries.extend(pyro_webapi::perf::loaf::timeline_entries(s, &p.target.page, name)?);
    }
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
        unload_start: nav.unload_times.get().0,
        unload_end: nav.unload_times.get().1,
        fetch: nav.fetch_timing.borrow().clone(),
    }
}

/// The document's `navigation` entry. Chrome adds it to the timeline as the
/// document starts loading and fills it in as it goes; an `about:` document
/// (the initial one, `setContent`) has none.
fn navigation_entry(p: &PerformanceData) -> Option<PerformanceEntryData> {
    let created = p.nav.creation_url.borrow().clone();
    let url = if created.is_empty() { p.nav.url() } else { created };
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
        "unloadEventStart" => at(n.unload_start),
        "unloadEventEnd" => at(n.unload_end),
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
        // NavigationTimingType constants, on the interface object and the prototype.
        def.constant("TYPE_NAVIGATE", 0.0);
        def.constant("TYPE_RELOAD", 1.0);
        def.constant("TYPE_BACK_FORWARD", 2.0);
        def.constant("TYPE_RESERVED", 255.0);
        def.getter("type", |s, n| Ok(s.number(legacy_navigation_type(n))));
        def.getter("redirectCount", |s, n| Ok(s.number(legacy_redirect_count(n))));
        def.method("toJSON", 0, |s, n, _a| {
            let o = s.object();
            o.set("type", s.number(legacy_navigation_type(n)))?;
            o.set("redirectCount", s.number(legacy_redirect_count(n)))?;
            Ok(o.value())
        });
    }
}

/// `performance.navigation.redirectCount`: the same-origin redirects the document's
/// fetch followed (the fetch timing already hides redirects through other origins).
fn legacy_redirect_count(n: &PageNav) -> f64 {
    n.fetch_timing.borrow().as_ref().map_or(0.0, |f| f64::from(f.redirect_count))
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
        // Event Timing's counters exist once the module is real.
        if pyro_webapi::perf::event::ENABLED {
            def.getter("eventCounts", |s, p| pyro_webapi::perf::event::event_counts(s, &p.target.page));
            def.getter("interactionCount", |s, p| Ok(s.number(f64::from(pyro_webapi::perf::event::interaction_count(&p.target.page)))));
        }
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
    // `perf/`: the base types and those of the modules that are really implemented.
    perf::observer_module(s, performance, &page, watch, &pyro_webapi::perf::supported_entry_types())
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
    // disabled() -> HTML "has entries and events disabled" (initial about:blank, opaque origin);
    // opaque() -> the origin is opaque; initialBlank() -> the initial about:blank document;
    // inUnload() -> the document's unload counter is above 0 (beforeunload, pagehide, unload).
    native!("disabled", |s, nav, _a| Ok(s.boolean(nav.navigation_disabled())));
    native!("opaque", |s, nav, _a| Ok(s.boolean(nav.has_opaque_origin())));
    native!("initialBlank", |s, nav, _a| Ok(s.boolean(nav.is_initial_blank())));
    native!("inUnload", |s, nav, _a| {
        let unloading = nav.page_dom.borrow().upgrade().is_some_and(|page| page.in_unload());
        Ok(s.boolean(unloading))
    });
    // entries() -> [index, [key, id, url, sameDocument, stateJSON]...]: the
    // same-origin entries around the current one (none for an opaque origin).
    native!("entries", |s, nav, _a| {
        if nav.has_opaque_origin() {
            return Ok(s.array(&[s.number(-1.0)])?.value());
        }
        // The live document's origin (an `about:blank` or `srcdoc` document has its creator's).
        let live = nav.page_dom.borrow().upgrade().map(|p| p.origin_key());
        let st = nav.state.borrow();
        let Some(current) = st.entries.get(st.index) else { return Ok(s.array(&[s.number(-1.0)])?.value()) };
        let here = live.clone().or_else(|| Url::parse(&current.url).ok().map(|u| u.origin().ascii_serialization()));
        let here = here.filter(|o| !o.starts_with("opaque") && o != "null");
        // An entry's document origin: what was recorded when it was left (an `about:blank`,
        // `srcdoc` or `blob:` document has its creator's, which its URL does not show); for the
        // entries of the live document its own; else the URL's.
        let origin_of = |e: &Entry| -> Option<String> {
            if e.doc == current.doc {
                return here.clone();
            }
            if let Some(o) = &e.facts.origin {
                return Some(o.clone());
            }
            Url::parse(&e.url).ok().map(|u| u.origin()).filter(|o| o.is_tuple()).map(|o| o.ascii_serialization())
        };
        let same = |e: &Entry| e.doc == current.doc || (here.is_some() && origin_of(e) == here);
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
            // The URL of a no-referrer document's entry is hidden once that document was left.
            let url = if e.facts.no_referrer && e.doc != current.doc { s.null() } else { s.string(&e.url) };
            let row = [s.string(&uuid_of(nav.salt, e.key)), s.string(&uuid_of(nav.salt, e.id)), url, s.boolean(e.doc == current.doc), state];
            values.push(s.array(&row)?.value());
        }
        Ok(s.array(&values)?.value())
    });
    // activation() -> null | [type, entryKey, entryId, entryUrl, from], from being null or
    // [key, id, url, stateJSON]: HTML `navigation.activation` of the document.
    native!("activation", |s, nav, _a| {
        let st = nav.state.borrow();
        let Some(act) = st.activation.as_ref() else { return Ok(s.null()) };
        let from = match &act.from {
            None => s.null(),
            Some((key, id, url, state)) => {
                let state = state.as_deref().map_or(s.undefined(), |j| s.string(j));
                let url = url.as_deref().map_or(s.null(), |u| s.string(u));
                s.array(&[s.string(&uuid_of(nav.salt, *key)), s.string(&uuid_of(nav.salt, *id)), url, state])?.value()
            }
        };
        let row = [
            s.string(act.ty),
            s.string(&uuid_of(nav.salt, act.entry.0)),
            s.string(&uuid_of(nav.salt, act.entry.1)),
            s.string(&act.entry.2),
            from,
        ];
        Ok(s.array(&row)?.value())
    });
    // active() -> whether the document is fully active (its frame not
    // removed, not navigated away): the methods reject InvalidStateError
    // otherwise.
    {
        let with_nav = with_nav.clone();
        let f = s.function("active", move |s, _this, _a| {
            // The frame's current document must be this realm's own: the session history belongs to
            // the frame, so a document navigated away from would otherwise read the next one's.
            let own = pyro_webapi::target::page_of(s).ok();
            let active = with_nav().ok().is_some_and(|nav| {
                nav.page_dom
                    .borrow()
                    .upgrade()
                    .is_some_and(|p| p.is_fully_active() && own.as_ref().is_none_or(|own| Rc::ptr_eq(own, &p)))
            });
            Ok(s.boolean(active))
        })?;
        natives.set_atom("active", f.value())?;
    }
    // resolve(url) -> the absolute URL, or SyntaxError.
    native!("resolve", |s, nav, a| Ok(s.string(parse_str(&nav, &a.get(0).to_string()?)?.as_str())));
    // canRewrite(url) -> HTML's "can have its URL rewritten" for the document's URL and `url`
    // (an absolute URL): what `redirect()` may target.
    native!("canRewrite", |s, nav, a| {
        let target = parse_str(&nav, &a.get(0).to_string()?)?;
        Ok(s.boolean(Url::parse(&nav.url()).is_ok_and(|current| can_rewrite_url(&current, &target))))
    });
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
            st.entries.iter().position(|e| uuid_of(nav.salt, e.key) == key).map(|i| i as i64 - st.index as i64)
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
    // HTML "restore scroll position data" of the current entry (an intercepted traversal's
    // scroll behavior), whatever its `history.scrollRestoration`.
    native!("restoreScroll", |s, nav, _a| {
        let entry = {
            let st = nav.state.borrow();
            st.entries.get(st.index).cloned()
        };
        if let Some(mut entry) = entry {
            entry.scroll_manual = false;
            nav.restore_scroll(&entry);
        }
        Ok(s.undefined())
    });
    // scrollToFragment(url): the scroll an intercepted push, replace or reload does after its
    // handlers (or on `scroll()`): to the URL's fragment, else the top of the document.
    native!("scrollToFragment", |s, nav, a| {
        if let Ok(url) = Url::parse(&a.get(0).to_string()?) {
            nav.scroll_to_fragment_or_top(&url);
        }
        Ok(s.undefined())
    });
    // The viewport's scroll position as "x,y" (has the page scrolled since?).
    native!("viewportScroll", |s, nav, _a| {
        let page = nav.page_dom.borrow().upgrade();
        let (x, y) = page.map(|p| pyro_webapi::layout_api::viewport_scroll(&p)).unwrap_or_default();
        Ok(s.string(&format!("{x},{y}")))
    });
    // dispatchTrusted(target, event): the user agent's own event, `isTrusted` true.
    native!("dispatchTrusted", |s, _nav, a| {
        pyro_webapi::dispatch_trusted_event(s, a.get(0), a.get(1))?;
        Ok(s.undefined())
    });
    // updateState(stateJSON, entryId?): the state of the current entry, or of the entry with that
    // id (the one `updateCurrentEntry()` began with, which its state's getters may have left).
    native!("updateState", |s, nav, a| {
        let state = a.opt(0).filter(|v| !v.is_undefined()).map(|v| v.to_string()).transpose()?;
        match a.opt(1).filter(|v| !v.is_undefined()).map(|v| v.to_string()).transpose()? {
            Some(id) => {
                let mut st = nav.state.borrow_mut();
                if let Some(entry) = st.entries.iter_mut().find(|e| uuid_of(nav.salt, e.id) == id) {
                    entry.nav_state = state;
                }
            }
            None => nav.set_navigation_state(state),
        }
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
