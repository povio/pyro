//! The painted-candidate scan shared by `paint`, `lcp`, `element` and `softnav`: what
//! the document shows that those entry types are about (images, text blocks,
//! background images, SVG, canvases, video posters), with the part of it that is
//! visible in the viewport, computed once per frame from layout on demand.
//!
//! # Use
//!
//! ```ignore
//! // in <module>::on_frame(page, frame):
//! let scan = scan::painted_candidates(page, frame);      // Rc<FrameScan>, shared by every module of the frame
//! for c in scan.newly_painted(frame) { ... }             // candidates painted for the first time in this frame
//! for c in &scan.candidates { ... }                      // everything painted now (largest-so-far, re-checks)
//! ```
//!
//! The first caller of a frame pays for the scan, the others get the same `Rc`
//! (memoized by `frame.presented`). Call it only when you will use it: a document
//! nobody observes (`page.timeline().is_observed(..)`, or the first paint not yet
//! reported) must not scan. A scan walks the document; it is skipped (the last
//! result is returned) when layout's `observation_stamp` is unchanged, no image
//! finished loading, no `<canvas>` seen undrawn has since been drawn on (a canvas's
//! first drawing changes no document state; later drawings change no candidate: a
//! canvas alone never forces a rescan) and no poster or CSS image is awaited (a
//! fetch that failed is not awaited, any other for at most [`MAX_WAIT_WALKS`]
//! walks in a document that does not change). [`walks`] counts the walks, for
//! tests that assert work and not time.
//!
//! # What is a candidate
//!
//! A [`Candidate`] is an element (a text block: its block container) that paints
//! something of its own and whose visible rectangle has an area:
//!
//! - `Image`: `<img>` and SVG `<image>` (and `<input type=image>` once the embedder
//!   reports its request like an `<img>`'s) whose request finished and decodes, with its
//!   natural size; `url` is the resolved current source. `VideoPoster`: a
//!   `<video poster>` Resource Timing reports as fetched (no natural size; no frames
//!   are decoded: a poster is painted, a video frame never is).
//! - `BackgroundImage`: an element whose computed `background-image` has a `url()`
//!   (gradients are not candidates, as in the specs). Pyro does not track the load
//!   of CSS images: it is loaded when Resource Timing has a finished entry for the
//!   URL (`load_time` its `responseEnd`); `data:` URLs count as loaded at once.
//! - `Text`: the non-whitespace text nodes of one block container (the nearest
//!   non-inline ancestor): `visible` is the union of their line boxes, each clipped.
//! - `Svg`: an `<svg>` root with a painted shape or text below it. `Canvas`: a
//!   `<canvas>` something was drawn on (2D contexts; there is no WebGL).
//!
//! Not candidates: `display: none`, `visibility: hidden` (the element itself; a
//! child may opt back in), an ancestor with `opacity: 0`, a rectangle clipped away by
//! an ancestor's overflow clip or outside the viewport, zero-area boxes. `visible`
//! is the border box (client coordinates of the document) intersected with the
//! overflow clips of the boxes that clip it (at their padding boxes; those of the
//! containing block chain: an absolutely positioned box is not clipped by an
//! `overflow` ancestor between it and its containing block, a fixed one only by
//! an ancestor that contains fixed boxes) and the viewport; transforms are those layout
//! reports (a `scale(0)` box has no area, a translated-away one is outside).
//! Not done: content boxes of replaced elements (`object-fit`), `clip-path`/`mask`
//! clipping, backface visibility, shadow trees (the walk stays in the light tree),
//! subframes (each frame's document scans itself: a frame's content does not count
//! for its embedder's candidates, as for FCP and LCP).
//!
//! # Identity and "first painted"
//!
//! Each candidate has an identity (`node`, kind and `url`): the scan remembers the
//! `presented` time of the first frame in which that identity was painted visible
//! ([`Candidate::first_painted`]). An element that is hidden and reappears keeps
//! its identity and its first-painted time (as in Chrome: a node's paint is
//! recorded once); one that is resized or whose text changes
//! keeps its identity (so modified text and resized images are not new, as LCP
//! says); an `<img>` whose `src` changes is a new identity.
//!
//! [`note_image_loaded`] (called from `PageDom::image_loaded`, only while some module
//! needs it) records `load_time`: the document-clock time the image request finished.
//!
//! # Owner
//!
//! `scan.rs` is core's. A lane that needs another fact from it (an LCP rule, an
//! element-timing attribute) adds a field to `Candidate` in a commit of its own and
//! says so in its hand-back; it does not write a second scan.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::layout::Rect;
use pyro_dom::style::CheckVisibilityOptions;
use pyro_dom::{Dom, NodeId, NodeType};
use rustc_hash::FxHashMap;

