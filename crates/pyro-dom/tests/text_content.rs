//! `Node.textContent` without copying: borrowed from the one node that
//! holds all of it, concatenated in one walk otherwise.

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::select::parse_query;
use pyro_dom::*;

#[test]
fn text_content_borrows_a_single_text_node() {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", "<!doctype html><body><p id=one>alone</p><p id=two>a<b>b</b><i></i>c</p><p id=none></p><p id=e><b></b>x</p></body>");
    let dom = dom.borrow();
    let one = |css: &str| dom.query_first(d, &parse_query(css).unwrap()).unwrap().unwrap();
    let t = dom.text_content_ref(one("#one")).unwrap().unwrap();
    assert!(matches!(t, Cow::Borrowed("alone")));
    assert_eq!(dom.text_content_ref(one("#two")).unwrap().unwrap(), "abc");
    assert!(matches!(dom.text_content_ref(one("#none")).unwrap().unwrap(), Cow::Borrowed("")));
    assert!(matches!(dom.text_content_ref(one("#e")).unwrap().unwrap(), Cow::Borrowed("x")));
    let first_text = dom.first_child(one("#one")).unwrap().unwrap();
    assert!(matches!(dom.text_content_ref(first_text).unwrap().unwrap(), Cow::Borrowed("alone")));
    assert_eq!(dom.text_content_ref(d).unwrap(), None);
    assert_eq!(dom.text_content(one("body")).unwrap().unwrap(), "aloneabcx");
}

fn active_doc(html: &str) -> (DomRef, NodeId) {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", html);
    dom.borrow_mut().set_document_active(d, true).unwrap();
    (dom, d)
}

fn observe_all(dom: &mut Dom, target: NodeId) -> pyro_dom::mutation::ObserverId {
    let obs = dom.create_observer();
    dom.observe(
        obs,
        target,
        pyro_dom::mutation::ObserveInit {
            child_list: true,
            character_data: Some(true),
            subtree: true,
            character_data_old_value: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    obs
}

/// `textContent = <the text its one text child holds>` replaces the child with an equal one and
/// records it (DOM "string replace all"; the same in Chrome).
#[test]
fn set_text_content_to_the_same_single_text_replaces_the_child() {
    let (dom, d) = active_doc("<!doctype html><body><div id=c>same text</div><p id=e></p><p id=m>a<b></b></p></body>");
    let mut dom = dom.borrow_mut();
    let one = |dom: &Dom, css: &str| dom.query_first(d, &parse_query(css).unwrap()).unwrap().unwrap();
    let (c, e, m) = (one(&dom, "#c"), one(&dom, "#e"), one(&dom, "#m"));
    let obs = observe_all(&mut dom, d);
    let child = dom.first_child(c).unwrap().unwrap();
    let version = dom.version();
    dom.set_text_content(c, "same text").unwrap();
    assert!(dom.version() > version);
    assert_ne!(dom.first_child(c).unwrap(), Some(child));
    assert_eq!(dom.text_content(c).unwrap().unwrap(), "same text");
    let records = dom.take_records(obs);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].removed, [child]);
    // A different text, an empty text, an empty element or several children
    // replace all too.
    dom.set_text_content(c, "other").unwrap();
    assert_eq!(dom.take_records(obs).len(), 1);
    let v = dom.version();
    dom.set_text_content(e, "").unwrap();
    assert_eq!(dom.version(), v, "nothing to remove from an empty element");
    dom.set_text_content(e, "x").unwrap();
    assert!(dom.version() > v);
    let v = dom.version();
    dom.set_text_content(m, "a").unwrap();
    assert!(dom.version() > v, "two children are replaced by one");
    assert_eq!(dom.text_content(m).unwrap().unwrap(), "a");
    assert_eq!(dom.take_records(obs).len(), 2, "one for `e`, one for `m`");
}

/// `data = <the same text>` is a mutation for observers (a characterData
/// record, the spec's "replace data"), not for layout: the version that
/// styles and geometry follow stays.
#[test]
fn replace_data_with_the_same_text_records_but_does_not_change_the_version() {
    let (dom, d) = active_doc("<!doctype html><body><p id=p>same</p></body>");
    let mut dom = dom.borrow_mut();
    let p = dom.query_first(d, &parse_query("#p").unwrap()).unwrap().unwrap();
    let text = dom.first_child(p).unwrap().unwrap();
    let obs = observe_all(&mut dom, d);
    let version = dom.version();
    dom.set_char_data(text, "same").unwrap();
    assert_eq!(dom.version(), version);
    let records = dom.take_records(obs);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, pyro_dom::mutation::RecordKind::CharacterData);
    assert_eq!(records[0].old_value.as_deref(), Some("same"));
    dom.set_char_data(text, "changed").unwrap();
    assert!(dom.version() > version);
    assert_eq!(dom.take_records(obs).len(), 1);
}
