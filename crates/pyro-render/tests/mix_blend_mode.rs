//! An element with a `mix-blend-mode` blends with what its stacking context painted, never with
//! what lies under that context: the context is an isolated group (Chromium isolates a stacking
//! context with blending descendants; the root element's group has a transparent backdrop too).
//! Without the group, the element blended with the page behind its context as well.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn render(html: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: html.into(),
        base_url: "http://test/".into(),
        width: 300,
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

/// WPT mix-blend-mode-overflowing-child: a `difference` box overflowing its stacking context's
/// box is its own colour where it overflows (over the context's transparent backdrop), black where
/// it covers the context's green box.
#[test]
fn a_blending_element_blends_inside_its_stacking_context_only() {
    let img = render(
        "<!doctype html><body style='margin:0;background:lightgray'>
         <div style='position:absolute;z-index:1;width:100px;height:100px;background:#0f0'>
         <div style='margin:50px;width:100px;height:100px;background:#0f0;mix-blend-mode:difference'></div></div>",
    );
    assert_eq!(pixel(&img, 75, 75), [0, 0, 0], "over the context's box");
    assert_eq!(pixel(&img, 125, 125), [0, 255, 0], "outside it (it blended with the page: magenta)");
    assert_eq!(pixel(&img, 250, 250), [211, 211, 211], "the page");
}

/// WPT mix-blend-mode-root-element-group: the root element's group has a transparent backdrop, so
/// a blending element directly in it renders as inside an isolated wrapper (it blended with the
/// white canvas and vanished).
#[test]
fn the_root_element_group_has_a_transparent_backdrop() {
    let page = |wrapper: &str| {
        render(&format!(
            "<!doctype html><style>div{{width:200px;height:200px}}.green{{background:rgba(0,160,0,.5);mix-blend-mode:soft-light}}
             .blue{{position:absolute;top:75px;left:75px;background:rgba(0,0,160,.5)}}</style>
             <body style='margin:0'><div style='{wrapper}'><div class=blue></div><div class=green></div></div>"
        ))
    };
    let test = page("width:auto;height:auto");
    let reference = page("width:auto;height:auto;isolation:isolate");
    assert!(test.data == reference.data, "{:?} vs {:?}", pixel(&test, 20, 20), pixel(&reference, 20, 20));
    assert_ne!(pixel(&test, 20, 20), [255; 3], "the green box is painted");
}
