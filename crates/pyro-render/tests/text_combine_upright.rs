//! `text-combine-upright: all` in a vertical line combines its text into one upright 1em square
//! (CSS Writing Modes 4 § 9.1.1): a box 1em by 1em, its text compressed to fit, and the line around
//! it advances by 1em. Not where the text is emphasized, nor in horizontal writing.
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

fn fetcher() -> Arc<dyn Fetcher> {
    Arc::new(NoFetch)
}

fn boxes(html: &str, ids: &[&str]) -> Vec<[f64; 4]> {
    use_fork_safe_fonts();
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", html);
    let d = dom.borrow();
    let keys: Vec<_> = ids.iter().map(|id| render_key(d.get_element_by_id(doc, id).unwrap().unwrap())).collect();
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
        width: 800,
        height: 600,
        device_scale_factor: 1.0,
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.sync_layout(&snap, &fetcher()).unwrap();
    keys.into_iter()
        .map(|key| {
            let b = renderer.element_box(1, key).unwrap();
            [b.x, b.y, b.x + b.width, b.y + b.height]
        })
        .collect()
}


/// The bounding box of the page's dark pixels.
fn ink(html: &str) -> [u32; 4] {
    use_fork_safe_fonts();
    let snap = Snapshot { html: html.into(), base_url: "http://test/".into(), width: 400, height: 400, device_scale_factor: 1.0, ..Default::default() };
    let (img, _, _) = Renderer::new().render(&snap, Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    let mut r = [u32::MAX, u32::MAX, 0, 0];
    for y in 0..img.height {
        for x in 0..img.width {
            if img.data[(y as usize * img.width as usize + x as usize) * 4] < 128 {
                r = [r[0].min(x), r[1].min(y), r[2].max(x + 1), r[3].max(y + 1)];
            }
        }
    }
    r
}

const PAGE: &str = "<!doctype html><style>body{margin:0}div{writing-mode:vertical-rl;font:40px/1 monospace}\
                    .t{text-combine-upright:all}</style>";

#[test]
fn combined_text_is_a_one_em_square() {
    let b = boxes(&format!("{PAGE}<div>x<span class=t id=t>1234</span>x</div>"), &["t"]);
    assert_eq!([b[0][2] - b[0][0], b[0][3] - b[0][1]], [40.0, 40.0], "{:?}", b[0]);
    // Compressed: its four digits paint within the square's width.
    let ink = ink(&format!("{PAGE}<div><span class=t>1234</span></div>"));
    assert!(ink[2] - ink[0] <= 40, "{ink:?}");
}

#[test]
fn emphasized_or_horizontal_text_is_not_combined() {
    let b = boxes(&format!("{PAGE}<div><span class=t id=t style='text-emphasis:filled'>1234</span></div>"), &["t"]);
    assert!(b[0][3] - b[0][1] > 60.0, "{:?}", b[0]);
    let b = boxes(&format!("{PAGE}<p style='font:40px monospace'><span class=t id=t>1234</span></p>"), &["t"]);
    assert!(b[0][2] - b[0][0] > 80.0, "{:?}", b[0]);
}
