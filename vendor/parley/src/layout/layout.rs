// Copyright 2025 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use crate::IndentOptions;
use crate::InlineBox;
use crate::layout::alignment::align;
use crate::layout::data::LayoutData;
use crate::layout::{
    ContentWidths, SpanMetrics, Style, alignment::Alignment, alignment::AlignmentOptions,
    line::Line, line_break::BreakLines,
};
use crate::style::Brush;

use core::cmp::Ordering;
use core::fmt;

/// Text layout.
///
/// The [`Debug`] implementation prints a compact summary by default.
/// The alternate form (`{:#?}`) formats the full underlying data.
///
/// [`Debug`]: core::fmt::Debug
#[derive(Clone, PartialEq)]
pub struct Layout<B: Brush> {
    pub(crate) data: LayoutData<B>,
}

impl<B: Brush> fmt::Debug for Layout<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            f.debug_struct("Layout").field("data", &self.data).finish()
        } else {
            f.debug_struct("Layout")
                .field("text_len", &self.data.text_len)
                .field("width", &self.data.width)
                .field("height", &self.data.height)
                .field("lines", &self.data.lines.len())
                .field("runs", &self.data.runs.len())
                .field("styles", &self.data.styles.len())
                .finish_non_exhaustive()
        }
    }
}

impl<B: Brush> Layout<B> {
    /// Creates an empty layout.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears the layout while retaining capacity.
    ///
    /// This clears underlying strong references to fonts, which would otherwise
    /// prevent those fonts from being freed until dropping or reusing this layout.
    pub fn clear(&mut self) {
        self.data.clear();
    }

    /// Returns the scale factor provided when creating the layout.
    pub fn scale(&self) -> f32 {
        self.data.scale
    }

    /// Returns the style collection for the layout.
    pub fn styles(&self) -> &[Style<B>] {
        &self.data.styles
    }

    /// pyro: the index in [`Self::styles`] of the style of the span enclosing the span of style
    /// `style_index`. The root style (index 0) is its own parent.
    pub fn style_parent(&self, style_index: u16) -> u16 {
        self.data
            .styles
            .get(usize::from(style_index))
            .map_or(0, |style| style.parent)
    }

    /// The inherent metrics of the root span (in CSS terms, that is the metrics of the
    /// [root inline box](https://www.w3.org/TR/css-inline-3/#root-inline-box)).
    ///
    /// These are available once the layout is built, and will be `None` otherwise.
    /// The metric values come from the first available font and font size resolved
    /// from [`push_default`](crate::RangedBuilder::push_default).
    ///
    /// The motivating use case is placing bullet list markers - Blink uses the root
    /// span's ascent and descent for their location and size.
    pub fn root_span_metrics(&self) -> Option<SpanMetrics> {
        self.data.style_metrics.first().map(|it| SpanMetrics {
            ascent: it.ascent,
            descent: it.descent,
            x_height: it.x_height,
        })
    }

    /// Returns the length of the source text, in bytes.
    pub fn text_len(&self) -> usize {
        self.data.text_len
    }

    /// Returns the alignment that was applied to the layout by [`Layout::align`].
    ///
    /// Returns `None` if the layout has not been aligned since it was last built.
    pub fn alignment(&self) -> Option<Alignment> {
        self.data.alignment
    }

    /// The `max_advance` that was used to line break the `Layout`
    pub fn layout_max_advance(&self) -> f32 {
        self.data.layout_max_advance
    }

    /// Returns the computed width of the layout excluding the width of hanging whitespace.
    pub fn width(&self) -> f32 {
        self.data.width
    }

    /// Returns the computed width of the layout including the width of trailing whitespace.
    pub fn full_width(&self) -> f32 {
        self.data.full_width
    }

    /// Returns the y-offset of the baseline of the first line in the layout.
    /// Returns `None` if the layout is empty.
    pub fn first_baseline(&self) -> Option<f32> {
        self.lines().next().map(|line| line.metrics().baseline)
    }

    /// Returns the y-offset of the baseline of the last line in the layout.
    /// Returns `None` if the layout is empty.
    pub fn last_baseline(&self) -> Option<f32> {
        self.lines().next_back().map(|line| line.metrics().baseline)
    }

