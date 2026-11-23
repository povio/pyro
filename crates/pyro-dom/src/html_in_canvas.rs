//! HTML-in-Canvas (WICG html-in-canvas): what the DOM journal says changed,
//! for the `paint` events of `<canvas content=drawable>` elements.

use crate::dom::{Change, Dom};
use crate::node::{NodeId, NIL};

/// How a change touched a node.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Touch {
    /// The node itself changed: an attribute or form-control state.
    Node,
    /// The node's children changed: one was inserted or removed, or the
    /// text of a child text node changed.
    Children,
    /// The node was inserted (its subtree may hold new canvases).
    Inserted,
}

impl Dom {
    /// The nodes the changes made after `version` touched, in journal order
    /// (dead nodes left out); `None` when the journal no longer covers
    /// `version` and the caller must treat everything as changed.
    pub fn touched_nodes_since(&self, version: u64) -> Option<Vec<(NodeId, Touch)>> {
        let changes = self.changes_since(version)?;
        let live = |i: u32| i != NIL && self.check(self.id_of(i)).is_ok();
        let mut out = Vec::new();
        for change in changes {
            match change {
                Change::Attr(i, _, _) | Change::State(i) if live(i) => out.push((self.id_of(i), Touch::Node)),
                Change::Inserted(parent, node) => {
                    if live(parent) {
                        out.push((self.id_of(parent), Touch::Children));
                    }
                    if live(node) {
                        out.push((self.id_of(node), Touch::Inserted));
                    }
                }
                Change::Removed(parent, _, _) if live(parent) => out.push((self.id_of(parent), Touch::Children)),
                Change::Text(i) if live(i) => {
                    let parent = self.n(i).parent;
                    if live(parent) {
                        out.push((self.id_of(parent), Touch::Children));
                    }
                }
                _ => {}
            }
        }
        Some(out)
    }
}
