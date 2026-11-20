// Copyright 2024 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use super::BreakReason;
use crate::data::LayoutData;
use crate::style::Brush;

/// Alignment of a layout.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Alignment {
    /// This is [`Alignment::Left`] for LTR text and [`Alignment::Right`] for RTL text.
    #[default]
    Start,
    /// This is [`Alignment::Right`] for LTR text and [`Alignment::Left`] for RTL text.
    End,
    /// Align content to the left edge.
    ///
    /// For alignment that should be aware of text direction, use [`Alignment::Start`] or
    /// [`Alignment::End`] instead.
    Left,
    /// Align each line centered within the container.
    Center,
    /// Align content to the right edge.
    ///
    /// For alignment that should be aware of text direction, use [`Alignment::Start`] or
    /// [`Alignment::End`] instead.
    Right,
    /// Justify each line by spacing out content.
    ///
    /// The last line of a paragraph is start-aligned instead, unless
    /// [`AlignmentOptions::last_line_alignment`] is set.
    Justify,
}

/// Additional options to fine tune alignment
#[derive(Debug, Clone, Copy)]
pub struct AlignmentOptions {
    /// If set to `true`, "end" and "center" alignment will apply even if the line contents are
    /// wider than the alignment width. If it is set to `false`, all overflowing lines will be
    /// [`Alignment::Start`] aligned.
    pub align_when_overflowing: bool,
    /// The alignment of the last line of each paragraph: the last line of the layout and every
    /// line that ends in an explicit line break. This corresponds to the CSS `text-align-last`
    /// property.
    ///
    /// If set to `None`, those lines use the alignment passed to [`Layout::align`], except that
    /// [`Alignment::Justify`] falls back to [`Alignment::Start`]. This is the behavior of
    /// `text-align-last: auto`.
    ///
    /// [`Layout::align`]: crate::Layout::align
    pub last_line_alignment: Option<Alignment>,
}

#[expect(
    clippy::derivable_impls,
    reason = "Make default values explicit rather than relying on the implicit default value of bool"
)]
impl Default for AlignmentOptions {
    fn default() -> Self {
        Self {
            align_when_overflowing: false,
            last_line_alignment: None,
        }
    }
}

/// Align the layout.
pub(crate) fn align<B: Brush>(
    layout: &mut LayoutData<B>,
    alignment: Alignment,
    options: AlignmentOptions,
) {
    layout.alignment = Some(alignment);

    let base_rtl = layout.base_level.is_rtl();
    let plaintext = !layout.paragraph_levels.is_empty();

    // Apply alignment to line items
    for line_index in 0..layout.lines.len() {
        // pyro: each line takes its paragraph's direction (`unicode-bidi: plaintext`).
        let is_rtl = if plaintext {
            layout.level_at(layout.lines[line_index].text_range.start).is_rtl()
        } else {
            base_rtl
        };
        let line = &mut layout.lines[line_index];
        line.justification.amount_per_opportunity = 0.;

        let indent = line.metrics.indent;

        if is_rtl {
            // In RTL text, trailing whitespace is on the left. As we hang that whitespace, offset
            // the line to the left. Note: indent is not subtracted here because `free_space` below
            // already accounts for it.
            line.metrics.offset = -line.metrics.hanging_advance;
        } else {
            line.metrics.offset = indent;
        }

        // Compute free space.
        let line_width = line.metrics.inline_max_coord - line.metrics.inline_min_coord;
        let free_space = line_width - indent - line.metrics.advance + line.metrics.hanging_advance;

        if !options.align_when_overflowing && free_space <= 0.0 {
            if is_rtl {
                // In RTL text, right-align on overflow.
                line.metrics.offset += free_space;
            }
            continue;
        }

        let is_last_line = matches!(line.break_reason, BreakReason::None | BreakReason::Explicit);
        let line_alignment = if !is_last_line {
            alignment
        } else if let Some(last_line_alignment) = options.last_line_alignment {
            last_line_alignment
        } else if alignment == Alignment::Justify {
            Alignment::Start
        } else {
            alignment
        };

        // pyro: the line's content is as wide as Blink's text items, each a whole number of layout
        // units (1/64 CSS px, `ShapeResult::SnappedWidth` rounds up): a right- or centre-aligned
        // line then starts where a shrink-to-fit box of the same text would.
        let snapped_free_space = || {
            let unit = 64.0 / layout.scale;
            let advance = line.metrics.advance - line.metrics.hanging_advance;
            let excess = |width: f32| ((width * unit - 0.01).ceil() / unit - width).max(0.0);
            if line.item_range.len() <= 1 || line.metrics.hanging_advance != 0.0 || line.text_fit.is_some() {
                return free_space - excess(advance);
            }
            let mut owner = None;
            let mut item_width = 0.0;
            let mut rounding = 0.0;
            for item in &layout.line_items[line.item_range.clone()] {
                if item.kind == super::data::LayoutItemKind::TextRun {
                    let slice = layout.shaped_text.run_slice(item.index as u32).narrow(item.shaped_cluster_range.clone());
                    let spacing = super::spacing::EffectiveSpacing::new(layout.runs[item.index].spacing, super::spacing::Justification::NONE);
                    let Some(first) = slice.characters().first() else { continue };
                    let shaping_item = (first.style_index, layout.shaped_text.runs()[item.index].segment_start);
                    if owner != Some(shaping_item) {
                        rounding += excess(item_width);
                        item_width = 0.0;
                        owner = Some(shaping_item);
                    }
                    let width = spacing.slice_advance(slice);
                    item_width += width;
                } else {
                    rounding += excess(item_width);
                    item_width = 0.0;
                    owner = None;
                }
            }
            free_space - rounding - excess(item_width)
        };
        match (line_alignment, is_rtl) {
            (Alignment::Left, _) | (Alignment::Start, false) | (Alignment::End, true) => {
                // Do nothing
            }
            (Alignment::Right, _) | (Alignment::Start, true) | (Alignment::End, false) => {
                line.metrics.offset += snapped_free_space();
            }
            (Alignment::Center, _) => {
                line.metrics.offset += snapped_free_space() * 0.5;
            }
            (Alignment::Justify, _) => {
                // Justified alignment doesn't have any effect if free_space is negative or zero
                if free_space <= 0.0 {
                    continue;
                }

                // Justified alignment doesn't apply if there are no whitespace gaps to adjust. In
                // that case, start-align, i.e., left-align for LTR text and right-align for RTL
                // text.
                if line.num_justification_opportunities == 0 {
                    if is_rtl {
                        line.metrics.offset += free_space;
                    }
                    continue;
                }

                line.justification.amount_per_opportunity =
                    free_space / line.num_justification_opportunities as f32;
            }
        }
    }
}
