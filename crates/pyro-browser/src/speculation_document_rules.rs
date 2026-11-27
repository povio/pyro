//! Speculation rules: document rules (`source: "document"`) and the eagerness triggers.
//!
//! The prefetch driver asks ONE function, [`Browser::document_rule_candidates`], for the
//! candidates that the document's links and the user's pointer make active right now, and is told
//! when they changed ([`Browser::speculation_changed`] with `SpeculationChange::Added`, whose id
//! is any set involved: a driver treats every notification as "read the candidates again").
//!
//! # What the function returns
//! - For each document rule of each registered rule set, one [`Candidate`] per distinct URL of
//!   the links the rule's `where` predicate selects, in document order. A link is an `<a>` or
//!   `<area>` of the document (shadow trees included) with an `href` that resolves to an http(s)
//!   URL, that is rendered (not `display: none`, not inside `content-visibility: hidden`
//!   contents, and in the flat tree: a light child of a shadow host that no slot takes has no
//!   box), without `rel=nofollow`, and that is not a fragment link to the document itself. The
//!   candidate's referrer policy is the rule's, else the link's `rel=noreferrer` /
//!   `referrerpolicy`.
//! - The list-rule candidates whose eagerness is not `immediate` once the user interacted with a
//!   link to their URL (a list rule's non-immediate candidates only apply to links of the
//!   document). Immediate list candidates stay with [`Browser::speculation_candidates`]; a
//!   driver starts those (`eagerness == Immediate` only: it must not start the others of that
//!   function) plus this function's result.
//! - Every returned candidate is eligible NOW: its `eagerness` is reported as `Immediate` (the
//!   rule's own is what the trigger waited for), so a driver starts all of them.
//!
//! # Eagerness
//! A rule's candidates are active when its eagerness is reached for the link's URL (not for the
//! rule's source): `immediate` always; `eager` once a link to it is hovered; `moderate` once it
//! was hovered for [`MODERATE_HOVER`] or pressed; `conservative` once it was pressed (even when
//! the page cancels `pointerdown`: the embedder reports the press before dispatching it). Once
//! reached, it stays reached for the document.
//!
//! # Re-evaluation
//! [`Browser::document_rules_after_task`] (one call in the checkpoint) re-evaluates after a task
//! when the document holds a rule set with a document rule and something it depends on changed:
//! the DOM, the style sheets or hover/focus state (the style engine's observation stamp), the
//! document URL (a `history.pushState` changes which links point at the document), the document
//! base URL (`href_matches` patterns resolve against it for inline rule sets), or the rule sets.
//! At most one evaluation per [`MIN_GAP`]: a later change is picked up by a timer. A document
//! without document rules pays one scan of its rule sets per task.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use pyro_dom::{Dom, NodeId};
use pyro_net::ReferrerPolicy;
use url::Url;

use crate::frames::FrameKey;
use crate::speculation_rules::{
    Candidate, Eagerness, Predicate, RuleSetId, RuleSetSource, RuleSource, SpeculationChange, SpeculationRules, UrlPatternInput,
};
use crate::{Browser, LoaderId, PageId};

/// How long a link is hovered before `moderate` candidates start (Chrome: 200 ms).
pub(crate) const MODERATE_HOVER: Duration = Duration::from_millis(200);
/// The shortest time between two evaluations of a document's links.
const MIN_GAP: Duration = Duration::from_millis(50);

/// What the pointer did to a link, as the highest eagerness it reaches.
const HOVER_EAGER: u8 = 1;
const HOVER_MODERATE: u8 = 2;
const POINTER_DOWN: u8 = 3;

/// The trigger level a candidate of `eagerness` waits for.
fn needed(eagerness: Eagerness) -> u8 {
    match eagerness {
        Eagerness::Immediate => 0,
        Eagerness::Eager => HOVER_EAGER,
        Eagerness::Moderate => HOVER_MODERATE,
        Eagerness::Conservative => POINTER_DOWN,
    }
}

