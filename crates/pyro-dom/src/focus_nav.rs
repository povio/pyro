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
//! A showing popover that has a popover invoker owns a scope too, and its
//! place in sequential order is right after its invoker rather than at its
//! position in the tree (HTML "popover invoker"; Chrome's FocusController):
//! Tab from the invoker enters the popover, and leaving the popover's scope
//! continues after the invoker.
//!
//! Layout is not visible here: callers pass `area`, the "focusable area"
//! test (focusable, not inert, being rendered). Only Tab and `focus()` on a
//! `delegatesFocus` host run this code; nothing is kept between calls.

use markup5ever::{local_name, ns};

use crate::error::DomResult;
use crate::forms::parse_html_int;

use crate::dom::Dom;
use crate::node::{shadow_flags, NodeId, NodeKind, NIL};

impl Dom {
    pub(crate) fn tabindex_attr(&self, i: u32) -> Option<i32> {
        let value = self.n(i).element()?.attr_local(&local_name!("tabindex"))?;
        i32::try_from(parse_html_int(value)?).ok()
    }

    /// HTML's tabIndex getter has tag-specific defaults even on disconnected,
    /// disabled or hidden elements. Focusability is checked separately.
    pub fn tab_index(&self, id: NodeId) -> DomResult<i32> {
        let i = self.check(id)?;
        if let Some(index) = self.tabindex_attr(i) {
            return Ok(index);
        }
        let el = self.element(id)?;
        let default_zero = if el.is_html() {
            match &*el.name.local {
                "a" | "area" | "button" | "frame" | "iframe" | "input" | "object" | "select" | "textarea" => true,
                "summary" => self.is_details_summary(id),
                _ => false,
            }
        } else {
            el.name.local == local_name!("a") && (el.namespace() == &ns!(svg) || el.namespace() == &ns!(mathml))
        };
        Ok(if default_zero { 0 } else { -1 })
    }

    /// Whether this is the first HTML summary child of an HTML details element.
    pub fn is_details_summary(&self, el: NodeId) -> bool {
        let Ok(Some(parent)) = self.parent(el) else { return false };
        self.element(parent).is_ok_and(|e| e.is_html() && e.name.local == local_name!("details"))
            && self.element_children(parent).ok().and_then(|mut children| {
                children.find(|&child| self.element(child).is_ok_and(|e| e.is_html() && e.name.local == local_name!("summary")))
            }) == Some(el)
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

    /// Tab traversal without an external style engine (DOM-only callers).
    pub fn sequential_focus_target(
        &self, doc: NodeId, current: Option<NodeId>, backward: bool, area: &dyn Fn(NodeId) -> bool,
    ) -> Option<NodeId> {
        FocusNavigation::new(self, None).next(doc, current, backward, area)
    }

    pub fn sequential_focus_target_with_layout(
        &self, doc: NodeId, current: Option<NodeId>, backward: bool,
        area: &dyn Fn(NodeId) -> bool, layout: &crate::layout::Layout,
    ) -> Option<NodeId> {
        FocusNavigation::new(self, Some(layout)).next(doc, current, backward, area)
    }
}

/// The reading-flow order of an element's children, as sequential
/// navigation sees it (scoped focusgroups use it for their item order).
/// Readbacks are cached for the life of the value.
pub(crate) struct ReadingOrder<'a>(FocusNavigation<'a>);

impl<'a> ReadingOrder<'a> {
    pub(crate) fn new(dom: &'a Dom, layout: &'a crate::layout::Layout) -> Self {
        Self(FocusNavigation::new(dom, Some(layout)))
    }

    /// Reorders `children` (flat-tree children of `parent`) into reading
    /// order when `parent` is a reading-flow container; leaves them alone
    /// otherwise. Non-element children are dropped from a reordered list.
    pub(crate) fn sort(&self, parent: u32, children: &mut Vec<u32>) {
        let flow = self.0.flow(parent);
        if flow == ReadingFlow::Normal {
            return;
        }
        children.retain(|&i| self.0.n(i).is_element());
        self.0.order_roots(parent, children, flow);
    }
}

use style::values::computed::ReadingFlow;
use style::values::specified::box_::{DisplayInside, DisplayOutside};
use rustc_hash::FxHashMap;

/// Per-navigation readback. It expires before listeners can mutate the DOM or styles.
struct FocusNavigation<'a> {
    dom: &'a Dom,
    layout: Option<&'a crate::layout::Layout>,
    flows: std::cell::RefCell<FxHashMap<u32, ReadingFlow>>,
    grid_contents: std::cell::RefCell<rustc_hash::FxHashSet<u32>>,
}

