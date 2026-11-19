use blitz_traits::node_id::NodeId;
use std::{ops::Range, sync::Arc};
use style::logical_geometry::WritingMode;
use stylo_taffy::CopyStyleGetters;

use atomic_refcell::AtomicRefCell;
use markup5ever::local_name;
use style::properties::style_structs::Border;
use style::servo_arc::Arc as ServoArc;
use style::typed_om::NumericBaseType;
use style::values::computed::calc::CalcPercentageLeaf;
use style::values::computed::length_percentage::{
    CalcLengthPercentage, CalcNode, ComputedLeaf, Unpacked as UnpackedLengthPercentage,
};
use style::values::computed::{Length, LengthPercentage};
use style::values::generics::Optional;
use style::values::specified::box_::{DisplayInside, DisplayOutside};
use style::{
    Atom, computed_values::border_collapse::T as BorderCollapse,
    computed_values::table_layout::T as TableLayout, values::computed::BorderStyle,
};
use style_traits::values::specified::AllowedNumericType;
use taffy::{
    DetailedGridInfo, LayoutPartialTree as _, ResolveOrZero, TrackSizingFunction, style_helpers,
};

use crate::BaseDocument;
use crate::layout::LayoutPassState;
use crate::node::{NodeFlags, SpecialElementData};

use super::damage::{CONSTRUCT_BOX, CONSTRUCT_DESCENDENT, CONSTRUCT_FC};
use super::resolve_calc_value;
use super::table_fixup::{Kind, Level};

pub struct TableTreeWrapper<'a, 'doc> {
    pub(crate) doc: &'a mut LayoutPassState<'doc>,
    pub(crate) ctx: Arc<TableContext>,
    /// The table's style with the column widths of [`Self::auto_column_widths`].
    pub(crate) style: Option<taffy::Style<Atom>>,
    /// pyro: whether the table's height is definite (a length, a percentage of a definite height,
    /// or one its flex or grid parent gave it). Else a cell's stretched height is no basis for its
    /// children's percentage heights: they resolve against the cell's own `height` when that is a
    /// length, else are indefinite (CSS Tables 3 §row-layout, as Blink; WPT css-tables
    /// `percent-height-table-cell-child`, `percent-height-replaced-in-percent-cell-003`).
    pub(crate) definite_height: bool,
    /// pyro: whether the table's `height` is not `auto` (`LayoutPassState::restricted_cell`).
    pub(crate) restricted: bool,
    /// pyro: the inline size of the grid (the table's content box, its outer border spacing
    /// aside) in the grid layout running, when the table's width is known
    /// (`Self::layout_grid`): what its cells' percentage padding resolves against.
    pub(crate) grid_width: Option<f32>,
}

impl TableTreeWrapper<'_, '_> {
    /// The table's style for a layout at `inputs`, with the column widths the
    /// automatic table layout algorithm gives its columns when they are all
    /// auto or all simple length-hinted tracks: a table wider than its columns' min-content widths
    /// shares the extra space in proportion to their max-content widths
    /// (from min-content up to max-content, then beyond it), where a grid
    /// shares it equally. `None`: the table's own grid tracks apply.
    pub(crate) fn auto_column_widths(
        &mut self,
        inputs: &taffy::tree::LayoutInput,
    ) -> Option<taffy::Style<Atom>> {
        use taffy::MaybeResolve as _;
        let ctx = Arc::clone(&self.ctx);
        let style = &ctx.style;
        let n = style.grid_template_columns.len();
        let all_auto = style.grid_template_columns.iter().all(|c| {
            matches!(c, taffy::GridTemplateComponent::Single(t) if t.min.is_auto() && t.max.is_auto())
        });
        let all_hinted = style.grid_template_columns.iter().all(|c| {
            matches!(c, taffy::GridTemplateComponent::Single(t)
                if t.min.is_min_content() && t.max.into_raw().tag() == taffy::CompactLength::FIT_CONTENT_PX_TAG)
        });
        if !ctx.is_fixed && n > 0 && self.has_percent_cell(&ctx) {
            return self.percent_column_widths(inputs, &ctx);
        }
        if ctx.is_fixed || n == 0 {
            return None;
        }
        // pyro: columns of mixed kinds (a `<col>` width beside auto columns, say) get the automatic
        // table layout's widths too (`percent_column_widths`, Blink's distribution), not the grid's.
        if !all_auto {
            return self.percent_column_widths(inputs, &ctx);
        }
        // The new hinted-track path inspects physical widths. Keep vertical
        // and orthogonal cells on the existing logical-axis grid fallback.
        #[cfg(feature = "writing-mode")]
        if all_hinted && (self.doc.layout_wm.is_vertical() || ctx.cells.iter().any(|cell| {
            self.doc.nodes[cell.node_id].primary_styles().is_some_and(|style| style.writing_mode.is_vertical())
        })) {
            return None;
        }
        // Keep percentage/calc cell sizing on the existing grid fallback
        // when extending allocation to previously unsupported hinted tracks.
        if all_hinted && ctx.cells.iter().any(|cell| {
            physical_size(&self.doc.nodes[cell.node_id]).is_some_and(|size| {
                !matches!(size.width.tag(), taffy::CompactLength::AUTO_TAG | taffy::CompactLength::LENGTH_TAG)
            })
        }) {
            return None;
        }
        let parent = inputs.parent_size.width;
        let padding = style.padding.resolve_or_zero(parent, resolve_calc_value);
        let border = style.border.resolve_or_zero(parent, resolve_calc_value);
        let extra = padding.left + padding.right + border.left + border.right;
        let border_box = |w: Option<f32>| {
            w.map(|w| match style.box_sizing {
                taffy::BoxSizing::ContentBox => w + extra,
                taffy::BoxSizing::BorderBox => w.max(extra),
            })
        };
        let min_width = border_box(style.min_size.width.maybe_resolve(parent, resolve_calc_value));
        // pyro: a table that shrinks to fit its columns is no wider than its `max-width` (nor
        // narrower than its columns' min-content widths: WPT css-text
        // `overflow-wrap-min-content-size-001`, `max-width: 0`).
        let max_width = border_box(style.max_size.width.maybe_resolve(parent, resolve_calc_value));
        // The table's width when it doesn't depend on its columns; else the
        // most it may take (it shrinks to fit its columns' max-content).
        let (width, shrink) = match inputs.known_dimensions.width {
            Some(w) => (w, false),
            None => match border_box(style.size.width.maybe_resolve(parent, resolve_calc_value)) {
                Some(w) => (w.max(min_width.unwrap_or(0.0)), false),
                None => match inputs.available_space.width {
                    taffy::AvailableSpace::Definite(a) => (a.max(min_width.unwrap_or(0.0)), true),
                    _ => return None,
                },
            },
        };
        let gap = style.gap.width.resolve_or_zero(Some(width), resolve_calc_value);
        let spacing = extra + gap * (n - 1) as f32;
        let inner = width - spacing;
        let measure = |this: &mut Self, idx: usize, space: taffy::AvailableSpace| {
            let input = taffy::tree::LayoutInput {
                run_mode: taffy::RunMode::ComputeSize,
                sizing_mode: taffy::SizingMode::InherentSize,
                axis: taffy::RequestedAxis::Horizontal,
                known_dimensions: taffy::Size::NONE,
                known_dimensions_are_definite: taffy::Size { width: true, height: true },
                // pyro: a cell's percentage padding resolves against zero in the column widths
                // (CSS Tables 3, as Blink and Gecko; `TableTreeWrapper::grid_inline_size`).
                parent_size: taffy::Size { width: Some(0.0), height: None },
                available_space: taffy::Size { width: space, height: taffy::AvailableSpace::MaxContent },
                vertical_margins_are_collapsible: taffy::Line::FALSE,
            };
            // A width-hinted cell's preferred width is its hint or its
            // min-content width, whichever is larger (Chromium's automatic
            // table layout), which the grid's capped contribution gives.
            this.compute_child_layout(taffy::NodeId::from(idx), input).size.width
        };
        let mut min: Vec<f32> = style.grid_template_columns.iter().map(|column| match column {
            taffy::GridTemplateComponent::Single(track) if track.max.into_raw().tag() == taffy::CompactLength::FIT_CONTENT_PX_TAG => track.max.into_raw().value(),
            _ => 0.0,
        }).collect();
        let mut max = min.clone();
        // pyro: the cells' widths are those last measured, but for the cells that changed
        // (`CellWidths`); a column's are summed again only when a cell that set them got narrower.
        // Another table content width (a shrink-to-fit table whose container got wider or
        // narrower) changes the intrinsic widths of the cells with percentage or `calc()` lengths
        // of their own alone (`TableCell::percent_lengths`): those are measured again.
        let mut widths = ctx.widths.borrow_mut();
        if widths.cells.len() == ctx.cells.len() && widths.inner != inner {
            widths.inner = inner;
            for (idx, cell) in ctx.cells.iter().enumerate() {
                if cell.percent_lengths {
                    widths.changed(idx);
                }
            }
        }
        if widths.cells.len() != ctx.cells.len() {
            *widths = CellWidths { inner, cells: vec![None; ctx.cells.len()], ..CellWidths::default() };
        }
        widths.update(&ctx, n, |idx| {
            let lo = measure(self, idx, taffy::AvailableSpace::MinContent);
            (lo, measure(self, idx, taffy::AvailableSpace::MaxContent).max(lo))
        });
        let columns = widths.columns.as_ref().unwrap();
        for c in 0..n {
            min[c] = min[c].max(columns.min[c]);
            max[c] = max[c].max(columns.max[c]);
        }
        let spanning: Vec<(usize, usize, f32, f32)> = columns
            .spanning
            .iter()
            .map(|&idx| {
                let cell = &ctx.cells[idx];
                let (lo, hi) = widths.cells[idx].unwrap_or_default();
                (cell.column as usize, (cell.colspan as usize).max(1), lo, hi)
            })
            .collect();
        drop(widths);
        // A spanning cell wider than the columns it spans: leave it to the grid.
        // pyro: but for one whose max-content width alone is (its min-content width fits) in a
        // table narrower than its columns' min-content widths: the columns are then as wide as
        // those, whatever the max-content widths are (the grid would keep the table's `width`
        // and let the cells overflow it).
        let sum_min = min.iter().sum::<f32>();
        let at_min = inner < sum_min && !shrink;
        for (column, span, lo, hi) in spanning {
            let end = (column + span).min(n);
            let spacing = gap * (end.saturating_sub(column + 1)) as f32;
            let sum = |w: &[f32]| w[column.min(end)..end].iter().sum::<f32>() + spacing;
            if lo > sum(&min) + 0.01 || (hi > sum(&max) + 0.01 && !at_min) {
                // pyro: distributed as Blink does (`percent_column_widths`), not by the grid, which
                // shares a spanning cell's excess over fixed columns too (WPT css-tables `colspan-004`).
                return self.percent_column_widths(inputs, &ctx);
            }
        }
        let (sum_min, sum_max) = (min.iter().sum::<f32>(), max.iter().sum::<f32>());
        if sum_max <= 0.0 {
            return None;
        }
        let mut style = style.clone();
        raise_max_width_to_columns(&mut style, sum_min + gap * (n - 1) as f32, extra, parent);
        if inner < sum_min {
            // A table never gets narrower than its columns' min-content widths.
            if shrink {
                return None;
            }
            let content = sum_min + gap * (n - 1) as f32;
            let grown = match style.box_sizing {
                taffy::BoxSizing::ContentBox => content,
                taffy::BoxSizing::BorderBox => content + extra,
            };
            style.min_size.width = style_helpers::length(grown);
            style.grid_template_columns = min.into_iter().map(|w| {
                let track: TrackSizingFunction = style_helpers::length(w);
                track.into()
            }).collect();
            return Some(style);
        }
        let min_inner = min_width.map_or(0.0, |w| w - spacing);
        let inner = if shrink {
            let max_inner = max_width.map_or(f32::INFINITY, |w| w - spacing);
            inner.min(sum_max.max(min_inner)).min(max_inner.max(sum_min).max(min_inner))
        } else {
            inner
        };
        let widths: Vec<f32> = if inner >= sum_max {
            max.iter().map(|m| m * inner / sum_max).collect()
        } else if sum_max > sum_min {
            let t = (inner - sum_min) / (sum_max - sum_min);
            min.iter().zip(&max).map(|(lo, hi)| lo + (hi - lo) * t).collect()
        } else {
            min
        };
        style.grid_template_columns = widths
            .into_iter()
            .map(|w| {
                let track: TrackSizingFunction = style_helpers::length(w);
                track.into()
            })
            .collect();
        Some(style)
    }
}

impl TableTreeWrapper<'_, '_> {
    /// The cell's computed `width`.
    fn cell_width(&self, cell: &TableCell) -> Option<taffy::Dimension> {
        physical_size(&self.doc.nodes[cell.node_id]).map(|size| size.width)
    }

    /// Whether the table is in a table cell.
    fn is_inside_a_cell(&self, ctx: &TableContext) -> bool {
        use style::values::specified::box_::DisplayInside;
        let Some(cell) = ctx.cells.first() else { return false };
        let mut at = self.doc.nodes[cell.node_id].parent;
        while let Some(id) = at {
            let node = &self.doc.nodes[id];
            // pyro: a flex or grid container too, as Blink's `TableNode::AllowColumnPercentages`: measured
            // there, a table with a 100% column was as wide as `f32::MAX / 4` (WPT
            // `table-with-infinite-max-intrinsic-width`, in css-flexbox and css-grid)
            if node.primary_styles().is_some_and(|s| {
                matches!(s.clone_display().inside(), DisplayInside::TableCell | DisplayInside::Flex | DisplayInside::Grid)
            }) {
                return true;
            }
            at = node.parent;
        }
        false
    }

    /// Whether a cell or `<col>` of an auto table has a percentage `width`.
    fn has_percent_cell(&self, ctx: &TableContext) -> bool {
        ctx.column_percents.iter().any(Option::is_some) || ctx.percent_cells > 0
    }

    /// The column widths of an auto table with percentage columns, as Blink
    /// (`DistributeInlineSizeToComputedInlineSizeAuto`, css-tables-3's width
    /// distribution): a percentage takes its share of the table, whatever its
    /// content, and only its content's min-content width limits it. The guesses
    /// go min-content, then percentages, then fixed widths, then auto columns'
    /// max-content, and the table's width falls between two of them; above the
    /// last, auto columns take the rest, else fixed, else percentage ones.
    fn percent_column_widths(
        &mut self,
        inputs: &taffy::tree::LayoutInput,
        ctx: &TableContext,
    ) -> Option<taffy::Style<Atom>> {
        use taffy::MaybeResolve as _;
        let style = &ctx.style;
        let n = style.grid_template_columns.len();
        #[cfg(feature = "writing-mode")]
        if self.doc.layout_wm.is_vertical() {
            return None;
        }
        let parent = inputs.parent_size.width;
        let padding = style.padding.resolve_or_zero(parent, resolve_calc_value);
        let border = style.border.resolve_or_zero(parent, resolve_calc_value);
        let extra = padding.left + padding.right + border.left + border.right;
        let gap = style.gap.width.resolve_or_zero(parent, resolve_calc_value);
        let spacing = extra + gap * (n - 1) as f32;
        let mut min = vec![0.0f32; n];
        let mut max = vec![0.0f32; n];
        let mut percent: Vec<Option<f32>> = (0..n).map(|c| ctx.column_percents.get(c).copied().flatten()).collect();
        let mut fixed = vec![false; n];
        // `<col>` and first-row lengths (pyro: also as the `fit-content(<length>)` hint tracks of
        // columns with cells, `build_table_context`).
        for (c, track) in style.grid_template_columns.iter().enumerate() {
            if let taffy::GridTemplateComponent::Single(track) = track {
                if matches!(track.max.into_raw().tag(), taffy::CompactLength::LENGTH_TAG | taffy::CompactLength::FIT_CONTENT_PX_TAG) {
                    let len = track.max.into_raw().value();
                    max[c] = len;
                    fixed[c] = len > 0.0;
                }
                // pyro: a `<col>`'s length `min-width` (`push_columns`: `minmax(<length>, auto)`)
                // is a floor of the column (WPT css-tables `col-definite-min-size-001`).
                if track.min.into_raw().tag() == taffy::CompactLength::LENGTH_TAG {
                    let len = track.min.into_raw().value();
                    min[c] = min[c].max(len);
                    max[c] = max[c].max(len);
                }
            }
        }
        let measure = |this: &mut Self, idx: usize, space: taffy::AvailableSpace| {
            let input = taffy::tree::LayoutInput {
                run_mode: taffy::RunMode::ComputeSize,
                // The cell's `width` is not its content: read apart below.
                sizing_mode: taffy::SizingMode::ContentSize,
                axis: taffy::RequestedAxis::Horizontal,
                known_dimensions: taffy::Size::NONE,
                known_dimensions_are_definite: taffy::Size { width: true, height: true },
                parent_size: taffy::Size::NONE,
                available_space: taffy::Size { width: space, height: taffy::AvailableSpace::MaxContent },
                vertical_margins_are_collapsible: taffy::Line::FALSE,
            };
            let cell = crate::taffy_node_id(this.ctx.cells[idx].node_id);
            this.doc.compute_child_layout(cell, input).size.width
        };
        let mut spanning = Vec::new();
        for (idx, cell) in ctx.cells.iter().enumerate() {
            let column = cell.column as usize;
            // (Its own `min-width` floors both, as `compute_child_layout` does for the grid path.)
            let lo = measure(self, idx, taffy::AvailableSpace::MinContent).max(cell_own_min_width(&cell.style, None).unwrap_or(0.0));
            let hi = measure(self, idx, taffy::AvailableSpace::MaxContent).max(lo);
            if cell.colspan > 1 || column >= n {
                // pyro: a spanning cell's length `width` is its max-content contribution (its
                // border box), as a single cell's is its column's.
                let hi = match self.cell_width(cell) {
                    Some(width) if width.tag() == taffy::CompactLength::LENGTH_TAG => {
                        let box_extra = match cell.style.box_sizing {
                            taffy::BoxSizing::ContentBox => {
                                let p = cell.style.padding.resolve_or_zero(None, resolve_calc_value);
                                let b = cell.style.border.resolve_or_zero(None, resolve_calc_value);
                                (p + b).horizontal_axis_sum()
                            }
                            taffy::BoxSizing::BorderBox => 0.0,
                        };
                        hi.max(cell.min_width.or(cell.length_width).unwrap_or(width.value() + box_extra))
                    }
                    _ => hi,
                };
                spanning.push((column, (cell.colspan as usize).max(1), lo, hi));
                continue;
            }
            min[column] = min[column].max(lo);
            let Some(width) = self.cell_width(cell) else { continue };
            match width.tag() {
                taffy::CompactLength::PERCENT_TAG => {
                    // A percentage cell's width is its share, of the table.
                    let share = width.value();
                    percent[column] = Some(percent[column].unwrap_or(0.0).max(share));
                    max[column] = max[column].max(hi);
                }
                taffy::CompactLength::LENGTH_TAG => {
                    let box_extra = match cell.style.box_sizing {
                        taffy::BoxSizing::ContentBox => {
                            let p = cell.style.padding.resolve_or_zero(None, resolve_calc_value);
                            let b = cell.style.border.resolve_or_zero(None, resolve_calc_value);
                            (p + b).horizontal_axis_sum()
                        }
                        taffy::BoxSizing::BorderBox => 0.0,
                    };
                    fixed[column] = true;
                    // (`min_width`: its border box with its halves of collapsed borders, no wider
                    // than a length `max-width` says, `length_width` before the collapsed borders)
                    max[column] = max[column]
                        .max(cell.min_width.or(cell.length_width).unwrap_or(width.value() + box_extra));
                }
                // pyro: a column with a length width (its `<col>`'s) keeps it as its max-content
                // width: its auto cells' content widens it only to their min-content width (Blink;
                // WPT CSS2 `vertical-align-baseline-003`'s reference).
                taffy::CompactLength::AUTO_TAG if fixed[column] => {}
                taffy::CompactLength::AUTO_TAG => max[column] = max[column].max(hi),
                _ => return None,
            }
        }
        for c in 0..n {
            max[c] = max[c].max(min[c]);
        }
        // A spanning cell wider than the columns it spans widens those without a percentage,
        // in proportion to their max-content widths (all of them when each has one).
        for (column, span, lo, hi) in spanning {
            let end = (column + span).min(n);
            let columns = column.min(end)..end;
            let spacing = gap * columns.len().saturating_sub(1) as f32;
            // pyro: auto columns first, then fixed ones, then percentage ones (Blink's
            // `DistributeColspanCellToColumns`: unconstrained columns take a spanning cell's excess).
            let auto: Vec<usize> = columns.clone().filter(|&c| percent[c].is_none() && !fixed[c]).collect();
            let free: Vec<usize> = columns.clone().filter(|&c| percent[c].is_none()).collect();
            let group = if !auto.is_empty() {
                auto
            } else if free.is_empty() {
                columns.collect()
            } else {
                free
            };
            let total: f32 = group.iter().map(|&c| max[c]).sum();
            let snapshot = max.clone();
            let max_share = |c: usize, total: f32| snapshot[c] / total;
            for (want, sizes) in [(lo, &mut min), (hi, &mut max)] {
                let have = (column.min(end)..end).map(|c| sizes[c]).sum::<f32>() + spacing;
                if want > have {
                    for &c in &group {
                        let part = if total > 0.0 { max_share(c, total) } else { 1.0 / group.len() as f32 };
                        sizes[c] += (want - have) * part;
                    }
                }
            }
        }
        for c in 0..n {
            max[c] = max[c].max(min[c]);
        }
        // Percentages add up to 100 at most, left to right; 0% is none.
        let mut total = 0.0;
        for p in percent.iter_mut() {
            if let Some(v) = *p {
                let v = v.min(1.0 - total);
                total += v;
                *p = (v > 0.0).then_some(v);
            }
        }
        let sum_min: f32 = min.iter().sum();
        // `ComputeGridInlineMinMax`: what percentages ask of the table's width.
        // (Not while measuring a table in a cell: `TableNode::AllowColumnPercentages`.)
        let measuring_in_cell = !matches!(inputs.available_space.width, taffy::AvailableSpace::Definite(_))
            && self.is_inside_a_cell(ctx);
        let grid_max = {
            let mut grid_max: f32 = max.iter().sum();
            let mut estimate = 0.0f32;
            let mut others = 0.0f32;
            for c in 0..n {
                match percent[c] {
                    Some(p) => estimate = estimate.max(max[c] / p),
                    None => others += max[c],
                }
            }
            let total: f32 = percent.iter().flatten().sum();
            if total > 0.0 && !measuring_in_cell {
                if others > 0.0 {
                    grid_max = grid_max.max(if total >= 0.9999 { f32::MAX / 4.0 } else { others / (1.0 - total) });
                }
                grid_max = grid_max.max(estimate);
            }
            grid_max.max(sum_min)
        };
        let border_box = |w: Option<f32>| {
            w.map(|w| match style.box_sizing {
                taffy::BoxSizing::ContentBox => w + extra,
                taffy::BoxSizing::BorderBox => w.max(extra),
            })
        };
        let min_width = border_box(style.min_size.width.maybe_resolve(parent, resolve_calc_value)).unwrap_or(0.0);
        // (No wider than its `max-width` says, which its `min-width` beats: CSS 2.1 § 10.4.)
        let max_width = border_box(style.max_size.width.maybe_resolve(parent, resolve_calc_value)).unwrap_or(f32::INFINITY);
        let inner = match inputs.known_dimensions.width {
            Some(w) => w - spacing,
            None => match border_box(style.size.width.maybe_resolve(parent, resolve_calc_value)) {
                Some(w) => w.min(max_width).max(min_width) - spacing,
                None => match inputs.available_space.width {
                    // A table shrinks to fit its columns.
                    taffy::AvailableSpace::Definite(a) => {
                        (a.min(max_width).max(min_width) - spacing).min(grid_max.max(min_width - spacing))
                    }
                    taffy::AvailableSpace::MinContent => sum_min.max(min_width - spacing),
                    taffy::AvailableSpace::MaxContent => grid_max.max(min_width - spacing),
                },
            },
        };
        let target = inner.max(sum_min);
        let share = |c: usize| percent[c].map(|p| min[c].max(p * target));
        // Guesses: min-content, percentages, fixed, max-content.
        let guess_percent: f32 = (0..n).map(|c| share(c).unwrap_or(min[c])).sum();
        let guess_fixed: f32 = (0..n).map(|c| share(c).unwrap_or(if fixed[c] { max[c] } else { min[c] })).sum();
        let guess_max: f32 = (0..n).map(|c| share(c).unwrap_or(max[c])).sum();
        let grown = |base: &[f32], top: &[f32], left: f32| -> Vec<f32> {
            let room: f32 = (0..n).map(|c| top[c] - base[c]).sum();
            let growing = (0..n).filter(|&c| top[c] != base[c]).count().max(1);
            (0..n)
                .map(|c| {
                    let part = if room > 0.0 {
                        (top[c] - base[c]) / room
                    } else {
                        1.0 / growing as f32
                    };
                    base[c] + left * part
                })
                .collect()
        };
        let at_percent: Vec<f32> = (0..n).map(|c| share(c).unwrap_or(min[c])).collect();
        let at_fixed: Vec<f32> = (0..n).map(|c| share(c).unwrap_or(if fixed[c] { max[c] } else { min[c] })).collect();
        let at_max: Vec<f32> = (0..n).map(|c| share(c).unwrap_or(max[c])).collect();
        let widths: Vec<f32> = if target <= sum_min {
            min.clone()
        } else if target <= guess_percent {
            grown(&min, &at_percent, target - sum_min)
        } else if target <= guess_fixed {
            grown(&at_percent, &at_fixed, target - guess_percent)
        } else if target == guess_max {
            at_max
        } else if target < guess_max {
            grown(&at_fixed, &at_max, target - guess_fixed)
        } else {
            let left = target - guess_max;
            let auto: Vec<usize> = (0..n).filter(|&c| percent[c].is_none() && !fixed[c]).collect();
            let fixed_columns: Vec<usize> = (0..n).filter(|&c| percent[c].is_none() && fixed[c]).collect();
            let mut out = at_max;
            let group = if !auto.is_empty() { auto } else { fixed_columns };
            if !group.is_empty() {
                let sum: f32 = group.iter().map(|&c| max[c]).sum();
                for &c in &group {
                    out[c] += if sum > 0.0 { left * max[c] / sum } else { left / group.len() as f32 };
                }
            } else {
                let total: f32 = percent.iter().flatten().sum();
                for c in 0..n {
                    if let Some(p) = percent[c] {
                        out[c] += left * p / total;
                    }
                }
            }
            out
        };
        let mut style = style.clone();
        raise_max_width_to_columns(&mut style, sum_min + gap * (n - 1) as f32, extra, parent);
        if inner < sum_min {
            let content = sum_min + gap * (n - 1) as f32;
            let grown = match style.box_sizing {
                taffy::BoxSizing::ContentBox => content,
                taffy::BoxSizing::BorderBox => content + extra,
            };
            style.min_size.width = style_helpers::length(grown);
        }
        style.grid_template_columns = widths
            .into_iter()
            .map(|w| {
                let track: TrackSizingFunction = style_helpers::length(w);
                track.into()
            })
            .collect();
        Some(style)
    }
}