use crate::page::PageDom;

use super::FrameInfo;

/// Whether some ENABLED module reads scan results (a const: the load-time
/// bookkeeping of [`note_image_loaded`] costs nothing while none does).
pub(crate) const WANTED: bool = super::paint::ENABLED || super::lcp::ENABLED || super::element::ENABLED || super::softnav::ENABLED;

/// How many walks a poster or CSS image whose fetch Resource Timing has not reported may keep
/// forcing in a document that does not change (one per frame): a load that never finishes does
/// not rescan forever.
const MAX_WAIT_WALKS: u32 = 120;

/// The most nodes a subtree check ([`painted_candidates_in`]) looks at: the rest of a huge subtree is not judged.
pub const SUBTREE_NODE_CAP: usize = 20_000;

const SVG_NS: &str = "http://www.w3.org/2000/svg";

/// What a candidate paints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Text,
    Image,
    VideoPoster,
    BackgroundImage,
    Svg,
    Canvas,
}

/// One painted thing of a frame. See the module docs.
#[derive(Clone, Debug)]
pub struct Candidate {
    /// The element (a text block's container).
    pub node: NodeId,
    pub kind: Kind,
    /// Resolved URL of an image, a poster or a background image.
    pub url: Option<String>,
    /// Natural size in CSS px of an image or poster.
    pub natural: Option<(f64, f64)>,
    /// The border box in client coordinates (text: the union of the line boxes), unclipped.
    pub border_box: Rect,
    /// The visible part of it: clipped by ancestors' overflow and the viewport.
    pub visible: Rect,
    /// `visible.width * visible.height`.
    pub area: f64,
    /// Document-clock time the image finished loading (`None`: unknown, text, svg, canvas).
    pub load_time: Option<f64>,
    /// `presented` of the first frame this identity was painted visible.
    pub first_painted: f64,
}

/// The candidates of one frame, in document order.
#[derive(Debug, Default)]
pub struct FrameScan {
    /// `FrameInfo::presented` of the frame whose scan this is (an unchanged document
    /// returns an earlier one: compare with the frame you are in, see
    /// [`FrameScan::newly_painted`]).
    pub presented: f64,
    /// Viewport size (CSS px) the visible rectangles were clipped to.
    pub viewport: (f64, f64),
    pub candidates: Vec<Candidate>,
    /// The walk skipped a poster or background image whose load Resource Timing has not
    /// reported yet: it can become a candidate in a later frame without the document changing,
    /// so a consumer that still wants candidates must ask for another frame.
    pub waiting: bool,
}

impl FrameScan {
    /// The candidates first painted in `frame`.
    pub fn newly_painted<'a>(&'a self, frame: &'a FrameInfo) -> impl Iterator<Item = &'a Candidate> + 'a {
        self.candidates.iter().filter(move |c| c.first_painted == frame.presented)
    }
}

type Stamp = ((u64, u64, u32, u32), u64);

#[derive(Default)]
pub(crate) struct ScanState {
    last: Option<Rc<FrameScan>>,
    stamp: Option<Stamp>,
    /// `Layout::sync_count` after the last walk: layout laid out again since (a load of its own
    /// finished: an SVG image's size) although the stamp did not move.
    synced: u64,
    /// The canvases the last walk saw with nothing drawn on them: rescan once one has.
    undrawn: Vec<NodeId>,
    /// The last walk skipped a poster or background image whose load Resource Timing
    /// has not reported yet (and has not failed): rescan next frame, for [`MAX_WAIT_WALKS`] walks.
    waiting: bool,
    /// Consecutive walks made while `waiting` in an unchanged document.
    wait_walks: u32,
    /// An image finished loading since the last walk.
    dirty: bool,
    /// `(img, resolved url)` -> time its request finished.
    loaded: FxHashMap<(NodeId, String), f64>,
    /// Identity -> `presented` of the frame it was first painted in.
    first: FxHashMap<(NodeId, Kind, Option<String>), f64>,
    walks: u64,
    /// The scan `paint` last judged (first paint, first contentful paint): the same scan again is
    /// the same document, so it is not judged again ([`judge_paint`]).
    paint_judged: Option<Rc<FrameScan>>,
    /// How many scans `paint` judged (a counter, see [`paint_judgements`]).
    paint_judgements: u64,
}

