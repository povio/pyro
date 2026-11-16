//! Computes the CSS block layout algorithm in the case that the block container being laid out contains only block-level boxes
use crate::geometry::{FloatInsets, Line, Point, Rect, Size};
use crate::style::{
    AlignContent, AlignContentKeyword, AlignItems, AlignItemsKeyword, AlignSelf, AlignmentSafety, AvailableSpace,
    CoreStyle, LengthPercentageAuto, Overflow, Position,
};
use crate::style_helpers::TaffyMaxContent;
use crate::tree::{
    AxisStaticEdge, AxisStaticPosition, LayoutPartialTree, LayoutPartialTreeExt, NodeId, OofCandidate,
    OofCandidates, OofPositioningArea,
};
use crate::tree::{Baselines, CollapsibleMarginSet, Layout, LayoutInput, LayoutOutput, RunMode, SizingMode};
use crate::util::debug::debug_log;
use crate::util::sys::f32_max;
use crate::util::sys::Vec;
use crate::util::MaybeMath;
use crate::util::{MaybeResolve, ResolveOrZero};
use crate::{
    BlockContainerStyle, BlockItemStyle, BoxGenerationMode, BoxSizing, Contain, Dimension, Direction,
    LayoutBlockContainer, RequestedAxis, TextAlign,
};

#[cfg(feature = "float_layout")]
use super::float::{BfcSlot, ContentSlot, FloatContext, FloatIntrinsicWidthCalculator, FIT_TOLERANCE};
#[cfg(feature = "float_layout")]
use crate::{Clear, Float, FloatDirection};

/// Compute the block-axis offset that `align-content` applies to the in-flow content of a block
/// container, given the free space between the container's content box and its content.
///
/// The entire stack of in-flow content is treated as a single alignment subject, so the
/// distribution keywords (`space-*`, `stretch`) invoke their single-subject fallbacks. This is
/// used both by block layout and by inline formatting contexts (line boxes) which are laid out
/// externally to Taffy.
pub fn compute_block_align_content_offset(align_content: AlignContent, free_space: f32) -> f32 {
    let keyword = apply_alignment_fallback(free_space, 1, align_content);
    compute_alignment_offset(free_space, 1, 0.0, keyword, false, true)
}

/// Context for positioning Block and Float boxes within a Block Formatting Context
pub struct BlockFormattingContext {
    /// The float positioning context that handles positioning floats within this Block Formatting Context
    #[cfg(feature = "float_layout")]
    float_context: FloatContext,
}

impl Default for BlockFormattingContext {
    fn default() -> Self {
        Self {
            #[cfg(feature = "float_layout")]
            float_context: FloatContext::new(),
        }
    }
}

impl BlockFormattingContext {
    /// Create a new `BlockFormattingContext` with the specified width constraint
    pub fn new() -> Self {
        Default::default()
    }

    /// Create an initial `BlockContext` for this `BlockFormattingContext`
    pub fn root_block_context(&mut self) -> BlockContext<'_> {
        BlockContext {
            bfc: self,
            y_offset: 0.0,
            insets: FloatInsets::ZERO,
            content_box_insets: FloatInsets::ZERO,
            float_content_contribution: f32::NEG_INFINITY,
            is_root: true,
            #[cfg(feature = "float_layout")]
            adjoining_floats: [false, false],
            #[cfg(feature = "float_layout")]
            top_adjoining_floats: None,
        }
    }
}

/// A mark of how many floats a Block Formatting Context held when it was taken.
/// See [`BlockContext::float_mark`].
#[cfg(feature = "float_layout")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FloatMark {
    /// The number of left floats
    left: usize,
    /// The number of right floats
    right: usize,
}

/// Context for each individual Block within a Block Formatting Context
///
/// Contains a mutable reference to the BlockFormattingContext + block-specific data
pub struct BlockContext<'bfc> {
    /// A mutable reference to the root BlockFormatttingContext that this BlockContext belongs to
    bfc: &'bfc mut BlockFormattingContext,
    /// The y-offset of the border-top of the block node, relative to the to the border-top of the
    /// root node of the Block Formatting Context it belongs to.
    y_offset: f32,
    /// The x-inset of the border-box in from each side of the block node, relative to the root node of the Block Formatting Context it belongs to.
    insets: FloatInsets,
    /// The x-insets of the content box
    content_box_insets: FloatInsets,
    /// The height that floats take up in the element (`f32::NEG_INFINITY` if the element's
    /// subtree does not contain any floats)
    float_content_contribution: f32,
    /// Whether the node is the root of the Block Formatting Context is belongs to.
    is_root: bool,
    /// Whether a float has been placed (on each side) whose position adjoins the current
    /// margin-collapse strut of this block (i.e. whose final position can still be moved by
    /// margins that collapse into that strut). Such floats force clearance on cleared elements
    /// whose margins adjoin the same strut.
    #[cfg(feature = "float_layout")]
    adjoining_floats: [bool; 2],
    /// The value of `adjoining_floats` frozen at the first point at which in-flow content was
    /// committed within this block (resolving the position of the block's top margin strut).
    /// `None` if no in-flow content has been committed yet.
    #[cfg(feature = "float_layout")]
    top_adjoining_floats: Option<[bool; 2]>,
}

impl BlockContext<'_> {
    /// Create a sub-`BlockContext` for a child block node
    pub fn sub_context(&mut self, additional_y_offset: f32, insets: FloatInsets) -> BlockContext<'_> {
        let insets = self.insets + insets;
        BlockContext {
            bfc: self.bfc,
            y_offset: self.y_offset + additional_y_offset,
            insets,
            content_box_insets: insets,
            float_content_contribution: f32::NEG_INFINITY,
            is_root: false,
            // Floats adjoining the parent's current strut also adjoin this block's top strut
            // (if this block's top margin collapses with its first child's, which is checked separately)
            #[cfg(feature = "float_layout")]
            adjoining_floats: self.adjoining_floats,
            #[cfg(feature = "float_layout")]
            top_adjoining_floats: None,
        }
    }

    /// Returns whether this block is the root block of it's Block Formatting Context
    pub fn is_bfc_root(&self) -> bool {
        self.is_root
    }
}

#[cfg(feature = "float_layout")]
impl BlockContext<'_> {
    /// Set the width of the overall Block Formatting Context. This is used to resolve positions
    /// that are relative to the right of the context such as right-floated boxes.
    ///
    /// Sub-blocks within a Block Formatting Context should use the `Self::sub_context` method to create
    /// a sub-`BlockContext` with `insets` instead.
    pub fn set_width(&mut self, available_width: f32) {
        self.bfc.float_context.set_width(available_width);
    }

    /// Set the x-axis content-box insets of the `BlockContext`. These are the difference between the border-box
    /// and the content-box of the box (padding + border + scrollbar_gutter).
    pub fn apply_content_box_inset(&mut self, content_box_x_insets: FloatInsets) {
        self.content_box_insets = self.insets + content_box_x_insets;
    }

    /// Whether the float context contains any floats
    #[inline(always)]
    pub fn has_floats(&self) -> bool {
        self.bfc.float_context.has_floats()
    }

    /// Whether the float context contains any floats that extend to or below min_y
    #[inline(always)]
    pub fn has_active_floats(&self, min_y: f32) -> bool {
        self.bfc.float_context.has_active_floats(min_y + self.y_offset)
    }

    /// Position a floated box with the context. `id` identifies the box in [`Self::floats_placed_since`].
    pub fn place_floated_box(
        &mut self,
        id: u64,
        floated_box: Size<f32>,
        min_y: f32,
        direction: FloatDirection,
        clear: Clear,
        adjoins_unresolved_strut: bool,
    ) -> Point<f32> {
        if adjoins_unresolved_strut {
            self.adjoining_floats[direction as usize] = true;
        }
        let mut pos = self.bfc.float_context.place_floated_box(
            id,
            floated_box,
            min_y + self.y_offset,
            self.content_box_insets,
            direction,
            clear,
        );
        pos.y -= self.y_offset;
        pos.x -= self.insets.left;

        self.float_content_contribution = self.float_content_contribution.max(pos.y + floated_box.height);

        pos
    }

    /// pyro: put the margin box of an initial letter into the context (see
    /// [`FloatContext::place_initial_letter`]); `location` is its top-left corner relative to
    /// this block, as positions returned by [`Self::place_floated_box`] are.
    pub fn place_initial_letter(&mut self, location: Point<f32>, size: Size<f32>, direction: FloatDirection) {
        self.bfc.float_context.place_initial_letter(
            location.x + self.insets.left,
            location.y + self.y_offset,
            size,
            direction,
        );
        self.float_content_contribution = self.float_content_contribution.max(location.y + size.height);
    }

    /// pyro: the bottom of the lowest initial letter in the context, relative to this block
    /// (see [`FloatContext::initial_letter_clearance`]).
    pub fn initial_letter_clearance(&self) -> Option<f32> {
        self.bfc.float_context.initial_letter_clearance().map(|bottom| bottom - self.y_offset)
    }

    /// A mark of the floats placed in the Block Formatting Context so far, for [`Self::floats_placed_since`]
    pub fn float_mark(&self) -> FloatMark {
        let float_context = &self.bfc.float_context;
        FloatMark { left: float_context.left_floats().len(), right: float_context.right_floats().len() }
    }

    /// The id and position of each floated box placed since `mark` was taken: left floats in the
    /// order they were placed, then right floats. Positions are relative to this block, as
    /// returned by [`Self::place_floated_box`].
    pub fn floats_placed_since(&self, mark: FloatMark) -> impl Iterator<Item = (u64, Point<f32>)> + '_ {
        let float_context = &self.bfc.float_context;
        let left = float_context.left_floats()[mark.left..].iter().map(|placed| (placed, FloatDirection::Left));
        let right = float_context.right_floats()[mark.right..].iter().map(|placed| (placed, FloatDirection::Right));
        left.chain(right).map(move |(placed, direction)| {
            let pos = float_context.placed_position(placed, direction);
            (placed.id, Point { x: pos.x - self.insets.left, y: pos.y - self.y_offset })
        })
    }

    /// Search a space suitable for laying out non-floated content of the given height into
    pub fn find_content_slot(&self, min_y: f32, height: f32, clear: Clear, after: Option<usize>) -> ContentSlot {
        let mut slot = self.bfc.float_context.find_content_slot(
            min_y + self.y_offset,
            height,
            self.content_box_insets,
            clear,
            after,
        );
        slot.y -= self.y_offset;
        slot.x -= self.insets.left;
        slot
    }

    /// Search for a space suitable for laying out a box that establishes an independent
    /// formatting context (whose border box must not overlap floats)
    pub fn find_bfc_slot(
        &self,
        min_y: f32,
        min_height: f32,
        margins: FloatInsets,
        direction: Direction,
        clear: Clear,
        after: Option<usize>,
    ) -> BfcSlot {
        let mut slot = self.bfc.float_context.find_bfc_slot(
            min_y + self.y_offset,
            min_height,
            self.content_box_insets,
            margins,
            direction,
            clear,
            after,
        );
        slot.y -= self.y_offset;
        slot.x -= self.insets.left;
        slot
    }

    /// Get the bottom of lowest relevant float for the specific clear property
    pub fn cleared_threshold(&self, clear: Clear) -> Option<f32> {
        self.bfc.float_context.cleared_threshold(clear).map(|threshold| threshold - self.y_offset)
    }

    /// Whether a float that is adjoining the current margin-collapse strut has been placed
    /// on the side(s) relevant to the passed clear property
    pub fn has_adjoining_float(&self, clear: Clear) -> bool {
        match clear {
            Clear::Left => self.adjoining_floats[0],
            Clear::Right => self.adjoining_floats[1],
            Clear::Both => self.adjoining_floats[0] || self.adjoining_floats[1],
            Clear::None => false,
        }
    }

    /// Merge adjoining float flags propagated from a child block into this block's flags
    fn merge_adjoining_floats(&mut self, flags: [bool; 2]) {
        self.adjoining_floats[0] |= flags[0];
        self.adjoining_floats[1] |= flags[1];
    }

    /// Record that in-flow content has been committed within this block, resolving the position of
    /// the current margin-collapse strut. Floats placed before this point no longer adjoin the
    /// current strut. The flags for the block's top strut are frozen at the first commit.
    fn commit_strut(&mut self) {
        if self.top_adjoining_floats.is_none() {
            self.top_adjoining_floats = Some(self.adjoining_floats);
        }
        self.adjoining_floats = [false, false];
    }

    /// The adjoining float flags for this block's top margin strut: floats placed while the
    /// position of the block's top strut was still unresolved
    fn top_adjoining_floats(&self) -> [bool; 2] {
        self.top_adjoining_floats.unwrap_or(self.adjoining_floats)
    }

    /// Update the height that descendent floats consume with the height that floats consume
    /// within a particular child (`child_contribution` is relative to this block's border-top)
    pub fn add_child_floated_content_height_contribution(&mut self, child_contribution: f32) {
        self.float_content_contribution = self.float_content_contribution.max(child_contribution);
    }

    /// Returns the height that descendent floats consume
    pub fn floated_content_height_contribution(&self) -> f32 {
        self.float_content_contribution
    }
}

#[cfg(not(feature = "float_layout"))]
impl BlockContext<'_> {
    #[inline(always)]
    /// Returns the height that descendent floats consume (always 0.0 when the float feature is disabled)
    fn float_content_contribution(&self) -> f32 {
        0.0
    }
}

use super::common::alignment::{apply_alignment_fallback, child_first_baseline, compute_alignment_offset};
#[cfg(feature = "content_size")]
use super::common::scrollable_overflow::{
    compute_scrollable_overflow_contribution, finalize_scrollable_overflow, ScrollableOverflowContainer,
};
use super::common::sizing_keyword::{resolve_sizing_keyword, SizingKeywordResolution};

