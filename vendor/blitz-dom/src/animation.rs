//! pyro: animated values the embedder samples into the cascade.
//!
//! pyro keeps the timing of every animation (CSS animations, CSS transitions
//! and Web Animations) on its own side and tells the document, per element,
//! which keyframe effects are in effect and how far along each one is
//! ([`ElementEffects`], [`BaseDocument::set_sampled_animations`]). This module
//! turns them into the declaration blocks Stylo cascades at the animations and
//! transitions levels (`TElement::animation_rule` and `transition_rule` in
//! `stylo.rs`):
//!
//! - Keyframes are resolved against the element's base style (its style
//!   without those two levels), as Stylo's servo animations resolve theirs:
//!   each keyframe's declarations are cascaded on top of the base rules, so
//!   `em`, `currentcolor`, `var()` and `inherit` resolve as for the element.
//!   The result is kept until the base rules or the parent's style change.
//! - Each property is sampled as Web Animations samples a keyframe effect: the
//!   effect's easing turns the iteration progress into a transformed progress;
//!   the keyframes around it (with neutral 0% and 100% keyframes taking the
//!   underlying value where none is given) are interpolated through the first
//!   keyframe's easing; `add` and `accumulate` compose with the underlying
//!   value; values that do not interpolate flip at the midpoint.
//! - Effects are composed in the order given: a later effect's value replaces
//!   (or composes with) an earlier one's.
//!
//! Stylo's own animation engine stays idle: the embedder's UA sheet declares
//! `animation` and `transition` `none !important`, so these blocks are the only
//! animated values. An element is restyled only when its sampled values
//! change, so a paused or finished-and-filling animation costs nothing per
//! frame.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use blitz_traits::node_id::NodeId;
use style::computed_value_flags::ComputedValueFlags;
use style::context::{CascadeInputs, SharedStyleContext, StyleContext, ThreadLocalStyleContext};
use style::dom::{TElement, TNode};
use style::global_style_data::GLOBAL_STYLE_DATA;
use style::invalidation::element::restyle_hints::RestyleHint;
use style::properties::animated_properties::{AnimationValue, AnimationValueMap};
use style::properties::{
    ComputedValues, Importance, LonghandId, OwnedPropertyDeclarationId, PropertyDeclaration,
    PropertyDeclarationBlock, PropertyDeclarationId,
};
use style::rule_tree::{CascadeLevel, CascadeOrigin, RuleCascadeFlags, StrongRuleNode};
use style::servo_arc::Arc as ServoArc;
use style::shared_lock::{Locked, StylesheetGuards};
use style::style_resolver::{PseudoElementResolution, StyleResolverForElement};
use style::stylesheets::keyframes_rule::KeyframesStepValue;
use style::stylesheets::layer_rule::LayerOrder;
use style::stylesheets::CssRuleType;
use style::stylist::RuleInclusion;
use style::traversal_flags::TraversalFlags;
use style::values::animated::Procedure;
use style::values::computed::TimingFunction;
use style::values::generics::easing::BeforeFlag;
use style::Atom;
use style::FxHashMap;

use crate::node::Node;
use crate::BaseDocument;

/// How a keyframe's value combines with the value underneath it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Composite {
    #[default]
    Replace,
    Add,
    Accumulate,
}

/// Where an effect's keyframes come from.
#[derive(Clone, Debug, PartialEq)]
pub enum Keyframes {
    /// The `@keyframes` rule `name` that applies to the element (a CSS
    /// animation). `easing` is the animation's `animation-timing-function`
    /// (each keyframe's unless it declares its own; `None`: `ease`) and
    /// `composite` its `animation-composition`.
    Css {
        name: Atom,
        easing: Option<Arc<str>>,
        composite: Composite,
    },
    /// Keyframes written as CSS text (Web Animations, CSS transitions).
    Text(Arc<TextKeyframes>),
}

/// Keyframes as CSS text, in offset order.
#[derive(Debug, Default, PartialEq)]
pub struct TextKeyframes {
    pub frames: Vec<TextKeyframe>,
}

#[derive(Debug, PartialEq)]
pub struct TextKeyframe {
    /// The computed keyframe offset, 0 to 1.
    pub offset: f64,
    /// The keyframe's easing (`None`: `linear`).
    pub easing: Option<String>,
    /// `None`: the effect's composite operation.
    pub composite: Option<Composite>,
    /// Declarations, as in a `style` attribute.
    pub declarations: String,
}

/// One keyframe effect in effect now.
#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    pub keyframes: Keyframes,
    /// The iteration progress (the directed progress, before the effect's
    /// easing).
    pub progress: f64,
    /// The before flag the effect's easing takes (step functions).
    pub before: bool,
    /// The effect's easing (`None`: `linear`).
    pub easing: Option<Arc<str>>,
    /// The effect's composite operation (keyframes without one of their own).
    pub composite: Composite,
    /// pyro: a CSS animation driven by a scroll or view progress timeline (`animation-timeline`):
    /// its progress comes from the scroll position at each sample (`scroll_timeline`), not from
    /// `progress`.
    pub timeline: Option<TimelineDriven>,
}

/// pyro: what a scroll-driven CSS animation's progress needs besides the scroll position: the index
/// of the animation in its target's `animation-*` lists, and its timing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineDriven {
    pub index: usize,
    pub iterations: f64,
    /// 0 `normal`, 1 `reverse`, 2 `alternate`, 3 `alternate-reverse`.
    pub direction: u8,
    /// 0 `none`, 1 `forwards`, 2 `backwards`, 3 `both`.
    pub fill: u8,
    /// `animation-delay` (ms) and `animation-duration` (ms; `None`: `auto`): on a progress-based
    /// timeline they are shares of the whole (Web Animations 2, "converting time values").
    pub delay_ms: f64,
    pub duration_ms: Option<f64>,
    /// On the element's `::before` (1) or `::after` (2), or the element itself (0).
    pub pseudo: u8,
}

impl TimelineDriven {
    /// The iteration progress at `p` through the attachment range, and the before flag; `None`
    /// outside the range without a fill that covers it.
    /// pyro: `at_end`: the timeline is at the end of its range, where the active end is inclusive;
    /// elsewhere progress at the active end is past it, in the after phase (Web Animations 2; WPT
    /// scroll-animations `view-timelines/range-boundary.tentative`).
    fn iteration_progress(&self, p: f64, at_end: bool) -> Option<(f64, bool)> {
        let n = if self.iterations.is_finite() { self.iterations.max(0.0) } else { 1.0 };
        let backwards = self.fill & 2 != 0;
        let forwards = self.fill & 1 != 0;
        // The start delay and the iteration duration as shares of the timeline: an `auto` duration
        // fills it (no delay); times are proportions of delay + duration x iterations.
        let (delay, iteration) = match self.duration_ms {
            None => (0.0, if n > 0.0 { 1.0 / n } else { 0.0 }),
            Some(duration) => {
                let total = self.delay_ms + duration * n;
                if total > 0.0 { (self.delay_ms / total, duration / total) } else { (0.0, 0.0) }
            }
        };
        let active = iteration * n;
        let local = p - delay;
        let overall = if local < 0.0 && !(active == 0.0 && p >= 0.0) {
            if !backwards {
                return None;
            }
            0.0
        } else if local > active
            || (!at_end && active > 0.0 && (local - active).abs() < 1e-9)
            || (active == 0.0 && local >= 0.0)
            || (active == 0.0 && p >= 0.0)
        {
            if !forwards {
                return None;
            }
            n
        } else {
            local / iteration
        };
        let mut iteration = overall.floor();
        let mut fraction = overall - iteration;
        if fraction == 0.0 && overall > 0.0 && overall >= n {
            iteration -= 1.0;
            fraction = 1.0;
        }
        let odd = iteration as i64 % 2 == 1;
        let reversed = match self.direction {
            1 => true,
            2 => odd,
            3 => !odd,
            _ => false,
        };
        let fraction = if n == 0.0 { 0.0 } else { fraction };
        Some((if reversed { 1.0 - fraction } else { fraction }, local < 0.0))
    }
}