/// The URL a trigger is recorded under: a link to `u#x` and one to `u` start the same candidate.
fn trigger_key(url: &Url) -> String {
    let mut url = url.clone();
    url.set_fragment(None);
    url.into()
}

/// What an evaluation read, to know whether another is needed.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Signature {
    /// `StyleEngine::observation_stamp`: DOM version, sheet generation, focused and hovered element.
    stamp: (u64, u64, u32, u32),
    document_url: String,
    base_url: String,
    sets: Vec<u64>,
}

struct Hover {
    node: NodeId,
}

/// The document rules state of one top-level document.
struct DocState {
    loader: LoaderId,
    signature: Option<Signature>,
    /// The candidates the driver was last told about.
    candidates: Vec<Candidate>,
    /// Trigger level reached per [`trigger_key`].
    triggers: HashMap<String, u8>,
    hover: Option<Hover>,
    /// Bumped when the hovered link changes: a hover timer of an older value is stale.
    serial: u64,
    last_eval: Option<Instant>,
    /// A timer will evaluate again (a change came inside [`MIN_GAP`]).
    recheck: bool,
}

impl DocState {
    fn new(loader: LoaderId) -> Self {
        DocState {
            loader,
            signature: None,
            candidates: Vec::new(),
            triggers: HashMap::new(),
            hover: None,
            serial: 0,
            last_eval: None,
            recheck: false,
        }
    }
}

/// [`Browser::document_rules`]: the state of each top-level document that has any.
#[derive(Default)]
pub(crate) struct DocumentRulesState {
    docs: HashMap<PageId, DocState>,
}

fn rules_of(set: &crate::speculation_rules::RuleSet) -> impl Iterator<Item = &crate::speculation_rules::Rule> {
    set.prefetch.iter().chain(&set.prerender).chain(&set.prefetch_with_subresources)
}

fn has_document_rule(registry: &SpeculationRules) -> bool {
    registry.entries().iter().any(|e| rules_of(&e.set).any(|r| matches!(r.source, RuleSource::Document(_))))
}

/// A link of the document with what the candidates take from it.
struct Link {
    node: NodeId,
    url: Url,
    referrer_policy: Option<ReferrerPolicy>,
}

/// Whether the `rel` attribute value has the token.
fn rel_has(rel: &str, token: &str) -> bool {
    rel.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case(token))
}

/// HTML "flat tree" membership of the link: every light child of a shadow host on the way up must
/// be assigned to a slot (an unslotted one is not rendered, and neither is what is inside it).
fn in_flat_tree(dom: &Dom, node: NodeId) -> bool {
    let mut cur = node;
    // Bounded: a tree never nests deeper than the DOM's own limits.
    for _ in 0..100_000 {
        let Ok(Some(parent)) = dom.parent(cur) else { return true };
        if dom.is_shadow_root(parent).unwrap_or(false) {
            match dom.fragment_host(parent) {
                Ok(Some(host)) => cur = host,
                _ => return true,
            }
            continue;
        }
        if dom.shadow_root(parent).ok().flatten().is_some() && dom.element(cur).is_ok() {
            match dom.assigned_slot_any(cur) {
                Some(slot) => cur = slot,
                None => return false,
            }
            continue;
        }
        cur = parent;
    }
    false
}

/// The `<a>` / `<area>` elements with an `href` under `root`, shadow trees included, in tree order.
fn collect_link_nodes(dom: &Dom, root: NodeId, out: &mut Vec<NodeId>) {
    let Ok(nodes) = dom.descendants(root) else { return };
    for node in nodes {
        let Ok(element) = dom.element(node) else { continue };
        if element.is_html() && matches!(&*element.name.local, "a" | "area") && dom.get_attribute(node, "href").ok().flatten().is_some() {
            out.push(node);
        }
        if let Some(shadow) = dom.shadow_root(node).ok().flatten() {
            collect_link_nodes(dom, shadow, out);
        }
    }
}

