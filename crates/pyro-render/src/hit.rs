//! Hit-test candidates by descending the document tree.
//!
//! pyro-dom hit-tests a viewport point against the elements' boxes
//! ([`Geometry::element_box`]) in paint order. Reading every element's box
//! for one point costs the whole document; this descent visits, from the
//! root element, only the subtrees whose bound can hold a box under the
//! point, and reports the elements whose own box may contain it (pyro-dom
//! applies its exact rules: visibility, `pointer-events`, clips, stacking)
//! together with every ancestor of such an element (their boxes feed the
//! paint order and the clips), each with its place in tree order.
//!
//! A subtree's bound is what Blitz keeps per box after layout: its border
//! box and its hit overflow (`Node::hit_overflow`: the children's boxes and
//! hit overflow it does not clip, transformed, its lines and the glyph
//! content areas reaching past them; unlike the scrollable overflow it keeps
//! rects of no area, such as lines of `line-height: 0`). Boxes that escape
//! it are reached another way:
//!
//! - out-of-flow boxes are listed by their containing block
//!   (`Node::hoisted_children`): one whose bound holds the point opens the
//!   tree path down to it;
//! - a table row or row group has no Blitz box: its bound is the union of
//!   its cells' contributions (`TableContext::row_hit_extents`);
//! - an inline element has no Blitz box either: its fragments are
//!   collected from its inline formatting context's lines in one pass over
//!   the lines when the context is entered;
//! - an element pyro-dom moves (a scrolled container's content, a sticky
//!   box) is a shift source: it and its ancestors are never culled, and
//!   below it the shift pyro-dom reports for a node (`offset`) applies to
//!   its whole subtree (shadow-tree content, which pyro-dom does not shift,
//!   is covered by also testing the unshifted point).
//!
//! The bound is a superset: a visited node that is not a candidate costs a
//! few comparisons, a missed candidate would be a wrong answer.

use blitz_dom::node::SpecialElementData;
use blitz_dom::{BaseDocument, NodeId};
use kurbo::{Affine, Rect};
use rustc_hash::{FxHashMap, FxHashSet};
use stylo_taffy::CopyStyleGetters;

use crate::geometry::{self, Geometry, LayoutBox};
use crate::mirror::Mirror;

/// A viewport point to hit-test, as pyro-dom poses it.
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

/// One element the descent reports: a candidate (`hit`: its box may hold
/// the point) or an ancestor of one, with its box and its place in tree
/// order among the reported elements.
#[derive(Clone, Debug, PartialEq)]
pub struct HitVisit {
    pub key: u64,
    pub order: u32,
    pub bounds: LayoutBox,
    pub hit: bool,
}

/// Tolerance of the bounds: the layout-unit truncation of reported boxes
/// (1/64 px) and f32 layout noise.
const EPS: f64 = 1.0 / 64.0;

struct Path {
    key: Option<u64>,
    order: u32,
    /// An element below (or itself) is a candidate: report this one too.
    needed: bool,
    /// Already in the output (as a candidate).
    reported: bool,
}

struct Descent<'a> {
    g: &'a Geometry,
    doc: &'a BaseDocument,
    m: &'a Mirror,
    probe: HitProbe,
    /// pyro-dom's shift of a node's box (scroll offsets less sticky offsets)
    /// by key, consulted only while `shifting`.
    offset: &'a dyn Fn(u64) -> (f64, f64),
    shifting: bool,
    scale: f64,
    transforms: FxHashMap<NodeId, (Affine, bool, bool)>,
    /// Never culled: shift sources, their ancestors, and the paths opened
    /// to out-of-flow boxes under the point.
    no_cull: FxHashSet<NodeId>,
    /// Inline elements' fragment unions (document CSS px), from the inline
    /// formatting contexts entered so far.
    unions: FxHashMap<NodeId, Rect>,
    /// Inline elements' block-axis border and padding (CSS px, before and
    /// after), as the unions grow by them.
    inline_growth: FxHashMap<NodeId, (f64, f64)>,
    /// Table rows' and row groups' bounds (document CSS px), from the
    /// tables entered so far.
    rows: FxHashMap<NodeId, Rect>,
    /// pyro: a multi-column container has been laid out in the document.
    multicol: bool,
    /// pyro: the boxes in a multi-column container's strip (blitz-dom `layout::multicol`): each
    /// is shown in the column where it lies ([`Geometry::transform`]), and none is culled (a
    /// box split by a column break has its parts in different columns).
    columns: FxHashSet<NodeId>,
    path: Vec<Path>,
    out: Vec<HitVisit>,
    order: u32,
}

