//! SVG filters on SVG content: a `<filter>`'s primitives and the filter functions, which usvg reads
//! and anyrender_vello_cpu applies as filter graphs. None was applied before.
use std::sync::Arc;

use pyro_render::*;

/// Serves its bytes for any URL (an SVG image's).
struct Serve(Vec<u8>);
impl Fetcher for Serve {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Ok(self.0.clone()));
    }
}

fn render(body: &str) -> RgbaImage {
    render_with(body, Vec::new())
}

fn render_with(body: &str, image: Vec<u8>) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(Serve(image));
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff'>{body}"),
        base_url: "http://test/".into(),
        width: 300,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: usize, y: usize) -> [u8; 3] {
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

/// `feFlood` fills the filter region (WPT svg-feflood-001); `feOffset` moves the source, and
/// `feMerge` puts the inputs over one another (WPT svg-feoffset-001).
#[test]
fn floods_offsets_and_merges() {
    let img = render(
        "<svg width='300' height='200' style='display:block'>
           <filter id='flood' x='0' y='0' width='1' height='1'><feFlood flood-color='#00f'/></filter>
           <filter id='double' x='0' y='0' width='2' height='1'>
             <feOffset dx='50' result='moved'/><feMerge><feMergeNode in='SourceGraphic'/><feMergeNode in='moved'/></feMerge>
           </filter>
           <rect width='50' height='50' fill='red' filter='url(#flood)'/>
           <rect y='100' width='50' height='50' fill='#0f0' filter='url(#double)'/>
         </svg>",
    );
    assert_eq!(pixel(&img, 25, 25), [0, 0, 255], "the flood over the rect");
    assert_eq!(pixel(&img, 75, 25), [255; 3], "nothing past the filter region");
    assert_eq!(pixel(&img, 25, 125), [0, 255, 0], "the source");
    assert_eq!(pixel(&img, 75, 125), [0, 255, 0], "its copy, moved");
    assert_eq!(pixel(&img, 125, 125), [255; 3], "nothing past the region");
}

/// Colour primitives in a mask's content apply (WPT svg-mutation-filter-used-by-mask, in its last
/// state): the red mask content hue-rotated, its green pushed to 1 and copied to every channel is
/// white, so the green rect shows.
#[test]
fn filters_apply_in_mask_content() {
    let img = render(
        "<svg width='300' height='200' style='display:block'>
           <filter id='f' color-interpolation-filters='sRGB'>
             <feColorMatrix type='hueRotate' values='90'/>
             <feComponentTransfer><feFuncG type='linear' slope='10000'/></feComponentTransfer>
             <feColorMatrix type='matrix' values='0 1 0 0 0, 0 1 0 0 0, 0 1 0 0 0, 0 0 0 1 0'/>
           </filter>
           <mask id='m'><rect width='100%' height='100%' fill='red' filter='url(#f)'/></mask>
           <rect width='100' height='100' fill='red'/>
           <rect width='100' height='100' fill='#008000' mask='url(#m)'/>
         </svg>",
    );
    assert_eq!(pixel(&img, 50, 50), [0, 128, 0]);
}

/// The root of an SVG image takes its `filter` (WPT svg-image-root-filter): red hue-rotated by
/// 90 degrees in sRGB, as the CSS function on a box (Chromium's matrix: green 0.356).
#[test]
fn an_svg_images_root_filter_applies() {
    let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='50' height='50' style='filter: hue-rotate(90deg)'>
               <rect width='20' height='20' fill='red'/></svg>";
    let img = render_with("<img style='display:block' src='image.svg'>", svg.as_bytes().to_vec());
    let [r, g, b] = pixel(&img, 10, 10);
    assert!(r <= 1 && (90..=92).contains(&g) && b <= 1, "{:?}", [r, g, b]);
}

/// A shape of no geometry renders nothing, its filter included (WPT
/// svg-empty-element-with-filter-001); an empty group's filter renders (WPT
/// mask-empty-container-with-filter).
#[test]
fn empty_shapes_render_no_filter_but_empty_groups_do() {
    let flood = "<filter id='f' filterUnits='userSpaceOnUse' x='0' y='0' width='100' height='100'><feFlood flood-color='red'/></filter>";
    for shape in [
        "<rect x='10' y='10' width='0' height='0' filter='url(#f)'/>",
        "<circle cx='40' cy='40' r='0' filter='url(#f)'/>",
        "<ellipse cx='50' cy='50' rx='0' ry='20' filter='url(#f)'/>",
        "<path d='' filter='url(#f)'/>",
        "<polygon points='' filter='url(#f)'/>",
    ] {
        let img = render(&format!("<svg width='300' height='200' style='display:block'>{flood}{shape}</svg>"));
        assert_eq!(pixel(&img, 50, 50), [255; 3], "{shape}");
    }
    let img =
        render(&format!("<svg width='300' height='200' style='display:block'>{flood}<g filter='url(#f)'/></svg>"));
    assert_eq!(pixel(&img, 50, 50), [255, 0, 0], "an empty group");
}

/// A filter with a primitive the renderer does not do (`feDropShadow` of two deviations), or one
/// usvg could not read (an `feImage` of an image it could not load, which it replaces with a
/// transparent flood), is left out: the content is drawn unfiltered, as before.
#[test]
fn unsupported_primitives_leave_the_filter_out() {
    for primitive in ["<feDropShadow stdDeviation='1 2'/>", "<feImage href='missing.png'/>"] {
        let img = render(&format!(
            "<svg width='300' height='200' style='display:block'><filter id='f'>{primitive}</filter>
             <rect width='100' height='100' fill='#0f0' filter='url(#f)'/></svg>"
        ));
        assert_eq!(pixel(&img, 50, 50), [0, 255, 0], "{primitive}");
    }
}

/// `color-interpolation-filters` is linearRGB by default: a table that inverts green, applied in
/// linearRGB, makes the green of `#008000` (0.216 linear) 0.784 linear, 228 in sRGB, not 127.
#[test]
fn primitives_work_in_linear_rgb_by_default() {
    let img = render(
        "<svg width='300' height='200' style='display:block'>
           <filter id='f'><feComponentTransfer><feFuncG type='table' tableValues='1 0'/></feComponentTransfer></filter>
           <rect width='100' height='100' fill='#008000' filter='url(#f)'/>
         </svg>",
    );
    let g = pixel(&img, 50, 50)[1];
    assert!((227..=229).contains(&g), "{g}");
}
