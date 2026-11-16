//! This module is a partial implementation of the CSS Grid Level 1 specification
//! <https://www.w3.org/TR/css-grid-1>
#[cfg(feature = "content_size")]
use super::common::scrollable_overflow::{finalize_scrollable_overflow, ScrollableOverflowContainer};
use crate::geometry::{AbsoluteAxis, AbstractAxis, InBothAbsAxis};
use crate::geometry::{Line, Point, Rect, Size};
use crate::style::{AlignContent, AlignItems, AvailableSpace, Contain, ContainingBlockClaims, Overflow};
use crate::tree::{
    AxisStaticPosition, Baselines, Layout, LayoutInput, LayoutOutput, LayoutPartialTreeExt, NodeId, OofCandidate,
    OofCandidates, OofPositioningArea, RunMode, SizingMode,
};
use crate::util::debug::debug_log;
use crate::util::sys::{f32_max, f32_min, GridTrackVec, Vec};
use crate::util::MaybeMath;
use crate::util::{MaybeResolve, ResolveOrZero};
use crate::{
    style_helpers::*, BoxGenerationMode, BoxSizing, CoreStyle, Direction, GridContainerStyle, GridItemStyle,
    LayoutGridContainer, RequestedAxis,
};
use alignment::{align_and_position_item, align_tracks};
use explicit_grid::{compute_explicit_grid_size_in_axis, initialize_grid_tracks, AutoRepeatStrategy};
use implicit_grid::compute_grid_size_estimate;
use placement::{place_grid_items, ItemPlacement};
use track_sizing::{
    determine_if_item_crosses_flexible_or_intrinsic_tracks, resolve_item_track_indexes, track_sizing_algorithm,
};
use types::{CellOccupancyMatrix, GridAreasMap, GridTrack, NamedLineResolver};

use crate::sys::{DefaultCheapStr, String};
use crate::{CheapCloneStr, GridAreaAxis, GridPlacement};
use core::ops::Range;
use types::{GridItem, GridTrackKind, TrackCounts};

pub(crate) use types::{GridCoordinate, GridLine, OriginZeroLine, MAX_GRID_TRACKS, MAX_OZ_LINE, MIN_OZ_LINE};

pub use types::{GridLineNames, GridLineNamesIter};

mod alignment;
mod baseline;
mod explicit_grid;
mod implicit_grid;
mod placement;
mod subgrid;
mod table_baseline;
mod table_reuse;
mod track_sizing;
mod types;
mod util;

pub use subgrid::{OwnTracks, SubgridAxisContext, SubgridContext};
pub use table_reuse::{TableGridReuse, TableGridSplice, TableGridState};
/// The grid container's resolved box, size constraints, alignment styles and available space,
/// computed once from the container's style and the layout inputs before the grid is laid out
#[derive(Clone, Copy, Debug, PartialEq)]
struct GridContainerConstants {
    /// The container's `direction` style
    direction: Direction,
    /// The container's `contain` style
    contain: Contain,
    /// Which positioning schemes the container is a containing block for
    containing_block_claims: ContainingBlockClaims,
    /// The container's resolved padding
    padding: Rect<f32>,
    /// The container's resolved border
    border: Rect<f32>,
    /// The sum of padding and border in each axis
    padding_border_size: Size<f32>,
    /// The container's border-box min size, if definite
    min_size: Size<Option<f32>>,
    /// The container's border-box max size, if definite
    max_size: Size<Option<f32>>,
    /// The container's border-box preferred size, if definite
    preferred_size: Size<Option<f32>>,
    /// The space reserved for scrollbars in each axis
    scrollbar_gutter: Point<f32>,
    /// Whether the container's overflow makes it a scroll container
    #[cfg(feature = "content_size")]
    is_scroll_container: bool,
    /// Padding, border and scrollbar gutter combined
    content_box_inset: Rect<f32>,
    /// The container's `align-content` style
    align_content: AlignContent,
    /// The container's `justify-content` style
    justify_content: AlignContent,
    /// The container's `align-items` style
    align_items: AlignItems,
    /// The container's `justify-items` style
    justify_items: AlignItems,
    /// The space available to size the grid tracks in each axis
    available_grid_space: Size<AvailableSpace>,
    /// The container's border-box size, if definite
    outer_node_size: Size<Option<f32>>,
    /// The container's content-box min size, if definite
    inner_min_size: Size<Option<f32>>,
    /// The container's content-box max size, if definite
    inner_max_size: Size<Option<f32>>,
    /// The container's content-box size, if definite
    inner_node_size: Size<Option<f32>>,
}

/// Resolve the grid container's style against the layout inputs (step 1 of `compute_grid_layout`)
fn compute_container_constants<Tree: LayoutGridContainer>(
    tree: &Tree,
    style: &Tree::GridContainerStyle<'_>,
    inputs: LayoutInput,
) -> GridContainerConstants {
    let LayoutInput { known_dimensions, parent_size, available_space, .. } = inputs;

    let direction = style.direction();
    let contain = style.contain();
    let containing_block_claims = style.is_containing_block();

    // 1. Compute "available grid space"
    // https://www.w3.org/TR/css-grid-1/#available-grid-space
    let aspect_ratio = style.aspect_ratio();
    let padding = style.padding().resolve_or_zero(parent_size.width, |val, basis| tree.calc(val, basis));
    let border = style.border().resolve_or_zero(parent_size.width, |val, basis| tree.calc(val, basis));
    let padding_border = padding + border;
    let padding_border_size = padding_border.sum_axes();
    let box_sizing_adjustment =
        if style.box_sizing() == BoxSizing::ContentBox { padding_border_size } else { Size::ZERO };

    let min_size = style
        .min_size()
        .maybe_resolve(parent_size, |val, basis| tree.calc(val, basis))
        .maybe_apply_aspect_ratio(aspect_ratio)
        .maybe_add(box_sizing_adjustment);
    let max_size = style
        .max_size()
        .maybe_resolve(parent_size, |val, basis| tree.calc(val, basis))
        .maybe_apply_aspect_ratio(aspect_ratio)
        .maybe_add(box_sizing_adjustment);
    let preferred_size = if inputs.sizing_mode == SizingMode::InherentSize {
        style
            .size()
            .maybe_resolve(parent_size, |val, basis| tree.calc(val, basis))
            .maybe_apply_aspect_ratio(style.aspect_ratio())
            .maybe_add(box_sizing_adjustment)
    } else {
        Size::NONE
    };

    // Scrollbar gutters are reserved when the `overflow` property is set to `Overflow::Scroll`.
    // However, the axis are switched (transposed) because a node that scrolls vertically needs
    // *horizontal* space to be reserved for a scrollbar
    let scrollbar_gutter = style.overflow().transpose().map(|overflow| match overflow {
        Overflow::Scroll => style.scrollbar_width(),
        _ => 0.0,
    });
    #[cfg(feature = "content_size")]
    let is_scroll_container = {
        let overflow = style.overflow();
        overflow.x.is_scroll_container() || overflow.y.is_scroll_container()
    };
    let mut content_box_inset = padding_border;
    content_box_inset.bottom += scrollbar_gutter.y;

    match direction {
        Direction::Ltr => content_box_inset.right += scrollbar_gutter.x,
        Direction::Rtl => content_box_inset.left += scrollbar_gutter.x,
    };

    // `normal` behaves as `stretch` for grid containers
    let align_content = style.grid_align_content(AbstractAxis::Block);
    let justify_content = style.grid_align_content(AbstractAxis::Inline);
    let align_items = style.align_items();
    let justify_items = style.justify_items();
    let constrained_available_space = known_dimensions
        .or(preferred_size)
        .map(|size| size.map(AvailableSpace::Definite))
        .unwrap_or(available_space)
        .maybe_clamp(min_size, max_size)
        .maybe_max(padding_border_size);

    let available_grid_space = Size {
        width: constrained_available_space
            .width
            .map_definite_value(|space| space - content_box_inset.horizontal_axis_sum()),
        height: constrained_available_space
            .height
            .map_definite_value(|space| space - content_box_inset.vertical_axis_sum()),
    };

    let outer_node_size =
        known_dimensions.or(preferred_size).maybe_clamp(min_size, max_size).maybe_max(padding_border_size);

    // The track sizing algorithm operates on the grid container's content box, so the min/max sizes
    // (which are border-box sizes) need converting to content-box sizes before being passed to it
    let inner_min_size = min_size.maybe_sub(content_box_inset.sum_axes());
    let inner_max_size = max_size.maybe_sub(content_box_inset.sum_axes());
    let inner_node_size = Size {
        width: outer_node_size.width.map(|space| space - content_box_inset.horizontal_axis_sum()),
        height: outer_node_size.height.map(|space| space - content_box_inset.vertical_axis_sum()),
    };

    GridContainerConstants {
        direction,
        contain,
        containing_block_claims,
        padding,
        border,
        padding_border_size,
        min_size,
        max_size,
        preferred_size,
        scrollbar_gutter,
        #[cfg(feature = "content_size")]
        is_scroll_container,
        content_box_inset,
        align_content,
        justify_content,
        align_items,
        justify_items,
        available_grid_space,
        outer_node_size,
        inner_min_size,
        inner_max_size,
        inner_node_size,
    }
}

