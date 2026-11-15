//! Background images and box edges as CSS Backgrounds 3 and Chromium paint them (WPT
//! `css/css-backgrounds`, `css/css-color`): the `round` step of `background-repeat` (the other
//! axis keeps the aspect ratio, the image is positioned at its rounded size), SVG images without
//! a size laid out in the size they are drawn at, SVG `<view>` fragments, Chromium's background
//! bleed avoidance under a rounded border, replaced content cut to the content edge's curve,
//! square border corners between colours painted by overdraw, the deprecated system colours and
//! `:visited` for links to the document itself.
use std::sync::Arc;

use pyro_render::*;

const BLUE: [u8; 3] = [0, 0, 255];
const GREEN: [u8; 3] = [0, 128, 0];
const RED: [u8; 3] = [255, 0, 0];
const WHITE: [u8; 3] = [255, 255, 255];

/// `n` by `n` pixels of `color`.
fn solid_png(n: u32, color: [u8; 3]) -> Vec<u8> {
    let data = (0..n * n).flat_map(|_| [color[0], color[1], color[2], 255]).collect();
    encode_png(&RgbaImage { width: n, height: n, data })
}

/// 40x40 pixels: the left half red, the right half green.
fn red_green_png() -> Vec<u8> {
    let data = (0..40 * 40)
        .flat_map(|i| {
            let c = if i % 40 < 20 { RED } else { GREEN };
            [c[0], c[1], c[2], 255]
        })
        .collect();
    encode_png(&RgbaImage { width: 40, height: 40, data })
}

/// No `width`, `height` or `viewBox`: a 20px blue stroke centred on the edges of the viewport.
const FRAME_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg"><rect width="100%" height="100%" fill="none" stroke="blue" stroke-width="20"/></svg>"#;

/// A sprite: the whole document is red with a green middle, and the view `v` shows its middle
/// half (x 4..12 of 16), which is green.
const SPRITE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg">
  <rect width="16" height="8" fill="red"/><rect x="4" width="8" height="8" fill="#008000"/>
  <view id="v" viewBox="4 0 8 8"/>
</svg>"##;

/// Serves the images above by name; anything else fails to load.
struct Images;
impl Fetcher for Images {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let name = url.rsplit('/').next().unwrap_or_default().split('#').next().unwrap_or_default().to_string();
        std::thread::spawn(move || {
            done(match name.as_str() {
                "blue.png" => Ok(solid_png(10, BLUE)),
                "red-green.png" => Ok(red_green_png()),
                "frame.svg" => Ok(FRAME_SVG.as_bytes().to_vec()),
                "sprite.svg" => Ok(SPRITE_SVG.as_bytes().to_vec()),
                _ => Err("404".into()),
            })
        });
    }
}

/// `body` on a white page without margins, 400x300 CSS px.
fn render(body: &str, scale: f64) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(Images);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff'>{body}"),
        base_url: "http://test/".into(),
        width: 400,
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

/// `round` in one axis with `background-size: auto` in the other scales the other axis too, so
/// the image keeps its aspect ratio (WPT `background-repeat-round-2`, `background-size-029`):
/// 40px tiles in a 100px box round to three of 33.3px, and so does their height. The height
/// stayed 40px.
#[test]
fn round_in_one_axis_keeps_the_aspect_ratio() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='width:100px;height:100px;background:url(blue.png) 0 0/40px auto;background-repeat:round no-repeat'></div>",
            scale,
        );
        assert_near(pixel(&img, scale, 5.0, 30.0), BLUE, &format!("inside the tile, scale {scale}"));
        assert_near(pixel(&img, scale, 5.0, 37.0), WHITE, &format!("below the rounded tile, scale {scale}"));
    }
}

/// A rounded image is positioned at its rounded size: 40px tiles rounded to 33.3px in a 100px
/// box, centred, start at 0 (the area less one tile, halved, is a whole tile), so each tile's red
/// half is at its left. Positioned at 40px they started at 30.
#[test]
fn a_rounded_image_is_positioned_at_its_rounded_size() {
    let img = render(
        "<div style='width:100px;height:40px;background:url(red-green.png) 50% 0/40px 40px;background-repeat:round no-repeat'></div>",
        1.0,
    );
    assert_near(pixel(&img, 1.0, 2.0, 20.0), RED, "the first tile's left half");
    assert_near(pixel(&img, 1.0, 31.0, 20.0), GREEN, "the first tile's right half");
    assert_near(pixel(&img, 1.0, 35.0, 20.0), RED, "the second tile's left half");
}

