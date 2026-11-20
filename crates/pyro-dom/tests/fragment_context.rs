use pyro_dom::parse::{parse_document, set_inner_html, DomRef};
use pyro_dom::select::parse_selectors;
use pyro_dom::{DocumentKind, Dom};

#[test]
fn tokenizer_uses_context_document_scripting() {
    let mut dom = Dom::new();
    let doc = dom.create_document(DocumentKind::Html, "about:blank");
    let context = dom.create_element(doc, "noscript").unwrap();
    for scripting in [false, true] {
        dom.set_document_scripting(doc, scripting).unwrap();
        set_inner_html(&mut dom, context, "<b>x</b>").unwrap();
        assert_eq!(dom.query_selector(context, &parse_selectors("b").unwrap()).unwrap().is_some(), !scripting);
    }
}

#[test]
fn fragment_initializes_form_pointer_from_the_context_tree() {
    let dom = DomRef::default();
    let doc = parse_document(&dom, "about:blank", "<!doctype html><form><div></div></form>");
    let mut dom = dom.borrow_mut();
    let context = dom.query_selector(doc, &parse_selectors("div").unwrap()).unwrap().unwrap();
    set_inner_html(&mut dom, context, "<form><input></form>").unwrap();
    assert_eq!(dom.inner_html(context).unwrap(), "<input>");
}

#[test]
fn table_in_paragraph_inherits_context_document_quirks() {
    for (doctype, expected) in [("", "<p><table></table></p>"), ("<!doctype html>", "<p></p><table></table>")] {
        let dom = DomRef::default();
        let doc = parse_document(&dom, "about:blank", &format!("{doctype}<div></div>"));
        let mut dom = dom.borrow_mut();
        let context = dom.query_selector(doc, &parse_selectors("div").unwrap()).unwrap().unwrap();
        set_inner_html(&mut dom, context, "<p><table></table>").unwrap();
        assert_eq!(dom.inner_html(context).unwrap(), expected);
    }
}
