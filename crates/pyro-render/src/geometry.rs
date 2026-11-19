//! Element boxes for layout queries (`getBoundingClientRect` and friends),
//! read from a Blitz document the way Chromium reports them. What Blitz lays
//! out and what Chromium reports differ in four places, adjusted here:
//!
//! - Boxes are the unrounded (fractional) layout; painting uses the rounded one.
//! - Table rows and row groups have no box in Blitz (cells sit in the table's
//!   grid): theirs is the union of their cells' (rows').
//! - Inline elements have no box either (their content is in the inline
//!   root's text layout): theirs is the union of their fragments.
//! - Collapsed table borders are gutters between borderless cells in Blitz
//!   (the first cell's widest borders; row borders are left out). Chromium's
//!   cells take half of each collapsed border, the row's included.
//!
//! Boxes are computed per element on request from the document as it is:
//! nothing is memoized between queries. An element's transform walks its
//! containing-block chain (O(depth)), and layout-tree membership reads the
//! list owner Blitz records on each node as it writes the layout child lists
//! (`Node::layout_child_of`).

use stylo_taffy::CopyStyleGetters;

use blitz_dom::{BaseDocument, NodeId};
use kurbo::{Affine, Rect};

/// One element's boxes, CSS px.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayoutBox {
    /// Border box, document coordinates (viewport coordinates inside a
    /// `position: fixed` box, at window scroll 0).
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Padding box size less scrollbars.
    pub client_width: f64,
    pub client_height: f64,
    pub scroll_width: f64,
    pub scroll_height: f64,
    /// Whether it scrolls from the end of the horizontal and the vertical axis (its scroll
    /// origin there: `direction: rtl`, `row-reverse`, `column-reverse`, `wrap-reverse`), to
    /// negative offsets.
    pub scroll_reversed: [bool; 2],
    /// Inside (or itself) a `position: fixed` box.
    pub fixed: bool,
    /// Border and padding widths: top, right, bottom, left.
    pub border: [f64; 4],
    pub padding: [f64; 4],
    pub position: BoxPosition,
    /// pyro: discarded by a line clamp (it or an ancestor in the clamping box): laid out, not
    /// painted, and not hit (blitz-dom `NodeFlags::HIDDEN_BY_CLAMP`).
    pub hidden: bool,
    /// pyro: the border box's origin with no CSS transform applied (document coordinates, as
    /// `x`/`y`), when a transform on its containing-block chain moves it: `offsetLeft` and
    /// `offsetTop` ignore transforms (CSSOM View; Chromium).
    pub layout_origin: Option<[f64; 2]>,
}

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

/// A `position: sticky` element's inputs (its box is laid out as if static).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StickyBox {
    /// `top`, `right`, `bottom`, `left`: px plus a fraction of the scrollport
    /// (`None`: auto).
    pub insets: [Option<(f64, f64)>; 4],
    /// Its containing block's content box (the table's for table parts),
    /// document coordinates: the box never leaves it.
    pub limit: Option<(f64, f64, f64, f64)>,
    /// The key of its containing block's element, which the sticky boxes holding it move.
    pub cb: Option<u64>,
}

/// A box's place in its containing block (CSS px): its location, then its CSS transform.
pub(crate) fn placement(doc: &BaseDocument, node: &blitz_dom::Node, scale: f64) -> Affine {
    let location = doc.physical_unrounded_layout(node.id).location;
    // Blitz keeps the CSS transform's translation in device px
    // (`Node::set_transform`); this chain is in CSS px.
    let own = node.transform().as_deref().copied().map_or(Affine::IDENTITY, |t| {
        let [a, b, c, d, e, f] = t.as_coeffs();
        Affine::new([a, b, c, d, e / scale, f / scale])
    });
    Affine::translate((location.x as f64, location.y as f64)) * own
}

/// pyro: the move into its column of the box `chain[0]` in a multi-column container's strip:
/// `chain` is its containing-block chain, leaf first, up to the container's child (blitz-dom
/// `layout::multicol`).
fn column_shift(
    doc: &BaseDocument,
    columns: &blitz_dom::ColumnFragments,
    chain: &[&blitz_dom::Node],
    scale: f64,
) -> Affine {
    let last = chain.len() - 1;
    // The box's origin in the container's strip, through the columns of any multi-column
    // container nested between them (where those show it: their rows lie in this strip).
    let mut origin = kurbo::Point::ZERO;
    for j in 0..last {
        origin = placement(doc, chain[j], scale) * origin;
        let parent = chain[j + 1];
        if let Some(nested) = parent.column_fragments() {
            let (x, y) = nested.shift_at(origin.y as f32);
            origin += kurbo::Vec2::new(x as f64, y as f64);
        }
        let scroll = parent.scroll_offset();
        origin -= kurbo::Vec2::new(scroll.x, scroll.y);
    }
    let origin = placement(doc, chain[last], scale) * origin;
    let (x, y) = columns.shift_at(origin.y as f32);
    Affine::translate((x as f64, y as f64))
}

/// pyro: for inline root `root_id` in a multi-column container (or being one), the move
/// (document CSS px) of its line at block position `y` (its own coordinates) into the column
/// that shows it, past the one [`Geometry::transform`] gives the root (by the root's top). `None`
/// when no multi-column container shows the root.
pub(crate) fn line_column_shift(doc: &BaseDocument, root_id: NodeId) -> Option<impl Fn(f64) -> Affine + '_> {
    if !doc.has_multicol() {
        return None;
    }
    let scale = doc.viewport().scale_f64();
    let root = doc.get_node(root_id)?;
    // The root's box to the strip of the container that shows it.
    let (columns, to_strip) = match root.column_fragments() {
        Some(columns) => (columns, Affine::IDENTITY),
        None => {
            let mut chain = vec![root];
            loop {
                let cb = doc.get_node(chain.last()?.containing_block()?)?;
                if let Some(columns) = cb.column_fragments() {
                    let last = chain.len() - 1;
                    let mut t = placement(doc, chain[last], scale);
                    for j in (0..last).rev() {
                        let scroll = chain[j + 1].scroll_offset();
                        t = t * Affine::translate((-scroll.x, -scroll.y)) * placement(doc, chain[j], scale);
                    }
                    break (columns, t);
                }
                chain.push(cb);
            }
        }
    };
    let base = columns.shift_at((to_strip * kurbo::Point::ZERO).y as f32);
    Some(move |y: f64| {
        let (x, y) = columns.shift_at((to_strip * kurbo::Point::new(0.0, y)).y as f32);
        Affine::translate(((x - base.0) as f64, (y - base.1) as f64))
    })
}

fn box_position(node: &blitz_dom::Node) -> BoxPosition {
    use style::computed_values::position::T;
    match node.primary_styles().map(|s| s.clone_position()) {
        Some(T::Relative) => BoxPosition::Relative,
        Some(T::Absolute) => BoxPosition::Absolute,
        Some(T::Fixed) => BoxPosition::Fixed,
        Some(T::Sticky) => BoxPosition::Sticky,
        _ => BoxPosition::Static,
    }
}

impl LayoutBox {
    fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.x + self.width, self.y + self.height)
    }

    fn from_rect(r: Rect, fixed: bool) -> LayoutBox {
        LayoutBox {
            x: r.x0,
            y: r.y0,
            width: r.width(),
            height: r.height(),
            client_width: r.width(),
            client_height: r.height(),
            scroll_width: r.width(),
            scroll_height: r.height(),
            fixed,
            ..LayoutBox::default()
        }
    }
}

/// Layout queries of one persistent document. It holds no state: every
/// query reads the document.
#[derive(Default)]
pub(crate) struct Geometry;

impl Geometry {
    /// Whether `id` is a box of the current layout: the root element, or a
    /// layout (or hoisted) child of its layout parent. O(1): Blitz records
    /// which list holds each layout child when it builds the lists
    /// (`Node::layout_child_of`), and Taffy the containing block of each
    /// hoisted box (`Node::oof_containing_block`).
    pub(crate) fn in_layout_tree(&self, doc: &BaseDocument, id: NodeId) -> bool {
        if doc.try_root_element().is_some_and(|root| root.id == id) {
            return true;
        }
        let Some(node) = doc.get_node(id) else { return false };
        let Some(parent) = node.layout_parent.get() else { return false };
        let found = doc.get_node(parent).is_some()
            && (node.layout_child_of.get() == Some(parent) || node.oof_containing_block.get() == Some(parent));
        debug_assert_eq!(found, in_layout_tree(doc, id), "layout tree membership of {id:?} under {parent:?}");
        found
    }

    /// Whether any element is `position: sticky` (the set Blitz's style
    /// traversal keeps, not a scan of the document).
    pub(crate) fn has_sticky(&self, doc: &BaseDocument) -> bool {
        doc.has_sticky()
    }

    /// `id`'s border box to the document and whether it is in a fixed box:
    /// composed down its containing-block chain, root first.
    pub(crate) fn transform(&self, doc: &BaseDocument, id: NodeId) -> Option<(Affine, bool)> {
        self.transform_and_planes(doc, id).map(|(t, fixed, _)| (t, fixed))
    }