/// An SVG image without `width`, `height` or `viewBox` is laid out in the size it is drawn at
/// (WPT `background-size-043`/`-044`, `diagonal-percentage-vector-background`): its `100%` rect
/// is the background's 200x100 and the 20px stroke shows 10px inside every edge. It was laid out
/// at usvg's 100x100 and stretched: the left and right bands were 20px wide. The stroke's outer
/// half, outside the image, is cut off.
#[test]
fn an_svg_background_without_a_size_is_laid_out_at_the_background_size() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='width:300px;height:150px;background:url(frame.svg) no-repeat 0 0;background-size:200px 100px'></div>",
            scale,
        );
        assert_near(pixel(&img, scale, 205.0, 50.0), WHITE, &format!("right of the image, scale {scale}"));
        assert_near(pixel(&img, scale, 100.0, 105.0), WHITE, &format!("below the image, scale {scale}"));
        assert_near(pixel(&img, scale, 5.0, 50.0), BLUE, &format!("the left band, scale {scale}"));
        assert_near(pixel(&img, scale, 15.0, 50.0), WHITE, &format!("past the 10px band, scale {scale}"));
        assert_near(pixel(&img, scale, 100.0, 5.0), BLUE, &format!("the top band, scale {scale}"));
        assert_near(pixel(&img, scale, 100.0, 15.0), WHITE, &format!("below the top band, scale {scale}"));
    }
}

/// A fragment naming a `<view>` shows the image through the view's `viewBox`, which gives it
/// that aspect ratio (WPT `background-size-contain-svg-view`, `-cover-svg-view`): the square
/// view of the sprite's green middle fills the 100x100 box. The fragment was ignored: the
/// whole 16x8 sprite (red sides) was drawn.
#[test]
fn an_svg_view_fragment_shows_the_view() {
    let img = render(
        "<div style='width:100px;height:100px;background:url(sprite.svg#v) no-repeat;background-size:contain'></div>",
        1.0,
    );
    for (x, y) in [(5.0, 5.0), (95.0, 50.0), (50.0, 95.0)] {
        assert_near(pixel(&img, 1.0, x, y), GREEN, &format!("({x}, {y})"));
    }
}

/// Under a rounded border that hides its edge (opaque, solid), the background is cut to the
/// border box inset by half the border widths, as Chromium does
/// (`kBackgroundBleedShrinkBackground`): around a white border on a white page no dark seam is
/// left where the black background showed through the border's antialiased outer edge.
#[test]
fn a_background_does_not_bleed_through_a_rounded_border() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='position:absolute;left:20px;top:20px;width:80px;height:80px;border:10px solid #fff;border-radius:50px;background:#000'></div>",
            scale,
        );
        // Every pixel of the top-left corner outside the padding edge's curve (radius 40 around
        // (70, 70)), with a pixel of margin, is white.
        let mut darkest = 255;
        let n = (50.0 * scale) as usize;
        for py in 0..n {
            for px in 0..n {
                let (x, y) = (20.0 + (px as f64 + 0.5) / scale, 20.0 + (py as f64 + 0.5) / scale);
                if ((x - 70.0).powi(2) + (y - 70.0).powi(2)).sqrt() < 40.0 + 1.5 {
                    continue;
                }
                let p = pixel(&img, scale, x, y);
                darkest = darkest.min(p[0]);
            }
        }
        assert!(darkest >= 253, "a seam {darkest} levels dark, scale {scale}");
    }
}

/// Under a side narrower than two device pixels the background keeps to the outer edge (the
/// background's inset edge and the border's two antialiased edges would share pixels, and what
/// is under the box showed through between them: WPT `filter-effects/blur-clip-border-radius`):
/// a 1px rounded border leaves the background's antialiased edge outside it at scale 1. At
/// scale 2 the same border is two device pixels wide, and the background is cut as under any
/// other rounded border (a 1px Tailwind `rounded border` on a DSF 2 screen).
#[test]
fn a_one_pixel_rounded_border_shrinks_the_background_from_two_device_pixels() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='position:absolute;left:20px;top:20px;width:80px;height:80px;border:1px solid #fff;border-radius:20px;background:#000'></div>",
            scale,
        );
        // The pixels of the top-left corner outside the border's outer curve (radius 20
        // around (40, 40)).
        let mut darkest = 255;
        let n = (20.0 * scale) as usize;
        for py in 0..n {
            for px in 0..n {
                let (x, y) = (20.0 + (px as f64 + 0.5) / scale, 20.0 + (py as f64 + 0.5) / scale);
                if ((x - 40.0).powi(2) + (y - 40.0).powi(2)).sqrt() <= 20.0 {
                    continue;
                }
                darkest = darkest.min(pixel(&img, scale, x, y)[0]);
            }
        }
        if scale == 1.0 {
            assert!(darkest < 240, "the background reaches the outer edge at scale 1 ({darkest})");
        } else {
            assert!(darkest >= 253, "a seam {darkest} levels dark, scale {scale}");
        }
    }
}

