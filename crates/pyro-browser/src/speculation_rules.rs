//! Speculation rules (WICG Speculation Rules): the rule-set parser, the per-document registry of
//! rule sets, and the script/header processing that feeds it. This is the CORE the prefetch,
//! No-Vary-Search, document-rules, tags and prerender lanes build on; it starts nothing itself.
//!
//! # Where rule sets come from
//! - `<script type="speculationrules">` (inline JSON; `ScriptKind::SpeculationRules`). On
//!   insertion (or when the parser reaches it) the text is parsed ONCE against the document base
//!   URL at that moment; later text changes, `src` changes and base URL changes do not re-parse
//!   (HTML: the script is "already started"). Removing the element removes its rule set. Invalid
//!   JSON or a non-object fires `error` at the element, then reports a `TypeError` at the window
//!   (both in one task, in that order). A script with `src` fires `error` (in a
//!   task) and never `load`.
//! - The `Speculation-Rules` response header of a TOP-LEVEL document: a structured-field list of
//!   strings, each a URL resolved against the response URL and fetched as
//!   `ResourceKind::SpeculationRules` (`Sec-Fetch-Dest: speculationrules`, CORS, same-origin
//!   credentials, no CSP governing it). A response is used only with status 200-299 and MIME
//!   `application/speculationrules+json`; its body is always decoded as UTF-8; its rules'
//!   relative URLs resolve against the rule set's own URL (`relative_to: "document"` switches to
//!   the document base URL). Failures are console warnings, never window errors.
//!
//! # Interface for the fan-out lanes
//! - Types: [`RuleSet`] (the three rule lists, the set's `tag`, parse `warnings`), [`Rule`] (a
//!   list rule's URLs, or a document rule's [`Predicate`]; `eagerness`, `referrer_policy`, `tag`,
//!   `requires`, `expects_no_vary_search` raw, `target_hint`), [`RuleSetEntry`] (a registered set:
//!   `id`, [`RuleSetSource`], the set), [`Candidate`] (one URL of a list rule with the rule's
//!   settings resolved: its effective `tag` is the rule's, else the set's) and [`DocumentRule`]
//!   (a document rule with its action and set).
//! - Reading: `Browser::speculation_candidates(page)` -> every URL of every LIST rule of every
//!   registered set, in registration order; `Browser::speculation_document_rules(page)` -> the
//!   `source: "document"` rules (the lane that evaluates predicates against `<a>`/`<area>`
//!   links turns them into candidates; [`Predicate`] only holds the parsed shape, URL patterns
//!   as raw JSON for `URLPattern`, selectors as validated strings); `Browser::speculation_sets(page)`
//!   -> the entries. `page` is a [`FrameKey`]; only top-level documents' sets matter to the
//!   prefetch lanes (the registry exists for every frame's document).
//! - Notification: every add/remove of a rule set (a script inserted or removed, a header set
//!   that arrived) calls `Browser::speculation_changed(page, SpeculationChange)` in this file.
//!   Its body is the ONE place a lane hooks in (call its driver there); the hook runs as a
//!   normal Browser task step (no DOM borrow held, no script running). A document's registry is
//!   dropped when its frame commits another document, with no notification (the driver's state
//!   for that document goes with the document's own).
//! - The parser is pure: [`parse`] needs no Browser. [`parse_header_urls`] parses the header.
//!
//! # The support switch
//! `HTMLScriptElement.supports('speculationrules')` answers [`SUPPORTED`]
//! (`pyro_webapi::html::SPECULATION_RULES_SUPPORTED`, the single constant), which is `true` since
//! `prefetch.rs` starts real prefetches from `prefetch` rules (it must not be true for a feature
//! without behaviour). Scripts are processed (parsed, registered, events fired) whatever the
//! switch says, as the registry is what the lanes consume. `document.prerendering` is not defined
//! by this module: the prerender lane adds it with its behaviour, and until then `prerender`
//! rules start nothing and `'prerendering' in document` stays false.
//!
//! Left out here (named so a lane can pick it up): CSP `script-src` / `inline-speculation-rules`
//! for inline sets, and rule sets in `document.write` markup (the written-markup script path in
//! pyro-webapi marks them started and drops them).

// The accessors and types below are the interface of the fan-out lanes; until they land only
// this module's own processing uses them.
#![allow(dead_code)]

use std::collections::HashSet;

use std::rc::Rc;

use pyro_dom::parse::DomRef;
use pyro_dom::NodeId;
use pyro_net::ReferrerPolicy;
use serde_json::{Map, Value};
use url::Url;

use crate::frames::FrameKey;
use crate::loader::ResourceNode;
use crate::{Browser, FetchRequest, Resource, ResourceKind};

/// The support switch (see the module header): what `HTMLScriptElement.supports` reports.
pub const SUPPORTED: bool = pyro_webapi::html::SPECULATION_RULES_SUPPORTED;

/// `eagerness` of a rule: when the speculation starts.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Eagerness {
    Immediate,
    Eager,
    Moderate,
    Conservative,
}

