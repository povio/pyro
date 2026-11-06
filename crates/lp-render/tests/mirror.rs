//! Persistent documents (`Live` snapshots): a page's Blitz document is
//! updated from lp-dom's render tree instead of being rebuilt, and must
//! render exactly what a fresh build of the same tree renders.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use lp_dom::parse::{parse_document, DomRef};
use lp_dom::select::parse_selectors;
use lp_dom::snapshot::{RenderHtml, RenderNodeKind, RenderTree};
use lp_dom::{Dom, NodeId, NodeType};
use lp_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn fetcher() -> Arc<dyn Fetcher> {
    Arc::new(NoFetch)
}

struct Page {
    dom: DomRef,
    doc: NodeId,
}

impl Page {
    fn new(html: &str) -> Page {
        let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
        let doc = parse_document(&dom, "http://test/", html);
        Page { dom, doc }
    }
    fn find(&self, css: &str) -> NodeId {
        self.dom.borrow().query_selector(self.doc, &parse_selectors(css).unwrap()).unwrap().unwrap()
    }
    fn opts() -> RenderHtml<'static> {
        RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None }
    }
    fn tree(&self) -> Arc<Tree> {
        Arc::new(convert(self.dom.borrow().render_tree(self.doc, &Self::opts()).unwrap()))
    }
    fn html(&self) -> String {
        self.dom.borrow().render_html(self.doc, &Self::opts()).unwrap()
    }
    fn snapshot(&self, page: u64, tree: Arc<Tree>) -> Snapshot {
        Snapshot {
            base_url: "http://test/".into(),
            width: 640,
            height: 480,
            device_scale_factor: 1.0,
            resource_scope: "doc".into(),
            live: Some(Live { page, tree, clip: None, interaction: None }),
            ..Default::default()
        }
    }
}

/// lp-dom's render tree as lp-render takes it (lp-cdp does the same).
fn convert(t: RenderTree) -> Tree {
    Tree {
        nodes: t
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
    }
}

fn paint(r: &mut Renderer, s: &Snapshot) -> Painted {
    r.paint(s, Area::Viewport, &fetcher()).unwrap()
}

/// Renders the page with the persistent renderer and with a fresh one;
/// returns the persistent render's stats.
#[track_caller]
fn check(live: &mut Renderer, page: &Page, what: &str) -> MirrorStats {
    let tree = page.tree();
    let mirrored = paint(live, &page.snapshot(1, tree.clone()));
    let fresh = paint(&mut Renderer::new(), &page.snapshot(1, tree));
    assert_eq!(mirrored.hash, fresh.hash, "after {what}");
    mirrored.timings.mirror.expect("live")
}

const PAGE: &str = r#"<!doctype html><html><head><style>
  body { margin: 0; font: 14px sans-serif } .row { display: flex; gap: 4px; height: 20px }
  .on { background: #0a0 } .wide { width: 300px } li:nth-child(even) { color: #00f }
  #panel > p + p { margin-top: 10px } .hidden { display: none }
</style></head><body>
<div id="panel"><p class="row">one</p><p class="row on">two</p><p>three</p></div>
<ul id="list"><li>a</li><li>b</li><li>c</li><li>d</li></ul>
<form><input id="name" value="x"><input id="agree" type="checkbox"><textarea id="note">n</textarea>
<select id="pick"><option>a</option><option id="b">b</option></select></form>
<svg id="icon" width="40" height="20"><rect id="bar" width="10" height="20" fill="red"/></svg>
<input id="kind" type="text" value="v">
<table id="t"><tbody><tr><td>1</td><td>2</td></tr><tr><td>3</td><td>4</td></tr></tbody></table>
</body></html>"#;

#[test]
fn each_mutation_kind_matches_a_fresh_build() {
    let page = Page::new(PAGE);
    let mut r = Renderer::new();
    assert_eq!(check(&mut r, &page, "the first render").rebuilt, Some(Rebuild::First));
    let unchanged = check(&mut r, &page, "no change");
    assert_eq!(unchanged, MirrorStats { rebuilt: None, mutations: 0, pages: 1 });

    type Step = Box<dyn Fn(&Page)>;
    let steps: Vec<(&str, Step)> = vec![
        (
            "a class toggle",
            Box::new(|p| {
                let n = p.find("#panel p");
                p.dom.borrow_mut().set_attribute(n, "class", "row on wide").unwrap();
            }),
        ),
        (
            "a style attribute",
            Box::new(|p| {
                let n = p.find("#list");
                p.dom.borrow_mut().set_attribute(n, "style", "padding: 30px").unwrap();
            }),
        ),
        (
            "an attribute removed",
            Box::new(|p| {
                {
                    let n = p.find("#panel p");
                    p.dom.borrow_mut().remove_attribute(n, "class").unwrap();
                };
            }),
        ),
        (
            "a text change",
            Box::new(|p| {
                let n = p.find("#t td");
                p.dom.borrow_mut().set_text_content(n, "one hundred").unwrap();
            }),
        ),
        (
            "an insertion",
            Box::new(|p| {
                let list = p.find("#list");
                let mut d = p.dom.borrow_mut();
                let li = d.create_element(p.doc, "li").unwrap();
                let t = d.create_text(p.doc, "new").unwrap();
                d.append_child(li, t).unwrap();
                let first = d.first_child(list).unwrap();
                d.insert_before(list, li, first).unwrap();
            }),
        ),
        (
            "a removal",
            Box::new(|p| {
                let row = p.find("#t tr");
                let parent = p.dom.borrow().parent(row).unwrap().unwrap();
                p.dom.borrow_mut().remove_child(parent, row).unwrap();
            }),
        ),
        (
            "a move to another parent",
            Box::new(|p| {
                let li = p.find("#list li:last-child");
                let panel = p.find("#panel");
                p.dom.borrow_mut().append_child(panel, li).unwrap();
            }),
        ),
        (
            "a move within the parent",
            Box::new(|p| {
                let last = p.find("#list li:last-child");
                let list = p.find("#list");
                let first = p.dom.borrow().first_child(list).unwrap();
                p.dom.borrow_mut().insert_before(list, last, first).unwrap();
            }),
        ),
        (
            "a typed value",
            Box::new(|p| {
                let n = p.find("#name");
                p.dom.borrow_mut().set_value(n, "typed text").unwrap();
            }),
        ),
        (
            "a checkbox",
            Box::new(|p| {
                let n = p.find("#agree");
                p.dom.borrow_mut().set_checked(n, true).unwrap();
            }),
        ),
        (
            "a textarea",
            Box::new(|p| {
                let n = p.find("#note");
                p.dom.borrow_mut().set_value(n, "line 1\nline 2").unwrap();
            }),
        ),
        (
            "an option",
            Box::new(|p| {
                let n = p.find("#b");
                p.dom.borrow_mut().set_option_selected(n, true).unwrap();
            }),
        ),
        (
            "a <style> edit",
            Box::new(|p| {
                let style = p.find("style");
                let text = p.dom.borrow().text_content(style).unwrap().unwrap() + " p { color: #c00 }";
                p.dom.borrow_mut().set_text_content(style, &text).unwrap();
            }),
        ),
        (
            "an attribute inside an inline svg",
            Box::new(|p| {
                let n = p.find("#bar");
                p.dom.borrow_mut().set_attribute(n, "width", "35").unwrap();
            }),
        ),
        (
            "an input's type",
            Box::new(|p| {
                let n = p.find("#kind");
                p.dom.borrow_mut().set_attribute(n, "type", "checkbox").unwrap();
            }),
        ),
        (
            "a hidden subtree",
            Box::new(|p| {
                let n = p.find("#t");
                p.dom.borrow_mut().set_attribute(n, "class", "hidden").unwrap();
            }),
        ),
        (
            "the checkbox again",
            Box::new(|p| {
                let n = p.find("#agree");
                p.dom.borrow_mut().set_checked(n, false).unwrap();
            }),
        ),
    ];
    for (what, step) in steps {
        step(&page);
        let stats = check(&mut r, &page, what);
        assert_eq!(stats.rebuilt, None, "{what}");
        assert!(stats.mutations > 0, "{what}");
    }
}

/// xorshift: deterministic, no dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<T: Copy>(&mut self, xs: &[T]) -> Option<T> {
        (!xs.is_empty()).then(|| xs[(self.next() % xs.len() as u64) as usize])
    }
}

