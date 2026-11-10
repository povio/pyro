//! `HTMLMediaElement` as a browser without an audio device runs it. The
//! resource is fetched through the embedder (a Media request, so CDP and
//! request interception see it) and its container headers are read
//! (`media_format`): duration, video size and tracks. A playable resource
//! then walks the ready states with their events (`durationchange`,
//! `resize`, `loadedmetadata`, `loadeddata`, `canplay`, `canplaythrough`);
//! one that is not fails with MEDIA_ERR_SRC_NOT_SUPPORTED.
//!
//! A `<video>` keeps its resource's bytes when this build decodes video
//! (lp-media): the frame at the playback position is decoded when a
//! screenshot, `drawImage`, `createImageBitmap` or a
//! `requestVideoFrameCallback` step needs it ([`current_frame`]).
//!
//! Playback runs a clock on the document's time (`performance.now()`, so
//! virtual time moves it too): `currentTime` advances at `playbackRate`
//! while the element is potentially playing, `timeupdate` fires every
//! 250 ms, and the end fires `ended` (or seeks back to 0 with `loop`).
//! Exactly one timer exists per playing element and none otherwise.
//!
//! Autoplay follows Chrome's default policy: muted media may always play;
//! unmuted media needs sticky user activation of the document (trusted
//! input, or a CDP evaluation with `userGesture`). Otherwise `play()`
//! rejects with NotAllowedError and the `autoplay` attribute does nothing.
//!
//! The DOM queues what starts a load (`lp_dom::media_element`); the queue is
//! drained with the iframe post-connection steps. Per-element state exists
//! only for elements script or a load touched.
//!
//! TimeRanges, MediaError and TextTrackList are made in JS (media.js) on a
//! realm's first use.
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use bytes::Bytes;
use lp_dom::media_element::{MediaChange, MediaEvent};
use lp_dom::NodeId;
use lp_js::{ClassDef, Deferred, JsClass, JsError, JsResult, Persistent, Scope, Value, WeakRealm};
use rustc_hash::FxHashMap;

use crate::html::HtmlMediaElement;
use crate::lazy_module::{lazy_globals, LazyModule};
use crate::media_format::{self, MediaInfo, PartProbe};
use crate::node::{node_value, NodeData};
use crate::page::{MediaFetchHandle, MediaRequest, MediaResponse, PageDom};
use crate::util::*;
use crate::window::WindowHost;

static MODULE: LazyModule = LazyModule {
    key: "lpMedia",
    source: include_str!("media.js"),
    url: "lightningpanda://internal/media.js",
    natives: None,
};

/// Adds the lazy interface names to lp-browser's `Window` class.
pub(crate) fn define_globals<W>(def: &mut ClassDef<W>)
where
    W: JsClass,
    W::Data: WindowHost,
{
    lazy_globals!(def, MODULE, "TimeRanges", "MediaError", "TextTrackList", "VideoPlaybackQuality");
}

const NETWORK_EMPTY: u8 = 0;
const NETWORK_IDLE: u8 = 1;
const NETWORK_LOADING: u8 = 2;
const NETWORK_NO_SOURCE: u8 = 3;
const HAVE_NOTHING: u8 = 0;
const HAVE_METADATA: u8 = 1;
const HAVE_CURRENT_DATA: u8 = 2;
const HAVE_FUTURE_DATA: u8 = 3;
const HAVE_ENOUGH_DATA: u8 = 4;
const MEDIA_ERR_SRC_NOT_SUPPORTED: u16 = 4;
/// Chrome's message for a source that could not be fetched.
const FORMAT_ERROR: &str = "MEDIA_ELEMENT_ERROR: Format error";
const NO_SOURCES: &str = "The element has no supported sources.";
const NOT_ALLOWED: &str =
    "play() failed because the user didn't interact with the document first. https://goo.gl/xX8pDD";
/// Chrome's periodic `timeupdate` interval.
const TIMEUPDATE_MS: f64 = 250.0;

/// Where a load's media data comes from.
enum Resource {
    Url(String),
    Bytes(Bytes),
    /// A `srcObject` that is not a Blob (MediaStream, MediaSource): nothing
    /// can be read from it.
    Unreadable,
}

