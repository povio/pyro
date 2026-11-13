//! An inline `<svg>` without a `viewBox` lays its content out in its CSS box,
//! as Chromium does: user units are CSS px (content is not scaled to the box)
//! and percentages, a `<use>` of a `<symbol>` sized 100% by default included,
//! resolve against the box.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
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
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    Renderer::new().render(&s, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// The bounds (`[x0, y0, x1, y1]`, inclusive) of the pixels of colour `rgb`.
fn bounds(img: &RgbaImage, rgb: [u8; 3]) -> Option<[u32; 4]> {
    let mut b: Option<[u32; 4]> = None;
    for y in 0..img.height {
        for x in 0..img.width {
            let i = (y as usize * img.width as usize + x as usize) * 4;
            if img.data[i..i + 3] == rgb {
                let c = b.get_or_insert([x, y, x, y]);
                *c = [c[0].min(x), c[1].min(y), c[2].max(x), c[3].max(y)];
            }
        }
    }
    b
}

#[test]
fn a_symbol_drawn_through_use_fills_the_svg_box() {
    // The symbol's viewBox maps onto the use's viewport, the svg's 48px box: its
    // left half is the left half of the box.
    let img = render(
        r##"<div style="width:48px;height:48px"><svg style="width:100%;height:100%"><defs>
        <symbol id="half" viewBox="0 0 24 24"><rect width="12" height="24" fill="#ff0000"/></symbol>
        </defs><use href="#half"/></svg></div>"##,
    );
    assert_eq!(bounds(&img, [255, 0, 0]), Some([0, 0, 23, 47]));
}

#[test]
fn content_is_in_css_px_and_percentages_resolve_against_the_box() {
    let img = render(
        r##"<svg width="24" height="24" style="width:48px;height:48px"><rect width="12" height="12" fill="#0000ff"/></svg>
        <svg style="width:40px;height:20px"><rect width="50%" height="100%" fill="#00ff00"/></svg>"##,
    );
    assert_eq!(bounds(&img, [0, 0, 255]), Some([0, 0, 11, 11]), "12 user units are 12px, not scaled to the box");
    assert_eq!(bounds(&img, [0, 255, 0]), Some([0, 48, 19, 67]), "50% of the 40px box");
}
