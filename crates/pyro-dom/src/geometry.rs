//! The geometry layout queries report: what an external engine (Blitz, in
//! pyro-render) laid the document out as. pyro-dom has no layout of its own; a
//! [`GeometrySource`] answers per element, and a [`LayoutResult`] holds the
//! whole document's boxes and words for queries that need them all (hit
//! testing, text ranges, carets, scroll anchoring).

use std::cell::RefCell;
use std::rc::Rc;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::dom::Dom;
use crate::layout::{Layout, Rect};
use crate::node::NodeId;

/// An element's `position`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BoxPosition {
    #[default]
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

/// Sides of a box: top, right, bottom, left.
pub const TOP: usize = 0;
pub const RIGHT: usize = 1;
pub const BOTTOM: usize = 2;
pub const LEFT: usize = 3;

/// One element's boxes from the external engine (document CSS px).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ExternalBox {
    /// Border box.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Padding box size (`clientWidth/Height`).
    pub client_width: f64,
    pub client_height: f64,
    /// `scrollWidth/Height`.
    pub scroll_width: f64,
    pub scroll_height: f64,
    /// Whether its scroll origin is at the end of the horizontal and the vertical axis (a
    /// right-to-left or reversed flex scroll container): it scrolls to negative offsets there.
    pub scroll_reversed: [bool; 2],
    /// Inside (or itself) a `position: fixed` box (then viewport coordinates).
    pub fixed: bool,
    /// Border and padding widths ([`TOP`], [`RIGHT`], [`BOTTOM`], [`LEFT`]).
    pub border: [f64; 4],
    pub padding: [f64; 4],
    pub position: BoxPosition,
}

/// One word of a text node on a line (document CSS px).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExternalWord {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub fixed: bool,
    /// Caret positions: UTF-16 offset into the node's data and its x, from
    /// the word's start to its end.
    pub edges: Vec<(u32, f64)>,
}

/// A `position: sticky` element's inputs (its box is laid out as if static).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ExternalSticky {
    /// [`TOP`], [`RIGHT`], [`BOTTOM`], [`LEFT`] insets: px plus a fraction
    /// of the scrollport (`None`: auto).
    pub insets: [Option<(f64, f64)>; 4],
    /// Its containing block's content box (x, y, width, height), document
    /// coordinates: the box never leaves it.
    pub limit: Option<(f64, f64, f64, f64)>,
    /// The [`crate::snapshot::render_key`] of its containing block's element:
    /// the sticky boxes holding that element move the limit with them.
    pub cb: Option<u64>,
}

/// The whole document's boxes as the external engine laid it out, keyed
/// by [`crate::snapshot::render_key`] (the words: [`GeometrySource::text_words`]).
#[derive(Clone, Debug, Default)]
pub struct ExternalGeometry {
    /// Element boxes in tree order.
    pub boxes: Vec<(u64, ExternalBox)>,
    pub sticky: Vec<(u64, ExternalSticky)>,
    /// Scroll size (at least the viewport).
    pub document_width: f64,
    pub document_height: f64,
}

/// A viewport point to hit-test ([`GeometrySource::hit_candidates`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitProbe {
    pub x: f64,
    pub y: f64,
    /// Only `y` counts (WebDriver's vertical hit test).
    pub ignore_x: bool,
    /// The window scroll offset: a box outside every `position: fixed` box
    /// lies at the point plus it in document coordinates.
    pub window_scroll: (f64, f64),
}

/// One element [`GeometrySource::hit_candidates`] reports: a candidate
/// (`hit`: its box may hold the point) or an ancestor of one, with its box
/// and its place in tree order among the reported elements.
#[derive(Clone, Debug, PartialEq)]
pub struct HitVisit {
    pub key: u64,
    pub order: u32,
    pub bounds: ExternalBox,
    pub hit: bool,
}

