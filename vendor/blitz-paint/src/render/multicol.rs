//! pyro: multi-column containers (blitz-dom `layout/multicol.rs`).
//!
//! A multi-column container's content is laid out in one strip a column wide; each column shows
//! a range of it (`ColumnFragment`). The content is painted once per column, moved to the column
//! and clipped to its range in the block axis only (what overflows a column sideways shows, as in
//! Chromium; the first column shows what lies above the strip, the last what lies below it). A box
//! a break goes through is drawn in both columns, each showing its part: `box-decoration-break:
//! slice`. The column rules are painted between columns that both have content, over the border
//! and under the content.

use super::ElementCx;
use crate::color::ToColorColor;
use anyrender::PaintScene;
use blitz_dom::CopyStyleGetters;
use kurbo::{Affine, Point, Rect, Vec2};
use peniko::{BlendMode, Fill};
use style::values::computed::BorderStyle;

/// Far enough in every direction to leave a column's content unclipped sideways (device px).
const UNBOUNDED: f64 = 1e7;

impl ElementCx<'_, '_> {
    /// Paints the content of a multi-column container, once per column. `false` (and nothing
    /// painted) when the element is no multi-column container.
    pub(super) fn draw_columns(&mut self, scene: &mut impl PaintScene, pos: Point, clip_rect: Rect) -> bool {
        // pyro: a vertical writing mode's columns are in its logical axes (block = y): its block
        // axis is physical x, running right to left in `vertical-rl` from the border box's width.
        let Some((columns, axes)) = self.node.columns_with_axes() else { return false };
        let vertical = self.node.vertical_column_fragments();
        let base = self.transform;
        // pyro: `pos` is the content box less the scroll offset, which `self.transform` (scrolled
        // for the content) applies already: the columns' content takes it once (WPT css-overflow
        // `scroll-markers/column-scroll-marker-007`).
        let scroll = *self.node.scroll_offset();
        let pos = Point { x: pos.x + scroll.x, y: pos.y + scroll.y };
        let screen = Affine::translate((-self.context.initial_x, -self.context.initial_y));
        let scale = self.scale;
        let single = columns.fragments.len() == 1;
        // pyro: the out-of-flow boxes the container is the containing block of are placed against
        // it, not in a column: painted once, after the columns (`multicol_skips`).
        let outer_pass = self.context.multicol_pass.replace(Some((self.node.id, false)));
        let row_spanners = self.context.dom.as_ref().column_spanners_in_order(self.node.id);
        let mut spanner_row = 0usize;
        for fragment in &columns.fragments {
            let (start, end) = (fragment.start as f64, fragment.end as f64);
            let offset = (axes * Point::new(fragment.offset.x as f64, fragment.offset.y as f64) - axes * Point::ORIGIN) * scale;
            let block_range = |low: f64, high: f64| (Affine::scale(scale) * axes).transform_rect_bbox(Rect::new(-UNBOUNDED / scale, low.max(-UNBOUNDED / scale), UNBOUNDED / scale, high.min(UNBOUNDED / scale)));
            let range = block_range(start, end);
            // pyro: a spanner's row shows its outline over the row's edges too.
            // pyro: a spanner's row paints its spanner alone (`spanner_row_spanner`), unclipped in
            // the block axis: what overflows it shows (the rows are in the spanners' order).
            let row_spanner = if fragment.spanner {
                spanner_row += 1;
                row_spanners.get(spanner_row - 1).copied()
            } else {
                None
            };
            // (A spanner's row that is the last fragment shows what comes after it too: a second
            // pass, clipped at its start, without the spanner.)
            let last_row_rest = row_spanner.filter(|_| fragment.end.is_infinite()).map(|spanner| {
                let outline = self.spanner_ink_outset() * scale;
                (spanner, block_range(start - outline / scale, f64::INFINITY))
            });
            let range = if fragment.spanner {
                match row_spanner {
                    Some(_) => block_range(f64::NEG_INFINITY, f64::INFINITY),
                    None => {
                        let outline = self.spanner_ink_outset() * scale;
                        block_range(start - outline / scale, end + outline / scale)
                    }
                }
            } else {
                range
            };
            // (Restored on every way out of this column, culled or painted.)
            let row_guard = Restore::set(&self.context.spanner_row_spanner, row_spanner);
            let column_guard = Restore::set(
                &self.context.column_pass_container,
                (!fragment.spanner).then_some(self.node.id),
            );
            let _inline_origin = Restore::set(
                &self.context.column_inline_origin,
                (!fragment.spanner && vertical.is_none() && fragment.exact_x.is_finite())
                    .then_some((self.node.id, fragment.offset.x + (fragment.exact_x - fragment.x))),
            );
            let _block_offset = Restore::set(&self.context.column_block_offset, self.context.column_block_offset.get() + if vertical.is_none() { f64::from(fragment.offset.y) } else { 0.0 });
            let _rounding = Restore::set(&self.context.column_block_rounding_error, self.context.column_block_rounding_error.get() + f64::from(fragment.block_rounding_error));
            let spanner_row_guard = Restore::set(
                &self.context.spanner_row_container,
                fragment.spanner.then_some(self.node.id),
            );
            let transform = base.pre_translate(offset);
            // The column's part of what is visible, for culling its content; a column out of
            // sight is not painted (`Rect::intersect` never gives a negative size: overlap it is).
            let shown = (screen * transform).transform_rect_bbox(range);
            if !single && !clip_rect.overlaps(shown) {
                continue;
            }
            let column_clip = clip_rect.intersect(shown);
            let pushed = self.context.layer_manager.maybe_push_layer(
                scene,
                !single,
                BlendMode::default(),
                1.0,
                transform,
                &range,
                None,
                None,
            );
            self.transform = transform;
            let outer_inline_range = self
                .context
                .inline_column_range
                .replace((!single && vertical.is_none()).then_some((self.node.id, fragment.start, fragment.end)));
            // (A spanner's row, unclipped in the block axis, shows its spanner alone: not the
            // container's own lines, which are the columns', nor its marker, unless the row is
            // its first fragment: WPT css-multicol `multicol-list-item-005`,
            // `multicol-span-all-list-item-001`.)
            if row_spanner.is_none() {
                self.draw_inline_layout(scene, pos);
            }
            // (The marker in a spanner's row only where its line is in that row's strip range: the
            // row is unclipped, and the marker would show at each row's offset otherwise.)
            if row_spanner.is_none() {
                self.draw_marker(scene, pos);
            } else {
                let row = block_range(start, end);
                let pushed = self.context.layer_manager.maybe_push_layer(scene, true, BlendMode::default(), 1.0, transform, &row, None, None);
                self.draw_marker(scene, pos);
                self.context.layer_manager.maybe_pop_layer(scene, pushed);
            }
            // pyro: the container's boxes are not clipped to the column in the block axis, as in
            // Chromium: a box the column holds whole is painted with what overflows it, above or
            // below; a box a break goes through is sliced; a box another column holds is left to
            // it (`PaintContext::column_range`, `draw_paint_child`). (Vertical writing modes keep
            // the clip.)
            let unclipped = !single && (vertical.is_none() || !columns.first_after_spanner.is_empty());
            if unclipped {
                self.context.layer_manager.maybe_pop_layer(scene, pushed);
            }
            let outer_range = self
                .context
                .column_range
                .replace(unclipped.then_some((self.node.id, fragment.start, fragment.end, transform, range)));
            self.draw_children(scene, transform, if single || unclipped { clip_rect } else { column_clip });
            self.context.column_range.set(outer_range);
            if let Some((spanner, rest_range)) = last_row_rest {
                let outer_row_spanner = self.context.spanner_row_spanner.replace(None);
                let outer_except = self.context.spanner_row_except.replace(Some(spanner));
                // (Sorted as a column's boxes: what starts in the row shows its overflow above.)
                let outer_range = self
                    .context
                    .column_range
                    .replace(Some((self.node.id, fragment.start, fragment.end, transform, rest_range)));
                self.draw_children(scene, transform, clip_rect);
                self.context.column_range.set(outer_range);
                self.context.spanner_row_except.set(outer_except);
                self.context.spanner_row_spanner.set(outer_row_spanner);
            }
            // (Kept while the column's boxes paint too: an inline root below the container
            // records its positioned boxes for the column as the container's own lines do.)
            self.context.inline_column_range.set(outer_inline_range);
            if !unclipped {
                self.context.layer_manager.maybe_pop_layer(scene, pushed);
            }
            drop((spanner_row_guard, column_guard, row_guard));
            // pyro: a box broken at the column's end fills the rest of the column (its fragment
            // is as tall as the column: Chromium; WPT css-multicol `multicol-fill-balance-041`).
            // (Not in the rows of an explicit `column-height`, where Chromium keeps a broken box
            // to its content: WPT css-multicol `column-height-024`.)
            let explicit_height = !matches!(
                self.style.get_column().column_height,
                style::values::generics::length::LengthPercentageOrAuto::Auto
            );
            if vertical.is_none() && !explicit_height && !fragment.spanner && fragment.end.is_finite() {
                // (Where the strip's `end` is shown, to the column box's bottom.)
                let free = (fragment.top + fragment.height - fragment.extended - (fragment.end + fragment.offset.y)) as f64;
                if free > 0.5 / scale {
                    let rest = Rect::new(-UNBOUNDED, end * scale, UNBOUNDED, (end + free) * scale);
                    let shown = (screen * transform).transform_rect_bbox(rest);
                    if clip_rect.overlaps(shown) {
                        let pushed =
                            self.context.layer_manager.maybe_push_layer(scene, true, BlendMode::default(), 1.0, transform, &rest, None, None);
                        self.draw_broken_boxes(scene, transform, self.node.id, Vec2::ZERO, fragment.end, free as f32, 0);
                        self.context.layer_manager.maybe_pop_layer(scene, pushed);
                    }
                }
            }
        }
        self.transform = base;
        self.context.multicol_pass.set(Some((self.node.id, true)));
        self.draw_children(scene, base, clip_rect);
        self.context.multicol_pass.set(outer_pass);
        true
    }

