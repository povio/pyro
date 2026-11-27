//! HTML parsing into the `Dom` via html5ever's TreeSink. Document parsing is
//! incremental and yields at `</script>` so the loader can execute blocking
//! scripts before resuming (HTML §13.2.6 "script end tag" handling).

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use html5ever::buffer_queue::BufferQueue;
use html5ever::tendril::StrTendril;
use html5ever::tokenizer::{Tokenizer, TokenizerOpts};
use html5ever::tree_builder::{
    ElementFlags, NodeOrText, QuirksMode as H5Quirks, TreeBuilder, TreeBuilderOpts, TreeSink,
};
use html5ever::{Attribute, QualName};
use markup5ever::{local_name, ns, TokenizerResult};

use crate::dom::Dom;
use crate::RegistryAssociation;
use crate::fragment_filter::{Disposition, FragmentFilter};
use rustc_hash::FxHashMap;
use crate::error::{DomError, DomResult};
use crate::node::{
    element_flags, shadow_flags, ElementData, FragmentData, NodeId, NodeKind, QuirksMode, ShadowRootMode, NIL,
};

mod streaming;
pub use streaming::StreamingFragmentParser;

/// Shared handle to a page's DOM store.
pub type DomRef = Rc<RefCell<Dom>>;

trait DomAccess {
    fn insertion_position(&self) -> Option<&streaming::InsertionPosition> {
        None
    }
    fn with<R>(&self, f: impl FnOnce(&mut Dom) -> R) -> R;
}
impl DomAccess for DomRef {
    fn with<R>(&self, f: impl FnOnce(&mut Dom) -> R) -> R {
        f(&mut self.borrow_mut())
    }
}
struct Borrowed<'a>(RefCell<&'a mut Dom>);
impl DomAccess for Borrowed<'_> {
    fn with<R>(&self, f: impl FnOnce(&mut Dom) -> R) -> R {
        f(&mut self.0.borrow_mut())
    }
}

struct Sink<A: DomAccess> {
    dom: A,
    doc: u32,
    /// Flags applied to every created element.
    elem_flags: u8,
    /// The main document parser (scripting enabled): elements of defined
    /// custom element names are constructed synchronously and the
    /// reactions its insertions queue run at once (HTML "create an element
    /// for a token", "insert an element at the adjusted insertion location").
    sync_custom: bool,
    filter: Option<Rc<FragmentFilter>>,
    /// The filter also applies to the children of the document: a document parser's `html`
    /// element and top-level comments. A fragment parser's synthetic `html` root is never filtered.
    filter_document: bool,
    /// Elements the filter replaces with their children, by id, with the place each would have
    /// been inserted at (parent, and the sibling it goes before or NIL; the parent is resolved
    /// through earlier entries). The element is created (the tree builder keeps it on its stack of
    /// open elements) but never inserted: what the parser inserts under it goes to that place
    /// (`Sink::append_to`, `Sink::attach_replaced`). Empty without a filter.
    forwards: RefCell<FxHashMap<u32, (u32, u32)>>,
    /// Present only on the detached, unfiltered parsers that support inline patches.
    patches: Option<RefCell<crate::template_patch::Patches>>,
    /// Elements of a defined name created for a token, with the token's
    /// attributes: constructed (or, in inert template contents, given their
    /// attributes) when the parser inserts them, which is when their
    /// intended parent, and so their node document, is known.
    pending_custom: RefCell<Vec<(u32, Vec<Attribute>)>>,
    /// `pending_custom` is not empty: what each insertion tests.
    has_pending_custom: Cell<bool>,
    quirks: Cell<Option<QuirksMode>>,
    // html5ever keeps raw handles across script pauses, including detached elements.
    pins: RefCell<Vec<NodeId>>,
    /// Real context document when the sink parses into a temporary arena document.
    fragment_document_registry: Option<RegistryAssociation>,
    /// Open `<template shadowrootmode>` elements and the shadow roots their
    /// contents go to. Such a template stays out of the tree and keeps its
    /// own fragment, so collecting it never takes the host's root along.
    declarative: RefCell<Vec<(u32, u32)>>,
    /// This parser created a `<selectedcontent>`: until then a popped option
    /// has none to fill, and the per-option select walk is skipped.
    selectedcontent: Cell<bool>,
    /// The tokenizer's line of the token being processed (`TreeSink::set_current_line`).
    current_line: Cell<u64>,
}

impl<A: DomAccess> Sink<A> {
    fn new(dom: A, doc: u32, elem_flags: u8, sync_custom: bool) -> Self {
        let id = dom.with(|d| {
            let id = d.id_of(doc);
            d.add_ref(id).expect("parser document");
            id
        });
        Sink {
            dom,
            doc,
            elem_flags,
            sync_custom,
            filter: None,
            filter_document: false,
            forwards: RefCell::new(FxHashMap::default()),
            patches: None,
            fragment_document_registry: None,
            pending_custom: RefCell::new(Vec::new()),
            has_pending_custom: Cell::new(false),
            quirks: Cell::new(None),
            pins: RefCell::new(vec![id]),
            declarative: RefCell::new(Vec::new()),
            selectedcontent: Cell::new(false),
            current_line: Cell::new(1),
        }
    }

    fn release_pins(&self) {
        // Elements of defined names the parser never inserted keep their
        // token's attributes.
        let pending = self.pending_custom.take();
        self.has_pending_custom.set(false);
        if !pending.is_empty() {
            self.dom.with(|d| {
                for (n, attrs) in pending {
                    d.set_parsed_attributes(n, attrs.into_iter().map(|a| (a.name, a.value.to_string())).collect());
                }
            });
        }
        let pins = self.pins.take();
        self.dom.with(|d| d.release_parser_pins(pins));
    }
}

impl<A: DomAccess> Drop for Sink<A> {
    fn drop(&mut self) {
        self.release_pins();
    }
}

/// An element's namespace and local name, which the tree builder asks for
/// many times per token (scope and open-element checks). The two atoms are
/// copied without the prefix: HTML's names are static atoms, so each copy
/// and its drop is a tag test, inlined.
#[derive(Debug)]
pub struct ElemNameRef {
    ns: html5ever::Namespace,
    local: html5ever::LocalName,
}
impl html5ever::tree_builder::ElemName for ElemNameRef {
    #[inline]
    fn ns(&self) -> &html5ever::Namespace {
        &self.ns
    }
    #[inline]
    fn local_name(&self) -> &html5ever::LocalName {
        &self.local
    }
}

fn append_text(dom: &mut Dom, parent: u32, before: u32, text: &str) {
    let prev = if before == NIL { dom.n(parent).last_child } else { dom.n(before).prev };
    if prev != NIL {
        if let NodeKind::Text(s) = &mut dom.n_mut(prev).kind {
            s.push_str(text);
            dom.bump_tree();
            dom.note(crate::dom::Change::Text(prev));
            return;
        }
    }
    let owner = dom.n(parent).owner_doc;
    let t = dom.alloc_raw(NodeKind::Text(text.to_owned()), owner);
    dom.link_raw(t, parent, before);
    // The parser's insertions are tree mutations for mutation observers.
    dom.queue_child_list(parent, &[t], &[], prev, before);
}

/// HTML "insert an element at the adjusted insertion location": "if it is not possible to insert
/// element at the adjusted insertion location, abort these steps". A custom element's
/// constructor, run before the insertion, may have made the parser's insertion parent its own
/// descendant (`this.appendChild(form)` for the form it was going into): inserting it would make
/// a cycle (WPT custom-elements custom-button-abort-insertion-crash).
fn cannot_insert(d: &Dom, parent: u32, child: &NodeOrText<u32>) -> bool {
    // A Document (one a script gave children before writing to it) takes no second element:
    // the parser's insertion is dropped, as DOM's pre-insert validity would refuse it.
    if matches!(d.n(parent).kind, NodeKind::Document(_)) {
        if let NodeOrText::AppendNode(n) = child {
            if d.n(*n).is_element() && document_has_child(d, parent, |c| c != *n && d.n(c).is_element()) {
                return true;
            }
        }
    }
    // A node the parser just made has no children: only one a script filled can be an ancestor.
    matches!(child, NodeOrText::AppendNode(n)
        if (d.n(*n).first_child != NIL || *n == parent) && d.is_host_including_inclusive_ancestor(*n, parent))
}

/// Whether a child of `doc` satisfies `pred`.
fn document_has_child(d: &Dom, doc: u32, pred: impl Fn(u32) -> bool) -> bool {
    let mut c = d.n(doc).last_child;
    while c != NIL {
        if pred(c) {
            return true;
        }
        c = d.n(c).prev;
    }
    false
}

fn assign_parser_registry(dom: &mut Dom, node: u32, parent: u32) {
    let Some(el) = dom.n_mut(node).element_mut() else { return };
    if std::mem::take(&mut el.parser_needs_registry) {
        let registry = dom.registry_idx(parent);
        dom.set_registry_idx(node, registry);
    }
}

fn insert_node(dom: &mut Dom, parent: u32, before: u32, child: NodeOrText<u32>) {
    match child {
        NodeOrText::AppendText(t) => append_text(dom, parent, before, &t),
        NodeOrText::AppendNode(n) => {
            assign_parser_registry(dom, n, parent);
            if dom.n(n).parent != NIL {
                dom.unlink_raw(n);
            }
            let owner = dom.n(parent).owner_doc;
            dom.set_owner_raw(n, owner);
            let prev = if before == NIL { dom.n(parent).last_child } else { dom.n(before).prev };
            dom.link_raw(n, parent, before);
            dom.queue_child_list(parent, &[n], &[], prev, before);
            dom.resources_after_parser_insert(n);
            if dom.internals.faces != 0 {
                dom.faces_after_tree_change(n);
            }
            if dom.input_seen {
                dom.radios_after_insert(&[n], dom.is_connected_idx(parent));
            }
            if dom.selectedcontent_seen {
                dom.selectedcontent_parsed(n);
            }
            // Elements of defined names were constructed before their
            // insertion (`Sink::settle_custom`): no upgrade candidates here.
            // Tested inline: no custom element, no call.
            if dom.reactions.any_custom && dom.is_connected_idx(parent) {
                dom.enqueue_custom_reactions(n, crate::custom::CustomReaction::Connected);
            }
            // A constructor may have attached a shadow root and filled it
            // with unupgraded candidates (a clone of a template's contents):
            // DOM "insert" tries to upgrade the shadow-including descendants
            // once the host is connected. They run after the host's
            // connectedCallback (queued just above), in shadow-including
            // tree order.
            // (`upgrades_after_insert` returns at once until a custom element was ever defined.)
            if dom.n(n).element().is_some_and(|e| e.shadow_root != NIL) && dom.is_connected_idx(parent) {
                dom.upgrades_after_insert(n);
            }
            // Nonce hiding applies to what the parser inserts as to what a script does (after
            // the connected reaction: a custom element's `attributeChangedCallback` for the
            // emptied attribute follows its `connectedCallback`). Returns at once without a
            // header-delivered policy.
            dom.hide_nonces(n);
        }
    }
}

impl<A: DomAccess> TreeSink for Sink<A> {
    type Handle = u32;
    type Output = ();
    type ElemName<'a>
        = ElemNameRef
    where
        Self: 'a;

