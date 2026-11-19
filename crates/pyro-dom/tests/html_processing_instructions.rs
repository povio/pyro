//! `<?target data>` in HTML is a processing instruction (vendor/html5ever `pi`; WPT
//! html/syntax/parsing `processing-instructions.dat`, as Chromium): a target of ASCII letters,
//! digits, `-` and `_` that starts with a letter or `_` and not with "xml", then white space and
//! the data, or a `?` that starts it; anything else stays the bogus comment it was. HTML
//! serializes one as `<?target data?>`. A void element's `innerHTML` is empty whatever its
//! children; `<noscript>` text is not escaped where scripting is enabled for the node.
use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::select::parse_selectors;
use pyro_dom::*;

fn body_html(html: &str) -> String {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom, "http://test/", html);
    let d = dom.borrow();
    let body = d.query_selector(doc, &parse_selectors("body").unwrap()).unwrap().unwrap();
    d.inner_html(body).unwrap()
}

#[test]
fn question_mark_markup_is_a_processing_instruction_when_its_target_is_one() {
    for (source, want) in [
        ("<?something>", "<?something ?>"),
        ("<?something else is good>", "<?something else is good?>"),
        ("<?good?>", "<?good ?>"),
        ("<?hey   there?>", "<?hey there?>"),
        ("<?hey?there>", "<?hey ?there?>"),
        ("<?something ? >", "<?something ? ?>"),
        ("<?t d > ?>", "<?t d ?> ?&gt;"),
        ("<?_-_ data><span></span>", "<?_-_ data?><span></span>"),
        // Not a target: the bogus comment it was.
        ("<?a$>", "<!--?a$-->"),
        ("<?xml-stylesheet href=a>", "<!--?xml-stylesheet href=a-->"),
        ("<?1st-place>", "<!--?1st-place-->"),
        ("<? >", "<!--? -->"),
        // A real comment whose text starts with `?` stays one.
        ("<!--?something-->", "<!--?something-->"),
        // The input ends in one: nothing; in a bogus comment: the comment.
        ("<?start data", ""),
        ("<?", ""),
        ("<? ", "<!--? -->"),
        // Raw text elements have none.
        ("<textarea><?pi></textarea>", "<textarea>&lt;?pi&gt;</textarea>"),
        ("<table><tr><?pi></tr></table>", "<table><tbody><tr><?pi ?></tr></tbody></table>"),
    ] {
        assert_eq!(body_html(&format!("<body>{source}")), want, "{source}");
    }
}

#[test]
fn void_elements_serialize_no_children() {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom, "http://test/", "<body><br>");
    let mut d = dom.borrow_mut();
    let br = d.query_selector(doc, &parse_selectors("br").unwrap()).unwrap().unwrap();
    let child = d.create_comment(doc, "abc").unwrap();
    d.append_child(br, child).unwrap();
    assert_eq!(d.inner_html(br).unwrap(), "");
}

/// HTML fragment serialization: `noscript` text is raw only "if scripting is
/// enabled for the node", which a document without a browsing context
/// (DOMParser, XHR, createHTMLDocument) and template contents never have
/// (WPT html/syntax/serializing-html-fragments/escaping.html,
/// html/syntax/parsing/noscript-template-querySelector.html).
#[test]
fn noscript_text_is_raw_only_where_scripting_is_enabled() {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom, "http://test/", "<body><div></div><template></template>");
    let mut d = dom.borrow_mut();
    let body = d.query_selector(doc, &parse_selectors("body").unwrap()).unwrap().unwrap();
    let noscript = d.create_element(doc, "noscript").unwrap();
    let text = d.create_text(doc, "&\u{a0}<>").unwrap();
    d.append_child(noscript, text).unwrap();
    d.append_child(body, noscript).unwrap();
    // A document without a browsing context: escaped, and the fragment
    // parser makes elements of `noscript` content.
    assert!(!d.document_scripting(doc).unwrap());
    assert_eq!(d.inner_html(noscript).unwrap(), "&amp;&nbsp;&lt;&gt;");
    let div = d.query_selector(doc, &parse_selectors("div").unwrap()).unwrap().unwrap();
    pyro_dom::parse::set_inner_html(&mut d, div, "<noscript><p>x</p></noscript>").unwrap();
    assert!(d.query_selector(div, &parse_selectors("noscript > p").unwrap()).unwrap().is_some());
    assert_eq!(d.inner_html(div).unwrap(), "<noscript><p>x</p></noscript>");

    // The document of a browsing context that runs scripts: raw text, both ways.
    d.set_document_scripting(doc, true).unwrap();
    assert_eq!(d.inner_html(noscript).unwrap(), "&\u{a0}<>");
    pyro_dom::parse::set_inner_html(&mut d, div, "<noscript><p>x</p></noscript>").unwrap();
    assert!(d.query_selector(div, &parse_selectors("noscript > p").unwrap()).unwrap().is_none());
    assert_eq!(d.inner_html(div).unwrap(), "<noscript><p>x</p></noscript>");
    // Template contents live in an inert document: parsed with the
    // template's document's flag (text), serialized escaped.
    let template = d.query_selector(doc, &parse_selectors("template").unwrap()).unwrap().unwrap();
    pyro_dom::parse::set_inner_html(&mut d, template, "<noscript><div></div></noscript>").unwrap();
    assert_eq!(d.inner_html(template).unwrap(), "<noscript>&lt;div&gt;&lt;/div&gt;</noscript>");
}
