//! Generated content and pseudo-elements, compared as reftests are: a page and its reference
//! render pixel for pixel alike in the same renderer (and unlike the page without the feature).
//!
//! - `::first-letter` styles the first letter of a block container's first line, with the
//!   punctuation before and after it (Unicode Ps, Pe, Pi, Pf, Po), also when the letter is in an
//!   inline, a `::before` or the block's first block child; out-of-flow boxes are passed over and an
//!   atomic inline before the text means there is no first letter.
//! - `counter()`, `counters()` and quotes in `content` are evaluated in tree order, with counter
//!   scopes (a counter ends with the parent of the element that made it) and counter styles.
//! - List markers: a `list-style-image` marker ends 7px before the item, its bottom on the first
//!   line's baseline; an inside marker of an item of blocks is a line before them; an item without
//!   content has the line its outside marker sits on; an outside marker goes with a line floats
//!   move.
use std::sync::Arc;

use pyro_render::*;

/// Serves `data:image/svg+xml,` URLs (percent-encoded SVG), on another thread as a page's loads
/// arrive; anything else fails.
struct DataUrls;
impl Fetcher for DataUrls {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        std::thread::spawn(move || match url.strip_prefix("data:image/svg+xml,") {
            Some(svg) => done(Ok(percent_decode(svg))),
            None => done(Err("offline".into())),
        });
    }
}

