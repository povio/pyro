use anyrender::PaintScene;
use blitz_dom::CopyStyleGetters;
use blitz_dom::node::SpecialElementData;
use kurbo::{BezPath, Cap, Circle, Insets, Join, Point, Rect, Shape as _, Stroke, Vec2};
use peniko::{Color, Fill};
use smallvec::SmallVec;
use style::{
    computed_values::border_collapse::T as BorderCollapse,
    values::computed::{BorderStyle, OutlineStyle},
};

use crate::{
    color::{ToColorColor as _, contrast_ratio},
    kurbo_css::{CssBox, Edge, NonUniformRoundedRectRadii},
    render::{ElementCx, PhysicalTracks},
};

/// A darker version of a colour (mirrors WebKit/Blink's `Color::Dark`): the
/// brightest channel is reduced by a fixed amount and the others scaled to match,
/// preserving hue.
fn darken(color: Color) -> Color {
    let [r, g, b, a] = color.components;
    let v = r.max(g).max(b);
    if v == 0.0 {
        return color;
    }
    let multiplier = ((v - 0.33) / v).max(0.0);
    Color::new([r * multiplier, g * multiplier, b * multiplier, a])
}

/// A lighter version of a colour (mirrors WebKit/Blink's `Color::Light`): the
/// brightest channel is increased by a fixed amount and the others scaled to
/// match, preserving hue.
fn lighten(color: Color) -> Color {
    let [r, g, b, a] = color.components;
    let v = r.max(g).max(b);
    if v == 0.0 {
        // Pure black: WebKit uses a fixed dark grey (0x545454).
        return Color::new([0.33, 0.33, 0.33, a]);
    }
    let multiplier = (v + 0.33).min(1.0) / v;
    Color::new([r * multiplier, g * multiplier, b * multiplier, a])
}

/// The colour of a single edge of an `inset`/`outset` border.
///
/// An `outset` border is raised: the top/left edges use the "lighter" shade and
/// the bottom/right edges the "darker" shade (`inset` is the reverse). The exact
/// shading matches Chrome's `CalculateInsetOutsetColor`:
///
/// * The darker edge is always darkened.
/// * The lighter edge keeps the base colour unchanged, *except* for colours dark
///   enough that the darkened edge wouldn't have enough contrast against the base
///   colour, which are lightened instead so the bevel stays visible.
fn beveled_edge_color(color: Color, edge: Edge, inset: bool) -> Color {
    let top_or_left = matches!(edge, Edge::Top | Edge::Left);
    let should_darken = top_or_left == inset;

    let dark_color = darken(color);
    if should_darken {
        return dark_color;
    }

    // Lighter edge. Chrome uses the base colour as-is unless the colour is dark
    // enough that the darkened edge lacks contrast against it. The red/green
    // shortcut mirrors Chrome: for those colours the contrast is always
    // sufficient, so the base colour is used without a contrast check.
    let [r, g, _, _] = color.components;
    if r >= 150.0 / 255.0 || g >= 92.0 / 255.0 {
        return color;
    }

    // Lighten only when the darkened edge would be too low-contrast to read.
    const MIN_CONTRAST_RATIO: f32 = 1.75;
    if contrast_ratio(color, dark_color) < MIN_CONTRAST_RATIO {
        lighten(color)
    } else {
        color
    }
}

/// The (outer half, inner half) colours of a single edge of a `groove`/`ridge`
/// border.
///
/// A `groove` looks carved into the page: its outer half is shaded like `inset`
/// and its inner half like `outset`. `ridge` is the reverse (raised).
fn grooved_edge_colors(color: Color, edge: Edge, ridge: bool) -> (Color, Color) {
    let outer = beveled_edge_color(color, edge, !ridge);
    let inner = beveled_edge_color(color, edge, ridge);
    (outer, inner)
}

/// A box side as the dotted and dashed painting sees it: its style and colour,
/// or `None` for a side without a line.
type Side = Option<(BorderStyle, Color)>;

/// The sides in the order [`Side`] arrays hold them, clockwise from the top.
const SIDES: [Edge; 4] = [Edge::Top, Edge::Right, Edge::Bottom, Edge::Left];

/// Whether Blink draws a dotted or dashed line of this width (CSS px) as dashes
/// (`StyledStrokeData::StrokeIsDashed`): a dashed line always, and a dotted one
/// up to 3px wide, whose dots are then squares. A wider dotted line has round
/// dots.
fn stroke_is_dashed(width: f64, style: BorderStyle) -> bool {
    style == BorderStyle::Dashed || width <= 3.0
}

/// Blink's `SelectBestDashGap`: the gap that fits a whole number of dashes of
/// length `dash` in `length`, the one of the two nearest counts whose gap is
/// closer to `gap`. An open line starts and ends with a dash; a closed one has
/// as many gaps as dashes.
fn select_best_dash_gap(length: f64, dash: f64, gap: f64, closed: bool) -> f64 {
    let available = if closed { length } else { length + gap };
    let min_dashes = (available / (dash + gap)).floor();
    let max_dashes = min_dashes + 1.0;
    let (min_gaps, max_gaps) = if closed {
        (min_dashes, max_dashes)
    } else {
        (min_dashes - 1.0, max_dashes - 1.0)
    };
    let min_gap = (length - min_dashes * dash) / min_gaps;
    let max_gap = (length - max_dashes * dash) / max_gaps;
    if max_gap <= 0.0 || (min_gap - gap).abs() < (max_gap - gap).abs() {
        min_gap
    } else {
        max_gap
    }
}

/// The `(dash, gap)` of a line Blink draws as dashes (`stroke_is_dashed`), in
/// CSS px, or `None` for a line too short for dashes, which is solid
/// (`StyledStrokeData::SetupPaintDashPathEffect`).
///
/// A dash is twice the width of a line of 3px or more and three times that of
/// a thinner one, the gap one width and two; the gap is then adjusted to the
/// length. A thin dotted line has dots and gaps of exactly its width.
fn dash_pattern(length: f64, width: f64, style: BorderStyle, closed: bool) -> Option<(f64, f64)> {
    let dashed = style == BorderStyle::Dashed;
    let (dash, gap) = match (dashed, width >= 3.0) {
        (false, _) => (width, width),
        (true, true) => (width * 2.0, width),
        (true, false) => (width * 3.0, width * 2.0),
    };
    if length <= dash * 2.0 {
        return None;
    }
    let two_dashes = 2.0 * dash + gap + if closed { gap } else { 0.0 };
    if length <= two_dashes {
        // Exactly two dashes, sized in proportion.
        let multiplier = length / two_dashes;
        return Some((dash * multiplier, gap * multiplier));
    }
    let gap = if dashed {
        select_best_dash_gap(length, dash, gap, closed)
    } else {
        gap
    };
    Some((dash, gap))
}

