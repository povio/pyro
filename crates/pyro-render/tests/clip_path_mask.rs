//! `clip-path` and `mask` on boxes, as Chromium paints them (WPT `css/css-masking/clip-path` and
//! `mask-image`): the reference boxes with their rounded corners (the margin box's radii grown by
//! the margins), the basic shapes' fill rules, percentages and keywords, `shape()`, an empty shape
//! clipping everything, and `mask-mode: luminance`, `mask-clip: no-clip` and the SVG boxes of
//! `mask-clip`/`mask-origin`.
use std::sync::Arc;

use pyro_render::*;

/// Serves `red.png` and `blue.png`, 20 by 20 opaque pixels of their colour.
struct Images;
impl Fetcher for Images {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let image = |rgba: [u8; 4]| encode_png(&RgbaImage { width: 20, height: 20, data: rgba.repeat(400) });
        let body = if url.ends_with("red.png") {
            Ok(image([255, 0, 0, 255]))
        } else if url.ends_with("blue.png") {
            Ok(image([0, 0, 255, 255]))
        } else {
            Err("404".into())
        };
        std::thread::spawn(move || done(body));
    }
}

fn render(body: &str, scale: f64) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(Images);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff;width:400px;height:300px'>{body}"),
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
        device_scale_factor: scale,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// The pixel at CSS px (x, y), its centre.
fn pixel(img: &RgbaImage, scale: f64, x: f64, y: f64) -> [u8; 3] {
    let (x, y) = (((x + 0.5) * scale) as usize, ((y + 0.5) * scale) as usize);
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

const WHITE: [u8; 3] = [255, 255, 255];
const GREEN: [u8; 3] = [0, 128, 0];

fn assert_pixel(img: &RgbaImage, scale: f64, (x, y): (f64, f64), want: [u8; 3], what: &str) {
    let got = pixel(img, scale, x, y);
    assert!(
        got.iter().zip(want).all(|(&g, w)| g.abs_diff(w) <= 2),
        "{what} at ({x}, {y}), scale {scale}: {got:?}, want {want:?}"
    );
}

/// `clip-path: border-box` (and the other boxes alone) clips to the box with its rounded corners:
/// a circle for `border-radius: 50%`. The corners were square.
#[test]
fn a_box_alone_clips_to_its_rounded_edges() {
    for scale in [1.0, 2.0] {
        for (geometry, border) in [
            ("border-box", ""),
            ("padding-box", "border:8px solid red;"),
            ("content-box", "border:4px solid red;padding:4px;"),
        ] {
            let img = render(
                &format!(
                    "<div style='width:100px;height:100px;box-sizing:border-box;background:green;border-radius:50px;\
                     {border}clip-path:{geometry};outline:8px solid red'></div>"
                ),
                scale,
            );
            let inset = if border.is_empty() { 0.0 } else { 8.0 };
            assert_pixel(
                &img,
                scale,
                (inset + 4.0, inset + 4.0),
                WHITE,
                &format!("{geometry}: outside the rounded corner"),
            );
            assert_pixel(&img, scale, (50.0, 50.0), GREEN, &format!("{geometry}: the middle"));
        }
    }
}

/// `clip-path: margin-box` is the margin box (it fell back to the border box), its radii grown
/// by the margins as `box-shadow` spread grows them: a 25px margin around a 50px circle is a 100px
/// circle, and a 10px radius with a 50px margin a 34.4px one.
#[test]
fn the_margin_box_clips_with_its_grown_radii() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='float:left;width:50px;height:50px;margin:25px;background:green;border-radius:50px;\
             clip-path:margin-box;outline:200px solid green'></div>",
            scale,
        );
        // The outline shows inside the margin box's circle, outside the border box.
        assert_pixel(&img, scale, (50.0, 2.0), GREEN, "top of the margin box's circle");
        assert_pixel(&img, scale, (2.0, 50.0), GREEN, "left of the margin box's circle");
        assert_pixel(&img, scale, (6.0, 6.0), WHITE, "outside the margin box's circle");
        let img = render(
            "<div style='float:left;width:50px;height:50px;margin:50px;background:green;border-radius:10px;\
             clip-path:margin-box;outline:200px solid green'></div>",
            scale,
        );
        // A corner of radius 34.4: its arc's middle is 34.4 * (1 - 1/sqrt 2) = 10.1px in.
        assert_pixel(&img, scale, (7.0, 7.0), WHITE, "outside the 34.4px corner");
        assert_pixel(&img, scale, (13.0, 13.0), GREEN, "inside the 34.4px corner");
    }
}

