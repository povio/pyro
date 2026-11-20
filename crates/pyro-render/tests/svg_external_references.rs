//! `clip-path`, `filter` and `mask-image` references to elements of other SVG documents
//! (`url(shapes.svg#clip)`, `data:` URLs too), on boxes and on the elements of an inline `<svg>`:
//! the document is loaded and the element taken from it. Before, only elements of the page were
//! found, and such a reference did nothing (WPT css-masking `clip-path-url-reference-external*`,
//! `mask-image-url-remote-mask`, filter-effects `filter-external-002-test`,
//! `svg-external-filter-resource`, `svg-data-uri-filter-resource`).
use std::sync::Arc;

use pyro_render::*;

const SHAPES: &str = "<?xml version='1.0'?>
<svg xmlns='http://www.w3.org/2000/svg'>
  <clipPath id='circle' clipPathUnits='objectBoundingBox'><circle cx='0.5' cy='0.5' r='0.5'/></clipPath>
  <clipPath id='left'><rect width='50' height='100'/></clipPath>
  <filter id='flood' x='0' y='0' width='1' height='1'><feFlood flood-color='blue'/></filter>
  <mask id='top' mask-type='alpha'><rect width='100' height='50'/></mask>
</svg>";

/// Serves `http://test/shapes.svg` (whatever the fragment) and percent-encoded
/// `data:image/svg+xml,` URLs; anything else fails.
struct Files;
impl Fetcher for Files {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        std::thread::spawn(move || {
            let document = url.split('#').next().unwrap_or(&url);
            if document == "http://test/shapes.svg" {
                done(Ok(SHAPES.as_bytes().to_vec()))
            } else if let Some(svg) = document.strip_prefix("data:image/svg+xml,") {
                done(Ok(svg.replace("%3C", "<").replace("%3E", ">").replace("%20", " ").into_bytes()))
            } else {
                done(Err("offline".into()))
            }
        });
    }
    /// The page's own origin: a CORS fetch (a `mask-image`'s, which CSS Masking makes one) gets
    /// the same response.
    fn fetch_cors(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        self.fetch(url, done)
    }
}

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(Files);
    let snapshot = Snapshot {
        html: format!("<!doctype html><style>body{{margin:0;background:white}} svg,div{{display:block}}</style>{body}"),
        base_url: "http://test/page.html".into(),
        width: 200,
        height: 150,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let i = (y as usize * img.width as usize + x as usize) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

const BLUE: [u8; 3] = [0, 0, 255];
const GREEN: [u8; 3] = [0, 128, 0];
const WHITE: [u8; 3] = [255; 3];

/// A box clipped by a `<clipPath>` of another document: `objectBoundingBox` units of its border
/// box, user units from its corner.
#[test]
fn a_box_is_clipped_by_another_documents_clip_path() {
    let img = render("<div style='width:100px;height:100px;background:green;clip-path:url(shapes.svg#circle)'></div>");
    assert_eq!(pixel(&img, 50, 50), GREEN, "in the circle");
    assert_eq!(pixel(&img, 4, 4), WHITE, "the corner outside it");
    assert_eq!(pixel(&img, 96, 96), WHITE, "the opposite corner");
    let img = render("<div style='width:100px;height:100px;background:green;clip-path:url(shapes.svg#left)'></div>");
    assert_eq!(pixel(&img, 25, 50), GREEN, "the left half");
    assert_eq!(pixel(&img, 75, 50), WHITE, "the right half");
}

/// A reference to an element that is no `<clipPath>`, or to a document that does not load, clips
/// nothing.
#[test]
fn another_kind_of_element_or_a_missing_document_clips_nothing() {
    for url in ["shapes.svg#top", "shapes.svg#none", "missing.svg#circle"] {
        let img = render(&format!(
            "<div style='width:100px;height:100px;background:green;clip-path:url({url})'></div>"
        ));
        assert_eq!(pixel(&img, 4, 4), GREEN, "{url}");
        assert_eq!(pixel(&img, 96, 50), GREEN, "{url}");
    }
}

/// An element of an inline `<svg>` clipped by another document's `<clipPath>`, by attribute and
/// from the page's CSS; a `data:` URL names its document in itself.
#[test]
fn an_svg_element_is_clipped_by_another_documents_clip_path() {
    let data = "data:image/svg+xml,%3Csvg%20xmlns='http://www.w3.org/2000/svg'%3E%3CclipPath%20id='c'%20clipPathUnits='objectBoundingBox'%3E%3Ccircle%20cx='0.5'%20cy='0.5'%20r='0.5'/%3E%3C/clipPath%3E%3C/svg%3E#c";
    for rect in [
        "<rect width='100' height='100' fill='green' clip-path='url(shapes.svg#circle)'/>".to_string(),
        "<rect width='100' height='100' fill='green' style='clip-path:url(shapes.svg#circle)'/>".to_string(),
        format!("<rect width='100' height='100' fill='green' clip-path=\"url({data})\"/>"),
    ] {
        let img = render(&format!("<svg width='100' height='100'>{rect}</svg>"));
        assert_eq!(pixel(&img, 50, 50), GREEN, "in the circle: {rect}");
        assert_eq!(pixel(&img, 4, 4), WHITE, "the corner: {rect}");
        assert_eq!(pixel(&img, 96, 4), WHITE, "the other corner: {rect}");
    }
}

/// Another document's `<filter>` on a box and on an element of an inline `<svg>` (its `filter`
/// attribute, which the style engine does not see).
#[test]
fn another_documents_filter_applies() {
    let img = render("<div style='width:100px;height:100px;background:green;filter:url(shapes.svg#flood)'></div>");
    assert_eq!(pixel(&img, 50, 50), BLUE, "the box");
    assert_eq!(pixel(&img, 150, 50), WHITE, "beside it");
    let img = render(
        "<svg width='100' height='100'><rect width='100' height='100' fill='green' filter='url(shapes.svg#flood)'/></svg>",
    );
    assert_eq!(pixel(&img, 50, 50), BLUE, "the rect");
}

/// `mask-image: url(shapes.svg#top)` names another document's `<mask>`: the mask is the layer,
/// not the document's own drawing (nothing), on a box and on an element of an inline `<svg>`.
#[test]
fn another_documents_mask_is_the_mask_layer() {
    let img = render("<div style='width:100px;height:100px;background:green;mask-image:url(shapes.svg#top)'></div>");
    assert_eq!(pixel(&img, 50, 25), GREEN, "the masked-in half");
    assert_eq!(pixel(&img, 50, 75), WHITE, "the masked-out half");
    let img = render(
        "<svg width='100' height='100'><rect width='100' height='100' fill='green' mask='url(shapes.svg#top)'/></svg>",
    );
    assert_eq!(pixel(&img, 50, 25), GREEN, "the rect's masked-in half");
    assert_eq!(pixel(&img, 50, 75), WHITE, "its masked-out half");
}
