//! MathML fractions (`layout::math` in blitz-dom): an ordinary `<mfrac>` (two element children)
//! stacks its numerator over its denominator, both centred, with a bar between them painted in
//! its colour; `linethickness="0"` draws none. An `mfrac` that is not
//! ordinary, and an HTML element of that name, keep their children on one line. Assertions are
//! structural (Chrome's numbers depend on its math font): who is above whom, centring, the 1px
//! padding beside the bar, and relative gaps.
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

const STYLE: &str = "<!doctype html><style>body{margin:0;background:white;font:20px monospace;color:black}\
  p{margin:0} math{font-family:inherit}</style>";

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

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("{STYLE}{body}"),
        base_url: "http://test/".into(),
        width: 300,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let i = (y as usize * img.width as usize + x as usize) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

const GREEN: [u8; 3] = [0, 128, 0];

fn bottom(r: &[f64; 4]) -> f64 {
    r[1] + r[3]
}

fn center(r: &[f64; 4]) -> f64 {
    r[0] + r[2] / 2.0
}

#[test]
fn an_mfrac_stacks_its_numerator_over_its_denominator() {
    // The receipt's three fractions (Chrome stacks them; pyro had laid both children on the line).
    for frac in [
        "<mfrac id=f><mn id=n>1</mn><mn id=d>2</mn></mfrac>",
        "<mfrac id=f><mn id=n>123</mn><mn id=d>4</mn></mfrac>",
        "<mfrac id=f linethickness=0><mn id=n>1</mn><mn id=d>2</mn></mfrac>",
    ] {
        let r = rects(&format!("<p><math>{frac}</math></p>"), &["f", "n", "d"]);
        let (f, n, d) = (r[0], r[1], r[2]);
        assert!(bottom(&n) <= d[1], "numerator above the denominator: {frac} {r:?}");
        assert!((center(&n) - center(&d)).abs() < 0.5, "centred on each other: {frac} {r:?}");
        // As wide as the wider child, with 1px of padding each side (`padding-inline: 1px`).
        assert!((f[2] - (n[2].max(d[2]) + 2.0)).abs() < 0.5, "width: {frac} {r:?}");
        // From the numerator's top to the denominator's bottom.
        assert!((f[1] - n[1]).abs() < 0.5 && (bottom(&f) - bottom(&d)).abs() < 0.5, "height: {frac} {r:?}");
    }
}

#[test]
fn a_ruleless_fraction_keeps_the_ruled_gap_without_a_math_table() {
    // The test font has no MATH table, so both gaps are Chromium's fallbacks: with a bar, a bar's
    // thickness on each side of it plus the bar (3 bars), without one the stack gap (3 bars).
    // Chrome 157, `<math style="font-family:monospace">` at 20px: 2.625 and 2.633px inline,
    // 6.141 and 6.148px in display math. (With a math font, e.g. Chrome's default STIX Two Math,
    // the ruleless gap is the tighter one; the test fonts have none.) The two gaps are equal by
    // construction, so this does not tell the paths apart: the stack path's shifts are checked by
    // `layout::math`'s unit test `shifts_without_a_math_table`, the missing bar by the paint test.
    let gap = |attrs: &str| {
        let r = rects(&format!("<p><math><mfrac id=f {attrs}><mn id=n>1</mn><mn id=d>2</mn></mfrac></math></p>"), &["n", "d"]);
        r[1][1] - bottom(&r[0])
    };
    let (ruled, ruleless, thick) = (gap(""), gap("linethickness=0"), gap("linethickness=10px"));
    assert!((ruleless - ruled).abs() < 0.5, "{ruleless} = {ruled}");
    assert!(ruled > 0.0, "{ruled}");
    // A thicker bar pushes them apart by its extra thickness.
    assert!(thick > ruled + 5.0, "{thick} > {ruled} + 5");
}

