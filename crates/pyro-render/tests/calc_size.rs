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
fn nested_percent_basis_uses_the_containing_axis() {
    let img = render("<div style='width:200px;height:80px'><div style='background:green;width:calc-size(calc-size(25%, size + 10px), size * 2);height:calc-size(50%, size - 5px)'></div></div>");
    check(&img, &[(119, 34, GREEN), (120, 34, WHITE), (119, 35, WHITE)]);
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

#[test]
fn calculated_maxima_resolve_against_the_containing_axis() {
    let img = render("<div style='width:200px;height:120px'><div style='width:200px;height:120px;max-inline-size:calc-size(calc-size(25%, size + 10px), size * 2);max-block-size:calc-size(any, 25% + 5px);background:green'></div></div>");
    check(&img, &[(119, 34, GREEN), (120, 34, WHITE), (119, 35, WHITE)]);
}

#[test]
fn intrinsic_maxima_calculate_in_the_sizing_box() {
    for (sizing, width, height) in [("content-box", 60, 50), ("border-box", 50, 40)] {
        let img = render(&format!("<div style='width:200px;height:120px'><div style='box-sizing:{sizing};padding:10px;width:180px;height:100px;max-width:calc-size(min-content, size / 2);max-height:calc-size(max-content, size / 2);background:green'><div style='width:80px;height:60px'></div></div></div>"));
        check(&img, &[(width - 1, height - 1, GREEN), (width, 0, WHITE), (0, height, WHITE)]);
    }
}

#[test]
fn intrinsic_maxima_bound_auto_height_but_do_not_enlarge_it() {
    for (calculation, expected) in [("size / 2 + 10%", 50), ("size + 20px", 80)] {
        let img = render(&format!("<div style='width:200px;height:100px'><div style='max-height:calc-size(fit-content, {calculation});background:green'><div style='height:80px'></div></div></div>"));
        check(&img, &[(10, expected - 1, GREEN), (10, expected, WHITE)]);
    }
}

#[test]
fn numeric_minima_win_over_calculated_maxima() {
    let img = render("<div style='width:200px;height:100px'><div style='margin-left:20px;min-width:110px;max-width:calc-size(stretch, size / 2);min-height:50px;max-height:calc-size(min-content, size / 2);background:green'><div style='height:80px'></div></div></div>");
    check(&img, &[(129, 49, GREEN), (130, 49, WHITE), (129, 50, WHITE)]);
}

#[test]
fn calculated_stretch_excludes_margins_and_distinguishes_size_from_percent() {
    let img = render("<div style='width:200px;height:100px'><div style='margin-left:20px;height:100px;max-width:calc-size(stretch, size / 2 + 10%);max-height:calc-size(stretch, size / 2);background:green'></div></div>");
    check(&img, &[(129, 49, GREEN), (130, 49, WHITE), (129, 50, WHITE)]);
}

#[test]
fn only_percentages_in_the_calculation_become_zero_in_an_indefinite_axis() {
    let img = render("<div><div style='height:80px;max-height:calc-size(any, 30px + 10%);background:green'></div><div style='height:60px;max-height:calc-size(50%, size + 10px);background:green'></div></div>");
    check(&img, &[(10, 89, GREEN), (10, 90, WHITE)]);
}
