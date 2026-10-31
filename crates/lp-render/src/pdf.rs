//! PDF output: the recorded display list ([`Scene`]) replayed into krilla,
//! one PDF page per page box. Vector paths, gradients, images and text
//! (embedded, subset fonts) stay vector; what the PDF model has no
//! equivalent for is skipped and counted in [`PdfStats`].
//!
//! Text is written as glyph ids with positions, as Blitz shaped it. The
//! display list carries no source text, so the text a glyph stands for is
//! recovered from the font's cmap (reverse lookup): plain text extracts and
//! copies, ligatures and glyphs reached only through GSUB do not.

use std::collections::HashMap;

use anyrender::recording::{RenderCommand, Scene};
use anyrender::Paint;
use krilla::color::rgb;
use krilla::geom::{Path, PathBuilder, Point as KPoint, Size, Transform};
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{
    Fill as KFill, FillRule, LineCap, LineJoin, LinearGradient, RadialGradient, SpreadMethod, Stop, Stroke as KStroke,
    StrokeDash, SweepGradient,
};
use krilla::surface::Surface;
use krilla::text::{Font, GlyphId, KrillaGlyph};
use kurbo::{Affine, BezPath, PathEl, Rect, Shape};
use peniko::color::{AlphaColor, Srgb};
use peniko::{Extend, Fill, FontData, Gradient, GradientKind, ImageAlphaType, ImageBrush, ImageFormat, Mix, Style};

/// CSS px to PDF points.
pub const PX_TO_PT: f64 = 0.75;

/// What the replay wrote and what it had to leave out.
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfStats {
    pub fills: u32,
    pub strokes: u32,
    pub glyph_runs: u32,
    pub glyphs: u32,
    /// Glyphs with no cmap entry: drawn, but no text behind them.
    pub glyphs_without_text: u32,
    pub images: u32,
    /// Zero-area or fully transparent fills: not written.
    pub degenerate_skipped: u32,
    /// Commands outside the page they were replayed for.
    pub culled: u32,
    /// Blurred shadows: not drawn.
    pub box_shadows_skipped: u32,
    /// Layer filters and backdrop filters: content drawn unfiltered.
    pub filters_skipped: u32,
    /// Compositing other than source-over: drawn as source-over.
    pub compose_skipped: u32,
    /// Variable-font runs: drawn with the font's default instance.
    pub variable_runs: u32,
    /// Synthetic bold runs: drawn without emboldening.
    pub embolden_skipped: u32,
    /// Fonts krilla could not read: their runs are not drawn.
    pub fonts_failed: u32,
    /// `Paint::Resource` / `Paint::Custom` brushes: not drawn.
    pub brushes_skipped: u32,
}

struct FontEntry {
    font: Font,
    /// Glyph id to the character it maps from (the lowest code point).
    chars: HashMap<u32, char>,
}

/// Fonts and images converted for krilla, kept across documents.
#[derive(Default)]
pub struct PdfCache {
    fonts: HashMap<(u64, u32), Option<FontEntry>>,
    images: HashMap<u64, Option<krilla::image::Image>>,
}

impl PdfCache {
    fn font(&mut self, data: &FontData) -> Option<&FontEntry> {
        self.fonts
            .entry((data.data.id(), data.index))
            .or_insert_with(|| {
                let blob = data.data.clone();
                let bytes: std::sync::Arc<dyn AsRef<[u8]> + Send + Sync> = std::sync::Arc::new(blob);
                let font = Font::new(krilla::Data::from(bytes), data.index)?;
                let mut chars = HashMap::new();
                use skrifa::MetadataProvider;
                if let Ok(f) = skrifa::FontRef::from_index(data.data.as_ref(), data.index) {
                    for (cp, gid) in f.charmap().mappings() {
                        if let Some(c) = char::from_u32(cp) {
                            chars.entry(gid.to_u32()).or_insert(c);
                        }
                    }
                }
                Some(FontEntry { font, chars })
            })
            .as_ref()
    }

