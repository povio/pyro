//! `Dom::style_text_revision`: moves whenever an HTML `<style>`'s child text
//! content may have changed, and only then; the style engine's author-sheets
//! key and sheet revisions follow it.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::parse::{parse_document, DocumentParser, DomRef};
use pyro_dom::select::parse_selectors;
use pyro_dom::snapshot::{render_key, DeltaOp, RenderHtml, RenderNodeKind, SYNTHETIC_TEXT};
use pyro_dom::style::StyleEngine;
use pyro_dom::*;

fn doc(html: &str) -> (DomRef, NodeId) {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", html);
    (dom, d)
}

fn find(dom: &Dom, d: NodeId, css: &str) -> NodeId {
    dom.query_selector(d, &parse_selectors(css).unwrap()).unwrap().unwrap()
}

/// Runs `f` and says whether the DOM-wide revision and `style`'s own
/// revision moved.
fn moved(dom: &DomRef, style: NodeId, f: impl FnOnce(&mut Dom)) -> (bool, bool) {
    let (rev, own) = {
        let dom = dom.borrow();
        (dom.style_text_revision(), dom.style_element_text_revision(style).unwrap())
    };
    f(&mut dom.borrow_mut());
    let dom = dom.borrow();
    (dom.style_text_revision() != rev, dom.style_element_text_revision(style).unwrap() != own)
}

#[test]
fn the_revision_moves_on_every_change_of_a_style_child_text() {
    let (dom, d) = doc("<head><style id=s>p{}</style></head><body><p id=p>x</p></body>");
    let s = find(&dom.borrow(), d, "#s");
    // textContent=
    assert_eq!(moved(&dom, s, |dom| dom.set_text_content(s, "a{}").unwrap()), (true, true));
    // appendChild(text)
    assert_eq!(
        moved(&dom, s, |dom| {
            let t = dom.create_text(d, "b{}").unwrap();
            dom.append_child(s, t).unwrap();
        }),
        (true, true)
    );
    let first = dom.borrow().first_child(s).unwrap().unwrap();
    // .data and appendData
    assert_eq!(moved(&dom, s, |dom| dom.set_char_data(first, "c{}").unwrap()), (true, true));
    assert_eq!(moved(&dom, s, |dom| dom.replace_data(first, 3, 0, "d{}").unwrap()), (true, true));
    assert_eq!(dom.borrow().child_text_content(s), "c{}d{}b{}");
    // Moving a child text out, then removing one.
    let p = find(&dom.borrow(), d, "#p");
    assert_eq!(
        moved(&dom, s, |dom| {
            dom.append_child(p, first).unwrap();
        }),
        (true, true)
    );
    let last = dom.borrow().first_child(s).unwrap().unwrap();
    assert_eq!(
        moved(&dom, s, |dom| {
            dom.remove_child(s, last).unwrap();
        }),
        (true, true)
    );
    assert_eq!(dom.borrow().child_text_content(s), "");
    // Moving a text node in.
    assert_eq!(
        moved(&dom, s, |dom| {
            dom.append_child(s, first).unwrap();
        }),
        (true, true)
    );
    assert_eq!(dom.borrow().child_text_content(s), "c{}d{}");
}

#[test]
fn the_revision_stays_across_changes_outside_style_children() {
    let (dom, d) = doc("<head><style id=s>p{}</style></head><body id=b></body>");
    let s = find(&dom.borrow(), d, "#s");
    let b = find(&dom.borrow(), d, "#b");
    let moves = moved(&dom, s, |dom| {
        for i in 0..1000 {
            let div = dom.create_element(d, "div").unwrap();
            dom.append_child(b, div).unwrap();
            let t = dom.create_text(d, "text").unwrap();
            dom.append_child(div, t).unwrap();
            dom.set_attribute(div, "class", &format!("c{i}")).unwrap();
            dom.set_char_data(t, "other").unwrap();
        }
        dom.set_attribute(s, "media", "print").unwrap();
    });
    assert_eq!(moves, (false, false));
}

