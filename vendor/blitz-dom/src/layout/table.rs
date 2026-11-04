use blitz_traits::node_id::NodeId;
use std::{ops::Range, sync::Arc};
use style::logical_geometry::WritingMode;
use style::properties::ComputedValues;

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

use super::damage::{CONSTRUCT_BOX, CONSTRUCT_DESCENDENT, CONSTRUCT_FC};
use super::resolve_calc_value;

pub struct TableTreeWrapper<'a, 'doc> {
    pub(crate) doc: &'a mut LayoutPassState<'doc>,
    pub(crate) ctx: Arc<TableContext>,
    /// The table's style with the column widths of [`Self::auto_column_widths`].
    pub(crate) style: Option<taffy::Style<Atom>>,
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
        if ctx.is_fixed || n == 0 || !(all_auto || all_hinted) {
            return None;
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
            self.doc.nodes[cell.node_id].primary_styles().is_some_and(|style| {
                let width = stylo_taffy::convert::dimension(&style.clone_width());
                !matches!(width.tag(), taffy::CompactLength::AUTO_TAG | taffy::CompactLength::LENGTH_TAG)
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
                parent_size: taffy::Size { width: Some(inner), height: None },
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
        let mut spanning = Vec::new();
        for (idx, cell) in ctx.cells.iter().enumerate() {
            let column = cell.column as usize;
            let lo = measure(self, idx, taffy::AvailableSpace::MinContent);
            let hi = measure(self, idx, taffy::AvailableSpace::MaxContent).max(lo);
            if cell.colspan == 1 && column < n {
                min[column] = min[column].max(lo);
                max[column] = max[column].max(hi);
            } else {
                spanning.push((column, (cell.colspan as usize).max(1), lo, hi));
            }
        }
        // A spanning cell wider than the columns it spans: leave it to the grid.
        for (column, span, lo, hi) in spanning {
            let end = (column + span).min(n);
            let spacing = gap * (end.saturating_sub(column + 1)) as f32;
            let sum = |w: &[f32]| w[column.min(end)..end].iter().sum::<f32>() + spacing;
            if lo > sum(&min) + 0.01 || hi > sum(&max) + 0.01 {
                return None;
            }
        }
        let (sum_min, sum_max) = (min.iter().sum::<f32>(), max.iter().sum::<f32>());
        if sum_max <= 0.0 {
            return None;
        }
        let mut style = style.clone();
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
        let inner = if shrink { inner.min(sum_max.max(min_inner)) } else { inner };
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
        let style = self.doc.nodes[cell.node_id].primary_styles()?;
        Some(stylo_taffy::convert::dimension(&style.clone_width()))
    }

    /// Whether the table is in a table cell.
    fn is_inside_a_cell(&self, ctx: &TableContext) -> bool {
        use style::values::specified::box_::DisplayInside;
        let Some(cell) = ctx.cells.first() else { return false };
        let mut at = self.doc.nodes[cell.node_id].parent;
        while let Some(id) = at {
            let node = &self.doc.nodes[id];
            if node.primary_styles().is_some_and(|s| s.clone_display().inside() == DisplayInside::TableCell) {
                return true;
            }
            at = node.parent;
        }
        false
    }

    /// Whether a cell or `<col>` of an auto table has a percentage `width`.
    fn has_percent_cell(&self, ctx: &TableContext) -> bool {
        ctx.column_percents.iter().any(Option::is_some)
            || ctx.cells.iter().any(|cell| {
                self.cell_width(cell).is_some_and(|width| width.tag() == taffy::CompactLength::PERCENT_TAG)
            })
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
        // `<col>` and first-row lengths.
        for (c, track) in style.grid_template_columns.iter().enumerate() {
            if let taffy::GridTemplateComponent::Single(track) = track {
                if track.max.into_raw().tag() == taffy::CompactLength::LENGTH_TAG {
                    let len = track.max.into_raw().value();
                    max[c] = len;
                    fixed[c] = len > 0.0;
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
            let lo = measure(self, idx, taffy::AvailableSpace::MinContent);
            let hi = measure(self, idx, taffy::AvailableSpace::MaxContent).max(lo);
            if cell.colspan > 1 || column >= n {
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
                    max[column] = max[column].max(width.value() + box_extra);
                }
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
            let free: Vec<usize> = columns.clone().filter(|&c| percent[c].is_none()).collect();
            let group = if free.is_empty() { columns.collect() } else { free };
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
        let inner = match inputs.known_dimensions.width {
            Some(w) => w - spacing,
            None => match border_box(style.size.width.maybe_resolve(parent, resolve_calc_value)) {
                Some(w) => w.max(min_width) - spacing,
                None => match inputs.available_space.width {
                    // A table shrinks to fit its columns.
                    taffy::AvailableSpace::Definite(a) => {
                        (a.max(min_width) - spacing).min(grid_max.max(min_width - spacing))
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
    /// Laid out with the fixed table layout algorithm: cell contents don't size
    /// the columns, so cells have no intrinsic width.
    pub is_fixed: bool,
    /// Backing storage for `calc()` track sizes synthesised by the table layout code.
    /// Taffy stores calc values as raw pointers, so these must outlive the `style`.
    #[allow(dead_code)]
    calc_values: Vec<LengthPercentage>,
}

// #[derive(Debug, Clone, Eq, PartialEq)]
// pub enum TableItemKind {
//     Row,
//     Cell,
// }

/// One segment of a collapsed border; width 0 when no border wins.
#[derive(Debug, Clone, Copy)]
pub struct CollapsedEdge {
    pub width: f32,
    pub color: style::color::AbsoluteColor,
    rank: u8,
    priority: u8,
    hidden: bool,
}

/// The collapsed borders of a table's grid lines, in the order of `css-tables-3` conflict resolution.
#[derive(Debug, Clone)]
pub struct CollapsedEdges {
    pub rows: usize,
    pub columns: usize,
    /// `rows * (columns + 1)`: the segment of each row on each column line.
    pub vertical: Vec<CollapsedEdge>,
    /// `(rows + 1) * columns`: the segment of each column on each row line.
    pub horizontal: Vec<CollapsedEdge>,
}

impl CollapsedEdges {
    pub fn vertical_at(&self, row: usize, line: usize) -> &CollapsedEdge {
        &self.vertical[row * (self.columns + 1) + line]
    }
    pub fn horizontal_at(&self, line: usize, column: usize) -> &CollapsedEdge {
        &self.horizontal[line * self.columns + column]
    }

    fn new(rows: usize, columns: usize) -> Self {
        let none = CollapsedEdge {
            width: 0.0,
            color: style::color::AbsoluteColor::TRANSPARENT_BLACK,
            rank: 0,
            priority: 0,
            hidden: false,
        };
        Self { rows, columns, vertical: vec![none; rows * (columns + 1)], horizontal: vec![none; (rows + 1) * columns] }
    }

    /// Uniform: every vertical segment one width, every horizontal one width, one color.
    fn uniform(&self) -> Option<(f32, f32)> {
        let (x, y) = (self.vertical.first()?.width, self.horizontal.first()?.width);
        let visible = self.vertical.iter().chain(&self.horizontal).find(|e| e.width > 0.0)?;
        let same_color = self.vertical.iter().chain(&self.horizontal).all(|e| e.width == 0.0 || e.color == visible.color);
        (same_color
            && self.vertical.iter().all(|e| e.width == x)
            && self.horizontal.iter().all(|e| e.width == y))
        .then_some((x, y))
    }

    fn is_empty(&self) -> bool {
        self.vertical.iter().chain(&self.horizontal).all(|e| e.width == 0.0)
    }
}

/// Conflict resolution (css-tables-3 §11): `hidden` wins, then the wider, then by style, then
/// the inner box; among equals the first offered (left and top) stays.
fn offer_edge(
    edge: &mut CollapsedEdge,
    border: &Border,
    side: usize,
    current_color: &style::color::AbsoluteColor,
    priority: u8,
) {
    use style::values::computed::BorderStyle as B;
    let (width, border_style, color) = match side {
        0 => (border.border_top_width.0, border.border_top_style, &border.border_top_color),
        1 => (border.border_right_width.0, border.border_right_style, &border.border_right_color),
        2 => (border.border_bottom_width.0, border.border_bottom_style, &border.border_bottom_color),
        _ => (border.border_left_width.0, border.border_left_style, &border.border_left_color),
    };
    if edge.hidden || border_style == B::None {
        return;
    }
    if border_style == B::Hidden {
        edge.hidden = true;
        edge.width = 0.0;
        return;
    }
    let width = width.to_f32_px();
    if width == 0.0 {
        return;
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
        *edge = CollapsedEdge { width, color: color.resolve_to_absolute(current_color), rank, priority, hidden: false };
    }
}

/// The grid area (rows, columns) each cell ends up in: the first free columns of its row,
/// which is how the grid's dense auto placement places it.
fn cell_areas(cells: &[TableCell], nrows: usize, ncols: usize) -> Vec<(usize, usize, usize, usize)> {
    let mut taken = vec![false; nrows * ncols];
    cells
        .iter()
        .map(|cell| {
            let rowspan = match cell.style.grid_row.end {
                taffy::GridPlacement::Span(n) => n as usize,
                _ => 1,
            };
            let r0 = cell.row as usize;
            let r1 = (r0 + rowspan).min(nrows);
            let colspan = cell.colspan as usize;
            let free = |c: usize| (c..(c + colspan).min(ncols)).all(|c| !taken[r0 * ncols + c]);
            let c0 = if r0 < nrows { (0..ncols).find(|&c| free(c)).unwrap_or(ncols) } else { ncols };
            let c1 = (c0 + colspan).min(ncols);
            for r in r0..r1 {
                for c in c0..c1 {
                    taken[r * ncols + c] = true;
                }
            }
            (r0, c0, r1, c1)
        })
        .collect()
}

/// Resolves the collapsed borders of the table's cells, rows, row groups and the table.
fn resolve_collapsed_edges(
    doc: &BaseDocument,
    table: NodeId,
    cells: &[TableCell],
    rows: &[TableRow],
    columns: usize,
) -> CollapsedEdges {
    let mut edges = CollapsedEdges::new(rows.len(), columns);
    let (nrows, ncols) = (rows.len(), columns);
    let mut offer = |id: NodeId, r0: usize, c0: usize, r1: usize, c1: usize, priority: u8| {
        let Some(styles) = doc.nodes[id].primary_styles() else { return };
        let border = styles.get_border();
        let color = styles.clone_color();
        for c in c0..c1 {
            offer_edge(&mut edges.horizontal[r0 * ncols + c], border, 0, &color, priority);
            offer_edge(&mut edges.horizontal[r1 * ncols + c], border, 2, &color, priority);
        }
        for r in r0..r1 {
            offer_edge(&mut edges.vertical[r * (ncols + 1) + c0], border, 3, &color, priority);
            offer_edge(&mut edges.vertical[r * (ncols + 1) + c1], border, 1, &color, priority);
        }
    };
    for (cell, (r0, c0, r1, c1)) in cells.iter().zip(cell_areas(cells, nrows, ncols)) {
        if r0 < r1 && c0 < c1 {
            offer(cell.node_id, r0, c0, r1, c1, 3);
        }
    }
    for (r, row) in rows.iter().enumerate() {
        offer(row.node_id, r, 0, r + 1, ncols, 2);
    }
    let mut r = 0;
    while r < nrows {
        let group = rows[r].group;
        let end = rows[r..].iter().take_while(|row| row.group == group).count() + r;
        if let Some(group) = group {
            offer(group, r, 0, end, ncols, 1);
        }
        r = end;
    }
    if nrows > 0 && ncols > 0 {
        offer(table, 0, 0, nrows, ncols, 0);
    }
    edges
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
        while self.rowspans.get(self.col as usize).is_some_and(|r| *r > 0) {
            self.col += 1;
        }
        self.col
    }

    fn place(&mut self, colspan: u16, rowspan: u16) {
        let end = self.col + colspan;
        if self.rowspans.len() < end as usize {
            self.rowspans.resize(end as usize, 0);
        }
        for remaining in &mut self.rowspans[self.col as usize..end as usize] {
            *remaining = rowspan - 1;
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
fn table_taffy_style(style: &ComputedValues, table_wm: WritingMode) -> taffy::Style<Atom> {
    stylo_taffy::to_taffy_style_in(style, table_wm)
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

pub(crate) fn build_table_context(
    doc: &mut BaseDocument,
    table_root_node_id: NodeId,
) -> (TableContext, Vec<NodeId>) {
    let mut cells: Vec<TableCell> = Vec::new();
    let mut oof_children: Vec<(NodeId, taffy::Position)> = Vec::new();
    let mut rows: Vec<TableRow> = Vec::new();
    let mut row = 0u16;
    let mut cursor = ColumnCursor::default();

    let wm = table_wm(doc, table_root_node_id);
    let root_node = &mut doc.nodes[table_root_node_id];

    let children = std::mem::take(&mut root_node.children);

    let Some(stylo_styles) = root_node.primary_styles() else {
        panic!("Ignoring table because it has no styles");
    };

    let mut style = table_taffy_style(&stylo_styles, wm);
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
    let mut first_cell_border: Option<ServoArc<Border>> = None;
    // Percentage widths set on first-row cells: (column index, percentage, padding + border)
    let mut percent_columns: Vec<(u16, f32, f32)> = Vec::new();
    let mut calc_values: Vec<LengthPercentage> = Vec::new();
    // Header row groups are laid out before other rows, and footer row groups after
    let row_group_order = |doc: &BaseDocument, child_id: NodeId| -> u8 {
        let display = doc.nodes[child_id]
            .primary_styles()
            .map(|s| s.clone_display());
        match display.map(|d| d.inside()) {
            Some(DisplayInside::TableHeaderGroup) => 0,
            Some(DisplayInside::TableFooterGroup) => 2,
            _ => 1,
        }
    };
    for order in 0..3 {
        for child_id in children.iter().copied() {
            if row_group_order(doc, child_id) != order {
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
            );
        }
    }
    // lightningpanda: a rowspan stops at the end of its row group (the HTML table model;
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
    while column_sizes.len() > cursor.num_columns as usize
        && column_sizes
            .last()
            .is_some_and(|c| c.min.into_raw().tag() == taffy::CompactLength::AUTO_TAG)
    {
        column_sizes.pop();
        columns.pop();
    }
    let num_columns = cursor.num_columns.max(column_sizes.len() as u16);
    column_sizes.resize(num_columns as usize, remaining_column);
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
    // A row's `height` is a minimum: the row grows to fit the content of its cells.
    // Excess table height is distributed to unconstrained rows (via the stretching of
    // `auto` tracks) if there are any, and to all rows otherwise. So constrained rows
    // use a `max-content` (rather than `auto`) maximum when there are unconstrained rows.
    let has_unconstrained_rows = rows.iter().any(|row| !row.is_constrained);
    style.grid_template_rows = rows
        .iter()
        .map(|row| {
            let min = match row.height {
                Some(height) => style_helpers::length(height),
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
            style_helpers::minmax(min, max)
        })
        .collect();

    style.gap = match border_collapse {
        BorderCollapse::Separate => {
            // In the separated borders model, `border-spacing` also applies between
            // the table border and the outermost cells, in addition to between cells.
            let spacing_x = border_spacing.width.px();
            let spacing_y = border_spacing.height.px();
            let padding = style.padding.resolve_or_zero(None, resolve_calc_value);
            style.padding = taffy::Rect {
                left: style_helpers::length(padding.left + spacing_x),
                right: style_helpers::length(padding.right + spacing_x),
                top: style_helpers::length(padding.top + spacing_y),
                bottom: style_helpers::length(padding.bottom + spacing_y),
            };
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
    }

    // lightningpanda: a table whose collapsed borders are not one uniform grid has each cell keep half
    // of the borders around it, and the table half of its outer ones, as Chromium's boxes do.
    let collapsed_edges = (border_collapse == BorderCollapse::Collapse && !wm.is_vertical())
        .then(|| resolve_collapsed_edges(doc, table_root_node_id, &cells, &rows, style.grid_template_columns.len()))
        .filter(|edges| {
            let gap = (style.gap.width.into_raw().value(), style.gap.height.into_raw().value());
            !edges.is_empty() && edges.uniform() != Some(gap) || (edges.is_empty() && gap != (0.0, 0.0))
        });
    if let Some(edges) = &collapsed_edges {
        let zero = style_helpers::length(0.0);
        style.gap = taffy::Size { width: zero, height: zero };
        let half_max = |it: &mut dyn Iterator<Item = f32>| style_helpers::length(it.fold(0.0f32, f32::max) / 2.0);
        let (r, c) = (edges.rows, edges.columns);
        style.border = taffy::Rect {
            left: half_max(&mut (0..r).map(|i| edges.vertical_at(i, 0).width)),
            right: half_max(&mut (0..r).map(|i| edges.vertical_at(i, c).width)),
            top: half_max(&mut (0..c).map(|i| edges.horizontal_at(0, i).width)),
            bottom: half_max(&mut (0..c).map(|i| edges.horizontal_at(r, i).width)),
        };
        let areas = cell_areas(&cells, r, c);
        for (cell, (r0, c0, r1, c1)) in cells.iter_mut().zip(areas) {
            if r0 >= r1 || c0 >= c1 {
                continue;
            }
            let top = (c0..c1).map(|i| edges.horizontal_at(r0, i).width).fold(0.0, f32::max) / 2.0;
            let bottom = (c0..c1).map(|i| edges.horizontal_at(r1, i).width).fold(0.0, f32::max) / 2.0;
            let left = (r0..r1).map(|i| edges.vertical_at(i, c0).width).fold(0.0, f32::max) / 2.0;
            let right = (r0..r1).map(|i| edges.vertical_at(i, c1).width).fold(0.0, f32::max) / 2.0;
            cell.style.border = taffy::Rect {
                left: style_helpers::length(left),
                right: style_helpers::length(right),
                top: style_helpers::length(top),
                bottom: style_helpers::length(bottom),
            };
            if let Some((height, content_box)) = cell.height_base {
                cell.min_height = Some(if content_box { height + top + bottom } else { height });
            }
            if let Some(width) = cell.min_width.as_mut() {
                *width += left + right;
            }
            doc.nodes[cell.node_id].layout_data_mut().collapsed_border = Some([top, right, bottom, left]);
        }
    } else {
        for cell in cells.iter() {
            doc.nodes[cell.node_id].layout_data_mut().collapsed_border = None;
        }
    }

    let layout_children = cells
        .iter()
        .map(|cell| cell.node_id)
        .chain(oof_children.iter().map(|(node_id, _)| *node_id))
        .collect();
    let root_node = &mut doc.nodes[table_root_node_id];
    root_node.children = children;

    (
        TableContext {
            style,
            cells,
            oof_children,
            rows,
            columns,
            column_percents,
        computed_grid_info: AtomicRefCell::new(None),
            border_collapse,
            border_style: first_cell_border,
            collapsed_edges,
            is_fixed,
            calc_values,
        },
        layout_children,
    )
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
        }
        DisplayInside::TableColumn => {
            let style = table_taffy_style(&node.primary_styles().unwrap(), wm);
            let span: u16 = node
                .attr(local_name!("span"))
                .and_then(|val| val.parse::<u16>().ok())
                .map(|v| v.max(1))
                .unwrap_or(1);
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
                taffy::CompactLength::PERCENT_TAG => {
                    style_helpers::percent(style.size.width.value())
                }
                // Browsers treat calc() widths on columns as auto
                _ => style_helpers::auto(),
            };
            // A definite `min-width` on a column acts as a floor on its width
            let column = match style.min_size.width.into_raw().tag() {
                taffy::CompactLength::LENGTH_TAG if column.max.is_auto() => style_helpers::minmax(
                    style_helpers::length(style.min_size.width.into_raw().value()),
                    style_helpers::auto(),
                ),
                _ => column,
            };
            for _ in 0..span {
                columns.push(TableColumn {
                    node_id,
                    group: None,
                });
                column_sizes.push(column);
            }
        }
        _ => {}
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
) {
    let node = &mut doc.nodes[node_id];

    if !node.is_element() {
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
        oof_children.push((node_id, stylo_taffy::convert::position(position)));
        return;
    }

    match display.inside() {
        DisplayInside::TableRowGroup
        | DisplayInside::TableHeaderGroup
        | DisplayInside::TableFooterGroup
        | DisplayInside::Contents => {
            let first_row = rows.len();
            let children = std::mem::take(&mut doc.nodes[node_id].children);
            for child_id in children.iter().copied() {
                doc.nodes[child_id]
                    .remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
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
                );
            }
            doc.nodes[node_id].children = children;
            if display.inside() != DisplayInside::Contents {
                for row in &mut rows[first_row..] {
                    row.group.get_or_insert(node_id);
                }
            }
        }
        DisplayInside::TableRow => {
            node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
            *row += 1;
            cursor.start_row();

            let height = node.primary_styles().and_then(|style| {
                let height = stylo_taffy::convert::dimension(&style.clone_height());
                (height.tag() == taffy::CompactLength::LENGTH_TAG).then(|| height.value())
            });
            rows.push(TableRow {
                node_id,
                group: None,
                height,
                is_constrained: height.is_some(),
            });

            let children = std::mem::take(&mut doc.nodes[node_id].children);
            for child_id in children.iter().copied() {
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
                );
            }
            doc.nodes[node_id].children = children;
        }
        DisplayInside::TableCell => {
            // node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
            let stylo_style = &node.primary_styles().unwrap();
            let colspan: u16 = node
                .attr(local_name!("colspan"))
                .and_then(|val| val.parse().ok())
                .unwrap_or(1);
            let rowspan: u16 = node
                .attr(local_name!("rowspan"))
                .and_then(|val| val.parse::<u16>().ok())
                .map(|v| v.clamp(1, 65534))
                .unwrap_or(1);
            let mut style = table_taffy_style(stylo_style, wm);
            let col = cursor.next_free();

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
            let length_width = style.size.width.into_option().map(|len| len + extra);

            // Column widths from `<col>` elements take precedence over cell widths
            let col_needs_width =
                (col as usize) >= columns.len() || columns[col as usize].max.is_auto();
            if *row == 1 && col_needs_width {
                let column: TrackSizingFunction = match style.size.width.tag() {
                    taffy::CompactLength::LENGTH_TAG => {
                        style_helpers::length(style.size.width.value() + extra)
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

            // Let Taffy auto-place the column. Combined with
            // `grid_auto_flow: RowDense` set on the table root, each cell
            // scans from the first track in its row for a free position,
            // which makes cells automatically skip columns occupied by
            // rowspan cells from earlier rows.
            style.grid_column = taffy::Line {
                start: style_helpers::auto(),
                end: style_helpers::span(colspan),
            };
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
            cells.push(TableCell {
                node_id,
                row: row.saturating_sub(1),
                column: col,
                colspan,
                min_width: if is_fixed { None } else { length_width },
                min_height,
                height_base,
                style,
            });

            cursor.place(colspan, rowspan);
        }
        DisplayInside::Flow
        | DisplayInside::FlowRoot
        | DisplayInside::Flex
        | DisplayInside::Grid => {
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
        let node_id = crate::taffy_node_id(self.ctx.cells[usize::from(node_id)].node_id);
        self.doc.set_unrounded_layout(node_id, layout)
    }

    fn compute_child_layout(
        &mut self,
        node_id: taffy::NodeId,
        inputs: taffy::tree::LayoutInput,
    ) -> taffy::LayoutOutput {
        let cell = &self.ctx.cells[usize::from(node_id)];
        let node_id = crate::taffy_node_id(cell.node_id);
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
            let own_min = {
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
            };
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

    fn set_detailed_grid_info(
        &mut self,
        _node_id: taffy::NodeId,
        detailed_grid_info: DetailedGridInfo<Atom>,
    ) {
        *self.ctx.computed_grid_info.borrow_mut() = Some(detailed_grid_info);
    }
}