/// The effects of one element, each list in composite order: `transitions`
/// cascade at the transitions level, `animations` (CSS animations, then
/// script animations) at the animations level.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ElementEffects {
    pub transitions: Vec<Effect>,
    pub animations: Vec<Effect>,
    /// pyro: animations that are current but not in effect (in their delay): no values, but the
    /// properties they animate act as if `will-change` named them (Web Animations 1, "side
    /// effects of animation"), as those of the effects in effect do.
    pub current: Vec<Effect>,
    /// pyro: animations in effect on the element's `::marker` (a list item's): their values go
    /// into the marker's style (`ElementData::sampled_marker`, `layout::list`), not the element's.
    pub marker: Vec<Effect>,
    /// pyro: animations in effect on a top-layer element's `::backdrop`: their values go into
    /// the backdrop's style (`ElementData::sampled_backdrop`, paint).
    pub backdrop: Vec<Effect>,
    /// pyro: the effects on the element's `::before` (`[0]`) and `::after` (`[1]`): resolved
    /// against the pseudo-element's own style and cascaded into it.
    pub pseudos: [PseudoEffects; 2],
}

/// pyro: the effects on one of an element's `::before`/`::after`, each list in composite order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PseudoEffects {
    pub transitions: Vec<Effect>,
    pub animations: Vec<Effect>,
}

impl PseudoEffects {
    pub fn is_empty(&self) -> bool {
        self.transitions.is_empty() && self.animations.is_empty()
    }

    fn all(&self) -> impl Iterator<Item = &Effect> {
        self.animations.iter().chain(&self.transitions)
    }
}

/// pyro: `::before` and `::after`, as `ElementEffects::pseudos` indexes them.
const PSEUDOS: [style::selector_parser::PseudoElement; 2] =
    [style::selector_parser::PseudoElement::Before, style::selector_parser::PseudoElement::After];

impl ElementEffects {
    pub fn is_empty(&self) -> bool {
        self.transitions.is_empty()
            && self.animations.is_empty()
            && self.current.is_empty()
            && self.marker.is_empty()
            && self.backdrop.is_empty()
            && self.pseudos.iter().all(PseudoEffects::is_empty)
    }

    /// pyro: every effect, in effect or only current.
    fn all(&self) -> impl Iterator<Item = &Effect> {
        self.own().chain(&self.marker).chain(&self.backdrop)
    }

    /// pyro: the effects on the element itself.
    fn own(&self) -> impl Iterator<Item = &Effect> {
        self.animations.iter().chain(&self.transitions).chain(&self.current)
    }
}

/// A document's sampled animations ([`BaseDocument::set_sampled_animations`]).
#[derive(Default)]
pub(crate) struct SampledAnimations {
    elements: FxHashMap<NodeId, ElementState>,
    /// Parsed easings by text (`None`: not an easing).
    easings: FxHashMap<Arc<str>, Option<TimingFunction>>,
    /// Elements whose keyframes must be resolved against the style the next
    /// traversal gives them.
    pending: bool,
}

impl SampledAnimations {
    /// A font arrival can change computed keyframes while rules and parent identity stay the same.
    pub(crate) fn invalidate_bases(&mut self) {
        for element in self.elements.values_mut() {
            element.base = None;
            element.pseudos = Default::default();
        }
        self.pending |= !self.is_empty();
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }
}

#[derive(Default)]
struct ElementState {
    /// pyro: the effects as the page gave them (scroll-driven ones with their timeline).
    given: ElementEffects,
    /// The effects in effect now: the given ones, scroll-driven ones at their progress now.
    effects: ElementEffects,
    /// Keyframes resolved against `base`, by effect keyframes.
    resolved: Vec<(Keyframes, Resolved)>,
    base: Option<BaseKey>,
    /// pyro: the same for the element's `::before` and `::after` (`ElementEffects::pseudos`).
    pseudos: [PseudoState; 2],
    /// The values sampled into the element's cascade.
    sampled: Sampled,
}

/// pyro: the resolved keyframes of the effects on one pseudo-element, against its base style.
#[derive(Default)]
struct PseudoState {
    resolved: Vec<(Keyframes, Resolved)>,
    base: Option<BaseKey>,
}

impl PseudoState {
    fn resolved_all(&self, effects: &PseudoEffects) -> bool {
        self.base.is_some() && effects.all().all(|e| self.resolved.iter().any(|(k, _)| *k == e.keyframes))
    }
}

#[derive(Clone, Default, PartialEq)]
struct Sampled {
    animations: Option<AnimationValueMap>,
    transitions: Option<AnimationValueMap>,
    /// pyro: the values of the effects on the element's `::marker`.
    marker: Option<AnimationValueMap>,
    /// pyro: the values of the effects on the element's `::backdrop`.
    backdrop: Option<AnimationValueMap>,
    /// pyro: the values of the effects on `::before` and `::after` (animations, transitions).
    pseudos: [(Option<AnimationValueMap>, Option<AnimationValueMap>); 2],
    /// pyro: an effect animates a property that makes a stacking context
    /// (`ElementData::animation_stacking_context`).
    stacking_context: bool,
}

/// What resolved keyframes depend on besides their declarations.
#[derive(Clone)]
struct BaseKey {
    rules: StrongRuleNode,
    /// The parent's style (inherited values), compared by address.
    parent: usize,
    base_style: ServoArc<ComputedValues>,
    siblings: SiblingDependency,
    /// pyro: the keyframes are a `::before`/`::after`'s (with the originating element's style):
    /// their values cascade as the pseudo-element's, inheriting from the element.
    pseudo: Option<(style::selector_parser::PseudoElement, ServoArc<ComputedValues>)>,
}

/// An effect's keyframes resolved against the element's base style.
#[derive(Default)]
struct Resolved {
    /// Sibling position used while computing a keyframe, if any frame depended on it.
    siblings: SiblingDependency,
    allow_size_keywords: bool,
    frames: Vec<ResolvedFrame>,
    /// The properties the keyframes set, with their base (underlying) value.
    properties: Vec<(OwnedPropertyDeclarationId, Option<AnimationValue>)>,
    /// The easing of the neutral keyframes made where 0% or 100% is missing.
    implicit_easing: Option<TimingFunction>,
}

#[derive(Clone, Copy, Default, PartialEq)]
struct SiblingDependency {
    index: Option<u32>,
    count: Option<u32>,
}

impl SiblingDependency {
    fn capture(flags: ComputedValueFlags, node: &Node, caches: &mut style::context::TreeCountingCaches) -> Self {
        if !flags.intersects(ComputedValueFlags::tree_counting_function_flags()) {
            return Self::default();
        }
        let position = caches.get_or_compute(&node);
        Self {
            index: flags.contains(ComputedValueFlags::USES_SIBLING_INDEX).then_some(position.sibling_index),
            count: flags.contains(ComputedValueFlags::USES_SIBLING_COUNT).then_some(position.sibling_count),
        }
    }

    fn is_current(self, node: &Node, caches: &mut style::context::TreeCountingCaches) -> bool {
        if self == Self::default() {
            return true;
        }
        let position = caches.get_or_compute(&node);
        self.index.is_none_or(|i| i == position.sibling_index) && self.count.is_none_or(|c| c == position.sibling_count)
    }
}

struct ResolvedFrame {
    offset: f64,
    easing: Option<TimingFunction>,
    composite: Option<Composite>,
    /// By index into `Resolved::properties`.
    values: Vec<Option<AnimationValue>>,
}

