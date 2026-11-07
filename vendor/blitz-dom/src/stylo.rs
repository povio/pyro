//! Enable the dom to participate in styling by servo
//!

use blitz_traits::node_id::NodeId;
use std::ptr::NonNull;
use std::sync::Mutex;
use std::sync::atomic::Ordering;

use crate::StyleThreading;
use crate::layout::damage::compute_layout_damage;
use crate::node::Node;
use crate::node::NodeData;
use markup5ever::{LocalName, LocalNameStaticSet, Namespace, NamespaceStaticSet, local_name, ns};
use selectors::bloom::BLOOM_HASH_MASK;
use selectors::{
    Element, OpaqueElement,
    attr::{AttrSelectorOperation, NamespaceConstraint},
    matching::{ElementSelectorFlags, MatchingContext, VisitedHandlingMode},
    sink::Push,
};
use style::CaseSensitivityExt;
use style::animation::AnimationSetKey;
use style::animation::AnimationState;
use style::applicable_declarations::ApplicableDeclarationBlock;
use style::bloom::each_relevant_element_hash;
use style::color::AbsoluteColor;
use style::data::{ElementDataMut, ElementDataRef};
use style::global_style_data::STYLE_THREAD_POOL;
use style::invalidation::element::restyle_hints::RestyleHint;
use style::properties::ComputedValues;
use style::properties::{Importance, PropertyDeclaration};
use style::rule_tree::CascadeLevel;
use style::rule_tree::CascadeOrigin;
use style::invalidation::element::invalidator::{InvalidationResult, SiblingTraversalMap};
use style::invalidation::element::relative_selector::{DomMutationOperation, RelativeSelectorInvalidator};
use style::selector_parser::PseudoElement;
use style::selector_parser::RestyleDamage;
use style::stylesheets::layer_rule::LayerOrder;
use style::stylesheets::scope_rule::ImplicitScopeRoot;
use style::values::AtomString;
use style::values::specified::{NoCalcPercentage, XLang};
use style::{
    Atom,
    context::{
        QuirksMode, RegisteredSpeculativePainter, RegisteredSpeculativePainters,
        SharedStyleContext, StyleContext, ThreadLocalStyleContext,
    },
    dom::{LayoutIterator, NodeInfo, OpaqueNode, TDocument, TElement, TNode, TShadowRoot},
    global_style_data::GLOBAL_STYLE_DATA,
    properties::PropertyDeclarationBlock,
    selector_parser::{NonTSPseudoClass, SelectorImpl},
    servo_arc::{Arc, ArcBorrow},
    shared_lock::{Locked, SharedRwLock, StylesheetGuards},
    stylist::RuleInclusion,
    thread_state::ThreadState,
    traversal::{DomTraversal, resolve_style},
    traversal_flags::TraversalFlags,
    values::{AtomIdent, GenericAtomIdent},
};
use style_dom::ElementState;

use style::values::computed::text::TextAlign as StyloTextAlign;

struct LayoutThreadScope;

impl LayoutThreadScope {
    fn enter() -> Self {
        style::thread_state::enter(ThreadState::LAYOUT);
        Self
    }
}

impl Drop for LayoutThreadScope {
    fn drop(&mut self) {
        style::thread_state::exit(ThreadState::LAYOUT);
    }
}

impl Node {
    /// lightningpanda: whether the element's `urn:x-lightningpanda:state`
    /// attribute (written by lp-dom's render tree) lists `state`: the
    /// pseudo-classes Blitz cannot derive itself (validity, placeholder
    /// shown, read-write, ...).
    fn has_lp_state(&self, state: &str) -> bool {
        self.element_data().is_some_and(|e| {
            e.attrs.iter().any(|a| {
                &*a.name.ns == "urn:x-lightningpanda:state" && a.value.split_ascii_whitespace().any(|s| s == state)
            })
        })
    }
}

impl crate::document::BaseDocument {
    pub fn resolve_stylist(&mut self, now: f64) {
        let _layout_thread_scope = LayoutThreadScope::enter();

        let guard = &self.guard;
        let guards = StylesheetGuards {
            author: &guard.read(),
            ua_or_user: &guard.read(),
        };

        let root = TDocument::as_node(&&self.nodes[self.root_node_id])
            .first_element_child()
            .unwrap()
            .as_element()
            .unwrap();

        self.stylist
            .flush(&guards)
            .process_style(root, Some(&self.snapshots));

        // Mark actively animating nodes as dirty
        let mut sets = self.animations.sets.write();
        for (key, set) in sets.iter_mut() {
            let node_id = NodeId::from_u64(key.node.id() as u64);

            // Drop animations belonging to nodes that are no longer in the
            // document. A removed element is never restyled, so it would never
            // get a chance to cancel its own animations; an infinite animation
            // would then keep `has_active_animations` set forever and force a
            // redraw every frame. Emptying the set here lets the `retain` below
            // discard it so the flag can clear on this same pass.
            let in_document = self
                .nodes
                .get(node_id)
                .is_some_and(|node| node.flags.is_in_document());
            if !in_document {
                set.animations.clear();
                set.transitions.clear();
                continue;
            }

            self.nodes[node_id].set_restyle_hint(RestyleHint::RESTYLE_SELF);

            for animation in set.animations.iter_mut() {
                if animation.state == AnimationState::Pending && animation.started_at <= now {
                    animation.state = AnimationState::Running;
                }
                animation.iterate_if_necessary(now);

                if animation.state == AnimationState::Running && animation.has_ended(now) {
                    animation.state = AnimationState::Finished;
                }
            }

            for transition in set.transitions.iter_mut() {
                if transition.state == AnimationState::Pending && transition.start_time <= now {
                    transition.state = AnimationState::Running;
                }
                if transition.state == AnimationState::Running && transition.has_ended(now) {
                    transition.state = AnimationState::Finished;
                }
            }
        }
        drop(sets);

        // Build the style context used by the style traversal
        let context = SharedStyleContext {
            traversal_flags: TraversalFlags::empty(),
            stylist: &self.stylist,
            options: GLOBAL_STYLE_DATA.options.clone(),
            guards,
            visited_styles_enabled: false,
            animations: self.animations.clone(),
            current_time_for_animations: now,
            snapshot_map: &self.snapshots,
            registered_speculative_painters: &RegisteredPaintersImpl,
        };

        // components/layout_2020/lib.rs:983
        let root = self.root_element();
        // dbg!(root);
        let token = RecalcStyle::pre_traverse(root, &context);

        let mut nodes_needing_style_image_flush = Vec::new();
        if token.should_traverse() {
            // Style the elements, resolving their data
            let mut traverser = RecalcStyle::new(context);
            // `Sequential` bypasses Stylo's global pool. See `StyleThreading`.
            let pool_guard = matches!(self.style_threading, StyleThreading::Parallel)
                .then(|| STYLE_THREAD_POOL.pool());
            let rayon_pool = pool_guard.as_ref().and_then(|g| g.as_ref());
            style::driver::traverse_dom(&traverser, token, rayon_pool);
            nodes_needing_style_image_flush =
                std::mem::take(traverser.nodes_needing_style_image_flush.get_mut().unwrap());
        }
        self.pending_style_image_nodes
            .extend(nodes_needing_style_image_flush);

        for opaque in self.snapshots.keys() {
            let id = NodeId::from_u64(opaque.id() as u64);
            if let Some(node) = self.nodes.get_mut(id) {
                node.set_has_snapshot(false);
            }
        }
        self.snapshots.clear();

        let mut sets = self.animations.sets.write();
        for set in sets.values_mut() {
            set.clear_canceled_animations();
            for animation in set.animations.iter_mut() {
                animation.is_new = false;
            }
            for transition in set.transitions.iter_mut() {
                transition.is_new = false;
            }
        }
        sets.retain(|_, state| !state.is_empty());
        self.has_active_animations = sets.values().any(|state| state.needs_animation_ticks());

        // Maybe run garbage collection. Stylo has internal to determine whether to run or not.
        self.stylist.rule_tree().maybe_gc();
    }

    /// Compute the style of an element which the regular style traversal
    /// skipped because it lives inside a `display: none` subtree.
    ///
    /// The style traversal culls `display: none` subtrees, so such elements
    /// never have styles stored on them, but `getComputedStyle()` must still
    /// return their computed values. This resolves the style on demand
    /// (styling any unstyled ancestors along the way) without storing it on the
    /// node. Returns `None` for non-elements and elements outside the document.
    pub fn resolve_undisplayed_style(&self, node_id: NodeId) -> Option<Arc<ComputedValues>> {
        let node = self.nodes.get(node_id)?;
        if !node.is_element() || !node.flags.is_in_document() {
            return None;
        }

        let _layout_thread_scope = LayoutThreadScope::enter();

        let guard = &self.guard;
        let guards = StylesheetGuards {
            author: &guard.read(),
            ua_or_user: &guard.read(),
        };
        let shared = SharedStyleContext {
            traversal_flags: TraversalFlags::empty(),
            stylist: &self.stylist,
            options: GLOBAL_STYLE_DATA.options.clone(),
            guards,
            visited_styles_enabled: false,
            animations: self.animations.clone(),
            current_time_for_animations: 0.0,
            snapshot_map: &self.snapshots,
            registered_speculative_painters: &RegisteredPaintersImpl,
        };
        let mut thread_local = ThreadLocalStyleContext::new();
        let mut context = StyleContext {
            shared: &shared,
            thread_local: &mut thread_local,
        };
        let styles = resolve_style(&mut context, node, RuleInclusion::All, None, None);

        Some(styles.primary().clone())
    }
}

