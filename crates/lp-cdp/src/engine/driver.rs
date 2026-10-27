//! `LP.*`: a native test driver.
//!
//! `LP.run` takes a whole scenario, a list of steps (`goto`, `route`, `act`,
//! `check`, `read`, `eval`, `request`), and replies once, when the last step
//! holds or the first one fails, with the values read and the requests the
//! page made meanwhile (`request` steps check that log).
//! `LP.act` and `LP.expect` are one-step runs. Locators (with `within` and
//! `filter` chains) are resolved against the arena DOM in Rust and the
//! actionability checks (unique, visible, enabled, editable, not covered) run
//! here, so a scenario costs one round trip and no JavaScript.
//!
//! A run whose current step cannot finish yet becomes a [`Waiter`]. Nothing
//! polls: a waiting step is tried again only when the page's DOM version moves
//! (or its document is replaced), a `goto` when its document reaches `load`,
//! and an action's settle when the requests it started are done.
//! `EngineHost::run_until_work` returns to the serve loop when a waiter is
//! ready, and `EngineHost::flush` drives the ready ones before sending queued
//! events.
//!
//! Settle: after an action (unless `settle: false`), the next step waits until
//! the page has no more requests in flight than before the action, then for
//! the tasks already queued to run (a marker task that re-queues itself while
//! others are pending). It gives up after [`SETTLE_MAX`] and carries on.
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use lp_browser::{Browser, NavigateOpts, PageId};
use lp_dom::select::SelectorCache;
use lp_dom::{Dom, NodeId, NodeKind};
use lp_runtime::{Scheduler, TaskSource};
use lp_webapi::input::{self, MouseButton, MouseInput, MouseKind};
use lp_webapi::PageDom;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{EngineHost, Queued};
use crate::msg::{self, CdpError, CdpResult};
use crate::target::{PageCommand, PageKey};

const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
/// Waiting runs per connection (each holds only its parsed steps).
const MAX_WAITERS: usize = 1024;
const MAX_STEPS: usize = 10_000;
/// Longest an action waits for the requests it started.
const SETTLE_MAX: Duration = Duration::from_millis(1_000);
/// How many times the settle marker lets queued tasks go first.
const MARKER_HOPS: u32 = 4;

/// What a locator names. Exactly one of `css`, `testId`, `text`, `role`,
/// `label` or `placeholder`, searched inside each match of `within` (else the
/// document), narrowed by `filter`; `nth` then picks one match instead of
/// requiring a unique one.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Locator {
    css: Option<String>,
    test_id: Option<String>,
    /// Attribute `testId` matches (default `data-testid`).
    test_id_attribute: Option<String>,
    text: Option<Pattern>,
    role: Option<String>,
    /// Accessible name for `role`.
    name: Option<Pattern>,
    label: Option<Pattern>,
    placeholder: Option<Pattern>,
    /// Whole-string, case-sensitive match for text, name, label and
    /// placeholder (after whitespace normalization); otherwise a
    /// case-insensitive substring. A `{"regex"}` pattern ignores it.
    #[serde(default)]
    exact: bool,
    within: Option<Box<Locator>>,
    filter: Option<Filter>,
    /// Index into the matches (negative counts from the end).
    nth: Option<i64>,
}

/// Narrows a locator's matches. Every given condition must hold.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Filter {
    visible: Option<bool>,
    /// Case-insensitive substring of the element's text.
    has_text: Option<Pattern>,
    has_not_text: Option<Pattern>,
    /// Matches at least one element inside.
    has: Option<Box<Locator>>,
    has_not: Option<Box<Locator>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Click,
    Fill,
    Focus,
    Check,
    Uncheck,
    /// Focus, then a key press (`value`: a key name such as `Enter`, or a
    /// chord such as `Shift+Tab`).
    Press,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Assert {
    Visible,
    Hidden,
    Text,
    ContainsText,
    Value,
    Count,
    Checked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Read {
    Count,
    Text,
    Value,
    Visible,
    Box,
}

/// Which requests a `request` step looks for in the run's log.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RequestMatch {
    method: Option<String>,
    /// Exact path (no query).
    path: Option<String>,
    /// Substring of (or regex on) the full URL.
    url: Option<Pattern>,
    status: Option<u16>,
    /// Exactly this many finished matches (default: at least one).
    count: Option<usize>,
}

#[derive(Clone, Debug)]
enum Step {
    /// Waits for the current document's `load` (a prepared page taken while
    /// it was still booting).
    Loaded,
    Goto {
        url: String,
        dom_ready: bool,
    },
    /// Same-document navigation: `history.pushState` and a `popstate` event,
    /// as a client-side router reacts to; settles like an action.
    Route {
        url: String,
    },
    Eval {
        expression: String,
        key: Option<String>,
    },
    Request(RequestMatch),
    Act {
        locator: Locator,
        action: Action,
        value: String,
        settle: bool,
    },
    Check {
        locator: Locator,
        assert: Assert,
        expected: Value,
        not: bool,
    },
    Read {
        locator: Locator,
        read: Read,
        key: Option<String>,
    },
}

impl Step {
    fn describe(&self) -> String {
        match self {
            Step::Goto { url, .. } => format!("goto {url}"),
            Step::Loaded => "loaded".into(),
            Step::Route { url } => format!("route {url}"),
            Step::Eval { .. } => "eval".into(),
            Step::Request(m) => format!("request {}", serde_json::to_value(m).map(prune).unwrap_or_default()),
            Step::Act { locator, action, .. } => format!("{action:?} {}", locator.describe()),
            Step::Check { locator, assert, .. } => format!("expect {assert:?} {}", locator.describe()),
            Step::Read { locator, read, .. } => format!("read {read:?} {}", locator.describe()),
        }
    }
}

/// One try of a step.
enum Attempt {
    Done(Value),
    /// Not yet: the reason is reported if the deadline passes.
    Retry(String),
}

/// A command's steps and what they produced so far.
struct Job {
    id: u64,
    session: String,
    page: PageKey,
    pid: PageId,
    steps: Vec<(Step, Duration)>,
    index: usize,
    results: Vec<Value>,
    reads: serde_json::Map<String, Value>,
    /// `LP.act` / `LP.expect`: reply with the step's own result.
    single: bool,
    started: Instant,
    /// Request log sequence when the run started (`LP.run` only).
    log_from: Option<usize>,
}

impl Job {
    fn reply(mut self, requests: Vec<Value>) -> Value {
        if self.single {
            return self.results.pop().unwrap_or_else(|| json!({}));
        }
        json!({
            "results": self.results,
            "reads": self.reads,
            "requests": requests,
            "ms": self.started.elapsed().as_secs_f64() * 1e3,
        })
    }

    fn fail(&self, error: CdpError) -> CdpError {
        if self.single {
            return error;
        }
        let what = self.steps.get(self.index).map(|(s, _)| s.describe()).unwrap_or_default();
        CdpError::server(format!("step {} ({what}): {}", self.index, error.message))
    }
}

