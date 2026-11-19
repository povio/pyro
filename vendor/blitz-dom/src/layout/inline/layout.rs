use blitz_traits::node_id::NodeId;
use parley::{AlignmentOptions, BreakReason, IndentOptions};
use style::values::specified::box_::{DisplayInside, DisplayOutside};
use style::values::{
    computed::{BaselineSource, CSSPixelLength, Contain},
    generics::text::GenericTextIndent,
};
use stylo_taffy::CopyStyleGetters;
use taffy::{
    AvailableSpace, AxisStaticPosition, BlockContainerStyle, BlockContext, BlockFormattingContext,
    BoxSizing, CollapsibleMarginSet, CompactLength, CoreStyle as _, Direction, LayoutInput,
    LayoutOutput, LayoutPartialTree as _, MaybeMath as _, MaybeResolve as _, OofCandidate,
    OofCandidates, OofItemStyle, OofPositioningArea, Overflow, Point, RequestedAxis,
    ResolveOrZero as _, RunMode, Size, SizingMode,
};

#[cfg(feature = "floats")]
use parley::{BreakerState, LineBreakData, YieldData};
#[cfg(feature = "floats")]
use taffy::{
    BlockItemStyle as _, Clear, ContentSlot, Float, FloatInsets, prelude::TaffyMaxContent,
};

use stylo_taffy::StyleFlags;

use crate::layout::LayoutPassState;
use crate::node::NodeFlags;
use crate::layout::resolve_calc_value;
use crate::stylo_to_parley;

/// Subtract a child's margins from the definite axes of the available space it is laid out in.
/// Taffy's convention is that the parent subtracts a child's margins from the available space
/// before passing it to the child, and that the child does not subtract them again.
fn subtract_margins(mut child_inputs: LayoutInput, margin: taffy::Rect<f32>) -> LayoutInput {
    child_inputs.available_space = child_inputs
        .available_space
        .maybe_sub(margin.sum_axes())
        .maybe_max(Size::ZERO);
    child_inputs
}

/// Layout inputs for an atomic inline box, with any sizing keyword on its `width` style
/// (`min-content`, `max-content`, `fit-content`, `fit-content(...)`, `stretch`) resolved
/// into the available space or known width the box is measured with.
fn inline_box_inputs(
    width_style: taffy::Dimension,
    height_style: taffy::Dimension,
    stretch_height: Option<f32>,
    margin: taffy::Rect<f32>,
    child_inputs: LayoutInput,
) -> LayoutInput {
    let mut inputs = subtract_margins(child_inputs, margin);
    // pyro: `height: stretch` fills the definite height of the containing block (`stretch_height`,
    // its content box), less the box's margins (WPT css-sizing `stretch/*`; an `inline-block`,
    // a button, an input with `height: -webkit-fill-available`). Replaced boxes resolve it
    // themselves (`replaced.rs`).
    if let (true, Some(height)) = (height_style.is_stretch(), stretch_height) {
        inputs.known_dimensions.height = Some((height - margin.vertical_axis_sum()).max(0.0));
    }
    let stretch_width = inputs.available_space.width.into_option();
    let percent_basis = child_inputs.parent_size.width;

    match width_style.tag() {
        CompactLength::MIN_CONTENT_TAG => inputs.available_space.width = AvailableSpace::MinContent,
        CompactLength::MAX_CONTENT_TAG => inputs.available_space.width = AvailableSpace::MaxContent,
        CompactLength::FIT_CONTENT_PX_TAG => {
            inputs.available_space.width = AvailableSpace::Definite(width_style.value())
        }
        CompactLength::FIT_CONTENT_PERCENT_TAG => {
            if let Some(basis) = percent_basis {
                inputs.available_space.width =
                    AvailableSpace::Definite(basis * width_style.value());
            }
        }
        CompactLength::FIT_CONTENT_KEYWORD_TAG => {
            if let Some(width) = stretch_width {
                inputs.available_space.width = AvailableSpace::Definite(width);
            }
        }
        _ if width_style.is_fit_content_calc() => {
            if let Some(basis) = percent_basis {
                inputs.available_space.width =
                    AvailableSpace::Definite(resolve_calc_value(width_style.calc_value(), basis));
            }
        }
        CompactLength::STRETCH_TAG => {
            if let Some(width) = stretch_width {
                inputs.known_dimensions.width = Some(width);
                inputs.available_space.width = AvailableSpace::Definite(width);
            }
        }
        _ => {}
    }
    inputs
}

/// `v` (device px at `scale`) rounded up to a layout unit (1/64 CSS px);
/// float noise within 1/100 of a unit rounds down.
fn ceil_layout_unit(v: f32, scale: f32) -> f32 {
    ((v / scale * 64.0 - 0.01).ceil() / 64.0) * scale
}

/// pyro: the ink of the glyphs of `layout` from their outlines' bounding boxes, `[left, top,
/// right, bottom]` in the layout's units (device px) from its origin. `None` without ink.
fn glyph_ink_bounds(layout: &parley::Layout<crate::node::TextBrush>) -> Option<[f32; 4]> {
    use skrifa::instance::{LocationRef, NormalizedCoord, Size as FontSize};
    use skrifa::MetadataProvider as _;

    /// The bounds of the points of an outline (its control points: a little more than its ink
    /// where it curves), for fonts whose glyphs carry no bounding box (CFF).
    #[derive(Default)]
    struct BoundsPen(Option<[f32; 4]>);
    impl BoundsPen {
        fn add(&mut self, x: f32, y: f32) {
            self.0 = Some(match self.0 {
                None => [x, y, x, y],
                Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
            });
        }
    }
    impl skrifa::outline::OutlinePen for BoundsPen {
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

    let mut ink: Option<[f32; 4]> = None;
    for line in layout.lines() {
        for item in line.items() {
            let parley::PositionedLayoutItem::GlyphRun(glyph_run) = item else { continue };
            let run = glyph_run.run();
            let font = run.font();
            let Ok(font_ref) = skrifa::FontRef::from_index(font.data.as_ref(), font.index) else { continue };
            let coords: Vec<NormalizedCoord> =
                run.normalized_coords().iter().map(|coord| NormalizedCoord::from_bits(coord.to_bits())).collect();
            let size = FontSize::new(run.font_size());
            let location = LocationRef::new(&coords);
            let metrics = font_ref.glyph_metrics(size, location);
            let outlines = font_ref.outline_glyphs();
            for glyph in glyph_run.positioned_glyphs() {
                let glyph_id = skrifa::GlyphId::new(glyph.id);
                // Font units scaled, y up from the glyph's origin.
                let bounds = metrics.bounds(glyph_id).map(|b| [b.x_min, b.y_min, b.x_max, b.y_max]).or_else(|| {
                    let mut pen = BoundsPen::default();
                    outlines
                        .get(glyph_id)?
                        .draw(skrifa::outline::DrawSettings::unhinted(size, location), &mut pen)
                        .ok()?;
                    pen.0
                });
                let Some([x_min, y_min, x_max, y_max]) = bounds else { continue };
                if x_max <= x_min || y_max <= y_min {
                    continue;
                }
                let rect = [glyph.x + x_min, glyph.y - y_max, glyph.x + x_max, glyph.y - y_min];
                ink = Some(match ink {
                    None => rect,
                    Some(i) => [i[0].min(rect[0]), i[1].min(rect[1]), i[2].max(rect[2]), i[3].max(rect[3])],
                });
            }
        }
    }
    ink
}

/// pyro: where the border box of an initial letter box starts below the top of the first line
/// (with `ascent`, its first baseline, and `height`, its border box's), and how far the line's
/// text moves down (raised and sunken letters): Blink's legacy `ComputeInitialLetterBoxBlockOffset`
/// (the size rounded up for the lines, but not for the cap height), in the lines' axes.
fn initial_letter_offset(letter: &crate::node::InitialLetter, ascent: f32, height: f32) -> (f32, f32) {
    let size = letter.size.ceil() as i32;
    if size < letter.sink {
        // Its bottom at the bottom of the sink's line.
        return (letter.line_height * letter.sink as f32 - height, 0.0);
    }
    let shift = letter.line_height * (size - letter.sink) as f32;
    if letter.centered {
        // Vertical lines of glyphs that are not sideways: centred on the lines it spans.
        return ((letter.line_height * size as f32 - height) / 2.0, shift);
    }
    (letter.line_height * letter.size - ascent - letter.descent, shift)
}

/// pyro: whether the lines of a root in writing mode `wm` align on a central baseline (vertical,
/// text not sideways), and whether they are inverted (`vertical-lr`: line-over at block-end).
fn vertical_line_kind(wm: stylo_taffy::WritingMode) -> (bool, bool) {
    let central = wm.is_vertical() && !wm.is_sideways();
    (central, central && wm.intersects(stylo_taffy::WritingMode::LINE_INVERTED))
}

/// pyro: whether the lines of a root in writing mode `wm` are sideways and inverted (`vertical-lr`
/// with `text-orientation: sideways`): their glyphs' tops, the line-over side, face block-end, so
/// their alphabetic baseline and line-under side are toward block-start. Parley lays a line out
/// over-side first and paint turns it within its slot (`draw_vertical_inline_layout`); its inline
/// boxes and the baselines the root exports are turned the same way (WPT css-flexbox
/// `baseline-synthesis-003`, css-grid `grid-self-baseline-004`).
fn sideways_inverted_lines(wm: stylo_taffy::WritingMode) -> bool {
    wm.is_vertical() && wm.is_sideways() && wm.intersects(stylo_taffy::WritingMode::LINE_INVERTED)
}

/// A float met mid-line, laid out but not yet placed in the block formatting context
#[cfg(feature = "floats")]
struct PendingFloat {
    node_id: NodeId,
    /// pyro: the index of its item in the layout (parley `BreakerState::item_index`)
    item: usize,
    output: LayoutOutput,
    margin: taffy::Rect<f32>,
    direction: taffy::FloatDirection,
    clear: Clear,
}

impl LayoutPassState<'_> {
    /// pyro: how many lines the line clamp of an ancestor of `node_id` (in its block formatting
    /// context) leaves to it: the clamp's count less the lines laid out before it; `None` without
    /// one (or one without a count, `line-clamp: auto`).
    fn clamp_lines_left(&self, node_id: NodeId) -> Option<usize> {
        fn lines_in(nodes: &crate::NodeTree, id: NodeId, depth: u32) -> usize {
            let node = &nodes[id];
            if let Some(text) = node.data.downcast_element().and_then(|el| el.inline_layout_data.as_deref()) {
                return text.counted_lines as usize;
            }
            if depth > 32 {
                return 0;
            }
            node.layout_children.borrow().iter().flatten().map(|&c| lines_in(nodes, c, depth + 1)).sum()
        }
        let mut used = 0usize;
        let mut child = node_id;
        for _ in 0..64 {
            let parent = self.nodes[child].layout_parent.get()?;
            let node = &self.nodes[parent];
            if !node.is_anonymous() && node.taffy_display() != taffy::Display::Block {
                return None;
            }
            if let Some(children) = node.layout_children.borrow().as_ref() {
                for &sibling in children.iter().take_while(|&&c| c != child) {
                    if !self.nodes[sibling].is_out_of_flow() {
                        used += lines_in(&self.nodes, sibling, 0);
                    }
                }
            }
            if let Some(spec) = self.line_clamp_of(parent) {
                // (`line-clamp: auto` balances first and clamps after, as Chromium does: WPT
                // css-overflow `line-clamp-balance-007`, `-010`.)
                return (spec.lines != u32::MAX).then(|| (spec.lines as usize).saturating_sub(used));
            }
            child = parent;
        }
        None
    }
}

/// pyro: the advance at which the lines of a `text-wrap-style: balance` block wrap: the least
/// one that breaks `layout` into as many lines as `max_advance` does. `None` when there is
/// nothing to balance (one line) or too much (more than Blink's six lines).
fn balanced_wrap_limit(
    layout: &mut parley::Layout<crate::node::TextBrush>,
    max_advance: f32,
    scale: f32,
    max_lines: usize,
) -> Option<f32> {
    const MAX_LINES: usize = 6;
    if !max_advance.is_finite() || max_advance <= 0.0 {
        return None;
    }
    // Lines that overflow or end at an emergency break (`overflow-wrap`) are no balance.
    let breaks = |layout: &parley::Layout<crate::node::TextBrush>, width: f32| {
        let mut emergency = 0;
        let mut overflows = 0;
        for line in layout.lines() {
            let metrics = line.metrics();
            emergency += usize::from(line.break_reason() == parley::layout::BreakReason::Emergency);
            overflows +=
                usize::from(metrics.indent + metrics.advance - metrics.hanging_advance > width + 0.01 * scale);
        }
        (layout.len(), emergency, overflows)
    };
    layout.break_all_lines(Some(max_advance));
    let (lines, emergency, overflows) = breaks(layout, max_advance);
    if !(2..=MAX_LINES.min(max_lines)).contains(&lines) {
        return None;
    }
    let (mut low, mut high) = (0.0f32, max_advance);
    // To a quarter of a CSS pixel.
    while high - low > 0.25 * scale {
        let middle = (low + high) / 2.0;
        layout.break_all_lines(Some(middle));
        let now = breaks(layout, middle);
        if now.0 <= lines && now.1 <= emergency && now.2 <= overflows {
            high = middle;
        } else {
            low = middle;
        }
    }
    (high < max_advance).then_some(high)
}

/// pyro: `text-wrap: balance` beside floats, as in Blink: every line's room to wrap in is
/// narrowed by the same amount, the most (to a quarter of a CSS pixel) that keeps the lines'
/// count, emergency breaks and block size, found by laying the lines out again from the floats
/// as they were before them (`BlockContext::float_snapshot`) for each width tried.
#[cfg(feature = "floats")]
struct FloatBalance {
    snapshot: taffy::FloatSnapshot,
    placed_floats: usize,
    br_clear_floor: f32,
    max_lines: usize,
    /// The lines, emergency breaks and bottom of the unbalanced layout.
    first: Option<(usize, usize, f64)>,
    /// The bisection: `low` keeps the layout, `high` does not.
    low: f32,
    high: f32,
    /// The reduction the last layout was made with.
    reduction: f32,
}

#[cfg(feature = "floats")]
impl FloatBalance {
    const MAX_LINES: usize = 6;

    /// The reduction to lay out with next after a layout with `self.reduction` gave `result`
    /// (lines, emergency breaks, bottom), or `None` when that layout is the one to keep.
    fn next(&mut self, result: (usize, usize, f64), scale: f32) -> Option<f32> {
        let Some(first) = self.first else {
            if !(2..=self.max_lines).contains(&result.0) {
                return None;
            }
            self.first = Some(result);
            return Some((self.low + self.high) / 2.0);
        };
        let keeps = result.0 <= first.0 && result.1 <= first.1 && result.2 <= first.2 + 0.01 * f64::from(scale);
        if keeps {
            self.low = self.reduction;
        } else {
            self.high = self.reduction;
        }
        if self.high - self.low > 0.25 * scale {
            return Some((self.low + self.high) / 2.0);
        }
        (self.reduction != self.low).then_some(self.low)
    }
}

/// Tolerance (in CSS px) for content fitting into a slot
#[cfg(feature = "floats")]
const FIT_TOLERANCE: f32 = 0.001;

/// Lay out the next line into `slot` (content-box relative, in CSS px)
#[cfg(feature = "floats")]
fn set_line_slot(state: &mut BreakerState, slot: &ContentSlot, scale: f32, slack: f32, mirror: bool) {
    // With huge insets, f32 rounding can make the slot slightly wider than the layout
    let max_advance = (slot.width * scale + slack).min(state.layout_max_advance());
    state.set_line_max_advance(max_advance);
    // pyro: the lines of `sideways-lr` run from the bottom, Taffy's right: a slot (left to
    // right in Taffy's axes) starts that far from the line-left side.
    let layout_width = state.layout_max_advance();
    if mirror && layout_width < f32::MAX / 2.0 {
        state.set_line_x((layout_width - (slot.x + slot.width) * scale).max(0.0));
    } else {
        state.set_line_x(slot.x * scale);
    }
    state.set_line_y((slot.y * scale) as f64);
}

/// The slot to retry a just-broken line in if it doesn't fit the slot it was laid out into
/// (CSS 2.1 §9.5): a line taller than its slot is retried in the slot narrowed by the floats
/// beside its full height, and a line whose content overflows its slot is moved down to the
/// next slot (or below the floats if there is none). Like browsers, content that can't wrap
/// (`text-wrap-mode: nowrap`) is left to overflow its slot instead of being moved down.
#[cfg(feature = "floats")]
fn retry_line_slot(
    block_ctx: &BlockContext,
    slot: &ContentSlot,
    line: &LineBreakData,
    wraps: bool,
    scale: f32,
) -> Option<ContentSlot> {
    let segment_id = slot.segment_id?;
    let line_y = (line.line_y_start / scale as f64) as f32;
    let line_height = ((line.line_y_end - line.line_y_start) / scale as f64) as f32;
    let advance = (line.metrics.advance - line.metrics.hanging_advance) / scale;

    if wraps && advance > slot.width + FIT_TOLERANCE {
        let next_slot =
            block_ctx.find_content_slot(line_y, line_height, Clear::None, Some(segment_id));
        if next_slot.segment_id.is_some() {
            return Some(next_slot);
        }
        return Some(block_ctx.find_content_slot(line_y, line_height, Clear::Both, None));
    }

    if line_height <= slot.height {
        return None;
    }
    let narrowed_slot = block_ctx.find_content_slot(line_y, line_height, Clear::None, None);
    let narrowed = narrowed_slot.x > slot.x + FIT_TOLERANCE
        || narrowed_slot.width < slot.width - FIT_TOLERANCE;
    narrowed.then_some(narrowed_slot)
}

