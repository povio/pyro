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
        let (columns, vertical) = match self.context.dom.column_fragments(self.node.id) {
            Some(columns) => (columns, None),
            None => match self.node.vertical_column_fragments() {
                Some((columns, rl)) => (columns, Some((rl, self.node.final_layout().size.width as f64))),
                None => return false,
            },
        };
        let base = self.transform;
        let screen = Affine::translate((-self.context.initial_x, -self.context.initial_y));
        let scale = self.scale;
        let single = columns.fragments.len() == 1;
        // pyro: the out-of-flow boxes the container is the containing block of are placed against
        // it, not in a column: painted once, after the columns (`multicol_skips`).
        let outer_pass = self.context.multicol_pass.replace(Some((self.node.id, false)));
        for fragment in &columns.fragments {
            let (start, end) = (fragment.start as f64, fragment.end as f64);
            let (offset, range) = match vertical {
                None => (
                    Vec2::new(fragment.offset.x as f64 * scale, fragment.offset.y as f64 * scale),
                    Rect::new(
                        -UNBOUNDED,
                        (start * scale).max(-UNBOUNDED),
                        UNBOUNDED,
                        (end * scale).min(UNBOUNDED),
                    ),
                ),
                Some((false, _)) => (
                    Vec2::new(fragment.offset.y as f64 * scale, fragment.offset.x as f64 * scale),
                    Rect::new(
                        (start * scale).max(-UNBOUNDED),
                        -UNBOUNDED,
                        (end * scale).min(UNBOUNDED),
                        UNBOUNDED,
                    ),
                ),
                Some((true, width)) => (
                    Vec2::new(-fragment.offset.y as f64 * scale, fragment.offset.x as f64 * scale),
                    Rect::new(
                        ((width - end) * scale).max(-UNBOUNDED),
                        -UNBOUNDED,
                        ((width - start) * scale).min(UNBOUNDED),
                        UNBOUNDED,
                    ),
                ),
            };
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
            self.draw_inline_layout(scene, pos);
            self.draw_marker(scene, pos);
            self.draw_children(scene, transform, if single { clip_rect } else { column_clip });
            self.context.layer_manager.maybe_pop_layer(scene, pushed);
        }
        self.transform = base;
        self.context.multicol_pass.set(Some((self.node.id, true)));
        self.draw_children(scene, base, clip_rect);
        self.context.multicol_pass.set(outer_pass);
        true
    }

    /// The column rules of a multi-column container (`column-rule`): one in the middle of the
    /// gap between each two neighbouring columns of a row, as tall as the columns, snapped to
    /// whole pixels.
    pub(super) fn draw_column_rules(&self, scene: &mut impl PaintScene) {
        let Some(columns) = self.context.dom.column_fragments(self.node.id) else {
            return;
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
        for pair in columns.fragments.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if a.spanner || b.spanner || (a.top - b.top).abs() > 0.5 {
                continue;
            }
            let (left, right) = if a.x <= b.x { (a, b) } else { (b, a) };
            let center = ((left.x + left.width) as f64 + right.x as f64) / 2.0 * scale;
            let x0 = (center - thickness / 2.0).round();
            let y0 = (a.top as f64 * scale).round();
            let y1 = ((a.top + a.height) as f64 * scale).round();
            if y1 <= y0 {
                continue;
            }
            let rect = Rect::new(x0, y0, x0 + thickness, y1);
            self.draw_rule(scene, rect, style, color);
        }
    }

    /// One column rule in `rect` (device px, the element's space), painted as Chromium paints
    /// it: as the left side of a box (`BoxBorderPainter::DrawLineForBoxSide`), its style the one
    /// a collapsed table border would have (`inset` as `ridge`, `outset` as `groove`).
    fn draw_rule(&self, scene: &mut impl PaintScene, rect: Rect, style: BorderStyle, color: peniko::Color) {
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
                );
                scene.fill(Fill::NonZero, self.transform, color, None, &marks);
            }
            _ => fill(scene, color, &rect),
        }
    }
}
