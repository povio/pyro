use kurbo::{Arc, BezPath, Insets, PathEl, Point, Rect, Shape as _, Vec2};
use std::{f64::consts::FRAC_PI_2, f64::consts::PI};

use super::contoured::{self, ContouredRect, CornerCurvature};
use super::non_uniform_radii::NonUniformRoundedRectRadii;
use super::{Corner, CssBoxKind, Direction, Edge, add_insets, get_corner_insets};

/// There are several nested boxes at play here:
/// We have 4 boxes, 4 corners, and clockwise/anticlockwise for a total of 16 different options
///
/// ```a
///    *--------------------------------------------------------*  <--- CssBox::OutlineBox
///    |                         Outline                        |
///    |    *----------------------------------------------*    |  <--- CssBox::BorderBox
///    |    |                    Border                    |    |
///    |    |    *------------------------------------*    |    |  <--- CssBox::PaddingBox
///    |    |    |               Padding              |    |    |
///    |    |    |    *--------------------------*    |    |    |  <--- CssBox::ContentBox
///    |    |    |    |          Content         |    |    |    |
///    |    |    |    |                          |    |    |    |
///    |    |    |    |                          |    |    |    |
///    |    |    |    |                          |    |    |    |
///    |    |    |    *--------------------------*    |    |    |
///    |    |    |                                    |    |    |
///    |    |    *------------------------------------*    |    |
///    |    |                                              |    |
///    |    *----------------------------------------------*    |
///    |                                                        |
///    *--------------------------------------------------------*
/// ```
///
#[derive(Debug, Clone)]
pub struct CssBox {
    pub border_box: Rect,
    pub padding_box: Rect,
    pub content_box: Rect,
    pub outline_box: Rect,

    pub padding_width: Insets,
    pub border_width: Insets,
    pub outline_width: f64,

    pub border_radii: NonUniformRoundedRectRadii,

    /// pyro: what the padding box's and the content box's radii (each border radius less the
    /// widths beside it) are scaled by to fit their box, 1 when they fit (see
    /// [`inner_radii_scale`]).
    padding_radii_scale: f64,
    content_radii_scale: f64,

    /// pyro: the corners' shapes (`corner-shape`), when one is not round, or round ones whose
    /// inner radii overflow their box: each corner's superellipse exponent
    /// ([`CssBox::with_corner_curvature`]). The box's edges are then [`ContouredRect`]s.
    pub corner_curvature: Option<CornerCurvature>,
    /// pyro: the border box (with its radii) this box's edges stay parallel to, when this box
    /// is one derived from an element's (an outline's, a shadow's, a line of a double border):
    /// its own border box otherwise.
    pub contour_origin: Option<(Rect, NonUniformRoundedRectRadii)>,
}

/// pyro: the scale an inner box's radii need to fit it, as Blink fits them
/// (`FloatRoundedRect::AdjustRadii` when the rect is not renderable): CSS Backgrounds 3 §5.5 has
/// the padding edge's radii the border radii less the border widths, which overflow a box with
/// one large corner (`border-radius: 100% 0 0 0` and a border), and its curve then left the box.
fn inner_radii_scale(rect: Rect, radii: &NonUniformRoundedRectRadii, insets: Insets) -> f64 {
    let inner = |r: Vec2, corner: Corner| {
        let r = r - get_corner_insets(insets, corner);
        if r.x <= 0.0 || r.y <= 0.0 {
            Vec2::ZERO
        } else {
            r
        }
    };
    let (tl, tr) = (
        inner(radii.top_left, Corner::TopLeft),
        inner(radii.top_right, Corner::TopRight),
    );
    let (br, bl) = (
        inner(radii.bottom_right, Corner::BottomRight),
        inner(radii.bottom_left, Corner::BottomLeft),
    );
    let widest = (tl.x + tr.x).max(bl.x + br.x);
    let tallest = (tl.y + bl.y).max(tr.y + br.y);
    if widest <= 0.0 || tallest <= 0.0 {
        return 1.0;
    }
    (rect.width().max(0.0) / widest)
        .min(rect.height().max(0.0) / tallest)
        .min(1.0)
}

impl CssBox {
    pub fn new(
        border_box: Rect,
        border: Insets,
        padding: Insets,
        outline_width: f64,
        mut border_radii: NonUniformRoundedRectRadii,
    ) -> Self {
        let padding_box = border_box - border;
        let content_box = padding_box - padding;
        let outline_box = border_box.inset(outline_width);

        // Correct the border radii if they are too big if two border radii would intersect, then we need to shrink
        // ALL border radii by the same factor such that they do not
        let top_overlap_factor =
            border_box.width() / (border_radii.top_left.x + border_radii.top_right.x);
        let bottom_overlap_factor =
            border_box.width() / (border_radii.bottom_left.x + border_radii.bottom_right.x);
        let left_overlap_factor =
            border_box.height() / (border_radii.top_left.y + border_radii.bottom_left.y);
        let right_overlap_factor =
            border_box.height() / (border_radii.top_right.y + border_radii.bottom_right.y);

        let min_factor = top_overlap_factor
            .min(bottom_overlap_factor)
            .min(left_overlap_factor)
            .min(right_overlap_factor)
            .min(1.0);
        if min_factor < 1.0 {
            border_radii *= min_factor
        }

        let padding_radii_scale = inner_radii_scale(padding_box, &border_radii, border);
        let content_radii_scale =
            inner_radii_scale(content_box, &border_radii, add_insets(border, padding));

        Self {
            padding_box,
            border_box,
            content_box,
            outline_box,
            outline_width,
            padding_width: padding,
            border_width: border,
            border_radii,
            padding_radii_scale,
            content_radii_scale,
            corner_curvature: None,
            contour_origin: None,
        }
    }