impl BaseDocument {
    pub(crate) fn animated_marker_style(
        &self, node: &Node, primary: &ServoArc<ComputedValues>, base: Option<&ServoArc<ComputedValues>>,
    ) -> Option<ServoArc<ComputedValues>> {
        let block = node.element_data()?.sampled_marker.as_ref()?;
        let guard = self.guard.read();
        let guards = StylesheetGuards::same(&guard);
        let tree = self.stylist.rule_tree();
        let path = base.map_or_else(|| tree.root(), |style| style.rules());
        let rules = tree.update_rule_at_level(CascadeLevel::new(CascadeOrigin::Animations), LayerOrder::root(),
            Some(block.borrow_arc()), path, &guards, &mut false)?;
        let inputs = CascadeInputs {
            rules: Some(rules), flags: base.map_or_else(Default::default, |style| style.flags.for_cascade_inputs()),
            ..Default::default()
        };
        Some(self.stylist.compute_pseudo_element_style_with_inputs(inputs, &style::selector_parser::PseudoElement::Marker,
            &guards, Some(primary), Some(node)))
    }

    pub(crate) fn sample_transition_style(&self, origin: NodeId, parent: NodeId, pseudo: style::selector_parser::PseudoElement, style: ServoArc<ComputedValues>, samples: &[(usize,f64)], generated: Option<&(Atom,String)>, budget: &mut usize) -> Option<ServoArc<ComputedValues>> {
        if samples.is_empty() { return Some(style); }
        use style_traits::ToCss;
        let node = self.nodes.get(origin)?;
        let primary = self.nodes[parent].primary_styles()?.clone();
        let _thread = crate::stylo::LayoutThreadScope::enter();
        let guard = self.guard.read();
        let shared = SharedStyleContext {
            traversal_flags: TraversalFlags::empty(), stylist: &self.stylist, options: GLOBAL_STYLE_DATA.options.clone(),
            guards: StylesheetGuards::same(&guard), visited_styles_enabled: true, animations: self.animations.clone(), current_time_for_animations: 0.0,
            snapshot_map: &self.snapshots, registered_speculative_painters: &crate::stylo::RegisteredPaintersImpl,
        };
        let mut thread = ThreadLocalStyleContext::new();
        let mut context = StyleContext { shared: &shared, thread_local: &mut thread };
        let base = BaseKey { rules: style.rules().clone(), parent: 0, base_style: style.clone(), siblings: SiblingDependency::default(), pseudo: Some((pseudo.clone(), primary.clone())) };
        let ui = style.get_ui();
        let mut state = ElementState::default();
        let mut easings = FxHashMap::default();
        for &(index,progress) in samples {
            let item = ui.animation_name_mod(index);
            let Some(name) = item.as_atom() else { continue; };
            let easing = ui.animation_timing_function_mod(index).to_css_string();
            let animation = match animation_name_scope(self, node) {
                Some(scope) => self.stylist.lookup_keyframes_scoped(name, scope, node),
                None => self.stylist.lookup_keyframes(name, node),
            };
            let keyframes = if let Some(animation) = animation {
                *budget = budget.checked_sub(animation.steps.len())?;
                let mut declarations = 0usize;
                for step in &animation.steps {
                    if let KeyframesStepValue::Declarations { block } = &step.value {
                        declarations = declarations.checked_add(block.read_with(&guard).len())?;
                    }
                }
                *budget = budget.checked_sub(declarations.checked_mul(declarations.max(animation.steps.len()))?)?;
                Keyframes::Css { name:name.clone(),easing:Some(easing.into()),composite:Composite::Replace }
            } else if let Some((_,css)) = generated.filter(|(id,_)| id == name) {
                *budget = budget.checked_sub(2)?;
                Keyframes::Text(Arc::new(TextKeyframes { frames:vec![TextKeyframe { offset:0.0,easing:Some(easing),composite:None,declarations:css.clone() },TextKeyframe {offset:1.0,easing:None,composite:None,declarations:String::new()}] }))
            } else { continue; };
            state.resolved.push((keyframes.clone(),resolve(self,node,&base,&keyframes,&mut context,&mut easings)));
            state.effects.animations.push(Effect {keyframes,progress,before:false,easing:None,composite:Composite::Replace,timeline:None});
        }
        if let Some(values) = sample(&state,&mut easings).animations {
            let block = ServoArc::new(self.guard.wrap(PropertyDeclarationBlock::from_animation_value_map(&values)));
            if let Some(rules) = self.stylist.rule_tree().update_rule_at_level(CascadeLevel::new(CascadeOrigin::Animations),LayerOrder::root(),Some(block.borrow_arc()),style.rules(),&shared.guards,&mut false) {
                return Some(cascade_pseudo(node,&mut context,rules,&style,&pseudo,&primary));
            }
        }
        Some(style)
    }
    /// Replaces the document's sampled animations: the effects of each element
    /// in effect now (an element left out has none). Elements whose sampled
    /// values change are marked for restyle; the next `resolve` cascades them.
    /// pyro: the animations of the frame documents (by frame client; see
    /// `BaseDocument::frame_animations`).
    pub fn set_frame_animations(&mut self, frames: std::collections::HashMap<String, std::sync::Arc<Vec<(u64, ElementEffects)>>>) {
        if frames.is_empty() && self.frame_animations.is_empty() {
            return;
        }
        self.frame_animations = std::sync::Arc::new(frames);
    }

    /// pyro: samples this frame document's animations (`frame`: by render key) into its elements
    /// that carry their key (`data-pyro-key`).
    pub(crate) fn sample_frame_animations(&mut self, frame: Option<&[(u64, ElementEffects)]>) {
        let Some(frame) = frame else {
            if self.has_sampled_animations() {
                self.set_sampled_animations(Vec::new());
            }
            return;
        };
        let attr = markup5ever::LocalName::from("data-pyro-key");
        let keyed: std::collections::HashMap<u64, NodeId> = self
            .nodes
            .iter()
            .filter(|(_, node)| node.element_data().is_some_and(|e| crate::engine_marked(&self.frames.nonce, e)))
            .filter_map(|(id, node)| Some((node.reserved_attr(&attr)?.parse::<u64>().ok()?, id)))
            .collect();
        let effects: Vec<_> = frame.iter().filter_map(|(key, e)| Some((*keyed.get(key)?, e.clone()))).collect();
        self.set_sampled_animations(effects);
    }

    pub fn set_sampled_animations(&mut self, effects: Vec<(NodeId, ElementEffects)>) {
        let mut state = std::mem::take(&mut self.sampled_animations);
        let mut next: FxHashMap<NodeId, ElementState> = FxHashMap::default();
        for (id, effects) in effects {
            if effects.is_empty()
                || !self
                    .nodes
                    .get(id)
                    .is_some_and(|n| n.is_element() && n.flags.is_in_document())
            {
                continue;
            }
            let mut element = state.elements.remove(&id).unwrap_or_default();
            // pyro: in skipped `content-visibility` contents its scroll-driven animations hold
            // where they were (Chromium does not update them; WPT css-contain
            // `content-visibility-animation-with-scroll-timeline-in-auto-subtree`).
            let frozen = element.given == effects && self.in_skipped_contents(id);
            if !frozen {
                element.effects = self.drive(id, &effects);
            }
            element.given = effects;
            element.resolved.retain(|(k, _)| element.effects.all().any(|e| e.keyframes == *k));
            for (state, effects) in element.pseudos.iter_mut().zip(&element.effects.pseudos) {
                state.resolved.retain(|(k, _)| effects.all().any(|e| e.keyframes == *k));
            }
            next.insert(id, element);
        }
        // Elements without effects any more lose their values.
        for (id, _) in state.elements.drain() {
            self.apply_sampled(id, Sampled::default());
        }
        state.elements = next;
        let ids: Vec<NodeId> = state.elements.keys().copied().collect();
        for id in ids {
            let element = state.elements.get_mut(&id).unwrap();
            let resolved = element.base.is_some()
                && element.effects.all().all(|e| element.resolved.iter().any(|(k, _)| *k == e.keyframes))
                && element.pseudos.iter().zip(&element.effects.pseudos).all(|(state, effects)| {
                    effects.is_empty() || state.resolved_all(effects)
                });
            if !resolved {
                // Resolved against the style the next traversal computes.
                state.pending = true;
                if let Some(node) = self.nodes.get_mut(id) {
                    node.set_restyle_hint(RestyleHint::RESTYLE_SELF);
                }
                continue;
            }
            let sampled = sample(element, &mut state.easings);
            if sampled != element.sampled {
                element.sampled = sampled;
                self.apply_sampled(id, element.sampled.clone());
            }
        }
        self.sampled_animations = state;
    }

