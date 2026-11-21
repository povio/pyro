//! WHATWG HTML “prepare content patching” for a parser-owned, detached fragment.
//! The tree builder still parses an ordinary template; its template-content
//! insertion point redirects to the patch target while that template is open.
use crate::dom::Dom;
use crate::node::{parse_pi_attributes, NodeId, NodeKind, NIL};
use rustc_hash::FxHashMap;

struct Patch {
    target: NodeId,
    start: NodeId,
    end: Option<NodeId>,
}

#[derive(Default)]
pub(super) struct Patches {
    // In a scriptless document parse the actual document body is on the
    // open-elements stack whenever the parser inserts a template into it.
    pub(super) document_scope: bool,
    // The tree builder asks for the template contents when inserting its
    // children. Nested template elements have different content fragments.
    by_contents: FxHashMap<u32, Patch>,
}

impl Patches {
    pub fn insertion(&self, dom: &Dom, parent: u32, before: u32) -> (u32, u32) {
        let Some(patch) = self.by_contents.get(&parent) else { return (parent, before) };
        let Ok(target) = dom.check(patch.target) else { return (parent, before) };
        let end = patch.end.and_then(|id| dom.check(id).ok()).filter(|&idx| dom.n(idx).parent == target);
        (target, end.unwrap_or(NIL))
    }

    pub fn prepare(&mut self, dom: &mut Dom, parser_doc: u32, scope: u32, template: u32) -> bool {
        let Some(el) = dom.n(template).element() else { return false };
        let contents = el.template_contents;
        // External content has a separate fetch and parsing lifecycle.
        if contents == NIL
            || el.attr("src").is_some()
            || el
                .attr("shadowrootmode")
                .is_some_and(|v| v.eq_ignore_ascii_case("open") || v.eq_ignore_ascii_case("closed"))
        {
            return false;
        }
        let Some(name) = el.attr("for").filter(|name| !name.is_empty()) else { return false };
        // Never search outwards into the caller's fragment context, its
        // template contents, or a neighboring shadow tree.
        if !owned_scope(dom, scope, parser_doc) {
            return false;
        }
        let scope = if self.document_scope && dom.body(dom.id_of(parser_doc)).ok().flatten() == Some(dom.id_of(scope)) {
            dom.n(scope).parent
        } else {
            scope
        };
        let Some((start, end)) = markers(dom, scope, name) else { return false };
        let target = dom.n(start).parent;
        let patch = Patch { target: dom.id_of(target), start: dom.id_of(start), end: end.map(|idx| dom.id_of(idx)) };
        if end != Some(start) {
            let mut removed = Vec::new();
            let mut sibling = dom.n(start).next;
            while sibling != NIL && Some(sibling) != end {
                removed.push(dom.id_of(sibling));
                sibling = dom.n(sibling).next;
            }
            for node in removed {
                remove_if_parented(dom, node);
            }
        }
        self.by_contents.insert(contents, patch);
        true
    }

    pub fn close(&mut self, dom: &mut Dom, template: u32) {
        let Some(contents) = dom.n(template).element().map(|e| e.template_contents) else { return };
        if let Some(patch) = self.by_contents.remove(&contents) {
            remove_if_parented(dom, patch.start);
            if let Some(end) = patch.end.filter(|&id| id != patch.start) {
                remove_if_parented(dom, end);
            }
        }
    }
}

fn owned_scope(dom: &Dom, mut scope: u32, doc: u32) -> bool {
    loop {
        let root = dom.root_idx(scope);
        if root == doc {
            return true;
        }
        match &dom.n(root).kind {
            NodeKind::Fragment(f) if f.host != NIL => scope = f.host,
            _ => return false,
        }
    }
}

fn remove_if_parented(dom: &mut Dom, node: NodeId) {
    if let Ok(Some(parent)) = dom.parent(node) {
        dom.remove_child(parent, node).expect("checked parent in detached parser output");
    }
}

fn markers(dom: &Dom, scope: u32, name: &str) -> Option<(u32, Option<u32>)> {
    let mut node = dom.n(scope).first_child;
    while node != NIL {
        if let NodeKind::ProcessingInstruction(pi) = &dom.n(node).kind {
            if matches!(pi.target.as_ref(), "marker" | "start")
                && parse_pi_attributes(&pi.data)
                    .is_some_and(|attrs| attrs.iter().any(|(key, value)| key == "name" && value == name))
            {
                if pi.target.as_ref() == "marker" {
                    return Some((node, Some(node)));
                }
                let mut depth = 0usize;
                let mut sibling = dom.n(node).next;
                while sibling != NIL {
                    if let NodeKind::ProcessingInstruction(pi) = &dom.n(sibling).kind {
                        match pi.target.as_ref() {
                            "start" => depth += 1,
                            "end" if depth == 0 => return Some((node, Some(sibling))),
                            "end" => depth -= 1,
                            _ => {}
                        }
                    }
                    sibling = dom.n(sibling).next;
                }
                return Some((node, None));
            }
        }
        node = dom.following_in(node, scope);
    }
    None
}