/// The distance between the centres of the round dots of a wide dotted line, in
/// CSS px (`StyledStrokeData::SetupPaintDashPathEffect`). Blink shortens it by
/// an epsilon so that rounding does not drop the last dot.
fn dot_period(length: f64, width: f64, closed: bool) -> f64 {
    if length < width * 2.0 {
        // No room for two dots.
        return width * 2.0;
    }
    select_best_dash_gap(length, width, width, closed) + width - 0.01
}

/// The marks of one dotted or dashed straight side, appended to `path`, as
/// Blink's `GraphicsContext::DrawLine` places them.
///
/// The side runs from `origin` (an outer corner of the box) for `length` along
/// `along` and is `thickness` deep along `across` (unit vectors; device px).
/// Blink works in whole CSS px, so the pattern is laid out in them and then
/// stretched over the side.
///
/// `dots` holds the round dots already in `path`: two sides alike each have a
/// dot in the corner they share, and the same dot twice in one fill would come
/// out bolder than the others (the coverage of its edge adds up).
fn straight_side_marks(
    path: &mut BezPath,
    dots: &mut Vec<(Point, f64)>,
    style: BorderStyle,
    origin: Point,
    (along, across): (Vec2, Vec2),
    (length, thickness): (f64, f64),
    scale: f64,
) {
    let width = (thickness / scale).round().max(1.0);
    let len = (length / scale).round();
    if len <= 0.0 {
        return;
    }
    let unit = length / len;
    let at = |a: f64, depth: f64| origin + along * (a * unit) + across * depth;
    let mut rect = |from: f64, to: f64| {
        if to > from {
            path.extend(Rect::from_points(at(from, 0.0), at(to, thickness)).path_elements(0.1));
        }
    };

    if !stroke_is_dashed(width, style) {
        // Round dots, the first and the last touching the ends of the side.
        let radius = thickness / 2.0;
        let mut dot = |a: f64| {
            let center = at(a, radius);
            let same = |&(c, r): &(Point, f64)| r == radius && (c - center).hypot() < 0.25 * scale;
            if !dots.iter().any(same) {
                dots.push((center, radius));
                path.extend(Circle::new(center, radius).path_elements(0.1));
            }
        };
        if len <= width {
            dot(len / 2.0);
            return;
        }
        let period = dot_period(len, width, false);
        let mut distance = 0.0;
        while distance < len - width {
            dot(width / 2.0 + distance);
            distance += period;
        }
        return;
    }

    let (mut start, mut end) = (0.0, len);
    if style == BorderStyle::Dotted {
        // `EnforceDotsAtEndpoints`: dots and gaps of exactly the width leave a
        // gap or a partial dot at the end unless the length is an odd multiple
        // of the width. Blink then draws the first dot, and sometimes the last
        // one, itself, a pixel longer or with a pixel more or less of gap.
        let (mod_2, mod_4, mod_6) = (len % 2.0, len % 4.0, len % 6.0);
        let (mut start_dot, mut start_growth, mut start_offset) = (false, 0.0, 0.0);
        let (mut end_dot, mut end_growth) = (false, 0.0);
        if (width == 1.0 && mod_2 == 0.0) || (width == 3.0 && mod_6 == 0.0) {
            (start_dot, start_growth, start_offset) = (true, 1.0, 1.0);
        }
        if (width == 2.0 && (mod_4 == 0.0 || mod_4 == 1.0))
            || (width == 3.0 && (mod_6 == 1.0 || mod_6 == 2.0))
        {
            (start_dot, start_offset) = (true, -1.0);
        }
        if (width == 2.0 && mod_4 == 0.0) || (width == 3.0 && mod_6 == 1.0) {
            end_dot = true;
        }
        if (width == 2.0 && mod_4 == 3.0) || (width == 3.0 && (mod_6 == 4.0 || mod_6 == 5.0)) {
            (start_dot, start_offset) = (true, 1.0);
        }
        if width == 3.0 && mod_6 == 5.0 {
            end_dot = true;
        } else if width == 3.0 && mod_6 == 0.0 {
            (end_dot, end_growth) = (true, 1.0);
        }
        if start_dot {
            rect(0.0, width + start_growth);
            start = 2.0 * width + start_offset;
        }
        if end_dot {
            rect(len - width - end_growth, len);
            end = len - (width + end_growth + 1.0);
        }
    }
    match dash_pattern(len, width, style, false) {
        None => rect(start, end),
        Some((dash, gap)) => {
            let mut from = start;
            while from < end {
                rect(from, (from + dash).min(end));
                from += dash + gap;
            }
        }
    }
}

/// A quarter-ellipse corner as Skia has it: a conic from the first point to the
/// last, its control point (the second) in the corner, of weight √½.
fn corner_point(points: &[Point; 3], t: f64) -> Point {
    let (a, b, c) = (
        (1.0 - t) * (1.0 - t),
        2.0 * std::f64::consts::FRAC_1_SQRT_2 * t * (1.0 - t),
        t * t,
    );
    let sum = points[0].to_vec2() * a + points[1].to_vec2() * b + points[2].to_vec2() * c;
    (sum / (a + b + c)).to_point()
}

/// The chords Skia's `SkContourMeasure` measures a corner by: it halves the
/// curve until no chord's middle is further than `tolerance` from the curve, on
/// either axis. Each chord as the parameter and the distance along the chords
/// at its end; the last distance is the length Skia gives the corner.
fn skia_corner_chords(points: &[Point; 3], tolerance: f64) -> Vec<(f64, f64)> {
    fn halve(
        points: &[Point; 3],
        (t0, p0): (f64, Point),
        (t1, p1): (f64, Point),
        tolerance: f64,
        depth: u32,
        chords: &mut Vec<(f64, f64)>,
    ) {
        let half_t = (t0 + t1) / 2.0;
        let half = corner_point(points, half_t);
        let off = half - p0.midpoint(p1);
        if depth < 20 && off.x.abs().max(off.y.abs()) > tolerance {
            halve(points, (t0, p0), (half_t, half), tolerance, depth + 1, chords);
            halve(points, (half_t, half), (t1, p1), tolerance, depth + 1, chords);
        } else {
            let distance = chords.last().map_or(0.0, |&(_, distance)| distance);
            chords.push((t1, distance + (p1 - p0).hypot()));
        }
    }
    let mut chords = Vec::new();
    halve(points, (0.0, points[0]), (1.0, points[2]), tolerance, 0, &mut chords);
    chords
}

