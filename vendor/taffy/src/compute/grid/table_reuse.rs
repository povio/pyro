//! pyro: a table's grid laid out again from its last layout.
//!
//! A table's rows are a grid's (`LayoutGridContainer::grid_rows_are_table_rows`). After a full layout
//! of such a grid, `compute_grid_layout` offers what it computed to the tree
//! (`LayoutGridContainer::store_table_grid_state`): its items, its tracks (the rows as sized from their
//! items and as finally sized) and where each item was positioned. On the next layout with the same
//! inputs, the tree hands it back (`take_table_grid_state`) with the items that changed since (their
//! subtree, or their style). Only the rows of those items (and rows appended since) are sized from their
//! items again; the steps after that run on every track as before, and an item whose grid area did not
//! change keeps its layout, one whose area only moved is moved, the others are positioned again.
//!
//! The state is only recorded for grids where nothing else depends on the items: every column track is
//! fixed (no intrinsic or percentage sizing, flexible ones only in a definite width), no row uses a
//! percentage, no item spans rows, is baseline-aligned or has out-of-flow descendants bubbling up,
//! there are no implicit tracks, no content-distribution gutters, and no track sizing is re-run.
use super::alignment::PositionedItem;
use super::explicit_grid::initialize_grid_tracks;
use super::placement::{place_grid_items, ItemPlacement};
use super::track_sizing::{determine_if_item_crosses_flexible_or_intrinsic_tracks, resolve_item_track_indexes};
use super::types::{CellOccupancyMatrix, GridItem, GridTrack, GridTrackKind, NamedLineResolver, OriginZeroLine, TrackCounts};
use super::GridContainerConstants;
use crate::geometry::{AbsoluteAxis, InBothAbsAxis};
use crate::style::{AlignItems, GridAutoFlow};
use crate::geometry::Size;
use crate::tree::{LayoutInput, LayoutOutput, NodeId};
use crate::util::sys::{Box, GridTrackVec, Vec};
use crate::{BoxGenerationMode, CoreStyle, GridItemStyle, LayoutGridContainer};

/// A table grid's last full layout (see the module documentation). Opaque to the tree, which keeps
/// it between layouts.
#[derive(Debug)]
pub struct TableGridState {
    pub(super) inputs: LayoutInput,
    pub(super) constants: GridContainerConstants,
    /// What the layout computed for the grid itself, for a layout that stands as it is (`stands`)
    pub(super) output: LayoutOutput,
    pub(super) col_counts: TrackCounts,
    pub(super) row_counts: TrackCounts,
    /// The columns as finally sized and aligned
    pub(super) columns: GridTrackVec<GridTrack>,
    /// The rows after their intrinsic sizes were resolved from their items (11.5), and as finally
    /// sized and aligned
    pub(super) rows_sized: GridTrackVec<GridTrack>,
    pub(super) rows: GridTrackVec<GridTrack>,
    pub(super) items: Vec<GridItem>,
    pub(super) positioned: Vec<PositionedItem>,
    /// Whether this layout was itself laid out again from the last one
    pub(super) laid_out_again: bool,
    /// Whether this layout stood as it was for the latest layout of the grid (`stands`): nothing was
    /// laid out
    pub(super) stood: bool,
}

impl TableGridState {
    /// Whether this layout was laid out again from the one before (rather than the whole grid)
    pub fn laid_out_again(&self) -> bool {
        self.laid_out_again
    }

    /// Whether the grid's latest layout kept this one as it was, laying out nothing
    pub fn stood(&self) -> bool {
        self.stood
    }
}

/// A table grid's last layout and the indices (among the grid's children) of the items that changed
/// since it was recorded. An item changes when its subtree or its style does; items appended since
/// (children after the last recorded one) need not be listed. Changed items keep their grid placement:
/// a table whose structure changed otherwise has no state to give back.
#[derive(Debug)]
pub struct TableGridReuse {
    /// The last layout
    pub state: Box<TableGridState>,
    /// The indices of the items that changed since (in the grid as it is now)
    pub changed: Vec<usize>,
    /// Rows replaced since by other rows (inserted, removed), with their items; `None`: rows were
    /// only appended, if any
    pub splice: Option<TableGridSplice>,
}

