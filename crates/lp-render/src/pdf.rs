//! PDF output: the recorded display list ([`Scene`]) replayed into krilla,
//! one PDF page per page box. Vector paths, gradients, images and text
//! (embedded, subset fonts) stay vector; what the PDF model has no
//! equivalent for is skipped and counted in [`PdfStats`].
//!
//! Text is written as glyph ids with positions, as Blitz shaped it. The
//! display list carries no source text, so the text a glyph stands for is
//! recovered from the font's cmap (reverse lookup): plain text extracts and
//! copies, ligatures and glyphs reached only through GSUB do not.
//!
//! Paper-cut documents (no page boxes) also pass [`Extras`]: the text runs
//! as painted, with their source text (ligatures copy as the characters
//! they stand for) and elements, which give link annotations, a tag tree
//! (headings, paragraphs, links). Blurred outset box shadows are drawn on
//! both (`LIGHTNINGPANDA_PDF_SHADOWS=0` skips them).

use std::collections::HashMap;

use anyrender::recording::{RenderCommand, Scene};
use anyrender::Paint;
use krilla::color::rgb;
use krilla::geom::{Path, PathBuilder, Point as KPoint, Quadrilateral, Rect as KRect, Size, Transform};
use krilla::image::{BitsPerComponent, CustomImage, ImageColorspace};
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
    /// Inset box shadows, written as box-minus-hole fills.
    pub inset_shadows: u32,
    /// Zero-area or fully transparent fills: not written.
    pub degenerate_skipped: u32,
    /// Commands outside the page they were replayed for.
    pub culled: u32,
    /// Blurred shadows: outset ones not drawn, inset ones drawn unblurred.
    pub box_shadows_skipped: u32,
    /// Blurred outset shadows drawn (as an image of the blurred shape).
    pub box_shadows: u32,
    /// Link annotations written.
    pub links: u32,
    /// Glyph runs written with their source text (ligatures, tags).
    pub runs_with_source: u32,
    /// Blurred outset shadows hidden on the page by their element's opaque
    /// background: not drawn.
    pub box_shadows_covered: u32,
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
    /// The bundled font list bullets are drawn with.
    marker: bool,
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
                let marker = data.data.as_ref() == blitz_dom::BULLET_FONT;
                Some(FontEntry { font, chars, marker })
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

/// Where a page's content goes on paper larger than it (printing with
/// margins or a scale).
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    /// The page box (pt).
    pub paper: (f64, f64),
    /// Top-left corner of the content (pt).
    pub offset: (f64, f64),
    /// Content scale (1 = 100%).
    pub scale: f64,
}

/// What a structure element of the tag tree is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StructKind {
    /// `h1`-`h6`.
    Heading(u8),
    Paragraph,
    /// The link of [`Extras::links`] at this index.
    Link(usize),
}

/// A structure element: its kind and the element it sits in (`None`: the
/// document).
#[derive(Clone, Debug)]
pub struct Struct {
    pub kind: StructKind,
    pub parent: Option<u32>,
}

/// Where a link goes.
#[derive(Clone, Debug)]
pub enum LinkTarget {
    Uri(String),
    /// A point in the document (device px, document coordinates).
    Internal(f64, f64),
}

/// A link: the areas it covers (device px, document coordinates) and its
/// structure element.
#[derive(Clone, Debug)]
pub struct Link {
    pub rects: Vec<Rect>,
    pub target: LinkTarget,
    pub structure: u32,
    /// It covers its border box (else its text, line by line).
    pub boxed: bool,
}

/// What a paper-cut document carries besides its display list.
#[derive(Clone, Debug, Default)]
pub struct Extras {
    /// The text runs in the order they were painted.
    pub runs: Vec<blitz_paint::TextRun>,
    /// Each run's innermost structure element.
    pub run_struct: Vec<Option<u32>>,
    pub structs: Vec<Struct>,
    pub links: Vec<Link>,
    /// Write a tag tree.
    pub tagged: bool,
    pub lang: Option<String>,
    /// A full slice's height (device px) and the canvas background: pages
    /// cut short (at a break) are filled with it to the slice's height.
    pub slice_height: f64,
    pub canvas: Option<[u8; 4]>,
    /// How much of a page's slice the paper's content box shows (device px).
    /// A page holds whole CSS px, so its last row can be cut by this.
    pub shown_height: f64,
}

