//! Two css-writing-modes behaviours (WPT `vertical-alignment-*`, `sizing-orthog*`):
//! - In vertical lines a span aligned by its box's edge (`vertical-align: top | bottom | text-top |
//!   text-bottom`) keeps the place the line gives it instead of being centred on the line's central
//!   baseline; a `middle` span is centred half the parent's x-height over that baseline.
//! - An orthogonal box's percentages in its inline axis resolve against the initial containing block
//!   when the containing block's size in that axis is indefinite, and its auto inline size is the
//!   available size less its margins in that axis.
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

/// The horizontal extent (left, right) of the pixels painted pure red.
fn red_columns(html: &str) -> (u32, u32) {
    use_fork_safe_fonts();
    let snap = Snapshot {
        html: html.into(),
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let (img, _, _) = Renderer::new().render(&snap, Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    let (mut left, mut right) = (u32::MAX, 0);
    for y in 0..img.height {
        for x in 0..img.width {
            let p = &img.data[(y as usize * img.width as usize + x as usize) * 4..][..3];
            if p[0] > 200 && p[1] < 60 && p[2] < 60 {
                left = left.min(x);
                right = right.max(x + 1);
            }
        }
    }
    (left, right)
}

fn line(align: &str, mode: &str) -> String {
    format!(
        "<!doctype html><style>body{{margin:0}}div{{writing-mode:{mode};font:40px/3 monospace;color:#ccc}}\
         span{{font-size:10px;color:red;vertical-align:{align}}}</style><div>MM<span>MM</span></div>"
    )
}

#[test]
fn an_edge_aligned_span_goes_to_its_edge_of_a_vertical_line() {
    for mode in ["vertical-rl", "vertical-lr"] {
        let (base_left, base_right) = red_columns(&line("baseline", mode));
        // The line is 120px wide: `top` puts the span against its line-over (right) edge, `bottom`
        // against its line-under (left) edge, well away from the centred text.
        let (_, top_right) = red_columns(&line("top", mode));
        let (bottom_left, _) = red_columns(&line("bottom", mode));
        assert!(top_right > base_right + 20, "{mode}: top {top_right} vs baseline {base_right}");
        assert!(bottom_left + 20 < base_left, "{mode}: bottom {bottom_left} vs baseline {base_left}");
    }
}

#[test]
fn a_middle_span_sits_over_the_central_baseline_of_a_vertical_line() {
    let (base_left, base_right) = red_columns(&line("baseline", "vertical-rl"));
    let (mid_left, mid_right) = red_columns(&line("middle", "vertical-rl"));
    // Half the parent's x-height (about 10px at 40px) toward line-over (right).
    assert!(mid_left > base_left && mid_right > base_right, "middle {mid_left}..{mid_right} vs {base_left}..{base_right}");
}

#[test]
fn an_orthogonal_percentage_of_an_indefinite_size_is_of_the_initial_containing_block() {
    let b = boxes(
        "<!doctype html><style>html{writing-mode:vertical-lr}body{margin:0}\
         #o{writing-mode:horizontal-tb;width:50%;font:10px monospace}</style>\
         <div><div id=o>xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx \
         xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx</div></div>",
        &["o"],
    );
    assert_eq!(b[0][2] - b[0][0], 400.0, "{:?}", b[0]);
}

#[test]
fn an_orthogonal_auto_inline_size_leaves_room_for_its_margins() {
    let b = boxes(
        "<!doctype html><style>body{margin:0}#c{width:500px;height:200px}\
         #o{writing-mode:vertical-lr;margin:10%;font:25px/1 monospace}</style>\
         <div id=c><div id=o>xx x x x xx x x xx x xx</div></div>",
        &["o"],
    );
    // 10% of the containing block's inline size (500px) is 50px on each side: 200 - 100.
    assert_eq!(b[0][3] - b[0][1], 100.0, "{:?}", b[0]);
    assert_eq!(b[0][1], 50.0, "{:?}", b[0]);
}
