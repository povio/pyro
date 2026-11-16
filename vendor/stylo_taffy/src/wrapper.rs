use crate::CopyStyleGetters;
use crate::convert;
use crate::writing_mode::WritingModeExt;
use bitflags::bitflags;
use convert::stylo;
use std::ops::Deref;
use style::logical_geometry::WritingMode;
use style::properties::ComputedValues;
use style::values::CustomIdent;
use style::{Atom, OwnedSlice};
use taffy::ResolveOrZero;

#[cfg(feature = "grid")]
use style::values::{
    computed::{GridTemplateAreas, LengthPercentage},
    generics::grid::{TrackListValue, TrackRepeat, TrackSize},
    specified::position::NamedArea,
};

bitflags! {
    /// Flags for style information that Taffy needs but which is not part of the
    /// stylo [`ComputedValues`]
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
    pub struct StyleFlags: u8 {
        /// Whether the node is a replaced element (e.g. an image or form control)
        const IS_REPLACED = 1 << 0;
        /// Whether an inline span laid out by the node (an inline root) is a containing block
        /// for `position: absolute` boxes
        const SPAN_ABSOLUTE_CB = 1 << 1;
        /// Whether an inline span laid out by the node (an inline root) is a containing block
        /// for `position: fixed` boxes
        const SPAN_FIXED_CB = 1 << 2;
        /// Whether the node is a button element
        const IS_BUTTON = 1 << 3;
        /// Whether the node is a button-type input whose label always uses block layout
        const IS_BUTTON_INPUT = 1 << 4;
        /// pyro: the node is an inline box split around block-level content (CSS 2.1
        /// § 9.2.1.1), laid out as a block box that wraps its pieces: its margins, borders,
        /// padding and sizes belong to its inline pieces, so as a box it has none.
        const IS_SPLIT_INLINE = 1 << 5;
        /// pyro: the node is a replaced element with a natural aspect ratio (a loaded image, a
        /// canvas): `aspect-ratio: auto <ratio>` then means the natural one (css-sizing-4 §5.1).
        const HAS_NATURAL_RATIO = 1 << 6;
        /// pyro: the node is the `<body>` whose `overflow` the viewport takes (CSS Overflow 3
        /// §3.3): its used `overflow` is `visible` in both axes, so it is no scroll container
        /// (it does not establish a block formatting context, its margins collapse).
        const OVERFLOW_TO_VIEWPORT = 1 << 7;
    }
}

/// A wrapper struct for anything that `Deref`s to a [`stylo::ComputedValues`](ComputedValues) (can be pointed to by an `&` reference, [`Arc`](std::sync::Arc),
/// [`Ref`](std::cell::Ref), etc). It implements [`taffy`]'s [layout traits](taffy::traits) and can used with Taffy's [layout algorithms](taffy::compute).
pub struct TaffyStyloStyle<T: Deref<Target = ComputedValues>> {
    /// The stylo style
    pub style: T,
    /// Extra node-derived flags that are not part of the stylo style
    pub flags: StyleFlags,
    /// The writing mode of the algorithm laying this box out, whose inline/block axes the Taffy
    /// getters are expressed in (see [`crate::writing_mode`]). Defaults to the style's own.
    #[cfg(feature = "writing-mode")]
    pub layout_wm: WritingMode,
    /// Whether `align-self` of this node is applied in the layout writing mode's inline axis (the node is an item
    /// or out-of-flow child of a column flex container) rather than its block axis.
    #[cfg(feature = "writing-mode")]
    pub align_axis_is_inline: bool,
    /// When set, percentages in `padding` resolve against this length instead of being passed to
    /// Taffy. Used for a box in an orthogonal flow, whose padding percentages resolve against its
    /// containing block's inline size while Taffy would resolve them against the (transposed)
    /// `parent_size.width`, i.e. the containing block's block size.
    #[cfg(feature = "writing-mode")]
    pub percent_basis: Option<f32>,
    /// pyro: the natural aspect ratio of a replaced element (width over height, physical), set by the
    /// tree; `CoreStyle::natural_aspect_ratio` answers it while `aspect-ratio` is `auto`.
    pub natural_ratio: Option<f32>,
}

impl<T: Deref<Target = ComputedValues>> TaffyStyloStyle<T> {
    /// The display used for contents layout, including button-input fixups.
    #[inline]
    pub fn effective_display(&self) -> stylo::Display {
        let display = self.style.clone_display();
        if self.flags.contains(StyleFlags::IS_BUTTON_INPUT) {
            convert::button_input_display(display)
        } else {
            display
        }
    }

    /// Create a new [`TaffyStyloStyle`] from a stylo style and [`StyleFlags`], expressed in the
    /// style's own writing mode
    pub fn new(style: T, flags: StyleFlags) -> Self {
        #[cfg(feature = "writing-mode")]
        let layout_wm = style.writing_mode;
        Self {
            style,
            flags,
            #[cfg(feature = "writing-mode")]
            layout_wm,
            #[cfg(feature = "writing-mode")]
            align_axis_is_inline: false,
            #[cfg(feature = "writing-mode")]
            percent_basis: None,
            natural_ratio: None,
        }
    }