/// A part of a [`CenterLine`].
enum LinePart {
    Straight(Point, Point),
    Corner {
        points: [Point; 3],
        center: Point,
        radii: Vec2,
        /// The angle of the first point, on the ellipse around `center`.
        start_angle: f64,
        chords: Vec<(f64, f64)>,
    },
}

/// The line Blink strokes for the dotted and dashed sides of a box with rounded
/// corners, measured as Skia measures it.
///
/// The line is the box's outer edge inset by half of each side's width in whole
/// CSS px (so that of a 1px side is the outer edge itself), each radius less
/// the insets. It starts where the top side leaves the top left corner and runs
/// clockwise, as Skia's rounded rectangle does.
///
/// Skia takes a corner for the chords that are within half a pixel of it, and a
/// distance along the corner for the same share of the curve's parameter as of
/// its chord. The dashes are laid out by these distances in device px, so they
/// are a little longer in the corners; Blink sizes them by the length in CSS
/// px, whose chords are fewer, cut to a whole number.
struct CenterLine {
    /// The parts, each with the distance at which it starts and its length.
    parts: Vec<(f64, f64, LinePart)>,
    /// The length in device px.
    length: f64,
    /// The length Blink sizes the pattern by, in CSS px.
    blink_length: f64,
    /// The whole closed line.
    path: BezPath,
}

impl CenterLine {
    fn new(frame: &CssBox, scale: f64) -> Self {
        let half = |width: f64| ((width / scale).round() / 2.0).floor() * scale;
        let widths = frame.border_width;
        let (left, top, right, bottom) = (half(widths.x0), half(widths.y0), half(widths.x1), half(widths.y1));
        let bb = frame.border_box;
        let rect = Rect::new(bb.x0 + left, bb.y0 + top, bb.x1 - right, bb.y1 - bottom);
        let radius = |r: Vec2, inset_x: f64, inset_y: f64| {
            let r = Vec2::new((r.x - inset_x).max(0.0), (r.y - inset_y).max(0.0));
            if r.x == 0.0 || r.y == 0.0 { Vec2::ZERO } else { r }
        };
        let radii = frame.border_radii;
        let tl = radius(radii.top_left, left, top);
        let tr = radius(radii.top_right, right, top);
        let br = radius(radii.bottom_right, right, bottom);
        let bl = radius(radii.bottom_left, left, bottom);
        // Each corner by the points where it starts and ends, its corner point
        // and its radii, in the order of the line.
        let corners = [
            (Point::new(rect.x1 - tr.x, rect.y0), Point::new(rect.x1, rect.y0 + tr.y), Point::new(rect.x1, rect.y0), tr),
            (Point::new(rect.x1, rect.y1 - br.y), Point::new(rect.x1 - br.x, rect.y1), Point::new(rect.x1, rect.y1), br),
            (Point::new(rect.x0 + bl.x, rect.y1), Point::new(rect.x0, rect.y1 - bl.y), Point::new(rect.x0, rect.y1), bl),
            (Point::new(rect.x0, rect.y0 + tl.y), Point::new(rect.x0 + tl.x, rect.y0), Point::new(rect.x0, rect.y0), tl),
        ];

        let mut line = Self { parts: Vec::new(), length: 0.0, blink_length: 0.0, path: BezPath::new() };
        let mut pen = Point::new(rect.x0 + tl.x, rect.y0);
        line.path.move_to(pen);
        for (i, (from, to, corner, radii)) in corners.into_iter().enumerate() {
            let straight = (from - pen).hypot();
            if straight > 0.0 {
                line.parts.push((line.length, straight, LinePart::Straight(pen, from)));
                line.length += straight;
                line.blink_length += straight;
                line.path.line_to(from);
            }
            if radii != Vec2::ZERO {
                let points = [from, corner, to];
                let center = Point::new(from.x + to.x - corner.x, from.y + to.y - corner.y);
                let start_angle = (i as f64 - 1.0) * std::f64::consts::FRAC_PI_2;
                let arc = kurbo::Arc::new(center, radii, start_angle, std::f64::consts::FRAC_PI_2, 0.0);
                line.path.extend(arc.append_iter(0.1));
                let chords = skia_corner_chords(&points, 0.5);
                let length = chords.last().map_or(0.0, |&(_, distance)| distance);
                line.blink_length += skia_corner_chords(&points, 0.5 * scale).last().map_or(0.0, |&(_, d)| d);
                line.parts.push((line.length, length, LinePart::Corner { points, center, radii, start_angle, chords }));
                line.length += length;
            }
            pen = to;
        }
        line.path.close_path();
        line.blink_length = (line.blink_length / scale).floor();
        line
    }

    /// The parameter of a corner at `distance` from its start.
    fn corner_t(chords: &[(f64, f64)], distance: f64) -> f64 {
        let (mut t0, mut d0) = (0.0, 0.0);
        for &(t1, d1) in chords {
            if distance <= d1 && d1 > d0 {
                return t0 + (t1 - t0) * ((distance - d0) / (d1 - d0)).clamp(0.0, 1.0);
            }
            (t0, d0) = (t1, d1);
        }
        1.0
    }

    /// The point at `distance` along the line.
    fn point_at(&self, distance: f64) -> Point {
        let part = self.parts.iter().rev().find(|(start, ..)| *start <= distance).or(self.parts.first());
        part.map_or(Point::ZERO, |(start, length, part)| Self::point_in(part, distance - start, *length))
    }

    /// The point at `distance` along a part of the given length.
    fn point_in(part: &LinePart, distance: f64, length: f64) -> Point {
        match part {
            LinePart::Straight(from, to) => from.lerp(*to, (distance / length).clamp(0.0, 1.0)),
            LinePart::Corner { points, chords, .. } => corner_point(points, Self::corner_t(chords, distance)),
        }
    }

