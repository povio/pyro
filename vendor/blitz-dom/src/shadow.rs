//! pyro: shadow trees (CSS Scoping 1) in the flat tree pyro-dom's render tree gives Blitz.
//!
//! The render tree is the flat tree: a shadow host's children are its shadow tree's nodes (the
//! host carries the [`SHADOW_HOST_ATTR`] marker), and a slot of a shadow tree with nodes
//! assigned to it has those, light children of its host, as its children (it carries the
//! [`ASSIGNED_SLOT_ATTR`] marker); a slot without has its fallback content. Layout and
//! inheritance follow that tree as before. Selector matching and the cascade need the node trees
//! the elements belong to, which follow from it:
//!
//! - the tree of an element whose parent is a host is that host's shadow tree; of one whose
//!   parent is an assigned slot, the tree of the slot's host; else its parent's
//!   ([`Node::containing_shadow_host_id`]);
//! - its parent in that tree ([`Node::dom_parent_element`], selector combinators) is none for the
//!   first level of a shadow tree (its parent is the shadow root: `:host` reaches the host), the
//!   host for an assigned node, else its parent. Its siblings are its parent's other children:
//!   an assigned node's are the nodes assigned to the same slot (not the host's other light
//!   children, which the render tree leaves out when unassigned);
//! - each host keeps the sheets of its shadow tree's `<style>` and `<link>` elements (adopted
//!   sheets come as `<style>` elements) in a Stylo [`AuthorStyles`], whose cascade data Stylo's
//!   rule collector applies to the shadow tree, `:host`, `::slotted()` and `::part()`
//!   ([`ShadowStyles`]), instead of the document's stylist.

use std::collections::BTreeMap;

use blitz_traits::node_id::NodeId;
use style::author_styles::AuthorStyles;
use style::invalidation::element::restyle_hints::RestyleHint;
use style::stylesheets::DocumentStyleSheet;

use crate::BaseDocument;
use crate::node::{Node, NodeFlags};

/// Name (in pyro-dom's state namespace, [`crate::PYRO_STATE_NS`]) of the marker attribute of a
/// shadow host.
pub const SHADOW_HOST_ATTR: &str = "shadow-host";
/// Name (in [`crate::PYRO_STATE_NS`]) of the marker attribute of a slot whose children are the
/// nodes assigned to it.
pub const ASSIGNED_SLOT_ATTR: &str = "assigned-slot";
/// pyro: the attribute (in [`crate::PYRO_STATE_NS`]) a light child element of a shadow host
/// carries: its index among the host's element children (pyro-dom `LIGHT_INDEX_ATTR`).
pub const LIGHT_INDEX_ATTR: &str = "light-index";

/// The styles of a shadow root: the sheets of its tree's `<style>` and `<link>` elements, in
/// node order, and Stylo's cascade data of them.
pub(crate) struct ShadowStyles {
    pub(crate) styles: AuthorStyles<DocumentStyleSheet>,
    pub(crate) sheets: BTreeMap<NodeId, DocumentStyleSheet>,
}

impl ShadowStyles {
    fn new() -> Self {
        ShadowStyles { styles: AuthorStyles::new(), sheets: BTreeMap::new() }
    }
}

impl Node {
    /// Whether the element is a shadow host (its children are its shadow tree's).
    #[inline]
    pub fn is_shadow_host(&self) -> bool {
        self.flags.contains(NodeFlags::IS_SHADOW_HOST)
    }

    /// pyro: whether the element is the slot a `<details>` element's content renders in
    /// (pyro-dom's render tree marks it: `DETAILS_CONTENT_ATTR`): its `::details-content`.
    pub fn is_details_content_slot(&self) -> bool {
        self.is_assigned_slot()
            && self.element_data().is_some_and(|data| {
                data.attrs.iter().any(|attr| &*attr.name.ns == crate::PYRO_STATE_NS && &*attr.name.local == "details-content")
            })
    }

    /// Whether the element is a slot whose children are the nodes assigned to it.
    #[inline]
    pub fn is_assigned_slot(&self) -> bool {
        self.flags.contains(NodeFlags::IS_ASSIGNED_SLOT)
    }

