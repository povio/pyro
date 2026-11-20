//! The abstractions that make up the core of Taffy's low-level API
//!
//! ## Examples
//!
//! The following examples demonstrate end-to-end implementation of Taffy's traits and usage of the low-level compute APIs:
//!
//!   - [custom_tree_vec](https://github.com/DioxusLabs/taffy/blob/main/examples/custom_tree_vec.rs) which implements a custom Taffy tree using a `Vec` as an arena with NodeId's being index's into the Vec.
//!   - [custom_tree_owned_partial](https://github.com/DioxusLabs/taffy/blob/main/examples/custom_tree_owned_partial.rs) which implements a custom Taffy tree using directly owned children with NodeId's being index's into vec on parent node.
//!   - [custom_tree_owned_unsafe](https://github.com/DioxusLabs/taffy/blob/main/examples/custom_tree_owned_unsafe.rs) which implements a custom Taffy tree using directly owned children with NodeId's being pointers.
//!
//! ## Overview
//!
//! ### Trait dependency tree
//!
//! The tree below illustrates which traits depend on which other traits.
//!
//! ```text
//! TraversePartialTree     - Access a node's children
//! ├──  LayoutPartialTree  - Run layout algorithms on a node and it's direct children
//! └──  TraverseTree       - Recursively access a node's descendants
//!     ├──  RoundTree      - Round a float-valued`  layout to integer pixels
//!     └──  PrintTree      - Print a debug representation of a node tree
//! ```
//!
//! ### A table of traits
//!
//! | Trait                 | Requires                | Enables                                                                                                                                                                                                                                                                                                                                                                                                                   |
//! | ---                   | ---                     | ---                                                                                                                                                                                                                                                                                                                                                                                                                       |
//! | [`LayoutPartialTree`] | [`TraversePartialTree`] | [`compute_flexbox_layout`](crate::compute_flexbox_layout)<br />[`compute_grid_layout`](crate::compute_grid_layout)<br />[`compute_block_layout`](crate::compute_block_layout)<br />[`compute_root_layout`](crate::compute_root_layout)<br />[`compute_leaf_layout`](crate::compute_leaf_layout)<br />[`compute_hidden_layout`](crate::compute_hidden_layout)<br />[`compute_cached_layout`](crate::compute_cached_layout) |
//! | [`RoundTree`]         | [`TraverseTree`]        | [`round_layout`](crate::round_layout)                                                                                                                                                                                                                                                                                                                                                                                     |
//! | [`PrintTree`]         | [`TraverseTree`]        | [`print_tree`](crate::print_tree)                                                                                                                                                                                                                                                                                                                                                                                         |
//!
//! ## All of the traits on one page
//!
//! ### TraversePartialTree and TraverseTree
//! These traits are Taffy's abstraction for downward tree traversal:
//!  - [`TraversePartialTree`] allows access to a single container node, and it's immediate children. This is the only "traverse" trait that is required
//!    for use of Taffy's core layout algorithms (flexbox, grid, etc).
//!  - [`TraverseTree`] is a marker trait which uses the same API signature as `TraversePartialTree`, but extends it with a guarantee that the child/children methods can be used to recurse
//!    infinitely down the tree. It is required by the `RoundTree` and
//!    the `PrintTree` traits.
//! ```rust
//! # use taffy::*;
//! pub trait TraversePartialTree {
//!     /// Type representing an iterator of the children of a node
//!     type ChildIter<'a>: Iterator<Item = NodeId>
//!     where
//!         Self: 'a;
//!
//!     /// Get the list of children IDs for the given node
//!     fn child_ids(&self, parent_node_id: NodeId) -> Self::ChildIter<'_>;
//!
//!     /// Get the number of children for the given node
//!     fn child_count(&self, parent_node_id: NodeId) -> usize;
//!
//!     /// Get a specific child of a node, where the index represents the nth child
//!     fn get_child_id(&self, parent_node_id: NodeId, child_index: usize) -> NodeId;
//! }
//!
//! pub trait TraverseTree: TraversePartialTree {}
//! ```
//!
//! You must implement [`TraversePartialTree`] to access any of Taffy's low-level API. If your tree implementation allows you to implement [`TraverseTree`] with
//! the correct semantics (full recursive traversal is available) then you should.
//!
//! ### LayoutPartialTree
//!
//! **Requires:** `TraversePartialTree`<br />
//! **Enables:** Flexbox, Grid, Block and Leaf layout algorithms from the [`crate::compute`] module
//!
//! Any type that implements [`LayoutPartialTree`] can be laid out using [Taffy's algorithms](crate::compute)
//!
//! Note that this trait extends [`TraversePartialTree`] (not [`TraverseTree`]). Taffy's algorithm implementations have been designed such that they can be used for a laying out a single
//! node that only has access to it's immediate children.
//!
//! ```rust
//! # use taffy::*;
//! pub trait LayoutPartialTree: TraversePartialTree {
//!     /// Get a reference to the [`Style`] for this node.
//!     fn get_style(&self, node_id: NodeId) -> &Style;
//!
//!     /// Set the node's unrounded layout
//!     fn set_unrounded_layout(&mut self, node_id: NodeId, layout: &Layout);
//!
//!     /// Get a mutable reference to the [`Cache`] for this node.
//!     fn get_cache_mut(&mut self, node_id: NodeId) -> &mut Cache;
//!
//!     /// Compute the specified node's size or full layout given the specified constraints
//!     fn compute_child_layout(&mut self, node_id: NodeId, inputs: LayoutInput) -> LayoutOutput;
//! }
//! ```
//!
//! ### RoundTree
//!
//! **Requires:** `TraverseTree`
//!
//! Trait used by the `round_layout` method which takes a tree of unrounded float-valued layouts and performs
//! rounding to snap the values to the pixel grid.
//!
//! As indicated by it's dependence on `TraverseTree`, it required full recursive access to the tree.
//!
//! ```rust
//! # use taffy::*;
//! pub trait RoundTree: TraverseTree {
//!     /// Get the node's unrounded layout
//!     fn get_unrounded_layout(&self, node_id: NodeId) -> Layout;
//!     /// Get a reference to the node's final layout
//!     fn set_final_layout(&mut self, node_id: NodeId, layout: &Layout);
//! }
//! ```
//!
//! ### PrintTree
//!
//! **Requires:** `TraverseTree`
//!
//! ```rust
//! /// Trait used by the `print_tree` method which prints a debug representation
//! ///
//! /// As indicated by it's dependence on `TraverseTree`, it required full recursive access to the tree.
//! # use taffy::*;
//! pub trait PrintTree: TraverseTree {
//!     /// Get a debug label for the node (typically the type of node: flexbox, grid, text, image, etc)
//!     fn get_debug_label(&self, node_id: NodeId) -> &'static str;
//!     /// Get a reference to the node's final layout
//!     fn get_final_layout(&self, node_id: NodeId) -> Layout;
//! }
//! ```
//!
use super::{DetailedLayoutInfo, Layout, LayoutInput, LayoutOutput, NodeId, RequestedAxis, RunMode, SizingMode};
use crate::debug::debug_log;
use crate::geometry::{AbsoluteAxis, Line, Size};
use crate::style::{AvailableSpace, CoreStyle, OofItemStyle};
#[cfg(feature = "flexbox")]
use crate::style::{FlexboxContainerStyle, FlexboxItemStyle};
#[cfg(feature = "grid")]
use crate::style::{GridContainerStyle, GridItemStyle};
use crate::CheapCloneStr;
#[cfg(feature = "block_layout")]
use crate::{BlockContainerStyle, BlockContext, BlockItemStyle};

