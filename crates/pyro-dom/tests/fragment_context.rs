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
fn template_fragment_keeps_forms_in_body_and_table_modes() {
    let dom = DomRef::default();
    let doc = parse_document(&dom, "about:blank", "<!doctype html><form><template></template></form>");
    let mut dom = dom.borrow_mut();
    let template = dom.query_selector(doc, &parse_selectors("template").unwrap()).unwrap().unwrap();
    let contents = dom.template_contents(template).unwrap().unwrap();
    set_inner_html(&mut dom, template, "<div><form></div><input></form><table><form></form></table>").unwrap();
    assert_eq!(dom.query_selector_all(contents, &parse_selectors("form").unwrap()).unwrap().len(), 2);
    let input = dom.query_selector(contents, &parse_selectors("input").unwrap()).unwrap().unwrap();
    assert_eq!(dom.form_owner(input).unwrap(), None);
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

#[test]
fn explicit_template_end_removes_patch_markers_but_eof_preserves_them() {
    let mut dom = Dom::new();
    let doc = dom.create_document(DocumentKind::Html, "about:blank");
    let context = dom.create_element(doc, "div").unwrap();
    let markup = "<div><?start name='a'><i>old</i><?end></div><template for=a><b>new";
    set_inner_html(&mut dom, context, markup).unwrap();
    let result = dom.inner_html(context).unwrap();
    assert!(result.contains("<?start"), "{result}");
    assert!(result.contains("<?end"), "{result}");
    assert!(result.contains("<b>new</b>"), "{result}");
    assert!(!result.contains("old") && !result.contains("<template"), "{result}");

    set_inner_html(&mut dom, context, &format!("{markup}</template>")).unwrap();
    assert_eq!(dom.inner_html(context).unwrap(), "<div><b>new</b></div>");
}

#[test]
fn sibling_patches_preserve_the_template_contents_scope() {
    let mut dom = Dom::new();
    let doc = dom.create_document(DocumentKind::Html, "about:blank");
    let context = dom.create_element(doc, "div").unwrap();
    let markup = concat!(
        "<p><?marker name='a'></p><template><i><?marker name='a'></i>",
        "<template for=a>inside</template></template><template for=a>outside</template>",
    );
    set_inner_html(&mut dom, context, markup).unwrap();
    assert_eq!(dom.inner_html(context).unwrap(), "<p>outside</p><template><i>inside</i></template>");
}

#[test]
fn range_editing_and_external_fragment_templates_keep_their_contents() {
    use pyro_dom::parse::{parse_contextual_fragment, parse_fragment};

    let mut dom = Dom::new();
    let doc = dom.create_document(DocumentKind::Html, "about:blank");
    let context = dom.create_element(doc, "div").unwrap();
    let fragment =
        parse_contextual_fragment(&mut dom, context, "<p><?marker name='a'></p><template for=a>range</template>")
            .unwrap();
    assert!(dom.query_selector(fragment, &parse_selectors("template[for]").unwrap()).unwrap().is_some());

    let editing_fragment =
        parse_fragment(&mut dom, context, "<p><?marker name='a'></p><template for=a>editing</template>").unwrap();
    assert!(dom.query_selector(editing_fragment, &parse_selectors("template[for]").unwrap()).unwrap().is_some());

    set_inner_html(&mut dom, context, "<p><?marker name='a'></p><template for=a src='part.html'>fallback</template>")
        .unwrap();
    assert!(dom.query_selector(context, &parse_selectors("template[src]").unwrap()).unwrap().is_some());
}

#[test]
fn xml_document_setter_preserves_inline_template_markers() {
    let mut dom = Dom::new();
    let doc = dom.create_document(DocumentKind::Xml, "about:blank");
    let context = dom.create_element_ns(doc, Some("http://www.w3.org/1999/xhtml"), "div").unwrap();
    set_inner_html(&mut dom, context, "<p><?marker name='a'></p><template for='a'>xml</template>").unwrap();
    assert!(dom.query_selector(context, &parse_selectors("template[for]").unwrap()).unwrap().is_some());
}
