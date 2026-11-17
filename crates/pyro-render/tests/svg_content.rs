//! SVG content drawn as Chromium draws it: `<pattern>` paint servers, clip
//! paths of any content (several shapes, `<use>`, nested clip paths), `<mask>`
//! (luminance and alpha), and the page's CSS on inline SVG elements (fills,
//! transforms in their `transform-box`, `clip-path` shapes, `mask-image`,
//! values inherited from HTML ancestors).
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn render(body: &str) -> RgbaImage {
    let s = Snapshot {
        html: format!("<!doctype html><style>body{{margin:0;background:white}}svg{{display:block}}</style>{body}"),
        base_url: "http://test/".into(),
        width: 200,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    Renderer::new().render(&s, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn px(img: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let i = (y as usize * img.width as usize + x as usize) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

const WHITE: [u8; 3] = [255, 255, 255];
const RED: [u8; 3] = [255, 0, 0];
const LIME: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const YELLOW: [u8; 3] = [255, 255, 0];

#[test]
fn a_pattern_tile_is_drawn_and_repeated() {
    // A 20x20 tile, four 10x10 quadrants, over an 80x40 rect stretched 2x
    // horizontally by its transform.
    let img = render(
        r##"<svg width="200" height="100"><defs>
        <pattern id="p" patternUnits="userSpaceOnUse" width="20" height="20">
          <rect width="10" height="10" fill="#ff0000"/><rect x="10" width="10" height="10" fill="#00ff00"/>
          <rect y="10" width="10" height="10" fill="#0000ff"/><rect x="10" y="10" width="10" height="10" fill="#ffff00"/>
        </pattern></defs>
        <rect width="40" height="40" fill="url(#p)" transform="matrix(2 0 0 1 0 0)"/></svg>"##,
    );
    assert_eq!(px(&img, 10, 5), RED);
    assert_eq!(px(&img, 30, 5), LIME);
    assert_eq!(px(&img, 10, 15), BLUE);
    assert_eq!(px(&img, 30, 15), YELLOW);
    // The second tile, across and down.
    assert_eq!(px(&img, 50, 25), RED);
    assert_eq!(px(&img, 70, 35), YELLOW);
    assert_eq!(px(&img, 90, 5), WHITE, "outside the rect");
}

#[test]
fn a_clip_path_is_the_union_of_its_shapes() {
    let img = render(
        r##"<svg width="200" height="100"><clipPath id="c">
          <rect width="20" height="20"/><rect x="40" width="20" height="20"/>
          <use href="#u"/></clipPath>
        <defs><rect id="u" x="80" width="20" height="20"/></defs>
        <rect width="120" height="20" fill="#0000ff" clip-path="url(#c)"/></svg>"##,
    );
    assert_eq!(px(&img, 10, 10), BLUE, "first shape");
    assert_eq!(px(&img, 30, 10), WHITE, "between the shapes");
    assert_eq!(px(&img, 50, 10), BLUE, "second shape");
    assert_eq!(px(&img, 90, 10), BLUE, "a <use> in the clip path");
    assert_eq!(px(&img, 110, 10), WHITE);
}

#[test]
fn a_clip_path_with_a_clip_path_is_their_intersection() {
    let img = render(
        r##"<svg width="200" height="100">
        <clipPath id="outer"><rect x="20" width="100" height="100"/></clipPath>
        <clipPath id="inner" clip-path="url(#outer)"><rect width="60" height="60"/></clipPath>
        <rect width="100" height="100" fill="#0000ff" clip-path="url(#inner)"/></svg>"##,
    );
    assert_eq!(px(&img, 10, 10), WHITE, "outside the clip path's own clip path");
    assert_eq!(px(&img, 40, 40), BLUE);
    assert_eq!(px(&img, 70, 40), WHITE, "outside the clip path's shape");
}

#[test]
fn a_clip_path_child_with_a_transform_is_placed_by_it() {
    let img = render(
        r##"<svg width="200" height="100"><clipPath id="c" transform="translate(50 0)">
          <rect width="20" height="20" transform="translate(0 30)"/></clipPath>
        <rect width="100" height="100" fill="#0000ff" clip-path="url(#c)"/></svg>"##,
    );
    assert_eq!(px(&img, 10, 10), WHITE);
    assert_eq!(px(&img, 60, 40), BLUE);
}

#[test]
fn a_luminance_mask_shows_where_it_is_white() {
    let img = render(
        r##"<svg width="200" height="100"><mask id="m" maskUnits="userSpaceOnUse" x="0" y="0" width="200" height="100">
          <rect width="50" height="100" fill="white"/><rect x="50" width="50" height="100" fill="black"/>
          <rect x="100" width="50" height="100" fill="rgb(128,128,128)"/></mask>
        <rect width="150" height="100" fill="#0000ff" mask="url(#m)"/></svg>"##,
    );
    assert_eq!(px(&img, 25, 50), BLUE);
    assert_eq!(px(&img, 75, 50), WHITE);
    let gray = px(&img, 125, 50);
    assert!((125..=131).contains(&gray[0]) && gray[2] == 255, "half covered: {gray:?}");
}

#[test]
fn an_alpha_mask_uses_the_alpha_of_its_content() {
    let img = render(
        r##"<svg width="200" height="100"><mask id="m" mask-type="alpha" maskUnits="userSpaceOnUse" x="0" y="0" width="200" height="100">
          <rect width="50" height="100" fill="black"/></mask>
        <rect width="100" height="100" fill="#0000ff" mask="url(#m)"/></svg>"##,
    );
    assert_eq!(px(&img, 25, 50), BLUE, "black is opaque in an alpha mask");
    assert_eq!(px(&img, 75, 50), WHITE);
}

#[test]
fn page_css_fills_and_strokes_svg_elements() {
    let img = render(
        r##"<style>.a { fill: #00ff00 } #b { fill: none; stroke: #0000ff; stroke-width: 10px }</style>
        <svg width="200" height="100"><rect class="a" width="40" height="40" fill="#ff0000"/>
        <rect id="b" x="60" y="5" width="30" height="30"/></svg>"##,
    );
    assert_eq!(px(&img, 20, 20), LIME, "CSS beats the presentation attribute");
    assert_eq!(px(&img, 61, 20), BLUE, "the stroke");
    assert_eq!(px(&img, 75, 20), WHITE, "fill: none");
}

#[test]
fn css_inherited_by_the_svg_root_from_html() {
    let img = render(
        r##"<div style="fill: #0000ff"><svg width="200" height="100"><rect width="40" height="40"/></svg></div>"##,
    );
    assert_eq!(px(&img, 20, 20), BLUE);
}

#[test]
fn css_transforms_resolve_against_their_transform_box() {
    let img = render(
        r##"<style>
          #fill { transform-box: fill-box; transform: translate(-50%, 0) }
          #view { transform-box: view-box; transform: translate(50%, 0) }
          #origin { transform-box: fill-box; transform-origin: center; transform: rotate(90deg) }
        </style>
        <svg width="200" height="100">
          <rect id="fill" x="100" width="40" height="20" fill="#0000ff"/>
          <rect id="view" y="30" width="20" height="20" fill="#00ff00"/>
          <rect id="origin" x="10" y="60" width="40" height="10" fill="#ff0000"/>
        </svg>"##,
    );
    // Half of its own 40px width to the left: 80..120.
    assert_eq!(px(&img, 85, 10), BLUE);
    assert_eq!(px(&img, 125, 10), WHITE);
    // Half of the 200px viewport to the right: 100..120.
    assert_eq!(px(&img, 110, 40), LIME);
    assert_eq!(px(&img, 10, 40), WHITE);
    // Rotated about its own centre (30, 65): 25..35 x 45..85.
    assert_eq!(px(&img, 30, 50), RED);
    assert_eq!(px(&img, 15, 65), WHITE);
}

#[test]
fn a_css_clip_path_shape_clips_an_svg_element() {
    let img = render(
        r##"<style>#r { clip-path: circle(20px at 50% 50%) fill-box }</style>
        <svg width="200" height="100"><rect id="r" x="20" y="20" width="60" height="60" fill="#0000ff"/></svg>"##,
    );
    assert_eq!(px(&img, 50, 50), BLUE);
    assert_eq!(px(&img, 25, 25), WHITE, "a corner outside the circle");
}

#[test]
fn a_css_mask_image_reference_masks_an_svg_element() {
    let img = render(
        r##"<style>#r { mask-image: url(#m) }</style>
        <svg width="200" height="100"><mask id="m" maskUnits="userSpaceOnUse" x="0" y="0" width="200" height="100">
          <rect width="50" height="100" fill="white"/></mask>
        <rect id="r" width="100" height="100" fill="#0000ff"/></svg>"##,
    );
    assert_eq!(px(&img, 25, 50), BLUE);
    assert_eq!(px(&img, 75, 50), WHITE);
}

#[test]
fn a_clip_path_attribute_takes_a_basic_shape() {
    // The presentation attribute is the CSS property: a shape and a box.
    let img = render(
        r##"<svg width="200" height="100"><rect x="20" y="20" width="60" height="60" fill="#0000ff"
            clip-path="circle(20px at 50% 50%) fill-box"/></svg>"##,
    );
    assert_eq!(px(&img, 50, 50), BLUE);
    assert_eq!(px(&img, 25, 25), WHITE);
}

#[test]
fn an_ellipse_clip_takes_each_radius_from_its_own_axis() {
    // `ellipse()` is `closest-side` on each axis: 100 x 50 in a 200 x 100 box.
    let img = render(
        r##"<svg width="200" height="100"><rect width="200" height="100" fill="#0000ff" style="clip-path: ellipse()"/></svg>"##,
    );
    assert_eq!(px(&img, 3, 50), BLUE, "near the left end of the 100px horizontal radius");
    assert_eq!(px(&img, 100, 3), BLUE);
    assert_eq!(px(&img, 10, 10), WHITE);
}

#[test]
fn an_invalid_css_transform_leaves_the_transform_attribute() {
    let img = render(
        r##"<svg width="200" height="100"><rect width="20" height="20" fill="#0000ff"
            style="transform: scale(invalid)" transform="translate(50 50)"/></svg>"##,
    );
    assert_eq!(px(&img, 60, 60), BLUE);
    assert_eq!(px(&img, 10, 10), WHITE);
}

#[test]
fn the_roots_opacity_applies_once() {
    // The outer <svg>'s `opacity` is its CSS box's (blitz-paint's), not written for
    // usvg as well.
    let img = render(
        r##"<svg width="100" height="100" style="opacity: 0.5"><rect width="100" height="100" fill="#000000"/></svg>"##,
    );
    let gray = px(&img, 50, 50);
    assert!((126..=129).contains(&gray[0]), "half black over white, not a quarter: {gray:?}");
}

#[test]
fn an_invalid_transform_origin_attribute_is_ignored() {
    // `50,50` is no CSS origin (usvg read it as 50 50): `0 0` applies, and the
    // half turn about it takes the rect out of view instead of to 80..100.
    let img = render(
        r##"<svg width="200" height="100"><rect width="20" height="20" fill="#0000ff"
            transform="rotate(180)" transform-origin="50,50"/></svg>"##,
    );
    assert_eq!(px(&img, 90, 90), WHITE);
}

#[test]
fn svg_presentation_variables_reach_painted_content() {
    let img = render(
        r#"<style>:root { --paint: blue; --weight: 400 } body { font-weight: 700 }</style>
        <svg width="200" height="100" fill="none" stroke="var(--paint)" stroke-width="8">
          <path d="M10 20H80"/>
          <text x="10" y="70" stroke="none" fill="black" font-size="32" font-weight="var(--weight)">word</text>
        </svg>"#,
    );
    assert_eq!(px(&img, 40, 20), BLUE, "computed presentation paint reaches usvg");
    let literal = render(
        r#"<style>body { font-weight: 700 }</style>
        <svg width="200" height="100" fill="none" stroke="blue" stroke-width="8">
          <path d="M10 20H80"/>
          <text x="10" y="70" stroke="none" fill="black" font-size="32" style="font-weight:400">word</text>
        </svg>"#,
    );
    assert_eq!(img.data, literal.data, "variable font weight paints like its resolved value");
}
