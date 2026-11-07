//! Document serialization for rendering (screenshots): the current tree as
//! another engine can lay out and paint it without running script, either
//! as HTML ([`Dom::render_html`]) or as a keyed node list
//! ([`Dom::render_tree`]) that a renderer can diff against the previous one.
//!
//! Differences from `outerHTML`:
//! - form state is written into markup: an input's current value and
//!   checkedness, a textarea's value, each option's selectedness;
//! - `<style>` text and `<link rel=stylesheet>` URLs come from the caller
//!   (the style engine holds the live text, CSSOM edits included);
//! - `<script>`, `<noscript>` (scripting is on), `<template>`, comments and
//!   processing instructions are left out, and so is an `<iframe>`'s `src`
//!   (frames render as empty boxes);
//! - a shadow host is serialized with its shadow tree in place of its
//!   children, each `<slot>` replaced by the light children assigned to it
//!   (its fallback content when none is): shadow styles become global;
//! - one element can be marked with an attribute (the clip target).

use std::borrow::Cow;

use markup5ever::{ns, QualName};

use crate::dom::{Change, Dom};
use crate::error::DomResult;
use crate::forms::InputType;
use crate::node::{NodeId, NodeKind, NIL};
use crate::serialize::{escape, is_void};

/// Namespace of the `state` attribute render trees carry on form controls
/// (see `Dom::render_states`): page CSS cannot match it (`[state]` matches
/// the null namespace only), the renderer maps it to pseudo-classes.
pub const STATE_NS: &str = "urn:x-lightningpanda:state";

/// What the caller supplies.
pub struct RenderHtml<'a> {
    /// Sheet text replacing a `<style>` element's own text (CSSOM edits);
    /// `None`: its text content.
    pub style_text: &'a dyn Fn(NodeId) -> Option<String>,
    /// Absolute URL to keep on a `<link rel=stylesheet>`; `None` drops the
    /// link (sheet not loaded).
    pub stylesheet_href: &'a dyn Fn(NodeId) -> Option<String>,
    /// Element to mark with `mark.1` (a valueless attribute).
    pub mark: Option<(NodeId, &'a str)>,
    /// The constructed sheets (id and text) a document or shadow root adopts
    /// (`adoptedStyleSheets`), in order; `None` when nothing adopts any.
    /// They render as `<style>` elements after the document element's
    /// children and after a shadow tree's, each text once: shadow styles are
    /// global in the render tree, and a sheet shared by many shadow roots
    /// (a component's) is one sheet to the renderer.
    pub adopted: Option<&'a dyn Fn(NodeId) -> Vec<(u64, std::rc::Rc<str>)>>,
}

/// Changes to a [`RenderTree`] ([`Dom::render_delta`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderDelta {
    pub ops: Vec<DeltaOp>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DeltaOp {
    /// The node's own content (an element's name and attributes, a text's
    /// data) is now `kind`; its children are unchanged.
    Node { key: u64, kind: RenderNodeKind },
    /// The render children of `parent` (`None`: the top level) are now
    /// `kids`; `nodes` are the subtrees of the inserted ones (parents index
    /// into `nodes`, [`TOP`] for a child of `parent`).
    Children { parent: Option<u64>, kids: Vec<u64>, nodes: Vec<RenderNode> },
}

