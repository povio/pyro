//! Gradient colour stops as CSS Images 3 §3.4.3 fixes them up and CSS Color 4 interpolates them
//! (WPT css/css-images/normalization-*, gradient/gradient-longer-hue-*, gradient-eval-*,
//! tiled-radial-gradients), and the images `image-set()` and `image(<color>)` make
//! (css/css-images/image-set/*-rendering, image-color-*).
use std::sync::Arc;

use pyro_render::*;

struct Offline;
impl Fetcher for Offline {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn render(body: &str, width: u32, height: u32) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(Offline);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff'>{body}"),
        base_url: "http://test/".into(),
        width,
        height,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: usize, y: usize) -> [u8; 3] {
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

fn near(a: [u8; 3], b: [u8; 3], tolerance: i32) -> bool {
    (0..3).all(|i| (a[i] as i32 - b[i] as i32).abs() <= tolerance)
}

fn boxed(background: &str) -> String {
    format!("<div style='width:100px;height:40px;background:{background}'></div>")
}

const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 128, 0];
const BLUE: [u8; 3] = [0, 0, 255];

#[test]
fn stops_before_the_box_leave_the_last_colour() {
    // Both stops are before the gradient line's start: the box is past the last one.
    let img = render(&boxed("linear-gradient(to right, green -50%, blue -50%)"), 100, 40);
    assert_eq!(pixel(&img, 5, 20), BLUE);
    assert_eq!(pixel(&img, 95, 20), BLUE);
}

#[test]
fn a_repeating_gradient_without_a_period_is_the_last_colour() {
    let img = render(&boxed("repeating-linear-gradient(to right, orange 50%, blue 50%)"), 100, 40);
    assert_eq!(pixel(&img, 5, 20), BLUE);
    assert_eq!(pixel(&img, 95, 20), BLUE);
}

#[test]
fn a_stop_before_its_predecessor_moves_up_to_it() {
    // `blue 20%` after `red 60%` is at 60%: red up to there, blue after.
    let img = render(&boxed("linear-gradient(to right, red 60%, blue 20%)"), 100, 40);
    assert_eq!(pixel(&img, 50, 20), RED);
    assert_eq!(pixel(&img, 70, 20), BLUE);
}

#[test]
fn the_colour_after_the_last_stop_is_not_interpolated() {
    // `longer hue` between the last stop and the box's end went once round the hue circle.
    let img = render(&boxed("linear-gradient(to right in hsl longer hue, red 0px, blue 50px)"), 100, 40);
    assert_eq!(pixel(&img, 60, 20), BLUE);
    assert_eq!(pixel(&img, 95, 20), BLUE);
    // Half way from red (0deg) the long way round to blue (240deg, the short way is down through
    // 300deg) is 120deg: green.
    assert!(near(pixel(&img, 25, 20), [0, 255, 0], 12), "{:?}", pixel(&img, 25, 20));
}

#[test]
fn a_narrow_part_of_a_wide_gradient_is_one_colour() {
    // WPT gradient-eval-001: the box is the middle 100px of 131,070,000px from black to lime.
    let img = render(
        &boxed("linear-gradient(to right, rgb(0% 0% 0%) calc(131070000px * -0.5), rgb(0% 100% 0%) calc(131070000px * 0.5))"),
        100,
        40,
    );
    for x in [2, 50, 97] {
        assert!(near(pixel(&img, x, 20), [0, 128, 0], 2), "{:?}", pixel(&img, x, 20));
    }
}

#[test]
fn transparent_stops_interpolate_premultiplied() {
    // Half way from red to transparent (black at no opacity) is half-opaque red, not dark red.
    let img = render(&boxed("linear-gradient(to right, red, transparent)"), 100, 40);
    let [r, g, b] = pixel(&img, 50, 20);
    assert!(r == 255 && (g as i32 - 128).abs() <= 4 && g == b, "{:?}", [r, g, b]);
}

