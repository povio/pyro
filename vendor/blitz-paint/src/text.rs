use anyrender::PaintScene;
use anyrender::filters::{Filter, FilterEffect};
use blitz_dom::CopyStyleGetters;
use blitz_dom::node::{TextAnnotation, TextAnnotationKind, can_take_emphasis_mark};
use blitz_dom::{BaseDocument, NodeId, node::TextBrush, util::ToColorColor};
use kurbo::{Affine, BezPath, Cap, Circle, Rect, Stroke, Vec2};
use parley::{Affinity, Cursor, Layout, Line, PositionedLayoutItem, Selection};
use peniko::{Fill, Mix};
use smallvec::SmallVec;
use std::rc::Rc;
use std::sync::Arc;
use style::properties::generated::longhands::text_decoration_style::computed_value::T as TextDecorationStyle;
use style::values::computed::{
    Length, LengthPercentage, TextDecorationLength, TextDecorationLine, TextUnderlinePosition,
};
use style::values::computed::TextDecorationSkipInk;
use style::values::generics::text::{GenericTextDecorationInset, GenericTextDecorationLength};

use crate::color::{Color, ToColorColor as _};
use crate::{CORE_TEXT_GLYPHS, FONT_EMBOLDEN_ENABLED, SELECTION_COLOR};

/// How the runs of one line of a vertical writing mode sit in it. The line is drawn turned a quarter
/// clockwise (see `ElementCx::draw_vertical_inline_layout`), so a run's block offset moves it across the line.
#[derive(Clone, Copy)]
pub(crate) struct VerticalLine {
    /// The line's central baseline, which `text-orientation: mixed` and `upright` align runs to; `sideways`
    /// keeps the alphabetic baseline.
    central: Option<f32>,
    /// `text-orientation: upright`: every glyph stands upright.
    all_upright: bool,
}

impl VerticalLine {
    pub(crate) fn new(line: &Line<'_, TextBrush>, sideways: bool, all_upright: bool) -> Self {
        // A fragment's box is its whole-pixel ascent and descent, around the middle of which the
        // tallest fragment in the line (the one that sets its height) has the central baseline.
        let central = (!sideways)
            .then(|| {
                line.items()
                    .filter_map(|item| match item {
                        PositionedLayoutItem::GlyphRun(run) => Some(run),
                        _ => None,
                    })
                    .max_by_key(|run| {
                        let metrics = run.run().font_metrics();
                        (metrics.ascent.round() + metrics.descent.round()) as i64
                    })
                    .map(|run| {
                        let metrics = run.run().font_metrics();
                        run.baseline() - (metrics.ascent.round() - metrics.descent.round()) / 2.0
                    })
            })
            .flatten();
        Self { central, all_upright }
    }

    /// Moves a run's fragment box from the alphabetic baseline to the central one.
    fn fragment_shift(self, run: &parley::GlyphRun<'_, TextBrush>) -> Affine {
        let Some(central) = self.central else {
            return Affine::IDENTITY;
        };
        let metrics = run.run().font_metrics();
        let at = run.baseline() - (metrics.ascent.round() - metrics.descent.round()) / 2.0;
        Affine::translate((0.0, (central - at) as f64))
    }

    /// Where a run's glyphs go: the shift of the run, and for one standing upright each glyph's
    /// transform. An upright glyph is turned back around the middle of its em box. Its advance is the one shaped horizontally: right for the full-width glyphs of CJK text,
    /// not for Latin (which in Chromium advances by the font's height), and glyphs have no `vert` forms.
    fn run_placement(
        self,
        run: &parley::GlyphRun<'_, TextBrush>,
        source: Option<&str>,
    ) -> (Affine, Option<Affine>) {
        let Some(central) = self.central else {
            return (Affine::IDENTITY, None);
        };
        let metrics = run.run().font_metrics();
        let upright = self.all_upright
            || source
                .and_then(|text| text.get(run.run().text_range())?.chars().next())
                .is_some_and(is_upright_char);
        if !upright {
            return (self.fragment_shift(run), None);
        }
        let em = run.run().font_size();
        let height = (metrics.ascent + metrics.descent).max(1.0);
        let ascent = (em * metrics.ascent / height) as f64;
        (
            Affine::translate((0.0, (central + em / 2.0 - run.baseline()) as f64)),
            Some(Affine::new([0.0, -1.0, 1.0, 0.0, ascent, 0.0])),
        )
    }
}

/// Whether `text-orientation: mixed` stands a character upright (UTR #50's `Vertical_Orientation`
/// other than R, by block).
fn is_upright_char(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF
        | 0x2E80..=0x4DBF
        | 0x4E00..=0xA4CF
        | 0xAC00..=0xD7FF
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F000..=0x1FAFF
        | 0x20000..=0x3FFFF)
}

/// Draw the backgrounds of inline elements (e.g. `<span style="background: ...">`).
///
/// Each glyph run carries the node id of the innermost inline element it belongs to
/// (via its brush). We look up that node's `background-color` and, if non-transparent,
/// fill a rectangle covering the run's advance and its font's ascent/descent so that the
/// background sits behind the text.
///
/// The inline root's own background is painted separately (as a normal block box), so
/// runs belonging to the root are skipped to avoid drawing it twice.
pub(crate) fn draw_inline_backgrounds<'a>(
    scene: &mut impl PaintScene,
    lines: impl Iterator<Item = Line<'a, TextBrush>>,
    doc: &BaseDocument,
    transform: Affine,
    inline_root_id: NodeId,
    vertical: Option<VerticalLine>,
) {
    for line in lines {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                continue;
            };
            let transform = match vertical {
                Some(vertical) => transform * vertical.fragment_shift(&glyph_run),
                None => transform,
            };

            let node_id = glyph_run.style().brush.id;
            if node_id == inline_root_id {
                continue;
            }

            // Text brushes point at the innermost inline. Its transparent
            // background must not hide backgrounds on enclosing inline boxes.
            let mut backgrounds = Vec::new();
            let mut current = Some(node_id);
            while let Some(id) = current {
                if id == inline_root_id {
                    break;
                }
                let Some(node) = doc.get_node(id) else {
                    break;
                };
                if let Some(styles) = node.primary_styles() {
                    let display = styles.clone_display();
                    if display.is_contents() {
                        current = node.parent;
                        continue;
                    }
                    // Anonymous inline roots are not in the DOM parent chain.
                    // Stop at the first block/atomic box instead of repainting it.
                    if !display.is_inline_flow() {
                        break;
                    }
                    let current_color = styles.clone_color();
                    let color = styles
                        .get_background()
                        .background_color
                        .resolve_to_absolute(&current_color)
                        .as_srgb_color();
                    if color != Color::TRANSPARENT {
                        backgrounds.push(color);
                    }
                }
                current = node.parent;
            }
            if backgrounds.is_empty() {
                continue;
            }

            let metrics = glyph_run.run().font_metrics();
            let x = glyph_run.offset() as f64;
            let w = glyph_run.advance() as f64;
            let baseline = glyph_run.baseline() as f64;
            let y0 = baseline - metrics.ascent as f64;
            let y1 = baseline + metrics.descent as f64;
            let rect = Rect::new(x, y0, x + w, y1);

            for color in backgrounds.into_iter().rev() {
                scene.fill(Fill::NonZero, transform, color, None, &rect);
            }
        }
    }
}

/// Mirrors Blink's `SelectBestDashGap`: choose the gap length (as close as possible to
/// `gap_length`) that fits a whole number of `dash_length` dashes across `stroke_length`,
/// so dashes are evenly distributed and a dash lands at each end of the line.
fn select_best_dash_gap(stroke_length: f64, dash_length: f64, gap_length: f64) -> f64 {
    let available_length = stroke_length + gap_length;
    let min_num_dashes = (available_length / (dash_length + gap_length)).floor();
    let max_num_dashes = min_num_dashes + 1.0;
    let min_num_gaps = min_num_dashes - 1.0;
    let max_num_gaps = max_num_dashes - 1.0;
    if min_num_gaps < 1.0 {
        return gap_length;
    }
    let min_gap = (stroke_length - min_num_dashes * dash_length) / min_num_gaps;
    let max_gap = (stroke_length - max_num_dashes * dash_length) / max_num_gaps;
    if max_gap <= 0.0 || (min_gap - gap_length).abs() < (max_gap - gap_length).abs() {
        min_gap
    } else {
        max_gap
    }
}

/// A text decoration resolved from a single "decorating box" (an element that
/// sets `text-decoration-line`), ready to be drawn over a glyph run. Because the
/// `text-decoration-*` properties do not inherit, decorations are propagated from
/// ancestors rather than inherited, so a run may be covered by several of these.
///
/// Only the run-independent (style-derived) values are stored here so an instance
/// can be cached on the ancestor stack and reused across every run within the same
/// decorating box. The `text-decoration-inset` and `text-underline-offset` values
/// depend on the run's advance/font-size and are resolved lazily when drawing.
#[derive(Clone)]
struct ResolvedDecoration {
    line: TextDecorationLine,
    style: TextDecorationStyle,
    color: Color,
    thickness: TextDecorationLength,
    /// `text-underline-offset`; `None` for `auto`. Resolved per run.
    underline_offset: Option<LengthPercentage>,
    /// Whether `text-underline-position: under` is in effect.
    underline_under: bool,
    /// Whether `text-underline-position: from-font` is in effect.
    underline_from_font: bool,
    /// `text-decoration-inset`. Resolved per run.
    inset: GenericTextDecorationInset<LengthPercentage>,
    /// pyro: `text-decoration-skip-ink`: underlines and overlines leave gaps where glyphs cross
    /// them, but for `auto` not where CJK glyphs do.
    skip_ink: TextDecorationSkipInk,
}

/// An element on the current ancestor path (inline root -> run node), caching the
/// values resolved from its computed style so descending into the same node across
/// consecutive runs doesn't re-resolve them.
struct DecorationStackEntry {
    node_id: NodeId,
    /// This node's (inherited) text fill colour, used for the glyphs of runs whose
    /// innermost node is this one.
    fill_color: Color,
    /// The decoration this node introduces as a decorating box, if any.
    decoration: Option<ResolvedDecoration>,
    /// pyro: this node's (inherited) `text-shadow`, for runs whose innermost node is this one.
    shadows: Vec<TextShadow>,
}

/// pyro: one `text-shadow`, resolved: its colour (`currentColor` is the text's `color`) and its
/// offset and blur radius in CSS px.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TextShadow {
    color: Color,
    x: f64,
    y: f64,
    blur: f64,
}