type Leaves = Vec<(krilla::tagging::Identifier, Option<u32>)>;

/// Writes `scene` (device px, document coordinates) as a PDF with one page
/// per rectangle of `pages`; `scale` converts device px to points. Without
/// a `placement` each page is its rectangle's size.
pub fn write(
    scene: &Scene,
    pages: &[Rect],
    scale: f64,
    placement: Option<Placement>,
    extras: Option<&Extras>,
    cache: &mut PdfCache,
) -> Result<(Vec<u8>, PdfStats), String> {
    let mut stats = PdfStats::default();
    let tagged = extras.is_some_and(|e| e.tagged);
    let mut doc = match extras {
        Some(_) => {
            krilla::Document::new_with(krilla::SerializeSettings { enable_tagging: tagged, ..Default::default() })
        }
        None => krilla::Document::new(),
    };
    let dpr = PX_TO_PT / scale;
    let mut leaves: Leaves = Vec::new();
    for page in pages {
        let (w, h) = match placement {
            Some(p) => (p.paper.0 as f32, p.paper.1 as f32),
            None => ((page.width() * scale) as f32, (page.height() * scale) as f32),
        };
        let settings = PageSettings::from_wh(w.max(1.0), h.max(1.0)).ok_or("bad page size")?;
        let mut p = doc.start_page_with(settings);
        let mut s = p.surface();
        let scale = match placement {
            Some(p) => {
                s.push_transform(&Transform::from_translate(p.offset.0 as f32, p.offset.1 as f32));
                scale * p.scale
            }
            None => scale,
        };
        s.push_transform(&Transform::from_scale(scale as f32, scale as f32));
        s.push_transform(&Transform::from_translate(-page.x0 as f32, -page.y0 as f32));
        if let Some(ex) = extras {
            fill_short_page(&mut s, *page, ex);
        }
        // What the paper's content box shows of the page.
        let shown = match extras {
            Some(ex) => Rect::new(page.x0, page.y0, page.x1, page.y1.min(page.y0 + ex.shown_height)),
            None => *page,
        };
        if let Some(clip) = rect_path(shown) {
            s.push_clip_path(&clip, &FillRule::NonZero);
        }
        Replay {
            s: &mut s,
            page: *page,
            cache,
            stats: &mut stats,
            layers: Vec::new(),
            extras,
            dpr,
            next_run: 0,
            leaves: if tagged { Some(&mut leaves) } else { None },
        }
        .run(scene);
        s.pop();
        s.pop();
        s.pop();
        if placement.is_some() {
            s.pop();
        }
        s.finish();
        if let Some(ex) = extras {
            // `scale` already includes the placement's scale.
            let offset = placement.map_or((0.0, 0.0), |p| p.offset);
            for link in &ex.links {
                let Some(annotation) = link_annotation(link, *page, shown, pages, offset, scale) else { continue };
                stats.links += 1;
                if tagged {
                    leaves.push((p.add_tagged_annotation(annotation), Some(link.structure)));
                } else {
                    p.add_annotation(annotation);
                }
            }
        }
        p.finish();
    }
    if let (true, Some(ex)) = (tagged, extras) {
        doc.set_tag_tree(tag_tree(ex, leaves));
    }
    let bytes = doc.finish().map_err(|e| format!("krilla: {e:?}"))?;
    Ok((bytes, stats))
}

/// A page cut short at a break: the rest of the slice in the canvas color.
fn fill_short_page(s: &mut Surface<'_>, page: Rect, ex: &Extras) {
    let Some(color) = ex.canvas else { return };
    if page.height() >= ex.slice_height - 0.5 {
        return;
    }
    let Some(rest) = rect_path(Rect::new(page.x0, page.y1, page.x1, page.y0 + ex.slice_height)) else { return };
    s.set_stroke(None);
    s.set_fill(Some(KFill {
        paint: rgb::Color::new(color[0], color[1], color[2]).into(),
        opacity: norm(color[3] as f32 / 255.0),
        rule: FillRule::NonZero,
    }));
    s.draw_path(&rest);
}

