//! 3D rendering contexts and perspective, drawn as Chromium's compositor draws them (lane H2):
//! the planes of a `preserve-3d` context are sorted by depth (a plane farther from the viewer
//! paints under a nearer one whatever their order in the tree; planes that cross are split where
//! they meet), a box turned out of the plane under a perspective is drawn through its 4x4 matrix
//! (a trapezoid, not the parallelogram that stood in for it), and a plane crossing the viewer's
//! plane shows the part in front of the viewer. Each test fails without the change.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

const GREEN: [u8; 3] = [0, 128, 0];
const WHITE: [u8; 3] = [255, 255, 255];
const BLUE: [u8; 3] = [0, 0, 255];
const FUCHSIA: [u8; 3] = [255, 0, 255];
const SILVER: [u8; 3] = [192, 192, 192];

fn render(body: &str, scale: f64) -> RgbaImage {
    render_area(body, scale, Area::Viewport).unwrap()
}

fn render_area(body: &str, scale: f64, area: Area) -> Result<RgbaImage, String> {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff'>{body}"),
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
        device_scale_factor: scale,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, area, Backend::Cpu, &fetcher).map(|r| r.0).map_err(|e| format!("{e:?}"))
}

/// The pixel at CSS px (x, y).
fn pixel(img: &RgbaImage, scale: f64, x: f64, y: f64) -> [u8; 3] {
    let (x, y) = ((x * scale) as usize, (y * scale) as usize);
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

/// A 3D rendering context draws a plane moved away from the viewer under one moved towards it,
/// whatever their order in the tree, and a parent turned so that its child lies behind it over
/// the child (WPT `transform3d-sorting-001`, `-005`). In tree order the later box covered the
/// other.
#[test]
fn planes_of_a_3d_rendering_context_are_sorted_by_depth() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='position:absolute;left:100px;top:100px;transform-style:preserve-3d'>\
             <div style='width:100px;height:100px;background:green;transform:translateZ(10px)'></div>\
             <div style='width:100px;height:100px;background:red;transform:translateZ(-10px) translateY(-100px)'></div></div>",
            scale,
        );
        assert_eq!(pixel(&img, scale, 150.0, 150.0), GREEN, "siblings, scale {scale}");
        let img = render(
            "<div style='position:absolute;left:100px;top:100px;width:100px;height:100px;background:green;\
             transform-style:preserve-3d;transform:rotateX(180deg)'>\
             <div style='width:100px;height:100px;background:red;transform:translateZ(10px)'></div></div>",
            scale,
        );
        assert_eq!(pixel(&img, scale, 150.0, 150.0), GREEN, "parent in front, scale {scale}");
    }
}

/// Elements of a context without a transform of their own paint in the plane of the element they
/// are in, in paint order, and that plane is sorted with the others: a box moved back by
/// `translateZ` stays under a later block and a box in it (WPT
/// `preserve3d-and-flattening-002`, `-z-order-006`).
#[test]
fn an_untransformed_box_paints_in_its_parents_plane() {
    let abs = "position:absolute;left:0;top:0;width:100px;height:100px";
    for scale in [1.0, 2.0] {
        let img = render(
            &format!(
                "<div style='position:absolute;left:100px;top:100px;width:100px;height:100px;background:gray;transform-style:preserve-3d'>\
                 <div style='{abs};background:fuchsia'><div style='{abs};background:silver;transform:translate3d(50px,0,10px)'></div></div>\
                 <div style='{abs};background:blue;transform-style:preserve-3d;transform:translate3d(25px,50px,-20px)'></div></div>"
            ),
            scale,
        );
        assert_eq!(pixel(&img, scale, 130.0, 175.0), FUCHSIA, "scale {scale}");
        assert_eq!(pixel(&img, scale, 210.0, 175.0), SILVER, "scale {scale}");
        assert_eq!(pixel(&img, scale, 175.0, 225.0), BLUE, "scale {scale}");
    }
}

/// An untransformed box in a plane drawn through a perspective is drawn at its layout place in
/// that plane: a box 40px in from the corner of a card turned 50deg about y under a perspective of
/// 500px spans x = 157.5 to 212.5 (the card's homography of its box). The affine that stood in for
/// the box's own projection, composed with the inverse of the card's, put it 8px to the right.
#[test]
fn an_untransformed_box_paints_at_its_place_in_a_plane_under_perspective() {
    let page = "<div style='position:absolute;left:0;top:0;width:400px;height:300px;perspective:500px'>\
        <div style='position:absolute;left:100px;top:50px;width:200px;height:200px;background:green;\
        transform-style:preserve-3d;transform:rotateY(50deg)'>\
        <div style='position:absolute;left:40px;top:40px;width:80px;height:80px;background:blue'></div></div></div>";
    for scale in [1.0, 2.0] {
        let img = render(page, scale);
        for (x, y, color) in [(161.0, 128.0, BLUE), (154.0, 128.0, GREEN), (209.0, 130.0, BLUE), (216.0, 130.0, GREEN)]
        {
            assert_eq!(pixel(&img, scale, x, y), color, "({x}, {y}), scale {scale}");
        }
    }
}

