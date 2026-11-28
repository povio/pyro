//! Soft Navigations: a `soft-navigation` entry when a user interaction is followed by a same-document
//! URL change, a DOM modification it caused and a contentful paint of what the modification touched
//! (https://wicg.github.io/soft-navigations/; Chrome's `SoftNavigationHeuristics` is the reference).
//!
//! # Contexts and attribution
//!
//! A trusted `click`, `keydown`, `pointerup` or `mouseup` that is dispatched to a listener opens a
//! *context* (its number is the interaction's attribution). While its listeners run, and until the
//! task that dispatched it (microtask checkpoint included) ends, `pyro_runtime::ambient()` is the
//! context's number; every task, timer, `scheduler.postTask`/`yield` continuation and
//! `requestAnimationFrame` callback posted meanwhile carries it and has it set when it runs
//! (`pyro-runtime`), and so do the tasks they post, which is how `setTimeout(..)`, a `hashchange`
//! or a `popstate` queued by `history.back()`, or a promise continuation that runs in such a task
//! stay attributed. Not carried: network completions (`fetch`, `XMLHttpRequest`), element load
//! events (`<link>`/`<script>`/`<img>` `load`), `MessageChannel`/`postMessage` deliveries and
//! callbacks of observers (their tasks are posted by the embedder from another thread or by
//! queues that do not remember who asked); a DOM change or a URL change made there is not
//! attributed, which Chrome's task attribution would follow.
//!
//! What a context collects while it is current:
//! - a same-document URL change that differs from the document's URL (`pushState`/`replaceState`
//!   with a new URL, a fragment navigation, an intercepted Navigation API navigation, a
//!   traversal): `nav.rs` calls [`url_changed`] (it reads the ambient context and does nothing
//!   without one). Its URL is the latest one; when another context of the document gets its URL
//!   change while this one has not been reported, this one is abandoned (the later URL change wins:
//!   two interactions racing to the same document produce one entry, the later's);
//! - the nodes a mutation touched under its attribution (`pyro_dom::Dom::set_touch_watch`: the node
//!   whose attribute or character data changed, the nodes a child list gained), at most 256 of
//!   them are judged;
//! - a contentful paint: the first frame whose painted candidates (images, text blocks, CSS images,
//!   SVG, canvases: [`scan::painted_candidates_in`]) under those nodes' subtrees include one. A
//!   detached node, a `display: none` one, or text with no box gives none. A text block counts
//!   through the touched text or the touched node that holds it (a modified inline `<span>`: the
//!   candidate is its block container, which holds it).
//!
//! When there is a URL change and a paint (in any order) the context reports its entry: `name` the
//! URL, `startTime` the interaction event's `timeStamp`, `navigationId` new (`TimelineBuffers::
//! allocate_navigation_id`, taken when the interaction began and adopted by the document), `interactionId` the Event Timing id of the interaction, `paintTime` and
//! `presentationTime` those of the largest paint so far (else of the frame that painted),
//! `navigationType` `push`, `replace` or `traverse`. One entry per interaction.
//!
//! # Interaction Contentful Paint
//!
//! Independent of the URL: each frame in which the painted candidates under the touched nodes include
//! one larger (by LCP's size rule, `lcp::size_of`) than every earlier one of the interaction queues an
//! `interaction-contentful-paint` entry holding a `LargestContentfulPaint` object for it
//! (`lcp::make_entry`), until the context ends: ten seconds after it opened, or when it has been
//! judged 300 times. The soft navigation's `getLargestInteractionContentfulPaint()` is the latest of
//! them. A context whose touched nodes are judged and unchanged costs nothing in the frames after.
//!
//! # Idle behaviour and cost
//!
//! Nothing runs for a document nobody interacted with: no hook is entered (`dispatch_begin` runs only
//! for trusted events, `on_frame` returns at its first line without a state), the DOM's touch watch is
//! off (a mutation tests one `Option`), the scheduler adds one thread-local read per task posted and
//! one write per task run. After an interaction, a mutation under an attribution costs a thread-local
//! read and a push (at most 1024 kept between frames); a task that ends under an attribution asks for
//! one rendering update; a frame judges, per context with touched nodes whose layout changed since it
//! was last judged, the subtrees of at most 256 touched nodes, at most [`scan::SUBTREE_NODE_CAP`]
//! nodes (the 60,000 nodes of a large document are never walked for this; the cap bounds a script that
//! appends 60,000 nodes in one interaction), and asks for another frame only while a context with
//! touched nodes has painted nothing yet, 300 judgements at most per context.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use pyro_dom::NodeId;
use pyro_js::{ClassDef, IdentityKey, JsClass, JsResult, Persistent, Scope, Value};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::page::PageDom;
use crate::performance::{PerformanceEntry, PerformanceEntryData};