    /// Create a new [`TaffyStyloStyle`] expressed in the axes of `layout_wm`, the writing mode of
    /// the algorithm laying this box out. Without the `writing-mode` feature it is ignored.
    #[cfg_attr(not(feature = "writing-mode"), allow(unused_variables))]
    pub fn new_in(style: T, flags: StyleFlags, layout_wm: WritingMode) -> Self {
        Self {
            style,
            flags,
            #[cfg(feature = "writing-mode")]
            layout_wm,
            #[cfg(feature = "writing-mode")]
            align_axis_is_inline: false,
            #[cfg(feature = "writing-mode")]
            percent_basis: None,
            natural_ratio: None,
        }
    }

    /// The writing mode the getters are expressed in (the style's own without the feature)
    #[inline(always)]
    fn layout_wm(&self) -> WritingMode {
        #[cfg(feature = "writing-mode")]
        {
            self.layout_wm
        }
        #[cfg(not(feature = "writing-mode"))]
        {
            self.style.writing_mode
        }
    }

    #[inline(always)]
    fn align_axis_is_inline(&self) -> bool {
        #[cfg(feature = "writing-mode")]
        {
            self.align_axis_is_inline
        }
        #[cfg(not(feature = "writing-mode"))]
        {
            false
        }
    }

    #[inline(always)]
    fn percent_basis(&self) -> Option<f32> {
        #[cfg(feature = "writing-mode")]
        {
            self.percent_basis
        }
        #[cfg(not(feature = "writing-mode"))]
        {
            None
        }
    }

    /// Map a physical rect into the layout writing mode's axes
    #[inline(always)]
    fn rect<U>(&self, physical: taffy::Rect<U>) -> taffy::Rect<U> {
        self.layout_wm().logical_rect(physical)
    }

    /// Map a physical size into the layout writing mode's axes
    #[inline(always)]
    fn size<U>(&self, physical: taffy::Size<U>) -> taffy::Size<U> {
        self.layout_wm().logical_size(physical)
    }
}

// Deref<stylo::ComputedValues> impl
impl<T: Deref<Target = ComputedValues>> From<T> for TaffyStyloStyle<T> {
    fn from(value: T) -> Self {
        Self::new(value, StyleFlags::empty())
    }
}

// Into<taffy::Style> impl
impl<T: Deref<Target = ComputedValues>> From<TaffyStyloStyle<T>> for taffy::Style<Atom> {
    fn from(value: TaffyStyloStyle<T>) -> Self {
        let mut style = convert::to_taffy_style(&value.style);
        value.layout_wm().transpose_style(&mut style);
        style.item_is_replaced = value.flags.contains(StyleFlags::IS_REPLACED);
        style
    }
}

// CoreStyle impl
impl<T: Deref<Target = ComputedValues>> taffy::CoreStyle for TaffyStyloStyle<T> {
    type CustomIdent = Atom;

    #[inline]
    fn box_generation_mode(&self) -> taffy::BoxGenerationMode {
        convert::box_generation_mode(self.style.get_box().display)
    }

    #[inline]
    fn is_block(&self) -> bool {
        // Button inputs always establish an independent formatting context.
        !self.flags.contains(StyleFlags::IS_BUTTON_INPUT)
            && convert::is_block(self.effective_display())
    }

    #[inline]
    fn is_compressible_replaced(&self) -> bool {
        self.flags.contains(StyleFlags::IS_REPLACED)
    }

    #[inline]
    fn is_replaced(&self) -> bool {
        self.flags.contains(StyleFlags::IS_REPLACED)
    }

    #[inline]
    fn box_sizing(&self) -> taffy::BoxSizing {
        convert::box_sizing(self.style.get_position().box_sizing)
    }

    #[inline]
    fn direction(&self) -> taffy::Direction {
        self.layout_wm().direction_of(self.style.writing_mode)
    }

    #[inline]
    fn overflow(&self) -> taffy::Point<taffy::Overflow> {
        if self.flags.contains(StyleFlags::OVERFLOW_TO_VIEWPORT) {
            return taffy::Point { x: taffy::Overflow::Visible, y: taffy::Overflow::Visible };
        }
        let box_styles = self.style.get_box();
        let overflow = taffy::Point {
            x: convert::overflow(box_styles.overflow_x),
            y: convert::overflow(box_styles.overflow_y),
        };
        self.layout_wm().logical_point(overflow)
    }

    #[inline]
    fn scrollbar_width(&self) -> f32 {
        0.0
    }

    #[inline]
    fn contain(&self) -> taffy::Contain {
        let box_styles = self.style.get_box();
        convert::contain(box_styles.contain, self.effective_display())
    }

    #[inline]
    fn position(&self) -> taffy::Position {
        convert::position(self.style.get_box().position)
    }

    #[inline]
    fn is_containing_block(&self) -> taffy::ContainingBlockClaims {
        let mut claims = convert::containing_block_claims(&self.style);
        claims.absolute |= self.flags.contains(StyleFlags::SPAN_ABSOLUTE_CB);
        claims.fixed |= self.flags.contains(StyleFlags::SPAN_FIXED_CB);
        claims
    }

    #[inline]
    fn inset(&self) -> taffy::Rect<taffy::LengthPercentageAuto> {
        self.rect(convert::inset_rect(&self.style))
    }

    #[inline]
    fn size(&self) -> taffy::Size<taffy::Dimension> {
        if self.flags.contains(StyleFlags::IS_SPLIT_INLINE) {
            return taffy::Size::auto();
        }
        let position_styles = self.style.get_position();
        self.size(taffy::Size {
            width: convert::dimension(&position_styles.width),
            height: convert::dimension(&position_styles.height),
        })
    }

