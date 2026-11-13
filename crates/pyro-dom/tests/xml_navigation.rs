//! The tree builder of a navigated XML document (`DocumentParser::new_xml`):
//! it stops at the end tag of each XHTML `<script>` until that script has run
//! (HTML "parsing XML documents"), so what follows a script is not in the
//! document, loads nothing and fires nothing before the script has run.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use pyro_dom::custom::{ParserConstruct, ParserCustomElements};
use pyro_dom::iframe::IframeChange;
use pyro_dom::mutation::ObserveInit;
use pyro_dom::parse::{DocumentParser, DomRef, ParseStep};
use pyro_dom::script::ResourceKind;
use pyro_dom::{DocumentKind, Dom, NodeId, QualName, QuirksMode};

const XHTML: &str = "http://www.w3.org/1999/xhtml";

/// The active document of a navigation to `source`, as far as its parser
/// builds it at the end of input: up to the first script, or all of it.
fn load(source: &str) -> (DomRef, NodeId, DocumentParser, ParseStep) {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = dom.borrow_mut().create_document(DocumentKind::Html, "https://example.test/page.xhtml");
    {
        let mut dom = dom.borrow_mut();
        dom.set_document_active(doc, true).unwrap();
        dom.watch_iframes(doc).unwrap();
        dom.set_resource_tracking(true);
    }
    let mut parser = DocumentParser::new_xml(dom.clone(), doc, "application/xhtml+xml").unwrap();
    assert_eq!(parser.feed_bytes(source.as_bytes()), ParseStep::NeedInput);
    assert!(dom.borrow().document_element(doc).unwrap().is_none(), "nothing is built before the end of input");
    let step = parser.finish_bytes();
    (dom, doc, parser, step)
}

fn script(step: ParseStep) -> NodeId {
    match step {
        ParseStep::Script(script) => script,
        ParseStep::NeedInput => panic!("the parser did not stop at a script"),
    }
}

fn by_id(dom: &Dom, doc: NodeId, id: &str) -> NodeId {
    dom.get_element_by_id(doc, id).unwrap().unwrap_or_else(|| panic!("no #{id} in the document"))
}

fn local_names(dom: &Dom, parent: NodeId) -> Vec<String> {
    dom.children(parent).unwrap().filter_map(|c| dom.element(c).ok().map(|e| e.name.local.to_string())).collect()
}

/// The children of the root element, serialized.
fn markup(dom: &Dom, doc: NodeId) -> String {
    dom.inner_html(dom.document_element(doc).unwrap().unwrap()).unwrap()
}

/// How many images asked to load since the last call.
fn images(dom: &mut Dom) -> usize {
    dom.take_resource_events().iter().filter(|e| e.kind == ResourceKind::Img).count()
}

#[test]
fn what_follows_a_script_is_not_in_the_document_until_the_script_has_run() {
    let source = format!(
        "<html xmlns='{XHTML}'><head><script id='a'>1</script><title>t</title><script id='b' src='b.js'/></head>\
         <body onload='x()'><iframe src='a.xml' onload='loaded()'/><img src='i.png'/></body></html>"
    );
    let (dom, doc, mut parser, step) = load(&source);
    let first = script(step);
    {
        let mut dom = dom.borrow_mut();
        assert_eq!(first, by_id(&dom, doc, "a"));
        assert_eq!(markup(&dom, doc), "<head><script id=\"a\">1</script></head>");
        assert!(dom.take_iframe_events(doc).is_empty());
        assert!(!dom.has_pending_inline_handlers());
        assert_eq!(images(&mut dom), 0);
    }
    let second = script(parser.resume());
    {
        let mut dom = dom.borrow_mut();
        assert_eq!(second, by_id(&dom, doc, "b"));
        assert_eq!(
            markup(&dom, doc),
            "<head><script id=\"a\">1</script><title>t</title><script id=\"b\" src=\"b.js\"></script></head>"
        );
        // The iframe whose `onload` calls what the scripts define is not
        // there to load, nor is its handler compiled, while a script is
        // still to run.
        assert!(dom.take_iframe_events(doc).is_empty());
        assert!(!dom.has_pending_inline_handlers());
        assert_eq!(images(&mut dom), 0);
    }
    assert_eq!(parser.resume(), ParseStep::NeedInput);
    parser.end();
    let mut dom = dom.borrow_mut();
    let body = dom.last_child(dom.document_element(doc).unwrap().unwrap()).unwrap().unwrap();
    assert_eq!(local_names(&dom, body), ["iframe", "img"]);
    let events = dom.take_iframe_events(doc);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].change, IframeChange::Connected);
    assert_eq!(dom.tag_name(events[0].node).unwrap(), "iframe");
    for e in events {
        dom.release(e.node);
    }
    let pending: Vec<String> =
        dom.take_pending_inline_handlers().into_iter().map(|n| dom.tag_name(n).unwrap()).collect();
    assert_eq!(pending, ["body", "iframe"]);
    assert_eq!(images(&mut dom), 1);
}

