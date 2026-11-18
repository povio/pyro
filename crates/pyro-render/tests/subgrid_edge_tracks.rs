//! A subgrid's margin, border and padding at an edge where none of its items lies go into its edge
//! track alone, not across the empty tracks before it (Chromium; Taffy grid `subgrid`
//! `collect_virtual_items` edge spacers). WPT css-grid `subgrid/grid-gap-002`,
//! `orthogonal-writing-mode-002`, `-003`. A percentage gap of a subgrid counts as zero while its
//! parent sizes the tracks and resolves against the subgrid's content box after (`subgrid/grid-gap-008`).
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
        width: 200,
        height: 200,
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
const BLUE: [u8; 3] = [0, 0, 255];

/// Rows 2-4 hold a subgrid with one 20px item in its first row and a 40px bottom border: rows 2
/// and 4 are 20px and 40px, row 3 stays empty, so a blue item in row 3 has no height and the
/// green one in row 5 starts right after the border, 60px down.
#[test]
fn subgrid_end_border_goes_into_its_last_track() {
    let img = render(
        "<div style='display:grid;grid-template-columns:20px 20px;place-content:start'>\
           <div style='grid-row:1;grid-column:1;height:0'></div>\
           <div style='grid-row:3;grid-column:1;background:blue;min-height:0'></div>\
           <div style='grid-row:5;grid-column:1;height:10px;background:green'></div>\
           <div style='display:grid;grid-template-rows:subgrid;grid-row:2/span 3;grid-column:2;border-bottom:40px solid #ccc'>\
             <div style='height:20px;background:red'></div>\
           </div>\
         </div>",
    );
    for y in 0..60 {
        assert_ne!(pixel(&img, 5, y), BLUE, "row 3 is empty, y = {y}");
    }
    assert_eq!(pixel(&img, 5, 59), WHITE);
    assert_eq!(pixel(&img, 5, 62), [0, 128, 0], "row 5 starts after the 40px border");
}

/// A subgrid's percentage row gap counts as zero while its parent sizes the rows (two 20px rows),
/// then resolves against the subgrid's content height (20% of 40px = 8px): the second item starts
/// 4px into its row, past half the gap, so its blue background begins at y = 24 (a gap of 20% of
/// the 100px column would put it at y = 40).
#[test]
fn subgrid_percentage_gap_resolves_against_its_final_size() {
    let img = render(
        "<div style='display:grid;grid-template-columns:100px;place-content:start'>\
           <div style='display:grid;grid-template-rows:subgrid;grid-row:1/span 2;row-gap:20%'>\
             <div style='height:20px'></div>\
             <div style='height:20px;background:blue;align-self:start'></div>\
           </div>\
         </div>",
    );
    assert_eq!(pixel(&img, 5, 22), WHITE, "half the gap before the second item");
    assert_eq!(pixel(&img, 5, 25), BLUE);
}

/// An item of a nested subgrid is measured against the rows of the subgrid that has rows of its own
/// (four 1fr rows in a 100px row, 25px each): its vertical text "X X X X" wraps into four 25px columns,
/// so the min-content grid is 100px wide (WPT css-grid `subgrid/standalone-axis-size-011`).
#[test]
fn nested_subgrid_item_is_measured_against_its_standalone_rows() {
    let img = render(
        "<div style='display:grid;width:min-content;height:100px;background:blue;font:25px/1 monospace;color:transparent'>\
           <div style='display:grid;grid-template-columns:subgrid;grid-template-rows:100px'>\
             <div style='display:grid;grid-template-columns:subgrid;grid-template-rows:repeat(4,1fr)'>\
               <div style='writing-mode:vertical-lr;line-height:25px'>X X X X</div>\
             </div>\
           </div>\
         </div>",
    );
    assert_eq!(pixel(&img, 60, 50), BLUE, "the grid is wider than one 25px column");
}
