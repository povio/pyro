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
/// CSS Color 4 `Mark` and `MarkText`, the target text's UA colours.
const MARK: Color = Color::from_rgb8(255, 255, 0);
const MARK_TEXT: Color = Color::from_rgb8(0, 0, 0);

/// What highlighted text is drawn with.
#[derive(Clone)]
pub(crate) struct Highlight {
    /// `None`: the text keeps its own colour (a `::selection` style that declares a
    /// `background-color` but no `color`), or the colour of the highlight below.
    pub color: Option<Color>,
    /// The element whose highlight style this is (`None`: the system's highlight), and the
    /// text decoration of that style: lines drawn over the highlighted text, as an element's own
    /// are over its content.
    pub owner: Option<NodeId>,
    pub decoration: Option<Rc<ResolvedDecoration>>,
    /// The top highlight's `text-shadow` (`None`: none).
    pub shadows: Option<Rc<Vec<crate::text::TextShadow>>>,
}

/// pyro: a highlight overlay (CSS Pseudo-Elements 4 § 3.5): custom highlights and the target
/// text under the selection.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Layer {
    Named(blitz_dom::HighlightName),
    Selection,
}

/// A highlight's background: none, a colour, or the highlight's own text colour.
#[derive(Clone, Copy, PartialEq)]
enum Background {
    Color(Color),
    Current,
}

/// One highlight's style for the text of one element.
#[derive(Clone)]
struct LayerStyle {
    /// `None`: the colour of the highlight below (or the text's own).
    color: Option<Color>,
    background: Background,
    owner: Option<NodeId>,
    decoration: Option<Rc<ResolvedDecoration>>,
    /// The nearest style of the highlight (for its `text-shadow`).
    style: Option<Style>,
}

/// The highlights an inline root is painted with, bottom first, the set drawn now (`mask`, bits
/// of `layers`), and the styles found so far.
#[derive(Default)]
pub(crate) struct HighlightCache {
    pub(crate) layers: Vec<Layer>,
    pub(crate) mask: u32,
    styles: Vec<((NodeId, usize), Option<LayerStyle>)>,
    effective: Vec<((NodeId, u32), Option<Highlight>)>,
}

impl HighlightCache {
    fn clear(&mut self) {
        self.styles.clear();
        self.effective.clear();
    }

    fn layer_style(&mut self, doc: &BaseDocument, node_id: NodeId, layer: usize) -> Option<LayerStyle> {
        if let Some((_, style)) = self.styles.iter().find(|(key, _)| *key == (node_id, layer)) {
            return style.clone();
        }
        let style = match &self.layers[layer] {
            Layer::Selection => selection_style(doc, node_id),
            Layer::Named(name) => named_style(doc, node_id, name),
        };
        self.styles.push(((node_id, layer), style.clone()));
        style
    }

    /// The colours of the highlights of `mask` over the text of `node_id`, bottom first: each
    /// layer's background, and the text colour at the top (`None`: its own).
    fn stack(&mut self, doc: &BaseDocument, node_id: NodeId, mask: u32) -> (SmallVec<[Color; 2]>, Option<Highlight>) {
        let mut backgrounds = SmallVec::new();
        let mut color: Option<Color> = None;
        let mut top: Option<LayerStyle> = None;
        let mut shadow_style: Option<Style> = None;
        for layer in 0..self.layers.len() {
            if mask & (1 << layer) == 0 {
                continue;
            }
            let Some(style) = self.layer_style(doc, node_id, layer) else { continue };
            if let Some(c) = style.color {
                color = Some(c);
            }
            shadow_style = style.style.clone();
            backgrounds.push(match style.background {
                Background::Color(c) => c,
                Background::Current => color.unwrap_or_else(|| own_color(doc, node_id)),
            });
            if style.decoration.is_some() || top.as_ref().is_none_or(|t| t.decoration.is_none()) {
                top = Some(style);
            }
        }
        // A highlight's `currentcolor` decoration takes the colour its text is drawn in.
        let highlight = top.map(|top| Highlight {
            color,
            owner: top.owner,
            decoration: top
                .decoration
                .map(|decoration| Rc::new((*decoration).clone().with_current_color(color.unwrap_or_else(|| own_color(doc, node_id))))),
            shadows: shadow_style.and_then(|style| {
                let shadows = crate::text::highlight_shadows(&style, color.unwrap_or_else(|| own_color(doc, node_id)));
                (!shadows.is_empty()).then(|| Rc::new(shadows))
            }),
        });
        (backgrounds, highlight)
    }
}