/// The links of the document that can be candidates (see the module header), with their URLs.
fn document_links(dom: &Dom, layout: &pyro_dom::layout::Layout, root: NodeId, document_url: Option<&Url>) -> Vec<Link> {
    let mut nodes = Vec::new();
    collect_link_nodes(dom, root, &mut nodes);
    let mut out = Vec::new();
    for node in nodes {
        let Some(href) = dom.get_attribute(node, "href").ok().flatten() else { continue };
        let rel = dom.get_attribute(node, "rel").ok().flatten().unwrap_or("");
        if rel_has(rel, "nofollow") {
            continue;
        }
        let Ok(url) = Url::parse(&pyro_webapi::html::resolve_url(dom, node, href)) else { continue };
        if !matches!(url.scheme(), "http" | "https") {
            continue;
        }
        // A fragment link to the document itself is a same-document navigation.
        if url.fragment().is_some() && document_url.is_some_and(|d| trigger_key(d) == trigger_key(&url)) {
            continue;
        }
        if !in_flat_tree(dom, node) {
            continue;
        }
        // Without a box (display: none, up the flat tree) or in skipped contents: not a link
        // anyone can follow.
        if !layout.check_visibility(dom, node, Default::default()) || layout.in_display_locked_contents(dom, node) {
            continue;
        }
        let referrer_policy = if rel_has(rel, "noreferrer") {
            Some(ReferrerPolicy::NoReferrer)
        } else {
            dom.get_attribute(node, "referrerpolicy").ok().flatten().and_then(ReferrerPolicy::parse)
        };
        out.push(Link { node, url, referrer_policy });
    }
    out
}

/// Evaluates document rule predicates against the links of one document.
struct Evaluator<'a, 's> {
    bindings: &'a std::rc::Rc<pyro_webapi::PageDom>,
    links: &'a [Link],
    urls: Vec<String>,
    scope: Option<&'a pyro_js::Scope<'s>>,
    /// The document base URL now, which `href_matches` of an inline rule set resolves against.
    document_base: &'a str,
}

impl Evaluator<'_, '_> {
    /// For each link, whether it matches `predicate`; `dynamic_base`: patterns resolve against
    /// the document base URL as it is now (an inline rule set's), not as it was at parsing.
    fn eval(&self, predicate: &Predicate, dynamic_base: bool) -> Vec<bool> {
        let n = self.links.len();
        match predicate {
            Predicate::And(items) => {
                let mut out = vec![true; n];
                for item in items {
                    for (o, r) in out.iter_mut().zip(self.eval(item, dynamic_base)) {
                        *o &= r;
                    }
                }
                out
            }
            Predicate::Or(items) => {
                let mut out = vec![false; n];
                for item in items {
                    for (o, r) in out.iter_mut().zip(self.eval(item, dynamic_base)) {
                        *o |= r;
                    }
                }
                out
            }
            Predicate::Not(inner) => self.eval(inner, dynamic_base).into_iter().map(|b| !b).collect(),
            Predicate::HrefMatches { patterns, base } => {
                let Some(scope) = self.scope else { return vec![false; n] };
                let patterns: Vec<pyro_webapi::PatternInput> = patterns
                    .iter()
                    .map(|p| match p {
                        UrlPatternInput::String(s) => pyro_webapi::PatternInput::String(s.clone()),
                        UrlPatternInput::Init(map) => pyro_webapi::PatternInput::Init(map.clone()),
                    })
                    .collect();
                let base = if dynamic_base { self.document_base } else { base.as_str() };
                // An invalid pattern makes the predicate match nothing.
                pyro_webapi::href_matches(scope, &patterns, base, &self.urls).unwrap_or_else(|| vec![false; n])
            }
            Predicate::SelectorMatches(selectors) => {
                let parsed: Vec<_> = selectors.iter().filter_map(|s| pyro_dom::select::parse_selectors(s).ok()).collect();
                // Borrowed only here: the `href_matches` leaves run script helpers.
                let Ok(dom) = self.bindings.dom.try_borrow() else { return vec![false; n] };
                self.links.iter().map(|l| parsed.iter().any(|s| dom.matches(l.node, s).unwrap_or(false))).collect()
            }
        }
    }
}

