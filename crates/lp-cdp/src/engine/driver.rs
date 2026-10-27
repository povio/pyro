//! `LP.*`: a native test driver.
//!
//! `LP.run` takes a whole scenario, a list of steps (`goto`, `act`, `check`,
//! `read`), and replies once, when the last step holds or the first one fails.
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
    text: Option<String>,
    role: Option<String>,
    /// Accessible name for `role`.
    name: Option<String>,
    label: Option<String>,
    placeholder: Option<String>,
    /// Whole-string, case-sensitive match for text, name, label and
    /// placeholder (after whitespace normalization); otherwise a
    /// case-insensitive substring.
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
    has_text: Option<String>,
    has_not_text: Option<String>,
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

#[derive(Clone, Debug)]
enum Step {
    Goto { url: String, dom_ready: bool },
    Act { locator: Locator, action: Action, value: String, settle: bool },
    Check { locator: Locator, assert: Assert, expected: Value, not: bool },
    Read { locator: Locator, read: Read, key: Option<String> },
}

impl Step {
    fn describe(&self) -> String {
        match self {
            Step::Goto { url, .. } => format!("goto {url}"),
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
}

impl Job {
    fn reply(mut self) -> Value {
        if self.single {
            return self.results.pop().unwrap_or_else(|| json!({}));
        }
        json!({
            "results": self.results,
            "reads": self.reads,
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

    /// Whether driving it can make progress.
    fn ready(&self, browser: &Browser, now: Instant) -> bool {
        if now >= self.deadline() {
            return true;
        }
        match &self.phase {
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
fn by_text(dom: &Dom, root: NodeId, want: &str, exact: bool, out: &mut Vec<NodeId>) -> bool {
    let children: Vec<NodeId> = dom
        .children(root)
        .into_iter()
        .flatten()
        .filter(|c| dom.element(*c).is_ok() && !skipped_for_text(dom, *c))
        .collect();
    let here = text_matches(&text_of(dom, root), want, exact);
    // A substring match in a child implies one here: prune subtrees that
    // cannot match. Exact matches need the whole walk.
    if !here && !exact {
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
                    Some(want) => text_matches(&accessible_name(&dom, doc, *n, &role), want, self.exact),
                    None => true,
                })
                .collect();
        } else if let Some(want) = &self.label {
            for n in elements(&dom, root).filter(inside) {
                let control = matches!(local(&dom, n), Some("input" | "textarea" | "select" | "button"));
                let labelled = attr(&dom, n, "aria-label").is_some_and(|l| text_matches(l, want, self.exact))
                    || (control && label_text(&dom, doc, n).is_some_and(|l| text_matches(&l, want, self.exact)));
                if labelled {
                    found.push(n);
                }
            }
        } else if let Some(want) = &self.placeholder {
            found = elements(&dom, root)
                .filter(inside)
                .filter(|n| matches!(local(&dom, *n), Some("input" | "textarea")))
                .filter(|n| attr(&dom, *n, "placeholder").is_some_and(|p| text_matches(p, want, self.exact)))
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
            if self.has_text.as_ref().is_some_and(|t| !text_matches(&text, t, false))
                || self.has_not_text.as_ref().is_some_and(|t| text_matches(&text, t, false))
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
/// or `{"read": {...}}`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
enum StepParams {
    Goto(GotoParams),
    Act(ActParams),
    Check(ExpectParams),
    Read(QueryParams),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunParams {
    steps: Vec<StepParams>,
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
            _ => return Err(CdpError::invalid_params("action must be click, fill, focus, check or uncheck")),
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
            Step::Goto { .. } => unreachable!("goto is not a locator step"),
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
                for (i, s) in p.steps.into_iter().enumerate() {
                    let (step, timeout) =
                        s.step().map_err(|e| CdpError::invalid_params(format!("step {i}: {}", e.message)))?;
                    steps.push((step, timeout_of(timeout, p.timeout)));
                }
                (steps, false)
            }
            "query" => return self.driver_query(cmd).map(Some),
            _ => return Err(CdpError::unsupported(&cmd.command.method)),
        };
        let pid = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
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
                        return Ok(Drive::Done(w.job.reply()));
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
                    let page = self.main_document(w.job.page).map_err(|e| w.job.fail(e))?;
                    let before =
                        matches!(step, Step::Act { settle: true, .. }).then(|| net_state(&self.thread.browser, pid));
                    match self.attempt(&page, &step).map_err(|e| w.job.fail(e))? {
                        Attempt::Done(value) => {
                            if let Step::Read { key: Some(key), .. } = &step {
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
                            w.seen = page.dom.borrow().version();
                            w.doc = Rc::downgrade(&page);
                            w.reason = reason;
                            return Ok(Drive::Park(Box::new(w)));
                        }
                    }
                }
            }
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
            return;
        }
        let now = Instant::now();
        let sessions = &self.sessions;
        self.waiters.list.retain(|w| sessions.contains_key(&w.job.session));
        let browser = &self.thread.browser;
        let (ready, waiting): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.waiters.list).into_iter().partition(|w| w.ready(browser, now));
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
