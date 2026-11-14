//! SVG filter primitives besides the colour, blur, offset and compositing ones (`feDisplacementMap`
//! with the taint rule, `feTile`, `feMorphology`, `feConvolveMatrix`, `feTurbulence`, the lighting
//! primitives, `feImage`), and SVG filters on HTML elements: before, a filter with one of these
//! primitives was left out, and `filter: url()` on an HTML element applied colour chains only.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("no network".into()));
    }
}

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
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

const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 128, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const WHITE: [u8; 3] = [255; 3];

/// The source of the WPT tainting-* pages: a gradient, red then green, over a blue box; a map of
/// green 1, blue 0.5 at a scale of 100 moves each pixel 50 px left: green, then the blue box.
fn displaced(primitives: &str) -> RgbaImage {
    render(&format!(
        "<svg width='300' height='150' style='display:block'>
           <linearGradient id='g'><stop stop-color='red'/><stop stop-color='red' offset='.5'/>
             <stop stop-color='green' offset='.5'/><stop stop-color='green' offset='1'/></linearGradient>
           <filter id='f' color-interpolation-filters='sRGB' filterUnits='userSpaceOnUse'>{primitives}
             <feDisplacementMap in='SourceGraphic' xChannelSelector='G' yChannelSelector='B' scale='100'/>
           </filter>
           <rect width='100' height='100' fill='blue'/>
           <rect width='100' height='100' fill='url(#g)' filter='url(#f)'/>
         </svg>"
    ))
}

/// WPT tainting-feflood-001, -feoffset-001, -fetile-001, -femorphology-001, -feconvolvematrix-001,
/// -fediffuselighting-001: a map through each primitive moves the source.
#[test]
fn an_untainted_map_displaces() {
    for primitives in [
        "<feFlood flood-color='rgb(0%, 100%, 50%)'/>",
        "<feFlood flood-color='rgb(0%, 100%, 50%)'/><feOffset/>",
        "<feFlood flood-color='rgb(0%, 100%, 50%)'/><feTile/>",
        "<feFlood flood-color='rgb(0%, 100%, 50%)'/><feMorphology radius='0.5'/>",
        "<feFlood flood-color='rgb(0%, 100%, 50%)'/><feConvolveMatrix order='1' kernelMatrix='1'/>",
        "<feFlood/><feDiffuseLighting lighting-color='rgb(0%, 100%, 50%)'><feDistantLight elevation='90'/></feDiffuseLighting>",
        "<feFlood/><feSpecularLighting lighting-color='rgb(0%, 100%, 50%)'><feDistantLight elevation='90'/></feSpecularLighting>",
        "<feFlood x='0' y='0' width='100' height='100'/>
         <feDropShadow width='100%' flood-color='rgb(0%, 100%, 50%)' stdDeviation='0' dx='100' dy='0'/>
         <feOffset dx='-100'/>",
    ] {
        let img = displaced(primitives);
        assert_eq!(pixel(&img, 25, 50), GREEN, "{primitives}");
        assert_eq!(pixel(&img, 75, 50), BLUE, "{primitives}");
    }
}

/// WPT tainting-feflood-002 and others: a map whose colour is `currentcolor` (a `:visited` link's
/// colour may be) is tainted, and a displacement through a tainted map passes its input on unmoved.
#[test]
fn a_tainted_map_displaces_nothing() {
    for primitives in [
        "<feFlood flood-color='currentcolor' style='color: rgb(0%, 100%, 50%)'/>",
        "<feFlood style='flood-color: color-mix(in srgb, currentcolor 99.9%, black); color: rgb(0%, 100%, 50%)'/>",
        "<feFlood flood-color='currentcolor' style='color: rgb(0%, 100%, 50%)'/><feTile/>",
        "<feFlood/><feDiffuseLighting lighting-color='currentcolor' style='color: rgb(0%, 100%, 50%)'>
           <feDistantLight elevation='90'/></feDiffuseLighting>",
    ] {
        let img = displaced(primitives);
        assert_eq!(pixel(&img, 25, 50), RED, "{primitives}");
        assert_eq!(pixel(&img, 75, 50), GREEN, "{primitives}");
    }
}

