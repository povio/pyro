//! Top-layer state of elements: modal dialogs (`:modal`) and showing
//! popovers (`:popover-open`), the top layer's order, the auto and hint
//! popover stacks and each showing popover's invoker. Removing an element
//! from its tree closes it and takes it out of all of them; `moveBefore`
//! keeps them.
//!
//! The topmost modal dialog blocks its document: everything outside it is
//! inert ([`Dom::is_inert`], and the computed `inert` the hit test and the
//! focus checks read).

use rustc_hash::{FxHashMap, FxHashSet};

use crate::dom::Dom;
use crate::error::DomResult;
use crate::node::NodeId;

/// The enumerated states of the HTML popover attribute. A missing attribute has no state.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PopoverType {
    Auto,
    Hint,
    Manual,
}

impl PopoverType {
    pub fn from_value(value: &str) -> Self {
        if value.is_empty() || value.eq_ignore_ascii_case("auto") {
            Self::Auto
        } else if value.eq_ignore_ascii_case("hint") {
            Self::Hint
        } else {
            Self::Manual
        }
    }

    pub fn keyword(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Hint => "hint",
            Self::Manual => "manual",
        }
    }
}

/// Which popover stack a showing popover belongs to (HTML "popover
/// attribute" states that have light dismiss).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PopoverStack {
    Auto,
    Hint,
}

#[derive(Default)]
pub(crate) struct TopLayer {
    /// Dialogs shown with `showModal()`.
    modal: FxHashSet<NodeId>,
    /// Elements whose fullscreen flag is set (`:fullscreen`; the embedder keeps the stack).
    fullscreen: FxHashSet<NodeId>,
    /// Popovers whose visibility state is "showing".
    popovers: FxHashSet<NodeId>,
    /// Showing elements whose popover attribute changed state during a DOM write.
    changed_popovers: Vec<NodeId>,
    /// The document's top layer, bottom first: modal dialogs and showing
    /// popovers in the order they entered it.
    order: Vec<NodeId>,
    /// Each element's index in `order`, so its rank (asked per element of
    /// every render snapshot and hit test) costs no scan of the top layer.
    /// Entering on top and leaving from the top keep it; leaving from below
    /// marks it stale (`None`), and the next read rebuilds it once, so
    /// hiding many popovers in a row costs no renumbering each.
    rank: std::cell::RefCell<Option<FxHashMap<NodeId, usize>>>,
    /// Showing `popover=auto` popovers, bottom first, and the selects whose
    /// picker is open (it is an auto popover).
    auto_stack: Vec<NodeId>,
    /// Selects (customizable, `appearance: base-select`) whose picker is
    /// open: they match `:open` ([`Dom::set_select_picker_open`]).
    pickers: FxHashSet<NodeId>,
    /// Showing `popover=hint` popovers, bottom first.
    hint_stack: Vec<NodeId>,
    /// The element that showed each showing popover (`popovertarget`,
    /// `commandfor`, `showPopover({source})`).
    invokers: FxHashMap<NodeId, NodeId>,
    /// Elements no longer modal or showing that stay in `order` while an `overlay` transition
    /// runs (css-position-4 "remove from the top layer later"; [`Dom::keep_in_top_layer`]).
    pending: FxHashSet<NodeId>,
}

impl TopLayer {
    /// Puts `el` on top of the top layer (out of its place first, if in it).
    fn enter(&mut self, el: NodeId) {
        self.pending.remove(&el);
        self.exit(el);
        if let Some(rank) = self.rank.get_mut() {
            rank.insert(el, self.order.len());
        }
        self.order.push(el);
    }

    /// Takes `el` out of the top layer's order; the ones above it move down.
    fn exit(&mut self, el: NodeId) {
        if !self.pending.is_empty() && self.pending.contains(&el) {
            return;
        }
        let at = match self.rank.get_mut() {
            Some(rank) => rank.remove(&el),
            None => self.order.iter().position(|&e| e == el),
        };
        let Some(at) = at else { return };
        self.order.remove(at);
        if at != self.order.len() {
            *self.rank.get_mut() = None;
        }
    }

