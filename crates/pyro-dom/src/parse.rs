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
use crate::fragment_filter::FragmentFilter;
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
    /// Open `<template shadowrootmode>` elements and the shadow roots their
    /// contents go to. Such a template stays out of the tree and keeps its
    /// own fragment, so collecting it never takes the host's root along.
    declarative: RefCell<Vec<(u32, u32)>>,
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
            pending_custom: RefCell::new(Vec::new()),
            has_pending_custom: Cell::new(false),
            quirks: Cell::new(None),
            pins: RefCell::new(vec![id]),
            declarative: RefCell::new(Vec::new()),
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
    // A node the parser just made has no children: only one a script filled can be an ancestor.
    matches!(child, NodeOrText::AppendNode(n)
        if (d.n(*n).first_child != NIL || *n == parent) && d.is_host_including_inclusive_ancestor(*n, parent))
}

fn insert_node(dom: &mut Dom, parent: u32, before: u32, child: NodeOrText<u32>) {
    match child {
        NodeOrText::AppendText(t) => append_text(dom, parent, before, &t),
        NodeOrText::AppendNode(n) => {
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
            // Elements of defined names were constructed before their
            // insertion (`Sink::settle_custom`): no upgrade candidates here.
            // Tested inline: no custom element, no call.
            if dom.reactions.any_custom && dom.is_connected_idx(parent) {
                dom.enqueue_custom_reactions(n, crate::custom::CustomReaction::Connected);
            }
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
            attrs.retain(|attr| filter.allows_attribute(&attr.name));
        }
        // An element of a defined name keeps its token's attributes for its
        // construction (`Sink::settle_custom`). Decided under the creation's
        // borrow: until a window defines a name, one set test per element.
        let template = flags.template;
        let created = self.dom.with(|d| {
            // HTML "create an element for a token": the token's `is` attribute is the is value
            // of an HTML element.
            let is = (name.ns == markup5ever::ns!(html))
                .then(|| attrs.iter().find(|a| a.name.ns == ns!() && &*a.name.local == "is"))
                .flatten()
                .map(|a| &*a.value);
            if self.sync_custom && !template && !self.filter.as_ref().is_some_and(|f| f.removes_element(&name))
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
        let mut parent = *parent;
        let mut before = NIL;
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
        if parent != self.doc && self.filter_child(&child) { return; }
        self.settle_custom(parent, &child);
        if self.stream_invalid() {
            return;
        }
        let queued = self.dom.with(|d| {
            // A constructor or script can move the anchor between tokens.
            if (before == NIL || d.n(before).parent == parent) && !cannot_insert(d, parent, &child) {
                insert_node(d, parent, before, child);
            }
            d.custom_reactions_queued()
        });
        if queued {
            self.run_custom_reactions();
        }
    }
    fn append_based_on_parent_node(&self, element: &u32, prev_element: &u32, child: NodeOrText<u32>) {
        let has_parent = self.dom.with(|d| d.n(*element).parent != NIL);
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
        if self.stream_invalid() || self.filter_child(&new_node) {
            return;
        }
        let parent = self.dom.with(|d| d.n(*sibling).parent);
        if parent == NIL {
            return;
        }
        self.settle_custom(parent, &new_node);
        if self.stream_invalid() {
            return;
        }
        let queued = self.dom.with(|d| {
            // The constructor may have moved the sibling.
            let parent = d.n(*sibling).parent;
            if parent != NIL && !cannot_insert(d, parent, &new_node) {
                insert_node(d, parent, *sibling, new_node);
            }
            d.custom_reactions_queued()
        });
        if queued {
            self.run_custom_reactions();
        }
    }
    fn add_attrs_if_missing(&self, target: &u32, attrs: Vec<Attribute>) {
        if self.stream_invalid() { return; }
        self.dom.with(|d| {
            for a in attrs {
                if self.filter.as_ref().is_some_and(|filter| !filter.allows_attribute(&a.name)) {
                    continue;
                }
                let Some(el) = d.n_mut(*target).element_mut() else { return };
                if !el.attrs.iter().any(|x| x.name == a.name) {
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
        });
    }
    fn associate_with_form(&self, _target: &u32, _form: &u32, _nodes: (&u32, Option<&u32>)) {}
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
        if let Some(filter) = &self.filter {
            let disallowed = self.dom.with(|d| filter.removes_element(&d.n(*template).element().unwrap().name));
            if disallowed || !attrs.iter().any(|a| a.name.local == markup5ever::LocalName::from("shadowrootmode") && filter.allows_attribute(&a.name)) {
                return false;
            }
        }
        let mut mode = ShadowRootMode::Open;
        let mut flags = 0;
        for a in attrs {
            if self.filter.as_ref().is_some_and(|f| !f.allows_attribute(&a.name)) {
                continue;
            }
            match &*a.name.local {
                "shadowrootmode" if a.value.eq_ignore_ascii_case("closed") => mode = ShadowRootMode::Closed,
                "shadowrootclonable" => flags |= shadow_flags::CLONABLE,
                "shadowrootserializable" => flags |= shadow_flags::SERIALIZABLE,
                "shadowrootdelegatesfocus" => flags |= shadow_flags::DELEGATES_FOCUS,
                "shadowrootslotassignment" if a.value.eq_ignore_ascii_case("manual") => {
                    flags |= shadow_flags::MANUAL_SLOTS
                }
                _ => {}
            }
        }
        let Some(root) = self.dom.with(|d| d.attach_declarative_shadow(*host, mode, flags)) else { return false };
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

impl<A: DomAccess> Sink<A> {
    fn filter_child(&self, child: &NodeOrText<u32>) -> bool {
        let (Some(filter), NodeOrText::AppendNode(id)) = (&self.filter, child) else { return false };
        self.dom.with(|d| match &d.n(*id).kind {
            NodeKind::Element(el) if filter.removes_element(&el.name) => {
                if self.dom.insertion_position().is_some() {
                    // The tree builder can keep a removed element open across
                    // writes. Its descendants must not be constructed in the
                    // live document's registry or start document resources.
                    let inert = d.inert_doc_for_pub(d.n(*id).owner_doc);
                    d.adopt_raw(*id, inert);
                }
                true
            }
            NodeKind::Comment(_) if !filter.comments => {
                d.release_candidate(*id);
                true
            }
            NodeKind::ProcessingInstruction(pi) if !filter.allows_pi(&pi.target) => {
                d.release_candidate(*id);
                true
            }
            _ => false,
        })
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
            local_name!("audio") | local_name!("video") => d.media_seen = true,
            local_name!("canvas") => d.canvas_seen = true,
            local_name!("animate") if name.ns == markup5ever::ns!(svg) => d.smil_seen = true,
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
        el.set_attrs(
            attrs.into_iter().map(|a| crate::node::Attr { name: a.name, value: (&*a.value).into() }).collect(),
        );
        el.flags = self.elem_flags;
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
            let hooks = d.parser_reactions_pending(doc)?;
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
    decoder: Option<encoding_rs::Decoder>,
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
    meta_charset(head).or_else(|| xml_encoding(head))
}

/// The `<meta>` part of the prescan.
fn meta_charset(head: &[u8]) -> Option<&'static encoding_rs::Encoding> {
    let b = &head[..head.len().min(1024)];
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
        if b[pos..].starts_with(b"<!--") {
            // The `-->` may share the dashes of `<!--`.
            match find(&b[pos + 2..], b"-->") {
                Some(e) => pos = pos + 2 + e + 3,
                None => return None,
            }
            continue;
        }
        if starts_ci(pos, b"<meta") && b.get(pos + 5).is_some_and(|&c| matches!(c, b'\t' | b'\n' | b'\x0c' | b'\r' | b' ' | b'/')) {
            pos += 6;
            let mut names: Vec<Vec<u8>> = Vec::new();
            let (mut got_pragma, mut need_pragma, mut charset) = (false, None::<bool>, None);
            while let Some((name, value)) = get_attribute(b, &mut pos) {
                if names.contains(&name) {
                    continue;
                }
                match name.as_slice() {
                    b"http-equiv" if value == b"content-type" => got_pragma = true,
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
            match (need_pragma, charset) {
                (None, _) | (_, None) => {}
                (Some(true), _) if !got_pragma => {}
                (_, Some(enc)) => return Some(fix(enc)),
            }
            continue;
        }
        let tag_start = |at: usize| b.get(at).is_some_and(|c| c.is_ascii_alphabetic());
        if b[pos] == b'<' && (tag_start(pos + 1) || (b.get(pos + 1) == Some(&b'/') && tag_start(pos + 2))) {
            // A tag: skip its name, then its attributes.
            while pos < b.len() && !matches!(b[pos], b'\t' | b'\n' | b'\x0c' | b'\r' | b' ' | b'>') {
                pos += 1;
            }
            while get_attribute(b, &mut pos).is_some() {}
            continue;
        }
        if b[pos..].starts_with(b"<!") || b[pos..].starts_with(b"</") || b[pos..].starts_with(b"<?") {
            match b[pos..].iter().position(|&c| c == b'>') {
                Some(e) => pos += e + 1,
                None => return None,
            }
            continue;
        }
        pos += 1;
    }
    None
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
    let enc = encoding_rs::Encoding::for_label(&b[pos..pos + close])?;
    Some(if enc == encoding_rs::UTF_16LE || enc == encoding_rs::UTF_16BE { encoding_rs::UTF_8 } else { enc })
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

impl DocumentParser {
    pub fn new(dom: DomRef, doc: NodeId, scripting: bool) -> DomResult<Self> {
        let d = dom.borrow().check(doc)?;
        let sink = Sink::new(dom.clone(), d, element_flags::PARSER_INSERTED, scripting);
        let tb_opts = TreeBuilderOpts { scripting_enabled: scripting, ..Default::default() };
        let tb = TreeBuilder::new(sink, tb_opts);
        let tok = Tokenizer::new(tb, TokenizerOpts::default());
        Ok(DocumentParser {
            tok,
            input: BufferQueue::default(),
            dom,
            doc,
            ended: false,
            plaintext: false,
            transport_charset: None,
            fallback_charset: None,
            decoder: None,
            decoded: String::new(),
            sniff: Vec::new(),
            saved: Vec::new(),
            appended: 0,
            written_low_mark: None,
            xml: None,
        })
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
        let mut parser = Self::new(dom.clone(), doc, false)?;
        parser.plaintext = true;
        // The synthetic LF consumes HTML's <pre> first-LF rule; response bytes
        // keep their own leading newline when the tokenizer changes state.
        parser.feed(
            "<!doctype html><html><head></head><body><pre style=\"white-space: pre-wrap; word-wrap: break-word\">\n",
        );
        parser.tok.set_plaintext_state();
        let mut dom = dom.borrow_mut();
        let index = dom.check(doc)?;
        if let NodeKind::Document(data) = &mut dom.n_mut(index).kind {
            data.content_type = "text/plain".into();
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
        if !decided && self.sniff.len() < 1024 {
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
            self.input.push_back(StrTendril::from_slice(chunk));
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
            let script = load.step(&mut self.dom.borrow_mut(), &mut self.tok.sink.sink.pins.borrow_mut());
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
    parse_document_with(dom, url, html, false)
}

/// `Document.parseHTMLUnsafe`: as `parse_document`, with declarative shadow roots.
pub fn parse_html_unsafe(dom: &DomRef, html: &str) -> NodeId {
    parse_document_with(dom, "about:blank", html, true)
}

fn parse_document_with(dom: &DomRef, url: &str, html: &str, allow_declarative_shadow_roots: bool) -> NodeId {
    let doc = {
        let mut d = dom.borrow_mut();
        let doc = d.create_document(crate::node::DocumentKind::Html, url);
        d.set_allow_declarative_shadow_roots(doc, allow_declarative_shadow_roots).expect("fresh document");
        doc
    };
    let mut p = DocumentParser::new(dom.clone(), doc, false).expect("fresh document");
    // Scripts are not executed here; html5ever still yields after each
    // </script>, so keep resuming until the input is drained.
    let mut step = p.feed(html);
    while let ParseStep::Script(_) = step {
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
    parse_fragment_with(dom, context, html, FragmentMode::Element, None)
}

/// Fragment insertion APIs use an HTML body context when there is no context
/// element or when the context is an HTML html element in an HTML document.
pub fn parse_insertion_fragment(dom: &mut Dom, context: NodeId, html: &str) -> DomResult<NodeId> {
    parse_fragment_with(dom, context, html, FragmentMode::Insertion, None)
}

/// `Range.createContextualFragment` (DOM Parsing): [`parse_fragment`], then
/// every script of the fragment is unmarked "already started" (and as a
/// fragment parser's), so it runs once the fragment is inserted into a
/// document, unlike `innerHTML`'s.
pub fn parse_contextual_fragment(dom: &mut Dom, context: NodeId, html: &str) -> DomResult<NodeId> {
    let frag = parse_insertion_fragment(dom, context, html)?;
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
    Insertion,
    Modern { run_scripts: bool },
}

fn install_formatting_filter<A: DomAccess>(tree: &mut TreeBuilder<u32, Sink<A>>) {
    if tree.sink.filter.as_ref().is_some_and(|filter| !filter.remove_elements.is_empty()) {
        tree.removed_formatting_element = Some(|sink, id| {
            sink.dom.with(|d| sink.filter.as_ref().unwrap().removes_element(&d.n(*id).element().unwrap().name))
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
    html: &str,
    mode: FragmentMode,
    filter: Option<Rc<FragmentFilter>>,
) -> DomResult<NodeId> {
    let ctx = dom.check(context)?;
    let owner = dom.n(ctx).owner_doc;
    let owner_id = dom.id_of(owner);
    let body_context = mode == FragmentMode::Insertion && dom.n(ctx).element().is_none_or(|el| {
        el.is_html() && el.name.local == local_name!("html") && !dom.is_xml_document(owner_id)
    });
    if mode == FragmentMode::Insertion && dom.is_xml_document(owner_id) {
        return crate::xml::parse_fragment(dom, context, html, body_context);
    }
    if !body_context && dom.n(ctx).element().is_none() {
        return Err(DomError::Type("fragment context must be an element"));
    }
    let opts = fragment_tree_options(dom, owner_id)?;
    // Temporary document for the parser; nodes are adopted into `owner`.
    let tmp_id = dom.create_document(crate::node::DocumentKind::Html, "about:blank");
    dom.set_allow_declarative_shadow_roots(tmp_id, matches!(mode, FragmentMode::Modern { .. }))?;
    let tmp = tmp_id.idx;
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
        let mut sink = Sink::new(Borrowed(RefCell::new(&mut *dom)), tmp, flags, false);
        sink.filter = filter;
        let scripting = opts.tree_builder.scripting_enabled;
        let mut parser = html5ever::driver::parse_fragment_for_element(sink, opts, context, scripting, form);
        install_formatting_filter(&mut parser.tokenizer.sink);
        use html5ever::tendril::TendrilSink;
        if matches!(mode, FragmentMode::Modern { run_scripts: true }) {
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
    // Only the synthetic fallback context belongs to this parse.
    dom.release_candidate(tmp);
    if body_context {
        dom.release_candidate(context);
    }
    Ok(frag)
}

/// `Element.innerHTML` setter.
pub fn set_inner_html(dom: &mut Dom, element: NodeId, html: &str) -> DomResult<()> {
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
    let frag = parse_fragment(dom, ctx, html)?;
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
    parse_fragment_with(dom, context, html, FragmentMode::Modern { run_scripts }, filter)
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
    let frag = parse_modern_html_fragment_with(dom, ctx, html, run_scripts, filter)?;
    dom.replace_all(into, Some(frag))
}