/// Per-child data that is accumulated and modified over the course of the layout algorithm
struct BlockItem {
    /// The identifier for the associated node
    node_id: NodeId,

    /// The "source order" of the item. This is the index of the item within the children iterator,
    /// and controls the order in which the nodes are placed
    order: u32,

    /// Items that are tables don't have stretch sizing applied to them
    is_table: bool,

    /// Compressible replaced items resolve an auto width to their intrinsic size rather than
    /// being stretch-sized <https://www.w3.org/TR/CSS22/visudet.html#block-replaced-width>
    is_compressible_replaced: bool,

    /// For compressible replaced items, the cap on their min-content contribution from a
    /// percentage `width` or `max-width` resolved against zero
    /// <https://drafts.csswg.org/css-sizing-3/#replaced-percentage-min-contribution>
    min_content_width_cap: Option<f32>,

    /// Whether the child is a non-independent block or inline node
    is_in_same_bfc: bool,

    #[cfg(feature = "float_layout")]
    /// The `float` style of the node
    float: Float,
    #[cfg(feature = "float_layout")]
    /// The `clear` style of the node
    clear: Clear,

    /// The size style of this item. Used to detect and resolve sizing keywords
    /// (`min-content`, `max-content`, `fit-content`, `fit-content(...)`, and `stretch`).
    /// In the height (block) axis only `stretch` requires resolution: the intrinsic sizing
    /// keywords are all equal to the content size, which is what an auto height already
    /// resolves to.
    size_style: Size<Dimension>,

    /// The base size of this item
    size: Size<Option<f32>>,
    /// pyro: the item's width is its height's through its preferred aspect ratio, with an
    /// automatic minimum: its min-content width, no larger than its `max-width` (css-sizing-4 §5.1).
    min_width_is_content: bool,
    /// pyro: the item's width is its height's through its preferred aspect ratio.
    width_from_ratio: bool,
    /// pyro: `min-width`/`min-height` and `max-width`/`max-height` that are sizing keywords
    /// (`BlockItemStyle::min_size_keyword`), which `min_size` and `max_size` do not hold.
    min_size_keyword: Size<Dimension>,
    max_size_keyword: Size<Dimension>,
    /// pyro: for an item that is no block container (a flex or grid container) and whose height is
    /// its width's through its preferred aspect ratio, with an automatic minimum: the ratio and
    /// what its box is smaller by than the border box. The item is as tall as its content when
    /// that is taller (no taller than its `max-height`); a block container sees to it itself.
    content_min_height_ratio: Option<(f32, Size<f32>)>,
    /// The minimum allowable size of this item
    min_size: Size<Option<f32>>,
    /// The maximum allowable size of this item
    max_size: Size<Option<f32>>,

    /// The overflow style of the item
    overflow: Point<Overflow>,
    /// The contain style of the item
    contain: Contain,
    /// The width of the item's scrollbars (if it has scrollbars)
    scrollbar_width: f32,

    /// The position style of the item
    position: Position,
    /// The `direction` style of the item (used to resolve `self-start`/`self-end`)
    direction: Direction,
    /// The `align-self` style of the item (used to derive the static position of out-of-flow items)
    align_self: Option<AlignSelf>,
    /// The `justify-self` style of the item (used to align in-flow items and to derive the static position of
    /// out-of-flow items)
    justify_self: Option<AlignSelf>,
    /// The final offset of this item
    inset: Rect<LengthPercentageAuto>,
    /// The margin of this item
    margin: Rect<LengthPercentageAuto>,
    /// The margin of this item
    padding: Rect<f32>,
    /// The margin of this item
    border: Rect<f32>,
    /// The sum of padding and border for this item
    padding_border_sum: Size<f32>,

    /// The computed border box size of this item
    computed_size: Size<f32>,
    /// The computed "static position" of this item. The static position is the position
    /// taking into account padding, border, margins, and scrollbar_gutters but not inset
    static_position: Point<f32>,
    /// pyro: the item is a block container (no replaced element): its last baseline is its own,
    /// none when it holds no line box.
    is_block_container: bool,
    /// pyro: the item is out of flow and was inline-level before `position` blockified it
    /// (`BlockItemStyle::original_display_is_inline`).
    is_inline_level_oof: bool,
    /// pyro: for such an item, the inline-axis point of its static position: where the content
    /// of a line starting at its place would sit (beside floats, by `text-align`).
    inline_static_x: Option<f32>,
    /// Whether margins can be collapsed through this item
    can_be_collapsed_through: bool,

    /// Pending layout for in-flow non-floated items. Held back from `set_unrounded_layout` so the
    /// post-loop `align-content` pass in `compute_inner` can shift `location.y` before commit.
    final_layout: Option<Layout>,

    /// Out-of-flow candidates bubbled out of this (in-flow) item's subtree. Anchors are relative
    /// to the item's border box until they are translated at merge time.
    oof_candidates: OofCandidates,
}

/// pyro: the sizes of a box with a preferred aspect ratio (css-sizing-4 §5), as outer sizes.
struct RatioSizes {
    /// `width`/`height`; the axis the ratio gives (from the other one, clamped by its own minimum
    /// and maximum first) is filled in.
    size: Size<Option<f32>>,
    /// `min-width`/`min-height`, and `max-width`/`max-height`. With both sizes `auto` the block
    /// axis' limits are transferred to the inline axis through the ratio: the minimum no larger
    /// than the inline maximum, the maximum no smaller than the inline minimum. Nothing is
    /// transferred into an axis whose size is given.
    min_size: Size<Option<f32>>,
    max_size: Size<Option<f32>>,
    /// The width is the height's through the ratio.
    width_from_ratio: bool,
    /// The height is the width's through the ratio (the used width's, when both are `auto`).
    height_from_ratio: bool,
}

/// pyro: see [`RatioSizes`]. `size`, `min_size` and `max_size` are the resolved style values, in
/// the box `box-sizing` names; `box_sizing_adjustment` makes them outer sizes; `ratio_inset` is what
/// the ratio's box is smaller by than that box (see [`aspect_ratio_inset`]).
fn resolve_sizes_with_aspect_ratio(
    mut size: Size<Option<f32>>,
    mut min_size: Size<Option<f32>>,
    mut max_size: Size<Option<f32>>,
    aspect_ratio: Option<f32>,
    ratio_inset: Size<f32>,
    box_sizing_adjustment: Size<f32>,
) -> RatioSizes {
    let mut width_from_ratio = false;
    let mut height_from_ratio = false;
    if let Some(ratio) = aspect_ratio {
        let to_height = |width: f32| (width - ratio_inset.width).max(0.0) / ratio + ratio_inset.height;
        let to_width = |height: f32| (height - ratio_inset.height).max(0.0) * ratio + ratio_inset.width;
        match (size.width, size.height) {
            (Some(width), None) => {
                size.height = Some(to_height(width.maybe_clamp(min_size.width, max_size.width)));
                height_from_ratio = true;
            }
            (None, Some(height)) => {
                size.width = Some(to_width(height.maybe_clamp(min_size.height, max_size.height)));
                width_from_ratio = true;
            }
            (None, None) => {
                let transferred_min = min_size.height.map(to_width).map(|min| min.maybe_min(max_size.width));
                let transferred_max = max_size.height.map(to_width).map(|max| max.maybe_max(min_size.width));
                min_size.width = match (min_size.width, transferred_min) {
                    (Some(own), Some(transferred)) => Some(own.max(transferred)),
                    (own, transferred) => own.or(transferred),
                };
                max_size.width = match (max_size.width, transferred_max) {
                    (Some(own), Some(transferred)) => Some(own.min(transferred)),
                    (own, transferred) => own.or(transferred),
                };
                height_from_ratio = true;
            }
            (Some(_), Some(_)) => {}
        }
    }
    RatioSizes {
        size: size.maybe_add(box_sizing_adjustment),
        min_size: min_size.maybe_add(box_sizing_adjustment),
        max_size: max_size.maybe_add(box_sizing_adjustment),
        width_from_ratio,
        height_from_ratio,
    }
}

/// pyro: what the box of a preferred aspect ratio is smaller by than the box `box-sizing` names:
/// nothing, but for `aspect-ratio: auto <ratio>` on a non-replaced box, whose ratio is its content
/// box's whatever `box-sizing` says (css-sizing-4 §5.1).
#[inline]
fn aspect_ratio_inset(style: &impl CoreStyle, padding_border_size: Size<f32>) -> Size<f32> {
    if style.aspect_ratio_of_content_box() && style.box_sizing() == BoxSizing::BorderBox {
        padding_border_size
    } else {
        Size::ZERO
    }
}

/// Computes the layout of [`LayoutPartialTree`] according to the block layout algorithm
pub fn compute_block_layout(
    tree: &mut impl LayoutBlockContainer,
    node_id: NodeId,
    inputs: LayoutInput,
    block_ctx: Option<&mut BlockContext<'_>>,
) -> LayoutOutput {
    let LayoutInput { known_dimensions, parent_size, run_mode, .. } = inputs;
    let style = tree.get_block_container_style(node_id);

    // Pull these out earlier to avoid borrowing issues
    let overflow = style.overflow();
    let is_scroll_container = overflow.x.is_scroll_container() || overflow.y.is_scroll_container();
    let contain = style.contain();
    // css-align-3 §5.1.1: a non-`normal` `align-content` makes a block container establish an
    // independent formatting context. <https://drafts.csswg.org/css-align-3/#distribution-block>
    // Layout and paint containment also establish an independent formatting context.
    // <https://drafts.csswg.org/css-contain-2/#containment-layout>
    let establishes_new_bfc = is_scroll_container
        || style.align_content().keyword != AlignContentKeyword::Normal
        || contain.establishes_independent_formatting_context();
    let aspect_ratio = style.aspect_ratio();
    let padding = style.padding().resolve_or_zero(parent_size.width, |val, basis| tree.calc(val, basis));
    let border = style.border().resolve_or_zero(parent_size.width, |val, basis| tree.calc(val, basis));
    let padding_border_size = (padding + border).sum_axes();
    let box_sizing_adjustment =
        if style.box_sizing() == BoxSizing::ContentBox { padding_border_size } else { Size::ZERO };

    // pyro: css-sizing-4 §5 ([`RatioSizes`])
    let sizes = resolve_sizes_with_aspect_ratio(
        style.size().maybe_resolve(parent_size, |val, basis| tree.calc(val, basis)),
        style.min_size().maybe_resolve(parent_size, |val, basis| tree.calc(val, basis)),
        style.max_size().maybe_resolve(parent_size, |val, basis| tree.calc(val, basis)),
        aspect_ratio,
        aspect_ratio_inset(&style, padding_border_size),
        box_sizing_adjustment,
    );
    let (min_size, max_size) = (sizes.min_size, sizes.max_size);
    let mut clamped_style_size = if inputs.sizing_mode == SizingMode::InherentSize {
        sizes.size.maybe_clamp(min_size, max_size)
    } else {
        Size::NONE
    };

    drop(style);

    // If both min and max in a given axis are set and max <= min then this determines the size in that axis
    let min_max_definite_size = min_size.zip_map(max_size, |min, max| match (min, max) {
        (Some(min), Some(max)) if max <= min => Some(min),
        _ => None,
    });

    // pyro: a height that is the width's through the aspect ratio is not known here: the box's
    // layout derives it from its used width, and its content can make it taller (the automatic
    // minimum size, css-sizing-4 §5.1).
    let height_follows_ratio = sizes.height_from_ratio
        && inputs.sizing_mode == SizingMode::InherentSize
        && known_dimensions.height.is_none()
        && min_max_definite_size.height.is_none();
    if height_follows_ratio {
        clamped_style_size.height = None;
    }

    let styled_based_known_dimensions =
        known_dimensions.or(min_max_definite_size).or(clamped_style_size).maybe_max(padding_border_size);

    // Short-circuit layout if the container's size is fully determined by the container's size and the run mode
    // is ComputeSize (and thus the container's size is all that we're interested in)
    if run_mode == RunMode::ComputeSize {
        if let Size { width: Some(width), height: Some(height) } = styled_based_known_dimensions {
            return LayoutOutput::from_outer_size(Size { width, height });
        }

        // We can also short-circuit if the width is known and only the width has been requested.
        if inputs.axis == RequestedAxis::Horizontal {
            if let Some(width) = styled_based_known_dimensions.width {
                return LayoutOutput::from_outer_size(Size { width, height: 0.0 });
            }
        }
    }

    // Unwrap the block formatting context if one was passed, or else create a new one
    debug_log!("BLOCK");
    let mut output = match block_ctx {
        Some(inherited_bfc) if !establishes_new_bfc => compute_inner(
            tree,
            node_id,
            LayoutInput { known_dimensions: styled_based_known_dimensions, ..inputs },
            height_follows_ratio,
            inherited_bfc,
        ),
        _ => compute_inner_in_new_bfc(
            tree,
            node_id,
            LayoutInput { known_dimensions: styled_based_known_dimensions, ..inputs },
            height_follows_ratio,
        ),
    };

    // Layout containment suppresses the box's baseline for baseline-alignment purposes
    if contain.suppresses_baseline() {
        output.baselines = Baselines::NONE;
    }

    output
}

/// [`compute_inner`] for a box that establishes a block formatting context.
// pyro: not inlined, so the formatting context is not part of the frame of
// `compute_block_layout` for the boxes that join their parent's (that frame is on the stack once
// per tree level of a recursive layout).
#[inline(never)]
fn compute_inner_in_new_bfc(
    tree: &mut impl LayoutBlockContainer,
    node_id: NodeId,
    inputs: LayoutInput,
    height_follows_ratio: bool,
) -> LayoutOutput {
    let mut root_bfc = BlockFormattingContext::new();
    let mut root_ctx = root_bfc.root_block_context();
    compute_inner(tree, node_id, inputs, height_follows_ratio, &mut root_ctx)
}

