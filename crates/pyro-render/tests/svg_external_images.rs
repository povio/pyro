//! SVG `<image href>`/`<feImage href>` of resources usvg cannot load itself (`svg_use::resolve_images`:
//! fetched, then inlined as `data:` URLs), and a table cell `width` that stays a `calc()` (`auto`).
use std::sync::Arc;

use pyro_render::*;

/// Serves a 4x4 green PNG at `http://test/green.png`.
struct Green;
impl Fetcher for Green {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        if url == "http://test/green.png" {
            let pixels = image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 128, 0, 255]));
            let mut png = Vec::new();
            image::DynamicImage::ImageRgba8(pixels)
                .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                .unwrap();
            done(Ok(png));
        } else {
            done(Err("offline".into()));
        }
    }
}

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(Green);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff'>{body}"),
        base_url: "http://test/".into(),
        width: 200,
        height: 100,
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
fn an_svg_image_of_a_relative_url_is_drawn() {
    let img = render("<svg width='50' height='50'><image href='green.png' width='50' height='50'/></svg>");
    assert_eq!(pixel(&img, 25, 25), [0, 128, 0]);
}

#[test]
fn an_fe_image_of_a_relative_url_is_drawn() {
    let img = render(
        "<div style='width:50px;height:50px;background:red;filter:url(#f)'></div>\
         <svg width='0' height='0'><filter id='f' x='0' y='0' width='1' height='1'><feImage href='green.png'/></filter></svg>",
    );
    assert_eq!(pixel(&img, 25, 25), [0, 128, 0]);
}

#[test]
fn a_cell_width_mixing_a_percentage_and_a_length_is_auto() {
    let table = |width: &str| {
        render(&format!(
            "<table style='border-spacing:0'><tr><td style='padding:0;{width}background:green'>x</td>\
             <td style='width:100px;padding:0;background:blue'>y</td></tr></table>"
        ))
    };
    let (calc, auto) = (table("width:calc(50% + 1px);"), table(""));
    assert_eq!(calc.data, auto.data);
}


#[test]
fn a_root_its_content_url_replaces_is_its_image() {
    // The root is the image at its natural size; its body is not rendered, nor its background
    // propagated to the canvas.
    let img = render(
        "<style>:root{content:url(green.png);width:auto}body{background:red}</style><p>text</p>",
    );
    assert_eq!(pixel(&img, 2, 2), [0, 128, 0]);
    assert_eq!(pixel(&img, 10, 10), [255, 255, 255]);
}

#[test]
fn an_inline_block_its_content_url_replaces_in_an_inline_block() {
    let img = render(
        "<div style='display:inline-block;width:100px'><div style='display:inline-block;content:url(green.png)'></div></div>\
         <div style='display:inline-block;width:100px'><div style='display:inline-block;content:url(missing.png)'>FAIL</div></div>",
    );
    // (It rendered: this used to panic.) The image shows; the broken one's text does not.
    let green = (0..img.height as usize).any(|y| (0..100).any(|x| pixel(&img, x, y) == [0, 128, 0]));
    let ink = (0..img.height as usize).any(|y| (100..200).any(|x| pixel(&img, x, y) != [255, 255, 255]));
    assert!(green && !ink);
}

#[test]
fn a_gradient_content_replaces_an_element_and_a_pseudo_element() {
    let gradient = |css: &str, html: &str| render(&format!("<style>{css}</style>{html}"));
    let element = gradient("div{width:50px;height:50px;content:linear-gradient(green,green)}", "<div>FAIL</div>");
    let pseudo = gradient(
        "div::before{content:linear-gradient(green,green);display:block;width:50px;height:50px}",
        "<div></div>",
    );
    for img in [&element, &pseudo] {
        assert_eq!(pixel(img, 25, 25), [0, 128, 0]);
        assert_eq!(pixel(img, 75, 25), [255, 255, 255]);
    }
}