#[cfg(feature = "grid")]
use crate::compute::grid::DetailedGridInfo;

/// Taffy's abstraction for downward tree traversal.
///
/// However, this trait does *not* require access to any node's other than a single container node's immediate children unless you also intend to implement `TraverseTree`.
pub trait TraversePartialTree {
    /// Type representing an iterator of the children of a node
    type ChildIter<'a>: Iterator<Item = NodeId>
    where
        Self: 'a;

    /// Get the list of children IDs for the given node
    fn child_ids(&self, parent_node_id: NodeId) -> Self::ChildIter<'_>;

    /// Get the number of children for the given node
    fn child_count(&self, parent_node_id: NodeId) -> usize;

    /// Get a specific child of a node, where the index represents the nth child
    fn get_child_id(&self, parent_node_id: NodeId, child_index: usize) -> NodeId;
}

/// A marker trait which extends `TraversePartialTree`
///
/// Implementing this trait implies the additional guarantee that the child/children methods can be used to recurse
/// infinitely down the tree. Is required by the `RoundTree` and the `PrintTree` traits.
pub trait TraverseTree: TraversePartialTree {}

/// Any type that implements [`LayoutPartialTree`] can be laid out using [Taffy's algorithms](crate::compute)
///
/// Note that this trait extends [`TraversePartialTree`] (not [`TraverseTree`]). Taffy's algorithm implementations have been designed such that they can be used for a laying out a single
/// node that only has access to it's immediate children.
pub trait LayoutPartialTree: TraversePartialTree {
    /// The style type representing the core container styles that all containers should have
    /// Used when laying out the root node of a tree
    type CoreContainerStyle<'a>: CoreStyle<CustomIdent = Self::CustomIdent>
    where
        Self: 'a;

    /// String type for representing "custom identifiers" (for example, named grid lines or areas)
    /// If you are unsure what to use here then consider `Arc<str>`.
    type CustomIdent: CheapCloneStr;

