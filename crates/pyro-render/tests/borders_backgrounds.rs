//! Borders, backgrounds and shadows as CSS Backgrounds 3 and Chromium paint them (WPT
//! `css/css-backgrounds`, `css/CSS2/backgrounds`, `css/compositing`): `border-image`, the canvas
//! background of the root element (and the body's, propagated to it), image tiles over the whole
//! background painting area, `background-clip: border-area`, and the shadow shape's radii.
use std::sync::Arc;

use pyro_render::*;

const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 128, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const WHITE: [u8; 3] = [255, 255, 255];
const BLACK: [u8; 3] = [0, 0, 0];

/// A 30x30 image of nine 10x10 cells, each its own colour (`GRID`, row by row), but the middle
/// one of the top row: its left half `TOP_A`, its right half `TOP_B`.
const GRID: [[u8; 3]; 9] = [
    [255, 0, 0],
    [0, 0, 0],
    [0, 0, 255],
    [255, 255, 0],
    [0, 255, 255],
    [255, 0, 255],
    [0, 128, 0],
    [128, 0, 128],
    [128, 128, 0],
];
const TOP_A: [u8; 3] = [0, 0, 0];
const TOP_B: [u8; 3] = [255, 128, 0];

fn grid_png() -> Vec<u8> {
    let mut data = Vec::with_capacity(30 * 30 * 4);
    for y in 0..30 {
        for x in 0..30 {
            let mut color = GRID[(y / 10) * 3 + x / 10];
            if y < 10 && (10..20).contains(&x) {
                color = if x < 15 { TOP_A } else { TOP_B };
            }
            data.extend_from_slice(&[color[0], color[1], color[2], 255]);
        }
    }
    encode_png(&RgbaImage { width: 30, height: 30, data })
}

/// `n` by `n` pixels of `color`.
fn solid_png(n: u32, color: [u8; 3]) -> Vec<u8> {
    let data = (0..n * n).flat_map(|_| [color[0], color[1], color[2], 255]).collect();
    encode_png(&RgbaImage { width: n, height: n, data })
}

/// 2x2 pixels: the top row red, the bottom one green.
fn red_over_green_png() -> Vec<u8> {
    let data = [RED, RED, GREEN, GREEN].iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect();
    encode_png(&RgbaImage { width: 2, height: 2, data })
}

/// Serves the images above by name; anything else fails to load.
struct Images;
impl Fetcher for Images {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let name = url.rsplit('/').next().unwrap_or_default().to_string();
        std::thread::spawn(move || {
            done(match name.as_str() {
                "grid.png" => Ok(grid_png()),
                "blue.png" => Ok(solid_png(10, BLUE)),
                "red-over-green.png" => Ok(red_over_green_png()),
                _ => Err("404".into()),
            })
        });
    }
}

