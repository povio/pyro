//! `HTMLMediaElement` as a browser without media decoding runs it: the
//! IDL state of a real element (currentTime, volume, muted, playback rates,
//! paused, networkState, error, TimeRanges) and the load, resource
//! selection and play/pause algorithms, where every resource fails as
//! unsupported. Setting `src` fires `loadstart`, then `error` with a
//! `MediaError` of code 4 (MEDIA_ERR_SRC_NOT_SUPPORTED); `<source>` children
//! each get an `error` event; `play()` fires `play` and `waiting` and its
//! promise is rejected with NotSupportedError when the source fails (or
//! AbortError by `pause()`/a new load). Nothing is fetched: no resource
//! could be decoded.
//!
//! The DOM queues what starts a load (`lp_dom::media_element`); the queue is
//! drained with the iframe post-connection steps. Per-element state exists
//! only for elements script or a load touched.
//!
//! TimeRanges, MediaError and TextTrackList are made in JS (media.js) on a
//! realm's first use.
use std::cell::RefCell;
use std::rc::{Rc, Weak};

use lp_dom::media_element::{MediaChange, MediaEvent};
use lp_dom::NodeId;
use lp_js::{ClassDef, Deferred, JsClass, JsError, JsResult, Scope, Value, WeakRealm};
use rustc_hash::FxHashMap;

use crate::html::HtmlMediaElement;
use crate::lazy_module::{lazy_globals, LazyModule};
use crate::node::{node_value, NodeData};
use crate::page::PageDom;
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
    lazy_globals!(def, MODULE, "TimeRanges", "MediaError", "TextTrackList");
}

const NETWORK_EMPTY: u8 = 0;
const NETWORK_IDLE: u8 = 1;
const NETWORK_LOADING: u8 = 2;
const NETWORK_NO_SOURCE: u8 = 3;
const MEDIA_ERR_SRC_NOT_SUPPORTED: u16 = 4;
const UNSUPPORTED: &str = "MEDIA_ELEMENT_ERROR: Format error";
const NO_SOURCES: &str = "The element has no supported sources.";

struct MediaState {
    network: u8,
    paused: bool,
    /// HTML "default playback start position": what `currentTime` sets while
    /// there is no media data.
    default_start: f64,
    volume: f64,
    /// The muted IDL attribute once set; until then the `muted` content
    /// attribute's presence.
    muted: Option<bool>,
    rate: f64,
    default_rate: f64,
    preserves_pitch: bool,
    /// The current MediaError: code and a serial for its object's identity.
    error: Option<(u16, u32)>,
    error_serial: u32,
    /// Bumped by each load: tasks of an earlier load do nothing.
    generation: u32,
    /// Resource selection ran out of `<source>` children and waits for more.
    awaiting_source: bool,
    current_src: String,
    has_src_object: bool,
    pending_play: Vec<(WeakRealm, Deferred)>,
}

impl Default for MediaState {
    fn default() -> Self {
        MediaState {
            network: NETWORK_EMPTY,
            paused: true,
            default_start: 0.0,
            volume: 1.0,
            muted: None,
            rate: 1.0,
            default_rate: 1.0,
            preserves_pitch: true,
            error: None,
            error_serial: 0,
            generation: 0,
            awaiting_source: false,
            current_src: String::new(),
            has_src_object: false,
            pending_play: Vec::new(),
        }
    }
}

/// The media state of a page's elements, made on an element's first use.
#[derive(Default)]
pub(crate) struct MediaElements {
    elements: RefCell<FxHashMap<NodeId, MediaState>>,
}

fn with<R>(page: &PageDom, node: NodeId, f: impl FnOnce(&mut MediaState) -> R) -> R {
    f(page.media.elements.borrow_mut().entry(node).or_default())
}

