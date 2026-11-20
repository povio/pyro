//! Animated values handed to the layout engine at one moment: per element,
//! the keyframe effects in effect and how far along each one is. The page
//! keeps the timing of CSS animations, CSS transitions and Web Animations
//! (pyro-webapi) and samples it into an [`AnimationFrame`]; the layout
//! engine resolves the keyframes against each element's style and cascades
//! the interpolated values (blitz-dom's `animation` module). Plain data, so
//! the render thread can take a frame with a snapshot.

use std::rc::Rc;
use std::sync::Arc;

/// How a keyframe's value combines with the value underneath it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Composite {
    #[default]
    Replace,
    Add,
    Accumulate,
}

impl Composite {
    /// `replace`, `add` or `accumulate`; `None` for anything else (`auto`).
    pub fn parse(text: &str) -> Option<Composite> {
        match text {
            "replace" => Some(Composite::Replace),
            "add" => Some(Composite::Add),
            "accumulate" => Some(Composite::Accumulate),
            _ => None,
        }
    }
}

/// Where an effect's keyframes come from.
#[derive(Clone, Debug, PartialEq)]
pub enum Keyframes {
    /// The `@keyframes` rule `name` that applies to the element (a CSS
    /// animation), with the animation's `animation-timing-function` (each
    /// keyframe's unless it declares one; `None`: `ease`) and
    /// `animation-composition`.
    Css { name: Arc<str>, easing: Option<Arc<str>>, composite: Composite },
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
    /// `None`: `linear`.
    pub easing: Option<String>,
    /// `None`: the effect's composite operation.
    pub composite: Option<Composite>,
    /// Declarations, as in a `style` attribute.
    pub declarations: String,
}

/// One keyframe effect in effect.
#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    pub keyframes: Keyframes,
    /// The iteration progress: the directed progress, before the effect's
    /// easing.
    pub progress: f64,
    /// The before flag the effect's easing takes (step functions).
    pub before: bool,
    /// The effect's easing (`None`: `linear`).
    pub easing: Option<Arc<str>>,
    pub composite: Composite,
    /// A CSS animation driven by a scroll or view progress timeline: the layout engine takes its
    /// progress from the scroll position (`progress` is unused).
    pub timeline: Option<TimelineDriven>,
}

/// A scroll-driven CSS animation's index in its target's `animation-*` lists and its timing.
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
    /// 0 the element, 1 its `::before`, 2 its `::after`.
    pub pseudo: u8,
}

/// One element's effects, each list in composite order: `transitions` at the
/// cascade's transitions level, `animations` (CSS animations, then script
/// animations) at its animations level.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ElementAnimations {
    /// The element's render key ([`crate::snapshot::render_key`]).
    pub key: u64,
    pub transitions: Vec<Effect>,
    pub animations: Vec<Effect>,
    /// CSS and script animations that are current but not in effect (in their delay, without a
    /// backwards fill): they give no values, but the properties they animate act as if
    /// `will-change` named them (Web Animations 1, "side effects of animation"), so one that
    /// makes a stacking context makes the element one meanwhile.
    pub current: Vec<Effect>,
    /// Script animations in effect on the element's `::marker` (`pseudoElement: '::marker'`), in
    /// composite order: their values go into the marker's style.
    pub marker: Vec<Effect>,
    /// Script animations in effect on a top-layer element's `::backdrop`: their values go into
    /// the backdrop's style.
    pub backdrop: Vec<Effect>,
    /// Effects on the element's `::before` (`[0]`) and `::after` (`[1]`): their values go into
    /// the pseudo-element's style.
    pub pseudos: [PseudoAnimations; 2],
}

/// The effects on one of an element's `::before`/`::after`, each list in composite order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PseudoAnimations {
    pub transitions: Vec<Effect>,
    pub animations: Vec<Effect>,
}

/// The document's animated elements at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimationFrame {
    pub elements: Vec<ElementAnimations>,
}

