//! Live NodeIterators (DOM "NodeIterator pre-removing steps"): a `Dom` keeps
//! a weak list of the iterators over its nodes and moves their reference
//! node when it is about to leave the tree.

use std::cell::Cell;
use std::rc::{Rc, Weak};

use crate::dom::Dom;
use crate::error::DomResult;
use crate::node::{NodeId, NIL};

/// A NodeIterator's reference node and pointer-before-reference flag. The
/// reference holds an external reference on its node; the owner releases the
/// final one when it drops the iterator.
pub struct LiveIterator {
    root: NodeId,
    reference: Cell<NodeId>,
    before: Cell<bool>,
}

impl LiveIterator {
    pub fn reference(&self) -> NodeId {
        self.reference.get()
    }
    pub fn before(&self) -> bool {
        self.before.get()
    }
}

impl Dom {
    /// A new iterator at `root` (pointer before it), tracked until its last
    /// `Rc` drops.
    pub fn new_live_iterator(&mut self, root: NodeId) -> DomResult<Rc<LiveIterator>> {
        self.add_ref(root)?;
        let it = Rc::new(LiveIterator { root, reference: Cell::new(root), before: Cell::new(true) });
        self.live_iterators.push(Rc::downgrade(&it));
        Ok(it)
    }

    /// Moves the iterator's reference.
    pub fn set_iterator_reference(&mut self, it: &LiveIterator, node: NodeId, before: bool) -> DomResult<()> {
        let from = it.reference.get();
        if from != node {
            self.add_ref(node)?;
            self.release(from);
        }
        it.reference.set(node);
        it.before.set(before);
        Ok(())
    }

    /// Remove steps: `node`, still linked, is about to leave its parent.
    #[cold]
    pub(crate) fn iterators_before_remove(&mut self, node: u32) {
        let mut list: Vec<Weak<LiveIterator>> = std::mem::take(&mut self.live_iterators);
        list.retain(|weak| {
            let Some(it) = weak.upgrade() else { return false };
            self.iterator_before_remove(&it, node);
            true
        });
        self.live_iterators = list;
    }

    fn iterator_before_remove(&mut self, it: &LiveIterator, removed: u32) {
        let Ok(reference) = self.check(it.reference.get()) else { return };
        let Ok(root) = self.check(it.root) else { return };
        if removed == root || !self.is_inclusive_ancestor(removed, reference) {
            return;
        }
        if it.before.get() {
            // The first following node of `removed` inside the root that is
            // not inside `removed`: its next sibling, or an ancestor's.
            let mut cur = removed;
            let mut next = NIL;
            while cur != root && cur != NIL {
                let n = self.n(cur);
                if n.next != NIL {
                    next = n.next;
                    break;
                }
                cur = n.parent;
            }
            if next != NIL {
                let id = self.id_of(next);
                let _ = self.set_iterator_reference(it, id, true);
                return;
            }
            it.before.set(false);
        }
        let prev = self.n(removed).prev;
        let target = if prev == NIL {
            self.n(removed).parent
        } else {
            let mut last = prev;
            while self.n(last).last_child != NIL {
                last = self.n(last).last_child;
            }
            last
        };
        let id = self.id_of(target);
        let _ = self.set_iterator_reference(it, id, false);
    }
}
