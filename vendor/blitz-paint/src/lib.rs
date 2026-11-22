//! Paint a [`blitz_dom::BaseDocument`] by pushing [`anyrender`] drawing commands into
//! an impl [`anyrender::PaintScene`].

#![allow(clippy::collapsible_if)]

mod broken_image_icon;
mod caret;
mod capture;
mod blend;
mod bounds;
mod color;
mod debug_overlay;
mod filters;
mod gradient;
mod highlight;
mod kurbo_css;
mod layers;
mod palette;
pub mod reach;
mod render;
mod sizing;
mod text;
pub use text::with_glyph_grid;

use std::collections::HashMap;

use anyrender::{PaintScene, Scene};
use blitz_dom::{BaseDocument, NodeId, util::Color};
use render::BlitzDomPainter;

const FONT_EMBOLDEN_ENABLED: bool = cfg!(any(
    feature = "font-embolden",
    all(feature = "apple-font-embolden", target_os = "macos"),
    all(feature = "apple-font-embolden", target_os = "ios"),
));

/// pyro: glyphs drawn as Chromium draws them on macOS, where Core Text renders text and Skia
/// positions it: outlines unhinted, and each glyph at a quarter of a device pixel along the
/// baseline (Skia's subpixel positions). Elsewhere glyphs keep the hinting and the exact x.
const CORE_TEXT_GLYPHS: bool = cfg!(target_os = "macos");

/// The default color for text selection highlights
const SELECTION_COLOR: Color = Color::from_rgb8(180, 213, 255);

/// Pre-computed `Scene`s for each CustomWidget, keyed by `(document id, node id)`
type CustomWidgetSceneMap = HashMap<(usize, NodeId), Scene>;

/// Paint a [`blitz_dom::BaseDocument`] by pushing drawing commands into
/// an impl [`anyrender::PaintScene`].
///
/// This function assumes that the styles and layout in the [`BaseDocument`] are already
/// resolved. Please ensure that this is the case before trying to paint.
///
/// The implementation of [`PaintScene`] is responsible for handling the commands that are pushed into it.
/// Generally this will involve executing them to draw a rasterized image/texture. But in some cases it may choose to
/// transform them to a vector format (e.g. SVG/PDF) or serialize them in raw form for later use.
pub fn paint_scene(
    scene: &mut impl PaintScene,
    doc: &mut BaseDocument,
    scale: f64,
    width: u32,
    height: u32,
    x_offset: u32,
    y_offset: u32,
) {
    // pyro: screenshots draw flat elements with a 3D transform as Chromium composites them
    // (`BlitzDomPainter::composite_3d`); printed output stays vector.
    paint_scene_inner(scene, doc, scale, width, height, x_offset, y_offset, true, true, true, Some(kurbo::Affine::IDENTITY))
}

/// pyro: [`paint_scene`] for a scene drawn into device pixels with `device` (a screenshot's
/// transform of the page scene: its origin and clip scale). Knowing the pixels, painting leaves
/// out clips that would cut nothing (`BlitzDomPainter::device_pixels`).
#[allow(clippy::too_many_arguments)]
pub fn paint_scene_for_device(
    scene: &mut impl PaintScene,
    doc: &mut BaseDocument,
    scale: f64,
    width: u32,
    height: u32,
    x_offset: u32,
    y_offset: u32,
    device: kurbo::Affine,
) {
    paint_scene_inner(scene, doc, scale, width, height, x_offset, y_offset, true, true, true, Some(device))
}