/// An element's CSS boxes as quads (CDP `DOM.getBoxModel` and
/// `getContentQuads`), document CSS px (viewport ones inside a fixed box).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExternalQuads {
    /// The border box as four points (x, y), clockwise from its top-left
    /// corner, after the transforms of the element and its containing
    /// blocks.
    pub border: [f64; 8],
    /// Whether a transform other than a translation applies (else `border`
    /// is the element's axis-aligned box).
    pub transformed: bool,
    /// Border, padding and margin widths ([`TOP`], [`RIGHT`], [`BOTTOM`],
    /// [`LEFT`]), in the element's own (untransformed) space.
    pub border_widths: [f64; 4],
    pub padding: [f64; 4],
    pub margin: [f64; 4],
    /// The untransformed border box size (`offsetWidth`/`offsetHeight`).
    pub width: f64,
    pub height: f64,
    /// One border-box quad per line fragment of an inline box laid out over
    /// several lines (empty for other boxes).
    pub fragments: Vec<[f64; 8]>,
    pub fixed: bool,
}

/// What a sub-resource a geometry source loads is for (the request's
/// destination: its `Accept`, `Sec-Fetch-Dest` and CDP resource type).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubresourceKind {
    Image,
    Font,
    /// An `@import`ed sheet.
    Style,
}

/// How a document's geometry source reaches the network for what it lays the
/// document out with (images, fonts, imported sheets): the embedder's loader
/// for that document ([`crate::layout::Layout::set_subresource_loader`]), which
/// sends each request as the document's own sub-resource requests go, through
/// its browser context (proxy, user agent, cookies, extra headers, offline
/// state, interception). A geometry source without one loads nothing from the
/// network.
pub trait SubresourceLoader: Send + Sync {
    /// Starts loading `url`. `done` gets the body of a successful response,
    /// or why there is none, on any thread. A loader that can no longer send
    /// the request (its document is gone) drops `done` without calling it.
    fn load(&self, url: String, kind: SubresourceKind, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>);
    /// The bodies loaded for the documents that go out with this one's
    /// network identity, which they reuse ([`SubresourceBodies`]).
    fn bodies(&self) -> &std::sync::Arc<SubresourceBodies>;
}

/// Sub-resource bodies loaded from the network for the layouts of the
/// documents of one network identity: the pages of a browser context with a
/// proxy or user agent of its own, or of every context that goes out as the
/// browser does. A body serves its identity only: one loaded through one
/// proxy, or as one user agent, never stands in for a request another
/// identity's document would send.
#[derive(Default)]
pub struct SubresourceBodies {
    bodies: std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<Vec<u8>>>>,
    arrivals: std::sync::atomic::AtomicU64,
}

