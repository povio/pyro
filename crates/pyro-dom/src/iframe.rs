//! Document-scoped iframe lifecycle notifications, independent of loader events.
use crate::node::NIL;
use crate::{Dom, DomResult, NodeId};
use markup5ever::local_name;
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
    /// HTML "prepare the script element" is due for a `<script>` the parser
    /// did not insert: it became connected, had a child inserted, or got a
    /// `src` attribute it lacked. Queued with the iframe events because both
    /// are post-connection steps and run in one (tree) order.
    Script,
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
        self.drop_media_events(document);
        for event in self.take_iframe_events(document) {
            self.release(event.node);
        }
    }
    /// Whether `document` is a watched browsing-context document.
    pub(crate) fn is_watched_document(&self, document: NodeId) -> bool {
        self.iframes.documents.contains(&document)
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
        // `<frame>` creates a child navigable like `<iframe>` does.
        self.n(idx)
            .element()
            .is_some_and(|e| matches!(e.name.local, local_name!("iframe") | local_name!("frame")) && e.is_html())
    }
    fn iframe_event(&mut self, document: NodeId, idx: u32, change: IframeChange) {
        // A `<use>` instance's frames load nothing and its scripts never run.
        if self.in_ua_shadow_tree(idx) {
            return;
        }
        let node = self.id_of(idx);
        self.add_ref(node).expect("live iframe");
        self.iframes.events.entry(document).or_default().push(IframeEvent { node, change });
        self.touch_hook();
    }
    fn iframe_subtree(&self, root: u32) -> Vec<u32> {
        let mut frames = Vec::new();
        // Children before shadow trees, in tree order.
        let mut idx = root;
        while idx != NIL {
            if self.iframe_element(idx) {
                frames.push(idx);
            }
            idx = self.next_shadow_including_light_first(idx, root);
        }
        frames
    }
    /// Whether `idx` is a script element "prepare the script element" can
    /// still run: not started, and not one the parser drives itself.
    fn preparable_script(&self, idx: u32) -> bool {
        use crate::node::element_flags::{PARSER_INSERTED, SCRIPT_ALREADY_STARTED};
        self.n(idx).element().is_some_and(|e| {
            e.name.local == local_name!("script")
                && (e.is_html() || e.name.ns == markup5ever::ns!(svg))
                && e.flags & (PARSER_INSERTED | SCRIPT_ALREADY_STARTED) == 0
        })
    }
    /// The iframes and preparable scripts of `root`'s subtree in
    /// shadow-including tree order: the order of their post-connection steps.
    fn post_connection_subtree(&self, root: u32) -> Vec<(u32, IframeChange)> {
        let mut found = Vec::new();
        let mut idx = root;
        while idx != NIL {
            if self.iframe_element(idx) {
                found.push((idx, IframeChange::Connected));
            } else if self.preparable_script(idx) {
                found.push((idx, IframeChange::Script));
            }
            idx = self.next_shadow_including(idx, root);
        }
        found
    }
    pub(crate) fn iframes_after_insert(&mut self, idx: u32) {
        // Nearly every inserted subtree holds neither an iframe nor a
        // script: its nodes are looked at before the document is looked up.
        if !self.is_connected_idx(idx) {
            return;
        }
        let found = self.post_connection_subtree(idx);
        if found.is_empty() {
            return;
        }
        let Some(document) = self.iframe_document(idx) else { return };
        for (node, change) in found {
            self.iframe_event(document, node, change);
        }
    }
    /// `iframes_after_insert` for a node without children or shadow root
    /// (`resources_after_parser_insert`): the node's own kind decides first.
    pub(crate) fn iframe_after_parser_insert(&mut self, idx: u32) {
        let change = if self.iframe_element(idx) {
            IframeChange::Connected
        } else if self.preparable_script(idx) {
            IframeChange::Script
        } else {
            return;
        };
        if let Some(document) = self.iframe_document(idx) {
            self.iframe_event(document, idx, change);
        }
    }
    /// Queues "prepare the script element" for the connected script `idx`
    /// (HTML's script "children changed steps" and `src` attribute steps).
    /// Returns whether it was queued.
    pub(crate) fn script_steps_due(&mut self, idx: u32) -> bool {
        if !self.preparable_script(idx) {
            return false;
        }
        match self.iframe_document(idx) {
            Some(document) => {
                self.iframe_event(document, idx, IframeChange::Script);
                true
            }
            None => false,
        }
    }
    pub(crate) fn iframes_before_remove(&mut self, idx: u32) {
        if !self.is_connected_idx(idx) {
            return;
        }
        let frames = self.iframe_subtree(idx);
        if frames.is_empty() {
            return;
        }
        let Some(document) = self.iframe_document(idx) else { return };
        for node in frames.into_iter().rev() {
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
