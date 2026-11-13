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
    /// The position a traversal in flight has reached (the node being
    /// filtered and its pointer-before flag): a filter that removes nodes
    /// moves it like the reference. It holds an external reference too.
    candidate: Cell<Option<(NodeId, bool)>>,
}

impl LiveIterator {
    pub fn reference(&self) -> NodeId {
        self.reference.get()
    }
    pub fn before(&self) -> bool {
        self.before.get()
    }
    /// The in-flight traversal position, if a traversal is filtering a node.
    pub fn candidate(&self) -> Option<(NodeId, bool)> {
        self.candidate.get()
    }
}

impl Dom {
    /// A new iterator at `root` (pointer before it), tracked until its last
    /// `Rc` drops.
    pub fn new_live_iterator(&mut self, root: NodeId) -> DomResult<Rc<LiveIterator>> {
        self.add_ref(root)?;
        let it =
            Rc::new(LiveIterator { root, reference: Cell::new(root), before: Cell::new(true), candidate: Cell::new(None) });
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

    /// Sets (or clears) the position of the traversal in flight.
    pub fn set_iterator_candidate(&mut self, it: &LiveIterator, position: Option<(NodeId, bool)>) -> DomResult<()> {
        if let Some((node, _)) = position {
            self.add_ref(node)?;
        }
        if let Some((old, _)) = it.candidate.replace(position) {
            self.release(old);
        }
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

    /// "NodeIterator pre-remove steps" for the reference, and for the
    /// position of a traversal in flight.
    fn iterator_before_remove(&mut self, it: &LiveIterator, removed: u32) {
        let Ok(root) = self.check(it.root) else { return };
        if let Some((node, before)) = self.position_before_remove(root, it.reference.get(), it.before.get(), removed) {
            let _ = self.set_iterator_reference(it, node, before);
        }
        if let Some((node, before)) = it.candidate.get() {
            if let Some(moved) = self.position_before_remove(root, node, before, removed) {
                let _ = self.set_iterator_candidate(it, Some(moved));
            }
        }
    }

    /// Where the position (`reference`, `before`) of an iterator over `root`
    /// moves when `removed` leaves the tree; None: it stays.
    fn position_before_remove(
        &self,
        root: u32,
        reference: NodeId,
        mut before: bool,
        removed: u32,
    ) -> Option<(NodeId, bool)> {
        let reference = self.check(reference).ok()?;
        // "If toBeRemovedNode is not an inclusive ancestor of the reference,
        // or is an inclusive ancestor of the root, then return."
        if !self.is_inclusive_ancestor(removed, reference) || self.is_inclusive_ancestor(removed, root) {
            return None;
        }
        if before {
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
                return Some((self.id_of(next), true));
            }
            before = false;
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
        Some((self.id_of(target), before))
    }
}