/// Where the current step is.
enum Phase {
    /// Tried; tried again when the document changes.
    Try,
    /// `goto` started; waits for the new document's `load` (or DOMContentLoaded).
    Load { loader: String, dom_ready: bool },
    /// The step's action ran; waits for the requests it started.
    Settle { loader: String, baseline: usize, marker: Option<Rc<Cell<bool>>>, since: Instant, until: Instant },
}

pub(super) struct Waiter {
    job: Job,
    phase: Phase,
    step_deadline: Instant,
    /// Document and DOM version the last try saw.
    doc: Weak<PageDom>,
    seen: u64,
    reason: String,
}

impl Waiter {
    fn deadline(&self) -> Instant {
        match &self.phase {
            Phase::Settle { until, .. } => *until,
            _ => self.step_deadline,
        }
    }

    /// A `request` step: tried again when the request log grows.
    fn waits_on_log(&self) -> bool {
        matches!(self.phase, Phase::Try) && matches!(self.job.steps.get(self.job.index), Some((Step::Request(_), _)))
    }

    /// Whether driving it can make progress.
    fn ready(&self, browser: &Browser, now: Instant) -> bool {
        if now >= self.deadline() {
            return true;
        }
        match &self.phase {
            Phase::Try if self.waits_on_log() => false,
            Phase::Try => match self.doc.upgrade() {
                Some(doc) => doc.is_disposed() || doc.dom.borrow().version() != self.seen,
                None => true,
            },
            Phase::Load { loader, dom_ready } => load_reached(browser, self.job.pid, loader, *dom_ready),
            Phase::Settle { loader, baseline, marker, .. } => match marker {
                Some(done) => done.get(),
                None => inflight(browser, self.job.pid, loader) <= *baseline,
            },
        }
    }
}

#[derive(Default)]
pub(crate) struct Waiters {
    list: Vec<Waiter>,
    selectors: RefCell<SelectorCache>,
    /// A page's request log grew since the waiters were last serviced.
    pub(super) log_changed: bool,
}

impl Waiters {
    /// Whether a waiter can make progress. Called between loop tasks.
    pub(super) fn ready(&self, browser: &Browser) -> bool {
        if self.list.is_empty() {
            return false;
        }
        let now = Instant::now();
        self.list.iter().any(|w| w.ready(browser, now))
    }

    pub(super) fn next_deadline(&self) -> Option<Instant> {
        self.list.iter().map(Waiter::deadline).min()
    }

    pub(super) fn clear(&mut self) {
        self.list.clear();
    }
}

fn load_reached(browser: &Browser, pid: PageId, loader: &str, dom_ready: bool) -> bool {
    match browser.page(pid) {
        None => true,
        Some(p) => {
            let f = &p.main_frame;
            f.loader_id.0 == loader && if dom_ready { f.dom_content_loaded } else { f.loaded }
        }
    }
}

/// The page's current document loader and its requests in flight.
fn net_state(browser: &Browser, pid: PageId) -> (String, usize) {
    let Some(p) = browser.page(pid) else { return (String::new(), 0) };
    let loader = p.main_frame.loader_id.0.clone();
    let n = inflight(browser, pid, &loader);
    (loader, n)
}

fn inflight(browser: &Browser, pid: PageId, loader: &str) -> usize {
    browser
        .page(pid)
        .and_then(|p| browser.context_net(p.context))
        .and_then(|net| net.activity(loader))
        .map_or(0, |a| a.inflight)
}

/// Sets `done` from a task that runs after the tasks queued now; while others
/// are queued behind it, it goes to the back again (up to `hops` times).
fn post_marker(sched: Scheduler<Browser>, done: Rc<Cell<bool>>, hops: u32) {
    let next = sched.clone();
    sched.post(TaskSource::Internal, move |_| {
        if hops > 0 && next.pending_tasks() > 0 {
            post_marker(next, done, hops - 1);
        } else {
            done.set(true);
        }
    });
}

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn text_matches(have: &str, want: &str, exact: bool) -> bool {
    let have = normalize(have);
    if exact {
        have == normalize(want)
    } else {
        have.to_lowercase().contains(&normalize(want).to_lowercase())
    }
}

/// A string to match (see `Locator::exact`), or `{"regex": "...", "flags": "i"}`
/// tested against the whitespace-normalized text.
#[derive(Clone, Debug)]
pub(super) struct Pattern {
    text: String,
    regex: Option<regex_lite::Regex>,
}

impl<'de> Deserialize<'de> for Pattern {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Text(String),
            Regex {
                regex: String,
                #[serde(default)]
                flags: String,
            },
        }
        Ok(match Raw::deserialize(d)? {
            Raw::Text(text) => Pattern { text, regex: None },
            Raw::Regex { regex, flags } => {
                let mut b = regex_lite::RegexBuilder::new(&regex);
                for f in flags.chars() {
                    match f {
                        'i' => b.case_insensitive(true),
                        'm' => b.multi_line(true),
                        's' => b.dot_matches_new_line(true),
                        _ => return Err(serde::de::Error::custom(format!("unsupported regex flag {f:?}"))),
                    };
                }
                let re = b.build().map_err(serde::de::Error::custom)?;
                Pattern { text: regex, regex: Some(re) }
            }
        })
    }
}

impl Serialize for Pattern {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match &self.regex {
            Some(_) => json!({ "regex": self.text }).serialize(s),
            None => self.text.serialize(s),
        }
    }
}

impl Pattern {
    fn matches(&self, have: &str, exact: bool) -> bool {
        match &self.regex {
            Some(re) => re.is_match(&normalize(have)),
            None => text_matches(have, &self.text, exact),
        }
    }
}

fn local(dom: &Dom, id: NodeId) -> Option<&str> {
    match &dom.node(id).ok()?.kind {
        NodeKind::Element(e) if e.is_html() => Some(&e.name.local),
        _ => None,
    }
}

fn attr<'d>(dom: &'d Dom, id: NodeId, name: &str) -> Option<&'d str> {
    dom.element(id).ok()?.attr(name)
}

fn text_of(dom: &Dom, id: NodeId) -> String {
    dom.text_content(id).ok().flatten().unwrap_or_default()
}

fn elements(dom: &Dom, root: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    dom.descendants(root).into_iter().flatten().filter(|n| dom.element(*n).is_ok())
}