/// Grid layout algorithm
/// This consists of a few phases:
///   - Resolving the explicit grid
///   - Placing items (which also resolves the implicit grid)
///   - Track (row/column) sizing
///   - Alignment & Final item placement
pub fn compute_grid_layout<Tree: LayoutGridContainer>(
    tree: &mut Tree,
    node: NodeId,
    inputs: LayoutInput,
) -> LayoutOutput {
    let LayoutInput { known_dimensions, run_mode, .. } = inputs;

    let table_rows = tree.grid_rows_are_table_rows(node);
    // pyro: a table's last layout, to lay out again only what changed since (`table_reuse`).
    let reuse = if table_rows && run_mode == RunMode::PerformLayout { tree.take_table_grid_state(node) } else { None };
    let style = tree.get_grid_container_style(node);
    let constants = compute_container_constants(tree, &style, inputs);
    let GridContainerConstants {
        direction,
        contain,
        containing_block_claims,
        padding,
        border,
        padding_border_size,
        min_size,
        max_size,
        preferred_size,
        scrollbar_gutter,
        #[cfg(feature = "content_size")]
        is_scroll_container,
        content_box_inset,
        align_content,
        justify_content,
        align_items,
        justify_items,
        available_grid_space,
        outer_node_size,
        inner_min_size,
        inner_max_size,
        mut inner_node_size,
    } = constants;

    // pyro: the tracks this grid takes from its parent grid as a subgrid (css-grid-2 §9), in which
    // `justify-content`/`align-content` do not apply: the subgrid's grid is aligned with its parent's.
    let own_subgrid = if table_rows { None } else { subgrid::own_context(tree, node, &style) };
    let justify_content = if own_subgrid.as_ref().is_some_and(|c| c.columns.is_some()) { AlignContent::START } else { justify_content };
    let align_content = if own_subgrid.as_ref().is_some_and(|c| c.rows.is_some()) { AlignContent::START } else { align_content };

    // Note: we avoid accessing the grid rows/columns methods more than once as this can
    // cause an expensive-ish computation
    let grid_template_columns = style.grid_template_columns();
    let grid_template_rows = style.grid_template_rows();
    let grid_auto_columns = style.grid_auto_columns();
    let grid_auto_rows = style.grid_auto_rows();

    debug_log!("parent_size", dbg:inputs.parent_size);
    debug_log!("outer_node_size", dbg:outer_node_size);
    debug_log!("inner_node_size", dbg:inner_node_size);

    // Short-circuit layout if the container's size is fully determined by the container's size and the run mode
    // is ComputeSize (and thus the container's size is all that we're interested in)
    if run_mode == RunMode::ComputeSize {
        if let Size { width: Some(width), height: Some(height) } = outer_node_size {
            return LayoutOutput::from_outer_size(Size { width, height });
        }

        // We can also short-circuit if the width is known and only the width has been requested.
        if inputs.axis == RequestedAxis::Horizontal {
            if let Some(width) = outer_node_size.width {
                return LayoutOutput::from_outer_size(Size { width, height: 0.0 });
            }
        }
    }

    // 2. Resolve the explicit grid

    // This is very similar to the inner_node_size except if the inner_node_size is not definite but the node
    // has a min- or max- size style then that will be used in it's place.
    let auto_fit_container_size = outer_node_size
        .or(max_size)
        .or(min_size)
        .maybe_clamp(min_size, max_size)
        .maybe_max(padding_border_size)
        .maybe_sub(content_box_inset.sum_axes());

    // If the grid container has a definite size or max size in the relevant axis:
    //   - then the number of repetitions is the largest possible positive integer that does not cause the grid to overflow the content
    //     box of its grid container.
    // Otherwise, if the grid container has a definite min size in the relevant axis:
    //   - then the number of repetitions is the smallest possible positive integer that fulfills that minimum requirement
    // Otherwise, the specified track list repeats only once.
    let auto_repeat_fit_strategy = outer_node_size.or(max_size).map(|val| match val {
        Some(_) => AutoRepeatStrategy::MaxRepetitionsThatDoNotOverflow,
        None => AutoRepeatStrategy::MinRepetitionsThatDoOverflow,
    });

    // Compute the number of rows and columns in the explicit grid *template*
    // (explicit tracks from grid_areas are computed separately below)
    // pyro: a subgridded axis has as many explicit tracks as the subgrid spans in its parent.
    let subgrid_columns = subgrid::subgrid_track_count(own_subgrid.as_ref(), AbsoluteAxis::Horizontal);
    let subgrid_rows = subgrid::subgrid_track_count(own_subgrid.as_ref(), AbsoluteAxis::Vertical);
    let (col_auto_repetition_count, grid_template_col_count) = match subgrid_columns {
        Some(count) => (0, count),
        None => compute_explicit_grid_size_in_axis(
            &style,
            auto_fit_container_size.width,
            auto_repeat_fit_strategy.width,
            |val, basis| tree.calc(val, basis),
            AbsoluteAxis::Horizontal,
        ),
    };
    let (row_auto_repetition_count, grid_template_row_count) = match subgrid_rows {
        Some(count) => (0, count),
        None => compute_explicit_grid_size_in_axis(
            &style,
            auto_fit_container_size.height,
            auto_repeat_fit_strategy.height,
            |val, basis| tree.calc(val, basis),
            AbsoluteAxis::Vertical,
        ),
    };

    // type CustomIdent<'a> = <<Tree as LayoutPartialTree>::CoreContainerStyle<'_> as CoreStyle>::CustomIdent;
    let mut name_resolver = NamedLineResolver::new(&style, col_auto_repetition_count, row_auto_repetition_count);

    // Clamp the explicit grid to MAX_GRID_TRACKS tracks in each axis
    // https://www.w3.org/TR/css-grid-1/#overlarge-grids
    let explicit_col_count = subgrid_columns
        .unwrap_or_else(|| grid_template_col_count.max(name_resolver.area_column_count()).min(MAX_GRID_TRACKS));
    let explicit_row_count = subgrid_rows
        .unwrap_or_else(|| grid_template_row_count.max(name_resolver.area_row_count()).min(MAX_GRID_TRACKS));

    name_resolver.set_explicit_column_count(explicit_col_count);
    name_resolver.set_explicit_row_count(explicit_row_count);
    if let Some(context) = &own_subgrid {
        subgrid::add_subgrid_line_names(&mut name_resolver, &style, context);
    }

    // Build the per-line names of the explicit grid from the name resolver's collected pairs
    let mut detailed_column_line_names = name_resolver.detailed_line_names(AbsoluteAxis::Horizontal);
    let mut detailed_row_line_names = name_resolver.detailed_line_names(AbsoluteAxis::Vertical);

    // 3. Create the grid items (in document order) and resolve the named lines in their `grid-row`/`grid-column` styles
    // Absolutely positioned children do not take part in grid placement and do not create implicit tracks,
    // so they are excluded here. Each child's style is read and its named lines resolved exactly once; the
    // results drive both the grid size estimate and item placement below.
    //
    // pyro: a tree that places the items itself (a table, which places its cells as it builds them)
    // answers each item's area instead. Named-line resolution, the size estimate and auto-placement are then
    // skipped (see `LayoutGridContainer::track_counts`).
    let tree_extent = tree.track_counts(node);
    let child_count = tree.child_count(node);
    // pyro: nothing changed since the table's last layout and the inputs differ at most in
    // the available width (its container got wider or narrower, the table not): that layout stands.
    let reuse = match reuse {
        Some(reuse)
            if table_reuse::stands::<Tree>(
                &reuse,
                &style,
                &inputs,
                &constants,
                child_count,
                explicit_col_count,
                explicit_row_count,
                col_auto_repetition_count,
                row_auto_repetition_count,
            ) =>
        {
            drop(name_resolver);
            drop(grid_template_rows);
            drop(grid_template_columns);
            drop(grid_auto_rows);
            drop(grid_auto_columns);
            drop(style);
            let mut state = reuse.state;
            state.stood = true;
            let output = state.output.clone();
            tree.store_table_grid_state(node, state);
            return output;
        }
        reuse => reuse,
    };
    let mut replay = reuse.and_then(|reuse| {
        table_reuse::prepare(
            tree,
            node,
            &style,
            reuse,
            inputs,
            constants,
            child_count,
            &name_resolver,
            explicit_col_count,
            explicit_row_count,
            col_auto_repetition_count,
            row_auto_repetition_count,
            align_items,
            justify_items,
            style.grid_auto_flow(),
        )
    });
    let (mut items, final_col_counts, final_row_counts, mut columns, mut rows) = match replay.as_mut() {
        Some(replay) => (
            core::mem::take(&mut replay.items),
            replay.col_counts,
            replay.row_counts,
            core::mem::take(&mut replay.columns),
            core::mem::take(&mut replay.rows),
        ),
        None => {
        let mut items: Vec<GridItem> = Vec::with_capacity(child_count);
        let mut placements: Vec<ItemPlacement> =
            Vec::with_capacity(if tree_extent.is_some() { 0 } else { child_count });
        for (index, child_node) in tree.child_ids(node).enumerate() {
            let child_style = tree.get_grid_child_style(child_node);
            if child_style.box_generation_mode() == BoxGenerationMode::None || child_style.position().is_out_of_flow() {
                continue;
            }
            let mut child_subgrid = Size { width: false, height: false };
            let tree_area = match tree_extent {
                Some(_) => Some(
                    tree.resolved_placement(node, child_node)
                        .expect("a tree answering track_counts answers resolved_placement for every in-flow child"),
                ),
                None => {
                    // pyro: a subgrid child's automatic span is the number of tracks its line names name.
                    let (placement, subgrid) = subgrid::child_placement(
                        &*tree,
                        child_node,
                        &child_style,
                        &name_resolver,
                        Size { width: explicit_col_count, height: explicit_row_count },
                    );
                    placements.push(placement);
                    child_subgrid = subgrid;
                    None
                }
            };
            let mut item =
                GridItem::new_with_style_and_order(child_node, child_style, align_items, justify_items, index as u16);
            item.subgrid = child_subgrid;
            if let Some(area) = tree_area {
                item.column = area.width.map(|line| OriginZeroLine(line as i16));
                item.row = area.height.map(|line| OriginZeroLine(line as i16));
            }
            items.push(item);
        }

        // pyro: a subgrid's items are clamped to its explicit grid in its subgridded axes before auto-placement.
        if let Some(context) = &own_subgrid {
            subgrid::clamp_placements(&mut placements, context);
        }

        // 4. Implicit Grid: Estimate Track Counts
        // 5. Grid Item Placement
        // Match items (children) to a definite grid position (row start/end and column start/end position).
        // `items` is in document order from here on (placement only fills in each item's grid area). The track
        // sizing and baseline passes sort references to the items rather than the items themselves.
        let (final_col_counts, final_row_counts, cell_occupancy_matrix) = match tree_extent {
            Some(extent) => {
                // The tree's items start at the first explicit line, so there are no negative implicit tracks, and
                // the items reach `extent` tracks: the implicit grid is whatever of that lies past the explicit grid.
                let counts =
                    |explicit: u16, extent: u16| TrackCounts::from_raw(0, explicit, extent.saturating_sub(explicit));
                let col_counts = counts(explicit_col_count, extent.width);
                let row_counts = counts(explicit_row_count, extent.height);
                #[cfg(debug_assertions)]
                debug_assert_tree_placement(
                    tree,
                    node,
                    &name_resolver,
                    (explicit_col_count, explicit_row_count),
                    style.grid_auto_flow(),
                    &items,
                    (col_counts, row_counts),
                );
                (col_counts, row_counts, None)
            }
            None => {
                // Estimate the number of rows and columns in the implicit grid (= the entire grid)
                // This is necessary as part of placement. Doing it early here is a perf optimisation to reduce allocations.
                let (est_col_counts, est_row_counts) =
                    compute_grid_size_estimate(explicit_col_count, explicit_row_count, &placements);
                let mut cell_occupancy_matrix = CellOccupancyMatrix::with_track_counts(est_col_counts, est_row_counts);
                place_grid_items(&mut cell_occupancy_matrix, &mut items, &placements, style.grid_auto_flow());
                // Extract track counts from previous step (auto-placement can expand the number of tracks)
                let mut final_col_counts = *cell_occupancy_matrix.track_counts(AbsoluteAxis::Horizontal);
                let mut final_row_counts = *cell_occupancy_matrix.track_counts(AbsoluteAxis::Vertical);
                // pyro: a subgrid has no implicit tracks in a subgridded axis: its items are clamped to it.
                if let Some(context) = &own_subgrid {
                    subgrid::clamp_items(&mut items, context);
                    if let Some(count) = subgrid_columns {
                        final_col_counts = TrackCounts::from_raw(0, count, 0);
                    }
                    if let Some(count) = subgrid_rows {
                        final_row_counts = TrackCounts::from_raw(0, count, 0);
                    }
                }
                (final_col_counts, final_row_counts, Some(cell_occupancy_matrix))
            }
        };
        // Whether a track has items, for collapsing empty `auto-fit` tracks. Without an occupancy matrix (the tree
        // placed the items, starting at track 0) a track has items when an item's area covers it.
        let track_has_items = |index: usize, axis: AbsoluteAxis| match &cell_occupancy_matrix {
            Some(matrix) => match axis {
                AbsoluteAxis::Horizontal => matrix.column_is_occupied(index),
                AbsoluteAxis::Vertical => matrix.row_is_occupied(index),
            },
            None => items.iter().any(|item| {
                let area = match axis {
                    AbsoluteAxis::Horizontal => item.column,
                    AbsoluteAxis::Vertical => item.row,
                };
                (area.start.0 as isize) <= index as isize && (index as isize) < area.end.0 as isize
            }),
        };

        // 5. Initialize Tracks
        // Initialize (explicit and implicit) grid tracks (and gutters)
        // This resolves the min and max track sizing functions for all tracks and gutters
        let mut columns = GridTrackVec::new();
        let mut rows = GridTrackVec::new();
        initialize_grid_tracks(
            &mut columns,
            final_col_counts,
            &style,
            AbsoluteAxis::Horizontal,
            col_auto_repetition_count,
            |column_index| track_has_items(column_index, AbsoluteAxis::Horizontal),
        );
        initialize_grid_tracks(
            &mut rows,
            final_row_counts,
            &style,
            AbsoluteAxis::Vertical,
            row_auto_repetition_count,
            |row_index| track_has_items(row_index, AbsoluteAxis::Vertical),
        );
        // pyro: a subgridded axis's tracks are fixed at the sizes the parent gave.
        if let Some(context) = &own_subgrid {
            if let Some(axis) = &context.columns {
                subgrid::subgrid_tracks(&mut columns, axis);
            }
            if let Some(axis) = &context.rows {
                subgrid::subgrid_tracks(&mut rows, axis);
            }
        }


        // 6. Track Sizing

        // Convert grid placements in origin-zero coordinates to indexes into the GridTrack (rows and columns) vectors
        // This computation is relatively trivial, but it requires the final number of negative (implicit) tracks in
        // each axis, and doing it up-front here means we don't have to keep repeating that calculation
        resolve_item_track_indexes(&mut items, final_col_counts, final_row_counts);
        // For each item, and in each axis, determine whether the item crosses any flexible (fr) tracks
        // Record this as a boolean (per-axis) on each item for later use in the track-sizing algorithm
        determine_if_item_crosses_flexible_or_intrinsic_tracks(&mut items, &columns, &rows, None);
            (items, final_col_counts, final_row_counts, columns, rows)
        }
    };
    // pyro: the gaps of this grid, for its subgrid children (in a subgridded axis, the gap it was given).
    let own_gap = {
        let gap = style.gap();
        let calc = |val, basis| tree.calc(val, basis);
        Size {
            width: own_subgrid
                .as_ref()
                .and_then(|c| c.columns.as_ref())
                .map_or_else(|| gap.width.resolve_or_zero(inner_node_size.width, calc), |c| c.gap),
            height: own_subgrid
                .as_ref()
                .and_then(|c| c.rows.as_ref())
                .map_or_else(|| gap.height.resolve_or_zero(inner_node_size.height, calc), |c| c.gap),
        }
    };
    drop(grid_template_rows);
    drop(grid_template_columns);
    drop(grid_auto_rows);
    drop(grid_auto_columns);
    // pyro: a preferred aspect ratio (between content boxes for `box-sizing: content-box`), and whether the
    // automatic minimum height applies (`min-height: auto`, no scroll container): for an `auto` height
    let aspect_ratio = style.aspect_ratio().map(|ratio| {
        let adjustment = if style.box_sizing() == BoxSizing::ContentBox { padding_border_size } else { Size::ZERO };
        let overflow = style.overflow();
        let content_minimum = style.min_size().height.is_auto() && !overflow.x.is_scroll_container() && !overflow.y.is_scroll_container();
        (ratio, adjustment, content_minimum)
    });
    let baseline_synthesis = style.baseline_synthesis();
    drop(style);

    // pyro: subgrid children (css-grid-2 §9): their contexts, and their items to size this grid's tracks
    // in their place (appended to `items` until the tracks are sized).
    let own_item_count = items.len();
    let subgrid_parent = subgrid::SubgridParent {
        names: &name_resolver,
        gap: own_gap,
        direction,
        percent_basis: inner_node_size.width,
    };
    let has_subgrid_items = !table_rows && replay.is_none() && items.iter().any(|item| item.subgrid.width || item.subgrid.height);
    if !table_rows && replay.is_none() {
        subgrid::prepare_subgrid_items(tree, &mut items, &subgrid_parent);
        if items.len() > own_item_count {
            resolve_item_track_indexes(&mut items[own_item_count..], final_col_counts, final_row_counts);
            determine_if_item_crosses_flexible_or_intrinsic_tracks(&mut items[own_item_count..], &columns, &rows, None);
        }
    }

    // pyro: a fixed table's cells are as wide as their columns.
    if table_rows && tree.table_cells_narrower_than_padding(node) {
        items.iter_mut().for_each(|item| item.width_unfloored = true);
    }

    // Determine if the grid has any baseline aligned items
    let has_baseline_aligned_item = !table_rows
        && items.iter().any(|item| {
            item.participates_in_baseline_alignment() || item.participates_in_inline_baseline_alignment()
        });
    let baseline_params = |other_axis_known: bool| {
        has_baseline_aligned_item.then_some(track_sizing::BaselineParams {
            synthesis: baseline_synthesis,
            direction,
            other_axis_known,
        })
    };

    // pyro: the heights of the items of subgrids in their columns only, in their own rows.
    if has_subgrid_items && replay.is_none() {
        subgrid::resolve_standalone_heights(tree, &mut items, own_item_count);
    }

    // Run track sizing algorithm for Inline axis
    // (A table laid out again keeps its columns: they depend on no item.)
    let inner_width_is_definite = inner_node_size.width.is_some();
    if replay.is_none() {
        track_sizing_algorithm(
            tree,
            AbstractAxis::Inline,
            inner_min_size.get(AbstractAxis::Inline),
            inner_max_size.get(AbstractAxis::Inline),
            justify_content,
            align_content,
            available_grid_space,
            inner_node_size,
            &mut columns,
            &mut rows,
            &mut items,
            |track: &GridTrack, parent_size: Option<f32>, tree: &Tree| {
                track.max_track_sizing_function.definite_value(parent_size, |val, basis| tree.calc(val, basis))
            },
            baseline_params(false),
            false,
            None,
            None,
        );
    }
    if has_subgrid_items {
        subgrid::update_subgrid_track_sizes(tree, &items[..own_item_count], AbsoluteAxis::Horizontal, &columns, false, &subgrid_parent);
    }
    // pyro: a subgrid in its rows only keeps its columns for its parent's row sizing.
    let records_own_columns = own_subgrid.as_ref().is_some_and(|c| c.rows.is_some() && c.columns.is_none());
    if records_own_columns {
        if let Some(width) = inner_node_size.width {
            subgrid::record_own_columns(tree, node, &columns, width);
        }
    }
    let initial_column_sum = columns.iter().map(|track| track.base_size).sum::<f32>();
    inner_node_size.width = inner_node_size.width.or_else(|| initial_column_sum.into());

    // If only the container's width has been requested then we can skip sizing the rows entirely (which avoids
    // measuring the height of every item), as column sizing is not re-run when only computing the container's size.
    // Note: the short-circuit above has already handled the case where the container's width is known.
    if run_mode == RunMode::ComputeSize && inputs.axis == RequestedAxis::Horizontal {
        let width = (initial_column_sum + content_box_inset.horizontal_axis_sum())
            .maybe_clamp(min_size.width, max_size.width)
            .max(padding_border_size.width);
        return LayoutOutput::from_outer_size(Size { width, height: 0.0 });
    }

    // (A table laid out again sizes only the items of its affected rows: theirs.)
    match replay.as_ref() {
        Some(replay) => items
            .iter_mut()
            .filter(|item| replay.affected_tracks[item.row_indexes.start as usize + 1])
            .for_each(|item| item.grid_area_size_cache = None),
        None => items.iter_mut().for_each(|item| item.grid_area_size_cache = None),
    }

    // Run track sizing algorithm for Block axis
    // pyro: a table's cells aligned on their row's baseline (`table_baseline`).
    let table_baselines = table_rows && table_baseline::mark_table_baseline_cells(tree, node, &mut items);
    if table_baselines {
        table_baseline::resolve_table_cell_baselines(tree, node, &mut items, &rows, &columns, inner_node_size);
    }

    // pyro: the widths of the items of subgrids in their rows only, in their own columns.
    if has_subgrid_items {
        subgrid::resolve_standalone_widths(tree, &mut items, own_item_count, &columns, &subgrid_parent);
    }
    // (A table laid out again sizes the rows of the items that changed from their items.)
    let mut rows_sized = table_rows.then(GridTrackVec::new);
    track_sizing_algorithm(
        tree,
        AbstractAxis::Block,
        inner_min_size.get(AbstractAxis::Block),
        inner_max_size.get(AbstractAxis::Block),
        align_content,
        justify_content,
        available_grid_space,
        inner_node_size,
        &mut rows,
        &mut columns,
        &mut items,
        |track: &GridTrack, _, _| Some(track.base_size),
        baseline_params(true),
        table_rows,
        replay.as_ref().map(|replay| replay.affected_tracks.as_slice()),
        rows_sized.as_mut(),
    );
    if has_subgrid_items {
        subgrid::update_subgrid_track_sizes(tree, &items[..own_item_count], AbsoluteAxis::Vertical, &rows, false, &subgrid_parent);
    }
    let initial_row_sum = rows.iter().map(|track| track.base_size).sum::<f32>();
    // pyro: a subgrid in its columns only keeps its rows for its parent's column sizing.
    if own_subgrid.as_ref().is_some_and(|c| c.columns.is_some() && c.rows.is_none()) {
        subgrid::record_own_rows(tree, node, &rows, inner_node_size.height.unwrap_or(initial_row_sum));
    }

    debug_log!("initial_column_sum", dbg:initial_column_sum);
    debug_log!(dbg: columns.iter().map(|track| track.base_size).collect::<Vec<_>>());
    debug_log!("initial_row_sum", dbg:initial_row_sum);
    debug_log!(dbg: rows.iter().map(|track| track.base_size).collect::<Vec<_>>());

    // 6. Compute container size
    let resolved_style_size = known_dimensions.or(preferred_size);
    let width = resolved_style_size
        .get(AbstractAxis::Inline)
        .unwrap_or_else(|| initial_column_sum + content_box_inset.horizontal_axis_sum())
        .maybe_clamp(min_size.width, max_size.width)
        .max(padding_border_size.width);
    // pyro: an `auto` height with a preferred aspect ratio is the width's through it (css-sizing-4 §5.1), at
    // least the rows' (its automatic minimum size) unless `min-height` is set or it is a scroll container
    let rows_height = initial_row_sum + content_box_inset.vertical_axis_sum();
    let auto_height = match aspect_ratio {
        Some((ratio, adjustment, content_minimum)) => {
            let height = (width - adjustment.width).max(0.0) / ratio + adjustment.height;
            if content_minimum {
                height.max(rows_height)
            } else {
                height
            }
        }
        None => rows_height,
    };
    let mut container_border_box = Size {
        width,
        height: resolved_style_size
            .get(AbstractAxis::Block)
            .unwrap_or(auto_height)
            .maybe_clamp(min_size.height, max_size.height)
            .max(padding_border_size.height),
    };
    // pyro: a table's height is a minimum (CSS 2.1 §17.5.3): rows taller than it (a `height`, or a
    // height its flex or grid parent gave it) make the table taller.
    if table_rows {
        container_border_box.height =
            container_border_box.height.max(initial_row_sum + content_box_inset.vertical_axis_sum());
    }
    let mut container_content_box = Size {
        width: f32_max(0.0, container_border_box.width - content_box_inset.horizontal_axis_sum()),
        height: f32_max(0.0, container_border_box.height - content_box_inset.vertical_axis_sum()),
    };

    // If only the container's size has been requested
    if run_mode == RunMode::ComputeSize {
        return LayoutOutput::from_outer_size(container_border_box);
    }

    // The container's size is now determined, so percentages resolve against its content box when re-running track sizing.
    // This may be a no-op if the container's size was known upfront (in which case it already equals `inner_node_size`).
    inner_node_size = container_content_box.map(Some);

    // Column sizing must be re-run (once) if:
    //   - The grid container's width was initially indefinite and there are any columns with percentage track sizing functions
    //   - Any grid item crossing an intrinsically sized track's min content contribution width has changed
    // TODO: Only rerun sizing for tracks that actually require it rather than for all tracks if any need it.
    let mut rerun_column_sizing;
    let mut intrinsic_column_contribution_changed = false;

    let has_percentage_column = columns.iter().any(|track| track.uses_percentage());
    let has_percentage_row = rows.iter().any(|track| track.uses_percentage());
    let parent_width_indefinite = outer_node_size.width.is_none();
    rerun_column_sizing = parent_width_indefinite && has_percentage_column;

    if !rerun_column_sizing {
        // Note: every item must be visited (no short-circuiting) as the closure updates each item's caches
        intrinsic_column_contribution_changed =
            items.iter_mut().filter(|item| item.crosses_intrinsic_column && !item.excluded.width).fold(false, |any_changed, item| {
                // pyro: the first column pass read nothing of this item that depends on its content or
                // grid area (an `fr` column it does not size, with `min-width: 0` say), so the columns cannot
                // change with it and measuring it here would only force a re-run on the empty cache. A definite
                // container width is required as well: with an indefinite one the re-run expands `fr` tracks
                // into the width found by the first pass, which the item does not take part in either, but is
                // left as upstream had it. Its caches are still reset in case another item forces the re-run.
                if !item.inline_contribution_consumed && !parent_width_indefinite {
                    item.grid_area_size_cache = None;
                    item.min_content_contribution_cache.width = None;
                    item.max_content_contribution_cache.width = None;
                    item.minimum_contribution_cache.width = None;
                    return any_changed;
                }
                let grid_area_size = item.grid_area_size(
                    AbstractAxis::Inline,
                    &columns,
                    &rows,
                    inner_node_size,
                    |track: &GridTrack, _| Some(track.base_size),
                    &|val, basis| tree.calc(val, basis),
                );
                let available_space = grid_area_size.with(AbstractAxis::Inline, None);
                let new_min_content_contribution =
                    item.min_content_contribution(AbstractAxis::Inline, tree, grid_area_size, available_space);

                let has_changed = Some(new_min_content_contribution) != item.min_content_contribution_cache.width;

                item.grid_area_size_cache = Some(grid_area_size);
                item.min_content_contribution_cache.width = Some(new_min_content_contribution);
                item.max_content_contribution_cache.width = None;
                item.minimum_contribution_cache.width = None;

                any_changed | has_changed
            });
        rerun_column_sizing = intrinsic_column_contribution_changed;
    } else {
        // Clear intrinsic width caches
        items.iter_mut().for_each(|item| {
            item.grid_area_size_cache = None;
            item.min_content_contribution_cache.width = None;
            item.max_content_contribution_cache.width = None;
            item.minimum_contribution_cache.width = None;
        });
    }

    // Row sizing must be re-run (once) if:
    //   - The grid container's height was initially indefinite and there are any rows with percentage track sizing functions
    //   - Column sizing was re-run and any grid item crossing an intrinsically sized track's min content contribution height has changed
    // TODO: Only rerun sizing for tracks that actually require it rather than for all tracks if any need it.
    let parent_height_indefinite = outer_node_size.height.is_none();
    let mut rerun_row_sizing = parent_height_indefinite && has_percentage_row;
    let mut intrinsic_row_contribution_changed = false;

    if rerun_row_sizing {
        // Clear intrinsic height caches
        items.iter_mut().for_each(|item| {
            item.grid_area_size_cache = None;
            item.min_content_contribution_cache.height = None;
            item.max_content_contribution_cache.height = None;
            item.minimum_contribution_cache.height = None;
        });
    }

    let sized_again = rerun_column_sizing || rerun_row_sizing;
    if rerun_column_sizing {
        // Re-run track sizing algorithm for Inline axis
        track_sizing_algorithm(
            tree,
            AbstractAxis::Inline,
            inner_min_size.get(AbstractAxis::Inline),
            inner_max_size.get(AbstractAxis::Inline),
            justify_content,
            align_content,
            available_grid_space,
            inner_node_size,
            &mut columns,
            &mut rows,
            &mut items,
            |track: &GridTrack, _, _| Some(track.base_size),
            baseline_params(true),
            false,
            None,
            None,
        );
        if has_subgrid_items {
            subgrid::update_subgrid_track_sizes(tree, &items[..own_item_count], AbsoluteAxis::Horizontal, &columns, false, &subgrid_parent);
            subgrid::resolve_standalone_widths(tree, &mut items, own_item_count, &columns, &subgrid_parent);
        }
        if records_own_columns {
            if let Some(width) = inner_node_size.width {
                subgrid::record_own_columns(tree, node, &columns, width);
            }
        }

        // The column widths may have changed, so the items' intrinsic height contributions need to be recomputed
        // (unless row sizing is being re-run anyway, in which case the caches have already been cleared).
        if !rerun_row_sizing {
            // Note: every item must be visited (no short-circuiting) as the closure updates each item's caches
            intrinsic_row_contribution_changed =
                items.iter_mut().filter(|item| item.crosses_intrinsic_row && !item.excluded.height).fold(false, |any_changed, item| {
                    let grid_area_size = item.grid_area_size(
                        AbstractAxis::Block,
                        &rows,
                        &columns,
                        inner_node_size,
                        |track: &GridTrack, _| Some(track.base_size),
                        &|val, basis| tree.calc(val, basis),
                    );
                    let available_space = grid_area_size.with(AbstractAxis::Block, None);
                    let new_min_content_contribution =
                        item.min_content_contribution(AbstractAxis::Block, tree, grid_area_size, available_space);

                    let has_changed = Some(new_min_content_contribution) != item.min_content_contribution_cache.height;

                    item.grid_area_size_cache = Some(grid_area_size);
                    item.min_content_contribution_cache.height = Some(new_min_content_contribution);
                    item.max_content_contribution_cache.height = None;
                    item.minimum_contribution_cache.height = None;

                    any_changed | has_changed
                });
            rerun_row_sizing = intrinsic_row_contribution_changed;
        }
    }

    if rerun_row_sizing {
        // pyro: the cells' baselines at the columns' new widths (`table_baseline`).
        if table_baselines && rerun_column_sizing {
            table_baseline::resolve_table_cell_baselines(tree, node, &mut items, &rows, &columns, inner_node_size);
        }
        // Re-run track sizing algorithm for Block axis
        track_sizing_algorithm(
            tree,
            AbstractAxis::Block,
            inner_min_size.get(AbstractAxis::Block),
            inner_max_size.get(AbstractAxis::Block),
            align_content,
            justify_content,
            available_grid_space,
            inner_node_size,
            &mut rows,
            &mut columns,
            &mut items,
            |track: &GridTrack, _, _| Some(track.base_size),
            baseline_params(true),
            table_rows,
            None,
            None,
        );
        if has_subgrid_items {
            subgrid::update_subgrid_track_sizes(tree, &items[..own_item_count], AbsoluteAxis::Vertical, &rows, false, &subgrid_parent);
        }
    }

    if (intrinsic_column_contribution_changed && !has_percentage_column)
        || (intrinsic_row_contribution_changed && !has_percentage_row)
    {
        let final_column_sum = columns.iter().map(|track| track.base_size).sum::<f32>();
        let final_row_sum = rows.iter().map(|track| track.base_size).sum::<f32>();

        if intrinsic_column_contribution_changed && !has_percentage_column {
            container_border_box.width = resolved_style_size
                .get(AbstractAxis::Inline)
                .unwrap_or_else(|| final_column_sum + content_box_inset.horizontal_axis_sum())
                .maybe_clamp(min_size.width, max_size.width)
                .max(padding_border_size.width);
            container_content_box.width =
                f32_max(0.0, container_border_box.width - content_box_inset.horizontal_axis_sum());
        }

        if intrinsic_row_contribution_changed && !has_percentage_row {
            container_border_box.height = resolved_style_size
                .get(AbstractAxis::Block)
                .unwrap_or_else(|| final_row_sum + content_box_inset.vertical_axis_sum())
                .maybe_clamp(min_size.height, max_size.height)
                .max(padding_border_size.height);
            container_content_box.height =
                f32_max(0.0, container_border_box.height - content_box_inset.vertical_axis_sum());
        }
    }
    // pyro: as above, after rows sized again
    if table_rows {
        let rows = rows.iter().map(|track| track.base_size).sum::<f32>() + content_box_inset.vertical_axis_sum();
        if container_border_box.height < rows {
            container_border_box.height = rows;
            container_content_box.height = f32_max(0.0, rows - content_box_inset.vertical_axis_sum());
        }
    }

    // If only the container's size has been requested
    if run_mode == RunMode::ComputeSize {
        return LayoutOutput::from_outer_size(container_border_box);
    }

    // 8. Track Alignment

    // Align columns
    let inline_size_without_scrollbar = f32_max(container_border_box.width - padding_border_size.width, 0.0);
    let inline_scrollbar_gutter_for_alignment = f32_min(scrollbar_gutter.x, inline_size_without_scrollbar);
    align_tracks(
        container_content_box.get(AbstractAxis::Inline),
        Line {
            start: padding.left + if direction.is_rtl() { inline_scrollbar_gutter_for_alignment } else { 0.0 },
            end: padding.right + if direction.is_rtl() { 0.0 } else { inline_scrollbar_gutter_for_alignment },
        },
        Line { start: border.left, end: border.right },
        &mut columns,
        justify_content,
        direction.is_rtl(),
    );
    // Align rows
    align_tracks(
        container_content_box.get(AbstractAxis::Block),
        Line { start: padding.top, end: padding.bottom },
        Line { start: border.top, end: border.bottom },
        &mut rows,
        align_content,
        false,
    );

    // pyro: a subgrid's tracks where its parent's lines put them (a track can end before it starts).
    if let Some(context) = &own_subgrid {
        if let Some(axis) = &context.columns {
            let content_left = border.left + padding.left + if direction.is_rtl() { scrollbar_gutter.x } else { 0.0 };
            subgrid::place_subgrid_tracks(&mut columns, axis, content_left, container_content_box.width, direction.is_rtl());
        }
        if let Some(axis) = &context.rows {
            subgrid::place_subgrid_tracks(&mut rows, axis, border.top + padding.top, container_content_box.height, false);
        }
    }

    // pyro: the baseline shims with the final track sizes (an item's baselines can depend on both), with the
    // parent's group baselines in a subgrid's subgridded axes; a subgrid's items here give it this grid's.
    if has_baseline_aligned_item && replay.is_none() {
        for axis in [AbstractAxis::Inline, AbstractAxis::Block] {
            let parent_axis = own_subgrid.as_ref().and_then(|context| match axis {
                AbstractAxis::Inline => context.columns.as_ref(),
                AbstractAxis::Block => context.rows.as_ref(),
            });
            let parent_baselines = parent_axis.and_then(|sub| sub.baselines.clone());
            let mut synthesis = baseline_synthesis;
            if let Some(parent_synthesis) = parent_axis.and_then(|sub| sub.baseline_synthesis) {
                synthesis.set(axis, parent_synthesis);
            }
            baseline::resolve_baseline_shims(
                tree,
                axis,
                &mut items,
                Some(&columns),
                Some(&rows),
                inner_node_size,
                synthesis,
                direction,
                parent_baselines.as_deref(),
            );
            if has_subgrid_items {
                subgrid::record_subgrid_baselines(tree, &items, axis, synthesis.get(axis));
            }
        }
    }

    // pyro: the subgrids' tracks as the aligned tracks place them; the subgrids' items are theirs to lay out.
    if has_subgrid_items {
        subgrid::update_subgrid_track_sizes(tree, &items[..own_item_count], AbsoluteAxis::Horizontal, &columns, true, &subgrid_parent);
        subgrid::update_subgrid_track_sizes(tree, &items[..own_item_count], AbsoluteAxis::Vertical, &rows, true, &subgrid_parent);
    }
    items.truncate(own_item_count);

    // 9. Size, Align, and Position Grid Items

    #[cfg_attr(not(feature = "content_size"), allow(unused_mut))]
    let mut item_overflow_rect = Rect::ZERO;

    // Out-of-flow candidates in document order: direct out-of-flow children of the grid
    // interleaved with candidates bubbled from in-flow children's subtrees. These are laid out
    // by the out-of-flow positioning pass (`compute_oof_layout`), which runs after this algorithm.
    let mut oof_candidates = OofCandidates::new();

    let container_alignment_styles = InBothAbsAxis { horizontal: justify_items, vertical: align_items };

    let padding_box_inset = border
        + Rect {
            left: if direction.is_rtl() { scrollbar_gutter.x } else { 0.0 },
            right: if direction.is_rtl() { 0.0 } else { scrollbar_gutter.x },
            top: 0.0,
            bottom: scrollbar_gutter.y,
        };
    #[cfg(feature = "content_size")]
    let overflow_container = ScrollableOverflowContainer {
        parent_is_scroll_container: is_scroll_container,
        parent_size: Size { width: container_border_box.width - padding_box_inset.horizontal_axis_sum(), height: 0.0 },
        reversed: Point { x: direction.is_rtl(), y: false },
        padding,
    };

    // Position in-flow children (stored in items vector)
    // pyro: in a table laid out again, an item that did not change keeps its layout in the
    // same grid area and is moved to an area of the same size (`table_reuse`).
    let mut positioned: Vec<alignment::PositionedItem> = match replay.as_mut() {
        Some(replay) => core::mem::take(&mut replay.positioned),
        None => Vec::with_capacity(if table_rows { items.len() } else { 0 }),
    };
    let reusable = positioned.len();
    for (index, item) in items.iter_mut().enumerate() {
        // Tracks are stored in logical order. In RTL the physical offsets are assigned
        // right-to-left, so an item's physical left edge is derived from its logical end
        // line and its physical right edge from its logical start line.
        let grid_area = Rect {
            top: rows[item.row_indexes.start as usize + 1].offset,
            bottom: rows[item.row_indexes.end as usize].offset,
            left: if direction.is_rtl() {
                columns[item.column_indexes.end as usize - 1].offset
            } else {
                columns[item.column_indexes.start as usize + 1].offset
            },
            right: if direction.is_rtl() {
                columns[item.column_indexes.start as usize].offset
            } else {
                columns[item.column_indexes.end as usize].offset
            },
        };
        let kept = match &replay {
            Some(replay) if index < reusable && !replay.changed[index] && !sized_again => {
                let old = &positioned[index];
                let size = |area: &Rect<f32>| (area.right - area.left, area.bottom - area.top);
                if old.area == grid_area {
                    Some(None)
                } else if old.area.left == grid_area.left
                    && old.area.right == grid_area.right
                    && size(&old.area) == size(&grid_area)
                {
                    let moved = old.moved_to(grid_area);
                    tree.set_unrounded_layout(item.node, &moved.layout);
                    Some(Some(moved))
                } else {
                    None
                }
            }
            _ => None,
        };
        let placed = match kept {
            Some(None) => {
                // The same layout in the same area.
                let old = &positioned[index];
                #[cfg(feature = "content_size")]
                {
                    item_overflow_rect = item_overflow_rect.union(old.contribution);
                }
                item.y_position = old.y_position;
                item.height = old.height;
                item.baseline = old.baseline;
                item.last_baseline = old.last_baseline;
                continue;
            }
            Some(Some(moved)) => moved,
            None => {
                // pyro: a baseline-aligned table cell fills its row, its content moved down inside it.
                let baseline_shim = if table_baselines {
                    let offset = if item.table_baseline { item.baseline_shim } else { 0.0 };
                    tree.set_table_cell_baseline_offset(node, item.node, offset);
                    if item.table_baseline {
                        0.0
                    } else {
                        item.baseline_shim
                    }
                } else {
                    item.baseline_shim
                };
                let baseline = alignment::ItemBaselineAlignment {
                    shim: Rect {
                        left: item.baseline_shim_inline.start,
                        right: item.baseline_shim_inline.end,
                        top: baseline_shim,
                        bottom: item.baseline_shim_block_end,
                    },
                    end: Size {
                        width: (has_baseline_aligned_item && item.participates_in_inline_baseline_alignment())
                            .then(|| item.baseline_group_is_end(AbstractAxis::Inline)),
                        height: (has_baseline_aligned_item && item.participates_in_baseline_alignment())
                            .then(|| item.baseline_group_is_end(AbstractAxis::Block)),
                    },
                };
                align_and_position_item(
                    tree,
                    item.node,
                    index as u32,
                    grid_area,
                    Size { width: grid_area.right - grid_area.left, height: grid_area.bottom - grid_area.top },
                    container_alignment_styles,
                    baseline,
                    item.width_unfloored,
                    item.subgrid,
                    direction,
                    #[cfg(feature = "content_size")]
                    padding_box_inset,
                    #[cfg(feature = "content_size")]
                    &overflow_container,
                    &mut item.oof_candidates,
                )
            }
        };
        #[cfg_attr(not(feature = "content_size"), allow(unused_variables))]
        let overflow_contribution = placed.contribution;
        item.y_position = placed.y_position;
        item.height = placed.height;
        item.baseline = placed.baseline;
        item.last_baseline = placed.last_baseline;
        if index < positioned.len() {
            positioned[index] = placed;
        } else if table_rows {
            positioned.push(placed);
        }

        #[cfg(feature = "content_size")]
        {
            item_overflow_rect = item_overflow_rect.union(overflow_contribution);
        }
    }

    // pyro: whether to keep this layout of a table for the next (`table_reuse`).
    let rows_sized = rows_sized.filter(|sized| {
        run_mode == RunMode::PerformLayout
            && !sized_again
            && sized.len() == rows.len()
            && table_reuse::can_record(
                &columns,
                &rows,
                &items,
                child_count,
                final_col_counts,
                final_row_counts,
                inner_width_is_definite,
                has_baseline_aligned_item,
            )
    });
    let kept_tracks = rows_sized.is_some().then(|| (columns.clone(), rows.clone()));

    // Position hidden and absolutely positioned children, merging in the candidates bubbled out
    // of in-flow children's subtrees (`items` is sorted by `source_order`, i.e. child index)
    let mut order = items.len() as u32;
    let mut in_flow_items = items.iter_mut().peekable();
    (0..tree.child_count(node)).for_each(|index| {
        if let Some(item) = in_flow_items.next_if(|item| item.source_order as usize == index) {
            oof_candidates.append(&mut item.oof_candidates);
            return;
        }

        let child = tree.get_child_id(node, index);
        let child_style = tree.get_grid_child_style(child);

        // Position hidden child
        if child_style.box_generation_mode() == BoxGenerationMode::None {
            drop(child_style);
            tree.set_unrounded_layout(child, &Layout::with_order(order));
            tree.perform_child_layout(
                child,
                Size::NONE,
                Size::NONE,
                Size::MAX_CONTENT,
                SizingMode::InherentSize,
                Line::FALSE,
            );
            order += 1;
            return;
        }

        // Emit out-of-flow children as candidates for the out-of-flow positioning pass
        // (`compute_oof_layout`), which lays out those for which this grid is the containing
        // block after this algorithm has run (resolving their grid areas from the detailed grid
        // info recorded below).
        if child_style.position().is_out_of_flow() {
            let position = child_style.position();
            let item_direction = child_style.direction();
            let justify_self = child_style.justify_self().unwrap_or(justify_items).resolve_self_relative(
                item_direction,
                direction,
                true,
            );
            let align_self =
                child_style.align_self().unwrap_or(align_items).resolve_self_relative(item_direction, direction, false);

            // The static-position rectangle: the grid area determined by the grid-placement
            // properties when this grid is the child's containing block, and otherwise the
            // grid's content box, as if the child were the sole grid item in a grid area
            // coinciding with the grid's content edges (grid placement does not apply when the
            // grid is not the containing block).
            // https://www.w3.org/TR/css-grid-1/#static-position
            let area = if containing_block_claims.for_position(position) {
                resolve_static_position_grid_area(
                    &child_style,
                    &name_resolver,
                    final_row_counts,
                    final_col_counts,
                    &rows,
                    &columns,
                    direction,
                    border,
                    scrollbar_gutter,
                    container_border_box,
                )
            } else {
                Rect {
                    left: border.left + padding.left + if direction.is_rtl() { scrollbar_gutter.x } else { 0.0 },
                    right: container_border_box.width
                        - border.right
                        - padding.right
                        - if direction.is_rtl() { 0.0 } else { scrollbar_gutter.x },
                    top: border.top + padding.top,
                    bottom: container_border_box.height - border.bottom - padding.bottom - scrollbar_gutter.y,
                }
            };
            drop(child_style);

            oof_candidates.push(OofCandidate { containing_block: None,
                node: child,
                order,
                position,
                static_position: Point {
                    x: AxisStaticPosition::from_alignment(
                        justify_self,
                        Line { start: area.left, end: area.right },
                        direction.is_rtl(),
                    ),
                    y: AxisStaticPosition::from_alignment(
                        align_self,
                        Line { start: area.top, end: area.bottom },
                        false,
                    ),
                },
            });

            order += 1;
        }
    });

    let absolute_position_area = container_border_box - padding_box_inset.sum_axes();
    let absolute_position_offset = Point { x: padding_box_inset.left, y: padding_box_inset.top };
    // Store the detailed grid info before the out-of-flow positioning pass so that the pass can
    // resolve the grid areas of out-of-flow boxes whose containing block is this grid
    let areas =
        name_resolver.populate_detailed_line_resolvers(&mut detailed_row_line_names, &mut detailed_column_line_names);
    let mut detailed_rows =
        DetailedGridTracksInfo::from_grid_tracks_and_track_count(final_row_counts, rows, detailed_row_line_names);
    let mut detailed_columns =
        DetailedGridTracksInfo::from_grid_tracks_and_track_count(final_col_counts, columns, detailed_column_line_names);
    detailed_rows.subgrid = subgrid_rows.is_some();
    detailed_columns.subgrid = subgrid_columns.is_some();
    tree.set_detailed_grid_info(
        node,
        DetailedGridInfo {
            rows: detailed_rows,
            columns: detailed_columns,
            items: items.iter().map(DetailedGridItemsInfo::from_grid_item).collect(),
            areas,
        },
    );

    let oof_positioning_area =
        Some(OofPositioningArea { size: absolute_position_area, offset: absolute_position_offset });

    // If there are no in-flow items then return the container size and the overflow (no baseline)
    if items.is_empty() {
        #[cfg(feature = "content_size")]
        let mut output = {
            let client_width =
                (container_border_box.width - border.horizontal_axis_sum() - scrollbar_gutter.x).max(0.0);
            let overflow_rect = if is_scroll_container {
                item_overflow_rect.union(Rect {
                    left: if direction.is_rtl() { (client_width - padding.left).min(0.0) } else { 0.0 },
                    right: if direction.is_rtl() { client_width } else { padding.right },
                    top: 0.0,
                    bottom: padding.bottom,
                })
            } else {
                item_overflow_rect
                    .union(Rect { right: if direction.is_rtl() { client_width } else { 0.0 }, ..Rect::ZERO })
            };
            LayoutOutput::from_sizes(
                container_border_box,
                finalize_scrollable_overflow(
                    overflow_rect,
                    container_border_box
                        - border.sum_axes()
                        - Size { width: scrollbar_gutter.x, height: scrollbar_gutter.y },
                    Point { x: direction.is_rtl(), y: false },
                    is_scroll_container,
                ),
            )
        };
        #[cfg(not(feature = "content_size"))]
        let mut output = LayoutOutput::from_outer_size(container_border_box);
        output.oof_candidates = oof_candidates;
        output.oof_positioning_area = oof_positioning_area;
        return output;
    }

    // Determine the grid container baselines
    // Layout containment suppresses the box's baseline for baseline-alignment purposes
    let (grid_container_baseline, grid_container_last_baseline) = if contain.suppresses_baseline() {
        (None, None)
    } else {
        // Get the row index of the first row containing items
        let first_row = items.iter().map(|item| item.row_indexes.start).min().unwrap();

        // Check if any items in *this row* participate in baseline alignment
        // (items with an auto block-axis margin do not participate: https://www.w3.org/TR/css-align-3/#baseline-align-self)
        let first_group = |item: &GridItem| {
            item.participates_in_baseline_alignment() && !item.baseline_group_is_end(AbstractAxis::Block)
        };
        let mut first_row_items = items.iter().filter(|item| item.row_indexes.start == first_row);
        let first_item = first_row_items.next().unwrap();
        let item = if first_group(first_item) {
            first_item
        } else {
            first_row_items.find(|item| first_group(item)).unwrap_or(first_item)
        };
        let first = item.y_position + item.baseline.unwrap_or(item.height);

        // pyro: the last baseline from the last row (css-grid-1 §10.1): its items in a last-baseline group, else
        // its last item in grid order
        let last_row = items.iter().map(|item| item.row_indexes.end).max().unwrap();
        let last_group = |item: &GridItem| {
            item.participates_in_baseline_alignment() && item.baseline_group_is_end(AbstractAxis::Block)
        };
        let last_row_items = || items.iter().filter(|item| item.row_indexes.end == last_row);
        let item = last_row_items().find(|item| last_group(item)).or_else(|| last_row_items().last()).unwrap();
        let last = item.y_position + item.last_baseline.unwrap_or(item.height);
        (Some(first), Some(last))
    };

    // A scroll container's own padding at the end of the content is part of its scrollable
    // overflow region, so it is included in the in-flow overflow rect. Boxes that are not
    // scroll containers do not extend their overflow region by their own padding.
    #[cfg(feature = "content_size")]
    let scrollable_overflow_rect = {
        let client_width = (container_border_box.width - border.horizontal_axis_sum() - scrollbar_gutter.x).max(0.0);
        let overflow_rect = if is_scroll_container {
            item_overflow_rect.union(Rect {
                left: if direction.is_rtl() { (client_width - padding.left).min(0.0) } else { 0.0 },
                right: if direction.is_rtl() { client_width } else { padding.right },
                top: 0.0,
                bottom: padding.bottom,
            })
        } else {
            item_overflow_rect.union(Rect { right: if direction.is_rtl() { client_width } else { 0.0 }, ..Rect::ZERO })
        };
        finalize_scrollable_overflow(
            overflow_rect,
            container_border_box - border.sum_axes() - Size { width: scrollbar_gutter.x, height: scrollbar_gutter.y },
            Point { x: direction.is_rtl(), y: false },
            is_scroll_container,
        )
    };
    #[cfg(not(feature = "content_size"))]
    let scrollable_overflow_rect = crate::tree::ScrollableOverflow::ZERO;

    let mut output = LayoutOutput::from_sizes_and_baselines(
        container_border_box,
        scrollable_overflow_rect,
        Baselines::new(grid_container_baseline, grid_container_last_baseline),
    );
    output.oof_candidates = oof_candidates;
    output.oof_positioning_area = oof_positioning_area;
    if let (Some(rows_sized), Some((columns, rows))) = (rows_sized, kept_tracks) {
        tree.store_table_grid_state(
            node,
            Box::new(TableGridState {
                inputs,
                constants,
                output: output.clone(),
                col_counts: final_col_counts,
                row_counts: final_row_counts,
                columns,
                rows_sized,
                rows,
                items,
                positioned,
                laid_out_again: replay.is_some(),
                stood: false,
            }),
        );
    }
    output
}

