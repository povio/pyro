use super::ElementCx;
use crate::kurbo_css::{CssBox, NonUniformRoundedRectRadii};
use anyrender::PaintScene;
use kurbo::{Affine, BezPath, Insets, Point, Rect, Shape, SvgArc, Vec2};
use peniko::Fill;
use style::values::computed::basic_shape::{BasicShape, ClipPath};
use style::values::computed::{Angle, CSSPixelLength, LengthPercentage};
use style::values::generics::basic_shape::{
    ArcSize, ArcSweep, AxisEndPoint, CommandEndPoint, ControlPoint, ControlReference, FillRule,
    GenericBasicShape, GenericPathOrShapeFunction, GenericShapeCommand, GenericShapeRadius,
    ShapeBox, ShapeGeometryBox,
};
use style::values::generics::border::GenericBorderRadius;
use style::values::generics::position::{GenericPosition, GenericPositionOrAuto};

/// pyro: what an element's `clip-path` leaves of everything it paints (its shadows, box, content,
/// descendants and outline), in CSS px from its border box's origin.
pub(super) enum ElementClip {
    /// Nothing is left: a shape of no area (an `inset()` past its box, an inside-out `rect()`, a
    /// circle of radius 0), or a reference box of no area.
    Nothing,
    /// One shape, filled with its fill rule.
    Shape(BezPath, Fill),
}

impl ElementClip {
    fn shape(path: BezPath, fill: Fill) -> Self {
        let bounds = path.bounding_box();
        if bounds.width() > 0.0 && bounds.height() > 0.0 && bounds.is_finite() {
            ElementClip::Shape(path, fill)
        } else {
            ElementClip::Nothing
        }
    }
}

/// pyro: paints `paint` clipped by an element's `clip-path` (none: unclipped). `transform` is the
/// element's (its border box's origin), `scale` the device scale.
pub(super) fn with_element_clip<S: PaintScene>(
    scene: &mut S,
    clip: Option<&ElementClip>,
    transform: Affine,
    scale: f64,
    paint: impl FnOnce(&mut S),
) {
    match clip {
        None => paint(scene),
        // A clip that leaves nothing paints nothing.
        Some(ElementClip::Nothing) => {}
        Some(ElementClip::Shape(path, fill)) => {
            scene.push_clip_layer(*fill, transform * Affine::scale(scale), path);
            paint(scene);
            scene.pop_layer();
        }
    }
}

impl ElementCx<'_, '_> {
    /// pyro: the visible area (the viewport) in this element's space, in device px; empty under
    /// a transform that cannot be inverted.
    pub(super) fn visible_area(&self) -> Rect {
        let painter = self.context;
        let viewport = Rect::new(
            painter.initial_x,
            painter.initial_y,
            painter.initial_x + painter.width as f64,
            painter.initial_y + painter.height as f64,
        );
        if self.transform.determinant().abs() < 1e-12 {
            return Rect::ZERO;
        }
        self.transform
            .inverse()
            .transform_rect_bbox(viewport)
            .inflate(1.0, 1.0)
    }

    /// Compute the rectangle (in scaled pixels, relative to the border box origin) described by
    /// the CSS 2 `clip` property, if it applies. `clip` only applies to absolutely positioned
    /// elements; `auto` components resolve to the corresponding border box edge.
    pub(super) fn css_clip_rect(&self) -> Option<Rect> {
        use style::computed_values::position::T as Position;
        use style::values::computed::LengthOrAuto;
        use style::values::generics::ClipRectOrAuto;

        if !matches!(
            self.style.get_box().position,
            Position::Absolute | Position::Fixed
        ) {
            return None;
        }
        let ClipRectOrAuto::Rect(rect) = &self.style.get_effects().clip else {
            return None;
        };

        let border_box = self.frame.border_box;
        let resolve = |component: &LengthOrAuto, auto: f64| match component {
            LengthOrAuto::Auto => auto,
            LengthOrAuto::LengthPercentage(length) => length.px() as f64 * self.scale,
        };
        let x0 = resolve(&rect.left, 0.0);
        let y0 = resolve(&rect.top, 0.0);
        let x1 = resolve(&rect.right, border_box.width());
        let y1 = resolve(&rect.bottom, border_box.height());

        Some(Rect::new(x0, y0, x1.max(x0), y1.max(y0)))
    }