/// A `requires` entry. Only one exists; a rule naming any other is invalid.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Requirement {
    AnonymousClientIpWhenCrossOrigin,
}

/// `target_hint` of a rule: where the speculated navigation is expected to happen.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TargetHint {
    Blank,
    SelfTarget,
}

/// What a rule set asks for: the key of its list.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Prefetch,
    Prerender,
    /// The legacy `prefetch_with_subresources` list.
    PrefetchWithSubresources,
}

/// One URL pattern of an `href_matches` predicate, unparsed: `URLPattern` takes the string (with
/// the predicate's base URL) or the init dictionary.
#[derive(Clone, Debug, PartialEq)]
pub enum UrlPatternInput {
    String(String),
    Init(Map<String, Value>),
}

/// A document rule's `where` predicate (parsed shape only; matching is the document-rules lane's).
#[derive(Clone, Debug, PartialEq)]
pub enum Predicate {
    And(Vec<Predicate>),
    Or(Vec<Predicate>),
    Not(Box<Predicate>),
    /// `href_matches`: matches when a link's URL matches any of the patterns; `base` is the URL
    /// the patterns are relative to (the rule's `relative_to`).
    HrefMatches { patterns: Vec<UrlPatternInput>, base: Url },
    /// `selector_matches`: matches when the link matches any selector (each parses as a selector).
    SelectorMatches(Vec<String>),
}

/// Where a rule's URLs come from.
#[derive(Clone, Debug, PartialEq)]
pub enum RuleSource {
    /// `source: "list"`: these http(s) URLs, resolved at parse time.
    List(Vec<Url>),
    /// `source: "document"`: the document's links matching `where` (`None`: every link).
    Document(Option<Predicate>),
}

/// A parsed speculation rule.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub source: RuleSource,
    pub eagerness: Eagerness,
    /// `referrer_policy` (`None`: absent or the empty string: the document's policy applies).
    pub referrer_policy: Option<ReferrerPolicy>,
    /// The rule's own `tag` (printable ASCII; an invalid one invalidates the rule); see [`Candidate::tag`].
    pub tag: Option<String>,
    pub requires: Vec<Requirement>,
    /// `expects_no_vary_search`: the raw header-syntax string, parsed by the No-Vary-Search lane.
    pub expects_no_vary_search: Option<String>,
    pub target_hint: Option<TargetHint>,
}

/// A parsed rule set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RuleSet {
    pub prefetch: Vec<Rule>,
    pub prerender: Vec<Rule>,
    pub prefetch_with_subresources: Vec<Rule>,
    /// The set's `tag` (an invalid one fails the whole parse).
    pub tag: Option<String>,
    /// Problems that skipped a rule or key; reported as console warnings.
    pub warnings: Vec<String>,
}

impl RuleSet {
    /// The rules of `action`.
    pub fn rules(&self, action: Action) -> &[Rule] {
        match action {
            Action::Prefetch => &self.prefetch,
            Action::Prerender => &self.prerender,
            Action::PrefetchWithSubresources => &self.prefetch_with_subresources,
        }
    }
}

/// A rule set the whole text of which could not be used: reported as a `TypeError`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError(pub String);

/// The URLs [`parse`] resolves against.
pub struct ParseContext<'a> {
    /// The rule set's own base: the document base URL for an inline set, the response URL of a
    /// header-supplied one.
    pub ruleset_base: &'a Url,
    /// The document's base URL (what `relative_to: "document"` names).
    pub document_base: &'a Url,
}

const RULE_KEYS: [&str; 10] = [
    "source",
    "urls",
    "where",
    "relative_to",
    "eagerness",
    "referrer_policy",
    "tag",
    "requires",
    "expects_no_vary_search",
    "target_hint",
];

/// HTML "parse a speculation rule set string": the JSON text of a rule set. Unknown top-level
/// keys are ignored; a rule that is invalid is skipped (a warning in the result); anything that
/// makes the whole text unusable (not JSON, not an object) is an error.
pub fn parse(text: &str, ctx: &ParseContext<'_>) -> Result<RuleSet, ParseError> {
    let value: Value = serde_json::from_str(text).map_err(|e| ParseError(format!("Invalid speculation rules: {e}")))?;
    let Value::Object(top) = value else {
        return Err(ParseError("Invalid speculation rules: the rule set must be a JSON object".into()));
    };
    let mut set = RuleSet::default();
    // An invalid set tag (present, but not a string of printable ASCII) invalidates the whole set.
    if let Some(tag) = top.get("tag") {
        match valid_tag(tag) {
            Some(tag) => set.tag = Some(tag),
            // An invalid rule-set tag makes the whole set invalid (an `error` event at the script).
            None => return Err(ParseError("Invalid speculation rules: \"tag\" must be a string of printable ASCII".into())),
        }
    }
    for (key, action) in
        [("prefetch", Action::Prefetch), ("prerender", Action::Prerender), ("prefetch_with_subresources", Action::PrefetchWithSubresources)]
    {
        let Some(rules) = top.get(key) else { continue };
        let Value::Array(rules) = rules else {
            set.warnings.push(format!("\"{key}\" must be an array of rules: ignored."));
            continue;
        };
        for rule in rules {
            match parse_rule(rule, ctx) {
                Ok(rule) => match action {
                    Action::Prefetch => set.prefetch.push(rule),
                    Action::Prerender => set.prerender.push(rule),
                    Action::PrefetchWithSubresources => set.prefetch_with_subresources.push(rule),
                },
                Err(why) => set.warnings.push(format!("A rule of \"{key}\" was ignored: {why}.")),
            }
        }
    }
    Ok(set)
}