fn percent_decode(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
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

const LETTER: &str = "<style>div::first-letter{color:#0a0;font-size:36px;line-height:2}
  .ref{color:#0a0;font-size:36px;line-height:2}</style>";

#[test]
fn the_first_letter_takes_the_punctuation_around_it() {
    for (punctuation, letter, rest) in
        [(")", "T", "est"), ("\u{00BF}", "T", "est"), ("\"", "T", "his"), ("", "4", "2 x")]
    {
        matches(
            &format!("{LETTER}<div>{punctuation}{letter}{punctuation}{rest}</div>"),
            &format!("{LETTER}<div><span class=ref>{punctuation}{letter}{punctuation}</span>{rest}</div>"),
            &format!("<div>{punctuation}{letter}{punctuation}{rest}</div>"),
        );
    }
    // A dash joins the letter before it, not after it (CSS Pseudo 4; WPT css-pseudo
    // `first-letter-trailing-punctuation`).
    matches(
        &format!("{LETTER}<div>-T-est</div>"),
        &format!("{LETTER}<div><span class=ref>-T</span>-est</div>"),
        "<div>-T-est</div>",
    );
}

#[test]
fn the_first_letter_is_found_in_inlines_before_content_and_first_blocks() {
    // In an inline, after white space: it inherits from the inline.
    matches(
        &format!("{LETTER}<div> <span style='font-style:italic'> Filler</span></div>"),
        &format!("{LETTER}<div> <span style='font-style:italic'> <span class=ref>F</span>iller</span></div>"),
        "<div> <span style='font-style:italic'> Filler</span></div>",
    );
    // In `::before` content.
    matches(
        &format!("{LETTER}<style>div::before{{content:'Filler'}}</style><div></div>"),
        &format!("{LETTER}<div><span class=ref>F</span>iller</div>"),
        "<style>div::before{content:'Filler'}</style><div></div>",
    );
    // In the first block child.
    matches(
        &format!("{LETTER}<div><p style='margin:0'>Adiv</p></div>"),
        &format!("{LETTER}<section><p style='margin:0'><span class=ref>A</span>div</p></section>"),
        "<div><p style='margin:0'>Adiv</p></div>",
    );
    // Past an absolutely positioned box.
    matches(
        &format!("{LETTER}<div><span style='position:absolute;color:transparent'>F</span>PASS</div>"),
        &format!(
            "{LETTER}<div><span style='position:absolute;color:transparent'>F</span><span class=ref>P</span>ASS</div>"
        ),
        "<div><span style='position:absolute;color:transparent'>F</span>PASS</div>",
    );
}

#[test]
fn no_first_letter_after_an_atomic_inline() {
    let page = "<div><span style='display:inline-block;width:10px;height:10px;background:#00f'></span>Filler</div>";
    let (with, without) = (render(&format!("{LETTER}{page}")), render(page));
    assert_eq!(differing(&with, &without), 0);
}

#[test]
fn counters_are_scoped_and_ordered() {
    // CSS 2.1 counters-order-000: resets before increments, on the element before its `::before`.
    let spans = ["", "one", "two", "two one", "four", "four one", "eight", "eight two", "eight four two one"];
    let test: String =
        spans.iter().map(|class| format!("<span class=c><span class='{class}'></span></span> ")).collect();
    matches(
        &format!(
            "<style>span.c span::before{{content:counter(c)}} span.c{{counter-reset:c 0}} span.one{{counter-reset:c 1}}
             span.two{{counter-increment:c 2}} span.four::before{{counter-reset:c 4}} span.eight::before{{counter-increment:c 8}}
             span.c span::after{{counter-increment:c 100;counter-reset:c 200}}</style><div>{test}</div>"
        ),
        "<div>0 1 2 3 4 4 8 10 12 </div>",
        &format!("<div>{}</div>", " ".repeat(spans.len())),
    );
    // `counters()` joins the nested counters; a counter ends with its creator's parent.
    matches(
        "<style>ol{counter-reset:item;list-style:none;margin:0;padding:0} li::before{counter-increment:item;content:counters(item,'.') ' '}</style>
         <ol><li>a<ol><li>b</li><li>c</li></ol></li><li>d</li></ol>",
        "<style>ol{list-style:none;margin:0;padding:0}</style><ol><li>1 a<ol><li>1.1 b</li><li>1.2 c</li></ol></li><li>2 d</li></ol>",
        "<style>ol{list-style:none;margin:0;padding:0}</style><ol><li>a<ol><li>b</li><li>c</li></ol></li><li>d</li></ol>",
    );
    // A counter no element reset is 0; one in a `display: none` element does not count.
    matches(
        "<style>i{counter-increment:n 5;display:none} b::before{content:counter(n) counter(m)}</style><div><i></i><b></b></div>",
        "<div><b>00</b></div>",
        "<div><b></b></div>",
    );
}

#[test]
fn counter_styles() {
    matches(
        "<style>div{counter-reset:c 1994} div::before{content:counter(c,upper-roman) ' ' counter(c,lower-alpha) ' ' counter(c,decimal-leading-zero)}</style><div></div>",
        "<div>MCMXCIV bxr 1994</div>",
        "<div></div>",
    );
    // (Greek letters, from a fallback font, are checked in blitz-dom's `counters` tests.)
    matches(
        "<style>div{counter-reset:c 3} div::before{content:counter(c,lower-roman) counter(c,upper-latin) counter(c,decimal-leading-zero)}</style><div></div>",
        "<div>iiiC03</div>",
        "<div></div>",
    );
}

#[test]
fn quotes_nest_by_depth() {
    matches(
        "<style>q{quotes:'[' ']' '(' ')'}</style><div><q>a <q>b</q> c</q> <q>d</q></div>",
        "<div>[a (b) c] [d]</div>",
        "<style>q::before,q::after{content:none}</style><div><q>a <q>b</q> c</q> <q>d</q></div>",
    );
    // A close quote at depth 0 is nothing; `no-open-quote` counts.
    matches(
        "<style>b::before{content:close-quote no-open-quote open-quote} b{quotes:'<' '>' '{' '}'}</style><div><b>x</b></div>",
        "<div><b>{x</b></div>",
        "<div><b>x</b></div>",
    );
}

#[test]
fn auto_quotes_follow_the_parent_language() {
    // CLDR quotes, the language of the `<q>`'s parent (WPT `quotes-030`), a region falling back
    // to its language, an unknown language to English's.
    matches(
        "<div lang=fr-CH><q>a <q lang=ja>b <q lang=zz>c</q></q></q></div>",
        "<div>\u{AB}a \u{2039}b \u{300E}c\u{300F}\u{203A}\u{BB}</div>",
        "<div>\u{201C}a \u{2018}b \u{2018}c\u{2019}\u{2019}\u{201D}</div>",
    );
}

#[test]
fn inlines_in_inlines_have_their_pseudo_elements() {
    matches(
        "<style>.x::before{content:'X'} .y::after{content:'Y'}</style><p>a <span>b <b class=x>c</b></span> <span><span><i class=y>h</i></span></span></p>",
        "<p>a <span>b <b>Xc</b></span> <span><span><i>hY</i></span></span></p>",
        "<p>a <span>b <b>c</b></span> <span><span><i>h</i></span></span></p>",
    );
}

#[test]
fn content_url_is_an_image() {
    // The image arrives after the box was built: its load lays it out again.
    matches(
        &format!("<style>div::before{{content:'a' url(\"{SQUARE}\") 'b'}}</style><div>c</div>"),
        &format!("<div>a<img src=\"{SQUARE}\">bc</div>"),
        "<div>abc</div>",
    );
}

#[test]
fn a_list_item_pseudo_element_has_a_marker() {
    matches(
        "<style>div::after{content:'Filler';display:list-item;margin-left:2em}</style><div>Text</div>",
        "<div>Text<div style='display:list-item;margin-left:2em'>Filler</div></div>",
        "<style>div::after{content:'Filler';display:block;margin-left:2em}</style><div>Text</div>",
    );
}

#[test]
fn a_page_can_show_area_and_color_its_body_text() {
    matches(
        "<style>area{display:block} area::before{content:'nohref'}</style><map><area alt=''></map>",
        "<div>nohref</div>",
        "<style>area::before{content:'nohref'}</style><map><area alt=''></map>",
    );
    matches("<body text=green>x</body>", "<body style='color:green'>x</body>", "<body>x</body>");
}

#[test]
fn an_image_marker_ends_7px_before_the_item_on_its_first_baseline() {
    // The image arrives after the item was built: its load lays the item out again.
    matches(
        &format!("<style>div{{list-style-image:url(\"{SQUARE}\")}}</style><div style='display:list-item;margin-left:40px'>Text</div>"),
        &format!("<div style='margin-left:13px'><img src=\"{SQUARE}\" style='margin-right:7px'>Text</div>"),
        "<div style='display:list-item;margin-left:40px;list-style:none'>Text</div>",
    );
}

#[test]
fn an_inside_marker_is_a_line_before_block_content() {
    matches(
        "<ol style='list-style-position:inside;margin:0;padding:0'><li><div>Text</div></li></ol>",
        "<div>1. </div><div>Text</div>",
        "<ol style='list-style:none inside;margin:0;padding:0'><li><div>Text</div></li></ol>",
    );
}

#[test]
fn an_empty_item_has_the_line_of_its_outside_marker() {
    // Its marker on that line's baseline, and the next box after the line.
    matches(
        "<ol style='margin:0 0 0 40px;padding:0'><li></li></ol><div>Text</div>",
        "<ol style='margin:0 0 0 40px;padding:0'><li>&nbsp;</li></ol><div>Text</div>",
        "<ol style='margin:0 0 0 40px;padding:0;list-style:none'><li></li></ol><div>Text</div>",
    );
}

#[test]
fn an_outside_marker_goes_with_the_line_a_float_moves() {
    // A number and a symbol (they are painted apart), beside a float as tall as both lines.
    let float = "<div style='float:left;width:60px;height:60px'></div>";
    let lists = |margin: &str, style: &str| {
        format!(
            "{float}<ol style='margin:0 0 0 {margin};padding:0;{style}'><li>Text</li></ol>\
             <ul style='margin:0 0 0 {margin};padding:0;list-style-type:square;{style}'><li>Text</li></ul>"
        )
    };
    matches(&lists("0", ""), &lists("60px", ""), &lists("0", "list-style:none"));
}
