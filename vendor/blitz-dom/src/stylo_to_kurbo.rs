use euclid::default::{Rect, Transform3D};
use kurbo::{Affine, Vec2};
use style::{
    properties::generated::style_structs::Box as BoxStyleStruct,
    values::{
        computed::{CSSPixelLength, Rotate},
        generics::{
            box_::GenericPerspective,
            transform::{Scale, Translate, get_normalized_vector_and_angle},
        },
    },
};

// 6. Current Transformation Matrix
//
// The transformation matrix is computed from the transform, transform-origin, translate, rotate, scale, and offset properties as follows:
//
//   - Start with the identity matrix.
//   - Translate by the computed X, Y, and Z values of transform-origin.
//   - Translate by the computed X, Y, and Z values of translate.
//   - Rotate by the computed <angle> about the specified axis of rotate.
//   - Scale by the computed X, Y, and Z values of scale.
//   - Translate and rotate by the transform specified by offset.
//   - Multiply by each of the transform functions in transform from left to right.
//   - Translate by the negated computed X, Y and Z values of transform-origin.
//
// <https://drafts.csswg.org/css-transforms-2/#ctm>
//
// pyro: a transformation matrix that is not 2D is `None` here; [`resolve_transform`] resolves
// it as a 4x4 matrix instead.
/// pyro: [`resolve_2d_transform`] for a reference box of `width` by `height` CSS px at the origin
/// (a table row's or row group's box, which blitz-paint transforms the cells by).
pub fn resolve_2d_transform_for_size(box_styles: &BoxStyleStruct, width: f32, height: f32) -> Option<Affine> {
    let reference_box = Rect::new(
        euclid::default::Point2D::new(CSSPixelLength::new(0.0), CSSPixelLength::new(0.0)),
        euclid::default::Size2D::new(CSSPixelLength::new(width), CSSPixelLength::new(height)),
    );
    resolve_2d_transform(box_styles, reference_box)
}

pub fn resolve_2d_transform(
    box_styles: &BoxStyleStruct,
    reference_box: Rect<CSSPixelLength>,
) -> Option<Affine> {
    match resolve_transform(box_styles, reference_box) {
        ResolvedTransform::Flat(affine) => affine,
        ResolvedTransform::Spatial(_) => None,
    }
}

/// pyro: an element's transformation matrix, as [`resolve_transform`] resolves it.
pub enum ResolvedTransform {
    /// Every function and property is 2D (or resolves to a 2D matrix, like
    /// `translate3d(x, y, 0)`): the 2D affine, `None` for the identity.
    Flat(Option<Affine>),
    /// A 3D matrix (CSS px, the border box's origin at 0): what it shows in
    /// the plane of the screen depends on the perspective of the containing
    /// block ([`resolve_perspective_matrix`]); [`flatten_transform`] projects it.
    Spatial(Transform3D<f64>),
}

/// pyro: the transformation matrix (CSS Transforms 2 §6, above). A 2D one resolves as it
/// always has (`kurbo` in f64, the transform list in f32), so 2D pages paint as before; a 3D
/// one is the 4x4 matrix in f64, which `transform-origin`'s depth also enters.
///
/// `reference_box`: the box `transform-box` names, in the border box's coordinates: its size
/// resolves percentages, its origin offsets `transform-origin`.
pub fn resolve_transform(
    box_styles: &BoxStyleStruct,
    reference_box: Rect<CSSPixelLength>,
) -> ResolvedTransform {
    // The transform list's matrix, which `is_3d` and the 2D path both read.
    let list = if box_styles.transform.0.is_empty() {
        None
    } else {
        box_styles
            .transform
            .to_transform_3d_matrix(Some(&reference_box))
            .ok()
            .map(|(t, _has_3d)| t)
    };

    if is_3d(box_styles, list.as_ref()) {
        return ResolvedTransform::Spatial(resolve_transform_3d(box_styles, reference_box));
    }

    let translate = match &box_styles.translate {
        Translate::None => None,
        Translate::Translate(x, y, _z) => Some(Vec2 {
            x: x.resolve(reference_box.width()).px() as f64,
            y: y.resolve(reference_box.height()).px() as f64,
        }),
    };

    let rotate = match &box_styles.rotate {
        Rotate::None => None,
        Rotate::Rotate(angle) => Some(angle.radians64()),
        // A rotation about the z axis stays in the plane; the sign of the
        // axis component decides the direction, its magnitude cancels out.
        // (`is_3d` sent any other axis to the 3D path.)
        Rotate::Rotate3D(_, _, z, angle) => Some(angle.radians64() * z.signum() as f64),
    };

    let scale_transform = match &box_styles.scale {
        Scale::None => None,
        Scale::Scale(x, y, _z) => Some(Vec2 {
            x: *x as f64,
            y: *y as f64,
        }),
    };

    // The `has_3d` flag of the list's matrix means a 3d function appeared in the list, not
    // that the matrix needs 3d: `translate3d(x, y, 0)` sets it while resolving to a plain 2d
    // translation. (`is_3d` sent the other matrices to the 3D path.)
    let transform = list.filter(|t| t.is_2d()).map(|t| {
        // See: https://drafts.csswg.org/css-transforms-2/#two-dimensional-subset
        // And https://docs.rs/kurbo/latest/kurbo/struct.Affine.html#method.new
        Affine::new([t.m11, t.m12, t.m21, t.m22, t.m41, t.m42].map(|v| v as f64))
    });

    // pyro: the offset transform (CSS Motion Path 1 § 4) of an `offset-path: path()` or `ray()`.
    let offset = offset_transform(box_styles, reference_box);

    if translate.is_none() && rotate.is_none() && scale_transform.is_none() && transform.is_none() && offset.is_none() {
        return ResolvedTransform::Flat(None);
    }

    // Apply the transform origin by:
    //   - Translating by the origin offset
    //   - Applying our transform
    //   - Translating by the inverse of the origin offset
    let transform_origin = &box_styles.transform_origin;
    let origin_translation = Affine::translate(Vec2 {
        x: reference_box.origin.x.px() as f64
            + transform_origin
                .horizontal
                .resolve(reference_box.width())
                .px() as f64,
        y: reference_box.origin.y.px() as f64
            + transform_origin
                .vertical
                .resolve(reference_box.height())
                .px() as f64,
    });

    let mut resolved = Affine::IDENTITY;

    if let Some(translation) = translate {
        resolved *= Affine::translate(translation)
    }

    if let Some(rotation) = rotate {
        resolved *= Affine::rotate(rotation)
    }

    if let Some(scale_transform) = scale_transform {
        resolved *= Affine::scale_non_uniform(scale_transform.x, scale_transform.y)
    }

    // The offset transform comes after the individual transform properties, before `transform`.
    if let Some(offset) = offset {
        resolved *= offset;
    }

    if let Some(transform) = transform {
        resolved *= transform;
    }

    // pyro: a `rotate` is in the matrix at the transform list's precision, f32 (stylo casts the
    // list's matrix to f32): `rotate: 45deg` is then the matrix `transform: rotate(45deg)` is, where
    // its f64 sine and cosine moved antialiased edges by a level (WPT individual-transform-2a/b/c).
    if rotate.is_some() {
        resolved = Affine::new(resolved.as_coeffs().map(|v| v as f32 as f64));
    }

    // pyro: coefficients a hair from whole numbers are whole, as Chromium's rotations by multiples
    // of 90 degrees are exact (`SinCosDegrees`): `rotate(360deg)` is the identity, so its text is
    // placed and snapped as untransformed text is (WPT css-transforms/transform-input-*).
    resolved = Affine::new(resolved.as_coeffs().map(|v| {
        let whole = v.round();
        if (v - whole).abs() < 1e-9 { whole } else { v }
    }));

    resolved = origin_translation * resolved * origin_translation.inverse();

    ResolvedTransform::Flat((resolved != Affine::IDENTITY).then_some(resolved))
}

