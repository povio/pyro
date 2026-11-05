//! Top-layer state of elements: modal dialogs (`:modal`) and showing
//! popovers (`:popover-open`). Removing an element from its tree closes
//! both; `moveBefore` keeps them.

use rustc_hash::FxHashSet;

use crate::dom::Dom;
use crate::error::DomResult;
use crate::node::NodeId;

#[derive(Default)]
pub(crate) struct TopLayer {
    /// Dialogs shown with `showModal()`.
    modal: FxHashSet<NodeId>,
    /// Popovers whose visibility state is "showing".
    popovers: FxHashSet<NodeId>,
}

impl Dom {
    /// Sets or clears a dialog's "is modal" flag.
    pub fn set_modal(&mut self, el: NodeId, modal: bool) -> DomResult<()> {
        self.check(el)?;
        let changed = if modal { self.top_layer.modal.insert(el) } else { self.top_layer.modal.remove(&el) };
        if changed {
            // An un-annotated bump: styles that read `:modal` start over.
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

    /// Sets a popover's visibility state: showing (`true`) or hidden.
    pub fn set_popover_showing(&mut self, el: NodeId, showing: bool) -> DomResult<()> {
        self.check(el)?;
        let changed = if showing { self.top_layer.popovers.insert(el) } else { self.top_layer.popovers.remove(&el) };
        if changed {
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

    /// Whether `el` is inert because it or a shadow-including ancestor has
    /// the `inert` attribute (modal dialogs block nothing here).
    pub fn is_inert(&self, el: NodeId) -> bool {
        let Ok(mut i) = self.check(el) else { return false };
        loop {
            if self.n(i).element().is_some_and(|e| e.is_html() && e.attr("inert").is_some()) {
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
                self.bump();
            }
        }
    }
}
