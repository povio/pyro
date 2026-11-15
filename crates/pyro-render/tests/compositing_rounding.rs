//! Pixel-exact compositing: pages that draw the same colours two ways (a translucent box over
//! another and the opaque colour they make, an inset shadow and a nested box, `rotate: 45deg` and
//! `transform: rotate(45deg)`, `drop-shadow()` and `blur()`) come out the same, to the level, as
//! in Chromium. Each was one or two levels apart (WPT css-backgrounds box-shadow-041/042 and
//! box-shadow-multiple-001, css-color t32-opacity-zorder-c, filter-effects
//! backdrop-filter-isolation-*, drop-shadow-scale, css-transforms individual-transform-2a/b/c).
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
        width: 800,
        height: 600,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// The pixel at (x, y).
fn pixel(img: &RgbaImage, x: usize, y: usize) -> [u8; 3] {
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

/// The pixels whose R, G or B differ (as WPT's reftest runner compares), and the largest difference.
fn difference(a: &RgbaImage, b: &RgbaImage) -> (usize, u8) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let mut count = 0;
    let mut max = 0;
    for (p, q) in a.data.chunks_exact(4).zip(b.data.chunks_exact(4)) {
        let d = (0..3).map(|c| p[c].abs_diff(q[c])).max().unwrap();
        if d > 0 {
            count += 1;
            max = max.max(d);
        }
    }
    (count, max)
}

/// Translucent shadows over translucent shadows make the colours WPT's box-shadow-multiple-001
/// spells out as opaque ones (`#BF7F7F`, `#FFD23F`, `#7F7FBF`, `#FF7FBF`): vello_cpu divided by 255
/// rounding up, and each came out a level high in one channel.
#[test]
fn translucent_shadows_over_translucent_shadows_round_as_chromium() {
    let img = render(
        "<div style='position:absolute;left:100px;top:100px;width:50px;height:50px;background:yellow;
          box-shadow: rgb(128 0 128 / 50%) 0 -50px, rgb(255 255 0 / 50%) 0 -50px,
                      rgb(0 0 255 / 50%) 50px 0, rgb(255 255 0 / 50%) 50px 0,
                      rgb(255 0 255 / 50%) 0 50px, rgb(255 255 0 / 50%) 0 50px,
                      rgb(255 165 0 / 50%) -50px 0, rgb(255 255 0 / 50%) -50px 0'></div>",
    );
    assert_eq!(pixel(&img, 125, 75), [191, 127, 127], "purple over yellow");
    assert_eq!(pixel(&img, 175, 125), [127, 127, 191], "blue over yellow");
    assert_eq!(pixel(&img, 125, 175), [255, 127, 191], "fuchsia over yellow");
    assert_eq!(pixel(&img, 75, 125), [255, 210, 63], "orange over yellow");
}

/// An inset shadow is the box drawn inside the padding box (box-shadow-041 and its reference).
#[test]
fn an_inset_shadow_paints_as_a_nested_box() {
    const BOX: &str = "background:rgba(0,0,255,.5);border:black double 18px;height:36px;width:36px;margin:36px";
    let test = render(&format!("<div style='{BOX};box-shadow:inset 18px 0 rgba(255,165,0,.5)'></div>"));
    let reference = render(&format!(
        "<div style='{BOX}'><div style='background:rgba(255,165,0,.5);height:36px;width:18px'></div></div>"
    ));
    assert_eq!(pixel(&reference, 36 + 18 + 5, 36 + 18 + 5), [191, 146, 127]);
    assert_eq!(difference(&test, &reference), (0, 0));
}

/// White seen through `opacity: .99` reds fades out, as in Chromium (t32-opacity-zorder-c: a
/// green box at `.99` over three such reds is the green box over one opaque red).
#[test]
fn layers_of_opacity_hide_what_lies_under_them() {
    const CELL: &str = "position:absolute;left:10px;top:10px;width:10px;height:10px;opacity:.99";
    let test = render(&format!(
        "<div style='{CELL};background:red'></div><div style='{CELL};background:red'></div>
         <div style='{CELL};background:red'></div><div style='{CELL};background:green'></div>"
    ));
    let reference = render(&format!(
        "<div style='{CELL};opacity:1;background:red'></div><div style='{CELL};background:green'></div>"
    ));
    assert_eq!(pixel(&reference, 15, 15), [3, 127, 0]);
    assert_eq!(pixel(&test, 15, 15), [3, 127, 0]);
}

/// A translucent box over the backdrop its `backdrop-filter` inverted is the colour the
/// reference spells out (backdrop-filter-isolation-isolate: `#ff08` over inverted green is
/// `#ffc377`).
#[test]
fn a_translucent_box_over_its_filtered_backdrop_rounds_as_chromium() {
    let img = render(
        "<div style='position:absolute;left:10px;top:10px;width:100px;height:100px;background:green'></div>
         <div style='position:absolute;left:40px;top:40px;width:100px;height:100px;backdrop-filter:invert(1);background:#ff08'></div>",
    );
    assert_eq!(pixel(&img, 60, 60), [255, 195, 119]);
}

/// `rotate: 45deg` is the matrix `transform: rotate(45deg)` is: the property's f64 sine and
/// cosine moved antialiased edges a level from the function's f32 ones (individual-transform-2a,
/// -2b and -2c, 27 pixels each).
#[test]
fn the_rotate_property_draws_as_the_rotate_function() {
    const BOX: &str = "position:fixed;width:100px;height:100px;top:200px;left:200px;transform-origin:0 0;\
                       border-style:solid;border-width:10px 0;border-color:lime";
    let reference = render(&format!(
        "<div style='{BOX};transform:translate(50px,50px) rotate(45deg) scale(2,2)'></div>"
    ));
    for individual in [
        "translate:50px 50px;rotate:45deg;scale:2 2",
        "rotate:45deg;scale:2 2;translate:50px 50px",
        "translate:50px 50px;rotate:45deg;transform:scale(2,2)",
    ] {
        let test = render(&format!("<div style='{BOX};{individual}'></div>"));
        assert_eq!(difference(&test, &reference), (0, 0), "{individual}");
    }
}

/// A large `drop-shadow()` (rendered at a reduced size) is the same blur as `blur()` of the same
/// box where it lands: its reduced pixels were laid on a grid half a reduced pixel off the frame's
/// when its offset was not a multiple of the reduction (drop-shadow-scale: 59,716 pixels a level
/// or two apart).
#[test]
fn a_reduced_drop_shadow_is_the_reduced_blur_where_it_lands() {
    const BOX: &str =
        "position:absolute;width:20px;height:20px;overflow:hidden;transform:scale(10);transform-origin:bottom left;will-change:transform";
    let test = render(&format!(
        "<div style='{BOX};left:20px;top:200px;background:green;filter:drop-shadow(30px 10px 4px blue)'></div>"
    ));
    let reference = render(&format!(
        "<div style='{BOX};left:320px;top:300px;background:blue;filter:blur(4px)'></div>
         <div style='{BOX};left:20px;top:200px;background:linear-gradient(green,green)'></div>"
    ));
    let (count, max) = difference(&test, &reference);
    assert!(count <= 1000 && max <= 2, "{count} pixels differ, by up to {max}");
}
