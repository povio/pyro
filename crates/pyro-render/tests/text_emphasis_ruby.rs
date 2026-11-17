//! `text-emphasis` marks and ruby annotations (`<ruby>`/`<rt>`) are drawn alike, as CSS Text
//! Decoration 3 defines the marks: "exactly as if each character was assigned the mark as its
//! ruby annotation text". Each is laid out on a line of its own and drawn with its content area
//! against its base's, centred over the base; marks skip spaces and punctuation, an annotation of
//! a base that already has one goes outside it, and both take room in their line where its line
//! height leaves too little. The checks compare renderings with each other, with the Inter subset
//! as the only font, so they hold whatever fonts the system has.
use std::sync::Arc;

use pyro_render::*;

/// Serves the Inter subset (`tests/fonts`) as `inter.ttf`.
struct InterFetch;
impl Fetcher for InterFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(if url.ends_with("/inter.ttf") {
            Ok(include_bytes!("fonts/Inter-subset.ttf").to_vec())
        } else {
            Err("offline".into())
        });
    }
}

/// `body` in 24px Inter on white, 400x200 CSS px.
fn render(body: &str, scale: f64) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(InterFetch);
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><style>@font-face {{ font-family: Inter; src: url(inter.ttf) }}
             body {{ margin: 8px; background: #fff; color: #000; font: 24px Inter }}</style>{body}"
        ),
        base_url: "http://test/".into(),
        width: 400,
        height: 200,
        device_scale_factor: scale,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn same(a: &RgbaImage, b: &RgbaImage) -> bool {
    a.width == b.width && a.height == b.height && a.data == b.data
}

/// The rows of `img` with a pixel darker than mid grey.
fn inked_rows(img: &RgbaImage) -> usize {
    img.data.chunks_exact(img.width as usize * 4).filter(|row| row.chunks_exact(4).any(|p| p[0] < 128)).count()
}

/// The first row of `img` with a pixel darker than mid grey.
fn top_inked_row(img: &RgbaImage) -> usize {
    img.data
        .chunks_exact(img.width as usize * 4)
        .position(|row| row.chunks_exact(4).any(|p| p[0] < 128))
        .unwrap_or(usize::MAX)
}

/// A mark over each character is drawn exactly where an `<rt>` of the same mark over each
/// character is.
#[test]
fn emphasis_marks_are_drawn_as_ruby_annotations() {
    for scale in [1.0, 2.0] {
        let marks = render("<div style='line-height:5;text-emphasis:\"x\"'>MONITOR</div>", scale);
        let ruby = render(
            "<div style='line-height:5'><ruby>M<rt>x</rt>O<rt>x</rt>N<rt>x</rt>I<rt>x</rt>T<rt>x</rt>O<rt>x</rt>R<rt>x</rt></ruby></div>",
            scale,
        );
        let plain = render("<div style='line-height:5'>MONITOR</div>", scale);
        assert!(same(&marks, &ruby), "scale {scale}: the marks are the annotations");
        assert!(!same(&marks, &plain), "scale {scale}: the marks are drawn");
    }
}

/// Spaces and punctuation take no mark (the reference marks the letters alone).
#[test]
fn emphasis_marks_skip_spaces_and_punctuation() {
    let marks = render("<div style='line-height:5;text-emphasis:\"x\"'>MO, NI.</div>", 1.0);
    let ruby = render(
        "<div style='line-height:5'><ruby>M<rt>x</rt>O<rt>x</rt></ruby>, <ruby>N<rt>x</rt>I<rt>x</rt></ruby>.</div>",
        1.0,
    );
    assert!(same(&marks, &ruby));
}

/// `text-emphasis-position: auto` is `under right` for Chinese, `over right` for the rest.
#[test]
fn emphasis_marks_go_under_chinese_text() {
    let page = |lang: &str, position: &str| {
        render(&format!("<div lang={lang} style='line-height:5;text-emphasis:\"x\";text-emphasis-position:{position}'>MON</div>"), 1.0)
    };
    assert!(same(&page("zh", "auto"), &page("zh", "under right")));
    assert!(same(&page("ZH-tw", "auto"), &page("zh", "under right")), "language tags in any case");
    assert!(same(&page("ja", "auto"), &page("ja", "over right")));
    assert!(!same(&page("zh", "auto"), &page("ja", "auto")));
}

/// An emphasis mark over a base with a ruby annotation goes outside the annotation, as an
/// annotation of a ruby base with an annotation of its own does (nested ruby).
#[test]
fn an_emphasis_mark_goes_outside_a_ruby_annotation() {
    let marks = render(
        "<div style='line-height:5'><span style='text-emphasis:\"x\"'><ruby>M<rt>A</rt></ruby></span></div>",
        1.0,
    );
    let nested = render("<div style='line-height:5'><ruby><ruby>M<rt>A</rt></ruby><rt>x</rt></ruby></div>", 1.0);
    let annotated = render("<div style='line-height:5'><ruby>M<rt>A</rt></ruby></div>", 1.0);
    assert!(same(&marks, &nested));
    assert!(top_inked_row(&marks) < top_inked_row(&annotated), "the mark is above the annotation");
}

/// Marks and annotations take room in a line whose line height leaves none for them: the line
/// box grows by their content area, as it does for both alike.
#[test]
fn emphasis_marks_and_annotations_take_room_in_the_line() {
    for scale in [1.0, 2.0] {
        let page = |content: &str, emphasis: &str| {
            render(&format!("<div style='line-height:1;width:300px;background:#000;{emphasis}'>{content}</div>"), scale)
        };
        let plain = page("MONITOR", "");
        let marks = page("MONITOR", "text-emphasis:\"x\"");
        let ruby =
            page("<ruby>M<rt>x</rt>O<rt>x</rt>N<rt>x</rt>I<rt>x</rt>T<rt>x</rt>O<rt>x</rt>R<rt>x</rt></ruby>", "");
        assert!(inked_rows(&marks) > inked_rows(&plain), "scale {scale}: the line grows");
        assert_eq!(inked_rows(&marks), inked_rows(&ruby), "scale {scale}: by as much for both");
    }
}

/// `text-emphasis: none` (and an `<rt>`, which the UA sheet gives it) draws no marks.
#[test]
fn no_marks_without_emphasis() {
    let none = render("<div style='line-height:5;text-emphasis:none'>MONITOR</div>", 1.0);
    let plain = render("<div style='line-height:5'>MONITOR</div>", 1.0);
    assert!(same(&none, &plain));
}

/// A mark is centred on its character, not on the character and the letter spacing after it
/// (WPT css-text `letter-spacing-211`): a spaced letter has its mark where the letter alone has it.
#[test]
fn emphasis_marks_are_centred_without_the_letter_spacing() {
    let spaced = render("<p style='letter-spacing:1em;text-emphasis:dot;margin:0'>A</p>", 1.0);
    let plain = render("<p style='text-emphasis:dot;margin:0'>A</p>", 1.0);
    let unmarked = render("<p style='letter-spacing:1em;margin:0'>A</p>", 1.0);
    assert!(!same(&spaced, &unmarked), "the mark is drawn");
    assert!(same(&spaced, &plain));
}

/// `-webkit-text-stroke` outlines the glyphs, even a stroke thinner than a pixel.
#[test]
fn text_stroke_outlines_glyphs() {
    let stroked = render("<p style='color:#fff;-webkit-text-stroke:0.1px #000'>H</p>", 1.0);
    let thick = render("<p style='color:#fff;-webkit-text-stroke:2px #000'>H</p>", 1.0);
    let plain = render("<p style='color:#fff'>H</p>", 1.0);
    assert!(!same(&stroked, &plain));
    assert!(!same(&stroked, &thick));
}
