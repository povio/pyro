//! pyro: work that shows in no pixel, left out of a frame without changing any.
//!
//! **Box shadows under an opaque fill.** An element paints its outset box shadows, then its
//! background over them. vello_cpu's blurred rounded rectangle evaluates its blur (three `powf`) at
//! every pixel of its rectangle, and its depth buffer skips a pixel only in 128-pixel buckets that
//! one opaque fill spans across a whole 4-pixel row: under a card's background most of the shadow
//! was still shaded, and a background shrunk away from a rounded border (blitz-paint's bleed
//! avoidance) lost the rows along its top and bottom edges as well. Box shadows followed directly
//! by a fill of an opaque colour whose shape is convex are drawn through a clip path (vello_cpu's
//! non-isolated clip: each fill's strips are cut to it) that leaves out a hole inside that shape
//! ([`shadow_clip`]). The hole stays more than a pixel inside the shape, so every pixel the clip
//! touches at all is one the fill covers whole, and replaces; elsewhere the clip covers whole
//! pixels, where vello_cpu copies the shadow's own coverage (and `(a * 255 + 255) >> 8` is `a`).
//!
//! **Clip layers in a row.** Each overflowing tile of an SVG background is drawn in a clip layer of
//! its own (blitz-paint), and every layer costs vello_cpu a buffer per row it touches and a
//! composite. A run of sibling clip layers whose clips are rectangles of whole device pixels, none
//! overlapping another, is drawn in fewer layers: a layer takes the tiles whose content reaches no
//! other of its tiles' rectangles ([`clip_layer_groups`]). Each pixel is then composited by the one
//! layer whose rectangle holds it, from the one tile's content that reaches it, as before.

use anyrender::Paint;
use anyrender::recording::{FillCommand, RenderCommand};
use kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape};

use crate::filters::PixelRect;
use crate::scene::{content_bounds, matching_pop, pixel_rect};

/// A hole smaller than this (device px²) is not worth the clip.
const MIN_HOLE_AREA: f64 = 1024.0;

/// The index after the box shadows that start at `start`.
pub(crate) fn shadow_run_end(commands: &[RenderCommand], start: usize) -> usize {
    let mut end = start;
    while matches!(commands.get(end), Some(RenderCommand::BoxShadow(_))) {
        end += 1;
    }
    end
}

/// The clip path (device px, even-odd) to draw the box shadows `shadows` (under `transform`)
/// through when `cover`, the command after them, fills an opaque colour over a hole worth leaving
/// out of them; `None` otherwise.
pub(crate) fn shadow_clip(
    shadows: &[RenderCommand],
    cover: &RenderCommand,
    transform: Affine,
) -> Option<BezPath> {
    let RenderCommand::Fill(fill) = cover else {
        return None;
    };
    let (wide, tall) = opaque_inside(fill, transform)?;
    // Where the shadows paint: vello_cpu fills each one's rectangle grown by 2.5 standard
    // deviations.
    let mut outer: Option<Rect> = None;
    for shadow in shadows {
        let RenderCommand::BoxShadow(c) = shadow else {
            return None;
        };
        let reach = 2.5 * c.std_dev.abs() + 1.0;
        let area = (transform * c.transform).transform_rect_bbox(c.rect.inflate(reach, reach));
        outer = Some(outer.map_or(area, |o| o.union(area)));
    }
    let outer = outer?.inflate(2.0, 2.0);
    let outer = Rect::new(
        outer.x0.floor(),
        outer.y0.floor(),
        outer.x1.ceil(),
        outer.y1.ceil(),
    );
    let within = outer.inflate(-1.0, -1.0);
    let clamp = |r: Option<Rect>| {
        r.map(|r| r.intersect(within))
            .filter(|r| r.width() > 0.0 && r.height() > 0.0)
    };
    let (wide, tall) = (clamp(wide), clamp(tall));
    let mut path = BezPath::new();
    path.move_to((outer.x0, outer.y0));
    path.line_to((outer.x1, outer.y0));
    path.line_to((outer.x1, outer.y1));
    path.line_to((outer.x0, outer.y1));
    path.close_path();
    let area = |r: Rect| r.width() * r.height();
    match (wide, tall) {
        // A cross: the wide rectangle's band with the tall one's ends.
        (Some(h), Some(v)) if v.x0 >= h.x0 && v.x1 <= h.x1 && v.y0 <= h.y0 && v.y1 >= h.y1 => {
            if area(h) + area(v) - area(h.intersect(v)) < MIN_HOLE_AREA {
                return None;
            }
            let points = [
                (v.x0, v.y0),
                (v.x1, v.y0),
                (v.x1, h.y0),
                (h.x1, h.y0),
                (h.x1, h.y1),
                (v.x1, h.y1),
                (v.x1, v.y1),
                (v.x0, v.y1),
                (v.x0, h.y1),
                (h.x0, h.y1),
                (h.x0, h.y0),
                (v.x0, h.y0),
            ];
            path.move_to(points[0]);
            for p in &points[1..] {
                path.line_to(*p);
            }
            path.close_path();
        }
        (h, v) => {
            let hole = [h, v]
                .into_iter()
                .flatten()
                .max_by(|a, b| area(*a).total_cmp(&area(*b)))?;
            if area(hole) < MIN_HOLE_AREA {
                return None;
            }
            path.move_to((hole.x0, hole.y0));
            path.line_to((hole.x0, hole.y1));
            path.line_to((hole.x1, hole.y1));
            path.line_to((hole.x1, hole.y0));
            path.close_path();
        }
    }
    Some(path)
}

