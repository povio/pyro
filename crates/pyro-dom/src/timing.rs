//! Author declarations of the CSS animation and transition properties, for
//! the events they fire (`animationstart`, `transitionend`, ...) and the
//! values the page samples into the layout engine's cascade.
//!
//! The layout engine's own animation engine stays off (its UA sheet declares
//! `animation` and `transition` `none !important`; the page samples animated
//! values in instead), so its computed values cannot say what the author
//! asked for. This module
//! reads it from the same sheets: the `animation*` and `transition*`
//! declarations of style rules (inside `@media` that matches, and inside
//! `@supports`/`@layer`/`@container`/`@scope` as if they applied) and of
//! `style` attributes, shorthands expanded, in cascade order (`!important`
//! first, then inline, specificity, source order), plus the names of the
//! `@keyframes` rules. None of these properties inherit.

use cssparser::{ParseError, Parser, Token};
use rustc_hash::FxHashSet;

use crate::dom::Dom;
use crate::media::Viewport;
use crate::node::NodeId;
use crate::stylo::select::List;

/// The longhands this module cascades.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Longhand {
    AnimationName,
    AnimationDuration,
    AnimationDelay,
    AnimationIterationCount,
    AnimationTimingFunction,
    AnimationDirection,
    AnimationFillMode,
    AnimationPlayState,
    AnimationComposition,
    TransitionProperty,
    TransitionDuration,
    TransitionDelay,
    TransitionTimingFunction,
}

/// One declaration: the longhand's comma-separated list, lowercased except
/// for animation names.
type Declared = (Longhand, Vec<String>, bool);

struct Rule {
    selectors: List,
    declarations: Vec<Declared>,
}

/// One CSS animation an element's style asks for.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationTiming {
    pub name: String,
    pub duration_ms: f64,
    pub delay_ms: f64,
    /// `f64::INFINITY` for `infinite`.
    pub iterations: f64,
    /// How it plays: changing these does not start it again
    /// ([`AnimationTiming::same_run`]).
    pub style: AnimationStyle,
}

impl AnimationTiming {
    /// Whether `other` is the same run: a change of its name or timing starts
    /// the animation again, one of its [`AnimationStyle`] does not.
    pub fn same_run(&self, other: &AnimationTiming) -> bool {
        self.name == other.name
            && self.duration_ms == other.duration_ms
            && self.delay_ms == other.delay_ms
            && self.iterations == other.iterations
    }
}

/// The parts of an animation's style that change how it plays, not when.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimationStyle {
    /// `animation-timing-function` as written (lowercased; `None`: `ease`).
    pub easing: Option<String>,
    /// `normal`, `reverse`, `alternate` or `alternate-reverse`.
    pub direction: String,
    /// `none`, `forwards`, `backwards` or `both`.
    pub fill: String,
    /// `animation-play-state: paused`.
    pub paused: bool,
    /// `replace`, `add` or `accumulate`.
    pub composition: String,
}

/// One entry of an element's transition list.
#[derive(Clone, Debug, PartialEq)]
pub struct TransitionTiming {
    /// A property name, or `all`.
    pub property: String,
    pub duration_ms: f64,
    pub delay_ms: f64,
    /// `transition-timing-function` as written (lowercased; `None`: `ease`).
    pub easing: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ElementTiming {
    /// Animations whose `@keyframes` rule exists.
    pub animations: Vec<AnimationTiming>,
    /// Transitions whose combined duration is positive.
    pub transitions: Vec<TransitionTiming>,
}

/// The animation and transition rules of a document's sheets.
#[derive(Default)]
pub struct TimingRules {
    rules: Vec<Rule>,
    keyframes: FxHashSet<String>,
    /// The longhands each `@keyframes` rule declares, by name.
    keyframe_longhands: rustc_hash::FxHashMap<String, FxHashSet<&'static str>>,
}

impl TimingRules {
    /// Reads `sheets` (texts in cascade order).
    pub fn parse<'a>(sheets: impl Iterator<Item = &'a str>, viewport: &Viewport) -> TimingRules {
        let mut out = TimingRules::default();
        for text in sheets {
            if mentions(text) || text.contains("keyframes") {
                rule_list(&mut Parser::new(text), text, viewport, &mut out);
            }
        }
        out
    }

    pub fn has_keyframes(&self, name: &str) -> bool {
        self.keyframes.contains(name)
    }