/// The document as [`Dom::render_html`] would serialize it, as nodes in
/// document order. Keys identify nodes across snapshots of the same `Dom`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderTree {
    pub nodes: Vec<RenderNode>,
    /// Key of the marked element (`RenderHtml::mark`; its attribute is not
    /// written into the tree).
    pub mark: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderNode {
    /// The node's [`render_key`]. Text a renderer gets in place of an
    /// element's children (a `<style>`'s sheet text, a textarea's value) has
    /// its element's key with [`SYNTHETIC_TEXT`] set.
    pub key: u64,
    /// Index of the parent in `RenderTree::nodes`, [`TOP`] at the top level.
    pub parent: u32,
    pub kind: RenderNodeKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RenderNodeKind {
    Element { name: QualName, attrs: Vec<(QualName, String)> },
    Text(String),
}

/// `RenderNode::parent` of a top-level node.
pub const TOP: u32 = u32::MAX;
/// Set in the key of text that stands for an element's children.
pub const SYNTHETIC_TEXT: u64 = 1 << 63;

/// A node's identity in render trees: stable while the node lives, never
/// reused for another node of the same `Dom`.
pub fn render_key(id: NodeId) -> u64 {
    ((id.generation() as u64) << 32) | id.index() as u64
}

/// Tree scope of a node being serialized: index into the shadow hosts
/// entered so far, `NONE` for the document.
type Scope = usize;
const NONE: Scope = usize::MAX;

enum Step {
    Enter(u32, Scope),
    /// An adopted sheet, as a `<style>` element of that key.
    Adopted(u64, std::rc::Rc<str>),
    Close,
    /// Text standing for element `.0`'s children.
    Synthetic(u32, String, TextKind),
}

#[derive(Clone, Copy, PartialEq)]
enum TextKind {
    /// A text node: escaped in HTML.
    Node,
    /// A `<style>`'s sheet text: raw in HTML.
    Style,
    /// A textarea's value: escaped, after the newline a parser drops.
    Textarea,
}

/// An attribute as written: `valueless` ones are serialized as a bare name.
struct RAttr<'a> {
    name: Cow<'a, QualName>,
    value: Cow<'a, str>,
    valueless: bool,
}

/// What the walk produces.
trait Sink {
    fn doctype(&mut self, name: &str);
    /// `tag`: the serialized tag name. `void`: no children and no `close`.
    fn open(&mut self, key: u64, tag: &str, name: &QualName, attrs: &[RAttr<'_>], marked: bool, void: bool);
    fn close(&mut self, tag: &str);
    fn text(&mut self, key: u64, text: &str, kind: TextKind);
}

struct HtmlSink<'a> {
    out: String,
    mark: Option<&'a str>,
    scroll: &'a [(u64, (f64, f64))],
}

impl Sink for HtmlSink<'_> {
    fn doctype(&mut self, name: &str) {
        self.out.push_str("<!DOCTYPE ");
        self.out.push_str(name);
        self.out.push('>');
    }
    fn open(&mut self, key: u64, tag: &str, _name: &QualName, attrs: &[RAttr<'_>], marked: bool, _void: bool) {
        let out = &mut self.out;
        out.push('<');
        out.push_str(tag);
        // The state attribute is for keyed trees: markup would lose its namespace.
        for a in attrs.iter().filter(|a| {
            &*a.name.ns != STATE_NS && !matches!(&*a.name.local, "data-lp-scroll-left" | "data-lp-scroll-top")
        }) {
            out.push(' ');
            match &a.name.prefix {
                Some(p) if !p.is_empty() => {
                    out.push_str(p);
                    out.push(':');
                    out.push_str(&a.name.local);
                }
                _ => out.push_str(&a.name.local),
            }
            if !a.valueless {
                out.push_str("=\"");
                escape(out, &a.value, true);
                out.push('"');
            }
        }
        if let (true, Some(name)) = (marked, self.mark) {
            out.push(' ');
            out.push_str(name);
        }
        if let Some((_, (x, y))) = self.scroll.iter().find(|(id, _)| *id == key) {
            use std::fmt::Write;
            let _ = write!(out, " data-lp-scroll-left=\"{x}\" data-lp-scroll-top=\"{y}\"");
        }
        out.push('>');
    }
    fn close(&mut self, tag: &str) {
        self.out.push_str("</");
        self.out.push_str(tag);
        self.out.push('>');
    }
    fn text(&mut self, _key: u64, text: &str, kind: TextKind) {
        match kind {
            TextKind::Node => escape(&mut self.out, text, false),
            TextKind::Style => self.out.push_str(text),
            TextKind::Textarea => {
                // A leading newline right after <textarea> is dropped by parsers.
                self.out.push('\n');
                escape(&mut self.out, text, false);
            }
        }
    }
}

#[derive(Default)]
struct TreeSink {
    tree: RenderTree,
    /// Open elements (indices into `tree.nodes`).
    open: Vec<u32>,
}

impl TreeSink {
    fn parent(&self) -> u32 {
        self.open.last().copied().unwrap_or(TOP)
    }
}

impl Sink for TreeSink {
    fn doctype(&mut self, _name: &str) {}
    fn open(&mut self, key: u64, _tag: &str, name: &QualName, attrs: &[RAttr<'_>], marked: bool, void: bool) {
        let attrs = attrs
            .iter()
            .map(|a| (a.name.clone().into_owned(), if a.valueless { String::new() } else { a.value.to_string() }))
            .collect();
        let parent = self.parent();
        self.tree.nodes.push(RenderNode { key, parent, kind: RenderNodeKind::Element { name: name.clone(), attrs } });
        if marked {
            self.tree.mark = Some(key);
        }
        if !void {
            self.open.push(self.tree.nodes.len() as u32 - 1);
        }
    }
    fn close(&mut self, _tag: &str) {
        self.open.pop();
    }
    fn text(&mut self, key: u64, text: &str, _kind: TextKind) {
        let parent = self.parent();
        self.tree.nodes.push(RenderNode { key, parent, kind: RenderNodeKind::Text(text.to_owned()) });
    }
}

impl Dom {
    /// Serializes document `doc` for rendering (see the module docs).
    pub fn render_html(&self, doc: NodeId, opts: &RenderHtml<'_>) -> DomResult<String> {
        self.render_html_scrolled(doc, opts, &[])
    }

