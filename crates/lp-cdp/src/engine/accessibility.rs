//! `Accessibility.getFullAXTree`, `getPartialAXTree` and `queryAXTree`.
//!
//! Each command walks the document once and builds a Chrome-shaped tree from
//! the driver's ARIA role and accessible-name computation (the same code
//! `getByRole` uses). Nothing is cached or tracked between commands, so DOM
//! mutations and tasks pay nothing for this domain.
//!
//! The tree follows Chromium's structure where that is cheap to derive: a
//! `RootWebArea` for the document, `<html>`/`<body>` and layout-only blocks as
//! ignored nodes, unsemantic inline elements flattened into their parent,
//! `StaticText` for text, `display:none` content left out and `aria-hidden`
//! subtrees kept as ignored nodes. Node ids are the DOM domain's backend node
//! ids. Not produced: `InlineTextBox` nodes, Chromium's synthesized nodes
//! (list markers, the `<select>` popup, text-field inner editors), name
//! `sources`, and shadow/child-frame contents (an `<iframe>` is an `Iframe`
//! leaf; its tree is fetched with `frameId`).
use std::collections::HashMap;
use std::rc::Rc;

use lp_dom::forms::InputType;
use lp_dom::layout::Layout;
use lp_dom::{Dom, NodeId, NodeKind};
use lp_webapi::PageDom;
use serde::Deserialize;
use serde_json::{json, Value};

use super::dom::NodeParams;
use super::driver::{accessible_name, aria_true, attr, id_refs, labels, local, normalize, role_of, text_of};
use super::EngineHost;
use crate::msg::{CdpError, CdpResult};
use crate::target::PageCommand;

/// Most nodes one command builds; past it the command fails explicitly
/// instead of serializing an unbounded reply.
const MAX_AX_NODES: usize = 16384;

/// Why a node is in the tree but ignored (Chromium's `ignoredReasons`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ignored {
    Uninteresting,
    Presentational,
    AriaHiddenElement,
    /// Inside the `aria-hidden`/`inert` element given.
    AriaHiddenSubtree(NodeId),
    /// A `<label>` wrapping the control given.
    LabelFor(NodeId),
    NotRendered,
}

/// A property value that names other DOM nodes, resolved to backend ids when
/// serialized.
enum Prop {
    Plain(Value),
    /// `nodeList`: related nodes with their text.
    Nodes(Vec<(NodeId, String)>),
    /// `idrefList`: the attribute value and the nodes its IDREFs name.
    IdRefs(String, Vec<(NodeId, String)>),
}

struct AxNode {
    node: NodeId,
    parent: Option<usize>,
    children: Vec<usize>,
    ignored: Option<Ignored>,
    internal: bool,
    role: &'static str,
    /// An explicit `role` token that is not one of the static names.
    custom_role: Option<String>,
    name: String,
    description: Option<String>,
    value: Option<Value>,
    props: Vec<(&'static str, Prop)>,
}

impl AxNode {
    fn role(&self) -> &str {
        self.custom_role.as_deref().unwrap_or(self.role)
    }
}

/// Outside the topmost modal dialog: neither it, inside it, nor one of its
/// ancestors (Chromium leaves what a modal dialog blocks out of the tree).
/// One empty-set check without a modal dialog.
fn blocked_by_modal(dom: &Dom, el: NodeId) -> bool {
    dom.topmost_modal().is_some_and(|m| !dom.contains(el, m).unwrap_or(true) && !dom.contains(m, el).unwrap_or(true))
}

/// Inherited walk state.
#[derive(Clone, Copy, Default)]
struct Ctx {
    /// The `aria-hidden`/`inert` element this node is inside.
    hidden: Option<NodeId>,
    /// `editable` of the enclosing contenteditable host.
    editable: Option<&'static str>,
    /// Text of a `<label>` wrapping its control (Chromium ignores it).
    label_text: bool,
    /// Parent has `visibility: hidden`.
    invisible: bool,
    /// Number of enclosing lists.
    lists: u32,
}

/// What an element contributes.
enum Class {
    /// Not in the tree; its children attach to the parent.
    Flatten,
    Node {
        role: &'static str,
        custom: Option<String>,
        internal: bool,
        ignored: Option<Ignored>,
    },
}

fn node(role: &'static str) -> Class {
    Class::Node { role, custom: None, internal: false, ignored: None }
}
fn internal(role: &'static str) -> Class {
    Class::Node { role, custom: None, internal: true, ignored: None }
}
fn ignored(reason: Ignored) -> Class {
    Class::Node { role: "none", custom: None, internal: false, ignored: Some(reason) }
}

/// Interns the ARIA roles the driver computes (all static names) so nodes
/// carry `&'static str`; anything else is kept as an owned custom role.
fn static_role(role: &str) -> Option<&'static str> {
    const ROLES: &[&str] = &[
        "alert",
        "alertdialog",
        "application",
        "article",
        "banner",
        "blockquote",
        "button",
        "caption",
        "cell",
        "checkbox",
        "code",
        "columnheader",
        "combobox",
        "complementary",
        "contentinfo",
        "definition",
        "deletion",
        "dialog",
        "directory",
        "document",
        "emphasis",
        "feed",
        "figure",
        "form",
        "generic",
        "grid",
        "gridcell",
        "group",
        "heading",
        "image",
        "insertion",
        "link",
        "list",
        "listbox",
        "listitem",
        "log",
        "main",
        "mark",
        "marquee",
        "math",
        "menu",
        "menubar",
        "menuitem",
        "menuitemcheckbox",
        "menuitemradio",
        "meter",
        "navigation",
        "none",
        "note",
        "option",
        "paragraph",
        "presentation",
        "progressbar",
        "radio",
        "radiogroup",
        "region",
        "row",
        "rowgroup",
        "rowheader",
        "scrollbar",
        "search",
        "searchbox",
        "separator",
        "slider",
        "spinbutton",
        "status",
        "strong",
        "subscript",
        "superscript",
        "switch",
        "tab",
        "table",
        "tablist",
        "tabpanel",
        "term",
        "textbox",
        "time",
        "timer",
        "toolbar",
        "tooltip",
        "tree",
        "treegrid",
        "treeitem",
    ];
    ROLES.iter().copied().find(|r| *r == role)
}

