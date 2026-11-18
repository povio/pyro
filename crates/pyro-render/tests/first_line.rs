//! `::first-line`, compared as reftests are: a page and its reference render pixel for pixel
//! alike in the same renderer (and unlike the page without the feature).
//!
//! - The first line of a block container (or of its first block child) is painted as if its
//!   content were in an inline box of the `::first-line` style: text that inherits its colour
//!   takes the first line's, decorations of the first line are drawn under the line's text, its
//!   background is a box around the line's content.
//! - Elements on the first line inherit through it: their `currentcolor` is the first line's
//!   colour unless they set their own.
//! - Background images of `::first-line` and `::first-letter` load.
//! - A block's first line can be in the anonymous block of an inline split around a block; the
//!   anonymous block after that block has no first line of the block (no `text-indent` either).
//! - A persistent document paints a changed `::first-line` style (it restyles the first-line box
//!   without laying the line out again).
use std::sync::Arc;

use pyro_render::*;

/// Serves `data:image/svg+xml,` URLs (percent-encoded SVG), on another thread as a page's loads
/// arrive; anything else fails.
struct DataUrls;
impl Fetcher for DataUrls {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        std::thread::spawn(move || match url.strip_prefix("data:image/svg+xml,") {
            Some(svg) => done(Ok(svg.replace("%23", "#").replace("%3C", "<").replace("%3E", ">").into_bytes())),
            None => done(Err("offline".into())),
        });
    }
}

/// A 20x20 green square.
const SQUARE: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='20' height='20'%3E%3Crect width='20' height='20' fill='%2300a000'/%3E%3C/svg%3E";

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(DataUrls);
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><style>body{{margin:0;background:white;font:20px/30px sans-serif;white-space:nowrap}}</style>{body}"
        ),
        base_url: "http://test/".into(),
        width: 400,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// How many pixels differ between two renders.
fn differing(a: &RgbaImage, b: &RgbaImage) -> usize {
    assert_eq!((a.width, a.height), (b.width, b.height));
    a.data.chunks(4).zip(b.data.chunks(4)).filter(|(p, q)| p[..3] != q[..3]).count()
}

/// `test` renders as `reference` does, and unlike `plain` (the page without the feature).
fn matches(test: &str, reference: &str, plain: &str) {
    let (t, r, p) = (render(test), render(reference), render(plain));
    assert_eq!(differing(&t, &r), 0, "{test}\n  differs from the reference\n{reference}");
    assert!(differing(&t, &p) > 0, "{test}\n  renders as the page without the feature\n{plain}");
}

#[test]
fn the_first_line_has_its_own_colour() {
    let style = "<style>div::first-line{color:#0a0}</style>";
    // Text that inherits the colour takes the first line's (`inherit` too); an element with its
    // own keeps it; the next line has the block's.
    matches(
        &format!("{style}<div>Text <b>bold</b> <i style='color:#00f'>own</i> <u style='color:inherit'>in</u><br>next</div>"),
        "<div><span style='color:#0a0'>Text <b>bold</b> </span><i style='color:#00f'>own</i><span style='color:#0a0'> <u>in</u></span><br>next</div>",
        "<div>Text <b>bold</b> <i style='color:#00f'>own</i> <u style='color:inherit'>in</u><br>next</div>",
    );
    // In the block's first block child.
    matches(
        &format!("{style}<div><p style='margin:0'>Text<br>next</p></div>"),
        "<div><p style='margin:0'><span style='color:#0a0'>Text</span><br>next</p></div>",
        "<div><p style='margin:0'>Text<br>next</p></div>",
    );
}

#[test]
fn the_first_line_box_has_a_background_and_decorations() {
    matches(
        "<style>div::first-line{background:#0a0;text-decoration:underline}</style><div>Text <b>bold</b><br>next</div>",
        "<div><span style='background:#0a0;text-decoration:underline'>Text <b>bold</b></span><br>next</div>",
        "<div>Text <b>bold</b><br>next</div>",
    );
}

#[test]
fn elements_on_the_first_line_inherit_through_it() {
    // The span's `currentcolor` (its gradient and its border) is the first line's colour.
    let span = "background-image:linear-gradient(currentcolor,currentcolor);border-bottom:3px solid";
    matches(
        &format!("<style>div{{color:#f00}} div::first-line{{color:#0a0}} span{{{span}}}</style><div><span>Green</span></div>"),
        &format!("<style>span{{{span}}}</style><div style='color:#0a0'><span>Green</span></div>"),
        &format!("<style>div{{color:#f00}} span{{{span}}}</style><div><span>Green</span></div>"),
    );
}

#[test]
fn first_line_and_first_letter_background_images_load() {
    let image = format!("background:url(\"{SQUARE}\") no-repeat");
    matches(
        &format!("<style>div::first-line{{{image}}}</style><div>Text</div>"),
        &format!("<style>span{{{image}}}</style><div><span>Text</span></div>"),
        "<div>Text</div>",
    );
    matches(
        &format!("<style>div::first-letter{{{image}}}</style><div>Text</div>"),
        &format!("<style>span{{{image}}}</style><div><span>T</span>ext</div>"),
        "<div>Text</div>",
    );
}

