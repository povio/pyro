//! pyro: table cells aligned on their row's baseline (`vertical-align: baseline`, CSS 2.1 §17.5.3;
//! css-tables-3 "row layout").
//!
//! A table's grid (`LayoutGridContainer::grid_rows_are_table_rows`) asks the tree which of its cells
//! are baseline-aligned (`table_cell_is_baseline_aligned`; only cells spanning one row). Before its
//! rows are sized, each such cell of a row with two or more of them is laid out at the width of its
//! columns: its baseline is its first line box's (its content's first baseline), else, when it has
//! content, the bottom of its content box; a cell with neither takes no part. The row's baseline is
//! the lowest of its cells', and each cell's `baseline_shim` is the row's baseline less its own: its
//! contribution to the row's height includes the shim as it includes a margin. The cell is then
//! positioned as any other cell, filling its row, and its content moves down by the shim inside it
//! (`set_table_cell_baseline_offset`), as Chromium's intrinsic padding does.

use super::types::{GridItem, GridTrack};
use crate::geometry::{AbstractAxis, Line, Size};
use crate::style::AvailableSpace;
use crate::tree::{LayoutPartialTreeExt, NodeId, SizingMode};
use crate::util::ResolveOrZero;
use crate::LayoutGridContainer;

/// Marks the table's baseline-aligned cells (`GridItem::table_baseline`). Whether any is.
pub(super) fn mark_table_baseline_cells(tree: &impl LayoutGridContainer, node: NodeId, items: &mut [GridItem]) -> bool {
    if !tree.table_has_baseline_cells(node) {
        return false;
    }
    let mut any = false;
    for item in items.iter_mut() {
        item.table_baseline =
            item.span(AbstractAxis::Block) == 1 && tree.table_cell_is_baseline_aligned(node, item.node);
        any |= item.table_baseline;
    }
    any
}

/// The shims of the baseline-aligned cells (see the module documentation). `items` are in document order,
/// a table's cells row by row; `columns` are sized.
pub(super) fn resolve_table_cell_baselines<Tree: LayoutGridContainer>(
    tree: &mut Tree,
    node: NodeId,
    items: &mut [GridItem],
    rows: &[GridTrack],
    columns: &[GridTrack],
    inner_node_size: Size<Option<f32>>,
) {
    let mut start = 0;
    while start < items.len() {
        let row = items[start].row.start;
        let end = items[start..].iter().position(|item| item.row.start != row).map_or(items.len(), |n| start + n);
        let run = &mut items[start..end];
        start = end;
        for item in run.iter_mut().filter(|item| item.table_baseline) {
            item.baseline_shim = 0.0;
            item.baseline = None;
            item.known_dimensions_cache = None;
        }
        if run.iter().filter(|item| item.table_baseline).count() < 2 {
            continue;
        }
        let mut row_baseline: Option<f32> = None;
        for item in run.iter_mut().filter(|item| item.table_baseline) {
            let baseline = cell_baseline(tree, node, item, rows, columns, inner_node_size);
            item.baseline = baseline;
            if let Some(baseline) = baseline {
                row_baseline = Some(row_baseline.map_or(baseline, |row: f32| row.max(baseline)));
            }
        }
        let Some(row_baseline) = row_baseline else { continue };
        for item in run.iter_mut().filter(|item| item.table_baseline) {
            if let Some(baseline) = item.baseline {
                item.baseline_shim = row_baseline - baseline;
                item.known_dimensions_cache = None;
            }
        }
    }
}

/// A cell's baseline from the top of its border box, laid out at the width of its columns, or `None` for a
/// cell without content.
fn cell_baseline<Tree: LayoutGridContainer>(
    tree: &mut Tree,
    node: NodeId,
    item: &mut GridItem,
    rows: &[GridTrack],
    columns: &[GridTrack],
    inner_node_size: Size<Option<f32>>,
) -> Option<f32> {
    let calc = |val, basis| tree.calc(val, basis);
    let area = item.grid_area_size(
        AbstractAxis::Block,
        rows,
        columns,
        inner_node_size,
        |track: &GridTrack, _| Some(track.base_size),
        &calc,
    );
    let known = item.known_dimensions_in(tree, area);
    let width = known.width.or(area.width);
    let output = tree.perform_child_layout(
        item.node,
        Size { width, height: known.height },
        area,
        Size {
            width: width.map_or(AvailableSpace::MaxContent, AvailableSpace::Definite),
            height: AvailableSpace::MaxContent,
        },
        SizingMode::InherentSize,
        Line::FALSE,
    );
    // (A layout run since the cell's last final layout has its content that much lower.)
    let offset = tree.table_cell_baseline_offset(node, item.node);
    if let Some(baseline) = output.baselines.first {
        return Some(baseline - offset);
    }
    let padding = item.padding.resolve_or_zero(area.width, |val, basis| tree.calc(val, basis));
    let border = item.border.resolve_or_zero(area.width, |val, basis| tree.calc(val, basis));
    let height = output.size.height;
    let content = height - padding.top - padding.bottom - border.top - border.bottom;
    (content > 0.0).then(|| height - padding.bottom - border.bottom)
}