/// A whole document, 400x300 CSS px.
fn render_document(html: &str, scale: f64) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(Images);
    let snapshot = Snapshot {
        html: html.into(),
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
        device_scale_factor: scale,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// `body` on a white page without margins.
fn render(body: &str, scale: f64) -> RgbaImage {
    render_document(&format!("<!doctype html><body style='margin:0;background:#fff'>{body}"), scale)
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

const BOX: &str = "position:absolute;left:20px;top:20px;width:60px;height:60px;border:20px solid red;";

/// A border image is cut in nine and each piece drawn in its part of the border: the corners
/// scaled to the border's widths, the sides stretched along them; the middle only with `fill`.
/// The border's own style is not drawn. It was not implemented: the red border was drawn.
#[test]
fn a_border_image_is_drawn_in_place_of_the_border() {
    for scale in [1.0, 2.0] {
        let img = render(&format!("<div style='{BOX}border-image:url(grid.png) 10'></div>"), scale);
        let at = |x, y| pixel(&img, scale, x, y);
        assert_near(at(30.0, 30.0), GRID[0], &format!("top left corner, scale {scale}"));
        assert_near(at(110.0, 30.0), GRID[2], &format!("top right corner, scale {scale}"));
        assert_near(at(30.0, 110.0), GRID[6], &format!("bottom left corner, scale {scale}"));
        assert_near(at(110.0, 110.0), GRID[8], &format!("bottom right corner, scale {scale}"));
        assert_near(at(30.0, 70.0), GRID[3], &format!("left side, scale {scale}"));
        assert_near(at(110.0, 70.0), GRID[5], &format!("right side, scale {scale}"));
        assert_near(at(70.0, 110.0), GRID[7], &format!("bottom side, scale {scale}"));
        // The top side's two halves, stretched over its 60px.
        assert_near(at(55.0, 30.0), TOP_A, &format!("top side's first half, scale {scale}"));
        assert_near(at(85.0, 30.0), TOP_B, &format!("top side's second half, scale {scale}"));
        assert_near(at(70.0, 70.0), WHITE, &format!("no middle without fill, scale {scale}"));

        let filled = render(&format!("<div style='{BOX}border-image:url(grid.png) 10 fill'></div>"), scale);
        assert_near(pixel(&filled, scale, 70.0, 70.0), GRID[4], &format!("the middle with fill, scale {scale}"));
    }
}

/// `round` fits whole tiles along a side, `repeat` centres them, `space` puts the space left
/// over around them.
#[test]
fn a_border_image_side_tiles_by_its_repeat_rule() {
    let scale = 1.0;
    // The top side is 60px long and its tiles are 20px (the 10px slice scaled to the 20px
    // border): three of them, each half TOP_A and half TOP_B.
    let img = render(&format!("<div style='{BOX}border-image:url(grid.png) 10 round'></div>"), scale);
    for (x, want) in [(45.0, TOP_A), (55.0, TOP_B), (65.0, TOP_A), (75.0, TOP_B)] {
        assert_near(pixel(&img, scale, x, 30.0), want, &format!("round, x {x}"));
    }
    // 70px: `repeat` centres a tile, so the side starts in the middle of one (half TOP_B).
    let wide = "position:absolute;left:20px;top:20px;width:70px;height:60px;border:20px solid red;";
    let img = render(&format!("<div style='{wide}border-image:url(grid.png) 10 repeat'></div>"), scale);
    for (x, want) in [(42.0, TOP_B), (50.0, TOP_A), (60.0, TOP_B)] {
        assert_near(pixel(&img, scale, x, 30.0), want, &format!("repeat, x {x}"));
    }
    // `space`: three 20px tiles in 70px, 2.5px apart; the first starts at 42.5.
    let img = render(&format!("<div style='{wide}border-image:url(grid.png) 10 space'></div>"), scale);
    assert_near(pixel(&img, scale, 41.0, 30.0), WHITE, "space before the first tile");
    assert_near(pixel(&img, scale, 45.0, 30.0), TOP_A, "space, the first tile");
}

/// A gradient is a border image too, sized as the border image area; `border-image-outset`
/// moves that area out of the border box.
#[test]
fn a_gradient_border_image_and_its_outset() {
    for scale in [1.0, 2.0] {
        let img = render(
            &format!("<div style='{BOX}border-image:linear-gradient(blue,blue) 10;border-image-outset:10px'></div>"),
            scale,
        );
        assert_near(pixel(&img, scale, 15.0, 15.0), BLUE, &format!("outset corner, scale {scale}"));
        assert_near(pixel(&img, scale, 60.0, 15.0), BLUE, &format!("outset side, scale {scale}"));
        // The area runs from 10px, its left side 20px wide: the old border's inner part (at
        // 37px) is empty, and the border's own red is not drawn.
        assert_near(pixel(&img, scale, 37.0, 60.0), WHITE, &format!("the old border's inner part, scale {scale}"));
    }
}

/// An image that does not load leaves the border to be drawn as its style says.
#[test]
fn a_border_image_that_does_not_load_leaves_the_border() {
    let img = render(&format!("<div style='{BOX}border-image:url(missing.png) 10'></div>"), 1.0);
    assert_near(pixel(&img, 1.0, 30.0, 30.0), RED, "the border");
}

/// The root element's background is the canvas's: over the whole page, its margin too, and
/// under the root's opacity (it was painted at full strength before the root, and again in the
/// root's border box).
#[test]
fn the_root_background_covers_the_canvas() {
    for scale in [1.0, 2.0] {
        let img = render_document(
            "<!doctype html><html style='margin:50px;height:100px;background:linear-gradient(blue,blue)'><body></body>",
            scale,
        );
        assert_near(pixel(&img, scale, 10.0, 10.0), BLUE, &format!("the root's margin, scale {scale}"));
        assert_near(pixel(&img, scale, 300.0, 250.0), BLUE, &format!("below the root, scale {scale}"));

        let img = render_document(
            "<!doctype html><html style='background:#000;opacity:.5;height:100px'><body></body>",
            scale,
        );
        for (x, y) in [(10.0, 10.0), (200.0, 250.0)] {
            assert_near(pixel(&img, scale, x, y), [128, 128, 128], &format!("faded canvas at {x},{y}, scale {scale}"));
        }

        // A translucent root background is painted once.
        let img =
            render_document("<!doctype html><html style='background:rgba(0,0,0,.5);height:100px'><body></body>", scale);
        for (x, y) in [(10.0, 10.0), (200.0, 250.0)] {
            assert_near(
                pixel(&img, scale, x, y),
                [128, 128, 128],
                &format!("translucent canvas at {x},{y}, scale {scale}"),
            );
        }
    }
}

/// The root's background images are positioned against the root element and repeat over the
/// canvas; the body's background, propagated to the canvas, is not painted again in the body.
#[test]
fn root_and_body_background_images_repeat_over_the_canvas() {
    let img = render_document(
        "<!doctype html><html style='margin:50px;background:url(blue.png) repeat-x'><body style='margin:0;height:10px'></body>",
        1.0,
    );
    assert_near(pixel(&img, 1.0, 5.0, 55.0), BLUE, "the row of tiles, in the root's margin");
    assert_near(pixel(&img, 1.0, 395.0, 55.0), BLUE, "the row of tiles, at the far end");
    assert_near(pixel(&img, 1.0, 5.0, 45.0), WHITE, "above the row");

    let img = render_document(
        "<!doctype html><html><body style='margin:50px;height:10px;background:url(blue.png) no-repeat 0 0'></body>",
        1.0,
    );
    // Positioned against the root (at 0,0), not the body (at 50,50).
    assert_near(pixel(&img, 1.0, 5.0, 5.0), BLUE, "the body's image, at the root's origin");
    assert_near(pixel(&img, 1.0, 55.0, 55.0), WHITE, "not in the body");
}

/// A body that is not rendered (`display: none` or `contents`) or a root that is not, has no
/// background on the canvas; a root or body with containment keeps the body's to the body.
#[test]
fn backgrounds_that_do_not_reach_the_canvas() {
    for html in [
        "<!doctype html><html><body style='background:red;display:none'></body>",
        "<!doctype html><html><body style='background:red;display:contents'></body>",
        "<!doctype html><html style='background:red;display:none'><body></body>",
    ] {
        let img = render_document(html, 1.0);
        assert_near(pixel(&img, 1.0, 200.0, 150.0), WHITE, html);
    }
    let img = render_document(
        "<!doctype html><html style='contain:paint'><body style='margin:0;height:50px;background:green'></body>",
        1.0,
    );
    assert_near(pixel(&img, 1.0, 200.0, 25.0), GREEN, "the body's own box");
    assert_near(pixel(&img, 1.0, 200.0, 150.0), WHITE, "the canvas, under a contained root");
}

/// A repeating image fills the background painting area: with the default `border-box` clip,
/// the border area past the padding box on every side (only the area left and above the image's
/// position was covered).
#[test]
fn a_repeating_image_fills_the_border_area() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='position:absolute;left:20px;top:20px;width:55px;height:55px;border:20px dashed transparent;background:url(blue.png)'></div>",
            scale,
        );
        for (x, y) in [(25.0, 25.0), (110.0, 60.0), (60.0, 110.0), (110.0, 110.0)] {
            assert_near(pixel(&img, scale, x, y), BLUE, &format!("border area at {x},{y}, scale {scale}"));
        }
    }
}