/// Record isolated named boxes in border-local device pixels. Each nested participant is omitted.
pub fn record_transition_boxes(
    doc: &mut BaseDocument,
    participants: &[NodeId],
    phases: &[(f64,f64)],
    recordings: &mut usize,
    work: &mut u64,
    mut consume: impl FnMut(NodeId, Scene, kurbo::Rect) -> Option<()>,
) -> Option<()> {
    if participants.len() > 256 || doc.tree().len() > 16_384 { return None; }
    if participants.is_empty() { return Some(()); }
    *work = work.checked_sub(doc.tree().len() as u64 + 1)?;
    #[cfg(feature = "svg")]
    let _raster_scope = anyrender_svg::PageRasterScope::enter(&doc.raster_budget);
    doc.ensure_paint_tree();
    let scale = doc.viewport().scale_f64();
    let (width, height) = doc.viewport().window_size;
    let widgets = HashMap::new();
    let named = participants.iter().copied().collect();
    let mut painter = BlitzDomPainter::new(doc, scale, width, height, 0.0, 0.0, &widgets);
    painter.transition_children = Some(&named);
    if phases.len() != participants.len() { return None; }
    pyro_raster::capture::scope(work, |budget| { let mut painter = painter; painter.capture_budget = Some(budget); for (&id,&phase) in participants.iter().zip(phases) {
        if !budget.take(1) { return None; }
        let node = doc.get_node(id)?;
        let size = node.final_layout().size;
        let local = kurbo::Rect::new(phase.0, phase.1, size.width as f64 * scale+phase.0, size.height as f64 * scale+phase.1);
        let parent = kurbo::Affine::translate(phase) * painter.node_transform(node, kurbo::Affine::IDENTITY).inverse();
        if !parent.is_finite() { return None; }
        painter.top_layer_root.set(Some(id));
        *recordings += 1;
        let mut scene = Scene::new();
        painter.render_element(&mut capture::Recording(&mut scene, budget), id, parent, kurbo::Rect::new(-1e7, -1e7, 1e7, 1e7));
        if budget.exhausted.get() { return None; }
        let ink = bounds::scene_bounds(&scene.commands).map_or(local, |bounds| local.union(bounds));
        consume(id, scene, ink.expand())?;
    }
    Some(()) })?
}

/// pyro: HTML-in-Canvas `drawElementImage`: the element `id` and its subtree alone (nothing under,
/// around or above it, no canvas background), its own transform cancelled and its border box's top
/// left at the origin, at `scale` device pixels per CSS px. Returns the scene with the border box's
/// size in device pixels, and whether the paint may hold what a page must not read back
/// ([`subtree_may_taint`]); `None` without such an element or with a transform that cannot be
/// undone. It paints a child of a canvas with content=drawable, which is not painted in place
/// (`render_element_clipped`), as the root of the paint.
pub fn paint_element_subtree(doc: &mut BaseDocument, id: NodeId, scale: f64) -> Option<(Scene, kurbo::Size, bool)> {
    #[cfg(feature = "svg")]
    let _raster_scope = anyrender_svg::PageRasterScope::enter(&doc.raster_budget);
    doc.ensure_paint_tree();
    let (width, height) = doc.viewport().window_size;
    let widgets = HashMap::new();
    let mut painter = BlitzDomPainter::new(doc, scale, width, height, 0.0, 0.0, &widgets);
    painter.privacy = Some(render::Privacy::default());
    let node = doc.get_node(id)?;
    let size = node.final_layout().size;
    let parent = painter.node_transform(node, kurbo::Affine::IDENTITY).inverse();
    if !parent.is_finite() {
        return None;
    }
    painter.top_layer_root.set(Some(id));
    let mut scene = Scene::new();
    painter.render_element(&mut scene, id, parent, kurbo::Rect::new(-1e7, -1e7, 1e7, 1e7));
    let privacy = painter.privacy.take().unwrap_or_default();
    let taints = privacy.unclassified.get() || subtree_may_taint(doc, id, &scene, &privacy.allowed.borrow());
    Some((scene, kurbo::Size::new(size.width as f64 * scale, size.height as f64 * scale), taints))
}