/// pyro: draws `shadow` of what `draw` draws (with the colour and transform it is given),
/// moved by its offset and in its colour, from the shapes alone (Chromium's text shadow ignores
/// the text's own alpha: transparent text casts one). A shadow with a blur radius is drawn in a
/// layer that blurs it by a Gaussian of half the radius (Chromium's `BlurRadiusToStdDev`),
/// clipped to `ink` (where `draw` paints, in the space of `transform`) moved and grown by the
/// blur's reach. A run's shadows are drawn the last first, so that the first is on top.
fn draw_shadow<S: PaintScene>(
    scene: &mut S,
    shadow: &TextShadow,
    scale: f64,
    transform: Affine,
    ink: Rect,
    mut draw: impl FnMut(&mut S, Color, Affine),
) {
    if shadow.color.components[3] <= 0.0 {
        return;
    }
    let offset = Vec2::new(shadow.x * scale, shadow.y * scale);
    let moved = transform * Affine::translate(offset);
    let sigma = shadow.blur * scale / 2.0;
    if sigma > 0.0 {
        let reach = (3.0 * sigma).ceil() + 1.0;
        let mut clip = (ink + offset).inflate(reach, reach);
        // On whole device pixels where the text is only moved (the backend then needs no
        // layer of its own for it).
        let [a, b, c, d, e, f] = transform.as_coeffs();
        if (a, b, c, d) == (1.0, 0.0, 0.0, 1.0) {
            clip = (clip + Vec2::new(e, f)).expand() - Vec2::new(e, f);
        }
        let blur = Filter::single(FilterEffect::blur(sigma as f32));
        scene.push_layer(Fill::NonZero, Mix::Normal, 1.0, transform, &clip, Some(Arc::new(blur)), None);
        draw(scene, shadow.color, moved);
        scene.pop_layer();
    } else {
        draw(scene, shadow.color, moved);
    }
}

/// pyro: the colour a node's text is filled with.
pub(crate) fn text_fill_color(doc: &BaseDocument, node_id: NodeId) -> Color {
    resolve_decoration_entry(doc, node_id).fill_color
}

/// Resolve the cached style values for a single node into a [`DecorationStackEntry`].
fn resolve_decoration_entry(doc: &BaseDocument, node_id: NodeId) -> DecorationStackEntry {
    let Some(styles) = doc.get_node(node_id).and_then(|node| node.primary_styles()) else {
        return DecorationStackEntry {
            node_id,
            fill_color: Color::BLACK,
            decoration: None,
            shadows: Vec::new(),
        };
    };

    let itext = styles.get_inherited_text();
    let fill_color = itext
        ._webkit_text_fill_color
        .resolve_to_absolute(&itext.color)
        .as_srgb_color();

    let shadows = itext
        .text_shadow
        .0
        .iter()
        .map(|shadow| TextShadow {
            color: shadow.color.resolve_to_absolute(&itext.color).as_srgb_color(),
            x: f64::from(shadow.horizontal.px()),
            y: f64::from(shadow.vertical.px()),
            blur: f64::from(shadow.blur.px()),
        })
        .collect();

    DecorationStackEntry {
        node_id,
        fill_color,
        decoration: own_decoration(&styles),
        shadows,
    }
}

/// The run-dependent geometry used to position and size a decoration, captured from a
/// single glyph run. Firefox draws a decoration once for the whole decorating box using
/// the box's own font, so we prefer the values from a run whose innermost node *is* the
/// decorating box (its own text), falling back to the first run it covers.
#[derive(Clone)]
struct DecorationRunGeometry {
    /// The baseline the decoration is positioned from (the run's own, `vertical-align`
    /// shifted, baseline).
    baseline: f32,
    /// pyro: the ascent and descent of the content area of the run's style: its first available
    /// font's, rounded as the layout rounds them (to whole CSS px; Blink's `FontMetrics::Ascent`).
    content_ascent: f32,
    content_descent: f32,
    underline_offset: f32,
    underline_size: f32,
    strikethrough_size: f32,
    css_font_size: f64,
}

/// A decoration accumulated across the runs of a single line for one decorating box.
///
/// The `text-decoration-*` properties describe the whole box, so we gather the horizontal
/// extent it covers on the line (`min_x`/`max_x`) and the font geometry to draw with, then
/// paint a single line spanning the box rather than one segment per run.
struct LineDecoration {
    node_id: NodeId,
    deco: ResolvedDecoration,
    /// pyro: the box is an ancestor of the inline root (a block whose decoration propagates
    /// into it): it is positioned from the line, as the root's own is.
    outer: bool,
    /// pyro: the `text-shadow` of the first run the box covers on the line (Chromium draws a
    /// decoration with the shadows of the text it decorates).
    shadows: Vec<TextShadow>,
    min_x: f64,
    max_x: f64,
    /// Geometry from a run whose innermost node is the decorating box itself.
    own: Option<DecorationRunGeometry>,
    /// Geometry from the first run the box covers (fallback when it has no own text on
    /// this line, e.g. it only wraps differently-sized descendants).
    first: Option<DecorationRunGeometry>,
}

/// pyro: what a glyph run of the line being drawn is drawn with, found by the pass over the
/// line's runs that gathers its decorations (underlines go under the text, so they are
/// drawn between that pass and the one that draws the glyphs).
struct RunPaint {
    fill_color: Color,
    /// Its text shadows, in [`DrawTextContext::run_shadows`].
    shadows: std::ops::Range<usize>,
    /// The glyphs `text-overflow` keeps, and their extent.
    kept: std::ops::Range<usize>,
    kept_extent: Option<(f32, f32)>,
    /// pyro: whether this pass draws it ([`RunFilter`]).
    drawn: bool,
}

/// pyro: the glyph runs of an inline root the next `stroke_text` draws, in a pass of
/// `draw_inline_effects` inside an inline element's areas: those whose glyphs (and text shadows)
/// can reach one of `areas` (in the layout's space, the text's shifts applied). The others are
/// clipped away there: each element's pass drew every glyph of the layout.
#[derive(Clone, Debug)]
pub(crate) struct RunFilter {
    /// The inline root whose text the pass draws (other roots draw all their runs).
    pub root: NodeId,
    pub areas: Vec<Rect>,
}

/// pyro: the decorations propagated into a box's content from its ancestors, outermost first.
type Propagated = Rc<[(NodeId, ResolvedDecoration)]>;

/// Reusable scratch storage for drawing text across all inline formatting contexts in a
/// document. The vectors are cleared between uses without releasing their allocations.
#[derive(Default)]
pub(crate) struct DrawTextContext {
    stack: Vec<DecorationStackEntry>,
    path_scratch: Vec<NodeId>,
    deco_boxes: Vec<LineDecoration>,
    /// pyro: what each glyph run of the line being drawn is drawn with ([`RunPaint`]).
    run_paints: Vec<RunPaint>,
    run_shadows: Vec<TextShadow>,
    /// pyro: by node slot, the decorations propagated into the node's box from its ancestors
    /// (see [`propagated_decorations`]), as far as this paint has needed them.
    propagated: Vec<Option<(NodeId, Propagated)>>,
    /// The glyph runs drawn, when recording ([`crate::paint_scene_recording_text`]).
    pub(crate) runs: Option<Vec<crate::TextRun>>,
    /// How far the next `stroke_text` moves its text along x (device px): from its box's whole-pixel
    /// x to its layout-unit x (`fractional_text_x`).
    pub(crate) text_shift: f64,
    /// pyro: the atoms the next `stroke_text` leaves out (`text-overflow: ellipsis`).
    pub(crate) cut: Option<TextCut>,
    /// pyro: where the next `stroke_text` puts the top of each line (none: where the layout has it).
    pub(crate) line_tops: Option<LineTops>,
    /// pyro: the glyph runs the next `stroke_text` draws (none: all).
    pub(crate) run_filter: Option<RunFilter>,
}

/// pyro: where Chromium paints the lines of a block's text: each line's top at the whole CSS pixel
/// nearest its unrounded place in the document (Blink's `TextFragmentPainter` rounds a text
/// fragment's top "to ensure consistent line heights"; its place within the line is in whole pixels
/// already), at any device scale. The layout has its lines at whole device pixels of its own origin.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LineTops {
    /// The unrounded top of the layout in the document (CSS px).
    pub origin: f64,
    /// Where that top is painted: the origin of `stroke_text`'s transform (CSS px of the document).
    pub painted: f64,
    /// Device pixels in a CSS pixel.
    pub scale: f64,
}

impl LineTops {
    /// How far down from its place in the layout the line is painted (device px).
    pub(crate) fn shift(&self, metrics: &parley::LineMetrics) -> f64 {
        let top = crate::snapped_line_top(self.origin, metrics.unquantized_block_min_coord, self.scale);
        (top - self.painted) * self.scale - metrics.block_min_coord as f64
    }
}

/// pyro: where `text-overflow: ellipsis` cuts a line: the atoms (clusters, ligatures) of its
/// glyph runs that end past `edge` (layout x; `keep_left`) or start before it (right-to-left
/// lines) are not drawn.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TextCut {
    pub edge: f32,
    pub keep_left: bool,
}

impl TextCut {
    /// The glyphs of `run` (in [`parley::GlyphRun::glyphs`] order) this cut keeps, and the x
    /// extent of their atoms (`None` when it keeps none). A left-to-right cut keeps a prefix of
    /// the run's atoms in visual order, a right-to-left one a suffix.
    pub(crate) fn kept(&self, run: &parley::GlyphRun<'_, TextBrush>) -> (std::ops::Range<usize>, Option<(f32, f32)>) {
        // Atom advances within a run add up to fractions of a layout unit off the line's.
        const EPSILON: f32 = 1.0 / 128.0;
        let advances: Vec<f32> = run.glyphs().map(|g| g.advance).collect();
        let (mut pen, mut index) = (run.offset(), 0);
        let (mut range, mut extent) = (None::<std::ops::Range<usize>>, None::<(f32, f32)>);
        for (_, count) in run.atom_texts() {
            let Some(glyphs) = advances.get(index..index + count) else { break };
            let advance: f32 = glyphs.iter().sum();
            let keep = if self.keep_left {
                pen + advance <= self.edge + EPSILON
            } else {
                pen >= self.edge - EPSILON
            };
            if keep {
                let r = range.get_or_insert(index..index);
                r.end = index + count;
                let e = extent.get_or_insert((pen, pen + advance));
                *e = (e.0.min(pen), e.1.max(pen + advance));
            }
            pen += advance;
            index += count;
        }
        (range.unwrap_or(0..0), extent)
    }
}

