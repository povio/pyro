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
    pub struct StyleFlags: u16 {
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
        /// pyro: the node is a `<fieldset>`: an inline one is an inline-block and a table-type one a
        /// block (HTML § 15.3.13).
        const IS_FIELDSET = 1 << 8;
        /// pyro: the element is replaced by the image of its `content: url()`.
        const IS_CONTENT_REPLACED = 1 << 9;
        /// pyro: the element is an `<input>`: its `overflow` makes it no scroll container for layout (an
        /// automatic minimum size of zero; Blink, WPT css-grid `stretch-grid-item-text-input-overflow`).
        const IS_INPUT = 1 << 10;
        /// pyro: classic scrollbars (`crate::scrollbars`): the node's `overflow-x: auto` shows its
        /// horizontal scrollbar (its content overflowed horizontally the last time it was laid out).
        const AUTO_SCROLLBAR_X = 1 << 11;
        /// pyro: classic scrollbars: the node's `overflow-y: auto` shows its vertical scrollbar.
        const AUTO_SCROLLBAR_Y = 1 << 12;
        /// pyro: the node is the root element, whose `overflow` and `scrollbar-gutter` are the
        /// viewport's: its own box takes no scrollbar gutters.
        const IS_ROOT = 1 << 13;
        /// pyro: the node never has scrollbars (a `<frameset>`: no scroll container in Chrome).
        const NO_SCROLLBARS = 1 << 14;
        /// pyro: the node's document lays out classic scrollbars (`crate::scrollbars`); without
        /// it scrollbars are overlay scrollbars, which take no space.
        const CLASSIC_SCROLLBARS = 1 << 15;
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
    #[allow(missing_docs)]
    pub custom_scrollbars: Option<taffy::Size<f32>>,
    #[allow(missing_docs)]
    pub scrollbar_size_limit: Option<taffy::Size<f32>>,
}

