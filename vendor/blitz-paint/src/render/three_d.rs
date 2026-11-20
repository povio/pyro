//! pyro (lane H2): 3D rendering contexts and planes drawn through a perspective.
//!
//! blitz-dom gives each element that paints as a plane of its own its 4x4 matrix
//! (`Node::plane_3d`): the elements of a 3D rendering context that Chromium gives a transform node
//! (a transform, `preserve-3d`, `backface-visibility: hidden`), and a flat element whose 3D
//! transform maps it projectively (a rotation out of the plane under `perspective`).
//!
//! The root of a context (or such a flat element) is painted as Chromium composites it:
//! - Each plane's painting is recorded on its own, as Blitz composes it (each element's 2D
//!   transform is the stand-in blitz-dom flattens its matrix to). Where a plane of the context is
//!   painted inside another, a marker records the place.
//! - The recordings are cut into chunks at the markers, in paint order: what a plane paints before
//!   a child plane, the child (and its own chunks), what the plane paints after it. A plane met
//!   inside a clip or a layer of another stays in it, flattened into that plane (`flattened_into`).
//! - An element of a context that is no plane paints in the plane of the element it is in, at its
//!   layout place there (blitz-dom gives it no transform).
//! - Each chunk is a polygon in 3D (its content's bounds in its plane, through its matrix, cut at
//!   the viewer's plane): the polygons are sorted back to front with a BSP tree, as Chromium's
//!   compositor sorts the quads of a 3D sorting context (viz `BspTree`): a polygon crossing another's
//!   plane is split, coplanar ones keep paint order.
//! - A chunk is drawn where its polygon is: through the affine its matrix flattens to when W does
//!   not vary over the plane (no perspective, or none across the plane), or as a projective layer
//!   (anyrender_vello_cpu `projective`) with the plane's homography. A split piece is clipped to
//!   its polygon.
//!
//! Printed output (`composite_3d` false) keeps the stand-ins and gets the sorting only (the
//! polygons of stand-ins are where the stand-ins put them); a plane crossing the viewer's plane has
//! no stand-in and is left out there.

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use anyrender::filters::FilterEffect;
use anyrender::recording::RenderCommand;
use anyrender::{Filter, PaintScene, Scene};
use blitz_dom::node::Plane3D;
use blitz_dom::{Node, NodeId};
use kurbo::{Affine, BezPath, Point, Rect, Vec2};
use peniko::{BlendMode, Fill};

use super::BlitzDomPainter;

/// The painter's state for 3D rendering contexts.
#[derive(Default)]
pub(crate) struct State {
    /// The contexts being recorded, innermost last.
    contexts: RefCell<Vec<Collector>>,
    /// The element [`BlitzDomPainter::paint_plane`] is recording: its next `render_element` paints it.
    entered: Cell<Option<NodeId>>,
    /// How many plane recordings the element painted is in.
    pub(crate) depth: Cell<u32>,
}

/// A context being recorded.
struct Collector {
    root: NodeId,
    /// The root's untransformed border box's origin in the scene (device px).
    origin: Affine,
    planes: Vec<Option<PlaneRecord>>,
}

/// What a plane painted, and where.
struct PlaneRecord {
    /// The transform Blitz painted the element with (its stand-in composed down the tree).
    recorded: Affine,
    /// From the element's own device px to the scene's, with depth (Z) and perspective (W).
    matrix: M4,
    own_hidden: bool,
    /// It crosses the viewer's plane (`Plane3D::crosses_viewer`): it has no stand-in.
    crosses_viewer: bool,
    commands: Vec<RenderCommand>,
}

/// A 4x4 matrix in euclid's row-vector convention: `(x, y, z, 1) * m`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct M4(pub(crate) [[f64; 4]; 4]);

impl M4 {
    fn of(plane: &Plane3D) -> Self {
        let m = &plane.matrix;
        Self([
            [m.m11, m.m12, m.m13, m.m14],
            [m.m21, m.m22, m.m23, m.m24],
            [m.m31, m.m32, m.m33, m.m34],
            [m.m41, m.m42, m.m43, m.m44],
        ])
    }

    fn scale(s: f64) -> Self {
        Self([[s, 0.0, 0.0, 0.0], [0.0, s, 0.0, 0.0], [0.0, 0.0, s, 0.0], [0.0, 0.0, 0.0, 1.0]])
    }

