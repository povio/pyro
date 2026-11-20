//! Enable the dom to participate in styling by servo
//!

use stylo_taffy::CopyStyleGetters;
use blitz_traits::node_id::NodeId;
use std::ptr::NonNull;
use std::sync::Mutex;
use std::sync::atomic::Ordering;

use crate::StyleThreading;
use crate::layout::damage::compute_layout_damage;
use crate::node::Node;
use crate::node::NodeData;
use crate::node::LayoutStyles;
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
use style::rule_tree::StyleSourceBorrow;
use crate::node::ElementData;
use style::bloom::each_relevant_element_hash;
use style::color::AbsoluteColor;
use style::data::{ElementDataMut, ElementDataRef};
use style::global_style_data::STYLE_THREAD_POOL;

/// Serialises `Parallel` style traversals across documents. Stylo's per-worker
/// caches (`StyleSharingCache`, `StyleBloom`) live in native thread-locals that
/// a traversal borrows mutably for its whole duration, so two traversals in
/// flight on the shared pool at once panic with `already mutably borrowed`
/// (<https://github.com/DioxusLabs/blitz/issues/430>).
static PARALLEL_TRAVERSAL_LOCK: Mutex<()> = Mutex::new(());
use style::invalidation::element::restyle_hints::RestyleHint;
use style::properties::ComputedValues;
use style::properties::{Importance, PropertyDeclaration};
use style::rule_tree::CascadeLevel;
use style::rule_tree::CascadeOrigin;
use style::invalidation::element::invalidation_map::TSStateForInvalidation;
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

pub(crate) struct LayoutThreadScope;

impl LayoutThreadScope {
    pub(crate) fn enter() -> Self {
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
    /// `:state(name)`: pyro's custom states attribute (names
    /// separated by U+001F).
    fn has_lp_custom_state(&self, state: &str) -> bool {
        self.element_data().is_some_and(|e| {
            e.attrs.iter().any(|a| {
                &*a.name.ns == "urn:x-pyro:state"
                    && &*a.name.local == "custom-states"
                    && a.value.split('\u{1f}').any(|s| s == state)
            })
        })
    }
}

impl crate::document::BaseDocument {
    pub fn resolve_stylist(&mut self, now: f64) {
        let _layout_thread_scope = LayoutThreadScope::enter();

        // pyro: shadow roots' sheets (`crate::shadow`).
        self.flush_shadow_styles();

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
            visited_styles_enabled: true,
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
        let mut style_changes = Vec::new();
        if token.should_traverse() {
            // Style the elements, resolving their data
            let mut traverser = RecalcStyle::new(context);
            traverser.track_style_changes = self.track_style_changes;
            traverser.track_counters = self.generated_counters && !self.counters_dirty;
            // `Sequential` bypasses Stylo's global pool. See `StyleThreading`.
            let parallel = matches!(self.style_threading, StyleThreading::Parallel);
            let _traversal_lock = parallel.then(|| {
                PARALLEL_TRAVERSAL_LOCK
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
            });
            let pool_guard = parallel.then(|| STYLE_THREAD_POOL.pool());
            let rayon_pool = pool_guard.as_ref().and_then(|g| g.as_ref());
            style::driver::traverse_dom(&traverser, token, rayon_pool);
            nodes_needing_style_image_flush =
                std::mem::take(traverser.nodes_needing_style_image_flush.get_mut().unwrap());
            self.sticky_nodes
                .extend(std::mem::take(traverser.sticky_nodes.get_mut().unwrap()));
            style_changes = std::mem::take(traverser.style_changes.get_mut().unwrap());
            // Sticky: set once an element is styled with a generated
            // `::before` / `::after`, never cleared.
            if traverser.generated_before_after.into_inner() {
                self.generated_before_after = true;
            }
            // pyro: sticky as well (`layout::first_letter`, `layout::counters`).
            if traverser.first_letter_styles.into_inner() {
                self.first_letter_styles = true;
            }
            if traverser.text_box_trim_styles.into_inner() {
                self.text_box_trim_styles = true;
            }
            if traverser.scroll_snap_styles.into_inner() {
                self.scroll_snap_styles = true;
            }
            if traverser.column_span_styles.into_inner() {
                self.column_span_styles = true;
            }
            if traverser.scroll_marker_groups.into_inner() {
                self.scroll_marker_groups = true;
            }
            // pyro: a restyle may change which scroll pseudo-elements generate boxes.
            self.scroll_markers_dirty = true;
            if traverser.generated_counters.into_inner() && !self.generated_counters {
                self.generated_counters = true;
                self.counters_dirty = true;
            }
            if traverser.counters_changed.into_inner() {
                self.counters_dirty = true;
            }
            self.first_letter_changed
                .extend(std::mem::take(traverser.first_letter_changed.get_mut().unwrap()));
            self.marker_items_restyled
                .extend(std::mem::take(traverser.marker_items_restyled.get_mut().unwrap()));
            if traverser.first_line_styles.into_inner() {
                self.first_line_styles = true;
            }
            self.first_line_changed.extend(std::mem::take(traverser.first_line_changed.get_mut().unwrap()));
        }
        // pyro: restyled nodes that stopped being sticky, and
        // removed ones, leave the set (only nodes that were sticky are in it).
        if !self.sticky_nodes.is_empty() {
            let nodes = &self.nodes;
            self.sticky_nodes
                .retain(|id| nodes.get(*id).is_some_and(|node| is_sticky(node)));
        }
        self.pending_style_image_nodes
            .extend(nodes_needing_style_image_flush);
        if !style_changes.is_empty() {
            crate::document::note_style_changes(&mut self.style_changes, &mut self.style_change_index, style_changes);
        }

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
        let mut finished = Vec::new();
        sets.retain(|key, state| {
            let keep = !state.is_empty();
            if !keep {
                finished.push(key.node);
            }
            keep
        });
        // Clear `may_have_animations` on elements whose last set (including
        // their pseudo-elements' sets) was just removed.
        for node in finished {
            if sets.keys().any(|key| key.node == node) {
                continue;
            }
            let node_id = NodeId::from_u64(node.id() as u64);
            if let Some(data) = self.nodes.get(node_id).and_then(|node| node.element_data()) {
                data.may_have_animations.store(false, Ordering::Relaxed);
            }
        }
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
        Some(self.resolve_undisplayed_styles(node_id, None)?.primary().clone())
    }

    /// pyro: [`Self::resolve_undisplayed_style`] with caches the caller keeps
    /// while the document does not change: each undisplayed ancestor's style
    /// (an element inherits its parent's instead of every read matching the
    /// whole chain from the `display: none` root again) and the selector
    /// caches (`:has()` filters and results, nth-index), built once instead
    /// of walking the subtree again for every element read. The caller must
    /// clear `caches` whenever the document's nodes, states or sheets change.
    pub fn resolve_undisplayed_style_cached(
        &self,
        node_id: NodeId,
        caches: &mut UndisplayedCaches,
    ) -> Option<Arc<ComputedValues>> {
        let node = self.nodes.get(node_id)?;
        if !node.is_element() || !node.flags.is_in_document() {
            return None;
        }
        if let Some(style) = caches.styles.get(&selectors::OpaqueElement::new(node)) {
            return Some(style.clone());
        }
        Some(self.resolve_undisplayed_styles(node_id, Some(caches))?.primary().clone())
    }

    /// pyro: `false` while no style traversal has kept a
    /// `::before` / `::after` style for any element, so
    /// [`Self::pseudo_element_style`] with `generated_only` answers `None`
    /// for every element (it may stay `true` after they are gone).
    pub fn may_have_generated_before_after(&self) -> bool {
        self.generated_before_after
    }

    /// pyro: the style `getComputedStyle(element, pseudo)` reads
    /// for the element `node_id`: an eagerly cascaded pseudo-element's
    /// (`::before`, `::after`, `::first-letter`, `::selection`) as the
    /// traversal kept it (resolved on demand inside `display: none`), a
    /// lazily cascaded one's computed from its rules, and otherwise (no rule
    /// generates it) a style with no rules of its own, inheriting from the
    /// element. `None` for non-elements and elements outside the document.
    /// With `generated_only`, only a stored eagerly cascaded style (a
    /// generated `::before` / `::after` of a rendered element), and no
    /// work otherwise.
    /// pyro: the `::backdrop` style with any running animation of it (`Element.animate` with
    /// `pseudoElement: '::backdrop'`, `ElementData::sampled_backdrop`) cascaded on it.
    pub fn backdrop_style(&self, node_id: NodeId) -> Option<Arc<ComputedValues>> {
        let base = self.pseudo_element_style(node_id, &style::selector_parser::PseudoElement::Backdrop, false)?;
        let Some(block) = self.nodes.get(node_id)?.element_data().and_then(|e| e.sampled_backdrop.clone()) else {
            return Some(base);
        };
        let read_guard = self.guard.read();
        let guards = style::shared_lock::StylesheetGuards::same(&read_guard);
        Some(self.stylist.compute_for_declarations::<&crate::Node>(&guards, &base, block))
    }

