//! Canvas text: one line shaped with Parley on the process's system fonts
//! ([`crate::fonts`]), with the glyphs, the advance width, the primary
//! font's ascent and descent, and the ink bounds `measureText` reports.
//!
//! The font context is made per thread on its first use (a clone of the
//! process's), so a page that never draws or measures text pays nothing.

use std::borrow::Cow;
use std::cell::RefCell;

use kurbo::Rect;
use parley::fontique::{FontStyle, FontWeight};
use parley::{FontContext, FontData, FontFamily, Layout, LayoutContext, PositionedLayoutItem, StyleProperty};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::TableProvider as _;
use skrifa::MetadataProvider;

/// The font a text is drawn with.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle<'a> {
    /// CSS `font-family` list.
    pub family: &'a str,
    /// px.
    pub size: f32,
    pub weight: f32,
    pub italic: bool,
    pub letter_spacing: f32,
    pub word_spacing: f32,
    /// BCP 47 language the text is shaped for (OpenType `locl`, such as Turkish
    /// keeping `fi` apart), `None` when unknown.
    pub lang: Option<&'a str>,
    /// `font-variant-caps` keyword (canvas `fontVariantCaps`); the small and
    /// petite caps values are synthesized ([`small_caps`]).
    pub variant_caps: &'a str,
}

/// Glyphs of one font, positioned relative to the text's origin (the
/// alphabetic baseline at the start of the line, y down).
#[derive(Clone, Debug)]
pub struct TextRun {
    pub font: FontData,
    pub size: f32,
    pub coords: Vec<i16>,
    pub glyphs: Vec<vello_cpu::Glyph>,
    /// Synthesized oblique angle (degrees).
    pub skew: Option<f32>,
}

#[derive(Clone, Debug, Default)]
pub struct TextLayout {
    pub runs: Vec<TextRun>,
    /// Advance width (px).
    pub width: f64,
    /// The primary font's ascent and descent (px, both positive below/above
    /// the baseline as CSS measures them).
    pub ascent: f64,
    pub descent: f64,
    /// Ink bounds relative to the origin (y down), `None` for no ink.
    pub ink: Option<Rect>,
    /// The primary font's baselines from its `BASE` table.
    pub baselines: Baselines,
}

/// Baselines a font's `BASE` table gives (its horizontal axis, the default
/// script's values), in px above the alphabetic baseline; `None` where the
/// table has none.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Baselines {
    pub hanging: Option<f64>,
    pub ideographic: Option<f64>,
}

struct Context {
    fonts: FontContext,
    layout: LayoutContext<()>,
}

thread_local! {
    static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) };
    /// pyro: contexts with web faces registered ([`layout_with_faces`]), by the key of their
    /// face set, least recently made first; at most [`FACED_CONTEXTS`].
    static FACED: RefCell<Vec<(u64, Context)>> = const { RefCell::new(Vec::new()) };
}

/// pyro: face sets whose contexts a thread keeps (a page that keeps adding faces replaces its
/// context instead of piling up font collections).
const FACED_CONTEXTS: usize = 4;

/// pyro: a web font a page gave canvas text (a document `@font-face` rule's loaded source or a
/// loaded script `FontFace`), found by `family` as an installed font is.
#[derive(Clone)]
pub struct WebFace {
    pub family: String,
    pub data: parley::fontique::Blob<u8>,
}

impl WebFace {
    /// `bytes`: an OpenType/TrueType font or collection (not WOFF-compressed).
    pub fn new(family: String, bytes: Vec<u8>) -> WebFace {
        WebFace::shared(family, std::sync::Arc::new(bytes))
    }

    /// [`WebFace::new`] over bytes held elsewhere too (not copied).
    pub fn shared(family: String, bytes: std::sync::Arc<Vec<u8>>) -> WebFace {
        WebFace { family, data: parley::fontique::Blob::new(bytes as _) }
    }
}

/// Drops this thread's text shaping state, the font collection included
/// (the web fonts pages registered are in it): the next [`layout`] makes it
/// again.
pub fn release_thread() {
    let context = CONTEXT.with(|cell| cell.try_borrow_mut().ok().and_then(|mut cell| cell.take()));
    drop(context);
    let faced = FACED.with(|cell| cell.try_borrow_mut().map(|mut cell| std::mem::take(&mut *cell)).unwrap_or_default());
    drop(faced);
}

/// HTML "text preparation": ASCII whitespace becomes spaces (one line). U+0000 is dropped: it
/// draws nothing and takes no space, as in Chromium (`2d.text.measure.width.nullCharacter`), where
/// a font without it would otherwise fall back to one that draws it.
fn prepare(text: &str) -> Cow<'_, str> {
    if text.bytes().any(|b| matches!(b, b'\t' | b'\n' | b'\x0c' | b'\r' | b'\0')) {
        Cow::Owned(
            text.chars()
                .filter(|&c| c != '\0')
                .map(|c| if matches!(c, '\t' | '\n' | '\x0c' | '\r') { ' ' } else { c })
                .collect(),
        )
    } else {
        Cow::Borrowed(text)
    }
}