/// `link`'s annotation on `page`, of what `shown` shows of it (`None`: it is
/// not on it). `offset` and `content_scale` place the page's content on paper
/// (pt).
fn link_annotation(
    link: &Link,
    page: Rect,
    shown: Rect,
    pages: &[Rect],
    offset: (f64, f64),
    content_scale: f64,
) -> Option<krilla::annotation::Annotation> {
    use krilla::annotation::{Annotation, LinkAnnotation, Target};
    let rects: Vec<KRect> = link
        .rects
        .iter()
        .filter_map(|r| {
            let r = r.intersect(shown);
            if r.width() <= 0.0 || r.height() <= 0.0 {
                return None;
            }
            let x = |v: f64| ((v - page.x0) * content_scale + offset.0) as f32;
            let y = |v: f64| ((v - page.y0) * content_scale + offset.1) as f32;
            KRect::from_ltrb(x(r.x0), y(r.y0), x(r.x1), y(r.y1))
        })
        .collect();
    if rects.is_empty() {
        return None;
    }
    let target = match &link.target {
        LinkTarget::Uri(uri) => Target::Action(krilla::action::LinkAction::new(uri.clone()).into()),
        LinkTarget::Internal(x, y) => {
            // A target in a margin dropped at a cut is at the next page's top.
            let i = pages.iter().position(|p| *y < p.y1)?;
            let px = (x - pages[i].x0).max(0.0) * content_scale + offset.0;
            let py = (y - pages[i].y0).max(0.0) * content_scale + offset.1;
            let dest = krilla::destination::XyzDestination::new(i, KPoint::from_xy(px as f32, py as f32));
            Target::Destination(krilla::destination::Destination::Xyz(dest))
        }
    };
    // One area: its rect, as Chrome writes them; several (a link wrapping
    // across lines): quad points.
    let annotation = match rects.as_slice() {
        [rect] => LinkAnnotation::new(*rect, target),
        _ => LinkAnnotation::new_with_quad_points(rects.into_iter().map(Quadrilateral::from).collect(), target),
    };
    Some(Annotation::new_link(annotation, None))
}

/// The tag tree of `ex`'s structure elements holding `leaves` (marked
/// content and annotations, in reading order). Structure elements come in
/// the order their first content does; text outside any is grouped in
/// `NonStruct` elements, as Chrome does.
fn tag_tree(ex: &Extras, leaves: Leaves) -> krilla::tagging::TagTree {
    use krilla::tagging::{Identifier, Node, Tag, TagGroup, TagKind, TagTree};
    enum Child {
        Leaf(Identifier),
        Struct(u32),
        Loose(Vec<Identifier>),
    }
    fn place(s: u32, ex: &Extras, placed: &mut [bool], children: &mut [Vec<Child>], root: &mut Vec<Child>) {
        let i = s as usize;
        if placed[i] {
            return;
        }
        placed[i] = true;
        match ex.structs[i].parent {
            Some(parent) => {
                place(parent, ex, placed, children, root);
                children[parent as usize].push(Child::Struct(s));
            }
            None => root.push(Child::Struct(s)),
        }
    }
    fn build(child: Child, ex: &Extras, children: &mut Vec<Vec<Child>>) -> Node {
        match child {
            Child::Leaf(id) => id.into(),
            Child::Loose(ids) => {
                TagGroup::with_children(Tag::NonStruct, ids.into_iter().map(Node::from).collect()).into()
            }
            Child::Struct(s) => {
                let kind: TagKind = match ex.structs[s as usize].kind {
                    StructKind::Heading(level) => {
                        let level =
                            std::num::NonZeroU16::new(level.clamp(1, 6) as u16).unwrap_or(std::num::NonZeroU16::MIN);
                        Tag::Hn(level, None).into()
                    }
                    StructKind::Paragraph => Tag::P.into(),
                    StructKind::Link(_) => Tag::Link.into(),
                };
                let kids = std::mem::take(&mut children[s as usize]);
                let nodes = kids.into_iter().map(|c| build(c, ex, children)).collect();
                TagGroup::with_children(kind, nodes).into()
            }
        }
    }
    let n = ex.structs.len();
    let mut children: Vec<Vec<Child>> = (0..n).map(|_| Vec::new()).collect();
    let mut placed = vec![false; n];
    let mut root: Vec<Child> = Vec::new();
    for (id, s) in leaves {
        match s.filter(|&s| (s as usize) < n) {
            Some(s) => {
                place(s, ex, &mut placed, &mut children, &mut root);
                children[s as usize].push(Child::Leaf(id));
            }
            None => match root.last_mut() {
                Some(Child::Loose(ids)) => ids.push(id),
                _ => root.push(Child::Loose(vec![id])),
            },
        }
    }
    let mut tree = TagTree::new().with_lang(ex.lang.clone());
    for child in root {
        tree.push(build(child, ex, &mut children));
    }
    tree
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
    /// What this and the layers under it let through.
    clip: Rect,
}