impl SubresourceBodies {
    /// The bodies by URL (what a load in flight or failed leaves is the
    /// embedder's loading policy).
    pub fn lock(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, std::sync::Arc<Vec<u8>>>> {
        self.bodies.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A body arrived: the documents of this identity laid out without it
    /// lay out again.
    pub fn arrived(&self) {
        self.arrivals.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// How many bodies arrived ([`Self::arrived`]).
    pub fn arrivals(&self) -> u64 {
        self.arrivals.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// A body the documents' own loader fetched (a `<style>`'s `@import`ed
    /// sheet) that their layouts would otherwise fetch again, behind the
    /// page's back: they take it from here. The documents laid out without
    /// it (an empty entry, the embedder's load in flight) or with another
    /// body lay out again.
    pub fn provide(&self, url: &str, body: &[u8]) {
        if body.is_empty() {
            return;
        }
        let previous = self.lock().insert(url.to_owned(), std::sync::Arc::new(body.to_vec()));
        if previous.is_some_and(|b| b.is_empty() || **b != *body) {
            self.arrived();
        }
    }
}

/// The engine laying the document out (Blitz in `render` builds).
pub trait GeometrySource {
    /// Brings the engine's layout up to the document; the other queries
    /// answer from it.
    fn sync(&self, dom: &Dom, layout: &Layout);
    /// Brings the engine's styles up to the document, leaving layout to
    /// the next [`GeometrySource::sync`] ([`GeometrySource::element_style`]
    /// answers from it).
    fn sync_styles(&self, _dom: &Dom, _style: &crate::style::StyleEngine) {}
    /// One element's box as of the last sync (`None`: not rendered).
    fn element_box(&self, key: u64) -> Option<ExternalBox>;
    /// One element's boxes as quads as of the last sync (`None`: not
    /// rendered, or the engine does not report them).
    fn element_quads(&self, _key: u64) -> Option<ExternalQuads> {
        None
    }
    /// The pieces of one element's border box when the engine splits it into fragments (column
    /// breaks), each where it is shown (document coordinates, as [`ExternalBox`]'s: x, y, width,
    /// height), as of the last sync; `None` when it is one piece. Hit testing asks only while
    /// [`GeometrySource::has_fragmented_boxes`].
    fn element_fragments(&self, _key: u64) -> Option<Vec<[f64; 4]>> {
        None
    }
    /// Whether any box may be split into fragments (as of the last sync).
    fn has_fragmented_boxes(&self) -> bool {
        false
    }
    /// The whole document's boxes as of the last sync.
    fn geometry(&self) -> Option<ExternalGeometry>;
    /// Every word of every text node as of the last sync, in layout order
    /// (`None`: the engine reports none).
    fn text_words(&self) -> Option<Vec<(u64, ExternalWord)>> {
        None
    }
    /// [`GeometrySource::text_words`] for the text nodes below the element
    /// `key` (more words may come; the caller filters), as of the last sync,
    /// in the same order (`None`: the engine reads no part of the document
    /// alone, or `key` is not one of its nodes).
    fn subtree_words(&self, _key: u64) -> Option<Vec<(u64, ExternalWord)>> {
        None
    }
    /// The `position: sticky` elements as of the last sync, with their
    /// inputs: [`ExternalGeometry::sticky`] without the boxes.
    fn sticky_elements(&self) -> Vec<(u64, ExternalSticky)> {
        self.geometry().map(|g| g.sticky).unwrap_or_default()
    }
    /// The elements whose box may hold `probe` and their ancestors, in
    /// tree order, from a descent of the engine's tree that skips the
    /// subtrees that cannot hold it (`None`: the engine has no such
    /// descent, and the whole document's boxes are tested). `shifted`: the
    /// keys of the elements this document moves from where the engine laid
    /// them out (scrolled containers, sticky boxes); `offset`: how far any
    /// element's box is moved (document px added to the point), consulted
    /// when `shifted` is not empty. The candidates are a superset: the
    /// caller tests each box itself.
    fn hit_candidates(
        &self,
        _probe: &HitProbe,
        _offset: &dyn Fn(u64) -> (f64, f64),
        _shifted: &[u64],
    ) -> Option<Vec<HitVisit>> {
        None
    }
    /// The document's scroll size as of the last sync.
    fn document_size(&self) -> Option<(f64, f64)>;
    /// Whether any element is `position: sticky` (as of the last sync).
    fn has_sticky(&self) -> bool;
    /// The document's URL (relative sub-resources resolve against it).
    fn set_base_url(&self, _url: &str) {}
    /// The document's network, for the sub-resources it is laid out with
    /// (see [`SubresourceLoader`]).
    fn set_subresource_loader(&self, _loader: std::sync::Arc<dyn SubresourceLoader>) {}
    /// The source itself (its owner reaches its engine's document).
    fn as_any(&self) -> &dyn std::any::Any;
    /// Whether the source answers now (else nothing is laid out).
    fn active(&self) -> bool {
        true
    }
    /// Whether a style rule of the engine's sheets depends on `:active`
    /// (as of the last sync); true when the engine cannot tell.
    fn styles_depend_on_active(&self) -> bool {
        true
    }
    /// Whether the engine must sync again although the document did not
    /// change (a sub-resource it laid out without arrived since).
    fn stale(&self) -> bool {
        false
    }
    /// One element's computed style as of the last sync (`None`: the engine
    /// has none for it, and the document's own cascade answers).
    fn element_style(&self, _key: u64) -> Option<style::servo_arc::Arc<style::properties::ComputedValues>> {
        None
    }
    /// Whether the element is outside every `display: none` subtree of the
    /// rendered tree, from the styles the engine keeps as of the last style
    /// sync (`None`: the engine cannot tell, and the derived
    /// [`crate::style::ComputedStyle`] answers).
    fn element_displayed(&self, _key: u64) -> Option<bool> {
        None
    }
    /// The elements whose style was replaced since the last call, by key,
    /// with the style before and after; recorded while the engine's
    /// [`crate::style::StyleEngine::track_style_changes`] is on.
    fn take_style_changes(
        &self,
    ) -> Vec<(u64, style::servo_arc::Arc<style::properties::ComputedValues>, style::servo_arc::Arc<style::properties::ComputedValues>)>
    {
        Vec::new()
    }
    /// The computed style of one element's pseudo-element `pseudo`
    /// (`getComputedStyle(el, pseudo)`) as of the last sync; with
    /// `generated_only`, only a stored style of a pseudo-element its rules
    /// generate.
    fn element_pseudo_style(
        &self,
        _key: u64,
        _pseudo: &style::selector_parser::PseudoElement,
        _generated_only: bool,
    ) -> Option<style::servo_arc::Arc<style::properties::ComputedValues>> {
        None
    }
    /// `false` when [`Self::element_pseudo_style`] with `generated_only`
    /// answers `None` for `::before` / `::after` of every element as of the
    /// last sync (no element is styled with a generated one).
    fn may_have_generated_before_after(&self) -> bool {
        true
    }
}

/// An element's boxes (document coordinates; viewport ones inside a fixed
/// box) and its place in tree order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayoutBox {
    pub rect: Rect,
    pub fixed: bool,
    pub client_width: f64,
    pub client_height: f64,
    pub scroll_width: f64,
    pub scroll_height: f64,
    /// See [`ExternalBox::scroll_reversed`].
    pub scroll_reversed: [bool; 2],
    pub border: [f64; 4],
    pub padding: [f64; 4],
    pub position: BoxPosition,
    /// Tree order (1-based), for paint order among equals.
    pub order: u32,
    /// It or an ancestor is positioned: painted in the positioned layer.
    pub in_positioned: bool,
}

impl LayoutBox {
    pub fn from_external(b: &ExternalBox, order: u32) -> LayoutBox {
        LayoutBox {
            rect: Rect { x: b.x, y: b.y, width: b.width, height: b.height },
            fixed: b.fixed,
            client_width: b.client_width,
            client_height: b.client_height,
            scroll_width: b.scroll_width,
            scroll_height: b.scroll_height,
            scroll_reversed: b.scroll_reversed,
            border: b.border,
            padding: b.padding,
            position: b.position,
            order,
            in_positioned: b.position != BoxPosition::Static,
        }
    }

    /// Positioned (relative/absolute/fixed/sticky), or inside such a box:
    /// painted above in-flow boxes.
    pub fn positioned(&self) -> bool {
        self.in_positioned
    }
}

/// One word of a text node on a line.
#[derive(Clone, Debug)]
pub struct TextRun {
    pub node: NodeId,
    /// UTF-16 offsets of the word in the node's data.
    pub start: u32,
    pub end: u32,
    pub rect: Rect,
    pub fixed: bool,
    /// Caret positions (offset, document x) from `start` to `end`.
    edges: Rc<[(u32, f64)]>,
}

impl TextRun {
    /// Caret positions (UTF-16 offset, document x) from `start` to `end`.
    pub fn edges(&self) -> &[(u32, f64)] {
        &self.edges
    }

    /// The offset of the character boundary nearest to `dx` px from the
    /// run's left edge, as Chromium places a caret under a point.
    pub fn offset_at(&self, dx: f64) -> u32 {
        let x = self.rect.x + dx;
        self.edges.iter().min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs())).map_or(self.start, |e| e.0)
    }
}

/// The whole document's boxes (or, for one hit test, the boxes of the
/// elements it may hit and of their ancestors: see
/// [`crate::layout::Layout::element_from_point`]).
#[derive(Clone, Debug, Default)]
pub struct LayoutResult {
    pub boxes: FxHashMap<NodeId, LayoutBox>,
    /// Document scroll size (at least the viewport).
    pub document_width: f64,
    pub document_height: f64,
    pub sticky: FxHashMap<NodeId, ExternalSticky>,
    /// Hit testing's derived data, filled per box on first use: it depends
    /// only on the tree, the styles and the boxes, which this result is
    /// valid for (a new result is read after any of them change).
    pub(crate) hit: RefCell<HitTestData>,
}

/// The whole document's words, for text ranges, carets and selection.
#[derive(Clone, Debug, Default)]
pub struct TextResult {
    /// Text nodes: the union of their words, and whether inside a fixed box.
    pub texts: FxHashMap<NodeId, (Rect, bool)>,
    /// Every word, in layout order.
    pub text_runs: Vec<TextRun>,
}

impl TextResult {
    pub fn from_words(dom: &Dom, words: Vec<(u64, ExternalWord)>) -> TextResult {
        let mut out = TextResult::default();
        let mut seen = FxHashSet::default();
        for (key, w) in words {
            let Some(node) = node_of(dom, key) else { continue };
            let (Some(first), Some(last)) = (w.edges.first(), w.edges.last()) else { continue };
            let rect = Rect { x: w.x, y: w.y, width: w.width, height: w.height };
            let (start, end) = (first.0.min(last.0), first.0.max(last.0));
            let union = out.texts.entry(node).or_insert((rect, w.fixed));
            if seen.insert(node) {
                *union = (rect, w.fixed);
            } else {
                let r = union.0;
                let (x0, y0) = (r.x.min(rect.x), r.y.min(rect.y));
                let (x1, y1) = ((r.x + r.width).max(rect.x + rect.width), (r.y + r.height).max(rect.y + rect.height));
                union.0 = Rect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 };
            }
            out.text_runs.push(TextRun { node, start, end, rect, fixed: w.fixed, edges: w.edges.into() });
        }
        out
    }
}

