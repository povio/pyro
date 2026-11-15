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

    // TODO: support the "offset" property
    // <https://developer.mozilla.org/en-US/docs/Web/CSS/Reference/Properties/offset>

    if translate.is_none() && rotate.is_none() && scale_transform.is_none() && transform.is_none() {
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

    if let Some(transform) = transform {
        resolved *= transform;
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
    // A W that varies by less than this over the box moves no point by a visible amount.
    let projective = (m.m14.abs() * w + m.m24.abs() * h) > 1e-9 * m.m44.abs();
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
        return Flattened::Hidden;
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
}