    /// pyro: the origin of `id`'s border box as its containing-block chain places it without
    /// the CSS transforms on it (scroll offsets and column shifts kept), when one has a
    /// transform. `None` when none has.
    fn untransformed_origin(&self, doc: &BaseDocument, id: NodeId) -> Option<[f64; 2]> {
        let mut chain = Vec::new();
        let mut cur = Some(id);
        while let Some(i) = cur {
            let node = doc.get_node(i)?;
            chain.push(node);
            cur = node.containing_block();
        }
        if !chain.iter().any(|node| node.transform().is_some()) {
            return None;
        }
        let scale = doc.viewport().scale_f64();
        let multicol = doc.has_multicol();
        let mut t = Affine::IDENTITY;
        let mut parent: Option<&blitz_dom::Node> = None;
        for (index, &node) in chain.iter().enumerate().rev() {
            let mut base = match parent {
                Some(p) => {
                    let scroll = p.scroll_offset();
                    t * Affine::translate((-scroll.x, -scroll.y))
                }
                None => Affine::IDENTITY,
            };
            if let Some(columns) = parent.filter(|_| multicol).and_then(|p| p.column_fragments()) {
                base *= column_shift(doc, columns, &chain[..=index], scale);
            }
            let location = doc.physical_unrounded_layout(node.id).location;
            t = base * Affine::translate((location.x as f64, location.y as f64));
            parent = Some(node);
        }
        let origin = t * kurbo::Point::ZERO;
        Some([origin.x, origin.y])
    }

    /// [`Self::transform`], and whether a plane on the chain is placed by its homography (drawn
    /// through a perspective, or in a context element placed so; then [`Self::projective`] places
    /// the box).
    fn transform_and_planes(&self, doc: &BaseDocument, id: NodeId) -> Option<(Affine, bool, bool)> {
        let mut chain = Vec::new();
        let mut projective = false;
        let mut cur = Some(id);
        while let Some(i) = cur {
            let node = doc.get_node(i)?;
            projective |= node.plane_3d().is_some_and(is_projective_plane);
            chain.push(node);
            cur = node.containing_block();
        }
        let scale = doc.viewport().scale_f64();
        let multicol = doc.has_multicol();
        let (mut t, mut fixed) = (Affine::IDENTITY, false);
        let mut parent: Option<&blitz_dom::Node> = None;
        for (index, &node) in chain.iter().enumerate().rev() {
            let mut base = match parent {
                Some(p) => {
                    let scroll = p.scroll_offset();
                    t * Affine::translate((-scroll.x, -scroll.y))
                }
                None => Affine::IDENTITY,
            };
            // pyro: a box in a multi-column container is shown in the column whose strip range
            // holds the box `id` itself (blitz-dom `layout::multicol`).
            if let Some(columns) = parent.filter(|_| multicol).and_then(|p| p.column_fragments()) {
                base *= column_shift(doc, columns, &chain[..=index], scale);
            }
            fixed = fixed
                || node
                    .primary_styles()
                    .is_some_and(|s| s.clone_position() == style::computed_values::position::T::Fixed);
            t = base * placement(doc, node, scale);
            parent = Some(node);
        }
        Some((t, fixed, projective))
    }

    /// pyro (lane H2): `id`'s border box to the document as a homography (CSS px), when a plane
    /// on its containing-block chain is placed by its homography (`Plane3D::places_by_local`): a 2D
    /// transform only stands in for such a plane, and a layout query reports the box where paint
    /// draws it: each such plane is composed by its homography (`Plane3D::local`, into its parent's
    /// coordinates as this chain places the parent). `None` otherwise (the chain's affine is exact).
    pub(crate) fn projective(&self, doc: &BaseDocument, id: NodeId) -> Option<Homography> {
        let mut chain = Vec::new();
        let mut projective = false;
        let mut cur = Some(id);
        while let Some(i) = cur {
            let node = doc.get_node(i)?;
            projective |= node.plane_3d().is_some_and(is_projective_plane);
            chain.push(node);
            cur = node.containing_block();
        }
        if !projective {
            return None;
        }
        let scale = doc.viewport().scale_f64();
        let mut t = Homography::IDENTITY;
        let mut parent: Option<&blitz_dom::Node> = None;
        for node in chain.into_iter().rev() {
            let base = match parent {
                Some(p) => {
                    let scroll = p.scroll_offset();
                    t.then_affine_before(Affine::translate((-scroll.x, -scroll.y)))
                }
                None => Homography::IDENTITY,
            };
            let location = doc.physical_unrounded_layout(node.id).location;
            let origin = base.then_affine_before(Affine::translate((location.x as f64, location.y as f64)));
            let own = node.transform().as_deref().copied().map_or(Affine::IDENTITY, |t| {
                let [a, b, c, d, e, f] = t.as_coeffs();
                Affine::new([a, b, c, d, e / scale, f / scale])
            });
            t = match node.plane_3d().filter(|plane| is_projective_plane(plane)) {
                Some(plane) => origin.mul(&Homography(plane.local)),
                None => origin.then_affine_before(own),
            };
            parent = Some(node);
        }
        Some(t)
    }

    /// pyro: the border box of `id` in the columns of the nearest multi-column container on its
    /// containing-block chain (blitz-dom `layout::multicol`): the bounding box of its pieces
    /// ([`Geometry::columns_pieces`], document CSS px), as Chromium unites a box's fragments for
    /// `getBoundingClientRect`. `None` when no such container shows it.
    pub(crate) fn columns_box(&self, doc: &BaseDocument, id: NodeId) -> Option<Rect> {
        self.columns_pieces(doc, id)?.into_iter().reduce(|a, b| a.union(b))
    }

    /// pyro: the border box of `id` cut at the column breaks it spans, each piece shown in its
    /// column (document CSS px; Chromium's fragments, which `getClientRects` lists and hit
    /// testing tests one by one). `None` when no multi-column container shows it.
    pub(crate) fn columns_pieces(&self, doc: &BaseDocument, id: NodeId) -> Option<Vec<Rect>> {
        if !doc.has_multicol() {
            return None;
        }
        let mut chain = vec![doc.get_node(id)?];
        let container = loop {
            let cb = doc.get_node(chain.last()?.containing_block()?)?;
            if cb.column_fragments().is_some() {
                break cb;
            }
            chain.push(cb);
        };
        let scale = doc.viewport().scale_f64();
        // `chain` (leaf first, up to the container's child) places a box in the container's strip.
        let to_strip = |chain: &[&blitz_dom::Node]| {
            let last = chain.len() - 1;
            let mut below = placement(doc, chain[last], scale);
            for j in (0..last).rev() {
                let scroll = chain[j + 1].scroll_offset();
                below = below * Affine::translate((-scroll.x, -scroll.y)) * placement(doc, chain[j], scale);
            }
            below
        };
        // A strip rectangle cut at the container's breaks, each piece where its column shows it
        // (the container's coordinates), with its height in the strip.
        let cut = |columns: &blitz_dom::ColumnFragments, strip: Rect, out: &mut Vec<(Rect, f64)>| {
            for f in &columns.fragments {
                let (y0, y1) = (strip.y0.max(f.start as f64), strip.y1.min(f.end as f64));
                let shows =
                    if strip.height() > 0.0 { y1 > y0 } else { strip.y0 >= f.start as f64 && strip.y0 < f.end as f64 };
                if shows {
                    let piece = Rect::new(strip.x0, y0, strip.x1, y1) + kurbo::Vec2::new(f.offset.x as f64, f.offset.y as f64);
                    out.push((piece, y1 - y0));
                }
            }
        };
        let l = doc.physical_unrounded_layout(id);
        let strip = to_strip(&chain).transform_rect_bbox(Rect::new(0.0, 0.0, l.size.width as f64, l.size.height as f64));
        let mut out: Vec<(Rect, f64)> = Vec::new();
        cut(container.column_fragments()?, strip, &mut out);
        // pyro: a sliver under a pixel where a box meets a column break (its unrounded strip
        // position against the rounded break) is no fragment of it: a monolithic box a column
        // holds stays one box (WPT css-overflow/scroll-markers/targeted-column-scroll-marker-*).
        if out.iter().any(|&(_, height)| height >= 1.0) {
            out.retain(|&(_, height)| height >= 1.0);
        }
        // pyro: a container nested in another's columns has its rows there too: the pieces are cut
        // again at the outer container's breaks, out to the outermost one (as paint shows them).
        let mut container = container;
        loop {
            let mut outer_chain = vec![container];
            let outer = loop {
                match outer_chain.last().and_then(|n| n.containing_block()).and_then(|cb| doc.get_node(cb)) {
                    Some(cb) if cb.column_fragments().is_some() => break Some(cb),
                    Some(cb) => outer_chain.push(cb),
                    None => break None,
                }
            };
            let Some(outer) = outer else { break };
            let scroll = container.scroll_offset();
            let into = to_strip(&outer_chain) * Affine::translate((-scroll.x, -scroll.y));
            let columns = outer.column_fragments()?;
            let mut next = Vec::new();
            for (piece, height) in out {
                let before = next.len();
                cut(columns, into.transform_rect_bbox(piece), &mut next);
                for entry in &mut next[before..] {
                    entry.1 = height;
                }
            }
            out = next;
            container = outer;
        }
        let (t, _) = self.transform(doc, container.id)?;
        let scroll = container.scroll_offset();
        let base = t * Affine::translate((-scroll.x, -scroll.y));
        (!out.is_empty()).then(|| out.into_iter().map(|(rect, _)| base.transform_rect_bbox(rect)).collect())
    }