    /// Get core style
    ///
    /// Only called for the node being laid out, never for its children (those go through the
    /// `get_*_child_style` methods), so it may be expressed in the node's own axes while child
    /// styles are expressed in the axes of the node laying them out.
    fn get_core_container_style(&self, node_id: NodeId) -> Self::CoreContainerStyle<'_>;

    /// Resolve calc value
    #[inline(always)]
    fn resolve_calc_value(&self, val: *const (), basis: f32) -> f32 {
        let _ = val;
        let _ = basis;
        0.0
    }

    /// Set the node's unrounded layout
    fn set_unrounded_layout(&mut self, node_id: NodeId, layout: &Layout);

    /// Compute the specified node's size or full layout given the specified constraints
    fn compute_child_layout(&mut self, node_id: NodeId, inputs: LayoutInput) -> LayoutOutput;

    /// pyro: a width that replaces the block-level item `node_id`'s own in its parent block
    /// container's intrinsic (content-based) width, whatever its style says: a column spanner
    /// while a multi-column container measures its columns without them.
    #[inline(always)]
    fn block_item_intrinsic_width_override(&self, node_id: NodeId) -> Option<f32> {
        let _ = node_id;
        None
    }
}

/// Extends [`LayoutPartialTree`] with the operations needed by the out-of-flow positioning pass
/// ([`compute_oof_layout`](crate::compute_oof_layout)), which lays out the absolute/fixed boxes
/// for which a node acts as the containing block.
///
/// This trait is required by [`compute_oof_layout`](crate::compute_oof_layout) and
/// [`compute_root_layout`](crate::compute_root_layout).
pub trait LayoutContainingBlock: LayoutPartialTree {
    /// The style type representing the styles of an out-of-flow (absolute/fixed) box being
    /// positioned by its containing block
    type OofItemStyle<'a>: OofItemStyle<CustomIdent = Self::CustomIdent>
    where
        Self: 'a;

    /// Get the style of an out-of-flow box being positioned by its containing block
    ///
    /// Interpreted in the containing block's axes (see [`LayoutPartialTree::get_core_container_style`]).
    fn get_oof_item_style(&self, node_id: NodeId) -> Self::OofItemStyle<'_>;

    fn position_try_options(&self, _node: NodeId) -> (usize, Option<bool>) { (0,None) }
    fn preferred_scroll_option(&self, _node: NodeId, _container: NodeId) -> Option<usize> { None }
    fn selected_option_this_flush(&self, _node: NodeId) -> Option<Option<usize>> { None }
    fn record_fitting_option(&mut self, _node: NodeId, _option: Option<usize>) {}
    fn uses_anchor_queries(&self, _node: NodeId) -> bool { false }
    fn checks_position_overflow(&self, _node: NodeId) -> bool { false }
    fn position_try_scroll_offset(&self, _node: NodeId, _container: NodeId) -> crate::Point<f32> { crate::Point::ZERO }

    fn oof_anchor_center(&self, _node: NodeId, _container: NodeId) -> crate::Point<Option<f32>> { crate::Point { x:None,y:None } }

    fn set_position_overflow(&mut self, _node: NodeId, _overflows: bool) {}

    fn set_position_try_option(&mut self, _node: NodeId, _index: Option<usize>) -> bool { false }

    /// Anchor-dependent preferred, minimum and maximum dimensions, and margins.
    fn oof_anchor_sizes(&self, _node: NodeId, _container: NodeId, _basis: Size<f32>) -> Option<([Size<Option<f32>>; 3], crate::Rect<Option<f32>>)> { None }

    /// Used lengths supplied by anchor functions in the physical insets.
    fn oof_anchor_insets(&self, _node: NodeId, _container: NodeId, _size: Size<f32>, _offset: crate::Point<f32>) -> crate::Rect<Option<f32>> {
        crate::Rect::NONE
    }

    /// Resolve the position-area rectangle and its normal alignment in the containing block.
    fn oof_anchor_area(&self, _node: NodeId, _container: NodeId, _size: Size<f32>, _offset: crate::Point<f32>, _trial: bool) -> Option<(Size<f32>, crate::Point<f32>, crate::Point<crate::AlignSelf>)> {
        None
    }

    /// Clear the list of out-of-flow (absolute/fixed) boxes whose containing block is `node_id`.
    ///
    /// This is called (exactly once) for each node laid out with `RunMode::PerformLayout`, before
    /// the boxes it lays out are recorded with [`add_hoisted_children`](Self::add_hoisted_children),
    /// so that lists recorded by previous layout runs do not persist.
    fn clear_hoisted_children(&mut self, node_id: NodeId);

