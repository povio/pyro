//! SVG `<foreignObject>`: its HTML content is laid out in a box of the element's `width` and
//! `height` and painted in the `<svg>`'s user space (its `x`/`y`, the `viewBox` scale, its
//! transform), cut at its box (`overflow: hidden` in the UA sheet), with its clip path. Nothing
//! of it was painted before (WPT css-masking `clip-path-foreignobject-non-zero-xy` and others).
use std::sync::Arc;

use pyro_render::*;

/// Serves `fo.svg`: a green square drawn by a `<foreignObject>`'s HTML content, and a script that
/// would turn it red if it ran.
struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        if url.ends_with("fo.svg") {
            let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
              <foreignObject x="0" y="50" width="100" height="50">
                <div xmlns="http://www.w3.org/1999/xhtml" id="d" style="height: 50px; background: green"></div>
                <script xmlns="http://www.w3.org/1999/xhtml">document.getElementById('d').style.background = 'red'</script>
              </foreignObject>
            </svg>"#;
            let body = svg.as_bytes().to_vec();
            std::thread::spawn(move || done(Ok(body)));
        } else {
            done(Err("404".into()));
        }
    }
}

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff'>{body}"),
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
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

fn assert_pixel(img: &RgbaImage, (x, y): (usize, usize), want: [u8; 3], what: &str) {
    let got = pixel(img, x, y);
    assert!(got.iter().zip(want).all(|(&g, w)| g.abs_diff(w) <= 2), "{what} at ({x}, {y}): {got:?}, want {want:?}");
}

/// The content sits at the element's `x`/`y`, scaled by the `viewBox`, and is cut at its box.
#[test]
fn content_is_placed_in_user_space_and_cut_at_its_box() {
    let img = render(
        r#"<svg width="200" height="200" viewBox="0 0 100 100">
             <foreignObject x="10" y="20" width="40" height="30">
               <div style="width: 60px; height: 60px; background: green"></div>
             </foreignObject>
           </svg>"#,
    );
    assert_pixel(&img, (25, 45), GREEN, "inside the box");
    assert_pixel(&img, (95, 95), GREEN, "the box's far corner");
    assert_pixel(&img, (105, 50), WHITE, "past the box's width (cut)");
    assert_pixel(&img, (50, 105), WHITE, "past the box's height (cut)");
    assert_pixel(&img, (15, 35), WHITE, "before its x");
}

/// A `clip-path` reference on the element applies to its content
/// in user space.
#[test]
fn clip_path_applies_to_the_content() {
    let img = render(
        r#"<svg width="200" height="100">
             <clipPath id="c"><rect x="0" y="0" width="50" height="100"/></clipPath>
             <foreignObject width="100" height="100" clip-path="url(#c)">
               <div style="height: 100px; background: blue"></div>
             </foreignObject>
           </svg>"#,
    );
    assert_pixel(&img, (25, 50), BLUE, "inside the clip");
    assert_pixel(&img, (75, 50), WHITE, "outside the clip");
}

/// Without a `width` (or `height`) it renders nothing (`auto` is 0 for it).
#[test]
fn without_a_size_nothing_renders() {
    let img = render(
        r#"<svg width="200" height="100">
             <foreignObject height="100"><div style="width: 100px; height: 100px; background: green"></div></foreignObject>
           </svg>"#,
    );
    assert_pixel(&img, (50, 50), WHITE, "no width");
}

/// An SVG image's `<foreignObject>` content is drawn too (as a document of its own, whose scripts
/// do not run); usvg alone drops it.
#[test]
fn an_svg_image_draws_its_foreign_object() {
    let img = render(r#"<img src="fo.svg" style="display: block">"#);
    assert_pixel(&img, (50, 75), GREEN, "the foreignObject's content");
    assert_pixel(&img, (50, 25), WHITE, "above it");
}