use super::lcp::{self, LcpEntry};
use super::scan;
use super::timeline::{queue_entry, request_rendering, TimelineEntry};
use super::FrameInfo;

/// Entry types this module makes real.
pub const ENTRY_TYPES: &[&str] = &["soft-navigation", "interaction-contentful-paint"];
/// Flip to true when the module really works: only then are its types in
/// `supportedEntryTypes`, its classes registered and its hooks called.
pub const ENABLED: bool = true;

/// Contexts of one document kept at once; the oldest goes first.
const MAX_CONTEXTS: usize = 32;
/// Touched nodes a context judges.
const MAX_ROOTS: usize = 256;
/// How long a context lives.
const LIFETIME: Duration = Duration::from_secs(10);
/// How many times a context is judged.
const MAX_FRAMES: u32 = 300;
/// How long a context that has no URL change and no touched node lives.
const IDLE_LIFETIME: Duration = Duration::from_secs(2);

/// Registers the module's classes (`JsClass`es); called once per engine when ENABLED.
pub fn register(engine: &pyro_js::Engine) {
    engine.register::<PerformanceSoftNavigation>();
    engine.register::<InteractionContentfulPaint>();
}

static NEXT_CONTEXT: AtomicU64 = AtomicU64::new(1);

/// One user interaction being followed.
struct Context {
    /// The attribution number (`pyro_runtime::ambient`), process-unique, never 0.
    id: u64,
    interaction_id: u32,
    /// The navigation id the interaction's entries carry (`TimelineBuffers::allocate_navigation_id`);
    /// the document's once the soft navigation is detected.
    navigation_id: u64,
    /// The interaction event's `timeStamp`.
    start: f64,
    opened: Instant,
    /// The latest same-document URL change under this context: the URL and its `navigationType`.
    url: Option<(String, &'static str)>,
    touched: Vec<NodeId>,
    /// `(paintTime, presentationTime)` of the first frame that painted what was touched.
    painted: Option<(f64, f64)>,
    /// Frames this context was judged in.
    frames: u32,
    /// The `soft-navigation` entry was queued.
    emitted: bool,
    /// The size of the largest interaction contentful paint so far.
    icp_largest: f64,
    /// The latest interaction contentful paint: what the soft navigation's
    /// `getLargestInteractionContentfulPaint()` returns (shared with its entry).
    largest: LargestIcp,
    /// The layout stamp and touched count of the last judgement that found something painted: the next
    /// one is skipped while neither moved.
    settled: Option<(Stamp, usize)>,
}

type Stamp = ((u64, u64, u32, u32), u64);
type LargestIcp = Rc<RefCell<Option<Rc<IcpEntry>>>>;

struct State {
    page: Weak<PageDom>,
    contexts: Vec<Context>,
    /// A rendering update was asked for and has not run: a task under an attribution asks once.
    frame_pending: bool,
}

thread_local! {
    /// Per document (`PageDom::id`); entries of dropped documents are pruned on access.
    static STATES: RefCell<FxHashMap<u64, State>> = RefCell::new(FxHashMap::default());
}

/// This module's part of [`crate::thread_state::reset_thread`]: the states of dropped documents go;
/// one still held names the static.
pub(super) fn thread_leftover() -> Option<&'static str> {
    STATES
        .with(|states| {
            let mut states = states.borrow_mut();
            states.retain(|_, s| s.page.upgrade().is_some_and(|p| !p.is_disposed()));
            !states.is_empty()
        })
        .then_some("softnav::STATES")
}

/// What [`dispatch_begin`] hands [`dispatch_end`] (and [`activation_begin`] [`activation_end`]).
pub struct DispatchToken {
    context: u64,
    /// The attribution before this one, and whether a task was running: outside a task nothing
    /// ends the attribution but [`dispatch_end`].
    previous: u64,
    in_task: bool,
}