fn from_role(role: &str) -> Class {
    match static_role(role) {
        Some(r) => node(r),
        None => Class::Node { role: "generic", custom: Some(role.to_owned()), internal: false, ignored: None },
    }
}

fn boolean(v: bool) -> Prop {
    Prop::Plain(json!({"type":"boolean","value":v}))
}
fn boolean_or_undefined(v: bool) -> Prop {
    Prop::Plain(json!({"type":"booleanOrUndefined","value":v}))
}
fn token(v: &str) -> Prop {
    Prop::Plain(json!({"type":"token","value":v}))
}
fn tristate(v: &str) -> Prop {
    Prop::Plain(json!({"type":"tristate","value":v}))
}
/// A JSON number, integral values without a fraction (as Chromium sends them).
fn number_value(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        json!(n as i64)
    } else {
        json!(n)
    }
}
fn number(n: f64) -> Prop {
    Prop::Plain(json!({"type":"number","value":number_value(n)}))
}

fn num_attr(dom: &Dom, id: NodeId, name: &str) -> Option<f64> {
    attr(dom, id, name).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite())
}

/// `href`/`src` resolved against the document base, or `None` when it does
/// not resolve (Chromium then reports no `url`).
fn resolve(dom: &Dom, id: NodeId, value: &str) -> Option<String> {
    let doc = dom.node_document(id).ok()?;
    let base = lp_net::Url::parse(dom.document_base_url(doc).ok()?).ok()?;
    base.join(value.trim()).ok().map(|u| u.to_string())
}

fn blank(node: NodeId, role: &'static str) -> AxNode {
    AxNode {
        node,
        parent: None,
        children: Vec::new(),
        ignored: None,
        internal: false,
        role,
        custom_role: None,
        name: String::new(),
        description: None,
        value: None,
        props: Vec::new(),
    }
}

struct Builder<'a> {
    dom: &'a Dom,
    layout: &'a Layout,
    doc: NodeId,
    focused: Option<NodeId>,
    nodes: Vec<AxNode>,
    /// Flattened (and skipped) elements and the tree node their content
    /// attaches to, for `getPartialAXTree` on them.
    flattened: Option<HashMap<NodeId, usize>>,
    stack: Vec<Work>,
    over_budget: bool,
}

/// A DOM node waiting to be added under tree node `parent`.
enum Work {
    Element { parent: usize, id: NodeId, ctx: Ctx },
    Text { parent: usize, id: NodeId, prev: Option<NodeId>, next: Option<NodeId>, ctx: Ctx },
}

impl<'a> Builder<'a> {
    fn new(dom: &'a Dom, layout: &'a Layout, doc: NodeId, track: bool) -> Self {
        let focused = dom.focused(doc).ok().flatten();
        Builder {
            dom,
            layout,
            doc,
            focused,
            nodes: Vec::new(),
            flattened: track.then(HashMap::new),
            stack: Vec::new(),
            over_budget: false,
        }
    }

