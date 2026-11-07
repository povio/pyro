//! Node storage types. Nodes live in a `Dom` arena slot and are addressed by
//! `NodeId` (slot index + generation) so stale handles are detected instead of
//! aliasing reused memory.

use markup5ever::{local_name, LocalName, Namespace, Prefix, QualName};

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
    /// `url` parsed, made on first use (`Dom::document_url_parsed`) and
    /// reset when the URL changes; `None` inside when it does not parse.
    pub(crate) parsed_url: std::cell::OnceCell<Option<url::Url>>,
    /// Embedder-supplied inherited base for initial blank/srcdoc documents.
    pub(crate) base_url: Option<Box<str>>,
    /// Resolved first `<base href>`, computed on demand and reset when a
    /// `<base>` enters or leaves the document tree, a connected one's `href`
    /// changes, or the fallback base URL changes.
    pub(crate) html_base: std::cell::OnceCell<Option<Box<str>>>,
    /// The URL's fragment, percent-decoded, as an id (`:target`); `None`
    /// without one (or when it does not decode to UTF-8).
    pub(crate) url_target: Option<Atom>,
    /// Elements of the document tree by id (`getElementById`).
    pub(crate) ids: crate::tree_scope::IdMap,
    /// Owner of `<template>` contents created while parsing this document.
    pub(crate) inert_doc: u32,
    /// Number of live nodes (including itself) whose owner document is this.
    pub(crate) owned: u32,
    /// How many of those nodes have an external reference (`Node::ext_refs`
    /// above zero). While it is not zero the document cannot be collected,
    /// which `Dom::collect_garbage` then knows without walking its tree: a
    /// page's wrappers die in many small batches after a navigation, and each
    /// batch otherwise cost a walk of the whole old document.
    pub(crate) held: u32,
    /// Set while a browsing context (window) displays this document. Such a
    /// document and its tree are never collected.
    pub(crate) active: bool,
    /// HTML "allow declarative shadow roots": documents displayed in a
    /// browsing context and `parseHTMLUnsafe`, not DOMParser or XHR.
    pub(crate) allow_declarative_shadow_roots: bool,
    /// The temporary document the HTML fragment parsing algorithm builds
    /// in. It is not connected (nothing observes its tree as such), so its
    /// nodes get no tree flags and its ids no map: the parsed nodes leave
    /// it without a traversal and enter their real tree scope once.
    pub(crate) fragment_parse_host: bool,
    /// Focused element (NIL = none, i.e. body is the active element).
    pub(crate) focused: u32,
    /// Element under the (virtual) pointer for :hover.
    pub(crate) hovered: u32,
    /// Element being activated (a primary press or Space on it) for
    /// `:active`, which also matches its shadow-including ancestors.
    pub(crate) activated: u32,
    /// The focused element matches `:focus-visible` (Chrome's heuristics,
    /// decided by the bindings when focus moves).
    pub(crate) focus_visible: bool,
    /// The document element (first element child), its first HTML `head`
    /// child and its first HTML `body` or `frameset` child; NIL for none.
    /// Kept by `tree_linked`/`tree_unlinking`, which every link and unlink
    /// passes, so `document.body` is one load, not two child-list walks.
    pub(crate) document_element: u32,
    pub(crate) head: u32,
    pub(crate) body: u32,
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

/// An `Attr` node: an attribute seen as a node (`getAttributeNode`,
/// `createAttribute`). While `owner` is set the element's attribute list holds
/// the value; `value` is the value of a detached one.
#[derive(Debug, Clone)]
pub struct AttrNodeData {
    pub name: QualName,
    pub(crate) value: String,
    /// Element this attribute belongs to; NIL while detached.
    pub(crate) owner: u32,
}

impl AttrNodeData {
    /// Qualified name (`Attr.name`).
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
    /// `shadow_flags` of a shadow root.
    pub(crate) flags: u8,
    /// A shadow root's elements by id, made when the first id enters it.
    pub(crate) ids: Option<Box<crate::tree_scope::IdMap>>,
}