/// pyro: checks the areas and track counts a tree answered through `LayoutGridContainer::track_counts`
/// and `resolved_placement` against what this module's own placement gives for the same items: named-line
/// resolution, the grid size estimate and the auto-placement algorithm over an occupancy matrix.
#[cfg(debug_assertions)]
fn debug_assert_tree_placement<Tree: LayoutGridContainer>(
    tree: &Tree,
    node: NodeId,
    name_resolver: &NamedLineResolver<Tree::CustomIdent>,
    (explicit_col_count, explicit_row_count): (u16, u16),
    grid_auto_flow: crate::GridAutoFlow,
    items: &[GridItem],
    (col_counts, row_counts): (TrackCounts, TrackCounts),
) {
    let mut placed: Vec<GridItem> = Vec::with_capacity(items.len());
    let mut placements: Vec<ItemPlacement> = Vec::with_capacity(items.len());
    for (index, child_node) in tree.child_ids(node).enumerate() {
        let child_style = tree.get_grid_child_style(child_node);
        if child_style.box_generation_mode() == BoxGenerationMode::None || child_style.position().is_out_of_flow() {
            continue;
        }
        placements.push(InBothAbsAxis {
            horizontal: name_resolver
                .resolve_column_names(&child_style.grid_column())
                .into_origin_zero(explicit_col_count),
            vertical: name_resolver.resolve_row_names(&child_style.grid_row()).into_origin_zero(explicit_row_count),
        });
        placed.push(GridItem::new_with_style_and_order(
            child_node,
            child_style,
            AlignItems::START,
            AlignItems::START,
            index as u16,
        ));
    }
    let (est_col_counts, est_row_counts) =
        compute_grid_size_estimate(explicit_col_count, explicit_row_count, &placements);
    let mut matrix = CellOccupancyMatrix::with_track_counts(est_col_counts, est_row_counts);
    place_grid_items(&mut matrix, &mut placed, &placements, grid_auto_flow);
    assert_eq!(items.len(), placed.len(), "the tree placed a different number of grid items");
    for (item, expected) in items.iter().zip(placed.iter()) {
        assert_eq!(
            (item.column, item.row),
            (expected.column, expected.row),
            "the tree's placement of grid item {:?} (columns, rows) differs from the grid's",
            item.node
        );
    }
    assert_eq!(
        (col_counts, row_counts),
        (*matrix.track_counts(AbsoluteAxis::Horizontal), *matrix.track_counts(AbsoluteAxis::Vertical)),
        "the tree's grid track counts (columns, rows) differ from the grid's"
    );
}

