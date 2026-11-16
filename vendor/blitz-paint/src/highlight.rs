//! pyro: the document selection painted as Chromium paints it (CSS Pseudo-Elements 4 § 3,
//! highlight painting).
//!
//! Behind the selected text goes the highlight's background over each selected stretch of a line,
//! from the line box's top to its bottom, or to the text's content area where that reaches
//! further (Blink's `ExpandSelectionRectToLineHeight`), its edges on whole CSS px as an inline
//! box's are. The selected glyphs are drawn in the highlight's colour, and so are the decorations
//! whose colour is the text's (`currentcolor`); decorations of another colour keep it, and the
//! line stays one line across the selection's edges. A `::selection` style's own text decoration
//! is drawn over the selected text, as a decoration of the element whose style it is. The highlight is the `::selection` style of
//! the nearest element that has one (the text's element, its parent, ...), else the system's:
//! `HighlightText` on `Highlight`, also for a `::selection` style that declares neither `color` nor
//! `background-color`. Text that `user-select: none` keeps from being selected is not
//! highlighted.
//!
//! Each line holding selected text is drawn twice, clipped: everything but the selected
//! stretches as usual, then the stretches in the highlight's colours (without text shadows).
//! Screenshots only: a recording paint (the PDF's) draws no selection, as Chromium prints none.

use anyrender::PaintScene;
use blitz_dom::CopyStyleGetters;
use blitz_dom::{BaseDocument, NodeId, node::TextBrush};
use kurbo::{Affine, BezPath, Rect, Shape};
use parley::{Layout, Line, PositionedLayoutItem};
use peniko::Fill;
use smallvec::SmallVec;
use style::selector_parser::PseudoElement;

use crate::color::{Color, ToColorColor as _};
use std::rc::Rc;

use crate::text::{DrawTextContext, LineTops, ResolvedDecoration, TextPaintOptions, own_decoration, stroke_text};

/// The system highlight colours, CSS Color 4 `Highlight` and `HighlightText` as Stylo's servo
/// device computes them (light colour scheme), so that text selected without a `::selection`
/// style looks like text styled with them (WPT `system-color-hightlights-vs-getSelection-*`).
const HIGHLIGHT: Color = Color::from_rgb8(0, 65, 198);
const HIGHLIGHT_TEXT: Color = Color::from_rgb8(0, 0, 0);

/// What selected text is drawn with.
#[derive(Clone)]
pub(crate) struct Highlight {
    pub background: Color,
    /// `None`: the text keeps its own colour (a `::selection` style that declares a
    /// `background-color` but no `color`).
    pub color: Option<Color>,
    /// The element whose `::selection` style this is (`None`: the system's highlight), and the
    /// text decoration of that style: lines drawn over the selected text, as an element's own
    /// are over its content.
    pub owner: Option<NodeId>,
    pub decoration: Option<Rc<ResolvedDecoration>>,
}