/// pyro: the offset transform (CSS Motion Path 1 § 4), in the frame of the transform origin, as
/// Chrome lays it out (measured): the anchor point (`offset-anchor`, `auto`: the transform origin)
/// goes to the point `offset-distance` along the path (a closed path wraps around, an open one
/// clamps) and the box turns by `offset-rotate` (`auto`, `reverse`: plus the path's direction).
/// `path()` is in the box's coordinates (its top-left corner at the origin); `ray()`, basic shapes,
/// `url()` and a coord box alone are in the containing block's coord box ([`MotionFrame`]).
fn offset_transform(box_styles: &BoxStyleStruct, reference_box: Rect<CSSPixelLength>) -> Option<Affine> {
    use style::values::generics::basic_shape::{GenericBasicShape, GenericPathOrShapeFunction};
    use style::values::generics::motion::{GenericOffsetPath, GenericOffsetPathFunction};
    let (path, coord_box) = match &box_styles.offset_path {
        GenericOffsetPath::OffsetPath { path, coord_box } => (path, coord_box),
        GenericOffsetPath::CoordBox(coord_box) => return coord_box_transform(box_styles, reference_box, *coord_box),
        GenericOffsetPath::None => return None,
    };
    let shape = match &**path {
        GenericOffsetPathFunction::Ray(ray) => return ray_transform(box_styles, reference_box, ray, *coord_box),
        GenericOffsetPathFunction::Shape(shape) => shape,
        GenericOffsetPathFunction::Url(_) => {
            // An SVG shape element's geometry, laid out as a basic shape is (Chrome); anything
            // else is `path("m 0 0")`: the anchor point goes to the box's top-left corner.
            let Some(path) = MOTION_URL_PATH.with(|p| p.borrow().clone()) else {
                let frame = MotionFrame::new(box_styles, reference_box, *coord_box);
                return Some(frame.place(kurbo::Point::ZERO, box_styles.offset_rotate.angle.radians64()));
            };
            let frame = MotionFrame::new(box_styles, reference_box, *coord_box);
            let mut path = path;
            path.apply_affine(Affine::translate(frame.area.origin().to_vec2()));
            let (point, direction) = point_along(&path, &box_styles.offset_distance)?;
            let rotate = &box_styles.offset_rotate;
            let mut angle = rotate.angle.radians64();
            if rotate.auto {
                angle += direction;
            }
            return Some(frame.place(point, angle));
        }
    };
    if let GenericBasicShape::PathOrShape(GenericPathOrShapeFunction::Path(path)) = shape {
        let bez = crate::shape_path::svg_path_to_bezpath(
            path.commands(),
            |v: &f32| *v as f64,
            |v: &f32| *v as f64,
            |v: &f32| *v as f64,
            |a: &f32| *a as f64,
        )?;
        // The path is in the box's coordinates (its top-left corner at the origin), and the anchor
        // point goes to the point along it (as Chrome, measured).
        let (point, direction) = point_along(&bez, &box_styles.offset_distance)?;
        let rotate = &box_styles.offset_rotate;
        let mut angle = rotate.angle.radians64();
        if rotate.auto {
            angle += direction;
        }
        let frame = MotionFrame::new(box_styles, reference_box, *coord_box);
        return Some(frame.place(point, angle));
    }
    shape_transform(box_styles, reference_box, shape, *coord_box)
}

/// pyro: the point `offset-distance` along `path` (a closed path wraps around, an open one
/// clamps) and the path's direction there (radians, 0 pointing right).
fn point_along(
    path: &kurbo::BezPath,
    offset_distance: &style::values::computed::LengthPercentage,
) -> Option<(kurbo::Point, f64)> {
    use kurbo::{ParamCurve, ParamCurveArclen, ParamCurveDeriv, PathSeg};
    let closed = path.elements().iter().any(|el| matches!(el, kurbo::PathEl::ClosePath));
    let segments: Vec<PathSeg> = path.segments().collect();
    const ACCURACY: f64 = 1e-7;
    let lengths: Vec<f64> = segments.iter().map(|s| s.arclen(ACCURACY)).collect();
    let total: f64 = lengths.iter().sum();
    let distance = offset_distance.resolve(CSSPixelLength::new(total as f32)).px() as f64;
    let distance = if closed && total > 0.0 { distance.rem_euclid(total) } else { distance.clamp(0.0, total) };
    if segments.is_empty() {
        let start = match path.elements().first() {
            Some(kurbo::PathEl::MoveTo(p)) => *p,
            _ => kurbo::Point::ZERO,
        };
        return Some((start, 0.0));
    }
    let mut left = distance;
    for (index, (seg, len)) in segments.iter().zip(&lengths).enumerate() {
        if left <= *len || index + 1 == segments.len() {
            let t = if *len > 0.0 { seg.inv_arclen(left.min(*len), ACCURACY) } else { 0.0 };
            let tangent = match seg {
                PathSeg::Line(l) => l.deriv().eval(t).to_vec2(),
                PathSeg::Quad(q) => q.deriv().eval(t).to_vec2(),
                PathSeg::Cubic(c) => c.deriv().eval(t).to_vec2(),
            };
            return Some((seg.eval(t), tangent.y.atan2(tangent.x)));
        }
        left -= len;
    }
    None
}

