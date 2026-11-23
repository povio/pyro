//! pyro: SVG path data (`path()`) and `shape()` commands as kurbo paths, shared by `clip-path`
//! (blitz-paint) and `shape-outside` (float layout). Moved from blitz-paint `render/clip_path.rs`.

use kurbo::{BezPath, Point, SvgArc, Vec2};
use style::values::generics::basic_shape::{
    ArcSize, ArcSweep, AxisEndPoint, CommandEndPoint, ControlPoint, ControlReference, GenericShapeCommand,
};
use style::values::generics::position::GenericPosition;

pub type GenericPathCommand<Angle, N> = GenericShapeCommand<Angle, N, GenericPosition<N, N>, N>;

pub fn polygon_path(points: impl Iterator<Item = Point>, radius: f64) -> BezPath {
    let mut path = BezPath::new();
    if radius <= 0.0 {
        for (index, point) in points.enumerate() {
            if index == 0 { path.move_to(point); } else { path.line_to(point); }
        }
    } else {
        let points: Vec<_> = points.collect();
        for (index, &vertex) in points.iter().enumerate() {
            let incoming = vertex - points[(index + points.len() - 1) % points.len()];
            let outgoing = points[(index + 1) % points.len()] - vertex;
            let limit = incoming.hypot().min(outgoing.hypot()) * 0.5;
            let turn = incoming.cross(outgoing).atan2(incoming.dot(outgoing));
            let tangent = (turn.abs() * 0.5).tan();
            let distance = (radius * tangent).min(limit);
            let arc = if distance > 0.0 && incoming.cross(outgoing) != 0.0 {
                let from = vertex - incoming.normalize() * distance;
                let to = vertex + outgoing.normalize() * distance;
                kurbo::Arc::from_svg_arc(&SvgArc { from, to, radii: Vec2::splat(distance / tangent),
                    x_rotation: 0.0, large_arc: false, sweep: turn > 0.0 }).map(|arc| (from, arc))
            } else { None };
            let start = arc.as_ref().map_or(vertex, |(from, _)| *from);
            if index == 0 { path.move_to(start); } else { path.line_to(start); }
            if let Some((_, arc)) = arc { path.extend(arc.append_iter(0.01)); }
        }
    }
    path.close_path();
    path
}

/// Convert an SVG path() or a shape() to a kurbo BezPath.
/// The returned path is in the path's own coordinate system (origin at 0,0).
/// The caller is responsible for translating it to the reference box origin.
/// `resolve_radius` resolves an arc radius given alone (both radii).
pub fn svg_path_to_bezpath<Angle: Copy, N>(
    commands: &[GenericPathCommand<Angle, N>],
    resolve_x: impl Fn(&N) -> f64,
    resolve_y: impl Fn(&N) -> f64,
    resolve_radius: impl Fn(&N) -> f64,
    resolve_angle: impl Fn(&Angle) -> f64,
) -> Option<BezPath> {
    if commands.is_empty() {
        return None;
    }

    let mut bez = BezPath::new();
    let mut cur = Point::ZERO;
    let mut subpath_start = cur;
    // Tracks the last control point for smooth continuation.
    let mut last_control: Option<Point> = None;

    for cmd in commands {
        match &cmd {
            GenericShapeCommand::Close => {
                bez.close_path();
                cur = subpath_start;
                last_control = None;
            }
            GenericShapeCommand::Move { point } => {
                let p = resolve_endpoint(point, cur, &resolve_x, &resolve_y);
                bez.move_to(p);
                cur = p;
                subpath_start = p;
                last_control = None;
            }
            GenericShapeCommand::Line { point } => {
                let p = resolve_endpoint(point, cur, &resolve_x, &resolve_y);
                bez.line_to(p);
                cur = p;
                last_control = None;
            }
            GenericShapeCommand::HLine { x } => {
                cur.x = resolve_axis_endpoint(x, cur.x, &resolve_x);
                bez.line_to(cur);
                last_control = None;
            }
            GenericShapeCommand::VLine { y } => {
                cur.y = resolve_axis_endpoint(y, cur.y, &resolve_y);
                bez.line_to(cur);
                last_control = None;
            }
            GenericShapeCommand::CubicCurve {
                point,
                control1,
                control2,
            } => {
                let p = resolve_endpoint(point, cur, &resolve_x, &resolve_y);
                let c1 = resolve_control_point(control1, cur, p, &resolve_x, &resolve_y);
                let c2 = resolve_control_point(control2, cur, p, &resolve_x, &resolve_y);
                bez.curve_to(c1, c2, p);
                last_control = Some(c2);
                cur = p;
            }
            GenericShapeCommand::QuadCurve { point, control1 } => {
                let p = resolve_endpoint(point, cur, &resolve_x, &resolve_y);
                let c1 = resolve_control_point(control1, cur, p, &resolve_x, &resolve_y);
                bez.quad_to(c1, p);
                last_control = Some(c1);
                cur = p;
            }
            GenericShapeCommand::SmoothCubic { point, control2 } => {
                let p = resolve_endpoint(point, cur, &resolve_x, &resolve_y);
                let c2 = resolve_control_point(control2, cur, p, &resolve_x, &resolve_y);
                let c1 = reflect_point(last_control, cur);
                bez.curve_to(c1, c2, p);
                last_control = Some(c2);
                cur = p;
            }
            GenericShapeCommand::SmoothQuad { point } => {
                let p = resolve_endpoint(point, cur, &resolve_x, &resolve_y);
                let c1 = reflect_point(last_control, cur);
                bez.quad_to(c1, p);
                last_control = Some(c1);
                cur = p;
            }
            GenericShapeCommand::Arc {
                point,
                radii,
                arc_sweep,
                arc_size,
                rotate,
            } => {
                let p = resolve_endpoint(point, cur, &resolve_x, &resolve_y);
                let radii = match radii.ry.as_ref() {
                    Some(ry) => Vec2::new(resolve_x(&radii.rx), resolve_y(ry)),
                    None => Vec2::splat(resolve_radius(&radii.rx)),
                };
                let svg_arc = SvgArc {
                    from: cur,
                    to: p,
                    radii,
                    // kurbo takes radians; CSS and SVG give degrees.
                    x_rotation: resolve_angle(rotate).to_radians(),
                    large_arc: matches!(arc_size, ArcSize::Large),
                    // `cw` is SVG's sweep flag 1 (the direction of increasing angles, clockwise
                    // with y down).
                    sweep: matches!(arc_sweep, ArcSweep::Cw),
                };
                match kurbo::Arc::from_svg_arc(&svg_arc) {
                    Some(arc) => {
                        for el in arc.append_iter(0.1) {
                            bez.push(el);
                        }
                    }
                    // An arc with a zero radius is a straight line (SVG's out-of-range rule);
                    // one ending where it starts draws nothing.
                    None if p != cur => bez.line_to(p),
                    None => {}
                }
                last_control = None;
                cur = p;
            }
        }
    }

    Some(bez)
}

