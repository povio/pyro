use super::ElementCx;
use kurbo::{BezPath, Circle, Ellipse, Point, Rect, Shape, SvgArc, Vec2};
use peniko::Fill;
use style::values::computed::basic_shape::{BasicShape, ClipPath};
use style::values::computed::{Angle, CSSPixelLength, LengthPercentage};
use style::values::generics::basic_shape::{
    ArcSize, ArcSweep, AxisEndPoint, CommandEndPoint, ControlPoint, ControlReference, FillRule,
    GenericBasicShape, GenericPathOrShapeFunction, GenericShapeCommand, GenericShapeRadius,
    ShapeBox, ShapeGeometryBox,
};
use style::values::generics::position::{GenericPosition, GenericPositionOrAuto};

impl ElementCx<'_, '_> {
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

    /// Compute the clip-path BezPath (if any) for this element, with its fill rule.
    /// Returns `None` if clip-path is `none` or unsupported.
    pub(super) fn clip_path_shape(&self) -> Option<(BezPath, Fill)> {
        match self.style.get_clip_path() {
            ClipPath::None => None,
            // pyro: a `<clipPath>` of the document (`url_clip_path`).
            ClipPath::Url(url) => self.url_clip_path(url),
            ClipPath::Shape(basic_shape, geometry_box) => {
                let reference_box = self.resolve_geometry_box(geometry_box);
                let fill = match &**basic_shape {
                    GenericBasicShape::Polygon(polygon) => polygon.fill,
                    GenericBasicShape::PathOrShape(GenericPathOrShapeFunction::Path(path)) => path.fill,
                    GenericBasicShape::PathOrShape(GenericPathOrShapeFunction::Shape(shape)) => shape.fill,
                    _ => FillRule::Nonzero,
                };
                self.basic_shape_to_path(basic_shape, reference_box).map(|p| (p, fill_of(fill)))
            }
            ClipPath::Box(geometry_box) => {
                let reference_box = self.resolve_geometry_box(geometry_box);
                Some((Rect::from(reference_box).into_path(0.1), Fill::NonZero))
            }
        }
    }

    /// pyro: `clip-path: url(#id)` naming an SVG `<clipPath>` of the document: the union of its
    /// shape children (`path`, `rect`, `circle`, `ellipse`, `polygon`, `polyline`, with their and
    /// its `transform`), in user space units from the border box's origin (`clipPathUnits`
    /// `objectBoundingBox`: fractions of the border box), filled by the first shape's
    /// `clip-rule`. A `<clipPath>` without shapes clips everything away; a reference to anything
    /// else clips nothing (the property is as if not given).
    fn url_clip_path(&self, url: &style::values::computed::url::ComputedUrl) -> Option<(BezPath, Fill)> {
        use blitz_dom::local_name;
        const SVG: &str = "http://www.w3.org/2000/svg";
        let url = url.url()?;
        let dom = self.context.dom;
        // Only a reference into this document (`url(#id)`): another document's is not loaded.
        let without_fragment = |u: &str| u.split('#').next().unwrap_or(u).to_owned();
        if without_fragment(url.as_str()) != without_fragment(dom.url().as_str()) {
            return None;
        }
        let id = url.fragment()?;
        let node = dom.get_node(dom.get_element_by_id(id)?)?;
        let clip = node.element_data()?;
        if &*clip.name.ns != SVG || &*clip.name.local != "clipPath" {
            return None;
        }
        let mut path = BezPath::new();
        let mut fill = None;
        // An element's transform: its CSS `transform` (set by script or a sheet), else its
        // attribute.
        let transform_of = |n: &blitz_dom::Node, el: &blitz_dom::ElementData| {
            let css = n.primary_styles().and_then(|s| blitz_dom::resolve_2d_transform_without_box(s.get_box()));
            css.or_else(|| el.attr(local_name!("transform")).and_then(svg_transform))
        };
        for &child in &node.children {
            let Some((n, el)) = dom.get_node(child).and_then(|c| Some((c, c.element_data()?))) else { continue };
            if &*el.name.ns != SVG {
                continue;
            }
            let Some(mut shape) = svg_shape(el) else { continue };
            if let Some(t) = transform_of(n, el) {
                shape.apply_affine(t);
            }
            let rule = el.attr(local_name!("clip-rule")).map(str::trim);
            fill.get_or_insert(if rule == Some("evenodd") { Fill::EvenOdd } else { Fill::NonZero });
            path.extend(shape);
        }
        if let Some(t) = transform_of(node, clip) {
            path.apply_affine(t);
        }
        let border = self.resolve_geometry_box(&ShapeGeometryBox::ShapeBox(ShapeBox::BorderBox));
        let units = clip.attrs().iter().find(|a| &*a.name.local == "clipPathUnits").map(|a| a.value.trim());
        let to_box = if units == Some("objectBoundingBox") {
            kurbo::Affine::new([border.width, 0.0, 0.0, border.height, border.x, border.y])
        } else {
            kurbo::Affine::translate((border.x, border.y))
        };
        path.apply_affine(to_box);
        Some((path, fill.unwrap_or(Fill::NonZero)))
    }