/// An image that does not repeat stops at its edge: a scaled one's last pixels are its own, not
/// a blend with the opposite edge (WPT `background-image-no-repeat-does-not-wrap.html`).
#[test]
fn a_non_repeating_image_does_not_wrap_at_its_edges() {
    let img = render(
        "<div style='position:absolute;left:0;top:0;width:200px;height:200px;background:url(red-over-green.png) no-repeat 0 0/100% 100%'></div>",
        1.0,
    );
    assert_near(pixel(&img, 1.0, 100.0, 199.0), GREEN, "the bottom edge");
    assert_near(pixel(&img, 1.0, 100.0, 0.0), RED, "the top edge");
}

/// `background-clip: border-area` paints where the border is, whatever its colour.
#[test]
fn border_area_clips_to_the_border() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='position:absolute;left:20px;top:20px;width:60px;height:60px;border:20px solid transparent;border-right-style:hidden;background:green;background-clip:border-area'></div>",
            scale,
        );
        assert_near(pixel(&img, scale, 30.0, 60.0), GREEN, &format!("left border, scale {scale}"));
        assert_near(pixel(&img, scale, 60.0, 30.0), GREEN, &format!("top border, scale {scale}"));
        assert_near(pixel(&img, scale, 60.0, 60.0), WHITE, &format!("padding box, scale {scale}"));
    }
}