#[test]
fn compact_fractions_shrink_their_children_and_display_ones_do_not() {
    // `m|mfrac > m|* { math-depth: auto-add; font-size: math }`: a script level down (0.71) in inline
    // math, none in display math (`math-style: normal`).
    let width = |math: &str| {
        rects(&format!("<p>{math}<mfrac><mn id=n>1111</mn><mn>2</mn></mfrac></math></p>"), &["n"])[0][2]
    };
    let (inline, display) = (width("<math>"), width("<math display=block>"));
    // Four 12px monospace digits at 20px.
    assert!((display - 48.0).abs() < 1.0, "{display}");
    assert!((inline - 48.0 * 0.71).abs() < 2.0, "{inline}");
}

#[test]
fn a_fraction_is_an_atomic_inline_on_its_line() {
    // Text before and after it on one line: the fraction is between them, and the line is
    // taller than one of text.
    let r = rects(
        "<p id=p><span id=a>x</span><math><mfrac id=f><mn>1</mn><mn>2</mn></mfrac></math><span id=b>y</span></p>",
        &["p", "a", "f", "b"],
    );
    let (p, a, f, b) = (r[0], r[1], r[2], r[3]);
    assert!(a[0] + a[2] <= f[0] + 0.5 && f[0] + f[2] <= b[0] + 0.5, "{r:?}");
    assert!((a[1] - b[1]).abs() < 0.5, "{r:?}");
    assert!(f[1] < a[1] && bottom(&f) > bottom(&a), "the fraction straddles the text: {r:?}");
    assert!(p[3] >= f[3], "{r:?}");
}

#[test]
fn the_bar_is_painted_between_them_in_the_fractions_colour() {
    let body = "<p><math style='font-size:40px;color:green'><mfrac id=f linethickness=4px><mn id=n>1</mn><mn id=d>2</mn></mfrac></math></p>";
    let r = rects(body, &["f", "n", "d"]);
    let (f, n, d) = (r[0], r[1], r[2]);
    let img = render(body);
    let x = center(&f).round() as u32;
    let rows: Vec<u32> = (bottom(&n).ceil() as u32..d[1].floor() as u32).filter(|&y| pixel(&img, x, y) == GREEN).collect();
    assert!((3..=5).contains(&rows.len()), "a 4px bar between them: {rows:?} {r:?}");
    // Across the content box, not the 1px of padding.
    let y = rows[rows.len() / 2];
    assert_eq!(pixel(&img, (f[0] + 1.5) as u32, y), GREEN, "{r:?}");
    assert_ne!(pixel(&img, f[0] as u32, y), GREEN, "{r:?}");

    // No bar with `linethickness="0"`.
    let body = "<p><math style='font-size:40px;color:green'><mfrac id=f linethickness=0><mn id=n>1</mn><mn id=d>2</mn></mfrac></math></p>";
    let r = rects(body, &["f", "n", "d"]);
    let img = render(body);
    let x = center(&r[0]).round() as u32;
    assert!((bottom(&r[1]).ceil() as u32..r[2][1].floor() as u32).all(|y| pixel(&img, x, y) != GREEN), "{r:?}");
}

#[test]
fn fractions_that_are_not_ordinary_and_html_controls_keep_their_layout() {
    // Three children: not a fraction, its children on one line.
    let r = rects("<p><math><mfrac><mn id=a>1</mn><mn id=b>2</mn><mn id=c>3</mn></mfrac></math></p>", &["a", "b", "c"]);
    assert!((r[0][1] - r[1][1]).abs() < 0.5 && (r[1][1] - r[2][1]).abs() < 0.5, "{r:?}");
    assert!(r[0][0] < r[1][0] && r[1][0] < r[2][0], "{r:?}");
    // An HTML element named mfrac (outside math) is an unknown inline.
    let r = rects("<p><mfrac><span id=a>1</span><span id=b>2</span></mfrac></p>", &["a", "b"]);
    assert!((r[0][1] - r[1][1]).abs() < 0.5 && r[0][0] < r[1][0], "{r:?}");
    // The receipt's HTML control beside fractions.
    let r = rects(
        "<p><math><mfrac><mn>1</mn><mn>2</mn></mfrac></math></p><div id=c style='width:40px;height:12px'></div>",
        &["c"],
    );
    assert_eq!((r[0][2], r[0][3]), (40.0, 12.0), "{r:?}");
}