    fn push(&mut self, parent: Option<usize>, n: AxNode) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(AxNode { parent, ..n });
        if let Some(p) = parent {
            self.nodes[p].children.push(idx);
        }
        idx
    }

    fn build(&mut self, title: String, url: Option<String>) {
        let mut root = blank(self.doc, "RootWebArea");
        root.internal = true;
        root.name = title;
        root.props.push(("focusable", boolean_or_undefined(true)));
        if self.focused.is_none() {
            root.props.push(("focused", boolean_or_undefined(true)));
        }
        if let Some(url) = url {
            root.props.push(("url", Prop::Plain(json!({"type":"string","value":url}))));
        }
        let idx = self.push(None, root);
        self.children(idx, self.doc, Ctx::default());
        // An explicit stack, so arbitrarily deep documents do not recurse.
        while let Some(work) = self.stack.pop() {
            if self.nodes.len() > MAX_AX_NODES {
                self.over_budget = true;
                return;
            }
            match work {
                Work::Element { parent, id, ctx } => self.element(parent, id, ctx),
                Work::Text { parent, id, prev, next, ctx } => {
                    let dom = self.dom;
                    if let Ok(NodeKind::Text(t) | NodeKind::CData(t)) = dom.node(id).map(|n| &n.kind) {
                        self.text(parent, id, t, prev, next, ctx);
                    }
                }
            }
        }
    }

    /// Queues the children of `of` under tree node `parent`, in order.
    fn children(&mut self, parent: usize, of: NodeId, ctx: Ctx) {
        let dom = self.dom;
        let kids: Vec<NodeId> = dom.children(of).into_iter().flatten().collect();
        let start = self.stack.len();
        for (i, &c) in kids.iter().enumerate() {
            match dom.node(c).map(|n| &n.kind) {
                Ok(NodeKind::Text(_) | NodeKind::CData(_)) => {
                    let prev = kids[..i].iter().rev().copied().find(|k| !self.is_comment(*k));
                    let next = kids[i + 1..].iter().copied().find(|k| !self.is_comment(*k));
                    self.stack.push(Work::Text { parent, id: c, prev, next, ctx });
                }
                Ok(NodeKind::Element(_)) => self.stack.push(Work::Element { parent, id: c, ctx }),
                _ => {}
            }
        }
        self.stack[start..].reverse();
    }

    fn is_comment(&self, id: NodeId) -> bool {
        matches!(self.dom.node(id).map(|n| &n.kind), Ok(NodeKind::Comment(_)))
    }

    /// Whether a sibling keeps adjacent whitespace on the same line: text or
    /// a plain inline element.
    fn inline_sibling(&self, id: Option<NodeId>) -> bool {
        let Some(id) = id else { return false };
        match self.dom.node(id).map(|n| &n.kind) {
            Ok(NodeKind::Text(t) | NodeKind::CData(t)) => !t.trim().is_empty(),
            Ok(NodeKind::Element(_)) => {
                local(self.dom, id) != Some("br")
                    && self.layout.computed_style_value(self.dom, id, "display") == "inline"
            }
            _ => false,
        }
    }

    fn text(&mut self, parent: usize, id: NodeId, raw: &str, prev: Option<NodeId>, next: Option<NodeId>, ctx: Ctx) {
        if ctx.invisible {
            return;
        }
        // Collapsed whitespace, trimmed at block edges.
        let mut text = String::with_capacity(raw.len());
        let mut space = false;
        for ch in raw.chars() {
            if ch.is_ascii_whitespace() {
                space = true;
            } else {
                if space {
                    text.push(' ');
                }
                space = false;
                text.push(ch);
            }
        }
        if space {
            text.push(' ');
        }
        // A lone space between replaced elements (images, frames, controls)
        // is not a text run in Chromium.
        let replaced = |n: Option<NodeId>| {
            n.is_some_and(|n| {
                self.is_svg_root(n)
                    || matches!(
                        local(self.dom, n),
                        Some(
                            "img"
                                | "iframe"
                                | "canvas"
                                | "input"
                                | "button"
                                | "select"
                                | "textarea"
                                | "video"
                                | "audio"
                                | "object"
                                | "embed"
                        )
                    )
            })
        };
        if text == " " && (replaced(prev) || replaced(next)) {
            return;
        }
        if text.starts_with(' ') && !self.inline_sibling(prev) {
            text.remove(0);
        }
        if text.ends_with(' ') && !self.inline_sibling(next) {
            text.pop();
        }
        if text.is_empty() {
            return;
        }
        let mut n = blank(id, "StaticText");
        n.internal = true;
        n.name = text;
        if let Some(root) = ctx.hidden {
            n.ignored = Some(Ignored::AriaHiddenSubtree(root));
        } else if ctx.label_text {
            n.ignored = Some(Ignored::Presentational);
        } else if let Some(e) = ctx.editable {
            n.props.push(("editable", token(e)));
        }
        self.push(Some(parent), n);
    }

    fn flatten(&mut self, el: NodeId, parent: usize) {
        if let Some(map) = &mut self.flattened {
            map.insert(el, parent);
        }
    }

    fn element(&mut self, parent: usize, el: NodeId, mut ctx: Ctx) {
        let dom = self.dom;
        let style = self.layout.style();
        let tag = local(dom, el);
        let in_select = matches!(tag, Some("option" | "optgroup"))
            && dom
                .parent_element(el)
                .ok()
                .flatten()
                .is_some_and(|p| matches!(local(dom, p), Some("select" | "optgroup")));
        let skipped = matches!(
            tag,
            Some(
                "script"
                    | "style"
                    | "noscript"
                    | "template"
                    | "head"
                    | "title"
                    | "meta"
                    | "link"
                    | "base"
                    | "canvas"
                    | "datalist"
                    | "param"
                    | "source"
                    | "track"
            )
        ) || tag == Some("input") && dom.input_type(el).is_ok_and(|t| t == InputType::Hidden)
            || if in_select { attr(dom, el, "hidden").is_some() } else { style.is_hidden(dom, el, Default::default()) };
        if skipped {
            self.flatten(el, parent);
            return;
        }
        if !in_select && style.has_visibility_hidden_inherited(dom, el) {
            // Not rendered itself; a `visibility: visible` descendant is.
            self.flatten(el, parent);
            self.children(parent, el, Ctx { invisible: true, ..ctx });
            return;
        }
        ctx.invisible = false;
        let hidden_root = ctx.hidden.is_none()
            && (aria_true(dom, el, "aria-hidden") || attr(dom, el, "inert").is_some() || blocked_by_modal(dom, el));
        if hidden_root {
            ctx.hidden = Some(el);
        }
        let (role, custom, internal_role, mut why) = match self.classify(el, tag) {
            Class::Flatten => {
                self.flatten(el, parent);
                self.children(parent, el, ctx);
                return;
            }
            Class::Node { role, custom, internal, ignored } => (role, custom, internal, ignored),
        };
        if let Some(Ignored::LabelFor(_)) = why {
            ctx.label_text = true;
        }
        if hidden_root {
            why = Some(Ignored::AriaHiddenElement);
        } else if let Some(root) = ctx.hidden {
            why = Some(Ignored::AriaHiddenSubtree(root));
        }
        let mut n = blank(el, role);
        n.internal = internal_role;
        n.custom_role = custom;
        n.ignored = why;
        if why.is_none() {
            self.describe(&mut n, tag, ctx);
        }
        let idx = self.push(Some(parent), n);
        let editable = attr(dom, el, "contenteditable").map(|v| v.trim().to_ascii_lowercase());
        match editable.as_deref() {
            Some("false") => ctx.editable = None,
            Some("plaintext-only") => ctx.editable = Some("plaintext"),
            Some(_) => ctx.editable = Some("richtext"),
            None => {}
        }
        if self.nodes[idx].role() == "list" {
            ctx.lists += 1;
        }
        // Leaves: replaced elements and controls whose DOM children are not
        // rendered content.
        let leaf = matches!(
            tag,
            Some(
                "input"
                    | "textarea"
                    | "img"
                    | "iframe"
                    | "frame"
                    | "hr"
                    | "br"
                    | "option"
                    | "meter"
                    | "progress"
                    | "video"
                    | "audio"
                    | "object"
                    | "embed"
            )
        ) || self.is_svg_root(el);
        if leaf {
            return;
        }
        if tag == Some("details") && attr(dom, el, "open").is_none() {
            // Closed: only the summary is rendered.
            let summary = dom.children(el).into_iter().flatten().find(|c| local(dom, *c) == Some("summary"));
            if let Some(s) = summary {
                self.stack.push(Work::Element { parent: idx, id: s, ctx });
            }
            return;
        }
        self.children(idx, el, ctx);
    }

    fn is_svg_root(&self, el: NodeId) -> bool {
        self.dom
            .element(el)
            .ok()
            .is_some_and(|e| &*e.name.local == "svg" && e.namespace() == "http://www.w3.org/2000/svg")
    }

    fn focusable(&self, el: NodeId, tag: Option<&str>) -> bool {
        let dom = self.dom;
        dom.is_focusable(el).unwrap_or(false)
            || tag == Some("dialog")
            || tag == Some("option")
                && !dom.is_disabled(el).unwrap_or(false)
                && dom
                    .parent_element(el)
                    .ok()
                    .flatten()
                    .is_some_and(|p| matches!(local(dom, p), Some("select" | "optgroup")))
    }

    /// Chromium's role for an element (or that it is flattened / ignored).
    fn classify(&self, el: NodeId, tag: Option<&str>) -> Class {
        let dom = self.dom;
        if let Some(explicit) = attr(dom, el, "role").and_then(|r| r.split_ascii_whitespace().next()) {
            let explicit = explicit.to_ascii_lowercase();
            match explicit.as_str() {
                // Presentational roles are ignored on focusable elements.
                "presentation" | "none" if !self.focusable(el, tag) => return Class::Flatten,
                "presentation" | "none" => {}
                "img" => return node("image"),
                "generic" => return node("generic"),
                other => return from_role(other),
            }
        }
        match tag {
            Some("html" | "body") => return ignored(Ignored::Uninteresting),
            Some("label") => {
                return match self.labelled_control(el) {
                    Some(control) => ignored(Ignored::LabelFor(control)),
                    None => internal("LabelText"),
                };
            }
            Some("abbr") => return internal("Abbr"),
            Some("dl") => return internal("DescriptionList"),
            Some("figcaption") => return internal("Figcaption"),
            Some("legend") => return internal("Legend"),
            Some("iframe" | "frame") => return internal("Iframe"),
            Some("br") => return internal("LineBreak"),
            Some("summary")
                if dom.parent_element(el).ok().flatten().is_some_and(|p| local(dom, p) == Some("details")) =>
            {
                return internal("DisclosureTriangle")
            }
            Some("s") => return node("deletion"),
            Some("form") => return node("form"),
            Some("tbody") => return ignored(Ignored::Uninteresting),
            _ => {}
        }
        if self.is_svg_root(el) {
            return node("image");
        }
        match role_of(dom, el).as_deref() {
            Some("presentation" | "none") => Class::Flatten,
            Some("img") => node("image"),
            // The document role is the RootWebArea.
            Some("document") => ignored(Ignored::Uninteresting),
            Some(role) => from_role(role),
            None => self.generic(el),
        }
    }

    /// An element with no role: a `generic` node when it carries anything an
    /// author could reference or a block holding inline content, flattened
    /// when inline, ignored otherwise.
    fn generic(&self, el: NodeId) -> Class {
        let dom = self.dom;
        let Ok(e) = dom.element(el) else { return Class::Flatten };
        let keep = e.attrs.iter().any(|a| {
            let n = &*a.name.local;
            matches!(n, "id" | "title" | "tabindex")
                || n == "contenteditable" && !a.value.eq_ignore_ascii_case("false")
                || n.starts_with("aria-") && n != "aria-hidden"
        });
        if keep {
            return node("generic");
        }
        let display = self.layout.computed_style_value(dom, el, "display");
        if display == "inline" || display == "contents" {
            return Class::Flatten;
        }
        let inline_content = dom.children(el).into_iter().flatten().any(|c| match dom.node(c).map(|n| &n.kind) {
            Ok(NodeKind::Text(t) | NodeKind::CData(t)) => !t.trim().is_empty(),
            Ok(NodeKind::Element(_)) => {
                local(dom, c) == Some("br") || {
                    let d = self.layout.computed_style_value(dom, c, "display");
                    d.starts_with("inline")
                }
            }
            _ => false,
        });
        if inline_content {
            node("generic")
        } else {
            ignored(Ignored::Uninteresting)
        }
    }

    /// The labelable control inside a `<label>` that it names.
    fn labelled_control(&self, label: NodeId) -> Option<NodeId> {
        let dom = self.dom;
        if attr(dom, label, "for").is_some() {
            return None;
        }
        dom.descendants(label).into_iter().flatten().skip(1).find(|d| match local(dom, *d) {
            Some("input") => !dom.input_type(*d).is_ok_and(|t| t == InputType::Hidden),
            Some("button" | "meter" | "output" | "progress" | "select" | "textarea") => true,
            _ => false,
        })
    }

    /// Name, description, value and properties of an unignored element.
    fn describe(&self, n: &mut AxNode, tag: Option<&str>, ctx: Ctx) {
        let dom = self.dom;
        let el = n.node;
        let role = n.role().to_owned();
        let role = role.as_str();
        n.name = match role {
            "LineBreak" => "\n".into(),
            // Options of a `<select>` have no box; name them from their label
            // or text as Chromium does.
            "option" if self.focusable(el, tag) || dom.is_disabled(el).unwrap_or(false) => attr(dom, el, "aria-label")
                .or_else(|| attr(dom, el, "label"))
                .map(normalize)
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| normalize(&text_of(dom, el))),
            "group" if tag == Some("optgroup") && attr(dom, el, "aria-label").is_none() => {
                attr(dom, el, "label").map(normalize).unwrap_or_default()
            }
            // Chromium names these only from aria-label/labelledby: rows not
            // from their cells, generics not from title, figures not from
            // their caption.
            "row" | "generic" | "figure"
                if attr(dom, el, "aria-label").is_none() && attr(dom, el, "aria-labelledby").is_none() =>
            {
                String::new()
            }
            _ => accessible_name(dom, self.layout, self.doc, el),
        };
        let input_type = (tag == Some("input")).then(|| dom.input_type(el).unwrap_or(InputType::Text));
        let text_field =
            tag == Some("textarea") || input_type.is_some_and(|t| t.is_textual() || matches!(t, InputType::Number));
        // Description: aria-describedby, aria-description, else an unused title.
        let described: Vec<NodeId> = id_refs(dom, self.doc, el, "aria-describedby").unwrap_or_default();
        let description = if !described.is_empty() {
            Some(normalize(&described.iter().map(|d| text_of(dom, *d)).collect::<Vec<_>>().join(" ")))
        } else if let Some(d) = attr(dom, el, "aria-description") {
            Some(normalize(d))
        } else {
            attr(dom, el, "title").map(normalize).filter(|t| *t != n.name)
        };
        n.description = description.filter(|d| !d.is_empty());

        // Value.
        n.value = if text_field {
            let v = dom.value(el).unwrap_or_default();
            let v = if input_type == Some(InputType::Password) { "•".repeat(v.chars().count()) } else { v };
            (!v.is_empty()).then(|| json!({"type":"string","value":v}))
        } else if tag == Some("select") && role == "combobox" {
            let selected = dom.selected_options(el).unwrap_or_default();
            let option = selected
                .first()
                .copied()
                .or_else(|| dom.descendants(el).into_iter().flatten().find(|o| local(dom, *o) == Some("option")));
            option.map(|o| json!({"type":"string","value":normalize(&text_of(dom, o))}))
        } else if matches!(role, "slider" | "progressbar" | "meter" | "spinbutton" | "scrollbar") {
            num_attr(dom, el, "aria-valuenow")
                .or_else(|| match tag {
                    Some("input") => dom.value(el).ok().and_then(|v| v.trim().parse::<f64>().ok()),
                    Some("progress" | "meter") => num_attr(dom, el, "value"),
                    _ => None,
                })
                .map(|v| json!({"type":"number","value":number_value(v)}))
        } else if attr(dom, el, "contenteditable").is_some_and(|v| !v.eq_ignore_ascii_case("false")) {
            let v = text_of(dom, el);
            (!v.is_empty()).then(|| json!({"type":"string","value":v}))
        } else {
            None
        };

        let props = &mut n.props;
        // Live regions.
        let live_default = match role {
            "status" | "log" => Some("polite"),
            "alert" => Some("assertive"),
            "marquee" | "timer" => Some("off"),
            _ => None,
        };
        if let Some(live) = attr(dom, el, "aria-live").or(live_default) {
            props.push(("live", token(live)));
            let atomic = attr(dom, el, "aria-atomic")
                .map(|v| v.trim().eq_ignore_ascii_case("true"))
                .unwrap_or(matches!(role, "status" | "alert"));
            props.push(("atomic", boolean(atomic)));
            let relevant = attr(dom, el, "aria-relevant").unwrap_or("additions text");
            props.push(("relevant", Prop::Plain(json!({"type":"tokenList","value":relevant}))));
        }
        if dom.is_disabled(el).unwrap_or(false) || aria_true(dom, el, "aria-disabled") {
            props.push(("disabled", boolean(true)));
        }
        let form_control = matches!(tag, Some("input" | "button" | "select" | "textarea" | "output" | "fieldset"));
        if let Some(invalid) = attr(dom, el, "aria-invalid") {
            let v = invalid.trim().to_ascii_lowercase();
            let v = match v.as_str() {
                "" | "false" => "false",
                "grammar" | "spelling" => v.as_str(),
                _ => "true",
            };
            props.push(("invalid", token(v)));
        } else if form_control {
            props.push(("invalid", token("false")));
        }
        if self.focusable(el, tag) && !matches!(tag, Some("iframe" | "frame")) {
            props.push(("focusable", boolean_or_undefined(true)));
        }
        if self.focused == Some(el) {
            props.push(("focused", boolean_or_undefined(true)));
        }
        let readonly = attr(dom, el, "readonly").is_some() || aria_true(dom, el, "aria-readonly");
        if text_field {
            props.push(("editable", token("plaintext")));
        } else if let Some(e) = ctx.editable {
            props.push(("editable", token(e)));
        } else if let Some(e) = attr(dom, el, "contenteditable").filter(|v| !v.eq_ignore_ascii_case("false")) {
            props.push((
                "editable",
                token(if e.eq_ignore_ascii_case("plaintext-only") { "plaintext" } else { "richtext" }),
            ));
        }
        let disabled = dom.is_disabled(el).unwrap_or(false);
        if (text_field && !readonly || input_type == Some(InputType::Range)) && !disabled {
            props.push(("settable", boolean_or_undefined(true)));
        }
        let select_combobox = tag == Some("select") && role == "combobox";
        let popup = match attr(dom, el, "aria-haspopup").map(|v| v.trim().to_ascii_lowercase()) {
            Some(v) if v == "false" => None,
            Some(v) if v == "true" || v.is_empty() => Some("menu".to_owned()),
            Some(v) => Some(v),
            None if select_combobox => Some("menu".into()),
            None if role == "combobox" => Some("listbox".into()),
            None => None,
        };
        if let Some(p) = popup {
            props.push(("hasPopup", token(&p)));
        }
        let level = match (role, tag) {
            ("heading", _) => Some(num_attr(dom, el, "aria-level").map(|l| l as i64).unwrap_or_else(|| match tag {
                Some(h) if h.len() == 2 && h.starts_with('h') => h[1..].parse().unwrap_or(2),
                _ => 2,
            })),
            ("listitem", _) => {
                Some(num_attr(dom, el, "aria-level").map(|l| l as i64).unwrap_or(ctx.lists.max(1) as i64))
            }
            ("treeitem" | "row", _) => num_attr(dom, el, "aria-level").map(|l| l as i64),
            _ => None,
        };
        if let Some(level) = level {
            props.push(("level", Prop::Plain(json!({"type":"integer","value":level}))));
        }
        if matches!(role, "listbox" | "tablist" | "grid" | "tree" | "treegrid") {
            let multi = aria_true(dom, el, "aria-multiselectable")
                || tag == Some("select") && attr(dom, el, "multiple").is_some();
            props.push(("multiselectable", boolean(multi)));
        }
        let orientation =
            attr(dom, el, "aria-orientation").map(|o| o.trim().to_ascii_lowercase()).or_else(|| match role {
                "tablist" | "menubar" | "toolbar" | "separator" | "slider" => Some("horizontal".into()),
                "menu" | "listbox" | "tree" | "scrollbar" => Some("vertical".into()),
                _ => None,
            });
        if let Some(o) = orientation.filter(|o| matches!(o.as_str(), "horizontal" | "vertical")) {
            props.push(("orientation", token(&o)));
        }
        if role == "textbox" {
            let multiline = tag == Some("textarea") || tag != Some("input") && aria_true(dom, el, "aria-multiline");
            props.push(("multiline", boolean(multiline)));
            props.push(("readonly", boolean(readonly)));
        }
        if matches!(role, "columnheader" | "rowheader") {
            props.push(("readonly", boolean(readonly)));
        }
        if matches!(role, "textbox" | "spinbutton" | "listbox" | "columnheader" | "rowheader")
            || role == "combobox" && !select_combobox
        {
            let required = attr(dom, el, "required").is_some() || aria_true(dom, el, "aria-required");
            props.push(("required", boolean(required)));
        }
        if matches!(role, "slider" | "progressbar" | "meter" | "spinbutton" | "scrollbar") {
            let (min, max) = match (tag, input_type) {
                (Some("input"), Some(InputType::Range)) => {
                    (num_attr(dom, el, "min").unwrap_or(0.0), num_attr(dom, el, "max").unwrap_or(100.0))
                }
                (Some("progress"), _) => (0.0, num_attr(dom, el, "max").filter(|m| *m > 0.0).unwrap_or(1.0)),
                (Some("meter"), _) => {
                    (num_attr(dom, el, "min").unwrap_or(0.0), num_attr(dom, el, "max").unwrap_or(1.0))
                }
                (Some("input"), _) => {
                    (num_attr(dom, el, "min").unwrap_or(0.0), num_attr(dom, el, "max").unwrap_or(0.0))
                }
                _ => (
                    num_attr(dom, el, "aria-valuemin").unwrap_or(0.0),
                    num_attr(dom, el, "aria-valuemax").unwrap_or(if role == "spinbutton" { 0.0 } else { 100.0 }),
                ),
            };
            props.push(("valuemin", number(min)));
            props.push(("valuemax", number(max)));
            let text = attr(dom, el, "aria-valuetext").map(str::to_owned).unwrap_or_else(|| {
                if input_type == Some(InputType::Range) {
                    dom.value(el).unwrap_or_default()
                } else {
                    String::new()
                }
            });
            props.push(("valuetext", Prop::Plain(json!({"type":"string","value":text}))));
        }
        if matches!(role, "checkbox" | "radio" | "switch" | "menuitemcheckbox" | "menuitemradio") {
            let checked = if matches!(input_type, Some(InputType::Checkbox | InputType::Radio)) {
                if dom.checked(el).unwrap_or(false) {
                    "true"
                } else {
                    "false"
                }
            } else {
                match attr(dom, el, "aria-checked").map(|v| v.trim().to_ascii_lowercase()).as_deref() {
                    Some("true") => "true",
                    Some("mixed") => "mixed",
                    _ => "false",
                }
            };
            props.push(("checked", tristate(checked)));
        }
        let expanded = match attr(dom, el, "aria-expanded").map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("true") => Some(true),
            Some("false") => Some(false),
            _ if select_combobox => Some(false),
            _ if role == "DisclosureTriangle" => {
                Some(dom.parent_element(el).ok().flatten().is_some_and(|d| attr(dom, d, "open").is_some()))
            }
            _ => None,
        };
        if let Some(e) = expanded {
            props.push(("expanded", boolean_or_undefined(e)));
        }
        if matches!(role, "dialog" | "alertdialog") {
            props.push(("modal", boolean(aria_true(dom, el, "aria-modal"))));
        }
        if let Some(p) = attr(dom, el, "aria-pressed").map(|v| v.trim().to_ascii_lowercase()) {
            if matches!(p.as_str(), "true" | "false" | "mixed") {
                props.push(("pressed", tristate(&p)));
            }
        }
        if tag == Some("option") && role == "option" && self.focusable(el, tag) {
            props.push(("selected", boolean_or_undefined(dom.option_selected(el).unwrap_or(false))));
        } else if let Some(s) = attr(dom, el, "aria-selected").map(|v| v.trim().to_ascii_lowercase()) {
            if matches!(s.as_str(), "true" | "false") {
                props.push(("selected", boolean_or_undefined(s == "true")));
            }
        }
        if !described.is_empty() {
            let value = attr(dom, el, "aria-describedby").unwrap_or_default().to_owned();
            props.push(("describedby", Prop::IdRefs(value, described.iter().map(|d| (*d, String::new())).collect())));
        }
        if let Some(refs) = id_refs(dom, self.doc, el, "aria-labelledby").filter(|r| !r.is_empty()) {
            let value = attr(dom, el, "aria-labelledby").unwrap_or_default().to_owned();
            props.push(("labelledby", Prop::IdRefs(value, refs.iter().map(|r| (*r, String::new())).collect())));
        } else {
            let mut native = labels(dom, self.doc, el);
            let caption = match tag {
                Some("fieldset") => Some("legend"),
                Some("table") => Some("caption"),
                _ => None,
            };
            if let Some(caption) = caption {
                native.extend(dom.children(el).into_iter().flatten().find(|c| local(dom, *c) == Some(caption)));
            }
            if !native.is_empty() {
                let related = native.into_iter().map(|l| (l, normalize(&text_of(dom, l)))).collect();
                props.push(("labelledby", Prop::Nodes(related)));
            }
        }
        let url = match (role, tag) {
            ("link", Some("a" | "area")) => attr(dom, el, "href").and_then(|h| resolve(dom, el, h)),
            ("image", Some("img")) => attr(dom, el, "src").and_then(|s| resolve(dom, el, s)),
            _ => None,
        };
        if let Some(url) = url {
            props.push(("url", Prop::Plain(json!({"type":"string","value":url}))));
        }
    }
}