    /// pyro: this box with shaped corners (`corner-shape`), `curvature` holding each corner's
    /// superellipse exponent (`2^k` for `superellipse(k)`). A corner without a radius stays
    /// round (Blink's `EffectiveCurvature`), and a box with a concave corner has its radii scaled
    /// so that the hulls of opposite corners do not overlap (`ComputeContouredBorderFromStyle`).
    /// A box whose corners are all round is left as it is, unless the radii of its padding or
    /// content box overflow that box (`border-radius: 100% 0 0 0` and a border): its inner edges
    /// are then contours too, their radii the border radii less the widths and their curves
    /// parallel to the border's, cut by their box, as Blink's `ContouredInnerBorder` draws them
    /// (WPT css-borders `corner-shape/render-corner-shape.html?corner-shape=1&
    /// border-top-left-radius=100%&border-width=20`), not radii scaled to fit.
    pub fn with_corner_curvature(mut self, mut curvature: CornerCurvature) -> Self {
        let radii = self.border_radii;
        for (c, r) in [
            (&mut curvature.top_left, radii.top_left),
            (&mut curvature.top_right, radii.top_right),
            (&mut curvature.bottom_right, radii.bottom_right),
            (&mut curvature.bottom_left, radii.bottom_left),
        ] {
            if r.x <= 0.0 || r.y <= 0.0 {
                *c = contoured::ROUND;
            }
        }
        // (A scale a rounding error under 1, as `border-radius: 9999px` scaled to fit gives, fits.)
        let fits = |scale: f64| scale >= 1.0 - 1e-9;
        if curvature.is_round() && fits(self.padding_radii_scale) && fits(self.content_radii_scale) {
            self.corner_curvature = None;
            return self;
        }
        self.border_radii =
            contoured::constrain_opposite_corners(self.border_box, self.border_radii, &curvature);
        self.corner_curvature = Some(curvature);
        self
    }

    /// pyro: this box (an outline's, a shadow's) shaped as `element`'s corners are, its curves
    /// parallel to `element`'s border box's.
    /// (Not round corners, contoured only for their inner edges: the boxes derived from them
    /// keep the spread rule's rounded rects, as Blink's.)
    pub fn contoured_like(mut self, element: &CssBox) -> Self {
        if let Some(curvature) = element.corner_curvature.filter(|c| !c.is_round()) {
            self.corner_curvature = Some(curvature);
            self.contour_origin = Some(element.contour_origin());
        }
        self
    }

    fn contour_origin(&self) -> (Rect, NonUniformRoundedRectRadii) {
        self.contour_origin
            .unwrap_or((self.border_box, self.border_radii))
    }

    /// pyro: the edge of one of this box's boxes as a [`ContouredRect`], when its corners are
    /// shaped (Blink's `ContouredBorder` and `ContouredInnerBorder`): an inner box's radii are the
    /// border radii less the widths between (each stopping at zero), and its curves stay
    /// parallel to the border's.
    pub fn contour(&self, kind: CssBoxKind) -> Option<ContouredRect> {
        let (rect, insets) = match kind {
            CssBoxKind::OutlineBox => return None,
            CssBoxKind::BorderBox => (self.border_box, Insets::ZERO),
            CssBoxKind::PaddingBox => (self.padding_box, self.border_width),
            CssBoxKind::ContentBox => (
                self.content_box,
                add_insets(self.border_width, self.padding_width),
            ),
        };
        self.contour_within(rect, insets)
    }

    /// pyro: the border box inset by `insets` as a [`ContouredRect`], when the corners are
    /// shaped: its radii the border radii less the insets (each stopping at zero) and its curves
    /// parallel to the border's, as [`CssBox::contour`] gives the padding and content boxes (an
    /// inset shadow's hole, a background cut to avoid bleeding under the border; Blink insets a
    /// `ContouredRect` so, not scaling radii that overflow the smaller box).
    pub fn inset_contour(&self, insets: Insets) -> Option<ContouredRect> {
        self.contour_within(self.border_box - insets, insets)
    }

    fn contour_within(&self, rect: Rect, insets: Insets) -> Option<ContouredRect> {
        let curvature = self.corner_curvature?;
        let shrink = |r: Vec2, corner: Corner| {
            let inset = get_corner_insets(insets, corner);
            Vec2::new(
                if r.x > 0.0 { (r.x - inset.x).max(0.0) } else { r.x },
                if r.y > 0.0 { (r.y - inset.y).max(0.0) } else { r.y },
            )
        };
        let r = &self.border_radii;
        let radii = NonUniformRoundedRectRadii {
            top_left: shrink(r.top_left, Corner::TopLeft),
            top_right: shrink(r.top_right, Corner::TopRight),
            bottom_right: shrink(r.bottom_right, Corner::BottomRight),
            bottom_left: shrink(r.bottom_left, Corner::BottomLeft),
        };
        let origin = self.contour_origin();
        let identity = origin == (rect, radii);
        Some(ContouredRect {
            rect,
            radii,
            curvature,
            origin: (!identity).then_some(origin),
        })
    }

    /// pyro: the area of the border (the border box less the padding box) of a box with shaped
    /// corners, and with `side` the part of it that side paints.
    fn contoured_border_area(&self, side: Option<Edge>) -> Option<BezPath> {
        let outer = self.contour(CssBoxKind::BorderBox)?;
        let inner = self.contour(CssBoxKind::PaddingBox)?;
        let mut shapes = vec![contoured::flatten(&outer.path()), contoured::flatten(&inner.path())];
        if let Some(edge) = side {
            shapes.push(vec![side_region(self.border_box, self.border_width, edge)]);
        }
        Some(contoured::boolean(&shapes, |inside| {
            inside[0] && !inside[1] && inside.get(2).copied().unwrap_or(true)
        }))
    }