    #[inline]
    fn min_size(&self) -> taffy::Size<taffy::LengthPercentageAuto> {
        if self.flags.contains(StyleFlags::IS_SPLIT_INLINE) {
            return taffy::Size::auto();
        }
        let position_styles = self.style.get_position();
        self.size(taffy::Size {
            width: convert::min_size(&position_styles.min_width),
            height: convert::min_size(&position_styles.min_height),
        })
    }

    #[inline]
    fn max_size(&self) -> taffy::Size<taffy::LengthPercentageAuto> {
        if self.flags.contains(StyleFlags::IS_SPLIT_INLINE) {
            return taffy::Size::auto();
        }
        let position_styles = self.style.get_position();
        self.size(taffy::Size {
            width: convert::max_size(&position_styles.max_width),
            height: convert::max_size(&position_styles.max_height),
        })
    }

    #[inline]
    fn aspect_ratio(&self) -> Option<f32> {
        let aspect_ratio = self.style.get_position().aspect_ratio;
        // pyro: `auto <ratio>` on a replaced element with a natural aspect ratio is `auto`: the
        // natural ratio, which its measure function applies.
        if aspect_ratio.auto && self.flags.contains(StyleFlags::HAS_NATURAL_RATIO) {
            return None;
        }
        let ratio = convert::aspect_ratio(aspect_ratio);
        self.layout_wm().logical_aspect_ratio(ratio)
    }

    #[inline]
    fn natural_aspect_ratio(&self) -> Option<f32> {
        if !self.style.get_position().aspect_ratio.auto {
            return None;
        }
        self.layout_wm().logical_aspect_ratio(self.natural_ratio)
    }

    #[inline]
    fn aspect_ratio_of_content_box(&self) -> bool {
        self.style.get_position().aspect_ratio.auto && !self.flags.contains(StyleFlags::IS_REPLACED)
    }

    #[inline]
    fn margin(&self) -> taffy::Rect<taffy::LengthPercentageAuto> {
        if self.flags.contains(StyleFlags::IS_SPLIT_INLINE) {
            return taffy::Rect::zero();
        }
        let margin_styles = self.style.get_margin();
        self.rect(taffy::Rect {
            left: convert::margin(&margin_styles.margin_left),
            right: convert::margin(&margin_styles.margin_right),
            top: convert::margin(&margin_styles.margin_top),
            bottom: convert::margin(&margin_styles.margin_bottom),
        })
    }

    #[inline]
    fn padding(&self) -> taffy::Rect<taffy::LengthPercentage> {
        if self.flags.contains(StyleFlags::IS_SPLIT_INLINE) {
            return taffy::Rect::zero();
        }
        let padding_styles = self.style.get_padding();
        if let Some(basis) = self.percent_basis() {
            let resolve = |value: &stylo::LengthPercentage| {
                taffy::LengthPercentage::length(
                    convert::length_percentage(value)
                        .resolve_or_zero(Some(basis), convert::resolve_calc_value),
                )
            };
            return self.rect(taffy::Rect {
                left: resolve(&padding_styles.padding_left.0),
                right: resolve(&padding_styles.padding_right.0),
                top: resolve(&padding_styles.padding_top.0),
                bottom: resolve(&padding_styles.padding_bottom.0),
            });
        }
        self.rect(taffy::Rect {
            left: convert::length_percentage(&padding_styles.padding_left.0),
            right: convert::length_percentage(&padding_styles.padding_right.0),
            top: convert::length_percentage(&padding_styles.padding_top.0),
            bottom: convert::length_percentage(&padding_styles.padding_bottom.0),
        })
    }

    #[inline]
    fn border(&self) -> taffy::Rect<taffy::LengthPercentage> {
        if self.flags.contains(StyleFlags::IS_SPLIT_INLINE) {
            return taffy::Rect::zero();
        }
        let border_styles = self.style.get_border();
        self.rect(taffy::Rect {
            left: convert::border(
                &border_styles.border_left_width,
                border_styles.border_left_style,
            ),
            right: convert::border(
                &border_styles.border_right_width,
                border_styles.border_right_style,
            ),
            top: convert::border(
                &border_styles.border_top_width,
                border_styles.border_top_style,
            ),
            bottom: convert::border(
                &border_styles.border_bottom_width,
                border_styles.border_bottom_style,
            ),
        })
    }
}

// BlockContainerStyle impl
#[cfg(feature = "block")]
impl<T: Deref<Target = ComputedValues>> taffy::BlockContainerStyle for TaffyStyloStyle<T> {
    #[inline]
    fn margin_trim(&self) -> taffy::Line<bool> {
        use style::values::computed::MarginTrim;
        let trim = *self.style.get_margin().get_margin_trim();
        taffy::Line {
            start: trim.contains(MarginTrim::BLOCK_START),
            end: trim.contains(MarginTrim::BLOCK_END),
        }
    }

    #[inline]
    fn text_align(&self) -> taffy::TextAlign {
        let align = convert::text_align(self.style.clone_text_align());
        self.layout_wm().logical_text_align(align)
    }

