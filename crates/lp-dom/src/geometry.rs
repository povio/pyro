//! The geometry layout queries report: what an external engine (Blitz, in
//! lp-render) laid the document out as. lp-dom has no layout of its own; a
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
}

/// The whole document as the external engine laid it out, keyed by
/// [`crate::snapshot::render_key`].
#[derive(Clone, Debug, Default)]
pub struct ExternalGeometry {
    /// Element boxes in tree order.
    pub boxes: Vec<(u64, ExternalBox)>,
    /// Words of text nodes, in layout order.
    pub words: Vec<(u64, ExternalWord)>,
    pub sticky: Vec<(u64, ExternalSticky)>,
    /// Scroll size (at least the viewport).
    pub document_width: f64,
    pub document_height: f64,
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
    /// The whole document as of the last sync.
    fn geometry(&self) -> Option<ExternalGeometry>;
    /// The document's scroll size as of the last sync.
    fn document_size(&self) -> Option<(f64, f64)>;
    /// Whether any element is `position: sticky` (as of the last sync).
    fn has_sticky(&self) -> bool;
    /// The document's URL (relative sub-resources resolve against it).
    fn set_base_url(&self, _url: &str) {}
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

/// The whole document's geometry.
#[derive(Clone, Debug, Default)]
pub struct LayoutResult {
    pub boxes: FxHashMap<NodeId, LayoutBox>,
    /// Text nodes: the union of their words, and whether inside a fixed box.
    pub texts: FxHashMap<NodeId, (Rect, bool)>,
    /// Every word, in layout order.
    pub text_runs: Vec<TextRun>,
    /// Document scroll size (at least the viewport).
    pub document_width: f64,
    pub document_height: f64,
    pub sticky: FxHashMap<NodeId, ExternalSticky>,
    /// Hit testing's derived data, filled per box on first use: it depends
    /// only on the tree, the styles and the boxes, which this result is
    /// valid for (a new result is read after any of them change).
    pub(crate) hit: RefCell<HitTestData>,
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
                let mut b = LayoutBox::from_external(b, i as u32 + 1);
                let mut cur = dom.parent_element(el).ok().flatten();
                while let Some(p) = cur {
                    if let Some(pb) = out.boxes.get(&p) {
                        b.in_positioned |= pb.in_positioned;
                        break;
                    }
                    cur = dom.parent_element(p).ok().flatten();
                }
                out.boxes.insert(el, b);
            }
        }
        let mut seen = FxHashSet::default();
        for (key, w) in g.words {
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
        for (key, s) in g.sticky {
            if let Some(el) = node_of(dom, key) {
                out.sticky.insert(el, s);
            }
        }
        out
    }
}
