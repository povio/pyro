//! The track processing model. A load is selected at a stable state and is
//! tied to the track's document, source and generation across network tasks.
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use pyro_dom::NodeId;
use pyro_js::Value;
use rustc_hash::FxHashMap;

use crate::page::{MediaFetchHandle, MediaResponse, PageDom};

const NONE: u16 = 0;
const LOADING: u16 = 1;
const LOADED: u16 = 2;
const ERROR: u16 = 3;

#[derive(PartialEq)]
struct Source {
    url: String,
    crossorigin: Option<String>,
}

struct Load {
    source: Source,
    generation: u64,
    ready: u16,
    fetch: Option<MediaFetchHandle>,
}

#[derive(Default)]
pub(super) struct Loads {
    tracks: RefCell<FxHashMap<NodeId, Load>>,
    generation: Cell<u64>,
    sweep_at: Cell<usize>,
}

impl Loads {
    pub(super) fn clear(&self) {
        self.tracks.borrow_mut().clear();
    }
}

pub(crate) fn track_ready_state(page: &PageDom, track: NodeId) -> u16 {
    page.media.track_loads.tracks.borrow().get(&track).map_or(NONE, |load| load.ready)
}

pub(super) fn media_parent(page: &PageDom, track: NodeId) -> Option<NodeId> {
    let dom = page.dom.borrow();
    let parent = dom.parent_element(track).ok()??;
    dom.element(parent)
        .is_ok_and(|e| e.is_html() && matches!(&*e.name.local, "audio" | "video"))
        .then_some(parent)
}

fn helper<T>(
    page: &Rc<PageDom>, node: NodeId, name: &str, text: Option<&str>,
    read: impl FnOnce(Value<'_>) -> pyro_js::JsResult<T>,
) -> Option<T> {
    let realm = page.main_world_realm()?;
    let scope = realm.scope().ok()?;
    let result = (|| {
        let element = crate::node::node_value(&scope, page, Some(node))?;
        let args = match text {
            Some(text) => vec![element, scope.string(text)],
            None => vec![element],
        };
        read(crate::text_tracks::helper(&scope, page, name, &args)?)
    })();
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            scope.report_exception(&error);
            None
        }
    }
}

pub(super) fn sync_parent(page: &Rc<PageDom>, media: NodeId) {
    helper(page, media, "syncMedia", None, |_| Ok(()));
}

fn source(page: &PageDom, track: NodeId, parent: NodeId) -> Source {
    let dom = page.dom.borrow();
    let url = dom.get_attribute(track, "src").ok().flatten()
        .filter(|s| !s.is_empty())
        .map(|s| crate::html::resolve_url(&dom, track, s))
        .unwrap_or_default();
    let crossorigin = dom.get_attribute(parent, "crossorigin").ok().flatten().map(|v| {
        if v.eq_ignore_ascii_case("use-credentials") { "use-credentials".to_owned() } else { "anonymous".to_owned() }
    });
    Source { url, crossorigin }
}

fn current(page: &PageDom, node: NodeId, generation: u64) -> bool {
    !page.is_disposed() && page.dom.borrow().node_document(node).ok() == Some(page.document()) &&
        page.media.track_loads.tracks.borrow().get(&node).is_some_and(|s| s.generation == generation)
}

