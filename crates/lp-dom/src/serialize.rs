//! HTML fragment serialization (HTML §13.3) for innerHTML/outerHTML, and XML
//! serialization (DOM Parsing §2.3, not requiring well-formedness) for
//! XMLSerializer.

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

/// XML character escaping; attributes also escape quotes and the whitespace
/// an XML parser would normalize away.
fn xml_escape(out: &mut String, s: &str, attr: bool) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attr => out.push_str("&quot;"),
            '\t' if attr => out.push_str("&#9;"),
            '\n' if attr => out.push_str("&#10;"),
            '\r' if attr => out.push_str("&#13;"),
            _ => out.push(c),
        }
    }
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

    /// `XMLSerializer.serializeToString`. Prefixes are kept as written and
    /// a namespace declaration is added where the in-scope ones do not cover
    /// a name; generated prefixes (`ns1`) are not implemented.
    pub fn xml_serialize(&self, id: NodeId) -> DomResult<String> {
        enum Step {
            Enter(u32),
            Close(String),
            PopScope(usize),
        }
        let root = self.check(id)?;
        let mut out = String::new();
        let xhtml = ns!(html);
        // In-scope (prefix, namespace) pairs, innermost last; "" is the default.
        let mut scope: Vec<(String, String)> = Vec::new();
        let mut stack = vec![Step::Enter(root)];
        while let Some(step) = stack.pop() {
            match step {
                Step::Close(tag) => {
                    out.push_str("</");
                    out.push_str(&tag);
                    out.push('>');
                }
                Step::PopScope(len) => scope.truncate(len),
                Step::Enter(n) => match &self.n(n).kind {
                    NodeKind::Element(e) => {
                        let scope_len = scope.len();
                        let prefix = e.name.prefix.as_ref().map_or("", |p| &**p);
                        let tag = if prefix.is_empty() {
                            e.name.local.to_string()
                        } else {
                            format!("{prefix}:{}", e.name.local)
                        };
                        out.push('<');
                        out.push_str(&tag);
                        // Declarations written on this element come first into scope.
                        for a in &e.attrs {
                            if a.name.ns == ns!(xmlns) {
                                let declared = if a.name.local == local_name!("xmlns") { "" } else { &*a.name.local };
                                scope.push((declared.to_owned(), a.value.to_string()));
                            }
                        }
                        let declare = |out: &mut String, scope: &mut Vec<(String, String)>, prefix: &str, ns: &str| {
                            if scope.iter().rev().find(|(p, _)| p == prefix).map(|(_, n)| n.as_str()).unwrap_or("")
                                != ns
                            {
                                out.push_str(if prefix.is_empty() { " xmlns" } else { " xmlns:" });
                                out.push_str(prefix);
                                out.push_str("=\"");
                                xml_escape(out, ns, true);
                                out.push('"');
                                scope.push((prefix.to_owned(), ns.to_owned()));
                            }
                        };
                        declare(&mut out, &mut scope, prefix, &e.name.ns);
                        for a in &e.attrs {
                            out.push(' ');
                            let ans = &a.name.ns;
                            if *ans == ns!() {
                                out.push_str(&a.name.local);
                            } else if *ans == ns!(xmlns) {
                                if a.name.local != local_name!("xmlns") {
                                    out.push_str("xmlns:");
                                }
                                out.push_str(&a.name.local);
                            } else if *ans == ns!(xml) {
                                out.push_str("xml:");
                                out.push_str(&a.name.local);
                            } else {
                                let aprefix = match &a.name.prefix {
                                    Some(p) if !p.is_empty() => p.to_string(),
                                    _ if *ans == ns!(xlink) => "xlink".to_owned(),
                                    _ => String::new(),
                                };
                                if aprefix.is_empty() {
                                    out.push_str(&a.name.local);
                                } else {
                                    // Written before this attribute: a declaration
                                    // is an attribute of the same element.
                                    let mut decl = String::new();
                                    declare(&mut decl, &mut scope, &aprefix, ans);
                                    if !decl.is_empty() {
                                        out.push_str(decl.trim_start());
                                        out.push(' ');
                                    }
                                    out.push_str(&aprefix);
                                    out.push(':');
                                    out.push_str(&a.name.local);
                                }
                            }
                            out.push_str("=\"");
                            xml_escape(&mut out, &a.value, true);
                            out.push('"');
                        }
                        let src = if e.template_contents != NIL { e.template_contents } else { n };
                        if self.n(src).first_child == NIL {
                            // HTML-namespace elements keep an explicit end tag unless void.
                            if e.name.ns != xhtml {
                                out.push_str("/>");
                                scope.truncate(scope_len);
                            } else if is_void(&e.name.local) {
                                out.push_str(" />");
                                scope.truncate(scope_len);
                            } else {
                                out.push_str("></");
                                out.push_str(&tag);
                                out.push('>');
                                scope.truncate(scope_len);
                            }
                            continue;
                        }
                        out.push('>');
                        stack.push(Step::PopScope(scope_len));
                        stack.push(Step::Close(tag));
                        let kids: Vec<u32> = self.child_idx_iter(src).collect();
                        for k in kids.into_iter().rev() {
                            stack.push(Step::Enter(k));
                        }
                    }
                    NodeKind::Text(s) => xml_escape(&mut out, s, false),
                    NodeKind::CData(s) => {
                        out.push_str("<![CDATA[");
                        out.push_str(s);
                        out.push_str("]]>");
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
                        out.push_str("?>");
                    }
                    NodeKind::Doctype(d) => {
                        out.push_str("<!DOCTYPE ");
                        out.push_str(&d.name);
                        if !d.public_id.is_empty() {
                            out.push_str(" PUBLIC \"");
                            out.push_str(&d.public_id);
                            out.push('"');
                        }
                        if !d.system_id.is_empty() {
                            if d.public_id.is_empty() {
                                out.push_str(" SYSTEM");
                            }
                            out.push_str(" \"");
                            out.push_str(&d.system_id);
                            out.push('"');
                        }
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
