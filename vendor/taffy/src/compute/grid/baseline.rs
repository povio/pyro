//! pyro: baseline self-alignment of grid items in both axes (css-align-3 §9, css-grid-1 §11.5 step 1).
//!
//! Items with `align-self` (block axis) or `justify-self` (inline axis) `baseline` / `last baseline` form
//! baseline-sharing groups: the items in the same track that align to the same side of it. An item's group
//! aligns to the start of its track for a first baseline and to the end for a last baseline, the other way
//! round when the item's block flow runs opposite to the grid's in that axis (`vertical-rl` in `vertical-lr`,
//! or an orthogonal item whose block-start is the grid's inline-end). Items whose block axis is
//! perpendicular to the alignment axis, or that have no baseline, get one synthesized from their border box
//! ([`GridContainerStyle::baseline_synthesis`](crate::GridContainerStyle::baseline_synthesis)).
//!
//! Each item gets a shim (an extra margin on its group's side) so that, aligned to that side, the baselines
//! of its group line up. Shims count in the items' contributions to track sizing; they are computed for an
//! axis as its tracks are sized (with the other axis' sizes when known), and again with the final sizes
//! before the items are placed.

use super::types::{GridItem, GridTrack};
use crate::compute::common::alignment::child_first_baseline;
use crate::geometry::{AbstractAxis, Line, Size};
use crate::style::AvailableSpace;
use crate::tree::{LayoutPartialTree, LayoutPartialTreeExt, SizingMode};
use crate::util::sys::Vec;
use crate::util::ResolveOrZero;
use crate::{AlignItemsKeyword, Direction};

/// The size of the area an item spans in `tracks` (tracks and the gutters between them)
fn span_size(tracks: &[GridTrack], range: Line<u16>) -> f32 {
    let (start, end) = (range.start as usize + 1, range.end as usize);
    if start >= end || end > tracks.len() {
        return 0.0;
    }
    tracks[start..end].iter().map(|track| track.base_size).sum()
}

/// Compute the baseline shims of the items in `axis`. `columns` and `rows` are the track sizes to lay the
/// items out with, when known.
#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_baseline_shims(
    tree: &mut impl LayoutPartialTree,
    axis: AbstractAxis,
    items: &mut [GridItem],
    columns: Option<&[GridTrack]>,
    rows: Option<&[GridTrack]>,
    inner_node_size: Size<Option<f32>>,
    synthesis: Size<f32>,
    direction: Direction,
) {
    // (key: whether the group aligns to the end, the line it aligns to; value: the item's distance from
    // that side of its margin box to its baseline)
    let mut measured: Vec<(usize, bool, u16, f32)> = Vec::new();
    for (index, item) in items.iter_mut().enumerate() {
        match axis {
            AbstractAxis::Block => {
                item.baseline_shim = 0.0;
                item.baseline_shim_block_end = 0.0;
            }
            AbstractAxis::Inline => item.baseline_shim_inline = Line { start: 0.0, end: 0.0 },
        }
        if !item.participates_in_baseline_alignment_in(axis) {
            continue;
        }
        item.known_dimensions_cache = None;
        item.min_content_contribution_cache = Size::NONE;
        item.max_content_contribution_cache = Size::NONE;
        item.minimum_contribution_cache = Size::NONE;

        let area = Size {
            width: columns.map(|tracks| span_size(tracks, item.column_indexes)),
            height: rows.map(|tracks| span_size(tracks, item.row_indexes)),
        };
        let margin_basis = area.width.or(inner_node_size.width);
        let margin = item.margin.resolve_or_zero(margin_basis, |val, basis| tree.calc(val, basis));
        let known = item.known_dimensions_cached(tree, area);
        let available = Size {
            width: area
                .width
                .map(|width| {
                    AvailableSpace::Definite(
                        (width - margin.left - margin.right - item.baseline_shim_inline.sum()).max(0.0),
                    )
                })
                .unwrap_or(AvailableSpace::MaxContent),
            height: area
                .height
                .map(|height| {
                    AvailableSpace::Definite(
                        (height - margin.top - margin.bottom - item.baseline_shim - item.baseline_shim_block_end)
                            .max(0.0),
                    )
                })
                .unwrap_or(AvailableSpace::MaxContent),
        };
        let output = tree.perform_child_layout(item.node, known, area, available, SizingMode::InherentSize, Line::FALSE);
        let size = known.get(axis).unwrap_or(output.size.get(axis));

        let is_last = match axis {
            AbstractAxis::Inline => item.justify_self.keyword,
            AbstractAxis::Block => item.align_self.keyword,
        } == AlignItemsKeyword::LastBaseline;
        let (flow, raw, overflow) = match axis {
            AbstractAxis::Inline => (
                item.baseline_flow.width,
                if is_last { output.baselines.orthogonal_last } else { output.baselines.orthogonal_first },
                item.overflow.x,
            ),
            AbstractAxis::Block => (
                item.baseline_flow.height,
                {
                    let (first, last) = output.baselines.dominant();
                    if is_last {
                        last
                    } else {
                        first
                    }
                },
                item.overflow.y,
            ),
        };
        let raw = flow.and_then(|_| child_first_baseline(raw, size, overflow));
        // The baseline's distance from the item's border-box start edge in the grid's axis
        let from_start = match (flow, raw) {
            (Some(reversed), Some(baseline)) => {
                if reversed {
                    size - baseline
                } else {
                    baseline
                }
            }
            _ => size * synthesis.get(axis),
        };
        let (margin_start, margin_end) = match axis {
            AbstractAxis::Block => (margin.top, margin.bottom),
            AbstractAxis::Inline if direction.is_rtl() => (margin.right, margin.left),
            AbstractAxis::Inline => (margin.left, margin.right),
        };
        let is_end = item.baseline_group_is_end(axis);
        let placement = item.placement_indexes(axis);
        let (line, distance) =
            if is_end { (placement.end, margin_end + size - from_start) } else { (placement.start, margin_start + from_start) };
        measured.push((index, is_end, line, distance));
    }

    for &(index, is_end, line, distance) in &measured {
        let group_max = measured
            .iter()
            .filter(|other| other.1 == is_end && other.2 == line)
            .map(|other| other.3)
            .fold(f32::NEG_INFINITY, f32::max);
        let shim = group_max - distance;
        let item = &mut items[index];
        match axis {
            AbstractAxis::Block if is_end => item.baseline_shim_block_end = shim,
            AbstractAxis::Block => item.baseline_shim = shim,
            AbstractAxis::Inline => {
                // `baseline_shim_inline` is physical: the logical start is the right side in `rtl`
                if is_end != direction.is_rtl() {
                    item.baseline_shim_inline.end = shim;
                } else {
                    item.baseline_shim_inline.start = shim;
                }
            }
        }
        item.known_dimensions_cache = None;
    }
}
