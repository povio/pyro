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
    /// A permission element (`<geolocation>`, `<install>`, `<usermedia>`, `<camera>`,
    /// `<microphone>`: WICG PEPC, tentative) became connected: it registers and checks its
    /// validity (`validationstatuschange`). A post-connection step like the others.
    PermissionElement,
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
    /// `<object>`/`<embed>` elements whose fetched resource turned out to be a document (its
    /// Content-Type; [`Dom::set_object_document`]), until their `data`/`src` or `type` changes.
    document_objects: FxHashSet<NodeId>,
    /// `<object>`/`<embed>` elements with a child navigable (their `Connected` queued, not yet
    /// their `Disconnected`): what an attribute change compares against.
    object_frames: FxHashSet<NodeId>,
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
        // `<frame>` creates a child navigable like `<iframe>` does, and so does an `<object>` or
        // `<embed>` whose resource is a document.
        self.n(idx).element().is_some_and(|e| {
            e.is_html()
                && (matches!(e.name.local, local_name!("iframe") | local_name!("frame"))
                    || (is_object_or_embed(e)
                        && self.iframes.document_objects.contains(&self.id_of(idx))
                        && !self.nested_in_content(idx)))
        })
    }
    /// Whether `idx` has an ancestor media element, or an ancestor `<object>` that shows a document
    /// rather than its fallback content: an `<object>` or `<embed>` there represents nothing (HTML
    /// "the embed element", "the object element").
    fn nested_in_content(&self, idx: u32) -> bool {
        let mut i = self.n(idx).parent;
        while i != NIL {
            if let Some(e) = self.n(i).element().filter(|e| e.is_html()) {
                if matches!(e.name.local, local_name!("video") | local_name!("audio")) {
                    return true;
                }
                if e.name.local == local_name!("object") && self.iframe_element(i) {
                    return true;
                }
            }
            i = self.n(i).parent;
        }
        false
    }
    /// The `<object>`/`<embed>` elements under the object `idx`, whose showing a document or not
    /// changed: each one's child navigable comes or goes with it.
    fn refresh_nested_objects(&mut self, idx: u32) {
        let mut nested = Vec::new();
        let mut i = self.next_shadow_including(idx, idx);
        while i != NIL {
            if self.n(i).element().is_some_and(is_object_or_embed) {
                nested.push(i);
            }
            i = self.next_shadow_including(i, idx);
        }
        let Some(document) = self.iframe_document(idx) else { return };
        for i in nested {
            let node = self.id_of(i);
            match (self.iframes.object_frames.contains(&node), self.iframe_element(i)) {
                (false, true) => self.iframe_event(document, i, IframeChange::Connected),
                (true, false) => self.iframe_event(document, i, IframeChange::Disconnected),
                _ => {}
            }
        }
    }
    fn iframe_event(&mut self, document: NodeId, idx: u32, change: IframeChange) {
        // A `<use>` instance's frames load nothing and its scripts never run.
        if self.in_ua_shadow_tree(idx) {
            return;
        }
        let node = self.id_of(idx);
        if self.n(idx).element().is_some_and(is_object_or_embed) {
            match change {
                IframeChange::Connected => {
                    self.iframes.object_frames.insert(node);
                }
                IframeChange::Disconnected => {
                    self.iframes.object_frames.remove(&node);
                }
                _ => {}
            }
        }
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
    /// Whether `idx` is an HTML permission element ([`IframeChange::PermissionElement`]).
    fn permission_element(&self, idx: u32) -> bool {
        self.n(idx).element().is_some_and(|e| {
            e.is_html() && matches!(&*e.name.local, "geolocation" | "install" | "usermedia" | "camera" | "microphone")
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
            } else if self.permission_element(idx) {
                found.push((idx, IframeChange::PermissionElement));
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
        } else if self.permission_element(idx) {
            IframeChange::PermissionElement
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
        if let Some(url_attribute) = self.n(idx).element().filter(|e| is_object_or_embed(e)).map(resource_attribute) {
            // HTML "the object element" / "the embed element": a change of the resource's URL or
            // type runs the steps again, so a child navigable may come or go.
            if local == url_attribute || local == "type" {
                self.object_steps_again(idx, local == url_attribute);
            }
            return;
        }
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

/// Whether an HTML `<object>` (by its `data`) or `<embed>` (by its `src`) names a document when its
/// response has no Content-Type: its `type` is an HTML or XHTML document type, or without a `type`
/// its URL's file extension names one (HTML "the object element": the Content-Type decides first).
pub fn object_hosts_document(e: &crate::node::ElementData) -> bool {
    let url = e.attr(resource_attribute(e));
    // Without `data` an object shows its fallback content (an embed without `src`, nothing).
    if url.is_none_or(|url| url.trim().is_empty()) {
        return false;
    }
    match e.attr("type").map(str::trim) {
        Some(mime) if !mime.is_empty() => {
            let essence = mime.split(';').next().unwrap_or_default().trim();
            essence.eq_ignore_ascii_case("text/html") || essence.eq_ignore_ascii_case("application/xhtml+xml")
        }
        _ => url.is_some_and(|data| {
            let path = data.split(['?', '#']).next().unwrap_or_default();
            let extension = path.rsplit_once('.').map_or("", |(_, ext)| ext);
            ["html", "htm", "xhtml", "xht"].iter().any(|known| extension.eq_ignore_ascii_case(known))
        }),
    }
}

fn is_object_or_embed(e: &crate::node::ElementData) -> bool {
    e.is_html() && matches!(e.name.local, local_name!("object") | local_name!("embed"))
}

/// The attribute naming an `<object>`'s (`data`) or `<embed>`'s (`src`) resource.
fn resource_attribute(e: &crate::node::ElementData) -> &'static str {
    if e.name.local == local_name!("embed") {
        "src"
    } else {
        "data"
    }
}

impl Dom {
    /// Whether `node` is an HTML `<object>` or `<embed>` showing a nested document: its fetched
    /// resource was one ([`Dom::set_object_document`]).
    pub fn is_document_object(&self, node: NodeId) -> bool {
        self.check(node).is_ok_and(|idx| self.n(idx).element().is_some_and(is_object_or_embed) && self.iframe_element(idx))
    }

    /// The fetched resource of the `<object>`/`<embed>` `node` is a document (its response's
    /// Content-Type): it gets a child navigable, as an iframe does, which loads it.
    pub fn set_object_document(&mut self, node: NodeId) {
        let Ok(idx) = self.check(node) else { return };
        if !self.n(idx).element().is_some_and(is_object_or_embed) {
            return;
        }
        self.iframes.document_objects.insert(node);
        if !self.iframes.object_frames.contains(&node) && self.iframe_element(idx) {
            if let Some(document) = self.iframe_document(idx) {
                self.iframe_event(document, idx, IframeChange::Connected);
                self.refresh_nested_objects(idx);
            }
        }
    }

    /// The `<object>`/`<embed>` `idx` changed its resource URL (`url_changed`) or `type`: the
    /// Content-Type of the old resource no longer decides, and a child navigable comes (a
    /// document now), goes (no longer), or navigates to the new URL.
    fn object_steps_again(&mut self, idx: u32, url_changed: bool) {
        let node = self.id_of(idx);
        self.iframes.document_objects.remove(&node);
        let was = self.iframes.object_frames.contains(&node);
        let now = self.iframe_element(idx);
        let Some(document) = self.iframe_document(idx) else { return };
        match (was, now) {
            (false, true) => self.iframe_event(document, idx, IframeChange::Connected),
            (true, false) => self.iframe_event(document, idx, IframeChange::Disconnected),
            (true, true) if url_changed => {
                self.iframe_event(document, idx, IframeChange::AttributeChanged(IframeAttribute::Src))
            }
            _ => {}
        }
        if was != now {
            self.refresh_nested_objects(idx);
        }
    }
}