#[test]
fn the_first_line_of_a_block_can_be_in_a_split_inline() {
    // The block's first line is the head of an inline split around a block (an anonymous block
    // of the inline's first piece): only that line is the first line.
    let style = "<style>div::first-line{color:#0a0}</style>";
    matches(
        &format!("{style}<div><span>First<br>second<p style='margin:0'>Block</p>after</span></div>"),
        "<div><span><span style='color:#0a0'>First</span><br>second<p style='margin:0'>Block</p>after</span></div>",
        "<div><span>First<br>second<p style='margin:0'>Block</p>after</span></div>",
    );
}

#[test]
fn text_indent_indents_only_first_formatted_lines() {
    // The anonymous block after the inline's block is not the block's first line (CSS 2.1 16.1);
    // the block inside the inline has a first line of its own.
    let indent = "text-indent:40px";
    matches(
        &format!("<div style='{indent}'><span>First<p style='margin:0'>Block</p>after</span></div>"),
        &format!("<div style='{indent}'>First</div><div style='{indent}'>Block</div><div>after</div>"),
        &format!("<div style='{indent}'>First</div><div style='{indent}'>Block</div><div style='{indent}'>after</div>"),
    );
}

/// A live page (`Live` snapshots) rendered, changed and rendered again: the persistent document's
/// render equals a fresh one's.
#[test]
fn a_changed_first_line_style_is_painted_in_a_persistent_document() {
    use pyro_dom::parse::{parse_document, DomRef};
    use pyro_dom::snapshot::{RenderHtml, RenderNodeKind};
    use std::{cell::RefCell, rc::Rc};
    let html = "<!doctype html><style>body{margin:0;background:white;font:20px/30px sans-serif}
      div::first-line{color:#0a0} .b::first-line{color:#00f}</style><div id=d>Text<br>next</div>";
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", html);
    let opts = RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None };
    let snapshot = |dom: &DomRef| {
        let tree = dom.borrow().render_tree(doc, &opts).unwrap();
        let nodes = tree
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
            .collect();
        Snapshot {
            base_url: "http://test/".into(),
            width: 200,
            height: 100,
            device_scale_factor: 1.0,
            resource_scope: "doc".into(),
            live: Some(Live { page: 1, tree: Arc::new(Tree { nodes }), clip: None, interaction: None }),
            ..Default::default()
        }
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(DataUrls);
    let mut live = Renderer::new();
    let before = live.paint(&snapshot(&dom), Area::Viewport, &fetcher).unwrap();
    let div = dom.borrow().get_element_by_id(doc, "d").unwrap().unwrap();
    dom.borrow_mut().set_attribute(div, "class", "b").unwrap();
    let after = live.paint(&snapshot(&dom), Area::Viewport, &fetcher).unwrap();
    let fresh = Renderer::new().paint(&snapshot(&dom), Area::Viewport, &fetcher).unwrap();
    assert_eq!(after.hash, fresh.hash, "the persistent document paints the new style");
    assert_ne!(after.hash, before.hash);
}

/// An inline-block `::before` is atomic: the `::first-line` colour does not reach its text.
#[test]
fn first_line_skips_an_inline_block_before() {
    matches(
        "<style>div::first-line{color:#f00} div::before{content:'ab';color:#080;display:inline-block}</style><div>cd</div>",
        "<div><span style='color:#080'>ab</span><span style='color:#f00'>cd</span></div>",
        "<div><span style='color:#f00'>abcd</span></div>",
    );
}

/// A `::first-letter` box inside the `::first-line` box (here of a `<q>`'s open quote, with
/// `vertical-align` and `opacity` so that it is painted on its own) is reparented through the
/// first-line box without a panic (WPT text-decoration-first-line-layer-crash): an anonymous
/// box in another one originates from the outer box.
#[test]
fn first_letter_in_first_line_box_paints() {
    let image = render(
        "<style>.c{float:right;text-decoration:overline}.c::first-letter{vertical-align:super;opacity:.1}\
         .c::first-line{text-decoration:overline}</style><q class=c>appxgrxebpwykansiejjsh</q>",
    );
    assert!(image.data.chunks(4).any(|p| p[..3] != [255, 255, 255]), "the text is painted");
}

/// A paragraph of more empty spans than parley has style ids (`u16`; each empty span puts an
/// empty-box strut in its own style) is laid out and painted instead of panicking with "too many
/// styles" (pyro-browser `review_a_journal_overflow_falls_back_to_every_candidate`).
#[test]
fn more_inline_spans_than_style_ids_paint() {
    let image = render(&format!("{}<p>end", "<span></span>".repeat(66_000)));
    assert!(image.data.chunks(4).any(|p| p[..3] != [255, 255, 255]), "the text is painted");
}