    /// Whether any element has sampled animation values.
    pub fn has_sampled_animations(&self) -> bool {
        !self.sampled_animations.is_empty()
    }

    /// Whether an animation or transition in effect on the node animates a
    /// property that makes a backdrop root (`opacity`, `filter`,
    /// `backdrop-filter`, `mix-blend-mode`, `clip-path`, `mask-image`): Web
    /// Animations has the element act as though it had `will-change` of that
    /// property while the effect is in effect, after it finishes too when it
    /// fills forwards (WPT backdrop-filter-backdrop-root-animation-in-effect).
    pub fn animates_backdrop_root(&self, node_id: NodeId) -> bool {
        use style::properties::{LonghandId, PropertyDeclarationId};
        let Some(data) = self.get_node(node_id).and_then(|node| node.element_data()) else {
            return false;
        };
        if data.sampled_animations.is_none() && data.sampled_transitions.is_none() {
            return false;
        }
        let guard = self.guard.read();
        [&data.sampled_animations, &data.sampled_transitions]
            .into_iter()
            .flatten()
            .any(|block| {
                let block = block.read_with(&guard);
                [
                    LonghandId::Opacity,
                    LonghandId::Filter,
                    LonghandId::BackdropFilter,
                    LonghandId::MixBlendMode,
                    LonghandId::ClipPath,
                    LonghandId::MaskImage,
                ]
                .into_iter()
                .any(|id| block.contains(PropertyDeclarationId::Longhand(id)))
            })
    }

    /// pyro: the starting style of element `id` (its style with its `@starting-style` rules,
    /// css-transitions-2 §3.2): the before-change style of a box that had none (it was
    /// `display: none`). `None`: no `@starting-style` rule applies to it, or it is not styled.
    pub fn starting_style(&self, id: crate::NodeId) -> Option<ServoArc<ComputedValues>> {
        let node = self.nodes.get(id)?;
        let style = node.primary_styles().map(|s| (*s).clone())?;
        if !style::rule_tree::RuleTree::has_starting_style(style.rules()) {
            return None;
        }
        let _layout_thread_scope = crate::stylo::LayoutThreadScope::enter();
        let guard = &self.guard;
        let guards = StylesheetGuards { author: &guard.read(), ua_or_user: &guard.read() };
        let shared = SharedStyleContext {
            traversal_flags: TraversalFlags::empty(),
            stylist: &self.stylist,
            options: GLOBAL_STYLE_DATA.options.clone(),
            guards,
            visited_styles_enabled: true,
            animations: self.animations.clone(),
            current_time_for_animations: 0.0,
            snapshot_map: &self.snapshots,
            registered_speculative_painters: &crate::stylo::RegisteredPaintersImpl,
        };
        let mut thread_local = ThreadLocalStyleContext::new();
        let mut context = StyleContext { shared: &shared, thread_local: &mut thread_local };
        context.thread_local.current_dom_depth = TNode::depth(&node);
        let mut resolver = StyleResolverForElement::new(
            node,
            &mut context,
            RuleInclusion::All,
            PseudoElementResolution::IfApplicable,
        );
        let inputs = CascadeInputs {
            rules: Some(style.rules().clone()),
            visited_rules: None,
            flags: style.flags.for_cascade_inputs(),
            included_cascade_flags: RuleCascadeFlags::STARTING_STYLE,
        };
        let starting = resolver.cascade_style_and_visited_with_default_parents(inputs).0;
        (!starting.get_box().display.is_none()).then_some(starting)
    }