/// A speculation rule tag: a string of printable ASCII (0x20 to 0x7E).
fn valid_tag(value: &Value) -> Option<String> {
    let s = value.as_str()?;
    s.bytes().all(|b| (0x20..=0x7e).contains(&b)).then(|| s.to_owned())
}

/// HTML "parse a speculation rule": `Err` names why the rule is skipped.
fn parse_rule(value: &Value, ctx: &ParseContext<'_>) -> Result<Rule, String> {
    let Value::Object(map) = value else { return Err("a rule must be an object".into()) };
    if let Some(key) = map.keys().find(|k| !RULE_KEYS.contains(&k.as_str())) {
        return Err(format!("unrecognized key \"{key}\""));
    }
    let document_source = match map.get("source") {
        Some(Value::String(s)) if s == "list" => false,
        Some(Value::String(s)) if s == "document" => true,
        Some(_) => return Err("\"source\" must be \"list\" or \"document\"".into()),
        None if map.contains_key("urls") => false,
        None if map.contains_key("where") => true,
        None => return Err("a rule needs \"urls\" or \"where\"".into()),
    };
    let relative_to_document = match map.get("relative_to") {
        None => false,
        Some(Value::String(s)) if s == "document" => true,
        Some(Value::String(s)) if s == "ruleset" => false,
        Some(_) => return Err("\"relative_to\" must be \"ruleset\" or \"document\"".into()),
    };
    let base = if relative_to_document { ctx.document_base } else { ctx.ruleset_base };
    let source = if document_source {
        if map.contains_key("urls") {
            return Err("a document rule has no \"urls\"".into());
        }
        let predicate = match map.get("where") {
            None => None,
            Some(w) => Some(parse_predicate(w, base).ok_or("invalid \"where\" predicate")?),
        };
        RuleSource::Document(predicate)
    } else {
        if map.contains_key("where") {
            return Err("a list rule has no \"where\"".into());
        }
        let Some(Value::Array(urls)) = map.get("urls") else { return Err("\"urls\" must be an array".into()) };
        let mut parsed = Vec::new();
        for url in urls {
            let Value::String(s) = url else { return Err("\"urls\" must hold strings".into()) };
            // A URL that does not parse, or is not http(s), is skipped; the rule stays.
            if let Ok(u) = Url::options().base_url(Some(base)).parse(s) {
                if matches!(u.scheme(), "http" | "https") {
                    parsed.push(u);
                }
            }
        }
        RuleSource::List(parsed)
    };
    let eagerness = match map.get("eagerness") {
        None => {
            if document_source {
                Eagerness::Conservative
            } else {
                Eagerness::Immediate
            }
        }
        Some(Value::String(s)) => match s.as_str() {
            "immediate" => Eagerness::Immediate,
            "eager" => Eagerness::Eager,
            "moderate" => Eagerness::Moderate,
            "conservative" => Eagerness::Conservative,
            _ => return Err("unknown \"eagerness\"".into()),
        },
        Some(_) => return Err("\"eagerness\" must be a string".into()),
    };
    let referrer_policy = match map.get("referrer_policy") {
        None => None,
        Some(Value::String(s)) if s.is_empty() => None,
        Some(Value::String(s)) => Some(ReferrerPolicy::parse(s).ok_or("unknown \"referrer_policy\"")?),
        Some(_) => return Err("\"referrer_policy\" must be a string".into()),
    };
    let mut requires = Vec::new();
    match map.get("requires") {
        None => {}
        Some(Value::Array(items)) => {
            for item in items {
                match item.as_str() {
                    Some("anonymous-client-ip-when-cross-origin") => {
                        requires.push(Requirement::AnonymousClientIpWhenCrossOrigin)
                    }
                    _ => return Err("unknown \"requires\" entry".into()),
                }
            }
        }
        Some(_) => return Err("\"requires\" must be an array".into()),
    }
    let expects_no_vary_search = match map.get("expects_no_vary_search") {
        None => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err("\"expects_no_vary_search\" must be a string".into()),
    };
    // A rule with an invalid tag is skipped; an absent tag is the null tag.
    let tag = match map.get("tag") {
        None => None,
        Some(v) => Some(valid_tag(v).ok_or("\"tag\" must be a string of printable ASCII")?),
    };
    let target_hint = match map.get("target_hint") {
        None => None,
        Some(Value::String(s)) if s == "_blank" => Some(TargetHint::Blank),
        Some(Value::String(s)) if s == "_self" => Some(TargetHint::SelfTarget),
        Some(_) => return Err("\"target_hint\" must be \"_blank\" or \"_self\"".into()),
    };
    Ok(Rule { source, eagerness, referrer_policy, tag, requires, expects_no_vary_search, target_hint })
}