fn mutate(page: &Page, rng: &mut Rng) -> String {
    let body = page.dom.borrow().body(page.doc).unwrap().unwrap();
    let (elements, texts): (Vec<NodeId>, Vec<NodeId>) = {
        let d = page.dom.borrow();
        let all: Vec<NodeId> = d.descendants(body).unwrap().filter(|&n| n != body).collect();
        let el = all.iter().copied().filter(|&n| d.node_type(n).ok() == Some(NodeType::Element)).collect();
        let tx = all.iter().copied().filter(|&n| d.node_type(n).ok() == Some(NodeType::Text)).collect();
        (el, tx)
    };
    let mut d = page.dom.borrow_mut();
    let classes = ["on", "wide", "hidden", "row", "active", "x"];
    match rng.next() % 8 {
        0 => {
            let Some(e) = rng.pick(&elements) else { return "nothing".to_string() };
            let c = rng.pick(&classes).unwrap();
            let current = d.get_attribute(e, "class").unwrap().unwrap_or("").to_owned();
            let next = if current.split_whitespace().any(|x| x == c) {
                current.split_whitespace().filter(|x| *x != c).collect::<Vec<_>>().join(" ")
            } else {
                format!("{current} {c}")
            };
            d.set_attribute(e, "class", &next).unwrap();
            format!("class {} {current:?} -> {next:?}", d.tag_name(e).unwrap())
        }
        1 => {
            let Some(t) = rng.pick(&texts) else { return "nothing".to_string() };
            d.set_text_content(t, &format!("text {}", rng.next() % 1000)).unwrap();
            "text".to_string()
        }
        2 => {
            let Some(parent) = rng.pick(&elements).or(Some(body)) else { return "nothing".to_string() };
            let parent_tag = d.tag_name(parent).unwrap();
            // A row into a row group, other elements where HTML allows them.
            let row = matches!(parent_tag.as_str(), "TBODY" | "THEAD" | "TFOOT");
            let tag = if row { "tr" } else { rng.pick(&["div", "span", "p", "b"]).unwrap() };
            if !allowed_in(&tag.to_ascii_uppercase(), &parent_tag) {
                return "nothing".to_string();
            }
            let e = d.create_element(page.doc, tag).unwrap();
            let cells = if row { 2 } else { 0 };
            for _ in 0..cells {
                let td = d.create_element(page.doc, "td").unwrap();
                let t = d.create_text(page.doc, "cell").unwrap();
                d.append_child(td, t).unwrap();
                d.append_child(e, td).unwrap();
            }
            if !row {
                let t = d.create_text(page.doc, "inserted").unwrap();
                d.append_child(e, t).unwrap();
            }
            let before = d.children(parent).unwrap().collect::<Vec<_>>();
            let anchor = rng.pick(&before);
            if d.insert_before(parent, e, anchor).is_err() {
                return "nothing".to_string();
            }
            "insert".to_string()
        }
        3 => {
            let Some(e) = rng.pick(&elements) else { return "nothing".to_string() };
            let Some(parent) = d.parent(e).unwrap() else { return "nothing".to_string() };
            d.remove_child(parent, e).unwrap();
            "remove".to_string()
        }
        4 => {
            let (Some(e), Some(to)) = (rng.pick(&elements), rng.pick(&elements)) else { return "nothing".to_string() };
            if !allowed_in(&d.tag_name(e).unwrap(), &d.tag_name(to).unwrap()) || d.append_child(to, e).is_err() {
                return "nothing".to_string();
            }
            format!("move {} into {}", d.tag_name(e).unwrap(), d.tag_name(to).unwrap())
        }
        5 => {
            let Some(e) = rng.pick(&elements) else { return "nothing".to_string() };
            let v = format!("padding-left: {}px", rng.next() % 40);
            d.set_attribute(e, "style", &v).unwrap();
            "style".to_string()
        }
        6 => {
            let Some(e) = rng.pick(&elements) else { return "nothing".to_string() };
            d.set_attribute(e, "data-x", &(rng.next() % 9).to_string()).unwrap();
            "data attribute".to_string()
        }
        _ => {
            // Form state: a typed value or a checkbox, when the page has one.
            let controls: Vec<NodeId> = elements
                .iter()
                .copied()
                .filter(|&e| d.tag_name(e).is_ok_and(|t| t == "INPUT" || t == "TEXTAREA"))
                .collect();
            let Some(e) = rng.pick(&controls) else { return "nothing".to_string() };
            let checkable = d.get_attribute(e, "type").unwrap().is_some_and(|t| t == "checkbox" || t == "radio");
            if checkable {
                let on = rng.next().is_multiple_of(2);
                d.set_checked(e, on).unwrap();
            } else {
                d.set_value(e, &format!("typed {}", rng.next() % 100)).unwrap();
            }
            "form state".to_string()
        }
    }
}

/// Whether HTML lets `child` sit in `parent` as far as tables go (blitz-dom
/// has no anonymous table boxes: table parts out of place lay out
/// differently after an incremental relayout than after a fresh one).
fn allowed_in(child: &str, parent: &str) -> bool {
    match child {
        "TR" => matches!(parent, "TBODY" | "THEAD" | "TFOOT" | "TABLE"),
        "TD" | "TH" => parent == "TR",
        "TBODY" | "THEAD" | "TFOOT" | "CAPTION" | "COLGROUP" => parent == "TABLE",
        _ => !matches!(parent, "TABLE" | "TBODY" | "THEAD" | "TFOOT" | "TR" | "COLGROUP"),
    }
}