    /// The box Blitz lays out for `id`, before the adjustments. A node that
    /// is not in the layout tree now (an inline element in an inline
    /// formatting context) has none, whatever an earlier layout left in it.
    fn laid_out(&self, doc: &BaseDocument, id: NodeId) -> Option<LayoutBox> {
        let node = doc.get_node(id)?;
        if !self.in_layout_tree(doc, id) {
            let (_, fixed) = self.transform(doc, id)?;
            return Some(LayoutBox { fixed, ..LayoutBox::default() });
        }
        let (t, fixed, projective) = self.transform_and_planes(doc, id)?;
        let l = doc.physical_unrounded_layout(id);
        let border = Rect::new(0.0, 0.0, l.size.width as f64, l.size.height as f64);
        // pyro (lane H2): a box in a plane drawn through a perspective is where paint draws it;
        // pyro: a box in columns is the union of its pieces in them.
        let rect = match projective.then(|| self.projective(doc, id)).flatten() {
            Some(h) => h.bbox(border).unwrap_or(Rect::ZERO),
            None => self.columns_box(doc, id).unwrap_or_else(|| t.transform_rect_bbox(border)),
        };
        let client_width = (l.size.width - l.border.left - l.border.right - l.scrollbar_size.width).max(0.0) as f64;
        let client_height = (l.size.height - l.border.top - l.border.bottom - l.scrollbar_size.height).max(0.0) as f64;
        // Scrollable overflow is kept in device px.
        let o = node.scrollable_overflow().scale_from_origin(1.0 / doc.viewport().scale_f64());
        // A box whose scroll origin is at the end of an axis (Taffy's scrollable overflow carries
        // it: `direction: rtl`, `row-reverse`, `column-reverse`, `wrap-reverse`) scrolls over the
        // overflow on its start side, from the padding box's end edge; one at the start, over
        // the overflow on its end side.
        let reversed = node.final_layout().scrollable_overflow_rect.is_reversed();
        let scroll_width = if reversed.x {
            client_width.max((l.size.width - l.border.right) as f64 - o.x0)
        } else {
            client_width.max(o.x1 - l.border.left as f64)
        };
        let scroll_height = if reversed.y {
            client_height.max((l.size.height - l.border.bottom) as f64 - o.y0)
        } else {
            client_height.max(o.y1 - l.border.top as f64)
        };
        Some(LayoutBox {
            x: rect.x0,
            y: rect.y0,
            width: rect.width(),
            height: rect.height(),
            client_width,
            client_height,
            scroll_width,
            scroll_height,
            scroll_reversed: [reversed.x, reversed.y],
            fixed,
            border: [l.border.top, l.border.right, l.border.bottom, l.border.left].map(f64::from),
            padding: [l.padding.top, l.padding.right, l.padding.bottom, l.padding.left].map(f64::from),
            position: BoxPosition::Static,
            hidden: hidden_by_clamp(doc, id),
            layout_origin: None,
        })
    }

    /// `id`'s box as Chromium reports it (see the module docs), in
    /// Chromium's layout units: each value truncated toward zero to 1/64 px
    /// (100.7px reports 100.6875). The values are exact binary fractions, so
    /// arithmetic on them agrees wherever it is done; a 1/100 unit tolerance
    /// keeps f32 layout noise (19.999998) from losing a whole unit.
    pub(crate) fn element_box(&self, doc: &BaseDocument, id: NodeId) -> Option<LayoutBox> {
        let mut b = self.adjusted_box(doc, id)?;
        // CSSOM View: an inline box has no client or scroll area (atomic
        // inlines, which `display: inline` controls and replaced elements
        // are, do).
        let atomic = matches!(
            tag(doc, id),
            Some(
                "img"
                    | "input"
                    | "textarea"
                    | "select"
                    | "button"
                    | "video"
                    | "canvas"
                    | "iframe"
                    | "object"
                    | "embed"
                    | "svg"
            )
        );
        let inline = !atomic
            && doc.get_node(id).and_then(|n| n.primary_styles()).is_some_and(|s| s.clone_display().is_inline_flow());
        if inline {
            (b.client_width, b.client_height, b.scroll_width, b.scroll_height) = (0.0, 0.0, 0.0, 0.0);
        }
        let unit = |v: f64| v.signum() * ((v.abs() * 64.0 + 0.01).trunc() / 64.0);
        Some(LayoutBox {
            x: unit(b.x),
            y: unit(b.y),
            width: unit(b.width),
            height: unit(b.height),
            client_width: unit(b.client_width),
            client_height: unit(b.client_height),
            scroll_width: unit(b.scroll_width),
            scroll_height: unit(b.scroll_height),
            scroll_reversed: b.scroll_reversed,
            fixed: b.fixed,
            border: b.border.map(unit),
            padding: b.padding.map(unit),
            position: box_position(doc.get_node(id)?),
            hidden: b.hidden,
            layout_origin: self.untransformed_origin(doc, id).map(|origin| origin.map(unit)),
        })
    }

    /// The sticky inputs of `id` (see [`StickyBox`]), and its containing block (whose key the
    /// caller sets as [`StickyBox::cb`]).
    pub(crate) fn sticky_box(&self, doc: &BaseDocument, id: NodeId) -> (StickyBox, Option<NodeId>) {
        let Some(style) = doc.get_node(id).and_then(|n| n.primary_styles()) else { return (StickyBox::default(), None) };
        let inset = |v: &style::values::computed::Inset| {
            let style::values::generics::position::GenericInset::LengthPercentage(lp) = v else { return None };
            let px = lp.resolve(style::values::computed::Length::new(0.0)).px() as f64;
            let hundred = lp.resolve(style::values::computed::Length::new(100.0)).px() as f64;
            Some((px, (hundred - px) / 100.0))
        };
        let insets = [
            inset(&style.slow_clone_top()),
            inset(&style.slow_clone_right()),
            inset(&style.slow_clone_bottom()),
            inset(&style.slow_clone_left()),
        ];
        // Containing block: the table for table parts, else the nearest ancestor that is no
        // inline box (CSS 2.1 § 10.1: the nearest block container; an inline box split around
        // a block-level sticky box is none: WPT `position-sticky-large-top-2`).
        let mut cb = doc.get_node(id).and_then(|n| n.parent);
        if is_table_part(doc, id) || is_cell(doc, id) {
            while let Some(c) = cb.filter(|&c| tag(doc, c) != Some("table")) {
                cb = doc.get_node(c).and_then(|n| n.parent);
            }
        } else {
            let inline = |c: NodeId| {
                doc.get_node(c).and_then(|n| n.primary_styles()).is_some_and(|s| {
                    let display = s.clone_display();
                    display.is_inline_flow() || display.is_contents()
                })
            };
            while let Some(c) = cb.filter(|&c| inline(c)) {
                cb = doc.get_node(c).and_then(|n| n.parent);
            }
            // A table row or row group has no box: the box's containing block is the anonymous
            // cell it is laid out in (WPT `position-sticky-contained-by-display-table`).
            let row_or_group = |c: NodeId| {
                use style::values::specified::box_::DisplayInside;
                doc.get_node(c).and_then(|n| n.primary_styles()).is_some_and(|s| {
                    matches!(
                        s.clone_display().inside(),
                        DisplayInside::TableRow
                            | DisplayInside::TableRowGroup
                            | DisplayInside::TableHeaderGroup
                            | DisplayInside::TableFooterGroup
                    )
                })
            };
            if cb.is_some_and(|c| row_or_group(c) && !self.in_layout_tree(doc, c)) {
                let layout_parent = doc.get_node(id).and_then(|n| n.layout_parent.get());
                if layout_parent.is_some_and(|p| doc.get_node(p).is_some_and(|p| p.is_anonymous())) {
                    cb = layout_parent;
                }
            }
        }
        let limit = cb.and_then(|c| {
            let b = self.element_box(doc, c)?;
            let (t, r, bo, l) = (
                b.border[0] + b.padding[0],
                b.border[1] + b.padding[1],
                b.border[2] + b.padding[2],
                b.border[3] + b.padding[3],
            );
            // A direct child of a scrolling box is bounded by its scrolling
            // content, rather than its unscrolled viewport-sized border box.
            let scroll_container = doc.get_node(c).and_then(|n| n.primary_styles()).is_some_and(|style| {
                use style::values::computed::Overflow;
                [style.clone_overflow_x(), style.clone_overflow_y()]
                    .into_iter()
                    .any(|overflow| overflow != Overflow::Visible)
            });
            let (width, height) = if scroll_container {
                (b.scroll_width + b.border[3] + b.border[1], b.scroll_height + b.border[0] + b.border[2])
            } else {
                (b.width, b.height)
            };
            // One whose scroll origin is at an axis' end (right-to-left, a reversed flex
            // container) has that content on the axis' start side: it reaches back from the
            // scrollport's end (WPT `position-sticky-column-reverse-flex`).
            let (dx, dy) = match scroll_container {
                true => (
                    if b.scroll_reversed[0] { (b.client_width - b.scroll_width).min(0.0) } else { 0.0 },
                    if b.scroll_reversed[1] { (b.client_height - b.scroll_height).min(0.0) } else { 0.0 },
                ),
                false => (0.0, 0.0),
            };
            Some((b.x + l + dx, b.y + t + dy, (width - l - r).max(0.0), (height - t - bo).max(0.0)))
        });
        (StickyBox { insets, limit, cb: None }, cb)
    }