/// A shadow's spread grows each corner radius by the spread formula (a 5px radius of a 20px box
/// spread by 20px is 17.6px), not the box's average radius left as it was; a circle's shadow stays
/// a circle (WPT `box-shadow-border-radius-001.html`).
#[test]
fn a_spread_shadow_grows_its_corner_radii() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='position:absolute;left:100px;top:100px;width:20px;height:20px;border-radius:5px;box-shadow:0 0 0 20px #000'></div>",
            scale,
        );
        // The shadow is 80..140; a 17.6px corner leaves (82, 82) out, a 5px one has it in.
        assert_near(pixel(&img, scale, 82.0, 82.0), WHITE, &format!("outside the round corner, scale {scale}"));
        assert_near(pixel(&img, scale, 90.0, 90.0), BLACK, &format!("inside it, scale {scale}"));
        assert_near(pixel(&img, scale, 110.0, 81.0), BLACK, &format!("the straight edge, scale {scale}"));

        // A 2px circle spread by 98px: a 200px circle around (101, 101), not a square.
        let img = render(
            "<div style='position:absolute;left:100px;top:100px;width:2px;height:2px;border-radius:50%;box-shadow:0 0 0 98px #000'></div>",
            scale,
        );
        assert_near(pixel(&img, scale, 10.0, 10.0), WHITE, &format!("the circle's corner, scale {scale}"));
        assert_near(pixel(&img, scale, 101.0, 10.0), BLACK, &format!("the circle's top, scale {scale}"));
    }
}

/// `background-attachment: local` is parsed (Stylo had it for Gecko only, so the whole
/// `background` declaration was dropped), and on a scroll container its layers, the colour
/// with them, are cut to the padding box: they belong to the scrolled content.
#[test]
fn a_local_background_is_cut_to_the_padding_box_of_a_scroll_container() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='position:absolute;left:20px;top:20px;width:60px;height:60px;border:20px dashed transparent;overflow:hidden;background:green local border-box'><div style='height:300px'></div></div>",
            scale,
        );
        assert_near(pixel(&img, scale, 60.0, 60.0), GREEN, &format!("the padding box, scale {scale}"));
        assert_near(pixel(&img, scale, 25.0, 60.0), WHITE, &format!("the border, scale {scale}"));
    }
}

