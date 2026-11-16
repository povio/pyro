//! `font-palette` and `@font-palette-values` (CSS Fonts 4 §9): a colour font's glyphs are drawn
//! in the palette their element selects: `light`, `dark`, a rule's `base-palette` with its
//! `override-colors`, or a `palette-mix()` of palettes. Pyro drew every COLR glyph in palette 0
//! (WPT css/css-fonts/font-palette-*, palette-values-rule-*).
use std::sync::Arc;

use pyro_render::*;

/// The WPT COLR test font: Ahem's boxes in colour, 16 palettes of 8 entries; palette 2 is the
/// light one, palette 3 the dark one.
const COLR: &[u8] = include_bytes!("fonts/COLR-palettes-test-font.ttf");

struct Fonts;
impl Fetcher for Fonts {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        if url.ends_with("/colr.ttf") {
            done(Ok(COLR.to_vec()))
        } else {
            done(Err("offline".into()))
        }
    }
}

/// The pixels of each 60px-tall band of the page, one `<div>` of `AB` per band (without the
/// band's edge rows, which a glyph's antialiased edge in the next band can reach).
fn bands(style: &str, divs: &[&str]) -> Vec<Vec<[u8; 4]>> {
    let body: String = divs.iter().map(|div| format!("<div {div}>AB</div>")).collect();
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><style>\
             @font-face {{ font-family: X; src: url(colr.ttf) }}\
             @font-face {{ font-family: Y; src: url(colr.ttf) }}\
             body {{ margin: 0; background: #fff }} div {{ height: 60px; font: 48px/1 X }} {style}</style>{body}"
        ),
        base_url: "http://test/".into(),
        width: 200,
        height: 60 * divs.len() as u32,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(Fonts);
    let img = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    let width = img.width as usize;
    (0..divs.len())
        .map(|band| {
            ((band * 60 + 2) * width..(band * 60 + 57) * width)
                .map(|i| [img.data[i * 4], img.data[i * 4 + 1], img.data[i * 4 + 2], img.data[i * 4 + 3]])
                .collect()
        })
        .collect()
}

/// How many pixels of `band` have the colour `rgb`.
fn count(band: &[[u8; 4]], rgb: [u8; 3]) -> usize {
    band.iter().filter(|p| p[..3] == rgb).count()
}

#[test]
fn an_element_selects_its_palette() {
    let b = bands(
        "@font-palette-values --P1 { font-family: X; base-palette: 1 }\
         @font-palette-values --P3 { font-family: X; base-palette: 3 }\
         @font-palette-values --Missing { font-family: X; base-palette: 99 }",
        &[
            "",
            "style='font-palette: --P1'",
            "style='font-palette: dark'",
            "style='font-palette: --P3'",
            "style='font-palette: light'",
            "style='font-palette: --Missing'",
            "style='font-palette: --Undefined'",
        ],
    );
    // Drawn in colour at all.
    assert!(b[0].iter().filter(|p| p[..3] != [255, 255, 255]).count() > 1000);
    assert_ne!(b[1], b[0], "base-palette: 1");
    assert_eq!(b[2], b[3], "dark is the palette the font marks dark (3)");
    assert_ne!(b[2], b[0]);
    assert_ne!(b[4], b[0], "light is the palette the font marks light (2)");
    assert_ne!(b[4], b[2]);
    assert_eq!(b[5], b[0], "a palette the font does not have is the default one");
    assert_eq!(b[6], b[0], "a palette name without a rule is the default palette");
}

#[test]
fn override_colors_and_families() {
    let red: String = (0..8).map(|i| format!("{i} #ff0000")).collect::<Vec<_>>().join(", ");
    let b = bands(
        &format!(
            "@font-palette-values --Red {{ font-family: X; override-colors: {red} }}\
             @font-palette-values --P1 {{ font-family: X; base-palette: 1 }}\
             @font-palette-values --P1Y {{ font-family: Y; base-palette: 1 }}\
             @font-palette-values --Calc {{ font-family: X; base-palette: calc(2 - sign(1em - 1px)) }}"
        ),
        &[
            "",
            "style='font-palette: --Red'",
            "style='font-palette: --P1'",
            "style='font-family: Y; font-palette: --P1'",
            "style='font-family: Y; font-palette: --P1Y'",
            "style='font-palette: --Calc'",
        ],
    );
    // Every entry red: the glyphs are red boxes on white.
    let inked = b[1].iter().filter(|p| p[..3] != [255, 255, 255]).count();
    assert!(inked > 1000 && count(&b[1], [255, 0, 0]) * 10 > inked * 9, "{inked}");
    // A rule applies to the fonts of the families it names: Y loads the same file as X.
    assert_eq!(b[3], b[0], "--P1 names X only");
    assert_eq!(b[4], b[2], "--P1Y names Y");
    // `1em` in a descriptor is the initial font size: calc(2 - 1) = 1.
    assert_eq!(b[5], b[2], "calc() in base-palette");
}

#[test]
fn palette_mix_mixes_entry_by_entry() {
    let b = bands(
        "@font-palette-values --A { font-family: X; override-colors: 0 rgb(200 0 0), 1 rgb(200 0 0), 2 rgb(200 0 0), 3 rgb(200 0 0), 4 rgb(200 0 0), 5 rgb(200 0 0), 6 rgb(200 0 0), 7 rgb(200 0 0) }\
         @font-palette-values --B { font-family: X; override-colors: 0 rgb(0 0 200), 1 rgb(0 0 200), 2 rgb(0 0 200), 3 rgb(0 0 200), 4 rgb(0 0 200), 5 rgb(0 0 200), 6 rgb(0 0 200), 7 rgb(0 0 200) }\
         @font-palette-values --P1 { font-family: X; base-palette: 1 }",
        &[
            "style='font-palette: palette-mix(in srgb, --A, --B)'",
            "style='font-palette: palette-mix(in srgb, --A 25%, --B 75%)'",
            "style='font-palette: palette-mix(in srgb, --P1, --P1)'",
            "style='font-palette: --P1'",
        ],
    );
    let inked = |band: &[[u8; 4]]| band.iter().filter(|p| p[..3] != [255, 255, 255]).count();
    assert!(count(&b[0], [100, 0, 100]) * 10 > inked(&b[0]) * 9, "50% each");
    assert!(count(&b[1], [50, 0, 150]) * 10 > inked(&b[1]) * 9, "25% and 75%");
    assert_eq!(b[2], b[3], "a palette mixed with itself");
}
