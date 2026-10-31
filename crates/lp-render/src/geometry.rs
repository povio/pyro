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
use rustc_hash::FxHashMap;

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
        }
    }
}

/// Per-document memo, valid until the document changes ([`Geometry::clear`]).
#[derive(Default)]
pub(crate) struct Geometry {
    transforms: RefCell<FxHashMap<NodeId, (Affine, bool)>>,
}

impl Geometry {
    pub(crate) fn clear(&self) {
        self.transforms.borrow_mut().clear();
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
            || node
                .primary_styles()
                .is_some_and(|s| s.clone_position() == style::computed_values::position::T::Fixed);
        let location = node.unrounded_layout().location;
        let t = base
            * Affine::translate((location.x as f64, location.y as f64))
            * node.transform().as_deref().copied().unwrap_or_default();
        self.transforms.borrow_mut().insert(id, (t, fixed));
        Some((t, fixed))
    }

    /// The box Blitz lays out for `id`, before the adjustments.
    fn laid_out(&self, doc: &BaseDocument, id: NodeId) -> Option<LayoutBox> {
        let node = doc.get_node(id)?;
        let (t, fixed) = self.transform(doc, id)?;
        let l = node.unrounded_layout();
        let rect = t.transform_rect_bbox(Rect::new(0.0, 0.0, l.size.width as f64, l.size.height as f64));
        let client_width = (l.size.width - l.border.left - l.border.right - l.scrollbar_size.width).max(0.0) as f64;
        let client_height = (l.size.height - l.border.top - l.border.bottom - l.scrollbar_size.height).max(0.0) as f64;
        let o = node.scrollable_overflow();
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
        })
    }

    /// `id`'s box as Chromium reports it (see the module docs).
    pub(crate) fn element_box(&self, doc: &BaseDocument, id: NodeId) -> Option<LayoutBox> {
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
            if let Some(r) = self.children_union(doc, id) {
                return Some(LayoutBox::from_rect(r, b.fixed));
            }
            return Some(b);
        }
        let inline = doc
            .get_node(id)
            .and_then(|n| n.primary_styles())
            .is_some_and(|s| s.clone_display().is_inline_flow());
        if inline {
            if let Some(r) = self.inline_union(doc, id) {
                return Some(LayoutBox { client_width: 0.0, client_height: 0.0, scroll_width: 0.0, scroll_height: 0.0, ..LayoutBox::from_rect(r, b.fixed) });
            }
        }
        Some(b)
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
    /// text layout: glyph runs (content area: ascent to descent) and atomic
    /// inlines inside it.
    fn inline_union(&self, doc: &BaseDocument, id: NodeId) -> Option<Rect> {
        let mut root_id = doc.get_node(id)?.parent;
        while let Some(r) = root_id {
            let n = doc.get_node(r)?;
            if n.flags.is_inline_root() {
                break;
            }
            root_id = n.parent;
        }
        let root_id = root_id?;
        let root = doc.get_node(root_id)?;
        let text = root.element_data()?.inline_layout_data.as_ref()?;
        let (t, _) = self.transform(doc, root_id)?;
        let l = root.unrounded_layout();
        let origin = t * Affine::translate((
            (l.border.left + l.padding.left) as f64,
            (l.border.top + l.padding.top) as f64 + text.block_offset as f64,
        ));
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
        let mut acc: Option<Rect> = None;
        for line in text.layout.lines() {
            for item in line.items() {
                let (start, r) = match item {
                    parley::PositionedLayoutItem::GlyphRun(run) => {
                        let metrics = run.run().font_metrics();
                        let (x, baseline) = (run.offset() as f64, run.baseline() as f64);
                        (
                            Some(run.style().brush.id),
                            Rect::new(
                                x,
                                baseline - metrics.ascent as f64,
                                x + run.advance() as f64,
                                baseline + metrics.descent as f64,
                            ),
                        )
                    }
                    parley::PositionedLayoutItem::InlineBox(b) => (
                        Some(NodeId::from_u64(b.id)),
                        Rect::new(b.x as f64, b.y as f64, (b.x + b.width) as f64, (b.y + b.height) as f64),
                    ),
                };
                if within(start) {
                    let r = origin
                        .transform_rect_bbox(Rect::new(r.x0 / scale, r.y0 / scale, r.x1 / scale, r.y1 / scale));
                    acc = Some(acc.map_or(r, |a| a.union(r)));
                }
            }
        }
        acc
    }
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
    let table = table?;
    let styles = doc.get_node(table)?.primary_styles()?;
    if styles.clone_border_collapse() != style::computed_values::border_collapse::T::Collapse {
        return None;
    }
    let cell = doc.get_node(first_cell(doc, table)?)?.primary_styles()?;
    let b = cell.get_border();
    let gx = side(b.border_left_width.0, b.border_left_style).max(side(b.border_right_width.0, b.border_right_style)) / 2.0;
    let gy = side(b.border_top_width.0, b.border_top_style).max(side(b.border_bottom_width.0, b.border_bottom_style)) / 2.0;
    let (mut top, mut bottom) = (gy, gy);
    if let Some(rs) = doc.get_node(id)?.parent.and_then(|p| doc.get_node(p)).and_then(|r| r.primary_styles()) {
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