/// Rectangles of whole device pixels that `fill` (under `transform`) covers whole with an opaque
/// colour, each pixel next to them too: a wide one (between the straight left and right sides of
/// its shape, the full width) and a tall one (between the top and bottom sides, the full height),
/// whose union is a cross clear of a rounded rectangle's corners; for a shape without straight
/// sides (an ellipse), one rectangle inscribed in it. `None` unless the colour is opaque, the
/// transform keeps axes and the shape is one convex outline (its control polygon convex, so a
/// rectangle whose corners are inside is inside).
fn opaque_inside(fill: &FillCommand, transform: Affine) -> Option<(Option<Rect>, Option<Rect>)> {
    let Paint::Solid(color) = &fill.brush else {
        return None;
    };
    if color.components[3] < 1.0 {
        return None;
    }
    let t = transform * fill.transform;
    let [a, b, c, d, _, _] = t.as_coeffs();
    if b != 0.0 || c != 0.0 || a <= 0.0 || d <= 0.0 {
        return None;
    }
    let mut path = fill.shape.clone();
    path.apply_affine(t);

    // One closed outline: its control polygon, and its straight segments.
    let mut polygon: Vec<Point> = Vec::new();
    let mut lines: Vec<(Point, Point)> = Vec::new();
    let (mut start, mut last, mut closed) = (None, None, false);
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                if start.is_some() {
                    return None;
                }
                (start, last) = (Some(p), Some(p));
                polygon.push(p);
            }
            PathEl::LineTo(p) if !closed => {
                lines.push((last?, p));
                polygon.push(p);
                last = Some(p);
            }
            PathEl::QuadTo(p1, p) if !closed => {
                last?;
                polygon.extend([p1, p]);
                last = Some(p);
            }
            PathEl::CurveTo(p1, p2, p) if !closed => {
                last?;
                polygon.extend([p1, p2, p]);
                last = Some(p);
            }
            PathEl::ClosePath if !closed => {
                closed = true;
                if let (Some(s), Some(l)) = (start, last)
                    && s != l
                {
                    lines.push((l, s));
                }
            }
            _ => return None,
        }
    }
    if !convex(&polygon) {
        return None;
    }
    let (x0, x1) = polygon
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
            (lo.min(p.x), hi.max(p.x))
        });
    let (y0, y1) = polygon
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
            (lo.min(p.y), hi.max(p.y))
        });
    if !(x1 - x0 > 4.0 && y1 - y0 > 4.0) {
        return None;
    }
    // The extent of the straight segments along each side of the bounding box. A side's points are
    // matched to a ten-thousandth of a pixel: arcs end, and put their control points, where a sine
    // and a cosine put them, so at fractional coordinates a rounded box's straight sides missed
    // its corners' extreme control points by a rounding, and its hole was the small rectangle
    // inscribed in its ellipse. The hole stays a pixel and a half inside, and is checked.
    let near = |a: f64, b: f64| (a - b).abs() < 1e-4;
    let side = |on: &dyn Fn(Point) -> bool, along: &dyn Fn(Point) -> f64| {
        lines
            .iter()
            .filter(|(p, q)| on(*p) && on(*q))
            .map(|(p, q)| (along(*p).min(along(*q)), along(*p).max(along(*q))))
            .reduce(|(a0, a1), (b0, b1)| (a0.min(b0), a1.max(b1)))
    };
    let left = side(&|p| near(p.x, x0), &|p| p.y);
    let right = side(&|p| near(p.x, x1), &|p| p.y);
    let top = side(&|p| near(p.y, y0), &|p| p.x);
    let bottom = side(&|p| near(p.y, y1), &|p| p.x);
    let wide = left
        .zip(right)
        .map(|(l, r)| Rect::new(x0, l.0.max(r.0), x1, l.1.min(r.1)));
    let tall = top
        .zip(bottom)
        .map(|(t, b)| Rect::new(t.0.max(b.0), y0, t.1.min(b.1), y1));
    // Whole pixels at least a pixel and a half inside, so that a pixel that touches the hole lies
    // inside the outline (checked at the corners of the hole grown by a pixel).
    let hole = |r: Rect| {
        let r = r.inflate(-1.5, -1.5);
        let r = Rect::new(r.x0.ceil(), r.y0.ceil(), r.x1.floor(), r.y1.floor());
        if !(r.width() > 0.0 && r.height() > 0.0) {
            return None;
        }
        let grown = r.inflate(1.0, 1.0);
        let corners = [
            (grown.x0, grown.y0),
            (grown.x1, grown.y0),
            (grown.x1, grown.y1),
            (grown.x0, grown.y1),
        ];
        corners
            .iter()
            .all(|&p| path.winding(Point::from(p)) != 0)
            .then_some(r)
    };
    let (wide, tall) = (wide.and_then(hole), tall.and_then(hole));
    if wide.is_none() && tall.is_none() {
        // No straight sides to speak of (a circle, an ellipse, a pill's round ends): the
        // rectangle inscribed in the ellipse the bounding box holds, its corners on that ellipse.
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let (hw, hh) = (
            (x1 - x0) / 2.0 / std::f64::consts::SQRT_2,
            (y1 - y0) / 2.0 / std::f64::consts::SQRT_2,
        );
        return hole(Rect::new(cx - hw, cy - hh, cx + hw, cy + hh)).map(|r| (Some(r), None));
    }
    Some((wide, tall))
}