    fn finish(self) {}
    fn parse_error(&self, _msg: Cow<'static, str>) {}
    fn set_current_line(&self, line_number: u64) {
        self.current_line.set(line_number);
    }
    fn get_document(&self) -> u32 {
        self.doc
    }
    fn elem_name<'a>(&'a self, target: &'a u32) -> ElemNameRef {
        self.dom.with(|d| {
            let name = &d.n(*target).element().expect("element").name;
            ElemNameRef { ns: name.ns.clone(), local: name.local.clone() }
        })
    }
    fn create_element(&self, name: QualName, mut attrs: Vec<Attribute>, flags: ElementFlags) -> u32 {
        if let Some(filter) = &self.filter {
            attrs.retain(|attr| filter.allows_attribute(&name, &attr.name, &attr.value));
        }
        // An element of a defined name keeps its token's attributes for its
        // construction (`Sink::settle_custom`). Decided under the creation's
        // borrow: until a window defines a name, one set test per element.
        let template = flags.template;
        if name.ns == markup5ever::ns!(html) && &*name.local == "selectedcontent" {
            self.selectedcontent.set(true);
            self.dom.with(|d| d.selectedcontent_seen = true);
        }
        let created = self.dom.with(|d| {
            // HTML "create an element for a token": the token's `is` attribute is the is value
            // of an HTML element.
            let is = (name.ns == markup5ever::ns!(html))
                .then(|| attrs.iter().find(|a| a.name.ns == ns!() && &*a.name.local == "is"))
                .flatten()
                .map(|a| &*a.value);
            if self.sync_custom && !template && !self.filter.as_ref().is_some_and(|f| f.disposition(&name) != Disposition::Keep)
                && d.parser_may_construct(self.doc_owner(d), &name, is) {
                return Err((name, attrs));
            }
            Ok(self.new_element(d, name, attrs, template))
        });
        match created {
            Ok(created) => created,
            Err((name, attrs)) => self.new_custom_element(name, attrs, template),
        }
    }
    fn create_comment(&self, text: StrTendril) -> u32 {
        // `<?target data>` in HTML (vendor/html5ever `pi`): the tokenizer marks the bogus comment
        // it read; it is a processing instruction when its target is one, else the comment.
        let text: &str = &text;
        let text = match text.strip_prefix(html5ever::pi::MARK) {
            Some(marked) => {
                if let Some((target, data)) = html5ever::pi::parse(marked) {
                    return self.create_pi(target.into(), data.into());
                }
                marked
            }
            None => text,
        };
        self.dom.with(|d| {
            let owner = self.doc_owner(d);
            d.alloc_raw(NodeKind::Comment(text.to_owned()), owner)
        })
    }
    fn create_pi(&self, target: StrTendril, data: StrTendril) -> u32 {
        self.dom.with(|d| {
            let owner = self.doc_owner(d);
            d.alloc_raw(
                NodeKind::ProcessingInstruction(Box::new(crate::node::PiData {
                    target: (&*target).into(),
                    data: data.to_string(),
                })),
                owner,
            )
        })
    }
    fn append(&self, parent: &u32, child: NodeOrText<u32>) {
        if self.stream_invalid() {
            return;
        }
        self.append_to(*parent, NIL, child);
    }
    fn append_based_on_parent_node(&self, element: &u32, prev_element: &u32, child: NodeOrText<u32>) {
        // A replaced element has no parent: the place it stands for is its position.
        let has_parent = self.dom.with(|d| d.n(*element).parent != NIL) || (self.filter.is_some() && self.forwarded(*element).is_some());
        if has_parent {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }
    fn append_doctype_to_document(&self, name: StrTendril, public_id: StrTendril, system_id: StrTendril) {
        if self.stream_invalid() { return; }
        self.dom.with(|d| {
            let data = crate::node::DoctypeData {
                name: (&*name).into(),
                public_id: (&*public_id).into(),
                system_id: (&*system_id).into(),
            };
            // A Document with a doctype or an element child already takes no doctype.
            if document_has_child(d, self.doc, |c| d.n(c).is_element() || matches!(d.n(c).kind, NodeKind::Doctype(_))) {
                return;
            }
            let owner = self.doc_owner(d);
            let n = d.alloc_raw(NodeKind::Doctype(Box::new(data)), owner);
            d.link_raw(n, self.doc, NIL);
        });
    }
    fn mark_script_already_started(&self, node: &u32) {
        if self.stream_invalid() { return; }
        self.dom.with(|d| {
            if let Some(e) = d.n_mut(*node).element_mut() {
                e.flags |= element_flags::SCRIPT_ALREADY_STARTED;
            }
        });
    }
    fn pop(&self, node: &u32) {
        if self.stream_invalid() {
            return;
        }
        {
            let mut declarative = self.declarative.borrow_mut();
            if declarative.last().is_some_and(|d| d.0 == *node) {
                declarative.pop();
            }
        }
        self.dom.with(|d| d.resources_element_popped(*node));
    }
    fn maybe_clone_an_option_into_selectedcontent(&self, option: &u32) {
        if !self.selectedcontent.get() || self.stream_invalid() {
            return;
        }
        let queued = self.dom.with(|d| {
            d.maybe_clone_option_into_selectedcontent(*option);
            d.custom_reactions_queued()
        });
        if queued {
            self.run_custom_reactions();
        }
    }
    fn get_template_contents(&self, target: &u32) -> u32 {
        if let Some(d) = self.declarative.borrow().iter().rev().find(|d| d.0 == *target) {
            return d.1;
        }
        self.dom.with(|d| d.n(*target).element().map(|e| e.template_contents).unwrap_or(NIL))
    }
    fn same_node(&self, x: &u32, y: &u32) -> bool {
        x == y
    }
    fn set_quirks_mode(&self, mode: H5Quirks) {
        if self.stream_invalid() { return; }
        let q = match mode {
            H5Quirks::Quirks => QuirksMode::Quirks,
            H5Quirks::LimitedQuirks => QuirksMode::LimitedQuirks,
            H5Quirks::NoQuirks => QuirksMode::NoQuirks,
        };
        self.quirks.set(Some(q));
        self.dom.with(|d| {
            if let NodeKind::Document(dd) = &mut d.n_mut(self.doc).kind {
                dd.quirks = q;
            }
        });
    }
    fn append_before_sibling(&self, sibling: &u32, new_node: NodeOrText<u32>) {
        if self.stream_invalid() {
            return;
        }
        if self.filter.is_some() {
            // A replaced sibling (a table the tree builder foster parents around) is the place
            // its children went to: insert there.
            if let Some((parent, before)) = self.forwarded(*sibling) {
                self.append_to(parent, before, new_node);
                return;
            }
        }
        let mut replaced = None;
        match self.filter_child(&new_node) {
            ChildFilter::Keep => {}
            ChildFilter::Drop => return,
            ChildFilter::Replace(id) => replaced = Some(id),
        }
        let parent = self.dom.with(|d| d.n(*sibling).parent);
        if parent == NIL {
            return;
        }
        if let Some(id) = replaced {
            self.attach_replaced(id, parent, *sibling);
            return;
        }
        if let (Some(patches), NodeOrText::AppendNode(node)) = (&self.patches, &new_node) {
            if self.dom.with(|d| patches.borrow_mut().prepare(d, self.doc, parent, *node)) {
                return;
            }
        }
        self.settle_custom(parent, &new_node);
        if self.stream_invalid() {
            return;
        }
        let queued = self.dom.with(|d| {
            // The constructor may have moved the sibling.
            let parent = d.n(*sibling).parent;
            if parent != NIL {
                // HTML "appropriate place for inserting a node": a location inside a template
                // element (a table a script moved into one) is inside its template contents,
                // after their last child (WPT `foster-parenting-into-template-element`).
                let contents = d
                    .n(parent)
                    .element()
                    .filter(|e| e.is_html() && e.name.local == local_name!("template"))
                    .map(|e| e.template_contents)
                    .filter(|&contents| contents != NIL);
                let (target, before) = match contents {
                    Some(contents) => (contents, NIL),
                    None => (parent, *sibling),
                };
                if !cannot_insert(d, target, &new_node) {
                    insert_node(d, target, before, new_node);
                }
            }
            d.custom_reactions_queued() || d.has_upgrade_candidates()
        });
        if queued {
            self.run_custom_reactions();
        }
    }
    fn add_attrs_if_missing(&self, target: &u32, attrs: Vec<Attribute>) {
        if self.stream_invalid() { return; }
        self.dom.with(|d| {
            let mut style_added = false;
            let element_name = self.filter.as_ref().and_then(|_| d.n(*target).element().map(|e| e.name.clone()));
            let mut nonce_added = false;
            for a in attrs {
                if let (Some(filter), Some(name)) = (&self.filter, &element_name) {
                    if !filter.allows_attribute(name, &a.name, &a.value) {
                        continue;
                    }
                }
                let Some(el) = d.n_mut(*target).element_mut() else { return };
                if !el.attrs.iter().any(|x| x.name == a.name) {
                    style_added |= a.name.ns == ns!() && a.name.local == local_name!("style");
                    nonce_added |= a.name.ns == ns!() && a.name.local == local_name!("nonce");
                    d.note_style_attr(*target, &a.name, None);
                    let change = crate::dom::Change::attr(*target, &a.name);
                    let name = a.name.clone();
                    let attr = crate::node::Attr { name: a.name, value: (&*a.value).into() };
                    d.edit_attrs(*target, &name, |el| el.push_attr(attr));
                    d.bump_tree();
                    d.note(change);
                }
            }
            d.note_inline_handlers(*target);
            if style_added {
                d.note_inline_style(*target);
            }
            // A dangling `<body nonce=...` or `<html nonce=...` merges its attributes into the
            // existing element: its nonce is hidden as any other (a CSS selector must not see it).
            if nonce_added {
                d.hide_nonces(*target);
            }
        });
    }
    /// HTML "create an element for the token" step: the element takes the form element pointer's
    /// form as its owner when the intended parent is in the form's tree, even where the form was
    /// popped at once (a `<form>` in `<table>`), so the ancestor walk of `form_owner_idx` misses it.
    fn associate_with_form(&self, target: &u32, form: &u32, nodes: (&u32, Option<&u32>)) {
        if self.stream_invalid() { return; }
        self.dom.with(|d| {
            if d.root_idx(*nodes.0) != d.root_idx(*form) {
                return;
            }
            let form = d.id_of(*form);
            if let Some(e) = d.n_mut(*target).element_mut() {
                e.parser_form = Some(form);
            }
        });
    }
    fn remove_from_parent(&self, target: &u32) {
        if self.stream_invalid() { return; }
        self.dom.with(|d| {
            if d.n(*target).parent != NIL {
                d.unlink_raw(*target);
            }
        });
    }
    fn reparent_children(&self, node: &u32, new_parent: &u32) {
        if self.stream_invalid() { return; }
        self.dom.with(|d| {
            let kids: Vec<u32> = d.child_idx_iter(*node).collect();
            for k in kids {
                d.unlink_raw(k);
                d.link_raw(k, *new_parent, NIL);
            }
        });
    }
    fn allow_declarative_shadow_roots(&self, _intended_parent: &u32) -> bool {
        self.dom.insertion_position().is_some() || self.dom.with(|d| d.allows_declarative_shadow_roots(self.doc))
    }
    fn attach_declarative_shadow(&self, host: &u32, template: &u32, attrs: &[Attribute]) -> bool {
        if self.stream_invalid() { return false; }
        let template_name = self.filter.as_ref().map(|_| self.dom.with(|d| d.n(*template).element().unwrap().name.clone()));
        if let (Some(filter), Some(template_name)) = (&self.filter, &template_name) {
            // A host the filter replaces with its children is not in the tree: its template stays a
            // plain template, in the place the host stands for.
            let disallowed = filter.disposition(template_name) != Disposition::Keep || self.forwarded(*host).is_some();
            if disallowed || !attrs.iter().any(|a| a.name.local == markup5ever::LocalName::from("shadowrootmode") && filter.allows_attribute(template_name, &a.name, &a.value)) {
                return false;
            }
        }
        let mut reference_target = None;
        let mut mode = ShadowRootMode::Open;
        let mut flags = 0;
        for a in attrs {
            if let (Some(filter), Some(template_name)) = (&self.filter, &template_name) {
                if !filter.allows_attribute(template_name, &a.name, &a.value) {
                    continue;
                }
            }
            match &*a.name.local {
                "shadowrootmode" if a.value.eq_ignore_ascii_case("closed") => mode = ShadowRootMode::Closed,
                "shadowrootcustomelementregistry" => flags |= shadow_flags::KEEP_REGISTRY_NULL,
                "shadowrootreferencetarget" => reference_target = Some(markup5ever::LocalName::from(&*a.value)),
                "shadowrootclonable" => flags |= shadow_flags::CLONABLE,
                "shadowrootserializable" => flags |= shadow_flags::SERIALIZABLE,
                "shadowrootdelegatesfocus" => flags |= shadow_flags::DELEGATES_FOCUS,
                "shadowrootslotassignment" if a.value.eq_ignore_ascii_case("manual") => {
                    flags |= shadow_flags::MANUAL_SLOTS
                }
                _ => {}
            }
        }
        let Some(root) = self.dom.with(|d| {
            let sr = d.attach_declarative_shadow(*host, mode, flags)?;
            if let NodeKind::Fragment(f) = &mut d.n_mut(sr).kind {
                f.reference_target = reference_target;
            }
            if flags & shadow_flags::KEEP_REGISTRY_NULL == 0 && d.n(*host).owner_doc == self.doc {
                if let Some(registry) = self.fragment_document_registry {
                    d.set_registry_idx(sr, registry);
                }
            }
            Some(sr)
        }) else { return false };
        self.declarative.borrow_mut().push((*template, root));
        true
    }
    fn is_mathml_annotation_xml_integration_point(&self, handle: &u32) -> bool {
        self.dom.with(|d| {
            d.n(*handle).element().is_some_and(|e| {
                e.name.ns == ns!(mathml)
                    && e.name.local == local_name!("annotation-xml")
                    && e.attr_local(&local_name!("encoding")).is_some_and(|v| {
                        v.eq_ignore_ascii_case("text/html") || v.eq_ignore_ascii_case("application/xhtml+xml")
                    })
            })
        })
    }
}

/// What the filter says about a node the tree builder is about to insert.
enum ChildFilter {
    Keep,
    /// The node and what is parsed under it are dropped.
    Drop,
    /// An element replaced with its children: it is never inserted (`Sink::forwards`).
    Replace(u32),
}

impl<A: DomAccess> Sink<A> {
    /// Where `id`'s children go, when the filter replaces `id` with its children.
    #[inline]
    fn forwarded(&self, id: u32) -> Option<(u32, u32)> {
        self.forwards.borrow().get(&id).copied()
    }

