//! Ports of upstream's innerText tests (lightpanda-io/browser @ 2ad38773):
//! tests/element/inner.html (#ids, #innerText) and
//! tests/css/text_transform.html (#innerText, #dynamic).

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::layout::Layout;
use pyro_dom::parse::{parse_document, set_inner_html, DomRef};
use pyro_dom::style::StyleEngine;
use pyro_dom::*;

/// Loads nothing (the pages have no sub-resources).
struct NoFetch;
impl pyro_render::Fetcher for NoFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err(format!("{url}: offline")));
    }
}

/// The page's style engine, with its Blitz document (computed styles).
struct Styles(Layout);
impl std::ops::Deref for Styles {
    type Target = StyleEngine;
    fn deref(&self) -> &StyleEngine {
        self.0.style()
    }
}

fn page(html: &str) -> (Dom, NodeId, Styles) {
    pyro_layout::install(std::sync::Arc::new(NoFetch));
    let dom_ref: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom_ref, "http://test/", html);
    let dom = Rc::try_unwrap(dom_ref).ok().expect("sole owner").into_inner();
    let mut layout = Layout::new(doc);
    layout.style_mut().discover_style_elements(&dom);
    (dom, doc, Styles(layout))
}

fn by_id(dom: &Dom, doc: NodeId, id: &str) -> Option<NodeId> {
    dom.get_element_by_id(doc, id).unwrap()
}

const INNER: &str = r#"<!DOCTYPE html>
<script src="../testing.js"></script>
<div id=d1>hello <em>world</em></div>
<div id=d2>
  <style>h1 { font-size: 1em; }</style>
  <!-- this is a comment -->
  This is a <br>
  text
</div>
<p id=d3><span>Hello</span> <span>World</span></p>
<p id=d4><span>Hello</span>
  <span>World</span></p>

<style>.ihide { display: none }</style>
<div id=d5>a<span class=ihide>X</span>b<span style="display:none">Y</span>c</div>
<div id=d6>before<div class=ihide>deep <span>nested</span></div>after</div>
"#;

// tests/element/inner.html <script id=ids>
#[test]
fn inner_text_ids() {
    let (mut dom, doc, style) = page(INNER);
    let d1 = by_id(&dom, doc, "d1").unwrap();
    set_inner_html(&mut dom, d1, "<a id=link>home</a>").unwrap();
    let link = by_id(&dom, doc, "link").unwrap();
    assert_eq!(dom.inner_text(&style, link).unwrap(), "home");
    set_inner_html(&mut dom, d1, "").unwrap();
    assert_eq!(by_id(&dom, doc, "link"), None);
    set_inner_html(&mut dom, d1, "<div><p><a id=link>hi</a></p></div>").unwrap();
    let link = by_id(&dom, doc, "link").unwrap();
    assert_eq!(dom.inner_text(&style, link).unwrap(), "hi");
}

