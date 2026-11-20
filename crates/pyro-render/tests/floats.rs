//! Floats (reftest lane P3): their paint order (CSS 2.1 Appendix E: block backgrounds, then the
//! floats, then inline content, for the whole normal flow of a stacking context), the placement of
//! boxes that avoid them (tables and formatting contexts go to the first band they fit in, beside
//! the floats of their whole height; zero-width floats; padding and border count), clearance,
//! block-level replaced elements beside floats, and floats in inline
//! formatting contexts (content that doesn't fit beside a leading float moves below it, a float
//! met past where a line wrapped goes below the next line, inline boxes beside a float paint).
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

const STYLE: &str = "<!doctype html><style>body{margin:0;background:white;font:20px/20px monospace;color:black}
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

fn dark_pixels(img: &RgbaImage, x0: u32, x1: u32, y0: u32, y1: u32) -> usize {
    let mut n = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let [r, g, b] = pixel(img, x, y);
            if r < 100 && g < 100 && b < 100 {
                n += 1;
            }
        }
    }
    n
}

const LIME: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.05
}

#[test]
fn a_float_paints_over_the_background_of_a_later_block() {
    // The float is in the first block's line, the red block after that block overlaps it: block
    // backgrounds of the whole flow paint before the floats (WPT stack-floats, overhanging-float).
    let img = render(
        "<div style='width:100px'><div>0<span style='float:left;width:100px;height:100px;background:#0f0'></span></div>
         <div style='height:100px;background:red'></div></div>",
    );
    assert_eq!(pixel(&img, 50, 60), LIME, "the float paints over the later block");
}

#[test]
fn a_float_paints_over_a_block_in_a_split_inline() {
    // An anonymous block holds the float, a split inline the red block (WPT stack-floats-001).
    let img = render(
        "<div style='width:100px'>0<span style='float:left;width:100px;height:100px;background:#0f0'></span>
         <span><div style='height:100px;background:red'></div></span></div>",
    );
    assert_eq!(pixel(&img, 50, 60), LIME, "the float paints over the block");
}

#[test]
fn inline_content_paints_over_a_float() {
    // The float takes no room (negative margin): the text over it paints after it (WPT floats-132).
    let img = render(
        "<p style='width:200px'><span style='float:left;width:200px;height:20px;margin-right:-200px;background:#0f0'></span>0000</p>",
    );
    assert!(dark_pixels(&img, 0, 80, 0, 20) > 0, "the text is over the float");
}

#[test]
fn the_outlines_of_a_flow_paint_over_its_inline_content() {
    // With the flow painted in phases (it has a float), the block's outline paints after the
    // inline-block on the next line, as Chromium has it (WPT z-index-020).
    let img = render(
        "<div style='position:relative;width:200px'><div style='float:right;width:10px;height:10px'></div>
         <div style='height:20px;outline:5px solid blue'></div><span style='display:inline-block;width:100px;height:50px;background:#0f0'></span></div>",
    );
    assert_eq!(pixel(&img, 50, 22), BLUE, "the outline over the inline-block");
}

#[test]
fn a_float_in_a_transparent_inline_is_transparent() {
    // The span's opacity applies to the float it holds (WPT inline-opacity-float-child).
    let img = render(
        "<div style='width:100px;height:100px;background:#0f0'><span style='opacity:0'>
         <div style='float:left;width:100px;height:100px;background:red'></div></span></div>",
    );
    assert_eq!(pixel(&img, 50, 50), LIME, "the float is not painted");
}

#[test]
fn a_table_beside_floats_goes_to_the_first_band_it_fits_in() {
    // Beside both floats there are 100px, and the table is 150px wide: it goes to the band below
    // the short float, beside the tall one (WPT floats-wrap-bfc-004).
    let laid = lay_out(
        "<div style='width:300px'><div style='float:left;width:100px;height:20px'></div>
         <div style='float:left;width:100px;height:6px'></div>
         <table id=t style='border-spacing:0'><tr><td style='padding:0'><div style='width:150px;height:10px'></div></td></tr></table></div>",
        &["t"],
    );
    let t = laid.rect(0);
    assert!(close(t[0], 100.0) && close(t[1], 6.0), "{t:?}");
}