/// Resolve the static-position rectangle (the grid area determined by the grid-placement
/// properties) for an out-of-flow child of a positioned grid container.
///
/// Content alignment (align-content/justify-content) may distribute free space before, between,
/// or after tracks. Grid lines used by absolutely positioned items resolve to the edges of the
/// tracks adjacent to the line rather than to the raw gutter offset:
///   - As a start edge, a line resolves to the start of the track that follows it
///   - As an end edge, a line resolves to the end of the track that precedes it
#[allow(clippy::too_many_arguments)]
fn resolve_static_position_grid_area<S: CheapCloneStr>(
    child_style: &impl GridItemStyle<CustomIdent = S>,
    name_resolver: &NamedLineResolver<S>,
    final_row_counts: TrackCounts,
    final_col_counts: TrackCounts,
    rows: &[GridTrack],
    columns: &[GridTrack],
    direction: Direction,
    border: Rect<f32>,
    scrollbar_gutter: Point<f32>,
    container_border_box: Size<f32>,
) -> Rect<f32> {
    // Convert grid-col-{start/end} into Option's of indexes into the columns vector
    // The Option is None if the style property is Auto and an unresolvable Span
    let maybe_col_indexes = name_resolver
        .resolve_column_names(&child_style.grid_column())
        .into_origin_zero(final_col_counts.explicit)
        .resolve_absolutely_positioned_grid_tracks()
        .map(|maybe_grid_line| {
            maybe_grid_line.and_then(|line: OriginZeroLine| line.try_into_track_vec_index(final_col_counts))
        });
    // Convert grid-row-{start/end} into Option's of indexes into the row vector
    // The Option is None if the style property is Auto and an unresolvable Span
    let maybe_row_indexes = name_resolver
        .resolve_row_names(&child_style.grid_row())
        .into_origin_zero(final_row_counts.explicit)
        .resolve_absolutely_positioned_grid_tracks()
        .map(|maybe_grid_line| {
            maybe_grid_line.and_then(|line: OriginZeroLine| line.try_into_track_vec_index(final_row_counts))
        });

    /// Resolve a grid line (by track vector index) used as a start edge to a position
    fn line_as_start_edge(tracks: &[GridTrack], index: usize) -> f32 {
        tracks.get(index + 1).unwrap_or(&tracks[index]).offset
    }
    /// Resolve a grid line (by track vector index) used as an end edge to a position
    fn line_as_end_edge(tracks: &[GridTrack], index: usize) -> f32 {
        if index == 0 {
            tracks.get(1).unwrap_or(&tracks[0]).offset
        } else {
            tracks[index].offset
        }
    }
    // In RTL, tracks remain in logical order but physical offsets are assigned
    // right-to-left: a line used as an inline-start edge resolves to the physical
    // *right* edge of the track that follows it (its gutter's offset), and a line
    // used as an inline-end edge resolves to the physical *left* edge (offset) of
    // the track that precedes it.
    /// Resolve a grid line used as an inline-start edge to a physical right x-position (RTL)
    fn rtl_line_as_start_edge(tracks: &[GridTrack], index: usize) -> f32 {
        if tracks.len() > index + 1 {
            // The gutter's offset is the physical right edge of the track that follows the line
            tracks[index].offset
        } else if index == 0 {
            tracks[0].offset
        } else {
            // No track follows the line: resolve to the line itself, which is the physical
            // left edge of the track that precedes it (the trailing gutter is assigned its
            // offset before any alignment offset is applied, so it cannot be used here)
            tracks[index - 1].offset
        }
    }
    /// Resolve a grid line used as an inline-end edge to a physical left x-position (RTL)
    fn rtl_line_as_end_edge(tracks: &[GridTrack], index: usize) -> f32 {
        if index == 0 {
            tracks[0].offset
        } else {
            tracks[index - 1].offset
        }
    }

    // In RTL the item's physical left edge derives from its logical end line and its
    // physical right edge from its logical start line.
    let (grid_area_left, grid_area_right) = if direction.is_rtl() {
        (
            maybe_col_indexes
                .end
                .map(|index| rtl_line_as_end_edge(columns, index))
                .unwrap_or(border.left + scrollbar_gutter.x),
            maybe_col_indexes
                .start
                .map(|index| rtl_line_as_start_edge(columns, index))
                .unwrap_or(container_border_box.width - border.right),
        )
    } else {
        (
            maybe_col_indexes.start.map(|index| line_as_start_edge(columns, index)).unwrap_or(border.left),
            maybe_col_indexes
                .end
                .map(|index| line_as_end_edge(columns, index))
                .unwrap_or(container_border_box.width - border.right - scrollbar_gutter.x),
        )
    };

    Rect {
        top: maybe_row_indexes.start.map(|index| line_as_start_edge(rows, index)).unwrap_or(border.top),
        bottom: maybe_row_indexes
            .end
            .map(|index| line_as_end_edge(rows, index))
            .unwrap_or(container_border_box.height - border.bottom - scrollbar_gutter.y),
        left: grid_area_left,
        right: grid_area_right,
    }
}