/// What the layout engine samples into its cascade at each sync (set by the
/// page, [`crate::style::StyleEngine::set_animation_source`]).
pub trait AnimationSource {
    /// Changes whenever [`AnimationSource::frame`] would answer differently:
    /// the effects in effect and their state, and, while an effect's value
    /// moves with time, the clock (one value per frame).
    fn stamp(&self) -> u64;
    /// The effects in effect now, for layout queries and computed styles.
    fn frame(&self) -> AnimationFrame;
    /// The value a running CSS transition of `longhand` gives `el` now, serialized
    /// ([`transition_value`]); `None` when none runs. `getComputedStyle` reports it: the layout
    /// engine samples no transitions.
    fn transition_value(&self, _el: crate::NodeId, _longhand: &str) -> Option<TransitionValue> {
        None
    }
    /// [`AnimationSource::transition_value`] of `el`'s `::before`/`::after`.
    fn pseudo_transition_value(
        &self,
        _el: crate::NodeId,
        _pseudo: crate::style::PseudoElement,
        _longhand: &str,
    ) -> Option<TransitionValue> {
        None
    }
}

pub type SharedAnimationSource = Rc<dyn AnimationSource>;

/// What a CSS transition gives `getComputedStyle` now ([`transition_value`]).
#[derive(Clone, Debug)]
pub enum TransitionValue {
    /// The serialized value (`display`, whose computed value the caller does not read from a style).
    Text(String),
    /// The after-change style with the transitioning longhand set to its value now, for the caller
    /// to serialize as it serializes any computed style.
    Style(crate::timing::StyleSnapshot),
}

/// The value of `longhand` a CSS transition from the style `before` to `after` gives at iteration
/// progress `progress` (before `easing`, CSS text; `None`: `ease`; `before_flag` for step easings).
/// Values that do not interpolate flip halfway; a transform's ends are the ends' own lists.
pub fn transition_value(
    longhand: &str,
    before: &crate::timing::StyleSnapshot,
    after: &crate::timing::StyleSnapshot,
    progress: f64,
    before_flag: bool,
    easing: Option<&str>,
) -> Option<TransitionValue> {
    use style::properties::animated_properties::AnimationValue;
    use style::properties::PropertyDeclarationId;
    use style::values::animated::{Animate, Procedure};
    use style::values::generics::easing::BeforeFlag;
    let id = crate::stylo::property_id(longhand)?;
    let PropertyDeclarationId::Longhand(id) = id.as_shorthand().err()? else { return None };
    let physical = PropertyDeclarationId::Longhand(id.to_physical(after.writing_mode));
    let from = AnimationValue::from_computed_values(physical, before)?;
    let to = AnimationValue::from_computed_values(physical, after)?;
    let eased = match easing_function(easing.unwrap_or("ease")) {
        Some(f) => {
            let flag = if before_flag { BeforeFlag::Set } else { BeforeFlag::Unset };
            f.calculate_output(progress, flag, 1e-7)
        }
        None => progress,
    };
    // `display` to or from `none` shows the other value throughout, `none` only at its end
    // (css-display-4 §2.1, Stylo's `Animate for Display`, which the discrete path skips).
    if longhand == "display" {
        let (a, b) = (before.computed_value_to_string(physical), after.computed_value_to_string(physical));
        return Some(TransitionValue::Text(match (a == "none", b == "none") {
            (false, true) => if eased < 1.0 { a } else { b },
            (true, false) => if eased > 0.0 { b } else { a },
            _ => if eased < 0.5 { a } else { b },
        }));
    }
    let discrete = || if eased < 0.5 { from.clone() } else { to.clone() };
    // A transform at an end is that end's own list (a `rotate3d()` axis not normalized), unless it
    // is `none`, which interpolates as the identity (`matrix(1, 0, 0, 1, 0, 0)`), as in Chromium;
    // any other property interpolates at its ends too (`blur(0px)` from `none`, `calc(0% + 16px)`).
    let own_end = |style: &crate::timing::StyleSnapshot| {
        // (Origins too: Chromium resolves them to lengths, never `calc(0% + 20px)`.)
        use style::properties::LonghandId as L;
        matches!(id, L::TransformOrigin | L::PerspectiveOrigin)
            || (matches!(id, L::Transform) && style.computed_value_to_string(physical) != "none")
    };
    let value = if eased == 0.0 && own_end(before) {
        from.clone()
    } else if eased == 1.0 && own_end(after) {
        to.clone()
    } else {
        match from.animate(&to, Procedure::Interpolate { progress: eased }) {
            // A transform interpolation Stylo defers (a matrix it cannot decompose) is discrete,
            // as in Chromium.
            Ok(v) if matches!(id, style::properties::LonghandId::Transform) && defers_matrix(&v) => discrete(),
            Ok(v) => v,
            Err(_) => discrete(),
        }
    };
    let mut style = (**after).clone();
    value.set_longhand_in_style(&mut style);
    // A computed `<opacity-value>` is clamped to [0, 1] (an easing can overshoot), as the
    // cascade clamps an animation's value.
    use style::properties::LonghandId as L;
    match id {
        L::Opacity => {
            let v = style.get_effects().opacity.clamp(0.0, 1.0);
            style.mutate_effects().set_opacity(v);
        }
        L::ShapeImageThreshold => {
            let v = style.get_box().shape_image_threshold.clamp(0.0, 1.0);
            style.mutate_box().set_shape_image_threshold(v);
        }
        L::FloodOpacity => {
            let v = style.get_svg().flood_opacity.clamp(0.0, 1.0);
            style.mutate_svg().set_flood_opacity(v);
        }
        L::StopOpacity => {
            let v = style.get_svg().stop_opacity.clamp(0.0, 1.0);
            style.mutate_svg().set_stop_opacity(v);
        }
        _ => {}
    }
    Some(TransitionValue::Style(style::servo_arc::Arc::new(style)))
}

