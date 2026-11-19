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
//! A column spanner (an in-flow block with `column-span: all` that is the container's child, or
//! a descendant through in-flow block boxes that make no formatting context of their own) is laid
//! out in the strip as wide as the container's content box (`LayoutPassState::column_spanners`)
//! and shown as a row of its own: the content before it is balanced into a row of columns above
//! it, the content after it into a row below.
//!
//! Not supported: `break-before`/
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
        // pyro: a table cell is a block container too (Chromium makes it a multi-column one).
        DisplayInside::FlowRoot | DisplayInside::TableCell => true,
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
    /// pyro: the outer fragmentation this container was laid out in, as a multi-column container
    /// nested in another's columns (`LayoutPassState::nested_fragmentation`): the outer column
    /// height and how far down its column the box starts.
    pub nested: Option<(f32, f32)>,
    /// pyro: so nested, its first row (not at the top of its outer column) cannot hold its first
    /// column's content without breaking what is kept together or overflowing it: the outer
    /// container breaks before it instead.
    pub pushed: bool,
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
    /// pyro: the container has `column-height: 0`: its columns hold what cannot break whole
    /// (`fill`).
    whole_in_empty_columns: bool,
    /// pyro: see `Breaks::fill` (`degenerate`).
    cut_in_empty_columns: bool,
    /// pyro: line boxes and monolithic boxes a column holds whole even when they are taller than
    /// it, overflowing it (`held_whole`). Merged.
    whole: Vec<(f32, f32)>,
    /// The space between consecutive in-flow blocks (their margins): a column after a break in it
    /// starts at its end. Sorted by start.
    gaps: Vec<(f32, f32)>,
    /// pyro: lines `text-box-trim` trims where they start or end a column (`layout::text_box`):
    /// `(top, bottom, start trim, end trim)`, sorted by top.
    trims: Vec<(f32, f32, f32, f32)>,
    /// pyro: forced breaks (`break-before`/`break-after: column`, `page`, ...), sorted.
    forced: Vec<f32>,
    /// pyro: the multi-column containers in the strip's flow (no other break context between
    /// them and the container) and their tops.
    nested: Vec<(crate::NodeId, f32)>,
}

/// pyro: the rows of a multi-column container with an explicit `column-height` and wrapping
/// columns (css-multicol-2), as Chromium lays them out (measured on Chrome 158 with WPT
/// css-multicol `column-height-006`, `-017`..`-020`): row `k` is the slot `origin + k * stride`,
/// `height` tall, `stride` being the column height plus the row gap. Column content and spanners
/// share a slot: content before a spanner is balanced within what is left of its slot, a spanner
/// follows it at once when it fits in the rest of the slot (or starts a slot; it may then reach
/// past the slot's end, into the gap and the slots after it) and otherwise starts the next slot,
/// and content after a spanner goes on in what is left of the slot the spanner ends in.
#[derive(Clone, Copy)]
struct RowSlots {
    origin: f32,
    height: f32,
    stride: f32,
}

impl RowSlots {
    /// The slot `y` is in or after: its top and its end.
    fn slot(&self, y: f32) -> (f32, f32) {
        if self.stride <= EPSILON {
            return (y, y + self.height);
        }
        let index = ((y - self.origin) / self.stride + 1e-4).floor().max(0.0);
        let top = self.origin + index * self.stride;
        (top, top + self.height)
    }

    /// The top of the slot after the one that starts at `top`.
    fn next(&self, top: f32) -> f32 {
        top + self.stride
    }

    /// Where column content goes that follows what ended at `y`, and the height its first row
    /// has: the rest of the slot `y` is in, or the next slot whole.
    fn content_start(&self, y: f32) -> (f32, f32) {
        let (top, end) = self.slot(y);
        if (y - top).abs() <= EPSILON {
            (top, self.height)
        } else if y < end - EPSILON {
            (y, end - y)
        } else {
            (self.next(top), self.height)
        }
    }