    /// Writes scroll offsets into the render copy without modifying the live DOM.
    pub fn render_html_scrolled(
        &self,
        doc: NodeId,
        opts: &RenderHtml<'_>,
        scroll: &[(u64, (f64, f64))],
    ) -> DomResult<String> {
        let mut sink = HtmlSink { out: String::with_capacity(64 * 1024), mark: opts.mark.map(|m| m.1), scroll };
        self.render_walk(doc, opts, &mut sink)?;
        Ok(sink.out)
    }

    /// The nodes [`Dom::render_html`] would serialize, keyed (see
    /// [`RenderTree`]).
    pub fn render_tree(&self, doc: NodeId, opts: &RenderHtml<'_>) -> DomResult<RenderTree> {
        let mut sink = TreeSink::default();
        self.render_walk(doc, opts, &mut sink)?;
        Ok(sink.tree)
    }

    fn render_walk(&self, doc: NodeId, opts: &RenderHtml<'_>, sink: &mut dyn Sink) -> DomResult<()> {
        let d = self.check(doc)?;
        let root = self.child_idx_iter(d).find(|&c| self.n(c).is_element()).unwrap_or(NIL);
        self.render_walk_from(self.child_idx_iter(d).collect(), opts, sink, root);
        Ok(())
    }

    /// Pushes the `<style>` steps of the sheets `scope` adopts (rendered
    /// after what `stack` holds above them), but texts already `emitted`.
    fn push_adopted(
        &self,
        stack: &mut Vec<Step>,
        opts: &RenderHtml<'_>,
        scope: u32,
        emitted: &mut rustc_hash::FxHashSet<std::rc::Rc<str>>,
    ) {
        let Some(adopted) = opts.adopted else { return };
        let mut sheets = Vec::new();
        for (_, text) in adopted(self.id_of(scope)) {
            if !text.is_empty() && emitted.insert(text.clone()) {
                sheets.push((adopted_key(emitted.len()), text));
            }
        }
        for (key, text) in sheets.into_iter().rev() {
            stack.push(Step::Adopted(key, text));
        }
    }