/// pyro: the thickness of a decoration line in CSS px, as Blink resolves it
/// (`ComputeDecorationThickness`) before it draws the line in whole CSS px ([`whole_css_px`]):
/// `text-decoration-thickness` (percentages of the font size) floored to whole CSS px (WPT's
/// `text-decoration-thickness-length-rounding-*`), `from-font` (the font's own, `metric_size` in
/// layout units) rounded, `auto` a tenth of the font size; at least 1px.
fn decoration_thickness(
    thickness: &TextDecorationLength,
    metric_size: f32,
    css_font_size: f64,
    scale: f64,
) -> f64 {
    let auto = (css_font_size / 10.0).max(1.0);
    match thickness {
        GenericTextDecorationLength::LengthPercentage(lp) => {
            f64::from(lp.resolve(Length::new(css_font_size as f32)).px()).floor().max(1.0)
        }
        GenericTextDecorationLength::FromFont if metric_size > 0.0 => {
            (f64::from(metric_size) / scale).round().max(1.0)
        }
        GenericTextDecorationLength::FromFont | GenericTextDecorationLength::Auto => auto,
    }
}

/// pyro: a decoration line's thickness drawn: whole CSS pixels (at least one), as Blink's
/// `GetRectForTextLine` cuts it, at any device scale.
fn whole_css_px(thickness: f64) -> f64 {
    thickness.trunc().max(1.0)
}

