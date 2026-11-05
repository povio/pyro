//! State kept at mutation time instead of recomputed per query: id and
//! class atoms parsed when attributes are written, connectedness bits,
//! `:target`'s id and the `<base>` URL reset only when it can change.

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::parse::{parse_document, DomRef};
use lp_dom::select::parse_query;
use lp_dom::*;

fn doc(url: &str, html: &str) -> (DomRef, NodeId) {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, url, html);
    dom.borrow_mut().set_document_active(d, true).unwrap();
    (dom, d)
}

#[track_caller]
fn verify(dom: &Dom) {
    dom.verify_tree_state().unwrap();
}

fn matches(dom: &Dom, el: NodeId, css: &str) -> bool {
    dom.element_matches(el, &parse_query(css).unwrap()).unwrap()
}

fn all(dom: &Dom, root: NodeId, css: &str) -> Vec<NodeId> {
    dom.query_all(root, &parse_query(css).unwrap()).unwrap()
}

#[test]
fn class_and_id_atoms_follow_every_attribute_write() {
    let (dom, d) = doc("http://test/", "<!doctype html><body><p id=p class='a  b a'></p><i class=b></i></body>");
    let mut dom = dom.borrow_mut();
    let p = dom.get_element_by_id(d, "p").unwrap().unwrap();
    let classes = |dom: &Dom| dom.element(p).unwrap().classes().iter().map(|a| a.to_string()).collect::<Vec<_>>();
    assert_eq!(classes(&dom), ["a", "b", "a"]);
    assert_eq!(all(&dom, d, ".b").len(), 2);
    dom.set_attribute(p, "class", "c").unwrap();
    assert_eq!(classes(&dom), ["c"]);
    assert_eq!(all(&dom, d, ".b").len(), 1);
    assert_eq!(all(&dom, d, ".c"), [p]);
    assert!(matches(&dom, p, "body > .c:not(.b)"));
    // A `class` attribute in a namespace is not the element's class.
    dom.set_attribute_ns(p, Some("urn:x"), "class", "z").unwrap();
    assert!(!matches(&dom, p, ".z"));
    dom.remove_attribute(p, "class").unwrap();
    assert!(classes(&dom).is_empty());
    assert!(!matches(&dom, p, ".c"));
    dom.set_attribute(p, "id", "q").unwrap();
    assert_eq!(dom.element(p).unwrap().id_atom().map(|a| a.to_string()).as_deref(), Some("q"));
    assert!(matches(&dom, p, "#q") && !matches(&dom, p, "#p"));
    // Clones copy the parsed atoms (`setAttribute` would find the namespaced `class` first).
    dom.remove_attribute_ns(p, Some("urn:x"), "class").unwrap();
    dom.set_attribute(p, "class", "k l").unwrap();
    let c = dom.clone_node(p, false, None).unwrap();
    assert!(matches(&dom, c, ".l#q"));
    verify(&dom);
}

#[test]
fn quirks_mode_compares_class_and_id_atoms_without_case() {
    // No doctype: quirks mode, where class and id selectors ignore ASCII case.
    let (dom, d) = doc("http://test/", "<body><p id=Main class=Big></p></body>");
    let dom = dom.borrow();
    let p = dom.get_element_by_id(d, "Main").unwrap().unwrap();
    assert!(matches(&dom, p, ".big") && matches(&dom, p, "#main") && matches(&dom, p, "p.BIG"));
    assert_eq!(all(&dom, d, ".bIG"), [p]);
}