    /// pyro: the clip of this element's `clip-path`, `None` for `none` and for what is not
    /// supported (a `url()` reference).
    pub(super) fn element_clip(&self) -> Option<ElementClip> {
        match self.style.get_clip_path() {
            ClipPath::None => None,
            ClipPath::Url(_) => {
                // URL references (e.g. `clip-path: url(#myClip)`) are not yet supported
                None
            }
            ClipPath::Shape(basic_shape, geometry_box) => {
                let reference = self.reference_box(geometry_box);
                if !(reference.width() >= 0.0 && reference.height() >= 0.0) {
                    return Some(ElementClip::Nothing);
                }
                Some(self.basic_shape_clip(basic_shape, reference))
            }
            // A box alone clips to its edges, rounded by the box's radii.
            ClipPath::Box(geometry_box) => Some(ElementClip::shape(
                self.rounded_reference_box(geometry_box),
                Fill::NonZero,
            )),
        }
    }

    /// The box a `<geometry-box>` names, in CSS px from the border box's origin.
    ///
    /// For elements with associated CSS layout box, the used value for fill-box is content-box
    /// and for stroke-box and view-box is border-box.
    fn reference_box(&self, geometry_box: &ShapeGeometryBox) -> Rect {
        let frame = &self.frame;
        let device = match geometry_box {
            ShapeGeometryBox::ElementDependent
            | ShapeGeometryBox::StrokeBox
            | ShapeGeometryBox::ViewBox
            | ShapeGeometryBox::ShapeBox(ShapeBox::BorderBox) => frame.border_box,
            ShapeGeometryBox::ShapeBox(ShapeBox::PaddingBox) => frame.padding_box,
            ShapeGeometryBox::FillBox | ShapeGeometryBox::ShapeBox(ShapeBox::ContentBox) => {
                frame.content_box
            }
            ShapeGeometryBox::ShapeBox(ShapeBox::MarginBox) => {
                return self.margin_box();
            }
        };
        scale_rect(device, 1.0 / self.scale)
    }

    /// The margin box, in CSS px from the border box's origin.
    fn margin_box(&self) -> Rect {
        let border_box = scale_rect(self.frame.border_box, 1.0 / self.scale);
        border_box + self.margins()
    }

    fn margins(&self) -> Insets {
        let margin = self.node.final_layout().margin;
        Insets::new(
            margin.left as f64,
            margin.top as f64,
            margin.right as f64,
            margin.bottom as f64,
        )
    }

    /// The edges of a `<geometry-box>` with its rounded corners (CSS Shapes' box shapes), in CSS
    /// px: the border radii for the border box, the inner radii for the padding and content boxes,
    /// and for the margin box the radii grown by the margins as `box-shadow`'s spread grows them
    /// (Blink's `FloatRoundedRect::OutsetForShapeMargin`).
    fn rounded_reference_box(&self, geometry_box: &ShapeGeometryBox) -> BezPath {
        let frame = &self.frame;
        let to_css = 1.0 / self.scale;
        let radii = frame.border_radii * to_css;
        let border = scale_insets(frame.border_width, to_css);
        let padding = scale_insets(frame.padding_width, to_css);
        let border_box = scale_rect(frame.border_box, to_css);
        let (rect, radii) = match geometry_box {
            ShapeGeometryBox::ElementDependent
            | ShapeGeometryBox::StrokeBox
            | ShapeGeometryBox::ViewBox
            | ShapeGeometryBox::ShapeBox(ShapeBox::BorderBox) => (border_box, radii),
            ShapeGeometryBox::ShapeBox(ShapeBox::PaddingBox) => {
                (border_box - border, inset_radii(radii, border))
            }
            ShapeGeometryBox::FillBox | ShapeGeometryBox::ShapeBox(ShapeBox::ContentBox) => {
                let inset = Insets::new(
                    border.x0 + padding.x0,
                    border.y0 + padding.y0,
                    border.x1 + padding.x1,
                    border.y1 + padding.y1,
                );
                (border_box - inset, inset_radii(radii, inset))
            }
            ShapeGeometryBox::ShapeBox(ShapeBox::MarginBox) => {
                let m = self.margins();
                let radii = NonUniformRoundedRectRadii {
                    top_left: outset_radius(radii.top_left, m.x0, m.y0),
                    top_right: outset_radius(radii.top_right, m.x1, m.y0),
                    bottom_right: outset_radius(radii.bottom_right, m.x1, m.y1),
                    bottom_left: outset_radius(radii.bottom_left, m.x0, m.y1),
                };
                (border_box + m, radii)
            }
        };
        rounded_rect(rect, radii)
    }