/// Draws a single decoration line of the given `text-decoration-style` spanning
/// `[x0, x0 + width]`, `size` thick from `top` down (pyro: Blink's line rect). Geometry is
/// built explicitly (rather than relying on backend dash support) so it renders consistently
/// across renderers.
///
/// `double_dir` points away from the text (`+1` = down, `-1` = up) and is used to place the
/// second line of a `double` decoration.
#[allow(clippy::too_many_arguments)]
fn draw_decoration_line(
    scene: &mut impl PaintScene,
    transform: Affine,
    scale: f64,
    x0: f64,
    width: f64,
    top: f64,
    size: f64,
    brush: &anyrender::Paint,
    double_dir: f64,
    deco_style: TextDecorationStyle,
    inset_start: f64,
    inset_end: f64,
) {
    // Inset the line from the box's start/end edges (assumes LTR: start is the left edge).
    // Negative insets extend the line past the text.
    let x = x0 + inset_start;
    let w = width - inset_start - inset_end;
    if w <= 0.0 {
        return;
    }
    let y = top + size / 2.0;
    let butt_stroke = Stroke::new(size).with_caps(Cap::Butt);

    match deco_style {
        TextDecorationStyle::None => {
            // no line. Equivalent to `text-decoration-line: none`
        }
        // `solid`
        TextDecorationStyle::Solid => {
            let line = kurbo::Line::new((x, y), (x + w, y));
            scene.stroke(&butt_stroke, transform, brush, None, &line);
        }
        // Two lines, each `size` thick, separated by a 1px (CSS) gap. This
        // matches Chrome, where the second line is offset by `thickness + 1px`
        // and sits further from the text (below for underline, above for
        // overline).
        TextDecorationStyle::Double => {
            let one_css_px = scale;
            for cy in [y, y + double_dir * (size + one_css_px)] {
                let line = kurbo::Line::new((x, cy), (x + w, cy));
                scene.stroke(&butt_stroke, transform, brush, None, &line);
            }
        }
        // Round dots (diameter = line thickness) spaced one dot apart.
        TextDecorationStyle::Dotted => {
            let radius = size / 2.0;
            let step = 2.0 * size;
            let mut cx = x + radius;
            while cx <= x + w - radius {
                let dot = Circle::new((cx, y), radius);
                scene.fill(Fill::NonZero, transform, brush, None, &dot);
                cx += step;
            }
        }
        // Dashes as filled rectangles. Dash/gap lengths are relative to the
        // thickness (matching Blink): thinner lines use proportionally longer
        // dashes and gaps so they don't read as dotted or solid.
        TextDecorationStyle::Dashed => {
            let (dash_ratio, gap_ratio) = if size >= 3.0 { (2.0, 1.0) } else { (3.0, 2.0) };
            let dash = size * dash_ratio;
            let nominal_gap = size * gap_ratio;
            // Nudge the gap so a whole number of dashes fits the run evenly.
            let gap = if w > 2.0 * dash {
                select_best_dash_gap(w, dash, nominal_gap)
            } else {
                nominal_gap
            };
            let mut dx = x;
            while dx < x + w {
                let end = (dx + dash).min(x + w);
                let rect = Rect::new(dx, y - size / 2.0, end, y + size / 2.0);
                scene.fill(Fill::NonZero, transform, brush, None, &rect);
                dx += dash + gap;
            }
        }
        // A squiggle built from alternating quadratic Béziers.
        TextDecorationStyle::Wavy => {
            let amplitude = size;
            let half_wave = (2.0 * size).max(1.0);
            let mut path = BezPath::new();
            path.move_to((x, y));
            let mut px = x;
            let mut up = true;
            while px < x + w {
                let nx = (px + half_wave).min(x + w);
                // A quad Bézier reaches half the control-point offset at its
                // midpoint, so use `2 * amplitude` to hit the target peak.
                let ctrl_y = if up {
                    y - 2.0 * amplitude
                } else {
                    y + 2.0 * amplitude
                };
                path.quad_to(((px + nx) / 2.0, ctrl_y), (nx, y));
                px = nx;
                up = !up;
            }
            let stroke = Stroke::new(size).with_caps(Cap::Round);
            scene.stroke(&stroke, transform, brush, None, &path);
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct TextPaintOptions {
    pub scale: f64,
    pub brush_override: Option<Color>,
    /// pyro: draw `text-shadow` (not in a `background-clip: text` mask, nor in a recording
    /// paint: the PDF matches each glyph run drawn to the text run recorded for it).
    pub shadows: bool,
}

/// pyro: what [`flush_line_decorations`] draws of a line's decorations.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DecorationPhase {
    /// Shadow `n` (by index in each box's `text-shadow`) of every line, under all the line draws.
    Shadow(usize),
    /// Underlines and overlines, under the text.
    UnderOver,
    /// Line-throughs, over the text.
    LineThrough,
}

/// pyro: the inline root's own font on a line (its strut), which places the decorations of the
/// root and of the blocks it is in where the root has no text of its own on the line.
struct StrutGeometry {
    baseline: f32,
    ascent: f32,
    descent: f32,
    css_font_size: f64,
}

/// Paint the decorations accumulated for one line, one line per decorating box: its
/// line-throughs (`line_through`), which go over the text, or its underlines and overlines,
/// which go under it (CSS 2 Appendix E).
///
/// pyro: the lines are placed as Blink places them, in whole CSS pixels from the baseline (at
/// any device scale), from the decorating box's font: its rounded ascent and descent
/// ([`DecorationRunGeometry::content_ascent`]) and the line's thickness `t` (CSS px). An
/// underline is `max(1, ceil(t / 2))` below the baseline (`ComputeUnderlineOffsetAuto`), at the
/// baseline plus `text-underline-offset` when one is given, below the descent for
/// `text-underline-position: under`, at the font's position for `from-font`; an overline rests
/// on the ascent; a line-through is centred two thirds of the ascent up from the text's top,
/// its top rounded half down to a CSS pixel.
#[allow(clippy::too_many_arguments)]
fn flush_line_decorations<S: PaintScene>(
    scene: &mut S,
    transform: Affine,
    options: TextPaintOptions,
    deco_boxes: &[LineDecoration],
    inline_root_id: NodeId,
    strut: &StrutGeometry,
    phase: DecorationPhase,
    line_runs: &[parley::GlyphRun<'_, TextBrush>],
    source: Option<&str>,
    vertical: bool,
) {
    let TextPaintOptions {
        scale,
        brush_override,
        ..
    } = options;
    let wanted = match phase {
        DecorationPhase::Shadow(_) => {
            TextDecorationLine::UNDERLINE | TextDecorationLine::OVERLINE | TextDecorationLine::LINE_THROUGH
        }
        DecorationPhase::LineThrough => TextDecorationLine::LINE_THROUGH,
        DecorationPhase::UnderOver => TextDecorationLine::UNDERLINE | TextDecorationLine::OVERLINE,
    };
    // Draw innermost boxes first so ancestors' decorations paint on top, matching the
    // per-run drawing order this replaced (`stack.iter().rev()`).
    for acc in deco_boxes.iter().rev() {
        let deco = &acc.deco;
        if !deco.line.intersects(wanted) {
            continue;
        }
        // Prefer the decorating box's own font; fall back to the first run it covers.
        let geom = match (&acc.own, &acc.first) {
            (Some(own), _) => own.clone(),
            (None, Some(first)) => {
                let mut geom = first.clone();
                // A descendant run's baseline may be shifted by `vertical-align`, but the
                // decoration is positioned from the decorating box's own baseline. For the
                // inline root (and a block it is in) that is the line's baseline, and its font
                // (pyro) the root's: the line's strut.
                if acc.node_id == inline_root_id || acc.outer {
                    geom.baseline = strut.baseline;
                    geom.content_ascent = strut.ascent;
                    geom.content_descent = strut.descent;
                    geom.css_font_size = strut.css_font_size;
                }
                geom
            }
            (None, None) => continue,
        };
        let geom = &geom;
        let width = acc.max_x - acc.min_x;
        if width <= 0.0 {
            continue;
        }
        let brush = anyrender::Paint::from(brush_override.unwrap_or(deco.color));

        // `text-decoration-inset` shortens (or, when negative, extends) the line from the
        // inline-start/end edges. Percentages resolve against the decoration line length
        // (the box's total advance on this line); `auto` is no inset.
        let (inset_start, inset_end) = match &deco.inset {
            GenericTextDecorationInset::LengthPercentage { start, end } => {
                // The extent is in device pixels; convert to CSS pixels so the resolved
                // result can be scaled back.
                let line_length = Length::new((width / scale) as f32);
                (
                    start.resolve(line_length).px() as f64 * scale,
                    end.resolve(line_length).px() as f64 * scale,
                )
            }
            GenericTextDecorationInset::Auto => (0.0, 0.0),
        };
        // The shadow this phase draws, if the box has it.
        let shadow = match phase {
            DecorationPhase::Shadow(index) => match acc.shadows.get(index) {
                Some(shadow) => Some(shadow),
                None => continue,
            },
            _ => None,
        };
        let baseline = f64::from(geom.baseline);
        // pyro: a decoration line (or its shadow), `top` CSS px below the baseline and
        // `thickness` CSS px thick (both whole); a wavy or double line reaches a few sizes further.
        // An underline or overline leaves gaps where glyphs cross it (`skip_ink`).
        // `skip` is the line's thickness (CSS px, before it is cut) where it skips ink.
        let draw_line = |scene: &mut S, top: f64, thickness: f64, double_dir: f64, skip: Option<f64>| {
            let (top, size) = (baseline + top * scale, thickness * scale);
            let gaps = match skip {
                Some(t) if deco.skip_ink != TextDecorationSkipInk::None && !vertical => ink_gaps(
                    line_runs,
                    source,
                    (acc.min_x, acc.max_x),
                    (top + 0.5 * scale, top + size - 0.5 * scale),
                    t.min(13.0) * scale,
                    deco.skip_ink == TextDecorationSkipInk::All,
                ),
                _ => SmallVec::new(),
            };
            let line = |scene: &mut S, brush: &anyrender::Paint, transform: Affine| {
                let draw = |scene: &mut S, x0: f64, width: f64, inset_start: f64, inset_end: f64| {
                    draw_decoration_line(
                        scene,
                        transform,
                        scale,
                        x0,
                        width,
                        top,
                        size,
                        brush,
                        double_dir,
                        deco.style,
                        inset_start,
                        inset_end,
                    )
                };
                if gaps.is_empty() {
                    return draw(scene, acc.min_x, width, inset_start, inset_end);
                }
                // The line between the gaps: a solid line is drawn piece by piece, any other
                // inside a clip of its pieces (its pattern runs on under the gaps). A gap's edges
                // are on whole device pixels (Blink clips them out aliased), so that a line drawn
                // over another with the same gaps covers it.
                let (start, end) = (acc.min_x + inset_start, acc.max_x - inset_end);
                let [a, b, c, d, tx, _] = transform.as_coeffs();
                let snap = |x: f64| if (a, b, c, d) == (1.0, 0.0, 0.0, 1.0) { (tx + x).round() - tx } else { x };
                let mut pieces: SmallVec<[(f64, f64); 8]> = SmallVec::new();
                let mut x = start;
                for &(gap_start, gap_end) in &gaps {
                    let (gap_start, gap_end) = (snap(gap_start), snap(gap_end));
                    if gap_start > x {
                        pieces.push((x, gap_start.min(end)));
                    }
                    x = x.max(gap_end);
                }
                if x < end {
                    pieces.push((x, end));
                }
                pieces.retain(|(a, b)| b > a);
                if deco.style == TextDecorationStyle::Solid {
                    // One shape of the pieces' rects.
                    let mut path = BezPath::new();
                    for (a, b) in pieces {
                        path.extend(&kurbo::Shape::to_path(&Rect::new(a, top, b, top + size), 0.1));
                    }
                    scene.fill(Fill::NonZero, transform, brush, None, &path);
                } else if !pieces.is_empty() {
                    let reach = size * 4.0 + scale;
                    let mut clip = BezPath::new();
                    for (a, b) in pieces {
                        clip.extend(&kurbo::Shape::to_path(&Rect::new(a, top - reach, b, top + size + reach), 0.1));
                    }
                    scene.push_layer(Fill::NonZero, Mix::Normal, 1.0, transform, &clip, None, None);
                    draw(scene, acc.min_x, width, inset_start, inset_end);
                    scene.pop_layer();
                }
            };
            match shadow {
                Some(shadow) => {
                    let y = top + size / 2.0;
                    let reach = size * 4.0 + scale;
                    let ink = Rect::new(acc.min_x - reach, y - reach, acc.max_x + reach, y + reach);
                    draw_shadow(scene, shadow, scale, transform, ink, |scene, color, transform| {
                        line(scene, &anyrender::Paint::from(color), transform)
                    });
                }
                None => line(scene, &brush, transform),
            }
        };
        // The font's ascent and descent in whole CSS px.
        let ascent = (f64::from(geom.content_ascent) / scale).round();
        let descent = (f64::from(geom.content_descent) / scale).round();

        if wanted.contains(TextDecorationLine::UNDERLINE) && deco.line.contains(TextDecorationLine::UNDERLINE) {
            let t = decoration_thickness(&deco.thickness, geom.underline_size, geom.css_font_size, scale);
            // `text-underline-offset` (percentages of 1em), in whole CSS px.
            let offset = deco
                .underline_offset
                .as_ref()
                .map(|lp| f64::from(lp.resolve(Length::new(geom.css_font_size as f32)).px()));
            let top = if deco.underline_under {
                // Below the descent, clear of descending glyphs like "gqy".
                descent + offset.unwrap_or(0.0).round()
            } else if deco.underline_from_font {
                (-f64::from(geom.underline_offset) / scale + offset.unwrap_or(0.0)).round()
            } else if let Some(offset) = offset {
                // An explicit offset under `auto` is taken from the alphabetic baseline.
                offset.round()
            } else {
                (t / 2.0).ceil().max(1.0)
            };
            // A `double` underline extends downward, away from the text.
            draw_line(scene, top, whole_css_px(t), 1.0, Some(t));
        }
        if wanted.contains(TextDecorationLine::OVERLINE) && deco.line.contains(TextDecorationLine::OVERLINE) {
            // Fonts don't provide a dedicated overline metric, so reuse the underline
            // thickness. The line rests on the ascent and extends upward from there.
            let t = decoration_thickness(&deco.thickness, geom.underline_size, geom.css_font_size, scale);
            let thickness = whole_css_px(t);
            // A `double` overline extends upward, away from the text.
            draw_line(scene, -ascent - thickness, thickness, -1.0, Some(t));
        }
        if wanted.contains(TextDecorationLine::LINE_THROUGH) && deco.line.contains(TextDecorationLine::LINE_THROUGH) {
            let t = decoration_thickness(&deco.thickness, geom.strikethrough_size, geom.css_font_size, scale);
            // Centred two thirds of the ascent down from the text's top: Blink's
            // `TextDecorationInfo` line offset, its top rounded half down to a CSS pixel.
            let top = (-ascent / 3.0 - t / 2.0 - 0.5).ceil();
            draw_line(scene, top, whole_css_px(t), 1.0, None);
        }
    }
}

/// pyro: the decoration a box draws as a decorating box (`text-decoration-line`), if any.
fn own_decoration(styles: &style::properties::ComputedValues) -> Option<ResolvedDecoration> {
    let itext = styles.get_inherited_text();
    let text = styles.get_text();
    let drawn_lines = TextDecorationLine::UNDERLINE
        | TextDecorationLine::OVERLINE
        | TextDecorationLine::LINE_THROUGH;
    let line = text.text_decoration_line;
    // Decorations propagate through the box tree, and a `display: contents` element
    // generates no box, so its decorations have no effect on descendants.
    if !line.intersects(drawn_lines) || styles.clone_display().is_contents() {
        return None;
    }
    // `text-decoration-color: currentColor` (the initial value) resolves against
    // the decorating box's own colour, not the descendant run's.
    let color = text
        .text_decoration_color
        .as_absolute()
        .map(ToColorColor::as_color_color)
        .unwrap_or(itext.color.as_color_color());
    Some(ResolvedDecoration {
        line,
        style: text.text_decoration_style,
        color,
        thickness: text.text_decoration_thickness.clone(),
        underline_offset: itext.text_underline_offset.non_auto(),
        underline_under: itext
            .text_underline_position
            .contains(TextUnderlinePosition::UNDER),
        underline_from_font: itext
            .text_underline_position
            .contains(TextUnderlinePosition::FROM_FONT),
        inset: text.text_decoration_inset.clone(),
        skip_ink: itext.text_decoration_skip_ink,
    })
}

/// pyro: how far (CSS px) a box's own text decorations can reach outside its line boxes: their
/// thickness and offset, and an em for where the font puts them.
pub(crate) fn decoration_ink_reach(styles: &style::properties::ComputedValues) -> f64 {
    let text = styles.get_text();
    if text.text_decoration_line.is_empty() {
        return 0.0;
    }
    let font_size = styles.get_font().font_size.used_size.0.px();
    let em = Length::new(font_size);
    let thickness = match &text.text_decoration_thickness {
        GenericTextDecorationLength::LengthPercentage(lp) => lp.resolve(em).px().abs(),
        _ => font_size / 10.0,
    };
    let offset = styles
        .get_inherited_text()
        .text_underline_offset
        .non_auto()
        .map_or(0.0, |lp| lp.resolve(em).px().abs());
    f64::from(font_size + thickness + offset)
}

/// pyro: whether the decorations of a box's ancestors stop at it (CSS Text Decoration 3, 2.1):
/// they are not propagated to out-of-flow boxes (floats, absolutely positioned boxes) nor
/// into the contents of atomic inlines (inline blocks, inline tables, inline flex and grid
/// containers).
fn stops_propagation(styles: &style::properties::ComputedValues) -> bool {
    use style::values::specified::box_::{DisplayInside, DisplayOutside};
    let display = styles.clone_display();
    // A `display: contents` element has no box (its `float` does nothing).
    if display.is_contents() {
        return false;
    }
    let box_style = styles.get_box();
    if box_style.position.is_absolutely_positioned() || box_style.float.is_floating() {
        return true;
    }
    display.outside() == DisplayOutside::Inline && display.inside() != DisplayInside::Flow
}

/// pyro: the decorations propagated into the content of `node_id`'s box from its ancestors,
/// outermost first: those of every decorating box above it in flow, up to the first box that
/// stops them ([`stops_propagation`]). Known results are kept in `cache` (by node index) for
/// the rest of the paint; `scratch` holds the chain walked.
fn propagated_decorations(
    doc: &BaseDocument,
    node_id: NodeId,
    cache: &mut Vec<Option<(NodeId, Propagated)>>,
    scratch: &mut Vec<NodeId>,
) -> Propagated {
    // A node id's low half is its slot in the tree (the high half a version).
    let slot = |id: NodeId| id.as_u64() as u32 as usize;
    let remember = |cache: &mut Vec<Option<(NodeId, Propagated)>>, id: NodeId, value: &Propagated| {
        if cache.len() <= slot(id) {
            cache.resize(slot(id) + 1, None);
        }
        cache[slot(id)] = Some((id, value.clone()));
    };
    scratch.clear();
    let mut id = node_id;
    // What propagates into `id`, the top of the walk.
    let mut result: Propagated = loop {
        if let Some(Some((known_id, known))) = cache.get(slot(id))
            && *known_id == id
        {
            break known.clone();
        }
        let Some(node) = doc.get_node(id) else {
            break Rc::from([]);
        };
        let stops = node.primary_styles().is_some_and(|styles| stops_propagation(&styles));
        // The DOM parent: a table part's box can be left out of the layout tree (a row group).
        let parent = node.parent.or_else(|| node.layout_parent.get());
        match parent {
            Some(parent) if !stops => {
                scratch.push(id);
                id = parent;
            }
            _ => {
                let empty: Propagated = Rc::from([]);
                remember(cache, id, &empty);
                break empty;
            }
        }
    };
    // Back down: each box gets what its parent gets, and the parent's own decoration.
    let mut parent = id;
    for &child in scratch.iter().rev() {
        let own = doc
            .get_node(parent)
            .and_then(|node| node.primary_styles())
            .and_then(|styles| own_decoration(&styles));
        if let Some(own) = own {
            result = result.iter().cloned().chain(std::iter::once((parent, own))).collect();
        }
        remember(cache, child, &result);
        parent = child;
    }
    result
}

/// `transform` with its content moved `shift` along the x axis of the text.
fn shifted(transform: Affine, shift: f64) -> Affine {
    if shift == 0.0 {
        transform
    } else {
        transform * Affine::translate((shift, 0.0))
    }
}

pub(crate) fn stroke_text<'a, S: PaintScene>(
    scene: &mut S,
    lines: impl Iterator<Item = Line<'a, TextBrush>>,
    doc: &BaseDocument,
    transform: Affine,
    options: TextPaintOptions,
    inline_root_id: NodeId,
    source: Option<&str>,
    context: &mut DrawTextContext,
    vertical: Option<VerticalLine>,
) {
    let TextPaintOptions {
        scale,
        brush_override,
        shadows: paint_shadows,
    } = options;
    // pyro: text shadows are drawn in screenshots only (see `TextPaintOptions::shadows`), and not
    // in vertical writing modes, whose lines are turned (a shadow's offset is physical).
    let paint_shadows = paint_shadows && context.runs.is_none() && vertical.is_none();
    let options = TextPaintOptions { shadows: paint_shadows, ..options };
    let DrawTextContext {
        stack,
        path_scratch,
        deco_boxes,
        run_paints,
        run_shadows,
        propagated,
        runs,
        text_shift,
        cut,
        line_tops,
        run_filter,
    } = context;
    let shift = *text_shift;
    let cut = *cut;
    let line_tops = line_tops.filter(|_| vertical.is_none());
    let run_filter = run_filter.as_ref().filter(|filter| filter.root == inline_root_id);
    // pyro: the decorations of the root's ancestors that propagate into it.
    let outer = propagated_decorations(doc, inline_root_id, propagated, path_scratch);
    let root_font_size = doc
        .get_node(inline_root_id)
        .and_then(|node| node.primary_styles())
        .map_or(16.0, |styles| f64::from(styles.get_font().font_size.used_size.0.px()));
    stack.clear();
    path_scratch.clear();
    deco_boxes.clear();

    // Persistent stack mirroring the ancestor path (inline root -> current run's
    // node) as we walk the runs. The `text-decoration-*` properties are *not*
    // inherited; instead a decoration set on an ancestor is propagated to the
    // descendant text it wraps. Rather than re-resolving styles for the whole
    // ancestor chain on every run, we cache each node's resolved values here and,
    // for each run, only resolve styles for the nodes newly descended into (popping
    // as we ascend). `path_scratch` is a reusable buffer for the run's node path.
    for line in lines {
        // pyro: the line at Chromium's whole CSS pixel, with its decorations.
        let transform = match line_tops {
            Some(tops) => transform * Affine::translate((0.0, tops.shift(line.metrics()))),
            None => transform,
        };
        // Decorations accumulated for this line, keyed by decorating box, so each box is
        // painted once (spanning all its runs) using its own font — matching Firefox, which
        // draws one decoration per box rather than one stepped segment per differently-sized
        // run. Clearing preserves the allocation for the next line and inline context.
        deco_boxes.clear();
        run_paints.clear();
        run_shadows.clear();
        // pyro: the line's glyph runs, walked by each pass over the line.
        let glyph_runs: SmallVec<[parley::GlyphRun<'_, TextBrush>; 16]> = line
            .items()
            .filter_map(|item| match item {
                PositionedLayoutItem::GlyphRun(run) => Some(run),
                _ => None,
            })
            .collect();

        // pyro: the line's decorations, and what each of its glyph runs is drawn with.
        for glyph_run in &glyph_runs {
            let run = glyph_run.run();
            let font_size = run.font_size();
            let metrics = run.font_metrics();
            let style = glyph_run.style();
            let css_font_size = font_size as f64 / scale;

            // Reconcile the stack with this run's ancestor path. Build the path
            // (inline root -> run node), keep the shared prefix already on the stack,
            // pop the rest, and resolve styles only for the newly-descended nodes.
            path_scratch.clear();
            let mut walk_id = Some(style.brush.id);
            while let Some(node_id) = walk_id {
                path_scratch.push(node_id);
                if node_id == inline_root_id {
                    break;
                }
                walk_id = doc.get_node(node_id).and_then(|node| node.parent);
            }
            path_scratch.reverse();

            let shared = stack
                .iter()
                .zip(path_scratch.iter())
                .take_while(|(entry, node_id)| entry.node_id == **node_id)
                .count();
            stack.truncate(shared);
            for &node_id in &path_scratch[shared..] {
                stack.push(resolve_decoration_entry(doc, node_id));
            }

            let fill_color = brush_override
                .unwrap_or_else(|| stack.last().map(|e| e.fill_color).unwrap_or(Color::BLACK));
            let shadows: &[TextShadow] = match stack.last() {
                Some(entry) if paint_shadows => &entry.shadows,
                _ => &[],
            };
            // pyro: the glyphs `text-overflow` keeps of the run (all without a cut).
            let (kept, kept_extent) = match cut {
                Some(cut) => cut.kept(&glyph_run),
                None => (0..usize::MAX, Some((glyph_run.offset(), glyph_run.offset() + glyph_run.advance()))),
            };
            // pyro: whether this pass draws the run, and which of its glyphs (see `RunFilter`): its
            // ink as its text shadows estimate it (an em around its advance and its content area,
            // and how far its shadows move and blur it), against the pass's areas. A screenshot
            // draws the glyphs within that reach of an area only (a run is often a line's text up
            // to the element); a recording paint keeps whole runs (the PDF maps them to text).
            let (kept, drawn) = match run_filter {
                None => (kept, true),
                Some(filter) => {
                    let em = f64::from(font_size);
                    let baseline =
                        f64::from(glyph_run.baseline()) + line_tops.map_or(0.0, |tops| tops.shift(line.metrics()));
                    let reach = shadows
                        .iter()
                        .map(|shadow| (shadow.x.abs().max(shadow.y.abs()) + 1.5 * shadow.blur) * scale + 1.0)
                        .fold(0.0, f64::max);
                    let x0 = f64::from(glyph_run.offset()) + shift;
                    let ink = Rect::new(
                        x0 - em,
                        baseline - f64::from(metrics.ascent) - em,
                        x0 + f64::from(glyph_run.advance()) + em,
                        baseline + f64::from(metrics.descent) + em,
                    )
                    .inflate(reach, reach);
                    let near: SmallVec<[Rect; 4]> = filter.areas.iter().copied().filter(|area| ink.overlaps(*area)).collect();
                    if near.is_empty() || runs.is_some() {
                        (kept, !near.is_empty())
                    } else {
                        let margin = em + reach;
                        let (mut first, mut last) = (usize::MAX, 0);
                        for (index, glyph) in glyph_run.positioned_glyphs().enumerate() {
                            let x = f64::from(glyph.x) + shift;
                            if near.iter().any(|area| x + margin > area.x0 && x - margin < area.x1) {
                                first = first.min(index);
                                last = index + 1;
                            }
                        }
                        let kept = kept.start.max(first)..kept.end.min(last);
                        let drawn = !kept.is_empty();
                        (kept, drawn)
                    }
                }
            };
            let first_shadow = run_shadows.len();
            run_shadows.extend_from_slice(shadows);
            run_paints.push(RunPaint {
                fill_color,
                shadows: first_shadow..run_shadows.len(),
                kept,
                kept_extent,
                drawn,
            });

            // Accumulate this run's contribution to each decorating box on its ancestor
            // path. The decoration is drawn once per box after the whole line has been
            // walked (see `flush_line_decorations`), so mixed font sizes within a box
            // produce a single straight line rather than one stepped segment per run.
            let (content_top, content_bottom) = line.span_content_area(glyph_run.style_index());
            let geometry = DecorationRunGeometry {
                baseline: glyph_run.baseline(),
                content_ascent: glyph_run.baseline() - content_top,
                content_descent: content_bottom - glyph_run.baseline(),
                underline_offset: metrics.underline_offset,
                underline_size: metrics.underline_size,
                strikethrough_size: metrics.strikethrough_size,
                css_font_size,
            };
            let run_node_id = style.brush.id;
            // pyro: decorations span what the cut keeps of the run.
            let Some((run_x0, run_x1)) = kept_extent else { continue };
            let (run_x0, run_x1) = (run_x0 as f64, run_x1 as f64);

            // pyro: the decorations of blocks the root is in come first (outermost).
            let boxes = outer
                .iter()
                .map(|(node_id, deco)| (*node_id, deco, true))
                .chain(stack.iter().filter_map(|entry| Some((entry.node_id, entry.decoration.as_ref()?, false))));
            for (node_id, deco, is_outer) in boxes {
                let idx = match deco_boxes.iter().position(|d| d.node_id == node_id) {
                    Some(idx) => idx,
                    None => {
                        deco_boxes.push(LineDecoration {
                            node_id,
                            deco: deco.clone(),
                            outer: is_outer,
                            shadows: shadows.to_vec(),
                            min_x: f64::INFINITY,
                            max_x: f64::NEG_INFINITY,
                            own: None,
                            first: None,
                        });
                        deco_boxes.len() - 1
                    }
                };
                let acc = &mut deco_boxes[idx];
                acc.min_x = acc.min_x.min(run_x0);
                acc.max_x = acc.max_x.max(run_x1);
                if acc.first.is_none() {
                    acc.first = Some(geometry.clone());
                }
                // A run whose innermost node is the box itself is the box's own text, so
                // its font is the one Firefox positions and sizes the decoration with. A block
                // the root is in has the root's own text.
                let own_node = if is_outer { inline_root_id } else { node_id };
                if own_node == run_node_id {
                    acc.own = Some(geometry.clone());
                }
            }
        }
        // pyro: a pass of `draw_inline_effects` that draws none of the line's runs leaves out the
        // line, unless it has decorations (which may cross the pass's areas between runs).
        if run_filter.is_some() && deco_boxes.is_empty() && !run_paints.iter().any(|paint| paint.drawn) {
            continue;
        }

        // pyro: the root's own font on the line, for the decorations of the root (and of the
        // blocks it is in) where it has no text of its own.
        let (strut_top, strut_bottom) = line.span_content_area(0);
        let line_baseline = line.metrics().baseline;
        let strut = StrutGeometry {
            baseline: line_baseline,
            ascent: line_baseline - strut_top,
            descent: strut_bottom - line_baseline,
            css_font_size: root_font_size,
        };
        let deco_transform = shifted(transform, shift);
        let flush = |scene: &mut S, phase| {
            flush_line_decorations(
                scene,
                deco_transform,
                options,
                deco_boxes,
                inline_root_id,
                &strut,
                phase,
                &glyph_runs,
                source,
                vertical.is_some(),
            )
        };

        // pyro: what a glyph run draws: its glyphs in a colour (its own, or a shadow's) moved by
        // a transform.
        let draw_run = |scene: &mut S, glyph_run: &parley::GlyphRun<'_, TextBrush>, paint: &RunPaint, shadow: Option<&TextShadow>, runs: &mut Option<Vec<crate::TextRun>>| {
            let run = glyph_run.run();
            let font = run.font();
            let font_size = run.font_size();
            let metrics = run.font_metrics();
            let synthesis = run.synthesis();
            let glyph_xform = synthesis
                .skew()
                .map(|angle| Affine::skew(angle.to_radians().tan() as f64, 0.0));
            let (transform, glyph_xform) = match vertical {
                Some(vertical) => {
                    let (shift, upright) = vertical.run_placement(glyph_run, source);
                    let glyph_xform = match (upright, glyph_xform) {
                        (Some(upright), Some(skew)) => Some(upright * skew),
                        (Some(upright), None) => Some(upright),
                        (None, skew) => skew,
                    };
                    (transform * shift, glyph_xform)
                }
                None => (transform, glyph_xform),
            };
            let fill_color = paint.fill_color;

            let embolden = if FONT_EMBOLDEN_ENABLED {
                font_embolden(font_size as f64, fill_color, transform)
            } else {
                kurbo::Vec2::default()
            } + if synthesis.embolden() {
                synthetic_bold(font_size as f64, scale, fill_color, transform)
            } else {
                kurbo::Vec2::default()
            };

            let normalized_coords: &[i16] = bytemuck::cast_slice(run.normalized_coords());
            let unshifted = transform;
            let transform = shifted(transform, shift);
            // pyro: on macOS glyph origins snap to quarters of a device pixel along the
            // baseline, as Skia's subpixel positioning does, where the run is drawn untransformed
            // (only translated) into a raster: a recording paint (the PDF's) keeps the exact x.
            let quarter_x = CORE_TEXT_GLYPHS && runs.is_none() && vertical.is_none() && {
                let [a, b, c, d, _, _] = transform.as_coeffs();
                a == 1.0 && b == 0.0 && c == 0.0 && d == 1.0
            };
            let tx = transform.as_coeffs()[4];
            let (kept, kept_extent) = (paint.kept.clone(), paint.kept_extent);
            // Zero-size text is invisible, but hinting can give its glyphs ink. Fully
            // transparent text is skipped too: backends draw color (emoji) glyphs opaque.
            // pyro: a recording paint (the PDF's) keeps it, as invisible text.
            let glyphs = || {
                glyph_run
                    .positioned_glyphs()
                    .skip(kept.start)
                    .take(kept.len())
                    .map(|glyph| anyrender::Glyph {
                        id: glyph.id as _,
                        x: if quarter_x {
                            (((tx + glyph.x as f64) * 4.0).round() / 4.0 - tx) as f32
                        } else {
                            glyph.x
                        },
                        y: glyph.y,
                    })
            };
            let draw = |scene: &mut S, color: Color, transform: Affine| {
                scene.draw_glyphs(
                    font,
                    font_size,
                    !FONT_EMBOLDEN_ENABLED && !CORE_TEXT_GLYPHS, // hint
                    normalized_coords,
                    embolden,
                    Fill::NonZero,
                    &anyrender::Paint::from(color),
                    1.0, // alpha
                    transform,
                    glyph_xform,
                    glyphs(),
                )
            };
            if font_size <= 0.0 || kept.is_empty() {
                return;
            }
            match shadow {
                // pyro: one of the run's text shadows.
                Some(shadow) => {
                    let run_extent = (glyph_run.offset(), glyph_run.offset() + glyph_run.advance());
                    let (x0, x1) = kept_extent.unwrap_or(run_extent);
                    let (baseline, em) = (f64::from(glyph_run.baseline()), f64::from(font_size));
                    let ink = Rect::new(
                        f64::from(x0) - em,
                        baseline - f64::from(metrics.ascent) - em * 0.5,
                        f64::from(x1) + em,
                        baseline + f64::from(metrics.descent) + em * 0.5,
                    );
                    draw_shadow(scene, shadow, scale, transform, ink, draw);
                }
                None if fill_color.components[3] > 0.0 || runs.is_some() => {
                    // Only drawn runs are recorded: the PDF matches each drawn glyph run to the
                    // next recorded one, so a run recorded but not drawn would stop the matching.
                    if let Some(runs) = runs.as_mut() {
                        runs.push(crate::TextRun::new(glyph_run, source, unshifted, kept.clone(), kept_extent));
                    }
                    draw(scene, fill_color, transform);
                }
                None => {}
            }
        };

        // pyro: where the line's decorations cast text shadows, the shadows go under all the line
        // draws, decorations' and glyphs' alike, a layer per shadow, the last lowest (as if each
        // shadow were the line drawn again in its colour). Otherwise each run's shadows go right
        // before its glyphs (Chromium paints a text fragment's shadows right before it). Then the
        // underlines and overlines, the glyphs, and the line-throughs (CSS 2 Appendix E).
        let layered = deco_boxes.iter().any(|deco| !deco.shadows.is_empty());
        if layered {
            let layers = run_paints
                .iter()
                .map(|paint| paint.shadows.len())
                .chain(deco_boxes.iter().map(|deco| deco.shadows.len()))
                .max()
                .unwrap_or(0);
            for layer in (0..layers).rev() {
                flush(scene, DecorationPhase::Shadow(layer));
                for (glyph_run, paint) in glyph_runs.iter().zip(run_paints.iter()).filter(|(_, paint)| paint.drawn) {
                    if let Some(shadow) = run_shadows[paint.shadows.clone()].get(layer) {
                        draw_run(scene, glyph_run, paint, Some(shadow), runs);
                    }
                }
            }
        }
        if !deco_boxes.is_empty() {
            flush(scene, DecorationPhase::UnderOver);
        }
        for (glyph_run, paint) in glyph_runs.iter().zip(run_paints.iter()).filter(|(_, paint)| paint.drawn) {
            if !layered {
                for shadow in run_shadows[paint.shadows.clone()].iter().rev() {
                    draw_run(scene, glyph_run, paint, Some(shadow), runs);
                }
            }
            draw_run(scene, glyph_run, paint, None, runs);
        }
        if !deco_boxes.is_empty() {
            flush(scene, DecorationPhase::LineThrough);
        }
    }
}

/// pyro: draws the annotations of one line of an inline root's text (blitz-dom's
/// `TextAnnotation`): over (or under) each base on the line, the annotation's own line with its
/// content area resting on the base's content area (the span content area of the base's style:
/// its first available font's ascent and descent), centred over the base. A ruby annotation's base
/// is a range of the root's text; emphasis marks are drawn over each character of the element's
/// own glyph runs that takes one, so a mark sits exactly where an `<rt>` of the same mark would.
/// `transform` is the line's (its whole-pixel top and the text's shift applied). `brush_override`
/// colours every annotation (a `background-clip: text` mask).
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_text_annotations<S: PaintScene>(
    scene: &mut S,
    line: &Line<'_, TextBrush>,
    annotations: &[TextAnnotation],
    text: &str,
    doc: &BaseDocument,
    transform: Affine,
    scale: f64,
    recording: bool,
    vertical: Option<VerticalLine>,
    brush_override: Option<Color>,
) {
    // Glyphs snap to quarter pixels as the text's do (`stroke_text`).
    let snap = !recording && vertical.is_none();
    let mut advances = Vec::new();
    let mut ruby_bases: Vec<Option<RubyBase>> = vec![None; annotations.len()];
    // Emphasis marks to draw: the annotation, the character's extent and content area.
    let mut marks: Vec<(usize, (f64, f64), (f64, f64))> = Vec::new();
    for item in line.items() {
        let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
            continue;
        };
        let shift = vertical.map_or(0.0, |vertical| vertical.fragment_shift(&glyph_run).translation().y);
        let (top, bottom) = line.span_content_area(glyph_run.style_index());
        let content = (f64::from(top) + shift, f64::from(bottom) + shift);
        let node_id = glyph_run.style().brush.id;
        advances.clear();
        advances.extend(glyph_run.glyphs().map(|glyph| glyph.advance));
        let advances: &[f32] = &advances;
        // Each atom (a cluster, or a ligature) with its extent.
        let atoms = || {
            let (mut pen, mut index) = (glyph_run.offset(), 0);
            glyph_run.atom_texts().map(move |(range, count)| {
                let advance: f32 = advances.get(index..index + count).map_or(0.0, |a| a.iter().sum());
                let extent = (f64::from(pen), f64::from(pen + advance));
                pen += advance;
                index += count;
                (range, count, extent)
            })
        };
        for (index, (annotation, ruby_base)) in annotations.iter().zip(ruby_bases.iter_mut()).enumerate() {
            match &annotation.kind {
                TextAnnotationKind::Emphasis if annotation.node_id == node_id => {
                    for (range, count, extent) in atoms() {
                        let marked = count > 0
                            && text.get(range).and_then(|t| t.chars().next()).is_some_and(can_take_emphasis_mark);
                        if marked {
                            marks.push((index, extent, content));
                        }
                    }
                }
                TextAnnotationKind::Ruby { base } => {
                    for (range, _, extent) in atoms() {
                        // White space around a base (collapsed into it from between bases) is not
                        // part of it.
                        let blank = text.get(range.clone()).is_none_or(|t| t.chars().all(char::is_whitespace));
                        if !base.contains(&range.start) || blank {
                            continue;
                        }
                        let found = ruby_base.get_or_insert(RubyBase { first: range.start, extent, content });
                        found.extent = (found.extent.0.min(extent.0), found.extent.1.max(extent.1));
                        // The base's content area is that of its first character's style.
                        if range.start < found.first {
                            *found = RubyBase { first: range.start, extent: found.extent, content };
                        }
                    }
                }
                _ => {}
            }
        }
    }
    // What is drawn over (or under) the line so far: an annotation of the same text goes outside
    // it (nested ruby; emphasis marks outside ruby annotations).
    let mut drawn: Vec<Drawn> = Vec::new();
    // A ruby annotation goes over the part of its base on the line its base starts on.
    for (annotation, ruby_base) in annotations.iter().zip(ruby_bases) {
        let (TextAnnotationKind::Ruby { base }, Some(ruby_base)) = (&annotation.kind, ruby_base) else {
            continue;
        };
        if base.start < line.text_range().start {
            continue;
        }
        let content = outside(&drawn, annotation.under, ruby_base.extent, ruby_base.content);
        let edge = draw_annotation(scene, annotation, ruby_base.extent, content, brush_override, doc, transform, scale, snap);
        drawn.push(Drawn { under: annotation.under, extent: ruby_base.extent, edge });
    }
    let ruby_drawn = drawn.len();
    for (index, extent, content) in marks {
        let annotation = &annotations[index];
        let color = brush_override.unwrap_or_else(|| emphasis_color(doc, annotation.node_id));
        let content = outside(&drawn[..ruby_drawn], annotation.under, extent, content);
        draw_annotation(scene, annotation, extent, content, Some(color), doc, transform, scale, snap);
    }
}

/// pyro: an annotation drawn over (or under) a base: its side, the base's extent (layout x) and
/// the annotation's outer edge (block axis).
struct Drawn {
    under: bool,
    extent: (f64, f64),
    edge: f64,
}

/// pyro: the content area an annotation of a base spanning `extent` rests on: the base's own
/// (`content`), moved outside the annotations already drawn on its side over overlapping bases.
fn outside(drawn: &[Drawn], under: bool, extent: (f64, f64), content: (f64, f64)) -> (f64, f64) {
    const EPSILON: f64 = 1.0 / 64.0;
    drawn
        .iter()
        .filter(|d| d.under == under && d.extent.0 < extent.1 - EPSILON && extent.0 < d.extent.1 - EPSILON)
        .fold(content, |(top, bottom), d| if under { (top, bottom.max(d.edge)) } else { (top.min(d.edge), bottom) })
}

/// pyro: the part of a ruby base on a line: where its first character there is in the text, its
/// extent (layout x) and the content area (block axis) of its first character's style.
#[derive(Clone, Copy)]
struct RubyBase {
    first: usize,
    extent: (f64, f64),
    content: (f64, f64),
}

/// pyro: the colour of an element's emphasis marks (`text-emphasis-color`, `currentcolor` its
/// `color`).
fn emphasis_color(doc: &BaseDocument, node_id: NodeId) -> Color {
    let Some(styles) = doc.get_node(node_id).and_then(|node| node.primary_styles()) else {
        return Color::BLACK;
    };
    let itext = styles.get_inherited_text();
    itext.text_emphasis_color.resolve_to_absolute(&itext.color).as_srgb_color()
}

/// pyro: draws an annotation's line over (or under) a base spanning `base` (layout x), whose
/// content area spans `content` (block axis): centred over the base, its content area against the
/// base's. Each glyph run in `color`, or (`None`) in its own element's text colour. Returns the
/// outer edge of the annotation's content area (block axis).
#[allow(clippy::too_many_arguments)]
fn draw_annotation<S: PaintScene>(
    scene: &mut S,
    annotation: &TextAnnotation,
    base: (f64, f64),
    content: (f64, f64),
    color: Option<Color>,
    doc: &BaseDocument,
    transform: Affine,
    scale: f64,
    snap: bool,
) -> f64 {
    let Some(line) = annotation.layout.lines().next() else {
        return if annotation.under { content.1 } else { content.0 };
    };
    let metrics = line.metrics();
    let baseline = f64::from(metrics.baseline);
    let y = if annotation.under {
        content.1 + baseline - f64::from(metrics.content_block_min_coord)
    } else {
        content.0 - (f64::from(metrics.content_block_max_coord) - baseline)
    };
    let x = (base.0 + base.1) / 2.0 - f64::from(annotation.layout.width()) / 2.0;
    let transform = transform * Affine::translate((x, y - baseline));
    for item in line.items() {
        let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
            continue;
        };
        let color = color.unwrap_or_else(|| text_fill_color(doc, glyph_run.style().brush.id));
        draw_glyph_run(scene, &glyph_run, color, transform, scale, snap);
    }
    // The outer edge of its content area.
    if annotation.under {
        y + f64::from(metrics.content_block_max_coord) - baseline
    } else {
        y - (baseline - f64::from(metrics.content_block_min_coord))
    }
}