/// Shapes `text` as one line in `style`.
pub fn layout(text: &str, style: &TextStyle<'_>) -> TextLayout {
    let text = prepare(text);
    CONTEXT.with(|cell| {
        let mut cell = cell.borrow_mut();
        let cx =
            cell.get_or_insert_with(|| Context { fonts: crate::fonts::font_context(), layout: LayoutContext::new() });
        line(cx, &text, style)
    })
}

/// pyro: [`layout`] with a page's web faces besides the process's fonts. `key` names the face set
/// (the caller changes it whenever `faces` change); the thread keeps the font collections of the
/// last few sets. Without faces, exactly [`layout`].
pub fn layout_with_faces(text: &str, style: &TextStyle<'_>, key: u64, faces: &[WebFace]) -> TextLayout {
    if faces.is_empty() {
        return layout(text, style);
    }
    let text = prepare(text);
    FACED.with(|cell| {
        let mut cell = cell.borrow_mut();
        let at = match cell.iter().position(|(k, _)| *k == key) {
            Some(at) => at,
            None => {
                if cell.len() >= FACED_CONTEXTS {
                    cell.remove(0);
                }
                let mut fonts = crate::fonts::font_context();
                for face in faces {
                    // As blitz-dom registers a document's `@font-face` fonts (document.rs).
                    let info_override =
                        parley::fontique::FontInfoOverride { family_name: Some(face.family.as_str()), ..Default::default() };
                    fonts.collection.register_fonts(face.data.clone(), Some(info_override));
                }
                cell.push((key, Context { fonts, layout: LayoutContext::new() }));
                cell.len() - 1
            }
        };
        line(&mut cell[at].1, &text, style)
    })
}

/// One prepared line in `cx`.
fn line(cx: &mut Context, text: &str, style: &TextStyle<'_>) -> TextLayout {
    if text.is_empty() {
        // No glyphs: the metrics of the font a space would use.
        let space = shape(cx, " ", style);
        return TextLayout {
            ascent: space.ascent,
            descent: space.descent,
            baselines: space.baselines,
            ..TextLayout::default()
        };
    }
    let mut out = shape(cx, text, style);
    if out.runs.is_empty() {
        // No glyphs (empty text): the metrics of the font a space
        // would use.
        let space = shape(cx, " ", style);
        out.ascent = space.ascent;
        out.descent = space.descent;
        out.baselines = space.baselines;
    }
    out
}

/// Synthesized small caps, as Chromium draws them for a font without them: the
/// letters `variant_caps` makes small (lowercase ones; for `all-` values
/// uppercase ones too) uppercased, and the byte ranges of `text` to draw at
/// 70% of the size. `None` for other values or nothing to make small.
fn small_caps(text: &str, variant_caps: &str) -> Option<(String, Vec<std::ops::Range<usize>>)> {
    let all = match variant_caps {
        "small-caps" | "petite-caps" => false,
        "all-small-caps" | "all-petite-caps" => true,
        _ => return None,
    };
    let mut out = String::with_capacity(text.len());
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for c in text.chars() {
        let small = c.is_lowercase() || (all && c.is_uppercase());
        let start = out.len();
        if small {
            out.extend(c.to_uppercase());
            match ranges.last_mut() {
                Some(r) if r.end == start => r.end = out.len(),
                _ => ranges.push(start..out.len()),
            }
        } else {
            out.push(c);
        }
    }
    (!ranges.is_empty()).then_some((out, ranges))
}

/// The share of the size synthesized small caps are drawn at (Chromium).
const SMALL_CAPS_SCALE: f32 = 0.7;

