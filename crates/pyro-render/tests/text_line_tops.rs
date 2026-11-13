//! A line of text is painted from a whole CSS pixel at any device scale factor,
//! as Chromium paints it: the line's top in the document, in layout units of
//! 1/64px, rounded half up. A line height given as a number is the font size
//! times it, cut to a layout unit. A single-line input's text is at
//! `line-height: normal` below a fixed height, and a list marker's symbol is a
//! shape at whole pixels. Expected values measured on Chrome 156
//! (`bench/fidelity/repros/text-line-tops.html`).
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
        html: format!(
            "<!doctype html><style>body{{margin:0;background:white;color:black}}
             .a{{position:absolute;top:0;white-space:nowrap}}</style>{body}"
        ),
        base_url: "http://test/".into(),
        width: 300,
        height: 160,
        device_scale_factor: scale,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// The first device row of each run of rows with ink in the CSS columns `x0..x1`.
fn ink_tops(img: &RgbaImage, scale: f64, x0: f64, x1: f64) -> Vec<i64> {
    let (x0, x1) = ((x0 * scale) as usize, ((x1 * scale) as usize).min(img.width as usize));
    let mut tops = Vec::new();
    let mut inked = false;
    for y in 0..img.height as usize {
        let row = &img.data[(y * img.width as usize + x0) * 4..(y * img.width as usize + x1) * 4];
        let ink = row.chunks_exact(4).any(|p| p[0] < 128);
        if ink && !inked {
            tops.push(y as i64);
        }
        inked = ink;
    }
    tops
}

const SCALES: [f64; 3] = [1.0, 2.0, 3.0];

#[test]
fn lines_of_a_fractional_pitch_start_at_whole_css_pixels() {
    for scale in SCALES {
        // Tops at 0, 21.5, 43 and 64.5px: painted from 0, 22, 43 and 65px. In
        // device pixels the second and the fourth were at 21.5px and 64.5px at
        // a scale of 2, and a third of a pixel from there at 3.
        let img = render("<div class=a style='left:10px;font:11px/21.5px monospace'>H<br>H<br>H<br>H</div>", scale);
        let tops = ink_tops(&img, scale, 10.0, 40.0);
        assert_eq!(tops.len(), 4, "scale {scale}: {tops:?}");
        let from_first: Vec<_> = tops.iter().map(|top| (top - tops[0]) as f64 / scale).collect();
        assert_eq!(from_first, [0.0, 22.0, 43.0, 65.0], "scale {scale}: {tops:?}");
    }
}

#[test]
fn a_number_line_height_is_cut_to_a_layout_unit() {
    for scale in SCALES {
        // 19px times 1.43 is 27.17px, and 27.15625px (1738/64) cut to a layout
        // unit: the fourth line's top is at 81.47px and painted from 81px. At
        // 27.17px it was at 81.51px, and painted from 82px.
        let img = render("<div class=a style='left:10px;font:19px/1.43 monospace'>H<br>H<br>H<br>H</div>", scale);
        let tops = ink_tops(&img, scale, 10.0, 40.0);
        assert_eq!(tops.len(), 4, "scale {scale}: {tops:?}");
        let from_first: Vec<_> = tops.iter().map(|top| (top - tops[0]) as f64 / scale).collect();
        assert_eq!(from_first, [0.0, 27.0, 54.0, 81.0], "scale {scale}: {tops:?}");
    }
}

#[test]
fn the_text_of_a_vertically_centred_cell_starts_at_a_whole_css_pixel() {
    for scale in SCALES {
        // A 16px line in a cell of 41px is 12.5px down, and painted from 13px:
        // where the line of a 42px cell is.
        let cell = |left: u32, height: u32| {
            format!(
                "<table class=a style='left:{left}px;border-collapse:collapse;font:11px/16px monospace'>
                 <tr><td style='padding:0;height:{height}px;vertical-align:middle'>H</td></tr></table>"
            )
        };
        let img = render(&format!("{}{}", cell(10, 41), cell(60, 42)), scale);
        let (odd, even) = (ink_tops(&img, scale, 10.0, 40.0), ink_tops(&img, scale, 60.0, 90.0));
        assert_eq!(odd.len(), 1, "scale {scale}: {odd:?}");
        assert_eq!(odd, even, "scale {scale}");
    }
}

