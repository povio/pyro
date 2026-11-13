//! `:has()` invalidation for element and form state: each state change
//! restyles the anchors whose `:has()` argument depends on it (Stylo's
//! relative selector invalidation in Blitz), checked incrementally and
//! against a fresh layout; and a change no `:has()` depends on costs no
//! restyle beyond what the element itself needs.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::layout::Layout;
use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::select::parse_selectors;
use pyro_dom::*;

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
    /// `css`'s color now, incrementally and from scratch (a fresh Blitz
    /// document of the same DOM).
    fn color(&self, css: &str) -> (String, String) {
        let incremental = self.value(css, "color");
        let fresh = layout_of(&self.dom.borrow(), self.doc);
        let el = self.find(css);
        (incremental, fresh.style().stylo_value(&self.dom.borrow(), el, "color").unwrap())
    }
}

const BLACK: &str = "rgb(0, 0, 0)";

/// Runs `change`, then checks `css`'s color is `want`, incrementally and
/// from scratch.
fn after(p: &Page, label: &str, change: impl FnOnce(&mut Dom), css: &str, want: &str) {
    change(&mut p.dom.borrow_mut());
    let (incremental, fresh) = p.color(css);
    assert_eq!(fresh, want, "{label}: {css} from scratch");
    assert_eq!(incremental, want, "{label}: {css} incrementally");
}

