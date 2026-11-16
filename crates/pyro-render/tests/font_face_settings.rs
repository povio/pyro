//! `@font-face` `font-feature-settings` and `font-variation-settings` descriptors (CSS Fonts 4
//! §4.7, §7.2): a face's own features and variation settings, under those of the properties and
//! over the automatic `opsz` of `font-optical-sizing`, which follows the face's `size-adjust`.
//! Pyro ignored both descriptors (WPT css/css-fonts/font-feature-settings-descriptor-*,
//! font-variation-settings-descriptor-*, variations/variable-opsz-size-adjust).
use std::sync::Arc;

use pyro_render::*;

/// Inter, cut to `E2E-DUN-MONITOR-OPEN 1,234.56 (EUR) x-y AB xqx`, with `tnum`.
const INTER: &[u8] = include_bytes!("fonts/Inter-subset.ttf");
/// Inter's variable font cut to the same characters: `opsz` 14 to 32, `wght` 100 to 900.
const INTER_VARIABLE: &[u8] = include_bytes!("fonts/Inter-variable-subset.ttf");

struct Fonts;
impl Fetcher for Fonts {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        match url.rsplit('/').next() {
            Some("inter.ttf") => done(Ok(INTER.to_vec())),
            Some("inter-var.ttf") => done(Ok(INTER_VARIABLE.to_vec())),
            _ => done(Err("offline".into())),
        }
    }
}