/// Planes that cross are split where they meet, each side drawn in depth order: a child turned
/// 120deg in a parent turned -60deg about y meets it along the middle, the child in front on the
/// left, the parent on the right (WPT `css-transform-3d-transform-style`). The child covered all
/// of it.
#[test]
fn crossing_planes_are_split_where_they_meet() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='position:absolute;left:100px;top:100px;width:200px;height:100px;background:blue;\
             transform-style:preserve-3d;transform:rotateY(-60deg)'>\
             <div style='width:200px;height:100px;background:green;transform:rotateY(120deg)'></div></div>",
            scale,
        );
        assert_eq!(pixel(&img, scale, 175.0, 150.0), GREEN, "left, scale {scale}");
        assert_eq!(pixel(&img, scale, 225.0, 150.0), BLUE, "right, scale {scale}");
        assert_eq!(pixel(&img, scale, 140.0, 150.0), WHITE, "outside, scale {scale}");
    }
}

/// A box turned about y under a perspective is a trapezoid: its near edge taller than its far one
/// (WPT `transform-3d-rotateY-stair-*`). The affine that stood in for it kept one height.
#[test]
fn a_rotation_under_perspective_is_drawn_as_a_trapezoid() {
    let page = "<div style='position:absolute;left:100px;top:50px;width:150px;height:150px;perspective:500px'>\
        <div style='width:150px;height:150px;background:green;transform:rotateY(50deg)'></div></div>";
    for scale in [1.0, 2.0] {
        let img = render(page, scale);
        // The near (left) edge spans y = 41.1 to 208.9 at x = 125, the far edge y = 57.2 to 192.8
        // at x = 215.
        for (x, y, color) in [
            (125.0, 44.0, GREEN),
            (125.0, 206.0, GREEN),
            (125.0, 38.0, WHITE),
            (215.0, 60.0, GREEN),
            (215.0, 54.0, WHITE),
            (215.0, 196.0, WHITE),
        ] {
            assert_eq!(pixel(&img, scale, x, y), color, "({x}, {y}), scale {scale}");
        }
    }
}

/// A plane that crosses the viewer's plane shows the part in front of the viewer: a floor laid
/// down under a perspective whose far side reaches past the viewer covers the bottom of the view
/// (WPT `perspective-split-by-zero-w`). It was not drawn at all.
#[test]
fn a_plane_crossing_the_viewer_shows_its_part_in_front() {
    for style in ["", "transform-style:preserve-3d;"] {
        let page = format!(
            "<div style='position:absolute;left:0;top:0;width:400px;height:300px;perspective:200px;perspective-origin:200px 50px'>\
             <div style='position:absolute;left:0;top:150px;width:400px;height:400px;background:green;{style}\
             transform-origin:50% 0;transform:rotateX(90deg)'></div></div>"
        );
        for scale in [1.0, 2.0] {
            let img = render(&page, scale);
            assert_eq!(pixel(&img, scale, 200.0, 250.0), GREEN, "{style} scale {scale}");
            assert_eq!(pixel(&img, scale, 10.0, 290.0), GREEN, "{style} scale {scale}");
            assert_eq!(pixel(&img, scale, 200.0, 140.0), WHITE, "{style} scale {scale}");
        }
        // Its projection has no bounds: the page keeps the untransformed box's size (550px tall),
        // where a stand-in spanning the projection made it 1e5 px tall, past what a full-page
        // screenshot draws.
        let full = render_area(&page, 1.0, Area::FullPage).expect("a full-page screenshot");
        assert!(full.height <= 600, "{style} {}", full.height);
        assert_eq!(pixel(&full, 1.0, 200.0, 250.0), GREEN, "{style} full page");
    }
}

/// A `display: contents` element has no box: its children are in the 3D rendering context of the
/// element around it (WPT `transform3d-preserve3d-014`). Two turns of 90deg about x make a flip;
/// the child was flattened into the edge-on parent, and vanished.
#[test]
fn a_display_contents_element_passes_the_context_on() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='position:absolute;left:100px;top:100px;transform:rotateX(90deg);transform-style:preserve-3d'>\
             <div style='display:contents'><div style='width:100px;height:100px;background:green;transform:rotateX(90deg)'></div></div></div>",
            scale,
        );
        assert_eq!(pixel(&img, scale, 150.0, 150.0), GREEN, "scale {scale}");
    }
}

/// `backface-visibility: hidden` on a `preserve-3d` element showing its back hides what it paints
/// itself (WPT `backface-visibility-with-sibling-001`); a child with a transform of its own is a
/// plane of the context, shown or hidden by its own face.
#[test]
fn a_preserve_3d_element_showing_its_back_hides_its_own_painting() {
    let abs = "position:absolute;left:0;top:0;width:100px;height:100px";
    for scale in [1.0, 2.0] {
        let img = render(
            &format!(
                "<div style='{abs};left:100px;top:100px;transform-style:preserve-3d;transform:rotateX(120deg)'>\
                 <div style='{abs};transform-style:preserve-3d;transform:translateX(0);backface-visibility:hidden;background:red'></div></div>"
            ),
            scale,
        );
        assert_eq!(pixel(&img, scale, 150.0, 150.0), WHITE, "scale {scale}");
        let img = render(
            &format!(
                "<div style='{abs};left:100px;top:100px;transform:rotateY(180deg);backface-visibility:hidden;\
                 transform-style:preserve-3d;background:red'><div style='{abs};background:green;transform:translateZ(1px)'></div></div>"
            ),
            scale,
        );
        assert_eq!(pixel(&img, scale, 150.0, 150.0), GREEN, "scale {scale}");
    }
}