    /// Whether any rule declares an animation or transition property: the
    /// page's style changes may start animations or transitions.
    pub fn animates(&self) -> bool {
        !self.rules.is_empty()
    }

    /// Whether the `@keyframes` rule `name` declares `longhand` (directly or
    /// through a shorthand).
    pub fn keyframes_declare(&self, name: &str, longhand: &str) -> bool {
        self.keyframe_longhands.get(name).is_some_and(|l| l.contains(longhand))
    }

    /// The elements of `doc` that may run an animation: those a rule
    /// declaring an animation name matches, and those whose `style`
    /// attribute declares one.
    pub fn animation_candidates(&self, dom: &Dom, doc: NodeId) -> Vec<NodeId> {
        if self.keyframes.is_empty() {
            return Vec::new();
        }
        self.candidates(dom, doc, Longhand::AnimationName, "animation")
    }

    fn candidates(&self, dom: &Dom, doc: NodeId, longhand: Longhand, inline_word: &str) -> Vec<NodeId> {
        let mut seen = FxHashSet::default();
        let mut out = Vec::new();
        let Ok(root) = dom.check(doc) else { return out };
        for rule in &self.rules {
            if !rule.declarations.iter().any(|d| d.0 == longhand) {
                continue;
            }
            for idx in crate::stylo::select::query(dom, root, &rule.selectors, false) {
                if seen.insert(idx) {
                    out.push(dom.id_of(idx));
                }
            }
        }
        if let Ok(descendants) = dom.descendants(doc) {
            for id in descendants {
                let inline = dom
                    .element(id)
                    .ok()
                    .and_then(|e| e.attr_local(&markup5ever::local_name!("style")))
                    .is_some_and(|s| s.contains(inline_word));
                if inline && seen.insert(id.index()) {
                    out.push(id);
                }
            }
        }
        out
    }