/// A handle to a node that Servo's style traits are implemented against
///
/// Since BlitzNodes are not persistent (IE we don't keep the pointers around between frames), we choose to just implement
/// the tree structure in the nodes themselves, and temporarily give out pointers during the layout phase.
type BlitzNode<'a> = &'a Node;

impl<'a> TDocument for BlitzNode<'a> {
    type ConcreteNode = BlitzNode<'a>;

    fn as_node(&self) -> Self::ConcreteNode {
        self
    }

    fn is_html_document(&self) -> bool {
        true
    }

    fn quirks_mode(&self) -> QuirksMode {
        QuirksMode::NoQuirks
    }

    fn shared_lock(&self) -> &SharedRwLock {
        self.guard()
    }
}

impl NodeInfo for BlitzNode<'_> {
    fn is_element(&self) -> bool {
        Node::is_element(self)
    }

    fn is_text_node(&self) -> bool {
        Node::is_text_node(self)
    }
}

impl<'a> TShadowRoot for BlitzNode<'a> {
    type ConcreteNode = BlitzNode<'a>;

    fn as_node(&self) -> Self::ConcreteNode {
        self
    }

    fn host(&self) -> <Self::ConcreteNode as TNode>::ConcreteElement {
        todo!("Shadow roots not implemented")
    }

    fn style_data<'b>(&self) -> Option<&'b style::stylist::CascadeData>
    where
        Self: 'b,
    {
        todo!("Shadow roots not implemented")
    }
}

// components/styleaapper.rs:
impl<'a> TNode for BlitzNode<'a> {
    type ConcreteElement = BlitzNode<'a>;
    type ConcreteDocument = BlitzNode<'a>;
    type ConcreteShadowRoot = BlitzNode<'a>;

    fn parent_node(&self) -> Option<Self> {
        self.parent.map(|id| self.with(id))
    }

    fn first_child(&self) -> Option<Self> {
        self.children.first().map(|id| self.with(*id))
    }

    fn last_child(&self) -> Option<Self> {
        self.children.last().map(|id| self.with(*id))
    }

    fn prev_sibling(&self) -> Option<Self> {
        self.backward(1)
    }

    fn next_sibling(&self) -> Option<Self> {
        self.forward(1)
    }

    fn owner_doc(&self) -> Self::ConcreteDocument {
        // Walk up the (layout-)parent chain to the root Document node.
        let mut node = *self;
        while let Some(parent_id) = node.parent {
            node = node.with(parent_id);
        }
        node
    }

    fn is_in_document(&self) -> bool {
        true
    }

    // I think this is the same as parent_node only in the cases when the direct parent is not a real element, forcing us
    // to travel upwards
    //
    // For the sake of this demo, we're just going to return the parent node ann
    fn traversal_parent(&self) -> Option<Self::ConcreteElement> {
        self.parent_node().and_then(|node| node.as_element())
    }

    fn opaque(&self) -> OpaqueNode {
        OpaqueNode(self.id.as_u64() as usize)
    }

    fn debug_id(self) -> usize {
        self.id.as_u64() as usize
    }

    fn as_element(&self) -> Option<Self::ConcreteElement> {
        match self.data {
            NodeData::Element { .. } => Some(self),
            _ => None,
        }
    }

    fn as_document(&self) -> Option<Self::ConcreteDocument> {
        match self.data {
            NodeData::Document(_) => Some(self),
            _ => None,
        }
    }

    fn as_shadow_root(&self) -> Option<Self::ConcreteShadowRoot> {
        // TODO: implement shadow DOM
        None
    }
}

impl selectors::Element for BlitzNode<'_> {
    type Impl = SelectorImpl;

    fn opaque(&self) -> selectors::OpaqueElement {
        // This correctly uses a unique id for the OpaqueElement (unlike using a pointer to the "slot")
        // However, it makes it impossible for us to "rehydrate" the OpaqueElement back into an actual Element
        // which is required to implement the `implicit_scope_for_sheet_in_shadow_root` method below
        //
        // We should see if selectors will accept a PR that allows us to use 128bits for the OpaqueElement. Or
        // find some other solution that will enable "rehydration". This is required to enable and use the
        // Shadow DOM functionality in Stylo.
        let non_null =
            NonNull::new((self.id.as_u64() as usize).wrapping_add(1) as *mut ()).unwrap();
        OpaqueElement::from_non_null_ptr(non_null)
    }

    fn parent_element(&self) -> Option<Self> {
        TElement::traversal_parent(self)
    }

    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }

    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }

    fn is_pseudo_element(&self) -> bool {
        matches!(self.data, NodeData::AnonymousBlock(_))
    }

    // These methods are implemented naively since we only threaded real nodes and not fake nodes
    // we should try and use `find` instead of this foward/backward stuff since its ugly and slow
    fn prev_sibling_element(&self) -> Option<Self> {
        let mut n = 1;
        while let Some(node) = self.backward(n) {
            if node.is_element() {
                return Some(node);
            }
            n += 1;
        }

        None
    }

    fn next_sibling_element(&self) -> Option<Self> {
        let mut n = 1;
        while let Some(node) = self.forward(n) {
            if node.is_element() {
                return Some(node);
            }
            n += 1;
        }

        None
    }

    fn first_element_child(&self) -> Option<Self> {
        let mut children = self.dom_children();
        children.find(|child| child.is_element())
    }

    fn is_html_element_in_html_document(&self) -> bool {
        true // self.has_namespace(ns!(html))
    }

    fn has_local_name(&self, local_name: &LocalName) -> bool {
        self.data.is_element_with_tag_name(local_name)
    }

    fn has_namespace(&self, ns: &Namespace) -> bool {
        self.element_data().expect("Not an element").name.ns == *ns
    }

    fn is_same_type(&self, other: &Self) -> bool {
        self.local_name() == other.local_name() && self.namespace() == other.namespace()
    }

    fn attr_matches(
        &self,
        ns: &NamespaceConstraint<&GenericAtomIdent<NamespaceStaticSet>>,
        local_name: &GenericAtomIdent<LocalNameStaticSet>,
        operation: &AttrSelectorOperation<&AtomString>,
    ) -> bool {
        let Some(attrs) = self.data.attrs() else {
            return false;
        };
        attrs.iter().any(|attr| {
            attr.name.local == local_name.0
                && match ns {
                    NamespaceConstraint::Any => true,
                    NamespaceConstraint::Specific(ns) => attr.name.ns == ns.0,
                }
                && operation.eval_str(&attr.value)
        })
    }

    fn match_non_ts_pseudo_class(
        &self,
        pseudo_class: &<Self::Impl as selectors::SelectorImpl>::NonTSPseudoClass,
        _context: &mut MatchingContext<Self::Impl>,
    ) -> bool {
        match *pseudo_class {
            NonTSPseudoClass::Active => self.element_state().contains(ElementState::ACTIVE),
            NonTSPseudoClass::AnyLink => self
                .element_state()
                .intersects(ElementState::VISITED_OR_UNVISITED),
            NonTSPseudoClass::Checked => self.element_state().contains(ElementState::CHECKED),
            NonTSPseudoClass::Valid => self.has_lp_state("valid"),
            NonTSPseudoClass::Invalid => self.has_lp_state("invalid"),
            NonTSPseudoClass::Defined => false,
            NonTSPseudoClass::Disabled => self.element_state().contains(ElementState::DISABLED),
            NonTSPseudoClass::Enabled => self.element_state().contains(ElementState::ENABLED),
            NonTSPseudoClass::Focus => self.element_state().contains(ElementState::FOCUS),
            NonTSPseudoClass::FocusWithin => self.element_state().contains(ElementState::FOCUS_WITHIN),
            NonTSPseudoClass::FocusVisible => self.element_state().contains(ElementState::FOCUS),
            NonTSPseudoClass::Fullscreen => false,
            NonTSPseudoClass::Hover => self.element_state().contains(ElementState::HOVER),
            NonTSPseudoClass::Indeterminate => self.has_lp_state("indeterminate"),
            NonTSPseudoClass::Lang(_) => false,
            NonTSPseudoClass::CustomState(_) => false,
            NonTSPseudoClass::Link => self.element_state().contains(ElementState::UNVISITED),
            NonTSPseudoClass::PlaceholderShown => self.has_lp_state("placeholder-shown"),
            NonTSPseudoClass::ReadWrite => self.has_lp_state("read-write"),
            NonTSPseudoClass::ReadOnly => !self.has_lp_state("read-write"),
            NonTSPseudoClass::ServoNonZeroBorder => false,
            NonTSPseudoClass::Target => false,
            NonTSPseudoClass::Visited => false,
            NonTSPseudoClass::Autofill => false,
            NonTSPseudoClass::Default => self.has_lp_state("default"),

            NonTSPseudoClass::InRange => false,
            NonTSPseudoClass::Modal => self.has_lp_state("modal"),
            NonTSPseudoClass::Open => self.has_lp_state("open"),
            NonTSPseudoClass::Optional => self.has_lp_state("optional"),
            NonTSPseudoClass::OutOfRange => false,
            NonTSPseudoClass::PopoverOpen => self.has_lp_state("popover-open"),
            NonTSPseudoClass::Required => self.has_lp_state("required"),
            NonTSPseudoClass::UserInvalid => false,
            NonTSPseudoClass::UserValid => false,
            NonTSPseudoClass::MozMeterOptimum => false,
            NonTSPseudoClass::MozMeterSubOptimum => false,
            NonTSPseudoClass::MozMeterSubSubOptimum => false,
        }
    }

    fn match_pseudo_element(
        &self,
        pe: &PseudoElement,
        _context: &mut MatchingContext<Self::Impl>,
    ) -> bool {
        let pseudo = match self.try_stylo_element_data().and_then(|s| s.get()) {
            Some(el) => el
                .styles
                .get_primary()
                .and_then(|s| s.pseudo())
                .or(match &self.data {
                    NodeData::AnonymousBlock(_) => Some(PseudoElement::ServoAnonymousBox),
                    _ => None,
                }),
            None => None,
        };

        pseudo.is_some_and(|psuedo| psuedo == *pe)
    }

    fn apply_selector_flags(&self, flags: ElementSelectorFlags) {
        // Handle flags that apply to the element.
        let self_flags = flags.for_self();
        if !self_flags.is_empty() {
            self.selector_flags()
                .set(self.selector_flags().get() | self_flags);
        }

        // Handle flags that apply to the parent.
        let parent_flags = flags.for_parent();
        if !parent_flags.is_empty() {
            if let Some(parent) = self.parent_node() {
                parent
                    .selector_flags()
                    .set(parent.selector_flags().get() | parent_flags);
            }
        }
    }

    fn is_link(&self) -> bool {
        self.data.is_element_with_tag_name(&local_name!("a"))
    }

    fn is_html_slot_element(&self) -> bool {
        false
    }

    fn has_id(
        &self,
        id: &<Self::Impl as selectors::SelectorImpl>::Identifier,
        case_sensitivity: selectors::attr::CaseSensitivity,
    ) -> bool {
        self.element_data()
            .and_then(|data| data.id.as_ref())
            .map(|id_attr| case_sensitivity.eq_atom(id_attr, id))
            .unwrap_or(false)
    }

    fn has_class(
        &self,
        search_name: &<Self::Impl as selectors::SelectorImpl>::Identifier,
        case_sensitivity: selectors::attr::CaseSensitivity,
    ) -> bool {
        self.element_data().is_some_and(|el| {
            el.attrs
                .class_atoms()
                .iter()
                .any(|atom| case_sensitivity.eq_atom(atom, search_name))
        })
    }

    fn imported_part(
        &self,
        _name: &<Self::Impl as selectors::SelectorImpl>::Identifier,
    ) -> Option<<Self::Impl as selectors::SelectorImpl>::Identifier> {
        None
    }

    fn is_part(&self, _name: &<Self::Impl as selectors::SelectorImpl>::Identifier) -> bool {
        false
    }

    fn is_empty(&self) -> bool {
        self.dom_children().next().is_none()
    }

    fn is_root(&self) -> bool {
        self.parent_node()
            .and_then(|parent| parent.parent_node())
            .is_none()
    }

    fn has_custom_state(
        &self,
        _name: &<Self::Impl as selectors::SelectorImpl>::Identifier,
    ) -> bool {
        false
    }

    fn add_element_unique_hashes(&self, filter: &mut selectors::bloom::BloomFilter) -> bool {
        each_relevant_element_hash(*self, |hash| filter.insert_hash(hash & BLOOM_HASH_MASK));
        true
    }
}