    /// Construct a BezPath representing one edge of a box's border.
    /// Takes into account border-radius and the possibility that the edges
    /// are different colors.
    ///
    /// Will construct the border by:
    /// - drawing an inner arc
    /// - jumping to an outer arc
    /// - jumping to the next outer arc (completing the edge with the previous)
    /// - drawing an inner arc
    pub fn border_edge_shape(&self, edge: Edge) -> BezPath {
        use {Corner::*, CssBoxKind::*, Direction::*, Edge::*};

        if let Some(path) = self.contoured_border_area(Some(edge)) {
            return path;
        }

        let mut path = BezPath::new();

        let (c0, c1) = match edge {
            Top => (TopLeft, TopRight),
            Right => (TopRight, BottomRight),
            Bottom => (BottomRight, BottomLeft),
            Left => (BottomLeft, TopLeft),
        };

        // 1. First corner
        // if the radius is bigger than the border, we need to draw the inner arc to fill in the gap
        if self.is_sharp(c0, BorderBox) {
            path.move_to(self.corner(c0, PaddingBox));
            path.line_to(self.corner(c0, BorderBox));
        } else {
            match self.corner_needs_infill(c0) {
                true => {
                    path.insert_arc(self.partial_corner_arc(c0, PaddingBox, edge, Anticlockwise))
                }
                false => path.move_to(self.corner(c0, PaddingBox)),
            }
            path.insert_arc(self.partial_corner_arc(c0, BorderBox, edge, Clockwise));
        }

        // 2. Second corner
        if self.is_sharp(c1, BorderBox) {
            path.line_to(self.corner(c1, BorderBox));
            path.line_to(self.corner(c1, PaddingBox));
        } else {
            path.insert_arc(self.partial_corner_arc(c1, BorderBox, edge, Clockwise));
            match self.corner_needs_infill(c1) {
                true => {
                    path.insert_arc(self.partial_corner_arc(c1, PaddingBox, edge, Anticlockwise))
                }
                false => path.line_to(self.corner(c1, PaddingBox)),
            }
        }

        path
    }

    /// Whether any corner of this box has a non-zero border radius.
    pub fn has_border_radius(&self) -> bool {
        let r = &self.border_radii;
        [r.top_left, r.top_right, r.bottom_right, r.bottom_left]
            .iter()
            .any(|radius| radius.x > 0.0 || radius.y > 0.0)
    }

    /// Construct a new [`CssBox`] representing a "slice" of this box's border,
    /// running from `start_frac` to `end_frac` of the border width (measured as
    /// a fraction from the outer border-box edge inwards).
    ///
    /// The returned box's border region is exactly the requested slice, so
    /// [`CssBox::border_edge_shape`] can then be used to render it. This is used
    /// to draw the two lines of a `double` border.
    pub fn border_slice(&self, start_frac: f64, end_frac: f64) -> CssBox {
        use Corner::*;

        let scale_insets = |frac: f64| Insets {
            x0: self.border_width.x0 * frac,
            y0: self.border_width.y0 * frac,
            x1: self.border_width.x1 * frac,
            y1: self.border_width.y1 * frac,
        };

        let start_insets = scale_insets(start_frac);
        let slice_border = scale_insets(end_frac - start_frac);

        // Move the outer edge of the box inwards to the start of the slice.
        let slice_border_box = self.border_box - start_insets;

        // Border radii shrink as we move inwards through the border, matching the
        // model used when computing inner (padding/content) box radii.
        let reduce = |radius: Vec2, corner: Corner| {
            let inset = get_corner_insets(start_insets, corner);
            Vec2 {
                x: (radius.x - inset.x).max(0.0),
                y: (radius.y - inset.y).max(0.0),
            }
        };
        let slice_radii = NonUniformRoundedRectRadii {
            top_left: reduce(self.border_radii.top_left, TopLeft),
            top_right: reduce(self.border_radii.top_right, TopRight),
            bottom_right: reduce(self.border_radii.bottom_right, BottomRight),
            bottom_left: reduce(self.border_radii.bottom_left, BottomLeft),
        };

        CssBox::new(
            slice_border_box,
            slice_border,
            Insets::ZERO,
            0.0,
            slice_radii,
        )
        .contoured_like(self)
    }

    /// Construct a bezpath drawing the outline
    // Unused since `draw_outline` builds the outline's own box, which follows
    // `outline-offset`; kept as upstream has it.
    #[allow(dead_code)]
    pub fn outline(&self) -> BezPath {
        let mut path = BezPath::new();

        // TODO: this has been known to produce quirky outputs with hugely rounded edges
        self.shape(&mut path, CssBoxKind::OutlineBox, Direction::Clockwise);
        path.move_to(self.corner(Corner::TopLeft, CssBoxKind::BorderBox));

        self.shape(&mut path, CssBoxKind::BorderBox, Direction::Anticlockwise);
        path.move_to(self.corner(Corner::TopLeft, CssBoxKind::BorderBox));

        path
    }

    /// Construct a bezpath of the whole border area: the border box less the
    /// padding box
    pub fn border_ring(&self) -> BezPath {
        // pyro: with shaped corners, the border edge's curves and the padding edge (inside it)
        // wound the other way, so that the ring fills by the nonzero rule.
        if let (Some(outer), Some(inner)) = (
            self.contour(CssBoxKind::BorderBox),
            self.contour(CssBoxKind::PaddingBox),
        ) {
            let mut path = outer.path();
            let inner = inner.path();
            if (path.area() > 0.0) == (inner.area() > 0.0) {
                path.extend(&inner.reverse_subpaths());
            } else {
                path.extend(&inner);
            }
            return path;
        }
        // Two closed contours, each starting on its own first corner (a
        // contour started on the box's corner point would cut that corner off
        // a rounded box).
        let mut path = BezPath::new();
        self.shape(&mut path, CssBoxKind::BorderBox, Direction::Clockwise);
        path.close_path();

        let mut hole = BezPath::new();
        self.shape(&mut hole, CssBoxKind::PaddingBox, Direction::Anticlockwise);
        hole.close_path();
        path.extend(&hole);

        path
    }

    /// Construct a bezpath drawing the frame border
    pub fn border_box_path(&self) -> BezPath {
        if let Some(contour) = self.contour(CssBoxKind::BorderBox) {
            return contour.path();
        }
        let mut path = BezPath::new();
        self.shape(&mut path, CssBoxKind::BorderBox, Direction::Clockwise);
        path
    }

    /// Construct a bezpath drawing the frame padding
    pub fn padding_box_path(&self) -> BezPath {
        if let Some(contour) = self.contour(CssBoxKind::PaddingBox) {
            return contour.path();
        }
        let mut path = BezPath::new();
        self.shape(&mut path, CssBoxKind::PaddingBox, Direction::Clockwise);
        path
    }

    /// Construct a bezpath drawing the frame content
    pub fn content_box_path(&self) -> BezPath {
        if let Some(contour) = self.contour(CssBoxKind::ContentBox) {
            return contour.path();
        }
        let mut path = BezPath::new();
        self.shape(&mut path, CssBoxKind::ContentBox, Direction::Clockwise);
        path
    }