/// A document's nodes, its flattened elements (when tracked) and its frame id.
type BuiltTree = (Vec<AxNode>, Option<HashMap<NodeId, usize>>, Option<String>);

/// Serialization of built nodes with backend ids.
struct Out<'h> {
    host: &'h mut EngineHost,
    key: crate::target::PageKey,
    page: Rc<PageDom>,
    ids: HashMap<NodeId, Option<u64>>,
    /// The page's backend id table is full: stop asking (each failed
    /// `identify` sweeps the table).
    exhausted: bool,
}

/// The AX node id: the DOM node's slot and generation, stable while the
/// node lives and needing no table.
fn ax_id(node: NodeId) -> String {
    ((u64::from(node.generation()) << 32) | u64::from(node.index())).to_string()
}

impl<'h> Out<'h> {
    fn new(host: &'h mut EngineHost, key: crate::target::PageKey, page: Rc<PageDom>) -> Self {
        Out { host, key, page, ids: HashMap::new(), exhausted: false }
    }

    /// The DOM domain's backend node id, or `None` once the page's backend id
    /// budget is spent (the node is then reported without one).
    fn backend(&mut self, node: NodeId) -> CdpResult<Option<u64>> {
        if let Some(id) = self.ids.get(&node) {
            return Ok(*id);
        }
        let id = if self.exhausted {
            None
        } else {
            let state = self.host.pages.get_mut(&self.key).ok_or_else(|| CdpError::server("page is gone"))?;
            match state.dom_nodes.identify(&self.page, node, &mut self.host.next_backend_node) {
                Ok(id) => Some(id),
                Err(_) => {
                    self.exhausted = true;
                    None
                }
            }
        };
        self.ids.insert(node, id);
        Ok(id)
    }

