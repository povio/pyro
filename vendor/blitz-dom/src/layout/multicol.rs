//! pyro: CSS Multi-column Layout (<https://drafts.csswg.org/css-multicol/>).
//!
//! A multi-column container lays its content out once, in a strip one column wide (its flow
//! thread, as legacy WebKit's `RenderFlowThread`), picks where the strip breaks into columns, and
//! keeps each column as a fragment: a range of the strip shown at the column's place. Every box
//! keeps its one layout, in strip coordinates (the container's border box); paint (blitz-paint
//! `render/multicol.rs`), hit testing, the scrollable overflow and the geometry queries map them
//! through the fragments. A box a break goes through is drawn sliced, which is what
//! `box-decoration-break: slice` (the initial value) asks for: its borders, background and shadow
//! are cut at the break.
//!
//! Breaks (CSS Fragmentation 3):
//! - A line box, a replaced element, a scroll container, a table, a flex or grid container and a
//!   nested multi-column container are monolithic: a break goes before them, unless one is taller
//!   than a column, which is then cut where the column ends.
//! - A block container breaks between its children and anywhere through its own empty space
//!   (a tall empty box is cut where the column ends).
//! - `orphans`, `widows` and `break-inside: avoid` keep lines and boxes together where a column
//!   can hold them; balancing grows the columns until they can.
//! - Margins at a break are truncated: the column after a break between two blocks starts at the
//!   second one's border box.
//!
//! Columns are balanced (the shortest height that fits the content in `column-count` columns)
//! unless `column-fill: auto` and the height is definite. Content that does not fit the
//! container's height continues in columns past its inline end, as in Chromium.
//!
//! A column spanner (an in-flow block child with `column-span: all`) is laid out in the strip as
//! wide as the container's content box (`LayoutPassState::column_spanners`) and shown as a row of
//! its own: the content before it is balanced into a row of columns above it, the content after
//! it into a row below.
//!
//! Not supported: spanners deeper in the content (laid out in a column), `break-before`/
//! `break-after` forced column breaks, and vertical writing modes (the container is laid out as a
//! single column there).

use style::computed_values::column_fill::T as ColumnFill;
use style::properties::ComputedValues;
use style::values::computed::{ColumnSpan, Float, Length, Overflow};
use style::values::generics::column::ColumnCount;
use style::values::generics::length::{LengthPercentageOrAuto, LengthPercentageOrNormal};
use style::values::specified::BreakWithin;
use style::values::specified::box_::{DisplayInside, DisplayOutside};
use stylo_taffy::CopyStyleGetters as _;
use taffy::{
    AvailableSpace, CollapsibleMarginSet, CoreStyle as _, LayoutInput, LayoutOutput, Line, MaybeMath as _,
    MaybeResolve as _, NodeId, OofPositioningArea, Point, Rect, RequestedAxis, ResolveOrZero as _, RunMode,
    ScrollableOverflow, Size, SizingMode, compute_block_layout,
};

use super::{LayoutPassState, resolve_calc_value};
use crate::{BaseDocument, Node, dom_node_id};

/// How many columns a container makes at most (its content is cut off past them).
const MAX_COLUMNS: usize = 1000;
/// Positions closer than this are the same (CSS px).
const EPSILON: f32 = 1.0 / 128.0;

/// Whether `style` makes its box a multi-column container: a `column-count` or `column-width`
/// other than `auto` on a block container (`block`, `flow-root`, `inline-block`, `list-item`). In
/// a vertical writing mode the container is laid out in its logical axes and its columns are shown
/// only by paint (`Node::vertical_column_fragments`); geometry queries and hit tests see the strip.
#[inline]
pub(crate) fn is_multicol_style(style: &ComputedValues) -> bool {
    if !style.get_column().is_multicol() {
        return false;
    }
    let display = style.clone_display();
    let block_container = match display.inside() {
        DisplayInside::FlowRoot => true,
        DisplayInside::Flow => display.outside() == DisplayOutside::Block,
        _ => false,
    };
    block_container
}

/// The columns of a multi-column container, in its border-box coordinates (CSS px, rounded to
/// device pixels): the strip's block ranges and where each is shown.
#[derive(Debug, Clone, Default)]
pub struct ColumnFragments {
    /// In order: the first shows the strip from its start, the last to its end. Columns of one
    /// row share their `top`; a column spanner is a row of its own.
    pub fragments: Vec<ColumnFragment>,
}

/// One column of a multi-column container, or a column spanner (`column-span: all`) between
/// rows of columns.
#[derive(Debug, Clone, Copy)]
pub struct ColumnFragment {
    /// The strip range the column shows, `start..end` in the block axis (the first column's
    /// start is `-inf`, the last column's end `+inf`: what overflows the strip's ends shows).
    pub start: f32,
    pub end: f32,
    /// The column box: its left edge, width, top and height.
    pub x: f32,
    pub width: f32,
    pub top: f32,
    pub height: f32,
    /// What is added to a strip position to show it in this column.
    pub offset: Point<f32>,
    /// A column spanner's row: no column rule beside it.
    pub spanner: bool,
}

impl ColumnFragment {
    /// Whether the column shows strip position `y`.
    #[inline]
    fn shows(&self, y: f32) -> bool {
        y >= self.start && y < self.end
    }
}

impl ColumnFragments {
    /// The column that shows strip position `y` (the first one before the strip, the last after).
    /// A position in a margin truncated at a break (after one column's end, before the next
    /// one's start) is in the column after the break, where what follows it is shown.
    pub fn fragment_at(&self, y: f32) -> Option<&ColumnFragment> {
        self.fragments.iter().find(|f| f.shows(y) || f.start > y).or_else(|| self.fragments.last())
    }

    /// The move (CSS px) of the column that shows strip position `y` (none past the columns).
    pub fn shift_at(&self, y: f32) -> (f32, f32) {
        self.fragment_at(y).map_or((0.0, 0.0), |f| (f.offset.x, f.offset.y))
    }

    /// Where strip point `point` is shown.
    pub fn strip_to_columns(&self, point: Point<f32>) -> Point<f32> {
        match self.fragment_at(point.y) {
            Some(f) => Point { x: point.x + f.offset.x, y: point.y + f.offset.y },
            None => point,
        }
    }

    /// The strip point shown at `point` (container coordinates), through the column nearest to
    /// it in the inline axis (a gap split down its middle). Above a column other than the first
    /// and below one other than the last nothing of the strip is shown: the point is moved past
    /// the strip's end.
    pub fn columns_to_strip(&self, point: Point<f32>) -> Point<f32> {
        let (Some(first), Some(last)) = (self.fragments.first(), self.fragments.last()) else {
            return point;
        };
        // The row under the point: the first above the rows, the last below them.
        let row_top = if point.y < first.top {
            first.top
        } else if point.y >= last.top + last.height {
            last.top
        } else {
            match self.fragments.iter().find(|f| point.y >= f.top && point.y < f.top + f.height) {
                Some(f) => f.top,
                None => return Point { x: point.x, y: f32::MAX },
            }
        };
        let nearest = self.fragments.iter().enumerate().filter(|(_, f)| f.top == row_top).min_by(|(_, a), (_, b)| {
            let da = (point.x - (a.x + a.width / 2.0)).abs();
            let db = (point.x - (b.x + b.width / 2.0)).abs();
            da.total_cmp(&db)
        });
        let last = self.fragments.len() - 1;
        match nearest {
            Some((i, f)) if (i == 0 || point.y >= f.top) && (i == last || point.y < f.top + f.height) => {
                Point { x: point.x - f.offset.x, y: point.y - f.offset.y }
            }
            _ => Point { x: point.x, y: f32::MAX },
        }
    }