// Deliberately not `Clone`: `style` may hold raw pointers into `calc_values`.
#[derive(Debug)]
pub struct TableContext {
    pub style: taffy::Style<Atom>,
    pub cells: Vec<TableCell>,
    /// Absolutely positioned descendants of the table's row groups and rows (and of the
    /// table itself). They take no part in the table grid.
    pub oof_children: Vec<(NodeId, taffy::Position)>,
    /// pyro: the table's captions, in order: laid out above (`caption-side: top`) or below the
    /// grid, at the table's width (the table's box is then its wrapper box, captions included).
    pub captions: Vec<NodeId>,
    /// pyro: with captions, where the grid (the table box proper, which paints the table's
    /// background and borders) is in the table's box: its top and its height.
    pub grid_box: AtomicRefCell<Option<(f32, f32)>>,
    pub rows: Vec<TableRow>,
    pub columns: Vec<TableColumn>,
    /// The percentage of each `<col>` of an auto table (0 to 1), in the columns' order.
    pub column_percents: Vec<Option<f32>>,
    pub computed_grid_info: AtomicRefCell<Option<DetailedGridInfo<Atom>>>,
    pub border_style: Option<ServoArc<Border>>,
    pub border_collapse: BorderCollapse,
    /// The winning collapsed borders of a table whose borders are not one uniform grid. Cells then have
    /// half of the borders around them as their own, and the table half of its outer ones. A
    /// uniform table instead has them as gutters of the grid (`style.gap`) and this is `None`.
    pub collapsed_edges: Option<CollapsedEdges>,
    /// pyro: the borders of a collapsed table's row groups its collapsed borders were
    /// resolved with: a row group restyled with others is built again (`update_rows` collects only
    /// the rows of a group).
    group_borders: Vec<(NodeId, ServoArc<Border>)>,
    /// pyro: the collapsed borders of a collapsed table that has them as gutters (when
    /// `collapsed_edges` is `None`), kept so that a change can be resolved locally, and the
    /// gutters the first cell's borders give.
    spare_edges: Option<CollapsedEdges>,
    collapse_gap: (f32, f32),
    /// Laid out with the fixed table layout algorithm: cell contents don't size
    /// the columns, so cells have no intrinsic width.
    pub is_fixed: bool,
    /// pyro: what `update_table_context` checks a change against: whether it can update
    /// the table at all (no cell spans rows, no box is out of flow, each row's cells follow those of
    /// the rows before), the columns the cells reach, and the cells spanning one column, per column.
    incremental: bool,
    cursor_columns: u16,
    single_cell_columns: Vec<u32>,
    /// The cells with a percentage `width`.
    percent_cells: usize,
    /// pyro: the cells that changed (their subtree, or their entry) since the grid was last
    /// laid out (indices into `cells`; damage propagation and `update_table_context` add to it), the
    /// grid's last layout (`taffy::TableGridState`), and each cell's min-content and max-content
    /// widths as the automatic table layout last measured them: the data a layout of the table starts
    /// from, so that it measures and lays out again only the cells that changed.
    pub(crate) changed_cells: AtomicRefCell<Vec<usize>>,
    grid_state: AtomicRefCell<Option<Box<taffy::TableGridState>>>,
    /// The rows replaced by others since the grid's last layout, if any (`update_rows`).
    grid_splice: AtomicRefCell<Option<taffy::TableGridSplice>>,
    pub(crate) widths: AtomicRefCell<CellWidths>,
    /// pyro: the cells the grid placed since rounding and `resolve_transforms` last
    /// visited the table, and where the table was then (`TableTreeWrapper::set_unrounded_layout`): a
    /// cell neither placed nor damaged since is rounded and transformed as it was.
    pub(crate) placed: AtomicRefCell<PlacedCells>,
    /// The grid area of each cell (parallel to `cells`) and the number of columns and rows they reach,
    /// placed as the table is built (`place_cells`). The grid reads them through
    /// `LayoutGridContainer::track_counts`/`resolved_placement` instead of placing the cells on every layout.
    /// `None` when a cell's rows or span are outside what `place_cells` handles; the grid then places them.
    pub placement: Option<CellPlacement>,
    /// pyro: some cell is aligned on its row's baseline (`TableCell::baseline`).
    baseline_cells: bool,
    /// Backing storage for `calc()` track sizes synthesised by the table layout code.
    /// Taffy stores calc values as raw pointers, so these must outlive the `style`.
    #[allow(dead_code)]
    calc_values: Vec<LengthPercentage>,
}

impl TableContext {
    /// pyro: the colour of a collapsed table's borders laid out as one uniform grid of gutters, as
    /// conflict resolution gave them (`Some(None)`: none is visible); `None` when they were not
    /// resolved (a vertical table: the first cell's borders are the gutters' then).
    pub fn gutter_border_color(&self) -> Option<Option<style::color::AbsoluteColor>> {
        let edges = self.spare_edges.as_ref()?;
        Some(edges.edges().find(|edge| edge.width > 0.0).map(|edge| edge.color))
    }

    /// pyro: each row's hit-test extent (parallel to `rows`): the union of its cells'
    /// hit contributions in the table's coordinates at `scale` (`BaseDocument::resolve_hit_overflow`,
    /// device px), as of the last hit pass; `f` gets an empty slice when the pass has not
    /// covered the current rows (the caller then computes from the cells).
    pub fn row_hit_extents<R>(&self, f: impl FnOnce(&[kurbo::Rect], f64) -> R) -> R {
        let placed = self.placed.borrow();
        if placed.row_overflow.len() == self.rows.len() {
            f(&placed.row_overflow, placed.hit_scale)
        } else {
            f(&[], placed.hit_scale)
        }
    }
}

/// The cells a table's grid placed since its last rounding and transform pass (`TableContext::placed`).
#[derive(Debug, Default)]
pub(crate) struct PlacedCells {
    /// Cells placed since the last rounding (all of them when `None`), and the fractional part of
    /// the table's position then (all its cells' rounding depends on: `rounded_layout`)
    round: Option<Vec<usize>>,
    rounded_at: Option<taffy::Point<f32>>,
    /// Cells placed or damaged since the last transform pass (all of them when `None`), and each
    /// cell's overflow in the table as the last pass computed it, at its scale
    transform: Option<Vec<usize>>,
    pub(crate) overflow: Vec<kurbo::Rect>,
    pub(crate) overflow_scale: f64,
    /// pyro: cells the transform passes visited since the last hit pass (all of them
    /// when `None`: `BaseDocument::table_cells_hit`), each cell's hit contribution in the table as
    /// that pass computed it (parallel to `cells`; see `LayoutData::hit_overflow`), at its scale.
    hit_again: Option<Vec<usize>>,
    pub(crate) hit: Vec<kurbo::Rect>,
    pub(crate) hit_scale: f64,
    /// pyro: the union of `hit` over the cells originating in each row (parallel to
    /// `TableContext::rows`, `Rect::ZERO` without cells), kept with `hit`; empty when the rows
    /// changed since (`splice`) until the next hit pass.
    pub(crate) row_overflow: Vec<kurbo::Rect>,
    /// Cells damaged or appended since the paint tree last visited the table (all of them when
    /// `None`), and, when every cell then was a plain box in it (no stacking context, nothing
    /// hoisted out of its subtree), their paint order (`build_paint_tree_impl`).
    paint: Option<Vec<usize>>,
    pub(crate) plain_paint_order: Option<(i32, i32)>,
}

impl PlacedCells {
    fn placed(&mut self, index: usize, cells: usize) {
        for list in [&mut self.round, &mut self.transform] {
            if let Some(cells_placed) = list {
                cells_placed.push(index);
                if cells_placed.len() > cells {
                    *list = None;
                }
            }
        }
    }

    /// A cell whose subtree changed (`propagate_damage_flags`).
    pub(crate) fn damaged(&mut self, index: usize, cells: usize) {
        for list in [&mut self.transform, &mut self.paint] {
            if let Some(cells_damaged) = list {
                cells_damaged.push(index);
                if cells_damaged.len() > cells {
                    *list = None;
                }
            }
        }
    }

    /// The cells `removed` replaced by `inserted` others (`update_rows`): the indices after them move,
    /// and the new cells are to be transformed and visited in the paint tree.
    fn splice(&mut self, removed: Range<usize>, inserted: usize, cells: usize) {
        for list in [&mut self.round, &mut self.transform, &mut self.paint, &mut self.hit_again].into_iter().flatten() {
            splice_indices(list, removed.clone(), inserted);
        }
        if !self.overflow.is_empty() {
            self.overflow.splice(removed.clone(), std::iter::repeat_n(kurbo::Rect::ZERO, inserted));
        }
        if !self.hit.is_empty() {
            self.hit.splice(removed.clone(), std::iter::repeat_n(kurbo::Rect::ZERO, inserted));
        }
        self.row_overflow.clear();
        for index in removed.start..removed.start + inserted {
            self.damaged(index, cells);
        }
    }

    /// The cells to visit again in the paint tree (all of them: `None`), and the next ones.
    pub(crate) fn take_paint(&mut self) -> Option<Vec<usize>> {
        std::mem::replace(&mut self.paint, Some(Vec::new())).map(|mut cells| {
            cells.sort_unstable();
            cells.dedup();
            cells
        })
    }

    /// The cells to round again for a table whose position's fractional part is now `at` (all of
    /// them: `None`), and the next ones.
    pub(crate) fn take_round(&mut self, at: taffy::Point<f32>) -> Option<Vec<usize>> {
        let same_place = self.rounded_at == Some(at);
        self.rounded_at = Some(at);
        let cells = std::mem::replace(&mut self.round, Some(Vec::new()));
        cells.filter(|_| same_place).map(|mut cells| {
            cells.sort_unstable();
            cells.dedup();
            cells
        })
    }

    /// The cells to transform again (all of them: `None`), and the next ones.
    pub(crate) fn take_transform(&mut self) -> Option<Vec<usize>> {
        std::mem::replace(&mut self.transform, Some(Vec::new())).map(|mut cells| {
            cells.sort_unstable();
            cells.dedup();
            cells
        })
    }

    /// pyro: a transform pass visited these cells (sorted; all of them: `None`), whose
    /// hit contributions the next hit pass computes again.
    pub(crate) fn transformed(&mut self, again: Option<&[usize]>, cells: usize) {
        match (again, &mut self.hit_again) {
            (Some(again), Some(list)) => {
                list.extend_from_slice(again);
                if list.len() > cells {
                    list.sort_unstable();
                    list.dedup();
                    if list.len() > cells / 2 {
                        self.hit_again = None;
                    }
                }
            }
            (None, _) => self.hit_again = None,
            (Some(_), None) => {}
        }
    }

    /// pyro: the cells whose hit contributions to compute again (all of them: `None`),
    /// and the next ones.
    pub(crate) fn take_hit(&mut self) -> Option<Vec<usize>> {
        std::mem::replace(&mut self.hit_again, Some(Vec::new())).map(|mut cells| {
            cells.sort_unstable();
            cells.dedup();
            cells
        })
    }
}

/// Indices into a list whose items `removed` were replaced by `inserted` others: those of the removed
/// items are dropped, those after them moved.
fn splice_indices(list: &mut Vec<usize>, removed: Range<usize>, inserted: usize) {
    list.retain_mut(|index| {
        if *index < removed.start {
            true
        } else if *index < removed.end {
            false
        } else {
            *index = *index + inserted - removed.len();
            true
        }
    });
}

/// The widths of a table's cells (`TableContext::widths`), measured for a table content width.
#[derive(Debug, Default)]
pub(crate) struct CellWidths {
    /// The table content width the cells with percentage or `calc()` lengths
    /// (`TableCell::percent_lengths`) were measured for; the other cells' widths do not depend on it.
    inner: f32,
    cells: Vec<Option<(f32, f32)>>,
    /// The cells to measure again (they changed, or are new): their last widths, if any, are still
    /// in `cells`.
    pending: Vec<usize>,
    /// What the column widths are computed from: for each column, the largest widths of the cells
    /// spanning it alone, and the cells spanning several. `None`: to be summed again from `cells`
    /// (cells were removed or moved to other columns).
    columns: Option<ColumnWidths>,
}

#[derive(Debug)]
struct ColumnWidths {
    min: Vec<f32>,
    max: Vec<f32>,
    spanning: Vec<usize>,
}

impl CellWidths {
    /// A cell whose subtree changed (`propagate_damage_flags`), or that was collected again.
    pub(crate) fn changed(&mut self, index: usize) {
        if self.pending.len() >= self.cells.len() {
            // Never measured (yet), or more changes than cells: all of them again.
            *self = CellWidths::default();
            return;
        }
        self.pending.push(index);
    }

    /// Brings the column widths up to date for the cells of `ctx`, measuring those that changed
    /// with `measure`: a column's widths are summed again only when a cell that set them got
    /// narrower.
    fn update(
        &mut self,
        ctx: &TableContext,
        columns: usize,
        mut measure: impl FnMut(usize) -> (f32, f32),
    ) {
        let mut pending = std::mem::take(&mut self.pending);
        pending.sort_unstable();
        pending.dedup();
        let mut stale = false;
        for idx in pending {
            let Some(cell) = ctx.cells.get(idx) else { continue };
            let old = self.cells[idx];
            let new = measure(idx);
            self.cells[idx] = Some(new);
            let Some(widths) = self.columns.as_mut() else { continue };
            let column = cell.column as usize;
            if cell.colspan != 1 || column >= columns {
                stale |= !widths.spanning.contains(&idx);
                continue;
            }
            for (width, new, old) in [(&mut widths.min[column], new.0, old.map(|o| o.0)), (&mut widths.max[column], new.1, old.map(|o| o.1))] {
                if new >= *width {
                    *width = new;
                } else if old.is_some_and(|old| old >= *width) {
                    // It may have been the widest.
                    stale = true;
                }
            }
        }
        if stale {
            self.columns = None;
        }
        if self.columns.is_none() {
            let mut widths = ColumnWidths { min: vec![0.0; columns], max: vec![0.0; columns], spanning: Vec::new() };
            for (idx, cell) in ctx.cells.iter().enumerate() {
                let (lo, hi) = match self.cells[idx] {
                    Some(width) => width,
                    None => {
                        let width = measure(idx);
                        self.cells[idx] = Some(width);
                        width
                    }
                };
                let column = cell.column as usize;
                if cell.colspan == 1 && column < columns {
                    widths.min[column] = widths.min[column].max(lo);
                    widths.max[column] = widths.max[column].max(hi);
                } else {
                    widths.spanning.push(idx);
                }
            }
            self.columns = Some(widths);
        }
    }
}

// #[derive(Debug, Clone, Eq, PartialEq)]
// pub enum TableItemKind {
//     Row,
//     Cell,
// }

/// One segment of a collapsed border; width 0 when no border wins.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CollapsedEdge {
    pub width: f32,
    pub color: style::color::AbsoluteColor,
    rank: u8,
    priority: u8,
    hidden: bool,
}

impl CollapsedEdge {
    /// pyro: whether this edge beats `other` where collapsed borders meet (wider, or as wide with a
    /// style that wins, then the box it comes from: a cell's over a row's, ... a column group's,
    /// as along an edge): the winner of a junction paints it (`blitz-paint`'s `draw_collapsed_edges`;
    /// WPT css-tables `tentative/paint/collapsed-border-large-cell`).
    pub fn outranks(&self, other: &CollapsedEdge) -> bool {
        (self.width, self.rank, self.priority) > (other.width, other.rank, other.priority)
    }

    /// pyro: whether the winning border is `double` (painted as two lines).
    pub fn is_double(&self) -> bool {
        self.rank == 8
    }

    const NONE: Self = CollapsedEdge {
        width: 0.0,
        color: style::color::AbsoluteColor::TRANSPARENT_BLACK,
        rank: 0,
        priority: 0,
        hidden: false,
    };
}

/// The collapsed borders of a table's grid lines, in the order of `css-tables-3` conflict resolution.
///
/// pyro: the segments are indexed by the grid's rows and columns in their own order (row 0 is the
/// first row and column 0 the first column, whichever way the table's writing mode and direction
/// run them), and `sides` says which physical border of a box faces each side of its grid area.
/// So conflict resolution, the half borders of the cells and the table's own border are the same
/// in every writing mode; only the two ends translate: `offer_box` reads a box's physical borders
/// through `sides`, and `taffy_rect`/`physical_sides` hand the result back in Taffy's axes (the
/// grid's) and as physical sides (a cell's own style). Paint maps the grid's space to the table's
/// box with `grid_to_physical`.
#[derive(Debug, Clone)]
pub struct CollapsedEdges {
    pub rows: usize,
    pub columns: usize,
    /// The physical side (0 top, 1 right, 2 bottom, 3 left) of a box that faces the row-start,
    /// column-end, row-end and column-start side of its grid area.
    sides: [u8; 4],
    /// The table's writing mode (the one its grid runs in).
    wm: WritingMode,
    /// `columns + 1` column lines: the segment of each row on each (pyro: runs, `EdgeLine`).
    vertical: Vec<EdgeLine>,
    /// `rows + 1` row lines: the segment of each column on each.
    horizontal: Vec<EdgeLine>,
}

/// pyro: the segments of one grid line, as runs of equal segments (the ones no border reaches are
/// gaps), so that a table's collapsed borders cost its lines and runs, not rows x columns: one
/// `colspan="1000" rowspan="10000"` cell over empty rows allocated 20 million segments.
#[derive(Debug, Clone, Default)]
struct EdgeLine {
    /// Sorted, disjoint `(start, end, edge)`, no edge `CollapsedEdge::NONE`.
    runs: Vec<(u32, u32, CollapsedEdge)>,
}

impl EdgeLine {
    fn get(&self, at: usize) -> &CollapsedEdge {
        let at = at as u32;
        let i = self.runs.partition_point(|run| run.1 <= at);
        match self.runs.get(i) {
            Some(run) if run.0 <= at => &run.2,
            _ => &CollapsedEdge::NONE,
        }
    }

    /// Replaces each segment of `range` by `f` of it, in time proportional to the runs it meets.
    fn update(&mut self, range: Range<usize>, f: impl Fn(CollapsedEdge) -> CollapsedEdge) {
        if range.is_empty() {
            return;
        }
        let (a, b) = (range.start as u32, range.end as u32);
        let first = self.runs.partition_point(|run| run.1 <= a);
        let last = self.runs.partition_point(|run| run.0 < b);
        let mut pieces: Vec<(u32, u32, CollapsedEdge)> = Vec::new();
        let mut push = |pieces: &mut Vec<(u32, u32, CollapsedEdge)>, s: u32, e: u32, edge: CollapsedEdge| {
            if s >= e || edge == CollapsedEdge::NONE {
                return;
            }
            match pieces.last_mut() {
                Some(prev) if prev.1 == s && prev.2 == edge => prev.1 = e,
                _ => pieces.push((s, e, edge)),
            }
        };
        let mut at = a;
        for &(s, e, edge) in &self.runs[first..last] {
            // The part of the run before the range stays as it is.
            if s < a {
                push(&mut pieces, s, a, edge);
            }
            let (s2, e2) = (s.max(a), e.min(b));
            push(&mut pieces, at, s2, f(CollapsedEdge::NONE));
            push(&mut pieces, s2, e2, f(edge));
            at = e2;
            if e > b {
                push(&mut pieces, b, e, edge);
            }
        }
        push(&mut pieces, at, b, f(CollapsedEdge::NONE));
        let count = pieces.len();
        self.runs.splice(first..last, pieces);
        // Runs equal to their neighbours outside the range join them: only at the range's two
        // edges (joining over the whole line made per-segment offers quadratic).
        let join = |runs: &mut Vec<(u32, u32, CollapsedEdge)>, i: usize| {
            if i > 0 && i < runs.len() && runs[i - 1].1 == runs[i].0 && runs[i - 1].2 == runs[i].2 {
                runs[i - 1].1 = runs[i].1;
                runs.remove(i);
                true
            } else {
                false
            }
        };
        // (The end first: joining at the start shifts the end's index.)
        join(&mut self.runs, first + count);
        join(&mut self.runs, first);
    }

    /// Replaces the segments `range` by `len` without borders, moving those after.
    fn splice(&mut self, range: Range<usize>, len: usize) {
        self.update(range.clone(), |_| CollapsedEdge::NONE);
        let (b, shift) = (range.end as i64, len as i64 - range.len() as i64);
        for run in &mut self.runs {
            if i64::from(run.0) >= b {
                run.0 = (i64::from(run.0) + shift) as u32;
                run.1 = (i64::from(run.1) + shift) as u32;
            }
        }
    }

    fn edges(&self) -> impl Iterator<Item = &CollapsedEdge> {
        self.runs.iter().map(|run| &run.2)
    }

    /// Whether every one of the line's `len` segments is `width` wide.
    fn all_width(&self, len: usize, width: f32) -> bool {
        let covered: u32 = self.runs.iter().map(|run| run.1 - run.0).sum();
        (covered as usize >= len || width == 0.0) && self.runs.iter().all(|run| run.2.width == width)
    }
}

impl CollapsedEdges {
    pub fn vertical_at(&self, row: usize, line: usize) -> &CollapsedEdge {
        self.vertical[line].get(row)
    }
    pub fn horizontal_at(&self, line: usize, column: usize) -> &CollapsedEdge {
        self.horizontal[line].get(column)
    }

    /// pyro: every edge some border reaches (the segments without one are `CollapsedEdge::NONE`).
    fn edges(&self) -> impl Iterator<Item = &CollapsedEdge> {
        self.vertical.iter().chain(&self.horizontal).flat_map(EdgeLine::edges)
    }

    /// pyro: the runs of equal segments on row line `line`: `(first column, end column, edge)`,
    /// the columns between them without a border (`blitz-paint`'s `draw_collapsed_edges`).
    pub fn horizontal_runs(&self, line: usize) -> impl Iterator<Item = (usize, usize, &CollapsedEdge)> {
        self.horizontal[line].runs.iter().map(|run| (run.0 as usize, run.1 as usize, &run.2))
    }

    /// pyro: the runs of equal segments on column line `line`: `(first row, end row, edge)`.
    pub fn vertical_runs(&self, line: usize) -> impl Iterator<Item = (usize, usize, &CollapsedEdge)> {
        self.vertical[line].runs.iter().map(|run| (run.0 as usize, run.1 as usize, &run.2))
    }

    fn new(rows: usize, columns: usize, wm: WritingMode) -> Self {
        use stylo_taffy::WritingModeExt as _;
        // Taffy's top and bottom are the grid's row-start and row-end sides; its left is the
        // column-start side unless the columns run from its right (`direction`).
        let taffy = wm.logical_rect(taffy::Rect { top: 0u8, right: 1, bottom: 2, left: 3 });
        let (column_start, column_end) = match wm.direction() {
            taffy::Direction::Ltr => (taffy.left, taffy.right),
            taffy::Direction::Rtl => (taffy.right, taffy.left),
        };
        Self {
            rows,
            columns,
            sides: [taffy.top, column_end, taffy.bottom, column_start],
            wm,
            vertical: vec![EdgeLine::default(); columns + 1],
            horizontal: vec![EdgeLine::default(); rows + 1],
        }
    }

    /// The physical widths (top, right, bottom, left) of the widths on the row-start, column-end,
    /// row-end and column-start sides of a grid area.
    fn physical_sides(&self, grid: [f32; 4]) -> [f32; 4] {
        let mut physical = [0.0; 4];
        for (side, width) in self.sides.iter().zip(grid) {
            physical[*side as usize] = width;
        }
        physical
    }

    /// The same widths in Taffy's axes, as the grid and its items have their borders.
    fn taffy_rect(&self, grid: [f32; 4]) -> taffy::Rect<f32> {
        use stylo_taffy::WritingModeExt as _;
        let [top, right, bottom, left] = self.physical_sides(grid);
        self.wm.logical_rect(taffy::Rect { top, right, bottom, left })
    }

    /// Whether the grid's axes are not the physical ones (a table in a vertical writing mode).
    pub fn is_transposed(&self) -> bool {
        self.wm.is_vertical()
    }

