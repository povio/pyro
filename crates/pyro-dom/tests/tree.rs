use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::events::{dispatch, AddOptions, EventState, ListenerHandle, ListenerInvoker, TargetKey};
use pyro_dom::mutation::{ObserveInit, RecordKind};
use pyro_dom::parse::{parse_document, parse_fragment, set_inner_html, DomRef};
use pyro_dom::*;

fn doc(html: &str) -> (DomRef, NodeId) {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", html);
    dom.borrow_mut().set_document_active(d, true).unwrap();
    (dom, d)
}

#[test]
fn parse_and_serialize_roundtrip() {
    let (dom, d) = doc("<!doctype html><html><head><title>x</title></head><body><p id=a class='b c'>Hi &amp; <b>there</b></p><br><template><i>t</i></template><script>if (a<b) {}</script></body></html>");
    let dom = dom.borrow();
    let body = dom.body(d).unwrap().unwrap();
    assert_eq!(
        dom.inner_html(body).unwrap(),
        "<p id=\"a\" class=\"b c\">Hi &amp; <b>there</b></p><br><template><i>t</i></template><script>if (a<b) {}</script>"
    );
    let p = dom.get_element_by_id(d, "a").unwrap().unwrap();
    assert_eq!(dom.tag_name(p).unwrap(), "P");
    assert_eq!(dom.text_content(p).unwrap().unwrap(), "Hi & there");
    assert!(dom.doctype(d).unwrap().is_some());
    assert!(dom.outer_html(d).unwrap().starts_with("<!DOCTYPE html><html>"));
}

#[test]
fn hierarchy_errors() {
    let (dom, d) = doc("<div id=x><span id=y></span></div>");
    let mut dom = dom.borrow_mut();
    let x = dom.get_element_by_id(d, "x").unwrap().unwrap();
    let y = dom.get_element_by_id(d, "y").unwrap().unwrap();
    // Inserting an ancestor into its descendant.
    let e = dom.append_child(y, x).unwrap_err();
    assert_eq!(e.kind(), Some(DomErrorKind::HierarchyRequest));
    // Reference child not a child of parent.
    let t = dom.create_text(d, "t").unwrap();
    let e = dom.insert_before(x, t, Some(x)).unwrap_err();
    assert_eq!(e.kind(), Some(DomErrorKind::NotFound));
    // Text directly under document.
    let e = dom.append_child(d, t).unwrap_err();
    assert_eq!(e.kind(), Some(DomErrorKind::HierarchyRequest));
    // Second document element.
    let el = dom.create_element(d, "div").unwrap();
    let e = dom.append_child(d, el).unwrap_err();
    assert_eq!(e.kind(), Some(DomErrorKind::HierarchyRequest));
    // removeChild of non-child.
    let e = dom.remove_child(y, x).unwrap_err();
    assert_eq!(e.kind(), Some(DomErrorKind::NotFound));
}

#[test]
fn fragment_insertion_and_order() {
    let (dom, d) = doc("<ul id=l><li>b</li></ul>");
    let mut dom = dom.borrow_mut();
    let l = dom.get_element_by_id(d, "l").unwrap().unwrap();
    let f = dom.create_fragment(d).unwrap();
    for t in ["x", "y"] {
        let li = dom.create_element(d, "li").unwrap();
        let tx = dom.create_text(d, t).unwrap();
        dom.append_child(li, tx).unwrap();
        dom.append_child(f, li).unwrap();
    }
    let first = dom.first_child(l).unwrap();
    dom.insert_before(l, f, first).unwrap();
    assert_eq!(dom.inner_html(l).unwrap(), "<li>x</li><li>y</li><li>b</li>");
    assert_eq!(dom.child_count(f).unwrap(), 0);
    assert_eq!(dom.child_count(l).unwrap(), 3);
    let second = dom.child_at(l, 1).unwrap().unwrap();
    assert_eq!(dom.text_content(second).unwrap().unwrap(), "y");
    assert_eq!(dom.child_at(l, 2).unwrap().map(|c| dom.text_content(c).unwrap().unwrap()), Some("b".into()));
    assert_eq!(dom.index_in_parent(second).unwrap(), 1);
    let last = dom.last_child(l).unwrap().unwrap();
    assert_eq!(dom.compare_document_position(second, last).unwrap(), position::FOLLOWING);
    assert_eq!(dom.compare_document_position(last, second).unwrap(), position::PRECEDING);
    assert_eq!(dom.compare_document_position(l, second).unwrap(), position::CONTAINED_BY | position::FOLLOWING);
}

