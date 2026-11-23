use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::parse::{prescan_charset, DocumentParser, DomRef, ParseStep};
use pyro_dom::script::{ResourceKind, ResourceReason, ScriptKind};
use pyro_dom::*;

#[test]
fn parser_scripts_start_at_their_start_tags_line() {
    // An inline script's text starts on the line of its start tag's end (a tag across lines:
    // the line of its `>`), as Chrome numbers inline script lines in the document; a script
    // created by script has none (1).
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = dom.borrow_mut().create_document(DocumentKind::Html, "http://t/");
    dom.borrow_mut().set_document_active(doc, true).unwrap();
    let mut p = DocumentParser::new(dom.clone(), doc, true).unwrap();
    let html = "<!doctype html>\n<p>a\n\n<script>\nx()\n</script>\n<script\n  nonce=abc>y()</script>";
    let mut lines = Vec::new();
    let mut step = p.feed(html);
    while let ParseStep::Script(s) = step {
        lines.push(dom.borrow().script_info(s).unwrap().start_line);
        step = p.resume();
    }
    p.end();
    assert_eq!(lines, [4, 8]);
    let mut d = dom.borrow_mut();
    let script = d.create_element(doc, "script").unwrap();
    assert_eq!(d.script_info(script).unwrap().start_line, 1);
}