    /// Append the line between two distances to `path`, as a contour of its own.
    fn append_range(&self, path: &mut BezPath, from: f64, to: f64) {
        let mut started = false;
        for (start, length, part) in &self.parts {
            let (a, b) = (from.max(*start), to.min(start + length));
            if b <= a {
                continue;
            }
            let (p0, p1) = (Self::point_in(part, a - start, *length), Self::point_in(part, b - start, *length));
            if !started {
                path.move_to(p0);
                started = true;
            }
            match part {
                LinePart::Straight(..) => path.line_to(p1),
                LinePart::Corner { center, radii, start_angle, .. } => {
                    // The angle of a point of this quarter, from its start.
                    let angle = |p: Point| {
                        let angle = ((p.y - center.y) / radii.y).atan2((p.x - center.x) / radii.x) - start_angle;
                        let angle = angle.rem_euclid(std::f64::consts::TAU);
                        if angle > std::f64::consts::PI { 0.0 } else { angle.min(std::f64::consts::FRAC_PI_2) }
                    };
                    let (a0, a1) = (angle(p0), angle(p1));
                    let arc = kurbo::Arc::new(*center, *radii, start_angle + a0, a1 - a0, 0.0);
                    path.extend(arc.append_iter(0.1));
                }
            }
        }
    }
}

