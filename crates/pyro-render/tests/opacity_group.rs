//! An element's `opacity` applies to everything it paints, as one group (Chromium's effect node):
//! its outset box-shadow, its box, its descendants (positioned ones outside its box too: it is their
//! stacking context) and its outline. The group's layer covers what it painted, not the element's
//! border box.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn render(body: &str, scale: f64) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
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

/// The pixel at CSS px (x, y).
fn pixel(img: &RgbaImage, scale: f64, x: u32, y: u32) -> [u8; 3] {
    let (x, y) = ((x as f64 * scale) as usize, (y as f64 * scale) as usize);
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

/// `color` at `alpha` over white.
fn over_white(color: [u8; 3], alpha: f64) -> [f64; 3] {
    color.map(|c| 255.0 - alpha * (255.0 - f64::from(c)))
}

fn assert_near(got: [u8; 3], want: [f64; 3], what: &str) {
    assert!(got.iter().zip(want).all(|(&g, w)| (f64::from(g) - w).abs() <= 2.0), "{what}: {got:?}, want {want:?}");
}

const KID: &str = "position:absolute;left:150px;top:100px;width:100px;height:100px;";

/// A spotlight ring (`box-shadow: 0 0 0 6000px`) of an element with `opacity` is drawn at that
/// opacity, as the element's box is: it was drawn at full strength, outside the element's layer.
#[test]
fn an_elements_opacity_fades_its_outset_box_shadow() {
    const RING: [u8; 3] = [9, 15, 27];
    for scale in [1.0, 2.0] {
        for (opacity, blur) in [(0.5, 0), (0.45, 2), (0.9, 2)] {
            let img = render(
                &format!("<div style='{KID}opacity:{opacity};box-shadow:0 0 {blur}px 6000px rgba(9,15,27,.62)'></div>"),
                scale,
            );
            assert_near(
                pixel(&img, scale, 20, 20),
                over_white(RING, 0.62 * opacity),
                &format!("opacity {opacity}, blur {blur}px, scale {scale}"),
            );
        }
        // The same ring of a child of a faded parent.
        let img = render(
            &format!(
                "<div style='opacity:.5'><div style='{KID}box-shadow:0 0 0 6000px rgba(9,15,27,.62)'></div></div>"
            ),
            scale,
        );
        assert_near(pixel(&img, scale, 20, 20), over_white(RING, 0.31), &format!("faded parent, scale {scale}"));
    }
}

/// A faded element's group covers its positioned descendants wherever they are: a zero-size
/// wrapper (a fading popover, menu or toast) around an absolutely positioned box. They were cut
/// off at the wrapper's border box.
#[test]
fn an_opacity_group_covers_descendants_outside_the_elements_box() {
    for scale in [1.0, 2.0] {
        let img = render(&format!("<div style='opacity:.5'><div style='{KID}background:#000'></div></div>"), scale);
        assert_near(pixel(&img, scale, 200, 150), over_white([0; 3], 0.5), &format!("child, scale {scale}"));
        let img = render(
            &format!("<div style='opacity:.5'><div style='{KID}box-shadow:0 0 0 6000px #000'></div></div>"),
            scale,
        );
        assert_near(pixel(&img, scale, 20, 20), over_white([0; 3], 0.5), &format!("child's shadow, scale {scale}"));
        // In-flow content overflowing a small faded box.
        let img = render(
            "<div style='opacity:.5;width:50px;height:50px'><div style='width:300px;height:50px;background:#000'></div></div>",
            scale,
        );
        assert_near(pixel(&img, scale, 200, 25), over_white([0; 3], 0.5), &format!("overflow, scale {scale}"));
        // The outline, outside the box.
        let img = render("<div style='position:absolute;left:50px;top:50px;width:50px;height:50px;opacity:.5;outline:10px solid #000'></div>", scale);
        assert_near(pixel(&img, scale, 45, 75), over_white([0; 3], 0.5), &format!("outline, scale {scale}"));
    }
}

/// A filtered child of a faded or filtered parent is painted: the parent's layer covers the
/// child's filter layer where it is (the child's offset moved the filter's reach, and with it the
/// parent's layer, off the child: it vanished). Found by the paint matrix (`bench/fidelity/paint.ts`).
#[test]
fn a_filtered_child_of_a_faded_or_filtered_parent_is_painted() {
    let half = over_white([0; 3], 0.5);
    for scale in [1.0, 2.0] {
        let shadowed = format!("{KID}background:#000;filter:drop-shadow(10px 10px 0 #000)");
        let img = render(&format!("<div style='opacity:.5'><div style='{shadowed}'></div></div>"), scale);
        assert_near(pixel(&img, scale, 200, 150), half, &format!("drop-shadow child, scale {scale}"));
        assert_near(pixel(&img, scale, 255, 205), half, &format!("its shadow, scale {scale}"));
        // A colour function is not applied (yet), but the child is painted.
        let img = render(
            &format!("<div style='opacity:.5'><div style='{KID}background:#000;filter:grayscale(1)'></div></div>"),
            scale,
        );
        assert_near(pixel(&img, scale, 200, 150), half, &format!("grayscale child, scale {scale}"));
        let img = render(&format!("<div style='filter:grayscale(1)'><div style='{shadowed}'></div></div>"), scale);
        assert_near(pixel(&img, scale, 200, 150), [0.0; 3], &format!("child of a filtered parent, scale {scale}"));
        assert_near(pixel(&img, scale, 255, 205), [0.0; 3], &format!("its shadow, scale {scale}"));
    }
}

/// The group is faded once: where two of its boxes overlap, the result is as faded as each.
#[test]
fn a_faded_group_composites_its_content_first() {
    let img = render(
        "<div style='opacity:.5'><div style='position:absolute;left:0;top:0;width:200px;height:100px;background:#000'></div>
         <div style='position:absolute;left:100px;top:0;width:200px;height:100px;background:#000'></div></div>",
        1.0,
    );
    for x in [50, 150, 250] {
        assert_near(pixel(&img, 1.0, x, 50), over_white([0; 3], 0.5), &format!("x {x}"));
    }
    assert_eq!(pixel(&img, 1.0, 350, 50), [255; 3]);
}