/// An image request of `el` for `src` finished (`PageDom::image_loaded`).
pub(crate) fn note_image_loaded(page: &PageDom, el: NodeId, src: &str) {
    if !WANTED {
        return;
    }
    let time = crate::event::page_now_ms(page);
    let mut state = page.timeline().scan.borrow_mut();
    state.loaded.insert((el, src.to_owned()), time);
    state.dirty = true;
}

/// A fetch of the document finished and its Resource Timing entry was added
/// (`PageDom::add_finished_fetches`). While the last walk awaited a poster or CSS image, that may
/// be the one: the next walk is forced and its budget of waiting walks starts over (an image that
/// finishes after the budget ran out, many frames into a slow load, must not be missed).
pub(crate) fn note_fetch_finished(page: &PageDom) {
    if !WANTED {
        return;
    }
    if let Ok(mut state) = page.timeline().scan.try_borrow_mut() {
        if state.waiting {
            state.dirty = true;
        }
    }
}

/// How many times the document was walked (a counter: tests assert on it, never on time).
pub fn walks(page: &PageDom) -> u64 {
    page.timeline().scan.borrow().walks
}

/// How many scans `paint` judged: its own walks of the document (text controls, boxes that paint)
/// run once per scan, not once per frame (a counter: tests assert on it, never on time).
pub fn paint_judgements(page: &PageDom) -> u64 {
    page.timeline().scan.borrow().paint_judgements
}

/// Whether `paint` has to judge `scan`: false for the scan it judged last (the document did not
/// change since: no frame of an animation that moves nothing else re-walks a large document for
/// a paint that is not there).
pub(crate) fn judge_paint(page: &PageDom, scan: &Rc<FrameScan>) -> bool {
    let mut state = page.timeline().scan.borrow_mut();
    if state.paint_judged.as_ref().is_some_and(|last| Rc::ptr_eq(last, scan)) {
        return false;
    }
    state.paint_judged = Some(scan.clone());
    state.paint_judgements += 1;
    true
}

/// Whether a scan now could find something the last one did not: the document was never
/// scanned, changed since (layout's `observation_stamp`: DOM, style, viewport, scroll), an image
/// finished loading, a canvas was drawn on, or a poster or CSS image is still awaited. What the
/// modules that must see every paint (`element`, `lcp`) ask after a task, to request the
/// frame that scans (`Browser::note_first_paint`). Cheap: no layout, no walk.
pub fn stale(page: &PageDom) -> bool {
    let state = page.timeline().scan.borrow();
    if state.last.is_none() || state.dirty || (state.waiting && state.wait_walks < MAX_WAIT_WALKS) {
        return true;
    }
    if state.undrawn.iter().any(|&canvas| canvas_drawn(page, canvas)) {
        return true;
    }
    let (Ok(layout), Ok(dom)) = (page.layout.try_borrow(), page.dom.try_borrow()) else { return true };
    // Layout's own loads (an SVG `<img>`'s size) arrive without moving the stamp.
    if layout.geometry_stale() || layout.sync_count() != state.synced {
        return true;
    }
    state.stamp != Some(layout.observation_stamp(&dom))
}

/// Whether layout laid out again, or is out of date with what reached it, since the last walk.
fn layout_moved(page: &PageDom, state: &ScanState) -> bool {
    let layout = page.layout();
    layout.geometry_stale() || layout.sync_count() != state.synced
}

