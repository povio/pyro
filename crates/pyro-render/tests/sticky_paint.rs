//! `position: sticky` in paint: a sticky box is laid out as if static and painted where its
//! scroll container holds it. The page computes that offset from its scroll positions
//! (pyro-dom `Layout::sticky_offsets`) and hands it over in `Snapshot::sticky_offsets`; blitz-paint
//! moves the box, its descendants and (for a table row or row group) its cells and background.
//! Pixel checks as in WPT's `css/css-position/sticky` reftests.
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

type Offsets = Vec<(u64, (f64, f64))>;

/// A live document (the mirror), rendered with scroll positions and sticky offsets by element id.
struct LivePage {
    renderer: Renderer,
    tree: Arc<Tree>,
    keys: Vec<u64>,
}

impl LivePage {
    fn new(html: &str, ids: &[&str]) -> LivePage {
        let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
        let doc = parse_document(&dom, "http://test/", html);
        let d = dom.borrow();
        let keys = ids.iter().map(|id| render_key(d.get_element_by_id(doc, id).unwrap().unwrap())).collect();
        let tree = d
            .render_tree(
                doc,
                &RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None },
            )
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
        LivePage { renderer: Renderer::new(), tree: Arc::new(tree), keys }
    }

    fn render(&mut self, scroll: Offsets, sticky: Offsets) -> RgbaImage {
        let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
        let snapshot = Snapshot {
            base_url: "http://test/".into(),
            width: 300,
            height: 300,
            device_scale_factor: 1.0,
            live: Some(Live { page: 1, tree: self.tree.clone(), clip: None, interaction: None }),
            element_scroll: Some(scroll),
            sticky_offsets: Some(sticky),
            ..Default::default()
        };
        self.renderer.render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
    }
}

fn pixel(img: &RgbaImage, x: usize, y: usize) -> [u8; 3] {
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

const WHITE: [u8; 3] = [255, 255, 255];
const GREEN: [u8; 3] = [0, 128, 0];
const BLUE: [u8; 3] = [0, 0, 255];

/// A sticky box in a scrolled container is painted at its offset, with its descendants, and goes
/// back when the offset is no longer given.
#[test]
fn a_sticky_box_is_painted_at_its_offset() {
    let html = "<!doctype html><style>body{margin:0}</style>\
        <div id=scroller style='width:100px;height:100px;overflow:hidden'>\
        <div style='height:50px'></div>\
        <div id=sticky style='position:sticky;top:0;height:20px;background:green'>\
        <div style='width:10px;height:10px;background:blue'></div></div>\
        <div style='height:500px'></div></div>";
    let mut page = LivePage::new(html, &["scroller", "sticky"]);
    let (scroller, sticky) = (page.keys[0], page.keys[1]);
    // At rest: laid out at 50..70.
    let img = page.render(vec![], vec![]);
    assert_eq!(pixel(&img, 50, 60), GREEN);
    assert_eq!(pixel(&img, 5, 55), BLUE);
    assert_eq!(pixel(&img, 50, 10), WHITE);
    // Scrolled by 80 the box would be at -30..-10: held at the top by an offset of 30.
    let img = page.render(vec![(scroller, (0.0, 80.0))], vec![(sticky, (0.0, 30.0))]);
    assert_eq!(pixel(&img, 50, 10), GREEN);
    assert_eq!(pixel(&img, 5, 5), BLUE);
    assert_eq!(pixel(&img, 50, 25), WHITE);
    // Scrolled back.
    let img = page.render(vec![], vec![]);
    assert_eq!(pixel(&img, 50, 60), GREEN);
    assert_eq!(pixel(&img, 50, 10), WHITE);
}

/// A fractional offset keeps the box on the pixel grid it is held at: laid out at 10.5 and held
/// 9.5 further, it is painted at 20, not at 21 (WPT `position-sticky-fractional-offset`).
#[test]
fn a_fractional_sticky_offset_snaps_from_the_unrounded_position() {
    let html = "<!doctype html><style>body{margin:0}</style>\
        <div id=scroller style='width:100px;height:100px;overflow:hidden'>\
        <div style='height:10.5px'></div>\
        <div id=sticky style='position:sticky;top:0;height:50px;background:green'></div>\
        <div style='height:500px'></div></div>";
    let mut page = LivePage::new(html, &["scroller", "sticky"]);
    let (scroller, sticky) = (page.keys[0], page.keys[1]);
    let img = page.render(vec![(scroller, (0.0, 20.0))], vec![(sticky, (0.0, 9.5))]);
    assert_eq!(pixel(&img, 50, 0), GREEN);
    assert_eq!(pixel(&img, 50, 49), GREEN);
    assert_eq!(pixel(&img, 50, 50), WHITE);
}

/// A sticky table row group moves its cells and the background it paints behind them.
#[test]
fn a_sticky_row_group_moves_its_cells_and_background() {
    let html = "<!doctype html><style>body{margin:0} table{border-collapse:collapse} td{padding:0}</style>\
        <div id=scroller style='width:100px;height:100px;overflow:hidden'>\
        <table><thead id=head style='position:sticky;top:0;background:green'>\
        <tr><td><div style='width:50px;height:20px'><div style='width:10px;height:10px;background:blue'></div></div></td></tr></thead>\
        <tbody><tr><td><div style='width:50px;height:500px'></div></td></tr></tbody></table></div>";
    let mut page = LivePage::new(html, &["scroller", "head"]);
    let (scroller, head) = (page.keys[0], page.keys[1]);
    let img = page.render(vec![], vec![]);
    assert_eq!(pixel(&img, 30, 10), GREEN);
    assert_eq!(pixel(&img, 5, 5), BLUE);
    let img = page.render(vec![(scroller, (0.0, 60.0))], vec![(head, (0.0, 60.0))]);
    assert_eq!(pixel(&img, 30, 10), GREEN);
    assert_eq!(pixel(&img, 5, 5), BLUE);
    assert_eq!(pixel(&img, 30, 30), WHITE);
}
