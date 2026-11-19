//! Boxes in a multi-column container (blitz-dom `layout::multicol`) are where its columns show
//! them: `getBoundingClientRect`, an inline element's fragments and hit testing (each path:
//! the scan of every box, the tree descent and the index) place a box in the column whose range
//! of the container's strip holds it. A box split by a break reports the union of its pieces
//! (`getBoundingClientRect`), one rect per piece (`getClientRects`), and is hit in its pieces only.
use crate::hit_descent::page;

fn rect(p: &crate::hit_descent::Page, id: &str) -> [f64; 4] {
    let r = p.layout.bounding_client_rect(&p.dom, p.id(id));
    [r.x, r.y, r.width, r.height]
}

#[test]
fn boxes_are_placed_in_their_columns() {
    let p = page(
        "<!doctype html><body style='margin:0'>\
         <div id=mc style='columns:2;column-gap:20px;width:220px'>\
         <div id=a style='height:50px'></div><div id=b style='height:50px;display:flow-root'><div id=c style='height:10px;margin-top:20px'></div></div></div>\
         <div id=after style='height:10px'></div>",
    );
    assert_eq!(rect(&p, "mc"), [0.0, 0.0, 220.0, 50.0], "the container is as tall as its balanced columns");
    assert_eq!(rect(&p, "a"), [0.0, 0.0, 100.0, 50.0]);
    assert_eq!(rect(&p, "b"), [120.0, 0.0, 100.0, 50.0], "the second block is at the top of the second column");
    assert_eq!(rect(&p, "c"), [120.0, 20.0, 100.0, 10.0], "a box inside it moves with it");
    assert_eq!(rect(&p, "after"), [0.0, 50.0, 800.0, 10.0]);
    assert_eq!(p.layout.element_from_point(&p.dom, 170.0, 25.0), Some(p.id("c")));
    assert_eq!(p.layout.element_from_point(&p.dom, 170.0, 45.0), Some(p.id("b")));
    assert_eq!(p.layout.element_from_point(&p.dom, 50.0, 25.0), Some(p.id("a")));
    p.compare("columns");
}

#[test]
fn an_inline_element_on_a_line_in_the_second_column_is_there() {
    let p = page(
        "<!doctype html><body style='margin:0'>\
         <div style='columns:2;column-gap:0;width:200px;font:20px/20px monospace;orphans:1;widows:1'>\
         <p style='margin:0'>aaaa<br><span id=s>bbbb</span></p></div>",
    );
    let [x, y, w, h] = rect(&p, "s");
    assert!(
        x >= 100.0 && x < 110.0 && y < 20.0 && w > 0.0 && h > 0.0,
        "the span on the second line: {:?}",
        [x, y, w, h]
    );
    assert_eq!(p.layout.element_from_point(&p.dom, x + 2.0, y + h / 2.0), Some(p.id("s")));
    p.compare("inline in columns");
}

/// A box split by a column break is hit only where its pieces are painted (Chromium hit-tests
/// each fragment): the earlier sibling under the union of its pieces is still hit there, and
/// `getClientRects` lists the pieces.
#[test]
fn a_box_split_over_columns_is_hit_only_in_its_pieces() {
    let p = page(
        "<!doctype html><body style='margin:0'>\
         <div id=mc style='columns:2;column-gap:20px;width:220px'>\
         <div id=a style='height:60px'></div><div id=b style='height:80px'></div></div>",
    );
    assert_eq!(rect(&p, "mc"), [0.0, 0.0, 220.0, 70.0], "balanced into two 70px columns");
    assert_eq!(rect(&p, "b"), [0.0, 0.0, 220.0, 70.0], "the bounding box unites the pieces");
    let rects: Vec<[f64; 4]> =
        p.layout.client_rects(&p.dom, p.id("b")).iter().map(|r| [r.x, r.y, r.width, r.height]).collect();
    assert_eq!(rects, vec![[0.0, 60.0, 100.0, 10.0], [120.0, 0.0, 100.0, 70.0]], "one rect per piece");
    assert_eq!(p.layout.element_from_point(&p.dom, 50.0, 30.0), Some(p.id("a")), "a, under b's union");
    assert_eq!(p.layout.element_from_point(&p.dom, 50.0, 65.0), Some(p.id("b")), "b's first piece");
    assert_eq!(p.layout.element_from_point(&p.dom, 170.0, 30.0), Some(p.id("b")), "b's second piece");
    assert_eq!(p.layout.element_from_point(&p.dom, 110.0, 30.0), Some(p.id("mc")), "the gap");
    p.compare("split box");
}