#[test]
fn connectedness_follows_insertion_removal_and_shadow_hosts() {
    let (dom, d) = doc("http://test/", "<!doctype html><body><div id=host></div></body>");
    let mut dom = dom.borrow_mut();
    let body = dom.body(d).unwrap().unwrap();
    let host = dom.get_element_by_id(d, "host").unwrap().unwrap();
    let sr = dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
    let inner = dom.create_element(d, "span").unwrap();
    dom.set_attribute(inner, "id", "inner").unwrap();
    dom.append_child(sr, inner).unwrap();
    assert!(dom.is_connected(inner).unwrap() && dom.is_in_active_document(inner));
    assert_eq!(dom.get_element_by_id(sr, "inner").unwrap(), Some(inner));
    assert_eq!(dom.get_element_by_id(d, "inner").unwrap(), None, "a shadow tree is its own scope");
    dom.remove_node(host).unwrap();
    assert!(!dom.is_connected(host).unwrap() && !dom.is_connected(inner).unwrap());
    assert!(!dom.is_in_active_document(inner));
    assert_eq!(dom.get_element_by_id(d, "host").unwrap(), None);
    assert_eq!(dom.get_element_by_id(sr, "inner").unwrap(), Some(inner), "a detached shadow root keeps its ids");
    verify(&dom);
    // A subtree built detached connects as a whole.
    let wrap = dom.create_element(d, "section").unwrap();
    dom.append_child(wrap, host).unwrap();
    assert!(!dom.is_connected(inner).unwrap());
    dom.append_child(body, wrap).unwrap();
    assert!(dom.is_connected(inner).unwrap());
    assert_eq!(dom.get_element_by_id(d, "host").unwrap(), Some(host));
    verify(&dom);
    // Duplicates: the first in tree order wins, and the next one after it leaves.
    let early = dom.create_element(d, "b").unwrap();
    dom.set_attribute(early, "id", "host").unwrap();
    let first = dom.first_child(body).unwrap();
    dom.insert_before(body, early, first).unwrap();
    assert_eq!(dom.get_element_by_id(d, "host").unwrap(), Some(early));
    verify(&dom);
    dom.remove_node(early).unwrap();
    assert_eq!(dom.get_element_by_id(d, "host").unwrap(), Some(host));
    // Another document's tree: its own scope.
    let other = dom.create_document(DocumentKind::Html, "about:blank");
    let html = dom.create_element(other, "html").unwrap();
    dom.append_child(other, html).unwrap();
    dom.append_child(html, wrap).unwrap();
    assert!(dom.is_connected(inner).unwrap() && !dom.is_in_active_document(inner));
    assert_eq!(dom.get_element_by_id(other, "host").unwrap(), Some(host));
    assert_eq!(dom.get_element_by_id(d, "host").unwrap(), None);
    verify(&dom);
}

#[test]
fn target_is_the_decoded_url_fragment() {
    let (dom, d) = doc("http://test/#t%C3%A9", "<!doctype html><body><p id=té></p><p id=other></p></body>");
    let mut dom = dom.borrow_mut();
    let p = dom.get_element_by_id(d, "té").unwrap().unwrap();
    let other = dom.get_element_by_id(d, "other").unwrap().unwrap();
    assert!(matches(&dom, p, ":target") && !matches(&dom, other, ":target"));
    dom.set_document_url(d, "http://test/#other").unwrap();
    assert!(!matches(&dom, p, ":target") && matches(&dom, other, ":target"));
    // Detached: not the target.
    dom.remove_node(other).unwrap();
    assert!(!matches(&dom, other, ":target"));
    dom.set_document_url(d, "http://test/").unwrap();
    assert!(!matches(&dom, p, ":target"));
}

#[test]
fn the_base_url_changes_with_base_elements_and_their_href() {
    let (dom, d) = doc("http://test/dir/page", "<!doctype html><head><base href=/a/></head><body></body>");
    let mut dom = dom.borrow_mut();
    let base_url = |dom: &Dom| dom.document_base_url(d).unwrap().to_owned();
    assert_eq!(base_url(&dom), "http://test/a/");
    let base = dom.query_first(d, &parse_query("base").unwrap()).unwrap().unwrap();
    // Unrelated mutations keep it.
    let body = dom.body(d).unwrap().unwrap();
    dom.set_attribute(body, "data-x", "1").unwrap();
    assert_eq!(base_url(&dom), "http://test/a/");
    dom.set_attribute(base, "href", "/b/").unwrap();
    assert_eq!(base_url(&dom), "http://test/b/");
    // Another <base> earlier in tree order wins; one in a detached or shadow tree does not count.
    let early = dom.create_element(d, "base").unwrap();
    dom.set_attribute(early, "href", "/c/").unwrap();
    let wrap = dom.create_element(d, "div").unwrap();
    dom.append_child(wrap, early).unwrap();
    assert_eq!(base_url(&dom), "http://test/b/");
    let host = dom.create_element(d, "div").unwrap();
    dom.append_child(body, host).unwrap();
    let sr = dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
    let shadowed = dom.create_element(d, "base").unwrap();
    dom.set_attribute(shadowed, "href", "/s/").unwrap();
    dom.remove_node(base).unwrap();
    assert_eq!(base_url(&dom), "http://test/dir/page");
    dom.append_child(sr, shadowed).unwrap();
    assert_eq!(base_url(&dom), "http://test/dir/page");
    let head = dom.head(d).unwrap().unwrap();
    dom.append_child(head, wrap).unwrap();
    assert_eq!(base_url(&dom), "http://test/c/");
    dom.append_child(head, base).unwrap();
    assert_eq!(base_url(&dom), "http://test/c/");
    // Moving the first one after the other.
    dom.move_before(head, wrap, None).unwrap();
    assert_eq!(base_url(&dom), "http://test/b/");
    dom.set_document_url(d, "http://elsewhere/x").unwrap();
    assert_eq!(base_url(&dom), "http://elsewhere/b/");
    verify(&dom);
}