    /// The host of the shadow tree the node is in; `None` in the document's tree.
    pub fn containing_shadow_host_id(&self) -> Option<NodeId> {
        if !self.tree().has_shadow_hosts {
            return None;
        }
        let mut child = self;
        loop {
            let parent = self.with(child.parent?);
            if parent.is_shadow_host() {
                return Some(parent.id);
            }
            if parent.is_assigned_slot() {
                // An assigned node is in the tree of the host whose shadow tree has the slot.
                let host = parent.containing_shadow_host_id()?;
                return crate::util::ensure_stack(|| self.with(host).containing_shadow_host_id());
            }
            child = parent;
        }
    }

    /// The element's parent in its node tree, for selector matching: `None` at the first level of
    /// a shadow tree (its parent is the shadow root) and at the top of the document; the host
    /// for a node assigned to a slot.
    pub fn dom_parent_element(&self) -> Option<&Node> {
        let parent = self.with(self.parent?);
        if self.tree().has_shadow_hosts {
            if parent.is_shadow_host() {
                return None;
            }
            if parent.is_assigned_slot() {
                return parent.containing_shadow_host_id().map(|host| self.with(host));
            }
        }
        parent.is_element().then_some(parent)
    }

    /// Whether the node's parent in its node tree is a shadow root.
    #[inline]
    pub fn parent_is_shadow_root(&self) -> bool {
        self.tree().has_shadow_hosts && self.parent.is_some_and(|p| self.with(p).is_shadow_host())
    }

    /// The slot the node is assigned to.
    #[inline]
    pub fn assigned_slot_id(&self) -> Option<NodeId> {
        if !self.tree().has_shadow_hosts {
            return None;
        }
        self.parent.filter(|&p| self.with(p).is_assigned_slot())
    }

    /// pyro: whether `found` holds for one of this slot's flattened slottables (DOM "find
    /// flattened slottables"): its assigned nodes, or else its fallback children (the flat tree's
    /// children of a slot are one or the other), an element or text node each, a slot among them
    /// standing for its own flattened slottables.
    pub fn any_flattened_slottable(&self, found: &mut impl FnMut(&Node) -> bool) -> bool {
        self.children.iter().any(|&id| {
            let child = self.with(id);
            match child.element_data() {
                Some(data) if data.name.ns == markup5ever::ns!(html) && &*data.name.local == "slot" => {
                    crate::util::ensure_stack(|| child.any_flattened_slottable(found))
                }
                Some(_) => found(child),
                None => child.is_text_node() && found(child),
            }
        })
    }

    /// pyro: an assigned node's light-tree sibling element `offset` places away (-1 the previous,
    /// 1 the next): found by its index among its host's element children (`LIGHT_INDEX_ATTR`)
    /// among the nodes assigned to the host's slots. `None` for a node that is not assigned (its
    /// flat-tree siblings are its tree's), `Some(None)` when there is no such sibling assigned.
    pub fn light_sibling_element(&self, offset: i64) -> Option<Option<&Node>> {
        self.assigned_slot_id()?;
        let index_of = |node: &Node| -> Option<i64> {
            node.element_data()?
                .attrs
                .iter()
                .find(|attr| &*attr.name.ns == crate::PYRO_STATE_NS && &*attr.name.local == LIGHT_INDEX_ATTR)
                .and_then(|attr| attr.value.parse().ok())
        };
        let index = index_of(self)?;
        // The flat-tree neighbour in the same slot, when it is the light-tree one (assigned nodes
        // keep their tree order in a slot): no walk.
        let mut step = 1;
        loop {
            let neighbour = if offset < 0 { self.backward(step) } else { self.forward(step) };
            let Some(neighbour) = neighbour else { break };
            if neighbour.is_element() {
                if index_of(neighbour) == Some(index + offset.signum()) {
                    return Some(Some(neighbour));
                }
                break;
            }
            step += 1;
        }
        let host = self.dom_parent_element()?;
        // The host's assigned nodes, by their index (the host's flat subtree, its slots').
        let mut found: Option<(i64, &Node)> = None;
        let mut stack: Vec<NodeId> = host.children.iter().copied().collect();
        while let Some(id) = stack.pop() {
            let node = self.with(id);
            if node.assigned_slot_id().is_some() && node.dom_parent_element().is_some_and(|p| p.id == host.id) {
                if let Some(other) = index_of(node) {
                    let wanted = if offset < 0 { other < index } else { other > index };
                    let closer = found.is_none_or(|(best, _)| if offset < 0 { other > best } else { other < best });
                    if wanted && closer {
                        found = Some((other, node));
                    }
                }
                continue;
            }
            stack.extend(node.children.iter().copied());
        }
        Some(found.map(|(_, node)| node))
    }

