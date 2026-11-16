//! `stretch` beside floats (a block-level replaced element's `width: stretch`, `max-width:
//! stretch` and `min-width: stretch` are the float slot's width), `stretch` sizes of absolutely
//! positioned boxes with auto insets (from the static position), and a replaced element's
//! `aspect-ratio: <ratio>` between `box-sizing` boxes (WPT css-sizing `stretch/*`,
//! `aspect-ratio/replaced-element-031..033`).
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
        width: 400,
        height: 300,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

const GREEN: [u8; 3] = [0, 128, 0];

/// The width and height of the green area that starts at `(x, y)`.
fn green_size_at(img: &RgbaImage, x0: usize, y0: usize) -> (usize, usize) {
    let at = |x: usize, y: usize| {
        let i = (y * img.width as usize + x) * 4;
        [img.data[i], img.data[i + 1], img.data[i + 2]] == GREEN
    };
    ((x0..img.width as usize).take_while(|&x| at(x, y0)).count(), (y0..img.height as usize).take_while(|&y| at(x0, y)).count())
}

#[track_caller]
fn check_at(body: &str, at: (usize, usize), want: (usize, usize)) {
    assert_eq!(green_size_at(&render(body), at.0, at.1), want, "{body}");
}

#[test]
fn replaced_stretch_widths_fit_the_float_slot() {
    let float = "<div style='float:left;width:100px;height:100px'></div>";
    for style in ["width:stretch", "width:1000px;max-width:stretch", "width:0;min-width:stretch"] {
        check_at(
            &format!(
                "<div style='width:200px'>{float}<canvas width=10 height=10 \
                 style='display:block;height:50px;background:green;{style}'></canvas></div>"
            ),
            (100, 0),
            (100, 50),
        );
    }
    // A block that establishes a formatting context, `max-width: stretch`
    check_at(
        &format!("<div style='width:200px'>{float}<div style='display:flow-root;height:50px;background:green;width:1000px;max-width:stretch'></div></div>"),
        (100, 0),
        (100, 50),
    );
}

#[test]
fn absolute_stretch_starts_at_the_static_position() {
    check_at(
        "<div style='position:relative;width:150px;height:150px'><div style='padding:50px 0 0 50px'>\
         <div style='position:absolute;width:stretch;height:stretch;background:green'></div></div></div>",
        (50, 50),
        (100, 100),
    );
}

#[test]
fn a_replaced_ratio_is_between_box_sizing_boxes() {
    // border-box: the 100px border box is 10px wide; content-box: the content box is
    check_at(
        "<canvas width=1 height=1 style='display:block;border-top:40px solid green;background:green;\
         height:100px;aspect-ratio:1/10;box-sizing:border-box'></canvas>",
        (0, 0),
        (10, 100),
    );
    check_at(
        "<canvas width=1 height=1 style='display:block;border-top:40px solid green;background:green;\
         height:60px;aspect-ratio:1/3;box-sizing:content-box'></canvas>",
        (0, 0),
        (20, 100),
    );
}
