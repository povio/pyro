//! Sequential focus navigation (HTML §6.6.3, "flattened tabindex-ordered
//! focus navigation scope") and focus delegates (§6.6.4, `delegatesFocus`).
//!
//! The document, every shadow host and every slot own a focus navigation
//! scope. A scope lists its elements in tree order without descending into
//! nested scope owners; positive tabindex values come first (ascending, ties
//! in tree order), then the rest. Navigation walks the current scope, enters
//! a nested owner's scope at its position (unless its tabindex is negative),
//! and continues after the owner once a nested scope is exhausted, as
//! Chrome's `FocusController` does.
//!
//! Layout is not visible here: callers pass `area`, the "focusable area"
//! test (focusable, not inert, being rendered). Only Tab and `focus()` on a
//! `delegatesFocus` host run this code; nothing is kept between calls.

use markup5ever::local_name;

use crate::dom::Dom;
use crate::node::{shadow_flags, NodeId, NodeKind, NIL};

impl Dom {
    fn tabindex_attr(&self, i: u32) -> Option<i32> {
        self.n(i).element()?.attr_local(&local_name!("tabindex"))?.trim().parse().ok()
    }

    fn delegates_focus_idx(&self, i: u32) -> bool {
        let sr = self.n(i).element().map_or(NIL, |e| e.shadow_root);
        sr != NIL && matches!(&self.n(sr).kind, NodeKind::Fragment(f) if f.flags & shadow_flags::DELEGATES_FOCUS != 0)
    }

    /// A shadow host whose shadow root delegates focus.
    pub fn is_delegates_focus_host(&self, el: NodeId) -> bool {
        self.check(el).is_ok_and(|i| self.delegates_focus_idx(i))
    }

    /// The slot `node` is assigned to, in an open or a closed shadow tree.
    pub fn assigned_slot_any(&self, node: NodeId) -> Option<NodeId> {
        let i = self.check(node).ok()?;
        self.find_slot(i, false).map(|s| self.id_of(s))
    }

    /// `node` is `anc` or a shadow-including descendant of it.
    pub fn shadow_including_contains(&self, anc: NodeId, node: NodeId) -> bool {
        match (self.check(anc), self.check(node)) {
            // Outside shadow trees this is plain containment.
            (Ok(a), Ok(n)) if !self.in_shadow_tree(n) => self.is_inclusive_ancestor(a, n),
            (Ok(a), Ok(n)) => self.shadow_including_inclusive_ancestor(a, n),
            _ => false,
        }
    }

    fn is_scope_owner(&self, i: u32) -> bool {
        self.n(i).element().is_some_and(|e| e.shadow_root != NIL) || self.is_slot(i)
    }

    /// The elements of `owner`'s scope, tabindex-ordered.
    fn scope_items(&self, owner: u32) -> Vec<u32> {
        let roots: Vec<u32> = match &self.n(owner).kind {
            NodeKind::Document(_) => self.child_idx_iter(owner).collect(),
            NodeKind::Element(e) if e.shadow_root != NIL => self.child_idx_iter(e.shadow_root).collect(),
            _ => {
                let assigned = self.slot_assigned(owner);
                if assigned.is_empty() {
                    self.child_idx_iter(owner).collect()
                } else {
                    assigned
                }
            }
        };
        let mut out = Vec::new();
        let mut stack: Vec<u32> = roots.into_iter().rev().collect();
        while let Some(i) = stack.pop() {
            if !self.n(i).is_element() {
                continue;
            }
            out.push(i);
            if !self.is_scope_owner(i) {
                let at = stack.len();
                stack.extend(self.child_idx_iter(i));
                stack[at..].reverse();
            }
        }
        // Stable: equal keys keep tree order.
        out.sort_by_key(|&i| match self.tabindex_attr(i) {
            Some(t) if t > 0 => (0, t),
            _ => (1, 0),
        });
        out
    }