    #[inline]
    fn align_content(&self) -> taffy::AlignContent {
        let display = self.effective_display();
        let align_content =
            convert::content_alignment(self.style.get_position().align_content, display);
        if align_content.keyword() == taffy::AlignContentKeyword::Normal
            && self
                .flags
                .intersects(StyleFlags::IS_BUTTON | StyleFlags::IS_BUTTON_INPUT)
            && matches!(
                display.inside(),
                stylo::DisplayInside::Flow | stylo::DisplayInside::FlowRoot
            )
        {
            taffy::AlignContent::SAFE_CENTER
        } else if align_content.keyword() == taffy::AlignContentKeyword::Normal
            && display.inside() == stylo::DisplayInside::TableCell
        {
            convert::table_cell_vertical_align(&self.style).unwrap_or(align_content)
        } else {
            align_content
        }
    }

    #[inline]
    fn justify_items(&self) -> taffy::AlignItems {
        self.inline_item_alignment((self.style.get_position().justify_items.computed.0).0)
            .unwrap_or(taffy::AlignItems::NORMAL)
    }
}

impl<T: Deref<Target = ComputedValues>> TaffyStyloStyle<T> {
    /// Item alignment in the layout writing mode's inline axis; `left`/`right` resolve against it.
    #[inline]
    fn inline_item_alignment(&self, input: stylo::AlignFlags) -> Option<taffy::AlignItems> {
        convert::item_alignment(input, self.layout_wm().inline_left_is_end())
    }

    /// How `left` resolves for content distributed along the layout writing mode's inline or block axis
    /// (`None`: the block axis of a horizontal writing mode, where `left`/`right` behave as `start`).
    #[inline]
    fn content_left_is_end(&self, main_is_inline: bool) -> Option<bool> {
        if main_is_inline {
            Some(self.layout_wm().inline_left_is_end())
        } else if self.layout_wm().swaps_axes() {
            Some(self.layout_wm().block_left_is_end())
        } else {
            None
        }
    }

    /// `align-self`/`justify-self` of a flex or grid item in the layout writing mode's block axis
    /// (`block_axis`) or inline axis.
    #[inline]
    fn self_alignment(
        &self,
        input: stylo::AlignFlags,
        block_axis: bool,
    ) -> Option<taffy::AlignItems> {
        if block_axis {
            self.block_item_alignment(input)
        } else {
            self.inline_item_alignment(input)
        }
    }

    /// `align-self`/`justify-self` of a block-level or out-of-flow box in the layout writing mode's
    /// block axis (`block_axis`) or inline axis.
    #[inline]
    fn oof_self_alignment(
        &self,
        input: stylo::AlignFlags,
        block_axis: bool,
    ) -> Option<taffy::AlignItems> {
        if block_axis {
            self.block_item_alignment(input)
        } else {
            self.oof_inline_item_alignment(input)
        }
    }

    /// Inline-axis alignment of a block-level or out-of-flow box: `left`/`right` resolve to the
    /// box's own `self-start`/`self-end`, which Taffy maps back through [`direction`](Self::direction).
    #[inline]
    fn oof_inline_item_alignment(&self, input: stylo::AlignFlags) -> Option<taffy::AlignItems> {
        let layout_wm = self.layout_wm();
        let own_is_rtl = layout_wm.direction_of(self.style.writing_mode) == taffy::Direction::Rtl;
        convert::oof_item_alignment(input, true, own_is_rtl != layout_wm.line_left_is_bottom())
    }

    /// Resolve percentage padding against `basis` instead of the parent's inline size (a box laid
    /// out in an orthogonal flow). No-op without the `writing-mode` feature.
    #[inline]
    #[cfg_attr(not(feature = "writing-mode"), allow(unused_variables))]
    pub fn set_percent_basis(&mut self, basis: Option<f32>) {
        #[cfg(feature = "writing-mode")]
        {
            self.percent_basis = basis;
        }
    }

    /// Item alignment in the layout writing mode's block axis. With the feature on, `self-start`/`self-end`
    /// are resolved here against
    /// the node's own writing mode because Taffy treats its block axis as having no direction.
    #[inline]
    fn block_item_alignment(&self, input: stylo::AlignFlags) -> Option<taffy::AlignItems> {
        let mut align = if self.layout_wm().swaps_axes() {
            convert::item_alignment(input, self.layout_wm().block_left_is_end())?
        } else {
            convert::oof_item_alignment(input, false, false)?
        };
        if cfg!(feature = "writing-mode") {
            let reversed = self
                .layout_wm()
                .block_start_is_reversed(self.style.writing_mode);
            align.keyword = match align.keyword {
                taffy::AlignItemsKeyword::SelfStart if reversed => taffy::AlignItemsKeyword::End,
                taffy::AlignItemsKeyword::SelfStart => taffy::AlignItemsKeyword::Start,
                taffy::AlignItemsKeyword::SelfEnd if reversed => taffy::AlignItemsKeyword::Start,
                taffy::AlignItemsKeyword::SelfEnd => taffy::AlignItemsKeyword::End,
                keyword => keyword,
            };
        }
        Some(align)
    }
}

// BlockItemStyle impl
#[cfg(feature = "block")]
impl<T: Deref<Target = ComputedValues>> taffy::BlockItemStyle for TaffyStyloStyle<T> {
    #[inline]
    fn is_table(&self) -> bool {
        convert::is_table(self.effective_display())
    }

    #[inline]
    fn min_size_keyword(&self) -> taffy::Size<taffy::Dimension> {
        let position = self.style.get_position();
        self.size(taffy::Size {
            width: convert::min_size_keyword(&position.min_width),
            height: convert::min_size_keyword(&position.min_height),
        })
    }