impl Browser {
    /// The candidates the links and the pointer of `page`'s document make active now (see the
    /// module header): the document rules' links, and the list rules' non-immediate candidates
    /// the user reached. The prefetch driver starts these and the immediate list candidates of
    /// [`Browser::speculation_candidates`].
    pub(crate) fn document_rule_candidates(&self, page: FrameKey) -> Vec<Candidate> {
        let mut out = Vec::new();
        let Some(loader) = self.doc(page).map(|d| d.main_frame.loader_id.clone()) else { return out };
        let triggers = self.document_rules.docs.get(&page.page).filter(|s| s.loader == loader).map(|s| &s.triggers);
        let reached = |eagerness: Eagerness, url: &Url| {
            let need = needed(eagerness);
            need == 0 || triggers.and_then(|t| t.get(&trigger_key(url))).is_some_and(|&level| level >= need)
        };
        for candidate in self.speculation_candidates(page) {
            if candidate.eagerness != Eagerness::Immediate && reached(candidate.eagerness, &candidate.url) {
                out.push(Candidate { eagerness: Eagerness::Immediate, ..candidate });
            }
        }
        let rules = self.speculation_document_rules(page);
        if rules.is_empty() {
            return out;
        }
        let (bindings, realm, inline_sets) = {
            let Some(doc) = self.doc(page) else { return out };
            let Some(bindings) = doc.main_frame.page_dom.clone() else { return out };
            let inline: HashSet<RuleSetId> =
                doc.speculation.entries().iter().filter(|e| matches!(e.source, RuleSetSource::Inline(_))).map(|e| e.id).collect();
            (bindings, doc.main_frame.main_realm().cloned(), inline)
        };
        // The borrows end with this block: the predicates run script helpers.
        let (links, document_base) = {
            // The layout first: it may need the DOM itself to bring its styles up to date.
            let Some(layout) = bindings.try_layout() else { return out };
            let Ok(dom) = bindings.dom.try_borrow() else { return out };
            let doc_node = bindings.document();
            let document_url = dom.document_url_parsed(doc_node).ok().flatten().cloned();
            let document_base = dom.document_base_url(doc_node).unwrap_or("").to_owned();
            (document_links(&dom, &layout, doc_node, document_url.as_ref()), document_base)
        };
        if links.is_empty() {
            return out;
        }
        let scope = realm.as_ref().and_then(|r| r.scope().ok());
        let evaluator = Evaluator {
            bindings: &bindings,
            links: &links,
            urls: links.iter().map(|l| l.url.to_string()).collect(),
            scope: scope.as_ref(),
            document_base: &document_base,
        };
        for document_rule in rules {
            let RuleSource::Document(predicate) = &document_rule.rule.source else { continue };
            let matched = match predicate {
                Some(p) => evaluator.eval(p, inline_sets.contains(&document_rule.set)),
                None => vec![true; links.len()],
            };
            let mut seen = HashSet::new();
            for (link, _) in links.iter().zip(matched).filter(|(_, m)| *m) {
                let rule = &document_rule.rule;
                if !reached(rule.eagerness, &link.url) || !seen.insert(link.url.to_string()) {
                    continue;
                }
                out.push(Candidate {
                    set: document_rule.set,
                    action: document_rule.action,
                    url: link.url.clone(),
                    eagerness: Eagerness::Immediate,
                    referrer_policy: rule.referrer_policy.or(link.referrer_policy),
                    tag: document_rule.tag.clone(),
                    requires: rule.requires.clone(),
                    expects_no_vary_search: rule.expects_no_vary_search.clone(),
                    target_hint: rule.target_hint,
                });
            }
        }
        out
    }