    fn related_node(&mut self, node: NodeId) -> CdpResult<serde_json::Map<String, Value>> {
        let mut m = serde_json::Map::new();
        if let Some(id) = self.backend(node)? {
            m.insert("backendDOMNodeId".into(), json!(id));
        }
        Ok(m)
    }

    fn related(&mut self, nodes: &[(NodeId, String)], idrefs: bool) -> CdpResult<Vec<Value>> {
        let dom = self.page.dom.clone();
        nodes
            .iter()
            .map(|(n, text)| {
                let mut m = self.related_node(*n)?;
                if idrefs {
                    m.insert("idref".into(), json!(attr(&dom.borrow(), *n, "id").unwrap_or_default()));
                } else {
                    m.insert("text".into(), json!(text));
                }
                Ok(Value::Object(m))
            })
            .collect()
    }

    /// One node. `query` reports ignored nodes with their role and name, as
    /// Chromium's `queryAXTree` does; the tree commands report them as `none`.
    fn node(&mut self, tree: &[AxNode], idx: usize, frame: Option<&str>, query: bool) -> CdpResult<Value> {
        let n = &tree[idx];
        let backend = self.backend(n.node)?;
        let mut v = serde_json::Map::new();
        v.insert("nodeId".into(), json!(ax_id(n.node)));
        v.insert("ignored".into(), json!(n.ignored.is_some()));
        if let Some(why) = n.ignored {
            let reason = match why {
                Ignored::Uninteresting => json!({"name":"uninteresting","value":{"type":"boolean","value":true}}),
                Ignored::Presentational => json!({"name":"presentationalRole","value":{"type":"boolean","value":true}}),
                Ignored::AriaHiddenElement => {
                    json!({"name":"ariaHiddenElement","value":{"type":"boolean","value":true}})
                }
                Ignored::NotRendered => json!({"name":"notRendered","value":{"type":"boolean","value":true}}),
                Ignored::AriaHiddenSubtree(root) => {
                    json!({"name":"ariaHiddenSubtree","value":{"type":"idref","relatedNodes":[self.related_node(root)?]}})
                }
                Ignored::LabelFor(control) => {
                    json!({"name":"labelFor","value":{"type":"idref","relatedNodes":[self.related_node(control)?]}})
                }
            };
            v.insert("ignoredReasons".into(), json!([reason]));
        }
        let show = n.ignored.is_none() || query;
        let (ty, role) =
            if show { (if n.internal { "internalRole" } else { "role" }, n.role()) } else { ("role", "none") };
        v.insert("role".into(), json!({"type":ty,"value":role}));
        if show {
            v.insert("name".into(), json!({"type":"computedString","value":n.name}));
        }
        if n.ignored.is_none() {
            if let Some(d) = &n.description {
                v.insert("description".into(), json!({"type":"computedString","value":d}));
            }
            if let Some(value) = &n.value {
                v.insert("value".into(), value.clone());
            }
            let mut props = Vec::with_capacity(n.props.len());
            for (name, prop) in &n.props {
                let value = match prop {
                    Prop::Plain(value) => value.clone(),
                    Prop::Nodes(nodes) => json!({"type":"nodeList","relatedNodes":self.related(nodes, false)?}),
                    Prop::IdRefs(attr, nodes) => {
                        json!({"type":"idrefList","value":attr,"relatedNodes":self.related(nodes, true)?})
                    }
                };
                props.push(json!({"name":name,"value":value}));
            }
            v.insert("properties".into(), json!(props));
        }
        if let Some(p) = n.parent {
            v.insert("parentId".into(), json!(ax_id(tree[p].node)));
        }
        let mut children = Vec::with_capacity(n.children.len());
        for c in &n.children {
            children.push(json!(ax_id(tree[*c].node)));
        }
        v.insert("childIds".into(), json!(children));
        if let Some(id) = backend {
            v.insert("backendDOMNodeId".into(), json!(id));
        }
        if let Some(frame) = frame.filter(|_| n.parent.is_none()) {
            v.insert("frameId".into(), json!(frame));
        }
        Ok(Value::Object(v))
    }
}

