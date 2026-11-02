//! Computed styles (`StyleEngine::stylo_value`, `getComputedStyle`) as the
//! page's Blitz document cascades them: values, inheritance, inline styles,
//! `display: none` subtrees, and restyling after DOM changes.

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::layout::Layout;
use lp_dom::parse::{parse_document, DomRef};
use lp_dom::select::parse_selectors;
use lp_dom::*;

/// Loads nothing (the pages have no sub-resources).
struct NoFetch;
impl lp_render::Fetcher for NoFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err(format!("{url}: offline")));
    }
}

struct Page {
    dom: DomRef,
    doc: NodeId,
    layout: Layout,
}

fn layout_of(dom: &Dom, doc: NodeId) -> Layout {
    lp_layout::install(std::sync::Arc::new(NoFetch));
    let mut layout = Layout::new(doc);
    layout.style_mut().discover_style_elements(dom);
    layout
}

impl Page {
    fn new(html: &str) -> Page {
        let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
        let doc = parse_document(&dom, "http://test/", html);
        dom.borrow_mut().set_document_active(doc, true).unwrap();
        let layout = layout_of(&dom.borrow(), doc);
        Page { dom, doc, layout }
    }
    fn find(&self, css: &str) -> NodeId {
        self.dom.borrow().query_selector(self.doc, &parse_selectors(css).unwrap()).unwrap().unwrap()
    }
    fn value(&self, css: &str, prop: &str) -> String {
        let el = self.find(css);
        self.layout.style().stylo_value(&self.dom.borrow(), el, prop).unwrap_or_else(|| "<none>".into())
    }
}

const PAGE: &str = r#"<!doctype html><html><head><style>
  body { color: rgb(1, 2, 3); font-size: 20px }
  .box { width: 100px; margin: 1em; display: flex }
  .box > p { color: red }
  .on + p { font-weight: bold }
  #hidden { display: none }
</style></head><body>
<div class=box id=b><p id=p1>a</p><p id=p2 class=on>b</p><p id=p3>c</p></div>
<span id=s style="color: rgb(0, 128, 0); padding-left: 2em">s</span>
<div id=hidden><i id=inner>x</i></div>
</body></html>"#;

#[test]
fn values_come_from_stylo() {
    let p = Page::new(PAGE);
    assert_eq!(p.value("#b", "width"), "100px");
    assert_eq!(p.value("#b", "margin-top"), "20px");
    assert_eq!(p.value("#b", "display"), "flex");
    assert_eq!(p.value("#b", "color"), "rgb(1, 2, 3)");
    assert_eq!(p.value("#p1", "color"), "rgb(255, 0, 0)");
    assert_eq!(p.value("#p1", "display"), "block");
    assert_eq!(p.value("#p3", "font-weight"), "700");
    assert_eq!(p.value("#p1", "font-weight"), "400");
    assert_eq!(p.value("#s", "color"), "rgb(0, 128, 0)");
    assert_eq!(p.value("#s", "padding-left"), "40px");
    assert_eq!(p.value("#s", "display"), "inline");
    assert_eq!(p.value("#hidden", "display"), "none");
    assert_eq!(p.value("#inner", "font-style"), "italic");
    assert_eq!(p.value("#inner", "color"), "rgb(1, 2, 3)");
    assert_eq!(p.value("#b", "margin"), "20px");
}

#[test]
fn dom_changes_restyle() {
    let p = Page::new(PAGE);
    assert_eq!(p.value("#p3", "font-weight"), "700");
    // The sibling combinator follows the class.
    let p2 = p.find("#p2");
    p.dom.borrow_mut().set_attribute(p2, "class", "").unwrap();
    assert_eq!(p.value("#p3", "font-weight"), "400");
    let p1 = p.find("#p1");
    p.dom.borrow_mut().set_attribute(p1, "class", "on").unwrap();
    assert_eq!(p.value("#p2", "font-weight"), "700");
    // An inline style change.
    let s = p.find("#s");
    p.dom.borrow_mut().set_attribute(s, "style", "color: blue").unwrap();
    assert_eq!(p.value("#s", "color"), "rgb(0, 0, 255)");
    assert_eq!(p.value("#s", "padding-left"), "0px");
    // An inserted element is styled.
    let b = p.find("#b");
    let q = p.dom.borrow_mut().create_element(p.doc, "p").unwrap();
    p.dom.borrow_mut().set_attribute(q, "id", "p4").unwrap();
    p.dom.borrow_mut().append_child(b, q).unwrap();
    assert_eq!(p.value("#p4", "color"), "rgb(255, 0, 0)");
    // A moved element inherits from its new parent.
    let body = p.dom.borrow().body(p.doc).unwrap().unwrap();
    p.dom.borrow_mut().append_child(body, q).unwrap();
    assert_eq!(p.value("#p4", "color"), "rgb(1, 2, 3)");
    // A shown subtree.
    let hidden = p.find("#hidden");
    p.dom.borrow_mut().set_attribute(hidden, "id", "shown").unwrap();
    assert_eq!(p.value("#shown", "display"), "block");
    assert_eq!(p.value("#inner", "font-style"), "italic");
}

