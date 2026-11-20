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
//!
//! Values are parsed as the cascade parses them (Stylo: an invalid one is
//! dropped, CSS-wide keywords apply) and kept as their computed values
//! serialize (`0.15s`, `cubic-bezier(0.4, 0, 0.2, 1)`). A declaration with
//! `var()` is resolved per element, with the element's custom properties,
//! as at computed-value time ([`TimingRules::lists`]). `getComputedStyle`
//! reports these properties from here too ([`TimingRules::computed_value`]),
//! so what it says and what runs agree.

use crate::style::PseudoElement;
use std::rc::Rc;

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
    TransitionBehavior,
    /// pyro: `animation-trigger` and `timeline-trigger` (CSS Animations 2, Web Animations 2
    /// triggers): items kept as written (Stylo does not parse them).
    AnimationTrigger,
    TimelineTrigger,
}

impl Longhand {
    const ALL: [Longhand; 16] = [
        Longhand::AnimationName,
        Longhand::AnimationDuration,
        Longhand::AnimationDelay,
        Longhand::AnimationIterationCount,
        Longhand::AnimationTimingFunction,
        Longhand::AnimationDirection,
        Longhand::AnimationFillMode,
        Longhand::AnimationPlayState,
        Longhand::AnimationComposition,
        Longhand::TransitionProperty,
        Longhand::TransitionDuration,
        Longhand::TransitionDelay,
        Longhand::TransitionTimingFunction,
        Longhand::TransitionBehavior,
        Longhand::AnimationTrigger,
        Longhand::TimelineTrigger,
    ];

    fn name(self) -> &'static str {
        match self {
            Longhand::AnimationName => "animation-name",
            Longhand::AnimationDuration => "animation-duration",
            Longhand::AnimationDelay => "animation-delay",
            Longhand::AnimationIterationCount => "animation-iteration-count",
            Longhand::AnimationTimingFunction => "animation-timing-function",
            Longhand::AnimationDirection => "animation-direction",
            Longhand::AnimationFillMode => "animation-fill-mode",
            Longhand::AnimationPlayState => "animation-play-state",
            Longhand::AnimationComposition => "animation-composition",
            Longhand::TransitionProperty => "transition-property",
            Longhand::TransitionDuration => "transition-duration",
            Longhand::TransitionDelay => "transition-delay",
            Longhand::TransitionTimingFunction => "transition-timing-function",
            Longhand::TransitionBehavior => "transition-behavior",
            Longhand::AnimationTrigger => "animation-trigger",
            Longhand::TimelineTrigger => "timeline-trigger",
        }
    }

    fn by_name(name: &str) -> Option<Longhand> {
        Longhand::ALL.into_iter().find(|l| l.name() == name)
    }

    /// The computed value of its initial value, as one item.
    fn initial(self) -> &'static str {
        match self {
            Longhand::AnimationName => "none",
            Longhand::AnimationDuration
            | Longhand::AnimationDelay
            | Longhand::TransitionDuration
            | Longhand::TransitionDelay => "0s",
            Longhand::AnimationIterationCount => "1",
            Longhand::AnimationTimingFunction | Longhand::TransitionTimingFunction => "ease",
            Longhand::AnimationDirection | Longhand::TransitionBehavior => "normal",
            Longhand::AnimationFillMode => "none",
            Longhand::AnimationPlayState => "running",
            Longhand::AnimationComposition => "replace",
            Longhand::TransitionProperty => "all",
            Longhand::AnimationTrigger | Longhand::TimelineTrigger => "none",
        }
    }
}

/// The longhands a property this module reads sets (`-webkit-` names
/// stripped first). The `animation` shorthand also resets the timeline and
/// range longhands, which nothing here reads.
fn longhands_of(property: &str) -> Option<&'static [Longhand]> {
    use Longhand::*;
    const ANIMATION: [Longhand; 8] = [
        AnimationName,
        AnimationDuration,
        AnimationTimingFunction,
        AnimationDelay,
        AnimationIterationCount,
        AnimationDirection,
        AnimationFillMode,
        AnimationPlayState,
    ];
    const TRANSITION: [Longhand; 5] =
        [TransitionProperty, TransitionDuration, TransitionTimingFunction, TransitionDelay, TransitionBehavior];
    Some(match property {
        "animation" => &ANIMATION,
        "transition" => &TRANSITION,
        name => {
            let i = Longhand::ALL.iter().position(|l| l.name() == name)?;
            std::slice::from_ref(&Longhand::ALL[i])
        }
    })
}