/// One stacking level of a paint key: (layer, z-index, box order).
pub(crate) type PaintEntry = (u8, i32, u32);

/// The paint-order state after an element's ancestor chain (itself
/// included), from which its own and its descendants' paint keys follow.
#[derive(Clone, Debug, Default)]
pub(crate) struct PaintChain {
    /// The stacking contexts entered, outermost first.
    pub(crate) contexts: Rc<[PaintEntry]>,
    /// The z-auto positioned box whose layer the content belongs to.
    pub(crate) positioned_layer: Option<PaintEntry>,
    /// The last box of the chain makes its own layer.
    pub(crate) own_layer: bool,
}

/// Per-box hit-testing data of one [`LayoutResult`].
#[derive(Clone, Debug, Default)]
pub(crate) struct HitTestData {
    pub(crate) chains: FxHashMap<NodeId, PaintChain>,
    pub(crate) keys: FxHashMap<NodeId, Rc<[PaintEntry]>>,
    /// The overflow clips that apply to an element: (ancestor, its clipped
    /// axes), nearest first.
    pub(crate) clips: FxHashMap<NodeId, Rc<[(NodeId, crate::style::ScrollAxes)]>>,
}

/// The node `key` names, when it is still in `dom`.
pub fn node_of(dom: &Dom, key: u64) -> Option<NodeId> {
    let id = NodeId { idx: key as u32, gen: (key >> 32) as u32 };
    dom.check(id).ok().map(|_| id)
}