    /// Construct a bezpath for clipping the contents of a text input: the content box
    /// horizontally (so scrolled text doesn't paint over the inline padding) and the
    /// padding box vertically (so glyphs overflowing a short line box aren't cut off).
    pub fn text_input_clip_path(&self) -> BezPath {
        Rect::new(
            self.content_box.x0,
            self.padding_box.y0,
            self.content_box.x1,
            self.padding_box.y1,
        )
        .to_path(0.1)
    }

    /// Whether the border box is a full ellipse (which includes a circle): every
    /// corner radius equals half the box in that axis, i.e. `border-radius: 50%`.
    /// Such a border is a single continuous curve with no straight edges; it is best
    /// drawn as a stroked ellipse rather than a filled two-contour annulus
    /// (see `draw_border`), which otherwise leaves seam notches on a thin ring where
    /// the quarter-arcs meet. Circles are already covered by the stroked-rounded-rect
    /// path (see [`Self::is_uniform_corner_border`]); this catches the true ellipses
    /// (unequal axes) that a rounded rect can't represent.
    pub fn is_elliptical_border(&self) -> bool {
        if self.corner_curvature.is_some() {
            return false;
        }
        let (rx, ry) = (
            self.border_box.width() / 2.0,
            self.border_box.height() / 2.0,
        );
        let is_half = |c: Vec2| (c.x - rx).abs() < 0.01 && (c.y - ry).abs() < 0.01;
        let radii = &self.border_radii;
        is_half(radii.top_left)
            && is_half(radii.top_right)
            && is_half(radii.bottom_right)
            && is_half(radii.bottom_left)
    }

    /// Whether every corner has an equal x and y radius, i.e. each corner is a
    /// circular (not elliptical) arc. Corners may still differ from one another, so
    /// this also covers plain rectangles (radius 0) and ordinary rounded rectangles,
    /// not just circles. Such a border can be drawn as a single stroked
    /// `kurbo::RoundedRect` rather than the filled per-edge annulus in
    /// `draw_border` — simpler and faster, since it avoids building and filling a
    /// separate path per edge. (`draw_border` additionally requires uniform border
    /// width/color and each radius to be 0 or ≥ the border width for the stroke to
    /// reproduce the CSS shape exactly.)
    pub fn is_uniform_corner_border(&self) -> bool {
        if self.corner_curvature.is_some() {
            return false;
        }
        let is_circular = |c: Vec2| (c.x - c.y).abs() < 0.01;
        let radii = &self.border_radii;
        is_circular(radii.top_left)
            && is_circular(radii.top_right)
            && is_circular(radii.bottom_right)
            && is_circular(radii.bottom_left)
    }

    fn shape(&self, path: &mut BezPath, line: CssBoxKind, direction: Direction) {
        use Corner::*;

        let route = match direction {
            Direction::Clockwise => [TopLeft, TopRight, BottomRight, BottomLeft],
            Direction::Anticlockwise => [TopLeft, BottomLeft, BottomRight, TopRight],
        };

        for corner in route {
            if self.is_sharp(corner, line) {
                path.insert_point(self.corner(corner, line));
            } else {
                path.insert_arc(self.corner_arc(corner, line, direction));
            }
        }
    }

    /// Construct a bezpath drawing the frame
    pub fn shadow_clip(&self, shadow_rect: Rect) -> BezPath {
        if let Some(contour) = self.contour(CssBoxKind::BorderBox) {
            let shapes = [
                vec![contoured::rect_polygon(shadow_rect)],
                contoured::flatten(&contour.path()),
            ];
            return contoured::boolean(&shapes, |inside| inside[0] && !inside[1]);
        }
        let mut path = BezPath::new();
        self.shadow_clip_shape(&mut path, shadow_rect);
        path
    }

    fn shadow_clip_shape(&self, path: &mut BezPath, shadow_rect: Rect) {
        use Corner::*;

        for corner in [TopLeft, TopRight, BottomRight, BottomLeft] {
            path.insert_point(self.shadow_clip_corner(corner, shadow_rect));
        }

        if self.is_sharp(TopLeft, CssBoxKind::BorderBox) {
            path.move_to(self.corner(TopLeft, CssBoxKind::BorderBox));
        } else {
            const TOLERANCE: f64 = 0.1;
            let arc = self.corner_arc(TopLeft, CssBoxKind::BorderBox, Direction::Anticlockwise);
            let elements = arc.path_elements(TOLERANCE);
            path.extend(elements);
        }

        for corner in [/*TopLeft, */ BottomLeft, BottomRight, TopRight] {
            if self.is_sharp(corner, CssBoxKind::BorderBox) {
                path.insert_point(self.corner(corner, CssBoxKind::BorderBox));
            } else {
                path.insert_arc(self.corner_arc(
                    corner,
                    CssBoxKind::BorderBox,
                    Direction::Anticlockwise,
                ));
            }
        }
    }

    fn corner(&self, corner: Corner, css_box: CssBoxKind) -> Point {
        let Rect { x0, y0, x1, y1 } = match css_box {
            CssBoxKind::OutlineBox => self.outline_box,
            CssBoxKind::BorderBox => self.border_box,
            CssBoxKind::PaddingBox => self.padding_box,
            CssBoxKind::ContentBox => self.content_box,
        };
        match corner {
            Corner::TopLeft => Point { x: x0, y: y0 },
            Corner::TopRight => Point { x: x1, y: y0 },
            Corner::BottomLeft => Point { x: x0, y: y1 },
            Corner::BottomRight => Point { x: x1, y: y1 },
        }
    }

    fn shadow_clip_corner(&self, corner: Corner, shadow_rect: Rect) -> Point {
        let (x, y) = match corner {
            Corner::TopLeft => (shadow_rect.x0, shadow_rect.y0),
            Corner::TopRight => (shadow_rect.x1, shadow_rect.y0),
            Corner::BottomRight => (shadow_rect.x1, shadow_rect.y1),
            Corner::BottomLeft => (shadow_rect.x0, shadow_rect.y1),
        };

        Point { x, y }
    }

