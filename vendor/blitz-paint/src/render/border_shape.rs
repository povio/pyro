//! pyro: `border-shape` (CSS Borders 4 §11), as Blink's `BorderShapePainter` draws it.
//!
//! With one shape the border is a stroke along it, as wide as the relevant side's border and in
//! its colour; with two it fills the area between the outer and the inner one. `border-radius`
//! and `corner-shape` are then ignored. The background is clipped to the inner shape (to the
//! shape itself with one), the overflow to the inner edge of the border, and shadows and the
//! outline follow its outer edge.

use super::ElementCx;
use blitz_dom::CopyStyleGetters;
use super::clip_path::ElementClip;
use crate::color::{Color, ToColorColor as _};
use crate::kurbo_css::contoured::{boolean, flatten_with, polygons_path};

/// How finely shape outlines are flattened, and their strokes expanded (device px): as vello_cpu
/// strokes and fills curves (`vello_common::flatten::TOL`), so that an edge worked out here (the
/// inner edge of a stroke, which clips the overflow) falls where the drawn stroke's does.
const TOLERANCE: f64 = 0.25;

fn flatten(path: &BezPath) -> Vec<Vec<Point>> {
    flatten_with(path, TOLERANCE)
}

/// `path` with every subpath closed: a stroke draws the closing side (shapes built for fills,
/// such as a rectangle's, end at their last corner).
/// pyro: the oval filling `rect`, drawn as a `border-radius: 50%` box is (as `ellipse()` is);
/// `None` for an empty rectangle.
pub(super) fn oval_path(rect: Rect) -> Option<BezPath> {
    if !(rect.width() > 0.0 && rect.height() > 0.0) {
        return None;
    }
    let radius = kurbo::Vec2::new(rect.width() / 2.0, rect.height() / 2.0);
    let radii = crate::kurbo_css::NonUniformRoundedRectRadii {
        top_left: radius,
        top_right: radius,
        bottom_right: radius,
        bottom_left: radius,
    };
    let insets = kurbo::Insets::ZERO;
    Some(closed(&crate::kurbo_css::CssBox::new(rect, insets, insets, 0.0, radii).border_box_path()))
}

fn closed(path: &BezPath) -> BezPath {
    let mut out = BezPath::new();
    let mut open = false;
    let mut start = Point::ZERO;
    // pyro: a subpath whose last segment ends a hair from its start (an arc to the start point
    // computed in floating point) ends exactly there: the close then adds no segment of its own,
    // whose arbitrary direction made a stray miter at the vertex (WPT css-borders
    // `border-shape-shape-arc-miter-limit`).
    let snap_end = |out: &mut BezPath, start: Point| {
        let elements = out.elements_mut();
        if let Some(last) = elements.last_mut() {
            let end = match last {
                PathEl::LineTo(p) | PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => Some(p),
                _ => None,
            };
            if let Some(end) = end.filter(|end| end.distance(start) < 1e-6) {
                *end = start;
            }
        }
    };
    for el in path.elements() {
        match el {
            PathEl::MoveTo(p) => {
                if open {
                    snap_end(&mut out, start);
                    out.close_path();
                }
                open = true;
                start = *p;
            }
            PathEl::ClosePath => {
                snap_end(&mut out, start);
                open = false;
            }
            _ => {}
        }
        out.push(*el);
    }
    if open {
        snap_end(&mut out, start);
        out.close_path();
    }
    out
}
use anyrender::PaintScene;
use kurbo::{BezPath, Cap, Join, PathEl, Point, Rect, Shape as _, Stroke, StrokeOpts};
use peniko::Fill;
use style::values::computed::basic_shape::BasicShape;
use style::values::generics::basic_shape::{
    BorderShapeBox, GenericBasicShape, GenericBorderShape, GenericBorderShapeItem, ShapeBox,
};
use style::values::specified::BorderStyle;