#[test]
fn random_mutation_sequences_match_fresh_builds() {
    let fixtures = ["login", "dashboard", "article"];
    // LP_MIRROR_STEPS / LP_MIRROR_SEED: longer runs, other sequences.
    let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u64>().ok());
    let steps = env("LP_MIRROR_STEPS").unwrap_or(if cfg!(debug_assertions) { 8 } else { 40 });
    let seed = env("LP_MIRROR_SEED").unwrap_or(0);
    for (i, name) in fixtures.iter().enumerate() {
        let path = format!("{}/../../bench/screenshot/fixtures/{name}.html", env!("CARGO_MANIFEST_DIR"));
        let page = Page::new(&std::fs::read_to_string(path).unwrap());
        let mut r = Renderer::new();
        check(&mut r, &page, "the first render");
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ (i as u64 + 1) ^ seed.wrapping_mul(0x2545_f491_4f6c_dd1d));
        let mut done = Vec::new();
        for _ in 0..steps {
            done.push(mutate(&page, &mut rng));
            let tree = page.tree();
            let mirrored = paint(&mut r, &page.snapshot(1, tree.clone()));
            let fresh = paint(&mut Renderer::new(), &page.snapshot(1, tree));
            if mirrored.hash != fresh.hash {
                let mut f = Renderer::new();
                paint(&mut f, &page.snapshot(1, page.tree()));
                let (a, b) = (r.mirrored_html(1).unwrap(), f.mirrored_html(1).unwrap());
                let (la, lb) = (r.mirrored_layout(1), f.mirrored_layout(1));
                for i in (0..la.len().min(lb.len())).filter(|&i| la[i] != lb[i]).take(25) {
                    eprintln!("DIFF {i}: {} | {}", la[i], lb[i].trim_start());
                }
                if let Some(i) = (0..la.len().min(lb.len())).find(|&i| la[i] != lb[i]) {
                    eprintln!("first layout difference at box {i}:\n  mirrored {}\n  fresh    {}", la[i], lb[i]);
                    for (k, line) in la.iter().enumerate().take(i + 4).skip(i.saturating_sub(6)) {
                        eprintln!("  {line} | {}", lb.get(k).map(String::as_str).unwrap_or(""));
                    }
                }
                if std::env::var_os("LP_MIRROR_DUMP").is_some() {
                    let (ia, _) = r.raster(mirrored, Backend::Cpu);
                    let (ib, _) = Renderer::new().raster(fresh, Backend::Cpu);
                    eprintln!("{:?}", compare(&ia, &ib, 0, None));
                    std::fs::write("/tmp/claude-mirror-a.png", encode_png(&ia)).unwrap();
                    std::fs::write("/tmp/claude-mirror-b.png", encode_png(&ib)).unwrap();
                    std::fs::write("/tmp/claude-mirror-d.png", encode_png(&diff_image(&ia, &ib, 0))).unwrap();
                    std::fs::write("/tmp/claude-mirror-a.html", &a).unwrap();
                    std::fs::write("/tmp/claude-mirror-b.html", &b).unwrap();
                }
                panic!("{name}: same markup {}; after {done:?}", a == b);
            }
        }
    }
}

#[test]
fn a_tree_builds_what_its_html_parses_to() {
    // The persistent document is built from the tree, not parsed: the
    // pixels must not depend on which.
    for name in ["login", "dashboard", "article"] {
        let path = format!("{}/../../bench/screenshot/fixtures/{name}.html", env!("CARGO_MANIFEST_DIR"));
        let page = Page::new(&std::fs::read_to_string(path).unwrap());
        let live = paint(&mut Renderer::new(), &page.snapshot(1, page.tree()));
        let parsed = Snapshot { html: page.html(), live: None, ..page.snapshot(1, page.tree()) };
        assert_eq!(live.hash, paint(&mut Renderer::new(), &parsed).hash, "{name}");
    }
}

#[test]
fn navigation_large_changes_and_forget_rebuild() {
    let page = Page::new(PAGE);
    let mut r = Renderer::new();
    check(&mut r, &page, "the first render");
    // Another document (a navigation gives the page a new loader id).
    let s = Snapshot { resource_scope: "next".into(), ..page.snapshot(1, page.tree()) };
    assert_eq!(paint(&mut r, &s).timings.mirror.unwrap().rebuilt, Some(Rebuild::Document));
    // Most of the page replaced.
    {
        let mut d = page.dom.borrow_mut();
        let body = d.body(page.doc).unwrap().unwrap();
        let fresh = d.create_element(page.doc, "main").unwrap();
        let t = d.create_text(page.doc, "replaced").unwrap();
        d.append_child(fresh, t).unwrap();
        d.replace_all(body, Some(fresh)).unwrap();
    }
    let s = Snapshot { resource_scope: "next".into(), ..page.snapshot(1, page.tree()) };
    assert_eq!(paint(&mut r, &s).timings.mirror.unwrap().rebuilt, Some(Rebuild::Changes));
    // Pages are independent; a forgotten page starts over.
    assert_eq!(paint(&mut r, &page.snapshot(2, page.tree())).timings.mirror.unwrap().rebuilt, Some(Rebuild::First));
    assert_eq!(r.mirrored_pages(), 2);
    r.forget(1);
    r.forget(2);
    assert_eq!(r.mirrored_pages(), 0);
    let s = Snapshot { resource_scope: "next".into(), ..page.snapshot(1, page.tree()) };
    assert_eq!(paint(&mut r, &s).timings.mirror.unwrap().rebuilt, Some(Rebuild::First));
}

#[test]
fn the_same_tree_is_not_diffed_again() {
    let page = Page::new(PAGE);
    let mut r = Renderer::new();
    let tree = page.tree();
    paint(&mut r, &page.snapshot(1, tree.clone()));
    let again = paint(&mut r, &page.snapshot(1, tree));
    assert_eq!(again.timings.mirror, Some(MirrorStats { rebuilt: None, mutations: 0, pages: 1 }));
    assert_eq!(again.timings.layout_passes, 1);
}

#[test]
fn viewport_and_scroll_changes_apply_to_the_persistent_document() {
    let page = Page::new(&PAGE.replace("</body>", "<div style=\"height: 2000px\"></div></body>"));
    let mut r = Renderer::new();
    let tree = page.tree();
    let fresh = |s: &Snapshot, area: Area| Renderer::new().paint(s, area, &fetcher()).unwrap().hash;
    for s in [
        page.snapshot(1, tree.clone()),
        Snapshot { width: 400, device_scale_factor: 2.0, ..page.snapshot(1, tree.clone()) },
        Snapshot { scroll_y: 300.0, ..page.snapshot(1, tree.clone()) },
        Snapshot { dark: true, ..page.snapshot(1, tree.clone()) },
    ] {
        assert_eq!(r.paint(&s, Area::Viewport, &fetcher()).unwrap().hash, fresh(&s, Area::Viewport));
        // A full-page capture after a scrolled one starts at the top.
        assert_eq!(r.paint(&s, Area::FullPage, &fetcher()).unwrap().hash, fresh(&s, Area::FullPage));
    }
}

#[test]
fn element_captures_find_their_element_by_key() {
    let page = Page::new(PAGE);
    let key = lp_dom::snapshot::render_key(page.find("#t"));
    let mut r = Renderer::new();
    let s = Snapshot {
        live: Some(Live { page: 1, tree: page.tree(), clip: Some(key), interaction: None }),
        ..page.snapshot(1, page.tree())
    };
    let painted = r.paint(&s, Area::Element, &fetcher()).unwrap();
    let html = Snapshot {
        html: page
            .dom
            .borrow()
            .render_html(
                page.doc,
                &RenderHtml {
                    style_text: &|_| None,
                    stylesheet_href: &|_| None,
                    mark: Some((page.find("#t"), CLIP_ATTRIBUTE)),
                },
            )
            .unwrap(),
        live: None,
        ..page.snapshot(1, page.tree())
    };
    let parsed = Renderer::new().paint(&html, Area::Element, &fetcher()).unwrap();
    assert_eq!((painted.width, painted.height), (parsed.width, parsed.height));
    assert_eq!(painted.hash, parsed.hash);
}

