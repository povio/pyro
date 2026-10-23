//! Differential tests: flow styles read through the typed declaration slots
//! (`style_flow.rs`) must equal those of the string cascade alone, for every
//! element, after every kind of change the slots can see.
use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::layout::{Geometry, Layout};
use lp_dom::media::Viewport;
use lp_dom::node::NodeType;
use lp_dom::parse::{parse_document, DomRef};
use lp_dom::{Dom, NodeId};

struct Page {
    dom: Dom,
    doc: NodeId,
    layout: Layout,
}

fn page(html: &str) -> Page {
    let dom_ref: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom_ref, "http://test/", html);
    let dom = Rc::try_unwrap(dom_ref).ok().expect("sole owner").into_inner();
    let mut layout = Layout::with_viewport(doc, Viewport::new(800, 600));
    layout.style_mut().discover_style_elements(&dom);
    layout.set_geometry(Geometry::Flow);
    Page { dom, doc, layout }
}

fn elements(p: &Page) -> Vec<NodeId> {
    p.dom.descendants(p.doc).unwrap().filter(|&n| p.dom.node_type(n).ok() == Some(NodeType::Element)).collect()
}

/// Every element's flow style through the typed slots equals the string
/// cascade's, and the laid-out geometry equals a fresh string-only layout's.
fn assert_same(p: &mut Page, step: &str) {
    let style = p.layout.style();
    for el in elements(p) {
        style.set_typed_flow_reads(true);
        let typed = style.flow_style_unshared(&p.dom, el);
        style.set_typed_flow_reads(false);
        let strings = style.flow_style_unshared(&p.dom, el);
        style.set_typed_flow_reads(true);
        let tag = p.dom.tag_name(el).unwrap_or_default();
        assert_eq!(typed, strings, "{step}: <{tag}> {el:?}");
    }
    let cached = p.layout.flow(&p.dom);
    let mut fresh = Layout::with_viewport(p.doc, p.layout.style().viewport().clone());
    fresh.style_mut().discover_style_elements(&p.dom);
    fresh.style().set_typed_flow_reads(false);
    fresh.set_geometry(Geometry::Flow);
    let f = fresh.flow(&p.dom);
    assert_eq!(cached.boxes.len(), f.boxes.len(), "{step}: box count");
    for (k, b) in &f.boxes {
        assert_eq!(cached.boxes.get(k).map(|c| c.rect), Some(b.rect), "{step}: geometry of {k:?}");
    }
}

const CSS: &str = "<style>\
@layer base, util;\
:root{--w:40px;--p:5%;--bad:nonsense;--sp:.25rem;font-size:15px}\
.lit{width:10px;height:50%;min-width:0;max-width:auto;padding:4px 8px;margin:auto 3px;gap:2px 6px;overflow:hidden auto;inset:1px 2px 3px 4px}\
.var{width:var(--w);height:var(--p);padding-left:var(--missing,7px);margin-top:var(--bad);row-gap:calc(var(--sp)*4);line-height:var(--lh,1.3)}\
.inh{width:inherit;height:unset;padding-top:initial;margin-left:revert;line-height:inherit;text-align:unset;white-space:revert-layer}\
.rel{width:12em;height:3rem;padding:1vw 2vh;margin:1.5lh 0;max-height:calc(100% - 10px);line-height:1.2em}\
.bad{width:nonsense;height:5;padding-top:12;margin-top:5 px;flex-grow:abc;line-height:big;zoom:huge}\
.imp{width:99px!important} .later{width:1px;height:2px!important} .later2{height:3px}\
@layer util{.layered{width:77px;padding-top:9px}} @layer base{.layered{width:66px!important;padding-top:8px}}\
.flex{display:flex;flex-direction:column-reverse;flex-wrap:wrap;align-items:flex-end;justify-content:space-between;flex:2 3 30%}\
.item{align-self:center;justify-self:end;flex-basis:content;flex-grow:2;flex-shrink:0}\
.item2{flex-basis:25px;flex-grow:1.5;flex-shrink:2;flex-basis:40%}\
.grid{display:grid;grid-template-columns:1fr 2fr;gap:3px;place-items:center start;grid-auto-flow:column dense;justify-items:end}\
.tbl{display:table;border-collapse:collapse;table-layout:fixed;border-spacing:3px 4px}\
.txt{white-space:nowrap;overflow-wrap:anywhere;word-break:break-all;text-align:center;line-height:1.5;zoom:150%}\
.txt2{white-space:pre;word-wrap:break-word;line-height:20px;text-align:END;zoom:1.25;line-height:120%}\
.pos{position:absolute;top:10%;left:5px;right:auto;bottom:0;box-sizing:border-box;border:2px solid;border-left-style:none;border-top-width:thin;border-right-width:0}\
.pos2{position:sticky;border-bottom-style:hidden;border-bottom-width:4px;border-top-width:5;border-left-width:50%}\
.up{POSITION:Relative;Display:INLINE-BLOCK;WIDTH:10PX;White-Space:NoWrap;overflow-x:Hidden;Box-Sizing:Border-Box;margin-top:AUTO}\
.cont{width:min-content;height:max-content;max-width:fit-content;min-height:-webkit-fit-content;flex-basis:auto}\
.pct{width:0%;padding-top:0;line-height:120%;flex-basis:0%;margin-left:0}\
.neg{margin-top:-5px;width:+5px;height:.5px;top:5.px;min-width:1e1px;flex-grow:2e0}\
.dn{display:none} .cnt{display:contents} .ib{display:inline-block}\
.sizes{font-size:20px} .sizes2{font-size:2em}\
</style>";