#[test]
fn the_part_before_the_first_script_gets_its_insertion_steps_at_that_script() {
    let source = format!(
        "<html xmlns='{XHTML}'><body onload='x()'><iframe onload='y()'/><img src='i.png'/>\
         <script>1</script><iframe/></body></html>"
    );
    let (dom, doc, mut parser, step) = load(&source);
    script(step);
    {
        let mut dom = dom.borrow_mut();
        let events = dom.take_iframe_events(doc);
        assert_eq!(events.len(), 1, "the iframe above the script");
        for e in events {
            dom.release(e.node);
        }
        let pending: Vec<String> =
            dom.take_pending_inline_handlers().into_iter().map(|n| dom.tag_name(n).unwrap()).collect();
        assert_eq!(pending, ["body", "iframe"]);
        assert_eq!(images(&mut dom), 1);
    }
    assert_eq!(parser.resume(), ParseStep::NeedInput);
    parser.end();
    let mut dom = dom.borrow_mut();
    let events = dom.take_iframe_events(doc);
    assert_eq!(events.len(), 1, "the iframe below it, once");
    for e in events {
        dom.release(e.node);
    }
    assert!(!dom.has_pending_inline_handlers(), "it has no handler, and those above are not queued twice");
    assert_eq!(images(&mut dom), 0);
}

#[test]
fn nested_scripts_stop_the_parser_innermost_first() {
    let source = format!(
        "<html xmlns='{XHTML}'><head><script id='outer'>a<script id='inner'>b</script>c</script><title/></head></html>"
    );
    let (dom, doc, mut parser, step) = load(&source);
    let inner = script(step);
    {
        let dom = dom.borrow();
        assert_eq!(inner, by_id(&dom, doc, "inner"));
        let outer = by_id(&dom, doc, "outer");
        assert_eq!(dom.text_content(outer).unwrap().as_deref(), Some("ab"), "the outer end tag is still ahead");
    }
    let outer = script(parser.resume());
    {
        let dom = dom.borrow();
        assert_eq!(outer, by_id(&dom, doc, "outer"));
        assert_eq!(dom.text_content(outer).unwrap().as_deref(), Some("abc"));
        assert_eq!(local_names(&dom, dom.parent(outer).unwrap().unwrap()), ["script"]);
    }
    assert_eq!(parser.resume(), ParseStep::NeedInput);
    assert_eq!(parser.resume(), ParseStep::NeedInput, "nothing is left");
    parser.end();
    let dom = dom.borrow();
    assert_eq!(local_names(&dom, dom.parent(outer).unwrap().unwrap()), ["script", "title"]);
}

#[test]
fn insertions_after_a_script_reach_mutation_observers() {
    let source = format!("<html xmlns='{XHTML}'><head><script>1</script><title>t</title></head><body/></html>");
    let (dom, doc, mut parser, step) = load(&source);
    script(step);
    let observer = {
        let mut dom = dom.borrow_mut();
        let observer = dom.create_observer();
        dom.observe(observer, doc, ObserveInit { child_list: true, subtree: true, ..Default::default() }).unwrap();
        observer
    };
    assert_eq!(parser.resume(), ParseStep::NeedInput);
    parser.end();
    let mut dom = dom.borrow_mut();
    let html = dom.document_element(doc).unwrap().unwrap();
    let head = dom.first_child(html).unwrap().unwrap();
    let title = dom.last_child(head).unwrap().unwrap();
    let body = dom.last_child(html).unwrap().unwrap();
    let records: Vec<(NodeId, Vec<NodeId>)> =
        dom.take_records(observer).into_iter().map(|r| (r.target, r.added)).collect();
    let text = dom.first_child(title).unwrap().unwrap();
    assert_eq!(records, [(head, vec![title]), (title, vec![text]), (html, vec![body])]);
}

#[test]
fn an_open_element_a_script_removed_still_gets_the_rest_of_its_content() {
    let source = format!(
        "<html xmlns='{XHTML}'><body><div id='d'><script>1</script><p id='inside'/></div><p id='after'/></body></html>"
    );
    let (dom, doc, mut parser, step) = load(&source);
    let script = script(step);
    let (div, body);
    {
        // What the script does: `d.remove()`, and nothing keeps `d`.
        let mut dom = dom.borrow_mut();
        div = by_id(&dom, doc, "d");
        body = dom.parent(div).unwrap().unwrap();
        dom.remove_child(body, div).unwrap();
        dom.collect_garbage();
        assert!(dom.is_alive(div) && dom.is_alive(script), "the parser holds the elements it has open");
    }
    assert_eq!(parser.resume(), ParseStep::NeedInput);
    {
        let dom = dom.borrow();
        assert_eq!(local_names(&dom, div), ["script", "p"]);
        assert!(!dom.is_connected(div).unwrap());
        assert_eq!(dom.get_element_by_id(doc, "inside").unwrap(), None);
        assert_eq!(local_names(&dom, body), ["p"]);
        assert_eq!(dom.first_child(body).unwrap(), Some(by_id(&dom, doc, "after")));
    }
    parser.end();
    let mut dom = dom.borrow_mut();
    dom.collect_garbage();
    assert!(!dom.is_alive(div), "until it ends");
    assert!(dom.is_alive(body));
}

