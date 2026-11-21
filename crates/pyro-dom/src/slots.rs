//! Slot assignment changes ("signal a slot change"): after a mutation that
//! can reassign slottables, the slots whose assigned nodes differ from the
//! last observed list are queued for a `slotchange` event, which the
//! bindings fire in the mutation observer microtask.

use rustc_hash::FxHashMap;

use crate::dom::Dom;
use crate::error::DomResult;
use crate::node::{NodeId, NodeKind, NIL};

#[derive(Default)]
pub(crate) struct SlotState {
    /// Assigned nodes last observed per slot (by index).
    assignments: FxHashMap<u32, Vec<u32>>,
    /// Slots to fire `slotchange` at, in signal order.
    signaled: Vec<u32>,
    /// DOM "manually assigned nodes" of each slot `HTMLSlotElement.assign()` was called on
    /// (a node is in one slot's list at most), in the order given. They are its assigned nodes
    /// while the slot is in a shadow tree that assigns manually and they are children of its host.
    pub(crate) manual: FxHashMap<NodeId, Vec<NodeId>>,
}

impl Dom {
    /// Recompute the assigned list, queuing slotchange only when it differs.
    fn check_slot(&mut self, slot: u32) -> bool {
        let now = self.slot_assigned(slot);
        if now.is_empty() {
            if self.slot_state.assignments.remove(&slot).is_none() {
                return false;
            }
        } else {
            if self.slot_state.assignments.get(&slot) == Some(&now) {
                return false;
            }
            self.slot_state.assignments.insert(slot, now);
        }
        if !self.slot_state.signaled.contains(&slot) {
            self.slot_state.signaled.push(slot);
            self.touch_hook();
        }
        true
    }

    /// Checks every slot of the shadow tree rooted at `root`.
    fn check_slots_in_root(&mut self, root: u32) -> bool {
        let mut slots = Vec::new();
        let mut cur = self.n(root).first_child;
        while cur != NIL {
            if self.is_slot(cur) {
                slots.push(cur);
            }
            cur = self.following_in(cur, root);
        }
        let mut changed = false;
        for slot in slots {
            changed |= self.check_slot(slot);
        }
        changed
    }

    /// The shadow root `host` hosts (by name or manually assigning: a child of the host that
    /// comes or goes changes either's assignment).
    fn named_shadow_root(&self, host: u32) -> Option<u32> {
        let sr = self.n(host).element()?.shadow_root;
        if sr == NIL {
            return None;
        }
        match &self.n(sr).kind {
            NodeKind::Fragment(f) if f.shadow.is_some() => Some(sr),
            _ => None,
        }
    }

    /// `HTMLSlotElement.assign(...nodes)` (DOM "manually assigned nodes"): `slot` takes `nodes`
    /// (each once, in order) from the slots they were assigned to, its old nodes are unassigned,
    /// and the slots of the trees concerned are checked for `slotchange`.
    pub fn slot_assign(&mut self, slot: NodeId, nodes: &[NodeId]) -> DomResult<()> {
        self.check(slot)?;
        let mut touched = vec![slot];
        self.slot_state.manual.remove(&slot);
        let mut set: Vec<NodeId> = Vec::with_capacity(nodes.len());
        for &node in nodes {
            if set.contains(&node) {
                continue;
            }
            for (other, list) in self.slot_state.manual.iter_mut() {
                if list.contains(&node) {
                    list.retain(|n| *n != node);
                    touched.push(*other);
                }
            }
            set.push(node);
        }
        self.slot_state.manual.insert(slot, set);
        self.slot_state.manual.retain(|_, list| !list.is_empty());
        // DOM "assign slottables for a tree" for the roots of the slots concerned.
        let mut roots: Vec<u32> = Vec::new();
        for id in touched {
            let Ok(i) = self.check(id) else { continue };
            if self.in_shadow_tree(i) {
                let root = self.root_idx(i);
                if !roots.contains(&root) {
                    roots.push(root);
                }
            }
        }
        let mut changed = false;
        for root in roots {
            changed |= self.check_slots_in_root(root);
        }
        if changed {
            // The flat tree changed without a light-tree mutation. Cached
            // layout and paint snapshots must pick up the new slot contents.
            self.bump();
        }
        Ok(())
    }

