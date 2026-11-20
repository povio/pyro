//! Soft hyphens (U+00AD; reftest lane T): a line that wraps at one shows the hyphen of the font
//! there and has room for it, one inside a line shows nothing, `hyphens: none` lets no line end
//! there, and `hyphenate-character` names the character shown (none for the empty string).
//! Measured on an inline box around the text up to the soft hyphen (`u`): its width is the
//! text's with what the soft hyphen shows, in a monospace font, so each character is one `ch`
//! whatever the machine's font. A block's height counts its 40px lines.
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

const STYLE: &str = "<!doctype html><style>body{margin:0;font:20px/40px monospace}
  div{width:6.5ch} b{position:absolute}</style>";

/// Each element's `[width, height]`.
fn sizes(body: &str, ids: &[&str]) -> Vec<[f64; 2]> {
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
            [b.width, b.height]
        })
        .collect()
}

/// The width of one character.
fn ch() -> f64 {
    sizes("<u id=c>x</u>", &["c"])[0][0]
}

#[track_caller]
fn assert_near(a: f64, b: f64) {
    assert!((a - b).abs() < 0.2, "{a} is not {b}");
}

#[test]
fn a_line_that_wraps_at_a_soft_hyphen_shows_a_hyphen() {
    let ch = ch();
    // "high-" / "way": the first line is five characters wide with its hyphen.
    let s = sizes(
        "<div id=d><u id=a>high&shy;</u>way</div><div id=e style='width:auto'><u id=b>high&shy;</u>way</div>",
        &["a", "d", "b", "e"],
    );
    assert_near(s[0][0], 5.0 * ch);
    assert_eq!(s[1][1], 80.0);
    // On one line the soft hyphen takes no room.
    assert_near(s[2][0], 4.0 * ch);
    assert_eq!(s[3][1], 40.0);
}

#[test]
fn the_hyphen_needs_room_on_its_line() {
    let ch = ch();
    // "abcde-" fits 6.5ch but not 5.5ch, where the line ends at the first soft hyphen instead.
    let s = sizes(
        "<div><u id=a>abc&shy;de&shy;</u>fg</div><div style='width:5.5ch'><u id=b>abc&shy;</u>de&shy;fg</div>",
        &["a", "b"],
    );
    assert_near(s[0][0], 6.0 * ch);
    assert_near(s[1][0], 4.0 * ch);
    // With no earlier place to break, the line ends at the soft hyphen though the hyphen overflows.
    let s = sizes("<div id=d style='width:0'>ab&shy;cd</div>", &["d"]);
    assert_eq!(s[0][1], 80.0);
}

#[test]
fn hyphens_none_keeps_the_word_together() {
    let ch = ch();
    let s = sizes("<div id=d style='hyphens:none'><u id=a>high&shy;</u>way</div>", &["a", "d"]);
    assert_near(s[0][0], 4.0 * ch);
    assert_eq!(s[1][1], 40.0);
}

#[test]
fn hyphenate_character_names_what_is_shown() {
    let ch = ch();
    let s = sizes(
        "<div id=d style='hyphenate-character:\"\"'><u id=a>high&shy;</u>way</div>\
         <div style='hyphenate-character:\"=\"'><u id=b>high&shy;</u>way</div>",
        &["a", "d", "b"],
    );
    // Nothing shown: the first line is its four letters. A character of the font: five.
    assert_near(s[0][0], 4.0 * ch);
    assert_eq!(s[1][1], 80.0);
    assert_near(s[2][0], 5.0 * ch);
}

#[test]
fn an_out_of_flow_box_beside_the_soft_hyphen_changes_nothing() {
    let ch = ch();
    let s = sizes(
        "<div id=d><u id=a>high&shy;<b>x</b></u>way</div><div id=e><u id=b>high<b>x</b>&shy;</u>way</div>",
        &["a", "d", "b", "e"],
    );
    assert_near(s[0][0], 5.0 * ch);
    assert_near(s[2][0], 5.0 * ch);
    assert_eq!((s[1][1], s[3][1]), (80.0, 80.0));
}

#[test]
fn a_hyphenate_character_string_shows_all_its_characters() {
    let ch = ch();
    let s = sizes("<div id=d style='width:7.5ch;hyphenate-character:\"/-/\"'><u id=a>high&shy;</u>ways</div>", &["a", "d"]);
    assert_near(s[0][0], 7.0 * ch);
    assert_near(s[1][1], 80.0);
}

#[test]
fn cached_arabic_glyphs_respect_isolating_inline_edges() {
    let widths = sizes("<section dir=rtl style='font:40px sans-serif'><p>ع<u style='color:red'>ع</u>ع</p><p>ع<u id=a style='padding:4px'>ع</u>ع</p><p>ع&zwnj;<u id=b style='padding:4px'>&zwnj;ع&zwnj;</u>&zwnj;ع</p></section>", &["a", "b"]);
    assert_near(widths[0][0], widths[1][0]);
}

#[test]
fn an_unused_soft_hyphen_preserves_cursive_joining() {
    let widths = sizes(
        "<div dir=rtl style='width:auto;font:32px sans-serif'><u id=a>قىل&shy;مىغانلىقى</u><br><u id=b>قىلمىغانلىقى</u></div>",
        &["a", "b"],
    );
    assert_near(widths[0][0], widths[1][0]);
}

#[test]
fn discretionary_shaping_preserves_ligatures_and_composes_replacements() {
    let render = |width: u32, replacement: &str, body: &str| {
        let snapshot = Snapshot {
            html: format!(
                "<!doctype html><style>body{{margin:0}}div{{direction:rtl;margin-left:120px;width:{width}px;font:32px/48px sans-serif;hyphenate-character:'{replacement}'}}</style><div>{body}</div>"
            ),
            base_url: "http://test/".into(),
            width: 450,
            height: 110,
            device_scale_factor: 1.,
            ..Default::default()
        };
        let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
        Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0.data
    };
    assert!(render(280, "-", "ل&shy;اميميميم") == render(280, "-", "لاميميميم"));
    assert!(render(56, "-", "ل&shy;اميميميم") == render(56, "-", "ل-<br>اميميميم"));
    assert!(render(56, "e\u{301}", "قل&shy;ميميميم") == render(56, "é", "قل&shy;ميميميم"));
}