#[test]
fn linked_sheet_edits_reach_the_persistent_document() {
    // A CSSOM edit changes a linked sheet's text, not the tree.
    let page = Page::new(
        r#"<!doctype html><html><head><link rel="stylesheet" href="a.css"></head><body><p>x</p></body></html>"#,
    );
    let url = "http://test/a.css".to_string();
    let tree = {
        let href = |_: NodeId| Some(url.clone());
        Arc::new(convert(
            page.dom
                .borrow()
                .render_tree(page.doc, &RenderHtml { style_text: &|_| None, stylesheet_href: &href, mark: None })
                .unwrap(),
        ))
    };
    let snap = |css: &str| {
        let mut s = page.snapshot(1, tree.clone());
        s.sheets.insert(url.clone(), css.into());
        s
    };
    let mut r = Renderer::new();
    let first = paint(&mut r, &snap("p { color: #00f }"));
    let edited = snap("p { color: #f00; padding: 20px }");
    let mirrored = paint(&mut r, &edited);
    assert_ne!(mirrored.hash, first.hash);
    assert_eq!(mirrored.hash, paint(&mut Renderer::new(), &edited).hash);
    assert_eq!(mirrored.timings.mirror.unwrap().rebuilt, None);
}

fn delta_ops(d: lp_dom::snapshot::RenderDelta) -> Vec<DeltaOp> {
    use lp_dom::snapshot::DeltaOp as D;
    let kind = |k: RenderNodeKind| match k {
        RenderNodeKind::Element { name, attrs } => TreeKind::Element { name, attrs },
        RenderNodeKind::Text(t) => TreeKind::Text(t),
    };
    d.ops
        .into_iter()
        .map(|op| match op {
            D::Node { key, kind: k } => DeltaOp::Node { key, kind: kind(k) },
            D::Children { parent, kids, nodes } => DeltaOp::Children {
                parent,
                kids,
                nodes: nodes
                    .into_iter()
                    .map(|n| TreeNode { key: n.key, parent: n.parent, kind: kind(n.kind) })
                    .collect(),
            },
        })
        .collect()
}

/// Runs random mutation sequences on the fixtures, syncing one renderer from
/// lp-dom's change journal (`sync_delta`) and one from the whole tree
/// (`sync_layout`), and calls `check(delta, whole, page, what)` after each.
fn delta_sequences(
    steps_debug: u64,
    steps_release: u64,
    mut check: impl FnMut(&Renderer, &Renderer, &Page, &str),
) -> (u32, u32) {
    let fixtures = ["login", "dashboard", "article"];
    let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u64>().ok());
    let steps = env("LP_MIRROR_STEPS").unwrap_or(if cfg!(debug_assertions) { steps_debug } else { steps_release });
    let seed = env("LP_MIRROR_SEED").unwrap_or(0);
    let (mut applied, mut total) = (0, 0);
    for (i, name) in fixtures.iter().enumerate() {
        let path = format!("{}/../../bench/screenshot/fixtures/{name}.html", env!("CARGO_MANIFEST_DIR"));
        let page = Page::new(&std::fs::read_to_string(path).unwrap());
        let (mut r, mut w) = (Renderer::new(), Renderer::new());
        r.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        w.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        let mut rng = Rng(0x5851_f42d_4c95_7f2d ^ (i as u64 + 1) ^ seed.wrapping_mul(0x2545_f491_4f6c_dd1d));
        let mut done = Vec::new();
        for _ in 0..steps {
            let since = page.dom.borrow().version();
            done.push(mutate(&page, &mut rng));
            total += 1;
            let delta = page.dom.borrow().render_delta(page.doc, since, &Page::opts());
            if delta.is_some_and(|d| r.sync_delta(1, &delta_ops(d), (640, 480, 1.0, false, false), false).is_ok()) {
                applied += 1;
            } else {
                r.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
            }
            w.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
            check(&r, &w, &page, &format!("{name} after {done:?}"));
        }
    }
    (applied, total)
}

/// Every element's box as layout queries report it.
fn boxes(r: &Renderer, page: &Page) -> Vec<(u64, Option<LayoutBox>)> {
    page.tree()
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, TreeKind::Element { .. }))
        .map(|n| (n.key, r.element_box(1, n.key)))
        .collect()
}

/// lp-dom's change journal applied as deltas (`Renderer::sync_delta`, the
/// layout-query path) gives exactly what syncing the whole tree gives.
#[test]
fn random_mutation_deltas_match_whole_tree_syncs() {
    let (applied, total) = delta_sequences(12, 60, |r, w, page, what| {
        assert_eq!(r.mirrored_html(1), w.mirrored_html(1), "markup: {what}");
        assert_eq!(boxes(r, page), boxes(w, page), "boxes: {what}");
    });
    assert!(applied * 2 > total, "most changes sync as deltas ({applied} of {total})");
}

/// Incremental documents report the boxes a fresh build of the same tree
/// does. Known gaps in blitz-dom's incremental layout remain (stale
/// scrollable overflow, inline/anonymous boxes after some moves): run with
/// `--ignored` to see them.
#[test]
#[ignore]
fn incremental_boxes_match_fresh_builds() {
    delta_sequences(12, 60, |r, _, page, what| {
        let mut f = Renderer::new();
        f.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        for ((k, a), (_, b)) in boxes(r, page).into_iter().zip(boxes(&f, page)) {
            assert_eq!(a, b, "box {k:x}: {what}");
        }
    });
}

/// Words of text nodes with their UTF-16 offsets (white space collapsed,
/// several nodes in one line) and caret edges; hit testing by key.
#[test]
fn page_layout_reports_words_and_hits() {
    let page = Page::new(
        "<!doctype html><body style=\"margin:0;font:16px sans-serif\"><p id=p style=\"margin:0\">  Hello   w\u{e9}rld<b id=b>bold</b> \u{1f600}x</p></body>",
    );
    let mut r = Renderer::new();
    r.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
    let out = r.page_layout(1).unwrap();
    let text_key = |sel: &str, i: usize| {
        let dom = page.dom.borrow();
        let el = page.find(sel);
        lp_dom::snapshot::render_key(dom.children(el).unwrap().nth(i).unwrap())
    };
    let words: Vec<(u64, u32, u32)> =
        out.words.iter().map(|(k, w)| (*k, w.edges[0].0, w.edges.last().unwrap().0)).collect();
    let (p0, b0, p2) = (text_key("#p", 0), text_key("#b", 0), text_key("#p", 2));
    assert_eq!(words, vec![(p0, 2, 7), (p0, 10, 15), (b0, 0, 4), (p2, 1, 3), (p2, 3, 4)]);
    let hello = &out.words[0].1;
    assert!(hello.width > 20.0 && hello.height > 10.0 && hello.edges.len() == 6, "{hello:?}");
    assert!(hello.edges.windows(2).all(|e| e[0].1 < e[1].1));
    let bold = &out.words[2].1;
    let hit = r.elements_from_point(1, bold.x + 2.0, bold.y + 2.0);
    let dom = page.dom.borrow();
    assert_eq!(hit[0], lp_dom::snapshot::render_key(page.find("#b")));
    assert_eq!(hit[1], lp_dom::snapshot::render_key(page.find("#p")));
    drop(dom);
}

