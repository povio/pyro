//! The page's selection (`Snapshot::selection`, pyro-webapi's document selection) is painted as
//! Chromium paints it, compared as reftests are: selected text renders as text styled with the
//! highlight's colours does.
//!
//! - Text selected under a `::selection` style takes its colour, on its background (transparent
//!   when it sets none); text without one takes `HighlightText` on `Highlight`.
//! - Decorations whose colour is the text's (`currentcolor`) take the highlight's colour over the
//!   selected text; one of another colour stays one line in its colour.
//! - Text in `user-select: none` is not highlighted.
//!
//! The lines are lower than the text's content area, which the highlight then covers (it covers
//! the line box or the content area, whichever reaches further), as a span's background does. The
//! text is aliased Ahem, as WPT's selection reftests draw it: its glyphs end at their advances, where
//! the selection's stretches end.
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
  body{margin:0;background:white;font:20px/16px Ahem;color:black;text-underline-offset:6px} p{margin:0 0 10px}</style>";

/// Renders `body` as a live page, the text of the element with id `start` selected from UTF-16
/// offset `from` and that of `end` up to `to` (their first text children).
fn render(body: &str, selection: Option<(&str, u32, &str, u32)>) -> RgbaImage {
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", &format!("{STYLE}{body}"));
    let d = dom.borrow();
    let text = |id: &str| {
        let element = d.get_element_by_id(doc, id).unwrap().unwrap();
        render_key(d.first_child(element).unwrap().unwrap())
    };
    let selection =
        selection.map(|(start, from, end, to)| TextSelection { start: (text(start), from), end: (text(end), to) });
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
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
        selection,
        ..Default::default()
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(AhemFetch);
    let mut renderer = Renderer::new();
    renderer.text_antialiasing = false;
    renderer.render(&snap, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn differing(a: &RgbaImage, b: &RgbaImage) -> usize {
    assert_eq!((a.width, a.height), (b.width, b.height));
    a.data.chunks(4).zip(b.data.chunks(4)).filter(|(p, q)| p[..3] != q[..3]).count()
}

/// `test` with `selection` renders as `reference` without one, and unlike `test` without it.
fn matches(test: &str, selection: (&str, u32, &str, u32), reference: &str) {
    let (t, r, p) = (render(test, Some(selection)), render(reference, None), render(test, None));
    assert_eq!(differing(&t, &r), 0, "{test} with {selection:?}\n  differs from the reference\n{reference}");
    assert!(differing(&t, &p) > 0, "{test}\n  renders the same with {selection:?}");
}

#[test]
fn selected_text_takes_the_selection_colour() {
    // `::selection` without a background: none.
    matches(
        "<style>p::selection{color:#0a0}</style><p id=a>Some text</p>",
        ("a", 2, "a", 7),
        "<p>So<span style='color:#0a0'>me te</span>xt</p>",
    );
    // From one element to another, with a background.
    matches(
        "<style>::selection{color:#fff;background:#00f}</style><p id=a>First</p><p id=b>Second</p>",
        ("a", 3, "b", 2),
        "<p>Fir<span style='color:#fff;background:#00f'>st</span></p><p><span style='color:#fff;background:#00f'>Se</span>cond</p>",
    );
}

#[test]
fn text_without_a_selection_style_takes_the_system_highlight() {
    matches(
        // (Ahem's glyphs cover their em boxes: the background shows between the words.)
        "<p><span id=a>Sel ected</span> not</p>",
        ("a", 0, "a", 9),
        "<p><span style='color:HighlightText;background:Highlight'>Sel ected</span> not</p>",
    );
}

#[test]
fn decorations_of_the_text_colour_take_the_selection_colour() {
    let style = "<style>p::selection{color:#0a0}</style>";
    matches(
        &format!("{style}<p id=a style='text-decoration:underline'>Underlined</p>"),
        ("a", 0, "a", 10),
        "<p style='text-decoration:underline;color:#0a0'>Underlined</p>",
    );
    // A decoration of its own colour stays one line in it.
    matches(
        &format!("{style}<p id=a style='text-decoration:underline #00f'>Underlined</p>"),
        ("a", 2, "a", 6),
        "<p style='text-decoration:underline #00f'>Un<span style='color:#0a0'>derl</span>ined</p>",
    );
}

#[test]
fn unselectable_text_is_not_highlighted() {
    matches(
        "<p><span id=a>Selected</span> <span id=b style='user-select:none'>kept</span></p>",
        ("a", 0, "b", 4),
        "<p><span style='color:HighlightText;background:Highlight'>Selected </span><span>kept</span></p>",
    );
}