/// Information from the computation of grid
#[derive(Debug, Clone, PartialEq)]
pub struct DetailedGridInfo<S: CheapCloneStr = DefaultCheapStr> {
    /// <https://drafts.csswg.org/css-grid-1/#grid-row>
    pub rows: DetailedGridTracksInfo<S>,
    /// <https://drafts.csswg.org/css-grid-1/#grid-column>
    pub columns: DetailedGridTracksInfo<S>,
    /// <https://drafts.csswg.org/css-grid-1/#grid-items>
    pub items: Vec<DetailedGridItemsInfo>,
    /// Grid area edge lookup used to resolve the grid placements of absolutely positioned children
    areas: GridAreasMap<S>,
}

impl<S: CheapCloneStr> DetailedGridTracksInfo<S> {
    /// Resolve an absolute placement in this axis to physical start and end coordinates
    fn resolve_absolute_grid_axis(
        &self,
        placement: Line<GridPlacement<S>>,
        areas: &GridAreasMap<S>,
        axis: GridAreaAxis,
        padding_start: f32,
        padding_end: f32,
        is_reversed: bool,
    ) -> Line<f32> {
        let track_counts = TrackCounts {
            negative_implicit: self.negative_implicit_tracks,
            explicit: self.explicit_tracks,
            positive_implicit: self.positive_implicit_tracks,
        };
        let min_line = -(track_counts.negative_implicit as i16);
        let max_line = (track_counts.explicit + track_counts.positive_implicit) as i16;
        // Resolve the placement to a pair of (sorted) lines first, and only then treat lines
        // that fall outside the grid as `auto`. A placement whose lines end up swapped (e.g.
        // `grid-column: 3 / 1`, or a non-existent named line which resolves past the end of the
        // explicit grid) is normalized before the out-of-grid check, so e.g. `foo / 3` on a
        // 2-column grid resolves to the area between line 3 and the end padding edge.
        let placement = self
            .line_names
            .resolve_line_names(&placement, areas, axis, self.explicit_tracks)
            .into_origin_zero(self.explicit_tracks)
            .resolve_absolutely_positioned_grid_tracks()
            .map(|line| line.filter(|line| line.0 >= min_line && line.0 <= max_line))
            .map(|line| line.and_then(|line| line.try_into_track_vec_index(track_counts).map(|index| index / 2)));
        let start_position = placement
            .start
            .and_then(|line| {
                (line < self.positions.len())
                    .then(|| self.absolute_track_edge(line, true, is_reversed))
                    .or_else(|| self.positions.last().map(|track| if is_reversed { track.start } else { track.end }))
                    .or(self.empty_axis_line)
            })
            .unwrap_or(if is_reversed { padding_end } else { padding_start });
        let end_position = placement
            .end
            .and_then(|line| {
                line.checked_sub(1)
                    .filter(|&line| line < self.positions.len())
                    .map(|line| self.absolute_track_edge(line, false, is_reversed))
                    .or_else(|| self.positions.first().map(|track| if is_reversed { track.end } else { track.start }))
                    .or(self.empty_axis_line)
            })
            .unwrap_or(if is_reversed { padding_start } else { padding_end });

        // An area whose lines all fall within a collapsed run is zero-sized and anchored at its
        // logical start edge (as in Chrome; Gecko anchors it before the shared gutter instead).
        if is_reversed {
            Line { start: f32_min(start_position, end_position), end: start_position }
        } else {
            Line { start: start_position, end: f32_max(start_position, end_position) }
        }
    }

