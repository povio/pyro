//! Computed styles (`StyleEngine::stylo_value`, `getComputedStyle`) as the
//! page's Blitz document cascades them: values, inheritance, inline styles,
//! `display: none` subtrees, and restyling after DOM changes.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::layout::Layout;
use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::select::parse_selectors;
use pyro_dom::*;

/// Loads nothing (the pages have no sub-resources).
struct NoFetch;
impl pyro_render::Fetcher for NoFetch {
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
    pyro_layout::install(std::sync::Arc::new(NoFetch));
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

#[test]
fn max_size_samples_keep_the_intrinsic_axis_and_policy() {
    use ::style::properties::animated_properties::AnimationValue;
    use ::style::values::animated::Procedure;
    use ::style::values::computed::{length::CSSPixelLength, MaxSize, Size};

    let page = Page::new(r#"<div id=keyword style="max-width:min-content;max-height:min-content"></div><div id=numeric style="max-width:80px;max-height:80px"></div>"#);
    let position = |selector| page.layout.style().stylo_style(&page.dom.borrow(), page.find(selector)).unwrap();
    let keyword = position("#keyword");
    let numeric = position("#numeric");
    for (from, to) in [
        (AnimationValue::MaxWidth(keyword.get_position().max_width.clone()), AnimationValue::MaxWidth(numeric.get_position().max_width.clone())),
        (AnimationValue::MaxHeight(keyword.get_position().max_height.clone()), AnimationValue::MaxHeight(numeric.get_position().max_height.clone())),
    ] {
        let midpoint = Procedure::Interpolate { progress: 0.5 };
        assert!(from.animate_with_size_policy(&to, midpoint, false).is_err());
        for (procedure, expected) in [(midpoint, 60.0), (Procedure::Add, 120.0)] {
            let sample = from.animate_with_size_policy(&to, procedure, true).unwrap();
            let (AnimationValue::MaxWidth(MaxSize::Calculation(value)) | AnimationValue::MaxHeight(MaxSize::Calculation(value))) = sample else { panic!("lost max-size formula") };
            let Size::CalcSizeMinContent(calculation) = value.as_ref() else { panic!("lost min-content basis") };
            assert_eq!(calculation.0.resolve_size(CSSPixelLength::new(40.0), CSSPixelLength::new(200.0)).px(), expected);
        }
    }
}

#[test]
fn computed_custom_names_exclude_invalid_and_unrelated_values() {
    let page = Page::new(r#"<style>@property --count { syntax: '<integer>'; inherits: false; initial-value: 2 } #parent { --shared: 1; } #target { --cyclic: var(--cyclic); } #other { --other: 0; }</style><div id=parent><i id=target></i><i id=other></i></div>"#);
    let target = page.find("#target");
    let names = || page.layout.style().computed_custom_names(&page.dom.borrow(), target, &stylo::PseudoArg::Element);
    assert_eq!(names(), ["--count", "--shared"]);
    page.dom.borrow_mut().set_attribute(target, "style", "--local: 1").unwrap();
    assert_eq!(names(), ["--count", "--local", "--shared"]);
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

/// getComputedStyle against Chrome: `PYRO_CS_CASES` names a JSON file of cases
/// (`{name, html, sel, props}` for one element, or `{name, html_file, width,
/// height, stride, props}` for every `stride`-th element of a page) and
/// `PYRO_CS_CHROME` Chrome's answers (`bench/computed-style/chrome.mjs`).
/// Prints the differences (`PYRO_CS_VERBOSE=1`: each one), the mismatches per
/// property, and the match rate.
#[test]
#[ignore = "needs Chrome's answers (bench/computed-style)"]
fn chrome_parity() {
    use std::collections::BTreeMap;
    let read = |var: &str| -> serde_json::Value {
        let path = std::env::var(var).unwrap_or_else(|_| panic!("{var}"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    };
    let (cases, chrome) = (read("PYRO_CS_CASES"), read("PYRO_CS_CHROME"));
    let verbose = std::env::var_os("PYRO_CS_VERBOSE").is_some();
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
        pyro_layout::install(std::sync::Arc::new(NoFetch));
        let mut layout = Layout::with_viewport(doc, pyro_dom::media::Viewport::new(w, h));
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
    p.layout.set_viewport(pyro_dom::media::Viewport::new(400, 300));
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
    // Stylo (pyro-dom resolves them, `gecko_only`): specificity, source order,
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

#[test]
fn scroll_margin_is_a_computed_length_with_the_normal_cascade() {
    // `scroll-margin-*` are Gecko-only in upstream Stylo; the vendored one parses and computes them for servo
    // too (vendor/stylo/README.pyro.md). SloSki's club roster styles its row actions with
    // `[&_button]:scroll-mb-14` (a descendant rule, in rem) and a media query, and reads the computed value
    // back: `56px` while the rule applies, `0px` (the initial value) when it does not.
    let p = Page::new(
        r#"<!doctype html><style>
        :root { --gap: 12px }
        .list button { scroll-margin-bottom: 3.5rem }
        @media (min-width: 5000px) { .list button { scroll-margin-bottom: 1px } }
        #em { font-size: 10px; scroll-margin-top: 2em }
        #var { scroll-margin-left: var(--gap) }
        #short { scroll-margin: 1px 2px 3px 4px }
        #two { scroll-margin: 5px 6px }
        #neg { scroll-margin-top: -4px }
        #pct { scroll-margin-top: 50% }
        #bad { scroll-margin-top: 9px; scroll-margin-top: banana }
        #imp { scroll-margin-top: 1px !important }
        #spec.s { scroll-margin-top: 2px } #spec { scroll-margin-top: 8px }
        #parent { scroll-margin-top: 20px } #child { }
        #inh { scroll-margin-top: inherit }
        #ini { scroll-margin-top: 30px; scroll-margin-top: initial }
        #vrl { writing-mode: vertical-rl; scroll-margin-block-start: 7px; scroll-margin-inline-start: 3px }
        #rtl { direction: rtl; scroll-margin-inline-start: 11px; scroll-margin-inline-end: 13px }
        #htb { scroll-margin-block: 5px 6px; scroll-margin-inline: 8px 9px }
        </style><body>
        <div class=list><button id=btn>x</button></div>
        <div id=plain>p</div>
        <div id=em>e</div><div id=var>v</div><div id=short>s</div><div id=two>t</div><div id=neg>n</div>
        <div id=pct>p</div><div id=bad>b</div><div id=imp style="scroll-margin-top: 99px">i</div>
        <div id=spec class=s>s</div>
        <div id=parent><div id=child>c</div><div id=inh>h</div></div><div id=ini>i</div>
        <div id=vrl>v</div><div id=rtl>r</div><div id=htb>h</div>
        <div id=inl style="scroll-margin-right: 15px">i</div>
        </body>"#,
    );
    let v = |css: &str, prop: &str| p.value(css, prop);
    assert_eq!(v("#btn", "scroll-margin-bottom"), "56px", "a rem length, from a descendant rule");
    assert_eq!(v("#btn", "scroll-margin-top"), "0px", "the other sides keep the initial value");
    for side in ["top", "right", "bottom", "left", "block-start", "block-end", "inline-start", "inline-end"] {
        assert_eq!(v("#plain", &format!("scroll-margin-{side}")), "0px", "initial scroll-margin-{side}");
    }
    assert_eq!(v("#em", "scroll-margin-top"), "20px", "em resolves against the element's font-size");
    assert_eq!(v("#var", "scroll-margin-left"), "12px", "var() substitutes");
    let sides = |id: &str| {
        ["top", "right", "bottom", "left"].map(|s| v(&format!("#{id}"), &format!("scroll-margin-{s}"))).join(" ")
    };
    assert_eq!(sides("short"), "1px 2px 3px 4px");
    assert_eq!(sides("two"), "5px 6px 5px 6px", "the two-value shorthand mirrors");
    assert_eq!(v("#neg", "scroll-margin-top"), "-4px", "negative lengths are valid");
    assert_eq!(v("#pct", "scroll-margin-top"), "0px", "a percentage is invalid, so the declaration is dropped");
    assert_eq!(v("#bad", "scroll-margin-top"), "9px", "an invalid later declaration does not replace a valid one");
    assert_eq!(v("#imp", "scroll-margin-top"), "1px", "!important beats the inline declaration");
    assert_eq!(v("#spec", "scroll-margin-top"), "2px", "specificity decides before source order");
    assert_eq!(v("#child", "scroll-margin-top"), "0px", "not inherited");
    assert_eq!(v("#inh", "scroll-margin-top"), "20px", "inherit takes the parent's");
    assert_eq!(v("#ini", "scroll-margin-top"), "0px", "initial");
    assert_eq!(v("#inl", "scroll-margin-right"), "15px", "inline style");
    // Logical longhands compute onto the physical sides the writing mode and direction name.
    assert_eq!(v("#htb", "scroll-margin-top"), "5px");
    assert_eq!(v("#htb", "scroll-margin-bottom"), "6px");
    assert_eq!(v("#htb", "scroll-margin-left"), "8px");
    assert_eq!(v("#htb", "scroll-margin-right"), "9px");
    assert_eq!(v("#vrl", "scroll-margin-right"), "7px", "vertical-rl: block-start is the right side");
    assert_eq!(v("#vrl", "scroll-margin-top"), "3px", "vertical-rl: inline-start is the top side");
    assert_eq!(v("#rtl", "scroll-margin-right"), "11px", "rtl: inline-start is the right side");
    assert_eq!(v("#rtl", "scroll-margin-left"), "13px");
}

#[test]
fn scroll_margin_follows_media_queries_and_restyles() {
    // The roster's mobile-only margin: the same rule, applied only below the breakpoint.
    let p = Page::new(
        "<!doctype html><style>@media (max-width: 100px) { #t { scroll-margin-bottom: 56px } }</style>\
         <div id=t>t</div><div id=c class=off>c</div>\
         <style>.on { scroll-margin-top: 4px }</style>",
    );
    // The page's viewport is wider than the query: the rule does not apply.
    assert_eq!(p.value("#t", "scroll-margin-bottom"), "0px");
    let c = p.find("#c");
    assert_eq!(p.value("#c", "scroll-margin-top"), "0px");
    p.dom.borrow_mut().set_attribute(c, "class", "on").unwrap();
    assert_eq!(p.value("#c", "scroll-margin-top"), "4px", "a class change restyles");
    p.dom.borrow_mut().set_attribute(c, "style", "scroll-margin-top: 6px").unwrap();
    assert_eq!(p.value("#c", "scroll-margin-top"), "6px", "an inline change restyles");
}

#[test]
fn flex_items_report_the_used_size_after_flexing() {
    let page = Page::new(r#"<!doctype html><div style="display:flex;width:120px"><div style="display:contents">
        <div id=row style="width:15px;flex:1"></div></div></div>
        <div style="display:flex;flex-direction:column;height:100px"><div id=column style="height:20px;flex:0 0 60px;padding:5px;box-sizing:border-box"></div></div>
        <div id=ordinary style="width:15px"></div>"#);
    let used = |selector, name| page.layout.computed_style_value(&page.dom.borrow(), page.find(selector), name);
    assert_eq!(used("#row", "width"), "120px");
    assert_eq!(used("#column", "height"), "60px");
    assert_eq!(used("#ordinary", "width"), "15px");
}

#[test]
fn flex_target_size_is_not_an_intrinsic_calculation_basis() {
    let page = Page::new(r#"<!doctype html><div style="display:flex;width:200px">
        <div id=row style="flex:0 0 70px;width:calc-size(fit-content,size + 12px)"></div></div>
        <div style="display:flex;flex-direction:column;height:100px">
        <div id=column style="flex:0 0 60px;height:calc-size(auto,size * 2)"></div></div>"#);
    let used = |selector, name| page.layout.computed_style_value(&page.dom.borrow(), page.find(selector), name);
    assert_eq!(used("#row", "width"), "70px");
    assert_eq!(used("#column", "height"), "60px");
}

#[test]
fn flex_basis_calculates_from_content_independently_of_height() {
    let page = Page::new(r#"<!doctype html><div style="display:flex;flex-direction:column;height:280px">
        <div id=item style="flex:0 0 calc-size(calc-size(content,size * 2),size + 7px);height:100px">
        <div style="height:20px"></div></div></div>
        <div id=ordinary style="width:50px;width:calc-size(content,size * 2)"></div>"#);
    let used = |selector, name| page.layout.computed_style_value(&page.dom.borrow(), page.find(selector), name);
    assert_eq!(used("#item", "height"), "47px");
    assert_eq!(used("#ordinary", "width"), "50px");
}

#[test]
fn intrinsic_size_and_container_percentage_are_independent() {
    let page = Page::new(r#"<!doctype html><div style="display:flex;width:300px">
        <div id=row style="min-width:0;flex:0 0 calc-size(max-content,size + 10%)"><div style="width:45px"></div></div></div>
        <div style="display:flex;flex-direction:column;height:200px">
        <div id=column style="flex:0 0 calc-size(calc-size(content,size + 10%),size + 20%)"><div style="height:25px"></div></div></div>
        <div style="display:flex;flex-direction:column"><div id=indefinite style="flex:0 0 calc-size(content,size + 50%)"><div style="height:35px"></div></div></div>"#);
    let used = |selector, name| page.layout.computed_style_value(&page.dom.borrow(), page.find(selector), name);
    assert_eq!(used("#row", "width"), "75px");
    assert_eq!(used("#column", "height"), "85px");
    assert_eq!(used("#indefinite", "height"), "35px");
}

#[test]
fn sampled_auto_basis_is_not_the_live_any_endpoint() {
    let page = Page::new(r#"<!doctype html><div style="width:200px;height:300px"><div id=live style="width:calc-size(any,50px);height:calc-size(any,50px)"><div style="height:100px"></div></div></div><div id=sample style="width:calc-size(auto,size * 0.25 + 37.5px);height:calc-size(auto,size * 0.25 + 37.5px)"></div>"#);
    let dom = page.dom.borrow();
    let live = page.find("#live");
    let style = page.layout.style();
    let sample = style.stylo_style(&dom, page.find("#sample")).unwrap();
    for (axis, expected) in [("width", "87.5px"), ("height", "62.5px")] {
        let used = || page.layout.computed_style_value(&dom, live, axis);
        assert_eq!(used(), "50px");
        assert_eq!(style.with_style_override(live, sample.clone(), used).unwrap(), expected);
        assert_eq!(used(), "50px");
    }
}

#[test]
fn sampled_intrinsic_block_height_uses_fixed_child_not_endpoint() {
    let page = Page::new(r#"<!doctype html><div style="width:200px;height:300px"><div id=live style="height:calc-size(any,50px);box-sizing:border-box;padding:5px"><div style="height:90px"></div></div></div><div id=min style="height:calc-size(min-content,size * 0.25 + 37.5px)"></div><div id=max style="height:calc-size(max-content,size * 0.25 + 37.5px)"></div><div id=fit style="height:calc-size(fit-content,size * 0.25 + 37.5px)"></div>"#);
    let dom = page.dom.borrow();
    let live = page.find("#live");
    let style = page.layout.style();
    let used = || page.layout.computed_style_value(&dom, live, "height");
    for selector in ["#min", "#max", "#fit"] {
        let sample = style.stylo_style(&dom, page.find(selector)).unwrap();
        assert_eq!(used(), "50px");
        assert_eq!(style.with_style_override(live, sample, used).unwrap(), "62.5px");
        assert_eq!(used(), "50px");
    }
}

#[test]
fn sampled_auto_height_with_percentage_endpoint() {
    let page = Page::new(r#"<!doctype html><div style="height:300px"><div id=live style="height:50%"><div style="height:100px"></div></div></div><div id=sample style="height:calc-size(auto,size * 0.25 + 37.5%)"></div>"#);
    let dom = page.dom.borrow();
    let style = page.layout.style();
    let live = page.find("#live");
    let sample = style.stylo_style(&dom, page.find("#sample")).unwrap();
    let used = || page.layout.computed_style_value(&dom, live, "height");
    assert_eq!(used(), "150px");
    assert_eq!(style.with_style_override(live, sample, used).unwrap(), "137.5px");
    assert_eq!(used(), "150px");
}

#[test]
fn sampled_keyword_is_independent_of_live_calc_endpoint() {
    let page = Page::new(r#"<!doctype html><div style="width:200px;height:300px"><div id=intrinsic style="width:calc-size(fit-content,size);height:calc-size(auto,size)"><div style="width:100px;height:100px"></div></div><div id=stretch style="width:calc-size(stretch,size);height:calc-size(stretch,size)"><div style="width:100px;height:100px"></div></div></div><div id=available style="width:auto;height:stretch"></div><div id=content style="width:max-content;height:max-content"></div>"#);
    let dom = page.dom.borrow();
    let style = page.layout.style();
    for (target, sampled, dimensions) in [
        ("#intrinsic", "#available", [("width", "100px", "200px"), ("height", "100px", "300px")]),
        ("#stretch", "#content", [("width", "200px", "100px"), ("height", "300px", "100px")]),
    ] {
        let live = page.find(target);
        let sample = style.stylo_style(&dom, page.find(sampled)).unwrap();
        for (axis, endpoint, expected) in dimensions {
            let used = || page.layout.computed_style_value(&dom, live, axis);
            assert_eq!(used(), endpoint);
            assert_eq!(style.with_style_override(live, sample.clone(), used).unwrap(), expected);
            assert_eq!(used(), endpoint);
        }
    }
}

#[test]
fn sampled_discrete_keyword_uses_selected_endpoint_geometry() {
    let page = Page::new(r#"<!doctype html><div style="width:150px;height:200px"><div id=live style="width:fit-content;height:stretch"><div style="width:100px;height:50px"></div></div></div><div id=sample style="width:auto;height:max-content"></div>"#);
    let dom = page.dom.borrow();
    let live = page.find("#live");
    let style = page.layout.style();
    let sample = style.stylo_style(&dom, page.find("#sample")).unwrap();
    for (axis, endpoint, selected) in [("width", "100px", "150px"), ("height", "200px", "50px")] {
        let used = || page.layout.computed_style_value(&dom, live, axis);
        assert_eq!(used(), endpoint);
        assert_eq!(style.with_style_override(live, sample.clone(), used).unwrap(), selected);
        assert_eq!(used(), endpoint);
    }
}

#[test]
fn sampled_keyword_endpoint_does_not_replace_intermediate_size() {
    let page = Page::new(r#"<!doctype html><div style="width:200px;height:300px"><div id=live style="width:fit-content;height:auto"><div style="width:100px;height:50px"></div></div></div><div id=sample style="width:calc-size(fit-content,size * 0.5 + 100px);height:calc-size(auto,size * 0.5)"></div>"#);
    let dom = page.dom.borrow();
    let live = page.find("#live");
    let style = page.layout.style();
    let sample = style.stylo_style(&dom, page.find("#sample")).unwrap();
    for (axis, endpoint, midpoint) in [("width", "100px", "150px"), ("height", "50px", "25px")] {
        let used = || page.layout.computed_style_value(&dom, live, axis);
        assert_eq!(used(), endpoint);
        assert_eq!(style.with_style_override(live, sample.clone(), used).unwrap(), midpoint);
        assert_eq!(used(), endpoint);
    }
}

#[test]
fn sampled_max_content_with_auto_parent_and_fixed_maximum() {
    let page = Page::new(r#"<!doctype html><style>#live::before{content:'';display:block;width:125px;height:75px}</style><div><div id=live style="width:100px;height:25px;max-height:42px"></div></div><div id=sample style="inline-size:calc-size(max-content,size * 0.5 + 50px);block-size:calc-size(max-content,size * 0.5 + 12.5px)"></div>"#);
    let dom = page.dom.borrow();
    let live = page.find("#live");
    let style = page.layout.style();
    let sample = style.stylo_style(&dom, page.find("#sample")).unwrap();
    for (axis, endpoint, expected) in [("inline-size", "100px", "112.5px"), ("block-size", "25px", "42px")] {
        let used = || page.layout.computed_style_value(&dom, live, axis);
        assert_eq!(used(), endpoint);
        assert_eq!(style.with_style_override(live, sample.clone(), used).unwrap(), expected);
        assert_eq!(used(), endpoint);
    }
}

#[test]
fn sampled_keyword_basis_with_plain_length_endpoint() {
    let page = Page::new(r#"<!doctype html><div style="width:200px;height:300px"><div id=live style="width:50px;height:50px"><div style="width:100px;height:100px"></div></div></div><div id=sample style="width:calc-size(auto,size * 0.25 + 37.5px);height:calc-size(auto,size * 0.25 + 37.5px)"></div>"#);
    let dom = page.dom.borrow();
    let live = page.find("#live");
    let style = page.layout.style();
    let sample = style.stylo_style(&dom, page.find("#sample")).unwrap();
    for (axis, expected) in [("width", "87.5px"), ("height", "62.5px")] {
        let used = || page.layout.computed_style_value(&dom, live, axis);
        assert_eq!(used(), "50px");
        assert_eq!(style.with_style_override(live, sample.clone(), used).unwrap(), expected);
        assert_eq!(used(), "50px");
    }
}

#[test]
fn sampled_intrinsic_width_uses_fixed_child_border_box() {
    let page = Page::new(r#"<!doctype html><div style="width:200px"><div id=live style="width:calc-size(any,50px);box-sizing:border-box;padding:5px"><div style="width:80px;border:5px solid"></div></div></div><div id=min style="width:calc-size(min-content,size * 0.25 + 37.5px)"></div><div id=max style="width:calc-size(max-content,size * 0.25 + 37.5px)"></div><div id=fit style="width:calc-size(fit-content,size * 0.25 + 37.5px)"></div>"#);
    let dom = page.dom.borrow();
    let live = page.find("#live");
    let style = page.layout.style();
    let used = || page.layout.computed_style_value(&dom, live, "width");
    for selector in ["#min", "#max", "#fit"] {
        let sample = style.stylo_style(&dom, page.find(selector)).unwrap();
        assert_eq!(used(), "50px");
        assert_eq!(style.with_style_override(live, sample, used).unwrap(), "62.5px");
        assert_eq!(used(), "50px");
    }
}

#[test]
fn sampled_stretch_uses_containing_block_margin_box_on_both_axes() {
    let page = Page::new(r#"<!doctype html><div style="width:200px;height:300px"><div id=content style="width:calc-size(any,50px);height:calc-size(any,50px);margin:5%;padding:5px"></div><div id=border style="width:calc-size(any,50px);height:calc-size(any,50px);margin:5%;padding:5px;box-sizing:border-box"></div></div><div id=sample style="width:calc-size(stretch,size * 0.25 + 37.5px);height:calc-size(stretch,size * 0.25 + 37.5px)"></div>"#);
    let dom = page.dom.borrow();
    let style = page.layout.style();
    for (selector, property, expected) in [("#content", "width", "80px"), ("#content", "height", "105px"), ("#border", "width", "82.5px"), ("#border", "height", "107.5px")] {
        let live = page.find(selector);
        let sample = style.stylo_style(&dom, page.find("#sample")).unwrap();
        let used = || page.layout.computed_style_value(&dom, live, property);
        assert_eq!(used(), "50px");
        assert_eq!(style.with_style_override(live, sample, used).unwrap(), expected);
        assert_eq!(used(), "50px");
    }
}

#[test]
fn percentage_endpoint_interpolates_with_intrinsic_size() {
    use ::style::values::animated::{Animate, Procedure};
    use ::style::values::computed::{length::CSSPixelLength, Size};

    let page = Page::new(r#"<div id=start style="height:calc-size(auto, size * 2)"></div><div id=end style="height:50%"></div>"#);
    let height = |selector| page.layout.style().stylo_style(&page.dom.borrow(), page.find(selector)).unwrap().get_position().height.clone();
    let Size::CalcSizeAuto(sample) = height("#start").animate(&height("#end"), Procedure::Interpolate { progress: 0.75 }).unwrap() else { panic!("lost intrinsic basis") };
    assert_eq!(sample.0.resolve_size(CSSPixelLength::new(100.0), CSSPixelLength::new(300.0)).px(), 162.5);
    assert_eq!(sample.0.resolve_size(CSSPixelLength::new(100.0), CSSPixelLength::new(0.0)).px(), 50.0);
}

#[test]
fn flex_basis_calculation_uses_the_items_box_sizing() {
    let page = Page::new(r#"<!doctype html><div style="display:flex;width:240px">
        <div id=content style="width:40px;padding:5px;flex:0 0 calc-size(auto,size * 2)"></div>
        <div id=border style="width:40px;padding:5px;box-sizing:border-box;flex:0 0 calc-size(auto,size * 2)"></div></div>
        <div style="display:flex;writing-mode:vertical-rl;height:120px"><div id=vertical style="height:20px;width:10px;flex:1"></div></div>"#);
    let used = |selector, name| page.layout.computed_style_value(&page.dom.borrow(), page.find(selector), name);
    assert_eq!(used("#content", "width"), "80px");
    assert_eq!(used("#border", "width"), "80px");
    assert_eq!(used("#vertical", "width"), "10px");
    assert_eq!(used("#vertical", "height"), "120px");
}

/// pyro (lane G): a subgrid's `grid-template-columns` resolves to `subgrid` with the names of each of
/// the lines it spans (its own), which only its layout knows (css-grid-2 §7.2.6.2).
#[test]
fn a_subgrids_template_resolves_to_its_lines() {
    let p = Page::new(
        r#"<!doctype html><div style="display:grid;grid-template-columns:10px 20px 30px">
        <div id=s style="display:grid;grid-template-columns:subgrid [a];grid-column:span 3"></div></div>"#,
    );
    let el = p.find("#s");
    assert_eq!(p.layout.computed_style_value(&p.dom.borrow(), el, "grid-template-columns"), "subgrid [a] [] [] []");
}

/// pyro (lane G): a grid container's `grid-template-columns`/`-rows` resolve to its used tracks, each
/// listed (css-grid-1 §7.2.6): `repeat(auto-fill, …)` in an absolutely positioned 100px square grid
/// is four 25px columns and five 20px rows (WPT `grid-auto-repeat-positioned-container-001`).
#[test]
fn a_grid_containers_template_resolves_to_its_used_tracks() {
    let p = Page::new(
        r#"<!doctype html><div style="position:relative;width:100px;height:100px">
        <div id=g style="display:grid;position:absolute;inset:0;grid:repeat(auto-fill, 20px) / repeat(auto-fill, 25px)"></div></div>"#,
    );
    let el = p.find("#g");
    let dom = p.dom.borrow();
    assert_eq!(p.layout.computed_style_value(&dom, el, "grid-template-columns"), "25px 25px 25px 25px");
    assert_eq!(p.layout.computed_style_value(&dom, el, "grid-template-rows"), "20px 20px 20px 20px 20px");
}

/// A grid container with `grid-template: none` reports the shorthand from its used tracks, as Chromium does;
/// a set template keeps its computed value (WPT css-grid `grid-layout-properties`).
#[test]
fn an_untemplated_grid_reports_grid_template_from_its_used_tracks() {
    let p = Page::new(
        r#"<!doctype html><div id=g style="display:grid;width:100px;grid-auto-rows:20px">
        <div></div><div></div></div><div id=t style="display:grid;width:100px;grid-template:[a] auto / 50px"></div>"#,
    );
    let dom = p.dom.borrow();
    assert_eq!(p.layout.computed_style_value(&dom, p.find("#g"), "grid-template"), "20px 20px / 100px");
    assert_eq!(p.layout.computed_style_value(&dom, p.find("#t"), "grid-template"), "[a] auto / 50px");
}

#[test]
fn svg_presentation_attributes_participate_in_the_cascade() {
    let p = Page::new(
        r#"<!doctype html><style>
        :root { --weight: 400 } body { color: white; font-weight: 700 }
        #override { stroke: red; font-weight: 500 }
        </style><svg id=svg stroke=currentColor fill=none>
        <text id=variable font-weight="var(--weight)">word</text>
        <text id=literal font-weight=400>word</text>
        <text id=override font-weight=400 stroke=blue>word</text>
        <path id=path d="M0 0L10 10"/></svg>"#,
    );
    assert_eq!(p.value("#variable", "font-weight"), "400");
    assert_eq!(p.value("#literal", "font-weight"), "400");
    assert_eq!(p.value("#override", "font-weight"), "500");
    assert_eq!(p.value("#override", "stroke"), "rgb(255, 0, 0)");
    assert_eq!(p.value("#svg", "stroke"), "rgb(255, 255, 255)");
    assert_eq!(p.value("#path", "stroke"), "rgb(255, 255, 255)");
    assert_eq!(p.value("#path", "fill"), "none");
    let svg = p.find("#svg");
    p.dom.borrow_mut().set_attribute(svg, "stroke", "blue").unwrap();
    assert_eq!(p.value("#path", "stroke"), "rgb(0, 0, 255)");
    let literal = p.find("#literal");
    p.dom.borrow_mut().remove_attribute(literal, "font-weight").unwrap();
    assert_eq!(p.value("#literal", "font-weight"), "700");
}

#[test]
fn auto_min_size_of_an_unrendered_grid_child_resolves_to_zero() {
    // WPT css-grid grid-item-min-auto-size-001: only a box laid out as a grid item keeps `auto`.
    let page = Page::new(r#"<!doctype html><style>.g{display:grid} .m{min-width:auto;min-height:auto}</style>
        <div class=g><div id=item class=m></div></div>
        <div style="display:none"><div class=g><div id=inside class=m></div></div></div>
        <div class=g><div style="display:contents"><div id=contents class=m></div></div></div>
        <div class=g><div id=none class=m style="display:none"></div></div>"#);
    let used = |selector, name| page.layout.computed_style_value(&page.dom.borrow(), page.find(selector), name);
    for (selector, expected) in [("#item", "auto"), ("#inside", "0px"), ("#contents", "auto"), ("#none", "0px")] {
        assert_eq!(used(selector, "min-width"), expected, "{selector}");
        assert_eq!(used(selector, "min-height"), expected, "{selector}");
    }
}