/// The painted candidates of `page`'s document in `frame`. See the module docs.
pub fn painted_candidates(page: &Rc<PageDom>, frame: &FrameInfo) -> Rc<FrameScan> {
    // Memo: another module already scanned this frame, or nothing changed since.
    let (stamp, force) = {
        let state = page.timeline().scan.borrow();
        if let Some(last) = &state.last {
            // (Unless something reached the document since that walk: two rendering updates may
            // share a `presented`, and the second must see what arrived between them.)
            if last.presented == frame.presented && !state.dirty && !layout_moved(page, &state) {
                return last.clone();
            }
        }
        let waiting = state.waiting && state.wait_walks < MAX_WAIT_WALKS;
        (state.stamp, state.dirty || waiting || layout_moved(page, &state) || state.undrawn.iter().any(|&canvas| canvas_drawn(page, canvas)))
    };
    let now_stamp = page.layout().observation_stamp(&page.dom.borrow());
    if !force && stamp == Some(now_stamp) {
        let last = page.timeline().scan.borrow().last.clone();
        if let Some(last) = last {
            return last;
        }
    }
    let scan = Rc::new(walk(page, frame, None));
    let mut state = page.timeline().scan.borrow_mut();
    let changed = stamp != Some(now_stamp) || state.dirty;
    state.wait_walks = if state.waiting && !changed { state.wait_walks + 1 } else { 0 };
    state.stamp = Some(now_stamp);
    state.synced = page.layout().sync_count();
    state.dirty = false;
    state.walks += 1;
    state.last = Some(scan.clone());
    scan
}

/// What the first pass of a walk found, before geometry.
enum Found {
    Image(NodeId, Kind),
    Background(NodeId),
    Svg(NodeId),
    Canvas(NodeId),
    Text(NodeId),
}

/// The painted candidates inside the subtrees of `roots` (connected nodes; others are skipped) as the
/// document shows them now, in no particular order and not memoized: Soft Navigations asks whether
/// what a user interaction modified was painted, and a walk of the whole document per frame for that
/// would cost O(document) where the answer lies in a few nodes. It does not move the scan's state (no
/// identity is recorded: every candidate reads `first_painted == frame.presented`), so it never changes
/// what [`painted_candidates`] reports. Cost: the nodes of the subtrees plus a style read for those
/// that can paint a background image; the caller bounds the roots.
pub fn painted_candidates_in(page: &Rc<PageDom>, frame: &FrameInfo, roots: &[NodeId]) -> FrameScan {
    walk(page, frame, Some(roots))
}