    /// Resolve a track edge, excluding gutters that overlap across a collapsed track run.
    fn absolute_track_edge(&self, index: usize, is_start: bool, is_reversed: bool) -> f32 {
        let range_index = self.collapsed_tracks.partition_point(|range| range.end <= index);
        let range = self.collapsed_tracks.get(range_index).filter(|range| range.contains(&index));
        let track = range
            .and_then(|range| {
                let adjacent = if is_start { Some(range.end) } else { range.start.checked_sub(1) };
                adjacent.and_then(|index| self.positions.get(index))
            })
            .unwrap_or(&self.positions[index]);
        // Collapsed gutters overlap: a start edge follows the shared gutter, while an end
        // edge precedes it.
        if is_start != is_reversed {
            track.start
        } else {
            track.end
        }
    }
}

impl<S: CheapCloneStr> DetailedGridInfo<S> {
    /// Write the used row track sizes and line names to the passed writer in the resolved value
    /// format of the `grid-template-rows` property
    /// (see <https://www.w3.org/TR/css-grid-1/#resolved-track-list>)
    /// Track sizes are divided by `zoom` (the container's effective CSS `zoom`, or `1.0`) to give unzoomed px.
    pub fn write_grid_template_rows(&self, out: &mut impl core::fmt::Write, zoom: f32) -> core::fmt::Result {
        self.rows.write_track_list(out, zoom)
    }