/// Whether the closed polygon `points` turns one way only, once around.
fn convex(points: &[Point]) -> bool {
    let mut pts: Vec<Point> = Vec::with_capacity(points.len());
    for &p in points {
        if pts.last() != Some(&p) {
            pts.push(p);
        }
    }
    while pts.len() > 1 && pts.first() == pts.last() {
        pts.pop();
    }
    let n = pts.len();
    if n < 3 {
        return false;
    }
    let (mut sign, mut turn) = (0.0f64, 0.0f64);
    for i in 0..n {
        let (p, q, r) = (pts[i], pts[(i + 1) % n], pts[(i + 2) % n]);
        let (u, v) = (q - p, r - q);
        let cross = u.cross(v);
        if cross.abs() > 1e-9 * u.hypot() * v.hypot() {
            if sign == 0.0 {
                sign = cross.signum();
            } else if cross.signum() != sign {
                return false;
            }
        }
        turn += cross.atan2(u.dot(v));
    }
    (turn.abs() - std::f64::consts::TAU).abs() < 1e-6
}

/// Clip layers drawn together: their rectangles' union (device px) and, in order, the commands
/// each one clipped (the range between its push and its pop).
pub(crate) struct ClipLayerGroup {
    pub(crate) clip: BezPath,
    pub(crate) members: Vec<(usize, usize)>,
}