/// pyro: the containing block of the box whose transform is being resolved, for `offset-path:
/// ray()`: its border box in the box's border-box coordinates, and its border widths and paddings
/// (top, right, bottom, left). Set by `Node::set_transform` around `resolve_transform`.
#[derive(Clone, Copy, Debug)]
pub struct MotionContainingBlock {
    pub border_box: kurbo::Rect,
    pub border: [f64; 4],
    pub padding: [f64; 4],
    /// Its border radii (top-left, top-right, bottom-right, bottom-left), resolved.
    pub radii: [kurbo::Vec2; 4],
}

thread_local! {
    pub static MOTION_CONTAINING_BLOCK: std::cell::Cell<Option<MotionContainingBlock>> =
        const { std::cell::Cell::new(None) };
    /// pyro: the geometry of the SVG shape element an `offset-path: url()` names, in its user
    /// units (`None`: it names none). Set by `BaseDocument::resolve_transforms`.
    pub static MOTION_URL_PATH: std::cell::RefCell<Option<kurbo::BezPath>> =
        const { std::cell::RefCell::new(None) };
}

/// pyro: the offset transform of `offset-path: ray()` (CSS Motion Path 1 § 2.1, as Blink's
/// `ApplyMotionPathTransform`, measured on Chrome), in the frame of the transform origin. The ray
/// starts at its `at` position, else at `offset-position` (`auto`: the box's top-left corner,
/// `normal`: the centre of the containing block's coord box, a position: in that box); a
/// percentage `offset-distance` is of the ray's length by its size keyword, against that box. The
/// anchor point (`offset-anchor`, `auto`: the transform origin) goes to the point that far along
/// the ray, and the box turns by `offset-rotate` (`auto`: the ray's direction, 0deg pointing
/// right). `contain` is not applied.
fn ray_transform(
    box_styles: &BoxStyleStruct,
    reference_box: Rect<CSSPixelLength>,
    ray: &style::values::computed::motion::RayFunction,
    coord_box: style::values::specified::motion::CoordBox,
) -> Option<Affine> {
    use style::values::generics::motion::RaySize;
    use style::values::generics::position::GenericPositionOrAuto;
    let theta = ray.angle.degrees() as f64;
    if !theta.is_finite() {
        return None;
    }
    let frame = MotionFrame::new(box_styles, reference_box, coord_box);
    let area = frame.area;
    let start = match &ray.position {
        GenericPositionOrAuto::Position(p) => frame.in_area(p),
        GenericPositionOrAuto::Auto => frame.starting_position(box_styles, kurbo::Point::ZERO),
    };
    let (sin, cos) = theta.to_radians().sin_cos();
    let direction = kurbo::Vec2::new(sin, -cos);
    let dx = [(start.x - area.x0).abs(), (area.x1 - start.x).abs()];
    let dy = [(start.y - area.y0).abs(), (area.y1 - start.y).abs()];
    let corners = [(dx[0], dy[0]), (dx[1], dy[0]), (dx[0], dy[1]), (dx[1], dy[1])].map(|(x, y)| x.hypot(y));
    let length = match ray.size {
        RaySize::ClosestSide => dx[0].min(dx[1]).min(dy[0]).min(dy[1]),
        RaySize::FarthestSide => dx[0].max(dx[1]).max(dy[0]).max(dy[1]),
        RaySize::ClosestCorner => corners.into_iter().fold(f64::INFINITY, f64::min),
        RaySize::FarthestCorner => corners.into_iter().fold(0.0, f64::max),
        RaySize::Sides => {
            if !area.contains(start) && !(start.x == area.x1 || start.y == area.y1) {
                0.0
            } else {
                let axis = |d: f64, from: f64, lo: f64, hi: f64| {
                    if d > 1e-12 {
                        (hi - from) / d
                    } else if d < -1e-12 {
                        (lo - from) / d
                    } else {
                        f64::INFINITY
                    }
                };
                axis(direction.x, start.x, area.x0, area.x1).min(axis(direction.y, start.y, area.y0, area.y1)).max(0.0)
            }
        }
    };
    let length = if length.is_finite() { length } else { 0.0 };
    let mut distance = box_styles.offset_distance.resolve(CSSPixelLength::new(length as f32)).px() as f64;
    let rotate = &box_styles.offset_rotate;
    let mut angle = rotate.angle.radians64();
    if rotate.auto {
        angle += (theta - 90.0).to_radians();
    }
    if ray.contain {
        // The box's corners about the anchor point, turned: each must stay within the circle of
        // the ray's length about its start, which bounds the distance to an interval per corner.
        let size = (reference_box.width().px() as f64, reference_box.height().px() as f64);
        let corners = [(0.0, 0.0), (size.0, 0.0), (size.0, size.1), (0.0, size.1)]
            .map(|(x, y)| Affine::rotate(angle) * (kurbo::Point::new(x, y) - frame.anchor.to_vec2()))
            .map(|p| p.to_vec2());
        let range = |radius: f64| {
            corners.iter().try_fold((f64::NEG_INFINITY, f64::INFINITY), |(lo, hi), c| {
                let b = direction.dot(*c);
                let disc = b * b - (c.hypot2() - radius * radius);
                if disc < 0.0 {
                    return None;
                }
                let root = disc.sqrt();
                let (a, z) = (lo.max(-b - root), hi.min(-b + root));
                (a <= z).then_some((a, z))
            })
        };
        // Chrome keeps half the box's larger side from the end of the ray, whatever the anchor
        // and direction (measured); the circle test decides only whether the box fits at all.
        let half = size.0.max(size.1) / 2.0;
        distance = match range(length) {
            Some(_) if length >= half => distance.clamp(-(length - half), length - half),
            Some((lo, hi)) => distance.clamp(lo, hi),
            // Too big to fit at any distance: the shortest ray it fits in, at the one distance
            // it fits there (CSS Motion Path 1: the path length is increased).
            None => {
                let (mut lo, mut hi) = (length, corners.iter().map(|c| c.hypot()).fold(length, f64::max) * 2.0 + 1.0);
                for _ in 0..60 {
                    let mid = (lo + hi) / 2.0;
                    if range(mid).is_some() { hi = mid } else { lo = mid }
                }
                range(hi).map_or(0.0, |(a, z)| (a + z) / 2.0)
            }
        };
    }
    let point = start + direction * distance;
    Some(frame.place(point, angle))
}