/// Chromium's `getFullAXTree` order and depth: breadth-first over unignored
/// nodes, each expanded node adding its children and, through ignored
/// children, theirs. The root is always expanded; another unignored node at
/// unignored depth `d` (root = 1) only while `d < depth`.
fn walk_to_depth(tree: &[AxNode], depth: Option<u32>) -> Vec<usize> {
    let mut out = vec![0];
    let mut queue = std::collections::VecDeque::from([(0usize, 1u32)]);
    while let Some((idx, d)) = queue.pop_front() {
        let mut stack: Vec<usize> = tree[idx].children.iter().rev().copied().collect();
        while let Some(c) = stack.pop() {
            out.push(c);
            if tree[c].ignored.is_some() {
                stack.extend(tree[c].children.iter().rev().copied());
            } else if depth.is_none_or(|max| d < max) {
                queue.push_back((c, d + 1));
            }
        }
    }
    out
}

impl EngineHost {
    pub(super) fn accessibility_domain(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        match action {
            "enable" | "disable" => Ok(Some(json!({}))),
            "getFullAXTree" => self.full_ax_tree(cmd).map(Some),
            "getPartialAXTree" => self.partial_ax_tree(cmd).map(Some),
            "queryAXTree" => self.query_ax_tree(cmd).map(Some),
            _ => Err(CdpError::unsupported(&cmd.command.method)),
        }
    }

