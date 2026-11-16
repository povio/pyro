//! `font-size-adjust` (CSS Fonts 5 §3.6): the used font size is scaled so that the named metric
//! of the font that draws the text has the given ratio to the font size (WPT
//! css/css-fonts/font-size-adjust-001, -002, -005, -009, -010, -zero-1/-2, -ic-height).
use std::sync::Arc;

use pyro_render::*;

struct Offline;
impl Fetcher for Offline {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

/// The dark pixels of each 100px-tall band of the page.
fn ink(body: &str, bands: usize) -> Vec<usize> {
    let fetcher: Arc<dyn Fetcher> = Arc::new(Offline);
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><style>body{{margin:0;background:#fff}} div{{height:100px;font:20px/1 serif;overflow:hidden}}</style>{body}"
        ),
        base_url: "http://test/".into(),
        width: 600,
        height: 100 * bands as u32,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let img = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    (0..bands)
        .map(|band| {
            (band * 100..band * 100 + 100)
                .flat_map(|y| (0..img.width as usize).map(move |x| (x, y)))
                .filter(|&(x, y)| img.data[(y * img.width as usize + x) * 4] < 128)
                .count()
        })
        .collect()
}

#[test]
fn the_used_font_size_follows_the_ex_height() {
    // An x-height of the whole font size makes the text about twice as large (a serif's
    // x-height is under half its size); the font's own ratio leaves it as it is.
    let counts = ink(
        "<div>xxxxxxxxxx</div><div style='font-size-adjust: 1'>xxxxxxxxxx</div>\
         <div style='font-size-adjust: from-font'>xxxxxxxxxx</div>",
        3,
    );
    assert!(counts[0] > 0, "{counts:?}");
    assert!(counts[1] > counts[0] * 3, "{counts:?}");
    assert_eq!(counts[2], counts[0], "{counts:?}");
}

#[test]
fn an_adjustment_of_zero_draws_nothing_and_children_inherit() {
    let counts = ink(
        "<div style='font-size-adjust: 0'>xxxxxxxxxx</div>\
         <div style='font-size-adjust: 1'><span>xxxxxxxxxx</span></div><div>xxxxxxxxxx</div>",
        3,
    );
    assert_eq!(counts[0], 0, "{counts:?}");
    assert!(counts[1] > counts[2] * 3, "{counts:?}");
}