/// Rows of a table grid replaced since its last layout: the items `items` (indices in the last layout)
/// in the explicit rows `rows` (0-based) are now `inserted` items in `inserted_rows` rows starting at
/// the same row and item. The items and rows after them are the same, moved.
#[derive(Debug, Clone)]
pub struct TableGridSplice {
    /// The items replaced, as indices in the last layout
    pub items: core::ops::Range<usize>,
    /// How many items replaced them
    pub inserted: usize,
    /// The rows replaced
    pub rows: core::ops::Range<u16>,
    /// How many rows replaced them
    pub inserted_rows: u16,
}

/// What `compute_grid_layout` lays out again from a `TableGridState`.
pub(super) struct Replay {
    /// The grid's items, the old ones kept and the changed and appended ones made again
    pub items: Vec<GridItem>,
    pub col_counts: TrackCounts,
    pub row_counts: TrackCounts,
    pub columns: GridTrackVec<GridTrack>,
    /// The rows: the ones not affected as their items sized them (11.5), the others to size again
    pub rows: GridTrackVec<GridTrack>,
    /// For each row track (gutters included), whether it is sized from its items again
    pub affected_tracks: Vec<bool>,
    /// For each item, whether it is positioned again whatever its area
    pub changed: Vec<bool>,
    /// The rows as finally sized and aligned last time, and where each old item was positioned
    pub old_rows: GridTrackVec<GridTrack>,
    pub positioned: Vec<PositionedItem>,
}

fn same_sizing(a: &GridTrack, b: &GridTrack) -> bool {
    a.kind == b.kind
        && a.is_collapsed == b.is_collapsed
        && a.min_track_sizing_function == b.min_track_sizing_function
        && a.max_track_sizing_function == b.max_track_sizing_function
}

/// Whether two layout inputs differ at most in the available width and the parent's width. A table
/// grid's columns are fixed tracks (the state is recorded for no other), which neither sizes; what
/// of the grid's own style resolves against the parent's width is in its `GridContainerConstants`,
/// compared besides.
fn same_but_for_available_width(a: &LayoutInput, b: &LayoutInput) -> bool {
    LayoutInput {
        available_space: Size { width: b.available_space.width, height: a.available_space.height },
        parent_size: Size { width: b.parent_size.width, height: a.parent_size.height },
        ..*a
    } == *b
}

impl GridContainerConstants {
    /// Whether the constants differ at most in the available grid width (see
    /// [`same_but_for_available_width`]).
    fn same_but_for_available_width(&self, other: &Self) -> bool {
        Self {
            available_grid_space: Size {
                width: other.available_grid_space.width,
                height: self.available_grid_space.height,
            },
            ..*self
        } == *other
    }
}

/// pyro: whether `reuse.state` is the layout at `inputs` as it is: nothing changed since it
/// was laid out (no item, no row replaced or appended, the same tracks) and the inputs differ at most
/// in the available width (and the parent's width, where the grid's own style resolves nothing against
/// it). A shrink-to-fit table in a container that got wider or narrower, but not past the table's
/// width, was laid out again cell by cell for the same result.
#[allow(clippy::too_many_arguments)]
pub(super) fn stands<Tree: LayoutGridContainer>(
    reuse: &TableGridReuse,
    style: &Tree::GridContainerStyle<'_>,
    inputs: &LayoutInput,
    constants: &GridContainerConstants,
    child_count: usize,
    explicit_col_count: u16,
    explicit_row_count: u16,
    col_auto_repetition_count: u16,
    row_auto_repetition_count: u16,
) -> bool {
    let state = &*reuse.state;
    if !reuse.changed.is_empty() || reuse.splice.is_some() || child_count != state.items.len() {
        return false;
    }
    if !same_but_for_available_width(&state.inputs, inputs) || !constants.same_but_for_available_width(&state.constants)
    {
        return false;
    }
    let col_counts = TrackCounts::from_raw(0, explicit_col_count, 0);
    let row_counts = TrackCounts::from_raw(0, explicit_row_count, 0);
    if state.col_counts != col_counts || state.row_counts != row_counts {
        return false;
    }
    let mut columns = GridTrackVec::new();
    initialize_grid_tracks(&mut columns, col_counts, style, AbsoluteAxis::Horizontal, col_auto_repetition_count, |_| true);
    let mut rows = GridTrackVec::new();
    initialize_grid_tracks(&mut rows, row_counts, style, AbsoluteAxis::Vertical, row_auto_repetition_count, |_| true);
    columns.len() == state.columns.len()
        && columns.iter().zip(&state.columns).all(|(a, b)| same_sizing(a, b))
        && rows.len() == state.rows_sized.len()
        && rows.iter().zip(&state.rows_sized).all(|(a, b)| same_sizing(a, b))
}