    /// The tree of one document, plus its frame id.
    fn build_ax(&self, cmd: &PageCommand<'_, '_>, page: &Rc<PageDom>, track: bool) -> CdpResult<BuiltTree> {
        let (frame_id, frame_url) = self
            .with_frame_of_document(cmd.page, page, |f| (f.id.0.clone(), f.url.clone()))
            .ok_or_else(|| CdpError::server("Could not find node with given id"))?;
        let title = page.document_title();
        let dom = page.dom.borrow();
        let layout = page.layout();
        let mut b = Builder::new(&dom, &layout, page.document(), track);
        b.build(title, Some(frame_url).filter(|u| !u.is_empty()));
        if b.over_budget {
            return Err(CdpError::server("Accessibility tree exceeds node budget"));
        }
        Ok((b.nodes, b.flattened, Some(frame_id)))
    }

    fn full_ax_tree(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<Value> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct P {
            depth: Option<i64>,
            frame_id: Option<String>,
        }
        let p: P = cmd.command.params()?;
        let pid = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        let frame = match &p.frame_id {
            Some(id) => self.thread.browser.frame(pid, &lp_browser::FrameId(id.clone())),
            None => self.thread.browser.page(pid).map(|p| p.main_frame.clone()),
        }
        .ok_or_else(|| CdpError::invalid_params("Frame with the given frameId is not found."))?;
        let page = frame.page_dom.ok_or_else(|| CdpError::invalid_params("No document for given frame found"))?;
        let (tree, _, frame) = self.build_ax(cmd, &page, false)?;
        let order = walk_to_depth(&tree, p.depth.map(|d| d.clamp(0, u32::MAX as i64) as u32));
        let mut out = Out::new(self, cmd.page, page);
        let nodes =
            order.into_iter().map(|i| out.node(&tree, i, frame.as_deref(), false)).collect::<CdpResult<Vec<_>>>()?;
        Ok(json!({ "nodes": nodes }))
    }