    /// Convert a CSS basic-shape in its reference box (CSS px) to a clip.
    fn basic_shape_clip(&self, shape: &BasicShape, reference: Rect) -> ElementClip {
        let w = reference.width();
        let h = reference.height();
        let ox = reference.x0;
        let oy = reference.y0;

        match shape {
            GenericBasicShape::Circle(circle) => {
                let center = resolve_position(&circle.position, reference);
                let r = circle_radius(&circle.radius, center, reference);
                ElementClip::shape(oval(center, r, r), Fill::NonZero)
            }
            GenericBasicShape::Ellipse(ellipse) => {
                let center = resolve_position(&ellipse.position, reference);
                let rx = ellipse_radius(&ellipse.semiaxis_x, center.x, reference.x0, reference.x1);
                let ry = ellipse_radius(&ellipse.semiaxis_y, center.y, reference.y0, reference.y1);
                ElementClip::shape(oval(center, rx, ry), Fill::NonZero)
            }
            GenericBasicShape::Polygon(polygon) => {
                let mut path = BezPath::new();
                for (i, coord) in polygon.coordinates.iter().enumerate() {
                    let point = Point::new(ox + resolve_lp(&coord.0, w), oy + resolve_lp(&coord.1, h));
                    if i == 0 {
                        path.move_to(point);
                    } else {
                        path.line_to(point);
                    }
                }
                path.close_path();
                ElementClip::shape(path, fill_rule(polygon.fill))
            }
            GenericBasicShape::Rect(inset_rect) => {
                // `inset()` (`rect()` and `xywh()` compute to it): insets that add up to more
                // than the box are reduced in proportion, to a rectangle of no area.
                let top = resolve_lp(&inset_rect.rect.0, h);
                let right = resolve_lp(&inset_rect.rect.1, w);
                let bottom = resolve_lp(&inset_rect.rect.2, h);
                let left = resolve_lp(&inset_rect.rect.3, w);
                let rect = Rect::new(ox + left, oy + top, ox + w - right, oy + h - bottom);
                if !(rect.x1 > rect.x0 && rect.y1 > rect.y0) {
                    return ElementClip::Nothing;
                }
                // `round`: corner radii as `border-radius` gives them, percentages of the
                // reference box, shrunk together when they overlap.
                let radii = resolve_radii(&inset_rect.round, w, h);
                ElementClip::shape(rounded_rect(rect, radii), Fill::NonZero)
            }
            GenericBasicShape::PathOrShape(path_or_shape) => {
                let (path, fill) = match path_or_shape {
                    GenericPathOrShapeFunction::Path(path) => {
                        // `path()` data is in CSS px, zoomed as lengths are.
                        let zoom = self.style.effective_zoom.value() as f64;
                        let path_data = svg_path_to_bezpath(
                            path.commands(),
                            |v| *v as f64 * zoom,
                            |v| *v as f64 * zoom,
                            |v| *v as f64 * zoom,
                            |v| *v as f64,
                        );
                        (path_data, path.fill)
                    }
                    GenericPathOrShapeFunction::Shape(shape) => {
                        // A single arc radius (and a percentage of it) is of the reference box's
                        // direction-agnostic size, as a circle's.
                        let agnostic = w.hypot(h) / std::f64::consts::SQRT_2;
                        let path_data = svg_path_to_bezpath(
                            &shape.commands,
                            move |v: &LengthPercentage| resolve_lp(v, w),
                            move |v: &LengthPercentage| resolve_lp(v, h),
                            move |v: &LengthPercentage| resolve_lp(v, agnostic),
                            move |v: &Angle| v.degrees() as f64,
                        );
                        (path_data, shape.fill)
                    }
                };
                match path {
                    Some(mut path) => {
                        path.apply_affine(Affine::translate((ox, oy)));
                        ElementClip::shape(path, fill_rule(fill))
                    }
                    None => ElementClip::Nothing,
                }
            }
        }
    }
}

