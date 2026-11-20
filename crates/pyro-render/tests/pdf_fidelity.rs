//! PDF output of paper-cut documents: link annotations, tags, source text
//! (ligatures, substituted glyphs), blurred shadows, paper-width layout and
//! page breaks.

use std::collections::HashMap;
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn snap(html: &str) -> Snapshot {
    Snapshot {
        html: html.into(),
        base_url: "http://test/dir/".into(),
        width: 800,
        height: 600,
        device_scale_factor: 1.0,
        ..Default::default()
    }
}

/// Letter, no margins: 816 x 1056 px.
fn print_with(html: &str, f: impl FnOnce(&mut PdfOptions)) -> PdfOutput {
    let mut opts = PdfOptions::new(".pz-page", (816.0, 1056.0));
    f(&mut opts);
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    Renderer::new().print_pdf(&snap(html), &fetcher, &opts).unwrap()
}

fn print(html: &str) -> PdfOutput {
    print_with(html, |_| {})
}

/// The PDF's bytes with every Flate stream inflated in place.
pub(crate) fn inflated(pdf: &[u8]) -> String {
    let mut out = String::from_utf8_lossy(pdf).into_owned();
    let mut i = 0;
    while let Some(at) = find(&pdf[i..], b"stream\n").map(|p| p + i + 7) {
        let Some(end) = find(&pdf[at..], b"endstream").map(|p| p + at) else { break };
        let mut data = &pdf[at..end];
        while let [rest @ .., b'\r' | b'\n'] = data {
            data = rest;
        }
        if let Ok(data) = miniz_oxide::inflate::decompress_to_vec_zlib(data) {
            out.push_str(&String::from_utf8_lossy(&data));
        }
        i = end + b"endstream".len();
    }
    out
}

pub(crate) fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The numbers of each `/Rect[..]` in the PDF.
fn rects(pdf: &str) -> Vec<Vec<f64>> {
    pdf.split("/Rect[")
        .skip(1)
        .map(|r| r.split(']').next().unwrap().split_whitespace().filter_map(|v| v.parse().ok()).collect())
        .collect()
}

#[test]
fn links_become_uri_and_goto_annotations_at_their_boxes() {
    let out = print(
        r##"<!doctype html><body style="margin:0;font:20px/30px sans-serif">
        <div style="height:100px"></div>
        <p style="margin:0"><a href="https://example.com/a?b=1">example</a> and <a href="/rel">rel</a> <a href="#far">far</a></p>
        <a href="https://example.org/block" style="display:block;margin-left:50px;width:200px;height:40px"></a>
        <div style="height:1500px"></div><h2 id="far">Far</h2>"##,
    );
    assert_eq!(out.stats.links, 4, "{:?}", out.stats);
    let pdf = String::from_utf8_lossy(&out.bytes).into_owned();
    assert!(pdf.contains("(https://example.com/a?b=1)"), "absolute URI");
    assert!(pdf.contains("(http://test/rel)"), "resolved against the base URL");
    assert!(pdf.contains("/GoTo") || pdf.contains("/Dest"), "the fragment link is internal");
    let rects = rects(&pdf);
    // The block link: its border box, 50,130 200x40 px; 792 - y in pt.
    let block = [50.0 * 0.75, 792.0 - 170.0 * 0.75, 250.0 * 0.75, 792.0 - 130.0 * 0.75];
    assert!(
        rects.iter().any(|r| r.len() == 4 && r.iter().zip(block).all(|(a, b)| (a - b).abs() < 0.5)),
        "block link rect {block:?} in {rects:?}"
    );
    // The first inline link: on the text's line (y 100..130 px), from x 0.
    assert!(
        rects.iter().any(|r| r.len() == 4
            && r[0].abs() < 0.5
            && r[3] <= 792.0 - 100.0 * 0.75 + 1.0
            && r[1] >= 792.0 - 130.0 * 0.75 - 1.0),
        "inline link rect in {rects:?}"
    );
}

#[test]
fn headings_paragraphs_and_links_are_tagged() {
    let html = r#"<!doctype html><html lang="en"><body style="margin:0;font:16px sans-serif">
        <h1>Title</h1><p>Text with <a href="https://example.com/">a link</a>.</p><h3>Three</h3><div>loose</div>"#;
    let out = print(html);
    let pdf = inflated(&out.bytes);
    assert!(pdf.contains("/StructTreeRoot"), "tagged");
    assert!(pdf.contains("/MarkInfo"), "marked");
    for tag in ["/H1", "/P", "/Link", "/H3", "/NonStruct", "/Document"] {
        assert!(pdf.contains(tag), "{tag} in the structure tree");
    }
    assert!(pdf.contains("/Lang(en)"), "the document's language");
    assert!(pdf.contains("BDC"), "marked content in the page");
    let untagged = print_with(html, |o| o.tagged = false);
    let pdf = inflated(&untagged.bytes);
    for absent in ["/StructTreeRoot", "/MarkInfo", "/StructParents", "BDC"] {
        assert!(!pdf.contains(absent), "generateTaggedPDF: false writes no {absent}");
    }
    assert_eq!(untagged.stats.links, 1, "links without tags");
}

#[test]
fn text_maps_to_its_source_characters() {
    // A face that ligates "fi"/"ffi" maps the ligature glyph to its letters;
    // the cmap reverse lookup alone gave U+FB01 (ﬁ) / U+FB03. Page boxes too.
    for body in ["office fit affluent", r#"<div class="pz-page" style="width:600px;height:200px">office fit affluent"#]
    {
        let out = print(&format!(
            r#"<!doctype html><body style="margin:0;font:28px Helvetica, Arial, sans-serif">{body}</body>"#
        ));
        assert!(out.stats.runs_with_source > 0, "{:?}", out.stats);
        assert_eq!(out.stats.glyphs_without_text, 0, "{:?}", out.stats);
        let pdf = inflated(&out.bytes).to_uppercase();
        assert!(pdf.contains("BEGINBFCHAR"), "the font's ToUnicode map");
        for lig in ["<FB01>", "<FB02>", "<FB03>", "<FB04>"] {
            assert!(!pdf.contains(lig), "a ligature maps to {lig}");
        }
    }
}

/// Inter, cut to the characters these tests show, with its layout features.
const INTER: &[u8] = include_bytes!("fonts/Inter-subset.ttf");

/// Serves [`INTER`] as `inter.ttf`.
struct InterFetch;
impl Fetcher for InterFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(if url.ends_with("/inter.ttf") { Ok(INTER.to_vec()) } else { Err("offline".into()) });
    }
}

