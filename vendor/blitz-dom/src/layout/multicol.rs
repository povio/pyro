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
/// pyro: how many column fragments one container makes at most, nested rows included (what is
/// left past them goes to its last column).
const MAX_FRAGMENTS: usize = 10_000;
/// pyro: a container's column fragments as they are made. [`FragmentSink::push`] is the only way
/// one is added: it checks and debits both the container's cap (`MAX_FRAGMENTS`) and the layout
/// pass's budget (`COLUMN_FRAGMENT_BUDGET`) before it adds anything. The last fragment either
/// allows is the rest of the strip from there (spanners and all), and nothing is added after it;
/// with the budget spent a container makes none (its strip is shown as it is, one column).
struct FragmentSink {
    fragments: Vec<ColumnFragment>,
    /// How many more the container may add.
    room: usize,
    /// What is left of the layout pass's budget (`LayoutPassState::column_fragment_budget`).
    budget: usize,
    closed: bool,
    /// The end of the strip, and the first column's left edge and width (the last fragment's).
    strip_end: f32,
    x: f32,
    width: f32,
    pb_left: f32,
}

impl FragmentSink {
    fn new(budget: usize, strip_end: f32, x: f32, width: f32, pb_left: f32) -> Self {
        let room = budget.min(MAX_FRAGMENTS);
        Self { fragments: Vec::new(), room, budget, closed: room == 0, strip_end, x, width, pb_left }
    }

    /// Whether nothing more is added (the last fragment is in, or there was no room at all).
    fn closed(&self) -> bool {
        self.closed
    }

    /// How many more fragments may be added (none once closed).
    fn room(&self) -> usize {
        if self.closed { 0 } else { self.room }
    }

    /// Adds `fragment`, or, where it takes the last room, one fragment holding the rest of the
    /// strip from its start; once closed, nothing.
    fn push(&mut self, fragment: ColumnFragment) {
        if self.closed || self.room == 0 || self.budget == 0 {
            self.closed = true;
            return;
        }
        self.room -= 1;
        self.budget -= 1;
        let fragment = if self.room == 0 || self.budget == 0 {
            self.closed = true;
            let start = fragment.start;
            ColumnFragment {
                start,
                end: f32::INFINITY,
                strip_end: f32::INFINITY,
                x: self.x,
                width: self.width,
                top: fragment.top,
                height: (self.strip_end - start).max(0.0),
                offset: Point { x: self.x - self.pb_left, y: fragment.top - start },
                spanner: false,
                extended: 0.0,
                exact_x: f32::NAN,
                block_rounding_error: 0.0,
            }
        } else {
            fragment
        };
        self.fragments.push(fragment);
    }

    /// The fragments, what is left of the budget, and whether it closed.
    fn finish(self) -> (Vec<ColumnFragment>, usize, bool) {
        (self.fragments, self.budget, self.closed)
    }
}

/// pyro: how many column fragments a layout pass makes in all its multi-column containers, nested
/// ones included (`LayoutPassState::column_fragment_budget`); past them a container makes none,
/// its strip shown as it is (`FragmentSink`; a page cannot make millions: a thousand columns in each of a thousand nested rows).
pub(crate) const COLUMN_FRAGMENT_BUDGET: usize = 200_000;
/// pyro: how many column fragments a layout pass makes in all, in the runs that only measure its
/// multi-column containers (`LayoutPassState::column_measure_budget`): each such run is held to
/// the pass's budget too, but a container measured again and again would make its fragments each
/// time. Past it a measured container makes none (its strip is measured as it is).
pub(crate) const MEASURE_FRAGMENT_BUDGET: usize = 1_000_000;

#[cfg(test)]
thread_local! {
    /// pyro: the fragments measure runs made (tests: `MEASURE_FRAGMENT_BUDGET` holds).
    static MEASURED_FRAGMENTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// pyro: the fragments rect mapping visited (tests: a rect visits the fragments it reaches).
    static MAPPED_FRAGMENTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static SPANNER_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
/// pyro: how many extra strip layouts the multi-column containers of one layout pass make to
/// fragment the ones nested in them (`LayoutPassState::nested_pass_budget`): nesting multiplies
/// them otherwise.
pub(crate) const NESTED_PASS_BUDGET: usize = 256;
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
    pub nested: Option<(f32, f32, usize, f32)>,
    /// pyro: so nested, its first row (not at the top of its outer column) cannot hold its first
    /// column's content without breaking what is kept together or overflowing it: the outer
    /// container breaks before it instead.
    pub pushed: bool,
    /// pyro: where the strip's content ends (the last fragment's real end, which shows to the
    /// strip's end): see `BaseDocument::collect_breaks` on nested containers.
    pub strip_end: f32,
    pub first_after_spanner: std::collections::BTreeMap<crate::NodeId, usize>,
    pub run_wrappers: std::collections::BTreeSet<crate::NodeId>,
    pub wrapper_initial_consumption: std::collections::HashMap<crate::NodeId, f32>,
    continuation_ends: std::collections::HashMap<crate::NodeId, f32>,
    consumed_by_owner: std::collections::HashMap<crate::NodeId, f32>,
}

/// One column of a multi-column container, or a column spanner (`column-span: all`) between
/// rows of columns.
#[derive(Debug, Clone, Copy)]
pub struct ColumnFragment {
    /// The strip range the column shows, `start..end` in the block axis (the first column's
    /// start is `-inf`, the last column's end `+inf`: what overflows the strip's ends shows).
    pub start: f32,
    pub end: f32,
    pub(crate) strip_end: f32,
    /// The column box: its left edge, width, top and height.
    pub x: f32,
    pub width: f32,
    pub top: f32,
    pub height: f32,
    /// What is added to a strip position to show it in this column.
    pub offset: Point<f32>,
    /// A column spanner's row: no column rule beside it.
    pub spanner: bool,
    /// pyro: how much of `height` reaches past the column's own height to the end of a
    /// definite-height box's content box (its last row, for its rules): no content fills it.
    pub extended: f32,
    /// pyro: the column box's left edge before it is snapped to device pixels (`x`): where its
    /// rules are placed in the gaps.
    pub exact_x: f32,
    pub block_rounding_error: f32,
}

impl ColumnFragment {
    /// Whether the column shows strip position `y`.
    #[inline]
    fn shows(&self, y: f32) -> bool {
        y >= self.start && y < self.end
    }
}

impl ColumnFragments {
    pub fn owner_spends_slot(&self, owner: crate::NodeId, end: f32) -> bool {
        self.consumed_by_owner.get(&owner).is_some_and(|start| end > *start)
    }

    pub fn continuation_end(&self, owner: crate::NodeId) -> Option<f32> {
        self.continuation_ends.get(&owner).copied()
    }
    pub fn child_start(&self, child: crate::NodeId, start: f32) -> Option<f32> {
        let Some(first) = self.first_after_spanner.get(&child).map(|&index| &self.fragments[index]) else { return Some(start) };
        (start >= first.start).then_some(if start == first.start { f32::NEG_INFINITY } else { start })
    }

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