fn walk(page: &Rc<PageDom>, frame: &FrameInfo, roots: Option<&[NodeId]>) -> FrameScan {
    // Pass 1 (DOM only): which elements and text nodes could paint something.
    let mut found: Vec<Found> = Vec::new();
    {
        let dom = page.dom.borrow();
        let document = page.document();
        // A `background-image` url comes from a sheet, a `style` attribute or the `background` attribute;
        // without one in any sheet, only elements with the latter two need their style read.
        let mut sheets_have_url = false;
        page.layout().style().for_each_author_sheet(&dom, &mut |text| sheets_have_url |= text.contains("url("));
        let mut budget = if roots.is_some() { SUBTREE_NODE_CAP } else { usize::MAX };
        let mut visit = |id: NodeId| {
            if budget == 0 {
                return;
            }
            budget -= 1;
            match dom.node_type(id) {
                Ok(NodeType::Text) => {
                    if dom.char_data(id).is_ok_and(|d| !d.trim().is_empty()) {
                        found.push(Found::Text(id));
                    }
                }
                Ok(NodeType::Element) => {
                    let Ok(el) = dom.element(id) else { return };
                    let svg = &*el.name.ns == SVG_NS;
                    match (&*el.name.local, svg) {
                        ("img", false) => found.push(Found::Image(id, Kind::Image)),
                        ("image", true) => found.push(Found::Image(id, Kind::Image)),
                        ("input", false) if el.attr("type").is_some_and(|t| t.eq_ignore_ascii_case("image")) => {
                            found.push(Found::Image(id, Kind::Image))
                        }
                        ("video", false) if el.attr("poster").is_some_and(|p| !p.trim().is_empty()) => {
                            found.push(Found::Image(id, Kind::VideoPoster))
                        }
                        ("canvas", false) => found.push(Found::Canvas(id)),
                        ("svg", true) if dom.parent_element(id).ok().flatten().is_none_or(|p| !is_svg(&dom, p)) => {
                            found.push(Found::Svg(id))
                        }
                        _ => {}
                    }
                    if sheets_have_url || el.attr("background").is_some() || el.attr("style").is_some_and(|style| style.contains("url(")) {
                        found.push(Found::Background(id));
                    }
                }
                _ => {}
            }
        };
        match roots {
            None => {
                let Ok(nodes) = dom.descendants(document) else { return FrameScan { presented: frame.presented, ..Default::default() } };
                nodes.for_each(&mut visit);
            }
            Some(roots) => {
                for &root in roots {
                    if !dom.is_connected(root).unwrap_or(false) {
                        continue;
                    }
                    if let Ok(nodes) = dom.descendants(root) {
                        nodes.for_each(&mut visit);
                    }
                }
            }
        }
    }
    // Pass 2: the sources the loader chose for the images (they read the viewport, so no
    // borrow of ours is held).
    let mut images: FxHashMap<NodeId, ImageInfo> = FxHashMap::default();
    for f in &found {
        if let Found::Image(id, Kind::Image) = f {
            // `complete` first: it adopts an image the embedder never reported (an available one).
            let complete = page.image_complete(*id);
            let request = page.images.borrow().get(id).cloned();
            if let (true, Some((loading, false, true))) = (complete, request) {
                images.insert(*id, ImageInfo { loading, source: crate::image_decode::selected_source(page, *id) });
            }
        }
    }
    // Resource Timing's finished fetches: the load of CSS images and posters, which the
    // loader does not report to the document otherwise.
    page.add_finished_fetches();
    let finished: FxHashMap<String, f64> = page
        .resource_timings()
        .entries()
        .iter()
        .filter(|e| e.response_status < 400 && e.response_end > 0.0)
        .map(|e| (e.name.clone(), e.response_end))
        .collect();
    // ... and the ones that failed (not awaited).
    let failed: rustc_hash::FxHashSet<String> =
        page.resource_timings().entries().iter().filter(|e| e.response_status >= 400).map(|e| e.name.clone()).collect();
    // Pass 3: geometry.
    let dom = page.dom.borrow();
    let layout = page.layout();
    // Lay out now: what arrived for layout since the last walk is then seen by this one, and
    // `Layout::geometry_stale` is false until something else arrives.
    layout.synced_geometry_source(&dom);
    let viewport = (f64::from(layout.inner_width()), f64::from(layout.inner_height()));
    let view = Rect { x: 0.0, y: 0.0, width: viewport.0, height: viewport.1 };
    let shown = CheckVisibilityOptions { check_visibility: true, check_opacity: true };
    let visible_el = |el: NodeId| layout.check_visibility(&dom, el, shown);
    let mut waiting = false;
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut undrawn: Vec<NodeId> = Vec::new();
    let mut texts: Vec<(NodeId, Rect, Rect)> = Vec::new();
    let mut blocks: FxHashMap<NodeId, NodeId> = FxHashMap::default();
    let state = page.timeline().scan.borrow();
    for f in &found {
        match *f {
            Found::Image(el, Kind::VideoPoster) => {
                let Some(poster) = dom.get_attribute(el, "poster").ok().flatten() else { continue };
                let url = crate::html::resolve_url(&dom, el, poster);
                let Some(load_time) = finished.get(&url).copied().or_else(|| url.starts_with("data:").then_some(0.0)) else {
                    waiting |= !failed.contains(&url);
                    continue;
                };
                if !visible_el(el) {
                    continue;
                }
                let border_box = layout.laid_out_client_rect(&dom, el);
                let Some(visible) = visible_rect(&dom, &layout, el, border_box, view) else { continue };
                candidates.push(Candidate { node: el, kind: Kind::VideoPoster, url: Some(url), natural: None, border_box, visible, area: visible.width * visible.height, load_time: Some(load_time), first_painted: 0.0 });
            }
            Found::Image(el, kind) => {
                let Some(info) = images.get(&el) else { continue };
                if !visible_el(el) {
                    continue;
                }
                let Some((nw, nh)) = dom.reported_natural_size(el) else { continue };
                let url = info.source.as_ref().map(|s| crate::html::resolve_url(&dom, el, s));
                let load_time = state.loaded.get(&(el, info.loading.clone())).copied();
                let border_box = layout.laid_out_client_rect(&dom, el);
                let Some(visible) = visible_rect(&dom, &layout, el, border_box, view) else { continue };
                candidates.push(Candidate { node: el, kind, url, natural: Some((nw, nh)), border_box, visible, area: visible.width * visible.height, load_time, first_painted: 0.0 });
            }
            Found::Background(el) => {
                let value = layout.computed_style_value(&dom, el, "background-image");
                // An element whose `content` is an image is a replaced element showing it (an `img` with
                // `content: url()`): a contentful image of its own, whatever its background.
                let content = layout.computed_style_value(&dom, el, "content");
                for (url, kind) in [(first_url(&value), Kind::BackgroundImage), (first_url(&content), Kind::Image)] {
                    let Some(url) = url else { continue };
                    if !visible_el(el) {
                        continue;
                    }
                    let url = crate::html::resolve_url(&dom, el, &url);
                    let load_time = if url.starts_with("data:") { Some(0.0) } else { finished.get(&url).copied() };
                    if load_time.is_none() {
                        waiting |= !failed.contains(&url);
                        continue;
                    }
                    // The root element's background (the body's when the root has none) is the canvas
                    // background: it paints the whole viewport, whatever size the element has.
                    let border_box = if kind == Kind::BackgroundImage && paints_canvas(&dom, &layout, el) { view } else { layout.laid_out_client_rect(&dom, el) };
                    let Some(visible) = visible_rect(&dom, &layout, el, border_box, view) else { continue };
                    candidates.push(Candidate { node: el, kind, url: Some(url), natural: None, border_box, visible, area: visible.width * visible.height, load_time, first_painted: 0.0 });
                }
            }
            Found::Svg(el) => {
                if !visible_el(el) || !has_painted_shape(&dom, el) {
                    continue;
                }
                let border_box = layout.laid_out_client_rect(&dom, el);
                let Some(visible) = visible_rect(&dom, &layout, el, border_box, view) else { continue };
                candidates.push(Candidate { node: el, kind: Kind::Svg, url: None, natural: None, border_box, visible, area: visible.width * visible.height, load_time: None, first_painted: 0.0 });
            }
            Found::Canvas(el) => {
                if !canvas_drawn(page, el) {
                    undrawn.push(el);
                    continue;
                }
                if !visible_el(el) {
                    continue;
                }
                let border_box = layout.laid_out_client_rect(&dom, el);
                let Some(visible) = visible_rect(&dom, &layout, el, border_box, view) else { continue };
                candidates.push(Candidate { node: el, kind: Kind::Canvas, url: None, natural: None, border_box, visible, area: visible.width * visible.height, load_time: None, first_painted: 0.0 });
            }
            Found::Text(text) => {
                let Some(parent) = dom.parent_element(text).ok().flatten() else { continue };
                if !visible_el(parent) {
                    continue;
                }
                let Some(line_box) = layout.text_rect(&dom, text) else { continue };
                let Some(visible) = visible_rect_from(&dom, &layout, Some(parent), line_box, view) else { continue };
                let block = *blocks.entry(parent).or_insert_with(|| block_container(&dom, &layout, parent));
                texts.push((block, line_box, visible));
            }
        }
    }
    // Text: one candidate per block container, the union of its text's visible boxes.
    let mut by_block: Vec<(NodeId, Rect, Rect)> = Vec::new();
    let mut slot: FxHashMap<NodeId, usize> = FxHashMap::default();
    for (block, line_box, visible) in texts {
        match slot.get(&block) {
            Some(&i) => {
                by_block[i].1 = union(by_block[i].1, line_box);
                by_block[i].2 = union(by_block[i].2, visible);
            }
            None => {
                slot.insert(block, by_block.len());
                by_block.push((block, line_box, visible));
            }
        }
    }
    for (block, border_box, visible) in by_block {
        candidates.push(Candidate { node: block, kind: Kind::Text, url: None, natural: None, border_box, visible, area: visible.width * visible.height, load_time: None, first_painted: 0.0 });
    }
    // Document order of the elements (text blocks come where their container does).
    drop(state);
    drop(layout);
    if roots.is_some() {
        // A subtree check: the candidates of overlapping roots are one (an element is met once per
        // root that holds it), and nothing is remembered.
        candidates.dedup_by(|a, b| a.node == b.node && a.kind == b.kind && a.url == b.url);
        for c in &mut candidates {
            c.first_painted = frame.presented;
        }
        return FrameScan { presented: frame.presented, viewport, candidates, waiting };
    }
    // (Nothing to put in order below two candidates: a document that shows nothing, or one thing,
    // does not number every node.)
    if candidates.len() > 1 {
        let order: FxHashMap<NodeId, usize> = dom
            .descendants(page.document())
            .map(|nodes| nodes.enumerate().map(|(i, n)| (n, i)).collect::<FxHashMap<NodeId, usize>>())
            .unwrap_or_default();
        candidates.sort_by_key(|c| order.get(&c.node).copied().unwrap_or(usize::MAX));
    }
    drop(dom);
    // First painted: remember the frame an identity was first seen painted in.
    let mut state = page.timeline().scan.borrow_mut();
    state.undrawn = undrawn;
    state.waiting = waiting;
    state.first.retain(|(node, _, _), _| page.dom.borrow().is_alive(*node));
    state.loaded.retain(|(node, _), _| page.dom.borrow().is_alive(*node));
    for c in &mut candidates {
        let key = (c.node, c.kind, c.url.clone());
        c.first_painted = *state.first.entry(key).or_insert(frame.presented);
    }
    FrameScan { presented: frame.presented, viewport, candidates, waiting }
}

