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
//! Boxes are computed per element on request; transforms along the
//! containing-block chains are memoized until the document changes.

use std::cell::RefCell;

use blitz_dom::{BaseDocument, NodeId};
use kurbo::{Affine, Rect};
use rustc_hash::{FxHashMap, FxHashSet};

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
    /// Inside (or itself) a `position: fixed` box.
    pub fixed: bool,
    /// Border and padding widths: top, right, bottom, left.
    pub border: [f64; 4],
    pub padding: [f64; 4],
    pub position: BoxPosition,
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

/// Per-document memo, valid until the document changes ([`Geometry::clear`]).
#[derive(Default)]
pub(crate) struct Geometry {
    transforms: RefCell<FxHashMap<NodeId, (Affine, bool)>>,
    /// The layout (and hoisted) children of each layout parent with many,
    /// indexed on first use: [`Geometry::in_layout_tree`] is O(1) where a
    /// scan of the parent's list made reading every box quadratic in the
    /// width of the tree.
    layout_children: RefCell<FxHashMap<NodeId, FxHashSet<NodeId>>>,
}

/// Whether any node of `doc` is `position: sticky`: Blitz's `NodeTree` counts
/// them as primary styles are written and nodes dropped. Debug builds check the
/// count against a scan of every node.
pub(crate) fn has_sticky(doc: &BaseDocument) -> bool {
    let tree = doc.tree();
    debug_assert_eq!(
        tree.sticky_count(),
        tree.iter()
            .filter(|(_, n)| {
                n.primary_styles().is_some_and(|s| s.clone_position() == style::computed_values::position::T::Sticky)
            })
            .count(),
        "NodeTree::sticky_count disagrees with a scan",
    );
    tree.has_sticky()
}

/// Layout parents with at most this many children are scanned, not indexed.
const SCAN_CHILDREN: usize = 16;

impl Geometry {
    pub(crate) fn clear(&self) {
        self.transforms.borrow_mut().clear();
        self.layout_children.borrow_mut().clear();
    }

    /// Whether `id` is a box of the current layout: the root element, or a
    /// layout child of its layout parent.
    fn in_layout_tree(&self, doc: &BaseDocument, id: NodeId) -> bool {
        if doc.root_element().id == id {
            return true;
        }
        let Some(parent) = doc.get_node(id).and_then(|n| n.layout_parent.get()) else { return false };
        let Some(p) = doc.get_node(parent) else { return false };
        let layout_children = p.layout_children.borrow();
        let hoisted = p.hoisted_children.borrow();
        let count = layout_children.as_ref().map_or(0, |c| c.len()) + hoisted.len();
        if count <= SCAN_CHILDREN {
            return layout_children.as_ref().is_some_and(|c| c.contains(&id)) || hoisted.contains(&id);
        }
        let mut index = self.layout_children.borrow_mut();
        let set = index
            .entry(parent)
            .or_insert_with(|| layout_children.iter().flatten().chain(hoisted.iter()).copied().collect());
        let found = set.contains(&id);
        debug_assert_eq!(found, in_layout_tree(doc, id), "layout child index of {parent}");
        found
    }