    /// A rectangle of the strip (device px, `scale` per CSS px), cut at the column breaks and
    /// shown in the columns: the bounding box of the pieces.
    pub fn map_rect(&self, rect: kurbo::Rect, scale: f64) -> kurbo::Rect {
        let mut out: Option<kurbo::Rect> = None;
        for f in &self.fragments {
            let y0 = rect.y0.max(f.start as f64 * scale);
            let y1 = rect.y1.min(f.end as f64 * scale);
            if y1 < y0 {
                continue;
            }
            let piece = kurbo::Rect::new(rect.x0, y0, rect.x1, y1)
                + kurbo::Vec2::new(f.offset.x as f64 * scale, f.offset.y as f64 * scale);
            out = Some(out.map_or(piece, |o| o.union(piece)));
        }
        out.unwrap_or(rect)
    }
}

/// Where a container's strip may break, from its laid-out content (container border-box
/// coordinates).
#[derive(Debug, Default, Clone)]
struct Breaks {
    /// The strip's content: `start..end`.
    start: f32,
    end: f32,
    /// What no break may go through when the columns can help it: line boxes, monolithic boxes,
    /// lines `orphans` and `widows` keep together, `break-inside: avoid` boxes. Merged: sorted,
    /// disjoint (touching ranges stay apart).
    strict: Vec<(f32, f32)>,
    /// What no break may go through at all (unless it is taller than a column): line boxes and
    /// monolithic boxes. Merged.
    loose: Vec<(f32, f32)>,
    /// The space between consecutive in-flow blocks (their margins): a column after a break in it
    /// starts at its end. Sorted by start.
    gaps: Vec<(f32, f32)>,
    /// pyro: lines `text-box-trim` trims where they start or end a column (`layout::text_box`):
    /// `(top, bottom, start trim, end trim)`, sorted by top.
    trims: Vec<(f32, f32, f32, f32)>,
    /// pyro: forced breaks (`break-before`/`break-after: column`, `page`, ...), sorted.
    forced: Vec<f32>,
}

/// How a strip fills columns of one height.
struct FillRun {
    /// Each column's strip range (`start..end`).
    columns: Vec<(f32, f32)>,
    /// The least growth of the columns that would move a break further on.
    shortage: Option<f32>,
    /// Breaks through what `Breaks::strict` keeps together.
    violations: usize,
}