/// What a filter makes past an `<svg>`'s box is cut off there (its `overflow: hidden`): a flood
/// whose region starts 50 px left of the viewport, in a box 20 px in (WPT tainting-*: their
/// displacements reach past the box by 10%).
#[test]
fn filter_output_stops_at_the_svg_box() {
    let img = render(
        "<div style='padding-left:20px'><svg width='300' height='150' style='display:block'>
           <filter id='f' color-interpolation-filters='sRGB' filterUnits='userSpaceOnUse' x='-50' y='0' width='150' height='100'>
             <feFlood flood-color='green'/>
           </filter>
           <rect width='100' height='100' fill='red' filter='url(#f)'/>
         </svg></div>",
    );
    assert_eq!(pixel(&img, 10, 50), WHITE, "left of the box");
    assert_eq!(pixel(&img, 25, 50), GREEN, "in it");
    // An `<svg>` without a declared size is Chromium's 300 x 150 viewport for the cut (its box
    // here is its content's, a hidden text's): the flood fills its 100 x 100 region (WPT
    // svg-visibility-hidden-element-with-filter-003).
    let img = render(
        "<svg style='display:block'>
           <filter id='f' filterUnits='userSpaceOnUse' x='0' y='0' width='100' height='100'>
             <feFlood flood-color='green'/></filter>
           <text fill='red' x='20' y='20' visibility='hidden' filter='url(#f)'>SVGText</text>
         </svg>",
    );
    assert_eq!(pixel(&img, 95, 95), GREEN);
}

/// `feTile`, `feMorphology`, `feConvolveMatrix` (a zero divisor is the kernel's sum) and lighting,
/// on SVG content.
#[test]
fn tiles_morphology_convolution_and_lighting() {
    let img = render(
        "<svg width='300' height='200' style='display:block'>
           <filter id='tile' filterUnits='userSpaceOnUse' x='0' y='0' width='100' height='100' color-interpolation-filters='sRGB'>
             <feFlood flood-color='blue' x='0' y='0' width='10' height='10' result='a'/>
             <feFlood flood-color='green' x='10' y='0' width='10' height='10'/>
             <feMerge x='0' y='0' width='20' height='10'><feMergeNode in='a'/><feMergeNode/></feMerge>
             <feTile/>
           </filter>
           <filter id='dilate' filterUnits='userSpaceOnUse' x='100' y='0' width='100' height='100' color-interpolation-filters='sRGB'>
             <feMorphology operator='dilate' radius='10'/>
           </filter>
           <filter id='convolve' filterUnits='userSpaceOnUse' x='200' y='0' width='100' height='100' color-interpolation-filters='sRGB'>
             <feConvolveMatrix kernelMatrix='1 1 1  1 1 1  1 1 1' divisor='0' bias='0'/>
           </filter>
           <filter id='light' x='0' y='0' width='1' height='1' color-interpolation-filters='sRGB'>
             <feDiffuseLighting lighting-color='blue'><feDistantLight azimuth='-45' elevation='390'/></feDiffuseLighting>
           </filter>
           <filter id='light2' x='0' y='0' width='1' height='1' color-interpolation-filters='sRGB'>
             <feDiffuseLighting lighting-color='blue'><feDistantLight azimuth='315' elevation='30'/></feDiffuseLighting>
           </filter>
           <rect width='100' height='100' fill='red' filter='url(#tile)'/>
           <rect x='140' y='40' width='20' height='20' fill='green' filter='url(#dilate)'/>
           <rect x='220' y='20' width='60' height='60' fill='green' filter='url(#convolve)'/>
           <rect y='100' width='50' height='50' filter='url(#light)'/>
           <rect x='50' y='100' width='50' height='50' filter='url(#light2)'/>
         </svg>",
    );
    assert_eq!(pixel(&img, 45, 95), BLUE, "the tile's first half, repeated");
    assert_eq!(pixel(&img, 55, 95), GREEN, "its second half");
    assert_eq!(pixel(&img, 132, 50), GREEN, "dilated by 10");
    assert_eq!(pixel(&img, 128, 50), WHITE, "not past 10");
    assert_eq!(pixel(&img, 250, 50), GREEN, "a box blur of nine (the divisor 0 is 9)");
    assert_eq!(pixel(&img, 25, 125)[2], pixel(&img, 75, 125)[2], "-45 and 390 degrees light as 315 and 30");
    assert!(pixel(&img, 25, 125)[2] > 0, "lit");
}