// tests/element/inner.html <script id=innerText>
#[test]
fn inner_text_getter_and_setter() {
    let (mut dom, doc, style) = page(INNER);
    let d1 = by_id(&dom, doc, "d1").unwrap();
    let it = |dom: &Dom, el: NodeId| dom.inner_text(&style, el).unwrap();

    set_inner_html(&mut dom, d1, "hello <em>world</em>").unwrap();
    assert_eq!(it(&dom, d1), "hello world");

    dom.set_inner_text(d1, "only text").unwrap();
    assert_eq!(it(&dom, d1), "only text");
    assert_eq!(dom.inner_html(d1).unwrap(), "only text");

    dom.set_inner_text(d1, "hello <div>world</div><b>!!</b>").unwrap();
    assert_eq!(it(&dom, d1), "hello <div>world</div><b>!!</b>");
    assert_eq!(dom.inner_html(d1).unwrap(), "hello &lt;div&gt;world&lt;/div&gt;&lt;b&gt;!!&lt;/b&gt;");

    dom.set_inner_text(d1, "").unwrap();
    assert_eq!(it(&dom, d1), "");
    assert_eq!(dom.inner_html(d1).unwrap(), "");

    set_inner_html(&mut dom, d1, "<div>hello <span>beautiful</span> world</div>").unwrap();
    assert_eq!(it(&dom, d1), "hello beautiful world");

    set_inner_html(&mut dom, d1, "<div><p><a id=link2>hi</a></p></div>").unwrap();
    assert_eq!(it(&dom, d1), "hi");
    dom.set_inner_text(d1, "new content").unwrap();
    assert_eq!(it(&dom, d1), "new content");
    assert_eq!(by_id(&dom, doc, "link2"), None);

    assert_eq!(it(&dom, by_id(&dom, doc, "d2").unwrap()), "This is a\ntext");
    assert_eq!(it(&dom, by_id(&dom, doc, "d3").unwrap()), "Hello World");
    assert_eq!(it(&dom, by_id(&dom, doc, "d4").unwrap()), "Hello World");
    assert_eq!(it(&dom, by_id(&dom, doc, "d5").unwrap()), "abc");
    assert_eq!(it(&dom, by_id(&dom, doc, "d6").unwrap()), "beforeafter");

    set_inner_html(&mut dom, d1, r#"<div id=dn style="display:none">a<span>b</span>c</div>"#).unwrap();
    assert_eq!(it(&dom, by_id(&dom, doc, "dn").unwrap()), "abc");

    set_inner_html(&mut dom, d1, r#"a<br style="display:none">b"#).unwrap();
    assert_eq!(it(&dom, d1), "ab");

    set_inner_html(&mut dom, d1, r#"<span style="visibility:hidden">X</span>Y"#).unwrap();
    assert_eq!(it(&dom, d1), "Y");

    let div = dom.create_element(doc, "div").unwrap();
    set_inner_html(&mut dom, div, "abc <span style=display:none> 123 </span> xyz").unwrap();
    let span = dom.query_selector(div, &pyro_dom::select::parse_selectors("span").unwrap()).unwrap().unwrap();
    assert_eq!(it(&dom, div), "abc  123  xyz");
    assert_eq!(it(&dom, span), " 123 ");
    let html = dom.document_element(doc).unwrap().unwrap();
    dom.append_child(html, div).unwrap();
    assert_eq!(it(&dom, div), "abc xyz");
    assert_eq!(it(&dom, span), " 123 ");
}

#[test]
fn rendered_text_fragment_breaks() {
    // renderedTextFragment (Html.zig:1074-1096): one <br> per break, CRLF once.
    let (mut dom, doc, _style) = page(INNER);
    let d1 = by_id(&dom, doc, "d1").unwrap();
    dom.set_inner_text(d1, "a\r\nb\n\nc\rd").unwrap();
    assert_eq!(dom.inner_html(d1).unwrap(), "a<br>b<br><br>c<br>d");
}

#[test]
fn outer_text_merges_neighbours() {
    // setOuterText (Html.zig:366-405)
    let (mut dom, doc, style) = page(INNER);
    let d1 = by_id(&dom, doc, "d1").unwrap();
    set_inner_html(&mut dom, d1, "x<b id=b>bold</b>y").unwrap();
    let b = by_id(&dom, doc, "b").unwrap();
    dom.set_outer_text(b, "Q").unwrap();
    assert_eq!(dom.inner_html(d1).unwrap(), "xQy");
    assert_eq!(dom.children(d1).unwrap().count(), 1);
    assert_eq!(dom.inner_text(&style, d1).unwrap(), "xQy");
    set_inner_html(&mut dom, d1, "x<b id=b>bold</b>y").unwrap();
    let b = by_id(&dom, doc, "b").unwrap();
    dom.set_outer_text(b, "1\n2").unwrap();
    assert_eq!(dom.inner_html(d1).unwrap(), "x1<br>2y");
    let detached = dom.create_element(doc, "p").unwrap();
    assert_eq!(dom.set_outer_text(detached, "z").unwrap_err().kind(), Some(DomErrorKind::NoModificationAllowed));
}

const TEXT_TRANSFORM: &str = r#"<!DOCTYPE html>
<head><style>
  .upper { text-transform: uppercase; }
  .lower { text-transform: lowercase; }
  .cap { text-transform: capitalize; }
  .reset { text-transform: none; }
  .combo { text-transform: uppercase full-width; }
  #id-wins { text-transform: lowercase; }
  .bogus { text-transform: shout; }
  .two-cases { text-transform: uppercase lowercase; }
</style></head>
<span id=inline style="text-transform: uppercase">unavailable</span>
<span id=class class=upper>unavailable</span>
<div class=upper><span id=inherited>unavailable</span></div>
<div class=upper><span id=reset class=reset>Mixed Case</span></div>
<span id=lower class=lower>UNAVAILABLE</span>
<span id=cap class=cap>hello wORLD, it's  é-mail 1st</span>
<span id=id-wins class=upper>Mixed</span>
<span id=combo class=combo>wide</span>
<div class=upper><span id=bogus class=bogus>still upper</span></div>
<div class=upper><span id=two-cases class=two-cases>still upper</span></div>
<div class=upper><span id=inline-inherit style="text-transform: inherit">kept</span></div>
<div class=upper><span id=inline-initial style="text-transform: initial">plain</span></div>
<span id=plain>Plain Text</span>
<div id=mixed>a <b class=upper>bold</b> <i class=cap>two words</i> z</div>
<div id=nested class=cap><span>one</span><span class=lower>TWO</span> three</div>
"#;

// tests/css/text_transform.html <script id=innerText> and <script id=dynamic>
#[test]
fn inner_text_applies_text_transform() {
    let (mut dom, doc, style) = page(TEXT_TRANSFORM);
    let it = |dom: &Dom, id: &str| dom.inner_text(&style, by_id(dom, doc, id).unwrap()).unwrap();
    assert_eq!(it(&dom, "inline"), "UNAVAILABLE");
    assert_eq!(it(&dom, "class"), "UNAVAILABLE");
    assert_eq!(it(&dom, "inherited"), "UNAVAILABLE");
    assert_eq!(it(&dom, "reset"), "Mixed Case");
    assert_eq!(it(&dom, "lower"), "unavailable");
    assert_eq!(it(&dom, "cap"), "hello wORLD, it's é-mail 1st");
    assert_eq!(it(&dom, "id-wins"), "mixed");
    assert_eq!(it(&dom, "combo"), "WIDE");
    assert_eq!(it(&dom, "bogus"), "STILL UPPER");
    assert_eq!(it(&dom, "two-cases"), "STILL UPPER");
    assert_eq!(it(&dom, "inline-inherit"), "KEPT");
    assert_eq!(it(&dom, "inline-initial"), "plain");
    assert_eq!(it(&dom, "plain"), "Plain Text");
    assert_eq!(it(&dom, "mixed"), "a BOLD two words z");
    assert_eq!(it(&dom, "nested"), "onetwo three");
    let class = by_id(&dom, doc, "class").unwrap();
    assert_eq!(dom.text_content(class).unwrap().unwrap(), "unavailable");

    // dynamic
    let plain = by_id(&dom, doc, "plain").unwrap();
    dom.set_attribute(plain, "class", "upper").unwrap();
    assert_eq!(it(&dom, "plain"), "PLAIN TEXT");
    dom.set_attribute(plain, "style", "text-transform: lowercase").unwrap();
    assert_eq!(it(&dom, "plain"), "plain text");
}

#[test]
fn inner_text_blocks_tables_and_pre() {
    let (dom, doc, style) = page(
        "<!DOCTYPE html><body><div id=r><p>one</p><p>two</p><div>three</div>\
         <table><tr><td>a</td> <td>b</td></tr>\n<tr><td>c</td><td>d</td></tr></table>\
         <pre>x\r\n  y</pre><select><option>o1</option>junk<option>o2</option></select>\
         <img alt=i><script>s()</script></div></body>",
    );
    let r = by_id(&dom, doc, "r").unwrap();
    assert_eq!(dom.inner_text(&style, r).unwrap(), "one\n\ntwo\n\nthree\na\tb\nc\td\nx\n  y\no1\no2");
}