#[test]
fn a_formatting_context_taller_than_its_band_moves_beside_the_floats_of_its_height() {
    // Beside the first float the box is 200px wide and 50px tall, which reaches the second float:
    // it is placed beside both, 100px wide, and is then 100px tall (WPT floats-bfc-003).
    let laid = lay_out(
        "<div style='width:300px'><div style='float:left;clear:left;width:100px;height:25px'></div>
         <div style='float:left;clear:left;width:200px;height:25px'></div>
         <div id=b style='overflow:hidden'><div style='float:left;width:100px;height:50px'></div>
         <div style='float:left;width:100px;height:50px'></div></div></div>",
        &["b"],
    );
    let b = laid.rect(0);
    assert!(close(b[0], 200.0) && close(b[1], 0.0) && close(b[2], 100.0) && close(b[3], 100.0), "{b:?}");
}

#[test]
fn zero_width_floats_leave_a_formatting_context_its_margins() {
    // Neither float reaches past the containing block's edges: the negative margins apply (WPT
    // zero-width-floats). With a float that does, a zero-width float absorbs its side's margin
    // (WPT zero-width-floats-positioning).
    let laid = lay_out(
        "<div style='margin-left:50px;width:100px'><div style='float:left;width:0;height:50px'></div>
         <div style='float:right;clear:left;width:0;height:150px'></div>
         <div id=a style='overflow:hidden;margin:0 -50px;height:100px'></div></div>
         <div style='width:125px;clear:both'><div style='float:left;width:0;height:50px'></div>
         <div style='float:right;clear:left;width:25px;height:50px'></div>
         <div id=b style='overflow:hidden;margin-left:-50px;height:100px'></div></div>",
        &["a", "b"],
    );
    let (a, b) = (laid.rect(0), laid.rect(1));
    assert!(close(a[0], 0.0) && close(a[2], 200.0), "{a:?}");
    assert!(close(b[0], 0.0) && close(b[2], 100.0), "{b:?}");
}

#[test]
fn a_formatting_context_whose_border_does_not_fit_beside_a_float_moves_below_it() {
    // 1px is left beside the float and the margin: the box's 2px of border don't fit (WPT
    // floats-wrap-bfc-with-margin-003).
    let laid = lay_out(
        "<div style='width:30px'><div style='float:right;width:14px;height:10px'></div>
         <div id=b style='display:flow-root;margin-left:15px;border:1px solid;height:5px'></div></div>",
        &["b"],
    );
    let b = laid.rect(0);
    assert!(close(b[1], 10.0) && close(b[2], 15.0), "{b:?}");
}

#[test]
fn a_float_clearing_its_side_goes_beside_the_floats_on_the_other_side() {
    // The last left float clears the left floats (bottom 90) and fits beside the right float
    // placed at 90 (WPT floats-placement-005).
    let laid = lay_out(
        "<div style='width:150px;display:flow-root'><div style='float:left;clear:left;width:50px;height:50px'></div>
         <div style='height:40px'></div><div style='float:right;clear:right;width:50px;height:50px'></div>
         <div style='float:left;clear:left;width:100px;height:40px'></div>
         <div style='float:right;clear:right;width:50px;height:50px'></div>
         <div id=l style='float:left;clear:left;width:50px;height:50px'></div></div>",
        &["l"],
    );
    let l = laid.rect(0);
    assert!(close(l[0], 0.0) && close(l[1], 90.0), "{l:?}");
}

#[test]
fn a_block_level_image_avoids_floats() {
    // A block-level replaced element keeps clear of floats as a table does (WPT floats-039).
    let laid = lay_out(
        "<div style='width:200px'><div style='float:left;width:48px;height:48px'></div>
         <img id=i style='display:block;width:48px;height:48px'></div>",
        &["i"],
    );
    let i = laid.rect(0);
    assert!(close(i[0], 48.0) && close(i[1], 0.0), "{i:?}");
}

#[test]
fn content_that_does_not_fit_beside_a_leading_float_moves_below_it() {
    // The float leaves no room on the line: the inline-block goes below it (WPT
    // floats-placement-004, c5525-fltwidth-001).
    let laid = lay_out(
        "<div style='width:100px;line-height:0'><div style='float:left;width:100px;height:50px'></div>
         <span id=i style='display:inline-block;width:100px;height:40px'></span></div>",
        &["i"],
    );
    let i = laid.rect(0);
    assert!(close(i[0], 0.0) && close(i[1], 50.0), "{i:?}");
}