/// The colour of the text of `node_id` (its element's).
fn own_color(doc: &BaseDocument, node_id: NodeId) -> Color {
    let mut current = Some(node_id);
    while let Some(id) = current {
        let Some(node) = doc.get_node(id) else { break };
        if let Some(style) = node.primary_styles() {
            return style.clone_color().as_srgb_color();
        }
        current = node.parent;
    }
    Color::BLACK
}

/// The highlight of text whose innermost element is `node_id` with the highlights `cache.mask`
/// (`None`: none applies), found once per paint of an inline root (`cache`).
pub(crate) fn highlight_of(doc: &BaseDocument, node_id: NodeId, cache: &mut HighlightCache) -> Option<Highlight> {
    let mask = cache.mask;
    if let Some((_, highlight)) = cache.effective.iter().find(|(key, _)| *key == (node_id, mask)) {
        return highlight.clone();
    }
    let (_, highlight) = cache.stack(doc, node_id, mask);
    cache.effective.push(((node_id, mask), highlight.clone()));
    highlight
}

type Style = style::servo_arc::Arc<style::properties::ComputedValues>;

/// A highlight's colours from the nearest styles (the element's, its parent's, ...) that declare
/// them, each property on its own (WPT `cascade-highlight-005`); `defaults` when none declares
/// either (a UA's colours).
fn resolve_layer(
    doc: &BaseDocument,
    node_id: NodeId,
    style_of: impl Fn(NodeId) -> Option<Style>,
    defaults: Option<(Color, Color)>,
) -> LayerStyle {
    use style::properties::LonghandId;
    let mut nearest: Option<(NodeId, Style)> = None;
    let mut color: Option<Style> = None;
    let mut background: Option<Style> = None;
    let mut current = Some(node_id);
    while let Some(id) = current {
        let Some(node) = doc.get_node(id) else { break };
        if node.is_element()
            && let Some(style) = style_of(id)
        {
            if color.is_none() && doc.style_declares(&style, &[LonghandId::Color]) {
                color = Some(style.clone());
            }
            if background.is_none() && doc.style_declares(&style, &[LonghandId::BackgroundColor]) {
                background = Some(style.clone());
            }
            nearest.get_or_insert((id, style));
        }
        if color.is_some() && background.is_some() {
            break;
        }
        current = node.parent;
    }
    let decoration = nearest.as_ref().and_then(|(_, style)| own_decoration(style));
    let owner = nearest.as_ref().map(|(id, _)| *id);
    let nearest_style = nearest.map(|(_, style)| style);
    match (color, background, defaults) {
        (None, None, Some((background, text))) => LayerStyle {
            color: Some(text),
            background: Background::Color(background),
            owner,
            decoration: decoration.map(|decoration| Rc::new(decoration.with_current_color(text))),
            style: nearest_style,
        },
        (color, background, _) => {
            let color = color.and_then(|style| {
                (!doc.style_declares_current_color(&style, LonghandId::Color)).then(|| style.clone_color().as_srgb_color())
            });
            let background = match background {
                Some(style) => {
                    let value = &style.get_background().background_color;
                    if value.is_currentcolor() {
                        Background::Current
                    } else {
                        Background::Color(value.resolve_to_absolute(&style.clone_color()).as_srgb_color())
                    }
                }
                None => Background::Color(Color::TRANSPARENT),
            };
            LayerStyle { color, background, owner, decoration: decoration.map(Rc::new), style: nearest_style }
        }
    }
}