/// The lines of a container whose only content is inline are where its padding puts them when
/// the columns break between them (its own layout is not written before its columns are made).
#[test]
fn lines_of_an_inline_container_with_padding_break_between_lines() {
    let p = page(
        "<!doctype html><body style='margin:0'>\
         <div id=mc style='columns:2;column-gap:0;width:200px;padding-top:10px;font:20px/20px monospace;\
         orphans:1;widows:1'><span id=a>a</span><br><span id=b>b</span><br><span id=c>c</span><br>\
         <span id=d>d</span></div>",
    );
    let [x, y, _, h] = rect(&p, "c");
    assert!(x >= 100.0 && (y - 10.0).abs() < 4.0 && h > 0.0, "c at the top of the second column: {:?}", rect(&p, "c"));
    let [x, y, _, _] = rect(&p, "b");
    assert!(x < 100.0 && y > 25.0 && y < 40.0, "b at the bottom of the first column: {:?}", rect(&p, "b"));
    assert_eq!(rect(&p, "mc")[3], 50.0, "two lines a column, below the padding");
}

/// pyro: a container nested in 1px-tall outer columns makes a row in each of them; a thousand
/// columns in each of a thousand rows would be a million fragments from a few elements. They
/// are capped per container (and per layout pass): what is left goes to the last column.
#[test]
fn nested_rows_of_many_columns_are_capped() {
    let inner = "<div style='column-count:1000;column-gap:0'><div class=tall style='height:100000000px'></div></div>";
    let p = page(&format!(
        "<!doctype html><body style='margin:0'><div style='columns:2;column-fill:auto;height:1px;width:400px'>{}</div>",
        inner.replace("class=tall", "id=t1") + &inner.replace("class=tall", "id=t2")
    ));
    for id in ["t1", "t2"] {
        let pieces = p.layout.client_rects(&p.dom, p.id(id)).len();
        assert!(pieces > 1 && pieces <= 10_000, "#{id}: {pieces} pieces");
    }
}

/// pyro: column breaks are of their own type (CSS Fragmentation 3 § 3.1, § 3.2): `break-before:
/// page` and `break-inside: avoid-page` do nothing in columns on screen, `column` and
/// `avoid-column` do; a nested container that avoids column breaks inside moves whole to the
/// next column (as Chromium lays these out).
#[test]
fn column_breaks_are_not_page_breaks() {
    let p = page(
        "<!doctype html><body style='margin:0'>\
         <div style='columns:2;column-gap:0;width:200px'><div style='height:20px'></div>\
         <div id=p style='break-before:page;break-inside:avoid;height:20px'></div><div style='height:20px'></div></div>\
         <div style='columns:2;column-gap:0;width:200px'><div style='height:20px'></div>\
         <div id=c style='break-before:column;break-inside:avoid;height:20px'></div><div style='height:20px'></div></div>\
         <div style='columns:2;column-gap:0;width:200px;column-fill:auto;height:50px'><div style='height:30px'></div>\
         <div id=q style='break-inside:avoid-page;height:40px'></div></div>\
         <div style='columns:2;column-gap:0;width:200px;column-fill:auto;height:100px'><div style='height:60px'></div>\
         <div id=n style='columns:2;column-gap:0;break-inside:avoid-column'><div style='height:30px'></div>\
         <div style='height:30px'></div><div style='height:30px'></div></div></div>",
    );
    assert_eq!(rect(&p, "p"), [0.0, 20.0, 100.0, 20.0], "break-before: page does not break the columns");
    assert_eq!(rect(&p, "c"), [100.0, 40.0, 100.0, 20.0], "break-before: column does");
    assert_eq!(p.layout.client_rects(&p.dom, p.id("q")).len(), 2, "break-inside: avoid-page does not keep it whole");
    let n = rect(&p, "n");
    assert_eq!((n[0], n[1]), (100.0, 130.0), "the nested container avoiding breaks starts the next column");
}