    /// pyro: the columns that show a box hoisted out of this container (a stacking context
    /// painted by an ancestor) whose strip range is `top..bottom`: those its range reaches, not
    /// spanners' rows, in strip order. Paint shows it in each (blitz-paint
    /// `draw_hoisted_children`), and hit testing finds it there only (`Node::hoisted_hit_points`).
    pub fn hoisted_columns(&self, top: f32, bottom: f32) -> impl DoubleEndedIterator<Item = &ColumnFragment> {
        let first = self.fragments.partition_point(|f| f.end <= top);
        let last = self.fragments.partition_point(|f| f.start < bottom).max(first);
        self.fragments[first..last].iter().filter(move |f| !f.spanner && top < f.end && bottom > f.start)
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
    /// pyro: the fragments a strip range `from..to` (CSS px) can reach, as a slice range: those
    /// whose range ends at `from` or after and starts at `to` or before (fragments are in strip
    /// order, their starts and ends ascending).
    fn reaching(&self, from: f64, to: f64) -> (usize, usize) {
        let first = self.fragments.partition_point(|f| (f.end as f64) < from);
        let last = self.fragments.partition_point(|f| (f.start as f64) <= to).max(first);
        (first, last)
    }

    pub fn map_rect(&self, rect: kurbo::Rect, scale: f64) -> kurbo::Rect {
        self.map_child_rect(None, rect, scale)
    }

    pub fn map_child_rect(&self, child: Option<crate::NodeId>, rect: kurbo::Rect, scale: f64) -> kurbo::Rect {
        let mut out: Option<kurbo::Rect> = None;
        // pyro: a rect only touching a fragment's range is not in it (a spanner's row starts where
        // the column before it ends: mapped through that column too, it widened the container's
        // scrollable overflow by its offset); an empty rect goes through one fragment.
        let empty = rect.y1 <= rect.y0;
        // (Only the fragments the rect can reach: they are in strip order.)
        let (mut first, mut last) = self.reaching(rect.y0 / scale, rect.y1 / scale);
        if let Some(index) = child.and_then(|id| self.first_after_spanner.get(&id)).copied() {
            first = first.max(index);
            last = last.max(index + 1);
        }
        for f in &self.fragments[first..last] {
            #[cfg(test)]
            MAPPED_FRAGMENTS.with(|m| m.set(m.get() + 1));
            let Some(start) = child.map_or(Some(f.start), |id| self.child_start(id, f.start)) else { continue };
            let y0 = rect.y0.max(start as f64 * scale);
            let y1 = rect.y1.min(f.end as f64 * scale);
            if y1 < y0 || (!empty && y1 <= y0) || (empty && out.is_some()) {
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
    /// pyro: the container has no block size (a size-contained one) and no `column-height`: its
    /// strip goes in one column, not in a run of 1px ones.
    one_column_when_empty: bool,
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
    overflow_breaks: Vec<FlowContinuation>,
    overlapping_starts: Vec<FlowContinuation>,
    overflow_parents: Vec<FlowContinuation>,
    overflow_continuations: std::collections::HashMap<crate::NodeId, f32>,
    parallel_floats: Vec<(crate::NodeId, f32, f32, f32)>,
    /// pyro: the multi-column containers in the strip's flow (no other break context between
    /// them and the container) and their tops.
    nested: Vec<(crate::NodeId, f32)>,
    /// pyro: the baselines of the strip's lines (its inline formatting contexts', not nested
    /// containers'), in tree order: the container's first baseline is the highest a column shows.
    baselines: Vec<f32>,
    /// pyro: the boxes of `box-decoration-break: clone` (their border boxes' block ranges and
    /// block-start border and padding): a column that starts inside one shows its block-start
    /// border and padding again first, so its content is that much lower.
    clones: Vec<(f32, f32, f32)>,
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
    fn plan(self, run: Part<'_>, row_top: f32, count: usize, left: usize, balance: bool) -> Vec<(f32, f32, f32, f32)> {
        let rows = self;
        let (top, available) = rows.content_start(row_top);
        // The row the run starts in may be partly taken (after a spanner): its
        // columns are as tall as what is left of it, the rows after it whole.
        let mut placed: Vec<(f32, f32, f32, f32)> = Vec::new(); // (start, end, row top, row height)
        let mut rest = Some(run);
        let mut row = (top, available);
        if available < rows.height - EPSILON {
            let first = run.fill(available, count + 1).columns;
            rest = first.get(count).map(|&(start, _)| Part { start, ..run });
            placed.extend(first.into_iter().take(count).map(|(a, b)| (a, b, top, available)));
            row = (rows.next(rows.slot(top).0), rows.height);
        }
        if let Some(rest) = rest {
            for (i, (a, b)) in rest.fill(rows.height, left.saturating_sub(placed.len()).max(1)).columns.into_iter().enumerate() {
                placed.push((a, b, row.0 + (i / count.max(1)) as f32 * rows.stride, rows.height));
            }
        }
        // `column-fill: balance` balances the last row within its height.
        if balance
            && let Some(&(_, _, last_top, limit)) = placed.last()
        {
            let from = placed.iter().position(|p| p.2 == last_top).unwrap_or(0);
            let tail = Part { breaks: run.breaks, start: placed[from].0, end: run.end };
            let height = tail.balance(count, Some(limit)).min(limit);
            let refilled = tail.fill(height, left).columns;
            if refilled.len() <= count {
                placed.truncate(from);
                placed.extend(refilled.into_iter().map(|(a, b)| (a, b, last_top, height)));
            }
        }
        placed
    }

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

/// pyro: a part of a strip (between spanners, or from a row on), its breaks shared.
#[derive(Clone, Copy)]
struct Part<'a> {
    breaks: &'a Breaks,
    start: f32,
    end: f32,
}

impl Part<'_> {
    fn fill(&self, height: f32, max_columns: usize) -> FillRun {
        self.breaks.fill_range(self.start, self.end, height, max_columns)
    }

    fn balance(&self, count: usize, limit: Option<f32>) -> f32 {
        self.breaks.balance_range(self.start, self.end, count, limit)
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

#[derive(Debug, Clone, Copy)]
struct FlowContinuation {
    owner: crate::NodeId,
    resume: crate::NodeId,
    origin: f32,
    consumed: f32,
}

impl FlowContinuation {
    fn next_start(self, run: &FillRun, slot_height: Option<f32>) -> Option<f32> {
        let position = self.origin + self.consumed;
        let next = run.columns.partition_point(|&(start, _)| start < position - EPSILON);
        run.columns.get(next).map(|column| column.0).or_else(|| {
            let height = slot_height?;
            run.columns.last().filter(|_| next < MAX_COLUMNS).map(|column| column.0 + height)
        })
    }
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
        self.fill_range(self.start, self.end, height, max_columns)
    }

    /// pyro: [`Self::fill`] for the part `from..to` of the strip (no copy of the breaks per
    /// part or row).
    fn fill_range(&self, from: f32, to: f32, height: f32, max_columns: usize) -> FillRun {
        let no_height = self.whole_in_empty_columns && height <= EPSILON;
        if self.one_column_when_empty && height <= EPSILON {
            return FillRun { columns: vec![(from, to)], shortage: None, violations: 0 };
        }
        let height = height.max(1.0);
        let mut run = FillRun { columns: Vec::new(), shortage: None, violations: 0 };
        let mut start = from;
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
                .then(|| self.forced.get(self.forced.partition_point(|&f| f <= start + EPSILON)).copied().filter(|&f| f < to - EPSILON))
                .flatten();
            if let Some(f) = forced.filter(|&f| f <= limit.max(start + height) + EPSILON) {
                run.columns.push((start, f));
                start = f;
                continue;
            }
            if limit >= to - EPSILON || run.columns.len() + 1 >= max_columns {
                run.columns.push((start, to.max(start)));
                return run;
            }
            // pyro: a break that would leave the column nothing but the margins before it is none
            // (a monolithic box after a margin at the column's start is cut where the column ends,
            // not moved to the next column; WPT css-grid `alignment/grid-item-mixed-baseline-004`).
            let holds_content = |end: &f32| self.break_at(*end).0 > start + EPSILON;
            let (strict, strict_short) = Self::break_before(&self.strict, start, limit);
            let strict = strict.filter(holds_content);
            note(&mut run, strict_short);
            let end = match strict {
                Some(end) => end,
                None => {
                    run.violations += 1;
                    let (loose, loose_short) = Self::break_before(&self.loose, start, limit);
                    let loose = loose.filter(holds_content);
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
                        _ => match containing(&self.whole, limit) {
                            Some((a, b)) if a <= start + EPSILON => b,
                            _ => limit,
                        },
                    })
                }
            };
            let (end, next) = self.break_at(end);
            run.columns.push((start, end.max(start)));
            if next >= to - EPSILON {
                // Nothing but margins left.
                return run;
            }
            start = if next > start + EPSILON { next } else { start + height };
        }
    }

    /// The balanced column height: the least that fits the strip in `count` columns, keeping
    /// together what `strict` keeps where it can, no more than `limit`.
    fn balance(&self, count: usize, limit: Option<f32>) -> f32 {
        self.balance_range(self.start, self.end, count, limit)
    }

    /// pyro: [`Self::balance`] for the part `from..to` of the strip.
    fn balance_range(&self, from: f32, to: f32, count: usize, limit: Option<f32>) -> f32 {
        let total = to - from;
        if total <= EPSILON {
            return 0.0;
        }
        // pyro: forced breaks split the content into segments, each starting a column: when there
        // are as many as the columns or more, every column holds one segment (or one and the
        // overflow columns after the last), so they are as tall as the tallest (Chromium; WPT
        // css-multicol `multicol-fill-balance-002`, `-040`).
        let mut cuts: Vec<f32> = vec![from];
        cuts.extend(self.forced.iter().copied().filter(|&f| f > from + EPSILON && f < to - EPSILON));
        cuts.push(to);
        let segments = cuts.len() - 1;
        let tallest_segment = cuts.windows(2).map(|w| w[1] - w[0]).fold(0.0, f32::max);
        // At most this many columns fit the content whatever their height.
        let fits = count.max(segments.min(MAX_COLUMNS));
        let mut height = if segments >= count { tallest_segment } else { total / count as f32 };
        if let Some(limit) = limit {
            height = height.min(limit);
        }
        for _ in 0..200 {
            let run = self.fill_range(from, to, height, MAX_COLUMNS);
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
                if self.fill_range(from, to, hi, MAX_COLUMNS).columns.len() > fits {
                    height = top;
                    break;
                }
                for _ in 0..40 {
                    if hi - lo <= EPSILON {
                        break;
                    }
                    let mid = (lo + hi) / 2.0;
                    if self.fill_range(from, to, mid, MAX_COLUMNS).columns.len() <= fits { hi = mid } else { lo = mid }
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
        let run = self.fill_range(from, to, height, MAX_COLUMNS);
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
        // pyro: and its scrollbar gutters (classic scrollbars): its columns are in its content box,
        // which they make smaller (Chrome: a 200px `overflow-x: scroll` container's columns are
        // 185px tall, WPT css-overflow `scroll-markers/targeted-column-scroll-marker-selection-001`).
        let gutter = taffy::CoreStyle::scrollbar_gutter(&self.nodes[dom_id].layout_style_in(self.layout_wm));
        let pb = padding + border + gutter;
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
        let is_cell = self.nodes[dom_id]
            .primary_styles()
            .is_some_and(|style| style.clone_display().inside() == DisplayInside::TableCell);
        // (A known height that is no percentage basis does not count: a table cell's, stretched
        // by its row, CSS 2.1 § 17.5.3.)
        let percent_height = inputs
            .known_dimensions
            .height
            .filter(|_| inputs.known_dimensions_are_definite.height || !is_cell)
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
        let mut content_top = pb.top;
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
                Some(height) => Some(height),
                None => definite_height.filter(|_| fill_auto),
            })
            .flatten()
            .filter(|&height| height >= 1.0);
        // pyro: a balanced container fragments a nested one by its balanced column height, found
        // again on each pass.
        let balancing = spanning.is_none() && !vertical && multicol.height.is_none() && !fill_auto;
        let wrapped_rows = multicol.height.filter(|&height| !vertical && multicol.wrap && height >= 1.0);
        let fragments_nested = fragmentainer.is_some() || balancing || wrapped_rows.is_some();
        let mut pass = 0;
        // pyro: the tops of nested containers to break before (`ColumnFragments::pushed`).
        let mut pushed_nested: std::collections::HashMap<crate::NodeId, f32> = std::collections::HashMap::new();
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
            // pyro: a fieldset's columns start below its rendered legend where that is taller than its
            // block-start border (block layout placed the legend over the border: taffy
            // `block_child_is_rendered_legend`).
            if let Some(legend) = self.doc.fieldset_rendered_legend(dom_id) {
                let layout = self.nodes[legend].unrounded_layout();
                let outer = layout.size.height + layout.margin.top + layout.margin.bottom;
                content_top += (outer - (pb.top - padding_top_of(&self.nodes[dom_id], inputs.parent_size.width))).max(0.0);
            }
            let strip_end = (inline_root || !clamps_block_size).then(|| (strip.size.height - pb.bottom).max(content_top));
            let mut breaks = self.doc.multicol_breaks(dom_id, content_top, strip_end, fragments_nested, fragmentainer.is_some());
            if !pushed_nested.is_empty() {
                breaks.forced.extend(pushed_nested.values().copied().filter(|&y| y > breaks.start + EPSILON));
                breaks.forced.sort_by(f32::total_cmp);
                breaks.forced.dedup_by(|a, b| (*a - *b).abs() <= EPSILON);
            }
            pass += 1;
            if pass <= 5 && self.nested_pass_budget > 0
                && let Some(&(outer, offset, _, _)) = self.nested_fragmentation.get(&dom_id)
                && offset > EPSILON && outer > offset + EPSILON
            {
                let room = outer - offset;
                let slots = breaks.fill_range(breaks.start, breaks.end, room, count + 1).columns;
                let row_end = slots.iter().take(count).last().map_or(content_top, |slot| slot.1);
                let mut pending = Vec::new();
                let mut floats: Vec<(f32, f32, f32, f32)> = Vec::new();
                pending.extend(self.nodes[dom_id].layout_children.borrow().iter().flatten().rev().take(MAX_FRAGMENTS).map(|&id| (id, 0.0, 0.0)));
                let mut work = MAX_FRAGMENTS;
                let mut changed = false;
                while let Some((id, parent_x, parent_y)) = pending.pop() {
                    if work == 0 { break; }
                    work -= 1;
                    let child = &self.nodes[id];
                    if child.is_out_of_flow() { continue; }
                    let Some(style) = child.primary_styles() else { continue };
                    let layout = child.unrounded_layout();
                    let x = parent_x + layout.location.x;
                    let y = parent_y + layout.location.y;
                    if style.clone_float() != Float::None {
                        floats.push((x - layout.margin.left, x + layout.size.width + layout.margin.right,
                            y - layout.margin.top, (y + layout.size.height + layout.margin.bottom).min(row_end)));
                    } else if fragmentable(child, &style) && style.clone_display().inside() == DisplayInside::Flow {
                        let item = child.layout_style();
                        if item.contain().establishes_independent_formatting_context()
                            || taffy::BlockItemStyle::align_content(&item).keyword != taffy::AlignContentKeyword::Normal
                            || taffy::BlockItemStyle::establishes_independent_formatting_context(&item)
                            || taffy::LayoutBlockContainer::block_child_establishes_formatting_context(self, crate::taffy_node_id(id))
                        { continue; }
                        let left = work.saturating_sub(pending.len());
                        pending.extend(child.layout_children.borrow().iter().flatten().rev().take(left).map(|&id| (id, x, y)));
                    } else if !fragmentable(child, &style)
                        && y < row_end && layout.size.height <= outer + EPSILON
                    {
                        let index = slots.partition_point(|slot| slot.1 <= y + EPSILON);
                        let Some(&(start, _)) = slots.get(index).filter(|_| index < count) else { continue };
                        if layout.size.height <= room - (y - start).max(0.0) + EPSILON { continue; }
                        if floats.len() > work { break; }
                        work -= floats.len();
                        let floor = floats.iter().filter(|&&(left, right, top, bottom)| {
                            x < right - EPSILON && x + layout.size.width > left + EPSILON && y < bottom - EPSILON && y + layout.size.height > top + EPSILON
                        }).map(|rect| rect.3).fold(y, f32::max);
                        if floor > y + EPSILON {
                            drop(style);
                            self.spanner_floors.insert(id, floor - parent_y);
                            use taffy::CacheTree as _;
                            let mut at = Some(id);
                            while let Some(node) = at.filter(|&n| n != dom_id) {
                                self.cache_clear(crate::taffy_node_id(node));
                                at = self.nodes[node].layout_parent.get();
                            }
                            changed = true;
                        }
                    }
                }
                if changed {
                    self.nested_pass_budget -= 1;
                    continue;
                }
            }
            if let Some(height) = fragmentainer && pass <= 5 && self.nested_pass_budget > 0 && !breaks.overflow_parents.is_empty() {
                let run = breaks.fill(height, MAX_COLUMNS);
                let mut changed = false;
                for &(id, parent_top, top, bottom) in &breaks.parallel_floats {
                    let index = run.columns.partition_point(|&(_, end)| end <= top + EPSILON);
                    if let Some(&(start, end)) = run.columns.get(index) && top > start + EPSILON && bottom > end + EPSILON && bottom - top <= height + EPSILON && let Some(&(next, _)) = run.columns.get(index + 1) {
                        let floor = next - parent_top;
                        self.spanner_floors.insert(id, floor);
                        use taffy::CacheTree as _;
                        let mut at = Some(id);
                        while let Some(node) = at.filter(|&n| n != dom_id) {
                            self.cache_clear(crate::taffy_node_id(node));
                            at = self.nodes[node].layout_parent.get();
                        }
                        changed = true;
                    }
                }
                let mut parents = std::collections::BTreeMap::new();
                for token in &breaks.overflow_parents {
                    let start = token.origin + token.consumed;
                    let entry = parents.entry(token.owner).or_insert((token.origin, start));
                    entry.1 = entry.1.min(start);
                }
                for (id, (origin, parallel_start)) in parents {
                    let bottom = origin + self.nodes[id].unrounded_layout().size.height;
                    let prior = self.column_space_used.get(&id).copied().unwrap_or(0.0);
                    let mut used = 0.0;
                    let short = &run.columns[..run.columns.len().saturating_sub(1)];
                    let first = short.partition_point(|&(_, end)| end <= parallel_start);
                    for &(start, end) in &short[first..] {
                        let left = bottom + prior - used - end;
                        if left <= EPSILON { break; }
                        used += (height - (end - start)).clamp(0.0, left);
                    }
                    if (prior - used).abs() > EPSILON {
                        self.column_space_used.insert(id, used);
                        use taffy::CacheTree as _;
                        let mut at = Some(id);
                        while let Some(node) = at.filter(|&n| n != dom_id) {
                            self.cache_clear(crate::taffy_node_id(node));
                            at = self.nodes[node].layout_parent.get();
                        }
                        changed = true;
                    }
                }
                if changed {
                    self.nested_pass_budget -= 1;
                    continue;
                }
            }
            if pass <= 5 && self.nested_pass_budget > 0 && !breaks.overflow_breaks.is_empty() {
                let slot_height = self.nested_fragmentation.get(&dom_id).map(|ctx| ctx.0).or(fragmentainer);
                let height = slot_height.unwrap_or_else(|| breaks.balance(count, limit));
                let run = breaks.fill(height, MAX_COLUMNS);
                let mut changed = false;
                for token in &breaks.overflow_breaks {
                    let id = token.resume;
                    if let Some(boundary) = token.next_start(&run, slot_height) {
                        let floor = boundary - token.origin;
                        if floor > self.nodes[id].unrounded_layout().location.y + EPSILON {
                            self.spanner_floors.insert(id, floor);
                            use taffy::CacheTree as _;
                            let mut at = Some(id);
                            while let Some(node) = at.filter(|&n| n != dom_id) {
                                self.cache_clear(crate::taffy_node_id(node));
                                at = self.nodes[node].layout_parent.get();
                            }
                            changed = true;
                        }
                    }
                }
                if changed {
                    self.nested_pass_budget -= 1;
                    continue;
                }
            }
            // pyro: a column spanner comes after the content before it, also what overflows the
            // boxes before it (Chromium lays that content out in the row of columns the spanner
            // ends; WPT css-multicol `multicol-span-all-children-height-013`): it is pushed below
            // it (`block_child_min_top`) and the strip laid out again.
            let floors_changed = spanning.is_some()
                && pass <= 5
                && self.nested_pass_budget > 0
                && self.update_spanner_floors(dom_id);
            if floors_changed {
                self.nested_pass_budget -= 1;
                continue;
            }
            if breaks.nested.is_empty() || pass > 5 || self.nested_pass_budget == 0 {
                break (strip, breaks);
            }
            // pyro: a multi-column container nested in these columns is fragmented by them: it is
            // told the column height and where in its column it starts, and the strip is laid out
            // again until that holds (a nested container's rows end where these columns do).
            let fragmentainer =
                fragmentainer.or_else(|| balancing.then(|| breaks.balance(count, limit)).filter(|&height| height >= 1.0));
            let columns = fragmentainer.map(|height| breaks.fill(height, MAX_COLUMNS).columns);
            let tail = fragmentainer.zip(columns.as_ref()).and_then(|(height, columns)| {
                if !multicol.wrap || !matches!(multicol.fill, ColumnFill::Balance) || columns.is_empty() { return None; }
                let index = (columns.len() - 1) / count * count;
                let start = columns[index].0;
                let balanced = breaks.balance_range(start, breaks.end, count, Some(height)).min(height).max(1.0);
                let pieces = breaks.fill_range(start, breaks.end, balanced, count + 1).columns;
                (pieces.len() <= count).then_some((index, start, balanced, pieces))
            });
            let mut planned = Vec::new();
            if let Some(height) = wrapped_rows.filter(|_| spanning.is_some()) {
                let rows = RowSlots { origin: content_top, height, stride: height + multicol.row_gap(definite_height) };
                let spans = self.doc.column_spanner_ranges(dom_id);
                let leads = self.doc.column_spanner_leads(dom_id);
                let ids = self.doc.column_spanners_of(dom_id);
                let mut start = content_top;
                let mut top = content_top;
                for part in 0..=spans.len() {
                    let lift = ids.get(part).map_or(0.0, |(id, _)| (-self.nodes[*id].unrounded_layout().margin.top).max(0.0));
                    let end = spans.get(part).map_or(breaks.end, |span| span.0 + lift);
                    if end > start + EPSILON {
                        let pieces = rows.plan(Part { breaks: &breaks, start, end }, top, count, MAX_COLUMNS.saturating_sub(planned.len()), matches!(multicol.fill, ColumnFill::Balance));
                        if let Some(last) = pieces.last() {
                            top = last.2 + last.3;
                            let tail = pieces.partition_point(|piece| piece.2 < last.2);
                            for (index, piece) in pieces.iter().enumerate() {
                                planned.push((piece.0, piece.1, piece.3, tail.saturating_sub(index), last.3));
                            }
                        }
                    }
                    if planned.len() >= MAX_COLUMNS { break; }
                    if let Some(&(before, after, _)) = spans.get(part) {
                        top = rows.spanner_top(top, after - before) - lift + after - before;
                        start = after - leads.get(part).copied().unwrap_or(0.0);
                    }
                }
            }
            let mut changed = false;
            for &(id, y) in &breaks.nested {
                let slot = planned.partition_point(|piece| piece.0 <= y + EPSILON).checked_sub(1).and_then(|index| planned.get(index)).filter(|piece| y < piece.1 + EPSILON);
                let wanted = slot.map(|&(start, _, height, left, tail)| (height, (y - start).clamp(0.0, height), left, tail)).or_else(|| fragmentainer.zip(columns.as_ref()).map(|(height, columns)| {
                    let index = columns.partition_point(|c| c.0 <= y + EPSILON).saturating_sub(1);
                    let start = columns.get(index).map_or(breaks.start, |c| c.0);
                    if let Some((tail_index, tail_start, balanced, pieces)) = &tail {
                        if y >= *tail_start - EPSILON {
                            let index = pieces.partition_point(|piece| piece.0 <= y + EPSILON).saturating_sub(1);
                            let start = pieces.get(index).map_or(*tail_start, |piece| piece.0);
                            return (*balanced, (y - start).clamp(0.0, *balanced), 0, *balanced);
                        }
                        return (height, (y - start).clamp(0.0, height), tail_index.saturating_sub(index), *balanced);
                    }
                    (height, (y - start).clamp(0.0, height), usize::MAX, height)
                }));
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
                    && !pushed_nested.contains_key(&id)
                {
                    pushed_nested.insert(id, margin_top);
                    changed = true;
                }
                match wanted {
                    Some(wanted) => self.nested_fragmentation.insert(id, wanted),
                    None => self.nested_fragmentation.remove(&id),
                };
                let same = match (used, wanted) {
                    (None, None) => true,
                    (Some(a), Some(b)) => (a.0 - b.0).abs() <= EPSILON && (a.1 - b.1).abs() <= EPSILON && a.2 == b.2 && a.3 == b.3,
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
            self.nested_pass_budget -= 1;
        };
        self.multicol_percent_height = outer_percent_height;
        self.column_spanners = outer_spanners;
        if inputs.run_mode == RunMode::ComputeSize {
            // The strip wrote its boxes' layouts at this width: a cached final layout of the
            // container at another width would leave them there.
            use taffy::CacheTree as _;
            self.cache_clear(node_id);
        }
        // pyro: so does a container of no definite height (a size-contained one): Chrome lays out no
        // run of 1px overflow columns (scrollWidth 752, not 8242: WPT css-contain
        // `contain-size-multicol-002`, where classic scrollbars show the overflow). Classic
        // scrollbars only, so that overlay mode keeps its results.
        breaks.whole_in_empty_columns = multicol.height.is_some_and(|height| height <= EPSILON);
        // (Not an authored `height: 0`: Chrome makes overflow columns there, css-multicol
        // `multicol-zero-height-001`.)
        breaks.one_column_when_empty = multicol.height.is_none()
            && size.height.is_none()
            && definite_height.is_some_and(|h| h <= EPSILON);
        // The strip's parts: runs of columns, and the spanners between them (strip ranges).
        let spanners = match spanning {
            Some(_) => self.doc.column_spanner_ranges(dom_id),
            None => Vec::new(),
        };
        let spanner_leads = match spanning {
            Some(_) => self.doc.column_spanner_leads(dom_id),
            None => Vec::new(),
        };
        // pyro: how far each spanner's negative top margin pulls its row up over the row before
        // it (that row still ends where its content does: Chromium; WPT css-multicol
        // `multicol-span-all-children-height-010`).
        let spanner_lifts: Vec<f32> = match spanning {
            Some(_) => self.doc.column_spanners_of(dom_id).into_iter().map(|(id, _)| (-self.nodes[id].unrounded_layout().margin.top).max(0.0)).collect(),
            None => Vec::new(),
        };
        let lift = |part: usize| spanner_lifts.get(part).copied().unwrap_or(0.0);
        // pyro: the outer fragmentation this container is nested in (see `fragmentainer`).
        let nested_ctx = (spanners.is_empty() && multicol.height.is_none() && !vertical)
            .then(|| self.nested_fragmentation.get(&dom_id).copied())
            .flatten();
        // pyro: so nested, with column spanners.
        let nested_span_ctx = (!spanners.is_empty() && (multicol.height.is_none() || multicol.wrap) && !vertical)
            .then(|| self.nested_fragmentation.get(&dom_id).copied())
            .flatten();
        // pyro: so nested with an explicit `column-height` and wrapping rows (css-multicol-2).
        let nested_rows_ctx = (spanners.is_empty() && multicol.height.is_some() && multicol.wrap && !vertical)
            .then(|| self.nested_fragmentation.get(&dom_id).copied())
            .flatten();

        // The fragments, row by row: as many columns as each run of content needs, past the
        // container's end if it must.
        let scale = self.doc.viewport.scale();
        let snap = |v: f32| (v * scale).round() / scale;
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
        // pyro: the fragments this container makes, within its cap and the pass's budget (the only
        // place they are added: `FragmentSink::push`).
        // (A run that only measures takes its fragments from the measure budget too.)
        let measuring = inputs.run_mode != RunMode::PerformLayout;
        let start_budget = if measuring {
            self.column_fragment_budget.min(self.column_measure_budget)
        } else {
            self.column_fragment_budget
        };
        let mut sink = FragmentSink::new(start_budget, breaks.end, column_x(0), column_width, pb.left);
        let mut pushed = false;
        // pyro: the rows a nested container of `box-decoration-break: clone` adds after its first,
        // each with its borders and padding repeated (`nested_ctx` below; its height grows by them).
        let mut clone_rows = 0usize;
        let mut clone_insets = 0.0f32;
        if let Some((outer_height, offset, mut full_slots, tail_height)) = nested_ctx {
            let box_end_of_content = definite_height.map(|height| content_top + height);
            // pyro: of `box-decoration-break: clone`, each row (each outer fragment of the box)
            // ends with the box's block-end border and padding, and each after the first starts
            // with its block-start ones: the rows are that much shorter, and end when the content
            // height is used up, not where the box's own height ends (Chromium; WPT css-multicol
            // `multicol-breaking-004`).
            let clone = self.nodes[dom_id]
                .primary_styles()
                .is_some_and(|style| style.get_border().box_decoration_break == style::values::computed::BoxDecorationBreak::Clone);
            let (clone_top, clone_bottom) = if clone { (pb.top, pb.bottom) } else { (0.0, 0.0) };
            clone_insets = clone_top + clone_bottom;
            // (As many fragments as its content height needs, rows of it or not: the first as
            // much as is left of its outer column, the others a whole one, less the decorations.)
            if let (true, Some(height)) = (clone, definite_height) {
                let first_left = outer_height - offset - content_top;
                let first_room = (if first_left >= 1.0 { first_left } else { outer_height }) - clone_bottom;
                let room = outer_height - clone_top - clone_bottom;
                if first_room >= 1.0 && room >= 1.0 && height > first_room + EPSILON {
                    clone_rows = (((height - first_room) / room - EPSILON).ceil() as usize).min(MAX_FRAGMENTS);
                }
            }
            let mut consumed = 0.0f32;
            let first = outer_height - offset - content_top;
            let (mut top, mut available) = if first >= 1.0 { (content_top, first) } else { (content_top, outer_height) };
            let mut start = breaks.start;
            let end = breaks.end;
            let mut placed: Vec<(f32, f32, f32, f32)> = Vec::new(); // (start, end, row top, row height)
            let mut last_row_extended = (f32::NAN, 0.0f32);
            loop {
                let box_end = box_end_of_content.map(|end| if clone { top + (end - content_top - consumed) } else { end });
                // (What the row may hold: its outer column's room less the repeated decorations.)
                let room = (available - clone_bottom - if !placed.is_empty() { clone_top } else { 0.0 }).max(1.0);
                let row_height = box_end.map_or(room, |end| room.min(end - top).max(1.0));
                full_slots = full_slots.saturating_sub(1);
                let next_height = if full_slots == 0 { tail_height } else { outer_height };
                // (Past the fragment cap, what is left goes to one last column.)
                if placed.len() + count + 1 >= sink.room() {
                    placed.push((start, end, top, row_height));
                    row_top = top + row_height;
                    break;
                }
                let last_box_row = box_end.is_some_and(|end| top + room >= end - EPSILON);
                let run = breaks.fill_range(start, end, row_height, count + 1);
                if placed.is_empty() && offset > EPSILON && first >= 1.0 {
                    let violates = |a: f32| containing(&breaks.strict, a + row_height).is_some_and(|(s, _)| s <= a + EPSILON);
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
                        // (Its columns from there are empty: a box broken there fills them, as
                        // Chromium shows it; WPT css-multicol `multicol-nested-016`.)
                        let at = run.columns[i].0;
                        placed.extend((i..count).map(|_| (at, at, top, row_height)));
                        start = at;
                        consumed += row_height;
                        top += row_height + clone_bottom + clone_top;
                        available = next_height;
                        row_top = top;
                        continue;
                    }
                }
                if run.columns.len() <= count || last_box_row {
                    let sequential = matches!(multicol.fill, ColumnFill::Auto) && !fill_auto;
                    let height = if (matches!(multicol.fill, ColumnFill::Balance) || !fill_auto) && !sequential {
                        breaks.balance_range(start, end, count, Some(row_height)).min(row_height)
                    } else {
                        row_height
                    };
                    let columns = breaks.fill_range(start, end, height, sink.room().saturating_sub(placed.len()).max(1)).columns;
                    let content = columns.iter().map(|&(a, b)| b - a).fold(0.0f32, f32::max).min(row_height);
                    let height = if sequential && columns.len() == 1 { content } else { height };
                    let shown = if box_end.is_some() { row_height } else { height };
                    placed.extend(columns.into_iter().map(|(a, b)| (a, b, top, shown)));
                    // (The row reaches past its balanced columns to the end of the box.)
                    last_row_extended = (top, shown - height);
                    row_top = top + shown;
                    break;
                }
                placed.extend(run.columns.iter().take(count).map(|&(a, b)| (a, b, top, row_height)));
                start = run.columns[count].0;
                consumed += row_height;
                top += row_height + clone_bottom + clone_top;
                available = next_height;
                row_top = top;
            }
            let mut index = 0;
            let mut previous_top = f32::NAN;
            for (start, end, row, height) in placed {
                let extended = if row == last_row_extended.0 { last_row_extended.1.max(0.0) } else { 0.0 };
                index = if row == previous_top { index + 1 } else { 0 };
                previous_top = row;
                let x = column_x(index);
                let start = snap(start);
                // (Not snapped: the rows meet the outer columns' breaks exactly.)
                sink.push(ColumnFragment {
                    start,
                    end: snap(end),
                    strip_end: end,
                    x,
                    width: column_width,
                    top: row,
                    height,
                    offset: Point { x: x - pb.left, y: row - start },
                    spanner: false,
                    extended,
                    exact_x: f32::NAN,
                    block_rounding_error: 0.0,
                });
            }
        }
        // pyro: nested in another container's columns with rows of an explicit `column-height`
        // (`nested_rows_ctx`): rows that height, `row-gap` apart, each filled in turn; a row that
        // does not fit what is left of its outer column starts the next one, the gap before it
        // truncated (Chromium; WPT css-multicol `column-height-027`, `-028`).
        if let (Some((outer_height, offset, _, _)), Some(row_height)) = (nested_rows_ctx, multicol.height) {
            let row_height = row_height.max(1.0);
            let row_gap = multicol.row_gap(definite_height);
            let end = breaks.end;
            let mut start = breaks.start;
            let mut top = content_top;
            let mut placed = 0usize;
            // What is left of the current row (a row taller than an outer column is cut there and
            // goes on at the top of the next, its pieces filled in turn: `column-height-025`).
            let mut row_left = row_height;
            // Where the outer column that holds `y` ends (`y` at a boundary: the one it starts).
            let boundary = |y: f32| ((offset + y) / outer_height + 1e-4).floor() * outer_height + outer_height - offset;
            loop {
                // (A new row after a gap: the outer column the gap starts in; a gap that reaches
                // past its end is truncated there.)
                let fresh = row_left == row_height;
                let mut column_end = boundary(if fresh && placed > 0 { top - row_gap } else { top });
                if top >= column_end - EPSILON {
                    top = column_end;
                    column_end = boundary(top);
                }
                let at_column_start = (top - (column_end - outer_height)).abs() <= EPSILON;
                if fresh && top + row_height > column_end + EPSILON && !at_column_start {
                    // A row that does not fit what is left of its outer column starts the next.
                    top = column_end;
                    continue;
                }
                let piece = row_left.min(column_end - top).max(1.0);
                let last = count + 1 >= sink.room();
                let run = breaks.fill_range(start, end, piece, if last { 1 } else { count + 1 });
                for (index, &(a, b)) in run.columns.iter().take(count).enumerate() {
                    let x = column_x(index);
                    let a = snap(a);
                    sink.push(ColumnFragment {
                        start: a,
                        end: snap(b),
                        strip_end: b,
                        x,
                        width: column_width,
                        top,
                        height: piece,
                        offset: Point { x: x - pb.left, y: top - a },
                        spanner: false,
                        extended: 0.0,
                        exact_x: f32::NAN,
                        block_rounding_error: 0.0,
                    });
                    placed += 1;
                }
                row_top = top + piece;
                match run.columns.get(count) {
                    Some(&(next, _)) if !last && next < end - EPSILON => {
                        start = next;
                        if piece < row_left - EPSILON {
                            // The row goes on in the next outer column.
                            row_left -= piece;
                            top = row_top;
                        } else {
                            row_left = row_height;
                            top = row_top + row_gap;
                        }
                    }
                    _ => break,
                }
            }
        }
        // pyro: nested in another container's columns, with spanners (`nested_span_ctx`): its
        // parts in turn from the top, each run of columns in rows that end where the outer
        // columns do (the run's last row balanced), each spanner after the run before it, cut
        // where the outer columns end (Chromium; WPT css-multicol `spanner-fragmentation-*`).
        if let Some((outer_height, offset, _, _)) = nested_span_ctx {
            let boundary = |y: f32| ((offset + y) / outer_height + 1e-4).floor() * outer_height + outer_height - offset;
            // (Where `y` is not at the top of an outer column, the next one's top.)
            let next_column = |y: f32| {
                let end = boundary(y);
                if (y - (end - outer_height)).abs() <= EPSILON { y } else { end }
            };
            // pyro: a spanner's forced breaks are breaks of the outer columns, where it is
            // (Chromium; WPT css-multicol `spanner-fragmentation-010`, `-012`).
            let forced: Vec<(bool, bool)> = self
                .doc
                .column_spanners_of(dom_id)
                .into_iter()
                .map(|(id, _)| {
                    self.nodes[id].primary_styles().map_or((false, false), |style| {
                        use style::values::computed::BreakBetween as B;
                        let forces = |b: B| matches!(b, B::Column | B::Always);
                        (forces(style.get_box().break_before), forces(style.get_box().break_after))
                    })
                })
                .collect();
            let mut top = content_top;
            let mut run_start = breaks.start;
            let slots_at = |y: f32| slotted.map(|rows| RowSlots { origin: content_top.max(boundary(y) - outer_height), ..rows });
            let fresh_row = |y: f32, rows: RowSlots| {
                let end = boundary(y);
                if y + rows.height > end + EPSILON && y > end - outer_height + EPSILON { end } else { y }
            };
            'parts: for part in 0..=spanners.len() {
                if sink.closed() {
                    break;
                }
                let run_end = spanners.get(part).map_or(breaks.end, |s| s.0 + lift(part));
                let mut start = run_start;
                while run_end > start + EPSILON {
                    if sink.closed() {
                        break 'parts;
                    }
                    let mut available = boundary(top) - top;
                    if let Some(rows) = slots_at(top) {
                        let (next, room) = rows.content_start(top);
                        top = if room == rows.height { fresh_row(next.min(boundary(top)), rows) } else { next };
                        available = room.min(boundary(top) - top);
                    }
                    if slotted.is_none() && available + EPSILON < outer_height
                        && containing(&breaks.loose, start + available).is_some_and(|(a, b)| a <= start + EPSILON && b - start <= outer_height)
                    {
                        top = boundary(top);
                        available = outer_height;
                    }
                    let available = available.max(1.0);
                    let run = breaks.fill_range(start, run_end, available, count + 1);
                    let (columns, height, next) = if run.columns.len() <= count {
                        let height = if part == spanners.len() && matches!(multicol.fill, ColumnFill::Auto) && top > content_top + EPSILON {
                            run.columns.iter().map(|(a, b)| b - a).fold(0.0, f32::max).min(available)
                        } else {
                            breaks.balance_range(start, run_end, count, Some(available)).min(available)
                        };
                        (breaks.fill_range(start, run_end, height, count).columns, height, None)
                    } else {
                        (run.columns[..count].to_vec(), available, Some(run.columns[count].0))
                    };
                    for (index, (a, b)) in columns.into_iter().enumerate() {
                        let x = column_x(index);
                        let a = snap(a);
                        sink.push(ColumnFragment {
                            start: a,
                            end: snap(b),
                            strip_end: b,
                            x,
                            width: column_width,
                            top,
                            height,
                            offset: Point { x: x - pb.left, y: top - a },
                            spanner: false,
                            extended: 0.0,
                            exact_x: f32::NAN,
                            block_rounding_error: 0.0,
                        });
                    }
                    top += height;
                    match next {
                        Some(next) => start = next,
                        None => break,
                    }
                }
                if let Some(&(start, end, indent)) = spanners.get(part) {
                    let strip_end = end;
                    let (start, end) = (snap(start), snap(end));
                    let (before, after) = forced.get(part).copied().unwrap_or((false, false));
                    if before && top > content_top + EPSILON {
                        top = next_column(top);
                    }
                    if let Some(rows) = slots_at(top) {
                        let next = rows.spanner_top(top, end - start);
                        if next > top + EPSILON {
                            top = fresh_row(next.min(boundary(top)), rows);
                        }
                    }
                    let spanner_top = top - lift(part);
                    sink.push(ColumnFragment {
                        start,
                        end,
                        strip_end,
                        x: pb.left,
                        width: content_width,
                        top: spanner_top,
                        height: end - start,
                        offset: Point { x: pb.left - indent, y: spanner_top - start },
                        spanner: true,
                        extended: 0.0,
                        exact_x: f32::NAN,
                        block_rounding_error: 0.0,
                    });
                    top = spanner_top + (end - start);
                    // (After the last of its content the break is after the container, the outer
                    // container's to make.)
                    let last = part + 1 == spanners.len() && breaks.end <= spanners[part].1 + EPSILON;
                    if after && !last {
                        top = next_column(top);
                    }
                    run_start = spanners[part].1 - spanner_leads.get(part).copied().unwrap_or(0.0);
                }
            }
            row_top = top;
        }
        for part in 0..=spanners.len() {
            if nested_ctx.is_some() || nested_rows_ctx.is_some() || nested_span_ctx.is_some() {
                break;
            }
            // pyro: once the cap is reached the rest of the strip, spanners and all, is in the
            // last fragment (`FragmentSink`).
            if sink.closed() {
                break;
            }
            let run_end = spanners.get(part).map_or(breaks.end, |s| s.0 + lift(part));
            if run_end > run_start + EPSILON || (part == spanners.len() && sink.fragments.is_empty()) {
                // (The part `run_start..run_end` of the strip: no copy of the breaks per part.)
                let run = Part { breaks: &breaks, start: run_start, end: run_end };
                // (What is left of the container's fragment cap: one cap for all its parts.)
                let left = sink.room().max(1);
                // pyro: rows of an explicit `column-height` (`slotted`, below).
                if let Some(rows) = slotted {
                    let placed = rows.plan(run, row_top, count, left, matches!(multicol.fill, ColumnFill::Balance));
                    let (top, _) = rows.content_start(row_top);
                    let last_height = placed.last().map_or(0.0, |piece| piece.3);
                    let mut index = 0;
                    let mut previous_top = f32::NAN;
                    let last_top = placed.last().map_or(top, |p| p.2);
                    for (start, end, row_top_of, height) in placed {
                        index = if row_top_of == previous_top { index + 1 } else { 0 };
                        previous_top = row_top_of;
                        let x = column_x(index);
                        let top = snap(row_top_of);
                        let start = snap(start);
                        sink.push(ColumnFragment {
                            start,
                            end: snap(end),
                            strip_end: end,
                            x,
                            width: column_width,
                            top,
                            height: if row_top_of == last_top { last_height } else { height },
                            offset: Point { x: x - pb.left, y: snap(top - start) },
                            spanner: false,
                            extended: 0.0,
                            exact_x: f32::NAN,
                            block_rounding_error: 0.0,
                        });
                    }
                    row_top = last_top + last_height;
                    if let Some(&(start, end, indent)) = spanners.get(part) {
                        let strip_end = end;
                        let (start, end) = (snap(start), snap(end));
                        let top = rows.spanner_top(row_top, end - start) - lift(part);
                        sink.push(ColumnFragment {
                            start,
                            end,
                            strip_end,
                            x: pb.left,
                            width: content_width,
                            top,
                            height: end - start,
                            offset: Point { x: pb.left - indent, y: snap(top - start) },
                            spanner: true,
                            extended: 0.0,
                            exact_x: f32::NAN,
                            block_rounding_error: 0.0,
                        });
                        row_top = top + (end - start);
                        // (Where the spanner ends in the strip, unsnapped: what follows it starts there.)
                        run_start = spanners[part].1 - spanner_leads.get(part).copied().unwrap_or(0.0);
                    }
                    continue;
                }
                // pyro: `column-wrap: wrap` with `column-height: auto` wraps columns as tall as
                // the container's definite height.
                let fixed_height = multicol.height.or(definite_height.filter(|_| multicol.wrap));
                // pyro: `column-wrap: wrap` with nothing to size the columns (no `column-height`,
                // no definite height) still wraps where forced breaks fill a row: each column holds
                // a run up to a forced break, each `count` of them a row as tall as its tallest
                // column, `row-gap` apart (Chromium; WPT css-multicol
                // `column-wrap-no-constraints-002`).
                if multicol.wrap && fixed_height.is_none() && !breaks.forced.is_empty() {
                    let unbounded = (run_end - run_start).max(1.0) + 1.0;
                    let columns = run.fill(unbounded, left).columns;
                    let row_gap = multicol.row_gap(definite_height);
                    let per_row = count.max(1);
                    let rows = columns.len().div_ceil(per_row);
                    for (r, row) in columns.chunks(per_row).enumerate() {
                        let height = row.iter().map(|&(a, b)| b - a).fold(0.0f32, f32::max);
                        let top = snap(row_top);
                        for (index, &(start, end)) in row.iter().enumerate() {
                            let x = column_x(index);
                            let start = snap(start);
                            sink.push(ColumnFragment {
                                start,
                                end: snap(end),
                                strip_end: end,
                                x,
                                width: column_width,
                                top,
                                height,
                                offset: Point { x: x - pb.left, y: snap(top - start) },
                                spanner: false,
                                extended: 0.0,
                                exact_x: f32::NAN,
                                block_rounding_error: 0.0,
                            });
                        }
                        row_top += height + if r + 1 < rows { row_gap } else { 0.0 };
                    }
                } else {
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
                let mut columns = run.fill(column_height, left).columns;
                let rows = columns.len().div_ceil(per_row).max(1);
                // pyro: wrapping at the container's height (`column-height: auto`), `column-fill:
                // balance` balances the last row (of at most `count` columns), that height its
                // limit; an explicit `column-height` keeps every row at it.
                let last_row = (rows - 1).saturating_mul(per_row.min(columns.len().max(1)));
                let mut last_height = column_height;
                if fixed_height.is_some()
                    && (multicol.height.is_none() || multicol.wrap)
                    && matches!(multicol.fill, ColumnFill::Balance)
                    && columns.len() - last_row.min(columns.len()) <= count
                    && let Some(&(tail_start, _)) = columns.get(last_row)
                {
                    let tail = Part { breaks: &breaks, start: tail_start, end: run_end };
                    let height = tail.balance(count, Some(column_height)).min(column_height);
                    let refilled = tail.fill(height, left).columns;
                    if refilled.len() <= count {
                        columns.truncate(last_row);
                        columns.extend(refilled);
                        last_height = height;
                    }
                }
                for (i, (start, end)) in columns.into_iter().enumerate() {
                    let (row, index) = if per_row == usize::MAX { (0, i) } else { (i / per_row, i % per_row) };
                    let x = column_x(index);
                    let top = row_top + row as f32 * (column_height + row_gap);
                    sink.push(ColumnFragment {
                        start,
                        end,
                        strip_end: end,
                        x,
                        width: column_width,
                        top,
                        height: if row + 1 == rows { last_height } else { column_height },
                        offset: Point { x: x - pb.left, y: top - start },
                        spanner: false,
                        extended: 0.0,
                        exact_x: f32::NAN,
                        block_rounding_error: 0.0,
                    });
                }
                let last_slot = if multicol.height.is_some() { column_height } else { last_height };
                row_top += (rows - 1) as f32 * (column_height + row_gap) + last_slot;
                }
            }
            if let Some(&(start, end, indent)) = spanners.get(part) {
                // pyro: in rows of an explicit `column-height` a spanner has its place in the
                // rows' grid (`RowSlots::spanner_top`).
                if let Some(rows) = slotted {
                    row_top = rows.spanner_top(row_top, end - start);
                }
                row_top -= lift(part);
                sink.push(ColumnFragment {
                    start,
                    end,
                    strip_end: end,
                    x: pb.left,
                    width: content_width,
                    top: row_top,
                    height: end - start,
                    // pyro: a spanner below a child of the container is as far in as that box
                    // (its padding, border and margin): its row starts at the container's edge.
                    offset: Point { x: pb.left - indent, y: row_top - start },
                    spanner: true,
                    extended: 0.0,
                    exact_x: f32::NAN,
                    block_rounding_error: 0.0,
                });
                row_top += end - start;
                // (Where the spanner ends in the strip, unsnapped: what follows it starts there.)
                run_start = spanners[part].1 - spanner_leads.get(part).copied().unwrap_or(0.0);
            }
        }
        let (mut fragments, budget_left, capped) = sink.finish();
        if measuring {
            let made = start_budget - budget_left;
            self.column_measure_budget = self.column_measure_budget.saturating_sub(made);
            #[cfg(test)]
            MEASURED_FRAGMENTS.with(|m| m.set(m.get() + made));
        }
        if capped {
            // (The rest of the strip is in the last fragment; with none, the strip is shown as it is.)
            row_top = fragments.last().map_or(content_top + (breaks.end - breaks.start).max(0.0), |f| f.top + f.height);
        }
        // pyro: in a box of a definite height the last row of columns (after the last spanner)
        // reaches the end of its content box, however short balancing made the content: the
        // column boxes and their rules (Chromium; WPT css-multicol
        // `multicol-rule-nested-balancing-001`, `multicol-span-all-rule-001`).
        if let Some(height) = definite_height
            && nested_ctx.is_none()
            && multicol.height.is_none()
            && !multicol.wrap
            && fragments.last().is_some_and(|f| !f.spanner)
        {
            let end = pb.top + height;
            let last_top = fragments.last().map_or(0.0, |f| f.top);
            for fragment in fragments.iter_mut().rev().take_while(|f| !f.spanner && f.top == last_top) {
                let height = fragment.height.max(end - fragment.top);
                fragment.extended = height - fragment.height;
                fragment.height = height;
            }
        }
        // pyro: a column that starts inside a box of `box-decoration-break: clone` shows the box's
        // block-start border and padding again first (`Breaks::clones`).
        if !breaks.clones.is_empty() {
            for fragment in fragments.iter_mut().filter(|f| !f.spanner) {
                let repeated: f32 = breaks
                    .clones
                    .iter()
                    .filter(|&&(top, bottom, _)| fragment.start > top + EPSILON && fragment.start < bottom - EPSILON)
                    .map(|&(_, _, inset)| inset)
                    .sum();
                fragment.offset.y += repeated;
            }
        }
        // pyro: the columns' unsnapped left edges (`ColumnFragment::exact_x`).
        let pitch = column_width + gap;
        for fragment in &mut fragments {
            fragment.exact_x = if fragment.spanner || pitch <= EPSILON {
                fragment.x
            } else if multicol.rtl {
                let start = pb.left + content_width - column_width;
                start - ((start - fragment.x) / pitch).round() * pitch
            } else {
                pb.left + ((fragment.x - pb.left) / pitch).round() * pitch
            };
        }
        // The first shows what lies above the strip, the last what lies below.
        if let Some(first) = fragments.first_mut() {
            first.start = f32::NEG_INFINITY;
        }
        if let Some(last) = fragments.last_mut() {
            last.end = f32::INFINITY;
            last.strip_end = f32::INFINITY;
        }
        let columns_height = row_top - content_top;
        // pyro: what a fieldset's rendered legend adds above its content box (see `content_top`).
        let legend_extra = (content_top - pb.top).max(0.0);

        let height = inputs
            .known_dimensions
            .height
            .unwrap_or_else(|| {
                size.height
                    .unwrap_or(columns_height + pb_sum.height + legend_extra)
                    .maybe_clamp(min_size.height, max_size.height)
            })
            .max(pb_sum.height + legend_extra)
            + clone_rows as f32 * clone_insets;

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
            // (The content moves; the column boxes, and their rules, stay the cell's.)
            for fragment in &mut fragments {
                fragment.offset.y += shift;
            }
        }
        if !vertical {
            for fragment in &mut fragments {
                let painted = snap(fragment.offset.y);
                fragment.block_rounding_error = fragment.offset.y - painted;
                fragment.offset.y = painted;
            }
        }
        let right = fragments.iter().map(|f| f.x + f.width).fold(width - border.right, f32::max);
        let left = fragments.iter().map(|f| f.x).fold(border.left, f32::min);
        // pyro: and what overflows its spanners below, shown under their rows (their rows paint
        // them unclipped; WPT css-multicol `spanner-fragmentation-004`).
        let spanner_overflow = if spanners.is_empty() {
            f32::NEG_INFINITY
        } else {
            let rows: Vec<&ColumnFragment> = fragments.iter().filter(|f| f.spanner).collect();
            self.doc
                .column_spanners_of(dom_id)
                .into_iter()
                .zip(rows)
                .map(|((id, offset), row)| {
                    let layout = self.nodes[id].unrounded_layout();
                    let y = offset.y + layout.location.y;
                    y + layout.size.height.max(layout.scrollable_overflow_rect.bottom().abs()) + row.offset.y
                })
                .fold(f32::NEG_INFINITY, f32::max)
        };
        let bottom = (content_top + columns_height).max(height - border.bottom).max(spanner_overflow);
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
        // pyro: the baselines are where their lines are shown, in their columns (Chromium takes
        // the first baseline from the first column with a line; WPT css-multicol `baseline-001`).
        let mut baselines = strip.baselines;
        let mut first_after_spanner = std::collections::BTreeMap::new();
        let mut run_wrappers = std::collections::BTreeSet::new();
        let mut wrapper_initial_consumption = std::collections::HashMap::new();
        if !spanners.is_empty() && !vertical {
            let mut owners = std::collections::HashMap::new();
            let mut paths = std::collections::HashSet::new();
            for ((id, _), (index, _)) in self.doc.column_spanners_of(dom_id).into_iter().zip(fragments.iter().enumerate().filter(|(_, f)| f.spanner)) {
                owners.insert(id, index + 1);
                let mut at = self.nodes[id].layout_parent.get();
                while let Some(parent) = at.filter(|&p| p != dom_id && paths.insert(p)) { at = self.nodes[parent].layout_parent.get(); }
            }
            let mut next = None;
            let mut pending = vec![(dom_id, 0.0)];
            while let Some((id, parent_y)) = pending.pop() {
                let child = &self.nodes[id];
                let y = if id == dom_id { 0.0 } else { parent_y + child.unrounded_layout().location.y };
                if let Some(&index) = owners.get(&id) {
                    next = Some(index);
                    let mut parent = child.layout_parent.get();
                    let mut owner_top = parent_y;
                    while let Some(owner) = parent.filter(|&p| p != dom_id && !wrapper_initial_consumption.contains_key(&p)) {
                        wrapper_initial_consumption.insert(owner, (fragments[index - 1].start - owner_top).max(0.0));
                        owner_top -= self.nodes[owner].unrounded_layout().location.y;
                        parent = self.nodes[owner].layout_parent.get();
                    }
                } else if id == dom_id || paths.contains(&id) {
                    if let Some(children) = child.layout_children.borrow().as_ref() {
                        pending.extend(children.iter().rev().map(|&child| (child, y)));
                    }
                } else if let Some(index) = next && let Some(first) = fragments.get(index) && !first.spanner {
                    if !child.is_out_of_flow() && y < first.start - EPSILON {
                        first_after_spanner.insert(id, index);
                        let mut at = child.layout_parent.get();
                        while let Some(parent) = at.filter(|&p| p != dom_id && run_wrappers.insert(p)) { at = self.nodes[parent].layout_parent.get(); }
                    }
                }
            }
        }
        for token in &breaks.overlapping_starts {
            let consumed = token.origin + token.consumed;
            let index = fragments.partition_point(|f| f.end < consumed - EPSILON);
            if let Some(first) = fragments.get(index) && !first.spanner && consumed > first.start + EPSILON
                && token.origin + self.nodes[token.resume].unrounded_layout().location.y < first.start - EPSILON {
                first_after_spanner.entry(token.resume).or_insert(index);
                let mut at = Some(token.owner);
                while let Some(parent) = at.filter(|&p| p != dom_id && run_wrappers.insert(p)) { at = self.nodes[parent].layout_parent.get(); }
            }
        }
        let mut continuation_ends = breaks.overflow_continuations;
        for token in &breaks.overflow_breaks {
            let layout = self.nodes[token.resume].unrounded_layout();
            let end = token.origin + layout.location.y + layout.size.height;
            let entry = continuation_ends.entry(token.owner).or_insert(end);
            *entry = entry.max(end);
        }
        let mut consumed_by_owner = std::collections::HashMap::<_, f32>::new();
        if fragmentainer.is_some() {
            for token in &breaks.overflow_parents {
                let start = token.origin + token.consumed;
                let earliest = consumed_by_owner.entry(token.owner).or_insert(start);
                *earliest = earliest.min(start);
            }
        }
        let columns = ColumnFragments { fragments, first_after_spanner, run_wrappers, wrapper_initial_consumption, continuation_ends, consumed_by_owner, nested: nested_ctx.or(nested_rows_ctx).or(nested_span_ctx), pushed, strip_end: breaks.end };
        if columns.fragments.len() > 1 && !vertical {
            for candidate in oof_candidates.as_mut_slice() {
                if candidate.containing_block.is_some() {
                    continue;
                }
                let y = candidate.static_position.y.area.start;
                let (dx, dy) = columns.shift_at(if columns.fragment_at(y).is_some_and(|f| f.spanner && f.start == y) {
                    y - EPSILON
                } else {
                    y
                });
                let position = &mut candidate.static_position;
                position.x.area.start += dx;
                position.x.area.end += dx;
                position.y.area.start += dy;
                position.y.area.end += dy;
            }
        }