    fn affine(a: Affine) -> Self {
        let [a, b, c, d, e, f] = a.as_coeffs();
        Self([[a, b, 0.0, 0.0], [c, d, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [e, f, 0.0, 1.0]])
    }

    /// `self` applied first, then `next`.
    fn then(&self, next: &M4) -> M4 {
        let mut out = [[0.0; 4]; 4];
        for (r, row) in out.iter_mut().enumerate() {
            for (c, v) in row.iter_mut().enumerate() {
                *v = (0..4).map(|k| self.0[r][k] * next.0[k][c]).sum();
            }
        }
        M4(out)
    }

    /// `(X, Y, Z, W)` of the plane point (x, y).
    fn apply(&self, x: f64, y: f64) -> [f64; 4] {
        let m = &self.0;
        [
            x * m[0][0] + y * m[1][0] + m[3][0],
            x * m[0][1] + y * m[1][1] + m[3][1],
            x * m[0][2] + y * m[1][2] + m[3][2],
            x * m[0][3] + y * m[1][3] + m[3][3],
        ]
    }

    /// The device matrix of a plane: from the element's own device px (`scale` per CSS px) to the
    /// scene, its matrix relative to the root's border box at `origin`.
    fn device(plane: &Plane3D, scale: f64, origin: Affine) -> M4 {
        M4::scale(1.0 / scale).then(&M4::of(plane)).then(&M4::scale(scale)).then(&M4::affine(origin))
    }

    /// The affine a plane with content in `bounds` is drawn with, when W does not vary over them
    /// (`None` when it does: the plane needs a projective draw). `Some(None)`: the plane lies
    /// behind the viewer.
    fn flat(&self, bounds: Rect) -> Option<Option<Affine>> {
        let m = &self.0;
        let reach =
            m[0][3].abs() * bounds.x0.abs().max(bounds.x1.abs()) + m[1][3].abs() * bounds.y0.abs().max(bounds.y1.abs());
        if reach > 1e-9 * m[3][3].abs() {
            return None;
        }
        if !(m[3][3] > 0.0) {
            return Some(None);
        }
        let k = 1.0 / m[3][3];
        Some(Some(Affine::new([m[0][0] * k, m[0][1] * k, m[1][0] * k, m[1][1] * k, m[3][0] * k, m[3][1] * k])))
    }

    /// The homography of the plane (row-major, column vectors): `(X, Y, W)` of `(x, y, 1)`.
    fn homography(&self) -> [f64; 9] {
        let m = &self.0;
        [m[0][0], m[1][0], m[3][0], m[0][1], m[1][1], m[3][1], m[0][3], m[1][3], m[3][3]]
    }
}

use blitz_dom::projection::{clip_w, project, MIN_W, PROJECT_LIMIT};

/// The clip rect a plane is recorded with: Blitz culls nothing in a plane (its stand-in can lie
/// elsewhere than the plane does).
const UNCULLED: Rect = Rect::new(-1e7, -1e7, 1e7, 1e7);

/// The filter of the marker a plane leaves in the recording of the plane it is painted in: an
/// offset by NaN, its index in the context. Never drawn: [`expand`] replaces each.
fn plane_marker(index: usize) -> Arc<Filter> {
    Arc::new(Filter::single(FilterEffect::Offset(Vec2::new(f64::NAN, index as f64))))
}

fn marker_index(command: &RenderCommand) -> Option<usize> {
    let RenderCommand::PushLayer(layer) = command else { return None };
    match layer.filter.as_deref()?.nodes() {
        [node] => match node.effect {
            FilterEffect::Offset(v) if v.x.is_nan() => Some(v.y as usize),
            _ => None,
        },
        _ => None,
    }
}

/// The filter that marks a projective layer with the homography `h` (row-major, from the layer's
/// content to its space), as anyrender_vello_cpu's `projective::homography` reads it: six offsets,
/// the first by negative zero, then `(h11, h21), (h12, h22), (h13, h23), (h31, h32), (h33, -0.0)`.
pub(crate) fn projective_marker(h: &[f64; 9]) -> Arc<Filter> {
    let v = |x: f64, y: f64| FilterEffect::Offset(Vec2::new(x, y));
    Arc::new(Filter::linear_list(
        [v(-0.0, -0.0), v(h[0], h[3]), v(h[1], h[4]), v(h[2], h[5]), v(h[6], h[7]), v(h[8], -0.0)].into_iter(),
    ))
}

/// Whether `filter` marks a projective layer ([`projective_marker`]).
pub(crate) fn is_projective_marker(filter: &Filter) -> bool {
    let nodes = filter.nodes();
    nodes.len() == 6
        && matches!(nodes[0].effect, FilterEffect::Offset(v)
            if v.x == 0.0 && v.y == 0.0 && v.x.is_sign_negative() && v.y.is_sign_negative())
        && matches!(nodes[5].effect, FilterEffect::Offset(v) if v.y == 0.0 && v.y.is_sign_negative())
}

impl BlitzDomPainter<'_, '_> {
    /// Paints `node` here if it is a plane ([`Plane3D`]) of a context being recorded (recorded on
    /// its own, a marker left in `scene`), or the root of a context or a flat projective plane
    /// (recorded with its context's planes, then drawn sorted). Returns whether it did.
    pub(super) fn paint_plane(&self, scene: &mut impl PaintScene, node: &Node, parent_transform: Affine) -> bool {
        let Some(plane) = node.plane_3d() else { return false };
        if !plane.matrix.to_array().iter().all(|v| v.is_finite()) {
            return true;
        }
        if self.three_d.entered.get() == Some(node.id) {
            self.three_d.entered.set(None);
            return false;
        }
        let collecting = self.three_d.contexts.borrow().iter().rposition(|c| c.root == plane.root);
        if let Some(at) = collecting {
            let index = self.record_plane(node, plane, parent_transform, at);
            scene.push_layer(
                Fill::NonZero,
                BlendMode::default(),
                1.0,
                Affine::IDENTITY,
                &Rect::ZERO,
                Some(plane_marker(index)),
                None,
            );
            scene.pop_layer();
            return true;
        }
        if plane.root != node.id {
            // Its context's root is not painted as one (it is in a plane flattened into another):
            // the stand-in.
            return false;
        }
        let location = node.final_layout().location;
        let origin =
            parent_transform * Affine::translate((location.x as f64 * self.scale, location.y as f64 * self.scale));
        self.three_d.contexts.borrow_mut().push(Collector { root: node.id, origin, planes: Vec::new() });
        let at = self.three_d.contexts.borrow().len() - 1;
        let top = self.record_plane(node, plane, parent_transform, at);
        let collector = self.three_d.contexts.borrow_mut().pop().expect("the context pushed above");
        self.compose(scene, collector, top);
        true
    }

    /// Records `node`'s painting into the context at `at`, returning its plane's index there.
    fn record_plane(&self, node: &Node, plane: &Plane3D, parent_transform: Affine, at: usize) -> usize {
        let mut recording = Scene::new();
        self.three_d.entered.set(Some(node.id));
        self.three_d.depth.set(self.three_d.depth.get() + 1);
        self.render_element(&mut recording, node.id, parent_transform, UNCULLED);
        self.three_d.depth.set(self.three_d.depth.get() - 1);
        self.three_d.entered.set(None);
        let recorded = self.node_transform(node, parent_transform);
        let mut contexts = self.three_d.contexts.borrow_mut();
        let collector = &mut contexts[at];
        collector.planes.push(Some(PlaneRecord {
            recorded,
            matrix: M4::device(plane, self.scale, collector.origin),
            own_hidden: plane.own_hidden,
            crosses_viewer: plane.crosses_viewer,
            commands: recording.commands,
        }));
        collector.planes.len() - 1
    }

