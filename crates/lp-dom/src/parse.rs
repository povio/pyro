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
    // html5ever keeps raw handles across script pauses, including detached elements.
    pins: RefCell<Vec<NodeId>>,
}

impl<A: DomAccess> Sink<A> {
    fn new(dom: A, doc: u32, elem_flags: u8) -> Self {
        let id = dom.with(|d| {
            let id = d.id_of(doc);
            d.add_ref(id).expect("parser document");
            id
        });
        Sink {
            dom,
            doc,
            elem_flags,
            quirks: Cell::new(None),
            first_created: Cell::new(NIL),
            pins: RefCell::new(vec![id]),
        }
    }

    fn release_pins(&self) {
        let pins = self.pins.take();
        self.dom.with(|d| {
            for id in pins {
                d.release(id);
            }
        });
    }
}

impl<A: DomAccess> Drop for Sink<A> {
    fn drop(&mut self) {
        self.release_pins();
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
            dom.resources_after_insert(n);
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
            let id = d.id_of(i);
            d.add_ref(id).expect("new parser element");
            self.pins.borrow_mut().push(id);
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
    fn pop(&self, node: &u32) {
        self.dom.with(|d| d.resources_element_popped(*node));
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
    /// Transport-layer charset label (Content-Type), used by `feed_bytes`.
    transport_charset: Option<&'static encoding_rs::Encoding>,
    decoder: Option<encoding_rs::Decoder>,
    decoded: String,
    /// Bytes held back until 1024 bytes (or EOF) are available for prescan.
    sniff: Vec<u8>,
}

/// Charset prescan of the first bytes for `<meta charset>` /
/// `<meta http-equiv=content-type content="...charset=...">` (simplified
/// HTML §13.2.3.2).
pub fn prescan_charset(head: &[u8]) -> Option<&'static encoding_rs::Encoding> {
    let head = &head[..head.len().min(1024)];
    let lower: Vec<u8> = head.iter().map(|b| b.to_ascii_lowercase()).collect();
    let mut pos = 0;
    while let Some(off) = find(&lower[pos..], b"<meta") {
        let start = pos + off;
        let end = find(&lower[start..], b">").map_or(lower.len(), |e| start + e);
        let tag = &lower[start..end];
        if let Some(c) = find(tag, b"charset") {
            let mut rest = &tag[c + 7..];
            while let [b' ' | b'\t' | b'\n' | b'\r' | b'=', tail @ ..] = rest {
                rest = tail;
            }
            let rest: &[u8] = match rest.first() {
                Some(b'"') | Some(b'\'') => &rest[1..],
                _ => rest,
            };
            let label_end = rest
                .iter()
                .position(|&b| matches!(b, b'"' | b'\'' | b';' | b' ' | b'/' | b'>' | b'\t' | b'\n'))
                .unwrap_or(rest.len());
            if let Some(enc) = encoding_rs::Encoding::for_label(&rest[..label_end]) {
                // UTF-16 declared in a byte-oriented prescan means UTF-8.
                return Some(if enc == encoding_rs::UTF_16LE || enc == encoding_rs::UTF_16BE {
                    encoding_rs::UTF_8
                } else {
                    enc
                });
            }
        }
        pos = end.max(start + 5);
    }
    None
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

impl DocumentParser {
    pub fn new(dom: DomRef, doc: NodeId, scripting: bool) -> DomResult<Self> {
        let d = dom.borrow().check(doc)?;
        let sink = Sink::new(dom.clone(), d, element_flags::PARSER_INSERTED);
        let tb_opts = TreeBuilderOpts { scripting_enabled: scripting, ..Default::default() };
        let tb = TreeBuilder::new(sink, tb_opts);
        let tok = Tokenizer::new(tb, TokenizerOpts::default());
        Ok(DocumentParser {
            tok,
            input: BufferQueue::default(),
            dom,
            doc,
            ended: false,
            transport_charset: None,
            decoder: None,
            decoded: String::new(),
            sniff: Vec::new(),
        })
    }

    /// Sets the Content-Type charset label for `feed_bytes`.
    pub fn set_transport_charset(&mut self, label: Option<&str>) {
        self.transport_charset = label.and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()));
    }

    /// Feeds raw network bytes. Encoding: BOM, then transport charset, then
    /// `<meta>` prescan of the first chunk, then UTF-8 (the spec's
    /// locale-dependent default is replaced by UTF-8 — documented deviation).
    pub fn feed_bytes(&mut self, bytes: &[u8]) -> ParseStep {
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
            .or_else(|| prescan_charset(&self.sniff))
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
            self.input.push_back(StrTendril::from_slice(chunk));
        }
        self.resume()
    }

    /// Continues after a script ran.
    pub fn resume(&mut self) -> ParseStep {
        if self.ended {
            return ParseStep::NeedInput;
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
            self.tok.end();
            self.tok.sink.sink.release_pins();
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