    /// Resolve a ShapeGeometryBox to a concrete rectangle (x, y, width, height) in scaled pixels
    ///
    /// For SVG elements without associated CSS layout box, the used value for content-box and padding-box is fill-box and for border-box and margin-box is stroke-box.
    /// For elements with associated CSS layout box, the used value for fill-box is content-box and for stroke-box and view-box is border-box.
    fn resolve_geometry_box(&self, geometry_box: &ShapeGeometryBox) -> ReferenceBox {
        match geometry_box {
            ShapeGeometryBox::ElementDependent
            | ShapeGeometryBox::StrokeBox
            | ShapeGeometryBox::ViewBox
            | ShapeGeometryBox::ShapeBox(ShapeBox::BorderBox) => ReferenceBox {
                x: 0.0,
                y: 0.0,
                width: self.frame.border_box.width() / self.scale,
                height: self.frame.border_box.height() / self.scale,
            },
            ShapeGeometryBox::ShapeBox(ShapeBox::PaddingBox) => ReferenceBox {
                x: self.frame.border_width.x0 / self.scale,
                y: self.frame.border_width.y0 / self.scale,
                width: self.frame.padding_box.width() / self.scale,
                height: self.frame.padding_box.height() / self.scale,
            },
            ShapeGeometryBox::FillBox | ShapeGeometryBox::ShapeBox(ShapeBox::ContentBox) => {
                ReferenceBox {
                    x: (self.frame.border_width.x0 + self.frame.padding_width.x0) / self.scale,
                    y: (self.frame.border_width.y0 + self.frame.padding_width.y0) / self.scale,
                    width: self.frame.content_box.width() / self.scale,
                    height: self.frame.content_box.height() / self.scale,
                }
            }
            ShapeGeometryBox::ShapeBox(ShapeBox::MarginBox) => {
                // Margin box is not tracked in CssBox, fall back to border box
                ReferenceBox {
                    x: 0.0,
                    y: 0.0,
                    width: self.frame.border_box.width() / self.scale,
                    height: self.frame.border_box.height() / self.scale,
                }
            }
        }
    }

    /// Convert a CSS basic-shape to a kurbo BezPath
    fn basic_shape_to_path(
        &self,
        shape: &BasicShape,
        reference_box: ReferenceBox,
    ) -> Option<BezPath> {
        let w = reference_box.width;
        let h = reference_box.height;
        let ox = reference_box.x;
        let oy = reference_box.y;

        match shape {
            GenericBasicShape::Circle(circle) => {
                let (cx, cy) = resolve_position(&circle.position, w, h, ox, oy);
                let r = resolve_shape_radius(&circle.radius, w, h, cx - ox, cy - oy);
                Some(Circle::new(Point::new(cx, cy), r).into_path(0.1))
            }
            GenericBasicShape::Ellipse(ellipse) => {
                let (cx, cy) = resolve_position(&ellipse.position, w, h, ox, oy);
                let rx = resolve_shape_radius(&ellipse.semiaxis_x, w, h, cx - ox, cy - oy);
                let ry = resolve_shape_radius(&ellipse.semiaxis_y, h, w, cy - oy, cx - ox);
                Some(Ellipse::new(Point::new(cx, cy), (rx, ry), 0.0).into_path(0.1))
            }
            GenericBasicShape::Polygon(polygon) => {
                let mut path = BezPath::new();
                let _fill = &polygon.fill;
                let coords = &polygon.coordinates;

                if coords.is_empty() {
                    return None;
                }

                for (i, coord) in coords.iter().enumerate() {
                    let x = ox + resolve_lp(&coord.0, w);
                    let y = oy + resolve_lp(&coord.1, h);
                    if i == 0 {
                        path.move_to(Point::new(x, y));
                    } else {
                        path.line_to(Point::new(x, y));
                    }
                }
                path.close_path();

                Some(path)
            }
            GenericBasicShape::Rect(inset_rect) => {
                // inset() function: inset(top right bottom left round border-radius)
                let top = resolve_lp(&inset_rect.rect.0, h);
                let right = resolve_lp(&inset_rect.rect.1, w);
                let bottom = resolve_lp(&inset_rect.rect.2, h);
                let left = resolve_lp(&inset_rect.rect.3, w);

                let x0 = ox + left;
                let y0 = oy + top;
                let x1 = ox + w - right;
                let y1 = oy + h - bottom;

                if x1 <= x0 || y1 <= y0 {
                    return None;
                }

                // TODO: Support border-radius on inset()
                Some(kurbo::Rect::new(x0, y0, x1, y1).into_path(0.1))
            }
            GenericBasicShape::PathOrShape(path_or_shape) => match path_or_shape {
                GenericPathOrShapeFunction::Path(path) => svg_path_to_bezpath(
                    path.commands(),
                    |v| *v as f64,
                    |v| *v as f64,
                    |v| *v as f64,
                )
                .map(|mut p| {
                    p.apply_affine(kurbo::Affine::translate((ox, oy)));
                    p
                }),
                GenericPathOrShapeFunction::Shape(shape) => svg_path_to_bezpath(
                    &shape.commands,
                    move |v: &LengthPercentage| {
                        v.resolve(CSSPixelLength::new(w as f32)).px() as f64
                    },
                    move |v: &LengthPercentage| {
                        v.resolve(CSSPixelLength::new(h as f32)).px() as f64
                    },
                    move |v: &Angle| v.degrees() as f64,
                )
                .map(|mut p| {
                    p.apply_affine(kurbo::Affine::translate((ox, oy)));
                    p
                }),
            },
        }
    }
}