impl LayoutResult {
    pub fn get(&self, el: NodeId) -> Option<&LayoutBox> {
        self.boxes.get(&el)
    }

    pub fn from_external(dom: &Dom, g: ExternalGeometry) -> LayoutResult {
        let mut out = LayoutResult {
            document_width: g.document_width,
            document_height: g.document_height,
            ..LayoutResult::default()
        };
        // Tree order: an element's ancestors are in before it.
        for (i, (key, b)) in g.boxes.iter().enumerate() {
            if let Some(el) = node_of(dom, *key) {
                out.insert_box(dom, el, LayoutBox::from_external(b, i as u32 + 1));
            }
        }
        for (key, s) in g.sticky {
            if let Some(el) = node_of(dom, key) {
                out.sticky.insert(el, s);
            }
        }
        out
    }

    /// Adds `el`'s box, inheriting the positioned layer from the nearest
    /// ancestor with a box (the ancestors are added before their
    /// descendants).
    pub(crate) fn insert_box(&mut self, dom: &Dom, el: NodeId, mut b: LayoutBox) {
        let mut cur = dom.parent_element(el).ok().flatten();
        while let Some(p) = cur {
            if let Some(pb) = self.boxes.get(&p) {
                b.in_positioned |= pb.in_positioned;
                break;
            }
            cur = dom.parent_element(p).ok().flatten();
        }
        self.boxes.insert(el, b);
    }
}