    /// Append to the list of out-of-flow boxes whose containing block is `node_id`.
    ///
    /// This is called by each containing block for the boxes it lays out, and by
    /// [`compute_root_layout`](crate::compute_root_layout) to record boxes (e.g. `position: fixed`
    /// boxes) which are positioned by the final root positioning pass, which runs after the root
    /// node's own layout algorithm has already recorded its list. The recorded lists are consumed
    /// by [`round_layout`](crate::round_layout) (out-of-flow boxes are rounded via their
    /// containing block rather than via their parent), and are also useful for consumers
    /// implementing paint/hit-testing traversals.
    fn add_hoisted_children(&mut self, node_id: NodeId, hoisted: &[NodeId]);

    /// pyro: whether the static-position rectangle of the out-of-flow box `node_id` is its whole
    /// containing block (its inset-resolution area) rather than its place in the flow: an
    /// element in the top layer's is the initial containing block (css-position-4 § 3.3; WPT
    /// `top-layer-box-uses-icb-*`), the box being aligned in it by its self-alignment.
    #[inline(always)]
    fn static_position_is_containing_block(&self, node_id: NodeId) -> bool {
        let _ = node_id;
        false
    }

    /// pyro: whether `container`, a containing block by its style, is not that of out-of-flow box
    /// `candidate` all the same (a column spanner between them: the spanner's row is no part of
    /// the container's fragments).
    #[inline(always)]
    fn containing_block_passes_over(&self, container: NodeId, candidate: NodeId) -> bool {
        let _ = (container, candidate);
        false
    }

    /// Read back the detailed layout information most recently recorded for `node_id`
    /// (as stored by [`LayoutGridContainer::set_detailed_grid_info`]).
    ///
    /// This is used by the out-of-flow positioning pass to resolve the grid area of absolutely
    /// positioned boxes whose containing block is a grid container. Implementing this method is
    /// optional: the default implementation returns [`DetailedLayoutInfo::None`], in which case
    /// such boxes are positioned relative to the grid container's padding box instead of their
    /// grid area.
    #[inline(always)]
    fn get_detailed_layout_info(&self, node_id: NodeId) -> &DetailedLayoutInfo<Self::CustomIdent> {
        let _ = node_id;
        &DetailedLayoutInfo::None
    }
}

/// Trait used by the `compute_cached_layout` method which allows cached layout results to be stored and retrieved.
///
/// The `Cache` struct implements a per-node cache that is compatible with this trait.
pub trait CacheTree {
    /// Try to retrieve a cached result from the cache
    fn cache_get(&mut self, node_id: NodeId, input: &LayoutInput) -> Option<LayoutOutput>;

    /// Store a computed size in the cache
    fn cache_store(&mut self, node_id: NodeId, input: &LayoutInput, layout_output: LayoutOutput);

    /// Clear all cache entries for the node
    fn cache_clear(&mut self, node_id: NodeId);
}

/// Trait used by the `round_layout` method which takes a tree of unrounded float-valued layouts and performs
/// rounding to snap the values to the pixel grid.
///
/// As indicated by it's dependence on `TraverseTree`, it required full recursive access to the tree.
pub trait RoundTree: TraverseTree {
    /// Get the node's unrounded layout
    fn get_unrounded_layout(&self, node_id: NodeId) -> Layout;
    /// Get a reference to the node's final layout
    fn set_final_layout(&mut self, node_id: NodeId, layout: &Layout);
    /// Whether the node is an out-of-flow (absolute/fixed) box. Out-of-flow boxes are hoisted to
    /// their containing block, so [`round_layout`](crate::round_layout) skips them when visiting a
    /// node's children and instead visits them via their containing block's hoisted child list.
    ///
    /// This should return `true` for box-generating nodes whose position style is `absolute` or
    /// `fixed`, and `false` otherwise (including for `display: none` nodes).
    fn is_out_of_flow(&self, node_id: NodeId) -> bool;
    /// The number of out-of-flow boxes whose containing block is `node_id`
    /// (as recorded by [`LayoutContainingBlock::add_hoisted_children`])
    fn hoisted_child_count(&self, node_id: NodeId) -> usize;
    /// Get the nth out-of-flow box whose containing block is `node_id`
    /// (as recorded by [`LayoutContainingBlock::add_hoisted_children`])
    fn get_hoisted_child_id(&self, node_id: NodeId, index: usize) -> NodeId;
    /// Whether `node_id`'s subtree is rounded as it was the last time: the tree knows the
    /// subtree's layout did not change and `round_layout` reached the node with the same
    /// unrounded layout at the same unrounded absolute offset. [`round_layout`](crate::round_layout)
    /// then skips the subtree. The default never skips.
    fn rounded_unchanged(&self, _node_id: NodeId, _unrounded: &Layout, _absolute_x: f32, _absolute_y: f32) -> bool {
        false
    }
    /// Records the unrounded layout and absolute offset `node_id` was just rounded from
    /// (for [`RoundTree::rounded_unchanged`]).
    fn record_rounded(&mut self, _node_id: NodeId, _unrounded: &Layout, _absolute_x: f32, _absolute_y: f32) {}
}

