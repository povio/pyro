//! Live range boundary points (DOM §5.5). A `Dom` keeps a weak list of the
//! ranges over its nodes and adjusts them as the tree and character data
//! change. With no Range around, each mutation path pays a single
//! `Vec::is_empty` check; dead entries are swept by the next adjustment.

use std::cell::Cell;
use std::rc::{Rc, Weak};

use crate::dom::Dom;
use crate::error::DomResult;
use crate::node::{NodeId, NIL};

/// A boundary point: node and offset.
pub type Point = (NodeId, u32);

/// The two boundary points of one Range. Each holds an external reference on
/// its node; the owner releases the final two when it drops the range.
pub struct LiveRange {
    shadow_including: Cell<bool>,
    start: Cell<Point>,
    end: Cell<Point>,
}

impl LiveRange {
    /// Composed selection endpoints follow a removed shadow host to its
    /// former position. Ordinary DOM ranges stay within their own tree.
    pub fn track_shadow_host_removal(&self) {
        self.shadow_including.set(true);
    }

    pub fn start(&self) -> Point {
        self.start.get()
    }
    pub fn end(&self) -> Point {
        self.end.get()
    }
}

impl Dom {
    /// A new range collapsed at `(node, 0)`, tracked until its last `Rc` drops.
    pub fn new_live_range(&mut self, node: NodeId) -> DomResult<Rc<LiveRange>> {
        self.add_ref(node)?;
        self.add_ref(node)?;
        let range = Rc::new(LiveRange { shadow_including: Cell::new(false), start: Cell::new((node, 0)), end: Cell::new((node, 0)) });
        self.live_ranges.push(Rc::downgrade(&range));
        Ok(range)
    }

    /// Sets both boundary points of `range`.
    pub fn set_live_range(&mut self, range: &LiveRange, start: Point, end: Point) -> DomResult<()> {
        self.set_cell(&range.start, start)?;
        self.set_cell(&range.end, end)
    }

    fn set_cell(&mut self, cell: &Cell<Point>, to: Point) -> DomResult<()> {
        let from = cell.get();
        if from.0 != to.0 {
            self.add_ref(to.0)?;
            self.release(from.0);
        }
        cell.set(to);
        Ok(())
    }

    /// Runs `f` over every boundary point of every live range; a `Some` moves
    /// the point. Sweeps ranges that have been dropped.
    fn adjust_ranges(&mut self, mut f: impl FnMut(&Dom, Point, bool) -> Option<Point>) {
        let mut list: Vec<Weak<LiveRange>> = std::mem::take(&mut self.live_ranges);
        list.retain(|weak| {
            let Some(range) = weak.upgrade() else { return false };
            for cell in [&range.start, &range.end] {
                if let Some(to) = f(self, cell.get(), range.shadow_including.get()) {
                    // Both nodes are alive: one is referenced, the other in the tree.
                    let _ = self.set_cell(cell, to);
                }
            }
            true
        });
        // Nothing above can register a range, so the taken slot is still empty.
        self.live_ranges = list;
    }

    fn index_idx(&self, idx: u32) -> u32 {
        let mut k = 0;
        let mut c = self.n(idx).prev;
        while c != NIL {
            k += 1;
            c = self.n(c).prev;
        }
        k
    }

    /// Insert steps: `count` nodes are about to go into `parent` before `child`.
    #[cold]
    pub(crate) fn ranges_before_insert(&mut self, parent: u32, child: u32, count: u32) {
        let index = self.index_idx(child);
        let parent = self.id_of(parent);
        self.adjust_ranges(|_, (n, o), _| (n == parent && o > index).then_some((n, o + count)));
    }

    /// Remove steps: `node`, still linked, is about to leave its parent.
    #[cold]
    pub(crate) fn ranges_before_remove(&mut self, node: u32) {
        let parent = self.n(node).parent;
        let index = self.index_idx(node);
        let parent = self.id_of(parent);
        self.adjust_ranges(|dom, (n, o), shadow_including| {
            let removed = if shadow_including {
                dom.is_host_including_inclusive_ancestor(node, n.idx)
            } else {
                dom.is_inclusive_ancestor(node, n.idx)
            };
            if removed {
                return Some((parent, index));
            }
            (n == parent && o > index).then(|| (n, o - 1))
        });
    }

    /// "Replace data" steps: `count` code units at `offset` became `added`.
    #[cold]
    pub(crate) fn ranges_after_replace_data(&mut self, node: u32, offset: u32, count: u32, added: u32) {
        let node = self.id_of(node);
        self.adjust_ranges(|_, (n, o), _| {
            if n != node || o <= offset {
                None
            } else if o <= offset + count {
                Some((n, offset))
            } else {
                Some((n, o + added - count))
            }
        });
    }

    /// `splitText` steps: `new`, already inserted after `node`, takes the data
    /// from `offset` on.
    #[cold]
    pub(crate) fn ranges_after_split(&mut self, node: u32, new: u32, offset: u32) {
        let parent = self.id_of(self.n(node).parent);
        let index = self.index_idx(node);
        let (node, new) = (self.id_of(node), self.id_of(new));
        self.adjust_ranges(|_, (n, o), _| {
            if n == node && o > offset {
                Some((new, o - offset))
            } else if n == parent && o == index + 1 {
                Some((n, o + 1))
            } else {
                None
            }
        });
    }

    /// `normalize` steps: the data of `sibling` now follows `base_len` code
    /// units of `base`, and `sibling` is about to be removed.
    #[cold]
    pub(crate) fn ranges_merge_text(&mut self, base: u32, sibling: u32, base_len: u32) {
        let parent = self.id_of(self.n(sibling).parent);
        let index = self.index_idx(sibling);
        let (base, sibling) = (self.id_of(base), self.id_of(sibling));
        self.adjust_ranges(|_, (n, o), _| {
            if n == sibling {
                Some((base, o + base_len))
            } else if n == parent && o == index {
                Some((base, base_len))
            } else {
                None
            }
        });
    }
}