impl std::ops::Deref for FocusNavigation<'_> {
    type Target = Dom;
    fn deref(&self) -> &Dom { self.dom }
}

impl<'a> FocusNavigation<'a> {
    fn new(dom: &'a Dom, layout: Option<&'a crate::layout::Layout>) -> Self {
        Self { dom, layout, flows: Default::default(), grid_contents: Default::default() }
    }

    fn flow(&self, i: u32) -> ReadingFlow {
        let Some(layout) = self.layout else { return ReadingFlow::Normal };
        let mut i = i;
        let mut contents = Vec::new();
        let result = loop {
            if i == NIL || !self.n(i).is_element() { break ReadingFlow::Normal; }
            if let Some(flow) = self.flows.borrow().get(&i) { break *flow; }
            let style = layout.style().stylo_style(self.dom, self.id_of(i));
            if style.as_ref().is_some_and(|s| s.get_box().display.inside() == DisplayInside::Contents) {
                contents.push(i);
                i = self.flat_parent(i);
                continue;
            }
            let flow = style.as_ref().map_or(ReadingFlow::Normal, |s| {
                let display = s.get_box().display;
                match (s.get_position().reading_flow, display.inside()) {
                    (ReadingFlow::SourceOrder, DisplayInside::Flow) if display.outside() == DisplayOutside::Block => ReadingFlow::SourceOrder,
                    (ReadingFlow::SourceOrder, DisplayInside::FlowRoot | DisplayInside::Flex | DisplayInside::Grid) => ReadingFlow::SourceOrder,
                    (f @ (ReadingFlow::FlexFlow | ReadingFlow::FlexVisual), DisplayInside::Flex) => f,
                    (f @ (ReadingFlow::GridOrder | ReadingFlow::GridRows | ReadingFlow::GridColumns), DisplayInside::Grid) => f,
                    _ => ReadingFlow::Normal,
                }
            });
            self.flows.borrow_mut().insert(i, flow);
            break flow;
        };
        if contents.is_empty() { return result; }
        let inherited = if result == ReadingFlow::GridOrder { result } else { ReadingFlow::Normal };
        for item in contents {
            self.flows.borrow_mut().insert(item, inherited);
            if inherited == ReadingFlow::GridOrder { self.grid_contents.borrow_mut().insert(item); }
        }
        inherited
    }

    fn flat_parent(&self, i: u32) -> u32 {
        if let Some(slot) = self.find_slot(i, false) { return slot; }
        let parent = self.n(i).parent;
        if parent != NIL {
            if let NodeKind::Fragment(f) = &self.n(parent).kind {
                if f.shadow.is_some() { return f.host; }
            }
        }
        parent
    }

    fn is_grid_contents(&self, i: u32) -> bool {
        self.flow(i) == ReadingFlow::GridOrder && self.grid_contents.borrow().contains(&i)
    }

    fn roots(&self, owner: u32) -> Vec<u32> {
        if let Some(host) = self.n(owner).element().filter(|e| e.shadow_root != NIL) {
            return self.child_idx_iter(host.shadow_root).collect();
        }
        if self.is_slot(owner) {
            let assigned = self.slot_assigned(owner);
            if !assigned.is_empty() { return assigned; }
        }
        self.child_idx_iter(owner).collect()
    }

    // A contents wrapper has no grid item. Give its focus scope the earliest
    // order of its generated children, keeping the scope's descendants together.
    fn contents_order(&self, root: u32, layout: &crate::layout::Layout) -> Option<(i32, i32)> {
        let mut pending = self.roots(root);
        let mut first = None;
        while let Some(i) = pending.pop() {
            if !self.n(i).is_element() { continue; }
            if self.is_grid_contents(i) { pending.extend(self.roots(i)); continue; }
            if let Some(style) = layout.style().stylo_style(self.dom, self.id_of(i)) {
                if style.get_box().display.inside() != DisplayInside::None {
                    let key = (style.get_position().reading_order, style.get_position().order);
                    first = Some(first.map_or(key, |old: (i32, i32)| old.min(key)));
                }
            }
        }
        first
    }

