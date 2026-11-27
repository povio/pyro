//! `textContent = <the one text child's own text>` replaces that child with an equal one and
//! records the replacement (DOM "string replace all"; Chrome does the same): which observers see
//! it, the cases around it (CDATA, comments, elements, two texts, an empty text), fragments and
//! shadow roots, `<style>` and `<textarea>`.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::mutation::{ObserveInit, ObserverId, RecordKind};
use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::select::parse_query;
use pyro_dom::snapshot::{DeltaOp, RenderHtml};
use pyro_dom::*;

fn active_doc(html: &str) -> (DomRef, NodeId) {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", html);
    dom.borrow_mut().set_document_active(d, true).unwrap();
    (dom, d)
}

fn one(dom: &Dom, root: NodeId, css: &str) -> NodeId {
    dom.query_first(root, &parse_query(css).unwrap()).unwrap().unwrap()
}

fn observe(dom: &mut Dom, target: NodeId, init: ObserveInit) -> ObserverId {
    let obs = dom.create_observer();
    dom.observe(obs, target, init).unwrap();
    obs
}

fn child_list() -> ObserveInit {
    ObserveInit { child_list: true, subtree: true, ..Default::default() }
}

fn character_data() -> ObserveInit {
    ObserveInit { character_data: Some(true), subtree: true, character_data_old_value: Some(true), ..Default::default() }
}

const OPTS: RenderHtml<'static> = RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None };

#[test]
fn observers_see_the_replacement_of_a_text_by_itself() {
    let (dom, d) = active_doc("<body><p id=p>same</p></body>");
    let mut dom = dom.borrow_mut();
    let p = one(&dom, d, "#p");
    let text = dom.first_child(p).unwrap().unwrap();
    let on_doc = observe(&mut dom, d, child_list());
    let on_p = observe(&mut dom, p, ObserveInit { child_list: true, ..Default::default() });
    let chars = observe(&mut dom, d, character_data());
    let on_text = observe(&mut dom, text, ObserveInit { character_data: Some(true), ..Default::default() });
    dom.set_text_content(p, "same").unwrap();
    for obs in [on_doc, on_p] {
        let records = dom.take_records(obs);
        assert_eq!(records.len(), 1, "{obs:?}");
        assert_eq!(records[0].kind, RecordKind::ChildList);
        assert_eq!(records[0].removed, [text]);
        assert_eq!(records[0].added.len(), 1);
    }
    // The old child was removed, not edited.
    assert!(dom.take_records(chars).is_empty());
    assert!(dom.take_records(on_text).is_empty());
    assert_ne!(dom.first_child(p).unwrap(), Some(text), "the child is a new node");
}

#[test]
fn every_text_content_set_replaces_what_the_element_had() {
    let (dom, d) = active_doc("<body><p id=c>x<!-- c --></p><p id=e><b>x</b></p><p id=t>x</p><p id=z></p><p id=w> x</p></body>");
    let mut dom = dom.borrow_mut();
    let (c, e, t, z, w) = (one(&dom, d, "#c"), one(&dom, d, "#e"), one(&dom, d, "#t"), one(&dom, d, "#z"), one(&dom, d, "#w"));
    let obs = observe(&mut dom, d, child_list());
    for (el, text) in [(c, "x"), (e, "x"), (t, "x"), (w, "x")] {
        let v = dom.version();
        dom.set_text_content(el, text).unwrap();
        assert!(dom.version() > v);
        assert_eq!(dom.child_count(el).unwrap(), 1);
        assert_eq!(dom.node_type(dom.first_child(el).unwrap().unwrap()).unwrap(), NodeType::Text);
        assert_eq!(dom.take_records(obs).len(), 1);
    }
    // One empty text child and "": the child is removed.
    let empty = dom.create_text(d, "").unwrap();
    dom.append_child(z, empty).unwrap();
    dom.take_records(obs);
    dom.set_text_content(z, "").unwrap();
    assert_eq!(dom.first_child(z).unwrap(), None);
    let records = dom.take_records(obs);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].removed, [empty]);
    // A fragment and a shadow root with one same text child: replaced too.
    let frag = dom.create_fragment(d).unwrap();
    let ft = dom.create_text(d, "f").unwrap();
    dom.append_child(frag, ft).unwrap();
    dom.set_text_content(frag, "f").unwrap();
    assert_ne!(dom.first_child(frag).unwrap(), Some(ft));
    let host = dom.create_element(d, "div").unwrap();
    let body = one(&dom, d, "body");
    dom.append_child(body, host).unwrap();
    let sr = dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
    let st = dom.create_text(d, "s").unwrap();
    dom.append_child(sr, st).unwrap();
    dom.set_text_content(sr, "s").unwrap();
    assert_ne!(dom.first_child(sr).unwrap(), Some(st));
}