    /// Draws a recorded context into `scene`: its chunks sorted back to front.
    fn compose(&self, scene: &mut impl PaintScene, mut collector: Collector, top: usize) {
        let mut chunks: Vec<(usize, Vec<RenderCommand>)> = Vec::new();
        expand(&mut collector.planes, top, &mut chunks);
        let viewport = Rect::new(
            self.initial_x,
            self.initial_y,
            self.initial_x + self.width as f64,
            self.initial_y + self.height as f64,
        );
        // Inside another plane's recording the scene is that plane's stand-in space, not where it
        // is drawn: a projective layer keeps a viewport's size of margin on each side.
        let viewport = if self.three_d.depth.get() > 0 {
            viewport.inflate(self.width as f64, self.height as f64)
        } else {
            viewport.inflate(64.0, 64.0)
        };

        let mut prepared: Vec<Chunk> = Vec::new();
        let mut polygons: Vec<bsp::Polygon> = Vec::new();
        for (plane, commands) in chunks {
            let Some(record) = collector.planes[plane].as_ref() else { continue };
            let (recorded, matrix) = (record.recorded, record.matrix);
            let determinant = recorded.determinant();
            if record.own_hidden || !recorded.is_finite() || !determinant.is_finite() || determinant.abs() <= 1e-12
                || !matrix.0.iter().flatten().all(|v| v.is_finite()) {
                continue;
            }
            let local = transformed(&commands, recorded.inverse());
            let Some(bounds) = crate::bounds::scene_bounds(&local) else { continue };
            let (draw, local) = match matrix.flat(bounds) {
                Some(Some(affine)) if !affine.is_finite() => continue,
                Some(None) => continue,
                Some(Some(affine)) if close(affine, recorded) => (Draw::AsRecorded, local),
                Some(Some(affine)) => (Draw::Affine(affine), local),
                // Print keeps the stand-ins; a plane crossing the viewer has none and is left out,
                // as it was before planes were drawn through their matrices.
                None if !self.composite_3d => {
                    if record.crosses_viewer {
                        continue;
                    }
                    (Draw::StandIn, local)
                }
                None => {
                    let h = matrix.homography();
                    // Plain fills and clips are projected as paths: exact edges, antialiased once, as
                    // a compositor draws a solid layer's quad. Anything else is rasterized in the
                    // plane and warped, but a backdrop filter, which reads what lies beneath the
                    // element (not in the plane's own pixels; a 3D transform makes no backdrop root),
                    // keeps the stand-in when it cannot be projected.
                    if let Some(projected) = project_commands(&local, &h, bounds) {
                        (Draw::Vector, projected)
                    } else if reads_backdrop(&commands) && !record.crosses_viewer {
                        (Draw::StandIn, local)
                    } else {
                        let projected = project(&h, bounds);
                        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
                        for &(x, y) in &projected {
                            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                        }
                        let clip = Rect::new(x0, y0, x1, y1).intersect(viewport);
                        if projected.len() < 3 || !(clip.width() > 0.0 && clip.height() > 0.0) {
                            continue;
                        }
                        (Draw::Projective(h, clip), local)
                    }
                }
            };
            // The polygon: the content's bounds in the plane, cut at the viewer's plane, projected;
            // for a stand-in, where the stand-in puts them (at the plane's depths), so that a split
            // cuts what is drawn.
            let corners =
                [(bounds.x0, bounds.y0), (bounds.x1, bounds.y0), (bounds.x1, bounds.y1), (bounds.x0, bounds.y1)];
            let points = match draw {
                Draw::StandIn => corners
                    .iter()
                    .map(|&(x, y)| {
                        let [_, _, z, w] = matrix.apply(x, y);
                        let p = recorded * Point::new(x, y);
                        [p.x, p.y, z / w.max(MIN_W)]
                    })
                    .collect(),
                _ => {
                    let homogeneous: Vec<[f64; 4]> = corners.iter().map(|&(x, y)| matrix.apply(x, y)).collect();
                    clip_w(&homogeneous)
                }
            };
            if points.len() < 3 || !points.iter().flatten().all(|v| v.is_finite()) {
                continue;
            }
            polygons.push(bsp::Polygon::new(points, prepared.len(), prepared.len()));
            let original = if matches!(draw, Draw::AsRecorded | Draw::StandIn) { commands } else { Vec::new() };
            prepared.push(Chunk { original, local, draw });
        }
        let ordered = if polygons.len() == 1 { polygons } else { bsp::back_to_front(polygons) };
        for piece in ordered {
            let chunk = &prepared[piece.chunk];
            // A piece of a split polygon is clipped where splits cut it, and only there: its own
            // edges are its content's, antialiased once. The cut is aliased on a backend that knows
            // the mark, so that two pieces of a plane share each pixel along it without a seam (viz
            // draws the interior edges of split quads without antialiasing).
            let clip = cut_clip(&piece);
            if let Some(clip) = &clip {
                if self.composite_3d {
                    scene.push_layer(
                        Fill::NonZero,
                        BlendMode::default(),
                        1.0,
                        Affine::IDENTITY,
                        clip,
                        Some(aliased_clip_marker()),
                        None,
                    );
                } else {
                    scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, clip);
                }
            }
            match &chunk.draw {
                Draw::AsRecorded | Draw::StandIn => {
                    scene.append_scene(scene_of(chunk.original.clone()), Affine::IDENTITY)
                }
                Draw::Affine(affine) => scene.append_scene(scene_of(chunk.local.clone()), *affine),
                Draw::Vector => scene.append_scene(scene_of(chunk.local.clone()), Affine::IDENTITY),
                Draw::Projective(h, clip) => {
                    scene.push_layer(
                        Fill::NonZero,
                        BlendMode::default(),
                        1.0,
                        Affine::IDENTITY,
                        clip,
                        Some(projective_marker(h)),
                        None,
                    );
                    scene.append_scene(scene_of(chunk.local.clone()), Affine::IDENTITY);
                    scene.pop_layer();
                }
            }
            if clip.is_some() {
                scene.pop_layer();
            }
        }
    }
}