    fn image(&mut self, brush: &ImageBrush) -> Option<krilla::image::Image> {
        let image = &brush.image;
        self.images
            .entry(image.data.id())
            .or_insert_with(|| {
                let (w, h) = (image.width, image.height);
                let src = image.data.data();
                if w == 0 || h == 0 || src.len() < (w * h * 4) as usize {
                    return None;
                }
                let mut rgba = src[..(w * h * 4) as usize].to_vec();
                for px in rgba.chunks_exact_mut(4) {
                    if image.format == ImageFormat::Bgra8 {
                        px.swap(0, 2);
                    }
                    if image.alpha_type == ImageAlphaType::AlphaPremultiplied && px[3] != 0 && px[3] != 255 {
                        let a = px[3] as u32;
                        for c in &mut px[..3] {
                            *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                        }
                    }
                }
                Some(krilla::image::Image::from_rgba8(rgba, w, h))
            })
            .clone()
    }
}

/// Writes `scene` (device px, document coordinates) as a PDF with one page
/// per rectangle of `pages`; `scale` converts device px to points.
pub fn write(scene: &Scene, pages: &[Rect], scale: f64, cache: &mut PdfCache) -> Result<(Vec<u8>, PdfStats), String> {
    let mut stats = PdfStats::default();
    let mut doc = krilla::Document::new();
    for page in pages {
        let (w, h) = ((page.width() * scale) as f32, (page.height() * scale) as f32);
        let settings = PageSettings::from_wh(w.max(1.0), h.max(1.0)).ok_or("bad page size")?;
        let mut p = doc.start_page_with(settings);
        let mut s = p.surface();
        s.push_transform(&Transform::from_scale(scale as f32, scale as f32));
        s.push_transform(&Transform::from_translate(-page.x0 as f32, -page.y0 as f32));
        if let Some(clip) = rect_path(*page) {
            s.push_clip_path(&clip, &FillRule::NonZero);
        }
        Replay { s: &mut s, page: *page, cache, stats: &mut stats, layers: Vec::new() }.run(scene);
        s.pop();
        s.pop();
        s.pop();
        s.finish();
        p.finish();
    }
    let bytes = doc.finish().map_err(|e| format!("krilla: {e:?}"))?;
    Ok((bytes, stats))
}

enum Layer {
    Clip { path: Option<Path>, rule: FillRule },
    Group { path: Option<Path>, rule: FillRule, alpha: f32, blend: krilla::blend::BlendMode },
}

/// A layer of the display list; pushed to krilla only once something is
/// drawn inside it on this page (most layers of a long document are on
/// other pages).
struct Pending {
    layer: Layer,
    /// Surface pushes made for it (`None`: not pushed yet).
    pushed: Option<u32>,
}

struct Replay<'a, 'b> {
    s: &'a mut Surface<'b>,
    page: Rect,
    cache: &'a mut PdfCache,
    stats: &'a mut PdfStats,
    layers: Vec<Pending>,
}

