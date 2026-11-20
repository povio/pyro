//! `calc-size()` in `width`/`height` (blitz-dom `LayoutPassState::calc_size_inputs`) and in
//! `min-width`/`min-height` (Taffy's automatic minimum through `CoreStyle::calc_size_min`), after WPT
//! css-values `calc-size/*`.
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

#[track_caller]
fn check(img: &RgbaImage, points: &[(usize, usize, [u8; 3])]) {
    for &(x, y, want) in points {
        assert_eq!(pixel(img, x, y), want, "pixel at ({x}, {y})");
    }
}

#[test]
fn width_and_height_are_the_calculation_of_their_basis() {
    // The auto height (the content's 40px) plus 30px; the auto width (stretched to 100px) halved.
    let img = render(
        "<div style='width:100px'><div style='width:calc-size(auto, size / 2);height:calc-size(auto, size + 30px);background:green'><div style='height:40px'></div></div></div>",
    );
    check(&img, &[(10, 10, GREEN), (45, 65, GREEN), (55, 10, WHITE), (10, 75, WHITE)]);
}

#[test]
fn nested_basis_keeps_negative_intermediates() {
    let img = render("<div style='background:green;width:calc-size(calc-size(10px, size - 30px), size + 70px);height:calc-size(15px, size * 2)'></div>");
    check(&img, &[(49, 29, GREEN), (50, 29, WHITE), (49, 30, WHITE)]);
}

#[test]
fn a_shrink_to_fit_parent_takes_the_calculated_contributions() {
    // Min-content 20px + 70px, max-content 40px + 70px: in 100px the parent is 100px wide.
    let img = render(
        "<div style='width:100px'><div style='background:green;width:fit-content;height:50px'><div style='width:calc-size(fit-content, size + 70px)'><span style='display:inline-block;width:20px'></span><span style='display:inline-block;width:20px'></span></div></div></div>",
    );
    check(&img, &[(95, 25, GREEN), (105, 25, WHITE)]);
}

#[test]
fn a_flex_item_minimum_is_the_calculation_of_its_automatic_minimum() {
    let img = render(
        "<div style='display:flex;width:0;height:50px'><div style='min-width:calc-size(auto, size + 20px);background:green'><div style='width:80px'></div></div></div>",
    );
    check(&img, &[(95, 25, GREEN), (105, 25, WHITE)]);
}

#[test]
fn a_calc_minimum_against_an_indefinite_height_takes_its_percentages_as_zero() {
    // `calc(25px + 50%)` in a box of auto height is 25px (Blink), not none.
    let img = render(
        "<div style='width:50px'><div style='min-height:calc(25px + 50%);background:green'></div></div>",
    );
    check(&img, &[(25, 20, GREEN), (25, 30, WHITE)]);
}
