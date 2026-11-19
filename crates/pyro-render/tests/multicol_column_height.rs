//! css-multicol-2 `column-height` and `column-wrap`: a fixed column height, columns wrapping into
//! rows `row-gap` apart, the container as tall as its rows; `column-height` alone makes a
//! multi-column container, and the `columns` shorthand takes it after a slash.
use std::{cell::RefCell, rc::Rc, sync::Arc};

use pyro_dom::{
    parse::{parse_document, DomRef},
    snapshot::{render_key, RenderHtml, RenderNodeKind},
};
use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn fetcher() -> Arc<dyn Fetcher> {
    Arc::new(NoFetch)
}

fn boxes(html: &str, ids: &[&str]) -> Vec<[f64; 4]> {
    use_fork_safe_fonts();
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", html);
    let d = dom.borrow();
    let keys: Vec<_> = ids.iter().map(|id| render_key(d.get_element_by_id(doc, id).unwrap().unwrap())).collect();
    let tree = d
        .render_tree(doc, &RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None })
        .unwrap();
    let tree = Tree {
        nodes: tree
            .nodes
            .into_iter()
            .map(|node| TreeNode {
                key: node.key,
                parent: node.parent,
                kind: match node.kind {
                    RenderNodeKind::Element { name, attrs } => TreeKind::Element { name, attrs },
                    RenderNodeKind::Text(text) => TreeKind::Text(text),
                },
            })
            .collect(),
    };
    let snap = Snapshot {
        base_url: "http://test/".into(),
        width: 800,
        height: 600,
        device_scale_factor: 1.0,
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.sync_layout(&snap, &fetcher()).unwrap();
    keys.into_iter()
        .map(|key| {
            let b = renderer.element_box(1, key).unwrap();
            [b.x, b.y, b.x + b.width, b.y + b.height]
        })
        .collect()
}


fn height(container: &str) -> f64 {
    let b = boxes(
        &format!("<!doctype html><style>body{{margin:0}}</style><div id=c style='width:100px;{container}'><div style='height:100px'></div></div>"),
        &["c"],
    );
    b[0][3] - b[0][1]
}

#[test]
fn columns_of_a_fixed_height_wrap_into_rows() {
    // 100px of content in 2 columns of 25px: 2 rows, 10px apart.
    assert_eq!(height("columns:2;gap:10px 0;column-height:25px;column-wrap:wrap"), 60.0);
    // `auto` wraps when the height is given; `nowrap` keeps one row (the container's own height
    // is then its content's, as without a column height: 0 here, since columns are not stacked).
    assert_eq!(height("columns:2;gap:10px 0;column-height:25px"), 60.0);
    assert_eq!(height("columns:2;column-height:25px;column-wrap:nowrap"), 25.0);
    // The shorthand, and `column-height` alone (one column per row).
    assert_eq!(height("columns:2 / 25px;row-gap:0"), 50.0);
    assert_eq!(height("column-height:40px;row-gap:20px"), 160.0); // 3 rows of 40px, 2 gaps of 20px
}

#[test]
fn without_a_column_height_nothing_changes() {
    assert_eq!(height("columns:2;gap:0"), 50.0);
    assert_eq!(height("column-wrap:wrap"), 100.0);
}