/// Computes the layout of [`LayoutBlockContainer`] according to the block layout algorithm
fn compute_inner(
    tree: &mut impl LayoutBlockContainer,
    node_id: NodeId,
    inputs: LayoutInput,
    // pyro: the height is the used width's through the preferred aspect ratio (css-sizing-4 §5).
    height_follows_ratio: bool,
    #[allow(unused_mut)] mut block_ctx: &mut BlockContext<'_>,
) -> LayoutOutput {
    let LayoutInput {
        known_dimensions, parent_size, available_space, run_mode, vertical_margins_are_collapsible, ..
    } = inputs;

    let style = tree.get_block_container_style(node_id);
    let raw_padding = style.padding();
    let raw_border = style.border();
    let raw_margin = style.margin();
    let aspect_ratio = style.aspect_ratio();
    let padding = raw_padding.resolve_or_zero(parent_size.width, |val, basis| tree.calc(val, basis));
    let border = raw_border.resolve_or_zero(parent_size.width, |val, basis| tree.calc(val, basis));
    // pyro: a box that is no containing block lays its children out in its containing block's
    // direction (`block_container_direction`).
    let direction = tree.block_container_direction(node_id).unwrap_or_else(|| style.direction());

    // Scrollbar gutters are reserved when the `overflow` property is set to `Overflow::Scroll`.
    // However, the axis are switched (transposed) because a node that scrolls vertically needs
    // *horizontal* space to be reserved for a scrollbar
    let scrollbar_gutter = {
        let offsets = style.overflow().transpose().map(|overflow| match overflow {
            Overflow::Scroll => style.scrollbar_width(),
            _ => 0.0,
        });
        match direction {
            Direction::Ltr => Rect { top: 0.0, left: 0.0, right: offsets.x, bottom: offsets.y },
            Direction::Rtl => Rect { top: 0.0, left: offsets.x, right: 0.0, bottom: offsets.y },
        }
    };
    let padding_border = padding + border;
    let padding_border_size = padding_border.sum_axes();
    let content_box_inset = padding_border + scrollbar_gutter;

    // Apply content box inset
    #[cfg(feature = "float_layout")]
    block_ctx.apply_content_box_inset(FloatInsets::new(content_box_inset.left, content_box_inset.right));

    let box_sizing_adjustment =
        if style.box_sizing() == BoxSizing::ContentBox { padding_border_size } else { Size::ZERO };
    // pyro: css-sizing-4 §5 ([`RatioSizes`])
    let ratio_inset = aspect_ratio_inset(&style, padding_border_size);
    let sizes = resolve_sizes_with_aspect_ratio(
        style.size().maybe_resolve(parent_size, |val, basis| tree.calc(val, basis)),
        style.min_size().maybe_resolve(parent_size, |val, basis| tree.calc(val, basis)),
        style.max_size().maybe_resolve(parent_size, |val, basis| tree.calc(val, basis)),
        aspect_ratio,
        ratio_inset,
        box_sizing_adjustment,
    );
    let (size, min_size, max_size) = (sizes.size, sizes.min_size, sizes.max_size);
    // pyro: the height the aspect ratio gives a border box of a width, between `min-height` and
    // `max-height`, when the height follows the ratio.
    let ratio_height = |outer_width: f32| -> Option<f32> {
        let ratio = aspect_ratio.filter(|_| height_follows_ratio)?;
        let ratio_box_inset = box_sizing_adjustment + ratio_inset;
        let height = (outer_width - ratio_box_inset.width).max(0.0) / ratio + ratio_box_inset.height;
        Some(height.maybe_clamp(min_size.height, max_size.height))
    };
    // pyro: the automatic minimum size in the block axis of a box whose height follows its ratio is
    // its content height (css-sizing-4 §5.1), unless it is a scroll container.
    let min_height_is_content = height_follows_ratio
        && style.min_size().height.is_auto()
        && !style.overflow().y.is_scroll_container()
        && !style.overflow().x.is_scroll_container();

    // css-sizing-4: a definite size in one axis transfers through `aspect-ratio`
    // to make the other definite. Deriving it from `known_dimensions` self-gates
    // the transfer — a block parent fills an axis only when it's a real
    // constraint (e.g. the stretched width at final layout) and leaves it None
    // while probing intrinsic sizes, so measure passes stay content-based. Only a
    // newly-filled axis is adopted (and clamped); an incoming known size is left
    // as the parent resolved it (re-clamping would undo padding/border overrides).
    let known_dimensions = if height_follows_ratio {
        // pyro: the height is settled once the content is laid out
        known_dimensions
    } else {
        let derived = known_dimensions.maybe_apply_aspect_ratio(aspect_ratio).maybe_clamp(min_size, max_size);
        Size { width: known_dimensions.width.or(derived.width), height: known_dimensions.height.or(derived.height) }
    };
    let percentage_basis_dimensions = Size {
        width: known_dimensions.width,
        height: known_dimensions
            .height
            .filter(|_| inputs.known_dimensions_are_definite.height)
            // pyro: percentages resolve against the height the ratio gives
            .or_else(|| known_dimensions.width.and_then(ratio_height)),
    };
    let container_content_box_size = percentage_basis_dimensions.maybe_sub(content_box_inset.sum_axes());

    let overflow = style.overflow();
    let is_scroll_container = overflow.x.is_scroll_container() || overflow.y.is_scroll_container();
    let establishes_new_bfc = is_scroll_container
        || style.align_content().keyword != AlignContentKeyword::Normal
        || style.contain().establishes_independent_formatting_context();

    // Determine margin collapsing behaviour
    //
    // Whether a child's margins could collapse through each of this box's block-axis edges: the
    // box does not establish a new BFC and has neither border nor padding on that side.
    let children_margins_adjoin_own_edges = Line {
        start: vertical_margins_are_collapsible.start
            && !establishes_new_bfc
            && !style.position().is_out_of_flow()
            && padding.top == 0.0
            && border.top == 0.0,
        end: vertical_margins_are_collapsible.end
            && !establishes_new_bfc
            && !style.position().is_out_of_flow()
            && padding.bottom == 0.0
            && border.bottom == 0.0,
    };
    let own_margins_collapse_with_children = Line {
        start: children_margins_adjoin_own_edges.start,
        end: children_margins_adjoin_own_edges.end && size.height.is_none(),
    };
    let has_styles_preventing_being_collapsed_through = !style.is_block()
        || block_ctx.is_bfc_root()
        || establishes_new_bfc
        || style.position().is_out_of_flow()
        || padding.top > 0.0
        || padding.bottom > 0.0
        || border.top > 0.0
        || border.bottom > 0.0;

    let text_align = style.text_align();
    // pyro: where `text-align` puts the content of a line (`BlockContainerStyle::text_align_edge`).
    let line_edge = style.text_align_edge();
    let align_content = style.align_content();
    let justify_items = style.justify_items();
    // pyro: `margin-trim`
    let margin_trim = style.margin_trim();
    drop(style);

    // 1. Generate items
    let mut items = generate_item_list(tree, node_id, container_content_box_size);

    // 2. Compute container width
    let container_outer_width = known_dimensions.width.unwrap_or_else(|| {
        let available_width = available_space.width.maybe_sub(content_box_inset.horizontal_axis_sum());
        // pyro: percentage heights in the children resolve against a definite height of the
        // container in this pass too, as at final layout (a float of `height: 100px` around a
        // `height: 100%` canvas is as wide as the canvas there; WPT css-sizing
        // `intrinsic-percent-replaced-*`).
        let percentage_height = percentage_basis_dimensions
            .height
            .or(if inputs.sizing_mode == SizingMode::InherentSize { size.height.maybe_max(min_size.height) } else { None })
            .or(if tree.block_container_passes_percentage_height(node_id) { parent_size.height } else { None })
            .maybe_sub(content_box_inset.vertical_axis_sum());
        let intrinsic_width = determine_content_based_container_width(tree, &items, available_width, percentage_height)
            + content_box_inset.horizontal_axis_sum();
        intrinsic_width.maybe_clamp(min_size.width, max_size.width).maybe_max(Some(padding_border_size.width))
    });

    // Short-circuit if computing size and both dimensions known
    if let (RunMode::ComputeSize, Some(container_outer_height)) = (run_mode, known_dimensions.height) {
        return LayoutOutput::from_outer_size(Size { width: container_outer_width, height: container_outer_height });
    }

    // We can also short-circuit if the width is known and only the width has been requested.
    if run_mode == RunMode::ComputeSize && inputs.axis == RequestedAxis::Horizontal {
        return LayoutOutput::from_outer_size(Size { width: container_outer_width, height: 0.0 });
    }

    // pyro: a width the content gave (a float's or an inline-block's shrink-to-fit width) is
    // the basis of the children's percentages from here on, as a given one is: their sizes
    // were resolved without it (`min-width: 200%` in a float did nothing; WPT css-sizing
    // `intrinsic-percent-non-replaced-*`).
    if container_content_box_size.width.is_none() {
        let inner_width = (container_outer_width - content_box_inset.horizontal_axis_sum()).max(0.0);
        items = generate_item_list(
            tree,
            node_id,
            Size { width: Some(inner_width), height: container_content_box_size.height },
        );
    }

    // Under `SizingMode::ContentSize` the container's own `height`/`min-height` are ignored,
    // so they cannot serve as the percentage basis for children either
    // (https://drafts.csswg.org/css-flexbox/#min-size-auto: the content size suggestion
    // must not be influenced by the item's specified height).
    let container_percentage_resolution_height = percentage_basis_dimensions
        .height
        // pyro
        .or_else(|| ratio_height(container_outer_width))
        .or(if inputs.sizing_mode == SizingMode::InherentSize { size.height.maybe_max(min_size.height) } else { None })
        // pyro: a box that is no containing block (an inline box laid out around its blocks)
        // hands its own containing block's height on.
        .or(if tree.block_container_passes_percentage_height(node_id) { parent_size.height } else { None })
        // pyro: a multi-column container's strip (the column height).
        .or_else(|| tree.block_container_percentage_height(node_id));

    // 3. Perform final item layout and return content height
    //
    // Percentage padding and borders resolve against the *containing block's* width
    // (`parent_size`), not the box's own width. These only differ when the box has a
    // non-stretch width. Fall back to the box's own width when the parent size is
    // unknown (e.g. at the root of the layout tree).
    let percentage_resolution_width = parent_size.width.unwrap_or(container_outer_width);
    let resolved_padding =
        raw_padding.resolve_or_zero(Some(percentage_resolution_width), |val, basis| tree.calc(val, basis));
    let resolved_border =
        raw_border.resolve_or_zero(Some(percentage_resolution_width), |val, basis| tree.calc(val, basis));
    let resolved_content_box_inset = resolved_padding + resolved_border + scrollbar_gutter;
    #[cfg_attr(not(feature = "content_size"), allow(unused_mut))]
    let (
        mut inflow_overflow_rect,
        mut intrinsic_outer_height,
        first_child_top_margin_set,
        last_child_bottom_margin_set,
        mut first_baseline,
        mut last_baseline,
    ) = perform_final_layout_on_in_flow_children(
        tree,
        run_mode,
        &mut items,
        container_outer_width,
        container_percentage_resolution_height,
        content_box_inset,
        resolved_content_box_inset,
        resolved_border,
        #[cfg(feature = "content_size")]
        resolved_padding,
        text_align,
        line_edge,
        justify_items,
        direction,
        own_margins_collapse_with_children,
        children_margins_adjoin_own_edges,
        margin_trim,
        #[cfg(feature = "content_size")]
        is_scroll_container,
        block_ctx,
    );

    // Root BFCs contain floats: the content height extends to the bottom margin edge of any
    // floated descendant, and the container's bottom padding and border sit below that
    #[cfg(feature = "float_layout")]
    if block_ctx.is_bfc_root() || establishes_new_bfc {
        intrinsic_outer_height = intrinsic_outer_height
            .max(block_ctx.floated_content_height_contribution() + resolved_content_box_inset.bottom);
    }

    let container_outer_height = known_dimensions
        .height
        .unwrap_or_else(|| match ratio_height(container_outer_width) {
            // pyro: the ratio's height, or the content's when that is taller and `min-height` is
            // `auto` (never past `max-height`).
            Some(height) if min_height_is_content => {
                height.max(intrinsic_outer_height.maybe_min(max_size.height))
            }
            Some(height) => height,
            None => intrinsic_outer_height.maybe_clamp(min_size.height, max_size.height),
        })
        .maybe_max(Some(padding_border_size.height));
    let final_outer_size = Size { width: container_outer_width, height: container_outer_height };

    // CSS2 §8.3.1: the bottom margin of a block with `height: auto` collapses with its last
    // in-flow child's bottom margin only if the box's `min-height` is less than the box's
    // used height. When `min-height` determines the used height, the last child's bottom
    // margin no longer adjoins the box's bottom edge, so it stays inside the box instead of
    // collapsing with the box's own bottom margin. (`max-height` has no such effect.)
    let height_constrained_by_min_height = matches!(min_size.height, Some(h) if h > 0.0 && h >= container_outer_height);
    let own_bottom_margin_collapses_with_children =
        own_margins_collapse_with_children.end && !height_constrained_by_min_height;

    // Apply `align-content` to in-flow non-floated items if requested. The per-item layouts were
    // held back in `item.final_layout` so that this step can shift `location.y` before tree commit.
    //
    // For block layout the entire stack of in-flow children is treated as a single alignment
    // subject. That means distribution keywords (`space-between`, `space-around`,
    // `space-evenly`, `stretch`) must invoke the single-subject fallback unconditionally —
    // which is what passing `num_items = 1` to `apply_alignment_fallback` does. The whole
    // group then shifts by one offset, with zero inter-item gap.
    if align_content.keyword != AlignContentKeyword::Normal {
        let container_inner_height = container_outer_height - resolved_content_box_inset.vertical_axis_sum();
        let inflow_content_height = intrinsic_outer_height - resolved_content_box_inset.vertical_axis_sum();
        let free_space = container_inner_height - inflow_content_height;
        let group_offset = compute_block_align_content_offset(align_content, free_space);
        first_baseline = first_baseline.map(|baseline| baseline + group_offset);
        last_baseline = last_baseline.map(|baseline| baseline + group_offset);
        for item in items.iter_mut() {
            if let Some(layout) = item.final_layout.as_mut() {
                layout.location.y += group_offset;
            }
            // The static positions of out-of-flow children move with the in-flow content
            if item.position.is_out_of_flow() {
                item.static_position.y += group_offset;
            }
        }
        let any_in_flow = items.iter().any(|item| item.final_layout.is_some());
        if any_in_flow {
            #[cfg(feature = "content_size")]
            {
                inflow_overflow_rect = Rect::ZERO;
                let overflow_container = ScrollableOverflowContainer {
                    parent_is_scroll_container: is_scroll_container,
                    parent_size: final_outer_size - resolved_border.sum_axes() - scrollbar_gutter.sum_axes(),
                    reversed: Point { x: direction.is_rtl(), y: false },
                    padding: resolved_padding,
                };
                for item in items.iter() {
                    if let Some(layout) = item.final_layout.as_ref() {
                        let contribution_location = Point {
                            x: layout.location.x - resolved_border.left - scrollbar_gutter.left,
                            y: layout.location.y - resolved_border.top,
                        };
                        inflow_overflow_rect = inflow_overflow_rect.union(compute_scrollable_overflow_contribution(
                            contribution_location,
                            layout.size,
                            layout.scrollable_overflow_rect.rect(),
                            layout.border,
                            item.overflow,
                            item.contain,
                            &overflow_container,
                            layout.margin.map(|value| value.min(0.0)),
                        ));
                    }
                }
            }
        }
    }

    // Determine whether this node can be collapsed through
    let all_in_flow_children_can_be_collapsed_through = items.iter().all(|item| {
        #[cfg(feature = "float_layout")]
        if item.float.is_floated() {
            return true;
        }
        item.position.is_out_of_flow() || item.can_be_collapsed_through
    });
    // CSS2 §8.3.1: a box's top and bottom margins collapse through it only if its *used* height
    // is zero, so `height`/`min-height`/`max-height` are judged by the height they resolve to
    // (e.g. `height: 1px; max-height: 0` collapses, `min-height: 1%` under an auto-height
    // parent collapses) rather than by their computed values.
    let can_be_collapsed_through = !has_styles_preventing_being_collapsed_through
        && container_outer_height == 0.0
        && all_in_flow_children_can_be_collapsed_through;

    let mut output = LayoutOutput {
        size: final_outer_size,
        #[cfg(feature = "content_size")]
        scrollable_overflow_rect: crate::tree::ScrollableOverflow::ZERO,
        baselines: Baselines { first: first_baseline, last: last_baseline },
        top_margin: if own_margins_collapse_with_children.start {
            first_child_top_margin_set
        } else {
            let margin_top = raw_margin.top.resolve_or_zero(parent_size.width, |val, basis| tree.calc(val, basis));
            CollapsibleMarginSet::from_margin(margin_top)
        },
        bottom_margin: if own_bottom_margin_collapses_with_children {
            last_child_bottom_margin_set
        } else {
            let margin_bottom =
                raw_margin.bottom.resolve_or_zero(parent_size.width, |val, basis| tree.calc(val, basis));
            CollapsibleMarginSet::from_margin(margin_bottom)
        },
        margins_can_collapse_through: can_be_collapsed_through,
        oof_candidates: OofCandidates::NONE,
        oof_positioning_area: None,
    };

    // Short-circuit if computing size.
    //
    // Note: it is important that we return the margin-collapsing related outputs here as Parent block containers
    // rely on the `top_margin`/`bottom_margin` of their children to compute their own intrinsic height.
    if run_mode == RunMode::ComputeSize {
        return output;
    }

    // Commit deferred child layouts to the tree.
    for item in items.iter() {
        if let Some(layout) = item.final_layout.as_ref() {
            tree.set_unrounded_layout(item.node_id, layout);
        }
    }

    // pyro: whether in-flow boxes overlap in the block axis (`set_block_flow_overlap`).
    {
        let mut overlap = false;
        let mut reach = f32::NEG_INFINITY;
        let mut after_overflow = false;
        for item in items.iter() {
            let Some(layout) = item.final_layout.as_ref() else { continue };
            if layout.size.height > 0.0 && (after_overflow || layout.location.y + 0.01 < reach) {
                overlap = true;
            }
            reach = reach.max(layout.location.y + layout.size.height);
            let (child_overlap, child_overflows) = tree.block_child_flow_overlap(item.node_id);
            overlap |= child_overlap;
            after_overflow |= child_overflows;
        }
        tree.set_block_flow_overlap(node_id, overlap, intrinsic_outer_height > container_outer_height + 0.01);
    }

    // 4. Collect out-of-flow candidates in document order (direct out-of-flow children plus
    // candidates bubbled from in-flow children's subtrees). These are laid out by the
    // out-of-flow positioning pass (`compute_oof_layout`), which runs after this algorithm.
    let mut candidates = OofCandidates::new();
    for item in items.iter_mut() {
        if item.position.is_out_of_flow() {
            // The static-position rectangle of an out-of-flow child of a block container spans
            // the container's content box in the inline axis, and is a zero-height line at the
            // child's hypothetical position in the block axis. The child's self-alignment
            // properties determine how it is aligned within that rectangle.
            // <https://www.w3.org/TR/css-position-3/#staticpos-rect>
            // pyro: an inline-level one's is a point on the line at its place.
            let inline_area = match item.inline_static_x {
                Some(x) => Line { start: x, end: x },
                None => Line {
                    start: resolved_content_box_inset.left,
                    end: container_outer_width - resolved_content_box_inset.right,
                },
            };
            let block_area = Line { start: item.static_position.y, end: item.static_position.y };
            // A `justify_self` of `None` takes the container's `justify-items`. Block containers have
            // no `align-items`, so an `align_self` of `None` behaves as `normal`.
            let justify_self =
                item.justify_self.unwrap_or(justify_items).resolve_self_relative(item.direction, direction, true);
            let align_self =
                item.align_self.unwrap_or(AlignItems::NORMAL).resolve_self_relative(item.direction, direction, false);
            candidates.push(OofCandidate { containing_block: None,
                node: item.node_id,
                order: item.order,
                position: item.position,
                static_position: Point {
                    x: AxisStaticPosition::from_alignment(justify_self, inline_area, direction.is_rtl()),
                    y: AxisStaticPosition::from_alignment(align_self, block_area, false),
                },
            });
        } else if !item.oof_candidates.is_empty() {
            // Translate anchors from item-relative to container-relative coordinates
            if let Some(layout) = item.final_layout.as_ref() {
                item.oof_candidates.translate(layout.location);
            }
            candidates.append(&mut item.oof_candidates);
        }
    }

    let absolute_position_inset = resolved_border + scrollbar_gutter;
    let absolute_position_area = final_outer_size - absolute_position_inset.sum_axes();
    let absolute_position_offset = Point { x: absolute_position_inset.left, y: absolute_position_inset.top };
    output.oof_candidates = candidates;
    output.oof_positioning_area =
        Some(OofPositioningArea { size: absolute_position_area, offset: absolute_position_offset });

    #[cfg(feature = "content_size")]
    {
        let client_width =
            (final_outer_size.width - resolved_border.horizontal_axis_sum() - scrollbar_gutter.horizontal_axis_sum())
                .max(0.0);
        if direction.is_rtl() {
            inflow_overflow_rect = inflow_overflow_rect.union(Rect { right: client_width, ..Rect::ZERO });
        }
        if is_scroll_container {
            inflow_overflow_rect = inflow_overflow_rect.union(Rect {
                left: if direction.is_rtl() { (client_width - resolved_padding.left).min(0.0) } else { 0.0 },
                right: if direction.is_rtl() { client_width } else { resolved_padding.right },
                top: 0.0,
                bottom: resolved_padding.bottom,
            });
        }
        output.scrollable_overflow_rect = finalize_scrollable_overflow(
            inflow_overflow_rect,
            final_outer_size - resolved_border.sum_axes() - scrollbar_gutter.sum_axes(),
            Point { x: direction.is_rtl(), y: false },
            is_scroll_container,
        );
    }

    // 5. Perform hidden layout on hidden children
    let len = tree.child_count(node_id);
    for order in 0..len {
        let child = tree.get_child_id(node_id, order);
        let child_style = tree.get_block_child_style(child);
        if child_style.box_generation_mode() == BoxGenerationMode::None {
            drop(child_style);
            tree.set_unrounded_layout(child, &Layout::with_order(order as u32));
            tree.perform_child_layout(
                child,
                Size::NONE,
                Size::NONE,
                Size::MAX_CONTENT,
                SizingMode::InherentSize,
                Line::FALSE,
            );
        }
    }

    output
}