/// The clip of a piece of a split polygon: the bounds of the piece (grown by a few pixels, which
/// its content's antialiased edges stay inside), cut by the line of each edge a split made, on the
/// piece's side. `None` for a polygon no split cut.
fn cut_clip(piece: &bsp::Polygon) -> Option<BezPath> {
    let mut cuts = piece.cuts().peekable();
    cuts.peek()?;
    let n = piece.points.len() as f64;
    let centre = piece.points.iter().fold([0.0, 0.0], |c, p| [c[0] + p[0] / n, c[1] + p[1] / n]);
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in &piece.points {
        (x0, y0, x1, y1) = (x0.min(p[0]), y0.min(p[1]), x1.max(p[0]), y1.max(p[1]));
    }
    const MARGIN: f64 = 4.0;
    let mut region = vec![
        [x0 - MARGIN, y0 - MARGIN],
        [x1 + MARGIN, y0 - MARGIN],
        [x1 + MARGIN, y1 + MARGIN],
        [x0 - MARGIN, y1 + MARGIN],
    ];
    for (at, along) in cuts {
        // Keep the side of the line the piece's centre is on.
        let side = |p: [f64; 2]| along[0] * (p[1] - at[1]) - along[1] * (p[0] - at[0]);
        let sign = side(centre).signum();
        let mut kept = Vec::with_capacity(region.len() + 1);
        for i in 0..region.len() {
            let (a, b) = (region[i], region[(i + 1) % region.len()]);
            let (da, db) = (side(a) * sign, side(b) * sign);
            if da >= 0.0 {
                kept.push(a);
            }
            if (da >= 0.0) != (db >= 0.0) {
                let t = da / (da - db);
                kept.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
            }
        }
        region = kept;
        if region.len() < 3 {
            return Some(BezPath::new());
        }
    }
    let mut outline = BezPath::new();
    for (i, p) in region.iter().enumerate() {
        if i == 0 {
            outline.move_to((p[0], p[1]));
        } else {
            outline.line_to((p[0], p[1]));
        }
    }
    outline.close_path();
    Some(outline)
}

/// The filter that marks a clip layer anyrender_vello_cpu rasterizes aliased (`replay`): an offset by
/// zero and negative zero, which moves nothing.
fn aliased_clip_marker() -> Arc<Filter> {
    Arc::new(Filter::single(FilterEffect::Offset(Vec2::new(0.0, -0.0))))
}

/// A chunk of a context, ready to draw.
struct Chunk {
    /// Its commands as Blitz recorded them (kept only when drawn so).
    original: Vec<RenderCommand>,
    /// The same in the plane's own device px.
    local: Vec<RenderCommand>,
    draw: Draw,
}

enum Draw {
    /// As recorded: the stand-in is the plane's affine.
    AsRecorded,
    /// As recorded, where the stand-in only approximates the plane (print, or a backdrop filter in
    /// a plane drawn through a perspective that cannot be projected).
    StandIn,
    /// Its local commands through this affine.
    Affine(Affine),
    /// Its local commands in a projective layer with this homography, clipped to this rect.
    Projective([f64; 9], Rect),
    /// Its local commands, projected as paths into the scene ([`project_commands`]).
    Vector,
}

/// `commands` (a plane's, in its own device px) with each path projected through the homography `h`
/// into the scene, when they are fills in one solid colour each and clips (layers without filters,
/// backdrop filters included): a path is flattened, cut at the viewer's plane and divided by W.
/// `None` for anything else (strokes, glyphs, gradients, images, shadows, filters), which is
/// rasterized in the plane instead. Clips reaching far past the content (`bounds`) are not
/// projected either: towards the horizon they would grow without bound.
fn project_commands(commands: &[RenderCommand], h: &[f64; 9], bounds: Rect) -> Option<Vec<RenderCommand>> {
    use anyrender::Paint;
    let reach = bounds.inflate(bounds.width().max(bounds.height()) + 64.0, bounds.width().max(bounds.height()) + 64.0);
    let project = |path: &BezPath, transform: Affine| -> Option<BezPath> {
        let local = transform.transform_rect_bbox(kurbo::Shape::bounding_box(path));
        if !(reach.contains(local.origin()) && reach.contains(Point::new(local.x1, local.y1))) {
            return None;
        }
        // A plane reaching towards the viewer's plane projects far out: rasterized in the plane,
        // each pixel samples only what it shows.
        let projected = project_path(path, transform, h);
        let far = kurbo::Shape::bounding_box(&projected);
        (far.x0 > -PROJECT_LIMIT && far.y0 > -PROJECT_LIMIT && far.x1 < PROJECT_LIMIT && far.y1 < PROJECT_LIMIT).then_some(projected)
    };
    let mut out = Vec::with_capacity(commands.len());
    for command in commands {
        out.push(match command {
            RenderCommand::Fill(fill) if matches!(fill.brush, Paint::Solid(_)) => {
                let mut fill = fill.clone();
                fill.shape = project(&fill.shape, fill.transform)?;
                fill.transform = Affine::IDENTITY;
                fill.brush_transform = None;
                RenderCommand::Fill(fill)
            }
            RenderCommand::PushLayer(layer) if layer.filter.is_none() => {
                let mut layer = layer.clone();
                layer.clip = project(&layer.clip, layer.transform)?;
                layer.transform = Affine::IDENTITY;
                RenderCommand::PushLayer(layer)
            }
            RenderCommand::PushClipLayer(layer) => {
                let mut layer = layer.clone();
                layer.clip = project(&layer.clip, layer.transform)?;
                layer.transform = Affine::IDENTITY;
                RenderCommand::PushClipLayer(layer)
            }
            RenderCommand::PopLayer => RenderCommand::PopLayer,
            _ => return None,
        });
    }
    Some(out)
}

