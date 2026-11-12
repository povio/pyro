//! The ARIA tree of an element, as Playwright's `locator.ariaSnapshot()` and
//! `toMatchAriaSnapshot()` see it (their default mode): elements with an ARIA
//! role, with their accessible name, states and text, and no generic
//! containers. The `read: "aria"` step returns it as JSON; the client renders
//! it as YAML and matches templates against it.
//!
//! Node: `{role, name, checked?, disabled?, expanded?, level?, pressed?,
//! selected?, props: {url?, placeholder?}, children: [string | node]}`. The
//! root is a `fragment` holding the element's own node (or its content when
//! it has no role).
//!
//! `mode: "ai"` (`aria_tree_ai`) is Playwright's AI snapshot: elements
//! hidden for ARIA but visible on screen are kept, elements without a role
//! are `generic`, and visible elements that receive pointer events get a
//! ref (`ref: "e5"`, `f1e5` in a child frame) that the `ariaRef` locator
//! finds again; nodes also carry `active` and `cursor: "pointer"`.
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

use pyro_dom::layout::Layout;
use pyro_dom::{Dom, NodeId, NodeKind};
use pyro_webapi::PageDom;
use serde_json::{json, Map, Value};

use super::driver::{accessible_name, attr, hidden_for_aria, js_number, local, natively_disabled, role_of};

/// Most nodes one snapshot holds; past it the read fails instead of
/// building an unbounded reply.
const MAX_NODES: usize = 50_000;

const CHECKED_ROLES: &[&str] =
    &["checkbox", "menuitemcheckbox", "option", "radio", "switch", "menuitemradio", "treeitem"];
const DISABLED_ROLES: &[&str] = &[
    "application",
    "button",
    "composite",
    "gridcell",
    "group",
    "input",
    "link",
    "menuitem",
    "scrollbar",
    "separator",
    "tab",
    "checkbox",
    "columnheader",
    "combobox",
    "grid",
    "listbox",
    "menu",
    "menubar",
    "menuitemcheckbox",
    "menuitemradio",
    "option",
    "radio",
    "radiogroup",
    "row",
    "rowheader",
    "searchbox",
    "select",
    "slider",
    "spinbutton",
    "switch",
    "tablist",
    "textbox",
    "toolbar",
    "tree",
    "treegrid",
    "treeitem",
];
const EXPANDED_ROLES: &[&str] = &[
    "application",
    "button",
    "checkbox",
    "combobox",
    "gridcell",
    "link",
    "listbox",
    "menuitem",
    "row",
    "rowheader",
    "tab",
    "treeitem",
    "columnheader",
    "menuitemcheckbox",
    "menuitemradio",
    "switch",
];
const LEVEL_ROLES: &[&str] = &["heading", "listitem", "row", "treeitem"];
const SELECTED_ROLES: &[&str] = &["gridcell", "option", "row", "tab", "rowheader", "columnheader", "treeitem"];

enum Child {
    Text(String),
    Node(Node),
}

struct Node {
    role: String,
    name: String,
    states: Map<String, Value>,
    props: Map<String, Value>,
    children: Vec<Child>,
    /// AI mode: the element's ref, whether it is the document's
    /// `activeElement`, and whether its cursor is `pointer`.
    aria_ref: Option<String>,
    active: bool,
    pointer: bool,
}

impl Node {
    fn new(role: &str, name: String) -> Node {
        Node {
            role: role.into(),
            name,
            states: Map::new(),
            props: Map::new(),
            children: Vec::new(),
            aria_ref: None,
            active: false,
            pointer: false,
        }
    }
}

/// A document's refs: Playwright keeps one on each element (`_ariaRef`,
/// kept while its role and name stay the same) and the last AI snapshot's
/// ref → element map, which the `aria-ref` selector engine reads.
struct DocRefs {
    page: Weak<PageDom>,
    last_ref: u64,
    by_node: HashMap<NodeId, (String, String, String)>,
    last: HashMap<String, NodeId>,
}