/// An element's resolved `border-shape`, in device px in the element's space.
pub(super) struct BorderShape {
    /// The outer shape (the only one in stroke mode).
    outer: BezPath,
    /// The inner shape, with two.
    pub(super) inner: Option<BezPath>,
    /// pyro: the inner shape is an oval inside the outer oval: the border between them is the two
    /// curves filled even-odd (`ring`), as a `border-radius` box's border is drawn.
    nested_ovals: bool,
    /// pyro: nested ovals a uniform width apart around one centre: the border is their centre-line
    /// ellipse stroked, as a solid `border-radius: 50%` border is drawn (`border.rs`).
    concentric_ovals: Option<(kurbo::Ellipse, f64)>,
    /// The outer shape's box when it is a circle or an ellipse (Skia grows those as ovals).
    pub(super) oval: Option<Rect>,
    /// Whether the outer shape is all straight lines: its stroke's miters are then never cut.
    polygon: bool,
    /// The relevant side's border width (the stroke's width) and colour.
    width: f64,
    color: Color,
}

impl BorderShape {
    /// The stroke Blink draws a border shape with (`GetStrokeData`): mitered, the miters of a
    /// polygon never cut, those of curves at 4 widths.
    fn stroke(&self, width: f64) -> Stroke {
        Stroke::new(width)
            .with_join(Join::Miter)
            .with_miter_limit(if self.polygon { 1e10 } else { 4.0 })
            .with_caps(Cap::Butt)
    }

    /// The area `path`'s stroke of `width` covers, as polygons.
    fn stroke_area(&self, path: &BezPath, width: f64) -> Vec<Vec<Point>> {
        let outline = kurbo::stroke(
            path.elements().iter().copied(),
            &self.stroke(width),
            &StrokeOpts::default(),
            TOLERANCE,
        );
        flatten(&outline)
    }

    /// The outer edge of the border: the outer shape, grown by half the stroke in stroke mode
    /// (`BorderShapePainter::OuterPath`).
    pub(super) fn outer_path(&self) -> BezPath {
        if self.inner.is_some() || self.width <= 0.0 {
            return self.outer.clone();
        }
        let shapes = [flatten(&self.outer), self.stroke_area(&self.outer, self.width)];
        boolean(&shapes, |inside| inside[0] || inside[1])
    }

    /// The inner edge of the border: the inner shape, or the shape less half the stroke in
    /// stroke mode (`BorderShapePainter::InnerPath`). It clips the overflow.
    pub(super) fn inner_path(&self) -> BezPath {
        if let Some(inner) = &self.inner {
            return inner.clone();
        }
        if self.width <= 0.0 {
            return self.outer.clone();
        }
        let shapes = [flatten(&self.outer), self.stroke_area(&self.outer, self.width)];
        boolean(&shapes, |inside| inside[0] && !inside[1])
    }

    /// What the background is clipped to: the inner shape, or the shape in stroke mode (the
    /// stroke is drawn over it).
    pub(super) fn background_clip(&self) -> BezPath {
        match (&self.inner, self.oval) {
            (Some(inner), _) => inner.clone(),
            // pyro: an oval as a `border-radius: 50%` box's edge (`oval_path`), as its shadow is
            // drawn: another description of the curve took its anti-aliased pixels apart (WPT
            // css-borders `border-shape-shadow-blur`).
            (None, Some(oval)) => oval_path(oval).unwrap_or_else(|| self.outer.clone()),
            (None, None) => self.outer.clone(),
        }
    }

    /// The outer edge moved out by `offset` (in, when negative), for shadows and outlines
    /// (`BorderShapePainter::OuterPathWithOffset`): an oval grows as an oval, anything else by
    /// its stroke.
    /// pyro: the visible outer edge's path: the outer shape, or with one shape (the border is
    /// drawn across it) the outer side of its stroke of the border width, in exact curves (not
    /// flattened as [`Self::outer_path_with_offset`]'s).
    pub(super) fn outer_edge(&self) -> BezPath {
        if self.inner.is_some() || self.width <= 0.0 {
            return self.outer.clone();
        }
        let stroked = kurbo::stroke(
            self.outer.iter(),
            &kurbo::Stroke::new(self.width),
            &kurbo::StrokeOpts::default(),
            0.01,
        );
        // Its contours: the one with the largest bounds is the outer side.
        let mut best: Option<(f64, BezPath)> = None;
        let mut current = BezPath::new();
        let mut take = |path: BezPath, best: &mut Option<(f64, BezPath)>| {
            use kurbo::Shape as _;
            let area = path.bounding_box().area();
            if path.elements().len() > 1 && best.as_ref().is_none_or(|(a, _)| area > *a) {
                *best = Some((area, path));
            }
        };
        for el in stroked.elements() {
            if let kurbo::PathEl::MoveTo(_) = el {
                take(std::mem::take(&mut current), &mut best);
            }
            current.push(*el);
        }
        take(current, &mut best);
        best.map_or_else(|| self.outer.clone(), |(_, path)| path)
    }

