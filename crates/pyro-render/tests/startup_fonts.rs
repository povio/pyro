//! `add_font_file` (`serve --font`): a font file given at startup is used by its family name, as an
//! installed font is, by HTML text and by SVG text (usvg's font database). WPT's runner installs
//! Ahem so; the reftest runner gives pyro its Ahem.ttf with `--font`. The family here, Inter 24pt
//! (the static Inter of tests/fonts), is not one a system has.
use std::path::Path;
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff'>{body}"),
        base_url: "http://test/".into(),
        width: 300,
        height: 100,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

#[test]
fn a_startup_font_is_found_by_its_family_name() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(add_font_file(&dir.join("Cargo.toml")).is_err(), "not a font");
    assert!(add_font_file(&dir.join("tests/fonts/no-such-font.ttf")).is_err());
    // Before any font use, as `serve` adds them.
    add_font_file(&dir.join("tests/fonts/Inter-subset.ttf")).unwrap();

    let html = |family: &str| render(&format!("<div style=\"font:40px '{family}', serif\">AB 1,234</div>"));
    let fallback = html("No Such Family");
    let inter = html("Inter 24pt");
    assert!(fallback.data.iter().any(|&v| v < 128), "the text has ink");
    assert_ne!(inter.data, fallback.data, "HTML text draws with the added font");

    let svg = |family: &str| {
        render(&format!(
            "<svg width='300' height='100'><text x='0' y='60' font-family=\"'{family}'\" font-size='40'>AB 1,234</text></svg>"
        ))
    };
    let fallback = svg("No Such Family");
    let inter = svg("Inter 24pt");
    assert!(fallback.data.iter().any(|&v| v < 128), "the SVG text has ink");
    assert_ne!(inter.data, fallback.data, "SVG text draws with the added font");
}