/// Trait used by the `print_tree` method which prints a debug representation
///
/// As indicated by it's dependence on `TraverseTree`, it required full recursive access to the tree.
pub trait PrintTree: TraverseTree {
    /// Get a debug label for the node (typically the type of node: flexbox, grid, text, image, etc)
    fn get_debug_label(&self, node_id: NodeId) -> &'static str;
    /// Get a reference to the node's final layout
    fn get_final_layout(&self, node_id: NodeId) -> Layout;
}

#[cfg(feature = "flexbox")]
/// Extends [`LayoutPartialTree`] with getters for the styles required for Flexbox layout
pub trait LayoutFlexboxContainer: LayoutPartialTree {
    /// The style type representing the Flexbox container's styles
    type FlexboxContainerStyle<'a>: FlexboxContainerStyle
    where
        Self: 'a;
    /// The style type representing each Flexbox item's styles
    type FlexboxItemStyle<'a>: FlexboxItemStyle
    where
        Self: 'a;

    /// Get the container's styles
    fn get_flexbox_container_style(&self, node_id: NodeId) -> Self::FlexboxContainerStyle<'_>;

    /// Get the child's styles
    ///
    /// Interpreted in the container's axes (see [`LayoutPartialTree::get_core_container_style`]).
    fn get_flexbox_child_style(&self, child_node_id: NodeId) -> Self::FlexboxItemStyle<'_>;
}

#[cfg(feature = "grid")]
/// Extends [`LayoutPartialTree`] with getters for the styles required for CSS Grid layout
pub trait LayoutGridContainer: LayoutPartialTree {
    /// The style type representing the CSS Grid container's styles
    type GridContainerStyle<'a>: GridContainerStyle<CustomIdent = Self::CustomIdent>
    where
        Self: 'a;

    /// The style type representing each CSS Grid item's styles
    type GridItemStyle<'a>: GridItemStyle<CustomIdent = Self::CustomIdent>
    where
        Self: 'a;

    /// Get the container's styles
    fn get_grid_container_style(&self, node_id: NodeId) -> Self::GridContainerStyle<'_>;

    /// Get the child's styles
    ///
    /// Interpreted in the container's axes (see [`LayoutPartialTree::get_core_container_style`]).
    fn get_grid_child_style(&self, child_node_id: NodeId) -> Self::GridItemStyle<'_>;

    /// Whether `node_id`'s rows are a table's (pyro): an item spanning several rows grows them
    /// as Chromium grows the rows under a rowspan cell (`DistributeExcessBlockSizeToRows`) rather than
    /// as a grid distributes a spanning item. The default is false.
    fn grid_rows_are_table_rows(&self, _node_id: NodeId) -> bool {
        false
    }

    /// pyro: a table's grid as its last full layout left it (see `TableGridState`), and the
    /// items that changed since, for `compute_grid_layout` to lay out again only the rows those are in.
    /// Asked once per layout (`RunMode::PerformLayout`) of a grid whose rows are a table's. `None`
    /// (the default): lay out the whole grid. A layout that can be done again this way offers its state
    /// to [`store_table_grid_state`](Self::store_table_grid_state).
    fn take_table_grid_state(&mut self, _node_id: NodeId) -> Option<crate::compute::grid::TableGridReuse> {
        None
    }

    /// pyro: keeps a table's grid layout for the next [`take_table_grid_state`](Self::take_table_grid_state).
    fn store_table_grid_state(&mut self, _node_id: NodeId, _state: crate::util::sys::Box<crate::compute::grid::TableGridState>) {}

    /// pyro: whether a table's cells are as wide as their columns even where those are narrower than the
    /// cells' horizontal padding and borders (the fixed table layout algorithm, as in Chromium): the grid does
    /// not floor their widths at them. The default is false.
    fn table_cells_narrower_than_padding(&self, _node_id: NodeId) -> bool {
        false
    }

    /// pyro: whether some in-flow child of a table's grid (`grid_rows_are_table_rows`) is a cell aligned on
    /// its row's baseline (see [`Self::table_cell_is_baseline_aligned`]). The default is false.
    fn table_has_baseline_cells(&self, _node_id: NodeId) -> bool {
        false
    }

    /// pyro: whether the in-flow child `child_node_id` of a table's grid is a cell aligned on its row's
    /// baseline (`vertical-align: baseline`, CSS 2.1 §17.5.3): its content moves down by its row's baseline
    /// less its own, inside its box, which still fills the row (see `table_baseline`). Asked only when
    /// [`Self::table_has_baseline_cells`] answers true.
    fn table_cell_is_baseline_aligned(&self, _node_id: NodeId, _child_node_id: NodeId) -> bool {
        false
    }