#[test]
fn a_radial_gradient_ending_in_a_hard_stop_is_transparent_past_it() {
    // WPT tiled-radial-gradients: red up to 40% of the radius, nothing after.
    let img = render(
        "<div style='width:100px;height:100px;background:radial-gradient(closest-side, red 40%, transparent 40%)'></div>",
        100,
        100,
    );
    assert_eq!(pixel(&img, 50, 50), RED);
    assert_eq!(pixel(&img, 50, 62), RED);
    assert_eq!(pixel(&img, 50, 80), [255, 255, 255]);
    assert_eq!(pixel(&img, 5, 5), [255, 255, 255]);
}

#[test]
fn a_colour_with_a_missing_component_makes_the_gradient_oklab() {
    // WPT gradient-eval-004: `rgb(0% 0% none)` is modern syntax, so the default interpolation is
    // Oklab, where the missing blue is 0: the middle of blue and black, not blue.
    let img = render(
        &boxed("linear-gradient(to right, rgb(0% 0% 100%) calc(131070000px * -0.5), rgb(0% 0% none) calc(131070000px * 0.5))"),
        100,
        40,
    );
    let [r, g, b] = pixel(&img, 50, 20);
    assert!(r < 8 && g < 8 && (60..=110).contains(&b), "{:?}", [r, g, b]);
}

#[test]
fn image_set_paints_its_selected_option() {
    let img = render(
        &(boxed("image-set(linear-gradient(green, green) 1x, linear-gradient(red, red) 2x)")
            + &boxed("image-set(linear-gradient(red, red) 2x, linear-gradient(green, green) 1x)")),
        100,
        80,
    );
    assert_eq!(pixel(&img, 50, 20), GREEN);
    assert_eq!(pixel(&img, 50, 60), GREEN);
}

#[test]
fn image_of_a_colour_is_sized_and_tiled_like_an_image() {
    let img = render(
        "<div style='width:100px;height:40px;background:image(green) no-repeat 20px 10px / 30px 20px, red'></div>\
         <div style='width:100px;height:40px;color:green;background-image:image(currentcolor)'></div>",
        100,
        80,
    );
    assert_eq!(pixel(&img, 30, 20), GREEN);
    assert_eq!(pixel(&img, 10, 20), RED);
    assert_eq!(pixel(&img, 60, 20), RED);
    assert_eq!(pixel(&img, 50, 60), GREEN);
}

#[test]
fn a_corner_gradient_s_middle_line_joins_the_other_corners() {
    // `to bottom right` in a 200x50 box: the 50% line runs from the bottom left corner to the
    // top right one, so both are the middle colour (the diagonal as the line put purple there
    // only at the centre).
    let img = render(
        "<div style='width:200px;height:50px;background:linear-gradient(to bottom right, red 50%, blue 50%)'></div>",
        200,
        50,
    );
    assert_eq!(pixel(&img, 20, 20), RED);
    assert_eq!(pixel(&img, 180, 30), BLUE);
    // Just inside each side of the line through (0, 50) and (200, 0).
    assert_eq!(pixel(&img, 100, 15), RED);
    assert_eq!(pixel(&img, 100, 35), BLUE);
}

/// pyro: in sRGB a missing component takes the neighbouring stop's value on each side (a stop
/// with one splits in two), and the gradient is the renderer's own interpolation between the
/// resolved ends: pixel for pixel the hard-stop gradient it equals (WPT
/// css/css-images/gradient/gradient-none-interpolation).
#[test]
fn missing_components_in_srgb_resolve_at_the_segment_ends() {
    let img = render(
        &(boxed("linear-gradient(to right in srgb, color(srgb 0.5 0 0), color(srgb none 0.5 0.5), color(srgb 1 1 1))")
            + &boxed("linear-gradient(to right in srgb, color(srgb 0.5 0 0), color(srgb 0.5 0.5 0.5) 50%, color(srgb 1 0.5 0.5) 50%, color(srgb 1 1 1))")
            + &boxed("linear-gradient(to right in srgb, color(srgb none 0 0), color(srgb none 1 1))")
            + &boxed("linear-gradient(to right in srgb, color(srgb 0 0 0), color(srgb 0 1 1))")),
        100,
        160,
    );
    for x in 0..100 {
        assert_eq!(pixel(&img, x, 20), pixel(&img, x, 60), "x = {x}");
        assert_eq!(pixel(&img, x, 100), pixel(&img, x, 140), "x = {x}");
    }
}