/// A declared value of one longhand.
#[derive(Clone, Debug, PartialEq)]
enum Value {
    /// Its list, each item as its computed value serializes (`0.15s`,
    /// `cubic-bezier(0.4, 0, 0.2, 1)`, `opacity`; animation names unquoted;
    /// times in full precision).
    List(Vec<String>),
    /// `initial`, `unset`, `revert`, `revert-layer`: the initial value (none
    /// of these properties inherit).
    Initial,
    /// `inherit`: the parent's value.
    Inherit,
    /// A declaration with `var()` (of `property`, the longhand or a
    /// shorthand setting it): its text, substituted and parsed per element.
    Pending { property: &'static str, text: Rc<str> },
}

/// One declaration: the longhand, its value, `!important`.
type Declared = (Longhand, Value, bool);

struct Rule {
    selectors: List,
    declarations: Vec<Declared>,
    /// The root of the tree whose sheet holds the rule (a document or a shadow root): its
    /// selectors match that tree's elements only (CSS Scoping). `None`: every element.
    scope: Option<NodeId>,
    /// `::part()` names the selectors end with (CSS Shadow Parts): the selectors match the host,
    /// and the rule applies to the elements of the host's shadow tree whose `part` attribute
    /// holds every name. Empty: an ordinary rule.
    part: Vec<String>,
    /// The `::before`/`::after` the selectors ended with (stripped): the rule applies to that
    /// pseudo-element of the elements they match. `None`: to the elements.
    pseudo: Option<PseudoElement>,
}

/// `selector` split at a trailing `::before`/`::after` (or the legacy one-colon forms): the
/// originating element's selector (`*` when there is none) and the pseudo-element.
fn pseudo_selector(selector: &str) -> Option<(String, PseudoElement)> {
    let selector = selector.trim();
    let lower = selector.to_ascii_lowercase();
    for (suffix, pseudo) in [
        ("::before", PseudoElement::Before),
        ("::after", PseudoElement::After),
        (":before", PseudoElement::Before),
        (":after", PseudoElement::After),
    ] {
        if lower.ends_with(suffix) {
            let host = selector[..selector.len() - suffix.len()].trim_end();
            // (Not `::part(x)::before`, nor a pseudo-element before it.)
            if host.ends_with(':') || host.contains("::") {
                return None;
            }
            return Some((if host.is_empty() { "*".to_owned() } else { host.to_owned() }, pseudo));
        }
    }
    None
}

/// `selector` split at a trailing `::part(<names>)`: the host's selector (`*` when there is
/// none) and the names; `None` without one.
fn part_selector(selector: &str) -> Option<(String, Vec<String>)> {
    let selector = selector.trim();
    let start = selector.rfind("::part(")?;
    let inner = selector[start + "::part(".len()..].strip_suffix(')')?;
    let names: Vec<String> = inner.split_whitespace().map(str::to_owned).collect();
    if names.is_empty() || inner.contains(['(', ')', ':']) {
        return None;
    }
    let host = selector[..start].trim();
    Some((if host.is_empty() { "*".to_owned() } else { host.to_owned() }, names))
}

/// Whether element `el`'s `part` attribute holds every one of `names`.
fn has_parts(dom: &Dom, el: NodeId, names: &[String]) -> bool {
    let Some(parts) = dom.element(el).ok().and_then(|e| e.attr_local(&markup5ever::local_name!("part"))) else {
        return false;
    };
    names.iter().all(|name| parts.split_ascii_whitespace().any(|part| part == name))
}

/// One CSS animation an element's style asks for.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationTiming {
    pub name: String,
    /// `animation-duration: auto` (which `duration_ms` shows as 0).
    pub duration_auto: bool,
    /// Its index in the element's `animation-name` list (the other `animation-*` lists repeat to it).
    pub index: usize,
    pub duration_ms: f64,
    pub delay_ms: f64,
    /// `f64::INFINITY` for `infinite`.
    pub iterations: f64,
    /// How it plays: changing these does not start it again
    /// ([`AnimationTiming::same_run`]).
    pub style: AnimationStyle,
    /// pyro: its `animation-trigger` item (`<trigger name> <action> [<action>]`), when not `none`.
    pub trigger: Option<String>,
}

impl AnimationTiming {
    /// Whether `other` is the same run: only a change of its name starts the animation again;
    /// its other `animation-*` values change the running animation (CSS Animations 1 § 3:
    /// "changes to the values of animation properties while the animation is running apply as if
    /// the animation had those values from when it began"; Chromium).
    pub fn same_run(&self, other: &AnimationTiming) -> bool {
        self.name == other.name
    }