    /// `append` at the resolved place: `parent` and the sibling `before` (NIL: the end).
    fn append_to(&self, mut parent: u32, mut before: u32, child: NodeOrText<u32>) {
        if self.filter.is_some() && (self.forwarded(parent).is_some() || (before != NIL && self.forwarded(before).is_some())) {
            (parent, before) = self.resolve_place(parent, before);
        }
        if let Some(position) = self.dom.insertion_position() {
            if parent == self.doc {
                // The tree builder starts a fragment with a synthetic html root.
                // Keep this handle on its stack, outside the live document.
                if let NodeOrText::AppendNode(root) = child {
                    let previous = position.root.replace(root);
                    debug_assert_eq!(previous, NIL);
                }
                return;
            }
            if parent == position.root.get() {
                parent = position.target;
                before = position.before;
            }
        }
        if self.filter.is_some() && (parent != self.doc || self.filter_document) {
            match self.filter_child(&child) {
                ChildFilter::Keep => {}
                ChildFilter::Drop => return,
                ChildFilter::Replace(id) => {
                    self.attach_replaced(id, parent, before);
                    return;
                }
            }
        }
        if let Some(patches) = &self.patches {
            let mut patches = patches.borrow_mut();
            (parent, before) = self.dom.with(|d| patches.insertion(d, parent, before));
            if let NodeOrText::AppendNode(node) = &child {
                if self.dom.with(|d| patches.prepare(d, self.doc, parent, *node)) {
                    return;
                }
            }
        }
        self.settle_custom(parent, &child);
        if self.stream_invalid() {
            return;
        }
        let queued = self.dom.with(|d| {
            // A constructor or script can move the anchor between tokens.
            // An anchor that is no longer a child of `parent` (script moved it): the end.
            let before = if before != NIL && d.n(before).parent != parent { NIL } else { before };
            if !cannot_insert(d, parent, &child) {
                insert_node(d, parent, before, child);
            }
            d.custom_reactions_queued() || d.has_upgrade_candidates()
        });
        if queued {
            self.run_custom_reactions();
        }
    }
}

impl<A: DomAccess> Sink<A> {
    fn filter_child(&self, child: &NodeOrText<u32>) -> ChildFilter {
        let (Some(filter), NodeOrText::AppendNode(id)) = (&self.filter, child) else { return ChildFilter::Keep };
        self.dom.with(|d| match &d.n(*id).kind {
            NodeKind::Element(el) => match filter.disposition(&el.name) {
                Disposition::Keep => ChildFilter::Keep,
                Disposition::ReplaceWithChildren => ChildFilter::Replace(*id),
                Disposition::Remove => {
                    if self.dom.insertion_position().is_some() {
                        // The tree builder can keep a removed element open across
                        // writes. Its descendants must not be constructed in the
                        // live document's registry or start document resources.
                        let inert = d.inert_doc_for_pub(d.n(*id).owner_doc);
                        d.adopt_raw(*id, inert);
                    }
                    ChildFilter::Drop
                }
            },
            NodeKind::Comment(_) if !filter.comments => {
                d.release_candidate(*id);
                ChildFilter::Drop
            }
            NodeKind::ProcessingInstruction(pi) if !filter.allows_pi(&pi.target) => {
                d.release_candidate(*id);
                ChildFilter::Drop
            }
            _ => ChildFilter::Keep,
        })
    }

    /// Where an element replaced with its children stands, `parent`/`before` followed through the
    /// replaced elements that stand in for them.
    fn resolve_place(&self, mut parent: u32, mut before: u32) -> (u32, u32) {
        let forwards = self.forwards.borrow();
        // Entries chain at most once per replaced element; a cycle (a script cannot make one,
        // the tree builder never nests an element in itself) ends after `len` hops.
        for _ in 0..=forwards.len() {
            match forwards.get(&parent) {
                Some(&place) => (parent, before) = place,
                None => break,
            }
        }
        // A sibling the filter replaced was never inserted: what stands for it is the place it
        // was given, when that is still in `parent`.
        for _ in 0..=forwards.len() {
            match forwards.get(&before) {
                Some(&(p, b)) if before != NIL && p == parent => before = b,
                _ => break,
            }
        }
        (parent, before)
    }

    /// The tree builder inserts the element `replaced`, which the filter replaces with its
    /// children, as a child of `parent` before `before` (NIL: last): it is not inserted. Its
    /// place is recorded for what is inserted under it from now on, and what was inserted under
    /// it while it had no place (the adoption agency fills a clone it has not inserted yet) goes
    /// to that place now, in order.
    fn attach_replaced(&self, replaced: u32, parent: u32, before: u32) {
        let (parent, before) = self.resolve_place(parent, before);
        if parent == replaced {
            return;
        }
        self.forwards.borrow_mut().insert(replaced, (parent, before));
        let queued = self.dom.with(|d| {
            let kids: Vec<u32> = d.child_idx_iter(replaced).collect();
            for kid in kids {
                let child = NodeOrText::AppendNode(kid);
                let before = if before != NIL && d.n(before).parent != parent { NIL } else { before };
                if !cannot_insert(d, parent, &child) {
                    insert_node(d, parent, before, child);
                }
            }
            d.custom_reactions_queued() || d.has_upgrade_candidates()
        });
        if queued {
            self.run_custom_reactions();
        }
    }

    #[inline]
    fn stream_invalid(&self) -> bool {
        self.dom.insertion_position().is_some_and(|p| self.dom.with(|d| p.invalid(d)))
    }

    fn doc_owner(&self, d: &Dom) -> u32 {
        let source = self.dom.insertion_position().map_or(self.doc, |p| p.target);
        d.n(source).owner_doc
    }

    /// Creates a parser element with the token's attributes (HTML "create an
    /// element for a token" for every element the parser does not construct).
    #[inline(always)]
    fn new_element(&self, d: &mut Dom, name: QualName, attrs: Vec<Attribute>, template: bool) -> u32 {
        let owner = self.doc_owner(d);
        d.base_seen |= name.local == local_name!("base");
        let is_details = name.local == local_name!("details") && name.ns == markup5ever::ns!(html);
        let is_style = name.local == local_name!("style") && name.ns == markup5ever::ns!(html);
        match name.local {
            local_name!("audio") | local_name!("video") | local_name!("track") => d.media_seen = true,
            local_name!("canvas") => {
                d.canvas_seen = true;
                d.drawable_canvas_seen |= attrs
                    .iter()
                    .any(|a| a.name.ns == ns!() && &*a.name.local == "content" && a.value.eq_ignore_ascii_case("drawable"));
            }
            local_name!("select") => d.select_seen = true,
            local_name!("dialog") => d.dialog_seen = true,
            local_name!("input") => d.input_seen = true,
            local_name!("animate") if name.ns == markup5ever::ns!(svg) => d.smil_seen = true,
            _ if name.ns == markup5ever::ns!(svg) && matches!(&*name.local, "animateTransform" | "set" | "animateMotion") => d.smil_seen = true,
            _ => {}
        }
        if !d.inline_transitions_seen
            && attrs.iter().any(|a| a.name.local == local_name!("style") && a.value.contains("transition"))
        {
            d.inline_transitions_seen = true;
        }
        let mut el = ElementData::new(name);
        if el.is_html() {
            el.is_value = attrs
                .iter()
                .find(|a| a.name.ns == ns!() && &*a.name.local == "is")
                .map(|a| html5ever::LocalName::from(&*a.value));
        }
        el.parser_needs_registry = true;
        el.set_attrs(
            attrs.into_iter().map(|a| crate::node::Attr { name: a.name, value: (&*a.value).into() }).collect(),
        );
        el.flags = self.elem_flags;
        // HTML: a script the document's parser creates has its force async set
        // to false. A fragment parser's scripts keep it: once connected they
        // run as soon as possible (`createContextualFragment`, as Chrome).
        if self.elem_flags & element_flags::FRAGMENT_PARSED == 0 {
            el.script_force_async = false;
            // The line of the start tag's end, where its inline text starts: the line its classic
            // script is evaluated from (as Chrome numbers inline script lines in the document),
            // and where a CSP violation of its inline `<script>` or `<style>` is located.
            if el.name.local == local_name!("script") || el.name.local == local_name!("style") {
                el.script_start_line = u32::try_from(self.current_line.get()).unwrap_or(u32::MAX);
            }
        }
        if is_style {
            // Until `pop` (`Dom::resources_element_popped`).
            el.flags |= crate::node::element_flags::STYLE_PARSING;
        }
        let i = d.alloc_raw(NodeKind::Element(Box::new(el)), owner);
        if template {
            let inert = d.inert_doc_for_pub(owner);
            let frag = d.alloc_raw(NodeKind::Fragment(FragmentData::new(i, None, 0)), inert);
            d.n_mut(i).element_mut().unwrap().template_contents = frag;
        }
        d.note_inline_handlers(i);
        d.note_inline_style(i);
        if is_details {
            d.details_created(i);
        }
        let id = d.id_of(i);
        d.add_ref(id).expect("new parser element");
        self.pins.borrow_mut().push(id);
        i
    }