/// pyro: where an offset path is laid out for a box (CSS Motion Path 1): its containing block's
/// coord box, the transform origin and the anchor point, in the box's border-box coordinates.
struct MotionFrame {
    area: kurbo::Rect,
    origin: kurbo::Point,
    anchor: kurbo::Point,
}

impl MotionFrame {
    fn new(
        box_styles: &BoxStyleStruct,
        reference_box: Rect<CSSPixelLength>,
        coord_box: style::values::specified::motion::CoordBox,
    ) -> Self {
        use style::values::generics::position::GenericPositionOrAuto;
        use style::values::specified::motion::CoordBox;
        let lp = |v: &style::values::computed::LengthPercentage, basis: f64| v.resolve(CSSPixelLength::new(basis as f32)).px() as f64;
        let (width, height) = (reference_box.width().px() as f64, reference_box.height().px() as f64);
        let origin = kurbo::Point::new(
            reference_box.origin.x.px() as f64 + lp(&box_styles.transform_origin.horizontal, width),
            reference_box.origin.y.px() as f64 + lp(&box_styles.transform_origin.vertical, height),
        );
        let cb = MOTION_CONTAINING_BLOCK.with(|c| c.get());
        let area = cb.map_or(kurbo::Rect::ZERO, |cb| {
            let inset = |r: kurbo::Rect, e: [f64; 4]| kurbo::Rect::new(r.x0 + e[3], r.y0 + e[0], r.x1 - e[1], r.y1 - e[2]);
            match coord_box {
                CoordBox::ContentBox => inset(inset(cb.border_box, cb.border), cb.padding),
                CoordBox::PaddingBox => inset(cb.border_box, cb.border),
                _ => cb.border_box,
            }
        });
        let anchor = match &box_styles.offset_anchor {
            GenericPositionOrAuto::Auto => origin,
            GenericPositionOrAuto::Position(p) => kurbo::Point::new(lp(&p.horizontal, width), lp(&p.vertical, height)),
        };
        MotionFrame { area, origin, anchor }
    }

    fn in_area(&self, p: &style::values::computed::Position) -> kurbo::Point {
        let lp = |v: &style::values::computed::LengthPercentage, basis: f64| v.resolve(CSSPixelLength::new(basis as f32)).px() as f64;
        kurbo::Point::new(self.area.x0 + lp(&p.horizontal, self.area.width()), self.area.y0 + lp(&p.vertical, self.area.height()))
    }

    /// `offset-position`: `auto` is `auto_point`, `normal` the coord box's centre.
    fn starting_position(&self, box_styles: &BoxStyleStruct, auto_point: kurbo::Point) -> kurbo::Point {
        use style::values::generics::motion::GenericOffsetPosition;
        match &box_styles.offset_position {
            GenericOffsetPosition::Auto => auto_point,
            GenericOffsetPosition::Normal => self.area.center(),
            GenericOffsetPosition::Position(p) => self.in_area(p),
        }
    }

    /// The anchor point at `point` turned by `angle`, in the frame of the transform origin. An
    /// angle within 1e-4 rad of a quarter turn is that quarter turn: a path's tangent (a cubic's
    /// derivative at a segment's end) is a hair off, and the rotation then antialiased edges
    /// Chrome draws square.
    fn place(&self, point: kurbo::Point, angle: f64) -> Affine {
        let quarter = std::f64::consts::FRAC_PI_2;
        let turns = (angle / quarter).round();
        let rotation = if (angle - turns * quarter).abs() < 1e-4 {
            // Exact sine and cosine of a multiple of 90deg.
            match (turns as i64).rem_euclid(4) {
                0 => Affine::IDENTITY,
                1 => Affine::new([0.0, 1.0, -1.0, 0.0, 0.0, 0.0]),
                2 => Affine::new([-1.0, 0.0, 0.0, -1.0, 0.0, 0.0]),
                _ => Affine::new([0.0, -1.0, 1.0, 0.0, 0.0, 0.0]),
            }
        } else {
            Affine::rotate(angle)
        };
        let motion = Affine::translate(point.to_vec2()) * rotation * Affine::translate(-self.anchor.to_vec2());
        let origin = Affine::translate(self.origin.to_vec2());
        origin.inverse() * motion * origin
    }
}

