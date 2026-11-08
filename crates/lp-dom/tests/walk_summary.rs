//! Review tests for the dense walk array and element summaries
//! (`Dom::walk`, `select_query` filters) and the text-only `innerHTML` path:
//! randomized mutation sequences over every tree and attribute path, with
//! compiled queries checked against `selectors` matching after each step.
//! Run in release too (`cargo test --release`): debug builds also assert the
//! walk entry on every read, release builds only compare results.

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::mutation::{MutationRecord, ObserveInit};
use lp_dom::parse::{parse_document, parse_fragment, set_inner_html, DomRef};
use lp_dom::select::{parse_query, parse_selectors};
use lp_dom::*;
use markup5ever::{ns, LocalName, QualName};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len())]
    }
}

const HTML: Option<&str> = Some("http://www.w3.org/1999/xhtml");
const SVG: Option<&str> = Some("http://www.w3.org/2000/svg");
const MATH: Option<&str> = Some("http://www.w3.org/1998/Math/MathML");

/// Compiled selectors (type, class, id, attribute, descendant/child).
const COMPILED: &[&str] = &[
    "*",
    "div",
    "span",
    "p",
    "DIV",
    "Div",
    "rect",
    "foreignObject",
    "foreignobject",
    "my-el",
    "MY-EL",
    "x-Up",
    ".a",
    ".A",
    ".b",
    ".a.b",
    ".a.b.c",
    ".k0.k39",
    ".k17",
    "div.a",
    "span.b",
    "my-el.a",
    "rect.a",
    "#i1",
    "#i1.a",
    "div .a",
    "div > .b",
    ".a .b",
    ".a > .b",
    ".a .b .c",
    "p span.a",
    "* > .a",
    "div, .b",
    ".a, span.b",
    "[class]",
    "[class~=a]",
    "[CLASS]",
    "div > span > .c",
    "section .a > span",
];

/// Selectors the compiled path does not take (still compared: they share
/// the tree walks' inputs through `selectors`).
const OTHER: &[&str] = &[":is(.a) .b", ":where(div) > .a", ".a:not(.b)", "div ~ .a", "div + span", ":scope > .a"];

const CLASS_VALUES: &[&str] = &[
    "a",
    "A",
    "a b",
    "b a c",
    "a\tb",
    "a\nb\nc",
    "a\x0cb",
    "a\rb",
    " a  a ",
    "\x0c\x0ca\r\n",
    "",
    "   ",
    "c",
    "aa",
    "a\u{a0}b",
    "B C",
    "k0 k1 k2 k3 k4 k5 k6 k7 k8 k9 k10 k11 k12 k13 k14 k15 k16 k17 k18 k19 k20 k39",
];

fn elements(dom: &Dom, root: NodeId) -> Vec<NodeId> {
    dom.descendants(root).unwrap().filter(|&n| dom.node_type(n).unwrap() == NodeType::Element).collect()
}

/// Compiled `querySelectorAll`/`querySelector`/`matches`/`closest` against
/// `selectors` matching each element (only `sample` elements for the
/// per-element calls).
fn agree(dom: &Dom, root: NodeId, css: &str, step: usize, rng: &mut Rng) {
    let q = parse_query(css).unwrap();
    let sel = parse_selectors(css).unwrap();
    let els = elements(dom, root);
    let want: Vec<NodeId> = els.iter().copied().filter(|&n| n != root && dom.matches(n, &sel).unwrap()).collect();
    assert_eq!(dom.query_all(root, &q).unwrap(), want, "step {step}: querySelectorAll({css}) on {root:?}");
    assert_eq!(dom.query_first(root, &q).unwrap(), want.first().copied(), "step {step}: querySelector({css})");
    for _ in 0..4.min(els.len()) {
        let n = *rng.pick(&els);
        assert_eq!(dom.element_matches(n, &q).unwrap(), dom.matches(n, &sel).unwrap(), "step {step}: matches({css})");
        assert_eq!(dom.element_closest(n, &q).unwrap(), dom.closest(n, &sel).unwrap(), "step {step}: closest({css})");
    }
}

fn alive(dom: &Dom, n: NodeId) -> bool {
    dom.node_type(n).is_ok()
}

