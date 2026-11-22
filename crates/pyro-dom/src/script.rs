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
    /// A classic script that is never run once prepared ("already started", no events): it
    /// has `nomodule`, or `event` and `for` attributes other than window's `onload`.
    pub nomodule: bool,
    pub parser_inserted: bool,
    pub already_started: bool,
    /// HTML "force async": a script-inserted external script without an
    /// `async` attribute runs as soon as possible unless this is false (it is
    /// then in its document's in-order list).
    pub force_async: bool,
    /// Child text content (inline script source).
    pub inline_text: String,
    pub crossorigin: Option<String>,
    pub integrity: Option<String>,
    /// The `referrerpolicy` content attribute.
    pub referrerpolicy: Option<String>,
    /// The `charset` content attribute of an HTML script (a classic script's
    /// fallback encoding when its response names none).
    pub charset: Option<String>,
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
    /// `<object data>`: fetched for its `load`/`error` events (the renderer draws the content).
    Object,
    /// `<embed src>`: fetched for its `load` event and, by its response's Content-Type, whether it
    /// shows a document (a child navigable).
    Embed,
    /// `<link rel=modulepreload>` / `<link rel=preload>`: fetched ahead, with
    /// `load`/`error` on the link (bundlers' preload helpers await them).
    Preload,
    /// `<meta http-equiv=refresh>`: its insertion schedules a declarative
    /// refresh (the loader handles `Connected` events only).
    Meta,
    /// SVG `<use>`: (re)builds its shadow tree of clones of the element its
    /// `href` names (the bindings' `svg_use`).
    SvgUse,
    /// SVG `<image>`: fetched for its `load`/`error` events (the renderer
    /// draws the image itself).
    SvgImage,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ResourceReason {
    Connected,
    AttrChanged,
    /// An `<img>`'s `src` or `srcset` was set (even to its value): HTML "update the image data"
    /// restarts the request (a relevant mutation).
    SourceChanged,
    Removed,
    /// An HTML `<link>`'s `rel` lost its `stylesheet` token: the link no longer creates an
    /// external resource link and its sheet goes. If the token is back by the time the loader
    /// looks (several writes in one task), the link is a new stylesheet link: it is fetched and
    /// fires `load` again, where an unchanged link keeps its sheet.
    Unlinked,
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
    /// Each `<style>`'s count of children changes (HTML: each one replaces its CSS style sheet
    /// object, also when its text stays the same).
    pub(crate) sheet_generations: rustc_hash::FxHashMap<NodeId, u32>,
    /// Script-inserted scripts prepared at their post-connection step whose
    /// fetch the loader has not started yet: what prepare saw, and the
    /// preparation-time document. Each entry pins its node.
    pub(crate) prepared_scripts: rustc_hash::FxHashMap<NodeId, (ScriptInfo, NodeId)>,
    /// Elements with `autofocus` connected since the embedder last took them, in insertion
    /// (tree) order: HTML's autofocus insertion steps, which the embedder completes by
    /// appending them to their top-level document's autofocus candidates.
    pub(crate) autofocus: Vec<NodeId>,
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
        // Nothing in a `<use>` instance loads or runs (SVG 2: its scripts do
        // not execute; it is not a document tree of its own).
        // (`push_resource` keeps a `<use>` instance's resources from loading.)
        self.is_connected_idx(i) || self.is_fragment_parse_host(self.n(i).owner_doc)
    }

    /// The elements with `autofocus` inserted into a document since the last call, in
    /// insertion order (each insertion again; the embedder keeps the last).
    pub fn take_autofocus_insertions(&mut self) -> Vec<NodeId> {
        std::mem::take(&mut self.resources.autofocus)
    }

    /// Whether `take_autofocus_insertions` would return anything.
    pub fn has_autofocus_insertions(&self) -> bool {
        !self.resources.autofocus.is_empty()
    }

    /// An HTML, SVG or MathML element with an `autofocus` attribute.
    fn is_autofocus_idx(&self, i: u32) -> bool {
        self.n(i).element().is_some_and(|e| {
            e.attr_local(&local_name!("autofocus")).is_some()
                && (e.is_html() || e.name.ns == ns!(svg) || e.name.ns == ns!(mathml))
        })
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
        if e.name.ns == ns!(svg) && e.name.local == local_name!("use") {
            return Some(ResourceKind::SvgUse);
        }
        if e.name.ns == ns!(svg) && e.name.local == local_name!("image") {
            return Some(ResourceKind::SvgImage);
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
            local_name!("object") if e.is_html() => ResourceKind::Object,
            local_name!("embed") if e.is_html() => ResourceKind::Embed,
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
        // A document's referrer policy is its own state, kept with or
        // without a loader listening for resource events.
        let meta = self.is_html_meta(node);
        if meta {
            self.process_referrer_meta(node);
        }
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
        if self.is_connected_idx(node) && self.is_autofocus_idx(node) {
            self.resources.autofocus.push(self.id_of(node));
        }
        if meta {
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
        // `src`) prepares it again. An SVG script's source is its `href`
        // (`xlink:href`), as in Chrome.
        let source_attr = match self.n(idx).element() {
            Some(e) if e.name.ns == ns!(svg) => "href",
            _ => "src",
        };
        // HTML: a script whose force async is true loses it when it gets an
        // `async` attribute.
        if local == "async" && added {
            if let Some(e) = self.n_mut(idx).element_mut() {
                if e.is_html() && e.name.local == local_name!("script") {
                    e.script_force_async = false;
                }
            }
        }
        let script_src = local == source_attr && added && self.script_steps_due(idx);
        let meta = matches!(local, "name" | "content") && self.is_html_meta(idx);
        if meta {
            self.process_referrer_meta(idx);
        }
        // pyro: a link given the `disabled` attribute loses its sheet object at once (Chrome: no
        // `sheet`; enabled again, a new one): its sheet generation moves on now, not when the
        // loader handles the event, so the old object is detached and its flag no longer applies.
        if local == "disabled"
            && added
            && self.n(idx).element().is_some_and(|e| e.is_html() && &*e.name.local == "link")
        {
            let generation = self.resources.sheet_generations.entry(self.id_of(idx)).or_default();
            *generation = generation.wrapping_add(1);
        }
        if !self.resources.enabled {
            return;
        }
        if meta {
            self.process_viewport_meta(idx);
        }
        // An `<img>`'s CORS settings and referrer policy are relevant mutations too (HTML "update
        // the image data" restarts with them).
        let relevant = matches!(local, "src" | "href" | "rel" | "media" | "srcset" | "type" | "disabled" | "data")
            || (matches!(local, "crossorigin" | "referrerpolicy" | "loading")
                && self.n(idx).element().is_some_and(|e| e.is_html() && e.name.local == local_name!("img")))
            // pyro: an SVG `<image>`'s CORS settings are its request's mode too (a change is a new
            // request, with its own tainting).
            || (local == "crossorigin"
                && self.n(idx).element().is_some_and(|e| e.name.ns == ns!(svg) && &*e.name.local == "image"));
        if !relevant {
            return;
        }
        if !self.connected_for_resources(idx) {
            // HTML "update the image data" runs for an `<img>` whether or not
            // it is connected (`new Image()`, a tree built before insertion).
            // The loader drops events of a document without a browsing
            // context (template contents, createHTMLDocument).
            if matches!(local, "src" | "srcset" | "loading")
                && self.n(idx).element().is_some_and(|e| e.is_html() && e.name.local == local_name!("img"))
            {
                let reason =
                    if local == "loading" { ResourceReason::AttrChanged } else { ResourceReason::SourceChanged };
                self.push_resource(idx, ResourceKind::Img, reason);
            }
            return;
        }
        if let Some(k) = self.resource_kind(idx) {
            if k == ResourceKind::Script && !script_src {
                return;
            }
            let reason = if k == ResourceKind::Img && matches!(local, "src" | "srcset") {
                ResourceReason::SourceChanged
            } else {
                ResourceReason::AttrChanged
            };
            self.push_resource(idx, k, reason);
        } else if local == "rel" {
            // rel removed "stylesheet": the old sheet must go.
            if self.n(idx).element().is_some_and(|e| e.is_html() && e.name.local == local_name!("link")) {
                self.push_resource(idx, ResourceKind::Stylesheet, ResourceReason::Unlinked);
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
            // An SVG `<image>`'s request is its own kind (`SVGImageElement.decode()`).
            let kind = if self.resource_kind(i) == Some(ResourceKind::SvgImage) {
                ResourceKind::SvgImage
            } else {
                ResourceKind::Img
            };
            self.push_resource(i, kind, ResourceReason::AttrChanged);
        }
    }

    /// Generation of a stylesheet owner's current CSS style sheet object.
    pub fn style_sheet_generation(&self, node: NodeId) -> u32 {
        self.resources.sheet_generations.get(&node).copied().unwrap_or(0)
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
        let track_autofocus = reason == ResourceReason::Connected && self.is_connected_idx(node);
        let mut autofocus = Vec::new();
        while i != NIL {
            if track_autofocus && self.is_autofocus_idx(i) {
                autofocus.push(self.id_of(i));
            }
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
        // The walk is in reverse: the insertion steps run in tree order.
        self.resources.autofocus.extend(autofocus.into_iter().rev());
        if metas && reason == ResourceReason::Connected {
            self.process_viewport_metas(node);
        }
    }

    pub(crate) fn push_resource(&mut self, idx: u32, kind: ResourceKind, reason: ResourceReason) {
        // Nothing in a `<use>` instance loads or runs but an SVG `<image>`,
        // as in browsers (its `load`/`error` reach the instance's clone).
        if kind != ResourceKind::SvgImage && self.n(idx).in_shadow_tree() && self.in_ua_shadow_tree(idx) {
            return;
        }
        let ev = ResourceEvent { node: self.id_of(idx), kind, reason };
        if matches!((kind, reason), (ResourceKind::StyleText | ResourceKind::Stylesheet, ResourceReason::Connected)
            | (ResourceKind::StyleText, ResourceReason::ChildrenChanged))
        {
            let generation = self.resources.sheet_generations.entry(ev.node).or_default();
            *generation = generation.wrapping_add(1);
        }
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
        // `language` is an attribute of HTML's `<script>` only: an SVG one ignores it.
        let lang = if e.is_html() { e.attr_local(&local_name!("language")) } else { None };
        let kind = match ty {
            Some(t) if !t.is_empty() => {
                // JavaScript MIME types match with surrounding ASCII
                // whitespace stripped; "module" and "importmap" must match
                // the whole attribute value (HTML "prepare the script
                // element" step 8: `type="module "` is a data block).
                let raw = t.to_ascii_lowercase();
                let t = raw.trim_matches(|c: char| c.is_ascii_whitespace());
                if JS_MIME.contains(&t) {
                    ScriptKind::Classic
                } else if raw == "module" {
                    ScriptKind::Module
                } else if raw == "importmap" {
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
        // An SVG script's source is its `href`, else its `xlink:href` (SVG 2), and it has no
        // `async`, `defer` or `nomodule` (Chrome: SVGScriptElement is never async or deferred).
        let svg = e.name.ns == ns!(svg);
        let src = if svg {
            e.attr_local(&local_name!("href")).or_else(|| {
                e.attrs
                    .iter()
                    .find(|a| a.name.ns == ns!(xlink) && a.name.local == local_name!("href"))
                    .map(|a| &*a.value)
            })
        } else {
            e.attr_local(&local_name!("src"))
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
            src: src.map(str::to_owned),
            async_: !svg && e.attr_local(&local_name!("async")).is_some(),
            defer: !svg && e.attr_local(&local_name!("defer")).is_some(),
            nomodule: !svg && (e.attr_local(&local_name!("nomodule")).is_some() || !for_window_onload(e)),
            parser_inserted: e.flags & element_flags::PARSER_INSERTED != 0,
            already_started: e.flags & element_flags::SCRIPT_ALREADY_STARTED != 0,
            force_async: e.script_force_async,
            inline_text,
            crossorigin: e.attr_local(&local_name!("crossorigin")).map(str::to_owned),
            integrity: e.attr_local(&local_name!("integrity")).map(str::to_owned),
            referrerpolicy: e.attr_local(&markup5ever::LocalName::from("referrerpolicy")).map(str::to_owned),
            charset: if svg { None } else { e.attr_local(&local_name!("charset")).map(str::to_owned) },
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

    /// Records a script-inserted script prepared at insertion (already
    /// started): its `info` as prepare saw it and its preparation-time
    /// `document`, for the loader's resource-event drain to start its fetch
    /// from even if the element was removed, moved or lost its `src` since.
    pub fn stash_prepared_script(&mut self, script: NodeId, info: ScriptInfo, document: NodeId) -> DomResult<()> {
        self.add_ref(script)?;
        if self.resources.prepared_scripts.insert(script, (info, document)).is_some() {
            self.release(script);
        }
        Ok(())
    }

    /// Takes what [`Dom::stash_prepared_script`] recorded for `script`.
    pub fn take_prepared_script(&mut self, script: NodeId) -> Option<(ScriptInfo, NodeId)> {
        let entry = self.resources.prepared_scripts.remove(&script)?;
        self.release(script);
        Some(entry)
    }

    /// Sets the script's "force async" (the `async` IDL attribute's setter
    /// clears it; a parser-inserted script that prepare returns from before
    /// "already started" without an `async` attribute gets it).
    pub fn set_script_force_async(&mut self, script: NodeId, force_async: bool) -> DomResult<()> {
        let i = self.check(script)?;
        let e = self.n_mut(i).element_mut().ok_or(DomError::Type("not an element"))?;
        e.script_force_async = force_async;
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

/// HTML "prepare the script element": a classic script with both `event` and `for`
/// attributes runs only for `for` "window" and `event` "onload" or "onload()" (ASCII
/// whitespace stripped, ASCII case-insensitive).
fn for_window_onload(e: &crate::node::ElementData) -> bool {
    let (Some(event), Some(for_)) = (e.attr("event"), e.attr("for")) else {
        return true;
    };
    let trim = |v: &str| v.trim_matches(|c: char| c.is_ascii_whitespace()).to_owned();
    let (event, for_) = (trim(event), trim(for_));
    for_.eq_ignore_ascii_case("window")
        && (event.eq_ignore_ascii_case("onload") || event.eq_ignore_ascii_case("onload()"))
}

#[cfg(test)]
mod language_attribute_tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::ScriptKind;
    use crate::parse::{parse_document, DomRef};
    use crate::Dom;

    /// `language` makes an HTML `<script>` a data block, never an SVG one
    /// (html/semantics/scripting-1/the-script-element/script-type-and-language-js-svg.svg).
    #[test]
    fn only_html_scripts_read_language() {
        let dom_ref: DomRef = Rc::new(RefCell::new(Dom::new()));
        let doc = parse_document(
            &dom_ref,
            "http://test/",
            "<!doctype html><script id=h language=javascript1.9></script><svg><script id=s language=javascript1.9></script></svg>",
        );
        let dom = dom_ref.borrow();
        let kind = |id: &str| dom.script_info(dom.get_element_by_id(doc, id).unwrap().unwrap()).unwrap().kind;
        assert_eq!(kind("h"), ScriptKind::Data);
        assert_eq!(kind("s"), ScriptKind::Classic);
    }

    /// Autofocus insertions are recorded in tree order, again on each insertion.
    #[test]
    fn autofocus_insertions_in_insertion_order() {
        let dom_ref: DomRef = Rc::new(RefCell::new(Dom::new()));
        dom_ref.borrow_mut().set_resource_tracking(true);
        let doc = parse_document(
            &dom_ref,
            "http://test/",
            "<!doctype html><input id=a autofocus><div><input id=b autofocus><input id=c></div>",
        );
        let mut dom = dom_ref.borrow_mut();
        let id = |dom: &Dom, name: &str| dom.get_element_by_id(doc, name).unwrap().unwrap();
        let (a, b) = (id(&dom, "a"), id(&dom, "b"));
        assert_eq!(dom.take_autofocus_insertions(), [a, b]);
        let div = dom.parent(b).unwrap().unwrap();
        dom.append_child(div, a).unwrap();
        assert_eq!(dom.take_autofocus_insertions(), [a]);
        assert!(!dom.has_autofocus_insertions());
    }

    /// `event` + `for` other than window's onload: never run, like `nomodule`
    /// (html/semantics/scripting-1/the-script-element/execution-timing/149.html).
    #[test]
    fn event_and_for_other_than_window_onload_never_run() {
        let dom_ref: DomRef = Rc::new(RefCell::new(Dom::new()));
        let doc = parse_document(
            &dom_ref,
            "http://test/",
            "<!doctype html><script id=a for=window event='onload()'></script>\
             <script id=b for=' WINdow\t' event='ONload\n'></script>\
             <script id=c for=window event=load></script>\
             <script id=d for=window event=onpageshow></script>\
             <script id=e for=document event=onload></script>\
             <script id=f event=load></script><script id=g for=document></script>",
        );
        let dom = dom_ref.borrow();
        let skipped = |id: &str| dom.script_info(dom.get_element_by_id(doc, id).unwrap().unwrap()).unwrap().nomodule;
        assert!(!skipped("a") && !skipped("b"));
        assert!(skipped("c") && skipped("d") && skipped("e"));
        // Only both attributes together are checked.
        assert!(!skipped("f") && !skipped("g"));
    }
}
