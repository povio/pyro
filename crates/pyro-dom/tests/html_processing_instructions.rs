//! `<?target data>` in HTML is a processing instruction (vendor/html5ever `pi`; WPT
//! html/syntax/parsing `processing-instructions.dat`, as Chromium): a target of ASCII letters,
//! digits, `-` and `_` that starts with a letter or `_` and not with "xml", then white space and
//! the data, or a `?` that starts it; anything else stays the bogus comment it was. HTML
//! serializes one as `<?target data?>`. A void element's `innerHTML` is empty whatever its
//! children, and `<noscript>` text is not escaped.
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
fn void_elements_serialize_no_children_and_noscript_text_is_raw() {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom, "http://test/", "<body><br><noscript><&></noscript>");
    let mut d = dom.borrow_mut();
    let br = d.query_selector(doc, &parse_selectors("br").unwrap()).unwrap().unwrap();
    let child = d.create_comment(doc, "abc").unwrap();
    d.append_child(br, child).unwrap();
    assert_eq!(d.inner_html(br).unwrap(), "");
    let noscript = d.query_selector(doc, &parse_selectors("noscript").unwrap()).unwrap().unwrap();
    assert_eq!(d.inner_html(noscript).unwrap(), "<&>");
}