    /// After a style traversal: resolves the keyframes of the elements whose
    /// base style changed (or that had none before), samples them again and
    /// marks those whose values changed for restyle. `true`: another
    /// traversal must run.
    pub(crate) fn resolve_sampled_animations(&mut self) -> bool {
        if self.sampled_animations.is_empty() {
            return false;
        }
        let mut state = std::mem::take(&mut self.sampled_animations);
        state.pending = false;
        let mut restyle = Vec::new();
        {
            let _layout_thread_scope = crate::stylo::LayoutThreadScope::enter();
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
                registered_speculative_painters: &crate::stylo::RegisteredPaintersImpl,
            };
            let mut thread_local = ThreadLocalStyleContext::new();
            let mut context = StyleContext {
                shared: &shared,
                thread_local: &mut thread_local,
            };
            state.elements.retain(|id, element| {
                let Some(node) = self.nodes.get(*id) else {
                    return false;
                };
                if !node.is_element() || !node.flags.is_in_document() {
                    return false;
                }
                // Not styled (inside `display: none`): nothing to sample into.
                let Some(style) = node.primary_styles().map(|s| (*s).clone()) else {
                    return true;
                };
                let rules = shared
                    .stylist
                    .rule_tree()
                    .remove_animation_rules(style.rules());
                let parent = parent_style_address(node);
                let same_base = element
                    .base
                    .as_ref()
                    .is_some_and(|b| b.rules == rules && b.parent == parent && b.siblings.is_current(node, &mut context.thread_local.tree_counting_caches));
                if !same_base {
                    let base_style = if rules == *style.rules() {
                        style.clone()
                    } else {
                        cascade(node, &mut context, rules.clone(), &style)
                    };
                    element.base = Some(BaseKey {
                        rules,
                        parent,
                        siblings: SiblingDependency::capture(base_style.flags, node, &mut context.thread_local.tree_counting_caches),
                        base_style,
                        pseudo: None,
                    });
                    element.resolved.clear();
                }
                let mut still_current = |(_, resolved): &(Keyframes, Resolved)| {
                    resolved.siblings.is_current(node, &mut context.thread_local.tree_counting_caches)
                };
                element.resolved.retain(&mut still_current);
                for ps in &mut element.pseudos {
                    ps.resolved.retain(&mut still_current);
                }
                let base = element.base.clone().unwrap();
                let effects: Vec<Keyframes> = element.effects.all().map(|e| e.keyframes.clone()).collect();
                for keyframes in effects {
                    if element.resolved.iter().any(|(k, _)| *k == keyframes) {
                        continue;
                    }
                    let resolved = resolve(
                        self,
                        node,
                        &base,
                        &keyframes,
                        &mut context,
                        &mut state.easings,
                    );
                    element.resolved.push((keyframes, resolved));
                }
                // pyro: the keyframes of the effects on `::before`/`::after`, against the
                // pseudo-element's style without the values sampled into it.
                for (i, pseudo) in PSEUDOS.iter().enumerate() {
                    if element.effects.pseudos[i].is_empty() {
                        element.pseudos[i] = PseudoState::default();
                        continue;
                    }
                    let Some(pseudo_style) = node.borrow_data().and_then(|d| d.styles.pseudos.get(pseudo).cloned()) else {
                        continue;
                    };
                    let rules = shared.stylist.rule_tree().remove_animation_rules(pseudo_style.rules());
                    let parent = &*style as *const ComputedValues as usize;
                    let ps = &mut element.pseudos[i];
                    if !ps.base.as_ref().is_some_and(|b| b.rules == rules && b.parent == parent && b.siblings.is_current(node, &mut context.thread_local.tree_counting_caches)) {
                        let base_style = if rules == *pseudo_style.rules() {
                            pseudo_style.clone()
                        } else {
                            cascade_pseudo(node, &mut context, rules.clone(), &pseudo_style, pseudo, &style)
                        };
                        let siblings = SiblingDependency::capture(base_style.flags, node, &mut context.thread_local.tree_counting_caches);
                        ps.base = Some(BaseKey { rules, parent, siblings, base_style, pseudo: Some((pseudo.clone(), style.clone())) });
                        ps.resolved.clear();
                    }
                    let base = ps.base.clone().unwrap();
                    let effects: Vec<Keyframes> = element.effects.pseudos[i].all().map(|e| e.keyframes.clone()).collect();
                    for keyframes in effects {
                        if ps.resolved.iter().any(|(k, _)| *k == keyframes) {
                            continue;
                        }
                        let resolved = resolve(self, node, &base, &keyframes, &mut context, &mut state.easings);
                        ps.resolved.push((keyframes, resolved));
                    }
                }
                let sampled = sample(element, &mut state.easings);
                if sampled != element.sampled {
                    element.sampled = sampled;
                    restyle.push(*id);
                }
                true
            });
        }
        for id in &restyle {
            self.apply_sampled(*id, state.elements[id].sampled.clone());
        }
        self.sampled_animations = state;
        !restyle.is_empty()
    }

    /// Puts `sampled` on the element's data (the blocks `animation_rule` and
    /// `transition_rule` answer) and marks it for restyle.
    fn apply_sampled(&mut self, id: NodeId, sampled: Sampled) {
        let Some(node) = self.nodes.get_mut(id) else {
            return;
        };
        let Some(guard) = node.element_data().and_then(|d| d.guard.clone()) else {
            return;
        };
        let block = |map: Option<AnimationValueMap>| {
            map.filter(|m| !m.is_empty()).map(|m| {
                ServoArc::new(guard.wrap(PropertyDeclarationBlock::from_animation_value_map(&m)))
            })
        };
        let (animations, transitions) = (block(sampled.animations), block(sampled.transitions));
        let marker = block(sampled.marker.map(|mut values| {
            let restriction = style::selector_parser::PseudoElement::Marker.property_restriction();
            values.retain(|property, _| match property.as_borrowed() {
                PropertyDeclarationId::Longhand(id) => id.flags().contains(restriction),
                PropertyDeclarationId::Custom(..) => true,
            });
            values
        }));
        let backdrop = block(sampled.backdrop);
        let [(before_a, before_t), (after_a, after_t)] = sampled.pseudos;
        let pseudos = [(block(before_a), block(before_t)), (block(after_a), block(after_t))];
        let Some(data) = node.element_data_mut() else {
            return;
        };
        // pyro: the backdrop's values: paint cascades them into its style.
        if data.sampled_backdrop.is_some() || backdrop.is_some() {
            data.sampled_backdrop = backdrop;
            node.insert_damage(crate::layout::damage::ALL_DAMAGE);
            node.mark_damaged();
        }
        let Some(data) = node.element_data_mut() else {
            return;
        };
        // pyro: the marker's values: the list the item is in builds its markers again.
        if data.sampled_marker.is_some() || marker.is_some() {
            data.sampled_marker = marker;
            let list = node.parent.unwrap_or(id);
            for target in [id, list] {
                if let Some(node) = self.nodes.get_mut(target) {
                    node.insert_damage(crate::layout::damage::ALL_DAMAGE);
                    node.mark_damaged();
                }
            }
        }
        let Some(node) = self.nodes.get_mut(id) else {
            return;
        };
        let Some(data) = node.element_data_mut() else {
            return;
        };
        // pyro: the values on `::before`/`::after`: the element's restyle cascades them into the
        // pseudo-elements' styles (`cascade_sampled_pseudos`).
        let pseudo_had = data.sampled_pseudos.iter().any(|(a, t)| a.is_some() || t.is_some());
        let pseudo_has = pseudos.iter().any(|(a, t)| a.is_some() || t.is_some());
        if pseudo_had || pseudo_has {
            data.sampled_pseudos = pseudos;
            if pseudo_has {
                data.may_have_animations.store(true, Ordering::Relaxed);
            }
            node.set_restyle_hint(RestyleHint::RESTYLE_SELF);
        }
        let Some(data) = node.element_data_mut() else {
            return;
        };
        // pyro: the element becomes or stops being a stacking context: its paint lists change.
        let stacking_changed = data.animation_stacking_context != sampled.stacking_context;
        data.animation_stacking_context = sampled.stacking_context;
        let had = data.sampled_animations.is_some() || data.sampled_transitions.is_some();
        let has = animations.is_some() || transitions.is_some();
        if stacking_changed {
            node.insert_damage(style::selector_parser::RestyleDamage::REBUILD_STACKING_CONTEXT);
            node.set_restyle_hint(RestyleHint::RESTYLE_SELF);
            if !had && !has {
                return;
            }
        }
        let Some(data) = node.element_data_mut() else {
            return;
        };
        if !had && !has {
            return;
        }
        data.sampled_animations = animations;
        data.sampled_transitions = transitions;
        if has {
            data.may_have_animations.store(true, Ordering::Relaxed);
        }
        node.set_restyle_hint(RestyleHint::RESTYLE_SELF);
    }
}

impl BaseDocument {
    /// pyro: `effects` with each scroll-driven one at its progress now (`scroll_timeline`), and
    /// those outside their range without a fill (or on an inactive timeline) left out.
    fn drive(&self, id: NodeId, effects: &ElementEffects) -> ElementEffects {
        let driven = |e: &Effect| e.timeline.is_some();
        let any = effects.animations.iter().any(driven)
            || effects.pseudos.iter().any(|p| p.animations.iter().any(driven));
        if !any {
            return effects.clone();
        }
        let node = self.get_node(id);
        let style_of = |pseudo: u8| -> Option<ServoArc<ComputedValues>> {
            let node = node?;
            match pseudo {
                0 => node.primary_styles().map(|s| (*s).clone()),
                i => {
                    let data = node.borrow_data()?;
                    data.styles.pseudos.get(&PSEUDOS[usize::from(i - 1)]).cloned()
                }
            }
        };
        let place = |list: &[Effect]| -> Vec<Effect> {
            list.iter()
                .filter_map(|e| match e.timeline {
                    None => Some(e.clone()),
                    Some(t) => {
                        let style = style_of(t.pseudo)?;
                        // (A pseudo-element's timeline from its own box: its nearest scroll
                        // container may be its originating element, and it is a `view()`'s
                        // subject; WPT scroll-animations `responsive/repaint-after-finished`.)
                        let at = match t.pseudo {
                            0 => id,
                            1 => node.and_then(|n| n.before()).unwrap_or(id),
                            _ => node.and_then(|n| n.after()).unwrap_or(id),
                        };
                        let (p, at_end) = crate::scroll_timeline::progress(self, at, &style, t.index)?;
                        let (progress, before) = t.iteration_progress(p, at_end)?;
                        Some(Effect { progress, before, ..e.clone() })
                    }
                })
                .collect()
        };
        let mut out = effects.clone();
        out.animations = place(&effects.animations);
        for (o, p) in out.pseudos.iter_mut().zip(&effects.pseudos) {
            o.animations = place(&p.animations);
        }
        out
    }

    /// pyro: at each resolve, the scroll-driven animations at the scroll position now (scrolling
    /// changes no page state the page's frame would show): elements whose values change are
    /// marked for restyle.
    /// pyro: whether a scroll-driven animation is sampled: its values depend on the layout (a
    /// view timeline's subject's position), so a style-only resolve is not enough for them.
    pub fn has_scroll_driven_animations(&self) -> bool {
        self.sampled_animations.elements.values().any(|e| {
            e.given.animations.iter().any(|x| x.timeline.is_some())
                || e.given.pseudos.iter().any(|p| p.animations.iter().any(|x| x.timeline.is_some()))
        })
    }