    /// An element of a defined name: created without attributes, which wait
    /// with it until the parser inserts it (`Sink::settle_custom`).
    #[inline(never)]
    fn new_custom_element(&self, name: QualName, attrs: Vec<Attribute>, template: bool) -> u32 {
        let is = attrs
            .iter()
            .find(|a| a.name.ns == ns!() && &*a.name.local == "is")
            .map(|a| html5ever::LocalName::from(&*a.value));
        let created = self.dom.with(|d| {
            let created = self.new_element(d, name, Vec::new(), template);
            if let Some(e) = d.n_mut(created).element_mut() {
                e.is_value = is;
            }
            created
        });
        self.pending_custom.borrow_mut().push((created, attrs));
        self.has_pending_custom.set(true);
        created
    }

    /// HTML "create an element for a token" for an element of a defined
    /// name, run as the parser inserts it into `parent`: in the document
    /// (not inert template contents), its definition's constructor runs now
    /// (no DOM borrow is held), then the token's attributes are appended
    /// and their reactions run; elsewhere it just gets its attributes.
    ///
    /// Inlined: until the parser holds such an element back, one test per
    /// insertion.
    #[inline(always)]
    fn settle_custom(&self, parent: u32, child: &NodeOrText<u32>) {
        if let NodeOrText::AppendNode(n) = child {
            if self.has_pending_custom.get() {
                self.settle_pending_custom(parent, n);
            }
        }
    }

    #[inline(never)]
    fn settle_pending_custom(&self, parent: u32, n: &u32) {
        let attrs = {
            let mut pending = self.pending_custom.borrow_mut();
            if pending.is_empty() {
                return;
            }
            let Some(k) = pending.iter().position(|(p, _)| p == n) else { return };
            let attrs = pending.swap_remove(k).1;
            self.has_pending_custom.set(!pending.is_empty());
            attrs
        };
        let n = *n;
        let attrs: Vec<(QualName, String)> = attrs.into_iter().map(|a| (a.name, a.value.to_string())).collect();
        let hooks = self.dom.with(|d| {
            assign_parser_registry(d, n, parent);
            let (name, is) = d.n(n).element().map(|e| (e.name.clone(), e.is_value.clone()))?;
            let doc = self.doc_owner(d);
            (d.n(parent).owner_doc == doc).then(|| d.parser_custom_elements(doc, &name, is.as_ref())).flatten()
        });
        let constructed = match hooks {
            Some(hooks) => {
                let el = self.dom.with(|d| d.id_of(n));
                matches!(hooks.construct(el, &attrs), crate::custom::ParserConstruct::Constructed)
            }
            None => false,
        };
        if !constructed {
            self.dom.with(|d| d.set_parsed_attributes(n, attrs));
        }
    }

    /// The element queue of a parser insertion: the reactions it queued
    /// run before parsing goes on (main document parser only).
    fn run_custom_reactions(&self) {
        if !self.sync_custom {
            return;
        }
        let hooks = self.dom.with(|d| {
            let doc = self.doc_owner(d);
            let hooks = d.parser_reactions_pending(doc).or_else(|| d.parser_custom_work(doc))?;
            matches!(&d.n(doc).kind, NodeKind::Document(data) if data.active).then_some(hooks)
        });
        if let Some(hooks) = hooks {
            hooks.run_reactions();
        }
    }
}

/// Outcome of feeding input to an incremental parser.
#[derive(Debug, PartialEq, Eq)]
pub enum ParseStep {
    /// A `</script>` was processed; run the script, then call `resume`.
    Script(NodeId),
    /// All buffered input was consumed.
    NeedInput,
}

/// Incremental document parser bound to one document in a shared store.
pub struct DocumentParser {
    tok: Tokenizer<TreeBuilder<u32, Sink<DomRef>>>,
    input: BufferQueue,
    dom: DomRef,
    doc: NodeId,
    ended: bool,
    plaintext: bool,
    /// Transport-layer charset label (Content-Type), used by `feed_bytes`.
    transport_charset: Option<&'static encoding_rs::Encoding>,
    /// The encoding when nothing declares one (a same-origin parent
    /// document's), before the UTF-8 default.
    fallback_charset: Option<&'static encoding_rs::Encoding>,
    /// Run the compact encoding detector (chardetng, as Chrome/Firefox do) on an
    /// unlabelled document with non-ASCII bytes, ahead of `fallback_charset`.
    detect_charset: bool,
    decoder: Option<encoding_rs::Decoder>,
    /// The first input chunk has been seen. Only one BOM is ever removed:
    /// by the decoder for bytes, or here once for input that came as text.
    bom_checked: bool,
    decoded: String,
    /// Bytes held back until 1024 bytes (or EOF) are available for prescan.
    sniff: Vec<u8>,
    /// Outer input set aside by `open_insertion_point` (innermost last).
    saved: Vec<BufferQueue>,
    /// Bytes appended to the back of the network input (`feed`,
    /// `push_input` outside an insertion point) since creation: with
    /// `unconsumed_input`, a preload scan tells apart input it has not
    /// seen at the front (written) from input at the back.
    appended: usize,
    /// The least `outer input length - appended` at the times written
    /// input was put in front of the network input (`close_insertion_point`
    /// with input left over), since `take_written_low_mark`: the network
    /// input consumed meanwhile is only measurable then, as written input
    /// is consumed before it.
    written_low_mark: Option<isize>,
    /// An XML document: the response is validated whole at end of input,
    /// then its tree is built up to each script in turn (`xml::Load`).
    xml: Option<XmlLoad>,
}

/// State of an XML (XHTML, SVG, generic XML) document load.
struct XmlLoad {
    content_type: String,
    /// The response so far, until the end of input.
    bytes: Vec<u8>,
    /// The tree builder, from the end of input on.
    load: Option<crate::xml::Load>,
}

/// Whether a navigation response of this MIME type is an XML document.
pub fn is_xml_mime(mime: &str) -> bool {
    let essence = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    matches!(essence.as_str(), "text/xml" | "application/xml" | "application/xhtml+xml" | "image/svg+xml")
        || essence.ends_with("+xml")
}

/// HTML "prescan a byte stream to determine its encoding" (§13.2.3.2) over
/// the first 1024 bytes, then "get an XML encoding" for input that starts
/// with an XML declaration (`<?xml ... encoding="..."`), as browsers do: comments and other markup are skipped, `<meta>` attributes are read
/// with the "get an attribute" algorithm, and `content` counts only with
/// `http-equiv="content-type"`.
pub fn prescan_charset(head: &[u8]) -> Option<&'static encoding_rs::Encoding> {
    // An XML declaration's `<?x` in UTF-16 (BOM-less) names that encoding before any `<meta>`
    // (Chromium; WPT html/syntax/xmldecl/xmldecl-2.html utf-16le-and-meta.htm).
    if head.starts_with(b"<\0?\0x\0") {
        return Some(encoding_rs::UTF_16LE);
    }
    if head.starts_with(b"\0<\0?\0x") {
        return Some(encoding_rs::UTF_16BE);
    }
    match meta_charset(head, true) {
        MetaScan::Found(enc) => Some(enc),
        _ => xml_encoding(head),
    }
}

/// What [`meta_charset`] found in the bytes so far.
enum MetaScan {
    Found(&'static encoding_rs::Encoding),
    /// No `<meta>` declares an encoding, whatever bytes come next.
    None,
    /// More bytes could still bring one.
    More,
}

/// The elements a `<head>` keeps: any other start tag ends the head for the scan.
const HEAD_TAGS: [&[u8]; 10] =
    [b"html", b"head", b"base", b"link", b"meta", b"object", b"script", b"noscript", b"style", b"title"];
/// Elements whose contents are text, where a `<meta>` is no element.
const RAW_TEXT_TAGS: [&[u8]; 9] =
    [b"script", b"style", b"title", b"textarea", b"xmp", b"iframe", b"noembed", b"noframes", b"noscript"];
/// The most bytes the scan waits for.
const MAX_META_SCAN: usize = 64 * 1024;

/// The `<meta>` part of the prescan, as Chromium scans so that a `<meta>` the tree builder would
/// act on ("change the encoding") is found before decoding starts: the contents of raw text
/// elements (`script`, `style`, `title`, ...) are skipped, the scan goes past the first 1024
/// bytes while the head lasts (it stops at the first token after them once a `</head>` or a
/// start tag a head does not hold was seen), and character references in the label are decoded
/// (WPT html/syntax/charset). `eof`: `b` is the whole input, so nothing is [`MetaScan::More`].
fn meta_charset(b: &[u8], eof: bool) -> MetaScan {
    let more = if eof { MetaScan::None } else { MetaScan::More };
    let mut in_head = true;
    let fix = |enc: &'static encoding_rs::Encoding| {
        if enc == encoding_rs::UTF_16LE || enc == encoding_rs::UTF_16BE {
            encoding_rs::UTF_8
        } else if enc == encoding_rs::X_USER_DEFINED {
            encoding_rs::WINDOWS_1252
        } else {
            enc
        }
    };
    let starts_ci = |at: usize, s: &[u8]| b.len() >= at + s.len() && b[at..at + s.len()].eq_ignore_ascii_case(s);
    let mut pos = 0;
    while pos < b.len() {
        if b[pos] == b'<' && ((!in_head && pos >= 1024) || pos >= MAX_META_SCAN) {
            return MetaScan::None;
        }
        if b[pos..].starts_with(b"<!--") {
            // The `-->` may share the dashes of `<!--`.
            match find(&b[pos + 2..], b"-->") {
                Some(e) => pos = pos + 2 + e + 3,
                None => return more,
            }
            continue;
        }
        if starts_ci(pos, b"<meta") && b.get(pos + 5).is_some_and(|&c| matches!(c, b'\t' | b'\n' | b'\x0c' | b'\r' | b' ' | b'/')) {
            pos += 6;
            let mut names: Vec<Vec<u8>> = Vec::new();
            let (mut got_pragma, mut need_pragma, mut charset) = (false, None::<bool>, None);
            while let Some((name, value)) = get_attribute(b, &mut pos) {
                let value = decode_char_refs(&value);
                if names.contains(&name) {
                    continue;
                }
                match name.as_slice() {
                    b"http-equiv" if &*value == b"content-type" => got_pragma = true,
                    b"content" if charset.is_none() => {
                        if let Some(enc) = charset_from_content(&value) {
                            charset = Some(enc);
                            need_pragma = Some(true);
                        }
                    }
                    b"charset" => {
                        charset = encoding_rs::Encoding::for_label(trim_ascii(&value));
                        need_pragma = Some(false);
                    }
                    _ => {}
                }
                names.push(name);
            }
            if pos >= b.len() && !eof {
                // The tag may hold more attributes.
                return MetaScan::More;
            }
            match (need_pragma, charset) {
                (None, _) | (_, None) => {}
                (Some(true), _) if !got_pragma => {}
                (_, Some(enc)) => return MetaScan::Found(fix(enc)),
            }
            continue;
        }
        let tag_start = |at: usize| b.get(at).is_some_and(|c| c.is_ascii_alphabetic());
        if b[pos] == b'<' && (tag_start(pos + 1) || (b.get(pos + 1) == Some(&b'/') && tag_start(pos + 2))) {
            // A tag: its name, then its attributes.
            let end_tag = b[pos + 1] == b'/';
            let name_start = if end_tag { pos + 2 } else { pos + 1 };
            pos = name_start;
            while pos < b.len() && !matches!(b[pos], b'\t' | b'\n' | b'\x0c' | b'\r' | b' ' | b'>' | b'/') {
                pos += 1;
            }
            let name = b[name_start..pos].to_ascii_lowercase();
            while get_attribute(b, &mut pos).is_some() {}
            if pos >= b.len() {
                return more;
            }
            if end_tag {
                if name == b"head" {
                    in_head = false;
                }
                continue;
            }
            if !HEAD_TAGS.contains(&name.as_slice()) {
                in_head = false;
            }
            if name == b"plaintext" {
                return MetaScan::None;
            }
            if RAW_TEXT_TAGS.contains(&name.as_slice()) {
                // Its text runs to its end tag, which the scan reads next.
                let mut close = b"</".to_vec();
                close.extend_from_slice(&name);
                match (pos..b.len()).find(|&at| starts_ci(at, &close)) {
                    Some(at) => pos = at,
                    None => return more,
                }
            }
            continue;
        }
        if b[pos..].starts_with(b"<!") || b[pos..].starts_with(b"</") || b[pos..].starts_with(b"<?") {
            match b[pos..].iter().position(|&c| c == b'>') {
                Some(e) => pos += e + 1,
                None => return more,
            }
            continue;
        }
        pos += 1;
    }
    more
}