/// The elements of `m`'s document whose box may hold `probe`, and their
/// ancestors (see the module docs). `shifted`: the keys of the elements
/// pyro-dom shifts (scrolled containers, sticky boxes); `offset`: the shift
/// of any element's box, consulted when `shifted` is not empty.
pub(crate) fn candidates(
    g: &Geometry,
    m: &Mirror,
    probe: &HitProbe,
    offset: &dyn Fn(u64) -> (f64, f64),
    shifted: &[u64],
) -> Vec<HitVisit> {
    let doc = &m.doc;
    let scale = doc.viewport().scale_f64();
    let mut d = Descent {
        g,
        doc,
        m,
        probe: *probe,
        offset,
        shifting: !shifted.is_empty(),
        scale,
        transforms: FxHashMap::default(),
        no_cull: FxHashSet::default(),
        unions: FxHashMap::default(),
        inline_growth: FxHashMap::default(),
        rows: FxHashMap::default(),
        multicol: doc.has_multicol(),
        columns: FxHashSet::default(),
        path: Vec::new(),
        out: Vec::new(),
        order: 0,
    };
    for &key in shifted {
        let mut cur = m.node(key);
        while let Some(id) = cur {
            if !d.no_cull.insert(id) {
                break;
            }
            cur = doc.get_node(id).and_then(|n| n.parent);
        }
    }
    if let Some(root) = doc.try_root_element() {
        d.visit(root.id);
    }
    let mut out = d.out;
    out.sort_by_key(|v| v.order);
    out
}