/// pyro: draws a glyph run as `stroke_text` draws one (no shadows, decorations or recording).
fn draw_glyph_run<S: PaintScene>(
    scene: &mut S,
    glyph_run: &parley::GlyphRun<'_, TextBrush>,
    color: Color,
    transform: Affine,
    scale: f64,
    snap: bool,
) {
    let run = glyph_run.run();
    let font_size = run.font_size();
    if font_size <= 0.0 || color.components[3] <= 0.0 {
        return;
    }
    let synthesis = run.synthesis();
    let glyph_xform = synthesis
        .skew()
        .map(|angle| Affine::skew(angle.to_radians().tan() as f64, 0.0));
    let embolden = if FONT_EMBOLDEN_ENABLED {
        font_embolden(font_size as f64, color, transform)
    } else {
        kurbo::Vec2::default()
    } + if synthesis.embolden() {
        synthetic_bold(font_size as f64, scale, color, transform)
    } else {
        kurbo::Vec2::default()
    };
    let normalized_coords: &[i16] = bytemuck::cast_slice(run.normalized_coords());
    let quarter_x = CORE_TEXT_GLYPHS && snap && {
        let [a, b, c, d, _, _] = transform.as_coeffs();
        a == 1.0 && b == 0.0 && c == 0.0 && d == 1.0
    };
    let tx = transform.as_coeffs()[4];
    scene.draw_glyphs(
        run.font(),
        font_size,
        !FONT_EMBOLDEN_ENABLED && !CORE_TEXT_GLYPHS, // hint
        normalized_coords,
        embolden,
        Fill::NonZero,
        &anyrender::Paint::from(color),
        1.0, // alpha
        transform,
        glyph_xform,
        glyph_run.positioned_glyphs().map(|glyph| anyrender::Glyph {
            id: glyph.id as _,
            x: if quarter_x {
                (((tx + glyph.x as f64) * 4.0).round() / 4.0 - tx) as f32
            } else {
                glyph.x
            },
            y: glyph.y,
        }),
    );
}