    /// `el`'s index in `order`.
    fn rank_of(&self, el: NodeId) -> Option<usize> {
        let mut rank = self.rank.borrow_mut();
        let rank = rank.get_or_insert_with(|| self.order.iter().enumerate().map(|(i, &e)| (e, i)).collect());
        rank.get(&el).copied()
    }

    fn leave(&mut self, el: NodeId) {
        self.exit(el);
        self.auto_stack.retain(|&e| e != el);
        self.hint_stack.retain(|&e| e != el);
        self.invokers.remove(&el);
    }

    fn stack(&mut self, which: PopoverStack) -> &mut Vec<NodeId> {
        match which {
            PopoverStack::Auto => &mut self.auto_stack,
            PopoverStack::Hint => &mut self.hint_stack,
        }
    }
}

impl Dom {
    /// Sets or clears a dialog's "is modal" flag; a modal dialog enters the
    /// top layer above everything in it.
    pub fn set_modal(&mut self, el: NodeId, modal: bool) -> DomResult<()> {
        self.check(el)?;
        let changed = if modal { self.top_layer.modal.insert(el) } else { self.top_layer.modal.remove(&el) };
        if changed {
            let tl = &mut self.top_layer;
            if modal {
                tl.enter(el);
            } else {
                tl.exit(el);
            }
            // An un-annotated bump: styles that read `:modal`, the inertness
            // the modal causes and the paint order start over.
            self.bump();
        }
        Ok(())
    }

    /// Sets or clears an element's fullscreen flag (Fullscreen: `:fullscreen` matches it).
    pub fn set_fullscreen(&mut self, el: NodeId, fullscreen: bool) -> DomResult<()> {
        self.check(el)?;
        let changed =
            if fullscreen { self.top_layer.fullscreen.insert(el) } else { self.top_layer.fullscreen.remove(&el) };
        if changed {
            // Styles that read `:fullscreen` start over.
            self.bump();
        }
        Ok(())
    }

    /// Whether the element's fullscreen flag is set.
    pub fn is_fullscreen(&self, el: NodeId) -> bool {
        self.top_layer.fullscreen.contains(&el)
    }

    pub(crate) fn is_fullscreen_idx(&self, idx: u32) -> bool {
        !self.top_layer.fullscreen.is_empty() && self.top_layer.fullscreen.contains(&self.id_of(idx))
    }

    /// Whether the dialog was shown with `showModal()` and not closed since.
    pub fn is_modal(&self, el: NodeId) -> bool {
        self.top_layer.modal.contains(&el)
    }

    /// pyro: whether the element at `idx` is a dialog in the top layer: modal, or kept there
    /// after closing for its `overlay` transition ([`Dom::keep_in_top_layer`]). The UA sheet
    /// lays such a dialog out as a modal one (`:-moz-topmost-modal`, Chromium's
    /// `:-internal-dialog-in-top-layer`; WPT css-position `overlay/overlay-transition-dialog`).
    pub(crate) fn is_dialog_in_top_layer_idx(&self, idx: u32) -> bool {
        let tl = &self.top_layer;
        if tl.modal.is_empty() && tl.pending.is_empty() {
            return false;
        }
        let id = self.id_of(idx);
        let dialog = self.n(idx).element().is_some_and(|e| e.is_html() && &*e.name.local == "dialog");
        dialog && (tl.modal.contains(&id) || tl.pending.contains(&id))
    }

