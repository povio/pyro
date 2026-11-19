//! Review findings of the round 6 paint lane: an inline element's `filter: blur()` with a
//! `clip-path` (blitz-paint `draw_inline_effects`), and a box's outline under its clipped
//! positioned children when another child escapes its clip.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        std::thread::spawn(move || done(Err("offline".into())));
    }
}

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff;width:400px;height:300px'>{body}"),
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: usize, y: usize) -> [u8; 3] {
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

/// A wrapping inline with `filter: blur(4px)` and `clip-path: inset(0)` is blurred inside its
/// clip: its background's edge, a pixel inside the box, is lighter than the background. The blur
/// was dropped (the spatial filter path left out elements with a clip path).
#[test]
fn an_inline_blur_with_a_clip_path_blurs() {
    let span = "<span style='background:#008000;filter:blur(4px);clip-path:inset(0);font:20px/40px monospace;line-height:40px'>xxxxxxxxxx</span>";
    let img = render(&format!("<div style='font:20px/40px monospace'>{span}</div>"));
    let unblurred = render(&format!("<div style='font:20px/40px monospace'>{}</div>", span.replace("filter:blur(4px);", "")));
    // The background's left edge: x = 0 is the box's first column.
    let (blurred_edge, sharp_edge) = (pixel(&img, 0, 20), pixel(&unblurred, 0, 20));
    assert_ne!(blurred_edge, sharp_edge, "the blur softens the edge: {blurred_edge:?} vs {sharp_edge:?}");
}

/// Renders `html` as a live page (its render tree from pyro-dom, as `pyro serve` paints pages).
fn render_live(html: &str) -> RgbaImage {
    use pyro_dom::snapshot::{RenderHtml, RenderNodeKind};
    let dom: pyro_dom::parse::DomRef = std::rc::Rc::new(std::cell::RefCell::new(pyro_dom::Dom::new()));
    let doc = pyro_dom::parse::parse_html_unsafe(&dom, html);
    let d = dom.borrow();
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
    let snap = Snapshot {
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
        device_scale_factor: 1.0,
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
        ..Default::default()
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let mut renderer = Renderer::new();
    // Twice, as a live page is painted again: the second frame keeps the first's paint tree.
    let first = renderer.paint(&snap, Area::Viewport, &fetcher).unwrap();
    drop(renderer.raster(first, Backend::Cpu));
    let painted = renderer.paint(&snap, Area::Viewport, &fetcher).unwrap();
    renderer.raster(painted, Backend::Cpu).0
}

/// A box's inset outline (a negative `outline-offset`) goes under its positioned children, also
/// when another child (fixed, its containing block the viewport) escapes the clip of the box (a
/// stacking context): the escaping child turned the outline's deferral off for the whole box, so
/// the clipped positioned child painted under the outline.
#[test]
fn an_inset_outline_goes_under_clipped_positioned_children_beside_an_escaping_one() {
    let page = |fixed: &str| {
        format!(
            "<!doctype html><body style='margin:0'>\
             <div style='position:relative;z-index:0;overflow:hidden;width:100px;height:100px;outline:10px solid #00f;outline-offset:-10px'>\
             <div style='position:absolute;left:0;top:0;width:50px;height:50px;background:#008000'></div>{fixed}</div>"
        )
    };
    let alone = render_live(&page(""));
    assert_eq!(pixel(&alone, 5, 5), [0, 128, 0], "over the outline");
    let beside = render_live(&page("<div style='position:fixed;left:300px;top:200px;width:20px;height:20px;background:#f00'></div>"));
    assert_eq!(pixel(&beside, 5, 5), [0, 128, 0], "over the outline, beside a fixed child");
    assert_eq!(pixel(&beside, 310, 210), [255, 0, 0], "the fixed child, outside the clip");
    assert_eq!(pixel(&beside, 95, 95), [0, 0, 255], "the outline");
}

