//! WPT round 4 lane D: word-by-word shaping in fonts without OpenType lookups (Times), glyph
//! origins and aligned lines on layout units, rounded corners of an inline box's fragments, and
//! the `background` shorthand on an SVG image's root.
use std::sync::Arc;

use pyro_render::*;

const GREEN: [u8; 3] = [0, 128, 0];

/// Serves `data:image/svg+xml,` URLs (percent-encoded SVG) on another thread; anything else fails.
struct DataUrls;
impl Fetcher for DataUrls {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        std::thread::spawn(move || match url.strip_prefix("data:image/svg+xml,") {
            Some(svg) => done(Ok(svg.replace("%20", " ").replace("%3C", "<").replace("%3E", ">").into_bytes())),
            None => done(Err("offline".into())),
        });
    }
}

/// `body` on a white page with 8px margins, 400x200 CSS px.
fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(DataUrls);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='background:#fff'>{body}"),
        base_url: "http://test/".into(),
        width: 400,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * img.width + x) * 4) as usize;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

fn differing(a: &RgbaImage, b: &RgbaImage) -> usize {
    a.data.chunks(4).zip(b.data.chunks(4)).filter(|(p, q)| p != q).count()
}

/// The macOS Times (legacy `kern` and `morx` tables only), which the shaping rule is about.
fn has_times() -> bool {
    std::path::Path::new("/System/Library/Fonts/Times.ttc").exists()
}

/// Times kerns a space against its neighbours in its `kern` table, but Blink shapes text in a
/// font whose OpenType lookups never take the space glyph word by word: text split into two table
/// cells at a no-break space draws as the same text on one line (WPT CSS2 `color-applies-to-*`).
#[test]
fn spaces_do_not_kern_in_fonts_without_lookups() {
    if !has_times() {
        return;
    }
    let style = "<style>body{font:16px Times}</style>";
    let cells = render(&format!(
        "{style}<div style='display:table'><div style='display:table-row'>\
         <div style='display:table-cell'>Filler</div><div style='display:table-cell'>&nbsp;Text</div></div></div>"
    ));
    let line = render(&format!("{style}<div>Filler Text</div>"));
    assert_eq!(differing(&cells, &line), 0, "cells against one line");
}

/// A glyph inside a run bins to the quarter pixel a box edge rounded up to a layout unit gives the
/// same glyph starting the next box: half-unit advances (1/128 px) no longer split them
/// (WPT CSS2 `display-008`).
#[test]
fn glyphs_in_a_run_bin_as_at_a_box_edge() {
    if !has_times() {
        return;
    }
    let style = "<style>body{font:16px Times}</style>";
    let cells = render(&format!(
        "{style}<div style='display:table'><div style='display:table-row'>\
         <div style='display:table-cell'>Filler Text</div><div style='display:table-cell'>Filler Text</div></div></div>"
    ));
    let line = render(&format!("{style}<div>Filler TextFiller Text</div>"));
    assert_eq!(differing(&cells, &line), 0, "cells against one line");
}

/// A right-aligned line starts where a right float of the same text does: its width is rounded
/// up to a layout unit, as Blink's text items are (WPT CSS2 `positioning-float-002`).
#[test]
fn right_aligned_text_matches_a_right_float() {
    if !has_times() {
        return;
    }
    let style = "<style>body{font:16px Times}</style>";
    let float = render(&format!("{style}<div><div style='float:right'>Floated element</div></div>"));
    let aligned = render(&format!("{style}<div style='text-align:right'>Floated element</div>"));
    assert_eq!(differing(&float, &aligned), 0, "float against aligned text");
}

/// The corners on an inline box's open sides are left out before overlapping radii are scaled
/// down, so each fragment keeps its full radii (WPT css-backgrounds `border-radius-012`).
#[test]
fn fragment_radii_scale_without_open_corners() {
    let sliced = render(
        "<style>.s{display:inline-block;width:20px}</style><div style='font-size:40px;line-height:100px'>\
         <span style='border:20px solid;border-radius:40px'><span class=s></span><br><span class=s></span></span></div>",
    );
    let pieces = render(
        "<style>.s{display:inline-block;width:20px}</style><div style='font-size:40px;line-height:100px'>\
         <span style='border:20px solid;border-radius:40px;border-right:none;border-top-right-radius:0;border-bottom-right-radius:0'>\
         <span class=s></span></span><br>\
         <span style='border:20px solid;border-radius:40px;border-left:none;border-top-left-radius:0;border-bottom-left-radius:0'>\
         <span class=s></span></span></div>",
    );
    assert_eq!(differing(&sliced, &pieces), 0, "sliced box against its pieces");
}

/// An SVG image's root `background` shorthand of a lone colour paints its canvas, as
/// `background-color` does (WPT css-backgrounds `background-image-007`).
#[test]
fn svg_root_background_shorthand() {
    let img = render(
        "<div style=\"width:100px;height:100px;background:red url(&quot;data:image/svg+xml,\
         <svg xmlns='http://www.w3.org/2000/svg' style='background: green'></svg>&quot;)\"></div>",
    );
    assert_eq!(pixel(&img, 58, 58), GREEN);
}

/// An SVG element with `display: contents` has no box, so its own `text-decoration` draws
/// nothing (WPT css-text-decor `text-decoration-propagation-display-contents`).
#[test]
fn display_contents_svg_text_has_no_decoration() {
    let svg = |tspan_style: &str| {
        render(&format!(
            "<svg width='300' height='60'><text y='30' style='font-size:20px'>\
             <tspan style='{tspan_style}'>Should have none</tspan></text></svg>"
        ))
    };
    let contents = svg("text-decoration: underline; display: contents");
    let plain = svg("");
    assert_eq!(differing(&contents, &plain), 0, "display: contents tspan against a plain one");
    assert_ne!(differing(&svg("text-decoration: underline"), &plain), 0, "an underline shows");
}