/// pyro: HTML-in-Canvas privacy: whether the paint of `id`'s subtree (`scene`) may show content a
/// page must not read back from a canvas, after `render::Privacy` left out the images of other
/// origins it can classify: any other image (a video frame, a nested canvas, an SVG `<image>`, an
/// `feImage`, one whose pixels are not among the `allowed` same-origin ones), a border, mask or
/// list-style image of another origin, a nested document, an embedded plugin or media element, or a
/// visited link (whose colours reveal history). The canvas drawn into is then no longer
/// origin-clean.
fn subtree_may_taint(doc: &BaseDocument, id: NodeId, scene: &Scene, allowed: &[u64]) -> bool {
    use anyrender::Paint;
    use anyrender::filters::FilterEffect;
    use anyrender::recording::RenderCommand;
    let image = |paint: &Paint| match paint {
        Paint::Solid(_) | Paint::Gradient(_) => false,
        Paint::Image(brush) => !allowed.contains(&brush.image.data.id()),
        _ => true,
    };
    let filter_image = |filter: &Option<std::sync::Arc<anyrender::Filter>>| {
        filter.as_ref().is_some_and(|f| f.nodes().iter().any(|node| matches!(node.effect, FilterEffect::Image(_))))
    };
    let in_scene = scene.commands.iter().any(|command| match command {
        RenderCommand::Fill(fill) => image(&fill.brush),
        RenderCommand::Stroke(stroke) => image(&stroke.brush),
        RenderCommand::GlyphRun(run) => image(&run.brush),
        RenderCommand::PushLayer(layer) => filter_image(&layer.filter) || filter_image(&layer.backdrop_filter),
        _ => false,
    });
    if in_scene {
        return true;
    }
    let mut stack = vec![id];
    while let Some(n) = stack.pop() {
        let Some(node) = doc.get_node(n) else { continue };
        if doc.is_visited_link(n) {
            return true;
        }
        if let Some(element) = node.element_data() {
            if element.sub_doc_data().is_some()
                || matches!(&*element.name.local, "iframe" | "frame" | "embed" | "object" | "video" | "audio")
            {
                return true;
            }
            let foreign = |images: &[Option<blitz_dom::node::ImageResourceData>]| {
                // An SVG image paints vector commands the scene scan does not see as an image: it
                // taints whatever its URL (a same-origin URL may redirect to another origin).
                images.iter().flatten().any(|image| {
                    !render::Privacy::same_origin(doc.url(), &image.url)
                        || !matches!(image.image, blitz_dom::node::ImageData::Raster(_))
                })
            };
            if foreign(&element.border_images) || foreign(&element.mask_images) || foreign(&element.list_style_images) {
                return true;
            }
        }
        stack.extend(node.children.iter().copied());
        if let Some(children) = node.layout_children.borrow().as_ref() {
            stack.extend(children.iter().copied());
        }
    }
    false
}

/// pyro: the top Chromium paints a line of text at, in CSS px of the document: the line's unrounded
/// top (`origin`, the unrounded top of its layout in the document in CSS px, plus the line's
/// `unquantized_top` in layout units, `scale` to a CSS px) in Blink's layout units of 1/64 px,
/// rounded half up to a whole pixel.
pub fn snapped_line_top(origin: f64, unquantized_top: f32, scale: f64) -> f64 {
    // The small term keeps a sum of floats that is a 64th short of one by noise on it.
    let top = ((origin + unquantized_top as f64 / scale) * 64.0 + 1e-3).floor() / 64.0;
    (top + 0.5).floor()
}

/// [`paint_scene`], with background colors and images left out when
/// `backgrounds` is false (printing without `printBackground`).
///
/// pyro: text is drawn at its layout-unit x, as Chromium positions it, not at the whole
/// pixel its box is painted at (`fractional_text_x`).
#[allow(clippy::too_many_arguments)]
pub fn paint_scene_with(
    scene: &mut impl PaintScene,
    doc: &mut BaseDocument,
    scale: f64,
    width: u32,
    height: u32,
    x_offset: u32,
    y_offset: u32,
    backgrounds: bool,
) {
    paint_scene_inner(scene, doc, scale, width, height, x_offset, y_offset, backgrounds, true, false, None)
}

/// [`paint_scene_with`] for print: text at its layout-unit x.
#[allow(clippy::too_many_arguments)]
pub fn paint_scene_for_print(
    scene: &mut impl PaintScene,
    doc: &mut BaseDocument,
    scale: f64,
    width: u32,
    height: u32,
    x_offset: u32,
    y_offset: u32,
    backgrounds: bool,
) {
    paint_scene_inner(scene, doc, scale, width, height, x_offset, y_offset, backgrounds, true, false, None)
}