/// HTML "parse a document rule predicate".
fn parse_predicate(value: &Value, base: &Url) -> Option<Predicate> {
    let Value::Object(map) = value else { return None };
    let combinators = ["and", "or", "not"].iter().filter(|k| map.contains_key(**k)).count();
    if combinators > 1 {
        return None;
    }
    if combinators == 1 {
        if map.len() != 1 {
            return None;
        }
        return match map.iter().next()? {
            (k, Value::Array(items)) if k == "and" || k == "or" => {
                let items = items.iter().map(|v| parse_predicate(v, base)).collect::<Option<Vec<_>>>()?;
                Some(if k == "and" { Predicate::And(items) } else { Predicate::Or(items) })
            }
            (k, v) if k == "not" => Some(Predicate::Not(Box::new(parse_predicate(v, base)?))),
            _ => None,
        };
    }
    // `href_matches` and `selector_matches` together are their conjunction.
    if map.is_empty() || map.keys().any(|k| k != "href_matches" && k != "selector_matches") {
        return None;
    }
    let mut parts = Vec::new();
    if let Some(v) = map.get("href_matches") {
        let items: Vec<&Value> = match v {
            Value::Array(items) => items.iter().collect(),
            single => vec![single],
        };
        let patterns = items
            .into_iter()
            .map(|v| match v {
                Value::String(s) => Some(UrlPatternInput::String(s.clone())),
                Value::Object(o) => Some(UrlPatternInput::Init(o.clone())),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;
        parts.push(Predicate::HrefMatches { patterns, base: base.clone() });
    }
    if let Some(v) = map.get("selector_matches") {
        let items: Vec<&Value> = match v {
            Value::Array(items) => items.iter().collect(),
            single => vec![single],
        };
        let selectors = items
            .into_iter()
            .map(|v| v.as_str().filter(|s| pyro_dom::select::parse_selectors(s).is_ok()).map(str::to_owned))
            .collect::<Option<Vec<_>>>()?;
        parts.push(Predicate::SelectorMatches(selectors));
    }
    if parts.len() == 1 {
        parts.pop()
    } else {
        Some(Predicate::And(parts))
    }
}

/// What a `Speculation-Rules` header names: the values of all its header lines joined as one
/// structured-field list; each string member is a URL (resolved against `response_url`, http(s)
/// only). A header that is not a list of items fails as a whole (no URLs); an inner list or a
/// non-string member is skipped. The `Err` is the reason, for a console warning.
pub fn parse_header_urls(values: &[String], response_url: &Url) -> Result<Vec<Url>, String> {
    let joined = values.join(", ");
    let members = parse_sf_list(&joined).ok_or("the Speculation-Rules header is not a structured field list")?;
    let mut out = Vec::new();
    for member in members {
        match member {
            SfMember::String(s) => {
                if let Ok(u) = response_url.join(&s) {
                    if matches!(u.scheme(), "http" | "https") {
                        out.push(u);
                    }
                }
            }
            SfMember::InnerList | SfMember::Other => {}
        }
    }
    Ok(out)
}

#[derive(Debug, PartialEq, Eq)]
enum SfMember {
    String(String),
    InnerList,
    Other,
}

/// A structured field list (RFC 8941 §4.2.1) as far as members' kinds: strings are decoded, an
/// inner list, token, number, boolean or binary is "other". Parameters are checked and dropped.
fn parse_sf_list(input: &str) -> Option<Vec<SfMember>> {
    let b = input.trim_matches(|c| c == ' ' || c == '\t').as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    if b.is_empty() {
        return Some(out);
    }
    loop {
        let member = if b.get(i) == Some(&b'(') {
            i += 1;
            loop {
                while b.get(i) == Some(&b' ') {
                    i += 1;
                }
                if b.get(i) == Some(&b')') {
                    i += 1;
                    break;
                }
                sf_bare_item(b, &mut i)?;
                sf_params(b, &mut i)?;
                if !matches!(b.get(i), Some(b' ') | Some(b')')) {
                    return None;
                }
            }
            sf_params(b, &mut i)?;
            SfMember::InnerList
        } else {
            let item = sf_bare_item(b, &mut i)?;
            sf_params(b, &mut i)?;
            item
        };
        out.push(member);
        while matches!(b.get(i), Some(b' ') | Some(b'\t')) {
            i += 1;
        }
        if i >= b.len() {
            return Some(out);
        }
        if b[i] != b',' {
            return None;
        }
        i += 1;
        while matches!(b.get(i), Some(b' ') | Some(b'\t')) {
            i += 1;
        }
        if i >= b.len() {
            return None;
        }
    }
}

fn sf_params(b: &[u8], i: &mut usize) -> Option<()> {
    while b.get(*i) == Some(&b';') {
        *i += 1;
        while b.get(*i) == Some(&b' ') {
            *i += 1;
        }
        let start = *i;
        if !b.get(*i).is_some_and(|c| c.is_ascii_lowercase() || *c == b'*') {
            return None;
        }
        while b.get(*i).is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(*c, b'_' | b'-' | b'.' | b'*')) {
            *i += 1;
        }
        debug_assert!(*i > start);
        if b.get(*i) == Some(&b'=') {
            *i += 1;
            sf_bare_item(b, i)?;
        }
    }
    Some(())
}

fn sf_bare_item(b: &[u8], i: &mut usize) -> Option<SfMember> {
    match *b.get(*i)? {
        b'"' => {
            *i += 1;
            let mut s = Vec::new();
            loop {
                let c = *b.get(*i)?;
                *i += 1;
                match c {
                    b'\\' => {
                        let e = *b.get(*i)?;
                        *i += 1;
                        if e != b'"' && e != b'\\' {
                            return None;
                        }
                        s.push(e);
                    }
                    b'"' => return Some(SfMember::String(String::from_utf8(s).ok()?)),
                    0x20..=0x7e => s.push(c),
                    _ => return None,
                }
            }
        }
        b':' => {
            *i += 1;
            while *b.get(*i)? != b':' {
                *i += 1;
            }
            *i += 1;
            Some(SfMember::Other)
        }
        b'?' => {
            *i += 1;
            if !matches!(b.get(*i), Some(b'0') | Some(b'1')) {
                return None;
            }
            *i += 1;
            Some(SfMember::Other)
        }
        c if c == b'-' || c.is_ascii_digit() => {
            *i += 1;
            while b.get(*i).is_some_and(|c| c.is_ascii_digit() || *c == b'.') {
                *i += 1;
            }
            Some(SfMember::Other)
        }
        c if c.is_ascii_alphabetic() || c == b'*' => {
            *i += 1;
            while b.get(*i).is_some_and(|c| c.is_ascii_alphanumeric() || b":/!#$%&'*+-.^_`|~".contains(c)) {
                *i += 1;
            }
            Some(SfMember::Other)
        }
        _ => None,
    }
}

/// Identifies a registered rule set within its document (never reused).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct RuleSetId(pub u64);

/// Where a registered rule set came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleSetSource {
    /// A `<script type="speculationrules">` of the document.
    Inline(NodeId),
    /// A `Speculation-Rules` header's URL (its response URL after redirects).
    Header(String),
}

