//! Effects and references in inline SVG content as Chromium draws them (WPT `css/filter-effects`
//! `svg-*`, `css/css-masking/mask-image` on SVG elements, `clip-path-svg-content`,
//! `mask-svg-content`, `css/compositing` in SVG): CSS filter functions without a filter region,
//! the page's `filter` reaching usvg, masks of opaque shapes at their shapes' coverage, references
//! to the wrong kind of element, invalid transform lists, `text-shadow` on SVG text, CSS mask
//! layers (images, gradients, `mask-composite`), `plus-lighter`, isolated SVG images, `zoom` and
//! the root's `perspective`. Also what review found: shadow lengths under `zoom`, the page's
//! `filter` over the `<svg>`'s own sheet, groups in a `<switch>`, references to another document's
//! `<mask>`, the transform attribute with a 3D transform, and `<filter>`s shaped like functions.
use std::sync::Arc;

use pyro_render::*;

const WHITE: [u8; 3] = [255, 255, 255];
const BLUE: [u8; 3] = [0, 0, 255];
const GREEN: [u8; 3] = [0, 128, 0];

/// A 20x20 SVG image whose top-left 10x10 quarter is opaque (`m.svg`), one whose green
/// rectangle blends `difference` with what is under it (`blend.svg`), and a sprite of a `<mask>`
/// that draws nothing itself (`sprite.svg`), and one without a size or `viewBox` (`sizeless.svg`).
struct Resources;
impl Fetcher for Resources {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let path = url.split('#').next().unwrap_or_default();
        let name = path.rsplit('/').next().unwrap_or_default().to_string();
        std::thread::spawn(move || {
            done(match name.as_str() {
                "m.svg" => Ok(br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="10" height="10"/></svg>"#.to_vec()),
                "sprite.svg" => Ok(br#"<svg xmlns="http://www.w3.org/2000/svg"><defs><mask id="m"><rect width="10" height="10" fill="white"/></mask></defs></svg>"#.to_vec()),
                "sizeless.svg" => Ok(br#"<svg xmlns="http://www.w3.org/2000/svg"><rect width="50" height="50" fill="blue"/></svg>"#.to_vec()),
                "blend.svg" => Ok(br#"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50"><rect width="50" height="50" style="fill:rgb(0,255,0);mix-blend-mode:difference"/></svg>"#.to_vec()),
                _ => Err("404".into()),
            })
        });
    }
    /// The page's own origin: a CORS fetch (a `mask-image`'s, which CSS Masking makes one) gets
    /// the same response.
    fn fetch_cors(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        self.fetch(url, done)
    }
}

fn render(body: &str) -> RgbaImage {
    let s = Snapshot {
        html: format!("<!doctype html><style>body{{margin:0;background:white}}svg{{display:block}}</style>{body}"),
        base_url: "http://test/".into(),
        width: 200,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(Resources);
    Renderer::new().render(&s, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn px(img: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let i = (y as usize * img.width as usize + x as usize) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

/// The largest channel difference between two renders in the rectangle `x0..x1`, `y0..y1`.
fn max_difference(a: &RgbaImage, b: &RgbaImage, (x0, y0, x1, y1): (u32, u32, u32, u32)) -> u8 {
    let mut max = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let (p, q) = (px(a, x, y), px(b, x, y));
            for c in 0..3 {
                max = max.max(p[c].abs_diff(q[c]));
            }
        }
    }
    max
}

#[test]
fn a_drop_shadow_function_reaches_past_half_the_elements_box() {
    // usvg gives a filter function the bounding box widened by half as its region, which cut
    // this shadow off at x = 75 (WPT svg-shorthand-drop-shadow-001).
    let img = render(
        r##"<svg width="200" height="100"><rect width="50" height="100" fill="#0000ff"
            filter="drop-shadow(50px 0 #0000ff)"/></svg>"##,
    );
    assert_eq!(px(&img, 90, 50), BLUE, "the far half of the shadow");
    assert_eq!(px(&img, 110, 50), WHITE);
}

#[test]
fn the_pages_drop_shadow_with_a_colour_function_reaches_usvg() {
    // Written in the `style` attribute, `drop-shadow(rgb(0, 0, 255) 30px 0px 0px)` was cut at
    // its first `)` by usvg's tokenizer and the element drawn unfiltered; `color-mix()` and
    // `currentcolor` are resolved for it.
    let img = render(
        r##"<style>#a { filter: drop-shadow(30px 0 0 rgb(0, 0, 255)) }
          #b { color: blue; filter: drop-shadow(30px 0 0 color-mix(in srgb, currentcolor 100%, black)) }</style>
        <svg width="200" height="100"><rect id="a" width="20" height="20" fill="#0000ff"/>
          <rect id="b" y="50" width="20" height="20" fill="#0000ff"/></svg>"##,
    );
    assert_eq!(px(&img, 40, 10), BLUE);
    assert_eq!(px(&img, 40, 60), BLUE);
}

#[test]
fn a_drop_shadow_layer_keeps_its_contents_anti_aliased_edges() {
    // The shadow's layer reaches two device pixels past the content: clipped at the circle's
    // stroke it lightened every anti-aliased pixel of its edge (by up to 55 levels).
    let shadowed = render(
        r##"<svg width="200" height="200"><circle cx="40.3" cy="40.3" r="20" stroke="black" stroke-width="4"
            filter="drop-shadow(100px 100px 0 red)"/></svg>"##,
    );
    let plain = render(
        r##"<svg width="200" height="200"><circle cx="40.3" cy="40.3" r="20" stroke="black" stroke-width="4"/></svg>"##,
    );
    assert!(max_difference(&shadowed, &plain, (0, 0, 100, 100)) <= 1);
}

#[test]
fn a_boxs_mask_of_one_opaque_shape_has_that_shapes_coverage() {
    // A box's `mask-image: url(#m)` of a circle is drawn as the circle rather than rasterized at
    // twice the resolution: a reference drawing the circle has the same edge (WPT
    // mask-image-3a..e were 52 levels apart).
    let masked = render(
        r##"<svg height="0" style="position: absolute"><mask id="m"><circle cx="50" cy="50" r="25" fill="white"/></mask></svg>
        <div style="width: 100px; height: 100px; background: #800080; mask-image: url(#m)"></div>"##,
    );
    let drawn = render(r##"<svg width="200" height="200"><circle cx="50" cy="50" r="25" fill="#800080"/></svg>"##);
    assert!(max_difference(&masked, &drawn, (0, 0, 100, 100)) <= 1);
}

#[test]
fn references_to_the_wrong_kind_of_element_are_ignored() {
    // A clip path or mask naming a gradient is no clip path or mask (usvg left the element out).
    let img = render(
        r##"<svg width="200" height="100"><linearGradient id="g"/>
            <rect width="40" height="40" fill="#0000ff" clip-path="url(#g)"/>
            <rect x="50" width="40" height="40" fill="#0000ff" mask="url(#g)"/></svg>"##,
    );
    assert_eq!(px(&img, 20, 20), BLUE);
    assert_eq!(px(&img, 70, 20), BLUE);
}

#[test]
fn a_transform_attribute_that_is_no_transform_list_is_ignored() {
    // `rotate(90,)`, which usvg read as `rotate(90)` (WPT svg-external-styles-014).
    let img = render(
        r##"<svg width="200" height="100"><rect x="10" y="10" width="20" height="20" fill="#0000ff"
            transform="rotate(90,)"/></svg>"##,
    );
    assert_eq!(px(&img, 20, 20), BLUE);
}

/// The rows that hold a pixel of `color`, top to bottom (`None` for none).
fn rows_of(img: &RgbaImage, color: [u8; 3]) -> Option<(u32, u32)> {
    let rows: Vec<u32> = (0..img.height).filter(|&y| (0..img.width).any(|x| px(img, x, y) == color)).collect();
    Some((*rows.first()?, *rows.last()?))
}

#[test]
fn svg_text_draws_its_text_shadow() {
    // The shadow is the text's copy 30 px down, under the text (which is drawn over its top).
    let img = render(
        r##"<svg width="200" height="100" style="font: bold 48px sans-serif; text-shadow: #0000ff 0 30px">
            <text x="10" y="45">HH</text></svg>"##,
    );
    let (text_top, text_bottom) = rows_of(&img, [0, 0, 0]).expect("the text");
    let (shadow_top, shadow_bottom) = rows_of(&img, BLUE).expect("the shadow");
    assert!(
        shadow_bottom.abs_diff(text_bottom + 30) <= 1,
        "text rows {text_top}..{text_bottom}, shadow to {shadow_bottom}"
    );
    assert!(shadow_top > text_bottom, "the text covers the shadow's top: shadow from {shadow_top}");
}

#[test]
fn shadow_lengths_on_zoomed_svg_content_are_zoomed_once() {
    // Stylo's computed lengths carry `zoom`; user units are scaled by the `<svg>`'s box: a 20 px
    // offset under `zoom: 2` is 40 device px, not 80.
    let img = render(
        r##"<svg width="100" height="50" viewBox="0 0 100 50" style="zoom: 2; font: bold 20px sans-serif; text-shadow: #0000ff 20px 0">
            <rect width="20" height="20" fill="#0000ff" style="filter: drop-shadow(20px 0 0 #0000ff)"/>
            <text x="0" y="45">I</text></svg>"##,
    );
    assert_eq!(px(&img, 60, 20), BLUE, "the drop shadow at 40..80");
    assert_eq!(px(&img, 100, 20), WHITE);
    let shadow = (0..img.width).find(|&x| (50..100).any(|y| px(&img, x, y) == BLUE)).expect("the text shadow");
    let text = (0..img.width).find(|&x| (50..100).any(|y| px(&img, x, y) == [0, 0, 0])).expect("the text");
    assert!(shadow.abs_diff(text + 40) <= 1, "text from {text}, its shadow from {shadow}");
}

#[test]
fn the_pages_filter_wins_over_the_svgs_own_style_sheet() {
    // A `style` declaration (as the page's value is written) overrides the `<svg>`'s `<style>`
    // rule in usvg, where an attribute would not.
    let img = render(
        r##"<svg width="200" height="100"><style>.a { filter: drop-shadow(30px 0 0 #0000ff) }</style>
            <rect class="a" style="filter: none" width="20" height="20" fill="#0000ff"/></svg>"##,
    );
    assert_eq!(px(&img, 10, 10), BLUE);
    assert_eq!(px(&img, 40, 10), WHITE, "no shadow");
}

#[test]
fn a_css_mask_gradient_masks_an_svg_element() {
    let img = render(
        r##"<svg width="200" height="100"><rect width="100" height="100" fill="#0000ff"
            style="mask-image: linear-gradient(black 50%, transparent 50%)"/></svg>"##,
    );
    assert_eq!(px(&img, 50, 25), BLUE);
    assert_eq!(px(&img, 50, 75), WHITE);
}

#[test]
fn a_css_mask_svg_image_is_tiled_over_an_svg_element() {
    // The image (loaded after the `<svg>` was first built) repeats every 20 px; its opaque
    // quarter shows the rect.
    let img = render(
        r##"<svg width="200" height="100"><rect width="60" height="60" fill="#0000ff"
            style="mask-image: url(m.svg)"/></svg>"##,
    );
    assert_eq!(px(&img, 5, 5), BLUE);
    assert_eq!(px(&img, 15, 15), WHITE);
    assert_eq!(px(&img, 25, 25), BLUE, "the next tile");
    assert_eq!(px(&img, 45, 5), BLUE);
}

#[test]
fn one_mask_reference_keeps_the_stroke_past_the_fill_box() {
    // With a `mask-mode`, the reference is still usvg's own `mask` (whose region holds the
    // stroke), not a layer of the fill box.
    let img = render(
        r##"<svg width="200" height="100"><mask id="m" maskUnits="userSpaceOnUse" x="0" y="0" width="200" height="100">
            <rect width="200" height="100" fill="white"/></mask>
            <rect x="20" y="20" width="40" height="40" fill="#0000ff" stroke="#0000ff" stroke-width="20"
                style="mask-image: url(#m); mask-mode: alpha"/></svg>"##,
    );
    assert_eq!(px(&img, 40, 40), BLUE);
    assert_eq!(px(&img, 14, 40), BLUE, "the stroke");
}

#[test]
fn a_mask_of_another_document_masks_the_element() {
    // `sprite.svg#m` names a `<mask>` of another document: the element is masked by it (its
    // white 10x10 square), not by what the sprite draws (nothing). An SVG image with a fragment
    // that names no `<mask>` is the layer's image.
    let img = render(
        r##"<svg width="200" height="100"><rect width="60" height="60" fill="#0000ff" style="mask-image: url(sprite.svg#m)"/>
            <rect x="100" width="60" height="60" fill="#0000ff" style="mask-image: url(m.svg#nothing)"/></svg>"##,
    );
    assert_eq!(px(&img, 5, 5), BLUE, "under the mask's square");
    assert_eq!(px(&img, 30, 30), WHITE, "outside it");
    assert_eq!(px(&img, 105, 5), BLUE);
    assert_eq!(px(&img, 115, 15), WHITE, "m.svg's transparent quarter");
}

/// `mask-mode: alpha` or `luminance` on the masked element is used in place of the `<mask>`'s
/// `mask-type`; `match-source` takes the `mask-type` (WPT `mask-mode-to-mask-type-svg`). The
/// mask's content is opaque black: nothing by its luminance, everything by its alpha.
#[test]
fn mask_mode_is_used_in_place_of_the_masks_mask_type() {
    let img = render(
        r##"<style>rect.m { fill: #0000ff; width: 20px; height: 20px }
              #lum { mask-type: luminance } #alpha { mask-type: alpha }</style>
            <svg width="200" height="100">
              <mask id="lum"><rect width="200" height="100" fill="black"/></mask>
              <mask id="alpha"><rect width="200" height="100" fill="black"/></mask>
              <rect class="m" x="0" style="mask-image: url(#lum); mask-mode: match-source"/>
              <rect class="m" x="30" style="mask-image: url(#alpha); mask-mode: match-source"/>
              <rect class="m" x="60" style="mask-image: url(#lum); mask-mode: alpha"/>
              <rect class="m" x="90" style="mask-image: url(#alpha); mask-mode: luminance"/>
              <rect class="m" x="120" style="mask-image: url(#lum); mask-mode: luminance"/>
              <rect class="m" x="150" style="mask-image: url(#alpha); mask-mode: alpha"/></svg>"##,
    );
    assert_eq!(px(&img, 10, 10), WHITE, "match-source of a luminance mask");
    assert_eq!(px(&img, 40, 10), BLUE, "match-source of an alpha mask");
    assert_eq!(px(&img, 70, 10), BLUE, "alpha over a luminance mask");
    assert_eq!(px(&img, 100, 10), WHITE, "luminance over an alpha mask");
    assert_eq!(px(&img, 130, 10), WHITE, "luminance of a luminance mask");
    assert_eq!(px(&img, 160, 10), BLUE, "alpha of an alpha mask");
}

#[test]
fn css_mask_layers_on_an_svg_element_composite() {
    // `intersect`: the left half and the top half leave the top-left quarter.
    let img = render(
        r##"<svg width="200" height="100"><rect width="100" height="100" fill="#0000ff"
            style="mask-image: linear-gradient(to right, black 50%, transparent 50%), linear-gradient(black 50%, transparent 50%);
                   mask-composite: intersect"/></svg>"##,
    );
    assert_eq!(px(&img, 25, 25), BLUE);
    assert_eq!(px(&img, 75, 25), WHITE);
    assert_eq!(px(&img, 25, 75), WHITE);
}

#[test]
fn plus_lighter_adds_svg_colours() {
    let img = render(
        r##"<style>rect { mix-blend-mode: plus-lighter }</style>
        <svg width="200" height="100"><g style="isolation: isolate">
            <rect width="60" height="60" fill="#ff0000"/><rect x="30" width="60" height="60" fill="#00ff00"/></g></svg>"##,
    );
    assert_eq!(px(&img, 45, 30), [255, 255, 0]);
}

#[test]
fn a_switch_chooses_the_element_not_a_group_written_for_it() {
    // The text shadow's copy and the `plus-lighter` group are not written in a `<switch>`, which
    // would choose them (they have no conditions) over the element.
    let img = render(
        r##"<style>.p { mix-blend-mode: plus-lighter }</style>
        <svg width="200" height="100" style="font: bold 48px sans-serif; text-shadow: #ff0000 0 30px">
            <switch><text x="10" y="45">HH</text></switch>
            <switch><rect class="p" systemLanguage="xx" x="150" width="40" height="40" fill="#ff0000"/>
                <rect x="150" width="40" height="40" fill="#0000ff"/></switch></svg>"##,
    );
    assert!(rows_of(&img, [0, 0, 0]).is_some(), "the text");
    assert_eq!(px(&img, 170, 20), BLUE, "the switch's second rect");
}

#[test]
fn an_svg_images_blend_modes_stay_inside_the_image() {
    // The image is an isolated group: its `difference` blends with nothing under it, not with
    // the white page (which made it magenta).
    let img = render(r##"<img src="blend.svg">"##);
    assert_eq!(px(&img, 25, 25), [0, 255, 0]);
}

#[test]
fn zoom_scales_inline_svg_content() {
    let img =
        render(r##"<svg width="50" height="50" style="zoom: 2"><rect width="10" height="10" fill="#0000ff"/></svg>"##);
    assert_eq!(px(&img, 15, 15), BLUE);
    assert_eq!(px(&img, 25, 25), WHITE);
}

#[test]
fn zoom_scales_a_sizeless_svg_images_content() {
    // An `<img>` of an SVG without a size or `viewBox` lays its content out in its box, in user
    // units of a CSS px times the zoom, as Chromium draws it (its 50-unit rect 100 px wide).
    let img = render(r##"<img src="sizeless.svg" style="zoom: 2; width: 100px; height: 100px; display: block">"##);
    assert_eq!(px(&img, 95, 95), BLUE);
    assert_eq!(px(&img, 105, 105), WHITE);
}

#[test]
fn the_roots_perspective_flattens_a_childs_depth() {
    // translateZ(-200px) under a 200px perspective about (100, 100): half the size (WPT
    // perspective-svg-child-001).
    let img = render(
        r##"<svg width="200" height="200" style="perspective: 200px"><rect x="40" y="40" width="120" height="120"
            fill="#008000" style="transform: translateZ(-200px)"/></svg>"##,
    );
    assert_eq!(px(&img, 75, 75), GREEN);
    assert_eq!(px(&img, 50, 50), WHITE);
}

#[test]
fn a_3d_individual_transform_keeps_the_transform_attribute() {
    // `rotate: y 60deg` (flattened: half the width about x = 0) after the attribute's
    // `translate(100 0)`: the rect at 50..70.
    let img = render(
        r##"<svg width="200" height="100"><rect width="40" height="40" fill="#0000ff" transform="translate(100 0)"
            style="rotate: y 60deg"/></svg>"##,
    );
    assert_eq!(px(&img, 60, 20), BLUE);
    assert_eq!(px(&img, 10, 20), WHITE);
    assert_eq!(px(&img, 120, 20), WHITE);
}

#[test]
fn a_filter_element_named_as_usvg_names_filter_functions_keeps_its_region() {
    // One `feColorMatrix` with `result="result"` over the default region, as usvg makes a
    // `saturate()`: a `<filter>` (its own id) floods its whole region (20..80) with the alpha
    // offset, not just the content's bounds.
    let img = render(
        r##"<svg width="200" height="100"><filter id="f"><feColorMatrix result="result" type="matrix"
            values="0 0 0 0 0  0 0 0 0 0  0 0 0 0 1  0 0 0 0 1"/></filter>
            <rect x="25" y="25" width="50" height="50" fill="#0000ff" filter="url(#f)"/></svg>"##,
    );
    assert_eq!(px(&img, 50, 50), BLUE);
    assert_eq!(px(&img, 21, 50), BLUE, "the region past the content");
    assert_eq!(px(&img, 18, 50), WHITE);
}

/// A shape in a `<mask>` that only a box of the page references has no box in usvg's tree of the
/// `<svg>`: its CSS `clip-path` basic shape resolves against the box of its geometry attributes
/// (WPT `mask-image-3h`). Before, the shape was clipped to nothing.
#[test]
fn a_basic_shape_clip_path_inside_a_mask_resolves_against_the_shapes_box() {
    let img = render(
        r##"<svg height="0"><mask id="m" x="0" y="0" width="1" height="1">
              <rect x="25" y="25" width="50" height="50" fill="white" clip-path="circle(50% at 50% 50%)"/></mask></svg>
            <div style="width:100px;height:100px;background:#0000ff;mask-image:url(#m)"></div>"##,
    );
    assert_eq!(px(&img, 50, 50), BLUE, "in the circle");
    assert_eq!(px(&img, 27, 27), WHITE, "the rect's corner, outside the circle");
    assert_eq!(px(&img, 10, 10), WHITE, "outside the rect");
}

/// A `<mask>` in user units reaches past the border box, and `mask-clip` does not cut a layer
/// that references a `<mask>`: it shows the element's box shadow where its content is (WPT
/// `mask-image-3f`).
#[test]
fn a_user_space_mask_shows_the_box_shadow_outside_the_border_box() {
    let img = render(
        r##"<svg height="0"><mask id="m" x="-50" y="-50" width="200" height="200" maskUnits="userSpaceOnUse">
              <rect x="-50" y="-50" width="40" height="40" fill="white"/></mask></svg>
            <div style="margin:60px;width:50px;height:50px;background:red;box-shadow:0 0 0 50px #0000ff;mask-image:url(#m)"></div>"##,
    );
    assert_eq!(px(&img, 30, 30), BLUE, "the shadow under the mask's square");
    assert_eq!(px(&img, 55, 30), WHITE, "the shadow beside it");
    assert_eq!(px(&img, 80, 80), WHITE, "the box, masked out");
}

/// An inline `<svg>` references resources of another inline `<svg>` of the page (a hidden one
/// of definitions): ids are the document's. usvg reads each `<svg>` alone, so the referenced
/// gradient (and the one its `href` names), filter and clip path are copied into the source.
/// Before, the fill was dropped and the filter and clip path ignored (WPT
/// `svg-filter-filter-units-user-space`).
#[test]
fn an_svg_references_the_resources_of_another_inline_svg() {
    let img = render(
        r##"<svg width="0" height="0" style="position:absolute"><defs>
              <linearGradient id="base"><stop offset="0" stop-color="#0000ff"/><stop offset="1" stop-color="#0000ff"/></linearGradient>
              <linearGradient id="g" href="#base"/>
              <filter id="f" x="0" y="0" width="1" height="1"><feFlood flood-color="green"/></filter>
              <clipPath id="c"><rect width="20" height="50"/></clipPath></defs></svg>
            <svg width="200" height="50">
              <rect width="50" height="50" fill="url(#g)"/>
              <rect x="60" width="50" height="50" fill="red" filter="url(#f)"/>
              <g transform="translate(120)"><rect width="50" height="50" fill="#0000ff" clip-path="url(#c)"/></g></svg>"##,
    );
    assert_eq!(px(&img, 25, 25), BLUE, "the gradient of the other svg, through its href");
    assert_eq!(px(&img, 85, 25), GREEN, "the filter of the other svg");
    assert_eq!(px(&img, 130, 25), BLUE, "inside the clip path of the other svg");
    assert_eq!(px(&img, 150, 25), WHITE, "outside it");
}