/// Runs `change`, then checks `css`'s `prop` is `want` both incrementally
/// and in a fresh layout (a new Blitz document) of the same DOM.
fn after<R>(p: &Page, change: impl FnOnce(&mut Dom) -> R, css: &str, prop: &str, want: &str) {
    change(&mut p.dom.borrow_mut());
    assert_eq!(p.value(css, prop), want, "{css} {prop} incrementally");
    let fresh = layout_of(&p.dom.borrow(), p.doc);
    let el = p.find(css);
    assert_eq!(fresh.style().stylo_value(&p.dom.borrow(), el, prop).unwrap(), want, "{css} {prop} from scratch");
}

#[test]
fn invalidation_follows_classes_attributes_and_siblings() {
    let p = Page::new(
        "<!doctype html><style>         .on .t{width:1px} [data-k=v] > .t{width:2px} .b ~ .t{width:4px} .a + .t{width:3px}         li:first-child{height:1px} li:nth-child(2n){height:2px} li:last-child{height:3px} ul:empty{height:9px}         </style><body><div id=box><span id=t class=t>t</span></div>         <div id=sib><i id=a1></i><i id=b1></i><i id=t2 class=t></i></div>         <ul id=list><li id=l1>1</li><li id=l2>2</li><li id=l3>3</li></ul><ul id=empty></ul></body>",
    );
    let id = |p: &Page, s: &str| p.find(&format!("#{s}"));
    assert_eq!(p.value("#t", "width"), "auto");
    let bx = id(&p, "box");
    after(&p, |d| d.set_attribute(bx, "class", "on").unwrap(), "#t", "width", "1px");
    after(&p, |d| d.set_attribute(bx, "data-k", "v").unwrap(), "#t", "width", "2px");
    after(&p, |d| d.remove_attribute(bx, "data-k").unwrap(), "#t", "width", "1px");
    after(&p, |d| d.set_attribute(bx, "class", "").unwrap(), "#t", "width", "auto");
    // Sibling combinators: the class arrives on an earlier sibling.
    let (a1, b1) = (id(&p, "a1"), id(&p, "b1"));
    after(&p, |d| d.set_attribute(a1, "class", "b").unwrap(), "#t2", "width", "4px");
    after(&p, |d| d.set_attribute(b1, "class", "a").unwrap(), "#t2", "width", "3px");
    after(&p, |d| d.remove_attribute(b1, "class").unwrap(), "#t2", "width", "4px");
    // Structural pseudo-classes follow insertions and removals.
    assert_eq!(p.value("#l3", "height"), "3px");
    let (list, l1) = (id(&p, "list"), id(&p, "l1"));
    after(&p, |d| d.remove_child(list, l1).unwrap(), "#l2", "height", "1px");
    assert_eq!(p.value("#l3", "height"), "3px");
    let empty = id(&p, "empty");
    assert_eq!(p.value("#empty", "height"), "9px");
    after(&p, |d| d.append_child(empty, l1).unwrap(), "#empty", "height", "auto");
    // Moved back to the end of the list: #l3 is no longer the last child.
    after(&p, |d| d.append_child(list, l1).unwrap(), "#l3", "height", "2px");
    assert_eq!(p.value("#empty", "height"), "9px");
}