    /// pyro: whether `id` is in contents `content-visibility: auto` skips (an ancestor's).
    fn in_skipped_contents(&self, id: NodeId) -> bool {
        let mut at = self.get_node(id).and_then(|n| n.parent);
        while let Some(n) = at {
            let Some(node) = self.get_node(n) else { return false };
            if node.flags.contains(crate::node::NodeFlags::CV_SKIPPED) {
                return true;
            }
            at = node.parent;
        }
        false
    }

    /// `true`: an element's values changed (marked for restyle).
    pub(crate) fn resample_scroll_driven(&mut self) -> bool {
        if self.sampled_animations.is_empty() {
            return false;
        }
        let mut changed = false;
        let mut state = std::mem::take(&mut self.sampled_animations);
        let ids: Vec<NodeId> = state
            .elements
            .iter()
            .filter(|(_, e)| {
                e.given.animations.iter().any(|x| x.timeline.is_some())
                    || e.given.pseudos.iter().any(|p| p.animations.iter().any(|x| x.timeline.is_some()))
            })
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            // (Held in skipped contents: see `set_sampled_animations`.)
            if self.in_skipped_contents(id) {
                continue;
            }
            let element = state.elements.get_mut(&id).unwrap();
            let effects = self.drive(id, &element.given);
            if effects == element.effects {
                continue;
            }
            element.effects = effects;
            let resolved = element.base.is_some()
                && element.effects.all().all(|e| element.resolved.iter().any(|(k, _)| *k == e.keyframes))
                && element.pseudos.iter().zip(&element.effects.pseudos).all(|(s, e)| e.is_empty() || s.resolved_all(e));
            changed = true;
            if !resolved {
                state.pending = true;
                if let Some(node) = self.nodes.get_mut(id) {
                    node.set_restyle_hint(RestyleHint::RESTYLE_SELF);
                }
                continue;
            }
            let sampled = sample(element, &mut state.easings);
            if sampled != element.sampled {
                element.sampled = sampled;
                self.apply_sampled(id, element.sampled.clone());
            }
        }
        self.sampled_animations = state;
        changed
    }
}

/// The address of the style `node` inherits from (0: none).
fn parent_style_address(node: &Node) -> usize {
    let element: &Node = node;
    TElement::inheritance_parent(&element)
        .and_then(|p| {
            p.borrow_data()
                .map(|d| &**d.styles.primary() as *const ComputedValues as usize)
        })
        .unwrap_or(0)
}

/// `rules` cascaded for `node` with its parent's style.
fn cascade<'a>(
    node: &'a Node,
    context: &mut StyleContext<'_, &'a Node>,
    rules: StrongRuleNode,
    style: &ComputedValues,
) -> ServoArc<ComputedValues> {
    context.thread_local.current_dom_depth = TNode::depth(&node);
    let mut resolver = StyleResolverForElement::new(
        node,
        context,
        RuleInclusion::All,
        PseudoElementResolution::IfApplicable,
    );
    let inputs = CascadeInputs {
        rules: Some(rules),
        visited_rules: None,
        flags: style.flags.for_cascade_inputs(),
        included_cascade_flags: RuleCascadeFlags::empty(),
    };
    resolver
        .cascade_style_and_visited_with_default_parents(inputs)
        .0
}

/// A computed easing from CSS text (cached); `None`: not one.
fn easing(
    easings: &mut FxHashMap<Arc<str>, Option<TimingFunction>>,
    text: &Arc<str>,
) -> Option<TimingFunction> {
    if let Some(e) = easings.get(text) {
        return e.clone();
    }
    let block = parse_declarations(&format!("animation-timing-function: {text}"));
    let parsed = block.declarations().iter().find_map(|d| match d {
        PropertyDeclaration::AnimationTimingFunction(v) => {
            v.0.iter()
                .next()
                .map(|f| f.to_computed_value_without_context())
        }
        _ => None,
    });
    easings.insert(text.clone(), parsed.clone());
    parsed
}

fn parse_declarations(text: &str) -> PropertyDeclarationBlock {
    parse_declarations_at(text, &crate::url::DocumentUrl::default().url_extra_data())
}

/// Declarations as in a `style` attribute; relative URLs against `url`.
fn parse_declarations_at(
    text: &str,
    url: &style::stylesheets::UrlExtraData,
) -> PropertyDeclarationBlock {
    style::properties::declaration_block::parse_style_attribute(
        text,
        url,
        None,
        style::context::QuirksMode::NoQuirks,
        CssRuleType::Style,
    )
}

/// One keyframe's declarations: animatable longhands, physical for the base style's writing
/// mode. pyro: `display` too (css-display-4 § 2.1: it animates discretely, a `none` end only at
/// that end; Stylo's `Animate for Display`): a transition of `display` to `none` with
/// `allow-discrete` keeps the box shown until it ends (WPT css-position
/// `overlay/overlay-transition-dialog`).
fn keyframe_block(
    block: &PropertyDeclarationBlock,
    base: &ComputedValues,
) -> PropertyDeclarationBlock {
    let mut out = PropertyDeclarationBlock::new();
    for declaration in block.normal_declaration_iter() {
        if let PropertyDeclarationId::Longhand(id) = declaration.id() {
            if id != LonghandId::Display && !id.is_animatable() {
                continue;
            }
        }
        out.push(
            declaration.to_physical(base.writing_mode),
            Importance::Normal,
        );
    }
    out
}