    /// pyro: how far down a cell's content is laid out in its box (`offset`, its row's baseline less its own),
    /// set before the cell's final layout in a table with baseline-aligned cells (0 for the other cells). Not
    /// called in a table without them: the tree resets the offset of a cell that stops being aligned itself.
    fn set_table_cell_baseline_offset(&mut self, _node_id: NodeId, _child_node_id: NodeId, _offset: f32) {}

    /// pyro: a baseline-aligned cell's baseline from the top of its border box, laid out with `inputs` (its
    /// columns' width), without any offset set for it: its first line box's (or row's), else the bottom of its
    /// content box when it has content; `None` for a cell without content (it takes no part). The default:
    /// `None`.
    fn table_cell_baseline(&mut self, _node_id: NodeId, _child_node_id: NodeId, _inputs: LayoutInput) -> Option<f32> {
        None
    }
    /// Whether the tree places `node_id`'s in-flow children itself (pyro: a table places its cells
    /// in rows and columns as it builds them). `Some(extent)`: the number of tracks the placed items reach from
    /// the grid's first explicit line, in each axis (`width`: columns, `height`: rows). The grid then reads each
    /// item's area from [`Self::resolved_placement`] instead of resolving named lines, estimating the grid's size
    /// and running the auto-placement algorithm over an occupancy matrix. The areas must be what that algorithm
    /// gives for the children's `grid-row`/`grid-column` styles and the container's `grid-auto-flow`, and no
    /// area may start before the first explicit line. `None` (the default): the grid places the items.
    fn track_counts(&self, _node_id: NodeId) -> Option<Size<u16>> {
        None
    }

    /// The grid area of the in-flow child `child_node_id` of `node_id`, as origin-zero lines (0 is the first
    /// explicit line; `width`: column lines, `height`: row lines). Read only when [`Self::track_counts`] answers
    /// `Some`, and then it must answer for every in-flow child.
    fn resolved_placement(&self, _node_id: NodeId, _child_node_id: NodeId) -> Option<Size<Line<u16>>> {
        None
    }

    /// Set the node's detailed grid information
    ///
    /// Implementing this method is optional. Doing so allows you to access details about the the grid such as
    /// the computed size of each grid track and the computed placement of each grid item.
    fn set_detailed_grid_info(&mut self, _node_id: NodeId, _detailed_grid_info: DetailedGridInfo<Self::CustomIdent>) {
        debug_log!("LayoutGridContainer::set_detailed_grid_info called");
    }

    /// pyro: what the parent grid of `node_id` last gave it as a subgrid ([`Self::set_subgrid_context`]).
    /// The default keeps nothing, and a grid whose template is `subgrid` then lays out as with `none`.
    fn subgrid_context(&self, _node_id: NodeId) -> Option<crate::compute::grid::SubgridContext<Self::CustomIdent>> {
        None
    }

    /// pyro: keep what a grid gives its child `node_id`, a subgrid (css-grid-2 §9): the tracks it spans, the
    /// names of their lines and, once the grid sized them, their sizes. The child's layout depends on it as on
    /// its inputs: a tree that keeps it must drop the child's cached layouts when it changes.
    fn set_subgrid_context(
        &mut self,
        _node_id: NodeId,
        _context: Option<crate::compute::grid::SubgridContext<Self::CustomIdent>>,
    ) {
    }
}

#[cfg(feature = "block_layout")]
/// Extends [`LayoutPartialTree`] with getters for the styles required for CSS Block layout
pub trait LayoutBlockContainer: LayoutPartialTree {
    /// The style type representing the CSS Block container's styles
    type BlockContainerStyle<'a>: BlockContainerStyle
    where
        Self: 'a;
    /// The style type representing each CSS Block item's styles
    type BlockItemStyle<'a>: BlockItemStyle
    where
        Self: 'a;

    /// Get the container's styles
    fn get_block_container_style(&self, node_id: NodeId) -> Self::BlockContainerStyle<'_>;

    /// Get the child's styles
    ///
    /// Interpreted in the container's axes (see [`LayoutPartialTree::get_core_container_style`]).
    fn get_block_child_style(&self, child_node_id: NodeId) -> Self::BlockItemStyle<'_>;

    /// Whether a block-level child's `auto` width fits its content rather than stretching
    /// (form controls in browsers), as for tables and replaced elements.
    fn block_child_width_fits_content(&self, child_node_id: NodeId) -> bool {
        let _ = child_node_id;
        false
    }