/// A trusted event is about to run its listeners; called when ENABLED. An interaction event opens a
/// context and makes it the ambient attribution until the running task ends.
pub fn dispatch_begin(page: &PageDom, event: &crate::event::EventData) -> Option<DispatchToken> {
    let start = {
        let state = event.state.borrow();
        if !matches!(&*state.ty, "click" | "keydown" | "pointerup" | "mouseup") {
            return None;
        }
        state.time_stamp
    };
    Some(open(page, start))
}

/// The activation behavior of a trusted click is about to run (a link is followed, a form submitted:
/// the navigation it starts descends from the interaction even when no listener saw the click);
/// called when ENABLED.
pub fn activation_begin(page: &PageDom, event: &crate::event::EventData) -> Option<DispatchToken> {
    let start = event.state.borrow().time_stamp;
    Some(open(page, start))
}

/// The context of the interaction whose event has the `timeStamp` `start`: the one a listener's
/// dispatch opened, or a new one. It becomes the ambient attribution.
fn open(page: &PageDom, start: f64) -> DispatchToken {
    let previous = pyro_runtime::ambient();
    let in_task = pyro_runtime::task_depth() > 0;
    let id = STATES.with(|states| {
        let mut states = states.borrow_mut();
        states.retain(|_, s| s.page.upgrade().is_some_and(|p| !p.is_disposed()));
        let state = states.entry(page.id()).or_insert_with(|| State { page: page.weak_self(), contexts: Vec::new(), frame_pending: false });
        // (A context that did nothing in two seconds, as most of those a click's `pointerup` and `mouseup` open,
        // goes.)
        state.contexts.retain(|c| {
            let age = c.opened.elapsed();
            age < LIFETIME && (age < IDLE_LIFETIME || c.url.is_some() || !c.touched.is_empty())
        });
        if let Some(c) = state.contexts.iter().find(|c| c.start == start) {
            return c.id;
        }
        if state.contexts.len() >= MAX_CONTEXTS {
            state.contexts.remove(0);
        }
        let id = NEXT_CONTEXT.fetch_add(1, Ordering::Relaxed);
        state.contexts.push(Context {
            id,
            interaction_id: 0,
            navigation_id: page.timeline().allocate_navigation_id(),
            start,
            opened: Instant::now(),
            url: None,
            touched: Vec::new(),
            painted: None,
            frames: 0,
            emitted: false,
            icp_largest: 0.0,
            largest: Rc::default(),
            settled: None,
        });
        id
    });
    // Mutations attributed to a context are recorded from now on.
    if let Ok(mut dom) = page.dom.try_borrow_mut() {
        if !dom.touch_watch_on() {
            dom.set_touch_watch(Some(pyro_runtime::ambient));
        }
    }
    page.timeline().set_soft_armed(true);
    pyro_runtime::set_ambient(id);
    DispatchToken { context: id, previous, in_task }
}

/// The listeners of the event ran; called when ENABLED. The interaction's Event Timing id is known now.
pub fn dispatch_end(page: &PageDom, _event: &crate::event::EventData, token: DispatchToken) {
    let interaction_id = if super::event::ENABLED { super::event::interaction_count(page) } else { 0 };
    STATES.with(|states| {
        if let Some(c) = states.borrow_mut().get_mut(&page.id()).and_then(|s| s.contexts.iter_mut().find(|c| c.id == token.context)) {
            c.interaction_id = interaction_id;
        }
    });
    activation_end(page, token);
}

/// What the interaction did so far is judged in the next rendering update, and (outside a task, where
/// nothing else would) the attribution ends.
pub fn activation_end(page: &PageDom, token: DispatchToken) {
    if !token.in_task {
        pyro_runtime::set_ambient(token.previous);
    }
    ask_frame(page, token.context);
}

/// Asks for a rendering update to judge `context`, once until it runs.
fn ask_frame(page: &PageDom, context: u64) {
    let ask = STATES.with(|states| {
        let mut states = states.borrow_mut();
        match states.get_mut(&page.id()) {
            Some(state) if !state.frame_pending && state.contexts.iter().any(|c| c.id == context) => {
                state.frame_pending = true;
                true
            }
            _ => false,
        }
    });
    if ask {
        request_rendering(page);
    }
}

