//! `content-visibility: auto` determines relevance in work linear in the number of tracked
//! elements: a page of thousands of auto cards scrolled (none of them changing) costs a bounded
//! number of steps per card, counted by `Renderer::content_visibility_work`. Last remembered sizes
//! (`contain-intrinsic-size: auto`) are kept without frames as a card scrolls away, never for a
//! box an element lacks, and in work linear in the elements that may remember one
//! (`Renderer::remembered_size_work`), none on a page without them.
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

fn live_tree(html: &str) -> Arc<Tree> {
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", html);
    let d = dom.borrow();
    let tree = d
        .render_tree(doc, &RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None })
        .unwrap();
    Arc::new(Tree {
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
    })
}

#[test]
fn relevance_of_ten_thousand_cards_is_linear_work() {
    const CARDS: u64 = 10_000;
    let cards: String = (0..CARDS).map(|_| "<div class=c><p>x</p></div>").collect();
    let html = format!(
        "<!doctype html><style>body{{margin:0}} .c{{content-visibility:auto;contain-intrinsic-size:auto 20px;height:20px}}</style>{cards}"
    );
    let tree = live_tree(&html);
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let mut renderer = Renderer::new();
    let snap = |scroll_y: f64| Snapshot {
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
        device_scale_factor: 1.0,
        scroll_y,
        element_scroll: Some(Vec::new()),
        live: Some(Live { page: 1, tree: tree.clone(), clip: None, interaction: None }),
        ..Default::default()
    };
    renderer.render(&snap(0.0), Area::Viewport, Backend::Cpu, &fetcher).unwrap();
    let first = renderer.content_visibility_work(1);
    assert!(first > 0, "the relevance pass ran");
    // Scrolled far down: relevance determined again, a bounded number of steps per card.
    renderer.render(&snap(100_000.0), Area::Viewport, Backend::Cpu, &fetcher).unwrap();
    let scrolled = renderer.content_visibility_work(1) - first;
    assert!(scrolled > 0, "scrolling determined relevance again");
    assert!(scrolled < 40 * CARDS, "{scrolled} steps for {CARDS} cards");
}