    #[inline]
    fn max_size_keyword(&self) -> taffy::Size<taffy::Dimension> {
        let position = self.style.get_position();
        self.size(taffy::Size {
            width: convert::max_size_keyword(&position.max_width),
            height: convert::max_size_keyword(&position.max_height),
        })
    }

    #[inline]
    fn breaks_lines_in_block_axis(&self) -> bool {
        let position = self.style.get_position();
        self.effective_display().inside() == stylo::DisplayInside::Flex
            && matches!(
                convert::flex_direction(position.flex_direction),
                taffy::FlexDirection::Column | taffy::FlexDirection::ColumnReverse
            )
            && convert::flex_wrap(position.flex_wrap) != taffy::FlexWrap::NoWrap
    }

    #[inline]
    fn align_self(&self) -> Option<taffy::AlignSelf> {
        self.oof_self_alignment(
            self.style.get_position().align_self.0,
            !self.align_axis_is_inline(),
        )
    }

    #[inline]
    fn justify_self(&self) -> Option<taffy::AlignSelf> {
        self.oof_self_alignment(
            self.style.get_position().justify_self.0,
            self.align_axis_is_inline(),
        )
    }

    #[inline]
    fn align_content(&self) -> taffy::AlignContent {
        taffy::BlockContainerStyle::align_content(self)
    }

    #[cfg(feature = "floats")]
    #[inline]
    fn float(&self) -> taffy::Float {
        // pyro: `inline-start` and `inline-end` are sides of the containing block's inline axis:
        // in a right-to-left one (the box's inherited direction stands in for it) they trade.
        let value = self.style.clone_float();
        let float = match (value, self.style.writing_mode.is_bidi_ltr()) {
            (stylo::Float::InlineStart, false) => taffy::Float::Right,
            (stylo::Float::InlineEnd, false) => taffy::Float::Left,
            _ => convert::float(value),
        };
        self.layout_wm().logical_float(float)
    }

    #[cfg(feature = "floats")]
    #[inline]
    fn clear(&self) -> taffy::Clear {
        let value = self.style.clone_clear();
        let clear = match (value, self.style.writing_mode.is_bidi_ltr()) {
            (stylo::Clear::InlineStart, false) => taffy::Clear::Right,
            (stylo::Clear::InlineEnd, false) => taffy::Clear::Left,
            _ => convert::clear(value),
        };
        self.layout_wm().logical_clear(clear)
    }
}

// FlexboxContainerStyle impl
#[cfg(feature = "flexbox")]
impl<T: Deref<Target = ComputedValues>> taffy::FlexboxContainerStyle for TaffyStyloStyle<T> {
    #[inline]
    fn flex_direction(&self) -> taffy::FlexDirection {
        convert::flex_direction(self.style.get_position().flex_direction)
    }

    #[inline]
    fn flex_wrap(&self) -> taffy::FlexWrap {
        convert::flex_wrap(self.style.get_position().flex_wrap)
    }

    #[inline]
    fn flex_line_count(&self) -> u16 {
        convert::flex_line_count(self.style.get_position().flex_line_count)
    }

    #[inline]
    fn gap(&self) -> taffy::Size<taffy::LengthPercentage> {
        let position_styles = self.style.get_position();
        taffy::Size {
            width: convert::gap(&position_styles.column_gap),
            height: convert::gap(&position_styles.row_gap),
        }
    }

    #[inline]
    fn align_content(&self) -> taffy::AlignContent {
        convert::content_alignment(
            self.style.get_position().align_content,
            self.style.clone_display(),
        )
    }

    #[inline]
    fn align_items(&self) -> taffy::AlignItems {
        convert::default_item_alignment(self.style.get_position().align_items.0, false)
    }

    #[inline]
    fn justify_content(&self) -> taffy::JustifyContent {
        let position_styles = self.style.get_position();
        {
            let main_is_inline = matches!(
                position_styles.flex_direction,
                stylo::FlexDirection::Row | stylo::FlexDirection::RowReverse
            );
            convert::justify_content_in(
                position_styles.justify_content,
                self.content_left_is_end(main_is_inline),
                self.style.clone_display(),
            )
        }
    }
}

// FlexboxItemStyle impl
#[cfg(feature = "flexbox")]
impl<T: Deref<Target = ComputedValues>> taffy::FlexboxItemStyle for TaffyStyloStyle<T> {
    #[inline]
    fn flex_basis(&self) -> taffy::Dimension {
        convert::flex_basis(&self.style.get_position().flex_basis)
    }

    #[inline]
    fn flex_grow(&self) -> f32 {
        self.style.get_position().flex_grow.0
    }

    #[inline]
    fn flex_shrink(&self) -> f32 {
        self.style.get_position().flex_shrink.0
    }

    #[inline]
    fn align_self(&self) -> Option<taffy::AlignSelf> {
        self.self_alignment(
            self.style.get_position().align_self.0,
            !self.align_axis_is_inline(),
        )
    }
}

#[cfg(feature = "grid")]
pub struct GridAreaWrapper<'a>(pub &'a [NamedArea]);
#[cfg(feature = "grid")]
impl<'a> IntoIterator for GridAreaWrapper<'a> {
    type Item = taffy::GridTemplateArea<Atom>;

    type IntoIter = std::iter::Map<
        std::slice::Iter<'a, NamedArea>,
        for<'b> fn(&'b NamedArea) -> taffy::GridTemplateArea<Atom>,
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter().map(convert::grid_template_area)
    }
}