fn shape(cx: &mut Context, text: &str, style: &TextStyle<'_>) -> TextLayout {
    let size = if style.size.is_finite() && style.size > 0.0 { style.size.min(10_000.0) } else { 10.0 };
    let caps = small_caps(text, style.variant_caps);
    let text = caps.as_ref().map_or(text, |(t, _)| t.as_str());
    let mut builder = cx.layout.ranged_builder(&mut cx.fonts, text, 1.0, false);
    builder.push_default(StyleProperty::FontFamily(FontFamily::Source(Cow::Borrowed(style.family))));
    builder.push_default(StyleProperty::FontSize(size));
    builder.push_default(StyleProperty::FontWeight(FontWeight::new(style.weight.clamp(1.0, 1000.0))));
    if style.italic {
        builder.push_default(StyleProperty::FontStyle(FontStyle::Italic));
    }
    if style.letter_spacing.is_finite() && style.letter_spacing != 0.0 {
        builder.push_default(StyleProperty::LetterSpacing(style.letter_spacing));
    }
    if style.word_spacing.is_finite() && style.word_spacing != 0.0 {
        builder.push_default(StyleProperty::WordSpacing(style.word_spacing));
    }
    if let Some(lang) = style.lang.and_then(|l| parley::Language::parse(l).ok()) {
        builder.push_default(StyleProperty::Locale(Some(lang)));
    }
    for range in caps.iter().flat_map(|(_, ranges)| ranges) {
        builder.push(StyleProperty::FontSize(size * SMALL_CAPS_SCALE), range.clone());
    }
    let mut layout: Layout<()> = builder.build(text);
    layout.break_all_lines(None);
    let mut out = TextLayout { width: f64::from(layout.full_width()), ..TextLayout::default() };
    let Some(line) = layout.lines().next() else { return out };
    let baseline = line.metrics().baseline;
    let mut first = true;
    for item in line.items() {
        let PositionedLayoutItem::GlyphRun(glyph_run) = item else { continue };
        let run = glyph_run.run();
        let metrics = run.font_metrics();
        if first {
            // At the text's size: a first run of small caps is drawn smaller.
            let scale = if run.font_size() > 0.0 { size / run.font_size() } else { 1.0 };
            out.ascent = f64::from(metrics.ascent * scale);
            out.descent = f64::from(metrics.descent * scale);
            out.baselines = base_baselines(run.font(), size).unwrap_or_default();
            first = false;
        }
        let glyphs: Vec<vello_cpu::Glyph> =
            glyph_run.positioned_glyphs().map(|g| vello_cpu::Glyph { id: g.id, x: g.x, y: g.y - baseline }).collect();
        let coords: Vec<i16> = bytemuck::cast_slice(run.normalized_coords()).to_vec();
        let font = run.font().clone();
        let ink = ink_bounds(&font, run.font_size(), &coords, &glyphs);
        out.ink = match (out.ink, ink) {
            (Some(a), Some(b)) => Some(a.union(b)),
            (a, b) => a.or(b),
        };
        out.runs.push(TextRun { font, size: run.font_size(), coords, glyphs, skew: run.synthesis().skew() });
    }
    out
}

/// The hanging and ideographic baselines of `font`'s `BASE` table at `size`
/// px (its default script: `DFLT`, else `latn`, else the first), as Chromium
/// reads them for `measureText` and `textBaseline`.
fn base_baselines(font: &FontData, size: f32) -> Option<Baselines> {
    let font_ref = skrifa::FontRef::from_index(font.data.as_ref(), font.index).ok()?;
    let upem = font_ref.head().ok()?.units_per_em();
    if upem == 0 {
        return None;
    }
    let base = font_ref.base().ok()?;
    let axis = base.horiz_axis()?.ok()?;
    let tags = axis.base_tag_list()?.ok()?;
    let tags = tags.baseline_tags();
    let scripts = axis.base_script_list().ok()?;
    let records = scripts.base_script_records();
    let record = [skrifa::Tag::new(b"DFLT"), skrifa::Tag::new(b"latn")]
        .iter()
        .find_map(|tag| records.iter().find(|r| r.base_script_tag() == *tag))
        .or_else(|| records.first())?;
    let script = record.base_script(scripts.offset_data()).ok()?;
    let values = script.base_values()?.ok()?;
    let coords = values.base_coords();
    let scale = f64::from(size) / f64::from(upem);
    let coord = |tag: &[u8; 4]| {
        let i = tags.iter().position(|t| t.get() == skrifa::Tag::new(tag))?;
        coords.get(i).ok().map(|c| f64::from(c.coordinate()) * scale)
    };
    Some(Baselines { hanging: coord(b"hang"), ideographic: coord(b"ideo") })
}

/// Ink bounds of `glyphs` (y down), from the outlines' control boxes.
fn ink_bounds(font: &FontData, size: f32, coords: &[i16], glyphs: &[vello_cpu::Glyph]) -> Option<Rect> {
    let font_ref = skrifa::FontRef::from_index(font.data.as_ref(), font.index).ok()?;
    let outlines = font_ref.outline_glyphs();
    let coords: &[skrifa::instance::NormalizedCoord] = bytemuck::cast_slice(coords);
    let location = LocationRef::new(coords);
    let mut bounds: Option<Rect> = None;
    for g in glyphs {
        let Some(outline) = outlines.get(skrifa::GlyphId::new(g.id)) else { continue };
        let mut pen = BoundsPen::default();
        if outline.draw(DrawSettings::unhinted(Size::new(size), location), &mut pen).is_err() {
            continue;
        }
        let Some(r) = pen.rect() else { continue };
        // Font units are y up.
        let r = Rect::new(f64::from(g.x) + r.x0, f64::from(g.y) - r.y1, f64::from(g.x) + r.x1, f64::from(g.y) - r.y0);
        bounds = Some(bounds.map_or(r, |b| b.union(r)));
    }
    bounds
}

#[derive(Default)]
struct BoundsPen {
    rect: Option<Rect>,
}

impl BoundsPen {
    fn add(&mut self, x: f32, y: f32) {
        let (x, y) = (f64::from(x), f64::from(y));
        self.rect = Some(match self.rect {
            Some(r) => r.union_pt(kurbo::Point::new(x, y)),
            None => Rect::new(x, y, x, y),
        });
    }
    fn rect(&self) -> Option<Rect> {
        self.rect
    }
}

impl OutlinePen for BoundsPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.add(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.add(x, y);
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.add(cx0, cy0);
        self.add(x, y);
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.add(cx0, cy0);
        self.add(cx1, cy1);
        self.add(x, y);
    }
    fn close(&mut self) {}
}