    /// The checkpoint's call: every top-level document that holds a document rule is evaluated
    /// again when something its candidates depend on changed (see the module header).
    pub(crate) fn document_rules_after_task(&mut self) {
        // The prefetches of a document that went away (or expired) stop being usable and are freed.
        self.sweep_prefetches();
        let pages: Vec<PageId> = self.pages.iter().filter(|p| has_document_rule(&p.document.speculation)).map(|p| p.id).collect();
        if pages.is_empty() && self.document_rules.docs.is_empty() {
            return;
        }
        // A document that left (or lost its document rules) has nothing to keep.
        if !self.document_rules.docs.is_empty() {
            let alive: HashSet<PageId> = self.pages.iter().map(|p| p.id).collect();
            self.document_rules.docs.retain(|id, _| alive.contains(id));
        }
        for id in pages {
            self.document_rules_check(FrameKey::from(id), false);
        }
    }

    /// The state of `page`'s current document, made when it has none (or has its predecessor's).
    fn rules_state(&mut self, page: FrameKey) -> Option<&mut DocState> {
        let loader = self.doc(page)?.main_frame.loader_id.clone();
        let slot = self.document_rules.docs.entry(page.page).or_insert_with(|| DocState::new(loader.clone()));
        if slot.loader != loader {
            *slot = DocState::new(loader);
        }
        Some(slot)
    }

    /// Evaluates `page`'s candidates when what they depend on changed (or `forced`), and tells
    /// the driver when they are not what it was last told.
    fn document_rules_check(&mut self, page: FrameKey, forced: bool) {
        if page.child.is_some() {
            return;
        }
        let Some(bindings) = self.doc(page).and_then(|d| d.main_frame.page_dom.clone()) else { return };
        let sets: Vec<u64> = self.doc(page).map(|d| d.speculation.entries().iter().map(|e| e.id.0).collect()).unwrap_or_default();
        let signature = {
            let Some(layout) = bindings.try_layout() else { return };
            let Ok(dom) = bindings.dom.try_borrow() else { return };
            let doc_node = bindings.document();
            Signature {
                stamp: layout.style().observation_stamp(&dom),
                document_url: dom.document_url_parsed(doc_node).ok().flatten().map_or_else(String::new, |u| u.to_string()),
                base_url: dom.document_base_url(doc_node).unwrap_or("").to_owned(),
                sets,
            }
        };
        let Some(state) = self.rules_state(page) else { return };
        if !forced {
            if state.signature.as_ref() == Some(&signature) {
                return;
            }
            if state.last_eval.is_some_and(|t| t.elapsed() < MIN_GAP) {
                if !state.recheck {
                    state.recheck = true;
                    let loader = state.loader.clone();
                    self.scheduler().set_timer(MIN_GAP, false, false, move |b: &mut Browser| {
                        if let Some(s) = b.rules_state(page).filter(|s| s.loader == loader) {
                            s.recheck = false;
                            b.document_rules_check(page, false);
                        }
                    });
                }
                return;
            }
        }
        state.signature = Some(signature);
        state.last_eval = Some(Instant::now());
        let candidates = self.document_rule_candidates(page);
        let Some(state) = self.rules_state(page) else { return };
        if state.candidates == candidates {
            return;
        }
        let set = candidates.first().map(|c| c.set).or_else(|| self.doc(page).and_then(|d| d.speculation.entries().first().map(|e| e.id)));
        if let Some(state) = self.rules_state(page) {
            state.candidates = candidates;
        }
        if let Some(set) = set {
            self.speculation_changed(page, SpeculationChange::Added(set));
        }
    }

    /// Records on the page whether its document's rule sets wait for a pointer, for the embedder hook
    /// that keeps hover and press tracking free otherwise.
    pub(crate) fn sync_pointer_rules(&self, page: FrameKey) {
        if page.child.is_some() {
            return;
        }
        let wants = self.wants_pointer(page);
        if let Some(p) = self.page(page.page) {
            p.frames.pointer_rules.set(wants);
        }
    }

