//! HTML "try to upgrade an element" when it becomes connected: the tree
//! records the elements with a custom element name that are not custom yet
//! as they are inserted into the document of a browsing context, and the
//! bindings upgrade those their window has a definition for when the DOM
//! call returns.

use crate::dom::Dom;
use crate::node::{element_flags, NodeId, NIL};

#[derive(Default)]
pub(crate) struct UpgradeQueue {
    /// Some window defined a custom element: until then no insertion walks a
    /// subtree looking for candidates.
    enabled: bool,
    /// Candidates in the order they became connected (shadow-including tree
    /// order within one insertion).
    queued: Vec<NodeId>,
}

impl Dom {
    /// A custom element was defined: connected insertions queue candidates.
    pub fn enable_custom_upgrades(&mut self) {
        self.upgrades.enabled = true;
    }

    /// Whether `i` could be upgraded: an HTML element with a custom element
    /// name (a hyphen in it) that is not custom yet.
    fn upgrade_candidate(&self, i: u32) -> bool {
        self.n(i)
            .element()
            .is_some_and(|e| e.flags & element_flags::CUSTOM_DEFINED == 0 && e.is_html() && e.name.local.contains('-'))
    }

    /// `root` was inserted and is connected: queues the candidates among it
    /// and its shadow-including descendants.
    pub(crate) fn upgrades_after_insert(&mut self, root: u32) {
        if !self.upgrades.enabled {
            return;
        }
        let doc = self.n(root).owner_doc;
        if !matches!(&self.n(doc).kind, crate::node::NodeKind::Document(d) if d.active) {
            return;
        }
        // Shadow trees before children, in tree order.
        let mut i = root;
        while i != NIL {
            if self.upgrade_candidate(i) {
                let id = self.id_of(i);
                self.upgrades.queued.push(id);
                self.touch_hook();
            }
            i = self.next_shadow_including(i, root);
        }
    }

    /// Whether elements wait for an upgrade attempt.
    pub fn has_upgrade_candidates(&self) -> bool {
        !self.upgrades.queued.is_empty()
    }

    /// The queued candidates, in order; cleared. One may have been removed,
    /// upgraded or collected since: check it before use.
    pub fn take_upgrade_candidates(&mut self) -> Vec<NodeId> {
        std::mem::take(&mut self.upgrades.queued)
    }

    /// Whether `el` is still a candidate: connected, and not custom.
    pub fn awaits_upgrade(&self, el: NodeId) -> bool {
        self.check(el).is_ok_and(|i| self.upgrade_candidate(i) && self.is_connected_idx(i))
    }
}