impl<'a> TElement for BlitzNode<'a> {
    type ConcreteNode = BlitzNode<'a>;

    type TraversalChildrenIterator = Traverser<'a>;

    fn as_node(&self) -> Self::ConcreteNode {
        self
    }

    fn implicit_scope_for_sheet_in_shadow_root(
        _opaque_host: OpaqueElement,
        _sheet_index: usize,
    ) -> Option<ImplicitScopeRoot> {
        // We cannot currently implement this as we are using the NodeId as the OpaqueElement,
        // and need a reference to the Slab to convert it back into an Element
        //
        // Luckily it is only needed for shadow dom.
        todo!();
    }

    fn traversal_children(&self) -> style::dom::LayoutIterator<Self::TraversalChildrenIterator> {
        LayoutIterator(Traverser {
            // dom: self.tree(),
            parent: self,
            child_index: 0,
        })
    }

    fn is_html_element(&self) -> bool {
        self.is_element()
    }

    // not implemented.....
    fn is_mathml_element(&self) -> bool {
        false
    }

    // need to check the namespace
    fn is_svg_element(&self) -> bool {
        false
    }

    fn style_attribute(&self) -> Option<ArcBorrow<'_, Locked<PropertyDeclarationBlock>>> {
        self.element_data()
            .expect("Not an element")
            .style_attribute
            .as_ref()
            .map(|f| f.borrow_arc())
    }

    fn state(&self) -> ElementState {
        *self.element_state()
    }

    fn has_part_attr(&self) -> bool {
        false
    }

    fn exports_any_part(&self) -> bool {
        false
    }

    fn id(&self) -> Option<&style::Atom> {
        self.element_data().and_then(|data| data.id.as_ref())
    }

    fn each_class<F>(&self, mut callback: F)
    where
        F: FnMut(&style::values::AtomIdent),
    {
        if let Some(el) = self.element_data() {
            for atom in el.attrs.class_atoms() {
                callback(AtomIdent::cast(atom));
            }
        }
    }

    fn each_attr_name<F>(&self, mut callback: F)
    where
        F: FnMut(&style::LocalName),
    {
        if let Some(attrs) = self.data.attrs() {
            for attr in attrs.iter() {
                callback(&GenericAtomIdent(attr.name.local.clone()));
            }
        }
    }

    fn has_dirty_descendants(&self) -> bool {
        Node::has_dirty_descendants(self)
    }

    fn has_snapshot(&self) -> bool {
        Node::has_snapshot(self)
    }

    fn handled_snapshot(&self) -> bool {
        self.snapshot_handled().load(Ordering::SeqCst)
    }

    unsafe fn set_handled_snapshot(&self) {
        self.snapshot_handled().store(true, Ordering::SeqCst);
    }

    // Stylo calls this on elements it is already traversing (or has already
    // reached via invalidation), so the ancestor chain must not be re-marked:
    // ancestors visited earlier in the preorder traversal have had their bits
    // cleared, and re-flagging them leaves stale `dirty_descendants` bits that
    // break the early-out invariant of `Node::mark_ancestors_dirty` (a set bit
    // implies all ancestors are set).
    unsafe fn set_dirty_descendants(&self) {
        Node::set_dirty_descendants(self);
    }

    unsafe fn unset_dirty_descendants(&self) {
        Node::unset_dirty_descendants(self);
    }

    fn store_children_to_process(&self, _n: isize) {
        unimplemented!()
    }

    fn did_process_child(&self) -> isize {
        unimplemented!()
    }

    unsafe fn ensure_data(&self) -> ElementDataMut<'_> {
        // SAFETY: stylo traversal has exclusive access to nodes
        unsafe { self.stylo_element_data().ensure_init() }
    }

    unsafe fn clear_data(&self) {
        if let Some(style) = self.primary_styles() {
            self.tree().note_primary_style_change(Some(&**style), None);
        }
        // SAFETY: stylo traversal has exclusive access to nodes
        unsafe { self.stylo_element_data().clear() }
    }

    fn has_data(&self) -> bool {
        self.try_stylo_element_data().is_some_and(|s| s.has_data())
    }

    fn borrow_data(&self) -> Option<ElementDataRef<'_>> {
        // lightningpanda: elements get their data when created, before they
        // are styled. Until then they are unstyled to Stylo (as in Gecko and
        // Servo, which give an element data when they style it): the
        // traversal styles them from scratch, and invalidation must not read
        // their (missing) styles.
        self.try_stylo_element_data()
            .and_then(|s| s.get())
            .filter(|data| data.styles.get_primary().is_some())
    }

    fn mutate_data(&self) -> Option<ElementDataMut<'_>> {
        unsafe { self.stylo_element_data().unsafe_stylo_only_mut() }
    }

    fn skip_item_display_fixup(&self) -> bool {
        false
    }

    fn may_have_animations(&self) -> bool {
        true
    }

    fn has_animations(&self, context: &SharedStyleContext) -> bool {
        self.has_css_animations(context, None) || self.has_css_transitions(context, None)
    }

    fn has_css_animations(
        &self,
        context: &SharedStyleContext,
        pseudo_element: Option<PseudoElement>,
    ) -> bool {
        let key = AnimationSetKey::new(TNode::opaque(&TElement::as_node(self)), pseudo_element);
        context.animations.has_active_animations(&key)
    }

    fn has_css_transitions(
        &self,
        context: &SharedStyleContext,
        pseudo_element: Option<PseudoElement>,
    ) -> bool {
        let key = AnimationSetKey::new(TNode::opaque(&TElement::as_node(self)), pseudo_element);
        context.animations.has_active_transitions(&key)
    }

    fn animation_rule(
        &self,
        context: &SharedStyleContext,
    ) -> Option<Arc<Locked<PropertyDeclarationBlock>>> {
        let opaque = TNode::opaque(&TElement::as_node(self));
        context.animations.get_animation_declarations(
            &AnimationSetKey::new_for_non_pseudo(opaque),
            context.current_time_for_animations,
            self.guard(),
        )
    }

    fn transition_rule(
        &self,
        context: &SharedStyleContext,
    ) -> Option<Arc<Locked<PropertyDeclarationBlock>>> {
        let opaque = TNode::opaque(&TElement::as_node(self));
        context.animations.get_transition_declarations(
            &AnimationSetKey::new_for_non_pseudo(opaque),
            context.current_time_for_animations,
            self.guard(),
        )
    }

    fn shadow_root(&self) -> Option<<Self::ConcreteNode as TNode>::ConcreteShadowRoot> {
        None
    }

    fn containing_shadow(&self) -> Option<<Self::ConcreteNode as TNode>::ConcreteShadowRoot> {
        None
    }

    fn get_attr(&self, attr: &style::LocalName, ns: &style::Namespace) -> Option<String> {
        // TODO: case-insensitive matching for HTML-ns attrs
        self.attrs()?
            .iter()
            .find(|a| a.name.local == attr.0 && a.name.ns == ns.0)
            .map(|a| a.value.clone())
    }

    fn lang_attr(&self) -> Option<style::selector_parser::AttrValue> {
        None
    }

    fn match_element_lang(
        &self,
        _override_lang: Option<Option<style::selector_parser::AttrValue>>,
        _value: &style::selector_parser::Lang,
    ) -> bool {
        false
    }

    fn is_html_document_body_element(&self) -> bool {
        // Check node is a <body> element
        let is_body_element = self.data.is_element_with_tag_name(&local_name!("body"));

        // If it isn't then return early
        if !is_body_element {
            return false;
        }

        // If it is then check if it is a child of the root (<html>) element
        let root_node = TNode::owner_doc(self);
        let root_element = TDocument::as_node(&root_node)
            .first_element_child()
            .unwrap();
        root_element.children.contains(&self.id)
    }

    fn synthesize_presentational_hints_for_legacy_attributes<V>(
        &self,
        _visited_handling: VisitedHandlingMode,
        hints: &mut V,
    ) where
        V: Push<style::applicable_declarations::ApplicableDeclarationBlock>,
    {
        let Some(elem) = self.data.downcast_element() else {
            return;
        };

        let tag = &elem.name.local;

        let mut push_style = |decl: PropertyDeclaration| {
            hints.push(ApplicableDeclarationBlock::from_declarations(
                Arc::new(
                    self.guard()
                        .wrap(PropertyDeclarationBlock::with_one(decl, Importance::Normal)),
                ),
                CascadeLevel::new(CascadeOrigin::PresHints),
                LayerOrder::root(),
            ));
        };

        fn parse_color_attr(value: &str) -> Option<(u8, u8, u8, f32)> {
            if !value.starts_with('#') {
                return None;
            }

            let value = &value[1..];
            if value.len() == 3 {
                let r = u8::from_str_radix(&value[0..1], 16).ok()?;
                let g = u8::from_str_radix(&value[1..2], 16).ok()?;
                let b = u8::from_str_radix(&value[2..3], 16).ok()?;
                return Some((r, g, b, 1.0));
            }

            if value.len() == 6 {
                let r = u8::from_str_radix(&value[0..2], 16).ok()?;
                let g = u8::from_str_radix(&value[2..4], 16).ok()?;
                let b = u8::from_str_radix(&value[4..6], 16).ok()?;
                return Some((r, g, b, 1.0));
            }

            None
        }

        /// The HTML "rules for parsing dimension values" -- Stylo's
        /// implementation of them -- packaged as a specified
        /// `<length-percentage>`. `ignoring_zero` selects the spec's separate
        /// "maps to the dimension property (ignoring zero)" mapping, where a
        /// zero value is dropped rather than honoured.
        ///
        /// https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#rules-for-parsing-dimension-values
        fn parse_size_attr(
            value: &str,
            ignoring_zero: bool,
        ) -> Option<style::values::specified::LengthPercentage> {
            use style::servo::attr::{
                LengthOrPercentageOrAuto, parse_length, parse_nonzero_length,
            };
            use style::values::specified::{LengthPercentage, NoCalcLength};
            let parsed = if ignoring_zero {
                parse_nonzero_length(value)
            } else {
                parse_length(value)
            };
            match parsed {
                LengthOrPercentageOrAuto::Length(length) => Some(LengthPercentage::Length(
                    NoCalcLength::from_px(length.to_f32_px()),
                )),
                LengthOrPercentageOrAuto::Percentage(fraction) => Some(
                    LengthPercentage::Percentage(NoCalcPercentage::new(fraction)),
                ),
                LengthOrPercentageOrAuto::Auto => None,
            }
        }

        /// Parse the value of an SVG `width`/`height` presentation attribute.
        /// Unlike the legacy HTML dimension attributes, these accept any CSS
        /// <length-percentage> (e.g. `1em`), and a unitless number means user
        /// units, which map to CSS px.
        fn parse_svg_size_attr(value: &str) -> Option<style::values::specified::LengthPercentage> {
            use style::values::specified::{LengthPercentage, NoCalcLength};
            use style_traits::ParsingMode;

            let value = value.trim();
            if let Some(number) = value.strip_suffix('%') {
                let val: f32 = number.trim().parse().ok()?;
                return (val >= 0.0)
                    .then(|| LengthPercentage::Percentage(NoCalcPercentage::new(val / 100.0)));
            }

            // Split into number and unit: the unit is the trailing run of
            // ASCII alphabetic characters (this never eats into a scientific
            // exponent such as `1e3`, which ends in a digit).
            let number_len = value
                .trim_end_matches(|c: char| c.is_ascii_alphabetic())
                .len();
            let (number, unit) = value.split_at(number_len);
            let val: f32 = number.trim().parse().ok().filter(|v| *v >= 0.0)?;
            let length = if unit.is_empty() {
                NoCalcLength::from_px(val)
            } else {
                NoCalcLength::parse_dimension_with_flags(ParsingMode::DEFAULT, false, val, unit)
                    .ok()?
            };
            Some(LengthPercentage::Length(length))
        }

        // `<input type=image>` is the only input type that is replaced
        // content, and it is the only one that takes the embedded-content
        // presentational attributes. The type attribute is matched ASCII
        // case-insensitively, as attribute keywords always are.
        let is_image_input = *tag == local_name!("input")
            && elem.attrs().iter().any(|attr| {
                attr.name.local == local_name!("type") && attr.value.eq_ignore_ascii_case("image")
            });

        // https://html.spec.whatwg.org/multipage/#the-lang-and-xml:lang-attributes
        let accepts_lang = elem.name.ns == ns!(html) || elem.name.ns == ns!(svg);
        let mut lang = None;

        for attr in elem.attrs() {
            let name = &attr.name.local;
            let value = attr.value.as_str();

            if *name == local_name!("lang") {
                if attr.name.ns == ns!(xml) {
                    lang = Some(value);
                } else if attr.name.ns == ns!() && accepts_lang {
                    lang = lang.or(Some(value));
                }
            }

            if *name == local_name!("align") {
                use style::values::specified::TextAlign;
                let keyword = match value {
                    "left" => Some(StyloTextAlign::MozLeft),
                    "right" => Some(StyloTextAlign::MozRight),
                    "center" => Some(StyloTextAlign::MozCenter),
                    _ => None,
                };

                if let Some(keyword) = keyword {
                    push_style(PropertyDeclaration::TextAlign(TextAlign::Keyword(keyword)));
                }
            }

            // The width/height attributes on these elements map to the
            // corresponding dimension properties (percentages allowed):
            // https://html.spec.whatwg.org/multipage/rendering.html#dimRendering
            // https://html.spec.whatwg.org/multipage/rendering.html#attributes-for-embedded-content-and-images
            let is_width = *name == local_name!("width");
            let is_height = *name == local_name!("height");
            // The elements whose width/height attributes map to the dimension
            // properties. `input` only joins them as type=image, which is the
            // one replaced input type.
            let is_embedded = *tag == local_name!("iframe")
                || *tag == local_name!("embed")
                || *tag == local_name!("video")
                || *tag == local_name!("object")
                || *tag == local_name!("img")
                || *tag == local_name!("marquee")
                || is_image_input;
            let maps_to_dimension = if is_width {
                is_embedded
                    || *tag == local_name!("table")
                    || *tag == local_name!("col")
                    || *tag == local_name!("colgroup")
                    || *tag == local_name!("tr")
                    || *tag == local_name!("td")
                    || *tag == local_name!("th")
                    || *tag == local_name!("hr")
            } else if is_height {
                is_embedded
                    || *tag == local_name!("table")
                    || *tag == local_name!("thead")
                    || *tag == local_name!("tbody")
                    || *tag == local_name!("tfoot")
                    || *tag == local_name!("tr")
                    || *tag == local_name!("td")
                    || *tag == local_name!("th")
            } else {
                false
            };
            if maps_to_dimension {
                // Three of these use the "(ignoring zero)" variant of the
                // mapping, where a zero is dropped instead of honoured:
                // `table width`, and `td`/`th` in both axes.
                let is_cell = *tag == local_name!("td") || *tag == local_name!("th");
                let ignoring_zero = is_cell || (is_width && *tag == local_name!("table"));
                if let Some(size) = parse_size_attr(value, ignoring_zero) {
                    use style::values::generics::{NonNegative, length::Size};
                    let size = Size::LengthPercentage(NonNegative(size));
                    push_style(if is_width {
                        PropertyDeclaration::Width(size)
                    } else {
                        PropertyDeclaration::Height(size)
                    });
                }
            }

            // hspace/vspace map to the horizontal and vertical margins as
            // dimension properties. This is a *smaller* set than the one that
            // takes width/height: `iframe` and `video` take width and height
            // but not hspace/vspace, and html/rendering/unmapped-attributes
            // checks exactly that -- browsers have got it wrong before.
            let takes_spacing = *tag == local_name!("embed")
                || *tag == local_name!("img")
                || *tag == local_name!("object")
                || *tag == local_name!("marquee")
                || is_image_input;
            if takes_spacing {
                let is_hspace = *name == local_name!("hspace");
                let is_vspace = *name == local_name!("vspace");
                if is_hspace || is_vspace {
                    if let Some(size) = parse_size_attr(value, false) {
                        use style::values::generics::length::GenericMargin;
                        let margin = GenericMargin::LengthPercentage(size);
                        if is_hspace {
                            push_style(PropertyDeclaration::MarginLeft(margin.clone()));
                            push_style(PropertyDeclaration::MarginRight(margin));
                        } else {
                            push_style(PropertyDeclaration::MarginTop(margin.clone()));
                            push_style(PropertyDeclaration::MarginBottom(margin));
                        }
                    }
                }
            }

            // https://svgwg.org/svg2-draft/geometry.html#Sizing
            // The `width` and `height` attributes on an `<svg>` element are
            // presentation attributes that map to the CSS `width`/`height`
            // properties, so e.g. `width="1em"` must resolve against the
            // element's font-size like any other CSS length.
            if *tag == local_name!("svg")
                && (*name == local_name!("width") || *name == local_name!("height"))
            {
                if let Some(size) = parse_svg_size_attr(value) {
                    use style::values::generics::{NonNegative, length::Size};
                    let size = Size::LengthPercentage(NonNegative(size));
                    push_style(if *name == local_name!("width") {
                        PropertyDeclaration::Width(size)
                    } else {
                        PropertyDeclaration::Height(size)
                    });
                }
            }

            // The `border` attribute maps to the four border widths as a
            // pixel length, plus the four border styles as `solid` -- width
            // alone would compute back to zero against the default
            // border-style of `none`. It is only these three elements:
            // `embed`, `iframe`, `marquee` and non-image `input` all have a
            // `border` attribute that must stay unmapped.
            if *name == local_name!("border")
                && (*tag == local_name!("img") || *tag == local_name!("object") || is_image_input)
            {
                if let Ok(px) = style::servo::attr::parse_unsigned_integer(value.chars()) {
                    use style::values::specified::{BorderSideWidth, BorderStyle};
                    let width = BorderSideWidth::from_px(px as f32);
                    push_style(PropertyDeclaration::BorderTopWidth(width.clone()));
                    push_style(PropertyDeclaration::BorderRightWidth(width.clone()));
                    push_style(PropertyDeclaration::BorderBottomWidth(width.clone()));
                    push_style(PropertyDeclaration::BorderLeftWidth(width));
                    push_style(PropertyDeclaration::BorderTopStyle(BorderStyle::Solid));
                    push_style(PropertyDeclaration::BorderRightStyle(BorderStyle::Solid));
                    push_style(PropertyDeclaration::BorderBottomStyle(BorderStyle::Solid));
                    push_style(PropertyDeclaration::BorderLeftStyle(BorderStyle::Solid));
                }
            }

            // `body` carries four legacy margin attributes, as pixel lengths:
            // marginwidth and marginheight set both sides of an axis, and
            // leftmargin and topmargin set one side each.
            //
            // There is deliberately no `rightmargin` or `bottommargin`. They
            // look like the obvious counterparts to the two that exist, but
            // the spec does not define them and browsers ignore them in both
            // standards and quirks mode -- body-margin-3a/3b assert exactly
            // that.
            if *tag == local_name!("body") {
                // Matched as strings: these are not in the static atom set,
                // so `local_name!` will not compile for them.
                let sides: &[u8] = match &**name {
                    "marginwidth" => b"lr",
                    "marginheight" => b"tb",
                    "leftmargin" => b"l",
                    "topmargin" => b"t",
                    _ => b"",
                };
                if !sides.is_empty() {
                    if let Ok(px) = style::servo::attr::parse_unsigned_integer(value.chars()) {
                        use style::values::generics::length::GenericMargin;
                        use style::values::specified::{LengthPercentage, NoCalcLength};
                        let margin = GenericMargin::LengthPercentage(LengthPercentage::Length(
                            NoCalcLength::from_px(px as f32),
                        ));
                        for side in sides {
                            push_style(match side {
                                b'l' => PropertyDeclaration::MarginLeft(margin.clone()),
                                b'r' => PropertyDeclaration::MarginRight(margin.clone()),
                                b't' => PropertyDeclaration::MarginTop(margin.clone()),
                                b'b' => PropertyDeclaration::MarginBottom(margin.clone()),
                                _ => unreachable!("side table above only yields lrtb"),
                            });
                        }
                    }
                }
            }

            if *name == local_name!("bgcolor") {
                use style::values::specified::Color;
                if let Some((r, g, b, a)) = parse_color_attr(value) {
                    push_style(PropertyDeclaration::BackgroundColor(
                        Color::from_absolute_color(AbsoluteColor::srgb_legacy(r, g, b, a)),
                    ));
                }
            }

            if *name == local_name!("hidden") {
                use style::values::specified::Display;
                push_style(PropertyDeclaration::Display(Display::None));
            }
        }

        if let Some(lang) = lang {
            push_style(PropertyDeclaration::XLang(XLang(Atom::from(lang))));
        }
    }

    fn local_name(&self) -> &LocalName {
        &self.element_data().expect("Not an element").name.local
    }

    fn namespace(&self) -> &Namespace {
        &self.element_data().expect("Not an element").name.ns
    }

    fn query_container_size(
        &self,
        display: &style::values::specified::Display,
    ) -> euclid::default::Size2D<Option<app_units::Au>> {
        if !self.flags.is_in_document() || display.is_none() || display.is_contents() || display.is_inline_flow() {
            return Default::default();
        }
        // Stylo can call while holding this element's mutable style data for
        // a pseudo. Eligibility comes from its fresh originating-element style;
        // axes and content-box dimensions are updated after traversal instead.
        self.tree().container_query_sizes.lock().unwrap().entry(self.id).or_default().size
    }

    // lightningpanda: elements have no custom states (`:state()`), which the
    // relative selector invalidator asks about. This was `todo!()`.
    fn each_custom_state<F>(&self, _callback: F)
    where
        F: FnMut(&AtomIdent),
    {
    }

    fn has_selector_flags(&self, flags: ElementSelectorFlags) -> bool {
        self.selector_flags().get().contains(flags)
    }

    fn relative_selector_search_direction(&self) -> ElementSelectorFlags {
        let flags = self.selector_flags().get();
        if flags.contains(ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR_SIBLING)
        {
            ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR_SIBLING
        } else if flags.contains(ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR)
        {
            ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR
        } else if flags.contains(ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_SIBLING) {
            ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_SIBLING
        } else {
            ElementSelectorFlags::empty()
        }
    }

    fn compute_layout_damage(old: &ComputedValues, new: &ComputedValues) -> RestyleDamage {
        compute_layout_damage(old, new)
        // ALL_DAMAGE
    }

    // fn update_animations(
    //     &self,
    //     before_change_style: Option<Arc<ComputedValues>>,
    //     tasks: style::context::UpdateAnimationsTasks,
    // ) {
    //     todo!()
    // }

    // fn process_post_animation(&self, tasks: style::context::PostAnimationTasks) {
    //     todo!()
    // }

    // fn needs_transitions_update(
    //     &self,
    //     before_change_style: &ComputedValues,
    //     after_change_style: &ComputedValues,
    // ) -> bool {
    //     todo!()
    // }
}