/// The red and green channels of a pixel of the page.
fn rg_at(body: &str, x: usize, y: usize) -> (u8, u8) {
    let snap = Snapshot {
        html: format!("<!doctype html><style>body{{margin:0}}</style>{body}"),
        base_url: "http://test/".into(),
        width: 200,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let (img, _, _) = Renderer::new().render(&snap, Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    let i = (y * img.width as usize + x) * 4;
    (img.data[i], img.data[i + 1])
}

#[test]
fn an_abspos_box_of_a_multicol_container_is_placed_against_it_not_in_a_column() {
    // Two 50px-tall columns: an abspos box at the container's (10, 60) lies past the first
    // column's range; painted in a column it was moved to the second (60, 10). It stays at its place.
    let body = "<div style='position:relative;columns:2;gap:0;width:100px;column-fill:auto;height:50px'>\
                <div style='height:100px'></div>\
                <div style='position:absolute;left:10px;top:60px;width:20px;height:20px;background:#f00'></div></div>";
    assert_eq!(rg_at(body, 20, 70), (255, 0), "the box at its place");
    assert_eq!(rg_at(body, 70, 20), (255, 255), "not in the second column");
}

#[test]
fn columns_after_a_spanner_fit_what_is_left_of_a_definite_height() {
    // WPT multicol-span-all-012: a 70px spanner, then 120px in 3 columns of a 100px-tall container:
    // 30px are left, so the columns are 30px tall (4 of them, the last past the content box).
    let body = "<div style='columns:3;width:75px;height:100px;column-gap:0'>\
                <div style='column-span:all;width:100px;height:70px;background:#0f0'></div>\
                <div style='height:120px;background:#f00'></div></div>";
    assert_eq!(rg_at(body, 10, 99), (255, 0), "the first column reaches the container's end");
    assert_eq!(rg_at(body, 90, 80), (255, 0), "a fourth column takes the rest");
    assert_eq!(rg_at(body, 10, 105), (255, 255), "nothing below the container");
}

/// `column-height: 0`: a column holds a box of size containment whole (it is monolithic and
/// overflows the column), so each of the ten 20px boxes has a column, two per row, the rows 0px
/// tall and a 20px row gap apart. The boxes were cut at each column's end (css-multicol
/// `column-height-023`).
#[test]
fn a_size_contained_box_is_not_cut_at_the_end_of_a_column_it_is_taller_than() {
    let items: String = (0..10).map(|i| format!("<div id=i{i} style='contain:size;height:20px'></div>")).collect();
    let ids: Vec<String> = (0..10).map(|i| format!("i{i}")).collect();
    let mut wanted: Vec<&str> = vec!["c"];
    wanted.extend(ids.iter().map(String::as_str));
    let b = boxes(
        &format!("<!doctype html><style>body{{margin:0}}</style><div id=c style='columns:2 / 0;width:100px;gap:20px 0;column-fill:auto'>{items}</div>"),
        &wanted,
    );
    // Five rows of height 0 and four gaps.
    assert_eq!(b[0], [0.0, 0.0, 100.0, 80.0]);
    for (i, item) in b[1..].iter().enumerate() {
        let (x, y) = ((i % 2) as f64 * 50.0, (i / 2) as f64 * 20.0);
        assert_eq!(*item, [x, y, x + 50.0, y + 20.0], "box {i}");
    }
}

/// Rows of an explicit `column-height` are a grid of slots that spanners take part in (Chromium's
/// layout, measured on Chrome 158; css-multicol `column-height-017`..`-020`). With 20px columns
/// and a 10px row gap: 50px of content fills row 0 and is balanced in row 1 (30..35), the 10px
/// spanner follows at once (35..45), the content after it goes on in what is left of that row
/// (45..50), then in the rows at 60 and 90. A spanner that does not fit in the rest of its row
/// starts the next row and may reach into the gap after it.
#[test]
fn spanners_share_the_rows_of_an_explicit_column_height() {
    let green = (0, 128);
    let red = (255, 0);
    let body = "<div style='width:100px;height:100px;background:#f00'>\
                <div style='columns:2;column-height:20px;column-wrap:wrap;gap:10px 0'>\
                <div style='height:50px;background:green'></div>\
                <div style='column-span:all;height:10px;background:green'></div>\
                <div style='height:70px;background:green'></div></div></div>";
    for (y, want, what) in [
        (10, green, "row 0"),
        (25, red, "the gap after row 0"),
        (32, green, "the balanced rest of the content before the spanner"),
        (40, green, "the spanner, right after it"),
        (47, green, "the content after the spanner, in what is left of the row"),
        (55, red, "the gap after row 1"),
        (70, green, "row 2"),
        (85, red, "the gap after row 2"),
        (95, green, "row 3"),
    ] {
        assert_eq!(rg_at(body, 25, y), want, "{what} (y {y}, left column)");
        assert_eq!(rg_at(body, 75, y), want, "{what} (y {y}, right column)");
    }
    // A 25px spanner after 20px of content (balanced: 10px a column) does not fit in the 10px left
    // of row 0: it starts row 1 (30..55), reaching into the gap, and the content after it row 2.
    let body = "<div style='width:100px;height:100px;background:#f00'>\
                <div style='columns:2;column-height:20px;column-wrap:wrap;gap:10px 0'>\
                <div style='height:20px;background:green'></div>\
                <div style='column-span:all;height:25px;background:green'></div>\
                <div style='height:60px;background:green'></div></div></div>";
    for (y, want, what) in [(5, green, "the balanced content"), (15, red, "the rest of row 0"), (25, red, "the gap"), (40, green, "the spanner"), (53, green, "the spanner in the gap"), (57, red, "the rest of the gap"), (70, green, "row 2")] {
        assert_eq!(rg_at(body, 25, y), want, "{what} (y {y})");
    }
}

/// A `column-span: all` box below a child of the container spans the columns too, as long as the
/// boxes between them are plain in-flow blocks (css-multicol-1 § 6: it is in the container's
/// formatting context). It was laid out inside a column. A wrapper that keeps its content to
/// itself (a transform here) has no spanner in it.
#[test]
fn a_spanner_below_a_child_of_the_container_spans_its_columns() {
    let (green, blue, white) = ((0, 128), (0, 0), (255, 255));
    let content = "<div style='height:40px;background:green'></div>\
                   <h3 style='column-span:all;height:10px;margin:0;background:#00f'></h3>\
                   <div style='height:40px;background:green'></div>";
    let body = format!("<div style='columns:2;width:100px;gap:0'><div>{content}</div></div>");
    // Two 20px columns, the spanner's row at 20..30, two more 20px columns.
    for (x, y, want, what) in [
        (25, 10, green, "the first column above the spanner"),
        (75, 10, green, "the second column above the spanner"),
        (25, 25, blue, "the spanner"),
        (75, 25, blue, "the spanner over the second column"),
        (25, 40, green, "the first column below the spanner"),
        (75, 40, green, "the second column below the spanner"),
        (75, 55, white, "below the container"),
    ] {
        assert_eq!(rg_at(&body, x, y), want, "{what} at ({x}, {y})");
    }
    // In a transformed wrapper the box is no spanner: 90px of content in two 45px columns, the
    // blue box at 40..45 of the first only.
    let body = format!("<div style='columns:2;width:100px;gap:0'><div style='transform:scale(1)'>{content}</div></div>");
    assert_eq!(rg_at(&body, 25, 42), blue, "the box in the first column");
    assert_eq!(rg_at(&body, 75, 42), green, "the second column beside it");
}