    fn adjusted_box(&self, doc: &BaseDocument, id: NodeId) -> Option<LayoutBox> {
        let mut b = self.laid_out(doc, id)?;
        // A cell with no box now (not in the layout tree: a `<td>` in a
        // block, which Blitz does not wrap in an anonymous table) gets no
        // gutters either: grown by them, its empty box would be hit.
        if is_cell(doc, id) && self.in_layout_tree(doc, id) {
            if let Some((gx, top, bottom)) = cell_gutters(doc, id) {
                b.x -= gx;
                b.y -= top;
                b.width += 2.0 * gx;
                b.height += top + bottom;
            }
            return Some(b);
        }
        if b.width != 0.0 || b.height != 0.0 {
            return Some(b);
        }
        if is_table_part(doc, id) {
            if tag(doc, id) == Some("tr") {
                if let Some(row) = self.row_track_box(doc, id) {
                    return Some(row);
                }
            }
            if let Some(r) = self.children_union(doc, id) {
                return Some(LayoutBox::from_rect(r, b.fixed));
            }
            return Some(b);
        }
        let inline =
            doc.get_node(id).and_then(|n| n.primary_styles()).is_some_and(|s| s.clone_display().is_inline_flow());
        if inline {
            if let Some(r) = self.inline_union(doc, id) {
                return Some(LayoutBox {
                    client_width: 0.0,
                    client_height: 0.0,
                    scroll_width: 0.0,
                    scroll_height: 0.0,
                    ..LayoutBox::from_rect(r, b.fixed)
                });
            }
            // No fragment (an empty span among collapsed white space): a
            // zero box at the content-box origin of the nearest box around it,
            // pyro: the anonymous block laying it out where there is one (an
            // empty line between two blocks is where that block is).
            if let Some(anon) = self.anonymous_inline_root(doc, id) {
                if let Some(pb) = self.laid_out(doc, anon) {
                    let (x, y) = (pb.x + pb.border[3] + pb.padding[3], pb.y + pb.border[0] + pb.padding[0]);
                    return Some(LayoutBox { fixed: pb.fixed, ..LayoutBox::from_rect(Rect::new(x, y, x, y), pb.fixed) });
                }
            }
            let mut cur = doc.get_node(id).and_then(|n| n.parent);
            while let Some(p) = cur {
                if self.in_layout_tree(doc, p) {
                    let pb = self.laid_out(doc, p)?;
                    let (x, y) = (pb.x + pb.border[3] + pb.padding[3], pb.y + pb.border[0] + pb.padding[0]);
                    return Some(LayoutBox {
                        fixed: pb.fixed,
                        ..LayoutBox::from_rect(Rect::new(x, y, x, y), pb.fixed)
                    });
                }
                cur = doc.get_node(p).and_then(|n| n.parent);
            }
        }
        Some(b)
    }

    /// Rows have no Blitz box. Project their actual grid track instead of
    /// unioning cells, since a cell originating here may span later rows.
    fn row_track_box(&self, doc: &BaseDocument, id: NodeId) -> Option<LayoutBox> {
        let table = table_of(doc, id)?;
        self.row_tracks_apply(doc, table)?;
        // Table/ancestor transforms are handled below. Keep the old child
        // projection for independently transformed row parts.
        let mut ancestor = id;
        while ancestor != table {
            let node = doc.get_node(ancestor)?;
            if transformed(node) {
                return None;
            }
            ancestor = node.parent?;
        }
        let element = doc.get_node(table)?.element_data()?;
        let blitz_dom::node::SpecialElementData::TableRoot(context) = &element.special_data else { return None };
        let index = context.rows.iter().position(|row| row.node_id == id)?;
        let info = context.computed_grid_info.try_borrow().ok()?;
        let info = info.as_ref()?;
        // Oversized rowspans can create grid tracks without corresponding DOM
        // rows. In that case a single track is not the row's complete box;
        // preserve the existing cell-union projection instead.
        if info.rows.positions.len() > context.rows.len() {
            return None;
        }
        let row = info.rows.positions.get(index)?;
        let columns = (info.columns.positions.first()?.start as f64, info.columns.positions.last()?.end as f64);
        let (transform, fixed) = self.transform(doc, table)?;
        let base = table_gutter_base(doc, table);
        Some(row_track_rect(doc, (row.start as f64, row.end as f64), columns, id, base, transform, fixed))
    }

    /// The checks of [`Geometry::row_track_box`] that depend on the table
    /// alone (`None`: no row of it is projected from its track).
    fn row_tracks_apply(&self, doc: &BaseDocument, table: NodeId) -> Option<()> {
        if doc.get_node(table)?.primary_styles()?.writing_mode.is_vertical() {
            return None;
        }
        // A table out of the layout tree (in a cell made a block) keeps the context of its last
        // layout, which places no row now.
        if !self.in_layout_tree(doc, table) {
            return None;
        }
        Some(())
    }

    /// [`Geometry::children_union`] of a row group whose rows are all
    /// projected from their tracks ([`Geometry::row_track_box`]), reading
    /// the grid once for all of them instead of once per row (which scans
    /// the table's rows for the row's index: quadratic in the rows). `None`
    /// when a child is not such a row (the generic union applies); the
    /// result is otherwise what the generic union would be.
    fn group_rows_union(&self, doc: &BaseDocument, group: NodeId) -> Option<Option<Rect>> {
        let table = table_of(doc, group)?;
        self.row_tracks_apply(doc, table)?;
        let node = doc.get_node(group)?;
        if transformed(node) || node.parent != Some(table) {
            return None;
        }
        let element = doc.get_node(table)?.element_data()?;
        let blitz_dom::node::SpecialElementData::TableRoot(context) = &element.special_data else { return None };
        let info = context.computed_grid_info.try_borrow().ok()?;
        let info = info.as_ref()?;
        if info.rows.positions.len() > context.rows.len() {
            return None;
        }
        let columns = (info.columns.positions.first()?.start as f64, info.columns.positions.last()?.end as f64);
        let (transform, fixed) = self.transform(doc, table)?;
        let base = table_gutter_base(doc, table);
        let unit = |v: f64| v.signum() * ((v.abs() * 64.0 + 0.01).trunc() / 64.0);
        let mut rows = context.rows.iter().enumerate().filter(|(_, row)| row.group == Some(group)).peekable();
        let mut acc: Option<Rect> = None;
        let mut add = |b: LayoutBox| {
            let (x, y, w, h) = (unit(b.x), unit(b.y), unit(b.width), unit(b.height));
            if w > 0.0 || h > 0.0 {
                let r = Rect::new(x, y, x + w, y + h);
                acc = Some(acc.map_or(r, |a| a.union(r)));
            }
        };
        for &c in &node.children {
            let Some(child) = doc.get_node(c) else { continue };
            if !child.is_element() {
                continue;
            }
            // The group's rows in the grid, in order: a row child the grid
            // placed is the next of them; one it did not (hidden) is read
            // as any other child.
            if tag(doc, c) == Some("tr") && rows.peek().is_some_and(|(_, row)| row.node_id == c) {
                let (index, _) = rows.next()?;
                if transformed(child) {
                    return None;
                }
                let track = info.rows.positions.get(index)?;
                add(row_track_rect(doc, (track.start as f64, track.end as f64), columns, c, base, transform, fixed));
            } else if let Some(b) = self.element_box(doc, c) {
                add(b);
            }
        }
        if rows.next().is_some() {
            return None;
        }
        Some(acc)
    }

    /// Union of the (adjusted) boxes of a table part's children.
    fn children_union(&self, doc: &BaseDocument, id: NodeId) -> Option<Rect> {
        if is_table_part(doc, id) && tag(doc, id) != Some("tr") {
            if let Some(union) = self.group_rows_union(doc, id) {
                return union;
            }
        }
        let node = doc.get_node(id)?;
        let mut acc: Option<Rect> = None;
        for &c in &node.children {
            if !doc.get_node(c).is_some_and(|n| n.is_element()) {
                continue;
            }
            let Some(b) = self.element_box(doc, c) else { continue };
            if b.width > 0.0 || b.height > 0.0 {
                acc = Some(acc.map_or(b.rect(), |a| a.union(b.rect())));
            }
        }
        acc
    }

