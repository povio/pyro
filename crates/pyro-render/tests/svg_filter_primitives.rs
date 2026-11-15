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

/// An SVG filter's region paints under an ancestor's opacity layer too: the layer's bounds take
/// the whole region of a filter that paints where its content is not (a flood), for an empty and
/// a hidden element, and for a flood past a box's content; before, the empty and hidden elements'
/// groups had no bounds and the ancestor drew nothing, and the flood was cut to the box.
#[test]
fn svg_filter_regions_under_an_opacity_ancestor() {
    let img = render(
        "<svg width='0' height='0' style='position:absolute'>
           <filter id='flood' color-interpolation-filters='sRGB' filterUnits='userSpaceOnUse' x='0' y='0' width='50' height='50'>
             <feFlood flood-color='green'/></filter>
           <filter id='wide' color-interpolation-filters='sRGB' filterUnits='userSpaceOnUse' x='0' y='0' width='100' height='100'>
             <feFlood flood-color='green'/></filter>
         </svg>
         <div style='opacity:0.5'>
           <div style='position:absolute; left:0; top:0; width:0; height:0; filter:url(#flood)'></div>
           <div style='position:absolute; left:100px; top:0; width:50px; height:50px; background:red; visibility:hidden; filter:url(#flood)'></div>
           <div style='position:absolute; left:0; top:100px; width:50px; height:50px; background:red; filter:url(#wide)'></div>
         </div>
         <div style='opacity:0.5; position:absolute; left:200px; top:0'>
           <svg width='50' height='50' style='display:block'>
             <filter id='inline' color-interpolation-filters='sRGB' filterUnits='userSpaceOnUse' x='0' y='0' width='50' height='50'>
               <feFlood flood-color='green'/></filter>
             <rect width='10' height='10' fill='red' filter='url(#inline)'/>
           </svg>
         </div>",
    );
    let half_green = [127, 191, 127];
    let near = |a: [u8; 3], b: [u8; 3]| a.iter().zip(b).all(|(a, b)| a.abs_diff(b) <= 2);
    for (x, y, what) in [
        (25, 25, "an empty element's region"),
        (125, 25, "a hidden element's"),
        (75, 175, "a flood past the box"),
        (240, 40, "an inline SVG's flood past its rectangle"),
    ] {
        assert!(near(pixel(&img, x, y), half_green), "{what}: {:?}", pixel(&img, x, y));
    }
}

/// usvg names the second and later uses of an `objectBoundingBox` filter (the default units)
/// `filterN`: the notes of the `<filter>` reach those uses too (a `currentcolor` map stays tainted
/// on both rectangles), and a `<filter href>` template's primitives give the notes of the filter
/// using it (a template no element uses itself, of another colour than `f`'s).
#[test]
fn a_filter_used_twice_or_through_a_template_keeps_its_taint() {
    let img = render(
        "<svg width='300' height='200' style='display:block'>
           <linearGradient id='g'><stop stop-color='red'/><stop stop-color='red' offset='.5'/>
             <stop stop-color='green' offset='.5'/><stop stop-color='green' offset='1'/></linearGradient>
           <filter id='f' color-interpolation-filters='sRGB'>
             <feFlood flood-color='currentcolor' style='color: rgb(0%, 100%, 50%)'/>
             <feDisplacementMap in='SourceGraphic' xChannelSelector='G' yChannelSelector='B' scale='100'/>
           </filter>
           <filter id='base' color-interpolation-filters='sRGB'>
             <feFlood flood-color='currentcolor' style='color: rgb(0%, 100%, 51%)'/>
             <feDisplacementMap in='SourceGraphic' xChannelSelector='G' yChannelSelector='B' scale='100'/>
           </filter>
           <filter id='t' href='#base' filterUnits='userSpaceOnUse' x='0' y='100' width='300' height='100'/>
           <rect width='100' height='100' fill='url(#g)' filter='url(#f)'/>
           <rect x='150' width='100' height='100' fill='url(#g)' filter='url(#f)'/>
           <rect y='100' width='100' height='100' fill='url(#g)' filter='url(#t)'/>
         </svg>",
    );
    for (x, y, what) in [(25, 50, "the first use"), (175, 50, "the second"), (25, 150, "the template")] {
        assert_eq!(pixel(&img, x, y), RED, "{what}: unmoved");
        assert_eq!(pixel(&img, x + 50, y), GREEN, "{what}: unmoved");
    }
}

/// Culling takes an SVG filter graph's region: a flood whose region reaches into view from a box
/// left of the viewport paints, and so does an empty `overflow: hidden` element's region.
#[test]
fn svg_filter_regions_are_not_culled() {
    let img = render(
        "<svg width='0' height='0' style='position:absolute'>
           <filter id='far' color-interpolation-filters='sRGB' filterUnits='userSpaceOnUse' x='0' y='0' width='300' height='50'>
             <feFlood flood-color='green'/></filter>
           <filter id='flood' color-interpolation-filters='sRGB' filterUnits='userSpaceOnUse' x='0' y='0' width='50' height='50'>
             <feFlood flood-color='green'/></filter>
         </svg>
         <div style='position:absolute; left:-200px; top:0; width:50px; height:50px; filter:url(#far)'></div>
         <div style='position:absolute; left:0; top:100px; width:0; height:0; overflow:hidden; filter:url(#flood)'></div>",
    );
    assert_eq!(pixel(&img, 50, 25), GREEN, "the region reaching into view");
    assert_eq!(pixel(&img, 25, 125), GREEN, "an empty overflow: hidden element's region");
}