        if columns.fragments.len() > 1 && !vertical {
            let shown = |y: Option<f32>| y.map(|y| y + columns.shift_at(y).1);
            baselines.first = shown(baselines.first);
            baselines.last = shown(baselines.last);
            // pyro: the first baseline is the highest of those the first row's columns show, not
            // the strip's first (Chromium; WPT css-multicol `baseline-002`).
            let first_top = columns.fragments[0].top;
            let row_end = columns
                .fragments
                .iter()
                .take_while(|f| f.top == first_top)
                .map(|f| f.end)
                .fold(f32::NEG_INFINITY, f32::max);
            for &y in breaks.baselines.iter().filter(|&&y| y < row_end) {
                let y = y + columns.shift_at(y).1;
                baselines.first = Some(baselines.first.map_or(y, |first| first.min(y)));
            }
        }
        if inputs.run_mode == RunMode::PerformLayout {
            // (Debited as they were made: `FragmentSink::push`.)
            self.column_fragment_budget = budget_left;
            self.doc.has_multicol = true;
            // (None made, the pass's budget spent: its strip is shown as it is.)
            self.nodes[dom_id].layout_data_mut().columns = (!columns.fragments.is_empty()).then(|| Box::new(columns));
        }

        let size = Size { width, height };
        LayoutOutput {
            size,
            scrollable_overflow_rect: overflow,
            baselines,
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
        let mut spanners: Vec<crate::NodeId> =
            if inline_root { Vec::new() } else { self.doc.column_spanners_of(dom_id).into_iter().map(|(id, _)| id).collect() };
        // pyro: a fieldset's rendered legend is measured on its own too: it is in no column (WPT
        // css-overflow `scroll-markers/column-scroll-marker-004`).
        if let Some(legend) = self.doc.fieldset_rendered_legend(dom_id).filter(|legend| !spanners.contains(legend)) {
            spanners.push(legend);
        }
        let outer_spanners = std::mem::replace(&mut self.column_spanners, (!spanners.is_empty()).then_some((dom_id, -1.0)));
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
    /// pyro: pushes each column spanner of `container` below what the boxes before it hold,
    /// overflow included (`spanner_floors`), clearing the layouts that must be redone. Whether a
    /// floor changed.
    fn update_spanner_floors(&mut self, container: crate::NodeId) -> bool {
        let mut changed = false;
        let mut cleared = std::collections::HashSet::new();
        for (id, parent_y, before, _) in self.doc.spanner_neighbors(container) {
            let layout = *self.nodes[id].unrounded_layout();
            // (A negative top margin pulls it back up over that content: Chromium; WPT
            // css-multicol `multicol-span-all-children-height-010`.)
            let current_top = parent_y + layout.location.y - layout.margin.top;
            let existing = self.spanner_floors.get(&id).copied();
            // The least top of its border box in its parent's coordinates.
            let wanted = (before > current_top + 0.5 || existing.is_some())
                .then(|| before + layout.margin.top - parent_y)
                .filter(|&floor| floor > layout.location.y - 0.5 || existing.is_some());
            let same = match (existing, wanted) {
                (None, None) => true,
                (Some(a), Some(b)) => (a - b).abs() <= 0.5,
                _ => false,
            };
            if same {
                continue;
            }
            match wanted {
                Some(floor) => self.spanner_floors.insert(id, floor),
                None => self.spanner_floors.remove(&id),
            };
            changed = true;
            use taffy::CacheTree as _;
            let mut at = Some(id);
            while let Some(node) = at.filter(|&n| n != container && cleared.insert(n)) {
                self.cache_clear(crate::taffy_node_id(node));
                at = self.nodes[node].layout_parent.get();
            }
        }
        changed
    }

    /// pyro: the margin-box heights of the column spanners inside box `node_id` of the strip being
    /// laid out (`column_spanners`), when it has a `known_height` (its own: spanners take none of
    /// it) and carries spanners. `None` when there are none.
    pub(crate) fn spanner_extent_in(&self, node_id: crate::NodeId, known_height: Option<f32>) -> Option<(f32, f32)> {
        let (container, width) = self.column_spanners?;
        let height = known_height?;
        if width < 0.0 || node_id == container || !self.doc.carries_spanners(node_id) {
            return None;
        }
        // The spanners inside it: their margin boxes in its coordinates.
        let mut spans: Vec<(f32, f32)> = Vec::new();
        for (id, _) in self.doc.column_spanners_of(container) {
            let layout = self.nodes[id].unrounded_layout();
            let mut y = layout.location.y;
            let mut at = self.nodes[id].layout_parent.get();
            while let Some(parent) = at.filter(|&p| p != container) {
                if parent == node_id {
                    spans.push((y - layout.margin.top.max(0.0), y + layout.size.height + layout.margin.bottom.max(0.0)));
                    break;
                }
                y += self.nodes[parent].unrounded_layout().location.y;
                at = self.nodes[parent].layout_parent.get();
            }
        }
        let &(last_start, last_end) = spans.last()?;
        let total: f32 = spans.iter().map(|&(a, b)| b - a).sum();
        // What of its height the content before the last spanner takes (spanners take none),
        // and what is left after it: the box goes on that far below the last spanner, at least
        // its height and the spanners' (content before a spanner that overflows the box puts the
        // spanner lower: Chromium; WPT css-multicol `multicol-span-all-children-height-012`).
        let before = (last_start - (total - (last_end - last_start))).max(0.0);
        let needed = (height + total).max(last_end + (height - before.min(height)));
        let mut border_end = height;
        for (start, end) in spans {
            if start >= border_end { break; }
            border_end += end - start;
        }
        (needed - height > EPSILON).then_some((needed, border_end))
    }


    /// pyro: whether `node_id` is a column spanner of the multi-column container whose strip is
    /// being laid out (`column_spanners`).
    /// pyro: whether `id` is the rendered legend of the fieldset `container` (measured on its
    /// own while its columns are: `multicol_content_widths`).
    pub(crate) fn is_measured_rendered_legend(&self, container: crate::NodeId, id: crate::NodeId) -> bool {
        self.nodes[id].layout_parent.get() == Some(container) && self.doc.is_rendered_legend(container, id)
    }

    pub(crate) fn is_current_column_spanner(&self, node_id: crate::NodeId) -> bool {
        let Some((container, _)) = self.column_spanners else { return false };
        let node = &self.nodes[node_id];
        !node.is_out_of_flow()
            && node.primary_styles().is_some_and(|style| is_spanner_style(&style))
            && self.spans_columns_of(container, node_id)
    }

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
        // pyro: a negative width: the columns are being measured without the spanners
        // (`multicol_content_widths`), which are as wide as nothing then.
        if width < 0.0 {
            inputs.parent_size.width = Some(0.0);
            inputs.available_space.width = AvailableSpace::Definite(0.0);
            inputs.known_dimensions.width = Some(0.0);
            return inputs;
        }
        let ls = node.layout_style();
        let margin = ls.margin().resolve_or_zero(Some(width), resolve_calc_value);
        inputs.parent_size.width = Some(width);
        inputs.available_space.width = AvailableSpace::Definite(width);
        // (A table is as wide as its content needs in that width, not stretched: Chromium; WPT
        // css-multicol `multicol-span-all-005`.)
        let replaced = node.element_data().is_some_and(super::replaced::is_replaced);
        // (Nor a replaced element: its `auto` width is its own, Chromium; WPT css-multicol
        // `replaced-content-spanner-auto-width`.)
        // (Nor one of an orthogonal writing mode: its width is its block size, from its content;
        // WPT css-multicol `orthogonal-writing-mode-spanner`.)
        let orthogonal = self.nodes[container]
            .primary_styles()
            .is_some_and(|outer| outer.writing_mode.is_vertical() != style.writing_mode.is_vertical());
        if ls.size().width.is_auto() && !node.flags.is_table_root() && !replaced && !orthogonal {
            inputs.known_dimensions.width = Some((width - margin.left - margin.right).max(0.0));
        }
        inputs
    }
}