    /// Where a spanner `height` tall goes that follows what ended at `y`.
    fn spanner_top(&self, y: f32, height: f32) -> f32 {
        let (top, end) = self.slot(y);
        if (y - top).abs() <= EPSILON || y + height <= end + EPSILON { y } else { self.next(top) }
    }
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
        let no_height = self.whole_in_empty_columns && height <= EPSILON;
        // pyro: columns of no height of a container whose own lines they show (an inline
        // formatting context root) cut those lines as before: a stopgap that keeps WPT
        // css-overflow `discard-multicol-*` as they were while the `lh` unit resolves to 0 under
        // `line-height: normal` (their `height: 2lh` boxes have no height). Chromium holds lines
        // whole in any column (`multicol-span-all-children-height-003`).
        let degenerate = self.cut_in_empty_columns && height <= EPSILON;
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
                    // pyro: but in columns of `column-height: 0` nothing fits a slice: what
                    // cannot break (a line, a monolithic box) is held whole and overflows its
                    // column (WPT css-multicol `column-height-023`). In any other column it is
                    // cut, as Chromium cuts monolithic content taller than its fragmentainer (WPT
                    // css-contain `contain-size-breaks-001`, `contain-size-multicol-004`), also in
                    // the 1px columns of a container with no height left
                    // (`multicol-span-all-children-height-003`).
                    loose.unwrap_or_else(|| match containing(&self.loose, limit) {
                        Some((a, b)) if no_height && a <= start + EPSILON => b,
                        _ => match containing(&self.whole, limit).filter(|_| !degenerate) {
                            Some((a, b)) if a <= start + EPSILON => b,
                            _ => limit,
                        },
                    })
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
        // pyro: forced breaks split the content into segments, each starting a column: when there
        // are as many as the columns or more, every column holds one segment (or one and the
        // overflow columns after the last), so they are as tall as the tallest (Chromium; WPT
        // css-multicol `multicol-fill-balance-002`, `-040`).
        let mut cuts: Vec<f32> = vec![self.start];
        cuts.extend(self.forced.iter().copied().filter(|&f| f > self.start + EPSILON && f < self.end - EPSILON));
        cuts.push(self.end);
        let segments = cuts.len() - 1;
        let tallest_segment = cuts.windows(2).map(|w| w[1] - w[0]).fold(0.0, f32::max);
        // At most this many columns fit the content whatever their height.
        let fits = count.max(segments.min(MAX_COLUMNS));
        let mut height = if segments >= count { tallest_segment } else { total / count as f32 };
        if let Some(limit) = limit {
            height = height.min(limit);
        }
        for _ in 0..200 {
            let run = self.fill(height, MAX_COLUMNS);
            if run.columns.len() <= fits && run.violations == 0 {
                break;
            }
            let Some(shortage) = run.shortage else {
                // Nothing in the way to grow past: the columns cut through what does not fit.
                if run.columns.len() <= fits {
                    break;
                }
                // pyro: the least height at which they fit (a column cut through empty space
                // moves its break by as much as it grows).
                let top = total.min(limit.unwrap_or(f32::INFINITY));
                let (mut lo, mut hi) = (height, top);
                if self.fill(hi, MAX_COLUMNS).columns.len() > fits {
                    height = top;
                    break;
                }
                for _ in 0..40 {
                    if hi - lo <= EPSILON {
                        break;
                    }
                    let mid = (lo + hi) / 2.0;
                    if self.fill(mid, MAX_COLUMNS).columns.len() <= fits { hi = mid } else { lo = mid }
                }
                height = hi;
                continue;
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
        let tallest = run.columns.iter().take(fits.max(1)).map(|&(a, b)| b - a).fold(0.0, f32::max);
        // pyro: (when what is kept together still breaks, the columns stay as tall as balancing
        // stretched them: Chromium; WPT css-multicol `multicol-nested-020`)
        if run.columns.len() <= fits && run.violations == 0 { tallest.min(height).max(EPSILON) } else { height }
    }
}

/// The number and width of a container's columns (CSS Multi-column Layout 1 §3.4): the content
/// box width `available`, `column-gap` `gap`.
fn column_boxes(count: Option<u32>, width: Option<f32>, available: f32, gap: f32) -> (usize, f32) {
    // pyro: a column width under 1px counts as 1px (Chromium's `ResolveUsedColumnCount`; WPT
    // css-multicol `zero-column-width-layout`).
    let fitting = |width: f32| {
        let pitch = width.max(1.0) + gap;
        ((available + gap) / pitch).floor().max(1.0) as usize
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
        let content_top = pb.top;
        // The column height.
        let definite_height = inputs
            .known_dimensions
            .height
            .or(size.height.maybe_clamp(min_size.height, max_size.height))
            .map(|h| (h - pb_sum.height).max(0.0));
        let limit = definite_height.or(max_size.height.map(|h| (h - pb_sum.height).max(0.0)));
        let fill_auto = matches!((multicol.fill, definite_height), (ColumnFill::Auto, Some(_)))
            && (size.height.is_some() || inputs.known_dimensions.height.is_some());
        let vertical = self.nodes[dom_id].primary_styles().is_some_and(|style| style.writing_mode.is_vertical());
        // pyro: the columns' height when it is known before the content is: what a multi-column
        // container nested in them is fragmented by (`nested_fragmentation`).
        let fragmentainer = (spanning.is_none() && !vertical)
            .then(|| match multicol.height {
                Some(height) if !multicol.wrap => Some(height),
                Some(_) => None,
                None => definite_height.filter(|_| fill_auto),
            })
            .flatten()
            .filter(|&height| height >= 1.0);
        // pyro: a balanced container fragments a nested one by its balanced column height, found
        // again on each pass.
        let balancing = spanning.is_none() && !vertical && multicol.height.is_none() && !fill_auto;
        let fragments_nested = fragmentainer.is_some() || balancing;
        let mut pass = 0;
        // pyro: the tops of nested containers to break before (`ColumnFragments::pushed`).
        let mut pushed_nested: Vec<f32> = Vec::new();
        let (strip, mut breaks) = loop {
            self.multicol_strip_depth += 1;
            let strip = if inline_root {
                self.compute_inline_layout(dom_id, strip_inputs, None)
            } else {
                compute_block_layout(self, node_id, strip_inputs, None)
            };
            self.multicol_strip_depth -= 1;
            // Taffy's block algorithm clamps the strip by the container's `min-height` and
            // `max-height` whatever the sizing mode: under either, the content's end is taken from
            // the laid-out content instead (empty space a `min-height` adds is no content to balance).
            let strip_end = (inline_root || !clamps_block_size).then(|| (strip.size.height - pb.bottom).max(content_top));
            let mut breaks = self.doc.multicol_breaks(dom_id, content_top, strip_end, fragments_nested);
            if !pushed_nested.is_empty() {
                breaks.forced.extend(pushed_nested.iter().copied().filter(|&y| y > breaks.start + EPSILON));
                breaks.forced.sort_by(f32::total_cmp);
                breaks.forced.dedup_by(|a, b| (*a - *b).abs() <= EPSILON);
            }
            pass += 1;
            if breaks.nested.is_empty() || pass > 5 {
                break (strip, breaks);
            }
            // pyro: a multi-column container nested in these columns is fragmented by them: it is
            // told the column height and where in its column it starts, and the strip is laid out
            // again until that holds (a nested container's rows end where these columns do).
            let fragmentainer =
                fragmentainer.or_else(|| balancing.then(|| breaks.balance(count, limit)).filter(|&height| height >= 1.0));
            let columns = fragmentainer.map(|height| breaks.fill(height, MAX_COLUMNS).columns);
            let mut changed = false;
            for &(id, y) in &breaks.nested {
                let wanted = fragmentainer.zip(columns.as_ref()).map(|(height, columns)| {
                    let start = columns.iter().rev().find(|c| c.0 <= y + EPSILON).map_or(breaks.start, |c| c.0);
                    (height, (y - start).clamp(0.0, height))
                });
                let laid_out = self.nodes[id].element_data().and_then(|e| e.layout_data.as_deref()).and_then(|l| l.columns.as_deref());
                let used = laid_out.and_then(|c| c.nested);
                // (Before its top margin: the margin is truncated at the break.)
                let margin_top = y - self.nodes[id].unrounded_layout().margin.top.max(0.0);
                // (Not past `break-before: avoid`, which outweighs what the row breaks.)
                let avoided = self.nodes[id].primary_styles().is_some_and(|style| {
                    use style::values::computed::BreakBetween as B;
                    matches!(style.get_box().break_before, B::Avoid | B::AvoidColumn)
                });
                if laid_out.is_some_and(|c| c.pushed)
                    && !avoided
                    && used.is_some()
                    && !pushed_nested.iter().any(|&p| (p - margin_top).abs() <= EPSILON)
                {
                    pushed_nested.push(margin_top);
                    changed = true;
                }
                self.nested_fragmentation.retain(|e| e.0 != id);
                if let Some(wanted) = wanted {
                    self.nested_fragmentation.push((id, wanted));
                }
                let same = match (used, wanted) {
                    (None, None) => true,
                    (Some(a), Some(b)) => (a.0 - b.0).abs() <= EPSILON && (a.1 - b.1).abs() <= EPSILON,
                    _ => false,
                };
                if !same {
                    changed = true;
                    use taffy::CacheTree as _;
                    let mut at = Some(id);
                    while let Some(node) = at.filter(|&n| n != dom_id) {
                        self.cache_clear(crate::taffy_node_id(node));
                        at = self.nodes[node].layout_parent.get();
                    }
                }
            }
            if !changed {
                break (strip, breaks);
            }
        };
        self.multicol_percent_height = outer_percent_height;
        self.column_spanners = outer_spanners;
        if inputs.run_mode == RunMode::ComputeSize {
            // The strip wrote its boxes' layouts at this width: a cached final layout of the
            // container at another width would leave them there.
            use taffy::CacheTree as _;
            self.cache_clear(node_id);
        }
        breaks.whole_in_empty_columns = multicol.height.is_some_and(|height| height <= EPSILON);
        breaks.cut_in_empty_columns = inline_root;
        // The strip's parts: runs of columns, and the spanners between them (strip ranges).
        let spanners = match spanning {
            Some(_) => self.doc.column_spanner_ranges(dom_id),
            None => Vec::new(),
        };
        // pyro: the outer fragmentation this container is nested in (see `fragmentainer`).
        let nested_ctx = (spanners.is_empty() && multicol.height.is_none() && !vertical)
            .then(|| self.nested_fragmentation.iter().find(|e| e.0 == dom_id).map(|e| e.1))
            .flatten();

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
        // pyro: an explicit `column-height` with wrapping columns lays the rows out on a grid
        // (`RowSlots`), which the spanners take part in.
        let slotted = multicol.height.filter(|_| multicol.wrap && !spanners.is_empty()).map(|height| {
            let height = height.max(0.0);
            RowSlots { origin: content_top, height, stride: height + multicol.row_gap(definite_height) }
        });
        // pyro: nested in another container's columns (`nested_ctx`), the columns make a row in
        // each outer column the box reaches: the first as tall as what is left of its outer
        // column, the others as tall as an outer column, each filled in turn; the row the content
        // ends in is balanced (unless `column-fill: auto`). A definite height ends the rows.
        let mut pushed = false;
        if let Some((outer_height, offset)) = nested_ctx {
            let box_end = definite_height.map(|height| content_top + height);
            let first = outer_height - offset - content_top;
            let (mut top, mut available) = if first >= 1.0 { (content_top, first) } else { (content_top, outer_height) };
            let mut start = breaks.start;
            let mut placed: Vec<(f32, f32, f32, f32)> = Vec::new(); // (start, end, row top, row height)
            for _ in 0..MAX_COLUMNS {
                let row_height = box_end.map_or(available, |end| available.min(end - top).max(1.0));
                let last_box_row = box_end.is_some_and(|end| top + available >= end - EPSILON);
                let rest = Breaks { start, ..breaks.clone() };
                let run = rest.fill(row_height, count + 1);
                if placed.is_empty() && offset > EPSILON && first >= 1.0 {
                    let violates = |a: f32| containing(&rest.strict, a + row_height).is_some_and(|(s, _)| s <= a + EPSILON);
                    pushed = run.columns.first().is_some_and(|&(a, b)| violates(a) || b - a > row_height + EPSILON);
                    // A later column of this row, partway down its outer column, that would break
                    // what is kept together is left empty: what it would hold starts the next
                    // row, a whole outer column tall (WPT css-multicol `multicol-nested-013`).
                    // (Not once the row's first column breaks something itself: then the row
                    // takes lesser breaks too, as Chromium weighs them.)
                    if !last_box_row
                        && run.columns.first().is_some_and(|c| !violates(c.0))
                        && let Some(i) = (1..count.min(run.columns.len())).find(|&i| violates(run.columns[i].0))
                    {
                        placed.extend(run.columns.iter().take(i).map(|&(a, b)| (a, b, top, row_height)));
                        start = run.columns[i].0;
                        top += row_height;
                        available = outer_height;
                        row_top = top;
                        continue;
                    }
                }
                if run.columns.len() <= count || last_box_row {
                    // (`column-fill: auto` without a definite height balances too.)
                    let height = if matches!(multicol.fill, ColumnFill::Balance) || !fill_auto {
                        rest.balance(count, Some(row_height)).min(row_height)
                    } else {
                        row_height
                    };
                    let columns = rest.fill(height, MAX_COLUMNS).columns;
                    let shown = if box_end.is_some() { row_height } else { height };
                    placed.extend(columns.into_iter().map(|(a, b)| (a, b, top, shown)));
                    row_top = top + shown;
                    break;
                }
                placed.extend(run.columns.iter().take(count).map(|&(a, b)| (a, b, top, row_height)));
                start = run.columns[count].0;
                top += row_height;
                available = outer_height;
                row_top = top;
            }
            let mut index = 0;
            let mut previous_top = f32::NAN;
            for (start, end, row, height) in placed {
                index = if row == previous_top { index + 1 } else { 0 };
                previous_top = row;
                let x = column_x(index);
                let start = snap(start);
                // (Not snapped: the rows meet the outer columns' breaks exactly.)
                fragments.push(ColumnFragment {
                    start,
                    end: snap(end),
                    x,
                    width: column_width,
                    top: row,
                    height,
                    offset: Point { x: x - pb.left, y: row - start },
                    spanner: false,
                });
            }
        }
        for part in 0..=spanners.len() {
            if nested_ctx.is_some() {
                break;
            }
            let run_end = spanners.get(part).map_or(breaks.end, |s| s.0);
            if run_end > run_start + EPSILON || (part == spanners.len() && fragments.is_empty()) {
                let run = Breaks { start: run_start, end: run_end, ..breaks.clone() };
                // pyro: rows of an explicit `column-height` (`slotted`, below).
                if let Some(rows) = slotted {
                    let (top, available) = rows.content_start(row_top);
                    // The row the run starts in may be partly taken (after a spanner): its
                    // columns are as tall as what is left of it, the rows after it whole.
                    let mut placed: Vec<(f32, f32, f32, f32)> = Vec::new(); // (start, end, row top, row height)
                    let mut rest = Some(run.clone());
                    let mut row = (top, available);
                    if available < rows.height - EPSILON {
                        let first = run.fill(available, count + 1).columns;
                        rest = first.get(count).map(|&(start, _)| Breaks { start, ..run.clone() });
                        placed.extend(first.into_iter().take(count).map(|(a, b)| (a, b, top, available)));
                        row = (rows.next(rows.slot(top).0), rows.height);
                    }
                    if let Some(rest) = rest {
                        for (i, (a, b)) in rest.fill(rows.height, MAX_COLUMNS).columns.into_iter().enumerate() {
                            placed.push((a, b, row.0 + (i / count.max(1)) as f32 * rows.stride, rows.height));
                        }
                    }
                    // `column-fill: balance` balances the last row within its height.
                    let mut last_height = placed.last().map_or(0.0, |p| p.3);
                    if matches!(multicol.fill, ColumnFill::Balance)
                        && let Some(&(_, _, last_top, limit)) = placed.last()
                    {
                        let from = placed.iter().position(|p| p.2 == last_top).unwrap_or(0);
                        let tail = Breaks { start: placed[from].0, end: run_end, ..breaks.clone() };
                        let height = tail.balance(count, Some(limit)).min(limit);
                        let refilled = tail.fill(height, MAX_COLUMNS).columns;
                        if refilled.len() <= count {
                            placed.truncate(from);
                            placed.extend(refilled.into_iter().map(|(a, b)| (a, b, last_top, height)));
                            last_height = height;
                        }
                    }
                    let mut index = 0;
                    let mut previous_top = f32::NAN;
                    let last_top = placed.last().map_or(top, |p| p.2);
                    for (start, end, row_top_of, height) in placed {
                        index = if row_top_of == previous_top { index + 1 } else { 0 };
                        previous_top = row_top_of;
                        let x = column_x(index);
                        let top = snap(row_top_of);
                        let start = snap(start);
                        fragments.push(ColumnFragment {
                            start,
                            end: snap(end),
                            x,
                            width: column_width,
                            top,
                            height: if row_top_of == last_top { last_height } else { height },
                            offset: Point { x: x - pb.left, y: snap(top - start) },
                            spanner: false,
                        });
                    }
                    row_top = last_top + last_height;
                    if let Some(&(start, end, indent)) = spanners.get(part) {
                        let (start, end) = (snap(start), snap(end));
                        let top = rows.spanner_top(row_top, end - start);
                        fragments.push(ColumnFragment {
                            start,
                            end,
                            x: pb.left,
                            width: content_width,
                            top,
                            height: end - start,
                            offset: Point { x: -indent, y: snap(top - start) },
                            spanner: true,
                        });
                        row_top = top + (end - start);
                        run_start = end;
                    }
                    continue;
                }
                // pyro: `column-wrap: wrap` with `column-height: auto` wraps columns as tall as
                // the container's definite height.
                let fixed_height = multicol.height.or(definite_height.filter(|_| multicol.wrap));
                let column_height = if let Some(height) = fixed_height {
                    height.max(0.0)
                } else if spanners.is_empty() && fill_auto {
                    definite_height.unwrap_or(0.0)
                } else if let Some(max) = limit.filter(|_| spanners.is_empty() && matches!(multicol.fill, ColumnFill::Auto)) {
                    // pyro: `column-fill: auto` under a `max-height` fills the columns in turn up
                    // to it (Chromium; WPT css-multicol `columnfill-auto-max-height-*`).
                    max.min((breaks.end - breaks.start).max(0.0))
                } else if fill_auto && part == spanners.len() {
                    // pyro: `column-fill: auto` fills the columns after the last spanner to the
                    // end of the container's height, unbalanced (only content followed by a
                    // spanner is balanced; WPT css-multicol `no-balancing-after-column-span`).
                    definite_height.map_or(0.0, |height| (height - (row_top - content_top)).max(0.0))
                } else {
                    // pyro: a run between or after spanners balances within what is left of a
                    // definite height (css-multicol-1 § 7.2: the columns after a spanner fit
                    // the rest of the container; overflow makes more columns).
                    let left = definite_height.map(|height| (height - (row_top - content_top)).max(0.0));
                    run.balance(count, if spanners.is_empty() { limit } else { left })
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
            if let Some(&(start, end, indent)) = spanners.get(part) {
                let (start, end) = (snap(start), snap(end));
                // pyro: in rows of an explicit `column-height` a spanner has its place in the
                // rows' grid (`RowSlots::spanner_top`).
                if let Some(rows) = slotted {
                    row_top = rows.spanner_top(row_top, end - start);
                }
                fragments.push(ColumnFragment {
                    start,
                    end,
                    x: pb.left,
                    width: content_width,
                    top: row_top,
                    height: end - start,
                    // pyro: a spanner below a child of the container is as far in as that box
                    // (its padding, border and margin): its row starts at the container's edge.
                    offset: Point { x: -indent, y: snap(row_top - start) },
                    spanner: true,
                });
                row_top += end - start;
                run_start = end;
            }
        }
        // pyro: one row of columns in a box of a definite height: the column boxes (and their
        // rules) are as tall as its content box, however short balancing made the content
        // (Chromium; WPT css-multicol `multicol-rule-nested-balancing-001`).
        if let Some(height) = definite_height
            && spanners.is_empty()
            && nested_ctx.is_none()
            && multicol.height.is_none()
            && !multicol.wrap
        {
            for fragment in &mut fragments {
                fragment.height = fragment.height.max(height);
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

        // pyro: a table cell's `vertical-align` (as `align-content`) moves its columns down in a
        // taller cell (`stylo_taffy::convert::table_cell_vertical_align`).
        let free = height - pb_sum.height - columns_height;
        if free > EPSILON
            && let Some(style) = self.nodes[dom_id].primary_styles()
            && style.clone_display().inside() == DisplayInside::TableCell
            && let Some(align) = stylo_taffy::convert::table_cell_vertical_align(&style)
        {
            let shift = match align.keyword {
                taffy::AlignContentKeyword::Center => snap(free / 2.0),
                taffy::AlignContentKeyword::End | taffy::AlignContentKeyword::FlexEnd => snap(free),
                _ => 0.0,
            };
            for fragment in &mut fragments {
                fragment.top += shift;
                fragment.offset.y += shift;
            }
        }
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

        // pyro: an out-of-flow box placed by its static position (in the strip) is placed where
        // that position is shown: in its column (Chromium places it in the fragment its static
        // position falls in).
        let mut oof_candidates = strip.oof_candidates;
        let columns = ColumnFragments { fragments, nested: nested_ctx, pushed };
        if columns.fragments.len() > 1 && !vertical {
            for candidate in oof_candidates.as_mut_slice() {
                if candidate.containing_block.is_some() {
                    continue;
                }
                let (dx, dy) = columns.shift_at(candidate.static_position.y.area.start);
                let position = &mut candidate.static_position;
                position.x.area.start += dx;
                position.x.area.end += dx;
                position.y.area.start += dy;
                position.y.area.end += dy;
            }
        }

        if inputs.run_mode == RunMode::PerformLayout {
            self.doc.has_multicol = true;
            self.nodes[dom_id].layout_data_mut().columns = Some(Box::new(columns));
        }

        let size = Size { width, height };
        LayoutOutput {
            size,
            scrollable_overflow_rect: overflow,
            baselines: strip.baselines,
            top_margin: CollapsibleMarginSet::from_margin(margin.top),
            bottom_margin: CollapsibleMarginSet::from_margin(margin.bottom),
            margins_can_collapse_through: false,
            oof_candidates,
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
        // pyro: the columns are measured without the spanners (as wide as nothing), which are
        // measured on their own: a spanner's widths are the container's at least, not
        // multiplied by the column count (Chromium; WPT css-multicol `intrinsic-size-002`).
        let dom_id = dom_node_id(node_id);
        let spanners: Vec<crate::NodeId> =
            if inline_root { Vec::new() } else { self.doc.column_spanners_of(dom_id).into_iter().map(|(id, _)| id).collect() };
        let outer_spanners = std::mem::replace(&mut self.column_spanners, (!spanners.is_empty()).then_some((dom_id, 0.0)));
        let mut min = measure(self, AvailableSpace::MinContent);
        let mut max = measure(self, AvailableSpace::MaxContent);
        self.column_spanners = outer_spanners;
        let (mut spanner_min, mut spanner_max) = (0.0f32, 0.0f32);
        for id in spanners {
            let child = crate::taffy_node_id(id);
            let margins = {
                let ls = self.nodes[id].layout_style();
                let margin = ls.margin().resolve_or_zero(None, resolve_calc_value);
                margin.left + margin.right
            };
            for (available, slot) in [(AvailableSpace::MinContent, &mut spanner_min), (AvailableSpace::MaxContent, &mut spanner_max)] {
                let input = LayoutInput {
                    run_mode: RunMode::ComputeSize,
                    sizing_mode: SizingMode::InherentSize,
                    axis: RequestedAxis::Horizontal,
                    known_dimensions: Size::NONE,
                    known_dimensions_are_definite: Size { width: true, height: true },
                    parent_size: inputs.parent_size,
                    available_space: Size { width: available, height: AvailableSpace::MaxContent },
                    vertical_margins_are_collapsible: Line::FALSE,
                };
                use taffy::LayoutPartialTree as _;
                let width = self.compute_child_layout(child, input).size.width + margins;
                *slot = slot.max(width);
            }
        }
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
        (min.max(spanner_min), max.max(spanner_max).max(min.max(spanner_min)))
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
        if node.is_out_of_flow() {
            return inputs;
        }
        let Some(style) = node.primary_styles() else {
            return inputs;
        };
        if !is_spanner_style(&style) || !self.spans_columns_of(container, node_id) {
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

    /// pyro: whether a spanner inside `id` spans the columns of the container above it: `id` is an
    /// in-flow block box that makes no formatting context of its own (a spanner is in the
    /// container's block formatting context, css-multicol-1 § 6) and that a break may go into.
    fn carries_spanners(&self, id: crate::NodeId) -> bool {
        let node = &self.nodes[id];
        if node.is_out_of_flow() {
            return false;
        }
        node.primary_styles().is_some_and(|style| {
            use style::values::specified::box_::Contain;
            let b = style.get_box();
            // (A box that is the containing block of fixed descendants keeps its content to
            // itself: a transform, a filter, layout or paint containment; WPT css-multicol
            // `multicol-span-all-010`, `-017`.)
            let contained = !b.transform.0.is_empty()
                || b.slow_clone_perspective() != style::values::computed::Perspective::None
                || b.slow_clone_contain().intersects(Contain::LAYOUT | Contain::PAINT)
                || !style.get_effects().filter.0.is_empty();
            !contained
                && style.clone_float() == Float::None
                && style.clone_display().inside() == DisplayInside::Flow
                && style.clone_display().outside() == DisplayOutside::Block
                && fragmentable(node, &style)
        })
    }

    /// pyro: whether `id` (of spanner style) is a spanner of `container`: its child, or below it
    /// through boxes that carry spanners.
    fn spans_columns_of(&self, container: crate::NodeId, id: crate::NodeId) -> bool {
        let mut at = self.nodes[id].layout_parent.get();
        // (Deep trees: a spanner's chain to its container is short, or it is none.)
        for _ in 0..64 {
            match at {
                Some(parent) if parent == container => return true,
                Some(parent) if self.carries_spanners(parent) => at = self.nodes[parent].layout_parent.get(),
                _ => return false,
            }
        }
        false
    }

    /// The column spanners of `container` in order: its in-flow block children with
    /// `column-span: all`, and those below its children through boxes that carry spanners
    /// (`carries_spanners`). With each, the offset of its parent's box from the container's
    /// (laid-out positions: zero while the strip is not laid out).
    fn column_spanners_of(&self, container: crate::NodeId) -> Vec<(crate::NodeId, Point<f32>)> {
        let mut found = Vec::new();
        self.collect_column_spanners(container, Point::ZERO, 0, &mut found);
        found
    }

    fn collect_column_spanners(
        &self,
        parent: crate::NodeId,
        offset: Point<f32>,
        depth: usize,
        found: &mut Vec<(crate::NodeId, Point<f32>)>,
    ) {
        let children = self.nodes[parent].layout_children.borrow();
        for id in children.iter().flatten().copied() {
            let node = &self.nodes[id];
            if node.is_out_of_flow() {
                continue;
            }
            if node.primary_styles().is_some_and(|s| is_spanner_style(&s)) {
                found.push((id, offset));
            } else if self.column_span_styles && depth < 64 && self.carries_spanners(id) {
                let location = node.unrounded_layout().location;
                let below = Point { x: offset.x + location.x, y: offset.y + location.y };
                self.collect_column_spanners(id, below, depth + 1, found);
            }
        }
    }

    /// Whether `container` has column spanners (see [`Self::column_spanners_of`]).
    fn has_column_spanners(&self, container: crate::NodeId) -> bool {
        !self.column_spanners_of(container).is_empty()
    }

    /// The strip ranges of `container`'s column spanners, their margins included, in order, and
    /// how far each spanner's parent is from the container's left edge.
    fn column_spanner_ranges(&self, container: crate::NodeId) -> Vec<(f32, f32, f32)> {
        let mut ranges: Vec<(f32, f32, f32)> = Vec::new();
        for (id, offset) in self.column_spanners_of(container) {
            let layout = self.nodes[id].unrounded_layout();
            let y = offset.y + layout.location.y;
            let start = y - layout.margin.top.max(0.0);
            let end = y + layout.size.height + layout.margin.bottom.max(0.0);
            let start = ranges.last().map_or(start, |&(_, previous, _)| start.max(previous));
            ranges.push((start, end.max(start), offset.x));
        }
        ranges
    }

    /// Where the strip of the multi-column container `container` may break, from its laid-out
    /// content: `start..end` its content (`start` its content box's top; `end` the strip's
    /// content end, or `None` to take it from the content: its last in-flow child's bottom
    /// margin edge, its floats' margin edges).
    /// pyro: `nested_breakable`: the multi-column containers nested in the strip's flow are
    /// fragmented by its columns (`Breaks::nested`), not monolithic.
    fn multicol_breaks(&self, container: crate::NodeId, start: f32, end: Option<f32>, nested_breakable: bool) -> Breaks {
        let mut found = Found { nested_breakable, ..Found::default() };
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
            whole_in_empty_columns: false,
            cut_in_empty_columns: false,
            whole: merge(found.whole),
            gaps: found.gaps,
            trims: found.trims,
            forced: found.forced,
            nested: found.nested,
        }
    }

    /// pyro: the multi-column table cells in table box `id` (whose border box starts at strip
    /// position `top`) and their tops, through its internal boxes.
    fn multicol_cells(&self, id: crate::NodeId, top: f32, depth: usize, out: &mut Vec<(crate::NodeId, f32)>) {
        let children = self.nodes[id].layout_children.borrow();
        let Some(children) = children.as_deref() else { return };
        for &child_id in children {
            let child = &self.nodes[child_id];
            let Some(style) = child.primary_styles() else { continue };
            let child_top = top + child.unrounded_layout().location.y;
            if style.clone_display().inside() == DisplayInside::TableCell {
                if is_multicol_style(&style) && !style.writing_mode.is_vertical() {
                    out.push((child_id, child_top));
                }
            } else if depth < 3 && !child.is_out_of_flow() {
                self.multicol_cells(child_id, child_top, depth + 1, out);
            }
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
            let mut lines: Vec<(f32, f32)> = text
                .layout
                .lines()
                .filter(|line| !line.text_range().is_empty() || line.items().next().is_some())
                .map(|line| {
                    let m = line.metrics();
                    (origin + m.block_min_coord / scale, origin + m.block_max_coord / scale)
                })
                .collect();
            // pyro: a line box is whole layout units tall where the block's height adds up the
            // unrounded ones (`line-height: 1.6` at 16px: lines 26px tall in a 25.59px box):
            // a line ends where the next starts, and the last no lower than the content box
            // where it passes it by less than a pixel (that rounding, not content overflowing a
            // box of a set height or a multicol container's), so that the lines of consecutive
            // blocks do not overlap into one that cannot break.
            {
                let layout = node.unrounded_layout();
                let content_bottom = top + layout.size.height - layout.padding.bottom - layout.border.bottom;
                let n = lines.len();
                for index in 0..n {
                    let end = match index + 1 < n {
                        true => lines[index + 1].0,
                        false if lines[index].1 - content_bottom < 1.0 => content_bottom,
                        false => continue,
                    };
                    if end > lines[index].0 + EPSILON {
                        lines[index].1 = lines[index].1.min(end);
                    }
                }
            }
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
            // pyro: a line taller than a column overflows it whole, as in Chromium.
            found.whole.extend_from_slice(&lines);
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
        // pyro: the previous in-flow block has `break-after: avoid`; where a break avoided after it
        // would go instead: its top when it has no content to break between (an empty box,
        // breakable anywhere through its space, is kept whole then), else its bottom.
        let mut avoid_after = false;
        let mut previous_unit_top: f32 = 0.0;
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
                // pyro: `break-before: avoid` (and the previous sibling's `break-after: avoid`)
                // keeps the break between them away where the columns can help it.
                let avoids = |b: style::values::computed::BreakBetween| {
                    use style::values::computed::BreakBetween as B;
                    matches!(b, B::Avoid | B::AvoidColumn)
                };
                let leaf = child.layout_children.borrow().as_deref().is_none_or(|c| c.is_empty())
                    && !child.flags.is_inline_root();
                if let Some((bottom, _)) = previous
                    && (avoids(style.get_box().break_before) || avoid_after)
                {
                    let end = if leaf { child_bottom } else { child_top };
                    found.strict.push((previous_unit_top.min(bottom.min(child_top)) - 2.0 * EPSILON, end + 2.0 * EPSILON));
                }
                avoid_after = avoids(style.get_box().break_after);
                previous_unit_top = if leaf { child_top } else { child_bottom };
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
                // pyro: a monolithic float is not separated from its margins (Chromium does not
                // truncate a float's margins at a break; WPT css-multicol `multicol-fill-balance-037`).
                if !fragmentable(child, &style) {
                    let margin_box = (child_top - layout.margin.top.max(0.0), child_bottom + layout.margin.bottom.max(0.0));
                    found.strict.push(margin_box);
                    found.loose.push(margin_box);
                }
            }
            let nested = is_multicol_style(&style) && !style.writing_mode.is_vertical();
            let mut cells = Vec::new();
            if found.nested_breakable && child.flags.is_table_root() && style.clone_float() == Float::None {
                self.multicol_cells(child_id, child_top, 0, &mut cells);
            }
            if nested && found.nested_breakable && style.clone_float() == Float::None && fragmentable_but_multicol(child, &style) {
                // pyro: fragmented by the columns (`Breaks::nested`): breakable through.
                found.nested.push((child_id, child_top));
            } else if !cells.is_empty() {
                // pyro: a table with multi-column cells is fragmented with them (Chromium
                // fragments tables): breakable through.
                found.nested.append(&mut cells);
            } else if fragmentable(child, &style) {
                if style.get_box().break_inside != BreakWithin::Auto {
                    found.strict.push((child_top, child_bottom));
                }
                // pyro: no break goes through a box's block-start border and padding: a column
                // holds them whole, as Chromium does (WPT css-contain `contain-size-breaks-001`).
                let head = (child_top, child_top + layout.border.top + layout.padding.top);
                if head.1 > head.0 + EPSILON {
                    found.strict.push(head);
                    found.loose.push(head);
                    found.whole.push(head);
                }
                crate::util::ensure_stack(|| self.collect_breaks(child_id, child_top, found, None, trims));
            } else {
                found.strict.push((child_top, child_bottom));
                found.loose.push((child_top, child_bottom));
                if held_whole(child, &style) {
                    found.whole.push((child_top, child_bottom));
                }
                if nested {
                    found.nested.push((child_id, child_top));
                }
            }
        }
    }
}

/// What [`BaseDocument::collect_breaks`] finds in a strip (see [`Breaks`]).
#[derive(Default)]
struct Found {
    strict: Vec<(f32, f32)>,
    /// pyro: see [`Breaks::whole`].
    whole: Vec<(f32, f32)>,
    /// pyro: see [`Breaks::nested`].
    nested: Vec<(crate::NodeId, f32)>,
    /// pyro: see [`BaseDocument::multicol_breaks`].
    nested_breakable: bool,
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

/// pyro: whether multi-column container `node` would be fragmentable if it were no multi-column
/// container (see [`fragmentable`]).
fn fragmentable_but_multicol(node: &crate::Node, style: &ComputedValues) -> bool {
    fragmentable_as(node, style, true)
}

/// Whether a break may go through `node`'s box (a block container that is no scroll container,
/// replaced element, form control or nested multi-column container); others are monolithic.
fn fragmentable(node: &crate::Node, style: &ComputedValues) -> bool {
    fragmentable_as(node, style, false)
}

fn fragmentable_as(node: &crate::Node, style: &ComputedValues, multicol_ok: bool) -> bool {
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
    if scrolls(box_style.overflow_x) || scrolls(box_style.overflow_y) || (!multicol_ok && is_multicol_style(style)) {
        return false;
    }
    // pyro: a box of size containment is monolithic (CSS Contain 2 § 3.1).
    if box_style.slow_clone_contain().contains(style::values::specified::box_::Contain::SIZE) {
        return false;
    }
    matches!(style.clone_display().inside(), DisplayInside::Flow | DisplayInside::FlowRoot)
}

/// pyro: whether monolithic `node` is held whole by a column it is taller than: a scroll
/// container, a replaced element, a form control or a box of size containment, which Chromium
/// never slices in a multi-column container (WPT css-multicol `overflow-unsplittable-*`). The
/// other boxes pyro keeps from breaks (tables, flex and grid containers, nested multi-column
/// containers) are fragmented by Chromium, which a cut at the column's end comes closer to.
fn held_whole(node: &crate::Node, style: &ComputedValues) -> bool {
    let Some(element) = node.element_data() else {
        return false;
    };
    if super::replaced::is_replaced(element)
        || element.text_input_data().is_some()
        || matches!(&*element.name.local, "button" | "select" | "input" | "textarea")
    {
        return true;
    }
    let box_style = style.get_box();
    let scrolls = |o: Overflow| !matches!(o, Overflow::Visible | Overflow::Clip);
    scrolls(box_style.overflow_x)
        || scrolls(box_style.overflow_y)
        || box_style.slow_clone_contain().contains(style::values::specified::box_::Contain::SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn breaks(strict: Vec<(f32, f32)>, loose: Vec<(f32, f32)>, end: f32) -> Breaks {
        Breaks {
            start: 0.0,
            end,
            strict: merge(strict),
            loose: merge(loose),
            whole_in_empty_columns: false,
            cut_in_empty_columns: false,
            whole: Vec::new(),
            gaps: Vec::new(),
            trims: Vec::new(),
            forced: Vec::new(),
            nested: Vec::new(),
        }
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
            nested: None,
            pushed: false,
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
            nested: None,
            pushed: false,
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
            nested: None,
            pushed: false,
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
