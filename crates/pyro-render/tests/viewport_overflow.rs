//! The `<body>` whose `overflow` the viewport takes (CSS Overflow 3 §3.3) is no scroll container:
//! its used `overflow` is `visible`, so it establishes no block formatting context and its
//! children's margins collapse through it, as in WPT's `css/css-grid/layout-algorithm/
//! grid-as-flex-item-should-not-shrink-to-fit-*` reftests (`body { overflow: hidden }`).
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn render(html: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: html.into(),
        base_url: "http://test/".into(),
        width: 200,
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

#[track_caller]
fn check(img: &RgbaImage, points: &[(usize, usize, [u8; 3])]) {
    for &(x, y, want) in points {
        assert_eq!(pixel(img, x, y), want, "pixel at ({x}, {y})");
    }
}

const CHILD: &str = "<div style='margin-top:40px;height:20px;background:green'></div>";

/// The child's 40px top margin collapses with the body's 8px one: the child starts at 40px,
/// not at 48px as below a scroll container's edge.
#[test]
fn a_body_whose_overflow_goes_to_the_viewport_collapses_its_childs_margin() {
    let img = render(&format!("<!doctype html><style>body {{ overflow: hidden }}</style><body>{CHILD}"));
    check(&img, &[(20, 39, WHITE), (20, 41, GREEN), (20, 59, GREEN), (20, 61, WHITE)]);
}

/// With the root's `overflow` other than `visible`, the root's goes to the viewport and the
/// body keeps its own: a scroll container, whose child's margin stays inside it.
#[test]
fn a_body_keeps_its_overflow_when_the_root_has_one() {
    let img = render(&format!(
        "<!doctype html><style>html {{ overflow: hidden }} body {{ overflow: hidden }}</style><body>{CHILD}"
    ));
    check(&img, &[(20, 47, WHITE), (20, 49, GREEN), (20, 67, GREEN), (20, 69, WHITE)]);
}
