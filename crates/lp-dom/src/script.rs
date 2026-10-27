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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    Script,
    /// `<link rel=stylesheet>` (also covers `rel` containing "stylesheet").
    Stylesheet,
    /// `<style>` element whose text changed or that was (dis)connected.
    StyleText,
    Iframe,
    Img,
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
        Some(match &*e.name.local {
            "script" => ResourceKind::Script,
            "style" => ResourceKind::StyleText,
            "link"
                if e.attr("rel")
                    .is_some_and(|r| r.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case("stylesheet"))) =>
            {
                ResourceKind::Stylesheet
            }
            "iframe" | "frame" => ResourceKind::Iframe,
            "img" => ResourceKind::Img,
            _ => return None,
        })
    }

    fn connected_idx(&self, idx: u32) -> bool {
        let mut i = idx;
        loop {
            i = self.root_idx(i);
            match &self.n(i).kind {
                NodeKind::Document(_) => return true,
                NodeKind::Fragment(f) if f.shadow.is_some() => i = f.host,
                _ => return false,
            }
        }
    }

    /// Called after `node` was inserted (tree mutation or parser).
    pub(crate) fn resources_after_insert(&mut self, node: u32) {
        self.iframes_after_insert(node);
        if !self.resources.enabled || !self.connected_idx(node) {
            return;
        }
        let parent = self.n(node).parent;
        if parent != NIL && self.resource_kind(parent) == Some(ResourceKind::StyleText) {
            self.push_resource(parent, ResourceKind::StyleText, ResourceReason::ChildrenChanged);
        }
        self.scan_subtree(node, ResourceReason::Connected);
    }

    /// Called before `node` is unlinked from a connected parent.
    pub(crate) fn resources_before_remove(&mut self, node: u32) {
        self.iframes_before_remove(node);
        if !self.resources.enabled || !self.connected_idx(node) {
            return;
        }
        let parent = self.n(node).parent;
        if parent != NIL && self.resource_kind(parent) == Some(ResourceKind::StyleText) {
            self.push_resource(parent, ResourceKind::StyleText, ResourceReason::ChildrenChanged);
        }
        // Only stylesheet owners need removal notifications.
        let mut stack = vec![node];
        while let Some(i) = stack.pop() {
            if let Some(k @ (ResourceKind::Stylesheet | ResourceKind::StyleText)) = self.resource_kind(i) {
                self.push_resource(i, k, ResourceReason::Removed);
            }
            stack.extend(self.child_idx_iter(i));
            if let Some(e) = self.n(i).element() {
                if e.shadow_root != NIL {
                    stack.push(e.shadow_root);
                }
            }
        }
    }

    pub(crate) fn resources_after_attr(&mut self, idx: u32, local: &str) {
        self.iframes_after_attr(idx, local);
        if !self.resources.enabled {
            return;
        }
        let relevant = matches!(local, "src" | "href" | "rel" | "media" | "srcset" | "type" | "disabled");
        if !relevant || !self.connected_idx(idx) {
            return;
        }
        if let Some(k) = self.resource_kind(idx) {
            self.push_resource(idx, k, ResourceReason::AttrChanged);
        } else if local == "rel" {
            // rel removed "stylesheet": the old sheet must go.
            if self.n(idx).element().is_some_and(|e| e.is_html() && e.name.local == local_name!("link")) {
                self.push_resource(idx, ResourceKind::Stylesheet, ResourceReason::Removed);
            }
        }
    }

    pub(crate) fn resources_after_text(&mut self, text_node: u32) {
        if !self.resources.enabled {
            return;
        }
        let p = self.n(text_node).parent;
        if p != NIL && self.resource_kind(p) == Some(ResourceKind::StyleText) && self.connected_idx(p) {
            self.push_resource(p, ResourceKind::StyleText, ResourceReason::ChildrenChanged);
        }
    }

    fn scan_subtree(&mut self, node: u32, reason: ResourceReason) {
        let mut stack = vec![node];
        while let Some(i) = stack.pop() {
            if let Some(k) = self.resource_kind(i) {
                // Parser-inserted scripts are driven by ParseStep::Script.
                let parser_script = k == ResourceKind::Script
                    && self.n(i).element().is_some_and(|e| e.flags & element_flags::PARSER_INSERTED != 0);
                if !parser_script {
                    self.push_resource(i, k, reason);
                }
            }
            stack.extend(self.child_idx_iter(i));
            if let Some(e) = self.n(i).element() {
                if e.shadow_root != NIL {
                    stack.push(e.shadow_root);
                }
            }
        }
    }

    fn push_resource(&mut self, idx: u32, kind: ResourceKind, reason: ResourceReason) {
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
        if self.resources.enabled && self.resource_kind(idx) == Some(ResourceKind::StyleText) && self.connected_idx(idx)
        {
            self.push_resource(idx, ResourceKind::StyleText, ResourceReason::ChildrenChanged);
        }
    }

    /// Classifies a `<script>` element for the script loader.
    pub fn script_info(&self, script: NodeId) -> DomResult<ScriptInfo> {
        let i = self.check(script)?;
        let e = self.n(i).element().ok_or(DomError::Type("not an element"))?;
        let ty = e.attr("type");
        let lang = e.attr("language");
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
            if let NodeKind::Text(s) = &self.n(c).kind {
                inline_text.push_str(s);
            }
        }
        Ok(ScriptInfo {
            kind,
            src: e.attr("src").map(str::to_owned),
            async_: e.attr("async").is_some(),
            defer: e.attr("defer").is_some(),
            nomodule: e.attr("nomodule").is_some(),
            parser_inserted: e.flags & element_flags::PARSER_INSERTED != 0,
            already_started: e.flags & element_flags::SCRIPT_ALREADY_STARTED != 0,
            inline_text,
            crossorigin: e.attr("crossorigin").map(str::to_owned),
            integrity: e.attr("integrity").map(str::to_owned),
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