    /// Write the used column track sizes and line names to the passed writer in the resolved value
    /// format of the `grid-template-columns` property
    /// (see <https://www.w3.org/TR/css-grid-1/#resolved-track-list>)
    /// Track sizes are divided by `zoom` (the container's effective CSS `zoom`, or `1.0`) to give unzoomed px.
    pub fn write_grid_template_columns(&self, out: &mut impl core::fmt::Write, zoom: f32) -> core::fmt::Result {
        self.columns.write_track_list(out, zoom)
    }

    /// Serialize the used row track sizes and line names in the resolved value format of the
    /// `grid-template-rows` property (see <https://www.w3.org/TR/css-grid-1/#resolved-track-list>)
    /// Track sizes are divided by `zoom` (the container's effective CSS `zoom`, or `1.0`) to give unzoomed px.
    pub fn grid_template_rows(&self, zoom: f32) -> String {
        self.rows.to_track_list_string(zoom)
    }

    /// Serialize the used column track sizes and line names in the resolved value format of the
    /// `grid-template-columns` property (see <https://www.w3.org/TR/css-grid-1/#resolved-track-list>)
    /// Track sizes are divided by `zoom` (the container's effective CSS `zoom`, or `1.0`) to give unzoomed px.
    pub fn grid_template_columns(&self, zoom: f32) -> String {
        self.columns.to_track_list_string(zoom)
    }

    /// Resolve the physical grid area for an absolutely positioned box from its grid placement.
    /// The padding box and returned area use coordinates relative to the grid container's border box.
    pub fn resolve_absolute_grid_area(
        &self,
        grid_row: Line<GridPlacement<S>>,
        grid_column: Line<GridPlacement<S>>,
        direction: Direction,
        padding_box: Rect<f32>,
    ) -> Rect<f32> {
        let columns = self.columns.resolve_absolute_grid_axis(
            grid_column,
            &self.areas,
            GridAreaAxis::Column,
            padding_box.left,
            padding_box.right,
            direction.is_rtl(),
        );
        let rows = self.rows.resolve_absolute_grid_axis(
            grid_row,
            &self.areas,
            GridAreaAxis::Row,
            padding_box.top,
            padding_box.bottom,
            false,
        );
        Rect { left: columns.start, right: columns.end, top: rows.start, bottom: rows.end }
    }

    /// Compute the location and size of the grid area occupied by the item at `item_index` (an
    /// index into [`DetailedGridInfo::items`]), relative to the grid container's border box.
    ///
    /// The edges resolve to the edges of the tracks bounding the item's grid area (a start line
    /// resolves to the start of the track that follows it and an end line to the end of the track
    /// that precedes it), so the area excludes any gutter or content-alignment spacing around it.
    ///
    /// Returns `None` if `item_index` is out of bounds.
    pub fn item_grid_area(&self, item_index: usize) -> Option<(Point<f32>, Size<f32>)> {
        let item = self.items.get(item_index)?;
        let start_col = self.columns.positions[item.column_start as usize - 1];
        let end_col = self.columns.positions[item.column_end as usize - 2];
        let left = f32_min(start_col.start, end_col.start);
        let right = f32_max(start_col.end, end_col.end);
        let top = self.rows.positions[item.row_start as usize - 1].start;
        let bottom = self.rows.positions[item.row_end as usize - 2].end;
        Some((Point { x: left, y: top }, Size { width: right - left, height: bottom - top }))
    }
}

/// Information from the computation of grids tracks
#[derive(Debug, Clone, PartialEq)]
pub struct DetailedGridTracksInfo<S: CheapCloneStr = DefaultCheapStr> {
    /// Number of leading implicit grid tracks
    pub negative_implicit_tracks: u16,
    /// Number of explicit grid tracks
    pub explicit_tracks: u16,
    /// Number of trailing implicit grid tracks
    pub positive_implicit_tracks: u16,

    /// The start and end position of each track relative to the grid container's border box.
    /// These positions account for the container's border and padding, the `gap` property,
    /// content alignment (`align-content`/`justify-content`), and collapsed tracks.
    pub positions: Vec<Line<f32>>,

    /// Sorted, disjoint ranges of collapsed `auto-fit` tracks (indices into `positions`), whose
    /// adjacent gutters overlap when resolving absolute grid-area edges. Empty (and never
    /// allocated) unless a track collapsed.
    pub collapsed_tracks: Vec<Range<usize>>,

    /// The position of the axis' single grid line relative to the grid container's border box
    /// when the axis has no tracks (an empty grid still contains one grid line in each axis,
    /// positioned by content alignment). `None` when the axis has tracks.
    pub empty_axis_line: Option<f32>,

    /// The names of each *explicit* grid line. Stored line `i` (0-indexed) bounds the start of
    /// explicit track `i`; use [`DetailedGridTracksInfo::names_for_line`] or
    /// [`DetailedGridTracksInfo::iter_line_names`] for indices relative to the full grid
    /// (including implicit tracks). Empty if the grid has no named lines.
    pub line_names: GridLineNames<S>,

    /// pyro: the axis is a subgrid's subgridded axis: its resolved `grid-template-*` is `subgrid` with the
    /// names of each of its lines (css-grid-2 §7.2.6.2)
    pub subgrid: bool,
}

impl<S: CheapCloneStr> DetailedGridTracksInfo<S> {
    /// Get the start and end position of each track relative to the grid container's border box,
    /// along with the index ranges of any runs of collapsed `auto-fit` tracks
    fn positions_from_grid_track_layout(grid_tracks: &[GridTrack]) -> (Vec<Line<f32>>, Vec<Range<usize>>) {
        let mut positions = Vec::with_capacity(grid_tracks.len() / 2);
        let mut collapsed_tracks: Vec<Range<usize>> = Vec::new();
        for track in grid_tracks.iter().filter(|track| track.kind == GridTrackKind::Track) {
            if track.is_collapsed {
                let index = positions.len();
                match collapsed_tracks.last_mut() {
                    Some(range) if range.end == index => range.end += 1,
                    _ => collapsed_tracks.push(index..index + 1),
                }
            }
            positions.push(Line { start: track.offset, end: track.offset + track.base_size });
        }
        (positions, collapsed_tracks)
    }