/// `path` under `transform` (into the plane's own px), then through the homography `h`: each
/// subpath flattened to lines, cut at the viewer's plane, divided by W.
fn project_path(path: &BezPath, transform: Affine, h: &[f64; 9]) -> BezPath {
    let mut out = BezPath::new();
    let mut subpath: Vec<Point> = Vec::new();
    let flush = |subpath: &mut Vec<Point>, out: &mut BezPath| {
        if subpath.len() >= 3 {
            let homogeneous: Vec<[f64; 4]> = subpath
                .iter()
                .map(|p| {
                    let [a, b, c, d, e, f, g, i, j] = *h;
                    [a * p.x + b * p.y + c, d * p.x + e * p.y + f, 0.0, g * p.x + i * p.y + j]
                })
                .collect();
            // On a grid of 1/1024 px: the same plane reached by another product of matrices (a
            // `perspective()` function or the `perspective` property) lands on the same pixels,
            // where rounding noise moved an edge's coverage by a level (WPT
            // `transform3d-perspective-001`).
            let snap = |v: f64| if v.abs() <= PROJECT_LIMIT { (v * 1024.0).round() / 1024.0 } else { v };
            let points: Vec<[f64; 3]> =
                clip_w(&homogeneous).into_iter().map(|p| [snap(p[0]), snap(p[1]), p[2]]).collect();
            for (k, p) in points.iter().enumerate() {
                if k == 0 {
                    out.move_to((p[0], p[1]));
                } else {
                    out.line_to((p[0], p[1]));
                }
            }
            if points.len() >= 3 {
                out.close_path();
            }
        }
        subpath.clear();
    };
    kurbo::flatten(transform * path.clone(), 0.05, |element| match element {
        kurbo::PathEl::MoveTo(p) => {
            flush(&mut subpath, &mut out);
            subpath.push(p);
        }
        kurbo::PathEl::LineTo(p) => subpath.push(p),
        kurbo::PathEl::ClosePath => flush(&mut subpath, &mut out),
        _ => {}
    });
    flush(&mut subpath, &mut out);
    out
}

/// Cuts the recordings of `planes` from `index` on into chunks, in paint order (see the module
/// docs).
fn expand(planes: &mut [Option<PlaneRecord>], index: usize, chunks: &mut Vec<(usize, Vec<RenderCommand>)>) {
    let Some(commands) = planes.get_mut(index).and_then(|p| p.as_mut()).map(|p| std::mem::take(&mut p.commands)) else {
        return;
    };
    let mut current = Vec::new();
    let mut depth = 0usize;
    let mut commands = commands.into_iter();
    while let Some(command) = commands.next() {
        if let Some(child) = marker_index(&command) {
            // The marker's pop.
            commands.next();
            if depth == 0 {
                if !current.is_empty() {
                    chunks.push((index, std::mem::take(&mut current)));
                }
                expand(planes, child, chunks);
            } else {
                inline(planes, child, index, &mut current);
            }
            continue;
        }
        match command {
            RenderCommand::PushLayer(_) | RenderCommand::PushClipLayer(_) => depth += 1,
            RenderCommand::PopLayer => depth = depth.saturating_sub(1),
            _ => {}
        }
        current.push(command);
    }
    if !current.is_empty() {
        chunks.push((index, current));
    }
}

/// Appends the recording of plane `index` to `out`, its own markers' planes too: a plane painted
/// inside a clip or a layer of the plane `into` stays there, flattened into that plane (see
/// [`flattened_into`]).
fn inline(planes: &mut [Option<PlaneRecord>], index: usize, into: usize, out: &mut Vec<RenderCommand>) {
    let Some(commands) = planes.get_mut(index).and_then(|p| p.as_mut()).map(|p| std::mem::take(&mut p.commands)) else {
        return;
    };
    let mut own = Vec::with_capacity(commands.len());
    let mut commands = commands.into_iter();
    while let Some(command) = commands.next() {
        if let Some(child) = marker_index(&command) {
            commands.next();
            inline(planes, child, index, &mut own);
        } else {
            own.push(command);
        }
    }
    match flattened_into(planes, index, into, &own) {
        Some(t) => out.extend(transformed(&own, t)),
        None => out.extend(own),
    }
}

/// The affine that moves the recording `commands` of plane `index` (painted with its stand-in) to
/// where it lies in the plane `into`, as that plane's recording has it: the map from the one plane
/// to the other (exact when it is affine, the affine closest to it at the content's corners when a
/// perspective lies between them), between their stand-ins. `None` when it is the identity or
/// there is none.
fn flattened_into(planes: &[Option<PlaneRecord>], index: usize, into: usize, commands: &[RenderCommand]) -> Option<Affine> {
    let (plane, outer) = (planes.get(index)?.as_ref()?, planes.get(into)?.as_ref()?);
    if !(plane.recorded.determinant().abs() > 1e-12) {
        return None;
    }
    let bounds = crate::bounds::scene_bounds(&transformed(commands, plane.recorded.inverse()))?;
    let g = mul3(&inverse3(&outer.matrix.homography())?, &plane.matrix.homography());
    let project = |x: f64, y: f64| {
        let w = g[6] * x + g[7] * y + g[8];
        (w >= MIN_W).then(|| ((g[0] * x + g[1] * y + g[2]) / w, (g[3] * x + g[4] * y + g[5]) / w))
    };
    let (x0, y0, x1, y1) = (bounds.x0, bounds.y0, bounds.x1.max(bounds.x0 + 1.0), bounds.y1.max(bounds.y0 + 1.0));
    let (p00, p10, p01, p11) = (project(x0, y0)?, project(x1, y0)?, project(x0, y1)?, project(x1, y1)?);
    // Least squares over the four corners, as blitz-dom's `flatten_transform`.
    let (w, h) = (x1 - x0, y1 - y0);
    let a = ((p10.0 + p11.0 - p00.0 - p01.0) / (2.0 * w), (p10.1 + p11.1 - p00.1 - p01.1) / (2.0 * w));
    let c = ((p01.0 + p11.0 - p00.0 - p10.0) / (2.0 * h), (p01.1 + p11.1 - p00.1 - p10.1) / (2.0 * h));
    let mean = ((p00.0 + p10.0 + p01.0 + p11.0) / 4.0, (p00.1 + p10.1 + p01.1 + p11.1) / 4.0);
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let e = (mean.0 - a.0 * cx - c.0 * cy, mean.1 - a.1 * cx - c.1 * cy);
    let between = Affine::new([a.0, a.1, c.0, c.1, e.0, e.1]);
    let t = outer.recorded * between * plane.recorded.inverse();
    // An edge-on plane has no map to speak of: the recording stays as it is.
    let usable = t.as_coeffs().iter().all(|v| v.is_finite() && v.abs() < 1e7) && t.determinant().abs() > 1e-9;
    (usable && !close(t, Affine::IDENTITY)).then_some(t)
}

