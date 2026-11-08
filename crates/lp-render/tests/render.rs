use std::sync::Arc;

use lp_render::worker::{match_baseline, Baseline, MatchOptions};
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
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
        device_scale_factor: 1.0,
        ..Default::default()
    }
}

const CARD: &str = r#"<!doctype html><html><head><style>
  body { margin: 0; background: #f3f4f6; }
  .card { width: 300px; height: 120px; margin: 40px; background: #6d28d9; }
</style></head><body><div class="card" id="card" data-lp-render-clip></div></body></html>"#;

fn fetcher() -> Arc<dyn Fetcher> {
    Arc::new(NoFetch)
}

fn pixel(img: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
    let i = (y as usize * img.width as usize + x as usize) * 4;
    img.data[i..i + 4].try_into().unwrap()
}

#[test]
fn renders_deterministically_on_the_cpu() {
    let mut r = Renderer::new();
    let (a, ha, _) = r.render(&snap(CARD), Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    let (b, hb, _) = r.render(&snap(CARD), Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    assert_eq!(ha, hb);
    assert!(compare(&a, &b, 0, None).equal);
    assert_eq!(pixel(&a, 100, 100), [0x6d, 0x28, 0xd9, 255]);
    assert_eq!(decode_png(&encode_png(&a)).unwrap(), a);
}

#[test]
fn element_area_is_the_border_box() {
    let mut r = Renderer::new();
    let (img, _, _) = r.render(&snap(CARD), Area::Element, Backend::Cpu, &fetcher()).unwrap();
    assert_eq!((img.width, img.height), (300, 120));
    assert_eq!(pixel(&img, 0, 0), [0x6d, 0x28, 0xd9, 255]);
    assert_eq!(pixel(&img, 299, 119), [0x6d, 0x28, 0xd9, 255]);
}

#[test]
fn a_hash_hit_skips_raster_and_a_change_fails() {
    let mut r = Renderer::new();
    let (base, _, _) = r.render(&snap(CARD), Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    let b = Baseline::from_image(base, None);
    let opts = MatchOptions::default();
    let first = match_baseline(&mut r, &snap(CARD), Area::Viewport, Some(&b), opts, &fetcher()).unwrap();
    assert!(first.pass && !first.hash_hit);
    let second = match_baseline(&mut r, &snap(CARD), Area::Viewport, Some(&b), opts, &fetcher()).unwrap();
    assert!(second.pass && second.hash_hit && second.timings.raster.is_zero());
    let changed = snap(&CARD.replace("#6d28d9", "#6d28da"));
    let third = match_baseline(&mut r, &changed, Area::Viewport, Some(&b), opts, &fetcher()).unwrap();
    assert!(!third.pass && third.diff_pixels == 300 * 120, "{third:?}");
    assert!(third.actual.is_some() && third.diff.is_some());
    let tolerant = MatchOptions { tolerance: 1, ..opts };
    assert!(match_baseline(&mut r, &changed, Area::Viewport, Some(&b), tolerant, &fetcher()).unwrap().pass);
}

#[test]
fn sheets_and_images_come_through_the_bridge() {
    // A 1x1 red PNG served by the fetcher, a link sheet from the snapshot.
    let red = encode_png(&RgbaImage { width: 1, height: 1, data: vec![255, 0, 0, 255] });
    struct One(Vec<u8>);
    impl Fetcher for One {
        fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
            let body = self.0.clone();
            std::thread::spawn(move || done(if url.ends_with("red.png") { Ok(body) } else { Err("404".into()) }));
        }
    }
    let mut s = snap(
        r#"<!doctype html><head><link rel="stylesheet" href="http://test/a.css"></head><body><img src="red.png" width="50" height="50"></body>"#,
    );
    s.sheets.insert("http://test/a.css".into(), "body { margin: 0; background: #00ff00 }".into());
    let fetcher: Arc<dyn Fetcher> = Arc::new(One(red));
    let mut r = Renderer::new();
    let (img, _, t) = r.render(&s, Area::Viewport, Backend::Cpu, &fetcher).unwrap();
    assert_eq!(pixel(&img, 25, 25), [255, 0, 0, 255], "image painted ({t:?})");
    assert_eq!(pixel(&img, 200, 200), [0, 255, 0, 255], "linked sheet applied");
}

const LATO: &[u8] = include_bytes!("../../../bench/screenshot/fixtures/fonts/Lato-Regular.ttf");
const TEXT: &str = r#"<!doctype html><html><head><style>
  body { margin: 0; font: 40px/1 Lato, serif; }
</style></head><body>Hamburgefonstiv</body></html>"#;

fn hash_of(r: &mut Renderer, s: &Snapshot) -> u64 {
    r.paint(s, Area::Viewport, &fetcher()).unwrap().hash
}

#[test]
fn renderers_share_system_fonts_but_not_registered_ones() {
    // Fonts are scanned once per process; each renderer works on a clone.
    let mut plain = Renderer::new();
    let fallback = hash_of(&mut plain, &snap(TEXT));
    let mut with_lato = Renderer::new();
    with_lato.register_font(LATO.to_vec());
    let lato = hash_of(&mut with_lato, &snap(TEXT));
    assert_ne!(fallback, lato, "the registered font is used");
    // A font registered on one renderer does not leak into later ones.
    assert_eq!(hash_of(&mut Renderer::new(), &snap(TEXT)), fallback);
    // Renderers on other threads (one per CDP connection) render the same.
    let others: Vec<u64> = (0..4)
        .map(|_| std::thread::spawn(|| hash_of(&mut Renderer::new(), &snap(TEXT))))
        .collect::<Vec<_>>()
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect();
    assert!(others.iter().all(|&h| h == fallback), "{others:x?} vs {fallback:x}");
    lp_render::prewarm().unwrap().join().unwrap();
    assert_eq!(hash_of(&mut Renderer::new(), &snap(TEXT)), fallback);
}

/// Serves Lato after `delay`, on another thread (as the page's loader does).
struct SlowFont(std::time::Duration);
impl Fetcher for SlowFont {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let delay = self.0;
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            done(if url.ends_with("lato.ttf") { Ok(LATO.to_vec()) } else { Err("404".into()) })
        });
    }
}

#[test]
fn the_first_layout_waits_for_web_fonts() {
    let web_font = TEXT.replace("<style>", r#"<style>@font-face { font-family: Lato; src: url(lato.ttf); }"#);
    let mut expected = Renderer::new();
    expected.register_font(LATO.to_vec());
    let lato = hash_of(&mut expected, &snap(TEXT));

    // The font arrives within the block period: one layout, with the font.
    let slow: Arc<dyn Fetcher> = Arc::new(SlowFont(std::time::Duration::from_millis(150)));
    let mut r = Renderer::new();
    let painted = r.paint(&snap(&web_font), Area::Viewport, &slow).unwrap();
    assert_eq!(painted.timings.layout_passes, 1);
    assert_eq!(painted.hash, lato, "laid out with the web font");

    // Past the block period the page is laid out with fallback fonts, then
    // again when the font arrives (within the fetch timeout).
    let mut r = Renderer::new();
    r.first_layout_wait = std::time::Duration::from_millis(20);
    let painted = r.paint(&snap(&web_font), Area::Viewport, &slow).unwrap();
    assert_eq!(painted.timings.layout_passes, 2);
    assert_eq!(painted.hash, lato);
}

#[test]
fn class_selectors_match_as_before() {
    // blitz-dom interns class tokens once per element (upstream #1240,
    // formerly a local patch); matching must not change.
    let html = "<!doctype html><html><head><style>
      body { margin: 0 } div { width: 10px; height: 10px; background: #000 }
      .a.b { background: #f00 } [class~=c] { background: #0f0 } .D { background: #00f }
    </style></head><body>
      <div class=\"b\ta\n\"></div><div class=\" x c \"></div><div class=\"d\"></div><div class=\"D\"></div><div class=\"\"></div>
    </body></html>";
    let mut r = Renderer::new();
    let (img, _, _) = r.render(&snap(html), Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    let at = |i: u32| pixel(&img, 5, i * 10 + 5);
    assert_eq!(at(0), [255, 0, 0, 255], "tokens split on any ASCII whitespace");
    assert_eq!(at(1), [0, 255, 0, 255], "[class~=] uses the same tokens");
    assert_eq!(at(2), [0, 0, 0, 255], "class names are case-sensitive");
    assert_eq!(at(3), [0, 0, 255, 255]);
    assert_eq!(at(4), [0, 0, 0, 255]);
}

#[test]
fn class_changes_invalidate_the_cached_tokens() {
    use blitz_dom::{local_name, ns, DocumentConfig, QualName};
    let mut doc = blitz_html::HtmlDocument::from_html(
        "<!doctype html><body><p id=p class=\"a\">x</p></body>",
        DocumentConfig::default(),
    );
    let p = doc.query_selector("#p").unwrap().unwrap();
    assert_eq!(doc.query_selector(".a").unwrap(), Some(p));
    let class = QualName::new(None, ns!(), local_name!("class"));
    doc.mutate().set_attribute(p, class.clone(), "b");
    assert_eq!(doc.query_selector(".a").unwrap(), None);
    assert_eq!(doc.query_selector(".b").unwrap(), Some(p));
    doc.mutate().clear_attribute(p, class);
    assert_eq!(doc.query_selector(".b").unwrap(), None);
}

#[test]
fn bench_fixtures_lay_out_once_with_their_web_font() {
    // The fixtures of bench/screenshot, with Lato as a web font as the
    // benchmark serves it: one layout, and the same display list as with
    // the font already registered.
    let face = r#"<style>@font-face { font-family: Lato; src: url(lato.ttf) format("truetype"); }</style>"#;
    let fast: Arc<dyn Fetcher> = Arc::new(SlowFont(std::time::Duration::from_millis(5)));
    for name in ["login", "dashboard", "article"] {
        let path = format!("{}/../../bench/screenshot/fixtures/{name}.html", env!("CARGO_MANIFEST_DIR"));
        let html = std::fs::read_to_string(path).unwrap();
        let mut s = snap(&html.replace("<head>", &format!("<head>{face}")));
        (s.width, s.height) = (1280, 720);
        let painted = Renderer::new().paint(&s, Area::Viewport, &fast).unwrap();
        assert_eq!(painted.timings.layout_passes, 1, "{name}");
        let mut registered = Renderer::new();
        registered.register_font(LATO.to_vec());
        assert_eq!(painted.hash, hash_of(&mut registered, &s), "{name}");
    }
}

#[test]
fn fixed_drawer_paints_above_its_ancestor_backdrop() {
    let s = snap(
        r#"<!doctype html><style>
      body {margin:0;background:white}
      .overlay {position:fixed;inset:0;z-index:60;background:rgba(0,0,0,.4)}
      .drawer {position:fixed;left:40px;top:30px;width:200px;height:100px;background:white}
    </style><div class="overlay"><div class="drawer" data-lp-render-clip>Drawer</div></div>"#,
    );
    let (img, _, _) = Renderer::new().render(&s, Area::Element, Backend::Cpu, &fetcher()).unwrap();
    assert_eq!((img.width, img.height), (200, 100));
    assert_eq!(pixel(&img, 100, 70), [255, 255, 255, 255], "a drawer must paint above its own backdrop");
}

#[test]
fn inline_highlight_paints_its_background() {
    let s = snap(
        r#"<!doctype html><style>body{margin:0;background:white;font:20px/30px Arial}mark{background:#ffff00}</style><div><mark><strong>Highlighted words</strong></mark></div>"#,
    );
    let (img, _, _) = Renderer::new().render(&s, Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    let yellow = img.data.chunks_exact(4).filter(|p| p[0] == 255 && p[1] == 255 && p[2] == 0 && p[3] == 255).count();
    assert!(yellow > 100, "inline highlight background missing: {yellow} yellow pixels");
}

#[test]
fn fixed_descendant_retains_viewport_coordinates_in_offset_scrolled_context() {
    let mut s = snap(
        r#"<!doctype html><style>
      body {margin:0;height:1000px;background:white}
      .overlay {position:fixed;left:80px;top:40px;width:300px;height:250px;z-index:60;background:rgba(0,0,0,.4)}
      .drawer {position:fixed;left:40px;top:30px;width:200px;height:100px;background:#00ff00}
    </style><div class="overlay"><div class="drawer" data-lp-render-clip></div></div>"#,
    );
    s.scroll_y = 75.0;
    let (img, _, _) = Renderer::new().render(&s, Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    assert_eq!(pixel(&img, 50, 40), [0, 255, 0, 255]);
    assert_eq!(pixel(&img, 230, 120), [0, 255, 0, 255]);
}

#[test]
fn fixed_descendant_cannot_escape_ancestor_stacking_context() {
    let s = snap(
        r#"<!doctype html><style>
      body {margin:0;background:white}
      .overlay {position:fixed;inset:0;z-index:60;background:rgba(0,0,0,.4)}
      .drawer {position:fixed;left:40px;top:30px;width:200px;height:100px;z-index:999;background:green}
      .above {position:fixed;left:40px;top:30px;width:200px;height:100px;z-index:61;background:blue}
    </style><div class="overlay"><div class="drawer" data-lp-render-clip></div></div><div class="above"></div>"#,
    );
    let (img, _, _) = Renderer::new().render(&s, Area::Element, Backend::Cpu, &fetcher()).unwrap();
    assert_eq!(pixel(&img, 100, 70), [0, 0, 255, 255]);
}

#[test]
fn auto_table_columns_grow_beyond_first_row_width_hints() {
    let s = snap(
        r#"<!doctype html><style>
      body {margin:0} table {width:380px;table-layout:auto;border-spacing:0}
      th,td {width:25px;min-width:25px;white-space:nowrap;overflow:hidden}
    </style><table><thead><tr><th></th><th>Show</th><th>Column</th></tr></thead>
    <tbody><tr><td>+</td><td>yes</td><td data-lp-render-clip>Destination Service Provider</td></tr></tbody></table>"#,
    );
    let (img, _, _) = Renderer::new().render(&s, Area::Element, Backend::Cpu, &fetcher()).unwrap();
    assert!(img.width > 150, "auto table label column collapsed to {}px", img.width);
}

#[test]
fn element_crop_includes_ancestor_translation_and_scale() {
    let s = snap(
        r#"<!doctype html><style>body{margin:0}.sheet{margin-left:10px;transform:translate(50px,70px) scale(1.5);transform-origin:0 0}.card{width:80px;height:40px;background:#6d28d9}</style><div class="sheet"><div class="card" data-lp-render-clip></div></div>"#,
    );
    let (img, _, _) = Renderer::new().render(&s, Area::Element, Backend::Cpu, &fetcher()).unwrap();
    assert_eq!((img.width, img.height), (120, 60));
    assert_eq!(pixel(&img, 5, 5), [0x6d, 0x28, 0xd9, 255]);
    assert_eq!(pixel(&img, 115, 55), [0x6d, 0x28, 0xd9, 255]);
}

struct SpriteFetch;
impl Fetcher for SpriteFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        assert_eq!(url, "http://test/icons.svg");
        done(Ok(br#"<svg xmlns="http://www.w3.org/2000/svg"><symbol id="check" viewBox="0 0 24 24"><path d="M4 12L9 17L20 6"/></symbol></svg>"#.to_vec()));
    }
}
#[test]
fn external_svg_symbol_keeps_current_color_stroke_and_viewbox() {
    let mut renderer = Renderer::new();
    let markup = r##"<html><body style="margin:0"><svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" style="color:red"><use href="/icons.svg#check"/></svg><svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" style="color:blue"><use href="/icons.svg#check"/></svg></body></html>"##;
    let reference = markup.replace(r##"<use href="/icons.svg#check"/>"##, r#"<path d="M4 12L9 17L20 6"/>"#);
    let sprite: Arc<dyn Fetcher> = Arc::new(SpriteFetch);
    let (actual, _, _) = renderer.render(&snap(markup), Area::FullPage, Backend::Cpu, &sprite).unwrap();
    let (expected, _, _) = renderer.render(&snap(&reference), Area::FullPage, Backend::Cpu, &fetcher()).unwrap();
    assert!(compare(&actual, &expected, 0, None).equal);
    assert!(actual.data.chunks_exact(4).any(|p| p == [255, 0, 0, 255]));
    assert!(actual.data.chunks_exact(4).any(|p| p == [0, 0, 255, 255]));
}
#[test]
fn full_page_does_not_include_overflow_clipped_by_the_root() {
    let mut renderer = Renderer::new();
    let markup = "<html style='height:300px;overflow:hidden'><body style='margin:0'><div style='height:900px;background:red'></div></body></html>";
    let (image, _, _) = renderer.render(&snap(markup), Area::FullPage, Backend::Cpu, &fetcher()).unwrap();
    assert_eq!((image.width, image.height), (400, 300));
}

#[test]
fn absolute_popover_in_display_contents_paints_above_fixed_drawer() {
    let markup = "<html style='height:300px;overflow:hidden'><body style='margin:0'><div style='height:300px'></div><div style='position:fixed;inset:0;background:red;z-index:50'></div><div style='display:contents'><div style='position:absolute;left:200px;top:100px;width:100px;height:60px;background:lime;z-index:100000'></div></div></body></html>";
    let (image, _, _) = Renderer::new().render(&snap(markup), Area::FullPage, Backend::Cpu, &fetcher()).unwrap();
    assert_eq!(pixel(&image, 220, 120), [0, 255, 0, 255]);
}

#[test]
fn external_svg_missing_symbol_fails_instead_of_silently_drawing_nothing() {
    let sprite: Arc<dyn Fetcher> = Arc::new(SpriteFetch);
    let result = Renderer::new().render(
        &snap(r##"<svg width="24" height="24"><use href="/icons.svg#missing"/></svg>"##),
        Area::Viewport,
        Backend::Cpu,
        &sprite,
    );
    assert!(result.err().unwrap().to_string().contains("missing SVG fragment"));
}

struct MultipleSprites;
impl Fetcher for MultipleSprites {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let color = if url.contains("first.svg") { "red" } else { "blue" };
        done(Ok(format!(r#"<svg xmlns="http://www.w3.org/2000/svg"><symbol id="same" viewBox="0 0 10 10"><rect width="10" height="10" fill="{color}"/></symbol></svg>"#).into_bytes()));
    }
}
#[test]
fn external_symbols_with_equal_ids_and_escaped_urls_remain_distinct() {
    let source = r##"<body style="margin:0"><svg width="20" height="10"><use href="/first.svg?a=1&amp;b=2#same" width="10" height="10"/><use href="/second.svg#same" x="10" width="10" height="10"/></svg></body>"##;
    let fetch: Arc<dyn Fetcher> = Arc::new(MultipleSprites);
    let (image, _, _) = Renderer::new().render(&snap(source), Area::Viewport, Backend::Cpu, &fetch).unwrap();
    assert_eq!(pixel(&image, 5, 5), [255, 0, 0, 255]);
    assert_eq!(pixel(&image, 15, 5), [0, 0, 255, 255]);
}

/// Element border-box width as Blitz lays it out (via an element capture).
fn element_width(html: &str) -> u32 {
    let mut r = Renderer::new();
    r.paint(&snap(html), Area::Element, &fetcher()).unwrap().width
}

#[test]
fn a_centered_flex_item_is_clamped_to_its_container() {
    // ToS invoice footers: a flex row in an `align-items: center` column is
    // fit-content (min(max-content, available)), so its text wraps.
    let text = "word ".repeat(200);
    let html = format!(
        r#"<!doctype html><html><body style="margin:0"><div style="display:flex;flex-direction:column;align-items:center">
        <div data-lp-render-clip style="display:flex;gap:30px"><p style="flex-shrink:0;margin:0">fixed</p><div><p style="margin:0">{text}</p></div></div>
        </div></body></html>"#
    );
    assert_eq!(element_width(&html), 400);
}

#[test]
fn a_fit_content_flex_row_is_clamped_to_the_available_width() {
    let text = "word ".repeat(200);
    let html = format!(
        r#"<!doctype html><html><body style="margin:0"><div data-lp-render-clip style="display:flex;width:fit-content">
        <div>{text}</div></div></body></html>"#
    );
    assert_eq!(element_width(&html), 400);
}

#[test]
fn a_padded_inline_flex_keeps_its_max_content_width() {
    // ToS "Add Cover" quick-action preview: the clamp compares outer sizes, so
    // an inline-flex measured at its own max-content width is not shrunk by
    // its padding and border.
    let html = r#"<!doctype html><html><body style="margin:0"><div style="display:flex;width:600px"><div style="flex:1"></div>
        <div><span data-lp-render-clip style="display:inline-flex;border:1px solid;padding:0 12px"><span style="width:60px;min-width:0;height:10px"></span></span></div>
        </div></body></html>"#;
    assert_eq!(element_width(html), 86);
}

#[test]
fn a_replaced_flex_item_shrinks_to_its_natural_width() {
    // ToS pdfgen logos: `<svg width=700 height=100 class="h-[37mm] w-auto">`
    // in a centered flex row. Its automatic minimum width is its natural
    // width (700), not its height through the ratio (980), so it shrinks to
    // the container as in Chromium.
    let html = r#"<!doctype html><html><body style="margin:0"><div style="display:flex;justify-content:center;width:719px">
        <svg data-lp-render-clip xmlns="http://www.w3.org/2000/svg" width="700" height="100" viewBox="0 0 700 100" style="height:140px;width:auto"><rect width="700" height="100"/></svg>
        </div></body></html>"#;
    assert_eq!(element_width(html), 719);
}

#[test]
/// `:has()` matches (Stylo gates it behind a pref Blitz did not set).
fn has_selectors_match() {
    let html = r#"<!doctype html><html><style>.group:has(input:disabled) .icon{width:150px} .icon{width:20px;height:10px} .g2:has(.x) .icon{width:77px}</style><body style="margin:0"><div class=group><div><input disabled></div><div><div class=icon data-lp-render-clip></div></div></div></body></html>"#;
    let a = element_width(html);
    let html2 = r#"<!doctype html><html><style>.g2:has(.x) .icon{width:77px} .icon{width:20px;height:10px}</style><body style="margin:0"><div class=g2><div class=x></div><div class=icon data-lp-render-clip></div></div></body></html>"#;
    let b = element_width(html2);
    assert_eq!((a, b), (150, 77));
}

#[test]
fn identical_text_keeps_each_elements_colour() {
    // Layouts built from the same text and styles are reused (parley's build
    // cache); each must still paint with its own element's colour.
    let s = snap(
        r#"<!doctype html><style>body{margin:0;background:white;font:bold 40px/50px Arial}</style><div style="color:#ff0000">WWWW</div><div style="color:#0000ff">WWWW</div>"#,
    );
    let (img, _, _) = Renderer::new().render(&s, Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    let count = |y0: u32, y1: u32, rgb: [u8; 3]| {
        (y0..y1)
            .flat_map(|y| (0..img.width).map(move |x| (x, y)))
            .filter(|&(x, y)| pixel(&img, x, y)[..3] == rgb)
            .count()
    };
    assert!(count(0, 50, [255, 0, 0]) > 100, "first line is red");
    assert!(count(50, 100, [0, 0, 255]) > 100, "second line is blue");
    assert_eq!(count(0, 50, [0, 0, 255]), 0, "no blue on the first line");
    assert_eq!(count(50, 100, [255, 0, 0]), 0, "no red on the second line");
}

fn print_with(html: &str, f: impl FnOnce(&mut PdfOptions)) -> PdfOutput {
    let mut opts = PdfOptions::new(".pz-page", (816.0, 1056.0));
    f(&mut opts);
    Renderer::new().print_pdf(&snap(html), &fetcher(), &opts).unwrap()
}

const TALL: &str =
    r#"<!doctype html><body style="margin:0;background:#eee"><div style="height:3000px;background:#ddd"></div>"#;

#[test]
fn print_pdf_cuts_the_paper_content_box_by_margins_and_scale() {
    assert_eq!(print_with(TALL, |_| {}).pages.len(), 3);
    let margins = print_with(TALL, |o| o.margins = [96.0, 48.0, 96.0, 48.0]);
    assert_eq!(margins.pages.len(), 3000usize.div_ceil(1056 - 192), "content box height");
    assert_eq!(margins.pages[0].rect.width(), 816.0 - 96.0);
    let half = print_with(TALL, |o| o.scale = 0.5);
    assert_eq!(half.pages.len(), 2, "at 50% a page holds twice the document");
    assert_eq!(half.pages[0].rect.height(), 2112.0);
}

#[test]
fn print_pdf_keeps_the_requested_page_ranges() {
    let all = print_with(TALL, |_| {});
    let some = print_with(TALL, |o| o.page_ranges = vec![(1, 1), (3, usize::MAX)]);
    assert_eq!(some.pages.len(), 2);
    assert_eq!((some.pages[0].rect, some.pages[1].rect), (all.pages[0].rect, all.pages[2].rect));
    let opts = PdfOptions { page_ranges: vec![(4, 4)], ..PdfOptions::new(".pz-page", (816.0, 1056.0)) };
    assert!(Renderer::new().print_pdf(&snap(TALL), &fetcher(), &opts).is_err(), "past the last page");
}

#[test]
fn print_pdf_without_backgrounds_drops_their_fills() {
    let with = print_with(TALL, |_| {});
    let without = print_with(TALL, |o| o.print_background = false);
    assert!(without.stats.fills < with.stats.fills, "{:?} vs {:?}", without.stats, with.stats);
}

#[test]
fn print_media_applies_print_rules() {
    let html = "<!doctype html><style>body{margin:0;background:#00f}@media print{body{background:#f00}}</style>";
    let mut r = Renderer::new();
    let (screen, _, _) = r.render(&snap(html), Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    assert_eq!(pixel(&screen, 5, 5), [0, 0, 255, 255]);
    let (print, _, _) =
        r.render(&Snapshot { print: true, ..snap(html) }, Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    assert_eq!(pixel(&print, 5, 5), [255, 0, 0, 255]);
}

#[test]
fn a_clip_within_the_viewport_keeps_the_scroll_and_fixed_elements() {
    let html = r#"<!doctype html><style>body{margin:0}
      header{position:fixed;top:0;left:0;width:100%;height:40px;background:#f00}
      main{height:3000px;background:#00f}</style><header></header><main></main>"#;
    let mut r = Renderer::new();
    let mut s = snap(html);
    s.scroll_y = 500.0;
    let clip =
        |beyond_viewport| Area::Clip { x: 0.0, y: 500.0, width: 400.0, height: 300.0, scale: 1.0, beyond_viewport };
    let (img, _, _) = r.render(&s, clip(false), Backend::Cpu, &fetcher()).unwrap();
    assert_eq!(pixel(&img, 10, 10), [255, 0, 0, 255], "the fixed header at the top of the visible viewport");
    assert_eq!(pixel(&img, 10, 100), [0, 0, 255, 255]);
    let (viewport, _, _) = r.render(&s, Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    assert_eq!(img.data, viewport.data, "the viewport's rect equals the viewport capture");
    let (img, _, _) = r.render(&s, clip(true), Backend::Cpu, &fetcher()).unwrap();
    assert_eq!(pixel(&img, 10, 10), [0, 0, 255, 255], "beyond the viewport: the header stays at the document top");
}

fn print(html: &str) -> PdfOutput {
    let opts = PdfOptions::new(".pz-page", (816.0, 1056.0));
    Renderer::new().print_pdf(&snap(html), &fetcher(), &opts).unwrap()
}

#[test]
fn print_pdf_makes_one_page_per_page_box() {
    let out = print(
        r#"<!doctype html><style>body{margin:0}.pz-page{width:400px;height:500px;background:#eee}</style>
        <div class="pz-page">one</div><div class="pz-page">two</div>"#,
    );
    assert!(out.bytes.starts_with(b"%PDF"));
    assert!(out.by_selector);
    assert_eq!(out.pages.len(), 2);
    assert_eq!((out.pages[1].rect.y0, out.pages[1].rect.height()), (500.0, 500.0));
}

#[test]
fn print_pdf_cuts_a_plain_document_by_the_paper() {
    let out = print(r#"<!doctype html><body style="margin:0"><div style="height:3000px;background:#ddd"></div>"#);
    assert!(out.bytes.starts_with(b"%PDF"));
    assert!(!out.by_selector);
    assert_eq!(out.pages.len(), 3000usize.div_ceil(1056));
}

#[test]
fn print_pdf_skips_degenerate_fills() {
    let out = print(
        r#"<!doctype html><body style="margin:0"><div style="width:200px;height:100px;border:0 solid red;border-top-width:2px"></div>
        <div style="width:200px;height:100px;background:rgba(0,255,0,0)"></div>"#,
    );
    assert!(out.stats.degenerate_skipped > 0, "{:?}", out.stats);
}

/// `overflow` on `body` is the viewport's unless the root has its own or `body` contains its paint. A 600 px red box
/// in a 300 px `body` on an 800 x 600 viewport, the pixel at 500,20 of Chrome 156's screenshot.
#[test]
fn overflow_on_body_is_the_viewports() {
    const RED: [u8; 4] = [255, 0, 0, 255];
    const WHITE: [u8; 4] = [255, 255, 255, 255];
    let wide = r#"<div style="width:600px;height:50px;background:#f00"></div>"#;
    for (html, body, chrome) in [
        ("", "overflow:hidden", RED),
        ("overflow:hidden", "overflow:hidden", WHITE),
        ("", "overflow:hidden;contain:paint", WHITE),
    ] {
        let page = format!(r#"<!doctype html><html style="{html}"><body style="margin:0;width:300px;{body}">{wide}"#);
        let snapshot = Snapshot { width: 800, height: 600, ..snap(&page) };
        let (img, _, _) = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
        assert_eq!(pixel(&img, 500, 20), chrome, "html {html:?} body {body:?}");
    }
    // Any other box clips.
    let page =
        format!(r#"<!doctype html><body style="margin:0"><div style="overflow:hidden;width:300px">{wide}</div>"#);
    let snapshot = Snapshot { width: 800, height: 600, ..snap(&page) };
    let (img, _, _) = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    assert_eq!(pixel(&img, 500, 20), WHITE);
}

/// Text in screenshots stays at its box's whole-pixel x; only PDF output moves it.
#[test]
fn screenshot_text_keeps_the_box_x() {
    let page = |padding: &str| {
        format!("<!doctype html><body style='margin:0;font:16px sans-serif'><div style='padding-left:{padding}'>Whole pixels</div>")
    };
    let mut r = Renderer::new();
    let (a, _, _) = r.render(&snap(&page("37.7953px")), Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    let (b, _, _) = r.render(&snap(&page("38px")), Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    assert!(compare(&a, &b, 0, None).equal);
}

/// Mixed fonts, sizes, scripts and a missing first family: every shaping path
/// (font fallback, bold/italic synthesis, run metrics, style metrics).
const TEXT_PAGE: &str = r#"<!doctype html><html><head><style>
  body { margin: 8px; font: 14px/1.4 Helvetica, Arial, sans-serif; width: 380px; }
  h1 { font: bold 22px Times, serif; }
  code { font-family: Courier, monospace; font-size: 12px; }
  .big { font-size: 18px; } .small { font-size: 10px; vertical-align: super; }
</style></head><body>
<h1>Shaping <i>twice</i></h1>
<p>Plain <b>bold</b> and <i>italic</i>, <span class="big">bigger</span> and <span class="small">smaller</span>, <code>mono text</code>.</p>
<p>Кириллица, Ελληνικά, 中文字, العربية, עברית, emoji 😀🎉 and ﬁ ligatures.</p>
<p style="font-family: 'No Such Font', Helvetica">Missing first family then Helvetica <b>bold</b>.</p>
</body></html>"#;

/// Text laid out again on a thread (its glyph cache holds the runs and their
/// metrics) renders what a thread shaping it for the first time renders.
#[test]
fn text_shaped_from_the_glyph_cache_renders_as_shaped_fresh() {
    let fresh = std::thread::spawn(|| {
        let mut r = Renderer::new();
        let (img, hash, _) = r.render(&snap(TEXT_PAGE), Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
        (img, hash)
    })
    .join()
    .unwrap();
    let mut r = Renderer::new();
    let (a, ha, _) = r.render(&snap(TEXT_PAGE), Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    let (b, hb, _) = r.render(&snap(TEXT_PAGE), Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    let mut other = Renderer::new();
    let (c, hc, _) = other.render(&snap(TEXT_PAGE), Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    assert!(a.data.chunks_exact(4).any(|p| p[0] < 128), "text was drawn");
    assert_eq!((ha, hb, hc), (fresh.1, fresh.1, fresh.1));
    assert!(compare(&a, &fresh.0, 0, None).equal);
    assert!(compare(&a, &b, 0, None).equal);
    assert!(compare(&a, &c, 0, None).equal);
    // The PNG row filter keeps the pixels.
    assert_eq!(decode_png(&encode_png(&a)).unwrap(), a);
}

/// JPEG screenshots decode to the render within the usual lossy error, and
/// `quality` keeps libjpeg's meaning (higher is larger and closer).
#[test]
fn jpeg_screenshots_decode_close_to_the_render() {
    let mut r = Renderer::new();
    let (img, _, _) = r.render(&snap(TEXT_PAGE), Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    let psnr = |quality: u8| -> (f64, usize) {
        let bytes = encode_jpeg(&img, quality).unwrap();
        assert_eq!(&bytes[..2], &[0xff, 0xd8], "JPEG magic");
        let decoded = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg).unwrap().to_rgb8();
        assert_eq!(decoded.dimensions(), (img.width, img.height));
        let src = img.data.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]);
        let mse = decoded.iter().zip(src).map(|(a, b)| (*a as f64 - b as f64).powi(2)).sum::<f64>()
            / (img.width as f64 * img.height as f64 * 3.0);
        (10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10(), bytes.len())
    };
    let (low, low_size) = psnr(30);
    let (mid, mid_size) = psnr(80);
    let (high, high_size) = psnr(95);
    assert!(mid > 32.0, "q80 PSNR {mid:.1} dB");
    assert!(low < mid && mid < high, "PSNR {low:.1} < {mid:.1} < {high:.1}");
    assert!(low_size < mid_size && mid_size < high_size, "sizes {low_size} < {mid_size} < {high_size}");
}

// Review (perf/encoders-and-shaping): the style metrics take each style's first available font
// from the shaper's font selector, and the glyph cache carries run metrics. Pages that stress
// font matching must render the same on a fresh thread (empty glyph cache, every font queried)
// and on a thread warmed by other pages (runs and metrics served from the cache).

/// First families without the text's glyphs, all-missing families, symbol and emoji fonts first,
/// script fallback, generic families, and empty styled spans that only feed the strut.
const FALLBACK_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><style>
  body { margin: 4px; font: 14px Helvetica, Arial, sans-serif; width: 390px; }
  p { margin: 2px 0; }
</style></head><body>
<p style="font-family:'Hiragino Sans',Helvetica">Latin then 日本語 and 한국어 and 中文.</p>
<p style="font-family:Helvetica,'PingFang SC'" lang="ja">直角 骨 ひらがな then Latin.</p>
<p style="font-family:'No Such Font','Hiragino Sans GB',serif">繁體 简体 「括弧」、。 Latin</p>
<p style="font-family:'Apple Color Emoji',Helvetica">Emoji family first then Latin 😀 ok.</p>
<p style="font-family:NoSuchA,NoSuchB">All missing: Latin 😀 Ж 中.</p>
<p style="font-family:Webdings,Helvetica">Webdings first abc 123</p>
<p style="font-family:Zapfino,Times;font-size:11px">Zapfino first fi fl</p>
<p style="font-family:LastResort,Helvetica">LastResort first</p>
<p>ZWJ 👨‍👩‍👧 flag 🇸🇮 keycap 1️⃣ ❤️ ❤︎ 👍🏽 &#xE000; ᱚᱛ ⵜⵉ 𒀀 𝔸 a&#x0301;</p>
<p><span style="font-family:Times">T</span><span>😀</span><span style="font-family:Courier">😀C</span><span style="font-family:Georgia">🎉</span></p>
<p><span style="font-family:serif">se</span> <span style="font-family:monospace">mo</span> <span style="font-family:cursive">cu</span> <span style="font-family:fantasy">fa</span> <span style="font-family:system-ui">ui</span> <span style="font-family:emoji">em😀</span> <span style="font-family:math">∑</span></p>
<p>a<span style="font-family:Zapfino;font-size:40px"></span>b<span style="font-family:'Apple Color Emoji';font-size:30px"></span>c<span style="font-family:NoSuch;font-size:30px"></span>d</p>
<p style="font-family:'Apple Color Emoji'">x</p>
<p style="font-family:Webdings">x</p>
<p><span style="font-size:30px;font-family:Zapfino;vertical-align:middle">M</span><span style="vertical-align:text-top;font-family:'Hiragino Sans';font-size:20px">漢</span><span style="vertical-align:text-bottom;font-family:Webdings">q</span><sub style="font-family:Chalkduster">s</sub><sup style="font-family:'Apple Color Emoji'">😀</sup></p>
</body></html>"#;

/// The same words at sizes and variation locations that differ only slightly, so glyph cache
/// keys differ only in size bits or variations.
const VARIATION_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><style>
  body { margin: 4px; font: 14px Skia, Helvetica; width: 390px; }
  p { margin: 1px 0; }
</style></head><body>
<p>same words</p>
<p style="font-variation-settings:'wght' 0.5">same words</p>
<p style="font-variation-settings:'wght' 2.5">same words</p>
<p style="font-variation-settings:'wdth' 0.7">same words</p>
<p style="font-weight:900">same words</p>
<p style="font-family:system-ui">same words <b>b</b> <i>i</i></p>
<p style="font-family:system-ui;font-weight:350">same words</p>
<p style="font-family:system-ui;font-variation-settings:'wght' 700,'opsz' 28">same words</p>
<p style="font-family:system-ui;font-size:30px">same words</p>
<p style="font-family:system-ui;font-size:30px;font-optical-sizing:none">same words</p>
<p style="font-family:system-ui;font-size:12px">same words</p>
<p style="font-family:system-ui;font-size:12.0001px">same words</p>
<p style="font-family:system-ui"><span style="font-size:10px">same</span><span style="font-size:20px">same</span><span style="font-size:10px">same</span><span style="font-size:20px;font-variation-settings:'wght' 800">same</span><span style="font-size:20px">same</span></p>
<p><span style="font-variation-settings:'wght' 0.5">Skia</span><span style="font-variation-settings:'wght' 2.5">Skia</span><span>Skia</span><span style="font-variation-settings:'wght' 0.5">Skia</span></p>
</body></html>"#;

/// Synthetic bold and italic of families without such faces.
const SYNTHESIS_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><style>
  body { margin: 4px; font: 16px Chalkduster, Helvetica; width: 390px; }
  p { margin: 1px 0; }
</style></head><body>
<p>plain <b>bold</b> <i>italic</i> <b><i>both</i></b> <span style="font-style:oblique 30deg">obl</span></p>
<p style="font-family:Zapfino;font-size:11px">Zap <b>bold</b> <i>ital</i></p>
<p style="font-family:'Apple Chancery'">Ch <b>bold</b> <i>ital</i> 日本 😀</p>
<p style="font-family:'Hiragino Sans'"><b>漢字 b</b> <i>漢字 i</i> 漢字</p>
<p style="font-family:Helvetica;font-weight:600">600 <span style="font-weight:300">300</span> <span style="font-weight:800">800</span></p>
<p style="font-synthesis:none"><b>no synth b</b> <i>no synth i</i></p>
</body></html>"#;

fn render_full(r: &mut Renderer, html: &str) -> (RgbaImage, u64) {
    let (img, hash, _) = r.render(&snap(html), Area::FullPage, Backend::Cpu, &fetcher()).unwrap();
    (img, hash)
}

#[test]
fn font_matching_pages_render_alike_on_fresh_and_warm_threads() {
    let pages = [FALLBACK_PAGE, VARIATION_PAGE, SYNTHESIS_PAGE, TEXT_PAGE];
    let fresh: Vec<(RgbaImage, u64)> = pages
        .iter()
        .map(|&p| std::thread::spawn(move || render_full(&mut Renderer::new(), p)).join().unwrap())
        .collect();
    // One thread renders every page in reverse order, then in order, then each page again with a
    // second renderer: every run of the later passes comes from the glyph cache.
    let warm = std::thread::spawn(move || {
        let mut r = Renderer::new();
        let mut out = Vec::new();
        for &p in pages.iter().rev() {
            out.push(render_full(&mut r, p));
        }
        out.reverse();
        let again: Vec<_> = pages.iter().map(|&p| render_full(&mut r, p)).collect();
        let other: Vec<_> = pages.iter().map(|&p| render_full(&mut Renderer::new(), p)).collect();
        (out, again, other)
    })
    .join()
    .unwrap();
    for (i, f) in fresh.iter().enumerate() {
        for (pass, w) in [("reverse", &warm.0[i]), ("again", &warm.1[i]), ("other", &warm.2[i])] {
            assert_eq!((f.0.width, f.0.height), (w.0.width, w.0.height), "page {i} {pass}: size (fresh vs warm)");
            assert_eq!(f.1, w.1, "page {i} {pass}: paint hash");
            assert!(compare(&f.0, &w.0, 0, None).equal, "page {i} {pass}: pixels");
        }
        assert_eq!(decode_png(&encode_png(&f.0)).unwrap(), f.0, "page {i}: PNG round trip");
    }
}

/// One paragraph per variant, rendered on a thread that shaped the same words with a
/// neighbouring size, variation or synthesis first.
#[test]
fn neighbouring_sizes_and_variations_do_not_share_cached_metrics() {
    let variant = |style: &str| {
        format!(
            r#"<!doctype html><html><head><meta charset="utf-8"><style>body {{ margin: 0; width: 390px; }}</style></head><body><p style="{style}">Same words Ågh 日本 😀</p></body></html>"#
        )
    };
    let styles = [
        "font: 20px Skia",
        "font: 20px Skia; font-variation-settings: 'wght' 3",
        "font: 20.001px Skia",
        "font: 20px system-ui",
        "font: 20px system-ui; font-optical-sizing: none",
        "font: 20px system-ui; font-variation-settings: 'opsz' 80",
        "font: bold 20px system-ui",
        "font: italic 20px Chalkduster",
        "font: 20px Chalkduster",
        "font: 20px 'Apple Color Emoji', Helvetica",
        "font: 20px Helvetica",
        "font: 40px Helvetica; line-height: normal",
    ];
    let pages: Vec<String> = styles.iter().map(|s| variant(s)).collect();
    let fresh: Vec<(RgbaImage, u64)> = pages
        .iter()
        .cloned()
        .map(|p| std::thread::spawn(move || render_full(&mut Renderer::new(), &p)).join().unwrap())
        .collect();
    let warm = std::thread::spawn(move || {
        let mut r = Renderer::new();
        // Every page after every other page: the glyph cache holds all neighbours.
        let mut out = Vec::new();
        for round in 0..2 {
            for i in 0..pages.len() {
                let j = if round == 0 { pages.len() - 1 - i } else { i };
                out.push((j, render_full(&mut r, &pages[j])));
            }
        }
        out
    })
    .join()
    .unwrap();
    for (j, (img, hash)) in warm {
        assert_eq!(hash, fresh[j].1, "style {:?}: paint hash", styles[j]);
        assert!(compare(&img, &fresh[j].0, 0, None).equal, "style {:?}: pixels", styles[j]);
    }
}

/// A family that matches nothing first, then the same family registered on the renderer: the
/// recorded first available font must be the registered font, not the earlier fallback.
#[test]
fn first_available_font_follows_a_font_registered_between_layouts() {
    let page = r#"<!doctype html><html><head><style>
      body { margin: 0; font: 30px Lato, 'Apple Color Emoji', serif; line-height: normal; }
    </style></head><body>Hamburg 日本 😀<span style="font-size:50px"></span></body></html>"#;
    let expected = std::thread::spawn(move || {
        let mut r = Renderer::new();
        r.register_font(LATO.to_vec());
        render_full(&mut r, page)
    })
    .join()
    .unwrap();
    let unregistered = std::thread::spawn(move || render_full(&mut Renderer::new(), page)).join().unwrap();
    let mut r = Renderer::new();
    let before = render_full(&mut r, page);
    assert_eq!(before.1, unregistered.1);
    r.register_font(LATO.to_vec());
    let after = render_full(&mut r, page);
    assert_ne!(after.1, before.1, "the registered font is used");
    assert_eq!(after.1, expected.1, "same as a renderer that had Lato from the start");
    assert!(compare(&after.0, &expected.0, 0, None).equal);
}

/// JPEG quality keeps libjpeg's meaning at the extremes, and a JPEG of a capture taller than
/// the JPEG limit is an error rather than a panic.
#[test]
fn jpeg_quality_extremes_and_limits() {
    let mut r = Renderer::new();
    let (img, _) = render_full(&mut r, FALLBACK_PAGE);
    let sizes: Vec<usize> = [0u8, 1, 2, 50, 99, 100, 255].iter().map(|&q| encode_jpeg(&img, q).unwrap().len()).collect();
    assert_eq!(sizes[0], sizes[1], "quality 0 is clamped to 1");
    assert_eq!(sizes[5], sizes[6], "quality 255 is clamped to 100");
    assert!(sizes[1] <= sizes[2] && sizes[2] < sizes[3] && sizes[3] < sizes[4] && sizes[4] < sizes[5], "{sizes:?}");
    for q in [1u8, 50, 100] {
        let bytes = encode_jpeg(&img, q).unwrap();
        let decoded = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (img.width, img.height));
    }
    let tall = RgbaImage { width: 1, height: 70_000, data: vec![255; 70_000 * 4] };
    assert!(encode_jpeg(&tall, 80).is_err());
    let one = RgbaImage { width: 1, height: 1, data: vec![10, 20, 30, 0] };
    assert_eq!(decode_png(&encode_png(&one)).unwrap(), one, "alpha kept by PNG");
    assert!(encode_jpeg(&one, 80).is_ok());
}

/// The fallback page after exactly one other page on the same thread (or itself) lays out as on a
/// fresh thread. On 34f0f2ba it did not: `chromium_ascent_adjustment` remembered its answer per
/// font data address, and a font allocated where a freed one had lived got the freed font's
/// Times/Helvetica/Courier extra ascent (or lost its own).
#[test]
fn fallback_page_after_one_other_page() {
    let fresh = std::thread::spawn(|| render_full(&mut Renderer::new(), FALLBACK_PAGE)).join().unwrap();
    let mut bad = Vec::new();
    for (name, other) in [("variation", VARIATION_PAGE), ("synthesis", SYNTHESIS_PAGE), ("text", TEXT_PAGE), ("self", FALLBACK_PAGE)] {
        let (w, same_renderer) = std::thread::spawn(move || {
            let mut r = Renderer::new();
            render_full(&mut r, other);
            let a = render_full(&mut r, FALLBACK_PAGE);
            let b = render_full(&mut Renderer::new(), FALLBACK_PAGE);
            (a, b)
        })
        .join()
        .unwrap();
        for (k, x) in [("same renderer", &w), ("new renderer", &same_renderer)] {
            if (x.0.width, x.0.height, x.1) != (fresh.0.width, fresh.0.height, fresh.1) {
                bad.push(format!("after {name} ({k}): {}x{} vs fresh {}x{}", x.0.width, x.0.height, fresh.0.width, fresh.0.height));
            }
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

