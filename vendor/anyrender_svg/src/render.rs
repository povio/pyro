// Copyright 2024 the Vello Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use crate::util;
use anyrender::{Filter, Paint, PaintScene, Scene};
use anyrender_vello_cpu::VelloCpuImageRenderer;
use std::sync::Arc;
use kurbo::{Affine, BezPath, Rect, Shape};
use peniko::{BlendMode, Blob, Color, Compose, Fill, ImageBrush, ImageData, Mix};
use usvg::{Node, Path};

/// pyro: what a render needs besides the scene.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Ctx {
    /// The device-space area the output can show. Offscreen rasters (masks,
    /// pattern tiles) cover no more than it.
    pub(crate) cull: Rect,
}

/// pyro: the most device pixels an offscreen raster (a mask, a pattern tile)
/// gets; a larger one is rendered at a lower resolution and scaled up.
const MAX_RASTER_PIXELS: f64 = 4096.0 * 4096.0;

/// Renders `group`'s children. `transform` maps the space their
/// `abs_transform`s are in (the root's, or a clip path's, mask's or pattern's
/// own root) to the tree's user space, and `global_transform` that to the
/// scene's.
pub(crate) fn render_group<S: PaintScene, F: FnMut(&mut S, &usvg::Node)>(
    scene: &mut S,
    group: &usvg::Group,
    transform: Affine,
    global_transform: Affine,
    error_handler: &mut F,
    ctx: &Ctx,
) {
    let base = transform;
    for node in group.children() {
        let transform = base * util::to_affine(&node.abs_transform());
        match node {
            usvg::Node::Group(g) => {
                render_group_node(
                    scene,
                    g,
                    base,
                    transform,
                    global_transform,
                    error_handler,
                    ctx,
                );
            }
            usvg::Node::Path(path) => {
                if !path.is_visible() {
                    continue;
                }
                let local_path = util::to_bez_path(path);

                let transform = global_transform * transform;
                match path.paint_order() {
                    usvg::PaintOrder::FillAndStroke => {
                        fill(
                            scene,
                            error_handler,
                            path,
                            transform,
                            &local_path,
                            node,
                            ctx,
                        );
                        stroke(
                            scene,
                            error_handler,
                            path,
                            transform,
                            &local_path,
                            node,
                            ctx,
                        );
                    }
                    usvg::PaintOrder::StrokeAndFill => {
                        stroke(
                            scene,
                            error_handler,
                            path,
                            transform,
                            &local_path,
                            node,
                            ctx,
                        );
                        fill(
                            scene,
                            error_handler,
                            path,
                            transform,
                            &local_path,
                            node,
                            ctx,
                        );
                    }
                }
            }
            usvg::Node::Image(img) => {
                if !img.is_visible() {
                    continue;
                }
                match img.kind() {
                    usvg::ImageKind::JPEG(_)
                    | usvg::ImageKind::PNG(_)
                    | usvg::ImageKind::GIF(_)
                    | usvg::ImageKind::WEBP(_) => {
                        #[cfg(feature = "image")]
                        {
                            let Ok(decoded_image) = util::decode_raw_raster_image(img.kind())
                            else {
                                error_handler(scene, node);
                                continue;
                            };
                            let image = util::into_image(decoded_image, img.rendering_mode());
                            let image_ts = global_transform * transform;
                            scene.draw_image(image.as_ref(), image_ts);
                        }

                        #[cfg(not(feature = "image"))]
                        {
                            error_handler(scene, node);
                            continue;
                        }
                    }
                    usvg::ImageKind::SVG(svg) => {
                        render_group(
                            scene,
                            svg.root(),
                            transform,
                            global_transform,
                            error_handler,
                            ctx,
                        );
                    }
                }
            }
            usvg::Node::Text(text) => {
                // The children of the flattened text group already carry the
                // text node's absolute transform (in this subtree's space), so
                // recurse with this subtree's base transform, as for a group.
                render_group(
                    scene,
                    text.flattened(),
                    base,
                    global_transform,
                    error_handler,
                    ctx,
                );
            }
        }
    }
}