    /// pyro: makes `el` the scope of an active element-scoped view transition (`Some`), or no
    /// longer one (`None`). The UA sheet gives a scope `view-transition-name: root` and
    /// `view-transition-scope: all` (`:-pyro-active-view-transition-scope`, as in Chrome). A
    /// scope that keeps its size (`Some(true)`, from the start of its transition until its update
    /// callback is done) is laid out at its border-box size from before (css-view-transitions-2:
    /// size containment with the last remembered size; not part of its computed style).
    pub fn set_view_transition_scope(&mut self, el: NodeId, state: Option<bool>) {
        let changed = match state {
            Some(contained) => self.view_transition_scopes.insert(el, contained) != Some(contained),
            None => self.view_transition_scopes.remove(&el).is_some(),
        };
        if changed {
            // (A state change of the element alone: its render state attribute.)
            self.bump();
            if let Ok(i) = self.check(el) {
                self.note(crate::dom::Change::State(i));
            }
        }
    }

    /// pyro: whether the element at `idx` is the scope of an active element-scoped view
    /// transition (`Some`), and whether it keeps its size ([`Dom::set_view_transition_scope`]).
    pub(crate) fn view_transition_scope_idx(&self, idx: u32) -> Option<bool> {
        if self.view_transition_scopes.is_empty() {
            return None;
        }
        self.view_transition_scopes.get(&self.id_of(idx)).copied()
    }

    pub(crate) fn is_modal_idx(&self, idx: u32) -> bool {
        !self.top_layer.modal.is_empty() && self.top_layer.modal.contains(&self.id_of(idx))
    }

    /// The modal dialog highest in the top layer: the one that blocks the
    /// document.
    pub fn topmost_modal(&self) -> Option<NodeId> {
        if self.top_layer.modal.is_empty() {
            return None;
        }
        // The modal dialogs are a handful, the top layer can be large.
        let tl = &self.top_layer;
        tl.modal.iter().filter_map(|&e| Some((tl.rank_of(e)?, e))).max_by_key(|&(r, _)| r).map(|(_, e)| e)
    }

    /// Sets a popover's visibility state: showing (`true`) or hidden. A
    /// showing popover enters the top layer; a hidden one leaves it and its
    /// popover stack.
    pub fn set_popover_showing(&mut self, el: NodeId, showing: bool) -> DomResult<()> {
        self.check(el)?;
        let changed = if showing { self.top_layer.popovers.insert(el) } else { self.top_layer.popovers.remove(&el) };
        if changed {
            if showing {
                self.top_layer.enter(el);
            } else {
                self.top_layer.leave(el);
            }
            // An un-annotated bump: styles that read `:popover-open` start over.
            self.bump();
        }
        Ok(())
    }

    /// Takes `el` out of the showing popovers and both popover stacks whatever its state (a node
    /// [`Dom::set_popover_showing`] refuses, or one left on a stack while not showing), so that
    /// "hide all popovers until" always makes progress.
    pub fn drop_popover(&mut self, el: NodeId) {
        if self.top_layer.popovers.remove(&el) {
            self.bump();
        }
        if self.top_layer.pickers.contains(&el) {
            let _ = self.set_select_picker_open(el, false);
        }
        self.top_layer.leave(el);
    }

    /// Opens or closes a select's picker. An open picker is an auto popover: it is on top of
    /// the auto popover stack (other auto popovers, dialogs and light dismiss close it) and the
    /// select matches `:open`. The bindings run the steps around it (`popover.rs`).
    pub fn set_select_picker_open(&mut self, el: NodeId, open: bool) -> DomResult<()> {
        let i = self.check(el)?;
        let changed = if open { self.top_layer.pickers.insert(el) } else { self.top_layer.pickers.remove(&el) };
        self.top_layer.auto_stack.retain(|&e| e != el);
        if open {
            self.top_layer.auto_stack.push(el);
        }
        if changed {
            self.bump();
            self.note(crate::dom::Change::State(i));
        }
        Ok(())
    }

    /// Whether a select's picker is open.
    pub fn is_select_picker_open(&self, el: NodeId) -> bool {
        !self.top_layer.pickers.is_empty() && self.top_layer.pickers.contains(&el)
    }