impl ElementCx<'_, '_> {
    /// Draw all borders for a node
    pub(crate) fn draw_border(&self, scene: &mut impl PaintScene) {
        // A table without uniform collapsed borders draws them all (`draw_collapsed_edges`).
        // pyro: a collapsed table with cells draws its outer borders as the collapsed ones that
        // won there, never its own (`draw_table_borders`).
        if self.node.layout_data().collapsed_border.is_some()
            || matches!(&self.element.special_data, SpecialElementData::TableRoot(table)
                if table.collapsed_edges.is_some()
                    || (table.border_collapse == BorderCollapse::Collapse && !table.cells.is_empty()))
        {
            return;
        }
        // pyro: a border image is drawn instead of the border's styles (Blink's
        // `BoxPainterBase::PaintBorder`); one that cannot be drawn leaves them.
        if self.draw_border_image(scene) {
            return;
        }

        let style = &*self.style;
        let border = style.get_border();
        let current_color = style.clone_color();

        // Fast path: a uniform-width, single-color solid border whose corners are
        // all circular arcs (equal x and y radius — sharp rectangles, ordinary
        // rounded rectangles and circles all qualify) can be drawn as a single
        // stroke along its center-line instead of a fill per edge. Besides being
        // faster, this works around Vello seam artifacts at the quarter-arc joins
        // when a thin circular/elliptical border is drawn as the
        // border-box/padding-box annulus.
        //
        // A full ellipse (`border-radius: 50%` on a non-square box) has the same
        // seam problem but no rounded-rect representation, so it is stroked as an
        // ellipse.
        let all_solid = border.border_top_style == BorderStyle::Solid
            && border.border_right_style == BorderStyle::Solid
            && border.border_bottom_style == BorderStyle::Solid
            && border.border_left_style == BorderStyle::Solid;
        if all_solid {
            let color = border
                .border_top_color
                .resolve_to_absolute(&current_color)
                .as_srgb_color();
            let same_color = [
                &border.border_right_color,
                &border.border_bottom_color,
                &border.border_left_color,
            ]
            .iter()
            .all(|c| c.resolve_to_absolute(&current_color).as_srgb_color() == color);
            let bw = self.frame.border_width;
            let uniform_width = bw.x0 == bw.y0 && bw.x0 == bw.x1 && bw.x0 == bw.y1 && bw.x0 > 0.0;
            if same_color && uniform_width && color.components[3] > 0.0 {
                let bb = self.frame.border_box;
                let pb = self.frame.padding_box;
                let width = (bb.width() - pb.width()) / 2.0;
                // Miter join keeps sharp corners square. It has no effect on rounded
                // corners, whose arcs meet the straight edges tangentially.
                let stroke = Stroke::new(width).with_join(Join::Miter);

                if self.frame.is_uniform_corner_border() {
                    let radii = self.frame.border_radii;
                    // The stroke matches the CSS border shape only when each corner is
                    // sharp or its radius is at least the border width (outer radius
                    // `r`, inner radius `r - width`, concentric). For 0 < r < width CSS
                    // draws a rounded outer edge against a sharp inner corner, which a
                    // stroke can't represent — leave those to the per-edge fill.
                    // pyro: and kurbo's `RoundedRect` keeps the radius: it clamps each to half
                    // the rect's smaller side, which a CSS corner beside a square one exceeds
                    // (`border-radius: 100% 0 0 0`).
                    let half_side = (bb.width().min(bb.height()) - width) / 2.0;
                    let is_exact = |r: Vec2| r.x == 0.0 || (r.x >= width && r.x - width / 2.0 <= half_side);
                    let stroke_is_exact = is_exact(radii.top_left)
                        && is_exact(radii.top_right)
                        && is_exact(radii.bottom_right)
                        && is_exact(radii.bottom_left);
                    if stroke_is_exact {
                        let half_width = width / 2.0;
                        // Center-line rect, and corner radii shrunk to match.
                        let centerline_box = bb - Insets::uniform(half_width);
                        let centerline_radius = |r: Vec2| (r.x - half_width).max(0.0);
                        let radii = kurbo::RoundedRectRadii::new(
                            centerline_radius(radii.top_left),
                            centerline_radius(radii.top_right),
                            centerline_radius(radii.bottom_right),
                            centerline_radius(radii.bottom_left),
                        );
                        let rr = kurbo::RoundedRect::from_rect(centerline_box, radii);
                        scene.stroke(&stroke, self.transform, color, None, &rr);
                        return;
                    }
                } else if self.frame.is_elliptical_border() {
                    // Center-line radii, straight from the boxes.
                    let rx = (bb.width() + pb.width()) / 4.0;
                    let ry = (bb.height() + pb.height()) / 4.0;
                    let e = kurbo::Ellipse::new(bb.center(), (rx, ry), 0.0);
                    scene.stroke(&stroke, self.transform, color, None, &e);
                    return;
                }
            }
        }

        // (colour, path) pairs to be filled. Several entries may share a colour;
        // they are grouped before filling so that adjacent same-coloured regions
        // are drawn together, avoiding anti-aliasing seams between them.
        //
        // At most 8 entries: 4 edges, each of which can contribute 2 (the outer
        // and inner halves of a `groove`/`ridge`).
        let mut borders: SmallVec<[(Color, BezPath); 8]> = SmallVec::new();
        let mut has_marks = false;

        for &edge in &[Edge::Top, Edge::Right, Edge::Bottom, Edge::Left] {
            let (color, edge_style) = match edge {
                Edge::Top => (&border.border_top_color, border.border_top_style),
                Edge::Right => (&border.border_right_color, border.border_right_style),
                Edge::Bottom => (&border.border_bottom_color, border.border_bottom_style),
                Edge::Left => (&border.border_left_color, border.border_left_style),
            };
            let color = color.resolve_to_absolute(&current_color).as_srgb_color();

            // A zero-width edge (Tailwind's preflight sets `border: 0 solid` on
            // every element) covers nothing, but its degenerate polygon is still
            // painted as a hairline by PDF rasterizers, which fill every pixel a
            // shape touches.
            if color.components[3] <= 0.0 || self.edge_width(edge) <= 0.0 {
                continue;
            }

            match edge_style {
                // `none`/`hidden` produce a zero-width border during layout, but
                // guard against drawing them anyway.
                BorderStyle::None | BorderStyle::Hidden => {}

                // Dashed and dotted edges are drawn together below, as their own
                // shapes rather than being batched with the solid edges.
                BorderStyle::Dotted | BorderStyle::Dashed => has_marks = true,

                // A double border is two solid lines separated by a gap, splitting
                // the border width into three equal parts (outer line / gap / inner
                // line). Both lines share a color, so both rings are placed into a
                // single path.
                BorderStyle::Double => {
                    // Needs at least 3px (one device pixel per line and per gap) to
                    // render as two lines; thinner borders fall back to a solid
                    // fill, matching browser behaviour.
                    let path = if self.edge_width(edge) < 3.0 * self.scale {
                        self.frame.border_edge_shape(edge)
                    } else {
                        let mut path = self
                            .frame
                            .border_slice(0.0, 1.0 / 3.0)
                            .border_edge_shape(edge);
                        path.extend(
                            &self
                                .frame
                                .border_slice(2.0 / 3.0, 1.0)
                                .border_edge_shape(edge),
                        );
                        path
                    };
                    borders.push((color, path));
                }

                // `inset`/`outset` are solid, but each edge is shaded lighter or
                // darker to give a bevelled (3D) appearance.
                BorderStyle::Inset | BorderStyle::Outset => {
                    let inset = edge_style == BorderStyle::Inset;
                    let shade = beveled_edge_color(color, edge, inset);
                    borders.push((shade, self.frame.border_edge_shape(edge)));
                }

                // `groove`/`ridge` split each edge into an outer and an inner half,
                // each shaded as if it were `inset`/`outset`, producing a carved or
                // raised ridge.
                BorderStyle::Groove | BorderStyle::Ridge => {
                    let ridge = edge_style == BorderStyle::Ridge;
                    let (outer, inner) = grooved_edge_colors(color, edge, ridge);
                    borders.push((
                        outer,
                        self.frame.border_slice(0.0, 0.5).border_edge_shape(edge),
                    ));
                    borders.push((
                        inner,
                        self.frame.border_slice(0.5, 1.0).border_edge_shape(edge),
                    ));
                }

                // Solid fills the whole edge region with the border colour.
                BorderStyle::Solid => {
                    borders.push((color, self.frame.border_edge_shape(edge)));
                }
            }
        }

        if has_marks {
            let side = |color: &style::values::computed::Color, style: BorderStyle, edge: Edge| {
                (!style.none_or_hidden() && self.edge_width(edge) > 0.0)
                    .then(|| (style, color.resolve_to_absolute(&current_color).as_srgb_color()))
            };
            let sides = [
                side(&border.border_top_color, border.border_top_style, Edge::Top),
                side(&border.border_right_color, border.border_right_style, Edge::Right),
                side(&border.border_bottom_color, border.border_bottom_style, Edge::Bottom),
                side(&border.border_left_color, border.border_left_style, Edge::Left),
            ];
            self.draw_dotted_dashed_sides(scene, &self.frame, sides);
        }

        if borders.is_empty() {
            return;
        }

        // Group together identical colors by sorting, then fill each group as a
        // single path.
        borders.sort_unstable_by(|a, b| {
            a.0.components
                .partial_cmp(&b.0.components)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut start = 0;
        while start < borders.len() {
            let color = borders[start].0;
            let mut path = std::mem::take(&mut borders[start].1);
            let mut next = start + 1;
            while next < borders.len() && borders[next].0 == color {
                path.extend(&borders[next].1);
                next += 1;
            }
            scene.fill(Fill::NonZero, self.transform, color, None, &path);
            start = next;
        }
    }

    /// pyro: the area this element's border paints, which `background-clip: border-area` (CSS
    /// Backgrounds 4) clips to: each side with a style and a width, all of it whatever its
    /// colour; a `double` side its two lines; a dotted or dashed side its marks when the corners
    /// are square (with rounded corners, the whole side).
    pub(super) fn border_area_path(&self) -> BezPath {
        let border = self.style.get_border();
        let sides = [
            (Edge::Top, border.border_top_style),
            (Edge::Right, border.border_right_style),
            (Edge::Bottom, border.border_bottom_style),
            (Edge::Left, border.border_left_style),
        ];
        let bb = self.frame.border_box;
        let mut path = BezPath::new();
        // The round dots already in `path`: two dotted sides share their corner's dot, which
        // must be in the path once (twice, its edge came out bolder).
        let mut dots = Vec::new();
        for (edge, style) in sides {
            let width = self.edge_width(edge);
            if style.none_or_hidden() || width <= 0.0 {
                continue;
            }
            match style {
                BorderStyle::Double if width >= 3.0 * self.scale => {
                    path.extend(&self.frame.border_slice(0.0, 1.0 / 3.0).border_edge_shape(edge));
                    path.extend(&self.frame.border_slice(2.0 / 3.0, 1.0).border_edge_shape(edge));
                }
                BorderStyle::Dotted | BorderStyle::Dashed if !self.frame.has_border_radius() => {
                    let (origin, along, across, length) = match edge {
                        Edge::Top => (Point::new(bb.x0, bb.y0), Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0), bb.width()),
                        Edge::Bottom => (Point::new(bb.x0, bb.y1), Vec2::new(1.0, 0.0), Vec2::new(0.0, -1.0), bb.width()),
                        Edge::Left => (Point::new(bb.x0, bb.y0), Vec2::new(0.0, 1.0), Vec2::new(1.0, 0.0), bb.height()),
                        Edge::Right => (Point::new(bb.x1, bb.y0), Vec2::new(0.0, 1.0), Vec2::new(-1.0, 0.0), bb.height()),
                    };
                    straight_side_marks(&mut path, &mut dots, style, origin, (along, across), (length, width), self.scale);
                }
                _ => path.extend(&self.frame.border_edge_shape(edge)),
            }
        }
        path
    }

    /// The border width (in device pixels) of a single edge.
    fn edge_width(&self, edge: Edge) -> f64 {
        match edge {
            Edge::Top => self.frame.border_width.y0,
            Edge::Bottom => self.frame.border_width.y1,
            Edge::Left => self.frame.border_width.x0,
            Edge::Right => self.frame.border_width.x1,
        }
    }

    /// Draw the dotted and dashed sides of `frame`'s border, which is this
    /// element's border or its outline, with Blink's geometry.
    ///
    /// On a box with square corners each side is a straight line from corner to
    /// corner over the whole side, so sides of one style and colour overlap in
    /// their corner (and a side is mitred only against a neighbour of another
    /// style or colour). On a box with rounded corners every side strokes one
    /// line around the whole box, clipped to the side.
    fn draw_dotted_dashed_sides(&self, scene: &mut impl PaintScene, frame: &CssBox, sides: [Side; 4]) {
        let scale = self.scale;
        let thickness = |edge: Edge| match edge {
            Edge::Top => frame.border_width.y0,
            Edge::Right => frame.border_width.x1,
            Edge::Bottom => frame.border_width.y1,
            Edge::Left => frame.border_width.x0,
        };
        let marked = |side: Side| {
            side.filter(|(style, color)| {
                matches!(style, BorderStyle::Dotted | BorderStyle::Dashed) && color.components[3] > 0.0
            })
        };
        let first_width = thickness(Edge::Top);
        let uniform = sides.iter().all(|side| *side == sides[0])
            && SIDES.iter().all(|&edge| thickness(edge) == first_width);

        if frame.has_border_radius() {
            let line = CenterLine::new(frame, scale);
            if line.blink_length <= 0.0 {
                return;
            }
            for (i, &edge) in SIDES.iter().enumerate() {
                let Some((style, color)) = marked(sides[i]) else {
                    continue;
                };
                // All four sides alike draw the same marks: once, in the ring.
                if uniform && i > 0 {
                    break;
                }
                let clip = if uniform {
                    frame.border_ring()
                } else {
                    frame.border_edge_shape(edge)
                };
                let width = (thickness(edge) / scale).round().max(1.0);
                scene.push_clip_layer(Fill::NonZero, self.transform, &clip);
                if stroke_is_dashed(width, style) {
                    // As wide as the widest of the side and its neighbours, and
                    // then some: the line is not in the middle of the side, and
                    // the clip cuts the stroke to the side.
                    let widest = thickness(edge)
                        .max(thickness(SIDES[(i + 1) % 4]))
                        .max(thickness(SIDES[(i + 3) % 4]));
                    let stroke = Stroke::new(widest * 2.2).with_caps(Cap::Butt).with_join(Join::Bevel);
                    match dash_pattern(line.blink_length, width, style, true) {
                        None => scene.stroke(&stroke, self.transform, color, None, &line.path),
                        Some((dash, gap)) => {
                            // A dash wherever the pattern starts over before
                            // the end of the line, which is longer than Blink
                            // sized the pattern for: the last dash is a sliver
                            // that runs into the first.
                            let (dash, period) = (dash * scale, (dash + gap) * scale);
                            let mut dashes = BezPath::new();
                            let mut from = 0.0;
                            while from < line.length {
                                line.append_range(&mut dashes, from, (from + dash).min(line.length));
                                from += period;
                            }
                            scene.stroke(&stroke, self.transform, color, None, &dashes);
                        }
                    }
                } else {
                    // A dot wherever the pattern starts over, as for dashes:
                    // the last dot is short of the first.
                    let period = dot_period(line.blink_length, width, true) * scale;
                    let radius = thickness(edge) / 2.0;
                    let mut dots = BezPath::new();
                    let mut at = 0.0;
                    while at < line.length {
                        dots.extend(Circle::new(line.point_at(at), radius).path_elements(0.1));
                        at += period;
                    }
                    scene.fill(Fill::NonZero, self.transform, color, None, &dots);
                }
                scene.pop_layer();
            }
            return;
        }

        let bb = frame.border_box;
        // The sides that need no clip, by colour, each colour filled at once so
        // that the marks of two sides in a corner are not blended twice.
        let mut open: SmallVec<[(Color, BezPath, Vec<(Point, f64)>); 4]> = SmallVec::new();
        for (i, &edge) in SIDES.iter().enumerate() {
            let Some((style, color)) = marked(sides[i]) else {
                continue;
            };
            let (origin, along, across, length) = match edge {
                Edge::Top => (Point::new(bb.x0, bb.y0), Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0), bb.width()),
                Edge::Bottom => (Point::new(bb.x0, bb.y1), Vec2::new(1.0, 0.0), Vec2::new(0.0, -1.0), bb.width()),
                Edge::Left => (Point::new(bb.x0, bb.y0), Vec2::new(0.0, 1.0), Vec2::new(1.0, 0.0), bb.height()),
                Edge::Right => (Point::new(bb.x1, bb.y0), Vec2::new(0.0, 1.0), Vec2::new(-1.0, 0.0), bb.height()),
            };
            let side = ((along, across), (length, thickness(edge)));
            // A neighbour of another style or colour meets this side in a
            // mitre; one alike is drawn over, and a missing one leaves the
            // corner to this side.
            let mitred = |neighbour: Side| neighbour.is_some_and(|other| other != (style, color));
            let (before, after) = (SIDES[(i + 3) % 4], SIDES[(i + 1) % 4]);
            let (miter_before, miter_after) = (mitred(sides[(i + 3) % 4]), mitred(sides[(i + 1) % 4]));
            if !miter_before && !miter_after {
                let group = match open.iter().position(|(c, ..)| *c == color) {
                    Some(group) => group,
                    None => {
                        open.push((color, BezPath::new(), Vec::new()));
                        open.len() - 1
                    }
                };
                let (_, marks, dots) = &mut open[group];
                straight_side_marks(marks, dots, style, origin, side.0, side.1, scale);
                continue;
            }
            let mut marks = BezPath::new();
            straight_side_marks(&mut marks, &mut Vec::new(), style, origin, side.0, side.1, scale);
            // The side's box, an end cut back along the corner's diagonal where
            // it is mitred. Clockwise, `before` is at the start of the top and
            // right sides and at the end of the bottom and left ones.
            let depth = thickness(edge);
            let cut = |mitred: bool, neighbour: Edge| if mitred { thickness(neighbour) } else { 0.0 };
            let (cut_before, cut_after) = (cut(miter_before, before), cut(miter_after, after));
            let (cut_start, cut_end) = match edge {
                Edge::Top | Edge::Right => (cut_before, cut_after),
                Edge::Bottom | Edge::Left => (cut_after, cut_before),
            };
            let mut clip = BezPath::new();
            clip.move_to(origin);
            clip.line_to(origin + along * length);
            clip.line_to(origin + along * (length - cut_end) + across * depth);
            clip.line_to(origin + along * cut_start + across * depth);
            clip.close_path();
            scene.push_clip_layer(Fill::NonZero, self.transform, &clip);
            scene.fill(Fill::NonZero, self.transform, color, None, &marks);
            scene.pop_layer();
        }
        for (color, marks, _) in open {
            scene.fill(Fill::NonZero, self.transform, color, None, &marks);
        }
    }