pub struct Traverser<'a> {
    // dom: &'a Slab<Node>,
    parent: BlitzNode<'a>,
    child_index: usize,
}

impl<'a> Iterator for Traverser<'a> {
    type Item = BlitzNode<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let node_id = self.parent.children.get(self.child_index)?;
        let node = self.parent.with(*node_id);

        self.child_index += 1;

        Some(node)
    }
}

impl std::hash::Hash for BlitzNode<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_u64(self.id.as_u64())
    }
}

/// Handle custom painters like images for layouting
///
/// todo: actually implement this
pub struct RegisteredPaintersImpl;
impl RegisteredSpeculativePainters for RegisteredPaintersImpl {
    fn get(&self, _name: &Atom) -> Option<&dyn RegisteredSpeculativePainter> {
        None
    }
}

use style::traversal::recalc_style_at;

pub struct RecalcStyle<'a> {
    context: SharedStyleContext<'a>,
    /// Nodes whose `background-image`/`mask-image` layers need flushing to
    /// dedicated storage on the node (see `flush_image_layers_from_style`)
    /// because their style changed during this traversal.
    nodes_needing_style_image_flush: Mutex<Vec<NodeId>>,
}

impl<'a> RecalcStyle<'a> {
    pub fn new(context: SharedStyleContext<'a>) -> Self {
        RecalcStyle {
            context,
            nodes_needing_style_image_flush: Mutex::new(Vec::new()),
        }
    }
}