impl<T: Deref<Target = ComputedValues>> TaffyStyloStyle<T> {
    /// The display used for contents layout, including button-input fixups.
    #[inline]
    pub fn effective_display(&self) -> stylo::Display {
        let display = self.style.clone_display();
        if self.flags.contains(StyleFlags::IS_BUTTON_INPUT) {
            convert::button_input_display(display)
        } else if self.flags.intersects(StyleFlags::IS_FIELDSET | StyleFlags::IS_BUTTON) {
            convert::fieldset_display(display)
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
            custom_scrollbars: None,
            scrollbar_size_limit: None,
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
            custom_scrollbars: None,
            scrollbar_size_limit: None,
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
    fn is_content_replaced(&self) -> bool {
        self.flags.contains(StyleFlags::IS_CONTENT_REPLACED)
    }

    #[inline]
    fn root_float_is_right(&self) -> Option<bool> {
        let ltr = self.style.writing_mode.is_bidi_ltr();
        match self.style.clone_float() {
            stylo::Float::None => None,
            stylo::Float::Left => Some(false),
            stylo::Float::Right => Some(true),
            stylo::Float::InlineStart => Some(!ltr),
            stylo::Float::InlineEnd => Some(ltr),
        }
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
        if self.flags.intersects(StyleFlags::OVERFLOW_TO_VIEWPORT | StyleFlags::IS_INPUT) {
            return taffy::Point { x: taffy::Overflow::Visible, y: taffy::Overflow::Visible };
        }
        let box_styles = self.style.get_box();
        let overflow = taffy::Point {
            x: convert::overflow(box_styles.overflow_x),
            y: convert::overflow(box_styles.overflow_y),
        };
        self.layout_wm().logical_point(overflow)
    }

    // pyro: overlay scrollbars (the default `crate::scrollbars::ScrollbarMode`) take no space, so
    // `scrollbar-gutter` changes no layout; classic ones take their thickness.
    #[inline]
    fn scrollbar_width(&self) -> f32 {
        if !self.flags.contains(StyleFlags::CLASSIC_SCROLLBARS) {
            return 0.0;
        }
        crate::scrollbars::thickness_in(crate::scrollbars::ScrollbarMode::Classic, self.style.clone_scrollbar_width())
    }

    // pyro: the gutters of classic scrollbars (`crate::scrollbars::GutterInputs::gutters`), in the
    // layout's axes.
    #[inline]
    fn scrollbar_gutter(&self) -> taffy::Rect<f32> {
        let zero = taffy::Rect { left: 0.0, right: 0.0, top: 0.0, bottom: 0.0 };
        // (Overlay scrollbars: one flag test, as cheap as the constant it replaced.)
        if !self.flags.contains(StyleFlags::CLASSIC_SCROLLBARS)
            || self.flags.intersects(StyleFlags::OVERFLOW_TO_VIEWPORT | StyleFlags::IS_INPUT | StyleFlags::IS_ROOT | StyleFlags::NO_SCROLLBARS | StyleFlags::IS_REPLACED)
        {
            return zero;
        }
        let thickness = self.scrollbar_width();
        if thickness <= 0.0 {
            return zero;
        }
        let box_styles = self.style.get_box();
        let gutter = box_styles.scrollbar_gutter;
        let own_wm = self.style.writing_mode;
        let inputs = crate::scrollbars::GutterInputs {
            thickness: self.custom_scrollbars.unwrap_or(taffy::Size { width: thickness, height: thickness }),
            overflow: taffy::Point { x: box_styles.overflow_x, y: box_styles.overflow_y },
            auto_shows: taffy::Point {
                x: self.flags.contains(StyleFlags::AUTO_SCROLLBAR_X),
                y: self.flags.contains(StyleFlags::AUTO_SCROLLBAR_Y),
            },
            stable: gutter.contains(style::values::computed::ScrollbarGutter::STABLE),
            both_edges: gutter.contains(style::values::computed::ScrollbarGutter::BOTH_EDGES),
            vertical: own_wm.is_vertical(),
            rtl: !own_wm.is_bidi_ltr(),
        };
        self.rect(inputs.layout_edges(self.scrollbar_size_limit))
    }

    #[inline]
    fn contain(&self) -> taffy::Contain {
        let box_styles = self.style.get_box();
        convert::contain(box_styles.used_containment(), self.effective_display())
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
    fn calc_size_min(&self) -> taffy::Size<Option<taffy::CalcSizeFunction>> {
        let position_styles = self.style.get_position();
        self.size(taffy::Size {
            width: convert::calc_size_auto(&position_styles.min_width),
            height: convert::calc_size_auto(&position_styles.min_height),
        })
    }

    fn max_size_percentage_basis(&self) -> taffy::Size<Option<bool>> {
        let position = self.style.get_position();
        let classify = |max: &stylo::MaxSize| {
            let stylo::MaxSize::Calculation(size) = max else { return None };
            match size.prepared_size() {
                stylo::Size::CalcSizeAny(_) => Some(false),
                stylo::Size::CalcSizePercentage(_) => Some(true),
                _ => None,
            }
        };
        self.size(taffy::Size { width: classify(&position.max_width), height: classify(&position.max_height) })
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
        // pyro: `aspect-ratio` does not apply to internal table boxes (css-sizing-4 §2; WPT
        // css-sizing `aspect-ratio/table-element-001`).
        {
            use style::values::specified::box_::DisplayInside as I;
            if matches!(
                self.style.get_box().display.inside(),
                I::TableRowGroup
                    | I::TableColumn
                    | I::TableColumnGroup
                    | I::TableHeaderGroup
                    | I::TableFooterGroup
                    | I::TableRow
                    | I::TableCell
            ) {
                return None;
            }
        }
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
    /// pyro: a block container with a standard `line-clamp` (not `-webkit-line-clamp`, whose
    /// box is a flow root already) establishes an independent formatting context.
    #[inline]
    fn establishes_independent_formatting_context(&self) -> bool {
        let clamp = &self.style.get_box().line_clamp;
        // pyro: so does a size container (css-conditional-5 § 3.1 `container-type: size |
        // inline-size`: "causes the box to establish an independent formatting context"; no layout
        // containment, so its baseline stays); it keeps clear of floats (WPT container-queries
        // `inline-size-bfc-floats`).
        (!clamp.webkit_legacy && !clamp.max_lines.is_none())
            || self.style.get_box().container_type.intersects(
                style::values::specified::box_::ContainerType::SIZE
                    | style::values::specified::box_::ContainerType::INLINE_SIZE,
            )
    }

    #[inline]
    fn own_height_keywords(&self) -> (taffy::Dimension, taffy::Dimension) {
        let (min, max) = (
            <Self as taffy::BlockItemStyle>::min_size_keyword(self),
            <Self as taffy::BlockItemStyle>::max_size_keyword(self),
        );
        (min.height, max.height)
    }

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

    /// pyro: `start`/`end` by the direction in Taffy's axes, `left`/`right` by the line-left
    /// side (the bottom in `sideways-lr`, Taffy's right there).
    #[inline]
    fn text_align_edge(&self) -> taffy::AxisStaticEdge {
        use taffy::AxisStaticEdge::{Center, End, Start};
        let (start, end) = if taffy::CoreStyle::direction(self).is_rtl() { (End, Start) } else { (Start, End) };
        let (left, right) = if self.layout_wm().line_left_is_bottom() { (End, Start) } else { (Start, End) };
        match self.style.clone_text_align() {
            stylo::TextAlign::Start | stylo::TextAlign::Justify | stylo::TextAlign::JustifyAll => start,
            stylo::TextAlign::End => end,
            stylo::TextAlign::Left | stylo::TextAlign::MozLeft => left,
            stylo::TextAlign::Right | stylo::TextAlign::MozRight => right,
            stylo::TextAlign::Center | stylo::TextAlign::MozCenter => Center,
        }
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
        // pyro: an anonymous block or a split inline is no box of its own to the author:
        // `justify-items` does not apply to it (WPT css-align blocks/justify-items-anonymous,
        // justify-self-block-in-inline-002; csswg-drafts#11461/#11462, as Blink)
        if self.is_anonymous_or_split_inline() {
            return taffy::AlignItems::NORMAL;
        }
        self.inline_item_alignment((self.style.get_position().justify_items.computed.0).0)
            .unwrap_or(taffy::AlignItems::NORMAL)
    }
}

impl<T: Deref<Target = ComputedValues>> TaffyStyloStyle<T> {
    /// pyro: an anonymous block box, or an inline box split around block-level content laid
    /// out as a block: its `justify-self: auto` is `normal`, and it passes no `justify-items`
    /// on to its children.
    #[inline]
    fn is_anonymous_or_split_inline(&self) -> bool {
        self.flags.contains(StyleFlags::IS_SPLIT_INLINE)
            || matches!(self.style.pseudo(), Some(style::selector_parser::PseudoElement::ServoAnonymousBox))
    }

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
    fn max_size_calculation(&self) -> taffy::Size<Option<(taffy::Dimension, taffy::CalcSizeFunction)>> {
        if self.flags.contains(StyleFlags::IS_SPLIT_INLINE) {
            return taffy::Size { width: None, height: None };
        }
        let position = self.style.get_position();
        self.size(taffy::Size {
            width: convert::max_size_calculation(&position.max_width),
            height: convert::max_size_calculation(&position.max_height),
        })
    }

    /// pyro: a block container with a standard `line-clamp` (not `-webkit-line-clamp`, whose
    /// box is a flow root already) establishes an independent formatting context.
    #[inline]
    fn establishes_independent_formatting_context(&self) -> bool {
        let clamp = &self.style.get_box().line_clamp;
        // pyro: so does a size container (css-conditional-5 § 3.1 `container-type: size |
        // inline-size`: "causes the box to establish an independent formatting context"; no layout
        // containment, so its baseline stays); it keeps clear of floats (WPT container-queries
        // `inline-size-bfc-floats`).
        (!clamp.webkit_legacy && !clamp.max_lines.is_none())
            || self.style.get_box().container_type.intersects(
                style::values::specified::box_::ContainerType::SIZE
                    | style::values::specified::box_::ContainerType::INLINE_SIZE,
            )
    }

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
    fn original_display_is_inline(&self) -> bool {
        self.style.get_box().original_display.outside() == stylo::DisplayOutside::Inline
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
        let justify_self = self.style.get_position().justify_self.0;
        if justify_self.value() == stylo::AlignFlags::AUTO && self.is_anonymous_or_split_inline() {
            return Some(taffy::AlignItems::NORMAL);
        }
        self.oof_self_alignment(justify_self, self.align_axis_is_inline())
    }

    #[inline]
    fn align_content(&self) -> taffy::AlignContent {
        taffy::BlockContainerStyle::align_content(self)
    }

    #[cfg(feature = "floats")]
    #[inline]
    fn float(&self) -> taffy::Float {
        // pyro: `inline-start` and `inline-end` are sides of the containing block's inline axis:
        // in a right-to-left one they trade. The containing block's direction is that of the
        // algorithm laying the box out (`layout_wm`), not the box's own (WPT css-logical
        // `logical-values-float-clear-reftest`).
        let value = self.style.clone_float();
        let float = match (value, self.layout_wm().is_bidi_ltr()) {
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
        let clear = match (value, self.layout_wm().is_bidi_ltr()) {
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
    /// pyro: see `GridContainerStyle::baseline_synthesis` (only the central case differs).
    #[inline]
    fn block_baseline_synthesis(&self) -> f32 {
        let wm = self.style.writing_mode;
        // pyro: an alphabetic baseline is synthesized at the line-under edge: block-end, but
        // block-start in `vertical-lr` (its line-under side is the left; WPT css-flexbox
        // `baseline-synthesis-vert-lr-line-under`).
        if wm.is_vertical() && !wm.is_sideways() {
            0.5
        } else if wm.is_vertical() && wm.is_vertical_lr() {
            0.0
        } else {
            1.0
        }
    }
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

    fn flex_basis_calculation(&self) -> Option<taffy::CalcSizeFunction> {
        let stylo::FlexBasis::Size(size) = &self.style.get_position().flex_basis else { return None };
        match size.prepared_size() {
            stylo::Size::CalcSizeAuto(calc) | stylo::Size::CalcSizeStretch(calc) |
            stylo::Size::CalcSizeFitContent(calc) | stylo::Size::CalcSizeMinContent(calc) |
            stylo::Size::CalcSizeContent(calc) | stylo::Size::CalcSizeMaxContent(calc) => Some(convert::calc_size_function(&calc.0)),
            _ => None,
        }
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
    fn is_table(&self) -> bool {
        convert::is_table(self.effective_display())
    }

    #[inline]
    fn is_collapsed(&self) -> bool {
        self.style.get_inherited_box().visibility == style::computed_values::visibility::T::Collapse
    }

    #[inline]
    fn min_size_keyword(&self) -> taffy::Size<taffy::Dimension> {
        <Self as taffy::BlockItemStyle>::min_size_keyword(self)
    }

    #[inline]
    fn max_size_keyword(&self) -> taffy::Size<taffy::Dimension> {
        <Self as taffy::BlockItemStyle>::max_size_keyword(self)
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

    /// pyro: synthesized baselines sit on the line-under edge (alphabetic) or in the middle (central:
    /// vertical writing modes with upright or mixed text orientation), as Blink's
    /// `LogicalBoxFragment::FirstBaselineOrSynthesize`.
    fn baseline_synthesis(&self) -> taffy::Size<f32> {
        let wm = self.style.writing_mode;
        let block = if !wm.is_vertical() {
            1.0
        } else if !wm.is_sideways() {
            0.5
        } else if wm.intersects(WritingMode::LINE_INVERTED) {
            // `vertical-lr`: the line-under side is the block-start side
            0.0
        } else {
            1.0
        };
        // pyro: in the inline axis of a horizontal grid, the line-under edge of the vertical lines it
        // would hold (sideways `vertical-lr`: the left, `sideways_inverted_lines` in blitz-dom), so
        // the start in `ltr` (WPT css-grid `grid-self-baseline-006`)
        let inline = if wm.is_vertical() || !wm.is_bidi_ltr() { 1.0 } else { 0.0 };
        taffy::Size { width: inline, height: block }
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
    fn grid_min_size_keyword(&self) -> taffy::Size<taffy::Dimension> {
        <Self as taffy::BlockItemStyle>::min_size_keyword(self)
    }

    #[inline]
    fn grid_max_size_keyword(&self) -> taffy::Size<taffy::Dimension> {
        <Self as taffy::BlockItemStyle>::max_size_keyword(self)
    }

    #[inline]
    fn grid_row(&self) -> taffy::Line<taffy::GridPlacement<Atom>> {
        self.grid_row_placement()
    }

    /// pyro: a grid container in flow whose template in the parent's `axis` is `subgrid` (its own other axis
    /// in an orthogonal flow), with neither layout nor paint containment (each makes it an independent
    /// formatting context, css-grid-2 §7.2).
    fn is_subgrid(&self, axis: taffy::AbsoluteAxis) -> bool {
        use style::values::computed::Contain;
        if self.effective_display().inside() != stylo::DisplayInside::Grid {
            return false;
        }
        if matches!(self.style.clone_position(), stylo::Position::Absolute | stylo::Position::Fixed) {
            return false;
        }
        if self.style.get_box().used_containment().intersects(Contain::LAYOUT | Contain::PAINT) {
            return false;
        }
        // A size container has layout containment too (css-contain-3 §2)
        if self.style.get_box().container_type.intersects(
            style::values::specified::box_::ContainerType::SIZE | style::values::specified::box_::ContainerType::INLINE_SIZE,
        ) {
            return false;
        }
        let source = match self.subgrid_axis(axis) {
            taffy::AbsoluteAxis::Horizontal => self.template_columns_source(),
            taffy::AbsoluteAxis::Vertical => self.template_rows_source(),
        };
        matches!(source, stylo::GenericGridTemplateComponent::Subgrid(_))
    }

    /// pyro: the parent's inline axis is this box's block axis in an orthogonal flow
    fn subgrid_axis(&self, axis: taffy::AbsoluteAxis) -> taffy::AbsoluteAxis {
        if self.style.writing_mode.is_vertical() != self.layout_wm().is_vertical() {
            axis.other_axis()
        } else {
            axis
        }
    }

    /// pyro: whether the parent's tracks in `axis` and this subgrid's in the matching axis of its own run
    /// opposite ways on the screen
    fn subgrid_reversed(&self, axis: taffy::AbsoluteAxis) -> bool {
        // Whether the tracks along a writing mode's inline (`Horizontal`) or block axis run right to left or
        // bottom to top
        let reversed = |wm: WritingMode, axis: taffy::AbsoluteAxis| match (axis, wm.is_vertical()) {
            (taffy::AbsoluteAxis::Horizontal, false) => !wm.is_bidi_ltr(),
            (taffy::AbsoluteAxis::Horizontal, true) => !wm.is_inline_tb(),
            (taffy::AbsoluteAxis::Vertical, false) => false,
            (taffy::AbsoluteAxis::Vertical, true) => !wm.is_vertical_lr(),
        };
        reversed(self.layout_wm(), axis) != reversed(self.style.writing_mode, self.subgrid_axis(axis))
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
    fn min_size_keyword(&self) -> taffy::Size<taffy::Dimension> {
        taffy::BlockItemStyle::min_size_keyword(self)
    }

    #[inline]
    fn max_size_keyword(&self) -> taffy::Size<taffy::Dimension> {
        taffy::BlockItemStyle::max_size_keyword(self)
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