#[test]
fn parser_yields_scripts_and_reports_resources() {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    dom.borrow_mut().set_resource_tracking(true);
    let doc = dom.borrow_mut().create_document(DocumentKind::Html, "http://t/");
    dom.borrow_mut().set_document_active(doc, true).unwrap();
    let mut p = DocumentParser::new(dom.clone(), doc, true).unwrap();
    let html = r#"<head><link rel=stylesheet href=a.css><style>p{}</style>
        <script src=app.js defer></script><script type=module>import x from "/m.js"</script>
        <script type="text/x-template">t</script></head><body><img src=i.png><iframe src=f.html></iframe>"#;
    let mut scripts = Vec::new();
    let mut step = p.feed(html);
    while let ParseStep::Script(s) = step {
        scripts.push(dom.borrow().script_info(s).unwrap());
        step = p.resume();
    }
    p.end();
    let kinds: Vec<_> = scripts.iter().map(|s| (s.kind, s.src.clone(), s.defer, s.parser_inserted)).collect();
    assert_eq!(
        kinds,
        [
            (ScriptKind::Classic, Some("app.js".to_string()), true, true),
            (ScriptKind::Module, None, false, true),
            (ScriptKind::Data, None, false, true),
        ]
    );
    assert_eq!(scripts[1].inline_text, r#"import x from "/m.js""#);
    let evs = dom.borrow_mut().take_resource_events();
    let summary: Vec<_> = evs.iter().map(|e| (e.kind, e.reason)).collect();
    assert!(summary.contains(&(ResourceKind::Stylesheet, ResourceReason::Connected)));
    assert!(summary.contains(&(ResourceKind::StyleText, ResourceReason::ChildrenChanged)));
    assert!(summary.contains(&(ResourceKind::Img, ResourceReason::Connected)));
    assert!(summary.contains(&(ResourceKind::Iframe, ResourceReason::Connected)));
    assert!(!summary.iter().any(|(k, _)| *k == ResourceKind::Script), "parser scripts go through ParseStep");

    // Script-inserted <link> and <script> (Vite pattern) are reported.
    let mut d = dom.borrow_mut();
    let head = d.head(doc).unwrap().unwrap();
    let link = d.create_element(doc, "link").unwrap();
    d.set_attribute(link, "rel", "stylesheet").unwrap();
    d.set_attribute(link, "href", "/v.css").unwrap();
    assert!(d.take_resource_events().is_empty(), "detached elements load nothing");
    d.append_child(head, link).unwrap();
    let script = d.create_element(doc, "script").unwrap();
    d.set_attribute(script, "src", "/dyn.js").unwrap();
    d.append_child(head, script).unwrap();
    let evs = d.take_resource_events();
    assert_eq!(evs.len(), 2);
    assert_eq!((evs[0].node, evs[0].kind, evs[0].reason), (link, ResourceKind::Stylesheet, ResourceReason::Connected));
    assert_eq!((evs[1].node, evs[1].kind), (script, ResourceKind::Script));
    assert!(!d.script_info(script).unwrap().parser_inserted);
    d.set_attribute(link, "href", "/w.css").unwrap();
    d.remove_node(link).unwrap();
    let evs = d.take_resource_events();
    assert_eq!(
        evs.iter().map(|e| e.reason).collect::<Vec<_>>(),
        [ResourceReason::AttrChanged, ResourceReason::Removed]
    );
}

#[test]
fn byte_decoding() {
    assert_eq!(prescan_charset(br#"<meta charset="windows-1252">"#).unwrap().name(), "windows-1252");
    assert_eq!(
        prescan_charset(br#"<meta http-equiv="Content-Type" content="text/html; charset=ISO-8859-2">"#).unwrap().name(),
        "ISO-8859-2"
    );
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = dom.borrow_mut().create_document(DocumentKind::Html, "http://t/");
    let mut p = DocumentParser::new(dom.clone(), doc, false).unwrap();
    // "café" in windows-1252, split so the multi-byte boundary logic is exercised.
    let bytes = b"<meta charset=windows-1252><p id=x>caf\xe9</p>";
    let (a, b) = bytes.split_at(20);
    let _ = p.feed_bytes(a);
    let _ = p.feed_bytes(b);
    let _ = p.finish_bytes();
    p.end();
    let d = dom.borrow();
    let x = d.get_element_by_id(doc, "x").unwrap().unwrap();
    assert_eq!(d.text_content(x).unwrap().unwrap(), "café");
    assert_eq!(d.document_data(doc).unwrap().charset, "windows-1252");

    // UTF-8 split inside a multi-byte sequence.
    let dom2: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc2 = dom2.borrow_mut().create_document(DocumentKind::Html, "http://t/");
    let mut p2 = DocumentParser::new(dom2.clone(), doc2, false).unwrap();
    p2.set_transport_charset(Some("utf-8"));
    let s = "<p id=y>žš😀</p>".as_bytes();
    let _ = p2.feed_bytes(&s[..10]);
    let _ = p2.feed_bytes(&s[10..]);
    let _ = p2.finish_bytes();
    p2.end();
    let d2 = dom2.borrow();
    let y = d2.get_element_by_id(doc2, "y").unwrap().unwrap();
    assert_eq!(d2.text_content(y).unwrap().unwrap(), "žš😀");
}

#[test]
fn regression_parser_roots_detached_open_elements() {
    let d: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = d.borrow_mut().create_document(DocumentKind::Html, "http://test/");
    d.borrow_mut().set_document_active(doc, true).unwrap();
    let mut p = DocumentParser::new(d.clone(), doc, true).unwrap();
    assert!(matches!(p.feed("<body><div id=x><script></script><span>after</span></div>"), ParseStep::Script(_)));
    let div = d.borrow().get_element_by_id(doc, "x").unwrap().unwrap();
    d.borrow_mut().remove_node(div).unwrap();
    d.borrow_mut().collect_garbage();
    p.resume();
    p.end();
    assert!(d.borrow().is_alive(div), "parser lost the open div");
    assert_eq!(d.borrow().text_content(div).unwrap().unwrap(), "after");
    d.borrow_mut().collect_garbage();
    assert!(!d.borrow().is_alive(div), "ended parser retained detached elements");
}

#[test]
fn ended_parser_releases_a_document_left_while_parsing() {
    let d: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = d.borrow_mut().create_document(DocumentKind::Html, "http://test/");
    d.borrow_mut().set_document_active(doc, true).unwrap();
    let mut p = DocumentParser::new(d.clone(), doc, true).unwrap();
    assert!(matches!(p.feed("<body><div id=x><script></script><span>after</span></div>"), ParseStep::Script(_)));
    let div = d.borrow().get_element_by_id(doc, "x").unwrap().unwrap();
    // Navigated away mid-parse: the parser's references keep the tree.
    d.borrow_mut().set_document_active(doc, false).unwrap();
    d.borrow_mut().collect_garbage();
    assert!(d.borrow().is_alive(div), "parser lost its document's tree");
    p.resume();
    p.end();
    // Its connected nodes are released through the document.
    d.borrow_mut().collect_garbage();
    assert!(!d.borrow().is_alive(div), "ended parser retained an inactive document's tree");
    assert!(!d.borrow().is_alive(doc), "ended parser retained an inactive document");
}
#[test]
fn regression_replace_child_record_sibling() {
    use pyro_dom::mutation::ObserveInit;
    let mut d = Dom::new();
    let doc = d.create_document(DocumentKind::Html, "http://test/");
    let p = d.create_element(doc, "p").unwrap();
    let a = d.create_element(doc, "a").unwrap();
    let b = d.create_element(doc, "b").unwrap();
    d.append_child(p, a).unwrap();
    d.append_child(p, b).unwrap();
    let o = d.create_observer();
    d.observe(o, p, ObserveInit { child_list: true, ..Default::default() }).unwrap();
    d.replace_child(p, a, b).unwrap();
    let records = d.take_records(o);
    assert_eq!(records.last().unwrap().previous_sibling, None, "previousSibling is the inserted node itself");
}
#[test]
fn regression_split_bom_beats_transport() {
    let d: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = d.borrow_mut().create_document(DocumentKind::Html, "http://test/");
    let mut p = DocumentParser::new(d.clone(), doc, false).unwrap();
    p.set_transport_charset(Some("windows-1252"));
    p.feed_bytes(&[0xef]);
    p.feed_bytes(b"\xbb\xbf<p id=x>\xc3\xa9</p>");
    p.finish_bytes();
    p.end();
    let d = d.borrow();
    let x = d.get_element_by_id(doc, "x").unwrap().unwrap();
    assert_eq!(d.text_content(x).unwrap().unwrap(), "é");
}
#[test]
fn regression_resource_queue_pins_nodes() {
    let mut d = Dom::new();
    let doc = d.create_document(DocumentKind::Html, "http://test/");
    d.set_document_active(doc, true).unwrap();
    d.set_resource_tracking(true);
    let s = d.create_element(doc, "script").unwrap();
    d.set_attribute(s, "src", "/side-effect.js").unwrap();
    d.append_child(doc, s).unwrap();
    d.remove_node(s).unwrap();
    d.collect_garbage();
    let events = d.take_resource_events();
    assert!(events.iter().all(|ev| d.is_alive(ev.node)), "resource event contains a stale node: {events:?}");
    d.collect_garbage();
    assert!(!d.is_alive(s), "drained resource queue retained detached script");
}

/// HTML "prescan a byte stream" and "get an XML encoding"
/// (html/syntax/charset/meta-*.html, html/syntax/xmldecl/): comments are
/// skipped, attributes parse as the prescan reads them, `content` needs
/// `http-equiv=content-type`, a `<meta>` beats an XML declaration, and an
/// XML declaration's encoding applies when no `<meta>` declares one.
#[test]
fn prescan_follows_the_html_algorithm() {
    let name = |bytes: &[u8]| prescan_charset(bytes).map(|e| e.name());
    assert_eq!(name(b"<!--<meta charset=\"ISO-8859-8\">--><meta charset  =\n \"ISO-8859-5\"/>"), Some("ISO-8859-5"));
    assert_eq!(name(b"<meta content=\"text/html; charset=ISO-8859-5\" http-equiv=\"Content-Type\">"), Some("ISO-8859-5"));
    assert_eq!(name(b"<meta content=\"text/html; charset=ISO-8859-5\">"), None);
    assert_eq!(name(b"<meta charset=x-user-defined>"), Some("windows-1252"));
    assert_eq!(name(b"<meta charset=utf-16le>"), Some("UTF-8"));
    assert_eq!(name(b"<p title=\"<meta charset=windows-1251>\"><meta charset=windows-1253>"), Some("windows-1253"));
    assert_eq!(name(b"<?xml version=\"1.0\" encoding=\"windows-1251\"?><p>"), Some("windows-1251"));
    assert_eq!(name(b"<?xml version=\"1.0\" encoding=\"windows-1251\"?>\n<meta charset=\"windows-1253\">"), Some("windows-1253"));
    assert_eq!(name(b"<?xml version=\"1.0\" <meta charset=\"windows-1253\" encoding=\"windows-1251\"?>"), Some("windows-1251"));
    assert_eq!(name(b"<?XML encoding=\"windows-1251\"?>"), None);
    assert_eq!(name(b"<?xml encoding=windows-1251?>"), None);
}

/// With no declared encoding, a document takes the fallback its embedder
/// sets (a same-origin parent document's), not UTF-8
/// (html/syntax/charset/with-inheritance.html).
#[test]
fn an_undeclared_encoding_falls_back_to_the_inherited_one() {
    let d: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = d.borrow_mut().create_document(DocumentKind::Html, "http://test/");
    let mut p = DocumentParser::new(d.clone(), doc, false).unwrap();
    p.set_fallback_charset(Some("windows-1251"));
    p.feed_bytes(b"<p id=x>\xe6</p>");
    p.finish_bytes();
    p.end();
    let d = d.borrow();
    let x = d.get_element_by_id(doc, "x").unwrap().unwrap();
    assert_eq!(d.text_content(x).unwrap().unwrap(), "ж");
}

#[test]
fn json_document_keeps_utf8_text_across_chunks_and_charset_hints() {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = dom.borrow_mut().create_document(DocumentKind::Html, "http://test/data");
    let mut parser = DocumentParser::new_json(dom.clone(), doc, "application/problem+json").unwrap();
    parser.set_fallback_charset(Some("windows-1251"));
    parser.set_transport_charset(Some("windows-1252"));
    let text = r#"{"text":"雪 <b> &amp;"}"#;
    for byte in b"\xef\xbb\xbf".iter().chain(text.as_bytes()) {
        parser.feed_bytes(std::slice::from_ref(byte));
    }
    parser.finish_bytes();
    parser.end();
    let dom = dom.borrow();
    let data = dom.document_data(doc).unwrap();
    assert_eq!(data.content_type.as_ref(), "application/problem+json");
    assert_eq!(data.charset, "UTF-8");
    assert_eq!(dom.text_content(dom.body(doc).unwrap().unwrap()).unwrap().as_deref(), Some(text));
}

#[test]
fn document_parser_keeps_sanitize_patches_inert() {
    // The document's parser patches a `<template for>` live, but has no sanitizer: a
    // `sanitize` one stays an ordinary template (its contents inert), the target untouched.
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = dom.borrow_mut().create_document(DocumentKind::Html, "http://t/");
    let mut p = DocumentParser::new(dom.clone(), doc, false).unwrap();
    p.enable_template_patches();
    let html = "<div id=a><?start name='a'>old a<?end></div><template for=a sanitize><b id=sa>new</b></template>\
                <div id=b><?start name='b'>old b<?end></div><template for=b><b id=sb>new</b></template>";
    let mut step = p.feed(html);
    while let ParseStep::Script(_) = step {
        step = p.resume();
    }
    p.end();
    let d = dom.borrow();
    let a = d.get_element_by_id(doc, "a").unwrap().unwrap();
    assert_eq!(d.text_content(a).unwrap().unwrap(), "old a");
    assert!(d.get_element_by_id(doc, "sa").unwrap().is_none());
    let b = d.get_element_by_id(doc, "b").unwrap().unwrap();
    assert_eq!(d.text_content(b).unwrap().unwrap(), "new");
}