/// pyro: a group with its clip path, mask, opacity and blend mode, applied in
/// that order (as resvg does) to its rendered children: a clip path with one
/// shape is a clip layer; any other clip path and every mask is a `DestIn`
/// layer over the children in an isolated layer.
fn render_group_node<S: PaintScene, F: FnMut(&mut S, &usvg::Node)>(
    scene: &mut S,
    g: &usvg::Group,
    base: Affine,
    transform: Affine,
    global_transform: Affine,
    error_handler: &mut F,
    ctx: &Ctx,
) {
    let alpha = g.opacity().get();
    let mix = util::to_mix(g.blend_mode());
    // The group's user space (its own `transform` included) in the scene.
    let ts = global_transform * transform;
    let filters = filter_layers(g);

    let clip_shapes = g.clip_path().and_then(|clip| clip_shapes(clip, ts));
    let simple_clip = clip_shapes
        .as_ref()
        .filter(|shapes| shapes.len() == 1)
        .map(|shapes| shapes[0].clone());
    let complex_clip = g.clip_path().filter(|_| simple_clip.is_none());
    let mask = g.mask();

    #[allow(deprecated)]
    let blends = mix != Mix::Normal || alpha < 1.0 || g.isolate();
    let needs_layer = blends || complex_clip.is_some() || mask.is_some();

    // The layer's extent: the group's layer bounding box (outset by 2px on
    // each side, like resvg, so anti-aliased pixels at its edge are kept),
    // within its clip path's and mask's bounds and what the output shows.
    let mut layer_rect = Rect::ZERO;
    if needs_layer {
        let bounding_box = g.layer_bounding_box();
        let rect = Rect::from_origin_size(
            (bounding_box.x() as f64, bounding_box.y() as f64),
            (bounding_box.width() as f64, bounding_box.height() as f64),
        );
        layer_rect = ts.transform_rect_bbox(rect).inflate(2.0, 2.0);
        if let Some(clip) = complex_clip {
            layer_rect = layer_rect.intersect(clip_bounds(clip, ts).inflate(1.0, 1.0));
        }
        if let Some(mask) = mask {
            layer_rect = layer_rect.intersect(mask_bounds(mask, ts).inflate(1.0, 1.0));
        }
        if mask.is_some() {
            // A mask is rasterized over the layer: no more than can show.
            layer_rect = layer_rect.intersect(ctx.cull.inflate(2.0, 2.0));
        }
        // Whole device pixels, so a mask raster lines up with them.
        layer_rect = Rect::new(
            layer_rect.x0.floor(),
            layer_rect.y0.floor(),
            layer_rect.x1.ceil(),
            layer_rect.y1.ceil(),
        );
        if complex_clip.is_some() || mask.is_some() {
            if !(layer_rect.width() > 0.0 && layer_rect.height() > 0.0) {
                // Clipped or masked out entirely.
                return;
            }
        }
    }

    // One filled rectangle clipped by one shape (a reftest's usual subject, a
    // background cut to a shape): the shape is filled with the rectangle's
    // paint, so its edges are a filled shape's (a clip layer's coverage rounds a
    // level apart from a fill's), as a reference drawing the shape has them.
    if let (Some(shapes), false, None, true) = (&clip_shapes, blends, mask, filters.is_empty()) {
        if fill_clip_shapes(scene, g, base, global_transform, shapes, ctx) {
            return;
        }
    }

    // One shape clipping the group (with its opacity and blend mode): one
    // layer, as upstream (a clip layer composites a level apart).
    if let (Some((rule, clip_ts, path)), None, None) = (&simple_clip, complex_clip, mask) {
        scene.push_layer(
            *rule,
            BlendMode {
                mix,
                compose: Compose::SrcOver,
            },
            alpha,
            *clip_ts,
            path,
            None,
            None,
        );
        render_filtered(scene, g, base, ts, &filters, global_transform, error_handler, ctx);
        scene.pop_layer();
        return;
    }

    if let Some((rule, clip_ts, path)) = &simple_clip {
        scene.push_layer(
            *rule,
            BlendMode::new(Mix::Normal, Compose::SrcOver),
            1.0,
            *clip_ts,
            path,
            None,
            None,
        );
    }
    if needs_layer {
        scene.push_layer(
            Fill::NonZero,
            BlendMode {
                mix,
                compose: Compose::SrcOver,
            },
            alpha,
            Affine::IDENTITY,
            &layer_rect,
            None,
            None,
        );
    }

    render_filtered(scene, g, base, ts, &filters, global_transform, error_handler, ctx);

    if let Some(clip) = complex_clip {
        apply_clip(scene, clip, ts, layer_rect);
    }
    if let Some(mask) = mask {
        apply_mask(scene, mask, ts, layer_rect);
    }

    if needs_layer {
        scene.pop_layer();
    }
    if simple_clip.is_some() {
        scene.pop_layer();
    }
}