    /// The scope owner whose scope lists `el`, if `el` is in one at all
    /// (an unassigned child of a shadow host is not).
    fn scope_of(&self, el: u32) -> Option<u32> {
        let mut x = el;
        loop {
            if let Some(slot) = self.find_slot(x, false) {
                return Some(slot);
            }
            let p = self.n(x).parent;
            if p == NIL {
                return None;
            }
            match &self.n(p).kind {
                NodeKind::Document(_) => return Some(p),
                NodeKind::Fragment(f) if f.shadow.is_some() => return Some(f.host),
                NodeKind::Element(e) if e.shadow_root != NIL => return None,
                NodeKind::Element(_) if self.is_slot(p) => return Some(p),
                NodeKind::Element(_) => x = p,
                _ => return None,
            }
        }
    }

    /// Navigation enters a scope owner unless its tabindex is negative.
    fn enters(&self, i: u32) -> bool {
        self.is_scope_owner(i) && self.tabindex_attr(i).unwrap_or(0) >= 0
    }

    /// Sequential navigation lands on `i`: a focusable area with a
    /// non-negative tabindex, not a slot, not a host that delegates focus.
    fn tabbable(&self, i: u32, area: &dyn Fn(NodeId) -> bool) -> bool {
        self.tabindex_attr(i).unwrap_or(0) >= 0
            && !self.is_slot(i)
            && !self.delegates_focus_idx(i)
            && area(self.id_of(i))
    }

    fn enter_forward(&self, i: u32, area: &dyn Fn(NodeId) -> bool) -> Option<u32> {
        if self.tabbable(i, area) {
            return Some(i);
        }
        if self.enters(i) {
            return self.first_in(i, area);
        }
        None
    }

    fn enter_backward(&self, i: u32, area: &dyn Fn(NodeId) -> bool) -> Option<u32> {
        if self.enters(i) {
            if let Some(r) = self.last_in(i, area) {
                return Some(r);
            }
        }
        self.tabbable(i, area).then_some(i)
    }

    fn first_in(&self, owner: u32, area: &dyn Fn(NodeId) -> bool) -> Option<u32> {
        self.scope_items(owner).into_iter().find_map(|i| self.enter_forward(i, area))
    }

    fn last_in(&self, owner: u32, area: &dyn Fn(NodeId) -> bool) -> Option<u32> {
        self.scope_items(owner).into_iter().rev().find_map(|i| self.enter_backward(i, area))
    }

    /// The element Tab (`backward`: Shift+Tab) moves focus to from
    /// `current` (the start or end of the document without one); `None`
    /// at either end of the document.
    pub fn sequential_focus_target(
        &self,
        doc: NodeId,
        current: Option<NodeId>,
        backward: bool,
        area: &dyn Fn(NodeId) -> bool,
    ) -> Option<NodeId> {
        let d = self.check(doc).ok()?;
        let from = current.and_then(|c| self.check(c).ok()).filter(|&c| self.scope_of(c).is_some());
        let Some(mut cur) = from else {
            let r = if backward { self.last_in(d, area) } else { self.first_in(d, area) };
            return r.map(|i| self.id_of(i));
        };
        // A focused scope owner's own scope comes right after it.
        if !backward && self.is_scope_owner(cur) {
            if let Some(r) = self.first_in(cur, area) {
                return Some(self.id_of(r));
            }
        }
        loop {
            let owner = self.scope_of(cur)?;
            let items = self.scope_items(owner);
            let pos = items.iter().position(|&i| i == cur)?;
            let found = if backward {
                items[..pos].iter().rev().find_map(|&i| self.enter_backward(i, area))
            } else {
                items[pos + 1..].iter().find_map(|&i| self.enter_forward(i, area))
            };
            if let Some(r) = found {
                return Some(self.id_of(r));
            }
            if matches!(self.n(owner).kind, NodeKind::Document(_)) {
                return None;
            }
            if backward && self.tabbable(owner, area) {
                return Some(self.id_of(owner));
            }
            cur = owner;
        }
    }