    pub(crate) fn is_select_picker_open_idx(&self, idx: u32) -> bool {
        !self.top_layer.pickers.is_empty() && self.top_layer.pickers.contains(&self.id_of(idx))
    }

    /// Whether the popover's visibility state is "showing".
    pub fn is_popover_showing(&self, el: NodeId) -> bool {
        self.top_layer.popovers.contains(&el)
    }

    pub(crate) fn is_popover_showing_idx(&self, idx: u32) -> bool {
        !self.top_layer.popovers.is_empty() && self.top_layer.popovers.contains(&self.id_of(idx))
    }

    pub(crate) fn popover_attribute_changed(&mut self, idx: u32, old: Option<&str>, new: Option<&str>) {
        if self.is_popover_showing_idx(idx) && old.map(PopoverType::from_value) != new.map(PopoverType::from_value) {
            let el = self.id_of(idx);
            if !self.top_layer.changed_popovers.contains(&el) {
                self.top_layer.changed_popovers.push(el);
                self.touch_hook();
            }
        }
    }

    pub fn has_changed_popovers(&self) -> bool {
        !self.top_layer.changed_popovers.is_empty()
    }

    /// Return showing elements belonging to this document. Nodes disconnected or
    /// collected since the mutation have already left the top layer.
    pub fn take_changed_popovers(&mut self, document: NodeId) -> Vec<NodeId> {
        let mut changed = Vec::new();
        for el in std::mem::take(&mut self.top_layer.changed_popovers) {
            if self.is_popover_showing(el) {
                if self.node_document(el).ok() == Some(document) {
                    changed.push(el);
                } else {
                    self.top_layer.changed_popovers.push(el);
                }
            }
        }
        changed
    }

    /// Pushes a showing popover on top of its popover stack.
    pub fn push_popover_stack(&mut self, el: NodeId, which: PopoverStack) {
        if self.top_layer.popovers.contains(&el) {
            let stack = self.top_layer.stack(which);
            stack.retain(|&e| e != el);
            stack.push(el);
        }
    }

    /// A popover stack, bottom first.
    pub fn popover_stack(&self, which: PopoverStack) -> &[NodeId] {
        match which {
            PopoverStack::Auto => &self.top_layer.auto_stack,
            PopoverStack::Hint => &self.top_layer.hint_stack,
        }
    }

    /// Records the element that showed a showing popover (its invoker).
    pub fn set_popover_invoker(&mut self, el: NodeId, invoker: Option<NodeId>) {
        match invoker {
            Some(i) if self.top_layer.popovers.contains(&el) => {
                self.top_layer.invokers.insert(el, i);
            }
            _ => {
                self.top_layer.invokers.remove(&el);
            }
        }
    }

    /// The element that showed the showing popover `el`.
    pub fn popover_invoker(&self, el: NodeId) -> Option<NodeId> {
        self.top_layer.invokers.get(&el).copied()
    }

    /// Whether any showing popover has an invoker (sequential focus navigation's fast path).
    pub(crate) fn has_popover_invokers(&self) -> bool {
        !self.top_layer.invokers.is_empty()
    }

    /// Each showing popover with an invoker, and that invoker.
    pub(crate) fn popover_invoker_pairs(&self) -> Vec<(NodeId, NodeId)> {
        self.top_layer.invokers.iter().map(|(&popover, &invoker)| (popover, invoker)).collect()
    }

    pub fn popover_anchor_keys(&self) -> Vec<(u64, u64)> {
        use crate::snapshot::render_key;
        self.top_layer.invokers.iter().map(|(&popover, &source)| (render_key(popover), render_key(source))).collect()
    }

