//! Document serialization for rendering (screenshots): the current tree as
//! HTML another engine can lay out and paint without running script.
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

use markup5ever::ns;

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

/// Tree scope of a node being serialized: index into the shadow hosts
/// entered so far, `NONE` for the document.
type Scope = usize;
const NONE: Scope = usize::MAX;

enum Step {
    Enter(u32, Scope),
    Close(u32),
    Raw(String),
}

impl Dom {
    /// Serializes document `doc` for rendering (see the module docs).
    pub fn render_html(&self, doc: NodeId, opts: &RenderHtml<'_>) -> DomResult<String> {
        let d = self.check(doc)?;
        let mark = opts.mark.and_then(|(id, name)| self.check(id).ok().map(|i| (i, name)));
        let mut out = String::with_capacity(64 * 1024);
        // Shadow hosts entered: (host, the host's own scope).
        let mut hosts: Vec<(u32, Scope)> = Vec::new();
        let mut stack = Vec::new();
        push_children(&mut stack, self.child_idx_iter(d).collect(), NONE);
        while let Some(step) = stack.pop() {
            let (n, scope) = match step {
                Step::Raw(s) => {
                    out.push_str(&s);
                    continue;
                }
                Step::Close(e) => {
                    out.push_str("</");
                    out.push_str(&self.serialized_tag(e));
                    out.push('>');
                    continue;
                }
                Step::Enter(n, scope) => (n, scope),
            };
            match &self.n(n).kind {
                NodeKind::Doctype(dt) => {
                    out.push_str("<!DOCTYPE ");
                    out.push_str(&dt.name);
                    out.push('>');
                }
                NodeKind::Text(s) | NodeKind::CData(s) => escape(&mut out, s, false),
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
                    let tag = self.serialized_tag(n);
                    out.push('<');
                    out.push_str(&tag);
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
                        out.push(' ');
                        out.push_str(&a.qualified_name());
                        out.push_str("=\"");
                        escape(&mut out, &a.value, true);
                        out.push('"');
                    }
                    if let Some(href) = &href {
                        out.push_str(" href=\"");
                        escape(&mut out, href, true);
                        out.push('"');
                    }
                    if let Some(ty) = input_type {
                        if matches!(ty, InputType::Checkbox | InputType::Radio) {
                            if self.checked(id).unwrap_or(false) {
                                out.push_str(" checked");
                            }
                            // Their value is not displayed.
                            if let Some(v) = e.attr("value") {
                                out.push_str(" value=\"");
                                escape(&mut out, v, true);
                                out.push('"');
                            }
                        } else if ty != InputType::File {
                            let v = self.value(id).unwrap_or_default();
                            if !v.is_empty() {
                                out.push_str(" value=\"");
                                escape(&mut out, &v, true);
                                out.push('"');
                            }
                        }
                    }
                    if html && local == "option" && self.option_selected(id).unwrap_or(false) {
                        out.push_str(" selected");
                    }
                    if let Some((m, name)) = mark {
                        if m == n {
                            out.push(' ');
                            out.push_str(name);
                        }
                    }
                    out.push('>');
                    if html && is_void(local) {
                        continue;
                    }
                    stack.push(Step::Close(n));
                    if html && local == "style" {
                        let text = (opts.style_text)(id)
                            .unwrap_or_else(|| self.text_content(id).ok().flatten().unwrap_or_default());
                        stack.push(Step::Raw(text));
                        continue;
                    }
                    if html && local == "textarea" {
                        let mut v = String::new();
                        escape(&mut v, &self.value(id).unwrap_or_default(), false);
                        // A leading newline right after <textarea> is dropped by parsers.
                        stack.push(Step::Raw(v));
                        stack.push(Step::Raw("\n".into()));
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
        Ok(out)
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