/// A registered rule set.
#[derive(Clone, Debug)]
pub struct RuleSetEntry {
    pub id: RuleSetId,
    pub source: RuleSetSource,
    pub set: RuleSet,
}

/// What changed in a document's registry ([`Browser::speculation_changed`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SpeculationChange {
    Added(RuleSetId),
    Removed(RuleSetId),
}

/// One URL of a list rule, with the rule's (and its set's) settings resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub set: RuleSetId,
    pub action: Action,
    pub url: Url,
    pub eagerness: Eagerness,
    pub referrer_policy: Option<ReferrerPolicy>,
    /// The rule's `tag`, else the set's.
    pub tag: Option<String>,
    pub requires: Vec<Requirement>,
    pub expects_no_vary_search: Option<String>,
    pub target_hint: Option<TargetHint>,
}

/// A `source: "document"` rule with its action and set. `tag` is the effective one.
#[derive(Clone, Debug, PartialEq)]
pub struct DocumentRule {
    pub set: RuleSetId,
    pub action: Action,
    pub rule: Rule,
    pub tag: Option<String>,
}

/// The per-document registry (`DocumentState::speculation`).
#[derive(Default)]
pub(crate) struct SpeculationRules {
    sets: Vec<RuleSetEntry>,
    next_id: u64,
}

impl SpeculationRules {
    pub(crate) fn add(&mut self, source: RuleSetSource, set: RuleSet) -> RuleSetId {
        let id = RuleSetId(self.next_id);
        self.next_id += 1;
        self.sets.push(RuleSetEntry { id, source, set });
        id
    }


    /// Removes the set of the script `node`, if it has one.
    pub(crate) fn remove_script(&mut self, node: NodeId) -> Option<RuleSetId> {
        let at = self.sets.iter().position(|e| e.source == RuleSetSource::Inline(node))?;
        Some(self.sets.remove(at).id)
    }

    pub(crate) fn entries(&self) -> &[RuleSetEntry] {
        &self.sets
    }
}