#[test]
fn many_duplicate_ids_enter_in_one_pass_and_keep_the_first() {
    let (dom, d) = doc("http://test/", "<!doctype html><body><p id=late></p></body>");
    let mut dom = dom.borrow_mut();
    let body = dom.body(d).unwrap().unwrap();
    let p = dom.get_element_by_id(d, "late").unwrap().unwrap();
    // A detached list of rows sharing one id, connected by one insert.
    let ul = dom.create_element(d, "ul").unwrap();
    let mut rows = Vec::new();
    for _ in 0..2000 {
        let li = dom.create_element(d, "li").unwrap();
        dom.set_attribute(li, "id", "row").unwrap();
        dom.append_child(ul, li).unwrap();
        rows.push(li);
    }
    dom.set_attribute(rows[1500], "id", "late").unwrap();
    // Before an existing `late`: the subtree's one becomes the first.
    dom.insert_before(body, ul, Some(p)).unwrap();
    assert_eq!(dom.get_element_by_id(d, "row").unwrap(), Some(rows[0]));
    assert_eq!(dom.get_element_by_id(d, "late").unwrap(), Some(rows[1500]));
    verify(&dom);
    // Into the middle, after the first: the first stays.
    let mid = dom.create_element(d, "li").unwrap();
    dom.set_attribute(mid, "id", "row").unwrap();
    dom.insert_before(ul, mid, Some(rows[1000])).unwrap();
    assert_eq!(dom.get_element_by_id(d, "row").unwrap(), Some(rows[0]));
    // Before the first: it becomes the first.
    let head = dom.create_element(d, "li").unwrap();
    dom.set_attribute(head, "id", "row").unwrap();
    dom.insert_before(ul, head, Some(rows[0])).unwrap();
    assert_eq!(dom.get_element_by_id(d, "row").unwrap(), Some(head));
    verify(&dom);
    // innerHTML of a connected element: the same rows, parsed.
    let html = format!("<li id=late></li>{}", "<li id=row></li>".repeat(2000));
    lp_dom::parse::set_inner_html(&mut dom, ul, &html).unwrap();
    let first = dom.first_child(ul).unwrap().unwrap();
    assert_eq!(dom.get_element_by_id(d, "late").unwrap(), Some(first));
    assert_eq!(dom.get_element_by_id(d, "row").unwrap(), dom.next_sibling(first).unwrap());
    verify(&dom);
    // innerHTML of a detached element: its ids are nowhere indexed.
    let detached = dom.create_element(d, "div").unwrap();
    lp_dom::parse::set_inner_html(&mut dom, detached, "<i id=row></i><template><b id=row></b></template>").unwrap();
    assert!(!dom.is_connected(dom.first_child(detached).unwrap().unwrap()).unwrap());
    assert_eq!(dom.get_element_by_id(d, "row").unwrap(), dom.next_sibling(first).unwrap());
    verify(&dom);
}

#[test]
fn tree_order_matches_a_preorder_index() {
    let (dom, d) = doc(
        "http://test/",
        "<!doctype html><body><div><a></a><b><i></i><u></u></b><s></s></div><p><q></q></p><em></em></body>",
    );
    let dom = dom.borrow();
    let mut order = Vec::new();
    let mut stack = vec![d];
    while let Some(n) = stack.pop() {
        order.push(n);
        let kids: Vec<_> = dom.children(n).unwrap().collect();
        stack.extend(kids.into_iter().rev());
    }
    for (i, &a) in order.iter().enumerate() {
        for (j, &b) in order.iter().enumerate() {
            if i != j {
                let following = dom.compare_document_position(a, b).unwrap() & 0x04 != 0;
                assert_eq!(following, i < j, "{i} vs {j}");
            }
        }
    }
}