    /// Union of the fragments of inline element `id` in its inline root's
    /// text layout (see [`Geometry::inline_items`]), each grown by the
    /// element's block-axis border and padding: its border box.
    /// pyro: the anonymous block that lays out inline element `id` (the first, for one split
    /// around blocks), if `id` is not laid out by a DOM ancestor.
    fn anonymous_inline_root(&self, doc: &BaseDocument, id: NodeId) -> Option<NodeId> {
        let mut child = id;
        let mut cur = doc.get_node(id)?.parent;
        loop {
            let n = doc.get_node(cur?)?;
            if n.flags.is_inline_root() {
                return None;
            }
            if let Some(anon) = n.anonymous_blocks.iter().copied().find(|&a| {
                doc.get_node(a).is_some_and(|a| a.flags.is_inline_root() && a.children.contains(&child))
            }) {
                return self.in_layout_tree(doc, anon).then_some(anon);
            }
            child = cur?;
            cur = n.parent;
        }
    }

    fn inline_union(&self, doc: &BaseDocument, id: NodeId) -> Option<Rect> {
        let (top, bottom) = inline_block_growth(doc, id);
        let mut acc: Option<Rect> = None;
        self.inline_items(doc, id, &mut |origin, _, r| {
            let r = origin.transform_rect_bbox(Rect::new(r.x0, r.y0 - top, r.x1, r.y1 + bottom));
            acc = Some(acc.map_or(r, |a| a.union(r)));
        })?;
        acc
    }

    /// Calls `f` with each fragment of inline element `id` in its inline
    /// root's text layout: the transform from the root's text space to the
    /// document, the line index and the fragment in text space (CSS px).
    /// The fragments are the glyph runs (content area: the ascent and
    /// descent of the span's first font, as the lines round them) and atomic
    /// inlines inside the element, and the edges of the spans inside it
    /// (inline-axis margin, border and padding; its own without the margin).
    /// `None` when `id` has no inline root.
    fn inline_items(&self, doc: &BaseDocument, id: NodeId, f: &mut dyn FnMut(Affine, usize, Rect)) -> Option<()> {
        // The nearest ancestor laying the element out inline: a DOM ancestor,
        // or an anonymous block one wraps its inline children in. pyro: an
        // inline box split around blocks (blitz-dom's `NodeFlags::IS_SPLIT_INLINE`)
        // is in two: the anonymous blocks holding its head and its tail.
        let mut child = id;
        let mut cur = doc.get_node(id)?.parent;
        let roots: Vec<NodeId> = loop {
            let r = cur?;
            let n = doc.get_node(r)?;
            if n.flags.is_inline_root() {
                break vec![r];
            }
            let anon: Vec<NodeId> = n
                .anonymous_blocks
                .iter()
                .copied()
                .filter(|&a| doc.get_node(a).is_some_and(|a| a.flags.is_inline_root() && a.children.contains(&child)))
                .collect();
            if !anon.is_empty() {
                break anon;
            }
            child = r;
            cur = n.parent;
        };
        // Line indices go on from one root to the next.
        let mut lines = 0;
        for root_id in roots {
            lines += self.root_inline_items(doc, id, root_id, lines, f).unwrap_or(0);
        }
        Some(())
    }

    /// The fragments of inline element `id` in the inline layout of `root_id` (see
    /// [`Self::inline_items`]), its lines numbered from `first_line`; how many lines it has.
    fn root_inline_items(
        &self,
        doc: &BaseDocument,
        id: NodeId,
        root_id: NodeId,
        first_line: usize,
        f: &mut dyn FnMut(Affine, usize, Rect),
    ) -> Option<usize> {
        let root = doc.get_node(root_id)?;
        let text = root.element_data()?.inline_layout_data.as_ref()?;
        let (t, _) = self.transform(doc, root_id)?;
        let line_origin = line_box_origin(doc, root_id, text);
        let origin = t * line_origin;
        let scale = text.layout.scale() as f64;
        // pyro: lines in a multi-column container's columns.
        let columns = line_column_shift(doc, root_id);
        // Whether `start` is `id` or inside it (below the root).
        let within = |start: Option<NodeId>| {
            let mut cur = start;
            while let Some(n) = cur {
                if n == id {
                    return true;
                }
                if n == root_id {
                    return false;
                }
                cur = doc.get_node(n).and_then(|n| n.parent);
            }
            false
        };
        for (index, line) in text.layout.lines().enumerate() {
            let mut origin = origin * line_slot(doc, root_id, &line.metrics(), scale);
            if let Some(shift) = &columns {
                let top = line_origin * kurbo::Point::new(0.0, line.metrics().block_min_coord as f64 / scale);
                origin = shift(top.y) * origin;
            }
            for item in line.items() {
                let (start, mut r) = inline_item_rect(&line, &item, scale, text.layout.is_rtl());
                if within(start) {
                    if start == Some(id) {
                        r = span_edge_box(doc, &item, r, scale);
                    }
                    f(origin, first_line + index, Rect::new(r.x0 / scale, r.y0 / scale, r.x1 / scale, r.y1 / scale));
                }
            }
        }
        Some(text.layout.len())
    }

    /// `id`'s CSS boxes as quads (CDP `DOM.getBoxModel`/`getContentQuads`),
    /// document coordinates. A box under a transform other than a
    /// translation maps its own border box through the transforms of its
    /// containing-block chain (the points, not their bounding box, as
    /// Chromium's quads); other boxes keep [`Geometry::element_box`]. An
    /// inline box wrapping lines gets one quad per line: its fragments on
    /// that line (its start and end edges among them), grown by its
    /// vertical border and padding.
    pub(crate) fn element_quads(&self, doc: &BaseDocument, id: NodeId) -> Option<BoxQuads> {
        let node = doc.get_node(id)?;
        let style = node.primary_styles();
        // Margins as laid out; an inline box's from its style (its block
        // margins do not apply, as Chromium reports them).
        let side = |v: &style::values::computed::LengthPercentage| {
            v.resolve(style::values::computed::Length::new(0.0)).px() as f64
        };
        if !self.in_layout_tree(doc, id) {
            let style = style?;
            let (b, p, m) = (style.get_border(), style.get_padding(), style.get_margin());
            let border = [
                used_border(&b.border_top_width, b.border_top_style),
                used_border(&b.border_right_width, b.border_right_style),
                used_border(&b.border_bottom_width, b.border_bottom_style),
                used_border(&b.border_left_width, b.border_left_style),
            ];
            let padding =
                [side(&p.padding_top.0), side(&p.padding_right.0), side(&p.padding_bottom.0), side(&p.padding_left.0)];
            let margin_side = |v: &style::values::computed::Margin| match v {
                style::values::generics::length::GenericMargin::LengthPercentage(lp) => side(lp),
                _ => 0.0,
            };
            let margin = [0.0, margin_side(&m.margin_right), 0.0, margin_side(&m.margin_left)];
            let mut lines: Vec<(Affine, Rect)> = Vec::new();
            let mut last_line = usize::MAX;
            self.inline_items(doc, id, &mut |origin, line, r| {
                if line == last_line {
                    if let Some((_, acc)) = lines.last_mut() {
                        *acc = acc.union(r);
                    }
                } else {
                    last_line = line;
                    lines.push((origin, r));
                }
            })?;
            let count = lines.len();
            let fragments: Vec<[f64; 8]> = lines
                .into_iter()
                .map(|(origin, r)| {
                    let r = Rect::new(r.x0, r.y0 - border[0] - padding[0], r.x1, r.y1 + border[2] + padding[2]);
                    quad(origin, r)
                })
                .collect();
            let bounds = fragments.iter().map(quad_bounds).reduce(|a, b| a.union(b))?;
            let fixed = self.transform(doc, id).is_some_and(|(_, fixed)| fixed);
            return Some(BoxQuads {
                border: quad(Affine::IDENTITY, bounds),
                transformed: false,
                border_widths: border,
                padding,
                margin,
                width: bounds.width(),
                height: bounds.height(),
                fragments: if count > 1 { fragments } else { Vec::new() },
                fixed,
            });
        }
        let l = doc.physical_unrounded_layout(id);
        let margin = [l.margin.top, l.margin.right, l.margin.bottom, l.margin.left].map(f64::from);
        let (t, fixed) = self.transform(doc, id)?;
        let [a, b, c, d, _, _] = t.as_coeffs();
        let translation_only = a == 1.0 && b == 0.0 && c == 0.0 && d == 1.0;
        if translation_only || is_cell(doc, id) || is_table_part(doc, id) {
            let e = self.element_box(doc, id)?;
            // pyro: a box split by column breaks has a quad per piece.
            let pieces = self.columns_pieces(doc, id).filter(|p| p.len() > 1);
            return Some(BoxQuads {
                border: quad(Affine::IDENTITY, e.rect()),
                transformed: false,
                border_widths: e.border,
                padding: e.padding,
                margin,
                width: e.width,
                height: e.height,
                fragments: pieces.map_or_else(Vec::new, |p| p.iter().map(|&r| quad(Affine::IDENTITY, r)).collect()),
                fixed: e.fixed,
            });
        }
        let (w, h) = (l.size.width as f64, l.size.height as f64);
        Some(BoxQuads {
            border: quad(t, Rect::new(0.0, 0.0, w, h)),
            transformed: true,
            border_widths: [l.border.top, l.border.right, l.border.bottom, l.border.left].map(f64::from),
            padding: [l.padding.top, l.padding.right, l.padding.bottom, l.padding.left].map(f64::from),
            margin,
            width: w,
            height: h,
            fragments: Vec::new(),
            fixed,
        })
    }
}