/// The highlight of selected text whose innermost element is `node_id` (`None`: the text is not
/// selectable), found once per paint of an inline root (`cache`).
pub(crate) fn highlight_of(
    doc: &BaseDocument,
    node_id: NodeId,
    cache: &mut Vec<(NodeId, Option<Highlight>)>,
) -> Option<Highlight> {
    if let Some((_, highlight)) = cache.iter().find(|(id, _)| *id == node_id) {
        return highlight.clone();
    }
    use style::properties::LonghandId;
    use style::values::computed::UserSelect;
    // pyro: the nearest `::selection` style that declares `color` or `background-color` is the
    // highlight's (an ancestor's when the nearer ones declare neither, WPT
    // `highlight-paired-cascade-008`); with none, the system's colours are (CSS Pseudo-Elements 4
    // § 3.4, as Chromium). The nearest style's own decoration is drawn either way.
    let declares = |style: &style::servo_arc::Arc<style::properties::ComputedValues>, longhand| {
        doc.style_declares(style, &[longhand])
    };
    let mut nearest: Option<(NodeId, style::servo_arc::Arc<style::properties::ComputedValues>)> = None;
    let mut declaring = None;
    let mut selectable = None;
    let mut current = Some(node_id);
    while let Some(id) = current {
        let Some(node) = doc.get_node(id) else { break };
        if declaring.is_none() && node.is_element() {
            let style = node.try_stylo_element_data().and_then(|data| {
                data.get().and_then(|data| data.styles.pseudos.get(&PseudoElement::Selection).cloned())
            });
            if let Some(style) = style {
                let color = declares(&style, LonghandId::Color);
                if color || declares(&style, LonghandId::BackgroundColor) {
                    declaring = Some((style.clone(), color));
                }
                nearest.get_or_insert((id, style));
            }
        }
        if selectable.is_none() {
            // `auto` is what the parent's is (`none` stays `none` in the content of an element).
            match node.primary_styles().map(|style| style.clone_user_select()) {
                Some(UserSelect::None) => selectable = Some(false),
                Some(UserSelect::Auto) | None => {}
                Some(_) => selectable = Some(true),
            }
        }
        if declaring.is_some() && selectable.is_some() {
            break;
        }
        current = node.parent;
    }
    let decoration = nearest.as_ref().and_then(|(_, style)| own_decoration(style));
    let owner = nearest.as_ref().map(|(id, _)| *id);
    let highlight = (selectable != Some(false)).then(|| match declaring {
        Some((style, declares_color)) => {
            let color = style.clone_color();
            Highlight {
                background: style.get_background().background_color.resolve_to_absolute(&color).as_srgb_color(),
                color: declares_color.then(|| color.as_srgb_color()),
                owner,
                decoration: decoration.map(Rc::new),
            }
        }
        None => Highlight {
            background: HIGHLIGHT,
            color: Some(HIGHLIGHT_TEXT),
            owner,
            decoration: decoration.map(|decoration| Rc::new(decoration.with_current_color(HIGHLIGHT_TEXT))),
        },
    });
    cache.push((node_id, highlight.clone()));
    highlight
}

/// A selected stretch of a line: from `x0` to `x1` (layout x), across `top..bottom` (layout y:
/// the line box, grown to the content areas of its runs), with its background.
#[derive(Clone, Copy, Debug)]
struct Piece {
    x0: f32,
    x1: f32,
    top: f32,
    bottom: f32,
    background: Color,
}

/// The selected stretches of `line` (`selection`: bytes of the layout's text), in visual order;
/// atoms of one run next to each other with the same highlight make one.
fn line_pieces(
    doc: &BaseDocument,
    line: &Line<'_, TextBrush>,
    selection: std::ops::Range<usize>,
    cache: &mut Vec<(NodeId, Option<Highlight>)>,
) -> SmallVec<[Piece; 4]> {
    let mut pieces: SmallVec<[Piece; 4]> = SmallVec::new();
    let metrics = line.metrics();
    let range = line.text_range();
    if range.end <= selection.start || range.start >= selection.end {
        return pieces;
    }
    for item in line.items() {
        let PositionedLayoutItem::GlyphRun(glyph_run) = item else { continue };
        let run_range = glyph_run.run().text_range();
        if run_range.end <= selection.start || run_range.start >= selection.end {
            continue;
        }
        let Some(highlight) = highlight_of(doc, glyph_run.style().brush.id, cache) else { continue };
        let (content_top, content_bottom) = line.span_content_area(glyph_run.style_index());
        let top = content_top.min(metrics.block_min_coord);
        let bottom = content_bottom.max(metrics.block_max_coord);
        let advances: SmallVec<[f32; 64]> = glyph_run.glyphs().map(|glyph| glyph.advance).collect();
        let (mut pen, mut index) = (glyph_run.offset(), 0);
        for (text, count) in glyph_run.atom_texts() {
            let Some(glyphs) = advances.get(index..index + count) else { break };
            let advance: f32 = glyphs.iter().sum();
            if text.start < selection.end && text.end > selection.start && advance > 0.0 {
                match pieces.last_mut() {
                    Some(last) if (last.x1 - pen).abs() < 0.01 && last.background == highlight.background => {
                        last.x1 = pen + advance;
                        last.top = last.top.min(top);
                        last.bottom = last.bottom.max(bottom);
                    }
                    _ => {
                        pieces.push(Piece { x0: pen, x1: pen + advance, top, bottom, background: highlight.background })
                    }
                }
            }
            pen += advance;
            index += count;
        }
    }
    pieces
}