#[cfg(feature = "grid")]
type SliceMapIter<'a, Input, Output> =
    core::iter::Map<core::slice::Iter<'a, Input>, for<'c> fn(&'c Input) -> Output>;
#[cfg(feature = "grid")]
type SliceMapRefIter<'a, Input, Output> =
    core::iter::Map<core::slice::Iter<'a, Input>, for<'c> fn(&'c Input) -> &'c Output>;

// Line name iterator type aliases
#[cfg(feature = "grid")]
type LineNameSetIter<'a> = SliceMapRefIter<'a, CustomIdent, Atom>;
#[cfg(feature = "grid")]
type LineNameIter<'a> = core::iter::Map<
    core::slice::Iter<'a, OwnedSlice<CustomIdent>>,
    fn(&OwnedSlice<CustomIdent>) -> LineNameSetIter<'_>,
>;

#[derive(Clone)]
#[cfg(feature = "grid")]
pub struct StyloLineNameIter<'a>(LineNameIter<'a>);
#[cfg(feature = "grid")]
impl<'a> StyloLineNameIter<'a> {
    /// Create a new StyloLineNameIter
    pub fn new(names: &'a OwnedSlice<OwnedSlice<CustomIdent>>) -> Self {
        Self(names.iter().map(|names| names.iter().map(|ident| &ident.0)))
    }
}
#[cfg(feature = "grid")]
impl<'a> Iterator for StyloLineNameIter<'a> {
    type Item = core::iter::Map<core::slice::Iter<'a, CustomIdent>, fn(&CustomIdent) -> &Atom>;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}
#[cfg(feature = "grid")]
impl ExactSizeIterator for StyloLineNameIter<'_> {}
#[cfg(feature = "grid")]
impl<'a> taffy::TemplateLineNames<'a, Atom> for StyloLineNameIter<'a> {
    type LineNameSet<'b>
        = SliceMapRefIter<'b, CustomIdent, Atom>
    where
        Self: 'b;
}
#[cfg(feature = "grid")]
pub struct RepetitionWrapper<'a>(&'a TrackRepeat<LengthPercentage, i32>);
#[cfg(feature = "grid")]
impl taffy::GenericRepetition for RepetitionWrapper<'_> {
    type CustomIdent = Atom;

    type RepetitionTrackList<'a>
        = SliceMapIter<'a, stylo::TrackSize<LengthPercentage>, taffy::TrackSizingFunction>
    where
        Self: 'a;

    type TemplateLineNames<'a>
        = StyloLineNameIter<'a>
    where
        Self: 'a;

    fn count(&self) -> taffy::RepetitionCount {
        convert::track_repeat(self.0.count)
    }

    fn tracks(&self) -> Self::RepetitionTrackList<'_> {
        self.0.track_sizes.iter().map(convert::track_size)
    }

    fn lines_names(&self) -> Self::TemplateLineNames<'_> {
        StyloLineNameIter::new(&self.0.line_names)
    }
}

/// Physical grid style accessors. The `*_source` helpers pick the physical property that plays the
/// given Taffy role in the layout writing mode (rows and columns swap in a vertical one).
#[cfg(feature = "grid")]
impl<T: Deref<Target = ComputedValues>> TaffyStyloStyle<T> {
    #[inline]
    fn template_rows_source(&self) -> &stylo::GenericGridTemplateComponent<LengthPercentage, i32> {
        let position_styles = self.style.get_position();
        &position_styles.grid_template_rows
    }

    #[inline]
    fn template_columns_source(
        &self,
    ) -> &stylo::GenericGridTemplateComponent<LengthPercentage, i32> {
        let position_styles = self.style.get_position();
        &position_styles.grid_template_columns
    }

    #[inline]
    fn auto_rows_source(&self) -> &stylo::ImplicitGridTracks {
        let position_styles = self.style.get_position();
        &position_styles.grid_auto_rows
    }

    #[inline]
    fn auto_columns_source(&self) -> &stylo::ImplicitGridTracks {
        let position_styles = self.style.get_position();
        &position_styles.grid_auto_columns
    }

    #[inline]
    fn grid_row_placement(&self) -> taffy::Line<taffy::GridPlacement<Atom>> {
        let position_styles = self.style.get_position();
        taffy::Line {
            start: convert::grid_line(&position_styles.grid_row_start),
            end: convert::grid_line(&position_styles.grid_row_end),
        }
    }

    #[inline]
    fn grid_column_placement(&self) -> taffy::Line<taffy::GridPlacement<Atom>> {
        let position_styles = self.style.get_position();
        taffy::Line {
            start: convert::grid_line(&position_styles.grid_column_start),
            end: convert::grid_line(&position_styles.grid_column_end),
        }
    }
}

#[cfg(feature = "grid")]
impl<T: Deref<Target = ComputedValues>> taffy::GridContainerStyle for TaffyStyloStyle<T> {
    type Repetition<'a>
        = RepetitionWrapper<'a>
    where
        Self: 'a;

    type TemplateTrackList<'a>
        = core::iter::Map<
        core::slice::Iter<'a, TrackListValue<LengthPercentage, i32>>,
        fn(
            &'a TrackListValue<LengthPercentage, i32>,
        ) -> taffy::GenericGridTemplateComponent<Atom, RepetitionWrapper<'a>>,
    >
    where
        Self: 'a;