    fn order_roots(&self, owner: u32, roots: &mut [u32], flow: ReadingFlow) {
        let Some(layout) = self.layout else { return };
        let mut placements = FxHashMap::default();
        if flow == ReadingFlow::FlexVisual {
            use style::computed_values::{flex_direction::T as Direction, flex_wrap::T as Wrap};
            let container = self.id_of(owner);
            if let (Some(style), Some(source)) = (layout.style().stylo_style(self.dom, container), layout.synced_geometry_source(self.dom)) {
                let position = style.get_position();
                let reverse_line = position.flex_wrap.contains(Wrap::WRAP_REVERSE);
                let reverse_item = matches!(position.flex_direction, Direction::RowReverse | Direction::ColumnReverse);
                for (key, line, item) in source.flex_reading_positions(crate::snapshot::render_key(container)) {
                    placements.insert(key, (if reverse_line { usize::MAX - line } else { line },
                        if reverse_item { usize::MAX - item } else { item }));
                }
            }
        }
        if matches!(flow, ReadingFlow::GridRows | ReadingFlow::GridColumns) {
            if let Some(source) = layout.synced_geometry_source(self.dom) {
                for (key, row, column) in source.grid_item_starts(crate::snapshot::render_key(self.id_of(owner))) {
                    let (row, column) = (usize::from(row), usize::from(column));
                    placements.insert(key, if flow == ReadingFlow::GridColumns { (column, row) } else { (row, column) });
                }
            }
        }
        roots.sort_by_cached_key(|&i| {
            let id = self.id_of(i);
            let style = layout.style().stylo_style(self.dom, id);
            if flow == ReadingFlow::GridOrder && style.as_ref().is_some_and(|s| s.get_box().display.inside() == DisplayInside::Contents) {
                if let Some((reading, order)) = self.contents_order(i, layout) { return (reading, (0, 0), order); }
            }
            let reading = style.as_ref().map_or(0, |s| s.get_position().reading_order);
            let order = if flow == ReadingFlow::SourceOrder { 0 } else { style.as_ref().map_or(0, |s| s.get_position().order) };
            let position = placements.get(&crate::snapshot::render_key(id)).copied().unwrap_or_default();
            (reading, position, order)
        });
    }

    fn is_scope_owner(&self, i: u32) -> bool {
        self.n(i).element().is_some_and(|e| e.shadow_root != NIL) || self.is_slot(i)
            || self.flow(i) != ReadingFlow::Normal || self.flow(self.n(i).parent) != ReadingFlow::Normal
            || self.flow(self.flat_parent(i)) == ReadingFlow::GridOrder
            || self.invoked_popover(i).is_some()
    }

    /// The invoker of `i` when `i` is a showing popover whose sequential place is right after
    /// its invoker: a connected element of the same document, outside the popover.
    fn invoked_popover(&self, i: u32) -> Option<u32> {
        if !self.has_popover_invokers() {
            return None;
        }
        let invoker = self.check(self.popover_invoker(self.id_of(i))?).ok()?;
        (self.n(invoker).is_element()
            && self.is_connected_idx(invoker)
            && self.n(invoker).owner_doc == self.n(i).owner_doc
            && !self.shadow_including_inclusive_ancestor(i, invoker))
        .then_some(invoker)
    }

    /// Moves each invoked popover of `items` (a scope's list) out of its tree position, and
    /// each invoked popover whose invoker is in `items` to right after that invoker.
    fn place_invoked_popovers(&self, mut items: Vec<u32>) -> Vec<u32> {
        if !self.has_popover_invokers() {
            return items;
        }
        let mut invoked: Vec<(u32, u32)> = self
            .popover_invoker_pairs()
            .into_iter()
            .filter_map(|(popover, _)| {
                let p = self.check(popover).ok()?;
                Some((p, self.invoked_popover(p)?))
            })
            .collect();
        if invoked.is_empty() {
            return items;
        }
        // Several popovers of one invoker follow it in tree order.
        invoked.sort_by(|a, b| match (a.0 == b.0, self.tree_order_before(a.0, b.0)) {
            (true, _) => std::cmp::Ordering::Equal,
            (false, true) => std::cmp::Ordering::Less,
            (false, false) => std::cmp::Ordering::Greater,
        });
        items.retain(|i| !invoked.iter().any(|&(p, _)| p == *i));
        let mut out = Vec::with_capacity(items.len() + invoked.len());
        for i in items {
            out.push(i);
            out.extend(invoked.iter().filter(|&&(_, invoker)| invoker == i).map(|&(p, _)| p));
        }
        out
    }