    /// The render walk over the subtrees of `roots` (in order), outside any
    /// shadow tree. `root` is the document element (whose rendered children
    /// end with the document's adopted sheets), `NIL` when not walked.
    fn render_walk_from(&self, roots: Vec<u32>, opts: &RenderHtml<'_>, sink: &mut dyn Sink, root: u32) {
        let mark = opts.mark.and_then(|(id, _)| self.check(id).ok());
        // Shadow hosts entered: (host, the host's own scope).
        let mut hosts: Vec<(u32, Scope)> = Vec::new();
        // Tags of the open elements, for `close`.
        let mut tags: Vec<String> = Vec::new();
        // Adopted sheet texts rendered so far.
        let mut emitted = rustc_hash::FxHashSet::default();
        let mut stack = Vec::new();
        push_children(&mut stack, roots, NONE);
        while let Some(step) = stack.pop() {
            let (n, scope) = match step {
                Step::Synthetic(e, text, kind) => {
                    sink.text(render_key(self.id_of(e)) | SYNTHETIC_TEXT, &text, kind);
                    continue;
                }
                Step::Close => {
                    let tag = tags.pop().unwrap_or_default();
                    sink.close(&tag);
                    continue;
                }
                Step::Adopted(key, text) => {
                    let name = QualName::new(None, ns!(html), markup5ever::local_name!("style"));
                    sink.open(key, "style", &name, &[], false, false);
                    sink.text(key | SYNTHETIC_TEXT, &text, TextKind::Style);
                    sink.close("style");
                    continue;
                }
                Step::Enter(n, scope) => (n, scope),
            };
            match &self.n(n).kind {
                NodeKind::Doctype(dt) => sink.doctype(&dt.name),
                NodeKind::Text(s) | NodeKind::CData(s) => sink.text(render_key(self.id_of(n)), s, TextKind::Node),
                NodeKind::Comment(_) | NodeKind::ProcessingInstruction(_) | NodeKind::Attr(_) => {}
                NodeKind::Document(_) | NodeKind::Fragment(_) => {
                    push_children(&mut stack, self.child_idx_iter(n).collect(), scope);
                }
                NodeKind::Element(e) => {
                    let html = e.is_html();
                    let local: &str = &e.name.local;
                    if html && matches!(local, "script" | "noscript" | "template") {
                        continue;
                    }
                    if e.name.ns == ns!(svg) && local == "script" {
                        continue;
                    }
                    // A slot in a shadow tree: the host's light children
                    // assigned to it, else its fallback content.
                    if html && local == "slot" && scope != NONE {
                        let (host, host_scope) = hosts[scope];
                        let name = e.attr("name").unwrap_or("");
                        let assigned = self.slotted(host, name);
                        if assigned.is_empty() {
                            push_children(&mut stack, self.child_idx_iter(n).collect(), scope);
                        } else {
                            push_children(&mut stack, assigned, host_scope);
                        }
                        continue;
                    }
                    let id = self.id_of(n);
                    let is_sheet_link = html
                        && local == "link"
                        && e.attr("rel")
                            .is_some_and(|r| r.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case("stylesheet")));
                    let href = if is_sheet_link {
                        match (opts.stylesheet_href)(id) {
                            Some(href) => Some(href),
                            None => continue,
                        }
                    } else {
                        None
                    };
                    let input_type = (html && local == "input").then(|| self.input_type_idx(n));
                    let mut attrs: Vec<RAttr<'_>> = Vec::with_capacity(e.attrs.len() + 1);
                    for a in &e.attrs {
                        let name: &str = &a.name.local;
                        // The state namespace is the snapshot's own: a page's
                        // `setAttributeNS` in it would fake pseudo-classes or
                        // the top layer for the renderer.
                        if &*a.name.ns == STATE_NS {
                            continue;
                        }
                        if a.name.ns == ns!() {
                            let skip = match (local, name) {
                                ("input", "value" | "checked") => input_type.is_some(),
                                ("option", "selected") => html,
                                ("link", "href") => href.is_some(),
                                ("iframe" | "frame", "src" | "srcdoc") => html,
                                _ => false,
                            } || (html && name.starts_with("on"));
                            if skip {
                                continue;
                            }
                        }
                        attrs.push(RAttr {
                            name: Cow::Borrowed(&a.name),
                            value: Cow::Borrowed(&a.value),
                            valueless: false,
                        });
                    }
                    let plain = |local: &str| Cow::Owned(QualName::new(None, ns!(), local.into()));
                    if let Some(href) = href {
                        attrs.push(RAttr { name: plain("href"), value: Cow::Owned(href), valueless: false });
                    }
                    if let Some(ty) = input_type {
                        if matches!(ty, InputType::Checkbox | InputType::Radio) {
                            if self.checked(id).unwrap_or(false) {
                                attrs.push(RAttr { name: plain("checked"), value: Cow::Borrowed(""), valueless: true });
                            }
                            // Their value is not displayed.
                            if let Some(v) = e.attr("value") {
                                attrs.push(RAttr { name: plain("value"), value: Cow::Borrowed(v), valueless: false });
                            }
                        } else if ty != InputType::File {
                            let v = self.value(id).unwrap_or_default();
                            let v = if ty == InputType::Password { "•".repeat(v.chars().count()) } else { v };
                            if !v.is_empty() {
                                attrs.push(RAttr { name: plain("value"), value: Cow::Owned(v), valueless: false });
                            }
                        }
                    }
                    if html && local == "option" && self.option_selected(id).unwrap_or(false) {
                        attrs.push(RAttr { name: plain("selected"), value: Cow::Borrowed(""), valueless: true });
                    }
                    if let Some(states) = self.render_states(n) {
                        attrs.push(RAttr {
                            name: Cow::Owned(QualName::new(None, STATE_NS.into(), "state".into())),
                            value: Cow::Owned(states),
                            valueless: false,
                        });
                    }
                    if let Some(states) = self.render_custom_states(n) {
                        attrs.push(RAttr {
                            name: Cow::Owned(QualName::new(None, STATE_NS.into(), "custom-states".into())),
                            value: Cow::Owned(states),
                            valueless: false,
                        });
                    }
                    let tag = self.serialized_tag(n);
                    let void = html && is_void(local);
                    sink.open(render_key(id), &tag, &e.name, &attrs, mark == Some(n), void);
                    if void {
                        continue;
                    }
                    tags.push(tag);
                    stack.push(Step::Close);
                    if html && local == "style" {
                        let text = (opts.style_text)(id).unwrap_or_else(|| self.child_text_content(id).into_owned());
                        stack.push(Step::Synthetic(n, text, TextKind::Style));
                        continue;
                    }
                    if html && local == "textarea" {
                        stack.push(Step::Synthetic(n, self.value(id).unwrap_or_default(), TextKind::Textarea));
                        continue;
                    }
                    if html && matches!(local, "iframe" | "frame") {
                        continue;
                    }
                    if e.shadow_root != NIL {
                        hosts.push((n, scope));
                        self.push_adopted(&mut stack, opts, e.shadow_root, &mut emitted);
                        push_children(&mut stack, self.child_idx_iter(e.shadow_root).collect(), hosts.len() - 1);
                    } else {
                        if n == root && self.n(n).parent != NIL {
                            self.push_adopted(&mut stack, opts, self.n(n).parent, &mut emitted);
                        }
                        push_children(&mut stack, self.child_idx_iter(n).collect(), scope);
                    }
                }
            }
        }
    }

