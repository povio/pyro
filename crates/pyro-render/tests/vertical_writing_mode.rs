//! Text in vertical writing modes: its boxes and where it paints. Expected values measured on Chrome
//! 156 with the same markup (Helvetica, so macOS only where they depend on advances).
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
        height: 500,
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

fn snap(html: &str) -> Snapshot {
    Snapshot {
        html: html.into(),
        base_url: "http://test/".into(),
        width: 800,
        height: 500,
        device_scale_factor: 1.0,
        ..Default::default()
    }
}

/// The bounding box of the page's dark pixels.
fn ink(html: &str) -> [u32; 4] {
    let (img, _, _) = Renderer::new().render(&snap(html), Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    let mut r = [u32::MAX, u32::MAX, 0, 0];
    for y in 0..img.height {
        for x in 0..img.width {
            if img.data[(y as usize * img.width as usize + x as usize) * 4] < 128 {
                r = [r[0].min(x), r[1].min(y), r[2].max(x), r[3].max(y)];
            }
        }
    }
    r
}

const FONT: &str = "<!doctype html><style>html,body{margin:0}body{font:16px Helvetica}</style>";

#[cfg(target_os = "macos")]
fn assert_close(got: [f64; 4], want: [f64; 4]) {
    assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() < 0.05), "{got:?}, Chrome {want:?}");
}

/// A flex column of boxes in a rotated `vertical-rl` box, like the side text of a CMR.
#[cfg(target_os = "macos")]
#[test]
fn boxes_in_a_rotated_vertical_flex_match_chrome() {
    let html = format!(
        "{FONT}<div style='position:absolute;left:20px;top:250px;height:220px;display:flex;justify-content:space-between;\
         align-items:center;writing-mode:vertical-rl;white-space:pre-line;font-size:7px;gap:10px;transform:rotate(180deg)'>\
         <span id=a>First item line one\nsecond line of it</span><strong id=b style='font-size:13px'>1 - 15</strong><span id=c>Other item</span></div>"
    );
    let got = boxes(&html, &["a", "b", "c"]);
    assert_close(got[0], [20.0, 414.75, 36.0, 470.0]);
    assert_close(got[1], [20.5, 332.09, 35.5, 365.34]);
    assert_close(got[2], [24.0, 250.0, 32.0, 282.69]);
}

#[cfg(target_os = "macos")]
#[test]
fn an_inline_box_runs_down_its_line() {
    let html = format!(
        "{FONT}<div style='position:absolute;left:560px;top:20px;width:80px;height:200px;writing-mode:vertical-rl'>Some <span id=s>inline span</span> text</div>"
    );
    assert_close(boxes(&html, &["s"])[0], [622.0, 66.25, 640.0, 142.75]);
}

#[test]
fn the_box_of_vertical_text_is_as_tall_as_its_text_and_as_wide_as_its_lines() {
    let html = format!(
        "{FONT}<div id=d style='position:absolute;left:10px;top:10px;writing-mode:vertical-lr;line-height:30px'><div id=e style='height:100px'>x</div></div>"
    );
    let got = boxes(&html, &["d", "e"]);
    assert_eq!(got[1], [10.0, 10.0, 40.0, 110.0]);
    assert_eq!(got[0], got[1]);
}

/// The ink's bounding box in a Chrome 156 screenshot (x0, y0, x1, y1), give or take a pixel of
/// antialiasing.
#[cfg(target_os = "macos")]
#[test]
fn vertical_text_paints_down_the_page() {
    let text = "Vertical first line of the text wraps to a second column here";
    for (css, chrome) in [
        ("writing-mode:vertical-rl", [130, 20, 177, 205]),
        ("writing-mode:vertical-lr", [104, 20, 151, 205]),
        ("writing-mode:vertical-rl;transform:rotate(180deg)", [102, 54, 149, 239]),
    ] {
        let got = ink(&format!(
            "{FONT}<div style='position:absolute;left:100px;top:20px;width:80px;height:220px;{css}'>{text}</div>"
        ));
        assert!(got.iter().zip(chrome).all(|(g, c)| (*g as i32 - c).abs() <= 1), "{css}: {got:?}, Chrome {chrome:?}");
    }
}

/// Text as wide as the box measured for it stays on one line, whatever float noise its
/// advances carry.
#[test]
fn shrink_to_fit_text_never_wraps() {
    let mut html = String::from(FONT);
    let mut ids = Vec::new();
    for i in 0..200 {
        ids.push(format!("t{i}"));
        let size = 5.0 + i as f64 * 0.137;
        html += &format!(
            "<div id=t{i} style='position:absolute;display:flex;line-height:20px;font-size:{size}px'><b>16 - 18 + 20 + 23</b></div>"
        );
    }
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    for (id, b) in ids.iter().zip(boxes(&html, &ids)) {
        assert!((b[3] - b[1] - 20.0).abs() < 0.01, "{id}: {b:?}");
    }
}

/// Chrome's first line of `font:16px Arial` (17 px of ascent and descent): one layout unit short of
/// it leaves no half-leading (Chromium's division truncates), two take a whole pixel off.
#[cfg(target_os = "macos")]
#[test]
fn a_line_a_layout_unit_short_keeps_its_baseline() {
    let top = |line_height: f64| {
        ink(&format!(
            "<!doctype html><body style='margin:0;font:16px/{line_height}px Arial'><div style='position:absolute;top:20px'>H</div>"
        ))[1]
    };
    assert_eq!(top(16.98), top(17.0));
    assert_eq!(top(16.97), top(17.0) - 1);
}