    /// Check if the corner width is smaller than the radius.
    /// If it is, we need to fill in the gap with an arc
    fn corner_needs_infill(&self, corner: Corner) -> bool {
        match corner {
            Corner::TopLeft => {
                self.border_radii.top_left.x > self.border_width.x0
                    && self.border_radii.top_left.y > self.border_width.y0
            }
            Corner::TopRight => {
                self.border_radii.top_right.x > self.border_width.x1
                    && self.border_radii.top_right.y > self.border_width.y0
            }
            Corner::BottomRight => {
                self.border_radii.bottom_right.x > self.border_width.x1
                    && self.border_radii.bottom_right.y > self.border_width.y1
            }
            Corner::BottomLeft => {
                self.border_radii.bottom_left.x > self.border_width.x0
                    && self.border_radii.bottom_left.y > self.border_width.y1
            }
        }
    }

    /// Get the complete arc for a corner, skipping the need for splitting the arc into pieces
    fn corner_arc(&self, corner: Corner, css_box: CssBoxKind, direction: Direction) -> Arc {
        let (center, radii) = self.ellipse(corner, css_box);

        // Sweep clockwise for outer arcs, counter clockwise for inner arcs
        let sweep_direction = match direction {
            Direction::Anticlockwise => -1.0,
            Direction::Clockwise => 1.0,
        };

        let offset = match corner {
            Corner::TopLeft => -FRAC_PI_2,
            Corner::TopRight => 0.0,
            Corner::BottomRight => FRAC_PI_2,
            Corner::BottomLeft => PI,
        };

        let offset = match direction {
            Direction::Clockwise => offset,
            Direction::Anticlockwise => offset + FRAC_PI_2,
        };

        Arc::new(
            center,
            radii,
            // Note that we apply a fixed offset to get us in the unit circle coordinate system
            // vello chooses the x axis as the start of the arc, so we need to offset by 3pi/2
            offset + PI + FRAC_PI_2,
            FRAC_PI_2 * sweep_direction,
            0.0,
        )
    }

    /// Get the arc for a half of a corner.
    /// This handles the case where adjacent border sides have different colors and thus
    /// the corner between the side changes color in the middle. We draw these as separate shapes
    /// and thus need to to get the arc up to the "middle point".
    ///
    /// The angle at which the color changes depends on the ratio of the border widths and radii of the corner
    fn partial_corner_arc(
        &self,
        corner: Corner,
        css_box: CssBoxKind,
        edge: Edge,
        direction: Direction,
    ) -> Arc {
        use Corner::*;
        use CssBoxKind::*;
        use Edge::*;

        let (center, radii) = self.ellipse(corner, css_box);

        // We solve a tiny system of equations to find the start angle
        // This is fixed to a single coordinate system, so we need to adjust the start angle
        let theta = self.start_angle(corner, radii);

        // Sweep clockwise for outer arcs, counter clockwise for inner arcs
        let sweep_direction = match direction {
            Direction::Anticlockwise => -1.0,
            Direction::Clockwise => 1.0,
        };

        // Easier to reason about if we think about just offsetting the turns from the start
        let offset = match edge {
            Top => 0.0,
            Right => FRAC_PI_2,
            Bottom => PI,
            Left => PI + FRAC_PI_2,
        };

        // On left/right gets theta, on top/bottom gets pi/2 - theta
        let theta = match edge {
            Top | Bottom => FRAC_PI_2 - theta,
            Right | Left => theta,
        };

        // Depededning on the edge, we need to adjust the start angle
        // We still sweep the same, but the theta split is different since we're cutting in half
        // I imagine you could mnake this simpler using a bit more math
        let start = match (edge, corner, css_box) {
            // Top Edge
            (Top, TopLeft, PaddingBox) => 0.0,
            (Top, TopLeft, BorderBox) => -theta,
            (Top, TopRight, BorderBox) => 0.0,
            (Top, TopRight, PaddingBox) => theta,

            // Right Edge
            (Right, TopRight, PaddingBox) => 0.0,
            (Right, TopRight, BorderBox) => -theta,
            (Right, BottomRight, BorderBox) => 0.0,
            (Right, BottomRight, PaddingBox) => theta,

            // Bottom Edge
            (Bottom, BottomRight, PaddingBox) => 0.0,
            (Bottom, BottomRight, BorderBox) => -theta,
            (Bottom, BottomLeft, BorderBox) => 0.0,
            (Bottom, BottomLeft, PaddingBox) => theta,

            // Left Edge
            (Left, BottomLeft, PaddingBox) => 0.0,
            (Left, BottomLeft, BorderBox) => -theta,
            (Left, TopLeft, BorderBox) => 0.0,
            (Left, TopLeft, PaddingBox) => theta,

            _ => unreachable!("Invalid edge/corner combination"),
        };

        Arc::new(
            center,
            radii,
            // Note that we apply a fixed offset to get us in the unit circle coordinate system
            // vello chooses the x axis as the start of the arc, so we need to offset by 3pi/2
            start + offset + PI + FRAC_PI_2,
            theta * sweep_direction,
            0.0,
        )
    }

    /// Check if a corner is sharp (IE the absolute radius is 0)
    fn is_sharp(&self, corner: Corner, side: CssBoxKind) -> bool {
        use Corner::*;
        use CssBoxKind::*;

        let corner_radii = match corner {
            TopLeft => self.border_radii.top_left,
            TopRight => self.border_radii.top_right,
            BottomLeft => self.border_radii.bottom_left,
            BottomRight => self.border_radii.bottom_right,
        };
        let is_sharp = (corner_radii.x == 0.0) | (corner_radii.y == 0.0);
        if is_sharp {
            return true;
        }

        let css_box: Insets = match side {
            OutlineBox => return false,
            BorderBox => return false,
            PaddingBox => self.border_width,
            ContentBox => add_insets(self.border_width, self.padding_width),
        };
        match corner {
            TopLeft => (corner_radii.x <= css_box.x0) | (corner_radii.y <= css_box.y0),
            TopRight => (corner_radii.x <= css_box.x1) | (corner_radii.y <= css_box.y0),
            BottomLeft => (corner_radii.x <= css_box.x0) | (corner_radii.y <= css_box.y1),
            BottomRight => (corner_radii.x <= css_box.x1) | (corner_radii.y <= css_box.y1),
        }
    }

