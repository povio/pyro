//! Variable fonts in PDFs. A PDF has no variable fonts: every instance a
//! document draws (a weight, an optical size) is embedded as a font of its
//! own, with that instance's outlines and advances. Google Fonts serves
//! browsers one variable file for every weight of a family, and
//! Pyro embedded only its default instance: bold text printed
//! Regular, at bold's spacing.

use std::collections::BTreeMap;
use std::sync::Arc;

use pyro_render::*;
use skrifa::instance::{LocationRef, Size};
use skrifa::MetadataProvider;

use crate::pdf_fidelity::{find, inflated};

/// Inter's variable font, cut to `E2E-DUN-MONITOR-OPEN 1,234.56 (EUR) x-y AB xqx`.
/// Axes: `opsz` 14 to 32 (default 14) and `wght` 100 to 900 (default 400).
const INTER: &[u8] = include_bytes!("fonts/Inter-variable-subset.ttf");

/// One rule for every weight of the file.
const ANY_WEIGHT: &str = "@font-face { font-family: V; src: url(inter.ttf); font-weight: 100 900 }";

struct Fonts;

impl Fetcher for Fonts {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        if url.ends_with("/inter.ttf") {
            done(Ok(INTER.to_vec()))
        } else {
            done(Err("offline".into()))
        }
    }
}

/// `body` in 40px `V`, the family `css` declares, printed on a Letter page.
/// `css` comes after the `font` shorthand, which resets `font-optical-sizing`.
fn print(css: &str, body: &str) -> PdfOutput {
    let html = format!(r#"<!doctype html><style>body {{ margin: 0; font: 40px V }} {css}</style>{body}"#);
    let snap = Snapshot {
        html,
        base_url: "http://test/dir/".into(),
        width: 800,
        height: 600,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(Fonts);
    Renderer::new().print_pdf(&snap, &fetcher, &PdfOptions::new(".pz-page", (816.0, 1056.0))).unwrap()
}

/// A glyph, in font units: its outline's (signed) area and its advance.
#[derive(Clone, Copy, Debug)]
struct Shape {
    area: f64,
    advance: f32,
}

impl Shape {
    /// The same glyph of the same instance. An embedded outline is the
    /// instance's with its points rounded to font units.
    fn is(self, other: Shape) -> bool {
        (self.area - other.area).abs() <= other.area * 0.005 && (self.advance - other.advance).abs() <= 1.0
    }
}

struct Pen(kurbo::BezPath);

impl skrifa::outline::OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to((x as f64, y as f64));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to((x as f64, y as f64));
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.0.quad_to((cx as f64, cy as f64), (x as f64, y as f64));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.curve_to((cx0 as f64, cy0 as f64), (cx1 as f64, cy1 as f64), (x as f64, y as f64));
    }
    fn close(&mut self) {
        self.0.close_path();
    }
}

fn shape(font: &skrifa::FontRef, glyph: u32, at: LocationRef) -> Shape {
    let glyph = skrifa::GlyphId::new(glyph);
    let mut pen = Pen(kurbo::BezPath::new());
    let settings = skrifa::outline::DrawSettings::unhinted(Size::unscaled(), at);
    font.outline_glyphs().get(glyph).expect("an outline").draw(settings, &mut pen).unwrap();
    let advance = font.glyph_metrics(Size::unscaled(), at).advance_width(glyph).unwrap();
    Shape { area: kurbo::Shape::area(&pen.0).abs(), advance }
}

/// `ch` in the variable font at `axes` (the other axes at their defaults).
fn instance(ch: char, axes: &[(&str, f32)]) -> Shape {
    let font = skrifa::FontRef::new(INTER).unwrap();
    let at = font.axes().location(axes.iter().copied());
    shape(&font, font.charmap().map(ch).unwrap().to_u32(), (&at).into())
}

/// A font a PDF embeds: its program (TrueType) and the glyph it draws each
/// character with.
struct Embedded {
    program: Vec<u8>,
    glyphs: BTreeMap<char, u32>,
}

impl Embedded {
    fn shape(&self, ch: char) -> Shape {
        shape(&skrifa::FontRef::new(&self.program).unwrap(), self.glyphs[&ch], LocationRef::default())
    }
}

/// The content of stream object `id`.
fn stream(pdf: &[u8], id: &str) -> Vec<u8> {
    let obj = find(pdf, format!("\n{id} 0 obj").as_bytes()).expect("the stream object");
    let start = find(&pdf[obj..], b"stream\n").unwrap() + obj + 7;
    let end = find(&pdf[start..], b"\nendstream").unwrap() + start;
    miniz_oxide::inflate::decompress_to_vec_zlib(&pdf[start..end]).unwrap()
}