/// Draw selection highlight rectangles for the given byte range in a layout.
/// Uses Parley's Selection type for accurate geometry calculation.
pub(crate) fn draw_text_selection(
    scene: &mut impl PaintScene,
    layout: &Layout<TextBrush>,
    line_transform: impl Fn(usize) -> Affine,
    selection_start: usize,
    selection_end: usize,
) {
    let anchor = Cursor::from_byte_index(layout, selection_start, Affinity::Downstream);
    let focus = Cursor::from_byte_index(layout, selection_end, Affinity::Downstream);
    let selection = Selection::new(anchor, focus);

    selection.geometry_with(layout, |rect, line_idx| {
        let rect = kurbo::Rect::new(rect.x0, rect.y0, rect.x1, rect.y1);
        scene.fill(Fill::NonZero, line_transform(line_idx), SELECTION_COLOR, None, &rect);
    });
}

/// pyro: how far each side of a glyph's outline grows when bold is synthesized (a font without
/// a face of the weight asked for): Skia's fake bold, which Chromium draws, strokes the outline
/// with a width of the text size times 1/24 at 9px and below, 1/32 at 36px and above, linear
/// between. `font_size` is the run's (device) size, `scale` device pixels per CSS pixel; the
/// result is in pixels at the run's size.
///
/// On macOS the growth is less for dark text. Skia rasterizes the stroked outline itself (Core
/// Text draws only the glyphs Skia leaves alone) and passes its coverage through the table its
/// mask gamma has for the text color, which thins dark text: measured against the plain stroke
/// (Lato, 12 to 144 device px, device scale 1 to 3), each edge loses the same fraction of a
/// device pixel at every size, one of four amounts by the color's level, none for white.
fn synthetic_bold(font_size: f64, scale: f64, color: Color, transform: Affine) -> kurbo::Vec2 {
    /// Device pixels an edge loses, by [`skia_mac_luminance_level`].
    const EDGE_LOSS: [f64; 4] = [0.26, 0.175, 0.085, 0.0];
    let css_size = if scale > 0.0 { font_size / scale } else { font_size };
    let t = ((css_size - 9.0) / (36.0 - 9.0)).clamp(0.0, 1.0);
    let ratio = 1.0 / 24.0 + t * (1.0 / 32.0 - 1.0 / 24.0);
    let mut side = font_size * ratio / 2.0;
    if CORE_TEXT_GLYPHS {
        let device_scale = transform.determinant().abs().sqrt();
        if device_scale > 0.0 && device_scale.is_finite() {
            side = (side - EDGE_LOSS[skia_mac_luminance_level(color)] / device_scale).max(0.0);
        }
    }
    kurbo::Vec2::new(side, side)
}

