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
    /// Popovers whose visibility state is "showing".
    popovers: FxHashSet<NodeId>,
    /// The document's top layer, bottom first: modal dialogs and showing
    /// popovers in the order they entered it. A handful of entries.
    order: Vec<NodeId>,
    /// Showing `popover=auto` popovers, bottom first.
    auto_stack: Vec<NodeId>,
    /// Showing `popover=hint` popovers, bottom first.
    hint_stack: Vec<NodeId>,
    /// The element that showed each showing popover (`popovertarget`,
    /// `commandfor`, `showPopover({source})`).
    invokers: FxHashMap<NodeId, NodeId>,
}

impl TopLayer {
    fn leave(&mut self, el: NodeId) {
        self.order.retain(|&e| e != el);
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
            tl.order.retain(|&e| e != el);
            if modal {
                tl.order.push(el);
            }
            // An un-annotated bump: styles that read `:modal`, the inertness
            // the modal causes and the paint order start over.
            self.bump();
        }
        Ok(())
    }

    /// Whether the dialog was shown with `showModal()` and not closed since.
    pub fn is_modal(&self, el: NodeId) -> bool {
        self.top_layer.modal.contains(&el)
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
        self.top_layer.order.iter().rev().copied().find(|e| self.top_layer.modal.contains(e))
    }

    /// Sets a popover's visibility state: showing (`true`) or hidden. A
    /// showing popover enters the top layer; a hidden one leaves it and its
    /// popover stack.
    pub fn set_popover_showing(&mut self, el: NodeId, showing: bool) -> DomResult<()> {
        self.check(el)?;
        let changed = if showing { self.top_layer.popovers.insert(el) } else { self.top_layer.popovers.remove(&el) };
        if changed {
            if showing {
                self.top_layer.order.retain(|&e| e != el);
                self.top_layer.order.push(el);
            } else {
                self.top_layer.leave(el);
            }
            // An un-annotated bump: styles that read `:popover-open` start over.
            self.bump();
        }
        Ok(())
    }

    /// Whether the popover's visibility state is "showing".
    pub fn is_popover_showing(&self, el: NodeId) -> bool {
        self.top_layer.popovers.contains(&el)
    }

    pub(crate) fn is_popover_showing_idx(&self, idx: u32) -> bool {
        !self.top_layer.popovers.is_empty() && self.top_layer.popovers.contains(&self.id_of(idx))
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

    /// The top layer, bottom first.
    pub fn top_layer(&self) -> &[NodeId] {
        &self.top_layer.order
    }

    /// The position of `el` in the top layer (0 at the bottom).
    pub(crate) fn top_layer_rank(&self, el: NodeId) -> Option<usize> {
        if self.top_layer.order.is_empty() {
            return None;
        }
        self.top_layer.order.iter().position(|&e| e == el)
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
        if self.top_layer.modal.is_empty() && self.top_layer.popovers.is_empty() {
            return;
        }
        for i in self.shadow_including_subtree(node) {
            let id = self.id_of(i);
            let was_modal = self.top_layer.modal.remove(&id);
            let was_showing = self.top_layer.popovers.remove(&id);
            if was_modal || was_showing {
                self.top_layer.leave(id);
                self.bump();
            }
        }
    }
}
