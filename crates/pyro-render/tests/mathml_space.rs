//! MathML `<mspace>` sizing (MathML Core § 3.2.4): its `width` attribute is its width, `height`
//! lies above the baseline and `depth` below it. Each must be a non-negative `<length>`; anything
//! else counts as absent (0). WPT's `MathMLFeatureDetection.has_mspace()` measures the first case,
//! and most `mathml/relations/css-styling` pages assert it before anything else.
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

const STYLE: &str = "<!doctype html><style>body{margin:0;background:white;font:20px serif} math{font-family:inherit}</style>";

/// Each element's border box: `[x, y, width, height]`.
fn rects(body: &str, ids: &[&str]) -> Vec<[f64; 4]> {
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
            [b.x, b.y, b.width, b.height]
        })
        .collect()
}

fn width(math: &str) -> f64 {
    rects(&format!("<div><math>{math}</math></div>"), &["s"])[0][2]
}

#[test]
fn the_width_attribute_is_the_width() {
    // `has_mspace()`: the two spaces of one `<math>`, no whitespace between them.
    let r = rects("<math><mspace id=a></mspace><mspace id=b width='20px'></mspace></math>", &["a", "b"]);
    assert_eq!((r[0][2], r[1][2]), (0.0, 20.0), "{r:?}");
    assert_eq!(r[1][0], r[0][0], "{r:?}");
    // `em` is the space's font size.
    assert_eq!(width("<mspace id=s width='2em' style='font-size:10px'></mspace>"), 20.0);
    assert_eq!(width("<mspace id=s width='calc(5px + 1em)'></mspace>"), 25.0);
}

#[test]
fn an_invalid_width_counts_as_absent() {
    for value in ["20%", "-5px", "20", "wide", ""] {
        assert_eq!(width(&format!("<mspace id=s width='{value}'></mspace>")), 0.0, "width={value:?}");
    }
    // Spaces around the value are allowed.
    assert_eq!(width("<mspace id=s width=' 7px '></mspace>"), 7.0);
}

#[test]
fn height_lies_above_the_baseline_and_depth_below() {
    // `h` stands on the baseline; `s` reaches as high and 12px lower.
    let r = rects(
        "<div><math><mspace id=h width='5px' height='30px'></mspace>\
         <mspace id=s width='5px' height='30px' depth='12px'></mspace>\
         <mspace id=d width='5px' depth='8px'></mspace></math></div>",
        &["h", "s", "d"],
    );
    let bottom = |b: [f64; 4]| b[1] + b[3];
    assert_eq!((r[0][3], r[1][3], r[2][3]), (30.0, 42.0, 8.0), "{r:?}");
    assert_eq!(r[1][1], r[0][1], "{r:?}");
    assert_eq!(bottom(r[1]) - bottom(r[0]), 12.0, "{r:?}");
    // Depth alone hangs entirely below the baseline.
    assert_eq!(r[2][1], bottom(r[0]), "{r:?}");
}

#[test]
fn author_styles_override_the_attributes() {
    assert_eq!(width("<mspace id=s width='20px' style='width:5px'></mspace>"), 5.0);
    // An inline box takes no width.
    assert_eq!(width("<mspace id=s width='20px' style='display:inline'></mspace>"), 0.0);
}

#[test]
fn only_mathml_mspace_is_sized() {
    // An HTML element of that name keeps its inline box.
    assert_eq!(rects("<div><mspace id=s width='20px'></mspace></div>", &["s"])[0][2], 0.0);
}
