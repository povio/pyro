//! Dotted and dashed borders and outlines have Chromium's marks: the dash and
//! dot counts Blink fits to a side, sides that overlap in their corners, square
//! dots up to 3px, and on a rounded box one pattern around the whole box. An
//! outline is drawn as a border of its style around the border box moved out by
//! `outline-offset`, over the box. Expected values measured on Chrome 156
//! (`bench/fidelity/repros/dotted-dashed-lines.html`).
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

/// One 120x40 box at (20, 20).
fn render(style: &str, scale: f64) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><style>body{{margin:0;background:white}}
             div{{position:absolute;left:20px;top:20px;width:120px;height:40px;box-sizing:border-box;{style}}}</style><div></div>"
        ),
        base_url: "http://test/".into(),
        width: 200,
        height: 100,
        device_scale_factor: scale,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let i = (y as usize * img.width as usize + x as usize) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

fn dark(img: &RgbaImage, x: u32, y: u32) -> bool {
    pixel(img, x, y)[0] < 128
}

/// The dark runs along a row (or a column) of CSS px `at`, as (start, end) in
/// CSS px from `origin`, sampled in the middle of the device px.
fn runs(img: &RgbaImage, scale: f64, horizontal: bool, at: f64, origin: f64) -> Vec<(f64, f64)> {
    let fixed = (at * scale) as u32;
    let len = if horizontal { img.width } else { img.height };
    let mut runs = Vec::new();
    let mut start = None;
    for i in 0..=len {
        let on = i < len && if horizontal { dark(img, i, fixed) } else { dark(img, fixed, i) };
        match (on, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                runs.push((s as f64 / scale - origin, i as f64 / scale - origin));
                start = None;
            }
            _ => {}
        }
    }
    runs
}

/// Whether `runs` are the `expected` ones, to a device px.
fn assert_runs(runs: &[(f64, f64)], expected: &[(f64, f64)], scale: f64, what: &str) {
    let tolerance = 1.0 / scale + 0.01;
    let same = runs.len() == expected.len()
        && runs.iter().zip(expected).all(|(a, b)| (a.0 - b.0).abs() <= tolerance && (a.1 - b.1).abs() <= tolerance);
    assert!(same, "{what} at scale {scale}: {runs:?}, expected {expected:?}");
}

const SCALES: [f64; 3] = [1.0, 2.0, 3.0];

#[test]
fn a_dashed_border_has_the_dashes_blink_fits_to_each_side() {
    for scale in SCALES {
        let img = render("border:2px dashed black", scale);
        // 6px dashes (three times the width under 3px): twelve in 120px with
        // gaps of 48/11, four in 40px with gaps of 16/3. A side starts and ends
        // with a whole dash, in the corner, where the next side's dash lies
        // over it.
        let top: Vec<_> = (0..12).map(|i| (i as f64 * (6.0 + 48.0 / 11.0), i as f64 * (6.0 + 48.0 / 11.0) + 6.0)).collect();
        let left: Vec<_> = (0..4).map(|i| (i as f64 * (6.0 + 16.0 / 3.0), i as f64 * (6.0 + 16.0 / 3.0) + 6.0)).collect();
        assert_runs(&runs(&img, scale, true, 21.0, 20.0), &top, scale, "top");
        assert_runs(&runs(&img, scale, true, 59.0, 20.0), &top, scale, "bottom");
        assert_runs(&runs(&img, scale, false, 21.0, 20.0), &left, scale, "left");
        assert_runs(&runs(&img, scale, false, 139.0, 20.0), &left, scale, "right");

        // From 3px a dash is twice the width: 8px, ten in 120px, gaps of 40/9.
        let img = render("border:4px dashed black", scale);
        let top: Vec<_> = (0..10).map(|i| (i as f64 * (8.0 + 40.0 / 9.0), i as f64 * (8.0 + 40.0 / 9.0) + 8.0)).collect();
        assert_runs(&runs(&img, scale, true, 22.0, 20.0), &top, scale, "top, 4px");
    }
}

#[test]
fn a_dotted_border_up_to_3px_has_square_dots_with_one_at_each_end() {
    for scale in SCALES {
        let img = render("border:2px dotted black", scale);
        // Dots and gaps of 2px do not end with a dot in 120px: Blink draws the
        // first and the last dot itself and starts the pattern a pixel early.
        let mut top = vec![(0.0, 2.0)];
        top.extend((0..29).map(|i| (3.0 + i as f64 * 4.0, 5.0 + i as f64 * 4.0)));
        top.push((118.0, 120.0));
        assert_runs(&runs(&img, scale, true, 21.0, 20.0), &top, scale, "top");
        // Square: the corner of a dot is as dark as its middle.
        let corner = ((20.0 + 7.0) * scale) as u32;
        assert!(dark(&img, corner, (20.0 * scale) as u32), "a dot's corner at scale {scale}");

        // 1px: every other pixel, the first two joined on an even length.
        let img = render("border:1px dotted black", scale);
        let mut top = vec![(0.0, 2.0)];
        top.extend((0..58).map(|i| (3.0 + i as f64 * 2.0, 4.0 + i as f64 * 2.0)));
        top.push((119.0, 120.0));
        assert_runs(&runs(&img, scale, true, 20.5, 20.0), &top, scale, "top, 1px");
    }
}