#[allow(clippy::too_many_arguments)]
fn paint_scene_inner(
    scene: &mut impl PaintScene,
    doc: &mut BaseDocument,
    scale: f64,
    width: u32,
    height: u32,
    x_offset: u32,
    y_offset: u32,
    backgrounds: bool,
    fractional_text_x: bool,
    composite_3d: bool,
    device_pixels: Option<kurbo::Affine>,
) {
    // Painting walks `paint_children` and the stacking contexts, which a
    // resolve only marks stale.
    doc.ensure_paint_tree();

    // Run `.paint()` on every custom widget in the document (and all subdocuments) ahead of time.
    // This helps us avoid borrow-checker issues as we recurse down the tree (`.paint()` require `&mut self`).
    //
    // TODO: Take widget and sub-document visibility into account
    #[allow(unused_mut)]
    let mut custom_widget_scenes: CustomWidgetSceneMap = HashMap::new();
    #[cfg(feature = "custom-widget")]
    build_custom_widget_scenes(&mut custom_widget_scenes, doc, scene, scale);

    let mut generator = BlitzDomPainter::new(
        doc,
        scale,
        width,
        height,
        x_offset as f64,
        y_offset as f64,
        &custom_widget_scenes,
    );
    generator.backgrounds = backgrounds;
    generator.fractional_text_x = fractional_text_x;
    generator.composite_3d = composite_3d;
    generator.device_pixels = device_pixels;
    generator.paint_scene(scene);

    // println!(
    //     "Rendered using {} clips (depth: {}) (wanted: {})",
    //     CLIPS_USED.load(atomic::Ordering::SeqCst),
    //     CLIP_DEPTH_USED.load(atomic::Ordering::SeqCst),
    //     CLIPS_WANTED.load(atomic::Ordering::SeqCst)
    // );
}

/// One glyph run [`paint_scene_recording_text`] drew.
#[derive(Clone, Debug)]
pub struct TextRun {
    /// The node whose style the run has (the innermost element around the text).
    pub node: NodeId,
    /// The number of glyphs and their [`glyphs_fingerprint`] as drawn: identifies the run's
    /// `draw_glyphs` call.
    pub glyphs: usize,
    pub fingerprint: u64,
    /// The source text the run shows and each glyph's byte range in it (a ligature's range
    /// spans all the characters it stands for). Empty when the source is unknown.
    pub text: String,
    pub glyph_text: Vec<std::ops::Range<u32>>,
    /// The run's content area (ascent to descent), in the scene's coordinates.
    pub bounds: kurbo::Rect,
}

impl TextRun {
    /// The run's glyphs in `kept` (indices in drawing order; pyro: `text-overflow` draws a
    /// part), whose atoms span `extent` (layout x).
    pub(crate) fn new(
        run: &parley::GlyphRun<'_, blitz_dom::node::TextBrush>,
        source: Option<&str>,
        transform: kurbo::Affine,
        kept: std::ops::Range<usize>,
        extent: Option<(f32, f32)>,
    ) -> TextRun {
        let mut count = 0;
        let fingerprint = glyphs_fingerprint(run.positioned_glyphs().skip(kept.start).take(kept.len()).map(|g| {
            count += 1;
            (g.id, g.x, g.y)
        }));
        let mut text = String::new();
        let mut glyph_text = Vec::new();
        if let Some(source) = source {
            let mut first = 0;
            let atoms: Vec<_> = run
                .atom_texts()
                .filter(|(_, n)| {
                    let at = first;
                    first += n;
                    at >= kept.start && at < kept.end
                })
                .collect();
            let start = atoms.iter().map(|(r, _)| r.start).min().unwrap_or(0);
            let end = atoms.iter().map(|(r, _)| r.end).max().unwrap_or(0);
            if let Some(t) = source.get(start..end) {
                text.push_str(t);
                for (r, n) in atoms {
                    let r = (r.start - start) as u32..(r.end - start) as u32;
                    glyph_text.extend(std::iter::repeat_n(r, n));
                }
            }
            if glyph_text.len() != count {
                text.clear();
                glyph_text.clear();
            }
        }
        let metrics = run.run().font_metrics();
        let (x0, x1) = extent.unwrap_or((run.offset(), run.offset() + run.advance()));
        let baseline = run.baseline() as f64;
        let rect = kurbo::Rect::new(
            x0 as f64,
            baseline - metrics.ascent as f64,
            x1 as f64,
            baseline + metrics.descent as f64,
        );
        TextRun {
            node: run.style().brush.id,
            glyphs: count,
            fingerprint,
            text,
            glyph_text,
            bounds: transform.transform_rect_bbox(rect),
        }
    }
}

/// A hash of glyphs as drawn (id, x, y): [`TextRun::fingerprint`] of a run's glyphs. Two runs
/// that start alike (table cells of dates, amounts) tell apart by the rest of their glyphs.
pub fn glyphs_fingerprint(glyphs: impl IntoIterator<Item = (u32, f32, f32)>) -> u64 {
    // FxHash's step: a run is only compared with the next recorded one.
    let mut hash = 0u64;
    for (id, x, y) in glyphs {
        for word in [u64::from(id), (u64::from(x.to_bits()) << 32) | u64::from(y.to_bits())] {
            hash = (hash.rotate_left(5) ^ word).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
        }
    }
    hash
}

