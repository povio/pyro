//! `:lang()` matches an element's language (HTML, "The lang and xml:lang attributes"): the nearest
//! `lang`, else the document's default from `<meta http-equiv="content-language">`, by extended
//! filtering and ASCII case-insensitively (WPT css/CSS2/selectors/lang-*, syntax/case-sensitive-004).
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

/// Whether each of `boxes` (inner HTML of 20px rows) is drawn green under `css`, with `head`.
fn green(head: &str, css: &str, boxes: &[&str]) -> Vec<bool> {
    let rows: String = boxes.iter().map(|b| format!("<div style='height:20px'>{b}</div>")).collect();
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><html><head>{head}<style>body{{margin:0;background:#fff}} \
             i{{display:block;height:20px}} {css}</style></head><body>{rows}</body></html>"
        ),
        base_url: "http://test/".into(),
        width: 100,
        height: 20 * boxes.len() as u32,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let img = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    (0..boxes.len())
        .map(|row| {
            let i = ((row * 20 + 10) * img.width as usize + 10) * 4;
            img.data[i..i + 3] == [0, 255, 0]
        })
        .collect()
}

#[test]
fn lang_matches_the_nearest_lang_by_extended_filtering() {
    let css = "i:lang(es), i:lang(en-GB), i:lang(FR) { background: #0f0 }";
    let boxes = [
        "<i lang=es></i>",
        "<i lang=es-MX></i>",
        "<i lang=ES></i>",
        "<i lang=en-GB-scouse></i>",
        "<b lang=fr><i></i></b>",
        "<i lang=MX-es></i>",
        "<i lang=en-US></i>",
        "<i lang=en></i>",
        "<b lang=es><i lang=de></i></b>",
        "<i></i>",
    ];
    let expected = [true, true, true, true, true, false, false, false, false, false];
    assert_eq!(green("", css, &boxes), expected);
}

#[test]
fn lang_is_ascii_case_insensitive() {
    // U+0130 and U+0131 (dotted and dotless i), U+212A (Kelvin sign) fold to ASCII only in Unicode.
    let css = "i:lang(f\u{130}), i:lang(F\u{131}), i:lang(\u{212a}l) { background: #0f0 }";
    assert_eq!(green("", css, &["<i lang=fi></i>", "<i lang=kl></i>"]), [false, false]);
}

#[test]
fn the_content_language_pragma_is_the_default_language() {
    let css = "i:lang(fr) { background: #0f0 }";
    let pragma = "<meta http-equiv=Content-Language content=' fr'>";
    assert_eq!(green(pragma, css, &["<i></i>", "<i lang=en></i>"]), [true, false]);
    // A list of languages sets none.
    let list = "<meta http-equiv=content-language content='fr, de'>";
    assert_eq!(green(list, css, &["<i></i>"]), [false]);
}