    pub(super) fn outer_path_with_offset(&self, offset: f64) -> BezPath {
        let total = offset + if self.inner.is_none() { self.width / 2.0 } else { 0.0 };
        if total == 0.0 {
            return self.outer.clone();
        }
        if let Some(oval) = self.oval {
            if let Some(path) = oval_path(oval.inflate(total, total)) {
                return path;
            }
            return BezPath::new();
        }
        let shapes = [flatten(&self.outer), self.stroke_area(&self.outer, total.abs() * 2.0)];
        if total > 0.0 {
            boolean(&shapes, |inside| inside[0] || inside[1])
        } else {
            boolean(&shapes, |inside| inside[0] && !inside[1])
        }
    }

    /// pyro: the box of the inner edge when it is a circle (an oval shape without an inner one):
    /// its inset shadow is drawn as a `border-radius: 50%` box's.
    pub(super) fn inner_circle(&self) -> Option<Rect> {
        if self.inner.is_some() {
            return None;
        }
        let half = self.width.max(0.0) / 2.0;
        let inner = self.oval?.inflate(-half, -half);
        ((inner.width() - inner.height()).abs() < 1e-6 && inner.width() > 0.0).then_some(inner)
    }

    /// The box of the outer edge moved out by `offset` when it is a circle: vello's blurred
    /// rounded rectangle draws its shadow as a `border-radius: 50%` box's.
    pub(super) fn circle_with_offset(&self, offset: f64) -> Option<Rect> {
        let total = offset + if self.inner.is_none() { self.width / 2.0 } else { 0.0 };
        let grown = self.oval?.inflate(total, total);
        ((grown.width() - grown.height()).abs() < 1e-6 && grown.width() > 0.0).then_some(grown)
    }

    /// `path` moved out by `offset` (in, when negative) along its normals: united with its
    /// stroke of twice the offset, or less it, mitered at 4 widths (Blink's default
    /// `StrokeData`, as an inner shadow's spread is applied).
    pub(super) fn offset_path(&self, path: &BezPath, offset: f64) -> BezPath {
        if offset == 0.0 {
            return path.clone();
        }
        let stroke = Stroke::new(offset.abs() * 2.0).with_join(Join::Miter).with_miter_limit(4.0);
        let outline = kurbo::stroke(path.elements().iter().copied(), &stroke, &StrokeOpts::default(), TOLERANCE);
        let shapes = [flatten(path), flatten(&outline)];
        if offset > 0.0 {
            boolean(&shapes, |inside| inside[0] || inside[1])
        } else {
            boolean(&shapes, |inside| inside[0] && !inside[1])
        }
    }

    /// pyro: the border between two nested ovals as their curves, filled even-odd.
    fn ring(&self) -> Option<BezPath> {
        let inner = self.inner.as_ref().filter(|_| self.nested_ovals)?;
        let mut ring = self.outer.clone();
        ring.extend(inner.iter());
        Some(ring)
    }

    /// The area the border covers (`background-clip: border-area` paints there): between the
    /// shapes, or the stroke along the shape (`BorderShapePainter::PaintBorderArea`).
    pub(super) fn border_area_path(&self) -> BezPath {
        // pyro: between concentric ovals, the stroke the border is drawn as (`paint`), so the
        // area and the border are one shape (WPT css-backgrounds `clip-border-area-border-shape`).
        if let Some((ellipse, width)) = self.concentric_ovals {
            return kurbo::stroke(ellipse.path_elements(TOLERANCE), &Stroke::new(width), &StrokeOpts::default(), TOLERANCE);
        }
        if let Some(inner) = &self.inner {
            let shapes = [flatten(&self.outer), flatten(inner)];
            return boolean(&shapes, |inside| inside[0] && !inside[1]);
        }
        if self.width <= 0.0 {
            return BezPath::new();
        }
        polygons_path(&self.stroke_area(&self.outer, self.width))
    }

