//! Script element classification ("prepare the script element", HTML
//! §4.12.1.1) and the resource-event queue the loader drains to fetch
//! scripts, stylesheets, iframes and images that become connected — whether
//! inserted by the parser or by script (Vite injects `<link>`/`<script>`).

use markup5ever::{local_name, ns};

use crate::dom::Dom;
use crate::error::{DomError, DomResult};
use crate::node::{element_flags, NodeId, NodeKind, NIL};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScriptKind {
    Classic,
    Module,
    ImportMap,
    /// Unknown type: a data block, never executed.
    Data,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptInfo {
    pub kind: ScriptKind,
    /// Raw `src` attribute (resolve against the document base URL).
    pub src: Option<String>,
    pub async_: bool,
    pub defer: bool,
    pub nomodule: bool,
    pub parser_inserted: bool,
    pub already_started: bool,
    /// Child text content (inline script source).
    pub inline_text: String,
    pub crossorigin: Option<String>,
    pub integrity: Option<String>,
}

const JS_MIME: &[&str] = &[
    "application/ecmascript",
    "application/javascript",
    "application/x-ecmascript",
    "application/x-javascript",
    "text/ecmascript",
    "text/javascript",
    "text/javascript1.0",
    "text/javascript1.1",
    "text/javascript1.2",
    "text/javascript1.3",
    "text/javascript1.4",
    "text/javascript1.5",
    "text/jscript",
    "text/livescript",
    "text/x-ecmascript",
    "text/x-javascript",
];

/// Whether a lowercased, trimmed `type` is a JavaScript MIME type essence
/// (HTML "JavaScript MIME type essence match").
pub fn is_javascript_mime(essence: &str) -> bool {
    JS_MIME.contains(&essence)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    Script,
    /// `<link rel=stylesheet>` (also covers `rel` containing "stylesheet").
    Stylesheet,
    /// `<style>` element whose text changed or that was (dis)connected.
    StyleText,
    Iframe,
    Img,
    /// `<link rel=modulepreload>` / `<link rel=preload>`: fetched ahead, with
    /// `load`/`error` on the link (bundlers' preload helpers await them).
    Preload,
    /// `<meta http-equiv=refresh>`: its insertion schedules a declarative
    /// refresh (the loader handles `Connected` events only).
    Meta,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ResourceReason {
    Connected,
    AttrChanged,
    Removed,
    ChildrenChanged,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ResourceEvent {
    pub node: NodeId,
    pub kind: ResourceKind,
    pub reason: ResourceReason,
}

#[derive(Default)]
pub(crate) struct ResourceQueue {
    pub(crate) enabled: bool,
    pub(crate) events: Vec<ResourceEvent>,
}

impl Dom {
    /// Enables the resource-event queue (the loader turns this on per page).
    pub fn set_resource_tracking(&mut self, on: bool) {
        self.resources.enabled = on;
        if !on {
            self.take_resource_events();
        }
    }

    /// Connected, for the resource hooks, or owned by the fragment parser's
    /// host document. That document is not connected (so its tree needs no
    /// tree flags or id map), but its nodes report their resources as a
    /// connected document's would: the loader resolves each event's document
    /// when it handles it, after the parsed nodes moved to their real one.
    fn connected_for_resources(&self, i: u32) -> bool {
        self.is_connected_idx(i) || self.is_fragment_parse_host(self.n(i).owner_doc)
    }

    /// Whether `take_resource_events` would return anything.
    pub fn has_resource_events(&self) -> bool {
        !self.resources.events.is_empty()
    }

    /// Drains pending resource events. Node ids stay valid until the next
    /// `collect_garbage`.
    pub fn take_resource_events(&mut self) -> Vec<ResourceEvent> {
        let events = std::mem::take(&mut self.resources.events);
        for ev in &events {
            self.release(ev.node);
        }
        events
    }

    pub(crate) fn resource_kind(&self, idx: u32) -> Option<ResourceKind> {
        let e = self.n(idx).element()?;
        if e.name.ns == ns!(svg) && e.name.local == local_name!("script") {
            return Some(ResourceKind::Script);
        }
        if !e.is_html() {
            return None;
        }
        // Atoms compare as integers: no string comparison per name.
        Some(match e.name.local {
            local_name!("script") => ResourceKind::Script,
            local_name!("style") => ResourceKind::StyleText,
            local_name!("link")
                if e.attr_local(&local_name!("rel"))
                    .is_some_and(|r| r.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case("stylesheet"))) =>
            {
                ResourceKind::Stylesheet
            }
            local_name!("link")
                if e.attr_local(&local_name!("rel")).is_some_and(|r| {
                    r.split_ascii_whitespace()
                        .any(|t| t.eq_ignore_ascii_case("modulepreload") || t.eq_ignore_ascii_case("preload"))
                }) =>
            {
                ResourceKind::Preload
            }
            local_name!("iframe") | local_name!("frame") => ResourceKind::Iframe,
            local_name!("img") => ResourceKind::Img,
            // HTML: a refresh pragma acts when inserted into the document
            // tree (connected, not in a shadow tree), decided here because
            // the loader may see the event after the meta was removed.
            local_name!("meta")
                if self.is_connected_idx(idx)
                    && !self.in_shadow_tree(idx)
                    && e.attr_local(&local_name!("http-equiv")).is_some_and(|v| v.eq_ignore_ascii_case("refresh")) =>
            {
                ResourceKind::Meta
            }
            // An image button fetches its `src` like `<img>` (its natural
            // size is its layout size); `type` is a relevant attribute, so
            // becoming one starts the load.
            local_name!("input")
                if e.attr_local(&local_name!("type")).is_some_and(|t| t.eq_ignore_ascii_case("image")) =>
            {
                ResourceKind::Img
            }
            _ => return None,
        })
    }

    /// Called after `node` was inserted (tree mutation or parser).
    pub(crate) fn resources_after_insert(&mut self, node: u32) {
        // The usual insertion of a script: one element holding at most text
        // (`createElement`, `textContent`, `appendChild`), or the text node
        // itself. Its subtree has one node that can be anything to the four
        // passes below, so that node's kind is looked at once, not once per
        // pass and walk.
        if self.subtree_has_one_element_at_most(node) {
            self.details_after_insert(node);
            if self.is_connected_idx(node) {
                self.iframe_after_parser_insert(node);
            }
            self.media_after_insert(node);
            return self.resources_after_single_insert(node);
        }
        self.details_after_insert(node);
        self.iframes_after_insert(node);
        self.media_after_insert(node);
        if !self.resources.enabled || !self.connected_for_resources(node) {
            return;
        }
        let parent = self.n(node).parent;
        if parent != NIL && self.resource_kind(parent) == Some(ResourceKind::StyleText) {
            self.push_resource(parent, ResourceKind::StyleText, ResourceReason::ChildrenChanged);
        }
        self.scan_subtree(node, ResourceReason::Connected);
    }

    /// `resources_after_insert` for a node the HTML parser inserted: nearly
    /// always one it just made, with no children or shadow root yet, whose
    /// subtree is itself. That case is checked without the subtree walks and
    /// their two stacks per parsed node.
    pub(crate) fn resources_after_parser_insert(&mut self, node: u32) {
        let n = self.n(node);
        if n.first_child != NIL || n.element().is_some_and(|e| e.shadow_root != NIL) {
            return self.resources_after_insert(node);
        }
        self.details_after_insert(node);
        self.iframe_after_parser_insert(node);
        self.media_after_insert(node);
        self.resources_after_single_insert(node);
    }

    /// Whether `node`'s shadow-including subtree holds no element but `node`
    /// itself (it has no shadow root, and its children are text, comments..).
    fn subtree_has_one_element_at_most(&self, node: u32) -> bool {
        let n = self.n(node);
        if n.element().is_some_and(|e| e.shadow_root != NIL) {
            return false;
        }
        let mut c = n.first_child;
        while c != NIL {
            let child = self.n(c);
            if child.first_child != NIL || child.element().is_some() {
                return false;
            }
            c = child.next;
        }
        true
    }

    /// The resource events of an inserted `node` whose subtree holds no
    /// other element: `scan_subtree` of the node alone.
    fn resources_after_single_insert(&mut self, node: u32) {
        if !self.resources.enabled || !self.connected_for_resources(node) {
            return;
        }
        let parent = self.n(node).parent;
        if parent != NIL && self.resource_kind(parent) == Some(ResourceKind::StyleText) {
            self.push_resource(parent, ResourceKind::StyleText, ResourceReason::ChildrenChanged);
        }
        if let Some(k) = self.resource_kind(node) {
            let parser_script = k == ResourceKind::Script
                && self.n(node).element().is_some_and(|e| e.flags & element_flags::PARSER_INSERTED != 0);
            if !parser_script {
                self.push_resource(node, k, ResourceReason::Connected);
            }
        }
        if self.is_html_meta(node) {
            self.process_viewport_meta(node);
        }
    }

    /// HTML's script "children changed steps", for an insertion into
    /// `parent` (removals do not prepare a script): called once per insert,
    /// before the post-connection steps of the inserted nodes.
    pub(crate) fn script_children_inserted(&mut self, parent: u32) {
        if self.script_steps_due(parent) && self.resources.enabled {
            self.push_resource(parent, ResourceKind::Script, ResourceReason::ChildrenChanged);
        }
    }

    /// Called before `node` is unlinked from a connected parent.
    pub(crate) fn resources_before_remove(&mut self, node: u32) {
        self.iframes_before_remove(node);
        self.media_before_remove(node);
        if !self.resources.enabled || !self.connected_for_resources(node) {
            return;
        }
        let parent = self.n(node).parent;
        if parent != NIL && self.resource_kind(parent) == Some(ResourceKind::StyleText) {
            self.push_resource(parent, ResourceKind::StyleText, ResourceReason::ChildrenChanged);
        }
        // Only stylesheet owners need removal notifications.
        let mut i = node;
        while i != NIL {
            if let Some(k @ (ResourceKind::Stylesheet | ResourceKind::StyleText)) = self.resource_kind(i) {
                self.push_resource(i, k, ResourceReason::Removed);
            }
            i = self.next_shadow_including_rev(i, node);
        }
    }

    /// Called after the attribute `local` of `idx` was set or removed;
    /// `added` when the element did not have it before.
    pub(crate) fn resources_after_attr(&mut self, idx: u32, local: &str, added: bool) {
        self.iframes_after_attr(idx, local);
        self.media_after_attr(idx, local);
        // HTML: a connected script is prepared when it gets a `src` it did
        // not have. No other attribute change (`type`, a changed or removed
        // `src`) prepares it again.
        let script_src = local == "src" && added && self.script_steps_due(idx);
        if !self.resources.enabled {
            return;
        }
        if matches!(local, "name" | "content") && self.is_html_meta(idx) {
            self.process_viewport_meta(idx);
        }
        let relevant = matches!(local, "src" | "href" | "rel" | "media" | "srcset" | "type" | "disabled");
        if !relevant {
            return;
        }
        if !self.connected_for_resources(idx) {
            // HTML "update the image data" runs for an `<img>` whether or not
            // it is connected (`new Image()`, a tree built before insertion).
            // The loader drops events of a document without a browsing
            // context (template contents, createHTMLDocument).
            if matches!(local, "src" | "srcset")
                && self.n(idx).element().is_some_and(|e| e.is_html() && e.name.local == local_name!("img"))
            {
                self.push_resource(idx, ResourceKind::Img, ResourceReason::AttrChanged);
            }
            return;
        }
        if let Some(k) = self.resource_kind(idx) {
            if k == ResourceKind::Script && !script_src {
                return;
            }
            self.push_resource(idx, k, ResourceReason::AttrChanged);
        } else if local == "rel" {
            // rel removed "stylesheet": the old sheet must go.
            if self.n(idx).element().is_some_and(|e| e.is_html() && e.name.local == local_name!("link")) {
                self.push_resource(idx, ResourceKind::Stylesheet, ResourceReason::Removed);
            }
        }
    }

    /// Asks the loader to update `img`'s image data (its `Img` event): what
    /// `decode()` waits on when no request of the selected source is known,
    /// e.g. a `<source>` of a detached `<picture>` changed.
    pub fn queue_image_update(&mut self, img: NodeId) {
        if !self.resources.enabled {
            return;
        }
        if let Ok(i) = self.check(img) {
            self.push_resource(i, ResourceKind::Img, ResourceReason::AttrChanged);
        }
    }

    pub(crate) fn resources_after_text(&mut self, text_node: u32) {
        if !self.resources.enabled {
            return;
        }
        let p = self.n(text_node).parent;
        if p != NIL && self.resource_kind(p) == Some(ResourceKind::StyleText) && self.connected_for_resources(p) {
            self.push_resource(p, ResourceKind::StyleText, ResourceReason::ChildrenChanged);
        }
    }

    fn scan_subtree(&mut self, node: u32, reason: ResourceReason) {
        // Shadow trees first, then children from the last: the order this
        // walk had with a stack, kept without one.
        let mut i = node;
        let mut metas = false;
        while i != NIL {
            if let Some(k) = self.resource_kind(i) {
                // Parser-inserted scripts are driven by ParseStep::Script.
                let parser_script = k == ResourceKind::Script
                    && self.n(i).element().is_some_and(|e| e.flags & element_flags::PARSER_INSERTED != 0);
                if !parser_script {
                    self.push_resource(i, k, reason);
                }
            }
            metas |= self.is_html_meta(i);
            i = self.next_shadow_including_rev(i, node);
        }
        if metas && reason == ResourceReason::Connected {
            self.process_viewport_metas(node);
        }
    }

    pub(crate) fn push_resource(&mut self, idx: u32, kind: ResourceKind, reason: ResourceReason) {
        let ev = ResourceEvent { node: self.id_of(idx), kind, reason };
        // Coalesce consecutive duplicates (e.g. many text appends to one <style>).
        if self.resources.events.last() != Some(&ev) {
            self.add_ref(ev.node).expect("live resource node");
            self.resources.events.push(ev);
        }
    }

    /// Parser hook: an element was closed (end tag / EOF). `<style>` content
    /// is complete at this point.
    pub(crate) fn resources_element_popped(&mut self, idx: u32) {
        if let Some(e) = self.n_mut(idx).element_mut() {
            e.flags &= !element_flags::STYLE_PARSING;
        }
        if self.resources.enabled
            && self.resource_kind(idx) == Some(ResourceKind::StyleText)
            && self.connected_for_resources(idx)
        {
            self.push_resource(idx, ResourceKind::StyleText, ResourceReason::ChildrenChanged);
        }
    }

    /// Classifies a `<script>` element for the script loader.
    pub fn script_info(&self, script: NodeId) -> DomResult<ScriptInfo> {
        let i = self.check(script)?;
        let e = self.n(i).element().ok_or(DomError::Type("not an element"))?;
        let ty = e.attr_local(&local_name!("type"));
        let lang = e.attr_local(&local_name!("language"));
        let kind = match ty {
            Some(t) if !t.is_empty() => {
                let t = t.trim().to_ascii_lowercase();
                if JS_MIME.contains(&t.as_str()) {
                    ScriptKind::Classic
                } else if t == "module" {
                    ScriptKind::Module
                } else if t == "importmap" {
                    ScriptKind::ImportMap
                } else {
                    ScriptKind::Data
                }
            }
            Some(_) => ScriptKind::Classic,
            None => match lang {
                Some(l) if !l.is_empty() => {
                    if JS_MIME.contains(&format!("text/{}", l.to_ascii_lowercase()).as_str()) {
                        ScriptKind::Classic
                    } else {
                        ScriptKind::Data
                    }
                }
                _ => ScriptKind::Classic,
            },
        };
        let mut inline_text = String::new();
        for c in self.child_idx_iter(i) {
            // "Child text content": Text and (XML) CDATA section children.
            if let NodeKind::Text(s) | NodeKind::CData(s) = &self.n(c).kind {
                inline_text.push_str(s);
            }
        }
        Ok(ScriptInfo {
            kind,
            src: e.attr_local(&local_name!("src")).map(str::to_owned),
            async_: e.attr_local(&local_name!("async")).is_some(),
            defer: e.attr_local(&local_name!("defer")).is_some(),
            nomodule: e.attr_local(&local_name!("nomodule")).is_some(),
            parser_inserted: e.flags & element_flags::PARSER_INSERTED != 0,
            already_started: e.flags & element_flags::SCRIPT_ALREADY_STARTED != 0,
            inline_text,
            crossorigin: e.attr_local(&local_name!("crossorigin")).map(str::to_owned),
            integrity: e.attr_local(&local_name!("integrity")).map(str::to_owned),
        })
    }

    /// Sets the script's "already started" flag (prepare step 1).
    pub fn mark_script_started(&mut self, script: NodeId) -> DomResult<()> {
        let i = self.check(script)?;
        let e = self.n_mut(i).element_mut().ok_or(DomError::Type("not an element"))?;
        e.flags |= element_flags::SCRIPT_ALREADY_STARTED;
        Ok(())
    }

    /// Marks a custom element as defined (`:defined`).
    pub fn set_custom_defined(&mut self, el: NodeId) -> DomResult<()> {
        let i = self.check(el)?;
        let e = self.n_mut(i).element_mut().ok_or(DomError::Type("not an element"))?;
        e.flags |= element_flags::CUSTOM_DEFINED;
        self.reactions.any_custom = true;
        // :defined is element state.
        self.bump();
        self.note(crate::dom::Change::State(i));
        Ok(())
    }

    /// Clears the parser-inserted flag (scripts moved/cloned by script; also
    /// "force async" semantics for dynamically inserted scripts).
    pub fn clear_parser_inserted(&mut self, script: NodeId) -> DomResult<()> {
        let i = self.check(script)?;
        let e = self.n_mut(i).element_mut().ok_or(DomError::Type("not an element"))?;
        e.flags &= !element_flags::PARSER_INSERTED;
        Ok(())
    }
}