/// Implicit ARIA role (the common HTML-AAM mappings), or the first token of
/// an explicit `role`.
fn role_of(dom: &Dom, id: NodeId) -> Option<String> {
    if let Some(role) = attr(dom, id, "role").and_then(|r| r.split_whitespace().next()) {
        return Some(role.to_ascii_lowercase());
    }
    let ty = || attr(dom, id, "type").unwrap_or("").to_ascii_lowercase();
    let role = match local(dom, id)? {
        "button" => "button",
        "a" | "area" if attr(dom, id, "href").is_some() => "link",
        "input" => match ty().as_str() {
            "button" | "submit" | "reset" | "image" => "button",
            "checkbox" => "checkbox",
            "radio" => "radio",
            "range" => "slider",
            "number" => "spinbutton",
            "search" => "searchbox",
            "hidden" | "file" | "color" | "date" | "datetime-local" | "month" | "time" | "week" => return None,
            _ if attr(dom, id, "list").is_some() => "combobox",
            _ => "textbox",
        },
        "textarea" => "textbox",
        "select" => {
            if attr(dom, id, "multiple").is_some() || attr(dom, id, "size").is_some_and(|s| s.trim() > "1") {
                "listbox"
            } else {
                "combobox"
            }
        }
        "option" => "option",
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => "heading",
        "img" if attr(dom, id, "alt") == Some("") => "presentation",
        "img" => "img",
        "ul" | "ol" | "menu" => "list",
        "li" => "listitem",
        "nav" => "navigation",
        "main" => "main",
        "header" => "banner",
        "footer" => "contentinfo",
        "aside" => "complementary",
        "dialog" => "dialog",
        "article" => "article",
        "table" => "table",
        "tr" => "row",
        "td" => "cell",
        "th" => "columnheader",
        "progress" => "progressbar",
        "form" => "form",
        _ => return None,
    };
    Some(role.into())
}

/// Roles whose accessible name comes from their content.
fn name_from_content(role: &str) -> bool {
    matches!(
        role,
        "button"
            | "link"
            | "heading"
            | "checkbox"
            | "radio"
            | "option"
            | "listitem"
            | "tab"
            | "menuitem"
            | "cell"
            | "columnheader"
            | "row"
            | "switch"
            | "tooltip"
            | "treeitem"
    )
}

/// Text of the `<label>`s of a form control: `label[for=id]` and an
/// enclosing label.
fn label_text(dom: &Dom, doc: NodeId, id: NodeId) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(own) = attr(dom, id, "id").filter(|v| !v.is_empty()) {
        for label in elements(dom, doc) {
            if local(dom, label) == Some("label") && attr(dom, label, "for") == Some(own) {
                parts.push(text_of(dom, label));
            }
        }
    }
    let mut cur = dom.parent(id).ok().flatten();
    while let Some(p) = cur {
        if local(dom, p) == Some("label") && attr(dom, p, "for").is_none() {
            parts.push(text_of(dom, p));
            break;
        }
        cur = dom.parent(p).ok().flatten();
    }
    let joined = normalize(&parts.join(" "));
    (!joined.is_empty()).then_some(joined)
}

/// The accessible name, by the common steps of accname 1.2: labelledby,
/// aria-label, native labels and alternatives, content, title.
fn accessible_name(dom: &Dom, doc: NodeId, id: NodeId, role: &str) -> String {
    if let Some(ids) = attr(dom, id, "aria-labelledby") {
        let text: Vec<String> = ids
            .split_whitespace()
            .filter_map(|r| dom.get_element_by_id(doc, r).ok().flatten())
            .map(|n| text_of(dom, n))
            .collect();
        let text = normalize(&text.join(" "));
        if !text.is_empty() {
            return text;
        }
    }
    if let Some(label) = attr(dom, id, "aria-label").map(normalize).filter(|l| !l.is_empty()) {
        return label;
    }
    match local(dom, id) {
        Some("input") => {
            let ty = attr(dom, id, "type").unwrap_or("").to_ascii_lowercase();
            match ty.as_str() {
                "submit" | "reset" | "button" => {
                    let fallback = match ty.as_str() {
                        "submit" => "Submit",
                        "reset" => "Reset",
                        _ => "",
                    };
                    return normalize(attr(dom, id, "value").unwrap_or(fallback));
                }
                "image" => return normalize(attr(dom, id, "alt").unwrap_or("Submit")),
                _ => {
                    if let Some(label) = label_text(dom, doc, id) {
                        return label;
                    }
                }
            }
        }
        Some("textarea" | "select" | "button" | "meter" | "progress") => {
            if let Some(label) = label_text(dom, doc, id) {
                return label;
            }
        }
        Some("img" | "area") => {
            if let Some(alt) = attr(dom, id, "alt") {
                return normalize(alt);
            }
        }
        _ => {}
    }
    if name_from_content(role) {
        let text = normalize(&text_of(dom, id));
        if !text.is_empty() {
            return text;
        }
    }
    normalize(attr(dom, id, "title").unwrap_or(""))
}

fn skipped_for_text(dom: &Dom, id: NodeId) -> bool {
    matches!(local(dom, id), Some("script" | "style" | "noscript" | "template" | "head" | "title"))
}

/// The smallest elements whose text matches: an element whose text matches
/// and none of whose child elements' text does.
fn by_text(dom: &Dom, root: NodeId, want: &Pattern, exact: bool, out: &mut Vec<NodeId>) -> bool {
    let children: Vec<NodeId> = dom
        .children(root)
        .into_iter()
        .flatten()
        .filter(|c| dom.element(*c).is_ok() && !skipped_for_text(dom, *c))
        .collect();
    let here = want.matches(&text_of(dom, root), exact);
    // A substring match in a child implies one here: prune subtrees that
    // cannot match. Exact and regex matches need the whole walk.
    if !here && !exact && want.regex.is_none() {
        return false;
    }
    let before = out.len();
    let mut child_matched = false;
    for c in children {
        child_matched |= by_text(dom, c, want, exact, out);
    }
    if here && !child_matched && out.len() == before {
        out.push(root);
        return true;
    }
    child_matched || here
}

/// Drops nulls and `false` from a serialized locator, for messages.
fn prune(v: Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.into_iter()
                .filter(|(_, v)| !v.is_null() && *v != Value::Bool(false))
                .map(|(k, v)| (k, prune(v)))
                .collect(),
        ),
        v => v,
    }
}

impl Locator {
    fn describe(&self) -> String {
        serde_json::to_value(self).map(prune).map(|v| v.to_string()).unwrap_or_default()
    }

    fn validate(&self) -> CdpResult<()> {
        let kinds = [
            self.css.is_some(),
            self.test_id.is_some(),
            self.text.is_some(),
            self.role.is_some(),
            self.label.is_some(),
            self.placeholder.is_some(),
        ];
        if kinds.iter().filter(|k| **k).count() != 1 {
            return Err(CdpError::invalid_params(
                "locator needs exactly one of css, testId, text, role, label, placeholder",
            ));
        }
        if let Some(within) = &self.within {
            within.validate()?;
        }
        if let Some(f) = &self.filter {
            for l in [&f.has, &f.has_not].into_iter().flatten() {
                l.validate()?;
            }
        }
        Ok(())
    }

    fn resolve(&self, page: &PageDom, selectors: &RefCell<SelectorCache>) -> CdpResult<Vec<NodeId>> {
        self.resolve_at(page, selectors, page.document())
    }