    /// A rectangle of the grid's space (Taffy's axes: x along the rows, y across them, from the
    /// table's border box) in the physical space of the table's border box, `width` wide.
    pub fn grid_to_physical(&self, (x0, y0, x1, y1): (f64, f64, f64, f64), width: f64) -> (f64, f64, f64, f64) {
        if !self.wm.is_vertical() {
            (x0, y0, x1, y1)
        } else if self.wm.is_vertical_lr() {
            (y0, x0, y1, x1)
        } else {
            (width - y1, x0, width - y0, x1)
        }
    }

    /// Uniform: every vertical segment one width, every horizontal one width, one color.
    fn uniform(&self) -> Option<(f32, f32)> {
        if self.rows == 0 || self.columns == 0 {
            return None;
        }
        let (x, y) = (self.vertical_at(0, 0).width, self.horizontal_at(0, 0).width);
        let visible = self.edges().find(|e| e.width > 0.0)?;
        let same_color = self.edges().all(|e| e.width == 0.0 || e.color == visible.color);
        (same_color
            && self.vertical.iter().all(|line| line.all_width(self.rows, x))
            && self.horizontal.iter().all(|line| line.all_width(self.columns, y)))
        .then_some((x, y))
    }

    fn is_empty(&self) -> bool {
        self.edges().all(|e| e.width == 0.0)
    }
}

/// Conflict resolution (css-tables-3 §11): `hidden` wins, then the wider, then by style, then
/// the inner box; among equals the first offered (left and top) stays.
fn offer_edge(
    mut edge: CollapsedEdge,
    border: &Border,
    side: usize,
    current_color: &style::color::AbsoluteColor,
    priority: u8,
) -> CollapsedEdge {
    use style::values::computed::BorderStyle as B;
    let (width, border_style, color) = match side {
        0 => (border.border_top_width.0, border.border_top_style, &border.border_top_color),
        1 => (border.border_right_width.0, border.border_right_style, &border.border_right_color),
        2 => (border.border_bottom_width.0, border.border_bottom_style, &border.border_bottom_color),
        _ => (border.border_left_width.0, border.border_left_style, &border.border_left_color),
    };
    if edge.hidden || border_style == B::None {
        return edge;
    }
    if border_style == B::Hidden {
        edge.hidden = true;
        edge.width = 0.0;
        return edge;
    }
    let width = width.to_f32_px();
    if width == 0.0 {
        return edge;
    }
    let rank = match border_style {
        B::Double => 8,
        B::Solid => 7,
        B::Dashed => 6,
        B::Dotted => 5,
        B::Ridge => 4,
        B::Outset => 3,
        B::Groove => 2,
        _ => 1,
    };
    if (width, rank, priority) > (edge.width, edge.rank, edge.priority) {
        edge = CollapsedEdge { width, color: color.resolve_to_absolute(current_color), rank, priority, hidden: false };
    }
    edge
}

/// The grid area (rows, columns) each cell ends up in: the column the HTML table model gives it
/// (`ColumnCursor`, `TableCell::column`: the first slot of its row no cell covers), which is where
/// `place_cells` puts it, overlapping or not.
fn cell_areas(cells: &[TableCell], nrows: usize, ncols: usize) -> Vec<(usize, usize, usize, usize)> {
    cells
        .iter()
        .map(|cell| {
            let rowspan = match cell.style.grid_row.end {
                taffy::GridPlacement::Span(n) => n as usize,
                _ => 1,
            };
            let r0 = cell.row as usize;
            let r1 = (r0 + rowspan).min(nrows);
            let c0 = if r0 < nrows { (cell.column as usize).min(ncols) } else { ncols };
            let c1 = (c0 + cell.colspan as usize).min(ncols);
            (r0, c0, r1, c1)
        })
        .collect()
}

/// Resolves the collapsed borders of the table's cells (in `areas`), rows, row groups and the table.
fn resolve_collapsed_edges(
    doc: &BaseDocument,
    table: NodeId,
    cells: &[TableCell],
    areas: &[(usize, usize, usize, usize)],
    rows: &[TableRow],
    table_columns: &[TableColumn],
    columns: usize,
    wm: WritingMode,
) -> CollapsedEdges {
    let mut edges = CollapsedEdges::new(rows.len(), columns, wm);
    let all = EdgeRange { lines: 0..rows.len() + 1, rows: 0..rows.len() };
    for (cell, &(r0, c0, r1, c1)) in cells.iter().zip(areas) {
        if r0 < r1 && c0 < c1 {
            offer_box(&mut edges, doc, cell.node_id, (r0, c0, r1, c1), CELL_PRIORITY, &all);
        }
    }
    offer_rows_groups_and_table(&mut edges, doc, table, rows, table_columns, 0..rows.len(), &all);
    for &area in areas {
        clear_spanned(&mut edges, area, &all);
    }
    edges
}

/// pyro: the grid lines inside a cell that spans rows or columns have no collapsed border: the
/// cell covers them (Blink resolves borders only on the edges of cells), so the borders rows,
/// columns or groups gave them are not drawn there.
fn clear_spanned(edges: &mut CollapsedEdges, (r0, c0, r1, c1): (usize, usize, usize, usize), range: &EdgeRange) {
    let ncols = edges.columns;
    if r1 > edges.rows || c1 > ncols {
        return;
    }
    for line in ((r0 + 1)..r1).filter(|line| range.lines.contains(line)) {
        edges.horizontal[line].update(c0..c1, |_| CollapsedEdge::NONE);
    }
    let rows = r0.max(range.rows.start)..r1.min(range.rows.end);
    for line in (c0 + 1)..c1 {
        edges.vertical[line].update(rows.clone(), |_| CollapsedEdge::NONE);
    }
}

/// The order in which the borders of a table's boxes win among equals (css-tables-3 §11.3):
/// cell, row, row group, column, column group, table.
const CELL_PRIORITY: u8 = 5;
const ROW_PRIORITY: u8 = 4;
const ROW_GROUP_PRIORITY: u8 = 3;
const COLUMN_PRIORITY: u8 = 2;
const COLUMN_GROUP_PRIORITY: u8 = 1;

/// The segments a partial resolution resets and resolves again: the horizontal ones on `lines`
/// and the vertical ones of `rows`.
struct EdgeRange {
    lines: Range<usize>,
    rows: Range<usize>,
}

/// Offers the borders of the box `id` around its grid area to the segments of `range`.
fn offer_box(
    edges: &mut CollapsedEdges,
    doc: &BaseDocument,
    id: NodeId,
    (r0, c0, r1, c1): (usize, usize, usize, usize),
    priority: u8,
    range: &EdgeRange,
) {
    let Some(styles) = doc.nodes[id].primary_styles() else { return };
    let border = styles.get_border();
    // pyro: a box without borders offers none (most rows, groups and columns).
    {
        use style::values::computed::BorderStyle as B;
        if [border.border_top_style, border.border_right_style, border.border_bottom_style, border.border_left_style]
            .iter()
            .all(|side| *side == B::None)
        {
            return;
        }
    }
    let color = styles.clone_color();
    let [row_start, column_end, row_end, column_start] = edges.sides.map(usize::from);
    for (line, side) in [(r0, row_start), (r1, row_end)] {
        if range.lines.contains(&line) {
            edges.horizontal[line].update(c0..c1, |edge| offer_edge(edge, border, side, &color, priority));
        }
    }
    let rows = r0.max(range.rows.start)..r1.min(range.rows.end);
    edges.vertical[c0].update(rows.clone(), |edge| offer_edge(edge, border, column_start, &color, priority));
    edges.vertical[c1].update(rows, |edge| offer_edge(edge, border, column_end, &color, priority));
}

/// Offers the borders of `rows[span]`, of the row groups they are in (each whole) and of the table
/// to the segments of `range`, after the cells' (the order of conflict resolution).
fn offer_rows_groups_and_table(
    edges: &mut CollapsedEdges,
    doc: &BaseDocument,
    table: NodeId,
    rows: &[TableRow],
    table_columns: &[TableColumn],
    span: Range<usize>,
    range: &EdgeRange,
) {
    let (nrows, ncols) = (rows.len(), edges.columns);
    for r in span.clone() {
        offer_box(edges, doc, rows[r].node_id, (r, 0, r + 1, ncols), ROW_PRIORITY, range);
    }
    if span.is_empty() {
        return;
    }
    // The groups of `span`, from the start of the first one.
    let mut r = span.start;
    while r > 0 && rows[r - 1].group == rows[span.start].group {
        r -= 1;
    }
    while r < span.end {
        let group = rows[r].group;
        let end = rows[r..].iter().take_while(|row| row.group == group).count() + r;
        if let Some(group) = group {
            offer_box(edges, doc, group, (r, 0, end, ncols), ROW_GROUP_PRIORITY, range);
        }
        r = end;
    }
    // pyro: the columns and column groups, each over every row (`<col>` and `<colgroup>`
    // borders were not offered).
    if nrows > 0 {
        let mut c = 0;
        while c < table_columns.len().min(ncols) {
            let column = &table_columns[c];
            let end = c + table_columns[c..ncols.min(table_columns.len())]
                .iter()
                .take_while(|other| other.node_id == column.node_id)
                .count();
            offer_box(edges, doc, column.node_id, (0, c, nrows, end), COLUMN_PRIORITY, range);
            c = end;
        }
        let mut c = 0;
        while c < table_columns.len().min(ncols) {
            let group = table_columns[c].group;
            let end = c + table_columns[c..ncols.min(table_columns.len())]
                .iter()
                .take_while(|other| other.group == group)
                .count();
            if let Some(group) = group {
                offer_box(edges, doc, group, (0, c, nrows, end), COLUMN_GROUP_PRIORITY, range);
            }
            c = end;
        }
    }
    if nrows > 0 && ncols > 0 {
        offer_box(edges, doc, table, (0, 0, nrows, ncols), 0, range);
    }
}

/// The grid areas of a table's cells, in the order of `TableContext::cells`.
#[derive(Debug, Clone, PartialEq)]
pub struct CellPlacement {
    /// The number of columns (`width`) and rows (`height`) the cells reach
    pub extent: taffy::Size<u16>,
    /// Each cell's columns (`width`) and rows (`height`), as origin-zero grid lines
    pub areas: Vec<taffy::Size<taffy::Line<u16>>>,
}

/// The most grid lines `place_cells` places a cell up to: Taffy's limited grid, past which it clamps placements.
const PLACEMENT_LINE_LIMIT: u16 = 10_000;

/// Places the cells as the grid's `row dense` auto-placement does: each cell, in order, has the rows its
/// `grid-row` gives and the first columns, scanning from the first, that are free in all of those rows.
/// Taffy runs that same algorithm over an occupancy matrix on every layout of the table; this runs it once, as
/// the table is built, over each row's occupied column ranges ([`OccupiedColumns`]). `None` (the grid then
/// places the cells) for a cell whose `grid-row` is not a positive line and a span, whose `grid-column` is not
/// `auto` and a span, or whose area would reach past Taffy's limited grid.
fn place_cells(cells: &[TableCell]) -> Option<CellPlacement> {
    use taffy::GridPlacement::{Auto, Line, Span};
    let mut occupied = OccupiedColumns::default();
    let mut extent = taffy::Size { width: 0u16, height: 0u16 };
    let mut areas = Vec::with_capacity(cells.len());
    for cell in cells {
        let (Line(start), Span(rowspan)) = (&cell.style.grid_row.start, &cell.style.grid_row.end) else {
            return None;
        };
        let (Auto | Line(_), Span(colspan)) = (&cell.style.grid_column.start, &cell.style.grid_column.end) else {
            return None;
        };
        let (start, rowspan, colspan) = (start.as_i16(), *rowspan, *colspan);
        if start < 1 || rowspan == 0 || colspan == 0 {
            return None;
        }
        let r0 = (start - 1) as u16;
        let r1 = r0.checked_add(rowspan).filter(|end| *end <= PLACEMENT_LINE_LIMIT)?;
        // pyro: the HTML table model places a cell in the first slot of its row that no cell covers
        // (`ColumnCursor`, `TableCell::column`), whatever its span then overlaps (a table model error
        // browsers lay out overlapping; WPT css-tables `tentative/border-collapse-spanning-cells-004`).
        // Without overlaps that is where the grid's `row dense` placement puts it.
        let columns = if r0 == cell.row {
            let c1 = cell.column.checked_add(colspan).filter(|end| *end <= PLACEMENT_LINE_LIMIT)?;
            occupied.occupy(r0..r1, cell.column..c1);
            cell.column..c1
        } else {
            occupied.place(r0..r1, colspan)?
        };
        extent.width = extent.width.max(columns.end);
        extent.height = extent.height.max(r1);
        areas.push(taffy::Size {
            width: taffy::Line { start: columns.start, end: columns.end },
            height: taffy::Line { start: r0, end: r1 },
        });
    }
    Some(CellPlacement { extent, areas })
}

/// The grid area of a cell of a table without rowspans: its row and the columns `ColumnCursor` gave it,
/// which is where `place_cells` (the grid's `row dense` placement) puts it when no cell spans rows, as
/// the cells `update_rows` collects again never do. `None` as for `place_cells`.
fn unspanned_cell_area(cell: &TableCell) -> Option<taffy::Size<taffy::Line<u16>>> {
    use taffy::GridPlacement::{Auto, Line, Span};
    let (Line(start), Span(1)) = (&cell.style.grid_row.start, &cell.style.grid_row.end) else {
        return None;
    };
    let (Auto | Line(_), Span(colspan)) = (&cell.style.grid_column.start, &cell.style.grid_column.end) else {
        return None;
    };
    if start.as_i16() != cell.row as i16 + 1 || *colspan == 0 {
        return None;
    }
    let end = cell.column.checked_add(*colspan).filter(|end| *end <= PLACEMENT_LINE_LIMIT)?;
    Some(taffy::Size {
        width: taffy::Line { start: cell.column, end },
        height: taffy::Line { start: cell.row, end: cell.row + 1 },
    })
}

/// For each row, the columns its cells occupy: disjoint ranges in column order, adjacent ranges merged, so a
/// row of contiguous cells is one range and placing a cell jumps over each run of occupied columns at once.
#[derive(Debug, Default)]
struct OccupiedColumns {
    rows: Vec<Vec<Range<u16>>>,
}

impl OccupiedColumns {
    /// pyro: occupies `columns` in `rows`, whatever they overlap (`place_cells`).
    fn occupy(&mut self, rows: Range<u16>, columns: Range<u16>) {
        if self.rows.len() < rows.end as usize {
            self.rows.resize_with(rows.end as usize, Vec::new);
        }
        for row in &mut self.rows[rows.start as usize..rows.end as usize] {
            row.push(columns.clone());
            row.sort_by_key(|range| range.start);
            let mut merged: Vec<Range<u16>> = Vec::with_capacity(row.len());
            for range in row.drain(..) {
                match merged.last_mut() {
                    Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                    _ => merged.push(range),
                }
            }
            *row = merged;
        }
    }

    /// Occupies, in `rows`, the first `colspan` columns from the first that are free in all of them, and
    /// returns them; `None` past [`PLACEMENT_LINE_LIMIT`].
    fn place(&mut self, rows: Range<u16>, colspan: u16) -> Option<Range<u16>> {
        if self.rows.len() < rows.end as usize {
            self.rows.resize_with(rows.end as usize, Vec::new);
        }
        let rows = &mut self.rows[rows.start as usize..rows.end as usize];
        let mut c0 = 0u16;
        let c1 = 'fit: loop {
            let c1 = c0.checked_add(colspan).filter(|end| *end <= PLACEMENT_LINE_LIMIT)?;
            for row in rows.iter() {
                // The first range that ends after `c0`; it overlaps the columns if it starts before `c1`
                let next = row.partition_point(|range| range.end <= c0);
                if let Some(range) = row.get(next).filter(|range| range.start < c1) {
                    c0 = range.end;
                    continue 'fit;
                }
            }
            break c1;
        };
        for row in rows.iter_mut() {
            let at = row.partition_point(|range| range.end <= c0);
            let joins_previous = at > 0 && row[at - 1].end == c0;
            let joins_next = row.get(at).is_some_and(|range| range.start == c1);
            match (joins_previous, joins_next) {
                (true, true) => {
                    row[at - 1].end = row[at].end;
                    row.remove(at);
                }
                (true, false) => row[at - 1].end = c1,
                (false, true) => row[at].start = c0,
                (false, false) => row.insert(at, c0..c1),
            }
        }
        Some(c0..c1)
    }
}

#[derive(Debug, Clone)]
pub struct TableCell {
    // kind: TableItemKind,
    pub node_id: NodeId,
    /// Index (into `TableContext::rows`) of the row the cell originates in
    pub row: u16,
    /// Index of the column the cell originates in
    pub column: u16,
    colspan: u16,
    /// The `rowspan` attribute (before a rowspan is cut at the end of its row group).
    rowspan: u16,
    /// Whether its computed `width` is a percentage (`TableTreeWrapper::has_percent_cell`).
    percent_width: bool,
    /// pyro: whether its intrinsic widths depend on the table's content width: a
    /// percentage or `calc()` `width`, `min-width`, `max-width`, padding or margin (`CellWidths`).
    percent_lengths: bool,
    /// In the auto table layout algorithm, the larger of the cell's and its column's
    /// length `width`. That width is a minimum: the column still grows to fit the
    /// min-content width of its cells, but not to their max-content width. Both of the
    /// cell's intrinsic widths are therefore its min-content width, and no less than this.
    min_width: Option<f32>,
    /// A numeric author cell height is a minimum of its border box, not a
    /// definite content height. Collapsed border gutters are excluded here.
    min_height: Option<f32>,
    /// `height` as a border box before collapsed borders, and whether they add to it (`content-box`).
    height_base: Option<(f32, bool)>,
    /// pyro: the cell's own length `width` as a border box (auto tables), and `min_width`
    /// and `min_height` before the halves of collapsed borders are added: what a cell rebuilt on its
    /// own starts from (`update_table_context`).
    length_width: Option<f32>,
    min_width_base: Option<f32>,
    min_height_base: Option<f32>,
    /// pyro: aligned on its row's baseline (`vertical-align: baseline`, spanning one row, in a
    /// horizontal table): its content moves down inside it (`taffy`'s `table_baseline`).
    baseline: bool,
    style: taffy::Style<Atom>,
}

/// Tracks the current column position while walking the table's cells, so that
/// cells are assigned the same columns Taffy's dense auto-placement will give them
/// (skipping columns still occupied by rowspan cells from earlier rows).
#[derive(Debug, Default)]
struct ColumnCursor {
    /// The column the next cell in the current row will be placed in
    col: u16,
    /// The total number of columns seen so far
    num_columns: u16,
    /// For each column, the number of further rows it is occupied by a rowspan cell
    rowspans: Vec<u16>,
}

impl ColumnCursor {
    fn start_row(&mut self) {
        self.col = 0;
        for remaining in self.rowspans.iter_mut() {
            *remaining = remaining.saturating_sub(1);
        }
    }

    /// Advance past occupied columns, returning the column of the next cell
    fn next_free(&mut self) -> u16 {
        // pyro: never past the last column a cell can start in (`PLACEMENT_LINE_LIMIT`): cells
        // beyond it share that column.
        while self.col < PLACEMENT_LINE_LIMIT - 1 && self.rowspans.get(self.col as usize).is_some_and(|r| *r > 0) {
            self.col += 1;
        }
        self.col = self.col.min(PLACEMENT_LINE_LIMIT - 1);
        self.col
    }

    /// pyro: a row group starts or ends: no rowspan reaches past its group.
    fn end_group(&mut self) {
        self.rowspans.iter_mut().for_each(|remaining| *remaining = 0);
    }

    fn place(&mut self, colspan: u16, rowspan: u16) {
        let end = self.col.saturating_add(colspan).min(PLACEMENT_LINE_LIMIT).max(self.col);
        if self.rowspans.len() < end as usize {
            self.rowspans.resize(end as usize, 0);
        }
        // pyro: `start_row` counts a row down before the next row looks: the rows after this one
        // that the cell still occupies are `rowspan - 1`, so `rowspan` before that count (a cell
        // after a two-row cell went into its column).
        // pyro: a cell overlapping a slot a cell from an earlier row still covers (an HTML table
        // model error) leaves that cell its rows (`place_cells`).
        for remaining in &mut self.rowspans[self.col as usize..end as usize] {
            *remaining = (*remaining).max(rowspan);
        }
        self.col = end;
        self.num_columns = self.num_columns.max(end);
    }
}

#[derive(Debug, Clone)]
pub struct TableColumn {
    pub node_id: NodeId,
    /// The column group (if any) that contains the column
    pub group: Option<NodeId>,
}

#[derive(Debug, Clone)]
pub struct TableRow {
    // kind: TableItemKind,
    pub node_id: NodeId,
    /// The row group (if any) that contains the row
    pub group: Option<NodeId>,
    /// The row's specified `height`, if it is a length
    pub height: Option<f32>,
    /// Whether the row or any cell originating in it (and spanning only this row) has a
    /// specified length `height`. Such rows do not receive excess table height when there
    /// are unconstrained rows to receive it instead.
    pub is_constrained: bool,
    /// pyro: the number of cells originating in the row (they follow those of the rows
    /// before it in `TableContext::cells`), and the column after its last cell.
    pub cell_count: u32,
    pub columns: u16,
    /// pyro: `visibility: collapse` (the row's, or inherited from its group): the row takes no
    /// height and its cells paint nothing (CSS 2.1 §17.5.5).
    pub collapsed: bool,
}

/// The used width of one border side: border widths are not adjusted for
/// border-style in computed styles, so a border with `none`/`hidden` style
/// must be treated as zero-width.
/// The writing mode the table's grid runs in (the table's own)
#[cfg_attr(not(feature = "writing-mode"), allow(unused_variables))]
fn table_wm(doc: &BaseDocument, table_root_node_id: NodeId) -> WritingMode {
    #[cfg(feature = "writing-mode")]
    {
        if doc.try_root_element().map(|root| root.id) == Some(table_root_node_id) {
            doc.root_layout_wm(table_root_node_id)
        } else {
            doc.nodes[table_root_node_id].writing_mode()
        }
    }
    #[cfg(not(feature = "writing-mode"))]
    {
        WritingMode::empty()
    }
}

/// Convert a table part's style into the table's writing mode
///
/// pyro: the physical Taffy style is the one converted when the node's style was
/// written (`Node::table_part_taffy_style`); only a node that is not a table part by its own style
/// is converted here.
fn table_taffy_style(node: &crate::Node, table_wm: WritingMode) -> taffy::Style<Atom> {
    use stylo_taffy::WritingModeExt as _;
    match node.table_part_taffy_style() {
        Some(style) => {
            let mut style = style.clone();
            table_wm.transpose_style(&mut style);
            style
        }
        None => stylo_taffy::to_taffy_style_in(&node.primary_styles().unwrap(), table_wm),
    }
}

/// A table part's computed physical `width` and `height` (`None` without styles).
/// Whether a length resolves against the containing block's width: a percentage or a `calc()`
/// (a calc's `tag()` carries pointer bits: `uses_percentage` tests the bit both set).
fn resolves_against_parent_width(length: taffy::CompactLength) -> bool {
    length.uses_percentage()
}

fn physical_size(node: &crate::Node) -> Option<taffy::Size<taffy::Dimension>> {
    match node.table_part_taffy_style() {
        Some(style) => Some(style.size),
        None => node.primary_styles().map(|style| taffy::Size {
            width: stylo_taffy::convert::dimension(style.get_width()),
            height: stylo_taffy::convert::dimension(style.get_height()),
        }),
    }
}

fn side_width(width: app_units::Au, style: BorderStyle) -> f32 {
    if style.none_or_hidden() {
        0.0
    } else {
        width.to_f32_px()
    }
}

/// Build a `calc(<percent> + <length>)` track sizing function. The calc value is
/// boxed and stored in `calc_values` so that the raw pointer Taffy holds stays valid.
fn percent_plus_length(
    calc_values: &mut Vec<LengthPercentage>,
    percent: f32,
    length: f32,
) -> TrackSizingFunction {
    let node = CalcNode::Sum(
        vec![
            CalcNode::Leaf(ComputedLeaf::Percentage(CalcPercentageLeaf::new(
                percent,
                Optional::Some(NumericBaseType::Length),
            ))),
            CalcNode::Leaf(ComputedLeaf::Length(Length::new(length))),
        ]
        .into(),
    );
    let value = LengthPercentage::new_calc(node, AllowedNumericType::NonNegative);
    let dim = match value.unpack() {
        UnpackedLengthPercentage::Calc(calc) => {
            let ptr = calc as *const CalcLengthPercentage as *const ();
            // SAFETY: the pointer targets the heap allocation owned by `value`, which
            // is kept alive by `calc_values` for as long as the TableContext exists.
            unsafe { taffy::Dimension::from_raw(taffy::CompactLength::calc(ptr)) }
        }
        UnpackedLengthPercentage::Length(len) => style_helpers::length(len.px()),
        UnpackedLengthPercentage::Percentage(p) => style_helpers::percent(p.0),
    };
    calc_values.push(value);
    dim.into()
}

/// The table's row tracks. A row's `height` is a minimum: the row grows to fit the content of its
/// cells. Excess table height is distributed to unconstrained rows (via the stretching of `auto`
/// tracks) if there are any, and to all rows otherwise. So constrained rows use a `max-content`
/// (rather than `auto`) maximum when there are unconstrained rows.
///
/// pyro: `gutter` is the height of the collapsed border between two rows where the table lays
/// its collapsed borders out as gaps. A row's `height` covers half of the border above it and
/// half of the one below, as a cell's does, so its track is that much less.
fn row_tracks(rows: &[TableRow], gutter: f32) -> Vec<taffy::GridTemplateComponent<Atom>> {
    let has_unconstrained_rows = rows.iter().any(|row| !row.is_constrained);
    rows.iter()
        .map(|row| {
            // pyro: a collapsed row is sized as the others, then collapsed (taffy
            // `Style::collapsed_rows`): a cell spanning it gives it its share.
            let min = match row.height {
                Some(height) => style_helpers::length((height - gutter).max(0.0)),
                // Table rows retain their cells' intrinsic minimum even when
                // a cell clips overflow. Grid's `auto` minimum would use zero
                // for that cell, allowing a column-flex ancestor to compress
                // the row below its content and authored cell-height minimum.
                None => style_helpers::min_content(),
            };
            let max = if row.is_constrained && has_unconstrained_rows {
                style_helpers::max_content()
            } else {
                style_helpers::auto()
            };
            let track: TrackSizingFunction = style_helpers::minmax(min, max);
            track.into()
        })
        .collect()
}