/// Create a `Vec` of `BlockItem` structs where each item in the `Vec` represents a child of the current node
// pyro: not inlined, so its locals stay out of `compute_inner`'s frame, which is on the
// stack once per tree level of a recursive layout.
#[inline(never)]
fn generate_item_list(
    tree: &impl LayoutBlockContainer,
    node: NodeId,
    node_inner_size: Size<Option<f32>>,
) -> Vec<BlockItem> {
    tree.child_ids(node)
        .map(|child_node_id| (child_node_id, tree.get_block_child_style(child_node_id)))
        .filter(|(_, style)| style.box_generation_mode() != BoxGenerationMode::None)
        .enumerate()
        .map(|(order, (child_node_id, child_style))| {
            let aspect_ratio = child_style.aspect_ratio();
            let padding =
                child_style.padding().resolve_or_zero(node_inner_size.width, |val, basis| tree.calc(val, basis));
            let border =
                child_style.border().resolve_or_zero(node_inner_size.width, |val, basis| tree.calc(val, basis));
            let pb_sum = (padding + border).sum_axes();
            let box_sizing_adjustment =
                if child_style.box_sizing() == BoxSizing::ContentBox { pb_sum } else { Size::ZERO };

            let position = child_style.position();
            let overflow = child_style.overflow();

            #[cfg(feature = "float_layout")]
            let float = child_style.float();
            #[cfg(feature = "float_layout")]
            let is_not_floated = float == Float::None;

            #[cfg(not(feature = "float_layout"))]
            let is_not_floated = true;

            // pyro: css-sizing-4 §5 ([`RatioSizes`])
            let sizes = resolve_sizes_with_aspect_ratio(
                child_style.size().maybe_resolve(node_inner_size, |val, basis| tree.calc(val, basis)),
                child_style.min_size().maybe_resolve(node_inner_size, |val, basis| tree.calc(val, basis)),
                child_style.max_size().maybe_resolve(node_inner_size, |val, basis| tree.calc(val, basis)),
                aspect_ratio,
                aspect_ratio_inset(&child_style, pb_sum),
                box_sizing_adjustment,
            );
            let min_width_is_content = sizes.width_from_ratio
                && child_style.min_size().width.is_auto()
                && !child_style.is_compressible_replaced()
                && !overflow.x.is_scroll_container()
                && !overflow.y.is_scroll_container();

            let is_block = child_style.is_block();
            let is_table = child_style.is_table();
            // pyro: a tree may keep a block-level child at its content width
            // (`block_child_width_fits_content`), as for compressible replaced elements.
            let is_compressible_replaced =
                child_style.is_compressible_replaced() || tree.block_child_width_fits_content(child_node_id);
            let min_content_width_cap = if child_style.is_compressible_replaced() {
                let calc = |val, basis| tree.calc(val, basis);
                let width = child_style.size().width.maybe_resolve(Some(0.0), calc);
                let max_width = child_style.max_size().width.maybe_resolve(Some(0.0), calc);
                width.map_or(max_width, |width| Some(width.maybe_min(max_width))).maybe_add(box_sizing_adjustment.width)
            } else {
                None
            };
            let is_scroll_container = overflow.x.is_scroll_container() || overflow.y.is_scroll_container();
            let contain = child_style.contain();

            let is_in_same_bfc: bool = is_block
                && !is_table
                && !position.is_out_of_flow()
                && is_not_floated
                && !is_scroll_container
                && !contain.establishes_independent_formatting_context()
                && child_style.align_content().keyword == AlignContentKeyword::Normal
                // pyro
                && !tree.block_child_establishes_formatting_context(child_node_id);

            BlockItem {
                node_id: child_node_id,
                order: order as u32,
                is_table,
                is_compressible_replaced,
                min_content_width_cap,
                // pyro: a block-level replaced element (or form control) keeps its border box clear
                // of floats, as a table or a new formatting context does (CSS2 §9.5; WPT `floats-039`).
                is_in_same_bfc: is_in_same_bfc && !is_compressible_replaced,
                #[cfg(feature = "float_layout")]
                float,
                #[cfg(feature = "float_layout")]
                clear: child_style.clear(),
                size_style: child_style.size(),
                // pyro: a height the aspect ratio gives is the item's own layout's to settle
                // (its content can make it taller): it is not handed down as known.
                size: if sizes.height_from_ratio && is_block && !is_table {
                    Size { width: sizes.size.width, height: None }
                } else {
                    sizes.size
                },
                min_size: sizes.min_size,
                max_size: sizes.max_size,
                min_width_is_content,
                width_from_ratio: sizes.width_from_ratio,
                min_size_keyword: child_style.min_size_keyword(),
                max_size_keyword: child_style.max_size_keyword(),
                content_min_height_ratio: aspect_ratio.filter(|_| {
                    sizes.height_from_ratio
                        && !is_block
                        && !is_table
                        && !child_style.is_compressible_replaced()
                        && !child_style.breaks_lines_in_block_axis()
                        && child_style.min_size().height.is_auto()
                        && !overflow.x.is_scroll_container()
                        && !overflow.y.is_scroll_container()
                })
                .map(|ratio| (ratio, box_sizing_adjustment + aspect_ratio_inset(&child_style, pb_sum))),
                overflow,
                contain,
                scrollbar_width: child_style.scrollbar_width(),
                position,
                direction: child_style.direction(),
                align_self: child_style.align_self(),
                justify_self: child_style.justify_self(),
                inset: child_style.inset(),
                margin: child_style.margin(),
                padding,
                border,
                padding_border_sum: pb_sum,

                // Fields to be computed later (for now we initialise with dummy values)
                computed_size: Size::zero(),
                static_position: Point::zero(),
                is_block_container: is_block && !child_style.is_compressible_replaced(),
                is_inline_level_oof: position.is_out_of_flow() && child_style.original_display_is_inline(),
                inline_static_x: None,
                can_be_collapsed_through: false,
                final_layout: None,
                oof_candidates: OofCandidates::new(),
            }
        })
        .collect()
}