fn run_random(seed: u64, steps: usize) {
    let mut rng = Rng(seed);
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let quirks_doc = seed % 2 == 0;
    let src = "<div id=i1 class=a><span class=b>t</span><p class='a b'><span class='c a'>x</span></p></div>\
               <section class=a><span class=b><i class=c></i></span></section><svg><rect class=a /><foreignObject class=b><div class=c></div></foreignObject></svg>";
    let d = parse_document(
        &dom,
        "http://test/",
        &if quirks_doc { src.to_string() } else { format!("<!doctype html>{src}") },
    );
    let mut dom = dom.borrow_mut();
    dom.set_document_active(d, true).unwrap();
    // A second HTML document (createHTMLDocument) and an XML one (DOMParser).
    let d2 = dom.create_document(DocumentKind::Html, "about:blank");
    let h2 = dom.create_element(d2, "html").unwrap();
    dom.append_child(d2, h2).unwrap();
    let x = lp_dom::xml::parse_document(
        &mut dom,
        "http://test/x.xml",
        "<root xmlns='http://www.w3.org/1999/xhtml'><DIV class='a'><div class='b'><x-Up class='a b'/></div></DIV><svg xmlns='http://www.w3.org/2000/svg'><rect class='a'/></svg></root>",
        "application/xml",
    );
    let docs = [d, d2, x];
    // Held, so garbage collection keeps them (and frees the rest).
    for doc in docs {
        dom.set_document_active(doc, true).unwrap();
    }
    let mut pool: Vec<NodeId> = docs.iter().flat_map(|&doc| dom.descendants(doc).unwrap().collect::<Vec<_>>()).collect();
    pool.extend(docs);

    let names: &[(Option<&str>, &str)] = &[
        (HTML, "div"),
        (HTML, "span"),
        (HTML, "p"),
        (HTML, "section"),
        (HTML, "DIV"),
        (HTML, "my-el"),
        (HTML, "x-Up"),
        (HTML, "template"),
        (HTML, "textarea"),
        (HTML, "option"),
        (SVG, "rect"),
        (SVG, "foreignObject"),
        (MATH, "mi"),
        (Some("urn:other"), "div"),
    ];
    for step in 0..steps {
        pool.retain(|&n| alive(&dom, n));
        let any = |rng: &mut Rng, pool: &[NodeId]| *rng.pick(pool);
        let els: Vec<NodeId> =
            pool.iter().copied().filter(|&n| dom.node_type(n).unwrap() == NodeType::Element).collect();
        if els.is_empty() {
            pool.push(dom.create_element(d, "div").unwrap());
            continue;
        }
        let el = |rng: &mut Rng| *rng.pick(&els);
        match rng.below(26) {
            0 | 1 => {
                let doc = *rng.pick(&docs);
                let (ns, name) = *rng.pick(names);
                if let Ok(e) = dom.create_element_ns(doc, ns, name) {
                    if rng.below(2) == 0 {
                        let _ = dom.set_attribute(e, "class", rng.pick(CLASS_VALUES));
                    }
                    pool.push(e);
                    if let Ok(Some(c)) = dom.template_contents(e) {
                        pool.push(c);
                    }
                }
            }
            2 => {
                let doc = *rng.pick(&docs);
                if let Ok(t) = dom.create_text(doc, "txt") {
                    pool.push(t);
                }
                if let Ok(c) = dom.create_comment(doc, "c") {
                    pool.push(c);
                }
            }
            3..=5 => {
                let (p, n) = (any(&mut rng, &pool), any(&mut rng, &pool));
                let _ = dom.append_child(p, n);
            }
            6 => {
                let (p, n) = (any(&mut rng, &pool), any(&mut rng, &pool));
                let kids: Vec<NodeId> = dom.children(p).map(|c| c.collect()).unwrap_or_default();
                let before = if kids.is_empty() { None } else { Some(*rng.pick(&kids)) };
                let _ = dom.insert_before(p, n, before);
            }
            7 => {
                let n = any(&mut rng, &pool);
                let _ = dom.remove_node(n);
            }
            8 => {
                let (p, n) = (any(&mut rng, &pool), any(&mut rng, &pool));
                let kids: Vec<NodeId> = dom.children(p).map(|c| c.collect()).unwrap_or_default();
                let before = if kids.is_empty() { None } else { Some(*rng.pick(&kids)) };
                let _ = dom.move_before(p, n, before);
            }
            9 => {
                let (p, n) = (any(&mut rng, &pool), any(&mut rng, &pool));
                let kids: Vec<NodeId> = dom.children(p).map(|c| c.collect()).unwrap_or_default();
                if !kids.is_empty() {
                    let old = *rng.pick(&kids);
                    let _ = dom.replace_child(p, n, old);
                }
            }
            10 => {
                let p = any(&mut rng, &pool);
                let n = if rng.below(3) == 0 { None } else { Some(any(&mut rng, &pool)) };
                let _ = dom.replace_all(p, n);
            }
            11 | 12 => {
                let e = el(&mut rng);
                let v = *rng.pick(CLASS_VALUES);
                let _ = match rng.below(6) {
                    0 => dom.set_attribute(e, "class", v),
                    1 => dom.set_attribute(e, "CLASS", v),
                    2 => dom.set_attribute_ns(e, None, "class", v),
                    3 => dom.set_attribute_ns(e, SVG, "s:class", v),
                    4 => dom.remove_attribute(e, "class").map(drop),
                    _ => dom.toggle_attribute(e, "class", None).map(drop),
                };
            }
            13 => {
                let e = el(&mut rng);
                let v = *rng.pick(CLASS_VALUES);
                match rng.below(3) {
                    0 => {
                        if let Ok(Some(a)) = dom.attribute_node(e, None, "class") {
                            let _ = dom.set_attr_node_value(a, v);
                            pool.push(a);
                        }
                    }
                    1 => {
                        let doc = dom.node_document(e).unwrap();
                        if let Ok(a) = dom.create_attribute(doc, QualName::new(None, ns!(), LocalName::from("class"))) {
                            let _ = dom.set_attr_node_value(a, v);
                            let _ = dom.set_attribute_node(e, a);
                            if rng.below(2) == 0 {
                                let _ = dom.set_attr_node_value(a, rng.pick(CLASS_VALUES));
                            }
                        }
                    }
                    _ => {
                        if let Ok(Some(a)) = dom.attribute_node(e, None, "class") {
                            let _ = dom.remove_attribute_node(e, a);
                            let _ = dom.set_attr_node_value(a, v);
                        }
                    }
                }
            }
            14 => {
                let n = any(&mut rng, &pool);
                if let Ok(c) = dom.clone_node(n, rng.below(2) == 0, None) {
                    pool.extend(dom.descendants(c).unwrap().collect::<Vec<_>>());
                    pool.push(c);
                }
            }
            15 => {
                let n = any(&mut rng, &pool);
                let doc = *rng.pick(&docs);
                if rng.below(2) == 0 {
                    let _ = dom.adopt_node(doc, n);
                } else if let Ok(c) = dom.clone_node(n, true, Some(doc)) {
                    pool.push(c);
                }
            }
            16 | 17 => {
                let e = el(&mut rng);
                let markup = *rng.pick(&[
                    "",
                    "text",
                    "a b\n c",
                    "<span class='a b'>x</span><div class=c><i class=a></i></div>",
                    "<p class=A>1</p>2<my-el class='a'></my-el>",
                    "x\r\ny",
                    "<svg><rect class=a /></svg>",
                    "<template><div class=a></div></template>",
                ]);
                let _ = set_inner_html(&mut dom, e, markup);
                pool.extend(dom.descendants(e).unwrap().collect::<Vec<_>>());
                if let Ok(Some(c)) = dom.template_contents(e) {
                    pool.extend(dom.descendants(c).unwrap().collect::<Vec<_>>());
                    pool.push(c);
                }
            }
            18 => {
                let e = el(&mut rng);
                let _ = dom.set_text_content(e, if rng.below(2) == 0 { "" } else { "tc" });
            }
            19 => {
                let e = el(&mut rng);
                if let Ok(sr) = dom.attach_shadow(e, ShadowRootMode::Open) {
                    if let Ok(f) = parse_fragment(&mut dom, e, "<span class=a>s</span><div class=b></div>") {
                        let _ = dom.append_child(sr, f);
                    }
                    pool.push(sr);
                    pool.extend(dom.descendants(sr).unwrap().collect::<Vec<_>>());
                }
            }
            20 => {
                let doc = *rng.pick(&docs);
                if let Ok(f) = dom.create_fragment(doc) {
                    pool.push(f);
                }
            }
            21 => {
                let e = el(&mut rng);
                if let Ok(f) = parse_fragment(&mut dom, e, "<div class=a><span class=b></span></div>t") {
                    pool.extend(dom.descendants(f).unwrap().collect::<Vec<_>>());
                    pool.push(f);
                }
            }
            22 => {
                let n = any(&mut rng, &pool);
                if dom.node_type(n).unwrap() == NodeType::Text {
                    if let Ok(t) = dom.split_text(n, 1) {
                        pool.push(t);
                    }
                } else {
                    let _ = dom.normalize(n);
                }
            }
            23 => {
                // Slot reuse: drop references to some nodes and collect.
                let keep = pool.len() / 2;
                pool.truncate(keep.max(docs.len()));
                pool.extend(docs);
                dom.collect_garbage();
            }
            24 => {
                let doc = *rng.pick(&docs);
                let q = *rng.pick(&[QuirksMode::NoQuirks, QuirksMode::Quirks, QuirksMode::LimitedQuirks]);
                let _ = dom.set_quirks_mode(doc, q);
            }
            _ => {
                let e = el(&mut rng);
                let _ = dom.set_attribute(e, "id", if rng.below(2) == 0 { "i1" } else { "I1" });
            }
        }
        pool.retain(|&n| alive(&dom, n));
        // Queries from the documents and a few random roots.
        let mut roots: Vec<NodeId> = docs.to_vec();
        for _ in 0..3 {
            roots.push(*rng.pick(&pool));
        }
        for &root in &roots {
            if !alive(&dom, root) {
                continue;
            }
            for css in COMPILED {
                agree(&dom, root, css, step, &mut rng);
            }
            for css in OTHER {
                let sel = parse_selectors(css).unwrap();
                let want: Vec<NodeId> =
                    elements(&dom, root).into_iter().filter(|&n| n != root && dom.matches(n, &sel).unwrap()).collect();
                let got = dom.query_selector_all(root, &sel).unwrap();
                if !css.starts_with(":scope") {
                    assert_eq!(got, want, "step {step}: {css}");
                }
            }
        }
    }
}