/// pyro: the offset transform of a basic shape (`circle()`, `ellipse()`, `inset()`/`rect()`/
/// `xywh()`, `polygon()`, `shape()`) as an offset path, laid out in the containing block's coord
/// box (as Chrome, measured): a circle or ellipse starts at its rightmost point, a rectangle at its
/// top-left corner (after its corner radius), both clockwise; a polygon is closed. A shape without a
/// position is centred at `offset-position` (`normal`: the coord box's centre, `auto`: the box's
/// top-left corner). `offset-path: <coord-box>` alone is that box of the containing block with its
/// border radii.
fn shape_transform(
    box_styles: &BoxStyleStruct,
    reference_box: Rect<CSSPixelLength>,
    shape: &style::values::computed::basic_shape::BasicShape,
    coord_box: style::values::specified::motion::CoordBox,
) -> Option<Affine> {
    use kurbo::Shape as _;
    use style::values::generics::basic_shape::{GenericBasicShape, GenericPathOrShapeFunction, GenericShapeRadius};
    use style::values::generics::position::GenericPositionOrAuto;
    let frame = MotionFrame::new(box_styles, reference_box, coord_box);
    let area = frame.area;
    let (w, h) = (area.width(), area.height());
    let lp = |v: &style::values::computed::LengthPercentage, basis: f64| v.resolve(CSSPixelLength::new(basis as f32)).px() as f64;
    let center_of = |p: &GenericPositionOrAuto<style::values::computed::Position>| match p {
        GenericPositionOrAuto::Position(p) => frame.in_area(p),
        GenericPositionOrAuto::Auto => frame.starting_position(box_styles, kurbo::Point::ZERO),
    };
    let axis_radius = |r: &GenericShapeRadius<style::values::computed::LengthPercentage>, c: f64, lo: f64, hi: f64, basis: f64| {
        let near = (c - lo).abs().min((hi - c).abs());
        let far = (c - lo).abs().max((hi - c).abs());
        match r {
            GenericShapeRadius::Length(v) => lp(&v.0, basis),
            GenericShapeRadius::ClosestSide | GenericShapeRadius::ClosestCorner => near,
            GenericShapeRadius::FarthestSide | GenericShapeRadius::FarthestCorner => far,
        }
    };
    let path = match shape {
        GenericBasicShape::Circle(c) => {
            let center = center_of(&c.position);
            let radius = match &c.radius {
                GenericShapeRadius::Length(v) => lp(&v.0, w.hypot(h) / std::f64::consts::SQRT_2),
                other => {
                    let x = axis_radius(other, center.x, area.x0, area.x1, w);
                    let y = axis_radius(other, center.y, area.y0, area.y1, h);
                    match other {
                        GenericShapeRadius::ClosestSide => x.min(y),
                        GenericShapeRadius::FarthestSide => x.max(y),
                        _ => x.hypot(y),
                    }
                }
            };
            kurbo::Circle::new(center, radius.max(0.0)).to_path(1e-6)
        }
        GenericBasicShape::Ellipse(e) => {
            let center = center_of(&e.position);
            let rx = axis_radius(&e.semiaxis_x, center.x, area.x0, area.x1, w).max(0.0);
            let ry = axis_radius(&e.semiaxis_y, center.y, area.y0, area.y1, h).max(0.0);
            kurbo::Ellipse::new(center, (rx, ry), 0.0).to_path(1e-6)
        }
        GenericBasicShape::Rect(inset) => {
            let rect = kurbo::Rect::new(
                area.x0 + lp(&inset.rect.3, w),
                area.y0 + lp(&inset.rect.0, h),
                area.x1 - lp(&inset.rect.1, w),
                area.y1 - lp(&inset.rect.2, h),
            );
            let r = |c: &style::values::computed::BorderCornerRadius| kurbo::Vec2::new(lp(&c.0.width.0, w), lp(&c.0.height.0, h));
            let round = &inset.round;
            rounded_rect_path(rect, [r(&round.top_left), r(&round.top_right), r(&round.bottom_right), r(&round.bottom_left)])
        }
        GenericBasicShape::Polygon(poly) => {
            let mut path = kurbo::BezPath::new();
            for (i, c) in poly.coordinates.iter().enumerate() {
                let p = kurbo::Point::new(area.x0 + lp(&c.0, w), area.y0 + lp(&c.1, h));
                if i == 0 { path.move_to(p) } else { path.line_to(p) }
            }
            path.close_path();
            path
        }
        GenericBasicShape::PathOrShape(GenericPathOrShapeFunction::Shape(shape)) => {
            let agnostic = w.hypot(h) / std::f64::consts::SQRT_2;
            let mut path = crate::shape_path::svg_path_to_bezpath(
                &shape.commands,
                |v: &style::values::computed::LengthPercentage| lp(v, w),
                |v: &style::values::computed::LengthPercentage| lp(v, h),
                |v: &style::values::computed::LengthPercentage| lp(v, agnostic),
                |a: &style::values::computed::Angle| a.degrees() as f64,
            )?;
            path.apply_affine(Affine::translate(area.origin().to_vec2()));
            path
        }
        GenericBasicShape::PathOrShape(GenericPathOrShapeFunction::Path(_)) => return None,
    };
    let (point, direction) = point_along(&path, &box_styles.offset_distance)?;
    let rotate = &box_styles.offset_rotate;
    let mut angle = rotate.angle.radians64();
    if rotate.auto {
        angle += direction;
    }
    Some(frame.place(point, angle))
}

/// pyro: `offset-path: <coord-box>`: the containing block's coord box with its border radii (less
/// the border, and the padding, inside), from its top-left corner after the radius, clockwise.
fn coord_box_transform(
    box_styles: &BoxStyleStruct,
    reference_box: Rect<CSSPixelLength>,
    coord_box: style::values::specified::motion::CoordBox,
) -> Option<Affine> {
    use style::values::specified::motion::CoordBox;
    let frame = MotionFrame::new(box_styles, reference_box, coord_box);
    let cb = MOTION_CONTAINING_BLOCK.with(|c| c.get())?;
    // How far the coord box is inside the border box at each side (top, right, bottom, left).
    let inset = match coord_box {
        CoordBox::ContentBox => std::array::from_fn(|i| cb.border[i] + cb.padding[i]),
        CoordBox::PaddingBox => cb.border,
        _ => [0.0; 4],
    };
    let [t, r, b, l] = inset;
    let shrink = |radius: kurbo::Vec2, dx: f64, dy: f64| kurbo::Vec2::new((radius.x - dx).max(0.0), (radius.y - dy).max(0.0));
    let radii = [
        shrink(cb.radii[0], l, t),
        shrink(cb.radii[1], r, t),
        shrink(cb.radii[2], r, b),
        shrink(cb.radii[3], l, b),
    ];
    let path = rounded_rect_path(frame.area, radii);
    let (point, direction) = point_along(&path, &box_styles.offset_distance)?;
    let rotate = &box_styles.offset_rotate;
    let mut angle = rotate.angle.radians64();
    if rotate.auto {
        angle += direction;
    }
    Some(frame.place(point, angle))
}

/// pyro: a rectangle with elliptical corners (top-left, top-right, bottom-right, bottom-left) as a path that starts at the
/// top edge after the top-left corner and runs clockwise; radii that do not fit are scaled down
/// together (CSS Backgrounds § 5.5).
pub(crate) fn rounded_rect_path(rect: kurbo::Rect, radii: [kurbo::Vec2; 4]) -> kurbo::BezPath {
    use kurbo::{Arc, Point, Vec2};
    let (w, h) = (rect.width().max(0.0), rect.height().max(0.0));
    let [tl, tr, br, bl] = radii.map(|r| Vec2::new(r.x.max(0.0), r.y.max(0.0)));
    let mut f: f64 = 1.0;
    for (sum, side) in [(tl.x + tr.x, w), (bl.x + br.x, w), (tl.y + bl.y, h), (tr.y + br.y, h)] {
        if sum > side && sum > 0.0 {
            f = f.min(side / sum);
        }
    }
    let [tl, tr, br, bl] = [tl * f, tr * f, br * f, bl * f];
    let mut path = kurbo::BezPath::new();
    let corner = |path: &mut kurbo::BezPath, center: Point, radii: Vec2, start: f64| {
        if radii.x > 0.0 && radii.y > 0.0 {
            let arc = Arc::new(center, radii, start, std::f64::consts::FRAC_PI_2, 0.0);
            for el in arc.append_iter(0.01) {
                path.push(el);
            }
        }
    };
    use std::f64::consts::{FRAC_PI_2, PI};
    path.move_to((rect.x0 + tl.x, rect.y0));
    path.line_to((rect.x1 - tr.x, rect.y0));
    corner(&mut path, Point::new(rect.x1 - tr.x, rect.y0 + tr.y), tr, -FRAC_PI_2);
    path.line_to((rect.x1, rect.y1 - br.y));
    corner(&mut path, Point::new(rect.x1 - br.x, rect.y1 - br.y), br, 0.0);
    path.line_to((rect.x0 + bl.x, rect.y1));
    corner(&mut path, Point::new(rect.x0 + bl.x, rect.y1 - bl.y), bl, FRAC_PI_2);
    path.line_to((rect.x0, rect.y0 + tl.y));
    corner(&mut path, Point::new(rect.x0 + tl.x, rect.y0 + tl.y), tl, PI);
    path.close_path();
    path
}