fn fill_of(rule: FillRule) -> Fill {
    match rule {
        FillRule::Nonzero => Fill::NonZero,
        FillRule::Evenodd => Fill::EvenOdd,
    }
}

/// pyro: an SVG basic shape element as a path (user units); `None` for anything else, or a
/// shape without area.
fn svg_shape(el: &blitz_dom::ElementData) -> Option<BezPath> {
    let attr = |name: &str| el.attrs().iter().find(|a| &*a.name.local == name).map(|a| a.value.trim());
    let num = |name: &str| attr(name).and_then(|v| v.trim_end_matches("px").parse::<f64>().ok()).unwrap_or(0.0);
    let points = || {
        let text = attr("points").unwrap_or("");
        let nums: Vec<f64> =
            text.split(|c: char| c == ',' || c.is_ascii_whitespace()).filter_map(|n| n.parse().ok()).collect();
        nums.chunks_exact(2).map(|p| Point::new(p[0], p[1])).collect::<Vec<_>>()
    };
    match &*el.name.local {
        "path" => BezPath::from_svg(attr("d")?).ok(),
        "rect" => {
            let (w, h) = (num("width"), num("height"));
            if w <= 0.0 || h <= 0.0 {
                return None;
            }
            let rect = Rect::new(num("x"), num("y"), num("x") + w, num("y") + h);
            let (rx, ry) = match (attr("rx").is_some(), attr("ry").is_some()) {
                (false, false) => (0.0, 0.0),
                (true, false) => (num("rx"), num("rx")),
                (false, true) => (num("ry"), num("ry")),
                (true, true) => (num("rx"), num("ry")),
            };
            Some(if rx > 0.0 || ry > 0.0 {
                kurbo::RoundedRect::from_rect(rect, rx.min(w / 2.0).max(ry.min(h / 2.0))).into_path(0.1)
            } else {
                rect.into_path(0.1)
            })
        }
        "circle" => (num("r") > 0.0).then(|| Circle::new(Point::new(num("cx"), num("cy")), num("r")).into_path(0.1)),
        "ellipse" => (num("rx") > 0.0 && num("ry") > 0.0)
            .then(|| Ellipse::new(Point::new(num("cx"), num("cy")), (num("rx"), num("ry")), 0.0).into_path(0.1)),
        "polygon" | "polyline" => {
            let pts = points();
            let (first, rest) = pts.split_first()?;
            let mut path = BezPath::new();
            path.move_to(*first);
            for p in rest {
                path.line_to(*p);
            }
            path.close_path();
            Some(path)
        }
        _ => None,
    }
}

/// pyro: an SVG `transform` attribute (`matrix`, `translate`, `scale`, `rotate`, `skewX`,
/// `skewY`, in order); `None` when it does not parse.
fn svg_transform(text: &str) -> Option<kurbo::Affine> {
    let mut out = kurbo::Affine::IDENTITY;
    let mut rest = text.trim();
    while !rest.is_empty() {
        let open = rest.find('(')?;
        let close = rest.find(')')?;
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let args: Vec<f64> = rest[open + 1..close]
            .split(|c: char| c == ',' || c.is_ascii_whitespace())
            .filter(|s| !s.is_empty())
            .map(|s| s.parse().ok())
            .collect::<Option<_>>()?;
        let arg = |i: usize, d: f64| args.get(i).copied().unwrap_or(d);
        let t = match name {
            "matrix" if args.len() == 6 => kurbo::Affine::new([args[0], args[1], args[2], args[3], args[4], args[5]]),
            "translate" => kurbo::Affine::translate((arg(0, 0.0), arg(1, 0.0))),
            "scale" => kurbo::Affine::scale_non_uniform(arg(0, 1.0), arg(1, arg(0, 1.0))),
            "rotate" => {
                let (c, r) = (Point::new(arg(1, 0.0), arg(2, 0.0)), arg(0, 0.0).to_radians());
                kurbo::Affine::translate(c.to_vec2()) * kurbo::Affine::rotate(r) * kurbo::Affine::translate(-c.to_vec2())
            }
            "skewX" => kurbo::Affine::skew(arg(0, 0.0).to_radians().tan(), 0.0),
            "skewY" => kurbo::Affine::skew(0.0, arg(0, 0.0).to_radians().tan()),
            _ => return None,
        };
        out *= t;
        rest = rest[close + 1..].trim_start();
    }
    Some(out)
}