    /// Construct DetailedGridTracksInfo from TrackCounts and GridTracks
    fn from_grid_tracks_and_track_count(
        track_count: TrackCounts,
        grid_tracks: Vec<GridTrack>,
        line_names: GridLineNames<S>,
    ) -> Self {
        let (positions, collapsed_tracks) = DetailedGridTracksInfo::<S>::positions_from_grid_track_layout(&grid_tracks);
        // An axis with no tracks consists of a single gutter whose offset is where the axis'
        // single grid line was positioned by content alignment
        let empty_axis_line = if positions.is_empty() { grid_tracks.first().map(|track| track.offset) } else { None };
        DetailedGridTracksInfo {
            negative_implicit_tracks: track_count.negative_implicit,
            explicit_tracks: track_count.explicit,
            positive_implicit_tracks: track_count.positive_implicit,
            positions,
            collapsed_tracks,
            empty_axis_line,
            line_names,
            subgrid: false,
        }
    }

    /// The names of the grid line with the passed 0-indexed line index, where line `i` bounds
    /// the start of track `i` of the full grid (including implicit tracks).
    /// Returns an empty slice if the line has no names or the index is out of range.
    pub fn names_for_line(&self, line_index: usize) -> &[S] {
        match line_index.checked_sub(self.negative_implicit_tracks as usize) {
            Some(stored_index) => self.line_names.line(stored_index),
            None => &[],
        }
    }

    /// Iterate over the name group (`&[S]`) of each grid line of the full grid (including
    /// implicit tracks) in line order, yielding empty groups for unnamed (implicit) lines.
    /// Yields nothing if the grid has no named lines.
    pub fn iter_line_names(&self) -> GridLineNamesIter<'_, S> {
        if self.line_names.is_empty() {
            return self.line_names.iter();
        }
        let total_line_count = self.positions.len() + 1;
        let leading_empty = self.negative_implicit_tracks as usize;
        let trailing_empty = total_line_count.saturating_sub(leading_empty + self.line_names.line_count());
        self.line_names.iter_padded(leading_empty, trailing_empty)
    }

    /// Write the used track sizes and line names of this axis to the passed writer in the
    /// resolved value format of the `grid-template-rows`/`grid-template-columns` properties
    /// (see <https://www.w3.org/TR/css-grid-1/#resolved-track-list>)
    /// Track sizes are divided by `zoom` (the container's effective CSS `zoom`, or `1.0`) to give unzoomed px.
    pub fn write_track_list(&self, out: &mut impl core::fmt::Write, zoom: f32) -> core::fmt::Result {
        /// Write a bracketed line name group (e.g. `[foo bar]`)
        fn write_line_names<S: CheapCloneStr>(out: &mut impl core::fmt::Write, names: &[S]) -> core::fmt::Result {
            out.write_char('[')?;
            for (i, name) in names.iter().enumerate() {
                if i != 0 {
                    out.write_char(' ')?;
                }
                out.write_str(name.as_ref())?;
            }
            out.write_char(']')
        }

        // pyro: `subgrid` and the subgrid's own names of each of its lines, the empty ones included
        if self.subgrid {
            out.write_str("subgrid")?;
            for line in 0..=self.explicit_tracks as usize {
                out.write_char(' ')?;
                write_line_names(out, self.line_names.line(line))?;
            }
            return Ok(());
        }

        if self.positions.is_empty() {
            return out.write_str("none");
        }

        let mut needs_space = false;
        for (track_index, position) in self.positions.iter().enumerate() {
            let names = self.names_for_line(track_index);
            if !names.is_empty() {
                if needs_space {
                    out.write_char(' ')?;
                }
                write_line_names(out, names)?;
                needs_space = true;
            }
            if needs_space {
                out.write_char(' ')?;
            }
            write!(out, "{}px", (position.end - position.start) / zoom)?;
            needs_space = true;
        }
        let trailing_names = self.names_for_line(self.positions.len());
        if !trailing_names.is_empty() {
            out.write_char(' ')?;
            write_line_names(out, trailing_names)?;
        }
        Ok(())
    }

    /// Serialize the used track sizes and line names of this axis in the resolved value format of
    /// the `grid-template-rows`/`grid-template-columns` properties
    /// (see <https://www.w3.org/TR/css-grid-1/#resolved-track-list>)
    /// Track sizes are divided by `zoom` (the container's effective CSS `zoom`, or `1.0`) to give unzoomed px.
    pub fn to_track_list_string(&self, zoom: f32) -> String {
        let mut out = String::new();
        self.write_track_list(&mut out, zoom).expect("writing to a String cannot fail");
        out
    }
}

/// Grid area information from the placement algorithm
///
/// The values is 1-indexed grid line numbers bounding the area.
/// This matches the Chrome and Firefox's format as of 2nd Jan 2024.
#[derive(Debug, Clone, PartialEq)]
pub struct DetailedGridItemsInfo {
    /// row-start with 1-indexed grid line numbers
    pub row_start: u16,
    /// row-end with 1-indexed grid line numbers
    pub row_end: u16,
    /// column-start with 1-indexed grid line numbers
    pub column_start: u16,
    /// column-end with 1-indexed grid line numbers
    pub column_end: u16,
}

/// Grid area information from the placement algorithm
impl DetailedGridItemsInfo {
    /// Construct from GridItems
    #[inline(always)]
    fn from_grid_item(grid_item: &GridItem) -> Self {
        /// Conversion from the indexes of Vec<GridTrack> into 1-indexed grid line numbers. See [`GridItem::row_indexes`] or [`GridItem::column_indexes`]
        #[inline(always)]
        fn to_one_indexed_grid_line(grid_track_index: u16) -> u16 {
            grid_track_index / 2 + 1
        }

        DetailedGridItemsInfo {
            row_start: to_one_indexed_grid_line(grid_item.row_indexes.start),
            row_end: to_one_indexed_grid_line(grid_item.row_indexes.end),
            column_start: to_one_indexed_grid_line(grid_item.column_indexes.start),
            column_end: to_one_indexed_grid_line(grid_item.column_indexes.end),
        }
    }
}

/// pyro: the re-check of the items' min-content widths after row sizing.
#[cfg(all(test, feature = "taffy_tree"))]
mod rerun_tests {
    use crate::prelude::*;
    use crate::style::Style;
    use crate::LayoutOutput;

    /// How a test leaf reports its size
    #[derive(Clone, Copy)]
    enum Leaf {
        /// 300 wide (max-content) or 100 wide (min-content), 10 tall
        Text,
        /// 20 wide once its height is known, 50 wide before, 10 tall
        HeightDependent,
        /// 10 wide (min-content); 30 wide (max-content) once its height is known, 50 before; 10 tall
        MaxContentHeightDependent,
    }

    /// Lay out a 200px-wide grid with the given columns and one item per column, returning the item widths
    /// and the number of times a leaf was measured for an intrinsic width (no known width, min- or max-content available).
    fn layout(columns: Vec<GridTemplateComponent<String>>, items: &[(Leaf, Style)]) -> (Vec<f32>, usize) {
        let mut taffy: TaffyTree<Leaf> = TaffyTree::new();
        let children: Vec<NodeId> =
            items.iter().map(|(leaf, style)| taffy.new_leaf_with_context(style.clone(), *leaf).unwrap()).collect();
        let grid = taffy
            .new_with_children(
                Style {
                    display: Display::Grid,
                    size: Size { width: length(200.0), height: auto() },
                    grid_template_columns: columns,
                    justify_content: JustifyContent::START,
                    ..Default::default()
                },
                &children,
            )
            .unwrap();
        let mut inline_measures = 0;
        taffy
            .compute_layout_with_measure(grid, Size::MAX_CONTENT, |inputs, _, context, style| {
                let Some(leaf) = context.map(|leaf| *leaf) else { return LayoutOutput::HIDDEN };
                crate::compute_leaf_layout(
                    inputs,
                    style,
                    |_, _| 0.0,
                    |known, available| {
                        if known.width.is_none() && !matches!(available.width, AvailableSpace::Definite(_)) {
                            inline_measures += 1;
                        }
                        let width = match leaf {
                            Leaf::Text => match available.width {
                                AvailableSpace::MinContent => 100.0,
                                AvailableSpace::MaxContent => 300.0,
                                AvailableSpace::Definite(width) => width.clamp(100.0, 300.0),
                            },
                            Leaf::HeightDependent => {
                                if known.height.is_some() {
                                    20.0
                                } else {
                                    50.0
                                }
                            }
                            Leaf::MaxContentHeightDependent => match (available.width, known.height) {
                                (AvailableSpace::MinContent, _) => 10.0,
                                (_, Some(_)) => 30.0,
                                (_, None) => 50.0,
                            },
                        };
                        Size { width: known.width.unwrap_or(width), height: known.height.unwrap_or(10.0) }
                    },
                )
            })
            .unwrap();
        let widths = children.iter().map(|child| taffy.layout(*child).unwrap().size.width).collect();
        (widths, inline_measures)
    }

    fn min_width_zero() -> Style {
        Style { min_size: Size { width: length(0.0), height: auto() }, ..Default::default() }
    }

    #[test]
    fn an_fr_column_with_a_min_width_0_item_is_not_rechecked() {
        let (widths, inline_measures) = layout(vec![fr(1.0)], &[(Leaf::Text, min_width_zero())]);
        assert_eq!(widths, vec![200.0]);
        // Column sizing read nothing of the item's width, so neither the re-check nor a re-run measures it
        assert_eq!(inline_measures, 0);
    }

    #[test]
    fn a_percentage_min_width_is_still_rechecked() {
        // The minimum contribution resolves against the grid area, so the item counts as read
        let style = Style { min_size: Size { width: percent(0.25), height: auto() }, ..Default::default() };
        let (widths, inline_measures) = layout(vec![fr(1.0)], &[(Leaf::Text, style)]);
        assert_eq!(widths, vec![200.0]);
        assert!(inline_measures > 0);
    }

    #[test]
    fn an_auto_column_whose_item_changes_with_its_row_still_reruns() {
        // Before row sizing the item's height is unknown and it is 50 wide; once its row is sized it is 20 wide,
        // so the re-check sees a changed min-content width and column sizing runs again.
        let (widths, inline_measures) = layout(vec![auto()], &[(Leaf::HeightDependent, Style::default())]);
        assert_eq!(widths, vec![20.0]);
        assert!(inline_measures > 0);
    }

    #[test]
    fn a_skipped_item_is_resized_when_another_item_forces_the_rerun() {
        let (widths, _) =
            layout(vec![auto(), fr(1.0)], &[(Leaf::HeightDependent, Style::default()), (Leaf::Text, min_width_zero())]);
        assert_eq!(widths, vec![20.0, 180.0]);
    }

    #[test]
    fn an_unread_item_does_not_change_the_other_columns() {
        // The auto column's item has a max-content width that changes with its row but a min-content width that
        // does not, so the re-check (which compares min-content widths, css-grid-1 §12.1 step 3) asks for no re-run
        // and the column keeps the first pass's 50px. Before the skip, the `min-width: 0` item in the `fr` column
        // forced a re-run on its empty cache, which took up the new max-content width and gave 30px: the auto
        // column's size depended on whether an unrelated item sat beside it.
        let item = (Leaf::MaxContentHeightDependent, Style::default());
        let (alone, _) = layout(vec![auto()], &[item.clone()]);
        let (beside, _) = layout(vec![auto(), fr(1.0)], &[item, (Leaf::Text, min_width_zero())]);
        assert_eq!(alone, vec![50.0]);
        assert_eq!(beside, vec![50.0, 150.0]);
    }
}