/// Resolve a CommandEndPoint to an absolute Point.
/// `ToPosition` is absolute; `ByCoordinate` is relative to `cur`.
fn resolve_endpoint<N>(
    endpoint: &CommandEndPoint<GenericPosition<N, N>, N>,
    cur: Point,
    resolve_x: impl Fn(&N) -> f64,
    resolve_y: impl Fn(&N) -> f64,
) -> Point {
    match endpoint {
        CommandEndPoint::ToPosition(pos) => {
            Point::new(resolve_x(&pos.horizontal), resolve_y(&pos.vertical))
        }
        CommandEndPoint::ByCoordinate(coord) => {
            Point::new(cur.x + resolve_x(&coord.x), cur.y + resolve_y(&coord.y))
        }
    }
}

/// Resolve a ControlPoint to an absolute Point.
/// - `Absolute`: uses the position directly.
/// - `Relative`: the coordinate pair is offset from a base point determined by `ControlReference`:
///   - `Start` (default): relative to the command's starting point (`cur`).
///   - `End`: relative to the command's end point (`end`).
///   - `Origin`: relative to the reference box origin (0,0 in path coordinates).
fn resolve_control_point<N>(
    control_point: &ControlPoint<GenericPosition<N, N>, N>,
    cur: Point,
    end: Point,
    resolve_x: impl Fn(&N) -> f64,
    resolve_y: impl Fn(&N) -> f64,
) -> Point {
    match control_point {
        ControlPoint::Absolute(pos) => {
            Point::new(resolve_x(&pos.horizontal), resolve_y(&pos.vertical))
        }
        ControlPoint::Relative(rel) => {
            let base = match rel.reference {
                ControlReference::Start => cur,
                ControlReference::End => end,
                ControlReference::Origin => Point::ZERO,
            };
            Point::new(
                base.x + resolve_x(&rel.coord.x),
                base.y + resolve_y(&rel.coord.y),
            )
        }
    }
}

/// Reflect a previous control point around the current point.
/// If there is no previous control point (previous command wasn't the matching curve type),
/// returns the current point itself.
fn reflect_point(last_control: Option<Point>, cur: Point) -> Point {
    match last_control {
        Some(ctrl) => Point::new(2.0 * cur.x - ctrl.x, 2.0 * cur.y - ctrl.y),
        None => cur,
    }
}

/// Resolve an AxisEndPoint to an absolute value.
/// `ToPosition` is absolute; `ByCoordinate` is relative to `cur_val`.
fn resolve_axis_endpoint<N>(
    endpoint: &AxisEndPoint<N, N>,
    cur_val: f64,
    resolve: impl Fn(&N) -> f64,
) -> f64 {
    match endpoint {
        AxisEndPoint::ToPosition(pos) => resolve(pos),
        AxisEndPoint::ByCoordinate(val) => cur_val + resolve(val),
    }
}
