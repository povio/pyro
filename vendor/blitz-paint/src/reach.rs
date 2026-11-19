//! pyro: which intervals along a line reach into a query interval, output-sensitively (the atomic
//! inlines of a line an inline box after them paints over: `ElementCx::draw_inline_boxes_over_atomics`).

/// Intervals `[start, end)` sorted by start, with a segment tree of the furthest end in each run
/// of them: a query visits only the subtrees holding an interval that reaches it, O((k + 1) log n)
/// nodes for k answers, whatever intervals lie between (a wide one early in the line keeps
/// reaching every later query without making the others answers).
pub struct ReachIndex {
    starts: Vec<f64>,
    /// `max_end[size + i]` is interval `i`'s end; `max_end[n]` the furthest of its children's.
    max_end: Vec<f64>,
    size: usize,
}

impl ReachIndex {
    /// `intervals` must be sorted by start.
    pub fn new(intervals: impl ExactSizeIterator<Item = (f64, f64)>) -> Self {
        let size = intervals.len().next_power_of_two();
        let mut starts = Vec::with_capacity(intervals.len());
        let mut max_end = vec![f64::NEG_INFINITY; 2 * size];
        for (i, (start, end)) in intervals.enumerate() {
            debug_assert!(starts.last().is_none_or(|&last| last <= start), "sorted by start");
            starts.push(start);
            max_end[size + i] = end;
        }
        for n in (1..size).rev() {
            max_end[n] = max_end[2 * n].max(max_end[2 * n + 1]);
        }
        Self { starts, max_end, size }
    }

    /// The indices (ascending) of the intervals that start before `end` and end after `start`,
    /// and how many tree nodes the search visited.
    pub fn query(&self, start: f64, end: f64) -> (Vec<usize>, usize) {
        let upto = self.starts.partition_point(|&s| s < end);
        let mut out = Vec::new();
        let mut visited = 0;
        let mut stack = vec![(1usize, 0usize, self.size)];
        while let Some((node, lo, hi)) = stack.pop() {
            visited += 1;
            if lo >= upto || self.max_end[node] <= start {
                continue;
            }
            if hi - lo == 1 {
                out.push(lo);
                continue;
            }
            let mid = (lo + hi) / 2;
            stack.push((2 * node + 1, mid, hi));
            stack.push((2 * node, lo, mid));
        }
        (out, visited)
    }
}
