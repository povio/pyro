//! A non-atomic inline box (a `span` badge, chip or pill set in text) has its
//! margin, border and padding in the line, as Chromium lays it out: its
//! inline-axis sides take room beside its first and last content, its
//! block-axis sides grow its border box without moving the line, and no line
//! breaks between a side and the content next to it. Each line's fragment
//! paints as a box: background, border and radii, with the sides between two
//! lines open.
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

/// `0` is 1ch wide in any font: widths in `ch` make the lines break where the
/// tests say, whatever the font.
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

    /// The border boxes per line: x, y, width, height.
    fn fragments(&self, i: usize) -> Vec<[f64; 4]> {
        let quads = self.renderer.element_quads(1, self.keys[i]).unwrap();
        quads.fragments.iter().map(|q| [q[0], q[1], q[2] - q[0], q[5] - q[1]]).collect()
    }
}

fn lay_out(body: &str, ids: &[&str], scale: f64) -> Laid {
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
        device_scale_factor: scale,
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.sync_layout(&snap, &(Arc::new(NoFetch) as Arc<dyn Fetcher>)).unwrap();
    Laid { renderer, keys }
}

fn render(body: &str, scale: f64) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("{STYLE}{body}"),
        base_url: "http://test/".into(),
        width: 600,
        height: 300,
        device_scale_factor: scale,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// The bounding box (x0, y0, x1, y1, exclusive ends) of the pixels in rows
/// `y0..y1` that `keep` accepts.
fn bounds(img: &RgbaImage, y0: u32, y1: u32, keep: impl Fn([u8; 3]) -> bool) -> Option<[u32; 4]> {
    let mut b: Option<[u32; 4]> = None;
    for y in y0..y1.min(img.height) {
        for x in 0..img.width {
            let i = (y as usize * img.width as usize + x as usize) * 4;
            if keep([img.data[i], img.data[i + 1], img.data[i + 2]]) {
                let b = b.get_or_insert([x, y, x + 1, y + 1]);
                *b = [b[0].min(x), b[1].min(y), b[2].max(x + 1), b[3].max(y + 1)];
            }
        }
    }
    b
}

fn pixel(img: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let i = (y as usize * img.width as usize + x as usize) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const WHITE: [u8; 3] = [255, 255, 255];

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.05
}

#[test]
fn a_spans_margin_border_and_padding_are_in_its_line() {
    for scale in [1.0, 2.0, 3.0] {
        let laid = lay_out(
            "<p>0<span id=plain>000</span><span id=next>0</span></p>
             <p>0<span id=box style='margin:0 5px 0 7px;border:1px solid;padding:2px 8px 4px 6px'>000</span><span id=after>0</span></p>",
            &["plain", "next", "box", "after"],
            scale,
        );
        let (plain, next, boxed, after) = (laid.rect(0), laid.rect(1), laid.rect(2), laid.rect(3));
        // The border box: after the left margin, as wide as the text and both sides.
        assert!(close(boxed[0], plain[0] + 7.0), "scale {scale}: x {boxed:?} vs {plain:?}");
        assert!(close(boxed[2], plain[2] + 1.0 + 6.0 + 8.0 + 1.0), "scale {scale}: width {boxed:?} vs {plain:?}");
        // The block-axis sides grow the box around the content area; the line stays.
        assert!(close(boxed[1] - 40.0, plain[1] - 3.0), "scale {scale}: y {boxed:?} vs {plain:?}");
        assert!(close(boxed[3], plain[3] + 3.0 + 5.0), "scale {scale}: height {boxed:?} vs {plain:?}");
        // What follows starts after the right margin.
        assert!(close(after[0], boxed[0] + boxed[2] + 5.0), "scale {scale}: next {after:?} after {boxed:?}");
        assert!(close(after[0] - next[0], 7.0 + 16.0 + 5.0), "scale {scale}: {after:?} vs {next:?}");
        assert!(close(after[1] - 40.0, next[1]), "scale {scale}: the line's text stays: {after:?} vs {next:?}");
    }
}

#[test]
fn a_side_without_a_border_style_has_no_border() {
    // `border-width` computes to `medium` (3px) whatever the style: it is used only with one.
    let laid = lay_out(
        "<p><span id=plain>000</span></p><p><span id=pad style='padding:1px 6px'>000</span></p>
         <p><span id=left style='border-left:4px solid;padding-left:5px'>000</span></p>",
        &["plain", "pad", "left"],
        1.0,
    );
    let (plain, pad, left) = (laid.rect(0), laid.rect(1), laid.rect(2));
    assert!(close(pad[2], plain[2] + 12.0) && close(pad[3], plain[3] + 2.0), "{pad:?} vs {plain:?}");
    assert!(close(left[2], plain[2] + 9.0) && close(left[3], plain[3]), "{left:?} vs {plain:?}");
}