    type AutoTrackList<'a>
        = SliceMapIter<'a, TrackSize<LengthPercentage>, taffy::TrackSizingFunction>
    where
        Self: 'a;

    type TemplateLineNames<'a>
        = StyloLineNameIter<'a>
    where
        Self: 'a;
    type GridTemplateAreas<'a>
        = SliceMapIter<'a, NamedArea, taffy::GridTemplateArea<Atom>>
    where
        Self: 'a;

    #[inline]
    fn grid_template_rows(&self) -> Option<Self::TemplateTrackList<'_>> {
        match self.template_rows_source() {
            stylo::GenericGridTemplateComponent::None => None,
            stylo::GenericGridTemplateComponent::TrackList(list) => {
                Some(list.values.iter().map(|track| match track {
                    stylo::TrackListValue::TrackSize(size) => {
                        taffy::GenericGridTemplateComponent::Single(convert::track_size(size))
                    }
                    stylo::TrackListValue::TrackRepeat(repeat) => {
                        taffy::GenericGridTemplateComponent::Repeat(RepetitionWrapper(repeat))
                    }
                }))
            }

            // TODO: Implement subgrid
            stylo::GenericGridTemplateComponent::Subgrid(_) => None,
        }
    }

    #[inline]
    fn grid_template_columns(&self) -> Option<Self::TemplateTrackList<'_>> {
        match self.template_columns_source() {
            stylo::GenericGridTemplateComponent::None => None,
            stylo::GenericGridTemplateComponent::TrackList(list) => {
                Some(list.values.iter().map(|track| match track {
                    stylo::TrackListValue::TrackSize(size) => {
                        taffy::GenericGridTemplateComponent::Single(convert::track_size(size))
                    }
                    stylo::TrackListValue::TrackRepeat(repeat) => {
                        taffy::GenericGridTemplateComponent::Repeat(RepetitionWrapper(repeat))
                    }
                }))
            }

            // TODO: Implement subgrid
            stylo::GenericGridTemplateComponent::Subgrid(_) => None,
        }
    }

    #[inline]
    fn grid_auto_rows(&self) -> Self::AutoTrackList<'_> {
        self.auto_rows_source().0.iter().map(convert::track_size)
    }

    #[inline]
    fn grid_auto_columns(&self) -> Self::AutoTrackList<'_> {
        self.auto_columns_source().0.iter().map(convert::track_size)
    }

    fn grid_template_areas(&self) -> Option<Self::GridTemplateAreas<'_>> {
        match &self.style.get_position().grid_template_areas {
            GridTemplateAreas::Areas(areas) => {
                Some(areas.0.areas.iter().map(convert::grid_template_area))
            }
            GridTemplateAreas::None => None,
        }
    }

    fn grid_template_area_row_count(&self) -> u16 {
        match &self.style.get_position().grid_template_areas {
            GridTemplateAreas::Areas(areas) => convert::saturating_u16(areas.0.strings.len()),
            GridTemplateAreas::None => 0,
        }
    }

    fn grid_template_area_column_count(&self) -> u16 {
        match &self.style.get_position().grid_template_areas {
            GridTemplateAreas::Areas(areas) => convert::saturating_u16(areas.0.width),
            GridTemplateAreas::None => 0,
        }
    }

    fn grid_template_column_names(&self) -> Option<Self::TemplateLineNames<'_>> {
        match self.template_columns_source() {
            stylo::GenericGridTemplateComponent::None => None,
            stylo::GenericGridTemplateComponent::TrackList(list) => {
                Some(StyloLineNameIter::new(&list.line_names))
            }
            // TODO: Implement subgrid
            stylo::GenericGridTemplateComponent::Subgrid(_) => None,
        }
    }

    fn grid_template_row_names(&self) -> Option<Self::TemplateLineNames<'_>> {
        match self.template_rows_source() {
            stylo::GenericGridTemplateComponent::None => None,
            stylo::GenericGridTemplateComponent::TrackList(list) => {
                Some(StyloLineNameIter::new(&list.line_names))
            }
            // TODO: Implement subgrid
            stylo::GenericGridTemplateComponent::Subgrid(_) => None,
        }
    }

    #[inline]
    fn grid_auto_flow(&self) -> taffy::GridAutoFlow {
        convert::grid_auto_flow(self.style.get_position().grid_auto_flow)
    }

    /// pyro: `subgrid <line-name-list>` (css-grid-2 §7.2), `repeat(<integer>, ...)` expanded and one
    /// repetition of `repeat(auto-fill, ...)` marked.
    fn subgrid_line_names(&self, axis: taffy::AbsoluteAxis) -> Option<taffy::SubgridLineNames<Atom>> {
        let source = match axis {
            taffy::AbsoluteAxis::Horizontal => self.template_columns_source(),
            taffy::AbsoluteAxis::Vertical => self.template_rows_source(),
        };
        let stylo::GenericGridTemplateComponent::Subgrid(list) = source else {
            return None;
        };
        let mut names = taffy::SubgridLineNames::default();
        let set = |idents: &OwnedSlice<CustomIdent>| idents.iter().map(|ident| ident.0.clone()).collect::<Vec<_>>();
        for value in list.line_names.iter() {
            match value {
                style::values::generics::grid::LineNameListValue::LineNames(idents) => names.names.push(set(idents)),
                style::values::generics::grid::LineNameListValue::Repeat(repeat) => match repeat.count {
                    style::values::generics::grid::RepeatCount::Number(count) => {
                        // (Taffy clamps a grid to 10,000 tracks)
                        for _ in 0..count.clamp(0, 10_000) {
                            names.names.extend(repeat.line_names.iter().map(set));
                        }
                    }
                    _ => {
                        let start = names.names.len();
                        names.names.extend(repeat.line_names.iter().map(set));
                        names.auto_fill = Some(start..names.names.len());
                    }
                },
            }
        }
        Some(names)
    }

    /// pyro: a subgrid's `normal` gap is its parent's.
    fn gap_is_normal(&self) -> taffy::Size<bool> {
        let position = self.style.get_position();
        taffy::Size {
            width: matches!(position.column_gap, stylo::Gap::Normal),
            height: matches!(position.row_gap, stylo::Gap::Normal),
        }
    }

    #[inline]
    fn gap(&self) -> taffy::Size<taffy::LengthPercentage> {
        let position_styles = self.style.get_position();
        taffy::Size {
            width: convert::gap(&position_styles.column_gap),
            height: convert::gap(&position_styles.row_gap),
        }
    }

    #[inline]
    fn align_content(&self) -> taffy::AlignContent {
        convert::content_alignment(
            self.style.get_position().align_content,
            self.style.clone_display(),
        )
    }

    #[inline]
    fn justify_content(&self) -> taffy::JustifyContent {
        let position_styles = self.style.get_position();
        {
            let main_is_inline = matches!(
                position_styles.flex_direction,
                stylo::FlexDirection::Row | stylo::FlexDirection::RowReverse
            );
            convert::justify_content_in(
                position_styles.justify_content,
                self.content_left_is_end(main_is_inline),
                self.style.clone_display(),
            )
        }
    }

    #[inline]
    fn align_items(&self) -> taffy::AlignItems {
        convert::default_item_alignment(self.style.get_position().align_items.0, false)
    }

    #[inline]
    fn justify_items(&self) -> taffy::AlignItems {
        self.inline_item_alignment((self.style.get_position().justify_items.computed.0).0)
            .unwrap_or(taffy::AlignItems::NORMAL)
    }
}