/// `body` in 20px Inter (a web font, as ToS's pdfgen loads it), on a
/// `.pz-page` box or cut into Letter pages.
fn print_inter(body: &str, page_box: bool) -> PdfOutput {
    let body = if page_box { format!(r#"<div class="pz-page">{body}</div>"#) } else { body.to_owned() };
    let html = format!(
        r#"<!doctype html><style>@font-face {{ font-family: InterTest; src: url(inter.ttf) }}
        body {{ margin: 0; font: 20px/30px InterTest }} .pz-page {{ width: 600px; height: 400px; padding: 20px }}
        .tnum {{ font-variant-numeric: tabular-nums }}</style>{body}"#
    );
    let fetcher: Arc<dyn Fetcher> = Arc::new(InterFetch);
    let out = Renderer::new().print_pdf(&snap(&html), &fetcher, &PdfOptions::new(".pz-page", (816.0, 1056.0))).unwrap();
    assert_eq!(out.by_selector, page_box);
    out
}

/// The PDF's ToUnicode map: glyph code to text. The PDF has one font.
fn to_unicode(pdf: &[u8]) -> HashMap<String, String> {
    let pdf = inflated(pdf);
    assert_eq!(pdf.matches("/ToUnicode").count(), 1, "one font");
    let mut map = HashMap::new();
    for section in pdf.split("beginbfchar").skip(1) {
        let section = section.split("endbfchar").next().unwrap();
        let hex: Vec<&str> = section.split(['<', '>']).map(str::trim).filter(|s| !s.is_empty()).collect();
        for pair in hex.chunks(2) {
            let units: Vec<u16> = pair[1]
                .as_bytes()
                .chunks(4)
                .map(|u| u16::from_str_radix(std::str::from_utf8(u).unwrap(), 16).unwrap())
                .collect();
            map.insert(pair[0].to_uppercase(), String::from_utf16(&units).unwrap());
        }
    }
    map
}

/// The text of each text object (`BT` .. `ET`) of the pages as pdf.js
/// extracts it: each glyph code read through the ToUnicode map, ActualText
/// ignored. A code the map lacks reads as U+FFFD (pdf.js shows the code as a
/// character).
fn shown_text(pdf: &[u8]) -> Vec<String> {
    let map = to_unicode(pdf);
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = find(&pdf[at..], b"/Contents ").map(|i| i + at + 10) {
        at = i;
        let id: String = pdf[i..].iter().map(|&b| b as char).take_while(char::is_ascii_digit).collect();
        let obj = find(pdf, format!("\n{id} 0 obj").as_bytes()).unwrap();
        let start = find(&pdf[obj..], b"stream\n").unwrap() + obj + 7;
        let end = find(&pdf[start..], b"\nendstream").unwrap() + start;
        let content = miniz_oxide::inflate::decompress_to_vec_zlib(&pdf[start..end]).unwrap();
        for codes in shown_codes(&content) {
            let text = codes.chunks(2).map(|c| {
                let code = format!("{:02X}{:02X}", c[0], c.get(1).copied().unwrap_or(0));
                map.get(&code).map_or("\u{FFFD}", String::as_str)
            });
            out.push(text.collect());
        }
    }
    out
}

/// The bytes of the strings each text object of a content stream shows.
fn shown_codes(s: &[u8]) -> Vec<Vec<u8>> {
    let (mut objects, mut object, mut operands) = (Vec::new(), None::<Vec<u8>>, Vec::new());
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'(' => {
                let mut depth = 0;
                loop {
                    let c = s[i];
                    i += 1;
                    match c {
                        b'(' => {
                            depth += 1;
                            if depth > 1 {
                                operands.push(c);
                            }
                        }
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                            operands.push(c);
                        }
                        b'\\' => {
                            let e = s[i];
                            i += 1;
                            match e {
                                b'n' => operands.push(b'\n'),
                                b'r' => operands.push(b'\r'),
                                b't' => operands.push(b'\t'),
                                b'b' => operands.push(8),
                                b'f' => operands.push(12),
                                b'0'..=b'7' => {
                                    let mut v = u32::from(e - b'0');
                                    for _ in 0..2 {
                                        if !matches!(s[i], b'0'..=b'7') {
                                            break;
                                        }
                                        v = v * 8 + u32::from(s[i] - b'0');
                                        i += 1;
                                    }
                                    operands.push(v as u8);
                                }
                                b'\n' | b'\r' => {}
                                other => operands.push(other),
                            }
                        }
                        c => operands.push(c),
                    }
                }
            }
            b'<' if s.get(i + 1) == Some(&b'<') => {
                // A dictionary (marked-content properties): skipped.
                let mut depth = 0;
                while i < s.len() {
                    match &s[i..] {
                        [b'<', b'<', ..] => {
                            depth += 1;
                            i += 2;
                        }
                        [b'>', b'>', ..] => {
                            depth -= 1;
                            i += 2;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => i += 1,
                    }
                }
            }
            b'<' => {
                let end = i + s[i..].iter().position(|&b| b == b'>').unwrap();
                let hex: Vec<u8> = s[i + 1..end].iter().copied().filter(u8::is_ascii_hexdigit).collect();
                operands
                    .extend(hex.chunks(2).map(|h| u8::from_str_radix(std::str::from_utf8(h).unwrap(), 16).unwrap()));
                i = end + 1;
            }
            b'/' => {
                i += 1;
                while i < s.len() && !b" \t\r\n/[]()<>".contains(&s[i]) {
                    i += 1;
                }
            }
            c if c.is_ascii_alphabetic() || c == b'\'' || c == b'"' => {
                let start = i;
                while i < s.len() && (s[i].is_ascii_alphanumeric() || b"*'\"".contains(&s[i])) {
                    i += 1;
                }
                match &s[start..i] {
                    b"BT" => object = Some(Vec::new()),
                    b"ET" => objects.extend(object.take()),
                    b"TJ" | b"Tj" | b"'" | b"\"" => object.get_or_insert_default().append(&mut operands),
                    _ => {}
                }
                operands.clear();
            }
            _ => i += 1,
        }
    }
    objects
}

/// The PDF's text as `pdftotext` (poppler) extracts it, one entry per non-empty line;
/// `None` without `pdftotext`.
fn pdftotext(pdf: &[u8]) -> Option<Vec<String>> {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("pyro-pdf-text-{}-{n}.pdf", std::process::id()));
    std::fs::write(&path, pdf).unwrap();
    let out = std::process::Command::new("pdftotext").args(["-enc", "UTF-8"]).arg(&path).arg("-").output();
    let _ = std::fs::remove_file(&path);
    let Ok(out) = out.inspect_err(|_| eprintln!("pdftotext not found: its checks are skipped")) else { return None };
    assert!(out.status.success(), "pdftotext: {}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8(out.stdout).unwrap();
    Some(text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect())
}