#[allow(unsafe_code)]
/// lightningpanda: what the style traversal must reach once Stylo's relative selector
/// invalidator invalidated `element` (as Gecko's `relative_selector_invalidated_at`): the
/// hints are set, the dirty-descendants bits lead to them.
fn relative_selector_invalidated_at(element: BlitzNode<'_>, result: &InvalidationResult) {
    if result.has_invalidated_siblings() {
        if let Some(parent) = <BlitzNode<'_> as TNode>::parent_element(&element) {
            Node::set_dirty_descendants(parent);
            parent.mark_ancestors_dirty();
        }
    } else if result.has_invalidated_descendants() {
        Node::set_dirty_descendants(element);
        element.mark_ancestors_dirty();
    } else if result.has_invalidated_self() {
        element.mark_ancestors_dirty();
    }
}

impl crate::BaseDocument {
    /// lightningpanda: the `:has()` anchors whose answer the insertion of `node_id` can change
    /// restyle. Stylo's relative selector invalidator (as Gecko's
    /// `MaybeInvalidateRelativeSelectorForInsertion`) looks the element's, and its
    /// descendants', tags, ids, classes and attributes up in the stylist's relative selector
    /// invalidation map, so an element no `:has()` argument can match invalidates nothing, and
    /// an anchor restyles itself and what the rest of its selector reaches, not its subtree.
    /// Called with the element linked in the document. It takes over its parent's and previous
    /// sibling's search-direction flags: a `:has()` search that reached them reaches it.
    pub(crate) fn invalidate_relative_selectors_for_insertion(&self, node_id: NodeId) {
        let node = &self.nodes[node_id];
        if !node.is_element() || !node.flags.is_in_document() {
            return;
        }
        let mut inherited = ElementSelectorFlags::empty();
        if let Some(parent) = <BlitzNode<'_> as TNode>::parent_element(&node) {
            inherited |= parent.relative_selector_search_direction();
        }
        if let Some(prev) = node.prev_sibling_element() {
            inherited |= prev.relative_selector_search_direction();
        }
        let operation = if node.next_sibling_element().is_none() {
            DomMutationOperation::Append
        } else {
            DomMutationOperation::Insert
        };
        self.invalidate_relative_selectors(node, inherited, operation);
    }

    /// lightningpanda: as [`Self::invalidate_relative_selectors_for_insertion`] for an element
    /// about to be removed (or moved away), called while it is still linked: the anchors are
    /// found through its parent and earlier siblings.
    pub(crate) fn invalidate_relative_selectors_for_removal(&self, node_id: NodeId) {
        let node = &self.nodes[node_id];
        if !node.is_element() || !node.flags.is_in_document() {
            return;
        }
        self.invalidate_relative_selectors(node, ElementSelectorFlags::empty(), DomMutationOperation::Remove);
    }

    fn invalidate_relative_selectors(
        &self,
        node: BlitzNode<'_>,
        inherited: ElementSelectorFlags,
        operation: DomMutationOperation,
    ) {
        let _layout_thread_scope = LayoutThreadScope::enter();
        fn invalidator<'dom>(
            element: BlitzNode<'dom>,
            sibling_traversal_map: SiblingTraversalMap<BlitzNode<'dom>>,
        ) -> RelativeSelectorInvalidator<'dom, 'dom, BlitzNode<'dom>> {
            RelativeSelectorInvalidator {
                element,
                quirks_mode: QuirksMode::NoQuirks,
                snapshot_table: None,
                invalidated: relative_selector_invalidated_at,
                sibling_traversal_map,
                _marker: std::marker::PhantomData,
            }
        }
        let prev = node.prev_sibling_element();
        let next = node.next_sibling_element();
        invalidator(node, SiblingTraversalMap::default()).invalidate_relative_selectors_for_dom_mutation(
            !node.children.is_empty(),
            &self.stylist,
            inherited,
            operation,
        );
        // The neighbours' sibling relations changed too (`:has(+ .a + .b)`). The next
        // sibling's previous one is the element's: what it was before an insertion, what it
        // is after a removal.
        if let Some(prev) = prev {
            invalidator(prev, SiblingTraversalMap::default()).invalidate_relative_selectors_for_dom_mutation(
                false,
                &self.stylist,
                ElementSelectorFlags::empty(),
                DomMutationOperation::SideEffectPrevSibling,
            );
        }
        if let Some(next) = next {
            let map = SiblingTraversalMap::new(next, prev, next.next_sibling_element());
            invalidator(next, map).invalidate_relative_selectors_for_dom_mutation(
                false,
                &self.stylist,
                ElementSelectorFlags::empty(),
                DomMutationOperation::SideEffectNextSibling,
            );
        }
    }
}