    /// Keeps `el` in the top layer when it next stops being modal or showing, until
    /// [`Dom::finish_top_layer_exit`]: an `overlay` transition keeps it rendered there
    /// (css-position-4 §4.1). Its popover stack and invoker go as usual.
    pub fn keep_in_top_layer(&mut self, el: NodeId) {
        if self.top_layer.rank_of(el).is_some() {
            self.top_layer.pending.insert(el);
        }
    }

    /// Takes `el`, kept by [`Dom::keep_in_top_layer`], out of the top layer if it is no longer
    /// modal or showing. Whether it left.
    pub fn finish_top_layer_exit(&mut self, el: NodeId) -> bool {
        if !self.top_layer.pending.remove(&el) {
            return false;
        }
        if self.top_layer.modal.contains(&el) || self.top_layer.popovers.contains(&el) {
            return false;
        }
        self.top_layer.exit(el);
        self.bump();
        true
    }

    /// The elements [`Dom::keep_in_top_layer`] keeps in the top layer that are no longer modal or
    /// showing.
    pub fn top_layer_leaving(&self) -> Vec<NodeId> {
        let tl = &self.top_layer;
        tl.pending.iter().copied().filter(|e| !tl.modal.contains(e) && !tl.popovers.contains(e)).collect()
    }

    /// The top layer, bottom first.
    pub fn top_layer(&self) -> &[NodeId] {
        &self.top_layer.order
    }

    /// The position of `el` in the top layer (0 at the bottom).
    pub(crate) fn top_layer_rank(&self, el: NodeId) -> Option<usize> {
        if self.top_layer.order.is_empty() {
            return None;
        }
        self.top_layer.rank_of(el)
    }

    /// Whether `el` is inert: it or a shadow-including ancestor has the
    /// `inert` attribute, or a modal dialog blocks its document and `el` is
    /// outside the topmost one.
    pub fn is_inert(&self, el: NodeId) -> bool {
        let Ok(start) = self.check(el) else { return false };
        if let Some(modal) = self.topmost_modal() {
            if let Ok(m) = self.check(modal) {
                if self.n(m).owner_doc == self.n(start).owner_doc && !self.shadow_including_inclusive_ancestor(m, start)
                {
                    return true;
                }
            }
        }
        let mut i = start;
        loop {
            if self
                .n(i)
                .element()
                .is_some_and(|e| e.is_html() && e.attr_local(&markup5ever::local_name!("inert")).is_some())
            {
                return true;
            }
            let parent = self.n(i).parent;
            i = match &self.n(i).kind {
                crate::node::NodeKind::Fragment(f) if f.shadow.is_some() => f.host,
                _ if parent != crate::node::NIL => parent,
                _ => return false,
            };
        }
    }

    /// Whether the topmost modal dialog blocks `el` and `el` does not lead
    /// to it: in the dialog's document, neither inside the dialog nor a
    /// shadow-including ancestor of it. One empty-set check without a
    /// modal dialog.
    pub fn blocked_by_modal(&self, el: NodeId) -> bool {
        let Some(modal) = self.topmost_modal() else { return false };
        let (Ok(m), Ok(e)) = (self.check(modal), self.check(el)) else { return false };
        self.n(m).owner_doc == self.n(e).owner_doc
            && !self.shadow_including_inclusive_ancestor(m, e)
            && !self.shadow_including_inclusive_ancestor(e, m)
    }

    /// Element `i` lost its `open` attribute: a modal dialog is no longer
    /// modal (it would otherwise keep its document inert while hidden).
    pub(crate) fn top_layer_open_removed(&mut self, i: u32) {
        if self.top_layer.modal.is_empty() {
            return;
        }
        let id = self.id_of(i);
        if self.top_layer.modal.contains(&id) {
            let _ = self.set_modal(id, false);
        }
    }