/// The grid of `state` brought up to date with the grid's style and children, or `None` if it can't be
/// (its inputs, its columns or its explicit grid changed, or children were removed).
#[allow(clippy::too_many_arguments)]
pub(super) fn prepare<Tree: LayoutGridContainer>(
    tree: &Tree,
    node: NodeId,
    style: &Tree::GridContainerStyle<'_>,
    reuse: TableGridReuse,
    inputs: LayoutInput,
    constants: GridContainerConstants,
    child_count: usize,
    name_resolver: &NamedLineResolver<Tree::CustomIdent>,
    explicit_col_count: u16,
    explicit_row_count: u16,
    col_auto_repetition_count: u16,
    row_auto_repetition_count: u16,
    align_items: AlignItems,
    justify_items: AlignItems,
    grid_auto_flow: GridAutoFlow,
) -> Option<Replay> {
    let TableGridReuse { state, changed: changed_indices, splice } = reuse;
    let state = *state;
    // (The available width sizes no fixed column: see `same_but_for_available_width`.)
    if !same_but_for_available_width(&state.inputs, &inputs) || !constants.same_but_for_available_width(&state.constants)
    {
        return None;
    }
    let old_len = state.items.len();
    let old_rows = state.row_counts.explicit;
    // Rows appended, if any, are a splice at the end.
    let splice = match splice {
        Some(splice) => splice,
        None => TableGridSplice {
            items: old_len..old_len,
            inserted: child_count.checked_sub(old_len)?,
            rows: old_rows..old_rows,
            inserted_rows: explicit_row_count.checked_sub(old_rows)?,
        },
    };
    let col_counts = TrackCounts::from_raw(0, explicit_col_count, 0);
    let row_counts = TrackCounts::from_raw(0, explicit_row_count, 0);
    if state.col_counts != col_counts
        || splice.items.end > old_len
        || splice.rows.end > old_rows
        || child_count + splice.items.len() != old_len + splice.inserted
        || explicit_row_count as usize + splice.rows.len() != old_rows as usize + splice.inserted_rows as usize
        // Rows come with cells.
        || (splice.inserted_rows > 0 && splice.inserted == 0)
    {
        return None;
    }
    // The items replaced are those of the rows replaced.
    let first_row = splice.rows.start as i16;
    if splice.items.start > 0 && state.items[splice.items.start - 1].row.end.0 > first_row
        || splice.items.end < old_len && state.items[splice.items.end].row.start.0 < splice.rows.end as i16
    {
        return None;
    }
    let row_shift = splice.inserted_rows as i16 - splice.rows.len() as i16;
    let after = splice.items.start + splice.inserted;

    // The tracks as the style now has them.
    let mut columns = GridTrackVec::new();
    initialize_grid_tracks(&mut columns, col_counts, style, AbsoluteAxis::Horizontal, col_auto_repetition_count, |_| true);
    if columns.len() != state.columns.len() || columns.iter().zip(&state.columns).any(|(a, b)| !same_sizing(a, b)) {
        return None;
    }
    let mut rows = GridTrackVec::new();
    initialize_grid_tracks(&mut rows, row_counts, style, AbsoluteAxis::Vertical, row_auto_repetition_count, |_| true);
    let mut affected_tracks = vec![false; rows.len()];
    // Track `2r + 1` is row `r`, the gutters are between them: the tracks before the splice are the
    // same, those after it moved.
    let (start, end) = (2 * splice.rows.start as usize, 2 * (splice.rows.start + splice.inserted_rows) as usize);
    for (index, track) in rows.iter_mut().enumerate() {
        let old = if index <= start {
            Some(index)
        } else if index >= end {
            (index as isize - 2 * row_shift as isize).try_into().ok()
        } else {
            None
        };
        match old.and_then(|old| state.rows_sized.get(old)) {
            Some(old) if same_sizing(old, track) => *track = old.clone(),
            // A row whose sizing changed, an inserted one, and the gutters around them.
            _ => affected_tracks[index] = true,
        }
    }

    let mut changed = vec![false; child_count];
    for index in changed_indices {
        if let Some(changed) = changed.get_mut(index) {
            *changed = true;
        }
    }
    let child_id = |index: usize| tree.get_child_id(node, index);

    // The items: the old ones before the splice, the inserted ones, the old ones after it (moved), and
    // the old ones made again from their style where they changed.
    let mut old_items = state.items;
    let tail: Vec<GridItem> = old_items.drain(splice.items.end..).collect();
    old_items.truncate(splice.items.start);
    let mut items = old_items;
    let mut inserted: Vec<GridItem> = Vec::with_capacity(splice.inserted);
    // A tree that places the items itself (`LayoutGridContainer::track_counts`) answers the inserted
    // items' areas as well; otherwise they are placed as the whole grid's placement places them.
    let tree_extent = tree.track_counts(node);
    let mut placements: Vec<ItemPlacement> = Vec::with_capacity(if tree_extent.is_some() { 0 } else { splice.inserted });
    for index in splice.items.start..after {
        let child_style = tree.get_grid_child_style(child_id(index));
        if child_style.box_generation_mode() == BoxGenerationMode::None || child_style.position().is_out_of_flow() {
            return None;
        }
        if tree_extent.is_none() {
            placements.push(InBothAbsAxis {
                horizontal: name_resolver.resolve_column_names(&child_style.grid_column()).into_origin_zero(explicit_col_count),
                vertical: name_resolver.resolve_row_names(&child_style.grid_row()).into_origin_zero(explicit_row_count),
            });
        }
        let mut item = GridItem::new_with_style_and_order(child_id(index), child_style, align_items, justify_items, index as u16);
        if tree_extent.is_some() {
            let area = tree
                .resolved_placement(node, child_id(index))
                .expect("a tree answering track_counts answers resolved_placement for every in-flow child");
            item.column = area.width.map(|line| OriginZeroLine(line as i16));
            item.row = area.height.map(|line| OriginZeroLine(line as i16));
        }
        inserted.push(item);
        changed[index] = true;
    }
    if !inserted.is_empty() {
        // The inserted items go to the inserted rows, which no other item reaches, and make no
        // implicit track.
        match tree_extent {
            Some(extent) => {
                if extent.width > explicit_col_count || extent.height > explicit_row_count {
                    return None;
                }
                #[cfg(debug_assertions)]
                {
                    let mut placed: Vec<GridItem> = Vec::with_capacity(inserted.len());
                    let mut placements: Vec<ItemPlacement> = Vec::with_capacity(inserted.len());
                    for item in &inserted {
                        let child_style = tree.get_grid_child_style(item.node);
                        placements.push(InBothAbsAxis {
                            horizontal: name_resolver.resolve_column_names(&child_style.grid_column()).into_origin_zero(explicit_col_count),
                            vertical: name_resolver.resolve_row_names(&child_style.grid_row()).into_origin_zero(explicit_row_count),
                        });
                        placed.push(GridItem::new_with_style_and_order(item.node, child_style, align_items, justify_items, item.source_order));
                    }
                    let mut matrix = CellOccupancyMatrix::with_track_counts(col_counts, row_counts);
                    place_grid_items(&mut matrix, &mut placed, &placements, grid_auto_flow);
                    for (item, expected) in inserted.iter().zip(&placed) {
                        debug_assert_eq!(
                            (item.column, item.row),
                            (expected.column, expected.row),
                            "the tree's placement of inserted grid item {:?} (columns, rows) differs from the grid's",
                            item.node
                        );
                    }
                }
            }
            None => {
                let mut matrix = CellOccupancyMatrix::with_track_counts(col_counts, row_counts);
                place_grid_items(&mut matrix, &mut inserted, &placements, grid_auto_flow);
                if *matrix.track_counts(AbsoluteAxis::Horizontal) != col_counts
                    || *matrix.track_counts(AbsoluteAxis::Vertical) != row_counts
                {
                    return None;
                }
            }
        }
        let rows_inserted = first_row..first_row + splice.inserted_rows as i16;
        if inserted.iter().any(|item| !rows_inserted.contains(&item.row.start.0) || item.row.end.0 > rows_inserted.end) {
            return None;
        }
        resolve_item_track_indexes(&mut inserted, col_counts, row_counts);
        items.extend(inserted);
    }
    for (offset, mut item) in tail.into_iter().enumerate() {
        let index = after + offset;
        item.node = child_id(index);
        item.source_order = index as u16;
        item.row.start.0 += row_shift;
        item.row.end.0 += row_shift;
        item.row_indexes.start = (item.row_indexes.start as i32 + 2 * row_shift as i32) as u16;
        item.row_indexes.end = (item.row_indexes.end as i32 + 2 * row_shift as i32) as u16;
        items.push(item);
    }
    for index in (0..child_count).filter(|&index| changed[index] && !(splice.items.start..after).contains(&index)) {
        let child_style = tree.get_grid_child_style(child_id(index));
        if child_style.box_generation_mode() == BoxGenerationMode::None || child_style.position().is_out_of_flow() {
            return None;
        }
        let old = &items[index];
        let mut item = GridItem::new_with_style_and_order(child_id(index), child_style, align_items, justify_items, index as u16);
        item.row = old.row;
        item.column = old.column;
        item.row_indexes = old.row_indexes;
        item.column_indexes = old.column_indexes;
        items[index] = item;
    }
    // Where the old items were positioned, in the new order (the inserted ones are positioned anew).
    let mut positioned = state.positioned;
    if !splice.items.is_empty() || splice.inserted > 0 {
        let filler = *positioned.get(splice.items.start.min(positioned.len().saturating_sub(1)))?;
        positioned.splice(splice.items.clone(), core::iter::repeat_n(filler, splice.inserted));
    }

    // The changed items' flags, and the rows they are in.
    determine_if_item_crosses_flexible_or_intrinsic_tracks(&mut items, &columns, &rows, Some(&changed));
    for (item, _) in items.iter().zip(&changed).filter(|(_, changed)| **changed) {
        for track in item.row_indexes.start as usize + 1..item.row_indexes.end as usize {
            affected_tracks[track] = true;
        }
    }

    Some(Replay {
        items,
        col_counts,
        row_counts,
        columns: state.columns,
        rows,
        affected_tracks,
        changed,
        old_rows: state.rows,
        positioned,
    })
}

