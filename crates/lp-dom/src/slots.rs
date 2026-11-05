//! Slot assignment changes ("signal a slot change"): after a mutation that
//! can reassign slottables, the slots whose assigned nodes differ from the
//! last observed list are queued for a `slotchange` event, which the
//! bindings fire in the mutation observer microtask.

use rustc_hash::FxHashMap;

use crate::dom::Dom;
use crate::node::{NodeId, NodeKind, NIL};

#[derive(Default)]
pub(crate) struct SlotState {
    /// Assigned nodes last observed per slot (by index).
    assignments: FxHashMap<u32, Vec<u32>>,
    /// Slots to fire `slotchange` at, in signal order.
    signaled: Vec<u32>,
}

impl Dom {
    /// Re-reads slot `slot`'s assignment; a change queues the signal.
    fn check_slot(&mut self, slot: u32) {
        let now = self.slot_assigned(slot);
        let changed = self.slot_state.assignments.get(&slot) != Some(&now);
        if changed {
            if now.is_empty() && !self.slot_state.assignments.contains_key(&slot) {
                // A slot never seen with assigned nodes: nothing changed.
                self.slot_state.assignments.insert(slot, now);
                return;
            }
            self.slot_state.assignments.insert(slot, now);
            if !self.slot_state.signaled.contains(&slot) {
                self.slot_state.signaled.push(slot);
            }
        }
    }

    /// Checks every slot of the shadow tree rooted at `root`.
    fn check_slots_in_root(&mut self, root: u32) {
        let mut slots = Vec::new();
        let mut cur = self.n(root).first_child;
        while cur != NIL {
            if self.is_slot(cur) {
                slots.push(cur);
            }
            cur = self.following_in(cur, root);
        }
        for slot in slots {
            self.check_slot(slot);
        }
    }

    /// The shadow root `host` hosts, when it has one that assigns by name.
    fn named_shadow_root(&self, host: u32) -> Option<u32> {
        let sr = self.n(host).element()?.shadow_root;
        if sr == NIL {
            return None;
        }
        match &self.n(sr).kind {
            NodeKind::Fragment(f) if f.shadow.is_some() && f.flags & crate::node::shadow_flags::MANUAL_SLOTS == 0 => {
                Some(sr)
            }
            _ => None,
        }
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
        if parent == NIL {
            return;
        }
        if let Some(sr) = self.named_shadow_root(parent) {
            self.check_slots_in_root(sr);
        }
        if self.in_shadow_tree(parent) && nodes.iter().any(|&n| self.subtree_has_slot(n)) {
            let root = self.root_idx(parent);
            for &n in nodes {
                if self.n(n).parent == NIL {
                    self.slot_state.assignments.remove(&n);
                }
            }
            self.check_slots_in_root(root);
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
