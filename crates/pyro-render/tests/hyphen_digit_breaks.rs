//! Chrome's soft wrap opportunity between a hyphen and a digit
//! (`DocumentConfig::line_break_override` = `parley::HYPHEN_DIGIT_LINE_BREAK_OVERRIDE`,
//! `Renderer::hyphen_digit_breaks`, `serve --hyphen-digit-breaks`): off, UAX #14 LB25 keeps
//! `HY × NU`; on, a hyphen after an ASCII letter or digit may end a line before a digit, while a
//! minus sign stays with its number. Line breaking and min-content widths must agree.
//!
//! Line counts are a box's height over its line height in a zero-width container, where every
//! opportunity ends a line, so they do not depend on the machine's fonts.
use std::sync::Arc;

use blitz_dom::DocumentConfig;
use blitz_html::HtmlDocument;
use pyro_render::{Area, Fetcher, Renderer, Snapshot};

const LINE: f32 = 20.0;

fn document(html: &str, on: bool) -> HtmlDocument {
    let config = DocumentConfig {
        line_break_override: on.then_some(parley::HYPHEN_DIGIT_LINE_BREAK_OVERRIDE),
        ..DocumentConfig::default()
    };
    let mut doc = HtmlDocument::from_html(html, config);
    doc.viewport_mut().window_size = (800, 600);
    doc.resolve(0.0);
    doc
}

fn size(doc: &HtmlDocument, selector: &str) -> (f32, f32) {
    let id = doc.query_selector(selector).unwrap().unwrap_or_else(|| panic!("no {selector}"));
    let s = doc.get_node(id).unwrap().unrounded_layout().size;
    (s.width, s.height)
}

/// Lines `text` takes in a zero-width block, without and with the setting.
fn lines(text: &str, style: &str) -> (u32, u32) {
    let html = format!(
        "<!doctype html><style>body{{margin:0;font:16px/{LINE}px sans-serif}}</style>\
         <div style='width:0'><div id=t style='{style}'>{text}</div></div>"
    );
    let count = |on| {
        let (_, h) = size(&document(&html, on), "#t");
        let n = h / LINE;
        assert!((n - n.round()).abs() < 0.01, "{text:?}: height {h} is not whole lines");
        n.round() as u32
    };
    (count(false), count(true))
}

#[test]
fn a_hyphen_after_a_letter_or_digit_breaks_before_a_digit_only_when_enabled() {
    assert_eq!(lines("4b87-916e", ""), (1, 2));
    assert_eq!(lines("8cfb1ebe-8c20", ""), (1, 2));
    assert_eq!(lines("10-20", ""), (1, 2));
    // Chrome: REG- / be08b23e- / 89be- / 48b8- / 9a6f- / ecb23f7628c6. Without the setting only
    // the hyphens before a letter break: REG- / be08b23e-89be-48b8-9a6f- / ecb23f7628c6.
    assert_eq!(lines("REG-be08b23e-89be-48b8-9a6f-ecb23f7628c6", ""), (3, 6));
    assert_eq!(lines("London 6bc72a51-5164-44cb", ""), (2, 4));
    // Markup between the characters changes nothing: breaks are found over the paragraph.
    assert_eq!(lines("4b87<b>-</b><i>916e</i>", ""), (1, 2));
}

#[test]
fn a_minus_sign_stays_with_its_number() {
    // `a--5`: the second hyphen follows a hyphen, not a letter or digit.
    for text in ["-120,00 EUR", "-5", "A -5", "(1 - 95)", "1 - 95", "x (-5)", "x/-5", "€-12", "a--5"] {
        let (off, on) = lines(text, "");
        assert_eq!(off, on, "{text:?}");
    }
    assert_eq!(lines("-120,00 EUR", ""), (2, 2));
    assert_eq!(lines("A -5", ""), (2, 2));
}