    /// pyro: the float area of a floated child with the given margin box size and margins, from
    /// its `shape-outside`, as rows of the margin box with the area's horizontal extent in each
    /// (see `FloatContext::shape_float`); `None` when it is the margin box. `containing_block_width`
    /// is the containing block's inline size (percentages of `shape-margin` and paddings).
    #[allow(clippy::type_complexity)]
    fn float_shape_bands(
        &self,
        child_node_id: NodeId,
        margin_box: crate::geometry::Size<f32>,
        margin: crate::geometry::Rect<f32>,
        containing_block_width: f32,
    ) -> Option<Vec<(f32, f32, Option<(f32, f32)>)>> {
        let _ = (child_node_id, margin_box, margin, containing_block_width);
        None
    }

    /// pyro: whether a block-level child that is a block container by its style establishes a
    /// formatting context of its own all the same (a multi-column container in browsers), so
    /// that this container keeps it clear of floats, as a `flow-root`.
    fn block_child_establishes_formatting_context(&self, child_node_id: NodeId) -> bool {
        let _ = child_node_id;
        false
    }

    /// pyro: the least position (its border box's top in this container's coordinates) of a
    /// block-level child: a column spanner after content that overflows the boxes before it.
    fn block_child_min_top(&self, child_node_id: NodeId) -> Option<f32> {
        let _ = child_node_id;
        None
    }

    /// pyro: whether a block-level child clears every float before it whatever its `clear` (a
    /// column spanner: the floats before it are in the row of columns it ends).
    fn block_child_clears_floats(&self, child_node_id: NodeId) -> bool {
        let _ = child_node_id;
        false
    }

    /// pyro: for a block-level child that is an inline-size container query container, the border-box
    /// width it was last styled at (its container query width; `Some(None)` before any), else `None`.
    fn block_child_container_query_width(&self, child_node_id: NodeId) -> Option<Option<f32>> {
        let _ = child_node_id;
        None
    }

    /// pyro: the top below which a container query container beside floats is placed
    /// (`set_block_child_float_floor`): placement beside floats moves forward only.
    fn block_child_float_floor(&self, child_node_id: NodeId) -> Option<f32> {
        let _ = child_node_id;
        None
    }

    /// pyro: records that a container query container is placed in the float-avoiding slot at `y` to
    /// be styled at that slot's width (the container is laid out again once its queries see it).
    fn set_block_child_float_floor(&mut self, child_node_id: NodeId, y: f32) {
        let _ = (child_node_id, y);
    }

    /// pyro: whether a block-level child is laid out in a writing mode orthogonal to this
    /// container's: it contributes no baseline set to it (css-align-3 §9.1).
    fn block_child_is_orthogonal(&self, child_node_id: NodeId) -> bool {
        let _ = child_node_id;
        false
    }

    /// pyro: whether percentage heights of the container's children resolve against the height
    /// of the container's own containing block: the container is no containing block itself (an
    /// inline box split around block-level content, laid out as a block box; CSS 2.1 § 10.1:
    /// the containing block is the nearest block container ancestor box).
    fn block_container_passes_percentage_height(&self, node_id: NodeId) -> bool {
        let _ = node_id;
        false
    }

    /// pyro: the border-box height the container's children resolve percentage heights against
    /// when its own height is not known: a multi-column container's strip, laid out as tall as its
    /// content, whose children's percentages resolve against the column height.
    fn block_container_percentage_height(&self, node_id: NodeId) -> Option<f32> {
        let _ = node_id;
        None
    }

    /// pyro: whether the container is laid out in a fragmentation context (a multi-column
    /// container's strip), where its `align-content` does not shift its content: fragmented
    /// alignment is undefined, and browsers lay the content out unaligned (css-align-3 § 5.1.1).
    fn block_container_is_fragmented(&self, node_id: NodeId) -> bool {
        let _ = node_id;
        false
    }

    /// pyro: whether the block child is a fieldset's rendered legend, which gives the fieldset no
    /// first baseline: that is its content's (HTML § 15.3.13; Chromium).
    fn block_child_is_rendered_legend(&self, child_node_id: NodeId) -> bool {
        let _ = child_node_id;
        false
    }

    /// pyro: whether a percentage-height scroll container among the container's children whose
    /// percentage cannot resolve is sized as if its height were 0 (then clamped by `min-height`)
    /// rather than `auto`: CSS Tables 3 §row-layout and Blink's restricted block size table cell
    /// children (a table cell's).
    fn block_container_restricts_percent_scrollers(&self, node_id: NodeId) -> bool {
        let _ = node_id;
        false
    }

    /// pyro: the border-box height a container of `auto` height fills at least, and that its children's
    /// `stretch` and percentage heights resolve against: Quirks Mode §3.6/§3.7 (the html element fills the
    /// viewport, the body fills the html element). `parent_size`: the container's containing block.
    fn block_container_fill_height(&self, node_id: NodeId, parent_size: crate::Size<Option<f32>>) -> Option<f32> {
        let _ = (node_id, parent_size);
        None
    }