/// The `::selection` highlight of text whose innermost element is `node_id` (`None`: the text is
/// not selectable). With no `::selection` style that declares `color` or `background-color`,
/// the system's colours (CSS Pseudo-Elements 4 § 3.4, as Chromium).
fn selection_style(doc: &BaseDocument, node_id: NodeId) -> Option<LayerStyle> {
    use style::values::computed::UserSelect;
    let mut current = Some(node_id);
    while let Some(id) = current {
        let Some(node) = doc.get_node(id) else { break };
        // `auto` is what the parent's is (`none` stays `none` in the content of an element).
        match node.primary_styles().map(|style| style.clone_user_select()) {
            Some(UserSelect::None) => return None,
            Some(UserSelect::Auto) | None => {}
            Some(_) => break,
        }
        current = node.parent;
    }
    let style_of = |id: NodeId| {
        let node = doc.get_node(id)?;
        let eager = node.try_stylo_element_data().and_then(|data| {
            data.get().and_then(|data| data.styles.pseudos.get(&PseudoElement::Selection).cloned())
        })?;
        // pyro: container units (`cqh`, ...) in an eager style resolve before layout gives the
        // containers sizes: computed again now (WPT css-conditional
        // `container-units-selection-pseudo`).
        // pyro: and a visited link's, whose eager style matched `:link` (`selection_pseudo_style`
        // takes it as visited).
        use style::computed_value_flags::ComputedValueFlags;
        if doc.is_visited_link(id) {
            return doc.selection_pseudo_style(id);
        }
        if eager.flags.contains(ComputedValueFlags::USES_CONTAINER_UNITS) {
            if let Some(again) = doc.selection_pseudo_style(id) {
                return Some(again);
            }
        }
        Some(eager)
    };
    Some(resolve_layer(doc, node_id, style_of, Some((HIGHLIGHT, HIGHLIGHT_TEXT))))
}

/// pyro: the colours a text control's selection is painted in (its `::selection` style, as on
/// page text): the background, and the selected text's colour (`None`: its own). `text` is the
/// control's text colour, for a `currentcolor` background.
pub(crate) fn control_selection_colors(doc: &BaseDocument, node_id: NodeId, text: Color) -> (Color, Option<Color>) {
    let Some(layer) = selection_style(doc, node_id) else {
        return (Color::TRANSPARENT, None);
    };
    let background = match layer.background {
        Background::Color(color) => color,
        Background::Current => layer.color.unwrap_or(text),
    };
    (background, layer.color)
}

/// A custom highlight's or the target text's highlight of the text of `node_id`. The target text
/// has the UA's `Mark` on `MarkText` unless a style declares its colours; a custom highlight
/// draws nothing of its own then.
fn named_style(doc: &BaseDocument, node_id: NodeId, name: &blitz_dom::HighlightName) -> Option<LayerStyle> {
    let style_of = |id: NodeId| doc.highlight_pseudo_style(id, name);
    let defaults = match name {
        blitz_dom::HighlightName::TargetText => Some((MARK, MARK_TEXT)),
        blitz_dom::HighlightName::Custom(_) => None,
    };
    Some(resolve_layer(doc, node_id, style_of, defaults))
}

/// A highlighted stretch of a line: from `x0` to `x1` (layout x), across `top..bottom` (layout
/// y: the line box, grown to the content areas of its runs), with the highlights over it
/// (`mask`) and their backgrounds, bottom first.
#[derive(Clone, Debug)]
struct Piece {
    x0: f32,
    x1: f32,
    top: f32,
    bottom: f32,
    mask: u32,
    backgrounds: SmallVec<[Color; 2]>,
}