/// Whether `node`'s style makes it a multi-column container (see [`is_multicol_style`]).
#[inline]
pub(crate) fn is_multicol_node(node: &Node) -> bool {
    node.primary_styles().is_some_and(|style| is_multicol_style(&style))
        && !uses_anonymous_content_box(node)
}

pub(crate) fn uses_anonymous_content_box(node: &Node) -> bool {
    node.data.downcast_element().is_some_and(|element| element.name == crate::qual_name!("button", html))
}

impl Node {
    /// pyro: the multi-column container (horizontal, no transform between) below this box that
    /// `child_id`, a box this one paints (a stacking context hoisted here, past the container's
    /// columns), is in: the container, its border box's offset from this box's (CSS px), and the
    /// child's border box range in its strip (its overflow below included). Paint shows such a
    /// child in each column it is in (blitz-paint `draw_hoisted_children`), at its strip position
    /// (`hoisted_child_strip_position`), and hit testing finds it there (`Node::hit_inner`).
    pub fn multicol_between(&self, child_id: crate::NodeId) -> Option<(crate::NodeId, Point<f32>, f32, f32)> {
        let tree = self.tree();
        let child = tree.get(child_id)?;
        let layout = child.final_layout();
        let mut top = layout.location.y;
        let bottom_extent = layout.size.height.max(layout.scrollable_overflow_rect.bottom().abs());
        let transformed = |n: &Node| n.primary_styles().is_some_and(|s| !s.get_box().transform.0.is_empty());
        let mut at = child.layout_parent.get();
        let mut container = None;
        let mut steps = 0;
        while let Some(node) = at {
            steps += 1;
            if steps > 64 || node == self.id {
                break;
            }
            let n = tree.get(node)?;
            if transformed(n) {
                return None;
            }
            if n.column_fragments().is_some() {
                container = Some(node);
                break;
            }
            top += n.final_layout().location.y;
            at = n.layout_parent.get();
        }
        let container = container?;
        // (An out-of-flow box placed against a box outside the container, or the container, is
        // placed in its column already or not fragmented: only one placed inside it is.)
        if child.is_out_of_flow() {
            let cb = child.containing_block()?;
            if cb == container {
                return None;
            }
            let mut up = Some(cb);
            let mut inside = false;
            while let Some(node) = up {
                if node == container {
                    inside = true;
                    break;
                }
                up = tree.get(node)?.layout_parent.get();
            }
            if !inside {
                return None;
            }
        }
        // Its offset from this box, through boxes with no transform.
        let mut origin = Point { x: 0.0f32, y: 0.0 };
        let mut at = Some(container);
        while let Some(node) = at.filter(|&n| n != self.id) {
            let n = tree.get(node)?;
            if transformed(n) {
                return None;
            }
            let location = n.final_layout().location;
            origin.x += location.x;
            origin.y += location.y;
            at = n.layout_parent.get();
        }
        at?;
        Some((container, origin, top, top + bottom_extent))
    }

