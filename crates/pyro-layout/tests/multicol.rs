//! Boxes in a multi-column container (blitz-dom `layout::multicol`) are where its columns show
//! them: `getBoundingClientRect`, an inline element's fragments and hit testing (each path:
//! the scan of every box, the tree descent and the index) place a box in the column whose range
//! of the container's strip holds it, as Chromium reports its first fragment.
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
