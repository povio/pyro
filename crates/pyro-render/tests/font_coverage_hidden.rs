//! A character the font lacks but HarfBuzz shapes in it all the same keeps the font, as in Blink:
//! U+2011 NON-BREAKING HYPHEN is drawn as the font's U+2010 HYPHEN, and a default-ignorable code
//! point (U+034F COMBINING GRAPHEME JOINER, U+2060 WORD JOINER) is hidden. Ahem has U+2010 but
//! neither of the others; at 20px every Ahem glyph is a 20px black square (WPT css-text
//! `line-break-anywhere-overrides-uax-behavior-011/013/014`).
use std::sync::Arc;

use pyro_render::*;

/// Serves Ahem (`tests/fonts`) as `ahem.ttf`.
struct AhemFetch;
impl Fetcher for AhemFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(if url.ends_with("/ahem.ttf") {
            Ok(include_bytes!("fonts/Ahem.ttf").to_vec())
        } else {
            Err("offline".into())
        });
    }
}

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(AhemFetch);
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><style>@font-face {{ font-family: A; src: url(ahem.ttf) }}
             html, body {{ margin: 0; background: #fff }} div {{ font: 20px/20px A; color: #000 }}</style>{body}"
        ),
        base_url: "http://test/".into(),
        width: 300,
        height: 100,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.text_antialiasing = false;
    renderer.render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// The black pixels of row 10.
fn black_in_row(img: &RgbaImage) -> usize {
    (0..img.width as usize).filter(|x| img.data[(10 * img.width as usize + x) * 4..][..3] == [0, 0, 0]).count()
}

#[test]
fn a_non_breaking_hyphen_is_drawn_as_the_fonts_hyphen() {
    assert_eq!(black_in_row(&render("<div>X&#x2011;X</div>")), 60);
}

#[test]
fn default_ignorable_code_points_are_hidden_in_the_font() {
    assert_eq!(black_in_row(&render("<div>X&#x034F;X&#x2060;X</div>")), 60);
}