    /// pyro: the direction of the containing block of the container's children when that is not
    /// the container (see [`Self::block_container_passes_percentage_height`]): it decides which
    /// margin an over-constrained child drops (CSS 2.1 § 10.3.3), not the container's own.
    fn block_container_direction(&self, node_id: NodeId) -> Option<crate::Direction> {
        let _ = node_id;
        None
    }

    /// pyro: what the last layout of an in-flow block-level child recorded with
    /// [`Self::set_block_flow_overlap`], where the container paints the child's parts with its
    /// own (`overlap`: boxes in it overlap; `overflows`: its content is taller than its box).
    fn block_child_flow_overlap(&self, child_node_id: NodeId) -> (bool, bool) {
        let _ = child_node_id;
        (false, false)
    }

    /// pyro: records, at the container's final layout, whether in-flow block-level boxes in it
    /// overlap in the block axis (`overlap`: a child starts above the end of one before it, or
    /// follows one whose content overflows it, or a child recorded an overlap), and whether its
    /// own in-flow content is taller than its box (`overflows`). A painter that draws box by box
    /// needs to know (CSS 2.1 Appendix E: block backgrounds first, then inline content).
    fn set_block_flow_overlap(&mut self, node_id: NodeId, overlap: bool, overflows: bool) {
        let _ = (node_id, overlap, overflows);
    }

    /// Compute the specified node's size or full layout given the specified constraints
    #[cfg(feature = "block_layout")]
    fn compute_block_child_layout(
        &mut self,
        node_id: NodeId,
        inputs: LayoutInput,
        block_ctx: Option<&mut BlockContext<'_>>,
    ) -> LayoutOutput {
        let _ = block_ctx;
        self.compute_child_layout(node_id, inputs)
    }
}

// --- PRIVATE TRAITS

/// A private trait which allows us to add extra convenience methods to types which implement
/// LayoutTree without making those methods public.
pub(crate) trait LayoutPartialTreeExt: LayoutPartialTree {
    /// Compute the size of the node given the specified constraints
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn measure_child_size(
        &mut self,
        node_id: NodeId,
        known_dimensions: Size<Option<f32>>,
        parent_size: Size<Option<f32>>,
        available_space: Size<AvailableSpace>,
        sizing_mode: SizingMode,
        axis: AbsoluteAxis,
        vertical_margins_are_collapsible: Line<bool>,
    ) -> f32 {
        self.compute_child_layout(
            node_id,
            LayoutInput {
                known_dimensions,
                known_dimensions_are_definite: Size { width: true, height: true },
                parent_size,
                available_space,
                sizing_mode,
                axis: axis.into(),
                run_mode: RunMode::ComputeSize,
                vertical_margins_are_collapsible,
            },
        )
        .size
        .get_abs(axis)
    }

    /// Compute the size of the node given the specified constraints
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn measure_child_size_both(
        &mut self,
        node_id: NodeId,
        known_dimensions: Size<Option<f32>>,
        parent_size: Size<Option<f32>>,
        available_space: Size<AvailableSpace>,
        sizing_mode: SizingMode,
        vertical_margins_are_collapsible: Line<bool>,
    ) -> Size<f32> {
        self.compute_child_layout(
            node_id,
            LayoutInput {
                known_dimensions,
                known_dimensions_are_definite: Size { width: true, height: true },
                parent_size,
                available_space,
                sizing_mode,
                axis: RequestedAxis::Both,
                run_mode: RunMode::ComputeSize,
                vertical_margins_are_collapsible,
            },
        )
        .size
    }

    /// Perform a full layout on the node given the specified constraints
    #[inline(always)]
    fn perform_child_layout(
        &mut self,
        node_id: NodeId,
        known_dimensions: Size<Option<f32>>,
        parent_size: Size<Option<f32>>,
        available_space: Size<AvailableSpace>,
        sizing_mode: SizingMode,
        vertical_margins_are_collapsible: Line<bool>,
    ) -> LayoutOutput {
        self.compute_child_layout(
            node_id,
            LayoutInput {
                known_dimensions,
                known_dimensions_are_definite: Size { width: true, height: true },
                parent_size,
                available_space,
                sizing_mode,
                axis: RequestedAxis::Both,
                run_mode: RunMode::PerformLayout,
                vertical_margins_are_collapsible,
            },
        )
    }

    /// Alias to `resolve_calc_value` with a shorter function name
    #[inline(always)]
    #[cfg(feature = "calc")]
    fn calc(&self, val: *const (), basis: f32) -> f32 {
        self.resolve_calc_value(val, basis)
    }

    /// Alias to `resolve_calc_value` with a shorter function name
    #[inline(always)]
    #[cfg(not(feature = "calc"))]
    fn calc(&self, _val: *const (), _basis: f32) -> f32 {
        0.0
    }
}

impl<T: LayoutPartialTree> LayoutPartialTreeExt for T {}