/// pyro: a linear coefficient within 1e-12 of 0, 1 or -1 is that value, and a translation
/// within 1e-9 px of a whole pixel is on it: the 3D paths' counterpart of the 2D path's
/// snapping above. A rotation by a multiple of 90deg leaves rounding noise (sin 180deg =
/// 1.2e-16 in f64) that made an axis-aligned transform a rotation, which text then draws
/// without its pixel snapping; Chromium computes those angles exactly.
pub(crate) fn snap(affine: Affine) -> Affine {
    let [a, b, c, d, e, f] = affine.as_coeffs();
    let linear = |v: f64| {
        let r = v.round();
        if r.abs() <= 1.0 && (v - r).abs() < 1e-12 {
            r
        } else {
            v
        }
    };
    let translation = |v: f64| {
        let r = v.round();
        if (v - r).abs() < 1e-9 { r } else { v }
    };
    Affine::new([
        linear(a),
        linear(b),
        linear(c),
        linear(d),
        translation(e),
        translation(f),
    ])
}

/// pyro: whether the transformation matrix leaves the plane z = 0 (or flips its z axis,
/// which only `backface-visibility` sees): a 3D matrix of the transform list (`list`), a
/// `translate` with a depth, a `rotate` about an axis other than z, a `scale` with a z factor
/// other than 1.
fn is_3d(box_styles: &BoxStyleStruct, list: Option<&Transform3D<f32>>) -> bool {
    let translate_z =
        matches!(&box_styles.translate, Translate::Translate(_, _, z) if z.px() != 0.0);
    let rotate_3d = matches!(
        &box_styles.rotate,
        Rotate::Rotate3D(x, y, _, angle) if (*x != 0.0 || *y != 0.0) && angle.radians64() != 0.0
    );
    let scale_z = matches!(&box_styles.scale, Scale::Scale(_, _, z) if *z != 1.0);
    translate_z || rotate_3d || scale_z || list.is_some_and(|t| !t.is_2d())
}

/// pyro: the transformation matrix as a 4x4 matrix in f64 (euclid's row vectors: a point is
/// `p * M`, so `a.then(&b)` applies `a` first).
fn resolve_transform_3d(
    box_styles: &BoxStyleStruct,
    reference_box: Rect<CSSPixelLength>,
) -> Transform3D<f64> {
    let mut resolved = Transform3D::identity();

    if !box_styles.transform.0.is_empty() {
        if let Ok((t, _)) = box_styles
            .transform
            .to_transform_3d_matrix_f64(Some(&reference_box))
        {
            resolved = t;
        }
    }

    // `resolved` is applied first, so the properties are applied in reverse order of the list
    // above: scale, rotate, translate.
    if let Scale::Scale(x, y, z) = &box_styles.scale {
        resolved = resolved.then(&Transform3D::scale(*x as f64, *y as f64, *z as f64));
    }

    match &box_styles.rotate {
        Rotate::None => {}
        Rotate::Rotate(angle) => {
            resolved = resolved.then(&Transform3D::rotation(
                0.0,
                0.0,
                1.0,
                euclid::Angle::radians(angle.radians64()),
            ));
        }
        Rotate::Rotate3D(x, y, z, angle) => {
            let (x, y, z, angle) = get_normalized_vector_and_angle(*x, *y, *z, angle.radians64());
            resolved = resolved.then(&Transform3D::rotation(
                x as f64,
                y as f64,
                z as f64,
                euclid::Angle::radians(angle),
            ));
        }
    }

    if let Translate::Translate(x, y, z) = &box_styles.translate {
        resolved = resolved.then(&Transform3D::translation(
            x.resolve(reference_box.width()).px() as f64,
            y.resolve(reference_box.height()).px() as f64,
            z.px() as f64,
        ));
    }

    let origin = &box_styles.transform_origin;
    let (ox, oy, oz) = (
        reference_box.origin.x.px() as f64
            + origin.horizontal.resolve(reference_box.width()).px() as f64,
        reference_box.origin.y.px() as f64
            + origin.vertical.resolve(reference_box.height()).px() as f64,
        origin.depth.px() as f64,
    );
    Transform3D::translation(-ox, -oy, -oz)
        .then(&resolved)
        .then(&Transform3D::translation(ox, oy, oz))
}

/// pyro: the perspective matrix an element with `perspective` gives the boxes it is the
/// containing block of (CSS Transforms 2 §6), in its border box's coordinates (CSS px):
///
///   - Start with the identity matrix.
///   - Translate by the computed X and Y values of perspective-origin.
///   - Multiply by the matrix that would be obtained from the perspective() transform function,
///     where the length is provided by the value of the perspective property.
///   - Translate by the negated computed X and Y values of perspective-origin.
///
/// A length below 1px is 1px, as for `perspective()` (`create_perspective_matrix`).
pub fn resolve_perspective_matrix(
    box_styles: &BoxStyleStruct,
    reference_box: Rect<CSSPixelLength>,
) -> Option<Transform3D<f64>> {
    let GenericPerspective::Length(length) = &box_styles.perspective else {
        return None;
    };
    let d = (length.0.px() as f64).max(1.0);
    let origin = &box_styles.perspective_origin;
    let (ox, oy) = (
        origin.horizontal.resolve(reference_box.width()).px() as f64,
        origin.vertical.resolve(reference_box.height()).px() as f64,
    );
    Some(
        Transform3D::translation(-ox, -oy, 0.0)
            .then(&Transform3D::perspective(d))
            .then(&Transform3D::translation(ox, oy, 0.0)),
    )
}

/// pyro: what a box of `size` (CSS px) under a 3D matrix shows in the plane of the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Flattened {
    /// The 2D affine to composite it with: exact when the box stays parallel to the screen
    /// (no perspective, or a perspective with only a depth and a scale), the closest affine
    /// to its projection otherwise (a rotation out of the plane under perspective).
    Affine(Affine),
    /// It lies behind the viewer: nothing of it is drawn.
    Hidden,
    /// pyro (lane H2): it crosses the viewer's plane: the part in front of the viewer is drawn
    /// (blitz-paint draws it through the 4x4 matrix, `Node::plane_3d`, cut there). Its projection
    /// has no bounds, so no 2D affine stands in for it: layout keeps its untransformed box.
    Crossing,
}