/// `a * b` of row-major 3x3 matrices (`b` applied first).
fn mul3(a: &[f64; 9], b: &[f64; 9]) -> [f64; 9] {
    std::array::from_fn(|i| {
        let (r, c) = (i / 3, i % 3);
        (0..3).map(|k| a[r * 3 + k] * b[k * 3 + c]).sum()
    })
}

/// The inverse of a row-major 3x3 matrix, `None` for a singular one.
fn inverse3(m: &[f64; 9]) -> Option<[f64; 9]> {
    let cofactor =
        |r0: usize, r1: usize, c0: usize, c1: usize| m[r0 * 3 + c0] * m[r1 * 3 + c1] - m[r0 * 3 + c1] * m[r1 * 3 + c0];
    let adjugate = [
        cofactor(1, 2, 1, 2),
        -cofactor(0, 2, 1, 2),
        cofactor(0, 1, 1, 2),
        -cofactor(1, 2, 0, 2),
        cofactor(0, 2, 0, 2),
        -cofactor(0, 1, 0, 2),
        cofactor(1, 2, 0, 1),
        -cofactor(0, 2, 0, 1),
        cofactor(0, 1, 0, 1),
    ];
    let determinant = m[0] * adjugate[0] + m[1] * adjugate[3] + m[2] * adjugate[6];
    (determinant.is_finite() && determinant != 0.0).then(|| adjugate.map(|v| v / determinant))
}

/// Whether `commands` draw a backdrop filter.
fn reads_backdrop(commands: &[RenderCommand]) -> bool {
    commands.iter().any(|command| matches!(command, RenderCommand::PushLayer(layer) if layer.backdrop_filter.is_some()))
}

fn scene_of(commands: Vec<RenderCommand>) -> Scene {
    let mut scene = Scene::new();
    scene.commands = commands;
    scene
}

/// `commands` with `t` applied after each one's transform.
fn transformed(commands: &[RenderCommand], t: Affine) -> Vec<RenderCommand> {
    let mut scene = Scene::new();
    scene.append_scene(scene_of(commands.to_vec()), t);
    scene.commands
}

/// Whether two affines are the same but for rounding.
fn close(a: Affine, b: Affine) -> bool {
    let (a, b) = (a.as_coeffs(), b.as_coeffs());
    (0..4).all(|i| (a[i] - b[i]).abs() <= 1e-9) && (4..6).all(|i| (a[i] - b[i]).abs() <= 1e-7)
}


#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    /// A square through a homography that keeps its top edge and halves its bottom edge about
    /// the centre is a trapezoid; a plane crossing the viewer's plane is cut there.
    #[test]
    fn paths_project_through_a_homography() {
        // W = 1 + y / 40: the bottom edge (y = 40) at W = 2.
        let h = [1.0, 0.5, 0.0, 0.0, 2.0, 0.0, 0.0, 1.0 / 40.0, 1.0];
        let path = project_path(&Rect::new(0.0, 0.0, 40.0, 40.0).to_path(0.1), Affine::IDENTITY, &h);
        let points: Vec<Point> = path.elements().iter().filter_map(|e| e.end_point()).collect();
        let close = |a: Point, b: (f64, f64)| (a.x - b.0).abs() < 1e-9 && (a.y - b.1).abs() < 1e-9;
        for corner in [(0.0, 0.0), (40.0, 0.0), (30.0, 40.0), (10.0, 40.0)] {
            assert!(points.iter().any(|&p| close(p, corner)), "{corner:?} in {points:?}");
        }
        // W = 1 - y / 20: the half below y = 20 is behind the viewer.
        let h = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, -1.0 / 20.0, 1.0];
        let cut = project_path(&Rect::new(0.0, 0.0, 40.0, 40.0).to_path(0.1), Affine::IDENTITY, &h);
        assert!(kurbo::Shape::bounding_box(&cut).y1 > 1e6, "reaches towards the horizon");
        assert!(cut.elements().iter().filter_map(|e| e.end_point()).all(|p| p.y >= 0.0));
    }

    /// Only commands a projection keeps exact are projected: fills in a solid colour and clips.
    #[test]
    fn only_plain_fills_and_clips_are_projected() {
        let h = [1.0, 0.5, 0.0, 0.0, 2.0, 0.0, 0.0, 1.0 / 40.0, 1.0];
        let square = Rect::new(0.0, 0.0, 40.0, 40.0);
        let mut plain = Scene::new();
        plain.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &square);
        plain.fill(Fill::NonZero, Affine::IDENTITY, peniko::color::palette::css::LIME, None, &square);
        plain.pop_layer();
        let projected = project_commands(&plain.commands, &h, square).expect("projected");
        assert!(projected.iter().all(|c| match c {
            RenderCommand::Fill(f) => f.transform == Affine::IDENTITY,
            RenderCommand::PushClipLayer(l) => l.transform == Affine::IDENTITY,
            _ => true,
        }));
        let mut stroked = Scene::new();
        stroked.stroke(&kurbo::Stroke::new(2.0), Affine::IDENTITY, peniko::color::palette::css::LIME, None, &square);
        assert!(project_commands(&stroked.commands, &h, square).is_none());
    }

    /// A piece of a split polygon is clipped along its cut only, on its own side, with room past its
    /// other edges for its content's antialiasing.
    #[test]
    fn a_split_piece_is_clipped_along_its_cut() {
        let falling =
            bsp::Polygon::new(vec![[0.0, 0.0, 5.0], [10.0, 0.0, -5.0], [10.0, 10.0, -5.0], [0.0, 10.0, 5.0]], 1, 1);
        let rising =
            bsp::Polygon::new(vec![[0.0, 0.0, -5.0], [10.0, 0.0, 5.0], [10.0, 10.0, 5.0], [0.0, 10.0, -5.0]], 0, 0);
        let sorted = bsp::back_to_front(vec![rising, falling]);
        let left = sorted.last().unwrap();
        let clip = cut_clip(left).expect("a cut piece");
        let bounds = kurbo::Shape::bounding_box(&clip);
        assert!((bounds.x1 - 5.0).abs() < 1e-9, "{bounds:?}");
        assert!(bounds.x0 < -1.0 && bounds.y0 < -1.0 && bounds.y1 > 11.0, "{bounds:?}");
        assert!(cut_clip(&sorted[1]).is_none());
    }
}