/// The document's URL changed without a navigation (`pushState`, `replaceState`, a fragment navigation,
/// a same-document Navigation API navigation or a traversal) from a URL that was different: `kind` is
/// the `navigationType`. A no-op unless the code running is attributed to a context of the document.
pub fn url_changed(page: &Rc<PageDom>, url: &str, kind: &'static str) {
    let id = pyro_runtime::ambient();
    if id == 0 {
        return;
    }
    let known = STATES.with(|states| {
        let mut states = states.borrow_mut();
        let Some(state) = states.get_mut(&page.id()) else { return false };
        let Some(i) = state.contexts.iter().position(|c| c.id == id) else { return false };
        state.contexts[i].url = Some((url.to_owned(), kind));
        // The later URL change wins: an earlier context that had its own and is still unreported
        // is abandoned.
        state.contexts.retain(|c| c.id == id || c.url.is_none() || c.emitted);
        true
    });
    if known {
        request_rendering(page);
    }
}

/// A task that ran under an attribution ended (`Browser::checkpoint`): what it changed is judged in the
/// next rendering update, which it asks for once.
pub fn after_task(page: &Rc<PageDom>) {
    let id = pyro_runtime::ambient();
    if id != 0 && page.timeline().soft_armed() {
        ask_frame(page, id);
    }
}

/// The roots of the subtrees to judge: the touched nodes still in the document, without those
/// inside another one, at most [`MAX_ROOTS`].
fn roots_of(page: &PageDom, touched: &[NodeId]) -> Vec<NodeId> {
    let dom = page.dom.borrow();
    let live: Vec<NodeId> = touched.iter().copied().filter(|&n| dom.is_alive(n) && dom.is_connected(n).unwrap_or(false)).collect();
    let set: FxHashSet<NodeId> = live.iter().copied().collect();
    let mut roots = Vec::new();
    let mut seen = FxHashSet::default();
    for &node in &live {
        let mut ancestor = dom.parent(node).ok().flatten();
        let mut inside = false;
        while let Some(a) = ancestor {
            if set.contains(&a) {
                inside = true;
                break;
            }
            ancestor = dom.parent(a).ok().flatten();
        }
        if !inside && seen.insert(node) {
            roots.push(node);
            if roots.len() >= MAX_ROOTS {
                break;
            }
        }
    }
    roots
}

/// The end of a rendering update of `page`'s document; called when ENABLED.
pub fn on_frame(page: &Rc<PageDom>, frame: &FrameInfo) {
    // Idle: a document nobody interacted with has no state.
    let mut contexts = match STATES.with(|states| {
        let mut states = states.borrow_mut();
        let state = states.get_mut(&page.id())?;
        state.frame_pending = false;
        Some(std::mem::take(&mut state.contexts))
    }) {
        Some(contexts) => contexts,
        None => return,
    };
    // The nodes mutations touched, to the contexts they were attributed to (those of a context that is gone
    // are dropped).
    let touched = match page.dom.try_borrow_mut() {
        Ok(mut dom) => dom.take_touched(),
        Err(_) => Vec::new(),
    };
    for (id, node) in touched {
        if let Some(c) = contexts.iter_mut().find(|c| c.id == id) {
            if c.touched.len() < MAX_ROOTS * 4 {
                c.touched.push(node);
            }
        }
    }
    contexts.retain(|c| c.opened.elapsed() < LIFETIME);
    let mut again = false;
    for c in contexts.iter_mut() {
        // The paint criterion and the interaction contentful paints: what is painted under the touched nodes.
        let mut new_icp: Option<Rc<IcpEntry>> = None;
        if !c.touched.is_empty() && c.frames < MAX_FRAMES {
            let roots = roots_of(page, &c.touched);
            let stamp = page.layout().observation_stamp(&page.dom.borrow());
            if !roots.is_empty() && c.settled != Some((stamp, c.touched.len())) {
                c.frames += 1;
                let scan = scan::painted_candidates_in(page, frame, &roots);
                if scan.candidates.is_empty() {
                    // Still awaiting it (an image loading, a layout to come): another frame.
                    again |= c.painted.is_none();
                } else {
                    c.painted.get_or_insert((frame.paint_time, frame.presented));
                    c.settled = Some((stamp, c.touched.len()));
                    let mut best: Option<(f64, &scan::Candidate)> = None;
                    for candidate in &scan.candidates {
                        let Some(size) = lcp::size_of(page, candidate, scan.viewport) else { continue };
                        if size > c.icp_largest && best.is_none_or(|(b, _)| size > b) {
                            best = Some((size, candidate));
                        }
                    }
                    if let Some((size, candidate)) = best {
                        if let Some(entry) = lcp::make_entry(page, frame, candidate, size) {
                            c.icp_largest = size;
                            new_icp = Some(Rc::new(IcpEntry { lcp: entry, interaction_id: c.interaction_id, navigation_id: c.navigation_id }));
                        }
                    }
                }
            }
        }
        // A URL change and a paint, in either order: the soft navigation (before the paint it made, so that
        // that paint is of its navigation).
        if let (false, Some((url, kind)), Some((painted, presented))) = (c.emitted, c.url.clone(), c.painted) {
            c.emitted = true;
            page.timeline().adopt_navigation_id(c.navigation_id);
            let latest = new_icp.clone().or_else(|| c.largest.borrow().clone());
            let (paint_time, presentation_time) = match latest {
                Some(icp) => (icp.lcp.paint_time, icp.lcp.presentation_time.unwrap_or(icp.lcp.paint_time)),
                None => (painted, presented),
            };
            let entry = SoftNavEntry {
                url,
                start: c.start,
                navigation_type: kind,
                interaction_id: c.interaction_id,
                navigation_id: c.navigation_id,
                paint_time,
                presentation_time,
                largest: c.largest.clone(),
            };
            queue_entry(page, Rc::new(SoftNavTiming(Rc::new(entry))));
        }
        if let Some(icp) = new_icp {
            *c.largest.borrow_mut() = Some(icp.clone());
            queue_entry(page, Rc::new(IcpTiming(icp)));
        }
    }
    let empty = STATES.with(|states| {
        let mut states = states.borrow_mut();
        let Some(state) = states.get_mut(&page.id()) else { return true };
        // (Contexts opened while this ran stay.)
        contexts.append(&mut state.contexts);
        state.contexts = contexts;
        let empty = state.contexts.is_empty();
        if empty {
            states.remove(&page.id());
        }
        empty
    });
    if empty {
        if let Ok(mut dom) = page.dom.try_borrow_mut() {
            dom.set_touch_watch(None);
        }
        page.timeline().set_soft_armed(false);
    } else if again {
        request_rendering(page);
    }
}