struct MediaState {
    network: u8,
    ready: u8,
    paused: bool,
    /// HTML "default playback start position": what `currentTime` sets while
    /// there is no media data.
    default_start: f64,
    /// The official playback position while the clock is stopped.
    position: f64,
    /// While potentially playing: the document time (ms) and position the
    /// clock started from.
    clock: Option<(f64, f64)>,
    duration: f64,
    video: (u32, u32),
    seeking: bool,
    seek_serial: u32,
    volume: f64,
    /// The muted IDL attribute once set; until then the `muted` content
    /// attribute's presence.
    muted: Option<bool>,
    rate: f64,
    default_rate: f64,
    preserves_pitch: bool,
    /// The current MediaError: code, message and a serial for its object's identity.
    error: Option<(u16, &'static str, u32)>,
    error_serial: u32,
    /// Bumped by each load: tasks of an earlier load do nothing.
    generation: u32,
    /// Resource selection ran out of `<source>` children and waits for more.
    awaiting_source: bool,
    /// The `<source>` child resource selection tried last.
    pointer: Option<NodeId>,
    current_src: String,
    /// `srcObject`: `Some(Some(bytes))` for a Blob, `Some(None)` for another object.
    src_object: Option<Option<Bytes>>,
    pending_play: Vec<(WeakRealm, Deferred)>,
    /// HTML "can autoplay flag".
    can_autoplay: bool,
    /// `preload=none` stopped the load before fetching: what to fetch once
    /// playback is requested, and the `<source>` it came from.
    suspended: Option<(Resource, Option<NodeId>)>,
    /// The fetch in flight; dropping it cancels the request.
    fetch: Option<MediaFetchHandle>,
    fetching: bool,
    timer: Option<u64>,
    timer_serial: u32,
    /// HTML "delaying-the-load-event flag".
    delaying_load: bool,
    /// The element holds a DOM reference (fetching or playing), so an
    /// unreferenced `new Audio()` keeps loading and playing.
    pinned: bool,
    /// Played ranges, merged and sorted.
    played: Vec<(f64, f64)>,
    /// HTML "show poster flag": a `<video>` with a poster paints it until
    /// playback or a seek.
    show_poster: bool,
    /// A `<video>`'s frames, while its resource has a video track.
    frames: Option<Box<VideoFrames>>,
    /// `requestVideoFrameCallback` callbacks: handle, realm, callback.
    frame_callbacks: Vec<(u32, WeakRealm, Persistent)>,
    next_frame_callback: u32,
    /// The frame (its index, or the position for a video without a
    /// decoder) last presented to frame callbacks, and how many were.
    presented: Option<f64>,
    presented_frames: u32,
}

/// A `<video>`'s resource, kept to decode frames from: the bytes until the
/// player (made on the first frame asked for) takes them.
struct VideoFrames {
    bytes: Option<Bytes>,
    player: Option<lp_media::video::Player>,
    /// The response may be read back (`drawImage` keeps a canvas clean).
    clean: bool,
}

impl VideoFrames {
    fn player(&mut self) -> Option<&mut lp_media::video::Player> {
        if self.player.is_none() {
            // No decoder for the codec, or no samples: the bytes go.
            self.player = lp_media::video::Player::open(self.bytes.take()?);
        }
        self.player.as_mut()
    }
}

impl Default for MediaState {
    fn default() -> Self {
        MediaState {
            network: NETWORK_EMPTY,
            ready: HAVE_NOTHING,
            paused: true,
            default_start: 0.0,
            position: 0.0,
            clock: None,
            duration: f64::NAN,
            video: (0, 0),
            seeking: false,
            seek_serial: 0,
            volume: 1.0,
            muted: None,
            rate: 1.0,
            default_rate: 1.0,
            preserves_pitch: true,
            error: None,
            error_serial: 0,
            generation: 0,
            awaiting_source: false,
            pointer: None,
            current_src: String::new(),
            src_object: None,
            pending_play: Vec::new(),
            can_autoplay: true,
            suspended: None,
            fetch: None,
            fetching: false,
            timer: None,
            timer_serial: 0,
            delaying_load: false,
            pinned: false,
            played: Vec::new(),
            show_poster: true,
            frames: None,
            frame_callbacks: Vec::new(),
            next_frame_callback: 1,
            presented: None,
            presented_frames: 0,
        }
    }
}

impl MediaState {
    /// The current playback position at document time `now`.
    fn position_at(&self, now: f64) -> f64 {
        match self.clock {
            Some((t0, p0)) => {
                let p = p0 + (now - t0).max(0.0) / 1000.0 * self.rate;
                if self.duration.is_finite() {
                    p.clamp(0.0, self.duration)
                } else {
                    p.max(0.0)
                }
            }
            None => self.position,
        }
    }
    fn at_end(&self, position: f64) -> bool {
        self.ready >= HAVE_METADATA && self.duration.is_finite() && position >= self.duration
    }
    /// HTML "potentially playing", less "ended playback" (the clock stops at the end itself).
    fn potentially_playing(&self) -> bool {
        !self.paused && self.ready >= HAVE_FUTURE_DATA && !self.seeking && self.error.is_none()
    }
    /// Stops the clock at `now`, recording the played range.
    fn fold(&mut self, now: f64) {
        if let Some((_, start)) = self.clock {
            let end = self.position_at(now);
            self.position = end;
            self.clock = None;
            add_range(&mut self.played, start.min(end), start.max(end));
        }
    }
    fn wants_pin(&self) -> bool {
        self.fetching || (!self.paused && self.ready >= HAVE_METADATA)
    }
}

/// Most disjoint played ranges kept; past it the closest neighbours merge
/// (a page seeking around cannot grow the list without bound).
const MAX_PLAYED_RANGES: usize = 64;

/// Adds [start, end] to sorted, disjoint `ranges`.
fn add_range(ranges: &mut Vec<(f64, f64)>, start: f64, end: f64) {
    let at = ranges.partition_point(|r| r.0 < start);
    ranges.insert(at, (start, end));
    let mut merged: Vec<(f64, f64)> = Vec::with_capacity(ranges.len());
    for &(s, e) in ranges.iter() {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    if merged.len() > MAX_PLAYED_RANGES {
        let gap = (1..merged.len())
            .min_by(|&a, &b| (merged[a].0 - merged[a - 1].1).total_cmp(&(merged[b].0 - merged[b - 1].1)))
            .unwrap_or(1);
        merged[gap - 1].1 = merged[gap].1;
        merged.remove(gap);
    }
    *ranges = merged;
}

/// The media state of a page's elements, made on an element's first use.
#[derive(Default)]
pub(crate) struct MediaElements {
    elements: RefCell<FxHashMap<NodeId, MediaState>>,
    /// Entry count at which those of collected elements are dropped.
    sweep_at: Cell<usize>,
    /// Elements whose delaying-the-load-event flag is set.
    delaying_load: Cell<usize>,
}

impl PageDom {
    /// Whether a media element of this document delays its load event
    /// (HTML "delaying-the-load-event flag"): its load has started and
    /// has not reached HAVE_CURRENT_DATA, failed or suspended.
    pub fn media_delays_load(&self) -> bool {
        self.media.delaying_load.get() != 0
    }
    /// Playback timers of this document's media elements: one per element
    /// that is playing, none otherwise.
    pub fn media_timers(&self) -> usize {
        self.media.elements.borrow().values().filter(|st| st.timer.is_some()).count()
    }
}

fn with<R>(page: &PageDom, node: NodeId, f: impl FnOnce(&mut MediaState) -> R) -> R {
    let mut elements = page.media.elements.borrow_mut();
    if !elements.contains_key(&node) && elements.len() >= page.media.sweep_at.get() {
        sweep(page, &mut elements);
    }
    f(elements.entry(node).or_default())
}

/// Drops the state of collected elements (a pinned element is alive).
fn sweep(page: &PageDom, elements: &mut FxHashMap<NodeId, MediaState>) {
    let Ok(dom) = page.dom.try_borrow() else { return };
    let delaying = page.media.delaying_load.get();
    elements.retain(|id, st| {
        let keep = st.pinned || dom.is_alive(*id);
        if !keep && st.delaying_load {
            page.media.delaying_load.set(page.media.delaying_load.get().saturating_sub(1));
        }
        keep
    });
    page.media.sweep_at.set((elements.len() * 2).max(64));
    // A collected element no longer delays the load event.
    if delaying != 0 && page.media.delaying_load.get() == 0 {
        if let Some(hooks) = page.hooks() {
            hooks.media_load_delay_released();
        }
    }
}

/// Reads state without making it: an untouched element's defaults.
fn peek<R>(page: &PageDom, node: NodeId, f: impl FnOnce(&MediaState) -> R) -> R {
    match page.media.elements.borrow().get(&node) {
        Some(state) => f(state),
        None => f(&MediaState::default()),
    }
}

fn now(page: &PageDom) -> f64 {
    crate::event::page_now_ms(page)
}

fn attr(page: &PageDom, node: NodeId, name: &str) -> bool {
    page.dom.borrow().has_attribute(node, name).unwrap_or(false)
}

fn is_video(page: &PageDom, node: NodeId) -> bool {
    page.dom.borrow().element(node).is_ok_and(|e| e.is_html() && &*e.name.local == "video")
}

fn muted(page: &PageDom, node: NodeId) -> bool {
    match peek(page, node, |st| st.muted) {
        Some(m) => m,
        None => attr(page, node, "muted"),
    }
}

/// Chrome's autoplay policy: muted media, or a document with sticky user activation.
fn may_play(page: &PageDom, node: NodeId) -> bool {
    muted(page, node) || page.has_sticky_activation()
}

/// Sets the element's delaying-the-load-event flag.
fn set_delaying(page: &PageDom, node: NodeId, delaying: bool) {
    let changed = with(page, node, |st| std::mem::replace(&mut st.delaying_load, delaying) != delaying);
    if !changed {
        return;
    }
    let count = page.media.delaying_load.get();
    if delaying {
        page.media.delaying_load.set(count + 1);
    } else {
        page.media.delaying_load.set(count.saturating_sub(1));
        if count == 1 {
            if let Some(hooks) = page.hooks() {
                hooks.media_load_delay_released();
            }
        }
    }
}

/// Holds or releases the element's DOM reference to match its state.
fn update_pin(page: &PageDom, node: NodeId) {
    let change = with(page, node, |st| {
        let want = st.wants_pin();
        (want != st.pinned).then(|| {
            st.pinned = want;
            want
        })
    });
    match change {
        Some(true) => {
            if page.dom.borrow_mut().add_ref(node).is_err() {
                with(page, node, |st| st.pinned = false);
            }
        }
        Some(false) => page.dom.borrow_mut().release(node),
        None => {}
    }
}

/// Drains the DOM's media load triggers for `page`'s document (with the
/// iframe post-connection steps).
pub(crate) fn flush(page: &PageDom, this: &Weak<PageDom>) {
    if page.is_disposed() {
        return;
    }
    let events = {
        let Ok(mut dom) = page.dom.try_borrow_mut() else { return };
        if !dom.has_media_events() {
            return;
        }
        dom.take_media_events(page.document())
    };
    if let Some(page) = this.upgrade() {
        for event in &events {
            handle(&page, event);
        }
    }
    release_events(page, events);
}

fn release_events(page: &PageDom, events: Vec<MediaEvent>) {
    let mut dom = page.dom.borrow_mut();
    for event in events {
        for held in event.change.held() {
            dom.release(held);
        }
        dom.release(event.node);
    }
}

/// HTML keeps the source pointer's place when the `<source>` it is after
/// is removed: it moves to after that node's previous sibling.
fn source_removed(page: &PageDom, node: NodeId, source: NodeId, previous: Option<NodeId>) {
    with(page, node, |st| {
        if st.pointer == Some(source) {
            st.pointer = previous;
        }
    });
}

/// Releases everything the page's media elements hold: timers, fetches,
/// DOM references. Called by `PageDom::dispose` while hooks still exist.
pub(crate) fn dispose(page: &PageDom) {
    let states = std::mem::take(&mut *page.media.elements.borrow_mut());
    page.media.delaying_load.set(0);
    let hooks = page.hooks();
    let mut pinned = Vec::new();
    for (node, mut st) in states {
        if let (Some(id), Some(hooks)) = (st.timer.take(), &hooks) {
            hooks.clear_media_timer(id);
        }
        drop(st.fetch.take());
        if st.pinned {
            pinned.push(node);
        }
    }
    if let Ok(mut dom) = page.dom.try_borrow_mut() {
        for node in pinned {
            dom.release(node);
        }
    }
}

fn handle(page: &Rc<PageDom>, event: &MediaEvent) {
    let node = event.node;
    if !page.dom.borrow().is_alive(node) {
        return;
    }
    match event.change {
        MediaChange::Src => load(page, node),
        MediaChange::Inserted => {
            let (empty, has_object) = peek(page, node, |st| (st.network == NETWORK_EMPTY, st.src_object.is_some()));
            if empty && (has_object || has_src(page, node) || !source_children(page, node).is_empty()) {
                resource_selection(page, node);
            }
        }
        MediaChange::Removed => {
            // HTML: removed from a document (and not put back in this
            // task), the element pauses.
            let connected = page.dom.borrow().is_connected(node).unwrap_or(false);
            if !connected && !peek(page, node, |st| st.paused) {
                internal_pause(page, node);
            }
        }
        MediaChange::SourceRemoved { source, previous } => source_removed(page, node, source, previous),
        MediaChange::SourceInserted(_) => {
            let (network, awaiting, generation) =
                peek(page, node, |st| (st.network, st.awaiting_source, st.generation));
            if network == NETWORK_EMPTY {
                // HTML: only for a media element without a `src` attribute.
                if !has_src(page, node) {
                    resource_selection(page, node);
                }
            } else if awaiting {
                // The waiting resource selection tries the new candidate.
                with(page, node, |st| {
                    st.awaiting_source = false;
                    st.network = NETWORK_LOADING;
                });
                set_delaying(page, node, true);
                next_source(page, node, generation);
            }
        }
    }
}

fn has_src(page: &PageDom, node: NodeId) -> bool {
    page.dom.borrow().get_attribute(node, "src").ok().flatten().is_some()
}

fn source_children(page: &PageDom, node: NodeId) -> Vec<NodeId> {
    let dom = page.dom.borrow();
    let Ok(children) = dom.children(node) else { return Vec::new() };
    children.into_iter().filter(|&c| dom.element(c).is_ok_and(|e| e.is_html() && &*e.name.local == "source")).collect()
}

/// Queues a media element task of the load `generation`: it does nothing
/// once another load started.
fn queue(page: &Rc<PageDom>, node: NodeId, generation: u32, f: impl FnOnce(&Rc<PageDom>) + 'static) {
    let weak = Rc::downgrade(page);
    page.post_task(Box::new(move || {
        let Some(page) = weak.upgrade() else { return };
        if page.is_disposed() || peek(&page, node, |st| st.generation) != generation {
            return;
        }
        f(&page);
    }));
}

fn queue_event(page: &Rc<PageDom>, node: NodeId, generation: u32, ty: &'static str) {
    queue(page, node, generation, move |page| {
        crate::input::fire_simple(page, node, ty, false, false);
    });
}

/// Fires `ty` at the element; false if a listener started another load.
fn fire(page: &Rc<PageDom>, node: NodeId, generation: u32, ty: &str) -> bool {
    crate::input::fire_simple(page, node, ty, false, false);
    !page.is_disposed() && peek(page, node, |st| st.generation) == generation
}

/// Rejects promises (in their realms) with a DOMException.
fn reject_all(promises: Vec<(WeakRealm, Deferred)>, name: &'static str, message: &'static str) {
    for (realm, deferred) in promises {
        let Some(realm) = realm.upgrade() else { continue };
        let Ok(s) = realm.scope() else { continue };
        let error = JsError::dom(name, message).to_value(&s);
        let _ = deferred.reject(&s, error);
    }
}

fn resolve_all(promises: Vec<(WeakRealm, Deferred)>) {
    for (realm, deferred) in promises {
        let Some(realm) = realm.upgrade() else { continue };
        let Ok(s) = realm.scope() else { continue };
        let _ = deferred.resolve(&s, s.undefined());
    }
}

/// Queues "reject pending play promises" for `promises`.
fn queue_reject(page: &Rc<PageDom>, promises: Vec<(WeakRealm, Deferred)>, name: &'static str, message: &'static str) {
    if promises.is_empty() {
        return;
    }
    page.post_task(Box::new(move || reject_all(promises, name, message)));
}

/// Stops or (re)starts the playback clock to match the element's state, and
/// keeps exactly one timer for it while it advances.
fn sync_clock(page: &Rc<PageDom>, node: NodeId) {
    let t = now(page);
    let (old, plan) = with(page, node, |st| {
        st.fold(t);
        let old = st.timer.take();
        st.timer_serial = st.timer_serial.wrapping_add(1);
        let mut plan = None;
        if st.potentially_playing() {
            st.clock = Some((t, st.position));
            if st.rate > 0.0 {
                let remaining = if st.duration.is_finite() {
                    (st.duration - st.position).max(0.0) / st.rate * 1000.0
                } else {
                    f64::INFINITY
                };
                plan = Some((remaining.min(TIMEUPDATE_MS), st.timer_serial));
            }
        }
        (old, plan)
    });
    let hooks = page.hooks();
    if let (Some(id), Some(hooks)) = (old, &hooks) {
        hooks.clear_media_timer(id);
    }
    want_frame(page, node);
    let Some((delay, serial)) = plan else { return };
    let weak = Rc::downgrade(page);
    let task: Box<dyn FnOnce()> = Box::new(move || {
        if let Some(page) = weak.upgrade() {
            if !page.is_disposed() {
                tick(&page, node, serial);
            }
        }
    });
    let delay = Duration::from_secs_f64(delay.max(0.0) / 1000.0);
    // Without an embedder there is no event loop to time playback with.
    if let Some(hooks) = hooks {
        let id = hooks.set_media_timer(delay, task);
        with(page, node, |st| st.timer = id);
    }
}

/// The playback timer: `timeupdate`, or the end of the resource.
fn tick(page: &Rc<PageDom>, node: NodeId, serial: u32) {
    let t = now(page);
    let ended = with(page, node, |st| {
        if st.timer_serial != serial || st.clock.is_none() {
            return None;
        }
        st.timer = None;
        let position = st.position_at(t);
        // Timer granularity: within half a millisecond of the end is the end.
        let remaining = if st.duration.is_finite() && st.rate > 0.0 {
            (st.duration - position) / st.rate * 1000.0
        } else {
            f64::INFINITY
        };
        Some(st.at_end(position) || remaining < 0.5)
    });
    match ended {
        None => {}
        Some(true) => reached_end(page, node),
        Some(false) => {
            crate::input::fire_simple(page, node, "timeupdate", false, false);
            sync_clock(page, node);
        }
    }
}

/// HTML: playback reached the end of the media resource.
fn reached_end(page: &Rc<PageDom>, node: NodeId) {
    if attr(page, node, "loop") {
        seek(page, node, 0.0);
        return;
    }
    let t = now(page);
    let (generation, timer) = with(page, node, |st| {
        st.fold(t);
        st.position = st.duration;
        st.timer_serial = st.timer_serial.wrapping_add(1);
        (st.generation, st.timer.take())
    });
    // The clock stays stopped: restarting it at the end would end again.
    if let (Some(id), Some(hooks)) = (timer, page.hooks()) {
        hooks.clear_media_timer(id);
    }
    if !fire(page, node, generation, "timeupdate") {
        return;
    }
    let pending = with(page, node, |st| {
        if !st.paused && st.at_end(st.position) {
            st.paused = true;
            Some(std::mem::take(&mut st.pending_play))
        } else {
            None
        }
    });
    update_pin(page, node);
    if let Some(pending) = pending {
        if !fire(page, node, generation, "pause") {
            return;
        }
        reject_all(pending, "AbortError", "The play() request was interrupted by a call to pause().");
    }
    crate::input::fire_simple(page, node, "ended", false, false);
}

/// HTML "seek" to `to`, with media data (readyState at least HAVE_METADATA).
fn seek(page: &Rc<PageDom>, node: NodeId, to: f64) {
    let t = now(page);
    let (serial, generation) = with(page, node, |st| {
        st.fold(t);
        let max = if st.duration.is_finite() { st.duration } else { f64::MAX };
        st.position = to.clamp(0.0, max.max(0.0));
        st.seeking = true;
        st.show_poster = false;
        st.seek_serial = st.seek_serial.wrapping_add(1);
        (st.seek_serial, st.generation)
    });
    sync_clock(page, node);
    queue_event(page, node, generation, "seeking");
    // Every byte is at hand: the seek completes in the next task.
    queue(page, node, generation, move |page| {
        let current = with(page, node, |st| {
            let current = st.seek_serial == serial && st.seeking;
            if current {
                st.seeking = false;
            }
            current
        });
        if !current || !fire(page, node, generation, "timeupdate") || !fire(page, node, generation, "seeked") {
            return;
        }
        sync_clock(page, node);
    });
}

/// HTML "notify about playing": `playing`, then the pending play() promises resolve.
fn notify_playing(page: &Rc<PageDom>, node: NodeId, generation: u32) {
    if !fire(page, node, generation, "playing") {
        return;
    }
    let pending = with(page, node, |st| std::mem::take(&mut st.pending_play));
    resolve_all(pending);
}

/// HTML "media element load algorithm".
fn load(page: &Rc<PageDom>, node: NodeId) {
    let t = now(page);
    let (network, generation, old_timer, old_fetch) = with(page, node, |st| {
        st.generation = st.generation.wrapping_add(1);
        st.fold(t);
        st.timer_serial = st.timer_serial.wrapping_add(1);
        st.fetching = false;
        st.suspended = None;
        (st.network, st.generation, st.timer.take(), st.fetch.take())
    });
    drop(old_fetch);
    if let (Some(id), Some(hooks)) = (old_timer, page.hooks()) {
        hooks.clear_media_timer(id);
    }
    if matches!(network, NETWORK_LOADING | NETWORK_IDLE) {
        queue_event(page, node, generation, "abort");
    }
    if network != NETWORK_EMPTY {
        queue_event(page, node, generation, "emptied");
        let (aborted, moved) = with(page, node, |st| {
            st.network = NETWORK_EMPTY;
            st.awaiting_source = false;
            st.ready = HAVE_NOTHING;
            st.seeking = false;
            let aborted = if st.paused {
                Vec::new()
            } else {
                st.paused = true;
                std::mem::take(&mut st.pending_play)
            };
            let moved = st.position != 0.0 || st.default_start != 0.0;
            st.position = 0.0;
            st.default_start = 0.0;
            st.duration = f64::NAN;
            st.video = (0, 0);
            st.played.clear();
            (aborted, moved)
        });
        page.dom.borrow_mut().set_natural_size(node, None);
        queue_reject(page, aborted, "AbortError", "The play() request was interrupted by a new load request.");
        if moved {
            queue_event(page, node, generation, "timeupdate");
        }
    }
    with(page, node, |st| {
        st.rate = st.default_rate;
        st.error = None;
        st.current_src.clear();
        st.can_autoplay = true;
        st.pointer = None;
        st.show_poster = true;
        st.frames = None;
        st.presented = None;
    });
    update_pin(page, node);
    resource_selection(page, node);
}

/// HTML "resource selection algorithm": the synchronous part, then a task
/// for the rest (after "await a stable state").
fn resource_selection(page: &Rc<PageDom>, node: NodeId) {
    let generation = with(page, node, |st| {
        st.network = NETWORK_NO_SOURCE;
        st.awaiting_source = false;
        st.generation
    });
    set_delaying(page, node, true);
    queue(page, node, generation, move |page| select(page, node, generation));
}

fn select(page: &Rc<PageDom>, node: NodeId, generation: u32) {
    let object = peek(page, node, |st| st.src_object.clone());
    let src = page.dom.borrow().get_attribute(node, "src").ok().flatten().map(str::to_owned);
    if object.is_none() && src.is_none() && source_children(page, node).is_empty() {
        with(page, node, |st| st.network = NETWORK_EMPTY);
        set_delaying(page, node, false);
        return;
    }
    with(page, node, |st| st.network = NETWORK_LOADING);
    queue_event(page, node, generation, "loadstart");
    if let Some(object) = object {
        let resource = match object {
            Some(bytes) => Resource::Bytes(bytes),
            None => Resource::Unreadable,
        };
        fetch_resource(page, node, generation, resource, None);
        return;
    }
    if let Some(src) = src {
        // Attribute mode.
        let url = if src.is_empty() { None } else { resolve(page, node, &src) };
        let Some(url) = url else {
            queue(page, node, generation, move |page| media_source_failed(page, node, FORMAT_ERROR));
            return;
        };
        with(page, node, |st| st.current_src = url.clone());
        fetch_resource(page, node, generation, Resource::Url(url), None);
        return;
    }
    next_source(page, node, generation);
}

fn resolve(page: &PageDom, node: NodeId, value: &str) -> Option<String> {
    let dom = page.dom.borrow();
    let resolved = crate::html::resolve_url(&dom, node, value);
    url::Url::parse(&resolved).ok().map(|u| u.to_string())
}

/// Children mode: tries the `<source>` children after the pointer in turn;
/// each one that cannot be used gets an `error` event. Out of candidates,
/// the element waits for another `<source>`.
fn next_source(page: &Rc<PageDom>, node: NodeId, generation: u32) {
    // Removals since the pointer last moved (still queued) keep its place.
    let removals = page.dom.borrow_mut().take_source_removals(node);
    if !removals.is_empty() {
        for event in &removals {
            if let MediaChange::SourceRemoved { source, previous } = event.change {
                source_removed(page, node, source, previous);
            }
        }
        release_events(page, removals);
    }
    let pointer = peek(page, node, |st| st.pointer);
    let sources = {
        let dom = page.dom.borrow();
        let first = match pointer {
            Some(p) if dom.parent(p).ok().flatten() == Some(node) => dom.next_sibling(p).ok().flatten(),
            _ => dom.first_child(node).ok().flatten(),
        };
        std::iter::successors(first, |&c| dom.next_sibling(c).ok().flatten())
            .filter(|&c| dom.element(c).is_ok_and(|e| e.is_html() && &*e.name.local == "source"))
            .collect::<Vec<_>>()
    };
    for &source in &sources {
        with(page, node, |st| st.pointer = Some(source));
        let (src, ty) = {
            let dom = page.dom.borrow();
            let get = |name| dom.get_attribute(source, name).ok().flatten().map(str::to_owned);
            (get("src"), get("type"))
        };
        let supported = ty.as_deref().is_none_or(|t| media_format::can_play_type(t) != media_format::CanPlay::No);
        let url = src.filter(|s| !s.is_empty() && supported).and_then(|s| resolve(page, source, &s));
        let Some(url) = url else {
            queue(page, node, generation, move |page| {
                crate::input::fire_simple(page, source, "error", false, false);
            });
            continue;
        };
        with(page, node, |st| st.current_src = url.clone());
        fetch_resource(page, node, generation, Resource::Url(url), Some(source));
        return;
    }
    queue(page, node, generation, move |page| {
        with(page, node, |st| {
            st.network = NETWORK_NO_SOURCE;
            st.awaiting_source = true;
        });
        set_delaying(page, node, false);
    });
}

/// HTML "resource fetch algorithm" for `resource` (from `source` in children mode).
fn fetch_resource(page: &Rc<PageDom>, node: NodeId, generation: u32, resource: Resource, source: Option<NodeId>) {
    let preload_none =
        page.dom.borrow().get_attribute(node, "preload").ok().flatten().is_some_and(|p| p.eq_ignore_ascii_case("none"));
    if preload_none && !attr(page, node, "autoplay") && peek(page, node, |st| st.paused) {
        with(page, node, |st| st.suspended = Some((resource, source)));
        queue(page, node, generation, move |page| {
            if peek(page, node, |st| st.suspended.is_none()) {
                return;
            }
            with(page, node, |st| st.network = NETWORK_IDLE);
            set_delaying(page, node, false);
            crate::input::fire_simple(page, node, "suspend", false, false);
        });
        return;
    }
    start_fetch(page, node, generation, resource, source, Seek::default());
}

/// Where a fetch starts in the resource: past the boxes of an MP4 that come
/// before its `moov` (a `Range` request), after `count` such hops.
#[derive(Clone, Copy, Default)]
struct Seek {
    start: u64,
    count: u8,
}

fn start_fetch(
    page: &Rc<PageDom>,
    node: NodeId,
    generation: u32,
    resource: Resource,
    source: Option<NodeId>,
    seek: Seek,
) {
    with(page, node, |st| st.fetching = true);
    update_pin(page, node);
    let weak = Rc::downgrade(page);
    let done = move |result: Result<MediaResponse, String>| {
        let Some(page) = weak.upgrade() else { return };
        if page.is_disposed() || peek(&page, node, |st| st.generation) != generation {
            return;
        }
        fetched(&page, node, generation, source, seek, result);
    };
    match resource {
        Resource::Bytes(body) => {
            let response =
                MediaResponse { status: 200, mime: String::new(), body: body.to_vec(), partial: None, clean: true };
            page.post_task(Box::new(move || done(Ok(response))));
        }
        Resource::Unreadable => page.post_task(Box::new(move || done(Err("unreadable srcObject".into())))),
        Resource::Url(url) if url.starts_with("blob:") => {
            let result = page
                .blob_url(&url)
                .map(|(bytes, mime)| MediaResponse {
                    status: 200,
                    mime,
                    body: bytes.to_vec(),
                    partial: None,
                    clean: true,
                })
                .ok_or_else(|| "revoked or unknown blob URL".to_owned());
            page.post_task(Box::new(move || done(result)));
        }
        Resource::Url(url) => {
            let crossorigin = page.dom.borrow().get_attribute(node, "crossorigin").ok().flatten().map(str::to_owned);
            // Only the container headers are read: a long or endless body
            // is cut once they are in (the load event waits for them only).
            let part = crate::page::BodyPart {
                start: seek.start,
                limit: media_format::PREFIX_LIMIT,
                enough: media_format::prefix_enough,
            };
            let request = MediaRequest { url, video: is_video(page, node), crossorigin, part };
            match page.hooks() {
                Some(hooks) => {
                    let handle = hooks.fetch_media(request, Box::new(done));
                    with(page, node, |st| st.fetch = handle);
                }
                None => page.post_task(Box::new(move || done(Err("no resource loader".into())))),
            }
        }
    }
}

/// The fetch of the current load finished.
fn fetched(
    page: &Rc<PageDom>,
    node: NodeId,
    generation: u32,
    source: Option<NodeId>,
    seek: Seek,
    result: Result<MediaResponse, String>,
) {
    // Dropping the fetch stops a transfer cut short.
    with(page, node, |st| {
        st.fetching = false;
        st.fetch = None;
    });
    update_pin(page, node);
    let mut body = None;
    let probed = match result {
        // A server that ignored the `Range` answers 200 from the start.
        Ok(r) if seek.start > 0 && r.status != 206 => Err(FORMAT_ERROR),
        Ok(r) if (200..300).contains(&r.status) && !r.body.is_empty() => {
            match media_format::probe_part(&r.body, r.partial.map(|p| p.total), seek.start) {
                PartProbe::Done(probe) => {
                    // From the start of the resource: what frames decode from.
                    if seek.start == 0 {
                        body = Some((r.body, r.clean));
                    }
                    probe.map_err(|e| e.message())
                }
                // An MP4 with its `moov` after the media data.
                PartProbe::Seek(offset) if seek.count < media_format::MAX_SEEKS => {
                    let url = peek(page, node, |st| st.current_src.clone());
                    let next = Seek { start: seek.start.saturating_add(offset), count: seek.count + 1 };
                    start_fetch(page, node, generation, Resource::Url(url), source, next);
                    return;
                }
                PartProbe::Seek(_) => Err(media_format::ProbeError::Unrecognized.message()),
            }
        }
        _ => Err(FORMAT_ERROR),
    };
    match probed {
        Ok(info) => {
            // A video track a decoder of this build may read: its bytes
            // stay for the frames (decoded on first use).
            let decoders = lp_media::video::AV1 || lp_media::video::H264;
            if let Some((bytes, clean)) = body.filter(|_| decoders && info.video.is_some() && is_video(page, node)) {
                with(page, node, |st| {
                    st.frames = Some(Box::new(VideoFrames { bytes: Some(Bytes::from(bytes)), player: None, clean }))
                });
            }
            media_ready(page, node, generation, info)
        }
        Err(message) => match source {
            // Children mode: this candidate failed; try the next one.
            Some(source) => {
                crate::input::fire_simple(page, source, "error", false, false);
                if peek(page, node, |st| st.generation) == generation {
                    next_source(page, node, generation);
                }
            }
            None => media_source_failed(page, node, message),
        },
    }
}

/// The metadata is known: the ready states follow, one task each.
fn media_ready(page: &Rc<PageDom>, node: NodeId, generation: u32, info: MediaInfo) {
    let video = is_video(page, node);
    let size = if video { info.video.unwrap_or((0, 0)) } else { (0, 0) };
    if !fire(page, node, generation, "progress") {
        return;
    }
    with(page, node, |st| {
        st.duration = info.duration;
        st.video = size;
        st.ready = HAVE_METADATA;
        st.position = 0.0;
    });
    if size != (0, 0) {
        page.dom.borrow_mut().set_natural_size(node, Some((f64::from(size.0), f64::from(size.1))));
    }
    if !fire(page, node, generation, "durationchange") {
        return;
    }
    if size != (0, 0) && !fire(page, node, generation, "resize") {
        return;
    }
    if !fire(page, node, generation, "loadedmetadata") {
        return;
    }
    let start = with(page, node, |st| std::mem::take(&mut st.default_start));
    if start != 0.0 {
        seek(page, node, start);
    }
    queue(page, node, generation, move |page| {
        with(page, node, |st| st.ready = HAVE_CURRENT_DATA);
        set_delaying(page, node, false);
        want_frame(page, node);
        fire(page, node, generation, "loadeddata");
    });
    queue(page, node, generation, move |page| {
        let playing = with(page, node, |st| {
            st.ready = HAVE_FUTURE_DATA;
            !st.paused
        });
        update_pin(page, node);
        if !fire(page, node, generation, "canplay") {
            return;
        }
        if playing && !peek(page, node, |st| st.paused) {
            sync_clock(page, node);
            notify_playing(page, node, generation);
        }
    });
    queue(page, node, generation, move |page| {
        with(page, node, |st| st.ready = HAVE_ENOUGH_DATA);
        if !fire(page, node, generation, "canplaythrough") {
            return;
        }
        // HTML: autoplay once enough data is there.
        let autoplay =
            peek(page, node, |st| st.can_autoplay && st.paused) && attr(page, node, "autoplay") && may_play(page, node);
        if autoplay {
            with(page, node, |st| {
                st.paused = false;
                st.show_poster = false;
            });
            update_pin(page, node);
            sync_clock(page, node);
            if fire(page, node, generation, "play") {
                notify_playing(page, node, generation);
            }
        }
    });
    queue(page, node, generation, move |page| {
        with(page, node, |st| st.network = NETWORK_IDLE);
        fire(page, node, generation, "suspend");
    });
}

/// HTML "dedicated media source failure steps".
fn media_source_failed(page: &Rc<PageDom>, node: NodeId, message: &'static str) {
    let pending = with(page, node, |st| {
        st.error_serial = st.error_serial.wrapping_add(1);
        st.error = Some((MEDIA_ERR_SRC_NOT_SUPPORTED, message, st.error_serial));
        st.network = NETWORK_NO_SOURCE;
        std::mem::take(&mut st.pending_play)
    });
    set_delaying(page, node, false);
    update_pin(page, node);
    crate::input::fire_simple(page, node, "error", false, false);
    reject_all(pending, "NotSupportedError", "Failed to load because no supported source was found.");
}

/// HTML "internal pause steps".
fn internal_pause(page: &Rc<PageDom>, node: NodeId) {
    if peek(page, node, |st| st.network == NETWORK_EMPTY) {
        resource_selection(page, node);
    }
    let (pending, generation) = with(page, node, |st| {
        st.can_autoplay = false;
        if st.paused {
            return (None, st.generation);
        }
        st.paused = true;
        (Some(std::mem::take(&mut st.pending_play)), st.generation)
    });
    sync_clock(page, node);
    update_pin(page, node);
    if let Some(pending) = pending {
        queue_event(page, node, generation, "timeupdate");
        queue_event(page, node, generation, "pause");
        queue_reject(page, pending, "AbortError", "The play() request was interrupted by a call to pause().");
    }
}

fn finite(v: Value<'_>, what: &str) -> JsResult<f64> {
    let n = v.to_number()?;
    if !n.is_finite() {
        return Err(JsError::Type(format!(
            "Failed to set the '{what}' property on 'HTMLMediaElement': The provided double value is non-finite."
        ).into()));
    }
    Ok(n)
}

fn page_rc(n: &NodeData) -> Rc<PageDom> {
    n.page().clone()
}

/// The frame a `<video>` shows now, decoded (lp-media), and whether its
/// pixels may be read back: `None` before `HAVE_CURRENT_DATA`, or without a
/// decoder for its video. `drawImage`, `createImageBitmap` and screenshots
/// use it.
pub(crate) fn current_frame(page: &PageDom, node: NodeId) -> Option<(lp_media::video::Frame, bool)> {
    let t = now(page);
    let mut elements = page.media.elements.borrow_mut();
    let st = elements.get_mut(&node)?;
    if st.ready < HAVE_CURRENT_DATA {
        return None;
    }
    let position = st.position_at(t);
    let frames = st.frames.as_mut()?;
    let clean = frames.clean;
    frames.player()?.frame_at(position).map(|f| (f, clean))
}

impl PageDom {
    /// The current frames of this document's `<video>` elements, for a
    /// render snapshot (painted as their content): those that show a frame
    /// (not their poster) and have one decoded. Unchanged frames keep their
    /// `Arc`. Documents that never loaded a video pay one map scan.
    pub fn video_bitmaps(&self) -> Vec<crate::canvas::CanvasBitmap> {
        let candidates: Vec<(NodeId, bool)> = {
            let elements = self.media.elements.borrow();
            if elements.is_empty() {
                return Vec::new();
            }
            elements
                .iter()
                .filter(|(_, st)| st.frames.is_some() && st.ready >= HAVE_CURRENT_DATA)
                .map(|(&node, st)| (node, st.show_poster))
                .collect()
        };
        let mut out = Vec::new();
        for (node, show_poster) in candidates {
            let poster = {
                let dom = self.dom.borrow();
                if !dom.is_connected(node).unwrap_or(false) {
                    continue;
                }
                dom.get_attribute(node, "poster").ok().flatten().is_some_and(|p| !p.trim().is_empty())
            };
            // HTML: the poster frame while the show poster flag is set.
            if show_poster && poster {
                continue;
            }
            if let Some((frame, _)) = current_frame(self, node) {
                out.push((node, frame.width, frame.height, frame.rgba));
            }
        }
        out
    }
}

// ---- requestVideoFrameCallback ------------------------------------------------

/// Asks the embedder for an animation frame when `node` has frame
/// callbacks waiting and may present a new frame.
fn want_frame(page: &PageDom, node: NodeId) {
    if peek(page, node, |st| !st.frame_callbacks.is_empty()) {
        if let Some(hooks) = page.hooks() {
            hooks.request_animation_frame();
        }
    }
}

/// What a video presents now: a key that changes with the frame (its index,
/// or the position for a video without a decoder), the frame's media time
/// and the frame size.
fn presented_frame(page: &PageDom, node: NodeId, t: f64) -> Option<(f64, f64, (u32, u32))> {
    let mut elements = page.media.elements.borrow_mut();
    let st = elements.get_mut(&node)?;
    if st.ready < HAVE_CURRENT_DATA {
        return None;
    }
    let position = st.position_at(t);
    let size = st.video;
    match st.frames.as_mut().and_then(|f| f.player()) {
        Some(player) => {
            let index = player.index_at(position);
            Some((index as f64, player.frame_time(index), size))
        }
        None => Some((position, position, size)),
    }
}

/// The `requestVideoFrameCallback` step of an animation frame (before the
/// `requestAnimationFrame` callbacks): each video that presents a frame it
/// did not present to its callbacks yet runs them with the frame's
/// metadata. Another frame is asked for while a video with callbacks plays.
pub fn run_video_frame_callbacks(page: &Rc<PageDom>, now_ms: f64) {
    let waiting: Vec<NodeId> = {
        let elements = page.media.elements.borrow();
        if elements.is_empty() {
            return;
        }
        elements.iter().filter(|(_, st)| !st.frame_callbacks.is_empty()).map(|(&n, _)| n).collect()
    };
    if waiting.is_empty() {
        return;
    }
    let t = now(page);
    let mut again = false;
    for node in waiting {
        let Some((key, media_time, (w, h))) = presented_frame(page, node, t) else { continue };
        let due = with(page, node, |st| {
            if st.presented == Some(key) {
                again |= st.potentially_playing();
                return None;
            }
            st.presented = Some(key);
            st.presented_frames += 1;
            Some((std::mem::take(&mut st.frame_callbacks), st.presented_frames))
        });
        let Some((callbacks, presented)) = due else { continue };
        for (_, realm, callback) in callbacks {
            let Some(realm) = realm.upgrade() else { continue };
            let Ok(s) = realm.scope() else { continue };
            let call = || -> JsResult<()> {
                let meta = s.object();
                meta.set_atom("presentationTime", s.number(now_ms))?;
                meta.set_atom("expectedDisplayTime", s.number(now_ms + 1000.0 / 60.0))?;
                meta.set_atom("width", s.number(f64::from(w)))?;
                meta.set_atom("height", s.number(f64::from(h)))?;
                meta.set_atom("mediaTime", s.number(media_time))?;
                meta.set_atom("presentedFrames", s.number(f64::from(presented)))?;
                let Some(f) = callback.get(&s).as_object() else { return Ok(()) };
                f.call(None, &[s.number(now_ms), meta.value()])?;
                Ok(())
            };
            if let Err(e) = call() {
                s.report_exception(&e);
            }
        }
        again |= peek(page, node, |st| !st.frame_callbacks.is_empty() && st.potentially_playing());
    }
    if again {
        if let Some(hooks) = page.hooks() {
            hooks.request_animation_frame();
        }
    }
}

/// `videoWidth`/`videoHeight`: the video track's size once its metadata is known.
pub(crate) fn video_size(n: &NodeData) -> (u32, u32) {
    peek(n.page(), n.id, |st| if st.ready >= HAVE_METADATA { st.video } else { (0, 0) })
}

/// The element's MediaError in this realm: one object per error.
fn error_value<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let Some((code, message, serial)) = peek(n.page(), n.id, |st| st.error) else { return Ok(s.null()) };
    let wrapper = node_value(s, n.page(), Some(n.id))?;
    let wrapper = wrapper.as_object().ok_or(JsError::ContextDestroyed)?;
    if let Some(cached) = s.get_hidden(wrapper, "__lpMediaError")?.and_then(|v| v.as_object()) {
        if cached.get_atom("serial")?.to_number()? == f64::from(serial) {
            return cached.get_atom("error");
        }
    }
    let error = MODULE.helper(s, n.page(), "mediaError", &[s.number(f64::from(code)), s.string(message)])?;
    let holder = s.object();
    holder.set_atom("serial", s.number(f64::from(serial)))?;
    holder.set_atom("error", error)?;
    s.set_hidden(wrapper, "__lpMediaError", holder.value())?;
    Ok(error)
}

fn time_ranges<'s>(s: &Scope<'s>, n: &NodeData, ranges: &[(f64, f64)]) -> JsResult<Value<'s>> {
    let flat: Vec<Value<'s>> = ranges.iter().flat_map(|&(a, b)| [s.number(a), s.number(b)]).collect();
    let list = s.array(&flat)?;
    MODULE.helper(s, n.page(), "timeRanges", &[list.value()])
}