/// pyro: which of its coverage tables Skia uses for text of a color on macOS, 0 (black) to 3
/// (white). Skia halves the color there (Core Graphics goes from a gamma of 2 for black text
/// to 1 for white), keeps the top three bits of each channel, repeated down the byte, and takes
/// the top three bits of that color's luminance: four levels in all. Measured in Chromium on 49
/// colors, all at the level this gives.
fn skia_mac_luminance_level(color: Color) -> usize {
    let rgba = color.to_rgba8();
    let canonical = |channel: u8| {
        let bits = u32::from(channel / 2 >> 5);
        bits << 5 | bits << 2 | bits >> 1
    };
    let luminance = (canonical(rgba.r) * 54 + canonical(rgba.g) * 183 + canonical(rgba.b) * 19) >> 8;
    (luminance >> 5).min(3) as usize
}

/// Approximates macOS font smoothing (stem dilation). CoreGraphics dilates each side by an amount
/// in device pixels that grows with the device ppem up to 0.3px (fitted to Safari's ink at 9-72px
/// and 1x-3x scale), and keeps dilating at large sizes, so there is no ppem cutoff. glifo applies
/// the embolden in user units, so the device amounts are divided by the transform's scale.
fn font_embolden(font_size: f64, color: Color, transform: Affine) -> kurbo::Vec2 {
    const MAX_DEVICE_EMBOLDEN: f64 = 0.3;
    let [a, b, c, d, _, _] = transform.as_coeffs();
    let (scale_x, scale_y) = (a.hypot(b), c.hypot(d));
    let device_ppem = font_size * (a * d - b * c).abs().sqrt();
    if scale_x == 0.0 || scale_y == 0.0 || !device_ppem.is_finite() {
        return kurbo::Vec2::default();
    }
    let fs = device_ppem * embolden_scale(color);
    kurbo::Vec2::new(
        (0.0121 * fs).min(MAX_DEVICE_EMBOLDEN) / scale_x,
        (0.015125 * fs).min(MAX_DEVICE_EMBOLDEN) / scale_y,
    )
}