/// `filter` lists mixing CSS functions and an SVG graph on HTML elements: the functions apply in
/// their place in the list (an offset, then a drop shadow of the moved content), and a
/// `drop-shadow()` coloured from `currentcolor` taints what follows (a displacement through the
/// shadowed source then moves nothing).
#[test]
fn filter_lists_mixing_functions_and_graphs() {
    let img = render(
        "<svg width='0' height='0' style='position:absolute'>
           <filter id='offset' filterUnits='userSpaceOnUse' x='-100' y='-100' width='400' height='400'>
             <feOffset dx='50' dy='0'/></filter>
           <filter id='displace' color-interpolation-filters='sRGB' filterUnits='userSpaceOnUse' x='0' y='0' width='100' height='50'>
             <feFlood flood-color='green' result='green'/>
             <feDisplacementMap in='green' in2='SourceGraphic' xChannelSelector='G' yChannelSelector='B' scale='100'/>
           </filter>
         </svg>
         <div style='position:absolute; left:0; top:0; width:25px; height:25px; background:green;
                     filter:url(#offset) drop-shadow(0 50px 0 blue)'></div>
         <div style='position:absolute; left:0; top:100px; width:100px; height:50px; background:rgb(0, 255, 128);
                     color:rgb(0, 255, 128); filter:drop-shadow(0 0 0 currentcolor) url(#displace)'></div>",
    );
    assert_eq!(pixel(&img, 12, 12), WHITE, "moved away");
    assert_eq!(pixel(&img, 62, 12), GREEN, "moved 50 px right");
    assert_eq!(pixel(&img, 62, 62), BLUE, "the shadow of the moved box");
    assert_eq!(pixel(&img, 12, 62), WHITE, "not of the box where it was");
    assert_eq!(pixel(&img, 10, 125), GREEN, "a tainted map moves nothing: all green");
    assert_eq!(pixel(&img, 90, 125), GREEN, "to the right edge");
}

/// An inline `<svg>` that declares `overflow: visible` shows what its filters make past its box
/// (an icon's glow); one that does not is cut at its box.
#[test]
fn an_svg_with_overflow_visible_shows_its_filter_output() {
    let img = render(
        "<div style='position:absolute; left:50px; top:50px'>
           <svg width='20' height='20' style='display:block; overflow:visible'>
             <filter id='f' color-interpolation-filters='sRGB' filterUnits='userSpaceOnUse' x='-40' y='-40' width='100' height='100'>
               <feFlood flood-color='green'/></filter>
             <rect width='20' height='20' fill='red' filter='url(#f)'/>
           </svg>
         </div>
         <div style='position:absolute; left:200px; top:50px'>
           <svg width='20' height='20' style='display:block' overflow='visible'>
             <filter id='g' color-interpolation-filters='sRGB' filterUnits='userSpaceOnUse' x='-40' y='-40' width='100' height='100'>
               <feFlood flood-color='green'/></filter>
             <rect width='20' height='20' fill='red' filter='url(#g)'/>
           </svg>
         </div>
         <div style='position:absolute; left:50px; top:150px'>
           <svg width='20' height='20' style='display:block'>
             <filter id='h' color-interpolation-filters='sRGB' filterUnits='userSpaceOnUse' x='-40' y='-40' width='100' height='100'>
               <feFlood flood-color='green'/></filter>
             <rect width='20' height='20' fill='red' filter='url(#h)'/>
           </svg>
         </div>",
    );
    assert_eq!(pixel(&img, 30, 30), GREEN, "the style's overflow: visible");
    assert_eq!(pixel(&img, 180, 30), GREEN, "the attribute's");
    assert_eq!(pixel(&img, 30, 130), WHITE, "cut at the box");
    assert_eq!(pixel(&img, 60, 160), GREEN, "in it");
}

/// A real `feFlood` of opacity 0 (Figma's `BackgroundImageFix`) is a transparent flood, not usvg's
/// stand-in for a primitive it could not read: the filter applies (the box moved 50 px right).
#[test]
fn a_transparent_flood_is_a_flood() {
    let img = render(
        "<svg width='300' height='200' style='display:block'>
           <filter id='f' filterUnits='userSpaceOnUse' x='0' y='0' width='300' height='200'>
             <feFlood flood-opacity='0' result='BackgroundImageFix'/>
             <feOffset in='SourceGraphic' dx='50' result='moved'/>
             <feMerge><feMergeNode in='BackgroundImageFix'/><feMergeNode in='moved'/></feMerge>
           </filter>
           <rect width='50' height='50' fill='green' filter='url(#f)'/>
         </svg>",
    );
    assert_eq!(pixel(&img, 25, 25), WHITE, "moved away");
    assert_eq!(pixel(&img, 75, 25), GREEN);
}