/// pyro: what building a table gives: its context, its layout children (cells, out-of-flow boxes
/// and captions) and the anonymous rows and cells it made (`table_fixup`), which it owns.
pub(crate) struct BuiltTable {
    pub(crate) context: TableContext,
    pub(crate) layout_children: Vec<NodeId>,
    pub(crate) anonymous: Vec<NodeId>,
}

pub(crate) fn build_table_context(doc: &mut BaseDocument, table_root_node_id: NodeId) -> BuiltTable {
    let mut cells: Vec<TableCell> = Vec::new();
    let mut oof_children: Vec<(NodeId, taffy::Position)> = Vec::new();
    let mut rows: Vec<TableRow> = Vec::new();
    let mut row = 0u16;
    let mut cursor = ColumnCursor::default();

    let wm = table_wm(doc, table_root_node_id);
    // pyro: the cells of the table built last time, released below if they are no longer.
    let former = former_table_context(doc, table_root_node_id);

    // pyro: the table's children with its pseudo-elements, `display: contents` boxes looked through,
    // and what is not a proper table child in anonymous rows (CSS 2.1 §17.2.1).
    let mut anonymous: Vec<NodeId> = Vec::new();
    let children =
        super::table_fixup::fixup_children(doc, table_root_node_id, Level::Table, wm.is_vertical(), &mut anonymous)
            .unwrap_or_else(|| doc.nodes[table_root_node_id].children.to_vec());
    // (Laid out in the table's axes, on its block-start and block-end sides: `lay_out_table_captions`.)
    let captions: Vec<NodeId> = children
        .iter()
        .copied()
        .filter(|&id| super::table_fixup::kind(doc, id) == Kind::Caption)
        .collect();
    let root_node = &mut doc.nodes[table_root_node_id];

    let Some(stylo_styles) = root_node.primary_styles() else {
        panic!("Ignoring table because it has no styles");
    };

    let mut style = table_taffy_style(root_node, wm);
    style.item_is_table = true;
    // Use `dense` row-flow so that each cell scans the row from its
    // leftmost column for the first free track. Without `dense`,
    // `place_definite_secondary_axis_item` keeps a per-item secondary
    // cursor across rows, which means cells in later rows do not
    // backfill columns freed up by rowspan cells from earlier rows.
    style.grid_auto_flow = taffy::GridAutoFlow::RowDense;
    style.grid_auto_columns = Vec::new();
    style.grid_auto_rows = Vec::new();

    // The fixed table layout algorithm only applies when the table has a non-auto width
    let is_fixed = match stylo_styles.clone_table_layout() {
        TableLayout::Fixed => !style.size.width.is_auto(),
        TableLayout::Auto => false,
    };

    let border_collapse = stylo_styles.clone_border_collapse();
    let border_spacing = stylo_styles.clone_border_spacing().0;

    drop(stylo_styles);

    let mut columns: Vec<TableColumn> = Vec::new();
    let mut column_sizes: Vec<taffy::TrackSizingFunction> = Vec::new();
    for child_id in children.iter().copied() {
        collect_columns(doc, child_id, wm, &mut columns, &mut column_sizes);
    }
    // In the automatic table layout algorithm percentage column widths are not grid tracks
    // (`TableTreeWrapper::percent_column_widths`).
    let mut column_percents: Vec<Option<f32>> = vec![None; column_sizes.len()];
    if !is_fixed {
        for (column, percent) in column_sizes.iter_mut().zip(column_percents.iter_mut()) {
            if column.max.into_raw().tag() == taffy::CompactLength::PERCENT_TAG {
                *percent = Some(column.max.into_raw().value());
                *column = style_helpers::auto();
            }
        }
    }
    // pyro: the columns a `<col>` gives a width (a first-row cell's width gives the others theirs).
    let col_widths: Vec<bool> = column_sizes.iter().map(|column| !column.max.is_auto()).collect();
    let mut first_cell_border: Option<ServoArc<Border>> = None;
    // Percentage widths set on first-row cells: (column index, percentage, padding + border)
    let mut percent_columns: Vec<(u16, f32, f32)> = Vec::new();
    let mut calc_values: Vec<LengthPercentage> = Vec::new();
    // Header row groups are laid out before other rows, and footer row groups after.
    // pyro: only the first of each (CSS 2.1 § 17.2: the others are treated as row groups, as
    // browsers do; WPT css-tables `row-group-order`).
    let group_kind = |doc: &BaseDocument, child_id: NodeId| {
        doc.nodes[child_id].primary_styles().map(|s| s.clone_display().inside())
    };
    let first_of = |kind: DisplayInside| {
        children
            .iter()
            .copied()
            .find(|&id| !captions.contains(&id) && group_kind(doc, id) == Some(kind))
    };
    let (first_header, first_footer) = (first_of(DisplayInside::TableHeaderGroup), first_of(DisplayInside::TableFooterGroup));
    let row_group_order = |_doc: &BaseDocument, child_id: NodeId| -> u8 {
        if Some(child_id) == first_header {
            0
        } else if Some(child_id) == first_footer {
            2
        } else {
            1
        }
    };
    for order in 0..3 {
        for child_id in children.iter().copied() {
            if row_group_order(doc, child_id) != order || captions.contains(&child_id) {
                continue;
            }
            collect_table_cells(
                doc,
                child_id,
                wm,
                is_fixed,
                border_collapse,
                &mut row,
                &mut cursor,
                &mut cells,
                &mut oof_children,
                &mut rows,
                &mut column_sizes,
                &mut first_cell_border,
                &mut percent_columns,
                &mut anonymous,
            );
        }
    }
    // pyro: a rowspan stops at the end of its row group (the HTML table model;
    // Chromium's effective rowspan), rather than adding implicit rows past it.
    for cell in cells.iter_mut() {
        let start = cell.row as usize;
        let Some(group) = rows.get(start).map(|row| row.group) else { continue };
        let group_rows = rows[start..].iter().take_while(|row| row.group == group).count() as u16;
        if let taffy::GridPlacement::Span(span) = cell.style.grid_row.end {
            if span > group_rows {
                cell.style.grid_row.end = style_helpers::span(group_rows.max(1));
            }
        }
    }
    let remaining_column = if is_fixed {
        style_helpers::minmax(style_helpers::length(0.0), style_helpers::fr(1.0))
    } else {
        style_helpers::auto()
    };
    // Trailing `<col>` columns which contain no cells and have no definite width
    // (i.e. would be zero-width) are dropped, so they don't add border-spacing.
    // pyro: those with a width too, past the last cell (Chromium; WPT css-tables
    // `table_grid_size_col_colspan`: the grid is no wider than its cells).
    while column_sizes.len() > cursor.num_columns as usize && cursor.num_columns > 0 {
        column_sizes.pop();
        columns.pop();
    }
    let num_columns = cursor.num_columns.max(column_sizes.len() as u16);
    column_sizes.resize(num_columns as usize, remaining_column);
    // pyro: the collapsed borders, resolved before the columns get their widths: in the fixed
    // table layout algorithm a first-row cell's `width` gives its column a border box with half
    // of the collapsed borders beside it (CSS 2.1 §17.6.2, as Chromium).
    let first_gap = first_cell_border.as_ref().map_or((0.0, 0.0), |border| {
        (
            side_width(border.border_left_width.0, border.border_left_style)
                .max(side_width(border.border_right_width.0, border.border_right_style)),
            side_width(border.border_top_width.0, border.border_top_style)
                .max(side_width(border.border_bottom_width.0, border.border_bottom_style)),
        )
    });
    let mut early_areas = None;
    let early_edges = (border_collapse == BorderCollapse::Collapse).then(|| {
        let areas = early_areas.insert(cell_areas(&cells, rows.len(), num_columns as usize));
        resolve_collapsed_edges(doc, table_root_node_id, &cells, areas, &rows, &columns, num_columns as usize, wm)
    });
    if is_fixed {
        if let Some(edges) = early_edges.as_ref().filter(|edges| edges_in_use(edges, first_gap)) {
            for cell in cells.iter().filter(|cell| cell.row == 0 && cell.colspan == 1) {
                let column = cell.column as usize;
                if col_widths.get(column).copied().unwrap_or(false) || column >= column_sizes.len() {
                    continue;
                }
                let track = column_sizes[column].max.into_raw();
                if track.tag() != taffy::CompactLength::LENGTH_TAG
                    || cell.style.box_sizing != taffy::BoxSizing::ContentBox
                    || edges.rows == 0
                {
                    continue;
                }
                let half = (edges.vertical_at(0, column).width + edges.vertical_at(0, column + 1).width) / 2.0;
                column_sizes[column] = style_helpers::length(track.value() + half);
            }
            // pyro: so does a first-row cell's percentage `width` (with its padding, `percent_columns`).
            if edges.rows > 0 {
                for (column, _, extra) in percent_columns.iter_mut() {
                    let column = *column as usize;
                    let content_box = cells.iter().any(|cell| {
                        cell.row == 0
                            && cell.colspan == 1
                            && cell.column as usize == column
                            && cell.style.box_sizing == taffy::BoxSizing::ContentBox
                    });
                    if content_box && column < edges.columns {
                        *extra += (edges.vertical_at(0, column).width + edges.vertical_at(0, column + 1).width) / 2.0;
                    }
                }
            }
        }
    }
    let mut single_cell_columns = vec![0u32; num_columns as usize];
    for cell in cells.iter().filter(|cell| cell.colspan == 1) {
        single_cell_columns[cell.column as usize] += 1;
    }
    let has_rowspan = cells.iter().any(|cell| cell.rowspan > 1);
    if is_fixed {
        // In the fixed table layout algorithm, percentage column widths resolve against
        // the table's content width minus the horizontal border-spacing between columns,
        // whereas Taffy resolves percentage tracks against the container's inner width
        // (from which only the outer border-spacing has been removed via padding). Fold
        // the inner border-spacing into the percentage using `calc(N% - N * spacing)`.
        let spacing_x = match border_collapse {
            BorderCollapse::Separate => border_spacing.width.px(),
            BorderCollapse::Collapse => 0.0,
        };
        let inner_spacing = spacing_x * num_columns.saturating_sub(1) as f32;
        let mut percent_track = |percent: f32, extra: f32| -> TrackSizingFunction {
            let extra = extra - percent * inner_spacing;
            if extra == 0.0 {
                style_helpers::percent(percent)
            } else {
                percent_plus_length(&mut calc_values, percent, extra)
            }
        };

        for column in column_sizes.iter_mut() {
            if column.max.is_auto() {
                *column = remaining_column;
            } else if column.max.into_raw().tag() == taffy::CompactLength::PERCENT_TAG {
                *column = percent_track(column.max.into_raw().value(), 0.0);
            }
        }

        // Percentage widths on cells apply to the cell's content box, so the
        // cell's horizontal padding and border must be added to the column width.
        for (col_idx, percent, extra) in percent_columns {
            if let Some(column) = column_sizes.get_mut(col_idx as usize) {
                *column = percent_track(percent, extra);
            }
        }

        // When every column has a length width, a table wider than their sum
        // widens them in proportion to their widths: a track flexing by its own
        // width (`minmax(W, W fr)`) never goes below W and shares the free space
        // in that proportion.
        let lengths: Option<Vec<f32>> = column_sizes
            .iter()
            .map(|c| (c.max.into_raw().tag() == taffy::CompactLength::LENGTH_TAG).then(|| c.max.into_raw().value()))
            .collect();
        if let Some(lengths) = lengths.filter(|l| l.iter().any(|w| *w > 0.0)) {
            for (column, width) in column_sizes.iter_mut().zip(lengths) {
                *column = style_helpers::minmax(style_helpers::length(width), style_helpers::fr(width));
            }
        }
    } else {
        // In the auto table layout algorithm a column's length width is a minimum: the
        // column still grows to fit the min-content width of the cells that originate in
        // it (see `TableCell::min_width`). Those cells report that width as both of their
        // intrinsic widths, so a `fit-content()` track capped at the column's width grows
        // to fit them, but not to fit cells spanning it, which widen the other columns
        // rather than this one. A column without such cells keeps its fixed track.
        let mut minimum_widths = vec![None; column_sizes.len()];
        for cell in cells.iter_mut().filter(|cell| cell.colspan == 1) {
            let column = cell.column as usize;
            let Some(width) = column_sizes[column]
                .max
                .definite_value(None, resolve_calc_value)
            else {
                continue;
            };
            cell.min_width = Some(cell.min_width.map_or(width, |w| w.max(width)));
            cell.min_width_base = cell.min_width;
            minimum_widths[column] = Some(width);
        }
        for (column, width) in column_sizes.iter_mut().zip(minimum_widths) {
            if let Some(width) = width {
                *column = taffy::MinMax {
                    min: taffy::MinTrackSizingFunction::min_content(),
                    max: taffy::MaxTrackSizingFunction::fit_content_px(width),
                };
            }
        }
    }

    style.grid_template_columns = column_sizes.into_iter().map(|dim| dim.into()).collect();

    style.gap = match border_collapse {
        BorderCollapse::Separate => {
            // In the separated borders model, `border-spacing` also applies between
            // the table border and the outermost cells, in addition to between cells.
            // pyro: none in a table without rows (a caption alone), as Chromium.
            let (spacing_x, spacing_y) =
                if rows.is_empty() { (0.0, 0.0) } else { (border_spacing.width.px(), border_spacing.height.px()) };
            let padding = style.padding.resolve_or_zero(None, resolve_calc_value);
            style.padding = taffy::Rect {
                left: style_helpers::length(padding.left + spacing_x),
                right: style_helpers::length(padding.right + spacing_x),
                top: style_helpers::length(padding.top + spacing_y),
                bottom: style_helpers::length(padding.bottom + spacing_y),
            };
            // pyro: a content-box table's lengths include the spacing around its cells (its content
            // box is its grid's), laid out here as padding: they are that much less.
            if style.box_sizing == taffy::BoxSizing::ContentBox {
                fn less(raw: taffy::CompactLength, by: f32) -> Option<f32> {
                    (by != 0.0 && raw.tag() == taffy::CompactLength::LENGTH_TAG).then(|| (raw.value() - by).max(0.0))
                }
                let (x, y) = (2.0 * spacing_x, 2.0 * spacing_y);
                if let Some(v) = less(style.size.width.into_raw(), x) {
                    style.size.width = style_helpers::length(v);
                }
                if let Some(v) = less(style.min_size.width.into_raw(), x) {
                    style.min_size.width = style_helpers::length(v);
                }
                if let Some(v) = less(style.max_size.width.into_raw(), x) {
                    style.max_size.width = style_helpers::length(v);
                }
                if let Some(v) = less(style.size.height.into_raw(), y) {
                    style.size.height = style_helpers::length(v);
                }
                if let Some(v) = less(style.min_size.height.into_raw(), y) {
                    style.min_size.height = style_helpers::length(v);
                }
                if let Some(v) = less(style.max_size.height.into_raw(), y) {
                    style.max_size.height = style_helpers::length(v);
                }
            }
            taffy::Size {
                width: style_helpers::length(spacing_x),
                height: style_helpers::length(spacing_y),
            }
        }
        BorderCollapse::Collapse => first_cell_border
            .as_ref()
            .map(|border| {
                let x = side_width(border.border_left_width.0, border.border_left_style).max(
                    side_width(border.border_right_width.0, border.border_right_style),
                );
                let y = side_width(border.border_top_width.0, border.border_top_style).max(
                    side_width(border.border_bottom_width.0, border.border_bottom_style),
                );
                taffy::Size {
                    width: style_helpers::length(x),
                    height: style_helpers::length(y),
                }
            })
            .unwrap_or(taffy::Size::ZERO.map(style_helpers::length)),
    };

    if border_collapse == BorderCollapse::Collapse {
        style.border = taffy::Rect {
            left: style.gap.width,
            right: style.gap.width,
            top: style.gap.height,
            bottom: style.gap.height,
        };
        // pyro: a table in the collapsing border model has no padding (CSS 2.1 §17.6.2).
        style.padding = taffy::Rect::ZERO.map(style_helpers::length);
    }

    // pyro: a table whose collapsed borders are not one uniform grid has each cell keep half
    // of the borders around it, and the table half of its outer ones, as Chromium's boxes do.
    let collapse_gap = (style.gap.width.into_raw().value(), style.gap.height.into_raw().value());
    let ncols = style.grid_template_columns.len();
    debug_assert!(early_edges.as_ref().is_none_or(|edges| edges.columns == ncols));
    let edges = early_edges;
    let (collapsed_edges, spare_edges) = match edges {
        Some(edges) if edges_in_use(&edges, collapse_gap) => (Some(edges), None),
        edges => (None, edges),
    };
    let gutter = match (border_collapse, &collapsed_edges) {
        (BorderCollapse::Collapse, None) => collapse_gap.1,
        _ => 0.0,
    };
    style.grid_template_rows = row_tracks(&rows, gutter);
    // pyro: a collapsed row takes its border spacing with it (taffy `Style::collapsed_rows`).
    style.collapsed_rows = rows.iter().enumerate().filter(|(_, row)| row.collapsed).map(|(i, _)| i as u16).collect();
    style.collapsed_columns = collapsed_columns(doc, &columns);
    // pyro: a `<col>`'s width has the halves of the collapsed borders beside its column in it (CSS 2.1
    // § 17.6.2, as Chromium): in uniform gutters, which hold the borders whole, the track is that much
    // narrower (WPT css-tables `table_grid_size_col_colspan`).
    if border_collapse == BorderCollapse::Collapse && collapsed_edges.is_none() && collapse_gap.0 > 0.0 {
        let gap = collapse_gap.0;
        for (column, track) in style.grid_template_columns.iter_mut().enumerate() {
            if !col_widths.get(column).copied().unwrap_or(false) {
                continue;
            }
            if let taffy::GridTemplateComponent::Single(track) = track {
                let raw = track.max.into_raw();
                match raw.tag() {
                    taffy::CompactLength::FIT_CONTENT_PX_TAG => {
                        track.max = taffy::MaxTrackSizingFunction::fit_content_px((raw.value() - gap).max(0.0));
                    }
                    taffy::CompactLength::LENGTH_TAG => {
                        let width = (raw.value() - gap).max(0.0);
                        *track = style_helpers::length(width);
                    }
                    _ => {}
                }
            }
        }
    }
    if let Some(edges) = &collapsed_edges {
        let zero = style_helpers::length(0.0);
        style.gap = taffy::Size { width: zero, height: zero };
        style.border = collapsed_table_border(edges);
        // (The cells' areas the edges were resolved over.)
        let areas = early_areas.unwrap_or_else(|| cell_areas(&cells, edges.rows, edges.columns));
        for (cell, area) in cells.iter_mut().zip(areas) {
            apply_collapsed_borders(doc, cell, area, edges);
        }
    } else {
        for cell in cells.iter() {
            doc.nodes[cell.node_id].set_collapsed_border(None);
        }
        // pyro: laid out as gutters, a uniform collapsed table's outer borders are its own border
        // whole, where its box (as Chromium's) holds half of each: a content-box table's lengths,
        // which count the half, are a half border less on each side.
        if border_collapse == BorderCollapse::Collapse && style.box_sizing == taffy::BoxSizing::ContentBox {
            fn less(raw: taffy::CompactLength, by: f32) -> Option<f32> {
                (by != 0.0 && raw.tag() == taffy::CompactLength::LENGTH_TAG).then(|| (raw.value() - by).max(0.0))
            }
            let (x, y) = collapse_gap;
            if let Some(v) = less(style.size.width.into_raw(), x) {
                style.size.width = style_helpers::length(v);
            }
            if let Some(v) = less(style.min_size.width.into_raw(), x) {
                style.min_size.width = style_helpers::length(v);
            }
            if let Some(v) = less(style.max_size.width.into_raw(), x) {
                style.max_size.width = style_helpers::length(v);
            }
            if let Some(v) = less(style.size.height.into_raw(), y) {
                style.size.height = style_helpers::length(v);
            }
            if let Some(v) = less(style.min_size.height.into_raw(), y) {
                style.min_size.height = style_helpers::length(v);
            }
            if let Some(v) = less(style.max_size.height.into_raw(), y) {
                style.max_size.height = style_helpers::length(v);
            }
        }
    }

    // pyro: a fixed table is never narrower than its columns' widths and the spacing between
    // them (CSS 2.1 §17.5.2.1: the greater of its `width` and that sum).
    if is_fixed && style.min_size.width.into_raw().tag() != taffy::CompactLength::PERCENT_TAG {
        let n = style.grid_template_columns.len();
        let columns: f32 = style
            .grid_template_columns
            .iter()
            .map(|column| match column {
                taffy::GridTemplateComponent::Single(track)
                    if track.min.into_raw().tag() == taffy::CompactLength::LENGTH_TAG =>
                {
                    track.min.into_raw().value()
                }
                _ => 0.0,
            })
            .sum();
        let gap = style.gap.width.resolve_or_zero(None, resolve_calc_value) * n.saturating_sub(1) as f32;
        let floor = match style.box_sizing {
            taffy::BoxSizing::ContentBox => columns + gap,
            taffy::BoxSizing::BorderBox => {
                let padding = style.padding.resolve_or_zero(None, resolve_calc_value);
                let border = style.border.resolve_or_zero(None, resolve_calc_value);
                columns + gap + padding.left + padding.right + border.left + border.right
            }
        };
        let current = match style.min_size.width.into_raw() {
            raw if raw.tag() == taffy::CompactLength::LENGTH_TAG => raw.value(),
            _ => 0.0,
        };
        if floor > current {
            style.min_size.width = style_helpers::length(floor);
        }
    }
    let placement = place_cells(&cells);
    let layout_children = cells
        .iter()
        .map(|cell| cell.node_id)
        .chain(oof_children.iter().map(|(node_id, _)| *node_id))
        .chain(captions.iter().copied())
        .collect();
    crate::resolve::phase_profile::table_built();
    // pyro: a table with anonymous boxes, generated cells or rows (pseudo-elements), captions, or
    // that is anonymous itself is built again on any change: `update_rows` collects rows from
    // their DOM children alone.
    // (Rows and cells reached through `display: contents` boxes are in its box tree, not its DOM
    // children's: such a table too.)
    let through_contents = rows.iter().any(|row| {
        let parent = doc.nodes[row.node_id].parent;
        parent != Some(table_root_node_id) && parent != row.group
    }) || cells.iter().any(|cell| {
        rows.get(cell.row as usize).is_some_and(|row| doc.nodes[cell.node_id].parent != Some(row.node_id))
    });
    let generated = doc.nodes[table_root_node_id].is_anonymous()
        || !anonymous.is_empty()
        || !captions.is_empty()
        || through_contents
        || cells.iter().any(|cell| doc.nodes[cell.node_id].is_anonymous())
        || rows.iter().any(|row| doc.nodes[row.node_id].is_anonymous());
    let incremental =
        !generated && !has_rowspan && oof_children.is_empty() && cells_follow_rows(&rows, &cells, 0);
    let cells_percent = cells.iter().filter(|cell| cell.percent_width).count();
    let baseline_cells = cells.iter().any(|cell| cell.baseline);
    for (at, cell) in cells.iter().enumerate() {
        doc.nodes[cell.node_id].layout_data_mut().table_cell_index = at as u32;
    }
    if let Some(former) = former {
        release_former_cells(doc, table_root_node_id, &former.cells, &cells);
    }
    let mut group_borders: Vec<(NodeId, ServoArc<Border>)> = Vec::new();
    if border_collapse == BorderCollapse::Collapse {
        for group in rows.iter().filter_map(|row| row.group) {
            if group_borders.last().is_some_and(|(last, _)| *last == group) {
                continue;
            }
            if let Some(styles) = doc.nodes[group].primary_styles() {
                group_borders.push((group, styles.clone_border()));
            }
        }
    }

    let context = TableContext {
        style,
        cells,
        oof_children,
        captions,
        grid_box: AtomicRefCell::new(None),
        rows,
        columns,
        column_percents,
        computed_grid_info: AtomicRefCell::new(None),
        border_collapse,
        border_style: first_cell_border,
        collapsed_edges,
        group_borders,
        spare_edges,
        collapse_gap,
        is_fixed,
        incremental,
        cursor_columns: cursor.num_columns,
        single_cell_columns,
        percent_cells: cells_percent,
        changed_cells: AtomicRefCell::new(Vec::new()),
        grid_state: AtomicRefCell::new(None),
        grid_splice: AtomicRefCell::new(None),
        widths: AtomicRefCell::new(CellWidths::default()),
        placed: AtomicRefCell::new(PlacedCells::default()),
        placement,
        baseline_cells,
        calc_values,
    };
    BuiltTable { context, layout_children, anonymous }
}

/// Whether a collapsed table's borders are cells' and the table's own (not one uniform grid of
/// gutters as wide as the first cell's borders).
///
/// pyro: always in a vertical writing mode (the gutters are the first cell's physical borders, and
/// are painted as such).
fn edges_in_use(edges: &CollapsedEdges, gap: (f32, f32)) -> bool {
    if edges.is_transposed() {
        return !edges.is_empty() || gap != (0.0, 0.0);
    }
    !edges.is_empty() && edges.uniform() != Some(gap) || (edges.is_empty() && gap != (0.0, 0.0))
}