/// Resolve the `stretch` sizing keyword for an item's height style. In the block axis the
/// intrinsic sizing keywords (`min-content`, `max-content`, `fit-content`, `fit-content(...)`)
/// are all equal to the content size, which is what an auto height already resolves to, so
/// only `stretch` requires explicit resolution.
///
/// Each of the item's block-axis margins is subtracted from the stretch size unless it is
/// flagged in `margins_treated_as_zero`: when stretching a block-level box, a margin that
/// could collapse through the corresponding edge of the containing block (which has neither
/// border nor padding on that side and does not establish a new BFC) is treated as zero.
/// <https://github.com/w3c/csswg-drafts/issues/11044#issuecomment-2599101601>
/// <https://github.com/w3c/csswg-drafts/issues/13260>
#[inline]
fn resolve_stretch_height(
    height_style: Dimension,
    container_inner_height: Option<f32>,
    item_margin: Rect<f32>,
    margins_treated_as_zero: Line<bool>,
) -> Option<f32> {
    if !height_style.is_stretch() {
        return None;
    }
    let mut height = container_inner_height?;
    if !margins_treated_as_zero.start {
        height -= item_margin.top;
    }
    if !margins_treated_as_zero.end {
        height -= item_margin.bottom;
    }
    Some(height)
}

/// Compute the offset applied to a box by its `inset` if it is `position: relative` (CSS2.2 9.4.3).
///
/// Percentages resolve against the containing block's content box (`parent_size`); a percentage
/// that cannot be resolved behaves as `auto`. If both insets in an axis are non-auto, `top` wins
/// over `bottom`, and `left` wins over `right` in LTR (`right` over `left` in RTL).
#[inline]
fn resolve_relative_offset(
    item: &BlockItem,
    parent_size: Size<Option<f32>>,
    direction: Direction,
    tree: &impl LayoutPartialTree,
) -> Point<f32> {
    if item.position != Position::Relative {
        return Point::ZERO;
    }
    let inset = item.inset.zip_size(parent_size, |p, s| p.maybe_resolve(s, |val, basis| tree.calc(val, basis)));
    let x = if direction.is_rtl() {
        inset.right.map(|x| -x).or(inset.left)
    } else {
        inset.left.or(inset.right.map(|x| -x))
    };
    Point { x: x.unwrap_or(0.0), y: inset.top.or(inset.bottom.map(|x| -x)).unwrap_or(0.0) }
}

/// Compute the content-based width in the case that the width of the container is not known
// pyro: not inlined (see `generate_item_list`).
#[inline(never)]
fn determine_content_based_container_width(
    tree: &mut impl LayoutPartialTree,
    items: &[BlockItem],
    available_width: AvailableSpace,
    percentage_height: Option<f32>,
) -> f32 {
    let available_space = Size { width: available_width, height: AvailableSpace::MinContent };
    let parent_size = Size { width: None, height: percentage_height };

    let mut max_child_width = 0.0;
    #[cfg(feature = "float_layout")]
    let mut float_contribution = FloatIntrinsicWidthCalculator::new(available_width);
    for item in items.iter().filter(|item| !item.position.is_out_of_flow()) {
        let mut known_dimensions = item.size.maybe_clamp(item.min_size, item.max_size);
        // pyro
        if let (true, Some(width)) = (item.min_width_is_content, known_dimensions.width) {
            known_dimensions.width = Some(width.max(ratio_min_content_width(tree, item, Size::NONE)));
        }
        // pyro: `min-width`/`max-width` that are sizing keywords (`fit-content` is the content's
        // width under this pass's own constraint)
        if let Some(width) = known_dimensions.width {
            let available = match available_space.width {
                AvailableSpace::Definite(available) => Some(available),
                AvailableSpace::MaxContent => Some(f32::INFINITY),
                AvailableSpace::MinContent => Some(0.0),
            };
            known_dimensions.width = Some(clamp_width_by_keywords(tree, item, width, available, None, Size::NONE));
        }

        // pyro: in an intrinsic size a percentage resolves against zero, also inside `calc()`:
        // `calc(10% + 100px)` counts 100px (upstream dropped the whole margin; WPT
        // `calc-margins-block`).
        let item_x_margin_sum = item
            .margin
            .resolve_or_zero(available_space.width.into_option().or(Some(0.0)), |val, basis| tree.calc(val, basis))
            .horizontal_axis_sum();
        let width = known_dimensions.width.unwrap_or_else(|| {
            let item_available_width =
                match resolve_sizing_keyword(item.size_style.width, None, None, |val, basis| tree.calc(val, basis)) {
                    Some(SizingKeywordResolution::Measure(available_width)) => available_width,
                    Some(SizingKeywordResolution::Exact(width)) => AvailableSpace::Definite(width),
                    None => available_space.width.maybe_sub(item_x_margin_sum),
                };
            let width = tree.measure_child_size(
                item.node_id,
                known_dimensions,
                parent_size,
                Size { width: item_available_width, height: available_space.height },
                SizingMode::InherentSize,
                crate::AbsoluteAxis::Horizontal,
                // Must match the value passed when laying the item out (see `Cache`)
                if item.is_in_same_bfc { Line::TRUE } else { Line::FALSE },
            );
            // The cap is the item's min-content contribution, so its fit-content contribution is
            // min(max-content, max(cap, available))
            match (item.min_content_width_cap, item_available_width) {
                (Some(cap), AvailableSpace::MinContent) => width.min(cap).maybe_max(item.min_size.width),
                (Some(cap), AvailableSpace::Definite(available)) => {
                    width.min(cap.max(available)).maybe_max(item.min_size.width)
                }
                _ => width,
            }
        });

        // pyro: a table is never narrower than its min-content width, whatever its `width` or
        // `max-width` (CSS Tables 3 §3.9.2): its contribution is at least that.
        let width = if item.is_table && (item.size.width.is_some() || item.max_size.width.is_some()) {
            let content = tree.measure_child_size(
                item.node_id,
                Size::NONE,
                Size::NONE,
                Size { width: AvailableSpace::MinContent, height: AvailableSpace::MaxContent },
                SizingMode::ContentSize,
                crate::AbsoluteAxis::Horizontal,
                if item.is_in_same_bfc { Line::TRUE } else { Line::FALSE },
            );
            width.max(content)
        } else {
            width
        };
        let width = f32_max(width, item.padding_border_sum.width) + item_x_margin_sum;

        #[cfg(feature = "float_layout")]
        if let Some(direction) = item.float.float_direction() {
            float_contribution.add_float(width, direction, item.clear);
            continue;
        }

        max_child_width = f32_max(max_child_width, width);
    }

    #[cfg(feature = "float_layout")]
    {
        max_child_width = max_child_width.max(float_contribution.result());
    }

    max_child_width
}

/// pyro: the automatic minimum width of an item whose width is its height's through its aspect
/// ratio (`BlockItem::min_width_is_content`): its min-content width, no larger than its `max-width`.
fn ratio_min_content_width(
    tree: &mut impl LayoutPartialTree,
    item: &BlockItem,
    parent_size: Size<Option<f32>>,
) -> f32 {
    let width = tree.measure_child_size(
        item.node_id,
        Size::NONE,
        parent_size,
        Size { width: AvailableSpace::MinContent, height: AvailableSpace::MaxContent },
        // The content's width: the item's own `width` and `height` (and so its ratio) aside
        SizingMode::ContentSize,
        crate::AbsoluteAxis::Horizontal,
        if item.is_in_same_bfc { Line::TRUE } else { Line::FALSE },
    );
    width.maybe_min(item.max_size.width)
}

/// pyro: a width between the item's `min-width` and `max-width` when they are sizing keywords
/// (`min-content`, `max-content`, `fit-content`, `stretch`): each is the item's content width
/// under that constraint, or its stretch width. `stretch_width` and `container_width`: what
/// `fit-content`/`stretch` and `fit-content(<percentage>)` resolve against (`None` in an
/// intrinsic sizing pass, where those keywords do nothing).
fn clamp_width_by_keywords(
    tree: &mut impl LayoutPartialTree,
    item: &BlockItem,
    width: f32,
    stretch_width: Option<f32>,
    container_width: Option<f32>,
    parent_size: Size<Option<f32>>,
) -> f32 {
    if item.min_size_keyword.width.is_auto() && item.max_size_keyword.width.is_auto() {
        return width;
    }
    let mut resolve = |keyword: Dimension| -> Option<f32> {
        let resolution = resolve_sizing_keyword(keyword, stretch_width, container_width, |val, basis| tree.calc(val, basis))?;
        Some(match resolution {
            SizingKeywordResolution::Exact(width) => width,
            SizingKeywordResolution::Measure(available_width) => {
                let mut measure = |available_width: AvailableSpace| {
                    tree.measure_child_size(
                        item.node_id,
                        // (its height stays: percentage heights in it resolve as they will)
                        Size { width: None, height: item.size.height.maybe_clamp(item.min_size.height, item.max_size.height) },
                        parent_size,
                        Size { width: available_width, height: AvailableSpace::MaxContent },
                        // The content's width, the item's own `width` aside
                        SizingMode::ContentSize,
                        crate::AbsoluteAxis::Horizontal,
                        if item.is_in_same_bfc { Line::TRUE } else { Line::FALSE },
                    )
                };
                let content_width = match available_width {
                    // `fit-content` (and `fit-content(<length-percentage>)`): the max-content
                    // width, at most the available width but at least the min-content width (CSS
                    // Sizing 3 §5.1), from the two intrinsic widths. A measure under the definite
                    // width is not that for every box: a grid container with `fr` columns
                    // measured its max-content width there, and `min-width: fit-content` (BBC
                    // News' grids) made it as wide as its `max-width`.
                    AvailableSpace::Definite(available) => {
                        let min_content = measure(AvailableSpace::MinContent);
                        let max_content = measure(AvailableSpace::MaxContent);
                        max_content.min(min_content.max(available))
                    }
                    intrinsic => measure(intrinsic),
                };
                // A width the preferred aspect ratio gives from a height is an intrinsic width
                if item.width_from_ratio {
                    content_width.max(width)
                } else {
                    content_width
                }
            }
        })
    };
    let max = resolve(item.max_size_keyword.width);
    let min = resolve(item.min_size_keyword.width);
    width.maybe_min(max).maybe_max(min)
}