/// pyro: the group's filters, each with its filter region (in the group's user space), the
/// first applied first; none when one has a primitive anyrender_vello_cpu does not do (the
/// group is then drawn unfiltered, as before; see `crate::filter`).
fn filter_layers(g: &usvg::Group) -> Vec<(Arc<Filter>, Rect)> {
    g.filters()
        .iter()
        .map(|filter| {
            let region = filter.rect();
            let region = Rect::new(
                region.left() as f64,
                region.top() as f64,
                region.right() as f64,
                region.bottom() as f64,
            );
            crate::filter::convert(filter).map(|graph| (Arc::new(graph), region))
        })
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default()
}

/// pyro: `g`'s children in its filters' layers (`ts` maps the group's user space to the scene),
/// the first filter innermost, clipped to its filter region when they reach past it (SVG's
/// source graphic stops there).
#[allow(clippy::too_many_arguments)]
fn render_filtered<S: PaintScene, F: FnMut(&mut S, &usvg::Node)>(
    scene: &mut S,
    g: &usvg::Group,
    base: Affine,
    ts: Affine,
    filters: &[(Arc<Filter>, Rect)],
    global_transform: Affine,
    error_handler: &mut F,
    ctx: &Ctx,
) {
    for (filter, region) in filters.iter().rev() {
        scene.push_layer(
            Fill::NonZero,
            BlendMode::default(),
            1.0,
            ts,
            region,
            Some(filter.clone()),
            None,
        );
    }
    let clip = filters.first().map(|(_, region)| *region).filter(|region| {
        let content = g.stroke_bounding_box();
        let content = Rect::new(
            content.left() as f64,
            content.top() as f64,
            content.right() as f64,
            content.bottom() as f64,
        );
        content.union(*region) != *region
    });
    if let Some(region) = clip {
        scene.push_clip_layer(Fill::NonZero, ts, &region);
    }
    render_group(scene, g, base, global_transform, error_handler, ctx);
    if clip.is_some() {
        scene.pop_layer();
    }
    for _ in filters {
        scene.pop_layer();
    }
}

/// pyro: `g`, one filled axis-aligned rectangle whose scene bounds hold those
/// of its clip path's shapes, drawn as the shapes filled with the rectangle's
/// paint, in order (several only with an opaque colour, whose overlaps then
/// look as the union's). `false` (nothing drawn) for any other group.
fn fill_clip_shapes<S: PaintScene>(
    scene: &mut S,
    g: &usvg::Group,
    base: Affine,
    global_transform: Affine,
    shapes: &[(Fill, Affine, BezPath)],
    ctx: &Ctx,
) -> bool {
    let [Node::Path(path)] = g.children() else {
        return false;
    };
    let Some(fill) = path.fill() else {
        return false;
    };
    if !path.is_visible() || path.stroke().is_some() || shapes.is_empty() {
        return false;
    }
    if shapes.len() > 1 {
        let usvg::Paint::Color(_) = fill.paint() else {
            return false;
        };
        if fill.opacity().get() < 1.0 {
            return false;
        }
    }
    if shapes
        .iter()
        .any(|(_, clip_ts, _)| clip_ts.determinant().abs() < 1e-12)
    {
        return false;
    }
    let content_ts = global_transform * base * util::to_affine(&path.abs_transform());
    let [_, b, c, _, _, _] = content_ts.as_coeffs();
    let content = util::to_bez_path(path);
    let Some(rect) = (b == 0.0 && c == 0.0)
        .then(|| path_rect(&content))
        .flatten()
    else {
        return false;
    };
    let content_rect = content_ts.transform_rect_bbox(rect);
    // Within a hundredth of a pixel: usvg's geometry is f32.
    let holds = shapes.iter().all(|(_, clip_ts, clip_path)| {
        let bounds = clip_ts.transform_rect_bbox(clip_path.bounding_box());
        content_rect.x0 <= bounds.x0 + 0.01
            && content_rect.y0 <= bounds.y0 + 0.01
            && content_rect.x1 >= bounds.x1 - 0.01
            && content_rect.y1 >= bounds.y1 - 0.01
    });
    if !holds {
        return false;
    }
    let Some((paint, brush_ts)) =
        brush(fill.paint(), fill.opacity(), content_ts, content_rect, ctx)
    else {
        return false;
    };
    for (rule, clip_ts, clip_path) in shapes {
        // The brush, from the rectangle's space to the clip shape's.
        let shape_brush_ts = clip_ts.inverse() * content_ts * brush_ts;
        scene.fill(
            *rule,
            *clip_ts,
            paint.as_ref(),
            Some(shape_brush_ts),
            clip_path,
        );
    }
    true
}