/// The table's border: half of the widest collapsed border on each of its sides.
fn collapsed_table_border(edges: &CollapsedEdges) -> taffy::Rect<taffy::LengthPercentage> {
    let half_max = |it: &mut dyn Iterator<Item = f32>| it.fold(0.0f32, f32::max) / 2.0;
    let (r, c) = (edges.rows, edges.columns);
    edges
        .taffy_rect([
            half_max(&mut (0..c).map(|i| edges.horizontal_at(0, i).width)),
            half_max(&mut (0..r).map(|i| edges.vertical_at(i, c).width)),
            half_max(&mut (0..c).map(|i| edges.horizontal_at(r, i).width)),
            half_max(&mut (0..r).map(|i| edges.vertical_at(i, 0).width)),
        ])
        .map(style_helpers::length)
}

/// Gives `cell` (in grid `area`) half of the collapsed borders around it, adding them to its
/// minimum sizes. Returns whether that changed the cell's box.
fn apply_collapsed_borders(
    doc: &mut BaseDocument,
    cell: &mut TableCell,
    (r0, c0, r1, c1): (usize, usize, usize, usize),
    edges: &CollapsedEdges,
) -> bool {
    if r0 >= r1 || c0 >= c1 {
        // pyro: a cell outside the grid's columns has no collapsed borders, as in a
        // fresh build (not those it had where it was before).
        doc.nodes[cell.node_id].set_collapsed_border(None);
        return false;
    }
    // (On the row-start, column-end, row-end and column-start sides of the area.)
    let grid = [
        (c0..c1).map(|i| edges.horizontal_at(r0, i).width).fold(0.0, f32::max) / 2.0,
        (r0..r1).map(|i| edges.vertical_at(i, c1).width).fold(0.0, f32::max) / 2.0,
        (c0..c1).map(|i| edges.horizontal_at(r1, i).width).fold(0.0, f32::max) / 2.0,
        (r0..r1).map(|i| edges.vertical_at(i, c0).width).fold(0.0, f32::max) / 2.0,
    ];
    // In Taffy's axes, as the cell's sizes are.
    let taffy::Rect { top, right, bottom, left } = edges.taffy_rect(grid);
    let border = taffy::Rect {
        left: style_helpers::length(left),
        right: style_helpers::length(right),
        top: style_helpers::length(top),
        bottom: style_helpers::length(bottom),
    };
    let min_height = match cell.height_base {
        Some((height, content_box)) => Some(if content_box { height + top + bottom } else { height }),
        None => cell.min_height_base,
    };
    // pyro: a column's width (`<col>`'s, in `min_width_base` when larger than the cell's own) has
    // the halves of the collapsed borders in it, the cell's own `width` does not (CSS 2.1 §17.6.2,
    // as Chromium), but for a `border-box` cell, whose `width` is its border box with them.
    let content_box = cell.style.box_sizing == taffy::BoxSizing::ContentBox;
    let min_width = cell.min_width_base.map(|width| match cell.length_width {
        Some(own) if content_box => width.max(own + left + right),
        Some(own) => width.max(own.max(left + right)),
        None => width,
    });
    let changed = cell.style.border != border || cell.min_height != min_height || cell.min_width != min_width;
    cell.style.border = border;
    cell.min_height = min_height;
    cell.min_width = min_width;
    doc.nodes[cell.node_id].set_collapsed_border(Some(edges.physical_sides(grid)));
    changed
}

/// Whether each row's cells follow those of the rows before it in `cells` (`TableRow::cell_count`),
/// the rows numbered from `first`: what `update_table_context` relies on to find a row's cells.
fn cells_follow_rows(rows: &[TableRow], cells: &[TableCell], first: usize) -> bool {
    let mut at = 0;
    for (r, row) in rows.iter().enumerate() {
        let end = at + row.cell_count as usize;
        if end > cells.len() || cells[at..end].iter().any(|cell| cell.row as usize != first + r) {
            return false;
        }
        at = end;
    }
    at == cells.len()
}

/// Rows `old` of the table, to be replaced by what `rows` (row nodes, in order) collect to.
struct RowChange {
    old: Range<usize>,
    rows: Vec<NodeId>,
    group: Option<NodeId>,
}

/// Whether the node or its subtree changed in this pass (a restyle damages a node without marking
/// it as having damaged descendants).
/// pyro: whether a row group of a collapsed table has the border its collapsed borders
/// were resolved with (`TableContext::group_borders`).
fn group_border_kept(ctx: &TableContext, group: &crate::Node) -> bool {
    if ctx.border_collapse != BorderCollapse::Collapse {
        return true;
    }
    let Some(styles) = group.primary_styles() else { return false };
    let border = styles.get_border();
    ctx.group_borders
        .iter()
        .find(|(id, _)| *id == group.id)
        .is_some_and(|(_, kept)| std::ptr::eq(&**kept, border) || **kept == *border)
}

fn changed(node: &crate::Node) -> bool {
    node.flags.contains(NodeFlags::OWN_LAYOUT_DAMAGE) || node.has_damaged_descendants()
}

/// Whether a row needs collecting again: it, or a cell in it, changed (a mutation or a restyle of its
/// own that reaches layout; a cell whose colour or background changed is no change to the row).
/// `None`: a change the row's cells can't be told from (in a `display: contents` box).
fn row_changed(doc: &BaseDocument, row: NodeId) -> Option<bool> {
    let node = &doc.nodes[row];
    if node.flags.contains(NodeFlags::OWN_LAYOUT_DAMAGE) {
        return Some(true);
    }
    for &child in node.children.iter() {
        let child = &doc.nodes[child];
        if !child.is_element() {
            continue;
        }
        // (A restyle damages a cell without marking it as having damaged descendants.)
        if child.flags.contains(NodeFlags::OWN_LAYOUT_DAMAGE) {
            return Some(true);
        }
        if !child.has_damaged_descendants() {
            continue;
        }
        if child.display_style().is_some_and(|d| d.inside() == DisplayInside::Contents) {
            return None;
        }
    }
    Some(false)
}

/// The changes to the rows of `group` (`rows[g0..g1]` of the table). A group with damage of its own
/// may have rows added, removed or replaced: those between the rows it starts and ends with as before.
fn group_changes(
    doc: &BaseDocument,
    ctx: &TableContext,
    group: NodeId,
    own: bool,
    changes: &mut Vec<RowChange>,
) -> bool {
    let Some(g0) = ctx.rows.iter().position(|row| row.group == Some(group)) else { return false };
    let g1 = g0 + ctx.rows[g0..].iter().take_while(|row| row.group == Some(group)).count();
    let children = &doc.nodes[group].children;
    let single = |at: usize, changes: &mut Vec<RowChange>| -> bool {
        let row = ctx.rows[at].node_id;
        if !changed(&doc.nodes[row]) {
            return true;
        }
        match row_changed(doc, row) {
            Some(true) => changes.push(RowChange { old: at..at + 1, rows: vec![row], group: Some(group) }),
            Some(false) => {}
            None => return false,
        }
        true
    };
    if !own {
        // The same children: the rows in order, among boxes the table ignores.
        let mut at = g0;
        for &child in children.iter() {
            if !doc.nodes[child].is_element() {
                continue;
            }
            if at < g1 && ctx.rows[at].node_id == child {
                if !single(at, changes) {
                    return false;
                }
                at += 1;
            } else if doc.nodes[child].flags.contains(NodeFlags::OWN_LAYOUT_DAMAGE) {
                // It may have become a row.
                return false;
            }
        }
        return at == g1;
    }
    // pyro: a group's `::before` and `::after` are in anonymous rows (`table_fixup`).
    if super::table_fixup::has_pseudo_boxes(&doc.nodes[group]) {
        return false;
    }
    let mut rows = Vec::new();
    for &child in children.iter() {
        let node = &doc.nodes[child];
        if !node.is_element() {
            // pyro: text other than white space is an anonymous row's (`table_fixup`).
            if node.is_text_node() && !node.is_whitespace_node() {
                return false;
            }
            continue;
        }
        let Some(display) = node.display_style() else { continue };
        if display.outside() == DisplayOutside::None && display.inside() != DisplayInside::Contents {
            continue;
        }
        if node.primary_styles().is_some_and(|s| s.clone_position().is_absolutely_positioned()) {
            return false;
        }
        match display.inside() {
            DisplayInside::TableRow => rows.push(child),
            // pyro: anything else is in an anonymous row (`table_fixup`).
            _ => return false,
        }
    }
    let old = &ctx.rows[g0..g1];
    let prefix = old.iter().zip(&rows).take_while(|(a, b)| a.node_id == **b).count();
    let most = old.len().min(rows.len()) - prefix;
    let suffix = old.iter().rev().zip(rows.iter().rev()).take(most).take_while(|(a, b)| a.node_id == **b).count();
    for at in (g0..g0 + prefix).chain(g1 - suffix..g1) {
        if !single(at, changes) {
            return false;
        }
    }
    if prefix + suffix < old.len().max(rows.len()) {
        changes.push(RowChange {
            old: g0 + prefix..g1 - suffix,
            rows: rows[prefix..rows.len() - suffix].to_vec(),
            group: Some(group),
        });
    }
    true
}

/// The cells of `table` (at `at`, the fractional part of its position) whose rounding may have changed
/// since it was last rounded: those its grid placed since, if it sits on the pixel grid as it did then
/// (`None`: all).
pub(crate) fn table_cells_to_round(
    doc: &BaseDocument,
    table: NodeId,
    at: taffy::Point<f32>,
) -> Option<Vec<NodeId>> {
    let node = &doc.nodes[table];
    if !node.flags.is_table_root() {
        return None;
    }
    let SpecialElementData::TableRoot(ctx) = &node.data.downcast_element()?.special_data else { return None };
    if !ctx.incremental || !ctx.oof_children.is_empty() {
        return None;
    }
    let cells = ctx.placed.borrow_mut().take_round(at)?;
    Some(cells.into_iter().filter_map(|index| ctx.cells.get(index).map(|cell| cell.node_id)).collect())
}

/// Whether `resolve_transforms` keeps the overflow of `ctx`'s cells between passes
/// (`table_cells_overflow`): a table whose layout children are its cells and pseudo-elements.
pub(crate) fn keeps_cells(ctx: &TableContext) -> bool {
    ctx.incremental && ctx.oof_children.is_empty()
}

/// pyro: the layout children `resolve_layout_children` visits in `table` when the table is
/// neither built again nor updated (a change inside its cells): the cells damage propagation noted
/// as changed in layout since the grid's last layout (`TableContext::changed_cells`), and the
/// pseudo-elements. `None`: `table` does not keep its cells, and all its layout children are visited.
///
/// Any other cell has nothing to build below it: construction anywhere in a cell's subtree reaches
/// the cell as layout damage, and `note_changed_table_cell` lists it. A cell edit in a long table
/// visits that cell, without a loop over its thousands of siblings.
pub(crate) fn cells_to_visit(doc: &BaseDocument, table: NodeId) -> Option<Vec<NodeId>> {
    let node = &doc.nodes[table];
    if !node.flags.is_table_root() {
        return None;
    }
    let SpecialElementData::TableRoot(ctx) = &node.data.downcast_element()?.special_data else { return None };
    if !keeps_cells(ctx) {
        return None;
    }
    let mut changed = ctx.changed_cells.borrow().clone();
    changed.sort_unstable();
    changed.dedup();
    #[cfg(debug_assertions)]
    for (index, cell) in ctx.cells.iter().enumerate() {
        if changed.binary_search(&index).is_err() {
            let Some(cell) = doc.nodes.get(cell.node_id) else { continue };
            debug_assert!(
                cell.is_clean_layout_child_of(table)
                    || (cell.layout_parent.get() == Some(table)
                        && !cell.damage().is_none_or(super::damage::reaches_layout)),
                "cell {index} ({:?}) of {table:?} is not listed as changed but has damage {:?}",
                cell.id,
                cell.damage(),
            );
        }
    }
    let pseudos = [node.before(), node.after()];
    Some(
        changed
            .into_iter()
            .filter_map(|index| ctx.cells.get(index).map(|cell| cell.node_id))
            .chain(pseudos.into_iter().flatten())
            .collect(),
    )
}

/// Whether `table` is a table whose `TableContext` `update_table_context` can update.
pub(crate) fn is_incremental(doc: &BaseDocument, table: NodeId) -> bool {
    let node = &doc.nodes[table];
    // (A table that left the layout tree, `BaseDocument::leave_layout_tree`, lists no cells.)
    node.flags.is_table_root()
        && node.layout_children.borrow().is_some()
        && node.data.downcast_element().is_some_and(|element| {
            matches!(&element.special_data, SpecialElementData::TableRoot(ctx) if ctx.incremental)
        })
}

/// pyro: brings the table's `TableContext` up to date when only some of its rows changed
/// (a cell's content or style, rows added to or removed from a row group): those rows are collected
/// again and the collapsed borders around them resolved again; the other rows and cells stay as they
/// are. `false`: the change is not one of those, and the table is to be built again.
///
/// The cells whose layout children may need building again: `Some(None)`, all of them (rows were
/// added or removed), else those that changed (with damage of their own or in their subtree, or
/// collected again).
pub(crate) fn update_table_context(doc: &mut BaseDocument, table: NodeId) -> Option<Option<Vec<NodeId>>> {
    let node = &doc.nodes[table];
    if !node.flags.is_table_root() || node.flags.contains(NodeFlags::OWN_LAYOUT_DAMAGE) {
        return None;
    }
    if [node.before(), node.after()]
        .into_iter()
        .flatten()
        .any(|id| doc.nodes[id].flags.contains(NodeFlags::OWN_LAYOUT_DAMAGE))
    {
        return None;
    }
    let element = doc.nodes[table].data.downcast_element_mut()?;
    let mut data = std::mem::take(&mut element.special_data);
    let updated = match &mut data {
        SpecialElementData::TableRoot(ctx) => Arc::get_mut(ctx).and_then(|ctx| {
            let boxes_changed = update_rows(doc, table, ctx)?;
            // (`changed_cells` holds the cells damage propagation noted, and those collected again
            // whose box changed; the others collected again are cells with damage of their own.)
            Some((!boxes_changed).then(|| {
                let cells = ctx.changed_cells.get_mut();
                cells.sort_unstable();
                cells.dedup();
                cells.iter().filter_map(|&index| ctx.cells.get(index).map(|cell| cell.node_id)).collect()
            }))
        }),
        _ => None,
    };
    doc.nodes[table].data.downcast_element_mut().unwrap().special_data = data;
    updated
}

fn update_rows(doc: &mut BaseDocument, table: NodeId, ctx: &mut TableContext) -> Option<bool> {
    if !ctx.incremental {
        return None;
    }
    let Some(first_cell_row) = ctx.cells.first().map(|cell| cell.row as usize) else { return None };

    // 1. The rows that changed.
    let mut changes: Vec<RowChange> = Vec::new();
    for &child in doc.nodes[table].children.iter() {
        let node = &doc.nodes[child];
        if !node.is_element() || !changed(node) {
            continue;
        }
        let own = node.flags.contains(NodeFlags::OWN_LAYOUT_DAMAGE);
        let Some(display) = node.display_style() else {
            if own {
                return None;
            }
            continue;
        };
        let absolute = node.primary_styles().is_some_and(|s| s.clone_position().is_absolutely_positioned());
        if display.outside() == DisplayOutside::None || absolute {
            if own {
                return None;
            }
            continue;
        }
        match display.inside() {
            DisplayInside::TableRowGroup => {
                // A collapsed table resolves its borders with its row groups'.
                if own && !group_border_kept(ctx, node) {
                    return None;
                }
                if !group_changes(doc, ctx, child, own, &mut changes) {
                    return None;
                }
            }
            DisplayInside::TableHeaderGroup | DisplayInside::TableFooterGroup if !own => {
                if !group_changes(doc, ctx, child, false, &mut changes) {
                    return None;
                }
            }
            DisplayInside::TableRow => {
                let Some(at) = ctx.rows.iter().position(|row| row.node_id == child) else { return None };
                match row_changed(doc, child) {
                    Some(true) => changes.push(RowChange { old: at..at + 1, rows: vec![child], group: ctx.rows[at].group }),
                    Some(false) => {}
                    None => return None,
                }
            }
            DisplayInside::Flow | DisplayInside::FlowRoot | DisplayInside::Flex | DisplayInside::Grid if !own => {}
            _ => return None,
        }
    }
    changes.sort_by_key(|change| (change.old.start, change.old.end));
    // The first row's cells give the columns their widths, and the first cell the gutters.
    if changes.first().is_some_and(|change| change.old.start <= first_cell_row)
        || changes.windows(2).any(|pair| pair[0].old.end > pair[1].old.start)
    {
        return None;
    }
    if changes.is_empty() {
        return Some(false);
    }

    // 2. Collect the changed rows again.
    let wm = table_wm(doc, table);
    let mut collected: Vec<(Vec<TableRow>, Vec<TableCell>)> = Vec::with_capacity(changes.len());
    for change in &changes {
        let mut row = change.old.start as u16;
        let mut cursor = ColumnCursor::default();
        let (mut cells, mut oof, mut rows) = (Vec::new(), Vec::new(), Vec::new());
        let (mut columns, mut percent_columns) = (Vec::new(), Vec::new());
        let mut first_cell_border = ctx.border_style.clone();
        let mut anonymous = Vec::new();
        for &node_id in &change.rows {
            let node = &doc.nodes[node_id];
            // (pyro: a row made `display: contents` hands its cells to an anonymous row:
            // `display: contents` has no display outside, and is structural all the same.)
            // pyro: anything but a row (or no box) is wrapped in anonymous boxes now
            // (`table_fixup`): the table is built again.
            let structural = node.is_element()
                && node.display_style().is_some_and(|display| {
                    display.inside() != DisplayInside::TableRow
                        && !(display.outside() == DisplayOutside::None && display.inside() != DisplayInside::Contents)
                });
            if structural {
                return None;
            }
            collect_table_cells(
                doc,
                node_id,
                wm,
                ctx.is_fixed,
                ctx.border_collapse,
                &mut row,
                &mut cursor,
                &mut cells,
                &mut oof,
                &mut rows,
                &mut columns,
                &mut first_cell_border,
                &mut percent_columns,
                &mut anonymous,
            );
        }
        // pyro: a row that now has anonymous cells (or what else the fixup wraps) is built with
        // the table again.
        if !anonymous.is_empty() || cells.iter().any(|cell| doc.nodes[cell.node_id].is_anonymous()) {
            for id in anonymous {
                doc.deallocate_anonymous_block(id);
            }
            return None;
        }
        if !oof.is_empty()
            || !columns.is_empty()
            || !percent_columns.is_empty()
            || cells.iter().any(|cell| cell.rowspan > 1)
            || !cells_follow_rows(&rows, &cells, change.old.start)
        {
            return None;
        }
        for row in rows.iter_mut() {
            row.group = change.group;
        }
        collected.push((rows, cells));
    }

    // pyro: a table left without rows has no border spacing (`build_table_context`): built again.
    let removed: usize = changes.iter().map(|change| change.old.len()).sum();
    let added: usize = collected.iter().map(|(rows, _)| rows.len()).sum();
    if ctx.rows.len() + added == removed {
        return None;
    }

    // The columns the cells reach, and the columns with cells of their own, stay the same.
    let mut columns = 0u16;
    let mut r = 0;
    for change in &changes {
        columns = ctx.rows[r..change.old.start].iter().map(|row| row.columns).fold(columns, u16::max);
        r = change.old.end;
    }
    columns = ctx.rows[r..].iter().map(|row| row.columns).fold(columns, u16::max);
    let mut single_cell_columns = ctx.single_cell_columns.clone();
    for (change, (rows, cells)) in changes.iter().zip(&collected) {
        columns = rows.iter().map(|row| row.columns).fold(columns, u16::max);
        for cell in cells_of(&ctx.cells, change.old.clone()).iter().filter(|cell| cell.colspan == 1) {
            single_cell_columns[cell.column as usize] -= 1;
        }
        for cell in cells.iter().filter(|cell| cell.colspan == 1) {
            let Some(count) = single_cell_columns.get_mut(cell.column as usize) else { return None };
            *count += 1;
        }
    }
    if columns != ctx.cursor_columns
        || single_cell_columns.iter().zip(&ctx.single_cell_columns).any(|(new, old)| (*new > 0) != (*old > 0))
    {
        return None;
    }
    ctx.single_cell_columns = single_cell_columns;

    // 3. Replace the rows, the last first so that the earlier ones keep their indices.
    let ncols = ctx.style.grid_template_columns.len();
    let lengths: Vec<usize> = collected.iter().map(|(rows, _)| rows.len()).collect();
    let old_rows = ctx.rows.len();
    let appended_only = changes.iter().all(|change| change.old.is_empty() && change.old.start == old_rows);
    let mut removed: Vec<TableCell> = Vec::new();
    let mut boxes_changed = false;
    let mut renumber = false;
    let mut first_moved = usize::MAX;
    let old_cells = ctx.cells.len();
    // The cells replaced by others (the grid lays out again from its last layout across one such change).
    let mut splices: Vec<taffy::TableGridSplice> = Vec::new();
    // pyro: whether a cell is aligned on its row's baseline, the new ones or one kept.
    let new_baseline = collected.iter().any(|(_, cells)| cells.iter().any(|cell| cell.baseline));
    for (change, (rows, cells)) in changes.iter().zip(collected).rev() {
        let (a, b) = (change.old.start, change.old.end);
        let (ca, cb) = cell_range(&ctx.cells, a..b);
        let same_boxes = cb - ca == cells.len()
            && ctx.cells[ca..cb].iter().zip(&cells).all(|(old, new)| old.node_id == new.node_id);
        let same_columns = same_boxes
            && ctx.cells[ca..cb].iter().zip(&cells).all(|(old, new)| (old.column, old.colspan) == (new.column, new.colspan));
        if !same_boxes {
            boxes_changed = true;
            if cb - ca != cells.len() {
                first_moved = first_moved.min(ca);
            }
        }
        renumber |= rows.len() != b - a;
        let (new_rows, new_cells) = (rows.len(), cells.len());
        // The cell widths follow the cells while they are kept: `CellWidths::changed` drops them
        // all (to measure every cell again) once more cells changed than it holds, which an
        // earlier change of this loop may have done.
        let widths_kept = ctx.widths.get_mut().cells.len() == ctx.cells.len();
        let percent = |cells: &[TableCell]| cells.iter().filter(|cell| cell.percent_width).count();
        ctx.percent_cells = ctx.percent_cells + percent(&cells) - percent(&ctx.cells[ca..cb]);
        // The cells' grid areas (`TableContext::placement`) follow the cells: no cell spans rows, so
        // each new cell's area is its row and the columns the cursor gave it.
        if ctx.placement.is_some() {
            let areas: Option<Vec<_>> = cells.iter().map(unspanned_cell_area).collect();
            match (ctx.placement.as_mut(), areas) {
                (Some(placement), Some(areas)) => {
                    debug_assert!(areas.iter().all(|area| area.width.end <= placement.extent.width));
                    placement.areas.splice(ca..cb, areas);
                }
                _ => ctx.placement = None,
            }
        }
        removed.extend(ctx.cells.splice(ca..cb, cells));
        ctx.rows.splice(a..b, rows);
        if !same_boxes {
            splices.push(taffy::TableGridSplice {
                items: ca..cb,
                inserted: new_cells,
                rows: a as u16..b as u16,
                inserted_rows: new_rows as u16,
            });
            // The indices after the replaced cells move.
            splice_indices(ctx.changed_cells.get_mut(), ca..cb, new_cells);
            splice_indices(&mut ctx.widths.get_mut().pending, ca..cb, new_cells);
            ctx.placed.get_mut().splice(ca..cb, new_cells, ctx.cells.len());
            // (A plain table's paint children are its cells: `table_paint_children_kept`.)
            if ctx.placed.get_mut().plain_paint_order.is_some() {
                if let Some(paint) = doc.nodes[table].paint_children.get_mut().as_mut() {
                    if paint.len() == ctx.cells.len() + (cb - ca) - new_cells {
                        paint.splice(ca..cb, ctx.cells[ca..ca + new_cells].iter().map(|cell| cell.node_id));
                    }
                }
            }
        }
        if widths_kept {
            // Measured again; the column widths stand while the cells keep their columns, or the
            // rows were appended.
            let widths = ctx.widths.get_mut();
            if !same_columns {
                widths.cells.splice(ca..cb, std::iter::repeat_n(None, new_cells));
                if !appended_only {
                    widths.columns = None;
                }
            }
            for index in ca..ca + new_cells {
                widths.changed(index);
            }
        }
        if let Some(edges) = ctx.collapsed_edges.as_mut().or(ctx.spare_edges.as_mut()) {
            debug_assert_eq!(edges.vertical.len(), ncols + 1);
            for line in &mut edges.vertical {
                line.splice(a..b, new_rows);
            }
            edges.horizontal.splice(a..b + 1, std::iter::repeat_n(EdgeLine::default(), new_rows + 1));
            edges.rows = edges.rows + new_rows - (b - a);
        }
    }
    ctx.baseline_cells = new_baseline || (ctx.baseline_cells && ctx.cells.iter().any(|cell| cell.baseline));
    if renumber {
        let mut at = 0;
        for (r, row) in ctx.rows.iter().enumerate() {
            for cell in &mut ctx.cells[at..at + row.cell_count as usize] {
                cell.row = r as u16;
                cell.style.grid_row.start = style_helpers::line(r as i16 + 1);
            }
            if let Some(placement) = ctx.placement.as_mut() {
                for area in &mut placement.areas[at..at + row.cell_count as usize] {
                    area.height = taffy::Line { start: r as u16, end: r as u16 + 1 };
                }
            }
            at += row.cell_count as usize;
        }
    }
    if let Some(placement) = ctx.placement.as_mut() {
        // The rows the cells reach (the columns stay: `cursor_columns` above).
        placement.extent.height = ctx.cells.last().map_or(0, |cell| cell.row + 1);
        debug_assert_eq!(placement.areas.len(), ctx.cells.len());
    }
    // The grid laid out last time has the same items but for those of the rows replaced (see
    // `take_table_grid_state`): across one replacement since its last layout (appended rows are one,
    // at its end).
    if !splices.is_empty() {
        let pending = ctx.grid_splice.get_mut();
        if splices.len() > 1 || pending.is_some() {
            *ctx.grid_state.get_mut() = None;
            *pending = None;
            ctx.changed_cells.get_mut().clear();
        } else if splices[0].items.start < old_cells || splices[0].rows.start as usize != old_rows {
            *pending = splices.pop();
        }
    }
    // Where the changed rows are now.
    let mut spans = Vec::with_capacity(changes.len());
    let mut shift = 0isize;
    for (change, &len) in changes.iter().zip(&lengths) {
        let start = (change.old.start as isize + shift) as usize;
        spans.push(start..start + len);
        shift += len as isize - change.old.len() as isize;
    }
    // Each cell's index, on its node: the cells moved and those collected again.
    for at in first_moved..ctx.cells.len() {
        doc.nodes[ctx.cells[at].node_id].layout_data_mut().table_cell_index = at as u32;
    }
    for span in &spans {
        let (ca, cb) = cell_range(&ctx.cells, span.clone());
        for at in ca..cb.min(first_moved) {
            doc.nodes[ctx.cells[at].node_id].layout_data_mut().table_cell_index = at as u32;
        }
    }

    // A cell's minimum width from its column's (auto tables; see `build_table_context`).
    if !ctx.is_fixed {
        for span in &spans {
            let (ca, cb) = cell_range(&ctx.cells, span.clone());
            for cell in ctx.cells[ca..cb].iter_mut().filter(|cell| cell.colspan == 1) {
                let Some(taffy::GridTemplateComponent::Single(track)) =
                    ctx.style.grid_template_columns.get(cell.column as usize)
                else {
                    continue;
                };
                if track.max.into_raw().tag() == taffy::CompactLength::FIT_CONTENT_PX_TAG {
                    let width = track.max.into_raw().value();
                    cell.min_width_base = Some(cell.length_width.map_or(width, |w| w.max(width)));
                    cell.min_width = cell.min_width_base;
                }
            }
        }
    }

    // 4. The collapsed borders around the changed rows, and the cells beside them.
    let nrows = ctx.rows.len();
    let mut touched = Vec::new();
    if let Some(edges) = ctx.collapsed_edges.as_mut().or(ctx.spare_edges.as_mut()) {
        for span in &spans {
            let range = EdgeRange { lines: span.start..span.end + 1, rows: span.clone() };
            let around = span.start.saturating_sub(1)..(span.end + 1).min(nrows);
            let (ca, cb) = cell_range(&ctx.cells, around.clone());
            for cell in &ctx.cells[ca..cb] {
                let area = cell_area(cell, ncols);
                if area.0 < area.2 && area.1 < area.3 {
                    offer_box(edges, doc, cell.node_id, area, CELL_PRIORITY, &range);
                }
            }
            offer_rows_groups_and_table(edges, doc, table, &ctx.rows, &ctx.columns, around, &range);
            for cell in &ctx.cells[ca..cb] {
                clear_spanned(edges, cell_area(cell, ncols), &range);
            }
            touched.push((ca, cb));
        }
        if edges_in_use(edges, ctx.collapse_gap) != ctx.collapsed_edges.is_some() {
            return None;
        }
    }
    let changed_cells = ctx.changed_cells.get_mut();
    if let Some(edges) = &ctx.collapsed_edges {
        ctx.style.border = collapsed_table_border(edges);
        for &(ca, cb) in &touched {
            for (at, cell) in ctx.cells[ca..cb].iter_mut().enumerate() {
                let area = cell_area(cell, ncols);
                // (Those collected again are compared with what they were below.)
                let collected = spans.iter().any(|span| span.contains(&(cell.row as usize)));
                if apply_collapsed_borders(doc, cell, area, edges) && !collected {
                    doc.nodes[cell.node_id].clear_layout_cache();
                    changed_cells.push(ca + at);
                }
            }
        }
    } else {
        for span in &spans {
            let (ca, cb) = cell_range(&ctx.cells, span.clone());
            for cell in &ctx.cells[ca..cb] {
                doc.nodes[cell.node_id].set_collapsed_border(None);
            }
        }
    }
    // A cell collected again whose box is not what it was (it, or a cell beside it, changed) lays out
    // again, and is measured and positioned again.
    for span in &spans {
        let (ca, cb) = cell_range(&ctx.cells, span.clone());
        for (at, cell) in ctx.cells[ca..cb].iter().enumerate() {
            let same = removed.iter().find(|old| old.node_id == cell.node_id).is_some_and(|old| {
                old.style == cell.style
                    && old.min_width == cell.min_width
                    && old.min_height == cell.min_height
                    && old.column == cell.column
            });
            if !same {
                doc.nodes[cell.node_id].clear_layout_cache();
                changed_cells.push(ca + at);
            }
        }
    }

    let gutter = match (ctx.border_collapse, &ctx.collapsed_edges) {
        (BorderCollapse::Collapse, None) => ctx.collapse_gap.1,
        _ => 0.0,
    };
    ctx.style.grid_template_rows = row_tracks(&ctx.rows, gutter);
    // pyro: a collapsed row takes its border spacing with it (taffy `Style::collapsed_rows`).
    ctx.style.collapsed_rows = ctx.rows.iter().enumerate().filter(|(_, row)| row.collapsed).map(|(i, _)| i as u16).collect();
    ctx.style.collapsed_columns = collapsed_columns(doc, &ctx.columns);
    if boxes_changed {
        let (before, after) = (doc.nodes[table].before(), doc.nodes[table].after());
        let layout_children = before
            .into_iter()
            .chain(ctx.cells.iter().map(|cell| cell.node_id))
            .chain(ctx.oof_children.iter().map(|(node_id, _)| *node_id))
            .chain(after)
            .collect::<thin_vec::ThinVec<NodeId>>();
        // The boxes that left the list leave it in `Node::layout_child_of` too, and the new ones
        // enter it (as `resolve_layout_children` does when it lists a node's children).
        leave_layout_children(doc, table);
        for &child in layout_children.iter() {
            doc.nodes[child].layout_child_of.set(Some(table));
        }
        *doc.nodes[table].layout_children.borrow_mut() = Some(layout_children);
        // The cells replaced and not collected again are no longer cells.
        release_former_cells(doc, table, &removed, &ctx.cells);
    }
    Some(boxes_changed)
}

