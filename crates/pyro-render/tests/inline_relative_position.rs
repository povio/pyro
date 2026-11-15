//! `position: relative` on an inline box that is not atomic (CSS 2.1 § 9.4.3) moves what it holds:
//! its text, its fragments' boxes and the atomic inline boxes in it, without moving the line or
//! anything around it.
use std::{cell::RefCell, rc::Rc, sync::Arc};

use pyro_dom::{
    parse::{parse_document, DomRef},
    snapshot::{render_key, RenderHtml, RenderNodeKind},
};
use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

const STYLE: &str = "<!doctype html><style>body{margin:0;background:white;font:20px/40px monospace;color:black}
  p{margin:0} b{display:inline-block;width:10px;height:10px}</style>";

fn rects(body: &str, ids: &[&str]) -> Vec<[f64; 2]> {
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", &format!("{STYLE}{body}"));
    let d = dom.borrow();
    let keys: Vec<_> = ids.iter().map(|id| render_key(d.get_element_by_id(doc, id).unwrap().unwrap())).collect();
    let tree =
        d.render_tree(doc, &RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None }).unwrap();
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
    let snap = Snapshot {
        base_url: "http://test/".into(),
        width: 600,
        height: 300,
        device_scale_factor: 1.0,
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.sync_layout(&snap, &(Arc::new(NoFetch) as Arc<dyn Fetcher>)).unwrap();
    keys.iter()
        .map(|&key| {
            let b = renderer.element_box(1, key).unwrap();
            [b.x, b.y]
        })
        .collect()
}

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("{STYLE}{body}"),
        base_url: "http://test/".into(),
        width: 600,
        height: 300,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// The columns of the image that have a pixel of `color`.
fn columns(img: &RgbaImage, color: [u8; 3]) -> Vec<u32> {
    (0..img.width)
        .filter(|&x| {
            (0..img.height).any(|y| {
                let i = (y as usize * img.width as usize + x as usize) * 4;
                [img.data[i], img.data[i + 1], img.data[i + 2]] == color
            })
        })
        .collect()
}

#[test]
fn atomic_boxes_in_a_relatively_positioned_span_move_with_it() {
    let at = rects(
        "<p><b id=a></b><span style='position:relative;left:30px;top:7px'><span><b id=b></b></span></span><b id=c></b></p>",
        &["a", "b", "c"],
    );
    assert_eq!(at[1][0] - at[0][0], 40.0, "{at:?}");
    assert_eq!(at[1][1] - at[0][1], 7.0, "{at:?}");
    assert_eq!(at[2][0] - at[0][0], 20.0, "what follows stays: {at:?}");
}

#[test]
fn a_relatively_positioned_spans_text_and_box_move() {
    // The span's background is blue: it starts 60px right of where its line puts it.
    let still = render("<p>00<span style='background:#00f'>00</span></p>");
    let moved = render("<p>00<span style='background:#00f;position:relative;left:60px'>00</span></p>");
    let (still, moved) = (columns(&still, [0, 0, 255]), columns(&moved, [0, 0, 255]));
    assert!(!still.is_empty(), "the span paints");
    assert_eq!(moved.first().map(|x| x - 60), still.first().copied(), "{still:?} {moved:?}");
    assert_eq!(moved.len(), still.len());
}