    /// pyro: the points (in the space `hoisted_child`, a stacking context hoisted to this box, is
    /// hit in) to hit it at for the point `x`, `y` (CSS px, this box's), in order. One shown in each
    /// column of a multi-column container below that it is in (`multicol_between`) is hit through
    /// each column paint shows it in (`ColumnFragments::hoisted_columns`), the last painted first,
    /// at its place in the strip, where the point is in that column's range; nowhere else.
    pub(crate) fn hoisted_hit_points(&self, hoisted_child: &crate::HoistedPaintChild, x: f32, y: f32) -> Vec<(f32, f32)> {
        let tree = self.tree();
        let in_columns = self.multicol_between(hoisted_child.node_id).and_then(|(container, origin, top, bottom)| {
            let columns = tree.get(container)?.column_fragments()?;
            (columns.fragments.len() > 1).then_some((container, origin, top, bottom, columns))
        });
        match in_columns {
            Some((container, origin, top, bottom, columns)) => {
                let pos = super::paint_tree::hoisted_child_strip_position(tree, self.id, hoisted_child.node_id, container);
                columns
                    .hoisted_columns(top, bottom)
                    .rev()
                    .filter_map(|fragment| {
                        // The point in the container's strip, as this column shows it.
                        let strip_x = x - origin.x - fragment.offset.x;
                        let strip_y = y - origin.y - fragment.offset.y;
                        (strip_y >= fragment.start && strip_y < fragment.end)
                            .then(|| (strip_x + origin.x - pos.x, strip_y + origin.y - pos.y))
                    })
                    .collect()
            }
            None => {
                let pos = hoisted_child.position(tree, self.id);
                vec![(x - pos.x, y - pos.y)]
            }
        }
    }

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
    /// pyro: whether a forced break before the first in-flow descendants of box `id` (no border
    /// or padding between) goes before it.
    fn first_descendant_forces_break(&self, id: crate::NodeId) -> bool {
        let mut at = id;
        for _ in 0..8 {
            let layout = self.nodes[at].unrounded_layout();
            if (layout.border.top > 0.0 || layout.padding.top > 0.0) && at != id {
                return false;
            }
            let children = self.nodes[at].layout_children.borrow();
            let Some(&first) = children.iter().flatten().find(|&&c| {
                let node = &self.nodes[c];
                !node.is_out_of_flow() && node.primary_styles().is_some_and(|s| !s.get_box().display.is_none())
            }) else {
                return false;
            };
            drop(children);
            let Some(style) = self.nodes[first].primary_styles() else { return false };
            use style::values::computed::BreakBetween as B;
            if matches!(style.get_box().break_before, B::Column | B::Always) {
                return true;
            }
            at = first;
        }
        false
    }

    /// pyro: whether a forced break after the last in-flow descendants of box `id` (no border or
    /// padding between) goes after it.
    fn last_descendant_forces_break_after(&self, id: crate::NodeId) -> bool {
        let mut at = id;
        for _ in 0..8 {
            let layout = self.nodes[at].unrounded_layout();
            if (layout.border.bottom > 0.0 || layout.padding.bottom > 0.0) && at != id {
                return false;
            }
            let children = self.nodes[at].layout_children.borrow();
            let Some(&last) = children.iter().flatten().rev().find(|&&c| {
                let node = &self.nodes[c];
                !node.is_out_of_flow() && node.primary_styles().is_some_and(|s| !s.get_box().display.is_none())
            }) else {
                return false;
            };
            drop(children);
            let Some(style) = self.nodes[last].primary_styles() else { return false };
            use style::values::computed::BreakBetween as B;
            if matches!(style.get_box().break_after, B::Column | B::Always) {
                return true;
            }
            at = last;
        }
        false
    }

    fn spanner_neighbors(&self, container: crate::NodeId) -> Vec<(crate::NodeId, f32, f32, f32)> {
        fn visit(doc: &BaseDocument, parent: crate::NodeId, top: f32, mut bottom: f32, depth: usize, out: &mut Vec<(crate::NodeId, f32, f32, f32)>) {
            let mut previous: Option<usize> = None;
            for &id in doc.nodes[parent].layout_children.borrow().iter().flatten() {
                #[cfg(test)]
                SPANNER_VISITS.with(|work| work.set(work.get() + 1));
                let node = &doc.nodes[id];
                if node.is_out_of_flow() { continue; }
                let Some(style) = node.primary_styles() else { continue };
                if style.get_box().display.is_none() { continue; }
                let spanner = is_spanner_style(&style);
                let layout = node.unrounded_layout();
                if let Some(index) = previous.take() {
                    if !spanner {
                        let margin = doc.nodes[out[index].0].unrounded_layout().margin.bottom;
                        out[index].3 = margin.max(0.0).min(layout.margin.top.max(0.0));
                    }
                }
                let child_top = top + layout.location.y;
                if spanner {
                    previous = Some(out.len());
                    out.push((id, top, bottom, 0.0));
                } else if depth < 64 && doc.column_span_styles && doc.carries_spanners(id) {
                    visit(doc, id, child_top, bottom, depth + 1, out);
                }
                let overflow = if spanner { 0.0 } else { layout.scrollable_overflow_rect.bottom().abs() };
                bottom = bottom.max(child_top + (layout.size.height + layout.margin.bottom.min(0.0)).max(overflow));
            }
        }
        let mut out = Vec::new();
        visit(self, container, 0.0, f32::NEG_INFINITY, 0, &mut out);
        out
    }

    /// pyro: whether box `id` carries spanners of multi-column container `container` (it is below
    /// it through boxes that carry spanners, and carries them itself): a spanner's row is no part
    /// of its box (its background and borders are not painted there).
    pub fn carries_spanners_of(&self, container: crate::NodeId, id: crate::NodeId) -> bool {
        if id == container || !self.carries_spanners(id) || !self.spans_columns_of(container, id) {
            return false;
        }
        // (Not the spanner itself, nor a box inside one: those are the row's content.)
        let mut at = Some(id);
        while let Some(node) = at.filter(|&n| n != container) {
            if self.nodes[node].primary_styles().is_some_and(|style| is_spanner_style(&style)) {
                return false;
            }
            at = self.nodes[node].layout_parent.get();
        }
        true
    }

    /// pyro: the column spanners of `container`, in the order of their rows.
    pub fn column_spanners_in_order(&self, container: crate::NodeId) -> Vec<crate::NodeId> {
        self.column_spanners_of(container).into_iter().map(|(id, _)| id).collect()
    }

    /// pyro: whether `id` is a column spanner of multi-column container `container` (painted in
    /// its own row, not in a column).
    pub fn is_column_spanner_of(&self, container: crate::NodeId, id: crate::NodeId) -> bool {
        let node = &self.nodes[id];
        !node.is_out_of_flow()
            && node.primary_styles().is_some_and(|style| is_spanner_style(&style))
            && self.spans_columns_of(container, id)
    }

    /// The columns of multi-column container `node_id` (see [`Node::column_fragments`]).
    #[inline]
    pub fn column_fragments(&self, node_id: crate::NodeId) -> Option<&ColumnFragments> {
        self.nodes.get(node_id)?.column_fragments()
    }

