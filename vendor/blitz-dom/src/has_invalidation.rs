//! pyro: `:has()` invalidation through Stylo's relative selector
//! invalidation, as Gecko runs it on every state and attribute change.
//!
//! Stylo leaves relative selectors to the embedder: the snapshot-driven
//! invalidation of a style traversal looks at an element, its descendants and
//! later siblings, while a `:has()` anchor sits above or before the element
//! that changed. Gecko calls `RelativeSelectorInvalidator` from its restyle
//! manager for each change; this does the same for Blitz:
//!
//! - an element state change (`:checked`, `:focus`, `:hover`, the form states
//!   pyro-dom's render tree mirrors as [`PYRO_STATE_NS`] tokens, ...) looks up the
//!   relative selector map's state dependencies for the changed bits;
//! - an attribute change looks up its id, class, attribute name and custom
//!   state dependencies.
//!
//! Only the anchors whose `:has()` argument depends on what changed, that a
//! `:has()` search actually reached (selector flags), and whose own part of
//! the selector matches are restyled, and only as far as the selector needs
//! (`RESTYLE_SELF`, plus the descendants or siblings it names).
//!
//! Cost when nothing depends on the change: one selector-flags test on the
//! element (no `:has()` search ever visited it), and for an attribute change
//! of such an element one flag test per origin (no `:has()` in the sheets:
//! always the case on a page without `:has()`), then a flag test on the relative selector map
//! ([`BaseDocument::relative_state_dependency`]) or one hash lookup per
//! changed attribute name, id or class.

use selectors::matching::ElementSelectorFlags as F;
use smallvec::SmallVec;
use style::Atom;
use style::values::GenericAtomIdent;
use style::invalidation::element::element_wrapper::Snapshots;
use style::invalidation::element::invalidator::{InvalidationResult, SiblingTraversalMap};
use style::invalidation::element::relative_selector::RelativeSelectorInvalidator;
use style::values::AtomIdent;
use style_dom::ElementState;

use crate::node::{Node, NodeData};
use crate::{BaseDocument, QualName, local_name, ns};
use blitz_traits::node_id::NodeId;

/// Namespace of the attributes pyro-dom's render tree adds for the
/// pseudo-classes Blitz cannot derive itself (pyro-dom `snapshot::STATE_NS`).
pub const PYRO_STATE_NS: &str = "urn:x-pyro:state";

/// The element states the `state` attribute of [`PYRO_STATE_NS`] carries.
pub(crate) const PYRO_STATES: ElementState = ElementState::VALID
    .union(ElementState::INVALID)
    .union(ElementState::INDETERMINATE)
    .union(ElementState::REQUIRED)
    .union(ElementState::OPTIONAL_)
    .union(ElementState::READWRITE)
    .union(ElementState::READONLY)
    .union(ElementState::PLACEHOLDER_SHOWN)
    .union(ElementState::DEFAULT)
    .union(ElementState::OPEN)
    .union(ElementState::MODAL)
    .union(ElementState::POPOVER_OPEN)
    .union(ElementState::INRANGE)
    .union(ElementState::OUTOFRANGE)
    .union(ElementState::USER_VALID)
    .union(ElementState::USER_INVALID)
    .union(ElementState::DEFINED);

/// The element states a [`PYRO_STATE_NS`] `state` attribute value (`None`: no
/// such attribute) stands for. An element is `:read-only` unless the value
/// says `read-write`, and `:defined` unless it says `undefined` (a custom element
/// not defined yet).
pub(crate) fn pyro_state_bits(value: Option<&str>) -> ElementState {
    let mut state = ElementState::empty();
    let mut undefined = false;
    for token in value.unwrap_or("").split_ascii_whitespace() {
        state |= match token {
            "valid" => ElementState::VALID,
            "invalid" => ElementState::INVALID,
            "indeterminate" => ElementState::INDETERMINATE,
            "required" => ElementState::REQUIRED,
            "optional" => ElementState::OPTIONAL_,
            "read-write" => ElementState::READWRITE,
            "placeholder-shown" => ElementState::PLACEHOLDER_SHOWN,
            "default" => ElementState::DEFAULT,
            "open" => ElementState::OPEN,
            "modal" => ElementState::MODAL,
            "popover-open" => ElementState::POPOVER_OPEN,
            "in-range" => ElementState::INRANGE,
            "out-of-range" => ElementState::OUTOFRANGE,
            "user-valid" => ElementState::USER_VALID,
            "user-invalid" => ElementState::USER_INVALID,
            // pyro: `:enabled`/`:disabled` (see `sync_lp_attribute_state`).
            "enabled" => ElementState::ENABLED,
            "disabled" => ElementState::DISABLED,
            "undefined" => {
                undefined = true;
                ElementState::empty()
            }
            _ => ElementState::empty(),
        };
    }
    if !state.contains(ElementState::READWRITE) {
        state |= ElementState::READONLY;
    }
    if !undefined {
        state |= ElementState::DEFINED;
    }
    state
}