/// pyro: the rectangle `path` is (four axis-aligned sides, closed), if it is one.
fn path_rect(path: &BezPath) -> Option<Rect> {
    use kurbo::PathEl;
    let mut points = Vec::with_capacity(5);
    let mut closed = false;
    for el in path.elements() {
        match el {
            PathEl::MoveTo(p) if points.is_empty() => points.push(*p),
            PathEl::LineTo(p) if !closed => points.push(*p),
            PathEl::ClosePath if !closed => closed = true,
            // A move after the close (`to_bez_path` writes one), nothing drawn.
            PathEl::MoveTo(_) if closed => {}
            _ => return None,
        }
    }
    if points.len() == 5 && points[4] == points[0] {
        points.pop();
    }
    if !closed || points.len() != 4 {
        return None;
    }
    for i in 0..4 {
        let (p, q) = (points[i], points[(i + 1) % 4]);
        if p.x != q.x && p.y != q.y {
            return None;
        }
    }
    let rect = Rect::from_points(points[0], points[2]);
    (rect.width() > 0.0 && rect.height() > 0.0).then_some(rect)
}

/// pyro: the shapes of a clip path that has no clip path of its own and whose
/// shapes have none either (groups and text hold paths), each with its clip
/// rule and its transform to the scene; `ts` is the referencing element's user
/// space in the scene. One shape clips with a clip layer.
fn clip_shapes(clip: &usvg::ClipPath, ts: Affine) -> Option<Vec<(Fill, Affine, BezPath)>> {
    fn collect(
        group: &usvg::Group,
        clip_ts: Affine,
        out: &mut Vec<(Fill, Affine, BezPath)>,
    ) -> Option<()> {
        for node in group.children() {
            match node {
                Node::Path(path) => {
                    if !path.is_visible() {
                        continue;
                    }
                    let rule = match path.fill().map(|fill| fill.rule()) {
                        Some(usvg::FillRule::EvenOdd) => Fill::EvenOdd,
                        _ => Fill::NonZero,
                    };
                    out.push((
                        rule,
                        clip_ts * util::to_affine(&path.abs_transform()),
                        util::to_bez_path(path),
                    ));
                }
                Node::Group(g) if g.clip_path().is_none() => collect(g, clip_ts, out)?,
                Node::Text(text) => collect(text.flattened(), clip_ts, out)?,
                Node::Group(_) => return None,
                Node::Image(_) => {}
            }
        }
        Some(())
    }
    if clip.clip_path().is_some() {
        return None;
    }
    let mut out = Vec::new();
    collect(
        clip.root(),
        ts * util::to_affine(&clip.transform()),
        &mut out,
    )?;
    (!out.is_empty()).then_some(out)
}

/// pyro: the scene-space bounds of what a clip path lets through.
fn clip_bounds(clip: &usvg::ClipPath, ts: Affine) -> Rect {
    let clip_ts = ts * util::to_affine(&clip.transform());
    let bbox = clip.root().bounding_box();
    let rect = Rect::new(
        bbox.left() as f64,
        bbox.top() as f64,
        bbox.right() as f64,
        bbox.bottom() as f64,
    );
    let mut bounds = clip_ts.transform_rect_bbox(rect);
    if let Some(nested) = clip.clip_path() {
        bounds = bounds.intersect(clip_bounds(nested, ts));
    }
    bounds
}

/// pyro: the scene-space bounds of what a mask lets through.
fn mask_bounds(mask: &usvg::Mask, ts: Affine) -> Rect {
    let rect = mask.rect();
    let rect = Rect::new(
        rect.left() as f64,
        rect.top() as f64,
        rect.right() as f64,
        rect.bottom() as f64,
    );
    let mut bounds = ts.transform_rect_bbox(rect);
    if let Some(nested) = mask.mask() {
        bounds = bounds.intersect(mask_bounds(nested, ts));
    }
    bounds
}

