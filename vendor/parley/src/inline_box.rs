// Copyright 2024 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use crate::VerticalAlign;

/// A box to be laid out inline with text
#[derive(PartialEq, Debug, Clone)]
pub struct InlineBox {
    /// User-specified identifier for the box, which can be used by the user to determine which box in
    /// parley's output corresponds to which box in its input.
    pub id: u64,
    /// Whether the box is in-flow (takes up space in the layout) or out-of-flow (e.g. absolutely positioned or floated)
    pub kind: InlineBoxKind,
    /// The byte offset into the underlying text string at which the box should be placed.
    /// This must not be within a Unicode code point.
    pub index: usize,
    /// The width of the box in pixels
    pub width: f32,
    /// The height of the box in pixels
    pub height: f32,
    /// The baseline of the box in pixels, relative to the top of the box
    ///
    /// If `None`, the baseline is the box's bottom edge.
    pub baseline: Option<f32>,
    /// Vertical alignment of the box within its line, relative to the enclosing style span.
    pub vertical_align: VerticalAlign,
    /// pyro: for a span edge ([`InlineBoxKind::SpanStart`], [`InlineBoxKind::SpanEnd`]), whether
    /// its span is right-to-left: its start edge is its right side and its end edge its left.
    /// A line that bidi reordering changed puts each edge on its side of the span's content there.
    pub rtl: bool,
}

/// An [`InlineBox`] together with the style index of the span that contains it.
#[derive(PartialEq, Debug, Clone)]
pub(crate) struct LayoutInlineBox {
    pub(crate) inline_box: InlineBox,
    /// Style index of the span containing the box, against which the box's `vertical_align`
    /// is resolved. The box itself has no style of its own.
    pub(crate) parent_style_index: u16,
    /// Offset of the box's baseline above the baseline of the independent aligned subtree it
    /// belongs to (that of the parent style), resolved when the box is placed on a line (box
    /// sizes may change between building and line breaking). Unused for out-of-flow and
    /// `vertical-align: top | bottom` boxes.
    pub(crate) baseline_offset: f32,
}

/// Whether a box is in-flow (takes up space in the layout) or out-of-flow (e.g. absolutely positioned)
/// or custom-out-of-flow (line-breaking should yield control flow)
#[derive(PartialEq, Debug, Clone, Copy)]
pub enum InlineBoxKind {
    /// `InFlow` boxes take up space in the layout and flow in line with text
    ///
    /// They correspond to `display: inline-block` boxes in CSS.
    InFlow,
    /// `OutOfFlow` boxes are assigned a position as if they were a zero-sized inline box, but
    /// do not take up space in the layout.
    ///
    /// They correspond to `position: absolute` boxes in CSS.
    OutOfFlow,
    /// `CustomOutOfFlow` boxes also do not take up space in the layout, but they are not assigned a position
    /// by Parley. When they are encountered, control flow is yielded back to the caller who is then responsible
    /// for laying out the box.
    ///
    /// They can be used to implement advanced layout modes such as CSS's `float`
    CustomOutOfFlow,
    /// pyro: the start edge of a span box (a CSS inline box that is not atomic): the margin,
    /// border and padding of its inline-start side, as the box's `width`. It takes that space in
    /// the line and adds nothing to the line's height. It is no content of its own: it stays on
    /// the line of the content after it (a wrap opportunity before that content is one before the
    /// edge), and white space collapses through it. Pushed inside the span (after its style).
    SpanStart,
    /// pyro: the end edge of a span box, as [`Self::SpanStart`] for the inline-end side. It
    /// stays on the line of the content before it. Pushed inside the span (before its style is
    /// popped).
    SpanEnd,
}

impl InlineBoxKind {
    /// pyro: whether this is the start or end edge of a span box ([`Self::SpanStart`],
    /// [`Self::SpanEnd`]) rather than a box.
    pub fn is_span_edge(self) -> bool {
        matches!(self, Self::SpanStart | Self::SpanEnd)
    }

    /// pyro: whether a box of this kind takes its width in the line: an in-flow box or a span
    /// edge.
    pub(crate) fn takes_inline_space(self) -> bool {
        matches!(self, Self::InFlow | Self::SpanStart | Self::SpanEnd)
    }
}