#[test]
fn glyphs_that_features_substituted_extract_as_their_source_text() {
    // Inter's tabular figures (`tnum`) and its hyphens and parentheses raised between capitals
    // (`calt`/`case`) are glyphs no cmap entry reaches: read back through the cmap, they had no
    // text (ToS account statements lost their amounts and invoice numbers' hyphens).
    let lines = ["E2E-DUN-MONITOR-OPEN 1,234.56", "1,234.56 (EUR) x-y"];
    for page_box in [true, false] {
        let out = print_inter(&format!(r#"<p class="tnum">{}</p><p>{}</p>"#, lines[0], lines[1]), page_box);
        assert_eq!(out.stats.glyphs_without_text, 0, "{:?}", out.stats);
        assert_eq!(shown_text(&out.bytes), lines, "page box: {page_box}");
        // The tabular and the proportional glyphs were both written, each with its text.
        let map = to_unicode(&out.bytes);
        let glyphs_of = |text: &str| map.values().filter(|t| *t == text).count();
        assert_eq!([glyphs_of("1"), glyphs_of("-"), glyphs_of(" ")], [2, 2, 2], "{map:?}");
        if let Some(text) = pdftotext(&out.bytes) {
            assert_eq!(text, lines, "pdftotext, page box: {page_box}");
        }
    }
}

#[test]
fn text_after_zero_size_text_keeps_its_source() {
    // The spaces between inline boxes in a `font-size: 0` box are laid out but not drawn: a run
    // recorded but not drawn stopped every later run from finding its source text.
    for page_box in [true, false] {
        let out = print_inter(
            r#"<div style="font-size:0"> <span style="font-size:20px">A</span> <span style="font-size:20px">B</span> </div>
            <p class="tnum">1,234.56</p>"#,
            page_box,
        );
        assert_eq!(out.stats.glyphs_without_text, 0, "{:?}", out.stats);
        assert_eq!(shown_text(&out.bytes).concat(), "AB1,234.56", "page box: {page_box}");
    }
}

#[test]
fn a_cluster_of_a_base_and_a_mark_keeps_both_characters() {
    // Inter has no precomposed q with acute: q and U+0301 are one cluster of two glyphs. Each
    // glyph maps to its own character, so readers without ActualText (pdf.js) get both.
    for page_box in [true, false] {
        let out = print_inter("<p>xq\u{301}x</p>", page_box);
        assert_eq!(out.stats.glyphs_without_text, 0, "{:?}", out.stats);
        assert_eq!(shown_text(&out.bytes), ["xq\u{301}x"], "page box: {page_box}");
        assert!(!inflated(&out.bytes).contains("ActualText"));
    }
}

#[test]
fn text_of_an_inline_root_with_faded_inline_elements_is_written_once_in_order() {
    // A screenshot paints an inline element's opacity or backdrop filter in passes over its
    // inline root: outside the elements' areas, then in each element's areas. In a PDF each pass
    // drew (and recorded) the root's glyph runs again, and the writer writes every glyph run
    // drawn: the paragraph's text was written once per pass, the elements' text after the rest.
    for effect in ["opacity:.6", "backdrop-filter:invert(1)"] {
        for page_box in [true, false] {
            let out = print_inter(
                &format!(
                    r#"<p>AB <span style="{effect}">(EUR)</span> x-y <span style="{effect}">MONITOR<br>OPEN</span> DUN</p>
                    <p class="tnum">1,234.56</p>"#
                ),
                page_box,
            );
            let what = format!("{effect}, page box: {page_box}");
            assert_eq!(out.stats.glyphs_without_text, 0, "{what}: {:?}", out.stats);
            assert_eq!(shown_text(&out.bytes).concat(), "AB (EUR) x-y MONITOROPEN DUN1,234.56", "{what}");
        }
    }
}

#[test]
fn text_after_an_inline_root_with_effects_out_of_view_keeps_its_source() {
    // An inline element's clip path paints its inline root in passes. The pass outside the
    // elements' areas is left out when what it paints is out of view: the runs it recorded were
    // not drawn, which stopped every later run from finding its source text (the tabular figures
    // below then had none).
    for page_box in [true, false] {
        let out = print_inter(
            r#"<p style="text-indent:-3000px">AB <span style="clip-path:inset(0)">(EUR)</span></p><p class="tnum">1,234.56</p>"#,
            page_box,
        );
        assert_eq!(out.stats.glyphs_without_text, 0, "page box: {page_box}: {:?}", out.stats);
        assert!(shown_text(&out.bytes).concat().ends_with("1,234.56"), "page box: {page_box}");
    }
}

#[test]
fn blurred_outset_shadows_are_drawn() {
    let html = r#"<!doctype html><body style="margin:40px">
        <div style="width:200px;height:100px;background:#fff;border-radius:12px;box-shadow:0 8px 24px rgba(0,0,0,.5)"></div>"#;
    let out = print(html);
    assert_eq!((out.stats.box_shadows, out.stats.box_shadows_skipped), (1, 0), "{:?}", out.stats);
    assert_eq!(out.stats.images, 0, "the shadow is not counted as a page image");
    assert!(inflated(&out.bytes).contains("/Interpolate true"), "the blurred mask is drawn smoothly");
}

#[test]
fn a_mask_layer_is_not_painted_over_what_it_masks() {
    // PDF has no destination-in: a mask's `DestIn` layer (here an SVG `<mask>`,
    // whose coverage is drawn as an image) is left out, and the content stays
    // unmasked instead of covered by the mask's image.
    let out = print(
        r##"<!doctype html><body style="margin:0"><svg width="100" height="100">
        <mask id="m"><rect width="50" height="100" fill="white"/></mask>
        <rect width="100" height="100" fill="#0000ff" mask="url(#m)"/></svg>"##,
    );
    assert_eq!((out.stats.compose_skipped, out.stats.images), (1, 0), "{:?}", out.stats);
    assert!(out.stats.fills >= 1, "the masked rect is drawn: {:?}", out.stats);
}

#[test]
fn page_boxes_keep_their_output() {
    // `.pz-page` documents: no tags, no links; text is written with its
    // source; blurred shadows are drawn (PYRO_PDF_SHADOWS=0 skips
    // them).
    let out = print(
        r#"<!doctype html><style>body{margin:0}.pz-page{width:400px;height:500px;box-shadow:0 0 20px #000}</style>
        <div class="pz-page"><h1>One</h1><a href="https://example.com/">x</a></div>"#,
    );
    assert!(out.by_selector);
    assert_eq!((out.stats.links, out.stats.runs_with_source), (0, 2));
    assert_eq!((out.stats.box_shadows, out.stats.box_shadows_skipped), (1, 0));
    let pdf = inflated(&out.bytes);
    for absent in ["/StructTreeRoot", "/MarkInfo", "/StructParents", "BDC"] {
        assert!(!pdf.contains(absent), "untagged: no {absent}");
    }
}

#[test]
fn a_page_box_shadow_under_its_own_background_is_not_drawn() {
    // Paprize's page shadow falls outside the printed page box, and the
    // box's white background covers the rest: nothing of it shows.
    let out = print(
        r#"<!doctype html><style>body{margin:0}.pz-page{width:400px;height:500px;margin-bottom:100px;background:#fff;box-shadow:0 4px 16px rgba(0,0,0,.3)}</style>
        <div class="pz-page">one</div><div class="pz-page">two</div>"#,
    );
    assert_eq!((out.stats.box_shadows, out.stats.box_shadows_covered), (0, 2), "{:?}", out.stats);
}

#[test]
fn the_document_is_laid_out_at_the_paper_width() {
    // 800 px viewport, 816 px paper with 0.5in margins: 720 px of content.
    let out = print_with(
        r#"<!doctype html><body style="margin:0"><div id="w" style="height:50px;background:#f00"></div>"#,
        |o| o.margins = [48.0; 4],
    );
    assert_eq!(out.pages.len(), 1);
    assert_eq!(out.pages[0].rect.width(), 720.0);
    // The full-width block is 720 px wide (540 pt), not cut from 800.
    let pdf = inflated(&out.bytes);
    assert!(pdf.contains("0 0 m 720 0 l 720 50 l"), "a 720x50 fill");
}

#[test]
fn page_cuts_do_not_split_a_line() {
    // 20 px lines starting at 1000 px: the paper's 1056 px cut falls inside
    // the line from 1040 to 1060, which moves to page 2.
    let words = "lorem ipsum dolor sit amet ".repeat(200);
    let html = format!(
        r#"<!doctype html><body style="margin:0;font:16px/20px sans-serif"><div style="height:1000px"></div><p style="margin:0">{words}</p>"#
    );
    let out = print(&html);
    let first = out.pages[0].rect;
    assert_eq!(first.y1, 1040.0, "cut at the top of the line crossing 1056");
    let lines_start_on_page_boundaries = out.pages.windows(2).all(|w| (w[1].rect.y0 - 1000.0) % 20.0 == 0.0);
    assert!(lines_start_on_page_boundaries, "{:?}", out.pages.iter().map(|p| p.rect).collect::<Vec<_>>());
    // PDF pages stay the paper's size.
    assert!(String::from_utf8_lossy(&out.bytes).contains("/MediaBox[0 0 612 792]"));
}

#[test]
fn forced_breaks_and_break_inside_avoid() {
    let out = print(
        r#"<!doctype html><body style="margin:0">
        <div style="height:200px">A</div>
        <div style="break-before:page;height:100px">B</div>
        <div style="height:100px;page-break-after:always">C</div>
        <div style="height:900px">D</div>
        <div style="break-inside:avoid;height:300px">E</div>"#,
    );
    let ys: Vec<f64> = out.pages.iter().map(|p| p.rect.y0).collect();
    // B starts page 2 (at 200), D page 3 (at 400); E (1300..1600) would
    // cross 400 + 1056 and moves to page 4.
    assert_eq!(ys, vec![0.0, 200.0, 400.0, 1300.0]);
}

fn close(got: &[f64], want: &[f64]) -> bool {
    got.len() == want.len() && got.iter().zip(want).all(|(a, b)| (a - b).abs() < 0.01)
}

/// Rects measured on Chrome 156 (Letter, 0.4 in margins), through the geometry `Page.printToPDF` uses.
#[test]
fn link_rects_land_where_chrome_puts_them() {
    let html = r#"<!doctype html><body style="margin:0"><div style="height:70px"></div>
        <a href="https://example.com/b" style="display:block;margin-left:40px;width:200px;height:50px"></a>"#;
    let (paper, margins) = pyro_render::chrome_page_geometry((8.5, 11.0), [0.4; 4]).unwrap();
    for (scale, want) in [
        (1.0, [57.75, 674.25, 207.75, 711.75]),
        (0.8, [51.75, 692.25, 171.75, 722.25]),
        (1.5, [72.75, 629.25, 297.75, 685.5]),
    ] {
        let out = print_with(html, |o| {
            o.paper = paper;
            o.margins = margins;
            o.scale = scale;
        });
        let found = rects(&inflated(&out.bytes));
        assert!(found.len() == 1 && close(&found[0], &want), "scale {scale}: {found:?}");
    }
}

/// The matrices glyphs are drawn through, in content order.
fn text_matrices(pdf: &str) -> Vec<[f64; 6]> {
    let mut out = Vec::new();
    for (i, _) in pdf.match_indices(" cm") {
        let head = &pdf[pdf[..i].rfind(['q', '\n']).unwrap_or(0)..i];
        let n: Vec<f64> = head.split_whitespace().filter_map(|v| v.parse().ok()).collect();
        if n.len() == 6 && pdf[i..].trim_start_matches(" cm").trim_start().contains("BT") {
            out.push([n[0], n[1], n[2], n[3], n[4], n[5]]);
        }
    }
    out
}

/// Rects measured on Chrome 156 (600 x 375.12 pt).
#[test]
fn vertical_text_is_rotated_and_its_link_areas_land_where_chrome_puts_them() {
    let html = r#"<!doctype html><body style="margin:0;font:20px/30px Helvetica">
        <div style="position:absolute;left:100px;top:50px;writing-mode:vertical-rl;height:300px;width:100px"><a href="https://example.com/a">example text</a></div>
        <div style="position:absolute;left:300px;top:50px;writing-mode:vertical-lr;height:300px;width:100px">x x <a href="https://example.com/b">lr link</a></div>"#;
    let out = print_with(html, |o| o.paper = (800.0, 500.16));
    let content = inflated(&out.bytes);
    let found = rects(&content);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(close(&found[0], &[130.5, 252.87, 147.75, 337.62]) || cfg!(not(target_os = "macos")), "{found:?}");
    assert!(close(&found[1], &[228.0, 279.12, 245.25, 314.37]) || cfg!(not(target_os = "macos")), "{found:?}");
    let matrices = text_matrices(&content);
    assert!(!matrices.is_empty());
    // A quarter turn clockwise: in a PDF's y-up page, (0 -1 -1 0) once its own y flip is applied.
    assert!(matrices.iter().all(|m| m[0].abs() < 1e-6 && m[3].abs() < 1e-6), "{matrices:?}");
}

/// Measured on Chrome 156 with Playwright's A4 (8.27 x 11.7 in, a 595.92 x 842.88 pt page): a page of a
/// fixed `@page` size is centered on the paper in whole CSS px, here 1 px down; without one the
/// page stays in the paper's corner.
#[test]
fn page_boxes_go_on_the_paper_as_in_chrome() {
    let doc = |rule: &str, style: &str| {
        format!(
            r#"<!doctype html><style>html,body{{margin:0}}{rule}</style><section style="{style}"><div class=pz-page style="width:210mm;height:297mm;background:#f00"></div></section>"#
        )
    };
    let (paper, margins) = pyro_render::chrome_page_geometry((8.27, 11.7), [0.0; 4]).unwrap();
    let print = |html: String| {
        let out = print_with(&html, |o| {
            o.paper = paper;
            o.margins = margins;
            o.place_on_paper = true;
        });
        inflated(&out.bytes)
    };
    for (html, top) in [
        (doc("@page{margin:0;size:210mm 297mm}", ""), "842.13"),
        (doc("@media screen{@page{margin:0;size:auto!important}}@page s{margin:0;size:210mm 297mm}", "page: s"), "842.13"),
        (doc("@page{margin:0;size:auto!important}@page s{margin:0;size:210mm 297mm}", "page: s"), "842.88"),
        (doc("@page s{margin:0;size:210mm 297mm}", ""), "842.88"),
        (doc("@page{margin:0}", ""), "842.88"),
    ] {
        let pdf = print(html.clone());
        let pdf = pdf.as_str();
        assert!(pdf.contains("/MediaBox[0 0 595.92 842.88]"), "{html}");
        let placed = format!("0.75 0 0 -0.75 0 {top} cm");
        assert!(pdf.contains(&placed), "{html}: {:?}", pdf.match_indices(" cm").next().and_then(|(i, _)| pdf.get(i.saturating_sub(40)..i + 3)));
    }
}

/// Measured on Chrome 156 under an emulated screen, A4: a `<link>` sheet's `@page` size places the page as
/// a `<style>` does, and `media` on the link decides whether it applies.
#[test]
fn page_boxes_read_linked_sheets() {
    let print = |link: &str, sheet: &str, box_css: &str, attrs: &str| {
        let mut s = snap(&format!(
            r#"<!doctype html><link rel=stylesheet href=p.css data-pyro-sheet=p {link}><style>html,body{{margin:0}}</style><section {attrs}><div class=pz-page style="{box_css};background:#f00"></div></section>"#
        ));
        s.sheets.insert("p".to_owned(), sheet.to_owned());
        let (paper, margins) = pyro_render::chrome_page_geometry((8.27, 11.7), [0.0; 4]).unwrap();
        let mut opts = PdfOptions::new(".pz-page", paper);
        opts.margins = margins;
        opts.place_on_paper = true;
        opts.print_media = false;
        let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
        inflated(&Renderer::new().print_pdf(&s, &fetcher, &opts).unwrap().bytes)
    };
    let page_500 = "width:500px;height:700px";
    let size_500 = "@page{margin:0;size:500px 700px}";
    // 147 and 212 CSS px.
    assert!(print("", size_500, page_500, "").contains("0.75 0 0 -0.75 110.25 683.88 cm"));
    assert!(print("media=screen", size_500, page_500, "").contains("0.75 0 0 -0.75 110.25 683.88 cm"));
    assert!(print("media=print", size_500, page_500, "").contains("0.75 0 0 -0.75 0 842.88 cm"));
    let named = "@media print{@page{size:auto!important}}@page s{margin:0;size:210mm 297mm}";
    assert!(print("", named, "width:210mm;height:297mm", r#"style="page: s""#).contains("0.75 0 0 -0.75 0 842.13 cm"));
}

/// Chrome 156 writes this text at 37.78125 CSS px (37.7953 px in layout units); the background of its box stays
/// on whole pixels, as does its link's area.
#[test]
fn text_sits_at_its_layout_unit_x() {
    let html = |selector: &str| {
        format!(
            r#"<!doctype html><style>html,body{{margin:0;font:16px/20px Arial}}</style><div class="{selector}" style="width:600px;height:700px;box-sizing:border-box;padding-left:37.7953px;background:#eee">Text</div>"#
        )
    };
    for class in ["pz-page", "plain"] {
        let pdf = inflated(&print(&html(class)).bytes);
        // 37.78125 px = 28.3359 pt; the page's other `cm`s are whole pixels.
        let cms: Vec<&str> = pdf.split(" cm").map(|s| s.rsplit('q').next().unwrap_or("")).collect();
        assert!(cms.iter().any(|c| c.contains(" 28.3359")), "{class}: {cms:?}");
    }
}

#[test]
fn inline_link_areas_are_whole_css_pixels() {
    let html = r#"<!doctype html><body style="margin:0;font:20px/30px sans-serif"><a href="https://example.com/a">example text</a>"#;
    let out = print(html);
    let found = rects(&inflated(&out.bytes));
    assert_eq!(found.len(), 1, "{found:?}");
    for v in &found[0] {
        let px = v / 0.75;
        assert!((px - px.round()).abs() < 0.01, "{found:?}");
    }
}

#[test]
fn a_short_document_on_a_page_smaller_than_the_viewport_is_one_page() {
    // The 600 px viewport is taller than a 300 px page; the content is 20 px.
    let out = print_with(r#"<!doctype html><body style="margin:0"><div style="height:20px">x</div>"#, |o| {
        o.paper = (816.0, 300.0)
    });
    assert_eq!(out.pages.len(), 1);
}

#[test]
fn a_heading_after_a_forced_break_is_written_on_its_page_only() {
    let out = print(
        r#"<!doctype html><body style="margin:8px;font:16px/20px sans-serif"><p>before</p><h2 style="break-before:page">UniqueHeading</h2>"#,
    );
    assert_eq!(out.pages.len(), 2);
    let text = inflated(&out.bytes);
    assert_eq!(out.stats.links, 0);
    // The heading's single tagged run is written once: one marked-content sequence per page's text.
    assert_eq!(text.matches("BDC").count(), 2, "one text run on each page");
}

/// A full-width box at Chrome's fractional content width (Letter, 0.4 in: 741.33 px), measured on Chrome 156
/// within LayoutUnit precision; the screen layout's whole pixels would end 0.25 pt short.
#[test]
fn a_full_width_link_ends_at_the_fractional_content_width() {
    let html = r#"<!doctype html><body style="margin:0"><a href="https://example.com/w" style="display:block;height:10px"></a>"#;
    let (paper, margins) = pyro_render::chrome_page_geometry((8.5, 11.0), [0.4; 4]).unwrap();
    for (scale, chrome_x1) in [(1.0, 583.75781), (0.8, 583.75781), (1.5, 583.78125)] {
        let out = print_with(html, |o| {
            o.paper = paper;
            o.margins = margins;
            o.scale = scale;
        });
        let found = rects(&inflated(&out.bytes));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!((found[0][0] - 27.75).abs() < 0.01, "scale {scale}: {found:?}");
        assert!((found[0][2] - chrome_x1).abs() < 0.1, "scale {scale}: {found:?}");
    }
}

/// Where each page starts on a 576 x 384 px paper of 19 lines of 20 px, 4 px to spare.
fn page_tops(body: &str) -> Vec<f64> {
    let html = format!(r#"<!doctype html><body style="margin:0;font:16px/20px sans-serif">{body}"#);
    print_with(&html, |o| o.paper = (576.0, 384.0)).pages.iter().map(|p| p.rect.y0).collect()
}

/// `n` lines of 20 px with these declarations.
fn lines(n: usize, style: &str) -> String {
    format!(r#"<div style="{style}">{}</div>"#, vec!["x"; n].join("<br>"))
}

fn spacer(height: u32) -> String {
    format!(r#"<div style="height:{height}px"></div>"#)
}

/// The expectations in the tests below are Chrome 156's, read off the lines of each page of the same fixtures.
#[test]
fn a_block_keeps_two_lines_on_each_side_of_a_cut() {
    // 20 lines: 18 + 2. 21: 19 + 2. 39: 19 + 18 + 2.
    assert_eq!(page_tops(&lines(20, "")), [0.0, 360.0]);
    assert_eq!(page_tops(&lines(21, "")), [0.0, 380.0]);
    assert_eq!(page_tops(&lines(39, "")), [0.0, 380.0, 740.0]);
    // One line fits: the block moves. Two fit, three lines long: orphans win, 2 + 1.
    assert_eq!(page_tops(&(spacer(360) + &lines(5, ""))), [0.0, 360.0]);
    assert_eq!(page_tops(&(spacer(340) + &lines(3, ""))), [0.0, 380.0]);
    // Each block container has its own lines: 10 + a block + 9 lines, 8 of them fit, 7 + 2.
    let anonymous = format!("<div>{0}<div>x</div>{1}</div>", vec!["x"; 10].join("<br>"), vec!["x"; 9].join("<br>"));
    assert_eq!(page_tops(&anonymous), [0.0, 360.0]);
}

#[test]
fn orphans_and_widows_are_honoured_and_orphans_win() {
    assert_eq!(page_tops(&lines(20, "widows:1")), [0.0, 380.0]);
    assert_eq!(page_tops(&(spacer(360) + &lines(5, "orphans:1"))), [0.0, 380.0]);
    // Both inherit; 0 is invalid and leaves the default.
    let html =
        r#"<!doctype html><body style="margin:0;font:16px/20px sans-serif;widows:1">"#.to_owned() + &lines(20, "");
    assert_eq!(print_with(&html, |o| o.paper = (576.0, 384.0)).pages[1].rect.y0, 380.0);
    assert_eq!(page_tops(&lines(20, "widows:0")), [0.0, 360.0]);
    // 5 lines, 4 fit, 3 + 3 wanted: 3 + 2.
    assert_eq!(page_tops(&(spacer(300) + &lines(5, "orphans:3;widows:3"))), [0.0, 360.0]);
    // 3 fit, 4 wanted: the block moves, although widows would be fine.
    assert_eq!(page_tops(&(spacer(320) + &lines(8, "orphans:4;widows:1"))), [0.0, 320.0]);
    // 23 lines, 12 + 12 wanted: 12 + 11, at the top of a page and below one.
    assert_eq!(page_tops(&lines(23, "orphans:12;widows:12")), [0.0, 240.0]);
    assert_eq!(page_tops(&(spacer(100) + &lines(23, "orphans:12;widows:12"))), [0.0, 340.0]);
    // Past the first part orphans still count: 40 lines, 12 + 12: 19 + 12 + 9.
    assert_eq!(page_tops(&lines(40, "orphans:12;widows:12")), [0.0, 380.0, 620.0]);
    // Orphans over a page: as much as fits, moved first when not at the top.
    assert_eq!(page_tops(&lines(30, "orphans:25")), [0.0, 380.0]);
    assert_eq!(page_tops(&(spacer(100) + &lines(30, "orphans:25"))), [0.0, 100.0, 480.0]);
    // Widows over a page: honoured for the first part only. 30 lines, 25: 5 + 19 + 6.
    assert_eq!(page_tops(&lines(30, "widows:25")), [0.0, 100.0, 480.0]);
    // ... where 19 still fit on a page, later parts honour them too: 60 lines: 19 + 19 + 3 + 19.
    assert_eq!(page_tops(&lines(60, "widows:19")), [0.0, 380.0, 760.0, 820.0]);
    // 20 don't: 60 lines: 19 + 19 + 19 + 3.
    assert_eq!(page_tops(&lines(60, "widows:20")), [0.0, 380.0, 760.0, 1140.0]);
}

#[test]
fn avoided_boxes_move_whole_unless_at_the_top_of_a_page() {
    let avoid = "break-inside:avoid";
    // Fits a page, not what is left.
    assert_eq!(page_tops(&(spacer(300) + &lines(10, avoid))), [0.0, 300.0]);
    // Taller than a page, not at the top: moved, then cut, also as avoid-page.
    assert_eq!(page_tops(&(spacer(300) + &lines(25, avoid))), [0.0, 300.0, 680.0]);
    assert_eq!(page_tops(&(spacer(300) + &lines(25, "break-inside:avoid-page"))), [0.0, 300.0, 680.0]);
    // At the top: cut; its lines keep their widows.
    assert_eq!(page_tops(&lines(25, avoid)), [0.0, 380.0]);
    assert_eq!(page_tops(&lines(20, avoid)), [0.0, 360.0]);
    // Margins, borders and padding are not content; a box of any height is.
    let at_top = |style: &str, tops: &[f64]| {
        let html = format!(r#"<div style="{style}">{}</div>"#, lines(25, avoid));
        assert_eq!(page_tops(&html), tops, "{style}");
    };
    at_top("margin-top:30px", &[0.0, 370.0]);
    at_top("padding-top:10px", &[0.0, 370.0]);
    at_top("border-top:10px solid", &[0.0, 370.0]);
    assert_eq!(page_tops(&(spacer(1) + &lines(25, avoid))), [0.0, 1.0, 381.0]);
    // Nested: when the outer one is cut, the inner ones still hold.
    let three = lines(10, avoid).repeat(3);
    assert_eq!(page_tops(&format!(r#"<div style="{avoid}">{three}</div>"#)), [0.0, 200.0, 400.0]);
    let five = lines(5, avoid).repeat(5);
    assert_eq!(page_tops(&(spacer(300) + &format!(r#"<div style="{avoid}">{five}</div>"#))), [0.0, 300.0, 600.0]);
    // An inner one taller than a page moves from below other content of the outer one.
    let outer = format!(r#"<div style="{avoid}">x{}</div>"#, lines(25, avoid));
    assert_eq!(page_tops(&(spacer(300) + &outer)), [0.0, 300.0, 320.0, 700.0]);
}

#[test]
fn replaced_elements_taller_than_a_page_move_and_run_over() {
    let img = |h: u32| format!(r#"<img style="display:block;width:100px;height:{h}px">"#);
    // Moved; what follows comes after it, 116 px down page 3.
    assert_eq!(page_tops(&(spacer(300) + &img(500) + "x")), [0.0, 300.0, 684.0]);
    assert_eq!(page_tops(&(img(1000) + "x")), [0.0, 384.0, 768.0]);
    assert_eq!(page_tops(&(spacer(300) + &img(385) + "x")), [0.0, 300.0, 684.0]);
}

#[test]
fn a_cut_drops_margins_and_a_forced_break_keeps_them() {
    // The cut falls in the margin: the next page starts at the content.
    assert_eq!(page_tops(&(spacer(380) + &lines(2, "margin-top:30px"))), [0.0, 410.0]);
    assert_eq!(page_tops(&(lines(19, "") + &lines(3, "margin-top:30px"))), [0.0, 410.0]);
    // Padding stays with what it is above.
    assert_eq!(page_tops(&(spacer(380) + &lines(2, "padding-top:30px"))), [0.0, 380.0]);
    // A forced break keeps the margin below it (the line starts 30 px down page 2).
    assert_eq!(page_tops(&(lines(1, "") + &lines(1, "break-before:page;margin-top:30px"))), [0.0, 20.0]);
    // Above the first content it does nothing.
    assert_eq!(page_tops(&lines(1, "break-before:page;margin-top:30px")), [0.0]);
    let html = r#"<!doctype html><body style="margin:8px"><div style="break-before:page">x</div>"#;
    assert_eq!(print_with(html, |o| o.paper = (576.0, 384.0)).pages.len(), 1);
}

/// A box this tall (fractional px).
fn fractional_box(height: f64, style: &str) -> String {
    format!(r#"<div style="height:{height}px;{style}"></div>"#)
}

#[test]
fn a_fraction_of_a_px_decides_whether_a_line_fits() {
    // 384 px pages, the line 20 px tall: its bottom is past the end at any fraction.
    assert_eq!(page_tops(&(fractional_box(364.0 + 1.0 / 64.0, "") + &lines(1, ""))), [0.0, 364.0]);
    assert_eq!(page_tops(&(fractional_box(364.25, "") + &lines(1, ""))), [0.0, 364.0]);
    // The cut is painted where the line is, 365 from 364.5 up.
    assert_eq!(page_tops(&(fractional_box(364.75, "") + &lines(1, ""))), [0.0, 365.0]);
    assert_eq!(page_tops(&(fractional_box(363.75, "") + &lines(1, ""))).len(), 1);
    // An avoided box likewise.
    assert_eq!(page_tops(&(fractional_box(100.0, "") + &fractional_box(284.25, "break-inside:avoid"))), [0.0, 100.0]);
    assert_eq!(page_tops(&(fractional_box(100.0, "") + &fractional_box(284.0, "break-inside:avoid"))).len(), 1);
}

#[test]
fn fractional_line_heights_break_as_in_chrome() {
    let tops = |line_height: f64, n: usize, style: &str| {
        page_tops(&format!(r#"<div style="line-height:{line_height}px;{style}">{}</div>"#, vec!["x"; n].join("<br>")))
    };
    // Chrome's lines per page: 18 + 18 + 13; 17 + 17 + 12; 12 + 12 + 9.
    assert_eq!(tops(20.5, 49, ""), [0.0, 369.0, 738.0]);
    assert_eq!(tops(21.92, 46, ""), [0.0, 373.0, 745.0]);
    assert_eq!(tops(30.7, 33, ""), [0.0, 368.0, 737.0]);
    // Each page holds a whole page's worth from its fractional start: 19 + 19 + 13.
    assert_eq!(page_tops(&(0..51).map(|_| lines(1, "height:19.7px")).collect::<String>()), [0.0, 374.0, 749.0]);
}

#[test]
fn a_fraction_past_the_page_end_makes_a_page() {
    let pages = |height: f64| page_tops(&fractional_box(height, "")).len();
    assert_eq!(pages(384.0), 1);
    assert_eq!(pages(384.0 - 1.0 / 64.0), 1);
    assert_eq!(pages(384.0 + 1.0 / 64.0), 2);
    assert_eq!(pages(384.25), 2);
    assert_eq!(pages(768.25), 3);
}

#[test]
fn a_forced_break_at_a_fraction_is_below_the_first_content() {
    // A box a quarter px tall is content: page 1 holds it alone.
    assert_eq!(page_tops(&(fractional_box(0.25, "") + &lines(1, "break-before:page"))).len(), 2);
}

/// `unrounded_layout` is in the placer's writing mode, so a vertical document stays on its snapped positions. These
/// are the pages it had before cuts read fractional positions, not Chrome's: vertical documents do not page as Chrome's.
#[test]
fn a_vertical_document_pages_on_snapped_positions() {
    let html = r#"<!doctype html><html style="writing-mode:vertical-lr"><body style="margin:0;width:400.5px;height:1000px"><div style="margin-inline-start:350px;margin-block-start:200.5px;inline-size:100px;block-size:50.25px;break-inside:avoid"></div>"#;
    let tops: Vec<f64> = print_with(html, |o| o.paper = (576.0, 384.0)).pages.iter().map(|p| p.rect.y0).collect();
    assert_eq!(tops, [0.0, 401.0, 802.0]);
}

#[test]
fn a_tall_monolithic_box_below_the_top_of_a_page_moves() {
    let monolithic = "contain:size;height:500px;width:100px";
    let body = |style: &str| format!(r#"<div style="{style}">k</div><div>z</div>"#);
    // Chrome's words per page: 1 + 1 at the top; below it, by any margin, 0 + 1 + 1.
    assert_eq!(page_tops(&body(monolithic)).len(), 2);
    for margin in [0.1, 0.3, 0.5, 0.8, 1.0, 5.0, 20.0] {
        let tops = page_tops(&body(&format!("{monolithic};margin-top:{margin}px")));
        assert_eq!(tops.len(), 3, "margin {margin}");
    }
    assert_eq!(page_tops(&format!(r#"<div style="padding-top:5px">{}</div>"#, body(monolithic))).len(), 3);
    // Its own border and padding are part of it.
    assert_eq!(page_tops(&body(&format!("{monolithic};border-top:5px solid"))).len(), 2);
    assert_eq!(page_tops(&body(&format!("{monolithic};padding-top:5px"))).len(), 2);
    // A vertical box is monolithic in a horizontal document.
    let vertical = "writing-mode:vertical-rl;height:500px;width:100px";
    assert_eq!(page_tops(&body(vertical)).len(), 2);
    assert_eq!(page_tops(&body(&format!("{vertical};margin-top:0.3px"))).len(), 3);
    // A `break-inside: avoid` box can be split, and stays.
    assert_eq!(page_tops(&body("height:500px;break-inside:avoid;margin-top:20px")).len(), 2);
}

/// What a page holds, whole CSS px, and overflowing documents, measured on Chrome 156 by 1 px rows: the rows of
/// page 1 are its height, and their written height the scale.
#[test]
fn pages_hold_whole_css_pixels() {
    for (paper, margin, scale, chrome_rows) in [
        ((8.5, 11.0), 0.0, 1.0, 1056.0),
        ((8.5, 11.0), 0.4, 1.0, 982.0),
        ((8.5, 11.0), 0.4, 1.5, 655.0),
        ((8.27, 11.69), 0.0, 1.0, 1123.0),
        ((8.27, 11.69), 0.4, 1.0, 1049.0),
        ((8.27, 11.69), 0.1, 1.0, 1105.0),
        ((8.27, 11.69), 0.5, 1.0, 1027.0),
        ((8.27, 11.69), 0.0, 1.5, 749.0),
    ] {
        let (paper_px, margins) = chrome_page_geometry(paper, [margin; 4]).unwrap();
        let out = print_with(r#"<!doctype html><body style="margin:0"><div style="height:5000px"></div>"#, |o| {
            o.paper = paper_px;
            o.margins = margins;
            o.page_area = Some(chrome_page_area(paper, [margin; 4]));
            o.scale = scale;
        });
        assert_eq!(out.pages[0].rect.height(), chrome_rows, "{paper:?} margin {margin} scale {scale}");
    }
}

#[test]
fn a_line_ending_at_the_rounded_up_page_end_fits() {
    // A4 without margins: 1122.67 px rounds up to 1123.
    let (paper, margins) = chrome_page_geometry((8.27, 11.69), [0.0; 4]).unwrap();
    let print = |top: u32| {
        let html = format!(
            r#"<!doctype html><body style="margin:0;font:16px/20px sans-serif">{}<div>x</div><div>y</div>"#,
            spacer(top)
        );
        print_with(&html, |o| {
            o.paper = paper;
            o.margins = margins;
            o.page_area = Some(chrome_page_area((8.27, 11.69), [0.0; 4]));
        })
    };
    assert_eq!(print(1103).pages[0].rect.y1, 1123.0);
    assert_eq!(print(1104).pages[0].rect.y1, 1104.0);
}

#[test]
fn overflowing_documents_shrink_to_fit_up_to_a_limit() {
    let html = |width: f64| {
        format!(
            r#"<!doctype html><body style="margin:0"><div style="width:{width}px;height:1px;margin-bottom:-1px"></div><a href="https://example.com/r" style="display:block;height:1px"></a><div style="height:5000px"></div>"#
        )
    };
    let print = |paper: (f64, f64), margin: f64, scale: f64, width: f64| {
        let (paper_px, margins) = chrome_page_geometry(paper, [margin; 4]).unwrap();
        print_with(&html(width), |o| {
            o.paper = paper_px;
            o.margins = margins;
            o.page_area = Some(chrome_page_area(paper, [margin; 4]));
            o.scale = scale;
        })
    };
    let (letter, a4) = ((8.5, 11.0), (8.27, 11.69));
    // Rows of page 1, written height of a row (pt).
    for (paper, margin, scale, width, rows, row_pt) in [
        // 494.22 px of content rounds up to 495: nothing happens to 495 px.
        (letter, 0.4, 1.5, 495.0, 655.0, 1.125),
        (letter, 0.4, 1.5, 495.25, 655.0, 1.1245),
        (letter, 0.4, 1.5, 496.0, 656.0, 1.1227),
        (letter, 0.4, 1.5, 500.0, 661.0, 1.1138),
        (letter, 0.4, 1.5, 700.0, 926.0, 0.7955),
        // At most by 1.5.
        (letter, 0.4, 1.5, 742.5, 982.0, 0.75),
        (letter, 0.4, 1.5, 2000.0, 982.0, 0.75),
        (letter, 0.4, 1.0, 743.0, 983.0, 0.749),
        (letter, 0.4, 1.0, 1200.0, 1472.0, 0.5),
        (letter, 0.0, 1.0, 817.0, 1058.0, 0.7491),
        // A4's 720 px with 0.4 in margins counts as 721.
        (a4, 0.4, 1.0, 721.0, 1049.0, 0.75),
        (a4, 0.4, 1.0, 722.0, 1050.0, 0.749),
    ] {
        let out = print(paper, margin, scale, width);
        let at = format!("{paper:?} margin {margin} scale {scale} width {width}");
        assert_eq!(out.pages[0].rect.height(), rows, "{at}");
        let found = rects(&inflated(&out.bytes));
        let height = found[0][3] - found[0][1];
        assert!((height - row_pt).abs() < 0.002, "{at}: {found:?}");
    }
}

#[test]
fn what_does_not_overflow_or_is_clipped_does_not_shrink() {
    let at_scale = |body: &str| {
        let html = format!(
            r#"<!doctype html><body style="margin:0"><a href="https://example.com/r" style="display:block;height:10px"></a>{body}"#
        );
        let (paper, margins) = chrome_page_geometry((8.5, 11.0), [0.4; 4]).unwrap();
        let out = print_with(&html, |o| {
            o.paper = paper;
            o.margins = margins;
            o.page_area = Some(chrome_page_area((8.5, 11.0), [0.4; 4]));
            o.scale = 1.5;
        });
        let found = rects(&inflated(&out.bytes));
        (found[0][3] - found[0][1]) / 11.25
    };
    let clipped = r#"<div style="overflow:hidden;width:400px"><div style="width:1000px;height:10px"></div></div>"#;
    assert!((at_scale(clipped) - 1.0).abs() < 0.001);
    assert!((at_scale(r#"<div style="width:400px;margin-left:-200px;height:1px"></div>"#) - 1.0).abs() < 0.001);
    assert!((at_scale(r#"<div style="width:450px;margin-right:100px;height:1px"></div>"#) - 1.0).abs() < 0.001);
    // These do: 550, 700, 700, 600 px on a 495 px limit.
    assert!((at_scale(r#"<div style="width:450px;margin-left:100px;height:1px"></div>"#) - 0.9).abs() < 0.001);
    let absolute = r#"<div style="position:absolute;left:600px;width:100px;height:1px"></div>"#;
    assert!((at_scale(absolute) - 495.0 / 700.0).abs() < 0.001);
    let transformed = r#"<div style="transform:translateX(600px);width:100px;height:1px"></div>"#;
    assert!((at_scale(transformed) - 495.0 / 700.0).abs() < 0.001);
    let padded = r#"<div style="width:500px;padding-right:100px;height:1px"></div>"#;
    assert!((at_scale(padded) - 0.825).abs() < 0.001);
}

/// A box of `height` px with one line.
fn tall(height: u32, style: &str) -> String {
    format!(r#"<div style="height:{height}px;{style}">x</div>"#)
}

#[test]
fn a_cut_between_boxes_that_avoid_it_goes_to_the_last_clean_break() {
    let (after, before) = ("break-after:avoid", "break-before:avoid");
    // The box after does not fit: the box before goes with it, page 2 starts at its top.
    let a = spacer(300) + &tall(80, after);
    assert_eq!(page_tops(&(a.clone() + &tall(60, ""))), [0.0, 300.0]);
    assert_eq!(page_tops(&(spacer(300) + &tall(80, "") + &tall(60, before))), [0.0, 300.0]);
    assert_eq!(page_tops(&(spacer(300) + &tall(80, "break-after:avoid-page") + &tall(60, ""))), [0.0, 300.0]);
    assert_eq!(page_tops(&(spacer(300) + &tall(80, "") + &tall(60, "page-break-before:avoid"))), [0.0, 300.0]);
    // avoid-column is not about pages.
    assert_eq!(page_tops(&(spacer(300) + &tall(80, "break-after:avoid-column") + &tall(60, ""))), [0.0, 380.0]);
    // Chains move together.
    let chain = spacer(200) + &tall(60, after) + &tall(60, after) + &tall(60, after) + &tall(60, "");
    assert_eq!(page_tops(&chain), [0.0, 200.0]);
    // From a box's last child, and to a box's first child, through blocks, flow roots and flex.
    for wrap in ["", "overflow:hidden", "display:flow-root", "display:flex"] {
        let wrapped = format!(r#"<div style="{wrap}">{}</div>"#, tall(80, after));
        assert_eq!(page_tops(&(spacer(300) + &wrapped + &tall(60, ""))), [0.0, 300.0], "{wrap}");
    }
    let first = format!(r#"<div style="margin-top:2px">{}</div>"#, tall(60, before));
    assert_eq!(page_tops(&(spacer(300) + &tall(80, "") + &first)), [0.0, 300.0]);
    // Where something can be cut clean, it is: the lines above keep their widows.
    assert_eq!(page_tops(&(lines(19, after) + "x")), [0.0, 340.0]);
    assert_eq!(page_tops(&(lines(19, "") + &lines(1, before))), [0.0, 340.0]);
    assert_eq!(page_tops(&(spacer(300) + &lines(4, after) + &lines(3, ""))), [0.0, 340.0]);
    // Fewer lines than orphans and widows take: all of them move.
    assert_eq!(page_tops(&(spacer(320) + &lines(3, after) + &lines(3, ""))), [0.0, 320.0]);
    // A break before an empty box and one after it are in one place, and one has nothing to avoid.
    for empty in [
        format!("{}<div></div>{}", tall(80, after), tall(60, "")),
        format!(r#"{}<div style="height:0;{after}"></div>{}"#, tall(80, ""), tall(60, "")),
    ] {
        assert_eq!(page_tops(&(spacer(300) + &empty)), [0.0, 380.0]);
    }
}

#[test]
fn when_every_break_breaks_an_avoid_the_last_is_taken() {
    let (after, avoid) = ("break-after:avoid", "break-inside:avoid");
    // First box at the top of the page: the cut stays between the boxes.
    assert_eq!(page_tops(&(tall(370, after) + &tall(370, after) + &tall(370, ""))), [0.0, 370.0, 740.0]);
    // Only `widows` is left to break, which beats breaking the avoid.
    assert_eq!(page_tops(&(lines(3, after) + &tall(380, avoid))), [0.0, 40.0, 424.0]);
    assert_eq!(page_tops(&(lines(2, after) + &tall(380, avoid))), [0.0, 20.0, 404.0]);
    // Nothing else: the cut goes inside the `break-inside: avoid` further down.
    assert_eq!(page_tops(&(tall(20, after) + &tall(20, after) + &tall(380, avoid))), [0.0, 384.0]);
    // A page does not go back into a box that began on an earlier page.
    let resumed = format!("<div>{}{}{}</div>", tall(380, ""), tall(360, ""), tall(20, after));
    assert_eq!(page_tops(&(resumed + &tall(40, avoid))), [0.0, 380.0, 760.0]);
    assert_eq!(page_tops(&(lines(38, after) + &tall(40, avoid))), [0.0, 380.0, 760.0]);
    // Forced breaks win.
    assert_eq!(page_tops(&(spacer(300) + &tall(60, after) + &tall(60, "break-before:page"))), [0.0, 360.0]);
}

#[test]
fn monolithic_boxes_move_whole_and_hold_no_breaks() {
    // What Chrome moved as one, 200 px at 300 px of a 384 px page.
    for (open, close) in [
        (r#"<div style="contain:size;height:200px">"#, "</div>"),
        (r#"<div style="contain:strict;height:200px">"#, "</div>"),
        (r#"<div style="container-type:size;height:200px">"#, "</div>"),
        (r#"<div style="writing-mode:vertical-rl;height:200px;width:100px">"#, "</div>"),
        (r#"<button style="display:block;height:200px;margin:0">"#, "</button>"),
        (r#"<div style="display:inline-block;width:200px;height:200px;vertical-align:top">"#, "</div>"),
        (r#"<div style="display:inline-flex;width:200px;height:200px;vertical-align:top">"#, "</div>"),
    ] {
        assert_eq!(page_tops(&format!("{}{open}x{close}<div>y</div>", spacer(300))), [0.0, 300.0], "{open}");
    }
    for element in [
        r#"<canvas width=100 height=200 style="display:block"></canvas>"#,
        r#"<textarea style="display:block;height:200px;margin:0"></textarea>"#,
        r#"<input style="display:block;height:200px;margin:0">"#,
        r#"<select size=5 style="display:block;height:200px;margin:0"></select>"#,
        r#"<object style="display:block;width:100px;height:200px"></object>"#,
    ] {
        assert_eq!(page_tops(&format!("{}{element}<div>y</div>", spacer(300))), [0.0, 300.0], "{element}");
    }
    // What it did not.
    for style in [
        "contain:paint",
        "contain:layout",
        "contain:content",
        "contain:inline-size",
        "container-type:inline-size",
        "overflow:hidden",
        "overflow:auto",
        "overflow:scroll",
        "overflow:clip",
    ] {
        let html = format!(r#"{}<div style="height:200px;{style}">{}</div><div>y</div>"#, spacer(300), lines(10, ""));
        assert_eq!(page_tops(&html), [0.0, 380.0], "{style}");
    }
    // Its lines do not move a cut: it runs across pages at whole page heights.
    let tall_box = format!(r#"<div style="contain:size;height:600px">{}</div>"#, lines(30, ""));
    assert_eq!(page_tops(&(spacer(300) + &tall_box)), [0.0, 300.0, 684.0]);
    // Forced breaks inside are ignored, and on an inline-block too.
    let inside = r#"<div style="display:inline-block;width:200px"><div style="break-before:page">x</div><div style="break-before:page">y</div></div>"#;
    assert_eq!(page_tops(inside), [0.0]);
    let forced = r#"x<div style="display:inline-block;width:200px;break-before:page">y</div>"#;
    assert_eq!(page_tops(forced), [0.0]);
}

#[test]
fn the_page_after_a_break_after_starts_with_the_margin_of_what_follows() {
    let (at, chrome) = ("break-after:page;margin-bottom:40px", "margin-top:30px");
    // 20 px of a box, then margins of 40 and 30, which collapse. Chrome: the next box 30 px down page 2.
    let tops = |body: &str| page_tops(body);
    assert_eq!(tops(&(lines(1, at) + &lines(1, chrome))), [0.0, 30.0]);
    // Its own margin is what stays, however the two collapsed.
    assert_eq!(tops(&(lines(1, "break-after:page;margin-bottom:30px") + &lines(1, "margin-top:40px"))), [0.0, 20.0]);
    assert_eq!(tops(&(lines(1, at) + &lines(1, ""))), [0.0, 60.0]);
    assert_eq!(tops(&(lines(1, at) + "x")), [0.0, 60.0]);
    // Larger than a page, gone just the same.
    assert_eq!(tops(&(lines(1, "break-after:page;margin-bottom:400px") + &lines(1, chrome))), [0.0, 390.0]);
    // From a parent's margin, and collapsed with a child's.
    let parent = format!(r#"<div style="margin-bottom:50px">{}</div>"#, lines(1, "break-after:page"));
    assert_eq!(tops(&(parent + &lines(1, chrome))), [0.0, 40.0]);
    let child = format!(r#"<div style="margin-top:10px">{}</div>"#, lines(1, chrome));
    assert_eq!(tops(&(lines(1, at) + &child)), [0.0, 30.0]);
    // Padding stops the collapse: the box's own margin is none, the child stays 40 px down.
    let padded = format!(r#"<div style="padding-top:10px">{}</div>"#, lines(1, chrome));
    assert_eq!(tops(&(lines(1, at) + &padded)), [0.0, 60.0]);
}

/// Letter, 0.4 in margins, scale 1.5 (a 495 px limit) and a 400 px box of text; the rows of page 1 say the scale.
fn shrink_of(html: &str) -> f64 {
    let (paper, margins) = chrome_page_geometry((8.5, 11.0), [0.4; 4]).unwrap();
    let out = print_with(html, |o| {
        o.paper = paper;
        o.margins = margins;
        o.page_area = Some(chrome_page_area((8.5, 11.0), [0.4; 4]));
        o.scale = 1.5;
    });
    let found = rects(&inflated(&out.bytes));
    (found[0][3] - found[0][1]) / 11.25
}

#[test]
fn shrink_to_fit_leaves_out_fixed_and_empty_boxes_and_body_clips_nothing() {
    let link = r#"<a href="https://example.com/r" style="display:block;height:10px"></a>"#;
    let doc =
        |body_style: &str, body: &str| format!(r#"<!doctype html><body style="margin:0;{body_style}">{link}{body}"#);
    // Fixed boxes and their content do not count.
    let fixed = r#"<div style="position:fixed;left:600px;top:0;width:100px;height:10px"></div>"#;
    assert!((shrink_of(&doc("", fixed)) - 1.0).abs() < 0.001);
    let inside = r#"<div style="position:fixed;left:0;top:0;width:100px;height:10px"><div style="width:800px;height:10px"></div></div>"#;
    assert!((shrink_of(&doc("", inside)) - 1.0).abs() < 0.001);
    // An empty box counts for nothing, wherever it is, but a border or what is in it does.
    for empty in [
        r#"<div style="width:800px;height:0"></div>"#,
        r#"<div style="width:800px;height:0;float:left"></div>"#,
        r#"<div style="width:800px;height:0;position:absolute"></div>"#,
        r#"<div style="width:0;height:100px;margin-left:800px"></div>"#,
    ] {
        assert!((shrink_of(&doc("", empty)) - 1.0).abs() < 0.001, "{empty}");
    }
    let bordered = r#"<div style="width:800px;height:0;border-top:1px solid"></div>"#;
    assert!((shrink_of(&doc("", bordered)) - 495.0 / 742.5).abs() < 0.001);
    let child = r#"<div style="width:800px;height:0"><div style="width:600px;height:10px"></div></div>"#;
    assert!((shrink_of(&doc("", child)) - 495.0 / 742.5).abs() < 0.001);
    // `overflow` on body is the viewport's: what is in it counts, 600 px on a 495 px limit.
    let wide = r#"<div style="width:600px;height:10px"></div>"#;
    for style in
        ["overflow:hidden", "overflow:clip", "overflow:auto", "overflow-x:hidden", "overflow:hidden;width:300px"]
    {
        assert!((shrink_of(&doc(style, wide)) - 0.825).abs() < 0.001, "{style}");
    }
    // Not when the root has its own, or body contains its paint.
    let root_too =
        format!(r#"<!doctype html><html style="overflow:hidden"><body style="margin:0;overflow:hidden">{link}{wide}"#);
    assert!((shrink_of(&root_too) - 1.0).abs() < 0.001);
    assert!((shrink_of(&doc("overflow:hidden;contain:paint", wide)) - 1.0).abs() < 0.001);
}

#[test]
fn page_area_is_in_layout_units() {
    // (paper, margin, scale, rows Chrome's page holds), read off where a line stops fitting.
    for (paper, margin, scale, holds) in [
        ((8.27, 11.69), 0.4, 1.0, 1049.0),
        ((8.27, 11.69), 0.4, 0.8, 1311.0),
        ((8.27, 11.69), 0.4, 0.5, 2097.0),
        ((8.27, 11.69), 0.4, 2.0, 525.0),
        ((8.27, 11.69), 0.1, 1.5, 736.0),
        ((8.27, 11.69), 0.1, 0.75, 1473.0),
        ((8.5, 11.0), 0.25, 1.0, 1008.0),
        ((8.5, 11.0), 0.1, 1.1, 944.0),
        ((8.5, 11.0), 0.0, 1.1, 960.0),
    ] {
        let (paper_px, margins) = chrome_page_geometry(paper, [margin; 4]).unwrap();
        let out = print_with(r#"<!doctype html><body style="margin:0"><div style="height:9000px"></div>"#, |o| {
            o.paper = paper_px;
            o.margins = margins;
            o.page_area = Some(chrome_page_area(paper, [margin; 4]));
            o.scale = scale;
        });
        assert_eq!(out.pages[0].rect.height(), holds, "{paper:?} margin {margin} scale {scale}");
    }
}

#[test]
fn bullets_are_not_text_and_clipped_glyphs_are_not_written() {
    let doc = |body: &str| format!(r#"<!doctype html><body style="margin:0;font:16px/20px sans-serif">{body}"#);
    let text_of = |html: &str| {
        let out = print_with(html, |o| o.paper = (576.0, 384.0));
        inflated(&out.bytes)
    };
    // A marker's char is not mapped back to text: no ToUnicode for the marker font.
    let list = text_of(&doc(r#"<ul><li>one</li></ul>"#));
    assert!(!list.contains("2022"), "disc written as text");
    let numbered = text_of(&doc(r#"<ol style="list-style-type:upper-roman"><li>a</li><li>b</li></ol>"#));
    assert!(numbered.contains("0049"), "roman numeral is text");
    // Text past an overflow clip: a font's subset holds only the glyphs written.
    let full = print_with(&doc(r#"<div style="white-space:nowrap">abcdefghijklmnopqrstuvwxyz</div>"#), |o| {
        o.paper = (576.0, 384.0)
    });
    let clipped = print_with(
        &doc(
            r#"<div style="overflow:hidden;width:40px"><div style="white-space:nowrap">abcdefghijklmnopqrstuvwxyz</div></div>"#,
        ),
        |o| o.paper = (576.0, 384.0),
    );
    assert!(clipped.bytes.len() < full.bytes.len());
}

/// A link's rect on the paper: at 50% of the content, of the initial containing block and of the viewport. Chrome 156:
/// content fills the page area rounded up and the others see it rounded down, then the box edge snaps to a whole px.
#[test]
fn content_fills_the_page_area_rounded_up_and_viewport_units_see_it_rounded_down() {
    let lefts = |paper: (f64, f64), margin: f64, scale: f64, style: &str| {
        let (paper_px, margins) = chrome_page_geometry(paper, [margin; 4]).unwrap();
        let html = format!(
            r#"<!doctype html><body style="margin:0"><a href="https://x.test/l" style="display:block;height:10px;{style}"></a>"#
        );
        let out = print_with(&html, |o| {
            o.paper = paper_px;
            o.margins = margins;
            o.page_area = Some(chrome_page_area(paper, [margin; 4]));
            o.scale = scale;
        });
        rects(&inflated(&out.bytes))[0][0]
    };
    const A4: (f64, f64) = (8.27, 11.69);
    const LETTER: (f64, f64) = (8.5, 11.0);
    // (paper, margin, scale, block at 50%, absolute at 50%, 50vw), left edges in pt.
    for (paper, margin, scale, block, absolute, vw) in [
        (A4, 0.0, 1.0, 298.5, 297.75, 297.75),
        (A4, 0.4, 1.0, 298.5, 297.75, 297.75),
        (LETTER, 0.4, 1.0, 306.0, 306.0, 306.0),
        (LETTER, 0.4, 0.8, 306.15, 305.55, 305.55),
        (A4, 0.0, 1.5, 298.125, 298.125, 298.125),
    ] {
        let at = |style| lefts(paper, margin, scale, style);
        let context = format!("{paper:?} margin {margin} scale {scale}");
        assert!((at("margin-left:50%") - block).abs() < 0.01, "block {context}");
        assert!((at("position:absolute;left:50%;width:10px") - absolute).abs() < 0.01, "absolute {context}");
        assert!((at("margin-left:50vw") - vw).abs() < 0.01, "vw {context}");
    }
}