/// The candidates of `entries`: each URL of each list rule.
fn candidates_of(entries: &[RuleSetEntry]) -> Vec<Candidate> {
    let mut out = Vec::new();
    for entry in entries {
        for action in [Action::Prefetch, Action::Prerender, Action::PrefetchWithSubresources] {
            for rule in entry.set.rules(action) {
                let RuleSource::List(urls) = &rule.source else { continue };
                let tag = rule.tag.clone().or_else(|| entry.set.tag.clone());
                // One candidate per distinct URL of the rule.
                let mut seen = HashSet::new();
                for url in urls.iter().filter(|u| seen.insert(u.as_str().to_owned())) {
                    out.push(Candidate {
                        set: entry.id,
                        action,
                        url: url.clone(),
                        eagerness: rule.eagerness,
                        referrer_policy: rule.referrer_policy,
                        tag: tag.clone(),
                        requires: rule.requires.clone(),
                        expects_no_vary_search: rule.expects_no_vary_search.clone(),
                        target_hint: rule.target_hint,
                    });
                }
            }
        }
    }
    out
}

impl Browser {
    /// Every URL of every list rule of the registered rule sets of `page`'s document.
    pub(crate) fn speculation_candidates(&self, page: FrameKey) -> Vec<Candidate> {
        self.doc(page).map(|d| candidates_of(d.speculation.entries())).unwrap_or_default()
    }

    /// The `source: "document"` rules of the registered rule sets of `page`'s document.
    pub(crate) fn speculation_document_rules(&self, page: FrameKey) -> Vec<DocumentRule> {
        let Some(doc) = self.doc(page) else { return Vec::new() };
        let mut out = Vec::new();
        for entry in doc.speculation.entries() {
            for action in [Action::Prefetch, Action::Prerender, Action::PrefetchWithSubresources] {
                for rule in entry.set.rules(action) {
                    if matches!(rule.source, RuleSource::Document(_)) {
                        out.push(DocumentRule {
                            set: entry.id,
                            action,
                            rule: rule.clone(),
                            tag: rule.tag.clone().or_else(|| entry.set.tag.clone()),
                        });
                    }
                }
            }
        }
        out
    }

    /// The registered rule sets of `page`'s document.
    pub(crate) fn speculation_sets(&self, page: FrameKey) -> Vec<RuleSetEntry> {
        self.doc(page).map(|d| d.speculation.entries().to_vec()).unwrap_or_default()
    }

    /// A rule set was added to or removed from `page`'s document (see the module header). THE
    /// hook point of the lanes that act on rules: they call their driver from here. The registry
    /// already reflects the change; `Browser::speculation_candidates` is the current state.
    pub(crate) fn speculation_changed(&mut self, page: FrameKey, change: SpeculationChange) {
        self.prefetch_rules_changed(page, change);
    }

    /// Registers `set` for `page`'s document, warns about what it skipped, and notifies.
    pub(crate) fn register_rule_set(&mut self, page: FrameKey, source: RuleSetSource, set: RuleSet) {
        if let Some(realm) = self.doc(page).and_then(|d| d.main_frame.main_realm().cloned()) {
            if let Ok(scope) = realm.scope() {
                for warning in &set.warnings {
                    scope.console_warn(&format!("Speculation rules: {warning}"));
                }
            }
        }
        let Some(mut doc) = self.doc_mut(page) else { return };
        let id = doc.speculation.add(source, set);
        drop(doc);
        self.speculation_changed(page, SpeculationChange::Added(id));
    }

    /// A `<script type="speculationrules">` that was prepared (already started): its text is
    /// parsed against the document base URL now and the rule set registered with the document
    /// (`speculation_rules`). Invalid JSON, or a `src` (rule sets are inline only), fires `error`
    /// at the element in a task, never `load`; invalid JSON then also reports a `TypeError`.
    pub(crate) fn add_speculation_rules(&mut self, page: FrameKey, dom: &DomRef, node: NodeId, info: &pyro_dom::script::ScriptInfo, base: &str) {
        if dom.borrow_mut().add_ref(node).is_err() {
            return;
        }
        let element = Rc::new(ResourceNode { dom: dom.clone(), node });
        if info.src.is_some() {
            self.speculation_error_task(page, element, None);
            return;
        }
        // A base that is no URL (it always is one for a loaded document) resolves nothing.
        let base = url::Url::parse(base).unwrap_or_else(|_| url::Url::parse("about:blank").expect("about:blank is a URL"));
        match parse(&info.inline_text, &ParseContext { ruleset_base: &base, document_base: &base }) {
            // A script removed again before its prepare ran (its removal was handled first) has
            // no rule set to give the document.
            Ok(set) if dom.borrow().is_connected(node).unwrap_or(false) => {
                self.register_rule_set(page, RuleSetSource::Inline(node), set)
            }
            Ok(_) => {}
            Err(e) => self.speculation_error_task(page, element, Some(e.0)),
        }
    }

    /// In a task of its own: `error` at the rule set's script element, then (`message`) the
    /// `TypeError` its text earned, reported at the window, so the element's listeners run first.
    fn speculation_error_task(&mut self, page: FrameKey, element: Rc<ResourceNode>, message: Option<String>) {
        let loader_id = self.doc(page).map(|d| d.main_frame.loader_id.clone());
        self.scheduler().post(pyro_runtime::TaskSource::DomManipulation, move |b| {
            if b.doc(page).map(|d| d.main_frame.loader_id.clone()) != loader_id {
                return;
            }
            let Some(bindings) = b.doc(page).and_then(|p| p.main_frame.page_dom.clone()) else { return };
            bindings.unblock_rendering(element.node);
            pyro_webapi::input::speculation_rules_error(&bindings, element.node, message);
        });
    }