    /// What `el`'s style asks for.
    pub fn element(&self, dom: &Dom, el: NodeId) -> ElementTiming {
        let mut out = ElementTiming::default();
        let Ok(idx) = dom.check(el) else { return out };
        if !dom.n(idx).is_element() {
            return out;
        }
        let inline: Vec<Declared> =
            match dom.element(el).ok().and_then(|e| e.attr_local(&markup5ever::local_name!("style"))) {
                Some(style) if mentions(style) => declarations(style),
                _ => Vec::new(),
            };
        // The rules that match, with their specificity, once for all longhands.
        let matching: Vec<(u32, usize, &Rule)> = self
            .rules
            .iter()
            .enumerate()
            .filter_map(|(order, rule)| {
                let specificity = rule
                    .selectors
                    .slice()
                    .iter()
                    .filter(|s| crate::stylo::select::matches(dom, idx, &List::from_one((*s).clone())))
                    .map(|s| s.specificity())
                    .max()?;
                Some((specificity, order, rule))
            })
            .collect();
        if inline.is_empty() && matching.is_empty() {
            return out;
        }
        let cascaded = |longhand: Longhand| -> Option<&Vec<String>> {
            let own = inline.iter().rev().find(|d| d.0 == longhand);
            if let Some((_, value, true)) = own {
                return Some(value);
            }
            // (important, specificity, order) of the best rule declaration so far.
            let mut best: Option<((bool, u32, usize), &Vec<String>)> = None;
            for (specificity, order, rule) in &matching {
                let Some((_, value, important)) = rule.declarations.iter().rev().find(|d| d.0 == longhand) else {
                    continue;
                };
                let key = (*important, *specificity, *order);
                if best.is_none_or(|(k, _)| key >= k) {
                    best = Some((key, value));
                }
            }
            match (best, own) {
                (Some(((true, ..), value)), _) => Some(value),
                (_, Some((_, value, _))) => Some(value),
                (Some((_, value)), None) => Some(value),
                (None, None) => None,
            }
        };
        // A list shorter than the names' repeats (css-animations-1 §4.2).
        fn nth<'a>(list: Option<&'a Vec<String>>, i: usize) -> Option<&'a str> {
            let list = list.filter(|l| !l.is_empty())?;
            Some(list[i % list.len()].as_str())
        }
        if let Some(names) = cascaded(Longhand::AnimationName) {
            let durations = cascaded(Longhand::AnimationDuration);
            let delays = cascaded(Longhand::AnimationDelay);
            let counts = cascaded(Longhand::AnimationIterationCount);
            let easings = cascaded(Longhand::AnimationTimingFunction);
            let directions = cascaded(Longhand::AnimationDirection);
            let fills = cascaded(Longhand::AnimationFillMode);
            let plays = cascaded(Longhand::AnimationPlayState);
            let compositions = cascaded(Longhand::AnimationComposition);
            let keyword = |list: Option<&Vec<String>>, i: usize, allowed: &[&str], initial: &str| {
                nth(list, i).filter(|v| allowed.contains(v)).unwrap_or(initial).to_owned()
            };
            for (i, name) in names.iter().enumerate() {
                if name == "none" || !self.has_keyframes(name) {
                    continue;
                }
                out.animations.push(AnimationTiming {
                    name: name.clone(),
                    duration_ms: nth(durations, i).and_then(time_ms).unwrap_or(0.0).max(0.0),
                    delay_ms: nth(delays, i).and_then(time_ms).unwrap_or(0.0),
                    iterations: match nth(counts, i) {
                        Some("infinite") => f64::INFINITY,
                        Some(n) => n.parse::<f64>().ok().filter(|n| *n >= 0.0).unwrap_or(1.0),
                        None => 1.0,
                    },
                    style: AnimationStyle {
                        easing: nth(easings, i).filter(|e| *e != "ease").map(str::to_owned),
                        direction: keyword(directions, i, &DIRECTIONS, "normal"),
                        fill: keyword(fills, i, &FILLS, "none"),
                        paused: nth(plays, i) == Some("paused"),
                        composition: keyword(compositions, i, &COMPOSITIONS, "replace"),
                    },
                });
            }
        }
        let durations = cascaded(Longhand::TransitionDuration);
        if durations.is_some() {
            let all = vec!["all".to_owned()];
            let properties = cascaded(Longhand::TransitionProperty).unwrap_or(&all);
            let delays = cascaded(Longhand::TransitionDelay);
            let easings = cascaded(Longhand::TransitionTimingFunction);
            for (i, property) in properties.iter().enumerate() {
                let duration_ms = nth(durations, i).and_then(time_ms).unwrap_or(0.0).max(0.0);
                let delay_ms = nth(delays, i).and_then(time_ms).unwrap_or(0.0);
                if property == "none" || duration_ms + delay_ms <= 0.0 {
                    continue;
                }
                let easing = nth(easings, i).filter(|e| *e != "ease").map(str::to_owned);
                out.transitions.push(TransitionTiming { property: property.clone(), duration_ms, delay_ms, easing });
            }
        }
        out
    }
}

/// An element's computed style at a style change, kept to tell which
/// properties a later one changed.
pub type StyleSnapshot = style::servo_arc::Arc<style::properties::ComputedValues>;

/// Whether `style` is `display: none` (the element is not rendered).
pub fn displays_none(style: &StyleSnapshot) -> bool {
    style.get_box().display.is_none()
}

/// The longhands `property` (a longhand, a shorthand or `all`) covers whose
/// computed value differs between `before` and `after`: the ones a
/// transition of `property` starts for.
pub fn changed_longhands(before: &StyleSnapshot, after: &StyleSnapshot, property: &str) -> Vec<&'static str> {
    use style::properties::animated_properties::AnimationValue;
    use style::properties::{PropertyDeclarationId, ShorthandId};
    use style::values::animated::{Animate, Procedure};
    // Different, and values Stylo can animate between (`width: auto` to a
    // length starts none; properties Stylo animates discretely, `float` or
    // `display`, still do). Animated values exist for physical longhands
    // only: a logical one (`inline-size`) is mapped through the after-change
    // style's writing mode.
    let differs = |longhand: style::properties::LonghandId| {
        let id = PropertyDeclarationId::Longhand(longhand);
        let physical = PropertyDeclarationId::Longhand(longhand.to_physical(after.writing_mode));
        before.computed_value_to_string(id) != after.computed_value_to_string(id)
            && match (
                AnimationValue::from_computed_values(physical, before),
                AnimationValue::from_computed_values(physical, after),
            ) {
                (Some(a), Some(b)) => a.animate(&b, Procedure::Interpolate { progress: 0.5 }).is_ok(),
                _ => false,
            }
    };
    let shorthand = if property == "all" {
        Some(ShorthandId::All)
    } else {
        match crate::stylo::property_id(property) {
            Some(id) => match id.as_shorthand() {
                Ok(shorthand) => Some(shorthand),
                Err(PropertyDeclarationId::Longhand(longhand)) => {
                    return if differs(longhand) { vec![longhand.name()] } else { Vec::new() };
                }
                Err(_) => return Vec::new(),
            },
            None => return Vec::new(),
        }
    };
    let Some(shorthand) = shorthand else { return Vec::new() };
    shorthand.longhands().filter(|l| differs(*l)).map(|l| l.name()).collect()
}