impl FragmentData {
    pub(crate) fn new(host: u32, shadow: Option<ShadowRootMode>, flags: u8) -> Self {
        FragmentData { host, shadow, flags, ids: None }
    }
}

/// Shadow root state (`attachShadow` init dictionary, HTML `shadowroot*`
/// attributes and DOM's "declarative").
pub mod shadow_flags {
    pub const CLONABLE: u8 = 1;
    pub const SERIALIZABLE: u8 = 2;
    pub const DELEGATES_FOCUS: u8 = 4;
    /// `slotAssignment: "manual"`.
    pub const MANUAL_SLOTS: u8 = 8;
    /// Made by the parser and not yet claimed by `attachShadow`.
    pub const DECLARATIVE: u8 = 16;
    /// "Available to element internals": attached while the host was a
    /// custom element (state "precustomized" or "custom"), so
    /// `ElementInternals.shadowRoot` returns it.
    pub const AVAILABLE_TO_INTERNALS: u8 = 32;
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
    /// Selection direction script gave (`true`: backward), with the
    /// selection it was given for: any other selection has direction "none".
    pub(crate) selection_direction: Option<(bool, (u32, u32))>,
    /// Value changed by user editing (enables tooLong/tooShort).
    pub user_edited: bool,
    /// HTML "user validity": the user committed a change, or a submission
    /// validated the form (`:user-valid` / `:user-invalid`); reset clears it.
    pub user_validity: bool,
    /// `setCustomValidity` message.
    pub custom_validity: Option<String>,
    /// `<option>` selectedness when explicitly set.
    pub selected: Option<bool>,
    /// `<select>`: script deselected every option (`selectedIndex = -1`) of
    /// this list of options, so none is selected by default until an option
    /// is inserted, removed or selected.
    pub(crate) no_selection: Option<Box<[u32]>>,
}

/// Atoms of ids and class names: Stylo's, which the cascade and both
/// selector engines compare (one `u64` per comparison).
pub type Atom = style::Atom;

#[derive(Debug)]
pub struct ElementData {
    pub name: QualName,
    /// Read freely; written only through the methods that keep `id` and
    /// `classes` in step (`set_attrs`, `push_attr`, `remove_attr`,
    /// `replace_attr_value`, `rename_attr`).
    pub attrs: Vec<Attr>,
    /// The `id` attribute (no namespace) as an atom, parsed when written.
    id: Option<Atom>,
    /// The `class` attribute (no namespace) split on ASCII whitespace,
    /// parsed when written.
    classes: Box<[Atom]>,
    pub(crate) template_contents: u32,
    pub(crate) shadow_root: u32,
    pub(crate) control: Option<Box<ControlState>>,
    /// `element_flags` bits.
    pub flags: u8,
    /// An HTML `<style>`: the `Dom::style_text_revision` at which its child
    /// text content last changed (0: not since it was created).
    pub(crate) style_text_revision: u64,
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
    /// The element's custom element definition observes attributes
    /// (`observedAttributes` with an `attributeChangedCallback`): attribute
    /// changes queue reactions. Unset, an attribute change pays one test.
    pub const OBSERVES_ATTRIBUTES: u8 = 16;
    /// Custom element state "failed": its constructor threw, so it is
    /// never upgraded again.
    pub const CUSTOM_FAILED: u8 = 32;
    /// A form-associated custom element (`static formAssociated = true`).
    pub const FORM_ASSOCIATED: u8 = 64;
}