/// The indices in `cells` of the cells of `rows`.
fn cell_range(cells: &[TableCell], rows: Range<usize>) -> (usize, usize) {
    let start = cells.partition_point(|cell| (cell.row as usize) < rows.start);
    let end = start + cells[start..].partition_point(|cell| (cell.row as usize) < rows.end);
    (start, end)
}

fn cells_of(cells: &[TableCell], rows: Range<usize>) -> &[TableCell] {
    let (start, end) = cell_range(cells, rows);
    &cells[start..end]
}

/// The grid area of a cell spanning one row (`cell_areas` for a table without rowspans).
fn cell_area(cell: &TableCell, ncols: usize) -> (usize, usize, usize, usize) {
    let (r, c) = (cell.row as usize, (cell.column as usize).min(ncols));
    (r, c, r + 1, (c + cell.colspan as usize).min(ncols))
}

/// Collect `<col>` elements (and `display: table-column` elements) along with their
/// widths, which take precedence over cell widths in the fixed table layout algorithm.
/// Columns without a specified width are recorded as `auto`.
fn collect_columns(
    doc: &mut BaseDocument,
    node_id: NodeId,
    wm: WritingMode,
    columns: &mut Vec<TableColumn>,
    column_sizes: &mut Vec<TrackSizingFunction>,
) {
    let node = &doc.nodes[node_id];
    if !node.is_element() {
        return;
    }
    let Some(display) = node.display_style() else {
        return;
    };

    match display.inside() {
        DisplayInside::TableColumnGroup => {
            let first_column = columns.len();
            let children = std::mem::take(&mut doc.nodes[node_id].children);
            for child_id in children.iter().copied() {
                collect_columns(doc, child_id, wm, columns, column_sizes);
            }
            doc.nodes[node_id].children = children;
            for column in &mut columns[first_column..] {
                column.group.get_or_insert(node_id);
            }
            // pyro: a column group without columns stands for its `span` columns (HTML's `colgroup`;
            // Chromium: one), with its own width, background and borders as theirs.
            if columns.len() == first_column {
                push_columns(&doc.nodes[node_id], wm, columns, column_sizes);
            }
        }
        DisplayInside::TableColumn => push_columns(node, wm, columns, column_sizes),
        _ => {}
    }
}

/// The columns of a column (or of a column group without columns): `span` of them, as wide as its
/// `width` says (its `min-width` and `max-width` too).
/// pyro: the columns whose `<col>` (or column group) is `visibility: collapse` (CSS 2.1 § 17.5.5):
/// sized with the others, then taking no width (taffy `Style::collapsed_columns`).
fn collapsed_columns(doc: &BaseDocument, columns: &[TableColumn]) -> Vec<u16> {
    columns
        .iter()
        .enumerate()
        .filter(|(_, column)| {
            doc.nodes[column.node_id].primary_styles().is_some_and(|style| {
                style.get_inherited_box().visibility == style::computed_values::visibility::T::Collapse
            })
        })
        .map(|(i, _)| i as u16)
        .collect()
}

fn push_columns(
    node: &crate::Node,
    wm: WritingMode,
    columns: &mut Vec<TableColumn>,
    column_sizes: &mut Vec<TrackSizingFunction>,
) {
    let style = table_taffy_style(node, wm);
    let span: u16 =
        node.attr(local_name!("span")).and_then(|val| val.parse::<u16>().ok()).map(|v| v.max(1)).unwrap_or(1);
    let column: TrackSizingFunction = match style.size.width.tag() {
        taffy::CompactLength::LENGTH_TAG => {
            // A definite `max-width` clamps the column width; a zero width is treated as auto
            let mut width = style.size.width.value();
            if style.max_size.width.into_raw().tag() == taffy::CompactLength::LENGTH_TAG {
                width = width.min(style.max_size.width.into_raw().value());
            }
            if width > 0.0 {
                style_helpers::length(width)
            } else {
                style_helpers::auto()
            }
        }
        taffy::CompactLength::PERCENT_TAG => style_helpers::percent(style.size.width.value()),
        // Browsers treat calc() widths on columns as auto
        _ => style_helpers::auto(),
    };
    // A definite `min-width` on a column acts as a floor on its width
    let column = match style.min_size.width.into_raw().tag() {
        taffy::CompactLength::LENGTH_TAG if column.max.is_auto() => {
            style_helpers::minmax(style_helpers::length(style.min_size.width.into_raw().value()), style_helpers::auto())
        }
        _ => column,
    };
    for _ in 0..span {
        columns.push(TableColumn { node_id: node.id, group: None });
        column_sizes.push(column);
    }
}

/// pyro: the `TableContext` `table` was last built with, if it was a table.
fn former_table_context(doc: &BaseDocument, table: NodeId) -> Option<Arc<TableContext>> {
    match &doc.nodes[table].data.downcast_element()?.special_data {
        SpecialElementData::TableRoot(ctx) => Some(Arc::clone(ctx)),
        _ => None,
    }
}

/// The row groups and rows (and the `display: contents` boxes between them and the table) of a table
/// that is now built as another box: built as boxes in their turn (`collect_layout_children_with_wrap`).
/// They are those of the table's last `TableContext`, whatever their `display` is now (a row group of
/// a table made a flex container is a block item now): table construction dropped the boxes they had
/// (`drop_part_box`). Its cells are released (`release_cell`), and the context dropped: the box is no
/// table (pyro-render reads a table's collapsed borders from it).
pub(crate) fn damage_former_table_parts(doc: &mut BaseDocument, node_id: NodeId) {
    let Some(element) = doc.nodes[node_id].data.downcast_element_mut() else { return };
    if !matches!(element.special_data, SpecialElementData::TableRoot(_)) {
        return;
    }
    let SpecialElementData::TableRoot(ctx) = std::mem::take(&mut element.special_data) else { return };
    // A row and its ancestors below the table (one moved out of the table since is left alone).
    let below_table = |doc: &BaseDocument, part: NodeId| -> Vec<NodeId> {
        let mut parts = Vec::new();
        let mut at = Some(part);
        while let Some(id) = at {
            if id == node_id {
                return parts;
            }
            parts.push(id);
            at = doc.nodes.get(id).and_then(|node| node.parent);
        }
        Vec::new()
    };
    let mut row_parts = std::collections::HashSet::new();
    for row in &ctx.rows {
        let parts = below_table(doc, row.node_id);
        row_parts.extend(parts.last().copied());
        for part in parts {
            let node = &mut doc.nodes[part];
            node.insert_damage(super::damage::ALL_DAMAGE);
            node.clear_layout_cache();
        }
    }
    // The children the table ignored (captions, columns, empty row groups): no pass built their
    // subtrees (`collect_table_cells` drops their damage), which are boxes now.
    let children = doc.nodes[node_id].children.clone();
    for child in children {
        if doc.nodes[child].is_element() && !row_parts.contains(&child) {
            enter_layout_tree(doc, child);
        }
    }
    release_former_cells(doc, node_id, &ctx.cells, &[]);
}

/// pyro: a cell collected while no container lists it (`Node::layout_child_of`): new, or
/// back from outside the layout tree (its row was `display: contents`, it was a block the table
/// ignored). Stylo styled it meanwhile and `clear_damage_and_dirty_flags` cleared the damage that no
/// pass consumed (no pass reached it), so what its subtree built before is stale: built again, as a
/// fresh build does.
fn enter_layout_tree(doc: &mut BaseDocument, node_id: NodeId) {
    doc.iter_subtree_mut(node_id, |id, doc| {
        let node = &mut doc.nodes[id];
        if node.is_element() {
            node.insert_damage(super::damage::ALL_DAMAGE);
        }
        node.clear_layout_cache();
    });
}

/// pyro: a node that is no longer one of a table's cells (a box of its own, out of flow, or
/// its table gone) has no half collapsed borders (its layout style and paint, which draws a node's
/// own borders only without them, follow) and no index into the table's cells.
fn release_cell(doc: &mut BaseDocument, node_id: NodeId) {
    let Some(node) = doc.nodes.get_mut(node_id) else { return };
    node.set_collapsed_border(None);
    node.layout_data_mut().table_cell_index = u32::MAX;
    set_cell_baseline_offset(node, 0.0);
}

/// pyro: whether a table cell styled `style` is aligned on its row's baseline: its `vertical-align` is
/// none of `top`, `middle` and `bottom` (CSS 2.1 §17.5.3: the other values act as `baseline`) and its
/// `align-content` is `normal`.
fn is_baseline_aligned(style: &style::properties::ComputedValues) -> bool {
    style.get_position().align_content.primary() == style::values::specified::align::AlignFlags::NORMAL
        && stylo_taffy::convert::table_cell_vertical_align(style).is_none()
}

/// pyro: sets how far down a table cell's content is laid out in its box (`shift_cell_content`). A
/// change clears the cell's layout cache: the next layout of the cell lays its content out again.
fn set_cell_baseline_offset(node: &mut crate::Node, offset: f32) {
    let had = node.flags.contains(NodeFlags::HAS_TABLE_BASELINE_OFFSET);
    if !had && offset == 0.0 {
        return;
    }
    let data = node.layout_data_mut();
    if had && data.table_baseline_offset == offset {
        return;
    }
    data.table_baseline_offset = offset;
    data.cache.clear();
    node.flags.set(NodeFlags::HAS_TABLE_BASELINE_OFFSET, offset != 0.0);
}

/// pyro: moves a table cell's content (`node_id`, just laid out: `output`) down in its box by its
/// `LayoutData::table_baseline_offset`, for a cell aligned on its row's baseline (`taffy`'s
/// `table_baseline`): its in-flow and floated children, its lines (`TextLayout::block_offset`), the
/// static positions of its out-of-flow descendants, its scrollable overflow and its baselines. Runs
/// inside the cell's layout cache (`LayoutPassState::compute_child_layout_in_current_wm`), so a layout of
/// the cell that the cache answers keeps its content where this put it.
pub(crate) fn shift_cell_content(doc: &mut BaseDocument, node_id: NodeId, output: &mut taffy::LayoutOutput) {
    let offset = doc.nodes[node_id].layout_data().table_baseline_offset;
    if offset == 0.0 {
        return;
    }
    let children: Vec<NodeId> = doc.nodes[node_id].layout_children.borrow().as_deref().unwrap_or(&[]).to_vec();
    for child in children {
        let node = &mut doc.nodes[child];
        if node.is_out_of_flow() {
            continue;
        }
        node.update_unrounded_layout(|layout| layout.location.y += offset);
    }
    if let Some(inline) = doc.nodes[node_id].element_data_mut().and_then(|el| el.inline_layout_data.as_mut()) {
        inline.block_offset += offset;
    }
    output.oof_candidates.translate(taffy::Point { x: 0.0, y: offset });
    let overflow = output.scrollable_overflow_rect;
    let mut rect = overflow.rect();
    rect.bottom += offset;
    output.scrollable_overflow_rect = taffy::ScrollableOverflow::new(rect, overflow.is_reversed());
    output.baselines.first = output.baselines.first.map(|baseline| baseline + offset);
    output.baselines.last = output.baselines.last.map(|baseline| baseline + offset);
}

/// pyro: `table`'s former cells (`old`) that it lists no more (`cells`, whose nodes hold
/// their index) are released (`release_cell`), except one another container collected since.
fn release_former_cells(doc: &mut BaseDocument, table: NodeId, old: &[TableCell], cells: &[TableCell]) {
    for cell in old {
        let Some(node) = doc.nodes.get(cell.node_id) else { continue };
        let index = node.layout_data().table_cell_index as usize;
        if cells.get(index).is_some_and(|kept| kept.node_id == cell.node_id) {
            continue;
        }
        if node.layout_child_of.get().is_some_and(|other| other != table) {
            continue;
        }
        release_cell(doc, cell.node_id);
    }
}

/// pyro: `node_id`'s layout children that are still listed there (`Node::layout_child_of`)
/// leave its list; one another container collected since points there and is left alone.
fn leave_layout_children(doc: &mut BaseDocument, node_id: NodeId) {
    let BaseDocument { nodes, left_layout_lists, .. } = doc;
    if let Some(children) = nodes[node_id].layout_children.borrow().as_ref() {
        for &child_id in children.iter() {
            if let Some(child) = nodes.get(child_id) {
                if child.layout_child_of.get() == Some(node_id) {
                    child.layout_child_of.set(None);
                    left_layout_lists.push(child_id);
                }
            }
        }
    }
}