    /// A console warning in `page`'s document.
    fn speculation_warn(&self, page: FrameKey, message: &str) {
        let realm = self.doc(page).and_then(|d| d.main_frame.main_realm().cloned());
        if let Some(scope) = realm.as_ref().and_then(|r| r.scope().ok()) {
            scope.console_warn(&format!("Speculation rules: {message}"));
        }
    }

    /// The `Speculation-Rules` header (`values`, one per header line) of the top-level document
    /// `page` just committed from the response at `response_url`: every URL it names is fetched
    /// now (`ResourceKind::SpeculationRules`), and a response that is a rule set is registered
    /// ([`RuleSetSource::Header`]). Whatever fails is a console warning only.
    pub(crate) fn speculation_header(&mut self, page: FrameKey, response_url: &str, values: &[String]) {
        const MAX_HEADER_RULE_SETS: usize = 16;
        let Ok(response_url) = Url::parse(response_url) else { return };
        let urls = match parse_header_urls(values, &response_url) {
            Ok(urls) => urls,
            Err(why) => return self.speculation_warn(page, &why),
        };
        let Some(frame) = self.doc(page).map(|d| d.main_frame.clone()) else { return };
        let Some(loader) = self.loader_for(page) else { return };
        for url in urls.into_iter().take(MAX_HEADER_RULE_SETS) {
            let loader_id = frame.loader_id.clone();
            let finish = move |b: &mut Browser, result: Result<Resource, String>| {
                // Another document replaced the one that asked.
                if !b.doc(page).is_some_and(|d| d.main_frame.loader_id == loader_id) {
                    return;
                }
                let r = match result {
                    Ok(r) if (200..300).contains(&r.status) => r,
                    Ok(r) => return b.speculation_warn(page, &format!("{} answered {}: ignored.", r.url, r.status)),
                    Err(e) => return b.speculation_warn(page, &format!("could not fetch a rule set: {e}")),
                };
                if r.mime != "application/speculationrules+json" {
                    return b.speculation_warn(page, &format!("{} has MIME type \"{}\", not application/speculationrules+json: ignored.", r.url, r.mime));
                }
                let Ok(ruleset_base) = Url::parse(&r.url) else { return };
                let document_base = b
                    .doc(page)
                    .and_then(|d| Url::parse(&d.main_frame.base_url).ok())
                    .unwrap_or_else(|| ruleset_base.clone());
                // Always UTF-8, whatever the response's charset says.
                let text = pyro_webapi::module_script::utf8_decode(&r.body);
                match parse(&text, &ParseContext { ruleset_base: &ruleset_base, document_base: &document_base }) {
                    Ok(set) => b.register_rule_set(page, RuleSetSource::Header(r.url.clone()), set),
                    Err(e) => b.speculation_warn(page, &e.0),
                }
            };
            let request = FetchRequest {
                csp: None,
                required_csp: None,
                purpose: None,
                parser_priority: false,
                credentials: None,
                post: None,
                url: url.to_string(),
                kind: ResourceKind::SpeculationRules,
                page: page.page,
                referrer: frame.navigation_referrer(),
                referrer_policy: None,
                frame_id: Some(frame.id.0.clone()),
                loader_id: Some(frame.loader_id.0.clone()),
                crossorigin: None,
                initiator: None,
                module: None,
                part: None,
            };
            if let Some(cancel) = self.fetch_document_resource(page, &loader, request, Box::new(finish)) {
                cancel.disarm();
            }
        }
    }