/// `keyframes` resolved for `node` against `base`.
fn resolve<'a>(
    doc: &BaseDocument,
    node: &'a Node,
    base: &BaseKey,
    keyframes: &Keyframes,
    context: &mut StyleContext<'_, &'a Node>,
    easings: &mut FxHashMap<Arc<str>, Option<TimingFunction>>,
) -> Resolved {
    let shared = context.shared;
    // (offset, easing, composite, declarations) per keyframe.
    let mut frames: Vec<(
        f64,
        Option<TimingFunction>,
        Option<Composite>,
        PropertyDeclarationBlock,
    )> = Vec::new();
    let implicit_easing;
    match keyframes {
        Keyframes::Css {
            name,
            easing: default,
            composite,
        } => {
            let default_easing = match default {
                Some(text) => easing(easings, text),
                None => Some(TimingFunction::ease()),
            };
            implicit_easing = default_easing.clone();
            // pyro: from the tree scope of the declaration that named it (`animation_name_scope`).
            let animation = match animation_name_scope(doc, node) {
                Some(scope) => shared.stylist.lookup_keyframes_scoped(name, scope, node),
                None => shared.stylist.lookup_keyframes(name, node),
            };
            if animation.is_none() && let Some(css) = doc.transition_group_from(node.id, name) {
                let block = keyframe_block(&parse_declarations_at(css, &doc.url.url_extra_data()), &base.base_style);
                frames.push((0.0, default_easing.clone(), Some(*composite), block));
                frames.push((1.0, default_easing, Some(*composite), PropertyDeclarationBlock::new()));
            } else if let Some(animation) = animation {
            let guard = &shared.guards.author;
            for step in &animation.steps {
                let offset = step.start_offset.percentage.0 as f64;
                let easing = step
                    .get_animation_timing_function(guard)
                    .map(|f| f.to_computed_value_without_context());
                let own_composite = step.get_animation_composition(guard).map(composite_of);
                let block = match &step.value {
                    KeyframesStepValue::Declarations { block } => {
                        keyframe_block(&block.read_with(guard), &base.base_style)
                    }
                    KeyframesStepValue::ComputedValues => PropertyDeclarationBlock::new(),
                };
                // Keyframes at one offset merge, later declarations winning.
                match frames.last_mut() {
                    Some(last) if last.0 == offset => {
                        for d in block.normal_declaration_iter() {
                            last.3.push(d.clone(), Importance::Normal);
                        }
                        if easing.is_some() {
                            last.1 = easing;
                        }
                        if own_composite.is_some() {
                            last.2 = own_composite;
                        }
                    }
                    _ => frames.push((
                        offset,
                        easing.or_else(|| default_easing.clone()),
                        Some(own_composite.unwrap_or(*composite)),
                        block,
                    )),
                }
            }
            } else { return Resolved::default(); }
        }
        Keyframes::Text(text) => {
            implicit_easing = None;
            let url = doc.url.url_extra_data();
            for frame in &text.frames {
                let block = keyframe_block(
                    &parse_declarations_at(&frame.declarations, &url),
                    &base.base_style,
                );
                let easing = frame
                    .easing
                    .as_deref()
                    .and_then(|e| easing(easings, &Arc::from(e)));
                frames.push((frame.offset, easing, frame.composite, block));
            }
        }
    }
    // The properties, in first-seen order.
    let mut properties: Vec<(OwnedPropertyDeclarationId, Option<AnimationValue>)> = Vec::new();
    for (_, _, _, block) in &frames {
        for d in block.normal_declaration_iter() {
            let id = d.id();
            if !properties.iter().any(|(p, _)| p.as_borrowed() == id) {
                let underlying = AnimationValue::from_computed_values(id, &base.base_style);
                properties.push((id.to_owned(), underlying));
            }
        }
    }
    let rule_tree = shared.stylist.rule_tree();
    let guards = &shared.guards;
    let mut frame_flags = ComputedValueFlags::empty();
    let resolved_frames = frames
        .into_iter()
        .map(|(offset, easing, composite, block)| {
            let mut values = vec![None; properties.len()];
            if block.any_normal() {
                let ids: Vec<usize> = block
                    .normal_declaration_iter()
                    .filter_map(|d| {
                        properties
                            .iter()
                            .position(|(p, _)| p.as_borrowed() == d.id())
                    })
                    .collect();
                let locked: ServoArc<Locked<PropertyDeclarationBlock>> =
                    ServoArc::new(doc.guard.wrap(block));
                let mut important_rules_changed = false;
                let rules = rule_tree
                    .update_rule_at_level(
                        CascadeLevel::new(CascadeOrigin::Animations),
                        LayerOrder::root(),
                        Some(locked.borrow_arc()),
                        &base.rules,
                        guards,
                        &mut important_rules_changed,
                    )
                    .unwrap_or_else(|| base.rules.clone());
                let style = match &base.pseudo {
                    Some((pseudo, primary)) => cascade_pseudo(node, context, rules, &base.base_style, pseudo, primary),
                    None => cascade(node, context, rules, &base.base_style),
                };
                frame_flags |= style.flags;
                for i in ids {
                    values[i] =
                        AnimationValue::from_computed_values(properties[i].0.as_borrowed(), &style);
                }
            }
            ResolvedFrame {
                offset,
                easing,
                composite,
                values,
            }
        })
        .collect();
    Resolved {
        siblings: SiblingDependency::capture(frame_flags, node, &mut context.thread_local.tree_counting_caches),
        allow_size_keywords: base.base_style.slow_clone_interpolate_size() == style::values::computed::InterpolateSize::AllowKeywords,
        frames: resolved_frames,
        properties,
        implicit_easing,
    }
}

fn composite_of(c: style::values::specified::animation::AnimationComposition) -> Composite {
    use style::values::specified::animation::AnimationComposition as C;
    match c {
        C::Replace => Composite::Replace,
        C::Add => Composite::Add,
        C::Accumulate => Composite::Accumulate,
    }
}

/// The element's values now: its effects sampled with their resolved keyframes.
fn sample(
    element: &ElementState,
    easings: &mut FxHashMap<Arc<str>, Option<TimingFunction>>,
) -> Sampled {
    let level_of = |all: &[(Keyframes, Resolved)], effects: &[Effect], easings: &mut FxHashMap<Arc<str>, Option<TimingFunction>>| {
        if effects.is_empty() {
            return None;
        }
        let mut map = AnimationValueMap::default();
        for effect in effects {
            let Some((_, resolved)) = all
                .iter()
                .find(|(k, _)| *k == effect.keyframes)
            else {
                continue;
            };
            let progress = match &effect.easing {
                Some(text) => match easing(easings, text) {
                    Some(f) => calculate(&f, effect.progress, effect.before),
                    None => effect.progress,
                },
                None => effect.progress,
            };
            for (i, (property, base)) in resolved.properties.iter().enumerate() {
                let underlying = map.get(property).cloned().or_else(|| base.clone());
                if let Some(value) =
                    sample_property(resolved, i, progress, underlying.as_ref(), effect.composite, effect.before)
                {
                    map.insert(property.clone(), value);
                }
            }
        }
        Some(map)
    };
    // pyro: the properties that make a stacking context when not at their initial value (Web
    // Animations' `will-change` side effect makes one for any value).
    const STACKING: [LonghandId; 12] = [
        LonghandId::Opacity,
        LonghandId::Transform,
        LonghandId::Translate,
        LonghandId::Rotate,
        LonghandId::Scale,
        LonghandId::Perspective,
        LonghandId::Filter,
        LonghandId::BackdropFilter,
        LonghandId::ClipPath,
        LonghandId::MaskImage,
        LonghandId::MixBlendMode,
        LonghandId::Isolation,
    ];
    let stacking_context = element.effects.own().any(|effect| {
        element.resolved.iter().find(|(k, _)| *k == effect.keyframes).is_some_and(|(_, resolved)| {
            resolved.properties.iter().any(|(property, _)| {
                matches!(property.as_borrowed(), PropertyDeclarationId::Longhand(id) if STACKING.contains(&id))
            })
        })
    });
    let level = |effects: &[Effect], easings: &mut FxHashMap<Arc<str>, Option<TimingFunction>>| {
        level_of(&element.resolved, effects, easings)
    };
    let pseudo = |i: usize, easings: &mut FxHashMap<Arc<str>, Option<TimingFunction>>| {
        let (state, effects) = (&element.pseudos[i], &element.effects.pseudos[i]);
        (
            level_of(&state.resolved, &effects.animations, easings),
            level_of(&state.resolved, &effects.transitions, easings),
        )
    };
    Sampled {
        animations: level(&element.effects.animations, easings),
        transitions: level(&element.effects.transitions, easings),
        marker: level(&element.effects.marker, easings),
        backdrop: level(&element.effects.backdrop, easings),
        pseudos: [pseudo(0, easings), pseudo(1, easings)],
        stacking_context,
    }
}

fn calculate(f: &TimingFunction, progress: f64, before: bool) -> f64 {
    let flag = if before {
        BeforeFlag::Set
    } else {
        BeforeFlag::Unset
    };
    f.calculate_output(progress, flag, 1e-7)
}