thread_local! {
    static REFS: RefCell<Vec<DocRefs>> = const { RefCell::new(Vec::new()) };
}

/// Forgets every snapshot's refs: the thread's pages are gone (a browser
/// thread between two clients).
pub(crate) fn forget_refs() {
    REFS.with(|refs| refs.borrow_mut().clear());
}

fn is_page(w: &Weak<PageDom>, page: &PageDom) -> bool {
    w.strong_count() > 0 && std::ptr::eq(w.as_ptr(), page)
}

/// The element the last AI snapshot of `page` gave the ref `r`, if it is
/// still in that document (Playwright's `aria-ref` engine).
pub(super) fn ref_node(page: &PageDom, r: &str) -> Option<NodeId> {
    let node =
        REFS.with(|refs| refs.borrow().iter().find(|d| is_page(&d.page, page)).and_then(|d| d.last.get(r).copied()))?;
    let dom = page.dom.borrow();
    (dom.root_node(node, true).ok() == Some(page.document())).then_some(node)
}

/// AI mode's state for one walk.
struct Ai<'p> {
    prefix: &'p str,
    refs: DocRefs,
    /// `document.activeElement`: the focused element, else the body.
    active: Option<NodeId>,
    /// `<iframe>`s given a ref, in tree order.
    iframes: Vec<(String, NodeId)>,
}

struct Walk<'a, 'p> {
    dom: &'a Dom,
    layout: &'a Layout,
    doc: NodeId,
    count: usize,
    /// Playwright's `visited`: each node once, so an `aria-owns` target is
    /// not shown twice and an ownership cycle ends.
    visited: HashSet<NodeId>,
    ai: Option<Ai<'p>>,
    /// AI mode: whether the element being walked is shown (its text
    /// counts), and whether it is visible for ARIA (so its children need
    /// not look at their ancestors again).
    parent_visible: bool,
    parent_aria_visible: bool,
}