/// `fixed` is `scroll` under a transform, an identity one (`translate(0)`) and `will-change:
/// transform` too (WPT `transform-fixed-bg-005.html`, `-008.tentative.html`): the image is at the
/// box's origin, not the viewport's.
#[test]
fn a_fixed_background_under_an_identity_transform_scrolls() {
    for transform in ["transform:translate(0)", "will-change:transform"] {
        let img = render(
            &format!("<div style='position:absolute;left:50px;top:50px;width:100px;height:100px;background:url(grid.png) fixed no-repeat 0 0;{transform}'></div>"),
            1.0,
        );
        assert_near(pixel(&img, 1.0, 55.0, 55.0), GRID[0], transform);
    }
}

/// `inset`/`outset` sides and the halves of `groove`/`ridge` take Blink's shades
/// (`BoxBorderPainter::CalculateInsetOutsetColor`: `Color::Dark` on the darkened sides,
/// `Color::Light` on the others, very dark colours lightened on both and very light ones kept).
/// The expected values are Chrome's (measured 2026-11-18). The light sides kept the base
/// colour before (`#888` outset's top was 136, Chrome's 221).
#[test]
fn bevelled_border_sides_take_chrome_shades() {
    let boxes = "<style>div{position:absolute;width:40px;height:40px;border-width:20px}</style>\
        <div style='left:0;top:0;border-style:inset;border-color:#888'></div>\
        <div style='left:100px;top:0;border-style:outset;border-color:#888'></div>\
        <div style='left:200px;top:0;border-style:inset;border-color:black'></div>\
        <div style='left:300px;top:0;border-style:inset;border-color:white'></div>\
        <div style='left:0;top:100px;border-style:inset;border-color:rgb(255,0,0)'></div>\
        <div style='left:100px;top:100px;border-style:groove;border-color:rgb(0,128,0)'></div>\
        <div style='left:200px;top:100px;border-style:ridge;border-color:rgb(0,0,255)'></div>\
        <div style='left:300px;top:100px;border-style:outset;border-color:rgb(20,20,20)'></div>";
    let img = render(boxes, 1.0);
    // (box origin, top outer half, top inner half, bottom inner half): Chrome's colours.
    let chrome: [((f64, f64), [u8; 3], [u8; 3], [u8; 3]); 8] = [
        ((0.0, 0.0), [52, 52, 52], [52, 52, 52], [221, 221, 221]),
        ((100.0, 0.0), [221, 221, 221], [221, 221, 221], [52, 52, 52]),
        ((200.0, 0.0), [84, 84, 84], [84, 84, 84], [168, 168, 168]),
        ((300.0, 0.0), [171, 171, 171], [171, 171, 171], [255, 255, 255]),
        ((0.0, 100.0), [171, 0, 0], [171, 0, 0], [255, 0, 0]),
        // A groove's bottom inner half is shaded as an inset's top (the halves swap at the
        // bottom and right: their outer half is the inner edge's opposite).
        ((100.0, 100.0), [0, 44, 0], [0, 212, 0], [0, 44, 0]),
        ((200.0, 100.0), [0, 0, 255], [0, 0, 171], [0, 0, 255]),
        ((300.0, 100.0), [188, 188, 188], [188, 188, 188], [104, 104, 104]),
    ];
    for ((x, y), top_outer, top_inner, bottom_inner) in chrome {
        let at = |dx, dy| pixel(&img, 1.0, x + dx, y + dy);
        assert_near(at(40.0, 5.0), top_outer, &format!("box at {x},{y}: top, outer half"));
        assert_near(at(40.0, 15.0), top_inner, &format!("box at {x},{y}: top, inner half"));
        assert_near(at(40.0, 65.0), bottom_inner, &format!("box at {x},{y}: bottom, inner half"));
        assert_near(at(5.0, 40.0), top_outer, &format!("box at {x},{y}: left, outer half"));
    }
    // A groove's corner joins its sides' halves on the diagonal, the background not showing
    // through: Chrome's top-right outer corner pixel is the even mix of the dark top half and the
    // light right half (it was mixed with the white page).
    assert_near(pixel(&img, 1.0, 179.0, 100.0), [0, 128, 0], "groove top-right corner diagonal");
    assert_near(pixel(&img, 1.0, 279.0, 100.0), [0, 0, 213], "ridge top-right corner diagonal");
}