#[test]
fn inner_html_and_template() {
    let (dom, d) = doc("<div id=x></div><template id=t></template>");
    let mut dom = dom.borrow_mut();
    let x = dom.get_element_by_id(d, "x").unwrap().unwrap();
    set_inner_html(&mut dom, x, "<b>1</b><script>alert(1)</script>2").unwrap();
    assert_eq!(dom.inner_html(x).unwrap(), "<b>1</b><script>alert(1)</script>2");
    let script = dom.child_at(x, 1).unwrap().unwrap();
    let flags = dom.element(script).unwrap().flags;
    assert!(flags & node::element_flags::SCRIPT_ALREADY_STARTED != 0, "innerHTML scripts never run");
    // tr context parses table content correctly.
    let tbl = dom.create_element(d, "tbody").unwrap();
    let frag = parse_fragment(&mut dom, tbl, "<tr><td>a</td></tr>").unwrap();
    assert_eq!(dom.outer_html(frag).unwrap(), "<tr><td>a</td></tr>");
    let t = dom.get_element_by_id(d, "t").unwrap().unwrap();
    set_inner_html(&mut dom, t, "<p>in</p>").unwrap();
    assert_eq!(dom.child_count(t).unwrap(), 0);
    let content = dom.template_contents(t).unwrap().unwrap();
    assert_eq!(dom.child_count(content).unwrap(), 1);
    assert_ne!(dom.node_document(content).unwrap(), d, "template contents live in the inert document");
}

#[test]
fn attributes_and_case() {
    let (dom, d) = doc("<div id=x DATA-Foo=1></div>");
    let mut dom = dom.borrow_mut();
    let x = dom.get_element_by_id(d, "x").unwrap().unwrap();
    assert_eq!(dom.get_attribute(x, "data-foo").unwrap(), Some("1"));
    assert_eq!(dom.get_attribute(x, "DATA-FOO").unwrap(), Some("1"));
    dom.set_attribute(x, "Title", "t").unwrap();
    assert_eq!(dom.attributes(x).unwrap().last().unwrap().name.local.to_string(), "title");
    assert!(dom.toggle_attribute(x, "hidden", None).unwrap());
    assert!(!dom.toggle_attribute(x, "hidden", None).unwrap());
    // DOM "valid attribute local name": digits may lead, but not `=`, `/`, `>`
    // or whitespace.
    assert!(dom.set_attribute(x, "1ok", "").is_ok());
    assert!(dom.set_attribute(x, "a=b", "").is_err());
    assert!(dom.set_attribute(x, "a b", "").is_err());
    dom.set_attribute_ns(x, Some("http://www.w3.org/1999/xlink"), "xlink:href", "#a").unwrap();
    assert_eq!(dom.get_attribute(x, "xlink:href").unwrap(), Some("#a"));
    assert_eq!(dom.get_attribute_ns(x, Some("http://www.w3.org/1999/xlink"), "href").unwrap(), Some("#a"));
    let e = dom.set_attribute_ns(x, None, "a:b", "").unwrap_err();
    assert_eq!(e.kind(), Some(DomErrorKind::Namespace));
}

#[test]
fn character_data_utf16() {
    let (dom, d) = doc("");
    let mut dom = dom.borrow_mut();
    let t = dom.create_text(d, "a😀b").unwrap();
    assert_eq!(dom.char_length(t).unwrap(), 4);
    assert_eq!(dom.substring_data(t, 1, 2).unwrap(), "😀");
    assert_eq!(dom.substring_data(t, 3, 10).unwrap(), "b");
    assert!(dom.substring_data(t, 5, 1).is_err());
    dom.replace_data(t, 1, 2, "é").unwrap();
    assert_eq!(dom.char_data(t).unwrap(), "aéb");
    let body = dom.body(d).unwrap().unwrap();
    dom.append_child(body, t).unwrap();
    let tail = dom.split_text(t, 2).unwrap();
    assert_eq!(dom.char_data(t).unwrap(), "aé");
    assert_eq!(dom.char_data(tail).unwrap(), "b");
    assert_eq!(dom.next_sibling(t).unwrap(), Some(tail));
    dom.normalize(body).unwrap();
    assert_eq!(dom.child_count(body).unwrap(), 1);
    assert_eq!(dom.char_data(t).unwrap(), "aéb");
}