#[test]
fn an_inputs_value_starts_at_a_whole_css_pixel() {
    for scale in SCALES {
        // The line is centred in the input: half a pixel further down for each
        // pixel of height, and painted from a whole pixel.
        let inputs: String = (0..6)
            .map(|at| {
                format!(
                    "<input class=a style='left:{}px;width:30px;height:{}px;margin:0;padding:0;border:0;
                     font:11px monospace' value=H>",
                    10 + at * 40,
                    30 + at
                )
            })
            .collect();
        let img = render(&inputs, scale);
        let tops: Vec<i64> = (0..6)
            .map(|at| {
                let left = 10.0 + at as f64 * 40.0;
                let tops = ink_tops(&img, scale, left, left + 30.0);
                assert_eq!(tops.len(), 1, "scale {scale}: {tops:?}");
                tops[0]
            })
            .collect();
        let steps: Vec<i64> = tops.windows(2).map(|pair| pair[1] - pair[0]).collect();
        assert!(steps.iter().all(|step| *step == 0 || *step == scale as i64), "scale {scale}: {tops:?}");
        let down = (tops[5] - tops[0]) / scale as i64;
        assert!(down == 2 || down == 3, "scale {scale}: {tops:?}");

        // Centred in the box as laid out: half a pixel more height is a quarter
        // of a pixel further down, which rounds back to the line of the input
        // half a pixel shorter (Chromium). Centred in the box rounded to 33 or
        // 34px, one of the two was a pixel lower.
        let inputs: String = [32.0, 32.5, 33.0, 33.5]
            .iter()
            .enumerate()
            .map(|(at, height)| {
                format!(
                    "<input class=a style='left:{}px;width:30px;height:{height}px;margin:0;padding:0;border:0;
                     font:11px monospace' value=H>",
                    10 + at * 40
                )
            })
            .collect();
        let img = render(&inputs, scale);
        let tops: Vec<Vec<i64>> = (0..4).map(|at| ink_tops(&img, scale, 10.0 + at as f64 * 40.0, 40.0 + at as f64 * 40.0)).collect();
        assert_eq!((&tops[1], &tops[3]), (&tops[0], &tops[2]), "scale {scale}");
    }
}

#[test]
fn an_inputs_line_is_normal_below_a_fixed_height() {
    for scale in SCALES {
        // Chromium's inner editor drops a line height that the input's fixed
        // height is above, and one that is no more than the font size: the
        // text is where it is with `line-height: normal`. Measured in Chrome
        // 156: a 14px Lato line (17px) in a 33px input at 16px/1.375 is 8px
        // down as it is at `normal`, not 9px.
        let input = |left: u32, font: &str, height: &str| {
            format!(
                "<input class=a style='left:{left}px;width:30px;{height}margin:0;padding:0;border:0;font:{font}' value=H>"
            )
        };
        let top = |img: &RgbaImage, left: f64| {
            let tops = ink_tops(img, scale, left, left + 30.0);
            assert_eq!(tops.len(), 1, "scale {scale}: {tops:?}");
            tops[0]
        };
        for height in [33, 34] {
            let height = format!("height:{height}px;");
            let img = render(
                &[
                    input(10, "11px monospace", &height),
                    input(50, "11px/16px monospace", &height),
                    input(90, "11px/17px monospace", &height),
                    input(130, "11px/1.9 monospace", &height),
                    input(170, "11px/8px monospace", &height),
                ]
                .concat(),
                scale,
            );
            let normal = top(&img, 10.0);
            for left in [50.0, 90.0, 130.0, 170.0] {
                assert_eq!(top(&img, left), normal, "scale {scale}, height {height} left {left}");
            }
        }
        // Without a height the input is as tall as its line, which is `normal`
        // for a line height of no more than the font size.
        let img = render(
            &[
                input(10, "11px monospace", "background:#000;color:#000;"),
                input(50, "11px/8px monospace", "background:#000;color:#000;"),
                input(90, "11px/11.9px monospace", "background:#000;color:#000;"),
                input(130, "11px/30px monospace", "background:#000;color:#000;"),
                "<input class=a style='left:170px;width:30px;margin:0;padding:0;border:0;font:11px/8px monospace;
                 background:#000'>"
                    .to_string(),
            ]
            .concat(),
            scale,
        );
        let height = |left: f64| {
            let (x0, x1) = ((left * scale) as usize, ((left + 30.0) * scale) as usize);
            (0..img.height as usize)
                .filter(|y| {
                    let row = &img.data[(y * img.width as usize + x0) * 4..(y * img.width as usize + x1) * 4];
                    row.chunks_exact(4).any(|p| p[0] < 128)
                })
                .count() as f64
                / scale
        };
        assert_eq!(height(50.0), height(10.0), "scale {scale}");
        assert_eq!(height(90.0), height(10.0), "scale {scale}");
        assert_eq!(height(130.0), 30.0, "scale {scale}");
        assert_eq!(height(170.0), height(10.0), "scale {scale}: an empty input");
    }
}