/// The keyframes a transition of `longhand` runs between: its computed value
/// in `before` and in `after`, as declarations of the physical longhand
/// (`None` for a name Stylo does not know).
pub fn transition_keyframes(
    before: &StyleSnapshot,
    after: &StyleSnapshot,
    longhand: &str,
) -> Option<crate::animation::TextKeyframes> {
    use style::properties::PropertyDeclarationId;
    let id = crate::stylo::property_id(longhand)?;
    let PropertyDeclarationId::Longhand(id) = id.as_shorthand().err()? else { return None };
    let physical = id.to_physical(after.writing_mode);
    let value = |style: &StyleSnapshot| style.computed_value_to_string(PropertyDeclarationId::Longhand(physical));
    let frame = |offset: f64, value: String| crate::animation::TextKeyframe {
        offset,
        easing: None,
        composite: None,
        declarations: format!("{}: {value}", physical.name()),
    };
    Some(crate::animation::TextKeyframes { frames: vec![frame(0.0, value(before)), frame(1.0, value(after))] })
}

/// Whether `text` can declare one of the properties.
fn mentions(text: &str) -> bool {
    text.contains("animation") || text.contains("transition")
}

fn next_tok<'i>(p: &mut Parser<'i>) -> Option<Token<'i>> {
    p.next_including_whitespace_and_comments().ok().cloned()
}

/// Consumes the contents of the block `t` opened, if it opened one.
fn skip_block(p: &mut Parser<'_>, t: &Token<'_>) {
    if matches!(t, Token::Function(_) | Token::ParenthesisBlock | Token::SquareBracketBlock | Token::CurlyBracketBlock)
    {
        let _: Result<(), ParseError<()>> = p.parse_nested_block(|n| {
            while n.next_including_whitespace_and_comments().is_ok() {}
            Ok(())
        });
    }
}

/// The rules of a rule list (`src` is the whole sheet: positions index it).
fn rule_list(p: &mut Parser<'_>, src: &str, viewport: &Viewport, out: &mut TimingRules) {
    loop {
        let start = p.position().byte_index();
        let Some(first) = next_tok(p) else { return };
        let at = match &first {
            Token::WhiteSpace(_) | Token::Comment(_) | Token::CDO | Token::CDC => continue,
            Token::AtKeyword(name) => Some(name.to_ascii_lowercase()),
            _ => None,
        };
        // The prelude, up to the rule's block (an at-rule may end at `;`).
        let prelude_start = if at.is_some() { p.position().byte_index() } else { start };
        let mut prelude_end = prelude_start;
        let mut block = matches!(first, Token::CurlyBracketBlock);
        if !block {
            if at.is_none() {
                skip_block(p, &first);
            }
            loop {
                let before = p.position().byte_index();
                match next_tok(p) {
                    None => return,
                    Some(Token::CurlyBracketBlock) => {
                        prelude_end = before;
                        block = true;
                        break;
                    }
                    Some(Token::Semicolon) if at.is_some() => break,
                    Some(t) => skip_block(p, &t),
                }
            }
        }
        if !block {
            continue;
        }
        let prelude = src.get(prelude_start..prelude_end).unwrap_or("").trim();
        let _: Result<(), ParseError<()>> = p.parse_nested_block(|n| {
            match at.as_deref() {
                Some("media") if crate::media::matches(prelude, viewport) => rule_list(n, src, viewport, out),
                Some("supports" | "layer" | "container" | "scope") => rule_list(n, src, viewport, out),
                Some("keyframes" | "-webkit-keyframes") => {
                    let name = prelude.trim_matches(|c| c == '"' || c == '\'');
                    if !name.is_empty() {
                        out.keyframes.insert(name.to_owned());
                        let longhands = keyframe_longhands(n, src);
                        out.keyframe_longhands.entry(name.to_owned()).or_default().extend(longhands);
                    }
                }
                Some(_) => {}
                None => {
                    let body_start = n.position().byte_index();
                    while n.next_including_whitespace_and_comments().is_ok() {}
                    let body_end = n.position().byte_index();
                    let body = src.get(body_start..body_end).unwrap_or("");
                    if mentions(body) {
                        let declarations = declarations(body);
                        if !declarations.is_empty() {
                            if let Some(selectors) = crate::stylo::select::parse(prelude) {
                                out.rules.push(Rule { selectors, declarations });
                            }
                        }
                    }
                }
            }
            while n.next_including_whitespace_and_comments().is_ok() {}
            Ok(())
        });
    }
}