    /// Matches inside `base` (the document, or the element a `has` filter tests).
    fn resolve_at(&self, page: &PageDom, selectors: &RefCell<SelectorCache>, base: NodeId) -> CdpResult<Vec<NodeId>> {
        let roots = match &self.within {
            Some(outer) => outer.resolve_at(page, selectors, base)?,
            None => vec![base],
        };
        let mut found = Vec::new();
        if let [root] = roots[..] {
            found = self.matches_in(page, selectors, root)?;
        } else {
            let mut seen = HashSet::new();
            for root in roots {
                found.extend(self.matches_in(page, selectors, root)?.into_iter().filter(|n| seen.insert(*n)));
            }
        }
        if let Some(f) = &self.filter {
            let mut kept = Vec::with_capacity(found.len());
            for n in found {
                if f.keeps(page, selectors, n)? {
                    kept.push(n);
                }
            }
            found = kept;
        }
        if let Some(nth) = self.nth {
            let len = found.len() as i64;
            let i = if nth < 0 { len + nth } else { nth };
            return Ok(if (0..len).contains(&i) { vec![found[i as usize]] } else { Vec::new() });
        }
        Ok(found)
    }

    /// Matches strictly inside `root` (the document, or a `within` match).
    fn matches_in(&self, page: &PageDom, selectors: &RefCell<SelectorCache>, root: NodeId) -> CdpResult<Vec<NodeId>> {
        let dom = page.dom.borrow();
        let doc = page.document();
        let inside = |n: &NodeId| *n != root;
        let mut found = Vec::new();
        if let Some(css) = &self.css {
            let sel = selectors.borrow_mut().get(css).map_err(|_| CdpError::invalid_params("invalid css selector"))?;
            found = dom.query_selector_all(root, &sel).map_err(|e| CdpError::server(format!("{e:?}")))?;
        } else if let Some(id) = &self.test_id {
            let name = self.test_id_attribute.as_deref().unwrap_or("data-testid");
            found = elements(&dom, root).filter(inside).filter(|n| attr(&dom, *n, name) == Some(id.as_str())).collect();
        } else if let Some(text) = &self.text {
            if root == doc {
                if let Ok(Some(html)) = dom.document_element(doc) {
                    by_text(&dom, html, text, self.exact, &mut found);
                }
            } else {
                for c in dom.children(root).into_iter().flatten() {
                    if dom.element(c).is_ok() && !skipped_for_text(&dom, c) {
                        by_text(&dom, c, text, self.exact, &mut found);
                    }
                }
            }
        } else if let Some(role) = &self.role {
            let role = role.to_ascii_lowercase();
            let layout = page.layout.borrow();
            found = elements(&dom, root)
                .filter(inside)
                .filter(|n| role_of(&dom, *n).as_deref() == Some(role.as_str()))
                .filter(|n| attr(&dom, *n, "aria-hidden") != Some("true") && layout.is_visible(&dom, *n))
                .filter(|n| match &self.name {
                    Some(want) => want.matches(&accessible_name(&dom, doc, *n, &role), self.exact),
                    None => true,
                })
                .collect();
        } else if let Some(want) = &self.label {
            for n in elements(&dom, root).filter(inside) {
                let control = matches!(local(&dom, n), Some("input" | "textarea" | "select" | "button"));
                let labelled = attr(&dom, n, "aria-label").is_some_and(|l| want.matches(l, self.exact))
                    || (control && label_text(&dom, doc, n).is_some_and(|l| want.matches(&l, self.exact)));
                if labelled {
                    found.push(n);
                }
            }
        } else if let Some(want) = &self.placeholder {
            found = elements(&dom, root)
                .filter(inside)
                .filter(|n| matches!(local(&dom, *n), Some("input" | "textarea")))
                .filter(|n| attr(&dom, *n, "placeholder").is_some_and(|p| want.matches(p, self.exact)))
                .collect();
        }
        Ok(found)
    }
}