/// pyro: multiplies what was drawn in the current (isolated) layer by the
/// coverage of `clip`: the union of its shapes (each with its clip rule and
/// clipped by its own clip path), within its own clip path. `rect` is the
/// isolated layer's extent; the `DestIn` layer must cover all of it, since
/// what it leaves out is not cleared.
fn apply_clip<S: PaintScene>(scene: &mut S, clip: &usvg::ClipPath, ts: Affine, rect: Rect) {
    scene.push_layer(
        Fill::NonZero,
        BlendMode::new(Mix::Normal, Compose::DestIn),
        1.0,
        Affine::IDENTITY,
        &rect,
        None,
        None,
    );
    draw_clip_shapes(
        scene,
        clip.root(),
        ts * util::to_affine(&clip.transform()),
        rect,
    );
    scene.pop_layer();
    if let Some(nested) = clip.clip_path() {
        apply_clip(scene, nested, ts, rect);
    }
}

/// pyro: fills each shape in a clip path's `group` opaque. `clip_ts` maps the
/// clip path's root space to the scene.
fn draw_clip_shapes<S: PaintScene>(
    scene: &mut S,
    group: &usvg::Group,
    clip_ts: Affine,
    rect: Rect,
) {
    for node in group.children() {
        match node {
            Node::Path(path) => {
                if !path.is_visible() {
                    continue;
                }
                let rule = match path.fill().map(|fill| fill.rule()) {
                    Some(usvg::FillRule::EvenOdd) => Fill::EvenOdd,
                    _ => Fill::NonZero,
                };
                scene.fill(
                    rule,
                    clip_ts * util::to_affine(&path.abs_transform()),
                    Color::BLACK,
                    None,
                    &util::to_bez_path(path),
                );
            }
            Node::Text(text) => draw_clip_shapes(scene, text.flattened(), clip_ts, rect),
            Node::Group(g) => match g.clip_path() {
                // A shape clipped by its own clip path is drawn in a layer of
                // its own and clipped there.
                Some(own_clip) => {
                    scene.push_layer(
                        Fill::NonZero,
                        Mix::Normal,
                        1.0,
                        Affine::IDENTITY,
                        &rect,
                        None,
                        None,
                    );
                    draw_clip_shapes(scene, g, clip_ts, rect);
                    let g_ts = clip_ts * util::to_affine(&g.abs_transform());
                    apply_clip(scene, own_clip, g_ts, rect);
                    scene.pop_layer();
                }
                None => draw_clip_shapes(scene, g, clip_ts, rect),
            },
            Node::Image(_) => {}
        }
    }
}

/// pyro: multiplies what was drawn in the current (isolated) layer by `mask`:
/// its content (within its rectangle) is rendered offscreen at the scene's
/// resolution over `rect`, turned into coverage by luminance or alpha, and
/// drawn into a `DestIn` layer covering `rect`. A mask on the mask element
/// multiplies the content too (as in resvg).
fn apply_mask<S: PaintScene>(scene: &mut S, mask: &usvg::Mask, ts: Affine, rect: Rect) {
    if let Some(nested) = mask.mask() {
        apply_mask(scene, nested, ts, rect);
    }
    scene.push_layer(
        Fill::NonZero,
        BlendMode::new(Mix::Normal, Compose::DestIn),
        1.0,
        Affine::IDENTITY,
        &rect,
        None,
        None,
    );
    if let Some((image, image_ts)) = mask_image(mask, ts, rect) {
        scene.draw_image(image.as_ref(), image_ts);
    }
    scene.pop_layer();
}