/// What pass 2 learned of a loaded `<img>`: its request's `src` and the source the loader chose.
struct ImageInfo {
    loading: String,
    source: Option<String>,
}

/// Whether something was drawn on the 2D canvas `el` (`pyro_canvas::Surface::is_allocated`).
fn canvas_drawn(page: &PageDom, el: NodeId) -> bool {
    let list = page.canvas_surfaces.borrow();
    for (canvas, surface, _, _) in list.iter() {
        if *canvas != el {
            continue;
        }
        let Some(surface) = surface.upgrade() else { continue };
        let drawn = surface.try_borrow().is_ok_and(|s| s.is_allocated());
        if drawn {
            return true;
        }
    }
    false
}

fn is_svg(dom: &Dom, el: NodeId) -> bool {
    dom.element(el).is_ok_and(|e| &*e.name.ns == SVG_NS)
}

/// An `<svg>` with something below it that paints (shapes, text, images, `<use>`).
fn has_painted_shape(dom: &Dom, svg: NodeId) -> bool {
    let Ok(nodes) = dom.descendants(svg) else { return false };
    nodes.skip(1).any(|n| {
        dom.element(n).is_ok_and(|e| {
            &*e.name.ns == SVG_NS
                && matches!(&*e.name.local, "path" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon" | "text" | "image" | "use" | "foreignObject")
                && e.attr("fill").is_none_or(|f| f != "none")
        })
    })
}

/// Whether `el`'s background image is the canvas background (CSS Backgrounds § 2.11.2): `html`'s,
/// or `body`'s when `html` has no background image.
fn paints_canvas(dom: &Dom, layout: &pyro_dom::layout::Layout, el: NodeId) -> bool {
    if !layout.is_root_container(dom, el) {
        return false;
    }
    let is_html = dom.element(el).is_ok_and(|e| &*e.name.local == "html");
    if is_html {
        return true;
    }
    // The body: the canvas takes its background only when the root element has none.
    match dom.parent_element(el).ok().flatten() {
        Some(root) => layout.computed_style_value(dom, root, "background-image").trim() == "none",
        None => true,
    }
}

/// The first `url(...)` of a computed `background-image` (gradients have none).
fn first_url(value: &str) -> Option<String> {
    let start = value.find("url(")? + 4;
    let rest = value[start..].trim_start();
    let (quote, rest) = match rest.chars().next()? {
        q @ ('"' | '\'') => (Some(q), &rest[1..]),
        _ => (None, rest),
    };
    let end = match quote {
        Some(q) => rest.find(q)?,
        None => rest.find(')')?,
    };
    Some(rest[..end].trim().to_owned()).filter(|u| !u.is_empty())
}

/// The nearest ancestor-or-self whose `display` is not inline-level flow (text's block container).
fn block_container(dom: &Dom, layout: &pyro_dom::layout::Layout, mut el: NodeId) -> NodeId {
    loop {
        let display = layout.computed_style_value(dom, el, "display");
        if !matches!(display.as_str(), "inline" | "contents") {
            return el;
        }
        match dom.parent_element(el).ok().flatten() {
            Some(parent) => el = parent,
            None => return el,
        }
    }
}

fn union(a: Rect, b: Rect) -> Rect {
    let (left, top) = (a.left().min(b.left()), a.top().min(b.top()));
    let (right, bottom) = (a.right().max(b.right()), a.bottom().max(b.bottom()));
    Rect { x: left, y: top, width: right - left, height: bottom - top }
}

/// The intersection with positive area.
fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let (left, top) = (a.left().max(b.left()), a.top().max(b.top()));
    let (right, bottom) = (a.right().min(b.right()), a.bottom().min(b.bottom()));
    (right > left && bottom > top).then_some(Rect { x: left, y: top, width: right - left, height: bottom - top })
}