    /// The elements of `owner`'s scope, tabindex-ordered.
    fn scope_items(&self, owner: u32) -> Vec<u32> {
        let flow = self.flow(owner);
        let mut roots = self.roots(owner);
        if flow == ReadingFlow::GridOrder && roots.iter().any(|&i| self.is_slot(i) && self.is_grid_contents(i)) {
            let mut expanded = Vec::new();
            let mut pending = roots.into_iter().rev().collect::<Vec<_>>();
            while let Some(i) = pending.pop() {
                if self.is_slot(i) && self.is_grid_contents(i) {
                    pending.extend(self.roots(i).into_iter().rev());
                } else { expanded.push(i); }
            }
            roots = expanded;
        }
        if flow != ReadingFlow::Normal {
            roots.retain(|&i| self.n(i).is_element());
            self.order_roots(owner, &mut roots, flow);
            // Direct element children are themselves scope owners by virtue of
            // their reading-flow parent. Shadow and assigned roots can have a
            // different DOM parent and still need the regular scope walk.
            if roots.iter().all(|&i| self.n(i).parent == owner) {
                return self.place_invoked_popovers(roots);
            }
        }
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
        if flow != ReadingFlow::Normal { return self.place_invoked_popovers(out); }
        out.sort_by_key(|&i| match self.tabindex_attr(i) {
            Some(t) if t > 0 => (0, t),
            _ => (1, 0),
        });
        self.place_invoked_popovers(out)
    }

    /// The scope owner whose scope lists `el`, if `el` is in one at all
    /// (an unassigned child of a shadow host is not).
    fn scope_of(&self, el: u32) -> Option<u32> {
        // An invoked popover is listed in its invoker's scope (a chain of popovers invoked by
        // popovers is followed, a cycle of them is not).
        let mut el = el;
        for _ in 0..32 {
            match self.invoked_popover(el) {
                Some(invoker) => el = invoker,
                None => break,
            }
        }
        let mut x = el;
        loop {
            if let Some(slot) = self.find_slot(x, false) {
                if self.is_grid_contents(slot) { x = slot; continue; }
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
                NodeKind::Element(_) if self.is_slot(p) && self.is_grid_contents(p) => x = p,
                NodeKind::Element(_) if self.is_scope_owner(p) => return Some(p),
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
            && self.radio_group_target(i, area)
    }

    /// A radio button in a group is reached by sequential navigation only as
    /// its group's one stop (Chromium): the checked radio when it is a
    /// focusable area, else the group's first radio that is, in tree order.
    /// Other elements are not affected.
    fn radio_group_target(&self, i: u32, area: &dyn Fn(NodeId) -> bool) -> bool {
        let radio = self.html_local(i) == Some(&local_name!("input"))
            && self.input_type_idx(i) == crate::forms::InputType::Radio;
        if !radio {
            return true;
        }
        let group = self.radio_group(i);
        if group.len() <= 1 {
            return true;
        }
        let focusable = |r: u32| r == i || area(self.id_of(r));
        let checked = group.iter().copied().find(|&r| self.checkedness_idx(r)).filter(|&r| focusable(r));
        let target = checked.or_else(|| group.iter().copied().find(|&r| focusable(r)));
        target == Some(i)
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
    fn next(
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

}

impl Dom {
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

    /// A showing popover with an invoker is navigated right after its invoker
    /// (html/semantics/popovers/popover-focus-2.html).
    #[test]
    fn invoked_popover_follows_its_invoker() {
        let dom = Rc::new(RefCell::new(Dom::new()));
        let doc = parse_document(
            &dom,
            "about:blank",
            "<button id=b1></button><div popover id=p><button id=in1></button><button id=in2></button></div>\
             <button id=b2></button><button id=inv></button><button id=b3></button>",
        );
        let [p, inv] = ids(&dom.borrow(), doc, &["p", "inv"])[..] else { unreachable!() };
        dom.borrow_mut().set_popover_showing(p, true).unwrap();
        dom.borrow_mut().set_popover_invoker(p, Some(inv));
        let d = dom.borrow();
        assert_eq!(order(&d, doc, false), ["b1", "b2", "inv", "in1", "in2", "b3"]);
        assert_eq!(order(&d, doc, true), ["b3", "in2", "in1", "inv", "b2", "b1"]);
        drop(d);
        // Without an invoker the popover keeps its tree position.
        dom.borrow_mut().set_popover_invoker(p, None);
        assert_eq!(order(&dom.borrow(), doc, false), ["b1", "in1", "in2", "b2", "inv", "b3"]);
    }
}
