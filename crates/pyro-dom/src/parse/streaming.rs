//! A live HTML fragment parser. The tree builder retains its synthetic root
//! and open elements between writes. Only insertion into that root is
//! redirected to the supplied DOM position.

use super::*;
use crate::{DomErrorKind, NodeType};

pub(super) struct InsertionPosition {
    pub(super) root: Cell<u32>,
    pub(super) target: u32,
    pub(super) before: u32,
    failed: Cell<bool>,
}

impl InsertionPosition {
    pub(super) fn invalid(&self, dom: &Dom) -> bool {
        if self.before != NIL && dom.n(self.before).parent != self.target {
            self.failed.set(true);
        }
        self.failed.get()
    }
}

struct LiveDom {
    dom: DomRef,
    position: InsertionPosition,
}

impl DomAccess for LiveDom {
    fn insertion_position(&self) -> Option<&InsertionPosition> {
        Some(&self.position)
    }

    fn with<R>(&self, f: impl FnOnce(&mut Dom) -> R) -> R {
        self.dom.with(f)
    }
}

// html5ever drives the tree builder for a whole chunk. A constructor or a
// reaction can invalidate the anchor partway through one token. The sink
// guards the rest of that token; this adapter discards all subsequent tokens
// before they can create nodes, queue resources or run user code.
struct AbortableTree(TreeBuilder<u32, Sink<LiveDom>>);

impl std::ops::Deref for AbortableTree {
    type Target = TreeBuilder<u32, Sink<LiveDom>>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl html5ever::tokenizer::TokenSink for AbortableTree {
    type Handle = u32;
    fn process_token(
        &self,
        token: html5ever::tokenizer::Token,
        line: u64,
    ) -> html5ever::tokenizer::TokenSinkResult<u32> {
        if self.sink.stream_invalid() {
            return html5ever::tokenizer::TokenSinkResult::Continue;
        }
        self.0.process_token(token, line)
    }
    fn end(&self) {
        if !self.sink.stream_invalid() {
            self.0.end();
        }
    }
    fn adjusted_current_node_present_but_not_in_html_namespace(&self) -> bool {
        self.0.adjusted_current_node_present_but_not_in_html_namespace()
    }
}

struct FragmentTokenizer {
    tokenizer: Tokenizer<AbortableTree>,
    input_buffer: BufferQueue,
}

/// A single incremental parse at a fixed insertion position. The owner can
/// yield to script after a completed script token, then resume the same write.
/// Dropping the parser aborts without sending EOF or changing inserted nodes.
pub struct StreamingFragmentParser {
    parser: FragmentTokenizer,
    run_scripts: bool,
    paused: bool,
    closing: bool,
    finished: bool,
}

impl StreamingFragmentParser {
    pub fn new(
        dom: DomRef,
        context: NodeId,
        target: NodeId,
        before: Option<NodeId>,
        run_scripts: bool,
    ) -> DomResult<Self> {
        Self::with_filter(dom, context, target, before, run_scripts, None)
    }