/// A resolved reference box in scaled pixel coordinates
#[derive(Clone, Copy)]
struct ReferenceBox {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl From<ReferenceBox> for kurbo::Rect {
    fn from(reference_box: ReferenceBox) -> Self {
        kurbo::Rect::new(
            reference_box.x,
            reference_box.y,
            reference_box.x + reference_box.width,
            reference_box.y + reference_box.height,
        )
    }
}

/// Resolve a LengthPercentage value against a basis length (already scaled)
fn resolve_lp(lp: &LengthPercentage, basis: f64) -> f64 {
    let basis_css = CSSPixelLength::new(basis as f32);
    lp.resolve(basis_css).px() as f64
}

/// Resolve a position (or auto => center) to absolute coordinates
fn resolve_position(
    position: &GenericPositionOrAuto<style::values::computed::Position>,
    w: f64,
    h: f64,
    ox: f64,
    oy: f64,
) -> (f64, f64) {
    match position {
        GenericPositionOrAuto::Auto => (ox + w / 2.0, oy + h / 2.0),
        GenericPositionOrAuto::Position(pos) => {
            let x = ox + resolve_lp(&pos.horizontal, w);
            let y = oy + resolve_lp(&pos.vertical, h);
            (x, y)
        }
    }
}

/// Resolve a shape radius keyword or length value
fn resolve_shape_radius(
    radius: &GenericShapeRadius<LengthPercentage>,
    primary_size: f64,
    secondary_size: f64,
    center_offset_primary: f64,
    center_offset_secondary: f64,
) -> f64 {
    match radius {
        GenericShapeRadius::Length(lp) => resolve_lp(&lp.0, primary_size),
        GenericShapeRadius::ClosestSide => center_offset_primary
            .min(primary_size - center_offset_primary)
            .min(center_offset_secondary)
            .min(secondary_size - center_offset_secondary)
            .max(0.0),
        GenericShapeRadius::FarthestSide => center_offset_primary
            .max(primary_size - center_offset_primary)
            .max(center_offset_secondary)
            .max(secondary_size - center_offset_secondary),

        GenericShapeRadius::FarthestCorner => {
            let dp = center_offset_primary.max(primary_size - center_offset_primary);
            let ds = center_offset_secondary.max(secondary_size - center_offset_secondary);
            dp.hypot(ds)
        }
        GenericShapeRadius::ClosestCorner => {
            let dp = center_offset_primary
                .min(primary_size - center_offset_primary)
                .max(0.0);
            let ds = center_offset_secondary
                .min(secondary_size - center_offset_secondary)
                .max(0.0);
            dp.hypot(ds)
        }
    }
}

type GenericPathCommand<Angle, N> = GenericShapeCommand<Angle, N, GenericPosition<N, N>, N>;

/// Convert an SVG path() to a kurbo BezPath.
/// The returned path is in the path's own coordinate system (origin at 0,0).
/// The caller is responsible for translating it to the reference box origin.
fn svg_path_to_bezpath<Angle: Copy, N>(
    commands: &[GenericPathCommand<Angle, N>],
    resolve_x: impl Fn(&N) -> f64,
    resolve_y: impl Fn(&N) -> f64,
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
                // SVG arc commands are complex; approximate as a line for now
                let p = resolve_endpoint(point, cur, &resolve_x, &resolve_y);
                let svg_arc = SvgArc {
                    from: cur,
                    to: p,
                    radii: Vec2 {
                        x: resolve_x(&radii.rx),
                        y: resolve_y(radii.ry.as_ref().unwrap_or(&radii.rx)),
                    },
                    x_rotation: resolve_angle(rotate),
                    large_arc: match arc_size {
                        ArcSize::Large => true,
                        ArcSize::Small => false,
                    },
                    // TODO: check mapping is correct
                    sweep: match arc_sweep {
                        ArcSweep::Ccw => true,
                        ArcSweep::Cw => false,
                    },
                };
                let arc = kurbo::Arc::from_svg_arc(&svg_arc)?;
                for el in arc.append_iter(0.1) {
                    bez.push(el);
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
