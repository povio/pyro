//! Shaped corners and borders of CSS Borders 4, as Chromium draws them (WPT `css/css-borders`).
//!
//! - `corner-shape`: each corner a superellipse quadrant (`bevel`, `scoop`, `notch`, ...) in the box
//!   its radii make. The background, the border, overflow clipping, shadows and the outline follow
//!   it, an inner or outer edge staying parallel to the border edge. Before, corners were round.
//! - `border-shape`: the border drawn as a stroke along one `<basic-shape>`, or as the area between
//!   two, the background clipped inside it. Before, the property was not parsed.
use std::sync::Arc;

use pyro_render::*;

const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 128, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const PURPLE: [u8; 3] = [128, 0, 128];
const YELLOW: [u8; 3] = [255, 255, 0];
const WHITE: [u8; 3] = [255, 255, 255];

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

/// `body` on a white 300x300 page without margins.
fn render(body: &str, scale: f64) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff'>{body}"),
        base_url: "http://test/".into(),
        width: 300,
        height: 300,
        device_scale_factor: scale,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// The pixel at CSS px (x, y).
fn pixel(img: &RgbaImage, scale: f64, x: f64, y: f64) -> [u8; 3] {
    let (x, y) = ((x * scale) as usize, (y * scale) as usize);
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

fn assert_near(got: [u8; 3], want: [u8; 3], what: &str) {
    assert!(
        got.iter().zip(want).all(|(&g, w)| (i32::from(g) - i32::from(w)).abs() <= 2),
        "{what}: {got:?}, want {want:?}"
    );
}

const BOX: &str = "position:absolute;left:0;top:0;width:100px;height:100px;box-sizing:border-box;background:green;";

#[test]
fn rotating_a_solid_circle_preserves_its_coverage() {
    let circle = |angle| render(&format!("<div style='{BOX}border-radius:50%;transform:rotate({angle}deg)'></div>"), 1.0);
    let plain = circle(0);
    assert_eq!(pixel(&plain, 1.0, 50.0, 50.0), GREEN);
    assert!(circle(45).data == plain.data);
}

/// A bevelled corner is the straight line between the ends of its radii, and the border's inner
/// edge is that line moved in by the border width along its normal (WPT `corner-shape-bevel`:
/// the padding edge meets the top at 27.071px).
#[test]
fn a_bevel_cuts_the_corner_and_the_border_stays_as_thick_along_it() {
    for scale in [1.0, 2.0] {
        let img = render(&format!("<div style='{BOX}border-radius:25px;border:5px solid red;corner-shape:bevel'></div>"), scale);
        let at = |x, y| pixel(&img, scale, x, y);
        assert_near(at(10.0, 10.0), WHITE, &format!("outside the bevel, scale {scale}"));
        assert_near(at(14.5, 14.5), RED, &format!("on the bevel's border, scale {scale}"));
        assert_near(at(20.0, 20.0), GREEN, &format!("inside the bevel, scale {scale}"));
        assert_near(at(50.0, 2.0), RED, &format!("the top side, scale {scale}"));
        assert_near(at(90.0, 90.0), WHITE, &format!("outside the bottom right bevel, scale {scale}"));
    }
}

/// A scoop is a quarter circle around the box's corner, a notch the square of its radii cut out.
#[test]
fn scoops_and_notches_cut_into_the_box() {
    let img = render(&format!("<div style='{BOX}border-radius:40px;corner-shape:scoop notch'></div>"), 1.0);
    let at = |x, y| pixel(&img, 1.0, x, y);
    // Top left scoop: the circle of radius 40 around (0, 0) is out.
    assert_near(at(25.0, 25.0), WHITE, "inside the scoop's circle");
    assert_near(at(32.0, 32.0), GREEN, "beyond the scoop's circle");
    assert_near(at(38.0, 2.0), WHITE, "the scoop near the top edge");
    // Top right notch: the 40px square at the corner is out, right up to its edges.
    assert_near(at(65.0, 35.0), WHITE, "inside the notch");
    assert_near(at(55.0, 35.0), GREEN, "beside the notch");
    assert_near(at(65.0, 45.0), GREEN, "below the notch");
}

/// Content clipped to a shaped box follows the corners (WPT `corner-shape-bevel-overflow`), as
/// does an outset shadow's spread and the outline.
#[test]
fn overflow_shadows_and_outlines_follow_the_corners() {
    let img = render(
        "<div style='position:absolute;left:50px;top:50px;width:100px;height:100px;border-radius:50px;\
         corner-shape:bevel;overflow:clip'><div style='width:200px;height:200px;background:green'></div></div>",
        1.0,
    );
    let at = |x, y| pixel(&img, 1.0, x, y);
    assert_near(at(100.0, 100.0), GREEN, "the diamond's middle");
    assert_near(at(60.0, 60.0), WHITE, "outside the diamond");
    assert_near(at(140.0, 140.0), WHITE, "outside the diamond, bottom right");

    let img = render(
        "<div style='position:absolute;left:50px;top:50px;width:100px;height:100px;border-radius:25px;\
         corner-shape:bevel;background:green;box-shadow:0 0 0 10px blue;outline:6px solid red;outline-offset:20px'></div>",
        1.0,
    );
    let at = |x, y| pixel(&img, 1.0, x, y);
    // The shadow's corner is a bevel too, parallel to the box's (10px out along its normal).
    assert_near(at(52.0, 52.0), WHITE, "outside the shadow's bevel");
    assert_near(at(58.0, 58.0), BLUE, "the shadow along the bevel");
    assert_near(at(100.0, 45.0), BLUE, "the shadow above the box");
    // The outline 20px out: its bevel passes 4.5px from the 20px-out corner of the box.
    assert_near(at(100.0, 27.0), RED, "the outline above the box");
    assert_near(at(31.0, 31.0), WHITE, "outside the outline's bevel");
}

/// `border-shape` with one shape strokes it with the border's width and colour, the background
/// filling the shape (WPT `border-shape-clips-background`: `circle()` in the half-border box).
#[test]
fn a_single_border_shape_is_a_stroke_around_the_background() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='position:absolute;left:0;top:0;width:100px;height:100px;border:10px solid purple;\
             border-shape:circle();background:green;border-radius:10px'></div>",
            scale,
        );
        let at = |x, y| pixel(&img, scale, x, y);
        assert_near(at(60.0, 60.0), GREEN, &format!("the middle, scale {scale}"));
        assert_near(at(60.0, 5.0), PURPLE, &format!("the stroke at the top, scale {scale}"));
        assert_near(at(60.0, 40.0), GREEN, &format!("inside the stroke, scale {scale}"));
        assert_near(at(8.0, 8.0), WHITE, &format!("outside the circle, scale {scale}"));
    }
}

/// With two shapes the border fills the area between them, in the relevant side's colour, and
/// the background fills the inner one (WPT `border-shape-inner-outer`).
#[test]
fn two_border_shapes_fill_the_area_between_them() {
    let img = render(
        "<div style='position:absolute;left:0;top:0;width:100px;height:100px;\
         border-shape:polygon(50% 0, 100% 50%, 50% 100%, 0 50%) circle(20%);border-color:yellow;background:green'></div>",
        1.0,
    );
    let at = |x, y| pixel(&img, 1.0, x, y);
    assert_near(at(50.0, 50.0), GREEN, "inside the inner circle");
    assert_near(at(50.0, 20.0), YELLOW, "between the diamond and the circle");
    assert_near(at(10.0, 10.0), WHITE, "outside the diamond");
    let _ = RED;
}
