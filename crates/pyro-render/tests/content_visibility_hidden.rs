//! `content-visibility: hidden` skips an element's contents (neither laid out nor painted) and
//! contains its size (CSS Contain 2 § 4.1); on an element containment does not apply to (a
//! non-atomic inline box) it does nothing. WPT css-contain `content-visibility/*`. A geometry
//! query lays its contents out (`Renderer::force_hidden_contents`) without resizing it, painting
//! them or letting a hit test find them, until the next frame or state of the page.
use std::{cell::RefCell, rc::Rc, sync::Arc};

use pyro_dom::{
    parse::{parse_document, DomRef},
    snapshot::{RenderHtml, RenderNodeKind},
};
use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

/// The number of pure green pixels and the number of pure blue ones.
fn counts(body: &str) -> (usize, usize) {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><style>body{{margin:0}} .g{{width:40px;height:40px;background:#0f0}}</style>{body}"),
        base_url: "http://test/".into(),
        width: 200,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let data = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0.data;
    let (mut green, mut blue) = (0, 0);
    for p in data.chunks(4) {
        match (p[0], p[1], p[2]) {
            (0, 255, 0) => green += 1,
            (0, 0, 255) => blue += 1,
            _ => {}
        }
    }
    (green, blue)
}

#[test]
fn hidden_skips_the_contents_and_contains_the_size() {
    // The block's own background paints; its child does not, and does not size it (height 0).
    assert_eq!(counts("<div style='content-visibility:hidden;width:50px;background:#00f'><div class=g></div></div>"), (0, 0));
    assert_eq!(
        counts("<div style='content-visibility:hidden;width:50px;height:50px;background:#00f'><div class=g></div></div>"),
        (0, 2500)
    );
    assert_eq!(counts("<div style='width:50px'><div class=g></div></div>"), (1600, 0));
}

#[test]
fn hidden_does_nothing_on_an_inline_box() {
    assert_eq!(counts("<span style='content-visibility:hidden'><div class=g></div></span>"), (1600, 0));
}

/// The live tree of `html` and the render keys of its elements `ids`.
fn live(html: &str, ids: &[&str]) -> (Arc<Tree>, Vec<u64>) {
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", html);
    let d = dom.borrow();
    let keys = ids.iter().map(|id| d.get_element_by_id(doc, id).unwrap().map(pyro_dom::snapshot::render_key).unwrap()).collect();
    let tree = d
        .render_tree(doc, &RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None })
        .unwrap();
    let tree = Tree {
        nodes: tree
            .nodes
            .into_iter()
            .map(|node| TreeNode {
                key: node.key,
                parent: node.parent,
                kind: match node.kind {
                    RenderNodeKind::Element { name, attrs } => TreeKind::Element { name, attrs },
                    RenderNodeKind::Text(text) => TreeKind::Text(text),
                },
            })
            .collect(),
    };
    (Arc::new(tree), keys)
}

fn live_snap(tree: &Arc<Tree>) -> Snapshot {
    Snapshot {
        base_url: "http://test/".into(),
        width: 200,
        height: 200,
        device_scale_factor: 1.0,
        element_scroll: Some(Vec::new()),
        live: Some(Live { page: 1, tree: tree.clone(), clip: None, interaction: None }),
        ..Default::default()
    }
}

/// Whether the element has a box with an area (what a geometry query forces the layout for).
fn laid_out(renderer: &Renderer, key: u64) -> bool {
    renderer.element_box(1, key).is_some_and(|b| b.width > 0.0 || b.height > 0.0)
}

const FORCED: &str = "<!doctype html><style>body{margin:0}#outer{width:100px;content-visibility:hidden;background:#00f}\
    #inner{margin:25px;width:50px;height:50px;background:#0f0}#after{height:10px}</style>\
    <div id=outer><div id=inner></div></div><div id=after></div>";

#[test]
fn a_query_lays_out_hidden_contents_without_resizing_the_element() {
    // WPT css-contain `content-visibility-088`, `-hidden-offsetTop-left-width-height`: the
    // contents get their boxes; the element keeps its size containment (0 tall), and what
    // follows it does not move.
    let (tree, keys) = live(FORCED, &["outer", "inner", "after"]);
    let (outer, inner, after) = (keys[0], keys[1], keys[2]);
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let mut renderer = Renderer::new();
    renderer.sync_layout(&live_snap(&tree), &fetcher).unwrap();
    assert!(!laid_out(&renderer, inner), "skipped: no box");
    let below = renderer.element_box(1, after).unwrap().y;
    assert!(renderer.force_hidden_contents(1, inner), "its hidden ancestor was skipping it");
    let b = renderer.element_box(1, inner).expect("laid out for the query");
    assert_eq!((b.x, b.y, b.width, b.height), (25.0, 25.0, 50.0, 50.0));
    assert_eq!(renderer.element_box(1, outer).unwrap().height, 0.0, "still size-contained");
    assert_eq!(renderer.element_box(1, after).unwrap().y, below, "what follows stays put");
    // Forced once: a second query lays nothing out again.
    assert!(!renderer.force_hidden_contents(1, inner));
    // A new state of the page skips them again (a later query forces them anew).
    renderer.sync_layout(&live_snap(&tree), &fetcher).unwrap();
    assert!(!laid_out(&renderer, inner), "skipped again");
    assert!(renderer.force_hidden_contents(1, inner));
    assert!(laid_out(&renderer, inner));
}

#[test]
fn forced_hidden_contents_are_not_painted_nor_hit() {
    let (tree, keys) = live(FORCED, &["outer", "inner"]);
    let (outer, inner) = (keys[0], keys[1]);
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let mut renderer = Renderer::new();
    renderer.sync_layout(&live_snap(&tree), &fetcher).unwrap();
    assert!(renderer.force_hidden_contents(1, inner));
    // A hit test does not find them (they are skipped again first).
    let hits = renderer.elements_from_point(1, 50.0, 50.0);
    assert!(!hits.contains(&inner), "{hits:?}");
    assert!(!laid_out(&renderer, inner));
    // A capture does not paint them: no green.
    assert!(renderer.force_hidden_contents(1, inner));
    let data = renderer.render(&live_snap(&tree), Area::Viewport, Backend::Cpu, &fetcher).unwrap().0.data;
    let green = data.chunks(4).filter(|p| (p[0], p[1], p[2]) == (0, 255, 0)).count();
    assert_eq!(green, 0);
    assert!(!laid_out(&renderer, inner), "the frame skipped them again");
    assert_eq!(renderer.element_box(1, outer).unwrap().height, 0.0);
}

#[test]
fn a_query_lays_out_every_nested_hidden_ancestor() {
    // WPT css-contain `content-visibility-forced-layout-client-rects`: nested hidden elements
    // are all laid out for the query, sized as shown.
    let html = "<!doctype html><style>body{margin:0}.h{content-visibility:hidden}.t{width:12px;height:34px;position:relative;left:5px;top:7px}</style>\
        <div class=h><div class=h><div class=h><div class=t id=t></div></div></div></div>\
        <div class=h id=abs style=width:200px><div id=a style=position:absolute;top:100px;left:100px;width:10px;height:10px></div></div>";
    let (tree, keys) = live(html, &["t", "a"]);
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let mut renderer = Renderer::new();
    renderer.sync_layout(&live_snap(&tree), &fetcher).unwrap();
    assert!(renderer.force_hidden_contents(1, keys[0]));
    let b = renderer.element_box(1, keys[0]).expect("laid out through three hidden ancestors");
    assert_eq!((b.x, b.y, b.width, b.height), (5.0, 7.0, 12.0, 34.0));
    // An absolutely positioned box: the hidden element (layout containment) contains it
    // (`content-visibility-hidden-boundingbox-query`).
    assert!(renderer.force_hidden_contents(1, keys[1]));
    let a = renderer.element_box(1, keys[1]).unwrap();
    assert_eq!((a.x, a.y, a.width, a.height), (100.0, 100.0, 10.0, 10.0));
}

#[test]
fn an_element_without_a_hidden_ancestor_forces_nothing() {
    // An empty box outside hidden contents (or a `content-visibility: auto` one alone) lays
    // nothing out again.
    let html = "<!doctype html><div id=e></div><div style=display:none><div id=n></div></div>";
    let (tree, keys) = live(html, &["e", "n"]);
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let mut renderer = Renderer::new();
    renderer.sync_layout(&live_snap(&tree), &fetcher).unwrap();
    assert!(!renderer.force_hidden_contents(1, keys[0]));
    assert!(!renderer.force_hidden_contents(1, keys[1]));
}