/// `rect` of `el`'s box clipped by the overflow clips of the ancestors of `el` and the viewport.
fn visible_rect(dom: &Dom, layout: &pyro_dom::layout::Layout, el: NodeId, rect: Rect, view: Rect) -> Option<Rect> {
    visible_rect_from(dom, layout, clipping_parent(dom, layout, el), rect, view)
}

/// The box whose overflow clip applies next to `el`'s box: its containing block (CSS Overflow 3 § 3:
/// a clip applies to the descendants whose containing block chain passes through the clipping box).
/// `None`: the viewport (an absolute box with no positioned ancestor, a fixed one outside any
/// transform-like ancestor).
fn clipping_parent(dom: &Dom, layout: &pyro_dom::layout::Layout, el: NodeId) -> Option<NodeId> {
    use pyro_dom::style::Position;
    let style = layout.style().computed(dom, el).unwrap_or_default();
    let mut ancestor = dom.parent_element(el).ok().flatten();
    let wanted: fn(&pyro_dom::style::ComputedStyle) -> bool = match style.position {
        Position::Absolute => |s| s.contains_absolute,
        Position::Fixed => |s| s.contains_fixed,
        _ => return ancestor,
    };
    while let Some(node) = ancestor {
        if layout.style().computed(dom, node).is_some_and(|s| wanted(&s)) {
            return Some(node);
        }
        ancestor = dom.parent_element(node).ok().flatten();
    }
    None
}

