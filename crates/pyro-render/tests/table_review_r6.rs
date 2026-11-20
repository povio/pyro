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

/// A collapsed middle column puts its two grid lines on one coordinate: the wider border there
/// wins, the narrower one does not stripe it.
#[test]
fn collapsed_column_edges_have_one_winner() {
    let img = render(
        "<table style='border-collapse:collapse'><col><col style='visibility:collapse'><col>\
         <tr><td style='width:50px;height:50px;padding:0;border-right:10px solid green'></td>\
         <td style='width:50px;height:50px;padding:0;border:none'></td>\
         <td style='width:50px;height:50px;padding:0;border-left:4px solid red'></td></tr></table>",
    );
    // The shared edge is 10px wide around x = 5 + 50 + 5 (the first cell's half border included).
    let row: Vec<[u8; 3]> = (40..80).map(|x| pixel(&img, x, 25)).collect();
    assert!(!row.contains(&[255, 0, 0]), "red stripes the green edge: {row:?}");
    assert!(row.contains(&GREEN), "no green edge: {row:?}");
}

/// In a fixed table, a cell's percentage padding resolves against the table's grid width (here
/// 200px: 10% is 20px), whose columns the fixed layout makes `minmax(W, W fr)` tracks.
#[test]
fn fixed_table_cell_percent_padding() {
    let img = render(
        "<table style='table-layout:fixed;width:200px;border-spacing:0'><col style='width:100px'><col style='width:100px'>\
         <tr><td style='padding:0 0 0 10%'><div style='width:10px;height:20px;background:green'></div></td><td style='padding:0'></td></tr></table>",
    );
    assert_eq!(pixel(&img, 25, 10), GREEN, "the div starts 20px in");
    assert_ne!(pixel(&img, 15, 10), GREEN, "not 10px in");
}

/// In an intrinsic size a `max-width: fit-content(calc(200px - 10%))` has a cyclic percentage:
/// it is as `fit-content(100%)` there (the max-content width), not the calculation against an
/// infinite basis (negative: the min-content width).
#[test]
fn fit_content_calc_max_width_in_intrinsic_size() {
    let img = render(
        "<div style='display:inline-block;background:green;height:20px'>\
         <div style='width:400px;max-width:fit-content(calc(200px - 10%));font-size:0'>\
         <span style='display:inline-block;width:60px;height:10px'></span> <span style='display:inline-block;width:60px;height:10px'></span>\
         </div></div>",
    );
    assert_eq!(pixel(&img, 100, 5), GREEN, "the shrink-to-fit box is the max-content width (120px)");
}

/// An empty editable flex container is a line tall only where its height is `auto`: a
/// `height: 0` one (or one its `max-height` caps) keeps its height.
#[test]
fn empty_editable_flex_keeps_its_height() {
    let img = render(
        "<div style='line-height:0;font-size:0'><div contenteditable style='display:inline-flex;width:50px;height:0;line-height:24px;background:red'></div>\
         <div contenteditable style='display:inline-flex;width:50px;max-height:4px;line-height:24px;background:red;vertical-align:top'></div></div>\
         <div style='width:100px;height:50px;background:green'></div>",
    );
    assert_eq!(pixel(&img, 10, 10), GREEN);
    assert_eq!(pixel(&img, 75, 10), GREEN);
}

/// A scroll container in a multi-column strip is monolithic: a block in it is not fragmented, and
/// its `align-content` still applies.
#[test]
fn align_content_in_scroller_in_columns() {
    let img = render(
        "<div style='columns:2;column-gap:0;width:200px;height:100px;column-fill:auto'>\
         <div style='overflow:auto;height:100px'>\
         <div style='height:100px;align-content:center'><div style='height:20px;width:50px;background:green'></div></div>\
         </div></div>",
    );
    assert_eq!(pixel(&img, 10, 50), GREEN, "centred");
    assert_ne!(pixel(&img, 10, 5), GREEN, "not at the top");
}