    /// The `(center, radii)` of the ellipse that a given corner traces along the
    /// given box edge.
    ///
    /// The radii are returned as an axis-aligned `(x, y)` pair, matching the CSS
    /// `border-radius` horizontal/vertical radii. We deliberately do *not* return
    /// a [`kurbo::Ellipse`]: `Ellipse::radii()` canonicalises the ellipse via an
    /// SVD, which swaps the axes and introduces a `π/2` rotation whenever the
    /// vertical radius exceeds the horizontal one (`ry > rx`). Callers here build
    /// [`kurbo::Arc`]s with a fixed `x_rotation` of `0`, so that swap would draw
    /// the corner with its axes transposed. Returning the raw radii avoids the
    /// round-trip entirely.
    fn ellipse(&self, corner: Corner, side: CssBoxKind) -> (Point, Vec2) {
        use {Corner::*, CssBoxKind::*};
        let CssBox {
            border_box,
            padding_width,
            border_width,
            border_radii,
            ..
        } = self;

        let corner_radii = match corner {
            TopLeft => border_radii.top_left,
            TopRight => border_radii.top_right,
            BottomLeft => border_radii.bottom_left,
            BottomRight => border_radii.bottom_right,
        };

        let center = match corner {
            TopLeft => corner_radii,
            TopRight => Vec2 {
                x: border_box.width() - corner_radii.x,
                y: corner_radii.y,
            },
            BottomLeft => Vec2 {
                x: corner_radii.x,
                y: border_box.height() - corner_radii.y,
            },
            BottomRight => Vec2 {
                x: border_box.width() - corner_radii.x,
                y: border_box.height() - corner_radii.y,
            },
        };

        let radii: Vec2 = match side {
            BorderBox => corner_radii,
            OutlineBox => corner_radii + Vec2::new(self.outline_width, self.outline_width),
            PaddingBox => corner_radii - get_corner_insets(*border_width, corner),
            ContentBox => {
                corner_radii - get_corner_insets(add_insets(*border_width, *padding_width), corner)
            }
        };

        // pyro: inner radii that do not fit their box are scaled to fit it; their centre is then
        // no longer the border's (in the inner box's corner, `radii` in from it).
        let (inner_box, scale) = match side {
            PaddingBox => (self.padding_box, self.padding_radii_scale),
            ContentBox => (self.content_box, self.content_radii_scale),
            BorderBox | OutlineBox => (*border_box, 1.0),
        };
        if scale < 1.0 && radii.x > 0.0 && radii.y > 0.0 {
            let radii = radii * scale;
            let center = match corner {
                TopLeft => Point::new(inner_box.x0 + radii.x, inner_box.y0 + radii.y),
                TopRight => Point::new(inner_box.x1 - radii.x, inner_box.y0 + radii.y),
                BottomLeft => Point::new(inner_box.x0 + radii.x, inner_box.y1 - radii.y),
                BottomRight => Point::new(inner_box.x1 - radii.x, inner_box.y1 - radii.y),
            };
            return (center, radii);
        }

        (border_box.origin() + center, radii)
    }

    fn start_angle(&self, corner: Corner, radii: Vec2) -> f64 {
        let corner_insets = get_corner_insets(self.border_width, corner);
        start_angle(corner_insets.y, corner_insets.x, radii)
    }
}

/// pyro: the part of a box (`rect`, its border `widths`) the border's side `edge` paints at
/// shaped corners: the points nearer that side than any other, measuring each side's distance
/// in its own width, as the miter from each outer corner to the inner one divides a border. A
/// side without width paints nothing; the others then share its corners.
fn side_region(rect: Rect, widths: Insets, edge: Edge) -> Vec<Point> {
    let width = |e: Edge| match e {
        Edge::Top => widths.y0,
        Edge::Right => widths.x1,
        Edge::Bottom => widths.y1,
        Edge::Left => widths.x0,
    };
    // A side's distance from a point as `a·x + b·y + c`, over its width.
    let distance = |e: Edge| -> (f64, f64, f64) {
        match e {
            Edge::Top => (0.0, 1.0, -rect.y0),
            Edge::Right => (-1.0, 0.0, rect.x1),
            Edge::Bottom => (0.0, -1.0, rect.y1),
            Edge::Left => (1.0, 0.0, -rect.x0),
        }
    };
    if width(edge) <= 0.0 {
        return Vec::new();
    }
    // (A little more than the box: curves may overshoot its edge by a hair.)
    let mut polygon = contoured::rect_polygon(rect.inflate(1.0, 1.0));
    let (a, b, c) = distance(edge);
    let w = width(edge);
    for other in [Edge::Top, Edge::Right, Edge::Bottom, Edge::Left] {
        if other == edge || width(other) <= 0.0 {
            continue;
        }
        let (oa, ob, oc) = distance(other);
        let ow = width(other);
        // Keep d/w <= od/ow: (a·ow - oa·w)·x + (b·ow - ob·w)·y + (c·ow - oc·w) <= 0.
        let (ka, kb, kc) = (a * ow - oa * w, b * ow - ob * w, c * ow - oc * w);
        polygon = clip_half_plane(&polygon, |p| ka * p.x + kb * p.y + kc);
    }
    polygon
}

/// The part of the convex `polygon` where `f` is not positive (Sutherland–Hodgman).
fn clip_half_plane(polygon: &[Point], f: impl Fn(Point) -> f64) -> Vec<Point> {
    let mut out = Vec::with_capacity(polygon.len() + 1);
    let n = polygon.len();
    for i in 0..n {
        let (p, q) = (polygon[i], polygon[(i + 1) % n]);
        let (fp, fq) = (f(p), f(q));
        if fp <= 0.0 {
            out.push(p);
        }
        if (fp <= 0.0) != (fq <= 0.0) {
            let t = fp / (fp - fq);
            out.push(p + (q - p) * t);
        }
    }
    out
}

/// Makes it easier to insert objects into a bezpath without having to do checks
/// Mostly because I consider the vello api slightly defficient
trait BuildBezpath {
    const TOLERANCE: f64;
    fn insert_arc(&mut self, arc: Arc);
    fn insert_point(&mut self, point: Point);
}

