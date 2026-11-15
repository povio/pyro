//! An inline box that holds block-level boxes is split around them (CSS 2.1 § 9.2.1.1): the
//! inline content before, between and after the blocks is in anonymous blocks, each a piece of the
//! inline box. Its start side (inline-axis margin, border and padding) is in its first piece only
//! and its end side in its last, also when a piece is empty; the blocks are not inside its margin,
//! border or padding and it paints no box around them; margins collapse through empty pieces; its
//! relative offset moves the blocks too. A nested split inline's head and tail share the outer
//! one's lines; a float before the first block stays on the line before it, unless nothing is
//! there; an inline box in a table part laid out outside a table is not split. Also: an empty
//! inline box's line height counts, and an inline-block's baseline is its last line's.
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

/// `0` is 1ch wide in any font.
const STYLE: &str = "<!doctype html><style>body{margin:0;background:white;font:20px/40px monospace;color:black}
  p{margin:0}</style>";

struct Laid {
    renderer: Renderer,
    keys: Vec<u64>,
}

impl Laid {
    fn rect(&self, i: usize) -> [f64; 4] {
        let b = self.renderer.element_box(1, self.keys[i]).unwrap();
        [b.x, b.y, b.width, b.height]
    }
}

fn lay_out(body: &str, ids: &[&str]) -> Laid {
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
    Laid { renderer, keys }
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

fn pixel(img: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let i = (y as usize * img.width as usize + x as usize) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

/// The columns of row `y` whose pixel is `color`.
fn columns(img: &RgbaImage, y: u32, color: [u8; 3]) -> Vec<u32> {
    (0..img.width).filter(|&x| pixel(img, x, y) == color).collect()
}

const BLUE: [u8; 3] = [0, 0, 255];

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.05
}

#[test]
fn the_blocks_are_outside_the_split_inlines_sides() {
    // The span's margin, border and padding are its pieces' sides, not around the block.
    let laid = lay_out(
        "<span id=s style='margin:0 30px;border:0 solid;border-width:0 5px;padding:4px 20px'>0<div id=d>00</div>0</span>",
        &["d"],
    );
    let d = laid.rect(0);
    assert!(close(d[0], 0.0), "the block starts at the container's edge: {d:?}");
    assert!(close(d[2], 600.0), "the block is as wide as the container: {d:?}");
    assert!(close(d[1], 40.0), "the block is below the first piece's line: {d:?}");
}

#[test]
fn the_start_side_is_in_the_first_piece_and_the_end_side_in_the_last() {
    // Lines of 40px: the first piece's line, the block, the last piece's line.
    let img = render(
        "<span style='border:0 solid blue;border-width:0 10px'>00<div style='width:100px'>00</div>00</span>",
    );
    // First line: a left border at 0..10, the text, no right border after it.
    let first = columns(&img, 20, BLUE);
    assert_eq!(first, (0..10).collect::<Vec<_>>(), "first piece: {first:?}");
    // The block's line: no border at all.
    assert!(columns(&img, 60, BLUE).is_empty(), "no border beside the block");
    // Last line: no left border, a right border after the two characters (24px each at 20px
    // monospace is font dependent, so only the count and the start are checked).
    let last = columns(&img, 100, BLUE);
    assert_eq!(last.len(), 10, "last piece: {last:?}");
    assert!(last[0] > 10, "the right border follows the text: {last:?}");
}

#[test]
fn an_empty_piece_with_a_side_has_a_line_and_one_without_has_none() {
    // A leading block: the first piece is empty. With a start side it has a strut-high line; the
    // end side here has no width, so the last piece has no line.
    let laid = lay_out(
        "<div><span style='border-left:4px solid blue'><div id=a>0</div></span><div id=b>0</div></div>
         <div><span><div id=c>0</div></span><div id=e>0</div></div>",
        &["a", "b", "c", "e"],
    );
    let (a, b, c, e) = (laid.rect(0), laid.rect(1), laid.rect(2), laid.rect(3));
    assert!(close(a[1], 40.0), "a line before the first block: {a:?}");
    assert!(close(b[1], a[1] + a[3]), "no line after it: {b:?} after {a:?}");
    assert!(close(c[1], b[1] + b[3]), "no line before the second block: {c:?}");
    assert!(close(e[1], c[1] + c[3]), "nor after it: {e:?}");
}

#[test]
fn margins_collapse_through_empty_pieces() {
    let laid = lay_out(
        "<div id=a style='height:10px;margin-bottom:40px'></div>
         <span><div id=b style='height:10px;margin-top:30px;margin-bottom:20px'></div></span>
         <div id=c style='height:10px;margin-top:25px'></div>",
        &["a", "b", "c"],
    );
    let (a, b, c) = (laid.rect(0), laid.rect(1), laid.rect(2));
    assert!(close(b[1] - (a[1] + a[3]), 40.0), "{a:?} {b:?}");
    assert!(close(c[1] - (b[1] + b[3]), 25.0), "{b:?} {c:?}");
}

#[test]
fn a_relatively_positioned_split_inline_moves_its_blocks() {
    let laid = lay_out(
        "<span style='position:relative;left:30px;top:5px'>0<div id=d style='width:50px'>0</div>0</span>",
        &["d"],
    );
    let d = laid.rect(0);
    assert!(close(d[0], 30.0) && close(d[1], 45.0), "{d:?}");
}

#[test]
fn a_split_inline_paints_no_box_around_its_blocks() {
    let img = render("<span style='background:blue'>0<div style='height:40px'></div>0</span>");
    assert!(columns(&img, 60, BLUE).is_empty(), "nothing blue beside the block");
    assert!(!columns(&img, 20, BLUE).is_empty(), "the first piece's fragment is blue");
}

#[test]
fn a_nested_split_inlines_head_and_tail_share_the_outer_ones_lines() {
    // "ab" on one line, the block, "cd" on one line (Chrome: 40 and 120).
    let laid = lay_out("<span>a<span>b<div id=d>X</div>c</span>d</span><p id=n>0</p>", &["d", "n"]);
    let (d, n) = (laid.rect(0), laid.rect(1));
    assert!(close(d[1], 40.0), "{d:?}");
    assert!(close(n[1], 120.0), "{n:?}");
}

#[test]
fn a_float_before_the_first_block_stays_on_the_line_before_it() {
    // After text, the float is on the text's line (Chrome: 0); with nothing before it, it floats
    // beside the block and paints over its background (WPT block-in-inline-margins-004).
    let laid = lay_out(
        "<span>0<div id=f style='float:left;width:20px;height:20px'></div><div id=d style='height:30px'></div>0</span>",
        &["f", "d"],
    );
    let (f, d) = (laid.rect(0), laid.rect(1));
    assert!(close(f[1], 0.0) && close(d[1], 40.0), "{f:?} {d:?}");
    let img = render(
        "<div style='width:100px'><span> <div style='float:left;width:10px;height:10px;background:blue'></div>
         <div style='height:100px;background:#f80'></div></span></div>",
    );
    assert_eq!(pixel(&img, 5, 5), BLUE, "the float paints over the block");
}

#[test]
fn an_inline_box_in_a_table_row_outside_a_table_is_not_split() {
    // The row is laid out as a flex row: split, the pieces were laid out side by side.
    let laid = lay_out("<div style='display:table-row'><span>0<div id=d>0</div>0</span></div>", &["d"]);
    let d = laid.rect(0);
    assert!(close(d[0], 0.0) && close(d[1], 40.0), "{d:?}");
}

#[test]
fn an_empty_inline_boxs_line_height_counts() {
    // The line is 50px high, as its empty span's line height (Chrome: 50).
    let laid = lay_out("<p style='line-height:10px'>0<span style='line-height:50px'></span></p><p id=n>0</p>", &["n"]);
    let n = laid.rect(0);
    assert!(close(n[1], 50.0), "{n:?}");
}

#[test]
fn an_inline_blocks_baseline_is_its_last_lines() {
    // The inline-block's two lines are in blocks: the 10px box after it sits on the second line's
    // baseline (Chrome: 57), not the first's.
    let laid = lay_out(
        "<div><span style='display:inline-block'><div>0</div><div>0</div></span><span id=x style='display:inline-block;width:10px;height:10px'></span></div>",
        &["x"],
    );
    let x = laid.rect(0);
    assert!(x[1] > 40.0, "{x:?}");
}

#[test]
fn what_a_split_inlines_head_and_tail_hold_have_their_boxes() {
    // The head's line, the block, the tail's line: an inline element in the tail has its box
    // there (it was looked for in the head's anonymous block only: an empty box at the block).
    let laid = lay_out("<span>0<i id=h>0</i><div id=d>00</div>0<b id=t>0</b></span>", &["h", "d", "t"]);
    let (h, d, t) = (laid.rect(0), laid.rect(1), laid.rect(2));
    assert!(close(d[1], 40.0) && close(d[3], 40.0), "{d:?}");
    assert!(h[1] < 40.0 && h[2] > 0.0, "{h:?}");
    assert!(t[1] >= 80.0 && t[2] > 0.0 && t[3] > 0.0, "{t:?}");
}
