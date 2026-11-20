//! `content-visibility: auto` determines relevance in work linear in the number of tracked
//! elements: a page of thousands of auto cards scrolled (none of them changing) costs a bounded
//! number of steps per card, counted by `Renderer::content_visibility_work`.
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