impl ElementData {
    pub fn new(name: QualName) -> Self {
        ElementData {
            name,
            attrs: Vec::new(),
            id: None,
            classes: Box::default(),
            template_contents: NIL,
            shadow_root: NIL,
            control: None,
            flags: 0,
            style_text_revision: 0,
        }
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
    /// The attribute in no namespace named `local` (a dynamic name; see
    /// [`ElementData::attr_local`] for a constant one).
    pub fn attr(&self, local: &str) -> Option<&str> {
        self.attrs.iter().find(|a| a.name.ns == markup5ever::ns!() && &*a.name.local == local).map(|a| &*a.value)
    }
    /// The attribute in no namespace named `local`, compared as atoms (for
    /// `local_name!(..)` names).
    #[inline]
    pub fn attr_local(&self, local: &LocalName) -> Option<&str> {
        self.attrs.iter().find(|a| a.name.local == *local && a.name.ns == markup5ever::ns!()).map(|a| &*a.value)
    }
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }
    /// The `id` attribute as an atom.
    #[inline]
    pub fn id_atom(&self) -> Option<&Atom> {
        self.id.as_ref()
    }
    /// The `class` attribute's tokens as atoms, in order (duplicates kept).
    #[inline]
    pub fn classes(&self) -> &[Atom] {
        &self.classes
    }
    /// The atom this element keeps for attribute `k`'s value, if any: its
    /// `id` atom when `k` is `id` (no namespace).
    #[inline]
    pub fn attr_atom(&self, k: usize) -> Option<&Atom> {
        let name = &self.attrs.get(k)?.name;
        if name.local == local_name!("id") && name.ns == markup5ever::ns!() {
            self.id.as_ref()
        } else {
            None
        }
    }
    /// The `class` attribute (no namespace), with its token's atom when the
    /// value is one token and nothing else (`className`).
    #[inline]
    pub fn class_value(&self) -> Option<(&str, Option<&Atom>)> {
        let value = self.attr_local(&local_name!("class"))?;
        // The token is the value less ASCII whitespace: one as long is the
        // whole value (and the value's text need not be read).
        let only = match &*self.classes {
            [only] if only.len() == value.len() => Some(only),
            _ => None,
        };
        Some((value, only))
    }
    /// Whether `class` contains the token `name` (case-sensitive).
    #[inline]
    pub fn has_class(&self, name: &Atom) -> bool {
        self.classes.contains(name)
    }

    // ----- attribute writes (keep `id` and `classes` in step) ----------

    fn refresh(&mut self, local: &LocalName) {
        if *local == local_name!("id") {
            self.id = self.attr_local(&local_name!("id")).map(Atom::from);
        } else if *local == local_name!("class") {
            self.classes = self.attr_local(&local_name!("class")).map(parse_classes).unwrap_or_default();
        }
    }
    /// Replaces every attribute (element creation, cloning).
    pub(crate) fn set_attrs(&mut self, attrs: Vec<Attr>) {
        self.attrs = attrs;
        self.refresh(&local_name!("id"));
        self.refresh(&local_name!("class"));
    }
    /// The attributes with their parsed atoms, for [`ElementData::set_attr_state`]
    /// (cloning: nothing is parsed again).
    pub(crate) fn attr_state(&self) -> AttrState {
        AttrState(self.attrs.clone(), self.id.clone(), self.classes.clone())
    }
    pub(crate) fn set_attr_state(&mut self, state: AttrState) {
        let AttrState(attrs, id, classes) = state;
        self.attrs = attrs;
        self.id = id;
        self.classes = classes;
    }
    pub(crate) fn push_attr(&mut self, attr: Attr) {
        let refresh = derives(&attr.name).then(|| attr.name.local.clone());
        self.attrs.push(attr);
        if let Some(local) = refresh {
            self.refresh(&local);
        }
    }
    pub(crate) fn remove_attr(&mut self, k: usize) -> Attr {
        let attr = self.attrs.remove(k);
        if derives(&attr.name) {
            self.refresh(&attr.name.local);
        }
        attr
    }
    /// Sets attribute `k`'s value, returning the old one.
    pub(crate) fn replace_attr_value(&mut self, k: usize, value: Box<str>) -> Box<str> {
        let old = std::mem::replace(&mut self.attrs[k].value, value);
        if derives(&self.attrs[k].name) && *old != *self.attrs[k].value {
            let local = self.attrs[k].name.local.clone();
            self.refresh(&local);
        }
        old
    }
    /// Gives attribute `k` another name.
    pub(crate) fn rename_attr(&mut self, k: usize, name: QualName) {
        let old = std::mem::replace(&mut self.attrs[k].name, name);
        if derives(&old) {
            self.refresh(&old.local);
        }
        if derives(&self.attrs[k].name) {
            let local = self.attrs[k].name.local.clone();
            self.refresh(&local);
        }
    }
    /// Whether `id` and `classes` match the attributes (tests and debug
    /// assertions).
    #[doc(hidden)]
    pub fn derived_in_step(&self) -> bool {
        let id = self.attr_local(&local_name!("id")).map(Atom::from);
        let classes: Box<[Atom]> = self.attr_local(&local_name!("class")).map(parse_classes).unwrap_or_default();
        self.id == id && self.classes == classes
    }
}