/// Pagination scripts add a container and fill it in one go: the sync must
/// still be a delta, and give what syncing the whole tree gives.
#[test]
fn a_new_container_filled_in_one_delta_syncs_as_a_delta() {
    let page = Page::new(
        "<!doctype html><html><head><style id=s>.box{height:10px}</style></head><body><div id=a class=box></div></body></html>",
    );
    let (mut r, mut w) = (Renderer::new(), Renderer::new());
    r.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
    let body = page.find("body");
    let steps: [&dyn Fn(&Page); 1] = [&|p: &Page| {
        let mut dom = p.dom.borrow_mut();
        let c = dom.create_element(p.doc, "div").unwrap();
        dom.append_child(body, c).unwrap();
        // Filled after it was inserted, within the same delta.
        let child = dom.create_element(p.doc, "div").unwrap();
        dom.set_attribute(child, "class", "box").unwrap();
        dom.append_child(c, child).unwrap();
    }];
    for (i, step) in steps.iter().enumerate() {
        let since = page.dom.borrow().version();
        step(&page);
        let delta = page.dom.borrow().render_delta(page.doc, since, &Page::opts()).expect("a delta");
        r.sync_delta(1, &delta_ops(delta), (640, 480, 1.0, false, false), false)
            .unwrap_or_else(|e| panic!("step {i}: {e:?}"));
        w.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        assert_eq!(r.mirrored_html(1), w.mirrored_html(1), "markup after step {i}");
        assert_eq!(boxes(&r, &page), boxes(&w, &page), "boxes after step {i}");
    }
}

/// Pagination scripts add a container, fill it and append to a `<style>` in
/// one go: each sync must still be a delta, and give what syncing the whole
/// tree gives.
#[test]
fn a_style_edit_after_a_new_container_syncs_as_deltas() {
    let page = Page::new(
        "<!doctype html><html><head><style id=s>.box{height:10px}</style></head><body><div id=a class=box></div></body></html>",
    );
    let (mut r, mut w) = (Renderer::new(), Renderer::new());
    r.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
    let body = page.find("body");
    let style = page.find("#s");
    let steps: [&dyn Fn(&Page); 4] = [
        &|p: &Page| {
            let mut dom = p.dom.borrow_mut();
            let c = dom.create_element(p.doc, "div").unwrap();
            dom.append_child(body, c).unwrap();
            // Filled after it was inserted, within the same delta.
            let child = dom.create_element(p.doc, "div").unwrap();
            dom.set_attribute(child, "class", "box").unwrap();
            dom.append_child(c, child).unwrap();
        },
        &|p: &Page| {
            p.dom.borrow_mut().set_text_content(style, ".box{height:30px}").unwrap();
        },
        // Its own attributes: a media query that does not match.
        &|p: &Page| {
            p.dom.borrow_mut().set_attribute(style, "media", "print").unwrap();
        },
        &|p: &Page| {
            p.dom.borrow_mut().remove_attribute(style, "media").unwrap();
        },
    ];
    for (i, step) in steps.iter().enumerate() {
        let since = page.dom.borrow().version();
        step(&page);
        let delta = page.dom.borrow().render_delta(page.doc, since, &Page::opts()).expect("a delta");
        r.sync_delta(1, &delta_ops(delta), (640, 480, 1.0, false, false), false)
            .unwrap_or_else(|e| panic!("step {i}: {e:?}"));
        w.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        assert_eq!(r.mirrored_html(1), w.mirrored_html(1), "markup after step {i}");
        assert_eq!(boxes(&r, &page), boxes(&w, &page), "boxes after step {i}");
    }
    let a = r.element_box(1, lp_dom::snapshot::render_key(page.find("#a"))).unwrap();
    assert_eq!(a.height, 30.0, "the edited sheet applies");
}

#[test]
fn textarea_attribute_deltas_match_whole_tree_value_geometry_and_paint() {
    use lp_dom::snapshot::render_key;
    let semantic_tree = |html: &str| {
        let parsed = Page::new(html);
        let mut tree = (*parsed.tree()).clone();
        for node in &mut tree.nodes {
            if let TreeKind::Element { attrs, .. } = &mut node.kind {
                attrs.sort_by_key(|(name, _)| {
                    (name.ns.to_string(), name.local.to_string(), name.prefix.as_ref().map(ToString::to_string))
                });
            }
        }
        tree
    };
    let page = Page::new("<!doctype html><style>body{margin:0}textarea{box-sizing:border-box;width:100px;height:40px;padding:0;border:0}.wide{width:180px}</style><form><textarea id=t>default</textarea></form>");
    let textarea = page.find("#t");
    page.dom.borrow_mut().set_value(textarea, "typed preserved\nline").unwrap();
    let (mut delta_renderer, mut whole_renderer) = (Renderer::new(), Renderer::new());
    delta_renderer.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
    whole_renderer.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
    for (name, value, width, height) in [
        ("class", "wide", 180.0, 40.0),
        ("style", "width:210px;height:60px", 210.0, 60.0),
        ("aria-label", "Updated notes", 210.0, 60.0),
        ("data-tracking", "kept", 210.0, 60.0),
    ] {
        let since = page.dom.borrow().version();
        page.dom.borrow_mut().set_attribute(textarea, name, value).unwrap();
        let delta = page
            .dom
            .borrow()
            .render_delta(page.doc, since, &Page::opts())
            .expect("textarea attributes must sync as deltas");
        delta_renderer.sync_delta(1, &delta_ops(delta), (640, 480, 1.0, false, false), false).unwrap();
        whole_renderer.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        assert_eq!(
            semantic_tree(&delta_renderer.mirrored_html(1).unwrap()),
            semantic_tree(&whole_renderer.mirrored_html(1).unwrap()),
            "semantic attributes, text/value and node order after {name}"
        );
        assert!(delta_renderer.mirrored_html(1).unwrap().contains("typed preserved"));
        assert_eq!(boxes(&delta_renderer, &page), boxes(&whole_renderer, &page), "geometry after {name}");
        let bounds = delta_renderer.element_box(1, render_key(textarea)).unwrap();
        assert_eq!((bounds.width, bounds.height), (width, height), "updated textarea size after {name}");
        assert_eq!(page.dom.borrow().value(textarea).unwrap(), "typed preserved\nline");
        assert_eq!(
            paint(&mut delta_renderer, &page.snapshot(1, page.tree())).hash,
            paint(&mut whole_renderer, &page.snapshot(1, page.tree())).hash,
            "paint after {name}"
        );
    }
}

/// `set_interaction` runs on every geometry read. Blitz's key-event focus falls back to the root
/// element, which once made it report a change, and a whole pass, with nothing focused.
#[test]
fn set_interaction_passes_only_for_a_change() {
    let page = Page::new(PAGE);
    let mut r = Renderer::new();
    check(&mut r, &page, "the first render");
    let key = lp_dom::snapshot::render_key(page.find("#name"));
    let root = lp_dom::snapshot::render_key(page.find("html"));

    for style_only in [false, true] {
        let before = blitz_passes();
        r.set_interaction(1, None, None, style_only);
        assert_eq!(blitz_passes(), before, "nothing focused or hovered (style_only {style_only})");
    }

    let before = blitz_passes();
    r.set_interaction(1, None, Some(key), false);
    assert!(blitz_passes().0 > before.0, "focusing an input");
    let before = blitz_passes();
    r.set_interaction(1, None, Some(key), false);
    assert_eq!(blitz_passes(), before, "the same focus");
    r.set_interaction(1, None, None, false);
    assert!(blitz_passes().0 > before.0, "blurring it");

    // The root element can hold focus itself: not the same as nothing focused.
    let before = blitz_passes();
    r.set_interaction(1, None, Some(root), false);
    assert!(blitz_passes().0 > before.0, "focusing the root element");
    let before = blitz_passes();
    r.set_interaction(1, None, None, false);
    assert!(blitz_passes().0 > before.0, "blurring the root element");
    check(&mut r, &page, "interaction cleared");
}