/// pyro: flattens `m` onto the plane z = 0, as a browser composites an element whose content
/// is flat (`transform-style: flat`): a point (x, y) of the box maps to (X / W, Y / W) with
/// `(X, Y, Z, W) = (x, y, 0, 1) * m`, the 3x3 matrix of rows and columns 1, 2 and 4.
///
/// W does not vary over the box (`m14 = m24 = 0`: no perspective, or a perspective with the
/// box parallel to the screen) and the result is affine. Otherwise it is projective, which a
/// 2D affine cannot draw: the affine closest to it at the box's corners (least squares)
/// stands in, so the box takes about the place and size of its true projection.
pub fn flatten_transform(m: &Transform3D<f64>, size: (f64, f64)) -> Flattened {
    let (w, h) = (size.0.max(1.0), size.1.max(1.0));
    let projective = is_projective(m, size);
    if !projective {
        if !(m.m44 > 0.0) {
            return Flattened::Hidden;
        }
        let k = 1.0 / m.m44;
        return Flattened::Affine(snap(Affine::new([
            m.m11 * k,
            m.m12 * k,
            m.m21 * k,
            m.m22 * k,
            m.m41 * k,
            m.m42 * k,
        ])));
    }
    let project = |x: f64, y: f64| -> Option<(f64, f64)> {
        let wv = x * m.m14 + y * m.m24 + m.m44;
        // A corner at or behind the viewer's plane has no image (Chromium clips the box there).
        (wv > 1e-6).then(|| {
            (
                (x * m.m11 + y * m.m21 + m.m41) / wv,
                (x * m.m12 + y * m.m22 + m.m42) / wv,
            )
        })
    };
    let (Some(p00), Some(p10), Some(p01), Some(p11)) = (
        project(0.0, 0.0),
        project(w, 0.0),
        project(0.0, h),
        project(w, h),
    ) else {
        // pyro (lane H2): a box that crosses the viewer's plane shows the part in front of it
        // (`Flattened::Crossing`). A box wholly behind the viewer shows nothing.
        return if clipped_projection(m, (w, h)).len() < 3 {
            Flattened::Hidden
        } else {
            Flattened::Crossing
        };
    };
    // Least squares over the four corners of a w x h box: each column of the linear part is
    // the mean difference across the box along that axis, the translation puts the box's
    // centre at the corners' mean.
    let a = (
        (p10.0 + p11.0 - p00.0 - p01.0) / (2.0 * w),
        (p10.1 + p11.1 - p00.1 - p01.1) / (2.0 * w),
    );
    let c = (
        (p01.0 + p11.0 - p00.0 - p10.0) / (2.0 * h),
        (p01.1 + p11.1 - p00.1 - p10.1) / (2.0 * h),
    );
    let mean = (
        (p00.0 + p10.0 + p01.0 + p11.0) / 4.0,
        (p00.1 + p10.1 + p01.1 + p11.1) / 4.0,
    );
    let e = (
        mean.0 - a.0 * w / 2.0 - c.0 * h / 2.0,
        mean.1 - a.1 * w / 2.0 - c.1 * h / 2.0,
    );
    Flattened::Affine(Affine::new([a.0, a.1, c.0, c.1, e.0, e.1]))
}

/// pyro (lane H2): whether `m` maps a box of `size` (CSS px) to the screen projectively: W varies
/// over the box (a rotation out of the plane under perspective), so no 2D affine draws it.
pub fn is_projective(m: &Transform3D<f64>, size: (f64, f64)) -> bool {
    let (w, h) = (size.0.max(1.0), size.1.max(1.0));
    // A W that varies by less than this over the box moves no point by a visible amount.
    (m.m14.abs() * w + m.m24.abs() * h) > 1e-9 * m.m44.abs()
}

/// pyro (lane H2): the smallest W a point may have and be drawn: the plane is clipped there
/// (Chromium's `gfx::kEpsilon`-style clipping of quads that cross the viewer's plane).
pub const MIN_W: f64 = 1e-5;

/// pyro (lane H2): the projection of a box of `size` (CSS px) under `m`, the part behind the
/// viewer (W below [`MIN_W`]) cut away first: a convex polygon in the plane of the screen, empty
/// when nothing of the box is in front of the viewer.
pub fn clipped_projection(m: &Transform3D<f64>, size: (f64, f64)) -> Vec<(f64, f64)> {
    let corners = [(0.0, 0.0), (size.0, 0.0), (size.0, size.1), (0.0, size.1)];
    let homogeneous: Vec<[f64; 3]> = corners
        .iter()
        .map(|&(x, y)| {
            [
                x * m.m11 + y * m.m21 + m.m41,
                x * m.m12 + y * m.m22 + m.m42,
                x * m.m14 + y * m.m24 + m.m44,
            ]
        })
        .collect();
    let mut out = Vec::with_capacity(6);
    for i in 0..homogeneous.len() {
        let (a, b) = (homogeneous[i], homogeneous[(i + 1) % homogeneous.len()]);
        let (a_in, b_in) = (a[2] >= MIN_W, b[2] >= MIN_W);
        if a_in {
            out.push((a[0] / a[2], a[1] / a[2]));
        }
        if a_in != b_in {
            let t = (MIN_W - a[2]) / (b[2] - a[2]);
            let p = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            out.push((p[0] / MIN_W, p[1] / MIN_W));
        }
    }
    out
}