    /// Whether `other` gives the run other times (duration, delay, iteration count).
    pub fn same_times(&self, other: &AnimationTiming) -> bool {
        self.duration_ms == other.duration_ms
            && self.duration_auto == other.duration_auto
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
    /// `transition-behavior: allow-discrete`: values that do not interpolate
    /// transition too (flipping halfway).
    pub allow_discrete: bool,
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
    /// `var()` declarations as substituted, parsed: by property and text
    /// (`None`: invalid at computed-value time).
    substituted: std::cell::RefCell<rustc_hash::FxHashMap<(&'static str, String), Option<Rc<Vec<(Longhand, Value)>>>>>,
    /// [`TimingRules::animation_candidates`] as last computed, kept up to date
    /// from the DOM's change journal.
    candidates: std::cell::RefCell<CandidateCache>,
    /// Whether an animation rule's selectors could change their matches on
    /// a text change or a `style` attribute change (`:empty`, `:has()`,
    /// `[style]`): computed once.
    journal_unsafe: std::cell::OnceCell<bool>,
}

/// The animation candidates of one document at one DOM version.
#[derive(Default)]
struct CandidateCache {
    /// DOM identity, version and document they were computed at.
    at: Option<(u64, u64, NodeId)>,
    elements: Vec<NodeId>,
    /// The elements whose `style` attribute declares an animation.
    inline: FxHashSet<NodeId>,
}

/// The cascaded value of each longhand for one element ([`TimingRules::lists`]):
/// `None` is the initial value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TimingLists([Option<Vec<String>>; 16]);

impl TimingLists {
    fn get(&self, longhand: Longhand) -> Option<&Vec<String>> {
        self.0[longhand as usize].as_ref().filter(|l| !l.is_empty())
    }

    /// The `i`th item of a longhand's list, repeated as lists repeat
    /// (css-animations-1 §4.2), else its initial value.
    fn item(&self, longhand: Longhand, i: usize) -> &str {
        match self.get(longhand) {
            Some(list) => list[i % list.len()].as_str(),
            None => longhand.initial(),
        }
    }

    fn is_empty(&self) -> bool {
        self.0.iter().all(Option::is_none)
    }
}

/// An element's computed style, for `var()` and `inherit` (`None`: unstyled).
pub type StyleOf<'a> = &'a dyn Fn(NodeId) -> Option<StyleSnapshot>;

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

    /// [`TimingRules::parse`] of sheets with the root of the tree each belongs to (a document or a
    /// shadow root): a rule matches the elements of its own tree only, as the cascade's do (a
    /// shadow tree's `div` rule does not animate its host or the document's divs).
    pub fn parse_scoped<'a>(sheets: impl Iterator<Item = (NodeId, &'a str)>, viewport: &Viewport) -> TimingRules {
        let mut out = TimingRules::default();
        for (scope, text) in sheets {
            if mentions(text) || text.contains("keyframes") {
                let first = out.rules.len();
                rule_list(&mut Parser::new(text), text, viewport, &mut out);
                for rule in &mut out.rules[first..] {
                    rule.scope = Some(scope);
                }
            }
        }
        out
    }

    /// Whether a rule declares animation or transition properties for a `::before`/`::after`.
    pub fn has_pseudo_rules(&self) -> bool {
        self.rules.iter().any(|rule| rule.pseudo.is_some())
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
    ///
    /// Cached per DOM version: after changes the journal shows to be text or
    /// `style` attributes only (a measuring probe written and read in a loop),
    /// the rules' matches stand and only those elements' inline declarations
    /// are checked again, instead of querying every rule over the document
    /// on each style flush.
    pub fn animation_candidates(&self, dom: &Dom, doc: NodeId) -> Vec<NodeId> {
        if self.keyframes.is_empty() {
            return Vec::new();
        }
        let now = (dom.uid(), dom.version(), doc);
        let mut cache = self.candidates.borrow_mut();
        if cache.at == Some(now) {
            return cache.elements.clone();
        }
        if let Some((uid, since, at_doc)) = cache.at {
            if uid == now.0 && at_doc == doc && !self.journal_unsafe() && self.only_text_or_style_since(dom, since, &cache.inline) {
                cache.at = Some(now);
                return cache.elements.clone();
            }
        }
        let elements = self.candidates(dom, doc, Longhand::AnimationName, "animation");
        let inline = elements.iter().copied().filter(|&id| inline_declares(dom, id, "animation")).collect();
        *cache = CandidateCache { at: Some(now), elements: elements.clone(), inline };
        elements
    }

    /// Whether every change after `since` is a text change or a `style`
    /// attribute change that leaves the element's inline animation
    /// declaration as it was (in `inline` or not).
    fn only_text_or_style_since(&self, dom: &Dom, since: u64, inline: &FxHashSet<NodeId>) -> bool {
        let Some(changes) = dom.changes_since(since) else { return false };
        let style = crate::dom::attr_name_hash("style");
        // Nodes outside the document are never candidates (they are looked up under `doc`):
        // their changes, such as a screenshot library copying `animation-*` onto clones, count
        // for nothing. A removal from or an insertion into a connected parent still counts.
        let outside = |i: u32| dom.slot_live(i) && !dom.n(i).is_connected();
        for change in changes {
            let skip = match change {
                crate::dom::Change::Attr(i, _, _) | crate::dom::Change::Text(i) | crate::dom::Change::State(i) => outside(i),
                crate::dom::Change::Inserted(p, _) | crate::dom::Change::Removed(p, _, _) => outside(p),
                _ => false,
            };
            if skip {
                continue;
            }
            match change {
                crate::dom::Change::Text(_) => {}
                // A text or comment node inserted or removed (`textContent = …`): no selector sees
                // it but `:empty` and `:has()` (journal_unsafe).
                crate::dom::Change::Inserted(_, node) | crate::dom::Change::Removed(_, node, _) => {
                    let id = dom.id_of(node);
                    if dom.check(id).is_err() || dom.n(node).is_element() {
                        return false;
                    }
                }
                crate::dom::Change::Attr(idx, name, false) if name == style => {
                    // A slot freed or reused later shows as a removal or insertion further on.
                    let id = dom.id_of(idx);
                    if inline_declares(dom, id, "animation") != inline.contains(&id) {
                        return false;
                    }
                }
                _ => return false,
            }
        }
        true
    }

    fn journal_unsafe(&self) -> bool {
        *self.journal_unsafe.get_or_init(|| {
            use cssparser::ToCss;
            self.rules.iter().any(|rule| {
                let text = rule.selectors.to_css_string().to_ascii_lowercase();
                text.contains(":empty") || text.contains(":has") || text.contains("[style")
            })
        })
    }

    /// pyro: the element declaring timeline trigger `name` (`timeline-trigger: <name> <timeline>
    /// <range>...`) and that item: `el` itself or its nearest ancestor declaring it, else the last
    /// element in tree order that does (trigger names are document-global without
    /// `trigger-scope`, which is not supported).
    pub fn timeline_trigger(&self, dom: &Dom, doc: NodeId, el: NodeId, name: &str, style_of: StyleOf<'_>) -> Option<(NodeId, String)> {
        let declares = |id: NodeId| -> Option<String> {
            let lists = self.lists(dom, id, style_of);
            lists.get(Longhand::TimelineTrigger)?.iter().find(|item| item.split_whitespace().next() == Some(name)).cloned()
        };
        let mut at = Some(el);
        while let Some(id) = at {
            if let Some(item) = declares(id) {
                return Some((id, item));
            }
            at = dom.parent_element(id).ok().flatten();
        }
        self.candidates(dom, doc, Longhand::TimelineTrigger, "timeline-trigger")
            .into_iter()
            .filter_map(|id| declares(id).map(|item| (id, item)))
            .last()
    }

    fn candidates(&self, dom: &Dom, doc: NodeId, longhand: Longhand, inline_word: &str) -> Vec<NodeId> {
        let mut seen = FxHashSet::default();
        let mut out = Vec::new();
        let Ok(document) = dom.check(doc) else { return out };
        for rule in &self.rules {
            if !rule.declarations.iter().any(|d| d.0 == longhand) {
                continue;
            }
            // The rule's tree: the document's, or a shadow tree of it.
            let root = match rule.scope.map(|scope| dom.check(scope)) {
                Some(Ok(scope)) => scope,
                Some(Err(_)) => continue,
                None => document,
            };
            for idx in crate::stylo::select::query(dom, root, &rule.selectors, false) {
                if !rule.part.is_empty() {
                    // The host's shadow tree's elements with the parts.
                    let Ok(Some(shadow)) = dom.shadow_root(dom.id_of(idx)) else { continue };
                    for id in dom.descendants(shadow).into_iter().flatten() {
                        if has_parts(dom, id, &rule.part) && seen.insert(id.index()) {
                            out.push(id);
                        }
                    }
                    continue;
                }
                if seen.insert(idx) {
                    out.push(dom.id_of(idx));
                }
            }
        }
        if let Ok(descendants) = dom.descendants(doc) {
            for id in descendants {
                let inline = inline_declares(dom, id, inline_word);
                if inline && seen.insert(id.index()) {
                    out.push(id);
                }
            }
        }
        out
    }

    /// The cascaded value of each longhand for `el`: its winning
    /// declaration (inline or rule), `var()` substituted with the custom
    /// properties of `style_of(el)`, `inherit` taken from the parent element.
    pub fn lists(&self, dom: &Dom, el: NodeId, style_of: StyleOf<'_>) -> TimingLists {
        self.lists_for(dom, el, None, style_of)
    }

    /// [`Self::lists`] of `el`'s `pseudo` (`None`: of `el`): the rules for that pseudo-element
    /// apply, its inline style is none, and it inherits from `el`.
    pub fn lists_for(&self, dom: &Dom, el: NodeId, pseudo: Option<PseudoElement>, style_of: StyleOf<'_>) -> TimingLists {
        let mut out = TimingLists::default();
        let Ok(idx) = dom.check(el) else { return out };
        if !dom.n(idx).is_element() {
            return out;
        }
        let inline: Vec<Declared> =
            match dom.element(el).ok().and_then(|e| e.attr_local(&markup5ever::local_name!("style"))) {
                Some(style) if pseudo.is_none() && mentions(style) => declarations(style),
                _ => Vec::new(),
            };
        // The rules that match, with their specificity, once for all longhands: those of the
        // element's own tree.
        let tree_root = dom.root_idx(idx);
        let tree = dom.id_of(tree_root);
        // A part's host: the host of the shadow tree it is in.
        let host = dom.fragment_host(tree).ok().flatten().and_then(|host| dom.check(host).ok());
        let matching: Vec<(u32, usize, &Rule)> = self
            .rules
            .iter()
            .enumerate()
            .filter_map(|(order, rule)| {
                if rule.pseudo != pseudo {
                    return None;
                }
                let (target, target_tree) = if rule.part.is_empty() {
                    (idx, tree)
                } else {
                    let host = host.filter(|_| has_parts(dom, el, &rule.part))?;
                    (host, dom.id_of(dom.root_idx(host)))
                };
                if !rule.scope.is_none_or(|scope| scope == target_tree) {
                    return None;
                }
                let specificity = rule
                    .selectors
                    .slice()
                    .iter()
                    .filter(|s| crate::stylo::select::matches(dom, target, &List::from_one((*s).clone())))
                    .map(|s| s.specificity())
                    .max()?;
                Some((specificity, order, rule))
            })
            .collect();
        if inline.is_empty() && matching.is_empty() {
            return out;
        }
        let cascaded = |longhand: Longhand| -> Option<&Value> {
            let own = inline.iter().rev().find(|d| d.0 == longhand);
            if let Some((_, value, true)) = own {
                return Some(value);
            }
            // (important, specificity, order) of the best rule declaration so far.
            let mut best: Option<((bool, u32, usize), &Value)> = None;
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
        let mut style = None;
        for longhand in Longhand::ALL {
            out.0[longhand as usize] = match cascaded(longhand) {
                None | Some(Value::Initial) => None,
                Some(Value::List(list)) => Some(list.clone()),
                Some(Value::Inherit) if pseudo.is_some() => self.lists(dom, el, style_of).0[longhand as usize].clone(),
                Some(Value::Inherit) => dom
                    .parent_element(el)
                    .ok()
                    .flatten()
                    .and_then(|parent| self.lists(dom, parent, style_of).0[longhand as usize].clone()),
                Some(Value::Pending { property, text }) => {
                    let style = style.get_or_insert_with(|| style_of(el));
                    match self.substitute(property, text, style.as_ref()) {
                        Some(values) => match values.iter().find(|(l, _)| *l == longhand) {
                            Some((_, Value::List(list))) => Some(list.clone()),
                            Some((_, Value::Inherit)) if pseudo.is_some() => {
                                self.lists(dom, el, style_of).0[longhand as usize].clone()
                            }
                            Some((_, Value::Inherit)) => dom
                                .parent_element(el)
                                .ok()
                                .flatten()
                                .and_then(|parent| self.lists(dom, parent, style_of).0[longhand as usize].clone()),
                            _ => None,
                        },
                        None => None,
                    }
                }
            };
        }
        out
    }

    /// A `var()` declaration of `property` substituted with the custom
    /// properties of `style` and parsed (cached by its substituted text);
    /// `None`: invalid at computed-value time (the longhands take their
    /// initial values).
    fn substitute(
        &self,
        property: &'static str,
        text: &str,
        style: Option<&StyleSnapshot>,
    ) -> Option<Rc<Vec<(Longhand, Value)>>> {
        let mut lookup = |name: &str| {
            let style = style?;
            crate::stylo::serialize(style, name).filter(|v| !v.trim().is_empty())
        };
        let text = crate::computed::substitute_vars(text, &mut lookup, 0)?;
        let key = (property, text);
        if let Some(parsed) = self.substituted.borrow().get(&key) {
            return parsed.clone();
        }
        let parsed = parse_property(property, &key.1).map(Rc::new);
        let mut cache = self.substituted.borrow_mut();
        if cache.len() >= 4096 {
            cache.clear();
        }
        cache.insert(key, parsed.clone());
        parsed
    }

    /// What `el`'s style asks for (`style_of` as for [`Self::lists`]).
    pub fn element(&self, dom: &Dom, el: NodeId, style_of: StyleOf<'_>) -> ElementTiming {
        self.element_for(dom, el, None, style_of)
    }

    /// [`Self::element`] for `el`'s `pseudo` (`None`: `el`).
    pub fn element_for(&self, dom: &Dom, el: NodeId, pseudo: Option<PseudoElement>, style_of: StyleOf<'_>) -> ElementTiming {
        let mut out = ElementTiming::default();
        let lists = self.lists_for(dom, el, pseudo, style_of);
        if lists.is_empty() {
            return out;
        }
        let keyword = |longhand: Longhand, i: usize, allowed: &[&str]| {
            let v = lists.item(longhand, i);
            if allowed.contains(&v) { v } else { longhand.initial() }.to_owned()
        };
        if let Some(names) = lists.get(Longhand::AnimationName) {
            for (i, name) in names.iter().enumerate() {
                if name == "none" || !self.has_keyframes(keyframes_name(name)) {
                    continue;
                }
                let name = &keyframes_name(name).to_owned();
                let easing = lists.item(Longhand::AnimationTimingFunction, i);
                out.animations.push(AnimationTiming {
                    name: name.clone(),
                    // (Not declared: `auto`, css-animations-2's initial value; shown as `0s`.)
                    duration_auto: lists.get(Longhand::AnimationDuration).is_none()
                        || lists.item(Longhand::AnimationDuration, i) == "auto",
                    index: i,
                    duration_ms: time_ms(lists.item(Longhand::AnimationDuration, i)).unwrap_or(0.0).max(0.0),
                    delay_ms: time_ms(lists.item(Longhand::AnimationDelay, i)).unwrap_or(0.0),
                    iterations: match lists.item(Longhand::AnimationIterationCount, i) {
                        "infinite" => f64::INFINITY,
                        n => n.parse::<f64>().ok().filter(|n| *n >= 0.0).unwrap_or(1.0),
                    },
                    style: AnimationStyle {
                        easing: (easing != "ease").then(|| easing.to_owned()),
                        direction: keyword(Longhand::AnimationDirection, i, &DIRECTIONS),
                        fill: keyword(Longhand::AnimationFillMode, i, &FILLS),
                        paused: lists.item(Longhand::AnimationPlayState, i) == "paused",
                        composition: keyword(Longhand::AnimationComposition, i, &COMPOSITIONS),
                    },
                    trigger: Some(lists.item(Longhand::AnimationTrigger, i))
                        .filter(|t| !t.is_empty() && *t != "none")
                        .map(str::to_owned),
                });
            }
        }
        if lists.get(Longhand::TransitionDuration).is_some() {
            let all = vec!["all".to_owned()];
            let properties = lists.get(Longhand::TransitionProperty).unwrap_or(&all);
            for (i, property) in properties.iter().enumerate() {
                let duration_ms = time_ms(lists.item(Longhand::TransitionDuration, i)).unwrap_or(0.0).max(0.0);
                let delay_ms = time_ms(lists.item(Longhand::TransitionDelay, i)).unwrap_or(0.0);
                if property == "none" || duration_ms + delay_ms <= 0.0 {
                    continue;
                }
                let easing = lists.item(Longhand::TransitionTimingFunction, i);
                out.transitions.push(TransitionTiming {
                    property: property.clone(),
                    duration_ms,
                    delay_ms,
                    easing: (easing != "ease").then(|| easing.to_owned()),
                    allow_discrete: lists.item(Longhand::TransitionBehavior, i) == "allow-discrete",
                });
            }
        }
        out
    }

    /// `getComputedStyle(el)`'s value of `name` (lowercase; `-webkit-`
    /// aliases too) for the transition and animation longhands this module
    /// cascades and the `transition` and `animation` shorthands, as Chromium
    /// serializes them; `None` for other properties.
    pub fn computed_value(&self, dom: &Dom, el: NodeId, name: &str, style_of: StyleOf<'_>) -> Option<String> {
        self.computed_value_for(dom, el, None, name, style_of)
    }

    /// [`Self::computed_value`] of `el`'s `pseudo` (`getComputedStyle(el, '::before')`).
    pub fn computed_value_for(
        &self,
        dom: &Dom,
        el: NodeId,
        pseudo: Option<PseudoElement>,
        name: &str,
        style_of: StyleOf<'_>,
    ) -> Option<String> {
        let name = name.strip_prefix("-webkit-").unwrap_or(name);
        if name != "transition" && name != "animation" && Longhand::by_name(name).is_none() {
            return None;
        }
        let lists = self.lists_for(dom, el, pseudo, style_of);
        Some(match name {
            "transition" => transition_shorthand(&lists),
            "animation" => animation_shorthand(&lists),
            longhand => {
                let longhand = Longhand::by_name(longhand)?;
                match lists.get(longhand) {
                    Some(list) => list.iter().map(|v| shown(longhand, v)).collect::<Vec<_>>().join(", "),
                    None => longhand.initial().to_owned(),
                }
            }
        })
    }
}

/// A longhand's item as `getComputedStyle` shows it: times in seconds with
/// six significant digits, animation names as identifiers (quoted when they
/// are not one).
fn shown(longhand: Longhand, item: &str) -> String {
    match longhand {
        Longhand::AnimationDuration if item == "auto" => "0s".to_owned(),
        Longhand::AnimationDuration
        | Longhand::AnimationDelay
        | Longhand::TransitionDuration
        | Longhand::TransitionDelay => match time_ms(item) {
            Some(ms) => format!("{}s", crate::computed::fmt_num(ms / 1000.0)),
            None => item.to_owned(),
        },
        Longhand::AnimationName if item == QUOTED_NONE => item.to_owned(),
        Longhand::AnimationName if item != "none" && !is_ident(item) => {
            let mut out = String::new();
            let _ = cssparser::serialize_string(item, &mut out);
            out
        }
        _ => item.to_owned(),
    }
}

/// An `animation-name` item that is the string "none" (`animation-name: "none"`, naming
/// `@keyframes "none"`), told apart from the keyword `none` in the timing lists.
const QUOTED_NONE: &str = "\"none\"";

/// The `@keyframes` name an `animation-name` item names.
fn keyframes_name(item: &str) -> &str {
    if item == QUOTED_NONE { "none" } else { item }
}

/// Whether `text` is one CSS identifier (and not a word the shorthand would read otherwise).
fn is_ident(text: &str) -> bool {
    let mut p = Parser::new(text);
    matches!(p.next(), Ok(Token::Ident(_))) && p.is_exhausted()
}

/// Chromium's computed `transition`: an item per `transition-property`
/// entry, other lists repeated to match, each giving only what is not its
/// initial value (the duration also when a delay follows), `all` when nothing is.
fn transition_shorthand(lists: &TimingLists) -> String {
    use Longhand::*;
    let count = lists.get(TransitionProperty).map_or(1, Vec::len);
    let items: Vec<String> = (0..count)
        .map(|i| {
            let mut parts = Vec::new();
            let property = lists.item(TransitionProperty, i);
            if property != "all" {
                parts.push(property.to_owned());
            }
            let duration = shown(TransitionDuration, lists.item(TransitionDuration, i));
            let delay = shown(TransitionDelay, lists.item(TransitionDelay, i));
            if duration != "0s" || delay != "0s" {
                parts.push(duration);
            }
            let easing = lists.item(TransitionTimingFunction, i);
            if easing != "ease" {
                parts.push(easing.to_owned());
            }
            if delay != "0s" {
                parts.push(delay);
            }
            let behavior = lists.item(TransitionBehavior, i);
            if behavior != "normal" {
                parts.push(behavior.to_owned());
            }
            if parts.is_empty() {
                parts.push("all".to_owned());
            }
            parts.join(" ")
        })
        .collect();
    items.join(", ")
}

/// Chromium's computed `animation`: an item per `animation-name` entry,
/// each giving what is not its initial value (duration, easing, delay,
/// iteration count, direction, fill mode, play state, then the name), `none`
/// when nothing is.
fn animation_shorthand(lists: &TimingLists) -> String {
    use Longhand::*;
    let count = lists.get(AnimationName).map_or(1, Vec::len);
    let items: Vec<String> = (0..count)
        .map(|i| {
            let mut parts = Vec::new();
            let duration = shown(AnimationDuration, lists.item(AnimationDuration, i));
            let delay = shown(AnimationDelay, lists.item(AnimationDelay, i));
            if duration != "0s" || delay != "0s" {
                parts.push(duration);
            }
            for (longhand, value) in [(AnimationTimingFunction, "ease")] {
                let v = lists.item(longhand, i);
                if v != value {
                    parts.push(v.to_owned());
                }
            }
            if delay != "0s" {
                parts.push(delay);
            }
            for longhand in [AnimationIterationCount, AnimationDirection, AnimationFillMode, AnimationPlayState] {
                let v = lists.item(longhand, i);
                if v != longhand.initial() {
                    parts.push(v.to_owned());
                }
            }
            let name = lists.item(AnimationName, i);
            if name != "none" {
                parts.push(shown(AnimationName, name));
            }
            if parts.is_empty() {
                parts.push("none".to_owned());
            }
            parts.join(" ")
        })
        .collect();
    items.join(", ")
}

/// An element's computed style at a style change, kept to tell which
/// properties a later one changed.
pub type StyleSnapshot = style::servo_arc::Arc<style::properties::ComputedValues>;

/// Whether `style` is `display: none` (the element is not rendered).
/// Whether a `::before`/`::after` style generates a box: not `display: none`, and `content` not
/// `normal` or `none`.
pub fn generates_pseudo_box(style: &StyleSnapshot) -> bool {
    use style::values::computed::counters::Content;
    !displays_none(style) && !matches!(style.get_counters().content, Content::Normal | Content::None)
}

/// Whether the animation at `index` of `style` is driven by a scroll or view progress timeline
/// (`animation-timeline` other than `auto`): its progress follows a scroll position.
pub fn timeline_driven(style: &StyleSnapshot, index: usize) -> bool {
    let ui = style.get_ui();
    let count = ui.animation_timeline_count();
    count > 0 && !ui.animation_timeline_at(index % count).is_auto()
}

/// The `animation-timeline` of the animation at `index` of `style`, serialized (`scroll()`,
/// `view(inline)`, `--name`).
pub fn timeline_text(style: &StyleSnapshot, index: usize) -> String {
    use style_traits::ToCss;
    let ui = style.get_ui();
    let count = ui.animation_timeline_count();
    if count == 0 {
        return "auto".to_owned();
    }
    ui.animation_timeline_at(index % count).to_css_string()
}

/// The `animation-range-start` and `animation-range-end` of the animation at `index` of `style`,
/// serialized (`normal`, `contain 0%`, `cover 100%`, `20%`).
pub fn range_text(style: &StyleSnapshot, index: usize) -> (String, String) {
    use style_traits::ToCss;
    let ui = style.get_ui();
    let s = ui.animation_range_start_count().max(1);
    let e = ui.animation_range_end_count().max(1);
    (ui.animation_range_start_at(index % s).to_css_string(), ui.animation_range_end_at(index % e).to_css_string())
}

pub fn displays_none(style: &StyleSnapshot) -> bool {
    style.get_box().display.is_none()
}

/// The longhands `property` (a longhand, a shorthand or `all`) covers whose
/// computed value differs between `before` and `after`: the ones a
/// transition of `property` starts for.
pub fn changed_longhands(before: &StyleSnapshot, after: &StyleSnapshot, property: &str) -> Vec<&'static str> {
    changed_longhands_with(before, after, property, false)
}

