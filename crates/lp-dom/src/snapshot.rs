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

use crate::dom::Dom;
use crate::error::DomResult;
use crate::forms::InputType;
use crate::node::{NodeId, NodeKind, NIL};
use crate::serialize::{escape, is_void};

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
}

impl Sink for HtmlSink<'_> {
    fn doctype(&mut self, name: &str) {
        self.out.push_str("<!DOCTYPE ");
        self.out.push_str(name);
        self.out.push('>');
    }
    fn open(&mut self, _key: u64, tag: &str, _name: &QualName, attrs: &[RAttr<'_>], marked: bool, _void: bool) {
        let out = &mut self.out;
        out.push('<');
        out.push_str(tag);
        for a in attrs {
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
        let mut sink = HtmlSink { out: String::with_capacity(64 * 1024), mark: opts.mark.map(|m| m.1) };
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
        let mark = opts.mark.and_then(|(id, _)| self.check(id).ok());
        // Shadow hosts entered: (host, the host's own scope).
        let mut hosts: Vec<(u32, Scope)> = Vec::new();
        // Tags of the open elements, for `close`.
        let mut tags: Vec<String> = Vec::new();
        let mut stack = Vec::new();
        push_children(&mut stack, self.child_idx_iter(d).collect(), NONE);
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
                Step::Enter(n, scope) => (n, scope),
            };
            match &self.n(n).kind {
                NodeKind::Doctype(dt) => sink.doctype(&dt.name),
                NodeKind::Text(s) | NodeKind::CData(s) => sink.text(render_key(self.id_of(n)), s, TextKind::Node),
                NodeKind::Comment(_) | NodeKind::ProcessingInstruction(_) => {}
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
                            if !v.is_empty() {
                                attrs.push(RAttr { name: plain("value"), value: Cow::Owned(v), valueless: false });
                            }
                        }
                    }
                    if html && local == "option" && self.option_selected(id).unwrap_or(false) {
                        attrs.push(RAttr { name: plain("selected"), value: Cow::Borrowed(""), valueless: true });
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
                        let text = (opts.style_text)(id)
                            .unwrap_or_else(|| self.text_content(id).ok().flatten().unwrap_or_default());
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
                        push_children(&mut stack, self.child_idx_iter(e.shadow_root).collect(), hosts.len() - 1);
                    } else {
                        push_children(&mut stack, self.child_idx_iter(n).collect(), scope);
                    }
                }
            }
        }
        Ok(())
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

fn push_children(stack: &mut Vec<Step>, kids: Vec<u32>, scope: Scope) {
    stack.extend(kids.into_iter().rev().map(|k| Step::Enter(k, scope)));
}