/// The numeric character references (`&#119;`, `&#x77;`) and the named ones a label can hold,
/// decoded in a lowercased attribute value, as the tree builder decodes them.
fn decode_char_refs(value: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if !value.contains(&b'&') {
        return std::borrow::Cow::Borrowed(value);
    }
    let mut out = Vec::with_capacity(value.len());
    let mut i = 0;
    while i < value.len() {
        if value[i] == b'&' {
            let rest = &value[i + 1..];
            let decoded = rest.iter().position(|&c| c == b';').and_then(|end| {
                let body = std::str::from_utf8(&rest[..end]).ok()?;
                let code = match body.strip_prefix('#') {
                    Some(hex) if hex.starts_with('x') => u32::from_str_radix(&hex[1..], 16).ok()?,
                    Some(dec) => dec.parse().ok()?,
                    None => match body {
                        "amp" => 0x26,
                        "lt" => 0x3C,
                        "gt" => 0x3E,
                        "quot" => 0x22,
                        "apos" => 0x27,
                        _ => return None,
                    },
                };
                Some((char::from_u32(code)?, end))
            });
            if let Some((c, end)) = decoded {
                let mut buf = [0; 4];
                out.extend_from_slice(c.to_ascii_lowercase().encode_utf8(&mut buf).as_bytes());
                i += end + 2;
                continue;
            }
        }
        out.push(value[i]);
        i += 1;
    }
    std::borrow::Cow::Owned(out)
}

fn trim_ascii(v: &[u8]) -> &[u8] {
    let start = v.iter().position(|c| !c.is_ascii_whitespace()).unwrap_or(v.len());
    let end = v.iter().rposition(|c| !c.is_ascii_whitespace()).map_or(start, |e| e + 1);
    &v[start..end]
}

/// HTML "get an attribute" (prescan): `(name, value)` lowercased, or `None`
/// at a `>` or the end. Leaves `pos` after the attribute.
fn get_attribute(b: &[u8], pos: &mut usize) -> Option<(Vec<u8>, Vec<u8>)> {
    let ws = |c: u8| matches!(c, b'\t' | b'\n' | b'\x0c' | b'\r' | b' ');
    while *pos < b.len() && (ws(b[*pos]) || b[*pos] == b'/') {
        *pos += 1;
    }
    if *pos >= b.len() || b[*pos] == b'>' {
        return None;
    }
    let (mut name, mut value) = (Vec::new(), Vec::new());
    // Name.
    loop {
        let c = *b.get(*pos)?;
        if c == b'=' && !name.is_empty() {
            *pos += 1;
            break;
        }
        if ws(c) {
            while *pos < b.len() && ws(b[*pos]) {
                *pos += 1;
            }
            if b.get(*pos) != Some(&b'=') {
                return Some((name, value));
            }
            *pos += 1;
            break;
        }
        if c == b'/' || c == b'>' {
            return Some((name, value));
        }
        name.push(c.to_ascii_lowercase());
        *pos += 1;
    }
    // Value.
    while *pos < b.len() && ws(b[*pos]) {
        *pos += 1;
    }
    let c = *b.get(*pos)?;
    if c == b'"' || c == b'\'' {
        *pos += 1;
        loop {
            let d = *b.get(*pos)?;
            *pos += 1;
            if d == c {
                return Some((name, value));
            }
            value.push(d.to_ascii_lowercase());
        }
    }
    if c == b'>' {
        return Some((name, value));
    }
    loop {
        let Some(&d) = b.get(*pos) else { return Some((name, value)) };
        if ws(d) || d == b'>' {
            return Some((name, value));
        }
        value.push(d.to_ascii_lowercase());
        *pos += 1;
    }
}

/// HTML "extract a character encoding from a meta element" (a `content`
/// value such as `text/html; charset=windows-1251`).
fn charset_from_content(v: &[u8]) -> Option<&'static encoding_rs::Encoding> {
    let mut pos = 0;
    loop {
        let at = find(&v[pos..], b"charset")?;
        pos += at + 7;
        let mut p = pos;
        while p < v.len() && v[p].is_ascii_whitespace() {
            p += 1;
        }
        if v.get(p) != Some(&b'=') {
            continue;
        }
        p += 1;
        while p < v.len() && v[p].is_ascii_whitespace() {
            p += 1;
        }
        let rest = &v[p..];
        let label = match rest.first() {
            Some(&q @ (b'"' | b'\'')) => {
                let end = rest[1..].iter().position(|&c| c == q)?;
                &rest[1..1 + end]
            }
            Some(_) => &rest[..rest.iter().position(|&c| c.is_ascii_whitespace() || c == b';').unwrap_or(rest.len())],
            None => return None,
        };
        return encoding_rs::Encoding::for_label(label);
    }
}

/// HTML "get an XML encoding": the `encoding` of an XML declaration the
/// input starts with (before its first `>`); UTF-16 labels mean UTF-8.
fn xml_encoding(b: &[u8]) -> Option<&'static encoding_rs::Encoding> {
    if !b.starts_with(b"<?xml") {
        return None;
    }
    let end = b.iter().position(|&c| c == b'>')?;
    let mut pos = find(&b[..end], b"encoding")? + 8;
    while pos < end && b[pos] <= 0x20 {
        pos += 1;
    }
    if b.get(pos) != Some(&b'=') {
        return None;
    }
    pos += 1;
    while pos < end && b[pos] <= 0x20 {
        pos += 1;
    }
    let quote = *b.get(pos).filter(|&&c| c == b'"' || c == b'\'')?;
    pos += 1;
    let close = b[pos..end].iter().position(|&c| c == quote)?;
    let label = &b[pos..pos + close];
    // The label is taken as is: white space around it makes it no label (Chromium; WPT
    // html/syntax/xmldecl/xmldecl-2.html space-around-label.htm).
    if label.first().is_some_and(|&c| c <= 0x20) || label.last().is_some_and(|&c| c <= 0x20) {
        return None;
    }
    let enc = encoding_rs::Encoding::for_label(label)?;
    Some(if enc == encoding_rs::UTF_16LE || enc == encoding_rs::UTF_16BE { encoding_rs::UTF_8 } else { enc })
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

impl DocumentParser {
    pub fn new(dom: DomRef, doc: NodeId, scripting: bool) -> DomResult<Self> {
        Self::new_with(dom, doc, scripting, false)
    }

    /// [`DocumentParser::new`]; `no_doctype_no_quirks`: a missing DOCTYPE leaves the document in
    /// no-quirks mode (html5ever's iframe srcdoc rule), for a synthesized document.
    fn new_with(dom: DomRef, doc: NodeId, scripting: bool, no_doctype_no_quirks: bool) -> DomResult<Self> {
        let d = dom.borrow().check(doc)?;
        let sink = Sink::new(dom.clone(), d, element_flags::PARSER_INSERTED, scripting);
        let tb_opts =
            TreeBuilderOpts { scripting_enabled: scripting, iframe_srcdoc: no_doctype_no_quirks, ..Default::default() };
        let tb = TreeBuilder::new(sink, tb_opts);
        // html5ever would drop a U+FEFF at the head of every feed; `feed` removes the one BOM instead.
        let tok = Tokenizer::new(tb, TokenizerOpts { discard_bom: false, ..Default::default() });
        Ok(DocumentParser {
            tok,
            input: BufferQueue::default(),
            dom,
            doc,
            ended: false,
            plaintext: false,
            transport_charset: None,
            fallback_charset: None,
            detect_charset: false,
            decoder: None,
            bom_checked: false,
            decoded: String::new(),
            sniff: Vec::new(),
            saved: Vec::new(),
            appended: 0,
            written_low_mark: None,
            xml: None,
        })
    }

    /// Declarative partial updates (WICG): a parsed `<template for=name>`
    /// patches its contents into the document in place of what lies between
    /// `<?start name=name>` and `<?end>`, or at `<?marker name=name>`, instead
    /// of being inserted. The document's own parser does this (the loader's
    /// and `document.open()`'s); script-created templates never patch.
    pub fn enable_template_patches(&mut self) {
        enable_inline_patches(&mut self.tok.sink, true);
        if let Some(patches) = &self.tok.sink.sink.patches {
            patches.borrow_mut().live = true;
        }
    }

    /// An XML document (`is_xml_mime`): validated whole once the response
    /// has arrived, then built up to each XHTML script's end tag in turn
    /// (`ParseStep::Script`, as for HTML), so content after a script is not
    /// in the document until the script has run.
    pub fn new_xml(dom: DomRef, doc: NodeId, content_type: &str) -> DomResult<Self> {
        let mut parser = Self::new(dom, doc, false)?;
        parser.xml = Some(XmlLoad { content_type: content_type.to_owned(), bytes: Vec::new(), load: None });
        Ok(parser)
    }

