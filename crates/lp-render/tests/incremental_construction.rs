//! Box construction damage stops at the box that owns it: after a change, an
//! incremental relayout of the persistent document must give the boxes and
//! pixels of a document built from scratch (every node damaged), while
//! re-collecting only the containers the change can reach.
use lp_dom::parse::{parse_document, DomRef};
use lp_dom::select::parse_selectors;
use lp_dom::snapshot::{RenderHtml, RenderNodeKind};
use lp_dom::{Dom, NodeId};
use lp_render::*;
use std::{cell::RefCell, rc::Rc, sync::Arc};

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

struct Page {
    dom: DomRef,
    doc: NodeId,
}

impl Page {
    fn new(html: &str) -> Self {
        let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
        let doc = parse_document(&dom, "http://test/", html);
        Self { dom, doc }
    }
    fn find(&self, css: &str) -> NodeId {
        self.dom.borrow().query_selector(self.doc, &parse_selectors(css).unwrap()).unwrap().unwrap()
    }
    fn text(&self, css: &str) -> NodeId {
        let el = self.find(css);
        self.dom.borrow().first_child(el).unwrap().unwrap()
    }
    fn snapshot(&self) -> Snapshot {
        let opts = RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None };
        let tree = self.dom.borrow().render_tree(self.doc, &opts).unwrap();
        let tree = Tree {
            nodes: tree
                .nodes
                .into_iter()
                .map(|n| TreeNode {
                    key: n.key,
                    parent: n.parent,
                    kind: match n.kind {
                        RenderNodeKind::Element { name, attrs } => TreeKind::Element { name, attrs },
                        RenderNodeKind::Text(t) => TreeKind::Text(t),
                    },
                })
                .collect(),
        };
        Snapshot {
            base_url: "http://test/".into(),
            width: 640,
            height: 480,
            device_scale_factor: 1.0,
            resource_scope: "doc".into(),
            live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
            ..Default::default()
        }
    }
}

fn paint(r: &mut Renderer, s: &Snapshot) -> Painted {
    r.paint(s, Area::Viewport, &(Arc::new(NoFetch) as Arc<dyn Fetcher>)).unwrap()
}

/// Applies `change` to `html`, relayouts incrementally, and compares with a
/// from-scratch build (every node damaged). Returns the layout-children
/// collections and table context builds the incremental relayout made.
#[track_caller]
fn check(what: &str, html: &str, change: &dyn Fn(&Page)) -> (u64, u64) {
    check_with(what, html, change, true)
}

/// `check`, comparing boxes only when `boxes`: an element that stops having a
/// box of its own (it becomes `display: contents`, or inline content of an
/// inline root) keeps its last `final_layout`, incremental construction or not.
#[track_caller]
fn check_with(what: &str, html: &str, change: &dyn Fn(&Page), boxes: bool) -> (u64, u64) {
    let page = Page::new(html);
    let mut live = Renderer::new();
    paint(&mut live, &page.snapshot());
    change(&page);
    let snap = page.snapshot();
    let before = blitz_constructions();
    let incremental = paint(&mut live, &snap);
    let after = blitz_constructions();
    assert_eq!(incremental.timings.mirror.unwrap().rebuilt, None, "{what}: synced as changes");
    let mut fresh = Renderer::new();
    let scratch = paint(&mut fresh, &snap);
    let (a, b) = (live.mirrored_layout(1), fresh.mirrored_layout(1));
    for (i, (x, y)) in a.iter().zip(&b).enumerate().filter(|_| boxes) {
        assert_eq!(x, y, "{what}: box {i} differs\n{}", a.join("\n"));
    }
    assert_eq!(a.len(), b.len(), "{what}: box count");
    assert_eq!(incremental.hash, scratch.hash, "{what}: pixels");
    (after.0 - before.0, after.1 - before.1)
}

const TABLE: &str = r#"<!doctype html><style id=s>body{margin:0;font:14px sans-serif} td{padding:2px}</style>
<p>before</p><table id=t><tbody id=tb><tr id=r1><td id=c1>one</td><td>two</td></tr><tr><td>three</td><td>four</td></tr></tbody></table><p>after</p>"#;

#[test]
fn text_inside_a_cell_does_not_rebuild_the_table() {
    let (collected, tables) = check("text in a td", TABLE, &|p| {
        let t = p.text("#c1");
        p.dom.borrow_mut().replace_data(t, 0, 3, "a much longer first cell").unwrap();
    });
    assert_eq!(tables, 0, "the cell's own content does not change the table's grid");
    assert_eq!(collected, 1, "only the cell is re-collected");
}