/// A row group or row is a part of its table, not a box: it has no layout children of its own
/// (those of a box it was before: a table built in place of a block).
fn drop_part_box(doc: &mut BaseDocument, node_id: NodeId) {
    // pyro: nor a container: what it was a layout child of (an anonymous cell it was a block in,
    // freed since) is no containing block of it.
    doc.nodes[node_id].layout_parent.set(None);
    doc.nodes[node_id].oof_containing_block.set(None);
    leave_layout_children(doc, node_id);
    *doc.nodes[node_id].layout_children.borrow_mut() = None;
    *doc.nodes[node_id].paint_children.borrow_mut() = None;
    for anonymous in std::mem::take(&mut doc.nodes[node_id].anonymous_blocks) {
        doc.deallocate_anonymous_block(anonymous);
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_table_cells(
    doc: &mut BaseDocument,
    node_id: NodeId,
    wm: WritingMode,
    is_fixed: bool,
    border_collapse: BorderCollapse,
    row: &mut u16,
    cursor: &mut ColumnCursor,
    cells: &mut Vec<TableCell>,
    oof_children: &mut Vec<(NodeId, taffy::Position)>,
    rows: &mut Vec<TableRow>,
    columns: &mut Vec<TrackSizingFunction>,
    first_cell_border: &mut Option<ServoArc<Border>>,
    percent_columns: &mut Vec<(u16, f32, f32)>,
    anonymous: &mut Vec<NodeId>,
) {
    let node = &mut doc.nodes[node_id];

    // (Anonymous rows and cells, and pseudo-elements, are `AnonymousBlock` nodes.)
    if !node.is_element() && !node.is_anonymous() {
        return;
    }

    let Some(display) = node.display_style() else {
        #[cfg(feature = "tracing")]
        tracing::info!("Ignoring table descendent because it has no styles");
        return;
    };

    if display.outside() == DisplayOutside::None {
        node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
        return;
    }

    // Absolutely positioned boxes are out-of-flow: they are laid out by their containing
    // block rather than as part of the table grid.
    let position = node.primary_styles().unwrap().clone_position();
    if position.is_absolutely_positioned() {
        // pyro: a box of its own, laid out and painted with its own borders.
        release_cell(doc, node_id);
        oof_children.push((node_id, stylo_taffy::convert::position(position)));
        return;
    }

    match display.inside() {
        DisplayInside::TableRowGroup
        | DisplayInside::TableHeaderGroup
        | DisplayInside::TableFooterGroup
        | DisplayInside::Contents => {
            drop_part_box(doc, node_id);
            let first_row = rows.len();
            if display.inside() != DisplayInside::Contents {
                cursor.end_group();
            }
            // pyro: what is not a row in anonymous rows (`table_fixup`); else its own children,
            // put back below.
            let fixed = super::table_fixup::fixup_children(doc, node_id, Level::RowGroup, wm.is_vertical(), anonymous);
            doc.nodes[node_id].remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
            let own =
                if fixed.is_none() { std::mem::take(&mut doc.nodes[node_id].children) } else { Default::default() };
            for child_id in fixed.as_deref().unwrap_or(&own[..]).iter().copied() {
                // pyro: not an absolutely positioned child's: it is a box of its own (the table's
                // `oof_children`), whose content is still to be built (it was never laid out: WPT
                // css-contain `contain-paint-015`).
                let out_of_flow = doc.nodes[child_id]
                    .primary_styles()
                    .is_some_and(|style| style.clone_position().is_absolutely_positioned());
                if !out_of_flow {
                    doc.nodes[child_id].remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
                }
                collect_table_cells(
                    doc,
                    child_id,
                    wm,
                    is_fixed,
                    border_collapse,
                    row,
                    cursor,
                    cells,
                    oof_children,
                    rows,
                    columns,
                    first_cell_border,
                    percent_columns,
                    anonymous,
                );
            }
            if fixed.is_none() {
                doc.nodes[node_id].children = own;
            }
            if display.inside() != DisplayInside::Contents {
                cursor.end_group();
                for row in &mut rows[first_row..] {
                    row.group.get_or_insert(node_id);
                }
            }
        }
        DisplayInside::TableRow => {
            node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
            *row += 1;
            cursor.start_row();

            // (Its block size in the table's writing mode: a vertical table's row is as wide as its
            // `width`; WPT css-tables `paint/col-paint-vrl-rtl`.)
            let height = if wm.is_vertical() {
                let size = table_taffy_style(node, wm).size;
                (size.height.tag() == taffy::CompactLength::LENGTH_TAG).then(|| size.height.value())
            } else {
                physical_size(node).and_then(|size| {
                    (size.height.tag() == taffy::CompactLength::LENGTH_TAG).then(|| size.height.value())
                })
            };
            let collapsed = node.primary_styles().is_some_and(|style| {
                style.get_inherited_box().visibility == style::computed_values::visibility::T::Collapse
            });
            rows.push(TableRow {
                node_id,
                group: None,
                height,
                is_constrained: height.is_some(),
                cell_count: 0,
                columns: 0,
                collapsed,
            });
            drop_part_box(doc, node_id);
            let row_index = rows.len() - 1;
            let first_cell = cells.len();

            // pyro: what is not a cell in anonymous cells (`table_fixup`); else its own children,
            // put back below.
            let fixed = super::table_fixup::fixup_children(doc, node_id, Level::Row, wm.is_vertical(), anonymous);
            doc.nodes[node_id].remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
            let own =
                if fixed.is_none() { std::mem::take(&mut doc.nodes[node_id].children) } else { Default::default() };
            for child_id in fixed.as_deref().unwrap_or(&own[..]).iter().copied() {
                collect_table_cells(
                    doc,
                    child_id,
                    wm,
                    is_fixed,
                    border_collapse,
                    row,
                    cursor,
                    cells,
                    oof_children,
                    rows,
                    columns,
                    first_cell_border,
                    percent_columns,
                    anonymous,
                );
            }
            if fixed.is_none() {
                doc.nodes[node_id].children = own;
            }
            rows[row_index].cell_count = (cells.len() - first_cell) as u32;
            rows[row_index].columns = cursor.col;
        }
        DisplayInside::TableCell => {
            // node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
            if node.layout_child_of.get().is_none() {
                enter_layout_tree(doc, node_id);
            }
            let relative = table_cell_relative_offset(
                doc,
                node_id,
                rows.last().map(|row| (row.node_id, row.group.or_else(|| row_group_of(doc, row.node_id)))),
            );
            let node = &mut doc.nodes[node_id];
            let rowspan: u16 = node
                .attr(local_name!("rowspan"))
                .and_then(|val| val.trim().parse::<u32>().ok().map(|v| v.min(65534) as u16))
                // pyro: `rowspan="0"` spans the rest of the row group (HTML § 4.9.11; cut at
                // the group's end below), WPT `css-tables/zero-rowspan-001/002`.
                .map(|v| if v == 0 { 65534 } else { v.clamp(1, 65534) })
                .unwrap_or(1);
            let baseline =
                !wm.is_vertical() && rowspan == 1 && is_baseline_aligned(&node.primary_styles().unwrap());
            // pyro: the grid sets the offsets only in a table with baseline-aligned cells
            // (`table_has_baseline_cells`): a cell that is no longer aligned so drops its own here,
            // or it would keep it once the table has none left (or in a table without any).
            if !baseline {
                set_cell_baseline_offset(node, 0.0);
            }
            let stylo_style = &node.primary_styles().unwrap();
            let percent_width = stylo_taffy::convert::dimension(stylo_style.get_width()).tag() == taffy::CompactLength::PERCENT_TAG;
            // pyro: HTML § 4.9.11: `colspan` is 1 to 1000 (0 and junk are 1); `rowspan` 0 to
            // 65534 (0: the rest of the row group, above).
            let colspan: u16 = node
                .attr(local_name!("colspan"))
                .and_then(|val| val.trim().parse::<u32>().ok())
                .map_or(1, |v| v.clamp(1, 1000) as u16);
            let mut style = table_taffy_style(node, wm);
            // pyro: a cell `width` that stays a `calc()` (a length and a percentage) is `auto` in
            // table layout, as in Blink (WPT css-values `calc-width-table-auto-1`, `-fixed-1`).
            if !wm.is_vertical()
                && matches!(stylo_style.get_width(), style::values::computed::Size::LengthPercentage(lp)
                    if matches!(lp.0.unpack(), UnpackedLengthPercentage::Calc(_)))
            {
                style.size.width = taffy::Dimension::auto();
            }
            let col = cursor.next_free();
            // pyro: no cell reaches past the table's last column line (`PLACEMENT_LINE_LIMIT`):
            // 66 cells of `colspan="1000"` overflowed the cursor's `u16` arithmetic.
            let colspan = colspan.min(PLACEMENT_LINE_LIMIT - col).max(1);
            // pyro: relative positioning of the cell, its row and its row group, which move the
            // cell (`table_cell_relative_offset`).
            if let Some(offset) = relative.filter(|_| !wm.is_vertical()) {
                let keeps_own_x = [style.inset.left, style.inset.right]
                    .into_iter()
                    .any(|inset| inset.into_raw().tag() != taffy::CompactLength::LENGTH_TAG && !inset.is_auto());
                style.position = taffy::Position::Relative;
                style.inset.top = style_helpers::length(offset.y);
                style.inset.bottom = style_helpers::auto();
                if !keeps_own_x {
                    style.inset.left = style_helpers::length(offset.x);
                    style.inset.right = style_helpers::auto();
                }
            }

            // In the collapsed borders model the borders are laid out as gutters between
            // the cells (see the table's `gap`) rather than as part of the cells
            if border_collapse == BorderCollapse::Collapse {
                style.border = taffy::Rect::ZERO.map(style_helpers::length);
            }

            if first_cell_border.is_none() {
                *first_cell_border = Some(stylo_style.clone_border());
            }

            // In the fixed table layout algorithm the widths of columns are not
            // affected by cell contents, so cells must not impose a min-content
            // floor on their tracks.
            if is_fixed {
                style.min_size.width = style_helpers::length(0.0);
            }

            // What converts the cell's `width` to the width of its border box
            let extra = match style.box_sizing {
                taffy::BoxSizing::ContentBox => {
                    let padding = style.padding.resolve_or_zero(None, resolve_calc_value);
                    let border = style.border.resolve_or_zero(None, resolve_calc_value);
                    (padding + border).horizontal_axis_sum()
                }
                taffy::BoxSizing::BorderBox => 0.0,
            };
            // A length `width`, as the width of the cell's border box
            // (pyro: no wider than a length `max-width` says, as in Chromium.)
            let max_width = (style.max_size.width.into_raw().tag() == taffy::CompactLength::LENGTH_TAG)
                .then(|| style.max_size.width.into_raw().value());
            let clamp_width = |width: f32| max_width.map_or(width, |max| width.min(max));
            let length_width = style.size.width.into_option().map(|len| clamp_width(len) + extra);

            // Column widths from `<col>` elements take precedence over cell widths
            let col_needs_width =
                (col as usize) >= columns.len() || columns[col as usize].max.is_auto();
            // pyro: in the automatic layout a cell spanning columns gives none of them its width: its
            // width is a minimum of the columns and the spacing it spans (`TableCell::min_width`).
            let spans_auto = colspan > 1 && !is_fixed;
            if *row == 1 && col_needs_width && !spans_auto {
                let column: TrackSizingFunction = match style.size.width.tag() {
                    taffy::CompactLength::LENGTH_TAG => {
                        style_helpers::length(clamp_width(style.size.width.value()) + extra)
                    }
                    taffy::CompactLength::PERCENT_TAG => {
                        if is_fixed {
                            // Resolved in `build_table_context` once the column count is known
                            percent_columns.push((col, style.size.width.value(), extra));
                            style_helpers::percent(style.size.width.value())
                        } else {
                            style_helpers::auto()
                        }
                    }
                    taffy::CompactLength::AUTO_TAG => style_helpers::auto(),
                    // Dimension values are always length, percentage, auto or calc(),
                    // so any other tag is a calc() value. Pass it through so that
                    // Taffy resolves it against the table's inner width.
                    _ => style.size.width.into(),
                };
                if (col as usize) < columns.len() {
                    if !column.max.is_auto() {
                        columns[col as usize] = column;
                    }
                } else {
                    columns.resize(col as usize, style_helpers::auto());
                    columns.push(column);
                }
            }

            // The margin properties do not apply to table-internal elements
            style.margin = taffy::Rect::ZERO.map(style_helpers::length);

            // pyro: the column the HTML table model gives the cell (`ColumnCursor`: the first slot of
            // its row that no cell covers), whatever its span then overlaps (WPT css-tables
            // `tentative/border-collapse-spanning-cells-004`). The grid's `row dense` auto-placement
            // moved such a cell past the cells it would overlap. (`colspan` 0, which is no span the
            // table places, is left to it.)
            let start = if colspan > 0 && col < PLACEMENT_LINE_LIMIT {
                style_helpers::line(col as i16 + 1)
            } else {
                style_helpers::auto()
            };
            style.grid_column = taffy::Line { start, end: style_helpers::span(colspan) };
            style.grid_row = taffy::Line {
                start: style_helpers::line(*row as i16),
                end: style_helpers::span(rowspan),
            };
            if rowspan == 1 && style.size.height.tag() == taffy::CompactLength::LENGTH_TAG {
                if let Some(row) = rows.last_mut() {
                    row.is_constrained = true;
                }
            }

            // `box-sizing: content-box` (the initial value) adds the cell's
            // vertical padding and border to its height, as in Chromium.
            let content_box_extra = {
                use taffy::ResolveOrZero as _;
                match style.box_sizing {
                    taffy::BoxSizing::ContentBox => {
                        let padding = style.padding.resolve_or_zero(None, resolve_calc_value);
                        let border = style.border.resolve_or_zero(None, resolve_calc_value);
                        padding.top + padding.bottom + border.top + border.bottom
                    }
                    taffy::BoxSizing::BorderBox => 0.0,
                }
            };
            let height_base = style
                .size
                .height
                .into_option()
                .map(|height| (height + content_box_extra, style.box_sizing == taffy::BoxSizing::ContentBox));
            let min_height = style.size.height.into_option().map(|height| {
                let (top, bottom) = match border_collapse {
                    BorderCollapse::Collapse => first_cell_border.as_ref().map_or((0.0, 0.0), |border| {
                        (
                            side_width(border.border_top_width.0, border.border_top_style),
                            side_width(border.border_bottom_width.0, border.border_bottom_style),
                        )
                    }),
                    BorderCollapse::Separate => (0.0, 0.0),
                };
                // A content-box cell in a collapsed table also has half of
                // each collapsed border inside its border box.
                let collapsed_half = match style.box_sizing {
                    taffy::BoxSizing::ContentBox => (top + bottom) / 2.0,
                    taffy::BoxSizing::BorderBox => 0.0,
                };
                let height = height + content_box_extra + collapsed_half;
                let gutter = top.max(bottom);
                (height - gutter).max(0.0)
            });
            if min_height.is_some() {
                style.size.height = style_helpers::auto();
            }
            style.size.width = style_helpers::auto();
            // pyro: `max-width` does not apply to a table cell (CSS 2.1 leaves it undefined;
            // Chromium ignores it): the cell spans its column, as wide as the column grew.
            style.max_size.width = style_helpers::auto();
            // pyro: nor does `max-height` (Chromium; WPT css-tables `table-cell-overflow-explicit-height-002`).
            style.max_size.height = style_helpers::auto();
            crate::resolve::phase_profile::table_cell_collected();
            let length_width = if is_fixed { None } else { length_width };
            let percent_lengths = percent_width
                || [style.padding.left, style.padding.right, style.padding.top, style.padding.bottom]
                    .into_iter()
                    .any(|length| resolves_against_parent_width(length.into_raw()))
                || [style.margin.left, style.margin.right, style.margin.top, style.margin.bottom]
                    .into_iter()
                    .any(|length| resolves_against_parent_width(length.into_raw()))
                || [style.min_size.width, style.max_size.width]
                    .into_iter()
                    .any(|length| resolves_against_parent_width(length.into_raw()));
            cells.push(TableCell {
                node_id,
                row: row.saturating_sub(1),
                column: col,
                colspan,
                rowspan,
                percent_width,
                percent_lengths,
                min_width: length_width,
                min_height,
                height_base,
                length_width,
                min_width_base: length_width,
                min_height_base: min_height,
                baseline,
                style,
            });

            cursor.place(colspan, rowspan);
        }
        DisplayInside::Flow
        | DisplayInside::FlowRoot
        | DisplayInside::Flex
        | DisplayInside::Grid
        | DisplayInside::GridLanes
        // pyro: never computed (Stylo's `adjust_for_webkit_box`).
        | DisplayInside::WebkitBox => {
            node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
            // Probably a table caption: ignore
            // println!(
            //     "Warning: ignoring non-table typed descendent of table ({:?})",
            //     display.inside()
            // );
        }
        DisplayInside::TableColumnGroup | DisplayInside::TableColumn | DisplayInside::Table => {
            node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
            //Ignore
        }
        DisplayInside::None => {
            node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
            // Ignore
        }
    }
}

pub struct RangeIter(Range<usize>);

impl Iterator for RangeIter {
    type Item = taffy::NodeId;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(taffy::NodeId::from)
    }
}

impl taffy::TraversePartialTree for TableTreeWrapper<'_, '_> {
    type ChildIter<'a>
        = RangeIter
    where
        Self: 'a;

    #[inline(always)]
    fn child_ids(&self, _node_id: taffy::NodeId) -> Self::ChildIter<'_> {
        RangeIter(0..self.ctx.cells.len())
    }

    #[inline(always)]
    fn child_count(&self, _node_id: taffy::NodeId) -> usize {
        self.ctx.cells.len()
    }

    #[inline(always)]
    fn get_child_id(&self, _node_id: taffy::NodeId, index: usize) -> taffy::NodeId {
        index.into()
    }
}
impl taffy::TraverseTree for TableTreeWrapper<'_, '_> {}

impl taffy::LayoutPartialTree for TableTreeWrapper<'_, '_> {
    type CoreContainerStyle<'a>
        = &'a taffy::Style<Atom>
    where
        Self: 'a;

    type CustomIdent = Atom;

    fn get_core_container_style(&self, _node_id: taffy::NodeId) -> &taffy::Style<Atom> {
        self.style.as_ref().unwrap_or(&self.ctx.style)
    }

    fn resolve_calc_value(&self, calc_ptr: *const (), parent_size: f32) -> f32 {
        resolve_calc_value(calc_ptr, parent_size)
    }

    fn set_unrounded_layout(&mut self, node_id: taffy::NodeId, layout: &taffy::Layout) {
        let index = usize::from(node_id);
        self.ctx.placed.borrow_mut().placed(index, self.ctx.cells.len());
        let node_id = crate::taffy_node_id(self.ctx.cells[index].node_id);
        self.doc.set_unrounded_layout(node_id, layout)
    }

    fn compute_child_layout(
        &mut self,
        node_id: taffy::NodeId,
        inputs: taffy::tree::LayoutInput,
    ) -> taffy::LayoutOutput {
        let ctx = Arc::clone(&self.ctx);
        let cell = &ctx.cells[usize::from(node_id)];
        let node_id = crate::taffy_node_id(cell.node_id);
        // pyro: unless the table's height or the row's is definite, the cell's height (stretched to
        // its row) is no basis for its children's percentages, nor the table for the cell's own
        // (`Self::definite_height`).
        let (mut inputs, own_basis) = self.cell_basis_inputs(cell, inputs);
        // pyro: a cell's percentage padding resolves against the table's width less its outer
        // border spacing (the grid's), once that is known (CSS Tables 3, as Blink and Gecko; it
        // resolved against the cell's columns; WPT css-tables `tentative/padding-percentage`).
        // In the column widths it resolves against zero (`percent_lengths`).
        if inputs.known_dimensions.width.is_some() && inputs.parent_size.width.is_some() {
            if let Some(grid_width) = self.grid_inline_size().filter(|_| self.has_percent_padding(cell)) {
                inputs.parent_size.width = Some(grid_width);
                // (Taffy's layout cache does not key on `parent_size`: a layout of the cell
                // against its area's width would come back.)
                use taffy::CacheTree as _;
                self.doc.cache_clear(node_id);
            }
        }
        let outer_basis = std::mem::replace(&mut self.doc.cell_percent_height, own_basis);
        let outer_restricted = std::mem::replace(&mut self.doc.restricted_cell, self.restricted.then_some(cell.node_id));
        let output = self.lay_out_cell(cell, node_id, inputs);
        self.doc.cell_percent_height = outer_basis;
        self.doc.restricted_cell = outer_restricted;
        output
    }
}

impl TableTreeWrapper<'_, '_> {
    /// pyro: the table's grid layout at `inputs`, with `grid_width` known while it runs.
    pub(crate) fn layout_grid(&mut self, node_id: taffy::NodeId, inputs: taffy::tree::LayoutInput) -> taffy::LayoutOutput {
        let grid_width = {
            use taffy::MaybeResolve as _;
            let style = self.style.as_ref().unwrap_or(&self.ctx.style);
            let parent = inputs.parent_size.width;
            let inset = style.padding.resolve_or_zero(parent, resolve_calc_value).horizontal_axis_sum()
                + style.border.resolve_or_zero(parent, resolve_calc_value).horizontal_axis_sum();
            // Its known width, else a fixed table's own length `width` (the grid's then; an auto
            // table's columns may make it wider: `grid_inline_size` sums them).
            match inputs.known_dimensions.width {
                Some(width) => Some((width - inset).max(0.0)),
                None if !self.ctx.is_fixed => None,
                None => style.size.width.maybe_resolve(parent, resolve_calc_value).map(|width| match style.box_sizing {
                    taffy::BoxSizing::ContentBox => width.max(0.0),
                    taffy::BoxSizing::BorderBox => (width - inset).max(0.0),
                }),
            }
        };
        let outer = std::mem::replace(&mut self.grid_width, grid_width);
        let output = taffy::compute_grid_layout(self, node_id, inputs);
        self.grid_width = outer;
        output
    }

    /// pyro: the width of the table's grid (its columns and the spacing between them): the one
    /// the running layout knows (`grid_width`: a fixed table's columns are `minmax(W, W fr)`
    /// tracks), else when every column has a length width.
    fn grid_inline_size(&self) -> Option<f32> {
        if let Some(width) = self.grid_width {
            return Some(width);
        }
        let style = self.style.as_ref().unwrap_or(&self.ctx.style);
        let mut width = 0.0;
        let mut n = 0;
        for column in &style.grid_template_columns {
            let taffy::GridTemplateComponent::Single(track) = column else { return None };
            let (min, max) = (track.min.into_raw(), track.max.into_raw());
            if min.tag() != taffy::CompactLength::LENGTH_TAG || max.tag() != taffy::CompactLength::LENGTH_TAG {
                return None;
            }
            width += max.value().max(min.value());
            n += 1;
        }
        let gap = style.gap.width.into_raw();
        let gap = if gap.tag() == taffy::CompactLength::LENGTH_TAG { gap.value() } else { 0.0 };
        (n > 0).then(|| width + gap * (n - 1) as f32)
    }

    /// pyro: whether the cell's padding has a percentage (in `calc()` too).
    fn has_percent_padding(&self, cell: &TableCell) -> bool {
        let padding = &cell.style.padding;
        [padding.left, padding.right, padding.top, padding.bottom].iter().any(|p| p.into_raw().uses_percentage())
    }

    /// The inputs `cell` is laid out with, and the basis of its children's percentage heights
    /// (`Self::definite_height`, `LayoutPassState::cell_percent_height`).
    fn cell_basis_inputs(
        &self,
        cell: &TableCell,
        mut inputs: taffy::tree::LayoutInput,
    ) -> (taffy::tree::LayoutInput, Option<(NodeId, f32)>) {
        let mut own_basis = None;
        if !self.definite_height && self.ctx.rows.get(cell.row as usize).is_none_or(|row| row.height.is_none()) {
            inputs.known_dimensions_are_definite.height = false;
            inputs.parent_size.height = None;
            // Its own length `height` (border box) is (the layout style has none: `stylo_data`).
            own_basis = cell.height_base.map(|(height, _)| (cell.node_id, height));
        }
        (inputs, own_basis)
    }

    fn lay_out_cell(&mut self, cell: &TableCell, node_id: taffy::NodeId, inputs: taffy::tree::LayoutInput) -> taffy::LayoutOutput {
        // Cell contents don't size the columns of a fixed table.
        if self.ctx.is_fixed && inputs.known_dimensions.width.is_none() {
            let mut output = self.doc.compute_child_layout(node_id, inputs);
            output.size.width = 0.0;
            output.size.height = output.size.height.max(cell.min_height.unwrap_or(0.0));
            return output;
        }

        // Measuring the intrinsic width of a cell with a minimum width (see
        // `TableCell::min_width`): the cell's own `width` would fix the width of its box,
        // so it is ignored to measure the contents.
        if let (Some(min_width), None) = (cell.min_width, inputs.known_dimensions.width) {
            let inputs = taffy::tree::LayoutInput {
                sizing_mode: taffy::SizingMode::ContentSize,
                available_space: taffy::Size {
                    width: taffy::AvailableSpace::MinContent,
                    ..inputs.available_space
                },
                ..inputs
            };
            let mut output = self.doc.compute_child_layout(node_id, inputs);
            // Its own `min-width` still applies (and wins over `max-width`).
            let style = &cell.style;
            let parent = inputs.parent_size.width;
            let own_min = cell_own_min_width(style, parent);
            output.size.width = output.size.width.max(min_width).max(own_min.unwrap_or(0.0));
            output.size.height = output.size.height.max(cell.min_height.unwrap_or(0.0));
            return output;
        }
        let mut output = self.doc.compute_child_layout(node_id, inputs);
        output.size.height = output.size.height.max(cell.min_height.unwrap_or(0.0));
        output
    }
}

impl taffy::LayoutGridContainer for TableTreeWrapper<'_, '_> {
    type GridContainerStyle<'a>
        = &'a taffy::Style<Atom>
    where
        Self: 'a;

    type GridItemStyle<'a>
        = &'a taffy::Style<Atom>
    where
        Self: 'a;

