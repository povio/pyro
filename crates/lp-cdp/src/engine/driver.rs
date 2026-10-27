//! `LP.*`: a native test driver, one command per user action.
//!
//! A test client sends `LP.act` (click, fill, ...) or `LP.expect` (visible,
//! text, ...) with a locator. The locator is resolved against the arena DOM in
//! Rust and the actionability checks (unique, visible, enabled, editable, not
//! covered) run here, so an action costs one round trip and no JavaScript.
//!
//! A command whose condition does not hold yet becomes a [`Waiter`]: it is
//! tried again only when the page's DOM version moves (or its document is
//! replaced) and fails at its deadline. Nothing polls: `EngineHost::run_until_work`
//! returns to the serve loop after a task that changed a waited-on document,
//! and `EngineHost::flush` retries the waiters before sending queued events.
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use lp_dom::select::SelectorCache;
use lp_dom::{Dom, NodeId, NodeKind};
use lp_webapi::input::{self, MouseButton, MouseInput, MouseKind};
use lp_webapi::PageDom;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{EngineHost, Queued};
use crate::msg::{self, CdpError, CdpResult};
use crate::target::{PageCommand, PageKey};

const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
/// Waiting commands per connection (each holds only its parsed request).
const MAX_WAITERS: usize = 1024;

/// What a locator names. Exactly one of `css`, `testId`, `text`, `role` or
/// `label`; `nth` picks one match instead of requiring a unique one.
#[derive(Deserialize, Clone, Debug, Default)]
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
    /// Whole-string, case-sensitive match for text, name and label (after
    /// whitespace normalization); otherwise a case-insensitive substring.
    #[serde(default)]
    exact: bool,
    /// Index into the matches (negative counts from the end).
    nth: Option<i64>,
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

#[derive(Clone, Debug)]
enum Request {
    Act { locator: Locator, action: Action, value: String },
    Expect { locator: Locator, assert: Assert, expected: Value, not: bool },
}

/// One try of a request.
enum Attempt {
    Done(Value),
    /// Not yet: the reason is reported if the deadline passes.
    Retry(String),
}

pub(super) struct Waiter {
    id: u64,
    session: String,
    page: PageKey,
    request: Request,
    deadline: Instant,
    timeout: Duration,
    /// Document and DOM version the last try saw.
    doc: Weak<PageDom>,
    seen: u64,
    reason: String,
}

#[derive(Default)]
pub(crate) struct Waiters {
    list: Vec<Waiter>,
    selectors: RefCell<SelectorCache>,
}

impl Waiters {
    /// Whether a waiter can make progress: its document changed or was
    /// replaced, or its deadline passed. Called between loop tasks.
    pub(super) fn ready(&self) -> bool {
        if self.list.is_empty() {
            return false;
        }
        let now = Instant::now();
        self.list.iter().any(|w| {
            now >= w.deadline
                || match w.doc.upgrade() {
                    Some(doc) => doc.is_disposed() || doc.dom.borrow().version() != w.seen,
                    None => true,
                }
        })
    }

    pub(super) fn next_deadline(&self) -> Option<Instant> {
        self.list.iter().map(|w| w.deadline).min()
    }

    pub(super) fn clear(&mut self) {
        self.list.clear();
    }
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

fn local<'d>(dom: &'d Dom, id: NodeId) -> Option<&'d str> {
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

impl Locator {
    fn describe(&self) -> String {
        serde_json::to_string(&json!({
            "css": self.css, "testId": self.test_id, "text": self.text, "role": self.role,
            "name": self.name, "label": self.label, "nth": self.nth,
        }))
        .unwrap_or_default()
        .replace(",\"name\":null", "")
    }

    fn validate(&self) -> CdpResult<()> {
        let kinds = [
            self.css.is_some(),
            self.test_id.is_some(),
            self.text.is_some(),
            self.role.is_some(),
            self.label.is_some(),
        ];
        if kinds.iter().filter(|k| **k).count() != 1 {
            return Err(CdpError::invalid_params("locator needs exactly one of css, testId, text, role, label"));
        }
        Ok(())
    }