#[test]
fn a_streaming_chunk_that_splits_a_style_body_moves_the_revision() {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = dom.borrow_mut().create_document(DocumentKind::Html, "http://t/");
    let mut p = DocumentParser::new(dom.clone(), d, false).unwrap();
    p.feed("<head><style id=s>p { color:");
    let s = find(&dom.borrow(), d, "#s");
    let before = (dom.borrow().style_text_revision(), dom.borrow().style_element_text_revision(s).unwrap());
    assert_ne!(before.1, 0, "the first chunk's text moved it");
    p.feed(" red }</style></head><body>");
    p.end();
    let dom = dom.borrow();
    assert_eq!(dom.child_text_content(s), "p { color: red }");
    assert_ne!(dom.style_text_revision(), before.0);
    assert_ne!(dom.style_element_text_revision(s).unwrap(), before.1);
}

#[test]
fn alternate_link_enabling_belongs_to_the_original_element() {
    let (dom, document) = doc("<link id=s title=alt rel='ALTERNATE StyleSheet' href='s.css'>");
    let link = find(&dom.borrow(), document, "#s");
    let mut style = StyleEngine::new(document);
    style.set_sheet_text(link, "a{}");
    assert!(!dom.borrow_mut().remove_attribute(link, "disabled").unwrap());
    assert_eq!(style.sheet_text(&dom.borrow(), link), Some(""));
    dom.borrow_mut().set_attribute(link, "disabled", "").unwrap();
    dom.borrow_mut().remove_attribute(link, "disabled").unwrap();
    assert_eq!(style.sheet_text(&dom.borrow(), link), Some("a{}"));
    let copy = dom.borrow_mut().clone_node(link, false, None).unwrap();
    let head = dom.borrow().parent(link).unwrap().unwrap();
    dom.borrow_mut().append_child(head, copy).unwrap();
    style.set_sheet_text(copy, "a{}");
    assert_eq!(style.sheet_text(&dom.borrow(), copy), Some(""));
    assert_eq!(style.sheet_own_text(copy), Some("a{}"));
}

#[test]
fn preferred_sheet_name_survives_removal_of_its_first_owner() {
    let (dom, document) = doc("<style title=A>a{}</style><style title=B>b{}</style><style title=A>c{}</style>");
    let first = find(&dom.borrow(), document, "style");
    let head = dom.borrow().parent(first).unwrap().unwrap();
    dom.borrow_mut().remove_child(head, first).unwrap();
    let mut style = StyleEngine::new(document);
    style.discover_style_elements(&dom.borrow());
    let mut texts = Vec::new();
    style.for_each_author_sheet(&dom.borrow(), &mut |text| texts.push(text.to_owned()));
    assert_eq!(texts, ["c{}"]);
}

#[test]
fn link_sheet_revision_tracks_whether_the_loaded_text_applies() {
    let (dom, document) = doc("<link id=sheet rel=stylesheet href='sheet.css'>");
    let link = find(&dom.borrow(), document, "#sheet");
    let mut style = StyleEngine::new(document);
    style.set_sheet_text(link, "p{}");
    let enabled = style.sheet_revision(&dom.borrow(), link);
    for (attribute, value) in [("disabled", ""), ("rel", "icon")] {
        dom.borrow_mut().set_attribute(link, attribute, value).unwrap();
        assert_eq!(style.sheet_text(&dom.borrow(), link), Some(""));
        assert_ne!(style.sheet_revision(&dom.borrow(), link), enabled);
        assert_eq!(style.sheet_own_text(link), Some("p{}"));
        dom.borrow_mut().remove_attribute(link, attribute).unwrap();
        dom.borrow_mut().set_attribute(link, "rel", "stylesheet").unwrap();
        assert_eq!(style.sheet_text(&dom.borrow(), link), Some("p{}"));
        assert_eq!(style.sheet_revision(&dom.borrow(), link), enabled);
    }
}