    /// The tree node for a DOM node: its own, or (for a node that is not in
    /// the tree) a synthesized ignored one under the nearest included
    /// ancestor.
    fn ax_target(
        &self,
        page: &Rc<PageDom>,
        tree: &mut Vec<AxNode>,
        flattened: &HashMap<NodeId, usize>,
        node: NodeId,
    ) -> usize {
        if let Some(i) = tree.iter().position(|n| n.node == node) {
            return i;
        }
        let dom = page.dom.borrow();
        let rendered =
            flattened.contains_key(&node) && !page.layout().style().is_hidden(&dom, node, Default::default());
        let mut cur = dom.parent(node).ok().flatten();
        let parent = loop {
            let Some(c) = cur else { break 0 };
            if let Some(i) = tree.iter().position(|n| n.node == c) {
                break i;
            }
            cur = dom.parent(c).ok().flatten();
        };
        let mut n = blank(node, "none");
        n.parent = Some(parent);
        n.ignored = Some(if rendered { Ignored::Uninteresting } else { Ignored::NotRendered });
        // Its content attaches to the parent: list what came from it.
        if rendered {
            n.children = tree[parent]
                .children
                .iter()
                .copied()
                .filter(|c| dom.contains(node, tree[*c].node).unwrap_or(false))
                .collect();
        }
        tree.push(n);
        tree.len() - 1
    }

    fn partial_ax_tree(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<Value> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct P {
            fetch_relatives: Option<bool>,
        }
        let p: P = cmd.command.params()?;
        let target: NodeParams = cmd.command.params()?;
        let (page, node) = self.resolve_dom_node(cmd, &target)?;
        let (mut tree, flattened, frame) = self.build_ax(cmd, &page, true)?;
        let flattened = flattened.unwrap_or_default();
        let idx = self.ax_target(&page, &mut tree, &flattened, node);
        let mut order = vec![idx];
        if p.fetch_relatives.unwrap_or(true) {
            let mut stack: Vec<usize> = tree[idx].children.iter().rev().copied().collect();
            while let Some(c) = stack.pop() {
                order.push(c);
                if tree[c].ignored.is_some() {
                    stack.extend(tree[c].children.iter().rev().copied());
                }
            }
            let mut cur = tree[idx].parent;
            while let Some(a) = cur {
                order.push(a);
                cur = tree[a].parent;
            }
        }
        let mut out = Out::new(self, cmd.page, page);
        let nodes =
            order.into_iter().map(|i| out.node(&tree, i, frame.as_deref(), false)).collect::<CdpResult<Vec<_>>>()?;
        Ok(json!({ "nodes": nodes }))
    }

    fn query_ax_tree(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<Value> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct P {
            accessible_name: Option<String>,
            role: Option<String>,
            node_id: Option<u64>,
            backend_node_id: Option<u64>,
            object_id: Option<String>,
        }
        let p: P = cmd.command.params()?;
        if p.node_id.is_none() && p.backend_node_id.is_none() && p.object_id.is_none() {
            return Err(CdpError::invalid_params("Either nodeId, backendNodeId or objectId must be specified"));
        }
        let target: NodeParams = cmd.command.params()?;
        let (page, root) = self.resolve_dom_node(cmd, &target)?;
        let (tree, _, frame) = self.build_ax(cmd, &page, false)?;
        let want_name = p.accessible_name.as_deref().map(normalize);
        let matches: Vec<usize> = {
            let dom = page.dom.borrow();
            // Tree order is document order: a pre-order walk from the root.
            let mut order = Vec::with_capacity(tree.len());
            let mut stack = vec![0usize];
            while let Some(i) = stack.pop() {
                order.push(i);
                stack.extend(tree[i].children.iter().rev().copied());
            }
            order
                .into_iter()
                .filter(|&i| {
                    let n = &tree[i];
                    (n.node == root || dom.contains(root, n.node).unwrap_or(false))
                        && p.role.as_deref().is_none_or(|r| r == n.role())
                        && want_name.as_deref().is_none_or(|name| *name == normalize(&n.name))
                })
                .collect()
        };
        let mut out = Out::new(self, cmd.page, page);
        let nodes =
            matches.into_iter().map(|i| out.node(&tree, i, frame.as_deref(), true)).collect::<CdpResult<Vec<_>>>()?;
        Ok(json!({ "nodes": nodes }))
    }
}
