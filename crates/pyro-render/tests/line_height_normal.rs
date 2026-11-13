//! `line-height: normal` is a whole number of CSS pixels at any device scale,
//! as in Chromium: the font's ascent, descent and line gap are each rounded to
//! whole CSS pixels (Blink's `FontMetrics`), not to device pixels. 13px Arial
//! (ascent 11.76, descent 2.76, line gap 0.43) is 12 + 3 + 0 = 15px tall; rounded
//! at deviceScaleFactor 3 it was 35 + 8 + 1 = 44 device px, 14.67px.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

/// The device-pixel height of the element marked `data-pyro-render-clip`.
fn height(body: &str, device_scale_factor: f64) -> u32 {
    let s = Snapshot {
        html: format!("<!doctype html><style>body{{margin:0}}</style>{body}"),
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
        device_scale_factor,
        ..Default::default()
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    Renderer::new().render(&s, Area::Element, Backend::Cpu, &fetcher).unwrap().0.height
}

#[test]
fn a_normal_line_is_whole_css_pixels_at_every_scale() {
    for (font, css) in
        [("13px Arial", 15), ("14px Arial", 16), ("15px Arial", 17), ("16px Arial", 18), ("17px Arial", 20)]
    {
        for scale in [1.0, 2.0, 3.0] {
            let one = format!("<div data-pyro-render-clip style='font:{font};line-height:normal'>Hg</div>");
            assert_eq!(height(&one, scale), (css as f64 * scale) as u32, "{font} at scale {scale}");
            // Lines stack in whole CSS pixels too.
            let three =
                format!("<div data-pyro-render-clip style='font:{font};line-height:normal'>Hg<br>Hg<br>Hg</div>");
            assert_eq!(
                height(&three, scale),
                (3.0 * css as f64 * scale) as u32,
                "three lines of {font} at scale {scale}"
            );
        }
    }
}