impl BuildBezpath for BezPath {
    /// Vello uses an inner tolerance for creating segments
    /// We're just reusing the value here
    const TOLERANCE: f64 = 0.1;

    fn insert_arc(&mut self, arc: Arc) {
        let mut elements = arc.path_elements(Self::TOLERANCE);
        match elements.next().unwrap() {
            PathEl::MoveTo(a) if !self.elements().is_empty() => self.push(PathEl::LineTo(a)),
            el => self.push(el),
        }
        self.extend(elements)
    }

    fn insert_point(&mut self, point: Point) {
        if self.elements().is_empty() {
            self.push(PathEl::MoveTo(point));
        } else {
            self.push(PathEl::LineTo(point));
        }
    }
}

/// Get the start angle of the arc based on the border width and the radii
fn start_angle(bt_width: f64, br_width: f64, radii: Vec2) -> f64 {
    // slope of the border intersection split
    let w = bt_width / br_width;
    let x = radii.y / (w * radii.x);

    /*
    Any point on an ellipse is given by:
    x = a cos(t)
    y = b sin(t)

    The equation of the border intersect is:
    y = w (x - a) + b

    where w is the ratio of the width to the height of the border
    and b is the y intercept of the border
    and x is the x intercept of the border

    This formula is the result of solving the system of equations:
    x = a cos(t)
    y = b sin(t)
    y = w (x - a) + b

    b/(w*a) = (cos(t) - 1)/(sin(t) - 1)

    Substituting s = tan(t/2) turns this into the quadratic

        (k - 2) s² - 2k s + k = 0     where k = b/(w*a) = x

    whose relevant root can be written (after rationalising to remove the
    catastrophic cancellation / removable singularity the naive quadratic
    formula has at k == 2) as:

        s = √k / (√k + √2)

    This form is well behaved for all k >= 0 (in particular around k == 2,
    which occurs for perfectly ordinary elliptical corners, e.g. a 80px/30px
    radius with 40px/10px border widths), always yielding t in [0, π/2).
    */

    use std::f64::consts::SQRT_2;
    let sqrt_x = x.sqrt();
    let s = sqrt_x / (sqrt_x + SQRT_2);
    s.atan() * 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `start_angle` must return the angle `t` at which the border colour split
    /// line crosses the corner ellipse, i.e. the solution of
    /// `(cos t - 1) / (sin t - 1) == k` where `k = radii.y / (w * radii.x)`.
    fn assert_solves(bt: f64, br: f64, radii: Vec2) {
        let t = start_angle(bt, br, radii);
        assert!(t.is_finite(), "start_angle returned {t} for {radii:?}");
        assert!(
            (0.0..=std::f64::consts::FRAC_PI_2).contains(&t),
            "t={t} out of range"
        );
        let w = bt / br;
        let k = radii.y / (w * radii.x);
        let lhs = (t.cos() - 1.0) / (t.sin() - 1.0);
        assert!(
            (lhs - k).abs() < 1e-9,
            "t={t} does not solve k={k} (got {lhs})"
        );
    }

    /// Regression test for elliptical corners where the vertical radius exceeds
    /// the horizontal one (`ry > rx`). `kurbo::Ellipse::radii()` canonicalises
    /// such an ellipse by swapping its axes and adding a `π/2` rotation; when
    /// that rotation was dropped the corner arcs were drawn transposed, skewing
    /// the whole box. The straight portions of each edge must stay axis aligned:
    /// the top/bottom edges horizontal and the left/right edges vertical.
    #[test]
    fn edges_stay_axis_aligned_for_tall_corners() {
        let b = CssBox::new(
            Rect::new(0.0, 0.0, 400.0, 200.0),
            Insets::uniform(10.0),
            Insets::ZERO,
            0.0,
            NonUniformRoundedRectRadii {
                top_left: Vec2::new(60.0, 20.0),
                top_right: Vec2::new(20.0, 50.0), // ry > rx
                bottom_right: Vec2::new(50.0, 10.0),
                bottom_left: Vec2::new(30.0, 40.0), // ry > rx
            },
        );

        // The outer border box corner y (top) / x (right) etc. that the straight
        // part of each edge should run along.
        let checks = [
            (Edge::Top, 0.0),      // outer top edge at y == 0
            (Edge::Bottom, 200.0), // outer bottom edge at y == 200
            (Edge::Left, 0.0),     // outer left edge at x == 0
            (Edge::Right, 400.0),  // outer right edge at x == 400
        ];
        // Collect every point (endpoints and Bézier control points) of a path.
        // A cubic Bézier lies within the convex hull of its control points, so
        // checking these is enough to prove the whole path stays in the box.
        let points = |path: &BezPath| -> Vec<Point> {
            path.elements()
                .iter()
                .flat_map(|el| match *el {
                    PathEl::MoveTo(p) | PathEl::LineTo(p) => vec![p],
                    PathEl::QuadTo(a, b) => vec![a, b],
                    PathEl::CurveTo(a, b, c) => vec![a, b, c],
                    PathEl::ClosePath => vec![],
                })
                .collect()
        };

        for (edge, expected) in checks {
            let path = b.border_edge_shape(edge);
            let pts = points(&path);
            // The transposed-axis bug pushed points well outside the border box.
            for p in &pts {
                assert!(
                    (-0.01..=400.01).contains(&p.x) && (-0.01..=200.01).contains(&p.y),
                    "{edge:?}: point {p:?} escaped the border box"
                );
            }
            // And the outer straight run must actually reach the box edge.
            let reaches = pts.iter().any(|p| match edge {
                Edge::Top | Edge::Bottom => (p.y - expected).abs() < 0.01,
                Edge::Left | Edge::Right => (p.x - expected).abs() < 0.01,
            });
            assert!(
                reaches,
                "{edge:?}: no point reached the outer edge {expected}"
            );
        }
    }

    #[test]
    fn should_solve_properly() {
        // 0.643501
        assert!((start_angle(4.0, 1.0, Vec2 { x: 1.0, y: 2.0 }) - 0.643501).abs() < 1e-5);
    }

    /// Regression test: when `k == radii.y / (w * radii.x)` is exactly 2 the
    /// old closed form evaluated `0 / 0` and produced `NaN`, corrupting the
    /// corner arc. This happens for ordinary elliptical corners such as an
    /// 80px/30px radius with 40px/10px border widths (inner/padding ellipse
    /// radii 40/20, widths 30/0 ... => k == 2).
    #[test]
    fn handles_k_equal_two() {
        // k = radii.y / (w * radii.x) = 40 / ((10/40) * 80) = 2.0
        assert_solves(10.0, 40.0, Vec2 { x: 80.0, y: 40.0 });
    }

    /// pyro: a round corner whose padding radii overflow the padding box (200x100, border 20,
    /// `border-top-left-radius: 100%`: 180x80 in 160x60) keeps its inner curve parallel to the
    /// border's, as Blink's `ContouredInnerBorder` (WPT css-borders `render-corner-shape.html?
    /// corner-shape=1&border-top-left-radius=100%&border-width=20`), not radii scaled to fit,
    /// which reached 40px further in at y = 28.
    #[test]
    fn overflowing_round_padding_radii_stay_parallel() {
        let radii = NonUniformRoundedRectRadii {
            top_left: Vec2::new(200.0, 100.0),
            top_right: Vec2::ZERO,
            bottom_right: Vec2::ZERO,
            bottom_left: Vec2::ZERO,
        };
        let b = CssBox::new(Rect::new(0.0, 0.0, 200.0, 100.0), Insets::uniform(20.0), Insets::ZERO, 0.0, radii)
            .with_corner_curvature(CornerCurvature::ROUND);
        assert!(b.corner_curvature.is_some());
        let padding = b.padding_box_path();
        assert!(!padding.contains(Point::new(100.0, 28.0)));
        assert!(padding.contains(Point::new(150.0, 28.0)));
        // The bleed-avoiding inset (half the border) is a contour too.
        assert!(b.inset_contour(Insets::uniform(10.0)).is_some());
        // Round radii that fit their boxes stay a plain rounded box.
        let fits = NonUniformRoundedRectRadii { top_left: Vec2::new(40.0, 40.0), ..radii };
        let b = CssBox::new(Rect::new(0.0, 0.0, 200.0, 100.0), Insets::uniform(20.0), Insets::ZERO, 0.0, fits)
            .with_corner_curvature(CornerCurvature::ROUND);
        assert!(b.corner_curvature.is_none());
    }

    #[test]
    fn solves_a_range_of_elliptical_corners() {
        for &(bt, br) in &[(1.0, 1.0), (1.0, 4.0), (4.0, 1.0), (3.0, 7.0)] {
            for &(rx, ry) in &[(80.0, 30.0), (30.0, 80.0), (60.0, 60.0), (120.0, 20.0)] {
                assert_solves(bt, br, Vec2 { x: rx, y: ry });
            }
        }
    }
}

