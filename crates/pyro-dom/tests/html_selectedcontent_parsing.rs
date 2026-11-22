//! The HTML parser runs "maybe clone an option into selectedcontent" for each option it pops off
//! the stack of open elements, closed by an end tag, by another option or at the end of the input
//! (WPT html/syntax/parsing `webkit02.dat`): the selected option's children are cloned into the
//! select's first selectedcontent, unless the select is multiple or the selectedcontent is in a
//! nested select.
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
fn parser_clones_the_selected_option_into_selectedcontent() {
    for (source, want) in [
        (
            "<select><button><selectedcontent></button><option>X",
            "<select><button><selectedcontent>X</selectedcontent></button><option>X</option></select>",
        ),
        (
            "<select><button><selectedcontent></button><option>x<i>i<b>ib</i>b",
            "<select><button><selectedcontent>x<i>i<b>ib</b></i><b>b</b></selectedcontent></button>\
             <option>x<i>i<b>ib</b></i><b>b</b></option></select>",
        ),
        (
            "<select><button><selectedcontent></button><option>X<option>Y",
            "<select><button><selectedcontent>X</selectedcontent></button><option>X</option><option>Y</option></select>",
        ),
        (
            "<select><button><selectedcontent></button><option>X<option selected>Y",
            "<select><button><selectedcontent>Y</selectedcontent></button>\
             <option>X</option><option selected=\"\">Y</option></select>",
        ),
        // An explicit end tag pops the option once.
        (
            "<select><button><selectedcontent></button><option>A</option><option>B</option></select>",
            "<select><button><selectedcontent>A</selectedcontent></button><option>A</option><option>B</option></select>",
        ),
        // A multiple select has no enabled selectedcontent.
        (
            "<select multiple><button><selectedcontent></button><option selected>X</option></select>",
            "<select multiple=\"\"><button><selectedcontent></selectedcontent></button>\
             <option selected=\"\">X</option></select>",
        ),
    ] {
        assert_eq!(body_html(source), want, "{source}");
    }
}