/// [`changed_longhands`]; with `allow_discrete` (`transition-behavior:
/// allow-discrete`), values Stylo does not interpolate between count too.
pub fn changed_longhands_with(
    before: &StyleSnapshot,
    after: &StyleSnapshot,
    property: &str,
    allow_discrete: bool,
) -> Vec<&'static str> {
    use style::properties::animated_properties::AnimationValue;
    use style::properties::{PropertyDeclarationId, ShorthandId};
    use style::values::animated::{Animate, Procedure};
    // Different, and values Stylo can animate between (`width: auto` to a
    // length starts none; properties Stylo animates discretely, `float` or
    // `display`, only with `allow_discrete`). Animated values exist for physical longhands
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
                // (A property Stylo animates discretely transitions only with `allow-discrete`,
                // as in Chromium, css-transitions-2 §2.1, even where its `Animate` succeeds; WPT
                // `*-no-interpolation`, `display-interpolation`.)
                (Some(a), Some(b)) => {
                    allow_discrete
                        || (!longhand.is_discrete_animatable()
                            && a.animate(&b, Procedure::Interpolate { progress: 0.5 })
                                .is_ok_and(|v| !crate::animation::defers_matrix(&v)))
                }
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
    // (`timeline-trigger` too: a rule may declare only that, for another element's
    // `animation-trigger`.)
    text.contains("animation") || text.contains("transition") || text.contains("timeline-trigger")
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
                            // `::part()` selectors become rules of their own on the host.
                            let mut plain = Vec::new();
                            // `::before`/`::after` selectors too, by pseudo-element.
                            let mut pseudos: [Vec<String>; 2] = Default::default();
                            for selector in split_selectors(prelude) {
                                if let Some((host, pseudo)) = pseudo_selector(selector) {
                                    pseudos[pseudo as usize].push(host);
                                    continue;
                                }
                                match part_selector(selector) {
                                    Some((host, part)) => {
                                        if let Some(selectors) = crate::stylo::select::parse(&host) {
                                            out.rules.push(Rule {
                                                selectors,
                                                declarations: declarations.clone(),
                                                scope: None,
                                                part,
                                                pseudo: None,
                                            });
                                        }
                                    }
                                    None => plain.push(selector),
                                }
                            }
                            for (pseudo, hosts) in [PseudoElement::Before, PseudoElement::After].into_iter().zip(&pseudos) {
                                if hosts.is_empty() {
                                    continue;
                                }
                                if let Some(selectors) = crate::stylo::select::parse(&hosts.join(",")) {
                                    out.rules.push(Rule {
                                        selectors,
                                        declarations: declarations.clone(),
                                        scope: None,
                                        part: Vec::new(),
                                        pseudo: Some(pseudo),
                                    });
                                }
                            }
                            if !plain.is_empty() {
                                if let Some(selectors) = crate::stylo::select::parse(&plain.join(",")) {
                                    out.rules.push(Rule {
                                        selectors,
                                        declarations,
                                        scope: None,
                                        part: Vec::new(),
                                        pseudo: None,
                                    });
                                }
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

/// The selectors of a selector list's text, split at its top-level commas.
fn split_selectors(prelude: &str) -> Vec<&str> {
    let (mut out, mut depth, mut start) = (Vec::new(), 0i32, 0);
    for (i, c) in prelude.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&prelude[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&prelude[start..]);
    out
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
/// longhands in source order. Invalid ones are dropped; one with `var()`
/// waits for an element to be resolved for ([`Value::Pending`]).
fn declarations(body: &str) -> Vec<Declared> {
    let mut out = Vec::new();
    for d in crate::style::parse_declarations(body) {
        let name = d.name.to_ascii_lowercase();
        let name = name.strip_prefix("-webkit-").unwrap_or(&name);
        let Some(longhands) = longhands_of(name) else { continue };
        let value = d.value.trim();
        if has_var(value) {
            let property = longhands[0].name();
            let property = if longhands.len() > 1 { shorthand_name(longhands) } else { property };
            let text: Rc<str> = Rc::from(value);
            out.extend(longhands.iter().map(|l| (*l, Value::Pending { property, text: text.clone() }, d.important)));
            continue;
        }
        if let Some(values) = parse_property(name, value) {
            out.extend(values.into_iter().map(|(l, v)| (l, v, d.important)));
        }
    }
    out
}

fn shorthand_name(longhands: &[Longhand]) -> &'static str {
    if longhands[0] == Longhand::AnimationName {
        "animation"
    } else {
        "transition"
    }
}

/// Whether a value needs substituting first (`var()`, ASCII case-insensitive).
fn has_var(value: &str) -> bool {
    value.as_bytes().windows(4).any(|w| w.eq_ignore_ascii_case(b"var("))
}

/// `property: value` parsed as the cascade parses it: each longhand it sets
/// with its value; `None` when Stylo rejects it.
fn parse_property(property: &str, value: &str) -> Option<Vec<(Longhand, Value)>> {
    use style::properties::{CSSWideKeyword, PropertyDeclaration};
    let longhands = longhands_of(property)?;
    // pyro: the trigger properties, which Stylo does not parse: their items as written.
    if let [l @ (Longhand::AnimationTrigger | Longhand::TimelineTrigger)] = longhands {
        let value = value.trim();
        let lower = value.to_ascii_lowercase();
        let v = match lower.as_str() {
            "inherit" => Value::Inherit,
            "initial" | "unset" | "revert" | "revert-layer" => Value::Initial,
            "" => return None,
            _ => Value::List(split_commas(value).into_iter().map(|i| i.split_whitespace().collect::<Vec<_>>().join(" ")).collect()),
        };
        return Some(vec![(*l, v)]);
    }
    let block = crate::stylo::declarations::parse_block(&format!("{property}: {value}"));
    let mut out = Vec::with_capacity(longhands.len());
    for longhand in longhands {
        let decl = block.declarations().iter().find(|d| d.id().name() == longhand.name())?;
        let value = match decl {
            PropertyDeclaration::CSSWideKeyword(w) => match w.keyword {
                CSSWideKeyword::Inherit => Value::Inherit,
                _ => Value::Initial,
            },
            PropertyDeclaration::WithVariables(_) => Value::Initial,
            // (The string "none" names `@keyframes "none"`: kept apart from the keyword, `QUOTED_NONE`.)
            PropertyDeclaration::AnimationName(list) => Value::List(
                list.0
                    .iter()
                    .map(|n| match n.as_atom() {
                        None => "none".to_owned(),
                        Some(a) if &**a == "none" => QUOTED_NONE.to_owned(),
                        Some(a) => a.to_string(),
                    })
                    .collect(),
            ),
            PropertyDeclaration::AnimationTimingFunction(list) => Value::List(list.0.iter().map(easing_text).collect()),
            PropertyDeclaration::TransitionTimingFunction(list) => {
                Value::List(list.0.iter().map(easing_text).collect())
            }
            other => {
                let mut css = String::new();
                other.to_css(&mut css).ok()?;
                Value::List(split_commas(&css).into_iter().map(|item| computed_item(*longhand, item.trim())).collect())
            }
        };
        out.push((*longhand, value));
    }
    Some(out)
}

/// A specified easing as its computed value serializes (`step-start` is
/// `steps(1, start)`, `steps(4, end)` is `steps(4)`).
fn easing_text(easing: &style::values::specified::TimingFunction) -> String {
    use style_traits::ToCss;
    easing.to_computed_value_without_context().to_css_string()
}

/// One specified item (Stylo's serialization) as its computed value: times
/// in seconds (full precision), `calc()` of one value unwrapped, an
/// `auto` animation duration `0s`.
fn computed_item(longhand: Longhand, item: &str) -> String {
    let lower = item.to_ascii_lowercase();
    let inner = lower.strip_prefix("calc(").and_then(|v| v.strip_suffix(')')).map(str::trim).unwrap_or(&lower);
    match longhand {
        // (`auto` is kept: a scroll-driven animation's duration fills its range; `shown` and the
        // time-based timing read it as 0s.)
        Longhand::AnimationDuration if inner == "auto" => "auto".to_owned(),
        Longhand::AnimationDuration
        | Longhand::AnimationDelay
        | Longhand::TransitionDuration
        | Longhand::TransitionDelay => match time_ms(inner) {
            Some(ms) => format!("{}s", ms / 1000.0),
            None => inner.to_owned(),
        },
        Longhand::AnimationIterationCount => inner.to_owned(),
        // Custom property names and unknown properties keep their case.
        Longhand::TransitionProperty => item.to_owned(),
        _ => lower,
    }
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

const DIRECTIONS: [&str; 4] = ["normal", "reverse", "alternate", "alternate-reverse"];
const FILLS: [&str; 4] = ["none", "forwards", "backwards", "both"];
const COMPOSITIONS: [&str; 3] = ["replace", "add", "accumulate"];

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

    /// The lists `property: value` declares, by longhand name.
    fn declared(property: &str, value: &str) -> Vec<(&'static str, Vec<String>)> {
        parse_property(property, value)
            .unwrap()
            .into_iter()
            .map(|(l, v)| match v {
                Value::List(list) => (l.name(), list),
                other => panic!("{} is {other:?}", l.name()),
            })
            .collect()
    }

    fn one<'a>(lists: &'a [(&'static str, Vec<String>)], name: &str) -> &'a str {
        let (_, list) = lists.iter().find(|(n, _)| *n == name).unwrap();
        assert_eq!(list.len(), 1, "{name}");
        &list[0]
    }

    #[test]
    fn part_selectors_split_into_host_and_names() {
        assert_eq!(split_selectors("#a::part(x), :is(b, c) d"), vec!["#a::part(x)", " :is(b, c) d"]);
        assert_eq!(part_selector("#shadow::part(target)"), Some(("#shadow".to_owned(), vec!["target".to_owned()])));
        assert_eq!(part_selector("::part(a b)"), Some(("*".to_owned(), vec!["a".to_owned(), "b".to_owned()])));
        assert_eq!(part_selector("x::part(a):hover"), None);
        assert_eq!(part_selector("div > p"), None);
    }

    #[test]
    fn declarations_parse_as_the_cascade_parses_them() {
        let a = declared("animation", "anim 100s -99.9s 2");
        assert_eq!(
            [
                one(&a, "animation-name"),
                one(&a, "animation-duration"),
                one(&a, "animation-delay"),
                one(&a, "animation-iteration-count")
            ],
            ["anim", "100s", "-99.9s", "2"]
        );
        let a = declared("animation", "1ms ease-in infinite alternate both spin");
        assert_eq!(
            [one(&a, "animation-name"), one(&a, "animation-duration"), one(&a, "animation-iteration-count")],
            ["spin", "0.001s", "infinite"]
        );
        // A keyword given already names the animation.
        assert_eq!(one(&declared("animation", "ease ease 1s"), "animation-name"), "ease");
        let a = declared("animation", "spin 1s steps(2, jump-end) -0.5s 3 reverse both paused");
        assert_eq!(
            [
                one(&a, "animation-timing-function"),
                one(&a, "animation-direction"),
                one(&a, "animation-fill-mode"),
                one(&a, "animation-play-state")
            ],
            ["steps(2)", "reverse", "both", "paused"]
        );
        let t = declared("transition", "width 1ms cubic-bezier(0, 0, 1, 1) 2s");
        assert_eq!(
            [
                one(&t, "transition-property"),
                one(&t, "transition-duration"),
                one(&t, "transition-timing-function"),
                one(&t, "transition-delay")
            ],
            ["width", "0.001s", "cubic-bezier(0, 0, 1, 1)", "2s"]
        );
        assert_eq!(
            one(&declared("transition", "clip-path 20s allow-discrete"), "transition-behavior"),
            "allow-discrete"
        );
        assert_eq!(one(&declared("transition", ".3s"), "transition-property"), "all");
        // Computed easings and times; names keep their case, quoted ones lose the quotes.
        let e = declared("transition-timing-function", "step-start, step-end, cubic-bezier(.4,0,.2,1), EASE-IN");
        assert_eq!(e[0].1, ["steps(1, start)", "steps(1)", "cubic-bezier(0.4, 0, 0.2, 1)", "ease-in"]);
        assert_eq!(declared("transition-duration", "calc(1s + 100ms), 150ms")[0].1, ["1.1s", "0.15s"]);
        assert_eq!(declared("animation-name", "Spin, \"a b\", none")[0].1, ["Spin", "a b", "none"]);
        assert_eq!(declared("transition-property", "OPACITY, --Foo")[0].1, ["opacity", "--Foo"]);
        // Invalid declarations are dropped; CSS-wide keywords apply.
        assert!(parse_property("transition-duration", "-1s").is_none());
        assert!(parse_property("animation-iteration-count", "-1").is_none());
        let inherit = parse_property("transition", "inherit").unwrap();
        assert_eq!(inherit.len(), 5);
        assert!(inherit.iter().all(|(_, v)| *v == Value::Inherit));
        assert!(parse_property("transition-delay", "unset").unwrap().iter().all(|(_, v)| *v == Value::Initial));
        assert_eq!(split_commas("a 1s, b steps(2, end) 2s"), ["a 1s", " b steps(2, end) 2s"]);
        assert_eq!(time_ms("-99.9s"), Some(-99900.0));
        assert_eq!(time_ms("30ms"), Some(30.0));
        assert_eq!(time_ms("auto"), None);
    }

    #[test]
    fn var_declarations_wait_for_an_element() {
        let d = declarations("transition: opacity var(--d); transition-timing-function: var(--e, ease-in); color: red");
        assert_eq!(d.len(), 6);
        assert!(matches!(&d[0].1, Value::Pending { property: "transition", .. }));
        assert!(matches!(&d[5].1, Value::Pending { property: "transition-timing-function", .. }));
        let rules = TimingRules::default();
        let resolved = rules.substitute("transition-timing-function", "var(--e, ease-in)", None).unwrap();
        assert_eq!(resolved[0].1, Value::List(vec!["ease-in".into()]));
        // Invalid at computed-value time: no value, no fallback.
        assert!(rules.substitute("transition-duration", "var(--d)", None).is_none());
    }

    #[test]
    fn shorthands_serialize_as_chromium_computes_them() {
        let mut lists = TimingLists::default();
        assert_eq!(transition_shorthand(&lists), "all");
        assert_eq!(animation_shorthand(&lists), "none");
        let set = |lists: &mut TimingLists, l: Longhand, v: &[&str]| {
            lists.0[l as usize] = Some(v.iter().map(|s| s.to_string()).collect());
        };
        set(&mut lists, Longhand::TransitionProperty, &["opacity", "transform", "color"]);
        set(&mut lists, Longhand::TransitionDuration, &["1s", "2s"]);
        assert_eq!(transition_shorthand(&lists), "opacity 1s, transform 2s, color 1s");
        let mut lists = TimingLists::default();
        set(&mut lists, Longhand::TransitionDelay, &["-0.5s"]);
        assert_eq!(transition_shorthand(&lists), "0s -0.5s");
        set(&mut lists, Longhand::TransitionTimingFunction, &["cubic-bezier(0.4, 0, 0.2, 1)"]);
        set(&mut lists, Longhand::TransitionBehavior, &["allow-discrete"]);
        assert_eq!(transition_shorthand(&lists), "0s cubic-bezier(0.4, 0, 0.2, 1) -0.5s allow-discrete");
        let mut lists = TimingLists::default();
        set(&mut lists, Longhand::AnimationName, &["a", "b"]);
        set(&mut lists, Longhand::AnimationDuration, &["1s", "2s"]);
        set(&mut lists, Longhand::AnimationDelay, &["0s", "0.5s"]);
        set(&mut lists, Longhand::AnimationIterationCount, &["infinite"]);
        assert_eq!(animation_shorthand(&lists), "1s infinite a, 2s 0.5s infinite b");
        assert_eq!(shown(Longhand::TransitionDuration, "0.150000001s"), "0.15s");
        assert_eq!(shown(Longhand::AnimationName, "a b"), "\"a b\"");
    }
}

/// Whether `id`'s `style` attribute mentions `word` (`animation`, `transition`).
fn inline_declares(dom: &Dom, id: NodeId, word: &str) -> bool {
    dom.element(id)
        .ok()
        .and_then(|e| e.attr_local(&markup5ever::local_name!("style")))
        .is_some_and(|s| s.contains(word))
}