    /// `id`'s border box to the document and whether it is in a fixed box.
    fn transform(&self, doc: &BaseDocument, id: NodeId) -> Option<(Affine, bool)> {
        if let Some(t) = self.transforms.borrow().get(&id) {
            return Some(*t);
        }
        let node = doc.get_node(id)?;
        let (base, in_fixed) = match node.containing_block() {
            Some(parent) => {
                let (p, fixed) = self.transform(doc, parent)?;
                let scroll = doc.get_node(parent)?.scroll_offset();
                (p * Affine::translate((-scroll.x, -scroll.y)), fixed)
            }
            None => (Affine::IDENTITY, false),
        };
        let fixed = in_fixed
            || node.primary_styles().is_some_and(|s| s.clone_position() == style::computed_values::position::T::Fixed);
        let location = doc.physical_unrounded_layout(id).location;
        // Blitz keeps the CSS transform's translation in device px
        // (`Node::set_transform`); this chain is in CSS px.
        let own = node.transform().as_deref().copied().map_or(Affine::IDENTITY, |t| {
            let scale = doc.viewport().scale_f64();
            let [a, b, c, d, e, f] = t.as_coeffs();
            Affine::new([a, b, c, d, e / scale, f / scale])
        });
        let t = base * Affine::translate((location.x as f64, location.y as f64)) * own;
        self.transforms.borrow_mut().insert(id, (t, fixed));
        Some((t, fixed))
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
        let (t, fixed) = self.transform(doc, id)?;
        let l = doc.physical_unrounded_layout(id);
        let rect = t.transform_rect_bbox(Rect::new(0.0, 0.0, l.size.width as f64, l.size.height as f64));
        let client_width = (l.size.width - l.border.left - l.border.right - l.scrollbar_size.width).max(0.0) as f64;
        let client_height = (l.size.height - l.border.top - l.border.bottom - l.scrollbar_size.height).max(0.0) as f64;
        // Scrollable overflow is kept in device px.
        let o = node.scrollable_overflow().scale_from_origin(1.0 / doc.viewport().scale_f64());
        Some(LayoutBox {
            x: rect.x0,
            y: rect.y0,
            width: rect.width(),
            height: rect.height(),
            client_width,
            client_height,
            scroll_width: client_width.max(o.x1 - l.border.left as f64),
            scroll_height: client_height.max(o.y1 - l.border.top as f64),
            fixed,
            border: [l.border.top, l.border.right, l.border.bottom, l.border.left].map(f64::from),
            padding: [l.padding.top, l.padding.right, l.padding.bottom, l.padding.left].map(f64::from),
            position: BoxPosition::Static,
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
            fixed: b.fixed,
            border: b.border.map(unit),
            padding: b.padding.map(unit),
            position: box_position(doc.get_node(id)?),
        })
    }

    /// The sticky inputs of `id` (see [`StickyBox`]).
    pub(crate) fn sticky_box(&self, doc: &BaseDocument, id: NodeId) -> StickyBox {
        let Some(style) = doc.get_node(id).and_then(|n| n.primary_styles()) else { return StickyBox::default() };
        let inset = |v: &style::values::computed::Inset| {
            let style::values::generics::position::GenericInset::LengthPercentage(lp) = v else { return None };
            let px = lp.resolve(style::values::computed::Length::new(0.0)).px() as f64;
            let hundred = lp.resolve(style::values::computed::Length::new(100.0)).px() as f64;
            Some((px, (hundred - px) / 100.0))
        };
        let insets = [
            inset(&style.clone_top()),
            inset(&style.clone_right()),
            inset(&style.clone_bottom()),
            inset(&style.clone_left()),
        ];
        // Containing block: the table for table parts, else the parent.
        let mut cb = doc.get_node(id).and_then(|n| n.parent);
        if is_table_part(doc, id) || is_cell(doc, id) {
            while let Some(c) = cb.filter(|&c| tag(doc, c) != Some("table")) {
                cb = doc.get_node(c).and_then(|n| n.parent);
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
            Some((b.x + l, b.y + t, (width - l - r).max(0.0), (height - t - bo).max(0.0)))
        });
        StickyBox { insets, limit }
    }