/// The longhands the keyframes of a `@keyframes` block declare (`p` is in
/// the block; `src` is the whole sheet).
fn keyframe_longhands(p: &mut Parser<'_>, src: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    loop {
        let Some(t) = next_tok(p) else { return out };
        if !matches!(t, Token::CurlyBracketBlock) {
            skip_block(p, &t);
            continue;
        }
        let _: Result<(), ParseError<()>> = p.parse_nested_block(|n| {
            let start = n.position().byte_index();
            while n.next_including_whitespace_and_comments().is_ok() {}
            let body = src.get(start..n.position().byte_index()).unwrap_or("");
            for d in crate::style::parse_declarations(body) {
                let Some(id) = crate::stylo::property_id(&d.name.to_ascii_lowercase()) else { continue };
                match id.as_shorthand() {
                    Ok(shorthand) => out.extend(shorthand.longhands().map(|l| l.name())),
                    Err(style::properties::PropertyDeclarationId::Longhand(l)) => out.push(l.name()),
                    Err(_) => {}
                }
            }
            Ok(())
        });
    }
}

/// The animation and transition declarations of a declaration block, as
/// longhands in source order.
fn declarations(body: &str) -> Vec<Declared> {
    let mut out = Vec::new();
    for d in crate::style::parse_declarations(body) {
        let name = d.name.to_ascii_lowercase();
        let name = name.strip_prefix("-webkit-").unwrap_or(&name);
        let value = d.value.trim();
        let list = |keep_case: bool| -> Vec<String> {
            split_commas(value)
                .into_iter()
                .map(|item| if keep_case { unquote(item.trim()) } else { item.trim().to_ascii_lowercase() })
                .collect()
        };
        match name {
            "animation" => {
                let mut items: [Vec<String>; 8] = Default::default();
                for item in split_commas(value) {
                    let item = animation_item(item);
                    for (list, v) in items.iter_mut().zip([
                        item.name,
                        item.duration,
                        item.delay,
                        item.count,
                        item.easing,
                        item.direction,
                        item.fill,
                        item.play,
                    ]) {
                        list.push(v);
                    }
                }
                let [names, durations, delays, counts, easings, directions, fills, plays] = items;
                out.push((Longhand::AnimationName, names, d.important));
                out.push((Longhand::AnimationDuration, durations, d.important));
                out.push((Longhand::AnimationDelay, delays, d.important));
                out.push((Longhand::AnimationIterationCount, counts, d.important));
                out.push((Longhand::AnimationTimingFunction, easings, d.important));
                out.push((Longhand::AnimationDirection, directions, d.important));
                out.push((Longhand::AnimationFillMode, fills, d.important));
                out.push((Longhand::AnimationPlayState, plays, d.important));
            }
            "transition" => {
                let (mut properties, mut durations, mut delays, mut easings) =
                    (Vec::new(), Vec::new(), Vec::new(), Vec::new());
                for item in split_commas(value) {
                    let item = transition_item(item);
                    properties.push(item.name);
                    durations.push(item.duration);
                    delays.push(item.delay);
                    easings.push(item.easing);
                }
                out.push((Longhand::TransitionProperty, properties, d.important));
                out.push((Longhand::TransitionDuration, durations, d.important));
                out.push((Longhand::TransitionDelay, delays, d.important));
                out.push((Longhand::TransitionTimingFunction, easings, d.important));
            }
            "animation-name" => out.push((Longhand::AnimationName, list(true), d.important)),
            "animation-duration" => out.push((Longhand::AnimationDuration, list(false), d.important)),
            "animation-delay" => out.push((Longhand::AnimationDelay, list(false), d.important)),
            "animation-iteration-count" => out.push((Longhand::AnimationIterationCount, list(false), d.important)),
            "animation-timing-function" => out.push((Longhand::AnimationTimingFunction, list(false), d.important)),
            "animation-direction" => out.push((Longhand::AnimationDirection, list(false), d.important)),
            "animation-fill-mode" => out.push((Longhand::AnimationFillMode, list(false), d.important)),
            "animation-play-state" => out.push((Longhand::AnimationPlayState, list(false), d.important)),
            "animation-composition" => out.push((Longhand::AnimationComposition, list(false), d.important)),
            "transition-property" => out.push((Longhand::TransitionProperty, list(false), d.important)),
            "transition-duration" => out.push((Longhand::TransitionDuration, list(false), d.important)),
            "transition-delay" => out.push((Longhand::TransitionDelay, list(false), d.important)),
            "transition-timing-function" => out.push((Longhand::TransitionTimingFunction, list(false), d.important)),
            _ => {}
        }
    }
    out
}