    pub(crate) fn draw_table_borders(&self, scene: &mut impl PaintScene) {
        let SpecialElementData::TableRoot(table) = &self.element.special_data else {
            return;
        };
        // Borders are only handled at the table level when BorderCollapse::Collapse
        if table.border_collapse != BorderCollapse::Collapse {
            return;
        }

        let Some(grid_info) = &mut *table.computed_grid_info.borrow_mut() else {
            return;
        };
        if let Some(edges) = &table.collapsed_edges {
            self.draw_collapsed_edges(scene, edges, grid_info);
            return;
        }
        let Some(border_style) = table.border_style.as_deref() else {
            return;
        };

        let outer_border_style = self.style.get_border();

        let cols = PhysicalTracks::from_tracks(&grid_info.columns);
        let rows = PhysicalTracks::from_tracks(&grid_info.rows);

        // TODO: support different colors for different borders
        let current_color = self.style.clone_color();
        let border_color = border_style
            .border_top_color
            .resolve_to_absolute(&current_color)
            .as_srgb_color();

        // No need to draw transparent borders (as they won't be visible anyway)
        if border_color == Color::TRANSPARENT {
            return;
        }

        // Border widths are not adjusted for border-style in computed styles,
        // so a border with `none`/`hidden` style must be treated as zero-width.
        if border_style.border_top_style.none_or_hidden() {
            return;
        }

        // The borders are laid out as the gutters between the tracks, and as the border
        // of the table around them. Track positions are relative to the table's border box.
        let border_width = table.style.gap.map(|gap| gap.into_raw().value() as f64);
        // pyro: the grid sits below the captions above it.
        let grid_top = table.grid_box.borrow().map_or(0.0, |(top, _)| top as f64);
        let left = cols.origin() as f64 - border_width.width;
        let right = (cols.origin() + cols.span()) as f64 + border_width.width;
        let top = grid_top + rows.origin() as f64 - border_width.height;
        let bottom = grid_top + (rows.origin() + rows.span()) as f64 + border_width.height;

        let mut fill = |x0: f64, y0: f64, x1: f64, y1: f64| {
            let shape = Rect::new(x0, y0, x1, y1).scale_from_origin(self.scale);
            scene.fill(Fill::NonZero, self.transform, border_color, None, &shape);
        };

        // Horizontal borders: the outer edges, then the gutters between adjacent row tracks.
        // The vertical borders are drawn between them so that no corner is painted twice.
        if outer_border_style.border_top_style != BorderStyle::Hidden {
            fill(left, top, right, top + border_width.height);
        }
        if outer_border_style.border_bottom_style != BorderStyle::Hidden {
            fill(left, bottom - border_width.height, right, bottom);
        }
        for (prev, next) in rows.iter().zip(rows.iter().skip(1)) {
            fill(left, grid_top + prev.end as f64, right, grid_top + next.start as f64);
        }

        // Vertical borders, one segment per row
        for row in rows.iter() {
            let (y0, y1) = (grid_top + row.start as f64, grid_top + row.end as f64);
            if outer_border_style.border_left_style != BorderStyle::Hidden {
                fill(left, y0, left + border_width.width, y1);
            }
            if outer_border_style.border_right_style != BorderStyle::Hidden {
                fill(right - border_width.width, y0, right, y1);
            }
            for (prev, next) in cols.iter().zip(cols.iter().skip(1)) {
                fill(prev.end as f64, y0, next.start as f64, y1);
            }
        }
    }

