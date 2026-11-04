//! Node storage types. Nodes live in a `Dom` arena slot and are addressed by
//! `NodeId` (slot index + generation) so stale handles are detected instead of
//! aliasing reused memory.

use markup5ever::{LocalName, Namespace, Prefix, QualName};

pub(crate) const NIL: u32 = u32::MAX;

/// Handle to a node inside one `Dom` store. Copyable; validity is checked on
/// every access via the generation counter.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId {
    pub(crate) idx: u32,
    pub(crate) gen: u32,
}

impl NodeId {
    /// Raw slot index; stable for the node's lifetime, reused after collection.
    pub fn index(self) -> u32 {
        self.idx
    }
    pub fn generation(self) -> u32 {
        self.gen
    }
}

/// DOM `Node.nodeType` values.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum NodeType {
    Element = 1,
    Attribute = 2,
    Text = 3,
    CDataSection = 4,
    ProcessingInstruction = 7,
    Comment = 8,
    Document = 9,
    DocumentType = 10,
    DocumentFragment = 11,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum QuirksMode {
    NoQuirks,
    LimitedQuirks,
    Quirks,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DocumentKind {
    Html,
    Xml,
}

#[derive(Debug)]
pub struct DocumentData {
    pub kind: DocumentKind,
    pub quirks: QuirksMode,
    pub content_type: Box<str>,
    /// Encoding name (`document.characterSet`).
    pub charset: &'static str,
    pub url: Box<str>,
    /// Embedder-supplied inherited base for initial blank/srcdoc documents.
    pub(crate) base_url: Option<Box<str>>,
    /// Resolved first `<base href>`, computed on demand; see `Dom::bump`.
    pub(crate) html_base: std::cell::OnceCell<Option<Box<str>>>,
    /// Owner of `<template>` contents created while parsing this document.
    pub(crate) inert_doc: u32,
    /// Number of live nodes (including itself) whose owner document is this.
    pub(crate) owned: u32,
    /// Set while a browsing context (window) displays this document. Such a
    /// document and its tree are never collected.
    pub(crate) active: bool,
    /// Focused element (NIL = none, i.e. body is the active element).
    pub(crate) focused: u32,
    /// Element under the (virtual) pointer for :hover.
    pub(crate) hovered: u32,
}

#[derive(Debug, Clone)]
pub struct PiData {
    pub target: Box<str>,
    pub data: String,
}

#[derive(Debug)]
pub struct DoctypeData {
    pub name: Box<str>,
    pub public_id: Box<str>,
    pub system_id: Box<str>,
}

#[derive(Debug, Clone)]
pub struct Attr {
    pub name: QualName,
    pub value: Box<str>,
}

impl Attr {
    /// Qualified name as exposed by `Attr.name` / `getAttributeNames()`.
    pub fn qualified_name(&self) -> String {
        match &self.name.prefix {
            Some(p) if !p.is_empty() => format!("{}:{}", p, self.name.local),
            _ => self.name.local.to_string(),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ShadowRootMode {
    Open,
    Closed,
}

#[derive(Debug)]
pub struct FragmentData {
    /// Template element (for template contents) or shadow host; NIL otherwise.
    pub(crate) host: u32,
    pub(crate) shadow: Option<ShadowRootMode>,
}

/// Mutable form-control state that is not reflected in attributes.
#[derive(Debug, Default, Clone)]
pub struct ControlState {
    /// Current value; `None` means "use default value" (value attribute / text).
    pub value: Option<String>,
    pub dirty_value: bool,
    pub checked: Option<bool>,
    pub dirty_checked: bool,
    pub indeterminate: bool,
    /// Selection in UTF-16 code units (start, end).
    pub selection: (u32, u32),
    /// Selection explicitly set (otherwise caret sits at the end of value).
    pub selection_set: bool,
    /// Value changed by user editing (enables tooLong/tooShort).
    pub user_edited: bool,
    /// `setCustomValidity` message.
    pub custom_validity: Option<String>,
    /// `<option>` selectedness when explicitly set.
    pub selected: Option<bool>,
}

#[derive(Debug)]
pub struct ElementData {
    pub name: QualName,
    pub attrs: Vec<Attr>,
    pub(crate) template_contents: u32,
    pub(crate) shadow_root: u32,
    pub(crate) control: Option<Box<ControlState>>,
    /// `element_flags` bits.
    pub flags: u8,
}

pub mod element_flags {
    /// Script "already started" (HTML §4.12.1.1).
    pub const SCRIPT_ALREADY_STARTED: u8 = 1;
    /// Inserted by the HTML parser (as opposed to DOM APIs).
    pub const PARSER_INSERTED: u8 = 2;
    /// Created by the fragment parser (innerHTML etc.); scripts never run.
    pub const FRAGMENT_PARSED: u8 = 4;
    /// Custom element has been defined/upgraded (`:defined`).
    pub const CUSTOM_DEFINED: u8 = 8;
}

impl ElementData {
    pub fn new(name: QualName) -> Self {
        ElementData { name, attrs: Vec::new(), template_contents: NIL, shadow_root: NIL, control: None, flags: 0 }
    }
    pub fn local_name(&self) -> &LocalName {
        &self.name.local
    }
    pub fn namespace(&self) -> &Namespace {
        &self.name.ns
    }
    pub fn prefix(&self) -> Option<&Prefix> {
        self.name.prefix.as_ref()
    }
    pub fn is_html(&self) -> bool {
        self.name.ns == markup5ever::ns!(html)
    }
    pub fn attr(&self, local: &str) -> Option<&str> {
        self.attrs.iter().find(|a| a.name.ns == markup5ever::ns!() && &*a.name.local == local).map(|a| &*a.value)
    }
    pub fn id(&self) -> Option<&str> {
        self.attr("id")
    }
}

#[derive(Debug)]
pub enum NodeKind {
    Document(Box<DocumentData>),
    Doctype(Box<DoctypeData>),
    Fragment(FragmentData),
    Element(Box<ElementData>),
    Text(String),
    CData(String),
    Comment(String),
    ProcessingInstruction(Box<PiData>),
}

impl NodeKind {
    pub fn node_type(&self) -> NodeType {
        match self {
            NodeKind::Document(_) => NodeType::Document,
            NodeKind::Doctype(_) => NodeType::DocumentType,
            NodeKind::Fragment(_) => NodeType::DocumentFragment,
            NodeKind::Element(_) => NodeType::Element,
            NodeKind::Text(_) => NodeType::Text,
            NodeKind::CData(_) => NodeType::CDataSection,
            NodeKind::Comment(_) => NodeType::Comment,
            NodeKind::ProcessingInstruction(_) => NodeType::ProcessingInstruction,
        }
    }
    /// `CharacterData.data` for text-like nodes.
    pub fn char_data(&self) -> Option<&str> {
        match self {
            NodeKind::Text(s) | NodeKind::CData(s) | NodeKind::Comment(s) => Some(s),
            NodeKind::ProcessingInstruction(p) => Some(&p.data),
            _ => None,
        }
    }
    pub(crate) fn char_data_mut(&mut self) -> Option<&mut String> {
        match self {
            NodeKind::Text(s) | NodeKind::CData(s) | NodeKind::Comment(s) => Some(s),
            NodeKind::ProcessingInstruction(p) => Some(&mut p.data),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct Node {
    pub(crate) parent: u32,
    pub(crate) first_child: u32,
    pub(crate) last_child: u32,
    pub(crate) prev: u32,
    pub(crate) next: u32,
    pub(crate) owner_doc: u32,
    pub(crate) child_count: u32,
    /// External strong references: JS wrappers (one per realm), pending
    /// mutation records, ranges, iterators. Collection requires zero refs in
    /// the whole detached tree.
    pub(crate) ext_refs: u32,
    pub kind: NodeKind,
}

impl Node {
    pub(crate) fn new(kind: NodeKind, owner_doc: u32) -> Self {
        Node {
            parent: NIL,
            first_child: NIL,
            last_child: NIL,
            prev: NIL,
            next: NIL,
            owner_doc,
            child_count: 0,
            ext_refs: 0,
            kind,
        }
    }
    pub fn element(&self) -> Option<&ElementData> {
        match &self.kind {
            NodeKind::Element(e) => Some(e),
            _ => None,
        }
    }
    pub(crate) fn element_mut(&mut self) -> Option<&mut ElementData> {
        match &mut self.kind {
            NodeKind::Element(e) => Some(e),
            _ => None,
        }
    }
    pub fn is_element(&self) -> bool {
        matches!(self.kind, NodeKind::Element(_))
    }
}

#[cfg(test)]
mod size_tests {
    use super::*;
    #[test]
    fn node_header_is_compact() {
        // 7 u32 links/counters + ext_refs + 32-byte kind (String payload + tag).
        assert!(std::mem::size_of::<Node>() <= 64, "Node = {}", std::mem::size_of::<Node>());
    }
}