    /// What changed in [`Dom::render_tree`] of `doc` since DOM version
    /// `since`, from the change journal: element attributes and text that
    /// changed in place, and the new render children of every node whose
    /// children changed, with the subtrees of those inserted. `None` when the
    /// journal does not cover `since` or a change needs the whole tree
    /// (unknown changes, shadow trees, `<textarea>`/`<link>`/frames, whose
    /// render content is not their DOM content). A change of a `<style>`'s
    /// content gives its sheet text.
    pub fn render_delta(&self, doc: NodeId, since: u64, opts: &RenderHtml<'_>) -> Option<RenderDelta> {
        let d = self.check(doc).ok()?;
        if self.shadow_root_count() != 0 || opts.mark.is_some() || opts.adopted.is_some() {
            return None;
        }
        let mut changed: Vec<u32> = Vec::new();
        // Attribute-only textarea patches do not replace the synthetic value
        // child. A state change in the same interval must keep the fallback.
        let mut textarea_attributes = rustc_hash::FxHashMap::<u32, bool>::default();
        let is_textarea =
            |i| self.slot_live(i) && self.n(i).element().is_some_and(|e| e.is_html() && &*e.name.local == "textarea");
        let mut parents: Vec<u32> = Vec::new();
        let mut inserted: rustc_hash::FxHashSet<u32> = rustc_hash::FxHashSet::default();
        for c in self.changes_since(since)? {
            match c {
                Change::Attr(i, _, _) => {
                    if is_textarea(i) {
                        textarea_attributes.entry(i).or_insert(true);
                    }
                    changed.push(i);
                }
                Change::State(i) | Change::Text(i) => {
                    if is_textarea(i) {
                        textarea_attributes.insert(i, false);
                    }
                    changed.push(i);
                }
                Change::Inserted(p, c) => {
                    parents.push(p);
                    inserted.insert(c);
                }
                Change::Removed(p, n, _) => {
                    parents.push(p);
                    // A removed radio leaves its group (the radios still in
                    // the document revalidate).
                    if self.slot_live(n) && self.slot_live(p) {
                        let is_radio = |k: u32| {
                            self.n(k).element().is_some_and(|e| {
                                e.is_html()
                                    && &*e.name.local == "input"
                                    && e.attr("type").is_some_and(|t| t.eq_ignore_ascii_case("radio"))
                            })
                        };
                        // The removed node or anything inside it.
                        let mut radio = false;
                        let mut k = n;
                        while k != NIL && !radio {
                            radio = is_radio(k);
                            k = self.following_in(k, n);
                        }
                        if radio {
                            let mut first = Vec::new();
                            let root = self.root_idx(p);
                            let mut k = root;
                            while k != NIL {
                                if is_radio(k) {
                                    first.push(k);
                                }
                                k = self.following_in(k, root);
                            }
                            changed.extend(first);
                        }
                    }
                }
                Change::Pseudo => {}
                Change::Unknown(_) => return None,
            }
        }
        // Where a node is rendered: connected to `doc`, outside elements whose
        // children are not rendered. `Err`: in an element rendered specially.
        let rendered = |mut i: u32| -> Result<bool, ()> {
            if !self.slot_live(i) {
                return Ok(false);
            }
            loop {
                if i == d {
                    return Ok(true);
                }
                if let NodeKind::Element(e) = &self.n(i).kind {
                    let local: &str = &e.name.local;
                    if e.is_html() && matches!(local, "script" | "noscript" | "template" | "iframe" | "frame") {
                        return Ok(false);
                    }
                    if e.is_html() && matches!(local, "style" | "textarea" | "link" | "slot") {
                        return Err(());
                    }
                    if e.name.ns == ns!(svg) && local == "script" {
                        return Ok(false);
                    }
                }
                let parent = self.n(i).parent;
                if parent == NIL {
                    return Ok(false);
                }
                i = parent;
            }
        };
        // The `<style>` element `i` is or is in: its render content is its
        // sheet's text, its child text content, which only a change of its
        // own children replaces (nothing deeper in it is rendered).
        let style_owner = |mut i: u32| -> Option<u32> {
            while i != NIL && i != d {
                if let NodeKind::Element(e) = &self.n(i).kind {
                    if e.is_html() && &*e.name.local == "style" {
                        return Some(i);
                    }
                }
                i = self.n(i).parent;
            }
            None
        };
        let mut styles: Vec<u32> = Vec::new();
        // Nodes inside a subtree that is serialized whole (an inserted one).
        let covered = |mut i: u32| {
            i = self.n(i).parent;
            while i != NIL && i != d {
                if inserted.contains(&i) {
                    return true;
                }
                i = self.n(i).parent;
            }
            false
        };
        let mut ops = Vec::new();
        let mut seen = rustc_hash::FxHashSet::default();
        parents.sort_unstable();
        parents.dedup();
        for &p in &parents {
            match rendered(p) {
                Err(()) => {
                    // A child of the `<style>` came or went; deeper in it,
                    // nothing rendered changed.
                    let owner = style_owner(p)?;
                    if owner == p {
                        styles.push(owner);
                    }
                    continue;
                }
                Ok(false) => continue,
                Ok(true) => {}
            }
            // An inserted node's children are in its own serialized subtree.
            if covered(p) || inserted.contains(&p) || !seen.insert(p) {
                continue;
            }
            let kids: Vec<u32> = self.child_idx_iter(p).filter(|&k| self.render_child(k)).collect();
            let fresh: Vec<u32> = kids.iter().copied().filter(|k| inserted.contains(k)).collect();
            let mut sink = TreeSink::default();
            self.render_walk_from(fresh, opts, &mut sink, NIL);
            ops.push(DeltaOp::Children {
                parent: (p != d).then(|| render_key(self.id_of(p))),
                kids: kids.iter().map(|&k| render_key(self.id_of(k))).collect(),
                nodes: sink.tree.nodes,
            });
        }
        // Validity is not local: a form or fieldset follows its controls,
        // and a required radio group spans its radios.
        let mut dependents = Vec::new();
        for &i in changed.iter().chain(parents.iter()) {
            self.state_dependents(i, &mut dependents);
        }
        changed.extend(dependents);
        changed.sort_unstable();
        changed.dedup();
        for i in changed {
            // Removed since: nothing to render.
            if !self.slot_live(i) {
                continue;
            }
            // A `<style>`'s own attributes: the element (its text stays its
            // sheet's).
            let is_style = style_owner(i) == Some(i);
            // A frame's own attributes (its children are not rendered, it is).
            let is_frame = matches!(&self.n(i).kind, NodeKind::Element(e)
                if e.is_html() && matches!(&*e.name.local, "iframe" | "frame"));
            let own_attributes = is_style || is_frame || textarea_attributes.get(&i) == Some(&true);
            let place =
                if own_attributes && self.n(i).parent != NIL { rendered(self.n(i).parent) } else { rendered(i) };
            match place {
                // Inside a `<style>`: its sheet text.
                Err(()) => match style_owner(i).filter(|&owner| owner != i) {
                    Some(owner) => {
                        // A child's data: the sheet text. Deeper: nothing
                        // rendered.
                        if self.n(i).parent == owner {
                            styles.push(owner);
                        }
                        continue;
                    }
                    None if covered(i) || inserted.contains(&i) => continue,
                    None => return None,
                },
                Ok(false) => continue,
                Ok(true) => {}
            }
            if covered(i) || inserted.contains(&i) {
                continue;
            }
            // The node alone (its subtree walk, first node).
            let mut sink = TreeSink::default();
            self.render_walk_from(vec![i], opts, &mut sink, NIL);
            let Some(first) = sink.tree.nodes.into_iter().next() else { continue };
            if first.key != render_key(self.id_of(i)) {
                return None;
            }
            ops.push(DeltaOp::Node { key: first.key, kind: first.kind });
            if is_style {
                styles.push(i);
            }
        }
        // A `<style>` whose content changed: its sheet text (unless the
        // element itself is new, and serialized whole above).
        styles.sort_unstable();
        styles.dedup();
        for owner in styles {
            if covered(owner) || inserted.contains(&owner) {
                continue;
            }
            let parent = self.n(owner).parent;
            if parent == NIL {
                continue;
            }
            match rendered(parent) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(()) => return None,
            }
            let id = self.id_of(owner);
            let text = (opts.style_text)(id).unwrap_or_else(|| self.child_text_content(id).into_owned());
            ops.push(DeltaOp::Node { key: render_key(id) | SYNTHETIC_TEXT, kind: RenderNodeKind::Text(text) });
        }
        Some(RenderDelta { ops })
    }