/// The pixels of each 60px band, one `<div>` of `text` per entry of `divs`.
fn bands(faces: &str, text: &str, divs: &[&str]) -> Vec<Vec<[u8; 3]>> {
    let body: String = divs.iter().map(|div| format!("<div style=\"{div}\">{text}</div>")).collect();
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><style>{faces} body {{ margin: 0; background: #fff }} div {{ height: 60px; font: 28px/60px serif; white-space: nowrap }}</style>{body}"
        ),
        base_url: "http://test/".into(),
        width: 500,
        height: 60 * divs.len() as u32,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(Fonts);
    let img = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    let width = img.width as usize;
    (0..divs.len())
        .map(|band| {
            ((band * 60 + 2) * width..(band * 60 + 58) * width)
                .map(|i| [img.data[i * 4], img.data[i * 4 + 1], img.data[i * 4 + 2]])
                .collect()
        })
        .collect()
}

fn ink(band: &[[u8; 3]]) -> usize {
    band.iter().filter(|p| p[0] < 128).count()
}

#[test]
fn face_features_apply_under_the_property() {
    let b = bands(
        "@font-face { font-family: Plain; src: url(inter.ttf) }\
         @font-face { font-family: Tabular; src: url(inter.ttf); font-feature-settings: 'tnum' }",
        "1,111.11 2,222.22",
        &[
            "font-family: Plain",
            "font-family: Tabular",
            "font-family: Plain; font-feature-settings: 'tnum'",
            "font-family: Tabular; font-feature-settings: 'tnum' 0",
        ],
    );
    assert_ne!(b[1], b[0], "the descriptor turns tabular figures on");
    assert_eq!(b[1], b[2], "as the property does");
    assert_eq!(b[3], b[0], "the property overrides the descriptor");
}

#[test]
fn face_variations_apply_under_the_property_and_over_the_automatic_opsz() {
    let b = bands(
        "@font-face { font-family: Var; src: url(inter-var.ttf) }\
         @font-face { font-family: Heavy; src: url(inter-var.ttf); font-variation-settings: 'wght' 900 }\
         @font-face { font-family: Small; src: url(inter-var.ttf); font-variation-settings: 'opsz' 14 }\
         @font-face { font-family: Double; src: url(inter-var.ttf); size-adjust: 200% }",
        "MONITOR 1,234",
        &[
            "font-family: Var",
            "font-family: Heavy",
            "font-family: Var; font-variation-settings: 'wght' 900",
            "font-family: Heavy; font-variation-settings: 'wght' 400",
            "font-family: Small",
            "font-family: Var; font-optical-sizing: none",
            "font-family: Double; font-size: 14px",
        ],
    );
    assert!(ink(&b[1]) > ink(&b[0]) * 5 / 4, "{} {}", ink(&b[1]), ink(&b[0]));
    assert_eq!(b[1], b[2], "the descriptor's weight is the property's");
    assert_eq!(b[3], b[0], "the property overrides the descriptor");
    // At 28px the automatic `opsz` is 28; the descriptor's 14 wins over it, as the font's
    // default (14) does without optical sizing.
    assert_ne!(b[0], b[5], "optical sizing changes the glyphs at 28px");
    assert_eq!(b[4], b[5], "the descriptor's opsz");
    // 14px at `size-adjust: 200%` is drawn at 28px, at the `opsz` of 28px.
    assert_eq!(b[6], b[0], "the automatic opsz follows size-adjust");
}

/// `@font-face` `font-weight` ranges (CSS Fonts 4 §5.2): a face is matched at its weight nearest
/// the one asked for, a reversed range is the same range, and a variable font's `wght` axis is
/// held to the range (a bold request on a `font-weight: 400` face is faked bold, WPT
/// variations/font-weight-3, font-descriptor-range-reversed).
#[test]
fn weight_ranges_hold_a_variable_font() {
    let b = bands(
        "@font-face { font-family: Var; src: url(inter-var.ttf) }\
         @font-face { font-family: Regular; src: url(inter-var.ttf); font-weight: 400 }\
         @font-face { font-family: Heavy; src: url(inter-var.ttf); font-weight: 900 600 }",
        "MONITOR",
        &[
            "font-family: Var; font-weight: 700",
            "font-family: Regular; font-weight: 700",
            "font-family: Var; font-weight: 600",
            "font-family: Heavy; font-weight: 300",
            "font-family: Regular; font-weight: 400",
            "font-family: Var; font-weight: 400",
        ],
    );
    assert_ne!(b[1], b[0], "a face of weight 400 is not drawn at wght 700");
    assert!(ink(&b[1]) > ink(&b[4]), "but faked bold");
    assert_eq!(b[3], b[2], "a request below a reversed range takes its nearest weight");
    assert_eq!(b[4], b[5], "a request within the range is unchanged");
}

/// macOS's own UI fonts go by names that start with a dot (`.AppleSystemUIFont`, `.SF NS Mono`):
/// pages cannot reach them, as in Safari and Chrome (WPT css/css-fonts/standard-font-family).
#[test]
fn hidden_system_font_names_fall_back() {
    let b = bands(
        "",
        "MONITOR 1,234",
        &["font-family: serif", "font-family: '.AppleSystemUIFont'", "font-family: '.SF NS Mono', serif"],
    );
    // Drawn in the serif fallback (the same ink; the system UI font has other glyphs).
    assert!(ink(&b[0]) > 500);
    assert_eq!([ink(&b[1]), ink(&b[2])], [ink(&b[0]); 2]);
}

/// Hiragana and Katakana shape in one run (both are OpenType's `kana`): the font's kerning between
/// `す` (Hiragana) and `ペ` (Katakana) applies (WPT css/css-fonts/hiragana-katakana-kerning).
#[test]
fn kerning_reaches_across_hiragana_and_katakana() {
    const KANA: &[u8] = include_bytes!("fonts/NotoSansJP-kana_test-subset.otf");
    struct Kana;
    impl Fetcher for Kana {
        fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
            if url.ends_with("/kana.otf") { done(Ok(KANA.to_vec())) } else { done(Err("offline".into())) }
        }
    }
    let snapshot = Snapshot {
        html: "<!doctype html><style>@font-face { font-family: Kana; src: url(kana.otf) }\
               body { margin: 0; background: #fff } p { margin: 0; height: 100px; font: 80px/100px Kana; font-feature-settings: 'palt' }</style>\
               <p style='font-kerning: normal'>すペ</p><p style='font-kerning: none'>すペ</p>"
            .into(),
        base_url: "http://test/".into(),
        width: 300,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(Kana);
    let img = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    let width = img.width as usize;
    let band = |b: usize| -> Vec<u8> { img.data[(b * 100 + 5) * width * 4..(b * 100 + 95) * width * 4].to_vec() };
    assert!(band(0).chunks(4).any(|p| p[0] < 128), "drawn");
    assert_ne!(band(0), band(1), "kerned");
}
