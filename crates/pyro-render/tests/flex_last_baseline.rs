//! `align-self: last baseline` in a flex row (Taffy `compute/flexbox.rs`): the items of the group
//! are aligned on their last baselines, from the line's cross end (css-align-3 §9.3); a box without
//! one has it synthesized from its border box's bottom edge. Pixel checks after WPT's
//! `css/css-flexbox/flexbox-align-self-baseline-horiz-001a`.
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
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: usize, y: usize) -> [u8; 3] {
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

const WHITE: [u8; 3] = [255, 255, 255];
const GREEN: [u8; 3] = [0, 128, 0];
const BLUE: [u8; 3] = [0, 0, 255];

#[track_caller]
fn check(img: &RgbaImage, points: &[(usize, usize, [u8; 3])]) {
    for &(x, y, want) in points {
        assert_eq!(pixel(img, x, y), want, "pixel at ({x}, {y})");
    }
}

/// Two boxes without baselines (synthesized at their bottom border edges): the blue one's 20px bottom
/// margin puts its last baseline 20px above its margin box's end, the green one's at its end. Aligned
/// on it, the blue box sits at 20..40 beside the green 0..40, and the line is 60px tall (with `end`,
/// the old fallback, the blue box sat at 0..20 in a 40px line).
#[test]
fn last_baseline_aligns_the_groups_last_baselines() {
    let img = render(
        "<div style='display:flex;align-items:last baseline;width:200px'>\
           <div style='width:50px;height:40px;background:green'></div>\
           <div style='width:50px;height:20px;margin-bottom:20px;background:blue'></div>\
         </div>\
         <div style='width:10px;height:10px;background:green'></div>",
    );
    check(&img, &[(25, 10, GREEN), (25, 35, GREEN), (75, 15, WHITE), (75, 25, BLUE), (75, 39, BLUE), (5, 55, WHITE), (5, 65, GREEN)]);
}
