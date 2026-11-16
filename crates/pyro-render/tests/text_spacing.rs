//! CSS Text 4 spacing of CJK text (reftest lane TS). `text-spacing-trim`: fullwidth punctuation
//! next to punctuation is set half-width with the font's `halt` (an opening bracket after an
//! opening or closing one or a middle dot, a closing bracket before a closing one or a middle dot),
//! opening brackets at a line's start as the value says (`normal` keeps them, `trim-start` trims
//! them, `space-first` trims them except on the first line and after a forced break), and closing
//! brackets that do not fit at a line's end when that makes the line fit. `text-autospace`: 1/8 em
//! between ideographs and letters or numerals, dropped at a line's end. Measured on inline boxes
//! (`span`s) in 20px Noto Sans CJK JP (a subset with `halt`), whose ideographs and fullwidth
//! punctuation are 20px wide, and on `b` markers (empty inline-blocks) that sit after text.
use std::{cell::RefCell, rc::Rc, sync::Arc};

use pyro_dom::{
    parse::{parse_document, DomRef},
    snapshot::{render_key, RenderHtml, RenderNodeKind},
};
use pyro_render::*;

/// Noto Sans CJK JP, cut to ASCII, 国, 水, （）「」。、：・ and the ideographic space, with `halt`.
const NOTO: &[u8] = include_bytes!("fonts/NotoSansCJKjp-halt-subset.otf");

struct Fonts;
impl Fetcher for Fonts {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        if url.ends_with("/noto.otf") {
            done(Ok(NOTO.to_vec()))
        } else {
            done(Err("offline".into()))
        }
    }
}

const STYLE: &str = "<!doctype html><style>@font-face{font-family:noto;src:url(noto.otf)}
  body{margin:0;font:20px/40px noto} b{display:inline-block;width:0;height:10px}</style>";

/// The border box of each element.
fn boxes(body: &str, ids: &[&str]) -> Vec<LayoutBox> {
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
        height: 400,
        device_scale_factor: 1.0,
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.sync_layout(&snap, &(Arc::new(Fonts) as Arc<dyn Fetcher>)).unwrap();
    keys.iter().map(|&key| renderer.element_box(1, key).unwrap()).collect()
}

fn widths(body: &str, ids: &[&str]) -> Vec<f64> {
    boxes(body, ids).iter().map(|b| b.width).collect()
}

#[track_caller]
fn assert_near(got: &[f64], want: &[f64]) {
    assert!(got.len() == want.len() && got.iter().zip(want).all(|(a, b)| (a - b).abs() < 0.02), "{got:?} != {want:?}");
}

#[test]
fn punctuation_next_to_punctuation_is_half_width() {
    let got = widths(
        "<p><span id=a>国（（国</span> <span id=b>国））国</span> <span id=c>国）（国</span> \
         <span id=d>国（）国</span> <span id=e>国）・国</span> <span id=f>国・（国</span> \
         <span id=g>国（・国</span> <span id=h>国)（国</span> <span id=i>国(（国</span></p>",
        &["a", "b", "c", "d", "e", "f", "g", "h", "i"],
    );
    let narrow = widths("<p><span id=p>(</span></p>", &["p"])[0];
    assert_near(&got, &[70.0, 70.0, 70.0, 80.0, 70.0, 70.0, 80.0, 60.0 + narrow, 50.0 + narrow]);
}

#[test]
fn space_all_and_width_features_leave_punctuation_alone() {
    let got = widths(
        "<p style='text-spacing-trim:space-all'><span id=a>国（（国</span></p>\
         <p style='font-feature-settings:\"qwid\"'><span id=b>国））国</span></p>\
         <p style='font-variant-east-asian:full-width'><span id=c>国（（国</span></p>",
        &["a", "b", "c"],
    );
    // Like `halt` and `palt`, `qwid` (which the font lacks) leaves the widths to the font;
    // `fwid` does not.
    assert_near(&got, &[80.0, 80.0, 70.0]);
}

#[test]
fn opening_brackets_at_line_starts_follow_the_value() {
    // The first line, a line wrapped after a closing bracket (four ideographs fill a line), a
    // line after a forced break.
    let body = |value: &str| {
        format!(
            "<div style='width:80px;text-spacing-trim:{value}'>（<b id=a></b>国国<br>国国国）（<b id=b></b>国国<br>\
             （<b id=c></b>国</div>"
        )
    };
    let x = |value: &str| boxes(&body(value), &["a", "b", "c"]).iter().map(|b| b.x).collect::<Vec<_>>();
    assert_near(&x("normal"), &[20.0, 20.0, 20.0]);
    assert_near(&x("trim-start"), &[10.0, 10.0, 10.0]);
    assert_near(&x("space-first"), &[20.0, 10.0, 20.0]);
}