    /// The cascade data of the shadow root of this host.
    pub(crate) fn shadow_cascade_data(&self) -> Option<&style::stylist::CascadeData> {
        if !self.is_shadow_host() {
            return None;
        }
        self.tree().shadow_roots.get(&self.id).map(|root| &*root.styles.data)
    }
}

/// The `inner:outer` mappings of an `exportparts` value (`name` alone maps a name to itself);
/// invalid entries are left out.
pub(crate) fn export_parts(value: &str) -> impl Iterator<Item = (&str, &str)> {
    value.split(',').filter_map(|entry| {
        let mut sides = entry.split(':');
        let inner = sides.next()?.trim_matches(|c: char| c.is_ascii_whitespace());
        let outer = match sides.next() {
            Some(outer) => outer.trim_matches(|c: char| c.is_ascii_whitespace()),
            None => inner,
        };
        let valid = |s: &str| !s.is_empty() && !s.contains(|c: char| c.is_ascii_whitespace());
        (sides.next().is_none() && valid(inner) && valid(outer)).then_some((inner, outer))
    })
}

/// Whether `name` is one of the shadow markers of the render tree.
pub(crate) fn is_shadow_marker(name: &markup5ever::QualName) -> bool {
    &*name.ns == crate::PYRO_STATE_NS && matches!(&*name.local, SHADOW_HOST_ATTR | ASSIGNED_SLOT_ATTR)
}

impl BaseDocument {
    /// `:host:has()`: Stylo's relative selector invalidation walks a change's ancestors up to
    /// the shadow root, not on to the host, the anchor of a `:has()` on the featureless host. A
    /// change in a shadow tree whose sheets have `:has()` rules restyles the host and its subtree.
    pub(crate) fn invalidate_host_relative_selectors(&self, node: &Node) {
        let Some(host) = node.containing_shadow_host_id() else { return };
        let Some(host) = self.nodes.get(host) else { return };
        if !host.shadow_cascade_data().is_some_and(|data| data.relative_invalidation_map_attributes().used) {
            return;
        }
        if let Some(mut data) = style::dom::TElement::mutate_data(&host) {
            data.hint.insert(RestyleHint::restyle_subtree());
        }
        host.set_dirty_descendants();
        host.mark_ancestors_dirty();
    }

    /// pyro: `:has-slotted(<selector>)` matches a slot by its assigned elements (`stylo.rs`), whose
    /// attributes and states Stylo's invalidation does not relate to the slot: a change of one of
    /// an element assigned to a slot restyles that slot, and the slots it is assigned to in turn
    /// (`:has-slotted` takes flattened slottables). Assignment changes restyle the slot as a
    /// change of its children (`restyle_for_child_list_change`).
    pub(crate) fn invalidate_has_slotted(&self, node_id: NodeId) {
        if !self.nodes.has_shadow_hosts {
            return;
        }
        let Some(mut current) = self.nodes.get(node_id).filter(|node| node.is_element()) else { return };
        while let Some(slot) = current.assigned_slot_id().and_then(|id| self.nodes.get(id)) {
            if let Some(mut data) = style::dom::TElement::mutate_data(&slot) {
                data.hint.insert(RestyleHint::RESTYLE_SELF);
            }
            slot.mark_ancestors_dirty();
            current = slot;
        }
    }

    /// pyro: Stylo invalidates a change of an element's attributes for its children (`details >
    /// p`) by the tree's children: for an element whose children render in slots of its user-agent
    /// shadow tree (a `<details>`, pyro-dom's render tree), the slots. The slots' children, its own
    /// children, are restyled with their subtrees (a closed `<details>` opened shows its content,
    /// and its styles change as a restyle, not as new boxes: transitions start).
    pub(crate) fn invalidate_slotted_children(&self, node_id: NodeId) {
        if !self.nodes.has_shadow_hosts {
            return;
        }
        let Some(node) = self.nodes.get(node_id).filter(|node| node.is_element()) else { return };
        for &slot in &node.children {
            let Some(slot) = self.nodes.get(slot) else { continue };
            if !slot.flags.contains(NodeFlags::IS_ASSIGNED_SLOT) {
                continue;
            }
            for &child in &slot.children {
                let Some(child) = self.nodes.get(child).filter(|child| child.is_element()) else { continue };
                if let Some(mut data) = style::dom::TElement::mutate_data(&child) {
                    data.hint.insert(RestyleHint::restyle_subtree());
                }
                child.mark_ancestors_dirty();
            }
        }
    }

