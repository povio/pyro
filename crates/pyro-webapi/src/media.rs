//! `HTMLMediaElement` as a browser without an audio device runs it. The
//! resource is fetched through the embedder (a Media request, so CDP and
//! request interception see it) and its container headers are read
//! (`media_format`): duration, video size and tracks. A playable resource
//! then walks the ready states with their events (`durationchange`,
//! `resize`, `loadedmetadata`, `loadeddata`, `canplay`, `canplaythrough`);
//! one that is not fails with MEDIA_ERR_SRC_NOT_SUPPORTED.
//!
//! A `<video>` keeps its resource's bytes when this build decodes video
//! (pyro-media): the frame at the playback position is decoded when a
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
//! `PYRO_AUTOPLAY_POLICY=no-user-gesture-required` lifts the policy (`may_play`).
//!
//! The DOM queues what starts a load (`pyro_dom::media_element`); the queue is
//! drained with the iframe post-connection steps. Per-element state exists
//! only for elements script or a load touched.
//!
//! TimeRanges, MediaError and TextTrackList are made in JS (media.js) on a
//! realm's first use.
#[path = "mse_playback.rs"]
pub(crate) mod mse_playback;
#[path = "track_load.rs"]
mod track_load;
pub(crate) use track_load::{check_track_resource, track_ready_state, track_url};

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use bytes::Bytes;
use pyro_dom::media_element::{MediaChange, MediaEvent};
use pyro_dom::NodeId;
use pyro_js::{ClassDef, Deferred, JsClass, JsError, JsResult, Persistent, Scope, Value, WeakRealm};
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
    url: "pyro://internal/media.js",
    natives: None,
};

/// Adds the lazy interface names to pyro-browser's `Window` class.
pub(crate) fn define_globals<W>(def: &mut ClassDef<W>)
where
    W: JsClass,
    W::Data: WindowHost,
{
    crate::text_tracks::define_globals(def);
    lazy_globals!(def, MODULE, "TimeRanges", "MediaError", "VideoPlaybackQuality");
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
    /// A `srcObject` that is not a Blob (MediaSource): nothing can be read from it.
    Unreadable,
    /// A `srcObject` MediaStream: no fetch, the element is ready with the tracks it has now
    /// (media_stream.js `streamInfo`; the duration is infinite).
    Stream(MediaInfo),
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
    /// The resource has an audio track (`captureStream()` makes a track for it).
    audio: bool,
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
    /// The `srcObject` MediaStream's tracks when it is one.
    stream: Option<MediaInfo>,
    pending_play: Vec<(WeakRealm, Deferred)>,
    /// HTML "can autoplay flag".
    can_autoplay: bool,
    /// The lazy load check let this `loading=lazy` element's next resource selection run.
    lazy_approved: bool,
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
    mse: Option<mse_playback::Attachment>,
    /// `requestVideoFrameCallback` callbacks: handle, realm, callback.
    frame_callbacks: Vec<(u32, WeakRealm, Persistent)>,
    next_frame_callback: u32,
    /// The frame (its index, or the position for a video without a
    /// decoder) last presented to frame callbacks, and how many were.
    presented: Option<f64>,
    presented_frames: u32,
    /// HTML "blocked on its media element's pending text tracks": the load
    /// of this generation stopped at HAVE_CURRENT_DATA until they settle.
    blocked_on_tracks: Option<u32>,
    /// EME: the `MediaKeys` set on this element (`setMediaKeys`), whose sessions' keys decrypt its samples.
    keys: Option<crate::eme::SharedKeys>,
}

/// A `<video>`'s resource, kept to decode frames from: the bytes until the
/// player (made on the first frame asked for) takes them.
struct VideoFrames {
    bytes: Option<Bytes>,
    player: Option<pyro_media::video::Player>,
    /// The response may be read back (`drawImage` keeps a canvas clean).
    clean: bool,
}

impl VideoFrames {
    fn player(&mut self) -> Option<&mut pyro_media::video::Player> {
        if self.player.is_none() {
            // No decoder for the codec, or no samples: the bytes go.
            self.player = pyro_media::video::Player::open(self.bytes.take()?);
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
            audio: false,
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
            stream: None,
            pending_play: Vec::new(),
            can_autoplay: true,
            lazy_approved: false,
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
            mse: None,
            frame_callbacks: Vec::new(),
            next_frame_callback: 1,
            presented: None,
            presented_frames: 0,
            blocked_on_tracks: None,
            keys: None,
        }
    }
}

