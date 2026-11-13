use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::select::{parse_selectors, SelectorCache};
use pyro_dom::*;

fn doc(html: &str) -> (DomRef, NodeId) {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", html);
    (dom, d)
}

fn ids(dom: &Dom, v: &[NodeId]) -> Vec<String> {
    v.iter().map(|&n| dom.get_attribute(n, "id").unwrap().unwrap_or("").to_owned()).collect()
}

#[test]
fn query_selectors() {
    let (dom, d) = doc(
        r#"<div id=a class="x y"><p id=b class=y>1</p><p id=c>2</p><span id=e data-k="v-1"></span></div><section id=f><p id=g></p></section>"#,
    );
    let dom = dom.borrow();
    let q = |s: &str| ids(&dom, &dom.query_selector_all(d, &parse_selectors(s).unwrap()).unwrap());
    assert_eq!(q("p"), ["b", "c", "g"]);
    assert_eq!(q(".y"), ["a", "b"]);
    assert_eq!(q("div > p:first-child"), ["b"]);
    assert_eq!(q("p:nth-child(2)"), ["c"]);
    assert_eq!(q("[data-k|=v]"), ["e"]);
    assert_eq!(q("div:has(> span)"), ["a"]);
    assert_eq!(q(":is(section, div) p:not(.y)"), ["c", "g"]);
    assert_eq!(q("p:nth-child(1 of .y, #g)"), ["b", "g"]);
    assert_eq!(q("p::before"), Vec::<String>::new());
    assert_eq!(q("P#B"), ["b"], "HTML type selectors are case-insensitive");
    let a = dom.get_element_by_id(d, "a").unwrap().unwrap();
    let sel = parse_selectors(":scope > p").unwrap();
    assert_eq!(ids(&dom, &dom.query_selector_all(a, &sel).unwrap()), ["b", "c"]);
    let b = dom.get_element_by_id(d, "b").unwrap().unwrap();
    assert_eq!(dom.closest(b, &parse_selectors("div").unwrap()).unwrap(), Some(a));
    assert!(dom.matches(b, &parse_selectors("#a .y").unwrap()).unwrap());
    let e = parse_selectors("p[").unwrap_err();
    assert_eq!(e.kind(), Some(DomErrorKind::Syntax));
    assert!(parse_selectors(":hover-nope").is_err());
    let mut cache = SelectorCache::default();
    assert!(cache.get("p").is_ok());
    assert!(cache.get("p").is_ok());
}