/// An element's CSS boxes as quads (see [`Geometry::element_quads`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoxQuads {
    /// Border box: four (x, y) points clockwise from the top-left corner.
    pub border: [f64; 8],
    /// A transform other than a translation applies.
    pub transformed: bool,
    /// Border, padding and margin widths: top, right, bottom, left.
    pub border_widths: [f64; 4],
    pub padding: [f64; 4],
    pub margin: [f64; 4],
    /// Untransformed border box size.
    pub width: f64,
    pub height: f64,
    /// One border-box quad per line of an inline box over several lines, or per piece of a box
    /// split by column breaks.
    pub fragments: Vec<[f64; 8]>,
    pub fixed: bool,
}

/// One item of `line` in its inline root's text space (device px): the
/// node it belongs to (a glyph run's text node or innermost element, an
/// inline box's element, a span edge's span) and its fragment rect.
///
/// `rtl`: the layout's paragraphs are right-to-left (`Layout::is_rtl`), so white space hanging at a
/// line's end is at its left.
pub(crate) fn inline_item_rect(
    line: &parley::Line<'_, blitz_dom::node::TextBrush>,
    item: &parley::PositionedLayoutItem<'_, blitz_dom::node::TextBrush>,
    scale: f64,
    rtl: bool,
) -> (Option<NodeId>, Rect) {
    match item {
        parley::PositionedLayoutItem::GlyphRun(run) => {
            // The content area of the span the run is in: the ascent and
            // descent of its first font in whole CSS pixels, as Chromium's
            // font metrics and Parley's quantized lines round them (18px SF:
            // 17 + 4, not 21.1875), whatever font the run fell back to.
            let (top, bottom) = line.span_content_area(run.style_index());
            let mut x = run.offset() as f64;
            // Chromium's text fragments are their advance ceiled to a layout
            // unit (1/64 px).
            let mut x1 = x + ((run.advance() as f64 / scale * 64.0 - 0.01).ceil() / 64.0) * scale;
            // Collapsible white space hanging at the line's end is in no fragment; preserved
            // and ideographic spaces that hang are content (`LineMetrics::collapsed_advance`).
            let metrics = line.metrics();
            if rtl && metrics.collapsed_advance > 0.0 {
                // pyro: in a right-to-left paragraph it is at the line's left end, at the
                // paragraph's level (UAX #9 L1), whatever the level of the text before it.
                let hung = (metrics.inline_min_coord + metrics.offset + metrics.hanging_advance) as f64
                    - (metrics.hanging_advance - metrics.collapsed_advance) as f64;
                x = x.max(hung).min(x1);
            } else if !run.run().is_rtl() && metrics.collapsed_advance > 0.0 {
                x1 = x1.min((metrics.offset + metrics.advance - metrics.collapsed_advance) as f64).max(x);
            }
            (Some(run.style().brush.id), Rect::new(x, top as f64, x1, bottom as f64))
        }
        parley::PositionedLayoutItem::InlineBox(b) => (
            Some(NodeId::from_u64(b.id)),
            Rect::new(b.x as f64, b.y as f64, (b.x + b.width) as f64, (b.y + b.height) as f64),
        ),
    }
}

/// The part of `r`, the rect of line item `item` (device px), that is in the
/// border box of the span the item belongs to: an edge of the span without
/// its margin; any other item whole.
pub(crate) fn span_edge_box(
    doc: &BaseDocument,
    item: &parley::PositionedLayoutItem<'_, blitz_dom::node::TextBrush>,
    r: Rect,
    scale: f64,
) -> Rect {
    use style::values::generics::length::GenericMargin;
    let parley::PositionedLayoutItem::InlineBox(b) = item else { return r };
    if !b.kind.is_span_edge() {
        return r;
    }
    let Some(style) = doc.get_node(NodeId::from_u64(b.id)).and_then(|n| n.primary_styles()) else { return r };
    let margin = |v: &style::values::computed::Margin| match v {
        GenericMargin::LengthPercentage(v) => v.resolve(style::values::computed::Length::new(0.0)).px() as f64 * scale,
        _ => 0.0,
    };
    let rtl = style.clone_direction() == style::computed_values::direction::T::Rtl;
    if (b.kind == parley::InlineBoxKind::SpanStart) != rtl {
        Rect::new(r.x0 + margin(&style.get_margin().margin_left), r.y0, r.x1, r.y1)
    } else {
        Rect::new(r.x0, r.y0, r.x1 - margin(&style.get_margin().margin_right), r.y1)
    }
}

/// The block-axis border and padding (CSS px) an inline element's fragments
/// grow by to its border box: before and after them.
pub(crate) fn inline_block_growth(doc: &BaseDocument, id: NodeId) -> (f64, f64) {
    let Some(style) = doc.get_node(id).and_then(|n| n.primary_styles()) else { return (0.0, 0.0) };
    let side = |v: &style::values::computed::LengthPercentage| {
        v.resolve(style::values::computed::Length::new(0.0)).px() as f64
    };
    let (b, p) = (style.get_border(), style.get_padding());
    (
        used_border(&b.border_top_width, b.border_top_style) + side(&p.padding_top.0),
        used_border(&b.border_bottom_width, b.border_bottom_style) + side(&p.padding_bottom.0),
    )
}

/// A border side's used width in CSS px: none where the side has no style.
fn used_border(width: &style::values::computed::BorderSideWidth, style: style::values::computed::BorderStyle) -> f64 {
    if style.none_or_hidden() { 0.0 } else { width.0.to_f64_px() }
}

/// The inline root's line coordinates (x inline, y block, in CSS px from the first line box) in
/// its border box. A vertical writing mode turns them a quarter clockwise; lines of `vertical-lr`
/// are then each rotated within their slot (see [`line_slot`]).
pub(crate) fn line_box_origin(doc: &BaseDocument, id: NodeId, text: &blitz_dom::node::TextLayout) -> Affine {
    let l = doc.physical_unrounded_layout(id);
    let (left, top) = ((l.border.left + l.padding.left) as f64, (l.border.top + l.padding.top) as f64);
    let block_offset = text.block_offset as f64;
    match doc.get_node(id).and_then(|n| n.vertical_block_axis()) {
        None => Affine::translate((left, top + block_offset)),
        // `sideways-lr`: a quarter counterclockwise, up from the content box's bottom.
        Some(true) if sideways_lr(doc, id) => {
            let bottom = (l.size.height - l.border.bottom - l.padding.bottom) as f64;
            Affine::new([0.0, -1.0, 1.0, 0.0, left + block_offset, bottom])
        }
        Some(true) => Affine::new([0.0, 1.0, -1.0, 0.0, left + block_offset, top]),
        Some(false) => {
            let right = (l.size.width - l.border.right - l.padding.right) as f64;
            Affine::new([0.0, 1.0, -1.0, 0.0, right - block_offset, top])
        }
    }
}

/// What moves a line (CSS px) to its slot in [`line_box_origin`]'s coordinates.
pub(crate) fn line_slot(doc: &BaseDocument, id: NodeId, metrics: &parley::layout::LineMetrics, scale: f64) -> Affine {
    match doc.get_node(id).and_then(|n| n.vertical_block_axis()) {
        Some(true) if !sideways_lr(doc, id) => {
            Affine::translate((0.0, -((metrics.block_min_coord + metrics.block_max_coord) as f64) / scale))
        }
        _ => Affine::IDENTITY,
    }
}

/// Whether `id`'s lines are `sideways-lr`'s: turned counterclockwise, their line-over side on the left
/// (blitz-paint's `draw_vertical_inline_layout`).
fn sideways_lr(doc: &BaseDocument, id: NodeId) -> bool {
    doc.get_node(id)
        .and_then(|n| n.primary_styles())
        .is_some_and(|s| s.writing_mode.contains(style::logical_geometry::WritingMode::WRITING_MODE_SIDEWAYS_LR))
}

/// `r`'s corners through `t`, clockwise from the top-left one.
fn quad(t: Affine, r: Rect) -> [f64; 8] {
    let p = [(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)].map(|(x, y)| t * kurbo::Point::new(x, y));
    [p[0].x, p[0].y, p[1].x, p[1].y, p[2].x, p[2].y, p[3].x, p[3].y]
}

fn quad_bounds(q: &[f64; 8]) -> Rect {
    let xs = [q[0], q[2], q[4], q[6]];
    let ys = [q[1], q[3], q[5], q[7]];
    let min = |v: [f64; 4]| v.into_iter().fold(f64::INFINITY, f64::min);
    let max = |v: [f64; 4]| v.into_iter().fold(f64::NEG_INFINITY, f64::max);
    Rect::new(min(xs), min(ys), max(xs), max(ys))
}

/// One word of a text node on a line (or the part of it in one glyph run):
/// its content area in document CSS px (viewport ones inside a fixed box).
#[derive(Clone, Debug, PartialEq)]
pub struct TextWord {
    pub node: NodeId,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub fixed: bool,
    /// Caret positions along the word: a UTF-16 offset into the node's data
    /// and its x, from the word's start to its end.
    pub edges: Vec<(u32, f64)>,
}

