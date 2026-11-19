//! Review findings on the round-6 table and layout code: page-controlled spans that overflowed or
//! allocated without bound, and edge cases of the round's fixes.
use std::sync::Arc;

use pyro_render::*;

const GREEN: [u8; 3] = [0, 128, 0];

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("404".into()));
    }
}

/// `body` on a white page without margins, 400x300 CSS px.
fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff'>{body}"),
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * img.width + x) * 4) as usize;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

/// 66 cells of the legal `colspan="1000"` in one row overflowed the column cursor's `u16`
/// arithmetic (a panic on a reversed slice); spans past 1000 and junk are clamped as HTML says.
#[test]
fn huge_colspans_lay_out() {
    let cells = "<td colspan=1000></td>".repeat(66);
    let img = render(&format!(
        "<table style='border-spacing:0'><tr>{cells}<td colspan=99999></td><td colspan=-3></td></tr></table>\
         <div style='width:50px;height:50px;background:green'></div>"
    ));
    assert_eq!(pixel(&img, 10, 10), GREEN);
}

/// `colspan="0"` is `colspan="1"` (HTML § 4.9.11): the next cell is in the second column.
#[test]
fn zero_colspan_is_one() {
    let img = render(
        "<table style='border-spacing:0'><tr><td colspan=0 style='width:50px;height:50px;padding:0'></td>\
         <td style='width:50px;height:50px;padding:0;background:green'></td></tr>\
         <tr><td style='width:50px;height:50px;padding:0'></td><td style='padding:0'></td></tr></table>",
    );
    assert_eq!(pixel(&img, 75, 25), GREEN);
}

/// A collapsed-border table with one huge spanning cell and thousands of empty rows: resolving its
/// collapsed borders must not cost (rows x columns) work and memory.
#[test]
fn huge_collapsed_span_is_cheap() {
    let rows = "<tr></tr>".repeat(9999);
    let start = std::time::Instant::now();
    let img = render(&format!(
        "<table style='border-collapse:collapse'><tr><td colspan=1000 rowspan=10000 style='padding:0'></td></tr>{rows}</table>\
         <div style='width:50px;height:50px;background:green'></div>"
    ));
    assert_eq!(pixel(&img, 10, 10), GREEN);
    let elapsed = start.elapsed();
    assert!(elapsed.as_secs_f64() < 5.0, "took {elapsed:?}");
}