#[test]
fn no_line_breaks_between_a_side_and_its_content() {
    // 7ch: "0000 " and the 1ch start side fit on the first line, the word after the side does
    // not. The side goes down with its word.
    let laid = lay_out(
        "<p id=p style='width:7ch'>0000 <span id=s style='padding-left:1ch;background:#00f'>0000</span></p>
         <p id=q style='width:7ch'>0000<span id=t style='padding:0 2ch'>00</span></p>
         <p id=r style='width:7ch'>00 <span id=u style='padding-right:3ch'>00</span> 0</p>",
        &["p", "s", "q", "t", "r", "u"],
        1.0,
    );
    let (p, s) = (laid.rect(0), laid.rect(1));
    assert!(close(s[0], p[0]) && s[1] > p[1] + 30.0, "the span starts the second line: {s:?} in {p:?}");
    assert!(close(p[3], 80.0), "two lines: {p:?}");
    // No break opportunity at either side: the line overflows.
    let (q, t) = (laid.rect(2), laid.rect(3));
    assert!(close(q[3], 40.0), "one line: {q:?}");
    assert!(t[0] + t[2] > q[0] + q[2], "overflowing: {t:?} in {q:?}");
    // "00 " + "00" + 3ch = 8ch: the span wraps whole (its end side stays with its word), and
    // the "0" after it (2 + 3 + 1 + 1 = 7ch) fits beside it.
    let (r, u) = (laid.rect(4), laid.rect(5));
    assert!(close(u[0], r[0]) && u[1] > r[1] + 30.0, "the span starts the second line: {u:?} in {r:?}");
    assert!(close(r[3], 80.0), "two lines: {r:?}");
}

#[test]
fn white_space_collapses_through_a_side() {
    // The space inside the start of the span collapses with the one before it, the one after
    // its end with the one inside: both spans are a side, "0", a space and a side.
    let laid = lay_out(
        "<p>0 <span id=s style='padding:0 1ch'> 0 </span> <span id=n>0</span></p>
         <p>0 <span id=t style='padding:0 1ch'>0 </span><span id=m>0</span></p>",
        &["s", "n", "t", "m"],
        1.0,
    );
    let (s, n, t, m) = (laid.rect(0), laid.rect(1), laid.rect(2), laid.rect(3));
    assert!(close(s[0], t[0]) && close(s[2], t[2]), "{s:?} vs {t:?}");
    assert!(close(n[0], m[0]), "{n:?} vs {m:?}");
}

#[test]
fn a_wrapped_span_has_a_box_per_line_with_open_sides_between() {
    // "0000 0000" in 6ch: a line each. Each line's fragment is its word and one side.
    let body = "<p id=p style='width:6ch'><span id=s style='padding:4px 10px;border:3px solid #f00;background:#00f;color:#00f'>0000 0000</span></p>";
    let laid = lay_out(body, &["p", "s"], 1.0);
    let (fragments, union) = (laid.fragments(1), laid.rect(1));
    assert_eq!(fragments.len(), 2, "{fragments:?}");
    let (first, second) = (fragments[0], fragments[1]);
    assert!(close(first[2], second[2]), "a word and a side each: {fragments:?}");
    assert!(close(second[1] - first[1], 40.0) && close(first[3], second[3]), "{fragments:?}");
    assert!(close(union[3], 40.0 + first[3]) && close(union[2], first[2]), "{union:?} of {fragments:?}");

    let img = render(body, 1.0);
    let is = |c: [u8; 3]| move |p: [u8; 3]| p == c;
    for (line, fragment) in fragments.iter().enumerate() {
        let (y0, y1) = (fragment[1].round() as u32, (fragment[1] + fragment[3]).round() as u32);
        let red = bounds(&img, y0, y1, is(RED)).unwrap();
        let blue = bounds(&img, y0, y1, is(BLUE)).unwrap();
        assert_eq!((red[1], red[3]), (y0, y1), "line {line}: the border is the fragment's height");
        assert_eq!((blue[1], blue[3]), (y0 + 3, y1 - 3), "line {line}: the padding box");
        let (x0, x1) = (fragment[0].round() as u32, (fragment[0] + fragment[2]).round() as u32);
        let mid = (y0 + y1) / 2;
        if line == 0 {
            // Closed at the left, open at the right.
            assert_eq!((red[0], blue[0], blue[2]), (x0, x0 + 3, x1), "line 0: {red:?} {blue:?}");
            assert_eq!(pixel(&img, x1 - 1, mid), BLUE, "line 0: no right border");
        } else {
            assert_eq!((blue[0], blue[2], red[2]), (x0, x1 - 3, x1), "line 1: {red:?} {blue:?}");
            assert_eq!(pixel(&img, x0, mid), BLUE, "line 1: no left border");
        }
        assert_eq!(pixel(&img, x1, mid), WHITE, "line {line}: nothing after the fragment");
    }
}

#[test]
fn a_fragment_paints_its_radii_and_snaps_to_css_pixels() {
    for scale in [1.0, 2.0, 3.0] {
        // A fractional position: the box's edges are at whole CSS pixels, as a block's are.
        let body = "<p style='padding-left:10.4px'><span style='padding:2px 8px;border:2px solid #f00;border-radius:10px;background:#00f;color:#00f'>000</span></p>";
        let img = render(body, scale);
        let s = scale as u32;
        let red = bounds(&img, 0, img.height, |p| p == RED).unwrap();
        assert!(red.iter().all(|v| v % s == 0), "scale {scale}: at whole CSS pixels: {red:?}");
        assert_eq!(red[0], 10 * s, "scale {scale}: {red:?}");
        let blue = bounds(&img, 0, img.height, |p| p == BLUE).unwrap();
        assert_eq!(
            [blue[0] - red[0], blue[1] - red[1], red[2] - blue[2], red[3] - blue[3]],
            [2 * s; 4],
            "scale {scale}: the border around the padding box"
        );
        // The corners are round, the sides are not.
        assert_eq!(pixel(&img, red[0], red[1]), WHITE, "scale {scale}: top left corner");
        assert_eq!(pixel(&img, red[2] - 1, red[3] - 1), WHITE, "scale {scale}: bottom right corner");
        assert_eq!(pixel(&img, red[0], (red[1] + red[3]) / 2), RED, "scale {scale}: left side");
        assert_eq!(pixel(&img, (red[0] + red[2]) / 2, red[3] - 1), RED, "scale {scale}: bottom side");
    }
}
