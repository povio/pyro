//! The tree and attribute changes that start an HTML media element's load:
//! `src` set or changed (the media element load algorithm), the element
//! inserted or created by the parser, and a `<source>` child inserted (the
//! resource selection algorithm, when the element's network state allows)
//! or removed (the algorithm's source pointer keeps its place).
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
    /// A direct track child was inserted or removed. The event's node is
    /// the media parent at the time of the mutation.
    TrackChildrenChanged(NodeId),
    /// A track's own source or metadata changed.
    TrackChanged,
    /// A `<source>` element was inserted as a child of the media element.
    SourceInserted(NodeId),
    /// A `<source>` child is about to be removed; `previous` is its
    /// previous sibling (any node) at that point. HTML keeps the resource
    /// selection pointer's position across removals, like a live range
    /// boundary: a pointer after `source` moves to after `previous`.
    SourceRemoved { source: NodeId, previous: Option<NodeId> },
    /// The element is about to be removed from its document (it pauses
    /// unless it is back in a document when the queue is drained).
    Removed,
}

impl MediaChange {
    /// The nodes besides the media element that the event holds a
    /// reference to (released with it).
    pub fn held(self) -> impl Iterator<Item = NodeId> {
        let (a, b) = match self {
            MediaChange::SourceInserted(source) | MediaChange::TrackChildrenChanged(source) => (Some(source), None),
            MediaChange::SourceRemoved { source, previous } => (Some(source), previous),
            _ => (None, None),
        };
        a.into_iter().chain(b)
    }
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

    fn is_track_idx(&self, idx: u32) -> bool {
        self.n(idx).element().is_some_and(|e| e.name.ns == ns!(html) && e.name.local == local_name!("track"))
    }

    fn queue_track_children(&mut self, media: u32) {
        let mut child = self.n(media).first_child;
        while child != NIL {
            if self.is_track_idx(child) {
                self.media_event(media, MediaChange::TrackChildrenChanged(self.id_of(child)));
            }
            child = self.n(child).next;
        }
    }

    fn media_event(&mut self, idx: u32, change: MediaChange) {
        let document = self.id_of(self.n(idx).owner_doc);
        if !self.is_watched_document(document) {
            return;
        }
        let node = self.id_of(idx);
        self.add_ref(node).expect("live media element");
        for held in change.held() {
            self.add_ref(held).expect("live node");
        }
        self.media.events.entry(document).or_default().push(MediaEvent { node, change });
        self.touch_hook();
    }

    /// After the attribute `local` of `idx` was set, changed or removed.
    pub(crate) fn media_after_attr(&mut self, idx: u32, local: &str) {
        if !self.media_seen {
            return;
        }
        if matches!(local, "src" | "default" | "kind" | "srclang" | "label") && self.is_track_idx(idx) {
            self.media_event(idx, MediaChange::TrackChanged);
            return;
        }
        if local == "crossorigin" && self.is_media_idx(idx) {
            self.queue_track_children(idx);
            return;
        }
        if local != "src" || !self.is_media_idx(idx) {
            return;
        }
        // Removing `src` does not invoke the load algorithm.
        let has_src = self.n(idx).element().is_some_and(|e| e.attr_local(&local_name!("src")).is_some());
        if has_src {
            self.media_event(idx, MediaChange::Src);
        }
    }

    /// After `node` was inserted into `parent`: media elements in its
    /// subtree when it is connected, and a `<source>` child of a media
    /// element.
    /// Inlined: until a media element exists, one flag test per insertion.
    #[inline(always)]
    pub(crate) fn media_after_insert(&mut self, node: u32) {
        if self.media_seen {
            self.media_inserted(node);
        }
    }

    #[inline(never)]
    fn media_inserted(&mut self, node: u32) {
        let document = self.id_of(self.n(node).owner_doc);
        if !self.is_watched_document(document) {
            return;
        }
        let parent = self.n(node).parent;
        if parent != NIL && self.is_track_idx(node) && self.is_media_idx(parent) {
            self.media_event(parent, MediaChange::TrackChildrenChanged(self.id_of(node)));
        }
        if parent != NIL && self.is_source_idx(node) && self.is_media_idx(parent) {
            let source = self.id_of(node);
            self.media_event(parent, MediaChange::SourceInserted(source));
        }
        // Only an insertion into a document starts resource selection.
        if !self.is_connected_idx(node) {
            return;
        }
        let mut i = node;
        while i != NIL {
            if self.is_media_idx(i) {
                self.media_event(i, MediaChange::Inserted);
                self.queue_track_children(i);
            }
            i = self.next_shadow_including(i, node);
        }
    }

    /// Before `node` is unlinked from a connected parent: the media
    /// elements of its subtree. Inlined: one flag test until a media
    /// element exists.
    #[inline(always)]
    pub(crate) fn media_before_remove(&mut self, node: u32) {
        if self.media_seen {
            self.media_removing(node);
        }
    }

