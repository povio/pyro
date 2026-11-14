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
use style::values::animated::{Animate, Procedure};
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
}

/// The effects of one element, each list in composite order: `transitions`
/// cascade at the transitions level, `animations` (CSS animations, then
/// script animations) at the animations level.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ElementEffects {
    pub transitions: Vec<Effect>,
    pub animations: Vec<Effect>,
}

impl ElementEffects {
    pub fn is_empty(&self) -> bool {
        self.transitions.is_empty() && self.animations.is_empty()
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
    pub(crate) fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }
}

#[derive(Default)]
struct ElementState {
    effects: ElementEffects,
    /// Keyframes resolved against `base`, by effect keyframes.
    resolved: Vec<(Keyframes, Resolved)>,
    base: Option<BaseKey>,
    /// The values sampled into the element's cascade.
    sampled: Sampled,
}

#[derive(Default, PartialEq)]
struct Sampled {
    animations: Option<AnimationValueMap>,
    transitions: Option<AnimationValueMap>,
}

/// What resolved keyframes depend on besides their declarations.
#[derive(Clone)]
struct BaseKey {
    rules: StrongRuleNode,
    /// The parent's style (inherited values), compared by address.
    parent: usize,
    base_style: ServoArc<ComputedValues>,
}

/// An effect's keyframes resolved against the element's base style.
#[derive(Default)]
struct Resolved {
    frames: Vec<ResolvedFrame>,
    /// The properties the keyframes set, with their base (underlying) value.
    properties: Vec<(OwnedPropertyDeclarationId, Option<AnimationValue>)>,
    /// The easing of the neutral keyframes made where 0% or 100% is missing.
    implicit_easing: Option<TimingFunction>,
}

struct ResolvedFrame {
    offset: f64,
    easing: Option<TimingFunction>,
    composite: Option<Composite>,
    /// By index into `Resolved::properties`.
    values: Vec<Option<AnimationValue>>,
}

impl BaseDocument {
    /// Replaces the document's sampled animations: the effects of each element
    /// in effect now (an element left out has none). Elements whose sampled
    /// values change are marked for restyle; the next `resolve` cascades them.
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
            element.effects = effects;
            element.resolved.retain(|(k, _)| {
                element
                    .effects
                    .animations
                    .iter()
                    .chain(&element.effects.transitions)
                    .any(|e| e.keyframes == *k)
            });
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
                && element
                    .effects
                    .animations
                    .iter()
                    .chain(&element.effects.transitions)
                    .all(|e| element.resolved.iter().any(|(k, _)| *k == e.keyframes));
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
                let sampled = Sampled {
                    animations: element.sampled.animations.clone(),
                    transitions: element.sampled.transitions.clone(),
                };
                self.apply_sampled(id, sampled);
            }
        }
        self.sampled_animations = state;
    }

    /// Whether any element has sampled animation values.
    pub fn has_sampled_animations(&self) -> bool {
        !self.sampled_animations.is_empty()
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
                visited_styles_enabled: false,
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
                    .is_some_and(|b| b.rules == rules && b.parent == parent);
                if !same_base {
                    let base_style = if rules == *style.rules() {
                        style.clone()
                    } else {
                        cascade(node, &mut context, rules.clone(), &style)
                    };
                    element.base = Some(BaseKey {
                        rules,
                        parent,
                        base_style,
                    });
                    element.resolved.clear();
                }
                let base = element.base.clone().unwrap();
                let effects: Vec<Keyframes> = element
                    .effects
                    .animations
                    .iter()
                    .chain(&element.effects.transitions)
                    .map(|e| e.keyframes.clone())
                    .collect();
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
                let sampled = sample(element, &mut state.easings);
                if sampled != element.sampled {
                    element.sampled = sampled;
                    restyle.push(*id);
                }
                true
            });
        }
        for id in &restyle {
            let element = &state.elements[id];
            let sampled = Sampled {
                animations: element.sampled.animations.clone(),
                transitions: element.sampled.transitions.clone(),
            };
            self.apply_sampled(*id, sampled);
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
        let Some(data) = node.element_data_mut() else {
            return;
        };
        let had = data.sampled_animations.is_some() || data.sampled_transitions.is_some();
        let has = animations.is_some() || transitions.is_some();
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

/// One keyframe's declarations: animatable longhands (not `display`), physical
/// for the base style's writing mode.
fn keyframe_block(
    block: &PropertyDeclarationBlock,
    base: &ComputedValues,
) -> PropertyDeclarationBlock {
    let mut out = PropertyDeclarationBlock::new();
    for declaration in block.normal_declaration_iter() {
        if let PropertyDeclarationId::Longhand(id) = declaration.id() {
            if id == LonghandId::Display || !id.is_animatable() {
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
            let Some(animation) = shared.stylist.lookup_keyframes(name, node) else {
                return Resolved::default();
            };
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
                let style = cascade(node, context, rules, &base.base_style);
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
    let level = |effects: &[Effect], easings: &mut FxHashMap<Arc<str>, Option<TimingFunction>>| {
        if effects.is_empty() {
            return None;
        }
        let mut map = AnimationValueMap::default();
        for effect in effects {
            let Some((_, resolved)) = element
                .resolved
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
                    sample_property(resolved, i, progress, underlying.as_ref(), effect.composite)
                {
                    map.insert(property.clone(), value);
                }
            }
        }
        Some(map)
    };
    Sampled {
        animations: level(&element.effects.animations, easings),
        transitions: level(&element.effects.transitions, easings),
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
                .animate(value, procedure)
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
        Some(f) => calculate(f, distance, false),
        None => distance,
    };
    Some(
        start
            .value
            .animate(
                &end.value,
                Procedure::Interpolate {
                    progress: transformed,
                },
            )
            .unwrap_or_else(|_| {
                if transformed < 0.5 {
                    start.value.clone()
                } else {
                    end.value.clone()
                }
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
    node.element_data()
        .is_some_and(|d| d.sampled_animations.is_some() || d.sampled_transitions.is_some())
}