    pub fn with_filter(
        dom: DomRef,
        context: NodeId,
        target: NodeId,
        before: Option<NodeId>,
        run_scripts: bool,
        filter: Option<Rc<FragmentFilter>>,
    ) -> DomResult<Self> {
        let (doc, opts, form, pins) = {
            let mut d = dom.borrow_mut();
            d.element(context)?;
            if !matches!(d.node_type(target)?, NodeType::Element | NodeType::DocumentFragment) {
                return Err(DomError::new(DomErrorKind::HierarchyRequest, "Invalid fragment insertion target"));
            }
            if let Some(child) = before {
                if d.parent(child)? != Some(target) {
                    return Err(Self::displaced());
                }
            }
            let doc = d.node_document(target)?;
            let opts = fragment_tree_options(&d, d.node_document(context)?)?;
            let form = fragment_form(&d, context.idx);
            // Context and form can become detached across writes, as can the
            // target and anchor. Parser handles are indices, so keep every one
            // alive until the sink releases its pins.
            let mut pins = vec![context, target];
            pins.extend(before);
            pins.extend(form.map(|i| d.id_of(i)));
            d.add_refs(&pins)?;
            (doc, opts, form, pins)
        };
        let position = InsertionPosition {
            root: Cell::new(NIL),
            target: target.idx,
            before: before.map_or(NIL, |n| n.idx),
            failed: Cell::new(false),
        };
        let flags = element_flags::FRAGMENT_PARSED | element_flags::SCRIPT_ALREADY_STARTED;
        let mut sink = Sink::new(LiveDom { dom, position }, doc.idx, flags, true);
        sink.filter = filter;
        sink.pins.borrow_mut().extend(pins);
        let scripting = opts.tree_builder.scripting_enabled;
        let mut tree = TreeBuilder::new_for_fragment(sink, context.idx, form, opts.tree_builder);
        install_formatting_filter(&mut tree);
        // `<template for>` patches the markers already in the target (or an earlier chunk's) with
        // the template's content, as the other fragment parsers do (WPT template-for-*).
        if tree.sink.filter.is_none() && !tree.sink.dom.with(|d| d.is_xml_document(doc)) {
            super::enable_inline_patches(&mut tree, false);
            if let Some(patches) = &tree.sink.patches {
                let mut patches = patches.borrow_mut();
                patches.live = true;
                patches.stream_target = Some(target.idx);
            }
        }
        let initial_state = tree.tokenizer_state_for_context_elem(scripting);
        let tokenizer =
            Tokenizer::new(AbortableTree(tree), TokenizerOpts { initial_state: Some(initial_state), ..opts.tokenizer });
        let parser = FragmentTokenizer { tokenizer, input_buffer: BufferQueue::default() };
        Ok(Self { parser, run_scripts: run_scripts && scripting, paused: false, closing: false, finished: false })
    }

    fn displaced() -> DomError {
        DomError::new(DomErrorKind::HierarchyRequest, "The insertion reference is no longer a child of the target")
    }

    pub fn write(&mut self, chunk: &str) -> DomResult<ParseStep> {
        self.check_anchor()?;
        if self.closing {
            return Err(DomError::new(DomErrorKind::InvalidState, "The stream is closing"));
        }
        if !chunk.is_empty() {
            self.parser.input_buffer.push_back(StrTendril::from_slice(chunk));
        }
        if self.paused {
            // The external script owns parser progress. Writes still append
            // after the unconsumed input from the current chunk and resolve.
            return Ok(ParseStep::NeedInput);
        }
        self.resume()
    }

    fn check_anchor(&self) -> DomResult<()> {
        if self.parser.tokenizer.sink.sink.stream_invalid() {
            Err(Self::displaced())
        } else {
            Ok(())
        }
    }

    pub fn resume(&mut self) -> DomResult<ParseStep> {
        self.check_anchor()?;
        self.paused = false;
        loop {
            let step = self.parser.tokenizer.feed(&self.parser.input_buffer);
            self.check_anchor()?;
            match step {
                TokenizerResult::Script(script) if self.run_scripts => {
                    let live = &self.parser.tokenizer.sink.sink.dom;
                    if let Some(script) = live.with(|d| {
                        if !d.is_connected_idx(script) {
                            return None;
                        }
                        d.n_mut(script).element_mut()?.flags &=
                            !(element_flags::SCRIPT_ALREADY_STARTED | element_flags::FRAGMENT_PARSED);
                        Some(d.id_of(script))
                    }) {
                        self.paused = true;
                        self.trim_pins();
                        return Ok(ParseStep::Script(script));
                    }
                }
                TokenizerResult::Script(_) | TokenizerResult::EncodingIndicator(_) => {}
                TokenizerResult::Done => {
                    if self.closing && !self.finished {
                        self.parser.tokenizer.end();
                        self.finished = true;
                        self.check_anchor()?;
                    }
                    self.trim_pins();
                    return Ok(ParseStep::NeedInput);
                }
            }
        }
    }

