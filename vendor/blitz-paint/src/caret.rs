//! pyro: the caret of the focused editing host (CSS UI 4 § 6.2, `caret-color` and
//! `caret-shape`), as Chromium paints it. The embedder gives the caret's place
//! ([`BaseDocument::set_dom_caret`]). A `block` caret over a character is painted as the top
//! highlight of that character (`highlight::Layer::Caret`: a background under the text, in every
//! writing mode, the text in `caret-color`'s second colour when it has one); the other carets are painted here over the text of their line: `bar` and
//! `auto` a thin bar at the caret, `underscore` a line under the character after it, a `block`
//! with no character after it (at the end of a line, before a replaced element, in an empty
//! host) a box the width of the font's `0`.
//!
//! Carets do not blink in screenshots (`caret-animation` is not parsed).

use anyrender::PaintScene;
use blitz_dom::CopyStyleGetters as _;
use blitz_dom::node::TextBrush;
use blitz_dom::{BaseDocument, CaretPosition, NodeId};
use kurbo::{Affine, Rect};
use parley::{Affinity, Cursor, Layout, Line};
use peniko::Fill;
use style::values::computed::{CaretShape, ColorOrAuto};

use crate::color::{Color, ToColorColor as _};

/// A caret to paint ([`caret_paint`]).
pub(crate) struct CaretPaint {
    /// The inline root whose text holds the caret, or the empty element it is in (`empty`).
    pub node: NodeId,
    pub empty: bool,
    /// The caret's byte offset in the inline root's text.
    pub byte: usize,
    /// A block caret over the character after it: the end of that character (painted as a
    /// highlight).
    pub covers: Option<usize>,
    pub shape: CaretShape,
    pub color: Color,
    /// The colour of the text under a block caret (`None`: its own).
    pub text_color: Option<Color>,
    /// The width of `0` (CSS px).
    pub ch: f64,
    /// The caret's element is right-to-left.
    pub rtl: bool,
    /// The line height of the caret's element (CSS px), for an empty host.
    pub line_height: f64,
}

/// The caret the document paints now (`None`: none).
pub(crate) fn caret_paint(doc: &BaseDocument) -> Option<CaretPaint> {
    use style::computed_values::direction::T as Direction;
    let (position, element) = doc.caret_position()?;
    let style = doc.get_node(element)?.primary_styles()?;
    let shape = style.get_inherited_ui().caret_shape;
    let block = shape == CaretShape::Block;
    let current = style.get_inherited_text().color;
    // `auto`: the text's colour; a block caret's is translucent, so the character shows through
    // (as Chromium paints it).
    let color = match &style.get_inherited_ui().caret_color.0 {
        ColorOrAuto::Auto if block => {
            let color = current.as_srgb_color();
            color.with_alpha(color.components[3] * 0.5)
        }
        ColorOrAuto::Auto => current.as_srgb_color(),
        ColorOrAuto::Color(color) => color.resolve_to_absolute(&current).as_srgb_color(),
    };
    let text_color = match &style.get_inherited_ui().caret_color.1 {
        ColorOrAuto::Auto => None,
        ColorOrAuto::Color(color) => Some(color.resolve_to_absolute(&current).as_srgb_color()),
    };
    let font_size = style.get_font().font_size.computed_size().px() as f64;
    let line_height = {
        use style::values::computed::font::LineHeight;
        match style.get_font().line_height {
            LineHeight::Normal => font_size * 1.2,
            LineHeight::Number(number) => number.0 as f64 * font_size,
            LineHeight::Length(length) => length.0.px() as f64,
        }
    };
    let (node, empty, byte, covers) = match position {
        CaretPosition::Text { root, byte } => {
            let text = doc
                .get_node(root)?
                .element_data()?
                .inline_layout_data
                .as_ref()
                .map(|layout| layout.text.as_str())?;
            // The character after the caret, with the combining marks on it.
            let covers = block
                .then(|| {
                    let mut chars = text.get(byte..)?.char_indices();
                    let (_, first) = chars.next()?;
                    if matches!(first, '\n' | '\r' | '\u{2028}') {
                        return None;
                    }
                    let end = chars
                        .find(|&(_, c)| !is_mark(c))
                        .map_or(text.len(), |(index, _)| index);
                    Some(byte + end)
                })
                .flatten();
            (root, false, byte, covers)
        }
        CaretPosition::Empty(element) => (element, true, 0, None),
    };
    Some(CaretPaint {
        node,
        empty,
        byte,
        covers,
        shape,
        color,
        text_color,
        ch: doc.zero_advance(element) as f64,
        rtl: style.clone_direction() == Direction::Rtl,
        line_height,
    })
}

fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
        || c == '\u{200D}'
        || matches!(c as u32, 0xFE00..=0xFE0F)
}

/// The width of a bar caret (CSS px).
const BAR_WIDTH: f64 = 1.0;

/// Paints `caret` in a horizontal inline layout (device px), each line placed by
/// `line_transform`, in a content box `content_width` CSS px wide. `scale`: device px per CSS px.
/// A block caret at the end of a line stays in the content box (it shows where the line
/// overflows, as Chromium's does).
pub(crate) fn draw_text_caret<'a>(
    scene: &mut impl PaintScene,
    caret: &CaretPaint,
    layout: &'a Layout<TextBrush>,
    line_transform: impl Fn(&Line<'a, TextBrush>) -> Affine,
    content_width: f64,
    scale: f64,
) {
    let cursor = Cursor::from_byte_index(layout, caret.byte, Affinity::Downstream);
    let geometry = cursor.geometry(layout, (BAR_WIDTH * scale) as f32);
    let line = layout
        .lines()
        .find(|line| line.text_range().contains(&caret.byte))
        .or_else(|| layout.lines().last());
    let Some(line) = line else { return };
    let transform = line_transform(&line);
    // The character after the caret, when there is one on its line.
    let next = parley::Cluster::from_byte_index(layout, caret.byte)
        .filter(|cluster| !cluster.is_hard_line_break() && line.text_range().contains(&caret.byte));
    let rtl = next.as_ref().map_or(caret.rtl, |cluster| cluster.is_rtl());
    let x = geometry.x0;
    let (top, bottom) = (geometry.y0, geometry.y1);
    let span = |width: f64| match rtl {
        true => (x - width, x),
        false => (x, x + width),
    };
    let rect = match caret.shape {
        CaretShape::Block => {
            let width = caret.ch * scale;
            let (mut x0, mut x1) = span(width);
            let limit = content_width * scale;
            if !rtl && x1 > limit && width <= limit {
                (x0, x1) = ((limit - width).min(x0), limit.min(x1));
            } else if rtl && x0 < 0.0 && width <= limit {
                (x0, x1) = (0.0, width.max(x1));
            }
            Rect::new(x0, top, x1, bottom)
        }
        CaretShape::Underscore => {
            let width = next.as_ref().map_or(caret.ch * scale, |cluster| cluster.advance() as f64);
            let (x0, x1) = span(width);
            let thickness = underscore_thickness(caret, scale);
            Rect::new(x0, bottom - thickness, x1, bottom)
        }
        CaretShape::Auto | CaretShape::Bar => Rect::new(x, top, x + BAR_WIDTH * scale, bottom),
    };
    scene.fill(Fill::NonZero, transform, caret.color, None, &rect);
}

/// Paints `caret` in an empty element whose content box starts at `transform`'s origin and is
/// `content_width` CSS px wide.
pub(crate) fn draw_empty_caret(
    scene: &mut impl PaintScene,
    caret: &CaretPaint,
    transform: Affine,
    content_width: f64,
    scale: f64,
) {
    let height = caret.line_height * scale;
    let start = if caret.rtl { content_width * scale } else { 0.0 };
    let span = |width: f64| match caret.rtl {
        true => (start - width, start),
        false => (start, start + width),
    };
    let rect = match caret.shape {
        CaretShape::Block => {
            let (x0, x1) = span(caret.ch * scale);
            Rect::new(x0, 0.0, x1, height)
        }
        CaretShape::Underscore => {
            let (x0, x1) = span(caret.ch * scale);
            Rect::new(x0, height - underscore_thickness(caret, scale), x1, height)
        }
        CaretShape::Auto | CaretShape::Bar => {
            let (x0, x1) = span(BAR_WIDTH * scale);
            Rect::new(x0, 0.0, x1, height)
        }
    };
    scene.fill(Fill::NonZero, transform, caret.color, None, &rect);
}

/// The thickness of an underscore caret (device px): a fifth of the font's `0` width, at least
/// a CSS px.
fn underscore_thickness(caret: &CaretPaint, scale: f64) -> f64 {
    (caret.ch / 5.0).max(1.0) * scale
}
