//! MathML `<mo>` spacing in a row (MathML Core § 3.2.4): the operator dictionary's `lspace` and
//! `rspace` for its text and form, or its own `lspace`/`rspace` attributes, lie between the
//! operator's box and its neighbours'. Chrome 158 at 36px: `+` 8px each side, `=` 10px, `,` 0 and
//! 6px, `lspace=12px rspace=4px` 12 and 4px. Fractions are not this file's (an operator directly
//! in an `<mfrac>` takes no row spacing).
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

const STYLE: &str = "<!doctype html><style>body{margin:0;background:white;font:36px serif} math{font-size:36px;font-family:inherit}</style>";

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

/// The gaps before and after the operator `o` between `a` and `b` in `math`.
fn gaps(math: &str) -> (f64, f64) {
    let r = rects(&format!("<div><math>{math}</math></div>"), &["a", "o", "b"]);
    // To 1/8 px: the boxes' own edges are 1/64 px off the glyph advances (the unspaced row too).
    let round = |v: f64| (v * 8.0).round() / 8.0;
    (round(r[1][0] - (r[0][0] + r[0][2])), round(r[2][0] - (r[1][0] + r[1][2])))
}

fn infix(mo: &str) -> (f64, f64) {
    gaps(&format!("<mrow><mi id=a>x</mi>{mo}<mi id=b>y</mi></mrow>"))
}

#[test]
fn dictionary_spaces_infix_operators() {
    assert_eq!(infix("<mo id=o>+</mo>"), (8.0, 8.0));
    assert_eq!(infix("<mo id=o>=</mo>"), (10.0, 10.0));
    assert_eq!(infix("<mo id=o>,</mo>"), (0.0, 6.0));
}

#[test]
fn explicit_lspace_and_rspace_replace_the_dictionary() {
    assert_eq!(infix("<mo id=o lspace=0px rspace=0px>+</mo>"), (0.0, 0.0));
    assert_eq!(infix("<mo id=o lspace=12px rspace=4px>+</mo>"), (12.0, 4.0));
    assert_eq!(infix("<mo id=o lspace=4px rspace=12px>+</mo>"), (4.0, 12.0));
    // One side given: the other is the dictionary's.
    assert_eq!(infix("<mo id=o lspace=2px>=</mo>"), (2.0, 10.0));
    // Not a `<length>` (or negative): the dictionary's.
    assert_eq!(infix("<mo id=o lspace=-3px rspace=wide>+</mo>"), (8.0, 8.0));
    // `em` is the operator's font size.
    assert_eq!(infix("<mo id=o lspace=0.5em rspace=0em>+</mo>"), (18.0, 0.0));
}

#[test]
fn the_form_follows_the_row_position() {
    // A leading `+` is a prefix operator (0, 0), after a leading `(` (a prefix, 0 and 0) an infix
    // one; `form` overrides the position.
    let lead = gaps("<mrow><mo id=a>(</mo><mo id=o>+</mo><mi id=b>y</mi></mrow>");
    assert_eq!(lead, (8.0, 8.0), "the `+` is infix between `(` and `y`");
    let r = rects("<div><math><mrow><mo id=o>+</mo><mi id=b>y</mi></mrow></math></div>", &["o", "b"]);
    assert!((r[1][0] - (r[0][0] + r[0][2])).abs() < 0.125, "{r:?}");
    assert_eq!(infix("<mo id=o form=prefix>+</mo>"), (0.0, 0.0));
}

#[test]
fn an_operator_outside_a_row_takes_no_spacing() {
    // In a fraction the operator lies where an unspaced one does, whatever the fraction's layout.
    let frac = |mo: &str| rects(&format!("<div><math><mfrac>{mo}<mi id=b>y</mi></mfrac></math></div>"), &["o", "b"]);
    assert_eq!(frac("<mo id=o>=</mo>"), frac("<mo id=o lspace=0px rspace=0px>=</mo>"));
    // A lone operator in its row (the `<math>`) is an infix one.
    let r = rects("<div><math><mo id=p>=</mo></math></div>", &["p"]);
    let alone = rects("<div><math><mi id=q>=</mi></math></div>", &["q"]);
    assert!((r[0][0] - alone[0][0] - 10.0).abs() < 0.125, "{r:?} {alone:?}");
}