    /// The winning borders of a table without uniform ones, centered on the grid lines. Segments of one
    /// width and color are drawn as one rectangle, so that they have no seams.
    fn draw_collapsed_edges(
        &self,
        scene: &mut impl PaintScene,
        edges: &blitz_dom::CollapsedEdges,
        grid_info: &taffy::DetailedGridInfo<style::Atom>,
    ) {
        let cols = PhysicalTracks::from_tracks(&grid_info.columns);
        let rows = PhysicalTracks::from_tracks(&grid_info.rows);
        let xs: Vec<f64> = line_positions(&cols);
        // pyro: the grid sits below the captions above it.
        let SpecialElementData::TableRoot(table) = &self.element.special_data else { return };
        let grid_top = table.grid_box.borrow().map_or(0.0, |(top, _)| top as f64);
        let ys: Vec<f64> = line_positions(&rows).into_iter().map(|y| y + grid_top).collect();
        if xs.len() != edges.columns + 1 || ys.len() != edges.rows + 1 {
            return;
        }
        let position = self.context.dom.physical_unrounded_geometry(self.node.id).0;
        let (ax, ay) = (position.x as f64, position.y as f64);
        // A rule is snapped to whole pixels of the page, as Chromium paints it: it starts at its centered
        // position, rounded, and is its width wide.
        let snap_x = |x: f64| (ax + x).round() - ax.round();
        let snap_y = |y: f64| (ay + y).round() - ay.round();
        let mut fill = |x0: f64, y0: f64, x1: f64, y1: f64, color: style::color::AbsoluteColor| {
            let shape = Rect::new(x0, y0, x1, y1).scale_from_origin(self.scale);
            scene.fill(Fill::NonZero, self.transform, color.as_srgb_color(), None, &shape);
        };
        // A rule starts and ends on the outer sides of the rules across it.
        for line in 0..=edges.rows {
            let mut c = 0;
            while c < edges.columns {
                let edge = *edges.horizontal_at(line, c);
                let mut end = c + 1;
                while end < edges.columns && same_edge(edges.horizontal_at(line, end), &edge) {
                    end += 1;
                }
                if edge.width > 0.0 {
                    let width = edge.width as f64;
                    let across = |col: usize| {
                        let up = line.checked_sub(1).map_or(0.0, |r| edges.vertical_at(r, col).width);
                        let down = (line < edges.rows).then(|| edges.vertical_at(line, col).width).unwrap_or(0.0);
                        up.max(down) as f64
                    };
                    let (a, b) = (across(c), across(end));
                    let x0 = snap_x(xs[c] - a / 2.0);
                    let x1 = snap_x(xs[end] - b / 2.0) + b;
                    let y0 = snap_y(ys[line] - width / 2.0);
                    fill(x0, y0, x1, y0 + width, edge.color);
                }
                c = end;
            }
        }
        for line in 0..=edges.columns {
            let mut r = 0;
            while r < edges.rows {
                let edge = *edges.vertical_at(r, line);
                let mut end = r + 1;
                while end < edges.rows && same_edge(edges.vertical_at(end, line), &edge) {
                    end += 1;
                }
                if edge.width > 0.0 {
                    let width = edge.width as f64;
                    let x0 = snap_x(xs[line] - width / 2.0);
                    fill(x0, snap_y(ys[r]), x0 + width, snap_y(ys[end]), edge.color);
                }
                r = end;
            }
        }
    }

