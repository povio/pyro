//! Gaps in flex containers (Taffy `compute/flexbox.rs`, `size_container_main_axis`): a column's
//! percentage gaps resolve against its height only when that is definite, as in Blink. Pixel checks
//! after WPT's `css/css-flexbox/gap-percentage-wrap`.
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


/// A column's 10% row gap resolves against its indefinite height, to zero: the two 50px items touch,
/// and the column is 100px tall (the gap resolved against that height pushed the second item 10px
/// down, past it).
#[test]
fn a_columns_percentage_gap_is_zero_without_a_definite_height() {
    let img = render(
        "<div style='display:flex;flex-direction:column;gap:10%;width:100px'>\
           <div style='height:50px;background:green'></div>\
           <div style='height:50px;background:blue'></div>\
         </div>",
    );
    check(&img, &[(10, 49, GREEN), (10, 50, BLUE), (10, 99, BLUE), (10, 101, WHITE)]);
}