const BODY: &str = "<body style=\"margin:0\">\
<div id=root class=sizes>\
 <div id=m class=lit><span>m</span><div class=item>i</div></div>\
 <div class=var><b>v</b></div><div class=inh><i>i</i></div><div class=rel>r</div><div class=bad>b</div>\
 <div class=\"imp later later2\">imp</div><div class=\"later imp\">imp2</div><div class=layered>l</div>\
 <div class=flex><div class=item>a</div><div class=item2>b</div><p class=cont>c</p></div>\
 <div class=grid><div>1</div><div class=item>2</div></div>\
 <table class=tbl><tr><th>h</th><td class=txt>t</td></tr></table>\
 <div class=txt2>x</div><div class=pos><div class=pos2>p</div></div><div class=up>u</div><div class=pct>%</div><div class=neg>n</div>\
 <div class=dn>hidden</div><div class=cnt><span>contents</span></div>\
 <div id=inl style=\"width:33px;padding:1px!important;--w:9px;height:VAR(--w);margin-top:inherit\">inline</div>\
 <div class=\"lit var\" style=\"width:var(--w)\">both</div>\
 <svg width=\"24\" height=\"24\" style=\"display:inline-block\"><rect width=\"10\" height=\"10\"/></svg>\
 <input class=lit value=x><img class=rel src=x.png><button class=item>b</button><textarea class=txt>t</textarea>\
</div></body>";

#[test]
fn typed_flow_slots_match_the_string_cascade_through_mutations() {
    let mut p = page(&format!("<!doctype html>{CSS}{BODY}"));
    assert_same(&mut p, "initial");
    let m = p.dom.get_element_by_id(p.doc, "m").unwrap().unwrap();
    // Winner transitions: literal → var() → inherit/unset → relative → invalid → !important/layers → case → keywords.
    for class in [
        "var",
        "inh",
        "rel",
        "bad",
        "imp later later2",
        "layered",
        "up",
        "cont",
        "pct",
        "neg",
        "flex item2",
        "txt2 pos2",
        "",
        "lit",
    ] {
        p.dom.set_attribute(m, "class", class).unwrap();
        assert_same(&mut p, &format!("class={class:?}"));
    }
    // Inline style transitions over the same element, including !important and custom properties.
    for style in [
        "width:var(--w)",
        "width:55px;height:20%",
        "width:inherit;padding-top:unset",
        "width:12em;--w:50px",
        "width:nonsense;margin-top:5 px",
        "width:1px!important",
        "WIDTH:7PX;Position:Fixed",
        "",
    ] {
        p.dom.set_attribute(m, "style", style).unwrap();
        assert_same(&mut p, &format!("style={style:?}"));
    }
    p.dom.remove_attribute(m, "style").unwrap();
    assert_same(&mut p, "style removed");
    // The parent's values change what inherits and what em/rem resolve against.
    let root = p.dom.get_element_by_id(p.doc, "root").unwrap().unwrap();
    for class in ["sizes2 txt", "txt2", "lit", ""] {
        p.dom.set_attribute(root, "class", class).unwrap();
        assert_same(&mut p, &format!("root class={class:?}"));
    }
    // Added sheets: a custom property's value, a new literal winner, a new var() winner.
    for css in [
        ":root{--w:70px;--lh:2}",
        ".lit{width:var(--w)}",
        ".lit{width:12px!important}",
        "#m{width:inherit}",
        "@media (min-width:500px){.lit{height:7px}}",
    ] {
        let style = p.dom.create_element(p.doc, "style").unwrap();
        let text = p.dom.create_text(p.doc, css).unwrap();
        p.dom.append_child(style, text).unwrap();
        let body = p.dom.body(p.doc).unwrap().unwrap();
        p.dom.append_child(body, style).unwrap();
        p.layout.style_mut().discover_style_elements(&p.dom);
        assert_same(&mut p, &format!("sheet {css:?}"));
    }
    // Viewport changes resolve vw/vh and @media differently.
    p.layout.set_viewport(Viewport::new(400, 300));
    assert_same(&mut p, "viewport 400x300");
    // Structural changes: a moved element inherits from a new parent; a removed one styles no longer.
    let inl = p.dom.get_element_by_id(p.doc, "inl").unwrap().unwrap();
    p.dom.append_child(m, inl).unwrap();
    assert_same(&mut p, "moved");
    p.dom.remove_child(m, inl).unwrap();
    assert_same(&mut p, "removed");
}

/// Real pages: `FLOW_SNAPSHOT_DIR=dir cargo test -p lp-dom --test typed_flow -- --ignored --nocapture`
/// compares every element of every `*.html` in the directory.
#[test]
#[ignore]
fn typed_flow_slots_match_the_string_cascade_on_snapshots() {
    let Ok(dir) = std::env::var("FLOW_SNAPSHOT_DIR") else { return };
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("snapshot dir")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "html"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no snapshots in {dir}");
    for file in files {
        let html = std::fs::read_to_string(&file).unwrap();
        let mut p = page(&html);
        let n = elements(&p).len();
        assert_same(&mut p, &file.display().to_string());
        eprintln!("{}: {n} elements identical", file.file_name().unwrap().to_string_lossy());
    }
}
