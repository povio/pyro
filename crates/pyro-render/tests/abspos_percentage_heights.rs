//! An absolutely positioned (or fixed) box whose height its content gives has no definite height:
//! its children's percentage heights behave as `auto` (CSS 2.1 § 10.5), as in browsers. They
//! resolved against the box's content height (WPT CSS2 `normal-flow/blocks-025`).
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
        html: format!(
            "<!doctype html><style>body {{ margin: 0; background: #fff }}
             .a {{ width: 80px; height: 80px; background: #0f0 }}
             .b {{ width: 20%; height: 20%; background: #f00 }}</style>{body}"
        ),
        base_url: "http://test/".into(),
        width: 400,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn has_red(img: &RgbaImage) -> bool {
    img.data.chunks_exact(4).any(|px| px[0] > 200 && px[1] < 50 && px[2] < 50)
}

#[test]
fn percentage_heights_in_an_auto_height_abspos_box_are_auto() {
    let abs = render("<div style='position:absolute;left:10px;top:10px'><div class=a></div><div class=b></div></div>");
    assert!(!has_red(&abs), "absolute");
    let fixed = render("<div style='position:fixed;left:10px;top:10px'><div class=a></div><div class=b></div></div>");
    assert!(!has_red(&fixed), "fixed");
}

#[test]
fn percentage_heights_in_a_definite_abspos_box_resolve() {
    // A `height`, or both insets, make the box's height definite.
    let sized = render("<div style='position:absolute;left:10px;top:10px;height:100px'><div class=a></div><div class=b></div></div>");
    assert!(has_red(&sized), "height");
    let insets = render("<div style='position:absolute;left:10px;top:10px;bottom:10px'><div class=a></div><div class=b></div></div>");
    assert!(has_red(&insets), "insets");
}