impl Geometry {
    /// Every word laid out in the document's inline formatting contexts, in
    /// layout order.
    pub(crate) fn text_words(&self, doc: &BaseDocument) -> Vec<TextWord> {
        let mut out = Vec::new();
        for (id, _) in doc.tree().iter() {
            self.root_words(doc, id, &mut out);
        }
        out
    }

    /// The words [`Geometry::text_words`] reports for the text nodes in
    /// `within`'s subtree, in the same order, with others: the words of the
    /// inline formatting contexts inside it (in the flat tree pyro-dom mirrors,
    /// where a slotted node lies below its host) and of the one it lies in.
    /// Only those contexts are read, not the document.
    pub(crate) fn subtree_words(&self, doc: &BaseDocument, within: NodeId) -> Vec<TextWord> {
        let mut roots = Vec::new();
        // The context `within` lies in when it is inline content (or no box,
        // `display: contents`): the nearest ancestor that is an inline root,
        // or that wraps inline content in anonymous blocks (all of them).
        let boxless = doc
            .get_node(within)
            .and_then(|n| n.primary_styles())
            .is_some_and(|s| s.clone_display().is_inline_flow() || s.clone_display().is_contents());
        let mut cur = doc.get_node(within).and_then(|n| n.parent).filter(|_| boxless);
        while let Some(r) = cur {
            let Some(n) = doc.get_node(r) else { break };
            if n.flags.is_inline_root() {
                roots.push(r);
                break;
            }
            if !n.anonymous_blocks.is_empty() {
                roots.extend(n.anonymous_blocks.iter().copied());
                break;
            }
            cur = n.parent;
        }
        // The contexts inside it: inline roots, and the anonymous blocks
        // wrapping inline children.
        let mut stack = vec![within];
        while let Some(id) = stack.pop() {
            let Some(node) = doc.get_node(id) else { continue };
            if !node.is_element() {
                continue;
            }
            if node.flags.is_inline_root() {
                roots.push(id);
            }
            roots.extend(node.anonymous_blocks.iter().copied());
            stack.extend(node.children.iter().copied());
        }
        // `text_words`' order: the slab's.
        roots.sort_unstable_by_key(|id| id.as_u64() as u32);
        roots.dedup();
        let mut out = Vec::new();
        for id in roots {
            self.root_words(doc, id, &mut out);
        }
        out
    }

    /// The words of inline formatting context `id` (nothing when it is not
    /// one laid out now), appended to `out`.
    fn root_words(&self, doc: &BaseDocument, id: NodeId, out: &mut Vec<TextWord>) {
        let Some(node) = doc.get_node(id) else { return };
        {
            let Some(text) = node.element_data().and_then(|e| e.inline_layout_data.as_ref()) else { return };
            if !node.flags.is_inline_root() || text.text_nodes.is_empty() || !self.in_layout_tree(doc, id) {
                return;
            }
            let Some((t, fixed)) = self.transform(doc, id) else { return };
            let origin = t * line_box_origin(doc, id, text);
            let scale = text.layout.scale() as f64;
            // Per text node: the UTF-16 offset of each byte of its laid-out text.
            let offsets: Vec<Vec<u32>> = text
                .text_nodes
                .iter()
                .map(|(n, range)| {
                    let source = doc.get_node(*n).and_then(|n| n.text_data()).map_or("", |d| d.content.as_str());
                    utf16_offsets(source, &text.text[range.clone()])
                })
                .collect();
            let owner = |byte: usize| {
                let i = text.text_nodes.partition_point(|(_, r)| r.end <= byte);
                text.text_nodes.get(i).filter(|(_, r)| r.start <= byte).map(|_| i)
            };
            for line in text.layout.lines() {
                let m = line.metrics();
                let origin = origin * line_slot(doc, id, m, scale);
                let (line_top, line_height) =
                    (m.block_min_coord as f64 / scale, (m.block_max_coord - m.block_min_coord) as f64 / scale);
                for item in line.items() {
                    let parley::PositionedLayoutItem::GlyphRun(run) = item else { continue };
                    let metrics = run.run().font_metrics();
                    // Chromium's text box: whole-pixel ascent and descent,
                    // the half-leading floored (CSS px, back to layout units).
                    let (a, d) = (metrics.ascent as f64 / scale, metrics.descent as f64 / scale);
                    let leading = ((line_height - a.round() - d.round()) * 64.0).round();
                    let top = line_top + ((leading / 2.0).trunc() / 64.0).floor();
                    let (top, bottom) = (top * scale, (top + a.round() + d.round()) * scale);
                    let mut x = run.offset() as f64;
                    let mut word: Option<(usize, Vec<(u32, f64)>)> = None;
                    let mut flush = |word: &mut Option<(usize, Vec<(u32, f64)>)>| {
                        let Some((i, edges)) = word.take() else { return };
                        let (x0, x1) = (edges[0].1, edges[edges.len() - 1].1);
                        let r =
                            origin.transform_rect_bbox(Rect::new(x0 / scale, top / scale, x1 / scale, bottom / scale));
                        let edges = edges
                            .into_iter()
                            .map(|(o, ex)| {
                                (o, origin.transform_rect_bbox(Rect::new(ex / scale, 0.0, ex / scale, 0.0)).x0)
                            })
                            .collect();
                        out.push(TextWord {
                            node: text.text_nodes[i].0,
                            x: r.x0,
                            y: r.y0,
                            // Chromium rounds a text fragment's width up to its layout unit.
                            width: (r.width() * 64.0 - 0.01).ceil() / 64.0,
                            height: r.height(),
                            fixed,
                            edges,
                        });
                    };
                    for cluster in run.run().visual_clusters() {
                        let advance = cluster.advance() as f64;
                        let range = cluster.text_range();
                        let at = owner(range.start);
                        let blank = cluster.is_space_or_nbsp() || cluster.is_hard_line_break() || at.is_none();
                        if blank || word.as_ref().is_some_and(|(i, _)| Some(*i) != at) {
                            flush(&mut word);
                        }
                        if let (false, Some(i)) = (blank, at) {
                            let base = text.text_nodes[i].1.start;
                            let map = &offsets[i];
                            let offset = |b: usize| map[(b - base).min(map.len() - 1)];
                            let (start, end) =
                                if cluster.is_rtl() { (range.end, range.start) } else { (range.start, range.end) };
                            let w = word.get_or_insert_with(|| (i, vec![(offset(start), x)]));
                            w.1.push((offset(end), x + advance));
                        }
                        x += advance;
                    }
                    flush(&mut word);
                }
            }
        }
    }
}

/// The UTF-16 offset into `source` (a text node's data) of each byte of
/// `laid` (what it became in the layout: white space collapsed, case
/// transformed), plus one past the end.
fn utf16_offsets(source: &str, laid: &str) -> Vec<u32> {
    let mut map = vec![0u32; laid.len() + 1];
    let mut src = source.chars().peekable();
    let mut at = 0u32;
    let same = |s: char, l: char| s == l || (s.is_whitespace() && l == ' ') || s.to_lowercase().eq(l.to_lowercase());
    for (b, c) in laid.char_indices() {
        // Source white space the layout collapsed away.
        while let Some(&s) = src.peek() {
            if same(s, c) || !s.is_whitespace() {
                break;
            }
            at += s.len_utf16() as u32;
            src.next();
        }
        map[b..b + c.len_utf8()].fill(at);
        if src.peek().is_some_and(|&s| same(s, c)) {
            at += src.next().unwrap().len_utf16() as u32;
        }
    }
    map[laid.len()] = at;
    map
}

/// Whether `id` is a box of the current layout: the root element, or a
/// layout child of its layout parent (by scanning the parent's lists;
/// [`Geometry::in_layout_tree`] reads the list owner Blitz records instead).
fn in_layout_tree(doc: &BaseDocument, id: NodeId) -> bool {
    if doc.try_root_element().is_some_and(|root| root.id == id) {
        return true;
    }
    let Some(parent) = doc.get_node(id).and_then(|n| n.layout_parent.get()) else { return false };
    doc.get_node(parent).is_some_and(|p| {
        p.layout_children.borrow().as_ref().is_some_and(|c| c.contains(&id))
            || p.hoisted_children.borrow().contains(&id)
    })
}

/// `id` and its ancestors as Blitz styled and laid them out (diagnostics).
pub(crate) fn describe(doc: &BaseDocument, id: NodeId) -> String {
    let mut out = Vec::new();
    let mut cur = Some(id);
    while let Some(i) = cur {
        let Some(n) = doc.get_node(i) else { break };
        if let Some(e) = n.element_data() {
            let class: String = e.attr(blitz_dom::local_name!("class")).unwrap_or("").chars().take(300).collect();
            let s = n.primary_styles();
            let display = s.as_ref().map(|s| format!("{:?}", s.clone_display())).unwrap_or_default();
            let height = s.as_ref().map(|s| format!("{:?}", s.slow_clone_height())).unwrap_or_default();
            let l = n.unrounded_layout();
            let text: String = n.text_content().chars().take(40).collect();
            let kids: Vec<String> = n
                .children
                .iter()
                .filter_map(|&c| doc.get_node(c))
                .map(|c| match c.element_data() {
                    Some(ce) => format!(
                        "<{} {:?} {:?}>",
                        ce.name.local,
                        ce.attr(blitz_dom::local_name!("class")).unwrap_or(""),
                        c.primary_styles().map(|s| format!("{:?}", s.clone_display())).unwrap_or_default()
                    ),
                    None => format!("text {:?}", c.text_content()),
                })
                .collect();
            out.push(format!(
                "<{} class={class:?}> display={display} height={height} laid={:.1}x{:.1} in_tree={} children={} kids={kids:?} text={text:?} padding={:?} border={:?}",
                e.name.local,
                l.size.width,
                l.size.height,
                in_layout_tree(doc, i),
                n.children.len(),
                l.padding,
                l.border,
            ));
        }
        if out.len() >= 8 {
            break;
        }
        cur = n.parent;
    }
    out.join("\n  ")
}