impl Replay<'_, '_> {
    fn run(&mut self, scene: &Scene) {
        for cmd in &scene.commands {
            match cmd {
                RenderCommand::PushClipLayer(c) => self.layers.push(Pending {
                    layer: Layer::Clip { path: to_path(&c.clip, c.transform), rule: rule(c.fill) },
                    pushed: None,
                }),
                RenderCommand::PushLayer(c) => {
                    if c.filter.is_some() || c.backdrop_filter.is_some() {
                        self.stats.filters_skipped += 1;
                    }
                    if c.blend.compose != peniko::Compose::SrcOver {
                        self.stats.compose_skipped += 1;
                    }
                    self.layers.push(Pending {
                        layer: Layer::Group {
                            path: to_path(&c.clip, c.transform),
                            rule: rule(c.fill),
                            alpha: c.alpha,
                            blend: blend(c.blend.mix),
                        },
                        pushed: None,
                    })
                }
                RenderCommand::PopLayer => {
                    if let Some(Pending { pushed: Some(n), .. }) = self.layers.pop() {
                        for _ in 0..n {
                            self.s.pop();
                        }
                    }
                }
                RenderCommand::Fill(c) => {
                    if !self.visible(c.transform.transform_rect_bbox(c.shape.bounding_box())) {
                        continue;
                    }
                    // Zero-area shapes (the ring of a zero-width border) and
                    // invisible colors: a PDF rasterizer paints every pixel a
                    // fill touches, so these would show as hairlines.
                    if c.shape.area().abs() < 1e-3 || matches!(&c.brush, Paint::Solid(col) if col.components[3] == 0.0) {
                        self.stats.degenerate_skipped += 1;
                        continue;
                    }
                    self.stats.fills += 1;
                    self.fill(&c.shape, c.transform, c.fill, &c.brush, c.brush_transform);
                }
                RenderCommand::Stroke(c) => {
                    let bounds = c.shape.bounding_box().inflate(c.style.width, c.style.width);
                    if !self.visible(c.transform.transform_rect_bbox(bounds)) {
                        continue;
                    }
                    let Some(paint) = self.paint(&c.brush, c.brush_transform) else { continue };
                    let Some(path) = to_path(&c.shape, Affine::IDENTITY) else { continue };
                    self.stats.strokes += 1;
                    self.flush();
                    let stroke = KStroke { opacity: paint.1, ..stroke(&c.style, paint.0) };
                    self.with_transform(c.transform, |s| {
                        s.set_fill(None);
                        s.set_stroke(Some(stroke));
                        s.draw_path(&path);
                        s.set_stroke(None);
                    });
                }
                RenderCommand::GlyphRun(c) => {
                    if c.glyphs.is_empty() {
                        continue;
                    }
                    let size = c.font_size as f64;
                    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
                    for g in &c.glyphs {
                        x0 = x0.min(g.x as f64);
                        y0 = y0.min(g.y as f64);
                        x1 = x1.max(g.x as f64);
                        y1 = y1.max(g.y as f64);
                    }
                    let bounds = Rect::new(x0 - size, y0 - size * 1.5, x1 + size * 2.0, y1 + size);
                    if !self.visible(c.transform.transform_rect_bbox(bounds)) {
                        continue;
                    }
                    self.glyph_run(c);
                }
                RenderCommand::BoxShadow(c) => {
                    if self.visible(c.transform.transform_rect_bbox(c.rect.inflate(c.radius, c.radius))) {
                        self.stats.box_shadows_skipped += 1;
                    }
                }
            }
        }
        while let Some(p) = self.layers.pop() {
            for _ in 0..p.pushed.unwrap_or(0) {
                self.s.pop();
            }
        }
    }

    fn visible(&mut self, bounds: Rect) -> bool {
        let hit = bounds.x1 >= self.page.x0
            && bounds.x0 <= self.page.x1
            && bounds.y1 >= self.page.y0
            && bounds.y0 <= self.page.y1;
        if !hit {
            self.stats.culled += 1;
        }
        hit
    }

    /// Pushes the layers nothing was drawn in yet.
    fn flush(&mut self) {
        for p in self.layers.iter_mut().filter(|p| p.pushed.is_none()) {
            let mut n = 0;
            match &p.layer {
                Layer::Clip { path, rule } => {
                    // An empty clip clips everything: a zero-size rect.
                    let empty = path.is_none().then(|| rect_path(Rect::new(0.0, 0.0, 0.0, 0.0))).flatten();
                    if let Some(path) = path.as_ref().or(empty.as_ref()) {
                        self.s.push_clip_path(path, rule);
                        n += 1;
                    }
                }
                Layer::Group { path, rule, alpha, blend } => {
                    if let Some(path) = path {
                        self.s.push_clip_path(path, rule);
                        n += 1;
                    }
                    if *blend != krilla::blend::BlendMode::Normal {
                        self.s.push_blend_mode(*blend);
                        n += 1;
                    }
                    if *alpha < 1.0 {
                        self.s.push_opacity(NormalizedF32::new(alpha.max(0.0)).unwrap_or(NormalizedF32::ZERO));
                        n += 1;
                    }
                }
            }
            p.pushed = Some(n);
        }
    }