/// The highlighted stretches of `line` (`ranges`: each a highlight's index and bytes of the
/// layout's text), in visual order; atoms of one run next to each other with the same highlights
/// make one.
fn line_pieces(
    doc: &BaseDocument,
    line: &Line<'_, TextBrush>,
    ranges: &[(usize, usize, usize)],
    cache: &mut HighlightCache,
) -> SmallVec<[Piece; 4]> {
    let mut pieces: SmallVec<[Piece; 4]> = SmallVec::new();
    let metrics = line.metrics();
    let range = line.text_range();
    if !ranges.iter().any(|&(_, start, end)| range.end > start && range.start < end) {
        return pieces;
    }
    for item in line.items() {
        let PositionedLayoutItem::GlyphRun(glyph_run) = item else { continue };
        let run_range = glyph_run.run().text_range();
        if !ranges.iter().any(|&(_, start, end)| run_range.end > start && run_range.start < end) {
            continue;
        }
        let node_id = glyph_run.style().brush.id;
        let (content_top, content_bottom) = line.span_content_area(glyph_run.style_index());
        let top = content_top.min(metrics.block_min_coord);
        let bottom = content_bottom.max(metrics.block_max_coord);
        let advances: SmallVec<[f32; 64]> = glyph_run.glyphs().map(|glyph| glyph.advance).collect();
        let (mut pen, mut index) = (glyph_run.offset(), 0);
        for (text, count) in glyph_run.atom_texts() {
            let Some(glyphs) = advances.get(index..index + count) else { break };
            let advance: f32 = glyphs.iter().sum();
            let mut mask = 0u32;
            for &(layer, start, end) in ranges {
                if text.start < end && text.end > start {
                    mask |= 1 << layer;
                }
            }
            // Highlights that do not apply to the text (`user-select: none`) leave it.
            if mask != 0 {
                for layer in 0..cache.layers.len() {
                    if mask & (1 << layer) != 0 && cache.layer_style(doc, node_id, layer).is_none() {
                        mask &= !(1 << layer);
                    }
                }
            }
            if mask != 0 && advance > 0.0 {
                let (backgrounds, _) = cache.stack(doc, node_id, mask);
                match pieces.last_mut() {
                    Some(last) if (last.x1 - pen).abs() < 0.01 && last.mask == mask && last.backgrounds == backgrounds => {
                        last.x1 = pen + advance;
                        last.top = last.top.min(top);
                        last.bottom = last.bottom.max(bottom);
                    }
                    _ => pieces.push(Piece { x0: pen, x1: pen + advance, top, bottom, mask, backgrounds }),
                }
            }
            pen += advance;
            index += count;
        }
    }
    pieces
}