fn tag(doc: &BaseDocument, id: NodeId) -> Option<&str> {
    doc.get_node(id).and_then(|n| n.element_data()).map(|e| &*e.name.local)
}

pub(crate) fn is_cell(doc: &BaseDocument, id: NodeId) -> bool {
    matches!(tag(doc, id), Some("td" | "th"))
}

pub(crate) fn is_table_part(doc: &BaseDocument, id: NodeId) -> bool {
    matches!(tag(doc, id), Some("tr" | "thead" | "tbody" | "tfoot"))
}

/// The nearest `<table>` ancestor of `id`.
fn table_of(doc: &BaseDocument, id: NodeId) -> Option<NodeId> {
    let mut table = doc.get_node(id)?.parent?;
    while tag(doc, table) != Some("table") {
        table = doc.get_node(table)?.parent?;
    }
    Some(table)
}

/// Whether `node` has a transform other than the identity.
fn transformed(node: &blitz_dom::Node) -> bool {
    node.transform().as_deref().is_some_and(|transform| *transform != Affine::IDENTITY)
}

/// The box of row `id` from its grid track `(start, end)` across the
/// table's columns `(left, right)` (see [`Geometry::row_track_box`]), grown
/// by the collapsed-border gutters (`base`, see [`table_gutter_base`]) and
/// placed by the table's `transform`.
fn row_track_rect(
    doc: &BaseDocument,
    track: (f64, f64),
    columns: (f64, f64),
    id: NodeId,
    base: Option<(f64, f64)>,
    transform: Affine,
    fixed: bool,
) -> LayoutBox {
    let (gx, top, bottom) = base.map(|b| row_gutters(doc, Some(id), b)).unwrap_or((0.0, 0.0, 0.0));
    let local = Rect::new(columns.0 - gx, track.0 - top, columns.1 + gx, track.1 + bottom);
    LayoutBox::from_rect(transform.transform_rect_bbox(local), fixed)
}

fn side(w: app_units::Au, s: style::values::computed::BorderStyle) -> f64 {
    if s.none_or_hidden() {
        0.0
    } else {
        w.to_f64_px()
    }
}

/// Half the collapsed borders around cell `id` (horizontal, top, bottom):
/// Blitz's gutter (the table's first cell's widest borders) and the row's own.
/// `None` outside collapsed-border tables.
pub(crate) fn cell_gutters(doc: &BaseDocument, id: NodeId) -> Option<(f64, f64, f64)> {
    let table = table_of(doc, id)?;
    table_gutters(doc, table, doc.get_node(id)?.parent)
}

fn table_gutters(doc: &BaseDocument, table: NodeId, row: Option<NodeId>) -> Option<(f64, f64, f64)> {
    Some(row_gutters(doc, row, table_gutter_base(doc, table)?))
}

/// The part of [`table_gutters`] that depends on the table alone: half its
/// first cell's widest horizontal and vertical borders (`None` outside
/// collapsed-border tables with uniform borders).
pub(crate) fn table_gutter_base(doc: &BaseDocument, table: NodeId) -> Option<(f64, f64)> {
    let styles = doc.get_node(table)?.primary_styles()?;
    if styles.clone_border_collapse() != style::computed_values::border_collapse::T::Collapse {
        return None;
    }
    // Without uniform borders the cell boxes already hold them.
    if matches!(
        doc.get_node(table)?.element_data()?.special_data,
        blitz_dom::node::SpecialElementData::TableRoot(ref context) if context.collapsed_edges.is_some()
    ) {
        return None;
    }
    let cell = doc.get_node(first_cell(doc, table)?)?.primary_styles()?;
    let b = cell.get_border();
    let gx =
        side(b.border_left_width.0, b.border_left_style).max(side(b.border_right_width.0, b.border_right_style)) / 2.0;
    let gy =
        side(b.border_top_width.0, b.border_top_style).max(side(b.border_bottom_width.0, b.border_bottom_style)) / 2.0;
    Some((gx, gy))
}

/// [`table_gutters`] of the cells of `row` from the table's `(gx, gy)`
/// ([`table_gutter_base`]): the row's own borders widen the vertical ones.
pub(crate) fn row_gutters(doc: &BaseDocument, row: Option<NodeId>, (gx, gy): (f64, f64)) -> (f64, f64, f64) {
    let (mut top, mut bottom) = (gy, gy);
    if let Some(rs) = row.and_then(|p| doc.get_node(p)).and_then(|r| r.primary_styles()) {
        let b = rs.get_border();
        top = top.max(side(b.border_top_width.0, b.border_top_style) / 2.0);
        bottom = bottom.max(side(b.border_bottom_width.0, b.border_bottom_style) / 2.0);
    }
    (gx, top, bottom)
}

fn first_cell(doc: &BaseDocument, id: NodeId) -> Option<NodeId> {
    for &c in &doc.get_node(id)?.children {
        if is_cell(doc, c) {
            return Some(c);
        }
        if tag(doc, c) != Some("table") {
            if let Some(f) = first_cell(doc, c) {
                return Some(f);
            }
        }
    }
    None
}

/// pyro (lane H2): whether layout queries place a plane by its homography (`Plane3D::local`):
/// paint draws it through a perspective, or it is in a context element placed so.
pub(crate) fn is_projective_plane(plane: &blitz_dom::node::Plane3D) -> bool {
    plane.places_by_local
}

/// pyro (lane H2): a homography of the plane, row-major: a point (x, y) maps to (X / W, Y / W)
/// with `(X, Y, W) = H (x, y, 1)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Homography(pub(crate) [f64; 9]);

impl Homography {
    pub(crate) const IDENTITY: Homography = Homography([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);

    fn mul(&self, next: &Homography) -> Homography {
        let (a, b) = (&self.0, &next.0);
        Homography(std::array::from_fn(|i| {
            let (row, col) = (i / 3, i % 3);
            (0..3).map(|k| a[row * 3 + k] * b[k * 3 + col]).sum()
        }))
    }

    /// `self` with `t` applied first.
    pub(crate) fn then_affine_before(&self, t: Affine) -> Homography {
        let [a, b, c, d, e, f] = t.as_coeffs();
        self.mul(&Homography([a, c, e, b, d, f, 0.0, 0.0, 1.0]))
    }

    /// The bounds of `rect`'s projection, its part behind the viewer cut away; `None` when nothing
    /// of it is in front.
    pub(crate) fn bbox(&self, rect: Rect) -> Option<Rect> {
        const MIN_W: f64 = 1e-6;
        let m = &self.0;
        let apply =
            |x: f64, y: f64| [m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5], m[6] * x + m[7] * y + m[8]];
        let corners =
            [(rect.x0, rect.y0), (rect.x1, rect.y0), (rect.x1, rect.y1), (rect.x0, rect.y1)].map(|(x, y)| apply(x, y));
        let mut bounds: Option<Rect> = None;
        let mut add = |x: f64, y: f64| {
            let p = Rect::new(x, y, x, y);
            bounds = Some(bounds.map_or(p, |b| b.union(p)));
        };
        for i in 0..4 {
            let (a, b) = (corners[i], corners[(i + 1) % 4]);
            if a[2] >= MIN_W {
                add(a[0] / a[2], a[1] / a[2]);
            }
            if (a[2] >= MIN_W) != (b[2] >= MIN_W) {
                let t = (MIN_W - a[2]) / (b[2] - a[2]);
                add((a[0] + (b[0] - a[0]) * t) / MIN_W, (a[1] + (b[1] - a[1]) * t) / MIN_W);
            }
        }
        // A plane reaching towards the horizon is bounded where layout can hold it.
        const LIMIT: f64 = 1e7;
        bounds.map(|b| Rect::new(b.x0.max(-LIMIT), b.y0.max(-LIMIT), b.x1.min(LIMIT), b.y1.min(LIMIT)))
    }
}

/// pyro: whether a line clamp discarded `id` or a box it is laid out in (blitz-dom
/// `NodeFlags::HIDDEN_BY_CLAMP`).
pub(crate) fn hidden_by_clamp(doc: &blitz_dom::BaseDocument, id: blitz_dom::NodeId) -> bool {
    let mut current = doc.get_node(id);
    while let Some(node) = current {
        if node.flags.contains(blitz_dom::node::NodeFlags::HIDDEN_BY_CLAMP) {
            return true;
        }
        current = node.layout_parent.get().and_then(|parent| doc.get_node(parent));
    }
    false
}