    /// HTML "focus delegate" of a `delegatesFocus` shadow host (null for
    /// any other host): its shadow tree's first autofocus element that is a
    /// focusable area, else its first focusable area in tree order, looking
    /// into nested hosts that delegate focus too. Slotted content is not
    /// considered (it is not in the shadow tree).
    pub fn focus_delegate(&self, host: NodeId, area: &dyn Fn(NodeId) -> bool) -> Option<NodeId> {
        let h = self.check(host).ok()?;
        if !self.delegates_focus_idx(h) {
            return None;
        }
        let root = self.n(h).element()?.shadow_root;
        let candidate = |i: u32| self.n(i).is_element() && !self.delegates_focus_idx(i) && area(self.id_of(i));
        let mut i = self.following_in(root, root);
        while i != NIL {
            if self.n(i).element().is_some_and(|e| e.attr_local(&local_name!("autofocus")).is_some()) && candidate(i) {
                return Some(self.id_of(i));
            }
            i = self.following_in(i, root);
        }
        let mut i = self.following_in(root, root);
        while i != NIL {
            if self.delegates_focus_idx(i) {
                if let Some(d) = self.focus_delegate(self.id_of(i), area) {
                    return Some(d);
                }
            } else if candidate(i) {
                return Some(self.id_of(i));
            }
            i = self.following_in(i, root);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use crate::parse::{parse_document, parse_html_unsafe};
    use crate::{Dom, NodeId};

    fn ids(dom: &Dom, doc: NodeId, names: &[&str]) -> Vec<NodeId> {
        names.iter().map(|n| dom.get_element_by_id(doc, n).unwrap().unwrap()).collect()
    }

    /// Tab order from the start, by id.
    fn order(dom: &Dom, doc: NodeId, backward: bool) -> Vec<String> {
        let area = |id: NodeId| dom.is_focusable(id).unwrap_or(false) && !dom.is_inert(id);
        let mut out = Vec::new();
        let mut cur = None;
        while let Some(n) = dom.sequential_focus_target(doc, cur, backward, &area) {
            out.push(dom.get_attribute(n, "id").unwrap().unwrap_or("?").to_owned());
            cur = Some(n);
            assert!(out.len() < 50, "cycle: {out:?}");
        }
        out
    }

    #[test]
    fn positive_tabindex_first_then_tree_order() {
        let dom = Rc::new(RefCell::new(Dom::new()));
        let doc = parse_document(
            &dom,
            "about:blank",
            "<input id=a><input id=b tabindex=2><input id=c tabindex=-1><button id=d tabindex=1></button>\
             <div id=e tabindex=0></div><input id=f disabled><div inert><input id=g></div><input id=h tabindex=2>",
        );
        let dom = dom.borrow();
        assert_eq!(order(&dom, doc, false), ["d", "b", "h", "a", "e"]);
        assert_eq!(order(&dom, doc, true), ["e", "a", "h", "b", "d"]);
    }

    #[test]
    fn shadow_scopes_and_slots() {
        let dom = Rc::new(RefCell::new(Dom::new()));
        let doc = parse_html_unsafe(
            &dom,
            "<input id=i0><div id=host tabindex=0><input id=i2 slot=s tabindex=2><input id=i1 slot=s tabindex=1>\
             <template shadowrootmode=open><input id=j1><slot name=s></slot><input id=j2></template></div>\
             <div id=neg tabindex=-1><template shadowrootmode=open><input id=hidden></template></div>\
             <div id=df><template shadowrootmode=open shadowrootdelegatesfocus><input id=k1></template></div>\
             <input id=last>",
        );
        let d = dom.borrow();
        let shadow_id = |host: &str, id: &str| {
            let h = ids(&d, doc, &[host])[0];
            let sr = d.shadow_root(h).unwrap().unwrap();
            d.get_element_by_id(sr, id).unwrap().unwrap()
        };
        let _ = shadow_id("host", "j1");
        let names: Vec<String> = order(&d, doc, false);
        assert_eq!(names, ["i0", "host", "j1", "i1", "i2", "j2", "k1", "last"]);
        let back: Vec<String> = order(&d, doc, true);
        assert_eq!(back, ["last", "k1", "j2", "i2", "i1", "j1", "host", "i0"]);
        // Inside a skipped (tabindex=-1) host, navigation continues after it.
        let hidden = shadow_id("neg", "hidden");
        let area = |id: NodeId| d.is_focusable(id).unwrap_or(false);
        let next = d.sequential_focus_target(doc, Some(hidden), false, &area).unwrap();
        assert_eq!(next, d.focus_delegate(ids(&d, doc, &["df"])[0], &area).unwrap());
        assert!(d.focus_delegate(ids(&d, doc, &["host"])[0], &area).is_none());
    }
}