/// A computed easing from CSS text; `None`: not one.
fn easing_function(text: &str) -> Option<style::values::computed::TimingFunction> {
    use style::properties::PropertyDeclaration;
    use style::values::computed::ToComputedValue as _;
    let url = crate::stylo::url_data();
    let block = style::properties::declaration_block::parse_style_attribute(
        &format!("animation-timing-function: {text}"),
        &url,
        None,
        style::context::QuirksMode::NoQuirks,
        style::stylesheets::CssRuleType::Style,
    );
    block.declarations().iter().find_map(|d| match d {
        PropertyDeclaration::AnimationTimingFunction(v) => v.0.iter().next().map(|f| f.to_computed_value_without_context()),
        _ => None,
    })
}

/// The longhands of the shorthand `name` (`None`: a longhand, `all` or unknown).
pub fn shorthand_longhands(name: &str) -> Option<Vec<&'static str>> {
    let id = crate::stylo::property_id(name)?;
    let shorthand = id.as_shorthand().ok()?;
    if shorthand == style::properties::ShorthandId::All {
        return None;
    }
    let longhands: Vec<&'static str> = shorthand.longhands().map(|l| l.name()).collect();
    Some(longhands)
}

/// `into` with `longhand` set to its value in `from` (the physical longhand, as
/// [`transition_value`] sets it).
pub fn with_longhand_of(
    into: crate::timing::StyleSnapshot,
    from: &crate::timing::StyleSnapshot,
    longhand: &str,
) -> crate::timing::StyleSnapshot {
    use style::properties::animated_properties::AnimationValue;
    use style::properties::PropertyDeclarationId;
    let Some(id) = crate::stylo::property_id(longhand) else { return into };
    let Some(PropertyDeclarationId::Longhand(id)) = id.as_shorthand().err() else { return into };
    let physical = PropertyDeclarationId::Longhand(id.to_physical(from.writing_mode));
    let Some(value) = AnimationValue::from_computed_values(physical, from) else { return into };
    let mut style = (*into).clone();
    value.set_longhand_in_style(&mut style);
    style::servo_arc::Arc::new(style)
}

/// Whether `longhand` has the same computed value in the styles `a` and `b`.
pub fn same_computed_value(longhand: &str, a: &crate::timing::StyleSnapshot, b: &crate::timing::StyleSnapshot) -> bool {
    use style::properties::PropertyDeclarationId;
    let Some(id) = crate::stylo::property_id(longhand) else { return false };
    let Some(PropertyDeclarationId::Longhand(id)) = id.as_shorthand().err() else { return false };
    let id = PropertyDeclarationId::Longhand(id);
    a.computed_value_to_string(id) == b.computed_value_to_string(id)
}

/// Whether an interpolated transform is one Stylo defers (`interpolatematrix()`,
/// `accumulatematrix()`: a matrix it cannot decompose), which Chromium does not interpolate.
pub(crate) fn defers_matrix(value: &style::properties::animated_properties::AnimationValue) -> bool {
    let mut css = String::new();
    let _ = value.uncompute().to_css(&mut css);
    css.contains("interpolatematrix") || css.contains("accumulatematrix")
}