    fn with_transform(&mut self, t: Affine, f: impl FnOnce(&mut Surface<'_>)) {
        if t == Affine::IDENTITY {
            f(self.s);
        } else {
            self.s.push_transform(&transform(t));
            f(self.s);
            self.s.pop();
        }
    }

    /// The brush as a krilla paint and its opacity.
    fn paint(&mut self, brush: &Paint, brush_transform: Option<Affine>) -> Option<(krilla::paint::Paint, NormalizedF32)> {
        match brush {
            Paint::Solid(c) => {
                let c = c.to_rgba8();
                Some((rgb::Color::new(c.r, c.g, c.b).into(), norm(c.a as f32 / 255.0)))
            }
            Paint::Gradient(g) => Some((gradient(g, brush_transform.unwrap_or(Affine::IDENTITY)), NormalizedF32::ONE)),
            _ => {
                self.stats.brushes_skipped += 1;
                None
            }
        }
    }

    fn fill(&mut self, shape: &BezPath, t: Affine, fill: Fill, brush: &Paint, brush_transform: Option<Affine>) {
        let Some(path) = to_path(shape, Affine::IDENTITY) else { return };
        if let Paint::Image(img) = brush {
            let Some(image) = self.cache.image(img) else {
                self.stats.brushes_skipped += 1;
                return;
            };
            self.stats.images += 1;
            self.flush();
            let bt = brush_transform.unwrap_or(Affine::IDENTITY);
            let (w, h) = (img.image.width as f64, img.image.height as f64);
            // Tiles of a repeated image covering the shape (in image space).
            let tiles = {
                let area = bt.inverse().transform_rect_bbox(shape.bounding_box());
                let range = |repeat: bool, lo: f64, hi: f64, step: f64| {
                    if repeat && step > 0.0 {
                        ((lo / step).floor() as i64, ((hi / step).ceil() as i64).max((lo / step).floor() as i64 + 1))
                    } else {
                        (0, 1)
                    }
                };
                let (xs, xe) = range(img.sampler.x_extend == Extend::Repeat, area.x0, area.x1, w);
                let (ys, ye) = range(img.sampler.y_extend == Extend::Repeat, area.y0, area.y1, h);
                let mut tiles = Vec::new();
                'outer: for ty in ys..ye {
                    for tx in xs..xe {
                        tiles.push((tx as f64 * w, ty as f64 * h));
                        if tiles.len() >= 2000 {
                            break 'outer;
                        }
                    }
                }
                tiles
            };
            let alpha = img.sampler.alpha;
            self.with_transform(t, |s| {
                s.push_clip_path(&path, &rule(fill));
                s.push_transform(&transform(bt));
                if alpha < 1.0 {
                    s.push_opacity(norm(alpha));
                }
                let size = Size::from_wh(w as f32, h as f32);
                for (x, y) in tiles {
                    s.push_transform(&Transform::from_translate(x as f32, y as f32));
                    if let Some(size) = size {
                        s.draw_image(image.clone(), size);
                    }
                    s.pop();
                }
                if alpha < 1.0 {
                    s.pop();
                }
                s.pop();
                s.pop();
            });
            return;
        }
        let Some((paint, opacity)) = self.paint(brush, brush_transform) else { return };
        self.flush();
        self.with_transform(t, |s| {
            s.set_stroke(None);
            s.set_fill(Some(KFill { paint, opacity, rule: rule(fill) }));
            s.draw_path(&path);
        });
    }

