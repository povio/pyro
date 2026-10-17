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
use crate::error::{DomError, DomResult};
use crate::node::{element_flags, ElementData, FragmentData, NodeId, NodeKind, QuirksMode, NIL};

/// Shared handle to a page's DOM store.
pub type DomRef = Rc<RefCell<Dom>>;

trait DomAccess {
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
    quirks: Cell<Option<QuirksMode>>,
    /// First element created (the fragment parser's context element).
    first_created: Cell<u32>,
}

impl<A: DomAccess> Sink<A> {
    fn new(dom: A, doc: u32, elem_flags: u8) -> Self {
        Sink { dom, doc, elem_flags, quirks: Cell::new(None), first_created: Cell::new(NIL) }
    }
}

#[derive(Debug)]
pub struct ElemNameRef(QualName);
impl html5ever::tree_builder::ElemName for ElemNameRef {
    fn ns(&self) -> &html5ever::Namespace {
        &self.0.ns
    }
    fn local_name(&self) -> &html5ever::LocalName {
        &self.0.local
    }
}

fn append_text(dom: &mut Dom, parent: u32, before: u32, text: &str) {
    let prev = if before == NIL { dom.n(parent).last_child } else { dom.n(before).prev };
    if prev != NIL {
        if let NodeKind::Text(s) = &mut dom.n_mut(prev).kind {
            s.push_str(text);
            return;
        }
    }
    let owner = dom.n(parent).owner_doc;
    let t = dom.alloc_raw(NodeKind::Text(text.to_owned()), owner);
    dom.link_raw(t, parent, before);
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
            dom.link_raw(n, parent, before);
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
        ElemNameRef(self.dom.with(|d| d.n(*target).element().map(|e| e.name.clone())).expect("element"))
    }
    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> u32 {
        let created = self.dom.with(|d| {
            let owner = self.doc_owner(d);
            let mut el = ElementData::new(name);
            el.attrs =
                attrs.into_iter().map(|a| crate::node::Attr { name: a.name, value: (&*a.value).into() }).collect();
            el.flags = self.elem_flags;
            let i = d.alloc_raw(NodeKind::Element(Box::new(el)), owner);
            if flags.template {
                let inert = d.inert_doc_for_pub(owner);
                let frag = d.alloc_raw(NodeKind::Fragment(FragmentData { host: i, shadow: None }), inert);
                d.n_mut(i).element_mut().unwrap().template_contents = frag;
            }
            i
        });
        if self.first_created.get() == NIL {
            self.first_created.set(created);
        }
        created
    }
    fn create_comment(&self, text: StrTendril) -> u32 {
        self.dom.with(|d| {
            let owner = self.doc_owner(d);
            d.alloc_raw(NodeKind::Comment(text.to_string()), owner)
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
        self.dom.with(|d| insert_node(d, *parent, NIL, child));
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
        self.dom.with(|d| {
            if let Some(e) = d.n_mut(*node).element_mut() {
                e.flags |= element_flags::SCRIPT_ALREADY_STARTED;
            }
        });
    }
    fn get_template_contents(&self, target: &u32) -> u32 {
        self.dom.with(|d| d.n(*target).element().map(|e| e.template_contents).unwrap_or(NIL))
    }
    fn same_node(&self, x: &u32, y: &u32) -> bool {
        x == y
    }
    fn set_quirks_mode(&self, mode: H5Quirks) {
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
        self.dom.with(|d| {
            let parent = d.n(*sibling).parent;
            if parent != NIL {
                insert_node(d, parent, *sibling, new_node);
            }
        });
    }
    fn add_attrs_if_missing(&self, target: &u32, attrs: Vec<Attribute>) {
        self.dom.with(|d| {
            let Some(el) = d.n_mut(*target).element_mut() else { return };
            for a in attrs {
                if !el.attrs.iter().any(|x| x.name == a.name) {
                    el.attrs.push(crate::node::Attr { name: a.name, value: (&*a.value).into() });
                }
            }
        });
    }
    fn associate_with_form(&self, _target: &u32, _form: &u32, _nodes: (&u32, Option<&u32>)) {}
    fn remove_from_parent(&self, target: &u32) {
        self.dom.with(|d| {
            if d.n(*target).parent != NIL {
                d.unlink_raw(*target);
            }
        });
    }
    fn reparent_children(&self, node: &u32, new_parent: &u32) {
        self.dom.with(|d| {
            let kids: Vec<u32> = d.child_idx_iter(*node).collect();
            for k in kids {
                d.unlink_raw(k);
                d.link_raw(k, *new_parent, NIL);
            }
        });
    }
    fn is_mathml_annotation_xml_integration_point(&self, handle: &u32) -> bool {
        self.dom.with(|d| {
            d.n(*handle).element().is_some_and(|e| {
                e.name.ns == ns!(mathml)
                    && e.name.local == local_name!("annotation-xml")
                    && e.attr("encoding").is_some_and(|v| {
                        v.eq_ignore_ascii_case("text/html") || v.eq_ignore_ascii_case("application/xhtml+xml")
                    })
            })
        })
    }
}

impl<A: DomAccess> Sink<A> {
    fn doc_owner(&self, d: &Dom) -> u32 {
        d.n(self.doc).owner_doc
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
}

impl DocumentParser {
    pub fn new(dom: DomRef, doc: NodeId, scripting: bool) -> DomResult<Self> {
        let d = dom.borrow().check(doc)?;
        let sink = Sink::new(dom.clone(), d, element_flags::PARSER_INSERTED);
        let tb_opts = TreeBuilderOpts { scripting_enabled: scripting, ..Default::default() };
        let tb = TreeBuilder::new(sink, tb_opts);
        let tok = Tokenizer::new(tb, TokenizerOpts::default());
        Ok(DocumentParser { tok, input: BufferQueue::default(), dom, doc, ended: false })
    }

    pub fn document(&self) -> NodeId {
        self.doc
    }

    /// Appends decoded input and runs the tokenizer until it needs more input
    /// or a script must execute.
    pub fn feed(&mut self, chunk: &str) -> ParseStep {
        if !chunk.is_empty() {
            self.input.push_back(StrTendril::from_slice(chunk));
        }
        self.resume()
    }

    /// Continues after a script ran.
    pub fn resume(&mut self) -> ParseStep {
        match self.tok.feed(&self.input) {
            TokenizerResult::Script(h) => ParseStep::Script(self.dom.borrow().id_of(h)),
            _ => ParseStep::NeedInput,
        }
    }

    /// Signals end of input. Any remaining `</script>` yields are reported
    /// by `resume` first; call `end` once `feed`/`resume` return `NeedInput`.
    pub fn end(&mut self) {
        if !self.ended {
            self.ended = true;
            self.tok.end();
        }
    }
}

/// Parses a complete HTML document (no script execution).
pub fn parse_document(dom: &DomRef, url: &str, html: &str) -> NodeId {
    let doc = dom.borrow_mut().create_document(crate::node::DocumentKind::Html, url);
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

/// HTML fragment parsing algorithm (innerHTML / insertAdjacentHTML /
/// createContextualFragment). Returns a new DocumentFragment owned by the
/// context element's node document.
pub fn parse_fragment(dom: &mut Dom, context: NodeId, html: &str) -> DomResult<NodeId> {
    let ctx = dom.check(context)?;
    let (ctx_name, ctx_attrs) = match &dom.n(ctx).kind {
        NodeKind::Element(e) => (
            e.name.clone(),
            e.attrs
                .iter()
                .map(|a| Attribute { name: a.name.clone(), value: StrTendril::from_slice(&a.value) })
                .collect::<Vec<_>>(),
        ),
        _ => return Err(DomError::Type("fragment context must be an element")),
    };
    let owner = dom.n(ctx).owner_doc;
    let owner_id = dom.id_of(owner);
    // Temporary document for the parser; nodes are adopted into `owner`.
    let tmp = dom.create_document(crate::node::DocumentKind::Html, "about:blank").idx;
    let flags = element_flags::FRAGMENT_PARSED | element_flags::SCRIPT_ALREADY_STARTED;

    let context_copy;
    {
        let sink = Sink::new(Borrowed(RefCell::new(&mut *dom)), tmp, flags);
        let opts = html5ever::driver::ParseOpts {
            tree_builder: TreeBuilderOpts { scripting_enabled: true, ..Default::default() },
            ..Default::default()
        };
        let parser = html5ever::driver::parse_fragment(sink, opts, ctx_name, ctx_attrs, true);
        context_copy = parser.tokenizer.sink.sink.first_created.get();
        use html5ever::tendril::TendrilSink;
        parser.one(StrTendril::from_slice(html));
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
    // The temp document, its html root and the stray context element
    // created by html5ever are now unreferenced.
    dom.release_candidate(tmp);
    if context_copy != NIL {
        dom.release_candidate(context_copy);
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
    let ctx = if matches!(dom.n(i).kind, NodeKind::Element(_)) {
        element
    } else {
        return Err(DomError::Type("innerHTML context must be an element"));
    };
    let frag = parse_fragment(dom, ctx, html)?;
    dom.replace_all(target, Some(frag))
}