/// `polygon()`, `path()` and `shape()` fill with their fill rule: `evenodd` leaves a hole.
#[test]
fn shapes_fill_with_their_fill_rule() {
    let scale = 1.0;
    for clip in [
        "polygon(evenodd, 0 0, 100px 0, 100px 100px, 0 100px, 0 25px, 75px 25px, 75px 75px, 25px 75px, 25px 25px, 0 25px)",
        "path(evenodd, \"M0,0h100v100h-100zM25,25h50v50h-50z\")",
        "shape(evenodd from 0 0, hline by 100px, vline by 100%, hline to 0, close, move to 25px 25px, \
         hline by 50px, vline by 50px, hline by -50px, close)",
    ] {
        let img =
            render(&format!("<div style='width:100px;height:100px;background:green;clip-path:{clip}'></div>"), scale);
        assert_pixel(&img, scale, (50.0, 50.0), WHITE, &format!("the hole of {clip}"));
        assert_pixel(&img, scale, (10.0, 50.0), GREEN, &format!("the ring of {clip}"));
    }
    // `nonzero`: the inner square drawn the same way round fills.
    let img = render(
        "<div style='width:100px;height:100px;background:green;clip-path:path(nonzero, \"M0,0h100v100h-100zM25,25h50v50h-50z\")'></div>",
        scale,
    );
    assert_pixel(&img, scale, (50.0, 50.0), GREEN, "nonzero path");
}

/// A shape with no area clips everything (it clipped nothing): an inside-out `rect()` and insets
/// past the box. `inset()` takes rounded corners; a `circle()` percentage is of the reference box's
/// direction-agnostic size; an `ellipse()`'s sides are measured along each axis.
#[test]
fn basic_shapes_are_resolved_as_css_shapes_says() {
    let scale = 1.0;
    for clip in ["rect(50px 0 0 50px)", "inset(60% 0 50% 0)"] {
        let img =
            render(&format!("<div style='width:100px;height:100px;background:red;clip-path:{clip}'></div>"), scale);
        assert_pixel(&img, scale, (75.0, 75.0), WHITE, clip);
        assert_pixel(&img, scale, (25.0, 25.0), WHITE, clip);
    }
    let img =
        render("<div style='width:100px;height:100px;background:green;clip-path:inset(10px round 30px)'></div>", scale);
    assert_pixel(&img, scale, (14.0, 14.0), WHITE, "outside the rounded inset's corner");
    assert_pixel(&img, scale, (50.0, 14.0), GREEN, "the rounded inset's top edge");
    // 10% of sqrt(1400² + 200²) / sqrt(2) = 100px.
    let img = render(
        "<div style='width:1400px;height:200px;background:green;clip-path:circle(10% at 100px 100px)'></div>",
        scale,
    );
    assert_pixel(&img, scale, (100.0, 4.0), GREEN, "circle top");
    assert_pixel(&img, scale, (195.0, 100.0), GREEN, "circle right");
    assert_pixel(&img, scale, (205.0, 100.0), WHITE, "outside the circle");
    // `ellipse()` is closest-side in each axis: 75 by 50 in a 150 by 100 box.
    let img = render("<div style='width:150px;height:100px;background:green;clip-path:ellipse()'></div>", scale);
    assert_pixel(&img, scale, (3.0, 50.0), GREEN, "ellipse left");
    assert_pixel(&img, scale, (75.0, 3.0), GREEN, "ellipse top");
}

