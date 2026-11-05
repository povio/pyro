//! Custom element callback reactions: the tree records which custom
//! elements were connected, disconnected or moved (`moveBefore`), per node
//! document, and the bindings run the callbacks when the DOM call returns.

use rustc_hash::FxHashMap;

use crate::dom::Dom;
use crate::node::{element_flags, NodeId, NIL};

/// A queued callback reaction of a custom element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CustomReaction {
    /// `connectedCallback`.
    Connected,
    /// `disconnectedCallback`.
    Disconnected,
    /// `connectedMoveCallback`, which falls back to `disconnectedCallback`
    /// then `connectedCallback` when the definition has none.
    Moved,
}

#[derive(Default)]
pub(crate) struct ReactionQueue {
    /// Some element was ever marked custom: until then no mutation walks a
    /// subtree looking for one.
    pub(crate) any_custom: bool,
    /// Reactions in enqueue order, per node document at enqueue time.
    queued: FxHashMap<NodeId, Vec<(NodeId, CustomReaction)>>,
}

impl Dom {
    /// `root` and its shadow-including descendants, in shadow-including tree
    /// order (an element's shadow tree comes before its children).
    pub(crate) fn shadow_including_subtree(&self, root: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(i) = stack.pop() {
            out.push(i);
            let mut child = self.n(i).last_child;
            while child != NIL {
                stack.push(child);
                child = self.n(child).prev;
            }
            if let Some(e) = self.n(i).element() {
                if e.shadow_root != NIL {
                    stack.push(e.shadow_root);
                }
            }
        }
        out
    }

    /// Enqueues `reaction` for every custom element among `root` and its
    /// shadow-including descendants, in shadow-including tree order.
    pub(crate) fn enqueue_custom_reactions(&mut self, root: u32, reaction: CustomReaction) {
        if !self.reactions.any_custom {
            return;
        }
        for i in self.shadow_including_subtree(root) {
            let custom = self.n(i).element().is_some_and(|e| e.flags & element_flags::CUSTOM_DEFINED != 0);
            if custom {
                let doc = self.id_of(self.n(i).owner_doc);
                let node = self.id_of(i);
                self.reactions.queued.entry(doc).or_default().push((node, reaction));
                self.touch_hook();
            }
        }
    }

    /// Whether any document has queued custom element reactions.
    pub fn has_custom_reactions(&self) -> bool {
        !self.reactions.queued.is_empty()
    }

    /// Whether `document` has queued custom element reactions.
    pub fn has_custom_reactions_for(&self, document: NodeId) -> bool {
        self.reactions.queued.contains_key(&document)
    }

    /// The reactions queued for elements of `document`, in order; cleared.
    /// An element may have been collected since: check it before use.
    pub fn take_custom_reactions(&mut self, document: NodeId) -> Vec<(NodeId, CustomReaction)> {
        self.reactions.queued.remove(&document).unwrap_or_default()
    }
}
