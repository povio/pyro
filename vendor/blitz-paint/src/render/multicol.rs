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

    /// One column rule in `rect` (device px, the element's space).
    fn draw_rule(&self, scene: &mut impl PaintScene, rect: Rect, style: BorderStyle, color: peniko::Color) {
        let t = rect.width();
        match style {
            BorderStyle::Double if t >= 3.0 => {
                let line = (t / 3.0).round().max(1.0);
                for r in [
                    Rect::new(rect.x0, rect.y0, rect.x0 + line, rect.y1),
                    Rect::new(rect.x1 - line, rect.y0, rect.x1, rect.y1),
                ] {
                    scene.fill(Fill::NonZero, self.transform, color, None, &r);
                }
            }
            BorderStyle::Dashed | BorderStyle::Dotted => {
                // Dashes three times as long as the rule is wide (dots as long), gaps alike, the
                // pattern stretched to end on a dash.
                let dash = if style == BorderStyle::Dashed { 3.0 * t } else { t };
                let length = rect.height();
                let count = ((length + dash) / (2.0 * dash)).floor().max(1.0);
                let pitch = if count > 1.0 { (length - dash) / (count - 1.0) } else { 0.0 };
                for i in 0..count as usize {
                    let y = rect.y0 + i as f64 * pitch;
                    let piece = Rect::new(rect.x0, y, rect.x1, (y + dash).min(rect.y1));
                    if style == BorderStyle::Dotted {
                        scene.fill(Fill::NonZero, self.transform, color, None, &kurbo::Ellipse::from_rect(piece));
                    } else {
                        scene.fill(Fill::NonZero, self.transform, color, None, &piece);
                    }
                }
            }
            _ => scene.fill(Fill::NonZero, self.transform, color, None, &rect),
        }
    }
}