    /// `rect` (device px, `node_id`'s coordinates), shown in `node_id`'s columns when it is a
    /// multi-column container: its scrollable overflow and hit bound.
    /// pyro: `rect` (device px) of a vertical writing mode's multi-column container's strip,
    /// shown in its columns as paint shows them (`draw_columns`): its scrollable overflow (Chrome:
    /// WPT css-overflow `no-scrollable-overflow-vertical-rl`). `None`: no such container.
    pub(crate) fn vertical_columns_rect(&self, node_id: crate::NodeId, rect: kurbo::Rect, scale: f64) -> Option<kurbo::Rect> {
        let node = self.nodes.get(node_id)?;
        let (columns, rl) = node.vertical_column_fragments()?;
        let width = node.final_layout().size.width as f64;
        let mut out: Option<kurbo::Rect> = None;
        // (A rect only touching a fragment's range is not in it: `ColumnFragments::map_rect`.)
        let empty = rect.x1 <= rect.x0;
        // (Only the fragments the rect can reach, in strip order: the strip's block axis runs from
        // the right where the block axis does.)
        let (from, to) = if rl { (width - rect.x1 / scale, width - rect.x0 / scale) } else { (rect.x0 / scale, rect.x1 / scale) };
        let (first, last) = columns.reaching(from, to);
        for f in &columns.fragments[first..last] {
            #[cfg(test)]
            MAPPED_FRAGMENTS.with(|m| m.set(m.get() + 1));
            let (start, end) = (f.start as f64, f.end as f64);
            let (range, offset) = if rl {
                ((width - end, width - start), kurbo::Vec2::new(-f.offset.y as f64, f.offset.x as f64))
            } else {
                ((start, end), kurbo::Vec2::new(f.offset.y as f64, f.offset.x as f64))
            };
            let x0 = rect.x0.max(range.0 * scale);
            let x1 = rect.x1.min(range.1 * scale);
            if x1 < x0 || (!empty && x1 <= x0) || (empty && out.is_some()) {
                continue;
            }
            let piece = kurbo::Rect::new(x0, rect.y0, x1, rect.y1) + offset * scale;
            out = Some(out.map_or(piece, |o| o.union(piece)));
        }
        Some(out.unwrap_or(rect))
    }

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
            // pyro: an inline split by block-level boxes (block-in-inline) carries them too
            // (WPT css-multicol `spanner-in-child-after-parallel-flow-003`).
            let split_inline = node.is_split_inline() || node.is_inline_with_only_block_children();
            !contained
                && style.clone_float() == Float::None
                && (split_inline
                    || (style.clone_display().inside() == DisplayInside::Flow
                        && style.clone_display().outside() == DisplayOutside::Block
                        && fragmentable(node, &style)))
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
    pub(crate) fn column_spanners_of(&self, container: crate::NodeId) -> Vec<(crate::NodeId, Point<f32>)> {
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
            // pyro: a fieldset's rendered legend is no spanner: block layout places it over the
            // fieldset's block-start border, outside the columns of its content box (taffy
            // `block_child_is_rendered_legend`), and it paints unfragmented (blitz-paint `multicol_skips`).
            if node.primary_styles().is_some_and(|s| is_spanner_style(&s)) {
                found.push((id, offset));
            } else if self.column_span_styles && depth < 64 && self.carries_spanners(id) {
                let location = node.unrounded_layout().location;
                let below = Point { x: offset.x + location.x, y: offset.y + location.y };
                self.collect_column_spanners(id, below, depth + 1, found);
            }
        }
    }

    /// pyro: whether `id` is the rendered legend of fieldset `container` (its first `<legend>`
    /// child, in flow and not floated): it is not part of the fieldset's columns, which are its
    /// anonymous content box's, but a row of its own above them, as a spanner is (Chromium; WPT
    /// css-multicol `multicol-span-all-fieldset-001`).
    fn is_rendered_legend(&self, container: crate::NodeId, id: crate::NodeId) -> bool {
        let is = |id: crate::NodeId, name: &str| self.nodes[id].element_data().is_some_and(|e| &*e.name.local == name);
        if !is(container, "fieldset") || !is(id, "legend") {
            return false;
        }
        let node = &self.nodes[id];
        if node.is_out_of_flow() || node.primary_styles().is_none_or(|s| s.clone_float() != Float::None) {
            return false;
        }
        let children = self.nodes[container].layout_children.borrow();
        children.iter().flatten().copied().find(|&c| is(c, "legend")) == Some(id)
    }

    /// Whether `container` has column spanners (see [`Self::column_spanners_of`]).
    fn has_column_spanners(&self, container: crate::NodeId) -> bool {
        !self.column_spanners_of(container).is_empty()
    }

    /// The strip ranges of `container`'s column spanners, their margins included, in order, and
    /// where each spanner's margin box starts (the container's coordinates).
    fn column_spanner_ranges(&self, container: crate::NodeId) -> Vec<(f32, f32, f32)> {
        let mut ranges: Vec<(f32, f32, f32)> = Vec::new();
        // The previous spanner's border-box bottom and parent.
        let mut previous_border_end = f32::NEG_INFINITY;
        let mut previous_parent = None;
        for (id, offset) in self.column_spanners_of(container) {
            let layout = self.nodes[id].unrounded_layout();
            let y = offset.y + layout.location.y;
            let start = y - layout.margin.top.max(0.0);
            let end = y + layout.size.height + layout.margin.bottom.max(0.0);
            // pyro: a spanner's row keeps its whole top margin, though the strip collapsed it with
            // the bottom margin of a spanner right before it in another box (the margins of
            // spanners in different boxes do not collapse: Chromium; WPT css-multicol
            // `multicol-span-all-margin-nested-001`; siblings' do, `multicol-span-all-margin-001`);
            // a row starts after the one before when content lies between them.
            let parent = self.nodes[id].layout_parent.get();
            let start = match ranges.last() {
                Some(&(_, previous, _)) if start < previous => {
                    if parent != previous_parent && start >= previous_border_end - EPSILON { start } else { previous }
                }
                _ => start,
            };
            previous_border_end = y + layout.size.height;
            previous_parent = parent;
            // (Where its margin box starts in the strip, inside the boxes that carry it.)
            ranges.push((start, end.max(start), offset.x + layout.location.x - layout.margin.left));
        }
        ranges
    }


    /// pyro: for each of `container`'s spanners, how much of its bottom margin is the top margin
    /// of the in-flow box after it (collapsed with it in the strip): the columns after the
    /// spanner show it, as margins do not collapse across a spanner's row (Chromium; WPT
    /// css-multicol `multicol-span-all-margin-bottom-001`).
    fn column_spanner_leads(&self, container: crate::NodeId) -> Vec<f32> {
        self.spanner_neighbors(container).into_iter().map(|(_, _, _, lead)| lead).collect()
    }

    /// Where the strip of the multi-column container `container` may break, from its laid-out
    /// content: `start..end` its content (`start` its content box's top; `end` the strip's
    /// content end, or `None` to take it from the content: its last in-flow child's bottom
    /// margin edge, its floats' margin edges).
    /// pyro: `nested_breakable`: the multi-column containers nested in the strip's flow are
    /// fragmented by its columns (`Breaks::nested`), not monolithic.
    fn multicol_breaks(&self, container: crate::NodeId, start: f32, end: Option<f32>, nested_breakable: bool, fixed_columns: bool) -> Breaks {
        let mut found = Found { nested_breakable, fixed_columns, container: Some(container), parallel_end: f32::NEG_INFINITY, ..Found::default() };
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
            one_column_when_empty: false,
            whole: merge(found.whole),
            gaps: found.gaps,
            trims: found.trims,
            forced: found.forced,
            overflow_breaks: found.overflow_breaks,
            overlapping_starts: found.overlapping_starts,
            overflow_parents: found.overflow_parents,
            overflow_continuations: found.overflow_continuations,
            parallel_floats: found.parallel_floats,
            nested: found.nested,
            baselines: found.baselines,
            clones: found.clones,
        }
    }

    /// pyro: the floats among inline root `parent`'s layout children (its border box at strip
    /// position `top`), into `found` as `collect_breaks` takes a block's floats.
    fn collect_inline_floats(&self, parent: crate::NodeId, top: f32, found: &mut Found) {
        let children = self.nodes[parent].layout_children.borrow();
        let Some(children) = children.as_deref() else { return };
        for &child_id in children {
            let child = &self.nodes[child_id];
            let Some(style) = child.primary_styles() else { continue };
            if style.clone_float() == Float::None || style.get_box().display.is_none() || child.is_out_of_flow() {
                continue;
            }
            let layout = child.unrounded_layout();
            let child_top = top + layout.location.y;
            let child_bottom = child_top + layout.size.height;
            found.bottom = found.bottom.max(child_bottom);
            found.floats.push((child_top, child_bottom));
            if found.parallel && avoids_column_break_inside(&style) {
                found.parallel_floats.push((child_id, top, child_top, child_bottom));
                continue;
            }
            if fragmentable(child, &style) {
                if avoids_column_break_inside(&style) {
                    found.strict.push((child_top, child_bottom));
                }
                crate::util::ensure_stack(|| self.collect_breaks(child_id, child_top, found, None, (false, false)));
            } else {
                let margin_box = (child_top - layout.margin.top.max(0.0), child_bottom + layout.margin.bottom.max(0.0));
                found.strict.push(margin_box);
                found.loose.push(margin_box);
                if held_whole(child, &style) {
                    found.whole.push((child_top, child_bottom));
                }
            }
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

    /// pyro: the block ranges of grid `id`'s rows (its border box at strip position `top`), with its
    /// in-flow items' boxes that reach past them (an item spanning rows) merged in. `None` for any
    /// other box.
    fn grid_row_bands(&self, id: crate::NodeId, top: f32) -> Option<Vec<(f32, f32)>> {
        let node = &self.nodes[id];
        let taffy::DetailedLayoutInfo::Grid(info) = &node.element_data()?.detailed_layout_info else {
            return None;
        };
        let mut ranges: Vec<(f32, f32)> =
            info.rows.positions.iter().map(|row| (top + row.start, top + row.end)).filter(|r| r.1 > r.0).collect();
        let children = node.layout_children.borrow();
        for &child_id in children.as_deref().unwrap_or(&[]) {
            let child = &self.nodes[child_id];
            if child.is_out_of_flow() || child.primary_styles().is_none() {
                continue;
            }
            let layout = child.unrounded_layout();
            let child_top = top + layout.location.y;
            ranges.push((child_top, child_top + layout.size.height));
        }
        Some(merge(ranges))
    }

    /// pyro: the block ranges of table `id`'s rows (its border box at strip position `top`): its
    /// cells' ranges, those that overlap (row spans) merged.
    fn table_row_bands(&self, id: crate::NodeId, top: f32) -> Vec<(f32, f32)> {
        fn cells(doc: &BaseDocument, id: crate::NodeId, top: f32, depth: usize, out: &mut Vec<(f32, f32)>) {
            let children = doc.nodes[id].layout_children.borrow();
            let Some(children) = children.as_deref() else { return };
            for &child_id in children {
                let child = &doc.nodes[child_id];
                let Some(style) = child.primary_styles() else { continue };
                let layout = child.unrounded_layout();
                let child_top = top + layout.location.y;
                if style.clone_display().inside() == DisplayInside::TableCell {
                    out.push((child_top, child_top + layout.size.height));
                } else if depth < 3 && !child.is_out_of_flow() {
                    cells(doc, child_id, child_top, depth + 1, out);
                }
            }
        }
        let mut ranges = Vec::new();
        cells(self, id, top, 0, &mut ranges);
        merge(ranges)
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
            // pyro: the floats of the inline formatting context are kept from breaks as those
            // of a block's (WPT css-multicol `multicol-fill-balance-032`, `-034`..`-036`).
            self.collect_inline_floats(parent, top, found);
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
                    found.baselines.push(origin + m.baseline / scale);
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
            if found.parallel { return; }
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
        let mut next_parallel_break = false;
        let mut previous_unit_top: f32 = 0.0;
        let mut parallel_ends = Vec::new();
        if children.iter().any(|&id| {
            let layout = self.nodes[id].unrounded_layout();
            layout.scrollable_overflow_rect.bottom() > layout.size.height + EPSILON
        }) {
            let mut end = f32::NEG_INFINITY;
            for &id in children.iter().rev() {
                parallel_ends.push(end);
                let sibling = &self.nodes[id];
                if !sibling.is_out_of_flow() && let Some(style) = sibling.primary_styles() && !style.get_box().display.is_none() {
                    let layout = sibling.unrounded_layout();
                    if is_spanner_style(&style) || (self.column_span_styles && self.carries_spanners(id) && self.has_column_spanners(id)) {
                        end = f32::NEG_INFINITY;
                    } else {
                        end = end.max(top + layout.location.y + layout.size.height);
                    }
                }
            }
            parallel_ends.reverse();
        }
        for (child_index, &child_id) in children.iter().enumerate() {
            let child = &self.nodes[child_id];
            let Some(style) = child.primary_styles() else {
                continue;
            };
            if style.get_box().display.is_none() {
                continue;
            }
            if child.is_out_of_flow() {
                // pyro: an out-of-flow box whose containing block is its parent in the strip (not
                // the container, whose own are not fragmented) is fragmented with the columns:
                // they go on as far as it does (Chromium; WPT css-multicol
                // `fixed-in-multicol-with-transform-container`).
                // (A monolithic one is not: it stays whole in the column of its static position,
                // overflowing it; WPT css-contain `contain-size-monolithic-002`.)
                if child.containing_block() == Some(parent)
                    && found.container.is_some_and(|c| c != parent)
                    && fragmentable(child, &style)
                {
                    let layout = child.unrounded_layout();
                    let child_top = top + layout.location.y;
                    found.bottom = found.bottom.max(child_top + layout.size.height);
                    crate::util::ensure_stack(|| self.collect_breaks(child_id, child_top, found, None, trims));
                }
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
            let parent_bottom = top + node.unrounded_layout().size.height;
            let parallel_tail = found.container != Some(parent)
                && layout.scrollable_overflow_rect.bottom() > layout.size.height + EPSILON
                && parent_bottom > child_bottom + EPSILON;
            if parallel_tail {
                found.overflow_parents.push(FlowContinuation { owner: parent, resume: child_id, origin: top, consumed: child_bottom - top });
            }
            found.bottom = found.bottom.max(child_bottom);
            if container_content_top.is_some() {
                // The content's end: margin edges (the strip is a formatting context root).
                found.flow_end = found.flow_end.max(child_bottom + layout.margin.bottom.max(0.0));
            }
            if style.clone_float() == Float::None {
                if layout.margin.top < 0.0 && !is_spanner_style(&style) && let Some((bottom, _)) = previous && child_top < bottom - EPSILON {
                    found.overlapping_starts.push(FlowContinuation { owner: parent, resume: child_id, origin: top, consumed: bottom - top });
                }
                if std::mem::take(&mut next_parallel_break) {
                    found.overflow_breaks.push(FlowContinuation { owner: parent, resume: child_id, origin: top, consumed: previous.map_or(child_top, |(end, _)| end) - top });
                }
                // pyro: forced breaks between in-flow siblings (margins after them kept).
                let forces = |b: style::values::computed::BreakBetween| {
                    use style::values::computed::BreakBetween as B;
                    // (In columns: `page`, `left` and `right` force page breaks only, which print
                    // pagination takes; CSS Fragmentation 3 § 3.1.)
                    matches!(b, B::Column | B::Always)
                };
                // pyro: and its first descendants', which a box that the columns do not look into
                // (a nested multi-column container) takes as its own (CSS Fragmentation 3 § 3.1;
                // WPT css-multicol `spanner-fragmentation-009`).
                // (Not through layout containment: its forced breaks do not propagate, CSS Contain 2
                // § 3.3.)
                let contains_layout = style
                    .get_box()
                    .slow_clone_contain()
                    .contains(style::values::specified::box_::Contain::LAYOUT);
                // (A first child's `break-before` is its parent's, CSS Fragmentation 3 § 3.1, except
                // that it does not leave a box of layout containment: then none, CSS Contain 2
                // § 3.3; WPT css-contain `contain-layout-breaks-002`.)
                let first_in_contained = previous.is_none()
                    && node.primary_styles().is_some_and(|parent| {
                        parent
                            .get_box()
                            .slow_clone_contain()
                            .contains(style::values::specified::box_::Contain::LAYOUT)
                    });
                if (forces(style.get_box().break_before) && !first_in_contained)
                    || (!fragmentable(child, &style)
                        && !contains_layout
                        && self.first_descendant_forces_break(child_id))
                {
                    let boundary = child_top - layout.margin.top.max(0.0);
                    if found.container != Some(parent) && previous.is_some()
                        && boundary >= parent_bottom - EPSILON
                        && (found.fixed_columns || boundary < found.parallel_end)
                    {
                        found.overflow_breaks.push(FlowContinuation { owner: parent, resume: child_id, origin: top, consumed: previous.unwrap().0 - top });
                    } else {
                        found.forced.push(boundary);
                    }
                }
                // pyro: and its last descendants', which a box the columns do not look into takes
                // as its own (WPT css-multicol `spanner-fragmentation-011`).
                if forces(style.get_box().break_after)
                    || (!fragmentable(child, &style) && self.last_descendant_forces_break_after(child_id))
                {
                    let boundary = child_bottom + layout.margin.bottom.max(0.0);
                    if found.container != Some(parent) && boundary >= parent_bottom - EPSILON && (found.fixed_columns || boundary < found.parallel_end) {
                        next_parallel_break = true;
                        found.parallel_break_after = true;
                    } else {
                        found.forced_after.push(boundary);
                    }
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
                    found.strict.push((previous_unit_top.min(bottom.min(child_top) - 2.0 * EPSILON), end.max(child_top + 2.0 * EPSILON)));
                }
                avoid_after = avoids(style.get_box().break_after);
                previous_unit_top = if leaf { child_top } else { child_bottom };
                if let Some((bottom, margin)) = previous.filter(|&(b, _)| child_top >= b) {
                    // The collapsed margin, no more (clearance is no margin).
                    let margin = margin.max(layout.margin.top).max(0.0);
                    found.gaps.push(((child_top - margin).max(bottom), child_top));
                } else if previous.is_none() && layout.margin.top > 0.0 && container_content_top.is_some() {
                    // pyro: the first child's own top margin, which a break before it truncates too
                    // (a column with nothing but it before the break holds no content: `fill_range`).
                    // (The container's first child only: in a box further down, its top is a break
                    // between it and the box before, which is taken; a break before its first child
                    // would leave the box's top shown nowhere.)
                    found.gaps.push(((child_top - layout.margin.top).max(top), child_top));
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
                // (What overflows it is content of these columns too.)
                found.bottom = found.bottom.max(child_top + layout.scrollable_overflow_rect.bottom().abs());
                // pyro: fragmented by the columns (`Breaks::nested`): breakable through, unless
                // it avoids breaks inside.
                found.nested.push((child_id, child_top));
                if avoids_column_break_inside(&style) {
                    found.strict.push((child_top, child_bottom));
                }
                // pyro: what a column of it holds past its row's end (a line or a monolithic box
                // taller than the row, held whole) stays in the outer column the row is in, which
                // holds it whole too: the next outer column does not show it again (Chromium; WPT
                // css-multicol `nested-with-too-tall-line`, `multicol-nested-011`). (From its last
                // layout: the passes make it hold.)
                if let Some(columns) = child.column_fragments().filter(|c| c.nested.is_some()) {
                    // pyro: and no break goes through its lines where its columns show them (from
                    // its last layout): the outer columns break between its rows of lines, which a
                    // balanced outer container's passes then settle on (Chromium; WPT
                    // css-multicol `multicol-nested-margin-004`, `-005`).
                    if columns.fragments.len() > 1 {
                        let mut inner = Found { container: Some(child_id), ..Found::default() };
                        let inner_top = layout.border.top + layout.padding.top;
                        crate::util::ensure_stack(|| self.collect_breaks(child_id, 0.0, &mut inner, Some(inner_top), (false, false)));
                        for &(a, b) in inner.loose.iter().take(4096) {
                            if let Some(fragment) = columns.fragment_at(a).filter(|f| !f.spanner) {
                                let range = (child_top + a + fragment.offset.y, child_top + b + fragment.offset.y);
                                if range.1 > range.0 + EPSILON {
                                    found.strict.push(range);
                                    found.loose.push(range);
                                }
                            }
                        }
                    }
                    for fragment in columns.fragments.iter().filter(|f| !f.spanner) {
                        let start = fragment.top - fragment.offset.y;
                        let end = if fragment.end.is_finite() { fragment.end } else { columns.strip_end };
                        let held = end - start;
                        if fragment.height >= 1.0 && held > fragment.height + 0.5 && held.is_finite() {
                            let range = (child_top + fragment.top, child_top + fragment.top + held);
                            found.strict.push(range);
                            found.loose.push(range);
                            found.whole.push(range);
                        }
                    }
                }
            } else if !cells.is_empty() {
                // pyro: a table with multi-column cells is fragmented with them (Chromium
                // fragments tables): breakable through, unless it avoids breaks inside.
                found.nested.append(&mut cells);
                if avoids_column_break_inside(&style) {
                    found.strict.push((child_top, child_bottom));
                }
            } else if fragmentable(child, &style) {
                if layout.scrollable_overflow_rect.bottom() > layout.size.height + EPSILON {
                    found.overflow_continuations.insert(child_id, child_top + layout.scrollable_overflow_rect.bottom());
                }
                if avoids_column_break_inside(&style) {
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
                // pyro: a box of `box-decoration-break: clone` repeats its borders and padding in
                // each fragment: one holds them with a pixel of its content at least, at either
                // end (so a column of no height makes progress with each: WPT css-multicol
                // `multicol-zero-height-003`), and a column starting inside it shows its
                // block-start ones again first (`Breaks::clones`).
                // (Only a box that holds its content: what overflows one, an out-of-flow box it
                // contains included, is fragmented by the columns alone, over its repeated borders:
                // WPT css-break `box-decoration-break-clone-005`, `-037`.)
                let holds_content = layout.scrollable_overflow_rect.bottom() <= layout.size.height + EPSILON;
                if style.get_border().box_decoration_break == style::values::computed::BoxDecorationBreak::Clone && holds_content {
                    let top_inset = layout.border.top + layout.padding.top;
                    let bottom_inset = layout.border.bottom + layout.padding.bottom;
                    let content = (layout.size.height - top_inset - bottom_inset).max(0.0);
                    let step = content.min(1.0);
                    for range in [(child_top, child_top + top_inset + step), (child_bottom - bottom_inset - step, child_bottom)] {
                        if range.1 > range.0 + EPSILON {
                            found.strict.push(range);
                            found.loose.push(range);
                            found.whole.push(range);
                        }
                    }
                    // (A box of `box-sizing: border-box` keeps its height across its fragments:
                    // the repeated borders and padding take its content's room, not more: WPT
                    // css-break `box-decoration-break-clone-007`.)
                    let border_box = style.get_position().box_sizing == style::computed_values::box_sizing::T::BorderBox;
                    if top_inset > EPSILON && !border_box {
                        found.clones.push((child_top, child_bottom, top_inset));
                    }
                }
                // pyro: not into a column spanner: it is a row of its own, and what overflows it
                // is no content of the columns after it (WPT css-multicol
                // `spanner-fragmentation-005`).
                if !found.container.is_some_and(|container| self.is_column_spanner_of(container, child_id)) {
                    let previous_parallel = found.parallel;
                    let previous_end = found.parallel_end;
                    let previous_after = std::mem::take(&mut found.parallel_break_after);
                    found.parallel_end = found.parallel_end.max(parallel_ends.get(child_index).copied().unwrap_or(f32::NEG_INFINITY));
                    found.parallel |= parallel_tail && found.fixed_columns && layout.size.height <= EPSILON;
                    crate::util::ensure_stack(|| self.collect_breaks(child_id, child_top, found, None, trims));
                    if found.parallel_break_after && parallel_ends.get(child_index).is_some_and(|&end| end > child_bottom + EPSILON) {
                        next_parallel_break = true;
                        found.parallel_break_after = false;
                    }
                    found.parallel_break_after |= previous_after;
                    found.parallel = previous_parallel;
                    found.parallel_end = previous_end;
                }
            } else if let Some(bands) = (!held_whole(child, &style))
                .then(|| self.grid_row_bands(child_id, child_top))
                .flatten()
                .filter(|b| b.len() > 1)
            {
                // pyro: a grid breaks between its rows as a table does (Chromium fragments grids; WPT
                // css-grid `alignment/grid-item-mixed-baseline-004`).
                for &band in &bands {
                    found.strict.push(band);
                    found.loose.push(band);
                }
                found.strict.push((child_top, bands[0].1));
                found.strict.push((bands[bands.len() - 1].0, child_bottom));
            } else if let Some(bands) = child.flags.is_table_root().then(|| self.table_row_bands(child_id, child_top)).filter(|b| b.len() > 1) {
                // pyro: a table breaks between its rows (Chromium fragments tables): each row,
                // and its first and last rows with the table's edges, kept together.
                for &band in &bands {
                    found.strict.push(band);
                    found.loose.push(band);
                }
                found.strict.push((child_top, bands[0].1));
                found.strict.push((bands[bands.len() - 1].0, child_bottom));
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
    parallel_end: f32,
    fixed_columns: bool,
    parallel: bool,
    parallel_floats: Vec<(crate::NodeId, f32, f32, f32)>,
    strict: Vec<(f32, f32)>,
    /// pyro: see [`Breaks::whole`].
    whole: Vec<(f32, f32)>,
    /// pyro: see [`Breaks::nested`].
    nested: Vec<(crate::NodeId, f32)>,
    /// pyro: see [`BaseDocument::multicol_breaks`].
    nested_breakable: bool,
    /// pyro: the multi-column container whose strip this is.
    container: Option<crate::NodeId>,
    loose: Vec<(f32, f32)>,
    gaps: Vec<(f32, f32)>,
    /// pyro: see [`Breaks::trims`].
    trims: Vec<(f32, f32, f32, f32)>,
    /// pyro: see [`Breaks::forced`]; `forced_after` from `break-after` (dropped after the last child).
    forced: Vec<f32>,
    forced_after: Vec<f32>,
    parallel_break_after: bool,
    overlapping_starts: Vec<FlowContinuation>,
    overflow_breaks: Vec<FlowContinuation>,
    overflow_parents: Vec<FlowContinuation>,
    overflow_continuations: std::collections::HashMap<crate::NodeId, f32>,
    /// The floats' block ranges.
    floats: Vec<(f32, f32)>,
    /// pyro: see [`Breaks::baselines`].
    baselines: Vec<f32>,
    /// pyro: see [`Breaks::clones`].
    clones: Vec<(f32, f32, f32)>,
    /// The furthest bottom of a box or line.
    bottom: f32,
    /// The furthest bottom margin edge of the container's children.
    flow_end: f32,
}

/// pyro: whether `style`'s `break-inside` avoids column breaks (`avoid`, `avoid-column`; not
/// `avoid-page`, which is for pages only: CSS Fragmentation 3 § 3.2).
fn avoids_column_break_inside(style: &ComputedValues) -> bool {
    matches!(style.get_box().break_inside, BreakWithin::Avoid | BreakWithin::AvoidColumn)
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
    // pyro: so is one of a writing mode orthogonal to its parent's (an orthogonal flow is not
    // fragmented in its parent's block axis: CSS Writing Modes 3 § 7.3; Chromium; WPT css-multicol
    // `column-fill-balance-orthog-block-001`).
    let parent_vertical = node
        .layout_parent
        .get()
        .and_then(|parent| node.tree().get(parent))
        .and_then(|parent| parent.primary_styles().map(|s| s.writing_mode.is_vertical()));
    if parent_vertical.is_some_and(|vertical| vertical != style.writing_mode.is_vertical()) {
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
    use crate::{Attribute, DocumentConfig, DocumentMutator, qual_name};
    use blitz_traits::shell::{ColorScheme, Viewport};

    /// A `<div>` with the `style` attribute `.0` and the children `.1`.
    struct Div(String, Vec<Div>);

    fn div(css: &str, children: Vec<Div>) -> Div {
        Div(css.to_string(), children)
    }

    /// A document of these `<div>`s in its body (no margin), laid out in an 800 by 600 viewport.
    fn laid_out(body: Vec<Div>) -> BaseDocument {
        fn build(m: &mut DocumentMutator<'_>, d: Div) -> crate::NodeId {
            let id = m.create_element(qual_name!("div", html), vec![Attribute { name: qual_name!("style"), value: d.0 }]);
            let children: Vec<crate::NodeId> = d.1.into_iter().map(|c| build(m, c)).collect();
            m.append_children(id, &children);
            id
        }
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        let root = doc.root_node().id;
        let mut m = doc.mutate();
        let html = m.create_element(qual_name!("html", html), vec![]);
        let body_id = m.create_element(qual_name!("body", html), vec![Attribute { name: qual_name!("style"), value: "margin:0".into() }]);
        let children: Vec<crate::NodeId> = body.into_iter().map(|d| build(&mut m, d)).collect();
        m.append_children(body_id, &children);
        m.append_children(html, &[body_id]);
        m.append_children(root, &[html]);
        drop(m);
        doc.resolve(0.0);
        doc
    }

    /// How many column fragments each multi-column container of `doc` has (none: not listed).
    fn fragment_counts(doc: &BaseDocument) -> Vec<usize> {
        doc.nodes.iter().filter_map(|(_, node)| node.column_fragments().map(|c| c.fragments.len())).collect()
    }

    fn spanners(n: usize) -> Vec<Div> {
        (0..n).map(|_| div("column-span:all;height:1px", vec![])).collect()
    }

    #[test]
    fn spanner_neighbor_work_scales_with_siblings() {
        let work = |count, columns, heading_first| {
            let children = (0..count * 2).map(|index| if (index % 2 == 0) == heading_first {
                div("column-span:all;height:1px", vec![])
            } else { div("height:10px", vec![]) }).collect();
            SPANNER_VISITS.with(|visits| visits.set(0));
            let doc = laid_out(vec![div(&format!("width:120px;columns:{columns};gap:0"), children)]);
            assert!(!fragment_counts(&doc).is_empty());
            SPANNER_VISITS.with(|visits| visits.get())
        };
        for (columns, first) in [(2, false), (3, false), (2, true)] {
            let small = work(1000, columns, first);
            let large = work(4000, columns, first);
            assert!(small >= 2000 && large >= 8000 && large <= small * 4 + 16, "{small} -> {large}");
        }
    }

    /// pyro: a stacking context hoisted out of a multi-column container that is none (a `z-index`
    /// box a break goes through) is hit in each column that shows a piece of it, as it is painted
    /// there, and nowhere else.
    #[test]
    fn a_hoisted_box_in_columns_is_hit_where_it_is_painted() {
        // Columns 75px tall: the box is 50..150 in the strip, 50..75 of the first column (the
        // bottom of it) and the whole of the second.
        let mut doc = laid_out(vec![div(
            "columns:2;column-gap:0;width:200px",
            vec![div("height:50px", vec![]), div("position:relative;z-index:1;height:100px", vec![])],
        )]);
        let ids: Vec<crate::NodeId> = doc.nodes.iter().map(|(id, _)| id).collect();
        let z = *ids.last().unwrap();
        let hit = |doc: &mut BaseDocument, x: f32, y: f32| doc.hit(x, y).map(|h| h.node_id);
        assert_eq!(hit(&mut doc, 50.0, 60.0), Some(z), "its piece in the first column");
        assert_eq!(hit(&mut doc, 150.0, 30.0), Some(z), "its piece in the second column");
        assert_ne!(hit(&mut doc, 50.0, 120.0), Some(z), "not below the container, where the strip has it");
    }

    /// pyro: (hit order, with this module's document helpers) a positioned sibling after a flex
    /// container in one paint layer paints over the container's overflowing item, at the positioned
    /// level after the inline content the item paints with: it takes the hit there, the item's
    /// pre-pass (`Node::items_in_phases`) goes after it. Review cycle 3, finding 4.
    #[test]
    fn a_positioned_sibling_over_an_overflowing_flex_item_is_hit() {
        let mut doc = laid_out(vec![div(
            "overflow:hidden",
            vec![
                div("display:flex;height:50px", vec![div("width:100px;height:120px;align-self:flex-start", vec![])]),
                div("position:relative;height:100px", vec![]),
            ],
        )]);
        let ids: Vec<crate::NodeId> = doc.nodes.iter().map(|(id, _)| id).collect();
        let (item, next) = (ids[ids.len() - 2], ids[ids.len() - 1]);
        let hit = |doc: &mut BaseDocument, x: f32, y: f32| doc.hit(x, y).map(|h| h.node_id);
        assert_eq!(hit(&mut doc, 50.0, 90.0), Some(next), "the positioned sibling over the item");
        assert_eq!(hit(&mut doc, 50.0, 30.0), Some(item), "the item above the sibling");
    }

    /// pyro: (hit order) a miss through nested positioned wrappers, each beside a flex container
    /// whose items are hit in a pre-pass, tests each wrapper once: the pre-pass and the ordinary pass
    /// visited the positioned children both, 2^depth tests in all. Review cycle 4, finding 1.
    #[test]
    fn a_miss_through_nested_positioned_wrappers_beside_flex_items_is_linear() {
        fn nest(depth: usize) -> Div {
            let flex = div("display:flex;height:10px", vec![div("width:10px;height:10px", vec![])]);
            let inner = if depth == 0 { vec![flex] } else { vec![nest(depth - 1), flex] };
            div("position:relative;pointer-events:none;width:400px", inner)
        }
        let mut doc = laid_out(vec![div("overflow:hidden;pointer-events:none", vec![nest(18)])]);
        let boxes = doc.nodes.len();
        crate::util::HIT_CALLS.with(|calls| calls.set(0));
        let _ = doc.hit(200.0, 5.0);
        let calls = crate::util::HIT_CALLS.with(|calls| calls.get());
        assert!(calls <= 2 * boxes, "{calls} box tests for {boxes} boxes");
    }

    /// pyro: (hit order) a miss through flex containers nested in each other's items tests each
    /// box a bounded number of times (the item pre-pass and the ordinary pass must not each walk the
    /// whole nest). Review cycle 4, finding 1.
    #[test]
    fn a_miss_through_nested_flex_items_is_linear() {
        fn nest(depth: usize) -> Div {
            let inner = if depth == 0 { vec![] } else { vec![nest(depth - 1)] };
            div("display:flex;pointer-events:none;width:400px;min-height:10px", vec![div("width:400px;min-height:10px", inner)])
        }
        let mut doc = laid_out(vec![div("pointer-events:none", vec![nest(18)])]);
        let boxes = doc.nodes.len();
        crate::util::HIT_CALLS.with(|calls| calls.set(0));
        let _ = doc.hit(200.0, 5.0);
        let calls = crate::util::HIT_CALLS.with(|calls| calls.get());
        assert!(calls <= 3 * boxes, "{calls} box tests for {boxes} boxes");
    }

    /// pyro: (paint tree) many sibling inline opacity groups each keeping an absolutely positioned
    /// box are restored to their owners and their context in work linear in the boxes: tree order is
    /// numbered once, not searched in sibling lists per comparison. Review cycle 4, finding 2.
    #[test]
    fn many_inline_owners_restore_in_linear_tree_order_work() {
        let groups: Vec<Div> =
            (0..2000).map(|_| div("display:inline;opacity:.9", vec![div("position:absolute;width:1px;height:1px", vec![])])).collect();
        crate::util::TREE_ORDER_COMPARISONS.with(|calls| calls.set(0));
        crate::util::TREE_ORDER_WALKS.with(|walks| walks.set(0));
        let doc = laid_out(vec![div("width:390px", groups)]);
        let boxes = doc.nodes.len();
        let comparisons = crate::util::TREE_ORDER_COMPARISONS.with(|calls| calls.get());
        let walks = crate::util::TREE_ORDER_WALKS.with(|walks| walks.get());
        assert!(comparisons <= 8 * boxes, "{comparisons} sibling-list entries searched for {boxes} boxes");
        assert!(walks <= 8 * boxes, "{walks} nodes numbered for {boxes} boxes");
    }

    /// pyro: (hit order) a miss in a wide flex container whose items are hit in a pre-pass (all
    /// `pointer-events: none`) takes child-list steps linear in its items: its second visit, for its
    /// own box, skipped its items by searching the item list for each. Review cycle 5, finding 2.
    #[test]
    fn a_miss_in_a_wide_flex_container_takes_linear_steps() {
        let items: Vec<Div> = (0..3000).map(|_| div("width:1px;height:10px;pointer-events:none", vec![])).collect();
        let mut doc = laid_out(vec![div("pointer-events:none", vec![div("display:flex;flex-wrap:wrap;width:400px;pointer-events:none", items)])]);
        let boxes = doc.nodes.len();
        crate::util::HIT_STEPS.with(|steps| steps.set(0));
        let _ = doc.hit(200.0, 5.0);
        let steps = crate::util::HIT_STEPS.with(|steps| steps.get());
        assert!(steps > 0 && steps <= 8 * boxes, "{steps} child-list steps for {boxes} boxes");
    }

    /// pyro: (hit order) text over a `z-index: -1` box behind it takes the hit: the negative
    /// z-index boxes paint under the inline content (CSS 2.1 Appendix E steps 3 and 7) and are hit
    /// after it. They were hit before the root's text. Review cycle 5, finding 3.
    #[test]
    fn text_over_a_negative_z_index_box_takes_the_hit() {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        let root = doc.root_node().id;
        let mut m = doc.mutate();
        let html = m.create_element(qual_name!("html", html), vec![]);
        let body = m.create_element(qual_name!("body", html), vec![Attribute { name: qual_name!("style"), value: "margin:0".into() }]);
        let para = m.create_element(
            qual_name!("div", html),
            vec![Attribute { name: qual_name!("style"), value: "font:20px/20px monospace;width:400px;position:relative;z-index:0".into() }],
        );
        let text = m.create_text_node("XXXXXXXXXX");
        let behind = m.create_element(
            qual_name!("div", html),
            vec![Attribute { name: qual_name!("style"), value: "position:absolute;z-index:-1;left:0;top:0;width:400px;height:20px".into() }],
        );
        m.append_children(para, &[text, behind]);
        m.append_children(body, &[para]);
        m.append_children(html, &[body]);
        m.append_children(root, &[html]);
        drop(m);
        doc.resolve(0.0);
        let hit = doc.hit(10.0, 10.0).map(|h| h.node_id);
        assert!(hit.is_some_and(|hit| hit == text || hit == para), "the text (or its block), not the box behind it ({behind:?}): {hit:?}");
    }

    /// pyro: (inline effects) deeply nested faded inline boxes under a box that paints find which of them
    /// paint at their own entry in one walk up from the content: each faded box walked up to the root
    /// again (depth^2). Review cycle 5, finding 5.
    #[test]
    fn nested_faded_inlines_find_their_entries_in_one_walk() {
        fn nest(depth: usize) -> Div {
            let inner = if depth == 0 { vec![div("display:inline-block;width:10px;height:10px", vec![])] } else { vec![nest(depth - 1)] };
            div("display:inline;opacity:.9", inner)
        }
        // (One box that paints, at the top: each faded box below found it by walking all the way up.)
        let doc = laid_out(vec![div("width:400px", vec![div("display:inline;background:yellow", vec![nest(199)])])]);
        let leaf = doc.nodes.iter().map(|(id, _)| id).last().unwrap();
        let parent = doc.nodes[leaf].parent.unwrap();
        let root = doc.nodes.iter().map(|(id, _)| id).find(|&id| doc.nodes[id].flags.is_inline_root()).unwrap();
        crate::util::CHAIN_STEPS.with(|steps| steps.set(0));
        let chain = doc.inline_span_positioned_chain(parent, root);
        let steps = crate::util::CHAIN_STEPS.with(|steps| steps.get());
        assert_eq!(chain.len(), 200, "the faded boxes under a painted one");
        assert!(steps <= 2 * 205, "{steps} ancestor steps for 201 nested boxes");
    }

    /// pyro: a hoisted box is hit only where paint shows it: in the columns its strip range
    /// reaches, not in a spanner's row (paint leaves those out), even where it reaches into one.
    #[test]
    fn a_hoisted_box_is_not_hit_in_a_spanners_row() {
        // Two columns of 50px, then the spanner's row (50..80), then two columns of 25px: the box
        // runs from the first columns through the spanner into the last ones.
        let mut doc = laid_out(vec![div(
            "columns:2;column-gap:0;width:200px",
            vec![div(
                "position:relative;z-index:1",
                vec![div("height:100px", vec![]), div("column-span:all;height:30px", vec![]), div("height:50px", vec![])],
            )],
        )]);
        let ids: Vec<crate::NodeId> = doc.nodes.iter().map(|(id, _)| id).collect();
        let z = ids[ids.len() - 4];
        let inside: Vec<crate::NodeId> = ids[ids.len() - 4..].to_vec();
        let hit = |doc: &mut BaseDocument, x: f32, y: f32| doc.hit(x, y).map(|h| h.node_id);
        assert!(hit(&mut doc, 150.0, 25.0).is_some_and(|h| inside.contains(&h)), "its second column");
        assert!(hit(&mut doc, 150.0, 95.0).is_some_and(|h| inside.contains(&h)), "its last row's second column");
        let over_spanner = hit(&mut doc, 150.0, 65.0);
        assert!(!over_spanner.is_some_and(|h| inside.contains(&h)), "not over the spanner's row, where it is not painted: {over_spanner:?} (box {z:?})");
    }

    /// pyro: the runs that only measure containers make no more than `MEASURE_FRAGMENT_BUDGET`
    /// fragments in all, however often each is measured (grid items of a thousand columns each,
    /// measured for their tracks).
    #[test]
    fn measure_runs_are_held_to_their_budget() {
        // (Each measured twice: 1.2 million fragments unbounded.)
        let items: Vec<Div> = (0..600)
            .map(|_| div("column-count:1000;column-gap:0", vec![div("height:100000px", vec![])]))
            .collect();
        MEASURED_FRAGMENTS.with(|m| m.set(0));
        let doc = laid_out(vec![div("display:grid;grid-template-columns:repeat(6, auto);width:600px", items)]);
        let measured = MEASURED_FRAGMENTS.with(|m| m.get());
        assert!(measured > 0 && measured <= MEASURE_FRAGMENT_BUDGET, "{measured} fragments measured");
        assert!(fragment_counts(&doc).iter().sum::<usize>() <= COLUMN_FRAGMENT_BUDGET);
    }

    /// pyro: mapping a container's boxes into its columns (its scrollable overflow) visits the
    /// fragments each box reaches, not all of them per box: a short scroller with a box per
    /// column costs boxes, not boxes times columns.
    #[test]
    fn mapping_boxes_into_columns_visits_the_columns_they_reach() {
        let boxes: Vec<Div> = (0..2_000).map(|_| div("height:10px", vec![])).collect();
        MAPPED_FRAGMENTS.with(|m| m.set(0));
        let doc = laid_out(vec![div("columns:2;column-fill:auto;column-gap:0;height:10px;width:200px;overflow:auto", boxes)]);
        let mapped = MAPPED_FRAGMENTS.with(|m| m.get());
        assert!(fragment_counts(&doc).iter().any(|&c| c >= 2_000), "a column per box: {:?}", fragment_counts(&doc));
        assert!(mapped <= 20_000, "{mapped} fragments visited for 2000 boxes");
    }

    /// pyro: a hoisted box inside an `overflow: clip` box broken across the columns is hit in
    /// each column within that box's clip, as Chromium hits it and paint shows it (blitz-paint
    /// `draw_hoisted_children`, `hoisted_child_clips_to`), and not past the clip.
    #[test]
    fn a_hoisted_box_in_a_clipping_box_is_hit_within_the_clip() {
        let mut doc = laid_out(vec![div(
            "columns:2;column-gap:0;width:200px",
            vec![
                div("height:50px", vec![]),
                div("overflow:clip;height:100px", vec![div("position:relative;z-index:1;height:100px;width:150px", vec![])]),
            ],
        )]);
        let ids: Vec<crate::NodeId> = doc.nodes.iter().map(|(id, _)| id).collect();
        let z = *ids.last().unwrap();
        let hit = |doc: &mut BaseDocument, x: f32, y: f32| doc.hit(x, y).map(|h| h.node_id);
        assert_eq!(hit(&mut doc, 50.0, 60.0), Some(z), "its piece in the first column");
        assert_eq!(hit(&mut doc, 190.0, 30.0), Some(z), "its piece in the second column, inside the clip");
        assert_ne!(hit(&mut doc, 240.0, 30.0), Some(z), "not past the clip");
    }

    /// pyro: no more than `MAX_FRAGMENTS` per container, however its fragments are made.
    #[test]
    fn adjacent_spanners_are_held_to_the_fragment_cap() {
        let alone = laid_out(vec![div("columns:2;column-gap:0;width:400px", spanners(12_000))]);
        assert_eq!(fragment_counts(&alone), vec![MAX_FRAGMENTS], "a container of its own");
        // (Nested, of an auto height, in outer columns of a definite height: `nested_span_ctx`.)
        let nested = laid_out(vec![div(
            "columns:2;column-fill:auto;height:100px;width:400px",
            vec![div("columns:2;column-gap:0", spanners(12_000))],
        )]);
        let counts = fragment_counts(&nested);
        assert!(counts.iter().all(|&c| c <= MAX_FRAGMENTS), "{counts:?}");
        assert!(counts.contains(&MAX_FRAGMENTS), "the inner one reaches the cap: {counts:?}");
    }

    #[test]
    fn alternating_spanners_are_held_to_the_fragment_cap() {
        let sections = || -> Vec<Div> {
            (0..6_000).flat_map(|_| [div("height:20px", vec![]), div("column-span:all;height:1px", vec![])]).collect()
        };
        let alone = laid_out(vec![div("columns:2;column-gap:0;height:10px;width:400px", sections())]);
        assert_eq!(fragment_counts(&alone), vec![MAX_FRAGMENTS], "a container of its own");
        let nested = laid_out(vec![div(
            "columns:2;column-fill:auto;height:100px;width:400px",
            vec![div("columns:2;column-gap:0", sections())],
        )]);
        let counts = fragment_counts(&nested);
        assert!(counts.iter().all(|&c| c <= MAX_FRAGMENTS), "{counts:?}");
        assert!(counts.contains(&MAX_FRAGMENTS), "the inner one reaches the cap: {counts:?}");
    }

    #[test]
    fn nested_containers_are_held_to_the_fragment_cap() {
        // A thousand columns in each outer column, a pixel tall: rows without end (`nested_ctx`),
        // and rows of an explicit `column-height` (`nested_rows_ctx`).
        for inner in ["column-count:1000;column-gap:0", "column-count:1000;column-gap:0;column-height:1px;column-wrap:wrap"] {
            let doc = laid_out(vec![div(
                "columns:2;column-fill:auto;height:1px;width:400px",
                vec![div(inner, vec![div("height:100000000px", vec![])]), div(inner, vec![div("height:100000000px", vec![])])],
            )]);
            let counts = fragment_counts(&doc);
            assert!(counts.iter().all(|&c| c <= MAX_FRAGMENTS), "{inner}: {counts:?}");
            assert!(counts.iter().sum::<usize>() <= COLUMN_FRAGMENT_BUDGET, "{inner}: {counts:?}");
            // (A row of a thousand short of the cap stops there: the rest is its last column.)
            assert!(counts.iter().any(|&c| c > MAX_FRAGMENTS - 1000), "{inner}: an inner one reaches the cap: {counts:?}");
        }
    }

    /// pyro: no more than `COLUMN_FRAGMENT_BUDGET` in all, and none past it (no minimum each).
    #[test]
    fn many_containers_are_held_to_the_document_budget() {
        let containers: Vec<Div> = (0..30)
            .map(|_| div("columns:1;column-fill:auto;height:1px;width:10px", vec![div("height:100000px", vec![])]))
            .collect();
        let doc = laid_out(containers);
        let counts = fragment_counts(&doc);
        assert_eq!(counts.iter().sum::<usize>(), COLUMN_FRAGMENT_BUDGET, "{counts:?}");
        assert!(counts.iter().all(|&c| c <= MAX_FRAGMENTS), "{counts:?}");
        // (The containers past the budget have no fragments: not listed.)
        assert_eq!(counts.len(), COLUMN_FRAGMENT_BUDGET / MAX_FRAGMENTS, "{counts:?}");
    }

    fn breaks(strict: Vec<(f32, f32)>, loose: Vec<(f32, f32)>, end: f32) -> Breaks {
        Breaks {
            start: 0.0,
            end,
            strict: merge(strict),
            loose: merge(loose),
            whole_in_empty_columns: false,
            one_column_when_empty: false,
            whole: Vec::new(),
            gaps: Vec::new(),
            trims: Vec::new(),
            forced: Vec::new(),
            overflow_breaks: Vec::new(),
            overlapping_starts: Vec::new(),
            overflow_parents: Vec::new(),
            overflow_continuations: Default::default(),
            parallel_floats: Vec::new(),
            nested: Vec::new(),
            baselines: Vec::new(),
            clones: Vec::new(),
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
            strip_end: end,
            x,
            width: 100.0,
            top,
            height: 100.0,
            offset: Point { x: offset.0, y: offset.1 },
            spanner: false,
            extended: 0.0,
            exact_x: f32::NAN,
            block_rounding_error: 0.0,
        }
    }

    #[test]
    fn fragments_map_both_ways() {
        let columns = ColumnFragments {
            consumed_by_owner: Default::default(),
            continuation_ends: Default::default(),
            run_wrappers: Default::default(),
            wrapper_initial_consumption: Default::default(),
            first_after_spanner: Default::default(),
            strip_end: f32::INFINITY,
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
            consumed_by_owner: Default::default(),
            continuation_ends: Default::default(),
            run_wrappers: Default::default(),
            wrapper_initial_consumption: Default::default(),
            first_after_spanner: Default::default(),
            strip_end: f32::INFINITY,
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
            strip_end: 230.0,
            x: 0.0,
            width: 220.0,
            top: 100.0,
            height: 30.0,
            offset: Point { x: 0.0, y: -100.0 },
            spanner: true,
            extended: 0.0,
            exact_x: f32::NAN,
            block_rounding_error: 0.0,
        };
        let columns = ColumnFragments {
            continuation_ends: Default::default(),
            consumed_by_owner: Default::default(),
            run_wrappers: Default::default(),
            wrapper_initial_consumption: Default::default(),
            first_after_spanner: Default::default(),
            strip_end: f32::INFINITY,
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

/// pyro: the resolved top padding of `node` (its percentages against `basis`).
fn padding_top_of(node: &crate::Node, basis: Option<f32>) -> f32 {
    node.layout_style().padding().top.resolve_or_zero(basis, resolve_calc_value)
}
