//! A box that clips its overflow around an empty content box still paints its borders (an `<hr>`,
//! `overflow: hidden` in the UA sheet), and a table's `max-width` does not make it narrower than
//! its min-content width (CSS 2.1 § 17.5.2).
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
        html: format!("<!doctype html><style>body{{margin:0;background:white}}</style>{body}"),
        base_url: "http://test/".into(),
        width: 300,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// The columns of the image that have a pixel of `color`.
fn columns(img: &RgbaImage, color: [u8; 3]) -> usize {
    (0..img.width)
        .filter(|&x| {
            (0..img.height).any(|y| {
                let i = (y as usize * img.width as usize + x as usize) * 4;
                [img.data[i], img.data[i + 1], img.data[i + 2]] == color
            })
        })
        .count()
}

#[test]
fn an_empty_box_that_clips_paints_its_borders() {
    let img = render("<hr style='border:1px solid #00f'>");
    assert!(columns(&img, [0, 0, 255]) > 200, "the rule is painted");
    let img = render("<div style='overflow:hidden;border-top:1px solid #00f;height:0'></div>");
    assert!(columns(&img, [0, 0, 255]) > 200, "so is an empty clipping box's border");
}

#[test]
fn a_tables_max_width_does_not_cut_its_min_content_width() {
    let img = render(
        "<div style='width:min-content;background:#00f'><div style='display:table;max-width:30px'>
         <div style='width:60px;height:10px'></div></div></div>",
    );
    assert_eq!(columns(&img, [0, 0, 255]), 60);
    // A fixed table with a `width` keeps to its `max-width` above its min-content width.
    let img = render(
        "<div style='display:table;table-layout:fixed;width:200px;max-width:50px;background:#00f;height:10px'></div>",
    );
    assert_eq!(columns(&img, [0, 0, 255]), 50);
}

#[test]
fn a_double_collapsed_border_is_two_lines() {
    let table = |style: &str| {
        render(&format!(
            "<table style='border-collapse:collapse;border:6px {style} #00f'><td style='width:50px;height:20px'></td></table>"
        ))
    };
    let (double, solid) = (table("double"), table("solid"));
    let blue = |img: &RgbaImage| img.data.chunks(4).filter(|p| p[..3] == [0, 0, 255]).count();
    assert!(blue(&double) * 4 < blue(&solid) * 3, "{} {}", blue(&double), blue(&solid));
}