/// Back-to-front order of planar polygons by a BSP tree, as viz's `BspTree` orders the quads of a
/// 3D sorting context: the first polygon splits the others into those in front of its plane and
/// those behind it (a polygon crossing the plane is split in two), and the polygons in its plane
/// draw in paint order. The viewer looks down the Z axis from +Z (CSS: Z towards the viewer),
/// after the perspective divide, where planes stay planes.
pub(crate) mod bsp {
    use std::collections::VecDeque;

    /// A convex polygon: its points (x and y in the scene, z the depth), its chunk, its paint order.
    #[derive(Clone, Debug)]
    pub(crate) struct Polygon {
        pub(crate) points: Vec<[f64; 3]>,
        /// Whether the edge from each point to the next is where a split cut the polygon (the
        /// piece is drawn clipped there, and only there).
        pub(crate) cut: Vec<bool>,
        pub(crate) chunk: usize,
        pub(crate) order: usize,
    }

    impl Polygon {
        /// A polygon of `points` none of whose edges is cut.
        pub(crate) fn new(points: Vec<[f64; 3]>, chunk: usize, order: usize) -> Polygon {
            let cut = vec![false; points.len()];
            Polygon { points, cut, chunk, order }
        }

        /// The lines (in the plane of the screen) where splits cut the piece: each as a point on it
        /// and the direction along it.
        pub(crate) fn cuts(&self) -> impl Iterator<Item = ([f64; 2], [f64; 2])> + '_ {
            let n = self.points.len();
            (0..n).filter(|&i| self.cut[i]).map(move |i| {
                let (a, b) = (self.points[i], self.points[(i + 1) % n]);
                ([a[0], a[1]], [b[0] - a[0], b[1] - a[1]])
            })
        }
    }

    /// How far (px) a point may lie off a plane and be in it (viz's `split_threshold`).
    const IN_PLANE: f64 = 0.05;

    struct Plane {
        normal: [f64; 3],
        d: f64,
    }

    impl Plane {
        /// The plane of `points` (Newell's method), `None` for a polygon of no area.
        fn of(points: &[[f64; 3]]) -> Option<Plane> {
            let mut n = [0.0; 3];
            let mut centre = [0.0; 3];
            for i in 0..points.len() {
                let (a, b) = (points[i], points[(i + 1) % points.len()]);
                n[0] += (a[1] - b[1]) * (a[2] + b[2]);
                n[1] += (a[2] - b[2]) * (a[0] + b[0]);
                n[2] += (a[0] - b[0]) * (a[1] + b[1]);
                for k in 0..3 {
                    centre[k] += a[k] / points.len() as f64;
                }
            }
            let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if !(length > 1e-9) {
                return None;
            }
            let normal = n.map(|v| v / length);
            let d = -(normal[0] * centre[0] + normal[1] * centre[1] + normal[2] * centre[2]);
            Some(Plane { normal, d })
        }

        fn distance(&self, p: &[f64; 3]) -> f64 {
            self.normal[0] * p[0] + self.normal[1] * p[1] + self.normal[2] * p[2] + self.d
        }
    }

    struct Node {
        plane: Plane,
        /// The polygons in its plane, in paint order.
        coplanar: Vec<Polygon>,
        front: Option<Box<Node>>,
        back: Option<Box<Node>>,
    }

    enum Side {
        Coplanar(Polygon),
        Front(Polygon),
        Back(Polygon),
        Both(Polygon, Polygon),
    }

    /// Where `polygon` lies relative to `plane`, split in two if it crosses it.
    fn classify(plane: &Plane, polygon: Polygon) -> Side {
        let distances: Vec<f64> = polygon.points.iter().map(|p| plane.distance(p)).collect();
        let front = distances.iter().any(|&d| d > IN_PLANE);
        let back = distances.iter().any(|&d| d < -IN_PLANE);
        match (front, back) {
            (false, false) => Side::Coplanar(polygon),
            (true, false) => Side::Front(polygon),
            (false, true) => Side::Back(polygon),
            (true, true) => {
                let n = polygon.points.len();
                let snapped = |d: f64| if d.abs() <= IN_PLANE { 0.0 } else { d };
                let distances: Vec<f64> = distances.into_iter().map(snapped).collect();
                // The piece on the side of `sign`: each point with whether the edge leaving it is
                // the cut (from where the polygon leaves the side to where it comes back).
                let piece = |sign: f64| {
                    let (mut points, mut cut) = (Vec::new(), Vec::new());
                    for i in 0..n {
                        let (a, b) = (polygon.points[i], polygon.points[(i + 1) % n]);
                        let (da, db) = (distances[i] * sign, distances[(i + 1) % n] * sign);
                        if da >= 0.0 {
                            points.push(a);
                            // The edge from a point on the plane runs along the new cut; from
                            // one off it, it goes on along edge i (cut where that was).
                            cut.push(if db < 0.0 { da == 0.0 || polygon.cut[i] } else { polygon.cut[i] });
                        }
                        if (da > 0.0 && db < 0.0) || (da < 0.0 && db > 0.0) {
                            let t = da / (da - db);
                            points.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]);
                            // Leaving the side, the cut follows; coming back, the edge goes on.
                            cut.push(if da > 0.0 { true } else { polygon.cut[i] });
                        }
                    }
                    Polygon { points, cut, ..polygon.clone() }
                };
                Side::Both(piece(1.0), piece(-1.0))
            }
        }
    }

    fn build(mut list: VecDeque<Polygon>) -> Option<Box<Node>> {
        let (first, plane) = loop {
            let first = list.pop_front()?;
            if let Some(plane) = Plane::of(&first.points) {
                break (first, plane);
            }
        };
        let mut coplanar = vec![first];
        let (mut front, mut back) = (VecDeque::new(), VecDeque::new());
        for polygon in list {
            match classify(&plane, polygon) {
                Side::Coplanar(p) => coplanar.push(p),
                Side::Front(p) => front.push_back(p),
                Side::Back(p) => back.push_back(p),
                Side::Both(a, b) => {
                    front.push_back(a);
                    back.push_back(b);
                }
            }
        }
        coplanar.sort_by_key(|p| p.order);
        Some(Box::new(Node { plane, coplanar, front: build(front), back: build(back) }))
    }

    fn walk(node: Node, out: &mut Vec<Polygon>) {
        // The viewer is on the front side when the normal points towards +Z: the back is farther.
        let (far, near) = if node.plane.normal[2] >= 0.0 { (node.back, node.front) } else { (node.front, node.back) };
        if let Some(far) = far {
            walk(*far, out);
        }
        // A plane seen edge-on shows nothing.
        if node.plane.normal[2].abs() > 1e-9 {
            out.extend(node.coplanar);
        }
        if let Some(near) = near {
            walk(*near, out);
        }
    }

    /// The polygons (in paint order) back to front, split where they cross.
    pub(crate) fn back_to_front(polygons: Vec<Polygon>) -> Vec<Polygon> {
        let mut out = Vec::with_capacity(polygons.len());
        if let Some(root) = build(polygons.into()) {
            walk(*root, &mut out);
        }
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn square(z: f64, order: usize) -> Polygon {
            Polygon::new(vec![[0.0, 0.0, z], [10.0, 0.0, z], [10.0, 10.0, z], [0.0, 10.0, z]], order, order)
        }

        #[test]
        fn nearer_planes_draw_later_and_coplanar_ones_in_paint_order() {
            let sorted = back_to_front(vec![square(10.0, 0), square(-10.0, 1), square(0.0, 2), square(0.0, 3)]);
            let order: Vec<usize> = sorted.iter().map(|p| p.order).collect();
            assert_eq!(order, [1, 2, 3, 0]);
        }

        #[test]
        fn crossing_planes_are_split_where_they_meet() {
            // Two planes through x = 5: one rising to the right, one falling.
            let rising =
                Polygon::new(vec![[0.0, 0.0, -5.0], [10.0, 0.0, 5.0], [10.0, 10.0, 5.0], [0.0, 10.0, -5.0]], 0, 0);
            let falling =
                Polygon::new(vec![[0.0, 0.0, 5.0], [10.0, 0.0, -5.0], [10.0, 10.0, -5.0], [0.0, 10.0, 5.0]], 1, 1);
            let sorted = back_to_front(vec![rising, falling]);
            assert_eq!(sorted.len(), 3);
            // The falling plane's piece left of x = 5 is in front of the rising plane: it draws
            // last; its right piece, behind, first. Each is cut along x = 5 only.
            let last = sorted.last().unwrap();
            assert_eq!(last.chunk, 1);
            assert!(last.points.iter().all(|p| p[0] <= 5.0 + 1e-9));
            let first = &sorted[0];
            assert!(first.chunk == 1 && first.points.iter().all(|p| p[0] >= 5.0 - 1e-9));
            for piece in [first, last] {
                let cuts: Vec<_> = piece.cuts().collect();
                assert_eq!(cuts.len(), 1, "{piece:?}");
                let (at, along) = cuts[0];
                assert!((at[0] - 5.0).abs() < 1e-9 && along[0].abs() < 1e-9, "{piece:?}");
            }
            assert!(sorted[1].cuts().next().is_none());
        }

        /// A piece cut once and cut again across its first cut keeps both cuts: the edge of the
        /// first cut it keeps part of stays a cut.
        #[test]
        fn a_piece_cut_twice_keeps_both_cuts() {
            let piece = |polygon: Polygon, plane: Plane, front: bool| match classify(&plane, polygon) {
                Side::Both(a, b) => {
                    if front {
                        a
                    } else {
                        b
                    }
                }
                _ => panic!("not split"),
            };
            // The square split at x = 5, then the left piece split at y = 5.
            let square = square(0.0, 0);
            let left = piece(square, Plane { normal: [-1.0, 0.0, 0.0], d: 5.0 }, true);
            assert!(left.points.iter().all(|p| p[0] <= 5.0));
            let corner = piece(left, Plane { normal: [0.0, -1.0, 0.0], d: 5.0 }, true);
            assert!(corner.points.iter().all(|p| p[0] <= 5.0 && p[1] <= 5.0), "{corner:?}");
            let mut cuts: Vec<_> = corner
                .cuts()
                .map(|(at, along)| if along[0].abs() < 1e-9 { ('x', at[0]) } else { ('y', at[1]) })
                .collect();
            cuts.sort_by(|a, b| a.partial_cmp(b).unwrap());
            assert_eq!(cuts, [('x', 5.0), ('y', 5.0)], "{corner:?}");
        }
    }
}
