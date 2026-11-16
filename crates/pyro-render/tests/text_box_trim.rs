//! `text-box-trim` / `text-box-edge` (CSS Inline 3 § 5): a block container's first and last
//! formatted lines lose what is over and under the edge metric; and what the WPT pages for it
//! needed besides: `<br>` with `clear`, relatively positioned inline boxes in vertical lines, and
//! stacking contexts of negative z-index under their parent's text.
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

const STYLE: &str = "<!doctype html><style>body{margin:0;background:white;font:20px/60px monospace;color:black}
  div{margin:0} b{display:block;height:10px}</style>";

/// The top of each element `ids` names.
fn tops(body: &str, ids: &[&str]) -> Vec<f64> {
    boxes(body, ids).into_iter().map(|at| at[1]).collect()
}

fn boxes(body: &str, ids: &[&str]) -> Vec<[f64; 2]> {
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

/// The rows of the image that have a pixel of `color`.
fn rows(img: &RgbaImage, color: [u8; 3]) -> Vec<u32> {
    (0..img.height)
        .filter(|&y| {
            (0..img.width).any(|x| {
                let i = (y as usize * img.width as usize + x as usize) * 4;
                [img.data[i], img.data[i + 1], img.data[i + 2]] == color
            })
        })
        .collect()
}

/// Where the block after a one-line block with `style` starts.
fn after(style: &str) -> f64 {
    tops(&format!("<div style='{style}'>x</div><b id=b></b>"), &["b"])[0]
}

#[test]
fn a_block_loses_the_half_leading_of_its_trimmed_sides() {
    let none = after("");
    let start = after("text-box-trim:trim-start;text-box-edge:text");
    let end = after("text-box-trim:trim-end;text-box-edge:text");
    let both = after("text-box-trim:trim-both;text-box-edge:text");
    assert_eq!(none, 60.0);
    assert!(start < 60.0 && start > 30.0, "{start}");
    assert!(end < 60.0 && (start - end).abs() <= 1.0, "half the leading on each side: {start} {end}");
    assert_eq!(60.0 - both, (60.0 - start) + (60.0 - end));
    // The shorthand, and `auto` as `text`.
    assert_eq!(after("text-box:trim-both text"), both);
    assert_eq!(after("text-box-trim:trim-both"), both);
    // The alphabetic baseline is higher than the descent, the cap height lower than the ascent.
    assert!(after("text-box:trim-end text alphabetic") < end);
    assert!(after("text-box:trim-start cap text") <= start);
    assert!(after("text-box:trim-start ex text") < start);
}

#[test]
fn the_text_moves_up_with_a_trimmed_start() {
    let plain = render("<div>x</div>");
    let trimmed = render("<div style='text-box:trim-start text'>x</div>");
    let (plain, trimmed) = (rows(&plain, [0, 0, 0]), rows(&trimmed, [0, 0, 0]));
    assert!(!plain.is_empty() && plain.len() == trimmed.len());
    assert!(trimmed[0] < plain[0], "{} {}", trimmed[0], plain[0]);
}

#[test]
fn the_first_and_last_lines_of_nested_blocks_are_trimmed() {
    let both = after("text-box-trim:trim-both;text-box-edge:text");
    let nested =
        tops("<div style='text-box-trim:trim-both'><div><div>x</div></div></div><b id=b></b>", &["b"])[0];
    assert_eq!(nested, both);
    // Three lines in three blocks: the outer two sides only.
    let lines = tops(
        "<div style='text-box-trim:trim-both'><div>x</div><div>x</div><div>x</div></div><b id=b></b>",
        &["b"],
    )[0];
    assert_eq!(lines, both + 120.0);
    // A child with padding or a border on a side keeps its line on that side (Chromium).
    let padded = tops(
        "<div style='text-box-trim:trim-both'><div style='padding-top:1px'>x</div></div><b id=b></b>",
        &["b"],
    )[0];
    assert_eq!(padded, after("text-box-trim:trim-end;text-box-edge:text") + 1.0);
    // The box that trims may have them.
    let own = tops("<div style='text-box-trim:trim-both;padding:5px 0'>x</div><b id=b></b>", &["b"])[0];
    assert_eq!(own, both + 10.0);
}

#[test]
fn a_br_with_clear_ends_its_line_below_the_floats() {
    let at = tops(
        "<div style='float:left;width:10px;height:150px'></div><div>x<br style='clear:both'>y</div><b id=b></b>",
        &["b"],
    );
    assert_eq!(at[0], 210.0, "the second line starts under the float");
    // Without a line after it, the block still ends under the float.
    let at = tops(
        "<div style='float:left;width:10px;height:150px'></div><div>x<br style='clear:both'></div><b id=b></b>",
        &["b"],
    );
    assert_eq!(at[0], 150.0);
    // A trimmed last line does not end above the float it cleared.
    let at = tops(
        "<div style='float:left;width:10px;height:55px'></div>
         <div style='text-box:trim-end text'>x<br style='clear:both'></div><b id=b></b>",
        &["b"],
    );
    assert_eq!(at[0], 55.0);
}

#[test]
fn a_negative_z_index_box_is_painted_under_its_stacking_contexts_text() {
    let img = render(
        "<div style='position:relative;z-index:0;font:60px/60px monospace'>MMMM<div
           style='position:absolute;z-index:-1;inset:0;background:#0f0'></div></div>",
    );
    assert!(!rows(&img, [0, 255, 0]).is_empty(), "the box paints");
    assert!(!rows(&img, [0, 0, 0]).is_empty(), "the text is over it");
}

#[test]
fn a_spans_border_is_painted_over_the_next_blocks_background() {
    let img = render(
        "<div><span style='border-bottom:30px solid #00f'>x</span></div><div style='background:#0f0;height:60px'></div>",
    );
    let blue = rows(&img, [0, 0, 255]);
    assert!(blue.iter().any(|&y| y >= 60), "{blue:?}");
}

#[test]
fn a_relatively_positioned_span_moves_its_text_in_vertical_lines() {
    let still = render("<div style='writing-mode:vertical-rl'>00<span style='color:#00f'>00</span></div>");
    let moved = render(
        "<div style='writing-mode:vertical-rl'>00<span style='color:#00f;position:relative;top:50px'>00</span></div>",
    );
    let (still, moved) = (rows(&still, [0, 0, 255]), rows(&moved, [0, 0, 255]));
    assert!(!still.is_empty(), "the span paints");
    assert_eq!(moved.first().map(|y| y - 50), still.first().copied(), "{still:?} {moved:?}");
}