/// `feImage` of an element (at the subregion's corner, as Blink places it) and of an SVG image
/// (filling the subregion), `feTurbulence` of no frequency (transparent), on SVG content.
#[test]
fn images_and_turbulence() {
    let img = render(
        "<svg width='300' height='200' style='display:block'>
           <defs><g id='img'><rect x='10' width='40' height='50' fill='green'/></g></defs>
           <filter id='element' filterUnits='userSpaceOnUse' x='0' y='0' width='100' height='100'>
             <feImage href='#img' x='20' y='0' width='80' height='100'/>
           </filter>
           <filter id='image' filterUnits='userSpaceOnUse' x='100' y='0' width='100' height='100'>
             <feImage preserveAspectRatio='none' href=\"data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' width='2' height='2'><rect x='1' width='1' height='2' fill='green'/></svg>\"/>
           </filter>
           <filter id='noise' x='0' y='0' width='1' height='1' color-interpolation-filters='sRGB'>
             <feTurbulence baseFrequency='-1 1'/>
             <feColorMatrix values='1 0 0 0 0, 0 1 0 0 0.502, 0 0 1 0 0, 0 0 0 1 1'/>
           </filter>
           <rect width='100' height='100' fill='red' filter='url(#element)'/>
           <rect x='100' width='100' height='100' fill='red' filter='url(#image)'/>
           <rect x='200' width='100' height='100' fill='red' filter='url(#noise)'/>
         </svg>",
    );
    assert_eq!(pixel(&img, 25, 25), WHITE, "the element is not at its own place");
    assert_eq!(pixel(&img, 45, 25), GREEN, "but 20 px right, at the subregion's corner");
    assert_eq!(pixel(&img, 125, 50), WHITE, "the image's transparent half");
    assert_eq!(pixel(&img, 175, 50), GREEN, "its green half");
    assert_eq!(pixel(&img, 250, 50), GREEN, "no noise, then green");
}

/// A `filter` that references nothing leaves the element unfiltered (WPT filter-invalid), where
/// usvg left it out.
#[test]
fn a_missing_filter_is_no_filter() {
    let img = render(
        "<svg width='300' height='200' style='display:block'>
           <rect width='100' height='100' fill='green' filter='url(#not-found)'/>
         </svg>",
    );
    assert_eq!(pixel(&img, 50, 50), GREEN);
}

/// `filter: url()` on HTML elements: the SVG filter's graph in the element's user space (CSS px
/// from its border box's corner), filling its filter region even for an empty or hidden element;
/// a colour matrix that makes transparent pixels visible fills the region too.
#[test]
fn svg_filters_on_html_elements() {
    let img = render(
        "<svg width='0' height='0' style='position:absolute'>
           <filter id='flood' color-interpolation-filters='sRGB' filterUnits='userSpaceOnUse' x='0' y='0' width='50' height='50'>
             <feFlood flood-color='green'/></filter>
           <filter id='matrix' x='0' y='0' width='100%' height='100%' color-interpolation-filters='sRGB'>
             <feColorMatrix values='0 0 0 0 0  0 0 0 0 0.502  0 0 0 0 0  0 0 0 0 1'/></filter>
           <filter id='offset' primitiveUnits='objectBoundingBox' x='0' y='0' width='1.25' height='1.25'>
             <feOffset dx='0.2' dy='0.2'/></filter>
         </svg>
         <div style='position:absolute; left:0; top:0; width:0; height:0; filter:url(#flood)'></div>
         <div style='position:absolute; left:100px; top:0; width:50px; height:50px; background:red; visibility:hidden; filter:url(#flood)'></div>
         <div style='position:absolute; left:200px; top:0; width:50px; height:50px; filter:url(#matrix)'></div>
         <div style='position:absolute; left:0; top:100px; width:50px; height:50px; background:green; filter:url(#offset)'></div>",
    );
    assert_eq!(pixel(&img, 25, 25), GREEN, "an empty element's region");
    assert_eq!(pixel(&img, 125, 25), GREEN, "a hidden element's region");
    assert_eq!(pixel(&img, 225, 25), GREEN, "transparent made green over the region");
    assert_eq!(pixel(&img, 5, 105), WHITE, "moved by a fifth of the box");
    assert_eq!(pixel(&img, 55, 155), GREEN, "to 10 px right and down");
}
