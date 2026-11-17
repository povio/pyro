//! `::marker` (CSS Lists 3 § 3.1, CSS Pseudo-Elements 4): a list item's marker takes the style of
//! the item's `::marker` (colour, font, shadows) and its `content` (strings, the item's number,
//! quotes; `none` leaves no marker); `::before::marker` / `::after::marker` style the marker of a
//! pseudo-element's list item box.
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
            "<!doctype html><style>body{{margin:0;background:white;font:40px/60px monospace;color:black}}
             ul,ol{{margin:0;padding-left:200px}}</style>{body}"
        ),
        base_url: "http://test/".into(),
        width: 600,
        height: 300,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// The columns of the image that have a pixel of `color`.
fn columns(img: &RgbaImage, color: [u8; 3]) -> Vec<u32> {
    (0..img.width)
        .filter(|&x| {
            (0..img.height).any(|y| {
                let i = (y as usize * img.width as usize + x as usize) * 4;
                [img.data[i], img.data[i + 1], img.data[i + 2]] == color
            })
        })
        .collect()
}

#[test]
fn a_marker_has_the_colour_of_its_pseudo_element() {
    for list in ["<ul><li>x</li></ul>", "<ol><li>x</li></ol>", "<ul style='list-style-position:inside'><li>x</li></ul>"]
    {
        let img = render(&format!("<style>li::marker{{color:#00f}}</style>{list}"));
        let blue = columns(&img, [0, 0, 255]);
        assert!(!blue.is_empty(), "{list}: the marker is blue");
        assert!(!columns(&img, [0, 0, 0]).is_empty(), "{list}: the text stays black");
    }
    // Without a rule the marker has the item's colour.
    let img = render("<ul style='color:#00f'><li>x</li></ul>");
    assert!(columns(&img, [0, 0, 0]).is_empty());
}

#[test]
fn marker_content_replaces_the_marker() {
    let plain = columns(&render("<style>li::marker{color:#00f}</style><ol><li>x</li></ol>"), [0, 0, 255]);
    let text = columns(&render("<style>li::marker{color:#00f;content:'MMMM'}</style><ol><li>x</li></ol>"), [0, 0, 255]);
    assert!(text.len() > plain.len() + 40, "four letters are wider than `1.`: {} {}", text.len(), plain.len());
    assert!(text.iter().all(|&x| x < 200), "outside the item");
    let none = columns(&render("<style>li::marker{color:#00f;content:none}</style><ol><li>x</li></ol>"), [0, 0, 255]);
    assert!(none.is_empty(), "{none:?}");
    // The item's number and quotes.
    let counter = columns(
        &render("<style>li::marker{color:#00f;content:open-quote counter(list-item) close-quote}</style><ol><li>x</li></ol>"),
        [0, 0, 255],
    );
    assert!(counter.len() > plain.len(), "{} {}", counter.len(), plain.len());
}

#[test]
fn the_marker_of_a_pseudo_elements_list_item_is_styled() {
    let img = render(
        "<style>div{margin-left:200px} div::after{content:'x';display:list-item}
         div::after::marker{content:attr(data-mark);color:#00f}</style><div data-mark='MM'></div>",
    );
    let blue = columns(&img, [0, 0, 255]);
    assert!(blue.len() > 20 && blue.iter().all(|&x| x < 200), "{blue:?}");
}

/// A `::marker` rule's `text-transform` transforms the marker's string, inside and outside the
/// item; the item's own `text-transform` does not reach its marker (Blink's UA sheet sets
/// `text-transform: none` on `::marker`).
#[test]
fn marker_text_transform_comes_from_marker_rules() {
    for position in ["outside", "inside"] {
        let list = |style: &str, ty: &str| {
            render(&format!(
                "<style>li::marker{{color:#00f}} {style}</style><ol style='list-style:{ty} {position}'><li>x</li><li>x</li><li>x</li></ol>"
            ))
        };
        let upper = columns(&list("", "upper-roman"), [0, 0, 255]);
        assert_eq!(
            columns(&list("li::marker{text-transform:uppercase}", "lower-roman"), [0, 0, 255]),
            upper,
            "{position}"
        );
        let lower = columns(&list("", "lower-roman"), [0, 0, 255]);
        assert_ne!(lower, upper);
        assert_eq!(columns(&list("li{text-transform:uppercase}", "lower-roman"), [0, 0, 255]), lower, "{position}");
    }
}

/// The text properties CSS Pseudo 4 allows on `::marker` (`word-spacing`, `letter-spacing`, ...)
/// reach the marker from a `::marker` rule as when inherited; a symbol marker keeps its symbol
/// and is not spaced by them.
#[test]
fn marker_rules_space_the_marker_text() {
    let list = |style: &str, ty: &str| {
        render(&format!(
            "<style>li::marker{{color:#00f}} ul{{list-style:{ty} inside}} {style}</style><ul><li>x</li></ul>"
        ))
    };
    let blue = |img: &RgbaImage| columns(img, [0, 0, 255]);
    let plain = list("", "'a b'");
    let inherited = list("ul{word-spacing:40px}", "'a b'");
    let explicit = list("li::marker{word-spacing:40px}", "'a b'");
    assert_ne!(blue(&inherited), blue(&plain));
    assert_eq!(blue(&explicit), blue(&inherited));
    // A disc and its space are not spaced, and stay a disc under a `::marker` rule.
    let disc = list("", "disc");
    assert_eq!(blue(&list("li::marker{word-spacing:40px;letter-spacing:20px}", "disc")), blue(&disc));
}

/// An item with only an inside symbol marker has a line for it.
#[test]
fn an_empty_item_has_the_line_of_its_inside_symbol_marker() {
    let img = render("<style>li::marker{color:#00f}</style><ul style='list-style:disc inside'><li></li></ul>");
    assert!(!columns(&img, [0, 0, 255]).is_empty());
}

/// An outside marker is painted before the item's text, which covers it where they overlap.
#[test]
fn text_covers_an_outside_marker() {
    let img = render(
        "<style>li::marker{color:#00f} li{text-indent:-200px} x{background:#000}</style><ol><li><x>XXXXXXXX</x></li></ol>",
    );
    assert!(columns(&img, [0, 0, 255]).is_empty());
}