#[test]
fn a_dotted_border_over_3px_has_round_dots_that_touch_the_corners() {
    for scale in SCALES {
        let img = render("border:6px dotted black", scale);
        // Eleven dots in 120px, the first and the last in the corners, 11.39px
        // apart (the gap nearest the width that fits, less Blink's epsilon).
        let top: Vec<_> = (0..11).map(|i| (i as f64 * 11.39, i as f64 * 11.39 + 6.0)).collect();
        assert_runs(&runs(&img, scale, true, 23.0, 20.0), &top, scale, "top");
        let left: Vec<_> = (0..4).map(|i| (i as f64 * 11.323, i as f64 * 11.323 + 6.0)).collect();
        assert_runs(&runs(&img, scale, false, 23.0, 20.0), &left, scale, "left");
        // Round: the corner of the box is outside the dot there.
        assert!(pixel(&img, (20.0 * scale) as u32, (20.0 * scale) as u32)[0] > 240, "scale {scale}");
    }
}

#[test]
fn a_rounded_dashed_border_is_one_pattern_around_the_box() {
    for scale in SCALES {
        let img = render("border:4px dashed black;border-radius:10px", scale);
        // The line runs 2px inside the outer edge and starts after the top left
        // corner, 10px in. Blink takes it to be 289px long: 24 dashes of 8px,
        // 12.04px apart. Skia measures it a pixel longer, so that a 25th dash
        // starts a pixel before the first. Eight dashes on the straight part.
        let top = runs(&img, scale, true, 22.0, 20.0);
        let mut expected = vec![(9.0, 18.0)];
        expected.extend((1..8).map(|i| (10.0 + i as f64 * 12.0417, 18.0 + i as f64 * 12.0417)));
        let on_side: Vec<_> = top.iter().copied().filter(|run| run.0 > 8.0 && run.1 < 104.0).collect();
        assert_runs(&on_side, &expected, scale, "top");
        // And it comes around to the left side still in step, three sides and
        // three corners later.
        let left = runs(&img, scale, false, 22.0, 20.0);
        let on_side: Vec<_> = left.iter().copied().filter(|run| run.0 > 12.0 && run.1 < 25.0).collect();
        assert_runs(&on_side, &[(14.67, 22.5)], scale, "left");
    }
}

#[test]
fn a_dashed_or_dotted_outline_is_drawn_as_a_border_around_the_offset_box() {
    for scale in SCALES {
        // 2px outside the border box, 3px away: a 130x50 box of 2px dashes.
        let img = render("outline:2px dashed black;outline-offset:3px;background:red", scale);
        let top = runs(&img, scale, true, 16.0, 15.0);
        assert_eq!(top.len(), 13, "scale {scale}: {top:?}");
        assert_runs(&[top[0], top[12]], &[(0.0, 6.0), (124.0, 130.0)], scale, "outline top");
        let left = runs(&img, scale, false, 16.0, 15.0);
        assert_eq!(left.len(), 5, "scale {scale}: {left:?}");
        // Nothing where the solid outline was, on the border box's edge.
        assert!(runs(&img, scale, true, 19.0, 15.0).len() > 1, "scale {scale}");
        assert_eq!(pixel(&img, (80.0 * scale) as u32, (18.5 * scale) as u32), [255, 255, 255], "scale {scale}");

        // Round dots from 4px.
        let img = render("outline:6px dotted black", scale);
        let top: Vec<_> = (0..12).map(|i| (i as f64 * 11.445, i as f64 * 11.445 + 6.0)).collect();
        assert_runs(&runs(&img, scale, true, 17.0, 14.0), &top, scale, "dotted outline top");
    }
}

#[test]
fn a_solid_outline_follows_its_offset_and_is_drawn_over_the_box() {
    for scale in SCALES {
        let at = |img: &RgbaImage, x: f64, y: f64| pixel(img, (x * scale) as u32, (y * scale) as u32);
        // Out by the offset: the outline is 4px to 6px from the box.
        let img = render("outline:2px solid blue;outline-offset:4px;background:red", scale);
        assert_eq!(at(&img, 15.0, 40.0), [0, 0, 255], "scale {scale}");
        assert_eq!(at(&img, 17.5, 40.0), [255, 255, 255], "scale {scale}");
        assert_eq!(at(&img, 19.0, 40.0), [255, 255, 255], "scale {scale}");
        assert_eq!(at(&img, 13.5, 40.0), [255, 255, 255], "scale {scale}");
        // In by the offset, over the background.
        let img = render("outline:2px solid blue;outline-offset:-4px;background:red", scale);
        assert_eq!(at(&img, 21.0, 40.0), [255, 0, 0], "scale {scale}");
        assert_eq!(at(&img, 23.0, 40.0), [0, 0, 255], "scale {scale}");
        assert_eq!(at(&img, 26.5, 40.0), [255, 0, 0], "scale {scale}");
        assert_eq!(at(&img, 19.0, 40.0), [255, 255, 255], "scale {scale}");
        // A rounded box's outline is rounded by the radius and its distance: no
        // ink in the corner of its box, ink on the arc.
        let img = render("outline:2px solid blue;outline-offset:2px;border-radius:8px", scale);
        assert_eq!(at(&img, 16.5, 16.5), [255, 255, 255], "scale {scale}");
        let on_arc = at(&img, 20.1, 20.1);
        assert!(on_arc[0] < 32 && on_arc[2] == 255, "scale {scale}: {on_arc:?}");
        // With the element's opacity.
        let img = render("outline:4px solid black;opacity:.5", scale);
        let grey = at(&img, 18.0, 40.0)[0];
        assert!((126..=129).contains(&grey), "scale {scale}: {grey}");
    }
}
