//! `clip-path` on HTML boxes: its fill rule (`evenodd` cuts the overlap of a path's subpaths), and
//! `url(#id)` naming an SVG `<clipPath>` of the document (its shapes, user units from the border
//! box, `clipPathUnits="objectBoundingBox"`, CSS transforms, `clip-rule`); a reference to another
//! document or to no `<clipPath>` clips nothing.
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
        html: format!(
            "<!doctype html><style>body{{margin:0;background:#fff}} .box{{width:100px;height:100px;background:rgb(0,128,0)}}</style>{body}"
        ),
        base_url: "http://test/page.html".into(),
        width: 200,
        height: 120,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn green(img: &RgbaImage, x: u32, y: u32) -> bool {
    let i = (y as usize * img.width as usize + x as usize) * 4;
    img.data[i..i + 3] == [0, 128, 0]
}

/// Two overlapping squares: `evenodd` leaves out where they overlap, `nonzero` keeps it.
#[test]
fn a_clip_paths_fill_rule_applies() {
    const SQUARES: &str = "'M10,10 H70 V70 H10Z M40,40 H90 V90 H40Z'";
    let img = render(&format!("<div class=box style=\"clip-path: path(evenodd, {SQUARES})\"></div>"));
    assert!(green(&img, 20, 20) && green(&img, 80, 80), "outside the overlap");
    assert!(!green(&img, 55, 55), "the overlap is cut out");
    let img = render(&format!("<div class=box style=\"clip-path: path({SQUARES})\"></div>"));
    assert!(green(&img, 55, 55), "nonzero keeps it");
    let img = render("<div class=box style='clip-path: polygon(evenodd, 0 0, 100% 0, 100% 100%, 0 100%, 0 0, 50% 0, 50% 50%, 0 50%)'></div>");
    assert!(!green(&img, 25, 25) && green(&img, 75, 75), "an evenodd polygon");
}

/// `url(#id)`: the `<clipPath>`'s shapes, from the border box's origin; `clip-rule` per shape.
#[test]
fn a_clip_path_reference_clips_by_the_clip_paths_shapes() {
    let img = render(
        "<div class=box style='clip-path:url(#c)'></div><svg width=0 height=0><clipPath id=c>\
         <rect x=10 y=10 width=30 height=30 /><circle cx=70 cy=70 r=20 /></clipPath></svg>",
    );
    assert!(green(&img, 20, 20) && green(&img, 70, 70), "inside the rect and the circle");
    assert!(!green(&img, 5, 5) && !green(&img, 70, 20), "outside both");
    let img = render(
        "<div class=box style='clip-path:url(#c)'></div><svg width=0 height=0><clipPath id=c>\
         <path clip-rule=evenodd d='M10,10 H70 V70 H10Z M40,40 H90 V90 H40Z' /></clipPath></svg>",
    );
    assert!(green(&img, 20, 20) && !green(&img, 55, 55), "clip-rule evenodd");
}

/// `objectBoundingBox` units and a CSS transform set on the `<clipPath>`.
#[test]
fn clip_path_units_and_transforms() {
    let img = render(
        "<div class=box style='clip-path:url(#c)'></div><svg width=0 height=0>\
         <clipPath id=c clipPathUnits=objectBoundingBox><rect width=0.5 height=0.5 /></clipPath></svg>",
    );
    assert!(green(&img, 25, 25) && !green(&img, 75, 75), "the box's top left quarter");
    let img = render(
        "<div class=box style='clip-path:url(#c)'></div><svg width=0 height=0>\
         <clipPath id=c style='transform:scale(60)'><rect width=1 height=1 /></clipPath></svg>",
    );
    assert!(green(&img, 50, 50) && !green(&img, 70, 70), "a 1x1 rect scaled by 60");
}

/// No clipping for a reference into another document, to a missing element, or to something
/// other than a `<clipPath>`; an empty `<clipPath>` clips everything.
#[test]
fn references_that_clip_nothing_or_everything() {
    for target in ["url(other.svg#c)", "url(#missing)", "url(#notclip)"] {
        let img = render(&format!(
            "<div class=box style='clip-path:{target}'></div><svg width=0 height=0>\
             <clipPath id=c><rect width=10 height=10 /></clipPath><rect id=notclip width=10 height=10 /></svg>"
        ));
        assert!(green(&img, 50, 50) && green(&img, 95, 95), "{target}: not clipped");
    }
    let img = render("<div class=box style='clip-path:url(#c)'></div><svg width=0 height=0><clipPath id=c /></svg>");
    assert!(!green(&img, 50, 50), "an empty clipPath clips the box away");
}