/// A pass with nothing damaged and the same viewport leaves layout alone; any damage runs it.
#[test]
fn a_pass_without_damage_runs_no_layout() {
    let page = Page::new(PAGE);
    let mut r = Renderer::new();
    check(&mut r, &page, "the first render");
    let before = blitz_passes();
    r.sync_delta(1, &[], (640, 480, 1f64, false, false), false).unwrap();
    let after = blitz_passes();
    assert!(after.0 > before.0, "the sync resolves");
    assert_eq!(after.1, before.1, "with nothing to lay out");

    let n = page.find("#list");
    page.dom.borrow_mut().set_attribute(n, "style", "padding: 13px").unwrap();
    check(&mut r, &page, "a damaging mutation");
    assert!(blitz_passes().1 > after.1, "damage runs layout");
}

// Incremental rounding: a persistent document must round as a fresh build does, whatever a
// relayout moved. Every box paints, and sizes are fractional so that rounding shows.

const PAINT_BOXES: &str =
    "<!doctype html><html><head><style>* { background: rgba(0, 0, 128, .2) }</style></head><body>";

type Mutate = Box<dyn Fn(&Page)>;

fn style(selector: &'static str, value: &'static str) -> (String, Mutate) {
    (
        format!("{selector} {{ {value} }}"),
        Box::new(move |p| {
            let n = p.find(selector);
            p.dom.borrow_mut().set_attribute(n, "style", value).unwrap();
        }),
    )
}

#[track_caller]
fn rounds_as_a_fresh_build(body: &str, steps: Vec<(String, Mutate)>) {
    let page = Page::new(&format!("{PAINT_BOXES}{body}</body></html>"));
    let mut r = Renderer::new();
    check(&mut r, &page, "the first render");
    for (what, mutate) in steps {
        mutate(&page);
        check(&mut r, &page, &what);
    }
}

/// An undamaged block keeps its box beside a float that resizes, while the inline-block inside it
/// moves (x 31 to 50, then 71): inline layout placed it without marking the change.
#[test]
fn rounding_follows_a_float_resizing_beside_an_undamaged_block() {
    rounds_as_a_fresh_build(
        r#"<div id=wrap style="width:300px"><div id=fl style="float:left;width:30.5px;height:60.5px"></div><div id=n><div id=inner style="float:left;width:10.5px;height:5.5px"></div><span id=ib style="display:inline-block;width:8.5px;height:4px"></span>text<div id=deep><span style="display:inline-block;width:6.5px;height:3px"></span></div></div></div>"#,
        vec![
            style("#fl", "float:left;width:50.25px;height:60.5px"),
            style("#fl", "float:left;width:70.75px;height:60.5px"),
            style("#fl", "float:right;width:70.75px;height:60.5px"),
            style("#fl", "float:right;width:70.75px;height:60.5px;display:none"),
            style("#fl", "float:right;width:70.75px;height:60.5px;display:block"),
        ],
    );
}

#[test]
fn rounding_follows_margin_collapsing_through_an_undamaged_block() {
    rounds_as_a_fresh_build(
        r#"<div id=s style="height:10px"></div><div id=n2><div id=c style="margin-top:7.5px;height:5px"><b style="display:block;height:1.5px"></b></div></div><div id=after style="height:3px"><i style="display:block;height:1px"></i></div>"#,
        vec![style("#s", "height:10px;margin-bottom:12.25px"), style("#c", "margin-top:9.25px;height:5px")],
    );
}

#[test]
fn rounding_follows_a_flex_line_resized_by_a_sibling() {
    rounds_as_a_fresh_build(
        r#"<div style="display:flex;align-items:center;width:300px"><div id=tall style="height:40.5px;width:50px"></div><div id=mid style="width:50px"><div style="height:3.5px"><u style="display:block;height:1px"></u></div></div></div>"#,
        vec![style("#tall", "height:50.25px;width:50px"), style("#tall", "height:60.75px;width:50px")],
    );
}

#[test]
fn rounding_follows_floats_flex_grid_and_tables() {
    rounds_as_a_fresh_build(
        r#"<div id=f style="display:flex;width:300.5px"><div id=f1 style="flex:1"><div style="width:50%;height:7.5px"><span>x</span></div></div><div id=f2 style="width:40.3px;order:0">y</div></div><div id=g style="display:grid;grid-template-columns:1fr 2fr;width:200.5px"><div id=g1><div style="height:50px"></div></div><div>b</div></div><table id=t style="width:200.5px"><tr><td id=c1 rowspan=2>a</td><td>b</td></tr><tr><td id=c2>c<br>d</td></tr></table><div id=fl style="float:left;width:30.5px;height:30.5px"></div><p id=pf>beside the float beside the float</p><div id=mh style="min-height:50px;width:30%"><div style="height:100%;width:50%"></div></div>"#,
        vec![
            style("#f2", "width:70.7px;order:0"),
            style("#f2", "width:70.7px;order:-1"),
            style("#f", "display:flex;width:250.25px"),
            style("#g", "display:grid;grid-template-columns:1fr 2fr;width:150.75px"),
            (
                "a longer table cell".into(),
                Box::new(|p| {
                    let n = p.find("#c2");
                    p.dom.borrow_mut().set_text_content(n, "longer long").unwrap();
                }),
            ),
            style("#fl", "float:left;width:60.25px;height:30.5px"),
            style("#fl", "float:left;width:60.25px;height:30.5px;display:none"),
            style("#fl", "float:left;width:60.25px;height:30.5px;display:block"),
            style("#pf", "clear:both"),
        ],
    );
}

#[test]
fn rounding_follows_out_of_flow_boxes() {
    rounds_as_a_fresh_build(
        r#"<div style="position:relative" id=cb><div id=s style="height:10.5px"></div><div id=w><div id=abs style="position:absolute;width:5.5px;height:5.5px"><b style="display:block;height:1.5px"></b></div></div><div id=fix style="position:fixed;bottom:1.5px;right:2.5px;width:10px;height:10px"><div style="height:50%"></div></div></div><div id=rel style="width:100px"><div id=far style="position:absolute;right:0;top:0;width:10%"><div style="height:3.5px"></div></div></div>"#,
        vec![
            style("#s", "height:20.25px"),
            style("#w", "padding-top:3.3px"),
            style("#cb", "position:static"),
            style("#cb", "position:relative"),
            style("#rel", "width:100px;position:relative"),
            style("#rel", "width:210.5px;position:relative"),
            style("#rel", "width:210.5px;position:static"),
        ],
    );
}

#[test]
fn rounding_follows_anonymous_boxes() {
    rounds_as_a_fresh_build(
        r#"<div id=m>text<div id=d style="height:12.5px">block</div>more<span id=sp>inline</span><div id=e>b2</div>tail</div>"#,
        vec![
            style("#d", "height:14.25px"),
            (
                "inline text grows".into(),
                Box::new(|p| {
                    let n = p.find("#sp");
                    p.dom.borrow_mut().set_text_content(n, "inline inline inline").unwrap();
                }),
            ),
            (
                "a block leaves".into(),
                Box::new(|p| {
                    let n = p.find("#d");
                    let parent = p.find("#m");
                    p.dom.borrow_mut().remove_child(parent, n).unwrap();
                }),
            ),
            (
                "a block arrives".into(),
                Box::new(|p| {
                    let mut d = p.dom.borrow_mut();
                    let e = d.create_element(p.doc, "div").unwrap();
                    let t = d.create_text(p.doc, "new").unwrap();
                    d.append_child(e, t).unwrap();
                    drop(d);
                    let (parent, before) = (p.find("#m"), p.find("#sp"));
                    p.dom.borrow_mut().insert_before(parent, e, Some(before)).unwrap();
                }),
            ),
            style("#m", "width:30px"),
        ],
    );
}