    /// The element of a registered inline rule set left the document.
    pub(crate) fn speculation_script_removed(&mut self, page: FrameKey, node: NodeId) {
        let Some(mut doc) = self.doc_mut(page) else { return };
        let removed = doc.speculation.remove_script(node);
        drop(doc);
        if let Some(id) = removed {
            self.speculation_changed(page, SpeculationChange::Removed(id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://example.com/dir/page.html").unwrap()
    }

    fn parse_str(text: &str) -> Result<RuleSet, ParseError> {
        let b = base();
        parse(text, &ParseContext { ruleset_base: &b, document_base: &b })
    }

    #[test]
    fn invalid_json_and_non_objects_are_errors() {
        assert!(parse_str("invalid json").is_err());
        assert!(parse_str("[]").is_err());
        assert!(parse_str("").is_err());
        assert!(parse_str("{}").is_ok());
    }

    #[test]
    fn a_list_rule_resolves_urls_and_defaults_to_immediate() {
        let set = parse_str(r#"{"prefetch":[{"urls":["a.html","https://other.test/x","mailto:a@b.c","javascript:1"]}],"unknown":1}"#).unwrap();
        let rule = &set.prefetch[0];
        assert_eq!(rule.eagerness, Eagerness::Immediate);
        let RuleSource::List(urls) = &rule.source else { panic!() };
        let urls: Vec<&str> = urls.iter().map(Url::as_str).collect();
        assert_eq!(urls, ["https://example.com/dir/a.html", "https://other.test/x"]);
    }

    #[test]
    fn an_unrecognized_key_or_bad_value_skips_the_rule() {
        let set = parse_str(
            r#"{"prefetch":[{"urls":["a"],"invalid_key":"value"},{"urls":["b"],"eagerness":"never"},
                {"urls":["c"],"referrer_policy":"no-referrrrrrrer"},{"urls":["d"],"requires":["x"]},{"urls":["e"]}]}"#,
        )
        .unwrap();
        assert_eq!(set.prefetch.len(), 1);
        assert_eq!(set.warnings.len(), 4);
    }

    #[test]
    fn the_empty_referrer_policy_is_valid_and_tags_follow_printable_ascii() {
        assert!(parse_str(r#"{"tag":"é","prefetch":[{"urls":["a"]}]}"#).is_err());
        assert!(parse_str(r#"{"tag":null}"#).is_err());
        assert!(parse_str(r#"{"tag":" ","prefetch":[{"urls":["a"],"tag":"\u007f"},{"urls":["b"],"tag":7}]}"#).unwrap().prefetch.is_empty());
        let set = parse_str(
            r#"{"tag":"set","prefetch":[{"urls":["a"],"referrer_policy":"","tag":"rule"},{"urls":["b"],"tag":"é"},{"urls":["c"]}]}"#,
        )
        .unwrap();
        // The rule with the non-ASCII tag is skipped.
        assert_eq!(set.prefetch.len(), 2);
        assert_eq!(set.prefetch[0].referrer_policy, None);
        assert_eq!(set.prefetch[0].tag.as_deref(), Some("rule"));
        assert_eq!(set.prefetch[1].tag, None);
        assert_eq!(set.tag.as_deref(), Some("set"));
        assert!(parse_str(r#"{"tag":null,"prefetch":[{"urls":["a"]}]}"#).is_err());
    }

    #[test]
    fn document_rules_parse_their_predicate() {
        let set = parse_str(
            r#"{"prerender":[{"source":"document","where":{"and":[{"href_matches":"/p/*"},{"not":{"selector_matches":".no"}}]}},
                {"where":{"selector_matches":"["}},{"source":"document"}]}"#,
        )
        .unwrap();
        // The invalid selector skips its rule; the rule without `where` matches every link.
        assert_eq!(set.prerender.len(), 2);
        assert_eq!(set.prerender[0].eagerness, Eagerness::Conservative);
        assert!(matches!(&set.prerender[0].source, RuleSource::Document(Some(Predicate::And(items))) if items.len() == 2));
        assert!(matches!(set.prerender[1].source, RuleSource::Document(None)));
    }

    #[test]
    fn relative_to_document_resolves_against_the_document() {
        let ruleset = Url::parse("https://cdn.test/rules/set.json").unwrap();
        let document = base();
        let ctx = ParseContext { ruleset_base: &ruleset, document_base: &document };
        let set = parse(r#"{"prefetch":[{"urls":["x"]},{"urls":["x"],"relative_to":"document"}]}"#, &ctx).unwrap();
        let first = |r: &Rule| match &r.source {
            RuleSource::List(u) => u[0].as_str().to_owned(),
            _ => panic!(),
        };
        assert_eq!(first(&set.prefetch[0]), "https://cdn.test/rules/x");
        assert_eq!(first(&set.prefetch[1]), "https://example.com/dir/x");
    }

    #[test]
    fn the_header_is_a_list_of_strings() {
        let b = Url::parse("https://example.com/a/b").unwrap();
        let urls = |v: &[&str]| parse_header_urls(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>(), &b);
        assert_eq!(urls(&["\"rules.json\""]).unwrap()[0].as_str(), "https://example.com/a/rules.json");
        assert_eq!(urls(&["\"/x\", (\"/y\"), \"/z\";q=1, token, 5"]).unwrap().len(), 2);
        assert_eq!(urls(&["\"/x\"", "\"/y\""]).unwrap().len(), 2);
        assert!(urls(&["\"unterminated"]).is_err());
        assert!(urls(&["\"/x\" \"/y\""]).is_err());
        assert!(urls(&["\"/x\","]).is_err());
        assert_eq!(urls(&[""]).unwrap().len(), 0);
    }

    #[test]
    fn candidates_take_the_rules_tag_over_the_sets() {
        let set = parse_str(r#"{"tag":"s","prefetch":[{"urls":["a","a"],"tag":"r"},{"urls":["b"]}]}"#).unwrap();
        let mut registry = SpeculationRules::default();
        registry.add(RuleSetSource::Header("https://example.com/r".into()), set);
        let candidates = candidates_of(registry.entries());
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].tag.as_deref(), Some("r"));
        assert_eq!(candidates[1].tag.as_deref(), Some("s"));
    }
}