/// pyro: `mask` rendered over the whole-pixel scene rectangle `rect`, as a
/// premultiplied image whose alpha is the mask's coverage, and the transform
/// that draws it there. `None` when the mask lets nothing through.
fn mask_image(mask: &usvg::Mask, ts: Affine, rect: Rect) -> Option<(ImageBrush, Affine)> {
    if !mask.root().has_children() {
        return None;
    }
    let (width, height, scale) = raster_size(rect.width(), rect.height())?;
    // A mask of up to a megapixel is rendered at twice that and averaged down:
    // vello flattens curves to a quarter pixel, which an image of the mask keeps
    // (two descriptions of one circle came out up to 39 levels apart at its edge).
    let ss: u32 = if width as u64 * height as u64 <= 1 << 20 {
        2
    } else {
        1
    };
    // Scene space to the raster's pixels.
    let to_raster = Affine::scale(scale * ss as f64) * Affine::translate((-rect.x0, -rect.y0));
    let mask_rect = mask.rect();
    let mask_rect = Rect::new(
        mask_rect.left() as f64,
        mask_rect.top() as f64,
        mask_rect.right() as f64,
        mask_rect.bottom() as f64,
    );
    let (rw, rh) = (width * ss, height * ss);
    let raster_ctx = Ctx {
        cull: Rect::new(0.0, 0.0, rw as f64, rh as f64),
    };
    let pixels = rasterize(rw, rh, |painter| {
        painter.push_clip_layer(Fill::NonZero, to_raster * ts, &mask_rect);
        let mut no_errors: fn(&mut Scene, &Node) = |_, _| {};
        render_group(
            painter,
            mask.root(),
            Affine::IDENTITY,
            to_raster * ts,
            &mut no_errors,
            &raster_ctx,
        );
        painter.pop_layer();
    });
    let luminance = mask.kind() == usvg::MaskType::Luminance;
    // Premultiplied, so luminance already carries the alpha.
    let coverage = |px: &[u8]| -> u32 {
        if luminance {
            (2125 * px[0] as u32 + 7154 * px[1] as u32 + 721 * px[2] as u32 + 5000) / 10000
        } else {
            px[3] as u32
        }
    };
    let mut out = vec![0u8; width as usize * height as usize * 4];
    let n = ss * ss;
    for y in 0..height as usize {
        for x in 0..width as usize {
            let mut sum = 0;
            for dy in 0..ss as usize {
                let row = (y * ss as usize + dy) * rw as usize;
                for dx in 0..ss as usize {
                    let i = (row + x * ss as usize + dx) * 4;
                    sum += coverage(&pixels[i..i + 4]).min(255);
                }
            }
            out[(y * width as usize + x) * 4 + 3] = ((sum + n / 2) / n) as u8;
        }
    }
    let image = premultiplied_image(out, width, height);
    Some((
        image,
        Affine::translate((rect.x0, rect.y0)) * Affine::scale(1.0 / scale),
    ))
}

/// pyro: the pixel size of an offscreen raster of a `width` x `height` scene
/// area, and the scale from scene to raster pixels (below 1 for an area of
/// more than `MAX_RASTER_PIXELS`).
fn raster_size(width: f64, height: f64) -> Option<(u32, u32, f64)> {
    if !(width >= 1.0 && height >= 1.0) || !width.is_finite() || !height.is_finite() {
        return None;
    }
    let scale = (MAX_RASTER_PIXELS / (width * height)).sqrt().min(1.0);
    let w = (width * scale).ceil().clamp(1.0, 16384.0) as u32;
    let h = (height * scale).ceil().clamp(1.0, 16384.0) as u32;
    Some((w, h, scale))
}

/// pyro: renders `draw` offscreen into a `width` x `height` premultiplied
/// RGBA8 raster, with the same rasterizer as the page. The commands are
/// recorded and replayed (`append_scene`), as the page's are: the replay is
/// what applies filter layers.
fn rasterize(width: u32, height: u32, draw: impl FnOnce(&mut Scene)) -> Vec<u8> {
    use anyrender::ImageRenderer;
    let mut scene = Scene::new();
    draw(&mut scene);
    let mut renderer = VelloCpuImageRenderer::with_threads(width, height, 0);
    let mut pixels = Vec::new();
    renderer.render_to_vec(|painter| painter.append_scene(scene, Affine::IDENTITY), &mut pixels);
    pixels
}

fn premultiplied_image(pixels: Vec<u8>, width: u32, height: u32) -> ImageBrush {
    ImageBrush::new(ImageData {
        data: Blob::new(std::sync::Arc::new(pixels)),
        format: peniko::ImageFormat::Rgba8,
        alpha_type: peniko::ImageAlphaType::AlphaPremultiplied,
        width,
        height,
    })
}

