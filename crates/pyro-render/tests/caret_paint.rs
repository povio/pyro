//! The caret (`Snapshot::caret`, a collapsed selection in editable content, and a focused text
//! control's `control_selection`) is painted in its `caret-shape` and `caret-color`, as Chromium
//! paints it: a bar or underscore on vertical lines too, an empty editing host's caret where its
//! `text-align` puts the line, and in a text control a block towards the covered character's side
//! (right-to-left text: to the left), an underscore under it, the text under a block in
//! `caret-color`'s second colour.
use std::{cell::RefCell, rc::Rc, sync::Arc};

use pyro_dom::{
    parse::{parse_document, DomRef},
    snapshot::{render_key, RenderHtml, RenderNodeKind},
};
use pyro_render::*;

/// Serves Ahem (`tests/fonts`) as `ahem.ttf`.
struct AhemFetch;
impl Fetcher for AhemFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(match url.ends_with("/ahem.ttf") {
            true => Ok(include_bytes!("fonts/Ahem.ttf").to_vec()),
            false => Err("offline".into()),
        });
    }
}

const STYLE: &str = "<!doctype html><style>@font-face{font-family:Ahem;src:url(ahem.ttf)}
  body{margin:0;background:white;font:20px/20px Ahem;color:black}
  #t{caret-color:rgb(0,255,0);outline:none}</style>";

/// Where the caret is: in the text of the element `#t` at a UTF-16 offset, in the empty element
/// `#t`, or in the focused text control `#t` at an offset of its value.
enum At {
    Text(u32),
    Empty,
    Control(u32),
}

fn render(body: &str, at: At) -> RgbaImage {
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", &format!("{STYLE}{body}"));
    let d = dom.borrow();
    let target = d.get_element_by_id(doc, "t").unwrap().unwrap();
    let (caret, control, focused) = match at {
        At::Text(offset) => (Some((render_key(d.first_child(target).unwrap().unwrap()), offset)), None, None),
        At::Empty => (Some((render_key(target), 0)), None, None),
        At::Control(offset) => (None, Some((render_key(target), offset, offset)), Some(render_key(target))),
    };
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
        width: 400,
        height: 120,
        device_scale_factor: 1.0,
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: Some((None, focused)) }),
        control_selection: control,
        caret,
        ..Default::default()
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(AhemFetch);
    let mut renderer = Renderer::new();
    renderer.text_antialiasing = false;
    // (Twice: the focus applies to the page's persistent document, made by the first.)
    renderer.render(&snap, Area::Viewport, Backend::Cpu, &fetcher).unwrap();
    renderer.set_interaction(1, None, focused, false);
    renderer.render(&snap, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// The bounding box (x0, y0, x1, y1, inclusive) of the pixels of `rgb`, and how many there are.
fn find(image: &RgbaImage, rgb: [u8; 3]) -> Option<(u32, u32, u32, u32, usize)> {
    let mut found: Option<(u32, u32, u32, u32, usize)> = None;
    for (i, p) in image.data.chunks(4).enumerate() {
        if p[..3] != rgb {
            continue;
        }
        let (x, y) = (i as u32 % image.width, i as u32 / image.width);
        found = Some(match found {
            None => (x, y, x, y, 1),
            Some((x0, y0, x1, y1, n)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y), n + 1),
        });
    }
    found
}

const LIME: [u8; 3] = [0, 255, 0];

#[test]
fn a_bar_or_underscore_caret_is_painted_on_vertical_lines() {
    for shape in ["bar", "underscore"] {
        let image = render(
            &format!("<div id=t contenteditable style='writing-mode:vertical-rl;caret-shape:{shape};height:100px'>XYZ</div>"),
            At::Text(1),
        );
        let (_, y0, _, y1, _) = find(&image, LIME).unwrap_or_else(|| panic!("no {shape} caret"));
        // After the first character, down the vertical line.
        assert!((18..=22).contains(&y0), "{shape} caret at y {y0}..{y1}");
    }
}

#[test]
fn an_empty_editing_host_has_its_caret_where_text_align_puts_the_line() {
    for (align, x) in [("left", 0), ("center", 200), ("right", 400)] {
        let image = render(
            &format!("<div id=t contenteditable style='caret-shape:bar;width:400px;text-align:{align}'></div>"),
            At::Empty,
        );
        let (x0, _, x1, _, _) = find(&image, LIME).unwrap_or_else(|| panic!("no caret for {align}"));
        assert!(x0.abs_diff(x) <= 1 || x1.abs_diff(x) <= 1, "{align}: caret at x {x0}..{x1}, not at {x}");
    }
}

#[test]
fn a_text_control_paints_block_and_underscore_carets_over_the_character_after_them() {
    // (Ahem's glyphs fill their em boxes, over a block caret: the caret covers a space.)
    let input = |value: &str, style: &str| {
        format!("<input id=t value='{value}' style='font:20px Ahem;border:0;padding:0;width:200px;{style}'>")
    };
    // A block over the second character: 20px wide, the line tall.
    let (x0, _, x1, _, _) = find(&render(&input("X Z", "caret-shape:block"), At::Control(1)), LIME).unwrap();
    assert_eq!((x0, x1), (20, 39), "block caret");
    // Right to left: the run is laid out from the right, the block covering the character on its left side.
    let rtl = render(&input(" YZ", "caret-shape:block;direction:rtl;unicode-bidi:bidi-override;text-align:left"), At::Control(0));
    let (x0, _, x1, _, _) = find(&rtl, LIME).unwrap();
    assert_eq!((x0, x1), (40, 59), "right-to-left block caret");
    // An underscore: a line under that character, far thinner than the line.
    let (x0, y0, x1, y1, _) = find(&render(&input("X Z", "caret-shape:underscore"), At::Control(1)), LIME).unwrap();
    assert_eq!((x0, x1), (20, 39), "underscore caret");
    assert!(y1 - y0 < 8, "an underscore, not a bar: y {y0}..{y1}");
    // The text under a block in the second colour.
    let image = render(&input("XYZ", "caret-shape:block;caret-color:rgb(0,255,0) rgb(0,0,255)"), At::Control(1));
    let (x0, _, x1, _, _) = find(&image, [0, 0, 255]).expect("the covered character in the second colour");
    assert!(x0 >= 20 && x1 <= 39, "only the covered character: x {x0}..{x1}");
}

/// A `display: contents` option of a list box select leaves no text among the layout children:
/// laying it out and rounding it panicked (`layout_data` of a text node; WPT selection/crashtests
/// `selection-modify-extend-forward-on-option`).
#[test]
fn a_list_box_with_a_display_contents_option_renders() {
    for display in ["contents", "none", "block"] {
        render(
            &format!("<select multiple style='display:table-cell'><option id=t style='display:{display}'>Test</option></select>"),
            At::Text(0),
        );
    }
}