fn edit(what: &'static str, f: impl Fn(&Page) + 'static) -> (String, Mutate) {
    (what.into(), Box::new(f))
}

fn class(selector: &'static str, value: &'static str) -> (String, Mutate) {
    (
        format!("{selector} class={value:?}"),
        Box::new(move |p| {
            let n = p.find(selector);
            p.dom.borrow_mut().set_attribute(n, "class", value).unwrap();
        }),
    )
}

/// Paint-only rules for the table tests: restyles that keep the layout styles derived before them.
const PAINT_ONLY: &str = "<style>.hot { color: #c00; background: #eef; outline: 1px solid #f0f } \
    .hot td { color: #060; background: #ffe } .cool { color: #00c; opacity: .8 }</style>";

/// Table parts keep their Taffy styles (and cells their layout styles with collapsed borders) from
/// when their style was written: text edits, rows arriving and leaving, spans, collapsed borders that
/// stop and start being uniform, display changes that make a box a table part and back, and
/// paint-only restyles (class toggles, colour) that keep them.
#[test]
fn tables_lay_out_from_the_styles_written_at_restyle() {
    rounds_as_a_fresh_build(
        &(PAINT_ONLY.to_owned() + r#"<table id=t style="border-collapse:collapse;width:300.5px"><colgroup><col id=col style="width:40.5px"><col></colgroup><tbody id=tb><tr id=r1 style="height:20.5px"><td id=a style="border:1px solid">a</td><td id=b colspan=2 style="height:15.5px;border:2px solid">b</td></tr><tr id=r2><td id=c rowspan=2>c</td><td id=d>d</td><td id=e style="padding:3.5px">e</td></tr><tr id=r3><td>f</td><td>g</td></tr></tbody></table><table id=s style="border-spacing:2.5px"><tr><td id=s1 style="width:30%;height:12.5px">s</td><td id=s2>t</td></tr></table><div id=flip style="width:120.5px"><span id=fc>x</span><span>y</span></div>"#),
        vec![
            class("#r1", "hot"),
            class("#t", "cool"),
            class("#a", "hot"),
            style("#a", "border:1px solid;color:#0c0;background:#fcf"),
            class("#r1", ""),
            edit("a cell's text", |p| {
                let n = p.find("#d");
                p.dom.borrow_mut().set_text_content(n, "a longer cell text that wraps").unwrap();
            }),
            style("#a", "border:3.5px solid"),
            style("#b", "height:25.25px;border:2px solid;width:90.5px"),
            style("#t", "border-collapse:separate;width:300.5px"),
            style("#t", "border-collapse:collapse;width:250.5px"),
            style("#col", "width:60.25px"),
            style("#r1", "height:30.75px"),
            edit("a row arrives", |p| {
                let mut d = p.dom.borrow_mut();
                let tr = d.create_element(p.doc, "tr").unwrap();
                for text in ["h", "i"] {
                    let td = d.create_element(p.doc, "td").unwrap();
                    let t = d.create_text(p.doc, text).unwrap();
                    d.append_child(td, t).unwrap();
                    d.append_child(tr, td).unwrap();
                }
                drop(d);
                let tb = p.find("#tb");
                p.dom.borrow_mut().append_child(tb, tr).unwrap();
            }),
            edit("a row leaves", |p| {
                let (tb, r2) = (p.find("#tb"), p.find("#r2"));
                p.dom.borrow_mut().remove_child(tb, r2).unwrap();
            }),
            edit("a colspan changes", |p| {
                let n = p.find("#b");
                p.dom.borrow_mut().set_attribute(n, "colspan", "1").unwrap();
            }),
            style("#s1", "width:40%;height:18.5px;display:block"),
            style("#s1", "width:40%;height:18.5px"),
            style("#s2", "padding:4.5px;border:1.5px solid"),
            style("#flip", "width:120.5px;display:table"),
            style("#fc", "display:table-cell;height:10.5px"),
            style("#fc", "display:table-cell;height:10.5px;border-collapse:collapse;border:2px solid"),
            style("#flip", "width:140.5px;display:block"),
            style("#fc", "display:inline"),
            class("#tb", "hot"),
            class("#t", ""),
        ],
    );
}

/// The same in a `vertical-rl` table, whose construction transposes the stored physical table-part
/// styles into its writing mode: cell sizes, spans and collapsed borders swap axes.
#[test]
fn vertical_tables_lay_out_from_the_styles_written_at_restyle() {
    rounds_as_a_fresh_build(
        &(PAINT_ONLY.to_owned() + r#"<table id=t style="writing-mode:vertical-rl;border-collapse:collapse;height:260.5px"><colgroup><col id=col style="height:40.5px"><col></colgroup><tbody id=tb><tr id=r1 style="width:20.5px"><td id=a style="border:1px solid">a</td><td id=b colspan=2 style="width:15.5px;border:2px solid">b</td></tr><tr id=r2><td id=c rowspan=2>c</td><td id=d>d</td><td id=e style="padding:3.5px 1.5px">e</td></tr><tr id=r3><td>f</td><td>g</td></tr></tbody></table><table id=s style="writing-mode:vertical-rl;border-spacing:2.5px 1.5px"><tr><td id=s1 style="height:30%;width:12.5px">s</td><td id=s2>t</td></tr></table>"#),
        vec![
            class("#r1", "hot"),
            class("#a", "cool"),
            edit("a cell's text", |p| {
                let n = p.find("#d");
                p.dom.borrow_mut().set_text_content(n, "a longer cell text that wraps").unwrap();
            }),
            style("#a", "border:3.5px solid"),
            style("#b", "width:25.25px;border:2px solid;height:90.5px"),
            style("#t", "writing-mode:vertical-rl;border-collapse:separate;height:260.5px"),
            style("#t", "writing-mode:vertical-rl;border-collapse:collapse;height:220.5px"),
            style("#col", "height:60.25px"),
            style("#r1", "width:30.75px"),
            edit("a row arrives", |p| {
                let mut d = p.dom.borrow_mut();
                let tr = d.create_element(p.doc, "tr").unwrap();
                for text in ["h", "i"] {
                    let td = d.create_element(p.doc, "td").unwrap();
                    let t = d.create_text(p.doc, text).unwrap();
                    d.append_child(td, t).unwrap();
                    d.append_child(tr, td).unwrap();
                }
                drop(d);
                let tb = p.find("#tb");
                p.dom.borrow_mut().append_child(tb, tr).unwrap();
            }),
            edit("a rowspan changes", |p| {
                let n = p.find("#c");
                p.dom.borrow_mut().set_attribute(n, "rowspan", "1").unwrap();
            }),
            style("#s1", "height:40%;width:18.5px"),
            style("#s", "writing-mode:horizontal-tb;border-spacing:2.5px 1.5px"),
            style("#s", "writing-mode:vertical-rl;border-spacing:3.5px"),
            class("#tb", "hot"),
            class("#r1", ""),
        ],
    );
}

#[test]
fn rounding_follows_relative_sizes_under_a_clamped_box() {
    rounds_as_a_fresh_build(
        r#"<div id=outer style="width:400.5px"><div id=cl style="max-width:100.5px;height:50px"><div style="width:50%;height:50%"><div style="position:absolute;width:25%">x</div></div></div></div><div id=vw style="width:50vw;height:10vh"><div style="width:50%">k</div></div><div id=cq style="container-type:inline-size;width:300px"><div style="width:10cqw;height:3px"><u>z</u></div></div>"#,
        vec![
            style("#outer", "width:300.25px"),
            style("#outer", "width:90.25px"),
            style("#cq", "container-type:inline-size;width:220.5px"),
        ],
    );
}

/// A `vertical-rl` box's x is measured from its placer's width, which the logical layout does not
/// show: the skip's key is the physical layout.
#[test]
fn rounding_follows_vertical_writing() {
    rounds_as_a_fresh_build(
        r#"<div id=v style="writing-mode:vertical-rl;height:200.5px;width:300.3px"><div id=a style="width:20.4px">a<span>x</span></div><div id=b style="width:30.3px"><div style="width:10.7px;height:50%"><p>t</p></div></div><div id=c style="width:15px;position:relative"><i style="position:absolute;top:3.3px;right:1.1px">abs</i></div></div>"#,
        vec![
            style("#a", "width:25.9px"),
            style("#v", "writing-mode:vertical-rl;height:200.5px;width:333.7px"),
            style("#v", "writing-mode:vertical-rl;height:150.25px;width:333.7px"),
            style("#b", "width:44px"),
            style("#v", "writing-mode:vertical-lr;height:150.25px;width:333.7px"),
            style("#v", "writing-mode:horizontal-tb;height:150.25px;width:333.7px"),
            style("#v", "writing-mode:vertical-rl;height:150.25px;width:333.7px"),
        ],
    );
}

#[test]
fn rounding_follows_a_vertical_root_and_orthogonal_flows() {
    rounds_as_a_fresh_build(
        r#"<div id=a style="height:30px;width:40.5px">a</div><div id=b style="width:20px"><div style="width:7.5px">x</div></div>"#,
        vec![
            style("body", "writing-mode:vertical-rl"),
            style("#a", "height:30px;width:60.5px"),
            style("body", "writing-mode:horizontal-tb"),
        ],
    );
    rounds_as_a_fresh_build(
        r#"<div id=o style="width:200.5px"><div id=p style="writing-mode:vertical-lr;height:80.5px"><div style="writing-mode:horizontal-tb;width:33.3px"><span>inner</span><div style="height:9.5px"></div></div></div><div id=q style="height:10.5px"></div></div>"#,
        vec![
            style("#q", "height:11.25px"),
            style("#o", "width:150.25px"),
            style("#p", "writing-mode:vertical-lr;height:90.75px"),
        ],
    );
}

/// A viewport change moves `*-rl` boxes and resizes what depends on it, damaged or not.
#[test]
fn rounding_follows_the_viewport() {
    for body in [
        r#"<div id=vw style="width:50vw;height:10vh"><div style="width:50%">k</div></div><div style="width:33%"><span style="display:inline-block;width:7.5px;height:7.5px"></span></div>"#,
        r#"<div style="writing-mode:vertical-rl;width:50.5%;height:40.5vh"><div style="width:20.5px"><p>x</p></div></div>"#,
    ] {
        let page = Page::new(&format!("{PAINT_BOXES}{body}</body></html>"));
        let mut r = Renderer::new();
        let tree = page.tree();
        for width in [640, 777, 1000, 333] {
            let s = Snapshot { width, ..page.snapshot(1, tree.clone()) };
            let fresh = Renderer::new().paint(&s, Area::Viewport, &fetcher()).unwrap().hash;
            assert_eq!(r.paint(&s, Area::Viewport, &fetcher()).unwrap().hash, fresh, "width {width}");
        }
    }
}

const LATO: &[u8] = include_bytes!("../../../bench/screenshot/fixtures/fonts/Lato-Regular.ttf");

/// Serves Lato after a delay, on another thread (as the page's loader does).
struct SlowFont(std::time::Duration);
impl Fetcher for SlowFont {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let delay = self.0;
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            done(if url.ends_with("lato.ttf") { Ok(LATO.to_vec()) } else { Err("404".into()) })
        });
    }
}