/// pyro: the slot to retry a line in that a retry narrowed for a taller attempt, when the
/// line's own height leaves it more room (the content that made the attempt taller moved on to
/// the next line): the slot beside the floats over the line's own height, if wider. Taken once
/// per line, so that the line cannot go back and forth.
#[cfg(feature = "floats")]
fn wider_line_slot(block_ctx: &BlockContext, slot: &ContentSlot, line: &LineBreakData, scale: f32) -> Option<ContentSlot> {
    slot.segment_id?;
    let line_y = (line.line_y_start / scale as f64) as f32;
    let line_height = ((line.line_y_end - line.line_y_start) / scale as f64) as f32;
    let wider = block_ctx.find_content_slot(line_y, line_height, Clear::None, None);
    let widens = wider.x < slot.x - FIT_TOLERANCE || wider.x + wider.width > slot.x + slot.width + FIT_TOLERANCE;
    (widens && (wider.y - line_y).abs() <= FIT_TOLERANCE && wider.x <= slot.x + FIT_TOLERANCE && wider.x + wider.width >= slot.x + slot.width - FIT_TOLERANCE).then_some(wider)
}

impl LayoutPassState<'_> {
    /// Place a float laid out by inline layout at or below `min_y` (content-box relative)
    #[cfg(feature = "floats")]
    fn place_inline_float(
        &mut self,
        block_ctx: &mut BlockContext,
        float: PendingFloat,
        min_y: f32,
        container_pb: taffy::Rect<f32>,
        oof_candidates: &mut OofCandidates,
        span_offset: taffy::Point<f32>,
        containing_block_width: f32,
        containing_block_height: Option<f32>,
        containing_block_rtl: bool,
    ) {
        let PendingFloat {
            node_id,
            item: _,
            mut output,
            margin,
            direction,
            clear,
        } = float;

        // Note: `pos` is content-box relative
        let margin_box = output.size + margin.sum_axes();
        // pyro: a float with a `shape-outside` float area (CSS Shapes 1)
        let wm = self.layout_wm;
        let node = &self.nodes[node_id];
        let image = node.element_data().and_then(|e| e.shape_images.first()).and_then(Option::as_ref).map(|d| &d.image);
        let bands = node.primary_styles().and_then(|style| {
            crate::layout::shape_outside::float_bands_for(&style, margin_box, margin, wm, containing_block_width, image)
        });
        let id = crate::taffy_node_id(node_id).into();
        let pos = match bands {
            Some(bands) => block_ctx.place_shaped_floated_box(id, margin_box, min_y, direction, clear, false, &bands),
            None => block_ctx.place_floated_box(id, margin_box, min_y, direction, clear, false),
        };

        // pyro: relatively positioned inline boxes around the float move it as they move the rest
        // of their content (CSS 2.1 § 9.4.3), not where it is placed (WPT
        // `block-in-inline-float-between-001`).
        // pyro: and its own `position: relative` offset moves it from where it is placed (CSS 2.1
        // § 9.4.3; WPT css-overflow `line-clamp-with-floats-008`). Its containing block's
        // direction decides between `left` and `right` (§ 9.4.3); a percentage `top` is against its
        // containing block's height when that is definite, else `auto` (§ 9.3.2).
        let cb_rtl = containing_block_rtl;
        let own = self.nodes[node_id]
            .primary_styles()
            .filter(|style| style.clone_position() == style::computed_values::position::T::Relative)
            .map_or(taffy::Point::ZERO, |style| {
                let mut own = crate::layout::inline::relative::relative_offset(
                    &style,
                    taffy::Size { width: containing_block_width, height: containing_block_height.unwrap_or(0.0) },
                    cb_rtl,
                );
                let percent_y = |inset: &style::values::computed::Inset| {
                    matches!(inset, style::values::computed::Inset::LengthPercentage(lp) if lp.has_percentage())
                };
                let position = style.get_position();
                let y_inset = if !matches!(position.top, style::values::computed::Inset::Auto) {
                    &position.top
                } else {
                    &position.bottom
                };
                if containing_block_height.is_none() && percent_y(y_inset) {
                    own.y = 0.0;
                }
                own
            });
        let location = taffy::Point {
            x: pos.x + margin.left + container_pb.left + span_offset.x + own.x,
            y: pos.y + margin.top + container_pb.top + span_offset.y + own.y,
        };
        let size = output.size;
        self.nodes[node_id].update_unrounded_layout(|layout| {
            layout.size = size;
            layout.location = location;
            layout.margin = margin;
        });
        self.inline_placed_floats.push((node_id, pos.y));

        // Translate anchors from item-relative to container-relative
        // coordinates and collect candidates bubbled from the float's subtree
        if !output.oof_candidates.is_empty() {
            output.oof_candidates.translate(location);
            oof_candidates.append(&mut output.oof_candidates);
        }
    }

    /// pyro: the layout of an initial letter box (`NodeFlags::IS_INITIAL_LETTER_BOX`,
    /// `layout::first_letter`): its letter on one line; the box as wide as its text indent and
    /// the ink of the glyphs, and as tall as the ink (Blink's `CalculateInitialLetterBoxInlineSize`
    /// and `AdjustInitialLetterInTextPosition`, the ink rounded out to layout units); the ink's
    /// top-left corner at the content box's top-left corner after the indent; the first baseline
    /// the ink's ascent below the content box's top.
    fn compute_initial_letter_box_layout(&mut self, node_id: NodeId, inputs: LayoutInput) -> LayoutOutput {
        let scale = self.viewport.scale();
        let style = self.nodes[node_id].layout_style();
        let padding = style.padding().resolve_or_zero(inputs.parent_size.width, resolve_calc_value);
        let border = style.border().resolve_or_zero(inputs.parent_size.width, resolve_calc_value);
        drop(style);
        let pb = padding + border;
        let indent = self.nodes[node_id]
            .primary_styles()
            .map_or(0.0, |s| s.slow_clone_text_indent().length.resolve(CSSPixelLength::new(0.0)).px());
        let Some(mut text) = self.nodes[node_id].data.downcast_element_mut().and_then(|e| e.take_inline_layout()) else {
            return LayoutOutput::HIDDEN;
        };
        let options = AlignmentOptions { align_when_overflowing: false, last_line_alignment: None };
        // pyro: preserved white space before the letter is in the box, before the ink, and a tab
        // goes to the paragraph's tab stops (the root's space, not the letter's: Blink; WPT
        // css-inline `initial-letter-with-tab`).
        let leading_space = text.text.starts_with(|c: char| c.is_whitespace());
        if leading_space {
            let parent_size = self.nodes[node_id]
                .parent
                .and_then(|parent| self.nodes[parent].primary_styles())
                .map(|s| s.get_font().font_size.used_size.0.px());
            let own_size = self.nodes[node_id].primary_styles().map(|s| s.get_font().font_size.used_size.0.px());
            if let (Some(parent_size), Some(own_size)) = (parent_size, own_size)
                && own_size > 0.0
            {
                let (space, min_advance) = text.layout.tab_stops();
                let ratio = parent_size / own_size;
                text.layout.set_tab_stops(space * ratio, min_advance * ratio);
            }
        }
        text.layout.set_text_indent(0.0, IndentOptions::default());
        text.layout.break_all_lines(None);
        text.layout.align(parley::Alignment::Start, options);
        let baseline = text.layout.get(0).map_or(0.0, |line| line.metrics().baseline) / scale;
        let out = |v: f32, up: bool| {
            let v = v * 64.0;
            (if up { (v - 0.01).ceil() } else { (v + 0.01).floor() }) / 64.0
        };
        let [left, top, right, bottom] = match glyph_ink_bounds(&text.layout) {
            Some([l, t, r, b]) => [out(l / scale, false), out(t / scale, false), out(r / scale, true), out(b / scale, true)],
            None => [0.0, 0.0, text.layout.width() / scale, text.layout.height() / scale],
        };
        let (left, right) = match leading_space {
            // Right-to-left, the white space is on the ink's right.
            true if text.layout.is_rtl() => (left, right.max(out(text.layout.full_width() / scale, true))),
            true => (left.min(0.0), right),
            false => (left, right),
        };
        text.layout.set_text_indent((indent - left) * scale, IndentOptions::default());
        text.layout.break_all_lines(None);
        text.layout.align(parley::Alignment::Start, options);
        text.block_offset = -top;
        text.hit_reach = None;
        text.line_box_top = pb.top;
        text.counted_lines = 1;
        let size = Size {
            width: (indent + right - left).max(0.0) + pb.left + pb.right,
            height: (bottom - top).max(0.0) + pb.top + pb.bottom,
        };
        let first = pb.top + baseline - top;
        self.nodes[node_id].data.downcast_element_mut().unwrap().inline_layout_data = Some(text);
        LayoutOutput::from_sizes_and_baselines(
            size,
            taffy::ScrollableOverflow::ZERO,
            taffy::Baselines::new(Some(first), Some(first)),
        )
    }

    pub(crate) fn compute_inline_layout(
        &mut self,
        node_id: NodeId,
        inputs: taffy::tree::LayoutInput,
        block_ctx: Option<&mut BlockContext<'_>>,
    ) -> taffy::LayoutOutput {
        // (A floated first letter's box is laid out as any inline root.)
        if self.nodes[node_id].flags.contains(NodeFlags::IS_INITIAL_LETTER_BOX)
            && self.nodes[node_id].primary_styles().is_some_and(|style| style.slow_clone_initial_letter().size > 0.0)
        {
            return self.compute_initial_letter_box_layout(node_id, inputs);
        }
        // pyro: the floats this inline layout places (`inline_placed_floats`), apart from those
        // of the layouts it runs.
        let outer_floats = core::mem::take(&mut self.inline_placed_floats);
        let output = self.compute_inline_root_layout(node_id, inputs, block_ctx);
        self.inline_placed_floats = outer_floats;
        output
    }

    fn compute_inline_root_layout(
        &mut self,
        node_id: NodeId,
        inputs: taffy::tree::LayoutInput,
        block_ctx: Option<&mut BlockContext<'_>>,
    ) -> taffy::LayoutOutput {
        let LayoutInput {
            known_dimensions,
            parent_size,
            run_mode,
            ..
        } = inputs;
        let mut style = self.nodes[node_id].layout_style();
        // pyro: a box laid out in an orthogonal flow resolves its percentage padding against its
        // containing block's inline size, not the transposed parent size (`layout::writing_mode`).
        style.set_percent_basis(self.orthogonal_percent_basis_of(node_id));

        // Pull these out earlier to avoid borrowing issues
        let is_scroll_container =
            style.overflow().x.is_scroll_container() || style.overflow().y.is_scroll_container();
        let padding = style
            .padding()
            .resolve_or_zero(parent_size.width, resolve_calc_value);
        let border = style
            .border()
            .resolve_or_zero(parent_size.width, resolve_calc_value);
        let padding_border_size = (padding + border).sum_axes();
        let box_sizing_adjustment = if style.box_sizing() == BoxSizing::ContentBox {
            padding_border_size
        } else {
            Size::ZERO
        };

        // Resolve node's preferred/min/max sizes (width/heights) against the available space (percentages resolve to pixel values)
        // For ContentSize mode, we pretend that the node has no size styles as these should be ignored.
        let (clamped_style_size, min_size, max_size, _aspect_ratio) = match inputs.sizing_mode {
            SizingMode::ContentSize => {
                let node_size = known_dimensions;
                let node_min_size = Size::NONE;
                let node_max_size = Size::NONE;
                (node_size, node_min_size, node_max_size, None)
            }
            SizingMode::InherentSize => {
                let aspect_ratio = style.aspect_ratio();
                let style_size = style
                    .size()
                    .maybe_resolve(parent_size, resolve_calc_value)
                    .maybe_apply_aspect_ratio(aspect_ratio)
                    .maybe_add(box_sizing_adjustment);
                let style_min_size = style
                    .min_size()
                    .maybe_resolve(parent_size, resolve_calc_value)
                    .maybe_apply_aspect_ratio(aspect_ratio)
                    .maybe_add(box_sizing_adjustment);
                let style_max_size = style
                    .max_size()
                    .maybe_resolve(parent_size, resolve_calc_value)
                    .maybe_add(box_sizing_adjustment);

                let node_size =
                    known_dimensions.or(style_size.maybe_clamp(style_min_size, style_max_size));
                (node_size, style_min_size, style_max_size, aspect_ratio)
            }
        };

        // If both min and max in a given axis are set and max <= min then this determines the size in that axis
        let min_max_definite_size = min_size.zip_map(max_size, |min, max| match (min, max) {
            (Some(min), Some(max)) if max <= min => Some(min),
            _ => None,
        });

        let styled_based_known_dimensions = known_dimensions
            .or(min_max_definite_size)
            .or(clamped_style_size)
            .maybe_max(padding_border_size);

        // Short-circuit layout if the container's size is fully determined by the container's size and the run mode
        // is ComputeSize (and thus the container's size is all that we're interested in)
        if run_mode == RunMode::ComputeSize {
            if let Size {
                width: Some(width),
                height: Some(height),
            } = styled_based_known_dimensions
            {
                return LayoutOutput::from_outer_size(Size { width, height });
            }
        }

        drop(style);

        // Unwrap the block formatting context if one was passed, or else create a new one
        match block_ctx {
            Some(inherited_bfc) if !is_scroll_container => self.compute_inline_layout_inner(
                node_id,
                LayoutInput {
                    known_dimensions: styled_based_known_dimensions,
                    ..inputs
                },
                inherited_bfc,
            ),
            _ => {
                let mut root_bfc = BlockFormattingContext::new();
                let mut root_ctx = root_bfc.root_block_context();
                self.compute_inline_layout_inner(
                    node_id,
                    LayoutInput {
                        known_dimensions: styled_based_known_dimensions,
                        ..inputs
                    },
                    &mut root_ctx,
                )
            }
        }
    }

    /// The out-of-flow positions for which an inline span between the inline box `box_id` and its
    /// inline root `root_id` is a containing block (as `SPAN_*_CB` flags)
    fn inline_span_cb_flags(&self, root_id: NodeId, box_id: NodeId) -> StyleFlags {
        let root = &self.nodes[root_id];
        // The spans of an anonymous inline root are descendants of its parent
        let root_parent = root.is_anonymous().then_some(root.parent).flatten();

        let mut flags = StyleFlags::empty();
        let mut current = self.nodes[box_id].parent;
        while let Some(id) = current {
            if id == root_id || Some(id) == root_parent {
                return flags;
            }
            let ancestor = &self.nodes[id];
            if let Some(style) = ancestor.primary_styles() {
                let display = style.clone_display();
                if display.outside() == DisplayOutside::Inline
                    && display.inside() == DisplayInside::Flow
                {
                    let claims = stylo_taffy::convert::inline_containing_block_claims(&style);
                    // pyro: not a split inline's: its parent lays its boxes out
                    // (`split_inline_containing_block`).
                    if claims.absolute && !(ancestor.is_split_inline() && self.split_inline_contains_absolute(id)) {
                        flags |= StyleFlags::SPAN_ABSOLUTE_CB;
                    }
                    if claims.fixed {
                        flags |= StyleFlags::SPAN_FIXED_CB;
                    }
                }
            }
            current = ancestor.parent;
        }
        StyleFlags::empty()
    }

    /// pyro: the inline box (not atomic) between the out-of-flow box `box_id` and its inline root
    /// `root_id` that is its containing block: the nearest positioned one (for `position:
    /// fixed`, one that contains fixed boxes), with no other box between them that is.
    fn inline_containing_block_of(&self, root_id: NodeId, box_id: NodeId, position: taffy::Position) -> Option<NodeId> {
        let root = &self.nodes[root_id];
        let root_parent = root.is_anonymous().then_some(root.parent).flatten();
        let mut current = self.nodes[box_id].parent;
        while let Some(id) = current {
            if id == root_id || Some(id) == root_parent {
                return None;
            }
            let ancestor = &self.nodes[id];
            if let Some(style) = ancestor.primary_styles() {
                let display = style.clone_display();
                if display.outside() == DisplayOutside::Inline && display.inside() == DisplayInside::Flow {
                    let claims = stylo_taffy::convert::inline_containing_block_claims(&style);
                    if claims.for_position(position) {
                        return Some(id);
                    }
                }
            }
            current = ancestor.parent;
        }
        None
    }

    fn compute_inline_layout_inner(
        &mut self,
        node_id: NodeId,
        inputs: taffy::tree::LayoutInput,
        block_ctx: &mut BlockContext<'_>,
    ) -> taffy::LayoutOutput {
        let scale = self.viewport.scale();
        let LayoutInput {
            known_dimensions,
            parent_size,
            available_space,
            sizing_mode,
            ..
        } = inputs;

        // Take inline layout to satisfy borrow checker
        let mut inline_layout = self.nodes[node_id]
            .data
            .downcast_element_mut()
            .unwrap()
            .take_inline_layout()
            .unwrap();

        let mut style = self.nodes[node_id].layout_style();
        // pyro: as in `compute_inline_layout`, an orthogonal box's percentages.
        style.set_percent_basis(self.orthogonal_percent_basis_of(node_id));

        // Note: both horizontal and vertical percentage padding/borders are resolved against the container's inline size (i.e. width).
        // This is not a bug, but is how CSS is specified (see: https://developer.mozilla.org/en-US/docs/Web/CSS/padding#values)
        let padding = style
            .padding()
            .resolve_or_zero(parent_size.width, resolve_calc_value);
        let border = style
            .border()
            .resolve_or_zero(parent_size.width, resolve_calc_value);
        let container_pb = padding + border;
        let pb_sum = container_pb.sum_axes();
        let box_sizing_adjustment = if style.box_sizing() == BoxSizing::ContentBox {
            pb_sum
        } else {
            Size::ZERO
        };
        // pyro: what the box an aspect ratio is between is smaller by than the border box.
        let ratio_box_inset = if style.box_sizing() == BoxSizing::ContentBox || style.aspect_ratio_of_content_box() {
            pb_sum
        } else {
            Size::ZERO
        };

        // Scrollbar gutters are reserved when the `overflow` property is set to `Overflow::Scroll`.
        // However, the axis are switched (transposed) because a node that scrolls vertically needs
        // *horizontal* space to be reserved for a scrollbar
        // pyro: `CoreStyle::scrollbar_gutter`: which edges hold scrollbar space (classic scrollbars:
        // `overflow: auto` with a scrollbar, `scrollbar-gutter`, the start edge in `direction: rtl`).
        let gutter_edges = style.scrollbar_gutter();
        let scrollbar_gutter =
            taffy::Point { x: gutter_edges.left + gutter_edges.right, y: gutter_edges.top + gutter_edges.bottom };
        let content_box_inset = container_pb + gutter_edges;
        // The lines start after the start-edge gutters (`Node::content_box_origin`, which paint
        // and hit testing read), in a horizontal writing mode.
        let container_pb = if self.nodes[node_id].primary_styles().is_some_and(|s| !s.writing_mode.is_vertical()) {
            taffy::Rect { left: container_pb.left + gutter_edges.left, top: container_pb.top + gutter_edges.top, ..container_pb }
        } else {
            container_pb
        };

        let has_styles_preventing_being_collapsed_through = !style.is_block()
            || style.overflow().x.is_scroll_container()
            || style.overflow().y.is_scroll_container()
            || style.position().is_out_of_flow()
            || padding.top > 0.0
            || padding.bottom > 0.0
            || border.top > 0.0
            || border.bottom > 0.0;
        // || matches!(node_size.height, Some(h) if h > 0.0)
        // || matches!(node_min_size.height, Some(h) if h > 0.0)
        // || !inline_layout.text.is_empty();
        // || inline_layout.layout.inline_boxes().len() > 0;

        // Resolve node's preferred/min/max sizes (width/heights) against the available space (percentages resolve to pixel values)
        // For ContentSize mode, we pretend that the node has no size styles as these should be ignored.
        // pyro: `min-height`/`max-height` content keywords, applied to the final height below
        // (taffy `compute_inner` does the same for block containers).
        let height_keywords = {
            let content = |keyword: taffy::Dimension| keyword.is_sizing_keyword() && !keyword.is_stretch();
            let (min, max) = taffy::BlockContainerStyle::own_height_keywords(&style);
            (sizing_mode == SizingMode::InherentSize && content(min), sizing_mode == SizingMode::InherentSize && content(max))
        };
        // pyro: the box's own height (`height` clamped), for a known height that is no basis for
        // percentages (`LayoutInput::known_dimensions_are_definite`: a table cell's stretched height).
        let mut own_height: Option<f32> = None;
        let (node_size, node_min_size, node_max_size, aspect_ratio) = match sizing_mode {
            SizingMode::ContentSize => {
                let node_size = known_dimensions;
                let node_min_size = Size::NONE;
                let node_max_size = Size::NONE;
                (node_size, node_min_size, node_max_size, None)
            }
            SizingMode::InherentSize => {
                let aspect_ratio = style.aspect_ratio();
                let style_size = style
                    .size()
                    .maybe_resolve(parent_size, resolve_calc_value)
                    .maybe_apply_aspect_ratio(aspect_ratio)
                    .maybe_add(box_sizing_adjustment);
                let style_min_size = style
                    .min_size()
                    .maybe_resolve(parent_size, resolve_calc_value)
                    .maybe_apply_aspect_ratio(aspect_ratio)
                    .maybe_add(box_sizing_adjustment);
                let style_max_size = style
                    .max_size()
                    .maybe_resolve(parent_size, resolve_calc_value)
                    .maybe_add(box_sizing_adjustment);

                // pyro: the quirky html and body fill the viewport (`quirky_fill_height`)
                let mut style_min_size = style_min_size;
                if style_size.height.is_none() && known_dimensions.height.is_none() {
                    if let Some(fill) = self.quirky_fill_height(node_id, parent_size) {
                        style_min_size.height = Some(style_min_size.height.map_or(fill, |min| min.max(fill)));
                    }
                }
                let node_size =
                    known_dimensions.or(style_size.maybe_clamp(style_min_size, style_max_size));
                own_height = style_size.height.maybe_clamp(style_min_size.height, style_max_size.height);
                (node_size, style_min_size, style_max_size, aspect_ratio)
            }
        };

        let suppresses_baseline = style.contain().suppresses_baseline();
        let overflow = style.overflow();
        let is_scroll_container =
            overflow.x.is_scroll_container() || overflow.y.is_scroll_container();
        drop(style);

        // Short circuit if inline context contains no text or inline boxes
        if !has_styles_preventing_being_collapsed_through
            && inline_layout.has_no_text()
            && inline_layout.layout.inline_boxes().len() == 0
        {
            // Put layout back
            self.nodes[node_id]
                .data
                .downcast_element_mut()
                .unwrap()
                .inline_layout_data = Some(inline_layout);
            // pyro: its own (or known) size still applies: a
            // `width: 200px` div holding only an empty `<i>` is 200px wide.
            let size = Size {
                width: node_size.width.unwrap_or(0.0),
                height: node_size.height.unwrap_or(0.0),
            }
            .maybe_max(container_pb.sum_axes().map(Some));
            // pyro: with no line it is empty, and margins collapse through it when it has no
            // height (an empty piece of a split inline between two blocks, a div holding only an
            // empty span), as through an empty block.
            return LayoutOutput {
                margins_can_collapse_through: size.height == 0.0 && node_min_size.height.unwrap_or(0.0) <= 0.0,
                ..LayoutOutput::from_outer_size(size)
            };
        }

        // Compute available space
        let available_space = Size {
            width: known_dimensions
                .width
                .map(AvailableSpace::from)
                .unwrap_or(available_space.width)
                .maybe_set(known_dimensions.width)
                .maybe_set(node_size.width)
                .map_definite_value(|size| {
                    (size.maybe_clamp(node_min_size.width, node_max_size.width)
                        - content_box_inset.horizontal_axis_sum())
                    .max(0.0)
                }),
            height: known_dimensions
                .height
                .map(AvailableSpace::from)
                .unwrap_or(available_space.height)
                .maybe_set(known_dimensions.height)
                .maybe_set(node_size.height)
                .map_definite_value(|size| {
                    (size.maybe_clamp(node_min_size.height, node_max_size.height)
                        - content_box_inset.vertical_axis_sum())
                    .max(0.0)
                }),
        };

        // Compute size of inline boxes
        // pyro: an anonymous block is no containing block: its atomic inlines stretch to the
        // height of the one around it too (`height: stretch` on an iframe; WPT css-sizing
        // `stretch/stretch-anonymous-block-001`).
        // pyro: the quirky html and body's filled height (`quirky_fill_height`), their content box's
        let quirky_fill = if sizing_mode == SizingMode::InherentSize
            && known_dimensions.height.is_none()
            && node_size.height.is_none()
        {
            self.quirky_fill_height(node_id, parent_size)
                .map(|fill| (fill - content_box_inset.vertical_axis_sum()).max(0.0))
        } else {
            None
        };
        let child_available_space = match (self.nodes[node_id].is_anonymous(), parent_size.height) {
            (true, Some(height)) => Size { height: AvailableSpace::Definite(height), ..available_space },
            _ => match quirky_fill {
                Some(height) => Size { height: AvailableSpace::Definite(height), ..available_space },
                None => available_space,
            },
        };
        // pyro: the height `height: stretch` fills (`inline_box_inputs`): this block's content
        // height when it is definite (an anonymous block's containing block's).
        let stretch_height = if self.nodes[node_id].is_anonymous() {
            parent_size.height
        } else if known_dimensions.height.is_some() || node_size.height.is_some() {
            available_space.height.into_option()
        } else if quirky_fill.is_some() {
            quirky_fill
        } else {
            None
        };
        let child_inputs = taffy::tree::LayoutInput {
            known_dimensions: Size::NONE,
            available_space: child_available_space,
            sizing_mode: SizingMode::InherentSize,
            parent_size: Size {
                width: available_space.width.into_option(),
                // Anonymous blocks do not establish the containing block for percentages.
                height: if self.nodes[node_id].is_anonymous() {
                    parent_size.height
                } else if known_dimensions.height.is_none()
                    && node_size.height.is_none()
                    && self.quirks_skips_for_percent_height(&self.nodes[node_id])
                {
                    // pyro: the percentage height quirk (`quirks_skips_for_percent_height`)
                    parent_size.height
                } else if known_dimensions.height.is_some()
                    && !inputs.known_dimensions_are_definite.height
                    && self.nodes[node_id].primary_styles().is_some_and(|style| {
                        style.get_box().display.inside() == style::values::specified::box_::DisplayInside::TableCell
                    })
                {
                    // pyro: a table cell's stretched height that is no basis
                    // (`table::TableTreeWrapper::definite_height`): its own length `height` is.
                    own_height
                        .or(self.cell_percent_height.filter(|(cell, _)| *cell == node_id).map(|(_, height)| height))
                        .map(|height| (height - content_box_inset.vertical_axis_sum()).max(0.0))
                } else if known_dimensions.height.is_none() && node_size.height.is_none() {
                    // pyro: a table cell measured for its row: its own length `height`
                    // (`LayoutPassState::cell_percent_height`).
                    match self.cell_percent_height.filter(|(cell, _)| *cell == node_id) {
                        Some((_, height)) => Some((height - content_box_inset.vertical_axis_sum()).max(0.0)),
                        None => available_space.height.into_option(),
                    }
                } else {
                    available_space.height.into_option()
                },
            },
            // Atomic inlines (e.g. inline-block) establish independent formatting
            // contexts: their margins never collapse with their children's margins.
            vertical_margins_are_collapsible: taffy::Line::FALSE,
            ..inputs
        };
        #[cfg(feature = "floats")]
        let float_child_inputs = taffy::tree::LayoutInput {
            available_space: Size::MAX_CONTENT,
            ..child_inputs
        };

        // Update inline boxes
        let percent_span_edges = inline_layout.percent_span_edges;
        // pyro: an in-flow atomic inline on a central baseline keeps its alphabetic baseline.
        let mut alphabetic_atomics = false;
        for ibox in inline_layout.layout.inline_boxes_mut() {
            // pyro: a span's edge has its width from the build, unless its margin or padding
            // is a percentage: of the containing block's width (CSS 2.1 § 8.3, 8.4), zero for
            // the intrinsic sizes.
            if ibox.kind.is_span_edge() {
                if percent_span_edges
                    && let Some(style) = self.nodes[NodeId::from_u64(ibox.id)].primary_styles()
                {
                    // (The root's own width when it is known: not while it is measured.)
                    let basis = known_dimensions
                        .width
                        .map_or(0.0, |width| (width - content_box_inset.horizontal_axis_sum()).max(0.0));
                    let (start, end, percent) = crate::layout::inline::span_edge_widths_in(&style, basis);
                    if percent {
                        let width = if ibox.kind == parley::InlineBoxKind::SpanStart { start } else { end };
                        ibox.width = width.unwrap_or(0.0) * scale;
                    }
                }
                continue;
            }
            let style = self.child_layout_style(&self.nodes[NodeId::from_u64(ibox.id)]);
            // pyro: an atomic inline's percentage margins are of its containing block, this
            // root's content box (zero while the root is measured, CSS Sizing 3 § 5.2.1), not of
            // the root's own parent (WPT `css/CSS2/normal-flow/intrinsic-size-with-negative-margins`).
            let margin_basis = Size {
                width: Some(
                    known_dimensions
                        .width
                        .map_or(0.0, |width| (width - content_box_inset.horizontal_axis_sum()).max(0.0)),
                ),
                height: inputs.parent_size.height,
            };
            let margin = style.margin().resolve_or_zero(margin_basis, resolve_calc_value);
            // pyro: a button with no baseline (empty, or layout-contained) has one synthesized at
            // its block-end content edge (HTML § 15.5.2 "the button element"; Chromium), not at
            // its bottom margin edge (WPT css-contain `contain-layout-button-001`).
            let button_content_end = style.flags.intersects(StyleFlags::IS_BUTTON | StyleFlags::IS_BUTTON_INPUT).then(|| {
                style.border().resolve_or_zero(margin_basis.width, resolve_calc_value).bottom
                    + style.padding().resolve_or_zero(margin_basis.width, resolve_calc_value).bottom
            });
            // pyro: a checkbox or a radio button has its baseline at its block-end border edge,
            // its margin below it (Chromium; WPT css-writing-modes
            // `forms/{checkbox,radio}-appearance-native-vertical-*-baseline` in sideways lines).
            let toggle_border_end = self.nodes[NodeId::from_u64(ibox.id)]
                .element_data()
                .is_some_and(|element| {
                    element.name.local == markup5ever::local_name!("input")
                        && element
                            .attr(markup5ever::local_name!("type"))
                            .is_some_and(|kind| kind.eq_ignore_ascii_case("checkbox") || kind.eq_ignore_ascii_case("radio"))
                });

            #[cfg(feature = "floats")]
            let is_floated = style.float().is_floated();
            #[cfg(not(feature = "floats"))]
            let is_floated = false;

            let is_out_of_flow = style.position().is_out_of_flow();
            // `baseline-source: auto` selects the last baseline for inline-blocks and the first
            // for other atomic inlines. A block-axis scroll-container inline-block (except buttons)
            // instead uses its bottom margin edge, but only with `auto` (css-align-3 §9.1).
            // `overflow: clip` is not a scroll container. Exported scroll-container baselines
            // are clamped to the border box.
            // A layout-contained box is treated as having no baseline (css-contain-1 §3.3).
            let overflow = style.overflow();
            let box_style = style.style.get_box();
            let is_flow = matches!(
                style.effective_display().inside(),
                DisplayInside::Flow | DisplayInside::FlowRoot
            );
            let baseline_source = box_style.slow_clone_baseline_source();
            let uses_last_baseline = match baseline_source {
                BaselineSource::Auto => is_flow,
                BaselineSource::First => false,
                BaselineSource::Last => true,
            };
            let is_scroll_container = !matches!(overflow.y, Overflow::Visible | Overflow::Clip);
            let is_block_axis_scroll_container = is_flow
                && is_scroll_container
                && !style
                    .flags
                    .intersects(StyleFlags::IS_BUTTON | StyleFlags::IS_BUTTON_INPUT);
            // pyro: a block container's last baseline is its own: with no line box in it (a
            // table gives none) it has none, and sits on its bottom margin edge (CSS 2.1
            // § 10.8.1), not on its first baseline.
            let own_last_baseline = is_flow
                && !style
                    .flags
                    .intersects(StyleFlags::IS_REPLACED | StyleFlags::IS_BUTTON | StyleFlags::IS_BUTTON_INPUT);
            let contain_layout = box_style.slow_clone_contain().contains(Contain::LAYOUT);
            let exports_baseline = !(contain_layout
                || (baseline_source == BaselineSource::Auto && is_block_axis_scroll_container));
            let box_inputs = inline_box_inputs(style.size().width, style.size().height, stretch_height.filter(|_| !style.flags.contains(StyleFlags::IS_REPLACED)), margin, child_inputs);
            drop(style);

            if is_out_of_flow || is_floated {
                ibox.width = 0.0;
                ibox.height = 0.0;
                ibox.baseline = None;
            } else {
                let mut output = self.compute_child_layout(taffy::NodeId::from(ibox.id), box_inputs);
                // Flex size-only layout omits its baseline. Atomic inline flex
                // boxes need the content baseline even during line measurement;
                // synthesizing a bottom-edge baseline adds a spurious descent.
                if exports_baseline
                    && box_inputs.run_mode == RunMode::ComputeSize
                    && output.baselines.first.is_none()
                    && output.baselines.last.is_none()
                    && matches!(self.nodes[NodeId::from_u64(ibox.id)].primary_styles().map(|s| s.clone_display().inside()), Some(DisplayInside::Flex))
                {
                    output = self.compute_child_layout(
                        taffy::NodeId::from(ibox.id),
                        LayoutInput { run_mode: RunMode::PerformLayout, ..box_inputs },
                    );
                }
                ibox.width = (margin.left + margin.right + output.size.width) * scale;
                let alphabetic = if exports_baseline {
                    let baseline = if uses_last_baseline && own_last_baseline {
                        output.baselines.last
                    } else if uses_last_baseline {
                        output.baselines.last.or(output.baselines.first)
                    } else {
                        output.baselines.first.or(output.baselines.last)
                    };
                    baseline.map(|baseline| {
                        let baseline = if is_scroll_container {
                            baseline.clamp(0.0, output.size.height)
                        } else {
                            baseline
                        };
                        (margin.top + baseline) * scale
                    })
                } else {
                    None
                };
                let alphabetic = alphabetic.or_else(|| {
                    // (On alphabetic lines; a central one synthesizes its own, below.)
                    button_content_end.filter(|_| !vertical_line_kind(self.nodes[node_id].writing_mode()).0).map(|end| (margin.top + (output.size.height - end).max(0.0)) * scale)
                });
                let alphabetic = alphabetic.or_else(|| {
                    // (Its line-under border edge: block-start in inverted sideways lines, whose
                    // baselines are turned below, `sideways_inverted_lines`.)
                    let wm = self.nodes[node_id].writing_mode();
                    (toggle_border_end && !vertical_line_kind(wm).0).then(|| match sideways_inverted_lines(wm) {
                        true => margin.top * scale,
                        false => (margin.top + output.size.height) * scale,
                    })
                });
                let (central_line, inverted_line) = vertical_line_kind(self.nodes[node_id].writing_mode());
                // pyro: on a central baseline (Blink's `PhysicalBoxFragment::BaselineMetrics` with
                // `kCentralBaseline`): the box's own central baseline, from its line-over side
                // (block-end in `vertical-lr`, whose lines are inverted), or one synthesized in the
                // middle of its margin box when it has no baseline at all (replaced, orthogonal,
                // scroll-container and empty boxes). A box with only an alphabetic baseline (flex,
                // grid, table, form controls) keeps it, and the lines holding one export no central
                // baseline (`alphabetic_atomics`).
                let dominant = if !central_line || !exports_baseline {
                    None
                } else if uses_last_baseline {
                    output.baselines.dominant_last.or(output.baselines.dominant_first)
                } else {
                    output.baselines.dominant_first.or(output.baselines.dominant_last)
                };
                ibox.central = central_line && (dominant.is_some() || alphabetic.is_none());
                alphabetic_atomics |= central_line && !ibox.central;
                ibox.baseline = if ibox.central {
                    let over = match dominant {
                        Some(baseline) if inverted_line => margin.bottom + output.size.height - baseline,
                        Some(baseline) => margin.top + baseline,
                        None => (margin.top + margin.bottom + output.size.height) / 2.0,
                    };
                    Some(over * scale)
                } else if sideways_inverted_lines(self.nodes[node_id].writing_mode()) {
                    // From the line-over side, block-end (`sideways_inverted_lines`).
                    let margin_box = margin.top + margin.bottom + output.size.height;
                    alphabetic.map(|alphabetic| margin_box * scale - alphabetic)
                } else {
                    alphabetic
                };
                // Vertical margins adjust the space the box reserves in the line. A box with a
                // baseline splits that space into ascent (`margin.top + baseline`) and descent
                // (`margin.bottom + height - baseline`), either of which may be negative. A box
                // without a baseline sits on the baseline and cannot reserve negative space.
                // Kept finite: huge author lengths can sum to infinity, which is taller than
                // the line breaker's `f32::MAX` height limit, and it then yields
                // `MaxHeightExceeded` for this box forever without advancing.
                let margin_box_height = margin.top + margin.bottom + output.size.height;
                ibox.height = if ibox.baseline.is_some() {
                    (margin_box_height * scale).min(f32::MAX)
                } else {
                    (margin_box_height.max(0.0) * scale).min(f32::MAX)
                };
            }
        }

        let text_indent = self.nodes[node_id]
            .primary_styles()
            .map(|s| s.slow_clone_text_indent())
            // pyro: not on the lines of an anonymous block after its block's first line, unless
            // each line after a forced break is indented (`each-line`: the line after a block-level
            // box too, WPT css-text `text-indent-each-line-after-block-level-box`).
            .filter(|indent| {
                indent.hanging || indent.each_line || crate::layout::first_line::starts_a_block(self.doc, node_id)
            })
            .unwrap_or_else(GenericTextIndent::zero);
        // pyro: the first line of an anonymous block is the first formatted line of its parent
        // element only when the block is the parent's first box (CSS Text 3 § 8.1: "the first
        // line of an anonymous block box is only affected if it is the first child of its
        // parent element"): the text after a block child is not indented again.
        let text_indent = {
            let node = &self.nodes[node_id];
            let later_anonymous_block = node.is_anonymous()
                && !text_indent.each_line
                && node.parent.is_some_and(|parent| {
                    !crate::layout::first_line::is_flex_or_grid_container(&self.nodes[parent])
                        && self.nodes[parent]
                        .layout_children
                        .borrow()
                        .as_ref()
                        .is_some_and(|children| children.first().is_some_and(|&first| first != node_id))
                });
            if later_anonymous_block { GenericTextIndent::zero() } else { text_indent }
        };
        // pyro: the element's first formatted line is in this layout (`hanging-punctuation: first`).
        let has_first_line = crate::layout::first_line::starts_a_block(self.doc, node_id) && {
            let node = &self.nodes[node_id];
            !(node.is_anonymous()
                && node.parent.is_some_and(|parent| {
                    self.nodes[parent]
                        .layout_children
                        .borrow()
                        .as_ref()
                        .is_some_and(|children| children.first().is_some_and(|&first| first != node_id))
                }))
        };
        // pyro: an initial letter box (`layout::first_letter`) starts the first line, and holds
        // the line's text indent in its width (Blink's `CalculateInitialLetterBoxInlineSize`): the
        // text after it is not indented again.
        let initial_letter = inline_layout
            .first_letter
            .as_ref()
            .and_then(|split| Some((split.style_node, split.initial?)))
            .filter(|&(box_id, _)| {
                self.nodes.get(box_id).is_some_and(|node| node.flags.contains(NodeFlags::IS_INITIAL_LETTER_BOX))
            });
        let letter_indent = match initial_letter {
            Some(_) if !text_indent.hanging => text_indent.length.clone(),
            _ => GenericTextIndent::zero().length,
        };
        let text_indent = match initial_letter {
            Some(_) if !text_indent.hanging && !text_indent.each_line => GenericTextIndent::zero(),
            _ => text_indent,
        };
        let indent_options = IndentOptions {
            each_line: text_indent.each_line,
            hanging: text_indent.hanging,
        };
        // pyro: CSS Text 3 `hanging-punctuation: first`: opening punctuation that starts the
        // element's first line hangs outside its start edge, as a negative indent of its advance
        // (also in the intrinsic widths), unless a box edge with width comes first.
        let hang_first = self.nodes[node_id]
            .primary_styles()
            .filter(|s| {
                s.get_inherited_text().hanging_punctuation.contains(style::values::computed::HangingPunctuation::FIRST)
            })
            .filter(|_| has_first_line && initial_letter.is_none() && !text_indent.each_line && !text_indent.hanging)
            .and_then(|_| inline_layout.layout.first_cluster_hang(&inline_layout.text, |ch| parley::hangs_at_edge(ch, true)))
            .unwrap_or(0.0);
        // Percentage indents do not contribute to intrinsic widths.
        let intrinsic_indent = text_indent.length.resolve(CSSPixelLength::new(0.0)).px() * scale - hang_first;
        inline_layout.layout.set_text_indent(intrinsic_indent, indent_options);

        // pyro: and the inline-axis scrollbar gutters (`content_box_inset`).
        let pbw = content_box_inset.horizontal_components().sum() * scale;
        let width = known_dimensions
            .width
            .map(|w| (w * scale) - pbw)
            .unwrap_or_else(|| {
                // pyro: the content widths of a layout without in-flow inline boxes are
                // kept with its shaped text (`TextLayout::content_widths`); with inline boxes, whose
                // sizes depend on whether this measures under a min-content or max-content
                // constraint, they are measured here.
                let content_sizes = inline_layout.content_widths(intrinsic_indent);
                let min_content_width = content_sizes.min;
                let max_content_width = content_sizes.max;

                #[cfg(feature = "floats")]
                let float_width = match available_space.width {
                    AvailableSpace::MinContent => {
                        let mut width: f32 = 0.0;
                        for ibox in inline_layout.layout.inline_boxes_mut() {
                            if ibox.kind.is_span_edge() {
                                continue;
                            }
                            let (is_floated, margin) = {
                                let style =
                                    self.child_layout_style(&self.nodes[NodeId::from_u64(ibox.id)]);
                                (
                                    style.float().is_floated(),
                                    style
                                        .margin()
                                        .resolve_or_zero(inputs.parent_size, resolve_calc_value),
                                )
                            };

                            if is_floated {
                                let output = self.compute_child_layout(
                                    taffy::NodeId::from(ibox.id),
                                    subtract_margins(child_inputs, margin),
                                );
                                width = width.max(output.size.width + margin.left + margin.right);
                            }
                        }

                        width * scale
                    }
                    // A definite available width only occurs here when shrink-to-fit sizing,
                    // which needs the max-content contribution of the floats.
                    AvailableSpace::MaxContent | AvailableSpace::Definite(_) => {
                        // When computing a max-content size the available width is effectively
                        // infinite, so floats never wrap onto a new "band" due to a lack of
                        // horizontal space. They only move below preceding floats when the `clear`
                        // property forces them to.
                        //
                        // Floats that share a band sit side-by-side and so their widths sum, whereas
                        // floats pushed onto a new band (via `clear`) stack vertically and so we
                        // take the maximum extent across bands rather than summing.
                        let mut left_band: f32 = 0.0;
                        let mut right_band: f32 = 0.0;
                        let mut width: f32 = 0.0;
                        for ibox in inline_layout.layout.inline_boxes_mut() {
                            if ibox.kind.is_span_edge() {
                                continue;
                            }
                            let (float, clear, margin) = {
                                let style =
                                    self.child_layout_style(&self.nodes[NodeId::from_u64(ibox.id)]);
                                (
                                    style.float(),
                                    style.clear(),
                                    style
                                        .margin()
                                        .resolve_or_zero(inputs.parent_size, resolve_calc_value),
                                )
                            };

                            if float.is_floated() {
                                if matches!(clear, Clear::Left | Clear::Both) {
                                    left_band = 0.0;
                                }
                                if matches!(clear, Clear::Right | Clear::Both) {
                                    right_band = 0.0;
                                }

                                let output = self.compute_child_layout(
                                    taffy::NodeId::from(ibox.id),
                                    subtract_margins(child_inputs, margin),
                                );
                                let box_width = output.size.width + margin.left + margin.right;

                                match float {
                                    Float::Left => left_band += box_width,
                                    Float::Right => right_band += box_width,
                                    Float::None => {}
                                }
                                width = width.max(left_band + right_band);
                            }
                        }

                        width * scale
                    }
                };

                #[cfg(not(feature = "floats"))]
                let float_width = 0.0;

                let computed_width = match available_space.width {
                    AvailableSpace::MinContent => min_content_width.max(float_width),
                    AvailableSpace::MaxContent => max_content_width + float_width,
                    AvailableSpace::Definite(limit) => (limit * scale)
                        .min(max_content_width + float_width)
                        .max(min_content_width),
                };
                // Up to Chromium's layout unit (1/64 px), not a whole pixel:
                // a shrink-to-fit box is as wide as its text.
                let computed_width = ceil_layout_unit(computed_width, scale);

                let style_width = node_size.width.map(|w| w * scale);
                let min_width = node_min_size.width.map(|w| w * scale);
                let max_width = node_max_size.width.map(|w| w * scale);

                (style_width)
                    .unwrap_or(computed_width + pbw)
                    .max(computed_width)
                    .maybe_clamp(min_width, max_width)
                    - pbw
            });

        #[cfg(not(feature = "floats"))]
        let _ = block_ctx; // Suppress unused variable warning

        // Set block context width if this is a block context root
        #[cfg(feature = "floats")]
        let is_bfc_root = block_ctx.is_bfc_root();
        #[cfg(feature = "floats")]
        if is_bfc_root {
            block_ctx.set_width((width + pbw) / scale);
        }

        // Create sub-context to account for the inline layout's padding/border
        #[cfg(feature = "floats")]
        let outer_block_ctx = block_ctx;
        #[cfg(feature = "floats")]
        let mut block_ctx = outer_block_ctx.sub_context(
            container_pb.top,
            FloatInsets::new(container_pb.left, container_pb.right),
        );
        // block_ctx.apply_content_box_inset([container_pb.left, container_pb.right]);

        if inputs.run_mode == taffy::RunMode::ComputeSize
            && inputs.axis == RequestedAxis::Horizontal
        {
            // Put layout back
            self.nodes[node_id]
                .data
                .downcast_element_mut()
                .unwrap()
                .inline_layout_data = Some(inline_layout);

            let measured_size = inputs.known_dimensions.unwrap_or(taffy::Size {
                width: ceil_layout_unit(width, scale) / scale,
                // Height is ignored if RequestedAxis if Horizontal
                height: 0.0,
            });

            let clamped_size = inputs
                .known_dimensions
                .or(node_size)
                .unwrap_or(measured_size + content_box_inset.sum_axes())
                .maybe_clamp(node_min_size, node_max_size)
                .maybe_max(container_pb.sum_axes().map(Some));

            return LayoutOutput::from_outer_size(clamped_size);
        }

        let resolved_text_indent = text_indent
            .length
            .resolve(CSSPixelLength::new((width / scale).max(0.0)))
            .px();
        inline_layout
            .layout
            .set_text_indent((resolved_text_indent * scale - hang_first), indent_options);

        // pyro: `::first-line` styles that change layout (`TextLayout::first_line_layout`): the
        // build gave them to the text up to where the first line ended last time (all of it at
        // first); a first line that ends elsewhere at this width builds the text again with that end.
        if inline_layout.first_line_layout {
            for _ in 0..2 {
                inline_layout.layout.break_all_lines(Some(width));
                let end = inline_layout.layout.lines().next().map(|line| line.text_range().end);
                if end.is_none() || end == inline_layout.first_line_end {
                    break;
                }
                inline_layout.first_line_end = end;
                let crate::BaseDocument { nodes, layout_ctx, font_ctx, viewport, .. } = &mut **self;
                let mut font_ctx = font_ctx.lock().unwrap();
                super::build_inline_layout_into(nodes, layout_ctx, &mut font_ctx, &mut inline_layout, viewport.scale(), node_id);
                drop(font_ctx);
                inline_layout.last_break = None;
                inline_layout.final_lines = None;
                inline_layout
                    .layout
                    .set_text_indent((resolved_text_indent * scale - hang_first), indent_options);
            }
        }

        // Content sized to fit exactly (e.g. shrink-to-fit) can end up a few ulps wider than
        // `width`: the width goes through round trips in CSS px, and the line breaker sums
        // advances in a different order to `calculate_content_widths`. Give the line breaker a
        // few ulps of slack in that case so that the content doesn't wrap.
        let max_advance = {
            let max_content_width = inline_layout.content_widths((resolved_text_indent * scale - hang_first)).max;
            let fit_width = width.max(max_content_width);
            let rounding_error = (fit_width + pbw).abs() * 8.0 * f32::EPSILON;
            // pyro: and the layout unit (1/64 px) each atomic inline's width was rounded up to
            // (`ceil_layout_unit`): text that fits beside them as Chromium measures it fits here
            // (WPT css-pseudo `highlight-painting-004-ref*`: `qui` and an inline-block `ck` in a box
            // as wide as `quick`).
            let atomics = inline_layout
                .layout
                .inline_boxes()
                .filter(|ibox| ibox.kind == parley::InlineBoxKind::InFlow && ibox.width > 0.0)
                .count();
            let rounding_error = rounding_error + atomics as f32 * scale / 64.0;
            if (max_content_width - width).abs() <= rounding_error {
                fit_width + rounding_error
            } else {
                width
            }
        };
        #[cfg(feature = "floats")]
        let slack = max_advance - width;
        let mirror_slots = self.nodes[node_id].writing_mode().contains(stylo_taffy::WritingMode::WRITING_MODE_SIDEWAYS_LR);

        #[cfg(not(feature = "floats"))]
        {
            inline_layout.layout.break_all_lines(Some(max_advance));
            inline_layout.hit_reach = None;
        }

        // Out-of-flow candidates bubbled up from this container and its in-flow subtree.
        // These are laid out by the out-of-flow positioning pass (`compute_oof_layout`).
        let mut oof_candidates = OofCandidates::new();

        // The bottoms of the last two line boxes (indexed by line index % 2), including any
        // space skipped to move them down past floats. Only the last counted line is read, which
        // is at most one line before the last.
        #[cfg(feature = "floats")]
        let mut line_bottoms = [0.0f64; 2];
        #[cfg(feature = "floats")]
        let mut line_count_so_far = 0usize;
        // pyro: the lowest point a `<br>` with `clear` moved the content to (layout px).
        let mut br_clear_floor = 0.0f32;

        // Perform inline layout
        #[cfg(feature = "floats")]
        {
            // pyro: breaking again with the inputs of the last break (no floats
            // involved) gives the same lines.
            let break_key = {
                use std::hash::{Hash, Hasher};
                let mut h = std::hash::DefaultHasher::new();
                max_advance.to_bits().hash(&mut h);
                resolved_text_indent.to_bits().hash(&mut h);
                for ibox in inline_layout.layout.inline_boxes() {
                    (ibox.id, ibox.width.to_bits(), ibox.height.to_bits(), ibox.baseline.map(f32::to_bits)).hash(&mut h);
                }
                // pyro: and `text-fit`.
                if let Some(fit) = self.nodes[node_id].primary_styles().and_then(|s| stylo_to_parley::text_fit(&s)) {
                    (fit.kind as u8, fit.target as u8, fit.limit.map(f32::to_bits)).hash(&mut h);
                }
                h.finish()
            };
            let floats_involved = block_ctx.find_content_slot(0.0, 0.0, Clear::None, None).segment_id.is_some()
                || inline_layout
                    .layout
                    .inline_boxes()
                    .any(|ibox| ibox.kind == parley::InlineBoxKind::CustomOutOfFlow);
            if floats_involved || inline_layout.last_break != Some(break_key) {
                // pyro: a measuring run breaks the lines of the last final layout again: note where
                // they are first, to tell whether it leaves them elsewhere (see the end of this run).
                if inputs.run_mode != RunMode::PerformLayout
                    && !self.nodes[node_id].flags.contains(NodeFlags::FINAL_LAYOUT_STALE)
                    && inline_layout.final_lines.is_some_and(|laid_out| {
                        laid_out.geometry.is_none()
                            && laid_out.last_break.is_some()
                            && laid_out.last_break == inline_layout.last_break
                    })
                {
                    let geometry = inline_layout.line_geometry();
                    if let Some(laid_out) = inline_layout.final_lines.as_mut() {
                        laid_out.geometry = Some(geometry);
                    }
                }
                let wraps = self.nodes[node_id].primary_styles().is_none_or(|s| {
                    s.clone_text_wrap_mode() == stylo_to_parley::stylo::TextWrapMode::Wrap
                });
                inline_layout.hit_reach = None;
                // pyro: the relative offset of the inline boxes a float is in (`place_inline_float`).
                let relative_spans = inline_layout.relative_spans;
                let br_clears = std::mem::take(&mut inline_layout.br_clears);
                let root_id = node_id;
                let span_cb = taffy::Size { width: width / scale, height: 0.0 };
                // (The content height of the container, the floats' containing block, when definite.)
                let float_cb_height = known_dimensions.height.map(|height| (height - container_pb.vertical_axis_sum()).max(0.0));
                let float_cb_rtl = self.nodes[node_id].layout_style().direction() == Direction::Rtl;
                let float_span_offset = |nodes: &crate::NodeTree, float_id: NodeId| match nodes[float_id].parent {
                    Some(parent) if relative_spans => {
                        crate::layout::inline::inline_span_flow_offset(nodes, parent, root_id, span_cb)
                    }
                    _ => taffy::Point::ZERO,
                };
                // pyro: `text-wrap-style: balance` (CSS Text 4 § 6.2): the lines wrap at the least
                // advance that keeps their count, found by bisection as in Blink, for up to its
                // six lines. Beside floats (whose lines differ in width) every line's room is
                // narrowed by the most that keeps the count, as in Blink (see `FloatBalance`).
                let balance_wanted = wraps
                    && self.nodes[node_id].primary_styles().is_some_and(|s| {
                        s.slow_clone_text_wrap_style() == style::values::computed::TextWrapStyle::Balance
                            // A block-size clamp (`max-height` with `line-clamp`) is not applied
                            // yet: the lines it would cut stay as they are.
                            && matches!(s.slow_clone_max_height(), style::values::computed::MaxSize::None)
                    });
                // Lines that `line-clamp` would cut off are left as they are.
                // pyro: and so are those an ancestor's line clamp cuts: only the lines it leaves
                // this block balance (WPT css-overflow `line-clamp/line-clamp-balance-002`).
                let ancestor_left = self.clamp_lines_left(node_id);
                let balance_clamp = || {
                    self.nodes[node_id]
                        .primary_styles()
                        .and_then(|s| s.slow_clone_line_clamp().max_lines.lines_value().map(|lines| lines.0))
                        .filter(|&lines| lines > 0)
                        .map_or(usize::MAX, |lines| lines as usize)
                        .min(ancestor_left.unwrap_or(usize::MAX))
                };
                let balance_limit = (balance_wanted && !floats_involved)
                    .then(|| balanced_wrap_limit(&mut inline_layout.layout, max_advance, scale, balance_clamp()))
                    .flatten();
                // pyro: CSS `text-fit` (CSS Text 5), not beside floats or with an initial letter,
                // as in Blink. `consistent` scales every line by what the widest (narrowest) line
                // needs, measured on the lines broken without it.
                let text_fit = self.nodes[node_id]
                    .primary_styles()
                    .and_then(|s| stylo_to_parley::text_fit(&s))
                    .filter(|_| !floats_involved && initial_letter.is_none());
                inline_layout.layout.set_text_fit(text_fit.unwrap_or_default(), None);
                if let Some(fit) = text_fit.filter(|fit| fit.target == parley::TextFitTarget::Consistent) {
                    inline_layout.layout.break_all_lines(Some(max_advance));
                    let scale = inline_layout.layout.text_fit_block_scale();
                    inline_layout.layout.set_text_fit(fit, scale);
                }
                let mut float_balance = (balance_wanted
                    && floats_involved
                    && initial_letter.is_none()
                    && text_fit.is_none()
                    && max_advance.is_finite())
                .then(|| FloatBalance {
                    snapshot: block_ctx.float_snapshot(),
                    placed_floats: self.inline_placed_floats.len(),
                    br_clear_floor,
                    max_lines: FloatBalance::MAX_LINES.min(balance_clamp()),
                    first: None,
                    low: 0.0,
                    high: max_advance,
                    reduction: 0.0,
                });
                let kept = loop {
                    let mut breaker = inline_layout.layout.break_lines();
                    let mut line_slot = block_ctx.find_content_slot(0.0, 0.0, Clear::None, None);
                    let mut has_active_floats = line_slot.segment_id.is_some();
                    let mut pending_floats: Vec<PendingFloat> = Vec::new();
                    // pyro: the item of the last float placed in this pass. A line that wraps before
                    // a float it placed meets it again on the next line: it keeps its place. (The
                    // floats before it are all placed: a float met after one left for below its line
                    // is left for below it too.)
                    let mut placed_through: Option<usize> = None;
                    // pyro: the initial letter placed on the first line, until that line ends: its box
                    // and the margin box `place_initial_letter` excluded, which move down with the
                    // line's text when the line's annotations need more room than the letter's raise
                    // left (see `LineMetrics::annotation_shift`).
                    let mut letter_on_line: Option<(NodeId, Point<f32>, Size<f32>, taffy::FloatDirection)> = None;
                    let state = breaker.state_mut();
                    state.set_layout_max_advance(max_advance);
                    if let Some(limit) = balance_limit {
                        state.set_wrap_limit(limit);
                    }
                    if let Some(balance) = &float_balance {
                        state.set_wrap_reduction(balance.reduction);
                    }
                    // pyro: a block that starts with an initial letter starts below the initial
                    // letters before it (CSS Inline 3 § 7.6, Blink's `InitialLetterClearanceOffset`).
                    if initial_letter.is_some()
                        && let Some(clear) = block_ctx.initial_letter_clearance().filter(|&clear| clear > 0.0)
                    {
                        state.set_line_y(f64::from(clear * scale));
                        line_slot = block_ctx.find_content_slot(clear, 0.0, Clear::None, None);
                        has_active_floats = line_slot.segment_id.is_some();
                    }
                    set_line_slot(state, &line_slot, scale, slack, mirror_slots);
                    // Without floats every line after the first gets the same geometry, so breaking
                    // can continue after the lines kept from the last build, when the text changed
                    // after them (parley checks the rest: same width, indent and line geometry, LTR,
                    // no inline boxes). Justified text is broken from the start, conservatively;
                    // `hyphens` and `::first-line` are not supported, so there is nothing to check.
                    let is_justified = self.nodes[node_id].primary_styles().is_some_and(|s| {
                        stylo_to_parley::text_align(s.clone_text_align()) == parley::Alignment::Justify
                            || stylo_to_parley::text_align_last(s.clone_text_align_last())
                                == Some(parley::Alignment::Justify)
                    });
                    // Lines after the first get x 0 and `max_advance` (see `YieldData::LineBreak`
                    // below).
                    let kept = if !floats_involved && !is_justified && balance_limit.is_none() {
                        breaker.resume(0.0, max_advance)
                    } else {
                        0
                    };
                    if kept > 0 {
                        // The kept lines were counted by the break they come from; the last of them
                        // ends where the breaker resumes (no floats: nothing was skipped).
                        line_bottoms[(kept - 1) % 2] = breaker.state().line_y();
                        line_count_so_far = kept;
                    }

                    // The state at the start of the current line, which is reverted to when the line
                    // needs to be retried lower down (or narrower) to avoid floats.
                    // `None` if the line can't be retried (no floats beside it, or a float was placed
                    // part way through it).
                    let mut line_start_state = has_active_floats.then(|| breaker.state().clone());
                    // pyro: whether the current line was retried, and whether in a wider slot
                    // (`wider_line_slot`).
                    let (mut line_narrowed, mut line_widened) = (false, false);

                    while let Some(yield_data) = breaker.break_next() {
                        match yield_data {
                            YieldData::LineBreak(line_break_data) => {
                                if let Some(saved_state) = line_start_state.take()
                                    && let Some(retry_slot) =
                                        retry_line_slot(&block_ctx, &line_slot, &line_break_data, wraps, scale).or_else(|| {
                                            let wider = (line_narrowed && !line_widened)
                                                .then(|| wider_line_slot(&block_ctx, &line_slot, &line_break_data, scale))
                                                .flatten();
                                            line_widened |= wider.is_some();
                                            wider
                                        })
                                {
                                    line_narrowed = true;
                                    // Floats queued during this line attempt will be encountered again
                                    pending_floats.clear();
                                    line_slot = retry_slot;
                                    has_active_floats = line_slot.segment_id.is_some();
                                    breaker.revert_to(saved_state);
                                    set_line_slot(breaker.state_mut(), &line_slot, scale, slack, mirror_slots);
                                    line_start_state = has_active_floats.then(|| breaker.state().clone());
                                    continue;
                                }
                                line_bottoms[line_count_so_far % 2] = line_break_data.line_y_end;
                                line_count_so_far += 1;
                                line_narrowed = false;
                                line_widened = false;

                                // pyro: the initial letter's cap height stays on its first line's text
                                // (Blink places it after the line, `PostPlaceInitialLetterBox`): what
                                // the line's annotations push the text down by beyond the room the
                                // letter's raise left moves the letter, and its exclusion, too (WPT
                                // css-inline `initial-letter-block-position-*-over-ruby*`).
                                if let Some((letter_id, at, size, direction)) = letter_on_line.take() {
                                    let metrics = &line_break_data.metrics;
                                    let extra = (metrics.annotation_overflow[0] - metrics.annotation_shift) / scale;
                                    if extra > 0.0 {
                                        self.nodes[letter_id].update_unrounded_layout(|layout| layout.location.y += extra);
                                        block_ctx.place_initial_letter(
                                            at,
                                            Size { width: size.width, height: size.height + extra },
                                            direction,
                                        );
                                    }
                                }

                                let state = breaker.state_mut();

                                // pyro: a `<br>` with `clear` ended the line: what follows starts
                                // below the floats it clears.
                                if line_break_data.reason == BreakReason::Explicit
                                    && let Some(&(_, br)) = br_clears
                                        .iter()
                                        .find(|&&(offset, _)| offset + 1 == line_break_data.text_end)
                                {
                                    let clear = self.child_layout_style(&self.nodes[br]).clear();
                                    let y = state.line_y() as f32 / scale;
                                    let cleared = block_ctx.find_content_slot(y, 0.0, clear, None).y;
                                    // The end of the floats beside the line: a trimmed last line
                                    // (`text-box-trim`) does not end the content above it.
                                    let line_top = line_break_data.line_y_start as f32 / scale;
                                    let beside = block_ctx.find_content_slot(line_top, 0.0, clear, None).y;
                                    if beside > line_top {
                                        br_clear_floor = br_clear_floor.max(beside * scale);
                                    }
                                    if cleared > y {
                                        let cleared = f64::from(cleared * scale);
                                        state.set_line_y(cleared);
                                        line_bottoms[(line_count_so_far - 1) % 2] = cleared;
                                        br_clear_floor = br_clear_floor.max(cleared as f32);
                                        has_active_floats = true;
                                    }
                                }

                                // pyro: a float met past the point where the line wrapped is met
                                // again on the next line, and placed after that one (CSS 2.1 §9.5.1
                                // rule 6; WPT `floats-line-wrap-shifted-001`).
                                let next_item = state.item_index();
                                pending_floats.retain(|float| float.item < next_item);
                                if !pending_floats.is_empty() {
                                    let min_y = state.line_y() as f32 / scale;
                                    for float in pending_floats.drain(..) {
                                        placed_through = placed_through.max(Some(float.item));
                                        let span_offset = float_span_offset(&self.nodes, float.node_id);
                                        self.place_inline_float(
                                            &mut block_ctx,
                                            float,
                                            min_y,
                                            container_pb,
                                            &mut oof_candidates,
                                            span_offset,
                                            width / scale,
                                            float_cb_height,
                                            float_cb_rtl,
                                        );
                                    }
                                    has_active_floats = true;
                                }

                                if has_active_floats {
                                    let min_y = state.line_y() / scale as f64;
                                    line_slot =
                                        block_ctx.find_content_slot(min_y as f32, 0.0, Clear::None, None);
                                    has_active_floats = line_slot.segment_id.is_some();
                                    set_line_slot(state, &line_slot, scale, slack, mirror_slots);
                                } else {
                                    state.set_line_x(0.0);
                                    state.set_line_max_advance(max_advance);
                                }

                                line_start_state = has_active_floats.then(|| breaker.state().clone());
                                continue;
                            }
                            YieldData::MaxHeightExceeded(_data) => {
                                // TODO
                                continue;
                            }
                            YieldData::InlineBoxBreak(box_break_data) => {
                                let item = breaker.state().item_index();
                                let state = breaker.state_mut();
                                let node_id = NodeId::from_u64(box_break_data.inline_box_id);

                                // pyro: met again after its line wrapped before it: it keeps its place.
                                if placed_through.is_some_and(|placed| item <= placed) {
                                    state.append_inline_box_to_line(
                                        box_break_data.advance,
                                        f32::NEG_INFINITY,
                                        f32::NEG_INFINITY,
                                    );
                                    continue;
                                }

                                // pyro: the initial letter box (`layout::first_letter`) is placed by
                                // the first line, which it starts: at the line's start (after the
                                // text indent, beside the floats along its whole height), its cap
                                // height on the line's, its baseline on the sink's line (Blink's
                                // `PostPlaceInitialLetterBox`), an exclusion from the line's top to
                                // its margin box's bottom. Raised and sunken letters move the
                                // line's text down.
                                if let Some((_, letter)) = initial_letter.filter(|&(box_id, _)| box_id == node_id) {
                                    let line_top = state.line_y() as f32 / scale;
                                    let (margin, rtl) = {
                                        let style = self.child_layout_style(&self.nodes[node_id]);
                                        (
                                            style.margin().resolve_or_zero(inputs.parent_size, resolve_calc_value),
                                            style.float() == Float::Right,
                                        )
                                    };
                                    let output = self.compute_child_layout(crate::taffy_node_id(node_id), float_child_inputs);
                                    let size = output.size;
                                    let ascent = output.baselines.first.unwrap_or(size.height);
                                    let (offset, mut shift) = initial_letter_offset(&letter, ascent, size.height);
                                    let border_top = offset + margin.top;
                                    // `text-box-trim` at the start of the first line cancels the text's
                                    // move (Blink's `ApplyTextBoxTrim`): the box stays where it is drawn
                                    // over the moved text, its exclusion where it would be.
                                    let mut drawn_top = border_top;
                                    if shift > 0.0 && crate::layout::text_box::block_trim(self.doc, root_id).0.is_some() {
                                        drawn_top -= shift;
                                        shift = 0.0;
                                    }
                                    let extent_top = offset.min(0.0);
                                    let extent_bottom = border_top + size.height + margin.bottom;
                                    let slot = block_ctx.find_content_slot(
                                        line_top,
                                        extent_bottom.max(letter.line_height),
                                        Clear::None,
                                        None,
                                    );
                                    let indent = letter_indent.resolve(CSSPixelLength::new((width / scale).max(0.0))).px();
                                    let margin_box_width = (margin.left + size.width + margin.right).max(0.0);
                                    let (x, border_x) = if rtl {
                                        let right = slot.x + slot.width - indent;
                                        (right - margin_box_width, right - margin.right - size.width)
                                    } else {
                                        let left = slot.x + indent;
                                        (left, left + margin.left)
                                    };
                                    let excluded = (
                                        node_id,
                                        Point { x, y: line_top + extent_top },
                                        Size { width: margin_box_width, height: (extent_bottom - extent_top).max(0.0) },
                                        if rtl { taffy::FloatDirection::Right } else { taffy::FloatDirection::Left },
                                    );
                                    block_ctx.place_initial_letter(excluded.1, excluded.2, excluded.3);
                                    letter_on_line = Some(excluded);
                                    let location = Point {
                                        x: border_x + container_pb.left,
                                        y: line_top + drawn_top + container_pb.top,
                                    };
                                    self.nodes[node_id].update_unrounded_layout(|layout| {
                                        layout.size = size;
                                        layout.location = location;
                                        layout.margin = margin;
                                    });
                                    placed_through = placed_through.max(Some(item));
                                    let line_y = line_top + shift;
                                    state.set_line_y(f64::from(line_y * scale));
                                    // An annotation over the line's text takes the room the raise
                                    // leaves before the text moves further (Blink's
                                    // `BlockStartAnnotationSpace`).
                                    state.set_annotation_space(shift * scale);
                                    line_slot = block_ctx.find_content_slot(line_y, 0.0, Clear::None, None);
                                    has_active_floats = line_slot.segment_id.is_some();
                                    set_line_slot(state, &line_slot, scale, slack, mirror_slots);
                                    state.append_inline_box_to_line(
                                        box_break_data.advance,
                                        f32::NEG_INFINITY,
                                        f32::NEG_INFINITY,
                                    );
                                    line_start_state = has_active_floats.then(|| breaker.state().clone());
                                    continue;
                                }

                                let (direction, clear, margin) = {
                                    let style = self.child_layout_style(&self.nodes[node_id]);
                                    // We can assume that the box is a float because we only set `break_on_box: true` for floats
                                    let direction = match style.float() {
                                        Float::Left => taffy::FloatDirection::Left,
                                        Float::Right => taffy::FloatDirection::Right,
                                        Float::None => unreachable!(),
                                    };
                                    (
                                        direction,
                                        style.clear(),
                                        style
                                            .margin()
                                            .resolve_or_zero(inputs.parent_size, resolve_calc_value),
                                    )
                                };

                                let output = self.compute_child_layout(
                                    crate::taffy_node_id(node_id),
                                    float_child_inputs,
                                );
                                let float_width = (output.size.width + margin.left + margin.right) * scale;
                                let float = PendingFloat {
                                    node_id,
                                    item,
                                    output,
                                    margin,
                                    direction,
                                    clear,
                                };

                                // A float that doesn't fit beside the content already on the line is
                                // placed below the line (CSS 2.1 §9.5.1), as are any floats after it
                                // (a float may not be higher than an earlier float).
                                let fits_on_line = pending_floats.is_empty()
                                    && (box_break_data.advance <= 0.0
                                        || box_break_data.advance + float_width
                                            <= state.line_max_advance() + 0.001 * scale);
                                // pyro: nothing precedes the float on its line.
                                let leads_line = box_break_data.advance <= 0.0;
                                if !fits_on_line {
                                    pending_floats.push(float);
                                } else {
                                    let min_y = state.line_y() as f32 / scale;
                                    placed_through = placed_through.max(Some(item));
                                    let span_offset = float_span_offset(&self.nodes, node_id);
                                    self.place_inline_float(
                                        &mut block_ctx,
                                        float,
                                        min_y,
                                        container_pb,
                                        &mut oof_candidates,
                                        span_offset,
                                        width / scale,
                                        float_cb_height,
                                        float_cb_rtl,
                                    );

                                    let min_y = state.line_y() / scale as f64;
                                    line_slot =
                                        block_ctx.find_content_slot(min_y as f32, 0.0, Clear::None, None);
                                    has_active_floats = line_slot.segment_id.is_some();
                                    set_line_slot(state, &line_slot, scale, slack, mirror_slots);

                                    // The float can't be unplaced, so the line can no longer be retried
                                    line_start_state = None;
                                }

                                // Floats are out-of-flow and must not contribute to the line's height.
                                state.append_inline_box_to_line(
                                    box_break_data.advance,
                                    f32::NEG_INFINITY,
                                    f32::NEG_INFINITY,
                                );

                                // pyro: a float placed with nothing before it on the line leaves
                                // the line to be retried from here, after the float: content that does
                                // not fit beside it moves down, the float stays (CSS 2.1 §9.5.1; WPT
                                // `floats-placement-004`, `c414-flt-fit-001`).
                                if fits_on_line && leads_line && has_active_floats {
                                    line_start_state = Some(breaker.state().clone());
                                }

                                // if float.is_floated() {
                                //     println!("INLINE FLOATED BOX ({}) {:?}", ibox.id, float);
                                //     println!(
                                //         "w:{} h:{} x:{}, y:{}",
                                //         layout.size.width, layout.size.height, 0, 0
                                //     );
                                // }
                            }
                        }
                    }
                    breaker.finish();
                    // pyro: balancing beside floats: the next width to try, from a fresh start.
                    let Some(balance) = float_balance.as_mut() else { break kept };
                    let lines = &inline_layout.layout;
                    let emergency =
                        lines.lines().filter(|line| line.break_reason() == BreakReason::Emergency).count();
                    let bottom = line_count_so_far.checked_sub(1).map_or(0.0, |last| line_bottoms[last % 2]);
                    let Some(reduction) = balance.next((lines.len(), emergency, bottom), scale) else { break kept };
                    balance.reduction = reduction;
                    block_ctx.restore_floats(&balance.snapshot);
                    self.inline_placed_floats.truncate(balance.placed_floats);
                    br_clear_floor = balance.br_clear_floor;
                    oof_candidates = OofCandidates::new();
                    line_bottoms = [0.0; 2];
                    line_count_so_far = 0;
                };
                inline_layout.br_clears = br_clears;
                inline_layout.last_break = (!floats_involved).then_some(break_key);
                let resumed_at = match kept {
                    0 => 0,
                    kept => inline_layout.layout.get(kept - 1).map_or(0, |line| line.text_range().end),
                };
                crate::phase_profile::broke(kept, inline_layout.text.len().saturating_sub(resumed_at));
            }
        }

        // Propagate the height consumed by floats placed within this container to the
        // enclosing block context (so that the BFC root can contain them)
        #[cfg(feature = "floats")]
        let float_height_contribution = block_ctx.floated_content_height_contribution();
        #[cfg(feature = "floats")]
        outer_block_ctx.add_child_floated_content_height_contribution(
            container_pb.top + float_height_contribution,
        );

        let (alignment, last_line_alignment) = self.nodes[node_id]
            .primary_styles()
            .map(|s| {
                (
                    stylo_to_parley::text_align(s.clone_text_align()),
                    // pyro: `text-align: justify-all` justifies the last line too (unless
                    // `text-align-last` says otherwise).
                    stylo_to_parley::text_align_last(s.clone_text_align_last()).or(
                        (s.clone_text_align() == style::values::specified::TextAlignKeyword::JustifyAll)
                            .then_some(parley::Alignment::Justify),
                    ),
                )
            })
            .unwrap_or((parley::layout::Alignment::Start, None));

        // pyro: aligning moves each line with its runs (the glyph
        // reach is measured per line, `TextLayout::hit_reach`), except that
        // justification spaces the runs out within it: justifying now or
        // before measures again.
        let justified = alignment == parley::layout::Alignment::Justify
            || last_line_alignment == Some(parley::layout::Alignment::Justify);
        if justified || inline_layout.justified {
            inline_layout.hit_reach = None;
        }
        inline_layout.justified = justified;
        // pyro: CSS Text 4 `text-group-align`: the lines align within the group of them first.
        let group_left = self.nodes[node_id].primary_styles().and_then(|s| {
            use style::values::computed::TextGroupAlign as G;
            let rtl = inline_layout.layout.is_rtl();
            match s.get_text().text_group_align {
                G::None => None,
                G::Left => Some(0.0),
                G::Right => Some(1.0),
                G::Center => Some(0.5),
                G::Start => Some(if rtl { 1.0 } else { 0.0 }),
                G::End => Some(if rtl { 0.0 } else { 1.0 }),
            }
        });
        let options = AlignmentOptions {
            align_when_overflowing: false,
            last_line_alignment,
        };
        match group_left {
            Some(left) => inline_layout.layout.align_in_group(alignment, options, left),
            None => inline_layout.layout.align(alignment, options),
        }

        // Parley lays out empty text as a single strut-height line (text-editor semantics),
        // but a line box containing no text, inline boxes or other in-flow content is a
        // zero-height line box in CSS (CSS2 §9.4.2).
        // pyro: out-of-flow boxes are no content: a line of empty inline boxes (their struts) and
        // absolutely positioned boxes is zero-height.
        let has_inline_content = !inline_layout.has_no_text()
            || inline_layout.layout.inline_boxes().any(|inline_box| {
                !matches!(inline_box.kind, parley::InlineBoxKind::OutOfFlow | parley::InlineBoxKind::CustomOutOfFlow)
            });

        // pyro: a line with an inline box's inline-axis margin, border or padding (a span edge,
        // also of zero width) is no phantom line box (CSS Inline 3 § 2.1): margins do not
        // collapse through its block, whatever its height.
        let has_span_edges = inline_layout.layout.inline_boxes().any(|inline_box| inline_box.kind.is_span_edge());
        let has_visible_text = inline_layout.text.chars().any(|c| {
            // (a no-break space is content: WPT css-color `t32-opacity-offscreen-multiple-boxes-2-c`)
            !matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{C}') && !matches!(c, '\u{200B}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{2061}' | '\u{FFFC}')
        });

        let mut height = if has_inline_content {
            inline_layout.layout.height()
        } else {
            0.0
        };

        // A forced line break (e.g. `<br>` or a preserved newline) at the end of the inline
        // content ends the final line box without starting a new one. Parley still emits an
        // empty line after it (so that editors have a line to place the cursor on), so that
        // line is excluded from the measured height (and from the last baseline).
        let line_count = inline_layout.layout.len();
        let mut trailing_empty_line = None;
        if line_count >= 2
            && let (Some(prev_line), Some(last_line)) = (
                inline_layout.layout.get(line_count - 2),
                inline_layout.layout.get(line_count - 1),
            )
            && prev_line.break_reason() == BreakReason::Explicit
            && last_line.text_range().is_empty()
            && last_line.items().next().is_none()
        {
            height -= last_line.metrics().line_height;
            trailing_empty_line = Some(line_count - 1);
        }

        // Parley's height is the sum of the line heights, which misses the space skipped when a
        // line is moved down past floats, so extend it to the bottom of the last counted line
        // (Parley doesn't count an empty last line either).
        #[cfg(feature = "floats")]
        {
            let counted_lines = match inline_layout.layout.get(line_count.wrapping_sub(1)) {
                Some(last_line) if last_line.is_empty() => line_count - 1,
                _ => trailing_empty_line.unwrap_or(line_count),
            };
            if has_inline_content
                && let Some(bottom) = counted_lines
                    .checked_sub(1)
                    .filter(|&last| last < line_count_so_far && last + 2 >= line_count_so_far)
                    .map(|last| line_bottoms[last % 2])
            {
                height = height.max(bottom as f32);
            }
        }

        // pyro: `text-box-trim` of the block whose first or last formatted line is here: the
        // lines move up by what the first line loses over its edge, and the content is shorter
        // by that and by what the last line loses under its edge (`layout::text_box`).
        let mut text_box_shift = 0.0;
        let mut text_box_overflows = false;
        let mut text_box_float_shift = None;
        if has_inline_content {
            let (start, end) = crate::layout::text_box::block_trim(self.doc, node_id);
            let layout = &inline_layout.layout;
            if let Some(trim) = start
                && let Some(line) = layout.get(0)
            {
                let top = line.metrics().block_min_coord;
                let edge = line.span_text_edge(line.root_style(), trim.edge, trim.over);
                // An inverted line (`vertical-lr`) has its under side at the block start.
                text_box_shift = if trim.over { edge - top } else { line.metrics().block_max_coord - edge };
            }
            // pyro: the last formatted line: not a phantom line holding only floats or empty
            // boxes (CSS Inline 3 § 4.1), which the end trim passes over.
            let last_formatted = {
                let mut last = trailing_empty_line.unwrap_or(line_count);
                while last > 1 && layout.get(last - 1).is_some_and(|line| line.metrics().invisible) {
                    last -= 1;
                }
                last.checked_sub(1)
            };
            let mut end_trim = 0.0;
            if let Some(trim) = end
                && let Some(line) = last_formatted.and_then(|i| layout.get(i))
            {
                let edge = line.span_text_edge(line.root_style(), trim.edge, trim.over);
                end_trim = if trim.over { edge - line.metrics().block_min_coord } else { line.metrics().block_max_coord - edge };
            }
            // (Not above where a `<br>`'s clearance put the end of the content.)
            height = (height - text_box_shift - end_trim).max((br_clear_floor - text_box_shift).min(height));
            text_box_overflows = text_box_shift > 0.0 || end_trim > 0.0;
            // pyro: the floats placed among the lines move with them; one placed after the last
            // line moves up by its trimmed end too, as that line box itself is shorter (Blink).
            let last_bottom = last_formatted
                .and_then(|i| layout.get(i))
                .map_or(f32::INFINITY, |line| line.metrics().block_max_coord / scale);
            if text_box_shift > 0.0 || end_trim > 0.0 {
                text_box_float_shift = Some((text_box_shift / scale, end_trim / scale, last_bottom));
            }
        }

        #[cfg(feature = "floats")]
        {
            // pyro: but for a `line-clamp` container: its height ends with its lines, floats or
            // not (WPT css-overflow `line-clamp-with-floats-016`).
            let line_clamp = self.nodes[node_id].primary_styles().is_some_and(|style| {
                let clamp = &style.get_box().line_clamp;
                !clamp.webkit_legacy && !clamp.max_lines.is_none()
            });
            // pyro: floats moved with the trimmed lines (`text_box_float_shift`) contribute where
            // they are now.
            let mut float_height_contribution = float_height_contribution;
            if let Some((start, end, last_bottom)) = text_box_float_shift {
                let mut bottom = f32::NEG_INFINITY;
                for (float_id, top) in core::mem::take(&mut self.inline_placed_floats) {
                    let shift = if top >= last_bottom { start + end } else { start };
                    let mut margin_bottom = 0.0;
                    self.nodes[float_id].update_unrounded_layout(|layout| {
                        layout.location.y -= shift;
                        margin_bottom = layout.location.y + layout.size.height + layout.margin.bottom;
                    });
                    bottom = bottom.max(margin_bottom - container_pb.top);
                }
                if bottom.is_finite() {
                    float_height_contribution = bottom.max(0.0);
                }
            }
            if is_bfc_root && !line_clamp {
                height = height.max(float_height_contribution * scale)
            };
        }

        // Note: `width` and `height` are content-box measurements of the inline content.
        // `known_dimensions` must not be substituted in here: those are border-box sizes, and
        // using them for the scrollable overflow rect (which adds padding below) would
        // double-count padding, incorrectly making the container's content overflow it.
        let measured_size = taffy::Size {
            width: width / scale,
            height: height / scale,
        };

        // Unbreakable content (e.g. `white-space: pre` text) may overflow the max advance the
        // lines were broken at. `width` is the advance the lines were broken at, so the actual
        // extent of the line boxes must be taken from the laid-out lines for `content_size`.
        let content_width = f32_max(width, inline_layout.layout.width()) / scale;

        let clamped_size = inputs
            .known_dimensions
            .or(node_size)
            .unwrap_or(measured_size + content_box_inset.sum_axes())
            .maybe_clamp(node_min_size, node_max_size);
        // pyro: content keywords in `min-height`/`max-height` (WPT css-sizing
        // `hori-block-size-small-or-larger-than-container-with-min-or-max-content-1`).
        let clamped_size = {
            let content_height = measured_size.height + content_box_inset.vertical_axis_sum();
            let mut height = clamped_size.height;
            if height_keywords.1 {
                height = height.min(content_height);
            }
            if height_keywords.0 {
                height = height.max(content_height);
            }
            Size { height, ..clamped_size }
        };
        let final_size = Size {
            width: clamped_size.width,
            // pyro: through the box the ratio is between (the content box under `box-sizing:
            // content-box` or `aspect-ratio: auto <ratio>`), clamped by `min-height`/`max-height`
            // (WPT css-sizing `aspect-ratio/block-aspect-ratio-027`).
            height: f32_max(
                clamped_size.height,
                aspect_ratio
                    .map(|ratio| {
                        let inset = ratio_box_inset;
                        let height = (clamped_size.width - inset.width).max(0.0) / ratio + inset.height;
                        height.maybe_clamp(node_min_size.height, node_max_size.height)
                    })
                    .unwrap_or(0.0),
            ),
        }
        .maybe_max(container_pb.sum_axes().map(Some));

        let container_direction = self.nodes[node_id].layout_style().direction();

        // `align-content` aligns the line boxes (as a single unit) within the content box in
        // the block axis. Floats are positioned relative to the formatting context and are not
        // moved. The offset is stored on the text layout for painting and hit-testing.
        let align_content = BlockContainerStyle::align_content(&self.nodes[node_id].layout_style());
        let block_offset = if align_content.keyword() == taffy::AlignContentKeyword::Normal {
            0.0
        } else {
            let free_space =
                final_size.height - content_box_inset.vertical_axis_sum() - measured_size.height;
            taffy::compute_block_align_content_offset(align_content, free_space)
        };
        let block_offset = block_offset - text_box_shift / scale;
        inline_layout.block_offset = block_offset;
        let line_box_top = container_pb.top + block_offset;
        // pyro: for a line clamp (`layout::line_clamp`), which a later layout of the box that
        // clamps sets again.
        inline_layout.line_box_top = line_box_top;
        // pyro: invisible line boxes at the end (phantom lines: CSS Inline 3 § 4.1) count for no
        // line clamp (WPT css-overflow `line-clamp-037`).
        inline_layout.counted_lines = match has_inline_content {
            true => {
                let mut counted = trailing_empty_line.unwrap_or(line_count);
                while counted > 1 && inline_layout.layout.get(counted - 1).is_some_and(|line| line.metrics().invisible) {
                    counted -= 1;
                }
                counted as u32
            }
            false => 0,
        };
        inline_layout.clamp = None;

        // Store sizes and positions of inline boxes. Walking a line's items builds every
        // glyph run on it, so lines are only walked when there are boxes to find.
        let mut ibox_order: u32 = 0;
        let mut span_cb_flags = StyleFlags::empty();
        let has_inline_boxes = inline_layout.layout.inline_boxes().len() > 0;
        let lines = has_inline_boxes.then(|| inline_layout.layout.lines());
        for line in lines.into_iter().flatten() {
            let mut line_has_in_flow_content = false;
            for item in line.items() {
                if let parley::layout::PositionedLayoutItem::GlyphRun(run) = &item {
                    line_has_in_flow_content |= run.advance() > 0.0;
                }
                if let parley::layout::PositionedLayoutItem::InlineBox(ibox) = item {
                    // pyro: a span's edge is part of the span, which has no box of its own. It is
                    // content of its line (CSS 2.1 § 9.4.2: a line with a margin, border or
                    // padding is no empty line): a block-level box after it would start below.
                    if ibox.kind.is_span_edge() {
                        line_has_in_flow_content = true;
                        continue;
                    }
                    let order = ibox_order;
                    ibox_order += 1;
                    if inputs.run_mode == RunMode::PerformLayout {
                        span_cb_flags |=
                            self.inline_span_cb_flags(node_id, NodeId::from_u64(ibox.id));
                    }
                    let node = &self.nodes[NodeId::from_u64(ibox.id)];
                    let style = self.child_layout_style(node);
                    let padding = style
                        .padding()
                        .resolve_or_zero(child_inputs.parent_size, resolve_calc_value);
                    let border = style
                        .border()
                        .resolve_or_zero(child_inputs.parent_size, resolve_calc_value);
                    let margin = style
                        .margin()
                        .resolve_or_zero(child_inputs.parent_size, resolve_calc_value);

                    #[cfg(feature = "floats")]
                    let is_floated = style.float() != Float::None;
                    #[cfg(not(feature = "floats"))]
                    let is_floated = false;

                    let position = style.position();
                    let is_absolute = position.is_out_of_flow();
                    let item_direction = style.direction();
                    // Inline formatting contexts have no `justify-items`/`align-items` for an
                    // `auto` self-alignment to defer to, so it behaves as `normal`.
                    let justify_self =
                        OofItemStyle::justify_self(&style).unwrap_or(taffy::AlignItems::NORMAL);
                    let align_self =
                        OofItemStyle::align_self(&style).unwrap_or(taffy::AlignItems::NORMAL);

                    // The static position of an absolutely positioned box depends on the
                    // display its hypothetical box would have had (the display specified
                    // before position:absolute blockified it): inline-level boxes sit at
                    // their position within the line, while block-level boxes start at the
                    // content-box left edge of their containing block.
                    let is_inline_level =
                        style.style.get_box().original_display.outside() == DisplayOutside::Inline;

                    // Resolve relative inset offsets against the containing block
                    // (the content box of the inline container).
                    let container_content_size = final_size - content_box_inset.sum_axes();
                    let inset_style = style.inset();
                    let inset = taffy::Rect {
                        left: inset_style
                            .left
                            .maybe_resolve(container_content_size.width, resolve_calc_value),
                        right: inset_style
                            .right
                            .maybe_resolve(container_content_size.width, resolve_calc_value),
                        top: inset_style
                            .top
                            .maybe_resolve(container_content_size.height, resolve_calc_value),
                        bottom: inset_style
                            .bottom
                            .maybe_resolve(container_content_size.height, resolve_calc_value),
                    };
                    let box_inputs = inline_box_inputs(style.size().width, style.size().height, stretch_height.filter(|_| !style.flags.contains(StyleFlags::IS_REPLACED)), margin, child_inputs);
                    drop(style);

                    if is_absolute {
                        // The static-position rectangle
                        // (https://www.w3.org/TR/css-position-3/#staticpos-rect):
                        // - An inline-level box's rectangle is zero-width at its position
                        //   within the line (`ibox.y` is the baseline as out-of-flow boxes are
                        //   zero-sized) and spans the line box in the block axis.
                        // - A block-level box's rectangle spans the containing block's content
                        //   box in the inline axis and is zero-height at the top of the line
                        //   box, or below it if in-flow content precedes it on the line (its
                        //   hypothetical box would start a new line after that content).
                        let line_metrics = line.metrics();
                        // pyro: relatively positioned inline boxes around it move its static
                        // position, as they move what else they hold.
                        // (Not in a vertical writing mode, where the lines are laid out turned
                        // and the offset is not applied to them.)
                        let span_offset = match self.nodes[NodeId::from_u64(ibox.id)].parent {
                            Some(parent) if inline_layout.relative_spans => {
                                crate::layout::inline::inline_span_flow_offset(&self.nodes, parent, node_id, container_content_size)
                            }
                            _ => taffy::Point::ZERO,
                        };
                        let line_top = (line_metrics.block_min_coord / scale) + line_box_top + span_offset.y;
                        let line_bottom = (line_metrics.block_max_coord / scale) + line_box_top + span_offset.y;
                        let (inline_area, block_area) = if is_inline_level {
                            let x = (ibox.x / scale) + container_pb.left + span_offset.x;
                            (
                                taffy::Line { start: x, end: x },
                                taffy::Line {
                                    start: line_top,
                                    end: line_bottom,
                                },
                            )
                        } else {
                            // pyro: in-flow content logically before it on the line (bidi
                            // reordering can put later content visually before it).
                            let preceded = if inline_layout.layout.is_rtl() || line_is_reordered(&line) {
                                let at = inline_layout
                                    .layout
                                    .inline_boxes()
                                    .find(|candidate| candidate.id == ibox.id)
                                    .map_or(0, |candidate| candidate.index);
                                first_in_flow_index(&line, &inline_layout.layout).is_some_and(|first| first < at)
                            } else {
                                line_has_in_flow_content
                            };
                            // pyro: the static-position rectangle is the line box's block
                            // extent, on the next line when content precedes the box, with no
                            // inline extent at the line's start (Blink; WPT css-align
                            // abspos/align-self-static-position-003/004/007).
                            let line_height = line_bottom - line_top;
                            let y = if preceded { line_bottom } else { line_top };
                            let x = if container_direction.is_rtl() {
                                final_size.width - container_pb.right + span_offset.x
                            } else {
                                container_pb.left + span_offset.x
                            };
                            (
                                taffy::Line { start: x, end: x },
                                taffy::Line { start: y, end: y + line_height },
                            )
                        };

                        // pyro: an inline-level box is put on the start side of its place in
                        // the direction of the text there (the bidi level it resolved to), not
                        // in the container's: Latin text in a right-to-left paragraph runs left
                        // to right, and so does the box after it (Blink's `InlineItem::Direction()`;
                        // WPT `static-position/htb-rtl-*`). A block-level box takes the container's.
                        // On a line with no in-flow content the box resolves to no text's level:
                        // it takes the container's direction (WPT css-text
                        // `text-indent-with-absolute-pos-child`, right to left).
                        let static_direction = if is_inline_level
                            && ibox.is_rtl != inline_layout.layout.is_rtl()
                            && first_in_flow_index(&line, &inline_layout.layout).is_some()
                        {
                            match container_direction {
                                Direction::Ltr => Direction::Rtl,
                                Direction::Rtl => Direction::Ltr,
                            }
                        } else {
                            container_direction
                        };
                        oof_candidates.push(OofCandidate { containing_block: None,
                            node: taffy::NodeId::from(ibox.id),
                            order,
                            position,
                            static_position: taffy::Point {
                                x: AxisStaticPosition::from_alignment(
                                    justify_self.resolve_self_relative(
                                        item_direction,
                                        static_direction,
                                        true,
                                    ),
                                    inline_area,
                                    static_direction.is_rtl(),
                                ),
                                y: AxisStaticPosition::from_alignment(
                                    align_self.resolve_self_relative(
                                        item_direction,
                                        container_direction,
                                        false,
                                    ),
                                    block_area,
                                    false,
                                ),
                            },
                        });
                    } else if is_floated {
                        self.nodes[NodeId::from_u64(ibox.id)].update_unrounded_layout(|layout| {
                            layout.padding = padding; //.map(|p| p / scale);
                            layout.border = border; //.map(|p| p / scale);
                            layout.margin = margin;
                        });
                    } else {
                        line_has_in_flow_content = true;
                        // Re-measure the box to get its border-box size (this hits the layout
                        // cache). The size cannot be recovered from `ibox` dimensions as the
                        // space reserved in the line is clamped to be non-negative.
                        let mut output =
                            self.compute_child_layout(taffy::NodeId::from(ibox.id), box_inputs);
                        let size = output.size;
                        // pyro: relatively positioned inline boxes around it move it too.
                        let span_offset = match self.nodes[NodeId::from_u64(ibox.id)].parent {
                            Some(parent) if inline_layout.relative_spans => {
                                crate::layout::inline::inline_span_flow_offset(&self.nodes, parent, node_id, container_content_size)
                            }
                            _ => taffy::Point::ZERO,
                        };
                        let wm = self.nodes[node_id].writing_mode();
                        let node = &mut self.nodes[NodeId::from_u64(ibox.id)];

                        let is_relative = position == taffy::Position::Relative;
                        let inset_offset = if is_relative {
                            taffy::Point {
                                x: if container_direction == Direction::Rtl {
                                    inset.right.map(|x| -x).or(inset.left).unwrap_or(0.0)
                                } else {
                                    inset.left.or(inset.right.map(|x| -x)).unwrap_or(0.0)
                                },
                                y: inset.top.or(inset.bottom.map(|x| -x)).unwrap_or(0.0),
                            }
                        } else {
                            taffy::Point::ZERO
                        };

                        let overflow = output.scrollable_overflow_rect;
                        // pyro: the lines of `vertical-lr` are inverted (line-over at block-end):
                        // the box goes to the other side of its line's slot. The lines of
                        // `sideways-lr` run from the bottom: it goes to the other end of its line.
                        let mut ibox_y = ibox.y;
                        let mut ibox_x = ibox.x;
                        // (A box that keeps its alphabetic baseline keeps its place too.)
                        let central_box = || {
                            inline_layout
                                .layout
                                .inline_boxes()
                                .find(|candidate| candidate.id == ibox.id && candidate.kind == parley::InlineBoxKind::InFlow)
                                .is_some_and(|candidate| candidate.central)
                        };
                        if (vertical_line_kind(wm).1 && central_box()) || sideways_inverted_lines(wm) {
                            let metrics = line.metrics();
                            ibox_y = metrics.block_min_coord + metrics.block_max_coord - ibox.y - ibox.height;
                        } else if wm.contains(stylo_taffy::WritingMode::WRITING_MODE_SIDEWAYS_LR) {
                            ibox_x = container_content_size.width * scale - ibox.x - ibox.width;
                        }
                        let location = taffy::Point {
                            x: (ibox_x / scale) + margin.left + container_pb.left + inset_offset.x + span_offset.x,
                            // Parley positions the margin box; offset to the border box even
                            // when a negative top margin makes it extend above the margin box.
                            y: (ibox_y / scale) + margin.top + line_box_top + inset_offset.y + span_offset.y,
                        };
                        node.update_unrounded_layout(|layout| {
                            layout.size = size;
                            layout.scrollable_overflow_rect = overflow;
                            layout.location = location;
                            layout.padding = padding; //.map(|p| p / scale);
                            layout.border = border; //.map(|p| p / scale);
                            layout.margin = margin;
                        });

                        // Translate anchors from item-relative to container-relative
                        // coordinates and collect candidates bubbled from the box's subtree
                        if !output.oof_candidates.is_empty() {
                            output.oof_candidates.translate(location);
                            oof_candidates.append(&mut output.oof_candidates);
                        }
                    }
                }
            }
        }

        // println!("INLINE LAYOUT FOR {:?}. max_advance: {:?}", node_id, max_advance);
        // dbg!(&inline_layout.text);
        // println!("Computed: w: {} h: {}", inline_layout.layout.width(), inline_layout.layout.height());
        // println!("known_dimensions: w: {:?} h: {:?}", inputs.known_dimensions.width, inputs.known_dimensions.height);
        // println!("\n");

        // pyro: an out-of-flow box whose containing block is an inline box (a positioned span)
        // is laid out against the padding box the span's fragments bound (CSS 2.1 § 10.1 4.1),
        // moved by the span's relative offset.
        if inputs.run_mode == RunMode::PerformLayout && !oof_candidates.is_empty() {
            let content_size = final_size - content_box_inset.sum_axes();
            for candidate in oof_candidates.as_mut_slice() {
                if candidate.containing_block.is_some() {
                    continue;
                }
                let Some(span) = self.inline_containing_block_of(node_id, crate::dom_node_id(candidate.node), candidate.position)
                else {
                    continue;
                };
                // pyro: a split inline's box spans its head, its block-level content and its tail:
                // its parent lays the box out (`split_inline_containing_block`).
                if self.nodes[span].is_split_inline() && self.split_inline_contains_absolute(span) {
                    continue;
                }
                let Some((x, y, width, height)) = span_padding_box(&self.nodes, &inline_layout, span, scale) else {
                    continue;
                };
                let offset = if inline_layout.relative_spans {
                    crate::layout::inline::inline_span_flow_offset(&self.nodes, span, node_id, content_size)
                } else {
                    taffy::Point::ZERO
                };
                candidate.containing_block = Some((
                    Size { width, height },
                    Point { x: x + container_pb.left + offset.x, y: y + line_box_top + offset.y },
                ));
            }
        }

        let sideways_inverted = sideways_inverted_lines(self.nodes[node_id].writing_mode());
        let line_baseline = |line: parley::Line<'_, _>| {
            let m = line.metrics();
            let baseline = if sideways_inverted { m.block_min_coord + m.block_max_coord - m.baseline } else { m.baseline };
            (baseline / scale) + line_box_top
        };
        let exports_baseline = has_inline_content && !suppresses_baseline;
        let first_baseline = exports_baseline
            .then(|| inline_layout.layout.lines().next().map(line_baseline))
            .flatten();
        // pyro: the last line box that is not a phantom one (CSS Inline 3 § 4.1: one holding only
        // floats or collapsed white space after a `<br>` has no baseline; WPT css-flexbox
        // `flexbox-baseline-multi-line-vert-001` whose reference is such an inline-block).
        let last_line_index = trailing_empty_line.unwrap_or(line_count).checked_sub(1).map(|mut last| {
            while last > 0 && inline_layout.layout.get(last).is_some_and(|line| line.metrics().invisible) {
                last -= 1;
            }
            last
        });
        let last_baseline = exports_baseline
            .then(|| last_line_index.and_then(|i| inline_layout.layout.get(i)))
            .flatten()
            .map(line_baseline);
        // pyro: in a vertical writing mode with a central dominant baseline (text not sideways; Blink's
        // `ComputedStyle::GetFontBaseline`), the baselines from the block-start edge, for grid baseline
        // alignment. The lines of `vertical-lr` are painted turned within their slot (line-over at block-end).
        // (Sideways text keeps `first_baseline`/`last_baseline`, turned in `vertical-lr`: `sideways_inverted_lines`.)
        let (dominant_first, dominant_last) = {
            let wm = self.nodes[node_id].writing_mode();
            if wm.is_vertical() && !wm.is_sideways() && exports_baseline && !alphabetic_atomics {
                let dominant = |line: parley::Line<'_, _>| {
                    let m = line.metrics();
                    let central = (m.content_block_min_coord + m.content_block_max_coord) / 2.0;
                    let central = if wm.intersects(stylo_taffy::WritingMode::LINE_INVERTED) {
                        m.block_min_coord + m.block_max_coord - central
                    } else {
                        central
                    };
                    central / scale + line_box_top
                };
                (
                    inline_layout.layout.lines().next().map(dominant),
                    last_line_index.and_then(|i| inline_layout.layout.get(i)).map(dominant),
                )
            } else {
                (None, None)
            }
        };

        let (block_offset_bits, line_box_top_bits) = (inline_layout.block_offset.to_bits(), line_box_top.to_bits());
        if inputs.run_mode == RunMode::PerformLayout {
            inline_layout.final_lines = Some(crate::node::LaidOutLines {
                last_break: inline_layout.last_break,
                block_offset: block_offset_bits,
                line_box_top: line_box_top_bits,
                inline_box_sizes: inline_layout.inline_box_sizes(),
                geometry: None,
            });
        } else if let Some(laid_out) = inline_layout.final_lines
            && !self.nodes[node_id].flags.contains(NodeFlags::FINAL_LAYOUT_STALE)
        {
            let stale = match (laid_out.last_break, inline_layout.last_break) {
                (Some(final_break), Some(last_break)) => {
                    laid_out.block_offset != block_offset_bits
                        || laid_out.line_box_top != line_box_top_bits
                        || (final_break != last_break
                            && (laid_out.inline_box_sizes != inline_layout.inline_box_sizes()
                                || laid_out.geometry != Some(inline_layout.line_geometry())))
                }
                _ => true,
            };
            if stale {
                self.mark_final_layout_stale(node_id);
            }
        }

        if inputs.run_mode == RunMode::PerformLayout {
            inline_layout.span_cb_flags = span_cb_flags;
            // pyro: lines taller than the box reach into what follows it (`FLOW_CONTENT_OVERFLOWS`).
            // So do lines trimmed by `text-box-trim` and inline boxes with a border or padding
            // over or under them.
            let overflows = measured_size.height > final_size.height - content_box_inset.vertical_axis_sum() + 0.01
                || text_box_overflows
                || inline_layout.span_block_edges;
            let flags = &mut self.nodes[node_id].flags;
            flags.remove(NodeFlags::FLOW_BOXES_OVERLAP);
            flags.set(NodeFlags::FLOW_CONTENT_OVERFLOWS, overflows);
        }

        // Put layout back
        self.nodes[node_id]
            .data
            .downcast_element_mut()
            .unwrap()
            .inline_layout_data = Some(inline_layout);

        let oof_position_inset = border + gutter_edges;

        LayoutOutput {
            size: final_size,
            scrollable_overflow_rect: {
                let content_extent = taffy::Size {
                    width: content_width,
                    height: measured_size.height,
                } + padding.sum_axes();
                let mut rect = taffy::Rect {
                    left: 0.0,
                    right: content_extent.width,
                    top: 0.0,
                    bottom: content_extent.height + block_offset.max(0.0),
                };
                let client_width =
                    final_size.width - border.left - border.right - scrollbar_gutter.x;
                let is_rtl = container_direction.is_rtl();
                if is_rtl {
                    rect.left = (client_width - rect.right).min(0.0);
                    rect.right = client_width.max(0.0);
                }
                if is_scroll_container {
                    rect.left = if is_rtl { rect.left } else { 0.0 };
                    rect.top = 0.0;
                }
                taffy::ScrollableOverflow::new(
                    rect,
                    taffy::Point {
                        x: is_scroll_container && is_rtl,
                        y: false,
                    },
                )
            },
            baselines: taffy::Baselines {
                dominant_first,
                dominant_last,
                ..taffy::Baselines::new(first_baseline, last_baseline)
            },
            top_margin: CollapsibleMarginSet::ZERO,
            bottom_margin: CollapsibleMarginSet::ZERO,
            // pyro: nor through lines that hold text, however short (`font-size: 0`, `line-height:
            // 0`): they are line boxes (CSS 2.1 §8.3.1; WPT css-fonts `font-size-zero-1`)
            margins_can_collapse_through: !has_styles_preventing_being_collapsed_through
                && !has_span_edges
                && final_size.height == 0.0
                && measured_size.height == 0.0
                && !has_visible_text,
            oof_candidates,
            oof_positioning_area: Some(OofPositioningArea {
                size: final_size - oof_position_inset.sum_axes(),
                offset: Point {
                    x: oof_position_inset.left,
                    y: oof_position_inset.top,
                },
            }),
        }
    }
}