#[test]
fn mutation_records() {
    let (dom, d) = doc("<div id=x><p>a</p></div>");
    let mut dom = dom.borrow_mut();
    let x = dom.get_element_by_id(d, "x").unwrap().unwrap();
    let obs = dom.create_observer();
    dom.observe(
        obs,
        x,
        ObserveInit {
            child_list: true,
            subtree: true,
            attribute_old_value: Some(true),
            character_data_old_value: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    let p = dom.first_child(x).unwrap().unwrap();
    let txt = dom.first_child(p).unwrap().unwrap();
    dom.set_attribute(p, "class", "k").unwrap();
    dom.set_attribute(p, "class", "m").unwrap();
    dom.set_char_data(txt, "b").unwrap();
    dom.remove_child(x, p).unwrap();
    // Transient registration: changes in removed subtree still observed until notify.
    dom.set_attribute(p, "id", "gone").unwrap();
    assert_eq!(dom.take_pending_observers(), vec![obs]);
    dom.clear_transient(obs);
    let recs = dom.take_records(obs);
    let kinds: Vec<_> = recs.iter().map(|r| r.kind).collect();
    assert_eq!(
        kinds,
        vec![
            RecordKind::Attributes,
            RecordKind::Attributes,
            RecordKind::CharacterData,
            RecordKind::ChildList,
            RecordKind::Attributes
        ]
    );
    assert_eq!(recs[0].old_value, None);
    assert_eq!(recs[1].old_value.as_deref(), Some("k"));
    assert_eq!(recs[2].old_value.as_deref(), Some("a"));
    assert_eq!(recs[3].removed, vec![p]);
    // After clear_transient, removed subtree is no longer observed.
    dom.set_attribute(p, "id", "again").unwrap();
    assert!(dom.take_records(obs).is_empty());
    // Invalid options.
    assert!(dom.observe(obs, x, ObserveInit::default()).is_err());
}

#[test]
fn garbage_collection_reclaims_detached_trees() {
    let (dom, d) = doc("<div id=x></div>");
    let mut dom = dom.borrow_mut();
    let x = dom.get_element_by_id(d, "x").unwrap().unwrap();
    dom.collect_garbage();
    let base = dom.live_nodes();
    // Build and drop 1000 subtrees, like an SPA re-render.
    for _ in 0..1000 {
        let div = dom.create_element(d, "div").unwrap();
        let t = dom.create_text(d, "row").unwrap();
        dom.append_child(div, t).unwrap();
        dom.append_child(x, div).unwrap();
        dom.remove_child(x, div).unwrap();
    }
    assert!(dom.live_nodes() >= base + 2000);
    dom.collect_garbage();
    assert_eq!(dom.live_nodes(), base, "detached unreferenced subtrees freed");

    // A wrapper reference keeps a detached subtree (and its children) alive.
    let div = dom.create_element(d, "div").unwrap();
    let t = dom.create_text(d, "kept").unwrap();
    dom.append_child(div, t).unwrap();
    dom.add_ref(t).unwrap();
    dom.collect_garbage();
    assert!(dom.is_alive(div) && dom.is_alive(t));
    assert_eq!(dom.parent(t).unwrap(), Some(div));
    dom.release(t);
    dom.collect_garbage();
    assert!(!dom.is_alive(div) && !dom.is_alive(t));
    // Stale handles are detected after slot reuse.
    let reused = dom.create_element(d, "span").unwrap();
    assert!(dom.node(t).is_err() || reused.index() != t.index());
    assert_eq!(dom.node(div).unwrap_err(), DomError::StaleNode);
    // Connected nodes are never collected.
    assert!(dom.is_alive(x));
}

#[test]
fn documents_are_kept_by_their_nodes() {
    let (dom, d) = doc("");
    let mut dom = dom.borrow_mut();
    let other = dom.create_document(DocumentKind::Html, "about:blank");
    let el = dom.create_element(other, "p").unwrap();
    dom.add_ref(el).unwrap();
    dom.collect_garbage();
    assert!(dom.is_alive(other), "ownerDocument must survive while its node lives");
    // Adopting into the main document releases the other document.
    let body = dom.body(d).unwrap().unwrap();
    dom.append_child(body, el).unwrap();
    assert_eq!(dom.node_document(el).unwrap(), d);
    dom.release(el);
    dom.collect_garbage();
    assert!(!dom.is_alive(other));
    assert!(dom.is_alive(el));
}

struct Recorder {
    log: Rc<RefCell<Vec<String>>>,
    dom: DomRef,
    event: Rc<RefCell<EventState>>,
}
impl ListenerInvoker for Recorder {
    fn invoke(&mut self, cb: ListenerHandle, current: TargetKey, _t: TargetKey) {
        let phase = self.event.borrow().phase as u16;
        self.log.borrow_mut().push(format!("{}@{:?}:{}", cb.0, current, phase));
        if cb.0 == 3 {
            self.event.borrow_mut().stop_propagation = true;
        }
        if cb.0 == 10 {
            // Removing a later listener during dispatch prevents it from firing.
            let k = current;
            self.dom.borrow_mut().remove_listener(k, "click", ListenerHandle(11), false);
        }
    }
}

#[test]
fn event_dispatch_order() {
    let (dom, d) = doc("<div id=outer><span id=inner></span></div>");
    let (outer, inner, win) = {
        let mut dm = dom.borrow_mut();
        let outer = dm.get_element_by_id(d, "outer").unwrap().unwrap();
        let inner = dm.get_element_by_id(d, "inner").unwrap().unwrap();
        let win = dm.new_target_key();
        dm.set_document_window(d, Some(win));
        (dm.node_key(outer).unwrap(), dm.node_key(inner).unwrap(), win)
    };
    let cap = AddOptions { capture: true, once: false, passive: None };
    let bub = AddOptions { capture: false, once: false, passive: None };
    {
        let mut dm = dom.borrow_mut();
        assert!(dm.add_listener(win, "click", ListenerHandle(1), cap));
        assert!(dm.add_listener(outer, "click", ListenerHandle(2), cap));
        assert!(!dm.add_listener(outer, "click", ListenerHandle(2), cap), "duplicate ignored");
        assert!(dm.add_listener(inner, "click", ListenerHandle(10), bub));
        assert!(dm.add_listener(inner, "click", ListenerHandle(11), bub));
        assert!(dm.add_listener(inner, "click", ListenerHandle(12), AddOptions { once: true, ..bub }));
        assert!(dm.add_listener(outer, "click", ListenerHandle(4), bub));
        assert!(dm.add_listener(win, "click", ListenerHandle(5), bub));
    }
    let log = Rc::new(RefCell::new(Vec::new()));
    let event = Rc::new(RefCell::new(EventState::new("click", true, true, false)));
    let mut rec = Recorder { log: log.clone(), dom: dom.clone(), event: event.clone() };
    assert!(dispatch(&dom, inner, &event, &mut rec));
    let l = log.borrow().clone();
    let ids: Vec<&str> = l.iter().map(|s| s.split('@').next().unwrap()).collect();
    assert_eq!(ids, vec!["1", "2", "10", "12", "4", "5"]);
    assert!(l[2].ends_with(":2"), "at-target phase");
    assert!(l[4].ends_with(":3"), "bubbling phase");
    // `once` listener removed and released.
    assert!(dom.borrow_mut().take_released_listeners().iter().any(|(_, h)| *h == ListenerHandle(12)));

    // stopPropagation in capture at outer stops everything below.
    log.borrow_mut().clear();
    dom.borrow_mut().add_listener(outer, "click", ListenerHandle(3), cap);
    let event2 = Rc::new(RefCell::new(EventState::new("click", true, true, false)));
    rec.event = event2.clone();
    dispatch(&dom, inner, &event2, &mut rec);
    let ids: Vec<String> = log.borrow().iter().map(|s| s.split('@').next().unwrap().to_owned()).collect();
    assert_eq!(ids, vec!["1", "2", "3"]);

    // Non-bubbling event skips ancestors in the bubble phase; load does not reach window.
    log.borrow_mut().clear();
    let event3 = Rc::new(RefCell::new(EventState::new("focus", false, false, false)));
    {
        let mut dm = dom.borrow_mut();
        dm.add_listener(win, "focus", ListenerHandle(20), bub);
        dm.add_listener(inner, "focus", ListenerHandle(21), bub);
    }
    rec.event = event3.clone();
    dispatch(&dom, inner, &event3, &mut rec);
    let ids: Vec<String> = log.borrow().iter().map(|s| s.split('@').next().unwrap().to_owned()).collect();
    assert_eq!(ids, vec!["21"]);
}

#[test]
fn shadow_retargeting_path() {
    let (dom, d) = doc("<div id=host></div>");
    let mut dm = dom.borrow_mut();
    let host = dm.get_element_by_id(d, "host").unwrap().unwrap();
    let sr = dm.attach_shadow(host, ShadowRootMode::Open).unwrap();
    let inner = dm.create_element(d, "b").unwrap();
    dm.append_child(sr, inner).unwrap();
    assert!(dm.is_connected(inner).unwrap());
    let ev = EventState::new("click", true, true, true);
    let path = dm.event_path(dm.node_key(inner).unwrap(), &ev);
    let host_entry = path.iter().find(|p| p.0 == dm.node_key(host).unwrap()).unwrap();
    assert_eq!(host_entry.1, dm.node_key(host).unwrap(), "target retargeted to host outside shadow tree");
    let ev2 = EventState::new("x", true, true, false);
    let path2 = dm.event_path(dm.node_key(inner).unwrap(), &ev2);
    assert_eq!(path2.len(), 2, "non-composed stops at shadow root");
    assert!(dm.attach_shadow(host, ShadowRootMode::Open).is_err());
}

#[test]
fn non_node_targets_take_an_explicit_event_parent() {
    let (dom, _d) = doc("");
    let mut dm = dom.borrow_mut();
    let (request, tx, db) = (dm.new_target_key(), dm.new_target_key(), dm.new_target_key());
    let ev = EventState::new("error", true, true, false);
    assert_eq!(dm.event_path(request, &ev), vec![(request, request)]);
    dm.set_target_parent(request, Some(tx));
    dm.set_target_parent(tx, Some(db));
    let path: Vec<TargetKey> = dm.event_path(request, &ev).into_iter().map(|(k, _)| k).collect();
    assert_eq!(path, vec![request, tx, db]);
    // A forgotten target leaves no link behind; unlinking removes the parent.
    dm.forget_target(request);
    assert_eq!(dm.event_path(request, &ev).len(), 1);
    dm.set_target_parent(tx, None);
    assert_eq!(dm.event_path(tx, &ev).len(), 1);
}

#[test]
fn clone_deep_with_template() {
    let (dom, d) = doc("<div id=x a=1><template><p>t</p></template><input id=i></div>");
    let mut dm = dom.borrow_mut();
    let x = dm.get_element_by_id(d, "x").unwrap().unwrap();
    let c = dm.clone_node(x, true, None).unwrap();
    assert_eq!(dm.outer_html(c).unwrap(), dm.outer_html(x).unwrap());
    assert_eq!(dm.parent(c).unwrap(), None);
    let shallow = dm.clone_node(x, false, None).unwrap();
    assert_eq!(dm.child_count(shallow).unwrap(), 0);
}

#[test]
fn get_element_by_id_index_tracks_mutations_in_tree_order() {
    // getElementById uses a per-version id index (Playwright's
    // accessible-name lookups made the tree walk quadratic on TOS pages).
    let (dom, d) =
        doc("<div id=a>1</div><section><p id=dup>first</p></section><p id=dup>second</p><span id=''></span>");
    let mut dom = dom.borrow_mut();
    let by = |dom: &Dom, id: &str| {
        dom.get_element_by_id(d, id).unwrap().map(|n| dom.text_content(n).unwrap().unwrap_or_default())
    };
    assert_eq!(by(&dom, "dup").as_deref(), Some("first"), "first in tree order");
    assert_eq!(by(&dom, ""), None);
    assert_eq!(by(&dom, "missing"), None);
    // An id change is visible to the next lookup.
    let a = dom.get_element_by_id(d, "a").unwrap().unwrap();
    dom.set_attribute(a, "id", "b").unwrap();
    assert_eq!(by(&dom, "a"), None);
    assert_eq!(by(&dom, "b").as_deref(), Some("1"));
    // A new element inserted before the first duplicate wins.
    let body = dom.body(d).unwrap().unwrap();
    let early = dom.create_element(d, "i").unwrap();
    dom.set_attribute(early, "id", "dup").unwrap();
    let t = dom.create_text(d, "early").unwrap();
    dom.append_child(early, t).unwrap();
    let first_child = dom.first_child(body).unwrap();
    dom.insert_before(body, early, first_child).unwrap();
    assert_eq!(by(&dom, "dup").as_deref(), Some("early"));
    // Removing it falls back to the next one; reinserting at the end does not win.
    dom.remove_child(body, early).unwrap();
    assert_eq!(by(&dom, "dup").as_deref(), Some("first"));
    dom.append_child(body, early).unwrap();
    assert_eq!(by(&dom, "dup").as_deref(), Some("first"));
    // Removing the id attribute drops the element from the index.
    let b = dom.get_element_by_id(d, "b").unwrap().unwrap();
    dom.remove_attribute(b, "id").unwrap();
    assert_eq!(by(&dom, "b"), None);
}

#[test]
fn declarative_shadow_roots_are_parser_only_and_outlive_their_template() {
    let (dom, d) = doc("<!doctype html><body><div id=w></div>");
    let mut dm = dom.borrow_mut();
    let w = dm.get_element_by_id(d, "w").unwrap().unwrap();
    let markup = "<div id=h><template shadowrootmode=closed shadowrootclonable shadowrootserializable><b>in</b></template><i>light</i></div>";

    set_inner_html(&mut dm, w, markup).unwrap();
    let h = dm.first_child(w).unwrap().unwrap();
    assert_eq!(dm.shadow_root(h).unwrap(), None, "innerHTML leaves a template");
    assert!(dm.inner_html(h).unwrap().starts_with("<template "));

    pyro_dom::parse::set_html_unsafe(&mut dm, w, markup).unwrap();
    dm.collect_garbage();
    let h = dm.first_child(w).unwrap().unwrap();
    assert_eq!(dm.inner_html(h).unwrap(), "<i>light</i>");
    let sr = dm.shadow_root(h).unwrap().expect("declarative root");
    assert_eq!(dm.shadow_root_mode(sr).unwrap(), Some(ShadowRootMode::Closed));
    assert_eq!(dm.inner_html(sr).unwrap(), "<b>in</b>", "contents survive the template's collection");
    assert_eq!(dm.get_html(h, false, &[]).unwrap(), "<i>light</i>");
    let serialized =
        "<template shadowrootmode=\"closed\" shadowrootserializable=\"\" shadowrootclonable=\"\"><b>in</b></template><i>light</i>";
    assert_eq!(dm.get_html(h, true, &[]).unwrap(), serialized);
    assert_eq!(dm.get_html(h, false, &[sr]).unwrap(), serialized);

    // attachShadow claims a declarative root of its mode once, emptied.
    assert!(dm.attach_shadow(h, ShadowRootMode::Open).is_err());
    assert_eq!(dm.attach_shadow(h, ShadowRootMode::Closed).unwrap(), sr);
    assert_eq!(dm.inner_html(sr).unwrap(), "");
    assert!(dm.attach_shadow(h, ShadowRootMode::Closed).is_err());
    let copy = dm.clone_node(h, false, None).unwrap();
    assert!(dm.shadow_root(copy).unwrap().is_some(), "clonable root comes along in a shallow clone");
    assert!(dm.clone_node(sr, true, None).is_err(), "a shadow root itself is not clonable");

    // Everything a parse makes is collected with the tree it replaced.
    pyro_dom::parse::set_html_unsafe(&mut dm, w, markup).unwrap();
    dm.collect_garbage();
    let first = dm.live_nodes();
    for _ in 0..3 {
        pyro_dom::parse::set_html_unsafe(&mut dm, w, markup).unwrap();
        dm.collect_garbage();
    }
    assert_eq!(dm.live_nodes(), first);
}

#[test]
fn declarative_shadow_roots_follow_the_document_opt_in() {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let html = "<!doctype html><body><div id=h><template shadowrootmode=open><b>x</b></template></div>";
    let plain = parse_document(&dom, "http://test/", html);
    let safe = pyro_dom::parse::parse_html_unsafe(&dom, html);
    let dm = dom.borrow();
    let h = dm.get_element_by_id(plain, "h").unwrap().unwrap();
    assert_eq!(dm.shadow_root(h).unwrap(), None);
    let h = dm.get_element_by_id(safe, "h").unwrap().unwrap();
    let sr = dm.shadow_root(h).unwrap().unwrap();
    assert_eq!(dm.inner_html(sr).unwrap(), "<b>x</b>");
}

#[test]
fn element_and_attribute_names_follow_the_dom_standard() {
    use pyro_dom::dom::{is_valid_attribute_local_name, is_valid_element_local_name};
    for ok in ["foo", "f}oo", "foo\u{ffff}", "_a", ":b", "\u{0bc6}foo"] {
        assert!(is_valid_element_local_name(ok), "{ok:?}");
    }
    for bad in ["", "1foo", "foo>", "foo/", "f oo", "<foo", "}foo", "\u{0bc6}fo o"] {
        assert!(!is_valid_element_local_name(bad), "{bad:?}");
    }
    assert!(is_valid_attribute_local_name("1bad"));
    assert!(!is_valid_attribute_local_name("a=b"));
    let (dom, d) = doc("<div id=x></div>");
    let mut dom = dom.borrow_mut();
    assert!(dom.create_element(d, "1foo").is_err());
    assert!(dom.create_element(d, "f}oo").is_ok());
    assert!(dom.create_element_ns(d, None, "a:b").is_err(), "a prefix needs a namespace");
}

#[test]
fn move_before_checks_connectivity_and_moves_without_removal_records() {
    let (dom, d) = doc("<div id=a><p id=p></p></div><div id=b></div>");
    let mut dom = dom.borrow_mut();
    let (a, b, p) = (
        dom.get_element_by_id(d, "a").unwrap().unwrap(),
        dom.get_element_by_id(d, "b").unwrap().unwrap(),
        dom.get_element_by_id(d, "p").unwrap().unwrap(),
    );
    let detached = dom.create_element(d, "span").unwrap();
    assert!(dom.move_before(b, detached, None).is_err(), "node must be connected");
    assert!(dom.move_before(detached, p, None).is_err(), "parent must be connected");
    assert!(dom.move_before(p, a, None).is_err(), "an ancestor cannot move into its descendant");
    dom.move_before(b, p, None).unwrap();
    assert_eq!(dom.parent(p).unwrap(), Some(b));
    assert_eq!(dom.first_child(a).unwrap(), None);
}

#[test]
fn namespace_prefix_lookups() {
    let (dom, d) = doc("<div id=x xmlns:p='urn:p'><svg id=s><circle id=c></circle></svg></div>");
    let frag = dom.borrow_mut().create_fragment(d).unwrap();
    let dom = dom.borrow();
    let c = dom.get_element_by_id(d, "c").unwrap().unwrap();
    assert_eq!(dom.lookup_prefix(c, Some("http://www.w3.org/2000/svg")).unwrap(), None, "no prefix on the element");
    assert_eq!(dom.lookup_namespace_uri(c, None).unwrap(), Some("http://www.w3.org/2000/svg"));
    assert_eq!(dom.lookup_namespace_uri(c, Some("xml")).unwrap(), Some("http://www.w3.org/XML/1998/namespace"));
    assert_eq!(dom.lookup_namespace_uri(frag, Some("xml")).unwrap(), None, "fragments have no namespace");
}

/// `innerHTML` set to text without markup takes a path around the fragment
/// parser; what it builds must be what the parser builds in every context.
#[test]
fn inner_html_text_matches_the_fragment_parser() {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", "<!doctype html><body>");
    let mut dom = dom.borrow_mut();
    fn describe(dom: &Dom, parent: NodeId) -> Vec<String> {
        dom.children(parent)
            .unwrap()
            .map(|c| match dom.char_data(c) {
                Ok(data) => format!("{:?} {data:?}", dom.node_type(c).unwrap()),
                Err(_) => format!("{} [{}]", dom.node_name(c).unwrap(), describe(dom, c).join(", ")),
            })
            .collect()
    }
    let html = Some("http://www.w3.org/1999/xhtml");
    let contexts: &[(Option<&str>, &str)] = &[
        (html, "div"),
        (html, "p"),
        (html, "body"),
        (html, "pre"),
        (html, "td"),
        (html, "tr"),
        (html, "table"),
        (html, "select"),
        (html, "textarea"),
        (html, "title"),
        (html, "style"),
        (html, "script"),
        (html, "noscript"),
        (html, "plaintext"),
        (html, "html"),
        (html, "head"),
        (html, "frameset"),
        (html, "template"),
        (Some("http://www.w3.org/2000/svg"), "svg"),
        (Some("http://www.w3.org/1998/Math/MathML"), "math"),
    ];
    let inputs = [
        "",
        "plain",
        "  ",
        "\n\nx",
        " a b ",
        "a&amp;b",
        "a<b",
        "x\ry",
        "x\r\ny",
        "nul\0",
        "ünï ✓",
        "\u{feff}x",
        "x\u{feff}",
    ];
    for &(ns, name) in contexts {
        for input in inputs {
            let el = dom.create_element_ns(d, ns, name).unwrap();
            set_inner_html(&mut dom, el, "<i>old</i>").unwrap();
            set_inner_html(&mut dom, el, input).unwrap();
            let frag = parse_fragment(&mut dom, el, input).unwrap();
            let got = if name == "template" {
                // The template's contents: what `innerHTML` replaced.
                dom.inner_html(el).unwrap()
            } else {
                describe(&dom, el).join(", ")
            };
            let want = if name == "template" {
                let tmp = dom.create_element(d, "div").unwrap();
                dom.append_child(tmp, frag).unwrap();
                dom.inner_html(tmp).unwrap()
            } else {
                describe(&dom, frag).join(", ")
            };
            assert_eq!(got, want, "<{name}>.innerHTML = {input:?}");
        }
    }
}

/// Collection frees the detached nodes a script made and dropped while the
/// trees it still holds, named by candidates between them, stay whole; a
/// kept tree is freed by a later collection once it is released.
#[test]
fn dropped_nodes_are_freed_between_candidates_of_kept_trees() {
    let mut dom = Dom::new();
    let doc = dom.create_document(DocumentKind::Html, "http://t/");
    dom.set_document_active(doc, true).unwrap();
    let live = dom.live_nodes();

    // Two kept trees: a root with a reference and two children each.
    let mut kept = Vec::new();
    for _ in 0..2 {
        let root = dom.create_element(doc, "div").unwrap();
        dom.add_ref(root).unwrap();
        let mut kids = Vec::new();
        for _ in 0..2 {
            let kid = dom.create_element(doc, "p").unwrap();
            dom.add_ref(kid).unwrap();
            dom.append_child(root, kid).unwrap();
            kids.push(kid);
        }
        kept.push((root, kids));
    }
    // Dropped nodes, each a component of its own (one with a child).
    let mut dropped = Vec::new();
    for i in 0..6 {
        let node =
            if i % 2 == 0 { dom.create_fragment(doc).unwrap() } else { dom.create_element(doc, "span").unwrap() };
        dom.add_ref(node).unwrap();
        dropped.push(node);
    }
    let text = dom.create_text(doc, "t").unwrap();
    dom.append_child(dropped[1], text).unwrap();

    // Candidates of kept and dropped components, interleaved.
    for i in 0..2 {
        dom.release(kept[0].1[i]);
        dom.release(dropped[3 * i]);
        dom.release(kept[1].1[i]);
        dom.release(dropped[3 * i + 1]);
        dom.release(dropped[3 * i + 2]);
    }
    assert_eq!(dom.collect_garbage(), 7, "six dropped nodes and one's text child");
    for &node in &dropped {
        assert!(!dom.is_alive(node), "a dropped node is gone");
    }
    for (root, kids) in &kept {
        assert_eq!(dom.child_count(*root).unwrap(), 2);
        for &kid in kids {
            assert_eq!(dom.parent(kid).unwrap(), Some(*root));
        }
    }
    assert_eq!(dom.live_nodes(), live + 6);

    for (root, _) in &kept {
        dom.release(*root);
    }
    assert_eq!(dom.collect_garbage(), 6);
    assert_eq!(dom.live_nodes(), live);
    assert!(dom.is_alive(doc), "the active document stays");
}

#[test]
fn the_store_gives_back_the_slots_of_a_large_document_that_ended() {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let open = |html: &str| {
        let d = parse_document(&dom, "http://test/", html);
        dom.borrow_mut().set_document_active(d, true).unwrap();
        d
    };
    let close = |d: NodeId| {
        let mut dom = dom.borrow_mut();
        dom.set_document_active(d, false).unwrap();
        dom.collect_garbage();
        assert!(!dom.is_alive(d));
    };
    let large = open(&format!("<body>{}</body>", "<p class=a>row</p>".repeat(6000)));
    let grown = dom.borrow().slot_count();
    assert!(grown > 12000);
    let last = {
        let d = dom.borrow();
        let body = d.body(large).unwrap().unwrap();
        d.last_child(body).unwrap().unwrap()
    };
    // The next document is made while the large one is alive: the store
    // keeps its room while the documents of the last while needed it.
    let second = open("<body><p id=x>second</p></body>");
    close(large);
    assert!(dom.borrow().slot_count() >= grown);
    // Once the documents since the last one ended are small, the free list
    // is remade lowest first: the next document takes the bottom slots, and
    // the store ends after it once the one before is gone.
    let third = open("<body><p id=x>third</p></body>");
    close(second);
    let fourth = open("<body><p id=x>fourth</p></body>");
    close(third);
    let small = dom.borrow().slot_count();
    assert!(small < 64, "{small} slots for one small document");
    assert!(!dom.borrow().is_alive(last));
    // A node made at an index given back is not the node that lived there.
    let mut d = dom.borrow_mut();
    let body = d.body(fourth).unwrap().unwrap();
    for _ in 0..grown {
        let p = d.create_element(fourth, "p").unwrap();
        d.append_child(body, p).unwrap();
    }
    assert!(d.slot_count() > last.index());
    assert!(!d.is_alive(last));
    assert_eq!(d.node(last).unwrap_err(), DomError::StaleNode);
    assert_eq!(d.text_content(d.get_element_by_id(fourth, "x").unwrap().unwrap()).unwrap().unwrap(), "fourth");
}
