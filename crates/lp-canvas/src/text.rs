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
}

struct Context {
    fonts: FontContext,
    layout: LayoutContext<()>,
}

thread_local! {
    static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) };
}

/// HTML "text preparation": ASCII whitespace becomes spaces (one line).
fn prepare(text: &str) -> Cow<'_, str> {
    if text.bytes().any(|b| matches!(b, b'\t' | b'\n' | b'\x0c' | b'\r')) {
        Cow::Owned(text.chars().map(|c| if matches!(c, '\t' | '\n' | '\x0c' | '\r') { ' ' } else { c }).collect())
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
        if text.is_empty() {
            // No glyphs: the metrics of the font a space would use.
            let space = shape(cx, " ", style);
            return TextLayout { ascent: space.ascent, descent: space.descent, ..TextLayout::default() };
        }
        let mut out = shape(cx, &text, style);
        if out.runs.is_empty() {
            // No glyphs (empty text): the metrics of the font a space
            // would use.
            let space = shape(cx, " ", style);
            out.ascent = space.ascent;
            out.descent = space.descent;
        }
        out
    })
}

fn shape(cx: &mut Context, text: &str, style: &TextStyle<'_>) -> TextLayout {
    let size = if style.size.is_finite() && style.size > 0.0 { style.size.min(10_000.0) } else { 10.0 };
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
            out.ascent = f64::from(metrics.ascent);
            out.descent = f64::from(metrics.descent);
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