#[test]
fn the_sheets_key_follows_the_style_text_and_connected_owners() {
    let (dom, d) = doc("<head><style id=s>p{}</style></head><body id=b></body>");
    let s = find(&dom.borrow(), d, "#s");
    let b = find(&dom.borrow(), d, "#b");
    let mut style = StyleEngine::new(d);
    style.discover_style_elements(&dom.borrow());
    let key = style.author_sheets_key(&dom.borrow());
    let rev = style.sheet_revision(&dom.borrow(), s).unwrap();
    // Non-style mutations: same key.
    for _ in 0..100 {
        let div = dom.borrow_mut().create_element(d, "div").unwrap();
        dom.borrow_mut().append_child(b, div).unwrap();
    }
    assert_eq!(style.author_sheets_key(&dom.borrow()), key);
    assert_eq!(style.sheet_revision(&dom.borrow(), s).unwrap(), rev);
    // The text changes: both move.
    dom.borrow_mut().set_text_content(s, "a{}").unwrap();
    let key2 = style.author_sheets_key(&dom.borrow());
    assert_ne!(key2, key);
    assert_ne!(style.sheet_revision(&dom.borrow(), s).unwrap(), rev);
    // Disconnecting the owner: the key moves, the owner's revision does not.
    let rev2 = style.sheet_revision(&dom.borrow(), s).unwrap();
    let head = dom.borrow().parent(s).unwrap().unwrap();
    dom.borrow_mut().remove_child(head, s).unwrap();
    assert_ne!(style.author_sheets_key(&dom.borrow()), key2);
    assert_eq!(style.sheet_revision(&dom.borrow(), s).unwrap(), rev2);
    let mut texts = Vec::new();
    style.for_each_author_sheet(&dom.borrow(), &mut |t| texts.push(t.to_owned()));
    assert!(texts.is_empty());
}

#[test]
fn a_style_sheet_text_is_its_child_text_content() {
    // An element inside a `<style>` (script can put one there): its text is
    // not part of the sheet, for the engine and the renderer alike.
    let (dom, d) = doc("<head><style id=s>p{}</style></head><body></body>");
    let s = find(&dom.borrow(), d, "#s");
    let opts = RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None };
    let (em, inner) = {
        let mut dom = dom.borrow_mut();
        let em = dom.create_element(d, "em").unwrap();
        let inner = dom.create_text(d, "q{}").unwrap();
        dom.append_child(em, inner).unwrap();
        dom.append_child(s, em).unwrap();
        let tail = dom.create_text(d, "a{}").unwrap();
        dom.append_child(s, tail).unwrap();
        (em, inner)
    };
    let mut style = StyleEngine::new(d);
    style.discover_style_elements(&dom.borrow());
    let mut texts = Vec::new();
    style.for_each_author_sheet(&dom.borrow(), &mut |t| texts.push(t.to_owned()));
    assert_eq!(texts, ["p{}a{}"]);
    let synthetic = |tree: &pyro_dom::snapshot::RenderTree| {
        tree.nodes.iter().find(|n| n.key == render_key(s) | SYNTHETIC_TEXT).map(|n| n.kind.clone())
    };
    let tree = dom.borrow().render_tree(d, &opts).unwrap();
    assert!(matches!(synthetic(&tree), Some(RenderNodeKind::Text(t)) if t == "p{}a{}"));
    assert!(matches!(dom.borrow().render_style_text(s, &opts),
        Some(DeltaOp::Node { kind: RenderNodeKind::Text(t), .. }) if t == "p{}a{}"));
    // A change below the nested element: the sheet text and the key stay,
    // and the delta has nothing to replace.
    let key = style.author_sheets_key(&dom.borrow());
    let version = dom.borrow().version();
    dom.borrow_mut().set_char_data(inner, "r{}").unwrap();
    let k = dom.borrow_mut().create_text(d, "z").unwrap();
    dom.borrow_mut().append_child(em, k).unwrap();
    assert_eq!(style.author_sheets_key(&dom.borrow()), key);
    let delta = dom.borrow().render_delta(d, version, &opts).expect("incremental");
    assert!(delta.ops.is_empty(), "{}", delta.ops.len());
    // A child's data: the sheet text is replaced.
    let first = dom.borrow().first_child(s).unwrap().unwrap();
    let version = dom.borrow().version();
    dom.borrow_mut().set_char_data(first, "b{}").unwrap();
    assert_ne!(style.author_sheets_key(&dom.borrow()), key);
    let delta = dom.borrow().render_delta(d, version, &opts).expect("incremental");
    assert!(
        delta.ops.iter().any(|op| matches!(op, DeltaOp::Node { key, kind: RenderNodeKind::Text(t) }
            if *key == render_key(s) | SYNTHETIC_TEXT && t == "b{}a{}")),
        "{}",
        delta.ops.len()
    );
}
