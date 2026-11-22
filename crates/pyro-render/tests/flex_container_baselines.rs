//! A flex container's first and last baselines (Taffy `compute/flexbox.rs` step 8.5, css-flexbox-1 §8.5; as Blink's
//! `BaselineAccumulator`): the last baseline comes from the endmost line's main-end item (or its baseline-sharing
//! group), and "main-start" is visual, so a `row-reverse` container's first baseline is its last item's. Pixel checks
//! after WPT css-flexbox `alignment/flex-align-baseline-flex-001`.
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

/// An inner flex row (100px, 10px bottom padding) whose items sit at its top: its last baseline is its main-end
/// (30px) item's, synthesized at that item's bottom, 30px down, 10px above its own bottom edge. Aligned on last
/// baselines, the blue box beside it sits at 10..30 (before, the container had no last baseline and one was
/// synthesized at its bottom edge, which put the blue box at 20..40).
#[test]
fn flex_container_exports_its_last_baseline() {
    let img = render(
        "<div style='display:flex;align-items:last baseline;width:200px'>\
           <div style='width:50px;height:20px;background:blue'></div>\
           <div style='display:flex;align-items:flex-start;width:100px;padding-bottom:10px;background:green'>\
             <div style='width:20px;height:10px'></div><div style='width:20px;height:30px'></div>\
           </div>\
         </div>",
    );
    check(&img, &[(25, 5, WHITE), (25, 15, BLUE), (25, 29, BLUE), (25, 35, WHITE), (100, 5, GREEN), (100, 35, GREEN)]);
}

/// A `row-reverse` flex row's first baseline is its visually first (leftmost) item's: the 30px box, last in order,
/// so its baseline is 30px down and the blue box beside it sits at 10..30 (before, it came from the first item in
/// order, the 10px box: the inner row was pushed down to 10..40 and the blue box sat at 0..20).
#[test]
fn row_reverse_first_baseline_comes_from_the_main_start_item() {
    let img = render(
        "<div style='display:flex;align-items:baseline;width:200px'>\
           <div style='width:50px;height:20px;background:blue'></div>\
           <div style='display:flex;flex-direction:row-reverse;align-items:flex-start;width:100px;background:green'>\
             <div style='width:20px;height:10px'></div><div style='width:20px;height:30px'></div>\
           </div>\
         </div>",
    );
    check(&img, &[(25, 5, WHITE), (25, 15, BLUE), (25, 29, BLUE), (100, 5, GREEN), (100, 35, WHITE)]);
}
