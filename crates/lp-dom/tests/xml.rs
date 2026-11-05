use lp_dom::{xml, Dom, NodeKind};

fn parse(source: &str) -> (Dom, lp_dom::NodeId) {
    let mut dom = Dom::new();
    let doc = xml::parse_document(&mut dom, "https://example.test/source", source, "application/xml");
    (dom, doc)
}
fn error(source: &str) {
    let (dom, doc) = parse(source);
    let root = dom.document_element(doc).unwrap().unwrap();
    assert_eq!(dom.tag_name(root).unwrap(), "parsererror", "accepted {source:?}");
    assert_eq!(&*dom.element(root).unwrap().name.ns, "http://www.mozilla.org/newlayout/xml/parsererror.xml");
}
#[test]
fn qualified_names_attributes_entities_and_default_namespace_reset() {
    let (dom, doc) = parse(
        r#"<?xml version="1.0"?><r xmlns="urn:root" xmlns:w="urn:w&amp;x"><w:Item w:id="A&#x9;B" plain="x
  y"><child xmlns=""/></w:Item></r>"#,
    );
    let root = dom.document_element(doc).unwrap().unwrap();
    assert_eq!(&*dom.element(root).unwrap().name.ns, "urn:root");
    let item = dom.first_child(root).unwrap().unwrap();
    assert_eq!(dom.tag_name(item).unwrap(), "w:Item");
    assert_eq!(&*dom.element(item).unwrap().name.ns, "urn:w&x");
    assert_eq!(dom.get_attribute_ns(item, Some("urn:w&x"), "id").unwrap(), Some("A\tB"));
    assert_eq!(dom.get_attribute(item, "plain").unwrap(), Some("x   y"));
    let child = dom.first_child(item).unwrap().unwrap();
    assert_eq!(&*dom.element(child).unwrap().name.ns, "");
    assert_eq!(dom.get_attribute(child, "xmlns").unwrap(), Some(""));
}
#[test]
fn text_cdata_comments_pi_and_whitespace_survive() {
    let (dom, doc) = parse("<?work ready?><r> A&amp;&#65;<![CDATA[<x>]]><!--c-->\r\nB</r>");
    let root = dom.document_element(doc).unwrap().unwrap();
    assert!(matches!(
        dom.node(dom.first_child(doc).unwrap().unwrap()).unwrap().kind,
        NodeKind::ProcessingInstruction(_)
    ));
    let children: Vec<_> = dom.children(root).unwrap().collect();
    assert_eq!(children.len(), 4);
    assert!(matches!(&dom.node(children[0]).unwrap().kind, NodeKind::Text(s) if s==" A&A"));
    assert!(matches!(&dom.node(children[1]).unwrap().kind, NodeKind::CData(s) if s=="<x>"));
    assert!(matches!(&dom.node(children[2]).unwrap().kind, NodeKind::Comment(s) if s=="c"));
    assert!(matches!(&dom.node(children[3]).unwrap().kind, NodeKind::Text(s) if s=="\nB"));
}
#[test]
fn malformed_namespaces_and_xml_are_error_documents() {
    for source in [
        "",
        "<r>",
        "<r></q>",
        "<r/><q/>",
        "x<r/>",
        "<r/>x",
        "<r>&unknown;</r>",
        "<r>&#0;</r>",
        "<r>\0</r>",
        "<r>]]></r>",
        "<r a='<'/>",
        "<?xml version='1.0' other='x'?><r/>",
        "<?xml version='1.0' encoding='2invalid'?><r/>",
        "<?xml version='1.0' standalone='yes' encoding='UTF-8'?><r/>",
        "<r><!--a---></r>",
        "<1r/>",
        "<r:1bad xmlns:r='u'/>",
        "<r a='1' a='2'/>",
        "<r xmlns:a='u' xmlns:b='u' a:x='1' b:x='2'/>",
        "<x:r/>",
        "<r x:a='1'/>",
        "<r xmlns:p=''/>",
        "<r xmlns:xml='u'/>",
        "<r xmlns:p='http://www.w3.org/XML/1998/namespace'/>",
        "<r><!--bad--comment--></r>",
        " <?xml version='1.0'?><r/>",
        "<?xml version='1.1'?><r/>",
        "<!DOCTYPE r [<!ENTITY x 'xxx'>]><r>&x;</r>",
        "<r/><!DOCTYPE r>",
    ] {
        error(source);
    }
}
#[test]
fn doctype_becomes_a_document_type_node() {
    let (dom, doc) = parse("<!DOCTYPE r PUBLIC '-//X//DTD r//EN' 'http://x/r.dtd'><r/>");
    let first = dom.first_child(doc).unwrap().unwrap();
    match &dom.node(first).unwrap().kind {
        NodeKind::Doctype(d) => {
            assert_eq!(&*d.name, "r");
            assert_eq!(&*d.public_id, "-//X//DTD r//EN");
            assert_eq!(&*d.system_id, "http://x/r.dtd");
        }
        other => panic!("expected a doctype, got {other:?}"),
    }
    assert_eq!(dom.tag_name(dom.document_element(doc).unwrap().unwrap()).unwrap(), "r");
}
#[test]
fn limits_reject_before_building_partial_dom_trees() {
    error(&format!("{}{}", "<r>".repeat(xml::MAX_DEPTH + 1), "</r>".repeat(xml::MAX_DEPTH + 1)));
    error(&format!("<r>{}</r>", "<n/>".repeat(xml::MAX_NODES)));
    error(&" ".repeat(xml::MAX_INPUT_BYTES + 1));
    error(&format!("<r {}/>", (0..257).map(|i| format!("a{i}='x'")).collect::<Vec<_>>().join(" ")));
}

#[test]
fn malformed_parse_allocates_only_error_document_and_gc_releases_it() {
    let mut dom = Dom::new();
    for _ in 0..10 {
        let source = format!("<r>{}", "<n/>".repeat(1000));
        let _doc = xml::parse_document(&mut dom, "https://example.test/", &source, "text/xml");
        assert_eq!(dom.live_nodes(), 3);
        dom.collect_garbage();
        assert_eq!(dom.live_nodes(), 0);
    }
}

#[test]
fn xmlns_local_element_name_is_not_a_namespace_declaration() {
    let (dom, doc) = parse("<xmlns/>");
    let root = dom.document_element(doc).unwrap().unwrap();
    assert_eq!(dom.tag_name(root).unwrap(), "xmlns");
    assert_eq!(&*dom.element(root).unwrap().name.ns, "");
    error("<xmlns:r/>");
}
