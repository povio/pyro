//! Style types for Block layout
use crate::geometry::{Line, Size};
use crate::style::Dimension;
use crate::tree::AxisStaticEdge;
use crate::style::{AlignContent, AlignItems, AlignSelf};
use crate::{CoreStyle, Style};

/// The set of styles required for a Block layout container
pub trait BlockContainerStyle: CoreStyle {
    /// Defines which row in the grid the item should start and end at
    #[inline(always)]
    fn text_align(&self) -> TextAlign {
        Style::<Self::CustomIdent>::DEFAULT.text_align
    }

    /// How children of this block container are aligned in the block (cross) axis
    #[inline(always)]
    fn align_content(&self) -> AlignContent {
        Style::<Self::CustomIdent>::DEFAULT.align_content
    }

    /// The default inline-axis alignment of this block container's children (`justify-items`).
    /// Out-of-flow children with `justify-self: auto` are aligned within their static-position
    /// rectangle according to this value.
    #[inline(always)]
    fn justify_items(&self) -> AlignItems {
        Style::<Self::CustomIdent>::DEFAULT.justify_items
    }

    /// pyro: `margin-trim` (css-box-4 §3.3) in the block axis: whether the margins of the
    /// container's in-flow children that adjoin its block-start (`start`) and block-end (`end`)
    /// edges are truncated to zero.
    #[inline(always)]
    fn margin_trim(&self) -> Line<bool> {
        Line::FALSE
    }

    /// pyro: the edge of a line box in Taffy's inline axis (`Start`: left) that `text-align`
    /// puts the line's content against, or its middle (`Center`); `justify` is the start edge.
    /// An inline-level out-of-flow child of a block container has its static position there,
    /// beside the floats at its place.
    #[inline(always)]
    fn text_align_edge(&self) -> AxisStaticEdge {
        if self.direction().is_rtl() {
            AxisStaticEdge::End
        } else {
            AxisStaticEdge::Start
        }
    }
}

/// The set of styles required for a Block layout item (child of a Block container)
pub trait BlockItemStyle: CoreStyle {
    /// Whether the item is a table. Table children are handled specially in block layout.
    #[inline(always)]
    fn is_table(&self) -> bool {
        false
    }

    /// pyro: `min-width`/`min-height` when they are sizing keywords (`min-content`, `max-content`,
    /// `fit-content`, `fit-content(...)`, `stretch`), which `CoreStyle::min_size` cannot say (it
    /// says `auto` then); `auto` in an axis without one.
    #[inline(always)]
    fn min_size_keyword(&self) -> Size<Dimension> {
        Size { width: Dimension::auto(), height: Dimension::auto() }
    }

    /// pyro: `max-width`/`max-height` when they are sizing keywords (see
    /// [`Self::min_size_keyword`]).
    #[inline(always)]
    fn max_size_keyword(&self) -> Size<Dimension> {
        Size { width: Dimension::auto(), height: Dimension::auto() }
    }

    /// pyro: whether the item is a flex container whose lines break in the block axis (a
    /// wrapping column container). Its content height depends on its own height, so it is not
    /// measured for the automatic minimum height of a box with a preferred aspect ratio.
    #[inline(always)]
    fn breaks_lines_in_block_axis(&self) -> bool {
        false
    }

    /// pyro: whether the item's `display` was inline-level before `position` blockified it (an
    /// absolutely positioned box's): its static position in a block container is then that of
    /// the start of a line at its place ([`BlockContainerStyle::text_align_edge`]).
    #[inline(always)]
    fn original_display_is_inline(&self) -> bool {
        false
    }

    /// How an out-of-flow (absolutely positioned) child is aligned in the block axis of its
    /// static-position rectangle (`align-self`). `None` corresponds to `normal`/`auto`.
    #[inline(always)]
    fn align_self(&self) -> Option<AlignSelf> {
        None
    }

    /// How an out-of-flow (absolutely positioned) child is aligned in the inline axis of its
    /// static-position rectangle (`justify-self`). `None` corresponds to `normal`/`auto`.
    #[inline(always)]
    fn justify_self(&self) -> Option<AlignSelf> {
        None
    }

    /// The `align-content` of the item (if it is itself a block container). A non-`normal`
    /// value makes the item establish an independent formatting context, so that it is laid
    /// out next to (rather than underneath) floats and does not collapse margins with its
    /// parent (<https://www.w3.org/TR/css-align-3/#distribution-block>).
    #[inline(always)]
    fn align_content(&self) -> AlignContent {
        AlignContent::NORMAL
    }

    /// Whether the item is a floated
    #[cfg(feature = "float_layout")]
    #[inline(always)]
    fn float(&self) -> super::Float {
        super::Float::None
    }

    /// Whether the item is a floated
    #[cfg(feature = "float_layout")]
    #[inline(always)]
    fn clear(&self) -> super::Clear {
        super::Clear::None
    }
}

/// Used by block layout to implement the legacy behaviour of `<center>` and `<div align="left | right | center">`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum TextAlign {
    /// No special legacy text align behaviour.
    #[default]
    Auto,
    /// Corresponds to `-webkit-left` or `-moz-left` in browsers
    LegacyLeft,
    /// Corresponds to `-webkit-right` or `-moz-right` in browsers
    LegacyRight,
    /// Corresponds to `-webkit-center` or `-moz-center` in browsers
    LegacyCenter,
}

#[cfg(feature = "parse")]
crate::util::parse::impl_parse_for_keyword_enum!(TextAlign,
    "auto" => Auto,
    "-webkit-left" => LegacyLeft,
    "-webkit-right" => LegacyRight,
    "-webkit-center" => LegacyCenter,
);