/// A string's contents, or the text as written.
fn unquote(text: &str) -> String {
    let inner = text
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .or_else(|| text.strip_prefix('\'').and_then(|t| t.strip_suffix('\'')));
    inner.unwrap_or(text).to_owned()
}

/// Splits a value at its top-level commas.
fn split_commas(value: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut p = Parser::new(value);
    let mut start = 0;
    loop {
        let before = p.position().byte_index();
        match next_tok(&mut p) {
            None => break,
            Some(Token::Comma) => {
                out.push(&value[start..before]);
                start = p.position().byte_index();
            }
            Some(t) => skip_block(&mut p, &t),
        }
    }
    out.push(&value[start..]);
    out
}

/// One item of a shorthand, with the initial value of what it leaves out.
struct ShorthandItem {
    name: String,
    duration: String,
    delay: String,
    count: String,
    easing: String,
    direction: String,
    fill: String,
    play: String,
}

impl ShorthandItem {
    fn new(name: &str) -> ShorthandItem {
        ShorthandItem {
            name: name.into(),
            duration: "0s".into(),
            delay: "0s".into(),
            count: "1".into(),
            easing: "ease".into(),
            direction: "normal".into(),
            fill: "none".into(),
            play: "running".into(),
        }
    }
}

const EASING: [&str; 7] = ["ease", "linear", "ease-in", "ease-out", "ease-in-out", "step-start", "step-end"];
/// Functions a `<easing-function>` is written with.
const EASING_FUNCTIONS: [&str; 3] = ["cubic-bezier(", "steps(", "linear("];
const DIRECTIONS: [&str; 4] = ["normal", "reverse", "alternate", "alternate-reverse"];
const FILLS: [&str; 4] = ["none", "forwards", "backwards", "both"];
const COMPOSITIONS: [&str; 3] = ["replace", "add", "accumulate"];

/// The tokens of one shorthand item: `(text, is_time, is_number, is_ident_or_string)`.
fn item_tokens(item: &str) -> Vec<(String, u8)> {
    const TIME: u8 = 0;
    const NUMBER: u8 = 1;
    const WORD: u8 = 2;
    const OTHER: u8 = 3;
    let mut out = Vec::new();
    let mut p = Parser::new(item);
    loop {
        let before = p.position().byte_index();
        let Some(token) = next_tok(&mut p) else { break };
        let kind = match &token {
            Token::WhiteSpace(_) | Token::Comment(_) => continue,
            Token::Dimension { unit, .. } if unit.eq_ignore_ascii_case("s") || unit.eq_ignore_ascii_case("ms") => TIME,
            Token::Number { .. } => NUMBER,
            Token::Ident(_) | Token::QuotedString(_) => WORD,
            _ => OTHER,
        };
        skip_block(&mut p, &token);
        let text = &item[before..p.position().byte_index()];
        out.push((if kind == WORD { unquote(text) } else { text.to_ascii_lowercase() }, kind));
    }
    out
}