impl Descent<'_> {
    fn transform(&mut self, id: NodeId) -> Option<(Affine, bool)> {
        self.transform_and_planes(id).map(|(t, fixed, _)| (t, fixed))
    }

    /// [`Self::transform`], and whether a plane placed by its homography is on `id`'s
    /// containing-block chain (pyro, lane H2: its box is then where `Geometry::projective` puts it).
    fn transform_and_planes(&mut self, id: NodeId) -> Option<(Affine, bool, bool)> {
        if let Some(t) = self.transforms.get(&id) {
            return Some(*t);
        }
        let node = self.doc.get_node(id)?;
        if let Some(p) = node.containing_block().filter(|_| self.multicol) {
            let (_, _, above) = self.transform_and_planes(p)?;
            if self.columns.contains(&p) || self.doc.get_node(p)?.columns_with_axes().is_some() {
                self.columns.insert(id);
                let (t, fixed) = self.g.transform(self.doc, id)?;
                let projective = above || node.plane_3d().is_some_and(geometry::is_projective_plane);
                self.transforms.insert(id, (t, fixed, projective));
                return Some((t, fixed, projective));
            }
        }
        let (base, fixed, projective) = match node.containing_block() {
            Some(p) => {
                let (t, fixed, projective) = self.transform_and_planes(p)?;
                let scroll = self.doc.get_node(p)?.scroll_offset();
                (t * Affine::translate((-scroll.x, -scroll.y)), fixed, projective)
            }
            None => (Affine::IDENTITY, false, false),
        };
        let node = self.doc.get_node(id)?;
        let fixed = fixed
            || node.primary_styles().is_some_and(|s| s.clone_position() == style::computed_values::position::T::Fixed);
        let projective = projective || node.plane_3d().is_some_and(geometry::is_projective_plane);
        // Composed as `Geometry::transform` composes it (the placement first), to the bit.
        let t = base * geometry::placement(self.doc, node, self.scale);
        debug_assert_eq!(Some((t, fixed)), self.g.transform(self.doc, id), "memoized transform of {id:?}");
        self.transforms.insert(id, (t, fixed, projective));
        Some((t, fixed, projective))
    }

    /// pyro (lane H2): `local` (CSS px, `id`'s border box's coordinates) where paint draws it:
    /// through `t`, or through the homography of a plane drawn through a perspective on `id`'s
    /// containing-block chain.
    fn placed(&mut self, id: NodeId, t: &Affine, local: Rect) -> Rect {
        if self.transform_and_planes(id).is_some_and(|(_, _, projective)| projective) {
            if let Some(h) = self.g.projective(self.doc, id) {
                return h.bbox(local).unwrap_or(Rect::ZERO);
            }
        }
        t.transform_rect_bbox(local)
    }

    /// The point in document coordinates for a box shifted by `shift`
    /// (`fixed`: the window scroll does not apply).
    fn point(&self, fixed: bool, shift: (f64, f64)) -> (f64, f64) {
        let (wx, wy) = if fixed { (0.0, 0.0) } else { self.probe.window_scroll };
        (self.probe.x + wx + shift.0, self.probe.y + wy + shift.1)
    }

    fn contains(&self, r: &Rect, p: (f64, f64)) -> bool {
        (self.probe.ignore_x || (p.0 >= r.x0 && p.0 <= r.x1)) && p.1 >= r.y0 && p.1 <= r.y1
    }

    /// Whether `r` holds the point for any box below a node shifted by
    /// `shift`: shifted or not (shadow-tree content), fixed or not.
    fn contains_any(&self, r: &Rect, shift: (f64, f64)) -> bool {
        let mut shifts = [shift, (0.0, 0.0)];
        if shift == (0.0, 0.0) {
            shifts[1] = shift;
        }
        shifts.iter().any(|&s| self.contains(r, self.point(false, s)) || self.contains(r, self.point(true, s)))
    }

    fn shift(&self, key: Option<u64>) -> (f64, f64) {
        match (self.shifting, key) {
            (true, Some(key)) => (self.offset)(key),
            _ => (0.0, 0.0),
        }
    }

    /// Grows `r` by the tolerance, scaled for a transform `t` (a bound grown
    /// before the transform grows by at most the transform's norm).
    fn grown(&self, r: Rect, t: &Affine) -> Rect {
        let [a, b, c, d, _, _] = t.as_coeffs();
        let m = EPS * (a * a + b * b + c * c + d * d).sqrt().max(1.0);
        Rect::new(r.x0 - m, r.y0 - m, r.x1 + m, r.y1 + m)
    }

    /// The bound of an in-tree box `id` (document CSS px): its border box
    /// and hit overflow, transformed and grown by the tolerance. A cell's
    /// box also takes its share of collapsed borders (its gutters), as its
    /// own box does in [`Self::visit`]: an out-of-flow cell, whose row does
    /// not hold it, was culled at a point in its gutters.
    fn bound(&mut self, id: NodeId) -> Option<(Rect, bool)> {
        let (t, fixed) = self.transform(id)?;
        let l = self.doc.physical_unrounded_layout(id);
        let node = self.doc.get_node(id)?;
        let border = Rect::new(0.0, 0.0, l.size.width as f64, l.size.height as f64);
        let local = border.union(node.hit_overflow().scale_from_origin(1.0 / self.scale));
        // pyro (lane H2): both where a plane drawn through a perspective is and where its stand-in
        // puts what is placed by it (inline fragments, table rows), as `Node::hit_bound_in_parent`.
        let mut r = t.transform_rect_bbox(local).union(self.placed(id, &t, local));
        if geometry::is_cell(self.doc, id) {
            if let Some((gx, top, bottom)) = geometry::cell_gutters(self.doc, id) {
                r = Rect::new(r.x0 - gx, r.y0 - top, r.x1 + gx, r.y1 + bottom);
            }
        }
        Some((self.grown(r, &t), fixed))
    }

    fn visit(&mut self, id: NodeId) {
        let Some(node) = self.doc.get_node(id) else { return };
        if !node.is_element() || node.hidden_by_anchor() {
            return;
        }
        // Not rendered (display: none, or inside it: never styled): no box
        // here or below.
        if !node.primary_styles().is_some_and(|s| !s.clone_display().is_none()) {
            return;
        }
        self.order += 1;
        let order = self.order;
        let key = self.m.key_of(id);
        let shift = self.shift(key);
        let in_tree = self.g.in_layout_tree(self.doc, id);
        let is_table_part = geometry::is_table_part(self.doc, id);
        let is_cell = geometry::is_cell(self.doc, id);
        // Whether the element's own box may hold the point, and the bound
        // of its subtree (`None`: not culled).
        let (candidate, cull): (bool, Option<Rect>) = if in_tree {
            let Some((t, fixed, projective)) = self.transform_and_planes(id) else { return };
            let l = self.doc.physical_unrounded_layout(id);
            let (w, h) = (l.size.width as f64, l.size.height as f64);
            // pyro (lane H2): a box in a plane drawn through a perspective is where paint draws it
            // (`Self::placed`); pyro: a box in columns, the union of its pieces in them
            // (`Geometry::columns_box`). The order is `Geometry::laid_out`'s.
            let mut own = match self.columns.contains(&id) && (!projective || node.plane_3d().is_some_and(geometry::is_projective_plane)) {
                true => self.g.columns_box(self.doc, id),
                false => None,
            }
            .unwrap_or_else(|| self.placed(id, &t, Rect::new(0.0, 0.0, w, h)));
            if is_cell {
                if let Some((gx, top, bottom)) = geometry::cell_gutters(self.doc, id) {
                    own = Rect::new(own.x0 - gx, own.y0 - top, own.x1 + gx, own.y1 + bottom);
                }
            }
            let own = Rect::new(own.x0 - EPS, own.y0 - EPS, own.x1 + EPS, own.y1 + EPS);
            let p = self.point(fixed, shift);
            // A zero box is never hit (pyro-dom skips it); its children may be.
            let candidate = (w > 0.0 || h > 0.0) && self.contains(&own, p);
            // pyro: nor is a multi-column container's subtree: its hit bound (its strip's overflow
            // cut at the breaks, `resolve.rs`) missed content the scan of every box reached
            // (`hit_adversarial`'s interleaved-mutations page 26).
            let in_columns = self.columns.contains(&id) || (self.multicol && node.columns_with_axes().is_some());
            let cull = if in_columns || node.is_split_inline() { None } else { self.bound(id).map(|(r, _)| r) };
            let may_hit_text = (w == 0.0 || h == 0.0) && (node.flags.is_inline_root() || !node.anonymous_blocks.is_empty());
            (candidate || (may_hit_text && cull.is_none_or(|r| self.contains_any(&r, shift))), cull)
        } else if is_table_part {
            match self.rows.get(&id).copied() {
                // Both fixed states: the row's box takes its table's.
                Some(r) => (self.contains_any(&r, shift), Some(r)),
                // A row the table's grid does not know: nothing is culled.
                None => (key.is_some(), None),
            }
        } else {
            let inline = node.primary_styles().is_some_and(|s| s.clone_display().is_inline_flow());
            let candidate = inline
                && match self.unions.get(&id).copied() {
                    Some(r) => {
                        let r = Rect::new(r.x0 - EPS, r.y0 - EPS, r.x1 + EPS, r.y1 + EPS);
                        self.contains_any(&r, shift)
                    }
                    None => false,
                };
            (candidate, None)
        };
        self.path.push(Path { key, order, needed: false, reported: false });
        // pyro: a box a line clamp discarded (a float after the clamp point too) is not hit; its
        // place stays in the order (blitz-dom `NodeFlags::HIDDEN_BY_CLAMP`).
        let candidate = candidate && !geometry::hidden_by_clamp(self.doc, id);
        if candidate {
            if let (Some(key), Some(b)) = (key, self.g.element_box(self.doc, id)) {
                if b.width > 0.0 || b.height > 0.0 {
                    self.out.push(HitVisit { key, order, bounds: b, hit: true });
                    for p in self.path.iter_mut().rev() {
                        if p.needed {
                            break;
                        }
                        p.needed = true;
                    }
                    self.path.last_mut().expect("the node's own path entry").reported = true;
                }
            }
        }
        // Out-of-flow boxes laid out by this node may lie outside its bound
        // (a fixed box below a short document's root): they open the path.
        // So do those laid out by the anonymous blocks it wraps its inline
        // children in (whose positioned inline element is their containing
        // block, an anonymous block claims them), which the descent, going
        // over DOM children, does not visit.
        self.open_hoisted(id, id);
        let anonymous: Vec<NodeId> = node.anonymous_blocks.to_vec();
        for a in anonymous {
            self.open_hoisted(a, id);
        }
        let descend = match cull {
            Some(r) => self.no_cull.contains(&id) || self.contains_any(&r, shift),
            None => true,
        };
        if descend {
            if in_tree {
                self.enter_inline_roots(id);
                self.enter_table(id);
            }
            let children: Vec<NodeId> = self.doc.get_node(id).map(|n| n.children.to_vec()).unwrap_or_default();
            for child in children {
                blitz_dom::util::ensure_stack(|| self.visit(child));
            }
        }
        let p = self.path.pop().expect("the node's own path entry");
        if p.needed && !p.reported {
            if let (Some(key), Some(b)) = (p.key, self.g.element_box(self.doc, id)) {
                self.out.push(HitVisit { key, order: p.order, bounds: b, hit: false });
            }
        }
    }

    /// Out-of-flow boxes laid out by `cb` (their containing block: `id`, or
    /// an anonymous block of `id`'s) whose bound holds the point open their
    /// tree path: the nodes between them and `id` (whose own bounds may
    /// leave them out) are not culled.
    fn open_hoisted(&mut self, cb: NodeId, id: NodeId) {
        let Some(node) = self.doc.get_node(cb) else { return };
        let hoisted: Vec<NodeId> = node.hoisted_children.borrow().to_vec();
        for h in hoisted {
            if self.doc.get_node(h).is_none() || !self.g.in_layout_tree(self.doc, h) {
                continue;
            }
            let shift = self.shift(self.m.key_of(h));
            let Some((r, _)) = self.bound(h) else { continue };
            if !self.contains_any(&r, shift) {
                continue;
            }
            let mut cur = self.doc.get_node(h).and_then(|n| n.parent);
            while let Some(a) = cur {
                if !self.no_cull.insert(a) || a == id {
                    break;
                }
                cur = self.doc.get_node(a).and_then(|n| n.parent);
            }
        }
    }

    /// The fragment unions of the inline elements laid out in `id`'s
    /// inline formatting context (itself, or the anonymous blocks it wraps
    /// its inline children in), as [`Geometry::inline_union`] reports them
    /// (document CSS px; a superset for an element split over several
    /// anonymous blocks).
    fn enter_inline_roots(&mut self, id: NodeId) {
        let Some(node) = self.doc.get_node(id) else { return };
        let mut roots = Vec::new();
        if node.flags.is_inline_root() {
            roots.push(id);
        }
        roots.extend(
            node.anonymous_blocks
                .iter()
                .copied()
                .filter(|&a| self.doc.get_node(a).is_some_and(|a| a.flags.is_inline_root())),
        );
        for root in roots {
            self.collect_unions(root, id);
        }
    }

    fn collect_unions(&mut self, root_id: NodeId, stop: NodeId) {
        let Some((t, _)) = self.transform(root_id) else { return };
        let Some(root) = self.doc.get_node(root_id) else { return };
        let Some(text) = root.element_data().and_then(|e| e.inline_layout_data.as_ref()) else { return };
        let doc = self.doc;
        let line_origin = geometry::line_box_origin(doc, root_id, text);
        let origin = t * line_origin;
        let scale = text.layout.scale() as f64;
        // pyro: lines in a multi-column container's columns.
        let columns = if self.multicol { geometry::line_column_shift(doc, root_id) } else { None };
        for line in text.layout.lines() {
            let mut origin = origin * geometry::line_slot(doc, root_id, &line.metrics(), scale);
            if let Some(shift) = &columns {
                let top = line_origin * kurbo::Point::new(0.0, line.metrics().block_min_coord as f64 / scale);
                origin = shift(top.y) * origin;
            }
            for item in line.items() {
                let (start, r) = geometry::inline_item_rect(&line, &item, scale, text.layout.is_rtl());
                let origin = geometry::inline_item_origin(doc, root_id, start, text, t, origin);
                // An element's fragment grows to its border box in the block axis.
                let to_doc = |r: Rect, (top, bottom): (f64, f64)| {
                    origin.transform_rect_bbox(Rect::new(
                        r.x0 / scale,
                        r.y0 / scale - top,
                        r.x1 / scale,
                        r.y1 / scale + bottom,
                    ))
                };
                // A span's own edge is in its box without the margin.
                let own = geometry::span_edge_box(doc, &item, r, scale);
                let mut cur = start;
                while let Some(n) = cur.filter(|&n| n != root_id && n != stop) {
                    let Some(node) = doc.get_node(n) else { break };
                    if node.is_element() {
                        let growth = *self.inline_growth.entry(n).or_insert_with(|| geometry::inline_block_growth(doc, n));
                        let r = to_doc(if cur == start { own } else { r }, growth);
                        self.unions.entry(n).and_modify(|u| *u = u.union(r)).or_insert(r);
                    }
                    cur = node.parent;
                }
            }
        }
    }

    /// The bounds of the rows and row groups of table `id` (document CSS
    /// px): each row's cells' overflow contributions in the table, grown by
    /// the collapsed-border gutters its cells' boxes get and the tolerance.
    fn enter_table(&mut self, id: NodeId) {
        let Some(node) = self.doc.get_node(id) else { return };
        if !node.flags.is_table_root() {
            return;
        }
        let Some(SpecialElementData::TableRoot(ctx)) = node.element_data().map(|e| &e.special_data) else { return };
        let ctx = std::sync::Arc::clone(ctx);
        let Some((t, _)) = self.transform(id) else { return };
        // pyro: a table in a multi-column container's strip may be cut across columns, its rows
        // shown in other columns than its origin's: they keep no bound (nothing is culled).
        if self.columns.contains(&id) {
            return;
        }
        let doc = self.doc;
        let scale = self.scale;
        let mut extents: Vec<Rect> =
            ctx.row_hit_extents(|rows, at| if at == scale { rows.to_vec() } else { Vec::new() });
        if extents.len() != ctx.rows.len() {
            // The transform pass keeps no per-row union for this table: the
            // cells' hit contributions, as that pass computes them.
            extents = vec![Rect::ZERO; ctx.rows.len()];
            for cell in &ctx.cells {
                let Some(node) = doc.get_node(cell.node_id) else { continue };
                let l = doc.physical_unrounded_layout(cell.node_id);
                let contribution = doc.overflow_contribution(cell.node_id, *node.hit_overflow(), l.size, scale);
                let location = (l.location.x as f64 * scale, l.location.y as f64 * scale);
                let r = node.hit_bound_in_parent(location, scale, contribution);
                if r.width() > 0.0 || r.height() > 0.0 {
                    if let Some(e) = extents.get_mut(cell.row as usize) {
                        *e = if e.width() > 0.0 || e.height() > 0.0 { e.union(r) } else { r };
                    }
                }
            }
        }
        let base = geometry::table_gutter_base(doc, id);
        let mut groups: FxHashMap<NodeId, Rect> = FxHashMap::default();
        for (row, extent) in ctx.rows.iter().zip(extents) {
            if extent.width() <= 0.0 && extent.height() <= 0.0 {
                self.rows.insert(row.node_id, Rect::ZERO);
                continue;
            }
            let (gx, top, bottom) =
                base.map(|b| geometry::row_gutters(doc, Some(row.node_id), b)).unwrap_or((0.0, 0.0, 0.0));
            let local = extent.scale_from_origin(1.0 / scale);
            let local = Rect::new(local.x0 - gx, local.y0 - top, local.x1 + gx, local.y1 + bottom);
            // pyro (lane H2): in a plane drawn through a perspective, where it is drawn too.
            let placed = t.transform_rect_bbox(local).union(self.placed(id, &t, local));
            let r = self.grown(placed, &t);
            self.rows.insert(row.node_id, r);
            if let Some(g) = row.group {
                groups.entry(g).and_modify(|u| *u = u.union(r)).or_insert(r);
            }
        }
        self.rows.extend(groups);
    }
}