#[inline(always)]
fn f32_max(a: f32, b: f32) -> f32 {
    a.max(b)
}

/// pyro: whether bidi reordering may have changed the order of `line`'s items (it has right-to-left
/// runs or boxes, or the layout is right-to-left).
fn line_is_reordered(line: &parley::Line<'_, crate::node::TextBrush>) -> bool {
    line.items().any(|item| match item {
        parley::layout::PositionedLayoutItem::GlyphRun(run) => run.run().is_rtl(),
        _ => false,
    })
}

/// pyro: the text index of the logically first in-flow content on `line` (a glyph run with an
/// advance, or an atomic inline box).
fn first_in_flow_index(line: &parley::Line<'_, crate::node::TextBrush>, layout: &parley::Layout<crate::node::TextBrush>) -> Option<usize> {
    line.items()
        .filter_map(|item| match item {
            parley::layout::PositionedLayoutItem::GlyphRun(run) if run.advance() > 0.0 => {
                Some(run.run().text_range().start)
            }
            parley::layout::PositionedLayoutItem::InlineBox(ibox)
                if ibox.kind == parley::InlineBoxKind::InFlow || ibox.kind.is_span_edge() =>
            {
                layout
                    .inline_boxes()
                    .find(|candidate| candidate.id == ibox.id && candidate.kind == ibox.kind)
                    .map(|candidate| candidate.index)
            }
            _ => None,
        })
        .min()
}