/// Resolve the known dimensions an in-flow block-level item is laid out with
#[allow(clippy::too_many_arguments)]
fn block_item_known_dimensions(
    tree: &mut impl LayoutBlockContainer,
    item: &BlockItem,
    stretch_width: f32,
    container_inner_width: f32,
    container_percentage_resolution_height: Option<f32>,
    item_non_auto_margin: Rect<f32>,
    children_margins_adjoin_own_edges: Line<bool>,
    parent_size: Size<Option<f32>>,
    is_stretch: bool,
    is_fit_content: bool,
) -> Size<Option<f32>> {
    // Unless stretched, tables and compressible replaced elements resolve their own width
    // <https://www.w3.org/TR/CSS22/visudet.html#block-replaced-width>
    if (item.is_table || item.is_compressible_replaced) && !is_stretch {
        Size::NONE
    } else {
        // The automatic width of a block-level box whose `justify-self` is neither `normal` nor `stretch`
        // is equivalent to `fit-content` rather than `stretch`
        let width_style = if is_fit_content && item.size_style.width.is_auto() {
            Dimension::fit_content()
        } else {
            item.size_style.width
        };

        // Items with a sizing keyword width (min-content, max-content, fit-content,
        // fit-content(...), stretch) resolve their width either directly or by measuring
        // the item under the corresponding available space constraint
        let keyword_width =
            resolve_sizing_keyword(width_style, Some(stretch_width), Some(container_inner_width), |val, basis| {
                tree.calc(val, basis)
            })
            .map(|resolution| match resolution {
                SizingKeywordResolution::Exact(width) => width,
                SizingKeywordResolution::Measure(item_available_width) => tree.measure_child_size(
                    item.node_id,
                    Size::NONE,
                    parent_size,
                    Size { width: item_available_width, height: AvailableSpace::MaxContent },
                    SizingMode::InherentSize,
                    crate::AbsoluteAxis::Horizontal,
                    // Must match the value passed when laying the item out (see `Cache`)
                    if item.is_in_same_bfc { Line::TRUE } else { Line::FALSE },
                ),
            });

        let keyword_height = resolve_stretch_height(
            item.size_style.height,
            container_percentage_resolution_height,
            item_non_auto_margin,
            children_margins_adjoin_own_edges,
        );

        // pyro
        let ratio_min_width =
            if item.min_width_is_content { Some(ratio_min_content_width(tree, item, parent_size)) } else { None };

        let mut known_dimensions = item
            .size
            .map_width(|width| {
                Some(
                    width
                        .or(keyword_width)
                        .unwrap_or(stretch_width)
                        .maybe_clamp(item.min_size.width, item.max_size.width)
                        .maybe_max(ratio_min_width),
                )
            })
            .map_height(|height| height.or(keyword_height))
            .maybe_clamp(item.min_size, item.max_size);

        // pyro: `min-width`/`max-width` that are sizing keywords
        if let Some(width) = known_dimensions.width {
            known_dimensions.width = Some(clamp_width_by_keywords(
                tree,
                item,
                width,
                Some(stretch_width),
                Some(container_inner_width),
                parent_size,
            ));
        }
        // pyro: `min-height`/`max-height` that are sizing keywords: the content height at the
        // item's width for the content keywords, the stretch height for `stretch`. A height of
        // `auto` is the content height already, so only `stretch` can change it.
        let (min_keyword, max_keyword) = (item.min_size_keyword.height, item.max_size_keyword.height);
        if let (Some(width), false) = (known_dimensions.width, min_keyword.is_auto() && max_keyword.is_auto()) {
            let mut content_height = None;
            let mut resolve = |keyword: Dimension| -> Option<f32> {
                if keyword.is_auto() {
                    None
                } else if keyword.is_stretch() {
                    resolve_stretch_height(
                        keyword,
                        container_percentage_resolution_height,
                        item_non_auto_margin,
                        children_margins_adjoin_own_edges,
                    )
                } else {
                    Some(*content_height.get_or_insert_with(|| {
                        tree.measure_child_size(
                            item.node_id,
                            Size { width: Some(width), height: None },
                            parent_size,
                            Size { width: AvailableSpace::Definite(width), height: AvailableSpace::MaxContent },
                            SizingMode::ContentSize,
                            crate::AbsoluteAxis::Vertical,
                            if item.is_in_same_bfc { Line::TRUE } else { Line::FALSE },
                        )
                    }))
                }
            };
            let (max, min) = (resolve(max_keyword), resolve(min_keyword));
            match known_dimensions.height {
                Some(height) => known_dimensions.height = Some(height.maybe_min(max).maybe_max(min)),
                None => {
                    // An `auto` height: the content's, unless `min-height: stretch` is taller.
                    if let (true, Some(min)) = (min_keyword.is_stretch(), min) {
                        let content = resolve(Dimension::max_content()).unwrap_or(0.0);
                        if min > content {
                            known_dimensions.height = Some(min);
                        }
                    }
                }
            }
        }

        // pyro: `BlockItem::content_min_height_ratio`
        if let (Some((ratio, inset)), Some(width), true) =
            (item.content_min_height_ratio, known_dimensions.width, keyword_height.is_none())
        {
            let height = known_dimensions.height.unwrap_or_else(|| {
                ((width - inset.width).max(0.0) / ratio + inset.height)
                    .maybe_clamp(item.min_size.height, item.max_size.height)
            });
            let content_height = tree.measure_child_size(
                item.node_id,
                Size { width: Some(width), height: None },
                parent_size,
                Size { width: AvailableSpace::Definite(width), height: AvailableSpace::MaxContent },
                SizingMode::ContentSize,
                crate::AbsoluteAxis::Vertical,
                if item.is_in_same_bfc { Line::TRUE } else { Line::FALSE },
            );
            known_dimensions.height = Some(height.max(content_height.maybe_min(item.max_size.height)));
        }
        known_dimensions
    }
}