    /// pyro: the backgrounds and borders of the in-flow block boxes below `parent` (its border
    /// box at `origin`, CSS px from the container's) that a column break at strip position `at`
    /// goes through, as `free` CSS px taller: drawn under a clip to the rest of the column, they
    /// fill it (a slice's block-end border stays out of view, `box-decoration-break: slice`).
    #[allow(clippy::too_many_arguments)]
    fn draw_broken_boxes(
        &self,
        scene: &mut impl PaintScene,
        transform: Affine,
        parent: blitz_dom::NodeId,
        origin: Vec2,
        at: f32,
        free: f32,
        depth: usize,
    ) {
        if depth > 64 {
            return;
        }
        let tree = self.context.dom.as_ref().tree();
        let children = tree[parent].layout_children.borrow();
        for &id in children.iter().flatten() {
            let node = &tree[id];
            let Some(style) = node.primary_styles() else { continue };
            if node.is_out_of_flow() || node.element_data().is_none() {
                continue;
            }
            let display = style.clone_display();
            let block = matches!(
                display.inside(),
                style::values::specified::box_::DisplayInside::Flow | style::values::specified::box_::DisplayInside::FlowRoot
            ) && display.outside() == style::values::specified::box_::DisplayOutside::Block;
            let visible = |o: style::values::computed::Overflow| {
                matches!(o, style::values::computed::Overflow::Visible | style::values::computed::Overflow::Clip)
            };
            let b = style.get_box();
            if !block || !visible(b.overflow_x) || !visible(b.overflow_y) || node.column_fragments().is_some() {
                continue;
            }
            let layout = *node.final_layout();
            let top = origin.y as f32 + layout.location.y;
            let bottom = top + layout.size.height;
            if !(top < at - 0.01 && bottom > at + 0.01) {
                continue;
            }
            let at_box = origin + Vec2::new(layout.location.x as f64, layout.location.y as f64);
            let mut taller = layout;
            taller.size.height += free;
            let box_transform = transform.pre_translate(at_box * self.scale);
            let cx = self.context.element_cx(node, taller, box_transform, None);
            cx.draw_background(scene);
            cx.draw_border(scene);
            self.draw_broken_boxes(scene, transform, id, at_box, at, free, depth + 1);
        }
    }

