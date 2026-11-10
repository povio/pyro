//! XML tree building of `<template>`: an XHTML template's children go into
//! its template contents (HTML "parsing XHTML documents"); templates in other
//! namespaces keep their children.

use lp_dom::{xml, Dom, NodeId};

const XHTML: &str = "http://www.w3.org/1999/xhtml";

fn parse(source: &str) -> (Dom, NodeId) {
    let mut dom = Dom::new();
    let doc = xml::parse_document(&mut dom, "https://example.test/source", source, "application/xml");
    (dom, doc)
}

fn local_names(dom: &Dom, parent: NodeId) -> Vec<String> {
    dom.children(parent).unwrap().filter_map(|c| dom.element(c).ok().map(|e| e.name.local.to_string())).collect()
}

#[test]
fn xhtml_template_children_go_into_template_contents() {
    let (dom, doc) = parse(&format!("<template xmlns='{XHTML}'><test/>text<!--c--></template>"));
    let template = dom.document_element(doc).unwrap().unwrap();
    assert_eq!(&*dom.element(template).unwrap().name.ns, XHTML);
    assert_eq!(dom.child_count(template).unwrap(), 0, "the template element itself stays empty");
    let contents = dom.template_contents(template).unwrap().expect("XHTML template has contents");
    assert_eq!(local_names(&dom, contents), ["test"]);
    assert_eq!(dom.child_count(contents).unwrap(), 3);
    let test = dom.first_child(contents).unwrap().unwrap();
    assert_eq!(&*dom.element(test).unwrap().name.ns, XHTML, "default namespace is inherited through the template");
    let inert = dom.node_document(contents).unwrap();
    assert_ne!(inert, doc, "template contents live in the inert document");
    assert_eq!(dom.node_document(test).unwrap(), inert);
    assert_eq!(dom.inner_html(template).unwrap(), "<test></test>text<!--c-->");
}

#[test]
fn nested_xhtml_templates_and_descendants_of_contents() {
    let (dom, doc) = parse(&format!("<html xmlns='{XHTML}'><body><template><p><template><i/></template></p></template></body></html>"));
    let html = dom.document_element(doc).unwrap().unwrap();
    let body = dom.first_child(html).unwrap().unwrap();
    let outer = dom.first_child(body).unwrap().unwrap();
    assert_eq!(dom.tag_name(outer).unwrap(), "template");
    assert_eq!(dom.child_count(outer).unwrap(), 0);
    let outer_contents = dom.template_contents(outer).unwrap().unwrap();
    let p = dom.first_child(outer_contents).unwrap().unwrap();
    assert_eq!(dom.tag_name(p).unwrap(), "p");
    let inner = dom.first_child(p).unwrap().unwrap();
    assert_eq!(dom.tag_name(inner).unwrap(), "template");
    assert_eq!(dom.child_count(inner).unwrap(), 0);
    let inner_contents = dom.template_contents(inner).unwrap().unwrap();
    assert_eq!(local_names(&dom, inner_contents), ["i"]);
    // One inert document serves the whole tree.
    assert_eq!(dom.node_document(inner_contents).unwrap(), dom.node_document(outer_contents).unwrap());
    assert_eq!(dom.outer_html(body).unwrap(), "<body><template><p><template><i></i></template></p></template></body>");
}

#[test]
fn templates_outside_the_xhtml_namespace_keep_their_children() {
    for source in ["<template><test/></template>", "<template xmlns='http://www.w3.org/2000/svg'><test/></template>"] {
        let (dom, doc) = parse(source);
        let template = dom.document_element(doc).unwrap().unwrap();
        assert_eq!(dom.template_contents(template).unwrap(), None, "{source}");
        assert_eq!(local_names(&dom, template), ["test"], "{source}");
        assert_eq!(dom.node_document(dom.first_child(template).unwrap().unwrap()).unwrap(), doc);
    }
}

#[test]
fn cloning_an_xhtml_template_copies_its_contents() {
    let (mut dom, doc) = parse(&format!("<template xmlns='{XHTML}'><test a='1'/></template>"));
    let template = dom.document_element(doc).unwrap().unwrap();
    let copy = dom.clone_node(template, true, None).unwrap();
    assert_eq!(dom.child_count(copy).unwrap(), 0);
    let contents = dom.template_contents(copy).unwrap().unwrap();
    assert_ne!(contents, dom.template_contents(template).unwrap().unwrap());
    assert_eq!(local_names(&dom, contents), ["test"]);
    assert_eq!(dom.outer_html(copy).unwrap(), dom.outer_html(template).unwrap());
    let shallow = dom.clone_node(template, false, None).unwrap();
    assert_eq!(dom.child_count(dom.template_contents(shallow).unwrap().unwrap()).unwrap(), 0);
}

#[test]
fn navigated_xhtml_scripts_inside_template_contents_do_not_run() {
    // The parser of a navigation stops at the runnable HTML `<script>`
    // elements; those in template contents belong to the inert document and
    // are not among them.
    use lp_dom::parse::{DocumentParser, ParseStep};
    let dom = std::rc::Rc::new(std::cell::RefCell::new(Dom::new()));
    let doc = dom.borrow_mut().create_document(lp_dom::DocumentKind::Html, "https://example.test/page.xhtml");
    let source =
        format!("<html xmlns='{XHTML}'><body><template><script>1</script></template><script>2</script></body></html>");
    let mut parser = DocumentParser::new_xml(dom.clone(), doc, "application/xhtml+xml").unwrap();
    assert_eq!(parser.feed_bytes(source.as_bytes()), ParseStep::NeedInput);
    let mut scripts = Vec::new();
    let mut step = parser.finish_bytes();
    while let ParseStep::Script(script) = step {
        scripts.push(script);
        step = parser.resume();
    }
    parser.end();
    let dom = dom.borrow();
    let html = dom.document_element(doc).unwrap().unwrap();
    let body = dom.first_child(html).unwrap().unwrap();
    assert_eq!(local_names(&dom, body), ["template", "script"]);
    let (template, runnable) = {
        let mut kids = dom.children(body).unwrap();
        (kids.next().unwrap(), kids.next().unwrap())
    };
    assert_eq!(scripts, vec![runnable]);
    let contents = dom.template_contents(template).unwrap().unwrap();
    assert_eq!(local_names(&dom, contents), ["script"]);
    assert_ne!(dom.node_document(contents).unwrap(), doc);
}

#[test]
fn xhr_response_bytes_build_template_contents_too() {
    // `parse_bytes` (XHR `responseXML`) shares the tree builder.
    let mut dom = Dom::new();
    let bytes = format!("<template xmlns='{XHTML}'><test/></template>");
    let doc = xml::parse_bytes(&mut dom, "https://example.test/x", bytes.as_bytes(), None, "text/xml").unwrap();
    let template = dom.document_element(doc).unwrap().unwrap();
    assert_eq!(dom.child_count(template).unwrap(), 0);
    let contents = dom.template_contents(template).unwrap().unwrap();
    assert_eq!(local_names(&dom, contents), ["test"]);
}