/// Scales Pathfinder's stem-darkening factors by the text's relative luminance. CoreText
/// dilates light text more than dark text; the points are fitted to Safari's ink for text
/// colours from black to white on black and white backgrounds.
fn embolden_scale(color: Color) -> f64 {
    const POINTS: [(f64, f64); 4] = [(0.1, 0.5), (0.125, 0.72), (0.35, 0.72), (1.0, 1.3)];
    let lum = color.discard_alpha().relative_luminance() as f64;
    let mut prev = POINTS[0];
    if lum <= prev.0 {
        return prev.1;
    }
    for point in &POINTS[1..] {
        if lum <= point.0 {
            return prev.1 + (point.1 - prev.1) * (lum - prev.0) / (point.0 - prev.0);
        }
        prev = *point;
    }
    prev.1
}

/// pyro: where `text-decoration-skip-ink` leaves gaps in a decoration line spanning `extent`
/// (layout x), as Blink's `TextPainter::ClipDecorationLine` cuts them: around the ink of every
/// glyph on the line inside `band` (layout y, top to bottom: the line less half a pixel at each
/// edge), grown by `dilation` on each side (the line's thickness, at most 13px); not around CJK
/// glyphs unless `cjk` (`skip-ink: all`). Merged and in order.
fn ink_gaps(
    runs: &[parley::GlyphRun<'_, TextBrush>],
    source: Option<&str>,
    extent: (f64, f64),
    band: (f64, f64),
    dilation: f64,
    cjk: bool,
) -> SmallVec<[(f64, f64); 8]> {
    let mut gaps: SmallVec<[(f64, f64); 8]> = SmallVec::new();
    // A 1px line's band is its middle: what crosses that line.
    if band.1 < band.0 {
        return gaps;
    }
    for glyph_run in runs {
        let (run_x0, run_x1) = (f64::from(glyph_run.offset()), f64::from(glyph_run.offset() + glyph_run.advance()));
        if run_x1 + dilation < extent.0 || run_x0 - dilation > extent.1 {
            continue;
        }
        let run = glyph_run.run();
        let (font, size) = (run.font(), run.font_size());
        let coords: &[i16] = bytemuck::cast_slice(run.normalized_coords());
        let reach = f64::from(size) + dilation;
        let mut glyphs = glyph_run.positioned_glyphs();
        'atoms: for (range, count) in glyph_run.atom_texts() {
            // Whether the atom is CJK, looked up only for a glyph that crosses the band.
            let mut atom_is_cjk = None;
            for glyph in glyphs.by_ref().take(count) {
                let x = f64::from(glyph.x);
                // Glyphs far outside the line cannot reach it (their ink is within an em of them);
                // past its end none can (glyphs come left to right, in either direction).
                if x - reach > extent.1 {
                    break 'atoms;
                }
                if x + reach < extent.0 || x - reach > extent.1 {
                    continue;
                }
                // The band in the glyph's own space (y up from its origin).
                let y = f64::from(glyph.y);
                let (lo, hi) = ((y - band.1) as f32, (y - band.0) as f32);
                let Some((a, b)) = glyph_intercept(font, coords, size, glyph.id, lo, hi) else {
                    continue;
                };
                // CJK glyphs are skipped unless `cjk`.
                if !cjk
                    && *atom_is_cjk.get_or_insert_with(|| {
                        source.and_then(|text| text.get(range.clone())?.chars().next()).is_some_and(is_cjk_char)
                    })
                {
                    continue;
                }
                gaps.push((x + f64::from(a) - dilation, x + f64::from(b) + dilation));
            }
        }
    }
    gaps.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: SmallVec<[(f64, f64); 8]> = SmallVec::new();
    for (a, b) in gaps {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    merged
}

/// pyro: whether a character is CJK (Han, kana, Hangul, their punctuation and compatibility
/// forms), whose glyphs `text-decoration-skip-ink: auto` does not skip (Blink's
/// `InkSkipCjkHandling::kExcludeCjk`).
fn is_cjk_char(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF
        | 0x2E80..=0x2FDF
        | 0x3000..=0x9FFF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7FF
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFFEF
        | 0x20000..=0x3FFFF)
}

/// pyro: `text-decoration-skip-ink`: the extent along the baseline (font px, from the glyph's
/// origin) of the ink of glyph `glyph` that lies in the band `lo..=hi` (font px above the
/// baseline, y up), as Skia's `SkFont::getIntercepts` gives it: the leftmost and rightmost
/// points of the outline inside the band. Cached by font, size, glyph and band (a paint asks for
/// the same glyphs of the same links again and again).
fn glyph_intercept(font: &parley::FontData, coords: &[i16], size: f32, glyph: u32, lo: f32, hi: f32) -> Option<(f32, f32)> {
    use skrifa::MetadataProvider as _;
    use skrifa::instance::{LocationRef, NormalizedCoord, Size};
    use std::cell::RefCell;
    use std::hash::{Hash, Hasher};

    /// A multiplicative hash of the key's words (the keys are a font id, numbers and float bits).
    #[derive(Default)]
    struct KeyHasher(u64);
    impl Hasher for KeyHasher {
        fn finish(&self) -> u64 {
            self.0
        }
        fn write(&mut self, bytes: &[u8]) {
            for &byte in bytes {
                self.write_u64(u64::from(byte));
            }
        }
        fn write_u64(&mut self, word: u64) {
            self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
        }
    }
    type Cache = std::collections::HashMap<[u64; 4], Option<(f32, f32)>, std::hash::BuildHasherDefault<KeyHasher>>;
    thread_local! {
        static INTERCEPTS: RefCell<Cache> = RefCell::default();
    }
    let coords_hash = if coords.is_empty() {
        0
    } else {
        let mut hasher = KeyHasher::default();
        coords.hash(&mut hasher);
        hasher.finish()
    };
    let key = [
        font.data.id(),
        u64::from(font.index) << 32 | u64::from(glyph),
        u64::from(size.to_bits()) << 32 | u64::from(coords_hash as u32),
        u64::from(lo.to_bits()) << 32 | u64::from(hi.to_bits()),
    ];
    if let Some(known) = INTERCEPTS.with(|cache| cache.borrow().get(&key).copied()) {
        return known;
    }

    /// The x extent of the parts of an outline inside a horizontal band.
    struct BandPen {
        lo: f32,
        hi: f32,
        at: (f32, f32),
        start: (f32, f32),
        extent: Option<(f32, f32)>,
    }
    impl BandPen {
        fn segment(&mut self, to: (f32, f32)) {
            let (from, (lo, hi)) = (self.at, (self.lo, self.hi));
            self.at = to;
            let ((x0, y0), (x1, y1)) = (from, to);
            if (y0 < lo && y1 < lo) || (y0 > hi && y1 > hi) {
                return;
            }
            let (ta, tb) = if y0 == y1 {
                (0.0, 1.0)
            } else {
                let (a, b) = ((lo - y0) / (y1 - y0), (hi - y0) / (y1 - y0));
                (a.min(b).max(0.0), a.max(b).min(1.0))
            };
            if ta > tb {
                return;
            }
            for t in [ta, tb] {
                let x = x0 + (x1 - x0) * t;
                let e = self.extent.get_or_insert((x, x));
                *e = (e.0.min(x), e.1.max(x));
            }
        }
    }
    impl skrifa::outline::OutlinePen for BandPen {
        fn move_to(&mut self, x: f32, y: f32) {
            self.at = (x, y);
            self.start = (x, y);
        }
        fn line_to(&mut self, x: f32, y: f32) {
            self.segment((x, y));
        }
        fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
            let (x0, y0) = self.at;
            const STEPS: usize = 8;
            for i in 1..=STEPS {
                let t = i as f32 / STEPS as f32;
                let u = 1.0 - t;
                self.segment((u * u * x0 + 2.0 * u * t * cx0 + t * t * x, u * u * y0 + 2.0 * u * t * cy0 + t * t * y));
            }
        }
        fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
            let (x0, y0) = self.at;
            const STEPS: usize = 12;
            for i in 1..=STEPS {
                let t = i as f32 / STEPS as f32;
                let u = 1.0 - t;
                let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                self.segment((a * x0 + b * cx0 + c * cx1 + d * x, a * y0 + b * cy0 + c * cy1 + d * y));
            }
        }
        fn close(&mut self) {
            let start = self.start;
            self.segment(start);
        }
    }

    let extent = (|| {
        let font_ref = skrifa::FontRef::from_index(font.data.as_ref(), font.index).ok()?;
        let outline = font_ref.outline_glyphs().get(skrifa::GlyphId::new(glyph))?;
        let location: Vec<NormalizedCoord> = coords.iter().map(|&c| NormalizedCoord::from_bits(c)).collect();
        let mut pen = BandPen { lo, hi, at: (0.0, 0.0), start: (0.0, 0.0), extent: None };
        outline
            .draw(skrifa::outline::DrawSettings::unhinted(Size::new(size), LocationRef::new(&location)), &mut pen)
            .ok()?;
        pen.extent
    })();
    INTERCEPTS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= 1 << 16 {
            cache.clear();
        }
        cache.insert(key, extent);
    });
    extent
}
