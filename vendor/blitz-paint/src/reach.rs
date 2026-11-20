//! pyro: which atomic inlines of a line each inline box after them overlaps, output-sensitively
//! (an inline box after an atomic inline paints over it where they meet:
//! `ElementCx::draw_inline_boxes_over_atomics`).
//!
//! A sweep along the line: the boxes open at each point are kept, per kind, in a segment tree over
//! the cross axis (the block axis), each node's boxes ordered by paint rank with the lowest and
//! highest rank open in its subtree. A box that opens queries the tree of the other kind for the
//! open boxes it meets across the line that paint on the other side of it (an atomic before a
//! fragment): only subtrees holding such a box in its cross extent are visited and only such boxes
//! read, so the work is O((n + k) log² n) for n boxes and k repainted pairs, whatever lies between
//! them along or across the line (a wide box early in the line, small boxes shifted up and down by
//! `vertical-align`, boxes overlapping in the wrong paint order).

use std::collections::BTreeSet;

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
/// of every box of the line. Each node keeps the boxes it is canonical for ordered by rank, and the
/// lowest and highest rank open in its subtree: a query for boxes of lower (or higher) rank than
/// its own skips the subtrees that hold none and reads only those of a node's boxes that qualify.
struct CrossTree {
    /// The distinct cross coordinates: leaf `i` is `[coords[i], coords[i + 1])`.
    coords: Vec<f64>,
    size: usize,
    sets: Vec<BTreeSet<(usize, usize)>>,
    min_rank: Vec<usize>,
    max_rank: Vec<usize>,
}

/// Which ranks a query wants: below its own (a fragment's atomics) or above (an atomic's later
/// fragments).
#[derive(Clone, Copy)]
enum Ranks {
    Below(usize),
    Above(usize),
}

impl CrossTree {
    fn new(coords: Vec<f64>) -> Self {
        let leaves = coords.len().saturating_sub(1).max(1);
        let size = leaves.next_power_of_two();
        Self {
            coords,
            size,
            sets: vec![BTreeSet::new(); 2 * size],
            min_rank: vec![usize::MAX; 2 * size],
            max_rank: vec![0; 2 * size],
        }
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

    /// Brings the subtree ranks of `node` and its ancestors up to date.
    fn update(&mut self, mut node: usize) {
        while node >= 1 {
            let own = &self.sets[node];
            let (mut low, mut high) =
                (own.first().map_or(usize::MAX, |&(rank, _)| rank), own.last().map_or(0, |&(rank, _)| rank));
            if node < self.size {
                low = low.min(self.min_rank[2 * node]).min(self.min_rank[2 * node + 1]);
                high = high.max(self.max_rank[2 * node]).max(self.max_rank[2 * node + 1]);
            }
            self.min_rank[node] = low;
            self.max_rank[node] = high;
            node /= 2;
        }
    }

    fn insert(&mut self, id: usize, rank: usize, nodes: &[usize]) {
        for &node in nodes {
            self.sets[node].insert((rank, id));
            self.update(node);
        }
    }

    fn remove(&mut self, id: usize, rank: usize, nodes: &[usize]) {
        for &node in nodes {
            self.sets[node].remove(&(rank, id));
            self.update(node);
        }
    }

    /// Calls `found` for each open box whose extent meets leaves `[lo, hi)` and whose rank is as
    /// `ranks` wants (a box may be found once per canonical node of it). Returns the nodes and
    /// entries visited.
    fn query(&self, lo: usize, hi: usize, ranks: Ranks, mut found: impl FnMut(usize)) -> usize {
        let mut work = 0;
        if lo >= hi {
            return 0;
        }
        let mut stack = vec![(1usize, 0usize, self.size)];
        while let Some((node, a, b)) = stack.pop() {
            work += 1;
            let wanted = match ranks {
                Ranks::Below(rank) => self.min_rank[node] < rank,
                Ranks::Above(rank) => self.max_rank[node] > rank,
            };
            if b <= lo || a >= hi || !wanted {
                continue;
            }
            // Every box listed here covers this whole range, which meets the query's: those of
            // the wanted ranks are read, no others.
            match ranks {
                Ranks::Below(rank) => {
                    for &(_, id) in self.sets[node].range(..(rank, 0)) {
                        work += 1;
                        found(id);
                    }
                }
                Ranks::Above(rank) => {
                    for &(_, id) in self.sets[node].range((rank + 1, 0)..) {
                        work += 1;
                        found(id);
                    }
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
    let mut nodes: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); boxes.len()];
    let mut seen: Vec<usize> = vec![usize::MAX; n];
    let mut work = 0;
    for (stamp, &(_, opens, id)) in events.iter().enumerate() {
        let k = kind(id);
        if !opens {
            let canonical = core::mem::take(&mut nodes[id]);
            trees[k].remove(id, item(id).rank, &canonical);
            continue;
        }
        let it = *item(id);
        let (lo, hi) = trees[k].leaves(it.cross_start, it.cross_end);
        // The open boxes of the other kind it meets across the line and that paint on the other
        // side of it: a fragment's atomics before it, an atomic's fragments after it.
        let ranks = if k == 0 { Ranks::Above(it.rank) } else { Ranks::Below(it.rank) };
        let mut met: Vec<usize> = Vec::new();
        work += trees[1 - k].query(lo, hi, ranks, |other| {
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
        trees[k].insert(id, it.rank, &canonical);
        nodes[id] = canonical;
    }
    for list in &mut out {
        list.sort_by_key(|&atomic| atomics[atomic].rank);
    }
    (out, work)
}