/// One row of thousands of cells with alternating top borders: runs that never join. Offering
/// each cell's border joins runs only at its own edges (the whole line was walked per offer).
#[test]
fn alternating_collapsed_borders_are_linear() {
    let cells: String = (0..8000)
        .map(|i| format!("<td style='padding:0;width:1px;border-top:2px solid {}'></td>", if i % 2 == 0 { "red" } else { "blue" }))
        .collect();
    let start = std::time::Instant::now();
    render(&format!("<table style='border-collapse:collapse'><tr>{cells}</tr></table>"));
    let elapsed = start.elapsed();
    assert!(elapsed.as_secs_f64() < 20.0, "took {elapsed:?}");
}

/// A table's `max-width` caps the columns its `<col>` width hints ask for (Blink's column
/// distribution path): both cells' contents fit in 150px, so the table is 150px wide.
#[test]
fn hinted_columns_keep_the_table_max_width() {
    let img = render(
        "<table style='max-width:150px;border-spacing:0;background:green'><col style='width:300px'><col>\
         <tr><td style='padding:0;height:20px'><span style='display:inline-block;width:20px'></span></td>\
         <td style='padding:0'><span style='display:inline-block;width:20px'></span></td></tr></table>",
    );
    assert_eq!(pixel(&img, 140, 10), GREEN);
    assert_eq!(pixel(&img, 160, 10), [255, 255, 255]);
}

/// A cell's own `min-width` floors its column in Blink's distribution path as in the grid path.
#[test]
fn cell_min_width_floors_its_hinted_column() {
    let img = render(
        "<table style='border-spacing:0'><col style='width:30px'><col>\
         <tr><td style='padding:0;height:20px;width:100px;min-width:250px;background:green'></td>\
         <td style='padding:0'><span style='display:inline-block;width:10px'></span></td></tr></table>",
    );
    assert_eq!(pixel(&img, 240, 10), GREEN);
}

/// A relatively positioned float beside inline content: its containing block's direction (LTR)
/// picks `left` over `right`, whatever the float's own direction.
#[test]
fn relative_float_offset_follows_the_containing_block_direction() {
    let img = render(
        "<div>x<div style='float:left;direction:rtl;position:relative;left:100px;right:20px;\
         width:20px;height:20px;background:green'></div></div>",
    );
    assert_eq!(pixel(&img, 110, 5), GREEN);
}

/// A percentage `top` of a relatively positioned float beside inline content resolves against the
/// definite height of its containing block.
#[test]
fn relative_float_percent_top_uses_a_definite_containing_block_height() {
    let img = render(
        "<div style='height:100px'>x<div style='float:left;position:relative;top:50%;\
         width:20px;height:20px;background:green'></div></div>",
    );
    assert_eq!(pixel(&img, 10, 60), GREEN);
}

/// Thousands of table rows inside an inline box are one anonymous inline table: building it (and the
/// inline content around it) stays linear in the rows (review: a membership scan per row was
/// quadratic).
#[test]
fn many_table_rows_in_an_inline_box_build_in_linear_time() {
    let rows = "<span style='display:table-row'><span style='display:table-cell'>x</span></span>".repeat(4000);
    let started = std::time::Instant::now();
    let img = render(&format!(
        "<div style='width:50px;height:50px;background:green'></div><span>{rows}</span>"
    ));
    assert_eq!(pixel(&img, 10, 10), GREEN);
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "took {:?}", started.elapsed());
}

/// A fixed-width size container in a wider slot beside floats is kept clear of the floats down its
/// whole height, as any box: it compared its styled width with the slot's (never equal) and took
/// the slot at once, over a lower float. Also when no container query reads it (no width recorded).
/// Review cycle 4, finding 1.
#[test]
fn fixed_width_size_container_avoids_lower_floats() {
    for query in ["", "@container (min-width: 50px) { #c > div { color: red } }"] {
        let img = render(&format!(
            "<style>{query}</style><div style='width:300px;display:flow-root'>\
             <div style='float:right;width:150px;height:20px;background:blue'></div>\
             <div style='float:left;clear:right;width:250px;height:20px;background:green'></div>\
             <div id=c style='width:100px;container-type:inline-size;overflow:hidden;background:orange'>\
             <div style='height:200px'></div></div></div>"
        ));
        // Beside the right float there is room, but the box is taller than that slot: it goes below
        // the left float (y 40), not over it.
        assert_eq!(pixel(&img, 50, 30), GREEN, "the left float, not the container ({query:?})");
        assert_eq!(pixel(&img, 50, 50), [255, 165, 0], "the container below the floats ({query:?})");
    }
}