    fn get_grid_container_style(&self, node_id: taffy::NodeId) -> Self::GridContainerStyle<'_> {
        self.get_core_container_style(node_id)
    }

    fn get_grid_child_style(&self, child_node_id: taffy::NodeId) -> Self::GridItemStyle<'_> {
        &self.ctx.cells[usize::from(child_node_id)].style
    }

    fn grid_rows_are_table_rows(&self, _node_id: taffy::NodeId) -> bool {
        true
    }

    fn take_table_grid_state(&mut self, _node_id: taffy::NodeId) -> Option<taffy::TableGridReuse> {
        // pyro: a table with captions moves its cells below the captions above it after each
        // layout of its grid (`lay_out_table_captions`), which a layout that stood would not redo.
        // (A table with cells aligned on their rows' baselines is laid out again whole once one of
        // its cells or rows changed, which may move the others' baselines: its last layout serves
        // only while nothing changed, when it stands.)
        if self.ctx.baseline_cells
            && (!self.ctx.changed_cells.borrow().is_empty() || self.ctx.grid_splice.borrow().is_some())
        {
            *self.ctx.grid_state.borrow_mut() = None;
            *self.ctx.grid_splice.borrow_mut() = None;
            self.ctx.changed_cells.borrow_mut().clear();
            return None;
        }
        if !self.ctx.captions.is_empty() {
            return None;
        }
        let mut changed = std::mem::take(&mut *self.ctx.changed_cells.borrow_mut());
        let splice = self.ctx.grid_splice.borrow_mut().take();
        let state = self.ctx.grid_state.borrow_mut().take()?;
        changed.sort_unstable();
        changed.dedup();
        Some(taffy::TableGridReuse { state, changed, splice })
    }

    fn store_table_grid_state(&mut self, _node_id: taffy::NodeId, state: Box<taffy::TableGridState>) {
        if !self.ctx.captions.is_empty() {
            return;
        }
        // (A layout that stood as it was, with only the available width changed, laid out nothing.)
        if !state.stood() {
            crate::resolve::phase_profile::table_grid_laid_out(state.laid_out_again());
        }
        *self.ctx.grid_state.borrow_mut() = Some(state);
    }

    fn table_cells_narrower_than_padding(&self, _node_id: taffy::NodeId) -> bool {
        self.ctx.is_fixed
    }

    fn table_has_baseline_cells(&self, _node_id: taffy::NodeId) -> bool {
        self.ctx.baseline_cells
    }

    fn table_cell_is_baseline_aligned(&self, _node_id: taffy::NodeId, child_node_id: taffy::NodeId) -> bool {
        self.ctx.cells[usize::from(child_node_id)].baseline
    }

    fn set_table_cell_baseline_offset(&mut self, _node_id: taffy::NodeId, child_node_id: taffy::NodeId, offset: f32) {
        let cell = self.ctx.cells[usize::from(child_node_id)].node_id;
        set_cell_baseline_offset(&mut self.doc.nodes[cell], offset);
    }

    fn table_cell_baseline(
        &mut self,
        _node_id: taffy::NodeId,
        child_node_id: taffy::NodeId,
        inputs: taffy::LayoutInput,
    ) -> Option<f32> {
        // (Its own layout, without the minimum height its `height` gives it in its row: an empty
        // cell has no content however tall it is.)
        let ctx = Arc::clone(&self.ctx);
        let (inputs, own_basis) = self.cell_basis_inputs(&ctx.cells[usize::from(child_node_id)], inputs);
        let cell = ctx.cells[usize::from(child_node_id)].node_id;
        let outer_basis = std::mem::replace(&mut self.doc.cell_percent_height, own_basis);
        let outer_restricted = std::mem::replace(&mut self.doc.restricted_cell, self.restricted.then_some(cell));
        let output = self.doc.compute_child_layout(crate::taffy_node_id(cell), inputs);
        self.doc.cell_percent_height = outer_basis;
        self.doc.restricted_cell = outer_restricted;
        let node = &self.doc.nodes[cell];
        // A layout run since the cell's last final layout has its content that much lower.
        let offset = if node.flags.contains(NodeFlags::HAS_TABLE_BASELINE_OFFSET) {
            node.layout_data().table_baseline_offset
        } else {
            0.0
        };
        if let Some(baseline) = output.baselines.first {
            return Some(baseline - offset);
        }
        // No line box: the bottom of its content box, when it has in-flow content (a cell without any,
        // however tall, takes no part in its row's baseline, as in Chromium; WPT css-align
        // `baseline-rules/synthesized-baseline-table-cell-001`).
        // (Its line boxes count: a layout-contained cell with text has no baseline but has content,
        // WPT css-contain `contain-layout-baseline-004`.)
        let has_lines = node
            .element_data()
            .and_then(|element| element.inline_layout_data.as_ref())
            .is_some_and(|layout| layout.layout.lines().next().is_some());
        let has_content = has_lines
            || node.layout_children.borrow().as_ref().is_some_and(|children| {
                children.iter().any(|&child| {
                    let child = &self.doc.nodes[child];
                    !child.is_out_of_flow() && !child.is_whitespace_node()
                })
            });
        if !has_content {
            return None;
        }
        use taffy::CoreStyle as _;
        let style = node.layout_style();
        let parent = inputs.parent_size.width;
        let padding = style.padding().resolve_or_zero(parent, resolve_calc_value);
        let border = style.border().resolve_or_zero(parent, resolve_calc_value);
        let height = output.size.height;
        let content = height - padding.top - padding.bottom - border.top - border.bottom;
        (content > 0.0).then(|| height - padding.bottom - border.bottom)
    }

    fn track_counts(&self, _node_id: taffy::NodeId) -> Option<taffy::Size<u16>> {
        #[cfg(test)]
        if tests::GRID_PLACES_CELLS.with(|grid_places| grid_places.get()) {
            return None;
        }
        self.ctx.placement.as_ref().map(|placement| placement.extent)
    }

    fn resolved_placement(
        &self,
        _node_id: taffy::NodeId,
        child_node_id: taffy::NodeId,
    ) -> Option<taffy::Size<taffy::Line<u16>>> {
        let placement = self.ctx.placement.as_ref()?;
        placement.areas.get(usize::from(child_node_id)).copied()
    }

    fn set_detailed_grid_info(
        &mut self,
        _node_id: taffy::NodeId,
        detailed_grid_info: DetailedGridInfo<Atom>,
    ) {
        *self.ctx.computed_grid_info.borrow_mut() = Some(detailed_grid_info);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::TableContext;
    use crate::node::SpecialElementData;
    use crate::{Attribute, BaseDocument, DocumentConfig, qual_name};
    use blitz_traits::shell::{ColorScheme, Viewport};
    use std::cell::Cell;
    use std::sync::Arc;
    use stylo_taffy::CopyStyleGetters;

    thread_local! {
        /// Set by the tests to have the grid place a table's cells itself, as without
        /// `LayoutGridContainer::track_counts`, to compare with the table's own placement.
        pub(crate) static GRID_PLACES_CELLS: Cell<bool> = const { Cell::new(false) };
    }

    /// A table: its style, its `<col span>`s, and its row groups, each a display, a style and rows of
    /// cells, a cell being `(colspan, rowspan, extra style)`.
    type Spec = (&'static str, &'static [u16], &'static [(&'static str, &'static [&'static [(u16, u16, &'static str)]])]);

    const TABLES: &[Spec] = &[
        // A rowspan hole that a colspan cell does not fit beside (the cursor and dense placement differ)
        ("", &[], &[("table-row-group", &[&[(1, 1, ""), (1, 2, ""), (1, 1, "")], &[(2, 1, ""), (1, 1, "")]])]),
        // Rowspans of several rows, leaving holes that later cells fill
        ("", &[], &[("table-row-group", &[
            &[(1, 3, ""), (1, 1, ""), (1, 2, ""), (1, 1, "")],
            &[(1, 1, ""), (1, 1, "")],
            &[(2, 1, ""), (1, 1, ""), (1, 1, "")],
            &[(1, 1, ""), (3, 1, "")],
        ])]),
        // A rowspan past its row group stops at the group's end; thead and tfoot are reordered
        ("border-collapse:collapse", &[], &[
            ("table-footer-group", &[&[(1, 1, ""), (2, 1, "")]]),
            ("table-row-group", &[&[(1, 5, ""), (1, 1, ""), (1, 1, "")], &[(1, 1, ""), (1, 2, "")]]),
            ("table-header-group", &[&[(3, 1, "")], &[(1, 2, ""), (1, 1, "")]]),
        ]),
        // `<col span>` gives more columns than the cells use, and a wide colspan more than the cols
        ("table-layout:fixed;width:300px", &[3, 1], &[("table-row-group", &[&[(1, 1, ""), (2, 1, "")], &[(6, 1, "")], &[(1, 2, ""), (1, 1, "")]])]),
        // Out-of-flow and display:none cells take no grid area
        ("position:relative", &[], &[("table-row-group", &[
            &[(1, 1, ""), (1, 1, "position:absolute"), (1, 2, "")],
            &[(1, 1, "display:none"), (1, 1, ""), (2, 1, "")],
        ])]),
        // A vertical table
        ("writing-mode:vertical-rl", &[], &[("table-row-group", &[&[(1, 2, ""), (2, 1, "")], &[(1, 1, ""), (1, 1, "")]])]),
        // colspan=0 is colspan=1 (HTML § 4.9.11)
        ("", &[], &[("table-row-group", &[&[(0, 1, ""), (1, 1, "")], &[(1, 1, ""), (1, 1, "")]])]),
    ];

    fn build(grid_places_cells: bool) -> BaseDocument {
        GRID_PLACES_CELLS.with(|grid_places| grid_places.set(grid_places_cells));
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        let root_id = doc.root_node().id;
        let attr = |name: &str, value: &str| Attribute {
            name: match name {
                "style" => qual_name!("style"),
                "span" => qual_name!("span"),
                "colspan" => qual_name!("colspan"),
                _ => qual_name!("rowspan"),
            },
            value: value.to_string(),
        };
        let mut mutator = doc.mutate();
        let html = mutator.create_element(qual_name!("html"), vec![]);
        let body = mutator.create_element(qual_name!("body"), vec![]);
        for (table_style, cols, groups) in TABLES {
            let table = mutator.create_element(qual_name!("table"), vec![attr("style", &format!("display:table;{table_style}"))]);
            if !cols.is_empty() {
                let colgroup = mutator.create_element(qual_name!("colgroup"), vec![attr("style", "display:table-column-group")]);
                for span in cols.iter() {
                    let col = mutator.create_element(
                        qual_name!("col"),
                        vec![attr("style", "display:table-column;width:20px"), attr("span", &span.to_string())],
                    );
                    mutator.append_children(colgroup, &[col]);
                }
                mutator.append_children(table, &[colgroup]);
            }
            for (group_display, rows) in groups.iter() {
                let group = mutator.create_element(qual_name!("tbody"), vec![attr("style", &format!("display:{group_display}"))]);
                for (r, cells) in rows.iter().enumerate() {
                    let tr = mutator.create_element(qual_name!("tr"), vec![attr("style", "display:table-row")]);
                    for (c, (colspan, rowspan, cell_style)) in cells.iter().enumerate() {
                        let td = mutator.create_element(
                            qual_name!("td"),
                            vec![
                                attr("style", &format!("display:table-cell;border:1px solid;{cell_style}")),
                                attr("colspan", &colspan.to_string()),
                                attr("rowspan", &rowspan.to_string()),
                            ],
                        );
                        // Different content widths, so that a misplaced cell changes the columns
                        let text = mutator.create_text_node(&"x".repeat(1 + (r * 3 + c) % 5));
                        mutator.append_children(td, &[text]);
                        mutator.append_children(tr, &[td]);
                    }
                    mutator.append_children(group, &[tr]);
                }
                mutator.append_children(table, &[group]);
            }
            mutator.append_children(body, &[table]);
        }
        mutator.append_children(html, &[body]);
        mutator.append_children(root_id, &[html]);
        drop(mutator);
        doc.resolve(0.0);
        GRID_PLACES_CELLS.with(|grid_places| grid_places.set(false));
        doc
    }

    fn tables(doc: &BaseDocument) -> Vec<Arc<TableContext>> {
        doc.nodes
            .iter()
            .filter_map(|(_, node)| match &node.element_data()?.special_data {
                SpecialElementData::TableRoot(ctx) => Some(Arc::clone(ctx)),
                _ => None,
            })
            .collect()
    }

    /// Rows appended, inserted and removed through `update_rows` (the table is not built again) keep
    /// `TableContext::placement` what `place_cells` gives for the cells as they are then.
    #[test]
    fn updated_rows_keep_the_cells_placement() {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        let root_id = doc.root_node().id;
        let style = |value: &str| Attribute { name: qual_name!("style"), value: value.to_string() };
        let mut mutator = doc.mutate();
        let html = mutator.create_element(qual_name!("html"), vec![]);
        let body = mutator.create_element(qual_name!("body"), vec![]);
        let table = mutator.create_element(qual_name!("table"), vec![style("display:table")]);
        let tbody = mutator.create_element(qual_name!("tbody"), vec![style("display:table-row-group")]);
        let row = |mutator: &mut crate::DocumentMutator, cells: &[u16]| {
            let tr = mutator.create_element(qual_name!("tr"), vec![style("display:table-row")]);
            for (c, colspan) in cells.iter().enumerate() {
                let td = mutator.create_element(
                    qual_name!("td"),
                    vec![
                        style("display:table-cell;border:1px solid"),
                        Attribute { name: qual_name!("colspan"), value: colspan.to_string() },
                    ],
                );
                let text = mutator.create_text_node(&"x".repeat(1 + c % 3));
                mutator.append_children(td, &[text]);
                mutator.append_children(tr, &[td]);
            }
            tr
        };
        let rows: Vec<_> = [&[1, 1, 1][..], &[1, 2], &[1, 1, 1], &[2, 1]].iter().map(|cells| row(&mut mutator, cells)).collect();
        mutator.append_children(tbody, &rows);
        mutator.append_children(table, &[tbody]);
        mutator.append_children(body, &[table]);
        mutator.append_children(html, &[body]);
        mutator.append_children(root_id, &[html]);
        drop(mutator);
        doc.resolve(0.0);
        let check = |doc: &BaseDocument, what: &str| {
            let ctx = match &doc.nodes[table].element_data().unwrap().special_data {
                SpecialElementData::TableRoot(ctx) => Arc::clone(ctx),
                _ => panic!("no table context {what}"),
            };
            assert!(ctx.placement.is_some(), "{what}");
            assert_eq!(ctx.placement, super::place_cells(&ctx.cells), "{what}");
            assert_eq!(ctx.placement.as_ref().unwrap().extent, taffy::Size { width: 3, height: ctx.rows.len() as u16 }, "{what}");
        };
        check(&doc, "built");
        let built = crate::resolve::phase_profile::tables().0;

        let mut mutator = doc.mutate();
        let tr = row(&mut mutator, &[1, 1, 1]);
        mutator.append_children(tbody, &[tr]);
        drop(mutator);
        doc.resolve(0.0);
        check(&doc, "an append");

        let third = doc.nodes[tbody].children[2];
        let mut mutator = doc.mutate();
        let inserted = [row(&mut mutator, &[3]), row(&mut mutator, &[1, 2])];
        mutator.insert_nodes_before(third, &inserted);
        drop(mutator);
        doc.resolve(0.0);
        check(&doc, "an insert");

        let second = doc.nodes[tbody].children[1];
        let mut mutator = doc.mutate();
        mutator.remove_node(second);
        drop(mutator);
        doc.resolve(0.0);
        check(&doc, "a removal");
        assert_eq!(crate::resolve::phase_profile::tables().0, built, "the table was updated, not built again");
    }

    /// The row-dense placement over a matrix of occupied cells, one column at a time, as the grid's
    /// auto-placement defines it: what `OccupiedColumns::place` must give.
    fn place_in_matrix(matrix: &mut Vec<Vec<bool>>, rows: std::ops::Range<u16>, colspan: u16) -> std::ops::Range<u16> {
        let (rows, colspan) = (rows.start as usize..rows.end as usize, colspan as usize);
        if matrix.len() < rows.end {
            matrix.resize_with(rows.end, Vec::new);
        }
        let free = |matrix: &Vec<Vec<bool>>, c: usize| rows.clone().all(|r| !matrix[r].get(c).copied().unwrap_or(false));
        let c0 = (0..).find(|&c0| (c0..c0 + colspan).all(|c| free(matrix, c))).unwrap();
        for r in rows.clone() {
            if matrix[r].len() < c0 + colspan {
                matrix[r].resize(c0 + colspan, false);
            }
            matrix[r][c0..c0 + colspan].fill(true);
        }
        c0 as u16..(c0 + colspan) as u16
    }

    /// `OccupiedColumns` places cells where the matrix placement does, over random tables of rowspans and
    /// colspans that leave holes for later cells to backfill.
    #[test]
    fn occupied_columns_place_as_the_matrix() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n) as u16
        };
        for _ in 0..300 {
            let (mut occupied, mut matrix) = (super::OccupiedColumns::default(), Vec::new());
            let row_count = 1 + next(8);
            for row in 0..row_count {
                for _ in 0..next(7) {
                    let rows = row..row + 1 + next(4);
                    let colspan = 1 + next(3);
                    assert_eq!(occupied.place(rows.clone(), colspan), Some(place_in_matrix(&mut matrix, rows, colspan)));
                }
            }
            // Each row's ranges are disjoint, in column order, and not adjacent (merged)
            for row in &occupied.rows {
                assert!(row.windows(2).all(|pair| pair[0].end < pair[1].start), "{row:?}");
            }
        }
    }

    /// A row of contiguous cells is one occupied range, so placing its k-th cell is one jump rather than a
    /// scan over the k earlier cells per column stepped (cubic in the row's cells), and a wide row is placed
    /// in linear-logarithmic work.
    #[test]
    fn occupied_columns_merge_contiguous_cells() {
        let mut occupied = super::OccupiedColumns::default();
        for column in 0..5000u16 {
            assert_eq!(occupied.place(0..1, 1), Some(column..column + 1));
        }
        assert_eq!(occupied.rows, vec![vec![0..5000]]);
        // A rowspan hole beside it: the next row's cells fill around it and merge across it once filled
        let mut occupied = super::OccupiedColumns::default();
        assert_eq!(occupied.place(0..2, 1), Some(0..1));
        assert_eq!(occupied.place(0..1, 1), Some(1..2));
        assert_eq!(occupied.place(0..2, 1), Some(2..3));
        assert_eq!(occupied.place(1..2, 1), Some(1..2));
        assert_eq!(occupied.rows, vec![vec![0..3], vec![0..3]]);
        assert_eq!(occupied.place(1..2, 10_000), None);
    }

    /// The table's own cell placement (`place_cells`) is what the grid's placement gives: the grid areas
    /// and track counts the grid records, and every box's layout, are the same when the grid places the
    /// cells itself. Covers rowspan holes beside colspans, rowspans clamped to their group, thead/tfoot
    /// reordering, `<col span>`, out-of-flow and `display:none` cells, a vertical table, and the fallback
    /// to the grid for a span the table does not place. (Debug builds also assert it on every grid layout.)
    /// pyro: a grid line's segments as runs (`EdgeLine`): offers over ranges split and join runs,
    /// clearing a span costs its runs, not its length, and a splice moves the runs after it.
    #[test]
    fn edge_line_runs() {
        use super::{CollapsedEdge, EdgeLine};
        let edge = |width: f32| CollapsedEdge { width, ..CollapsedEdge::NONE };
        let mut line = EdgeLine::default();
        line.update(0..1_000_000, |_| edge(2.0));
        assert_eq!(line.runs.len(), 1);
        line.update(10..20, |e| if e.width < 5.0 { edge(5.0) } else { e });
        assert_eq!(line.runs.iter().map(|r| (r.0, r.1)).collect::<Vec<_>>(), [(0, 10), (10, 20), (20, 1_000_000)]);
        assert_eq!(line.get(15).width, 5.0);
        assert_eq!(line.get(25).width, 2.0);
        line.update(10..20, |_| edge(2.0));
        assert_eq!(line.runs.len(), 1, "equal neighbours join");
        line.update(1..999_999, |_| CollapsedEdge::NONE);
        assert_eq!(line.runs.len(), 2);
        assert_eq!(line.get(500).width, 0.0);
        line.splice(0..1, 3);
        assert_eq!(line.get(2).width, 0.0);
        assert_eq!(line.get(1_000_001).width, 2.0);
        assert!(line.all_width(1_000_002, 2.0) == false);
    }

    /// pyro: offers of alternating edges, segment by segment, cost linear time: runs join only at
    /// the edges of the range offered (the whole line was walked per offer).
    #[test]
    fn edge_line_alternating_offers_are_linear() {
        use super::{CollapsedEdge, EdgeLine};
        let edge = |width: f32| CollapsedEdge { width, ..CollapsedEdge::NONE };
        let mut line = EdgeLine::default();
        let start = std::time::Instant::now();
        for c in 0..100_000 {
            line.update(c..c + 1, |_| edge(if c % 2 == 0 { 1.0 } else { 2.0 }));
        }
        assert_eq!(line.runs.len(), 100_000);
        assert!(start.elapsed().as_secs_f64() < 2.0, "took {:?}", start.elapsed());
    }

    #[test]
    fn table_cell_placement_is_the_grids() {
        let (own, grid) = (build(false), build(true));
        let (own_tables, grid_tables) = (tables(&own), tables(&grid));
        assert_eq!(own_tables.len(), TABLES.len());
        assert_eq!(grid_tables.len(), TABLES.len());
        let mut placed = 0;
        for (own_ctx, grid_ctx) in own_tables.iter().zip(grid_tables.iter()) {
            let (own_info, grid_info) = (own_ctx.computed_grid_info.borrow(), grid_ctx.computed_grid_info.borrow());
            let (own_info, grid_info) = (own_info.as_ref().unwrap(), grid_info.as_ref().unwrap());
            assert_eq!(own_info, grid_info);
            if let Some(placement) = &own_ctx.placement {
                placed += 1;
                let lines = |line: taffy::Line<u16>| (line.start + 1, line.end + 1);
                let areas: Vec<_> = placement.areas.iter().map(|area| (lines(area.height), lines(area.width))).collect();
                let in_flow: Vec<_> = own_ctx
                    .cells
                    .iter()
                    .zip(areas)
                    .filter(|(cell, _)| {
                        own.nodes[cell.node_id].primary_styles().is_some_and(|style| {
                            !style.clone_position().is_absolutely_positioned()
                                && style.clone_display() != style::values::specified::box_::Display::None
                        })
                    })
                    .map(|(_, area)| area)
                    .collect();
                let recorded: Vec<_> = own_info
                    .items
                    .iter()
                    .map(|item| ((item.row_start, item.row_end), (item.column_start, item.column_end)))
                    .collect();
                assert_eq!(in_flow, recorded);
                for (tracks, extent) in [(&own_info.columns, placement.extent.width), (&own_info.rows, placement.extent.height)] {
                    assert_eq!(tracks.negative_implicit_tracks, 0);
                    assert_eq!(tracks.explicit_tracks + tracks.positive_implicit_tracks, tracks.explicit_tracks.max(extent));
                }
            }
        }
        // Every table is placed (colspan=0 is colspan=1)
        assert_eq!(placed, TABLES.len());
        assert_eq!(own.nodes.len(), grid.nodes.len());
        for ((id, own_node), (_, grid_node)) in own.nodes.iter().zip(grid.nodes.iter()) {
            if !(own_node.is_element() || own_node.is_anonymous()) {
                continue;
            }
            assert_eq!(own_node.final_layout(), grid_node.final_layout(), "node {id}: {}", own_node.node_debug_str());
        }
    }
}

/// pyro: whether table part `node_id` (a row group, row or cell) is relatively positioned or
/// sticky: it paints as a positioned box, with its cells (a row group and a row have no boxes of
/// their own), and is the containing block of the absolutely positioned boxes in it.
pub(crate) fn is_positioned_table_part(doc: &BaseDocument, node_id: NodeId) -> bool {
    use style::computed_values::position::T as Position;
    doc.nodes
        .get(node_id)
        .and_then(|node| node.primary_styles())
        .is_some_and(|style| matches!(style.clone_position(), Position::Relative | Position::Sticky))
}

/// pyro: whether some row or row group of `table` is positioned ([`is_positioned_table_part`]).
pub(crate) fn table_has_positioned_parts(doc: &BaseDocument, table: &TableContext) -> bool {
    table.rows.iter().any(|row| {
        is_positioned_table_part(doc, row.node_id) || row.group.is_some_and(|group| is_positioned_table_part(doc, group))
    })
}

/// pyro: the offset relative positioning (CSS 2.1 § 9.4.3) gives table part `node_id` (a cell,
/// row or row group) by its own insets, `None` when it is not relatively positioned. A percentage
/// `top` or `bottom` resolves against the height `parent` (the row for a cell, the row group for a
/// row, the table for a row group) is given as a length, else it is `auto`, as in Chromium (WPT
/// `position-relative-010` to `-013`); a percentage `left` or `right` is `auto` (the width it would
/// resolve against is not known yet).
pub(crate) fn table_part_relative_offset(
    doc: &BaseDocument,
    node_id: NodeId,
    parent: Option<NodeId>,
) -> Option<taffy::Point<f32>> {
    use style::computed_values::direction::T as Direction;
    use style::computed_values::position::T as Position;
    use style::values::computed::Inset;
    use style::values::generics::length::GenericSize;
    let style = doc.nodes.get(node_id)?.primary_styles()?;
    if style.clone_position() != Position::Relative {
        return None;
    }
    let parent_height = parent
        .and_then(|parent| doc.nodes.get(parent))
        .and_then(|parent| parent.primary_styles())
        .and_then(|parent| match parent.get_position().height {
            GenericSize::LengthPercentage(ref height) => height.0.to_length().map(|length| length.px()),
            _ => None,
        });
    let resolve = |inset: &Inset, basis: Option<f32>| match inset {
        Inset::LengthPercentage(value) => match value.to_length() {
            Some(length) => Some(length.px()),
            None => basis.map(|basis| value.resolve(Length::new(basis)).px()),
        },
        _ => None,
    };
    let position = style.get_position();
    let (left, right) = (resolve(&position.left, None), resolve(&position.right, None));
    let (top, bottom) = (resolve(&position.top, parent_height), resolve(&position.bottom, parent_height));
    let x = match style.clone_direction() {
        Direction::Rtl => right.map(|right| -right).or(left),
        Direction::Ltr => left.or(right.map(|right| -right)),
    };
    Some(taffy::Point { x: x.unwrap_or(0.0), y: top.or(bottom.map(|bottom| -bottom)).unwrap_or(0.0) })
}

/// pyro: the row group row `row_id` is in (its parent, when that is one): known before
/// `collect_table_cells` sets `TableRow::group`, after the group's rows.
fn row_group_of(doc: &BaseDocument, row_id: NodeId) -> Option<NodeId> {
    let parent = doc.nodes.get(row_id)?.parent?;
    let display = doc.nodes.get(parent)?.display_style()?;
    matches!(
        display.inside(),
        DisplayInside::TableRowGroup | DisplayInside::TableHeaderGroup | DisplayInside::TableFooterGroup
    )
    .then_some(parent)
}

/// pyro: the table `node_id` is in: its nearest ancestor that is a table (or itself).
fn table_of(doc: &BaseDocument, node_id: NodeId) -> Option<NodeId> {
    let mut current = Some(node_id);
    while let Some(id) = current {
        let node = doc.nodes.get(id)?;
        if node.display_style().is_some_and(|display| display.inside() == DisplayInside::Table) {
            return Some(id);
        }
        current = node.parent;
    }
    None
}

/// pyro: how far relative positioning moves table cell `cell_id`: its own offset and those of
/// its row and row group (`row`: the row and its group), which have no boxes of their own and move
/// their cells ([`table_part_relative_offset`]). `None` when none of them is relatively positioned.
fn table_cell_relative_offset(
    doc: &BaseDocument,
    cell_id: NodeId,
    row: Option<(NodeId, Option<NodeId>)>,
) -> Option<taffy::Point<f32>> {
    let (row, group) = row.map_or((None, None), |(row, group)| (Some(row), group));
    let table = table_of(doc, cell_id);
    let parts = [(Some(cell_id), row), (row, group.or(table)), (group, table)];
    let mut offset: Option<taffy::Point<f32>> = None;
    for (part, parent) in parts {
        let Some(own) = part.and_then(|part| table_part_relative_offset(doc, part, parent)) else { continue };
        let sum = offset.get_or_insert(taffy::Point::ZERO);
        sum.x += own.x;
        sum.y += own.y;
    }
    offset
}

impl BaseDocument {
    /// pyro: the row group and row (`[group, row]`) of table cell `cell_id`, from its table's
    /// context; `None` for a node that is no cell of a table laid out.
    pub fn table_cell_parts(&self, cell_id: NodeId) -> Option<[Option<NodeId>; 2]> {
        let node = self.nodes.get(cell_id)?;
        let index = node.layout_data().table_cell_index;
        if index == u32::MAX {
            return None;
        }
        let table = node.layout_parent.get().and_then(|id| self.nodes.get(id))?;
        let SpecialElementData::TableRoot(ctx) = &table.element_data()?.special_data else { return None };
        let cell = ctx.cells.get(index as usize).filter(|cell| cell.node_id == cell_id)?;
        let row = ctx.rows.get(cell.row as usize)?;
        Some([row.group, Some(row.node_id)])
    }

    /// pyro: the containing block of the out-of-flow box `box_id` (`position` absolute or fixed)
    /// below table `table_id`: the nearest ancestor of the box below the table that makes one for
    /// it (positioned, transformed, ...). When that is a row or row group, which have no boxes of
    /// their own, the table lays the box out against the part's area, and it paints in the part's
    /// transform.
    pub fn containing_block_below_table(
        &self,
        table_id: NodeId,
        box_id: NodeId,
        position: taffy::Position,
    ) -> Option<NodeId> {
        let mut current = self.nodes.get(box_id)?.parent;
        loop {
            let id = current?;
            if id == table_id {
                return None;
            }
            let node = self.nodes.get(id)?;
            if node
                .primary_styles()
                .is_some_and(|style| stylo_taffy::convert::containing_block_claims(&style).for_position(position))
            {
                return Some(id);
            }
            current = node.parent;
        }
    }

    /// pyro: the row group and row of table cell `cell_id` that are positioned (relative or
    /// sticky): the cell paints with them, as positioned boxes paint, over their backgrounds.
    pub fn table_cell_positioned_parts(&self, cell_id: NodeId) -> [Option<NodeId>; 2] {
        self.table_cell_parts(cell_id)
            .map_or([None, None], |parts| parts.map(|part| part.filter(|&part| is_positioned_table_part(self, part))))
    }
}

/// pyro: nor does a table's `max-width` make it narrower than its columns' min-content widths (`content`,
/// spacing included; `extra` its padding and borders): a width its flex or grid parent gives it beyond the
/// maximum still stands up to them (CSS 2.1 §17.5.2, as in Blink; WPT `css/css-flexbox/table-as-item-*`).
fn raise_max_width_to_columns(style: &mut taffy::Style<Atom>, content: f32, extra: f32, parent: Option<f32>) {
    let grown = match style.box_sizing {
        taffy::BoxSizing::ContentBox => content,
        taffy::BoxSizing::BorderBox => content + extra,
    };
    use taffy::MaybeResolve;
    if style.max_size.width.maybe_resolve(parent, resolve_calc_value).is_some_and(|max: f32| max < grown) {
        style.max_size.width = style_helpers::length(grown);
    }
}

/// pyro: a cell's own `min-width` as the width of its border box.
fn cell_own_min_width(style: &taffy::Style<Atom>, parent: Option<f32>) -> Option<f32> {
    use taffy::MaybeResolve as _;
    style.min_size.width.maybe_resolve(parent, resolve_calc_value).map(|w| {
        let padding = style.padding.resolve_or_zero(parent, resolve_calc_value);
        let border = style.border.resolve_or_zero(parent, resolve_calc_value);
        let extra = padding.left + padding.right + border.left + border.right;
        match style.box_sizing {
            taffy::BoxSizing::ContentBox => w + extra,
            taffy::BoxSizing::BorderBox => w.max(extra),
        }
    })
}