    /// A text/plain document uses the normal decoder, but never interprets
    /// markup or character references from the response as HTML.
    pub fn new_plaintext(dom: DomRef, doc: NodeId) -> DomResult<Self> {
        Self::new_text_document(dom, doc, "text/plain", None)
    }

    /// JSON exchanged between systems uses UTF-8 irrespective of the parent
    /// encoding or a Content-Type charset parameter (RFC 8259 §§8.1, 11).
    pub fn new_json(dom: DomRef, doc: NodeId, mime: &str) -> DomResult<Self> {
        Self::new_text_document(dom, doc, mime, Some(encoding_rs::UTF_8))
    }

    fn new_text_document(
        dom: DomRef, doc: NodeId, mime: &str, encoding: Option<&'static encoding_rs::Encoding>,
    ) -> DomResult<Self> {
        // No doctype (`document.doctype` is null), and still no-quirks mode, as Chromium's text
        // documents.
        let mut parser = Self::new_with(dom.clone(), doc, false, true)?;
        parser.plaintext = true;
        // The synthetic LF consumes HTML's <pre> first-LF rule; response bytes
        // keep their own leading newline when the tokenizer changes state.
        parser.feed("<html><head></head><body><pre style=\"white-space: pre-wrap; word-wrap: break-word\">\n");
        parser.tok.set_plaintext_state();
        let mut dom = dom.borrow_mut();
        let index = dom.check(doc)?;
        if let NodeKind::Document(data) = &mut dom.n_mut(index).kind {
            data.content_type = mime.into();
            if let Some(encoding) = encoding {
                data.charset = encoding.name();
                parser.decoder = Some(encoding.new_decoder_with_bom_removal());
            }
        }
        Ok(parser)
    }

    /// Sets the encoding used when neither the transport nor the prescan
    /// declares one (HTML: a same-origin container document's encoding).
    /// UTF-16 is not inherited.
    pub fn set_fallback_charset(&mut self, label: Option<&str>) {
        self.fallback_charset = label
            .and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()))
            .filter(|e| *e != encoding_rs::UTF_16LE && *e != encoding_rs::UTF_16BE);
    }

    /// Enables encoding detection for documents that declare no encoding
    /// (no BOM, transport charset or `<meta>`), before the fallback charset.
    pub fn set_detect_charset(&mut self, on: bool) {
        self.detect_charset = on;
    }

    /// Sets the Content-Type charset label for `feed_bytes`.
    pub fn set_transport_charset(&mut self, label: Option<&str>) {
        self.transport_charset = label.and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()));
    }

    /// Feeds raw network bytes. Encoding: BOM, then transport charset, then
    /// `<meta>` prescan of the first chunk, then UTF-8 (the spec's
    /// locale-dependent default is replaced by UTF-8 — documented deviation).
    pub fn feed_bytes(&mut self, bytes: &[u8]) -> ParseStep {
        if let Some(xml) = &mut self.xml {
            if xml.load.is_none() {
                xml.bytes.extend_from_slice(bytes);
            }
            return ParseStep::NeedInput;
        }
        if self.decoder.is_some() {
            return self.decode(bytes, false);
        }
        self.sniff.extend_from_slice(bytes);
        // A transport charset cannot win until a partial BOM is ruled out.
        if [b"\xef\xbb\xbf".as_slice(), b"\xff\xfe", b"\xfe\xff"]
            .iter()
            .any(|bom| self.sniff.len() < bom.len() && bom.starts_with(&self.sniff))
        {
            return ParseStep::NeedInput;
        }
        let decided = encoding_rs::Encoding::for_bom(&self.sniff).is_some() || self.transport_charset.is_some();
        // The `<meta>` scan waits for its decision (at least the first 1024 bytes, more while
        // the head lasts).
        let waiting = if self.plaintext {
            self.sniff.len() < 1024
        } else {
            matches!(meta_charset(&self.sniff, false), MetaScan::More) && self.sniff.len() < MAX_META_SCAN
        };
        // Detection looks at more than the meta window: keep buffering while the bytes are all ASCII
        // and the prescan found no charset (a declared one wins over detection: nothing to wait for).
        let detect_waiting = self.detect_charset
            && !self.plaintext
            && self.sniff.len() < 4096
            && self.sniff.is_ascii()
            && prescan_charset(&self.sniff).is_none();
        if !decided && (waiting || detect_waiting) {
            return ParseStep::NeedInput;
        }
        self.start_decoder();
        let buf = std::mem::take(&mut self.sniff);
        self.decode(&buf, false)
    }

    fn start_decoder(&mut self) {
        let enc = encoding_rs::Encoding::for_bom(&self.sniff)
            .map(|(e, _)| e)
            .or(self.transport_charset)
            .or_else(|| if self.plaintext { None } else { prescan_charset(&self.sniff) })
            .or_else(|| {
                if !self.detect_charset || self.plaintext || self.sniff.is_ascii() {
                    return None;
                }
                let mut det = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
                det.feed(&self.sniff, true);
                Some(det.guess(None, chardetng::Utf8Detection::Allow))
            })
            .or(self.fallback_charset)
            .unwrap_or(encoding_rs::UTF_8);
        self.decoder = Some(enc.new_decoder_with_bom_removal());
        if let Ok(mut d) = self.dom.try_borrow_mut() {
            if let Ok(i) = d.check(self.doc) {
                if let NodeKind::Document(dd) = &mut d.n_mut(i).kind {
                    dd.charset = enc.name();
                }
            }
        }
    }

    /// Flushes buffered bytes and the decoder at end of input (call before
    /// `end`; resume on `Script` as usual).
    pub fn finish_bytes(&mut self) -> ParseStep {
        if let Some(xml) = &mut self.xml {
            if xml.load.is_none() && !self.ended {
                let bytes = std::mem::take(&mut xml.bytes);
                let charset = self.transport_charset.map(|e| e.name());
                let load =
                    crate::xml::Load::new(&mut self.dom.borrow_mut(), self.doc, &bytes, charset, &xml.content_type);
                xml.load = Some(load);
            }
            return self.resume();
        }
        if self.decoder.is_none() {
            self.start_decoder();
        }
        let buf = std::mem::take(&mut self.sniff);
        self.decode(&buf, true)
    }

    fn decode(&mut self, bytes: &[u8], last: bool) -> ParseStep {
        let dec = self.decoder.as_mut().unwrap();
        let mut out = std::mem::take(&mut self.decoded);
        out.clear();
        out.reserve(dec.max_utf8_buffer_length(bytes.len()).unwrap_or(bytes.len() * 3));
        let _ = dec.decode_to_string(bytes, &mut out, last);
        let step = self.feed(&out);
        self.decoded = out;
        step
    }

    pub fn document(&self) -> NodeId {
        self.doc
    }

    /// Appends decoded input and runs the tokenizer until it needs more input
    /// or a script must execute.
    pub fn feed(&mut self, chunk: &str) -> ParseStep {
        if self.ended {
            return ParseStep::NeedInput;
        }
        if !chunk.is_empty() {
            if self.saved.is_empty() {
                self.appended += chunk.len();
            }
            let mut chunk = chunk;
            if !self.bom_checked {
                self.bom_checked = true;
                if self.decoder.is_none() && self.saved.is_empty() {
                    chunk = chunk.strip_prefix('\u{feff}').unwrap_or(chunk);
                }
            }
            if !chunk.is_empty() {
                self.input.push_back(StrTendril::from_slice(chunk));
            }
        }
        self.resume()
    }

    /// Bytes ever appended to the back of the network input (not the
    /// markup scripts wrote, which goes in front of what remains).
    pub fn appended_input(&self) -> usize {
        self.appended
    }

    /// Whether a script's insertion point is open (its writes are being
    /// parsed; the network input is set aside).
    pub fn in_insertion_point(&self) -> bool {
        !self.saved.is_empty()
    }

    /// What the input held behind written markup, as the least
    /// `length - appended_input()` seen each time a script's written input
    /// was left in front of it since the last call (`None`: no such time).
    /// A preload scan adds the appended count of its previous scan to tell
    /// how much of what it scanned then is still unconsumed: the parser
    /// consumes written input first, so only at those times is the input
    /// behind it all network input.
    pub fn take_written_low_mark(&mut self) -> Option<isize> {
        self.written_low_mark.take()
    }

    /// The input the tokenizer has not consumed, front to back, as `f` sees
    /// it; `None` inside an insertion point (a script is writing). Nothing
    /// is consumed. The total length is the first value.
    pub fn unconsumed_input<R>(&self, f: impl FnOnce(usize, &[&str]) -> R) -> Option<R> {
        if !self.saved.is_empty() || self.xml.is_some() {
            return None;
        }
        let mut chunks = Vec::new();
        while let Some(chunk) = self.input.pop_front() {
            chunks.push(chunk);
        }
        let views: Vec<&str> = chunks.iter().map(|c| &**c).collect();
        let total = views.iter().map(|c| c.len()).sum();
        let r = f(total, &views);
        drop(views);
        for chunk in chunks.into_iter().rev() {
            self.input.push_front(chunk);
        }
        Some(r)
    }

    /// A script this parser yielded for is about to run: its
    /// `document.write`s go to an insertion point ahead of the remaining
    /// input, which is set aside until `close_insertion_point`.
    pub fn open_insertion_point(&mut self) {
        let nested = BufferQueue::default();
        self.input.swap_with(&nested);
        self.saved.push(nested);
    }

    /// The script finished: restores the input set aside by the matching
    /// `open_insertion_point`; written input the tokenizer did not consume
    /// (a written blocking script stopped it) stays in front of it.
    pub fn close_insertion_point(&mut self) {
        let Some(outer) = self.saved.pop() else { return };
        let mut rest = Vec::new();
        while let Some(chunk) = self.input.pop_front() {
            rest.push(chunk);
        }
        self.input.swap_with(&outer);
        if self.saved.is_empty() && !rest.is_empty() {
            let mut outer_len = 0;
            let mut chunks = Vec::new();
            while let Some(chunk) = self.input.pop_front() {
                outer_len += chunk.len();
                chunks.push(chunk);
            }
            for chunk in chunks.into_iter().rev() {
                self.input.push_front(chunk);
            }
            let mark = outer_len as isize - self.appended as isize;
            self.written_low_mark = Some(self.written_low_mark.map_or(mark, |m| m.min(mark)));
        }
        for chunk in rest.into_iter().rev() {
            self.input.push_front(chunk);
        }
    }

    /// Queues input without tokenizing: a written blocking script is pending.
    pub fn push_input(&mut self, chunk: &str) {
        if !self.ended && !chunk.is_empty() {
            if self.saved.is_empty() {
                self.appended += chunk.len();
            }
            self.input.push_back(StrTendril::from_slice(chunk));
        }
    }

    /// Continues after a script ran.
    pub fn resume(&mut self) -> ParseStep {
        if self.ended {
            return ParseStep::NeedInput;
        }
        if let Some(xml) = &mut self.xml {
            let Some(load) = &mut xml.load else { return ParseStep::NeedInput };
            let script = load.step(&self.dom, &self.tok.sink.sink.pins);
            // What the insertions left for the document's custom elements
            // runs before the script does (no DOM borrow is held here).
            let hooks = self.dom.borrow().parser_custom_work(self.doc.index());
            if let Some(hooks) = hooks {
                hooks.run_reactions();
            }
            return script.map_or(ParseStep::NeedInput, ParseStep::Script);
        }
        loop {
            match self.tok.feed(&self.input) {
                TokenizerResult::Script(h) => return ParseStep::Script(self.dom.borrow().id_of(h)),
                // A late <meta charset> after decoding started: the prescan
                // already chose the encoding; keep parsing (no reparse).
                TokenizerResult::EncodingIndicator(_) => continue,
                TokenizerResult::Done => return ParseStep::NeedInput,
            }
        }
    }

    /// Signals end of input. Any remaining `</script>` yields are reported
    /// by `resume` first; call `end` once `feed`/`resume` return `NeedInput`.
    pub fn end(&mut self) {
        if !self.ended {
            self.ended = true;
            // The HTML tokenizer of an XML document was fed nothing: its end
            // of file would append `<html><head><body>` to the document and
            // put it in quirks mode.
            if self.xml.is_none() {
                self.tok.end();
            }
            self.tok.sink.sink.release_pins();
        }
    }
}