#[test]
fn other_hyphens_and_nowrap_are_unchanged() {
    // U+2011 NON-BREAKING HYPHEN, U+2010 HYPHEN, a soft hyphen before a digit, U+2212 MINUS SIGN.
    for text in ["4b87\u{2011}916e", "4b87\u{2010}916e", "4b87&shy;916e", "4b87\u{2212}916e", "4b87&shy;-916e"] {
        let (off, on) = lines(text, "");
        assert_eq!(off, on, "{text:?}");
    }
    assert_eq!(lines("REG-be08b23e-89be-48b8-9a6f-ecb23f7628c6", "white-space:nowrap"), (1, 1));
    assert_eq!(lines("10-20 30-40", "white-space:pre"), (1, 1));
    assert_eq!(lines("10-20", "text-wrap-mode:nowrap"), (1, 1));
    // `line-break: anywhere` already breaks everywhere; the setting removes nothing.
    assert_eq!(lines("-10-20", "line-break:anywhere"), (6, 6));
}

const TABLE: &str = "<!doctype html><style>body{margin:0;font:16px/20px sans-serif}\
    table{border-spacing:0;width:1px}td{padding:0}span{display:inline-block;white-space:nowrap}</style>\
    <table><tr><td id=id>8cfb1ebe-8c20-4b87-916e</td><td id=label>Ref</td></tr></table>\
    <span id=piece>8cfb1ebe-</span><span id=whole>8cfb1ebe-8c20-4b87-916e</span>\
    <div style='display:flex;width:0'><div id=item>10-20-30</div></div>\
    <span id=ten>10-</span><span id=all>10-20-30</span>";

#[test]
fn table_columns_and_flex_items_shrink_to_the_same_breaks() {
    let off = document(TABLE, false);
    let on = document(TABLE, true);
    // Off, the identifier has no break: the column is as wide as all of it.
    assert!((size(&off, "#id").0 - size(&off, "#whole").0).abs() < 0.01);
    // On, its min-content width is its widest piece, "8cfb1ebe-", and it takes four lines.
    let (w, h) = size(&on, "#id");
    assert!((w - size(&on, "#piece").0).abs() < 0.01, "column {w} vs piece {}", size(&on, "#piece").0);
    assert_eq!(h, 4.0 * LINE);
    assert_eq!(size(&off, "#id").1, LINE);
    // The other column keeps its width (its row is taller).
    assert_eq!(size(&off, "#label").0, size(&on, "#label").0);
    // A flex item's automatic minimum size is its min-content width.
    assert!((size(&off, "#item").0 - size(&off, "#all").0).abs() < 0.01);
    assert!((size(&on, "#item").0 - size(&on, "#ten").0).abs() < 0.01);
    assert_eq!(size(&on, "#item").1, 3.0 * LINE);
}

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

#[test]
fn the_renderer_setting_reaches_its_documents() {
    let snap = Snapshot {
        html: "<!doctype html><style>body{margin:0;font:16px/100px sans-serif}</style>\
               <div style='width:0'>REG-be08b23e-89be-48b8-9a6f-ecb23f7628c6</div>"
            .into(),
        base_url: "http://test/".into(),
        width: 400,
        height: 10,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let mut renderer = Renderer::new();
    assert!(!renderer.hyphen_digit_breaks, "off by default");
    let height = |r: &mut Renderer| r.paint(&snap, Area::FullPage, &fetcher).unwrap().height;
    assert_eq!(height(&mut renderer), 300);
    renderer.hyphen_digit_breaks = true;
    assert_eq!(height(&mut renderer), 600);
}

/// A context's `'static` override is part of parley's build cache key: the same text built before
/// and after it changes is broken with the override in force.
#[test]
fn the_override_is_part_of_the_build_cache_key() {
    let mut font_ctx = parley::FontContext::new();
    let mut layout_ctx = parley::LayoutContext::<[u8; 4]>::new();
    let mut lines = |o: Option<&'static parley::LineBreakOverrideFn>| {
        layout_ctx.set_line_break_override(o);
        let text = "a1-23-45";
        let mut layout = layout_ctx.ranged_builder(&mut font_ctx, text, 1.0, true).build(text);
        layout.break_all_lines(Some(0.0));
        layout.len()
    };
    let on = Some(parley::HYPHEN_DIGIT_LINE_BREAK_OVERRIDE);
    assert_eq!((lines(None), lines(on)), (1, 3));
    // Both are cached now, each under its own key.
    assert_eq!((lines(None), lines(on)), (1, 3));
}