#[test]
fn rows_and_cells_inserted_rebuild_the_table() {
    let (_, tables) = check("a tr inserted", TABLE, &|p| {
        let tb = p.find("#tb");
        let mut d = p.dom.borrow_mut();
        let tr = d.create_element(p.doc, "tr").unwrap();
        for text in ["five", "six", "seven"] {
            let td = d.create_element(p.doc, "td").unwrap();
            let t = d.create_text(p.doc, text).unwrap();
            d.append_child(td, t).unwrap();
            d.append_child(tr, td).unwrap();
        }
        d.append_child(tb, tr).unwrap();
    });
    assert_eq!(tables, 1);
    let (_, tables) = check("a td inserted", TABLE, &|p| {
        let r1 = p.find("#r1");
        let mut d = p.dom.borrow_mut();
        let td = d.create_element(p.doc, "td").unwrap();
        let t = d.create_text(p.doc, "extra column").unwrap();
        d.append_child(td, t).unwrap();
        d.append_child(r1, td).unwrap();
    });
    assert_eq!(tables, 1);
    let (_, tables) = check("an abspos child added to a tr", TABLE, &|p| {
        let r1 = p.find("#r1");
        let mut d = p.dom.borrow_mut();
        let div = d.create_element(p.doc, "div").unwrap();
        d.set_attribute(div, "style", "position:absolute;top:0;left:0;width:30px;height:30px").unwrap();
        d.append_child(r1, div).unwrap();
    });
    assert_eq!(tables, 1);
    check("a tr removed", TABLE, &|p| {
        let tb = p.find("#tb");
        let r1 = p.find("#r1");
        p.dom.borrow_mut().remove_child(tb, r1).unwrap();
    });
}

#[test]
fn a_stylesheet_change_to_table_parts_rebuilds_the_table() {
    // Cell width, padding and border, row heights and the table's own style
    // are copied into the table's context: a restyle that only relayouts
    // them must still rebuild it.
    for (what, css) in [
        ("td width", "td{padding:6px 12px;width:150px;border:3px solid}"),
        ("tr height", "td{padding:2px} tr{height:60px}"),
        ("table border-spacing", "td{padding:2px} table{border-spacing:9px}"),
    ] {
        let (_, tables) = check(what, TABLE, &|p| {
            let s = p.find("#s");
            let css = format!("body{{margin:0;font:14px sans-serif}} {css}");
            p.dom.borrow_mut().set_text_content(s, &css).unwrap();
        });
        assert_eq!(tables, 1, "{what}");
    }
}

#[test]
fn list_items_inserted_below_a_list() {
    let html = r#"<!doctype html><style>body{margin:0;font:14px sans-serif}</style>
<ol id=o><li>a</li><div id=d><li>b</li></div><li>c</li></ol>
<ul><li id=y>y</li><li>z</li></ul>"#;
    check("an li inserted inside a div inside an ol", html, &|p| {
        let d = p.find("#d");
        let mut dom = p.dom.borrow_mut();
        let li = dom.create_element(p.doc, "li").unwrap();
        let t = dom.create_text(p.doc, "new").unwrap();
        dom.append_child(li, t).unwrap();
        let first = dom.first_child(d).unwrap();
        dom.insert_before(d, li, first).unwrap();
    });
    check("an li appended to an ol", html, &|p| {
        let o = p.find("#o");
        let mut dom = p.dom.borrow_mut();
        let li = dom.create_element(p.doc, "li").unwrap();
        let t = dom.create_text(p.doc, "last").unwrap();
        dom.append_child(li, t).unwrap();
        dom.append_child(o, li).unwrap();
    });
    check("text inside an li", html, &|p| {
        let t = p.text("#y");
        p.dom.borrow_mut().replace_data(t, 0, 1, "a longer item").unwrap();
    });
}

const INLINE: &str = r#"<!doctype html><style>body{margin:0;font:14px sans-serif} .blk{display:block} .ib{display:inline-block}</style>
<div id=outer><p id=p>start <b id=b>bold <i id=i>italic</i></b> end</p>
<div id=mixed><p>block</p>loose <em id=em>text</em></div>
<p>a <span class=ib id=ib>inline block</span> b</p></div>"#;

