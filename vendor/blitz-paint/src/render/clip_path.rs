use super::ElementCx;
use blitz_dom::CopyStyleGetters;
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
    /// pyro: an SVG `<clipPath>` of several shapes (or text, or with a clip path of its own),
    /// referenced with `url()`: a document that draws its clipping region opaque, in user units
    /// that `transform` takes to CSS px from the border box's origin. The element is painted in
    /// an isolated layer, and the region is drawn into it with `DestIn`.
    #[cfg(feature = "svg")]
    Svg {
        tree: std::sync::Arc<anyrender_svg::usvg::Tree>,
        transform: Affine,
    },
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
/// element's (its border box's origin), `scale` the device scale, `visible` the visible area in
/// the element's space (device px).
pub(super) fn with_element_clip<S: PaintScene>(
    scene: &mut S,
    clip: Option<&ElementClip>,
    transform: Affine,
    scale: f64,
    #[cfg_attr(not(feature = "svg"), allow(unused_variables))] visible: Rect,
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
        #[cfg(feature = "svg")]
        Some(ElementClip::Svg {
            tree,
            transform: user,
        }) => {
            // Everything the element paints may be in the region: the layers cover the whole
            // visible area (a destructive layer leaves alone what it does not cover).
            let area = visible;
            scene.push_layer(
                Fill::NonZero,
                peniko::Mix::Normal,
                super::ALMOST_OPAQUE,
                transform,
                &area,
                None,
                None,
            );
            paint(scene);
            scene.push_layer(
                Fill::NonZero,
                peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::DestIn),
                1.0,
                transform,
                &area,
                None,
                None,
            );
            anyrender_svg::render_svg_tree(scene, tree, transform * Affine::scale(scale) * *user);
            scene.pop_layer();
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
    /// supported (a `url()` reference to another document).
    pub(super) fn element_clip(&self) -> Option<ElementClip> {
        match self.style.get_clip_path() {
            ClipPath::None => None,
            #[cfg(feature = "svg")]
            ClipPath::Url(url) => self.svg_clip(url),
            #[cfg(not(feature = "svg"))]
            ClipPath::Url(_) => None,
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

    /// The margins (CSS px). An inline box (no box of layout's own) has its style's, in every
    /// side, as Chromium takes them for its margin box.
    fn margins(&self) -> Insets {
        if self.style.get_box().display.is_inline_flow() {
            let margin = self.style.get_margin();
            let resolve = |v: &style::values::computed::Margin| match v {
                style::values::generics::length::GenericMargin::LengthPercentage(v) => {
                    v.resolve(CSSPixelLength::new(0.0)).px() as f64
                }
                _ => 0.0,
            };
            return Insets::new(
                resolve(&margin.margin_left),
                resolve(&margin.margin_top),
                resolve(&margin.margin_right),
                resolve(&margin.margin_bottom),
            );
        }
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

/// pyro: `url()` references from a box's CSS to SVG elements of the page (`clip-path: url(#c)`
/// naming a `<clipPath>`, `mask-image: url(#m)` a `<mask>`).
#[cfg(feature = "svg")]
pub(super) mod svg_reference {
    use anyrender_svg::usvg;
    use blitz_dom::{BaseDocument, Node, NodeId, local_name};
    use kurbo::{Affine, BezPath};
    use peniko::Fill;
    use style::values::computed::url::ComputedUrl;

    /// The element a `url()` names when it is a reference into this document: `url(#id)`, or
    /// the document's own URL with a fragment (a fragment alone is resolved against the base
    /// URL, which a `<base>` may have moved). `None` for another document's.
    pub(in crate::render) fn local_reference(dom: &BaseDocument, url: &ComputedUrl) -> Option<NodeId> {
        let url = url.url()?;
        let fragment = url.fragment()?;
        let document = |url: &str| url.split_once('#').map_or(url, |(document, _)| document).to_owned();
        let target = document(url.as_str());
        if target != document(dom.url().as_str()) && target != document(dom.base_url().as_str()) {
            return None;
        }
        dom.get_element_by_id(&percent_decode(fragment))
    }

    /// pyro: the SVG document a `url()` names an element of, when it is another document that
    /// the element has loaded (`images`: its `reference_images`, or its `mask_images`), with the
    /// element's id: `clip-path: url(shapes.svg#clip)`, a `data:` URL too.
    pub(crate) fn external_reference(
        images: &[Option<blitz_dom::node::ImageResourceData>],
        url: &ComputedUrl,
    ) -> Option<(std::sync::Arc<[u8]>, String)> {
        use blitz_dom::node::{ImageData, Status};
        let url = url.url()?;
        let fragment = url.fragment()?;
        let loaded = images.iter().flatten().find(|image| *image.url == **url && image.status == Status::Ok)?;
        let ImageData::Svg(svg) = &loaded.image else {
            return None;
        };
        Some((svg.source.clone()?, percent_decode(fragment)))
    }

    fn percent_decode(text: &str) -> String {
        let bytes = text.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if bytes[i] == b'%'
                && i + 2 < bytes.len()
                && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
            {
                out.push((h * 16 + l) as u8);
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// Whether `node` is the SVG element `name`.
    pub(in crate::render) fn is_svg_element(node: &Node, name: &str) -> bool {
        node.element_data().is_some_and(|element| {
            element.name.ns == blitz_dom::ns!(svg) && &*element.name.local == name
        })
    }

    /// Whether the clip paths and masks `start` references, directly or through others (its own
    /// `clip-path` and `mask`, and its content's), lead back to one of them: a reference cycle,
    /// which makes them all invalid (usvg follows a long one without end).
    pub(in crate::render) fn reference_cycle(dom: &BaseDocument, start: NodeId) -> bool {
        fn references(dom: &BaseDocument, element: NodeId, out: &mut Vec<NodeId>) {
            let Some(node) = dom.get_node(element) else {
                return;
            };
            for name in [local_name!("clip-path"), local_name!("mask"), local_name!("style")] {
                if let Some(value) = node.attr(name) {
                    let mut rest = value;
                    while let Some(at) = rest.find("url(") {
                        let inner = &rest[at + 4..];
                        let end = inner.find(')').unwrap_or(inner.len());
                        let reference = inner[..end].trim().trim_matches(|c| c == '"' || c == '\'');
                        if let Some(id) = reference.rsplit_once('#').map(|(_, id)| id)
                            && let Some(target) = dom.get_element_by_id(id)
                        {
                            out.push(target);
                        }
                        rest = &inner[end..];
                    }
                }
            }
            if let Some(style) = node.primary_styles() {
                if let style::values::computed::basic_shape::ClipPath::Url(url) = style.get_clip_path()
                    && let Some(target) = local_reference(dom, url)
                {
                    out.push(target);
                }
                for image in style.get_svg().mask_image.0.iter() {
                    if let style::values::generics::image::GenericImage::Url(url) = image
                        && let Some(target) = local_reference(dom, url)
                    {
                        out.push(target);
                    }
                }
            }
            for &child in &node.children {
                references(dom, child, out);
            }
        }
        // A depth-first search over the referenced elements: a reference to one on the path
        // is a cycle.
        fn visit(dom: &BaseDocument, element: NodeId, path: &mut Vec<NodeId>, done: &mut Vec<NodeId>) -> bool {
            if path.contains(&element) {
                return true;
            }
            if done.contains(&element) || path.len() > 64 {
                return path.len() > 64;
            }
            path.push(element);
            let mut targets = Vec::new();
            references(dom, element, &mut targets);
            for target in targets {
                if visit(dom, target, path, done) {
                    return true;
                }
            }
            path.pop();
            done.push(element);
            false
        }
        visit(dom, start, &mut Vec::new(), &mut Vec::new())
    }

    /// An attribute value, escaped for a double-quoted attribute.
    pub(in crate::render) fn escape_attribute(value: &str) -> String {
        value.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;")
    }

    /// A rectangle drawn opaque, whatever the referenced document's style sheets say about
    /// `rect`s (an inline style wins over them).
    pub(in crate::render) fn opaque_rect(rect: kurbo::Rect, attributes: &str) -> String {
        format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" style=\"fill:#000;fill-opacity:1;opacity:1;\
             stroke:none;visibility:visible;display:inline\" {attributes}/>",
            rect.x0,
            rect.y0,
            rect.width(),
            rect.height()
        )
    }

    /// The first group of `group` (in tree order) with a clip path: the shape that references it.
    pub(in crate::render) fn clipped_group(group: &usvg::Group) -> Option<&usvg::Group> {
        for node in group.children() {
            if let usvg::Node::Group(child) = node {
                if child.clip_path().is_some() {
                    return Some(child);
                }
                if let Some(found) = clipped_group(child) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// A clip path that is one shape (with no clip path of its own, through groups that only
    /// transform): that shape in the user space of the element that references it, with its
    /// clip rule. An empty path when it has no visible shape. `None` for any other clip path.
    pub(in crate::render) fn single_shape(clip: &usvg::ClipPath) -> Option<(BezPath, Fill)> {
        fn walk<'a>(group: &'a usvg::Group, found: &mut Option<&'a usvg::Path>) -> bool {
            for node in group.children() {
                match node {
                    usvg::Node::Path(path) => {
                        if !path.is_visible() {
                            continue;
                        }
                        if found.is_some() {
                            return false;
                        }
                        *found = Some(path);
                    }
                    usvg::Node::Group(child) => {
                        if child.clip_path().is_some()
                            || child.mask().is_some()
                            || !child.filters().is_empty()
                            || !walk(child, found)
                        {
                            return false;
                        }
                    }
                    _ => return false,
                }
            }
            true
        }
        if clip.clip_path().is_some() {
            return None;
        }
        let mut found = None;
        if !walk(clip.root(), &mut found) {
            return None;
        }
        let Some(path) = found else {
            return Some((BezPath::new(), Fill::NonZero));
        };
        let mut shape = to_bez_path(path.data());
        shape.apply_affine(to_affine(clip.transform()) * to_affine(path.abs_transform()));
        let fill = match path.fill().map(|fill| fill.rule()) {
            Some(usvg::FillRule::EvenOdd) => Fill::EvenOdd,
            _ => Fill::NonZero,
        };
        Some((shape, fill))
    }

    pub(in crate::render) fn to_affine(ts: usvg::Transform) -> Affine {
        Affine::new([ts.sx, ts.ky, ts.kx, ts.sy, ts.tx, ts.ty].map(f64::from))
    }

    fn to_bez_path(path: &usvg::tiny_skia_path::Path) -> BezPath {
        use usvg::tiny_skia_path::PathSegment;
        let point = |p: usvg::tiny_skia_path::Point| kurbo::Point::new(p.x as f64, p.y as f64);
        let mut out = BezPath::new();
        for segment in path.segments() {
            match segment {
                PathSegment::MoveTo(p) => out.move_to(point(p)),
                PathSegment::LineTo(p) => out.line_to(point(p)),
                PathSegment::QuadTo(p1, p2) => out.quad_to(point(p1), point(p2)),
                PathSegment::CubicTo(p1, p2, p3) => out.curve_to(point(p1), point(p2), point(p3)),
                PathSegment::Close => out.close_path(),
            }
        }
        out
    }
}

#[cfg(feature = "svg")]
impl ElementCx<'_, '_> {
    /// pyro: `clip-path: url(#id)` naming a `<clipPath>` of this document, as Chromium applies one
    /// to a box: the reference box is the border box (`clipPathUnits="objectBoundingBox"`
    /// fractions of it), user space has its origin at the border box's and the element's zoom.
    /// A clip path of one shape is a clip layer of it; any other is drawn (see
    /// [`ElementClip::Svg`]). A reference to anything but a `<clipPath>` clips nothing; one in a
    /// reference cycle clips everything.
    fn svg_clip(&self, url: &style::values::computed::url::ComputedUrl) -> Option<ElementClip> {
        use svg_reference::*;
        let dom = self.context.dom;
        // pyro: a `<clipPath>` of this document, or of another one the element has loaded
        // (`url(shapes.svg#clip)`).
        let mut external: Option<std::sync::Arc<[u8]>> = None;
        let (target, id, object_bounding_box) = match local_reference(dom, url) {
            Some(target) => {
                let node = dom.get_node(target)?;
                if !is_svg_element(node, "clipPath") {
                    return None;
                }
                if reference_cycle(dom, target) {
                    return Some(ElementClip::Nothing);
                }
                let units = node.attr(blitz_dom::local_name!("clipPathUnits")) == Some("objectBoundingBox");
                (Some(target), node.attr(blitz_dom::local_name!("id"))?.to_string(), units)
            }
            None => {
                let (source, id) = external_reference(&self.element.reference_images, url)?;
                let (name, units) = blitz_dom::node::svg_source_element(&source, &id, "clipPathUnits")?;
                if name != "clipPath" {
                    return None;
                }
                external = Some(source);
                (None, id, units.as_deref() == Some("objectBoundingBox"))
            }
        };
        let id = id.as_str();
        // User units are CSS px unzoomed.
        let zoom = self.style.effective_zoom.value() as f64;
        let to_user = 1.0 / (self.scale * zoom);
        // The shape the clip path is taken from: the border box, whose box `objectBoundingBox`
        // units are fractions of; with user units, anything visible may be in the region.
        let shape = scale_rect(
            if object_bounding_box {
                self.frame.border_box
            } else {
                self.visible_area()
            },
            to_user,
        );
        let painter = self.context;
        let content = opaque_rect(shape, &format!("clip-path=\"url(#{})\"", escape_attribute(id)));
        let (width, height) = ((painter.width as f64 * to_user) as f32, (painter.height as f64 * to_user) as f32);
        let svg = match target {
            Some(target) => dom.svg_reference_document(target, &content, width, height)?,
            None => blitz_dom::node::SvgImageData::reference_document_from_source(external.as_deref().unwrap_or_default(), &content, width, height)?,
        };
        let user = Affine::scale(zoom);
        // usvg leaves out a shape whose clip path is empty or invalid: it clips everything.
        let Some(clip) = clipped_group(svg.tree.root()).and_then(|group| group.clip_path()) else {
            return Some(ElementClip::Nothing);
        };
        match single_shape(clip) {
            Some((mut shape, fill)) => {
                shape.apply_affine(user);
                Some(ElementClip::shape(shape, fill))
            }
            None => Some(ElementClip::Svg {
                tree: svg.tree.clone(),
                transform: user,
            }),
        }
    }
}

thread_local! {
    /// pyro: set while an inline layout is painted for its inline elements' clip paths and masks
    /// ([`ElementCx::draw_inline_effects`]), whose passes paint it plainly.
    static INLINE_EFFECTS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// pyro: an inline element's piece on one line, in an inline layout's space (device px, with the
/// text's placement shifts).
#[derive(Clone, Copy)]
struct InlineFragment {
    /// Its border box.
    border_box: Rect,
    /// What it may paint into: the border box, as tall as the line box at least (glyphs overflow
    /// their content area).
    area: Rect,
}

impl ElementCx<'_, '_> {
    /// pyro: paints this inline root's layout when inline elements in it (not atomic) have a
    /// `clip-path` or a mask, and returns `true`; `false` (nothing painted) when none has. As in
    /// Chromium each such element is a stacking context painted after the rest of the layout: the
    /// layout is painted without the elements' areas, then into each element's area under its clip
    /// path (whose reference box is the element's first fragment, as Blink's and Gecko's are) and
    /// mask (each fragment masked as a piece of one box of all fragments side by side, as
    /// `box-decoration-break: slice` lays them out).
    pub(super) fn draw_inline_effects(&self, scene: &mut impl PaintScene, pos: Point) -> bool {
        use parley::{InlineBoxKind, PositionedLayoutItem};
        use style::values::computed::basic_shape::ClipPath as StyloClipPath;
        use style::values::generics::image::GenericImage;

        if INLINE_EFFECTS.get() || self.node.vertical_block_axis().is_some() {
            return false;
        }
        let Some(text_layout) = self.element.inline_layout_data.as_ref() else {
            return false;
        };
        let layout = &text_layout.layout;
        let styles = layout.styles();
        if styles.len() < 2 {
            return false;
        }
        let dom = self.context.dom;
        let root = self.node.id;
        // pyro: a recording paint (the PDF's text) leaves out inline opacity and backdrop
        // filters: each pass would draw (and record) the layout's glyph runs again, and the PDF
        // writes every glyph run drawn, so the text would be written once per pass (and the
        // element's text after the rest, out of reading order). Their text is written unfaded,
        // once, in order.
        let recording = self.context.draw_text_context.borrow().runs.is_some();
        let has_effect = |id: blitz_dom::NodeId| {
            dom.get_node(id)
                .filter(|node| node.element_data().is_some())
                .and_then(|node| node.primary_styles())
                .is_some_and(|style| {
                    !matches!(style.get_clip_path(), StyloClipPath::None)
                        || style
                            .get_svg()
                            .mask_image
                            .0
                            .iter()
                            .any(|image| !matches!(image, GenericImage::None))
                        || (!recording
                            && (style.get_effects().opacity < 1.0 || !style.get_effects().backdrop_filter.0.is_empty())
                            && paints_alone(dom, root, id))
                })
        };
        let mut effects: Vec<blitz_dom::NodeId> = Vec::new();
        for style in styles {
            let id = style.brush.id;
            if id != root && !effects.contains(&id) && has_effect(id) {
                effects.push(id);
            }
        }
        if effects.is_empty() {
            return false;
        }
        // The outermost ones: an inner one is painted with its ancestor.
        let all = effects.clone();
        effects.retain(|&id| {
            let mut parent = dom.get_node(id).and_then(|node| node.parent);
            while let Some(ancestor) = parent.filter(|&ancestor| ancestor != root) {
                if all.contains(&ancestor) {
                    return false;
                }
                parent = dom.get_node(ancestor).and_then(|node| node.parent);
            }
            true
        });

        // The layout's space, as `draw_inline_layout` paints it.
        let subpixel = text_layout.content_box_subpixel_offset;
        let origin = Vec2::new(
            (pos.x + subpixel.x as f64) * self.scale,
            (pos.y + subpixel.y as f64 + text_layout.block_offset as f64) * self.scale,
        );
        let transform = self.transform * Affine::translate(origin);
        let (text_shift, tops) = self.text_placement(text_layout.block_offset as f64);
        let visible = self.visible_area() - origin;

        // Each element's fragments, line by line.
        let scale = self.scale as f32;
        let length = |v: &LengthPercentage| v.resolve(CSSPixelLength::new(0.0)).px();
        let mut fragments: Vec<Vec<InlineFragment>> = vec![Vec::new(); effects.len()];
        let mut chain: Vec<(blitz_dom::NodeId, u16)> = Vec::new();
        for line in layout.lines() {
            let metrics = line.metrics();
            // pyro: from the line's start, past the floats before it (`inline_box_fragments`).
            let content_end =
                metrics.inline_min_coord + metrics.offset + metrics.advance - metrics.hanging_advance;
            let shift = tops.map_or(0.0, |tops| tops.shift(metrics));
            // Per element: its x range on the line and its own style.
            let mut spans: Vec<Option<(f32, f32, u16)>> = vec![None; effects.len()];
            for item in line.items() {
                let (style_index, x0, x1, edge) = match &item {
                    PositionedLayoutItem::GlyphRun(run) => {
                        let x0 = run.offset();
                        let mut x1 = x0 + run.advance();
                        if !layout.is_rtl() {
                            x1 = x1.min(content_end).max(x0);
                        }
                        (run.style_index(), x0, x1, None)
                    }
                    PositionedLayoutItem::InlineBox(ibox) => match ibox.kind {
                        InlineBoxKind::InFlow => (ibox.style_index, ibox.x, ibox.x + ibox.width, None),
                        InlineBoxKind::SpanStart | InlineBoxKind::SpanEnd => (
                            ibox.style_index,
                            ibox.x,
                            ibox.x + ibox.width,
                            Some((blitz_dom::NodeId::from_u64(ibox.id), ibox.kind)),
                        ),
                        _ => continue,
                    },
                };
                // The inline elements the item is in, innermost first, with their styles.
                chain.clear();
                let mut index = style_index;
                loop {
                    let Some(style) = styles.get(usize::from(index)) else {
                        break;
                    };
                    let id = style.brush.id;
                    if id == root {
                        break;
                    }
                    match chain.last_mut() {
                        Some(last) if last.0 == id => last.1 = index,
                        _ => chain.push((id, index)),
                    }
                    if index == 0 {
                        break;
                    }
                    index = layout.style_parent(index);
                }
                for (slot, &effect) in effects.iter().enumerate() {
                    let Some(&(_, own_style)) = chain.iter().find(|(id, _)| *id == effect) else {
                        continue;
                    };
                    // The element's own edge is in its box without its margin.
                    let (mut x0, mut x1) = (x0, x1);
                    if let Some((_, kind)) = edge.filter(|&(id, _)| id == effect)
                        && let Some(style) = dom.get_node(effect).and_then(|node| node.primary_styles())
                    {
                        let margin = |v: &style::values::computed::Margin| match v {
                            style::values::generics::length::GenericMargin::LengthPercentage(v) => {
                                length(v) * scale
                            }
                            _ => 0.0,
                        };
                        let rtl = style.clone_direction() == style::computed_values::direction::T::Rtl;
                        if (kind == InlineBoxKind::SpanStart) != rtl {
                            x0 += margin(&style.get_margin().margin_left);
                        } else {
                            x1 -= margin(&style.get_margin().margin_right);
                        }
                    }
                    spans[slot] = Some(match spans[slot] {
                        Some((a, b, s)) => (a.min(x0), b.max(x1), s),
                        None => (x0, x1, own_style),
                    });
                }
            }
            for (slot, span) in spans.into_iter().enumerate() {
                let Some((x0, x1, own_style)) = span else {
                    continue;
                };
                let Some(style) = dom.get_node(effects[slot]).and_then(|node| node.primary_styles()) else {
                    continue;
                };
                let (border, padding) = (style.get_border(), style.get_padding());
                let above = border.border_top_width.0.to_f32_px() * f32::from(!border.border_top_style.none_or_hidden())
                    + length(&padding.padding_top.0);
                let below = border.border_bottom_width.0.to_f32_px()
                    * f32::from(!border.border_bottom_style.none_or_hidden())
                    + length(&padding.padding_bottom.0);
                let (top, bottom) = line.span_content_area(own_style);
                let border_box = Rect::new(
                    (x0 as f64) + text_shift,
                    ((top - above * scale) as f64) + shift,
                    (x1 as f64) + text_shift,
                    ((bottom + below * scale) as f64) + shift,
                );
                // Its outline and shadows reach past its border box.
                let outline = style.get_outline();
                let mut reach = match outline.outline_style {
                    style::values::computed::OutlineStyle::BorderStyle(line) if line.none_or_hidden() => 0.0,
                    _ => outline.outline_width.0.to_f64_px() + outline.outline_offset.to_f64_px().max(0.0),
                };
                for shadow in style.get_effects().box_shadow.0.iter().filter(|shadow| !shadow.inset) {
                    let offset = (shadow.base.horizontal.px() as f64).abs().max((shadow.base.vertical.px() as f64).abs());
                    reach = reach.max(offset + shadow.base.blur.px() as f64 + (shadow.spread.px() as f64).max(0.0));
                }
                let reach = reach * self.scale;
                let area = border_box
                    .union(Rect::new(
                        border_box.x0,
                        metrics.block_min_coord as f64 + shift,
                        border_box.x1,
                        metrics.block_max_coord as f64 + shift,
                    ))
                    .inflate(reach, reach);
                // pyro: under a translation, out to whole device pixels around the box as
                // `draw_inline_boxes` snaps it (to whole CSS px): the passes inside and outside
                // the areas then meet on pixel edges, where an edge through a pixel cut the box's
                // border and let a sliver of what lies under it through (WPT
                // `backdrop-filter-inline-positioning`).
                let [a, b, c, d, tx, ty] = transform.as_coeffs();
                let area = if a == 1.0 && b == 0.0 && c == 0.0 && d == 1.0 {
                    let snap = |v: f64, t: f64| ((t + v) / self.scale).round() * self.scale - t;
                    let snapped = Rect::new(
                        snap(border_box.x0, tx),
                        snap(border_box.y0, ty),
                        snap(border_box.x1, tx),
                        snap(border_box.y1, ty),
                    );
                    let area = area.union(snapped);
                    Rect::new(
                        (tx + area.x0).floor() - tx,
                        (ty + area.y0).floor() - ty,
                        (tx + area.x1).ceil() - tx,
                        (ty + area.y1).ceil() - ty,
                    )
                } else {
                    area
                };
                fragments[slot].push(InlineFragment { border_box, area });
            }
        }

        // pyro: each element's areas without those of the elements before it: where neighbours'
        // areas met in a pixel column (each snapped out to whole device pixels), both elements'
        // passes painted it.
        let mut taken: Vec<Rect> = Vec::new();
        let pieces: Vec<Vec<Vec<Rect>>> = fragments
            .iter()
            .map(|fragments| {
                let own = fragments.iter().map(|fragment| subtract(fragment.area, &taken)).collect();
                taken.extend(fragments.iter().map(|fragment| fragment.area));
                own
            })
            .collect();
        let path_of = |rects: &mut dyn Iterator<Item = &Rect>| {
            let mut path = BezPath::new();
            for rect in rects {
                path.extend(rect.to_path(0.1));
            }
            path
        };

        // pyro: each element's pass draws the glyph runs that can reach its areas only
        // (`RunFilter`): drawing every run of the layout clipped to the areas made a paragraph's
        // cost grow with its faded spans times its length.
        let text = &self.context.draw_text_context;
        let set_filter = |areas: Option<Vec<Rect>>| {
            text.borrow_mut().run_filter = areas.map(|areas| crate::text::RunFilter { root, areas });
        };

        INLINE_EFFECTS.set(true);
        // The layout without the elements' areas.
        // pyro: cut into pieces that do not overlap, inside the visible area: where two areas
        // overlapped (lines closer than their content is tall) the even-odd rule counted them
        // twice and painted the layout there at full strength under the faded or clipped copy.
        // pyro: the clip is cut to what the layout paints (its ink, recorded first), not the whole
        // visible area: a clip layer the size of the frame cost a 4K frame of a paragraph with a
        // faded span 8% more instructions.
        let mut layout = anyrender::Scene::new();
        let recorded = text.borrow().runs.as_ref().map(Vec::len);
        self.draw_inline_layout(&mut layout, pos);
        let mut appended = false;
        if let Some(ink) = crate::bounds::scene_bounds(&layout.commands)
            && transform.determinant().abs() > 1e-12
        {
            let outer = transform.inverse().transform_rect_bbox(ink).intersect(visible);
            if outer.width() > 0.0 && outer.height() > 0.0 {
                let mut outside = outer.to_path(0.1);
                for piece in disjoint_rects(fragments.iter().flatten().map(|fragment| fragment.area.intersect(outer))) {
                    outside.extend(piece.to_path(0.1));
                }
                scene.push_clip_layer(Fill::EvenOdd, transform, &outside);
                scene.append_scene(layout, Affine::IDENTITY);
                scene.pop_layer();
                appended = true;
            }
        }
        // pyro: a pass left out records no runs: the PDF matches each glyph run drawn to the next
        // run recorded.
        if !appended && let (Some(len), Some(runs)) = (recorded, text.borrow_mut().runs.as_mut()) {
            runs.truncate(len);
        }

        // Each element's area, under its clip path and mask.
        for ((&effect, fragments), pieces) in effects.iter().zip(&fragments).zip(&pieces) {
            let (Some(node), Some(first)) = (dom.get_node(effect), fragments.first()) else {
                continue;
            };
            let Some(style) = node.primary_styles() else {
                continue;
            };
            let (border, padding) = (style.get_border(), style.get_padding());
            let side = |width: &style::values::computed::BorderSideWidth,
                        line: style::values::computed::BorderStyle| {
                if line.none_or_hidden() { 0.0 } else { width.0.to_f32_px() }
            };
            let rtl = style.clone_direction() == style::computed_values::direction::T::Rtl;
            let single = fragments.len() == 1;
            // The first fragment has the start side, the last the end side (`slice`).
            let (has_left, has_right) = if rtl { (single, true) } else { (true, single) };
            let edges = |left: bool, right: bool| {
                (
                    taffy::Rect {
                        left: if left { side(&border.border_left_width, border.border_left_style) } else { 0.0 },
                        right: if right { side(&border.border_right_width, border.border_right_style) } else { 0.0 },
                        top: side(&border.border_top_width, border.border_top_style),
                        bottom: side(&border.border_bottom_width, border.border_bottom_style),
                    },
                    taffy::Rect {
                        left: if left { length(&padding.padding_left.0) } else { 0.0 },
                        right: if right { length(&padding.padding_right.0) } else { 0.0 },
                        top: length(&padding.padding_top.0),
                        bottom: length(&padding.padding_bottom.0),
                    },
                )
            };
            let has_mask = style
                .get_svg()
                .mask_image
                .0
                .iter()
                .any(|image| !matches!(image, GenericImage::None));
            let first_edges = edges(has_left, has_right);
            let strip_edges = edges(true, true);
            let opacity = style.get_effects().opacity;
            let alone = paints_alone(dom, root, effect);
            // pyro: its backdrop filter, over its fragments' border boxes (WPT
            // `backdrop-filter-inline-positioning`): the backdrop is captured before its layers and
            // drawn first inside them, as `render_element` does for a box.
            let backdrop = alone
                .then(|| {
                    crate::filters::convert_filters(
                        &style.get_effects().backdrop_filter.0,
                        self.scale,
                        &style.clone_color(),
                        dom,
                    )
                })
                .flatten()
                .map(std::sync::Arc::new);
            drop(style);
            let mut boxes = BezPath::new();
            if let Some(backdrop) = &backdrop {
                // At whole CSS pixels under a translation, as `draw_inline_boxes` puts the boxes.
                let [a, b, c, d, tx, ty] = transform.as_coeffs();
                let snap = |v: f64, t: f64| {
                    if a == 1.0 && b == 0.0 && c == 0.0 && d == 1.0 {
                        ((t + v) / self.scale).round() * self.scale - t
                    } else {
                        v
                    }
                };
                for fragment in fragments {
                    let r = fragment.border_box;
                    boxes.extend(Rect::new(snap(r.x0, tx), snap(r.y0, ty), snap(r.x1, tx), snap(r.y1, ty)).to_path(0.1));
                }
                scene.push_layer(Fill::NonZero, peniko::BlendMode::default(), 0.0, transform, &boxes, None, Some(backdrop.clone()));
                scene.pop_layer();
            }
            // pyro: its opacity, over all of its fragments as one group (CSS Color 4 §3.2, WPT
            // `t32-opacity-offscreen-multiple-boxes-*`), when nothing else paints in its areas.
            let faded = opacity < 1.0 && alone;
            let areas = path_of(&mut pieces.iter().flatten());
            if faded {
                scene.push_layer(Fill::NonZero, peniko::BlendMode::default(), opacity, transform, &areas, None, None);
            }
            let box_layout = |rect: Rect, (border, padding): (taffy::Rect<f32>, taffy::Rect<f32>)| taffy::Layout {
                size: taffy::Size {
                    width: (rect.width() / self.scale) as f32,
                    height: (rect.height() / self.scale) as f32,
                },
                border,
                padding,
                ..taffy::Layout::new()
            };

            // The clip path, against the first fragment's border box.
            let first_transform = transform * Affine::translate(first.border_box.origin().to_vec2());
            let first_cx = self.context.element_cx(
                node,
                box_layout(first.border_box, first_edges),
                first_transform,
                None,
            );
            let clip = first_cx.element_clip();
            let first_visible = visible - first.border_box.origin().to_vec2();
            with_element_clip(scene, clip.as_ref(), first_transform, self.scale, first_visible, |scene| {
                if let Some(backdrop) = &backdrop {
                    scene.push_layer(Fill::NonZero, peniko::BlendMode::default(), 1.0, transform, &boxes, None, Some(backdrop.clone()));
                    scene.pop_layer();
                }
                if !has_mask {
                    if pieces.iter().all(Vec::is_empty) {
                        return;
                    }
                    set_filter(Some(pieces.iter().flatten().copied().collect()));
                    scene.push_clip_layer(Fill::NonZero, transform, &areas);
                    self.draw_inline_layout(scene, pos);
                    scene.pop_layer();
                    set_filter(None);
                    return;
                }
                // The mask, fragment by fragment, over all fragments side by side.
                let width: f64 = fragments.iter().map(|fragment| fragment.border_box.width()).sum();
                let height = fragments
                    .iter()
                    .map(|fragment| fragment.border_box.height())
                    .fold(0.0, f64::max);
                let mut offset = 0.0;
                for (fragment, pieces) in fragments.iter().zip(pieces) {
                    let strip = Rect::new(0.0, 0.0, width, height);
                    let strip_transform =
                        transform * Affine::translate((fragment.border_box.x0 - offset, fragment.border_box.y0));
                    offset += fragment.border_box.width();
                    if pieces.is_empty() {
                        continue;
                    }
                    let mask_cx = self.context.element_cx(node, box_layout(strip, strip_edges), strip_transform, None);
                    set_filter(Some(pieces.clone()));
                    scene.push_clip_layer(Fill::NonZero, transform, &path_of(&mut pieces.iter()));
                    let pushed = mask_cx.maybe_push_css_mask_layer(scene);
                    self.draw_inline_layout(scene, pos);
                    mask_cx.maybe_pop_css_mask_layer(scene, pushed);
                    scene.pop_layer();
                    set_filter(None);
                }
            });
            if faded {
                scene.pop_layer();
            }
        }
        INLINE_EFFECTS.set(false);
        true
    }
}

/// pyro: rectangles covering what `rects` cover, no two of which overlap (empty ones left out).
fn disjoint_rects(rects: impl Iterator<Item = Rect>) -> Vec<Rect> {
    let mut out: Vec<Rect> = Vec::new();
    for rect in rects {
        let pieces = subtract(rect, &out);
        out.extend(pieces);
    }
    out
}

/// pyro: rectangles covering what `rect` covers outside `taken`, no two of which overlap (empty
/// ones left out).
fn subtract(rect: Rect, taken: &[Rect]) -> Vec<Rect> {
    let mut pieces: Vec<Rect> = if rect.width() > 0.0 && rect.height() > 0.0 { vec![rect] } else { Vec::new() };
    for &taken in taken {
        pieces = pieces
            .into_iter()
            .flat_map(|piece| {
                let cut = piece.intersect(taken);
                if !(cut.width() > 0.0 && cut.height() > 0.0) {
                    return vec![piece];
                }
                [
                    Rect::new(piece.x0, piece.y0, piece.x1, cut.y0),
                    Rect::new(piece.x0, cut.y1, piece.x1, piece.y1),
                    Rect::new(piece.x0, cut.y0, cut.x0, cut.y1),
                    Rect::new(cut.x1, cut.y0, piece.x1, cut.y1),
                ]
                .into_iter()
                .filter(|r| r.width() > 0.0 && r.height() > 0.0)
                .collect()
            })
            .collect();
    }
    pieces
}

/// pyro: whether the inline element `id` of the inline root `root` can be faded (or painted over
/// its filtered backdrop) by painting the layout in its areas in its group: no inline element
/// between them paints a box (a background, a border, a shadow) that would show there, which the
/// element paints over, not with.
fn paints_alone(dom: &blitz_dom::BaseDocument, root: blitz_dom::NodeId, id: blitz_dom::NodeId) -> bool {
    // An anonymous block's inline elements are its parent's children: the walk stops there too.
    let container = dom.get_node(root).filter(|node| node.is_anonymous()).and_then(|node| node.parent);
    let mut ancestor = dom.get_node(id).and_then(|node| node.parent);
    while let Some(node) = ancestor.filter(|&a| a != root && Some(a) != container).and_then(|a| dom.get_node(a)) {
        if let Some(style) = node.primary_styles() {
            let background = style.get_background();
            let border = style.get_border();
            let paints = background.background_color.resolve_to_absolute(&style.clone_color()).alpha > 0.0
                || background
                    .background_image
                    .0
                    .iter()
                    .any(|image| !matches!(image, style::values::generics::image::GenericImage::None))
                || [
                    (border.border_top_style, &border.border_top_width),
                    (border.border_right_style, &border.border_right_width),
                    (border.border_bottom_style, &border.border_bottom_width),
                    (border.border_left_style, &border.border_left_width),
                ]
                .iter()
                .any(|(line, width)| !line.none_or_hidden() && width.0.to_f32_px() > 0.0)
                || !style.get_effects().box_shadow.0.is_empty();
            if paints {
                return false;
            }
        }
        ancestor = node.parent;
    }
    true
}