/// The whole resource once it is loaded: every byte is buffered and seekable.
fn loaded_range(n: &NodeData) -> Vec<(f64, f64)> {
    peek(n.page(), n.id, |st| {
        if st.ready >= HAVE_METADATA && st.duration.is_finite() {
            vec![(0.0, st.duration)]
        } else {
            Vec::new()
        }
    })
}

/// A rate setter's shared steps: fold the clock, set, replan, `ratechange`.
fn set_rate(n: &NodeData, rate: f64, default: bool) {
    let page = page_rc(n);
    let t = now(&page);
    let (changed, generation) = with(&page, n.id, |st| {
        let old = if default { st.default_rate } else { st.rate };
        let changed = old != rate;
        if changed && !default {
            st.fold(t);
        }
        if default {
            st.default_rate = rate;
        } else {
            st.rate = rate;
        }
        (changed, st.generation)
    });
    if changed {
        if !default {
            sync_clock(&page, n.id);
        }
        queue_event(&page, n.id, generation, "ratechange");
    }
}

/// The HTMLMediaElement members (`html.rs` keeps the reflected attributes).
pub(crate) fn define(def: &mut ClassDef<HtmlMediaElement>) {
    for (name, value) in
        [("NETWORK_EMPTY", 0.0), ("NETWORK_IDLE", 1.0), ("NETWORK_LOADING", 2.0), ("NETWORK_NO_SOURCE", 3.0)]
    {
        def.constant(name, value);
    }
    for (name, value) in [
        ("HAVE_NOTHING", 0.0),
        ("HAVE_METADATA", 1.0),
        ("HAVE_CURRENT_DATA", 2.0),
        ("HAVE_FUTURE_DATA", 3.0),
        ("HAVE_ENOUGH_DATA", 4.0),
    ] {
        def.constant(name, value);
    }
    def.getter("networkState", |s, n| {
        flush(n.page(), &Rc::downgrade(n.page()));
        Ok(s.number(f64::from(peek(n.page(), n.id, |st| st.network))))
    });
    def.getter("readyState", |s, n| Ok(s.number(f64::from(peek(n.page(), n.id, |st| st.ready)))));
    def.getter("error", |s, n| {
        flush(n.page(), &Rc::downgrade(n.page()));
        error_value(s, n)
    });
    def.getter("currentSrc", |s, n| Ok(s.string(&peek(n.page(), n.id, |st| st.current_src.clone()))));
    def.getter("paused", |s, n| Ok(s.boolean(peek(n.page(), n.id, |st| st.paused))));
    def.getter("ended", |s, n| {
        let t = now(n.page());
        let ended = peek(n.page(), n.id, |st| st.at_end(st.position_at(t)));
        Ok(s.boolean(ended && !attr(n.page(), n.id, "loop")))
    });
    def.getter("seeking", |s, n| Ok(s.boolean(peek(n.page(), n.id, |st| st.seeking))));
    def.getter("duration", |s, n| Ok(s.number(peek(n.page(), n.id, |st| st.duration))));
    def.getter("buffered", |s, n| time_ranges(s, n, &loaded_range(n)));
    def.getter("seekable", |s, n| time_ranges(s, n, &loaded_range(n)));
    def.getter("played", |s, n| {
        let t = now(n.page());
        let ranges = peek(n.page(), n.id, |st| {
            let mut ranges = st.played.clone();
            if let Some((_, start)) = st.clock {
                let end = st.position_at(t);
                add_range(&mut ranges, start.min(end), start.max(end));
            }
            ranges
        });
        time_ranges(s, n, &ranges)
    });
    def.getter("textTracks", |s, n| {
        let wrapper = node_value(s, n.page(), Some(n.id))?;
        let wrapper = wrapper.as_object().ok_or(JsError::ContextDestroyed)?;
        if let Some(list) = s.get_hidden(wrapper, "__lpTextTracks")? {
            return Ok(list);
        }
        let list = MODULE.helper(s, n.page(), "textTrackList", &[])?;
        s.set_hidden(wrapper, "__lpTextTracks", list)?;
        Ok(list)
    });
    def.accessor(
        "currentTime",
        |s, n| {
            let t = now(n.page());
            Ok(s.number(peek(n.page(), n.id, |st| {
                if st.ready == HAVE_NOTHING {
                    st.default_start
                } else {
                    st.position_at(t)
                }
            })))
        },
        |_s, n, v| {
            let t = finite(v, "currentTime")?;
            set_current_time(n, t);
            Ok(())
        },
    );
    def.method("fastSeek", 1, |s, n, a| {
        a.require(1, "HTMLMediaElement.fastSeek")?;
        let t = finite(a.get(0), "currentTime")?;
        set_current_time(n, t);
        Ok(s.undefined())
    });
    def.accessor(
        "volume",
        |s, n| Ok(s.number(peek(n.page(), n.id, |st| st.volume))),
        |_s, n, v| {
            let volume = finite(v, "volume")?;
            if !(0.0..=1.0).contains(&volume) {
                return Err(JsError::dom(
                    "IndexSizeError",
                    format!("Failed to set the 'volume' property on 'HTMLMediaElement': The volume provided ({volume}) is outside the range [0, 1]."),
                ));
            }
            let (changed, generation) = with(n.page(), n.id, |st| {
                let changed = st.volume != volume;
                st.volume = volume;
                (changed, st.generation)
            });
            if changed {
                queue_event(&page_rc(n), n.id, generation, "volumechange");
            }
            Ok(())
        },
    );
    def.accessor(
        "muted",
        |s, n| Ok(s.boolean(muted(n.page(), n.id))),
        |_s, n, v| {
            let muted = v.to_bool();
            let attr = attr(n.page(), n.id, "muted");
            let (changed, generation) = with(n.page(), n.id, |st| {
                let changed = st.muted.unwrap_or(attr) != muted;
                st.muted = Some(muted);
                (changed, st.generation)
            });
            if changed {
                queue_event(&page_rc(n), n.id, generation, "volumechange");
            }
            Ok(())
        },
    );
    def.accessor(
        "playbackRate",
        |s, n| Ok(s.number(peek(n.page(), n.id, |st| st.rate))),
        |_s, n, v| {
            let rate = finite(v, "playbackRate")?;
            // Chrome's supported range; 0 pauses.
            if rate != 0.0 && !(0.0625..=16.0).contains(&rate) {
                return Err(JsError::dom(
                    "NotSupportedError",
                    format!("Failed to set the 'playbackRate' property on 'HTMLMediaElement': The provided playback rate ({rate}) is not in the supported playback range."),
                ));
            }
            set_rate(n, rate, false);
            Ok(())
        },
    );
    def.accessor(
        "defaultPlaybackRate",
        |s, n| Ok(s.number(peek(n.page(), n.id, |st| st.default_rate))),
        |_s, n, v| {
            let rate = finite(v, "defaultPlaybackRate")?;
            set_rate(n, rate, true);
            Ok(())
        },
    );
    def.accessor(
        "preservesPitch",
        |s, n| Ok(s.boolean(peek(n.page(), n.id, |st| st.preserves_pitch))),
        |_s, n, v| {
            with(n.page(), n.id, |st| st.preserves_pitch = v.to_bool());
            Ok(())
        },
    );
    def.accessor(
        "srcObject",
        |s, n| {
            let wrapper = node_value(s, n.page(), Some(n.id))?;
            let wrapper = wrapper.as_object().ok_or(JsError::ContextDestroyed)?;
            Ok(s.get_hidden(wrapper, "__lpSrcObject")?.unwrap_or_else(|| s.null()))
        },
        |s, n, v| {
            let wrapper = node_value(s, n.page(), Some(n.id))?;
            let wrapper = wrapper.as_object().ok_or(JsError::ContextDestroyed)?;
            let value = if v.is_undefined() { s.null() } else { v };
            if !value.is_null() && !value.is_object() {
                return Err(JsError::Type(
                    "Failed to set the 'srcObject' property on 'HTMLMediaElement': The provided value is not of type '(Blob or MediaSource or MediaStream)'."
                        .into(),
                ));
            }
            s.set_hidden(wrapper, "__lpSrcObject", value)?;
            // A Blob is read as it is now (a Blob is immutable).
            let object = (!value.is_null()).then(|| crate::file::blob_parts_of(s, value).map(|(bytes, _, _)| bytes));
            with(n.page(), n.id, |st| st.src_object = object);
            load(&page_rc(n), n.id);
            Ok(())
        },
    );
    def.method("load", 0, |s, n, _a| {
        let page = page_rc(n);
        flush(&page, &Rc::downgrade(&page));
        load(&page, n.id);
        Ok(s.undefined())
    });
    def.method("play", 0, |s, n, _a| {
        let page = page_rc(n);
        flush(&page, &Rc::downgrade(&page));
        let rejected = |name: &'static str, message: &'static str| -> JsResult<Value<'_>> {
            let error = JsError::dom(name, message).to_value(s);
            let deferred = s.deferred()?;
            deferred.reject(s, error)?;
            Ok(deferred.promise.get(s))
        };
        // Chrome asks the autoplay policy first.
        if !may_play(&page, n.id) {
            return rejected("NotAllowedError", NOT_ALLOWED);
        }
        if peek(&page, n.id, |st| st.error.is_some_and(|(code, _, _)| code == MEDIA_ERR_SRC_NOT_SUPPORTED)) {
            return rejected("NotSupportedError", NO_SOURCES);
        }
        let deferred = s.deferred()?;
        let promise = deferred.promise.get(s);
        let t = now(&page);
        let (empty, suspended, ended, generation) = with(&page, n.id, |st| {
            st.pending_play.push((s.realm().downgrade(), deferred));
            (st.network == NETWORK_EMPTY, st.suspended.take(), st.at_end(st.position_at(t)), st.generation)
        });
        if empty {
            resource_selection(&page, n.id);
        }
        if ended {
            seek(&page, n.id, 0.0);
        }
        let (was_paused, ready) = with(&page, n.id, |st| {
            let was_paused = st.paused;
            st.paused = false;
            st.can_autoplay = false;
            st.show_poster = false;
            (was_paused, st.ready)
        });
        if let Some((resource, source)) = suspended {
            // preload=none: playback requested, the fetch goes ahead.
            with(&page, n.id, |st| st.network = NETWORK_LOADING);
            set_delaying(&page, n.id, true);
            fetch_resource(&page, n.id, generation, resource, source);
        }
        update_pin(&page, n.id);
        sync_clock(&page, n.id);
        if was_paused {
            queue_event(&page, n.id, generation, "play");
            if ready <= HAVE_CURRENT_DATA {
                queue_event(&page, n.id, generation, "waiting");
            } else {
                let node = n.id;
                queue(&page, node, generation, move |page| notify_playing(page, node, generation));
            }
        } else if ready >= HAVE_FUTURE_DATA {
            let pending = with(&page, n.id, |st| std::mem::take(&mut st.pending_play));
            page.post_task(Box::new(move || resolve_all(pending)));
        }
        Ok(promise)
    });
    def.method("pause", 0, |s, n, _a| {
        let page = page_rc(n);
        flush(&page, &Rc::downgrade(&page));
        internal_pause(&page, n.id);
        Ok(s.undefined())
    });
    def.pure_method("canPlayType", 1, |s, _n, a| {
        a.require(1, "HTMLMediaElement.canPlayType")?;
        let ty = dom_string(a.get(0))?;
        Ok(s.string(media_format::can_play_type(&ty).as_str()))
    });
}

