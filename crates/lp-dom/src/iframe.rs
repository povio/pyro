//! Document-scoped iframe lifecycle notifications, independent of loader events.
use crate::node::NIL;
use crate::{Dom, DomResult, NodeId};
use rustc_hash::{FxHashMap, FxHashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IframeAttribute {
    Src,
    Srcdoc,
    Name,
    Sandbox,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IframeChange {
    Connected,
    Disconnected,
    AttributeChanged(IframeAttribute),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IframeEvent {
    pub node: NodeId,
    pub change: IframeChange,
}

#[derive(Default)]
pub(crate) struct IframeQueue {
    documents: FxHashSet<NodeId>,
    events: FxHashMap<NodeId, Vec<IframeEvent>>,
}
impl Dom {
    /// Registers a browsing-context document and queues its existing iframes.
    /// Inert/parser-only documents are not watched unless explicitly registered.
    pub fn watch_iframes(&mut self, document: NodeId) -> DomResult<()> {
        let idx = self.check(document)?;
        if self.iframes.documents.insert(document) {
            self.iframes_after_insert(idx);
        }
        Ok(())
    }
    /// Releases queued nodes for a replaced/disposed browsing context.
    pub fn unwatch_iframes(&mut self, document: NodeId) {
        self.iframes.documents.remove(&document);
        for event in self.take_iframe_events(document) {
            self.release(event.node);
        }
    }
    /// Transfers pinned events to the caller. Call `release(event.node)` once
    /// per event after callbacks, including skipped events. Other documents'
    /// queues are untouched; loader resource draining does not consume these.
    /// Whether any document has queued iframe events.
    pub fn has_iframe_events(&self) -> bool {
        self.iframes.events.values().any(|v| !v.is_empty())
    }
    pub fn take_iframe_events(&mut self, document: NodeId) -> Vec<IframeEvent> {
        self.iframes.events.remove(&document).unwrap_or_default()
    }
    fn iframe_document(&self, idx: u32) -> Option<NodeId> {
        let doc = self.id_of(self.n(idx).owner_doc);
        (self.iframes.documents.contains(&doc) && self.is_connected_idx(idx)).then_some(doc)
    }
    fn iframe_element(&self, idx: u32) -> bool {
        self.n(idx).element().is_some_and(|e| e.is_html() && &*e.name.local == "iframe")
    }
    fn iframe_event(&mut self, document: NodeId, idx: u32, change: IframeChange) {
        let node = self.id_of(idx);
        self.add_ref(node).expect("live iframe");
        self.iframes.events.entry(document).or_default().push(IframeEvent { node, change });
    }
    fn iframe_subtree(&self, root: u32) -> Vec<u32> {
        let mut frames = Vec::new();
        let mut stack = vec![root];
        while let Some(idx) = stack.pop() {
            if self.iframe_element(idx) {
                frames.push(idx);
            }
            // Push siblings backwards without allocating a child list per node.
            if let Some(e) = self.n(idx).element() {
                if e.shadow_root != NIL {
                    stack.push(e.shadow_root);
                }
            }
            let mut child = self.n(idx).last_child;
            while child != NIL {
                stack.push(child);
                child = self.n(child).prev;
            }
        }
        frames
    }
    pub(crate) fn iframes_after_insert(&mut self, idx: u32) {
        let Some(document) = self.iframe_document(idx) else { return };
        for node in self.iframe_subtree(idx) {
            self.iframe_event(document, node, IframeChange::Connected);
        }
    }
    pub(crate) fn iframes_before_remove(&mut self, idx: u32) {
        let Some(document) = self.iframe_document(idx) else { return };
        for node in self.iframe_subtree(idx).into_iter().rev() {
            self.iframe_event(document, node, IframeChange::Disconnected);
        }
    }
    pub(crate) fn iframes_after_attr(&mut self, idx: u32, local: &str) {
        if !self.iframe_element(idx) {
            return;
        }
        let attribute = match local {
            "src" => IframeAttribute::Src,
            "srcdoc" => IframeAttribute::Srcdoc,
            "name" => IframeAttribute::Name,
            "sandbox" => IframeAttribute::Sandbox,
            _ => return,
        };
        if let Some(document) = self.iframe_document(idx) {
            self.iframe_event(document, idx, IframeChange::AttributeChanged(attribute));
        }
    }
}