/// What a `soft-navigation` entry holds.
pub struct SoftNavEntry {
    url: String,
    start: f64,
    navigation_type: &'static str,
    interaction_id: u32,
    navigation_id: u64,
    paint_time: f64,
    presentation_time: f64,
    /// The interaction's latest contentful paint (it can still change after the entry was made).
    largest: LargestIcp,
}

/// What an `interaction-contentful-paint` entry holds.
pub struct IcpEntry {
    lcp: Rc<LcpEntry>,
    interaction_id: u32,
    /// The navigation the entry was made in.
    navigation_id: u64,
}

pub struct IcpData {
    entry: Rc<IcpEntry>,
    base: PerformanceEntryData,
    /// The `largestContentfulPaint` object ([SameObject]).
    lcp: RefCell<Option<Persistent>>,
}

fn icp_data(entry: &Rc<IcpEntry>) -> IcpData {
    let mut base = PerformanceEntryData::new("", "interaction-contentful-paint", entry.lcp.start(), 0.0);
    base.navigation_id = entry.navigation_id;
    IcpData { entry: entry.clone(), base, lcp: RefCell::new(None) }
}

/// `InteractionContentfulPaint`.
pub struct InteractionContentfulPaint;

impl JsClass for InteractionContentfulPaint {
    const NAME: &'static str = "InteractionContentfulPaint";
    type Data = IcpData;
    type Parent = PerformanceEntry;

    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &IcpData| &d.base);
        def.getter("largestContentfulPaint", |s, d| {
            if let Some(cached) = &*d.lcp.borrow() {
                return Ok(cached.get(s));
            }
            let object = lcp::wrap_lcp(s, &d.entry.lcp, d.entry.navigation_id)?;
            *d.lcp.borrow_mut() = Some(Persistent::new(s, object));
            Ok(object)
        });
        def.getter("interactionId", |s, d| Ok(s.number(f64::from(d.entry.interaction_id))));
        // PaintTimingMixin.
        def.getter("paintTime", |s, d| Ok(s.number(d.entry.lcp.paint_time)));
        def.getter("presentationTime", |s, d| Ok(d.entry.lcp.presentation_time.map_or(s.null(), |t| s.number(t))));
        def.method("toJSON", 0, |s, d, _a| {
            let e = &d.entry;
            let o = s.object();
            o.set("name", s.string(""))?;
            o.set("entryType", s.string("interaction-contentful-paint"))?;
            o.set("startTime", s.number(e.lcp.start()))?;
            o.set("duration", s.number(0.0))?;
            o.set("navigationId", s.number(e.navigation_id as f64))?;
            o.set("interactionId", s.number(f64::from(e.interaction_id)))?;
            o.set("largestContentfulPaint", lcp::wrap_lcp(s, &e.lcp, e.navigation_id)?)?;
            o.set("paintTime", s.number(e.lcp.paint_time))?;
            o.set("presentationTime", e.lcp.presentation_time.map_or(s.null(), |t| s.number(t)))?;
            Ok(o.value())
        });
    }
}