#[test]
fn a_float_met_past_where_a_line_wraps_goes_below_the_next_line() {
    // The float doesn't fit on the first line; the line then wraps before the nobr holding it:
    // the float goes below the second line, not the first (WPT floats-line-wrap-shifted-001).
    let laid = lay_out(
        "<div style='width:12ch;font-size:10px;line-height:1'>1111 <nobr>2222
         <div id=f style='float:left;width:12ch;height:1em'></div> 3333</nobr></div>",
        &["f"],
    );
    let f = laid.rect(0);
    assert!(close(f[1], 20.0), "{f:?}");
}

#[test]
fn an_inline_box_beside_a_float_paints_its_background() {
    // The line starts past the float: the span's box was cut to nothing (WPT clear-inline-001).
    let img = render(
        "<div><span style='float:left;width:96px;height:96px;background:orange'></span><span style='background:blue'>00</span></div>",
    );
    assert_eq!(pixel(&img, 100, 2), BLUE, "the span's background beside the float");
}

#[test]
fn a_float_moves_with_the_relatively_positioned_inline_it_is_in() {
    // The span's offset moves its content, the float in it too (WPT
    // block-in-inline-float-between-001, whose test has the span split around blocks).
    let laid = lay_out(
        "<div><span style='position:relative;left:100px'><span id=f style='float:left'>0</span></span></div>",
        &["f"],
    );
    let f = laid.rect(0);
    assert!(close(f[0], 100.0) && close(f[1], 0.0), "{f:?}");
}

/// Beside a float, a negative margin on the side away from it does not widen the space for a box that
/// avoids floats: in the 16px left beside a 14px float, `margin-right: -16px` leaves the box 16px wide,
/// and `margin-right: -17px` makes it too wide, so it moves below the float, 47px wide (Chromium; WPT
/// CSS2 `floats-wrap-bfc-with-margin-001`, `-002`).
#[test]
fn negative_margin_away_from_a_float_does_not_widen_its_space() {
    let laid = lay_out(
        "<div style='width:30px'><div style='float:left;width:14px;height:8px'></div>\
           <div id=a style='display:flow-root;height:5px;margin-right:-16px'></div></div>\
         <div style='width:30px'><div style='float:left;width:14px;height:8px'></div>\
           <div id=b style='display:flow-root;height:5px;margin-right:-17px'></div></div>",
        &["a", "b"],
    );
    let [ax, ay, aw, _] = laid.rect(0);
    assert!(close(ax, 14.0) && close(ay, 0.0) && close(aw, 16.0), "beside the float: {:?}", laid.rect(0));
    let [bx, by, bw, _] = laid.rect(1);
    assert!(close(bx, 0.0) && close(by, 13.0) && close(bw, 47.0), "below the float: {:?}", laid.rect(1));
}

/// A block's inset in the formatting context is where its own content edge is, not a float on that
/// side: beside a 10px float of a 120px block (in a 100px context), a 115px float of a 150px sibling
/// fits at 10px, and an inline-block 135px wide fits beside it on a line of that sibling (Chrome).
#[test]
fn a_wider_blocks_edge_is_no_float_beside_a_narrower_blocks_float() {
    let laid = lay_out(
        "<div style='display:flow-root;width:100px'>\
           <div style='width:120px'><div style='float:left;width:10px;height:10px'></div></div>\
           <div style='width:150px'><div id=b style='float:left;width:115px;height:10px'></div></div></div>\
         <div style='display:flow-root;width:100px;margin-top:50px'>\
           <div style='width:120px'><div style='float:left;width:10px;height:30px'></div></div>\
           <div style='width:150px'><span id=c style='display:inline-block;width:135px;height:5px'></span></div></div>",
        &["b", "c"],
    );
    let [bx, by, _, _] = laid.rect(0);
    assert!(close(bx, 10.0) && close(by, 0.0), "beside the float: {:?}", laid.rect(0));
    let [cx, cy, _, _] = laid.rect(1);
    assert!(close(cx, 10.0) && cy < 80.0, "beside the float: {:?}", laid.rect(1));
}