impl<'dom> DomTraversal<BlitzNode<'dom>> for RecalcStyle<'_> {
    fn process_preorder<F: FnMut(BlitzNode<'dom>)>(
        &self,
        context: &mut StyleContext<BlitzNode<'dom>>,
        node: BlitzNode<'dom>,
        note_child: F,
    ) {
        crate::resolve::phase_profile::restyled();
        if let Some(el) = node.as_element() {
            // let mut data = el.mutate_data().unwrap();
            let mut data = unsafe { el.ensure_data() };
            let before = context.thread_local.statistics.clone();
            let old_sticky = data.styles.get_primary().is_some_and(|s| crate::tree::is_sticky(s));
            recalc_style_at(self, context, el, &mut data, note_child);
            let new_sticky = data.styles.get_primary().is_some_and(|s| crate::tree::is_sticky(s));
            if old_sticky != new_sticky {
                el.tree().note_sticky_change(old_sticky, new_sticky);
            }
            let after = &context.thread_local.statistics;
            crate::resolve::phase_profile::style_stats([
                after.elements_styled - before.elements_styled,
                after.elements_matched - before.elements_matched,
                after.styles_shared - before.styles_shared,
                after.styles_reused - before.styles_reused,
            ]);

            sync_pseudo_element_styles(el, &data, &self.nodes_needing_style_image_flush);

            if !data.damage.is_empty() {
                // Mark the ancestor chain so that the damage propagation pass
                // visits this element.
                el.mark_damaged();

                if data
                    .styles
                    .get_primary()
                    .is_some_and(|style| needs_style_image_flush(el, style))
                {
                    self.nodes_needing_style_image_flush
                        .lock()
                        .unwrap()
                        .push(el.id);
                }
            }

            // Gets set later on
            el.unset_dirty_descendants();
        }
    }

    #[inline]
    fn needs_postorder_traversal() -> bool {
        false
    }

    fn process_postorder(
        &self,
        _style_context: &mut StyleContext<BlitzNode<'dom>>,
        _node: BlitzNode<'dom>,
    ) {
        panic!("this should never be called")
    }

    #[inline]
    fn shared_context(&self) -> &SharedStyleContext<'_> {
        &self.context
    }
}

/// Flush updated pseudo-element (`::before`/`::after`) styles from the owning
/// element's freshly-computed stylo data to the pseudo-element's anonymous node.
///
/// Pseudo-element styles are normally flushed to the pseudo-element's node
/// during box construction (see `flush_pseudo_elements`), but in incremental
/// mode box construction only runs for nodes with construction damage.
/// Pseudo-element style changes which don't require reconstruction (e.g.
/// animations/transitions of repaint- or relayout-only properties) must still
/// be flushed to the pseudo-element's node - along with the damage they imply -
/// so that layout and paint see the new style.
///
/// This runs during the style traversal, immediately after the owning element
/// has been restyled, because that is where the old and new pseudo styles can
/// be diffed precisely (the pseudo node's stored primary style is the "old"
/// style; the owner's just-computed pseudo styles are the "new" ones).
///
/// SAFETY: the style traversal has exclusive access to the tree, and each
/// pseudo-element node is only ever accessed from its owning element's
/// `process_preorder` (pseudo nodes are not part of the DOM child list and are
/// not themselves visited by the traversal), so mutating the pseudo node's
/// stylo data here cannot race with any other traversal thread.
#[allow(unsafe_code)]
fn sync_pseudo_element_styles(
    el: &Node,
    data: &style::data::ElementData,
    nodes_needing_style_image_flush: &Mutex<Vec<NodeId>>,
) {
    let before_node_id = el.before();
    let after_node_id = el.after();
    if before_node_id.is_none() && after_node_id.is_none() {
        return;
    }

    // Note: yes these are kinda backwards (see `flush_pseudo_elements`)
    let pseudos = data.styles.pseudos.as_array();
    let before_style = pseudos[1].clone();
    let after_style = pseudos[0].clone();

    // Creation and removal of pseudo-elements is handled during box construction
    // (Stylo generates construction damage for those cases), so only the case
    // where the pseudo-element both was and remains present is handled here.
    for (pe_node_id, pe_style) in [(before_node_id, before_style), (after_node_id, after_style)] {
        let (Some(pe_node_id), Some(pe_style)) = (pe_node_id, pe_style) else {
            continue;
        };
        let pe_node = el.with(pe_node_id);
        let Some(stylo_data) = pe_node.try_stylo_element_data() else {
            continue;
        };
        let mut pe_data = match unsafe { stylo_data.unsafe_stylo_only_mut() } {
            Some(data) => data,
            None => continue,
        };
        let Some(old_style) = pe_data.styles.primary.clone() else {
            continue;
        };
        if std::ptr::eq(&*old_style, &*pe_style) {
            continue;
        }

        let diff = RestyleDamage::compute_style_difference::<&Node>(&old_style, &pe_style);
        if !diff.damage.is_empty() {
            pe_data.damage.insert(diff.damage);
            pe_node.mark_damaged();

            if needs_style_image_flush(pe_node, &pe_style) {
                nodes_needing_style_image_flush
                    .lock()
                    .unwrap()
                    .push(pe_node_id);
            }
        }
        pe_node.tree().note_primary_style_change(Some(&*old_style), Some(&*pe_style));
        pe_data.styles.primary = Some(pe_style);
        pe_data.set_restyled();
    }
}