struct IcpTiming(Rc<IcpEntry>);

impl TimelineEntry for IcpTiming {
    fn entry_type(&self) -> &'static str {
        "interaction-contentful-paint"
    }
    fn start_time(&self) -> f64 {
        self.0.lcp.start()
    }
    fn wrap<'s>(&self, s: &Scope<'s>, key: IdentityKey) -> JsResult<Value<'s>> {
        Ok(s.wrap::<InteractionContentfulPaint>(icp_data(&self.0), Some(key))?.value())
    }
}

pub struct SoftNavData {
    entry: Rc<SoftNavEntry>,
    base: PerformanceEntryData,
}

/// `PerformanceSoftNavigation`.
pub struct PerformanceSoftNavigation;

impl JsClass for PerformanceSoftNavigation {
    const NAME: &'static str = "PerformanceSoftNavigation";
    type Data = SoftNavData;
    type Parent = PerformanceEntry;

    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &SoftNavData| &d.base);
        def.getter("navigationType", |s, d| Ok(s.string(d.entry.navigation_type)));
        def.getter("interactionId", |s, d| Ok(s.number(f64::from(d.entry.interaction_id))));
        // PaintTimingMixin.
        def.getter("paintTime", |s, d| Ok(s.number(d.entry.paint_time)));
        def.getter("presentationTime", |s, d| Ok(s.number(d.entry.presentation_time)));
        def.method("getLargestInteractionContentfulPaint", 0, |s, d, _a| {
            let latest = d.entry.largest.borrow().clone();
            match latest {
                Some(icp) => Ok(s.wrap::<InteractionContentfulPaint>(icp_data(&icp), None)?.value()),
                None => Ok(s.null()),
            }
        });
        def.method("toJSON", 0, |s, d, _a| {
            let e = &d.entry;
            let o = s.object();
            o.set("name", s.string(&e.url))?;
            o.set("entryType", s.string("soft-navigation"))?;
            o.set("startTime", s.number(e.start))?;
            o.set("duration", s.number(0.0))?;
            o.set("navigationId", s.number(e.navigation_id as f64))?;
            o.set("navigationType", s.string(e.navigation_type))?;
            o.set("interactionId", s.number(f64::from(e.interaction_id)))?;
            o.set("paintTime", s.number(e.paint_time))?;
            o.set("presentationTime", s.number(e.presentation_time))?;
            Ok(o.value())
        });
    }
}

struct SoftNavTiming(Rc<SoftNavEntry>);

impl TimelineEntry for SoftNavTiming {
    fn entry_type(&self) -> &'static str {
        "soft-navigation"
    }
    fn start_time(&self) -> f64 {
        self.0.start
    }
    fn wrap<'s>(&self, s: &Scope<'s>, key: IdentityKey) -> JsResult<Value<'s>> {
        let mut base = PerformanceEntryData::new(&self.0.url, "soft-navigation", self.0.start, 0.0);
        base.navigation_id = self.0.navigation_id;
        Ok(s.wrap::<PerformanceSoftNavigation>(SoftNavData { entry: self.0.clone(), base }, Some(key))?.value())
    }
}

/// The document's `soft-navigation` entries as `getEntries*()` lists them (they are on the performance
/// timeline), as `(startTime, entry)` pairs; `name` filters.
pub fn timeline_entries<'s>(s: &Scope<'s>, page: &PageDom, name: Option<&str>) -> JsResult<Vec<(f64, Value<'s>)>> {
    let mut entries = super::event::buffered_entries(s, page, "soft-navigation")?;
    if let Some(name) = name {
        entries.retain(|(_, entry)| {
            entry.as_object().is_some_and(|o| o.get("name").and_then(|v| v.to_string()).is_ok_and(|n| n == name))
        });
    }
    Ok(entries)
}
