//! Regression tests for review findings on the round-6 writing-mode work: orthogonal blocks beside
//! floats and their percentage padding, `text-combine-upright` on horizontal text, the root's
//! anonymous text under a bidi override, per-span `text-orientation` in sideways blocks, and the
//! direction of outside marker images and the select arrow in vertical modes.
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

fn pixels(html: &str) -> Vec<u8> {
    use_fork_safe_fonts();
    let snap = Snapshot {
        html: html.into(),
        base_url: "http://test/".into(),
        width: 300,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snap, Area::Viewport, Backend::Cpu, &fetcher()).unwrap().0.data
}

/// The pixels of `html` after `edit` changed its DOM (scripts do not run in these renders).
fn pixels_after(html: &str, edit: impl FnOnce(&mut pyro_dom::Dom, pyro_dom::NodeId)) -> Vec<u8> {
    use_fork_safe_fonts();
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", html);
    edit(&mut dom.borrow_mut(), doc);
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
        width: 300,
        height: 200,
        device_scale_factor: 1.0,
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
        ..Default::default()
    };
    Renderer::new().render(&snap, Area::Viewport, Backend::Cpu, &fetcher()).unwrap().0.data
}

fn same_pixels(test: &str, reference: &str) -> bool {
    pixels(test) == pixels(reference)
}

fn overlap(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

#[test]
fn an_orthogonal_auto_width_block_does_not_overlap_a_float_it_cannot_fit_beside() {
    let html = "<!doctype html><style>body{margin:0}#c{width:200px}#f{float:right;width:150px;height:50px;background:red}\
        #v{writing-mode:vertical-rl}#i{width:100px;height:20px}</style>\
        <div id=c><div id=f></div><div id=v><div id=i></div></div></div>";
    let b = boxes(html, &["f", "v"]);
    assert!(!overlap(b[0], b[1]), "float {:?} and orthogonal block {:?} overlap", b[0], b[1]);
}

#[test]
fn an_orthogonal_max_inline_size_clamp_takes_percentage_padding_of_the_inline_size() {
    let html = "<!doctype html><style>body{margin:0}#p{width:400px;height:200px}\
        #v{writing-mode:vertical-rl;max-inline-size:100px;padding-inline:10%;font:20px/1 monospace}</style>\
        <div id=p><div id=v>xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx</div></div>";
    let b = boxes(html, &["v"]);
    let height = b[0][3] - b[0][1];
    assert!((height - 180.0).abs() < 0.5, "height {height}, expected 100 + 2 * 10% of 400");
}

#[test]
fn text_combine_upright_does_nothing_to_horizontal_text() {
    let test = "<!doctype html><style>body{margin:0;font:20px monospace}span{letter-spacing:10px}</style>\
        <p><span style=text-combine-upright:all>ab cd</span> <span style='text-combine-upright:all;display:inline-block'>ef</span> gh</p>";
    let reference = "<!doctype html><style>body{margin:0;font:20px monospace}span{letter-spacing:10px}</style>\
        <p><span>ab cd</span> <span style=display:inline-block>ef</span> gh</p>";
    assert!(same_pixels(test, reference));
}

#[test]
fn the_roots_own_text_takes_bodys_direction_under_the_roots_bidi_override() {
    let append_abc = |dom: &mut pyro_dom::Dom, doc: pyro_dom::NodeId| {
        let root = dom.document_element(doc).unwrap().unwrap();
        let text = dom.create_text(doc, "ABC").unwrap();
        dom.append_child(root, text).unwrap();
    };
    let test = pixels_after(
        "<!doctype html><html style='unicode-bidi:bidi-override;font:20px monospace'><body dir=rtl style=margin:0></body></html>",
        append_abc,
    );
    let reference = pixels_after(
        "<!doctype html><html style='font:20px monospace'><body style=margin:0>\
         <div dir=rtl style='unicode-bidi:bidi-override'>ABC</div></body></html>",
        |_, _| {},
    );
    let ltr = pixels_after(
        "<!doctype html><html style='unicode-bidi:bidi-override;font:20px monospace'><body style=margin:0></body></html>",
        append_abc,
    );
    assert!(test != ltr, "the override did not take body's direction");
    assert!(test == reference);
}

#[test]
fn an_upright_span_stands_upright_in_a_sideways_vertical_block() {
    let test = "<!doctype html><style>body{margin:0}div{writing-mode:vertical-rl;text-orientation:sideways;font:20px monospace}\
        span{text-orientation:upright}</style><div><span>ab</span></div>";
    let reference = "<!doctype html><style>body{margin:0}div{writing-mode:vertical-rl;text-orientation:upright;font:20px monospace}\
        </style><div><span>ab</span></div>";
    assert!(same_pixels(test, reference));
}

#[test]
fn a_right_to_left_vertical_items_outside_marker_image_goes_below_it() {
    // A green square marker: in `vertical-rl; direction: rtl` the item's inline start is its bottom.
    let html = |dir: &str| {
        format!(
            "<!doctype html><style>body{{margin:0}}ul{{writing-mode:vertical-rl;direction:{dir};margin:40px;padding:0;\
             list-style-image:linear-gradient(lime,lime);font:20px/1 monospace;height:100px}}</style><ul><li>x</li></ul>"
        )
    };
    let lime_rows = |html: &str| {
        let data = pixels(html);
        let rows: Vec<usize> = (0..200)
            .filter(|y| (0..300).any(|x| {
                let p = &data[(y * 300 + x) * 4..][..3];
                p[0] < 60 && p[1] > 200 && p[2] < 60
            }))
            .collect();
        (rows.first().copied(), rows.last().copied())
    };
    let (ltr_top, _) = lime_rows(&html("ltr"));
    let (rtl_top, _) = lime_rows(&html("rtl"));
    assert!(ltr_top.unwrap() < 40, "ltr marker above the item: {ltr_top:?}");
    assert!(rtl_top.unwrap() >= 140, "rtl marker below the item: {rtl_top:?}");
}

#[test]
fn a_sideways_lr_selects_arrow_is_at_its_inline_end_the_top() {
    let html = "<!doctype html><style>body{margin:0}select{writing-mode:sideways-lr;color:red;width:30px;height:100px}</style>\
        <select><option>a</option></select>";
    let data = pixels(html);
    let red_rows: Vec<usize> = (0..200)
        .filter(|y| (0..300).any(|x| {
            let p = &data[(y * 300 + x) * 4..][..3];
            p[0] > 150 && p[1] < 80 && p[2] < 80
        }))
        .collect();
    assert!(!red_rows.is_empty(), "no arrow painted");
    assert!(*red_rows.last().unwrap() < 50, "arrow rows {red_rows:?}: expected at the top (inline end of sideways-lr)");
}

#[test]
fn a_small_selects_arrow_is_still_painted() {
    let html = "<!doctype html><style>body{margin:0}select{color:red;font-size:8px;width:15px;height:14px}</style>\
        <select><option>a</option></select>";
    let data = pixels(html);
    let red = (0..200 * 300).any(|i| {
        let p = &data[i * 4..][..3];
        p[0] > 150 && p[1] < 80 && p[2] < 80
    });
    assert!(red, "no arrow in a select whose padding box is under 19px");
}

#[test]
fn a_canvas_lays_out_at_its_attributes_read_as_reflection_reads_them() {
    let html = "<!doctype html><style>body{margin:0}canvas{display:block}</style>\
        <canvas id=a width=100px height=40></canvas><canvas id=b width=4294967296 height=-5></canvas>\
        <canvas id=c width=-0 height=' 20'></canvas>";
    let b = boxes(html, &["a", "b", "c"]);
    let size = |r: [f64; 4]| (r[2] - r[0], r[3] - r[1]);
    assert_eq!(size(b[0]), (100.0, 40.0));
    assert_eq!(size(b[1]), (300.0, 150.0));
    assert_eq!(size(b[2]), (0.0, 20.0));
}

/// The pixels of a page with the text control `id` focused and its value's selection from `start`
/// to `end` (UTF-16 code units), or unfocused (`None`).
fn control_pixels(html: &str, id: &str, selection: Option<(u32, u32)>) -> Vec<u8> {
    use_fork_safe_fonts();
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", html);
    let d = dom.borrow();
    let key = render_key(d.get_element_by_id(doc, id).unwrap().unwrap());
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
        width: 300,
        height: 200,
        device_scale_factor: 1.0,
        live: Some(Live {
            page: 1,
            tree: Arc::new(tree),
            clip: None,
            interaction: Some((None, selection.map(|_| key))),
        }),
        control_selection: selection.map(|(start, end)| (key, start, end)),
        ..Default::default()
    };
    Renderer::new().render(&snap, Area::Viewport, Backend::Cpu, &fetcher()).unwrap().0.data
}