    /// HTML removing steps: a removed dialog is no longer modal and a removed
    /// popover is hidden (no events), as are those inside the removed subtree.
    pub(crate) fn top_layer_before_remove(&mut self, node: u32) {
        if self.top_layer.modal.is_empty()
            && self.top_layer.popovers.is_empty()
            && self.top_layer.pending.is_empty()
            && self.top_layer.pickers.is_empty()
            && self.top_layer.fullscreen.is_empty()
        {
            return;
        }
        // The popovers this removal hides: those whose invokers they hold are hidden with them.
        let mut hidden: Vec<NodeId> = Vec::new();
        for i in self.shadow_including_subtree(node) {
            let id = self.id_of(i);
            // Fullscreen "removing steps": a removed element is no longer fullscreen (its
            // document's fullscreen stack drops it when next read).
            if self.top_layer.fullscreen.remove(&id) {
                self.bump();
            }
            // A removed select's picker closes.
            if self.top_layer.pickers.contains(&id) {
                let _ = self.set_select_picker_open(id, false);
            }
            if self.top_layer.pending.remove(&id) {
                self.top_layer.exit(id);
                self.bump();
            }
            let was_modal = self.top_layer.modal.remove(&id);
            // HTML "hide popover" (no events) of a removed showing popover closes the popovers
            // above it in its stack first.
            if self.top_layer.popovers.contains(&id) {
                for which in [PopoverStack::Auto, PopoverStack::Hint] {
                    let stack = self.top_layer.stack(which);
                    if let Some(at) = stack.iter().position(|&e| e == id) {
                        let above: Vec<NodeId> = stack[at + 1..].to_vec();
                        for el in above.into_iter().rev() {
                            self.top_layer.popovers.remove(&el);
                            self.top_layer.leave(el);
                            hidden.push(el);
                        }
                    }
                }
            }
            let was_showing = self.top_layer.popovers.remove(&id);
            if was_showing {
                hidden.push(id);
            }
            if was_modal || was_showing {
                self.top_layer.leave(id);
                self.bump();
            }
        }
        // Popovers invoked from inside a popover this removal hid (a hint whose invoker was in a
        // removed auto popover) are hidden with it, and those invoked from them. Removing only an
        // invoker closes nothing (WPT popovers/popover-target-element-disabled).
        while !hidden.is_empty() {
            let holders: Vec<u32> = hidden.iter().filter_map(|&h| self.check(h).ok()).collect();
            let invoked: Vec<NodeId> = self
                .top_layer
                .invokers
                .iter()
                .filter(|(_, invoker)| {
                    self.check(**invoker)
                        .is_ok_and(|inv| holders.iter().any(|&h| self.shadow_including_inclusive_ancestor(h, inv)))
                })
                .map(|(popover, _)| *popover)
                .filter(|popover| self.top_layer.popovers.contains(popover))
                .collect();
            for &el in &invoked {
                self.top_layer.popovers.remove(&el);
                self.top_layer.leave(el);
            }
            if !invoked.is_empty() {
                self.bump();
            }
            hidden = invoked;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::DocumentKind;

    #[test]
    fn dropping_stale_popover_exposes_next_stack_entry() {
        let mut dom = Dom::new();
        let doc = dom.create_document(DocumentKind::Html, "about:blank");
        let next = dom.create_element(doc, "div").unwrap();
        let stale = dom.create_element(doc, "div").unwrap();
        for popover in [next, stale] {
            dom.set_popover_showing(popover, true).unwrap();
            dom.push_popover_stack(popover, PopoverStack::Auto);
        }
        dom.set_popover_invoker(stale, Some(next));
        assert!(dom.top_layer.popovers.remove(&stale));

        dom.set_popover_showing(stale, false).unwrap();
        assert_eq!(dom.popover_stack(PopoverStack::Auto), [next, stale]);

        dom.drop_popover(stale);
        assert_eq!(dom.popover_stack(PopoverStack::Auto), [next]);
        assert_eq!(dom.top_layer(), [next]);
        assert_eq!(dom.popover_invoker(stale), None);
        assert!(dom.is_popover_showing(next));
    }
}