/// A web font arriving after the first layout re-flows text with no style change: inline-blocks
/// and floats move inside boxes the first pass already rounded.
#[test]
fn rounding_follows_a_web_font_arriving() {
    let body = r#"<div style="width:311.5px;font:17px Lato, serif">some words <span style="display:inline-block;width:9.5px;height:9.5px"></span> more words that wrap at some point or other <b style="float:right;width:7.5px;height:7.5px"></b> and go on for a while, <u style="position:absolute;width:3px;height:3px"></u>further</div>"#;
    let page = Page::new(&format!("{PAINT_BOXES}{body}</body></html>"));
    let slow: Arc<dyn Fetcher> = Arc::new(SlowFont(std::time::Duration::from_millis(300)));

    let mut r = Renderer::new();
    r.first_layout_wait = std::time::Duration::from_millis(20);
    let before = r.paint(&page.snapshot(1, page.tree()), Area::Viewport, &slow).unwrap();

    let sheet = page.find("style");
    let text = "@font-face { font-family: Lato; src: url(lato.ttf); } * { background: rgba(0, 0, 128, .2) }";
    page.dom.borrow_mut().set_text_content(sheet, text).unwrap();
    let live = r.paint(&page.snapshot(1, page.tree()), Area::Viewport, &slow).unwrap();
    assert_eq!(live.timings.layout_passes, 2, "laid out with the fallback font, then with Lato");
    assert_ne!(live.hash, before.hash, "Lato changes the layout");

    let mut fresh = Renderer::new();
    let fresh = fresh.paint(&page.snapshot(1, page.tree()), Area::Viewport, &slow).unwrap();
    assert_eq!(fresh.timings.layout_passes, 1);
    assert_eq!(live.hash, fresh.hash);
}

/// The skip disappeared unnoticed once (see vendor/taffy/README.lightningpanda.md): a local change
/// must leave most of the tree unvisited.
#[test]
fn rounding_skips_what_a_change_did_not_touch() {
    let rows: String = (0..200).map(|i| format!("<div class=row id=r{i}><span>row {i}</span><p>x</p></div>")).collect();
    let page = Page::new(&format!("{PAINT_BOXES}{rows}</body></html>"));
    let mut r = Renderer::new();

    let start = blitz_rounding();
    check(&mut r, &page, "the first render");
    let first = blitz_rounding();
    // `check`'s fresh build rounds the same tree, as a first pass.
    let whole = (first.0 - start.0) / 2;
    assert!(whole > 400, "{whole} nodes");
    assert_eq!(first.1, start.1, "a first pass has nothing to skip");

    let last = page.find("#r199");
    page.dom.borrow_mut().set_attribute(last, "style", "height: 33.5px").unwrap();
    let tree = page.tree();
    let before = blitz_rounding();
    let painted = paint(&mut r, &page.snapshot(1, tree.clone()));
    let after = blitz_rounding();
    assert_eq!(painted.hash, Renderer::new().paint(&page.snapshot(1, tree), Area::Viewport, &fetcher()).unwrap().hash);
    assert!(after.1 - before.1 >= 190, "skipped {} subtrees", after.1 - before.1);
    assert!(after.0 - before.0 < whole / 2, "reached {} of {whole} nodes", after.0 - before.0);
}