    /// The outline's style, colour and width (device px), and how far its
    /// outer edge is from the border box; `None` for an element that draws no
    /// outline.
    ///
    /// Blink has the width and the offset in whole pixels, and the offset no
    /// further in than to the middle of the box.
    fn outline_line(&self) -> Option<(BorderStyle, Color, f64, f64)> {
        let outline = self.style.get_outline();
        let style = match outline.outline_style {
            OutlineStyle::Auto => return None,
            OutlineStyle::BorderStyle(style) => style,
        };
        let color = outline
            .outline_color
            .resolve_to_absolute(&self.style.clone_color())
            .as_srgb_color();
        if style.none_or_hidden() || self.frame.outline_width <= 0.0 || color.components[3] <= 0.0 {
            return None;
        }
        let scale = self.scale;
        let width = (self.frame.outline_width / scale).floor().max(1.0) * scale;
        let bb = self.frame.border_box;
        let offset = (outline.outline_offset.to_f64_px().trunc() * scale)
            .max(-(bb.width() / 2.0).floor())
            .max(-(bb.height() / 2.0).floor());
        Some((style, color, width, offset + width))
    }

    /// Draw the outline: a line of the outline's width around the border box
    /// moved out by `outline-offset` (in, when negative), its corners rounded
    /// as the border's are, drawn as Blink draws a border of that style
    /// (`BoxBorderPainter::PaintSingleRectOutline`).
    ///
    /// ✅ dotted - Defines a dotted border
    /// ✅ dashed - Defines a dashed border
    /// ✅ solid - Defines a solid border
    /// ❌ double - Defines a double border
    /// ❌ groove - Defines a 3D grooved border. The effect depends on the border-color value
    /// ❌ ridge - Defines a 3D ridged border. The effect depends on the border-color value
    /// ❌ inset - Defines a 3D inset border. The effect depends on the border-color value
    /// ❌ outset - Defines a 3D outset border. The effect depends on the border-color value
    /// ✅ none - Defines no border
    /// ✅ hidden - Defines a hidden border
    pub(crate) fn draw_outline(&self, scene: &mut impl PaintScene) {
        let Some((style, color, width, outset)) = self.outline_line() else {
            return;
        };
        let bb = self.frame.border_box;
        // A rounded corner grows with the outline, and a square one stays
        // square.
        let corner = |r: Vec2| {
            if r.x == 0.0 || r.y == 0.0 {
                r
            } else {
                Vec2::new((r.x + outset).max(0.0), (r.y + outset).max(0.0))
            }
        };
        let radii = self.frame.border_radii;
        let frame = CssBox::new(
            Rect::new(bb.x0 - outset, bb.y0 - outset, bb.x1 + outset, bb.y1 + outset),
            Insets::uniform(width),
            Insets::ZERO,
            0.0,
            NonUniformRoundedRectRadii {
                top_left: corner(radii.top_left),
                top_right: corner(radii.top_right),
                bottom_right: corner(radii.bottom_right),
                bottom_left: corner(radii.bottom_left),
            },
        );

        match style {
            BorderStyle::None | BorderStyle::Hidden => {}
            BorderStyle::Dotted | BorderStyle::Dashed => {
                self.draw_dotted_dashed_sides(scene, &frame, [Some((style, color)); 4]);
            }
            // TODO: Implement other border styles
            BorderStyle::Solid
            | BorderStyle::Inset
            | BorderStyle::Groove
            | BorderStyle::Outset
            | BorderStyle::Ridge
            | BorderStyle::Double => {
                // An outline pulled in to the middle of a small box leaves no
                // room inside it.
                let hollow = frame.border_box.width().min(frame.border_box.height()) > 2.0 * width;
                let path = if hollow { frame.border_ring() } else { frame.border_box_path() };
                scene.fill(Fill::NonZero, self.transform, color, None, &path);
            }
        }
    }
}

fn same_edge(a: &blitz_dom::CollapsedEdge, b: &blitz_dom::CollapsedEdge) -> bool {
    a.width == b.width && a.color == b.color
}

/// The grid lines around `tracks`, in physical order.
fn line_positions(tracks: &PhysicalTracks<'_>) -> Vec<f64> {
    let mut lines: Vec<f64> = tracks.iter().map(|track| track.start as f64).collect();
    if let Some(last) = tracks.iter().last() {
        lines.push(last.end as f64);
    }
    lines
}
