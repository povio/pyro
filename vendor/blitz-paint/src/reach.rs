//! pyro: which atomic inlines of a line each inline box after them overlaps, output-sensitively
//! (an inline box after an atomic inline paints over it where they meet:
//! `ElementCx::draw_inline_boxes_over_atomics`).
//!
//! A sweep along the line: the boxes open at each point are kept, per kind, in a segment tree over
//! the cross axis (the block axis) with a count of live entries in each subtree. A box that opens
//! queries the tree of the other kind for the open boxes it meets across the line: only subtrees
//! with live entries in its cross extent are visited, so the work is O((n + k) log² n) for n boxes
//! and k overlapping pairs, whatever lies between them along or across the line (a wide box early
//! in the line, small boxes shifted up and down by `vertical-align`).

/// One box of a line: where it starts and ends along the line, its extent across it, and its
/// rank in paint order (tree order).
#[derive(Clone, Copy, Debug)]
pub struct ReachItem {
    pub start: f64,
    pub end: f64,
    pub cross_start: f64,
    pub cross_end: f64,
    pub rank: usize,
}

/// The open boxes of one kind, by their cross extents (half-open), over the cross coordinates
/// of every box of the line.
struct CrossTree {
    /// The distinct cross coordinates: leaf `i` is `[coords[i], coords[i + 1])`.
    coords: Vec<f64>,
    size: usize,
    /// Per node: the boxes whose extent covers the node's range and not its parent's (alive or
    /// not, dead ones dropped when met), and the live entries in its subtree.
    lists: Vec<Vec<usize>>,
    live: Vec<usize>,
}

impl CrossTree {
    fn new(coords: Vec<f64>) -> Self {
        let leaves = coords.len().saturating_sub(1).max(1);
        let size = leaves.next_power_of_two();
        Self { coords, size, lists: vec![Vec::new(); 2 * size], live: vec![0; 2 * size] }
    }

    /// The leaves `[lo, hi)` a cross extent covers (empty for an empty extent).
    fn leaves(&self, start: f64, end: f64) -> (usize, usize) {
        let lo = self.coords.partition_point(|&c| c < start);
        let hi = self.coords.partition_point(|&c| c < end);
        (lo, hi.min(self.size))
    }

    /// The canonical nodes covering leaves `[lo, hi)`.
    fn canonical(&self, lo: usize, hi: usize, out: &mut Vec<usize>) {
        out.clear();
        let mut stack = vec![(1usize, 0usize, self.size)];
        while let Some((node, a, b)) = stack.pop() {
            if b <= lo || a >= hi {
                continue;
            }
            if lo <= a && b <= hi {
                out.push(node);
                continue;
            }
            let mid = (a + b) / 2;
            stack.push((2 * node, a, mid));
            stack.push((2 * node + 1, mid, b));
        }
    }

    fn insert(&mut self, id: usize, nodes: &[usize]) {
        for &node in nodes {
            self.lists[node].push(id);
            let mut n = node;
            while n >= 1 {
                self.live[n] += 1;
                n /= 2;
            }
        }
    }

    fn remove(&mut self, nodes: &[usize]) {
        for &node in nodes {
            let mut n = node;
            while n >= 1 {
                self.live[n] -= 1;
                n /= 2;
            }
        }
    }

    /// Calls `found` for each open box whose extent meets leaves `[lo, hi)` (a box may be found
    /// more than once: once per canonical node of it met); `alive` says which are open. Returns
    /// the nodes and entries visited.
    fn query(&mut self, lo: usize, hi: usize, alive: &[bool], mut found: impl FnMut(usize)) -> usize {
        let mut work = 0;
        if lo >= hi {
            return 0;
        }
        let mut stack = vec![(1usize, 0usize, self.size)];
        while let Some((node, a, b)) = stack.pop() {
            work += 1;
            if b <= lo || a >= hi || self.live[node] == 0 {
                continue;
            }
            // Every box listed here covers this whole range, which meets the query's.
            let list = &mut self.lists[node];
            let mut i = 0;
            while i < list.len() {
                work += 1;
                if alive[list[i]] {
                    found(list[i]);
                    i += 1;
                } else {
                    list.swap_remove(i);
                }
            }
            if b - a > 1 {
                let mid = (a + b) / 2;
                stack.push((2 * node, a, mid));
                stack.push((2 * node + 1, mid, b));
            }
        }
        work
    }
}

/// For each of `boxes` (the inline box fragments of a line), the indices of the `atomics` it
/// overlaps on both axes and comes after in paint order, ascending by rank; and the work done
/// (tree nodes and list entries visited), for tests.
pub fn overlapping_atomics(atomics: &[ReachItem], boxes: &[ReachItem]) -> (Vec<Vec<usize>>, usize) {
    let n = atomics.len() + boxes.len();
    let item = |id: usize| if id < atomics.len() { &atomics[id] } else { &boxes[id - atomics.len()] };
    let mut coords: Vec<f64> = (0..n).flat_map(|id| [item(id).cross_start, item(id).cross_end]).collect();
    coords.sort_by(f64::total_cmp);
    coords.dedup();
    // Ids: atomics first, then boxes. Each kind has its own tree.
    let mut trees = [CrossTree::new(coords.clone()), CrossTree::new(coords)];
    let kind = |id: usize| usize::from(id >= atomics.len());
    // Events along the line: ends before starts at one point (half-open extents); boxes with no
    // extent open nothing.
    let mut events: Vec<(f64, bool, usize)> = Vec::with_capacity(2 * n);
    for id in 0..n {
        let it = item(id);
        if it.end > it.start && it.cross_end > it.cross_start {
            events.push((it.start, true, id));
            events.push((it.end, false, id));
        }
    }
    events.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut alive = vec![false; n];
    let mut nodes: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); boxes.len()];
    let mut seen: Vec<usize> = vec![usize::MAX; n];
    let mut work = 0;
    for (stamp, &(_, opens, id)) in events.iter().enumerate() {
        let k = kind(id);
        if !opens {
            alive[id] = false;
            let canonical = core::mem::take(&mut nodes[id]);
            trees[k].remove(&canonical);
            continue;
        }
        let it = *item(id);
        let (lo, hi) = trees[k].leaves(it.cross_start, it.cross_end);
        // The open boxes of the other kind it meets across the line.
        let mut met: Vec<usize> = Vec::new();
        work += trees[1 - k].query(lo, hi, &alive, |other| {
            if seen[other] != stamp {
                seen[other] = stamp;
                met.push(other);
            }
        });
        for other in met {
            let (atomic, fragment) = if k == 0 { (id, other - atomics.len()) } else { (other, id - atomics.len()) };
            if atomics[atomic].rank < boxes[fragment].rank {
                out[fragment].push(atomic);
            }
        }
        let mut canonical = Vec::new();
        trees[k].canonical(lo, hi, &mut canonical);
        trees[k].insert(id, &canonical);
        nodes[id] = canonical;
        alive[id] = true;
    }
    for list in &mut out {
        list.sort_by_key(|&atomic| atomics[atomic].rank);
    }
    (out, work)
}