/// Whether a grid laid out with these tracks and items can be laid out again from its state.
#[allow(clippy::too_many_arguments)]
pub(super) fn can_record(
    columns: &[GridTrack],
    rows: &[GridTrack],
    items: &[GridItem],
    child_count: usize,
    col_counts: TrackCounts,
    row_counts: TrackCounts,
    inner_width_is_definite: bool,
    has_baseline_aligned_item: bool,
) -> bool {
    let implicit = |counts: TrackCounts| counts.negative_implicit != 0 || counts.positive_implicit != 0;
    items.len() == child_count
        && !has_baseline_aligned_item
        && !implicit(col_counts)
        && !implicit(row_counts)
        && columns.iter().all(|track| {
            !track.has_intrinsic_sizing_function()
                && !track.uses_percentage()
                && (inner_width_is_definite || !track.is_flexible())
                && track.content_alignment_adjustment == 0.0
                && !(track.is_collapsed && track.kind == GridTrackKind::Track)
        })
        && rows.iter().all(|track| !track.uses_percentage() && track.content_alignment_adjustment == 0.0 && !(track.is_collapsed && track.kind == GridTrackKind::Track))
        && items.iter().all(|item| {
            item.span(crate::geometry::AbstractAxis::Block) == 1 && item.oof_candidates.is_empty()
        })
}