impl Filter {
    fn keeps(&self, page: &PageDom, selectors: &RefCell<SelectorCache>, n: NodeId) -> CdpResult<bool> {
        if let Some(want) = self.visible {
            if visible_box(page, n).is_some() != want {
                return Ok(false);
            }
        }
        if self.has_text.is_some() || self.has_not_text.is_some() {
            let text = text_of(&page.dom.borrow(), n);
            if self.has_text.as_ref().is_some_and(|t| !t.matches(&text, false))
                || self.has_not_text.as_ref().is_some_and(|t| t.matches(&text, false))
            {
                return Ok(false);
            }
        }
        for (inner, want) in [(&self.has, true), (&self.has_not, false)] {
            if let Some(inner) = inner {
                if inner.resolve_at(page, selectors, n)?.is_empty() == want {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

/// The one element a strict locator names, or why there is none yet.
fn single(found: &[NodeId], dom: &Dom, locator: &Locator) -> Result<Option<NodeId>, CdpError> {
    match found {
        [] => Ok(None),
        [one] => Ok(Some(*one)),
        many => {
            let names: Vec<String> = many
                .iter()
                .take(5)
                .map(|n| {
                    let tag = local(dom, *n).unwrap_or("?");
                    let text = normalize(&text_of(dom, *n));
                    let text: String = text.chars().take(40).collect();
                    format!("<{tag}> {text:?}")
                })
                .collect();
            Err(CdpError::server(format!(
                "strict mode violation: {} matched {} elements: {}",
                locator.describe(),
                many.len(),
                names.join(", ")
            )))
        }
    }
}

fn fillable(dom: &Dom, id: NodeId) -> bool {
    match local(dom, id) {
        Some("textarea") => attr(dom, id, "readonly").is_none(),
        Some("input") => {
            let ty = attr(dom, id, "type").unwrap_or("").to_ascii_lowercase();
            matches!(ty.as_str(), "" | "text" | "email" | "password" | "search" | "tel" | "url" | "number")
                && attr(dom, id, "readonly").is_none()
        }
        _ => attr(dom, id, "contenteditable").is_some_and(|v| !v.eq_ignore_ascii_case("false")),
    }
}

fn disabled(dom: &Dom, id: NodeId) -> bool {
    dom.is_disabled(id).unwrap_or(false) || attr(dom, id, "aria-disabled") == Some("true")
}

fn contains(dom: &Dom, ancestor: NodeId, mut node: NodeId) -> bool {
    loop {
        if node == ancestor {
            return true;
        }
        match dom.parent(node).ok().flatten() {
            Some(p) => node = p,
            None => return false,
        }
    }
}

/// Visible box with area, in viewport coordinates.
fn visible_box(page: &PageDom, id: NodeId) -> Option<lp_dom::layout::Rect> {
    let dom = page.dom.borrow();
    page.layout.borrow().client_rect(&dom, id).filter(|r| r.width > 0.0 && r.height > 0.0)
}

fn act(page: &Rc<PageDom>, id: NodeId, action: Action, value: &str) -> Attempt {
    {
        let dom = page.dom.borrow();
        if dom.root_node(id, true).ok() != Some(page.document()) {
            return Attempt::Retry("element is detached".into());
        }
    }
    if visible_box(page, id).is_none() {
        return Attempt::Retry("element is not visible".into());
    }
    if disabled(&page.dom.borrow(), id) {
        return Attempt::Retry("element is disabled".into());
    }
    match action {
        Action::Check | Action::Uncheck => {
            let checked = page.dom.borrow().checked(id).unwrap_or(false);
            if checked == (action == Action::Check) {
                return Attempt::Done(json!({}));
            }
            click(page, id)
        }
        Action::Click => click(page, id),
        Action::Press => {
            let Some(keys) = key_chord(value) else {
                return Attempt::Retry(format!("unknown key {value:?}"));
            };
            if input::key_target(page) != Some(id) {
                input::focus_element(page, id);
            }
            for (kind, k) in keys {
                input::dispatch_key(page, kind, &k);
            }
            Attempt::Done(json!({}))
        }
        Action::Focus => {
            input::focus_element(page, id);
            Attempt::Done(json!({}))
        }
        Action::Fill => {
            let is_control = {
                let dom = page.dom.borrow();
                if !fillable(&dom, id) {
                    return Attempt::Retry("element is not editable".into());
                }
                matches!(local(&dom, id), Some("input" | "textarea"))
            };
            input::focus_element(page, id);
            if is_control {
                let _ = page.dom.borrow_mut().set_selection_range(id, 0, u32::MAX);
            } else {
                lp_webapi::editing::select_all(page, id);
            }
            // Empty text deletes the selection (Input.insertText semantics).
            input::insert_text(page, value);
            Attempt::Done(json!({}))
        }
    }
}

/// `key`, `code` and Windows key code of a key name (`Enter`, `ArrowDown`,
/// `F5`, `a`, ...).
fn key_def(name: &str) -> Option<(String, String, u32)> {
    let named = |code: &str, kc: u32| Some((name.to_owned(), code.to_owned(), kc));
    match name {
        "Enter" => named("Enter", 13),
        "Tab" => named("Tab", 9),
        "Escape" => named("Escape", 27),
        "Backspace" => named("Backspace", 8),
        "Delete" => named("Delete", 46),
        "Space" | " " => Some((" ".into(), "Space".into(), 32)),
        "ArrowLeft" => named("ArrowLeft", 37),
        "ArrowUp" => named("ArrowUp", 38),
        "ArrowRight" => named("ArrowRight", 39),
        "ArrowDown" => named("ArrowDown", 40),
        "Home" => named("Home", 36),
        "End" => named("End", 35),
        "PageUp" => named("PageUp", 33),
        "PageDown" => named("PageDown", 34),
        "Shift" => named("ShiftLeft", 16),
        "Control" => named("ControlLeft", 17),
        "Alt" => named("AltLeft", 18),
        "Meta" => named("MetaLeft", 91),
        _ => {
            if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()).filter(|n| (1..=12).contains(n))
            {
                return named(name, 111 + n);
            }
            let mut chars = name.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else { return None };
            let upper = c.to_ascii_uppercase();
            let (code, kc) = if c.is_ascii_alphabetic() {
                (format!("Key{upper}"), upper as u32)
            } else if c.is_ascii_digit() {
                (format!("Digit{c}"), c as u32)
            } else {
                (String::new(), 0)
            };
            Some((name.to_owned(), code, kc))
        }
    }
}

/// The key events of a press: modifiers down, the key down and up,
/// modifiers up. `ControlOrMeta` is Meta on macOS.
fn key_chord(chord: &str) -> Option<Vec<(input::KeyKind, input::KeyInput)>> {
    let parts: Vec<&str> = if chord == "+" { vec!["+"] } else { chord.split('+').collect() };
    let (key, mods) = parts.split_last()?;
    let mut modifiers = input::Modifiers::default();
    let mut held = Vec::new();
    for m in mods {
        let m = match *m {
            "ControlOrMeta" if cfg!(target_os = "macos") => "Meta",
            "ControlOrMeta" => "Control",
            m => m,
        };
        match m {
            "Shift" => modifiers.shift = true,
            "Control" => modifiers.ctrl = true,
            "Alt" => modifiers.alt = true,
            "Meta" => modifiers.meta = true,
            _ => return None,
        }
        let (k, code, key_code) = key_def(m)?;
        held.push(input::KeyInput { key: k, code, modifiers, key_code, ..Default::default() });
    }
    let (mut k, code, key_code) = key_def(key)?;
    if modifiers.shift && k.chars().count() == 1 {
        k = k.to_uppercase();
    }
    let text = match k.as_str() {
        _ if modifiers.ctrl || modifiers.meta || modifiers.alt => String::new(),
        "Enter" => "\r".into(),
        _ if k.chars().count() == 1 => k.clone(),
        _ => String::new(),
    };
    let main = input::KeyInput { key: k, code, text, modifiers, key_code, ..Default::default() };
    let mut events: Vec<_> = held.iter().map(|k| (input::KeyKind::RawKeyDown, k.clone())).collect();
    let down = if main.text.is_empty() { input::KeyKind::RawKeyDown } else { input::KeyKind::KeyDown };
    events.push((down, main.clone()));
    events.push((input::KeyKind::KeyUp, main));
    events.extend(held.into_iter().rev().map(|k| (input::KeyKind::KeyUp, k)));
    Some(events)
}

/// Scrolls the element into view, checks that its center receives pointer
/// events, then dispatches move, press and release there.
fn click(page: &Rc<PageDom>, id: NodeId) -> Attempt {
    super::dom::reveal(page, id);
    let Some(rect) = visible_box(page, id) else { return Attempt::Retry("element is not visible".into()) };
    let (x, y) = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
    match lp_webapi::layout_api::hit_test(page, x, y) {
        Some(hit) if contains(&page.dom.borrow(), id, hit) => {}
        Some(hit) => {
            let tag = local(&page.dom.borrow(), hit).unwrap_or("?").to_owned();
            return Attempt::Retry(format!("<{tag}> intercepts pointer events"));
        }
        None => return Attempt::Retry("element is outside the viewport".into()),
    }
    let mut m = MouseInput {
        x,
        y,
        button: MouseButton::None,
        buttons: 0,
        click_count: 0,
        modifiers: Default::default(),
        delta_x: 0.0,
        delta_y: 0.0,
    };
    let steps = [
        (MouseKind::Moved, MouseButton::None, 0, 0),
        (MouseKind::Pressed, MouseButton::Left, 1, 1),
        (MouseKind::Released, MouseButton::Left, 0, 1),
    ];
    for (kind, button, buttons, count) in steps {
        m.button = button;
        m.buttons = buttons;
        m.click_count = count;
        if let Err(e) = input::dispatch_mouse(page, kind, &m) {
            return Attempt::Retry(e);
        }
    }
    Attempt::Done(json!({}))
}

fn expect(
    page: &PageDom,
    found: &[NodeId],
    locator: &Locator,
    assert: Assert,
    expected: &Value,
    not: bool,
) -> CdpResult<Attempt> {
    let dom = page.dom.borrow();
    let (holds, actual) = match assert {
        Assert::Count => {
            let want = expected.as_u64().ok_or_else(|| CdpError::invalid_params("count needs a number"))?;
            (found.len() as u64 == want, json!(found.len()))
        }
        Assert::Visible | Assert::Hidden => {
            let el = if found.is_empty() { None } else { single(found, &dom, locator)? };
            drop(dom);
            let visible = el.is_some_and(|el| visible_box(page, el).is_some());
            (visible == (assert == Assert::Visible), json!(visible))
        }
        Assert::Text | Assert::ContainsText | Assert::Value | Assert::Checked => {
            let Some(el) = single(found, &dom, locator)? else {
                return Ok(Attempt::Retry(format!("{} matched no element", locator.describe())));
            };
            match assert {
                Assert::Checked => {
                    let checked = dom.checked(el).unwrap_or(false);
                    (checked == expected.as_bool().unwrap_or(true), json!(checked))
                }
                Assert::Value => {
                    let want = expected.as_str().ok_or_else(|| CdpError::invalid_params("value needs a string"))?;
                    let have = dom.value(el).unwrap_or_default();
                    (have == want, json!(have))
                }
                _ => {
                    let want = expected.as_str().ok_or_else(|| CdpError::invalid_params("text needs a string"))?;
                    let have = normalize(&text_of(&dom, el));
                    let ok =
                        if assert == Assert::Text { have == normalize(want) } else { have.contains(&normalize(want)) };
                    (ok, json!(have))
                }
            }
        }
    };
    Ok(if holds != not {
        Attempt::Done(json!({ "actual": actual }))
    } else {
        Attempt::Retry(format!("expected {}{:?} {expected}, got {actual}", if not { "not " } else { "" }, assert))
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GotoParams {
    url: String,
    /// `load` (default) or `domcontentloaded`.
    wait_until: Option<String>,
    timeout: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ActParams {
    locator: Locator,
    action: String,
    #[serde(default)]
    value: String,
    /// Wait for the requests the action started (default: true in `LP.run`).
    settle: Option<bool>,
    timeout: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExpectParams {
    locator: Locator,
    assert: String,
    #[serde(default)]
    expected: Value,
    #[serde(default)]
    not: bool,
    timeout: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QueryParams {
    locator: Locator,
    read: String,
    /// Key of the value in the run's `reads`.
    #[serde(rename = "as")]
    key: Option<String>,
    timeout: Option<u64>,
}

/// One `LP.run` step: `{"goto": {...}}`, `{"act": {...}}`, `{"check": {...}}`
/// `{"read": {...}}`, `{"route": {...}}`, `{"eval": {...}}` or
/// `{"request": {...}}`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
enum StepParams {
    Goto(GotoParams),
    Loaded(LoadedParams),
    Route(RouteParams),
    Act(ActParams),
    Check(ExpectParams),
    Read(QueryParams),
    Eval(EvalParams),
    Request(RequestParams),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LoadedParams {
    timeout: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RouteParams {
    url: String,
    timeout: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EvalParams {
    expression: String,
    #[serde(rename = "as")]
    key: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestParams {
    #[serde(flatten)]
    matcher: RequestMatch,
    timeout: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunParams {
    /// Parsed one at a time, so a malformed step is reported by its index.
    steps: Vec<Box<serde_json::value::RawValue>>,
    /// Per-step timeout unless a step sets its own.
    timeout: Option<u64>,
}

fn timeout_of(step: Option<u64>, default: Option<u64>) -> Duration {
    Duration::from_millis(step.or(default).unwrap_or(DEFAULT_TIMEOUT_MS).min(MAX_TIMEOUT_MS))
}

impl ActParams {
    fn step(self, settle_default: bool) -> CdpResult<(Step, Option<u64>)> {
        self.locator.validate()?;
        let action = match self.action.as_str() {
            "click" => Action::Click,
            "fill" => Action::Fill,
            "focus" => Action::Focus,
            "check" => Action::Check,
            "uncheck" => Action::Uncheck,
            "press" => Action::Press,
            _ => return Err(CdpError::invalid_params("action must be click, fill, focus, check, uncheck or press")),
        };
        let settle = self.settle.unwrap_or(settle_default);
        Ok((Step::Act { locator: self.locator, action, value: self.value, settle }, self.timeout))
    }
}

impl ExpectParams {
    fn step(self) -> CdpResult<(Step, Option<u64>)> {
        self.locator.validate()?;
        let assert = match self.assert.as_str() {
            "visible" => Assert::Visible,
            "hidden" => Assert::Hidden,
            "text" => Assert::Text,
            "containsText" => Assert::ContainsText,
            "value" => Assert::Value,
            "count" => Assert::Count,
            "checked" => Assert::Checked,
            _ => {
                return Err(CdpError::invalid_params(
                    "assert must be visible, hidden, text, containsText, value, count or checked",
                ))
            }
        };
        Ok((Step::Check { locator: self.locator, assert, expected: self.expected, not: self.not }, self.timeout))
    }
}

impl QueryParams {
    fn read(&self) -> CdpResult<Read> {
        self.locator.validate()?;
        Ok(match self.read.as_str() {
            "count" => Read::Count,
            "text" => Read::Text,
            "value" => Read::Value,
            "visible" => Read::Visible,
            "box" => Read::Box,
            _ => return Err(CdpError::invalid_params("read must be count, text, value, visible or box")),
        })
    }
}

impl StepParams {
    fn step(self) -> CdpResult<(Step, Option<u64>)> {
        match self {
            StepParams::Goto(p) => {
                let dom_ready = match p.wait_until.as_deref() {
                    None | Some("load") => false,
                    Some("domcontentloaded") => true,
                    _ => return Err(CdpError::invalid_params("waitUntil must be load or domcontentloaded")),
                };
                Ok((Step::Goto { url: p.url, dom_ready }, p.timeout))
            }
            StepParams::Loaded(p) => Ok((Step::Loaded, p.timeout)),
            StepParams::Route(p) => Ok((Step::Route { url: p.url }, p.timeout)),
            StepParams::Eval(p) => Ok((Step::Eval { expression: p.expression, key: p.key }, None)),
            StepParams::Request(p) => Ok((Step::Request(p.matcher), p.timeout)),
            StepParams::Act(p) => p.step(true),
            StepParams::Check(p) => p.step(),
            StepParams::Read(p) => {
                let read = p.read()?;
                Ok((Step::Read { locator: p.locator, read, key: p.key }, p.timeout))
            }
        }
    }
}

fn read_value(page: &PageDom, el: NodeId, read: Read) -> Value {
    match read {
        Read::Count => Value::Null,
        Read::Text => json!(normalize(&text_of(&page.dom.borrow(), el))),
        Read::Value => json!(page.dom.borrow().value(el).unwrap_or_default()),
        Read::Visible => json!(visible_box(page, el).is_some()),
        Read::Box => match visible_box(page, el) {
            Some(r) => json!({"x": r.x, "y": r.y, "width": r.width, "height": r.height}),
            None => Value::Null,
        },
    }
}

/// How driving a run ended for now.
enum Drive {
    Done(Value),
    Park(Box<Waiter>),
}

impl EngineHost {
    fn main_document(&self, page: PageKey) -> CdpResult<Rc<PageDom>> {
        let id = self.pages.get(&page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        self.thread
            .browser
            .page(id)
            .and_then(|p| p.main_frame.page_dom.clone())
            .ok_or_else(|| CdpError::server("page has no document"))
    }

    /// One try of a locator step.
    fn attempt(&mut self, page: &Rc<PageDom>, step: &Step) -> CdpResult<Attempt> {
        let no_match = |locator: &Locator| Attempt::Retry(format!("{} matched no element", locator.describe()));
        match step {
            Step::Goto { .. } | Step::Loaded | Step::Route { .. } | Step::Eval { .. } | Step::Request(_) => {
                unreachable!("not a locator step")
            }
            Step::Act { locator, action, value, .. } => {
                let found = locator.resolve(page, &self.waiters.selectors)?;
                let Some(el) = single(&found, &page.dom.borrow(), locator)? else { return Ok(no_match(locator)) };
                let result = act(page, el, *action, value);
                if matches!(result, Attempt::Done(_)) {
                    self.thread.browser.checkpoint();
                }
                Ok(result)
            }
            Step::Check { locator, assert, expected, not } => {
                let found = locator.resolve(page, &self.waiters.selectors)?;
                expect(page, &found, locator, *assert, expected, *not)
            }
            Step::Read { locator, read, .. } => {
                let found = locator.resolve(page, &self.waiters.selectors)?;
                if *read == Read::Count {
                    return Ok(Attempt::Done(json!(found.len())));
                }
                let Some(el) = single(&found, &page.dom.borrow(), locator)? else { return Ok(no_match(locator)) };
                Ok(Attempt::Done(read_value(page, el, *read)))
            }
        }
    }

    pub(super) fn driver_domain(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        let (steps, single) = match action {
            "act" => {
                let (step, timeout) = cmd.command.params::<ActParams>()?.step(false)?;
                (vec![(step, timeout_of(timeout, None))], true)
            }
            "expect" => {
                let (step, timeout) = cmd.command.params::<ExpectParams>()?.step()?;
                (vec![(step, timeout_of(timeout, None))], true)
            }
            "run" => {
                let p: RunParams = cmd.command.params()?;
                if p.steps.len() > MAX_STEPS {
                    return Err(CdpError::invalid_params("too many steps"));
                }
                let mut steps = Vec::with_capacity(p.steps.len());
                for (i, raw) in p.steps.iter().enumerate() {
                    let (step, timeout) = serde_json::from_str::<StepParams>(raw.get())
                        .map_err(|e| CdpError::invalid_params(e.to_string()))
                        .and_then(StepParams::step)
                        .map_err(|e| CdpError::invalid_params(format!("step {i}: {}", e.message)))?;
                    steps.push((step, timeout_of(timeout, p.timeout)));
                }
                (steps, false)
            }
            "query" => return self.driver_query(cmd).map(Some),
            _ => return Err(CdpError::unsupported(&cmd.command.method)),
        };
        let state = self.pages.get_mut(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
        let pid = state.id;
        // A run records the page's requests from here on (`request` steps, reply).
        let log_from = if single {
            None
        } else if let Some(log) = &state.network.log {
            Some(log.seq())
        } else {
            state.network.log = Some(Default::default());
            self.update_network_subscriptions();
            Some(0)
        };
        let now = Instant::now();
        let first = steps.first().map_or(Duration::ZERO, |(_, t)| *t);
        let job = Job {
            id: cmd.command.id,
            session: cmd.session_id.to_owned(),
            page: cmd.page,
            pid,
            steps,
            index: 0,
            results: Vec::new(),
            reads: Default::default(),
            single,
            started: now,
            log_from,
        };
        let waiter = Waiter {
            job,
            phase: Phase::Try,
            step_deadline: now + first,
            doc: Weak::new(),
            seen: 0,
            reason: String::new(),
        };
        match self.drive(waiter)? {
            Drive::Done(value) => Ok(Some(value)),
            Drive::Park(w) => {
                if self.waiters.list.len() >= MAX_WAITERS {
                    return Err(CdpError::server("too many waiting LP commands"));
                }
                self.waiters.list.push(*w);
                // Replied by `service_waiters`.
                Ok(None)
            }
        }
    }

    /// Runs a job's steps until one has to wait or the last one is done.
    fn drive(&mut self, mut w: Waiter) -> CdpResult<Drive> {
        loop {
            let now = Instant::now();
            let pid = w.job.pid;
            match std::mem::replace(&mut w.phase, Phase::Try) {
                Phase::Settle { loader, baseline, mut marker, since, until } => {
                    let gave_up = now >= until;
                    if !gave_up {
                        if inflight(&self.thread.browser, pid, &loader) > baseline {
                            // A request is (again) in flight: the marker restarts after it.
                            w.phase = Phase::Settle { loader, baseline, marker: None, since, until };
                            return Ok(Drive::Park(Box::new(w)));
                        }
                        let done = marker.get_or_insert_with(|| {
                            let done = Rc::new(Cell::new(false));
                            post_marker(self.thread.browser.scheduler(), done.clone(), MARKER_HOPS);
                            done
                        });
                        if !done.get() {
                            w.phase = Phase::Settle { loader, baseline, marker, since, until };
                            return Ok(Drive::Park(Box::new(w)));
                        }
                    }
                    if let Some(Value::Object(r)) = w.job.results.last_mut() {
                        r.insert("settleMs".into(), json!(since.elapsed().as_secs_f64() * 1e3));
                        if gave_up {
                            r.insert("settled".into(), json!(false));
                        }
                    }
                    self.next_step(&mut w);
                }
                Phase::Load { loader, dom_ready } => {
                    if !load_reached(&self.thread.browser, pid, &loader, dom_ready) {
                        if now >= w.step_deadline {
                            let error = CdpError::server(format!(
                                "timed out after {} ms waiting for {}",
                                w.job.steps[w.job.index].1.as_millis(),
                                if dom_ready { "DOMContentLoaded" } else { "load" }
                            ));
                            return Err(w.job.fail(error));
                        }
                        w.phase = Phase::Load { loader, dom_ready };
                        return Ok(Drive::Park(Box::new(w)));
                    }
                    if self.thread.browser.page(pid).is_none() {
                        return Err(w.job.fail(CdpError::server("page is gone")));
                    }
                    w.job.results.push(json!({}));
                    self.next_step(&mut w);
                }
                Phase::Try => {
                    let Some((step, timeout)) = w.job.steps.get(w.job.index) else {
                        let requests = self.logged_requests(&w.job);
                        return Ok(Drive::Done(w.job.reply(requests)));
                    };
                    let (step, timeout) = (step.clone(), *timeout);
                    if let Step::Goto { url, dom_ready } = &step {
                        let loader = self
                            .thread
                            .browser
                            .navigate(pid, NavigateOpts::address_bar(url.as_str()))
                            .map_err(|e| w.job.fail(CdpError::server(format!("navigation failed: {e}"))))?;
                        self.thread.browser.checkpoint();
                        w.phase = Phase::Load { loader: loader.0, dom_ready: *dom_ready };
                        continue;
                    }
                    if let Step::Loaded = step {
                        let (loader, _) = net_state(&self.thread.browser, pid);
                        w.phase = Phase::Load { loader, dom_ready: false };
                        continue;
                    }
                    let before = matches!(step, Step::Act { settle: true, .. } | Step::Route { .. })
                        .then(|| net_state(&self.thread.browser, pid));
                    let (attempt, page) = match &step {
                        Step::Request(m) => (self.match_request(&w.job, m), None),
                        Step::Eval { expression, .. } => {
                            let value = self.eval_step(pid, expression).map_err(|e| w.job.fail(e))?;
                            (Attempt::Done(value), None)
                        }
                        Step::Route { url } => {
                            let url = serde_json::to_string(url).unwrap_or_default();
                            let src = format!(
                                "history.pushState(null, '', {url}); \
                                 dispatchEvent(new PopStateEvent('popstate', {{ state: history.state }})); null"
                            );
                            self.eval_step(pid, &src).map_err(|e| w.job.fail(e))?;
                            (Attempt::Done(json!({})), None)
                        }
                        _ => {
                            let page = self.main_document(w.job.page).map_err(|e| w.job.fail(e))?;
                            (self.attempt(&page, &step).map_err(|e| w.job.fail(e))?, Some(page))
                        }
                    };
                    match attempt {
                        Attempt::Done(value) => {
                            if let Step::Read { key: Some(key), .. } | Step::Eval { key: Some(key), .. } = &step {
                                w.job.reads.insert(key.clone(), value.clone());
                            }
                            w.job.results.push(value);
                            match before {
                                Some((loader, baseline)) => {
                                    let since = Instant::now();
                                    w.phase = Phase::Settle {
                                        loader,
                                        baseline,
                                        marker: None,
                                        since,
                                        until: since + SETTLE_MAX,
                                    };
                                }
                                None => self.next_step(&mut w),
                            }
                        }
                        Attempt::Retry(reason) => {
                            if now >= w.step_deadline {
                                let error = CdpError::server(format!(
                                    "timed out after {} ms: {}: {reason}",
                                    timeout.as_millis(),
                                    step.describe()
                                ));
                                return Err(w.job.fail(error));
                            }
                            if let Some(page) = page {
                                w.seen = page.dom.borrow().version();
                                w.doc = Rc::downgrade(&page);
                            }
                            w.reason = reason;
                            return Ok(Drive::Park(Box::new(w)));
                        }
                    }
                }
            }
        }
    }

    /// Evaluates `src` in the page's main world; its JSON value.
    fn eval_step(&mut self, pid: PageId, src: &str) -> CdpResult<Value> {
        let realm = self
            .thread
            .browser
            .page(pid)
            .and_then(|p| p.main_frame.main_realm().cloned())
            .ok_or_else(|| CdpError::server("page has no realm"))?;
        let value = {
            let scope = realm.scope().map_err(super::remote::js_to_cdp)?;
            match scope.eval(src, Some("lp-run"), 1) {
                Ok(v) => v.to_json(0).ok().flatten().and_then(|j| serde_json::from_str(&j).ok()).unwrap_or(Value::Null),
                Err(e) => return Err(CdpError::server(format!("threw: {}", e.details(&scope).message))),
            }
        };
        self.thread.browser.checkpoint();
        Ok(value)
    }

    /// A `request` step against the requests the run saw so far.
    fn match_request(&self, job: &Job, m: &RequestMatch) -> Attempt {
        let log = self.pages.get(&job.page).and_then(|p| p.network.log.as_ref());
        let (Some(log), Some(from)) = (log, job.log_from) else {
            return Attempt::Retry("the page has no request log".into());
        };
        let matching: Vec<_> = log
            .since(from)
            .iter()
            .filter(|r| m.method.as_ref().is_none_or(|x| r.method.eq_ignore_ascii_case(x)))
            .filter(|r| m.path.as_ref().is_none_or(|p| r.path() == p))
            .filter(|r| m.url.as_ref().is_none_or(|u| u.matches(&r.url, false)))
            .collect();
        let finished: Vec<_> = matching.iter().filter(|r| r.done).collect();
        if let Some(n) = m.count {
            return if matching.len() == n && finished.len() == n {
                Attempt::Done(json!({ "count": n }))
            } else {
                Attempt::Retry(format!("{} matching requests ({} finished), want {n}", matching.len(), finished.len()))
            };
        }
        match finished.iter().rev().find(|r| m.status.is_none_or(|s| r.status == Some(s))) {
            Some(r) => Attempt::Done(r.to_json()),
            None => Attempt::Retry(match finished.last() {
                Some(r) => format!("last match was {}", r.to_json()),
                None if matching.is_empty() => "no matching request yet".into(),
                None => "the matching request is still in flight".into(),
            }),
        }
    }

    /// The requests a run saw, for its reply.
    fn logged_requests(&self, job: &Job) -> Vec<Value> {
        let log = self.pages.get(&job.page).and_then(|p| p.network.log.as_ref());
        match (log, job.log_from) {
            (Some(log), Some(from)) => log.since(from).iter().map(|r| r.to_json()).collect(),
            _ => Vec::new(),
        }
    }

    fn next_step(&mut self, w: &mut Waiter) {
        w.job.index += 1;
        w.phase = Phase::Try;
        if let Some((_, timeout)) = w.job.steps.get(w.job.index) {
            w.step_deadline = Instant::now() + *timeout;
        }
    }

    fn driver_query(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<Value> {
        let p: QueryParams = cmd.command.params()?;
        let read = p.read()?;
        let page = self.main_document(cmd.page)?;
        let found = p.locator.resolve(&page, &self.waiters.selectors)?;
        if read == Read::Count {
            return Ok(json!({ "value": found.len() }));
        }
        let el = single(&found, &page.dom.borrow(), &p.locator)?
            .ok_or_else(|| CdpError::server(format!("{} matched no element", p.locator.describe())))?;
        Ok(json!({ "value": read_value(&page, el, read) }))
    }

    /// Drives the waiters that can make progress and replies to those that
    /// finished or failed. Replies are queued behind the events the steps
    /// produced.
    pub(super) fn service_waiters(&mut self) {
        if self.waiters.list.is_empty() {
            self.waiters.log_changed = false;
            return;
        }
        let now = Instant::now();
        let sessions = &self.sessions;
        self.waiters.list.retain(|w| sessions.contains_key(&w.job.session));
        let browser = &self.thread.browser;
        let log_changed = std::mem::take(&mut self.waiters.log_changed);
        let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.waiters.list)
            .into_iter()
            .partition(|w| w.ready(browser, now) || (log_changed && w.waits_on_log()));
        self.waiters.list = waiting;
        for w in ready {
            let (id, session) = (w.job.id, w.job.session.clone());
            let frame = match self.drive(w) {
                Ok(Drive::Park(w)) => {
                    self.waiters.list.push(*w);
                    continue;
                }
                Ok(Drive::Done(value)) => msg::result_frame(id, &value, Some(&session)),
                Err(error) => msg::error_frame(Some(id), &error, Some(&session)),
            };
            self.collector.push(Queued::Frame(frame));
        }
    }
}