/// The custom states (`:state()`) a [`PYRO_STATE_NS`] `custom-states`
/// attribute value lists.
pub(crate) fn pyro_custom_states(value: Option<&str>) -> impl Iterator<Item = &str> {
    value
        .unwrap_or("")
        .split('\u{1f}')
        .filter(|s| !s.is_empty())
}

/// What an attribute change changed, for
/// [`BaseDocument::invalidate_relative_attribute`]: taken before the change
/// ([`AttributeChange::before`]) and completed after it
/// ([`AttributeChange::after`]).
pub(crate) struct AttributeChange {
    /// The attribute's local name, as Stylo's attribute dependency map keys it.
    local: style::LocalName,
    kind: AttrKind,
    /// Element state bits before (then: the bits that changed).
    state: ElementState,
    /// Old id, classes or custom states; then those added or removed.
    names: SmallVec<[Atom; 4]>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AttrKind {
    Id,
    Class,
    CustomStates,
    Other,
}

/// The `:has()` search directions an element visited by a search carries.
const SEARCHED: F = F::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR_SIBLING;

fn searched(node: &Node) -> bool {
    node.is_element() && node.selector_flags().get().intersects(SEARCHED)
}

fn custom_states_attr(node: &Node) -> Option<&str> {
    node.attrs()?
        .iter()
        .find(|a| &*a.name.ns == PYRO_STATE_NS && &*a.name.local == "custom-states")
        .map(|a| a.value.as_str())
}

/// The ids, classes or custom states `kind` names on `node`.
fn names_of(node: &Node, kind: AttrKind) -> SmallVec<[Atom; 4]> {
    let mut out = SmallVec::new();
    let Some(el) = node.element_data() else { return out };
    match kind {
        AttrKind::Id => out.extend(el.id.clone()),
        AttrKind::Class => out.extend(el.classes.iter().cloned()),
        AttrKind::CustomStates => out.extend(pyro_custom_states(custom_states_attr(node)).map(Atom::from)),
        AttrKind::Other => {}
    }
    out
}

impl AttributeChange {
    /// `node_id`'s state before its attribute `name` changes; `None` when no
    /// `:has()` can depend on the element (always the case on a page without
    /// `:has()`).
    ///
    /// pyro: an element no `:has()` search visited still matters when a
    /// complex selector inside the argument reaches it from the searched
    /// subtree (`.a:has(.c:is(.b .c))` with `.b` above the anchor, or a sibling
    /// of one): Stylo's dependency lookup decides, as in Gecko.
    pub(crate) fn before(doc: &BaseDocument, node_id: NodeId, name: &QualName) -> Option<Self> {
        let node = doc.nodes.get(node_id)?;
        if !node.flags.is_in_document() {
            return None;
        }
        if !searched(node) && !(doc.relative_selectors_used() || node.containing_shadow_host_id().is_some()) {
            return None;
        }
        let kind = if name.ns == ns!() && name.local == local_name!("id") {
            AttrKind::Id
        } else if name.ns == ns!() && name.local == local_name!("class") {
            AttrKind::Class
        } else if &*name.ns == PYRO_STATE_NS && &*name.local == "custom-states" {
            AttrKind::CustomStates
        } else {
            AttrKind::Other
        };
        Some(AttributeChange {
            local: GenericAtomIdent(name.local.clone()),
            kind,
            state: *node.element_state(),
            names: names_of(node, kind),
        })
    }

    /// Completes the change with the element as it is after it.
    pub(crate) fn after(mut self, doc: &BaseDocument, node_id: NodeId) -> Option<Self> {
        let node = doc.nodes.get(node_id)?;
        if !node.is_element() {
            return None;
        }
        self.state ^= *node.element_state();
        if self.kind != AttrKind::Other {
            let old = std::mem::take(&mut self.names);
            let new = names_of(node, self.kind);
            if old.len() + new.len() <= 64 {
                // The ones added or removed.
                self.names.extend(old.iter().filter(|c| !new.contains(c)).cloned());
                self.names.extend(new.iter().filter(|c| !old.contains(c)).cloned());
            } else {
                // A long list: every name (one lookup each) rather than a
                // quadratic difference.
                self.names = old;
                self.names.extend(new);
            }
        }
        Some(self)
    }
}

/// Makes the style traversal reach what the relative selector invalidation
/// gave a restyle hint: the anchor (and the descendants it propagated its
/// dirty bit from), or the siblings it invalidated.
fn anchor_invalidated(element: &Node, result: &InvalidationResult) {
    if result.has_invalidated_siblings() {
        if let Some(parent) = element.parent {
            let parent = &element.tree()[parent];
            parent.set_dirty_descendants();
            parent.mark_ancestors_dirty();
        }
    }
    if result.has_invalidated_descendants() {
        element.set_dirty_descendants();
    }
    if result.has_invalidated_self() || result.has_invalidated_descendants() {
        element.mark_ancestors_dirty();
    }
}

impl BaseDocument {
    /// pyro: whether the document's sheets have any `:has()` (a flag test per
    /// origin).
    pub(crate) fn relative_selectors_used(&self) -> bool {
        self.stylist.iter_origins().any(|(data, _)| data.relative_invalidation_map_attributes().used)
    }