#[test]
fn a_cdata_child_with_the_same_text_is_replaced() {
    let mut dom = Dom::new();
    let d = xml::parse_document(&mut dom, "http://test/x.xml", "<r><![CDATA[same]]></r>", "application/xml");
    let r = dom.document_element(d).unwrap().unwrap();
    let cdata = dom.first_child(r).unwrap().unwrap();
    assert_eq!(dom.node_type(cdata).unwrap(), NodeType::CDataSection);
    assert_eq!(dom.child_count(r).unwrap(), 1);
    let obs = observe(&mut dom, d, child_list());
    dom.set_text_content(r, "same").unwrap();
    let first = dom.first_child(r).unwrap().unwrap();
    assert_ne!(first, cdata);
    assert_eq!(dom.node_type(first).unwrap(), NodeType::Text);
    assert_eq!(dom.take_records(obs).len(), 1);
    // Now a Text child: replaced as well.
    dom.set_text_content(r, "same").unwrap();
    assert_ne!(dom.first_child(r).unwrap(), Some(first));
    assert_eq!(dom.take_records(obs).len(), 1);
}

#[test]
fn style_and_textarea_text_set_to_itself_render_as_before() {
    let (dom, d) = active_doc("<head><style id=s>p{color:red}</style></head><body><textarea id=t>def</textarea><p id=p>x</p></body>");
    let mut dom = dom.borrow_mut();
    let (s, t) = (one(&dom, d, "#s"), one(&dom, d, "#t"));
    let html = dom.render_html(d, &OPTS).unwrap();
    dom.set_text_content(s, "p{color:red}").unwrap();
    assert_eq!(dom.render_html(d, &OPTS).unwrap(), html);
    // The textarea's default value is its text: unchanged, and a typed
    // value (dirty) stays, as replacing the text with itself would leave it.
    assert_eq!(dom.value(t).unwrap(), "def");
    dom.set_text_content(t, "def").unwrap();
    assert_eq!(dom.value(t).unwrap(), "def");
    dom.set_value(t, "typed").unwrap();
    dom.set_text_content(t, "def").unwrap();
    assert_eq!(dom.value(t).unwrap(), "typed");
    assert_eq!(dom.text_content(t).unwrap().unwrap(), "def");
    // A real change to the default value after typing keeps the typed value
    // (dirty), as before.
    dom.set_text_content(t, "other").unwrap();
    assert_eq!(dom.value(t).unwrap(), "typed");
    assert_eq!(dom.text_content(t).unwrap().unwrap(), "other");
    // And the style's real change is still a sheet-text op.
    let since = dom.version();
    dom.set_text_content(s, "p{color:blue}").unwrap();
    let delta = dom.render_delta(d, since, &OPTS).expect("a delta");
    assert!(delta.ops.iter().any(|op| matches!(op, DeltaOp::Node { kind: pyro_dom::snapshot::RenderNodeKind::Text(t), .. } if t == "p{color:blue}")), "{:?}", delta.ops);
}
