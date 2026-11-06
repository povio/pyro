//! `<details>` attribute steps kept at mutation time: every change of the
//! `open` attribute's presence is queued for the embedder's `toggle` event
//! (HTML "queue a details toggle event task"), and the `name` attribute's
//! exclusive accordion closes the other open details of the group.
//!
//! Pages without `<details>` pay one atom comparison per attribute write.
//! The same hook takes a modal dialog out of the top layer when its `open`
//! attribute is removed.

use markup5ever::{local_name, ns, QualName};

use crate::dom::Dom;
use crate::node::{NodeId, NIL};

#[derive(Default)]
pub(crate) struct DetailsState {
    /// The details elements whose `open` attribute appeared or went, with
    /// whether it was open before, oldest first. Each entry holds a
    /// reference to its node until the embedder of its document takes it
    /// ([`Dom::take_details_toggles`]).
    toggles: Vec<(NodeId, bool)>,
    /// Some `details` element has had a `name` attribute: insertion then
    /// checks exclusivity. Never reset (a page that uses accordions keeps
    /// paying the one subtree check per insertion).
    named: bool,
}

impl Dom {
    fn is_html_details(&self, i: u32) -> bool {
        self.n(i).element().is_some_and(|e| e.is_html() && e.name.local == local_name!("details"))
    }

    /// After attribute `name` of element `i` was added
    /// (`Some(true)`), removed (`Some(false)`) or changed value (`None`).
    pub(crate) fn details_after_attr(&mut self, i: u32, name: &QualName, presence: Option<bool>) {
        let open = name.local == local_name!("open");
        if !(open || name.local == local_name!("name")) || name.ns != ns!() {
            return;
        }
        if open && presence == Some(false) {
            // A modal dialog whose `open` goes leaves the top layer.
            self.top_layer_open_removed(i);
        }
        if !self.is_html_details(i) {
            return;
        }
        if open {
            let Some(added) = presence else { return };
            self.queue_details_toggle(i, !added);
            if added {
                self.close_other_details(i);
            }
        } else {
            if presence == Some(true) {
                self.details.named = true;
            }
            if presence != Some(false) {
                self.close_details_if_needed(i);
            }
        }
    }

    /// The parser made details element `i` with its attributes.
    pub(crate) fn details_created(&mut self, i: u32) {
        let Some(e) = self.n(i).element() else { return };
        let (open, named) =
            (e.attr_local(&local_name!("open")).is_some(), e.attr_local(&local_name!("name")).is_some());
        self.details.named |= named;
        if open {
            self.queue_details_toggle(i, false);
        }
    }

    fn queue_details_toggle(&mut self, i: u32, was_open: bool) {
        let id = self.id_of(i);
        if self.add_ref(id).is_ok() {
            self.details.toggles.push((id, was_open));
            self.touch_hook();
        }
    }

    /// Whether details toggles are queued (for any document).
    pub fn has_details_toggles(&self) -> bool {
        !self.details.toggles.is_empty()
    }

    /// Takes the details toggles of the details now in `document`: each
    /// node with whether it was open before. The caller releases each node
    /// once it is done with it. Toggles of details in documents no embedder
    /// watches (template contents, `DOMParser` documents, a fragment
    /// parser's document left behind) are dropped: their tasks would never
    /// run. Other watched documents' toggles stay for their embedder. The
    /// node document is read now, not when queued: fragment parsing creates
    /// elements in an inert document and adopts them on insertion.
    pub fn take_details_toggles(&mut self, document: NodeId) -> Vec<(NodeId, bool)> {
        let mut out = Vec::new();
        let mut keep = Vec::new();
        for (node, was_open) in std::mem::take(&mut self.details.toggles) {
            let doc = self.check(node).ok().map(|i| self.id_of(self.n(i).owner_doc));
            if doc == Some(document) {
                out.push((node, was_open));
            } else if doc.is_some_and(|d| self.is_watched_document(d)) {
                keep.push((node, was_open));
            } else {
                self.release(node);
            }
        }
        self.details.toggles = keep;
        out
    }

    /// The details name group of `i` without `i`: the other details
    /// elements of its tree with the same non-empty `name`.
    fn details_group(&self, i: u32) -> Vec<u32> {
        let Some(name) = self.n(i).element().and_then(|e| e.attr_local(&local_name!("name"))) else {
            return Vec::new();
        };
        if name.is_empty() {
            return Vec::new();
        }
        let root = self.root_idx(i);
        let mut out = Vec::new();
        let mut cur = root;
        while cur != NIL {
            if cur != i
                && self.is_html_details(cur)
                && self.n(cur).element().and_then(|e| e.attr_local(&local_name!("name"))) == Some(name)
            {
                out.push(cur);
            }
            cur = self.next_in_tree(cur, root);
        }
        out
    }

    fn next_in_tree(&self, idx: u32, root: u32) -> u32 {
        let n = self.n(idx);
        if n.first_child != NIL {
            return n.first_child;
        }
        let mut cur = idx;
        while cur != root && cur != NIL {
            let next = self.n(cur).next;
            if next != NIL {
                return next;
            }
            cur = self.n(cur).parent;
        }
        NIL
    }

    fn is_open(&self, i: u32) -> bool {
        self.n(i).element().is_some_and(|e| e.attr_local(&local_name!("open")).is_some())
    }

    fn remove_open(&mut self, i: u32) {
        let id = self.id_of(i);
        let _ = self.remove_attribute(id, "open");
    }

    /// HTML details "attribute changed steps" for `open` added: the other
    /// open details of the name group close.
    fn close_other_details(&mut self, i: u32) {
        if !self.details.named {
            return;
        }
        for other in self.details_group(i) {
            if self.is_open(other) {
                self.remove_open(other);
            }
        }
    }

    /// HTML "ensure details exclusivity by closing the given element if
    /// needed": an open details of a group that already has another open
    /// one closes.
    fn close_details_if_needed(&mut self, i: u32) {
        if !self.is_open(i) {
            return;
        }
        if self.details_group(i).into_iter().any(|other| self.is_open(other)) {
            self.remove_open(i);
        }
    }

    /// HTML details insertion steps, for the details elements of the
    /// inserted subtree `node`. Nothing to do until a details has a name.
    pub(crate) fn details_after_insert(&mut self, node: u32) {
        if !self.details.named {
            return;
        }
        let mut found = Vec::new();
        let mut cur = node;
        while cur != NIL {
            if self.is_html_details(cur) && self.is_open(cur) {
                found.push(cur);
            }
            cur = self.next_in_tree(cur, node);
        }
        for d in found {
            self.close_details_if_needed(d);
        }
    }
}