/// The ink box of the CSS columns `x0..x1` in device pixels: left, top, right, bottom.
fn ink_box(img: &RgbaImage, scale: f64, x0: f64, x1: f64) -> (i64, i64, i64, i64) {
    let (x0, x1) = ((x0 * scale) as usize, ((x1 * scale) as usize).min(img.width as usize));
    let (mut left, mut top, mut right, mut bottom) = (i64::MAX, i64::MAX, 0, 0);
    for y in 0..img.height as usize {
        for x in x0..x1 {
            if img.data[(y * img.width as usize + x) * 4] < 128 {
                left = left.min(x as i64);
                top = top.min(y as i64);
                right = right.max(x as i64 + 1);
                bottom = bottom.max(y as i64 + 1);
            }
        }
    }
    (left, top, right, bottom)
}

#[test]
fn a_list_markers_symbol_is_a_shape_sized_from_the_font_at_whole_css_pixels() {
    // Chromium paints a `disc`, `circle` or `square` marker as a shape: with
    // the font's ascent `a` in whole pixels and `o = a * 2 / 3`, it is
    // `(o + 1) / 2` px wide and high and starts `o + 7` px before the item.
    // Measured in Chrome 156: 4px and 15px for 13px system-ui, 5px and 16px
    // for 14px Lato. It was a glyph of a bullet font.
    let list = |kind: &str, size: u32, item: &str| {
        format!(
            "<ul style='margin:0;padding:0 0 0 50px;font:{size}px/1.5 monospace;list-style:{kind}'><li>{item}</li></ul>"
        )
    };
    for scale in SCALES {
        let device = scale as i64;
        for size in [13, 16, 24] {
            let img = render(&list("square", size, "H"), scale);
            let square = ink_box(&img, scale, 0.0, 50.0);
            let (left, top, right, bottom) = square;
            assert!([left, top, right, bottom].iter().all(|edge| edge % device == 0), "scale {scale}: {square:?}");
            let (before, side) = (50 - left / device, (right - left) / device);
            assert_eq!((bottom - top) / device, side, "scale {scale}, {size}px: {square:?}");
            assert_eq!(side, (before - 6) / 2, "scale {scale}, {size}px: {square:?}");
            assert!((size as i64 / 4..=size as i64 / 2).contains(&side), "scale {scale}, {size}px: {square:?}");

            // The first line of a box in the item is the item's.
            let img = render(&list("square", size, "<div>H</div>"), scale);
            assert_eq!(ink_box(&img, scale, 0.0, 50.0), square, "scale {scale}, {size}px");

            // The size is the item's font's, whatever the text's.
            let img = render(&list("square", size, "<span style='font-size:40px'>H</span>"), scale);
            let (left, top, right, bottom) = ink_box(&img, scale, 0.0, 50.0);
            assert_eq!((left, right - left, bottom - top), (square.0, side * device, side * device), "scale {scale}, {size}px");

            // A disc fills the square's box, and a circle is a line around it.
            let img = render(&list("disc", size, "H"), scale);
            let disc = ink_box(&img, scale, 0.0, 50.0);
            let edges = |b: (i64, i64, i64, i64)| [b.0, b.1, b.2, b.3];
            let near = |a: (i64, i64, i64, i64), b: (i64, i64, i64, i64), by: i64| {
                edges(a).iter().zip(edges(b)).all(|(a, b)| (a - b).abs() <= by)
            };
            assert!(near(disc, square, 1), "scale {scale}, {size}px: {disc:?} {square:?}");
            let img = render(&list("circle", size, "H"), scale);
            let circle = ink_box(&img, scale, 0.0, 50.0);
            assert!(near(circle, square, device), "scale {scale}, {size}px: {circle:?} {square:?}");
        }
    }
}