/// The merged form of `ranges`: sorted, overlapping ranges joined (touching ones kept apart, as a
/// break may go between them), empty ones dropped.
fn merge(mut ranges: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    ranges.retain(|&(a, b)| b - a > EPSILON);
    ranges.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<(f32, f32)> = Vec::with_capacity(ranges.len());
    for (a, b) in ranges {
        match out.last_mut() {
            Some(last) if a < last.1 - EPSILON => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// The merged range `y` lies strictly inside.
fn containing(ranges: &[(f32, f32)], y: f32) -> Option<(f32, f32)> {
    let index = ranges.partition_point(|&(a, _)| a < y - EPSILON);
    let (a, b) = *ranges.get(index.checked_sub(1)?)?;
    (b > y + EPSILON).then_some((a, b))
}

impl Breaks {
    /// pyro: what the line that starts at `start` loses over its edge when it starts a column.
    fn start_trim(&self, start: f32) -> f32 {
        let index = self.trims.partition_point(|t| t.0 < start - EPSILON);
        self.trims.get(index).filter(|t| (t.0 - start).abs() <= EPSILON).map_or(0.0, |t| t.2)
    }

    /// pyro: a column end of `limit`, moved to the bottom of the line it cuts when that line fits
    /// above `limit` once its end is trimmed.
    fn trimmed_limit(&self, limit: f32) -> f32 {
        let index = self.trims.partition_point(|t| t.0 < limit);
        match index.checked_sub(1).and_then(|i| self.trims.get(i)) {
            Some(&(_, bottom, _, end)) if end > 0.0 && bottom > limit && bottom - end <= limit + EPSILON => bottom,
            _ => limit,
        }
    }

    /// The break of a column that starts at `start` and holds up to `limit`: the furthest
    /// position after `start` and no further than `limit` that no range of `ranges` goes
    /// through, and how much further the range in the way reaches. `None`: a range taller than
    /// the column is in the way.
    fn break_before(ranges: &[(f32, f32)], start: f32, limit: f32) -> (Option<f32>, Option<f32>) {
        match containing(ranges, limit) {
            None => (Some(limit), None),
            Some((a, b)) => ((a > start + EPSILON).then_some(a), Some(b - limit)),
        }
    }

    /// Where the column before a break at `at` ends and the column after it starts: at the
    /// margins it lies in, the first ends where they start and the second starts past them.
    fn break_at(&self, at: f32) -> (f32, f32) {
        let index = self.gaps.partition_point(|&(a, _)| a <= at + EPSILON);
        self.gaps[..index]
            .iter()
            .rev()
            .take(4)
            .find(|&&(a, b)| a <= at + EPSILON && at <= b + EPSILON)
            .map_or((at, at), |&(a, b)| (a.min(at), b.max(at)))
    }

    /// The strip in columns of height `height`, at most `max_columns` of them (the last takes the
    /// rest).
    fn fill(&self, height: f32, max_columns: usize) -> FillRun {
        let height = height.max(1.0);
        let mut run = FillRun { columns: Vec::new(), shortage: None, violations: 0 };
        let mut start = self.start;
        let note = |run: &mut FillRun, s: Option<f32>| {
            if let Some(s) = s.filter(|&s| s > EPSILON) {
                run.shortage = Some(run.shortage.map_or(s, |m: f32| m.min(s)));
            }
        };
        loop {
            // pyro: a column after a break starts below the half-leading its first line trims, and
            // holds a line whose trimmed end fits.
            if !run.columns.is_empty() {
                start += self.start_trim(start);
            }
            let limit = self.trimmed_limit(start + height);
            // pyro: a forced break before the column's end ends it there.
            let forced = (run.columns.len() + 1 < max_columns)
                .then(|| self.forced.iter().copied().find(|&f| f > start + EPSILON && f < self.end - EPSILON))
                .flatten();
            if let Some(f) = forced.filter(|&f| f <= limit.max(start + height) + EPSILON) {
                run.columns.push((start, f));
                start = f;
                continue;
            }
            if limit >= self.end - EPSILON || run.columns.len() + 1 >= max_columns {
                run.columns.push((start, self.end.max(start)));
                return run;
            }
            let (strict, strict_short) = Self::break_before(&self.strict, start, limit);
            note(&mut run, strict_short);
            let end = match strict {
                Some(end) => end,
                None => {
                    run.violations += 1;
                    let (loose, loose_short) = Self::break_before(&self.loose, start, limit);
                    note(&mut run, loose_short);
                    // Taller than a column: cut where the column ends.
                    loose.unwrap_or(limit)
                }
            };
            let (end, next) = self.break_at(end);
            run.columns.push((start, end.max(start)));
            if next >= self.end - EPSILON {
                // Nothing but margins left.
                return run;
            }
            start = if next > start + EPSILON { next } else { start + height };
        }
    }

    /// The balanced column height: the least that fits the strip in `count` columns, keeping
    /// together what `strict` keeps where it can, no more than `limit`.
    fn balance(&self, count: usize, limit: Option<f32>) -> f32 {
        let total = self.end - self.start;
        if total <= EPSILON {
            return 0.0;
        }
        let mut height = total / count as f32;
        if let Some(limit) = limit {
            height = height.min(limit);
        }
        for _ in 0..200 {
            let run = self.fill(height, MAX_COLUMNS);
            if run.columns.len() <= count && run.violations == 0 {
                break;
            }
            let Some(shortage) = run.shortage else {
                // Nothing in the way to grow past: the columns cut through what does not fit.
                if run.columns.len() <= count {
                    break;
                }
                height = total.min(limit.unwrap_or(f32::INFINITY));
                break;
            };
            let next = height + shortage.max(EPSILON);
            match limit {
                Some(limit) if next >= limit => {
                    height = limit;
                    break;
                }
                _ => height = next,
            }
            if height >= total {
                height = total;
                break;
            }
        }
        // The tallest column's content, not more.
        let run = self.fill(height, MAX_COLUMNS);
        let tallest = run.columns.iter().take(count.max(1)).map(|&(a, b)| b - a).fold(0.0, f32::max);
        if run.columns.len() <= count { tallest.min(height).max(EPSILON) } else { height }
    }
}

/// The number and width of a container's columns (CSS Multi-column Layout 1 §3.4): the content
/// box width `available`, `column-gap` `gap`.
fn column_boxes(count: Option<u32>, width: Option<f32>, available: f32, gap: f32) -> (usize, f32) {
    let fitting = |width: f32| {
        let pitch = width.max(0.0) + gap;
        if pitch > 0.0 { ((available + gap) / pitch).floor().max(1.0) as usize } else { MAX_COLUMNS }
    };
    let n = match (count, width) {
        (Some(count), None) => count as usize,
        (None, Some(width)) => fitting(width),
        (Some(count), Some(width)) => (count as usize).min(fitting(width)),
        (None, None) => 1,
    }
    .clamp(1, MAX_COLUMNS);
    let w = ((available - gap * (n - 1) as f32) / n as f32).max(0.0);
    (n, w)
}

/// A multi-column container's own properties.
struct Multicol {
    count: Option<u32>,
    width: Option<f32>,
    /// `column-gap`, resolved against the content box width when it is known.
    gap: style::values::computed::length::NonNegativeLengthPercentageOrNormal,
    font_size: f32,
    fill: ColumnFill,
    rtl: bool,
    /// pyro: `column-height` (css-multicol-2): the height of every column, unbalanced.
    height: Option<f32>,
    /// pyro: `column-wrap` (`auto` wraps when `column-height` is given): columns that do not
    /// fit a row go to a new row below, `row-gap` apart.
    wrap: bool,
    /// `row-gap` (`normal` is 0 here).
    row_gap: style::values::computed::length::NonNegativeLengthPercentageOrNormal,
}

impl Multicol {
    fn of(style: &ComputedValues) -> Self {
        let column = style.get_column();
        Self {
            count: match column.column_count {
                ColumnCount::Integer(n) => Some(n.0.max(1) as u32),
                ColumnCount::Auto => None,
            },
            width: match column.column_width {
                LengthPercentageOrAuto::LengthPercentage(width) => Some(width.0.px()),
                LengthPercentageOrAuto::Auto => None,
            },
            gap: style.get_position().column_gap.clone(),
            font_size: style.slow_clone_font_size().used_size().px(),
            fill: column.column_fill,
            rtl: style.clone_direction() == style::computed_values::direction::T::Rtl,
            height: match column.column_height {
                LengthPercentageOrAuto::LengthPercentage(height) => Some(height.0.px()),
                LengthPercentageOrAuto::Auto => None,
            },
            wrap: match column.column_wrap {
                style::values::computed::ColumnWrap::Wrap => true,
                style::values::computed::ColumnWrap::Nowrap => false,
                style::values::computed::ColumnWrap::Auto => !matches!(column.column_height, LengthPercentageOrAuto::Auto),
            },
            row_gap: style.get_position().row_gap.clone(),
        }
    }

    /// pyro: the used row gap between rows of columns: `normal` is 0; a percentage resolves
    /// against `basis` (the content box height when known).
    fn row_gap(&self, basis: Option<f32>) -> f32 {
        match &self.row_gap {
            LengthPercentageOrNormal::Normal => 0.0,
            LengthPercentageOrNormal::LengthPercentage(lp) => lp.0.resolve(Length::new(basis.unwrap_or(0.0))).px(),
        }
        .max(0.0)
    }

    /// The used column gap: `normal` is 1em; a percentage of an unknown width is none.
    fn gap(&self, basis: Option<f32>) -> f32 {
        match &self.gap {
            LengthPercentageOrNormal::Normal => self.font_size,
            LengthPercentageOrNormal::LengthPercentage(lp) => lp.0.resolve(Length::new(basis.unwrap_or(0.0))).px(),
        }
        .max(0.0)
    }
}

impl LayoutPassState<'_> {
    /// Lays out the multi-column container `node_id`: its content in a strip one column wide
    /// (`inline_root`: an inline formatting context), broken into columns. The fragments are
    /// stored in its `LayoutData::columns` by a `PerformLayout` run.
    #[inline(never)]
    pub(crate) fn compute_multicol_layout(
        &mut self,
        node_id: NodeId,
        inputs: LayoutInput,
        inline_root: bool,
    ) -> LayoutOutput {
        let dom_id = dom_node_id(node_id);
        let (multicol, padding, border, margin, size, min_size, max_size, clamps_block_size) = {
            let node = &self.nodes[dom_id];
            let Some(style) = node.primary_styles() else {
                return LayoutOutput::HIDDEN;
            };
            let multicol = Multicol::of(&style);
            // (In the container's own writing mode: its algorithm runs in its logical axes.)
            let ls = node.layout_style_in(self.layout_wm);
            let clamps_block_size = !ls.min_size().height.is_auto() || !ls.max_size().height.is_auto();
            let parent_width = inputs.parent_size.width;
            let padding = ls.padding().resolve_or_zero(parent_width, resolve_calc_value);
            let border = ls.border().resolve_or_zero(parent_width, resolve_calc_value);
            let margin = ls.margin().resolve_or_zero(parent_width, resolve_calc_value);
            let adjust = if ls.box_sizing() == taffy::BoxSizing::ContentBox {
                (padding + border).sum_axes()
            } else {
                Size::ZERO
            };
            let (size, min_size, max_size) = if inputs.sizing_mode == SizingMode::InherentSize {
                let parent = inputs.parent_size;
                (
                    ls.size().maybe_resolve(parent, resolve_calc_value).maybe_add(adjust),
                    ls.min_size().maybe_resolve(parent, resolve_calc_value).maybe_add(adjust),
                    ls.max_size().maybe_resolve(parent, resolve_calc_value).maybe_add(adjust),
                )
            } else {
                (Size::NONE, Size::NONE, Size::NONE)
            };
            (multicol, padding, border, margin, size, min_size, max_size, clamps_block_size)
        };
        let pb = padding + border;
        let pb_sum = pb.sum_axes();

        // The border box's width.
        let width = match inputs.known_dimensions.width.or(size.width.maybe_clamp(min_size.width, max_size.width)) {
            Some(width) => width,
            None => {
                let (min, max) = self.multicol_content_widths(node_id, &inputs, inline_root, &multicol, pb_sum.width);
                let content = match inputs.available_space.width {
                    AvailableSpace::Definite(available) => max.min(min.max(available - pb_sum.width)),
                    AvailableSpace::MinContent => min,
                    AvailableSpace::MaxContent => max,
                };
                (content + pb_sum.width).maybe_clamp(min_size.width, max_size.width)
            }
        }
        .max(pb_sum.width);
        if inputs.run_mode == RunMode::ComputeSize {
            if inputs.axis == RequestedAxis::Horizontal {
                return LayoutOutput::from_outer_size(Size { width, height: 0.0 });
            }
            if let Some(height) = inputs.known_dimensions.height {
                return LayoutOutput::from_outer_size(Size { width, height });
            }
        }

        let content_width = (width - pb_sum.width).max(0.0);
        let gap = multicol.gap(Some(content_width));
        let (count, column_width) = column_boxes(multicol.count, multicol.width, content_width, gap);

        // pyro: the content's percentage heights resolve against the column height, the
        // container's content box height when it is definite (its border box here).
        let percent_height = inputs
            .known_dimensions
            .height
            .or(size.height.maybe_clamp(min_size.height, max_size.height))
            .map(|height| height.max(pb_sum.height));

        // The strip: the content laid out one column wide, as tall as it is.
        let strip_inputs = LayoutInput {
            run_mode: RunMode::PerformLayout,
            sizing_mode: SizingMode::ContentSize,
            axis: RequestedAxis::Both,
            known_dimensions: Size { width: Some(column_width + pb_sum.width), height: None },
            known_dimensions_are_definite: Size { width: true, height: true },
            parent_size: inputs.parent_size,
            available_space: Size {
                width: AvailableSpace::Definite(column_width + pb_sum.width),
                // (An inline formatting context's atomic inlines resolve percentage heights
                // against its definite available height.)
                height: match (inline_root, percent_height) {
                    (true, Some(height)) => AvailableSpace::Definite(height),
                    _ => AvailableSpace::MaxContent,
                },
            },
            vertical_margins_are_collapsible: Line::FALSE,
        };
        // Column spanners (`column-span: all` children) are laid out as wide as the container.
        let spanning = (!inline_root).then(|| (dom_id, content_width)).filter(|_| self.doc.has_column_spanners(dom_id));
        let outer_spanners = std::mem::replace(&mut self.column_spanners, spanning);
        let outer_percent_height = std::mem::replace(&mut self.multicol_percent_height, percent_height.map(|height| (dom_id, height)));
        let strip = if inline_root {
            self.compute_inline_layout(dom_id, strip_inputs, None)
        } else {
            compute_block_layout(self, node_id, strip_inputs, None)
        };
        self.multicol_percent_height = outer_percent_height;
        self.column_spanners = outer_spanners;
        if inputs.run_mode == RunMode::ComputeSize {
            // The strip wrote its boxes' layouts at this width: a cached final layout of the
            // container at another width would leave them there.
            use taffy::CacheTree as _;
            self.cache_clear(node_id);
        }
        let content_top = pb.top;
        // Taffy's block algorithm clamps the strip by the container's `min-height` and
        // `max-height` whatever the sizing mode: under either, the content's end is taken from
        // the laid-out content instead (empty space a `min-height` adds is no content to balance).
        let strip_end = (inline_root || !clamps_block_size).then(|| (strip.size.height - pb.bottom).max(content_top));
        let breaks = self.doc.multicol_breaks(dom_id, content_top, strip_end);
        // The strip's parts: runs of columns, and the spanners between them (strip ranges).
        let spanners = match spanning {
            Some(_) => self.doc.column_spanner_ranges(dom_id),
            None => Vec::new(),
        };

        // The column height.
        let definite_height = inputs
            .known_dimensions
            .height
            .or(size.height.maybe_clamp(min_size.height, max_size.height))
            .map(|h| (h - pb_sum.height).max(0.0));
        let limit = definite_height.or(max_size.height.map(|h| (h - pb_sum.height).max(0.0)));
        let fill_auto = matches!((multicol.fill, definite_height), (ColumnFill::Auto, Some(_)))
            && (size.height.is_some() || inputs.known_dimensions.height.is_some());

        // The fragments, row by row: as many columns as each run of content needs, past the
        // container's end if it must.
        let scale = self.doc.viewport.scale();
        let snap = |v: f32| (v * scale).round() / scale;
        let mut fragments: Vec<ColumnFragment> = Vec::new();
        let mut row_top = content_top;
        let mut run_start = content_top;
        let column_x = |i: usize| {
            let step = i as f32 * (column_width + gap);
            snap(if multicol.rtl { pb.left + content_width - column_width - step } else { pb.left + step })
        };
        for part in 0..=spanners.len() {
            let run_end = spanners.get(part).map_or(breaks.end, |s| s.0);
            if run_end > run_start + EPSILON || (part == spanners.len() && fragments.is_empty()) {
                let run = Breaks { start: run_start, end: run_end, ..breaks.clone() };
                // pyro: `column-wrap: wrap` with `column-height: auto` wraps columns as tall as
                // the container's definite height.
                let fixed_height = multicol.height.or(definite_height.filter(|_| multicol.wrap));
                let column_height = if let Some(height) = fixed_height {
                    height.max(0.0)
                } else if spanners.is_empty() && fill_auto {
                    definite_height.unwrap_or(0.0)
                } else {
                    run.balance(count, if spanners.is_empty() { limit } else { None })
                };
                // pyro: `column-wrap: wrap`: each `count` columns make a row (css-multicol-2).
                let per_row = if fixed_height.is_some() && multicol.wrap { count.max(1) } else { usize::MAX };
                let row_gap = multicol.row_gap(definite_height);
                let mut columns = run.fill(column_height, MAX_COLUMNS).columns;
                let rows = columns.len().div_ceil(per_row).max(1);
                // pyro: wrapping at the container's height (`column-height: auto`), `column-fill:
                // balance` balances the last row (of at most `count` columns), that height its
                // limit; an explicit `column-height` keeps every row at it.
                let last_row = (rows - 1).saturating_mul(per_row.min(columns.len().max(1)));
                let mut last_height = column_height;
                if fixed_height.is_some()
                    && multicol.height.is_none()
                    && matches!(multicol.fill, ColumnFill::Balance)
                    && columns.len() - last_row.min(columns.len()) <= count
                    && let Some(&(tail_start, _)) = columns.get(last_row)
                {
                    let tail = Breaks { start: tail_start, end: run_end, ..breaks.clone() };
                    let height = tail.balance(count, Some(column_height)).min(column_height);
                    let refilled = tail.fill(height, MAX_COLUMNS).columns;
                    if refilled.len() <= count {
                        columns.truncate(last_row);
                        columns.extend(refilled);
                        last_height = height;
                    }
                }
                for (i, (start, end)) in columns.into_iter().enumerate() {
                    let (row, index) = if per_row == usize::MAX { (0, i) } else { (i / per_row, i % per_row) };
                    let x = column_x(index);
                    let top = snap(row_top + row as f32 * (column_height + row_gap));
                    let start = snap(start);
                    fragments.push(ColumnFragment {
                        start,
                        end: snap(end),
                        x,
                        width: column_width,
                        top,
                        height: if row + 1 == rows { last_height } else { column_height },
                        offset: Point { x: x - pb.left, y: snap(top - start) },
                        spanner: false,
                    });
                }
                row_top += (rows - 1) as f32 * (column_height + row_gap) + last_height;
            }
            if let Some(&(start, end)) = spanners.get(part) {
                let (start, end) = (snap(start), snap(end));
                fragments.push(ColumnFragment {
                    start,
                    end,
                    x: pb.left,
                    width: content_width,
                    top: row_top,
                    height: end - start,
                    offset: Point { x: 0.0, y: snap(row_top - start) },
                    spanner: true,
                });
                row_top += end - start;
                run_start = end;
            }
        }
        // The first shows what lies above the strip, the last what lies below.
        if let Some(first) = fragments.first_mut() {
            first.start = f32::NEG_INFINITY;
        }
        if let Some(last) = fragments.last_mut() {
            last.end = f32::INFINITY;
        }
        let columns_height = row_top - content_top;

        let height = inputs
            .known_dimensions
            .height
            .unwrap_or_else(|| {
                size.height.unwrap_or(columns_height + pb_sum.height).maybe_clamp(min_size.height, max_size.height)
            })
            .max(pb_sum.height);

        let right = fragments.iter().map(|f| f.x + f.width).fold(width - border.right, f32::max);
        let left = fragments.iter().map(|f| f.x).fold(border.left, f32::min);
        let bottom = (content_top + columns_height).max(height - border.bottom);
        let overflow = ScrollableOverflow::new(
            Rect {
                left: (left - border.left).min(0.0),
                top: 0.0,
                right: (right - border.left).max(0.0),
                bottom: (bottom - border.top).max(0.0),
            },
            Point { x: multicol.rtl, y: false },
        );

        if inputs.run_mode == RunMode::PerformLayout {
            self.doc.has_multicol = true;
            self.nodes[dom_id].layout_data_mut().columns = Some(Box::new(ColumnFragments { fragments }));
        }

        let size = Size { width, height };
        LayoutOutput {
            size,
            scrollable_overflow_rect: overflow,
            baselines: strip.baselines,
            top_margin: CollapsibleMarginSet::from_margin(margin.top),
            bottom_margin: CollapsibleMarginSet::from_margin(margin.bottom),
            margins_can_collapse_through: false,
            oof_candidates: strip.oof_candidates,
            oof_positioning_area: Some(OofPositioningArea {
                size: size - border.sum_axes(),
                offset: Point { x: border.left, y: border.top },
            }),
        }
    }

    /// The min-content and max-content widths of a multi-column container's content box, as
    /// Chromium's `ColumnLayoutAlgorithm::ComputeMinMaxSizes` computes them from its content's:
    /// `column-width` lowers the min-content width and raises the max-content width, and the
    /// column count multiplies them (not the min-content width when `column-width` is given).
    fn multicol_content_widths(
        &mut self,
        node_id: NodeId,
        inputs: &LayoutInput,
        inline_root: bool,
        multicol: &Multicol,
        pb_width: f32,
    ) -> (f32, f32) {
        let measure = |this: &mut Self, available: AvailableSpace| {
            let input = LayoutInput {
                run_mode: RunMode::ComputeSize,
                sizing_mode: SizingMode::ContentSize,
                axis: RequestedAxis::Horizontal,
                known_dimensions: Size::NONE,
                known_dimensions_are_definite: Size { width: true, height: true },
                parent_size: inputs.parent_size,
                available_space: Size { width: available, height: AvailableSpace::MaxContent },
                vertical_margins_are_collapsible: Line::FALSE,
            };
            let output = if inline_root {
                this.compute_inline_layout(dom_node_id(node_id), input, None)
            } else {
                compute_block_layout(this, node_id, input, None)
            };
            (output.size.width - pb_width).max(0.0)
        };
        let mut min = measure(self, AvailableSpace::MinContent);
        let mut max = measure(self, AvailableSpace::MaxContent);
        if let Some(width) = multicol.width {
            min = min.min(width);
            max = max.max(width).max(min);
        }
        let count = multicol.count.unwrap_or(1) as f32;
        let gaps = multicol.gap(None) * (count - 1.0);
        if multicol.width.is_none() {
            min = min * count + gaps;
        }
        max = max * count + gaps;
        (min, max)
    }
}

/// Whether `style` makes its box a column spanner when it is an in-flow block child of a
/// multi-column container: `column-span: all` on a block-level box that is not floated.
#[inline]
fn is_spanner_style(style: &ComputedValues) -> bool {
    style.get_column().column_span == ColumnSpan::All
        && style.clone_display().outside() == DisplayOutside::Block
        && style.clone_float() == Float::None
}

impl LayoutPassState<'_> {
    /// pyro: `inputs` for laying out `node_id` as a column spanner of the multi-column container
    /// whose strip is being laid out (`column_spanners`): as wide as the container's content box,
    /// less its margins, unless its width is not `auto`.
    #[inline(never)]
    pub(crate) fn column_spanner_inputs(&self, node_id: crate::NodeId, mut inputs: LayoutInput) -> LayoutInput {
        let Some((container, width)) = self.column_spanners else {
            return inputs;
        };
        let node = &self.nodes[node_id];
        if node.layout_parent.get() != Some(container) || node.is_out_of_flow() {
            return inputs;
        }
        let Some(style) = node.primary_styles() else {
            return inputs;
        };
        if !is_spanner_style(&style) {
            return inputs;
        }
        let ls = node.layout_style();
        let margin = ls.margin().resolve_or_zero(Some(width), resolve_calc_value);
        inputs.parent_size.width = Some(width);
        inputs.available_space.width = AvailableSpace::Definite(width);
        if ls.size().width.is_auto() {
            inputs.known_dimensions.width = Some((width - margin.left - margin.right).max(0.0));
        }
        inputs
    }
}

/// Whether `node`'s style makes it a multi-column container (see [`is_multicol_style`]).
#[inline]
pub(crate) fn is_multicol_node(node: &Node) -> bool {
    node.primary_styles().is_some_and(|style| is_multicol_style(&style))
}

impl Node {
    /// The columns of this multi-column container (`None` for any other box, or a box whose
    /// style no longer makes it one).
    #[inline]
    pub fn column_fragments(&self) -> Option<&ColumnFragments> {
        let columns = self.element_data()?.layout_data.as_deref()?.columns.as_deref()?;
        let style = self.primary_styles()?;
        (is_multicol_style(&style) && !style.writing_mode.is_vertical()).then_some(columns)
    }

    /// pyro: the columns of this multi-column container of a vertical writing mode, in its logical
    /// axes (inline = x, block = y), and whether its block axis runs right to left: paint shows
    /// them (see [`is_multicol_style`]).
    pub fn vertical_column_fragments(&self) -> Option<(&ColumnFragments, bool)> {
        let columns = self.element_data()?.layout_data.as_deref()?.columns.as_deref()?;
        let style = self.primary_styles()?;
        let wm = style.writing_mode;
        (is_multicol_style(&style) && wm.is_vertical()).then_some((columns, !wm.is_vertical_lr()))
    }

    /// `(x, y)` in this box's coordinates, moved from its columns into its strip when it is a
    /// multi-column container (hit testing).
    #[inline]
    pub(crate) fn columns_to_strip(&self, x: f32, y: f32) -> (f32, f32) {
        match self.column_fragments() {
            Some(columns) => {
                let p = columns.columns_to_strip(Point { x, y });
                (p.x, p.y)
            }
            None => (x, y),
        }
    }

    /// `point` in this box's coordinates, moved from its strip into its columns when it is a
    /// multi-column container (geometry queries).
    #[inline]
    pub(crate) fn strip_to_columns(&self, point: crate::util::Point<f32>) -> crate::util::Point<f32> {
        match self.column_fragments() {
            Some(columns) => {
                let p = columns.strip_to_columns(Point { x: point.x, y: point.y });
                crate::util::Point { x: p.x, y: p.y }
            }
            None => point,
        }
    }
}

impl BaseDocument {
    /// The columns of multi-column container `node_id` (see [`Node::column_fragments`]).
    #[inline]
    pub fn column_fragments(&self, node_id: crate::NodeId) -> Option<&ColumnFragments> {
        self.nodes.get(node_id)?.column_fragments()
    }

    /// `rect` (device px, `node_id`'s coordinates), shown in `node_id`'s columns when it is a
    /// multi-column container: its scrollable overflow and hit bound.
    pub(crate) fn columns_rect(&self, node_id: crate::NodeId, rect: kurbo::Rect, scale: f64) -> kurbo::Rect {
        match self.column_fragments(node_id) {
            Some(columns) => columns.map_rect(rect, scale),
            None => rect,
        }
    }

    /// The column spanners of `container`: its in-flow block children with `column-span: all`
    /// (a spanner deeper in its content is laid out in a column).
    fn column_spanners_of(&self, container: crate::NodeId) -> Vec<crate::NodeId> {
        let children = self.nodes[container].layout_children.borrow();
        children
            .iter()
            .flatten()
            .copied()
            .filter(|&id| {
                let node = &self.nodes[id];
                !node.is_out_of_flow() && node.primary_styles().is_some_and(|s| is_spanner_style(&s))
            })
            .collect()
    }

    /// Whether `container` has column spanners (see [`Self::column_spanners_of`]).
    fn has_column_spanners(&self, container: crate::NodeId) -> bool {
        !self.column_spanners_of(container).is_empty()
    }

    /// The strip ranges of `container`'s column spanners, their margins included, in order.
    fn column_spanner_ranges(&self, container: crate::NodeId) -> Vec<(f32, f32)> {
        let mut ranges: Vec<(f32, f32)> = Vec::new();
        for id in self.column_spanners_of(container) {
            let layout = self.nodes[id].unrounded_layout();
            let start = layout.location.y - layout.margin.top.max(0.0);
            let end = layout.location.y + layout.size.height + layout.margin.bottom.max(0.0);
            let start = ranges.last().map_or(start, |&(_, previous)| start.max(previous));
            ranges.push((start, end.max(start)));
        }
        ranges
    }

    /// Where the strip of the multi-column container `container` may break, from its laid-out
    /// content: `start..end` its content (`start` its content box's top; `end` the strip's
    /// content end, or `None` to take it from the content: its last in-flow child's bottom
    /// margin edge, its floats' margin edges).
    fn multicol_breaks(&self, container: crate::NodeId, start: f32, end: Option<f32>) -> Breaks {
        let mut found = Found::default();
        let trims = self.nodes[container]
            .primary_styles()
            .map_or((false, false), |style| super::text_box::fragment_trims(&style));
        self.collect_breaks(container, 0.0, &mut found, Some(start), trims);
        let end = end.unwrap_or(found.flow_end).max(start);
        // Margins only: a float reaching into the space between two blocks (the second one
        // cleared it) is content there.
        let floats = merge(found.floats);
        found.gaps.retain(|&(a, b)| {
            let index = floats.partition_point(|&(_, end)| end <= a + EPSILON);
            floats.get(index).is_none_or(|&(start, _)| start >= b - EPSILON)
        });
        found.gaps.sort_by(|a, b| a.0.total_cmp(&b.0));
        // What overflows a box of a fixed height is content of the strip too.
        let end = end.max(found.bottom);
        found.trims.sort_by(|a, b| a.0.total_cmp(&b.0));
        found.forced.append(&mut found.forced_after);
        found.forced.sort_by(f32::total_cmp);
        found.forced.dedup_by(|a, b| (*a - *b).abs() <= EPSILON);
        Breaks {
            start,
            end,
            strict: merge(found.strict),
            loose: merge(found.loose),
            gaps: found.gaps,
            trims: found.trims,
            forced: found.forced,
        }
    }

    /// pyro: whether anonymous block `id` comes right after, and right before, a split inline's
    /// block-level box among its parent's layout children.
    fn split_inline_neighbours(&self, id: crate::NodeId) -> (bool, bool) {
        let node = &self.nodes[id];
        if !node.is_anonymous() {
            return (false, false);
        }
        let Some(parent) = node.layout_parent.get() else { return (false, false) };
        let children = self.nodes[parent].layout_children.borrow();
        let Some(children) = children.as_deref() else { return (false, false) };
        let Some(at) = children.iter().position(|&c| c == id) else { return (false, false) };
        let split = |i: Option<usize>| i.and_then(|i| children.get(i)).is_some_and(|&c| self.nodes[c].is_split_inline());
        (split(at.checked_sub(1)), split(Some(at + 1)))
    }

    /// Adds what `parent`'s content keeps from breaks, and the margins between its blocks, to
    /// `found`: `parent`'s border box starts at strip position `top`. `container_content_top`:
    /// `parent` is the container, its content box's top at that (its own layout is written by its
    /// parent after its columns are made: what it holds now is a previous layout's or none).
    /// `trims`: whether lines here trim their block-start and block-end sides at column breaks.
    fn collect_breaks(
        &self,
        parent: crate::NodeId,
        top: f32,
        found: &mut Found,
        container_content_top: Option<f32>,
        trims: (bool, bool),
    ) {
        let node = &self.nodes[parent];
        let trims = match (container_content_top, node.primary_styles()) {
            (None, Some(style)) => super::text_box::fragment_trims_within(&style, trims),
            _ => trims,
        };
        if node.flags.is_inline_root() {
            let Some(text) = node.element_data().and_then(|e| e.inline_layout_data.as_deref()) else {
                return;
            };
            let content_top = container_content_top.unwrap_or_else(|| {
                let layout = node.unrounded_layout();
                layout.padding.top + layout.border.top
            });
            let origin = top + content_top + text.block_offset;
            let scale = text.layout.scale();
            // Lines with text or an atomic inline (a line of boxes alone has an empty text
            // range); parley's empty line after a trailing `<br>` has neither.
            let lines: Vec<(f32, f32)> = text
                .layout
                .lines()
                .filter(|line| !line.text_range().is_empty() || line.items().next().is_some())
                .map(|line| {
                    let m = line.metrics();
                    (origin + m.block_min_coord / scale, origin + m.block_max_coord / scale)
                })
                .collect();
            let Some(&(_, last_bottom)) = lines.last() else {
                return;
            };
            found.bottom = found.bottom.max(last_bottom);
            if (trims.0 || trims.1)
                && let Some(style) = node.primary_styles()
            {
                for line in text.layout.lines() {
                    if line.text_range().is_empty() && line.items().next().is_none() {
                        continue;
                    }
                    let m = line.metrics();
                    let trim = |start: bool| {
                        let on = if start { trims.0 } else { trims.1 };
                        if on { super::text_box::fragment_line_trim(&line, &style, start) / scale } else { 0.0 }
                    };
                    found.trims.push((
                        origin + m.block_min_coord / scale,
                        origin + m.block_max_coord / scale,
                        trim(true),
                        trim(false),
                    ));
                }
            }
            found.loose.extend_from_slice(&lines);
            found.strict.extend_from_slice(&lines);
            let (orphans, widows) = node.primary_styles().map_or((2, 2), |s| {
                let text = s.get_inherited_text();
                (text.orphans.0.max(1) as usize, text.widows.0.max(1) as usize)
            });
            let n = lines.len();
            // The anonymous blocks around an inline split by block-level content hold lines of one
            // inline formatting context with the block between them, a line of it too (Chromium's
            // block-in-inline): orphans and widows count across them (WPT css-position
            // `position-absolute-in-inline-007b`, `-008b`).
            let (after_split, before_split) = self.split_inline_neighbours(parent);
            if !after_split {
                found.strict.push((lines[0].0, lines[orphans.min(n) - 1].1));
            }
            if !before_split {
                found.strict.push((lines[n - widows.min(n)].0, lines[n - 1].1));
            }
            return;
        }
        let children = node.layout_children.borrow();
        let Some(children) = children.as_deref() else {
            return;
        };
        // The previous in-flow block's border-box bottom and bottom margin.
        let mut previous: Option<(f32, f32)> = None;
        for &child_id in children {
            let child = &self.nodes[child_id];
            let Some(style) = child.primary_styles() else {
                continue;
            };
            if child.is_out_of_flow() || style.get_box().display.is_none() {
                continue;
            }
            let layout = child.unrounded_layout();
            // pyro: breaks go by the flow: a relatively positioned box is fragmented where it would
            // be, then moved (Blink).
            let relative = match style.get_box().position {
                style::computed_values::position::T::Relative => {
                    super::inline::relative::relative_offset(&style, node.unrounded_layout().size, false).y
                }
                _ => 0.0,
            };
            let child_top = top + layout.location.y - relative;
            let child_bottom = child_top + layout.size.height;
            found.bottom = found.bottom.max(child_bottom);
            if container_content_top.is_some() {
                // The content's end: margin edges (the strip is a formatting context root).
                found.flow_end = found.flow_end.max(child_bottom + layout.margin.bottom.max(0.0));
            }
            if style.clone_float() == Float::None {
                // pyro: forced breaks between in-flow siblings (margins after them kept).
                let forces = |b: style::values::computed::BreakBetween| {
                    use style::values::computed::BreakBetween as B;
                    matches!(b, B::Column | B::Page | B::Always | B::Left | B::Right)
                };
                if forces(style.get_box().break_before) {
                    found.forced.push(child_top - layout.margin.top.max(0.0));
                }
                if forces(style.get_box().break_after) {
                    found.forced_after.push(child_bottom + layout.margin.bottom.max(0.0));
                }
                if let Some((bottom, margin)) = previous.filter(|&(b, _)| child_top >= b) {
                    // The collapsed margin, no more (clearance is no margin).
                    let margin = margin.max(layout.margin.top).max(0.0);
                    found.gaps.push(((child_top - margin).max(bottom), child_top));
                }
                previous = Some(match previous {
                    Some((bottom, _)) if bottom > child_bottom => (bottom, 0.0),
                    _ => (child_bottom, layout.margin.bottom),
                });
            } else {
                found.floats.push((child_top, child_bottom));
            }
            if fragmentable(child, &style) {
                if style.get_box().break_inside != BreakWithin::Auto {
                    found.strict.push((child_top, child_bottom));
                }
                crate::util::ensure_stack(|| self.collect_breaks(child_id, child_top, found, None, trims));
            } else {
                found.strict.push((child_top, child_bottom));
                found.loose.push((child_top, child_bottom));
            }
        }
    }
}

/// What [`BaseDocument::collect_breaks`] finds in a strip (see [`Breaks`]).
#[derive(Default)]
struct Found {
    strict: Vec<(f32, f32)>,
    loose: Vec<(f32, f32)>,
    gaps: Vec<(f32, f32)>,
    /// pyro: see [`Breaks::trims`].
    trims: Vec<(f32, f32, f32, f32)>,
    /// pyro: see [`Breaks::forced`]; `forced_after` from `break-after` (dropped after the last child).
    forced: Vec<f32>,
    forced_after: Vec<f32>,
    /// The floats' block ranges.
    floats: Vec<(f32, f32)>,
    /// The furthest bottom of a box or line.
    bottom: f32,
    /// The furthest bottom margin edge of the container's children.
    flow_end: f32,
}

/// Whether a break may go through `node`'s box (a block container that is no scroll container,
/// replaced element, form control or nested multi-column container); others are monolithic.
fn fragmentable(node: &crate::Node, style: &ComputedValues) -> bool {
    let Some(element) = node.element_data() else {
        return false;
    };
    if super::replaced::is_replaced(element) || element.text_input_data().is_some() || node.flags.is_table_root() {
        return false;
    }
    if matches!(&*element.name.local, "button" | "select" | "input" | "textarea") {
        return false;
    }
    let box_style = style.get_box();
    let scrolls = |o: Overflow| !matches!(o, Overflow::Visible | Overflow::Clip);
    if scrolls(box_style.overflow_x) || scrolls(box_style.overflow_y) || is_multicol_style(style) {
        return false;
    }
    matches!(style.clone_display().inside(), DisplayInside::Flow | DisplayInside::FlowRoot)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn breaks(strict: Vec<(f32, f32)>, loose: Vec<(f32, f32)>, end: f32) -> Breaks {
        Breaks { start: 0.0, end, strict: merge(strict), loose: merge(loose), gaps: Vec::new(), trims: Vec::new(), forced: Vec::new() }
    }

    #[test]
    fn column_count_and_width() {
        // column-count: 3 in 340px with a 20px gap.
        assert_eq!(column_boxes(Some(3), None, 340.0, 20.0), (3, 100.0));
        // column-width: 100px in 300px, no gap: three.
        assert_eq!(column_boxes(None, Some(100.0), 300.0, 0.0), (3, 100.0));
        // column-width wider than the box: one column, the box's width.
        assert_eq!(column_boxes(None, Some(500.0), 300.0, 16.0), (1, 300.0));
        // Both: the count is a maximum.
        assert_eq!(column_boxes(Some(5), Some(100.0), 300.0, 0.0), (3, 100.0));
    }

    #[test]
    fn empty_box_balances_evenly() {
        // A 100px empty block in two columns: 50px each, cut in the middle.
        let b = breaks(vec![], vec![], 100.0);
        assert_eq!(b.balance(2, Some(100.0)), 50.0);
        let run = b.fill(50.0, MAX_COLUMNS);
        assert_eq!(run.columns, vec![(0.0, 50.0), (50.0, 100.0)]);
    }

    #[test]
    fn lines_break_between() {
        // Three 46px lines, orphans and widows 1: two columns of two lines and one.
        let lines = vec![(0.0, 46.0), (46.0, 92.0), (92.0, 138.0)];
        let b = breaks(lines.clone(), lines, 138.0);
        assert_eq!(b.balance(2, None), 92.0);
        assert_eq!(b.fill(92.0, MAX_COLUMNS).columns, vec![(0.0, 92.0), (92.0, 138.0)]);
    }

    #[test]
    fn orphans_and_widows_keep_short_paragraphs_together() {
        // Two lines with orphans and widows 2 cannot split: one column holds both.
        let lines = vec![(0.0, 20.0), (20.0, 40.0)];
        let mut strict = lines.clone();
        strict.push((0.0, 40.0));
        let b = breaks(strict, lines, 40.0);
        assert_eq!(b.balance(2, None), 40.0);
    }

    #[test]
    fn monolithic_taller_than_column_is_cut() {
        // A 300px monolithic box in 100px columns (column-fill: auto) is cut at each column end.
        let b = breaks(vec![(0.0, 300.0)], vec![(0.0, 300.0)], 300.0);
        let run = b.fill(100.0, MAX_COLUMNS);
        assert_eq!(run.columns, vec![(0.0, 100.0), (100.0, 200.0), (200.0, 300.0)]);
    }

    #[test]
    fn column_after_break_between_blocks_starts_at_next_block() {
        // Two 18px paragraphs, 16px margins: the second column starts at the second paragraph,
        // its top margin truncated.
        let lines = vec![(16.0, 34.0), (50.0, 68.0)];
        let mut b = breaks(lines.clone(), lines, 84.0);
        b.gaps = vec![(34.0, 50.0)];
        let run = b.fill(34.0, MAX_COLUMNS);
        assert_eq!(run.columns, vec![(0.0, 34.0), (50.0, 84.0)]);
        // A column that ends in the margins ends where they start.
        assert_eq!(b.fill(40.0, MAX_COLUMNS).columns, vec![(0.0, 34.0), (50.0, 84.0)]);
        assert_eq!(b.balance(2, None), 34.0);
    }

    fn column(start: f32, end: f32, x: f32, top: f32, offset: (f32, f32)) -> ColumnFragment {
        ColumnFragment {
            start,
            end,
            x,
            width: 100.0,
            top,
            height: 100.0,
            offset: Point { x: offset.0, y: offset.1 },
            spanner: false,
        }
    }

    #[test]
    fn fragments_map_both_ways() {
        let columns = ColumnFragments {
            fragments: vec![
                column(f32::NEG_INFINITY, 100.0, 0.0, 0.0, (0.0, 0.0)),
                column(100.0, f32::INFINITY, 120.0, 0.0, (120.0, -100.0)),
            ],
        };
        assert_eq!(columns.strip_to_columns(Point { x: 10.0, y: 150.0 }), Point { x: 130.0, y: 50.0 });
        assert_eq!(columns.columns_to_strip(Point { x: 130.0, y: 50.0 }), Point { x: 10.0, y: 150.0 });
        // In the gap's second half: the second column.
        assert_eq!(columns.columns_to_strip(Point { x: 112.0, y: 10.0 }), Point { x: -8.0, y: 110.0 });
        // Below the first column: nothing of the strip; below the last: its overflow.
        assert_eq!(columns.columns_to_strip(Point { x: 50.0, y: 120.0 }).y, f32::MAX);
        assert_eq!(columns.columns_to_strip(Point { x: 170.0, y: 120.0 }), Point { x: 50.0, y: 220.0 });
        let rect = columns.map_rect(kurbo::Rect::new(0.0, 0.0, 100.0, 200.0), 1.0);
        assert_eq!(rect, kurbo::Rect::new(0.0, 0.0, 220.0, 100.0));
    }

    #[test]
    fn a_position_in_a_truncated_margin_is_in_the_column_after_the_break() {
        // The first column ends at 100 where a margin starts, the second starts past it at 120;
        // a third follows.
        let columns = ColumnFragments {
            fragments: vec![
                column(f32::NEG_INFINITY, 100.0, 0.0, 0.0, (0.0, 0.0)),
                column(120.0, 220.0, 120.0, 0.0, (120.0, -120.0)),
                column(220.0, f32::INFINITY, 240.0, 0.0, (240.0, -220.0)),
            ],
        };
        // An empty box at the margin's start is in the second column, where what follows it is
        // shown (not in the last).
        assert_eq!(columns.strip_to_columns(Point { x: 10.0, y: 100.0 }), Point { x: 130.0, y: -20.0 });
        assert_eq!(columns.shift_at(110.0), (120.0, -120.0));
        assert_eq!(columns.shift_at(99.0), (0.0, 0.0));
        assert_eq!(columns.shift_at(230.0), (240.0, -220.0));
    }

    #[test]
    fn a_spanner_is_a_row_of_its_own() {
        // Two columns of 0..200, a 30px spanner at 200..230, then two columns of 230..430.
        let spanner = ColumnFragment {
            start: 200.0,
            end: 230.0,
            x: 0.0,
            width: 220.0,
            top: 100.0,
            height: 30.0,
            offset: Point { x: 0.0, y: -100.0 },
            spanner: true,
        };
        let columns = ColumnFragments {
            fragments: vec![
                column(f32::NEG_INFINITY, 100.0, 0.0, 0.0, (0.0, 0.0)),
                column(100.0, 200.0, 120.0, 0.0, (120.0, -100.0)),
                spanner,
                column(230.0, 330.0, 0.0, 130.0, (0.0, -100.0)),
                column(330.0, f32::INFINITY, 120.0, 130.0, (120.0, -200.0)),
            ],
        };
        assert_eq!(columns.strip_to_columns(Point { x: 10.0, y: 210.0 }), Point { x: 10.0, y: 110.0 });
        assert_eq!(columns.strip_to_columns(Point { x: 10.0, y: 340.0 }), Point { x: 130.0, y: 140.0 });
        assert_eq!(columns.columns_to_strip(Point { x: 200.0, y: 110.0 }), Point { x: 200.0, y: 210.0 });
        assert_eq!(columns.columns_to_strip(Point { x: 130.0, y: 140.0 }), Point { x: 10.0, y: 340.0 });
    }
}