/// The sibling clip layers that start at `start` (under `transform`) drawn in fewer layers, and
/// the index after the last of them; `None` when they are not worth it or not safe to: fewer than
/// three, a clip that is not a rectangle of whole device pixels, two rectangles that overlap, or no
/// two layers that can share one. Layers join a group in order, a layer the first group none of
/// whose members' content reaches its rectangle, and whose content reaches none of theirs (content
/// bounds as `content_bounds` makes them, grown by a pixel).
pub(crate) fn clip_layer_groups(
    commands: &[RenderCommand],
    start: usize,
    transform: Affine,
) -> Option<(Vec<ClipLayerGroup>, usize)> {
    const MAX_LAYERS: usize = 512;
    struct Member {
        push: usize,
        pop: usize,
        rect: PixelRect,
        reach: PixelRect,
    }
    let mut members: Vec<Member> = Vec::new();
    let mut i = start;
    while let Some(RenderCommand::PushClipLayer(clip)) = commands.get(i) {
        if members.len() == MAX_LAYERS {
            break;
        }
        let pop = matching_pop(commands, i);
        if pop >= commands.len() {
            break;
        }
        let Some(rect) = pixel_rect(&clip.clip, transform * clip.transform) else {
            break;
        };
        let reach = content_bounds(&commands[i + 1..pop], transform).map_or(
            PixelRect {
                x0: 0,
                y0: 0,
                x1: 0,
                y1: 0,
            },
            |b| PixelRect::around(b.inflate(1.0, 1.0)),
        );
        members.push(Member {
            push: i,
            pop,
            rect,
            reach,
        });
        i = pop + 1;
    }
    if members.len() < 3 {
        return None;
    }
    let disjoint =
        |a: PixelRect, b: PixelRect| a.is_empty() || b.is_empty() || a.intersect(b).is_empty();
    for (k, a) in members.iter().enumerate() {
        if members[k + 1..].iter().any(|b| !disjoint(a.rect, b.rect)) {
            return None;
        }
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (k, m) in members.iter().enumerate() {
        let fits = |group: &Vec<usize>| {
            group.iter().all(|&n| {
                let other = &members[n];
                disjoint(m.reach, other.rect) && disjoint(other.reach, m.rect)
            })
        };
        match groups.iter_mut().find(|group| fits(group)) {
            Some(group) => group.push(k),
            None => groups.push(vec![k]),
        }
    }
    if groups.len() == members.len() {
        return None;
    }
    let groups = groups
        .into_iter()
        .map(|group| {
            let mut clip = BezPath::new();
            for &k in &group {
                let r = members[k].rect;
                let (x0, y0, x1, y1) = (
                    f64::from(r.x0),
                    f64::from(r.y0),
                    f64::from(r.x1),
                    f64::from(r.y1),
                );
                clip.move_to((x0, y0));
                clip.line_to((x1, y0));
                clip.line_to((x1, y1));
                clip.line_to((x0, y1));
                clip.close_path();
            }
            ClipLayerGroup {
                clip,
                members: group
                    .iter()
                    .map(|&k| (members[k].push, members[k].pop))
                    .collect(),
            }
        })
        .collect();
    Some((groups, i))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ImageCacheConfig, VelloCpuScenePainter};
    use anyrender::PaintScene;
    use anyrender::recording::Scene;
    use kurbo::{Circle, RoundedRect, Stroke};
    use peniko::{Color, Fill};

    /// `scene`'s pixels, rendered on `threads` worker threads (0: the calling thread), with or
    /// without leaving out what shows in no pixel.
    fn render(scene: &Scene, width: u16, height: u16, occlusion: bool, threads: u16) -> Vec<u8> {
        let settings = vello_cpu::RenderSettings {
            num_threads: threads,
            ..Default::default()
        };
        let mut painter = VelloCpuScenePainter::with_settings(
            width,
            height,
            settings,
            ImageCacheConfig::default(),
        );
        painter.occlusion = occlusion;
        painter.append_scene(scene.clone(), Affine::IDENTITY);
        painter.render_ctx.flush();
        painter.finish().data_as_u8_slice().to_vec()
    }

    fn assert_same_pixels(scene: &Scene, width: u16, height: u16) {
        let threads: &[u16] = if cfg!(feature = "multithreading") {
            &[0, 3]
        } else {
            &[0]
        };
        for &threads in threads {
            let plain = render(scene, width, height, false, threads);
            let culled = render(scene, width, height, true, threads);
            let differing = plain
                .chunks(4)
                .zip(culled.chunks(4))
                .filter(|(a, b)| a != b)
                .count();
            assert_eq!(
                differing, 0,
                "{differing} pixels differ ({threads} threads)"
            );
        }
    }

    /// Cards as blitz-paint paints them at a device scale of 2: two outset shadows, a background
    /// shrunk by half the border (bleed avoidance) or not, the border; rounded, square and pill
    /// shaped, at fractional positions; one with a translucent background, one with a circle.
    fn cards(scale: f64, offset: (f64, f64)) -> Scene {
        let mut scene = Scene::new();
        let page = Affine::translate(offset) * Affine::scale(scale);
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::from_rgb8(0xf3, 0xf4, 0xf6),
            None,
            &Rect::new(0.0, 0.0, 900.0, 400.0),
        );
        let cards: [(Rect, f64, bool, u8); 5] = [
            (Rect::new(10.3, 12.0, 110.3, 132.0), 12.0, true, 255),
            (Rect::new(130.0, 20.5, 230.0, 140.5), 0.0, false, 255),
            (Rect::new(250.25, 15.75, 330.25, 55.75), 20.0, true, 255),
            (Rect::new(10.0, 150.0, 110.0, 190.0), 6.0, false, 128),
            (Rect::new(350.0, 10.0, 420.0, 80.0), 35.0, true, 255),
        ];
        for (rect, radius, shrink, alpha) in cards {
            scene.draw_box_shadow(
                page * Affine::translate((0.0, 1.0)),
                rect,
                Color::from_rgba8(0, 0, 0, 26),
                radius,
                1.5,
            );
            scene.draw_box_shadow(
                page * Affine::translate((0.0, 4.0)),
                rect.inflate(2.0, 2.0),
                Color::from_rgba8(0, 0, 0, 40),
                radius + 2.0,
                6.0,
            );
            let inset = if shrink { 0.5 } else { 0.0 };
            let background =
                RoundedRect::from_rect(rect.inflate(-inset, -inset), (radius - inset).max(0.0));
            scene.fill(
                Fill::NonZero,
                page,
                Color::from_rgba8(255, 255, 255, alpha),
                None,
                &background,
            );
            let border = RoundedRect::from_rect(rect.inflate(-0.5, -0.5), (radius - 0.5).max(0.0));
            scene.stroke(
                &Stroke::new(1.0),
                page,
                Color::from_rgb8(0xe5, 0xe7, 0xeb),
                None,
                &border,
            );
        }
        let avatar = Circle::new((200.0, 300.0), 40.0);
        scene.draw_box_shadow(
            page,
            avatar.bounding_box(),
            Color::from_rgba8(80, 0, 120, 90),
            40.0,
            4.0,
        );
        scene.fill(
            Fill::NonZero,
            page,
            Color::from_rgb8(0xa7, 0x8b, 0xfa),
            None,
            &avatar,
        );
        scene
    }

    fn shadow_clips(scene: &Scene) -> Vec<Option<BezPath>> {
        let commands = &scene.commands;
        let mut clips = Vec::new();
        let mut i = 0;
        while i < commands.len() {
            if matches!(commands[i], RenderCommand::BoxShadow(_)) {
                let end = shadow_run_end(commands, i);
                clips.push(
                    commands
                        .get(end)
                        .and_then(|cover| shadow_clip(&commands[i..end], cover, Affine::IDENTITY)),
                );
                i = end;
            } else {
                i += 1;
            }
        }
        clips
    }

    #[test]
    fn shadows_under_an_opaque_background_keep_their_pixels() {
        for (scale, offset) in [
            (1.0, (0.0, 0.0)),
            (2.0, (0.0, 0.0)),
            (2.0, (0.25, 0.5)),
            (1.5, (3.0, 1.0)),
        ] {
            let scene = cards(scale, offset);
            // Each opaque card and the avatar get a hole; the translucent card none.
            let holes: Vec<bool> = shadow_clips(&scene).iter().map(Option::is_some).collect();
            assert_eq!(
                holes,
                [true, true, true, false, true, true],
                "scale {scale}, offset {offset:?}"
            );
            assert_same_pixels(&scene, 900, 400);
        }
    }

    /// A rounded box's outline as blitz-paint's `CssBox` draws it: a `kurbo::Arc` per corner,
    /// joined by lines, so that its straight sides are a rounding off its corners' extreme
    /// control points.
    fn css_box_outline(rect: Rect, radius: f64) -> BezPath {
        use std::f64::consts::{FRAC_PI_2, PI};
        let mut path = BezPath::new();
        let corners = [
            ((rect.x0 + radius, rect.y0 + radius), -FRAC_PI_2),
            ((rect.x1 - radius, rect.y0 + radius), 0.0),
            ((rect.x1 - radius, rect.y1 - radius), FRAC_PI_2),
            ((rect.x0 + radius, rect.y1 - radius), PI),
        ];
        for (center, offset) in corners {
            let arc = kurbo::Arc::new(
                center,
                (radius, radius),
                offset + PI + FRAC_PI_2,
                FRAC_PI_2,
                0.0,
            );
            let mut elements = arc.path_elements(0.1);
            match elements.next().unwrap() {
                PathEl::MoveTo(p) if !path.elements().is_empty() => path.push(PathEl::LineTo(p)),
                el => path.push(el),
            }
            path.extend(elements);
        }
        path.close_path();
        path
    }

    /// A card's background as blitz-paint draws it (1px border at a device scale of 2, shrunk by
    /// bleed avoidance), also at fractional and large coordinates: the hole is the cross between
    /// its straight sides, reaching to 3 px from them, not the rectangle inscribed in its ellipse.
    #[test]
    fn a_rounded_box_drawn_from_arcs_leaves_a_cross() {
        for (k, rect) in [
            Rect::new(32.0, 32.0, 476.0, 404.0),
            Rect::new(32.3, 31.7, 476.9, 404.1),
            Rect::new(3001.1, 1700.35, 3445.7, 2072.2),
        ]
        .into_iter()
        .enumerate()
        {
            let mut scene = Scene::new();
            let shadow = Color::from_rgba8(0, 0, 0, 26);
            scene.draw_box_shadow(Affine::translate((0.0, 2.0)), rect, shadow, 24.0, 3.0);
            let inner = rect.inflate(-1.0, -1.0);
            let outline = css_box_outline(inner, 23.3);
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                Color::WHITE,
                None,
                &outline,
            );
            let clip = shadow_clip(&scene.commands[..1], &scene.commands[1], Affine::IDENTITY);
            let clip = clip.unwrap();
            let c = inner.center();
            for point in [
                (inner.x0 + 4.0, c.y),
                (inner.x1 - 4.0, c.y),
                (c.x, inner.y0 + 4.0),
                (c.x, inner.y1 - 4.0),
                (c.x, c.y),
            ] {
                let winding = clip.winding(Point::from(point));
                assert_eq!(winding % 2, 0, "{point:?} is shaded");
            }
            // In a corner the shadow is shaded.
            let corner = Point::new(inner.x0 + 3.0, inner.y0 + 8.0);
            assert_ne!(clip.winding(corner) % 2, 0);
            if k == 0 {
                assert_same_pixels(&scene, 520, 440);
            }
        }
    }

    #[test]
    fn a_shadow_is_cut_only_under_a_convex_opaque_fill() {
        let rect = Rect::new(20.0, 20.0, 220.0, 170.0);
        let with_cover = |cover: &dyn Fn(&mut Scene), transform: Affine| {
            let mut scene = Scene::new();
            scene.draw_box_shadow(transform, rect, Color::BLACK, 10.0, 4.0);
            cover(&mut scene);
            shadow_clip(&scene.commands[..1], &scene.commands[1], Affine::IDENTITY)
        };
        let rounded = RoundedRect::from_rect(rect, 10.0);
        assert!(
            with_cover(
                &|s| s.fill(
                    Fill::NonZero,
                    Affine::IDENTITY,
                    Color::WHITE,
                    None,
                    &rounded
                ),
                Affine::IDENTITY
            )
            .is_some()
        );
        // Translucent: what is under it shows.
        let translucent = Color::from_rgba8(255, 255, 255, 254);
        assert!(
            with_cover(
                &|s| s.fill(Fill::NonZero, Affine::IDENTITY, translucent, None, &rounded),
                Affine::IDENTITY
            )
            .is_none()
        );
        // A ring (a border): not convex, its middle is empty.
        let mut ring = rounded.to_path(0.1);
        ring.extend(RoundedRect::from_rect(rect.inflate(-8.0, -8.0), 4.0).to_path(0.1));
        assert!(
            with_cover(
                &|s| s.fill(Fill::EvenOdd, Affine::IDENTITY, Color::WHITE, None, &ring),
                Affine::IDENTITY
            )
            .is_none()
        );
        // A concave outline.
        let mut notch = BezPath::new();
        for p in [
            (20.0, 20.0),
            (220.0, 20.0),
            (120.0, 90.0),
            (220.0, 170.0),
            (20.0, 170.0),
        ] {
            if notch.elements().is_empty() {
                notch.move_to(p)
            } else {
                notch.line_to(p)
            }
        }
        notch.close_path();
        assert!(
            with_cover(
                &|s| s.fill(Fill::NonZero, Affine::IDENTITY, Color::WHITE, None, &notch),
                Affine::IDENTITY
            )
            .is_none()
        );
        // A rotated fill.
        let turned = Affine::rotate_about(0.3, (120.0, 95.0));
        assert!(
            with_cover(
                &|s| s.fill(Fill::NonZero, turned, Color::WHITE, None, &rounded),
                Affine::IDENTITY
            )
            .is_none()
        );
        // A small one is not worth a clip.
        let small = RoundedRect::new(20.0, 20.0, 40.0, 40.0, 2.0);
        assert!(
            with_cover(
                &|s| s.fill(Fill::NonZero, Affine::IDENTITY, Color::WHITE, None, &small),
                Affine::IDENTITY
            )
            .is_none()
        );
        // The hole leaves the corners, and a pixel and a half along every side.
        let clip = with_cover(
            &|s| {
                s.fill(
                    Fill::NonZero,
                    Affine::IDENTITY,
                    Color::WHITE,
                    None,
                    &rounded,
                )
            },
            Affine::IDENTITY,
        )
        .unwrap();
        for (point, inside_hole) in [
            ((120.0, 95.0), true),
            ((22.5, 95.0), true),
            ((21.5, 95.0), false),
            ((23.0, 23.0), false),
            ((120.0, 22.5), true),
            ((120.0, 21.5), false),
        ] {
            let shaded = clip.winding(Point::from(point)) % 2 != 0;
            assert_eq!(shaded, !inside_hole, "at {point:?}");
        }
    }

    /// Translucent tiles whose strokes overflow their clip rectangles, as blitz-paint draws an
    /// SVG background tile by tile.
    fn tiles(origin: (f64, f64), stroke: f64) -> Scene {
        let mut scene = Scene::new();
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::from_rgb8(250, 250, 250),
            None,
            &Rect::new(0.0, 0.0, 320.0, 160.0),
        );
        for row in 0..4 {
            for col in 0..6 {
                let tile = Rect::from_origin_size(
                    (
                        origin.0 + f64::from(col) * 40.0,
                        origin.1 + f64::from(row) * 30.0,
                    ),
                    (40.0, 30.0),
                );
                scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &tile);
                scene.fill(
                    Fill::NonZero,
                    Affine::IDENTITY,
                    Color::from_rgba8(224, 231, 255, 200),
                    None,
                    &tile,
                );
                scene.stroke(
                    &Stroke::new(stroke),
                    Affine::IDENTITY,
                    Color::from_rgba8(99, 102, 241, 170),
                    None,
                    &tile,
                );
                scene.pop_layer();
            }
        }
        scene
    }

    #[test]
    fn tiles_clipped_on_whole_pixels_share_layers_and_keep_their_pixels() {
        let scene = tiles((8.0, 6.0), 6.0);
        let (groups, end) = clip_layer_groups(&scene.commands, 1, Affine::IDENTITY).unwrap();
        assert_eq!(end, scene.commands.len());
        assert_eq!(groups.len(), 4, "every other tile across and down");
        assert_eq!(groups.iter().map(|g| g.members.len()).sum::<usize>(), 24);
        assert_same_pixels(&scene, 320, 160);
        // Content that reaches past the next tile shares with fewer, or none.
        let wide = tiles((8.0, 6.0), 30.0);
        if let Some((groups, _)) = clip_layer_groups(&wide.commands, 1, Affine::IDENTITY) {
            assert!(groups.len() > 4);
        }
        assert_same_pixels(&wide, 320, 160);
        // Edges between pixels: each tile keeps its own layer.
        let between = tiles((8.5, 6.25), 6.0);
        assert!(clip_layer_groups(&between.commands, 1, Affine::IDENTITY).is_none());
        assert_same_pixels(&between, 320, 160);
    }
}