    /// Whether `node`'s subtree holds a slot element.
    fn subtree_has_slot(&self, node: u32) -> bool {
        if self.is_slot(node) {
            return true;
        }
        let mut cur = self.n(node).first_child;
        while cur != NIL {
            if self.is_slot(cur) {
                return true;
            }
            cur = self.following_in(cur, node);
        }
        false
    }

    /// The children of `parent` changed (`nodes` were inserted or removed,
    /// already linked or unlinked).
    pub(crate) fn slots_after_child_change(&mut self, parent: u32, nodes: &[u32]) {
        // Every step below concerns a shadow tree or a shadow host: none
        // without a shadow root.
        if parent == NIL || self.shadow_root_count() == 0 {
            return;
        }
        self.signal_fallback_change(parent);
        if let Some(sr) = self.named_shadow_root(parent) {
            self.check_slots_in_root(sr);
        }
        if self.in_shadow_tree(parent) && nodes.iter().any(|&n| self.subtree_has_slot(n)) {
            let root = self.root_idx(parent);
            self.check_slots_in_root(root);
            // Removal assigns slottables in the former parent's tree first,
            // then in the removed subtree. Detached slots lose their list.
            for &n in nodes {
                if self.n(n).parent == NIL {
                    for slot in self.slots_in_subtree(n) {
                        self.check_slot(slot);
                    }
                }
            }
        }
    }

    /// DOM insert, remove and move: "if parent's root is a shadow root, and
    /// parent is a slot whose assigned nodes is the empty list, then run
    /// signal a slot change for parent" (its fallback content changed).
    fn signal_fallback_change(&mut self, parent: u32) {
        if self.is_slot(parent)
            && self.in_shadow_tree(parent)
            && self.slot_assigned(parent).is_empty()
            && !self.slot_state.signaled.contains(&parent)
        {
            self.slot_state.signaled.push(parent);
            self.touch_hook();
        }
    }

    /// The slots of `node`'s subtree, `node` included, in tree order.
    fn slots_in_subtree(&self, node: u32) -> Vec<u32> {
        let mut slots = Vec::new();
        if self.is_slot(node) {
            slots.push(node);
        }
        let mut cur = self.n(node).first_child;
        while cur != NIL {
            if self.is_slot(cur) {
                slots.push(cur);
            }
            cur = self.following_in(cur, node);
        }
        slots
    }

    /// DOM "move": `node` went from `old_parent` to `new_parent` (already
    /// linked there). Unlike a removal, a moved slot keeps the assignment it
    /// was last seen with, so one that loses or gains its assigned nodes by
    /// leaving or entering a shadow tree is signaled.
    pub(crate) fn slots_after_move(&mut self, old_parent: u32, new_parent: u32, node: u32) {
        self.signal_fallback_change(old_parent);
        if let Some(sr) = self.named_shadow_root(old_parent) {
            self.check_slots_in_root(sr);
        }
        let moved_slots = self.slots_in_subtree(node);
        if !moved_slots.is_empty() && self.in_shadow_tree(old_parent) {
            let root = self.root_idx(old_parent);
            self.check_slots_in_root(root);
        }
        if let Some(sr) = self.named_shadow_root(new_parent) {
            self.check_slots_in_root(sr);
        }
        self.signal_fallback_change(new_parent);
        if !moved_slots.is_empty() {
            if self.in_shadow_tree(new_parent) {
                let root = self.root_idx(new_parent);
                self.check_slots_in_root(root);
            } else {
                for slot in moved_slots {
                    self.check_slot(slot);
                }
            }
        }
    }

    /// Attribute `local` of element `el` changed.
    pub(crate) fn slots_after_attr_change(&mut self, el: u32, local: &str) {
        match local {
            "slot" => {
                let parent = self.n(el).parent;
                if parent != NIL {
                    if let Some(sr) = self.named_shadow_root(parent) {
                        self.check_slots_in_root(sr);
                    }
                }
            }
            "name" if self.is_slot(el) && self.in_shadow_tree(el) => {
                let root = self.root_idx(el);
                self.check_slots_in_root(root);
            }
            _ => {}
        }
    }

    pub fn has_signaled_slots(&self) -> bool {
        !self.slot_state.signaled.is_empty()
    }

    /// The slots to fire `slotchange` at, in order; cleared.
    pub fn take_signaled_slots(&mut self) -> Vec<NodeId> {
        let slots = std::mem::take(&mut self.slot_state.signaled);
        slots.into_iter().filter(|&s| self.n(s).is_element()).map(|s| self.id_of(s)).collect()
    }
}