    /// Whether some `:has()` argument depends on any of the `state` bits: a
    /// flag test per origin's relative selector map.
    pub fn relative_state_dependency(&self, state: ElementState) -> bool {
        self.stylist
            .iter_origins()
            .any(|(data, _)| data.relative_invalidation_map_attributes().state_dependencies.intersects(state))
    }

    /// `:has()` invalidation for a change of the `changed` state bits of
    /// `node_id` (made already; a snapshot holds the state before it).
    pub(crate) fn invalidate_relative_state(&self, node_id: NodeId, changed: ElementState) {
        // pyro: `:has-slotted(<selector>)` of its slot (`crate::shadow`).
        if !changed.is_empty() {
            self.invalidate_has_slotted(node_id);
        }
        let Some(node) = self.nodes.get(node_id) else { return };
        // pyro: a shadow tree's own sheets may depend on it (`relative_state_dependency` checks the
        // document's).
        if changed.is_empty()
            || !searched(node)
            || !(self.relative_state_dependency(changed) || node.containing_shadow_host_id().is_some())
        {
            return;
        }
        self.run_relative_invalidation(node, changed, None);
    }

    /// `:has()` invalidation for an attribute change of `node_id`.
    pub(crate) fn invalidate_relative_attribute(&self, node_id: NodeId, change: AttributeChange) {
        let Some(node) = self.nodes.get(node_id) else { return };
        let state = if self.relative_state_dependency(change.state) { change.state } else { ElementState::empty() };
        self.run_relative_invalidation(node, state, Some(&change));
    }

    fn run_relative_invalidation(&self, node: &Node, state: ElementState, attr: Option<&AttributeChange>) {
        if !matches!(node.data, NodeData::Element(_)) {
            return;
        }
        // pyro: `:host:has()` (`crate::shadow`).
        self.invalidate_host_relative_selectors(node);
        let snapshots = Snapshots::new(&self.snapshots);
        // Given snapshots, Stylo (0.24 on) drops a dependency whose compound
        // matches the element as it did per its snapshot. That holds only
        // when the snapshot has what changed: an element never styled (in a
        // `display: none` subtree) has none (`snapshot_node_impl`), and
        // Servo's snapshot keeps no custom states. Both would read as
        // unchanged.
        let comparable =
            node.has_snapshot() && !attr.is_some_and(|attr| matches!(attr.kind, AttrKind::CustomStates));
        let invalidator = RelativeSelectorInvalidator {
            element: node,
            quirks_mode: self.stylist.quirks_mode(),
            snapshots: comparable.then_some(&snapshots),
            invalidated: anchor_invalidated,
            sibling_traversal_map: SiblingTraversalMap::default(),
        };
        invalidator.invalidate_relative_selectors_for_this(
            &self.stylist,
            |element, scope, data, quirks_mode, collector| {
                let map = data.relative_selector_invalidation_map();
                if !state.is_empty() {
                    map.state_affecting_selectors.lookup_with_additional(
                        *element,
                        quirks_mode,
                        None,
                        &[],
                        state,
                        |dependency| {
                            if dependency.state.intersects(state) {
                                collector.add_dependency(&dependency.dep, *element, scope);
                            }
                            true
                        },
                    );
                }
                let Some(attr) = attr else { return };
                if let Some(deps) = map.other_attribute_affecting_selectors.get(&attr.local) {
                    for dependency in deps {
                        collector.add_dependency(dependency, *element, scope);
                    }
                }
                for name in &attr.names {
                    let deps = match attr.kind {
                        AttrKind::Id => map.id_to_selector.get(name, quirks_mode),
                        AttrKind::Class => map.class_to_selector.get(name, quirks_mode),
                        AttrKind::CustomStates => {
                            map.custom_state_affecting_selectors.get(AtomIdent::cast(name))
                        }
                        AttrKind::Other => None,
                    };
                    for dependency in deps.into_iter().flatten() {
                        collector.add_dependency(dependency, *element, scope);
                    }
                }
            },
        );
    }
}
