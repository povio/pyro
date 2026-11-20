//! The `alignment-baseline` longhand (and an inherited `dominant-baseline`) align a span's baseline
//! of that name with its parent's (css-inline-3, as Blink does), while `vertical-align`'s keywords
//! keep CSS 2.1's alignments: `alignment-baseline: middle` on a span of the parent's font moves
//! nothing, `vertical-align: middle` moves it.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn render(body: &str) -> Vec<u8> {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><style>body{{margin:0;font:60px/2 monospace}}</style>{body}"),
        base_url: "http://test/".into(),
        width: 300,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0.data
}

#[test]
fn bottom_aligned_atom_reaches_fractional_line_end() {
    let data = render("<div style='margin-top:0.4px;font:16px/19.2px serif;background:red'>\
        <span style='display:inline-block;vertical-align:bottom;width:100%;height:19.2px;background:green'></span></div>");
    assert_eq!(&data[(19 * 300 + 10) * 4..][..3], &[0, 128, 0]);
}

#[test]
fn the_longhand_aligns_baselines_of_its_name() {
    let plain = render("<div>X<span>X</span></div>");
    for value in ["middle", "text-top", "text-bottom"] {
        assert!(
            render(&format!("<div>X<span style='alignment-baseline:{value}'>X</span></div>")) == plain,
            "alignment-baseline: {value}"
        );
        assert!(
            render(&format!("<div style='dominant-baseline:{value}'>X<span>X</span></div>")) == plain,
            "dominant-baseline: {value}"
        );
    }
    assert!(render("<div>X<span style='vertical-align:middle'>X</span></div>") != plain, "vertical-align: middle");
    assert!(render("<div>X<span style='vertical-align:text-top'>X</span></div>") != plain, "vertical-align: text-top");
}

/// A baseline shift of a negative half pixel rounds up, as Blink snaps positions: an empty
/// inline-block (its baseline is its bottom margin edge) lowered by `vertical-align: -7.5px` and
/// lifted back by `margin-bottom: 7.5px` sits exactly where one with neither does (WPT CSS2
/// `visudet/inline-block-baseline-011`, `-014`). Rounding halves away from zero put it a pixel low.
#[test]
fn a_negative_half_pixel_shift_rounds_up() {
    let square = "display:inline-block;width:15px;height:15px;background:#00f";
    let top = |data: &[u8]| (0..200).find(|&y| (0..300).any(|x| data[(y * 300 + x) * 4..][..3] == [0, 0, 255]));
    let plain = render(&format!("<p style='font-size:15px;line-height:5;margin:0'>A <span style='{square}'></span></p>"));
    let shifted = render(&format!(
        "<p style='font-size:15px;line-height:5;margin:0'>A <span style='{square};vertical-align:-7.5px;margin-bottom:7.5px'></span></p>"
    ));
    assert!(top(&plain).is_some());
    assert_eq!(top(&shifted), top(&plain));
}