/// Playwright's `normalizeWhiteSpace`.
fn squash(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '\u{200b}' | '\u{ad}'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The tree under `root`, or an error past the node budget.
pub(super) fn aria_tree(dom: &Dom, layout: &Layout, doc: NodeId, root: NodeId) -> Result<Value, String> {
    let mut w = Walk::new(dom, layout, doc, None);
    let mut top = Node::new("fragment", String::new());
    // The root is checked against its ancestors too: inside a display:none
    // or aria-hidden subtree it gives an empty snapshot, as in Playwright.
    if !(dom.element(root).is_ok() && hidden_for_aria(dom, layout, root, true)) {
        w.visit(&mut top, root);
    }
    w.finish(top)
}

/// The AI-mode tree under `root` (`ariaSnapshot({ mode: "ai" })`), new refs
/// named `{prefix}e{n}`, and the `<iframe>`s it gave a ref. It becomes the
/// document's last snapshot for `ref_node`.
pub(super) fn aria_tree_ai(
    page: &Rc<PageDom>,
    root: NodeId,
    prefix: &str,
) -> Result<(Value, Vec<(String, NodeId)>), String> {
    let refs = REFS.with(|r| {
        let mut r = r.borrow_mut();
        r.retain(|d| d.page.strong_count() > 0);
        match r.iter().position(|d| is_page(&d.page, page)) {
            Some(i) => r.swap_remove(i),
            None => DocRefs { page: Rc::downgrade(page), last_ref: 0, by_node: HashMap::new(), last: HashMap::new() },
        }
    });
    let dom = page.dom.borrow();
    let layout = page.layout();
    let doc = page.document();
    let active = dom.active_element(doc).ok().flatten();
    let mut w = Walk::new(&dom, &layout, doc, Some(Ai { prefix, refs, active, iframes: Vec::new() }));
    if let Some(ai) = &mut w.ai {
        ai.refs.last.clear();
    }
    // Playwright checks the root against its ancestors as any element.
    w.parent_aria_visible = false;
    let mut top = Node::new("fragment", String::new());
    w.visit(&mut top, root);
    let ai = w.ai.take().expect("ai walk");
    REFS.with(|r| r.borrow_mut().push(ai.refs));
    let tree = w.finish(top)?;
    Ok((tree, ai.iframes))
}

impl<'a, 'p> Walk<'a, 'p> {
    fn new(dom: &'a Dom, layout: &'a Layout, doc: NodeId, ai: Option<Ai<'p>>) -> Self {
        Walk {
            dom,
            layout,
            doc,
            count: 0,
            visited: HashSet::new(),
            ai,
            parent_visible: true,
            parent_aria_visible: true,
        }
    }

    fn finish(&self, mut top: Node) -> Result<Value, String> {
        if self.count > MAX_NODES {
            return Err(format!("the aria snapshot has more than {MAX_NODES} nodes"));
        }
        normalize_text(&mut top);
        normalize_generic(&mut top);
        Ok(to_json(top))
    }

    fn visit(&mut self, parent: &mut Node, id: NodeId) {
        if !self.visited.insert(id) {
            return;
        }
        self.count += 1;
        if self.count > MAX_NODES {
            return;
        }
        let dom = self.dom;
        match dom.node(id).map(|n| &n.kind) {
            Ok(NodeKind::Text(t) | NodeKind::CData(t)) => {
                if self.parent_visible && parent.role != "textbox" && !t.is_empty() {
                    parent.children.push(Child::Text(t.to_string()));
                }
                return;
            }
            Ok(NodeKind::Element(_)) => {}
            _ => return,
        }
        // AI mode ("ariaOrVisible"): an element hidden for ARIA but shown
        // on screen stays, and a hidden one's children are still walked.
        let (visible, aria_visible) = if self.ai.is_some() {
            let aria_visible = !hidden_for_aria(dom, self.layout, id, !self.parent_aria_visible);
            (aria_visible || self.box_visible(id), aria_visible)
        } else if hidden_for_aria(dom, self.layout, id, false) {
            return;
        } else {
            (true, true)
        };
        let owned: Vec<NodeId> = attr(dom, id, "aria-owns")
            .map(|v| v.split_whitespace().filter_map(|i| dom.get_element_by_id(self.doc, i).ok().flatten()).collect())
            .unwrap_or_default();
        // An owned element can sit anywhere: like Playwright's
        // isElementHiddenForAria, its display:none / aria-hidden ancestors
        // hide it too, not only its own style. (AI mode checks each one as
        // it visits it.)
        let owned: Vec<NodeId> = if self.ai.is_some() {
            owned
        } else {
            owned.into_iter().filter(|&o| !hidden_for_aria(dom, self.layout, o, true)).collect()
        };
        let saved = (self.parent_visible, self.parent_aria_visible);
        (self.parent_visible, self.parent_aria_visible) = (visible, aria_visible);
        match if visible { self.node_for(id) } else { None } {
            Some(mut node) => {
                self.content(&mut node, id, &owned);
                parent.children.push(Child::Node(node));
            }
            None => self.content(parent, id, &owned),
        }
        (self.parent_visible, self.parent_aria_visible) = saved;
    }

    /// Playwright's `computeBox(element).visible`: shown (`checkVisibility()`
    /// and `visibility: visible`) with an area; a `display: contents`
    /// element through its children.
    fn box_visible(&self, id: NodeId) -> bool {
        let (dom, layout) = (self.dom, self.layout);
        if layout.computed_style_value(dom, id, "display") == "contents" {
            return dom.children(id).into_iter().flatten().any(|c| match dom.node(c).map(|n| &n.kind) {
                Ok(NodeKind::Element(_)) => self.box_visible(c),
                Ok(NodeKind::Text(t)) => !t.trim().is_empty() && layout.is_visible(dom, id),
                _ => false,
            });
        }
        layout.is_visible(dom, id)
            && !layout.style().has_visibility_hidden_inherited(dom, id)
            && layout.client_rect(dom, id).is_some_and(|r| r.width > 0.0 && r.height > 0.0)
    }

    /// Playwright's `computeAriaRef` (refs "interactable"), `active` and
    /// the cursor, for a node of the AI tree.
    fn ai_fields(&mut self, node: &mut Node, id: NodeId, pointer_events: bool) {
        let Some(ai) = &self.ai else { return };
        node.active = ai.active == Some(id);
        if !pointer_events || !self.box_visible(id) {
            return;
        }
        let pointer = self.layout.computed_style_value(self.dom, id, "cursor") == "pointer";
        let ai = self.ai.as_mut().expect("ai walk");
        let refs = &mut ai.refs;
        let r = match refs.by_node.get(&id) {
            Some((role, name, r)) if *role == node.role && *name == node.name => r.clone(),
            _ => {
                refs.last_ref += 1;
                let r = format!("{}e{}", ai.prefix, refs.last_ref);
                refs.by_node.insert(id, (node.role.clone(), node.name.clone(), r.clone()));
                r
            }
        };
        refs.last.insert(r.clone(), id);
        if node.role == "iframe" {
            ai.iframes.push((r.clone(), id));
        }
        node.aria_ref = Some(r);
        node.pointer = pointer;
    }

    /// Playwright's `toAriaNode`: `None` for elements without a role.
    fn node_for(&mut self, id: NodeId) -> Option<Node> {
        let dom = self.dom;
        if local(dom, id) == Some("iframe") {
            let mut n = Node::new("iframe", String::new());
            self.ai_fields(&mut n, id, true);
            return Some(n);
        }
        // AI mode's `includeGenericRole`: no role is `generic`.
        let role = match role_of(dom, id) {
            Some(r) => r,
            None if self.ai.is_some() => "generic".into(),
            None => return None,
        };
        if matches!(role.as_str(), "presentation" | "none") {
            return None;
        }
        let r = role.as_str();
        let display = self.layout.computed_style_value(dom, id, "display");
        // Playwright's `computeBox` calls only `display: inline` inline.
        if r == "generic" && display == "inline" && only_text_child(dom, id) {
            return None;
        }
        let name = squash(&accessible_name(dom, self.layout, self.doc, id));
        let mut n = Node::new(r, name);
        let pointer_events = !self.layout.style().has_pointer_events_none(dom, id);
        self.ai_fields(&mut n, id, pointer_events);
        if CHECKED_ROLES.contains(&r) {
            n.states.insert("checked".into(), checked(dom, id));
        }
        if DISABLED_ROLES.contains(&r) {
            n.states.insert("disabled".into(), json!(natively_disabled(dom, id) || aria_disabled(dom, id, false)));
        }
        if let Some(e) = expanded(dom, id, r) {
            n.states.insert("expanded".into(), json!(e));
        }
        if LEVEL_ROLES.contains(&r) {
            n.states.insert("level".into(), json!(level(dom, id, r)));
        }
        if r == "button" {
            n.states.insert(
                "pressed".into(),
                match attr(dom, id, "aria-pressed") {
                    Some("true") => json!(true),
                    Some("mixed") => json!("mixed"),
                    _ => json!(false),
                },
            );
        }
        if SELECTED_ROLES.contains(&r) {
            let selected = if local(dom, id) == Some("option") {
                dom.option_selected(id).unwrap_or(false)
            } else {
                attr(dom, id, "aria-selected").is_some_and(|v| v.eq_ignore_ascii_case("true"))
            };
            n.states.insert("selected".into(), json!(selected));
        }
        if matches!(local(dom, id), Some("input" | "textarea")) {
            let ty = attr(dom, id, "type").unwrap_or("").to_ascii_lowercase();
            if !matches!(ty.as_str(), "checkbox" | "radio" | "file") {
                n.children.push(Child::Text(dom.value(id).unwrap_or_default()));
            }
        }
        Some(n)
    }

    /// Playwright's `processElement`: the element's content into `node`.
    fn content(&mut self, node: &mut Node, id: NodeId, owned: &[NodeId]) {
        let dom = self.dom;
        let display = self.layout.computed_style_value(dom, id, "display");
        let block = display != "inline" || local(dom, id) == Some("br");
        if block {
            node.children.push(Child::Text(" ".into()));
        }
        // CSS generated content (Playwright's `getCSSContent`).
        let pseudo = |p| self.layout.style().css_content(dom, id, Some(p)).filter(|t| !t.is_empty());
        if let Some(before) = pseudo(pyro_dom::style::PseudoElement::Before) {
            node.children.push(Child::Text(before));
        }
        let after = pseudo(pyro_dom::style::PseudoElement::After);
        let assigned =
            if local(dom, id) == Some("slot") { dom.assigned_nodes(id, false).unwrap_or_default() } else { vec![] };
        if !assigned.is_empty() {
            for c in assigned {
                self.visit(node, c);
            }
        } else {
            let mut child = dom.first_child(id).ok().flatten();
            while let Some(c) = child {
                if dom.assigned_slot(c).ok().flatten().is_none() {
                    self.visit(node, c);
                }
                child = dom.next_sibling(c).ok().flatten();
            }
            if let Some(shadow) = dom.shadow_root(id).ok().flatten() {
                let mut child = dom.first_child(shadow).ok().flatten();
                while let Some(c) = child {
                    self.visit(node, c);
                    child = dom.next_sibling(c).ok().flatten();
                }
            }
        }
        // AI mode: an owned element's ancestors are not this element's.
        let saved = self.parent_aria_visible;
        if self.ai.is_some() {
            self.parent_aria_visible = false;
        }
        for &c in owned {
            self.visit(node, c);
        }
        self.parent_aria_visible = saved;
        if let Some(after) = after {
            node.children.push(Child::Text(after));
        }
        if block {
            node.children.push(Child::Text(" ".into()));
        }
        if node.children.len() == 1 && matches!(&node.children[0], Child::Text(t) if *t == node.name) {
            node.children.clear();
        }
        if node.role == "link" {
            if let Some(href) = attr(dom, id, "href") {
                node.props.insert("url".into(), json!(href));
            }
        }
        if node.role == "textbox" {
            if let Some(p) = attr(dom, id, "placeholder").filter(|p| *p != node.name) {
                node.props.insert("placeholder".into(), json!(p));
            }
        }
    }
}

fn only_text_child(dom: &Dom, id: NodeId) -> bool {
    let mut it = dom.children(id).into_iter().flatten();
    matches!((it.next(), it.next()), (Some(c), None) if matches!(dom.node(c).map(|n| &n.kind), Ok(NodeKind::Text(_))))
}

fn checked(dom: &Dom, id: NodeId) -> Value {
    let native = local(dom, id) == Some("input")
        && attr(dom, id, "type").is_some_and(|t| t.eq_ignore_ascii_case("checkbox") || t.eq_ignore_ascii_case("radio"));
    if local(dom, id) == Some("input") && dom.control_state(id).ok().flatten().is_some_and(|c| c.indeterminate) {
        return json!("mixed");
    }
    if native {
        return json!(dom.checked(id).unwrap_or(false));
    }
    match attr(dom, id, "aria-checked") {
        Some("true") => json!(true),
        Some("mixed") => json!("mixed"),
        _ => json!(false),
    }
}

/// Playwright's `hasExplicitAriaDisabled`: `aria-disabled` here or, through
/// `false`-less ancestors, above.
fn aria_disabled(dom: &Dom, id: NodeId, ancestor: bool) -> bool {
    let mut cur = Some(id);
    let mut is_ancestor = ancestor;
    while let Some(n) = cur {
        if !is_ancestor && !role_of(dom, n).is_some_and(|r| DISABLED_ROLES.contains(&r.as_str())) {
            return false;
        }
        match attr(dom, n, "aria-disabled").map(|v| v.to_ascii_lowercase()).as_deref() {
            Some("true") => return true,
            Some("false") => return false,
            _ => {}
        }
        is_ancestor = true;
        cur = dom.parent_element(n).ok().flatten();
    }
    false
}

/// Playwright's `getAriaExpanded`, asked only for `kAriaExpandedRoles`: a
/// `<details>` (role `group`) is not one of them.
fn expanded(dom: &Dom, id: NodeId, role: &str) -> Option<bool> {
    if !EXPANDED_ROLES.contains(&role) {
        return None;
    }
    if local(dom, id) == Some("details") {
        return Some(attr(dom, id, "open").is_some());
    }
    attr(dom, id, "aria-expanded").map(|v| v == "true")
}

fn heading_level(dom: &Dom, id: NodeId) -> Option<u32> {
    local(dom, id).and_then(|t| t.strip_prefix('h')).and_then(|d| d.parse::<u32>().ok()).filter(|d| (1..=6).contains(d))
}

fn level(dom: &Dom, id: NodeId, role: &str) -> u32 {
    if let Some(l) = heading_level(dom, id) {
        return l;
    }
    if LEVEL_ROLES.contains(&role) {
        // `Number(attr)` then `Number.isInteger(value) && value >= 1`.
        if let Some(v) = attr(dom, id, "aria-level").map(js_number).filter(|l| l.fract() == 0.0 && *l >= 1.0) {
            return if v > u32::MAX as f64 { u32::MAX } else { v as u32 };
        }
    }
    0
}

/// Playwright's `normalizeStringChildren`: adjacent text joined and
/// whitespace-normalized, a lone child equal to the name dropped.
fn normalize_text(node: &mut Node) {
    let mut out = Vec::with_capacity(node.children.len());
    let mut buf = String::new();
    let flush = |buf: &mut String, out: &mut Vec<Child>| {
        let t = squash(buf);
        if !t.is_empty() {
            out.push(Child::Text(t));
        }
        buf.clear();
    };
    for c in std::mem::take(&mut node.children) {
        match c {
            Child::Text(t) => buf.push_str(&t),
            Child::Node(mut n) => {
                flush(&mut buf, &mut out);
                normalize_text(&mut n);
                out.push(Child::Node(n));
            }
        }
    }
    flush(&mut buf, &mut out);
    if out.len() == 1 && matches!(&out[0], Child::Text(t) if *t == node.name) {
        out.clear();
    }
    node.children = out;
}

/// Playwright's `normalizeGenericRoles`: an unnamed `generic` with no
/// children, or (AI mode) with only one child that has a ref, gives way to
/// its children.
fn normalize_generic(node: &mut Node) {
    let mut out = Vec::with_capacity(node.children.len());
    for c in std::mem::take(&mut node.children) {
        match c {
            Child::Text(t) => out.push(Child::Text(t)),
            Child::Node(mut n) => {
                normalize_generic(&mut n);
                let lone_refs = n.children.len() <= 1
                    && n.children.iter().all(|c| matches!(c, Child::Node(m) if m.aria_ref.is_some()));
                if n.role == "generic" && n.name.is_empty() && lone_refs {
                    out.append(&mut n.children);
                } else {
                    out.push(Child::Node(n));
                }
            }
        }
    }
    node.children = out;
}

fn to_json(n: Node) -> Value {
    let mut o = Map::new();
    o.insert("role".into(), json!(n.role));
    o.insert("name".into(), json!(n.name));
    if n.active {
        o.insert("active".into(), json!(true));
    }
    if let Some(r) = n.aria_ref {
        o.insert("ref".into(), json!(r));
        if n.pointer {
            o.insert("cursor".into(), json!("pointer"));
        }
    }
    o.extend(n.states);
    o.insert("props".into(), Value::Object(n.props));
    o.insert(
        "children".into(),
        Value::Array(
            n.children
                .into_iter()
                .map(|c| match c {
                    Child::Text(t) => Value::String(t),
                    Child::Node(n) => to_json(n),
                })
                .collect(),
        ),
    );
    Value::Object(o)
}