/// The value of property `i` at transformed `progress` (Web Animations, "the
/// effect value of a keyframe effect").
fn sample_property(
    resolved: &Resolved,
    i: usize,
    progress: f64,
    underlying: Option<&AnimationValue>,
    effect_composite: Composite,
    before: bool,
) -> Option<AnimationValue> {
    struct Frame<'a> {
        offset: f64,
        easing: Option<&'a TimingFunction>,
        value: AnimationValue,
    }
    let compose = |value: &AnimationValue, composite: Composite| -> AnimationValue {
        let procedure = match composite {
            Composite::Replace => return value.clone(),
            Composite::Add => Procedure::Add,
            Composite::Accumulate => Procedure::Accumulate { count: 1 },
        };
        match underlying {
            Some(u) => u
                .animate_with_size_policy(value, procedure, resolved.allow_size_keywords)
                .unwrap_or_else(|_| value.clone()),
            None => value.clone(),
        }
    };
    let mut frames: Vec<Frame> = resolved
        .frames
        .iter()
        .filter_map(|f| {
            let value = f.values[i].as_ref()?;
            Some(Frame {
                offset: f.offset,
                easing: f.easing.as_ref(),
                value: compose(value, f.composite.unwrap_or(effect_composite)),
            })
        })
        .collect();
    if frames.is_empty() {
        return None;
    }
    // Neutral keyframes: the underlying value.
    if frames[0].offset != 0.0 {
        let value = underlying?.clone();
        frames.insert(
            0,
            Frame {
                offset: 0.0,
                easing: resolved.implicit_easing.as_ref(),
                value,
            },
        );
    }
    if frames[frames.len() - 1].offset != 1.0 {
        let value = underlying?.clone();
        frames.push(Frame {
            offset: 1.0,
            easing: resolved.implicit_easing.as_ref(),
            value,
        });
    }
    let at = |offset: f64| frames.iter().filter(|f| f.offset == offset).count();
    if progress < 0.0 && at(0.0) > 1 {
        return Some(frames.iter().find(|f| f.offset == 0.0)?.value.clone());
    }
    if progress >= 1.0 && at(1.0) > 1 {
        return Some(frames.iter().rev().find(|f| f.offset == 1.0)?.value.clone());
    }
    // The last keyframe at or before `progress` and before 1 (else the last at 0).
    let start = frames
        .iter()
        .rposition(|f| f.offset <= progress && f.offset < 1.0)
        .or_else(|| frames.iter().rposition(|f| f.offset == 0.0))?;
    let Some(end) = frames.get(start + 1) else {
        return Some(frames[start].value.clone());
    };
    let start = &frames[start];
    let span = end.offset - start.offset;
    let distance = if span == 0.0 {
        0.0
    } else {
        (progress - start.offset) / span
    };
    let transformed = match start.easing {
        // pyro: with the effect's before flag, as Blink's keyframe easings take its limit
        // direction (a `jump-start` step in the before phase stays at its start).
        Some(f) => calculate(f, distance, before),
        None => distance,
    };
    Some(
        start
            .value
            .animate_with_size_policy(
                &end.value,
                Procedure::Interpolate {
                    progress: transformed,
                },
                resolved.allow_size_keywords,
            )
            .unwrap_or_else(|_| match (&start.value, &end.value) {
                // pyro: `display` to or from `none` shows the other value throughout, `none` only
                // at its end (css-display-4 §2.1; Stylo's `Animate for Display`, which the
                // discrete path skips).
                (AnimationValue::Display(a), AnimationValue::Display(b)) if a.is_none() != b.is_none() => {
                    let other = if b.is_none() { transformed < 1.0 } else { transformed <= 0.0 };
                    if other {
                        start.value.clone()
                    } else {
                        end.value.clone()
                    }
                }
                _ if transformed < 0.5 => start.value.clone(),
                _ => end.value.clone(),
            }),
    )
}

/// The element's sampled blocks, for `TElement::animation_rule` and
/// `transition_rule`.
pub(crate) fn sampled_rule(
    node: &Node,
    transitions: bool,
) -> Option<ServoArc<Locked<PropertyDeclarationBlock>>> {
    let data = node.element_data()?;
    if transitions {
        data.sampled_transitions.clone()
    } else {
        data.sampled_animations.clone()
    }
}

/// Whether the element has sampled values (no style sharing).
pub(crate) fn has_sampled(node: &Node) -> bool {
    node.element_data().is_some_and(|d| {
        d.sampled_animations.is_some()
            || d.sampled_transitions.is_some()
            || d.sampled_pseudos.iter().any(|(a, t)| a.is_some() || t.is_some())
    })
}

/// pyro: after the element's restyle, the values sampled for its `::before` and `::after`
/// (`ElementData::sampled_pseudos`) cascaded into their styles, at the animations and
/// transitions levels.
pub(crate) fn cascade_sampled_pseudos<'a>(
    node: &'a Node,
    context: &mut StyleContext<'_, &'a Node>,
    data: &mut style::data::ElementData,
) {
    let Some(element) = node.element_data() else { return };
    if element.sampled_pseudos.iter().all(|(a, t)| a.is_none() && t.is_none()) {
        return;
    }
    if data.styles.pseudos.is_empty() {
        return;
    }
    let Some(primary) = data.styles.primary.clone() else { return };
    for (i, pseudo) in PSEUDOS.iter().enumerate() {
        let (animations, transitions) = &element.sampled_pseudos[i];
        if animations.is_none() && transitions.is_none() {
            continue;
        }
        let Some(style) = data.styles.pseudos.get(pseudo).cloned() else { continue };
        let shared = context.shared;
        let rule_tree = shared.stylist.rule_tree();
        let mut rules = rule_tree.remove_animation_rules(style.rules());
        let mut important_changed = false;
        for (origin, block) in [(CascadeOrigin::Transitions, transitions), (CascadeOrigin::Animations, animations)] {
            if let Some(block) = block {
                if let Some(path) = rule_tree.update_rule_at_level(
                    CascadeLevel::new(origin),
                    LayerOrder::root(),
                    Some(block.borrow_arc()),
                    &rules,
                    &shared.guards,
                    &mut important_changed,
                ) {
                    rules = path;
                }
            }
        }
        if rules == *style.rules() {
            continue;
        }
        let animated = cascade_pseudo(node, context, rules, &style, pseudo, &primary);
        data.styles.pseudos.set(pseudo, animated);
    }
}

/// pyro: `rules` cascaded as the `pseudo` of `node` (whose style is `primary`).
fn cascade_pseudo<'a>(
    node: &'a Node,
    context: &mut StyleContext<'_, &'a Node>,
    rules: StrongRuleNode,
    style: &ComputedValues,
    pseudo: &style::selector_parser::PseudoElement,
    primary: &ServoArc<ComputedValues>,
) -> ServoArc<ComputedValues> {
    let inputs = CascadeInputs {
        rules: Some(rules),
        visited_rules: None,
        flags: style.flags.for_cascade_inputs(),
        included_cascade_flags: RuleCascadeFlags::empty(),
    };
    if !pseudo.is_eager() {
        return context.shared.stylist.compute_pseudo_element_style_with_inputs(
            inputs, pseudo, &context.shared.guards, Some(primary), Some(node),
        );
    }
    context.thread_local.current_dom_depth = TNode::depth(&node);
    let mut resolver =
        StyleResolverForElement::new(node, context, RuleInclusion::All, PseudoElementResolution::IfApplicable);
    let primary = style::style_resolver::PrimaryStyle {
        style: style::style_resolver::ResolvedStyle(primary.clone()),
        reused_via_rule_node: false,
    };
    resolver.cascade_style_and_visited_for_pseudo_with_default_parents(inputs, pseudo, &primary).0
}

/// pyro: the cascade level of the declaration of `animation-name` that won `node`'s cascade (the
/// highest priority one, of its rule node's importance tier), whose tree scope a `@keyframes` name is looked up in; `None` without one
/// (inherited or initial) or for a user agent rule.
fn animation_name_scope(doc: &BaseDocument, node: &Node) -> Option<style::rule_tree::CascadeLevel> {
    use style::properties::{LonghandId, PropertyDeclarationId};
    use style::stylesheets::Origin;
    let style = node.primary_styles()?;
    let guard = doc.guard.read();
    for rule in style.rules().self_and_ancestors() {
        let level = rule.cascade_level();
        if level.origin().origin() == Origin::UserAgent {
            continue;
        }
        let Some(source) = rule.style_source() else { continue };
        // (Only a declaration of the rule node's own tier: a rule with some `!important`
        // declaration has a node in the important tier too, where its normal `animation-name`
        // does not take part.)
        let declared = source.read(&guard).get(PropertyDeclarationId::Longhand(LonghandId::AnimationName));
        if declared.is_some_and(|(_, importance)| importance.important() == level.is_important()) {
            return Some(level);
        }
    }
    None
}
