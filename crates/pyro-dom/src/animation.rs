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
}

pub type SharedAnimationSource = Rc<dyn AnimationSource>;