/// The green pixels of a capture of `html` at `scroll_y`, with element `#h` (when there is one)
/// held `sticky` down by its scroll container (as the embedder gives sticky offsets).
fn green_pixels(html: &str, scroll_y: f64, sticky: f64) -> usize {
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", html);
    let header = dom.borrow().get_element_by_id(doc, "h").unwrap().map(pyro_dom::snapshot::render_key);
    drop(dom);
    let tree = live_tree(html);
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let mut renderer = Renderer::new();
    let snap = Snapshot {
        base_url: "http://test/".into(),
        width: 300,
        height: 200,
        device_scale_factor: 1.0,
        scroll_y,
        element_scroll: Some(Vec::new()),
        sticky_offsets: header.map(|k| vec![(k, (0.0, sticky))]),
        live: Some(Live { page: 1, tree, clip: None, interaction: None }),
        ..Default::default()
    };
    let image = renderer.render(&snap, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    image.data.chunks(4).filter(|p| p[..3] == [0, 128, 0]).count()
}

#[test]
fn a_card_in_a_sticky_header_stays_relevant_where_paint_holds_it() {
    // The header sticks to the top: scrolled 5000px down, paint draws it (and the card in it) in
    // view, so the card is relevant and its contents shown.
    let html = "<!doctype html><style>body{margin:0;height:10000px}#h{position:sticky;top:0;height:50px}\
        .c{content-visibility:auto;contain-intrinsic-size:auto 50px}.g{height:50px;width:50px;background:green}</style>\
        <div id=h><div class=c><div class=g></div></div></div>";
    assert_eq!(green_pixels(html, 5000.0, 5000.0), 2500);
}

/// The render key of element `#id` of `html`.
fn key_of(html: &str, id: &str) -> u64 {
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", html);
    let key = dom.borrow().get_element_by_id(doc, id).unwrap().map(pyro_dom::snapshot::render_key).unwrap();
    key
}

/// A 400x300 live snapshot of `tree` (page 1) scrolled to `scroll_y`.
fn snap_at(tree: &Arc<Tree>, scroll_y: f64) -> Snapshot {
    Snapshot {
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
        device_scale_factor: 1.0,
        scroll_y,
        element_scroll: Some(Vec::new()),
        live: Some(Live { page: 1, tree: tree.clone(), clip: None, interaction: None }),
        ..Default::default()
    }
}

/// The last remembered size (`[inline, block]`) the renderer reports for element `key`.
fn remembered(renderer: &Renderer, key: u64) -> Option<(bool, [Option<f32>; 2])> {
    renderer.content_visibility_state(1).into_iter().find(|&(k, ..)| k == key).map(|(_, skipped, size)| (skipped, size))
}

#[test]
fn a_shown_auto_card_keeps_its_size_scrolled_away_without_a_frame() {
    // No rendering update's ResizeObserver step runs (no animation frame or ResizeObserver
    // callbacks: `record_remembered_sizes` is never called). Scrolled away, the card skips its
    // contents and is sized as it was shown (300px tall), not as its fallback (20px), so what
    // follows it does not move (CSS Sizing 4 § 5.1, CSS Contain 2 § 4.1).
    let html = "<!doctype html><style>body{margin:0}.c{content-visibility:auto;contain-intrinsic-size:auto 20px}\
        .g{height:300px}#s{height:20000px}</style><div class=c id=c><div class=g></div></div><div id=s></div><div id=after></div>";
    let (card, after) = (key_of(html, "c"), key_of(html, "after"));
    let tree = live_tree(html);
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let mut renderer = Renderer::new();
    renderer.sync_layout(&snap_at(&tree, 0.0), &fetcher).unwrap();
    assert_eq!(renderer.element_box(1, card).unwrap().height, 300.0, "shown with its contents");
    let below = renderer.element_box(1, after).unwrap().y;
    renderer.sync_layout(&snap_at(&tree, 15_000.0), &fetcher).unwrap();
    let (skipped, size) = remembered(&renderer, card).expect("a tracked auto element");
    assert!(skipped, "scrolled away, it skips its contents");
    assert_eq!(size[1], Some(300.0), "the block size it was shown at, remembered as it started skipping");
    assert!(size[0].is_some_and(|w| w > 0.0), "and its inline size: {size:?}");
    assert_eq!(renderer.element_box(1, card).unwrap().height, 300.0, "sized as remembered, not as its 20px fallback");
    assert_eq!(renderer.element_box(1, after).unwrap().y, below, "what follows it stays put");
    // Back in view and away again: still the size it shows at.
    renderer.sync_layout(&snap_at(&tree, 0.0), &fetcher).unwrap();
    renderer.sync_layout(&snap_at(&tree, 15_000.0), &fetcher).unwrap();
    assert_eq!(renderer.element_box(1, card).unwrap().height, 300.0);
}

#[test]
fn an_element_without_a_box_is_not_remembered_as_zero() {
    // `#hidden` has no box (`display: none` around it): a rendering update leaves it without a
    // remembered size, where `#shown` remembers its own.
    let html = "<!doctype html><style>body{margin:0}.a{contain-intrinsic-size:auto 7px}.g{width:50px;height:30px}</style>\
        <div style=display:none><div class=a id=hidden><div class=g></div></div></div>\
        <div class=a id=shown style=width:max-content><div class=g></div></div>";
    let (hidden, shown) = (key_of(html, "hidden"), key_of(html, "shown"));
    let tree = live_tree(html);
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let mut renderer = Renderer::new();
    renderer.sync_layout(&snap_at(&tree, 0.0), &fetcher).unwrap();
    renderer.record_remembered_sizes(1, true);
    assert_eq!(remembered(&renderer, shown), Some((false, [Some(50.0), Some(30.0)])));
    assert_eq!(remembered(&renderer, hidden), None, "no 0x0 size remembered for a box it does not have");
}

#[test]
fn remembering_sizes_is_linear_work_none_without_auto_and_never_lays_out() {
    // A page without `contain-intrinsic-size: auto`: a rendering update's step costs nothing.
    let plain: String = (0..1_000).map(|_| "<div><p>x</p></div>").collect();
    let tree = live_tree(&format!("<!doctype html>{plain}"));
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let mut renderer = Renderer::new();
    renderer.sync_layout(&snap_at(&tree, 0.0), &fetcher).unwrap();
    assert!(!renderer.has_remembered_size_candidates(1));
    renderer.record_remembered_sizes(1, true);
    renderer.record_remembered_sizes(1, false);
    assert_eq!(renderer.remembered_size_work(1), 0, "no element may remember a size");

    // Cards that do: a bounded number of steps per card (the card and its ancestors).
    const CARDS: u64 = 2_000;
    let cards: String = (0..CARDS).map(|_| "<div class=c><p>x</p></div>").collect();
    let tree = live_tree(&format!(
        "<!doctype html><style>body{{margin:0}} .c{{contain-intrinsic-size:auto 20px;height:20px}}</style>{cards}"
    ));
    let mut renderer = Renderer::new();
    renderer.sync_layout(&snap_at(&tree, 0.0), &fetcher).unwrap();
    assert!(renderer.has_remembered_size_candidates(1));
    let before = renderer.remembered_size_work(1);
    renderer.record_remembered_sizes(1, true);
    let recorded = renderer.remembered_size_work(1) - before;
    assert!(recorded >= CARDS, "each card was looked up: {recorded}");
    assert!(recorded < 8 * CARDS, "{recorded} steps for {CARDS} cards");

    // A stale layout: the step asks for the sizes at the next layout and lays nothing out now.
    let before = renderer.remembered_size_work(1);
    renderer.record_remembered_sizes(1, false);
    assert_eq!(renderer.remembered_size_work(1), before, "nothing recorded (or laid out) at the step");
    renderer.sync_layout(&snap_at(&tree, 10.0), &fetcher).unwrap();
    assert!(renderer.remembered_size_work(1) > before, "recorded at the next layout");
    let after = renderer.remembered_size_work(1);
    renderer.sync_layout(&snap_at(&tree, 20.0), &fetcher).unwrap();
    assert_eq!(renderer.remembered_size_work(1), after, "once: no layout records them unasked");
}