/// pyro: the padding box of the inline box (not atomic) `span` in the lines of `text_layout`, as
/// the containing block of the out-of-flow boxes in it (CSS 2.1 § 10.1 4.1): from the top and
/// left padding edges of its first fragment to the bottom and right padding edges of its last
/// (right and left where it is right-to-left), `(x, y, width, height)` in CSS px from the lines'
/// origin. A fragment is what the span holds on a line: text, atomic boxes, its own edges
/// without their margin and border, and the places of out-of-flow boxes; its block extent is the
/// span's content area and block-axis padding, or nothing on a line without content (a span
/// holding only the out-of-flow box is at the line's top). `None` when it is on no line.
fn span_padding_box(
    nodes: &crate::NodeTree,
    text_layout: &crate::node::TextLayout,
    span: NodeId,
    scale: f32,
) -> Option<(f32, f32, f32, f32)> {
    use parley::InlineBoxKind;
    use parley::layout::PositionedLayoutItem;
    use style::values::computed::{Length, LengthPercentage, Margin};
    use style::values::generics::length::GenericMargin;

    let layout = &text_layout.layout;
    let styles = layout.styles();
    // The style the span made, when the content of style `index` is in the span.
    let span_style = |mut index: u16| -> Option<u16> {
        let mut found = None;
        loop {
            let style = styles.get(usize::from(index))?;
            if style.brush.id == span {
                found = Some(index);
            } else if found.is_some() {
                break;
            }
            if index == 0 {
                break;
            }
            index = layout.style_parent(index);
        }
        found
    };
    let rtl_of = |style: &style::properties::ComputedValues| {
        style.clone_direction() == style::computed_values::direction::T::Rtl
    };
    let style = nodes[span].primary_styles()?;
    let length = |v: &LengthPercentage| v.resolve(Length::new(0.0)).px() * scale;
    let margin = |v: &Margin| match v {
        GenericMargin::LengthPercentage(v) => length(v),
        _ => 0.0,
    };
    let (b, p, m) = (style.get_border(), style.get_padding(), style.get_margin());
    let border = |width: &style::values::computed::BorderSideWidth, style: style::values::computed::BorderStyle| {
        if style.none_or_hidden() { 0.0 } else { width.0.to_f32_px() * scale }
    };
    // pyro: in a vertical writing mode a span edge is its margin only (`span_edge_widths_in`).
    let (outer_left, outer_right) = if style.writing_mode.is_vertical() {
        let (start, end) = crate::layout::inline::span_edge_margins(&style, 0.0);
        if rtl_of(&style) { (end * scale, start * scale) } else { (start * scale, end * scale) }
    } else {
        (
            margin(&m.margin_left) + border(&b.border_left_width, b.border_left_style),
            margin(&m.margin_right) + border(&b.border_right_width, b.border_right_style),
        )
    };
    let (padding_top, padding_bottom) = (length(&p.padding_top.0), length(&p.padding_bottom.0));
    let rtl = rtl_of(&style);
    drop(style);

    // (x0, x1, top, bottom) of the first and the last fragment.
    let mut first: Option<(f32, f32, f32, f32)> = None;
    let mut last = None;
    for line in layout.lines() {
        let mut fragment: Option<(f32, f32, u16)> = None;
        let mut line_has_content = false;
        for item in line.items() {
            let (style_index, x0, x1) = match &item {
                PositionedLayoutItem::GlyphRun(run) => {
                    line_has_content |= run.advance() > 0.0;
                    (run.style_index(), run.offset(), run.offset() + run.advance())
                }
                PositionedLayoutItem::InlineBox(ibox) => match ibox.kind {
                    InlineBoxKind::InFlow => {
                        line_has_content = true;
                        (ibox.style_index, ibox.x, ibox.x + ibox.width)
                    }
                    InlineBoxKind::SpanStart | InlineBoxKind::SpanEnd => {
                        line_has_content = true;
                        let (mut x0, mut x1) = (ibox.x, ibox.x + ibox.width);
                        if ibox.id == span.as_u64() {
                            if (ibox.kind == InlineBoxKind::SpanStart) != rtl {
                                x0 += outer_left;
                            } else {
                                x1 -= outer_right;
                            }
                        }
                        (ibox.style_index, x0, x1)
                    }
                    _ => (ibox.style_index, ibox.x, ibox.x),
                },
            };
            let Some(own) = span_style(style_index) else {
                continue;
            };
            fragment = Some(match fragment {
                Some((left, right, own)) => (left.min(x0), right.max(x1), own),
                None => (x0, x1, own),
            });
        }
        if let Some((x0, mut x1, own)) = fragment {
            // pyro: the collapsible white space hanging past the end of a left-to-right line is in
            // no box (`LineMetrics::collapsed_advance`): a right-to-left span whose run ends the
            // line holds it all the same, which put its edge past the line's (WPT css-position
            // `multicol/*-ltr-rtl-in-multicols`).
            let metrics = line.metrics();
            if !layout.is_rtl() && metrics.collapsed_advance > 0.0 {
                let content_end = metrics.offset + metrics.advance - metrics.collapsed_advance;
                if x1 > content_end && x1 - content_end <= metrics.collapsed_advance + 0.01 {
                    x1 = content_end.max(x0);
                }
            }
            let (top, bottom) = if line_has_content {
                let (top, bottom) = line.span_box_area(own);
                (top - padding_top, bottom + padding_bottom)
            } else {
                let top = line.metrics().block_min_coord;
                (top, top)
            };
            first.get_or_insert((x0, x1, top, bottom));
            last = Some((x0, x1, top, bottom));
        }
    }
    let (first, last) = (first?, last?);
    // pyro: the start edge of the first fragment and the end edge of the last are those of the
    // lines' direction, the block container's, not the span's own (Blink: an `rtl` span in an
    // `ltr` line contains from the left of its first fragment to the right of its last; WPT
    // css-position `multicol/*-ltr-rtl-in-multicols`, `*-rtl-ltr-*`).
    let (left, right) = if layout.is_rtl() { (last.0, first.1) } else { (first.0, last.1) };
    let (top, bottom) = (first.2, last.3);
    Some((left / scale, top / scale, (right - left).max(0.0) / scale, (bottom - top).max(0.0) / scale))
}