// GridItemStyle impl
#[cfg(feature = "grid")]
impl<T: Deref<Target = ComputedValues>> taffy::GridItemStyle for TaffyStyloStyle<T> {
    #[inline]
    fn grid_row(&self) -> taffy::Line<taffy::GridPlacement<Atom>> {
        self.grid_row_placement()
    }

    /// pyro: a grid container in flow whose template in the parent's `axis` is `subgrid`, with neither
    /// layout nor paint containment (each makes it an independent formatting context, css-grid-2 §7.2),
    /// in the parent's orientation (orthogonal subgrids are not laid out as such).
    fn is_subgrid(&self, axis: taffy::AbsoluteAxis) -> bool {
        use style::values::computed::Contain;
        if self.effective_display().inside() != stylo::DisplayInside::Grid {
            return false;
        }
        if self.style.writing_mode.is_vertical() != self.layout_wm().is_vertical() {
            return false;
        }
        if matches!(self.style.clone_position(), stylo::Position::Absolute | stylo::Position::Fixed) {
            return false;
        }
        if self.style.get_box().contain.intersects(Contain::LAYOUT | Contain::PAINT) {
            return false;
        }
        // A size container has layout containment too (css-contain-3 §2)
        if self.style.get_box().container_type.intersects(
            style::values::specified::box_::ContainerType::SIZE | style::values::specified::box_::ContainerType::INLINE_SIZE,
        ) {
            return false;
        }
        let source = match axis {
            taffy::AbsoluteAxis::Horizontal => self.template_columns_source(),
            taffy::AbsoluteAxis::Vertical => self.template_rows_source(),
        };
        matches!(source, stylo::GenericGridTemplateComponent::Subgrid(_))
    }

    #[inline]
    fn grid_column(&self) -> taffy::Line<taffy::GridPlacement<Atom>> {
        self.grid_column_placement()
    }

    #[inline]
    fn align_self(&self) -> Option<taffy::AlignSelf> {
        self.self_alignment(
            self.style.get_position().align_self.0,
            !self.align_axis_is_inline(),
        )
    }

    #[inline]
    fn justify_self(&self) -> Option<taffy::AlignSelf> {
        self.self_alignment(
            self.style.get_position().justify_self.0,
            self.align_axis_is_inline(),
        )
    }
}

impl<T: Deref<Target = ComputedValues>> taffy::OofItemStyle for TaffyStyloStyle<T> {
    #[inline]
    fn align_self(&self) -> Option<taffy::AlignSelf> {
        self.oof_self_alignment(
            self.style.get_position().align_self.0,
            !self.align_axis_is_inline(),
        )
    }

    #[inline]
    fn justify_self(&self) -> Option<taffy::AlignSelf> {
        self.oof_self_alignment(
            self.style.get_position().justify_self.0,
            self.align_axis_is_inline(),
        )
    }

    #[inline]
    fn is_table(&self) -> bool {
        convert::is_table(self.effective_display())
    }

    #[inline]
    fn grid_row(&self) -> taffy::Line<taffy::GridPlacement<Atom>> {
        self.grid_row_placement()
    }

    #[inline]
    fn grid_column(&self) -> taffy::Line<taffy::GridPlacement<Atom>> {
        self.grid_column_placement()
    }
}