/// Parses a complete HTML document (no script execution).
pub fn parse_document(dom: &DomRef, url: &str, html: &str) -> NodeId {
    parse_document_with(dom, url, html, InertDocumentMode::Document, None)
}

/// `Document.parseHTMLUnsafe`: as `parse_document`, with declarative shadow roots.
pub fn parse_html_unsafe(dom: &DomRef, html: &str) -> NodeId {
    parse_document_with(dom, "about:blank", html, InertDocumentMode::DeclarativeShadowRoots, None)
}

/// `Document.parseHTMLUnsafe(html, {sanitizer})` and `Document.parseHTML(html, {sanitizer})`: a
/// new document parsed with declarative shadow roots, every node (the `html`, `head` and `body`
/// elements, and the attributes merged onto them from later start tags, included) filtered by
/// `filter` while it is built. `None` is [`parse_html_unsafe`]. The caller applies the safe
/// methods' baseline (`sanitizer_defaults::remove_unsafe`) to the filter before it calls.
pub fn parse_html_unsafe_with(dom: &DomRef, html: &str, filter: Option<Rc<FragmentFilter>>) -> NodeId {
    parse_document_with(dom, "about:blank", html, InertDocumentMode::DeclarativeShadowRoots, filter)
}

/// `Document.parseHTML`: [`parse_html_unsafe_with`] (the safe method differs only in the filter its
/// caller builds; templates and their `shadowrootmode` are the filter's business).
pub fn parse_html_with(dom: &DomRef, html: &str, filter: Rc<FragmentFilter>) -> NodeId {
    parse_html_unsafe_with(dom, html, Some(filter))
}