fn fill_rule(rule: FillRule) -> Fill {
    match rule {
        FillRule::Nonzero => Fill::NonZero,
        FillRule::Evenodd => Fill::EvenOdd,
    }
}

fn scale_rect(rect: Rect, by: f64) -> Rect {
    Rect::new(rect.x0 * by, rect.y0 * by, rect.x1 * by, rect.y1 * by)
}

/// An ellipse as every clip draws one: as the box of `border-radius: 50%` is drawn, so that a
/// `circle()` clips what a round box covers (WPT's references are such boxes).
fn oval(center: Point, rx: f64, ry: f64) -> BezPath {
    let radius = Vec2::new(rx, ry);
    let radii = NonUniformRoundedRectRadii {
        top_left: radius,
        top_right: radius,
        bottom_right: radius,
        bottom_left: radius,
    };
    let rect = Rect::new(center.x - rx, center.y - ry, center.x + rx, center.y + ry);
    CssBox::new(rect, Insets::ZERO, Insets::ZERO, 0.0, radii).border_box_path()
}

fn scale_insets(insets: Insets, by: f64) -> Insets {
    Insets::new(insets.x0 * by, insets.y0 * by, insets.x1 * by, insets.y1 * by)
}

/// The radii of a box `inset` inside one with `radii` (CSS Backgrounds' inner radii).
fn inset_radii(radii: NonUniformRoundedRectRadii, inset: Insets) -> NonUniformRoundedRectRadii {
    let shrink = |r: Vec2, x: f64, y: f64| Vec2::new((r.x - x).max(0.0), (r.y - y).max(0.0));
    NonUniformRoundedRectRadii {
        top_left: shrink(radii.top_left, inset.x0, inset.y0),
        top_right: shrink(radii.top_right, inset.x1, inset.y0),
        bottom_right: shrink(radii.bottom_right, inset.x1, inset.y1),
        bottom_left: shrink(radii.bottom_left, inset.x0, inset.y1),
    }
}

/// A rectangle with elliptical corners, the radii shrunk together where they overlap (as
/// `border-radius`'s are). One whose corners make a whole ellipse is that ellipse, drawn as
/// `circle()` and `ellipse()` are (the same box clips alike whichever way it is given).
fn rounded_rect(rect: Rect, radii: NonUniformRoundedRectRadii) -> BezPath {
    let css_box = CssBox::new(rect, Insets::ZERO, Insets::ZERO, 0.0, radii);
    let (rx, ry) = (rect.width() / 2.0, rect.height() / 2.0);
    let r = css_box.border_radii;
    let half = |c: Vec2| (c.x - rx).abs() < 1e-9 && (c.y - ry).abs() < 1e-9;
    if rx > 0.0 && ry > 0.0 && half(r.top_left) && half(r.top_right) && half(r.bottom_right) && half(r.bottom_left) {
        return oval(rect.center(), rx, ry);
    }
    css_box.border_box_path()
}

/// The `<border-radius>` of an `inset()`, in CSS px: widths of `w`, heights of `h`.
fn resolve_radii(
    round: &GenericBorderRadius<style::values::computed::NonNegativeLengthPercentage>,
    w: f64,
    h: f64,
) -> NonUniformRoundedRectRadii {
    let resolve = |corner: &style::values::computed::BorderCornerRadius| Vec2 {
        x: resolve_lp(&corner.0.width.0, w),
        y: resolve_lp(&corner.0.height.0, h),
    };
    NonUniformRoundedRectRadii {
        top_left: resolve(&round.top_left),
        top_right: resolve(&round.top_right),
        bottom_right: resolve(&round.bottom_right),
        bottom_left: resolve(&round.bottom_left),
    }
}