/// Whether a node's `background-image`/`mask-image` layers need flushing to
/// dedicated storage on the node: the URLs referenced by the new style differ
/// from the image data currently stored on the node.
fn needs_style_image_flush(node: &Node, style: &ComputedValues) -> bool {
    use crate::node::ImageResourceData;
    use style::url::ComputedUrl;
    use style::values::computed::image::Image;

    fn out_of_sync(style_images: &[Image], stored: &[Option<ImageResourceData>]) -> bool {
        if style_images.len() != stored.len() {
            // Differing lengths only matter if either side references an image
            // (a style list of `none` layers and empty storage are in sync).
            return style_images
                .iter()
                .any(|image| matches!(image, Image::Url(_)))
                || stored.iter().any(Option::is_some);
        }
        std::iter::zip(style_images, stored).any(|(style_image, stored)| {
            match (style_image, stored) {
                (Image::Url(ComputedUrl::Valid(url)), Some(data)) => **url != *data.url,
                (Image::Url(ComputedUrl::Valid(_)), None) => true,
                (_, Some(_)) => true,
                (_, None) => false,
            }
        })
    }

    node.data.downcast_element().is_some_and(|el| {
        out_of_sync(
            &style.get_background().background_image.0,
            &el.background_images,
        ) || out_of_sync(&style.get_svg().mask_image.0, &el.mask_images)
    })
}

#[test]
fn assert_size_of_equals() {
    // use std::mem;

    // fn assert_layout<E>() {
    //     assert_eq!(
    //         mem::size_of::<SharingCache<E>>(),
    //         mem::size_of::<TypelessSharingCache>()
    //     );
    //     assert_eq!(
    //         mem::align_of::<SharingCache<E>>(),
    //         mem::align_of::<TypelessSharingCache>()
    //     );
    // }

    // let size = mem::size_of::<StyleSharingCandidate<BlitzNode>>();
    // dbg!(size);
}

#[test]
fn parse_inline() {
    // let attrs = style::attr::AttrValue::from_serialized_tokenlist(
    //     r#"visibility: hidden; left: 1306.5px; top: 50px; display: none;"#.to_string(),
    // );

    // let val = CSSInlineStyleDeclaration();
}

/// lightningpanda: Stylo skips the eager `::before`/`::after` match and cascade when no rule
/// that can generate a box matches (`Stylist::may_generate_box_for_pseudo`). What generates,
/// and the styles Stylo keeps, must not change.
#[cfg(test)]
mod relative_selector_invalidation_tests {
    use super::*;
    use crate::{Attribute, BaseDocument, DocumentConfig, DocumentMutator, qual_name};
    use blitz_traits::shell::{ColorScheme, Viewport};

    /// `<html><head><style>{css}</style></head><body>`, resolved; the body's id. `tag` keeps
    /// the sheet text apart from other tests' (author sheets are shared per thread by text).
    fn make_doc(css: &str, tag: &str) -> (BaseDocument, NodeId) {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        let root_id = doc.root_node().id;
        let mut m = doc.mutate();
        let html = m.create_element(qual_name!("html"), vec![]);
        let head = m.create_element(qual_name!("head"), vec![]);
        let style = m.create_element(qual_name!("style"), vec![]);
        let text = m.create_text_node(&format!("/* {tag} */ {css}"));
        m.append_children(style, &[text]);
        m.append_children(head, &[style]);
        let body = m.create_element(qual_name!("body"), vec![]);
        m.append_children(html, &[head, body]);
        m.append_children(root_id, &[html]);
        drop(m);
        doc.resolve(0.0);
        (doc, body)
    }

    fn div(m: &mut DocumentMutator<'_>, class: &str) -> NodeId {
        m.create_element(qual_name!("div"), vec![Attribute { name: qual_name!("class"), value: class.to_string() }])
    }

    /// The computed `order` of `id`: 7 where the test's `:has()` rule matched, else 0.
    fn order(doc: &BaseDocument, id: NodeId) -> i32 {
        doc.nodes[id].primary_styles().unwrap().get_position().order
    }

    /// The restyle hint waiting on `id` for the next resolve.
    fn hint(doc: &BaseDocument, id: NodeId) -> RestyleHint {
        doc.nodes[id].stylo_element_data().get().unwrap().hint
    }

    #[test]
    fn subject_anchor_follows_insertions_and_removals() {
        let (mut d, body) = make_doc(".a:has(.x) { order: 7 }", "has-subject");
        let mut m = d.mutate();
        let (a, p, b) = (div(&mut m, "a"), div(&mut m, "p"), div(&mut m, "b"));
        m.append_children(a, &[p]);
        m.append_children(body, &[a, b]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 0);

        // An element no `:has()` argument can match invalidates no anchor.
        let mut m = d.mutate();
        let y = div(&mut m, "y");
        m.append_children(p, &[y]);
        drop(m);
        assert!(hint(&d, a).is_empty(), "nothing matches .y");
        d.resolve(0.0);
        assert_eq!(order(&d, a), 0);

        let mut m = d.mutate();
        let x = div(&mut m, "x");
        m.append_children(p, &[x]);
        drop(m);
        assert!(hint(&d, a).contains(RestyleHint::RESTYLE_SELF), "the anchor restyles itself");
        assert!(hint(&d, p).is_empty(), "not the rest of its subtree");
        assert!(hint(&d, b).is_empty(), "nor a sibling that anchors nothing");
        d.resolve(0.0);
        assert_eq!(order(&d, a), 7);

        let mut m = d.mutate();
        m.remove_node(x);
        drop(m);
        assert!(hint(&d, a).contains(RestyleHint::RESTYLE_SELF));
        d.resolve(0.0);
        assert_eq!(order(&d, a), 0);
    }

    #[test]
    fn child_combinator_argument_needs_a_direct_child() {
        let (mut d, body) = make_doc(".a:has(> .x) { order: 7 }", "has-child");
        let mut m = d.mutate();
        let (a, p) = (div(&mut m, "a"), div(&mut m, "p"));
        m.append_children(a, &[p]);
        m.append_children(body, &[a]);
        drop(m);
        d.resolve(0.0);
        let mut m = d.mutate();
        let x = div(&mut m, "x");
        m.append_children(p, &[x]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 0, "a grandchild");
        let mut m = d.mutate();
        let x2 = div(&mut m, "x");
        m.append_children(a, &[x2]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 7, "a child");
        let mut m = d.mutate();
        m.remove_node(x2);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 0);
    }

    #[test]
    fn next_sibling_argument_follows_what_comes_between() {
        let (mut d, body) = make_doc(".a:has(+ .x) { order: 7 }", "has-next-sibling");
        let mut m = d.mutate();
        let (a, q) = (div(&mut m, "a"), div(&mut m, "q"));
        m.append_children(body, &[a, q]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 0);
        let mut m = d.mutate();
        let x = div(&mut m, "x");
        m.insert_nodes_before(q, &[x]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 7, ".x right after .a");
        let mut m = d.mutate();
        let z = div(&mut m, "z");
        m.insert_nodes_before(x, &[z]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 0, ".z came between");
        let mut m = d.mutate();
        m.remove_node(z);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 7, ".z went away");
    }

    #[test]
    fn later_sibling_subtree_argument_sees_a_deep_insertion() {
        let (mut d, body) = make_doc(".a:has(~ .s .x) { order: 7 }", "has-later-sibling");
        let mut m = d.mutate();
        let (a, s, p) = (div(&mut m, "a"), div(&mut m, "s"), div(&mut m, "p"));
        m.append_children(s, &[p]);
        m.append_children(body, &[a, s]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 0);
        let mut m = d.mutate();
        let x = div(&mut m, "x");
        m.append_children(p, &[x]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 7);
        let mut m = d.mutate();
        m.remove_node(x);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 0);
    }

    #[test]
    fn non_subject_anchor_restyles_what_its_selector_reaches() {
        let (mut d, body) = make_doc(".a:has(.x) .d { order: 7 }", "has-non-subject");
        let mut m = d.mutate();
        let (a, dd, p) = (div(&mut m, "a"), div(&mut m, "d"), div(&mut m, "p"));
        m.append_children(a, &[dd, p]);
        m.append_children(body, &[a]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, dd), 0);
        let mut m = d.mutate();
        let x = div(&mut m, "x");
        m.append_children(p, &[x]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, dd), 7);
        let mut m = d.mutate();
        m.remove_node(x);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, dd), 0);
    }

    #[test]
    fn an_inserted_subtree_is_searched() {
        let (mut d, body) = make_doc(".a:has(.x) { order: 7 }", "has-subtree");
        let mut m = d.mutate();
        let a = div(&mut m, "a");
        m.append_children(body, &[a]);
        drop(m);
        d.resolve(0.0);
        let mut m = d.mutate();
        let (w, v, x) = (div(&mut m, "w"), div(&mut m, "v"), div(&mut m, "x"));
        m.append_children(v, &[x]);
        m.append_children(w, &[v]);
        m.append_children(a, &[w]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 7, ".x deep in the inserted subtree");
        let mut m = d.mutate();
        m.remove_node(w);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 0);
    }

    #[test]
    fn a_moved_argument_updates_both_anchors() {
        let (mut d, body) = make_doc(".a:has(.x) { order: 7 }", "has-move");
        let mut m = d.mutate();
        let (a1, a2, x) = (div(&mut m, "a"), div(&mut m, "a"), div(&mut m, "x"));
        m.append_children(a1, &[x]);
        m.append_children(body, &[a1, a2]);
        drop(m);
        d.resolve(0.0);
        assert_eq!((order(&d, a1), order(&d, a2)), (7, 0));
        let mut m = d.mutate();
        m.append_children(a2, &[x]);
        drop(m);
        d.resolve(0.0);
        assert_eq!((order(&d, a1), order(&d, a2)), (0, 7));
    }
}

#[cfg(test)]
mod box_generating_pseudo_tests {
    use super::*;
    use crate::{Attribute, BaseDocument, DocumentConfig, QualName, UserAgentStylesheet, qual_name};
    use blitz_traits::shell::{ColorScheme, Viewport};
    use style::stylist::RuleInclusion;
    use style::traversal::resolve_style;