#[test]
fn has_follows_element_and_form_state() {
    let p = Page::new(
        "<!doctype html><style>\
         #f:has(input:invalid){color:rgb(1,0,0)}\
         label:has(input:checked){color:rgb(2,0,0)}\
         #fd:has(:focus){color:rgb(3,0,0)}\
         #hv:has(.d:hover){color:rgb(4,0,0)}\
         #ac:has(.d:hover:active){color:rgb(5,0,0)}\
         #sl:has(option:checked){color:rgb(6,0,0)}\
         #ds:has(:disabled){color:rgb(7,0,0)}\
         #ph:has(:placeholder-shown){color:rgb(8,0,0)}\
         #op:has(:open){color:rgb(9,0,0)}\
         #sib:has(+ input:checked){color:rgb(11,0,0)}\
         #fv:has(:focus-visible){color:rgb(12,0,0)}\
         #fv:has(:focus-visible) .other{color:rgb(13,0,0)}\
         #rg:has(:out-of-range){color:rgb(14,0,0)}\
         #rg:has(:in-range) + #after{color:rgb(15,0,0)}\
         </style><body>\
         <form id=f><input id=fi required></form>\
         <label id=l><input id=cb type=checkbox></label>\
         <div id=fd><input id=fdi></div>\
         <div id=hv><span id=hvs class=d>x</span></div>\
         <div id=ac><span id=acs class=d>x</span></div>\
         <div id=sl><select id=sel><option id=o1>a</option><option id=o2 selected>b</option></select></div>\
         <div id=ds><input id=dsi></div>\
         <div id=ph><input id=phi placeholder=p></div>\
         <div id=op><details id=det><summary>s</summary>x</details></div>\
         <span id=sib>s</span><input id=cb2 type=checkbox>\
         <div id=fv><button id=fvb>b</button><p class=other id=fvo>o</p></div>\
         <div id=rg><input id=num type=number min=1 max=10 value=5></div><p id=after>a</p>\
         </body>",
    );
    let doc = p.doc;
    let id = |p: &Page, s: &str| p.find(&format!("#{s}"));

    assert_eq!(p.color("#f").0, "rgb(1, 0, 0)");
    let fi = id(&p, "fi");
    after(&p, "value makes the form valid", |d| d.set_value(fi, "x").unwrap(), "#f", BLACK);
    after(&p, "value makes it invalid", |d| d.set_value(fi, "").unwrap(), "#f", "rgb(1, 0, 0)");

    let cb = id(&p, "cb");
    after(&p, "checked", |d| d.set_checked(cb, true).unwrap(), "#l", "rgb(2, 0, 0)");
    after(&p, "unchecked", |d| d.set_checked(cb, false).unwrap(), "#l", BLACK);

    let fdi = id(&p, "fdi");
    after(&p, "focus", |d| { let _ = d.set_focused(doc, Some(fdi)).unwrap(); }, "#fd", "rgb(3, 0, 0)");
    after(&p, "blur", |d| { let _ = d.set_focused(doc, None).unwrap(); }, "#fd", BLACK);

    let hvs = id(&p, "hvs");
    after(&p, "hover", |d| d.set_hovered(doc, Some(hvs)).unwrap(), "#hv", "rgb(4, 0, 0)");
    after(&p, "unhover", |d| d.set_hovered(doc, None).unwrap(), "#hv", BLACK);
    let acs = id(&p, "acs");
    p.dom.borrow_mut().set_hovered(doc, Some(acs)).unwrap();
    after(&p, "hover and active", |d| d.set_activated(doc, Some(acs), true).unwrap(), "#ac", "rgb(5, 0, 0)");
    after(&p, "no longer active", |d| d.set_activated(doc, None, true).unwrap(), "#ac", BLACK);
    p.dom.borrow_mut().set_hovered(doc, None).unwrap();

    assert_eq!(p.color("#sl").0, "rgb(6, 0, 0)", "a selected option is :checked");
    let sel = id(&p, "sel");
    after(&p, "no option selected", |d| d.set_selected_index(sel, -1).unwrap(), "#sl", BLACK);
    after(&p, "an option selected", |d| d.set_selected_index(sel, 0).unwrap(), "#sl", "rgb(6, 0, 0)");

    let dsi = id(&p, "dsi");
    after(&p, "disabled", |d| d.set_attribute(dsi, "disabled", "").unwrap(), "#ds", "rgb(7, 0, 0)");
    after(&p, "enabled", |d| { let _ = d.remove_attribute(dsi, "disabled").unwrap(); }, "#ds", BLACK);

    assert_eq!(p.color("#ph").0, "rgb(8, 0, 0)");
    let phi = id(&p, "phi");
    after(&p, "placeholder hidden", |d| d.set_value(phi, "v").unwrap(), "#ph", BLACK);
    after(&p, "placeholder shown", |d| d.set_value(phi, "").unwrap(), "#ph", "rgb(8, 0, 0)");

    let det = id(&p, "det");
    after(&p, "details open", |d| d.set_attribute(det, "open", "").unwrap(), "#op", "rgb(9, 0, 0)");
    after(&p, "details closed", |d| { let _ = d.remove_attribute(det, "open").unwrap(); }, "#op", BLACK);

    let cb2 = id(&p, "cb2");
    after(&p, "next sibling checked", |d| d.set_checked(cb2, true).unwrap(), "#sib", "rgb(11, 0, 0)");

    // :focus-visible, with a descendant of the anchor named after :has().
    let fvb = id(&p, "fvb");
    p.dom.borrow_mut().set_focus_visible(doc, true).unwrap();
    after(&p, "focus-visible", |d| { let _ = d.set_focused(doc, Some(fvb)).unwrap(); }, "#fv", "rgb(12, 0, 0)");
    assert_eq!(p.color("#fvo").0, "rgb(13, 0, 0)");
    after(&p, "focus leaves", |d| { let _ = d.set_focused(doc, None).unwrap(); }, "#fvo", BLACK);

    // :in-range / :out-of-range, and a sibling after the anchor.
    assert_eq!(p.color("#after").0, "rgb(15, 0, 0)");
    let num = id(&p, "num");
    after(&p, "out of range", |d| d.set_value(num, "12").unwrap(), "#rg", "rgb(14, 0, 0)");
    assert_eq!(p.color("#after").0, BLACK);
    after(&p, "in range", |d| d.set_value(num, "3").unwrap(), "#after", "rgb(15, 0, 0)");
}

/// Elements Blitz styled (cascaded) on this thread so far.
fn styled() -> u64 {
    pyro_render::blitz_style_stats()[0]
}