/// Replaced content is cut to the content edge's curve (WPT `border-radius-clip-001`): with a
/// border and padding inside a 50% radius, the image's corners are outside the content circle
/// (radius 50 around (80, 80)) though inside the padding box's (radius 60). The image was cut to
/// the padding box only.
#[test]
fn an_image_is_cut_to_its_content_edge_curve() {
    let img = render(
        "<img src=blue.png style='position:absolute;left:0;top:0;width:100px;height:100px;padding:10px;border:20px solid transparent;border-radius:50%'>",
        1.0,
    );
    assert_near(pixel(&img, 1.0, 80.0, 80.0), BLUE, "the middle");
    assert_near(pixel(&img, 1.0, 38.0, 38.0), WHITE, "outside the content curve");
}

/// The deprecated system colours `ThreeDHighlight` and `ThreeDLightShadow` are `ButtonBorder`
/// (CSS Color 4 §6.2; WPT `deprecated-sameas-018`/`-019`). They were `ButtonFace`.
#[test]
fn deprecated_three_d_colours_are_button_border() {
    let img = render(
        "<div style='width:40px;height:20px;background:ButtonBorder'></div><div style='width:40px;height:20px;background:ThreeDHighlight'></div><div style='width:40px;height:20px;background:ThreeDLightShadow'></div>",
        1.0,
    );
    let border = pixel(&img, 1.0, 10.0, 10.0);
    assert_ne!(border, WHITE);
    assert_near(pixel(&img, 1.0, 10.0, 30.0), border, "ThreeDHighlight");
    assert_near(pixel(&img, 1.0, 10.0, 50.0), border, "ThreeDLightShadow");
}

/// Square corners between opaque solid sides of different colours are painted by overdraw, as
/// Blink paints them (WPT `CSS2/border-seams-001`): the right side's antialiased diagonal lies
/// over the top and bottom sides, painted first through the corners, so the red background does
/// not show through the seams. As two trapezoids antialiased over the background it did.
#[test]
fn border_corners_between_colours_do_not_let_the_background_through() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='position:absolute;left:10px;top:10px;width:100px;height:60px;border:30px solid #fefefe;border-right-color:#fdfdfd;background:red'></div>",
            scale,
        );
        let mut reddest = 0;
        for py in 0..(140.0 * scale) as usize {
            for px in 0..(170.0 * scale) as usize {
                let (x, y) = (10.0 + (px as f64 + 0.5) / scale, 10.0 + (py as f64 + 0.5) / scale);
                // The border only: outside the padding box.
                if (40.0..140.0).contains(&x) && (40.0..100.0).contains(&y) {
                    continue;
                }
                let p = pixel(&img, scale, x, y);
                reddest = reddest.max(i32::from(p[0]) - i32::from(p[1]));
            }
        }
        assert!(reddest <= 2, "red {reddest} levels through the seams, scale {scale}");
    }
}

/// A link to the document itself is visited, as the page a browser shows is in its history (WPT
/// `css-color/currentcolor-004`, `color-mix-currentcolor-visited`, `href=""`): `:visited` rules
/// apply to it, after `:link` rules, which still match it as they match every link in
/// Chromium's regular style. Other links stay unvisited. Inside `:has()` every link is
/// unvisited, as in Chromium (WPT `css/selectors/has-visited.html`,
/// `invalidation/link-pseudo-in-has.html`). No link was visited.
#[test]
fn a_link_to_the_document_itself_is_visited() {
    let img = render(
        "<style>a{display:block;width:40px;height:20px;background:currentColor}a:link{color:red}a:visited{color:green}</style><a href=''></a><a href='other.html'></a>",
        1.0,
    );
    assert_near(pixel(&img, 1.0, 10.0, 10.0), GREEN, "href=\"\"");
    assert_near(pixel(&img, 1.0, 10.0, 30.0), RED, "another page");

    // A `:link` rule that is not about colour still applies to the visited link.
    let img = render(
        "<style>a{display:block;width:40px;height:20px;background:red}a:link{background:green}</style><a href=''></a>",
        1.0,
    );
    assert_near(pixel(&img, 1.0, 10.0, 10.0), GREEN, "a:link on href=\"\"");

    // `:has(> :link)` matches around it, `:has(> :visited)` does not.
    let img = render(
        "<style>p{margin:0;width:40px;height:20px}.l:has(> :link){background:green}.v:has(> :visited){background:red}</style><p class=l><a href=''></a></p><p class=v><a href=''></a></p>",
        1.0,
    );
    assert_near(pixel(&img, 1.0, 10.0, 10.0), GREEN, ":has(> :link)");
    assert_near(pixel(&img, 1.0, 10.0, 30.0), WHITE, ":has(> :visited)");
}