    /// pyro: how far the outlines of the container's spanners reach out of their rows (CSS px):
    /// an outline's width and positive offset.
    fn spanner_ink_outset(&self) -> f64 {
        let dom = self.context.dom.as_ref();
        let tree = dom.tree();
        let mut out = 0.0f64;
        let mut stack: Vec<blitz_dom::NodeId> = tree[self.node.id].layout_children.borrow().iter().flatten().copied().collect();
        let mut seen = 0;
        while let Some(id) = stack.pop() {
            seen += 1;
            if seen > 4096 {
                break;
            }
            if dom.is_column_spanner_of(self.node.id, id) {
                if let Some(style) = tree[id].primary_styles() {
                    let outline = style.get_outline();
                    // (An outline that is drawn: `none` keeps its computed width.)
                    let drawn = match outline.outline_style {
                        style::values::computed::OutlineStyle::Auto => true,
                        style::values::computed::OutlineStyle::BorderStyle(style) => !style.none_or_hidden(),
                    };
                    if drawn {
                        let reach = outline.outline_width.0.to_f64_px() + outline.outline_offset.px(outline.outline_width.0.to_f64_px()).max(0.0);
                        out = out.max(reach);
                    }
                }
                continue;
            }
            if let Some(children) = tree[id].layout_children.borrow().as_deref() {
                stack.extend(children.iter().copied());
            }
        }
        out
    }

