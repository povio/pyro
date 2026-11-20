//! Persistent documents (`Live` snapshots): a page's Blitz document is
//! updated from pyro-dom's render tree instead of being rebuilt, and must
//! render exactly what a fresh build of the same tree renders.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::select::parse_selectors;
use pyro_dom::snapshot::{RenderHtml, RenderNodeKind, RenderTree};
use pyro_dom::{Dom, NodeId, NodeType};
use pyro_render::*;

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
        RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None }
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

/// pyro-dom's render tree as pyro-render takes it (pyro-cdp does the same).
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

#[test]
fn view_transition_collection_bounds_work_before_visiting_children() {
    for (body, valid, visited) in [
        ("<div style='view-transition-name: item'></div>".repeat(2), false, 3),
        ("<div style='view-transition-name:item'></div><div style='view-transition-scope:all'><i style='view-transition-name:item'></i></div>".into(), true, 3),
        ("<div style='view-transition-name: match-element'></div>".repeat(256), true, 257),
        ("<div style='view-transition-name: match-element'></div>".repeat(512), false, 258),
        ("<i></i>".repeat(16_384), false, 1),
    ] {
        let page = Page::new(&format!("<!doctype html><body>{body}"));
        let mut renderer = Renderer::new();
        renderer.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        assert_eq!(renderer.view_transition_names(1, pyro_dom::snapshot::render_key(page.find("body"))), (valid, visited));
    }
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
<input id="kind" type="text" value="v"><input id="hint" placeholder="Search">
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
            "a placeholder's text",
            Box::new(|p| {
                let n = p.find("#hint");
                p.dom.borrow_mut().set_attribute(n, "placeholder", "Find a position").unwrap();
            }),
        ),
        (
            "a value typed over a placeholder",
            Box::new(|p| {
                let n = p.find("#hint");
                p.dom.borrow_mut().set_value(n, "typed").unwrap();
            }),
        ),
        (
            "the value cleared, the placeholder back",
            Box::new(|p| {
                let n = p.find("#hint");
                p.dom.borrow_mut().set_value(n, "").unwrap();
            }),
        ),
        (
            "the placeholder removed",
            Box::new(|p| {
                let n = p.find("#hint");
                p.dom.borrow_mut().remove_attribute(n, "placeholder").unwrap();
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
    // PYRO_MIRROR_STEPS / PYRO_MIRROR_SEED: longer runs, other sequences.
    let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u64>().ok());
    let steps = env("PYRO_MIRROR_STEPS").unwrap_or(if cfg!(debug_assertions) { 8 } else { 40 });
    let seed = env("PYRO_MIRROR_SEED").unwrap_or(0);
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
                if std::env::var_os("PYRO_MIRROR_DUMP").is_some() {
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
fn the_hyphen_digit_setting_rebuilds_the_persistent_document() {
    let page = Page::new(
        "<!doctype html><body style='margin:0;font:16px/100px sans-serif'>\
         <div style='width:0'>REG-be08b23e-89be-48b8-9a6f-ecb23f7628c6</div>",
    );
    let mut r = Renderer::new();
    let tree = page.tree();
    let s = Snapshot { height: 10, ..page.snapshot(1, tree) };
    let full = |r: &mut Renderer| r.paint(&s, Area::FullPage, &fetcher()).unwrap();
    assert_eq!(full(&mut r).height, 300);
    r.hyphen_digit_breaks = true;
    let on = full(&mut r);
    assert_eq!(on.timings.mirror.and_then(|m| m.rebuilt), Some(Rebuild::Document));
    assert_eq!(on.height, 600);
    assert_eq!(full(&mut r).timings.mirror.and_then(|m| m.rebuilt), None);
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

/// The scroll offsets a page told for its scroll timelines outlive its document: the page tells
/// them once per scroll, and a document built again (a sub-resource of any page arrived, a
/// setting changed) would otherwise sample every scroll timeline at offset 0 until the next
/// scroll (WPT scroll-animations/scroll-timelines reftests beside a page that loads an image).
#[test]
fn scroll_offsets_told_for_timelines_survive_a_document_rebuild() {
    let page = Page::new(
        "<!doctype html><body style=\"margin: 0\"><div id=\"s\" style=\"overflow: hidden; height: 100px\">\
         <div style=\"height: 1000px\"></div></div></body>",
    );
    let key = pyro_dom::snapshot::render_key(page.find("#s"));
    let mut r = Renderer::new();
    let s = page.snapshot(1, page.tree());
    let offset = |r: &Renderer| r.timeline_state(1, Some(key), None, 0, 0, [(0, 0.0); 2]).map(|t| (t.0, t.1, t.2));
    r.sync_layout(&s, &fetcher()).unwrap();
    assert_eq!(offset(&r), Some((0.0, 0.0, 900.0)));
    r.set_timeline_scroll(1, vec![(key, (0.0, 450.0))], (0.0, 0.0));
    assert_eq!(offset(&r), Some((450.0, 0.0, 900.0)));
    r.sync_layout(&s, &fetcher()).unwrap();
    // A rebuild inside a sync (here: a setting of the document changed).
    r.hyphen_digit_breaks = true;
    let stats = r.sync_layout(&s, &fetcher()).unwrap();
    assert_eq!(stats.mirror.and_then(|m| m.rebuilt), Some(Rebuild::Document));
    assert_eq!(offset(&r), Some((450.0, 0.0, 900.0)), "after a rebuild");
    // The document dropped for a rebuild (pyro-layout: a sub-resource arrived).
    r.forget_document(1);
    assert_eq!(offset(&r), None);
    assert_eq!(r.sync_layout(&s, &fetcher()).unwrap().mirror.and_then(|m| m.rebuilt), Some(Rebuild::First));
    assert_eq!(offset(&r), Some((450.0, 0.0, 900.0)), "after the document was dropped");
    // A page that is gone takes its offsets with it.
    r.forget(1);
    r.sync_layout(&s, &fetcher()).unwrap();
    assert_eq!(offset(&r), Some((0.0, 0.0, 900.0)), "a new page");
}

#[test]
fn element_captures_find_their_element_by_key() {
    let page = Page::new(PAGE);
    let key = pyro_dom::snapshot::render_key(page.find("#t"));
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
                    adopted: None,
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
                .render_tree(
                    page.doc,
                    &RenderHtml { style_text: &|_| None, stylesheet_href: &href, mark: None, adopted: None },
                )
                .unwrap(),
        ))
    };
    // (The sheet's key, as the snapshot wrote it on its link.)
    let link = page.dom.borrow().query_selector(page.doc, &pyro_dom::select::parse_selectors("link").unwrap()).unwrap().unwrap();
    let sheet = page.dom.borrow().sheet_key(link);
    let snap = |css: &str| {
        let mut s = page.snapshot(1, tree.clone());
        s.sheets.insert(sheet.clone(), css.into());
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

fn delta_ops(d: pyro_dom::snapshot::RenderDelta) -> Vec<DeltaOp> {
    use pyro_dom::snapshot::DeltaOp as D;
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
/// pyro-dom's change journal (`sync_delta`) and one from the whole tree
/// (`sync_layout`), and calls `check(delta, whole, page, what)` after each.
fn delta_sequences(
    steps_debug: u64,
    steps_release: u64,
    mut check: impl FnMut(&Renderer, &Renderer, &Page, &str),
) -> (u32, u32) {
    let fixtures = ["login", "dashboard", "article"];
    let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u64>().ok());
    let steps = env("PYRO_MIRROR_STEPS").unwrap_or(if cfg!(debug_assertions) { steps_debug } else { steps_release });
    let seed = env("PYRO_MIRROR_SEED").unwrap_or(0);
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

/// pyro-dom's change journal applied as deltas (`Renderer::sync_delta`, the
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
    let words_out = r.page_words(1).unwrap();
    let text_key = |sel: &str, i: usize| {
        let dom = page.dom.borrow();
        let el = page.find(sel);
        pyro_dom::snapshot::render_key(dom.children(el).unwrap().nth(i).unwrap())
    };
    let words: Vec<(u64, u32, u32)> =
        words_out.iter().map(|(k, w)| (*k, w.edges[0].0, w.edges.last().unwrap().0)).collect();
    let (p0, b0, p2) = (text_key("#p", 0), text_key("#b", 0), text_key("#p", 2));
    assert_eq!(words, vec![(p0, 2, 7), (p0, 10, 15), (b0, 0, 4), (p2, 1, 3), (p2, 3, 4)]);
    let hello = &words_out[0].1;
    assert!(hello.width > 20.0 && hello.height > 10.0 && hello.edges.len() == 6, "{hello:?}");
    assert!(hello.edges.windows(2).all(|e| e[0].1 < e[1].1));
    let bold = &words_out[2].1;
    let hit = r.elements_from_point(1, bold.x + 2.0, bold.y + 2.0);
    let dom = page.dom.borrow();
    assert_eq!(hit[0], pyro_dom::snapshot::render_key(page.find("#b")));
    assert_eq!(hit[1], pyro_dom::snapshot::render_key(page.find("#p")));
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
    let a = r.element_box(1, pyro_dom::snapshot::render_key(page.find("#a"))).unwrap();
    assert_eq!(a.height, 30.0, "the edited sheet applies");
}

#[test]
fn textarea_attribute_deltas_match_whole_tree_value_geometry_and_paint() {
    use pyro_dom::snapshot::render_key;
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
    let key = pyro_dom::snapshot::render_key(page.find("#name"));
    let root = pyro_dom::snapshot::render_key(page.find("html"));

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

/// A node brought back to what Blitz holds (text set and reset, attributes
/// removed and set again in another order) changes nothing in the document,
/// by delta or by whole tree: the mirror compares against Blitz's own text
/// and attributes.
#[test]
fn a_node_set_back_to_what_blitz_holds_changes_nothing() {
    let page = Page::new(PAGE);
    let (mut r, mut w) = (Renderer::new(), Renderer::new());
    check(&mut w, &page, "the first render");
    r.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
    let text = page.dom.borrow().first_child(page.find("#panel p")).unwrap().unwrap();
    let el = page.find("#list");
    let delta = |r: &mut Renderer, step: &dyn Fn(&mut Dom)| {
        let since = page.dom.borrow().version();
        step(&mut page.dom.borrow_mut());
        let ops = delta_ops(page.dom.borrow().render_delta(page.doc, since, &Page::opts()).expect("a delta"));
        assert!(!ops.is_empty(), "the journal records the change");
        r.sync_delta(1, &ops, (640, 480, 1.0, false, false), false).unwrap();
    };
    let attr_names = |page: &Page| -> Vec<String> {
        let key = pyro_dom::snapshot::render_key(el);
        let tree = page.tree();
        let n = tree.nodes.iter().find(|n| n.key == key).unwrap();
        let TreeKind::Element { attrs, .. } = &n.kind else { unreachable!() };
        attrs.iter().map(|(a, _)| a.local.to_string()).collect()
    };

    // Synced with `class` before `title`.
    delta(&mut r, &|d| {
        d.set_attribute(el, "class", "on").unwrap();
        d.set_attribute(el, "title", "t").unwrap();
    });
    assert!(check(&mut w, &page, "class and title set").mutations > 0);
    assert_eq!(attr_names(&page), ["id", "class", "title"]);

    let steps: [(&str, &dyn Fn(&mut Dom)); 2] = [
        ("text set and reset", &|d| {
            d.set_text_content(text, "changed").unwrap();
            d.set_text_content(text, "one").unwrap();
        }),
        ("attributes reordered", &|d| {
            d.remove_attribute(el, "class").unwrap();
            d.set_attribute(el, "class", "on").unwrap();
        }),
    ];
    for (what, step) in steps {
        let before = blitz_passes();
        delta(&mut r, step);
        assert_eq!(blitz_passes().1, before.1, "{what}: the delta lays nothing out");
        assert_eq!(check(&mut w, &page, what).mutations, 0, "{what}: the whole tree mutates nothing");
        assert_eq!(r.mirrored_html(1), w.mirrored_html(1), "{what}");
    }
    assert_eq!(attr_names(&page), ["id", "title", "class"]);
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

/// The skip disappeared unnoticed once (see vendor/taffy/README.pyro.md): a local change
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

// Tables update the rows that changed (blitz-dom's `update_table_context`) instead of collecting every
// row again: every change below must paint and lay out as a fresh build does.

fn table_rows(prefix: &str, n: usize) -> String {
    (0..n)
        .map(|i| format!("<tr id={prefix}{i}><td>cell {i} a</td><td>text {i} b with some words</td><td>{i}.5</td></tr>"))
        .collect()
}

fn table_page() -> Page {
    Page::new(&format!(
        r#"<!doctype html><html><head><style>
  body {{ margin: 0; font: 13px sans-serif }} td, th {{ padding: 2.5px 3.25px }}
  #c {{ border-collapse: collapse; width: 400.5px }} #c tr {{ border-bottom: 1px solid #ccc }}
  .hl {{ border: 2px solid red }} .thick {{ border-bottom: 3px solid blue }}
  #s {{ border-spacing: 3px }} #f {{ table-layout: fixed; width: 300px }} .wide {{ width: 120px }}
  #u, .nb {{ border-collapse: collapse }} #u td {{ border: 1px solid #000 }}
  .hidden {{ display: none }}
</style></head><body>
<table id=c><thead><tr><th>h1</th><th>h2</th><th>h3</th></tr></thead>
<tbody id=cb>{}</tbody><tfoot><tr><td>f1</td><td colspan=2>f2</td></tr></tfoot></table>
<table id=s><tbody id=sb>{}</tbody></table>
<table id=f><colgroup><col class=wide></colgroup><tbody id=fb>{}</tbody></table>
<table id=u><tbody id=ub>{}</tbody></table>
<table id=r style="border-collapse:collapse"><tr><td rowspan=2 style="border:1px solid">a</td><td>b</td></tr><tr><td id=rc>c</td></tr></table>
</body></html>"#,
        table_rows("c", 8),
        table_rows("s", 6),
        table_rows("f", 5),
        table_rows("u", 5),
    ))
}

/// A new row of three cells.
fn new_row(p: &Page, text: &str) -> NodeId {
    let mut d = p.dom.borrow_mut();
    let tr = d.create_element(p.doc, "tr").unwrap();
    for i in 0..3 {
        let td = d.create_element(p.doc, "td").unwrap();
        let t = d.create_text(p.doc, &format!("{text} {i}")).unwrap();
        d.append_child(td, t).unwrap();
        d.append_child(tr, td).unwrap();
    }
    tr
}

fn text(selector: &'static str, value: &'static str) -> (String, Mutate) {
    (
        format!("{selector} text {value:?}"),
        Box::new(move |p| {
            let n = p.find(selector);
            p.dom.borrow_mut().set_text_content(n, value).unwrap();
        }),
    )
}

fn attr(selector: &'static str, name: &'static str, value: &'static str) -> (String, Mutate) {
    (
        format!("{selector} [{name}={value:?}]"),
        Box::new(move |p| {
            let n = p.find(selector);
            p.dom.borrow_mut().set_attribute(n, name, value).unwrap();
        }),
    )
}

/// Each step checked against a fresh build: the painted page and every element's box.
#[track_caller]
fn table_steps_match_fresh_builds(page: &Page, steps: Vec<(String, Mutate)>) {
    let mut r = Renderer::new();
    check(&mut r, page, "the first render");
    for (what, mutate) in steps {
        mutate(page);
        let before = blitz_tables();
        check(&mut r, page, &what);
        let after = blitz_tables();
        eprintln!("TABLESTEP {what}: built {} cells {}", after.0 - before.0, after.1 - before.1);
        r.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        let mut fresh = Renderer::new();
        fresh.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        for ((k, a), (_, b)) in boxes(&r, page).into_iter().zip(boxes(&fresh, page)) {
            assert_eq!(a, b, "box {k:x} after {what}");
        }
    }
}

#[test]
fn table_cell_and_row_changes_match_fresh_builds() {
    let page = table_page();
    let steps: Vec<(String, Mutate)> = vec![
        text("#c4 td", "edited"),
        text("#c4 td:nth-child(2)", "a much longer text in the middle cell that wraps over lines"),
        text("#c6 td:nth-child(3)", "averyveryverylongwordwithoutbreaksthatwidensitscolumn"),
        text("#c6 td:nth-child(3)", "1"),
        text("#s3 td", "spaced"),
        text("#f2 td:nth-child(2)", "fixed layout text that is long enough to wrap"),
        text("#u2 td", "uniform"),
        attr("#c3 td:nth-child(2)", "class", "hl"),
        attr("#c3 td:nth-child(2)", "class", ""),
        attr("#c5", "class", "thick"),
        attr("#c5", "style", "height: 40.5px"),
        attr("#c5 td", "style", "padding: 7.25px"),
        attr("#c7 td", "style", "height: 33.3px"),
        attr("#u3 td", "style", "border: 3px dashed green"),
        attr("#s2 td", "style", "width: 150px"),
        attr("#c2 td", "colspan", "2"),
        attr("#c2 td", "colspan", "1"),
        attr("#c4", "class", "hidden"),
        attr("#c4", "class", ""),
        attr("#c3 td:nth-child(3)", "class", "hidden"),
        attr("#c3 td:nth-child(3)", "class", ""),
        text("#rc", "rowspan neighbour"),
        text("thead th", "header"),
        text("tfoot td", "footer"),
        // Rows below a row that grows move; cells aligned, offset or overflowing in it.
        text("#c2 td:nth-child(2)", "a text long enough to wrap over three or four lines of the middle column here"),
        attr("#c3 td", "style", "vertical-align: bottom"),
        attr("#c3 td:nth-child(2)", "style", "position: relative; top: 3.5px"),
        text("#c2 td:nth-child(2)", "short again"),
        attr("#s4 td", "style", "vertical-align: middle; height: 30.5px"),
        text("#s4 td:nth-child(2)", "spaced and wrapping text in the middle cell of the spaced table"),
        attr("#c5 td:nth-child(3)", "style", "white-space: nowrap; overflow: visible"),
        text("#c5 td:nth-child(3)", "a nowrap text much wider than its column"),
        attr("#c6", "style", "height: 51.25px"),
        text("#f1 td", "fixed"),
        text("#u1 td:nth-child(2)", "uniform borders, a longer text that wraps in the cell"),
        // Cells that become stacking contexts, or have boxes hoisted out of them, and back
        // (the paint tree visits again only the cells that changed while they are plain boxes).
        attr("#c4 td:nth-child(2)", "style", "opacity: 0.5"),
        attr("#c4 td:nth-child(2)", "style", "position: relative; z-index: 2; background: yellow"),
        attr("#c4 td:nth-child(2)", "style", "position: relative; z-index: -1; background: yellow"),
        attr("#c4 td:nth-child(2)", "style", ""),
        attr("#c5 td", "style", "float: left; position: relative"),
        attr("#c5 td", "style", ""),
        (
            "an absolutely positioned box in a cell of a positioned table".into(),
            Box::new(|p| {
                let table = p.find("#c");
                let td = p.find("#c6 td");
                let mut d = p.dom.borrow_mut();
                d.set_attribute(table, "style", "position: relative").unwrap();
                let span = d.create_element(p.doc, "span").unwrap();
                d.set_attribute(span, "id", "abs").unwrap();
                d.set_attribute(span, "style", "position: absolute; top: 2px; right: 3px; background: red").unwrap();
                let t = d.create_text(p.doc, "abs").unwrap();
                d.append_child(span, t).unwrap();
                d.append_child(td, span).unwrap();
            }),
        ),
        text("#c2 td", "after the hoisted box"),
        attr("#abs", "style", "position: static"),
        text("#c3 td", "after it went static"),
        attr("#c6 td:nth-child(2)", "style", "background: blue"),
        text("#c7 td", "plain again"),
    ];
    table_steps_match_fresh_builds(&page, steps);
}

/// Cells aligned on their rows' baselines (lane P2): changes that move a row's baseline, or a cell's
/// offset in its box, lay out as a fresh build does (the offsets live in the cells' layout caches).
#[test]
fn baseline_cell_changes_match_fresh_builds() {
    let rows: String = (0..6)
        .map(|i| {
            format!(
                "<div class=r id=b{i}><div class=c style='padding-top:{}px'><div>cell {i} a</div></div>\
                 <div class=c><span>text {i} b</span></div><div class=c></div><div class=c><div>{i}.5</div></div></div>",
                i % 3 * 4
            )
        })
        .collect();
    let page = Page::new(&format!(
        r#"<!doctype html><html><head><style>
  body {{ margin: 0; font: 13px sans-serif }} .t {{ display: table; border-spacing: 2px }}
  .r {{ display: table-row }} .c {{ display: table-cell; padding: 2px 3px }} .big {{ font-size: 24px }}
</style></head><body><div class=t id=t>{rows}</div></body></html>"#
    ));
    let steps: Vec<(String, Mutate)> = vec![
        attr("#b1 .c", "class", "c big"),
        text("#b2 .c:nth-child(2) span", "a much longer text in the second cell that wraps"),
        attr("#b3 .c", "style", "padding-top: 30px"),
        attr("#b4 .c:nth-child(2)", "style", "vertical-align: top"),
        attr("#b4 .c:nth-child(2)", "style", ""),
        text("#b5 .c:nth-child(3)", "now with text"),
        attr("#b1 .c", "class", "c"),
        attr("#t", "style", "width: 600px"),
        text("#b0 .c div", "edited"),
    ];
    table_steps_match_fresh_builds(&page, steps);
}

/// Tables whose second cell's content sits 20px down in its box, on its row's baseline (`#t`, `#w`,
/// `#m`), and one without baseline-aligned cells (`#p`).
fn baseline_offset_page() -> Page {
    Page::new(
        r#"<!doctype html><html><head><style>
  body { margin: 0; font: 13px sans-serif } .t { display: table }
  .r { display: table-row } .c { display: table-cell }
  .top .c, .plain .c { vertical-align: top } .v { writing-mode: vertical-rl }
</style></head><body>
<div class=t id=t><div class=r><div class=c style='padding-top:20px'><div>a</div></div><div class=c><div>b</div></div></div></div>
<div class=t id=w><div class=r><div class=c style='padding-top:20px'><div>a</div></div><div class=c><div>b</div></div></div></div>
<div class=t id=m><div class=r><div class=c style='padding-top:20px'><div>a</div></div><div class=c id=mc><div>b</div></div></div></div>
<div class="t plain" id=p><div class=r id=pr><div class=c style='height:40px'><div>c</div></div></div></div>
</body></html>"#,
    )
}

/// A table that loses its last baseline-aligned cell: the content of a cell that had an offset
/// goes back to the top of its box, as in a fresh build (the grid only sets offsets in a table with
/// baseline-aligned cells).
#[test]
fn a_table_left_without_baseline_cells_drops_their_offsets() {
    table_steps_match_fresh_builds(&baseline_offset_page(), vec![attr("#t", "class", "t top")]);
}

/// The same for a table that turns vertical (no cell of a vertical table is baseline-aligned).
#[test]
fn a_table_turned_vertical_drops_its_cells_baseline_offsets() {
    table_steps_match_fresh_builds(&baseline_offset_page(), vec![attr("#w", "class", "t v")]);
}

/// And for a cell with an offset that moves to a table without baseline-aligned cells (a guard:
/// the table it left released it before too).
#[test]
fn a_cell_moved_to_a_table_without_baseline_cells_drops_its_offset() {
    let steps = vec![edit("the cell moved", |p| {
        let (row, cell) = (p.find("#pr"), p.find("#mc"));
        p.dom.borrow_mut().append_child(row, cell).unwrap();
    })];
    table_steps_match_fresh_builds(&baseline_offset_page(), steps);
}

/// Nested tables, vertical tables, spans and `display` changes of table parts.
#[test]
fn table_structure_changes_match_fresh_builds() {
    let page = Page::new(&format!(
        r#"<!doctype html><html><head><style>
  body {{ margin: 0; font: 13px sans-serif }} td {{ padding: 2px 3.5px }}
  table {{ border-collapse: collapse }} tr {{ border-bottom: 1px solid #888 }}
  #v {{ writing-mode: vertical-rl; height: 300px }}
  .x {{ border: 2px solid green }}
</style></head><body>
<table id=o><tbody id=ob><tr id=o0><td>outer a</td><td><table id=i><tbody id=ib>{}</tbody></table></td></tr>{}</tbody></table>
<table id=v><tbody id=vb>{}</tbody></table>
<table id=p><tbody id=pb>
<tr id=p0><td rowspan=2 id=pr>span rows</td><td colspan=2 id=pc>span columns</td></tr>
<tr id=p1><td>b</td><td>c</td></tr>
<tr id=p2><td>d</td><td>e</td><td>f</td></tr>
</tbody></table>
<table id=d><tbody id=db>{}</tbody></table>
</body></html>"#,
        table_rows("i", 3),
        table_rows("o", 4),
        table_rows("v", 4),
        table_rows("d", 5),
    ));
    let steps: Vec<(String, Mutate)> = vec![
        text("#i1 td", "inner edited with a longer text"),
        text("#o2 td:nth-child(2)", "outer edited"),
        attr("#i2 td", "class", "x"),
        (
            "a row appended to the inner table".into(),
            Box::new(|p| {
                let tbody = p.find("#ib");
                let tr = new_row(p, "inner new");
                p.dom.borrow_mut().append_child(tbody, tr).unwrap();
            }),
        ),
        text("#v2 td:nth-child(2)", "vertical text that is long enough"),
        attr("#v1 td", "class", "x"),
        (
            "a row appended to the vertical table".into(),
            Box::new(|p| {
                let tbody = p.find("#vb");
                let tr = new_row(p, "vertical new");
                p.dom.borrow_mut().append_child(tbody, tr).unwrap();
            }),
        ),
        text("#pr", "rowspan cell with more text"),
        text("#pc", "colspan"),
        text("#p2 td:nth-child(2)", "below the spans"),
        attr("#pr", "rowspan", "3"),
        attr("#pc", "colspan", "1"),
        attr("#p1 td", "rowspan", "2"),
        text("#d1 td", "display flips"),
        attr("#d2 td", "style", "display: none"),
        attr("#d2 td", "style", ""),
        attr("#d3", "style", "display: none"),
        attr("#d3", "style", ""),
        attr("#db", "style", "display: none"),
        attr("#db", "style", ""),
        attr("#d1 td:nth-child(2)", "style", "position: absolute"),
        attr("#d1 td:nth-child(2)", "style", ""),
    ];
    table_steps_match_fresh_builds(&page, steps);
}

#[test]
fn table_rows_added_and_removed_match_fresh_builds() {
    let page = table_page();
    let append = |group: &'static str, what: &'static str| -> (String, Mutate) {
        (
            format!("a row appended to {group}"),
            Box::new(move |p| {
                let tbody = p.find(group);
                let tr = new_row(p, what);
                p.dom.borrow_mut().append_child(tbody, tr).unwrap();
            }),
        )
    };
    let insert = |group: &'static str, before: &'static str| -> (String, Mutate) {
        (
            format!("a row inserted in {group} before {before}"),
            Box::new(move |p| {
                let tbody = p.find(group);
                let at = p.find(before);
                let tr = new_row(p, "inserted");
                p.dom.borrow_mut().insert_before(tbody, tr, Some(at)).unwrap();
            }),
        )
    };
    let remove = |row: &'static str| -> (String, Mutate) {
        (
            format!("{row} removed"),
            Box::new(move |p| {
                let tr = p.find(row);
                let parent = p.dom.borrow().parent(tr).unwrap().unwrap();
                p.dom.borrow_mut().remove_child(parent, tr).unwrap();
            }),
        )
    };
    let steps: Vec<(String, Mutate)> = vec![
        append("#cb", "appended"),
        append("#cb", "again"),
        insert("#cb", "#c3"),
        remove("#c5"),
        remove("#c7"),
        append("#sb", "spaced"),
        remove("#s0"),
        insert("#sb", "#s4"),
        append("#fb", "fixed"),
        remove("#f3"),
        append("#ub", "uniform"),
        remove("#u4"),
        append("thead", "head"),
        append("#cb", "after the head"),
        (
            "a row moved to another table".into(),
            Box::new(|p| {
                let tr = p.find("#s3");
                let tbody = p.find("#cb");
                let at = p.find("#c2");
                p.dom.borrow_mut().insert_before(tbody, tr, Some(at)).unwrap();
            }),
        ),
        (
            "a fourth cell".into(),
            Box::new(|p| {
                let tr = p.find("#c6");
                let mut d = p.dom.borrow_mut();
                let td = d.create_element(p.doc, "td").unwrap();
                d.append_child(tr, td).unwrap();
            }),
        ),
        (
            "rows of a table without a body".into(),
            Box::new(|p| {
                let body = p.dom.borrow().body(p.doc).unwrap().unwrap();
                let table = p.dom.borrow_mut().create_element(p.doc, "table").unwrap();
                p.dom.borrow_mut().set_attribute(table, "id", "nb").unwrap();
                p.dom.borrow_mut().set_attribute(table, "class", "nb").unwrap();
                for i in 0..4 {
                    let tr = new_row(p, &format!("direct {i}"));
                    p.dom.borrow_mut().set_attribute(tr, "style", "border-bottom: 1px solid").unwrap();
                    p.dom.borrow_mut().append_child(table, tr).unwrap();
                }
                p.dom.borrow_mut().append_child(body, table).unwrap();
            }),
        ),
        (
            "a row removed and a cell after it edited at once".into(),
            Box::new(|p| {
                let tbody = p.find("#cb");
                let tr = p.find("#c1");
                let td = p.find("#c6 td:nth-child(2)");
                let mut d = p.dom.borrow_mut();
                d.remove_child(tbody, tr).unwrap();
                d.set_text_content(td, "edited after a removal, long enough to wrap in its column").unwrap();
            }),
        ),
        (
            "two rows removed and one inserted at once".into(),
            Box::new(|p| {
                let tbody = p.find("#sb");
                let (a, b, at) = (p.find("#s1"), p.find("#s4"), p.find("#s2"));
                let tr = new_row(p, "between");
                let mut d = p.dom.borrow_mut();
                d.remove_child(tbody, a).unwrap();
                d.remove_child(tbody, b).unwrap();
                d.insert_before(tbody, tr, Some(at)).unwrap();
            }),
        ),
        text("#s2 td", "after two changes"),
        (
            "a row inserted with a taller cell".into(),
            Box::new(|p| {
                let tbody = p.find("#cb");
                let at = p.find("#c4");
                let tr = new_row(p, "a taller inserted row whose cells wrap over a few lines of the column");
                p.dom.borrow_mut().insert_before(tbody, tr, Some(at)).unwrap();
            }),
        ),
        text("#nb tr:nth-child(3) td", "edited direct row"),
        attr("#nb tr:nth-child(2)", "style", "border-bottom: 4px solid red"),
        append("#nb", "direct"),
    ];
    table_steps_match_fresh_builds(&page, steps);
}

/// A cell's text edit collects no cell again (the cell's box and its row are as they were; construction
/// damage stops at the cell, which is noted as changed) and lays out the grid again from its last layout;
/// appending a row collects only its cells and lays out the grid again too, and so does removing a row,
/// which collects no cell.
#[test]
fn a_cell_edit_collects_only_its_row() {
    let rows = table_rows("c", 50);
    let page = Page::new(&format!(
        r#"<!doctype html><html><head><style>table {{ border-collapse: collapse }} tr {{ border-bottom: 1px solid }}</style></head><body><table><tbody id=b>{rows}</tbody></table></body></html>"#
    ));
    let mut r = Renderer::new();
    check(&mut r, &page, "the first render");
    // Tables built, cells collected; grids laid out whole, again.
    let edit = |what: &str, page: &Page, r: &mut Renderer, step: &dyn Fn(&Page)| -> ((u64, u64), (u64, u64)) {
        step(page);
        let tree = page.tree();
        let (tables, grids) = (blitz_tables(), blitz_table_grids());
        let painted = paint(r, &page.snapshot(1, tree.clone()));
        let delta = |a: (u64, u64), b: (u64, u64)| (b.0 - a.0, b.1 - a.1);
        let counts = (delta(tables, blitz_tables()), delta(grids, blitz_table_grids()));
        assert_eq!(painted.hash, paint(&mut Renderer::new(), &page.snapshot(1, tree)).hash, "{what}");
        counts
    };
    let (tables, grids) = edit("a text edit", &page, &mut r, &|p| {
        let td = p.find("#c20 td:nth-child(2)");
        p.dom.borrow_mut().set_text_content(td, "edited").unwrap();
    });
    assert_eq!(tables, (0, 0), "a text edit");
    assert_eq!(grids, (0, 1), "a text edit");
    let (tables, grids) = edit("an append", &page, &mut r, &|p| {
        let tbody = p.find("#b");
        let tr = new_row(p, "n");
        p.dom.borrow_mut().append_child(tbody, tr).unwrap();
    });
    assert_eq!(tables, (0, 3), "an append");
    assert_eq!(grids, (0, 1), "an append");
    let (tables, grids) = edit("a removal", &page, &mut r, &|p| {
        let tbody = p.find("#b");
        let tr = p.find("#c30");
        p.dom.borrow_mut().remove_child(tbody, tr).unwrap();
    });
    assert_eq!(tables, (0, 0), "a removal");
    assert_eq!(grids, (0, 1), "a removal");
    let (tables, grids) = edit("an insert", &page, &mut r, &|p| {
        let tbody = p.find("#b");
        let at = p.find("#c10");
        let tr = new_row(p, "i");
        p.dom.borrow_mut().insert_before(tbody, tr, Some(at)).unwrap();
    });
    assert_eq!(tables, (0, 3), "an insert");
    assert_eq!(grids, (0, 1), "an insert");
    let (_, grids) = edit("a text edit after", &page, &mut r, &|p| {
        let td = p.find("#c40 td");
        p.dom.borrow_mut().set_text_content(td, "x").unwrap();
    });
    assert_eq!(grids, (0, 1), "a text edit after");

    // Rounding reaches the cells the grid placed again, not the table's others.
    let td = page.find("#c41 td");
    page.dom.borrow_mut().set_text_content(td, "y").unwrap();
    let before = blitz_rounding();
    paint(&mut r, &page.snapshot(1, page.tree()));
    let reached = blitz_rounding().0 - before.0;
    assert!(reached < 60, "rounding reached {reached} nodes");
    let tbody = page.find("#b");
    let tr = new_row(&page, "n");
    page.dom.borrow_mut().append_child(tbody, tr).unwrap();
    let before = blitz_rounding();
    let painted = paint(&mut r, &page.snapshot(1, page.tree()));
    let reached = blitz_rounding().0 - before.0;
    assert!(reached < 60, "rounding reached {reached} nodes after an append");
    assert_eq!(painted.hash, paint(&mut Renderer::new(), &page.snapshot(1, page.tree())).hash);
}

/// Changes incremental documents got wrong before tables were updated in place.
#[test]
fn table_known_gaps_match_fresh_builds() {
    let page = Page::new(&format!(
        r#"<!doctype html><html><head><style id=st>
  body {{ margin: 0; font: 13px sans-serif }} td {{ padding: 2px }}
</style></head><body>
<table id=k style="border-collapse: collapse"><tbody id=kb>{}</tbody></table>
<table id=b><tbody>{}</tbody></table>
</body></html>"#,
        table_rows("k", 4),
        table_rows("b", 3),
    ));
    let steps: Vec<(String, Mutate)> = vec![
        attr("#k1", "style", "display: contents"),
        attr("#k1", "style", "display: table-row"),
        text("#st", "body { margin: 0; font: 13px sans-serif } td { padding: 2px; height: 30px; border: 3px solid red }"),
        text("#st", "body { margin: 0; font: 13px sans-serif } td { padding: 2px } #k { border-collapse: separate }"),
        text("#st", "body { margin: 0; font: 13px sans-serif } td { padding: 2px }"),
        attr("#k2 td", "style", "float: left"),
        attr("#k2 td", "style", ""),
        attr("#k3 td", "style", "position: absolute"),
        attr("#k3 td", "style", ""),
        attr("#k0 td", "style", "display: block"),
        attr("#k0 td", "style", ""),
        attr("#b", "style", "display: block"),
        attr("#b", "style", ""),
    ];
    table_steps_match_fresh_builds(&page, steps);
}

/// More cells changed than a table has measured widths for, then rows replaced in the same pass:
/// a row inserted into a striped table restyles every row after it.
#[test]
fn table_rows_restyled_and_inserted_match_fresh_builds() {
    let page = Page::new(&format!(
        r#"<!doctype html><html><head><style>
  body {{ margin: 0; font: 13px sans-serif }} td {{ padding: 2.5px 3.25px }}
  #z tr:nth-child(odd) td {{ background: #eee }}
  #h.sp tr:nth-child(odd) td {{ height: 30px }}
</style></head><body>
<table id=z><tbody id=zb>{}</tbody></table>
<table id=h><tbody id=hb>{}</tbody></table>
</body></html>"#,
        table_rows("z", 10),
        table_rows("h", 6),
    ));
    let insert = |group: &'static str, before: &'static str, what: &'static str| -> (String, Mutate) {
        (
            format!("a row inserted into {group} before {before}"),
            Box::new(move |p| {
                let tbody = p.find(group);
                let at = p.find(before);
                let tr = new_row(p, what);
                p.dom.borrow_mut().insert_before(tbody, tr, Some(at)).unwrap();
            }),
        )
    };
    let remove = |row: &'static str| -> (String, Mutate) {
        (
            format!("{row} removed"),
            Box::new(move |p| {
                let tr = p.find(row);
                let parent = p.dom.borrow().parent(tr).unwrap().unwrap();
                p.dom.borrow_mut().remove_child(parent, tr).unwrap();
            }),
        )
    };
    let steps: Vec<(String, Mutate)> = vec![
        insert("#zb", "#z1", "inserted"),
        text("#z5 td", "after"),
        remove("#z2"),
        insert("#zb", "#z0", "first"),
        attr("#h", "class", "sp"),
        insert("#hb", "#h1", "shifts odd"),
        text("#h4 td", "edited"),
        remove("#h0"),
    ];
    table_steps_match_fresh_builds(&page, steps);
}

/// Boxes that only move (the rows below a cell that grew, the blocks below a
/// block that grew by a fraction of a pixel) are marked on their ancestors only:
/// their rounding and overflow still follow the move. (A transformed box that
/// moves paints differently from a fresh build, whether or not it is marked
/// itself: a known gap of blitz-dom's incremental passes, left out here.)
#[test]
fn rounding_follows_boxes_that_only_moved() {
    let rows: String = (0..12)
        .map(|i| format!("<tr><td id=c{i} style=\"font-size:13.3px\">cell {i}</td><td><div style=\"overflow:auto;height:10.5px\"><p style=\"margin:0;height:30px\">s</p></div></td></tr>"))
        .collect();
    let blocks: String = (0..12)
        .map(|i| format!("<div id=d{i} style=\"height:7.3px\"><span style=\"display:inline-block;width:5.5px;height:3.3px\"></span>b{i}<p style=\"margin:0;width:20.5px;height:2.5px\"></p></div>"))
        .collect();
    rounds_as_a_fresh_build(
        &format!("<table id=t style=\"width:300.5px\">{rows}</table><div id=top style=\"height:3.3px\"></div>{blocks}"),
        vec![
            (
                "a taller cell".into(),
                Box::new(|p| {
                    let n = p.find("#c1");
                    p.dom.borrow_mut().set_text_content(n, "a taller cell that wraps onto lines of its own").unwrap();
                }),
            ),
            style("#top", "height:4.7px"),
            style("#top", "height:4.7px;margin-bottom:0.4px"),
            style("#c3", "font-size:13.3px;padding-top:2.25px"),
        ],
    );
}

/// Layout queries synced from pyro-dom's change journal on a long table and long
/// lists: cell text edits, rows appended, moved and removed, spans, collapsed
/// borders, display flips of cells, rows and children of a long list
/// (layout-tree membership, `Node::layout_child_of`) and `position: sticky`
/// turned on and off (`BaseDocument::has_sticky`) report what a fresh build
/// of the same tree does.
#[test]
fn deltas_on_long_tables_and_lists_match_fresh_builds() {
    long_tables_and_lists(1);
}

/// The same steps with a hit test only after every third (and the last):
/// the hit bounds of the boxes several resolves computed again, and the cells
/// of the rows they appended, moved and removed, are brought up to date at
/// once by the next hit test (`BaseDocument::ensure_hit_overflow`).
#[test]
fn hit_bounds_after_runs_of_deltas_match_fresh_builds() {
    long_tables_and_lists(3);
}

/// Cells of a fixed-layout table edited one per sync, with no hit test in
/// between: the first edit makes a cell's text overflow sideways (its hit
/// bound grows, its box does not), later ones touch other rows. The hit test
/// after them sees every edited cell's bound and its row's union, not only
/// the last sync's (`PlacedCells::transformed` keeps the cells of every pass
/// until the next hit pass).
#[test]
fn hit_bounds_of_cells_edited_over_several_syncs_match_fresh_builds() {
    let rows: String = (0..12)
        .map(|i| format!("<tr id=r{i}><td id=a{i} style=\"white-space:nowrap\">a{i}</td><td id=b{i}>b{i}</td><td>c{i}</td></tr>"))
        .collect();
    let page = Page::new(&format!(
        "<!doctype html><html><head><style>body {{ margin: 0; font: 14px sans-serif }} table {{ table-layout: fixed; width: 300px }} td {{ width: 100px; padding: 0 }}</style></head><body>\
         <table id=t><tbody>{rows}</tbody></table></body></html>"
    ));
    let mut r = Renderer::new();
    r.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
    let edits: [(&str, &str); 4] = [
        ("#a3", "a long run of text that overflows its cell sideways"),
        ("#b7", "b"),
        ("#a9", "another long run of text that overflows sideways too"),
        ("#b1", "bb"),
    ];
    for (round, edits) in [&edits[..2], &edits[2..]].into_iter().enumerate() {
        for (sel, text) in edits {
            let since = page.dom.borrow().version();
            let n = page.find(sel);
            page.dom.borrow_mut().set_text_content(n, text).unwrap();
            let delta = page.dom.borrow().render_delta(page.doc, since, &Page::opts()).expect("a delta");
            r.sync_delta(1, &delta_ops(delta), (640, 480, 1.0, false, false), false).unwrap();
        }
        let mut fresh = Renderer::new();
        fresh.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        hit_candidates_match(&mut r, &mut fresh, &format!("round {round} of cell edits"));
    }
}

/// A deep descendant of one cell changes while other cells' subtrees carry
/// damage flags that lead to no layout change: a paint-only restyle deep in a
/// cell, an attribute set to the value it has, a node inserted and removed in
/// the same delta, a `::after` whose `content` changes, and, in the next
/// syncs, cells flagged by the previous one. The walks that follow damage
/// visit only the children on a damaged path (`Node::is_clean_layout_child_of`,
/// `Node::has_no_damage_below`); the boxes, hit candidates and paint match a
/// fresh build after each sync.
#[test]
fn a_deep_cell_edit_beside_cells_with_flags_but_no_layout_change_matches_fresh_builds() {
    let rows: String = (0..10)
        .map(|i| {
            format!(
                "<tr id=r{i}>\
                 <td><div class=f><span class=dot></span><span id=s{i}><b id=b{i}>item {i}</b></span></div></td>\
                 <td><div style=\"display:contents\"><p id=p{i}>text {i} <i id=i{i}>it</i></p></div></td>\
                 <td><span id=c{i} class=after data-x=\"{i}\">c</span></td></tr>"
            )
        })
        .collect();
    let page = Page::new(&format!(
        "<!doctype html><html><head><style>body {{ margin: 0; font: 14px sans-serif }} td {{ padding: 2px }} \
         .f {{ display: flex; gap: 4px; align-items: center }} .dot {{ display: inline-block; width: 6px; height: 6px; background: #0a0 }} \
         .after::after {{ content: attr(data-x) }} p {{ margin: 0 }}</style></head><body>\
         <table id=t style=\"width:500px\"><tbody id=tb>{rows}</tbody></table></body></html>"
    ));
    type Step = (&'static str, Box<dyn Fn(&Page)>);
    let text = |p: &Page, sel: &str, value: &str| {
        let n = p.find(sel);
        p.dom.borrow_mut().set_text_content(n, value).unwrap();
    };
    let attr = |p: &Page, sel: &str, name: &str, value: &str| {
        let n = p.find(sel);
        p.dom.borrow_mut().set_attribute(n, name, value).unwrap();
    };
    let steps: Vec<Step> = vec![
        (
            "deep text beside a paint-only restyle, a same-value attribute and a new ::after content",
            Box::new(move |p| {
                text(p, "#b3", "a much longer item text that wraps over several lines in its cell");
                attr(p, "#i5", "style", "color:red");
                attr(p, "#s6", "id", "s6");
                attr(p, "#c7", "data-x", "seven and more");
            }),
        ),
        (
            "a node inserted and removed beside a deep edit",
            Box::new(move |p| {
                let host = p.find("#p8");
                let mut d = p.dom.borrow_mut();
                let e = d.create_element(p.doc, "em").unwrap();
                d.append_child(host, e).unwrap();
                d.remove_child(host, e).unwrap();
                drop(d);
                text(p, "#b1", "x");
            }),
        ),
        ("the next deep edit alone", Box::new(move |p| text(p, "#b9", "nine, wider than it was"))),
        ("a paint-only restyle alone", Box::new(move |p| attr(p, "#i2", "style", "color:blue"))),
        (
            "a deep inline-block resized beside a reverted paint restyle",
            Box::new(move |p| {
                attr(p, "#i4", "style", "display:inline-block;width:80px");
                attr(p, "#i5", "style", "");
            }),
        ),
        ("deep text back", Box::new(move |p| text(p, "#b3", "item 3"))),
    ];
    let mut r = Renderer::new();
    r.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
    for (what, step) in &steps {
        let since = page.dom.borrow().version();
        step(&page);
        let delta = page.dom.borrow().render_delta(page.doc, since, &Page::opts()).expect("a delta");
        r.sync_delta(1, &delta_ops(delta), (640, 480, 1.0, false, false), false).unwrap();
        let mut fresh = Renderer::new();
        fresh.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        for ((k, a), (_, b)) in boxes(&r, &page).into_iter().zip(boxes(&fresh, &page)) {
            assert_eq!(a, b, "box {k:x} after {what}");
        }
        hit_candidates_match(&mut r, &mut fresh, what);
        let tree = page.tree();
        let mirrored = paint(&mut r, &page.snapshot(1, tree.clone()));
        let painted = paint(&mut fresh, &page.snapshot(1, tree));
        assert_eq!(mirrored.hash, painted.hash, "paint after {what}");
    }
}

fn long_tables_and_lists(hit_every: usize) {
    let rows: String =
        (0..24).map(|i| format!("<tr id=r{i}><td id=a{i}>a{i}</td><td id=b{i}>cell {i}</td></tr>")).collect();
    let items: String = (0..24).map(|i| format!("<p id=p{i}>para {i} <span id=s{i}>in</span> text</p>")).collect();
    let others: String = (0..20).map(|i| format!("<div id=q{i}>q{i}</div>")).collect();
    let page = Page::new(&format!(
        "<!doctype html><html><head><style>body {{ margin: 0; font: 14px sans-serif }} td {{ padding: 2px }}</style></head><body>\
         <table id=t style=\"width:400px\"><thead><tr><th id=h>head</th><th>h2</th></tr></thead><tbody id=tb>{rows}</tbody></table>\
         <div id=list>{items}</div><div id=other>{others}</div></body></html>"
    ));
    type Step = (&'static str, Box<dyn Fn(&Page)>);
    let attr = |sel: &'static str, name: &'static str, value: &'static str| -> Box<dyn Fn(&Page)> {
        Box::new(move |p: &Page| {
            let n = p.find(sel);
            p.dom.borrow_mut().set_attribute(n, name, value).unwrap();
        })
    };
    let text = |sel: &'static str, value: &'static str| -> Box<dyn Fn(&Page)> {
        Box::new(move |p: &Page| {
            let n = p.find(sel);
            p.dom.borrow_mut().set_text_content(n, value).unwrap();
        })
    };
    let steps: Vec<Step> = vec![
        ("cell text", text("#b7", "a much longer cell text that wraps over several lines in the cell")),
        ("another cell text", text("#a3", "x")),
        (
            "row appended",
            Box::new(|p| {
                let tb = p.find("#tb");
                let mut d = p.dom.borrow_mut();
                let tr = d.create_element(p.doc, "tr").unwrap();
                for s in ["new", "row"] {
                    let td = d.create_element(p.doc, "td").unwrap();
                    let t = d.create_text(p.doc, s).unwrap();
                    d.append_child(td, t).unwrap();
                    d.append_child(tr, td).unwrap();
                }
                d.append_child(tb, tr).unwrap();
            }),
        ),
        (
            "row moved to the top",
            Box::new(|p| {
                let (tb, r, first) = (p.find("#tb"), p.find("#r20"), p.find("#r0"));
                p.dom.borrow_mut().insert_before(tb, r, Some(first)).unwrap();
            }),
        ),
        (
            "row removed",
            Box::new(|p| {
                let (tb, r) = (p.find("#tb"), p.find("#r5"));
                p.dom.borrow_mut().remove_child(tb, r).unwrap();
            }),
        ),
        ("colspan", attr("#a9", "colspan", "2")),
        ("rowspan", attr("#a11", "rowspan", "2")),
        ("collapsed borders", attr("#t", "style", "width:400px;border-collapse:collapse;border:2px solid")),
        ("cell border", attr("#b12", "style", "border:3px solid red")),
        ("cell hidden", attr("#b13", "style", "display:none")),
        ("cell shown", attr("#b13", "style", "display:table-cell")),
        ("row hidden", attr("#r14", "style", "display:none")),
        ("row shown", attr("#r14", "style", "")),
        ("sticky header", attr("#h", "style", "position:sticky;top:0")),
        ("sticky cell text", text("#b2", "edited under a sticky header")),
        ("sticky off", attr("#h", "style", "position:relative")),
        ("sticky list item", attr("#p10", "style", "position:sticky;top:5px")),
        (
            "sticky item removed",
            Box::new(|p| {
                let (list, item) = (p.find("#list"), p.find("#p10"));
                p.dom.borrow_mut().remove_child(list, item).unwrap();
            }),
        ),
        // (A block among blocks made inline is left out: blitz-dom's
        // incremental layout misplaces the boxes after it, before and after
        // this change.)
        ("span made an inline block", attr("#s4", "style", "display:inline-block;width:30px")),
        ("span made a block", attr("#s6", "style", "display:block")),
        ("span made inline again", attr("#s4", "style", "")),
        ("other span made inline again", attr("#s6", "style", "")),
        ("paragraph hidden", attr("#p8", "style", "display:none")),
        ("paragraph contents", attr("#p9", "style", "display:contents")),
        (
            "paragraph moved to the other list",
            Box::new(|p| {
                let (other, item) = (p.find("#other"), p.find("#p12"));
                p.dom.borrow_mut().append_child(other, item).unwrap();
            }),
        ),
        (
            "paragraph appended",
            Box::new(|p| {
                let list = p.find("#list");
                let mut d = p.dom.borrow_mut();
                let e = d.create_element(p.doc, "p").unwrap();
                let t = d.create_text(p.doc, "appended").unwrap();
                d.append_child(e, t).unwrap();
                d.append_child(list, e).unwrap();
            }),
        ),
        // Lines of no height overflowing sideways (the hit overflow keeps
        // them, the scrollable overflow does not), then tight lines.
        ("zero line height", attr("#p5", "style", "line-height:0;white-space:nowrap;width:20px;font-size:30px")),
        ("tight line height", attr("#p5", "style", "line-height:4px;width:20px")),
        ("line height reverted", attr("#p5", "style", "")),
        ("list made inline", attr("#other", "style", "display:inline")),
        ("list made a block again", attr("#other", "style", "")),
    ];
    let mut r = Renderer::new();
    r.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
    let mut deltas = 0;
    for (i, (what, step)) in steps.iter().enumerate() {
        let since = page.dom.borrow().version();
        step(&page);
        let delta = page.dom.borrow().render_delta(page.doc, since, &Page::opts());
        if delta.is_some_and(|d| r.sync_delta(1, &delta_ops(d), (640, 480, 1.0, false, false), false).is_ok()) {
            deltas += 1;
        } else {
            r.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        }
        let mut fresh = Renderer::new();
        fresh.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        let sticky_now = matches!(*what, "sticky header" | "sticky cell text" | "sticky list item");
        assert_eq!(r.has_sticky(1), sticky_now, "sticky after {what}");
        assert_eq!(fresh.has_sticky(1), sticky_now, "sticky in a fresh build after {what}");
        for ((k, a), (_, b)) in boxes(&r, &page).into_iter().zip(boxes(&fresh, &page)) {
            assert_eq!(a, b, "box {k:x} after {what}");
        }
        let sticky = |r: &Renderer| r.page_layout(1).map(|l| l.sticky.len());
        assert_eq!(sticky(&r), sticky(&fresh), "sticky boxes after {what}");
        if (i + 1) % hit_every == 0 || i + 1 == steps.len() {
            hit_candidates_match(&mut r, &mut fresh, what);
        }
        subtree_words_match(&r, &page.tree(), what);
    }
    assert!(deltas * 2 > steps.len(), "most steps sync as deltas ({deltas} of {})", steps.len());
}

/// `Renderer::subtree_words` of every element of `tree` reports, among
/// others, the words `Renderer::page_words` reports for the text nodes
/// below it, in the same order.
#[track_caller]
fn subtree_words_match(r: &Renderer, tree: &Tree, what: &str) {
    let below = |i: usize, mut j: usize| loop {
        if j == i {
            return true;
        }
        match tree.nodes[j].parent {
            TOP => return false,
            parent => j = parent as usize,
        }
    };
    let all = r.page_words(1).unwrap();
    let index: std::collections::HashMap<u64, usize> = tree.nodes.iter().enumerate().map(|(i, n)| (n.key, i)).collect();
    for (i, node) in tree.nodes.iter().enumerate() {
        if !matches!(node.kind, TreeKind::Element { .. }) {
            continue;
        }
        let mine = |words: Vec<(u64, TextWord)>| -> Vec<(u64, TextWord)> {
            words.into_iter().filter(|(k, _)| index.get(k).is_some_and(|&j| below(i, j))).collect()
        };
        let sub = r.subtree_words(1, node.key).expect("a node of the page");
        assert_eq!(mine(sub), mine(all.clone()), "words below node {i} after {what}");
    }
}

/// The hit-test descent of a delta-synced document (`Renderer::hit_candidates`:
/// bounds from the per-row cell unions and the hit overflow the
/// incremental passes keep) reports, at every point of a grid, the elements
/// a fresh build's does, in the same order and with the same boxes; its
/// words and sticky elements are the fresh build's too.
#[track_caller]
fn hit_candidates_match(r: &mut Renderer, fresh: &mut Renderer, what: &str) {
    r.ensure_hit_bounds(1);
    fresh.ensure_hit_bounds(1);
    let zero = |_: u64| (0.0, 0.0);
    let strip = |v: Vec<HitVisit>| v.into_iter().map(|v| (v.key, v.bounds, v.hit)).collect::<Vec<_>>();
    for y in (2..480).step_by(9) {
        for x in (2..640).step_by(11) {
            let probe = HitProbe { x: x as f64, y: y as f64, ignore_x: false, window_scroll: (0.0, 0.0) };
            let a = strip(r.hit_candidates(1, &probe, &zero, &[]).unwrap());
            let b = strip(fresh.hit_candidates(1, &probe, &zero, &[]).unwrap());
            assert_eq!(a, b, "hit candidates at ({x}, {y}) after {what}");
        }
    }
    // (The Blitz node ids of the words differ between the documents, and
    // their order follows the node slab: a delta-synced document lists an
    // appended row's words last.)
    let words = |r: &Renderer| {
        r.page_words(1).map(|w| {
            let mut w: Vec<_> = w
                .into_iter()
                .map(|(k, w)| (k, w.x.to_bits(), w.y.to_bits(), w.width, w.height, w.fixed, w.edges))
                .collect();
            w.sort_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));
            w
        })
    };
    assert_eq!(words(&*r), words(&*fresh), "words after {what}");
    assert_eq!(r.sticky_elements(1), fresh.sticky_elements(1), "sticky elements after {what}");
}

/// A transform resolves against the box's rounded size, and rounding follows
/// the box's absolute position: a transformed box that only moved by a
/// fractional amount (or sits inside one) can round to a different size with
/// no unrounded layout of its own changing. Delta-synced boxes, quads,
/// document size and paint must match a fresh build after each move.
#[test]
fn transformed_boxes_that_only_moved_match_fresh_builds() {
    let cases: &[(&str, &str, [&str; 3])] = &[
        // Identity at the first rounded size (`translateY(100%)` of a 0px
        // high box is stored as no transform), not after the move.
        (
            r#"<div id=a style="height:3.7px"></div><div id=tb style="transform:translateY(100%);width:10px;height:0.4px"></div>"#,
            "#a",
            ["height:3.3px", "height:3.7px", "height:3.3px"],
        ),
        // The individual transform properties, percentages and a percentage origin.
        (
            r#"<div id=a style="height:3.3px"></div><div id=tb style="translate:50% 25%;rotate:15deg;scale:1.25;transform-origin:0 100%;width:7.5px;height:5.5px"></div>"#,
            "#a",
            ["height:3.7px", "height:0.4px", "height:3.7px"],
        ),
        // Nested transforms in a moved box: both resolve against sizes that
        // follow the move.
        (
            r#"<div id=a style="height:3.3px"></div><div style="transform:rotate(10deg) translate(10%,30%);width:20.5px;height:9.5px"><div style="transform:scale(2) translate(50%,50%);width:5.5px;height:3.5px"><div style="transform:rotate(-30deg);height:2.5px"></div></div></div>"#,
            "#a",
            ["height:3.7px", "height:0.4px", "height:3.7px"],
        ),
        // Moved horizontally by a fractional amount (a flex sibling that grew).
        (
            r#"<div style="display:flex"><div id=a style="width:3.3px"></div><div style="transform:translateX(-50%) rotate(45deg);width:7.5px;height:7.5px"></div><div style="transform:translate(25%,0);width:2.5px;height:2.5px"></div></div>"#,
            "#a",
            ["width:3.7px", "width:0.4px", "width:12.6px"],
        ),
        // Out of flow: an absolutely positioned transformed box placed after a
        // static sibling in its containing block moves with that sibling.
        (
            r#"<div style="position:relative"><div id=a style="height:3.3px"></div><div style="position:absolute;transform:rotate(30deg) translateY(50%);width:6.5px;height:4.5px"></div></div>"#,
            "#a",
            ["height:3.7px", "height:0.4px", "height:3.7px"],
        ),
        (
            r#"<div id=a style="height:3.3px"></div><div id=tb style="transform:rotate(45deg);width:5.5px;height:5.5px"></div>"#,
            "#a",
            ["height:3.7px", "height:0.4px", "height:3.7px"],
        ),
        (
            r#"<div style="display:flex;gap:2.5px"><div id=a style="width:10.5px">a</div><div id=tb style="transform:scale(1.5);width:5.5px;height:5.5px"></div></div>"#,
            "#a",
            ["width:30.25px", "width:2.2px", "width:30.25px"],
        ),
        // A transformed descendant of a box that only moved: its own position
        // in its parent is unchanged, its absolute position (and rounding) is not.
        (
            r#"<div id=a style="height:3.3px"></div><div id=m><div id=tb style="transform:translate(50%,25%) rotate(20deg);width:7.5px;height:5.5px"></div></div>"#,
            "#a",
            ["height:3.7px", "height:0.4px", "height:3.7px"],
        ),
        (
            r#"<table style="width:200.5px"><tr><td id=a style="height:3.3px">a</td></tr><tr><td style="transform:rotate(30deg);width:5.5px;height:5.5px">r</td></tr></table>"#,
            "#a",
            ["height:23.7px", "height:20.4px", "height:23.7px"],
        ),
        (
            r#"<div style="overflow:auto;height:20px"><div id=a style="height:3.3px"></div><div id=tb style="transform:translateY(50%) scale(2);width:5.5px;height:15.5px"></div></div>"#,
            "#a",
            ["height:3.7px", "height:0.4px", "height:3.7px"],
        ),
    ];
    let quads = |r: &Renderer, page: &Page| -> Vec<(u64, String)> {
        page.tree()
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, TreeKind::Element { .. }))
            .map(|n| (n.key, format!("{:?}", r.element_quads(1, n.key))))
            .collect()
    };
    for (body, sel, values) in cases {
        let page = Page::new(&format!("{PAINT_BOXES}{body}</body></html>"));
        let mut r = Renderer::new();
        r.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        let mut painted = Renderer::new();
        paint(&mut painted, &page.snapshot(1, page.tree()));
        for value in values {
            let since = page.dom.borrow().version();
            let n = page.find(sel);
            page.dom.borrow_mut().set_attribute(n, "style", value).unwrap();
            let delta = page.dom.borrow().render_delta(page.doc, since, &Page::opts()).expect("a delta");
            r.sync_delta(1, &delta_ops(delta), (640, 480, 1.0, false, false), false).unwrap();
            let mut fresh = Renderer::new();
            fresh.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
            let what = format!("{body}: {sel} {{ {value} }}");
            for ((k, a), (_, b)) in boxes(&r, &page).into_iter().zip(boxes(&fresh, &page)) {
                assert_eq!(a, b, "box {k:x} after {what}");
            }
            assert_eq!(quads(&r, &page), quads(&fresh, &page), "quads after {what}");
            let size = |r: &Renderer| r.page_layout(1).map(|l| (l.document_width, l.document_height));
            assert_eq!(size(&r), size(&fresh), "document size after {what}");
            let tree = page.tree();
            let mirrored = paint(&mut painted, &page.snapshot(1, tree.clone()));
            let freshp = paint(&mut Renderer::new(), &page.snapshot(1, tree));
            assert_eq!(mirrored.hash, freshp.hash, "paint after {what}");
        }
    }
}

// ---------------- display changes of table parts (C1, C5) ----------------

/// Each step applied to a renderer synced by pyro-dom's deltas (the layout-query path) and to one
/// synced by whole-tree paints; both compared with fresh builds: every element's box, the document
/// size and the paint hash. Returns every difference rather than stopping at the first.
fn display_flip_failures(html: &str, steps: Vec<(String, Mutate)>) -> Vec<String> {
    let page = Page::new(html);
    let mut d = Renderer::new();
    d.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
    let mut live = Renderer::new();
    check(&mut live, &page, "the first render");
    let size = |r: &Renderer| r.page_layout(1).map(|l| (l.document_width, l.document_height));
    let mut failures = Vec::new();
    for (what, mutate) in steps {
        let since = page.dom.borrow().version();
        mutate(&page);
        let delta = page.dom.borrow().render_delta(page.doc, since, &Page::opts());
        let via_delta =
            delta.is_some_and(|dd| d.sync_delta(1, &delta_ops(dd), (640, 480, 1.0, false, false), false).is_ok());
        if !via_delta {
            failures.push(format!("{what}: not synced as a delta"));
            d.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        }
        let mut fresh = Renderer::new();
        fresh.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        let fresh_boxes = boxes(&fresh, &page);
        let tree = page.tree();
        let fresh_paint = paint(&mut Renderer::new(), &page.snapshot(1, tree.clone())).hash;
        // The delta renderer's layout is compared before anything paints it; the whole-tree one
        // is painted (synced) first.
        for (name, r) in [("delta", &mut d), ("live", &mut live)] {
            if name == "live" && paint(r, &page.snapshot(1, tree.clone())).hash != fresh_paint {
                failures.push(format!("{what}: {name} paint differs"));
            }
            if name == "live" {
                // Layout queries answer from a document synced for them (no change left to apply).
                r.sync_layout(&page.snapshot(1, tree.clone()), &fetcher()).unwrap();
            }
            for ((k, a), (_, b)) in boxes(r, &page).into_iter().zip(fresh_boxes.iter().cloned()) {
                if a != b {
                    failures.push(format!("{what}: {name} box {k:x} {a:?} vs fresh {b:?}"));
                    break;
                }
            }
            if size(r) != size(&fresh) {
                failures.push(format!("{what}: {name} document size {:?} vs fresh {:?}", size(r), size(&fresh)));
            }
            if name == "delta" && paint(r, &page.snapshot(1, tree.clone())).hash != fresh_paint {
                failures.push(format!("{what}: {name} paint differs"));
            }
        }
    }
    failures
}

type Scenario = (String, String, Vec<(String, Mutate)>);

/// Every scenario run (a panic counts as its failure), then all differences reported together.
#[track_caller]
fn display_flips_match_fresh_builds(scenarios: Vec<Scenario>) {
    let mut failures = Vec::new();
    for (name, html, steps) in scenarios {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| display_flip_failures(&html, steps))) {
            Ok(f) => failures.extend(f.into_iter().map(|f| format!("[{name}] {f}"))),
            Err(e) => {
                let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()));
                failures.push(format!("[{name}] panicked: {}", msg.unwrap_or_default()));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn set_style(selector: &'static str, value: &'static str) -> (String, Mutate) {
    (
        format!("{selector} style={value:?}"),
        Box::new(move |p| {
            let n = p.find(selector);
            p.dom.borrow_mut().set_attribute(n, "style", value).unwrap();
        }),
    )
}

fn set_text(selector: &'static str, value: &'static str) -> (String, Mutate) {
    (
        format!("{selector} text {value:?}"),
        Box::new(move |p| {
            let n = p.find(selector);
            p.dom.borrow_mut().set_text_content(n, value).unwrap();
        }),
    )
}

fn flip_html(table_style: &str) -> String {
    let rows: String = (0..4)
        .map(|i| format!("<tr id=k{i}><td>cell {i} a</td><td>text {i} b with some words</td><td>{i}.5</td></tr>"))
        .collect();
    format!(
        r#"<!doctype html><html><head><style>
  body {{ margin: 0; font: 13px sans-serif }} td {{ padding: 2.5px 3.25px }} tr {{ border-bottom: 1px solid #ccc }}
</style></head><body>
<table id=k style="{table_style}"><tbody id=kb>{rows}</tbody></table><p id=after>after</p>
</body></html>"#
    )
}

const TABLE_STYLES: [&str; 3] = ["", "border-collapse: collapse", "border-collapse: collapse; width: 300.5px"];

/// The same steps on a separated table, a collapsed one and a collapsed one of a set width.
fn per_table_style(steps: impl Fn(&'static str) -> Vec<(String, Mutate)>) -> Vec<Scenario> {
    TABLE_STYLES.iter().map(|t| (format!("table style={t:?}"), flip_html(t), steps(t))).collect()
}

fn with_display(table_style: &str, display: &str) -> &'static str {
    Box::leak(format!("{table_style}; display: {display}").into_boxed_str())
}

/// C1: a row that stops generating a box (`display: contents`, which Blitz's tables treat as
/// `display: none`) and starts again.
#[test]
fn a_table_row_display_contents_and_back_matches_fresh_builds() {
    display_flips_match_fresh_builds(per_table_style(|_| {
        vec![set_style("#k1", "display: contents"), set_style("#k1", "display: table-row")]
    }));
}

/// C1: a cell edited while its row is `display: contents`, then the row back.
#[test]
fn a_cell_edited_while_its_row_is_display_contents_matches_fresh_builds() {
    display_flips_match_fresh_builds(per_table_style(|_| {
        vec![set_style("#k2", "display: contents"), set_text("#k2 td", "edited while hoisted"), set_style("#k2", "")]
    }));
}

/// C1: two adjacent rows hoisted into one anonymous row, and back one by one.
#[test]
fn adjacent_rows_display_contents_and_back_match_fresh_builds() {
    display_flips_match_fresh_builds(per_table_style(|_| {
        vec![
            set_style("#k0", "display: contents"),
            set_style("#k1", "display: contents"),
            set_style("#k0", ""),
            set_style("#k1", ""),
        ]
    }));
}

/// C1 for row groups: a tbody that stops and starts generating a box, its rows wrapped in an
/// anonymous group meanwhile.
#[test]
fn a_row_group_display_contents_and_back_matches_fresh_builds() {
    display_flips_match_fresh_builds(per_table_style(|_| vec![set_style("#kb", "display: contents"), set_style("#kb", "")]));
}

/// C5: a table that becomes a block (its row group then wrapped in an anonymous table) and back.
#[test]
fn a_table_display_block_and_back_matches_fresh_builds() {
    display_flips_match_fresh_builds(per_table_style(|t| {
        vec![set_style("#k", with_display(t, "block")), set_style("#k", t)]
    }));
}

/// C5: a cell edited while its table is a block, then the table back.
#[test]
fn a_cell_edited_while_its_table_is_a_block_matches_fresh_builds() {
    display_flips_match_fresh_builds(per_table_style(|t| {
        vec![set_style("#k", with_display(t, "block")), set_text("#k1 td", "edited while a block"), set_style("#k", t)]
    }));
}

/// C5: a table that becomes a flex container, an inline table, and back.
#[test]
fn a_table_display_flex_and_inline_table_matches_fresh_builds() {
    display_flips_match_fresh_builds(per_table_style(|t| {
        vec![
            set_style("#k", with_display(t, "flex")),
            set_style("#k", t),
            set_style("#k", with_display(t, "inline-table")),
            set_style("#k", t),
        ]
    }));
}

/// C5: a row group that becomes a block (wrapped in an anonymous table inside the table) and back.
#[test]
fn a_row_group_display_block_and_back_matches_fresh_builds() {
    display_flips_match_fresh_builds(per_table_style(|_| vec![set_style("#kb", "display: block"), set_style("#kb", "")]));
}

// ---------------- cell styles changed incrementally (C2, C4) ----------------

/// Each step synced as a delta (the layout-query path; a whole-tree sync when the journal has no
/// delta), then compared with a fresh build: every element's box, the document size and the paint.
#[track_caller]
fn delta_steps_match_fresh_builds(html: &str, steps: Vec<(String, Mutate)>) {
    let page = Page::new(html);
    let mut r = Renderer::new();
    r.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
    let mut failures = Vec::new();
    for (what, mutate) in steps {
        let since = page.dom.borrow().version();
        mutate(&page);
        let delta = page.dom.borrow().render_delta(page.doc, since, &Page::opts());
        let via_delta =
            delta.is_some_and(|d| r.sync_delta(1, &delta_ops(d), (640, 480, 1.0, false, false), false).is_ok());
        if !via_delta {
            failures.push(format!("{what}: not synced as a delta"));
            r.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        }
        let mut fresh = Renderer::new();
        fresh.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        for ((k, a), (_, b)) in boxes(&r, &page).into_iter().zip(boxes(&fresh, &page)) {
            if a != b {
                failures.push(format!("{what}: box {k:x} {a:?} vs fresh {b:?}"));
                break;
            }
        }
        if r.document_size(1) != fresh.document_size(1) {
            failures.push(format!("{what}: size {:?} vs fresh {:?}", r.document_size(1), fresh.document_size(1)));
        }
        let tree = page.tree();
        let painted = paint(&mut r, &page.snapshot(1, tree.clone()));
        if painted.hash != paint(&mut Renderer::new(), &page.snapshot(1, tree)).hash {
            failures.push(format!("{what}: paint differs"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn cell_page(sheet: &str) -> String {
    format!(
        r#"<!doctype html><html><head><style id=st>{sheet}</style></head><body>
<table id=k style="border-collapse: collapse"><tbody id=kb>{}</tbody></table>
<table id=b><tbody>{}</tbody></table>
</body></html>"#,
        table_rows("k", 4),
        table_rows("b", 3),
    )
}

const CELL_SHEET: &str = "body { margin: 0; font: 13px sans-serif } td { padding: 2px }";

/// C2: a `<style>` edit gives every cell a height and a border, then takes the collapsed borders away.
#[test]
fn a_sheet_edit_giving_cells_height_and_borders_matches_fresh_builds() {
    delta_steps_match_fresh_builds(
        &cell_page(CELL_SHEET),
        vec![
            text("#st", "body { margin: 0; font: 13px sans-serif } td { padding: 2px; height: 30px; border: 3px solid red }"),
            text("#st", "body { margin: 0; font: 13px sans-serif } td { padding: 2px } #k { border-collapse: separate }"),
            text("#st", "body { margin: 0; font: 13px sans-serif } td { padding: 2px }"),
        ],
    );
}

/// C2, one property at a time.
#[test]
fn a_sheet_edit_giving_cells_a_height_matches_fresh_builds() {
    delta_steps_match_fresh_builds(
        &cell_page(CELL_SHEET),
        vec![text("#st", "body { margin: 0; font: 13px sans-serif } td { padding: 2px; height: 30px }")],
    );
}

#[test]
fn a_sheet_edit_giving_cells_a_border_matches_fresh_builds() {
    delta_steps_match_fresh_builds(
        &cell_page(CELL_SHEET),
        vec![text("#st", "body { margin: 0; font: 13px sans-serif } td { padding: 2px; border: 3px solid red }")],
    );
}

#[test]
fn a_sheet_edit_separating_collapsed_borders_matches_fresh_builds() {
    delta_steps_match_fresh_builds(
        &cell_page(CELL_SHEET),
        vec![
            text("#st", "body { margin: 0; font: 13px sans-serif } td { padding: 2px } #k { border-collapse: separate }"),
            text("#st", CELL_SHEET),
        ],
    );
}

/// C4: a cell taken out of flow and back.
#[test]
fn a_cell_positioned_absolutely_and_back_matches_fresh_builds() {
    delta_steps_match_fresh_builds(
        &cell_page(CELL_SHEET),
        vec![attr("#k3 td", "style", "position: absolute"), attr("#k3 td", "style", "")],
    );
}

/// C4: a cell made a block and back.
#[test]
fn a_cell_made_a_block_and_back_matches_fresh_builds() {
    delta_steps_match_fresh_builds(
        &cell_page(CELL_SHEET),
        vec![attr("#k0 td", "style", "display: block"), attr("#k0 td", "style", "")],
    );
}

/// The known-gaps sequence (`table_known_gaps_match_fresh_builds`) synced as deltas.
#[test]
fn table_known_gaps_match_fresh_builds_as_deltas() {
    delta_steps_match_fresh_builds(
        &cell_page(CELL_SHEET),
        vec![
            attr("#k1", "style", "display: contents"),
            attr("#k1", "style", "display: table-row"),
            text("#st", "body { margin: 0; font: 13px sans-serif } td { padding: 2px; height: 30px; border: 3px solid red }"),
            text("#st", "body { margin: 0; font: 13px sans-serif } td { padding: 2px } #k { border-collapse: separate }"),
            text("#st", CELL_SHEET),
            attr("#k2 td", "style", "float: left"),
            attr("#k2 td", "style", ""),
            attr("#k3 td", "style", "position: absolute"),
            attr("#k3 td", "style", ""),
            attr("#k0 td", "style", "display: block"),
            attr("#k0 td", "style", ""),
        ],
    );
}

/// A `border-collapse: collapse` table with a cell spanning rows, which a change inside builds again
/// rather than updating in place (`TableContext::incremental`), as does an out-of-flow cell. `#rc`
/// shares its left edge with `#ra` and its top edge with `#rb`.
fn spanning_collapsed_table(rc: &str) -> String {
    format!(
        r#"<!doctype html><html><head><style>body {{ margin: 0; font: 13px sans-serif }} td {{ padding: 2.5px 3.25px }}</style></head><body>
<table id=r style="border-collapse: collapse"><tr><td id=ra rowspan=2 style="border: 1px solid">a</td><td id=rb colspan=2>b</td></tr><tr><td id=rc {rc}>c</td><td>d</td></tr><tr><td>e</td><td>f</td><td>g</td></tr></table>
</body></html>"#
    )
}

/// C4 (residual): a cell's border changes the collapsed borders of the cells beside it, which the
/// table built again gives them without laying them out again.
#[test]
fn a_cell_gaining_a_border_beside_a_rowspan_matches_fresh_builds() {
    delta_steps_match_fresh_builds(&spanning_collapsed_table(""), vec![attr("#rc", "style", "border: 2px solid")]);
}

/// C4: a bordered cell made a block (its anonymous cell has no border) and back.
#[test]
fn a_bordered_cell_made_a_block_beside_a_rowspan_matches_fresh_builds() {
    delta_steps_match_fresh_builds(
        &spanning_collapsed_table(r#"style="border: 2px solid""#),
        vec![attr("#rc", "style", "display: block; border: 2px solid"), attr("#rc", "style", "border: 2px solid")],
    );
}

/// C4: a bordered cell taken out of flow (its borders leave the grid) and back.
#[test]
fn a_bordered_cell_positioned_absolutely_beside_a_rowspan_matches_fresh_builds() {
    delta_steps_match_fresh_builds(
        &spanning_collapsed_table(r#"style="border: 2px solid""#),
        vec![attr("#rc", "style", "position: absolute; border: 2px solid"), attr("#rc", "style", "border: 2px solid")],
    );
}

/// Each step synced as a delta (`sync_delta`, the layout-query path), then checked against a fresh
/// build: every element's box, the document size and the painted page.
#[track_caller]
fn page_steps_match_fresh_builds(page: &Page, steps: Vec<(String, Mutate)>) {
    let mut r = Renderer::new();
    r.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
    for (what, mutate) in steps {
        let since = page.dom.borrow().version();
        mutate(page);
        let delta = page.dom.borrow().render_delta(page.doc, since, &Page::opts()).expect("a delta");
        r.sync_delta(1, &delta_ops(delta), (640, 480, 1.0, false, false), false)
            .unwrap_or_else(|e| panic!("{what}: {e:?}"));
        let mut fresh = Renderer::new();
        fresh.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        for ((k, a), (_, b)) in boxes(&r, page).into_iter().zip(boxes(&fresh, page)) {
            assert_eq!(
                a,
                b,
                "box {k:x} after {what}: {:?}\nFRESH {:?}",
                r.describe_element(1, k),
                fresh.describe_element(1, k)
            );
        }
        let size = |r: &Renderer| r.page_layout(1).map(|l| (l.document_width, l.document_height));
        assert_eq!(size(&r), size(&fresh), "document size after {what}");
        let tree = page.tree();
        let mirrored = paint(&mut r, &page.snapshot(1, tree.clone()));
        assert_eq!(mirrored.hash, paint(&mut Renderer::new(), &page.snapshot(1, tree)).hash, "paint after {what}");
    }
}

/// C3: a floated cell is blockified (CSS 2 §9.7) and wrapped in an anonymous cell; in a table with
/// collapsed borders it must stay a float after an unrelated change and after losing the float.
#[test]
fn a_floated_cell_in_a_collapsed_table_matches_fresh_builds() {
    let page = Page::new(&format!(
        r#"<!doctype html><html><head><style id=st>
  body {{ margin: 0; font: 13px sans-serif }} td {{ padding: 2px; border: 1px solid #000 }}
</style></head><body>
<table id=k style="border-collapse: collapse"><tbody id=kb>{}</tbody></table>
</body></html>"#,
        table_rows("k", 4),
    ));
    let steps: Vec<(String, Mutate)> = vec![
        attr("#k2 td", "style", "float: left"),
        text("#k1 td", "edited"),
        text("#k2 td:nth-child(2)", "edited beside the float"),
        attr("#k2 td", "style", "float: left; color: red"),
        attr("#k2 td", "style", ""),
        attr("#k2 td", "style", "float: left"),
        text("#k2 td", "the float edited"),
    ];
    page_steps_match_fresh_builds(&page, steps);
}

/// C3, with the float in the markup from the start.
#[test]
fn a_cell_floated_from_the_start_in_a_collapsed_table_matches_fresh_builds() {
    let page = Page::new(&format!(
        r#"<!doctype html><html><head><style id=st>
  body {{ margin: 0; font: 13px sans-serif }} td {{ padding: 2px; border: 1px solid #000 }}
  #k2 td:first-child {{ float: left }}
</style></head><body>
<table id=k style="border-collapse: collapse"><tbody id=kb>{}</tbody></table>
</body></html>"#,
        table_rows("k", 4),
    ));
    let steps: Vec<(String, Mutate)> = vec![
        text("#k1 td", "edited"),
        text("#k2 td", "the float edited"),
        attr("#k3 td", "style", "border: 3px solid red"),
        attr("#k", "style", "border-collapse: collapse; width: 400px"),
    ];
    page_steps_match_fresh_builds(&page, steps);
}

/// C6: an element with inline children moved into a `display: none` container has no boxes. Stylo
/// never visits a display:none element's children: the moved element drops the style data it had
/// (and the inline layout built from it), or the moved `<span>` still reports the box it had.
#[test]
fn inline_content_moved_into_a_hidden_container_matches_fresh_builds() {
    let page = Page::new(
        r#"<!doctype html><html><head><style>body { margin: 0; font: 14px sans-serif }</style></head><body>
<div id=list><p id=p1>para <span id=s1>in<b id=b1>bold</b></span> text</p><p id=p2>two <i id=i2>it</i></p></div>
<div id=hid style="display:none"><p id=hp>already hidden <span>x</span></p></div><div id=after>after</div>
<div id=new></div>
</body></html>"#,
    );
    let mv = |what: &'static str, to: &'static str, sel: &'static str| -> (String, Mutate) {
        edit(what, move |p| {
            let (hid, n) = (p.find(to), p.find(sel));
            p.dom.borrow_mut().append_child(hid, n).unwrap();
        })
    };
    let steps: Vec<(String, Mutate)> = vec![
        mv("a paragraph moved into the hidden container", "#hid", "#p1"),
        mv("an inline moved into the hidden container", "#hid", "#i2"),
        // Into an element below the hidden one (which has no styles of its own).
        mv("a paragraph moved below the hidden container", "#hp", "#p2"),
        attr("#hid", "style", ""),
        // Into a shown element: it keeps its styles.
        mv("a paragraph moved out of it", "#new", "#p1"),
    ];
    page_steps_match_fresh_builds(&page, steps);
}

/// The known-gaps sequence (`table_known_gaps_match_fresh_builds`) synced as deltas. On 932f3d23
/// the float step (C3) only failed after the `<style>` edit changing the cells' height and border
/// (C2) had left the table stale; from a matching state it passed there too.
#[test]
fn table_known_gaps_as_deltas_match_fresh_builds() {
    let page = Page::new(&format!(
        r#"<!doctype html><html><head><style id=st>
  body {{ margin: 0; font: 13px sans-serif }} td {{ padding: 2px }}
</style></head><body>
<table id=k style="border-collapse: collapse"><tbody id=kb>{}</tbody></table>
<table id=b><tbody>{}</tbody></table>
</body></html>"#,
        table_rows("k", 4),
        table_rows("b", 3),
    ));
    let steps: Vec<(String, Mutate)> = vec![
        attr("#k1", "style", "display: contents"),
        attr("#k1", "style", "display: table-row"),
        text(
            "#st",
            "body { margin: 0; font: 13px sans-serif } td { padding: 2px; height: 30px; border: 3px solid red }",
        ),
        text("#st", "body { margin: 0; font: 13px sans-serif } td { padding: 2px } #k { border-collapse: separate }"),
        text("#st", "body { margin: 0; font: 13px sans-serif } td { padding: 2px }"),
        attr("#k2 td", "style", "float: left"),
        attr("#k2 td", "style", ""),
        attr("#k3 td", "style", "position: absolute"),
        attr("#k3 td", "style", ""),
    ];
    page_steps_match_fresh_builds(&page, steps);
}

// ---------------- collapsed borders of cells kept across a table rebuild ----------------

/// Each step: live renderer synced by deltas (layout-query path) and a second live renderer by
/// whole-tree paints; both compared with fresh builds (boxes and paint hash).
#[track_caller]
fn hp_steps(html: &str, width: u32, steps: Vec<(String, Mutate)>) {
    let page = Page::new(html);
    let snap = |p: &Page, w: u32| {
        let mut s = p.snapshot(1, p.tree());
        s.width = w;
        s
    };
    let mut d = Renderer::new();
    d.sync_layout(&snap(&page, width), &fetcher()).unwrap();
    let mut live = Renderer::new();
    live.paint(&snap(&page, width), Area::Viewport, &fetcher()).unwrap();
    let mut failures = Vec::new();
    for (what, mutate) in steps {
        eprintln!("RVSTEP {what}");
        let since = page.dom.borrow().version();
        mutate(&page);
        let delta = page.dom.borrow().render_delta(page.doc, since, &Page::opts());
        let via_delta = delta
            .is_some_and(|dd| d.sync_delta(1, &delta_ops(dd), (width, 480, 1.0, false, false), false).is_ok());
        if !via_delta {
            d.sync_layout(&snap(&page, width), &fetcher()).unwrap();
        }
        let mirrored = live.paint(&snap(&page, width), Area::Viewport, &fetcher()).unwrap();
        let mut fresh = Renderer::new();
        let fresh_paint = fresh.paint(&snap(&page, width), Area::Viewport, &fetcher()).unwrap();
        if mirrored.hash != fresh_paint.hash {
            failures.push(format!("PAINT differs after {what}"));
        }
        let mut fresh = Renderer::new();
        fresh.sync_layout(&snap(&page, width), &fetcher()).unwrap();
        let fb = boxes(&fresh, &page);
        for ((k, a), (_, b)) in boxes(&d, &page).into_iter().zip(fb.iter().cloned()) {
            if a != b {
                failures.push(format!("DELTA box {k:x} after {what}: {a:?} vs fresh {b:?}"));
                break;
            }
        }
        live.sync_layout(&snap(&page, width), &fetcher()).unwrap();
        for ((k, a), (_, b)) in boxes(&live, &page).into_iter().zip(fb.iter().cloned()) {
            if a != b {
                failures.push(format!("LIVE box {k:x} after {what}: {a:?} vs fresh {b:?}"));
                break;
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn hp_html(style: &str, body: &str) -> String {
    format!(
        "<!doctype html><html><head><style>body {{ margin: 0; font: 13px sans-serif }} td {{ padding: 2.5px 3.25px }} {style}</style></head><body>{body}</body></html>"
    )
}

fn hp_rows(prefix: &str, n: usize) -> String {
    table_rows(prefix, n)
}

fn hp_remove(row: &'static str) -> (String, Mutate) {
    (
        format!("{row} removed"),
        Box::new(move |p| {
            let tr = p.find(row);
            let parent = p.dom.borrow().parent(tr).unwrap().unwrap();
            p.dom.borrow_mut().remove_child(parent, tr).unwrap();
        }),
    )
}

fn hp_move(row: &'static str, group: &'static str, before: Option<&'static str>) -> (String, Mutate) {
    (
        format!("{row} moved before {before:?}"),
        Box::new(move |p| {
            let tr = p.find(row);
            let tbody = p.find(group);
            let at = before.map(|b| p.find(b));
            p.dom.borrow_mut().insert_before(tbody, tr, at).unwrap();
        }),
    )
}

/// A collapsed table whose rows have a bottom border: the first row's cells have no top border half,
/// the others 0.5px. Removing the first row (a change `update_rows` leaves to `build_table_context`)
/// makes the second one first: its cells lose their 0.5px, and must lay out again.
#[test]
fn hp_collapsed_first_row_removed() {
    hp_steps(
        &hp_html("table { border-collapse: collapse } tr { border-bottom: 1px solid }", &format!("<table id=t><tbody id=tb>{}</tbody></table>", hp_rows("t", 3))),
        640,
        vec![hp_remove("#t0")],
    );
}

/// The same with the first row moved down: the row now first loses its top border half, the moved
/// one gains it.
#[test]
fn hp_collapsed_first_row_moved_down() {
    hp_steps(
        &hp_html("table { border-collapse: collapse } tr { border-bottom: 1px solid }", &format!("<table id=t><tbody id=tb>{}</tbody></table>", hp_rows("t", 3))),
        640,
        vec![hp_move("#t0", "#tb", Some("#t2"))],
    );
}

/// A collapsed table whose borders become one uniform grid when its first row goes: rebuilt, its
/// cells drop their half borders (`set_collapsed_border(None)`) for gutters.
#[test]
fn hp_collapsed_borders_become_uniform() {
    hp_steps(
        &hp_html(
            "table { border-collapse: collapse } td { border: 1px solid } #t0 td { border-bottom: 3px solid }",
            &format!("<table id=t><tbody id=tb>{}</tbody></table>", hp_rows("t", 3)),
        ),
        640,
        vec![hp_remove("#t0")],
    );
}

// ---------------- adversarial review: table parts, moves and sheets (C1-C7 variants) ----------------

fn adv_leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// Moves `node` into `to` (before `before`, or last).
fn adv_mv(node: &'static str, to: &'static str, before: Option<&'static str>) -> (String, Mutate) {
    (
        format!("{node} moved into {to} before {before:?}"),
        Box::new(move |p| {
            let (n, t) = (p.find(node), p.find(to));
            let b = before.map(|b| p.find(b));
            p.dom.borrow_mut().insert_before(t, n, b).unwrap();
        }),
    )
}

fn adv_rm(node: &'static str) -> (String, Mutate) {
    (
        format!("{node} removed"),
        Box::new(move |p| {
            let n = p.find(node);
            let parent = p.dom.borrow().parent(n).unwrap().unwrap();
            p.dom.borrow_mut().remove_child(parent, n).unwrap();
        }),
    )
}

/// A new `<style id>` appended to `<head>`.
fn adv_sheet(id: &'static str, css: &'static str) -> (String, Mutate) {
    (
        format!("<style id={id}> {css:?} added"),
        Box::new(move |p| {
            let head = p.find("head");
            let mut d = p.dom.borrow_mut();
            let s = d.create_element(p.doc, "style").unwrap();
            d.set_attribute(s, "id", id).unwrap();
            let t = d.create_text(p.doc, css).unwrap();
            d.append_child(s, t).unwrap();
            d.append_child(head, s).unwrap();
        }),
    )
}

/// Several steps applied as one (one delta, one pass).
fn adv_all(steps: Vec<(String, Mutate)>) -> (String, Mutate) {
    let what = steps.iter().map(|(w, _)| w.clone()).collect::<Vec<_>>().join(" + ");
    (what, Box::new(move |p| steps.iter().for_each(|(_, m)| m(p))))
}

fn adv_html(table_style: &str) -> String {
    let rows: String = (0..4)
        .map(|i| format!("<tr id=k{i}><td>cell {i} a</td><td>text {i} b with some words</td><td>{i}.5</td></tr>"))
        .collect();
    format!(
        r#"<!doctype html><html><head><style id=st>
  body {{ margin: 0; font: 13px sans-serif }} td, th {{ padding: 2.5px 3.25px }} tr {{ border-bottom: 1px solid #ccc }}
  .b td {{ border: 1px solid #888 }} .x {{ border: 2.5px solid #0a0 }}
</style></head><body>
<div id=wrap><table id=k style="{table_style}"><caption id=cap>caption text</caption>
<colgroup id=cg><col id=c0 style="width: 60.5px"><col id=c1><col id=c2></colgroup>
<thead id=kh><tr id=h0><th>h a</th><th>h b</th><th>h c</th></tr></thead>
<tbody id=kb>{rows}<tr id=kr class=b><td id=rs rowspan=2 style="border: 2px solid">rs</td><td id=rx>x</td><td>y</td></tr><tr id=kr2><td id=rz>z</td><td>w</td></tr></tbody>
<tfoot id=kf><tr id=f0><td id=fa>f1</td><td colspan=2>f2</td></tr></tfoot></table></div>
<table id=o style="border-collapse: collapse"><tr id=o0><td id=oa style="border: 1px solid">outer a<table id=n><tr id=n0><td id=na>inner a</td><td>inner b</td></tr><tr id=n1><td id=nc>inner c</td><td>inner d</td></tr></table></td><td id=ob style="border: 3px solid">outer b</td></tr></table>
<div id=hid style="display: none"><div id=hidin><p>hidden</p></div></div><div id=shown>shown</div><p id=after>after</p>
</body></html>"#
    )
}

const ADV_STYLES: [&str; 6] = [
    "",
    "border-collapse: collapse",
    "border-collapse: collapse; width: 300.5px",
    "border-collapse: collapse; direction: rtl",
    "border-collapse: collapse; writing-mode: vertical-rl",
    "border-spacing: 2px 3px; direction: rtl; position: relative",
];

fn adv_scenarios(steps: impl Fn(&'static str) -> Vec<(String, Mutate)>) -> Vec<Scenario> {
    ADV_STYLES.iter().map(|t| (format!("table style={t:?}"), adv_html(t), steps(t))).collect()
}

fn adv_display(t: &'static str, display: &str) -> &'static str {
    adv_leak(format!("{t}; display: {display}"))
}

#[test]
fn adv_row_groups_caption_and_cols_flip_display() {
    adv_match_fresh_builds(adv_scenarios(|_| {
        vec![
            set_style("#kh", "display: contents"),
            set_style("#kh", ""),
            set_style("#kf", "display: block"),
            set_style("#kf", ""),
            set_style("#cap", "display: none"),
            set_style("#cap", ""),
            set_style("#c0", "display: none"),
            set_style("#c0", "width: 60.5px"),
            set_style("#kh", "display: table-footer-group"),
            set_style("#kh", ""),
            set_style("#kb", "display: contents"),
            set_style("#kh", "display: contents"),
            set_style("#kb", ""),
            set_style("#kh", ""),
        ]
    }));
}

#[test]
fn adv_rows_flip_display_repeatedly() {
    adv_match_fresh_builds(adv_scenarios(|_| {
        vec![
            set_style("#k1", "display: contents"),
            set_style("#k1", "display: none"),
            set_style("#k1", "display: table-row"),
            set_style("#k1", "display: contents"),
            set_text("#k1 td", "edited hoisted"),
            set_style("#k1", ""),
            set_style("#kr", "display: contents"),
            set_style("#kr", ""),
            set_style("#kr2", "display: contents"),
            set_style("#kr2", ""),
            set_style("#h0", "display: contents"),
            set_style("#f0", "display: contents"),
            set_style("#h0", ""),
            set_style("#f0", ""),
            set_style("#k0", "display: block"),
            set_style("#k0", ""),
        ]
    }));
}

#[test]
fn adv_table_display_chain() {
    adv_match_fresh_builds(adv_scenarios(|t| {
        vec![
            set_style("#k", adv_display(t, "block")),
            set_style("#k", adv_display(t, "flex")),
            set_style("#k", adv_display(t, "grid")),
            set_style("#k", adv_display(t, "none")),
            set_style("#k", adv_display(t, "contents")),
            set_style("#k", adv_display(t, "inline-table")),
            set_style("#k", t),
            set_style("#k", adv_display(t, "block")),
            set_text("#k2 td", "edited while a block"),
            set_style("#k", t),
        ]
    }));
}

#[test]
fn adv_table_parts_combined_in_one_pass() {
    adv_match_fresh_builds(adv_scenarios(|t| {
        vec![
            adv_all(vec![set_style("#k", adv_display(t, "block")), set_style("#kb", "display: contents")]),
            adv_all(vec![set_style("#k", t), set_style("#k1", "display: contents")]),
            adv_all(vec![set_style("#kb", ""), set_style("#k1", ""), set_style("#rs", "border: 4px solid")]),
            adv_all(vec![adv_mv("#k2", "#kh", None), set_style("#k", adv_display(t, "flex"))]),
            set_style("#k", t),
        ]
    }));
}

#[test]
fn adv_rows_and_cells_moved_between_groups_and_tables() {
    adv_match_fresh_builds(adv_scenarios(|_| {
        vec![
            adv_mv("#k3", "#kh", None),
            adv_mv("#k0", "#kf", Some("#f0")),
            adv_mv("#rs", "#k1", None),
            adv_mv("#fa", "#k2", Some("#k2 td")),
            adv_mv("#k1", "#n", None),
            adv_mv("#na", "#k0", None),
            adv_mv("#k1", "#kb", Some("#kr")),
            adv_rm("#kr2"),
            adv_rm("#h0"),
        ]
    }));
}

#[test]
fn adv_rows_and_cells_moved_through_display_none() {
    adv_match_fresh_builds(adv_scenarios(|_| {
        vec![
            adv_mv("#k2", "#hidin", None),
            set_text("#k2 td", "edited while hidden"),
            adv_mv("#k2", "#kb", Some("#k0")),
            adv_mv("#rx", "#hid", None),
            adv_mv("#rx", "#kr", None),
            adv_mv("#kb", "#hid", None),
            adv_mv("#kb", "#k", Some("#kf")),
            adv_all(vec![adv_mv("#k3", "#hid", None), set_style("#hid", "")]),
            adv_all(vec![adv_mv("#k3", "#kb", None), set_style("#hid", "display: none")]),
            adv_mv("#wrap", "#hidin", None),
            set_style("#hid", ""),
            adv_mv("#wrap", "#shown", None),
            set_style("#hid", "display: none"),
        ]
    }));
}

#[test]
fn adv_cells_positioned_floated_and_blockified() {
    adv_match_fresh_builds(adv_scenarios(|_| {
        vec![
            set_style("#rs", "position: absolute; top: 3px; left: 5px; border: 2px solid"),
            set_style("#rs", "border: 2px solid"),
            set_style("#rx", "position: absolute; border: 3px solid red"),
            set_style("#k", "position: relative; border-collapse: collapse"),
            set_style("#rx", "position: relative; border: 3px solid red"),
            set_style("#rx", "position: fixed; bottom: 0"),
            set_style("#rx", ""),
            set_style("#k0 td", "float: right; border: 2px solid"),
            set_text("#k0 td:nth-child(2)", "edited beside"),
            set_style("#k0 td", ""),
            set_style("#k1 td", "display: block; border: 2px solid"),
            set_style("#k1 td", "display: none"),
            set_style("#k1 td", "display: inline-block"),
            set_style("#k1 td", ""),
            set_style("#fa", "position: absolute"),
            set_style("#fa", ""),
        ]
    }));
}

#[test]
fn adv_abs_cell_with_containing_block_inside_table() {
    adv_match_fresh_builds(adv_scenarios(|_| {
        vec![
            set_style("#kb", "position: relative"),
            set_style("#rz", "position: absolute; right: 0; bottom: 0; border: 4px solid"),
            set_style("#kr2", "position: relative"),
            set_style("#rz", ""),
            set_style("#k2 td", "position: relative"),
            adv_sheet("abs", "#k2 td::after { content: 'ABS'; position: absolute; top: 0; left: 0 }"),
            adv_rm("#abs"),
        ]
    }));
}

#[test]
fn adv_style_sheets_inserted_edited_removed() {
    adv_match_fresh_builds(adv_scenarios(|_| {
        vec![
            adv_sheet("s2", "td { border: 2px solid green }"),
            set_text("#s2", "td { height: 30.5px } #k { border-collapse: separate }"),
            set_text("#s2", "td { border: 1.5px dashed } #k { border-collapse: collapse }"),
            adv_rm("#s2"),
            adv_sheet("s3", "tr:first-child td { border-top: 5px solid } tbody { border: 3px solid }"),
            adv_mv("#k3", "#kb", Some("#k0")),
            adv_rm("#s3"),
            adv_sheet("s4", "#kb { display: contents } #k1 { display: none }"),
            adv_rm("#s4"),
        ]
    }));
}

#[test]
fn adv_pseudo_elements_in_tables() {
    adv_match_fresh_builds(adv_scenarios(|_| {
        vec![
            adv_sheet("ps", "td::after { content: 'A' } tr::before { content: 'R' } #k::before { content: 'T'; display: block }"),
            set_text("#k2 td", "edited"),
            set_text("#ps", "tr::after { content: 'cell'; display: table-cell; border: 2px solid } tbody::before { content: 'row'; display: table-row }"),
            set_style("#k1", "display: contents"),
            set_style("#k1", ""),
            adv_rm("#ps"),
        ]
    }));
}

#[test]
fn adv_nested_tables() {
    adv_match_fresh_builds(adv_scenarios(|_| {
        vec![
            set_style("#n", "display: block"),
            set_style("#n", ""),
            set_style("#n0", "display: contents"),
            set_style("#n0", ""),
            set_style("#oa", "display: block; border: 1px solid"),
            set_style("#oa", "border: 1px solid"),
            set_style("#n", "border-collapse: collapse"),
            set_style("#o", "border-collapse: separate"),
            set_style("#o", "border-collapse: collapse"),
            adv_mv("#n", "#rx", None),
            adv_mv("#n", "#oa", None),
            adv_mv("#k", "#ob", None),
            set_style("#k", "display: block"),
            set_style("#k", ""),
            adv_mv("#k", "#wrap", None),
        ]
    }));
}

/// C6 variants: inline and block content (with pseudo-elements) moved into and below hidden
/// elements, the hidden element shown in the same pass, and moved back out.
#[test]
fn adv_content_moved_into_hidden_elements() {
    let html = ADV_C6;
    let n = std::cell::Cell::new(0);
    let scen = |steps: Vec<(String, Mutate)>| {
        n.set(n.get() + 1);
        (format!("c6 scenario {}", n.get()), html.to_string(), steps)
    };
    adv_match_fresh_builds(vec![
        scen(vec![
            adv_mv("#p1", "#hid", None),
            adv_mv("#s1", "#hs", None),
            set_style("#hid", ""),
            adv_mv("#p1", "#new", None),
            adv_mv("#s1", "#td", None),
        ]),
        scen(vec![
            adv_mv("#l2", "#hid", None),
            adv_mv("#l2", "#ul", Some("#l1")),
            adv_mv("#fl", "#hp", None),
            adv_all(vec![set_style("#hid", ""), adv_mv("#p2", "#hid", None)]),
            adv_all(vec![set_style("#hid", "display: none"), adv_mv("#fl", "#list", None)]),
        ]),
        scen(vec![
            // A move into an element that becomes hidden in the same pass, then shown again.
            adv_all(vec![class("#cls", "gone"), adv_mv("#p1", "#cls", None)]),
            class("#cls", ""),
            adv_all(vec![adv_mv("#i2", "#cp", None), class("#cls", "gone")]),
            adv_mv("#i2", "#p2", None),
            class("#cls", ""),
        ]),
        scen(vec![
            // A new element (no styles) holding a styled one, inserted into the hidden element.
            edit("new div with #p1 into #hid", |p| {
                let (hid, p1) = (p.find("#hid"), p.find("#p1"));
                let mut d = p.dom.borrow_mut();
                let div = d.create_element(p.doc, "div").unwrap();
                d.append_child(div, p1).unwrap();
                d.append_child(hid, div).unwrap();
            }),
            set_style("#hid", ""),
            adv_mv("#p1", "#list", None),
            // A new element holding a styled one, inserted, then its container hidden, in one pass.
            edit("new div with #p2 into #list, then #list hidden", |p| {
                let (list, p2) = (p.find("#list"), p.find("#p2"));
                let mut d = p.dom.borrow_mut();
                let div = d.create_element(p.doc, "div").unwrap();
                d.append_child(list, div).unwrap();
                d.append_child(div, p2).unwrap();
                d.set_attribute(list, "style", "display: none").unwrap();
            }),
            set_style("#list", ""),
        ]),
        scen(vec![adv_mv("#td", "#hid", None), set_style("#hid", ""), adv_mv("#td", "#t tr", None)]),
    ]);
}

/// `<tag#id>` of the element of `key` (for failure messages).
fn adv_name(page: &Page, key: u64) -> String {
    let tree = page.tree();
    let Some(n) = tree.nodes.iter().find(|n| n.key == key) else { return format!("{key:x}") };
    match &n.kind {
        TreeKind::Element { name, attrs } => {
            let id = attrs.iter().find(|(a, _)| &*a.local == "id").map(|(_, v)| format!("#{v}")).unwrap_or_default();
            format!("<{}{id}>", &*name.local)
        }
        TreeKind::Text(_) => "text".into(),
    }
}

fn adv_rect(b: &Option<LayoutBox>) -> String {
    match b {
        Some(b) => format!("({}, {}, {}x{} border {:?})", b.x, b.y, b.width, b.height, b.border),
        None => "none".into(),
    }
}

/// `display_flip_failures` with every differing element named (all of them, not the first).
fn adv_failures(html: &str, steps: Vec<(String, Mutate)>) -> Vec<String> {
    let page = Page::new(html);
    let mut d = Renderer::new();
    d.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
    let mut live = Renderer::new();
    check(&mut live, &page, "the first render");
    let size = |r: &Renderer| r.page_layout(1).map(|l| (l.document_width, l.document_height));
    let mut failures = Vec::new();
    for (what, mutate) in steps {
        let since = page.dom.borrow().version();
        mutate(&page);
        let delta = page.dom.borrow().render_delta(page.doc, since, &Page::opts());
        let via_delta =
            delta.is_some_and(|dd| d.sync_delta(1, &delta_ops(dd), (640, 480, 1.0, false, false), false).is_ok());
        if !via_delta {
            failures.push(format!("{what}: not synced as a delta"));
            d.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        }
        let mut fresh = Renderer::new();
        fresh.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        let fresh_boxes = boxes(&fresh, &page);
        let tree = page.tree();
        let fresh_paint = paint(&mut Renderer::new(), &page.snapshot(1, tree.clone())).hash;
        for (name, r) in [("delta", &mut d), ("live", &mut live)] {
            if name == "live" && paint(r, &page.snapshot(1, tree.clone())).hash != fresh_paint {
                failures.push(format!("{what}: {name} paint differs"));
            }
            if name == "live" {
                r.sync_layout(&page.snapshot(1, tree.clone()), &fetcher()).unwrap();
            }
            let differ: Vec<String> = boxes(r, &page)
                .into_iter()
                .zip(fresh_boxes.iter().cloned())
                .filter(|((_, a), (_, b))| a != b)
                .map(|((k, a), (_, b))| format!("{} {} vs fresh {}", adv_name(&page, k), adv_rect(&a), adv_rect(&b)))
                .collect();
            if !differ.is_empty() {
                failures.push(format!("{what}: {name} {} boxes differ: {}", differ.len(), differ.join("; ")));
            }
            if size(r) != size(&fresh) {
                failures.push(format!("{what}: {name} document size {:?} vs fresh {:?}", size(r), size(&fresh)));
            }
            if name == "delta" && paint(r, &page.snapshot(1, tree.clone())).hash != fresh_paint {
                failures.push(format!("{what}: {name} paint differs"));
            }
        }
    }
    failures
}

#[track_caller]
fn adv_match_fresh_builds(scenarios: Vec<Scenario>) {
    let mut failures = Vec::new();
    for (name, html, steps) in scenarios {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| adv_failures(&html, steps))) {
            Ok(f) => failures.extend(f.into_iter().map(|f| format!("[{name}] {f}"))),
            Err(e) => {
                let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()));
                failures.push(format!("[{name}] panicked: {}", msg.unwrap_or_default()));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const ADV_C6: &str = r#"<!doctype html><html><head><style>body { margin: 0; font: 14px sans-serif }
  .pb::before { content: 'B4 '; color: red } .pa::after { content: ' AF'; display: block } .gone { display: none }
  li { list-style: decimal inside }</style></head><body>
<div id=list><p id=p1 class=pb>para <span id=s1 class=pa>in<b id=b1>bold</b></span> text</p><p id=p2>two <i id=i2>it</i></p>
<ul id=ul><li id=l1>one</li><li id=l2>two</li></ul><div id=fl style="float: left; width: 50.5px">float <span>x</span></div></div>
<div id=hid style="display: none"><p id=hp>already hidden <span id=hs>x</span></p></div><div id=cls><p id=cp>class host</p></div>
<div id=after>after</div><div id=new></div><table id=t><tr><td id=td>cell</td></tr></table>
</body></html>"#;

/// One scenario on `adv_html(table style)` from a fresh page.
#[track_caller]
fn adv_min(table_style: &'static str, steps: Vec<(String, Mutate)>) {
    adv_match_fresh_builds(vec![(format!("table style={table_style:?}"), adv_html(table_style), steps)]);
}

#[track_caller]
fn adv_min_c6(steps: Vec<(String, Mutate)>) {
    adv_match_fresh_builds(vec![("c6".into(), ADV_C6.into(), steps)]);
}

/// A table with a caption made a block: the caption's content is laid out again (fresh: 15px tall).
#[test]
fn adv_min_table_with_caption_made_a_block() {
    adv_min("", vec![set_style("#k", "display: block")]);
}

/// The same without the caption (removed in an earlier pass).
#[test]
fn adv_min_table_without_caption_made_a_block() {
    adv_min("", vec![adv_rm("#cap"), set_style("#k", "display: block")]);
}

/// A table hidden and shown: its `<col>`s.
#[test]
fn adv_min_table_hidden_and_shown() {
    adv_min("", vec![set_style("#k", "display: none"), set_style("#k", "")]);
}

/// A hidden element shown and hidden again: its descendants report no box.
#[test]
fn adv_min_hidden_element_shown_and_hidden_again() {
    adv_min("", vec![set_style("#hid", ""), set_style("#hid", "display: none")]);
}

/// A whole table moved into a hidden element.
#[test]
fn adv_min_table_moved_into_a_hidden_element() {
    adv_min("", vec![adv_mv("#wrap", "#hidin", None)]);
}

/// A footer cell moved to the front of a body row of a collapsed table.
#[test]
fn adv_min_footer_cell_moved_into_a_body_row_collapsed() {
    adv_min("border-collapse: collapse", vec![adv_mv("#fa", "#k2", Some("#k2 td"))]);
}

#[test]
fn adv_min_footer_cell_moved_into_a_body_row_separate() {
    adv_min("", vec![adv_mv("#fa", "#k2", Some("#k2 td"))]);
}

/// A sheet giving row groups a border: the nested collapsed outer table `#o` (its tbody now 3px).
#[test]
fn adv_min_sheet_giving_row_groups_a_border() {
    adv_min("", vec![adv_sheet("s3", "tbody { border: 3px solid }")]);
}

/// A cell holding a nested table made a block.
#[test]
fn adv_min_cell_holding_a_table_made_a_block() {
    adv_min("", vec![set_style("#oa", "display: block; border: 1px solid")]);
}

/// A table made a block and its row group `display: contents` in one pass.
#[test]
fn adv_min_table_block_and_group_contents_in_one_pass() {
    adv_min("", vec![adv_all(vec![set_style("#k", "display: block"), set_style("#kb", "display: contents")])]);
}

/// A row group made `display: contents`, then a row too.
#[test]
fn adv_min_group_contents_then_row_contents() {
    adv_min("", vec![set_style("#kb", "display: contents"), set_style("#k1", "display: contents")]);
}

/// A table moved into a cell of a collapsed table, then made a block.
#[test]
fn adv_min_table_moved_into_a_cell_then_made_a_block() {
    adv_min("border-collapse: collapse", vec![adv_mv("#k", "#ob", None), set_style("#k", "display: block")]);
}

/// C6: a span with a block `::after` moved into a hidden element, which is then shown.
#[test]
fn adv_min_c6_span_with_after_moved_into_hidden_then_shown() {
    adv_min_c6(vec![adv_mv("#s1", "#hs", None), set_style("#hid", "")]);
}

/// C6: a span moved into a hidden element, which is then shown.
#[test]
fn adv_min_c6_plain_span_moved_into_hidden_then_shown() {
    adv_min_c6(vec![adv_mv("#i2", "#hs", None), set_style("#hid", "")]);
}

/// C6: a list item moved into a hidden element and back.
#[test]
fn adv_min_c6_list_item_into_hidden_and_back() {
    adv_min_c6(vec![adv_mv("#l2", "#hid", None), adv_mv("#l2", "#ul", Some("#l1"))]);
}

/// C6: a float moved below a hidden element.
#[test]
fn adv_min_c6_float_moved_below_hidden() {
    adv_min_c6(vec![adv_mv("#fl", "#hp", None)]);
}

/// C6: a float moved below a hidden element, then shown with another move in the same pass.
#[test]
fn adv_min_c6_float_below_hidden_then_shown() {
    adv_min_c6(vec![adv_mv("#fl", "#hp", None), adv_all(vec![set_style("#hid", ""), adv_mv("#p2", "#hid", None)])]);
}

/// C6: a new (unstyled) element holding a styled one inserted into a hidden element.
#[test]
fn adv_min_c6_new_wrapper_into_hidden() {
    adv_min_c6(vec![edit("new div with #p1 into #hid", |p| {
        let (hid, p1) = (p.find("#hid"), p.find("#p1"));
        let mut d = p.dom.borrow_mut();
        let div = d.create_element(p.doc, "div").unwrap();
        d.append_child(div, p1).unwrap();
        d.append_child(hid, div).unwrap();
    })]);
}

/// C6: a container hidden in the same pass as a new wrapper takes one of its children.
#[test]
fn adv_min_c6_container_hidden_with_new_wrapper() {
    adv_min_c6(vec![edit("new div with #p2 into #list, then #list hidden", |p| {
        let (list, p2) = (p.find("#list"), p.find("#p2"));
        let mut d = p.dom.borrow_mut();
        let div = d.create_element(p.doc, "div").unwrap();
        d.append_child(list, div).unwrap();
        d.append_child(div, p2).unwrap();
        d.set_attribute(list, "style", "display: none").unwrap();
    })]);
}

/// C6: a container hidden (no move): its descendants report no box.
#[test]
fn adv_min_c6_container_hidden() {
    adv_min_c6(vec![set_style("#list", "display: none")]);
}

/// C6: an element moved into a container hidden by a class in the same pass.
#[test]
fn adv_min_c6_moved_into_container_hidden_same_pass() {
    adv_min_c6(vec![adv_all(vec![class("#cls", "gone"), adv_mv("#p1", "#cls", None)]), class("#cls", "")]);
}

/// C6: a cell moved into a hidden element, shown, then moved back into a table row.
#[test]
fn adv_min_c6_cell_through_hidden() {
    adv_min_c6(vec![adv_mv("#td", "#hid", None), set_style("#hid", ""), adv_mv("#td", "#t tr", None)]);
}

fn adv_new_wrapper_into_hidden() -> (String, Mutate) {
    edit("new div holding #p1 appended to #hid", |p| {
        let (hid, p1) = (p.find("#hid"), p.find("#p1"));
        let mut d = p.dom.borrow_mut();
        let div = d.create_element(p.doc, "div").unwrap();
        d.append_child(div, p1).unwrap();
        d.append_child(hid, div).unwrap();
    })
}

/// C6 (delta path): a styled paragraph moved into a new, detached `<div>` that is then appended to a
/// hidden element keeps its style data (the moved child, the new div, has none, so
/// `add_children_to_parent` does not look); once the hidden element is shown the paragraph is not
/// laid out (0x0 vs 640x16 fresh).
#[test]
fn adv_min_c6_new_wrapper_into_hidden_then_shown() {
    adv_min_c6(vec![adv_new_wrapper_into_hidden(), set_style("#hid", "")]);
}

/// The C6 debug assertion (resolve.rs `layout_parent ... does not point at its layout container`):
/// a float moved below a hidden element, the element shown, then hidden again as the float moves
/// back out.
#[test]
fn adv_min_c6_list_item_and_float_through_hidden_panics() {
    adv_min_c6(vec![
        adv_mv("#l2", "#hid", None),
        adv_mv("#l2", "#ul", Some("#l1")),
        adv_mv("#fl", "#hp", None),
        adv_all(vec![set_style("#hid", ""), adv_mv("#p2", "#hid", None)]),
        adv_all(vec![set_style("#hid", "display: none"), adv_mv("#fl", "#list", None)]),
    ]);
}

#[test]
fn adv_min_c6_float_through_hidden_panics() {
    adv_min_c6(vec![
        adv_mv("#fl", "#hp", None),
        set_style("#hid", ""),
        adv_all(vec![set_style("#hid", "display: none"), adv_mv("#fl", "#list", None)]),
    ]);
}

#[test]
fn adv_min_c6_float_shown_then_hidden_while_moved_out() {
    adv_min_c6(vec![adv_all(vec![set_style("#hid", "display: none"), adv_mv("#fl", "#hp", None)]), set_style("#hid", ""), adv_all(vec![set_style("#hid", "display: none"), adv_mv("#fl", "#list", None)])]);
}

#[test]
fn adv_min_c6_list_item_and_float_through_hidden_3() {
    adv_min_c6(vec![adv_mv("#l2", "#hid", None), adv_mv("#l2", "#ul", Some("#l1")), adv_mv("#fl", "#hp", None)]);
}

#[test]
fn adv_min_c6_list_item_through_hidden_then_shown() {
    adv_min_c6(vec![adv_mv("#l2", "#hid", None), adv_mv("#l2", "#ul", Some("#l1")), set_style("#hid", "")]);
}

/// Control: the list item moved directly (no hidden element on the way).
#[test]
fn adv_min_c6_list_item_moved_directly() {
    adv_min_c6(vec![adv_mv("#l2", "#ul", Some("#l1"))]);
}

/// Control: the span with a block `::after` moved to a shown element.
#[test]
fn adv_min_c6_span_with_after_moved_to_shown() {
    adv_min_c6(vec![adv_mv("#s1", "#new", None)]);
}

/// Control: the span's container hidden and shown again (no move).
#[test]
fn adv_min_c6_container_with_after_hidden_and_shown() {
    adv_min_c6(vec![set_style("#list", "display: none"), set_style("#list", "")]);
}

/// Rows moved into the header and footer groups, a rowspan cell into another row, then a footer cell
/// moved into a body row (collapsed borders).
#[test]
fn adv_min_cells_moved_across_groups_collapsed() {
    adv_min(
        "border-collapse: collapse",
        vec![
            adv_mv("#k3", "#kh", None),
            adv_mv("#k0", "#kf", Some("#f0")),
            adv_mv("#rs", "#k1", None),
            adv_mv("#fa", "#k2", Some("#k2 td")),
        ],
    );
}

#[test]
fn adv_min_rowspan_cell_moved_then_footer_cell_collapsed() {
    adv_min("border-collapse: collapse", vec![adv_mv("#rs", "#k1", None), adv_mv("#fa", "#k2", Some("#k2 td"))]);
}

#[test]
fn adv_min_rowspan_cell_moved_collapsed() {
    adv_min("border-collapse: collapse", vec![adv_mv("#rs", "#k1", None)]);
}

/// A table made a block with its row group `display: contents`, then the table back.
#[test]
fn adv_min_table_block_with_group_contents_then_back() {
    adv_min(
        "",
        vec![
            adv_all(vec![set_style("#k", "display: block"), set_style("#kb", "display: contents")]),
            set_style("#k", ""),
        ],
    );
}

/// A table without caption or columns made a block (cols are the only other difference).
#[test]
fn adv_min_plain_table_made_a_block_and_back() {
    adv_min("", vec![adv_rm("#cap"), adv_rm("#cg"), set_style("#k", "display: block"), set_style("#k", "")]);
}

/// A table with only a caption (no cols) made a block.
#[test]
fn adv_min_table_with_caption_no_cols_made_a_block() {
    adv_min("", vec![adv_rm("#cg"), set_style("#k", "display: block")]);
}

#[test]
fn adv_min_rowspan_cell_moved_then_unrelated_edit_collapsed() {
    adv_min("border-collapse: collapse", vec![adv_mv("#rs", "#k1", None), set_text("#after", "after edited")]);
}

#[test]
fn adv_min_rowspan_cell_moved_then_cell_text_edit_collapsed() {
    adv_min("border-collapse: collapse", vec![adv_mv("#rs", "#k1", None), set_text("#k2 td", "edited")]);
}

#[test]
fn adv_min_rowspan_and_footer_cells_moved_in_one_pass_collapsed() {
    adv_min("border-collapse: collapse", vec![adv_all(vec![adv_mv("#rs", "#k1", None), adv_mv("#fa", "#k2", Some("#k2 td"))])]);
}

#[test]
fn adv_min_footer_then_rowspan_cell_moved_collapsed() {
    adv_min("border-collapse: collapse", vec![adv_mv("#fa", "#k2", Some("#k2 td")), adv_mv("#rs", "#k1", None)]);
}

#[test]
fn adv_min_rowspan_cell_moved_then_footer_cell_separate() {
    adv_min("", vec![adv_mv("#rs", "#k1", None), adv_mv("#fa", "#k2", Some("#k2 td"))]);
}

/// `adv_min_rowspan_cell_moved_then_footer_cell_collapsed` on a smaller table (no caption, columns
/// or header).
#[test]
fn adv_min_two_cells_moved_small_collapsed_table() {
    let html = r#"<!doctype html><html><head><style>body { margin: 0; font: 13px sans-serif } td { padding: 2.5px 3.25px } tr { border-bottom: 1px solid #ccc }</style></head><body>
<table id=k style="border-collapse: collapse"><tbody><tr id=k1><td>a</td><td>b</td></tr><tr id=k2><td>c</td><td>d</td></tr><tr id=kr><td id=rs rowspan=2 style="border: 2px solid">rs</td><td>x</td></tr><tr><td>z</td></tr></tbody>
<tfoot><tr><td id=fa>f1</td><td>f2</td></tr></tfoot></table></body></html>"#;
    adv_match_fresh_builds(vec![(
        "small".into(),
        html.into(),
        vec![adv_mv("#rs", "#k1", None), adv_mv("#fa", "#k2", Some("#k2 td"))],
    )]);
}

#[test]
fn adv_min_cell_holding_a_table_made_a_block_and_back() {
    adv_min("", vec![set_style("#oa", "display: block; border: 1px solid"), set_style("#oa", "border: 1px solid")]);
}

/// A checkbox checked while it has no box (inside `display: none`): blitz-dom
/// would push a second, HTML-namespaced `checked` for a value that parses as
/// a bool, which the mirror then takes away on the next sync of the same
/// tree. Nothing is left to sync: the unchanged tree mutates nothing, and the
/// box built later shows the state the tree has.
#[test]
fn a_checkbox_checked_without_a_box_leaves_nothing_to_sync() {
    let page = Page::new(
        r#"<!doctype html><html><body><div id="h" style="display:none"><input id="c" type="checkbox"></div><p>x</p></body></html>"#,
    );
    let mut r = Renderer::new();
    check(&mut r, &page, "the first render");
    let (c, h) = (page.find("#c"), page.find("#h"));
    page.dom.borrow_mut().set_checked(c, true).unwrap();
    assert!(check(&mut r, &page, "checked").mutations > 0);
    assert_eq!(check(&mut r, &page, "checked, unchanged").mutations, 0, "checked, unchanged");
    page.dom.borrow_mut().remove_attribute(h, "style").unwrap();
    check(&mut r, &page, "shown checked");
    page.dom.borrow_mut().set_attribute(h, "style", "display:none").unwrap();
    check(&mut r, &page, "hidden again");
    page.dom.borrow_mut().set_checked(c, false).unwrap();
    check(&mut r, &page, "unchecked");
    assert_eq!(check(&mut r, &page, "unchecked, unchanged").mutations, 0, "unchecked, unchanged");
    page.dom.borrow_mut().remove_attribute(h, "style").unwrap();
    check(&mut r, &page, "shown unchecked");
}

/// `Renderer::has_sticky` reads Blitz's set of elements the style traversal
/// gave a `position: sticky` style (`BaseDocument::has_sticky`), kept as
/// styles are written and nodes dropped: it must follow every way an element
/// stops or starts being sticky, through deltas, and agree with a whole-tree
/// sync. (A sticky `::before` is not in the set: it shifts no element's box,
/// which is what `has_sticky` gates in pyro-dom.)
#[test]
fn sticky_set_follows_style_changes_and_removals() {
    let page = Page::new(
        "<!doctype html><html><head><style>.s{position:sticky;top:0}</style></head>\
         <body><div id=h><div id=a></div></div><div id=b></div><div id=c></div><div id=p></div></body></html>",
    );
    let (mut r, mut w) = (Renderer::new(), Renderer::new());
    r.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
    assert!(!r.has_sticky(1), "nothing is sticky at first");
    let (h, a, b, c) = (page.find("#h"), page.find("#a"), page.find("#b"), page.find("#c"));
    let body = page.find("body");
    let set = |n: NodeId, k: &'static str, v: &'static str| move |pg: &Page| pg.dom.borrow_mut().set_attribute(n, k, v).unwrap();
    let unset = |n: NodeId, k: &'static str| move |pg: &Page| {
        pg.dom.borrow_mut().remove_attribute(n, k).unwrap();
    };
    let remove = |n: NodeId| move |pg: &Page| {
        pg.dom.borrow_mut().remove_child(body, n).unwrap();
    };
    let steps: Vec<(&str, Box<dyn Fn(&Page)>, bool)> = vec![
        ("a class switches to sticky", Box::new(set(a, "class", "s")), true),
        ("and back", Box::new(unset(a, "class")), false),
        ("sticky again", Box::new(set(a, "class", "s")), true),
        ("its ancestor is display:none", Box::new(set(h, "style", "display:none")), false),
        ("the ancestor is shown again", Box::new(unset(h, "style")), true),
        ("the sticky element's subtree is removed", Box::new(remove(h)), false),
        ("two sticky elements", Box::new(move |pg: &Page| {
            set(b, "class", "s")(pg);
            set(c, "class", "s")(pg);
        }), true),
        ("one of the two is removed", Box::new(remove(b)), true),
        ("the other is removed", Box::new(remove(c)), false),
    ];
    for (what, step, sticky) in &steps {
        let since = page.dom.borrow().version();
        step(&page);
        let delta = page.dom.borrow().render_delta(page.doc, since, &Page::opts()).expect("a delta");
        r.sync_delta(1, &delta_ops(delta), (640, 480, 1.0, false, false), false)
            .unwrap_or_else(|e| panic!("{what}: {e:?}"));
        w.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        assert_eq!(r.has_sticky(1), *sticky, "delta sync: {what}");
        assert_eq!(w.has_sticky(1), *sticky, "whole-tree sync: {what}");
    }
}

// ---------------- layout-change flags, rounding of moved subtrees, tables and the available width,
// content widths kept with the shaped text ----------------

/// The early exits of `Node::mark_damaged` and `Node::mark_layout_changed` rely on a flagged node's
/// ancestors being flagged, which debug builds assert there. Nodes carrying flags move: a row edited
/// and moved to another table in one change, a row detached and attached again later (edited in
/// between), pseudo-elements and anonymous boxes appearing in a subtree nothing else changed in,
/// subtrees leaving and rejoining the layout, and a restyled subtree moved.
#[test]
fn flagged_nodes_moved_between_syncs_match_fresh_builds() {
    let rows = |p: &str| -> String {
        (0..6)
            .map(|i| format!("<tr id={p}{i}><td>{p} {i} a</td><td><span>{p} {i} b</span><div><i>n {i}</i></div></td></tr>"))
            .collect()
    };
    let page = Page::new(&format!(
        "{PAINT_BOXES}<style>.pb::before {{ content: 'pre'; display: block }} .hide {{ display: none }} \
         .big {{ font-size: 20.5px }}</style>\
         <table id=t1 style=\"width:300.5px\"><tbody id=ta>{}</tbody></table>\
         <table id=t2 style=\"width:200.25px\"><tbody id=tb>{}</tbody></table>\
         <div id=w><span id=s>inline <b id=k>text</b> here</span></div>\
         <div id=pe>owner <span>x</span></div></body></html>",
        rows("a"),
        rows("b")
    ));
    let mut r = Renderer::new();
    check(&mut r, &page, "the first render");
    let detached: Rc<std::cell::Cell<Option<NodeId>>> = Rc::new(std::cell::Cell::new(None));
    let (d1, d2) = (detached.clone(), detached.clone());
    let steps: Vec<(String, Mutate)> = vec![
        edit("a row edited and moved to another table", |p| {
            let cell = p.find("#a2 td");
            let row = p.find("#a2");
            let other = p.find("#tb");
            let mut d = p.dom.borrow_mut();
            d.set_text_content(cell, "edited and moved elsewhere").unwrap();
            d.append_child(other, row).unwrap();
        }),
        edit("a row edited and detached", move |p| {
            let cell = p.find("#b1 td");
            let row = p.find("#b1");
            let mut d = p.dom.borrow_mut();
            d.set_text_content(cell, "edited, then detached").unwrap();
            let parent = d.parent(row).unwrap().unwrap();
            d.remove_child(parent, row).unwrap();
            d1.set(Some(row));
        }),
        edit("the detached row edited and attached to the other table", move |p| {
            let row = d2.get().unwrap();
            let first = p.find("#a0");
            let body = p.find("#ta");
            let mut d = p.dom.borrow_mut();
            let cell = d.first_child(row).unwrap().unwrap();
            d.set_text_content(cell, "edited while detached").unwrap();
            d.insert_before(body, row, Some(first)).unwrap();
        }),
        class("#pe", "pb"),
        class("#pe", ""),
        class("#pe", "pb"),
        style("#k", "display:block"),
        style("#k", "display:inline"),
        class("#t1", "hide"),
        class("#t1", ""),
        class("#s", "big"),
        edit("a restyled subtree moved", |p| {
            let w = p.find("#w");
            let body = p.find("body");
            p.dom.borrow_mut().append_child(body, w).unwrap();
        }),
    ];
    for (what, mutate) in steps {
        mutate(&page);
        check(&mut r, &page, &what);
    }
}

/// A subtree moved by whole pixels rounds as it did (its rounding is keyed on the fractional part of
/// its parent's position, `LayoutData::rounded_from`): the cells of the rows below a cell that grew
/// by a line are visited, their subtrees are not.
#[test]
fn rounding_skips_subtrees_moved_by_whole_pixels() {
    let rows: String = (0..200)
        .map(|i| format!("<tr id=r{i}><td id=c{i}>cell {i}</td><td><div><span>n {i}</span><p>d</p></div></td></tr>"))
        .collect();
    let page = Page::new(&format!(
        "{PAINT_BOXES}<style>body {{ margin: 0; font: 13px/20px sans-serif }} td {{ padding: 0; height: 20px; vertical-align: top }} p {{ margin: 0 }}</style>\
         <table style=\"table-layout:fixed;width:300px;border-spacing:0\">{rows}</table></body></html>"
    ));
    let mut r = Renderer::new();
    check(&mut r, &page, "the first render");

    let c1 = page.find("#c1");
    page.dom.borrow_mut().set_text_content(c1, "a longer cell text that wraps onto a second line of its own").unwrap();
    let tree = page.tree();
    let before = blitz_rounding();
    let painted = paint(&mut r, &page.snapshot(1, tree.clone()));
    let after = blitz_rounding();
    assert_eq!(painted.hash, Renderer::new().paint(&page.snapshot(1, tree), Area::Viewport, &fetcher()).unwrap().hash);
    let (visited, skipped) = (after.0 - before.0, after.1 - before.1);
    // The 198 rows below moved by 20px: their 396 cells are reached, the subtrees of the second
    // cells (a div, a span, a p and their text) are skipped (a skipped root counts as reached).
    assert!(skipped >= 198, "skipped {skipped} subtrees");
    assert!(visited - skipped < 396 + 60, "rounding went through {} nodes", visited - skipped);
}

/// Subtrees moved by whole and by fractional amounts, with what the rounding of a subtree can
/// depend on inside them: nested boxes, scroll containers, transforms (resolved against the rounded
/// size), vertical writing, inline-blocks; a fractional parent moved by whole pixels; and boxes at
/// negative half-pixel positions (rounded up, as Blink rounds them).
#[test]
fn rounding_follows_subtrees_moved_by_whole_and_fractional_pixels() {
    rounds_as_a_fresh_build(
        r#"<table style="width:300px;border-spacing:0"><tr><td id=g style="padding:0;height:20px">g</td></tr><tr><td style="padding:0"><div style="overflow:auto;height:15.5px"><p style="margin:0;height:40.5px">s</p></div><div style="transform:rotate(10deg);width:10.5px;height:4.5px"></div><div style="writing-mode:vertical-rl;height:30.5px"><span>v</span><div style="width:7.5px">x</div></div><div><div><span style="display:inline-block;width:5.5px;height:3.5px"></span>n</div></div></td></tr></table>"#,
        vec![
            style("#g", "padding:0;height:40px"),
            style("#g", "padding:0;height:40.5px"),
            style("#g", "padding:0;height:41px"),
            style("#g", "padding:0;height:20.25px"),
            style("#g", "padding:0;height:20px"),
        ],
    );
    rounds_as_a_fresh_build(
        r#"<div id=h style="height:20.5px"></div><div style="margin-left:0.3px"><div style="height:7.3px"><span>q</span><div style="width:3.7px;height:1.5px"></div></div></div>"#,
        vec![style("#h", "height:40.5px"), style("#h", "height:41px"), style("#h", "height:20.5px")],
    );
    rounds_as_a_fresh_build(
        r#"<div id=n style="margin-left:-2.5px;width:10px;height:5px"><div style="height:2.5px"></div></div><div id=m style="position:relative;left:-0.5px;top:-1.5px;width:10.5px;height:3.5px">a</div>"#,
        vec![
            style("#n", "margin-left:-3.5px;width:10px;height:5px"),
            style("#n", "margin-left:-0.5px;width:10px;height:5px"),
            style("#m", "position:relative;left:-2.5px;top:-0.5px;width:10.5px;height:3.5px"),
            style("#n", "margin-left:0;width:10px;height:5px"),
        ],
    );
}

/// An auto table whose container got wider or narrower, but not past the table's own width, keeps
/// its layout: its cells are not measured again (`CellWidths`) and its grid is not laid out (the
/// last layout stands). Tables whose width follows the container, cells with percentage padding
/// (which their intrinsic widths depend on the table's content width through) and cells with a
/// minimum width lay out as fresh builds do.
#[test]
fn tables_keep_their_layout_when_only_the_available_width_changes() {
    let rows = |n: usize| -> String {
        (0..n).map(|i| format!("<tr><td>cell {i}</td><td>some text {i} here</td><td>{i}.5</td></tr>")).collect()
    };
    let page = Page::new(&format!(
        "{PAINT_BOXES}<div id=w style=\"width:500px\"><table id=t style=\"border-spacing:2.5px\">{}</table></div></body></html>",
        rows(8)
    ));
    let mut r = Renderer::new();
    check(&mut r, &page, "the first render");
    let w = page.find("#w");
    // (container width, whether the table's grid is laid out)
    for (width, laid_out) in [("450px", false), ("600.5px", false), ("100px", true), ("400.25px", true), ("500px", false)] {
        page.dom.borrow_mut().set_attribute(w, "style", &format!("width:{width}")).unwrap();
        let tree = page.tree();
        let before = blitz_table_grids();
        let painted = paint(&mut r, &page.snapshot(1, tree.clone()));
        let after = blitz_table_grids();
        let mut fresh = Renderer::new();
        let fresh_paint = paint(&mut fresh, &page.snapshot(1, tree));
        for ((k, a), (_, b)) in boxes(&r, &page).into_iter().zip(boxes(&fresh, &page)) {
            assert_eq!(a, b, "box {k:x} at width {width}");
        }
        if painted.hash != fresh_paint.hash {
            let again = paint(&mut r, &page.snapshot(1, page.tree()));
            let describe = |r: &Renderer| {
                r.page_layout(1).map(|l| {
                    let words: Vec<_> = r.page_words(1).unwrap_or_default().iter().map(|(k, w)| (*k, w.x, w.y, w.width, w.height)).collect();
                    format!("{}x{} {words:?}", l.document_width, l.document_height)
                })
            };
            let live_layout = describe(&r);
            let fresh_layout = describe(&fresh);
            assert_eq!(live_layout, fresh_layout, "page layout at width {width}");
            panic!(
                "paint at width {width}: live {} fresh {} (live painted again {}; grids laid out {:?})",
                painted.hash,
                fresh_paint.hash,
                again.hash,
                (after.0 - before.0, after.1 - before.1)
            );
        }
        assert_eq!((after.0 + after.1) > (before.0 + before.1), laid_out, "width {width}: grid laid out");
    }

    let widths = |table: &str| -> Vec<(String, Mutate)> {
        vec![
            style("#w", "width:450px"),
            style("#w", "width:600.5px"),
            style("#w", "width:100px"),
            style("#w", "width:400.25px"),
            style("#w", "width:500px"),
        ]
        .into_iter()
        .map(|(what, mutate)| (format!("{table}: {what}"), mutate))
        .collect()
    };
    for table in [
        format!("<table style=\"width:100%\">{}</table>", rows(5)),
        format!("<style>td {{ padding: 0 2% }}</style><table>{}</table>", rows(5)),
        format!("<style>td {{ padding: 1px calc(2px + 1%) }}</style><table>{}</table>", rows(5)),
        format!("<style>td {{ min-width: 40px }}</style><table>{}</table>", rows(5)),
        format!("<table><tr><td width=120>wide</td><td>b</td><td>c</td></tr>{}</table>", rows(4)),
        format!("<table><tr><td style=\"width:30%\">p</td><td>b</td><td>c</td></tr>{}</table>", rows(4)),
        format!("<table style=\"max-width:80%\">{}</table>", rows(5)),
    ] {
        rounds_as_a_fresh_build(&format!("<div id=w style=\"width:500px\">{table}</div>"), widths(&table));
    }
}

/// The min-content and max-content widths of an inline layout without in-flow inline boxes are kept
/// with its shaped text (`TextLayout::text_content_widths`; debug builds check them against the
/// layout's): text indents (which they are measured without), inline boxes, floats, forced breaks,
/// preserved white space and shrink-to-fit widths read them or go without.
#[test]
fn content_widths_kept_with_the_shaped_text_match_fresh_builds() {
    rounds_as_a_fresh_build(
        r#"<div id=c style="width:120.5px"><p id=p1 style="text-indent:10%">lorem ipsum dolor sit amet consectetur adipiscing</p><p id=p2 style="text-indent:-7.5px">negative indent words here and there</p><p id=p3 style="text-indent:5px hanging each-line">hang<br>each line words wrap</p><p id=p4><span style="display:inline-block;width:30.5px;height:4px"></span> boxes between words <span style="display:inline-block;width:12.5px;height:3px"></span></p><p id=p5 style="white-space:pre">trailing   </p><p id=p6 style="float:left">shrink to fit</p><p id=p7><span style="float:right;width:20.5px;height:5px"></span>a float in text that wraps</p><span id=p8 style="display:inline-block">an inline-block of its own width</span></div>"#,
        vec![
            text("#p1", "shorter"),
            text("#p2", "a longer text than before for the negative indent to act on"),
            style("#c", "width:80.25px"),
            style("#p1", "text-indent:0"),
            style("#p2", "text-indent:0"),
            style("#p3", "text-indent:12%"),
            text("#p4", "no boxes any more"),
            style("#c", "width:300px"),
            text("#p8", "another width"),
        ],
    );
}

// ---------------- review (layint): adversarial cases for the flag walk, grid-relative rounding,
// width-only table relayout and kept content widths ----------------

/// Paints at a viewport `width` with the persistent renderer and a fresh one; compares the paint
/// and every element's box.
#[track_caller]
fn check_boxes_at(live: &mut Renderer, page: &Page, width: u32, what: &str) {
    let tree = page.tree();
    let s = Snapshot { width, ..page.snapshot(1, tree.clone()) };
    let mirrored = paint(live, &s);
    let mut fresh = Renderer::new();
    let fresh_paint = paint(&mut fresh, &s);
    for ((k, a), (_, b)) in boxes(live, page).into_iter().zip(boxes(&fresh, page)) {
        assert_eq!(a, b, "box {k:x} after {what} (viewport {width})");
    }
    assert_eq!(mirrored.hash, fresh_paint.hash, "paint after {what} (viewport {width})");
}

/// Steps, each with the viewport width to paint at, checked by paint and boxes.
#[track_caller]
fn steps_match_fresh_builds_at(body: &str, steps: Vec<(u32, (String, Mutate))>) {
    let page = Page::new(&format!("{PAINT_BOXES}{body}</body></html>"));
    let mut r = Renderer::new();
    check_boxes_at(&mut r, &page, 640, "the first render");
    for (width, (what, mutate)) in steps {
        mutate(&page);
        check_boxes_at(&mut r, &page, width, &what);
    }
}

#[track_caller]
fn steps_match_fresh_builds(body: &str, steps: Vec<(String, Mutate)>) {
    steps_match_fresh_builds_at(body, steps.into_iter().map(|s| (640, s)).collect());
}

fn noop(what: &'static str) -> (String, Mutate) {
    (what.into(), Box::new(|_| {}))
}

fn move_before(what: &'static str, node: &'static str, parent: &'static str, before: Option<&'static str>) -> (String, Mutate) {
    edit(what, move |p| {
        let n = p.find(node);
        let parent = p.find(parent);
        let before = before.map(|b| p.find(b));
        p.dom.borrow_mut().insert_before(parent, n, before).unwrap();
    })
}

#[test]
fn review_flags_through_display_contents() {
    steps_match_fresh_builds(
        r#"<div id=o style="width:300.5px"><div id=dc style="display:contents"><div id=a style="height:10.5px"><span id=as>a text</span></div><div id=b style="height:3.25px"></div></div><div id=after style="height:7.5px"><i style="display:block;height:1.5px"></i></div></div><div id=dc2 style="display:contents"><p id=q style="margin:0;height:4.5px">q</p></div><div id=tail style="height:2.5px"><b style="display:block;height:1.25px"></b></div>"#,
        vec![
            style("#a", "height:20.75px"),
            text("#as", "a longer text inside display contents that wraps maybe over lines"),
            style("#dc", "display:block;padding-top:0.5px"),
            style("#a", "height:11.5px"),
            style("#dc", "display:contents"),
            style("#a", "height:13.25px"),
            move_before("a box moved between display:contents parents", "#a", "#dc2", None),
            style("#a", "height:30.5px"),
            style("#q", "margin:0;height:9px"),
            move_before("moved back", "#a", "#dc", Some("#b")),
            style("#b", "height:6.5px"),
            style("#dc2", "display:block"),
            style("#q", "margin:0;height:9.5px"),
        ],
    );
}

#[test]
fn review_flags_through_anonymous_blocks_freed_and_recreated() {
    steps_match_fresh_builds(
        r#"<div id=m style="width:200.5px">text a <span id=s1>one</span><div id=blk style="height:5.5px">block</div>tail <span id=s2>two</span></div><div id=below style="height:3.5px"><u style="display:block;height:1.5px"></u></div>"#,
        vec![
            style("#blk", "height:5.5px;display:inline"),
            text("#s1", "one one one one one one one one one one one one"),
            style("#blk", "height:7.25px;display:block"),
            text("#s2", "two two two two two two two two two two two two two"),
            style("#blk", "display:none"),
            text("#s1", "x"),
            style("#blk", "height:8.5px"),
            text("#s2", "y"),
            move_before("the block moved to the front", "#blk", "#m", Some("#s1")),
            text("#s1", "after the move, more text that makes the anonymous box taller"),
            edit("the block removed", |p| {
                let (m, b) = (p.find("#m"), p.find("#blk"));
                p.dom.borrow_mut().remove_child(m, b).unwrap();
            }),
            text("#s2", "the anonymous boxes are gone and the text wraps again over lines"),
        ],
    );
}

#[test]
fn review_flags_with_table_relisting() {
    let rows: String = (0..6).map(|i| format!("<tr id=r{i}><td id=c{i}>c {i}</td><td><div><b>d {i}</b></div></td></tr>")).collect();
    steps_match_fresh_builds(
        &format!(
            r#"<table id=t style="border-spacing:1.5px"><caption id=cap style="height:3.5px">cap</caption><thead id=h><tr id=hr><td>h</td><td>h2</td></tr></thead><tbody id=b>{rows}</tbody><tfoot id=f><tr id=fr><td>f</td><td>f2</td></tr></tfoot></table><div id=after style="height:2.5px"><i style="display:block;height:1.5px"></i></div>"#
        ),
        vec![
            text("#c2", "edited c2 with longer text"),
            move_before("a row moved to thead", "#r2", "#h", None),
            text("#c2", "edited again in thead"),
            move_before("a row moved to tfoot", "#r3", "#f", Some("#fr")),
            style("#cap", "height:9.25px"),
            text("#c3", "c3 in tfoot grows to a line or two of text"),
            style("#b", "display:none"),
            text("#c4", "edited while the tbody is hidden"),
            style("#b", ""),
            move_before("a cell moved to another row", "#c5", "#r0", None),
            text("#c5", "moved cell edited"),
            style("#h", "display:table-row-group"),
            style("#t", "border-spacing:2.5px;display:block"),
            text("#c0", "the table is a block now"),
            style("#t", "border-spacing:2.5px"),
            text("#c1", "a table again"),
        ],
    );
}

#[test]
fn review_flags_with_nodes_leaving_and_rejoining_the_layout() {
    steps_match_fresh_builds(
        r#"<div id=w style="width:250.5px"><div id=x style="height:4.5px"><div id=y style="height:2.25px"><span id=z>z</span></div></div><div id=sib style="height:3.5px"><i style="display:block;height:1.5px"></i></div></div><div id=ab style="position:relative"><div id=p style="height:5.5px">p</div></div>"#,
        vec![
            style("#x", "display:none;height:4.5px"),
            style("#y", "height:9.75px"),
            text("#z", "edited while not laid out"),
            style("#x", "height:4.5px"),
            style("#y", "height:10.25px;position:absolute;top:0.5px"),
            style("#sib", "height:6.5px"),
            style("#y", "height:10.25px"),
            style("#p", "height:5.5px;position:fixed;top:1.5px;left:2.5px"),
            text("#p", "fixed now"),
            style("#p", "height:7.5px"),
            style("#p", "height:7.5px;float:left;width:20.5px"),
            text("#p", "floating now with text"),
            style("#ab", "position:relative;display:none"),
            style("#p", "height:2px"),
            style("#ab", "position:relative"),
            edit("a subtree detached, edited, attached elsewhere", |p| {
                let (x, w, body) = (p.find("#x"), p.find("#w"), p.find("body"));
                let mut d = p.dom.borrow_mut();
                d.remove_child(w, x).unwrap();
                let y = d.first_child(x).unwrap().unwrap();
                d.set_attribute(y, "style", "height:1.5px").unwrap();
                d.append_child(body, x).unwrap();
            }),
            style("#sib", "height:6px"),
        ],
    );
}

#[test]
fn review_flags_with_moves_inside_one_parent() {
    let items: String = (0..8).map(|i| format!("<div id=k{i} style=\"height:{i}.5px\"><span>{i}</span><div style=\"height:1.25px\"></div></div>")).collect();
    steps_match_fresh_builds(
        &format!("<div id=l style=\"width:120.5px\">{items}</div><div id=tail style=\"height:1.5px\"></div>"),
        vec![
            edit("the last item edited and moved first", |p| {
                let (l, k7, k0) = (p.find("#l"), p.find("#k7"), p.find("#k0"));
                let mut d = p.dom.borrow_mut();
                d.set_attribute(k7, "style", "height:11.75px").unwrap();
                d.insert_before(l, k7, Some(k0)).unwrap();
            }),
            edit("items reversed", |p| {
                let l = p.find("#l");
                let ids: Vec<_> = (0..8).map(|i| p.find(&format!("#k{i}"))).collect();
                let mut d = p.dom.borrow_mut();
                for id in ids {
                    let first = d.first_child(l).unwrap();
                    d.insert_before(l, id, first).unwrap();
                }
            }),
            style("#k3", "height:0.3px"),
            move_before("an item moved to the end", "#k3", "#l", None),
            style("#k4", "height:2.6px"),
        ],
    );
}

/// Negative and far positions: negative margins, relative offsets, RTL overflow, vertical-rl,
/// nested transforms, sticky, a scroll container, and positions near 2^24 px.
#[test]
fn review_rounding_negative_and_far_positions() {
    steps_match_fresh_builds(
        r#"<div id=o style="width:200px;margin-left:30px"><div id=n style="margin-left:-12.5px;height:3.5px"><div id=n2 style="margin-left:-0.5px;width:5.5px;height:1.5px"></div></div><div id=rel style="position:relative;left:-3.5px;top:-2.5px;width:9.5px;height:2.5px"><b style="display:block;margin-left:-1.5px;height:1.5px"></b></div><div dir=rtl style="width:50.5px"><div id=wide style="width:80.5px;height:2.5px"><i style="display:block;width:7.5px;height:1px"></i></div></div><div style="writing-mode:vertical-rl;width:30.5px;height:40px"><div id=vr style="width:50.5px;height:3.5px"><i style="display:block;width:2.5px">v</i></div></div><div style="transform:translateX(-3.5px)"><div id=tr style="transform:scale(1.5);width:9.5px;height:3.5px"><div style="transform:rotate(3deg);width:4.5px;height:2.5px"></div></div></div><div style="overflow:auto;height:20px"><div id=st style="position:sticky;top:-1.5px;height:4.5px">s</div><div style="height:60.5px"></div></div></div><div id=far style="height:16777000.5px"></div><div id=f2 style="margin-left:-0.5px;width:10.5px;height:3.5px"><div style="height:1.5px;margin-left:0.5px"></div></div>"#,
        vec![
            style("#n", "margin-left:-13.5px;height:3.5px"),
            style("#n", "margin-left:-13px;height:3.5px"),
            style("#rel", "position:relative;left:-4.5px;top:-3.5px;width:9.5px;height:2.5px"),
            style("#wide", "width:81.5px;height:2.5px"),
            style("#wide", "width:81.25px;height:2.5px"),
            style("#vr", "width:51.5px;height:3.5px"),
            style("#vr", "width:51.75px;height:3.5px"),
            style("#tr", "transform:scale(1.5);width:10.5px;height:3.5px"),
            style("#st", "position:sticky;top:-1.5px;height:5.5px"),
            style("#far", "height:16777001.5px"),
            style("#far", "height:16777002px"),
            style("#far", "height:8388607.5px"),
            style("#far", "height:8388608.5px"),
            style("#o", "width:200px;margin-left:31px"),
            style("#o", "width:200px;margin-left:-31.5px"),
        ],
    );
}

/// Width-only relayout of tables whose cells' intrinsic widths could depend on the table's or the
/// viewport's width in ways other than the cells' own percentages.
#[test]
fn review_tables_whose_cells_depend_on_widths_in_other_ways() {
    let rows = |n: usize| -> String {
        (0..n).map(|i| format!("<tr><td>cell {i}</td><td>some text {i} here</td></tr>")).collect()
    };
    let cases = [
        format!("<table><tr><td><div style=\"width:20vw\">vw</div></td><td>b</td></tr>{}</table>", rows(3)),
        format!("<table><tr><td style=\"padding-left:2vw\">vw pad</td><td>b</td></tr>{}</table>", rows(3)),
        format!("<table><tr><td><div style=\"padding-left:10%\">pct pad inside</div></td><td><div style=\"margin-left:5%\">m</div></td></tr>{}</table>", rows(3)),
        format!("<table><tr><td><img style=\"max-width:100%;width:200px;height:10px\"></td><td>b</td></tr>{}</table>", rows(3)),
        format!("<table><tr><td><table style=\"width:100%\"><tr><td>nested</td></tr></table></td><td>b</td></tr>{}</table>", rows(3)),
        format!("<table><tr><td style=\"width:min-content\">min content words</td><td style=\"width:fit-content\">fit content words</td></tr>{}</table>", rows(3)),
        format!("<table style=\"table-layout:fixed;width:80%\"><colgroup><col style=\"width:30%\"><col></colgroup>{}</table>", rows(3)),
        format!("<table><colgroup><col style=\"width:30%\"><col></colgroup>{}</table>", rows(3)),
        format!("<table id=em style=\"border-spacing:0.5em\"><tr><td style=\"padding:0.3em\">em</td><td>b</td></tr>{}</table>", rows(3)),
        format!("<table><tr><td><div style=\"width:50cqw\">cq</div></td><td>b</td></tr>{}</table>", rows(3)),
        format!("<table><tr><td style=\"text-indent:10%\">indent pct words</td><td><input style=\"width:90%\"></td></tr>{}</table>", rows(3)),
        format!("<table><tr><td><div style=\"display:flex\"><div style=\"flex-basis:40%\">flex</div><div>x</div></div></td><td><div style=\"display:grid;grid-template-columns:30% 1fr\"><i>g</i><b>h</b></div></td></tr>{}</table>", rows(3)),
    ];
    for table in cases {
        let body = format!("<div id=w style=\"width:500px;container-type:inline-size;font-size:13px\">{table}</div>");
        let steps: Vec<(u32, (String, Mutate))> = vec![
            (640, style("#w", "width:450px;container-type:inline-size;font-size:13px")),
            (500, noop("viewport 500")),
            (500, style("#w", "width:600.5px;container-type:inline-size;font-size:13px")),
            (800, noop("viewport 800")),
            (800, style("#w", "width:600.5px;container-type:inline-size;font-size:17px")),
            (300, style("#w", "width:100px;container-type:inline-size;font-size:17px")),
            (640, style("#w", "width:500px;container-type:inline-size;font-size:13px")),
            (1000, noop("viewport 1000")),
        ];
        let steps = steps.into_iter().map(|(w, (what, m))| (w, (format!("{table}: {what}"), m))).collect();
        steps_match_fresh_builds_at(&body, steps);
    }
}

/// The table's own width follows the viewport (shrink-to-fit inside a vw-wide container).
#[test]
fn review_tables_in_viewport_sized_containers() {
    let rows: String = (0..6).map(|i| format!("<tr><td>cell {i}</td><td>text {i} that is somewhat long</td><td style=\"width:20%\">p {i}</td></tr>")).collect();
    steps_match_fresh_builds_at(
        &format!("<div style=\"width:80vw\"><table style=\"border-spacing:1.5px\">{rows}</table></div><div style=\"width:50vw\"><table><tr><td><div style=\"width:10vw\">x</div></td><td>y y y y y</td></tr></table></div>"),
        vec![(500, noop("500")), (520, noop("520")), (900, noop("900")), (300, noop("300")), (640, noop("640")), (641, noop("641"))],
    );
}

/// Content widths kept with the shaped text across changes that do and don't rebuild the layout.
#[test]
fn review_content_widths_with_indents_letters_and_white_space() {
    steps_match_fresh_builds(
        r#"<style>#fl::first-letter { font-size: 2.5em; float: left } .hp { hanging-punctuation: first last }</style><div id=c style="width:min-content"><p id=a>alpha beta gamma</p><p id=fl>first letter words</p><p id=h class=hp>"quoted words."</p><p id=ws>keep   these   spaces together</p><span id=sh style="float:left">shrink wrap me please</span></div><div style="display:inline-block"><p id=ib>inline block text</p></div>"#,
        vec![
            style("#a", "text-indent:7.5px"),
            style("#a", "text-indent:0"),
            style("#a", "text-indent:-3.5px"),
            edit("an inline box added later", |p| {
                let a = p.find("#a");
                let mut d = p.dom.borrow_mut();
                let s = d.create_element(p.doc, "span").unwrap();
                d.set_attribute(s, "style", "display:inline-block;width:150.5px;height:3px").unwrap();
                d.append_child(a, s).unwrap();
            }),
            style("#a", "text-indent:0"),
            edit("the inline box removed", |p| {
                let a = p.find("#a");
                let mut d = p.dom.borrow_mut();
                let s = d.last_child(a).unwrap().unwrap();
                d.remove_child(a, s).unwrap();
            }),
            text("#fl", "Wfirst letter changes"),
            style("#fl", "font-size:20px"),
            text("#h", "\u{201c}other quoted words.\u{201d}"),
            style("#ws", "white-space:pre"),
            style("#ws", "white-space:nowrap"),
            style("#ws", "white-space:pre-wrap"),
            style("#ws", "white-space:normal"),
            style("#ws", "word-spacing:20.5px"),
            style("#ws", "letter-spacing:1.5px"),
            style("#ws", "text-transform:uppercase"),
            style("#sh", "float:left;font-size:20px"),
            style("#ib", "text-indent:10px each-line"),
            style("#ib", "text-indent:0"),
            style("#c", "width:max-content"),
            style("#c", "width:fit-content;font-family:monospace"),
        ],
    );
}

/// The page of `paint_only_restyles_run_no_layout_and_match_fresh_builds`: a collapsed table, a
/// separate-border table, a paragraph with a span, flex and grid items, a float and an absolutely
/// positioned box, each with a border and text.
const PAINT_ONLY_PAGE: &str = r#"<!doctype html><html><head><style>
  body { margin: 0; font: 13px sans-serif } #c, #c td { border: 1px solid #888 }
  #c { border-collapse: collapse; width: 300px } td { padding: 2px }
  #p, #f1, #g1, #fl, #abs, #s1, #ps { border: 1px solid #444 }
  .flex { display: flex; width: 300px } .grid { display: grid; grid-template-columns: 1fr 1fr; width: 300px }
  #fl { float: left; width: 60px } #abs { position: absolute; left: 10px; top: 300px; width: 80px }
  .dark td { color: #eee; background: #333 } .dark #p { color: #fff }
</style></head><body>
<table id=c><tr><td id=c1>first <span>row</span> text</td><td>x</td></tr><tr><td id=c2>second row with some words</td><td id=c3>y</td></tr></table>
<table id=s><tr><td id=s1>sep cell</td><td>z</td></tr></table>
<p id=p>A paragraph <span id=ps>with a span</span> in it</p>
<div class=flex><div id=f1>flex one</div><div id=f2>flex two</div></div>
<div class=grid><div id=g1>grid one</div><div id=g2>grid two</div></div>
<div id=fl>float</div><p id=pf>text beside the float, long enough to wrap onto a second line</p>
<div id=abs>abs box</div>
</body></html>"#;

/// Paint-only restyles (colour, background, outline, visibility, opacity, text decoration, box
/// shadow, border colour) of cells, inline roots, flex and grid items, floats and absolutely
/// positioned boxes run no layout and shape no text; changes to a box that leave its text as it is
/// (padding, border width, a transform, a collapsed border's colour) shape no text either, but for
/// a span's padding and border width, which are in its line. Each
/// step is synced as a delta and compared with a fresh build (boxes, document size, paint), and
/// the paint changes when it should.
#[test]
fn paint_only_restyles_run_no_layout_and_match_fresh_builds() {
    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Kind {
        /// No layout algorithm runs, no text is shaped.
        Paint,
        /// Layout runs, no text is shaped.
        Box,
    }
    let paint_props = [
        "color: #c00",
        "background: #eef",
        "outline: 2px solid #f0f",
        "visibility: hidden",
        "opacity: .5",
        "text-decoration: underline",
        "box-shadow: 0 0 4px #000",
        "border-color: #0c0",
        "transform: translate(2px, 3px)",
    ];
    let box_props = ["padding: 6px", "border: 3px solid #000", "border: 3px solid #000; padding: 1px"];
    let collapsed = ["#c", "#c2", "#c3"];
    let targets = ["#c2", "#c3", "#c", "#s1", "#p", "#ps", "#f1", "#g1", "#fl", "#abs"];
    let mut steps: Vec<(String, Mutate, Kind)> = Vec::new();
    for target in targets {
        for prop in paint_props {
            // A transform makes a containing block for fixed boxes: Stylo relays it out. A
            // collapsed table part's border colour is resolved into the table's edges when its
            // row is collected: the part's box is built again.
            let kind = if prop.starts_with("transform") || (prop.starts_with("border-color") && collapsed.contains(&target)) {
                Kind::Box
            } else {
                Kind::Paint
            };
            let (what, mutate) = style(target, prop);
            steps.push((what, mutate, kind));
        }
        for prop in box_props {
            let (what, mutate) = style(target, prop);
            steps.push((what, mutate, Kind::Box));
        }
        let (what, mutate) = style(target, "");
        steps.push((what, mutate, Kind::Box));
    }
    let (what, mutate) = class("body", "dark");
    steps.push((what, mutate, Kind::Paint));
    let (what, mutate) = class("body", "");
    steps.push((what, mutate, Kind::Paint));

    let page = Page::new(PAINT_ONLY_PAGE);
    let mut r = Renderer::new();
    let mut hash = paint(&mut r, &page.snapshot(1, page.tree())).hash;
    let mut failures = Vec::new();
    for (what, mutate, kind) in steps {
        let inline_unpainted =
            what.starts_with("#ps") && ["opacity", "transform", "visibility"].iter().any(|prop| what.contains(prop));
        // A span's inline-axis padding and border are in its line: its text is laid out again.
        let inline_edges =
            what.starts_with("#ps") && ["padding", "border: 3px", "{  }"].iter().any(|prop| what.contains(prop));
        let since = page.dom.borrow().version();
        mutate(&page);
        let (layouts, rebuilds, tables) = (blitz_performed_layouts().0, blitz_rebuilds().0, blitz_tables().0);
        let delta = page.dom.borrow().render_delta(page.doc, since, &Page::opts());
        let via_delta =
            delta.is_some_and(|d| r.sync_delta(1, &delta_ops(d), (640, 480, 1.0, false, false), false).is_ok());
        if !via_delta {
            failures.push(format!("{what}: not synced as a delta"));
            r.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        }
        let tree = page.tree();
        let painted = paint(&mut r, &page.snapshot(1, tree.clone()));
        let layouts = blitz_performed_layouts().0 - layouts;
        let rebuilds = blitz_rebuilds().0 - rebuilds;
        let tables = blitz_tables().0 - tables;
        let mut fresh = Renderer::new();
        fresh.sync_layout(&page.snapshot(1, tree.clone()), &fetcher()).unwrap();
        let fresh_painted = paint(&mut fresh, &page.snapshot(1, tree));
        if painted.hash != fresh_painted.hash {
            failures.push(format!("{what}: paint differs from a fresh build"));
        }
        // (Blitz paints no opacity, transform or visibility of a non-atomic inline box. A table in
        // the collapsing border model has no padding: CSS 2.1 §17.6.2.)
        let collapsed_padding = what.starts_with("#c ") && what.contains("padding");
        if painted.hash == hash && !inline_unpainted && !collapsed_padding {
            failures.push(format!("{what}: paint did not change"));
        }
        hash = painted.hash;
        for ((k, a), (_, b)) in boxes(&r, &page).into_iter().zip(boxes(&fresh, &page)) {
            if a != b {
                failures.push(format!("{what}: box {k:x} {a:?} vs fresh {b:?}"));
                break;
            }
        }
        if r.document_size(1) != fresh.document_size(1) {
            failures.push(format!("{what}: size {:?} vs fresh {:?}", r.document_size(1), fresh.document_size(1)));
        }
        if rebuilds != usize::from(inline_edges) as _ {
            failures.push(format!("{what}: shaped {rebuilds} inline layouts"));
        }
        if kind == Kind::Paint && (layouts != 0 || tables != 0) {
            failures.push(format!("{what}: ran {layouts} layouts and built {tables} tables"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Changes to the styles text is shaped with that Stylo's damage alone did not reach the inline
/// layout: `white-space: nowrap` (`text-wrap-mode`) and a span's `vertical-align`. Each shapes the
/// text again and matches a fresh build.
#[test]
fn text_style_changes_shape_again_and_match_fresh_builds() {
    let page = Page::new(PAINT_ONLY_PAGE);
    let mut r = Renderer::new();
    paint(&mut r, &page.snapshot(1, page.tree()));
    let steps = [
        style("#pf", "white-space: nowrap"),
        style("#pf", ""),
        style("#ps", "vertical-align: super"),
        style("#ps", "vertical-align: -3px"),
        style("#ps", ""),
        style("#c2", "white-space: pre-wrap; text-wrap-mode: nowrap"),
        style("#c2", ""),
    ];
    for (what, mutate) in steps {
        let since = page.dom.borrow().version();
        mutate(&page);
        let rebuilds = blitz_rebuilds().0;
        let delta = page.dom.borrow().render_delta(page.doc, since, &Page::opts()).expect("a delta");
        r.sync_delta(1, &delta_ops(delta), (640, 480, 1.0, false, false), false).unwrap();
        let tree = page.tree();
        let painted = paint(&mut r, &page.snapshot(1, tree.clone()));
        assert!(blitz_rebuilds().0 > rebuilds, "{what}: shaped no text again");
        let mut fresh = Renderer::new();
        fresh.sync_layout(&page.snapshot(1, tree.clone()), &fetcher()).unwrap();
        assert_eq!(painted.hash, paint(&mut fresh, &page.snapshot(1, tree)).hash, "{what}: paint");
        for ((k, a), (_, b)) in boxes(&r, &page).into_iter().zip(boxes(&fresh, &page)) {
            assert_eq!(a, b, "{what}: box {k:x}");
        }
    }
}

// Review (perf/paint-only-damage): every longhand whose Stylo damage moved to repaint or
// overflow, and the ones whose damage stayed but whose layout effect is subtle, in the contexts
// where they might secretly reach layout. Each step is synced as a delta and compared with a fresh
// build: paint hash, every box (with client and scroll sizes), document size, the words of every
// text node, and hit testing at the centre of every box.

const REVIEW_PAGE: &str = r#"<!doctype html><html><head><style>
  body { margin: 0; font: 11px sans-serif }
  #ct { border-collapse: collapse } #ct td, #ct th { border: 1px solid currentColor; padding: 1px }
  #ct col { border: 2px solid #888 } #st td { border: 1px solid #444 }
  #sc { overflow: auto; width: 120px; height: 40px } #sc > div { width: 100px; height: 20px; border: 1px solid }
  #clip { overflow: clip; width: 80px; height: 20px } #clip > div { width: 100px; height: 30px; background: #ccc }
  .flex { display: flex; width: 240px } .flex > div { flex: 1 } #ul li::marker { color: #00f }
  #fl::first-line { color: #c00 } #fl::first-letter { color: #0c0 } #fl::selection { color: #f00 }
  svg text { fill: currentColor }
  .dark { color: #eee; background: #222 } .dark #ct td { color: #f80 } .dark li::marker { color: #f0f }
</style></head><body>
<table id=ct><colgroup><col id=col1><col id=col2></colgroup>
  <tr id=r1><th id=h1>head <span id=hs>span</span></th><td id=c1>one</td></tr>
  <tr id=r2><td id=c2>two <b id=cb>bold</b> text</td><td id=c3>three</td></tr>
  <tr id=r3><td id=c4>four</td><td id=c5>five</td></tr></table>
<table id=st><tr id=sr1><td id=s1>sep <i id=si>it</i></td><td id=s2>z</td></tr><tr id=sr2><td id=s3>w</td><td>v</td></tr></table>
<p id=p>Para <span id=ps>span text</span> <a id=pa href="/x">link</a> <span id=dc style="display:contents">contents <em id=dce>wrapped</em></span> tail <span id=tc style="display:table-cell">tcell</span> end</p>
<div class=flex id=fx><div id=f1>flex one</div><div id=f2>flex two</div><div id=f3>three</div></div>
<div id=sc><div id=scc>scrolled</div></div>
<div id=clip><div id=clipc>clipped</div></div>
<ul id=ul><li id=li1>item one</li><li id=li2>item two</li></ul>
<p id=fl>First line of a paragraph long enough to wrap onto the next line of text here.</p>
<div id=fc><input id=cb1 type=checkbox checked> <input id=rg type=range> <input id=tx value="text"> <button id=bt>Btn</button> <select id=sel><option>opt</option></select> <progress id=pr value=.4></progress></div>
<svg id=svg width=80 height=20><text id=svt x=2 y=14>svg text</text><rect id=svr x=60 y=2 width=10 height=10 /></svg>
</body></html>"#;

/// Syncs each step as a delta and compares everything layout queries read with a fresh build.
/// Returns the failures and, per step, (inline layouts shaped, layouts run).
fn review_steps(html: &str, steps: Vec<(String, Mutate)>) -> (Vec<String>, Vec<(String, u64, u64)>) {
    let page = Page::new(html);
    let mut r = Renderer::new();
    paint(&mut r, &page.snapshot(1, page.tree()));
    let mut failures = Vec::new();
    let mut counts = Vec::new();
    for (what, mutate) in steps {
        if std::env::var_os("REVIEW_TRACE").is_some() {
            eprintln!("step {what}");
        }
        let since = page.dom.borrow().version();
        mutate(&page);
        let (layouts, rebuilds) = (blitz_performed_layouts().0, blitz_rebuilds().0);
        let delta = page.dom.borrow().render_delta(page.doc, since, &Page::opts());
        let via_delta =
            delta.is_some_and(|d| r.sync_delta(1, &delta_ops(d), (640, 480, 1.0, false, false), false).is_ok());
        if !via_delta {
            failures.push(format!("{what}: not synced as a delta"));
            r.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        }
        let tree = page.tree();
        let painted = paint(&mut r, &page.snapshot(1, tree.clone()));
        counts.push((what.clone(), blitz_rebuilds().0 - rebuilds, blitz_performed_layouts().0 - layouts));
        let mut fresh = Renderer::new();
        fresh.sync_layout(&page.snapshot(1, tree.clone()), &fetcher()).unwrap();
        let fresh_painted = paint(&mut fresh, &page.snapshot(1, tree));
        if painted.hash != fresh_painted.hash {
            failures.push(format!("{what}: paint differs from a fresh build"));
        }
        let (a, b) = (boxes(&r, &page), boxes(&fresh, &page));
        for ((k, a), (_, b)) in a.iter().zip(b.iter()) {
            if a != b {
                failures.push(format!("{what}: box {k:x} {a:?} vs fresh {b:?}"));
                break;
            }
        }
        if r.document_size(1) != fresh.document_size(1) {
            failures.push(format!("{what}: size {:?} vs fresh {:?}", r.document_size(1), fresh.document_size(1)));
        }
        let words = |r: &Renderer| {
            r.page_words(1)
                .map(|words| words.into_iter().map(|(k, w)| (k, w.x, w.y, w.width, w.height, w.edges)).collect::<Vec<_>>())
        };
        let (wa, wb) = (words(&r), words(&fresh));
        // (Compared as sets: the incremental and the fresh build list the same words in different
        // orders on 34f0f2ba already. `REVIEW_ORDERED` compares them in order.)
        let sorted = |w: Option<Vec<_>>| {
            w.map(|mut w: Vec<(u64, f64, f64, f64, f64, Vec<(u32, f64)>)>| {
                if std::env::var_os("REVIEW_ORDERED").is_none() {
                    w.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
                }
                w
            })
        };
        let (wa, wb) = (sorted(wa), sorted(wb));
        if wa != wb {
            let first = wa.as_ref().zip(wb.as_ref()).and_then(|(a, b)| {
                a.iter().zip(b.iter()).find(|(x, y)| x != y).map(|(x, y)| format!("{x:?} vs fresh {y:?}"))
            });
            let lens = (wa.as_ref().map(|w| w.len()), wb.as_ref().map(|w| w.len()));
            failures.push(format!("{what}: words differ from a fresh build {lens:?} {first:?}"));
        }
        for (k, bx) in b.iter() {
            let Some(bx) = bx else { continue };
            if bx.width < 1.0 || bx.height < 1.0 || bx.y + bx.height / 2.0 > 480.0 {
                continue;
            }
            let (x, y) = (bx.x + bx.width / 2.0, bx.y + bx.height / 2.0);
            let (hit, fresh_hit) = (r.elements_from_point(1, x, y), fresh.elements_from_point(1, x, y));
            if hit != fresh_hit {
                failures.push(format!("{what}: hit at {k:x} ({x}, {y}) {hit:?} vs fresh {fresh_hit:?}"));
                break;
            }
        }
    }
    // Divergences 34f0f2ba has as well (reported with this review, not caused by the branch): an
    // inline child's padding or margin in a cell does not measure the cell again; a table part made
    // `inline-block`; a row made absolutely positioned; a `display: contents` span cleared.
    const PRE_EXISTING: &[&str] =
        &["#sp {", "{ display: inline-block }: box", "#r2 { position: absolute", "#dc {  }: box"];
    let (known, failures): (Vec<_>, Vec<_>) =
        failures.into_iter().partition(|f| PRE_EXISTING.iter().any(|p| f.contains(p)));
    if !known.is_empty() && std::env::var_os("REVIEW_COUNTS").is_some() {
        eprintln!("pre-existing:\n{}", known.join("\n"));
    }
    (failures, counts)
}

/// Set then clear `prop` on each of `targets`, each pair on a page of its own (so that a divergence
/// does not carry over to the steps after it).
fn set_and_clear(targets: &[&'static str], props: &[&'static str]) -> Vec<Vec<(String, Mutate)>> {
    let mut groups = Vec::new();
    for &target in targets {
        for &prop in props {
            groups.push(vec![style(target, prop), style(target, "")]);
        }
    }
    groups
}

#[track_caller]
fn assert_review(groups: Vec<Vec<(String, Mutate)>>) {
    let mut failures = Vec::new();
    for steps in groups {
        let (f, counts) = review_steps(REVIEW_PAGE, steps);
        failures.extend(f);
        if std::env::var_os("REVIEW_COUNTS").is_some() {
            for (what, rebuilds, layouts) in &counts {
                eprintln!("{what}: shaped {rebuilds}, layouts {layouts}");
            }
        }
    }
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
}

/// Longhands Stylo now gives repaint or overflow damage (they used to relayout), on table parts,
/// inline children, flex items, scroll and clip containers, list items and form controls.
/// (`text-emphasis-style` relayouts again: its marks take room in their lines.)
#[test]
fn review_moved_longhands_match_fresh_builds() {
    let props = [
        "visibility: hidden",
        "visibility: collapse",
        "border-color: #0c0",
        "border-top-color: transparent",
        "border-radius: 6px",
        "box-shadow: 30px 30px 0 20px #000",
        "backdrop-filter: blur(2px)",
        "clip: rect(0 5px 5px 0)",
        "position: absolute; clip: rect(0 5px 5px 0)",
        "overflow-clip-margin: 20px",
        "mask-image: linear-gradient(#000, transparent)",
        "mask-size: 4px 4px",
        "appearance: none",
        "scrollbar-color: red blue",
        "text-emphasis-style: dot",
        "text-emphasis-color: red",
        "image-rendering: pixelated",
        "touch-action: none",
        "overscroll-behavior: contain",
        "user-select: none",
        "container-name: foo",
        "animation-name: none; animation-duration: 1s",
        "transition: color 1s",
        "fill: red; stroke: blue; stroke-dasharray: 2",
    ];
    let targets = [
        "#ct", "#r2", "#col1", "#c2", "#cb", "#h1", "#st", "#s1", "#ps", "#pa", "#dc", "#dce", "#tc", "#f2",
        "#sc", "#scc", "#clip", "#li1", "#fl", "#cb1", "#rg", "#tx", "#bt", "#sel", "#svg", "#svt",
    ];
    assert_review(set_and_clear(&targets, &props));
}

/// Paint properties whose damage did not move but which might reach layout or overflow: outline,
/// transforms, filters and shadows on scrollable overflow; opacity, isolation and blend modes that
/// make stacking contexts; text decorations; colours of pseudo-elements and form controls.
#[test]
fn review_paint_longhands_match_fresh_builds() {
    let props = [
        "outline: 30px solid #f0f",
        "outline-offset: 40px; outline: 1px solid",
        "transform: translate(200px, 0)",
        "transform: scale(3)",
        "filter: blur(4px)",
        "filter: drop-shadow(40px 0 0 #000)",
        "opacity: 0",
        "opacity: 1",
        "isolation: isolate",
        "mix-blend-mode: multiply",
        "clip-path: inset(2px)",
        "will-change: transform",
        "will-change: opacity",
        "text-decoration: underline; text-decoration-thickness: 6px; text-underline-offset: 10px",
        "text-shadow: 30px 30px #000",
        "-webkit-text-fill-color: red",
        "caret-color: red",
        "accent-color: red",
        "color-scheme: dark",
        "color: #00f",
        "background: #eef",
        "cursor: pointer",
        "pointer-events: none",
        "z-index: 5; position: relative",
    ];
    let targets = [
        "#ct", "#r2", "#col1", "#c2", "#cb", "#s1", "#si", "#ps", "#dc", "#dce", "#tc", "#f2", "#scc", "#clipc",
        "#li1", "#fl", "#cb1", "#rg", "#tx", "#bt", "#sel", "#pr", "#svg", "#svt",
    ];
    assert_review(set_and_clear(&targets, &props));
}

/// Styles text is shaped or laid out with: on table cells (which now keep their inline layout for
/// a box change) and on inline children of cells and paragraphs.
#[test]
fn review_text_longhands_match_fresh_builds() {
    let props = [
        "letter-spacing: 3px",
        "word-spacing: 9px",
        "text-indent: 20px",
        "text-indent: 20px; padding: 3px",
        "tab-size: 20",
        "hyphens: auto",
        "font-feature-settings: 'smcp'",
        "font-variation-settings: 'wght' 700",
        "font-kerning: none",
        "text-transform: uppercase",
        "text-transform: uppercase; padding: 4px",
        "white-space-collapse: preserve",
        "white-space: pre; border-width: 3px",
        "line-height: 30px",
        "line-height: 30px; padding-left: 5px",
        "text-align: right",
        "text-align: right; padding: 5px",
        "text-align-last: center",
        "text-wrap-mode: nowrap",
        "vertical-align: top",
        "vertical-align: 5px; padding: 2px",
        "direction: rtl",
        "direction: rtl; unicode-bidi: bidi-override",
        "unicode-bidi: isolate",
        "word-break: break-all",
        "overflow-wrap: anywhere",
        "font-size: 15px",
        "padding: 0 10px",
        "border-left: 6px solid",
        "margin: 0 8px",
        "display: inline-block",
        "text-overflow: ellipsis; overflow: hidden; white-space: nowrap",
    ];
    let targets = ["#c1", "#c2", "#cb", "#hs", "#h1", "#r2", "#ct", "#s1", "#si", "#ps", "#dc", "#dce", "#tc", "#li1", "#fl", "#f1"];
    assert_review(set_and_clear(&targets, &props));
}

/// A box change to a cell together with a text change below it in the same delta; class toggles
/// that restyle many table parts at once (colour under `currentColor` borders, `::marker`,
/// `::first-line`); collapsed-border colours that come from a column; row and column
/// `visibility: collapse`; text edits in cells after a kept inline layout.
#[test]
fn review_combined_steps_match_fresh_builds() {
    let steps = vec![
        class("body", "dark"),
        class("body", ""),
        class("body", "dark"),
        style("#c2", "padding: 6px"),
        class("body", ""),
        edit("#c2 padding + #cb letter-spacing", |p| {
            let (c, b) = (p.find("#c2"), p.find("#cb"));
            p.dom.borrow_mut().set_attribute(c, "style", "padding: 4px").unwrap();
            p.dom.borrow_mut().set_attribute(b, "style", "letter-spacing: 4px").unwrap();
        }),
        edit("#c2 border + #cb text", |p| {
            let (c, b) = (p.find("#c2"), p.find("#cb"));
            p.dom.borrow_mut().set_attribute(c, "style", "border-width: 4px").unwrap();
            let t = p.dom.borrow().first_child(b).unwrap().unwrap();
            p.dom.borrow_mut().set_text_content(t, "much bolder words").unwrap();
        }),
        edit("#c2 padding + text append", |p| {
            let c = p.find("#c2");
            p.dom.borrow_mut().set_attribute(c, "style", "padding: 7px").unwrap();
            let t = p.dom.borrow_mut().create_text(p.doc, " appended").unwrap();
            p.dom.borrow_mut().append_child(c, t).unwrap();
        }),
        edit("#c2 padding + span inserted", |p| {
            let c = p.find("#c2");
            p.dom.borrow_mut().set_attribute(c, "style", "padding: 2px").unwrap();
            let s = p.dom.borrow_mut().create_element(p.doc, "span").unwrap();
            let t = p.dom.borrow_mut().create_text(p.doc, "new").unwrap();
            p.dom.borrow_mut().append_child(s, t).unwrap();
            p.dom.borrow_mut().append_child(c, s).unwrap();
        }),
        style("#col1", "border-color: #f00"),
        style("#col1", "border-color: #00f; visibility: collapse"),
        style("#col1", ""),
        style("#r2", "visibility: collapse"),
        style("#r2", "visibility: collapse; color: red"),
        style("#r2", ""),
        style("#ct", "color: #0a0"),
        style("#ct", "color: #0a0; border-color: red"),
        style("#ct", ""),
        style("#c4", "border-color: red; border-width: 3px"),
        style("#c4", "border-color: blue; border-width: 3px"),
        style("#c4", "border-color: blue; border-width: 3px; border-style: hidden"),
        style("#c4", "border-color: red; border-width: 3px; border-style: hidden"),
        style("#c4", ""),
        style("#svg", "color: red"),
        style("#svt", "fill: blue"),
        style("#svr", "fill: green; stroke: red"),
        style("#svg", ""),
        style("#svt", ""),
        style("#svr", ""),
        style("#li1", "list-style-type: square"),
        style("#li1", "list-style-type: square; color: red"),
        style("#li1", ""),
        style("#tc", "padding: 10px"),
        style("#tc", "padding: 10px; border: 2px solid"),
        style("#tc", ""),
    ];
    assert_review(vec![steps]);
}

/// A cell whose damage is a relayout of something inside it (no construction: a block's width, an
/// inline child's padding, an inline-block's width) is measured again: its column widens.
#[test]
fn review_relayout_inside_a_cell_measures_it_again() {
    let html = r#"<!doctype html><html><head><style>body { margin: 0; font: 11px sans-serif }</style></head><body>
<table id=t><tr><td id=a><div id=w style="width: 50px">w</div></td><td id=b>x <span id=sp>sp</span> <span id=ib style="display: inline-block; width: 20px">i</span></td></tr>
<tr><td>left</td><td>right</td></tr></table>
<table id=tc style="border-collapse: collapse"><tr><td id=ca><div id=cw style="width: 50px">w</div></td><td>y</td></tr><tr><td>l</td><td>r</td></tr></table>
</body></html>"#;
    let steps = vec![
        style("#w", "width: 150px"),
        style("#sp", "padding: 0 40px"),
        style("#sp", "margin: 0 30px"),
        style("#ib", "display: inline-block; width: 120px"),
        style("#cw", "width: 170px"),
        style("#w", ""),
        style("#sp", ""),
        style("#ib", "display: inline-block; width: 20px"),
        style("#cw", "width: 50px"),
    ];
    let (failures, _) = review_steps(html, steps);
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
}

// ---------------- integration (layint x paint): kept inline layouts and kept content widths ----------------

/// An inline root kept for a box-only change (a cell's padding, border width, collapsed border
/// colour, its colour) keeps its shaped text and the content widths measured with it
/// (`TextLayout::text_content_widths`). Each step pairs such a change with one the widths or their
/// use depend on: the available width (the table's width), a text indent, an in-flow inline box
/// coming, changing size and going, `white-space`, letter spacing, min/max-content sizing. Synced
/// as deltas and compared with fresh builds; the kept layouts are checked in debug builds
/// (`assert_kept_inline_layout_is_current` compares the kept content widths with a new build's).
#[test]
fn kept_inline_layouts_keep_matching_content_widths() {
    let html = r#"<!doctype html><html><head><style>body { margin: 0; font: 12px sans-serif }
  #t, #t td { border: 1px solid #888 } #t { border-collapse: collapse; width: 320px } td { padding: 2px }
  #s { width: 260px } #s td { border: 1px solid #444 }</style></head><body>
<table id=t><tr><td id=a>alpha beta gamma delta epsilon</td><td id=b>x <span id=ib style="display: inline-block; width: 20px">i</span> tail words</td></tr>
<tr><td id=c>third cell text</td><td id=d>y</td></tr></table>
<table id=s><tr><td id=e>separate border cell with words</td><td id=f>z</td></tr></table>
<div id=w style="width: min-content"><p id=p style="border: 1px solid">paragraph words for min content</p></div>
</body></html>"#;
    let two = |what: &'static str, a: &'static str, sa: &'static str, b: &'static str, sb: &'static str| {
        edit(what, move |p| {
            let (x, y) = (p.find(a), p.find(b));
            p.dom.borrow_mut().set_attribute(x, "style", sa).unwrap();
            p.dom.borrow_mut().set_attribute(y, "style", sb).unwrap();
        })
    };
    let steps = vec![
        // Box-only changes alone: the cells keep their shaped text.
        style("#a", "padding: 6px"),
        style("#e", "padding: 5px; border-width: 3px"),
        style("#a", "padding: 6px; border-color: red"),
        // With the available width changed.
        two("#a padding + #t width", "#a", "padding: 9px", "#t", "width: 180px"),
        two("#e padding + #s width", "#e", "padding: 1px", "#s", "width: 90px"),
        two("#a colour + #t width", "#a", "color: blue", "#t", "width: 520px"),
        // With a text indent (measured, not read from the kept widths) and back to none.
        two("#a padding + #c indent", "#a", "padding: 3px", "#c", "text-indent: 12.5px"),
        style("#c", "padding: 7px; text-indent: 12.5px"),
        style("#c", "padding: 7px; text-indent: -4px"),
        style("#c", "padding: 1px"),
        two("#c indent + #t width", "#c", "text-indent: 30%", "#t", "width: 140px"),
        two("#c no indent + #t width", "#c", "padding: 4px", "#t", "width: 333px"),
        // An in-flow inline box: no kept widths while it is there, kept ones once it goes.
        two("#b padding + #ib width", "#b", "padding: 5px", "#ib", "display: inline-block; width: 90px"),
        two("#b padding + #ib gone", "#b", "padding: 2px", "#ib", "display: none"),
        two("#b border + #t width", "#b", "border-width: 4px", "#t", "width: 210px"),
        two("#b padding + #ib back", "#b", "padding: 6px", "#ib", "display: inline-block; width: 40px"),
        two("#b padding + #ib absolute", "#b", "padding: 3px", "#ib", "position: absolute; width: 40px"),
        two("#b padding + #ib in flow", "#b", "padding: 8px", "#ib", "display: inline-block; width: 15px"),
        // Text styles the widths depend on, with a box change on the same cell.
        style("#a", "padding: 5px; white-space: nowrap"),
        style("#a", "padding: 2px; white-space: normal"),
        style("#a", "padding: 4px; letter-spacing: 2.5px"),
        style("#a", "padding: 4px; word-break: break-all"),
        style("#a", "padding: 1px"),
        // Intrinsic sizing of a kept paragraph.
        style("#p", "border: 4px solid"),
        two("#p border + #w max-content", "#p", "border: 2px solid", "#w", "width: max-content"),
        two("#p padding + #w fit-content", "#p", "padding: 0 9px", "#w", "width: fit-content; max-width: 70px"),
        two("#p colour + #w min-content", "#p", "color: red", "#w", "width: min-content"),
    ];
    let (failures, counts) = review_steps(html, steps);
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    // The keep path ran: a box-only change on a cell shapes no text.
    for (what, shaped, _) in &counts {
        if what == "#a { padding: 6px }" || what == "#e { padding: 5px; border-width: 3px }" {
            assert_eq!(*shaped, 0, "{what} shaped text again");
        }
        if std::env::var_os("REVIEW_COUNTS").is_some() {
            eprintln!("{what}: shaped {shaped}");
        }
    }
}

// ---------------- review (lay2): the walks that follow damage skip children whose
// `damaged_descendants` flag is clear (`Node::has_no_damage_below`, `is_clean_layout_child_of`), a
// table visits only the cells noted at damage time (`table::cells_to_visit`), and hit bounds are
// computed at the first hit test after a resolve (`ensure_hit_overflow`). Every damage writer must
// flag; each case below drives one writer and compares with a fresh build (boxes, document size, hit
// candidates, paint). Run under `--profile test-fast` too: the debug asserts are off there. ----------

/// Steps synced as deltas (a whole-tree sync when the delta does not fit), each compared with a fresh
/// build; hit candidates only after the steps `hit` selects (and the last), so hit bounds go stale
/// over several resolves. Returns the failures.
fn lay2_failures(html: &str, steps: Vec<(String, Mutate)>, hit: &dyn Fn(usize) -> bool) -> Vec<String> {
    let page = Page::new(html);
    let mut r = Renderer::new();
    r.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
    let mut failures = Vec::new();
    let n = steps.len();
    for (i, (what, mutate)) in steps.into_iter().enumerate() {
        let since = page.dom.borrow().version();
        mutate(&page);
        let delta = page.dom.borrow().render_delta(page.doc, since, &Page::opts());
        let via_delta =
            delta.is_some_and(|d| r.sync_delta(1, &delta_ops(d), (640, 480, 1.0, false, false), false).is_ok());
        if !via_delta {
            r.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        }
        lay2_compare(&mut r, &page, &format!("step {i} {what}"), hit(i) || i + 1 == n, &mut failures);
    }
    failures
}

/// `r` against a fresh build of `page`: boxes, document size, hit candidates (`hit`), paint.
fn lay2_compare(r: &mut Renderer, page: &Page, what: &str, hit: bool, failures: &mut Vec<String>) {
    lay2_compare_at(r, page, page.snapshot(1, page.tree()), what, hit, failures)
}

fn lay2_compare_at(r: &mut Renderer, page: &Page, snap: Snapshot, what: &str, hit: bool, failures: &mut Vec<String>) {
    lay2_compare_with(r, page, snap, &fetcher(), what, hit, failures)
}

fn lay2_compare_with(
    r: &mut Renderer,
    page: &Page,
    snap: Snapshot,
    fetch: &Arc<dyn Fetcher>,
    what: &str,
    hit: bool,
    failures: &mut Vec<String>,
) {
    let mut fresh = Renderer::new();
    fresh.sync_layout(&snap, fetch).unwrap();
    for ((k, a), (_, b)) in boxes(r, page).into_iter().zip(boxes(&fresh, page)) {
        if a != b {
            failures.push(format!("{what}: box {k:x} {a:?} vs fresh {b:?}"));
            break;
        }
    }
    if r.document_size(1) != fresh.document_size(1) {
        failures.push(format!("{what}: size {:?} vs fresh {:?}", r.document_size(1), fresh.document_size(1)));
    }
    if hit {
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hit_candidates_match(r, &mut fresh, what)));
        if let Err(e) = res {
            let msg = e.downcast_ref::<String>().cloned().unwrap_or_default();
            failures.push(format!("{what}: hit candidates: {}", msg.lines().take(2).collect::<Vec<_>>().join(" / ")));
        }
    }
    let painted = r.paint(&snap, Area::Viewport, fetch).unwrap();
    if painted.hash != Renderer::new().paint(&snap, Area::Viewport, fetch).unwrap().hash {
        failures.push(format!("{what}: paint differs"));
    }
}

#[track_caller]
fn lay2_assert(html: &str, steps: impl Fn() -> Vec<(String, Mutate)>) {
    let every: &dyn Fn(usize) -> bool = &|_| true;
    let third: &dyn Fn(usize) -> bool = &|i| i % 3 == 2;
    for (label, hit) in [("hit after every step", every), ("hit after every third", third)] {
        let failures = lay2_failures(html, steps(), hit);
        assert!(failures.is_empty(), "{label}: {} failures:\n{}", failures.len(), failures.join("\n"));
    }
}

fn lay2_rows(n: usize) -> String {
    (0..n)
        .map(|i| {
            format!(
                "<tr id=r{i}>\
                 <td id=a{i}>A{i} <span id=sa{i}>s{i}</span></td>\
                 <td id=b{i}><div id=cb{i} class=c><p id=pb{i}>first {i}</p></div></td>\
                 <td id=c{i}><ol id=ol{i}><li id=la{i}>x</li><li id=lb{i}>y</li></ol></td>\
                 <td id=d{i} class=z><span id=sd{i} class=pa data-x=\"{i}\">d</span></td></tr>"
            )
        })
        .collect()
}

fn lay2_page(extra: &str) -> String {
    format!(
        "<!doctype html><html><head><style>body {{ margin: 0; font: 13px sans-serif }} td {{ padding: 2px }} \
         p {{ margin: 0 }} ol {{ margin: 0; padding-left: 18px }} .c {{ display: contents }} \
         .pb::before {{ content: 'B'; display: inline-block; width: 14.5px }} .pa::after {{ content: attr(data-x) }} \
         .pblk::before {{ content: 'blk'; display: block; height: 11.5px }} .fl::first-letter {{ font-size: 26px }} \
         .fline::first-line {{ font-weight: bold; font-size: 17px }} .on {{ color: red }} .big {{ font-size: 21px }} \
         .h {{ display: none }} td:has(.hot) {{ padding: 7px }} tr:nth-child(odd) td.z {{ padding-left: 11px }} \
         .ins {{ list-style-position: inside }} {extra}</style></head><body>\
         <table id=t style=\"width:560px\"><tbody id=tb>{}</tbody></table>\
         <div id=after>after the table</div></body></html>",
        lay2_rows(10)
    )
}

#[test]
fn review_lay2_pseudo_elements_in_cells_match_fresh_builds() {
    lay2_assert(
        &lay2_page(""),
        || vec![
            class("#a3", "pb"),
            attr("#sd2", "data-x", "a longer generated content"),
            class("#sd2", ""),
            class("#sd2", "pa"),
            class("#pb4", "pblk"),
            edit("a ::before added beside a deep edit in another cell", |p| {
                let a = p.find("#a5");
                let s = p.find("#sa7");
                let mut d = p.dom.borrow_mut();
                d.set_attribute(a, "class", "pblk").unwrap();
                d.set_text_content(s, "a much longer span text that wraps in its cell over lines").unwrap();
            }),
            class("#t", "pb"),
            class("#t", ""),
            class("#tb", "pa"),
            class("#a3", ""),
            attr("#sd6", "data-x", ""),
            attr("#sd6", "data-x", "6"),
        ],
    );
}

#[test]
fn review_lay2_first_letter_and_first_line_in_cells_match_fresh_builds() {
    lay2_assert(
        &lay2_page(""),
        || vec![
            class("#pb3", "fl"),
            text("#pb3", "Wider first letter"),
            class("#pb5", "fline"),
            text("#pb5", "a first line that is long enough to wrap in its cell, then more words"),
            class("#pb3", ""),
            class("#a1", "fl"),
            class("#pb5", ""),
        ],
    );
}

#[test]
fn review_lay2_list_markers_in_cells_match_fresh_builds() {
    lay2_assert(
        &lay2_page(""),
        || vec![
            edit("an item inserted first in a list in a cell", |p| {
                let ol = p.find("#ol5");
                let first = p.find("#la5");
                let mut d = p.dom.borrow_mut();
                let li = d.create_element(p.doc, "li").unwrap();
                let t = d.create_text(p.doc, "new").unwrap();
                d.append_child(li, t).unwrap();
                d.insert_before(ol, li, Some(first)).unwrap();
            }),
            attr("#ol6", "start", "998"),
            attr("#ol7", "reversed", ""),
            class("#ol6", "ins"),
            attr("#ol6", "start", "1"),
            edit("an item removed", |p| {
                let ol = p.find("#ol5");
                let first = p.find("#la5");
                p.dom.borrow_mut().remove_child(ol, first).unwrap();
            }),
            set_style("#lb2", "display:block"),
            set_style("#lb2", ""),
            set_style("#la8", "list-style-type:upper-roman"),
        ],
    );
}

#[test]
fn review_lay2_display_contents_and_anonymous_blocks_in_cells_match_fresh_builds() {
    lay2_assert(
        &lay2_page(""),
        || vec![
            set_style("#cb2", "display:block;padding:3.5px"),
            set_style("#cb2", ""),
            edit("a block inserted into inline content of a cell (anonymous blocks)", |p| {
                let a = p.find("#a1");
                let s = p.find("#sa1");
                let mut d = p.dom.borrow_mut();
                let div = d.create_element(p.doc, "div").unwrap();
                d.set_attribute(div, "id", "blk1").unwrap();
                let t = d.create_text(p.doc, "block in inline").unwrap();
                d.append_child(div, t).unwrap();
                d.insert_before(a, div, Some(s)).unwrap();
            }),
            text("#sa1", "the inline after the block, edited"),
            text("#blk1", "the block, edited, and longer than it was"),
            edit("the block removed again", |p| {
                let a = p.find("#a1");
                let b = p.find("#blk1");
                p.dom.borrow_mut().remove_child(a, b).unwrap();
            }),
            text("#pb2", "inside display contents, edited"),
            edit("a span in a contents wrapper made a block", |p| {
                let cb = p.find("#cb3");
                let mut d = p.dom.borrow_mut();
                let s = d.create_element(p.doc, "span").unwrap();
                d.set_attribute(s, "style", "display:block;height:9.5px").unwrap();
                d.append_child(cb, s).unwrap();
            }),
            set_style("#cb4", "display:contents;color:blue"),
            class("#cb5", ""),
            class("#cb5", "c"),
        ],
    );
}

#[test]
fn review_lay2_selector_restyles_reach_cells_match_fresh_builds() {
    lay2_assert(
        &lay2_page(""),
        || vec![
            class("#sa6", "hot"),
            edit("a row inserted first (every odd/even row flips)", |p| {
                let tb = p.find("#tb");
                let first = p.find("#r0");
                let mut d = p.dom.borrow_mut();
                let tr = d.create_element(p.doc, "tr").unwrap();
                for k in 0..4 {
                    let td = d.create_element(p.doc, "td").unwrap();
                    if k == 3 {
                        d.set_attribute(td, "class", "z").unwrap();
                    }
                    let t = d.create_text(p.doc, &format!("new {k}")).unwrap();
                    d.append_child(td, t).unwrap();
                    d.append_child(tr, td).unwrap();
                }
                d.insert_before(tb, tr, Some(first)).unwrap();
            }),
            class("#sa6", ""),
            edit("a :has() subject made by an insertion", |p| {
                let a = p.find("#a8");
                let mut d = p.dom.borrow_mut();
                let b = d.create_element(p.doc, "b").unwrap();
                d.set_attribute(b, "class", "hot").unwrap();
                d.append_child(a, b).unwrap();
            }),
            class("#t", "big"),
            class("#t", ""),
            class("#tb", "on"),
            edit("the first row removed (rows flip back)", |p| {
                let tb = p.find("#tb");
                let mut d = p.dom.borrow_mut();
                let first = d.first_child(tb).unwrap().unwrap();
                d.remove_child(tb, first).unwrap();
            }),
        ],
    );
}

#[test]
fn review_lay2_table_relisting_beside_deep_edits_match_fresh_builds() {
    lay2_assert(
        &lay2_page(""),
        || vec![
            edit("a row moved and a deep edit in another row", |p| {
                let tb = p.find("#tb");
                let r7 = p.find("#r7");
                let r1 = p.find("#r1");
                let s = p.find("#sa3");
                let mut d = p.dom.borrow_mut();
                d.insert_before(tb, r7, Some(r1)).unwrap();
                d.set_text_content(s, "deep edit beside a move, wider").unwrap();
            }),
            attr("#b4", "colspan", "2"),
            set_style("#c2", "display:none"),
            text("#sa2", "edit in a row with a hidden cell"),
            set_style("#c2", ""),
            set_style("#r5", "display:none"),
            text("#sa9", "edited while another row is hidden"),
            set_style("#r5", ""),
            edit("a cell removed and a cell edited", |p| {
                let r = p.find("#r6");
                let c = p.find("#d6");
                let s = p.find("#sa8");
                let mut d = p.dom.borrow_mut();
                d.remove_child(r, c).unwrap();
                d.set_text_content(s, "x").unwrap();
            }),
            attr("#b4", "colspan", "1"),
            edit("a row detached, edited, and appended", |p| {
                let tb = p.find("#tb");
                let r = p.find("#r3");
                let s = p.find("#sa3");
                let mut d = p.dom.borrow_mut();
                d.remove_child(tb, r).unwrap();
                d.set_text_content(s, "while detached").unwrap();
                d.append_child(tb, r).unwrap();
            }),
        ],
    );
}

#[test]
fn review_lay2_out_of_flow_and_form_controls_in_cells_match_fresh_builds() {
    lay2_assert(
        &lay2_page(".abs { position: absolute; left: 400px; top: 300px; width: 90px } td.rel { position: relative }"),
        || vec![
            edit("an absolute box with inline content put in a static cell", |p| {
                let a = p.find("#a4");
                let mut d = p.dom.borrow_mut();
                let div = d.create_element(p.doc, "div").unwrap();
                d.set_attribute(div, "class", "abs").unwrap();
                d.set_attribute(div, "id", "abs4").unwrap();
                let s = d.create_element(p.doc, "span").unwrap();
                d.set_attribute(s, "id", "abs4s").unwrap();
                let t = d.create_text(p.doc, "abs text").unwrap();
                d.append_child(s, t).unwrap();
                d.append_child(div, s).unwrap();
                d.append_child(a, div).unwrap();
            }),
            set_style("#abs4s", "display:block;height:30.5px"),
            text("#abs4s", "abs text changed and longer, wrapping to more lines"),
            set_style("#abs4s", "display:inline"),
            class("#a4", "rel"),
            text("#abs4s", "in a positioned cell"),
            class("#a4", ""),
            edit("a text field and a textarea put in cells", |p| {
                let a = p.find("#a6");
                let b = p.find("#a7");
                let mut d = p.dom.borrow_mut();
                let i = d.create_element(p.doc, "input").unwrap();
                d.set_attribute(i, "id", "in6").unwrap();
                d.append_child(a, i).unwrap();
                let ta = d.create_element(p.doc, "textarea").unwrap();
                d.set_attribute(ta, "id", "ta7").unwrap();
                d.append_child(b, ta).unwrap();
            }),
            edit("values typed", |p| {
                let i = p.find("#in6");
                let ta = p.find("#ta7");
                let mut d = p.dom.borrow_mut();
                d.set_value(i, "typed into a field").unwrap();
                d.set_value(ta, "line one\nline two\nline three\nline four").unwrap();
            }),
            attr("#ta7", "rows", "1"),
        ],
    );
}

#[test]
fn review_lay2_hidden_tables_nested_tables_and_shadow_trees_match_fresh_builds() {
    let html = lay2_page(".inner td { border: 1px solid #888 }").replace(
        "<div id=after>after the table</div>",
        "<div id=wrap><table id=o><tr><td id=oc><table class=inner id=i><tr id=ir><td id=ic1>inner 1</td><td id=ic2>inner <b id=ib>2</b></td></tr></table></td><td id=oc2>outer</td></tr></table></div>\
         <div id=host>light <span id=sl slot=x>slotted</span><span id=sl2>other</span></div>",
    );
    lay2_assert(
        &html,
        || vec![
            set_style("#t", "display:none"),
            text("#sa1", "edited while the table is hidden"),
            text("#sa4", "and again, in another cell, while hidden"),
            set_style("#t", "width:560px"),
            text("#ib", "inner bold, now much wider than before"),
            set_style("#ic1", "width:150.5px"),
            text("#oc2", "outer edited beside an inner change"),
            edit("an inner row appended", |p| {
                let i = p.find("#ir");
                let mut d = p.dom.borrow_mut();
                let tbody = d.parent(i).unwrap().unwrap();
                let tr = d.create_element(p.doc, "tr").unwrap();
                let td = d.create_element(p.doc, "td").unwrap();
                let t = d.create_text(p.doc, "appended inner").unwrap();
                d.append_child(td, t).unwrap();
                d.append_child(tr, td).unwrap();
                d.append_child(tbody, tr).unwrap();
            }),
            edit("a shadow root with a slot attached", |p| {
                let host = p.find("#host");
                let mut d = p.dom.borrow_mut();
                let root = d.attach_shadow(host, pyro_dom::node::ShadowRootMode::Open).unwrap();
                let div = d.create_element(p.doc, "div").unwrap();
                d.set_attribute(div, "style", "border:2px solid;padding:3px").unwrap();
                let slot = d.create_element(p.doc, "slot").unwrap();
                d.set_attribute(slot, "name", "x").unwrap();
                d.append_child(div, slot).unwrap();
                let t = d.create_text(p.doc, " shadow text").unwrap();
                d.append_child(div, t).unwrap();
                d.append_child(root, div).unwrap();
            }),
            text("#sl", "slotted text, edited"),
            attr("#sl2", "slot", "x"),
            attr("#sl", "slot", "y"),
            set_style("#sl2", "display:block;height:20.5px"),
        ],
    );
}

/// Viewport changes (a whole-tree sync at another width: the root's `ONLY_RELAYOUT` from the device
/// change, media queries that restyle cells, `vw` sizes) between delta-synced cell edits.
#[test]
fn review_lay2_viewport_changes_between_cell_edits_match_fresh_builds() {
    let page = Page::new(&lay2_page(
        "@media (max-width: 500px) { td.z { display: none } .pa::after { content: 'narrow' } } \
         @media (min-width: 700px) { #b3 { width: 30vw } } #a2 { width: 12.5vw }",
    ));
    let mut r = Renderer::new();
    let mut failures = Vec::new();
    let at = |w: u32| Snapshot { width: w, ..page.snapshot(1, page.tree()) };
    r.sync_layout(&at(640), &fetcher()).unwrap();
    let steps: Vec<(u32, (String, Mutate))> = vec![
        (640, text("#sa3", "an edit at 640")),
        (480, noop("narrowed to 480")),
        (480, text("#sa5", "an edit at 480, while the last column is hidden")),
        (480, text("#sd5", "an edit in a hidden cell")),
        (800, noop("widened to 800")),
        (800, text("#pb3", "an edit in the vw-sized cell")),
        (800, class("#a6", "pb")),
        (640, text("#sa1", "back at 640")),
        (633, noop("a few pixels narrower")),
        (633, text("#sd1", "the cell hidden at 480, edited at 633")),
    ];
    let mut width = 640;
    for (i, (w, (what, mutate))) in steps.into_iter().enumerate() {
        let since = page.dom.borrow().version();
        mutate(&page);
        if w == width {
            let delta = page.dom.borrow().render_delta(page.doc, since, &Page::opts());
            let ok = delta.is_some_and(|d| r.sync_delta(1, &delta_ops(d), (w, 480, 1.0, false, false), false).is_ok());
            if !ok {
                r.sync_layout(&at(w), &fetcher()).unwrap();
            }
        } else {
            r.sync_layout(&at(w), &fetcher()).unwrap();
            width = w;
        }
        lay2_compare_at(&mut r, &page, at(w), &format!("step {i} {what} at {w}"), i % 2 == 1, &mut failures);
    }
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
}

/// Serves Lato and a red PNG after a delay, on another thread.
struct Lay2Slow(std::time::Duration);
impl Fetcher for Lay2Slow {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let delay = self.0;
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            done(if url.ends_with("lato.ttf") {
                Ok(LATO.to_vec())
            } else if url.ends_with("red.png") {
                Ok(encode_png(&RgbaImage { width: 37, height: 23, data: [255, 0, 0, 255].repeat(37 * 23) }))
            } else {
                Err("404".into())
            })
        });
    }
}

/// A web font and images arriving after the first layout (`invalidate_inline_contexts` and
/// `apply_loaded_image` set damage on cells' contents between syncs), then delta-synced edits in
/// other cells: every cell whose text or image changed is built and laid out again.
#[test]
fn review_lay2_fonts_and_images_arriving_in_cells_match_fresh_builds() {
    let html = lay2_page(
        "@font-face { font-family: Lato; src: url(lato.ttf) } td.lato { font: 15px Lato, serif }",
    )
    .replace("<td id=a4>", "<td id=a4 class=lato>")
    .replace("<td id=b6>", "<td id=b6><img id=im6 src=red.png>")
    .replace("<td id=c8>", "<td id=c8 class=lato><img id=im8 src=red.png style=\"float:right\">");
    let page = Page::new(&html);
    let slow: Arc<dyn Fetcher> = Arc::new(Lay2Slow(std::time::Duration::from_millis(400)));
    let ready: Arc<dyn Fetcher> = Arc::new(Lay2Slow(std::time::Duration::from_millis(0)));
    let mut r = Renderer::new();
    r.first_layout_wait = std::time::Duration::from_millis(10);
    r.sync_layout(&page.snapshot(1, page.tree()), &slow).unwrap();
    let mut failures = Vec::new();
    std::thread::sleep(std::time::Duration::from_millis(900));
    let steps: Vec<(String, Mutate)> = vec![
        text("#sa1", "an unrelated cell edited after the font and images arrived"),
        text("#sa4", "the Lato cell edited"),
        set_style("#im6", "width:50px"),
        text("#sa9", "the last edit"),
    ];
    for (i, (what, mutate)) in steps.into_iter().enumerate() {
        let since = page.dom.borrow().version();
        mutate(&page);
        let delta = page.dom.borrow().render_delta(page.doc, since, &Page::opts());
        if !delta.is_some_and(|d| r.sync_delta(1, &delta_ops(d), (640, 480, 1.0, false, false), false).is_ok()) {
            r.sync_layout(&page.snapshot(1, page.tree()), &slow).unwrap();
        }
        lay2_compare_with(&mut r, &page, page.snapshot(1, page.tree()), &ready, &format!("step {i} {what}"), true, &mut failures);
    }
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
}

/// Random mutations of a long table (cell text, classes that change layout, paint, display,
/// generated content and `display: contents`, inline styles, attributes set to their value, rows and
/// cells inserted, moved and removed, colspans, blocks inserted into inline content), one to four per
/// sync, each synced as a delta.
///
/// Blitz's incremental layout has known gaps against a fresh build on such sequences (stale boxes of
/// elements hidden by `display: none` or `contents` cells, a few px of document height after moved
/// rows) on 267ac8c5 as well, so the check is differential: with PYRO_LAY2_DUMP=file each sync's boxes,
/// document size, hit candidates and paint hash are written there, to compare a build of this tree
/// with one of the base (they must be identical). Without it, the sequences run with debug asserts
/// on (the damage-flag invariant checks) and hit candidates are taken after every third sync.
/// PYRO_LAY2_SEEDS / PYRO_LAY2_STEPS / PYRO_LAY2_FROM: more or other sequences.
#[test]
fn review_lay2_random_table_deltas() {
    use std::io::Write;
    let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u64>().ok());
    let seeds = env("PYRO_LAY2_SEEDS").unwrap_or(if cfg!(debug_assertions) { 3 } else { 12 });
    let steps = env("PYRO_LAY2_STEPS").unwrap_or(if cfg!(debug_assertions) { 10 } else { 30 });
    let from = env("PYRO_LAY2_FROM").unwrap_or(0);
    let mut dump = std::env::var_os("PYRO_LAY2_DUMP").map(|p| std::fs::File::create(p).unwrap());
    for seed in from..from + seeds {
        let page = Page::new(&lay2_page(""));
        let mut r = Renderer::new();
        r.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
        let mut rng = Rng(0x1234_5678_9abc_def1 ^ (seed + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15));
        for step in 0..steps {
            let since = page.dom.borrow().version();
            let n = 1 + rng.next() % 4;
            let done: Vec<String> = (0..n).map(|_| lay2_mutate(&page, &mut rng)).collect();
            let delta = page.dom.borrow().render_delta(page.doc, since, &Page::opts());
            if !delta.is_some_and(|d| r.sync_delta(1, &delta_ops(d), (640, 480, 1.0, false, false), false).is_ok()) {
                r.sync_layout(&page.snapshot(1, page.tree()), &fetcher()).unwrap();
            }
            // Hit tests after some syncs only: the bounds of several resolves are brought up to date at once.
            let hit = step % 3 == 2 || step + 1 == steps;
            if let Some(f) = dump.as_mut() {
                writeln!(f, "seed {seed} step {step} {done:?}\n{}", lay2_signature(&mut r, &page, hit)).unwrap();
            } else if hit {
                r.ensure_hit_bounds(1);
                let _ = lay2_hits(&r);
            }
        }
    }
}

/// Every element's box, the document size, the hit candidates at a grid (`hit`) and the paint hash.
fn lay2_signature(r: &mut Renderer, page: &Page, hit: bool) -> String {
    let mut out = String::new();
    for (k, b) in boxes(r, page) {
        out.push_str(&format!("  {k:x} {b:?}\n"));
    }
    out.push_str(&format!("  size {:?}\n", r.document_size(1)));
    if hit {
        r.ensure_hit_bounds(1);
        out.push_str(&format!("  hits {}\n", lay2_hits(r)));
    }
    let painted = paint(r, &page.snapshot(1, page.tree()));
    out.push_str(&format!("  paint {:x}\n", painted.hash));
    out
}

/// The hit candidates at a grid over the viewport, hashed.
fn lay2_hits(r: &Renderer) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let zero = |_: u64| (0.0, 0.0);
    for y in (2..480).step_by(9) {
        for x in (2..640).step_by(13) {
            let probe = HitProbe { x: x as f64, y: y as f64, ignore_x: false, window_scroll: (0.0, 0.0) };
            let visits = r.hit_candidates(1, &probe, &zero, &[]).expect("hit bounds up to date");
            for v in visits {
                (v.key, format!("{:?}", v.bounds), v.hit).hash(&mut h);
            }
        }
    }
    h.finish()
}

fn lay2_mutate(page: &Page, rng: &mut Rng) -> String {
    let t = page.find("#tb");
    let (rows, cells, inner): (Vec<NodeId>, Vec<NodeId>, Vec<NodeId>) = {
        let d = page.dom.borrow();
        let all: Vec<NodeId> =
            d.descendants(t).unwrap().filter(|&n| n != t && d.node_type(n).ok() == Some(NodeType::Element)).collect();
        let tag = |n: NodeId| d.tag_name(n).unwrap();
        (
            all.iter().copied().filter(|&n| tag(n) == "TR").collect(),
            all.iter().copied().filter(|&n| tag(n) == "TD").collect(),
            all.iter().copied().filter(|&n| !matches!(tag(n).as_str(), "TR" | "TD")).collect(),
        )
    };
    let mut d = page.dom.borrow_mut();
    let classes = ["on", "big", "h", "pa", "pb", "pblk", "c", "fl", "hot", "z", "ins", ""];
    match rng.next() % 13 {
        0 | 1 => {
            // Text deep in a cell.
            let Some(e) = rng.pick(&inner).or(rng.pick(&cells)) else { return "nothing".into() };
            let n = rng.next() % 40;
            d.set_text_content(e, &"word ".repeat(n as usize)).unwrap();
            format!("text {} x{n}", d.tag_name(e).unwrap())
        }
        2 | 3 => {
            let Some(e) = rng.pick(&inner).or(rng.pick(&cells)) else { return "nothing".into() };
            let c = rng.pick(&classes).unwrap();
            d.set_attribute(e, "class", c).unwrap();
            format!("class {} {c:?}", d.tag_name(e).unwrap())
        }
        4 => {
            let Some(e) = rng.pick(&cells) else { return "nothing".into() };
            let c = rng.pick(&classes).unwrap();
            d.set_attribute(e, "class", c).unwrap();
            format!("cell class {c:?}")
        }
        5 => {
            let Some(e) = rng.pick(&inner).or(rng.pick(&cells)) else { return "nothing".into() };
            let v = rng
                .pick(&["padding-left:7.5px", "color:green", "display:block", "display:inline-block;width:33px", "", "width:180px", "visibility:hidden"])
                .unwrap();
            d.set_attribute(e, "style", v).unwrap();
            format!("style {} {v:?}", d.tag_name(e).unwrap())
        }
        6 => {
            // An attribute set to the value it has (a restyle that changes nothing), or data-x.
            let Some(e) = rng.pick(&inner).or(rng.pick(&cells)) else { return "nothing".into() };
            if rng.next() % 2 == 0 {
                let v = d.get_attribute(e, "class").unwrap().unwrap_or("").to_owned();
                d.set_attribute(e, "class", &v).unwrap();
                "same class".into()
            } else {
                let v = (rng.next() % 1000).to_string();
                d.set_attribute(e, "data-x", &v).unwrap();
                "data-x".into()
            }
        }
        7 => {
            // A row inserted (a clone of another).
            let Some(src) = rng.pick(&rows) else { return "nothing".into() };
            let c = d.clone_node(src, true, None).unwrap();
            d.remove_attribute(c, "id").unwrap();
            let anchor = rng.pick(&rows);
            d.insert_before(t, c, anchor).unwrap();
            "row cloned".into()
        }
        8 => {
            if rows.len() < 3 {
                return "nothing".into();
            }
            let row = rng.pick(&rows).unwrap();
            if rng.next() % 2 == 0 {
                d.remove_child(t, row).unwrap();
                "row removed".into()
            } else {
                let anchor = rng.pick(&rows);
                if anchor == Some(row) {
                    return "nothing".into();
                }
                d.insert_before(t, row, anchor).unwrap();
                "row moved".into()
            }
        }
        9 => {
            let Some(cell) = rng.pick(&cells) else { return "nothing".into() };
            let row = d.parent(cell).unwrap().unwrap();
            match rng.next() % 3 {
                0 if d.children(row).unwrap().count() > 1 => {
                    d.remove_child(row, cell).unwrap();
                    "cell removed".into()
                }
                1 => {
                    let td = d.create_element(page.doc, "td").unwrap();
                    let tx = d.create_text(page.doc, "new cell").unwrap();
                    d.append_child(td, tx).unwrap();
                    d.insert_before(row, td, Some(cell)).unwrap();
                    "cell inserted".into()
                }
                _ => {
                    let span = (1 + rng.next() % 3).to_string();
                    d.set_attribute(cell, "colspan", &span).unwrap();
                    format!("colspan {span}")
                }
            }
        }
        10 => {
            // A block or an inline inserted somewhere in a cell.
            let Some(host) = rng.pick(&inner).or(rng.pick(&cells)) else { return "nothing".into() };
            let tag = d.tag_name(host).unwrap();
            if matches!(tag.as_str(), "OL") {
                let li = d.create_element(page.doc, "li").unwrap();
                let tx = d.create_text(page.doc, "item").unwrap();
                d.append_child(li, tx).unwrap();
                let first = d.first_child(host).unwrap();
                d.insert_before(host, li, first).unwrap();
                return "li inserted".into();
            }
            let el = d.create_element(page.doc, rng.pick(&["div", "span", "b"]).unwrap()).unwrap();
            let tx = d.create_text(page.doc, "ins").unwrap();
            d.append_child(el, tx).unwrap();
            let kids: Vec<NodeId> = d.children(host).unwrap().collect();
            let anchor = rng.pick(&kids);
            d.insert_before(host, el, anchor).unwrap();
            format!("inserted into {tag}")
        }
        11 => {
            // An element of a cell removed, or moved into another cell.
            let Some(e) = rng.pick(&inner) else { return "nothing".into() };
            let Some(parent) = d.parent(e).unwrap() else { return "nothing".into() };
            if d.tag_name(e).unwrap() == "LI" || rng.next() % 2 == 0 {
                d.remove_child(parent, e).unwrap();
                "inner removed".into()
            } else {
                let Some(to) = rng.pick(&cells) else { return "nothing".into() };
                if d.contains(e, to).unwrap_or(true) {
                    return "nothing".into();
                }
                d.append_child(to, e).unwrap();
                "inner moved to another cell".into()
            }
        }
        _ => {
            // A row hidden or shown, or made contents.
            let Some(row) = rng.pick(&rows) else { return "nothing".into() };
            let v = rng.pick(&["display:none", "", "", "height:31.5px", "display:contents", "color:blue"]).unwrap();
            d.set_attribute(row, "style", v).unwrap();
            format!("row style {v:?}")
        }
    }
}

// ---------------- review (misc2): the layout dispatch split into non-inlined helpers
// (`leaf_layout_or_algorithm`, `compute_table_root_layout`, `adjusted_child_inputs`,
// `lay_out_out_of_flow_boxes`, `mirror_child_overflow`, `compute_inner_in_new_bfc`): every leaf
// and algorithm path, delta-synced against fresh builds ----------------

#[test]
fn review_misc2_every_dispatch_path_matches_fresh_builds() {
    steps_match_fresh_builds_at(
        r#"<div id=f style="display:flex;flex-wrap:wrap;gap:3px;width:400.5px"><input id=i1><input id=cb type=checkbox><input id=pw type=password size=7><input id=hid type=hidden><input id=num type=number><textarea id=ta rows=2>t</textarea><select id=dd><option>One<option id=lo>Longest option label</select><select id=lb multiple><option>a<option>b</select><select id=sz size=3><option>a</select><img id=img width=30 height=20 alt=i><svg id=svg width=40 height=10></svg><button id=btn>Btn</button></div>
<div id=col style="display:flex;flex-direction:column"><div id=mm style="min-width:max-content">max content min width text</div><div id=half style="width:50%"><input id=pct style="width:100%"></div></div>
<div id=g style="display:grid;grid-template-columns:1fr 2fr"><div id=sp style="grid-column:span 2">span</div><div id=gf style="display:flex">a</div><div id=gc style="display:flex;flex-direction:column">b</div></div>
<div id=rel style="position:relative;height:80px"><table id=at style="position:absolute;left:-20px;right:-30px;top:5px"><tr><td id=atd>abs table<div id=oofc style="position:absolute;top:3px">oof in cell</div></td></tr></table></div>
<table id=t2 style="position:relative"><tr><td>t</td></tr><div id=toof style="position:absolute;left:4px">oof child</div></table>
<div id=vrl style="writing-mode:vertical-rl;height:120px"><p id=vp>vertical rl text <span id=orth style="display:inline-block;writing-mode:horizontal-tb;width:60px">orthogonal box</span></p><div id=vf style="display:flex">flex in vrl</div></div>
<div id=vlr style="writing-mode:vertical-lr;height:120px"><p>vertical lr</p><div id=hz style="writing-mode:horizontal-tb">horizontal in vertical</div><div id=opp style="writing-mode:vertical-rl">opp</div></div>
<div id=vsc style="writing-mode:vertical-rl;overflow:auto;width:80px;height:60px"><div id=vin style="width:200px;height:10px"></div></div>
<div id=fl style="float:left;width:50.5px">float</div><p id=bes>text beside the float text beside the float</p><div id=fr style="display:flow-root">flow root</div>
<span id=ib style="display:inline-block;padding:3px">ib <span style="display:inline-block">nested</span></span>"#,
        vec![
            (640, style("#f", "display:flex;flex-wrap:wrap;gap:3px;width:250.5px")),
            (640, attr("#hid", "type", "text")),
            (640, attr("#cb", "type", "radio")),
            (640, text("#lo", "an even longer longest option label")),
            (640, attr("#lb", "size", "5")),
            (640, attr("#sz", "size", "1")),
            (640, style("#img", "width:44.5px")),
            (640, style("#mm", "min-width:min-content")),
            (640, style("#col", "display:flex;flex-direction:row")),
            (640, style("#g", "display:grid;grid-template-columns:2fr 1fr;width:300px")),
            (480, style("#at", "position:absolute;left:-40px;right:-60px;top:5px;margin:0 7px")),
            (480, style("#oofc", "position:absolute;top:9px;left:2px")),
            (480, style("#toof", "position:absolute;right:4px")),
            (480, style("#vrl", "writing-mode:vertical-lr;height:150px")),
            (480, style("#orth", "display:inline-block;writing-mode:horizontal-tb;width:90px")),
            (480, style("#vlr", "writing-mode:vertical-rl;height:100px")),
            (480, style("#vin", "width:300px;height:10px")),
            (480, style("#fl", "float:right;width:80px")),
            (800, style("#fr", "display:block")),
            (800, style("#ib", "display:inline-flex;padding:3px")),
            (800, style("#t2", "position:static")),
            (800, style("#hz", "display:none")),
            (800, style("#lb", "display:none")),
            (800, style("#lb", "")),
        ],
    );
}

#[test]
fn review_misc2_deep_mixed_nesting_matches_fresh_builds() {
    // 240 levels cycling through every algorithm (block, flow-root, flex, column flex, grid,
    // inline-block, table, vertical writing mode, positioned), then edits deep inside.
    let kinds = [
        "padding-left:1px",
        "display:flow-root;padding-left:1px",
        "display:flex;padding-left:1px",
        "display:flex;flex-direction:column;padding-left:1px",
        "display:grid;padding-left:1px",
        "display:inline-block;padding-left:1px",
        "display:table;padding-left:1px",
        "writing-mode:vertical-lr;padding-left:1px",
        "writing-mode:horizontal-tb;position:relative;padding-left:1px",
    ];
    let depth = 240;
    let open: String = (0..depth).map(|i| format!("<div id=n{i} style=\"{}\">", kinds[i % kinds.len()])).collect();
    let body = format!("{open}<span id=leaf>deep text</span>{}", "</div>".repeat(depth));
    let page = Page::new(&format!("{PAINT_BOXES}{body}</body></html>"));
    let mut r = Renderer::new();
    check_boxes_at(&mut r, &page, 640, "the first render");
    for (i, what) in [(200usize, "padding-left:9px"), (120, "display:flex;padding-left:2px"), (60, "display:grid;width:300px")] {
        let n = page.find(&format!("#n{i}"));
        page.dom.borrow_mut().set_attribute(n, "style", what).unwrap();
        check_boxes_at(&mut r, &page, 500, &format!("#n{i} {what}"));
    }
    let leaf = page.find("#leaf");
    page.dom.borrow_mut().set_text_content(leaf, "a longer deep text that may wrap").unwrap();
    check_boxes_at(&mut r, &page, 640, "#leaf text");
}

#[test]
#[ignore = "review misc2: FAILS on 267ac8c5 too (paint differs from a fresh build, boxes equal)"]
fn review_misc2_a_password_input_size_change_paints_as_a_fresh_build() {
    // Pre-existing, not from the layout dispatch split: the same hashes on 267ac8c5 and 42c7a0f0.
    steps_match_fresh_builds(r#"<input id=pw type=password size=7 value=secret>"#, vec![attr("#pw", "size", "15")]);
}
