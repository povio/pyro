//! Labels (HTML §4.10.4): a `<label>`'s labeled control, and a control's
//! labels (`element.labels`), answered from the tree scope's index of
//! `<label>` elements by `for` (kept at mutation time, see `tree_scope`)
//! and a walk up the control's ancestors, not a walk of the document.

use std::cmp::Ordering;

use markup5ever::local_name;

use crate::dom::Dom;
use crate::error::DomResult;
use crate::forms::InputType;
use crate::node::{NodeId, NIL};
use crate::tree_scope::label_for_of;

impl Dom {
    /// Whether `idx` is a labelable element (HTML §4.10.2): a button,
    /// meter, output, progress, select, textarea, a non-hidden input or a
    /// form-associated custom element.
    pub(crate) fn labelable_idx(&self, idx: u32) -> bool {
        let Some(e) = self.n(idx).element() else { return false };
        if !e.is_html() {
            return false;
        }
        match &e.name.local {
            l if *l == local_name!("input") => self.input_type_idx(idx) != InputType::Hidden,
            l if *l == local_name!("button")
                || *l == local_name!("meter")
                || *l == local_name!("output")
                || *l == local_name!("progress")
                || *l == local_name!("select")
                || *l == local_name!("textarea") =>
            {
                true
            }
            _ => self.is_face_idx(idx),
        }
    }

    /// Whether `id` is a labelable element.
    pub fn is_labelable(&self, id: NodeId) -> DomResult<bool> {
        Ok(self.labelable_idx(self.check(id)?))
    }

    /// The labeled control of the `<label>` `label`: with a `for`, the
    /// first element of its tree with that id when labelable; without one,
    /// its first labelable descendant. `None` for neither, and for a node
    /// that is no HTML `<label>`.
    pub fn labeled_control(&self, label: NodeId) -> DomResult<Option<NodeId>> {
        let l = self.check(label)?;
        let is_label = self.n(l).element().is_some_and(|e| e.is_html() && e.name.local == local_name!("label"));
        if !is_label {
            return Ok(None);
        }
        let c = self.labeled_control_idx(l);
        Ok((c != NIL).then(|| self.id_of(c)))
    }

    fn labeled_control_idx(&self, l: u32) -> u32 {
        let e = self.n(l).element().expect("label element");
        if let Some(f) = e.attr_local(&local_name!("for")) {
            let target = self.first_by_id(self.root_idx(l), f);
            return if target != NIL && self.labelable_idx(target) { target } else { NIL };
        }
        let mut cur = self.n(l).first_child;
        while cur != NIL {
            if self.labelable_idx(cur) {
                return cur;
            }
            cur = self.following_in(cur, l);
        }
        NIL
    }

    /// `element.labels`: the `<label>` elements of `control`'s tree whose
    /// labeled control it is, in tree order. Empty for an element that is
    /// not labelable.
    pub fn labels(&self, control: NodeId) -> DomResult<Vec<NodeId>> {
        let c = self.check(control)?;
        if !self.labelable_idx(c) {
            return Ok(Vec::new());
        }
        let root = self.root_idx(c);
        let mut out: Vec<u32> = Vec::new();
        // Labels naming it by `for`: those whose `for` is its id, when it is
        // the first element of the tree with that id.
        if let Some(id) = self.n(c).element().and_then(|e| e.id()).filter(|id| !id.is_empty()) {
            if self.first_by_id(root, id) == c {
                match self.labels_for_indexed(root, id) {
                    Some(labels) => out.extend_from_slice(labels),
                    // A detached tree has no index: its root included.
                    None => {
                        let mut cur = root;
                        while cur != NIL {
                            if self.n(cur).element().and_then(label_for_of) == Some(id) {
                                out.push(cur);
                            }
                            cur = self.following_in(cur, root);
                        }
                    }
                }
            }
        }
        // Labels wrapping it without a `for`, when it is their first
        // labelable descendant.
        let mut a = self.n(c).parent;
        while a != NIL {
            let wraps = self.n(a).element().is_some_and(|e| {
                e.is_html() && e.name.local == local_name!("label") && e.attr_local(&local_name!("for")).is_none()
            });
            if wraps && self.labeled_control_idx(a) == c {
                out.push(a);
            }
            a = self.n(a).parent;
        }
        out.sort_unstable_by(|&x, &y| {
            if x == y {
                Ordering::Equal
            } else if self.tree_order_before(x, y) {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        });
        Ok(out.into_iter().map(|i| self.id_of(i)).collect())
    }
}