    fn glyph_run(&mut self, c: &anyrender::recording::GlyphRunCommand) {
        if !c.normalized_coords.is_empty() {
            self.stats.variable_runs += 1;
        }
        if c.embolden != kurbo::Vec2::ZERO {
            self.stats.embolden_skipped += 1;
        }
        let Some((paint, opacity)) = self.paint(&c.brush, None) else { return };
        let opacity = norm(opacity.get() * c.brush_alpha);
        let Some(entry) = self.cache.font(&c.font_data) else {
            self.stats.fonts_failed += 1;
            return;
        };
        let font = entry.font.clone();
        let size = c.font_size;
        let mut text = String::new();
        let mut glyphs = Vec::with_capacity(c.glyphs.len());
        let (gx, gy) = (c.glyphs[0].x, c.glyphs[0].y);
        for (i, g) in c.glyphs.iter().enumerate() {
            let start = text.len();
            match entry.chars.get(&g.id) {
                Some(ch) => text.push(*ch),
                None => self.stats.glyphs_without_text += 1,
            }
            let advance = c.glyphs.get(i + 1).map_or(0.0, |n| n.x - g.x);
            glyphs.push(KrillaGlyph::new(
                GlyphId::new(g.id),
                advance / size,
                0.0,
                -(g.y - gy) / size,
                0.0,
                start..text.len(),
                None,
            ));
        }
        self.stats.glyph_runs += 1;
        self.stats.glyphs += glyphs.len() as u32;
        self.flush();
        let style = c.style.clone();
        let glyph_transform = c.glyph_transform;
        self.with_transform(c.transform, |s| {
            match &style {
                Style::Fill(f) => {
                    s.set_stroke(None);
                    s.set_fill(Some(KFill { paint, opacity, rule: rule(*f) }));
                }
                Style::Stroke(st) => {
                    s.set_fill(None);
                    s.set_stroke(Some(KStroke { opacity, ..stroke(st, paint) }));
                }
            }
            match glyph_transform {
                // Per-glyph transform (synthetic italic): one glyph at a time.
                Some(gt) => {
                    for (g, kg) in c.glyphs.iter().zip(&glyphs) {
                        s.push_transform(&transform(Affine::translate((g.x as f64, g.y as f64)) * gt));
                        let one = KrillaGlyph { y_offset: 0.0, ..kg.clone() };
                        s.draw_glyphs(KPoint::from_xy(0.0, 0.0), &[one], font.clone(), &text, size, false);
                        s.pop();
                    }
                }
                None => s.draw_glyphs(KPoint::from_xy(gx, gy), &glyphs, font, &text, size, false),
            }
            s.set_stroke(None);
        });
    }
}

fn norm(v: f32) -> NormalizedF32 {
    NormalizedF32::new(v.clamp(0.0, 1.0)).unwrap_or(NormalizedF32::ONE)
}

fn rule(f: Fill) -> FillRule {
    match f {
        Fill::NonZero => FillRule::NonZero,
        Fill::EvenOdd => FillRule::EvenOdd,
    }
}

fn transform(a: Affine) -> Transform {
    let [sx, ky, kx, sy, tx, ty] = a.as_coeffs();
    Transform::from_row(sx as f32, ky as f32, kx as f32, sy as f32, tx as f32, ty as f32)
}

fn rect_path(r: Rect) -> Option<Path> {
    let mut b = PathBuilder::new();
    b.push_rect(krilla::geom::Rect::from_ltrb(r.x0 as f32, r.y0 as f32, r.x1.max(r.x0 + 0.001) as f32, r.y1.max(r.y0 + 0.001) as f32)?);
    b.finish()
}

fn to_path(p: &BezPath, t: Affine) -> Option<Path> {
    let mut b = PathBuilder::new();
    let id = t == Affine::IDENTITY;
    let pt = |p: kurbo::Point| {
        let p = if id { p } else { t * p };
        (p.x as f32, p.y as f32)
    };
    for el in p.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                let (x, y) = pt(p);
                b.move_to(x, y)
            }
            PathEl::LineTo(p) => {
                let (x, y) = pt(p);
                b.line_to(x, y)
            }
            PathEl::QuadTo(p1, p) => {
                let ((x1, y1), (x, y)) = (pt(p1), pt(p));
                b.quad_to(x1, y1, x, y)
            }
            PathEl::CurveTo(p1, p2, p) => {
                let ((x1, y1), (x2, y2), (x, y)) = (pt(p1), pt(p2), pt(p));
                b.cubic_to(x1, y1, x2, y2, x, y)
            }
            PathEl::ClosePath => b.close(),
        }
    }
    b.finish()
}