    /// Draws the border: the area between the shapes, or the stroke along the shape.
    pub(super) fn paint(&self, scene: &mut impl PaintScene, transform: kurbo::Affine) {
        if self.color.components[3] <= 0.0 {
            return;
        }
        if let Some((ellipse, width)) = self.concentric_ovals {
            scene.stroke(&Stroke::new(width), transform, self.color, None, &ellipse);
            return;
        }
        if let Some(ring) = self.ring() {
            scene.fill(Fill::EvenOdd, transform, self.color, None, &ring);
            return;
        }
        if let Some(inner) = &self.inner {
            let shapes = [flatten(&self.outer), flatten(inner)];
            let area = boolean(&shapes, |inside| inside[0] && !inside[1]);
            scene.fill(Fill::NonZero, transform, self.color, None, &area);
            return;
        }
        // A stroke of no width would be a hairline: nothing is drawn.
        if self.width <= 0.0 {
            return;
        }
        scene.stroke(&self.stroke(self.width), transform, self.color, None, &self.outer);
    }
}

impl ElementCx<'_, '_> {
    /// pyro: this element's `border-shape`, resolved; `None` for `none` (or a shape that cannot
    /// be drawn).
    pub(super) fn border_shape(&self) -> Option<BorderShape> {
        let (outer, inner) = match &self.style.get_border().border_shape {
            GenericBorderShape::None => return None,
            GenericBorderShape::Single(item) => (item, None),
            GenericBorderShape::Double(outer, inner) => (outer, Some(inner)),
        };
        let path = |item: &GenericBorderShapeItem<BasicShape>, default: BorderShapeBox| {
            let reference = match item.reference {
                BorderShapeBox::Default => default,
                other => other,
            };
            let reference = self.border_shape_reference(reference);
            match self.basic_shape_clip_with(&item.shape, reference, false) {
                ElementClip::Shape(mut path, _) => {
                    path.apply_affine(kurbo::Affine::scale(self.scale));
                    // pyro: an oval as a `border-radius: 50%` box's edge (`oval_path`), the
                    // curve its border, outline and shadows are compared with (WPT css-borders
                    // `border-shape-outline-with-border-ellipse`).
                    if matches!(*item.shape, GenericBasicShape::Circle(_) | GenericBasicShape::Ellipse(_)) {
                        if let Some(oval) = oval_path(path.bounding_box()) {
                            return oval;
                        }
                    }
                    closed(&path)
                }
                _ => BezPath::new(),
            }
        };
        let (outer_default, inner_default) = if inner.is_some() {
            (
                BorderShapeBox::ShapeBox(ShapeBox::BorderBox),
                BorderShapeBox::ShapeBox(ShapeBox::PaddingBox),
            )
        } else {
            (BorderShapeBox::HalfBorderBox, BorderShapeBox::HalfBorderBox)
        };
        let outer_path = path(outer, outer_default);
        let inner_path = inner.map(|inner| path(inner, inner_default));
        let oval = matches!(*outer.shape, GenericBasicShape::Circle(_) | GenericBasicShape::Ellipse(_))
            .then(|| outer_path.bounding_box())
            .filter(|b| b.width() > 0.0 && b.height() > 0.0);
        let is_oval = |shape: &BasicShape| matches!(shape, GenericBasicShape::Circle(_) | GenericBasicShape::Ellipse(_));
        let nested_ovals = oval.is_some()
            && inner.is_some_and(|inner| is_oval(&inner.shape))
            && inner_path.as_ref().is_some_and(|inner_path| {
                let (o, i) = (outer_path.bounding_box(), inner_path.bounding_box());
                i.x0 >= o.x0 && i.y0 >= o.y0 && i.x1 <= o.x1 && i.y1 <= o.y1
            });
        let concentric_ovals = inner_path.as_ref().filter(|_| nested_ovals).and_then(|inner_path| {
            let (o, i) = (outer_path.bounding_box(), inner_path.bounding_box());
            let width = (o.width() - i.width()) / 2.0;
            let same = |a: f64, b: f64| (a - b).abs() < 1e-3;
            (width > 0.0 && same(width, (o.height() - i.height()) / 2.0) && same(o.center().x, i.center().x)
                && same(o.center().y, i.center().y))
            .then(|| {
                let radii = ((o.width() + i.width()) / 4.0, (o.height() + i.height()) / 4.0);
                (kurbo::Ellipse::new(o.center(), radii, 0.0), width)
            })
        });
        let polygon = outer_path
            .elements()
            .iter()
            .all(|el| matches!(el, PathEl::MoveTo(_) | PathEl::LineTo(_) | PathEl::ClosePath));
        let (width, color) = self.border_shape_side();
        Some(BorderShape {
            outer: outer_path,
            inner: inner_path,
            nested_ovals,
            concentric_ovals,
            oval,
            polygon,
            width,
            color,
        })
    }