#[test]
fn content_of_an_element_a_script_moved_to_another_document_is_that_documents() {
    let source = format!(
        "<html xmlns='{XHTML}'><body><div id='d'><script>1</script><script>2</script><p/></div>\
         <script>3</script></body></html>"
    );
    let (dom, doc, mut parser, step) = load(&source);
    script(step);
    let (div, other);
    {
        // What the script does: `otherDocument.adoptNode(d)`.
        let mut dom = dom.borrow_mut();
        div = by_id(&dom, doc, "d");
        other = dom.create_document(DocumentKind::Html, "about:blank");
        dom.adopt_node(other, div).unwrap();
    }
    // The second script is made for the other document, which runs none.
    let last = script(parser.resume());
    {
        let dom = dom.borrow();
        assert_eq!(dom.text_content(last).unwrap().as_deref(), Some("3"));
        assert_eq!(dom.node_document(last).unwrap(), doc);
        assert_eq!(local_names(&dom, div), ["script", "script", "p"]);
        for child in dom.children(div).unwrap() {
            assert_eq!(dom.node_document(child).unwrap(), other);
        }
    }
    assert_eq!(parser.resume(), ParseStep::NeedInput);
    parser.end();
}

#[test]
fn a_malformed_document_is_an_error_document_that_runs_no_script() {
    let source = format!("<html xmlns='{XHTML}'><head><script>1</script></head><body><p></body></html>");
    let (dom, doc, mut parser, step) = load(&source);
    assert_eq!(step, ParseStep::NeedInput);
    assert_eq!(parser.resume(), ParseStep::NeedInput);
    parser.end();
    let mut dom = dom.borrow_mut();
    assert_eq!(dom.child_count(doc).unwrap(), 1);
    let root = dom.document_element(doc).unwrap().unwrap();
    assert_eq!(dom.tag_name(root).unwrap(), "parsererror");
    assert_eq!(local_names(&dom, root), [] as [&str; 0]);
    assert!(dom.take_iframe_events(doc).is_empty());
}

#[test]
fn the_end_of_input_adds_no_html_structure() {
    // The parser of a navigation is an HTML one underneath: its end of file
    // (`<html><head><body>`, quirks mode without a doctype) is not for an
    // XML document.
    for source in ["<root><a/></root>".to_owned(), format!("<html xmlns='{XHTML}'><head><script/></head></html>")] {
        let (dom, doc, mut parser, mut step) = load(&source);
        while let ParseStep::Script(_) = step {
            step = parser.resume();
        }
        parser.end();
        let dom = dom.borrow();
        assert_eq!(dom.child_count(doc).unwrap(), 1, "{source}");
        let root = dom.document_element(doc).unwrap().unwrap();
        assert!(local_names(&dom, root).iter().all(|name| name != "body"), "{source}");
        assert_eq!(dom.document_data(doc).unwrap().quirks, QuirksMode::NoQuirks, "{source}");
    }
}

/// The bindings' parser hooks, counting the calls.
#[derive(Default)]
struct Hooks {
    runs: Cell<u32>,
}

impl ParserCustomElements for Hooks {
    fn construct(&self, _: NodeId, _: &[(QualName, String)]) -> ParserConstruct {
        ParserConstruct::NotDefined
    }
    fn run_reactions(&self) {
        self.runs.set(self.runs.get() + 1);
    }
}

#[test]
fn elements_after_the_script_that_defined_them_are_upgraded_before_the_next_script() {
    let source = format!(
        "<html xmlns='{XHTML}'><body><x-a id='before'/><script>1</script><x-a id='after'/><script>2</script>\
         <p/></body></html>"
    );
    let (dom, doc, mut parser, step) = load(&source);
    script(step);
    let hooks = Rc::new(Hooks::default());
    {
        // What the script does: `customElements.define('x-a', …)`. The
        // definition itself upgrades the element above the script.
        let mut dom = dom.borrow_mut();
        assert!(!dom.has_upgrade_candidates());
        dom.enable_custom_upgrades();
        dom.set_parser_custom_elements(doc, Some(hooks.clone()));
    }
    script(parser.resume());
    assert_eq!(hooks.runs.get(), 1, "the bindings ran what the insertions queued");
    {
        let mut dom = dom.borrow_mut();
        let after = by_id(&dom, doc, "after");
        assert_eq!(dom.take_upgrade_candidates(), [after]);
    }
    assert_eq!(parser.resume(), ParseStep::NeedInput);
    assert_eq!(hooks.runs.get(), 1, "the rest left them nothing to do");
    parser.end();
}