#[test]
fn a_trimmed_opening_bracket_makes_room() {
    // 72px fits three and a half ideographs: only a half-width bracket lets the line hold four.
    let got = boxes(
        "<div id=d style='width:72px;text-spacing-trim:trim-start'>（国国国（国国国</div>",
        &["d"],
    );
    assert_near(&[got[0].height], &[80.0]);
    let got = boxes("<div id=d style='width:72px'>（国国国（国国国</div>", &["d"]);
    assert_near(&[got[0].height], &[120.0]);
}

#[test]
fn a_closing_bracket_that_does_not_fit_ends_its_line_half_width() {
    let got = boxes("<div id=d style='width:72px'>国国国）国国国）</div>", &["d"]);
    assert_near(&[got[0].height], &[80.0]);
    let got = boxes("<div id=d style='width:72px;text-spacing-trim:space-all'>国国国）国国国）</div>", &["d"]);
    assert_near(&[got[0].height], &[120.0]);
}

#[test]
fn ideographs_and_letters_are_spaced_an_eighth_em() {
    let got = widths(
        "<p style='text-autospace:normal'><span id=a>国X</span> <span id=b>X国</span> <span id=c>国1国</span> <span id=d>国。X</span>\
         <span id=e style='text-autospace:no-autospace'>国X</span></p>",
        &["a", "b", "c", "d", "e"],
    );
    let x = widths("<p><span id=x>X</span> <span id=one>1</span></p>", &["x", "one"]);
    assert_near(&got, &[22.5 + x[0], 22.5 + x[0], 45.0 + x[1], 40.0 + x[0], 20.0 + x[0]]);
}

#[test]
fn the_space_after_the_last_ideograph_of_a_line_goes() {
    // The fourth ideograph fits with its space: the line ends there, without the space.
    let got = widths("<div style='width:84px;text-autospace:normal'><span id=s>国国国国X国</span></div>", &["s"]);
    assert_near(&got, &[80.0]);
}

#[test]
fn justified_cjk_text_spreads_between_characters() {
    // Four ideographs (80px) in 100px: three opportunities share 20px (`text-justify: auto`).
    let x = |style: &str| {
        boxes(
            &format!(
                "<div style='width:100px;text-align:justify;text-align-last:justify;{style}'>\
                 <span id=a>国</span><span id=b>国</span><span id=c>国</span><span id=d>国</span></div>"
            ),
            &["a", "b", "c", "d"],
        )
        .iter()
        .map(|b| b.x)
        .collect::<Vec<_>>()
    };
    let third = 20.0 / 3.0;
    assert_near(&x(""), &[0.0, 20.0 + third, 40.0 + 2.0 * third, 80.0]);
    assert_near(&x("text-justify:inter-character"), &[0.0, 20.0 + third, 40.0 + 2.0 * third, 80.0]);
    // `none` justifies nothing: the line stays at the start.
    assert_near(&x("text-justify:none"), &[0.0, 20.0, 40.0, 60.0]);
    // `inter-word` only between words: none here.
    assert_near(&x("text-justify:inter-word"), &[0.0, 20.0, 40.0, 60.0]);
}

#[test]
fn white_space_changes_wrap_as_in_chromium() {
    // One ideograph per line where the line may wrap between the two.
    let lines = |body: &str| {
        boxes(&format!("<style>.nw{{white-space:nowrap}}.n{{white-space:normal}}</style><div id=d style='width:20px'>{body}</div>"), &["d"])[0].height / 40.0
    };
    assert_near(
        &[
            lines("<span class=nw>国</span>国"),
            lines("<span class=nw>国</span><span class=nw>国</span>"),
            lines("<span class=nw>国国</span>"),
            lines("<span class=nw>a </span>b"),
            lines("<span class=nw><span class=n>国</span>国</span>"),
            lines("<span class=nw>国<span class=n>国</span></span>"),
        ],
        &[2.0, 2.0, 1.0, 1.0, 2.0, 1.0],
    );
}