struct Replay<'a, 'b> {
    s: &'a mut Surface<'b>,
    page: Rect,
    cache: &'a mut PdfCache,
    stats: &'a mut PdfStats,
    layers: Vec<Pending>,
    extras: Option<&'a Extras>,
    /// Device px per CSS px.
    dpr: f64,
    /// The next of `extras.runs` to match a glyph run command with.
    next_run: usize,
    /// Tagged content written, with its structure element.
    leaves: Option<&'a mut Leaves>,
}

impl Replay<'_, '_> {
    fn run(&mut self, scene: &Scene) {
        let cmds = &scene.commands;
        let mut i = 0;
        while i < cmds.len() {
            let cmd = &cmds[i];
            i += 1;
            // Blitz's inset box-shadow: fill the box, then knock the shadow's
            // hole out with a `DestOut` layer. PDF has no destination-out;
            // the same pixels are one even-odd fill of box minus hole (the
            // blur is dropped).
            if let (
                RenderCommand::Fill(f),
                Some(RenderCommand::PushLayer(l)),
                Some(RenderCommand::BoxShadow(b)),
                Some(RenderCommand::PopLayer),
            ) = (cmd, cmds.get(i), cmds.get(i + 1), cmds.get(i + 2))
            {
                if l.blend.compose == peniko::Compose::DestOut {
                    i += 3;
                    let mut path = f.shape.clone();
                    path.apply_affine(f.transform);
                    let mut hole = kurbo::RoundedRect::from_rect(b.rect, b.radius).to_path(0.1);
                    hole.apply_affine(b.transform);
                    path.extend(hole);
                    if !self.visible(path.bounding_box()) {
                        continue;
                    }
                    if b.std_dev > 0.5 {
                        self.stats.box_shadows_skipped += 1;
                    }
                    self.stats.inset_shadows += 1;
                    self.fill(&path, Affine::IDENTITY, Fill::EvenOdd, &f.brush, f.brush_transform);
                    continue;
                }
            }
            match cmd {
                RenderCommand::PushClipLayer(c) => {
                    let clip = self.clip_with(c.transform.transform_rect_bbox(c.clip.bounding_box()));
                    self.layers.push(Pending {
                        layer: Layer::Clip { path: to_path(&c.clip, c.transform), rule: rule(c.fill) },
                        pushed: None,
                        clip,
                    })
                }
                RenderCommand::PushLayer(c) => {
                    if c.filter.is_some() || c.backdrop_filter.is_some() {
                        self.stats.filters_skipped += 1;
                    }
                    if c.blend.compose != peniko::Compose::SrcOver {
                        self.stats.compose_skipped += 1;
                    }
                    let clip = self.clip_with(c.transform.transform_rect_bbox(c.clip.bounding_box()));
                    self.layers.push(Pending {
                        clip,
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
                    if c.shape.area().abs() < 1e-3 || matches!(&c.brush, Paint::Solid(col) if col.components[3] == 0.0)
                    {
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
                    let source = self.match_run(c);
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
                    // A recorded run's middle: a line on the next page (cuts are
                    // at line tops, and a content area can start above its line
                    // box) is not written on this one.
                    let bounds = match source.and_then(|i| self.extras?.runs.get(i)) {
                        Some(r) => r.bounds.inflate(-0.5, 0.5 - r.bounds.height() / 2.0),
                        None => c.transform.transform_rect_bbox(bounds),
                    };
                    if !self.visible(bounds) {
                        continue;
                    }
                    self.glyph_run(c, source);
                }
                RenderCommand::BoxShadow(c) => {
                    if shadows_enabled() {
                        // Chrome's blur: a Gaussian of standard deviation blur / 2.
                        let sigma = c.std_dev / 2.0 * self.dpr;
                        let reach = c.transform.transform_rect_bbox(c.rect.inflate(sigma * 3.0, sigma * 3.0));
                        if !self.visible(reach) {
                            continue;
                        }
                        // The element's own opaque background is painted right
                        // after its shadows: what of the shadow it covers on
                        // this page (a page box's shadow on its own page) is
                        // not drawn.
                        if covered(&cmds[i..], reach.intersect(self.page)) {
                            self.stats.box_shadows_covered += 1;
                            continue;
                        }
                        self.box_shadow(c, sigma);
                    } else if self.visible(c.transform.transform_rect_bbox(c.rect.inflate(c.radius, c.radius))) {
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

    /// The painted text run `c` is (`extras.runs` are in paint order; glyph
    /// runs Blitz's text painting did not draw, such as SVG text, match none).
    fn match_run(&mut self, c: &anyrender::recording::GlyphRunCommand) -> Option<usize> {
        let run = self.extras?.runs.get(self.next_run)?;
        let first = c.glyphs.first().map(|g| (g.id, g.x, g.y));
        if run.glyphs != c.glyphs.len() || (run.glyphs != 0 && first != Some(run.first)) {
            return None;
        }
        self.next_run += 1;
        Some(self.next_run - 1)
    }

    /// A blurred outset box shadow: its rounded rectangle blurred by `sigma`
    /// (device px) into an image's alpha, drawn in the shadow's color.
    fn box_shadow(&mut self, c: &anyrender::recording::BoxShadowCommand, sigma: f64) {
        let color = c.brush.to_rgba8();
        if color.a == 0 {
            return;
        }
        if sigma < 0.25 {
            let path = kurbo::RoundedRect::from_rect(c.rect, c.radius).to_path(0.1);
            self.stats.box_shadows += 1;
            self.fill(&path, c.transform, Fill::NonZero, &Paint::Solid(c.brush), None);
            return;
        }
        // Only the part on this page (a page-sized shadow crossing pages
        // becomes one image per page).
        let limit = c.transform.inverse().transform_rect_bbox(self.page.inflate(1.0, 1.0));
        let Some((image, area)) = shadow_image(c.rect, c.radius, sigma, [color.r, color.g, color.b, color.a], limit)
        else {
            return;
        };
        let Some(size) = Size::from_wh(area.width() as f32, area.height() as f32) else { return };
        self.stats.box_shadows += 1;
        self.flush();
        self.with_transform(c.transform * Affine::translate((area.x0, area.y0)), |s| s.draw_image(image, size));
    }

    /// What the current layers let through, also clipped by `bounds`.
    fn clip_with(&self, bounds: Rect) -> Rect {
        self.layers.last().map_or(bounds, |l| l.clip.intersect(bounds))
    }

    /// Whether no glyph of a run can be clipped, which most runs show at the cost
    /// of one rectangle instead of one per glyph.
    fn run_all_shown(&self, c: &anyrender::recording::GlyphRunCommand) -> bool {
        let size = c.font_size as f64;
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for g in &c.glyphs {
            x0 = x0.min(g.x as f64);
            x1 = x1.max(g.x as f64);
            y0 = y0.min(g.y as f64);
            y1 = y1.max(g.y as f64);
        }
        let run = c.transform.transform_rect_bbox(Rect::new(x0, y0 - size, x1 + size, y1 + size * 0.3));
        run.x0 >= self.page.x0
            && run.x1 <= self.page.x1
            && self.layers.last().is_none_or(|l| {
                run.x0 >= l.clip.x0 && run.x1 <= l.clip.x1 && run.y0 >= l.clip.y0 && run.y1 <= l.clip.y1
            })
    }

    /// The glyphs of a run a paper-cut page writes. Chrome's PDF has no clipped
    /// text: Skia's PDF device skips a glyph outside the clip's bounds (the page's
    /// sides or an overflow clip), so it is not extracted either. The page's top
    /// and bottom are not looked at: a line's place on a page is decided for the
    /// whole run.
    fn glyphs_shown(&self, c: &anyrender::recording::GlyphRunCommand) -> Vec<usize> {
        let size = c.font_size as f64;
        let clip = self.layers.last().map(|l| l.clip);
        let (left, right) = clip.map_or((self.page.x0, self.page.x1), |l| {
            (self.page.x0.max(l.x0), self.page.x1.min(l.x1))
        });
        let (top, bottom) = clip.map_or((f64::MIN, f64::MAX), |l| (l.y0, l.y1));
        let inside = |glyph: Rect| glyph.x0 < right && left < glyph.x1 && glyph.y0 < bottom && top < glyph.y1;
        let edges = |i: usize| {
            let g = &c.glyphs[i];
            let (x, y) = (g.x as f64, g.y as f64);
            let end = c.glyphs.get(i + 1).map_or(x + size * 0.6, |n| (n.x as f64).max(x));
            Rect::new(x, y - size, end, y + size * 0.3)
        };
        let all = 0..c.glyphs.len();
        let [a, b, skew, d, e, f] = c.transform.as_coeffs();
        // Text is hardly ever rotated: no corners to work out per glyph.
        if b == 0.0 && skew == 0.0 && a > 0.0 && d > 0.0 {
            all.filter(|&i| {
                let r = edges(i);
                inside(Rect::new(a * r.x0 + e, d * r.y0 + f, a * r.x1 + e, d * r.y1 + f))
            })
            .collect()
        } else {
            all.filter(|&i| inside(c.transform.transform_rect_bbox(edges(i)))).collect()
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
    fn paint(
        &mut self,
        brush: &Paint,
        brush_transform: Option<Affine>,
    ) -> Option<(krilla::paint::Paint, NormalizedF32)> {
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

    fn glyph_run(&mut self, c: &anyrender::recording::GlyphRunCommand, source: Option<usize>) {
        if !c.normalized_coords.is_empty() {
            self.stats.variable_runs += 1;
        }
        if c.embolden != kurbo::Vec2::ZERO {
            self.stats.embolden_skipped += 1;
        }
        // Only a paper-cut page: `.pz-page` documents keep what they had.
        let paper_cut = self.extras.is_some();
        let shown: Vec<usize> = if paper_cut && !self.run_all_shown(c) {
            self.glyphs_shown(c)
        } else {
            (0..c.glyphs.len()).collect()
        };
        let Some(&first) = shown.first() else { return };
        let Some((paint, opacity)) = self.paint(&c.brush, None) else { return };
        let opacity = norm(opacity.get() * c.brush_alpha);
        let Some(entry) = self.cache.font(&c.font_data) else {
            self.stats.fonts_failed += 1;
            return;
        };
        let font = entry.font.clone();
        // Chrome draws bullets (disc, circle, square, disclosure) as shapes,
        // which are not text in the PDF. Here they are glyphs of the marker
        // font, drawn as outlines.
        let outlined = paper_cut && entry.marker;
        let size = c.font_size;
        let mut text = String::new();
        let mut glyphs = Vec::with_capacity(shown.len());
        let (gx, gy) = (c.glyphs[first].x, c.glyphs[first].y);
        // The run's source text when it was recorded: each glyph stands for
        // its cluster's characters (all of them for a ligature).
        let ex = self.extras;
        let recorded = source.and_then(|i| ex?.runs.get(i)).filter(|r| !r.text.is_empty());
        if let Some(r) = recorded {
            text.push_str(&r.text);
            self.stats.runs_with_source += 1;
        }
        for (at, &i) in shown.iter().enumerate() {
            let g = &c.glyphs[i];
            let range = match recorded {
                Some(r) => r.glyph_text[i].start as usize..r.glyph_text[i].end as usize,
                None => {
                    let start = text.len();
                    if let Some(ch) = entry.chars.get(&g.id) {
                        text.push(*ch);
                    }
                    start..text.len()
                }
            };
            if range.is_empty() {
                self.stats.glyphs_without_text += 1;
            }
            let advance = shown.get(at + 1).map_or(0.0, |&n| c.glyphs[n].x - g.x);
            glyphs.push(KrillaGlyph::new(
                GlyphId::new(g.id),
                advance / size,
                0.0,
                -(g.y - gy) / size,
                0.0,
                range,
                None,
            ));
        }
        let structure = source.and_then(|i| ex?.run_struct.get(i).copied().flatten());
        self.stats.glyph_runs += 1;
        self.stats.glyphs += glyphs.len() as u32;
        self.flush();
        let style = c.style.clone();
        let glyph_transform = c.glyph_transform;
        let mut leaves = self.leaves.take();
        self.with_transform(c.transform, |s| {
            let span = krilla::tagging::ContentTag::Span(krilla::tagging::SpanTag::empty());
            let tag = leaves.as_deref_mut().map(|l| (l, s.start_tagged(span)));
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
                    for (&i, kg) in shown.iter().zip(&glyphs) {
                        let g = &c.glyphs[i];
                        s.push_transform(&transform(Affine::translate((g.x as f64, g.y as f64)) * gt));
                        let one = KrillaGlyph { y_offset: 0.0, ..kg.clone() };
                        s.draw_glyphs(KPoint::from_xy(0.0, 0.0), &[one], font.clone(), &text, size, outlined);
                        s.pop();
                    }
                }
                None => s.draw_glyphs(KPoint::from_xy(gx, gy), &glyphs, font, &text, size, outlined),
            }
            s.set_stroke(None);
            if let Some((leaves, id)) = tag {
                s.end_tagged();
                leaves.push((id, structure));
            }
        });
        self.leaves = leaves;
    }
}

/// Whether `area` (device px) is under the opaque rectangular fill that
/// follows the box shadows at the start of `next` (an element paints its
/// outset shadows, then its background; a shadow clipped to outside the box
/// is in a layer, and the layer comes first).
fn covered(next: &[RenderCommand], area: Rect) -> bool {
    let Some(RenderCommand::Fill(f)) = next.iter().find(|c| !matches!(c, RenderCommand::BoxShadow(_))) else {
        return false;
    };
    let opaque = matches!(&f.brush, Paint::Solid(c) if c.components[3] >= 1.0);
    if !opaque || area.width() <= 0.0 || area.height() <= 0.0 {
        return false;
    }
    // Only an axis-aligned rectangle: its bounding box is what it covers.
    let b = f.shape.bounding_box();
    let corners = f.shape.elements().iter().all(|el| match el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) => (p.x == b.x0 || p.x == b.x1) && (p.y == b.y0 || p.y == b.y1),
        PathEl::ClosePath => true,
        _ => false,
    });
    let [sx, ky, kx, sy, _, _] = f.transform.as_coeffs();
    if !corners || ky != 0.0 || kx != 0.0 || sx <= 0.0 || sy <= 0.0 {
        return false;
    }
    let cover = f.transform.transform_rect_bbox(b);
    cover.x0 <= area.x0 && cover.y0 <= area.y0 && cover.x1 >= area.x1 && cover.y1 >= area.y1
}

/// A shadow's alpha as an image: color channels constant, drawn with
/// interpolation so the low-resolution mask stays smooth.
#[derive(Clone, Hash)]
struct ShadowImage {
    rgb: std::sync::Arc<Vec<u8>>,
    alpha: std::sync::Arc<Vec<u8>>,
    size: (u32, u32),
}

impl CustomImage for ShadowImage {
    fn color_channel(&self) -> &[u8] {
        &self.rgb
    }
    fn alpha_channel(&self) -> Option<&[u8]> {
        Some(&self.alpha)
    }
    fn bits_per_component(&self) -> BitsPerComponent {
        BitsPerComponent::Eight
    }
    fn size(&self) -> (u32, u32) {
        self.size
    }
    fn icc_profile(&self) -> Option<&[u8]> {
        None
    }
    fn color_space(&self) -> ImageColorspace {
        ImageColorspace::Rgb
    }
}

/// The rounded rectangle `rect` (radius `radius`) blurred by a Gaussian of
/// standard deviation `sigma`, as an image and the area it covers (both in
/// the shadow's coordinates). The shape is sampled at three cells per sigma
/// (at most 512 cells a side), then blurred with a separable kernel.
fn shadow_image(
    rect: Rect,
    radius: f64,
    sigma: f64,
    color: [u8; 4],
    limit: Rect,
) -> Option<(krilla::image::Image, Rect)> {
    let (image, area) = shadow_mask(rect, radius, sigma, color, limit)?;
    Some((krilla::image::Image::from_custom(image, true).ok()?, area))
}

/// Whether blurred outset box shadows are drawn (`LIGHTNINGPANDA_PDF_SHADOWS=0`
/// skips them, counted in [`PdfStats::box_shadows_skipped`], as before).
fn shadows_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("LIGHTNINGPANDA_PDF_SHADOWS").map_or(true, |v| v != "0"))
}

/// [`shadow_image`]'s pixels: straight (not premultiplied) alpha over a
/// constant color, for the part of the blur inside `limit`.
fn shadow_mask(rect: Rect, radius: f64, sigma: f64, color: [u8; 4], limit: Rect) -> Option<(ShadowImage, Rect)> {
    let area = rect.inflate(sigma * 3.0, sigma * 3.0).intersect(limit);
    if !(area.width() > 0.0 && area.height() > 0.0 && sigma.is_finite()) {
        return None;
    }
    let cell = (sigma / 3.0).max(0.5).max(area.width().max(area.height()) / 512.0);
    let (w, h) = ((area.width() / cell).ceil() as usize, (area.height() / cell).ceil() as usize);
    let area = Rect::new(area.x0, area.y0, area.x0 + w as f64 * cell, area.y0 + h as f64 * cell);
    let r = radius.clamp(0.0, rect.width().min(rect.height()) / 2.0);
    let (cx, cy) = (rect.center().x, rect.center().y);
    let (hw, hh) = (rect.width() / 2.0 - r, rect.height() / 2.0 - r);
    // Coverage of each cell, from the shape's signed distance.
    let mut cov = vec![0f32; w * h];
    for y in 0..h {
        let py = area.y0 + (y as f64 + 0.5) * cell;
        for x in 0..w {
            let px = area.x0 + (x as f64 + 0.5) * cell;
            let (dx, dy) = ((px - cx).abs() - hw, (py - cy).abs() - hh);
            let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
            let d = outside + dx.max(dy).min(0.0) - r;
            cov[y * w + x] = (0.5 - d / cell).clamp(0.0, 1.0) as f32;
        }
    }
    let s = sigma / cell;
    let half = (s * 3.0).ceil() as isize;
    let kernel: Vec<f32> = (-half..=half).map(|i| (-(i as f64).powi(2) / (2.0 * s * s)).exp() as f32).collect();
    let total: f32 = kernel.iter().sum();
    let kernel: Vec<f32> = kernel.iter().map(|k| k / total).collect();
    // One pass along lines of `len` samples `step` apart, `count` lines `stride` apart.
    let blur = |src: &[f32], dst: &mut [f32], len: usize, count: usize, stride: usize, step: usize| {
        for line in 0..count {
            for i in 0..len {
                let mut acc = 0.0;
                for (k, weight) in kernel.iter().enumerate() {
                    let j = i as isize + k as isize - half;
                    if j >= 0 && (j as usize) < len {
                        acc += src[line * stride + j as usize * step] * weight;
                    }
                }
                dst[line * stride + i * step] = acc;
            }
        }
    };
    let mut tmp = vec![0f32; w * h];
    blur(&cov, &mut tmp, w, h, w, 1);
    blur(&tmp, &mut cov, h, w, 1, w);
    let a = color[3] as f32 / 255.0;
    let alpha: Vec<u8> = cov.iter().map(|v| (v * a * 255.0).round().clamp(0.0, 255.0) as u8).collect();
    let rgb: Vec<u8> = std::iter::repeat_n([color[0], color[1], color[2]], w * h).flatten().collect();
    Some((ShadowImage { rgb: rgb.into(), alpha: alpha.into(), size: (w as u32, h as u32) }, area))
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
    b.push_rect(krilla::geom::Rect::from_ltrb(
        r.x0 as f32,
        r.y0 as f32,
        r.x1.max(r.x0 + 0.001) as f32,
        r.y1.max(r.y0 + 0.001) as f32,
    )?);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadow_masks_are_straight_alpha_and_limited_to_the_page() {
        // A 50% black shadow: full coverage well inside the shape keeps
        // alpha 128 over color 0 (straight alpha, nothing premultiplied).
        let rect = Rect::new(0.0, 0.0, 400.0, 400.0);
        let (img, area) = shadow_mask(rect, 0.0, 4.0, [10, 20, 30, 128], Rect::new(-1e9, -1e9, 1e9, 1e9)).unwrap();
        let (w, h) = img.size;
        let center = (h / 2 * w + w / 2) as usize;
        assert_eq!(img.alpha[center], 128);
        assert_eq!(&img.rgb[center * 3..center * 3 + 3], &[10, 20, 30]);
        assert_eq!(img.alpha[0], 0, "transparent past three sigma");
        assert_eq!(area, Rect::new(-12.0, -12.0, 412.0, 412.0));
        // Limited to a page: only that part is sampled.
        let (_, part) = shadow_mask(rect, 0.0, 4.0, [0, 0, 0, 255], Rect::new(0.0, 200.0, 1000.0, 1000.0)).unwrap();
        assert_eq!((part.y0, part.y1), (200.0, 412.0));
    }
}