    /// Calculates the lower and upper bounds on the width of the layout. These
    /// are recalculated every time this method is called.
    ///
    /// This method currently may not return the correct results for
    /// mixed-direction text.
    pub fn calculate_content_widths(&self) -> ContentWidths {
        self.data.calculate_content_widths()
    }

    /// Returns the height of the layout.
    pub fn height(&self) -> f32 {
        self.data.height
    }

    /// Returns the number of lines in the layout.
    pub fn len(&self) -> usize {
        self.data.lines.len()
    }

    /// Returns `true` if the layout is empty.
    pub fn is_empty(&self) -> bool {
        self.data.lines.is_empty()
    }

    /// Returns the line at the specified index.
    ///
    /// Returns `None` if the index is out of bounds, i.e. if it's
    /// not less than [`self.len()`](Self::len).
    pub fn get(&self, index: usize) -> Option<Line<'_, B>> {
        Some(Line {
            index: index as u32,
            layout: self,
            data: self.data.lines.get(index)?,
        })
    }

    /// Returns `true` if the dominant direction of the layout is right-to-left.
    pub fn is_rtl(&self) -> bool {
        self.data.base_level.is_rtl()
    }

    /// Iterator over inline boxes in text index order.
    pub fn inline_boxes(&self) -> impl ExactSizeIterator<Item = &InlineBox> + '_ {
        self.data.inline_boxes.iter().map(|b| &b.inline_box)
    }

    /// Mutable iterator over the inline boxes in text index order.
    pub fn inline_boxes_mut(&mut self) -> impl ExactSizeIterator<Item = &mut InlineBox> + '_ {
        self.data.inline_boxes.iter_mut().map(|b| &mut b.inline_box)
    }

    /// pyro: moves line `index` to start `offset` from its inline start (its alignment offset;
    /// a line clamp aligns the clamp line with its ellipsis again).
    pub fn set_line_offset(&mut self, index: usize, offset: f32) {
        if let Some(line) = self.data.lines.get_mut(index) {
            line.metrics.offset = offset;
        }
    }

    /// Returns an iterator over the lines in the layout.
    pub fn lines(
        &self,
    ) -> impl ExactSizeIterator<Item = Line<'_, B>> + DoubleEndedIterator + '_ + Clone {
        self.data
            .lines
            .iter()
            .enumerate()
            .map(move |(index, data)| Line {
                index: index as u32,
                layout: self,
                data,
            })
    }

    /// Sets the text-indent for the layout.
    ///
    /// The indent is applied as a margin on the start edge of indented lines, reducing the
    /// available width for line breaking and offsetting content during alignment. Negative
    /// values cause the line to protrude beyond the start edge.
    ///
    /// This must be called before [`Layout::break_all_lines`] or [`Layout::break_lines`],
    /// and before [`Layout::align`].
    pub fn set_text_indent(&mut self, amount: f32, options: IndentOptions) {
        self.data.indent_amount = amount;
        self.data.indent_options = options;
    }

    /// pyro: the text index the first line ends at the latest: content that starts there or after
    /// it does not fit on it (a `::first-line` style that changes layout ends the line where it
    /// ends in that style); `None` by default. Like the indent, set before the lines are broken.
    pub fn set_first_line_limit(&mut self, end: Option<usize>) {
        self.data.first_line_limit = end;
    }

    /// pyro: makes the layout's preserved tabs go to tab stops, one every `tab-size` of the
    /// tab's style ([`TabSize`](crate::TabSize)) from the start edge of the lines' container.
    /// `space` is the advance that a number of spaces counts in, in layout units; a tab closer
    /// than `min_advance` to its stop goes on to the next. Without this a tab has the advance of
    /// a space.
    ///
    /// Like the indent, this must be set before the content widths are read and the lines broken.
    /// pyro: the `(space, min_advance)` [`Self::set_tab_stops`] set.
    pub fn tab_stops(&self) -> (f32, f32) {
        (self.data.tab_space, self.data.tab_min_advance)
    }

    /// pyro: the advance of the layout's first cluster, if it starts the content (after nothing
    /// but zero-width span start edges and out-of-flow boxes) and `hangs` says its first character
    /// (`text` is the layout's text) may hang (CSS `hanging-punctuation: first`).
    pub fn first_cluster_hang(&self, text: &str, hangs: impl Fn(char) -> bool) -> Option<f32> {
        use crate::layout::LayoutItemKind;
        let data = &self.data;
        for item in &data.items {
            match item.kind {
                LayoutItemKind::InlineBox => {
                    let inline_box = &data.inline_boxes[item.index].inline_box;
                    match inline_box.kind {
                        crate::InlineBoxKind::InFlow => return None,
                        crate::InlineBoxKind::SpanStart | crate::InlineBoxKind::SpanEnd if inline_box.width > 0.0 => {
                            return None;
                        }
                        _ => {}
                    }
                }
                LayoutItemKind::TextRun => {
                    let slice = data.shaped_text.run_slice(item.index as u32);
                    let cluster = slice.shaped_clusters().first()?;
                    let character = slice.characters_in(cluster.chars_range()).first()?;
                    let ch = text.get(character.text_byte_start as usize..)?.chars().next()?;
                    return hangs(ch).then_some(cluster.advance);
                }
            }
        }
        None
    }

    pub fn set_tab_stops(&mut self, space: f32, min_advance: f32) {
        self.data.tab_space = if space.is_finite() { space.max(0.0) } else { 0.0 };
        self.data.tab_min_advance = min_advance.max(0.0);
    }

    /// pyro: the line height calculation quirk (Quirks Mode §3.3): the root span box (the strut)
    /// counts on a line only when the line holds text; a line of inline boxes alone is as tall
    /// as they are. Set before the lines are broken.
    pub fn set_line_height_quirk(&mut self, quirk: bool) {
        self.data.line_height_quirk = quirk;
    }

    /// pyro: the lines align on a central dominant baseline (vertical writing modes whose text is
    /// not sideways; Blink's `FontBaseline::kCentralBaseline`): an in-flow inline box's
    /// [`InlineBox::baseline`](crate::InlineBox::baseline) is its central baseline, which a
    /// baseline-aligned box puts on the central baseline of the span around it (the middle of that
    /// span's content area), not on its alphabetic baseline.
    /// Text keeps its alphabetic geometry: the strut's line-height box is the same around either
    /// baseline. Set before the lines are broken.
    pub fn set_central_baseline(&mut self, central: bool) {
        self.data.central_baseline = central;
    }

    /// pyro: makes the layout's lines scale their text to fill them (CSS `text-fit`); `scale` is
    /// the factor of every line for [`TextFitTarget::Consistent`](crate::TextFitTarget), which
    /// [`Self::text_fit_block_scale`] measures on the lines broken without it.
    ///
    /// Like the indent, this must be set before the lines are broken.
    pub fn set_text_fit(&mut self, fit: crate::TextFit, scale: Option<f32>) {
        self.data.text_fit = fit;
        self.data.text_fit_scale = scale.filter(|scale| scale.is_finite() && *scale > 0.0);
    }

    /// pyro: the factor `text-fit: grow consistent` or `shrink consistent` scales every line of
    /// this layout by, measured on its lines as broken without one (Blink's
    /// `MeasurePerBlockScale`): the smallest that makes a line fill its width, of the lines with
    /// text that do not already (within two pixels). `None` when it would not grow (shrink) the
    /// text.
    pub fn text_fit_block_scale(&self) -> Option<f32> {
        use crate::{TextFitKind, TextFitTarget};
        let fit = self.data.text_fit;
        if fit.kind == TextFitKind::None || fit.target != TextFitTarget::Consistent {
            return None;
        }
        let epsilon = 2.0 * self.data.scale;
        let mut minimum = f32::INFINITY;
        for line in &self.data.lines {
            let items = self.data.line_items.get(line.item_range.clone()).unwrap_or_default();
            let fixed = self.data.line_fixed_advance(items);
            let content = line.metrics.advance - line.metrics.hanging_advance;
            let remaining = line.max_advance - content;
            if !remaining.is_finite() || remaining.abs() < epsilon {
                continue;
            }
            let flexible = content - fixed;
            if flexible <= 0.0 || remaining + flexible <= 0.0 {
                continue;
            }
            minimum = minimum.min((remaining + flexible) / flexible);
        }
        if !minimum.is_finite() {
            return None;
        }
        let scale = fit.restrict(minimum);
        let applies = match fit.kind {
            TextFitKind::Grow => scale > 1.0,
            TextFitKind::Shrink => scale < 1.0,
            TextFitKind::None => false,
        };
        applies.then_some(scale)
    }

    /// Returns line breaker to compute lines for the layout.
    pub fn break_lines(&mut self) -> BreakLines<'_, B> {
        BreakLines::new(self)
    }

    /// Breaks all lines with the specified maximum advance.
    pub fn break_all_lines(&mut self, max_advance: Option<f32>) {
        self.break_lines()
            .break_remaining(max_advance.unwrap_or(f32::MAX));
    }

    /// Apply alignment to the layout.
    ///
    /// You must perform line breaking prior to aligning, through [`Layout::break_lines`] or
    /// [`Layout::break_all_lines`].
    ///
    /// If a finite `max_advance` is supplied to `Layout::break_all_lines` then that width will be applied
    /// relative to that width. Otherwise alignment will be applied relative to the width of the
    /// longest line as computed by line breaking.
    ///
    /// If line-specific `offset` and `max_advance` are set using the advanced methods on the `BreakLines`
    /// struct then each line will be aligned individually within its line box.
    pub fn align(&mut self, alignment: Alignment, options: AlignmentOptions) {
        align(&mut self.data, alignment, options);
    }

    /// pyro: CSS Text 4 `text-group-align`: aligns the lines (as [`Self::align`]) within a group
    /// as wide as the widest line's content, then places the group in each line box: `left` of
    /// the free space goes to the group's left (0 at the left edge, 1 at the right, 0.5 centred).
    pub fn align_in_group(&mut self, alignment: Alignment, options: AlignmentOptions, left: f32) {
        let group = self
            .data
            .lines
            .iter()
            .map(|line| line.metrics.indent + line.metrics.advance - line.metrics.hanging_advance)
            .fold(0.0f32, f32::max);
        // As wide as a box of the group's min-content width: whole layout units (1/64 CSS px).
        let scale = if self.data.scale > 0.0 { self.data.scale } else { 1.0 };
        let group = (group / scale * 64.0).ceil() / 64.0 * scale;
        let saved: alloc::vec::Vec<(f32, f32)> = self
            .data
            .lines
            .iter_mut()
            .map(|line| {
                let saved = (line.metrics.inline_min_coord, line.metrics.inline_max_coord);
                line.metrics.inline_max_coord = line.metrics.inline_min_coord + group;
                saved
            })
            .collect();
        align(&mut self.data, alignment, options);
        for (line, (min, max)) in self.data.lines.iter_mut().zip(saved) {
            let free = (max - min - group).max(0.0);
            line.metrics.inline_min_coord = min;
            line.metrics.inline_max_coord = max;
            line.metrics.offset += free * left;
        }
    }

    /// Returns the index and `Line` object for the line containing the
    /// given byte `index` in the source text.
    pub(crate) fn line_for_byte_index(&self, index: usize) -> Option<(usize, Line<'_, B>)> {
        let line_index = self
            .data
            .lines
            .binary_search_by(|line| {
                if index < line.text_range.start {
                    Ordering::Greater
                } else if index >= line.text_range.end {
                    Ordering::Less
                } else {
                    Ordering::Equal
                }
            })
            .ok()?;
        Some((line_index, self.get(line_index)?))
    }

    /// Returns the index and `Line` object for the line containing the
    /// given `offset`.
    ///
    /// The offset is specified in the direction orthogonal to line direction.
    /// For horizontal text, this is a vertical or y offset. If the offset is
    /// on a line boundary, it is considered to be contained by the later line.
    pub(crate) fn line_for_offset(&self, offset: f32) -> Option<(usize, Line<'_, B>)> {
        if offset < 0.0 {
            return Some((0, self.get(0)?));
        }
        let maybe_line_index = self.data.lines.binary_search_by(|line| {
            if offset < line.metrics.block_min_coord {
                Ordering::Greater
            } else if offset >= line.metrics.block_max_coord {
                Ordering::Less
            } else {
                Ordering::Equal
            }
        });
        let line_index = match maybe_line_index {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        };
        Some((line_index, self.get(line_index)?))
    }
}

impl<B: Brush> Default for Layout<B> {
    fn default() -> Self {
        Self {
            data: LayoutData::default(),
        }
    }
}