/// A corner radius grown by the margins beside it (`dx` the horizontal one, `dy` the vertical):
/// the radius plus the margin, the margin first scaled by `1 + (ratio - 1)^3` where the radius is
/// smaller than it (CSS Backgrounds' spread rule); a sharp corner stays sharp.
fn outset_radius(radius: Vec2, dx: f64, dy: f64) -> Vec2 {
    fn grow(radius: f64, outset: f64) -> f64 {
        if radius <= 0.0 {
            return 0.0;
        }
        if outset <= 0.0 {
            return (radius + outset).max(0.0);
        }
        if radius >= outset {
            return radius + outset;
        }
        let ratio = radius / outset;
        radius + outset * (1.0 + (ratio - 1.0).powi(3))
    }
    if radius.x <= 0.0 || radius.y <= 0.0 {
        return Vec2::ZERO;
    }
    Vec2::new(grow(radius.x, dx), grow(radius.y, dy))
}

/// Resolve a LengthPercentage value against a basis length
fn resolve_lp(lp: &LengthPercentage, basis: f64) -> f64 {
    let basis_css = CSSPixelLength::new(basis as f32);
    lp.resolve(basis_css).px() as f64
}

/// Resolve a position (or auto => center) in the reference box
fn resolve_position(
    position: &GenericPositionOrAuto<style::values::computed::Position>,
    reference: Rect,
) -> Point {
    match position {
        GenericPositionOrAuto::Auto => reference.center(),
        GenericPositionOrAuto::Position(pos) => Point::new(
            reference.x0 + resolve_lp(&pos.horizontal, reference.width()),
            reference.y0 + resolve_lp(&pos.vertical, reference.height()),
        ),
    }
}

/// A `circle()`'s radius: a percentage is of the reference box's direction-agnostic size
/// (`sqrt(w² + h²) / sqrt(2)`); the side keywords measure to the box's sides in both axes, the
/// corner keywords to its corners.
fn circle_radius(
    radius: &GenericShapeRadius<LengthPercentage>,
    center: Point,
    reference: Rect,
) -> f64 {
    let dx = [(center.x - reference.x0).abs(), (reference.x1 - center.x).abs()];
    let dy = [(center.y - reference.y0).abs(), (reference.y1 - center.y).abs()];
    match radius {
        GenericShapeRadius::Length(lp) => {
            let agnostic = reference.width().hypot(reference.height()) / std::f64::consts::SQRT_2;
            resolve_lp(&lp.0, agnostic)
        }
        GenericShapeRadius::ClosestSide => dx[0].min(dx[1]).min(dy[0]).min(dy[1]),
        GenericShapeRadius::FarthestSide => dx[0].max(dx[1]).max(dy[0]).max(dy[1]),
        GenericShapeRadius::ClosestCorner => dx[0].min(dx[1]).hypot(dy[0].min(dy[1])),
        GenericShapeRadius::FarthestCorner => dx[0].max(dx[1]).hypot(dy[0].max(dy[1])),
    }
}

/// One of an `ellipse()`'s radii, along the axis whose reference box edges are `start` and `end`
/// and center coordinate `center`: the side keywords measure to the sides in that axis only.
fn ellipse_radius(
    radius: &GenericShapeRadius<LengthPercentage>,
    center: f64,
    start: f64,
    end: f64,
) -> f64 {
    let near = (center - start).abs().min((end - center).abs());
    let far = (center - start).abs().max((end - center).abs());
    match radius {
        GenericShapeRadius::Length(lp) => resolve_lp(&lp.0, end - start),
        GenericShapeRadius::ClosestSide => near,
        GenericShapeRadius::FarthestSide => far,
        // Behind a pref that is off (`layout.css.ellipse-corners.enabled`): the corner's
        // distance along this axis.
        GenericShapeRadius::ClosestCorner => near,
        GenericShapeRadius::FarthestCorner => far,
    }
}

type GenericPathCommand<Angle, N> = GenericShapeCommand<Angle, N, GenericPosition<N, N>, N>;

/// Convert an SVG path() or a shape() to a kurbo BezPath.
/// The returned path is in the path's own coordinate system (origin at 0,0).
/// The caller is responsible for translating it to the reference box origin.
/// `resolve_radius` resolves an arc radius given alone (both radii).
fn svg_path_to_bezpath<Angle: Copy, N>(
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