pub(crate) fn check_track_resource(page: &Rc<PageDom>, node: NodeId) {
    if page.is_disposed() || page.dom.borrow().node_document(node).ok() != Some(page.document()) { return }
    // Reflect the URL mutation synchronously, including for an unattached
    // track whose preexisting, live cue list is still accessible to script.
    helper(page, node, "clearCues", None, |_| Ok(()));
    let loads = &page.media.track_loads;
    let Some(parent) = media_parent(page, node) else {
        // A completed resource belongs to the TextTrack. Keep it across
        // removal/reinsertion. Discard unfinished work and its network handle.
        let mut tracks = loads.tracks.borrow_mut();
        if tracks.get(&node).is_some_and(|l| l.ready < LOADED) { tracks.remove(&node); }
        return;
    };
    let key = source(page, node, parent);
    let enabled = helper(page, node, "trackMode", None, |v| v.to_string())
        .is_some_and(|m| m == "showing" || m == "hidden");
    let (old, aborted) = {
        let mut tracks = loads.tracks.borrow_mut();
        if tracks.get(&node).is_some_and(|load| load.source == key) { return }
        let old = tracks.remove(&node);
        let aborted = old.as_ref().is_some_and(|load| load.ready == LOADING);
        (old, aborted)
    };
    if let Some(previous) = old {
        // A parent CORS change selects a new resource even when the src
        // attribute did not change. Its old parsed cues must not duplicate.
        if previous.source.url == key.url && previous.source.crossorigin != key.crossorigin {
            helper(page, node, "resetResourceCues", None, |_| Ok(()));
        }
        drop(previous);
    }
    if !enabled { return }
    let generation = loads.generation.get().wrapping_add(1);
    loads.generation.set(generation);
    {
        let mut tracks = loads.tracks.borrow_mut();
        if tracks.len() >= loads.sweep_at.get() {
            let dom = page.dom.borrow();
            tracks.retain(|node, _| dom.is_alive(*node));
            loads.sweep_at.set((tracks.len() * 2).max(64));
        }
        tracks.insert(node, Load { source: key, generation, ready: NONE, fetch: None });
    }
    let weak = Rc::downgrade(page);
    page.post_task(Box::new(move || {
        let Some(page) = weak.upgrade() else { return };
        if !current(&page, node, generation) { return }
        if aborted {
            page.media.track_loads.tracks.borrow_mut().get_mut(&node).unwrap().ready = ERROR;
            crate::input::fire_simple(&page, node, "error", false, false);
            if !current(&page, node, generation) { return }
        }
        let Some(parent) = media_parent(&page, node) else { return };
        let key = source(&page, node, parent);
        if !page.media.track_loads.tracks.borrow().get(&node).is_some_and(|l| l.source == key) {
            check_track_resource(&page, node);
            return;
        }
        // Mode may change again before the queued selection begins.
        if helper(&page, node, "trackMode", None, |v| v.to_string()).is_none_or(|m| m == "disabled") {
            page.media.track_loads.tracks.borrow_mut().remove(&node);
            return;
        }
        page.media.track_loads.tracks.borrow_mut().get_mut(&node).unwrap().ready = LOADING;
        let Some(hooks) = page.hooks().filter(|_| !key.url.is_empty()) else {
            finish(&page, node, generation, Err("no text track resource".into()));
            return;
        };
        let weak = Rc::downgrade(&page);
        let handle = hooks.fetch_text_track(key.url, key.crossorigin, Box::new(move |result| {
            if let Some(page) = weak.upgrade() { finish(&page, node, generation, result); }
        }));
        if let Some(load) = page.media.track_loads.tracks.borrow_mut().get_mut(&node) {
            if load.generation == generation && load.ready == LOADING { load.fetch = handle; }
        };
    }));
}

fn finish(page: &Rc<PageDom>, node: NodeId, generation: u64, result: Result<MediaResponse, String>) {
    if !current(page, node, generation) { return }
    let Some(parent) = media_parent(page, node) else { return };
    if !page.media.track_loads.tracks.borrow().get(&node).is_some_and(|l| l.source == source(page, node, parent)) {
        check_track_resource(page, node);
        return;
    }
    let success = result.ok().filter(|r| (200..300).contains(&r.status) && r.clean && r.partial.is_none())
        .is_some_and(|r| helper(page, node, "loadText", Some(&String::from_utf8_lossy(&r.body)), |v| Ok(v.to_bool())) == Some(true));
    if !current(page, node, generation) { return }
    {
        let mut tracks = page.media.track_loads.tracks.borrow_mut();
        let load = tracks.get_mut(&node).unwrap();
        load.ready = if success { LOADED } else { ERROR };
        load.fetch.take();
    }
    crate::input::fire_simple(page, node, if success { "load" } else { "error" }, false, false);
}