/// The fonts the PDF embeds. A font's dictionary names its ToUnicode map and
/// its descriptor names its program, both under the font's name with its
/// subset tag. The tag is of the glyphs a font draws, in the order they are
/// first drawn: the instances a test tells apart draw different text.
fn embedded(pdf: &[u8]) -> Vec<Embedded> {
    let text = inflated(pdf);
    // By the font name under `name`: the object `key` refers to in the same dictionary.
    let refs = |name: &str, key: &str| -> BTreeMap<String, String> {
        let mut refs = BTreeMap::new();
        for dict in text.split(name).skip(1) {
            let font: String = dict
                .trim_start()
                .trim_start_matches('/')
                .chars()
                .take_while(|c| !c.is_whitespace() && !"/<>[]".contains(*c))
                .collect();
            let dict = &dict[..dict.find(">>").unwrap_or(dict.len())];
            let Some(at) = dict.find(key) else { continue };
            let id: String = dict[at + key.len()..].trim_start().chars().take_while(char::is_ascii_digit).collect();
            assert!(refs.insert(font.clone(), id).is_none(), "{font} names two fonts");
        }
        refs
    };
    let programs = refs("/FontName", "/FontFile2");
    let maps = refs("/BaseFont", "/ToUnicode");
    assert_eq!(programs.keys().collect::<Vec<_>>(), maps.keys().collect::<Vec<_>>(), "a program for every font");
    programs
        .iter()
        .map(|(font, program)| {
            let cmap = stream(pdf, &maps[font]);
            let cmap = String::from_utf8_lossy(&cmap);
            let mut glyphs = BTreeMap::new();
            for section in cmap.split("beginbfchar").skip(1) {
                let section = section.split("endbfchar").next().unwrap();
                let hex: Vec<&str> = section.split(['<', '>']).map(str::trim).filter(|s| !s.is_empty()).collect();
                for pair in hex.chunks(2) {
                    let units: Vec<u16> = pair[1]
                        .as_bytes()
                        .chunks(4)
                        .map(|u| u16::from_str_radix(std::str::from_utf8(u).unwrap(), 16).unwrap())
                        .collect();
                    let mut chars = char::decode_utf16(units).map(Result::unwrap);
                    if let (Some(ch), None) = (chars.next(), chars.next()) {
                        glyphs.insert(ch, u32::from_str_radix(pair[0], 16).unwrap());
                    }
                }
            }
            Embedded { program: stream(pdf, program), glyphs }
        })
        .collect()
}

/// The embedded font that draws `ch`: the only one.
fn drawing(fonts: &[Embedded], ch: char) -> &Embedded {
    let mut with = fonts.iter().filter(|f| f.glyphs.contains_key(&ch));
    match (with.next(), with.next()) {
        (Some(font), None) => font,
        _ => panic!("one font draws {ch:?}: {:?}", fonts.iter().map(|f| &f.glyphs).collect::<Vec<_>>()),
    }
}

#[test]
fn each_weight_embeds_its_own_instance() {
    // The optical size axis stays at its default: only the weight varies.
    let out = print(
        &format!("{ANY_WEIGHT} body {{ font-optical-sizing: none }}"),
        r#"<p>DUN</p><p style="font-weight: 700">OPEN</p><p style="font-weight: 250">MIN</p>"#,
    );
    let fonts = embedded(&out.bytes);
    assert_eq!(fonts.len(), 3, "an embedded font per weight");
    let [light, regular, bold] = [('M', 250.0), ('D', 400.0), ('O', 700.0)].map(|(only, wght)| {
        let n = drawing(&fonts, only).shape('N');
        let expected = instance('N', &[("wght", wght)]);
        assert!(n.is(expected), "N at weight {wght}: {n:?}, the instance's is {expected:?}");
        n
    });
    assert!(regular.is(instance('N', &[])), "400 is the default instance");
    // Stems thicken with the weight.
    assert!(light.area * 1.2 < regular.area && regular.area * 1.2 < bold.area, "{light:?} {regular:?} {bold:?}");
    assert_eq!(out.stats.variable_runs, 2, "the default instance's run is not one");
}

#[test]
fn a_rule_per_weight_of_one_file_embeds_each_weight() {
    // Google Fonts, asked for the weights 400 and 700: a rule per weight, both of the same file.
    let out = print(
        "@font-face { font-family: V; src: url(inter.ttf); font-weight: 400 }
        @font-face { font-family: V; src: url(inter.ttf); font-weight: 700 }
        body { font-optical-sizing: none }",
        "<p>DUN</p><p><b>OPEN</b></p>",
    );
    let fonts = embedded(&out.bytes);
    assert_eq!(fonts.len(), 2, "an embedded font per weight");
    let (regular, bold) = (drawing(&fonts, 'D').shape('N'), drawing(&fonts, 'O').shape('N'));
    assert!(regular.is(instance('N', &[])), "{regular:?}");
    assert!(bold.is(instance('N', &[("wght", 700.0)])), "{bold:?}");
}

#[test]
fn optical_size_and_variation_settings_pick_the_instance() {
    // `font-optical-sizing: auto`, the initial value: the optical size is the font size,
    // 40px, which the axis limits to 32.
    let out = print(ANY_WEIGHT, "<p>DUN</p>");
    let n = drawing(&embedded(&out.bytes), 'D').shape('N');
    let (display, text) = (instance('N', &[("opsz", 32.0)]), instance('N', &[]));
    assert!(n.is(display), "{n:?}, at optical size 32 {display:?}");
    assert!(!n.is(text), "{n:?}, at the default optical size {text:?}");

    // Two axes, each between its named positions.
    let out = print(ANY_WEIGHT, r#"<p style="font-variation-settings: 'opsz' 24, 'wght' 650">DUN</p>"#);
    let n = drawing(&embedded(&out.bytes), 'D').shape('N');
    let expected = instance('N', &[("opsz", 24.0), ("wght", 650.0)]);
    assert!(n.is(expected), "{n:?}, the instance's is {expected:?}");
    for near in [instance('N', &[("opsz", 24.0), ("wght", 600.0)]), instance('N', &[("opsz", 14.0), ("wght", 650.0)])] {
        assert!(!n.is(near), "{n:?} is not {near:?}");
    }
}