fn rects(p: &crate::hit_descent::Page, id: &str) -> Vec<[f64; 4]> {
    p.layout.client_rects(&p.dom, p.id(id)).iter().map(|r| [r.x, r.y, r.width, r.height]).collect()
}

/// pyro: a nested container laid out in rows of an outer container's columns is laid out again
/// when that fragmentation goes away (a sibling becomes a spanner, the outer box stops being a
/// multi-column container): it is where a fresh layout of the final page puts it.
#[test]
fn nested_rows_go_with_their_outer_fragmentation() {
    let html = |outer: &str, sibling: &str| {
        format!(
            "<!doctype html><body style='margin:0'><div id=o style='{outer}'>\
             <div style='columns:2;column-gap:0'><div id=t style='height:300px'></div></div>\
             <div id=s style='{sibling}height:10px'></div></div>"
        )
    };
    let fragmented = "columns:2;column-gap:0;column-fill:auto;height:100px;width:200px";
    for (outer, sibling) in [(fragmented, "column-span:all;"), ("width:200px", "")] {
        let mut p = page(&html(fragmented, ""));
        let before = rects(&p, "t");
        assert!(before.len() >= 2, "{before:?}");
        let (o, s) = (p.id("o"), p.id("s"));
        p.dom.set_attribute(o, "style", outer).unwrap();
        p.dom.set_attribute(s, "style", &format!("{sibling}height:10px")).unwrap();
        let fresh = page(&html(outer, sibling));
        assert_eq!(rects(&p, "t"), rects(&fresh, "t"), "outer {outer:?}, sibling {sibling:?}");
    }
}

/// pyro: a multi-column table cell of `height: auto` stretched by its row gives its content no
/// percentage basis (CSS 2.1 § 17.5.3), and `vertical-align: bottom` moves its content, not its
/// column boxes (hit where it is shown).
#[test]
fn a_multicol_cell_stretched_by_its_row() {
    let p = page(
        "<!doctype html><body style='margin:0'>\
         <table style='border-spacing:0'><tr><td style='columns:2;column-gap:0;width:100px;padding:0'>\
         <div id=h style='height:100%'></div><div style='height:20px'></div></td>\
         <td style='height:200px;padding:0;width:10px'></td></tr></table>\
         <table style='border-spacing:0'><tr><td style='columns:2;column-gap:10px;column-rule:4px solid;width:100px;padding:0;vertical-align:bottom'>\
         <div id=b style='height:40px'></div></td><td style='height:200px;padding:0;width:10px'></td></tr></table>",
    );
    assert_eq!(rect(&p, "h")[3], 0.0, "no percentage basis");
    assert_eq!(rect(&p, "b")[1], 380.0, "the content at the bottom of the cell");
    assert_eq!(p.layout.element_from_point(&p.dom, 10.0, 390.0), Some(p.id("b")));
}

/// pyro: a box in a nested container's row in a later outer column is where paint shows it:
/// its pieces are cut at the nested container's breaks and then at the outer one's
/// (`getClientRects`, hit testing), as Chromium reports them.
#[test]
fn a_box_in_a_nested_row_is_where_it_is_painted() {
    let p = page(
        "<!doctype html><body style='margin:0'>\
         <div style='columns:2;column-gap:0;column-fill:auto;height:100px;width:400px'>\
         <div style='columns:2;column-gap:0'><div style='height:250px'></div>\
         <div id=m style='height:20px'></div><div style='height:50px'></div></div></div>",
    );
    assert_eq!(rects(&p, "m"), vec![[200.0, 50.0, 100.0, 10.0], [300.0, 0.0, 100.0, 10.0]]);
    assert_eq!(p.layout.element_from_point(&p.dom, 310.0, 5.0), Some(p.id("m")));
}
