//! The tree and attribute changes that start an HTML media element's load:
//! `src` set or changed (the media element load algorithm), the element
//! inserted or created by the parser, and a `<source>` child inserted (the
//! resource selection algorithm, when the element's network state allows).
//! Queued per watched (browsing-context) document, like the iframe events;
//! the bindings decide what each one does from the element's media state.
//!
//! Documents that never had an `<audio>` or `<video>` element pay one flag
//! test per attribute change and insertion (`Dom::media_seen`).
use markup5ever::{local_name, ns};
use rustc_hash::FxHashMap;

use crate::node::NIL;
use crate::{Dom, NodeId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaChange {
    /// The `src` content attribute was set or changed.
    Src,
    /// The element was inserted into a parent (by the parser too).
    Inserted,
    /// A `<source>` element was inserted as a child of the media element.
    SourceInserted(NodeId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaEvent {
    /// The `<audio>` or `<video>` element.
    pub node: NodeId,
    pub change: MediaChange,
}

#[derive(Default)]
pub(crate) struct MediaQueue {
    events: FxHashMap<NodeId, Vec<MediaEvent>>,
}

impl Dom {
    pub(crate) fn is_media_idx(&self, idx: u32) -> bool {
        self.n(idx).element().is_some_and(|e| {
            e.name.ns == ns!(html) && matches!(e.name.local, local_name!("audio") | local_name!("video"))
        })
    }

    fn is_source_idx(&self, idx: u32) -> bool {
        self.n(idx).element().is_some_and(|e| e.name.ns == ns!(html) && e.name.local == local_name!("source"))
    }

    fn media_event(&mut self, idx: u32, change: MediaChange) {
        let document = self.id_of(self.n(idx).owner_doc);
        if !self.is_watched_document(document) {
            return;
        }
        let node = self.id_of(idx);
        self.add_ref(node).expect("live media element");
        if let MediaChange::SourceInserted(source) = change {
            self.add_ref(source).expect("live source element");
        }
        self.media.events.entry(document).or_default().push(MediaEvent { node, change });
        self.touch_hook();
    }

    /// After the attribute `local` of `idx` was set, changed or removed.
    pub(crate) fn media_after_attr(&mut self, idx: u32, local: &str) {
        if !self.media_seen || local != "src" || !self.is_media_idx(idx) {
            return;
        }
        // Removing `src` does not invoke the load algorithm.
        let has_src = self.n(idx).element().is_some_and(|e| e.attr_local(&local_name!("src")).is_some());
        if has_src {
            self.media_event(idx, MediaChange::Src);
        }
    }

    /// After `node` was inserted into `parent`: media elements in its
    /// subtree, and a `<source>` child of a media element.
    pub(crate) fn media_after_insert(&mut self, node: u32) {
        if !self.media_seen {
            return;
        }
        let parent = self.n(node).parent;
        if parent != NIL && self.is_source_idx(node) && self.is_media_idx(parent) {
            let source = self.id_of(node);
            self.media_event(parent, MediaChange::SourceInserted(source));
        }
        let mut i = node;
        while i != NIL {
            if self.is_media_idx(i) {
                self.media_event(i, MediaChange::Inserted);
            }
            i = self.next_shadow_including(i, node);
        }
    }

    /// Whether any watched document has queued media events.
    pub fn has_media_events(&self) -> bool {
        !self.media.events.is_empty()
    }

    /// Transfers `document`'s queued events to the caller, who calls
    /// `release` once for each event's node and once for each
    /// `SourceInserted` source, after handling them.
    pub fn take_media_events(&mut self, document: NodeId) -> Vec<MediaEvent> {
        self.media.events.remove(&document).unwrap_or_default()
    }

    /// Drops `document`'s queue (its browsing context went away).
    pub(crate) fn drop_media_events(&mut self, document: NodeId) {
        for event in self.take_media_events(document) {
            if let MediaChange::SourceInserted(source) = event.change {
                self.release(source);
            }
            self.release(event.node);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DocumentKind;

    fn changes(dom: &mut Dom, doc: NodeId) -> Vec<(NodeId, MediaChange)> {
        let events = dom.take_media_events(doc);
        for e in &events {
            if let MediaChange::SourceInserted(source) = e.change {
                dom.release(source);
            }
            dom.release(e.node);
        }
        events.into_iter().map(|e| (e.node, e.change)).collect()
    }

    #[test]
    fn src_insertion_and_source_children_are_queued_for_watched_documents() {
        let mut dom = Dom::new();
        let doc = dom.create_document(DocumentKind::Html, "http://site.test/");
        dom.watch_iframes(doc).unwrap();
        let div = dom.create_element(doc, "div").unwrap();
        dom.set_attribute(div, "src", "x").unwrap();
        assert!(!dom.has_media_events(), "no media element yet");
        let video = dom.create_element(doc, "video").unwrap();
        dom.set_attribute(video, "src", "a.mp4").unwrap();
        dom.set_attribute(video, "src", "a.mp4").unwrap();
        dom.remove_attribute(video, "src").unwrap();
        assert_eq!(changes(&mut dom, doc), [(video, MediaChange::Src), (video, MediaChange::Src)]);
        let source = dom.create_element(doc, "source").unwrap();
        dom.append_child(video, source).unwrap();
        dom.append_child(div, video).unwrap();
        assert_eq!(
            changes(&mut dom, doc),
            [(video, MediaChange::SourceInserted(source)), (video, MediaChange::Inserted)]
        );
        // Documents without a browsing context queue nothing.
        let other = dom.create_document(DocumentKind::Html, "about:blank");
        let audio = dom.create_element(other, "audio").unwrap();
        dom.set_attribute(audio, "src", "a.mp3").unwrap();
        assert!(!dom.has_media_events());
    }
}
