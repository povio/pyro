//! `content-visibility: hidden` skips an element's contents (neither laid out nor painted) and
//! contains its size (CSS Contain 2 § 4.1); on an element containment does not apply to (a
//! non-atomic inline box) it does nothing. WPT css-contain `content-visibility/*`.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

/// The number of pure green pixels and the number of pure blue ones.
fn counts(body: &str) -> (usize, usize) {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><style>body{{margin:0}} .g{{width:40px;height:40px;background:#0f0}}</style>{body}"),
        base_url: "http://test/".into(),
        width: 200,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let data = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0.data;
    let (mut green, mut blue) = (0, 0);
    for p in data.chunks(4) {
        match (p[0], p[1], p[2]) {
            (0, 255, 0) => green += 1,
            (0, 0, 255) => blue += 1,
            _ => {}
        }
    }
    (green, blue)
}

#[test]
fn hidden_skips_the_contents_and_contains_the_size() {
    // The block's own background paints; its child does not, and does not size it (height 0).
    assert_eq!(counts("<div style='content-visibility:hidden;width:50px;background:#00f'><div class=g></div></div>"), (0, 0));
    assert_eq!(
        counts("<div style='content-visibility:hidden;width:50px;height:50px;background:#00f'><div class=g></div></div>"),
        (0, 2500)
    );
    assert_eq!(counts("<div style='width:50px'><div class=g></div></div>"), (1600, 0));
}

#[test]
fn hidden_does_nothing_on_an_inline_box() {
    assert_eq!(counts("<span style='content-visibility:hidden'><div class=g></div></span>"), (1600, 0));
}
