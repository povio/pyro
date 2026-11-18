//! Vertical lines align atomic inlines on the central baseline and `vertical-lr` lines are
//! inverted (line-over at block-end), as in Blink (parley `Layout::set_central_baseline`,
//! blitz-dom inline layout); a file input paints its `::file-selector-button` and label; an
//! atomic inline that does not fit before a line's ellipsis is hidden. Pixel checks after WPT
//! css-inline `initial-letter-raise-initial-vlr`, css-pseudo `file-selector-button-001`,
//! css-overflow `text-overflow-010`.
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
        html: format!("<!doctype html><body style='margin:0;background:#fff'>{body}"),
        base_url: "http://test/".into(),
        width: 300,
        height: 300,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: usize, y: usize) -> [u8; 3] {
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

const WHITE: [u8; 3] = [255, 255, 255];
const GREEN: [u8; 3] = [0, 128, 0];
const BLUE: [u8; 3] = [0, 0, 255];

#[track_caller]
fn check(img: &RgbaImage, points: &[(usize, usize, [u8; 3])]) {
    for &(x, y, want) in points {
        assert_eq!(pixel(img, x, y), want, "pixel at ({x}, {y})");
    }
}

/// In `vertical-lr` the line-over side is the right (block-end) side: a box taller than the
/// rest of the line with `vertical-align: top` takes the line from its over side, and the
/// baseline-aligned content sits at the right of the 96px line, not at its left.
#[test]
fn vertical_lr_lines_are_inverted() {
    let img = render(
        "<div style='writing-mode:vertical-lr;height:200px;font:10px/24px monospace'>\
           <span style='display:inline-block;width:20px;height:30px;background:green'></span>\
           <span style='display:inline-block;width:96px;height:20px;vertical-align:top;background:blue'></span>\
         </div>",
    );
    check(&img, &[(85, 15, GREEN), (10, 15, WHITE), (50, 40, BLUE)]);
}

/// On a central baseline an empty inline-block is centred on the strut: a 50px box holds the
/// 24px strut, so the line is 50px wide and the next one ends 74px from the block start (an
/// alphabetic baseline would add the strut's descent).
#[test]
fn vertical_atomic_inlines_center_on_the_line() {
    for mode in ["vertical-lr", "vertical-rl"] {
        let img = render(&format!(
            "<div style='writing-mode:{mode};height:200px;font:10px/24px monospace;background:green;display:inline-block'>\
               <span style='display:inline-block;width:50px;height:30px'></span><br>x\
             </div>"
        ));
        check(&img, &[(73, 100, GREEN), (75, 100, WHITE)]);
    }
}

/// A file input paints its button (Chromium's "Choose file" button face) at its start.
#[test]
fn file_input_paints_its_button() {
    let img = render("<input type=file style='margin:0'>");
    assert_eq!(pixel(&img, 4, 10), [0xEF, 0xEF, 0xEF], "button face");
}

/// An inline-block that does not fit before the ellipsis of its line is not painted.
#[test]
fn atomic_inline_past_the_ellipsis_is_hidden() {
    let img = render(
        "<div style='white-space:pre;overflow:hidden;text-overflow:ellipsis;width:52px;font:10px/20px monospace;color:transparent'>\
         <span style='display:inline-block;margin-left:40px;width:10px;height:10px;background:blue'></span>          </div>",
    );
    for x in 0..60 {
        assert_ne!(pixel(&img, x, 10), BLUE, "pixel at ({x}, 10)");
    }
}