impl MediaState {
    /// The current playback position at document time `now`.
    fn position_at(&self, now: f64) -> f64 {
        match self.clock {
            Some((t0, p0)) => {
                let p = p0 + (now - t0).max(0.0) / 1000.0 * self.rate;
                let p = self
                    .mse
                    .as_ref()
                    .and_then(|m| m.buffered.iter().find(|&&(a, b)| a <= p0 && p0 < b))
                    .map_or(p, |&(a, b)| p.clamp(a, b));
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
        self.mse.as_ref().is_none_or(|m| m.ended)
            && self.ready >= HAVE_METADATA
            && self.duration.is_finite()
            && position >= self.duration
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
    track_loads: track_load::Loads,
    source_urls: RefCell<FxHashMap<String, Rc<mse_playback::Source>>>,
    /// Entry count at which those of collected elements are dropped.
    sweep_at: Cell<usize>,
    /// Elements whose delaying-the-load-event flag is set.
    delaying_load: Cell<usize>,
    /// `loading=lazy` elements whose resource selection waits for the lazy load check.
    lazy: RefCell<Vec<NodeId>>,
    /// The observation stamp (DOM, style, viewport, scroll) and completely-loaded state at the
    /// last lazy load check.
    lazy_checked: Cell<Option<(((u64, u64, u32, u32), u64), bool)>>,
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
    let detached = if !elements.contains_key(&node) && elements.len() >= page.media.sweep_at.get() {
        sweep(page, &mut elements)
    } else {
        Vec::new()
    };
    let result = f(elements.entry(node).or_default());
    drop(elements);
    for attachment in detached {
        attachment.detach();
    }
    result
}

/// Drops the state of collected elements (a pinned element is alive).
fn sweep(page: &PageDom, elements: &mut FxHashMap<NodeId, MediaState>) -> Vec<mse_playback::Attachment> {
    let mut detached = Vec::new();
    let Ok(dom) = page.dom.try_borrow() else { return detached };
    let delaying = page.media.delaying_load.get();
    elements.retain(|id, st| {
        let keep = st.pinned || dom.is_alive(*id);
        if !keep {
            detached.extend(st.mse.take());
        }
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
    detached
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

/// Chrome's autoplay policy: muted media, or a document with sticky user activation. With
/// `PYRO_AUTOPLAY_POLICY=no-user-gesture-required` in the environment (Chrome's
/// `--autoplay-policy` of that name, which wptrunner starts Chrome with) any media may play:
/// a WPT page with `<video autoplay>` otherwise plays only if a client evaluation (a user
/// gesture) happens to reach the document before its media has enough data.
fn may_play(page: &PageDom, node: NodeId) -> bool {
    !frame_hidden_by_policy(page) && (muted(page, node) || autoplay_allowed(page))
}

/// Whether the document's permissions policy disables `media-playback-while-not-visible` and
/// its frame is not rendered now (https://wicg.github.io/iframe-media-pausing/): its media may
/// not play and its audio contexts do not run. Costs one `RefCell` read for any other document.
pub(crate) fn frame_hidden_by_policy(page: &PageDom) -> bool {
    !page.media_while_hidden_allowed() && !frame_rendered(page)
}

/// Whether the document's frame is rendered: every frame owner up to the top-level document is
/// connected, not `display: none` (no box), has `visibility: visible` and a content box that is
/// not empty. A frame merely scrolled out of the viewport is rendered. A top-level (or removed)
/// document counts as rendered.
pub(crate) fn frame_rendered(page: &PageDom) -> bool {
    let options = pyro_dom::style::CheckVisibilityOptions { check_visibility: true, check_opacity: false };
    let mut container = page.container();
    loop {
        let crate::page::FrameContainer::Frame(parent, owner) = container else { return true };
        let shown = {
            let pdom = parent.dom.borrow();
            let layout = parent.layout();
            let shown = pdom.is_connected(owner).unwrap_or(false)
                && layout.check_visibility(&pdom, owner, options)
                && layout.content_size(&pdom, owner).is_some_and(|(w, h)| w > 0.0 && h > 0.0);
            shown
        };
        if !shown {
            return false;
        }
        container = parent.container();
    }
}

/// The checkpoint pass for a document whose policy disables `media-playback-while-not-visible`:
/// when its frame became rendered or not rendered since the last pass, a task pauses its playing
/// media (not rendered) and tells its audio contexts (`webaudio.js` `frameRendered`).
pub fn refresh_frame_rendered(page: &Rc<PageDom>) {
    if page.media_while_hidden_allowed() || page.is_disposed() {
        return;
    }
    let rendered = frame_rendered(page);
    if page.media_frame_rendered.replace(rendered) == rendered {
        // A context a lazy check interrupted before any pass saw the frame hidden: resume it now.
        if rendered && page.media_audio_blocked.replace(false) {
            let weak = Rc::downgrade(page);
            page.post_task(Box::new(move || {
                if let Some(page) = weak.upgrade() {
                    if !page.is_disposed() && page.media_frame_rendered.get() {
                        crate::webaudio::frame_rendered_changed(&page, true);
                    }
                }
            }));
        }
        return;
    }
    page.media_audio_blocked.set(false);
    let weak = Rc::downgrade(page);
    page.post_task(Box::new(move || {
        let Some(page) = weak.upgrade() else { return };
        // A later pass may have changed it again: its own task follows.
        if page.is_disposed() || page.media_frame_rendered.get() != rendered {
            return;
        }
        if !rendered {
            // "Pause" every playing media element of the document (internal pause steps).
            let playing: Vec<NodeId> =
                page.media.elements.borrow().iter().filter(|(_, st)| !st.paused).map(|(id, _)| *id).collect();
            for node in playing {
                internal_pause(&page, node);
            }
        }
        crate::webaudio::frame_rendered_changed(&page, rendered);
    }));
}

/// A document whose sticky activation is set, or any document under
/// `PYRO_AUTOPLAY_POLICY=no-user-gesture-required` (shared with Web Audio's `AudioContext`
/// autoplay check, which Chrome gates by the same flag).
pub(crate) fn autoplay_allowed(page: &PageDom) -> bool {
    static NO_GESTURE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    page.has_sticky_activation()
        || *NO_GESTURE
            .get_or_init(|| std::env::var("PYRO_AUTOPLAY_POLICY").is_ok_and(|v| v == "no-user-gesture-required"))
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
    page.media.track_loads.clear();
    page.media.source_urls.borrow_mut().clear();
    let states = std::mem::take(&mut *page.media.elements.borrow_mut());
    page.media.delaying_load.set(0);
    let hooks = page.hooks();
    let mut pinned = Vec::new();
    for (node, mut st) in states {
        if let (Some(id), Some(hooks)) = (st.timer.take(), &hooks) {
            hooks.clear_media_timer(id);
        }
        drop(st.fetch.take());
        if let Some(attachment) = st.mse.take() {
            attachment.detach();
        }
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
        MediaChange::TrackChildrenChanged(track) => {
            track_load::sync_parent(page, node);
            check_track_resource(page, track);
            text_tracks_settled(page, node);
        }
        MediaChange::TrackChanged => {
            let parent = track_load::media_parent(page, node);
            if let Some(parent) = parent {
                track_load::sync_parent(page, parent);
            }
            check_track_resource(page, node);
            if let Some(parent) = parent {
                text_tracks_settled(page, parent);
            }
        }
        MediaChange::Src => load(page, node),
        MediaChange::Inserted => {
            let (empty, has_object) = peek(page, node, |st| (st.network == NETWORK_EMPTY, st.src_object.is_some()));
            if empty && (has_object || has_src(page, node) || !source_children(page, node).is_empty()) {
                resource_selection(page, node);
            }
        }
        MediaChange::Removed => {
            // HTML removing steps: "await a stable state" (the microtask checkpoint after the
            // removing script, not synchronously: `paused` stays false in that script), then an
            // element still not in a document runs the internal pause steps. `paused` is true
            // before any task queued meanwhile runs; the `pause` event is a task of its own.
            let weak = Rc::downgrade(page);
            let pause = move || {
                let Some(page) = weak.upgrade() else { return };
                if page.is_disposed() || !page.dom.borrow().is_alive(node) {
                    return;
                }
                let connected = page.dom.borrow().is_connected(node).unwrap_or(false);
                if !connected && !peek(&page, node, |st| st.paused) {
                    internal_pause(&page, node);
                }
            };
            removed_at_stable_state(page, pause);
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

/// HTML "show poster flag": no text track cue is current while it is set.
pub(crate) fn show_poster(page: &PageDom, node: NodeId) -> bool {
    peek(page, node, |st| st.show_poster)
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
/// Runs `f` at the next stable state: a microtask of the page's main world, so after the
/// running script and before any task it queued; without a scriptable realm, a task.
fn removed_at_stable_state(page: &Rc<PageDom>, f: impl FnOnce() + 'static) {
    let f = std::cell::Cell::new(Some(f));
    let run = std::rc::Rc::new(move || {
        if let Some(f) = f.take() {
            f();
        }
    });
    let queued = page.main_world_realm().is_some_and(|realm| {
        realm.scope().is_ok_and(|s| {
            let run = run.clone();
            s.then(s.undefined(), move |_, _| run(), |_, _| {}).is_ok()
        })
    });
    if !queued {
        page.post_task(Box::new(move || run()));
    }
}

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
                plan = Some((remaining.min(TIMEUPDATE_MS), st.timer_serial, st.position, st.rate));
            }
        }
        (old, plan)
    });
    let hooks = page.hooks();
    if let (Some(id), Some(hooks)) = (old, &hooks) {
        hooks.clear_media_timer(id);
    }
    want_frame(page, node);
    let Some((delay, serial, position, rate)) = plan else { return };
    let cue_delay = (crate::text_tracks::next_pause(page, node, position) - position) / rate * 1000.;
    let delay = delay.min(cue_delay);
    let weak = Rc::downgrade(page);
    let task: Box<dyn FnOnce()> = Box::new(move || {
        if let Some(page) = weak.upgrade() {
            if !page.is_disposed() {
                tick(&page, node, serial);
            }
        }
    });
    let delay = crate::util::delay_from_ms(delay);
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
        Some(st.at_end(position) || remaining < 0.5 && st.mse.as_ref().is_none_or(|m| m.ended))
    });
    match ended {
        None => {}
        Some(true) => reached_end(page, node),
        Some(false) => {
            if peek(page, node, |st| st.mse.is_some()) {
                mse_playback::refresh(page, node);
            }
            if crate::text_tracks::playback(page, node, true) {
                internal_pause(page, node);
                return;
            }
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
    if crate::text_tracks::playback(page, node, true) { internal_pause(page, node); }
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
    crate::text_tracks::playback(page, node, false);
    queue_event(page, node, generation, "seeking");
    if peek(page, node, |st| st.mse.is_some()) {
        mse_playback::refresh(page, node);
    } else {
        complete_seek(page, node, generation, serial);
    }
}

fn complete_seek(page: &Rc<PageDom>, node: NodeId, generation: u32, serial: u32) {
    queue(page, node, generation, move |page| {
        let current = with(page, node, |st| {
            let current = st.seek_serial == serial && st.seeking && (st.mse.is_none() || st.ready >= HAVE_CURRENT_DATA);
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
    // The cues current at the playback position are active by `playing`.
    crate::text_tracks::playback(page, node, false);
    // HTML takes the pending play promises before `playing` fires: a `pause()` in its
    // listener rejects none of them, and they resolve after it.
    let pending = with(page, node, |st| std::mem::take(&mut st.pending_play));
    if !fire(page, node, generation, "playing") {
        with(page, node, |st| {
            let later = std::mem::replace(&mut st.pending_play, pending);
            st.pending_play.extend(later);
        });
        return;
    }
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
    if let Some(attachment) = with(page, node, |st| st.mse.take()) {
        attachment.detach();
    }
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
    // The show poster flag is set again: no cue stays active.
    crate::text_tracks::playback(page, node, false);
    resource_selection(page, node);
}

/// HTML "resource selection algorithm": the synchronous part, then a task
/// for the rest (after "await a stable state").
fn resource_selection(page: &Rc<PageDom>, node: NodeId) {
    let approved = peek(page, node, |st| st.lazy_approved);
    if defer_lazy(page, node) {
        // A deferred element does not delay the load event (a previous load may have set it).
        set_delaying(page, node, false);
        return;
    }
    let generation = with(page, node, |st| {
        st.network = NETWORK_NO_SOURCE;
        st.awaiting_source = false;
        st.generation
    });
    set_delaying(page, node, true);
    queue(page, node, generation, move |page| {
        // After "await a stable state" the element's `loading` is what script left it as: a
        // `src` set before `loading=lazy` in one task is a lazy element too (WPT
        // video-loading-lazy-below-viewport-load-event). An element the lazy load check let go
        // is not deferred again.
        if !approved && defer_lazy(page, node) {
            with(page, node, |st| st.network = NETWORK_EMPTY);
            set_delaying(page, node, false);
            return;
        }
        select(page, node, generation)
    });
}

/// HTML "will lazy load element steps" for a media element (`loading=lazy`, scripting
/// enabled): resource selection waits for the lazy load check (`lazy_after_task`), the
/// element staying at NETWORK_EMPTY without delaying the load event. False when the check let
/// it go, or it is not lazy.
fn defer_lazy(page: &Rc<PageDom>, node: NodeId) -> bool {
    if page.scripting_disabled() || !is_lazy_media(&page.dom.borrow(), node) {
        return false;
    }
    if with(page, node, |st| std::mem::take(&mut st.lazy_approved)) {
        return false;
    }
    let mut lazy = page.media.lazy.borrow_mut();
    if !lazy.contains(&node) {
        lazy.push(node);
    }
    page.media.lazy_checked.set(None);
    true
}

fn is_lazy_media(dom: &pyro_dom::Dom, node: NodeId) -> bool {
    dom.element(node).is_ok_and(|e| {
        e.is_html()
            && matches!(&*e.name.local, "video" | "audio")
            && e.attr("loading").is_some_and(|l| l.eq_ignore_ascii_case("lazy"))
    })
}

/// After each task: the deferred lazy media elements that stopped being lazy, or that
/// intersect the viewport expanded by the lazy load margin once the document has completely
/// loaded, run resource selection.
pub(crate) fn lazy_after_task(page: &Rc<PageDom>) {
    if page.media.lazy.borrow().is_empty() || page.is_disposed() {
        return;
    }
    let loaded = page.is_completely_loaded();
    // The layout first: it may sync the viewport, which touches the DOM.
    let stamp = || {
        let layout = page.layout();
        let dom = page.dom.try_borrow().ok()?;
        Some((layout.observation_stamp(&dom), loaded))
    };
    let Some(now) = stamp() else { return };
    if page.media.lazy_checked.get() == Some(now) {
        return;
    }
    let nodes: Vec<NodeId> = page.media.lazy.borrow().clone();
    for node in nodes {
        let (alive, connected, lazy) = {
            let dom = page.dom.borrow();
            (dom.is_alive(node), dom.is_connected(node).unwrap_or(false), is_lazy_media(&dom, node))
        };
        if !alive {
            page.media.lazy.borrow_mut().retain(|n| *n != node);
            continue;
        }
        let go = !lazy || (connected && loaded && crate::intersection_observer::lazy_load_intersects(page, node));
        if go {
            page.media.lazy.borrow_mut().retain(|n| *n != node);
            with(page, node, |st| st.lazy_approved = true);
            resource_selection(page, node);
        }
    }
    page.media.lazy_checked.set(stamp());
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
            None => match peek(page, node, |st| st.stream) {
                Some(info) => Resource::Stream(info),
                None => Resource::Unreadable,
            },
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
    // Fetch "potentially dangling markup" (as Chrome): a URL written with a newline or tab
    // (which the URL parser strips) and a `<` is blocked, so the source fails.
    if value.contains(['\n', '\r', '\t']) && value.contains('<') {
        return None;
    }
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
        // HTML: a `media` attribute that does not match the environment
        // fails the candidate (evaluated now; later changes do not react).
        let media = page.dom.borrow().get_attribute(source, "media").ok().flatten().map(str::to_owned);
        let media_ok = media.as_deref().is_none_or(|m| {
            m.trim().is_empty() || page.layout().style().matches_media_list(&pyro_dom::cssom::MediaQueryList::parse(m))
        });
        let url = src.filter(|s| !s.is_empty() && supported && media_ok).and_then(|s| resolve(page, source, &s));
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
    // HTML "waiting": the network state changes now (an `error` queued above
    // already sees it); the load event stops waiting in a task.
    with(page, node, |st| {
        st.network = NETWORK_NO_SOURCE;
        st.awaiting_source = true;
    });
    queue(page, node, generation, move |page| {
        if peek(page, node, |st| st.awaiting_source) {
            set_delaying(page, node, false);
        }
    });
}

/// HTML "resource fetch algorithm" for `resource` (from `source` in children mode).
fn fetch_resource(page: &Rc<PageDom>, node: NodeId, generation: u32, resource: Resource, source: Option<NodeId>) {
    if let Resource::Url(url) = &resource {
        if url.starts_with("blob:") {
            if let Some(attached) = mse_playback::attach(page, node, generation, url) {
                if !attached {
                    fetched(page, node, generation, source, Seek::default(), Err(FORMAT_ERROR.into()));
                }
                return;
            }
        }
    }
    // HTML: the deferred load steps of `preload=none` do not apply to a MediaStream.
    if matches!(resource, Resource::Stream(_)) {
        start_fetch(page, node, generation, resource, source, Seek::default());
        return;
    }
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
        Resource::Stream(info) => {
            let weak = Rc::downgrade(page);
            page.post_task(Box::new(move || {
                let Some(page) = weak.upgrade() else { return };
                if page.is_disposed() || peek(&page, node, |st| st.generation) != generation {
                    return;
                }
                with(&page, node, |st| st.fetching = false);
                update_pin(&page, node);
                media_ready(&page, node, generation, info);
            }));
        }
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
            let decoders = pyro_media::video::AV1 || pyro_media::video::H264;
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
                // A candidate removed or moved away meanwhile gets no `error`.
                if page.dom.borrow().parent(source).ok().flatten() == Some(node) {
                    crate::input::fire_simple(page, source, "error", false, false);
                }
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
        st.audio = info.audio;
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
    let (start, url) = with(page, node, |st| (std::mem::take(&mut st.default_start), st.current_src.clone()));
    // HTML: a start time the URL indicates (Media Fragments `#t=`) is the initial position.
    let start = if start != 0.0 { start } else { media_fragment_start(&url).unwrap_or(0.0) };
    if start != 0.0 {
        seek(page, node, start);
    }
    queue(page, node, generation, move |page| {
        // A MediaStream has no buffering: it has enough data as soon as it has any.
        with(page, node, |st| st.ready = if st.stream.is_some() { HAVE_ENOUGH_DATA } else { HAVE_CURRENT_DATA });
        set_delaying(page, node, false);
        want_frame(page, node);
        if fire(page, node, generation, "loadeddata") {
            past_current_data(page, node, generation);
        }
    });
}

/// The start of a Media Fragments URI temporal dimension in `url`'s fragment (`t=[npt:]start[,end]`,
/// name and value percent-decoded; seconds, `mm:ss` or `hh:mm:ss`, with a fraction), if any.
fn media_fragment_start(url: &str) -> Option<f64> {
    let (_, fragment) = url.split_once('#')?;
    let decode = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8().ok().map(|c| c.into_owned());
    let value = fragment.split('&').rev().find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (decode(name)? == "t").then(|| decode(value)).flatten()
    })?;
    let value = value.strip_prefix("npt:").unwrap_or(&value);
    let start = value.split(',').next()?;
    if start.is_empty() {
        return None;
    }
    let mut seconds = 0.0;
    for part in start.split(':') {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
            return None;
        }
        seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
    }
    (seconds.is_finite() && seconds > 0.0).then_some(seconds)
}

/// After HAVE_CURRENT_DATA: the ready states that follow, one task each,
/// unless pending text tracks block the element there (HTML: until each
/// track whose mode is not disabled has loaded or failed).
fn past_current_data(page: &Rc<PageDom>, node: NodeId, generation: u32) {
    if track_load::pending_tracks(page, node) {
        with(page, node, |st| st.blocked_on_tracks = Some(generation));
        return;
    }
    queue(page, node, generation, move |page| {
        let playing = with(page, node, |st| {
            st.ready = st.ready.max(HAVE_FUTURE_DATA);
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
        // A MediaStream is never fully fetched: the element keeps loading and does not suspend.
        if peek(page, node, |st| st.stream.is_some()) {
            return;
        }
        with(page, node, |st| st.network = NETWORK_IDLE);
        fire(page, node, generation, "suspend");
    });
}

/// A text track of `media` loaded, failed, changed or went: a load blocked
/// on pending text tracks goes on once none is left.
pub(crate) fn text_tracks_settled(page: &Rc<PageDom>, media: NodeId) {
    let Some(generation) = peek(page, media, |st| st.blocked_on_tracks) else { return };
    if peek(page, media, |st| st.generation) != generation {
        with(page, media, |st| st.blocked_on_tracks = None);
        return;
    }
    if track_load::pending_tracks(page, media) {
        return;
    }
    with(page, media, |st| st.blocked_on_tracks = None);
    past_current_data(page, media, generation);
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

/// EME `setMediaKeys` (after eme.js checked the argument): the element decrypts with `keys` from now on and
/// attempts to resume playback it was blocked on.
pub(crate) fn set_media_keys(page: &Rc<PageDom>, node: NodeId, keys: Option<crate::eme::SharedKeys>) {
    let has_mse = with(page, node, |st| {
        st.keys = keys;
        let keys = st.keys.clone();
        if let Some(m) = st.mse.as_mut() {
            m.presentation.set_keys(keys);
            m.reset_waiting_key();
        }
        st.mse.is_some()
    });
    if has_mse {
        mse_playback::refresh(page, node);
    }
}

/// A session of `store` learned or lost keys: elements blocked on a missing key attempt to resume.
pub(crate) fn keys_changed(page: &Rc<PageDom>, store: &crate::eme::SharedKeys) {
    let nodes: Vec<NodeId> = page
        .media
        .elements
        .borrow()
        .iter()
        .filter(|(_, st)| st.mse.is_some() && st.keys.as_ref().is_some_and(|k| Rc::ptr_eq(k, store)))
        .map(|(node, _)| *node)
        .collect();
    for node in nodes {
        mse_playback::refresh(page, node);
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

/// The frame a `<video>` shows now, decoded (pyro-media), and whether its
/// pixels may be read back: `None` before `HAVE_CURRENT_DATA`, or without a
/// decoder for its video. `drawImage`, `createImageBitmap` and screenshots
/// use it.
pub(crate) fn current_frame(page: &PageDom, node: NodeId) -> Option<(pyro_media::video::Frame, bool)> {
    let t = now(page);
    let mut elements = page.media.elements.borrow_mut();
    let st = elements.get_mut(&node)?;
    if st.ready < HAVE_CURRENT_DATA {
        return None;
    }
    let position = st.position_at(t);
    if let Some(m) = st.mse.as_mut() {
        return m.video_frame(position, st.duration).map(|f| (f, true));
    }
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
        self.video_frames().into_iter().map(|(bitmap, _)| bitmap).collect()
    }

    /// [`PageDom::video_bitmaps`], each with whether its pixels may be read back (`current_frame`'s
    /// `clean`: false for media of another origin without CORS), which the HTML-in-Canvas privacy
    /// paint needs to taint the canvas.
    pub fn video_frames(&self) -> Vec<(crate::canvas::CanvasBitmap, bool)> {
        let candidates: Vec<(NodeId, bool)> = {
            let elements = self.media.elements.borrow();
            if elements.is_empty() {
                return Vec::new();
            }
            elements
                .iter()
                .filter(|(_, st)| {
                    (st.frames.is_some() || st.mse.is_some() && st.video.0 > 0) && st.ready >= HAVE_CURRENT_DATA
                })
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
            if let Some((frame, clean)) = current_frame(self, node) {
                out.push(((node, frame.width, frame.height, frame.rgba), clean));
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
    if let Some(m) = st.mse.as_mut() {
        return m.video_frame(position, st.duration).map(|f| (f.time, f.time, (f.width, f.height)));
    }
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
    let t = now(n.page());
    peek(n.page(), n.id, |st| {
        if st.ready < HAVE_METADATA {
            return (0, 0);
        }
        st.mse.as_ref().map_or(st.video, |m| m.presentation.dimensions(st.position_at(t)))
    })
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

pub(crate) fn mse_time_ranges<'s>(s: &Scope<'s>, flat: Value<'s>) -> JsResult<Value<'s>> {
    MODULE.helper(s, &crate::target::page_of(s)?, "timeRanges", &[flat])
}

/// What `captureStream()` (media_stream.js) can capture from a media element now: bit 0 an audio
/// track, bit 1 a video track, bit 2 the metadata is loaded.
pub(crate) fn capture_kinds<'s>(s: &Scope<'s>, element: Value<'s>) -> JsResult<Value<'s>> {
    let n = s.unwrap::<crate::node::Node>(element).ok_or_else(|| JsError::Type("The provided value is not of type 'HTMLMediaElement'.".into()))?;
    let kinds = peek(n.page(), n.id, |st| {
        if st.ready < HAVE_METADATA {
            return 0u32;
        }
        let video = st.video != (0, 0);
        4 | u32::from(st.audio) | (u32::from(video) << 1)
    });
    Ok(s.number(f64::from(kinds)))
}

fn time_ranges<'s>(s: &Scope<'s>, n: &NodeData, ranges: &[(f64, f64)]) -> JsResult<Value<'s>> {
    let flat: Vec<Value<'s>> = ranges.iter().flat_map(|&(a, b)| [s.number(a), s.number(b)]).collect();
    let list = s.array(&flat)?;
    MODULE.helper(s, n.page(), "timeRanges", &[list.value()])
}

/// The whole resource once it is loaded: every byte is buffered and seekable.
fn loaded_range(n: &NodeData, seekable: bool) -> Vec<(f64, f64)> {
    peek(n.page(), n.id, |st| {
        if let Some(m) = &st.mse {
            return if seekable { m.seekable.clone() } else { m.buffered.clone() };
        }
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
    def.getter("buffered", |s, n| time_ranges(s, n, &loaded_range(n, false)));
    def.getter("seekable", |s, n| time_ranges(s, n, &loaded_range(n, true)));
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
    def.getter("textTracks", |s, n| crate::text_tracks::element_helper(s, n, "syncMedia", &[]));
    def.method("addTextTrack", 1, |s, n, a| {
        a.require(1, "addTextTrack")?;
        crate::text_tracks::element_helper(s, n, "addTextTrack", &[a.get(0), a.get(1), a.get(2)])
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
            // A MediaStream plays the tracks it has now: [audio tracks, video tracks, width, height].
            let mut stream = None;
            if matches!(object, Some(None)) {
                let info = crate::media_stream::stream_helper(s, n.page(), "streamInfo", &[value])?;
                if let Some(list) = info.as_object() {
                    let num = |i: u32| -> JsResult<f64> { list.get_index(i)?.to_number() };
                    let (audio, video, w, h) = (num(0)?, num(1)?, num(2)? as u32, num(3)? as u32);
                    stream = Some(MediaInfo {
                        duration: f64::INFINITY,
                        video: (video > 0.0).then_some((w, h)),
                        audio: audio > 0.0,
                    });
                }
            }
            with(n.page(), n.id, |st| {
                st.src_object = object;
                st.stream = stream;
            });
            load(&page_rc(n), n.id);
            Ok(())
        },
    );
    // Media Capture from DOM Elements: a MediaStream whose tracks follow what the element plays.
    def.method("captureStream", 0, |s, n, _a| {
        let wrapper = node_value(s, n.page(), Some(n.id))?;
        crate::media_stream::stream_helper(s, n.page(), "captureElement", &[wrapper])
    });
    def.accessor(
        "onencrypted",
        |s, n| crate::target::event_handler(s, &n.target, "encrypted"),
        |s, n, v| crate::target::set_event_handler(s, &n.target, "encrypted", v),
    );
    def.accessor(
        "onwaitingforkey",
        |s, n| crate::target::event_handler(s, &n.target, "waitingforkey"),
        |s, n, v| crate::target::set_event_handler(s, &n.target, "waitingforkey", v),
    );
    def.getter("mediaKeys", |s, n| crate::eme::media_keys(s, n.page(), n.id));
    def.method("setMediaKeys", 1, |s, n, a| {
        let page = page_rc(n);
        crate::eme::set_media_keys(s, &page, n.id, a.get(0), a.len())
    });
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
        let (was_paused, ready, poster) = with(&page, n.id, |st| {
            let was_paused = st.paused;
            let poster = std::mem::replace(&mut st.show_poster, false);
            st.paused = false;
            st.can_autoplay = false;
            (was_paused, st.ready, poster)
        });
        // HTML play(): clearing the show poster flag runs "time marches on".
        if poster {
            crate::text_tracks::playback(&page, n.id, false);
        }
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