    /// The `<style>` sheet text the render tree gives `owner` (see
    /// [`Dom::render_delta`]), as a delta op.
    pub fn render_style_text(&self, owner: NodeId, opts: &RenderHtml<'_>) -> Option<DeltaOp> {
        let e = self.node(owner).ok()?.element()?;
        if !(e.is_html() && &*e.name.local == "style") {
            return None;
        }
        let text = (opts.style_text)(owner).unwrap_or_else(|| self.child_text_content(owner).into_owned());
        Some(DeltaOp::Node { key: render_key(owner) | SYNTHETIC_TEXT, kind: RenderNodeKind::Text(text) })
    }

    /// The pseudo-class states of element `n` a renderer cannot derive from
    /// its markup (validity, placeholder shown, read-write, ...), as the
    /// space-separated value of the [`STATE_NS`] `state` attribute; `None`
    /// for elements without any.
    fn render_states(&self, n: u32) -> Option<String> {
        use crate::select::PseudoClass::*;
        let e = self.n(n).element()?;
        let local: &str = &e.name.local;
        let control = matches!(local, "input" | "textarea" | "select" | "button" | "output");
        let kind = e.is_html()
            && (control
                || matches!(local, "form" | "fieldset" | "option" | "optgroup" | "details" | "dialog" | "progress")
                || e.attr("popover").is_some()
                || e.flags & crate::node::element_flags::FORM_ASSOCIATED != 0);
        let editable = e.attr("contenteditable").is_some();
        // An image button (`<input type=image>`) is laid out like `<img>`:
        // its natural size travels the same way.
        // A `<video>` with known metadata reports its video size the same way.
        let is_image = local == "img"
            || local == "video"
            || (local == "input" && e.attr("type").is_some_and(|t| t.eq_ignore_ascii_case("image")));
        let natural = (e.is_html() && is_image).then(|| self.natural_size(self.id_of(n))).flatten();
        if !kind && !editable && natural.is_none() {
            return None;
        }
        let mut out = String::new();
        if let Some((w, h)) = natural {
            out.push_str(&format!("natural={w},{h}"));
        }
        for (pc, name) in [
            (Valid, "valid"),
            (Invalid, "invalid"),
            (Indeterminate, "indeterminate"),
            (Required, "required"),
            (Optional, "optional"),
            (ReadWrite, "read-write"),
            (PlaceholderShown, "placeholder-shown"),
            (Default, "default"),
            (Open, "open"),
            (Modal, "modal"),
            (PopoverOpen, "popover-open"),
            (InRange, "in-range"),
            (OutOfRange, "out-of-range"),
            (UserValid, "user-valid"),
            (UserInvalid, "user-invalid"),
        ] {
            if self.matches_state(n, &pc) {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(name);
            }
        }
        // The top layer's paint order: renderers paint these elements above
        // the document by rank (one empty check without a top layer).
        if let Some(rank) = self.top_layer_rank(self.id_of(n)) {
            if !out.is_empty() {
                out.push(' ');
            }
            let _ = std::fmt::Write::write_fmt(&mut out, format_args!("top-layer={rank}"));
        }
        (!out.is_empty()).then_some(out)
    }

