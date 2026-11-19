//! An inline box after an atomic inline on its line paints over it where they meet (CSS 2.1
//! Appendix E: a line's inline-level boxes paint in tree order), and finding those pairs costs
//! linear time: a long line of alternating inline-blocks and bordered spans paints quickly.
use std::{cell::RefCell, rc::Rc, sync::Arc, time::Instant};

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

fn render(html: &str, width: u32, height: u32) -> RgbaImage {
    use_fork_safe_fonts();
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", html);
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
        width,
        height,
        device_scale_factor: 1.0,
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
        ..Default::default()
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let mut renderer = Renderer::new();
    renderer.render(&snap, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn rgb(image: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * image.width + x) * 4) as usize;
    [image.data[i], image.data[i + 1], image.data[i + 2]]
}

#[test]
fn a_box_after_an_atomic_inline_paints_over_it_and_one_before_does_not() {
    let image = render(
        "<!doctype html><body style='margin:0;background:white;font:20px/40px monospace'>\
         <div><span style='background:blue'>xx</span><span style='display:inline-block;width:40px;height:30px;background:red;margin-left:-20px'></span>\
         <span style='background:lime;margin-left:-20px'>xx</span></div>",
        200,
        60,
    );
    // Where the lime box was pulled back over the red inline-block it is lime; where the
    // inline-block was pulled back over the blue box before it, red.
    let red_rows: Vec<u32> = (0..60).filter(|&y| (0..200).any(|x| rgb(&image, x, y) == [255, 0, 0])).collect();
    let y = red_rows[red_rows.len() / 2];
    let row: Vec<[u8; 3]> = (0..200).map(|x| rgb(&image, x, y)).collect();
    let red = row.iter().position(|&p| p == [255, 0, 0]).expect("the inline-block is painted");
    let lime = row.iter().position(|&p| p == [0, 255, 0]).expect("the box after it is painted");
    assert!(red < lime, "{row:?}");
    let blue_end = row.iter().rposition(|&p| p == [0, 0, 255]).expect("the box before is painted");
    assert!(blue_end < red, "the inline-block covers the box before it: {row:?}");
    let red_end = row.iter().rposition(|&p| p == [255, 0, 0]).unwrap();
    assert!(red_end < lime, "the box after covers the inline-block: {row:?}");
}

#[test]
fn thousands_of_alternating_atomic_inlines_and_boxes_paint_in_linear_time() {
    let item = "<i style='display:inline-block;width:6px;height:6px;background:red'></i>\
                <b style='border:1px solid blue;margin-left:-2px'>x</b>";
    for nowrap in [true, false] {
        let html = format!(
            "<!doctype html><body style='margin:0;font:8px/10px monospace'><div style='white-space:{}'>{}</div>",
            if nowrap { "nowrap" } else { "normal" },
            item.repeat(6000)
        );
        let start = Instant::now();
        render(&html, 800, 600);
        let took = start.elapsed();
        // The pairwise search took over 20 s here for half as many (quadratic wrapped, cubic on
        // one line); a linear one takes well under a second.
        assert!(took.as_secs() < 10, "nowrap {nowrap}: {took:?}");
    }
}