#[test]
fn a_focused_vertical_text_control_shows_its_caret_selection_and_scrolls_to_its_caret() {
    let value = "abcdefghijklmnopqrstuvwxyz0123456789";
    let html = format!(
        "<!doctype html><style>body{{margin:0}}input{{writing-mode:vertical-rl;font:16px monospace;height:80px}}</style>\
         <input id=i value={value}>"
    );
    let len = value.len() as u32;
    let unfocused = control_pixels(&html, "i", None);
    let caret_at_start = control_pixels(&html, "i", Some((0, 0)));
    let caret_at_end = control_pixels(&html, "i", Some((len, len)));
    let selected = control_pixels(&html, "i", Some((0, 3)));
    assert!(caret_at_start != unfocused, "no caret painted");
    // The value overflows the 80px column: a caret at its end scrolls it.
    let differing = caret_at_end.chunks(4).zip(caret_at_start.chunks(4)).filter(|(a, b)| a != b).count();
    assert!(differing > 200, "the text did not follow the caret ({differing} px differ)");
    assert!(selected != caret_at_start, "no selection painted");
}

#[test]
fn a_focused_text_control_scrolls_to_its_caret() {
    let value = "abcdefghijklmnopqrstuvwxyz0123456789";
    let html = format!("<!doctype html><style>body{{margin:0}}input{{font:16px monospace;width:80px}}</style><input id=i value={value}>");
    let len = value.len() as u32;
    let at_start = control_pixels(&html, "i", Some((0, 0)));
    let at_end = control_pixels(&html, "i", Some((len, len)));
    let differing = at_start.chunks(4).zip(at_end.chunks(4)).filter(|(a, b)| a != b).count();
    assert!(differing > 200, "the text did not follow the caret ({differing} px differ)");
}