    /// The column rules of a multi-column container (`column-rule`): one in the middle of the
    /// gap between each two neighbouring columns of a row, as tall as the columns, snapped to
    /// whole pixels. `clip_rect` (screen space): what can show; dotted and dashed rules make
    /// their marks there only.
    pub(super) fn draw_column_rules(&self, scene: &mut impl PaintScene, clip_rect: Rect) {
        // (Of a vertical writing mode: its columns in its logical axes, and whether its block axis
        // runs right to left.)
        let (columns, vertical) = match self.context.dom.column_fragments(self.node.id) {
            Some(columns) => (columns, None),
            None => match self.node.vertical_column_fragments() {
                Some((columns, rl)) => (columns, Some(rl)),
                None => return,
            },
        };
        if columns.fragments.len() < 2 {
            return;
        }
        let column = self.style.get_column();
        let style = column.column_rule_style;
        let width = column.column_rule_width.0.to_f64_px();
        if matches!(style, BorderStyle::None | BorderStyle::Hidden) || width <= 0.0 {
            return;
        }
        let color = column.column_rule_color.resolve_to_absolute(&self.style.clone_color()).as_srgb_color();
        if color.components[3] <= 0.0 {
            return;
        }
        let scale = self.scale;
        let thickness = (width * scale).round().max(1.0);
        // The visible part of the element's space (device px), for dotted and dashed rules.
        let screen = Affine::translate((-self.context.initial_x, -self.context.initial_y));
        let visible = (screen * self.transform).inverse().transform_rect_bbox(clip_rect);
        // (An unbounded clip gives no bound, not NaN.)
        let (seen_from, seen_to) = (
            if visible.y0.is_nan() { f64::NEG_INFINITY } else { visible.y0 },
            if visible.y1.is_nan() { f64::INFINITY } else { visible.y1 },
        );
        for pair in columns.fragments.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if a.spanner || b.spanner || (a.top - b.top).abs() > 0.5 {
                continue;
            }
            let (left, right) = if a.x <= b.x { (a, b) } else { (b, a) };
            // pyro: in a vertical writing mode the columns stack down the inline axis (physical
            // y) and the rule runs across the row's block extent (physical x, from the right
            // where the block axis runs right to left), in the middle of the gap (WPT
            // css-multicol `multicol-span-all-rule-002`). (Dotted and dashed ones are not drawn.)
            if let Some(rl) = vertical {
                if matches!(style, BorderStyle::Dotted | BorderStyle::Dashed) {
                    continue;
                }
                let width = self.node.final_layout().size.width as f64;
                let center = ((left.x + left.width) as f64 + right.x as f64) / 2.0 * scale;
                let y0 = (center - thickness / 2.0).round();
                let (from, to) = (a.top as f64, (a.top + a.height) as f64);
                let (from, to) = if rl { (width - to, width - from) } else { (from, to) };
                let (x0, x1) = ((from * scale).round(), (to * scale).round());
                if x1 <= x0 {
                    continue;
                }
                let paint = peniko::Color::from(color);
                PaintScene::fill(scene, Fill::NonZero, self.transform, paint, None, &Rect::new(x0, y0, x1, y0 + thickness));
                continue;
            }
            // pyro: centred in the gap between the columns' unsnapped edges, and snapped outwards:
            // the content beside it is snapped from those edges in Chromium (a column box at
            // 47.375..55.5 paints 47..56), here from its own (47..55), which would leave the gap's
            // partial pixels bare (WPT css-multicol `equal-gap-and-rule`).
            // (Solid rules only: a dotted or dashed one's marks are sized by its width.)
            let exact = left.exact_x.is_finite() && right.exact_x.is_finite();
            let (left_x, right_x) = if exact { (left.exact_x, right.exact_x) } else { (left.x, right.x) };
            let center = ((left_x + left.width) as f64 + right_x as f64) / 2.0 * scale;
            let outwards = exact && style == BorderStyle::Solid;
            let x0 = if outwards { (center - thickness / 2.0).floor() } else { (center - thickness / 2.0).round() };
            let thickness = if outwards { (center + thickness / 2.0).ceil() - x0 } else { thickness };
            let y0 = (a.top as f64 * scale).round();
            let y1 = ((a.top + a.height) as f64 * scale).round();
            if y1 <= y0 {
                continue;
            }
            let rect = Rect::new(x0, y0, x0 + thickness, y1);
            self.draw_rule(scene, rect, style, color, (seen_from - rect.y0, seen_to - rect.y0));
        }
    }

    /// One column rule in `rect` (device px, the element's space), painted as Chromium paints
    /// it: as the left side of a box (`BoxBorderPainter::DrawLineForBoxSide`), its style the one
    /// a collapsed table border would have (`inset` as `ridge`, `outset` as `groove`).
    fn draw_rule(&self, scene: &mut impl PaintScene, rect: Rect, style: BorderStyle, color: peniko::Color, visible: (f64, f64)) {
        use super::border::{grooved_edge_colors, straight_side_marks};
        use crate::kurbo_css::Edge;
        let t = rect.width();
        let style = match style {
            BorderStyle::Inset => BorderStyle::Ridge,
            BorderStyle::Outset => BorderStyle::Groove,
            style => style,
        };
        let fill = |scene: &mut _, color, r: &Rect| PaintScene::fill(scene, Fill::NonZero, self.transform, color, None, r);
        match style {
            BorderStyle::Double if t >= 3.0 => {
                fill(scene, color, &Rect::new(rect.x0, rect.y0, rect.x0 + t / 3.0, rect.y1));
                fill(scene, color, &Rect::new(rect.x1 - t / 3.0, rect.y0, rect.x1, rect.y1));
            }
            BorderStyle::Groove | BorderStyle::Ridge if t >= 2.0 => {
                let (outer, inner) = grooved_edge_colors(color, Edge::Left, style == BorderStyle::Ridge);
                let half = rect.x0 + t / 2.0;
                fill(scene, outer, &Rect::new(rect.x0, rect.y0, half, rect.y1));
                fill(scene, inner, &Rect::new(half, rect.y0, rect.x1, rect.y1));
            }
            BorderStyle::Dashed | BorderStyle::Dotted => {
                let mut marks = kurbo::BezPath::new();
                straight_side_marks(
                    &mut marks,
                    &mut Vec::new(),
                    style,
                    Point::new(rect.x0, rect.y0),
                    (Vec2::new(0.0, 1.0), Vec2::new(1.0, 0.0)),
                    (rect.height(), t),
                    self.scale,
                    Some(visible),
                );
                scene.fill(Fill::NonZero, self.transform, color, None, &marks);
            }
            _ => fill(scene, color, &rect),
        }
    }
}

/// pyro: a paint-context cell set for a while: its earlier value is put back when this is dropped,
/// on every way out of the scope that set it.
struct Restore<'c, T: Copy> {
    cell: &'c std::cell::Cell<T>,
    outer: T,
}

impl<'c, T: Copy> Restore<'c, T> {
    fn set(cell: &'c std::cell::Cell<T>, value: T) -> Self {
        Self { cell, outer: cell.replace(value) }
    }
}

impl<T: Copy> Drop for Restore<'_, T> {
    fn drop(&mut self) {
        self.cell.set(self.outer);
    }
}