/// `<single-animation>`: the first time is the duration, the second the
/// delay; a keyword valid for another longhand not given yet goes to that
/// longhand, any other word is the name.
fn animation_item(item: &str) -> ShorthandItem {
    let mut out = ShorthandItem::new("none");
    let (mut times, mut easing, mut direction, mut fill, mut play, mut count) = (0, false, false, false, false, false);
    for (text, kind) in item_tokens(item) {
        let lower = text.to_ascii_lowercase();
        match kind {
            0 => {
                match times {
                    0 => out.duration = lower,
                    _ => out.delay = lower,
                }
                times += 1;
            }
            1 if !count => {
                out.count = lower;
                count = true;
            }
            2 => match lower.as_str() {
                "infinite" if !count => {
                    out.count = lower;
                    count = true;
                }
                w if EASING.contains(&w) && !easing => {
                    out.easing = lower;
                    easing = true;
                }
                w if DIRECTIONS.contains(&w) && !direction => {
                    out.direction = lower;
                    direction = true;
                }
                w if FILLS.contains(&w) && !fill => {
                    out.fill = lower;
                    fill = true;
                }
                "running" | "paused" if !play => {
                    out.play = lower;
                    play = true;
                }
                _ => out.name = text,
            },
            3 if !easing && EASING_FUNCTIONS.iter().any(|f| lower.starts_with(f)) => {
                out.easing = lower;
                easing = true;
            }
            _ => {}
        }
    }
    out
}

/// `<single-transition>`: times as in an animation, an easing keyword, a
/// behavior keyword, any other word is the property.
fn transition_item(item: &str) -> ShorthandItem {
    let mut out = ShorthandItem::new("all");
    let (mut times, mut easing) = (0, false);
    for (text, kind) in item_tokens(item) {
        let lower = text.to_ascii_lowercase();
        match kind {
            0 => {
                match times {
                    0 => out.duration = lower,
                    _ => out.delay = lower,
                }
                times += 1;
            }
            2 => match lower.as_str() {
                w if EASING.contains(&w) && !easing => {
                    out.easing = lower;
                    easing = true;
                }
                "allow-discrete" | "normal" => {}
                _ => out.name = lower,
            },
            3 if !easing && EASING_FUNCTIONS.iter().any(|f| lower.starts_with(f)) => {
                out.easing = lower;
                easing = true;
            }
            _ => {}
        }
    }
    out
}

/// A `<time>` in milliseconds.
fn time_ms(text: &str) -> Option<f64> {
    let text = text.trim();
    if let Some(ms) = text.strip_suffix("ms") {
        return ms.trim().parse().ok();
    }
    text.strip_suffix('s').and_then(|s| s.trim().parse::<f64>().ok()).map(|s| s * 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shorthands_expand_to_their_longhands() {
        let item = animation_item("anim 100s -99.9s 2");
        assert_eq!(
            (item.name.as_str(), item.duration.as_str(), item.delay.as_str(), item.count.as_str()),
            ("anim", "100s", "-99.9s", "2")
        );
        let item = animation_item("1ms ease-in infinite alternate both spin");
        assert_eq!((item.name.as_str(), item.duration.as_str(), item.count.as_str()), ("spin", "1ms", "infinite"));
        // A keyword already given names the animation.
        assert_eq!(animation_item("ease ease 1s").name, "ease");
        assert_eq!(animation_item("none").name, "none");
        let item = transition_item("width 1ms cubic-bezier(0, 0, 1, 1) 2s");
        assert_eq!((item.name.as_str(), item.duration.as_str(), item.delay.as_str()), ("width", "1ms", "2s"));
        assert_eq!(item.easing, "cubic-bezier(0, 0, 1, 1)");
        let item = animation_item("spin 1s steps(2, jump-end) -0.5s 3 reverse both paused");
        assert_eq!(
            (item.easing.as_str(), item.direction.as_str(), item.fill.as_str(), item.play.as_str()),
            ("steps(2, jump-end)", "reverse", "both", "paused")
        );
        let item = animation_item("spin 1s");
        assert_eq!((item.easing.as_str(), item.fill.as_str(), item.play.as_str()), ("ease", "none", "running"));
        assert_eq!(transition_item(".3s").name, "all");
        assert_eq!(split_commas("a 1s, b steps(2, end) 2s"), ["a 1s", " b steps(2, end) 2s"]);
        assert_eq!(time_ms("-99.9s"), Some(-99900.0));
        assert_eq!(time_ms("30ms"), Some(30.0));
        assert_eq!(time_ms("auto"), None);
    }
}