    /// Elements whose [`Dom::render_states`] a change at `i` can change
    /// besides `i`: the forms, fieldsets and selects around it, and every
    /// radio when `i` is one (a renamed radio leaves one group for another).
    fn state_dependents(&self, i: u32, out: &mut Vec<u32>) {
        if !self.slot_live(i) {
            return;
        }
        let mut cur = self.n(i).parent;
        while cur != NIL {
            if let Some(e) = self.n(cur).element() {
                if e.is_html() && matches!(&*e.name.local, "form" | "fieldset" | "select") {
                    out.push(cur);
                }
            }
            cur = self.n(cur).parent;
        }
        let is_radio = |e: &crate::node::ElementData| {
            e.is_html() && &*e.name.local == "input" && e.attr("type").is_some_and(|t| t.eq_ignore_ascii_case("radio"))
        };
        if self.n(i).element().is_some_and(is_radio) {
            let root = self.root_idx(i);
            let mut n = root;
            while n != NIL {
                if n != i && self.n(n).element().is_some_and(is_radio) {
                    out.push(n);
                }
                n = self.following_in(n, root);
            }
        }
    }

    /// Whether child `k` appears in the render tree (as itself).
    fn render_child(&self, k: u32) -> bool {
        match &self.n(k).kind {
            NodeKind::Text(_) | NodeKind::CData(_) => true,
            NodeKind::Element(e) => {
                let local: &str = &e.name.local;
                !(e.is_html() && matches!(local, "script" | "noscript" | "template"))
                    && !(e.name.ns == ns!(svg) && local == "script")
            }
            _ => false,
        }
    }

    /// Light children of `host` assigned to the slot named `name` (the
    /// default slot takes text and children without a `slot` attribute).
    fn slotted(&self, host: u32, name: &str) -> Vec<u32> {
        self.child_idx_iter(host)
            .filter(|&c| match &self.n(c).kind {
                NodeKind::Text(_) => name.is_empty(),
                NodeKind::Element(e) => e.attr("slot").unwrap_or("") == name,
                _ => false,
            })
            .collect()
    }
}

/// The render key of the `n`th adopted sheet rendered: bit 62 marks it
/// apart from node keys. Keys follow the rendering order (not the sheet), so
/// a reordered list changes the texts of the same elements: the renderer
/// cascades its sheets in the order it made their elements.
fn adopted_key(n: usize) -> u64 {
    (1 << 62) | n as u64
}

fn push_children(stack: &mut Vec<Step>, kids: Vec<u32>, scope: Scope) {
    stack.extend(kids.into_iter().rev().map(|k| Step::Enter(k, scope)));
}