/// An `<img>` that fails to load (or has no source) and has `alt` text shows that text, as
/// browsers do (HTML rendering § images); an empty `alt` shows nothing. It showed nothing at all.
#[test]
fn a_broken_image_shows_its_alt_text() {
    let ink = |body: &str| {
        let img = render(body, 1.0);
        (0..60u32)
            .flat_map(|y| (0..300u32).map(move |x| (x, y)))
            .filter(|&(x, y)| pixel(&img, 1.0, f64::from(x), f64::from(y))[0] < 128)
            .count()
    };
    let style = "style='font:40px/1 serif;color:black'";
    assert!(ink(&format!("<img src='missing.png' alt='Broken' {style}>")) > 100, "failed load");
    assert!(ink(&format!("<img alt='Broken' {style}>")) > 100, "no source");
    assert_eq!(ink(&format!("<img src='missing.png' alt='' {style}>")), 0, "empty alt");
    let fallback = render("<img src='missing.png' alt='X' width=200 height=100 style='background:red'>", 1.0);
    assert_eq!(pixel(&fallback, 1.0, 150.0, 75.0), WHITE, "inline alt text ignores replaced dimensions");
}

/// A broken image draws Chromium's broken-image icon: before its alt text, and, given a size
/// without alt text, 2px inside a 1px rgb(192, 192, 192) border round its box (measured in
/// Chrome 2026-11-18). Neither was drawn.
#[test]
fn a_broken_image_draws_the_broken_image_icon() {
    let img = render(
        "<img src='missing.png' width=100 height=40 style='position:absolute;left:10px;top:10px'>\
         <img src='missing.png' alt='Text' style='position:absolute;left:10px;top:100px;font:16px serif'>",
        1.0,
    );
    let at = |x: f64, y: f64| pixel(&img, 1.0, x, y);
    assert_eq!(at(50.0, 10.0), [192, 192, 192], "the border's top");
    assert_eq!(at(10.0, 30.0), [192, 192, 192], "the border's left");
    assert_eq!(at(109.0, 30.0), [192, 192, 192], "the border's right");
    // The icon's grass (green) near its bottom left corner, in both.
    let green = |p: [u8; 3]| p[1] > 120 && p[0] < 150 && p[2] < 150;
    assert!((0..16).any(|d| green(at(12.0 + f64::from(d), 25.0))), "icon in the sized box");
    assert!((0..16).any(|d| green(at(10.0 + f64::from(d), 113.0))), "icon before the alt text");
}

/// `background-clip: border-area text` (either order) paints the union of the border area and
/// the text, not the whole box (WPT `clip-border-area-text.html`).
#[test]
fn border_area_and_text_clip_combine() {
    for clip in ["border-area text", "text border-area"] {
        let img = render(
            &format!("<div style='position:absolute;left:0;top:0;width:200px;height:100px;box-sizing:border-box;border:20px solid transparent;font:bold 40px/60px monospace;color:transparent;background:linear-gradient(blue,blue);background-clip:{clip}'>X</div>"),
            1.0,
        );
        assert_near(pixel(&img, 1.0, 10.0, 50.0), BLUE, &format!("{clip}: the border"));
        assert_near(pixel(&img, 1.0, 150.0, 50.0), WHITE, &format!("{clip}: the padding box past the text"));
    }
}