    /// Whether any rule of the document waits for a pointer (so hover and press matter).
    fn wants_pointer(&self, page: FrameKey) -> bool {
        self.doc(page).is_some_and(|d| d.speculation.entries().iter().any(|e| rules_of(&e.set).any(|r| r.eagerness != Eagerness::Immediate)))
    }

    /// The URL (fragment kept: the key strips it) `link` of `page`'s document points at, when http(s).
    fn link_trigger_key(&self, page: FrameKey, link: NodeId) -> Option<String> {
        let bindings = self.doc(page)?.main_frame.page_dom.clone()?;
        let dom = bindings.dom.try_borrow().ok()?;
        let href = dom.get_attribute(link, "href").ok().flatten()?;
        let url = Url::parse(&pyro_webapi::html::resolve_url(&dom, link, href)).ok()?;
        matches!(url.scheme(), "http" | "https").then(|| trigger_key(&url))
    }

    /// Records that the user reached `level` on `key` and evaluates again when it is new.
    fn raise_trigger(&mut self, page: FrameKey, key: &str, level: u8) {
        let Some(state) = self.rules_state(page) else { return };
        let reached = state.triggers.entry(key.to_owned()).or_insert(0);
        if *reached >= level {
            return;
        }
        *reached = level;
        self.document_rules_check(page, true);
    }

    /// The pointer is over `link` now (`None`: over no link), reported by the embedder's
    /// `PageHooks::link_hover` in a task of its own. `eager` candidates start at once and
    /// `moderate` ones after [`MODERATE_HOVER`] on the same link.
    pub(crate) fn speculation_link_hover(&mut self, page: FrameKey, link: Option<NodeId>) {
        if page.child.is_some() || !self.wants_pointer(page) {
            return;
        }
        let found = link.and_then(|node| self.link_trigger_key(page, node).map(|key| (node, key)));
        let Some(state) = self.rules_state(page) else { return };
        if state.hover.as_ref().map(|h| h.node) == found.as_ref().map(|f| f.0) {
            return;
        }
        state.serial += 1;
        let serial = state.serial;
        let loader = state.loader.clone();
        state.hover = found.as_ref().map(|f| Hover { node: f.0 });
        let Some((_, key)) = found else { return };
        self.raise_trigger(page, &key, HOVER_EAGER);
        self.scheduler().set_timer(MODERATE_HOVER, false, true, move |b: &mut Browser| {
            // Still the same hover of the same document.
            if b.rules_state(page).is_some_and(|s| s.loader == loader && s.serial == serial) {
                b.raise_trigger(page, &key, HOVER_MODERATE);
            }
        });
    }

    /// A press went down on `link` (`PageHooks::link_pointer_down`): every eagerness is reached.
    pub(crate) fn speculation_link_pointer_down(&mut self, page: FrameKey, link: NodeId) {
        if page.child.is_some() || !self.wants_pointer(page) {
            return;
        }
        if let Some(key) = self.link_trigger_key(page, link) {
            self.raise_trigger(page, &key, POINTER_DOWN);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eagerness_levels_are_ordered() {
        assert_eq!(needed(Eagerness::Immediate), 0);
        assert!(needed(Eagerness::Eager) < needed(Eagerness::Moderate));
        assert!(needed(Eagerness::Moderate) < needed(Eagerness::Conservative));
        // A press reaches every eagerness, a hover of 200 ms not the conservative one.
        assert!(POINTER_DOWN >= needed(Eagerness::Conservative));
        assert!(HOVER_MODERATE < needed(Eagerness::Conservative));
    }

    #[test]
    fn a_trigger_ignores_the_fragment() {
        let a = Url::parse("https://example.com/p?x=1#top").unwrap();
        let b = Url::parse("https://example.com/p?x=1").unwrap();
        assert_eq!(trigger_key(&a), trigger_key(&b));
    }

    #[test]
    fn rel_tokens_are_ascii_case_insensitive() {
        assert!(rel_has("noopener NoFollow", "nofollow"));
        assert!(!rel_has("nofollowing", "nofollow"));
    }
}