    /// Sets the element's shadow flags from its marker attributes.
    pub(crate) fn sync_shadow_markers(&mut self, node_id: NodeId) {
        let Some(node) = self.nodes.get_mut(node_id) else { return };
        let Some(element) = node.element_data() else { return };
        let mut host = false;
        let mut slot = false;
        for attr in element.attrs.iter() {
            if &*attr.name.ns == crate::PYRO_STATE_NS {
                match &*attr.name.local {
                    SHADOW_HOST_ATTR => host = true,
                    ASSIGNED_SLOT_ATTR => slot = true,
                    _ => {}
                }
            }
        }
        node.flags.set(NodeFlags::IS_SHADOW_HOST, host);
        node.flags.set(NodeFlags::IS_ASSIGNED_SLOT, slot);
        if host || slot {
            self.nodes.has_shadow_hosts = true;
        }
    }

    /// Adds `sheet`, the sheet of the `<style>` or `<link>` element `node_id` in the shadow tree
    /// of `host`, to that shadow root's styles (in node order).
    pub(crate) fn add_shadow_stylesheet(&mut self, host: NodeId, node_id: NodeId, sheet: DocumentStyleSheet) {
        let guard = self.guard.read();
        let root = self.nodes.shadow_roots.entry(host).or_insert_with(ShadowStyles::new);
        if let Some(old) = root.sheets.insert(node_id, sheet.clone()) {
            let custom_media = root.styles.data.custom_media_map();
            root.styles.stylesheets.remove_stylesheet(None, custom_media, old, &guard);
        }
        let before = root.sheets.range((std::ops::Bound::Excluded(node_id), std::ops::Bound::Unbounded)).next();
        match before.map(|(_, s)| s.clone()) {
            Some(before) => root.styles.stylesheets.insert_stylesheet_before(sheet, before, &guard),
            None => root.styles.stylesheets.append_stylesheet(sheet, &guard),
        }
        self.shadow_sheet_hosts.insert(node_id, host);
    }

    /// Removes the sheet of element `node_id` from the shadow root that holds it; whether one did.
    pub(crate) fn remove_shadow_stylesheet(&mut self, node_id: NodeId) -> bool {
        let Some(host) = self.shadow_sheet_hosts.remove(&node_id) else { return false };
        let guard = self.guard.read();
        if let Some(root) = self.nodes.shadow_roots.get_mut(&host)
            && let Some(sheet) = root.sheets.remove(&node_id)
        {
            let custom_media = root.styles.data.custom_media_map();
            root.styles.stylesheets.remove_stylesheet(None, custom_media, sheet, &guard);
        }
        true
    }

    /// Rebuilds the cascade data of the shadow roots whose sheets changed (the shadow trees,
    /// hosts and assigned nodes are restyled), and drops the styles of hosts no longer in the
    /// document.
    pub(crate) fn flush_shadow_styles(&mut self) {
        if self.nodes.shadow_roots.is_empty() {
            return;
        }
        let gone: Vec<NodeId> = self
            .nodes
            .shadow_roots
            .keys()
            .copied()
            .filter(|&host| {
                !self.nodes.get(host).is_some_and(|node| node.is_shadow_host() && node.flags.is_in_document())
            })
            .collect();
        for host in gone {
            if let Some(root) = self.nodes.shadow_roots.remove(&host) {
                for node_id in root.sheets.keys() {
                    self.shadow_sheet_hosts.remove(node_id);
                }
            }
        }
        let guard = self.guard.read();
        let mut restyle = Vec::new();
        for (&host, root) in self.nodes.shadow_roots.iter_mut() {
            if root.styles.stylesheets.dirty() {
                root.styles.flush(&mut self.stylist, &guard);
                restyle.push(host);
            }
        }
        drop(guard);
        for host in restyle {
            // `:host` rules, the shadow tree and `::slotted()` (the assigned nodes are in the
            // host's flat subtree).
            self.nodes[host].set_restyle_hint(RestyleHint::restyle_subtree());
        }
    }

    /// Marks every shadow root's sheets for a rebuild (the device changed: media queries).
    pub(crate) fn invalidate_shadow_styles(&mut self) {
        for root in self.nodes.shadow_roots.values_mut() {
            root.styles.stylesheets.force_dirty();
        }
    }
}
