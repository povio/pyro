//! Round-4 lane A (css-text, css-inline): `word-break: auto-phrase` (phrase breaks after hiragana
//! before kanji or katakana, `normal` breaks only for a line whose phrase does not fit), `<wbr>`,
//! the quirks-mode `<br>` line height, per-paragraph direction under `unicode-bidi: plaintext`,
//! and soft hyphens that end no line.
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

/// Each element's `[x, y, width, height]`.
fn boxes(html: &str, ids: &[&str]) -> Vec<[f64; 4]> {
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", html);
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
        quirks_mode: !html.starts_with("<!doctype"),
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

const STYLE: &str = "<!doctype html><style>body{margin:0;font:20px/40px monospace}</style>";

fn height(body: &str) -> f64 {
    boxes(&format!("{STYLE}{body}"), &["d"])[0][3]
}

#[test]
fn auto_phrase_breaks_between_phrases_only() {
    // keep-all keeps the sentence on one line at min-content; auto-phrase breaks after へ.
    let keep_all = height("<div id=d lang=ja style='width:min-content;word-break:keep-all'>東京へ行きましょう。</div>");
    let phrase = height("<div id=d lang=ja style='width:min-content;word-break:auto-phrase'>東京へ行きましょう。</div>");
    assert_eq!(keep_all, 40.0);
    assert_eq!(phrase, 80.0);
    // No phrase break across a no-break space.
    let glued = height("<div id=d lang=ja style='width:min-content;word-break:auto-phrase'>東京&nbsp;へ&nbsp;行きましょう。</div>");
    assert_eq!(glued, 40.0);
}

#[test]
fn auto_phrase_falls_back_to_normal_breaks_for_a_phrase_that_does_not_fit() {
    // ドライブ is one phrase; at 3em it breaks as under `normal`.
    let h = height("<div id=d lang=ja style='width:3em;word-break:auto-phrase'>ドライブ</div>");
    assert_eq!(h, 80.0);
    // Soft hyphens are such fallbacks too: not at min-content, but at width 0.
    let min = height("<div id=d lang=en style='width:min-content;word-break:auto-phrase'>con&shy;sid</div>");
    let zero = height("<div id=d lang=en style='width:0;word-break:auto-phrase'>con&shy;sid</div>");
    assert_eq!(min, 40.0);
    assert_eq!(zero, 80.0);
}

#[test]
fn wbr_is_a_soft_wrap_opportunity() {
    assert_eq!(height("<div id=d style='width:min-content'>abc<wbr>def</div>"), 80.0);
    assert_eq!(height("<div id=d style='width:min-content'>abcdef</div>"), 40.0);
    assert_eq!(height("<div id=d lang=ja style='width:min-content;word-break:keep-all'>東京へ<wbr>行きましょう。</div>"), 80.0);
}

#[test]
fn hyphens_none_soft_hyphens_do_not_narrow_min_content() {
    assert_eq!(height("<div id=d style='width:min-content;hyphens:none'>con&shy;sid</div>"), 40.0);
    assert_eq!(height("<div id=d style='width:min-content'>con&shy;sid</div>"), 80.0);
}

#[test]
fn quirks_mode_br_alone_sets_its_line_height() {
    let quirks = "<html><style>body{margin:0} div{font:20px/1 monospace} br{line-height:200px}</style>";
    let h = |doc: &str| boxes(&format!("{doc}<div id=d><br>X</div>"), &["d"])[0][3];
    assert_eq!(h(quirks), 220.0);
    // Standards mode: the strut, not the `<br>`'s line height.
    assert_eq!(h(&format!("<!doctype html>{}", &quirks[6..])), 40.0);
    // A line with text: the `<br>`'s inherited line height does not count.
    let inherited = "<html><style>body{margin:0} .o{font:20px/100px monospace} .b{display:inline-block}</style>\
        <div class=o><div class=b id=b><span style='line-height:20px'>X</span><br><span style='line-height:40px'>X</span></div></div>";
    assert_eq!(boxes(inherited, &["b"])[0][3], 60.0);
}

#[test]
fn plaintext_paragraphs_take_their_own_direction() {
    let b = boxes(
        &format!("{STYLE}<pre dir=auto style='width:300px;text-align:start;font:20px/40px monospace'><span id=a>abc</span>\n&rlm;<span id=b>xyz</span></pre>"),
        &["a", "b"],
    );
    assert_eq!(b[0][0], 0.0);
    assert!(b[1][0] > 200.0, "{b:?}");
}

#[test]
fn a_line_of_empty_inline_boxes_is_invisible() {
    assert_eq!(height("<div id=d><span></span></div>"), 0.0);
    assert_eq!(height("<div id=d><ruby></ruby></div>"), 0.0);
    // With text on the line, an empty box's strut still counts.
    assert_eq!(height("<div id=d><span style='line-height:60px'></span>X</div>"), 60.0);
}

#[test]
fn first_line_font_size_and_line_height_apply_in_layout() {
    // ::first-line doubles the font: its line is 80px tall, the second 40px.
    let h = height("<style>#d::first-line{font-size:40px}</style><div id=d style='font:20px/2 monospace'>Line1<br>Line2</div>");
    assert_eq!(h, 120.0);
    // A paragraph that wraps: the first line takes the ::first-line line height only.
    let h = height(
        "<style>#d::first-line{line-height:60px}</style><div id=d style='font:20px/20px monospace;width:5ch'>aaaa bbbb cccc</div>",
    );
    assert_eq!(h, 100.0);
    // An `em` border on the first line follows the first-line font size.
    let b = boxes(
        &format!("{STYLE}<style>#d::first-line{{font-size:10px}} #s{{border-left:solid 1em}}</style><div id=d style='font:40px/1 monospace'><span id=s>x</span></div>"),
        &["s"],
    );
    assert!((b[0][2] - (10.0 + 6.0)).abs() < 1.5, "{b:?}");
}