    pub fn pseudo_element_style(
        &self,
        node_id: NodeId,
        pseudo: &style::selector_parser::PseudoElement,
        generated_only: bool,
    ) -> Option<Arc<ComputedValues>> {
        let node = self.nodes.get(node_id)?;
        if !node.is_element() || !node.flags.is_in_document() {
            return None;
        }
        let stored = node.try_stylo_element_data().and_then(|d| d.get()).and_then(|d| {
            let primary = d.styles.get_primary()?.clone();
            let eager = if pseudo.is_eager() { d.styles.pseudos.get(pseudo).cloned() } else { None };
            Some((primary, eager))
        });
        if generated_only {
            return stored.and_then(|(_, eager)| eager);
        }
        let (primary, eager) = match stored {
            Some(styles) => styles,
            None => {
                let styles = self.resolve_undisplayed_styles(node_id, None)?;
                let eager = if pseudo.is_eager() { styles.pseudos.get(pseudo).cloned() } else { None };
                (styles.primary().clone(), eager)
            }
        };
        if eager.is_some() {
            return eager;
        }
        if pseudo.is_before_or_after() {
            // The traversal drops a `::before` / `::after` that generates no
            // box (`content: none`), but its rules still apply to its
            // computed style (Chromium): match them against this element
            // alone. With no `::before` / `::after` rules this is an empty
            // selector-map lookup.
            use selectors::context::{MatchingForInvalidation, NeedsSelectorFlags, SelectorCaches};
            use selectors::matching::MatchingMode;
            let _layout_thread_scope = LayoutThreadScope::enter();
            let guard = &self.guard;
            let guards = StylesheetGuards {
                author: &guard.read(),
                ua_or_user: &guard.read(),
            };
            let mut caches = SelectorCaches::default();
            let mut matching = MatchingContext::<SelectorImpl>::new(
                MatchingMode::ForStatelessPseudoElement,
                None,
                &mut caches,
                self.stylist.quirks_mode(),
                NeedsSelectorFlags::No,
                MatchingForInvalidation::No,
            );
            matching.extra_data.originating_element_style = Some(&primary);
            let animation_declarations = style::properties::AnimationDeclarations::default();
            let mut declarations = style::applicable_declarations::ApplicableDeclarationList::new();
            self.stylist.push_applicable_declarations(
                node,
                Some(pseudo),
                None,
                None,
                &animation_declarations,
                RuleInclusion::All,
                &mut declarations,
                &mut matching,
            );
            if !declarations.is_empty() {
                let flags = matching.extra_data.cascade_input_flags;
                let rules = self.stylist.rule_tree().compute_rule_node(&mut declarations, &guards);
                let inputs = style::context::CascadeInputs {
                    rules: Some(rules),
                    flags,
                    ..Default::default()
                };
                return Some(self.stylist.compute_pseudo_element_style_with_inputs(
                    inputs,
                    pseudo,
                    &guards,
                    Some(&primary),
                    Some(node),
                ));
            }
        }
        if pseudo.is_lazy() {
            let _layout_thread_scope = LayoutThreadScope::enter();
            let guard = &self.guard;
            let guards = StylesheetGuards {
                author: &guard.read(),
                ua_or_user: &guard.read(),
            };
            // pyro: `::highlight(<name>)` rules share one map; the name must match.
            let wanted = *pseudo;
            let matches = move |pe: &PseudoElement| *pe == wanted;
            let lazy = self.stylist.lazily_compute_pseudo_element_style(
                &guards,
                node,
                &pseudo.rule_map_key(),
                RuleInclusion::All,
                &primary,
                false,
                pseudo.is_highlight().then_some(&matches as &dyn Fn(&PseudoElement) -> bool),
            );
            if lazy.is_some() {
                return lazy;
            }
        }
        Some(
            style::properties::StyleBuilder::for_inheritance(
                self.stylist.device(),
                Some(&self.stylist),
                Some(&primary),
                Some(pseudo),
            )
            .build(),
        )
    }

    /// The styles [`Self::resolve_undisplayed_style`] resolves, eagerly
    /// cascaded pseudo-elements included.
    fn resolve_undisplayed_styles(
        &self,
        node_id: NodeId,
        caches: Option<&mut UndisplayedCaches>,
    ) -> Option<style::data::ElementStyles> {
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
            visited_styles_enabled: true,
            animations: self.animations.clone(),
            current_time_for_animations: 0.0,
            snapshot_map: &self.snapshots,
            registered_speculative_painters: &RegisteredPaintersImpl,
        };
        // The thread-local context's sequential task list asserts the LAYOUT state when dropped
        let mut caches = caches;
        let styles = {
            let mut thread_local = ThreadLocalStyleContext::new();
            if let Some(selectors) = caches.as_deref_mut().and_then(|c| c.selectors.take()) {
                thread_local.selector_caches = selectors;
            }
            let styles = {
                let mut context = StyleContext {
                    shared: &shared,
                    thread_local: &mut thread_local,
                };
                resolve_style(&mut context, node, RuleInclusion::All, None, caches.as_deref_mut().map(|c| &mut c.styles))
            };
            if let Some(caches) = caches.as_deref_mut() {
                caches.selectors = Some(std::mem::take(&mut thread_local.selector_caches));
            }
            styles
        };

        Some(styles)
    }
}

/// pyro: what [`BaseDocument::resolve_undisplayed_style_cached`] keeps
/// between reads while the document stays the same.
#[derive(Default)]
pub struct UndisplayedCaches {
    styles: style::traversal::UndisplayedStyleCache,
    selectors: Option<selectors::context::SelectorCaches>,
}

