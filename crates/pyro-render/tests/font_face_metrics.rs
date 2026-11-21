//! `@font-face` `size-adjust`, `ascent-override`, `descent-override` and `line-gap-override` (CSS
//! Fonts 5 §4.9-§4.11): a face is used at its size times `size-adjust` (glyphs, advances and
//! metrics), and its metrics can be replaced by fractions of that size. Pyro ignored all four
//! (WPT css/css-fonts/size-adjust-*, ascent-descent-override, line-gap-override). Next.js and
//! other font loaders size their fallback fonts with them.
use std::sync::Arc;

use pyro_render::*;

/// Ahem: every glyph a box of one em, ascent 0.8 em, descent 0.2 em, no line gap.
const AHEM: &[u8] = include_bytes!("fonts/Ahem.ttf");

struct Fonts;
impl Fetcher for Fonts {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        if url.ends_with("/ahem.ttf") {
            done(Ok(AHEM.to_vec()))
        } else {
            done(Err("offline".into()))
        }
    }
}

/// Renders `body` (with `faces`, each an `@font-face` rule body for family `F<i>`) and gives, for
/// each colour in `colors`, the height and width of the box its pixels span.
fn extents(faces: &[&str], body: &str, colors: &[[u8; 3]]) -> Vec<(usize, usize)> {
    let faces: String = faces
        .iter()
        .enumerate()
        .map(|(i, rule)| format!("@font-face {{ font-family: F{i}; src: url(ahem.ttf); {rule} }}"))
        .collect();
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><style>{faces} body {{ margin: 0; background: #fff }} div {{ width: 10px; color: transparent }}</style>{body}"
        ),
        base_url: "http://test/".into(),
        width: 400,
        height: 400,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(Fonts);
    let img = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    let (w, h) = (img.width as usize, img.height as usize);
    colors
        .iter()
        .map(|rgb| {
            let hits: Vec<(usize, usize)> = (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .filter(|&(x, y)| img.data[(y * w + x) * 4..(y * w + x) * 4 + 3] == rgb[..])
                .collect();
            if hits.is_empty() {
                return (0, 0);
            }
            let (x0, x1) = (hits.iter().map(|p| p.0).min().unwrap(), hits.iter().map(|p| p.0).max().unwrap());
            let (y0, y1) = (hits.iter().map(|p| p.1).min().unwrap(), hits.iter().map(|p| p.1).max().unwrap());
            (y1 - y0 + 1, x1 - x0 + 1)
        })
        .collect()
}

const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const GREEN: [u8; 3] = [0, 128, 0];
const YELLOW: [u8; 3] = [255, 255, 0];

#[test]
fn fallback_runs_share_their_text_items_alignment_width() {
    let pairs = "ab".repeat(1024);
    let snapshot = Snapshot {
        html: format!("<!doctype html><style>@font-face{{font-family:A;src:url(ahem.ttf);unicode-range:U+61}}@font-face{{font-family:B;src:url(ahem.ttf);unicode-range:U+62}}body{{margin:0}}div{{width:30000px;margin-left:-29600px;white-space:nowrap;text-align:right;font:10px A,B;letter-spacing:.003px}}i{{display:inline-block;width:4px;height:10px;background:red}}</style><div>{pairs}<i></i></div>"),
        base_url: "http://test/".into(), width: 400, height: 40, device_scale_factor: 1.0, ..Default::default()
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(Fonts);
    let image = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    let red: Vec<_> = (0..400).filter(|&x| (0..40).any(|y| {
        let index = (y * 400 + x) * 4;
        image.data[index..index + 3] == RED
    })).collect();
    assert_eq!(red, [396, 397, 398, 399]);
}

#[test]
fn size_adjust_scales_the_glyphs_of_its_face_only() {
    // 20px text: the plain face's box is 20px, the one at 150% 30px, a fallback for a character
    // outside the adjusted face's range keeps the element's size.
    let e = extents(
        &["", "size-adjust: 150%", "size-adjust: 150%; unicode-range: U+58"],
        "<div style='font: 20px/50px F0; color: #f00; width: auto'>XX</div>\
         <div style='font: 20px/50px F1; color: #00f; width: auto'>XX</div>\
         <div style='font: 20px/50px F2, F0; color: #008000; width: auto'>X</div>",
        &[RED, BLUE, GREEN],
    );
    assert_eq!(e[0], (20, 40), "{e:?}");
    assert_eq!(e[1], (30, 60), "{e:?}");
    assert_eq!(e[2], (30, 30), "{e:?}");
}

#[test]
fn metric_overrides_set_the_normal_line_height() {
    // Ahem's normal line height is its em (0.8 + 0.2); the overrides make it 1 + 0.5, a line gap
    // of a whole em doubles it, and overrides are fractions of the adjusted size (0.5 × 40px).
    let e = extents(
        &[
            "",
            "ascent-override: 100%; descent-override: 50%",
            "line-gap-override: 100%",
            "size-adjust: 200%; ascent-override: 50%; descent-override: 0%",
        ],
        "<div style='font: 20px F0; background: #f00'>X</div>\
         <div style='font: 20px F1; background: #00f'>X</div>\
         <div style='font: 20px F2; background: #008000'>X</div>\
         <div style='font: 20px F3; background: #ff0'>X</div>",
        &[RED, BLUE, GREEN, YELLOW],
    );
    assert_eq!(e[0].0, 20, "{e:?}");
    assert_eq!(e[1].0, 30, "{e:?}");
    assert_eq!(e[2].0, 40, "{e:?}");
    assert_eq!(e[3].0, 20, "{e:?}");
}

/// `ex`, `ch` and `ic` under `font-size-adjust` measure the font at the adjusted size (`em` stays
/// the computed one), and a font without the named metric takes the spec's fallback (WPT
/// css/css-fonts/font-size-adjust-units-001, -order-001, -011).
#[test]
fn font_relative_units_use_the_adjusted_size() {
    // Ahem's x-height is 0.8 em: `font-size-adjust: 0.4` draws 40px Ahem at 20px, where `1ch` is
    // 20px and `1ex` 16px. Ahem has no U+6C34: `ic-width 1` takes its ic as 1em, a size of 40px.
    let e = extents(
        &[""],
        "<div style='font: 40px F0; font-size-adjust: 0.4; width: 3ch; height: 2ex; background: #00f'></div>\
         <div style='font: 40px F0; font-size-adjust: ic-width 1; width: 1ch; height: 1em; background: #f00'></div>",
        &[BLUE, RED],
    );
    assert_eq!(e[0], (32, 60), "{e:?}");
    assert_eq!(e[1], (40, 40), "{e:?}");
}

#[test]
fn font_load_updates_boxes_sized_from_root_and_pseudo_font_metrics() {
    let sizes = extents(
        &[""],
        "<style>
           html { font: 20px F0, monospace }
           #root-units { background: red; width: 2rch; height: 3rlh }
           #pseudo::before { content: ' '; display: block; font: 25px F0, monospace;
                             background: blue; width: 3ch; height: 2lh }
         </style><div id='root-units'></div><div id='pseudo'></div>",
        &[RED, BLUE],
    );
    assert_eq!(sizes, [(60, 40), (50, 75)]);
}