    /// The box a `border-shape` shape is drawn in (CSS px from the border box's origin):
    /// `half-border-box` is the border box less half of each border width; `fill-box` is the
    /// content box and `stroke-box` and `view-box` the border box, as for `clip-path`.
    fn border_shape_reference(&self, reference: BorderShapeBox) -> Rect {
        let frame = &self.frame;
        let device = match reference {
            BorderShapeBox::Default
            | BorderShapeBox::HalfBorderBox => {
                let b = frame.border_width;
                Rect::new(
                    frame.border_box.x0 + b.x0 / 2.0,
                    frame.border_box.y0 + b.y0 / 2.0,
                    frame.border_box.x1 - b.x1 / 2.0,
                    frame.border_box.y1 - b.y1 / 2.0,
                )
            }
            BorderShapeBox::StrokeBox
            | BorderShapeBox::ViewBox
            | BorderShapeBox::ShapeBox(ShapeBox::BorderBox) => frame.border_box,
            BorderShapeBox::ShapeBox(ShapeBox::PaddingBox) => frame.padding_box,
            BorderShapeBox::FillBox | BorderShapeBox::ShapeBox(ShapeBox::ContentBox) => {
                frame.content_box
            }
            BorderShapeBox::ShapeBox(ShapeBox::MarginBox) => {
                let to_css = 1.0 / self.scale;
                let border_box = Rect::new(
                    frame.border_box.x0 * to_css,
                    frame.border_box.y0 * to_css,
                    frame.border_box.x1 * to_css,
                    frame.border_box.y1 * to_css,
                );
                return border_box + self.margins();
            }
        };
        let to_css = 1.0 / self.scale;
        Rect::new(device.x0 * to_css, device.y0 * to_css, device.x1 * to_css, device.y1 * to_css)
    }

    /// The width (device px) and colour of the relevant side for `border-shape`: the first of
    /// block-start, inline-start, block-end and inline-end whose style is not `none`, else
    /// block-start.
    fn border_shape_side(&self) -> (f64, Color) {
        use style::logical_geometry::PhysicalSide;
        let border = self.style.get_border();
        let wm = self.style.writing_mode;
        let side = |s: PhysicalSide| match s {
            PhysicalSide::Top => (border.border_top_style, &border.border_top_color, self.frame.border_width.y0),
            PhysicalSide::Right => (border.border_right_style, &border.border_right_color, self.frame.border_width.x1),
            PhysicalSide::Bottom => (border.border_bottom_style, &border.border_bottom_color, self.frame.border_width.y1),
            PhysicalSide::Left => (border.border_left_style, &border.border_left_color, self.frame.border_width.x0),
        };
        let order = [
            wm.block_start_physical_side(),
            wm.inline_start_physical_side(),
            wm.block_end_physical_side(),
            wm.inline_end_physical_side(),
        ];
        let (_, color, width) = order
            .iter()
            .map(|&s| side(s))
            .find(|(style, _, _)| *style != BorderStyle::None)
            .unwrap_or_else(|| side(order[0]));
        let color = color
            .resolve_to_absolute(&self.style.clone_color())
            .as_srgb_color();
        (width, color)
    }
}