fn stroke(st: &kurbo::Stroke, paint: krilla::paint::Paint) -> KStroke {
    KStroke {
        paint,
        width: st.width as f32,
        miter_limit: st.miter_limit as f32,
        line_cap: match st.start_cap {
            kurbo::Cap::Butt => LineCap::Butt,
            kurbo::Cap::Round => LineCap::Round,
            kurbo::Cap::Square => LineCap::Square,
        },
        line_join: match st.join {
            kurbo::Join::Bevel => LineJoin::Bevel,
            kurbo::Join::Miter => LineJoin::Miter,
            kurbo::Join::Round => LineJoin::Round,
        },
        opacity: NormalizedF32::ONE,
        dash: (!st.dash_pattern.is_empty()).then(|| StrokeDash {
            array: st.dash_pattern.iter().map(|d| *d as f32).collect(),
            offset: st.dash_offset as f32,
        }),
    }
}

fn blend(mix: Mix) -> krilla::blend::BlendMode {
    use krilla::blend::BlendMode as B;
    #[allow(deprecated)]
    match mix {
        Mix::Multiply => B::Multiply,
        Mix::Screen => B::Screen,
        Mix::Overlay => B::Overlay,
        Mix::Darken => B::Darken,
        Mix::Lighten => B::Lighten,
        Mix::ColorDodge => B::ColorDodge,
        Mix::ColorBurn => B::ColorBurn,
        Mix::HardLight => B::HardLight,
        Mix::SoftLight => B::SoftLight,
        Mix::Difference => B::Difference,
        Mix::Exclusion => B::Exclusion,
        Mix::Hue => B::Hue,
        Mix::Saturation => B::Saturation,
        Mix::Color => B::Color,
        Mix::Luminosity => B::Luminosity,
        _ => B::Normal,
    }
}

fn gradient(g: &Gradient, bt: Affine) -> krilla::paint::Paint {
    let stops: Vec<Stop> = g
        .stops
        .iter()
        .map(|s| {
            let c: AlphaColor<Srgb> = s.color.to_alpha_color();
            let c = c.to_rgba8();
            Stop {
                offset: norm(s.offset),
                color: rgb::Color::new(c.r, c.g, c.b).into(),
                opacity: norm(c.a as f32 / 255.0),
            }
        })
        .collect();
    let spread_method = match g.extend {
        Extend::Pad => SpreadMethod::Pad,
        Extend::Repeat => SpreadMethod::Repeat,
        Extend::Reflect => SpreadMethod::Reflect,
    };
    let transform = transform(bt);
    match g.kind {
        GradientKind::Linear(l) => LinearGradient {
            x1: l.start.x as f32,
            y1: l.start.y as f32,
            x2: l.end.x as f32,
            y2: l.end.y as f32,
            transform,
            spread_method,
            stops,
            anti_alias: true,
        }
        .into(),
        GradientKind::Radial(r) => RadialGradient {
            fx: r.start_center.x as f32,
            fy: r.start_center.y as f32,
            fr: r.start_radius,
            cx: r.end_center.x as f32,
            cy: r.end_center.y as f32,
            cr: r.end_radius,
            transform,
            spread_method,
            stops,
            anti_alias: true,
        }
        .into(),
        GradientKind::Sweep(s) => SweepGradient {
            cx: s.center.x as f32,
            cy: s.center.y as f32,
            start_angle: s.start_angle.to_degrees(),
            end_angle: s.end_angle.to_degrees(),
            transform,
            spread_method,
            stops,
            anti_alias: true,
        }
        .into(),
    }
}
