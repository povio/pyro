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
use std::collections::HashSet;

use lp_dom::layout::Layout;
use lp_dom::{Dom, NodeId, NodeKind};
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
}

impl Node {
    fn new(role: &str, name: String) -> Node {
        Node { role: role.into(), name, states: Map::new(), props: Map::new(), children: Vec::new() }
    }
}

struct Walk<'a> {
    dom: &'a Dom,
    layout: &'a Layout,
    doc: NodeId,
    count: usize,
    /// Playwright's `visited`: each node once, so an `aria-owns` target is
    /// not shown twice and an ownership cycle ends.
    visited: HashSet<NodeId>,
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
    let mut w = Walk { dom, layout, doc, count: 0, visited: HashSet::new() };
    let mut top = Node::new("fragment", String::new());
    // The root is checked against its ancestors too: inside a display:none
    // or aria-hidden subtree it gives an empty snapshot, as in Playwright.
    if !(dom.element(root).is_ok() && hidden_for_aria(dom, layout, root, true)) {
        w.visit(&mut top, root);
    }
    if w.count > MAX_NODES {
        return Err(format!("the aria snapshot has more than {MAX_NODES} nodes"));
    }
    normalize_text(&mut top);
    normalize_generic(&mut top);
    Ok(to_json(top))
}

impl Walk<'_> {
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
                if parent.role != "textbox" && !t.is_empty() {
                    parent.children.push(Child::Text(t.to_string()));
                }
                return;
            }
            Ok(NodeKind::Element(_)) => {}
            _ => return,
        }
        if hidden_for_aria(dom, self.layout, id, false) {
            return;
        }
        let owned: Vec<NodeId> = attr(dom, id, "aria-owns")
            .map(|v| v.split_whitespace().filter_map(|i| dom.get_element_by_id(self.doc, i).ok().flatten()).collect())
            .unwrap_or_default();
        // An owned element can sit anywhere: like Playwright's
        // isElementHiddenForAria, its display:none / aria-hidden ancestors
        // hide it too, not only its own style.
        let owned: Vec<NodeId> = owned.into_iter().filter(|&o| !hidden_for_aria(dom, self.layout, o, true)).collect();
        match self.node_for(id) {
            Some(mut node) => {
                self.content(&mut node, id, &owned);
                parent.children.push(Child::Node(node));
            }
            None => self.content(parent, id, &owned),
        }
    }

    /// Playwright's `toAriaNode`: `None` for elements without a role.
    fn node_for(&self, id: NodeId) -> Option<Node> {
        let dom = self.dom;
        if local(dom, id) == Some("iframe") {
            return Some(Node::new("iframe", String::new()));
        }
        let role = role_of(dom, id)?;
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
        for &c in owned {
            self.visit(node, c);
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

/// Playwright's `normalizeGenericRoles` (refs off): an unnamed `generic`
/// left with no children is dropped.
fn normalize_generic(node: &mut Node) {
    node.children.retain_mut(|c| match c {
        Child::Text(_) => true,
        Child::Node(n) => {
            normalize_generic(n);
            !(n.role == "generic" && n.name.is_empty() && n.children.is_empty())
        }
    });
}

fn to_json(n: Node) -> Value {
    let mut o = Map::new();
    o.insert("role".into(), json!(n.role));
    o.insert("name".into(), json!(n.name));
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