    fn resolve(&self, page: &PageDom, selectors: &RefCell<SelectorCache>) -> CdpResult<Vec<NodeId>> {
        let dom = page.dom.borrow();
        let doc = page.document();
        let mut found = Vec::new();
        if let Some(css) = &self.css {
            let sel = selectors.borrow_mut().get(css).map_err(|_| CdpError::invalid_params("invalid css selector"))?;
            found = dom.query_selector_all(doc, &sel).map_err(|e| CdpError::server(format!("{e:?}")))?;
        } else if let Some(id) = &self.test_id {
            let name = self.test_id_attribute.as_deref().unwrap_or("data-testid");
            found = elements(&dom, doc).filter(|n| attr(&dom, *n, name) == Some(id.as_str())).collect();
        } else if let Some(text) = &self.text {
            if let Ok(Some(body)) = dom.document_element(doc) {
                by_text(&dom, body, text, self.exact, &mut found);
            }
        } else if let Some(role) = &self.role {
            let role = role.to_ascii_lowercase();
            let layout = page.layout.borrow();
            found = elements(&dom, doc)
                .filter(|n| role_of(&dom, *n).as_deref() == Some(role.as_str()))
                .filter(|n| attr(&dom, *n, "aria-hidden") != Some("true") && layout.is_visible(&dom, *n))
                .filter(|n| match &self.name {
                    Some(want) => text_matches(&accessible_name(&dom, doc, *n, &role), want, self.exact),
                    None => true,
                })
                .collect();
        } else if let Some(want) = &self.label {
            for n in elements(&dom, doc) {
                let control = matches!(local(&dom, n), Some("input" | "textarea" | "select" | "button"));
                let labelled = attr(&dom, n, "aria-label").is_some_and(|l| text_matches(l, want, self.exact))
                    || (control && label_text(&dom, doc, n).is_some_and(|l| text_matches(&l, want, self.exact)));
                if labelled {
                    found.push(n);
                }
            }
        }
        if let Some(nth) = self.nth {
            let len = found.len() as i64;
            let i = if nth < 0 { len + nth } else { nth };
            return Ok(if (0..len).contains(&i) { vec![found[i as usize]] } else { Vec::new() });
        }
        Ok(found)
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
#[serde(rename_all = "camelCase")]
struct ActParams {
    locator: Locator,
    action: String,
    #[serde(default)]
    value: String,
    timeout: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
struct QueryParams {
    locator: Locator,
    read: String,
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

    fn attempt(&mut self, page: &Rc<PageDom>, request: &Request) -> CdpResult<Attempt> {
        match request {
            Request::Act { locator, action, value } => {
                let found = locator.resolve(page, &self.waiters.selectors)?;
                let el = single(&found, &page.dom.borrow(), locator)?;
                let Some(el) = el else {
                    return Ok(Attempt::Retry(format!("{} matched no element", locator.describe())));
                };
                let result = act(page, el, *action, value);
                if matches!(result, Attempt::Done(_)) {
                    self.thread.browser.checkpoint();
                }
                Ok(result)
            }
            Request::Expect { locator, assert, expected, not } => {
                let found = locator.resolve(page, &self.waiters.selectors)?;
                expect(page, &found, locator, *assert, expected, *not)
            }
        }
    }

    pub(super) fn driver_domain(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        let (request, timeout) = match action {
            "act" => {
                let p: ActParams = cmd.command.params()?;
                p.locator.validate()?;
                let action = match p.action.as_str() {
                    "click" => Action::Click,
                    "fill" => Action::Fill,
                    "focus" => Action::Focus,
                    "check" => Action::Check,
                    "uncheck" => Action::Uncheck,
                    _ => return Err(CdpError::invalid_params("action must be click, fill, focus, check or uncheck")),
                };
                (Request::Act { locator: p.locator, action, value: p.value }, p.timeout)
            }
            "expect" => {
                let p: ExpectParams = cmd.command.params()?;
                p.locator.validate()?;
                let assert = match p.assert.as_str() {
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
                (Request::Expect { locator: p.locator, assert, expected: p.expected, not: p.not }, p.timeout)
            }
            "query" => return self.driver_query(cmd).map(Some),
            _ => return Err(CdpError::unsupported(&cmd.command.method)),
        };
        let timeout = Duration::from_millis(timeout.unwrap_or(DEFAULT_TIMEOUT_MS).min(MAX_TIMEOUT_MS));
        let page = self.main_document(cmd.page)?;
        let reason = match self.attempt(&page, &request)? {
            Attempt::Done(value) => return Ok(Some(value)),
            Attempt::Retry(reason) => reason,
        };
        if timeout.is_zero() {
            return Err(CdpError::server(format!("{}: {reason}", cmd.command.method)));
        }
        if self.waiters.list.len() >= MAX_WAITERS {
            return Err(CdpError::server("too many waiting LP commands"));
        }
        let seen = page.dom.borrow().version();
        self.waiters.list.push(Waiter {
            id: cmd.command.id,
            session: cmd.session_id.to_owned(),
            page: cmd.page,
            request,
            deadline: Instant::now() + timeout,
            timeout,
            doc: Rc::downgrade(&page),
            seen,
            reason,
        });
        // Replied by `service_waiters`.
        Ok(None)
    }

    fn driver_query(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<Value> {
        let p: QueryParams = cmd.command.params()?;
        p.locator.validate()?;
        let page = self.main_document(cmd.page)?;
        let found = p.locator.resolve(&page, &self.waiters.selectors)?;
        if p.read == "count" {
            return Ok(json!({ "value": found.len() }));
        }
        let el = single(&found, &page.dom.borrow(), &p.locator)?
            .ok_or_else(|| CdpError::server(format!("{} matched no element", p.locator.describe())))?;
        let value = match p.read.as_str() {
            "text" => json!(normalize(&text_of(&page.dom.borrow(), el))),
            "value" => json!(page.dom.borrow().value(el).unwrap_or_default()),
            "visible" => json!(visible_box(&page, el).is_some()),
            "box" => match visible_box(&page, el) {
                Some(r) => json!({"x": r.x, "y": r.y, "width": r.width, "height": r.height}),
                None => Value::Null,
            },
            _ => return Err(CdpError::invalid_params("read must be count, text, value, visible or box")),
        };
        Ok(json!({ "value": value }))
    }

    /// Retries the waiters whose document changed, replies to those that now
    /// hold or whose deadline passed. Replies are queued behind the events
    /// the retry produced.
    pub(super) fn service_waiters(&mut self) {
        if self.waiters.list.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut i = 0;
        while i < self.waiters.list.len() {
            let w = &self.waiters.list[i];
            if !self.sessions.contains_key(&w.session) {
                self.waiters.list.swap_remove(i);
                continue;
            }
            let expired = now >= w.deadline;
            let changed = match w.doc.upgrade() {
                Some(doc) => doc.is_disposed() || doc.dom.borrow().version() != w.seen,
                None => true,
            };
            if !changed && !expired {
                i += 1;
                continue;
            }
            let w = self.waiters.list.swap_remove(i);
            let outcome = self.main_document(w.page).and_then(|page| {
                let attempt = if changed { self.attempt(&page, &w.request)? } else { Attempt::Retry(w.reason.clone()) };
                Ok((page, attempt))
            });
            let frame = match outcome {
                Ok((_, Attempt::Done(value))) => msg::result_frame(w.id, &value, Some(&w.session)),
                Ok((page, Attempt::Retry(reason))) if !expired => {
                    let seen = page.dom.borrow().version();
                    let id = self.waiters.list.len();
                    self.waiters.list.push(Waiter { doc: Rc::downgrade(&page), seen, reason, ..w });
                    // Keep scanning from the same index: the moved-in entry
                    // was not visited yet unless it is the one just pushed.
                    if i == id {
                        i += 1;
                    }
                    continue;
                }
                Ok((_, Attempt::Retry(reason))) => {
                    let what = match &w.request {
                        Request::Act { locator, action, .. } => format!("{action:?} {}", locator.describe()),
                        Request::Expect { locator, assert, .. } => format!("expect {assert:?} {}", locator.describe()),
                    };
                    let error =
                        CdpError::server(format!("timed out after {} ms: {what}: {reason}", w.timeout.as_millis()));
                    msg::error_frame(Some(w.id), &error, Some(&w.session))
                }
                Err(error) => msg::error_frame(Some(w.id), &error, Some(&w.session)),
            };
            self.collector.push(Queued::Frame(frame));
        }
    }
}