/// Reads state without making it: an untouched element's defaults.
fn peek<R>(page: &PageDom, node: NodeId, f: impl FnOnce(&MediaState) -> R) -> R {
    match page.media.elements.borrow().get(&node) {
        Some(state) => f(state),
        None => f(&MediaState::default()),
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
    let mut dom = page.dom.borrow_mut();
    for event in events {
        if let MediaChange::SourceInserted(source) = event.change {
            dom.release(source);
        }
        dom.release(event.node);
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
            let (empty, has_object) = peek(page, node, |st| (st.network == NETWORK_EMPTY, st.has_src_object));
            if empty && (has_object || has_src(page, node) || !source_children(page, node).is_empty()) {
                resource_selection(page, node);
            }
        }
        MediaChange::SourceInserted(source) => {
            let (network, awaiting, generation) =
                peek(page, node, |st| (st.network, st.awaiting_source, st.generation));
            if network == NETWORK_EMPTY {
                resource_selection(page, node);
            } else if awaiting {
                // The waiting resource selection tries the new candidate.
                with(page, node, |st| st.network = NETWORK_LOADING);
                let src = source_url(page, source);
                queue(page, node, generation, move |page| {
                    with(page, node, |st| st.current_src = src);
                    crate::input::fire_simple(page, source, "error", false, false);
                    with(page, node, |st| st.network = NETWORK_NO_SOURCE);
                });
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

fn source_url(page: &PageDom, source: NodeId) -> String {
    let dom = page.dom.borrow();
    match dom.get_attribute(source, "src").ok().flatten() {
        Some(src) if !src.is_empty() => crate::html::resolve_url(&dom, source, src),
        _ => String::new(),
    }
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

/// Rejects promises (in their realms) with a DOMException.
fn reject_all(promises: Vec<(WeakRealm, Deferred)>, name: &'static str, message: &'static str) {
    for (realm, deferred) in promises {
        let Some(realm) = realm.upgrade() else { continue };
        let Ok(s) = realm.scope() else { continue };
        let error = JsError::dom(name, message).to_value(&s);
        let _ = deferred.reject(&s, error);
    }
}

/// Queues "reject pending play promises" for `promises`.
fn queue_reject(page: &Rc<PageDom>, promises: Vec<(WeakRealm, Deferred)>, name: &'static str, message: &'static str) {
    if promises.is_empty() {
        return;
    }
    page.post_task(Box::new(move || reject_all(promises, name, message)));
}

/// HTML "media element load algorithm".
fn load(page: &Rc<PageDom>, node: NodeId) {
    let (network, generation) = with(page, node, |st| {
        st.generation = st.generation.wrapping_add(1);
        (st.network, st.generation)
    });
    if matches!(network, NETWORK_LOADING | NETWORK_IDLE) {
        queue_event(page, node, generation, "abort");
    }
    if network != NETWORK_EMPTY {
        queue_event(page, node, generation, "emptied");
        let (aborted, seeked) = with(page, node, |st| {
            st.network = NETWORK_EMPTY;
            st.awaiting_source = false;
            let aborted = if st.paused {
                Vec::new()
            } else {
                st.paused = true;
                std::mem::take(&mut st.pending_play)
            };
            let seeked = st.default_start != 0.0;
            st.default_start = 0.0;
            (aborted, seeked)
        });
        queue_reject(page, aborted, "AbortError", "The play() request was interrupted by a new load request.");
        if seeked {
            queue_event(page, node, generation, "timeupdate");
        }
    }
    with(page, node, |st| {
        st.rate = st.default_rate;
        st.error = None;
        st.current_src.clear();
    });
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
    queue(page, node, generation, move |page| select(page, node, generation));
}

fn select(page: &Rc<PageDom>, node: NodeId, generation: u32) {
    let has_object = peek(page, node, |st| st.has_src_object);
    let src = page.dom.borrow().get_attribute(node, "src").ok().flatten().map(str::to_owned);
    let sources = if has_object || src.is_some() { Vec::new() } else { source_children(page, node) };
    if !has_object && src.is_none() && sources.is_empty() {
        with(page, node, |st| st.network = NETWORK_EMPTY);
        return;
    }
    with(page, node, |st| st.network = NETWORK_LOADING);
    queue_event(page, node, generation, "loadstart");
    if has_object || src.is_some() {
        // Attribute or object mode: the resource cannot be decoded.
        let current = match src.as_deref() {
            Some(v) if !has_object && !v.is_empty() => crate::html::resolve_url(&page.dom.borrow(), node, v),
            _ => String::new(),
        };
        with(page, node, |st| st.current_src = current);
        queue(page, node, generation, move |page| media_source_failed(page, node));
        return;
    }
    // Children mode: each candidate fails at its `<source>`; then the
    // algorithm waits for another one.
    for source in sources {
        let url = source_url(page, source);
        queue(page, node, generation, move |page| {
            with(page, node, |st| st.current_src = url);
            crate::input::fire_simple(page, source, "error", false, false);
        });
    }
    queue(page, node, generation, move |page| {
        with(page, node, |st| {
            st.network = NETWORK_NO_SOURCE;
            st.awaiting_source = true;
        });
    });
}

/// HTML "dedicated media source failure steps".
fn media_source_failed(page: &Rc<PageDom>, node: NodeId) {
    let pending = with(page, node, |st| {
        st.error_serial = st.error_serial.wrapping_add(1);
        st.error = Some((MEDIA_ERR_SRC_NOT_SUPPORTED, st.error_serial));
        st.network = NETWORK_NO_SOURCE;
        std::mem::take(&mut st.pending_play)
    });
    crate::input::fire_simple(page, node, "error", false, false);
    reject_all(pending, "NotSupportedError", "Failed to load because no supported source was found.");
}

fn finite(v: Value<'_>, what: &str) -> JsResult<f64> {
    let n = v.to_number()?;
    if !n.is_finite() {
        return Err(JsError::Type(format!(
            "Failed to set the '{what}' property on 'HTMLMediaElement': The provided double value is non-finite."
        )));
    }
    Ok(n)
}

fn page_rc(n: &NodeData) -> Rc<PageDom> {
    n.page().clone()
}

/// The element's MediaError in this realm: one object per error.
fn error_value<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let Some((code, serial)) = peek(n.page(), n.id, |st| st.error) else { return Ok(s.null()) };
    let wrapper = node_value(s, n.page(), Some(n.id))?;
    let wrapper = wrapper.as_object().ok_or(JsError::ContextDestroyed)?;
    if let Some(cached) = s.get_hidden(wrapper, "__lpMediaError")?.and_then(|v| v.as_object()) {
        if cached.get_atom("serial")?.to_number()? == f64::from(serial) {
            return cached.get_atom("error");
        }
    }
    let error = MODULE.helper(s, n.page(), "mediaError", &[s.number(f64::from(code)), s.string(UNSUPPORTED)])?;
    let holder = s.object();
    holder.set_atom("serial", s.number(f64::from(serial)))?;
    holder.set_atom("error", error)?;
    s.set_hidden(wrapper, "__lpMediaError", holder.value())?;
    Ok(error)
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
    def.getter("readyState", |s, _n| Ok(s.number(0.0)));
    def.getter("error", |s, n| {
        flush(n.page(), &Rc::downgrade(n.page()));
        error_value(s, n)
    });
    def.getter("currentSrc", |s, n| Ok(s.string(&peek(n.page(), n.id, |st| st.current_src.clone()))));
    def.getter("paused", |s, n| Ok(s.boolean(peek(n.page(), n.id, |st| st.paused))));
    def.getter("ended", |s, _n| Ok(s.boolean(false)));
    def.getter("seeking", |s, _n| Ok(s.boolean(false)));
    def.getter("duration", |s, _n| Ok(s.number(f64::NAN)));
    for name in ["buffered", "played", "seekable"] {
        def.getter(name, |s, n| MODULE.helper(s, n.page(), "timeRanges", &[]));
    }
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
        |s, n| Ok(s.number(peek(n.page(), n.id, |st| st.default_start))),
        |_s, n, v| {
            // No media data (HAVE_NOTHING): the default playback start position.
            let t = finite(v, "currentTime")?;
            with(n.page(), n.id, |st| st.default_start = t);
            Ok(())
        },
    );
    def.method("fastSeek", 1, |s, n, a| {
        a.require(1, "HTMLMediaElement.fastSeek")?;
        let t = finite(a.get(0), "currentTime")?;
        with(n.page(), n.id, |st| st.default_start = t);
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
        |s, n| {
            let muted = peek(n.page(), n.id, |st| st.muted);
            Ok(s.boolean(match muted {
                Some(m) => m,
                None => n.page().dom.borrow().has_attribute(n.id, "muted").unwrap_or(false),
            }))
        },
        |_s, n, v| {
            let muted = v.to_bool();
            let attr = n.page().dom.borrow().has_attribute(n.id, "muted").unwrap_or(false);
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
            let (changed, generation) = with(n.page(), n.id, |st| {
                let changed = st.rate != rate;
                st.rate = rate;
                (changed, st.generation)
            });
            if changed {
                queue_event(&page_rc(n), n.id, generation, "ratechange");
            }
            Ok(())
        },
    );
    def.accessor(
        "defaultPlaybackRate",
        |s, n| Ok(s.number(peek(n.page(), n.id, |st| st.default_rate))),
        |_s, n, v| {
            let rate = finite(v, "defaultPlaybackRate")?;
            let (changed, generation) = with(n.page(), n.id, |st| {
                let changed = st.default_rate != rate;
                st.default_rate = rate;
                (changed, st.generation)
            });
            if changed {
                queue_event(&page_rc(n), n.id, generation, "ratechange");
            }
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
            with(n.page(), n.id, |st| st.has_src_object = !value.is_null());
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
        if peek(&page, n.id, |st| st.error.is_some_and(|(code, _)| code == MEDIA_ERR_SRC_NOT_SUPPORTED)) {
            let error = JsError::dom("NotSupportedError", NO_SOURCES).to_value(s);
            let deferred = s.deferred()?;
            deferred.reject(s, error)?;
            return Ok(deferred.promise.get(s));
        }
        let deferred = s.deferred()?;
        let promise = deferred.promise.get(s);
        let (empty, was_paused, generation) = with(&page, n.id, |st| {
            st.pending_play.push((s.realm().downgrade(), deferred));
            (st.network == NETWORK_EMPTY, st.paused, st.generation)
        });
        if empty {
            resource_selection(&page, n.id);
        }
        if was_paused {
            with(&page, n.id, |st| st.paused = false);
            queue_event(&page, n.id, generation, "play");
            // readyState is HAVE_NOTHING: playback waits for data.
            queue_event(&page, n.id, generation, "waiting");
        }
        Ok(promise)
    });
    def.method("pause", 0, |s, n, _a| {
        let page = page_rc(n);
        flush(&page, &Rc::downgrade(&page));
        if peek(&page, n.id, |st| st.network == NETWORK_EMPTY) {
            resource_selection(&page, n.id);
        }
        let (pending, generation) = with(&page, n.id, |st| {
            if st.paused {
                return (None, st.generation);
            }
            st.paused = true;
            (Some(std::mem::take(&mut st.pending_play)), st.generation)
        });
        if let Some(pending) = pending {
            queue_event(&page, n.id, generation, "timeupdate");
            queue_event(&page, n.id, generation, "pause");
            queue_reject(&page, pending, "AbortError", "The play() request was interrupted by a call to pause().");
        }
        Ok(s.undefined())
    });
    def.method("canPlayType", 1, |s, _n, a| {
        a.require(1, "HTMLMediaElement.canPlayType")?;
        dom_string(a.get(0))?;
        // No decoder: nothing can be played.
        Ok(s.string(""))
    });
}