/// `shape()` commands: control points relative to the start, the end or the reference box's
/// origin, and arcs: a single radius is of the direction-agnostic size, `cw` is SVG's sweep flag 1.
#[test]
fn shape_commands_are_placed_in_the_reference_box() {
    let scale = 1.0;
    // A lens from the left middle to the right middle, bulging up and down.
    let img = render(
        "<div style='width:200px;height:200px;background:green;clip-path:shape(from center left, \
         curve to center right with center top / center top, curve to center left with center bottom / center bottom, close)'></div>",
        scale,
    );
    assert_pixel(&img, scale, (100.0, 100.0), GREEN, "the lens's middle");
    assert_pixel(&img, scale, (100.0, 30.0), GREEN, "inside the upper curve");
    assert_pixel(&img, scale, (10.0, 10.0), WHITE, "outside the lens");
    // A half disc of radius 50 above the line from (0, 100) to (100, 100), drawn clockwise.
    let img = render(
        "<div style='width:100px;height:200px;background:green;clip-path:shape(from 0 100px, arc to 100px 100px of 50px cw, close)'></div>",
        scale,
    );
    assert_pixel(&img, scale, (50.0, 60.0), GREEN, "inside the clockwise arc");
    assert_pixel(&img, scale, (50.0, 140.0), WHITE, "below the line");
}

/// `blue` at `alpha` over white.
fn blue_over_white(alpha: f64) -> [u8; 3] {
    let c = (255.0 * (1.0 - alpha)).round() as u8;
    [c, c, 255]
}

/// `mask-mode: luminance` masks with the image's luminance (0.2125 R + 0.7154 G + 0.0721 B); it
/// was masked with its alpha. Raster images and gradients alike.
#[test]
fn luminance_masks_use_the_images_luminance() {
    for scale in [1.0, 2.0] {
        for (image, luminance) in [
            ("url(red.png)", 0.2125),
            ("url(blue.png)", 0.0721),
            ("linear-gradient(lime, lime)", 0.7154),
            ("linear-gradient(red, red)", 0.2125),
        ] {
            let img = render(
                &format!(
                    "<div style='width:100px;height:100px;background:blue;mask-image:{image};mask-mode:luminance'></div>"
                ),
                scale,
            );
            assert_pixel(&img, scale, (50.0, 50.0), blue_over_white(luminance), &format!("luminance of {image}"));
            // The default (`match-source`, an image's alpha) is unchanged.
            let img = render(
                &format!("<div style='width:100px;height:100px;background:blue;mask-image:{image}'></div>"),
                scale,
            );
            assert_pixel(&img, scale, (50.0, 50.0), [0, 0, 255], &format!("alpha of {image}"));
        }
    }
}

/// `mask-clip: no-clip` leaves what the element paints outside its border box (a positioned
/// child); the SVG boxes of `mask-clip` and `mask-origin` are, for a box, the content box
/// (`fill-box`) and the border box (`stroke-box`, `view-box`). Stylo parsed them for Gecko only.
#[test]
fn mask_clip_and_origin_take_no_clip_and_the_svg_boxes() {
    let scale = 1.0;
    let child = "<div style='position:absolute;left:-30px;top:0;width:20px;height:20px;background:green'></div>";
    let img = render(
        &format!(
            "<div style='position:absolute;left:50px;top:50px;width:40px;height:40px;background:green;\
             mask-image:linear-gradient(#000, #000);mask-clip:no-clip'>{child}</div>"
        ),
        scale,
    );
    assert_pixel(&img, scale, (30.0, 60.0), GREEN, "no-clip: the child outside the box");
    let img = render(
        &format!(
            "<div style='position:absolute;left:50px;top:50px;width:40px;height:40px;background:green;\
             mask-image:linear-gradient(#000, #000)'>{child}</div>"
        ),
        scale,
    );
    assert_pixel(&img, scale, (30.0, 60.0), WHITE, "border-box: the child is cut off");
    // `fill-box` clips to the content box, inside the 10px padding.
    let img = render(
        "<div style='width:40px;height:40px;padding:10px;background:green;\
         mask-image:linear-gradient(#000, #000);mask-clip:fill-box'></div>",
        scale,
    );
    assert_pixel(&img, scale, (5.0, 30.0), WHITE, "fill-box: the padding is clipped");
    assert_pixel(&img, scale, (30.0, 30.0), GREEN, "fill-box: the content");
    // `mask-origin: fill-box` positions a 20px mask image at the content box.
    let img = render(
        "<div style='width:40px;height:40px;padding:10px;background:green;\
         mask-image:url(red.png);mask-repeat:no-repeat;mask-origin:fill-box'></div>",
        scale,
    );
    assert_pixel(&img, scale, (5.0, 5.0), WHITE, "fill-box origin: not at the padding box");
    assert_pixel(&img, scale, (15.0, 15.0), GREEN, "fill-box origin: at the content box");
}