/// [`paint_scene_with`], also returning the glyph runs of the document's text in the order
/// they were drawn (PDF output maps them back to their text and elements).
#[allow(clippy::too_many_arguments)]
pub fn paint_scene_recording_text(
    scene: &mut impl PaintScene,
    doc: &mut BaseDocument,
    scale: f64,
    width: u32,
    height: u32,
    x_offset: u32,
    y_offset: u32,
    backgrounds: bool,
) -> Vec<TextRun> {
    // Painting walks `paint_children` and the stacking contexts, which a
    // resolve only marks stale.
    doc.ensure_paint_tree();
    #[allow(unused_mut)]
    let mut custom_widget_scenes: CustomWidgetSceneMap = HashMap::new();
    #[cfg(feature = "custom-widget")]
    build_custom_widget_scenes(&mut custom_widget_scenes, doc, scene, scale);
    let mut generator = BlitzDomPainter::new(
        doc,
        scale,
        width,
        height,
        x_offset as f64,
        y_offset as f64,
        &custom_widget_scenes,
    );
    generator.backgrounds = backgrounds;
    generator.fractional_text_x = true;
    generator.draw_text_context.borrow_mut().runs = Some(Vec::new());
    generator.paint_scene(scene);
    let runs = generator.draw_text_context.borrow_mut().runs.take();
    runs.unwrap_or_default()
}

#[cfg(feature = "custom-widget")]
fn build_custom_widget_scenes(
    custom_widget_scenes: &mut CustomWidgetSceneMap,
    doc: &mut BaseDocument,
    render_ctx: &mut impl anyrender::RenderContext,
    scale: f64,
) {
    let doc_id = doc.id();

    // Process scenes for every custom widget in the document
    let custom_widget_node_ids = doc.custom_widget_node_ids();
    for node_id in custom_widget_node_ids.into_iter() {
        if let Some(scene) = process_custom_widget_node(doc, render_ctx, node_id, scale) {
            custom_widget_scenes.insert((doc_id, node_id), scene);
        }
    }

    // Recurse into sub documents
    let sub_document_node_ids = doc.sub_document_node_ids();
    for node_id in sub_document_node_ids.into_iter() {
        if let Some(sub_doc) = doc.get_node_mut(node_id).and_then(|node| node.subdoc_mut()) {
            let mut inner = sub_doc.inner_mut();
            build_custom_widget_scenes(custom_widget_scenes, &mut inner, render_ctx, scale);
        }
    }
}

#[cfg(feature = "custom-widget")]
fn process_custom_widget_node(
    doc: &mut BaseDocument,
    render_ctx: &mut impl anyrender::RenderContext,
    node_id: NodeId,
    scale: f64,
) -> Option<Scene> {
    use blitz_dom::node::{CustomWidgetStatus, ProxyRenderContext};

    let node = doc.get_node_mut(node_id)?;
    let width = (node.final_layout().size.width as f64 * scale) as u32;
    let height = (node.final_layout().size.height as f64 * scale) as u32;

    if width == 0 || height == 0 {
        return None;
    }

    let style = (*node.stylo_element_data().primary_styles()?).clone();
    let element = node.data.downcast_element_mut()?;
    let widget_data = element.custom_widget_data_mut()?;

    let mut render_ctx = ProxyRenderContext {
        inner: render_ctx,
        resource_ids: &mut widget_data.active_resource_ids,
    };

    if widget_data.status == CustomWidgetStatus::Suspended {
        widget_data.widget.can_create_surfaces(&mut render_ctx);
        widget_data.status = CustomWidgetStatus::Active;
    }

    let widget_scene = widget_data
        .widget
        .paint(&mut render_ctx, &style, width, height, scale);

    Some(widget_scene)
}

/// pyro: how many lines of inline layouts the painter has built inline box fragments for on this
/// thread: a measure of its work, which tests bound (a wall clock flakes under load).
pub fn inline_lines_visited() -> usize {
    render::INLINE_LINES_VISITED.with(|visited| visited.get())
}