/// Strokes an inline root's text with the selection `selection` (bytes of its text) highlighted
/// (see the module): line by line, the highlight's backgrounds, then the text outside the
/// selected stretches, then the text in them in the highlight's colours. `transform` is the
/// root's text's, `tops` where its lines go; `context` has the text's shift and line tops set.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stroke_highlighted_text<S: PaintScene>(
    scene: &mut S,
    doc: &BaseDocument,
    root: NodeId,
    layout: &Layout<TextBrush>,
    text: &str,
    transform: Affine,
    tops: Option<LineTops>,
    options: TextPaintOptions,
    selection: (usize, usize),
    context: &mut DrawTextContext,
) {
    let scale = options.scale;
    let shift = context.text_shift;
    let mut cache = std::mem::take(&mut context.highlights);
    cache.clear();
    let [a, b, c, d, tx, ty] = transform.as_coeffs();
    let translation_only = (a, b, c, d) == (1.0, 0.0, 0.0, 1.0);
    for line in layout.lines() {
        let pieces = line_pieces(doc, &line, selection.0..selection.1, &mut cache);
        if pieces.is_empty() {
            stroke_text(
                scene,
                std::iter::once(line),
                doc,
                transform,
                options,
                root,
                Some(text),
                context,
                None,
                Some(layout),
            );
            continue;
        }
        let line_shift = tops.map_or(0.0, |tops| tops.shift(line.metrics()));
        // A piece's rectangle in the space of `transform`: on whole CSS px where the text is only
        // moved, as inline boxes are placed (`inline_box_fragments`).
        let rect = |piece: &Piece| {
            let rect = Rect::new(
                f64::from(piece.x0) + shift,
                f64::from(piece.top) + line_shift,
                f64::from(piece.x1) + shift,
                f64::from(piece.bottom) + line_shift,
            );
            if translation_only {
                let snap = |v: f64, t: f64| ((t + v) / scale).round() * scale - t;
                Rect::new(snap(rect.x0, tx), snap(rect.y0, ty), snap(rect.x1, tx), snap(rect.y1, ty))
            } else {
                rect
            }
        };
        let rects: SmallVec<[Rect; 4]> = pieces.iter().map(rect).collect();
        for (piece, rect) in pieces.iter().zip(&rects) {
            if piece.background.components[3] > 0.0 && rect.width() > 0.0 {
                scene.fill(Fill::NonZero, transform, piece.background, None, rect);
            }
        }
        // The text outside the stretches (across the whole height: glyphs and decorations reach
        // past the line box), then inside them.
        const FAR: f64 = 1e6;
        let mut outside = BezPath::new();
        let mut x = -FAR;
        let mut sorted = rects.clone();
        sorted.sort_by(|p, q| p.x0.total_cmp(&q.x0));
        for rect in &sorted {
            if rect.x0 > x {
                outside.extend(Rect::new(x, -FAR, rect.x0, FAR).path_elements(0.1));
            }
            x = x.max(rect.x1);
        }
        outside.extend(Rect::new(x, -FAR, FAR, FAR).path_elements(0.1));
        let mut inside = BezPath::new();
        for rect in &sorted {
            inside.extend(Rect::new(rect.x0, -FAR, rect.x1, FAR).path_elements(0.1));
        }
        scene.push_clip_layer(Fill::NonZero, transform, &outside);
        stroke_text(
            scene,
            std::iter::once(line),
            doc,
            transform,
            options,
            root,
            Some(text),
            context,
            None,
            Some(layout),
        );
        scene.pop_layer();
        scene.push_clip_layer(Fill::NonZero, transform, &inside);
        context.highlights = std::mem::take(&mut cache);
        context.highlighting = true;
        let selected = TextPaintOptions { shadows: false, ..options };
        stroke_text(
            scene,
            std::iter::once(line),
            doc,
            transform,
            selected,
            root,
            Some(text),
            context,
            None,
            Some(layout),
        );
        context.highlighting = false;
        cache = std::mem::take(&mut context.highlights);
        scene.pop_layer();
    }
    context.highlights = cache;
}