#[test]
fn compiled_queries_agree_under_random_mutations() {
    for q in COMPILED {
        assert!(parse_query(q).unwrap().is_compiled(), "{q} is compiled");
    }
    let seeds: u64 = std::env::var("WALK_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
    let steps: usize = std::env::var("WALK_STEPS").ok().and_then(|s| s.parse().ok()).unwrap_or(400);
    for seed in 1..=seeds {
        run_random(0x9E37_79B9 ^ (seed * 0x1234_5678_9ABC), steps);
    }
}

/// Classes written in every way, in quirks and no-quirks documents, and
/// case variants of names.
#[test]
fn class_summary_every_write_path() {
    for quirks in [false, true] {
        let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
        let src = "<body><div id=t class=old></div><svg><g id=s class=old></g></svg><my-el id=c class=OLD></my-el>";
        let d = parse_document(&dom, "http://t/", &if quirks { src.to_string() } else { format!("<!doctype html>{src}") });
        let mut dom = dom.borrow_mut();
        let by = |dom: &Dom, id: &str| dom.query_first(d, &parse_query(&format!("#{id}")).unwrap()).unwrap().unwrap();
        let (t, s, c) = (by(&dom, "t"), by(&dom, "s"), by(&dom, "c"));
        let check = |dom: &Dom, what: &str| {
            for css in [".old", ".OLD", ".new", ".NEW", ".x", ".new.x", "div.new", "g.new", "my-el.new", "MY-EL.new"] {
                let q = parse_query(css).unwrap();
                let sel = parse_selectors(css).unwrap();
                let want: Vec<NodeId> = elements(dom, d).into_iter().filter(|&n| dom.matches(n, &sel).unwrap()).collect();
                assert_eq!(dom.query_all(d, &q).unwrap(), want, "{what} quirks={quirks} {css}");
            }
        };
        for &e in &[t, s, c] {
            dom.set_attribute(e, "class", "new").unwrap();
            check(&dom, "setAttribute");
            dom.set_attribute(e, "class", "\x0cNEW\r\nx\t").unwrap();
            check(&dom, "whitespace");
            dom.remove_attribute(e, "class").unwrap();
            check(&dom, "removeAttribute");
            dom.set_attribute_ns(e, None, "class", "new new x").unwrap();
            check(&dom, "setAttributeNS");
            dom.toggle_attribute(e, "class", Some(false)).unwrap();
            check(&dom, "toggleAttribute");
            dom.set_attribute_ns(e, SVG, "q:class", "new").unwrap();
            check(&dom, "namespaced");
            let a = dom.create_attribute(d, QualName::new(None, ns!(), LocalName::from("class"))).unwrap();
            dom.set_attr_node_value(a, "x").unwrap();
            dom.set_attribute_node(e, a).unwrap();
            check(&dom, "setAttributeNode");
            dom.set_attr_node_value(a, "new x").unwrap();
            check(&dom, "Attr.value");
            dom.remove_attribute_node(e, a).unwrap();
            check(&dom, "removeAttributeNode");
            dom.set_attr_node_value(a, "old").unwrap();
            check(&dom, "detached Attr.value");
            dom.set_attribute(e, "CLASS", "new").unwrap();
            check(&dom, "uppercase name");
        }
    }
}

fn describe(dom: &Dom, parent: NodeId) -> Vec<String> {
    dom.children(parent)
        .unwrap()
        .map(|c| match dom.char_data(c) {
            Ok(data) => format!("{:?} {data:?}", dom.node_type(c).unwrap()),
            Err(_) => format!("{} [{}]", dom.node_name(c).unwrap(), describe(dom, c).join(", ")),
        })
        .collect()
}

fn record_shape(dom: &Dom, r: &MutationRecord) -> String {
    let text = |ids: &[NodeId]| ids.iter().map(|&n| format!("{:?}", dom.char_data(n).ok())).collect::<Vec<_>>();
    format!(
        "{:?} removed={} added={:?} prev={:?} next={:?}",
        r.kind,
        r.removed.len(),
        text(&r.added),
        r.previous_sibling.is_some(),
        r.next_sibling.is_some()
    )
}

/// The text path against the parser in more contexts (`option`, `listing`,
/// custom elements, void elements, `button`, `a`, `li`, `dd`, `body` of a
/// frameset document, XML documents), more inputs (form feed, controls,
/// long strings), and the same mutation records.
#[test]
fn inner_html_text_path_matches_the_parser_more() {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", "<!doctype html><body>");
    let mut dom = dom.borrow_mut();
    let x = lp_dom::xml::parse_document(
        &mut dom,
        "http://test/x.xml",
        "<root xmlns='http://www.w3.org/1999/xhtml'/>",
        "application/xml",
    );
    let long = "lorem ipsum ".repeat(20_000);
    let inputs: Vec<&str> =
        vec!["", "plain", "\x0c", "a\x0cb", "\u{1}\u{7f}\u{9f}", "\u{fffe}\u{ffff}", "  \n\t", "]]>", "\u{200b}x", &long];
    let contexts = [
        "div", "option", "optgroup", "listing", "pre", "my-el", "x-Up", "br", "img", "input", "button", "a", "li", "dd",
        "body", "p", "form", "label", "math", "svg", "object", "ruby", "rt", "image", "menu", "applet", "marquee",
    ];
    for doc in [d, x] {
        for name in contexts {
            for &input in &inputs {
                let el = dom.create_element_ns(doc, HTML, name).unwrap();
                let parent = dom.create_element_ns(doc, HTML, "div").unwrap();
                dom.append_child(parent, el).unwrap();
                set_inner_html(&mut dom, el, "<i>old</i>tail").unwrap();
                let obs = dom.create_observer();
                dom.observe(obs, el, ObserveInit { child_list: true, subtree: true, ..Default::default() }).unwrap();
                set_inner_html(&mut dom, el, input).unwrap();
                let fast: Vec<String> = dom.take_records(obs).iter().map(|r| record_shape(&dom, r)).collect();
                let got = describe(&dom, el);
                let got_owner: Vec<_> =
                    dom.children(el).unwrap().map(|c| dom.node_document(c).unwrap()).collect::<Vec<_>>();
                // The old path: parse, then replace all.
                set_inner_html(&mut dom, el, "<i>old</i>tail").unwrap();
                dom.take_records(obs);
                let frag = parse_fragment(&mut dom, el, input).unwrap();
                dom.replace_all(el, Some(frag)).unwrap();
                let slow: Vec<String> = dom.take_records(obs).iter().map(|r| record_shape(&dom, r)).collect();
                let want = describe(&dom, el);
                let want_owner: Vec<_> = dom.children(el).unwrap().map(|c| dom.node_document(c).unwrap()).collect();
                let short = &input[..input.len().min(20)];
                assert_eq!(got, want, "<{name}>.innerHTML = {short:?} (doc {doc:?})");
                assert_eq!(fast, slow, "records <{name}>.innerHTML = {short:?}");
                assert_eq!(got_owner, want_owner, "owner <{name}>");
            }
        }
    }
}
