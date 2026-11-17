//! Lane round-4 C: size containers make no containing block (container-queries
//! no-layout-containment-*), `display: contents` on SVG elements (css-display
//! display-contents-svg-elements), custom `@counter-style` systems for list markers, also over a
//! predefined name (css-shadow scoped-reference-animation-*).
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
        html: format!("<!doctype html><html><head><style>body{{margin:0;background:#fff}}</style></head><body>{body}</body></html>"),
        base_url: "http://test/".into(),
        width: 200,
        height: 120,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: usize, y: usize) -> [u8; 3] {
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

#[test]
fn a_size_container_is_no_containing_block_for_positioned_descendants() {
    let img = render(
        "<div style='position:relative;width:100px;height:100px;background:red'>\
           <div style='container-type:inline-size;height:0'>\
             <div style='position:absolute;inset:0;background:#0f0'></div></div></div>",
    );
    assert_eq!(pixel(&img, 50, 50), [0, 255, 0]);
    assert_eq!(pixel(&img, 50, 95), [0, 255, 0]);
}

#[test]
fn display_contents_unboxes_svg_groups_and_hides_other_svg_elements() {
    // The group's opacity goes with its box; a rect with `display: contents` renders nothing.
    let img = render(
        "<svg width=200 height=100>\
           <g style='display:contents;opacity:0'><rect width=50 height=50 fill='#0f0'/></g>\
           <rect x=100 width=50 height=50 fill='red' style='display:contents'/></svg>",
    );
    assert_eq!(pixel(&img, 25, 25), [0, 255, 0]);
    assert_eq!(pixel(&img, 125, 25), [255, 255, 255]);
}

#[test]
fn custom_counter_style_systems_format_list_markers() {
    let custom = render(
        "<style>@counter-style lower-roman { system: cyclic; symbols: 'X' }\
           @counter-style abc { system: alphabetic; symbols: A B C; suffix: ') ' }</style>\
         <ul style='list-style-type:lower-roman;font:20px monospace'><li>a</ul>\
         <ul style='list-style-type:abc;font:20px monospace'><li>b<li>c<li>d<li>e</ul>",
    );
    let strings = render(
        "<ul style='list-style-type:\"X. \";font:20px monospace'><li>a</ul>\
         <ul style='font:20px monospace;list-style-type:none'>\
           <li style='list-style-type:\"A) \"'>b<li style='list-style-type:\"B) \"'>c\
           <li style='list-style-type:\"C) \"'>d<li style='list-style-type:\"AA) \"'>e</ul>",
    );
    assert!(custom.data == strings.data, "custom counter styles draw as their strings");
}
