//! HTML "try to upgrade an element" when it becomes connected: the tree
//! records the elements with a custom element name that are not custom yet
//! as they are inserted into the document of a browsing context, and the
//! bindings upgrade those their window has a definition for when the DOM
//! call returns.

use crate::dom::Dom;
use crate::node::{element_flags, NodeId, RegistryAssociation, NIL};
use markup5ever::LocalName;
use rustc_hash::FxHashMap;

#[derive(Default)]
pub(crate) struct UpgradeQueue {
    /// Some window defined a custom element: until then no insertion walks a
    /// subtree looking for candidates.
    enabled: bool,
    /// Preserve reaction order across parser creations and connected insertions.
    queued: Vec<UpgradeCandidate>,
    // Includes reactions currently being dispatched, because a constructor
    // can synchronously adopt a later sibling or trigger garbage collection.
    pub(crate) pending_creations: FxHashMap<NodeId, RegistryAssociation>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum UpgradeCandidate {
    Connected(NodeId),
    Created { node: NodeId, document: NodeId, registry: RegistryAssociation, name: LocalName },
}

impl Dom {
    /// A custom element was defined: connected insertions queue candidates.
    pub fn enable_custom_upgrades(&mut self) {
        self.upgrades.enabled = true;
    }

    /// Whether `i` could be upgraded: an HTML element with a custom element
    /// name (a hyphen in it) or an is value, that is not custom yet.
    fn upgrade_candidate(&self, i: u32) -> bool {
        if !self.upgrades.pending_creations.is_empty() && self.has_pending_created_upgrade(self.id_of(i)) {
            return false;
        }
        self.n(i).element().is_some_and(|e| {
            e.flags & (element_flags::CUSTOM_DEFINED | element_flags::CUSTOM_FAILED) == 0
                && e.is_html()
                && (e.is_value.is_some() || e.name.local.contains('-'))
        })
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
                self.upgrades.queued.push(UpgradeCandidate::Connected(id));
                self.touch_hook();
            }
            i = self.next_shadow_including(i, root);
        }
    }

    /// Creating an element during fragment parsing enqueues an upgrade when
    /// its registry has a matching definition, even for a detached fragment.
    /// Capture the definition before any constructor can adopt a later sibling.
    /// Template contents are a distinct tree and are not traversed.
    pub(crate) fn upgrades_after_fragment_parse(&mut self, root: u32) {
        if !self.upgrades.enabled { return; }
        let mut i = root;
        while i != NIL {
            if self.upgrade_candidate(i) {
                if let Some(name) = self.fragment_definition_name(i) {
                    let node = self.id_of(i);
                    let document = self.id_of(self.n(i).owner_doc);
                    let registry = self.registry_idx(i);
                    self.add_refs(&[node, document]).expect("live fragment node and its document");
                    self.upgrades.pending_creations.insert(node, registry);
                    self.upgrades.queued.push(UpgradeCandidate::Created { node, document, registry, name });
                    self.touch_hook();
                }
            }
            i = self.next_shadow_including(i, root);
        }
    }

    /// The not-yet-custom HTML elements a definition named `local` may be for (their local name
    /// or their is value) among the shadow-including descendants of
    /// `root`, in shadow-including tree order (shadow trees before children): the elements
    /// `customElements.define` upgrades (HTML "upgrade candidates", which include the elements of
    /// shadow trees: a component's shadow tree holds the components it uses).
    pub fn upgrade_candidates_named(&self, root: NodeId, local: &markup5ever::LocalName) -> Vec<NodeId> {
        let Ok(root) = self.check(root) else { return Vec::new() };
        let mut out = Vec::new();
        let mut i = root;
        while i != NIL {
            if self.upgrade_candidate(i) && self.n(i).element().is_some_and(|e| e.may_be_defined_as(local)) {
                out.push(self.id_of(i));
            }
            i = self.next_shadow_including(i, root);
        }
        out
    }

    /// Whether elements wait for an upgrade attempt.
    pub fn has_upgrade_candidates(&self) -> bool {
        !self.upgrades.queued.is_empty()
    }

    /// The queued candidates, in order; cleared. One may have been removed,
    /// upgraded or collected since: check it before use.
    pub fn take_upgrade_candidates(&mut self) -> Vec<NodeId> {
        self.take_upgrade_reactions().into_iter().map(|item| match item {
            UpgradeCandidate::Connected(node) => node,
            UpgradeCandidate::Created { node, document, registry, .. } => {
                self.finish_created_upgrade(node, document, registry);
                node
            }
        }).collect()
    }

    pub fn has_pending_created_upgrade(&self, node: NodeId) -> bool {
        self.upgrades.pending_creations.contains_key(&node)
    }

    /// Balance the creation reaction's pins after invoking (or discarding)
    /// it, including when custom element construction reported an exception.
    pub fn finish_created_upgrade(&mut self, node: NodeId, document: NodeId, registry: RegistryAssociation) {
        let pending = self.upgrades.pending_creations.remove(&node);
        debug_assert_eq!(pending, Some(registry));
        self.release(node);
        self.release(document);
    }

    /// Created entries keep their node, original document and registry alive
    /// until the caller invokes `finish_created_upgrade`.
    pub fn take_upgrade_reactions(&mut self) -> Vec<UpgradeCandidate> {
        std::mem::take(&mut self.upgrades.queued)
    }

    /// Whether `el` is still a candidate: connected, and not custom.
    pub fn awaits_upgrade(&self, el: NodeId) -> bool {
        self.check(el).is_ok_and(|i| self.upgrade_candidate(i) && self.is_connected_idx(i))
    }
}