    #[inline(never)]
    fn media_removing(&mut self, node: u32) {
        let document = self.id_of(self.n(node).owner_doc);
        if !self.is_watched_document(document) {
            return;
        }
        let parent = self.n(node).parent;
        if parent != NIL && self.is_track_idx(node) && self.is_media_idx(parent) {
            self.media_event(parent, MediaChange::TrackChildrenChanged(self.id_of(node)));
        }
        if parent != NIL && self.is_source_idx(node) && self.is_media_idx(parent) {
            let prev = self.n(node).prev;
            let change = MediaChange::SourceRemoved {
                source: self.id_of(node),
                previous: (prev != NIL).then(|| self.id_of(prev)),
            };
            self.media_event(parent, change);
        }
        if !self.is_connected_idx(node) {
            return;
        }
        let mut i = node;
        while i != NIL {
            if self.is_media_idx(i) {
                self.media_event(i, MediaChange::Removed);
            }
            i = self.next_shadow_including(i, node);
        }
    }

    /// Before `node` moves (`moveBefore`): a `<source>` leaving a media
    /// element. A move keeps media elements playing (no `Removed`).
    pub(crate) fn media_before_move(&mut self, node: u32) {
        if !self.media_seen || !self.is_source_idx(node) {
            return;
        }
        let parent = self.n(node).parent;
        if parent != NIL && self.is_media_idx(parent) {
            let prev = self.n(node).prev;
            let change = MediaChange::SourceRemoved {
                source: self.id_of(node),
                previous: (prev != NIL).then(|| self.id_of(prev)),
            };
            self.media_event(parent, change);
        }
    }

    /// After `node` moved: a `<source>` that is now a media element's child
    /// (HTML: the source element's moving steps run its insertion steps).
    pub(crate) fn media_after_move(&mut self, node: u32) {
        if !self.media_seen || !self.is_source_idx(node) {
            return;
        }
        let parent = self.n(node).parent;
        if parent != NIL && self.is_media_idx(parent) {
            self.media_event(parent, MediaChange::SourceInserted(self.id_of(node)));
        }
    }

    /// Whether any watched document has queued media events.
    pub fn has_media_events(&self) -> bool {
        !self.media.events.is_empty()
    }

    /// Transfers `document`'s queued events to the caller, who calls
    /// `release` once for each event's node and once for each node of
    /// `MediaChange::held`, after handling them.
    pub fn take_media_events(&mut self, document: NodeId) -> Vec<MediaEvent> {
        self.media.events.remove(&document).unwrap_or_default()
    }

    /// Takes the queued `SourceRemoved` events of the media element
    /// `media` (in order), for a resource selection that needs its pointer
    /// now; the caller releases them as `take_media_events` says.
    pub fn take_source_removals(&mut self, media: NodeId) -> Vec<MediaEvent> {
        if self.media.events.is_empty() {
            return Vec::new();
        }
        let Ok(document) = self.node_document(media) else { return Vec::new() };
        let Some(events) = self.media.events.get_mut(&document) else { return Vec::new() };
        let mut taken = Vec::new();
        events.retain(|e| {
            let removal = e.node == media && matches!(e.change, MediaChange::SourceRemoved { .. });
            if removal {
                taken.push(*e);
            }
            !removal
        });
        if events.is_empty() {
            self.media.events.remove(&document);
        }
        taken
    }

    /// Drops `document`'s queue (its browsing context went away).
    pub(crate) fn drop_media_events(&mut self, document: NodeId) {
        for event in self.take_media_events(document) {
            for held in event.change.held() {
                self.release(held);
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
            for held in e.change.held() {
                dom.release(held);
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
        // `div` is not in the document: no insertion steps for the element.
        assert_eq!(changes(&mut dom, doc), [(video, MediaChange::SourceInserted(source))]);
        let body = dom.create_element(doc, "body").unwrap();
        dom.append_child(doc, body).unwrap();
        dom.append_child(body, div).unwrap();
        assert_eq!(changes(&mut dom, doc), [(video, MediaChange::Inserted)]);
        dom.remove_node(div).unwrap();
        assert_eq!(changes(&mut dom, doc), [(video, MediaChange::Removed)]);
        // Removing a `<source>` child names its previous sibling.
        let first = dom.create_element(doc, "source").unwrap();
        dom.insert_before(video, first, Some(source)).unwrap();
        let _ = changes(&mut dom, doc);
        dom.remove_node(source).unwrap();
        dom.remove_node(first).unwrap();
        assert_eq!(
            changes(&mut dom, doc),
            [
                (video, MediaChange::SourceRemoved { source, previous: Some(first) }),
                (video, MediaChange::SourceRemoved { source: first, previous: None }),
            ]
        );
        dom.append_child(video, first).unwrap();
        dom.append_child(video, source).unwrap();
        let _ = changes(&mut dom, doc);
        dom.remove_node(source).unwrap();
        let taken = dom.take_source_removals(video);
        assert_eq!(taken.len(), 1);
        for e in &taken {
            for held in e.change.held() {
                dom.release(held);
            }
            dom.release(e.node);
        }
        assert!(!dom.has_media_events());
        // Documents without a browsing context queue nothing.
        let other = dom.create_document(DocumentKind::Html, "about:blank");
        let audio = dom.create_element(other, "audio").unwrap();
        dom.set_attribute(audio, "src", "a.mp3").unwrap();
        assert!(!dom.has_media_events());
    }
}