#[test]
fn invalidation_follows_state() {
    let p = Page::new(
        "<!doctype html><style>         input:checked + label{color:rgb(1, 1, 1)} input:focus{outline-style:dotted}          form:focus-within{color:rgb(2, 2, 2)} div:hover > span{color:rgb(3, 3, 3)}          input:disabled{opacity:0.5} form:invalid{height:5px}         </style><body><form id=f><input id=c type=checkbox><label id=l>l</label><input id=x required></form>         <div id=h><span id=s>s</span></div></body>",
    );
    let id = |s: &str| p.find(&format!("#{s}"));
    let (c, x, h, doc) = (id("c"), id("x"), id("h"), p.doc);
    after(&p, |d| d.set_checked(c, true).unwrap(), "#l", "color", "rgb(1, 1, 1)");
    after(&p, |d| d.set_checked(c, false).unwrap(), "#l", "color", "rgb(0, 0, 0)");
    after(&p, |d| d.set_focused(doc, Some(x)).map(|_| ()).unwrap(), "#x", "outline-style", "dotted");
    assert_eq!(p.value("#f", "color"), "rgb(2, 2, 2)");
    after(&p, |d| d.set_focused(doc, None).map(|_| ()).unwrap(), "#f", "color", "rgb(0, 0, 0)");
    after(&p, |d| d.set_hovered(doc, Some(h)).unwrap(), "#s", "color", "rgb(3, 3, 3)");
    after(&p, |d| d.set_hovered(doc, None).unwrap(), "#s", "color", "rgb(0, 0, 0)");
    after(&p, |d| d.set_attribute(x, "disabled", "").unwrap(), "#x", "opacity", "0.5");
    assert_eq!(p.value("#f", "height"), "auto", "a disabled control is not validated");
    after(&p, |d| d.remove_attribute(x, "disabled").unwrap(), "#f", "height", "5px");
    after(&p, |d| d.set_value(x, "v").unwrap(), "#f", "height", "auto");
}

#[test]
fn has_follows_attributes_and_state() {
    let p = Page::new(
        "<!doctype html><style>.group:has(input:disabled){color:rgb(255, 0, 0)} \
         .card:has(:invalid){outline:1px solid}</style><body>\
         <div id=g class=group><div><label><input id=i></label></div></div>\
         <div id=c class=card><input id=r required></div></body>",
    );
    assert_eq!(p.value("#g", "color"), "rgb(0, 0, 0)");
    assert_eq!(p.value("#c", "outline-style"), "solid");
    let i = p.find("#i");
    p.dom.borrow_mut().set_attribute(i, "disabled", "").unwrap();
    assert_eq!(p.value("#g", "color"), "rgb(255, 0, 0)");
    let r = p.find("#r");
    p.dom.borrow_mut().set_value(r, "x").unwrap();
    assert_eq!(p.value("#c", "outline-style"), "none");
}