/// The HTMLVideoElement members of frames (`html.rs` keeps the others).
pub(crate) fn define_video(def: &mut ClassDef<crate::html::HtmlVideoElement>) {
    def.method("requestVideoFrameCallback", 1, |s, n, a| {
        a.require(1, "HTMLVideoElement.requestVideoFrameCallback")?;
        let callback = a.get(0);
        if !callback.is_function() {
            return Err(JsError::Type(
                "Failed to execute 'requestVideoFrameCallback' on 'HTMLVideoElement': The callback provided as parameter 1 is not a function."
                    .into(),
            ));
        }
        let page = page_rc(n);
        let persistent = Persistent::new(s, callback);
        let handle = with(&page, n.id, |st| {
            let handle = st.next_frame_callback;
            st.next_frame_callback = st.next_frame_callback.wrapping_add(1).max(1);
            st.frame_callbacks.push((handle, s.realm().downgrade(), persistent));
            handle
        });
        want_frame(&page, n.id);
        Ok(s.number(f64::from(handle)))
    });
    def.method("cancelVideoFrameCallback", 1, |s, n, a| {
        a.require(1, "HTMLVideoElement.cancelVideoFrameCallback")?;
        let handle = a.get(0).to_number()?;
        if peek(n.page(), n.id, |st| !st.frame_callbacks.is_empty()) {
            with(n.page(), n.id, |st| st.frame_callbacks.retain(|(h, _, _)| f64::from(*h) != handle));
        }
        Ok(s.undefined())
    });
    def.method("getVideoPlaybackQuality", 0, |s, n, _a| {
        let total = peek(n.page(), n.id, |st| st.presented_frames);
        MODULE.helper(s, n.page(), "playbackQuality", &[s.number(now(n.page())), s.number(f64::from(total))])
    });
}

fn set_current_time(n: &NodeData, t: f64) {
    let page = page_rc(n);
    if peek(&page, n.id, |st| st.ready) == HAVE_NOTHING {
        // No media data: the default playback start position.
        with(&page, n.id, |st| st.default_start = t);
    } else {
        seek(&page, n.id, t);
    }
}
