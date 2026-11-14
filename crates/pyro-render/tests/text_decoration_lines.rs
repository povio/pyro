//! Text decoration lines are placed and painted as Chromium paints them: in whole CSS pixels
//! from the baseline at any device scale (an underline `max(1, ceil(t / 2))` below it, a line
//! as thick as its thickness cut to a whole CSS pixel), underlines and overlines under the text
//! and line-throughs over it, the decorations of a block on the text of its in-flow descendants,
//! `text-decoration-inset` percentages of the line, `text-decoration-skip-ink` gaps where glyphs
//! cross a line, text shadows under the decorations too, and a box's thick decorations drawn
//! where they reach into a clip its box is outside of. The Inter subset is the only font.
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

/// `body` in Inter on white, 400x200 CSS px; `style` is added to the body's rules.
fn render(style: &str, body: &str, scale: f64) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(InterFetch);
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><style>@font-face {{ font-family: Inter; src: url(inter.ttf) }}
             body {{ margin: 8px; background: #fff; color: #000; font: 20px Inter; {style} }}</style>{body}"
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

fn red(img: &RgbaImage, x: usize, y: usize) -> u8 {
    img.data[(y * img.width as usize + x) * 4]
}

/// The red channel of column `x`, top to bottom.
fn column(img: &RgbaImage, x: usize) -> Vec<u8> {
    (0..img.height as usize).map(|y| red(img, x, y)).collect()
}

/// An underline of transparent text is a run of fully covered rows (no partly covered edge
/// rows), as thick as a tenth of the font size cut to whole CSS pixels, at every device scale.
#[test]
fn an_underline_is_whole_css_pixels() {
    for scale in [1.0, 2.0, 3.0] {
        for size in [13, 16, 20, 24, 32] {
            let img = render(
                "",
                &format!(
                    "<span style='font-size:{size}px;color:transparent;text-decoration:underline #000'>MONITOR</span>"
                ),
                scale,
            );
            let rows = column(&img, (12.0 * scale) as usize);
            let inked: Vec<usize> = rows.iter().enumerate().filter(|(_, v)| **v < 255).map(|(y, _)| y).collect();
            assert!(!inked.is_empty(), "{size}px at {scale}: an underline");
            assert!(inked.iter().all(|&y| rows[y] == 0), "{size}px at {scale}: no partly covered rows: {inked:?}");
            let thickness = ((size as f64 / 10.0).max(1.0).trunc() * scale) as usize;
            assert_eq!(inked.len(), thickness, "{size}px at {scale}: rows {inked:?}");
            assert_eq!(inked[0] % scale as usize, 0, "{size}px at {scale}: on a whole CSS pixel");
        }
    }
}

/// An underline goes under the text and a line-through over it (CSS 2 Appendix E): where a thick
/// green underline at the baseline crosses the descender of black "q", the glyph shows; where a
/// thick green line-through crosses "M", the line shows.
#[test]
fn underlines_go_under_the_text_and_line_throughs_over_it() {
    let line = |kind: &str| {
        render(
            "text-decoration-skip-ink:none",
            &format!("<span style='font-size:60px;text-decoration:{kind} #0f0;text-decoration-thickness:12px;text-underline-offset:0'>qM</span>"),
            1.0,
        )
    };
    let plain = render("", "<span style='font-size:60px'>qM</span>", 1.0);
    let underlined = line("underline");
    let struck = line("line-through");
    // Black pixels of the plain text: under the underline they stay black (the text is on top),
    // under the line-through some turn green.
    let (mut kept, mut covered) = (0, 0);
    for y in 0..plain.height as usize {
        for x in 0..plain.width as usize {
            if red(&plain, x, y) == 0 {
                let i = (y * plain.width as usize + x) * 4;
                kept += usize::from(underlined.data[i + 1] == 0);
                covered += usize::from(struck.data[i] == 0 && struck.data[i + 1] == 255);
            }
        }
    }
    let black = plain.data.chunks_exact(4).filter(|p| p[0] == 0).count();
    assert_eq!(kept, black, "every black pixel of the text stays black over the underline");
    assert!(covered > 0, "the line-through covers some of the text");
}

/// A block's decoration is drawn on the text of its in-flow descendants, not into an inline
/// block, a float or an absolutely positioned box, and through a `display: contents` element.
#[test]
fn decorations_propagate_to_in_flow_descendants() {
    let page = |body: &str| render("", body, 1.0);
    assert!(same(
        &page("<div style='text-decoration:underline'><p>MONITOR</p></div>"),
        &page("<div><p style='text-decoration:underline'>MONITOR</p></div>"),
    ));
    assert!(same(
        &page("<div style='text-decoration:underline'><div style='display:contents'><p>MONITOR</p></div></div>"),
        &page("<div><p style='text-decoration:underline'>MONITOR</p></div>"),
    ));
    assert!(same(
        &page("<div style='display:table;text-decoration:underline'><div style='display:table-row-group'><div style='display:table-row'><div style='display:table-cell'>MONITOR</div></div></div></div>"),
        &page("<div style='display:table'><div style='display:table-row-group'><div style='display:table-row'><div style='display:table-cell;text-decoration:underline'>MONITOR</div></div></div></div>"),
    ));
    for stops in ["display:inline-block", "float:left", "position:absolute"] {
        assert!(
            same(
                &page(&format!("<div style='text-decoration:underline'><span style='{stops}'>MONITOR</span></div>")),
                &page(&format!("<div><span style='{stops}'>MONITOR</span></div>")),
            ),
            "{stops} stops the decoration"
        );
    }
}

/// `text-decoration-inset` percentages are of the line's length: half of it at each end leaves
/// nothing, and a negative one extends the line.
#[test]
fn inset_percentages_are_of_the_line() {
    let page = |inset: &str| render("", &format!("<u style='text-decoration-inset:{inset}'>MONITOR</u>"), 1.0);
    let plain = render("", "<span>MONITOR</span>", 1.0);
    assert!(same(&page("50%"), &plain), "a 50% inset at each end leaves no line");
    assert!(!same(&page("-25%"), &page("0")), "a negative percentage extends the line");
    assert!(same(&page("calc(10% + 0px)"), &page("10%")));
}

/// `text-decoration-skip-ink: auto` (the initial value) leaves gaps in an underline where the
/// descenders of "q" and "y" cross it; `none` draws it whole.
#[test]
fn skip_ink_leaves_gaps_where_glyphs_cross_the_line() {
    let page = |skip: &str| {
        render(
            &format!("text-decoration-skip-ink:{skip}"),
            "<span style='font-size:40px;color:transparent;text-decoration:underline #000'>qyqyxx</span>",
            1.0,
        )
    };
    let ink = |img: &RgbaImage| img.data.chunks_exact(4).filter(|p| p[0] < 128).count();
    let (auto, none) = (page("auto"), page("none"));
    assert!(ink(&auto) < ink(&none), "gaps around the descenders");
    assert!(ink(&auto) > 0, "the line is drawn between them");
    assert!(same(&page("initial"), &auto), "auto is the initial value");
}

/// Text shadows go under the decorations as well as the text: a red shadow 6px down of black,
/// underlined text paints as the same text drawn red 6px down under the black text.
#[test]
fn text_shadows_are_under_decorations() {
    let shadowed = render(
        "",
        "<div style='position:absolute;left:20px;top:20px;text-shadow:#f00 6px 6px;text-decoration:underline'>MONITOR</div>",
        1.0,
    );
    let layered = render(
        "",
        "<div style='position:absolute;left:26px;top:26px;color:#f00;text-decoration:underline'>MONITOR</div>\
         <div style='position:absolute;left:20px;top:20px;text-decoration:underline'>MONITOR</div>",
        1.0,
    );
    assert!(same(&shadowed, &layered));
}

/// A box moved out of the clip of its parent still paints the part of its thick underline that
/// reaches into the clip (WPT's `text-decoration-thickness-underline-001`).
#[test]
fn a_thick_decoration_outside_its_boxs_clip_is_drawn() {
    let img = render(
        "",
        "<div style='overflow:hidden;width:80px;height:20px;background:#f00;line-height:1'>\
           <div style='position:relative;bottom:60px;color:transparent;text-decoration:#0f0 underline;\
             text-decoration-skip-ink:none;text-decoration-thickness:80px'>MONITOR</div></div>",
        1.0,
    );
    for x in 8..88 {
        for y in 8..28 {
            assert_eq!(red(&img, x, y), 0, "({x}, {y}) is covered by the underline");
        }
    }
}