/// The text/html branch of DOMParser.parseFromString parses a fresh HTML
/// document with scripting disabled and no sanitizer or declarative shadow roots.
pub fn parse_domparser_document(dom: &DomRef, url: &str, html: &str) -> NodeId {
    parse_document_with(dom, url, html, InertDocumentMode::DomParser, None)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InertDocumentMode {
    Document,
    DomParser,
    DeclarativeShadowRoots,
}

fn parse_document_with(dom: &DomRef, url: &str, html: &str, mode: InertDocumentMode, filter: Option<Rc<FragmentFilter>>) -> NodeId {
    let doc = {
        let mut d = dom.borrow_mut();
        let doc = d.create_document(crate::node::DocumentKind::Html, url);
        d.set_allow_declarative_shadow_roots(doc, mode == InertDocumentMode::DeclarativeShadowRoots).expect("fresh document");
        if mode == InertDocumentMode::DomParser && html.as_bytes().windows(8).any(|w| w.eq_ignore_ascii_case(b"<details")) {
            d.queue_details_toggles_of_parsed_document(doc);
        }
        doc
    };
    let mut p = DocumentParser::new(dom.clone(), doc, false).expect("fresh document");
    if filter.is_some() {
        p.tok.sink.sink.filter = filter;
        p.tok.sink.sink.filter_document = true;
        install_formatting_filter(&mut p.tok.sink);
    }
    if mode == InertDocumentMode::DomParser && html.contains("<?") {
        enable_inline_patches(&mut p.tok.sink, true);
    }
    // Scripts are not executed here; html5ever still yields after each
    // </script>, so keep resuming until the input is drained.
    let mut step = p.feed(html);
    while let ParseStep::Script(script) = step {
        if mode == InertDocumentMode::DomParser {
            // HTML's script preparation marks connected, nonempty scripts
            // of a recognized type started before checking scripting enabled.
            // A deep import copies this flag. Empty/data scripts remain reusable.
            let mut d = dom.borrow_mut();
            let info = d.script_info(script).expect("parser yielded script");
            let supported = info.kind != crate::script::ScriptKind::Data
                || d.element(script).unwrap().attr("type").is_some_and(|t| t.eq_ignore_ascii_case("speculationrules"));
            if supported && (info.src.is_some() || !info.inline_text.is_empty()) && d.is_connected(script).unwrap() {
                d.mark_script_started(script).unwrap();
            } else {
                d.clear_parser_inserted(script).unwrap();
            }
        }
        step = p.resume();
    }
    p.end();
    doc
}

/// Parses response bytes into a new HTML document (no script execution), with
/// the usual encoding sniffing after the transport `charset`.
pub fn parse_document_bytes(dom: &DomRef, url: &str, bytes: &[u8], charset: Option<&str>) -> NodeId {
    let doc = dom.borrow_mut().create_document(crate::node::DocumentKind::Html, url);
    let mut p = DocumentParser::new(dom.clone(), doc, false).expect("fresh document");
    p.set_transport_charset(charset);
    let mut step = p.feed_bytes(bytes);
    while let ParseStep::Script(_) = step {
        step = p.resume();
    }
    step = p.finish_bytes();
    while let ParseStep::Script(_) = step {
        step = p.resume();
    }
    p.end();
    doc
}

/// HTML fragment parsing algorithm (innerHTML / insertAdjacentHTML /
/// createContextualFragment). Returns a new DocumentFragment owned by the
/// context element's node document.
pub fn parse_fragment(dom: &mut Dom, context: NodeId, html: &str) -> DomResult<NodeId> {
    parse_fragment_with(dom, context, context, html, FragmentMode::Element, false, None)
}

/// The legacy innerHTML and outerHTML setters parse into detached output,
/// without a subsequent sanitization step or script activation.
pub fn parse_setter_fragment(dom: &mut Dom, context: NodeId, html: &str) -> DomResult<NodeId> {
    parse_setter_fragment_with(dom, context, html, false, None)
}

/// [`parse_setter_fragment`] with the options a Trusted Types default policy's `createParserOptions`
/// can give the legacy setters: `run_scripts` leaves the scripts of the output not "already
/// started" (they run once the fragment is inserted into a document), `filter` is a sanitizer.
pub fn parse_setter_fragment_with(dom: &mut Dom, context: NodeId, html: &str, run_scripts: bool, filter: Option<Rc<FragmentFilter>>) -> DomResult<NodeId> {
    parse_fragment_with(dom, context, context, html, FragmentMode::Setter, run_scripts, filter)
}

/// A shadow root supplies the registry even though its host supplies the
/// HTML tree builder's context element.
pub fn parse_setter_fragment_in(dom: &mut Dom, context: NodeId, registry_source: NodeId, html: &str) -> DomResult<NodeId> {
    parse_setter_fragment_in_with(dom, context, registry_source, html, false, None)
}

/// [`parse_setter_fragment_in`] with [`parse_setter_fragment_with`]'s options.
pub fn parse_setter_fragment_in_with(
    dom: &mut Dom,
    context: NodeId,
    registry_source: NodeId,
    html: &str,
    run_scripts: bool,
    filter: Option<Rc<FragmentFilter>>,
) -> DomResult<NodeId> {
    parse_fragment_with(dom, context, registry_source, html, FragmentMode::Setter, run_scripts, filter)
}

/// Fragment insertion APIs use an HTML body context when there is no context
/// element or when the context is an HTML html element in an HTML document.
pub fn parse_insertion_fragment(dom: &mut Dom, context: NodeId, html: &str) -> DomResult<NodeId> {
    parse_insertion_fragment_with(dom, context, html, false, None)
}

/// [`parse_insertion_fragment`] with [`parse_setter_fragment_with`]'s options.
pub fn parse_insertion_fragment_with(dom: &mut Dom, context: NodeId, html: &str, run_scripts: bool, filter: Option<Rc<FragmentFilter>>) -> DomResult<NodeId> {
    parse_fragment_with(dom, context, context, html, FragmentMode::Insertion, run_scripts, filter)
}

/// `Range.createContextualFragment` (DOM Parsing): [`parse_fragment`], then
/// every script of the fragment is unmarked "already started" (and as a
/// fragment parser's), so it runs once the fragment is inserted into a
/// document, unlike `innerHTML`'s.
pub fn parse_contextual_fragment(dom: &mut Dom, context: NodeId, html: &str) -> DomResult<NodeId> {
    parse_contextual_fragment_with(dom, context, html, true, None)
}

/// [`parse_contextual_fragment`] with the options a Trusted Types default policy's
/// `createParserOptions` can give it: without `run_scripts` the scripts stay "already started"
/// and never run; `filter` is a sanitizer.
pub fn parse_contextual_fragment_with(dom: &mut Dom, context: NodeId, html: &str, run_scripts: bool, filter: Option<Rc<FragmentFilter>>) -> DomResult<NodeId> {
    let frag = parse_fragment_with(dom, context, context, html, FragmentMode::Range, false, filter)?;
    if !run_scripts {
        return Ok(frag);
    }
    let scripts: Vec<u32> = dom
        .descendants(frag)?
        .filter_map(|id| {
            let i = dom.check(id).ok()?;
            dom.n(i).element().is_some_and(|e| e.name.local == local_name!("script")).then_some(i)
        })
        .collect();
    for i in scripts {
        if let Some(e) = dom.n_mut(i).element_mut() {
            e.flags &= !(element_flags::SCRIPT_ALREADY_STARTED | element_flags::FRAGMENT_PARSED);
        }
    }
    Ok(frag)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FragmentMode {
    Element,
    Setter,
    Insertion,
    Range,
    Modern { run_scripts: bool },
}

fn enable_inline_patches<A: DomAccess>(tree: &mut TreeBuilder<u32, Sink<A>>, document_scope: bool) {
    let mut patches = crate::template_patch::Patches::default();
    patches.document_scope = document_scope;
    tree.sink.patches = Some(RefCell::new(patches));
    tree.close_patch = Some(|sink, template| {
        sink.dom.with(|d| sink.patches.as_ref().unwrap().borrow_mut().close(d, *template));
    });
}

fn install_formatting_filter<A: DomAccess>(tree: &mut TreeBuilder<u32, Sink<A>>) {
    // Only dropped elements: one the filter replaces with its children takes part in the adoption
    // agency like any other (its children's place follows it, `Sink::attach_replaced`).
    if tree.sink.filter.as_ref().is_some_and(|filter| filter.elements.is_some() || !filter.remove_elements.is_empty()) {
        tree.removed_formatting_element = Some(|sink, id| {
            sink.dom.with(|d| sink.filter.as_ref().unwrap().disposition(&d.n(*id).element().unwrap().name) == Disposition::Remove)
        });
    }
}

fn fragment_tree_options(dom: &Dom, owner: NodeId) -> DomResult<html5ever::driver::ParseOpts> {
    let doc = dom.document_data(owner)?;
    let quirks_mode = match doc.quirks {
        QuirksMode::Quirks => H5Quirks::Quirks,
        QuirksMode::LimitedQuirks => H5Quirks::LimitedQuirks,
        QuirksMode::NoQuirks => H5Quirks::NoQuirks,
    };
    Ok(html5ever::driver::ParseOpts {
        tree_builder: TreeBuilderOpts { scripting_enabled: doc.scripting, quirks_mode, ..Default::default() },
        ..Default::default()
    })
}

fn fragment_form(dom: &Dom, mut ancestor: u32) -> Option<u32> {
    while ancestor != NIL {
        let node = dom.n(ancestor);
        if node.element().is_some_and(|e| e.is_html() && e.name.local == local_name!("form")) {
            return Some(ancestor);
        }
        ancestor = node.parent;
    }
    None
}

fn parse_fragment_with(
    dom: &mut Dom,
    context: NodeId,
    registry_source: NodeId,
    html: &str,
    mode: FragmentMode,
    // The scripts of the output are left to run when it is inserted (a script end tag clears their
    // "already started" flag; an unclosed one stays inert): `Modern { run_scripts: true }`, and the
    // legacy setters when a Trusted Types default policy's `createParserOptions` says so.
    scripts: bool,
    filter: Option<Rc<FragmentFilter>>,
) -> DomResult<NodeId> {
    let ctx = dom.check(context)?;
    let owner = dom.n(ctx).owner_doc;
    let owner_id = dom.id_of(owner);
    let insertion_context = matches!(mode, FragmentMode::Insertion | FragmentMode::Range);
    let body_context = insertion_context && dom.n(ctx).element().is_none_or(|el| {
        el.is_html() && el.name.local == local_name!("html") && !dom.is_xml_document(owner_id)
    });
    if insertion_context && dom.is_xml_document(owner_id) {
        return crate::xml::parse_fragment(dom, context, html, body_context);
    }
    if !body_context && dom.n(ctx).element().is_none() {
        return Err(DomError::Type("fragment context must be an element"));
    }
    let mut opts = fragment_tree_options(dom, owner_id)?;
    // A template's contents belong to its inert document, which has no browsing context:
    // parsed with scripting disabled (`<noscript>` contents are markup), as Chromium does.
    if dom.n(ctx).element().is_some_and(|e| e.template_contents != NIL) {
        opts.tree_builder.scripting_enabled = false;
    }
    // Temporary document for the parser; nodes are adopted into `owner`.
    let tmp_id = dom.create_document(crate::node::DocumentKind::Html, "about:blank");
    dom.set_allow_declarative_shadow_roots(tmp_id, matches!(mode, FragmentMode::Modern { .. }))?;
    let tmp = tmp_id.idx;
    let registry = if dom.n(ctx).element().is_some_and(|e| e.template_contents != NIL) {
        RegistryAssociation::Null
    } else {
        dom.registry_association(registry_source)?
    };
    // The parser's synthetic root receives this association. For the common
    // global case this also avoids storing a registry on each temporary node.
    if registry == RegistryAssociation::Default {
        if let NodeKind::Document(d) = &mut dom.n_mut(tmp).kind { d.has_global_registry = true; }
    }
    dom.set_registry_idx(tmp, registry);
    dom.make_fragment_parse_host(tmp);
    let flags = element_flags::FRAGMENT_PARSED | element_flags::SCRIPT_ALREADY_STARTED;

    // The fragment context is inspected by the tree builder but is never on
    // the stack of open elements. Its namespace, integration-point attributes
    // and nearest inclusive ancestor form all come from the caller's tree.
    let context = if body_context {
        dom.create_element_raw(tmp_id, QualName::new(None, ns!(html), local_name!("body")))?.idx
    } else {
        ctx
    };
    let form = fragment_form(dom, context);
    {
        // The detached parser cannot see markers outside its output. Without
        // a PI opener in the input there can be no target, so keep the ordinary
        // parser path for fragments without processing instructions.
        let patches_enabled = matches!(
            mode,
            FragmentMode::Setter | FragmentMode::Insertion | FragmentMode::Range | FragmentMode::Modern { run_scripts: false }
        ) && filter.is_none()
            && !scripts
            && html.contains("<?")
            && !dom.is_xml_document(owner_id);
        let document_registry = dom.registry_idx(owner);
        let mut sink = Sink::new(Borrowed(RefCell::new(&mut *dom)), tmp, flags, false);
        sink.fragment_document_registry = Some(document_registry);
        sink.filter = filter;
        let scripting = opts.tree_builder.scripting_enabled;
        let mut parser = html5ever::driver::parse_fragment_for_element(sink, opts, context, scripting, form);
        install_formatting_filter(&mut parser.tokenizer.sink);
        if patches_enabled {
            enable_inline_patches(&mut parser.tokenizer.sink, false);
        }
        use html5ever::tendril::TendrilSink;
        if scripts {
            // A script end tag yields its node. Keep the EOF parser flag on
            // unclosed scripts, which must stay inert after insertion.
            parser.input_buffer.push_back(StrTendril::from_slice(html));
            loop {
                match parser.tokenizer.feed(&parser.input_buffer) {
                    TokenizerResult::Script(script) => parser.tokenizer.sink.sink.dom.with(|d| {
                        if let Some(e) = d.n_mut(script).element_mut() {
                            e.flags &= !(element_flags::SCRIPT_ALREADY_STARTED | element_flags::FRAGMENT_PARSED);
                        }
                    }),
                    TokenizerResult::EncodingIndicator(_) => continue,
                    TokenizerResult::Done => break,
                }
            }
            parser.finish();
        } else {
            parser.one(StrTendril::from_slice(html));
        }
    }
    let root_html = dom.n(tmp).first_child;
    let frag = dom.create_fragment(owner_id)?;
    if root_html != NIL {
        let kids: Vec<u32> = dom.child_idx_iter(root_html).collect();
        for k in kids {
            dom.unlink_raw(k);
            dom.adopt_raw(k, owner);
            dom.link_raw(k, frag.idx, NIL);
        }
    }
    dom.upgrades_after_fragment_parse(frag.idx);
    // Only the synthetic fallback context belongs to this parse.
    dom.release_candidate(tmp);
    if body_context {
        dom.release_candidate(context);
    }
    Ok(frag)
}

/// `Element.innerHTML` setter.
pub fn set_inner_html(dom: &mut Dom, element: NodeId, html: &str) -> DomResult<()> {
    set_inner_html_with(dom, element, html, false, None)
}

/// [`set_inner_html`] with [`parse_setter_fragment_with`]'s options.
pub fn set_inner_html_with(dom: &mut Dom, element: NodeId, html: &str, run_scripts: bool, filter: Option<Rc<FragmentFilter>>) -> DomResult<()> {
    let i = dom.check(element)?;
    // For <template>, replace the template contents instead.
    let target = match dom.n(i).element() {
        Some(e) if e.template_contents != NIL => dom.id_of(e.template_contents),
        _ => element,
    };
    let ctx = match &dom.n(i).kind {
        NodeKind::Element(e) => {
            if parses_as_text(e, html) {
                // What the fragment parser would build: one text node with
                // the string (none for ""), replacing all children.
                let text = if html.is_empty() {
                    None
                } else {
                    let owner = dom.id_of(dom.n(i).owner_doc);
                    Some(dom.create_text(owner, html)?)
                };
                return dom.replace_all(target, text);
            }
            element
        }
        _ => return Err(DomError::Type("innerHTML context must be an element")),
    };
    let frag = parse_setter_fragment_with(dom, ctx, html, run_scripts, filter)?;
    dom.replace_all(target, Some(frag))
}

/// Whether the HTML fragment parsing algorithm, run on `html` in the
/// context of `ctx`, yields exactly one text node holding `html` (or nothing
/// for ""): the input has no markup (`<`), no character reference (`&`) and
/// nothing the input stream or tree builder rewrites (CR, NUL, a leading
/// byte order mark, which html5ever's tokenizer drops), and the context puts
/// the tokenizer in its data state and the tree builder "in body" (an HTML
/// element that is no table part, `select`, `template`, `html`, `head`,
/// `frameset` or a raw-text/RCDATA/plaintext element).
fn parses_as_text(ctx: &ElementData, html: &str) -> bool {
    if ctx.name.ns != ns!(html)
        || html.starts_with('\u{feff}')
        || html.bytes().any(|b| matches!(b, b'<' | b'&' | b'\r' | b'\0'))
    {
        return false;
    }
    !matches!(
        ctx.name.local,
        local_name!("table")
            | local_name!("tbody")
            | local_name!("thead")
            | local_name!("tfoot")
            | local_name!("tr")
            | local_name!("td")
            | local_name!("th")
            | local_name!("caption")
            | local_name!("colgroup")
            | local_name!("col")
            | local_name!("select")
            | local_name!("template")
            | local_name!("html")
            | local_name!("head")
            | local_name!("frameset")
            | local_name!("frame")
            | local_name!("title")
            | local_name!("textarea")
            | local_name!("style")
            | local_name!("xmp")
            | local_name!("iframe")
            | local_name!("noembed")
            | local_name!("noframes")
            | local_name!("noscript")
            | local_name!("script")
            | local_name!("plaintext")
    )
}

/// Parse modern HTML insertion methods in the target or host element's context.
pub fn parse_modern_html_fragment(dom: &mut Dom, context: NodeId, html: &str, run_scripts: bool) -> DomResult<NodeId> {
    parse_modern_html_fragment_with(dom, context, html, run_scripts, None)
}

pub fn parse_modern_html_fragment_with(dom: &mut Dom, context: NodeId, html: &str, run_scripts: bool, filter: Option<Rc<FragmentFilter>>) -> DomResult<NodeId> {
    parse_fragment_with(dom, context, context, html, FragmentMode::Modern { run_scripts }, run_scripts, filter)
}

/// `Element.setHTMLUnsafe` / `ShadowRoot.setHTMLUnsafe`: as `innerHTML`, with
/// declarative shadow roots. `target` is the context element, or a shadow
/// root (parsed in its host's context).
pub fn set_html_unsafe(dom: &mut Dom, target: NodeId, html: &str) -> DomResult<()> {
    set_html_unsafe_with(dom, target, html, false, None)
}

pub fn set_html_unsafe_with(dom: &mut Dom, target: NodeId, html: &str, run_scripts: bool, filter: Option<Rc<FragmentFilter>>) -> DomResult<()> {
    let i = dom.check(target)?;
    let (ctx, into) = match &dom.n(i).kind {
        NodeKind::Fragment(f) if f.shadow.is_some() => (dom.id_of(f.host), target),
        NodeKind::Element(e) if e.template_contents != NIL => (target, dom.id_of(e.template_contents)),
        NodeKind::Element(_) => (target, target),
        _ => return Err(DomError::Type("setHTMLUnsafe context must be an element")),
    };
    let frag = parse_fragment_with(dom, ctx, target, html, FragmentMode::Modern { run_scripts }, run_scripts, filter)?;
    dom.replace_all(into, Some(frag))
}