/// Strokes an inline root's text with its highlights (`ranges`: each a highlight's index in
/// `layers`, bottom first, and bytes of its text; see the module): line by line, the highlights'
/// backgrounds, then the text outside the highlighted stretches, then the text in them in the
/// colours of their highlights. `transform` is the root's text's, `tops` where its lines go;
/// `context` has the text's shift and line tops set.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stroke_highlighted_text<S: PaintScene>(
    scene: &mut S,
    doc: &BaseDocument,
    root: NodeId,
    layout: &Layout<TextBrush>,
    text: &str,
    line_transform: impl Fn(usize) -> Affine,
    tops: Option<LineTops>,
    options: TextPaintOptions,
    layers: &[Layer],
    ranges: &[(usize, usize, usize)],
    context: &mut DrawTextContext,
    // pyro: how the runs of a vertical line sit in it (lines turned by `line_transform`).
    vertical: impl Fn(&parley::Line<'_, TextBrush>) -> Option<crate::text::VerticalLine>,
) {
    let scale = options.scale;
    let shift = context.text_shift;
    let mut cache = std::mem::take(&mut context.highlights);
    cache.clear();
    cache.layers.clear();
    cache.layers.extend_from_slice(layers);
    for (index, line) in layout.lines().enumerate() {
        let transform = line_transform(index);
        let turned = vertical(&line);
        let [a, b, c, d, tx, ty] = transform.as_coeffs();
        let translation_only = (a, b, c, d) == (1.0, 0.0, 0.0, 1.0);
        let pieces = line_pieces(doc, &line, ranges, &mut cache);
        if pieces.is_empty() {
            context.highlights = std::mem::take(&mut cache);
            stroke_text(
                scene,
                std::iter::once(line),
                doc,
                transform,
                options,
                root,
                Some(text),
                context,
                turned,
                Some(layout),
            );
            cache = std::mem::take(&mut context.highlights);
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
        // The shadows the text casts in the stretches, under their backgrounds (as Chromium:
        // the highlights paint over the originating text's shadows).
        const FAR: f64 = 1e6;
        if options.shadows {
            let mut inside = BezPath::new();
            for rect in &rects {
                inside.extend(Rect::new(rect.x0, -FAR, rect.x1, FAR).path_elements(0.1));
            }
            context.highlights = std::mem::take(&mut cache);
            context.shadows_only = true;
            scene.push_clip_layer(Fill::NonZero, transform, &inside);
            stroke_text(
                scene,
                std::iter::once(line),
                doc,
                transform,
                options,
                root,
                Some(text),
                context,
                turned,
                Some(layout),
            );
            scene.pop_layer();
            context.shadows_only = false;
            cache = std::mem::take(&mut context.highlights);
        }
        // pyro: the glyphs outside the stretches where they reach into one (an italic glyph's
        // overhang), under its background: Chromium paints the highlight over them (WPT css-pseudo
        // `selection-background-painting-order`). Not in vertical lines, whose turned runs place
        // glyphs off their layout positions (`glyph_spans` would take selected ones too).
        if turned.is_none() {
            let mut spans: SmallVec<[(f32, f32); 4]> = pieces.iter().map(|p| (p.x0, p.x1)).collect();
            spans.sort_by(|p, q| p.0.total_cmp(&q.0));
            let mut gaps: SmallVec<[(f32, f32); 4]> = SmallVec::new();
            let mut x = f32::NEG_INFINITY;
            for (x0, x1) in &spans {
                if *x0 > x {
                    gaps.push((x, *x0));
                }
                x = x.max(*x1);
            }
            gaps.push((x, f32::INFINITY));
            let mut inside = BezPath::new();
            for rect in &rects {
                inside.extend(Rect::new(rect.x0, -FAR, rect.x1, FAR).path_elements(0.1));
            }
            scene.push_clip_layer(Fill::NonZero, transform, &inside);
            context.highlights = std::mem::take(&mut cache);
            context.glyph_spans = Some(gaps);
            context.glyphs_only = true;
            stroke_text(
                scene,
                std::iter::once(line.clone()),
                doc,
                transform,
                TextPaintOptions { shadows: false, ..options },
                root,
                Some(text),
                context,
                turned,
                Some(layout),
            );
            context.glyph_spans = None;
            context.glyphs_only = false;
            cache = std::mem::take(&mut context.highlights);
            scene.pop_layer();
        }
        for (piece, rect) in pieces.iter().zip(&rects) {
            for background in &piece.backgrounds {
                if background.components[3] > 0.0 && rect.width() > 0.0 {
                    scene.fill(Fill::NonZero, transform, *background, None, rect);
                }
            }
        }
        // The text outside the stretches (across the whole height: glyphs and decorations reach
        // past the line box), then inside them, a pass per set of highlights.
        let mut outside = BezPath::new();
        let mut x = -FAR;
        let mut sorted: SmallVec<[(Rect, u32); 4]> = rects.iter().copied().zip(pieces.iter().map(|p| p.mask)).collect();
        sorted.sort_by(|p, q| p.0.x0.total_cmp(&q.0.x0));
        for (rect, _) in &sorted {
            if rect.x0 > x {
                outside.extend(Rect::new(x, -FAR, rect.x0, FAR).path_elements(0.1));
            }
            x = x.max(rect.x1);
        }
        outside.extend(Rect::new(x, -FAR, FAR, FAR).path_elements(0.1));
        context.highlights = std::mem::take(&mut cache);
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
            turned,
            Some(layout),
        );
        scene.pop_layer();
        let mut masks: SmallVec<[u32; 4]> = sorted.iter().map(|(_, mask)| *mask).collect();
        masks.sort_unstable();
        masks.dedup();
        for mask in masks {
            let mut inside = BezPath::new();
            for (rect, _) in sorted.iter().filter(|(_, m)| *m == mask) {
                inside.extend(Rect::new(rect.x0, -FAR, rect.x1, FAR).path_elements(0.1));
            }
            context.highlights.mask = mask;
            context.highlighting = true;
            // The highlight's shadows, cast by its glyphs alone (unclipped), then its text.
            if options.shadows {
                context.shadows_only = true;
                context.glyph_spans = Some(
                    pieces.iter().filter(|piece| piece.mask == mask).map(|piece| (piece.x0, piece.x1)).collect(),
                );
                stroke_text(
                    scene,
                    std::iter::once(line),
                    doc,
                    transform,
                    options,
                    root,
                    Some(text),
                    context,
                    turned,
                    Some(layout),
                );
                context.shadows_only = false;
                context.glyph_spans = None;
            }
            scene.push_clip_layer(Fill::NonZero, transform, &inside);
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
                turned,
                Some(layout),
            );
            context.highlighting = false;
            scene.pop_layer();
        }
        cache = std::mem::take(&mut context.highlights);
    }
    context.highlights = cache;
}
