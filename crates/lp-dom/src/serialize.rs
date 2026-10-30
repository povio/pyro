//! HTML fragment serialization (HTML §13.3) for innerHTML/outerHTML and
//! XMLSerializer on HTML documents.

use markup5ever::{local_name, ns};

use crate::dom::Dom;
use crate::error::DomResult;
use crate::node::{NodeId, NodeKind, NIL};

pub(crate) fn is_void(local: &str) -> bool {
    matches!(
        local,
        "area"
            | "base"
            | "basefont"
            | "bgsound"
            | "br"
            | "col"
            | "embed"
            | "frame"
            | "hr"
            | "img"
            | "input"
            | "keygen"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

pub(crate) fn escape(out: &mut String, s: &str, attr: bool) {
    let mut last = 0;
    for (i, c) in s.char_indices() {
        let rep = match c {
            '&' => "&amp;",
            '\u{a0}' => "&nbsp;",
            '"' if attr => "&quot;",
            '<' => "&lt;",
            '>' => "&gt;",
            _ => continue,
        };
        out.push_str(&s[last..i]);
        out.push_str(rep);
        last = i + c.len_utf8();
    }
    out.push_str(&s[last..]);
}

impl Dom {
    /// `innerHTML` getter: serializes the children (template: contents).
    pub fn inner_html(&self, id: NodeId) -> DomResult<String> {
        let i = self.check(id)?;
        let mut out = String::new();
        let src = match self.n(i).element() {
            Some(e) if e.template_contents != NIL => e.template_contents,
            _ => i,
        };
        self.serialize_children(src, &mut out);
        Ok(out)
    }

    /// `outerHTML` getter.
    pub fn outer_html(&self, id: NodeId) -> DomResult<String> {
        let i = self.check(id)?;
        let mut out = String::new();
        self.serialize_node(i, &mut out);
        Ok(out)
    }

    fn serialize_children(&self, parent: u32, out: &mut String) {
        for c in self.child_idx_iter(parent) {
            self.serialize_node(c, out);
        }
    }

    fn serialize_node(&self, i: u32, out: &mut String) {
        // Iterative pre/post-order walk so deep trees cannot overflow the stack.
        enum Step {
            Enter(u32),
            Close(u32),
        }
        let mut stack = vec![Step::Enter(i)];
        while let Some(step) = stack.pop() {
            match step {
                Step::Close(e) => {
                    out.push_str("</");
                    out.push_str(&self.serialized_tag(e));
                    out.push('>');
                }
                Step::Enter(n) => match &self.n(n).kind {
                    NodeKind::Element(e) => {
                        let tag = self.serialized_tag(n);
                        out.push('<');
                        out.push_str(&tag);
                        for a in &e.attrs {
                            out.push(' ');
                            let ns = &a.name.ns;
                            if *ns == ns!() {
                                out.push_str(&a.name.local);
                            } else if *ns == ns!(xml) {
                                out.push_str("xml:");
                                out.push_str(&a.name.local);
                            } else if *ns == ns!(xmlns) {
                                if a.name.local != local_name!("xmlns") {
                                    out.push_str("xmlns:");
                                }
                                out.push_str(&a.name.local);
                            } else if *ns == ns!(xlink) {
                                out.push_str("xlink:");
                                out.push_str(&a.name.local);
                            } else {
                                out.push_str(&a.qualified_name());
                            }
                            out.push_str("=\"");
                            escape(out, &a.value, true);
                            out.push('"');
                        }
                        out.push('>');
                        if e.is_html() && is_void(&e.name.local) {
                            continue;
                        }
                        stack.push(Step::Close(n));
                        let src = if e.template_contents != NIL { e.template_contents } else { n };
                        let kids: Vec<u32> = self.child_idx_iter(src).collect();
                        for k in kids.into_iter().rev() {
                            stack.push(Step::Enter(k));
                        }
                    }
                    NodeKind::Text(s) | NodeKind::CData(s) => {
                        let p = self.n(n).parent;
                        let raw = p != NIL
                            && self.n(p).element().is_some_and(|e| {
                                e.is_html()
                                    && matches!(
                                        &*e.name.local,
                                        "style" | "script" | "xmp" | "iframe" | "noembed" | "noframes" | "plaintext"
                                    )
                            });
                        if raw {
                            out.push_str(s);
                        } else {
                            escape(out, s, false);
                        }
                    }
                    NodeKind::Comment(s) => {
                        out.push_str("<!--");
                        out.push_str(s);
                        out.push_str("-->");
                    }
                    NodeKind::ProcessingInstruction(pi) => {
                        out.push_str("<?");
                        out.push_str(&pi.target);
                        out.push(' ');
                        out.push_str(&pi.data);
                        out.push('>');
                    }
                    NodeKind::Doctype(d) => {
                        out.push_str("<!DOCTYPE ");
                        out.push_str(&d.name);
                        out.push('>');
                    }
                    NodeKind::Document(_) | NodeKind::Fragment(_) => {
                        let kids: Vec<u32> = self.child_idx_iter(n).collect();
                        for k in kids.into_iter().rev() {
                            stack.push(Step::Enter(k));
                        }
                    }
                },
            }
        }
    }

    pub(crate) fn serialized_tag(&self, i: u32) -> String {
        let e = self.n(i).element().expect("element");
        if e.is_html() || e.name.ns == ns!(svg) || e.name.ns == ns!(mathml) {
            e.name.local.to_string()
        } else {
            match &e.name.prefix {
                Some(p) if !p.is_empty() => format!("{}:{}", p, e.name.local),
                _ => e.name.local.to_string(),
            }
        }
    }
}
