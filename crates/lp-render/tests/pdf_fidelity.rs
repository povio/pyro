//! PDF output of paper-cut documents: link annotations, tags, source text
//! (ligatures), blurred shadows, paper-width layout and page breaks.

use std::sync::Arc;

use lp_render::*;

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
fn inflated(pdf: &[u8]) -> String {
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

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
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
    // the cmap reverse lookup alone gave U+FB01 (ﬁ) / U+FB03.
    let out =
        print(r#"<!doctype html><body style="font:28px Helvetica, Arial, sans-serif">office fit affluent</body>"#);
    assert!(out.stats.runs_with_source > 0, "{:?}", out.stats);
    assert_eq!(out.stats.glyphs_without_text, 0, "{:?}", out.stats);
    let pdf = inflated(&out.bytes).to_uppercase();
    assert!(pdf.contains("BEGINBFCHAR"), "the font's ToUnicode map");
    for lig in ["<FB01>", "<FB02>", "<FB03>", "<FB04>"] {
        assert!(!pdf.contains(lig), "a ligature maps to {lig}");
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
fn page_boxes_keep_their_output() {
    // `.pz-page` documents: no tags, no recorded text, no links; blurred
    // shadows are drawn (LIGHTNINGPANDA_PDF_SHADOWS=0 skips them).
    let out = print(
        r#"<!doctype html><style>body{margin:0}.pz-page{width:400px;height:500px;box-shadow:0 0 20px #000}</style>
        <div class="pz-page"><h1>One</h1><a href="https://example.com/">x</a></div>"#,
    );
    assert!(out.by_selector);
    assert_eq!((out.stats.links, out.stats.runs_with_source), (0, 0));
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
    let (paper, margins) = lp_render::chrome_page_geometry((8.5, 11.0), [0.4; 4]).unwrap();
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
    let (paper, margins) = lp_render::chrome_page_geometry((8.5, 11.0), [0.4; 4]).unwrap();
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