/// An element's attributes with their parsed id and classes.
pub(crate) struct AttrState(Vec<Attr>, Option<Atom>, Box<[Atom]>);

/// Whether an attribute of this name feeds `ElementData::id` or `classes`.
#[inline]
fn derives(name: &QualName) -> bool {
    name.ns == markup5ever::ns!() && (name.local == local_name!("id") || name.local == local_name!("class"))
}

fn parse_classes(value: &str) -> Box<[Atom]> {
    // Sized once: boxing a collected `Vec` reallocates it to its length.
    let mut classes = Vec::with_capacity(value.split_ascii_whitespace().count());
    classes.extend(value.split_ascii_whitespace().map(Atom::from));
    classes.into_boxed_slice()
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
    /// An attribute as a node; never linked into a tree.
    Attr(Box<AttrNodeData>),
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
            NodeKind::Attr(_) => NodeType::Attribute,
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
    /// Number of children (low bits) and `tree_flags` (high bits), packed so
    /// the header keeps to 64 bytes.
    counts: u32,
    /// External strong references: JS wrappers (one per realm), pending
    /// mutation records, ranges, iterators. Collection requires zero refs in
    /// the whole detached tree.
    pub(crate) ext_refs: u32,
    pub kind: NodeKind,
}

/// Where a node is, kept in its header by every link and unlink (see
/// `crate::tree_scope`) so the questions are bit tests, not walks to the root.
pub(crate) mod tree_flags {
    /// Its shadow-including root is a document (`isConnected`).
    pub const CONNECTED: u32 = 1 << 31;
    /// Its root is a shadow root (the shadow root itself included).
    pub const IN_SHADOW_TREE: u32 = 1 << 30;
    pub const MASK: u32 = CONNECTED | IN_SHADOW_TREE;
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
            counts: 0,
            ext_refs: 0,
            kind,
        }
    }
    #[inline]
    pub(crate) fn child_count(&self) -> u32 {
        self.counts & !tree_flags::MASK
    }
    #[inline]
    pub(crate) fn add_child_count(&mut self) {
        debug_assert!(self.child_count() < !tree_flags::MASK, "child count overflow");
        self.counts += 1;
    }
    #[inline]
    pub(crate) fn sub_child_count(&mut self) {
        debug_assert!(self.child_count() > 0);
        self.counts -= 1;
    }
    /// `tree_flags` bits.
    #[inline]
    pub(crate) fn tree_flags(&self) -> u32 {
        self.counts & tree_flags::MASK
    }
    #[inline]
    pub(crate) fn set_tree_flags(&mut self, flags: u32) {
        self.counts = (self.counts & !tree_flags::MASK) | (flags & tree_flags::MASK);
    }
    #[inline]
    pub(crate) fn is_connected(&self) -> bool {
        self.counts & tree_flags::CONNECTED != 0
    }
    #[inline]
    pub(crate) fn in_shadow_tree(&self) -> bool {
        self.counts & tree_flags::IN_SHADOW_TREE != 0
    }
    /// In a document's tree (connected, not in a shadow tree): Stylo's
    /// `is_in_document`.
    #[inline]
    pub(crate) fn in_document_tree(&self) -> bool {
        self.tree_flags() == tree_flags::CONNECTED
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