    fn adjusted_box(&self, doc: &BaseDocument, id: NodeId) -> Option<LayoutBox> {
        let mut b = self.laid_out(doc, id)?;
        if is_cell(doc, id) {
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
            // zero box at the content-box origin of the nearest box around it.
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
        let mut table = doc.get_node(id)?.parent?;
        while tag(doc, table) != Some("table") {
            table = doc.get_node(table)?.parent?;
        }
        if doc.get_node(table)?.primary_styles()?.writing_mode.is_vertical() {
            return None;
        }
        // Table/ancestor transforms are handled below. Keep the old child
        // projection for independently transformed row parts.
        let mut ancestor = id;
        while ancestor != table {
            let node = doc.get_node(ancestor)?;
            if node.transform().as_deref().is_some_and(|transform| *transform != Affine::IDENTITY) {
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
        let left = info.columns.positions.first()?.start as f64;
        let right = info.columns.positions.last()?.end as f64;
        let (gx, top, bottom) = table_gutters(doc, table, Some(id)).unwrap_or((0.0, 0.0, 0.0));
        let local = Rect::new(left - gx, row.start as f64 - top, right + gx, row.end as f64 + bottom);
        let (transform, fixed) = self.transform(doc, table)?;
        Some(LayoutBox::from_rect(transform.transform_rect_bbox(local), fixed))
    }

    /// Union of the (adjusted) boxes of a table part's children.
    fn children_union(&self, doc: &BaseDocument, id: NodeId) -> Option<Rect> {
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
    /// text layout: glyph runs (content area: rounded ascent to descent) and atomic
    /// inlines inside it.
    fn inline_union(&self, doc: &BaseDocument, id: NodeId) -> Option<Rect> {
        let mut acc: Option<Rect> = None;
        self.inline_items(doc, id, &mut |origin, _, r| {
            let r = origin.transform_rect_bbox(r);
            acc = Some(acc.map_or(r, |a| a.union(r)));
        })?;
        acc
    }

    /// Calls `f` with each fragment of inline element `id` in its inline
    /// root's text layout (see [`Geometry::inline_union`]): the transform
    /// from the root's text space to the document, the line index and the
    /// fragment in text space (CSS px). `None` when `id` has no inline root.
    fn inline_items(&self, doc: &BaseDocument, id: NodeId, f: &mut dyn FnMut(Affine, usize, Rect)) -> Option<()> {
        // The nearest ancestor laying the element out inline: a DOM ancestor,
        // or an anonymous block one wraps its inline children in.
        let mut child = id;
        let mut cur = doc.get_node(id)?.parent;
        let root_id = loop {
            let r = cur?;
            let n = doc.get_node(r)?;
            if n.flags.is_inline_root() {
                break r;
            }
            let anon =
                n.anonymous_blocks.iter().copied().find(|&a| {
                    doc.get_node(a).is_some_and(|a| a.flags.is_inline_root() && a.children.contains(&child))
                });
            if let Some(a) = anon {
                break a;
            }
            child = r;
            cur = n.parent;
        };
        let root = doc.get_node(root_id)?;
        let text = root.element_data()?.inline_layout_data.as_ref()?;
        let (t, _) = self.transform(doc, root_id)?;
        let origin = t * line_box_origin(doc, root_id, text);
        let scale = text.layout.scale() as f64;
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
            let origin = origin * line_slot(doc, root_id, &line.metrics(), scale);
            for item in line.items() {
                let (start, r) = match item {
                    parley::PositionedLayoutItem::GlyphRun(run) => {
                        // The content area from whole-pixel ascent and descent, as
                        // Chromium's font metrics and Parley's quantized lines round them
                        // (18px SF: 17 + 4, not 21.1875).
                        let metrics = run.run().font_metrics();
                        let (x, baseline) = (run.offset() as f64, run.baseline() as f64);
                        (
                            Some(run.style().brush.id),
                            Rect::new(
                                x,
                                baseline - metrics.ascent.round() as f64,
                                // Chromium's text fragments are their advance ceiled to
                                // a layout unit (1/64 px).
                                x + ((run.advance() as f64 / scale * 64.0 - 0.01).ceil() / 64.0) * scale,
                                baseline + metrics.descent.round() as f64,
                            ),
                        )
                    }
                    parley::PositionedLayoutItem::InlineBox(b) => (
                        Some(NodeId::from_u64(b.id)),
                        Rect::new(b.x as f64, b.y as f64, (b.x + b.width) as f64, (b.y + b.height) as f64),
                    ),
                };
                if within(start) {
                    f(origin, index, Rect::new(r.x0 / scale, r.y0 / scale, r.x1 / scale, r.y1 / scale));
                }
            }
        }
        Some(())
    }

    /// `id`'s CSS boxes as quads (CDP `DOM.getBoxModel`/`getContentQuads`),
    /// document coordinates. A box under a transform other than a
    /// translation maps its own border box through the transforms of its
    /// containing-block chain (the points, not their bounding box, as
    /// Chromium's quads); other boxes keep [`Geometry::element_box`]. An
    /// inline box wrapping lines gets one quad per line: its fragments on
    /// that line, grown by its vertical border and padding, and by its
    /// start (end) border and padding on its first (last) line.
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
                b.border_top_width.0.to_f64_px(),
                b.border_right_width.0.to_f64_px(),
                b.border_bottom_width.0.to_f64_px(),
                b.border_left_width.0.to_f64_px(),
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
                .enumerate()
                .map(|(i, (origin, r))| {
                    let left = if i == 0 { border[3] + padding[3] } else { 0.0 };
                    let right = if i + 1 == count { border[1] + padding[1] } else { 0.0 };
                    let r = Rect::new(
                        r.x0 - left,
                        r.y0 - border[0] - padding[0],
                        r.x1 + right,
                        r.y1 + border[2] + padding[2],
                    );
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
            return Some(BoxQuads {
                border: quad(Affine::IDENTITY, e.rect()),
                transformed: false,
                border_widths: e.border,
                padding: e.padding,
                margin,
                width: e.width,
                height: e.height,
                fragments: Vec::new(),
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
    /// One border-box quad per line of an inline box over several lines.
    pub fragments: Vec<[f64; 8]>,
    pub fixed: bool,
}

/// `r`'s corners through `t`, clockwise from the top-left one.
/// The inline root's line coordinates (x inline, y block, in CSS px from the first line box) in
/// its border box. A vertical writing mode turns them a quarter clockwise; lines of `vertical-lr`
/// are then each rotated within their slot (see [`line_slot`]).
fn line_box_origin(doc: &BaseDocument, id: NodeId, text: &blitz_dom::node::TextLayout) -> Affine {
    let l = doc.physical_unrounded_layout(id);
    let (left, top) = ((l.border.left + l.padding.left) as f64, (l.border.top + l.padding.top) as f64);
    let block_offset = text.block_offset as f64;
    match doc.get_node(id).and_then(|n| n.vertical_block_axis()) {
        None => Affine::translate((left, top + block_offset)),
        Some(true) => Affine::new([0.0, 1.0, -1.0, 0.0, left + block_offset, top]),
        Some(false) => {
            let right = (l.size.width - l.border.right - l.padding.right) as f64;
            Affine::new([0.0, 1.0, -1.0, 0.0, right - block_offset, top])
        }
    }
}

/// What moves a line (CSS px) to its slot in [`line_box_origin`]'s coordinates.
fn line_slot(doc: &BaseDocument, id: NodeId, metrics: &parley::layout::LineMetrics, scale: f64) -> Affine {
    match doc.get_node(id).and_then(|n| n.vertical_block_axis()) {
        Some(true) => Affine::translate((0.0, -((metrics.block_min_coord + metrics.block_max_coord) as f64) / scale)),
        _ => Affine::IDENTITY,
    }
}

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
        for (id, node) in doc.tree().iter() {
            let Some(text) = node.element_data().and_then(|e| e.inline_layout_data.as_ref()) else { continue };
            if !node.flags.is_inline_root() || text.text_nodes.is_empty() || !self.in_layout_tree(doc, id) {
                continue;
            }
            let Some((t, fixed)) = self.transform(doc, id) else { continue };
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
        out
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
/// [`Geometry::in_layout_tree`] indexes them).
fn in_layout_tree(doc: &BaseDocument, id: NodeId) -> bool {
    if doc.root_element().id == id {
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
            let height = s.as_ref().map(|s| format!("{:?}", s.clone_height())).unwrap_or_default();
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

fn is_cell(doc: &BaseDocument, id: NodeId) -> bool {
    matches!(tag(doc, id), Some("td" | "th"))
}

fn is_table_part(doc: &BaseDocument, id: NodeId) -> bool {
    matches!(tag(doc, id), Some("tr" | "thead" | "tbody" | "tfoot"))
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
fn cell_gutters(doc: &BaseDocument, id: NodeId) -> Option<(f64, f64, f64)> {
    let mut table = doc.get_node(id)?.parent;
    while let Some(t) = table {
        if tag(doc, t) == Some("table") {
            break;
        }
        table = doc.get_node(t)?.parent;
    }
    table_gutters(doc, table?, doc.get_node(id)?.parent)
}

fn table_gutters(doc: &BaseDocument, table: NodeId, row: Option<NodeId>) -> Option<(f64, f64, f64)> {
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
    let (mut top, mut bottom) = (gy, gy);
    if let Some(rs) = row.and_then(|p| doc.get_node(p)).and_then(|r| r.primary_styles()) {
        let b = rs.get_border();
        top = top.max(side(b.border_top_width.0, b.border_top_style) / 2.0);
        bottom = bottom.max(side(b.border_bottom_width.0, b.border_bottom_style) / 2.0);
    }
    Some((gx, top, bottom))
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