/// Elements styled again after `change` (the page restyled first).
fn restyled_by(p: &Page, change: impl FnOnce(&mut Dom), css: &str) -> u64 {
    let _ = p.value(css, "color");
    let before = styled();
    change(&mut p.dom.borrow_mut());
    let _ = p.value(css, "color");
    styled() - before
}

fn page_with(rules: &str) -> Page {
    let rows: String = (0..200).map(|i| format!("<div class=row id=r{i}>row {i}</div>")).collect();
    Page::new(&format!(
        "<!doctype html><style>input:checked{{outline:1px solid}} .x{{outline-color:red}} {rules}</style>\
         <body><main id=m>{rows}<input id=cb type=checkbox><div id=t>t</div></main></body>"
    ))
}

#[test]
fn state_changes_no_has_depends_on_restyle_nothing_more() {
    // No :has() at all.
    let plain = page_with("");
    let cb = plain.find("#cb");
    let toggled = restyled_by(&plain, |d| d.set_checked(cb, true).unwrap(), "#cb");
    let t = plain.find("#t");
    let classed = restyled_by(&plain, |d| d.set_attribute(t, "class", "x").unwrap(), "#t");
    let unchecked = restyled_by(&plain, |d| d.set_checked(cb, false).unwrap(), "#cb");

    // A `:has()` anchoring at the body: every element is in its search path,
    // but it depends on neither `:checked` nor class `x`.
    let has = page_with("body:has(.modal-open){outline-style:dotted}");
    assert_eq!(has.value("body", "outline-style"), "none");
    let cb = has.find("#cb");
    assert_eq!(restyled_by(&has, |d| d.set_checked(cb, true).unwrap(), "#cb"), toggled, ":checked");
    let t = has.find("#t");
    assert_eq!(restyled_by(&has, |d| d.set_attribute(t, "class", "x").unwrap(), "#t"), classed, "class x");
    assert_eq!(restyled_by(&has, |d| d.set_checked(cb, false).unwrap(), "#cb"), unchecked, ":checked off");

    // A class it depends on restyles the anchor itself, not its subtree.
    let opened = restyled_by(&has, |d| d.set_attribute(t, "class", "x modal-open").unwrap(), "body");
    assert_eq!(has.value("body", "outline-style"), "dotted");
    assert!(opened <= classed + 2, "the anchor restyles alone: {opened} (a class change: {classed})");
}

#[test]
fn has_follows_changes_under_an_anchor_the_bloom_filter_fast_rejected() {
    // `.card:has(.a)` matches at the span and stops searching; the second
    // lookup for the same anchor builds a bloom filter that fast-rejects
    // `.b` without visiting (or flagging) anything. Changes to elements only
    // the filter saw still reach the anchor, for classes, other attributes
    // and element state; and for a sibling search.
    let cases: [(&str, &str, &str, fn(&mut Dom, NodeId)); 4] = [
        (".card:has(.b)", "<i id=t></i>", ".card", |d, t| d.set_attribute(t, "class", "b").unwrap()),
        (".card:has([data-x])", "<i id=t></i>", ".card", |d, t| d.set_attribute(t, "data-x", "").unwrap()),
        (".card:has(input:checked)", "<input id=t type=checkbox>", ".card", |d, t| d.set_checked(t, true).unwrap()),
        (".sib:has(~ .b)", "<i id=t></i>", ".sib", |d, t| d.set_attribute(t, "class", "b").unwrap()),
    ];
    for (rule, target, anchor, change) in cases {
        let html = if anchor == ".sib" {
            format!("<p class=sib></p><span class=a></span><b></b>{target}")
        } else {
            format!("<div class=card><span class=a></span><div>{target}</div></div>")
        };
        let p = Page::new(&format!(
            "<!doctype html><style>.card:has(.a),.sib:has(~ .a){{outline-style:dotted}}{rule}{{color:rgb(1,0,0)}}</style>\
             <body><main>{html}</main></body>"
        ));
        assert_eq!(p.color(anchor).0, BLACK, "{rule}");
        let t = p.find("#t");
        after(&p, rule, |d| change(d, t), anchor, "rgb(1, 0, 0)");
    }
}