#[test]
fn form_state_and_pseudo_classes() {
    let (dom, d) = doc(r#"<form id=f>
        <input id=t value="init" required>
        <input id=cb type=checkbox checked>
        <input id=r1 type=radio name=g checked><input id=r2 type=radio name=g>
        <fieldset disabled><legend><input id=inlegend></legend><input id=dis></fieldset>
        <select id=s><option id=o1 value=a>A</option><option id=o2>  B  text </option></select>
        <textarea id=ta>hello</textarea>
        <input id=num type=number value="12x">
    </form>"#);
    let mut dom = dom.borrow_mut();
    let g = |dom: &Dom, id: &str| dom.get_element_by_id(d, id).unwrap().unwrap();
    let t = g(&dom, "t");
    assert_eq!(dom.value(t).unwrap(), "init");
    dom.set_value(t, "new\nline").unwrap();
    assert_eq!(dom.value(t).unwrap(), "newline");
    assert_eq!(dom.get_attribute(t, "value").unwrap(), Some("init"), "value attr is the default value");
    let num = g(&dom, "num");
    assert_eq!(dom.value(num).unwrap(), "", "number sanitization");

    let cb = g(&dom, "cb");
    assert!(dom.checked(cb).unwrap());
    dom.set_checked(cb, false).unwrap();
    assert!(!dom.checked(cb).unwrap());
    assert_eq!(dom.value(cb).unwrap(), "on");

    let r1 = g(&dom, "r1");
    let r2 = g(&dom, "r2");
    dom.set_checked(r2, true).unwrap();
    assert!(!dom.checked(r1).unwrap() && dom.checked(r2).unwrap(), "radio group exclusivity");

    assert!(!dom.is_disabled(g(&dom, "inlegend")).unwrap());
    assert!(dom.is_disabled(g(&dom, "dis")).unwrap());

    let s = g(&dom, "s");
    assert_eq!(dom.value(s).unwrap(), "a", "first option selected by default");
    assert_eq!(dom.selected_index(s).unwrap(), 0);
    dom.set_value(s, "B text").unwrap();
    assert_eq!(dom.selected_index(s).unwrap(), 1);
    assert!(dom.option_selected(g(&dom, "o2")).unwrap());

    let ta = g(&dom, "ta");
    assert_eq!(dom.value(ta).unwrap(), "hello");

    let q = |dom: &Dom, s: &str| ids(dom, &dom.query_selector_all(d, &parse_selectors(s).unwrap()).unwrap());
    assert_eq!(q(&dom, "input:checked"), ["r2"]);
    assert_eq!(q(&dom, "input:disabled"), ["dis"]);
    assert_eq!(q(&dom, ":required"), ["t"]);
    assert_eq!(q(&dom, "option:checked"), ["o2"]);
    assert!(q(&dom, "input:enabled").contains(&"t".to_string()));

    // Focus state.
    dom.set_focused(d, Some(t)).unwrap();
    assert_eq!(q(&dom, ":focus"), ["t"]);
    assert_eq!(q(&dom, "form:focus-within"), ["f"]);
    assert!(dom.is_focusable(t).unwrap());
    assert!(!dom.is_focusable(g(&dom, "dis")).unwrap());
}

#[test]
fn text_editing() {
    let (dom, d) = doc(
        r#"<input id=t value="abc"><textarea id=ta></textarea><input id=ro readonly value=x><input id=m maxlength=3>"#,
    );
    let mut dom = dom.borrow_mut();
    let t = dom.get_element_by_id(d, "t").unwrap().unwrap();
    assert_eq!(dom.selection(t).unwrap(), (3, 3));
    assert!(dom.insert_text(t, "d").unwrap());
    assert_eq!(dom.value(t).unwrap(), "abcd");
    dom.set_selection_range(t, 0, 4).unwrap();
    // TOS fillInput clear path: select all, insert "".
    assert!(dom.insert_text(t, "").unwrap());
    assert_eq!(dom.value(t).unwrap(), "");
    assert!(!dom.delete_backward(t).unwrap());
    dom.insert_text(t, "x😀y").unwrap();
    assert_eq!(dom.selection(t).unwrap(), (4, 4));
    dom.set_selection_range(t, 3, 3).unwrap();
    assert!(dom.delete_backward(t).unwrap());
    assert_eq!(dom.value(t).unwrap(), "xy");
    assert_eq!(dom.selection(t).unwrap(), (1, 1));
    assert!(dom.delete_forward(t).unwrap());
    assert_eq!(dom.value(t).unwrap(), "x");
    let ta = dom.get_element_by_id(d, "ta").unwrap().unwrap();
    dom.insert_text(ta, "a\nb").unwrap();
    assert_eq!(dom.value(ta).unwrap(), "a\nb");
    let ro = dom.get_element_by_id(d, "ro").unwrap().unwrap();
    assert!(!dom.insert_text(ro, "z").unwrap());
    let m = dom.get_element_by_id(d, "m").unwrap().unwrap();
    dom.insert_text(m, "abc").unwrap();
    assert!(!dom.insert_text(m, "d").unwrap(), "maxlength");
}

#[test]
fn constraint_validation() {
    let (dom, d) =
        doc(r#"<form id=f><input id=r required><input id=e type=email value="x"><input id=n type=number min=3 value=1>
        <input id=h type=hidden required><input id=ok value=fine required></form>"#);
    let mut dom = dom.borrow_mut();
    let g = |dom: &Dom, id: &str| dom.get_element_by_id(d, id).unwrap().unwrap();
    assert!(dom.validity(g(&dom, "r")).unwrap().value_missing);
    assert!(dom.validity(g(&dom, "e")).unwrap().type_mismatch);
    assert!(dom.validity(g(&dom, "n")).unwrap().range_underflow);
    assert!(!dom.will_validate(g(&dom, "h")).unwrap());
    assert!(dom.validity(g(&dom, "ok")).unwrap().valid());
    let q = |dom: &Dom, s: &str| ids(dom, &dom.query_selector_all(d, &parse_selectors(s).unwrap()).unwrap());
    assert_eq!(q(&dom, "input:invalid"), ["r", "e", "n"]);
    assert_eq!(q(&dom, "form:invalid"), ["f"]);
    let ok = g(&dom, "ok");
    dom.set_custom_validity(ok, "bad").unwrap();
    assert!(dom.validity(ok).unwrap().custom_error);
    let r = g(&dom, "r");
    dom.set_value(r, "x").unwrap();
    assert!(dom.validity(r).unwrap().valid());
}

#[test]
fn class_matching_tracks_mutations_and_documents() {
    // Class tokens are cached per element for one (DOM, version): a class
    // change, and a second document with the same node indices, must not
    // reuse stale tokens.
    let (r, d) = doc("<div id=a class='x  big y'><span id=s class='x'></span></div>");
    let mut dom = r.borrow_mut();
    let a = dom.get_element_by_id(d, "a").unwrap().unwrap();
    let m = |dom: &Dom, el: NodeId, s: &str| dom.matches(el, &parse_selectors(s).unwrap()).unwrap();
    assert!(m(&dom, a, ".big") && m(&dom, a, ".x.y") && !m(&dom, a, ".bi") && !m(&dom, a, ".bigger"));
    let s = dom.get_element_by_id(d, "s").unwrap().unwrap();
    assert!(m(&dom, s, ".big .x") && m(&dom, s, ".y > .x"));
    dom.set_attribute(a, "class", "z").unwrap();
    assert!(!m(&dom, a, ".big") && m(&dom, a, ".z") && !m(&dom, s, ".big .x") && m(&dom, s, ".z .x"));
    dom.remove_attribute(a, "class").unwrap();
    assert!(!m(&dom, a, ".z"));
    drop(dom);
    // Another document whose nodes reuse the same indices.
    let (r2, d2) = doc("<div id=a class='q'><span id=s class='x'></span></div>");
    let dom2 = r2.borrow();
    let a2 = dom2.get_element_by_id(d2, "a").unwrap().unwrap();
    assert!(m(&dom2, a2, ".q") && !m(&dom2, a2, ".z"));
    let dom = r.borrow();
    assert!(m(&dom, s, ".x") && !m(&dom, a, ".q"));
}