/// [`visible_rect`] starting the clip chain at `start` (inclusive).
fn visible_rect_from(dom: &Dom, layout: &pyro_dom::layout::Layout, start: Option<NodeId>, rect: Rect, view: Rect) -> Option<Rect> {
    if !(rect.width > 0.0 && rect.height > 0.0) {
        return None;
    }
    let mut rect = rect;
    let mut ancestor = start;
    while let Some(node) = ancestor {
        // The viewport takes the root containers' overflow.
        if !layout.is_root_container(dom, node) {
            let axes = layout.style().overflow_clip_axes(dom, node);
            if axes.x || axes.y {
                // (What is laid out: a clip container inside contents `content-visibility: auto` skips has
                // no box, and asking its rect would render them; nothing in it paints.)
                let mut b = layout.laid_out_client_rect(dom, node);
                if !(b.width > 0.0 || b.height > 0.0) {
                    return None;
                }
                // The clip is at the padding box: inside the borders (not under a transform, where the
                // client sizes are not the painted ones; there the border box stays).
                if !layout.style().computed(dom, node).is_some_and(|s| s.transformed) {
                    let (left, top) = (layout.client_left(dom, node), layout.client_top(dom, node));
                    let (width, height) = (layout.client_width(dom, node), layout.client_height(dom, node));
                    if width > 0.0 && height > 0.0 {
                        b = Rect { x: b.x + left, y: b.y + top, width, height };
                    }
                }
                let clip = Rect {
                    x: if axes.x { b.x } else { f64::MIN / 4.0 },
                    y: if axes.y { b.y } else { f64::MIN / 4.0 },
                    width: if axes.x { b.width } else { f64::MAX / 2.0 },
                    height: if axes.y { b.height } else { f64::MAX / 2.0 },
                };
                rect = intersect(rect, clip)?;
            }
        }
        ancestor = clipping_parent(dom, layout, node);
    }
    intersect(rect, view)
}