/// pyro: a pattern as a brush: its tile rendered offscreen at the scene's
/// resolution, repeated. `ts` maps the painted path's user space to the scene;
/// the returned transform maps the tile's pixels to that user space.
fn pattern_brush(
    pattern: &usvg::Pattern,
    opacity: usvg::Opacity,
    ts: Affine,
    bounds: Rect,
    ctx: &Ctx,
) -> Option<(ImageBrush, Affine)> {
    let rect = pattern.rect();
    let pattern_ts = util::to_affine(&pattern.transform());
    let [a, b, c, d, _, _] = (ts * pattern_ts).as_coeffs();
    let (sx, sy) = (a.hypot(b), c.hypot(d));
    let (tile_w, tile_h) = (rect.width() as f64, rect.height() as f64);
    if !(sx > 0.0 && sy > 0.0 && sx.is_finite() && sy.is_finite()) {
        return None;
    }
    // An empty shape needs no tile.
    if !(bounds.width() > 0.0 && bounds.height() > 0.0) {
        return None;
    }
    let _ = ctx;
    let (width, height, _) = raster_size(
        (tile_w * sx).round().max(1.0),
        (tile_h * sy).round().max(1.0),
    )?;
    // The tile's content space to its pixels: whole pixels across the tile.
    let content_ts = Affine::scale_non_uniform(width as f64 / tile_w, height as f64 / tile_h);
    let raster_ctx = Ctx {
        cull: Rect::new(0.0, 0.0, width as f64, height as f64),
    };
    let pixels = rasterize(width, height, |painter| {
        let mut no_errors: fn(&mut Scene, &Node) = |_, _| {};
        render_group(
            painter,
            pattern.root(),
            Affine::IDENTITY,
            content_ts,
            &mut no_errors,
            &raster_ctx,
        );
    });
    let image = premultiplied_image(pixels, width, height)
        .with_extend(peniko::Extend::Repeat)
        .with_quality(peniko::ImageQuality::Medium)
        .with_alpha(opacity.get());
    let brush_ts = pattern_ts
        * Affine::translate((rect.x() as f64, rect.y() as f64))
        * Affine::scale_non_uniform(tile_w / width as f64, tile_h / height as f64);
    Some((image, brush_ts))
}

/// The brush of a fill or stroke paint, with its transform; patterns are
/// rendered here (pyro), other paints in `util::to_brush`.
fn brush(
    paint: &usvg::Paint,
    opacity: usvg::Opacity,
    transform: Affine,
    bounds: Rect,
    ctx: &Ctx,
) -> Option<(Paint, Affine)> {
    match paint {
        usvg::Paint::Pattern(pattern) => pattern_brush(pattern, opacity, transform, bounds, ctx)
            .map(|(image, ts)| (Paint::Image(image), ts)),
        paint => util::to_brush(paint, opacity),
    }
}

fn fill<S: PaintScene, F: FnMut(&mut S, &usvg::Node)>(
    scene: &mut S,
    error_handler: &mut F,
    path: &Path,
    transform: Affine,
    local_path: &BezPath,
    node: &Node,
    ctx: &Ctx,
) {
    if let Some(fill) = &path.fill() {
        let bounds = transform.transform_rect_bbox(local_path.bounding_box());
        if let Some((paint, brush_transform)) =
            brush(fill.paint(), fill.opacity(), transform, bounds, ctx)
        {
            scene.fill(
                match fill.rule() {
                    usvg::FillRule::NonZero => Fill::NonZero,
                    usvg::FillRule::EvenOdd => Fill::EvenOdd,
                },
                transform,
                paint.as_ref(),
                Some(brush_transform),
                local_path,
            );
        } else if !matches!(fill.paint(), usvg::Paint::Pattern(_)) {
            error_handler(scene, node);
        }
    }
}

fn stroke<S: PaintScene, F: FnMut(&mut S, &usvg::Node)>(
    scene: &mut S,
    error_handler: &mut F,
    path: &Path,
    transform: Affine,
    local_path: &BezPath,
    node: &Node,
    ctx: &Ctx,
) {
    if let Some(stroke) = &path.stroke() {
        let width = stroke.width().get() as f64;
        let bounds = transform.transform_rect_bbox(local_path.bounding_box().inflate(width, width));
        if let Some((paint, brush_transform)) =
            brush(stroke.paint(), stroke.opacity(), transform, bounds, ctx)
        {
            let conv_stroke = util::to_stroke(stroke);
            scene.stroke(
                &conv_stroke,
                transform,
                paint.as_ref(),
                Some(brush_transform),
                local_path,
            );
        } else if !matches!(stroke.paint(), usvg::Paint::Pattern(_)) {
            error_handler(scene, node);
        }
    }
}