    struct Doc {
        doc: BaseDocument,
        style: NodeId,
        body: NodeId,
    }

    /// `<html><head><style>{css}</style></head><body>` with a `div` per `(id class, class)`.
    /// A per-test `tag` keeps the sheet text apart from other tests' (author sheets are shared
    /// per thread by text, and one test edits its sheet through the CSSOM).
    fn make_doc(ua: Option<&str>, css: &str, tag: &str, divs: &[(&str, &str)]) -> (Doc, Vec<NodeId>) {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        if let Some(ua) = ua {
            doc.add_user_agent_stylesheet(UserAgentStylesheet::parse(ua));
        }
        let root_id = doc.root_node().id;
        let attr = |name, value: &str| Attribute { name, value: value.to_string() };
        let mut ids = Vec::new();
        let mut m = doc.mutate();
        let html = m.create_element(qual_name!("html"), vec![]);
        let head = m.create_element(qual_name!("head"), vec![]);
        let style = m.create_element(qual_name!("style"), vec![]);
        let text = m.create_text_node(&format!("/* {tag} */ {css}"));
        m.append_children(style, &[text]);
        m.append_children(head, &[style]);
        let body = m.create_element(qual_name!("body"), vec![]);
        for (tag, class) in divs {
            let local = LocalName::from(*tag);
            let name = QualName::new(None, ns!(html), local);
            let el = m.create_element(name, vec![attr(qual_name!("class"), class)]);
            m.append_children(body, &[el]);
            ids.push(el);
        }
        m.append_children(html, &[head, body]);
        m.append_children(root_id, &[html]);
        drop(m);
        doc.resolve(0.0);
        (Doc { doc, style, body }, ids)
    }

    /// The eager (`before`, `after`) styles Stylo kept for `id`.
    fn kept(doc: &BaseDocument, id: NodeId) -> (bool, bool) {
        let data = doc.nodes[id].stylo_element_data().get().unwrap();
        let pseudos = data.styles.pseudos.as_array();
        (pseudos[1].is_some(), pseudos[0].is_some())
    }

    /// The (`::before`, `::after`) boxes of `id`.
    fn boxes(doc: &BaseDocument, id: NodeId) -> (bool, bool) {
        let node = &doc.nodes[id];
        (node.before().is_some(), node.after().is_some())
    }

    #[test]
    fn non_generating_rules_keep_no_pseudo_style() {
        let css = "*::before, *::after { box-sizing: border-box; color: red } \
                   .x::before { content: '' } .y::after { content: 'y' } \
                   .d::before { display: inherit } .v::before { content: var(--c) } \
                   .all::after { all: inherit } .an::before { animation-name: a }";
        let (d, ids) = make_doc(
            Some("q:before { content: open-quote } q:after { content: close-quote }"),
            css,
            "non-generating",
            &[("div", ""), ("div", "x"), ("div", "y"), ("q", ""), ("div", "d"), ("div", "v"), ("div", "all"), ("div", "an")],
        );
        let doc = &d.doc;
        assert_eq!(kept(doc, ids[0]), (false, false), "box-sizing and color alone generate nothing");
        assert_eq!(boxes(doc, ids[0]), (false, false));
        assert_eq!(boxes(doc, ids[1]), (true, false), ".x::before");
        assert_eq!(boxes(doc, ids[2]), (false, true), ".y::after");
        assert_eq!(boxes(doc, ids[3]), (true, true), "q's UA quotes");
        // `display: inherit` and `content: var()` keep the style as upstream does
        // (`DISPLAY_OR_CONTENT_DEPEND_ON_INHERITED_STYLE`); `content` stays `normal`, no box.
        assert_eq!(kept(doc, ids[4]), (true, false), "display: inherit");
        assert_eq!(boxes(doc, ids[4]), (false, false));
        assert_eq!(kept(doc, ids[5]), (true, false), "content: var(--c)");
        assert_eq!(kept(doc, ids[6]), (false, true), "all: inherit");
        assert_eq!(kept(doc, ids[7]), (false, false), "animation-name alone: content is normal");
    }

    /// Which declaration blocks the index files: a value that cannot make the box appear
    /// (`content: normal | none`, `animation-name: none`) keeps a rule out of it, so the UA
    /// `*::before, *::after { animation: none !important }` does not match every element.
    #[test]
    fn index_ignores_declarations_that_cannot_generate_a_box() {
        use style::properties::declaration_block::parse_style_attribute;
        use style::stylesheets::CssRuleType;
        use style::stylist::declarations_may_generate_pseudo_box;
        let (d, _) = make_doc(None, "", "index-decls", &[]);
        let url = d.doc.url.url_extra_data();
        let generates = |css: &str| {
            let block = parse_style_attribute(css, &url, None, QuirksMode::NoQuirks, CssRuleType::Style);
            declarations_may_generate_pseudo_box(&block)
        };
        assert!(!generates("transition: none; animation: none"));
        assert!(!generates("animation-name: none, none"));
        assert!(!generates("content: normal"));
        assert!(!generates("content: none; color: red"));
        assert!(generates("animation-name: a"));
        assert!(generates("animation-name: none, a"));
        assert!(generates("animation: var(--a)"));
        assert!(generates("animation-name: inherit"));
        assert!(generates("content: ''"));
        assert!(generates("content: var(--c)"));
        assert!(generates("content: inherit"));
        assert!(generates("display: none"));
        assert!(generates("all: unset"));
    }

    /// `PseudoElementResolution::Force` (out-of-document styles) still resolves every eager
    /// pseudo-element any rule matches.
    #[test]
    fn forced_resolution_keeps_non_generating_pseudo_styles() {
        let (mut d, ids) = make_doc(
            None,
            "div::before { color: red } .h { display: none }",
            "forced",
            &[("div", ""), ("div", "h")],
        );
        assert_eq!(kept(&d.doc, ids[0]), (false, false));
        // An element in a `display: none` subtree is left unstyled, as `resolve_style` wants.
        let mut m = d.doc.mutate();
        let hidden = m.create_element(qual_name!("div"), vec![]);
        m.append_children(ids[1], &[hidden]);
        drop(m);
        d.doc.resolve(0.0);
        let doc = &d.doc;

        let _layout_thread_scope = LayoutThreadScope::enter();
        let guard = &doc.guard;
        let guards = StylesheetGuards { author: &guard.read(), ua_or_user: &guard.read() };
        let shared = SharedStyleContext {
            traversal_flags: TraversalFlags::empty(),
            stylist: &doc.stylist,
            options: GLOBAL_STYLE_DATA.options.clone(),
            guards,
            visited_styles_enabled: false,
            animations: doc.animations.clone(),
            current_time_for_animations: 0.0,
            snapshot_map: &doc.snapshots,
            registered_speculative_painters: &RegisteredPaintersImpl,
        };
        let mut thread_local = ThreadLocalStyleContext::new();
        let mut context = StyleContext { shared: &shared, thread_local: &mut thread_local };
        let node = &doc.nodes[hidden];
        let styles = resolve_style(&mut context, node, RuleInclusion::All, None, None);
        assert!(styles.pseudos.get(&PseudoElement::Before).is_some(), "forced ::before");
        assert!(styles.pseudos.get(&PseudoElement::After).is_none(), "nothing matches ::after");
    }

    /// The index is rebuilt with the rules: a CSSOM edit that adds `content` makes the box
    /// appear. This fails if `RuleChangeKind::StyleRuleDeclarations` stops rebuilding the
    /// cascade data (stylesheet_set.rs).
    #[test]
    fn cssom_declaration_edit_makes_the_pseudo_appear() {
        let (mut d, ids) = make_doc(None, ".c::before { color: red }", "cssom", &[("div", "c")]);
        assert_eq!(boxes(&d.doc, ids[0]), (false, false));
        d.doc
            .stylesheet_rule_style_set_property(d.style, &[0], "content", "\"a\"", false)
            .unwrap();
        d.doc.resolve(0.0);
        assert_eq!(boxes(&d.doc, ids[0]), (true, false), "content set through the CSSOM");
        d.doc.stylesheet_rule_style_remove_property(d.style, &[0], "content").unwrap();
        d.doc.resolve(0.0);
        assert_eq!(boxes(&d.doc, ids[0]), (false, false), "and removed");
    }

    /// Selector flags from the index's own match drive sibling invalidation.
    #[test]
    fn last_child_after_follows_appends_and_removals() {
        let (mut d, ids) = make_doc(
            None,
            ".a:last-child::after { content: '' } .a::after { color: red }",
            "last-child",
            &[("div", "a"), ("div", "a")],
        );
        assert_eq!(boxes(&d.doc, ids[0]), (false, false));
        assert_eq!(boxes(&d.doc, ids[1]), (false, true));

        let body = d.body;
        let mut m = d.doc.mutate();
        let attr = Attribute { name: qual_name!("class"), value: "a".into() };
        let added = m.create_element(qual_name!("div"), vec![attr]);
        m.append_children(body, &[added]);
        drop(m);
        d.doc.resolve(0.0);
        assert_eq!(boxes(&d.doc, ids[1]), (false, false), "no longer the last child");
        assert_eq!(boxes(&d.doc, added), (false, true), "the new last child");

        let mut m = d.doc.mutate();
        m.remove_node(added);
        drop(m);
        d.doc.resolve(0.0);
        assert_eq!(boxes(&d.doc, ids[1]), (false, true), "the last child again");
    }
}