#[test]
fn detects_elliptical_border() {
    let corners = |x: f64, y: f64| NonUniformRoundedRectRadii {
        top_left: Vec2::new(x, y),
        top_right: Vec2::new(x, y),
        bottom_right: Vec2::new(x, y),
        bottom_left: Vec2::new(x, y),
    };
    let css_box = |w: f64, h: f64, radii: NonUniformRoundedRectRadii| {
        CssBox::new(
            Rect::new(0.0, 0.0, w, h),
            Insets::uniform(1.0),
            Insets::ZERO,
            0.0,
            radii,
        )
    };

    // Circle: square box, every radius == half the side.
    assert!(css_box(44.0, 44.0, corners(22.0, 22.0)).is_elliptical_border());
    // Ellipse: non-square box, radii == half each axis.
    assert!(css_box(120.0, 64.0, corners(60.0, 32.0)).is_elliptical_border());
    // Rounded rectangle: radius smaller than half → has straight edges.
    assert!(!css_box(44.0, 44.0, corners(10.0, 10.0)).is_elliptical_border());
    // Sharp rectangle: no rounding.
    assert!(!css_box(44.0, 44.0, corners(0.0, 0.0)).is_elliptical_border());
}

#[test]
fn detects_uniform_corner_border() {
    let corners = |x: f64, y: f64| NonUniformRoundedRectRadii {
        top_left: Vec2::new(x, y),
        top_right: Vec2::new(x, y),
        bottom_right: Vec2::new(x, y),
        bottom_left: Vec2::new(x, y),
    };
    let css_box = |w: f64, h: f64, radii: NonUniformRoundedRectRadii| {
        CssBox::new(
            Rect::new(0.0, 0.0, w, h),
            Insets::uniform(1.0),
            Insets::ZERO,
            0.0,
            radii,
        )
    };

    // Sharp rectangle: no rounding, still trivially "uniform" (0 == 0).
    assert!(css_box(44.0, 44.0, corners(0.0, 0.0)).is_uniform_corner_border());
    // Ordinary rounded rectangle: each corner is a circular arc.
    assert!(css_box(44.0, 44.0, corners(10.0, 10.0)).is_uniform_corner_border());
    // Circle: square box, every radius == half the side.
    assert!(css_box(44.0, 44.0, corners(22.0, 22.0)).is_uniform_corner_border());
    // Ellipse: non-square box, radii == half each axis → corners aren't circular.
    assert!(!css_box(120.0, 64.0, corners(60.0, 32.0)).is_uniform_corner_border());
    // Per-corner elliptical radius (rx != ry) anywhere disqualifies the box, even
    // if it isn't a full border-radius: 50% ellipse.
    let mixed = NonUniformRoundedRectRadii {
        top_left: Vec2::new(10.0, 5.0),
        ..corners(10.0, 10.0)
    };
    assert!(!css_box(44.0, 44.0, mixed).is_uniform_corner_border());

    // Corners may differ in radius from each other, as long as each is circular.
    let differing = NonUniformRoundedRectRadii {
        top_left: Vec2::new(10.0, 10.0),
        top_right: Vec2::new(5.0, 5.0),
        bottom_right: Vec2::new(8.0, 8.0),
        bottom_left: Vec2::new(2.0, 2.0),
    };
    assert!(css_box(44.0, 44.0, differing).is_uniform_corner_border());
}