impl UndisplayedCaches {
    /// Forgets everything: the document changed.
    pub fn clear(&mut self) {
        self.styles.clear();
        self.selectors = None;
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
        // pyro: XHTML and other XML documents are not HTML documents.
        !self.tree().xml_document
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

    // pyro: a shadow root stands for itself by its host (`crate::shadow`): the render tree has
    // no node for it, the host's children being the shadow tree's.
    fn host(&self) -> <Self::ConcreteNode as TNode>::ConcreteElement {
        self
    }

    fn style_data<'b>(&self) -> Option<&'b style::stylist::CascadeData>
    where
        Self: 'b,
    {
        let host: &'b Node = self;
        host.shadow_cascade_data()
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
        OpaqueElement::from_ptr(non_null)
    }

    // pyro: selectors walk the node trees, which are not the flat tree when there are shadow
    // trees (`crate::shadow`).
    fn parent_element(&self) -> Option<Self> {
        if !self.is_element() || !self.tree().has_shadow_hosts {
            return TElement::traversal_parent(self);
        }
        self.dom_parent_element()
    }

    fn parent_node_is_shadow_root(&self) -> bool {
        self.is_element() && self.parent_is_shadow_root()
    }

    fn containing_shadow_host(&self) -> Option<Self> {
        self.containing_shadow_host_id().map(|host| self.with(host))
    }

    fn assigned_slot(&self) -> Option<Self> {
        self.assigned_slot_id().map(|slot| self.with(slot))
    }

    fn is_pseudo_element(&self) -> bool {
        matches!(self.data, NodeData::AnonymousBlock(_)) || self.is_details_content_slot()
    }

    // pyro: a scroll pseudo-element's box originates from the element it names, not from its
    // parent in the tree (a marker's is its group, `layout::scroll_markers`). An anonymous box
    // nested in another one (a `::first-letter` box inside a `::first-line` box) originates
    // from the outer box, and so on up to an element (`ultimate_originating_element` walks it).
    fn pseudo_element_originating_element(&self) -> Option<Self> {
        // pyro: a `<details>` element's content slot is its `::details-content`.
        if self.is_details_content_slot() {
            return TNode::parent_node(self).filter(|parent| parent.is_element());
        }
        match self.scroll_pseudo() {
            Some((_, origin)) => Some(self.with(origin)),
            None => match TNode::parent_node(self) {
                Some(parent) if parent.is_pseudo_element() => Some(parent),
                _ => selectors::Element::parent_element(self),
            },
        }
    }

    // These methods are implemented naively since we only threaded real nodes and not fake nodes
    // we should try and use `find` instead of this foward/backward stuff since its ugly and slow
    fn prev_sibling_element(&self) -> Option<Self> {
        // pyro: an assigned node's siblings are its light tree's (`light_sibling_element`).
        if let Some(sibling) = self.light_sibling_element(-1) {
            return sibling;
        }
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
        if let Some(sibling) = self.light_sibling_element(1) {
            return sibling;
        }
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
        // pyro: in an XML document, type and attribute selectors match case-sensitively (an
        // XHTML `DIV` selector does not match `div`), and so they do for an element of another
        // namespace in an HTML document (Selectors 4 §6.1: `foreignObject` and `linearGradient`
        // match SVG elements by their mixed-case names, `foreignobject` does not).
        !self.tree().xml_document && self.has_namespace(&ns!(html))
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
        context: &mut MatchingContext<Self::Impl>,
    ) -> bool {
        match *pseudo_class {
            NonTSPseudoClass::Active => self.element_state().contains(ElementState::ACTIVE),
            NonTSPseudoClass::AnyLink => self
                .element_state()
                .intersects(ElementState::VISITED_OR_UNVISITED),
            NonTSPseudoClass::Checked => self.element_state().contains(ElementState::CHECKED),
            NonTSPseudoClass::Valid => self.element_state().contains(ElementState::VALID),
            NonTSPseudoClass::Invalid => self.element_state().contains(ElementState::INVALID),
            // pyro: every element but a custom element not defined yet (pyro-dom's `undefined`
            // state token).
            NonTSPseudoClass::Defined => self.element_state().contains(ElementState::DEFINED),
            NonTSPseudoClass::Disabled => self.element_state().contains(ElementState::DISABLED),
            NonTSPseudoClass::Enabled => self.element_state().contains(ElementState::ENABLED),
            NonTSPseudoClass::Focus => self.element_state().contains(ElementState::FOCUS),
            NonTSPseudoClass::FocusWithin => self.element_state().contains(ElementState::FOCUS_WITHIN),
            NonTSPseudoClass::FocusVisible => self.element_state().contains(ElementState::FOCUSRING),
            NonTSPseudoClass::Fullscreen => false,
            NonTSPseudoClass::Hover => self.element_state().contains(ElementState::HOVER),
            NonTSPseudoClass::Indeterminate => {
                self.element_state().contains(ElementState::INDETERMINATE)
            }
            NonTSPseudoClass::Lang(ref lang) => self.match_element_lang(None, lang),
            // pyro: the element's directionality (`crate::direction`).
            NonTSPseudoClass::Dir(ref dir) => match &**dir {
                "ltr" => !crate::direction::is_rtl(self),
                "rtl" => crate::direction::is_rtl(self),
                _ => false,
            },
            NonTSPseudoClass::CustomState(ref state) => self.has_lp_custom_state(&state.0),
            // pyro: a slot (of a shadow tree) with flattened slottables, one of them an element
            // matching the argument (CSS Scoping 1 § 3.2.5), which is matched in its own tree.
            NonTSPseudoClass::HasSlotted(ref argument) => {
                let is_slot = self.element_data().is_some_and(|data| {
                    data.name.ns == markup5ever::ns!(html) && &*data.name.local == "slot"
                }) && self.containing_shadow_host_id().is_some();
                is_slot
                    && self.any_flattened_slottable(&mut |node: &Node| match argument {
                        None => true,
                        Some(argument) => {
                            node.is_element()
                                && context.nest(|context| {
                                    selectors::matching::matches_selector_list(&argument.0, node, context)
                                })
                        }
                    })
            }
            // pyro: every link, visited or not, as browsers match `:link` for an element's style
            // (only the colours a visited link paints come from `:visited`).
            // A visited link's highlight styles (`::selection`) take it as visited, as Chromium's
            // visited style (WPT css-pseudo `selection-link-002`; `crate::dom_selection`).
            NonTSPseudoClass::Link => {
                self.element_state().intersects(ElementState::VISITED_OR_UNVISITED)
                    // Inside `:has()` every link is unvisited (as `:visited` below).
                    && (context.visited_handling().matches_unvisited() || context.relative_selector_anchor().is_some())
                    && !(crate::dom_selection::LINKS_AS_VISITED.get()
                        && self.element_state().contains(ElementState::VISITED))
            }
            NonTSPseudoClass::PlaceholderShown => {
                self.element_state().contains(ElementState::PLACEHOLDER_SHOWN)
            }
            NonTSPseudoClass::ReadWrite => self.element_state().contains(ElementState::READWRITE),
            NonTSPseudoClass::ReadOnly => self.element_state().contains(ElementState::READONLY),
            NonTSPseudoClass::ServoNonZeroBorder => false,
            NonTSPseudoClass::ScrollbarVertical | NonTSPseudoClass::ScrollbarHorizontal => false,
            // pyro: the embedder's indicated element (`BaseDocument::set_url_target`).
            NonTSPseudoClass::Target => self.element_state().contains(ElementState::URLTARGET),
            // pyro: the active scroll marker of its group (`layout::scroll_markers`).
            NonTSPseudoClass::TargetCurrent => {
                self.element_state().contains(style::selector_parser::TARGET_CURRENT)
            }
            NonTSPseudoClass::TargetBefore => {
                self.element_state().contains(style::selector_parser::TARGET_BEFORE)
            }
            NonTSPseudoClass::TargetAfter => {
                self.element_state().contains(style::selector_parser::TARGET_AFTER)
            }
            // pyro: links to the document itself (`ElementData::flush_link_visited`), but not
            // inside `:has()`, where Chromium takes every link as unvisited (WPT
            // `css/selectors/has-visited.html`, `invalidation/link-pseudo-in-has.html`).
            // The element's style matches every link as unvisited and its visited style (Stylo's
            // `visited_styles_enabled`) the relevant link as visited, whose visited-dependent
            // colours a visited link takes (`StyleBuilder::pyro_take_visited_dependent`).
            NonTSPseudoClass::Visited => {
                self.element_state().intersects(ElementState::VISITED_OR_UNVISITED)
                    && (context.visited_handling().matches_visited()
                        || (crate::dom_selection::LINKS_AS_VISITED.get()
                            && self.element_state().contains(ElementState::VISITED))
                        // Invalidation (`:has()` insertion stand-ins) matches by the state, as
                        // before visited styles: in its normal mode `:visited` would never match.
                        || (context.matching_for_invalidation()
                            && self.element_state().contains(ElementState::VISITED)))
                    && context.relative_selector_anchor().is_none()
            }
            NonTSPseudoClass::Autofill => false,
            NonTSPseudoClass::Default => self.element_state().contains(ElementState::DEFAULT),

            NonTSPseudoClass::InRange => self.element_state().contains(ElementState::INRANGE),
            NonTSPseudoClass::Modal => self.element_state().contains(ElementState::MODAL),
            NonTSPseudoClass::MozInert => self.element_state().contains(ElementState::INERT),
            NonTSPseudoClass::Open => self.element_state().contains(ElementState::OPEN),
            NonTSPseudoClass::Optional => self.element_state().contains(ElementState::OPTIONAL_),
            NonTSPseudoClass::OutOfRange => self.element_state().contains(ElementState::OUTOFRANGE),
            NonTSPseudoClass::PopoverOpen => {
                self.element_state().contains(ElementState::POPOVER_OPEN)
            }
            NonTSPseudoClass::Required => self.element_state().contains(ElementState::REQUIRED),
            NonTSPseudoClass::UserInvalid => {
                self.element_state().contains(ElementState::USER_INVALID)
            }
            NonTSPseudoClass::UserValid => self.element_state().contains(ElementState::USER_VALID),
            NonTSPseudoClass::MozMeterOptimum => false,
            NonTSPseudoClass::MozMeterSubOptimum => false,
            NonTSPseudoClass::MozMeterSubSubOptimum => false,
            // pyro: a dialog in the top layer (modal, or kept there for its `overlay` transition):
            // the state pyro-dom gives it (Chromium's `:-internal-dialog-in-top-layer`).
            NonTSPseudoClass::MozTopmostModal => self.element_state().contains(ElementState::TOPMOST_MODAL),
        }
    }

    fn match_pseudo_element(
        &self,
        pe: &PseudoElement,
        _context: &mut MatchingContext<Self::Impl>,
    ) -> bool {
        // pyro: a `<details>` element's content slot is its `::details-content`, styled or not yet
        // (its style is what is being computed when the slotted rules match it).
        if self.is_details_content_slot() {
            return *pe == PseudoElement::DetailsContent;
        }
        // pyro: the box of a scroll pseudo-element; a button matches the arguments that select
        // its direction (`layout::scroll_markers`).
        if let Some((pseudo, origin)) = self.scroll_pseudo() {
            if pseudo == PseudoElement::ScrollButtonAll {
                let direction = self.scroll_pseudos().map_or(0, |pseudos| pseudos.direction);
                let Some(wm) = self.tree()[origin].primary_styles().map(|style| style.writing_mode) else {
                    return false;
                };
                return crate::layout::scroll_markers::scroll_button_matches(pe, direction, wm);
            }
            return *pe == pseudo;
        }
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

    // pyro: an `<a>` (any, as before; `:link`/`:visited` still need link state). Not keyed on the
    // link state: that is set after insertion, which `:has()` invalidation would miss.
    fn is_link(&self) -> bool {
        self.data.is_element_with_tag_name(&local_name!("a"))
    }

    fn is_html_slot_element(&self) -> bool {
        self.element_data()
            .is_some_and(|e| e.name.local == local_name!("slot") && e.name.ns == ns!(html))
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
        self.element_data().is_some_and(|data| {
            data.classes
                .iter()
                .any(|class| case_sensitivity.eq_atom(class, search_name))
        })
    }

    // pyro: `exportparts` maps the host's inner part names to outer ones: the inner name an outer
    // one stands for.
    fn imported_part(
        &self,
        name: &<Self::Impl as selectors::SelectorImpl>::Identifier,
    ) -> Option<<Self::Impl as selectors::SelectorImpl>::Identifier> {
        let value = self.attr(local_name!("exportparts"))?;
        crate::shadow::export_parts(value)
            .find(|(_, outer)| *outer == &*name.0)
            .map(|(inner, _)| AtomIdent::from(inner))
    }

    fn is_part(&self, name: &<Self::Impl as selectors::SelectorImpl>::Identifier) -> bool {
        self.attr(local_name!("part"))
            .is_some_and(|parts| parts.split_ascii_whitespace().any(|p| p == &*name.0))
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
        // pyro: this element is in the subtree (or among the
        // siblings) a `:has()` bloom filter covers, and a search the filter
        // fast-rejects visits nothing and so flags nothing. Flag it as
        // searched in both directions, as a full search would have, so a
        // later change to it still reaches the anchors
        // (`has_invalidation`). Filters are built only for `:has()`.
        self.selector_flags().set(
            self.selector_flags().get()
                | ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR_SIBLING,
        );
        each_relevant_element_hash(*self, |hash| filter.insert_hash(hash & BLOOM_HASH_MASK));
        true
    }
}

impl<'a> TElement for BlitzNode<'a> {
    type ConcreteNode = BlitzNode<'a>;

    type TraversalChildrenIterator = Traverser<'a>;

    // pyro: the box of an element's `::before` or `::after` is that pseudo-element: the rules of
    // `::before::marker` / `::after::marker` are looked up for it (`layout::list`).
    fn implemented_pseudo_element(&self) -> Option<PseudoElement> {
        // pyro: the slot a `<details>` element's content renders in (pyro-dom's render tree gives
        // it the user-agent shadow tree Chromium gives it) is its `::details-content`.
        if self.is_details_content_slot() {
            return Some(PseudoElement::DetailsContent);
        }
        if !matches!(self.data, NodeData::AnonymousBlock(_)) {
            return None;
        }
        // pyro: the box of a scroll pseudo-element (`layout::scroll_markers`).
        if let Some((pseudo, _)) = self.scroll_pseudo() {
            return Some(pseudo);
        }
        let parent = &self.tree()[self.parent?];
        if parent.before() == Some(self.id) {
            Some(PseudoElement::Before)
        } else if parent.after() == Some(self.id) {
            Some(PseudoElement::After)
        } else {
            None
        }
    }

    fn as_node(&self) -> Self::ConcreteNode {
        self
    }

    // pyro: styles inherit through the flat tree (a slot's assigned nodes from the slot, a shadow
    // tree's first level from the host), which `parent_element` leaves for the node trees
    // (`crate::shadow`).
    fn inheritance_parent(&self) -> Option<Self> {
        TElement::traversal_parent(self)
    }

    fn implicit_scope_for_sheet_in_shadow_root(
        _opaque_host: OpaqueElement,
        _sheet_index: usize,
    ) -> Option<ImplicitScopeRoot> {
        // We cannot currently implement this as we are using the NodeId as the OpaqueElement,
        // and need a reference to the Slab to convert it back into an Element.
        // pyro: a prelude-less `@scope` in a shadow tree's sheet then scopes nothing (its rules
        // do not apply) rather than panicking.
        None
    }

    fn traversal_children(&self) -> style::dom::LayoutIterator<Self::TraversalChildrenIterator> {
        LayoutIterator(Traverser {
            // dom: self.tree(),
            parent: self,
            child_index: 0,
        })
    }

    // pyro: by namespace (stylo's display: contents unboxing rules, CSS Display 3 appendix B,
    // and `<use>` shadow trees depend on it; WPT css-display `display-contents-svg-elements`).
    fn is_html_element(&self) -> bool {
        self.element_data().is_some_and(|data| data.name.ns == markup5ever::ns!(html))
    }

    fn is_mathml_element(&self) -> bool {
        self.element_data().is_some_and(|data| data.name.ns == markup5ever::ns!(mathml))
    }

    fn is_svg_element(&self) -> bool {
        self.element_data().is_some_and(|data| data.name.ns == markup5ever::ns!(svg))
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
        self.attr(local_name!("part")).is_some()
    }

    fn exports_any_part(&self) -> bool {
        self.attr(local_name!("exportparts")).is_some()
    }

    fn each_part<F>(&self, mut callback: F)
    where
        F: FnMut(&style::values::AtomIdent),
    {
        if let Some(parts) = self.attr(local_name!("part")) {
            for part in parts.split_ascii_whitespace() {
                callback(&AtomIdent::from(part));
            }
        }
    }

    fn each_exported_part<F>(&self, name: &style::values::AtomIdent, mut callback: F)
    where
        F: FnMut(&style::values::AtomIdent),
    {
        if let Some(value) = self.attr(local_name!("exportparts")) {
            for (inner, outer) in crate::shadow::export_parts(value) {
                if inner == &*name.0 {
                    callback(&AtomIdent::from(outer));
                }
            }
        }
    }

    fn id(&self) -> Option<&style::Atom> {
        self.element_data().and_then(|data| data.id.as_ref())
    }

    fn each_class<F>(&self, mut callback: F)
    where
        F: FnMut(&style::values::AtomIdent),
    {
        if let Some(data) = self.element_data() {
            for class in &data.classes {
                callback(AtomIdent::cast(class));
            }
        }
    }

    fn each_attr_name<F>(&self, mut callback: F)
    where
        F: FnMut(&style::LocalName),
    {
        if let Some(attrs) = self.data.attrs() {
            for attr in attrs.iter() {
                callback(GenericAtomIdent::cast(&attr.name.local));
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
        // SAFETY: stylo traversal has exclusive access to nodes
        unsafe { self.stylo_element_data().clear() }
    }

    fn has_data(&self) -> bool {
        self.try_stylo_element_data().is_some_and(|s| s.has_data())
    }

    fn borrow_data(&self) -> Option<ElementDataRef<'_>> {
        // Stylo reads the existence of element data as "has been styled" (its invalidator
        // unwraps the primary style of every element with data it descends into), but an
        // element here can carry data before it has styles.
        self.try_stylo_element_data()
            .and_then(|s| s.get())
            .filter(|data| data.styles.primary.is_some())
    }

    fn mutate_data(&self) -> Option<ElementDataMut<'_>> {
        unsafe { self.stylo_element_data().unsafe_stylo_only_mut() }
    }

    fn skip_item_display_fixup(&self) -> bool {
        false
    }

    fn may_have_animations(&self) -> bool {
        self.element_data()
            .is_some_and(|data| data.may_have_animations.load(Ordering::Relaxed))
            // pyro: values the embedder samples in (`animation.rs`).
            || crate::animation::has_sampled(self)
    }

    fn set_may_have_animations(&self) {
        if let Some(data) = self.element_data() {
            data.may_have_animations.store(true, Ordering::Relaxed);
        }
    }

    fn has_animations(&self, context: &SharedStyleContext) -> bool {
        if !self.may_have_animations() {
            return false;
        }
        // pyro: sampled values keep the element out of style sharing.
        if crate::animation::has_sampled(self) {
            return true;
        }
        let key = AnimationSetKey::new_for_non_pseudo(TNode::opaque(&TElement::as_node(self)));
        context
            .animations
            .has_active_animations_or_transitions(&key)
    }

    fn has_css_animations(
        &self,
        context: &SharedStyleContext,
        pseudo_element: Option<PseudoElement>,
    ) -> bool {
        if !self.may_have_animations() {
            return false;
        }
        let key = AnimationSetKey::new(TNode::opaque(&TElement::as_node(self)), pseudo_element);
        context.animations.has_active_animations(&key)
    }

    fn has_css_transitions(
        &self,
        context: &SharedStyleContext,
        pseudo_element: Option<PseudoElement>,
    ) -> bool {
        if !self.may_have_animations() {
            return false;
        }
        let key = AnimationSetKey::new(TNode::opaque(&TElement::as_node(self)), pseudo_element);
        context.animations.has_active_transitions(&key)
    }

    fn animation_rule(
        &self,
        context: &SharedStyleContext,
    ) -> Option<Arc<Locked<PropertyDeclarationBlock>>> {
        // pyro: the values the embedder samples in (`animation.rs`).
        if let Some(block) = crate::animation::sampled_rule(self, false) {
            return Some(block);
        }
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
        if let Some(block) = crate::animation::sampled_rule(self, true) {
            return Some(block);
        }
        let opaque = TNode::opaque(&TElement::as_node(self));
        context.animations.get_transition_declarations(
            &AnimationSetKey::new_for_non_pseudo(opaque),
            context.current_time_for_animations,
            self.guard(),
        )
    }

    fn shadow_root(&self) -> Option<<Self::ConcreteNode as TNode>::ConcreteShadowRoot> {
        self.is_shadow_host().then_some(*self)
    }

    fn containing_shadow(&self) -> Option<<Self::ConcreteNode as TNode>::ConcreteShadowRoot> {
        self.containing_shadow_host_id().map(|host| self.with(host))
    }

    fn get_attr(&self, attr: &style::LocalName, ns: &style::Namespace) -> Option<String> {
        // pyro: `attr(Title)` names the `title` attribute of an HTML element in an HTML document
        // (css-values-5 § attr(): the name is lowercased there); XML documents keep its case.
        let lowered = (!self.tree().xml_document
            && self.element_data().is_some_and(|element| element.name.ns == ns!(html))
            && attr.0.bytes().any(|b| b.is_ascii_uppercase()))
        .then(|| markup5ever::LocalName::from(attr.0.to_ascii_lowercase()));
        let name = lowered.as_ref().unwrap_or(&attr.0);
        // pyro: for a pseudo-element's box (the marker style of a `::before` or `::after` list
        // item is cascaded on it), the attribute of its originating element.
        // pyro: and of a scroll pseudo-element's box, of its originating element.
        if let Some((_, origin)) = self.scroll_pseudo() {
            return self.with(origin).get_attr(attr, ns);
        }
        if matches!(self.data, NodeData::AnonymousBlock(_))
            && let Some(parent) = self.parent.map(|id| &self.tree()[id])
            && (parent.before() == Some(self.id) || parent.after() == Some(self.id))
        {
            return parent.get_attr(attr, ns);
        }
        self.attrs()?
            .iter()
            .find(|a| a.name.local == *name && a.name.ns == ns.0)
            .map(|a| a.value.clone())
    }

    fn lang_attr(&self) -> Option<style::selector_parser::AttrValue> {
        // pyro
        crate::lang::own_lang(self).map(AtomString::from)
    }

    fn match_element_lang(
        &self,
        override_lang: Option<Option<style::selector_parser::AttrValue>>,
        value: &style::selector_parser::Lang,
    ) -> bool {
        // pyro: the element's language (`crate::lang`), or the attribute a snapshot had.
        match override_lang {
            Some(Some(lang)) => crate::lang::range_matches(lang.as_ref(), value),
            Some(None) => false,
            None => crate::lang::language(self).is_some_and(|lang| crate::lang::range_matches(lang, value)),
        }
    }

    // pyro: a link to the document itself (`ElementData::flush_link_visited`); its visited style
    // gives its colours (`StyleAdjuster::adjust_for_links`).
    fn is_visited_link(&self) -> bool {
        self.element_state().contains(ElementState::VISITED)
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

    fn synthesize_presentational_hints_for_legacy_attributes<'h, V>(
        &self,
        _visited_handling: VisitedHandlingMode,
        hints: &mut V,
    ) where
        V: Push<style::applicable_declarations::ApplicableDeclarationBlock<'h>>,
        Self: 'h,
    {
        // pyro: Stylo 0.24 borrows the declaration blocks for the
        // cascade, so the element owns them (`Attributes::presentational_hints`).
        let node: &'h Node = self;
        let Some(elem) = node.data.downcast_element() else {
            return;
        };
        let mut push = |block: &'h Arc<Locked<PropertyDeclarationBlock>>| {
            hints.push(ApplicableDeclarationBlock::from_declarations(
                StyleSourceBorrow::from_declarations(block.borrow_arc()),
                CascadeLevel::new(CascadeOrigin::PresHints),
                LayerOrder::root(),
            ));
        };
        let own = elem.attrs.presentational_hints(|| node.build_presentational_hints(elem));
        own.own.iter().for_each(&mut push);
        let tag = &elem.name.local;
        if *tag == local_name!("td") || *tag == local_name!("th") {
            let table = node.cell_table().and_then(|table| Some((table, table.data.downcast_element()?)));
            if let Some((table, table_elem)) = table {
                let of_table = table_elem.attrs.presentational_hints(|| table.build_presentational_hints(table_elem));
                of_table.cells.iter().for_each(&mut push);
            }
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
        let mut sizes = self.tree().container_query_sizes.lock().unwrap();
        if let Some(entry) = sizes.get(&self.id) {
            return entry.size;
        }
        // pyro: a first query from a laid-out element outside the style traversal (a lazy pseudo
        // style such as `::highlight()`/`::selection` resolved at paint) samples the size now:
        // no later pass would fill it in (WPT container-queries/pseudo-elements-011,
        // container-units-selection-pseudo). In a traversal this is the last layout's size; the
        // refresh after the traversal corrects it.
        let mut entry = crate::tree::ContainerQuerySize::default();
        if self.element_data().is_some_and(|element| element.layout_data.is_some()) {
            if let Some(values) = self.primary_styles() {
                use style::values::specified::ContainerType;
                let kind = values.get_box().slow_clone_container_type();
                let both = kind.intersects(ContainerType::SIZE);
                let inline = kind.intersects(ContainerType::INLINE_SIZE);
                if both || inline {
                    let horizontal = values.writing_mode.is_horizontal();
                    entry.axes = euclid::default::Size2D::new(both || (inline && horizontal), both || (inline && !horizontal));
                    let layout = self.final_layout();
                    // pyro: less its scrollbar gutters (`Node::container_query_gutters`)
                    let g = self.container_query_gutters();
                    entry.size = euclid::default::Size2D::new(
                        entry.axes.width.then(|| app_units::Au::from_f32_px((layout.content_box_width() - g.width).max(0.0))),
                        entry.axes.height.then(|| app_units::Au::from_f32_px((layout.content_box_height() - g.height).max(0.0))),
                    );
                }
            }
        }
        let size = entry.size;
        sizes.insert(self.id, entry);
        size
    }

    fn each_custom_state<F>(&self, mut callback: F)
    where
        F: FnMut(&AtomIdent),
    {
        // pyro: pyro-dom's render tree lists them (`has_lp_custom_state`).
        let value = self.attrs().and_then(|attrs| {
            attrs
                .iter()
                .find(|a| &*a.name.ns == crate::PYRO_STATE_NS && &*a.name.local == "custom-states")
        });
        if let Some(attr) = value {
            for state in crate::has_invalidation::pyro_custom_states(Some(&attr.value)) {
                callback(AtomIdent::cast(&style::Atom::from(state)));
            }
        }
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

impl Node {
    /// The declaration blocks the element's legacy attributes map to, one
    /// per declaration.
    fn build_presentational_hints(&self, elem: &ElementData) -> crate::node::PresentationalHints {
        let tag = &elem.name.local;
        let mut own = Vec::new();

        let mut push_style = |decl: PropertyDeclaration| {
            own.push(Arc::new(
                self.guard()
                    .wrap(PropertyDeclarationBlock::with_one(decl, Importance::Normal)),
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

        /// The rules for parsing a legacy font size: an integer from 1 to 7, either
        /// absolute or relative (`+n`, `-n`) to the default size 3.
        ///
        /// https://html.spec.whatwg.org/multipage/rendering.html#rules-for-parsing-a-legacy-font-size
        fn parse_legacy_font_size(value: &str) -> Option<u8> {
            use style::str::{HTML_SPACE_CHARACTERS, read_numbers};

            let mut chars = value
                .trim_start_matches(HTML_SPACE_CHARACTERS)
                .chars()
                .peekable();
            let sign = chars.next_if(|&c| matches!(c, '+' | '-'));
            let number = match read_numbers(chars) {
                (Some(number), _) => number,
                // No digits
                (None, 0) => return None,
                // Too many digits for an i64, but every number above 7 gives the same size
                (None, _) => i64::MAX,
            };
            let size = match sign {
                Some('+') => number.saturating_add(3),
                Some('-') => 3 - number,
                _ => number,
            };
            Some(size.clamp(1, 7) as u8)
        }

        /// The `face` attribute's value parsed as a `font-family` list, as Gecko and
        /// Blink parse it: CSS-wide keywords and `var()` are not family lists.
        fn parse_font_face_attr(value: &str) -> Option<PropertyDeclaration> {
            use style::properties::{
                PropertyId, SourcePropertyDeclaration, parse_one_declaration_into,
            };
            use style::stylesheets::{CssRuleType, Origin};
            use style_traits::ParsingMode;

            let property = PropertyId::parse_enabled_for_all_content("font-family").ok()?;
            let mut declarations = SourcePropertyDeclaration::default();
            parse_one_declaration_into(
                &mut declarations,
                property,
                value,
                Origin::Author,
                &crate::url::DocumentUrl::default().url_extra_data(),
                None,
                ParsingMode::DEFAULT,
                style::context::QuirksMode::NoQuirks,
                CssRuleType::Style,
            )
            .ok()?;
            declarations
                .drain()
                .declarations
                .find(|declaration| matches!(declaration, PropertyDeclaration::FontFamily(_)))
        }

        /// pyro: an SVG presentation attribute's value parsed as the CSS
        /// property's value (`property` is its name): the declarations of a
        /// shorthand's longhands (`overflow`), or the longhand's.
        fn parse_svg_presentation_attr(property: &str, value: &str) -> Vec<PropertyDeclaration> {
            use style::properties::{PropertyId, SourcePropertyDeclaration, parse_one_declaration_into};
            use style::stylesheets::{CssRuleType, Origin};
            use style_traits::ParsingMode;

            let Ok(property) = PropertyId::parse_enabled_for_all_content(property) else {
                return Vec::new();
            };
            let mut declarations = SourcePropertyDeclaration::default();
            let parsed = parse_one_declaration_into(
                &mut declarations,
                property,
                value,
                Origin::Author,
                &crate::url::DocumentUrl::default().url_extra_data(),
                None,
                // A number is a length in user units (CSS px), as Gecko parses
                // presentation attributes.
                ParsingMode::ALLOW_UNITLESS_LENGTH,
                style::context::QuirksMode::NoQuirks,
                CssRuleType::Style,
            );
            match parsed {
                Ok(()) => declarations.drain().declarations.collect(),
                Err(_) => Vec::new(),
            }
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

            // SVG presentation attributes enter the author cascade with zero
            // specificity. This also resolves custom properties and currentColor
            // before SVG serialization, text measurement, and getComputedStyle.
            // Geometry attributes have their own mapping below.
            if elem.name.ns == ns!(svg)
                && attr.name.ns == ns!()
                && matches!(&**name,
                    "clip-path" | "transform-origin" | "overflow"
                    | "fill" | "fill-opacity" | "fill-rule"
                    | "stroke" | "stroke-width" | "stroke-opacity" | "stroke-linecap"
                    | "stroke-linejoin" | "stroke-miterlimit" | "stroke-dasharray" | "stroke-dashoffset"
                    | "opacity" | "color" | "visibility" | "display"
                    | "font-family" | "font-size" | "font-weight" | "font-style" | "font-stretch"
                    | "letter-spacing" | "word-spacing" | "text-anchor" | "text-decoration"
                    | "dominant-baseline" | "alignment-baseline" | "baseline-shift"
                    | "paint-order" | "vector-effect" | "shape-rendering" | "text-rendering")
            {
                for declaration in parse_svg_presentation_attr(&**name, value) {
                    push_style(declaration);
                }
            }

            if *name == local_name!("lang") {
                if attr.name.ns == ns!(xml) {
                    lang = Some(value);
                } else if attr.name.ns == ns!() && accepts_lang {
                    lang = lang.or(Some(value));
                }
            }

            // A `font` element's `face` sets `font-family` to the attribute's value, and
            // its `size` maps to a font size keyword.
            // https://html.spec.whatwg.org/multipage/rendering.html#phrasing-content-3
            if *tag == local_name!("font") && elem.name.ns == ns!(html) {
                if *name == local_name!("face")
                    && let Some(family) = parse_font_face_attr(value)
                {
                    push_style(family);
                } else if *name == local_name!("size")
                    && let Some(size) = parse_legacy_font_size(value)
                {
                    push_style(PropertyDeclaration::FontSize(
                        style::values::specified::FontSize::from_html_size(size),
                    ));
                }
            }

            // pyro: `<br clear>` maps to `clear` (HTML § 15.3.10, "Phrasing content"):
            // `left`, `right`, `all` and `both`, ASCII case-insensitively.
            if *name == local_name!("clear") && *tag == local_name!("br") && elem.name.ns == ns!(html) {
                use style::values::specified::box_::Clear;
                let clear = if value.eq_ignore_ascii_case("left") {
                    Some(Clear::Left)
                } else if value.eq_ignore_ascii_case("right") {
                    Some(Clear::Right)
                } else if value.eq_ignore_ascii_case("all") || value.eq_ignore_ascii_case("both") {
                    Some(Clear::Both)
                } else {
                    None
                };
                if let Some(clear) = clear {
                    push_style(PropertyDeclaration::Clear(clear));
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

            // `valign` on HTML table row groups, rows and cells maps to
            // `vertical-align`, matched ASCII case-insensitively. `vertical-align`
            // is a shorthand in Stylo, so push the three longhands each keyword
            // expands to (as `shorthands::vertical_align::parse_value` does).
            // https://html.spec.whatwg.org/multipage/rendering.html#tables-2
            if *name == local_name!("valign")
                && elem.name.ns == ns!(html)
                && matches!(
                    *tag,
                    local_name!("thead")
                        | local_name!("tbody")
                        | local_name!("tfoot")
                        | local_name!("tr")
                        | local_name!("td")
                        | local_name!("th")
                )
            {
                use style::values::generics::box_::BaselineShiftKeyword;
                use style::values::specified::{AlignmentBaseline, BaselineShift, BaselineSource};
                let longhands = if value.eq_ignore_ascii_case("top") {
                    Some((
                        AlignmentBaseline::Baseline,
                        BaselineShift::Keyword(BaselineShiftKeyword::Top),
                    ))
                } else if value.eq_ignore_ascii_case("middle") {
                    Some((AlignmentBaseline::VerticalAlignMiddle, BaselineShift::zero()))
                } else if value.eq_ignore_ascii_case("bottom") {
                    Some((
                        AlignmentBaseline::Baseline,
                        BaselineShift::Keyword(BaselineShiftKeyword::Bottom),
                    ))
                } else if value.eq_ignore_ascii_case("baseline") {
                    Some((AlignmentBaseline::Baseline, BaselineShift::zero()))
                } else {
                    None
                };

                if let Some((alignment_baseline, baseline_shift)) = longhands {
                    push_style(PropertyDeclaration::BaselineSource(BaselineSource::Auto));
                    push_style(PropertyDeclaration::AlignmentBaseline(alignment_baseline));
                    push_style(PropertyDeclaration::BaselineShift(baseline_shift));
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
            // pyro: so are those of a `<foreignObject>` (SVG 2 geometry properties), whose
            // HTML content is laid out in a box of that size (`BaseDocument::svg_foreign_objects`).
            if (*tag == local_name!("svg") || (*tag == local_name!("foreignObject") && elem.name.ns == ns!(svg)))
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

            // https://html.spec.whatwg.org/multipage/rendering.html#tables-2
            if *name == local_name!("cellspacing") && *tag == local_name!("table") {
                if let Ok(px) = style::servo::attr::parse_unsigned_integer(value.chars()) {
                    use style::values::specified::{BorderSpacing, NonNegativeLength};
                    let spacing = NonNegativeLength::from_px(px as f32);
                    push_style(PropertyDeclaration::BorderSpacing(BorderSpacing::new(
                        spacing.clone(),
                        spacing,
                    )));
                }
            }

            // pyro: `<body text>` colours the document's text (HTML rendering, "phrasing
            // content"), a legacy colour value (named colours included).
            if *name == local_name!("text") && *tag == local_name!("body") {
                use style::values::specified::{Color, ColorPropertyValue};
                if let Ok(color) = style::servo::attr::parse_legacy_color(value) {
                    push_style(PropertyDeclaration::Color(ColorPropertyValue(Color::from_absolute_color(color))));
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

        // A table's `cellpadding` attribute maps to the padding of its cells (not
        // those of nested tables), as a pixel length. The table owns the
        // declarations and its cells borrow them.
        // https://html.spec.whatwg.org/multipage/rendering.html#tables-2
        let mut cells = Vec::new();
        if *tag == local_name!("table") {
            let cellpadding = elem
                .attr(local_name!("cellpadding"))
                .and_then(|value| style::servo::attr::parse_unsigned_integer(value.chars()).ok());
            if let Some(px) = cellpadding {
                use style::values::generics::NonNegative;
                use style::values::specified::{LengthPercentage, NoCalcLength};
                let padding =
                    NonNegative(LengthPercentage::Length(NoCalcLength::from_px(px as f32)));
                for decl in [
                    PropertyDeclaration::PaddingTop(padding.clone()),
                    PropertyDeclaration::PaddingRight(padding.clone()),
                    PropertyDeclaration::PaddingBottom(padding.clone()),
                    PropertyDeclaration::PaddingLeft(padding),
                ] {
                    cells.push(Arc::new(
                        self.guard()
                            .wrap(PropertyDeclarationBlock::with_one(decl, Importance::Normal)),
                    ));
                }
            }
        }

        crate::node::PresentationalHints {
            own: own.into_boxed_slice(),
            cells: cells.into_boxed_slice(),
        }
    }

    /// The table a cell takes its `cellpadding` from: the first ancestor
    /// that isn't a row or row group, when that is a table.
    fn cell_table(&self) -> Option<&Node> {
        let mut ancestor = self.parent_node();
        while let Some(node) = ancestor {
            let name = &node.data.downcast_element()?.name.local;
            let is_table_internal = *name == local_name!("tr")
                || *name == local_name!("tbody")
                || *name == local_name!("thead")
                || *name == local_name!("tfoot");
            if !is_table_internal {
                return (*name == local_name!("table")).then_some(node);
            }
            ancestor = node.parent_node();
        }
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
    /// pyro: an element styled in this traversal kept a
    /// `::before` / `::after` style (one its rules generate).
    generated_before_after: std::sync::atomic::AtomicBool,
    /// pyro: elements styled `position: sticky` in this traversal
    /// (`BaseDocument::sticky_nodes`).
    sticky_nodes: Mutex<Vec<NodeId>>,
    /// pyro: the elements whose primary style this traversal replaced, with the old
    /// and new style, when `track_style_changes` (`BaseDocument::style_changes`).
    style_changes: Mutex<Vec<crate::StyleChange>>,
    track_style_changes: bool,
    /// pyro: an element styled in this traversal kept a `::first-letter` style.
    first_letter_styles: std::sync::atomic::AtomicBool,
    /// pyro: the elements whose `::first-letter` style this traversal changed
    /// (`layout::first_letter::style_changed`).
    first_letter_changed: Mutex<Vec<NodeId>>,
    /// pyro: restyled list items with a `::marker` style node (`layout::list::item_restyled`).
    marker_items_restyled: Mutex<Vec<NodeId>>,
    scroll_marker_groups: std::sync::atomic::AtomicBool,
    /// pyro: an element styled in this traversal kept a `::first-line` style, and the elements
    /// whose `::first-line` style it changed (`layout::first_line::style_changed`).
    first_line_styles: std::sync::atomic::AtomicBool,
    first_line_changed: Mutex<Vec<NodeId>>,
    /// pyro: an element styled in this traversal has a `::before` / `::after` whose content
    /// reads counters or quotes (`layout::counters`).
    generated_counters: std::sync::atomic::AtomicBool,
    /// pyro: an element was styled with `text-box-trim` (`layout::text_box`).
    text_box_trim_styles: std::sync::atomic::AtomicBool,
    /// pyro: an element was styled `column-span: all` (`BaseDocument::column_span_styles`).
    column_span_styles: std::sync::atomic::AtomicBool,
    /// pyro: an element was styled with a `scroll-snap-type` (`BaseDocument::scroll_snap_styles`).
    scroll_snap_styles: std::sync::atomic::AtomicBool,
    /// pyro: the document evaluates counters and quotes: restyles note whether they changed
    /// any (`counters_changed`).
    track_counters: bool,
    counters_changed: std::sync::atomic::AtomicBool,
}

impl<'a> RecalcStyle<'a> {
    pub fn new(context: SharedStyleContext<'a>) -> Self {
        RecalcStyle {
            context,
            nodes_needing_style_image_flush: Mutex::new(Vec::new()),
            generated_before_after: std::sync::atomic::AtomicBool::new(false),
            sticky_nodes: Mutex::new(Vec::new()),
            style_changes: Mutex::new(Vec::new()),
            track_style_changes: false,
            first_letter_styles: std::sync::atomic::AtomicBool::new(false),
            first_letter_changed: Mutex::new(Vec::new()),
            marker_items_restyled: Mutex::new(Vec::new()),
            scroll_marker_groups: std::sync::atomic::AtomicBool::new(false),
            first_line_styles: std::sync::atomic::AtomicBool::new(false),
            first_line_changed: Mutex::new(Vec::new()),
            generated_counters: std::sync::atomic::AtomicBool::new(false),
            text_box_trim_styles: std::sync::atomic::AtomicBool::new(false),
            column_span_styles: std::sync::atomic::AtomicBool::new(false),
            scroll_snap_styles: std::sync::atomic::AtomicBool::new(false),
            track_counters: false,
            counters_changed: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

#[allow(unsafe_code)]
/// pyro: what the style traversal must reach once Stylo's relative selector
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
    /// pyro: the `:has()` anchors whose answer the insertion of `node_id` can change
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

    /// pyro: as [`Self::invalidate_relative_selectors_for_insertion`] for an element
    /// about to be removed (or moved away), called while it is still linked: the anchors are
    /// found through its parent and earlier siblings.
    pub(crate) fn invalidate_relative_selectors_for_removal(&self, node_id: NodeId) {
        let node = &self.nodes[node_id];
        if !node.is_element() || !node.flags.is_in_document() {
            return;
        }
        self.invalidate_relative_selectors(node, ElementSelectorFlags::empty(), DomMutationOperation::Remove);
    }

    /// pyro: whether a style sheet of the document's origins or of a shadow tree has a relative
    /// selector (`:has()`).
    fn uses_relative_selectors(&self) -> bool {
        let data = self.stylist.cascade_data();
        use style::stylesheets::Origin;
        [Origin::UserAgent, Origin::User, Origin::Author]
            .into_iter()
            .any(|origin| data.borrow_for_origin(origin).relative_invalidation_map_attributes().used)
            || self.nodes.shadow_roots.values().any(|root| root.styles.data.relative_invalidation_map_attributes().used)
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
                snapshots: None,
                invalidated: relative_selector_invalidated_at,
                sibling_traversal_map,
            }
        }
        // pyro: `:host:has()` (`crate::shadow`).
        self.invalidate_host_relative_selectors(node);
        // pyro: without a relative selector in any style sheet there is nothing to invalidate.
        // Asked per element, Stylo walks the styles of every shadow-including ancestor, which
        // is quadratic in deeply nested shadow trees (WPT many-slots-crash: 1,000 levels).
        if inherited.is_empty() && !self.uses_relative_selectors() {
            return;
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

    /// pyro: the `:has()` anchors whose answer a change of `node_id`'s emptiness or
    /// sibling position can flip restyle (as Gecko's
    /// `MaybeInvalidateRelativeSelectorForEmptyDependency`, `...ForNthEdgeDependency` and
    /// `...ForNthDependencyFromSibling`). `:empty`, `:nth-child()` and `:first-child`/
    /// `:last-child` in an argument match by the tree around an element, not by its identity,
    /// so the element that came or went (`.z` in `.a:has(> .x:first-child)` with `.z` inserted
    /// before `.x`, a text node in `.a:has(> .p:empty)`) tells the mutation invalidator nothing:
    /// instead the element whose condition changed is looked up in the stylist's
    /// tree-structural dependency map for the `state`s given. Called after the mutation, with
    /// `node_id` linked in the document; an element no `:has()` search has reached cannot
    /// change an anchor's answer and is skipped.
    pub(crate) fn invalidate_relative_selectors_for_ts_state(
        &self,
        node_id: NodeId,
        state: TSStateForInvalidation,
    ) {
        let node = &self.nodes[node_id];
        if !node.is_element()
            || !node.flags.is_in_document()
            || node.relative_selector_search_direction().is_empty()
        {
            return;
        }
        let _layout_thread_scope = LayoutThreadScope::enter();
        let invalidator = RelativeSelectorInvalidator {
            element: node,
            quirks_mode: QuirksMode::NoQuirks,
            snapshots: None,
            invalidated: relative_selector_invalidated_at,
            sibling_traversal_map: SiblingTraversalMap::default(),
        };
        invalidator.invalidate_relative_selectors_for_this(
            &self.stylist,
            |element, scope, data, quirks_mode, collector| {
                data.relative_invalidation_map_attributes()
                    .ts_state_to_selector
                    .lookup_with_additional(
                        *element,
                        quirks_mode,
                        None,
                        &[],
                        ElementState::empty(),
                        |dependency| {
                            if dependency.state.intersects(state) {
                                collector.add_dependency(&dependency.dep, *element, scope);
                            }
                            true
                        },
                    );
            },
        );
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
            // Held (not just its address) so that a replacement cannot reuse the allocation.
            let old_primary = data.styles.get_primary().cloned();
            // pyro: the `::first-letter` style before the restyle (`layout::first_letter`).
            let old_first_letter = match data.styles.pseudos.is_empty() {
                true => None,
                false => data.styles.pseudos.get(&style::selector_parser::PseudoElement::FirstLetter).cloned(),
            };
            let old_first_line = match data.styles.pseudos.is_empty() {
                true => None,
                false => data.styles.pseudos.get(&style::selector_parser::PseudoElement::FirstLine).cloned(),
            };
            // pyro: what counters and quotes read of the element before the restyle.
            // pyro: the `::before`/`::after` styles before the restyle, for the embedder's transitions.
            let old_generated_boxes = (self.track_style_changes && !data.styles.pseudos.is_empty()).then(|| {
                [style::selector_parser::PseudoElement::Before, style::selector_parser::PseudoElement::After]
                    .map(|pseudo| data.styles.pseudos.get(&pseudo).cloned())
            });
            let old_generated = (self.track_counters && !self.counters_changed.load(std::sync::atomic::Ordering::Relaxed))
                .then(|| crate::layout::counters::CounterInputs::of(&data.styles));
            recalc_style_at(self, context, el, &mut data, note_child);
            // pyro: a replaced `::before`/`::after` style is a style change of the pseudo-element
            // (before its sampled values are cascaded in, as the element's own).
            if let Some(old) = old_generated_boxes {
                for (i, pseudo) in
                    [style::selector_parser::PseudoElement::Before, style::selector_parser::PseudoElement::After]
                        .iter()
                        .enumerate()
                {
                    let new = match data.styles.pseudos.is_empty() {
                        true => None,
                        false => data.styles.pseudos.get(pseudo),
                    };
                    if let (Some(old), Some(new)) = (&old[i], new) {
                        if !Arc::ptr_eq(old, new) {
                            self.style_changes.lock().unwrap().push(crate::StyleChange {
                                node: el.id,
                                pseudo: i as u8 + 1,
                                before: old.clone(),
                                after: new.clone(),
                            });
                        }
                    }
                }
            }
            // pyro: the animated values of the element's `::before`/`::after`.
            crate::animation::cascade_sampled_pseudos(el, context, &mut data);
            if let Some(old) = old_generated {
                if old != crate::layout::counters::CounterInputs::of(&data.styles) {
                    self.counters_changed.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
            // pyro: a replaced primary style rewrites what layout reads of it.
            let new_primary = data.styles.get_primary();
            let replaced = match (&old_primary, new_primary) {
                (Some(old), Some(new)) => !Arc::ptr_eq(old, new),
                (None, None) => false,
                _ => true,
            };
            // pyro: the embedder's transitions want the style before and after a
            // replacement; a first style, or styles dropped, is no change.
            if self.track_style_changes && replaced {
                if let (Some(old), Some(new)) = (&old_primary, new_primary) {
                    self.style_changes.lock().unwrap().push(crate::StyleChange {
                        node: el.id,
                        pseudo: 0,
                        before: old.clone(),
                        after: new.clone(),
                    });
                }
            }
            // Stylo diffs the old and new style into `data.damage` (nothing when there was no old
            // style, so a first style is always derived). A replacement without layout damage
            // (colour, background, :hover, a class that changes only paint) leaves the derived
            // layout styles as they are.
            let derive =
                replaced && (old_primary.is_none() || LayoutStyles::affected_by(data.damage));
            // A table without uniform collapsed borders resolves their colours when it collects
            // its rows (`CollapsedEdges`): a paint-only restyle of a table part that changes the
            // colour of one of its borders (a border colour, or `color` under a `currentColor`
            // border) collects its row again, with the text in its cells shaped as it was
            // (`BOX_DAMAGE`). Its widths and styles reach layout on their own. Upstream gave
            // every attribute change layout damage, which hid this.
            let collapsed_edges_changed = !derive
                && match (&old_primary, new_primary) {
                    (Some(old), Some(new)) if replaced => {
                        use style::computed_values::border_collapse::T as BorderCollapse;
                        let display = *new.get_display();
                        new.get_inherited_table().border_collapse == BorderCollapse::Collapse
                            && (display.outside() == style::values::specified::box_::DisplayOutside::InternalTable
                                || display.inside() == style::values::specified::box_::DisplayInside::Table)
                            && resolved_border_colors(old) != resolved_border_colors(new)
                    }
                    _ => false,
                }
                // pyro: as does `visibility: collapse` coming or going on a row or row group
                // (`TableRow::collapsed`: the row's track is taken away), or on a flex item (it
                // leaves its line, which keeps its cross size: Taffy's `measure_collapsed_item_struts`).
                || match (&old_primary, new_primary) {
                    (Some(old), Some(new)) if replaced => {
                        use style::computed_values::visibility::T as Visibility;
                        use style::values::specified::box_::DisplayInside;
                        let (was, is) = (old.get_inherited_box().visibility, new.get_inherited_box().visibility);
                        was != is
                            && (was == Visibility::Collapse || is == Visibility::Collapse)
                            && (matches!(
                                new.get_display().inside(),
                                DisplayInside::TableRow
                                    | DisplayInside::TableRowGroup
                                    | DisplayInside::TableHeaderGroup
                                    | DisplayInside::TableFooterGroup
                                    // pyro: or a column or column group (`Style::collapsed_columns`)
                                    | DisplayInside::TableColumn
                                    | DisplayInside::TableColumnGroup
                            ) || el.layout_parent.get().or(el.parent).is_some_and(|parent| {
                                el.tree()[parent]
                                    .primary_styles()
                                    .is_some_and(|style| style.get_display().inside() == DisplayInside::Flex)
                            }))
                    }
                    _ => false,
                };
            drop(old_primary);
            if derive {
                let derived = match new_primary {
                    Some(style) => LayoutStyles::derive(style, el.collapsed_border(), el.is_fieldset()),
                    None => LayoutStyles::default(),
                };
                // SAFETY: the traversal has exclusive access to `el`, and nothing reads its
                // layout styles while it runs.
                if let Some(stylo) = el.try_stylo_element_data() {
                    unsafe { stylo.set_layout_styles(derived) };
                }
            } else if replaced {
                #[cfg(debug_assertions)]
                if let (Some(style), Some(stylo)) = (new_primary, el.try_stylo_element_data()) {
                    stylo
                        .layout_styles()
                        .assert_current(style, el.collapsed_border(), el.is_fieldset(), el.id);
                }
            }
            if collapsed_edges_changed {
                data.damage.insert(crate::layout::damage::BOX_DAMAGE);
            }
            let after = &context.thread_local.statistics;
            crate::resolve::phase_profile::style_stats([
                after.elements_styled - before.elements_styled,
                after.elements_matched - before.elements_matched,
                after.styles_shared - before.styles_shared,
                after.styles_reused - before.styles_reused,
            ]);

            sync_pseudo_element_styles(el, &data, &self.nodes_needing_style_image_flush);
            if let Some(style) = data.styles.get_primary() {
                el.record_out_of_flow(style);
                if !style.slow_clone_text_box_trim().is_empty() {
                    self.text_box_trim_styles.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                if style.get_column().column_span == style::values::computed::ColumnSpan::All {
                    self.column_span_styles.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                if style.get_box().scroll_snap_type != style::values::computed::ScrollSnapType::none() {
                    self.scroll_snap_styles.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                // pyro: list items number their markers with the `list-item` counter
                // (`layout::counters`).
                if style.get_box().display.is_list_item() {
                    self.generated_counters.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }

            if data.styles.get_primary().is_some_and(|style| {
                style.clone_position() == style::computed_values::position::T::Sticky
            }) {
                self.sticky_nodes.lock().unwrap().push(el.id);
            }

            if el.element_data().is_some_and(|element| element.marker_style.is_some()) {
                self.marker_items_restyled.lock().unwrap().push(el.id);
            }

            // pyro: a scroll marker group or a scroll target group (`layout::scroll_markers`).
            if data.styles.get_primary().is_some_and(|style| {
                style.get_box().slow_clone_scroll_marker_group().generates_group()
                    || style.get_box().slow_clone_scroll_target_group() != style::values::computed::ScrollTargetGroup::None
            }) {
                self.scroll_marker_groups.store(true, std::sync::atomic::Ordering::Relaxed);
            }

            // One empty check for an element without eager pseudo styles.
            if !data.styles.pseudos.is_empty() {
                use style::selector_parser::PseudoElement;
                let before = data.styles.pseudos.get(&PseudoElement::Before);
                let after = data.styles.pseudos.get(&PseudoElement::After);
                if before.is_some() || after.is_some() {
                    self.generated_before_after
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    if before.into_iter().chain(after).any(|style| crate::layout::counters::reads_counters(style)) {
                        self.generated_counters.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
            // pyro: a `::first-letter` style that came, went or changed restyles the inline
            // layout holding the letter (`layout::first_letter::style_changed`).
            {
                use style::selector_parser::PseudoElement;
                let new_first_letter = match data.styles.pseudos.is_empty() {
                    true => None,
                    false => data.styles.pseudos.get(&PseudoElement::FirstLetter),
                };
                if new_first_letter.is_some() {
                    self.first_letter_styles.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                let changed = match (&old_first_letter, new_first_letter) {
                    (None, None) => false,
                    (Some(old), Some(new)) => {
                        !Arc::ptr_eq(old, new)
                            && matches!(
                                RestyleDamage::compute_style_difference::<&Node>(old, new).change,
                                style::matching::StyleChange::Changed { .. }
                            )
                    }
                    _ => true,
                };
                if changed {
                    self.first_letter_changed.lock().unwrap().push(el.id);
                }
                // pyro: as for `::first-line` (`layout::first_line::style_changed`).
                let new_first_line = match data.styles.pseudos.is_empty() {
                    true => None,
                    false => data.styles.pseudos.get(&PseudoElement::FirstLine),
                };
                if new_first_line.is_some() {
                    self.first_line_styles.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                let line_changed = match (&old_first_line, new_first_line) {
                    (None, None) => false,
                    (Some(old), Some(new)) => {
                        !Arc::ptr_eq(old, new)
                            && matches!(
                                RestyleDamage::compute_style_difference::<&Node>(old, new).change,
                                style::matching::StyleChange::Changed { .. }
                            )
                    }
                    _ => true,
                };
                if line_changed {
                    self.first_line_changed.lock().unwrap().push(el.id);
                }
            }

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

/// pyro: whether `node`'s primary style is `position: sticky`.
pub(crate) fn is_sticky(node: &Node) -> bool {
    node.primary_styles()
        .is_some_and(|style| style.clone_position() == style::computed_values::position::T::Sticky)
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
        if LayoutStyles::affected_by(diff.damage) {
            let derived = LayoutStyles::derive(&pe_style, pe_node.collapsed_border(), false);
            // SAFETY: as for `pe_data` above (only the owner's `process_preorder` reaches it).
            unsafe { stylo_data.set_layout_styles(derived) };
        } else {
            #[cfg(debug_assertions)]
            stylo_data
                .layout_styles()
                .assert_current(&pe_style, pe_node.collapsed_border(), false, pe_node_id);
        }
        pe_node.record_out_of_flow(&pe_style);
        pe_data.styles.primary = Some(pe_style);
        pe_data.set_restyled();
    }
}

/// The colours of a style's four visible borders (top, right, bottom, left; `None` for one with no
/// width or style) with `currentColor` resolved: what a collapsed table's `CollapsedEdges` keep of
/// them.
fn resolved_border_colors(style: &ComputedValues) -> [Option<AbsoluteColor>; 4] {
    use style::values::computed::{BorderStyle, Color};
    let border = style.get_border();
    let current = style.clone_color();
    let resolve = |width: f32, border_style: BorderStyle, color: &Color| {
        (width > 0.0 && !matches!(border_style, BorderStyle::None | BorderStyle::Hidden))
            .then(|| color.resolve_to_absolute(&current))
    };
    [
        resolve(border.border_top_width.0.to_f32_px(), border.border_top_style, &border.border_top_color),
        resolve(border.border_right_width.0.to_f32_px(), border.border_right_style, &border.border_right_color),
        resolve(border.border_bottom_width.0.to_f32_px(), border.border_bottom_style, &border.border_bottom_color),
        resolve(border.border_left_width.0.to_f32_px(), border.border_left_style, &border.border_left_color),
    ]
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
                .any(|image| matches!(crate::util::unfiltered_image(image), Image::Url(_)))
                || stored.iter().any(Option::is_some);
        }
        std::iter::zip(style_images, stored).any(|(style_image, stored)| {
            // pyro: a `filter()` image loads the image it filters.
            match (crate::util::unfiltered_image(style_image), stored) {
                // pyro: or another request for it (its modifiers changed).
                (Image::Url(ComputedUrl::Valid(url, modifiers)), Some(data)) => {
                    **url != *data.url || !crate::net::asks_same_request(&data.options, modifiers.as_deref())
                }
                (Image::Url(ComputedUrl::Valid(..)), None) => true,
                (_, Some(_)) => true,
                (_, None) => false,
            }
        })
    }

    // pyro: with the `url()`s of the layers' `cross-fade()`s (`util::image_slots`).
    let slots = |images: &[Image]| -> Vec<Image> { crate::util::image_slots(images).into_iter().cloned().collect() };
    node.data.downcast_element().is_some_and(|el| {
        out_of_sync(&slots(&style.get_background().background_image.0), &el.background_images)
            || out_of_sync(&slots(&style.get_svg().mask_image.0), &el.mask_images)
            || out_of_sync(&crate::layout::damage::border_image_sources(style), &el.border_images)
            || out_of_sync(std::slice::from_ref(&style.get_list().list_style_image), &el.list_style_images)
            || out_of_sync(crate::layout::damage::shape_image(style), &el.shape_images)
            // pyro: an element whose `content` is one image shows it (`flush_content_image`).
            || {
                use style::values::computed::{Content, ContentItem};
                let wants = matches!(&style.get_counters().content,
                    Content::Items(items) if matches!(&items.items[..], [ContentItem::Image(_)]));
                (wants || el.content_image.is_some() || el.content_gradient) && !node.is_anonymous()
            }
            // pyro: the SVG documents its `clip-path`, `filter` and `mask` reference.
            || {
                #[cfg(feature = "svg")]
                let out_of_sync = crate::node::references_out_of_sync(el, style);
                #[cfg(not(feature = "svg"))]
                let out_of_sync = false;
                out_of_sync
            }
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

/// pyro: `:has()` anchors follow insertions and removals through Stylo's relative
/// selector invalidator (`invalidate_relative_selectors_for_insertion`/`_removal`), and
/// emptiness and sibling-position changes inside an argument through its tree-structural
/// dependencies (`invalidate_relative_selectors_for_ts_state`).
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

    // Arguments that match by position or emptiness: the element that came or went is not
    // what the argument matches, the sibling (or parent) whose condition flipped is.

    #[test]
    fn first_child_argument_follows_an_unrelated_sibling() {
        let (mut d, body) = make_doc(".a:has(> .x:first-child) { order: 7 }", "has-first-child");
        let mut m = d.mutate();
        let (a, x) = (div(&mut m, "a"), div(&mut m, "x"));
        m.append_children(a, &[x]);
        m.append_children(body, &[a]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 7);
        let mut m = d.mutate();
        let z = div(&mut m, "z");
        m.insert_nodes_before(x, &[z]);
        drop(m);
        assert!(hint(&d, a).contains(RestyleHint::RESTYLE_SELF), "the anchor restyles itself");
        d.resolve(0.0);
        assert_eq!(order(&d, a), 0, ".z inserted before .x");
        let mut m = d.mutate();
        m.remove_node(z);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 7, ".z removed");
    }

    #[test]
    fn last_child_argument_follows_an_appended_sibling() {
        let (mut d, body) = make_doc(".a:has(> .x:last-child) { order: 7 }", "has-last-child");
        let mut m = d.mutate();
        let (a, x) = (div(&mut m, "a"), div(&mut m, "x"));
        m.append_children(a, &[x]);
        m.append_children(body, &[a]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 7);
        let mut m = d.mutate();
        let z = div(&mut m, "z");
        m.append_children(a, &[z]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 0, ".z appended after .x");
        let mut m = d.mutate();
        m.remove_node(z);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 7, ".z removed");
        // A same-parent move: `.x` leaves its position and comes back last.
        let mut m = d.mutate();
        let y = div(&mut m, "y");
        m.append_children(a, &[y]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 0);
        let mut m = d.mutate();
        m.append_children(a, &[x]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 7, ".x moved last");
    }

    #[test]
    fn nth_child_argument_follows_a_sibling_inserted_before_it() {
        let (mut d, body) = make_doc(".a:has(> .x:nth-child(2)) { order: 7 }", "has-nth-child");
        let mut m = d.mutate();
        let (a, x) = (div(&mut m, "a"), div(&mut m, "x"));
        m.append_children(a, &[x]);
        m.append_children(body, &[a]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 0);
        let mut m = d.mutate();
        let z = div(&mut m, "z");
        m.insert_nodes_before(x, &[z]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 7, ".z inserted before .x");
        // Not at the front: `.x` is a later sibling of the change.
        let mut m = d.mutate();
        let w = div(&mut m, "w");
        m.insert_nodes_before(x, &[w]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 0, ".w inserted between");
        let mut m = d.mutate();
        m.remove_node(z);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 7, ".z removed");
    }

    #[test]
    fn nth_last_child_argument_follows_a_sibling_appended_after_it() {
        let (mut d, body) =
            make_doc(".a:has(> .x:nth-last-child(2)) { order: 7 }", "has-nth-last-child");
        let mut m = d.mutate();
        let (a, x) = (div(&mut m, "a"), div(&mut m, "x"));
        m.append_children(a, &[x]);
        m.append_children(body, &[a]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 0);
        let mut m = d.mutate();
        let z = div(&mut m, "z");
        m.append_children(a, &[z]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 7, ".z appended after .x");
    }

    #[test]
    fn empty_argument_follows_the_children_of_its_element() {
        let (mut d, body) = make_doc(".a:has(> .p:empty) { order: 7 }", "has-empty");
        let mut m = d.mutate();
        let (a, p, q) = (div(&mut m, "a"), div(&mut m, "p"), div(&mut m, "q"));
        m.append_children(p, &[q]);
        m.append_children(a, &[p]);
        m.append_children(body, &[a]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 0);
        let mut m = d.mutate();
        m.remove_node(q);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 7, ".q removed, .p empty");
        let mut m = d.mutate();
        let q2 = div(&mut m, "q");
        m.append_children(p, &[q2]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 0, ".q appended again");
        // Text nodes count for `:empty` too, though no `:has()` argument matches them.
        let mut m = d.mutate();
        m.remove_and_drop_all_children(p);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 7, "all children dropped");
        let mut m = d.mutate();
        let t = m.create_text_node("text");
        m.append_children(p, &[t]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 0, "a text node");
        let mut m = d.mutate();
        m.remove_node(t);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 7, "the text node removed");
    }

    #[test]
    fn dropping_all_children_leaves_their_anchors() {
        let (mut d, body) = make_doc(".a:has(.x) { order: 7 }", "has-drop-all");
        let mut m = d.mutate();
        let (a, p, x) = (div(&mut m, "a"), div(&mut m, "p"), div(&mut m, "x"));
        m.append_children(p, &[x]);
        m.append_children(a, &[p]);
        m.append_children(body, &[a]);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 7);
        let mut m = d.mutate();
        m.remove_and_drop_all_children(p);
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, a), 0, ".x dropped with its siblings");
    }

    /// A link to the document itself (`href=""`) is visited, yet it still matches `:link` (as
    /// every link does for Chromium's regular style), and inside `:has()` it is unvisited, as
    /// Chromium takes every link there (WPT `css/selectors/invalidation/link-pseudo-in-has.html`,
    /// `css/selectors/has-visited.html`): `:has(> :link)` matches its parent and
    /// `:has(> :visited)` does not, whether `href` was set before or after the insertion.
    #[test]
    fn a_link_to_the_document_is_a_link_and_unvisited_inside_has() {
        let (mut d, body) = make_doc(
            "a:link { order: 1 } a:visited { order: 3 } a:link { z-index: 5 } \
             .p:has(> :link) { order: 7 } .p:has(> :visited) { z-index: 9 }",
            "has-visited-link",
        );
        d.set_base_url("http://test/page.html");
        let z_index = |d: &BaseDocument, id: NodeId| {
            use style::values::generics::position::ZIndex;
            match d.nodes[id].primary_styles().unwrap().get_position().z_index {
                ZIndex::Integer(z) => z,
                ZIndex::Auto => 0,
            }
        };
        let mut m = d.mutate();
        let (p, q) = (div(&mut m, "p"), div(&mut m, "p"));
        // `href` set before the insertion, as the WPT page does.
        let a = m.create_element(qual_name!("a"), vec![]);
        m.set_attribute(a, qual_name!("href"), "");
        m.append_children(p, &[a]);
        let b = m.create_element(qual_name!("a"), vec![]);
        m.append_children(q, &[b]);
        m.append_children(body, &[p, q]);
        drop(m);
        d.resolve(0.0);
        // `order` is not visited-dependent: the visited style's `order: 3` stays out.
        assert_eq!(order(&d, a), 1, "a:visited sets colours only");
        assert_eq!(z_index(&d, a), 5, "a:link applies to the visited link");
        assert_eq!(order(&d, p), 7, ":has(> :link) matches");
        assert_eq!(z_index(&d, p), 0, ":has(> :visited) does not");
        assert_eq!(order(&d, q), 0, "no link yet");

        // `href` set after the insertion: invalidation reaches the anchor.
        let mut m = d.mutate();
        m.set_attribute(b, qual_name!("href"), "");
        drop(m);
        d.resolve(0.0);
        assert_eq!(order(&d, q), 7, ":has(> :link) matches after the change");
        assert_eq!(z_index(&d, q), 0, ":has(> :visited) still does not");
    }
}

/// pyro: Stylo skips the eager `::before`/`::after` match and cascade when no rule
/// that can generate a box matches (`Stylist::may_generate_box_for_pseudo`). What generates,
/// and the styles Stylo keeps, must not change.
#[cfg(test)]
mod style_change_tests {
    use super::*;
    use crate::{Attribute, BaseDocument, DocumentConfig, qual_name};
    use blitz_traits::shell::{ColorScheme, Viewport};

    #[test]
    fn animation_timeline_cascades() {
        let (doc, div) = make_doc(".a { animation: g 1s; animation-timeline: scroll(nearest); scroll-timeline-name: --t }", "atl");
        let style = doc.nodes[div].primary_styles().unwrap();
        use style_traits::ToCss;
        let ui = style.get_ui();
        assert_eq!(ui.animation_timeline_at(0).to_css_string(), "scroll()");
        assert_eq!(ui.scroll_timeline_name_at(0).to_css_string(), "--t");
    }

    /// `<html><head><style>{css}</style></head><body><div class=a>`, resolved.
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
        let div = m.create_element(qual_name!("div"), vec![Attribute { name: qual_name!("class"), value: "a".into() }]);
        m.append_children(body, &[div]);
        m.append_children(html, &[head, body]);
        m.append_children(root_id, &[html]);
        drop(m);
        doc.resolve(0.0);
        (doc, div)
    }

    fn set_class(doc: &mut BaseDocument, id: NodeId, class: &str) {
        let mut m = doc.mutate();
        m.set_attribute(id, qual_name!("class"), class);
        drop(m);
    }

    fn order(style: &style::properties::ComputedValues) -> i32 {
        style.get_position().order
    }

    #[test]
    fn replaced_styles_are_recorded_once_tracked_with_the_first_old_and_last_new() {
        let (mut d, div) = make_doc(".a { order: 1 } .b { order: 2 } .c { order: 3 }", "style-changes");
        // Nothing is recorded until asked: the first styles above are no change either way.
        set_class(&mut d, div, "b");
        d.resolve(0.0);
        assert!(d.take_style_changes().is_empty(), "not tracked");
        d.set_track_style_changes(true);
        set_class(&mut d, div, "c");
        d.resolve(0.0);
        let changes = d.take_style_changes();
        assert_eq!(changes.len(), 1);
        assert_eq!((changes[0].node, order(&changes[0].before), order(&changes[0].after)), (div, 2, 3));
        assert!(d.take_style_changes().is_empty(), "taken");
        // Two restyles before a take: the style before the first and after the last.
        set_class(&mut d, div, "a");
        d.resolve(0.0);
        set_class(&mut d, div, "b");
        d.resolve(0.0);
        let changes = d.take_style_changes();
        assert_eq!(changes.len(), 1);
        assert_eq!((order(&changes[0].before), order(&changes[0].after)), (3, 2));
        // A first style is no change: a new element appears in no entry.
        let mut m = d.mutate();
        let fresh = m.create_element(qual_name!("div"), vec![Attribute { name: qual_name!("class"), value: "c".into() }]);
        m.append_children(div, &[fresh]);
        drop(m);
        d.resolve(0.0);
        assert!(d.take_style_changes().is_empty(), "a first style");
        // Turning tracking off drops what was recorded.
        set_class(&mut d, div, "c");
        d.resolve(0.0);
        d.set_track_style_changes(false);
        assert!(d.take_style_changes().is_empty());
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
            visited_styles_enabled: true,
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