    /// Queue EOF. A pending parser-blocking script resumes the input queue
    /// before EOF is processed. The caller settles close when this returns
    /// true, or after resume has made is_finished() true.
    pub fn close(&mut self) -> DomResult<bool> {
        self.check_anchor()?;
        self.closing = true;
        if !self.paused && !self.finished {
            self.resume()?;
        }
        Ok(self.finished)
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    fn trim_pins(&self) {
        use html5ever::tree_builder::Tracer;
        use rustc_hash::FxHashSet;
        #[derive(Default)]
        struct Handles(RefCell<FxHashSet<u32>>);
        impl Tracer for Handles {
            type Handle = u32;
            fn trace_handle(&self, handle: &u32) {
                self.0.borrow_mut().insert(*handle);
            }
        }
        let tree = &self.parser.tokenizer.sink;
        let handles = Handles::default();
        tree.trace_handles(&handles);
        let mut live = handles.0.into_inner();
        let sink = &tree.sink;
        live.insert(sink.dom.position.target);
        live.insert(sink.dom.position.before);
        for &(template, root) in sink.declarative.borrow().iter() {
            live.extend([template, root]);
        }
        live.extend(sink.pending_custom.borrow().iter().map(|(id, _)| *id));
        {
            // A replaced element that left the parser's stacks stands for nothing any more: its
            // id may be reused. The places of the others stay alive.
            let mut forwards = sink.forwards.borrow_mut();
            forwards.retain(|id, _| live.contains(id));
            live.extend(forwards.values().flat_map(|&(parent, before)| [parent, before]));
        }
        let mut release = Vec::new();
        sink.pins.borrow_mut().retain(|id| {
            if live.contains(&id.idx) {
                true
            } else {
                release.push(*id);
                false
            }
        });
        sink.dom.with(|d| d.release_parser_pins(release));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (DomRef, NodeId, NodeId) {
        let dom = DomRef::default();
        let doc = parse_document(&dom, "about:blank", "<!doctype html><body><i></i></body>");
        dom.borrow_mut().set_document_scripting(doc, true).unwrap();
        let body = dom.borrow().body(doc).unwrap().unwrap();
        let anchor = dom.borrow().first_child(body).unwrap().unwrap();
        (dom, body, anchor)
    }

    #[test]
    fn writes_expose_partial_tree_and_continue_open_elements() {
        let (dom, body, before) = fixture();
        let mut parser = StreamingFragmentParser::new(dom.clone(), body, body, Some(before), false).unwrap();
        parser.write("<p>one &am").unwrap();
        let p = dom.borrow().first_child(body).unwrap().unwrap();
        assert_eq!(dom.borrow().inner_html(body).unwrap(), "<p>one </p><i></i>");
        parser.write("p; two <b>three</b></p>tail").unwrap();
        parser.close().unwrap();
        assert_eq!(dom.borrow().first_child(body).unwrap(), Some(p));
        assert_eq!(dom.borrow().inner_html(body).unwrap(), "<p>one &amp; two <b>three</b></p>tail<i></i>");
    }

    #[test]
    fn tokenizer_context_and_incomplete_entity_span_writes() {
        let (dom, body, _) = fixture();
        let doc = dom.borrow().node_document(body).unwrap();
        let context = dom.borrow_mut().create_element(doc, "textarea").unwrap();
        let mut parser = StreamingFragmentParser::new(dom.clone(), context, context, None, false).unwrap();
        parser.write("<b>&am").unwrap();
        parser.write("p;</b>").unwrap();
        parser.close().unwrap();
        assert_eq!(dom.borrow().text_content(context).unwrap().as_deref(), Some("<b>&</b>"));
        assert_eq!(dom.borrow().children(context).unwrap().count(), 1);
    }

    #[test]
    fn scripts_disabled_accepts_script_tokens_without_pausing() {
        let (dom, body, before) = fixture();
        let mut parser = StreamingFragmentParser::new(dom.clone(), body, body, Some(before), false).unwrap();
        assert_eq!(parser.write("<script>test</script><b></b>").unwrap(), ParseStep::NeedInput);
        parser.close().unwrap();
        let script = dom.borrow().first_child(body).unwrap().unwrap();
        assert_ne!(dom.borrow().element(script).unwrap().flags & element_flags::SCRIPT_ALREADY_STARTED, 0);
        assert_eq!(dom.borrow().inner_html(body).unwrap(), "<script>test</script><b></b><i></i>");
    }

    #[test]
    fn table_and_foreign_content_survive_chunk_boundaries() {
        let (dom, body, _) = fixture();
        let doc = dom.borrow().node_document(body).unwrap();
        let table = dom.borrow_mut().create_element(doc, "table").unwrap();
        let mut parser = StreamingFragmentParser::new(dom.clone(), table, table, None, false).unwrap();
        parser.write("<tr><td><sv").unwrap();
        parser.write("g><circle/></svg></td></tr>").unwrap();
        parser.close().unwrap();
        assert_eq!(
            dom.borrow().inner_html(table).unwrap(),
            "<tbody><tr><td><svg><circle></circle></svg></td></tr></tbody>"
        );
        let circle =
            dom.borrow().query_selector(table, &crate::select::parse_selectors("circle").unwrap()).unwrap().unwrap();
        assert_eq!(dom.borrow().element(circle).unwrap().namespace(), &ns!(svg));
    }

    #[test]
    fn displaced_anchor_rejects_next_write_and_survives_collection() {
        let (dom, body, before) = fixture();
        let mut parser = StreamingFragmentParser::new(dom.clone(), body, body, Some(before), false).unwrap();
        parser.write("<p>before</p>").unwrap();
        dom.borrow_mut().remove_child(body, before).unwrap();
        dom.borrow_mut().collect_garbage();
        assert!(dom.borrow().is_alive(before));
        assert_eq!(parser.write("after").unwrap_err().kind(), Some(DomErrorKind::HierarchyRequest));
        assert_eq!(dom.borrow().inner_html(body).unwrap(), "<p>before</p>");
        drop(parser);
        dom.borrow_mut().collect_garbage();
        assert!(!dom.borrow().is_alive(before));
    }

    #[test]
    fn script_pause_and_displaced_anchor_error_the_current_write_and_close() {
        let (dom, body, before) = fixture();
        let mut parser = StreamingFragmentParser::new(dom.clone(), body, body, Some(before), true).unwrap();
        let step = parser.write("<script></script><b>after</b>").unwrap();
        assert!(matches!(step, ParseStep::Script(_)));
        assert_eq!(parser.write("queued").unwrap(), ParseStep::NeedInput);
        assert!(!parser.close().unwrap());
        assert_eq!(dom.borrow().inner_html(body).unwrap(), "<script></script><i></i>");
        dom.borrow_mut().remove_child(body, before).unwrap();
        assert_eq!(parser.resume().unwrap_err().kind(), Some(DomErrorKind::HierarchyRequest));
        assert_eq!(parser.close().unwrap_err().kind(), Some(DomErrorKind::HierarchyRequest));
        assert_eq!(dom.borrow().inner_html(body).unwrap(), "<script></script>");
    }

    #[test]
    fn blocked_writes_are_fifo_and_close_waits_for_resume() {
        let (dom, body, anchor) = fixture();
        let mut p = StreamingFragmentParser::new(dom.clone(), body, body, Some(anchor), true).unwrap();
        assert!(matches!(p.write("<script></script><b>one").unwrap(), ParseStep::Script(_)));
        assert_eq!(p.write("two</b><i>three</i>").unwrap(), ParseStep::NeedInput);
        assert!(!p.close().unwrap());
        assert!(!p.is_finished());
        assert_eq!(dom.borrow().inner_html(body).unwrap(), "<script></script><i></i>");
        assert_eq!(p.resume().unwrap(), ParseStep::NeedInput);
        assert!(p.is_finished());
        assert_eq!(dom.borrow().inner_html(body).unwrap(), "<script></script><b>onetwo</b><i>three</i><i></i>");
    }

    #[test]
    fn completed_detached_elements_can_be_collected_before_close() {
        let (dom, body, anchor) = fixture();
        let mut p = StreamingFragmentParser::new(dom.clone(), body, body, Some(anchor), false).unwrap();
        p.write("<div></div><section>").unwrap();
        let completed = dom.borrow().first_child(body).unwrap().unwrap();
        let open = dom.borrow().next_sibling(completed).unwrap().unwrap();
        dom.borrow_mut().remove_child(body, completed).unwrap();
        dom.borrow_mut().remove_child(body, open).unwrap();
        dom.borrow_mut().collect_garbage();
        assert!(!dom.borrow().is_alive(completed));
        assert!(dom.borrow().is_alive(open));
        p.write("more</section>").unwrap();
        assert_eq!(dom.borrow().text_content(open).unwrap().as_deref(), Some("more"));
        dom.borrow_mut().collect_garbage();
        assert!(!dom.borrow().is_alive(open));
    }

    struct MoveAnchor {
        dom: DomRef,
        body: NodeId,
        anchor: NodeId,
        count: Cell<usize>,
        expected_doc: NodeId,
    }

    impl crate::custom::ParserCustomElements for MoveAnchor {
        fn construct(&self, element: NodeId, _: &[(QualName, String)]) -> crate::custom::ParserConstruct {
            assert_eq!(self.dom.borrow().node_document(element).unwrap(), self.expected_doc);
            self.count.set(self.count.get() + 1);
            self.dom.borrow_mut().remove_child(self.body, self.anchor).unwrap();
            crate::custom::ParserConstruct::NotDefined
        }
        fn run_reactions(&self) {}
    }

    #[test]
    fn custom_constructor_in_adopted_document_aborts_current_write_on_displacement() {
        let (dom, body, anchor) = fixture();
        let mut p = StreamingFragmentParser::new(dom.clone(), body, body, Some(anchor), false).unwrap();
        let destination = dom.borrow_mut().create_document(crate::DocumentKind::Html, "about:blank");
        dom.borrow_mut().adopt_node(destination, body).unwrap();
        let hooks =
            Rc::new(MoveAnchor { dom: dom.clone(), body, anchor, count: Cell::new(0), expected_doc: destination });
        dom.borrow_mut().note_custom_definition(&"x-move".into(), Vec::new());
        dom.borrow_mut().set_parser_custom_elements(destination, Some(hooks.clone()));
        assert_eq!(
            p.write("<div><x-move></x-move><template shadowrootmode=open><b></b></template></div><x-move></x-move>")
                .unwrap_err()
                .kind(),
            Some(DomErrorKind::HierarchyRequest)
        );
        assert_eq!(hooks.count.get(), 1);
        assert_eq!(dom.borrow().inner_html(body).unwrap(), "<div></div>");
        let host = dom.borrow().first_child(body).unwrap().unwrap();
        assert_eq!(dom.borrow().shadow_root(host).unwrap(), None);
        dom.borrow_mut().append_child(body, anchor).unwrap();
        assert_eq!(p.write("more").unwrap_err().kind(), Some(DomErrorKind::HierarchyRequest));
        assert_eq!(p.close().unwrap_err().kind(), Some(DomErrorKind::HierarchyRequest));
        dom.borrow_mut().set_parser_custom_elements(destination, None);
    }

    #[test]
    fn dropped_subtree_never_constructs_custom_descendants_across_writes() {
        let (dom, body, anchor) = fixture();
        let doc = dom.borrow().node_document(body).unwrap();
        let hooks = Rc::new(MoveAnchor { dom: dom.clone(), body, anchor, count: Cell::new(0), expected_doc: doc });
        dom.borrow_mut().note_custom_definition(&"x-move".into(), Vec::new());
        dom.borrow_mut().set_parser_custom_elements(doc, Some(hooks.clone()));
        let mut filter = FragmentFilter::default();
        filter.remove_elements.insert((ns!(html), "section".into()));
        filter.remove_attributes.insert((ns!(), "title".into()));
        let mut parser =
            StreamingFragmentParser::with_filter(dom.clone(), body, body, Some(anchor), false, Some(Rc::new(filter)))
                .unwrap();
        parser.write("<section><div>").unwrap();
        parser.write("<x-move></x-move></div></section><p title=drop id=keep>visible</p>").unwrap();
        parser.close().unwrap();
        assert_eq!(hooks.count.get(), 0);
        assert_eq!(dom.borrow().inner_html(body).unwrap(), "<p id=\"keep\">visible</p><i></i>");
        dom.borrow_mut().set_parser_custom_elements(doc, None);
    }

    #[test]
    fn live_and_buffered_filters_agree_on_reconstructed_formatting_elements() {
        let (dom, body, _) = fixture();
        let doc = dom.borrow().node_document(body).unwrap();
        let markup = "<b><i>A<p>B</b>C</i>D";
        for removed in ["b", "p", "i"] {
            let mut filter = FragmentFilter::default();
            filter.remove_elements.insert((ns!(html), removed.into()));
            let filter = Rc::new(filter);
            let control = dom.borrow_mut().create_element(doc, "div").unwrap();
            set_html_unsafe_with(&mut dom.borrow_mut(), control, markup, false, Some(filter.clone())).unwrap();
            let target = dom.borrow_mut().create_element(doc, "div").unwrap();
            let mut parser =
                StreamingFragmentParser::with_filter(dom.clone(), target, target, None, false, Some(filter)).unwrap();
            parser.write("<b><i>A<p>").unwrap();
            parser.write("B</b>C</i>D").unwrap();
            parser.close().unwrap();
            assert_eq!(dom.borrow().inner_html(target).unwrap(), dom.borrow().inner_html(control).unwrap());
        }
    }

    /// Parses `markup` into a fresh `div` with `filter` by `setHTMLUnsafe`'s path and by the
    /// streaming parser (split at every position of `markup` into two writes), and returns the
    /// markup of both after checking they agree.
    fn filtered(markup: &str, filter: FragmentFilter) -> String {
        let (dom, body, _) = fixture();
        let doc = dom.borrow().node_document(body).unwrap();
        let filter = Rc::new(filter);
        let control = dom.borrow_mut().create_element(doc, "div").unwrap();
        set_html_unsafe_with(&mut dom.borrow_mut(), control, markup, false, Some(filter.clone())).unwrap();
        let expected = dom.borrow().inner_html(control).unwrap();
        for split in (0..=markup.len()).filter(|&i| markup.is_char_boundary(i)) {
            let target = dom.borrow_mut().create_element(doc, "div").unwrap();
            let mut parser =
                StreamingFragmentParser::with_filter(dom.clone(), target, target, None, false, Some(filter.clone())).unwrap();
            parser.write(&markup[..split]).unwrap();
            parser.write(&markup[split..]).unwrap();
            parser.close().unwrap();
            assert_eq!(dom.borrow().inner_html(target).unwrap(), expected, "split at {split} of {markup:?}");
        }
        expected
    }

    #[test]
    fn replace_with_children_hoists_children_and_follows_the_adoption_agency() {
        let replacing = |names: &[&str]| {
            let mut filter = FragmentFilter::default();
            for name in names {
                filter.replace_with_children.insert((ns!(html), (*name).into()));
            }
            filter
        };
        assert_eq!(filtered("<div><p>Hello <b>World!</b></p></div>", replacing(&["p"])), "<div>Hello <b>World!</b></div>");
        assert_eq!(filtered("<p>a<span>b</span>c</p>", replacing(&["span"])), "<p>abc</p>");
        // A replaced block inside a formatting element the adoption agency splits (WPT sanitizer-in-adoption-agency).
        assert_eq!(filtered("<b><div>Text</b>", replacing(&["div"])), "<b>Text</b><b></b>");
        assert_eq!(filtered("<b>1<p>foo</b>bar", replacing(&["b"])), "1<p>foobar</p>");
        // Table foster parenting around a replaced table goes to the table's place.
        assert_eq!(filtered("<table>f<tr><td>x</td></tr></table>", replacing(&["table"])), "f<tbody><tr><td>x</td></tr></tbody>");
    }

    #[test]
    fn element_allow_list_attribute_rules_and_javascript_urls() {
        let mut filter = crate::sanitizer_defaults::safe_default();
        crate::sanitizer_defaults::remove_unsafe(&mut filter, &["onclick"]);
        let markup = "<div onclick=x id=i><script>1</script><a href=\"jav&#9;ascript:1\" title=t>l</a><a href=\"/x\">m</a><!--c--><img src=x>z</div>";
        assert_eq!(filtered(markup, filter), "<div><a title=\"t\">l</a><a href=\"/x\">m</a>z</div>");
    }

    #[test]
    fn data_attributes_and_per_element_rules() {
        let mut filter = FragmentFilter::default();
        filter.attributes = Some([(ns!(), "class".into())].into_iter().collect());
        filter.data_attributes = true;
        let mut elements: FxHashMap<(crate::Namespace, crate::LocalName), crate::fragment_filter::ElementRule> = FxHashMap::default();
        elements.insert((ns!(html), "div".into()), crate::fragment_filter::ElementRule {
            attributes: Some([(ns!(), "title".into())].into_iter().collect()),
            remove_attributes: [(ns!(), "class".into())].into_iter().collect(),
        });
        elements.insert((ns!(html), "p".into()), crate::fragment_filter::ElementRule::default());
        filter.elements = Some(elements);
        assert_eq!(
            filtered("<div class=c title=t id=i data-x=1><p class=c id=i data-y=2>q</p><span>s</span></div>", filter),
            "<div title=\"t\" data-x=\"1\"><p class=\"c\" data-y=\"2\">q</p></div>"
        );
    }

    #[test]
    fn abort_keeps_completed_tokens_without_flushing_pending_input() {
        let (dom, body, before) = fixture();
        let mut parser = StreamingFragmentParser::new(dom.clone(), body, body, Some(before), false).unwrap();
        parser.write("<p>visible</p><!--unpushed").unwrap();
        drop(parser);
        assert_eq!(dom.borrow().inner_html(body).unwrap(), "<p>visible</p><i></i>");
    }
}