impl LayoutPassState<'_> {
    /// pyro: whether the split inline `span` is the containing block of absolutely positioned
    /// boxes (it is positioned; `split_inline_containing_block`, in the logical axes layout runs
    /// in, so vertical writing modes too).
    pub(crate) fn split_inline_contains_absolute(&self, span: NodeId) -> bool {
        use style::computed_values::position::T as Position;
        self.nodes[span].primary_styles().is_some_and(|style| {
            style.clone_position() != Position::Static
        })
    }

    /// pyro: whether a layout child of `parent` is a split inline that is a containing block
    /// ([`Self::split_inline_contains_absolute`]).
    pub(crate) fn has_positioned_split_inline_child(&self, parent: NodeId) -> bool {
        self.nodes[parent].layout_children.borrow().as_ref().is_some_and(|children| {
            children
                .iter()
                .any(|&child| self.nodes[child].is_split_inline() && self.split_inline_contains_absolute(child))
        })
    }

    /// pyro: the split inline that is the containing block of the absolutely positioned box
    /// `box_id`, when it is a layout child of `parent`: the box's nearest positioned ancestor.
    pub(crate) fn split_inline_holding(&self, parent: NodeId, box_id: NodeId) -> Option<NodeId> {
        use style::computed_values::position::T as Position;
        let mut current = self.nodes[box_id].parent;
        while let Some(id) = current {
            if id == parent {
                return None;
            }
            let node = &self.nodes[id];
            if node.primary_styles().is_some_and(|style| style.clone_position() != Position::Static) {
                return (node.is_split_inline()
                    && node.layout_parent.get() == Some(parent)
                    && self.split_inline_contains_absolute(id))
                .then_some(id);
            }
            current = node.parent;
        }
        None
    }

    /// pyro: the containing block of the absolutely positioned boxes in the split inline `span`
    /// (a positioned inline box split around block-level content), a layout child of `parent`: the
    /// bounding box of the padding boxes of its first and last fragments (CSS 2.1 § 10.1 4.1), its
    /// head at the end of the anonymous block before its box and its tail at the start of the one
    /// after, moved by its relative offset; as `(size, offset)` from `parent`'s border box. A piece
    /// with no fragment there (an empty head or tail) is a point: the end of the last line before,
    /// the start of the first line after, or the top or the bottom of its box without such a line
    /// (as Chromium places an empty fragment; WPT `position-absolute-in-inline-003`,
    /// `position-absolute-dynamic-relayout-*`).
    pub(crate) fn split_inline_containing_block(
        &self,
        parent: NodeId,
        span: NodeId,
    ) -> Option<(taffy::Size<f32>, taffy::Point<f32>)> {
        let scale = self.viewport.scale();
        let children = self.nodes[parent].layout_children.borrow();
        let children = children.as_ref()?;
        let at = children.iter().position(|&child| child == span)?;
        let style = self.nodes[span].primary_styles()?;
        let rtl = style.clone_direction() == style::computed_values::direction::T::Rtl;
        let parent_layout = self.nodes[parent].unrounded_layout();
        let cb_size = taffy::Size {
            width: parent_layout.size.width - parent_layout.border.left - parent_layout.border.right
                - parent_layout.padding.left - parent_layout.padding.right,
            height: parent_layout.size.height - parent_layout.border.top - parent_layout.border.bottom
                - parent_layout.padding.top - parent_layout.padding.bottom,
        };
        let parent_rtl = self.nodes[parent]
            .primary_styles()
            .is_some_and(|style| style.clone_direction() == style::computed_values::direction::T::Rtl);
        let relative = if style.clone_position() == style::computed_values::position::T::Relative {
            crate::layout::inline::relative::relative_offset(&style, cb_size, parent_rtl)
        } else {
            taffy::Point::ZERO
        };
        drop(style);
        // The box laid out for the span's block-level content, without its relative offset.
        let own = self.nodes[span].unrounded_layout();
        let own_x = own.location.x - relative.x;
        let own_y = own.location.y - relative.y;
        // (x0, y0, x1, y1) of a piece, from the parent's border box.
        // (An anonymous block next to the box holds the head or the tail; when the span starts or
        // ends with a block-level box, its empty fragment is on that block's line all the same.)
        // (The flag says whether the piece is a fragment of the span, not a point.)
        let piece = |anonymous: NodeId, last: bool| -> Option<(f32, f32, f32, f32, bool)> {
            let node = &self.nodes[anonymous];
            if !node.is_anonymous() {
                return None;
            }
            let text_layout = node.data.downcast_element()?.inline_layout_data.as_deref()?;
            let location = node.unrounded_layout().location;
            let origin = (location.x, location.y + text_layout.line_box_top);
            if let Some((x, y, width, height)) = span_padding_box(&self.nodes, text_layout, span, scale) {
                return Some((origin.0 + x, origin.1 + y, origin.0 + x + width, origin.1 + y + height, true));
            }
            let line = if last { text_layout.layout.lines().last() } else { text_layout.layout.lines().next() };
            let metrics = *line?.metrics();
            let start = metrics.offset / scale;
            let end = (metrics.offset + metrics.advance - metrics.collapsed_advance) / scale;
            // The end of the last line (in the line's direction), the start of the first.
            let x = if last != rtl { end } else { start };
            let (top, bottom) = (metrics.block_min_coord / scale, metrics.block_max_coord / scale);
            Some((origin.0 + x, origin.1 + top, origin.0 + x, origin.1 + bottom, false))
        };
        let head = at
            .checked_sub(1)
            .and_then(|before| piece(children[before], true))
            .unwrap_or((own_x, own_y, own_x, own_y, false));
        let tail = children
            .get(at + 1)
            .and_then(|&after| piece(after, false))
            .unwrap_or((own_x, own_y + own.size.height, own_x, own_y + own.size.height, false));
        // The start edge and the top of the first fragment, the end edge and the bottom of the last
        // (CSS 2.1 § 10.1 4.1: left/top of the first box, right/bottom of the last, in `ltr`); the
        // tail's line can start left of the head's (WPT `position-absolute-in-inline-007b`).
        // An empty piece (a point) gives no inline edge when the other piece is a fragment: a span
        // that ends (starts) with its block-level box spans its head (tail) alone (Chromium; WPT
        // `abspos-in-block-in-inline-in-relpos-inline-002`).
        let (first, last) = match (head.4, tail.4) {
            (true, false) => (head, head),
            (false, true) => (tail, tail),
            _ => (head, tail),
        };
        let (x0, x1) = if rtl { (last.0, first.2) } else { (first.0, last.2) };
        let (y0, y1) = (head.1, tail.3);
        let x1 = x1.max(x0);
        Some((
            taffy::Size { width: (x1 - x0).max(0.0), height: (y1 - y0).max(0.0) },
            taffy::Point { x: x0 + relative.x, y: y0 + relative.y },
        ))
    }
}