/// getComputedStyle against Chrome: `LP_CS_CASES` names a JSON file of cases
/// (`{name, html, sel, props}` for one element, or `{name, html_file, width,
/// height, stride, props}` for every `stride`-th element of a page) and
/// `LP_CS_CHROME` Chrome's answers (`bench/computed-style/chrome.mjs`).
/// Prints the differences (`LP_CS_VERBOSE=1`: each one), the mismatches per
/// property, and the match rate.
#[test]
#[ignore = "needs Chrome's answers (bench/computed-style)"]
fn chrome_parity() {
    use std::collections::BTreeMap;
    let read = |var: &str| -> serde_json::Value {
        let path = std::env::var(var).unwrap_or_else(|_| panic!("{var}"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    };
    let (cases, chrome) = (read("LP_CS_CASES"), read("LP_CS_CHROME"));
    let verbose = std::env::var_os("LP_CS_VERBOSE").is_some();
    let mut by_prop: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let (mut same, mut total) = (0, 0);
    for (case, answer) in cases.as_array().unwrap().iter().zip(chrome.as_array().unwrap()) {
        let name = case["name"].as_str().unwrap();
        let html = match case["html"].as_str() {
            Some(h) => h.to_owned(),
            None => std::fs::read_to_string(case["html_file"].as_str().unwrap()).unwrap(),
        };
        let (w, h) = (case["width"].as_u64().unwrap_or(800) as u32, case["height"].as_u64().unwrap_or(600) as u32);
        let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
        let doc = parse_document(&dom, "http://test/", &html);
        let dom = Rc::try_unwrap(dom).ok().unwrap().into_inner();
        lp_layout::install(std::sync::Arc::new(NoFetch));
        let mut layout = Layout::with_viewport(doc, lp_dom::media::Viewport::new(w, h));
        layout.style_mut().discover_style_elements(&dom);
        let mut targets = Vec::new();
        if let Some(sel) = case["sel"].as_str() {
            let el = dom.query_selector(doc, &parse_selectors(sel).unwrap()).unwrap();
            targets.extend(el.map(|e| (e, &answer["r"])));
        } else {
            let all = dom.query_selector_all(doc, &parse_selectors("*").unwrap()).unwrap();
            let stride = case["stride"].as_u64().unwrap_or(1) as usize;
            let answers = answer["r"].as_array().unwrap();
            if answers.len() != all.len() {
                println!("{name}: {} elements, Chrome {}", all.len(), answers.len());
            }
            targets.extend(
                all.iter().zip(answers).enumerate().filter(|(i, _)| i % stride == 0).map(|(_, (&e, a))| (e, a)),
            );
        }
        for (el, values) in targets {
            for prop in case["props"].as_array().unwrap() {
                let prop = prop.as_str().unwrap();
                let ours = layout.computed_style_value(&dom, el, prop);
                let theirs = values[prop].as_str().unwrap_or("<none>");
                let tally = by_prop.entry(prop.to_owned()).or_default();
                total += 1;
                tally.1 += 1;
                if ours == theirs {
                    same += 1;
                    tally.0 += 1;
                } else if verbose || case["sel"].is_string() {
                    let tag = dom.tag_name(el).unwrap_or_default();
                    println!("{name} <{tag}> {prop}: lp {ours:?} chrome {theirs:?}");
                }
            }
        }
    }
    for (prop, (ok, n)) in &by_prop {
        if ok != n {
            println!("{prop}: {ok}/{n}");
        }
    }
    println!("{same}/{total} values match Chrome");
}

#[test]
fn media_queries_follow_the_viewport() {
    let mut p = Page::new(
        "<!doctype html><style>#x{height:1px} @media (min-width:500px){#x{height:7px}} .v{width:10vw}</style>\
         <body><div id=x class=v></div></body>",
    );
    assert_eq!(p.value("#x", "height"), "7px");
    assert_eq!(p.value("#x", "width"), "192px");
    p.layout.set_viewport(lp_dom::media::Viewport::new(400, 300));
    assert_eq!(p.value("#x", "height"), "1px");
    assert_eq!(p.value("#x", "width"), "40px");
}

#[test]
fn styles_under_display_none_follow_changes() {
    let p = Page::new(
        "<!doctype html><style>#h{display:none} .on #c{color:rgb(4, 4, 4)} .on + #h #c{width:4px}</style>\
         <body><div id=a></div><div id=h><i id=c>c</i></div></body>",
    );
    assert_eq!(p.value("#c", "color"), "rgb(0, 0, 0)");
    let body = p.dom.borrow().body(p.doc).unwrap().unwrap();
    p.dom.borrow_mut().set_attribute(body, "class", "on").unwrap();
    assert_eq!(p.value("#c", "color"), "rgb(4, 4, 4)");
    let a = p.find("#a");
    p.dom.borrow_mut().set_attribute(a, "class", "on").unwrap();
    assert_eq!(p.value("#c", "width"), "4px");
    p.dom.borrow_mut().set_attribute(a, "class", "").unwrap();
    assert_eq!(p.value("#c", "width"), "auto");
}

#[test]
fn properties_stylo_does_not_parse_still_cascade() {
    // `overflow-anchor`, `text-anchor` and `line-clamp` are Gecko-only in
    // Stylo (lp-dom resolves them, `gecko_only`): specificity, source order,
    // `!important` over inline, `@media`, inheritance only for text-anchor.
    let p = Page::new(
        "<!doctype html><style>\
         #a{overflow-anchor:none} div.x{overflow-anchor:auto} .x{overflow-anchor:none}\
         #i{overflow-anchor:none !important}\
         svg{text-anchor:middle} #t2{text-anchor:end} #t3{text-anchor:inherit}\
         .c{line-clamp:2} @media (max-width:100px){.c{line-clamp:5}} @media (min-width:100px){.d{line-clamp:3}}\
         </style><body>\
         <div id=a class=x>a</div><div id=b class=x>b</div><p id=n class=x>n</p>\
         <div id=i style='overflow-anchor:auto'>i</div><div id=s style='overflow-anchor:none'>s</div>\
         <svg id=svg><g id=g><text id=t1>1</text><text id=t2>2</text></g><g id=g2 style='text-anchor:start'><text id=t3>3</text></g></svg>\
         <div id=c class='c d'><span id=cc>x</span></div></body>",
    );
    let v = |id: &str, prop: &str| p.layout.computed_style_value(&p.dom.borrow(), p.find(&format!("#{id}")), prop);
    assert_eq!(v("a", "overflow-anchor"), "none", "an id beats a class");
    assert_eq!(v("b", "overflow-anchor"), "auto", "more specific wins over a later rule");
    assert_eq!(v("n", "overflow-anchor"), "none");
    assert_eq!(v("i", "overflow-anchor"), "none", "an !important rule beats a normal inline declaration");
    assert_eq!(v("s", "overflow-anchor"), "none");
    assert_eq!(v("cc", "overflow-anchor"), "auto", "not inherited");
    assert_eq!(v("t1", "text-anchor"), "middle", "inherited");
    assert_eq!(v("t2", "text-anchor"), "end");
    assert_eq!(v("t3", "text-anchor"), "start", "inherit takes the parent's");
    assert_eq!(v("c", "line-clamp"), "3", "only matching @media apply");
    assert_eq!(v("cc", "line-clamp"), "none", "not inherited");
}