/// Compute each child's final size and position
#[inline]
#[allow(clippy::too_many_arguments)]
fn perform_final_layout_on_in_flow_children<Tree: LayoutBlockContainer>(
    tree: &mut Tree,
    run_mode: RunMode,
    items: &mut [BlockItem],
    container_outer_width: f32,
    container_percentage_resolution_height: Option<f32>,
    content_box_inset: Rect<f32>,
    resolved_content_box_inset: Rect<f32>,
    resolved_border: Rect<f32>,
    #[cfg(feature = "content_size")] resolved_padding: Rect<f32>,
    text_align: TextAlign,
    #[cfg_attr(not(feature = "float_layout"), allow(unused_variables))] line_edge: AxisStaticEdge,
    justify_items: AlignItems,
    direction: Direction,
    own_margins_collapse_with_children: Line<bool>,
    children_margins_adjoin_own_edges: Line<bool>,
    margin_trim: Line<bool>,
    #[cfg(feature = "content_size")] is_scroll_container: bool,
    block_ctx: &mut BlockContext<'_>,
) -> (Rect<f32>, f32, CollapsibleMarginSet, CollapsibleMarginSet, Option<f32>, Option<f32>) {
    // Resolve container_inner_width for sizing child nodes using initial content_box_inset
    let container_inner_width = (container_outer_width - resolved_content_box_inset.horizontal_axis_sum()).max(0.0);
    #[cfg(feature = "content_size")]
    let overflow_container = ScrollableOverflowContainer {
        parent_is_scroll_container: is_scroll_container,
        parent_size: Size { width: container_inner_width + resolved_padding.horizontal_axis_sum(), height: 0.0 },
        reversed: Point { x: direction.is_rtl(), y: false },
        padding: resolved_padding,
    };
    let container_percentage_resolution_height =
        container_percentage_resolution_height.maybe_sub(resolved_content_box_inset.vertical_axis_sum());
    let parent_size = Size { width: Some(container_inner_width), height: container_percentage_resolution_height };
    // Vertical available space in block flow is indefinite, NOT a min-content
    // constraint: MaxContent is taffy's representation of "indefinite".
    // Passing MinContent here made every descendant grid believe it was being
    // sized under a min-content constraint, in which the maximize-tracks step
    // has zero free space — so auto rows containing only scroll-container
    // items (overflow != visible, automatic minimum size = 0) collapsed to
    // zero height. Browsers size such rows to the item's content.
    let available_space =
        Size { width: AvailableSpace::Definite(container_inner_width), height: AvailableSpace::MaxContent };

    // TODO: handle nested blocks with different widths
    #[cfg(feature = "float_layout")]
    if block_ctx.is_bfc_root() {
        block_ctx.set_width(container_outer_width);
        block_ctx.apply_content_box_inset(FloatInsets::new(
            resolved_content_box_inset.left,
            resolved_content_box_inset.right,
        ));
    }

    // If this block's top margin does not collapse with its children's then the position of its
    // top margin strut is resolved relative to it, and floats adjoining ancestor struts do not
    // adjoin this block's strut.
    #[cfg(feature = "float_layout")]
    if !own_margins_collapse_with_children.start {
        block_ctx.commit_strut();
    }

    #[cfg_attr(not(feature = "content_size"), allow(unused_mut))]
    let mut inflow_overflow_rect = Rect::ZERO;
    let mut committed_y_offset = resolved_content_box_inset.top;
    let mut y_offset_for_absolute = resolved_content_box_inset.top;
    let mut first_child_top_margin_set = CollapsibleMarginSet::ZERO;
    let mut active_collapsible_margin_set = CollapsibleMarginSet::ZERO;
    let mut is_collapsing_with_first_margin_set = true;
    let mut first_baseline: Option<f32> = None;
    // pyro: and its last baseline, the last baseline of its last in-flow child that has one.
    let mut last_baseline: Option<f32> = None;
    // Whether the active margin set contains the margins of a self-collapsing element with
    // clearance. Such margins collapse with the margins of following siblings but the resulting
    // margin does not collapse with the bottom margin of the parent block.
    let mut active_margin_set_has_clearance = false;
    // pyro: `margin-trim: block-start` (css-box-4 §3.3): the block-start margins of the in-flow
    // boxes at the container's block-start edge are truncated to zero, with every margin that
    // collapses with them: the margins their descendants collapse through them, and both margins
    // of a box that is collapsed through, after which the next box is at the edge.
    let mut trims_start_margin = margin_trim.start;
    // pyro: `margin-trim: block-end`: the bottom of the last in-flow box that is not collapsed
    // through, and whether there is one.
    let mut last_solid_item_bottom: Option<f32> = None;
    let mut has_in_flow_box = false;

    #[cfg(feature = "float_layout")]
    let mut has_active_floats = block_ctx.has_active_floats(committed_y_offset);
    #[cfg(not(feature = "float_layout"))]
    let has_active_floats = false;

    for item in items.iter_mut() {
        if item.position.is_out_of_flow() {
            let x = match direction {
                Direction::Ltr => resolved_content_box_inset.left,
                Direction::Rtl => container_outer_width - resolved_content_box_inset.right,
            };
            item.static_position = Point { x, y: y_offset_for_absolute };
            // pyro: an inline-level out-of-flow child (inline-level before `position` blockified
            // it) has the static position of the start of a line at its place: beside the floats
            // there, where `text-align` puts the line's content (Blink's
            // `HandleOutOfFlowPositioned`; WPT `inline-level-absolute-in-block-level-context-*`).
            #[cfg(feature = "float_layout")]
            if item.is_inline_level_oof {
                let line_start = resolved_content_box_inset.left;
                let line_end = container_outer_width - resolved_content_box_inset.right;
                let (left, right) = if block_ctx.has_active_floats(y_offset_for_absolute) {
                    let slot = block_ctx.find_content_slot(y_offset_for_absolute, 0.0, Clear::None, None);
                    (slot.x.max(line_start), (slot.x + slot.width).min(line_end))
                } else {
                    (line_start, line_end)
                };
                let right = right.max(left);
                item.inline_static_x = Some(match line_edge {
                    AxisStaticEdge::Start => left,
                    AxisStaticEdge::End => right,
                    AxisStaticEdge::Center => (left + right) / 2.0,
                });
            }
        } else {
            #[cfg_attr(not(feature = "float_layout"), allow(unused_mut))]
            let mut item_margin = item
                .margin
                .map(|margin| margin.resolve_to_option(container_inner_width, |val, basis| tree.calc(val, basis)));
            // pyro: `margin-trim: block-start` (floats are not in flow and keep their margins)
            #[cfg(feature = "float_layout")]
            let trims_item_start_margin = trims_start_margin && !item.float.is_floated();
            #[cfg(not(feature = "float_layout"))]
            let trims_item_start_margin = trims_start_margin;
            if trims_item_start_margin {
                item_margin.top = Some(0.0);
            }
            let item_non_auto_margin = item_margin.map(|m| m.unwrap_or(0.0));
            let item_non_auto_x_margin_sum = item_non_auto_margin.horizontal_axis_sum();

            let scrollbar_size = Size {
                width: if item.overflow.y == Overflow::Scroll { item.scrollbar_width } else { 0.0 },
                height: if item.overflow.x == Overflow::Scroll { item.scrollbar_width } else { 0.0 },
            };

            // Handle floated boxes
            #[cfg(feature = "float_layout")]
            if let Some(float_direction) = item.float.float_direction() {
                has_active_floats = true;

                // A float with `width: auto` is shrink-to-fit (fit-content) sized: the available
                // space clamped between its min-content and max-content sizes.
                let available_width = (container_inner_width - item_non_auto_x_margin_sum).max(0.0);
                let (item_known_width, item_available_width) = match resolve_sizing_keyword(
                    item.size_style.width,
                    Some(available_width),
                    Some(container_inner_width),
                    |val, basis| tree.calc(val, basis),
                ) {
                    Some(SizingKeywordResolution::Measure(available)) => (None, available),
                    Some(SizingKeywordResolution::Exact(width)) => (Some(width), AvailableSpace::Definite(width)),
                    None => (None, AvailableSpace::Definite(available_width)),
                };
                let item_known_height = resolve_stretch_height(
                    item.size_style.height,
                    container_percentage_resolution_height,
                    item_non_auto_margin,
                    // The margins of a float never collapse
                    Line::FALSE,
                );
                let mut item_layout = tree.perform_child_layout(
                    item.node_id,
                    Size { width: item_known_width, height: item_known_height },
                    parent_size,
                    Size { width: item_available_width, height: AvailableSpace::MaxContent },
                    SizingMode::InherentSize,
                    // A float establishes a new block formatting context: its margins do not
                    // collapse with the margins of its children
                    Line::FALSE,
                );
                item.oof_candidates = item_layout.oof_candidates.take();
                let margin_box = item_layout.size + item_non_auto_margin.sum_axes();

                // Floats that occur between collapsing margins are positioned as if they had an otherwise
                // empty anonymous block parent taking part in the flow, so the pending collapsible margins
                // contribute to the float's minimum y position (unless those margins collapse with the
                // container's own top margin, in which case they are applied outside the container).
                //
                // In the latter case the position of the float is not fully resolved: margins contributed
                // by later siblings can still collapse into the strut and move the container (and float).
                // Such floats force clearance on cleared elements whose margins adjoin the same strut.
                let adjoins_unresolved_strut =
                    is_collapsing_with_first_margin_set && own_margins_collapse_with_children.start;
                let y_offset_for_float = if adjoins_unresolved_strut {
                    committed_y_offset
                } else {
                    committed_y_offset + active_collapsible_margin_set.resolve()
                };

                let mut location = block_ctx.place_floated_box(
                    item.node_id.into(),
                    margin_box,
                    y_offset_for_float,
                    float_direction,
                    item.clear,
                    adjoins_unresolved_strut,
                );

                // Ensure that content that appears after a float does not get positioned before/above the float
                //
                // FIXME: this isn't quite right, because a second float at the same location
                // shouldn't cause content to push down to it's level
                // committed_y_offset = committed_y_offset.max(location.y);
                // y_offset_for_absolute = y_offset_for_absolute.max(location.y);

                // Convert the margin-box location returned by float placement into a border-box location
                // for the output Layout
                location.y += item_non_auto_margin.top;
                location.x += item_non_auto_margin.left;

                // A relatively positioned float is placed as a float first, then offset by its
                // inset. The offset does not affect the placement of other content (CSS 2.2 9.4.3).
                let inset_offset = resolve_relative_offset(item, parent_size, direction, tree);
                location.x += inset_offset.x;
                location.y += inset_offset.y;

                // println!("BLOCK FLOATED BOX ({:?}) {:?}", item.node_id, float_direction);
                // println!("w:{} h:{} x:{}, y:{}", margin_box.width, margin_box.height, location.x, location.y);

                // Deferred to the post-loop pass in `compute_inner` (like in-flow items) so that
                // `align-content` can shift `location.y` before the layout is committed.
                item.final_layout = Some(Layout {
                    order: item.order,
                    size: item_layout.size,
                    #[cfg(feature = "content_size")]
                    scrollable_overflow_rect: item_layout.scrollable_overflow_rect,
                    scrollbar_size,
                    location,
                    padding: item.padding,
                    border: item.border,
                    margin: item_non_auto_margin,
                });

                #[cfg(feature = "content_size")]
                {
                    // TODO: Should the overflow of floated boxes count as "inflow_overflow_rect"
                    // or should it be counted separately?
                    let contribution_location = Point {
                        x: location.x - resolved_content_box_inset.left + resolved_padding.left,
                        y: location.y - resolved_border.top,
                    };
                    inflow_overflow_rect = inflow_overflow_rect.union(compute_scrollable_overflow_contribution(
                        contribution_location,
                        item_layout.size,
                        item_layout.scrollable_overflow_rect.rect(),
                        item.border,
                        item.overflow,
                        item.contain,
                        &overflow_container,
                        item_non_auto_margin.map(|value| value.min(0.0)),
                    ));
                }

                continue;
            }

            // Handle non-floated boxes

            // `justify-self` on an in-flow block-level box (css-align-3 §6.1.1). `None` takes the
            // container's `justify-items`, and `normal` lays the box out according to the default
            // block layout rules. `stretch` does too: the default rules already stretch an
            // auto-width box, so it only differs from `normal` for tables and replaced boxes, which
            // would otherwise resolve their own width.
            let justify_self =
                item.justify_self.unwrap_or(justify_items).resolve_self_relative(item.direction, direction, true);
            let is_stretch = justify_self.keyword == AlignItemsKeyword::Stretch;
            // The (resolved) `justify-self` if it is neither `normal` nor `stretch`
            let non_stretch_justify_self = match justify_self.keyword {
                AlignItemsKeyword::Normal | AlignItemsKeyword::Stretch => None,
                _ => Some(justify_self),
            };

            // The inputs to lay the item out with, given the stretch width available to it
            let item_inputs = |tree: &mut Tree, stretch_width: f32| LayoutInput {
                run_mode,
                sizing_mode: SizingMode::InherentSize,
                axis: RequestedAxis::Both,
                known_dimensions: block_item_known_dimensions(
                    tree,
                    item,
                    stretch_width,
                    container_inner_width,
                    container_percentage_resolution_height,
                    item_non_auto_margin,
                    children_margins_adjoin_own_edges,
                    parent_size,
                    is_stretch,
                    non_stretch_justify_self.is_some(),
                ),
                known_dimensions_are_definite: Size { width: true, height: true },
                parent_size,
                available_space: available_space.map_width(|_| AvailableSpace::Definite(stretch_width)),
                vertical_margins_are_collapsible: if item.is_in_same_bfc { Line::TRUE } else { Line::FALSE },
            };

            let mut y_margin_offset: f32 = 0.0;
            #[cfg(feature = "float_layout")]
            let mut item_avoids_floats = false;
            #[cfg(feature = "float_layout")]
            let mut item_pushed_below_float = false;

            let (stretch_width, float_avoiding_position, float_avoiding_width) = if item.is_in_same_bfc {
                let stretch_width = (container_inner_width - item_non_auto_x_margin_sum).max(0.0);
                let position = Point { x: 0.0, y: 0.0 };
                let width = 0.0;

                (stretch_width, position, width)
            } else {
                'block: {
                    // Set y_margin_offset (different bfc child)
                    if !is_collapsing_with_first_margin_set || !own_margins_collapse_with_children.start {
                        y_margin_offset =
                            active_collapsible_margin_set.collapse_with_margin(item_non_auto_margin.top).resolve();
                    };
                    let min_y = committed_y_offset + y_margin_offset;

                    // In addition to the running flag, check the float context directly:
                    // floats placed by the subtree of a preceding in-flow sibling (in the same
                    // BFC) are not reflected in the flag
                    #[cfg(feature = "float_layout")]
                    if has_active_floats || block_ctx.has_active_floats(min_y) {
                        let x_margins = FloatInsets::new(item_non_auto_margin.left, item_non_auto_margin.right);
                        // An auto width resolves to at least the negation of the margin sum
                        // (so that the margin box width is non-negative, per CSS2 §10.3.3)
                        let min_auto_width = -item_non_auto_x_margin_sum;

                        // Find the highest slot (at or below `min_y`) with enough horizontal space
                        // for the item's border box, which must not overlap any float
                        // The item's height isn't known until it is laid out, so only its styled
                        // (minimum) height is checked against floats below its top edge
                        let mut min_height = item
                            .size
                            .height
                            .maybe_clamp(item.min_size.height, item.max_size.height)
                            .or(item.min_size.height)
                            .unwrap_or(0.0)
                            .max(item.padding_border_sum.height);
                        // pyro: the narrowest border box the item can have in a slot: a table, a
                        // replaced element or a box of an intrinsic width is as wide as it is laid
                        // out in the slot (WPT `floats-wrap-bfc-004`), any other auto width shrinks
                        // to the slot, down to its padding and border, its `min-width` and what its
                        // negative margins prop it up to (CSS2 §10.3.3). The layout is the one the
                        // item gets below if the slot stands (the same inputs: cached).
                        let plain_auto = item.size.width.is_none()
                            && item.size_style.width.is_auto()
                            && !item.is_table
                            && !item.is_compressible_replaced
                            && non_stretch_justify_self.is_none();
                        let mut slot_segment = None;
                        let mut height_retries = 0;
                        let slot = loop {
                            let slot = block_ctx.find_bfc_slot(
                                min_y,
                                min_height,
                                x_margins,
                                direction,
                                item.clear,
                                slot_segment,
                            );
                            let Some(segment_id) = slot.segment_id else { break slot };
                            let stretch_width = slot.stretch_width.max(min_auto_width).max(0.0);
                            let mut laid_out = None;
                            let width = if plain_auto {
                                min_auto_width.maybe_clamp(item.min_size.width, item.max_size.width)
                            } else if item.is_table || item.size.width.is_none() {
                                let inputs = item_inputs(tree, stretch_width);
                                let size = tree.compute_child_layout(item.node_id, inputs).size;
                                laid_out = Some(size);
                                size.width
                            } else {
                                item.size.width.unwrap_or(0.0).maybe_clamp(item.min_size.width, item.max_size.width)
                            };
                            if width.max(item.padding_border_sum.width) > slot.border_width + FIT_TOLERANCE {
                                slot_segment = Some(segment_id);
                                continue;
                            }
                            // pyro: the item must not overlap the floats beside any of its height,
                            // which it has once laid out in the slot: taller than the slot stays as
                            // wide, it is placed again beside the floats of its height (WPT
                            // `table-among-floats-001`, `floats-bfc-003`).
                            if slot.height.is_finite() && height_retries < 8 {
                                let height = match laid_out {
                                    Some(size) => size.height,
                                    None => {
                                        let inputs = item_inputs(tree, stretch_width);
                                        tree.compute_child_layout(item.node_id, inputs).size.height
                                    }
                                };
                                if height > slot.height + FIT_TOLERANCE && height > min_height + FIT_TOLERANCE {
                                    min_height = height;
                                    height_retries += 1;
                                    continue;
                                }
                            }
                            break slot;
                        };

                        // If the item had to move down to avoid floats then it "separates from the
                        // float": similarly to clearance, its top margin no longer collapses with
                        // the parent's margins.
                        if slot.y > min_y {
                            item_pushed_below_float = true;
                        }

                        has_active_floats = slot.segment_id.is_some();
                        item_avoids_floats = true;
                        let stretch_width = slot.stretch_width.max(min_auto_width).max(0.0);
                        break 'block (stretch_width, Point { x: slot.x, y: slot.y }, slot.border_width);
                    }

                    if !has_active_floats {
                        let stretch_width = (container_inner_width - item_non_auto_x_margin_sum).max(0.0);
                        break 'block (
                            stretch_width,
                            Point { x: resolved_content_box_inset.left, y: min_y },
                            container_inner_width,
                        );
                    }

                    unreachable!("One of the above cases will always be hit");
                }
            };

            let inputs = item_inputs(tree, stretch_width);

            #[cfg(feature = "float_layout")]
            let clear_threshold = block_ctx.cleared_threshold(item.clear);
            #[cfg(feature = "float_layout")]
            let clear_pos = clear_threshold.unwrap_or(f32::NEG_INFINITY);
            #[cfg(not(feature = "float_layout"))]
            let clear_pos = f32::NEG_INFINITY;

            // The height consumed by floats within a same-BFC child, relative to the child's
            // border-top. Added to the parent's contribution once the child's position is known.
            #[cfg(feature = "float_layout")]
            let mut child_float_contribution = f32::NEG_INFINITY;

            let mut item_layout = if item.is_in_same_bfc {
                // Replaced elements may not have a known width (they are sized by their
                // measure function rather than stretch-sized)
                let width = inputs.known_dimensions.width.unwrap_or(stretch_width).max(item.padding_border_sum.width);

                // TODO: account for auto margins
                let inset_left = item_non_auto_margin.left + content_box_inset.left;
                let inset_right = container_outer_width - width - inset_left;
                let insets = FloatInsets::new(inset_left, inset_right);

                // Compute child layout. The child's position is estimated from its top margin collapsed with
                // the preceding margins (the child's own collapsed-through margins are not yet known).
                let estimated_y = committed_y_offset
                    + active_collapsible_margin_set.collapse_with_margin(item_non_auto_margin.top).resolve();
                let mut child_block_ctx = block_ctx.sub_context(estimated_y.max(clear_pos), insets);
                let output = tree.compute_block_child_layout(item.node_id, inputs, Some(&mut child_block_ctx));

                // Extract float contribution from child block context
                #[cfg(feature = "float_layout")]
                {
                    child_float_contribution = child_block_ctx.floated_content_height_contribution();
                    let child_top_adjoining_floats = child_block_ctx.top_adjoining_floats();
                    // Floats placed while the position of the child's top margin strut was unresolved
                    // also adjoin this block's current strut
                    block_ctx.merge_adjoining_floats(child_top_adjoining_floats);
                }

                output
            } else {
                tree.compute_child_layout(item.node_id, inputs)
            };
            item.oof_candidates = item_layout.oof_candidates.take();
            let final_size = item_layout.size;

            has_in_flow_box = true;
            // pyro: `margin-trim: block-start`
            if trims_item_start_margin {
                item_layout.top_margin = CollapsibleMarginSet::ZERO;
                if item_layout.margins_can_collapse_through {
                    item_layout.bottom_margin = CollapsibleMarginSet::ZERO;
                    item_margin.bottom = Some(0.0);
                } else {
                    trims_start_margin = false;
                }
            }

            let top_margin_set = item_layout.top_margin.collapse_with_margin(item_margin.top.unwrap_or(0.0));
            let bottom_margin_set = item_layout.bottom_margin.collapse_with_margin(item_margin.bottom.unwrap_or(0.0));

            // Expand auto margins to fill available space
            // Note: Vertical auto-margins for relatively positioned block items simply resolve to 0.
            // See: https://www.w3.org/TR/CSS21/visudet.html#abs-non-replaced-width
            let free_x_space = f32_max(0.0, stretch_width - final_size.width);
            let x_axis_auto_margin_size = {
                let auto_margin_count = item_margin.left.is_none() as u8 + item_margin.right.is_none() as u8;
                if auto_margin_count > 0 {
                    free_x_space / auto_margin_count as f32
                } else {
                    0.0
                }
            };
            let resolved_margin = Rect {
                left: item_margin.left.unwrap_or(x_axis_auto_margin_size),
                right: item_margin.right.unwrap_or(x_axis_auto_margin_size),
                top: item_margin.top.unwrap_or(0.0),
                bottom: item_margin.bottom.unwrap_or(0.0),
            };

            // Resolve item inset
            let inset_offset = resolve_relative_offset(item, parent_size, direction, tree);

            // Set y_margin_offset (same bfc child)
            if item.is_in_same_bfc
                && (!is_collapsing_with_first_margin_set || !own_margins_collapse_with_children.start)
            {
                y_margin_offset = active_collapsible_margin_set.collapse_with_set(top_margin_set).resolve()
            };

            // Compute clearance (CSS2.2 9.5.2). Clearance is introduced if the hypothetical position of the
            // item's top border edge (the position it would have with normal margin collapsing) is not past
            // the bottom of the relevant floats. When clearance is introduced the item's border edge is
            // placed at `max(float bottom, hypothetical position)`.
            #[cfg(feature = "float_layout")]
            let mut has_clearance = false;
            #[cfg(not(feature = "float_layout"))]
            let has_clearance = false;
            #[cfg(feature = "float_layout")]
            if item.is_in_same_bfc {
                if let Some(threshold) = clear_threshold {
                    // The hypothetical position always includes the item's collapsed top margin set, even
                    // when those margins collapse with the container's own top margin (and are thus applied
                    // outside the container): in that case they still move the container (and hence the item)
                    // relative to the floats.
                    let hypothetical_y =
                        committed_y_offset + active_collapsible_margin_set.collapse_with_set(top_margin_set).resolve();
                    // Clearance is forced (regardless of the hypothetical position) if a relevant float is
                    // adjoining the margin-collapse strut that the item's top margin would collapse into:
                    // if the margins were allowed to collapse they would pull the float down with the item,
                    // so clearance is inserted to separate the two, placing the item just below the float.
                    let forced_clearance = block_ctx.has_adjoining_float(item.clear);
                    if forced_clearance || hypothetical_y < threshold {
                        has_clearance = true;
                        // Clearance stops the item's top margin collapsing with preceding margins. If those
                        // preceding margins collapse with the container's own top margin they are applied
                        // outside the container (moving it down), so the item's cleared position within the
                        // container must be reduced by that amount to keep its absolute position correct.
                        let escaped_margin =
                            if is_collapsing_with_first_margin_set && own_margins_collapse_with_children.start {
                                active_collapsible_margin_set.resolve()
                            } else {
                                0.0
                            };
                        y_margin_offset = threshold - committed_y_offset - escaped_margin;
                    }
                }
            }

            item.computed_size = item_layout.size;
            item.can_be_collapsed_through = item_layout.margins_can_collapse_through && !has_clearance;
            item.static_position = if item.is_in_same_bfc {
                let uncleared_y = committed_y_offset + active_collapsible_margin_set.resolve();
                Point {
                    x: match direction {
                        Direction::Ltr => resolved_content_box_inset.left,
                        Direction::Rtl => container_outer_width - resolved_content_box_inset.right - final_size.width,
                    },
                    y: uncleared_y.max(clear_pos),
                }
            } else {
                // TODO: handle inset and margins
                Point {
                    x: match direction {
                        Direction::Ltr => float_avoiding_position.x,
                        Direction::Rtl => float_avoiding_position.x + float_avoiding_width - final_size.width,
                    },
                    y: float_avoiding_position.y,
                }
            };
            let mut location = if item.is_in_same_bfc {
                Point {
                    x: match direction {
                        Direction::Ltr => resolved_content_box_inset.left + inset_offset.x + resolved_margin.left,
                        Direction::Rtl => {
                            container_outer_width
                                - resolved_content_box_inset.right
                                - final_size.width
                                - resolved_margin.right
                                + inset_offset.x
                        }
                    },
                    y: committed_y_offset + y_margin_offset + inset_offset.y,
                }
            } else {
                // When the item avoids floats, its non-auto margins are already accounted for in the
                // slot's border-box position/width (margins may overlap floats), so only the auto
                // portion of the resolved margin is added here.
                #[cfg(feature = "float_layout")]
                let (extra_margin_left, extra_margin_right) = if item_avoids_floats {
                    (
                        resolved_margin.left - item_non_auto_margin.left,
                        resolved_margin.right - item_non_auto_margin.right,
                    )
                } else {
                    (resolved_margin.left, resolved_margin.right)
                };
                #[cfg(not(feature = "float_layout"))]
                let (extra_margin_left, extra_margin_right) = (resolved_margin.left, resolved_margin.right);

                // TODO: handle inset and margins
                Point {
                    x: match direction {
                        Direction::Ltr => float_avoiding_position.x + extra_margin_left + inset_offset.x,
                        Direction::Rtl => {
                            float_avoiding_position.x + float_avoiding_width - final_size.width - extra_margin_right
                                + inset_offset.x
                        }
                    },
                    y: float_avoiding_position.y + inset_offset.y,
                }
            };

            // Floats within the child are positioned relative to the child's border-top, so the
            // child's (margin-collapsed) position, rather than its relative offset, is added.
            #[cfg(feature = "float_layout")]
            if item.is_in_same_bfc {
                block_ctx.add_child_floated_content_height_contribution(
                    location.y - inset_offset.y + child_float_contribution,
                );
            }

            // Apply alignment. `justify-self` aligns the item's margin box within its alignment
            // container and takes precedence over legacy `text-align`, but auto margins (which have
            // already absorbed the free space) take precedence over `justify-self`.
            let has_auto_x_margin = item_margin.left.is_none() || item_margin.right.is_none();
            let item_outer_width = item_layout.size.width + resolved_margin.horizontal_axis_sum();
            if let (Some(justify_self), false) = (non_stretch_justify_self, has_auto_x_margin) {
                // The alignment container is the containing block, except that a BFC-establishing
                // box placed next to a float aligns within the space left over by the float (in
                // which case the slot is already inset by the item's margins).
                #[cfg(feature = "float_layout")]
                let (container_start, free_x_space) = if item_avoids_floats {
                    (float_avoiding_position.x - resolved_margin.left, float_avoiding_width - item_layout.size.width)
                } else {
                    (resolved_content_box_inset.left, container_inner_width - item_outer_width)
                };
                #[cfg(not(feature = "float_layout"))]
                let (container_start, free_x_space) =
                    (resolved_content_box_inset.left, container_inner_width - item_outer_width);

                let keyword = if matches!(justify_self.safety, AlignmentSafety::Safe) && free_x_space < 0.0 {
                    AlignItemsKeyword::Start
                } else {
                    justify_self.keyword
                };
                let align_to_end = match keyword {
                    AlignItemsKeyword::Center => None,
                    // pyro: `last baseline` falls back to `end` (it aligns flex rows only)
                    AlignItemsKeyword::End
                    | AlignItemsKeyword::FlexEnd
                    | AlignItemsKeyword::SelfEnd
                    | AlignItemsKeyword::LastBaseline => Some(true),
                    _ => Some(false),
                };
                let offset = match align_to_end {
                    None => free_x_space / 2.0,
                    Some(end) if end != direction.is_rtl() => free_x_space,
                    Some(_) => 0.0,
                };
                location.x = container_start + resolved_margin.left + inset_offset.x + offset;
            } else if non_stretch_justify_self.is_none() && item_outer_width < container_inner_width {
                let free_x_space = container_inner_width - item_outer_width;
                match (text_align, direction) {
                    (TextAlign::Auto, _) => {
                        // Do nothing
                    }
                    (TextAlign::LegacyLeft, Direction::Ltr) => {
                        // Do nothing. Left aligned by default.
                    }
                    (TextAlign::LegacyLeft, Direction::Rtl) => location.x -= free_x_space,
                    (TextAlign::LegacyRight, Direction::Ltr) => location.x += free_x_space,
                    (TextAlign::LegacyRight, Direction::Rtl) => {
                        // Do nothing. Right aligned by default.
                    }
                    (TextAlign::LegacyCenter, Direction::Ltr) => location.x += free_x_space / 2.0,
                    (TextAlign::LegacyCenter, Direction::Rtl) => location.x -= free_x_space / 2.0,
                }
            }

            // A block container's first baseline is the first baseline of its first in-flow child
            // that has one. The child's relative offset does not affect it.
            if first_baseline.is_none() {
                let child_baseline =
                    child_first_baseline(item_layout.baselines.first, item_layout.size.height, item.overflow.y);
                first_baseline = child_baseline.map(|baseline| location.y - inset_offset.y + baseline);
            }
            // pyro: a block container's last baseline is the last baseline of its last in-flow
            // child that has one (an inline-block's, CSS 2.1 § 10.8.1, is its last line box's).
            // A child that is a scroll container gives its bottom margin edge, whatever lines it
            // holds (as an inline-block that is one does; WPT `baseline-block-with-overflow-001`).
            // A table gives none: it is skipped, as in Chromium (an inline-block holding only
            // tables sits on its bottom margin edge whatever their rows hold; WPT
            // `position-relative-table-*`).
            // A block container gives its own: none when it holds no line box (but tables).
            let child_last_baseline = if item.is_table {
                None
            } else if item.overflow.y.is_scroll_container() {
                Some(item_layout.size.height + resolved_margin.bottom)
            } else if item.is_block_container {
                item_layout.baselines.last
            } else {
                item_layout.baselines.last.or(item_layout.baselines.first)
            };
            if let Some(baseline) = child_last_baseline {
                last_baseline = Some(location.y - inset_offset.y + baseline);
            }

            // Used auto margins include float-avoidance offsets, unlike the alignment margins above.
            #[cfg(feature = "float_layout")]
            let resolved_margin = {
                let mut margin = resolved_margin;
                if item_avoids_floats {
                    if item_margin.left.is_none() {
                        margin.left += float_avoiding_position.x - resolved_content_box_inset.left;
                    }
                    if item_margin.right.is_none() {
                        margin.right += container_outer_width
                            - resolved_content_box_inset.right
                            - float_avoiding_position.x
                            - float_avoiding_width;
                    }
                }
                margin
            };

            // Defer `set_unrounded_layout` to the post-loop pass in `compute_inner` so that
            // `align-content` can shift `location.y` before the layout is committed to the tree.
            item.final_layout = Some(Layout {
                order: item.order,
                size: item_layout.size,
                #[cfg(feature = "content_size")]
                scrollable_overflow_rect: item_layout.scrollable_overflow_rect,
                scrollbar_size,
                location,
                padding: item.padding,
                border: item.border,
                margin: resolved_margin,
            });

            #[cfg(feature = "content_size")]
            {
                let contribution_location = Point {
                    x: location.x - resolved_content_box_inset.left + resolved_padding.left,
                    y: location.y - resolved_border.top,
                };
                inflow_overflow_rect = inflow_overflow_rect.union(compute_scrollable_overflow_contribution(
                    contribution_location,
                    final_size,
                    item_layout.scrollable_overflow_rect.rect(),
                    item.border,
                    item.overflow,
                    item.contain,
                    &overflow_container,
                    resolved_margin.map(|value| value.min(0.0)),
                ));
            }

            // Update first_child_top_margin_set
            //
            // The top margin of an item with clearance does not collapse with the container's top margin,
            // so clearance terminates collapsing without contributing the item's own margins.
            #[cfg(feature = "float_layout")]
            if is_collapsing_with_first_margin_set && item_pushed_below_float {
                // The item's top margin "separated from the float" and must not
                // propagate to the parent
                is_collapsing_with_first_margin_set = false;
            }
            if is_collapsing_with_first_margin_set && has_clearance {
                is_collapsing_with_first_margin_set = false;
            } else if is_collapsing_with_first_margin_set {
                if item.can_be_collapsed_through {
                    first_child_top_margin_set = first_child_top_margin_set
                        .collapse_with_set(top_margin_set)
                        .collapse_with_set(bottom_margin_set);
                } else {
                    first_child_top_margin_set = first_child_top_margin_set.collapse_with_set(top_margin_set);
                    is_collapsing_with_first_margin_set = false;
                }
            }

            // Update active_collapsible_margin_set
            if item.can_be_collapsed_through {
                active_collapsible_margin_set = active_collapsible_margin_set
                    .collapse_with_set(top_margin_set)
                    .collapse_with_set(bottom_margin_set);
                y_offset_for_absolute = committed_y_offset + item_layout.size.height + y_margin_offset;
            } else {
                committed_y_offset = location.y - inset_offset.y + item_layout.size.height;
                last_solid_item_bottom = Some(committed_y_offset);
                // A self-collapsing item with clearance is not collapsed through (its margins do not collapse
                // with margins of preceding siblings), but its top and bottom margins still collapse with each
                // other and with the margins of following siblings.
                if has_clearance && item_layout.margins_can_collapse_through {
                    // The element's border edge stays at the cleared position, but its collapsed margin
                    // extends below it: the border edge sits `top margin` inside the collapsed margin, so
                    // following content is offset by `collapsed margin - top margin` from the border edge.
                    committed_y_offset -= top_margin_set.resolve();
                    active_collapsible_margin_set = top_margin_set.collapse_with_set(bottom_margin_set);
                    active_margin_set_has_clearance = true;
                } else {
                    active_collapsible_margin_set = bottom_margin_set;
                    active_margin_set_has_clearance = false;
                }
                y_offset_for_absolute = committed_y_offset + active_collapsible_margin_set.resolve();
                // Committing in-flow content resolves the position of the current margin-collapse strut,
                // so floats placed before this point no longer force clearance
                #[cfg(feature = "float_layout")]
                block_ctx.commit_strut();
            }
        }
    }

    // pyro: `margin-trim: block-end` (css-box-4 §3.3): the block-end margin of the last in-flow
    // box is truncated to zero, with every margin that collapses with it: those of the boxes
    // after the last one that is not collapsed through (which then sit at its bottom edge), or
    // every margin when all the boxes are collapsed through.
    if margin_trim.end && has_in_flow_box {
        let edge = last_solid_item_bottom.unwrap_or(resolved_content_box_inset.top);
        for item in items.iter_mut().rev() {
            if item.position.is_out_of_flow() {
                item.static_position.y = edge;
                continue;
            }
            #[cfg(feature = "float_layout")]
            if item.float.is_floated() {
                continue;
            }
            let inset_offset = resolve_relative_offset(item, parent_size, direction, tree);
            let collapsed_through = item.can_be_collapsed_through;
            let Some(layout) = item.final_layout.as_mut() else { continue };
            layout.margin.bottom = 0.0;
            if !collapsed_through {
                break;
            }
            layout.location.y = edge + inset_offset.y;
            layout.margin.top = 0.0;
        }
        if last_solid_item_bottom.is_none() {
            first_child_top_margin_set = CollapsibleMarginSet::ZERO;
        }
        active_collapsible_margin_set = CollapsibleMarginSet::ZERO;
        active_margin_set_has_clearance = false;
    }

    // The margins of a self-collapsing element with clearance do not collapse with the bottom
    // margin of the parent block: they extend the parent's content height instead of escaping it
    let last_child_bottom_margin_set =
        if active_margin_set_has_clearance { CollapsibleMarginSet::ZERO } else { active_collapsible_margin_set };
    let bottom_y_margin_offset = if active_margin_set_has_clearance {
        active_collapsible_margin_set.resolve()
    } else if own_margins_collapse_with_children.end
        || (is_collapsing_with_first_margin_set && own_margins_collapse_with_children.start)
    {
        0.0
    } else {
        last_child_bottom_margin_set.resolve()
    };

    committed_y_offset += resolved_content_box_inset.bottom + bottom_y_margin_offset;
    let content_height = f32_max(0.0, committed_y_offset);
    (inflow_overflow_rect, content_height, first_child_top_margin_set, last_child_bottom_margin_set, first_baseline, last_baseline)
}
