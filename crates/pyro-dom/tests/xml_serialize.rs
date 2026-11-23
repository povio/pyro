//! `XMLSerializer` namespace fixup of HTML-parsed foreign content.
use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::select::parse_selectors;
use pyro_dom::*;

/// An `xmlns` attribute written on HTML-parsed `<svg>` is its default namespace declaration, not
/// a prefix `xmlns` (the HTML parser gives it the empty prefix): `XMLSerializer` keeps `<svg>`
/// (WPT css/filter-effects `fecolormatrix-special-value`, which loads the serialized SVG as an
/// image).
#[test]
fn an_html_parsed_xmlns_attribute_declares_the_default_namespace() {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(
        &dom,
        "http://test/",
        r##"<svg viewBox="0 0 1 1" xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><defs><use xlink:href="#a"/></defs></svg>"##,
    );
    let d = dom.borrow();
    let svg = d.query_selector(doc, &parse_selectors("svg").unwrap()).unwrap().unwrap();
    assert_eq!(
        d.xml_serialize(svg).unwrap(),
        r##"<svg viewBox="0 0 1 1" xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><defs><use xlink:href="#a"/></defs></svg>"##
    );
}