#[test]
fn inline_changes_reach_their_inline_root() {
    check("text inside nested inline boxes", INLINE, &|p| {
        let t = p.text("#i");
        p.dom.borrow_mut().replace_data(t, 0, 6, "a far longer italic run of text").unwrap();
    });
    check("an inline child appended to an inline box", INLINE, &|p| {
        let b = p.find("#b");
        let mut d = p.dom.borrow_mut();
        let u = d.create_element(p.doc, "u").unwrap();
        let t = d.create_text(p.doc, " underlined").unwrap();
        d.append_child(u, t).unwrap();
        d.append_child(b, u).unwrap();
    });
    check("text inside an inline box in an anonymous block", INLINE, &|p| {
        let t = p.text("#em");
        p.dom.borrow_mut().replace_data(t, 0, 4, "words that wrap differently").unwrap();
    });
    check("text inside an inline-block", INLINE, &|p| {
        let t = p.text("#ib");
        p.dom.borrow_mut().replace_data(t, 0, 6, "a wider").unwrap();
    });
    check("a span switching to display:block", INLINE, &|p| {
        let i = p.find("#i");
        p.dom.borrow_mut().set_attribute(i, "class", "blk").unwrap();
    });
    check("a block appended inside an inline box", INLINE, &|p| {
        let b = p.find("#b");
        let mut d = p.dom.borrow_mut();
        let div = d.create_element(p.doc, "div").unwrap();
        let t = d.create_text(p.doc, "block in inline").unwrap();
        d.append_child(div, t).unwrap();
        d.append_child(b, div).unwrap();
    });
}

#[test]
fn display_contents_forwards_to_its_box_parent() {
    let html = r#"<!doctype html><style>body{margin:0;font:14px sans-serif} .c{display:contents} .flex{display:flex;gap:4px}</style>
<div class=flex id=f><span>x</span><div class=c id=c><span>y</span></div></div>
<div id=flow><div class=c id=c2>loose text</div><p>block</p></div>"#;
    check("a child appended to a contents element in a flex container", html, &|p| {
        let c = p.find("#c");
        let mut d = p.dom.borrow_mut();
        let t = d.create_text(p.doc, "bare text").unwrap();
        d.append_child(c, t).unwrap();
    });
    check("text inside a contents element in a flow container", html, &|p| {
        let t = p.text("#c2");
        p.dom.borrow_mut().replace_data(t, 0, 5, "much more loose").unwrap();
    });
    check_with(
        "an element switching to display:contents",
        html,
        &|p| {
            let f = p.find("#f");
            let span = p.dom.borrow().first_child(f).unwrap().unwrap();
            p.dom.borrow_mut().set_attribute(span, "class", "c").unwrap();
        },
        false,
    );
    check_with(
        "an element switching from display:contents",
        html,
        &|p| {
            let c = p.find("#c");
            p.dom.borrow_mut().remove_attribute(c, "class").unwrap();
        },
        false,
    );
    check("a contents element's child switching to display:block", html, &|p| {
        let c = p.find("#c2");
        let mut d = p.dom.borrow_mut();
        let b = d.create_element(p.doc, "b").unwrap();
        let t = d.create_text(p.doc, "bold").unwrap();
        d.append_child(b, t).unwrap();
        d.append_child(c, b).unwrap();
    });
}

#[test]
fn generated_content_changes() {
    let html = r#"<!doctype html><style>body{margin:0;font:14px sans-serif} .on::before{content:"generated words "}</style>
<div><p id=p>text <b id=b>bold</b></p></div>"#;
    check("::before added", html, &|p| {
        let n = p.find("#p");
        p.dom.borrow_mut().set_attribute(n, "class", "on").unwrap();
    });
    check("::before added on an inline box", html, &|p| {
        let n = p.find("#b");
        p.dom.borrow_mut().set_attribute(n, "class", "on").unwrap();
    });
}

#[test]
fn an_appended_block_reconstructs_only_its_container() {
    // What pagination does: append a block to a container deep in the page.
    let html = r#"<!doctype html><style>body{margin:0;font:14px sans-serif}</style>
<main><section><article id=a><p>one</p><p>two</p></article></section><aside><p>side</p></aside></main>"#;
    let (collected, _) = check("a paragraph appended", html, &|p| {
        let a = p.find("#a");
        let mut d = p.dom.borrow_mut();
        let para = d.create_element(p.doc, "p").unwrap();
        let t = d.create_text(p.doc, "three").unwrap();
        d.append_child(para, t).unwrap();
        d.append_child(a, para).unwrap();
    });
    // The article and the new paragraph, not every ancestor up to the root.
    assert_eq!(collected, 2);
}
