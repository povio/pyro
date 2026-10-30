//! The Stylo cascade over lp-dom (`StyleEngine::stylo_value`): values,
//! inheritance, inline styles, `display: none` subtrees, and restyling after
//! DOM changes.

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::layout::Layout;
use lp_dom::parse::{parse_document, DomRef};
use lp_dom::select::parse_selectors;
use lp_dom::*;

struct Page {
    dom: DomRef,
    doc: NodeId,
    layout: Layout,
}

impl Page {
    fn new(html: &str) -> Page {
        let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
        let doc = parse_document(&dom, "http://test/", html);
        dom.borrow_mut().set_document_active(doc, true).unwrap();
        let mut layout = Layout::new(doc);
        layout.style_mut().discover_style_elements(&dom.borrow());
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

#[test]
fn a_local_change_restyles_locally() {
    if !lp_dom::style::stylo_cascade() {
        return; // Stylo's bookkeeping
    }
    let rows: String = (0..500).map(|i| format!("<li class=r{i}>row {i}</li>")).collect();
    let p = Page::new(&format!(
        "<!doctype html><style>.x {{ color: red }}</style><ul id=a>{rows}</ul><ul id=b><li id=one>x</li></ul>"
    ));
    p.layout.style().style_document(&p.dom.borrow());
    let (styled, ..) = p.layout.style().stylo_stats();
    assert!(styled > 500, "{styled}");
    let one = p.find("#one");
    p.dom.borrow_mut().set_attribute(one, "class", "x").unwrap();
    assert_eq!(p.value("#one", "color"), "rgb(255, 0, 0)");
    let (_, restyled, visited) = p.layout.style().stylo_stats();
    assert_eq!(restyled, 1, "only #one's values changed");
    assert!(visited <= 4, "#one and the path to it: {visited}");
}

/// Runs `change`, then checks `css`'s `prop` is `want` both incrementally
/// and in a fresh cascade of the same DOM.
fn after<R>(p: &Page, change: impl FnOnce(&mut Dom) -> R, css: &str, prop: &str, want: &str) {
    change(&mut p.dom.borrow_mut());
    assert_eq!(p.value(css, prop), want, "{css} {prop} incrementally");
    let mut fresh = Layout::new(p.doc);
    fresh.style_mut().discover_style_elements(&p.dom.borrow());
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
fn dom_queries_go_through_stylo() {
    use lp_dom::select::parse_query;
    let p = Page::new(
        "<!doctype html><body><div id=a class='x y'><p id=p1>1</p><p id=p2 class=x>2</p></div>\
         <section id=s><div id=b><input id=i disabled></div></section></body>",
    );
    let dom = p.dom.borrow();
    let ids = |v: Vec<NodeId>| {
        v.into_iter().map(|n| dom.get_attribute(n, "id").unwrap().unwrap().to_owned()).collect::<Vec<_>>()
    };
    let all = |root: NodeId, s: &str| ids(dom.query_all(root, &parse_query(s).unwrap()).unwrap());
    assert_eq!(all(p.doc, ".x"), ["a", "p2"]);
    assert_eq!(all(p.doc, "div:has(> p.x) p"), ["p1", "p2"]);
    assert_eq!(all(p.doc, "p:nth-child(2 of .x, p)"), ["p2"]);
    assert_eq!(all(p.doc, "input:disabled, #p1"), ["p1", "i"]);
    let s = p.find("#s");
    assert_eq!(all(s, ":scope > div input"), ["i"]);
    assert_eq!(all(s, "body div"), ["b"], "ancestors outside the root still match");
    let first = dom.query_first(p.doc, &parse_query("p").unwrap()).unwrap();
    assert_eq!(first, Some(p.find("#p1")));
    let i = p.find("#i");
    assert!(dom.element_matches(i, &parse_query("section input:disabled").unwrap()).unwrap());
    assert_eq!(dom.element_closest(i, &parse_query("section, div").unwrap()).unwrap(), Some(p.find("#b")));
    assert!(parse_query("p:::nope").is_err());
}

/// Heap bytes in use (macOS malloc zones).
#[cfg(target_os = "macos")]
fn heap_in_use() -> usize {
    #[repr(C)]
    #[derive(Default)]
    struct Stats {
        blocks_in_use: u32,
        size_in_use: usize,
        max_size_in_use: usize,
        size_allocated: usize,
    }
    extern "C" {
        fn malloc_zone_statistics(zone: *mut std::ffi::c_void, stats: *mut Stats);
    }
    let mut s = Stats::default();
    // SAFETY: a null zone sums every zone into `s`.
    unsafe { malloc_zone_statistics(std::ptr::null_mut(), &mut s) };
    s.size_in_use
}

/// Style memory per page: heap growth from the first computed-style read
/// (cascade data, per-element style data, computed values), and timings.
/// `LP_CS_PAGES` lists HTML files (`file:width:height`); run with
/// `--test-threads=1` and compare with `LP_CASCADE=legacy`.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "needs pages (LP_CS_PAGES)"]
fn memory_per_page() {
    let pages = std::env::var("LP_CS_PAGES").expect("LP_CS_PAGES");
    for spec in pages.split(',') {
        let mut parts = spec.split(':');
        let file = parts.next().unwrap();
        let w = parts.next().map_or(1280, |v| v.parse().unwrap());
        let h = parts.next().map_or(720, |v| v.parse().unwrap());
        let html = std::fs::read_to_string(file).unwrap();
        let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
        let doc = parse_document(&dom, "http://test/", &html);
        let dom = Rc::try_unwrap(dom).ok().unwrap().into_inner();
        let mut layout = Layout::with_viewport(doc, lp_dom::media::Viewport::new(w, h));
        layout.style_mut().discover_style_elements(&dom);
        let elements = dom.query_selector_all(doc, &parse_selectors("*").unwrap()).unwrap();
        let before = heap_in_use();
        let t = std::time::Instant::now();
        let body = dom.body(doc).unwrap().unwrap();
        layout.computed_style_value(&dom, body, "color");
        let first = t.elapsed();
        let after_first = heap_in_use();
        let t = std::time::Instant::now();
        for &e in &elements {
            layout.computed_style_value(&dom, e, "display");
        }
        let all = t.elapsed();
        let after_all = heap_in_use();
        let name = std::path::Path::new(file).file_stem().unwrap().to_string_lossy();
        println!(
            "{name}: {} elements; first style {:.1} ms, +{} KiB; every element's display {:.1} ms, +{} KiB total ({} B/element); styled {:?}",
            elements.len(),
            first.as_secs_f64() * 1e3,
            (after_first.saturating_sub(before)) / 1024,
            all.as_secs_f64() * 1e3,
            (after_all.saturating_sub(before)) / 1024,
            after_all.saturating_sub(before) / elements.len().max(1),
            layout.style().stylo_stats().0,
        );
    }
}

#[test]
fn changes_restyle_locally_with_has_and_sibling_rules() {
    if !lp_dom::style::stylo_cascade() {
        return; // Stylo's bookkeeping
    }
    let rows: String = (0..300).map(|i| format!("<li class=r{i}><span>row {i}</span></li>")).collect();
    let p = Page::new(&format!(
        "<!doctype html><style>.card:has(.on){{outline-style:dotted}} li:nth-child(2n){{width:2px}} \
         .x + li{{height:3px}} .card .on{{color:rgb(1, 1, 1)}}</style>\
         <div class=card id=card><ul id=list>{rows}</ul></div><p id=far>far</p>"
    ));
    let visited = |p: &Page| p.layout.style().stylo_stats().2;
    assert_eq!(p.value("#card", "outline-style"), "none");
    // A class on one row: that row, the `:has()` anchor, the path down.
    let row = p.find(".r150");
    p.dom.borrow_mut().set_attribute(row, "class", "r150 on").unwrap();
    assert_eq!(p.value("#card", "outline-style"), "dotted");
    assert_eq!(p.value(".r150", "color"), "rgb(1, 1, 1)");
    assert!(visited(&p) < 40, "visited {}", visited(&p));
    // A sibling-combinator class: the row and the one after.
    let row = p.find(".r10");
    p.dom.borrow_mut().set_attribute(row, "class", "r10 x").unwrap();
    assert_eq!(p.value(".r11", "height"), "3px");
    assert!(visited(&p) < 40, "visited {}", visited(&p));
    // An element far from any anchor.
    let far = p.find("#far");
    p.dom.borrow_mut().set_attribute(far, "title", "t").unwrap();
    p.value("#far", "color");
    assert!(visited(&p) < 10, "visited {}", visited(&p));
}

#[test]
fn only_attributes_selectors_test_are_snapshotted() {
    if !lp_dom::style::stylo_cascade() {
        return; // Stylo's bookkeeping
    }
    let p = Page::new(
        "<!doctype html><style>[data-on] > .x{color:rgb(1, 1, 1)} .big{width:9px} [hidden]{display:none}</style>\
         <body><div id=d><span id=x class=x>x</span></div></body>",
    );
    let d = p.find("#d");
    let snapshots = |p: &Page| p.dom.borrow().style_snapshot_count();
    let visited = |p: &Page| p.layout.style().stylo_stats().2;
    p.layout.style().style_document(&p.dom.borrow());
    assert_eq!(p.value("#x", "color"), "rgb(0, 0, 0)");
    // No selector tests these: no snapshot, and the next read restyles nothing.
    p.dom.borrow_mut().set_attribute(d, "data-other", "1").unwrap();
    p.dom.borrow_mut().set_attribute(d, "aria-label", "l").unwrap();
    assert_eq!(snapshots(&p), 0);
    assert_eq!(p.value("#x", "color"), "rgb(0, 0, 0)");
    assert_eq!(visited(&p), 0);
    // A non-selector attribute that is not data-/aria-: the element itself only.
    p.dom.borrow_mut().set_attribute(d, "accesskey", "k").unwrap();
    assert_eq!(snapshots(&p), 0);
    assert_eq!(p.value("#x", "color"), "rgb(0, 0, 0)");
    assert!(visited(&p) <= 3, "visited {}", visited(&p));
    // Attributes selectors test: snapshotted once per element until the next read.
    p.dom.borrow_mut().set_attribute(d, "data-on", "").unwrap();
    p.dom.borrow_mut().set_attribute(d, "data-on", "again").unwrap();
    p.dom.borrow_mut().set_attribute(d, "class", "big").unwrap();
    assert_eq!(snapshots(&p), 1);
    assert_eq!(p.value("#x", "color"), "rgb(1, 1, 1)");
    assert_eq!(p.value("#d", "width"), "9px");
    assert_eq!(snapshots(&p), 0);
    p.dom.borrow_mut().remove_attribute(d, "data-on").unwrap();
    assert_eq!(p.value("#x", "color"), "rgb(0, 0, 0)");
    // A style attribute (no selector reads it) still restyles the element.
    p.dom.borrow_mut().set_attribute(d, "style", "width: 3px").unwrap();
    assert_eq!(snapshots(&p), 0);
    assert_eq!(p.value("#d", "width"), "3px");
    // Detached elements are not snapshotted.
    let fresh = p.dom.borrow_mut().create_element(p.doc, "div").unwrap();
    p.dom.borrow_mut().set_attribute(fresh, "class", "big").unwrap();
    assert_eq!(snapshots(&p), 0);
}

/// In-process timings of the style reads dom-bench measures over CDP:
/// `LP_CS_PAGES` as for `memory_per_page`; `LP_CS_ITERS` repetitions
/// (default 200). Prints the median and the 10th-90th percentile spread
/// of each operation, per page. Compare with `LP_CASCADE=legacy`.
#[test]
#[ignore = "needs pages (LP_CS_PAGES)"]
fn style_read_timings() {
    use lp_dom::layout::Geometry;
    let pages = std::env::var("LP_CS_PAGES").expect("LP_CS_PAGES");
    let iters: usize = std::env::var("LP_CS_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(200);
    for spec in pages.split(',') {
        let file = spec.split(':').next().unwrap();
        let html = std::fs::read_to_string(file).unwrap();
        let load = || {
            let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
            let doc = parse_document(&dom, "http://test/", &html);
            let dom = Rc::try_unwrap(dom).ok().unwrap().into_inner();
            let mut layout = Layout::with_viewport(doc, lp_dom::media::Viewport::new(1280, 720));
            layout.style_mut().discover_style_elements(&dom);
            layout.set_geometry(Geometry::Flow);
            (dom, doc, layout)
        };
        let stats = |mut v: Vec<f64>| {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let at = |q: f64| v[((v.len() - 1) as f64 * q) as usize];
            format!("{:>9.1} us [{:.1}-{:.1}]", at(0.5), at(0.1), at(0.9))
        };
        let name = std::path::Path::new(file).file_stem().unwrap().to_string_lossy().into_owned();
        // First read after load: one getComputedStyle, one visibility check.
        let mut first_cs = Vec::new();
        let mut first_vis = Vec::new();
        for _ in 0..iters.min(std::env::var("LP_CS_FIRST").ok().and_then(|v| v.parse().ok()).unwrap_or(50)) {
            let (dom, doc, layout) = load();
            let body = dom.body(doc).unwrap().unwrap();
            let t = std::time::Instant::now();
            layout.computed_style_value(&dom, body, "display");
            first_cs.push(t.elapsed().as_secs_f64() * 1e6);
            let (dom, doc, layout) = load();
            let body = dom.body(doc).unwrap().unwrap();
            let t = std::time::Instant::now();
            layout.is_visible(&dom, body);
            first_vis.push(t.elapsed().as_secs_f64() * 1e6);
        }
        let (mut dom, doc, layout) = load();
        let all = dom.query_selector_all(doc, &parse_selectors("*").unwrap()).unwrap();
        let step = (all.len() / 100).max(1);
        let hundred: Vec<NodeId> = all.iter().step_by(step).take(100).copied().collect();
        let row = dom
            .query_selector(doc, &parse_selectors("tbody tr").unwrap())
            .unwrap()
            .or_else(|| dom.query_selector(doc, &parse_selectors("li, p").unwrap()).unwrap())
            .unwrap_or_else(|| dom.body(doc).unwrap().unwrap());
        let body = dom.body(doc).unwrap().unwrap();
        let run = |dom: &mut Dom, target: NodeId, read: &dyn Fn(&Dom)| {
            let mut v = Vec::new();
            for i in 0..iters {
                let class = if i % 2 == 0 { "bench-dirty" } else { "" };
                dom.set_attribute(target, "class", class).unwrap();
                let t = std::time::Instant::now();
                read(dom);
                v.push(t.elapsed().as_secs_f64() * 1e6);
            }
            stats(v)
        };
        let cs = |dom: &Dom| {
            for &e in &hundred {
                std::hint::black_box(layout.computed_style_value(dom, e, "color"));
            }
        };
        let rect = |dom: &Dom| {
            for &e in &hundred {
                std::hint::black_box(layout.bounding_client_rect(dom, e));
            }
        };
        layout.bounding_client_rect(&dom, body);
        let row_cs = run(&mut dom, row, &cs);
        let row_rect = run(&mut dom, row, &rect);
        let body_cs = run(&mut dom, body, &cs);
        let body_rect = run(&mut dom, body, &rect);
        let mut clean = Vec::new();
        for _ in 0..iters {
            let t = std::time::Instant::now();
            cs(&dom);
            clean.push(t.elapsed().as_secs_f64() * 1e6);
        }
        let query = lp_dom::select::parse_query("div span, tbody tr td, li a").unwrap();
        let mut q = Vec::new();
        for _ in 0..iters {
            let t = std::time::Instant::now();
            std::hint::black_box(dom.query_all(doc, &query).unwrap());
            q.push(t.elapsed().as_secs_f64() * 1e6);
        }
        // DOM writes after the page's styles were read (snapshots on).
        let mut ins = Vec::new();
        let mut attrs = Vec::new();
        for _ in 0..iters.min(100) {
            let holder = dom.create_element(doc, "div").unwrap();
            dom.append_child(body, holder).unwrap();
            let t = std::time::Instant::now();
            for _ in 0..1000 {
                let span = dom.create_element(doc, "span").unwrap();
                let first = dom.first_child(holder).unwrap();
                dom.insert_before(holder, span, first).unwrap();
            }
            ins.push(t.elapsed().as_secs_f64() * 1e6);
            let kids: Vec<NodeId> = dom.children(holder).unwrap().collect();
            let t = std::time::Instant::now();
            for (i, &k) in kids.iter().enumerate() {
                dom.set_attribute(k, "data-bench", &i.to_string()).unwrap();
                dom.set_attribute(k, "class", if i % 2 == 0 { "a" } else { "b" }).unwrap();
            }
            attrs.push(t.elapsed().as_secs_f64() * 1e6);
            dom.remove_child(body, holder).unwrap();
            cs(&dom);
        }
        println!("{name} ({} elements)", all.len());
        println!("  insertBefore 1000 after reads      {}", stats(ins));
        println!("  setAttribute x2000 after reads     {}", stats(attrs));
        println!("  first getComputedStyle after load {}", stats(first_cs));
        println!("  first visibility check after load {}", stats(first_vis));
        println!("  getComputedStyle x100 clean       {}", stats(clean));
        println!("  getComputedStyle x100 row changed {row_cs}");
        println!("  rect x100 row changed             {row_rect}");
        println!("  getComputedStyle x100 body class  {body_cs}");
        println!("  rect x100 body class              {body_rect}");
        println!("  css query (descendant)            {}", stats(q));
    }
}

#[test]
fn styles_under_display_none_follow_changes() {
    let p = Page::new(
        "<!doctype html><style>#h{display:none} .on #c{color:rgb(4, 4, 4)} .on + #h #c{width:4px}</style>\
         <body><div id=a></div><div id=h><i id=c>c</i></div></body>",
    );
    p.layout.style().style_document(&p.dom.borrow());
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