/// pyro: whether the back face of a box under `m` faces the viewer, as Chromium decides it
/// (`gfx::Transform::IsBackFaceVisible`): the z component of the box's normal (0, 0, 1)
/// transformed by the inverse transpose of `m` is negative. That component is the cofactor of
/// m33 over the determinant, and the cofactor is the determinant of the flattened 3x3 matrix
/// (rows and columns 1, 2 and 4), so the sign of their product decides. A matrix with no
/// inverse shows no back face.
pub fn is_back_face_visible(m: &Transform3D<f64>) -> bool {
    let determinant = m.determinant();
    if determinant == 0.0 {
        return false;
    }
    let cofactor_33 = m.m11 * (m.m22 * m.m44 - m.m24 * m.m42)
        - m.m12 * (m.m21 * m.m44 - m.m24 * m.m41)
        + m.m14 * (m.m21 * m.m42 - m.m22 * m.m41);
    cofactor_33 * determinant < -f64::EPSILON
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Affine, b: Affine) -> bool {
        a.as_coeffs()
            .iter()
            .zip(b.as_coeffs())
            .all(|(x, y)| (x - y).abs() < 1e-9)
    }

    #[test]
    fn rotations_out_of_the_plane_flatten_to_their_orthographic_projection() {
        // rotateY(60deg): the x axis shrinks by cos 60 = 0.5, y is untouched.
        let m = Transform3D::rotation(0.0, 1.0, 0.0, euclid::Angle::degrees(60.0));
        let Flattened::Affine(a) = flatten_transform(&m, (100.0, 100.0)) else {
            panic!()
        };
        assert!(
            close(a, Affine::new([0.5, 0.0, 0.0, 1.0, 0.0, 0.0])),
            "{a:?}"
        );
        // rotateX(180deg) mirrors y, and shows the back face; scaleZ(-1) changes nothing in
        // the plane but shows the back face too; a 2D mirror does not.
        let m = Transform3D::rotation(1.0, 0.0, 0.0, euclid::Angle::degrees(180.0));
        let Flattened::Affine(a) = flatten_transform(&m, (100.0, 100.0)) else {
            panic!()
        };
        assert!(
            close(a, Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, 0.0])),
            "{a:?}"
        );
        assert!(is_back_face_visible(&m));
        assert!(is_back_face_visible(&Transform3D::scale(1.0, 1.0, -1.0)));
        assert!(!is_back_face_visible(&Transform3D::scale(-1.0, 1.0, 1.0)));
        assert!(!is_back_face_visible(&Transform3D::rotation(
            0.0,
            1.0,
            0.0,
            euclid::Angle::degrees(89.0)
        )));
        assert!(is_back_face_visible(&Transform3D::rotation(
            0.0,
            1.0,
            0.0,
            euclid::Angle::degrees(91.0)
        )));
    }

    #[test]
    fn a_depth_under_perspective_scales_about_the_perspective_origin() {
        // perspective: 3px with its origin at (75, 75), a box translated 1px towards the
        // viewer: scaled by d / (d - z) = 1.5 about the origin.
        let perspective = Transform3D::translation(-75.0, -75.0, 0.0)
            .then(&Transform3D::perspective(3.0))
            .then(&Transform3D::translation(75.0, 75.0, 0.0));
        let m = Transform3D::translation(0.0, 0.0, 1.0).then(&perspective);
        let Flattened::Affine(a) = flatten_transform(&m, (100.0, 100.0)) else {
            panic!()
        };
        assert!(
            close(a, Affine::new([1.5, 0.0, 0.0, 1.5, -37.5, -37.5])),
            "{a:?}"
        );
        // At the viewer's distance or beyond, nothing is drawn.
        let m = Transform3D::translation(0.0, 0.0, 3.0).then(&perspective);
        assert_eq!(flatten_transform(&m, (100.0, 100.0)), Flattened::Hidden);
    }

    #[test]
    fn a_rotation_under_perspective_gets_the_affine_closest_to_its_projection() {
        // rotateX(45deg) under perspective(1000px), origin at the box's centre: the
        // projection is a trapezoid from y = 50 - 34.15 (its far edge) to 50 + 36.65 (its
        // near edge). The stand-in spans the same rows, and differs from the flat projection.
        let centre = |m: Transform3D<f64>| {
            Transform3D::translation(-50.0, -50.0, 0.0)
                .then(&m)
                .then(&Transform3D::translation(50.0, 50.0, 0.0))
        };
        let rotate = Transform3D::rotation(1.0, 0.0, 0.0, euclid::Angle::degrees(45.0));
        let m = centre(rotate.then(&Transform3D::perspective(1000.0)));
        let Flattened::Affine(a) = flatten_transform(&m, (100.0, 100.0)) else {
            panic!()
        };
        let Flattened::Affine(flat) = flatten_transform(&centre(rotate), (100.0, 100.0)) else {
            panic!()
        };
        let (top, bottom) = (
            a * kurbo::Point::new(50.0, 0.0),
            a * kurbo::Point::new(50.0, 100.0),
        );
        assert!(
            (top.y - 15.85).abs() < 0.01 && (bottom.y - 86.65).abs() < 0.01,
            "{top:?} {bottom:?}"
        );
        assert!((top.x - 50.0).abs() < 1e-9, "{top:?}");
        assert!(!close(a, flat));
    }

    /// pyro (lane H2): a box laid down under a perspective whose far side reaches past the viewer
    /// shows the part in front of the viewer (it was hidden): it crosses the viewer's plane, which
    /// no 2D affine stands in for, and it is projective (blitz-paint draws it through its matrix).
    #[test]
    fn a_box_crossing_the_viewer_shows_the_part_in_front() {
        // perspective(100px) seen from 50px above the box's top edge; the box turned 90deg about
        // that edge, its far side 100px towards the viewer (W = 0 there).
        let perspective = Transform3D::translation(-50.0, 50.0, 0.0)
            .then(&Transform3D::perspective(100.0))
            .then(&Transform3D::translation(50.0, -50.0, 0.0));
        let crossing = [90.0, -90.0]
            .into_iter()
            .map(|deg| Transform3D::rotation(1.0, 0.0, 0.0, euclid::Angle::degrees(deg)).then(&perspective))
            .find(|m| 100.0 * m.m24 + m.m44 < 1e-9)
            .expect("one of the turns brings the far edge to the viewer");
        assert!(is_projective(&crossing, (100.0, 100.0)));
        let visible = clipped_projection(&crossing, (100.0, 100.0));
        assert!(visible.len() >= 3, "{visible:?}");
        // The near edge stays on the top edge; the cut edge lies far below it.
        assert!(visible.iter().any(|&(_, y)| y.abs() < 1e-9), "{visible:?}");
        assert!(visible.iter().any(|&(_, y)| y > 1e5), "{visible:?}");
        assert_eq!(flatten_transform(&crossing, (100.0, 100.0)), Flattened::Crossing);
        // Turned and moved past the viewer it lies wholly behind: hidden.
        let behind = Transform3D::rotation(0.0, 1.0, 0.0, euclid::Angle::degrees(30.0))
            .then(&Transform3D::translation(0.0, 0.0, 300.0))
            .then(&Transform3D::perspective(100.0));
        assert!(is_projective(&behind, (100.0, 100.0)));
        assert!(clipped_projection(&behind, (100.0, 100.0)).is_empty());
        assert_eq!(flatten_transform(&behind, (100.0, 100.0)), Flattened::Hidden);
    }
}
