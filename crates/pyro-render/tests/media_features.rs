//! Media Queries 4/5 features answer as headless Chrome does (WPT css/mediaqueries mq-gamut-*,
//! negation-*, mq-negative-range-*, device-aspect-ratio-*, scripting-*), and `@supports
//! font-format()` / `font-tech()` evaluate (css-conditional at-supports-font-*).
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

/// Whether the box of each `(rule prelude)` is drawn green: row `n` gets `@<prelude> { #rN { green } }`.
fn green(preludes: &[&str]) -> Vec<bool> {
    let css: String = preludes
        .iter()
        .enumerate()
        .map(|(n, p)| format!("@{p} {{ #r{n} {{ background: #0f0 }} }}"))
        .collect();
    let rows: String = (0..preludes.len()).map(|n| format!("<i id=r{n}></i>")).collect();
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><html><head><style>body{{margin:0;background:#fff}} \
             i{{display:block;height:20px}} {css}</style></head><body>{rows}</body></html>"
        ),
        base_url: "http://test/".into(),
        width: 100,
        height: 20 * preludes.len() as u32,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let img = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    (0..preludes.len())
        .map(|row| {
            let i = ((row * 20 + 10) * img.width as usize + 10) * 4;
            img.data[i..i + 3] == [0, 255, 0]
        })
        .collect()
}

#[test]
fn media_features_answer_like_headless_chrome() {
    let cases = [
        ("media (color)", true),
        ("media (min-color: 8)", true),
        ("media (monochrome)", false),
        ("media not (monochrome)", true),
        ("media (color-index)", false),
        ("media (grid)", false),
        ("media (grid: 0)", true),
        ("media (color-gamut: srgb)", true),
        ("media (color-gamut: p3)", false),
        ("media (dynamic-range: standard)", true),
        ("media (prefers-reduced-motion: no-preference)", true),
        ("media (prefers-reduced-motion)", false),
        ("media (prefers-contrast)", false),
        ("media (forced-colors: none)", true),
        ("media (inverted-colors: none)", true),
        ("media (scripting)", true),
        ("media (scripting: none)", false),
        ("media (update: fast)", true),
        ("media (overflow-block: scroll)", true),
        ("media (display-mode: browser)", true),
        ("media (min-device-aspect-ratio: 1/100)", true),
        ("media (-webkit-transform-3d)", true),
    ];
    let preludes: Vec<&str> = cases.iter().map(|c| c.0).collect();
    let expected: Vec<bool> = cases.iter().map(|c| c.1).collect();
    assert_eq!(green(&preludes), expected, "{preludes:?}");
}

#[test]
fn supports_font_format_and_tech() {
    let cases = [
        ("supports font-format(opentype)", true),
        ("supports font-format(woff2)", true),
        ("supports font-format(embedded-opentype)", false),
        ("supports font-tech(features-opentype)", true),
        ("supports font-tech(color-COLRv1)", true),
        ("supports font-tech(color-svg)", false),
        ("supports not font-tech(features-opentype, color-COLRv0)", true),
    ];
    let preludes: Vec<&str> = cases.iter().map(|c| c.0).collect();
    let expected: Vec<bool> = cases.iter().map(|c| c.1).collect();
    assert_eq!(green(&preludes), expected, "{preludes:?}");
}
