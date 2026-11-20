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

#[test]
fn nested_layout_shares_the_wrapped_run_slots() {
    for (count, spans, nested, heights, heading) in [
        (3, true, true, &[28., 44.][..], 44.),
        (2, true, true, &[60., 6., 6.][..], 76.),
        (3, false, true, &[58., 14.][..], 14.),
        (3, false, false, &[58., 14.][..], 14.),
    ] {
        let span = if spans { "all" } else { "none" };
        let inner = if nested { "columns:2;gap:0" } else { "height:72px" };
        let content = if nested { "<article></article>" } else { "" };
        let p = page(&format!("<!doctype html><style>*{{margin:0}}main{{width:180px;columns:{count};column-height:60px;column-wrap:wrap;gap:10px 0}}header{{height:60px}}section{{{inner}}}article{{height:144px}}h2{{column-span:{span};height:15px}}footer{{height:30px}}</style><main><header></header><section id=s>{content}</section><h2 id=h></h2><footer></footer></main>"));
        assert_eq!(rects(&p, "s").iter().map(|rect| rect[3]).collect::<Vec<_>>(), heights);
        assert_eq!(rect(&p, "h")[1], heading);
    }
}

#[test]
fn shared_wrapper_resumes_each_overlapping_column_run() {
    for (count, first_spans) in [(2, true), (3, true), (2, false)] {
        let first = if first_spans { "all" } else { "none" };
        let p = page(&format!("<!doctype html><style>*{{margin:0}}main{{columns:{count};width:120px;gap:0}}h2{{height:30px;column-span:all}}h2:first-child{{column-span:{first}}}article{{height:25px;margin-top:-15px}}</style><main><section id=s><h2></h2><article id=a></article><h2 id=h></h2><article id=b></article></section></main>"));
        let first_pieces = rects(&p, "a");
        let second_pieces = rects(&p, "b");
        assert_eq!(first_pieces.len(), if first_spans { count } else { 1 });
        assert_eq!(second_pieces.len(), count);
        if !first_spans { assert_eq!(first_pieces, [[60.0, -5.0, 60.0, 25.0]]); }
        for (id, pieces) in [("a", first_pieces), ("b", second_pieces)] {
            for (index, [x, y, _, height]) in pieces.into_iter().enumerate() {
                let target = if id == "b" && index == 0 && !first_spans { "h" } else { id };
                assert_eq!(p.layout.element_from_point(&p.dom, x + 3.0, y + height / 2.0), Some(p.id(target)), "{count} {first_spans} {id} {index}");
            }
        }
        assert_eq!(rects(&p, "s").len(), if first_spans { 1 + 2 * count } else { 2 * count });
    }
}

#[test]
fn sequential_nested_columns_spend_the_slot_when_they_break() {
    for (count, height, forced, expected) in [(2, 20, false, 40), (2, 20, true, 90), (3, 50, true, 90)] {
        let before = if forced { "column" } else { "auto" };
        let p = page(&format!("<!doctype html><style>*{{margin:0}}main,section{{columns:2;gap:0;column-fill:auto}}main{{columns:{count};width:{}px;height:140px}}header{{height:50px}}section{{break-before:avoid}}article{{height:{height}px}}article+article{{break-before:{before}}}footer{{height:100px}}</style><main><header></header><section id=inner><article></article><article></article></section><footer></footer></main>", count * 50));
        assert_eq!(rect(&p, "inner"), [0.0, 50.0, 50.0, f64::from(expected)]);
    }
}

#[test]
fn contained_floats_fragment_without_excluding_siblings() {
    for (count, side) in [(2, "left"), (3, "left"), (2, "right")] {
        let p = page(&format!("<!doctype html><style>*{{margin:0}}main{{gap:0;columns:{count};width:160px;height:100px;column-fill:auto}}header{{height:40px}}section{{columns:2;gap:0}}aside{{contain:layout;height:0}}b{{float:{side};width:12px;height:100px}}article{{contain:size;width:20px;height:80px}}</style><main><header></header><section id=s><aside id=a><b id=f></b></aside><article id=m></article></section></main>"));
        let outer_width = 160.0 / f64::from(count);
        for (id, heights) in [("s", &[80.0][..]), ("m", &[80.0]), ("a", &[0.0, 0.0]), ("f", &[80.0, 20.0])] {
            let pieces = rects(&p, id);
            assert_eq!(pieces.len(), heights.len(), "{count} {side} {id}: {pieces:?}");
            for (index, (piece, height)) in pieces.iter().zip(heights).enumerate() {
                let inset = if id == "f" && side == "right" { outer_width / 2.0 - 12.0 } else { 0.0 };
                let x = outer_width + index as f64 * outer_width / 2.0 + inset;
                assert!((piece[0] - x).abs() <= 0.51 && piece[1] == 0.0 && piece[3] == *height, "{count} {side} {id} {piece:?} vs ({x}, 0, _, {height})");
            }
        }
    }
}

#[test]
fn parallel_children_spend_the_parent_slot_independently_of_later_siblings() {
    for (count, tail) in [(4, "static"), (5, "static"), (4, "absolute")] {
        let p = page(&format!("<!doctype html><style>*{{margin:0}}main{{gap:0;columns:{count};width:200px;height:50px;column-fill:auto}}section{{min-height:190px;border-inline:4px solid}}header{{height:10px}}aside{{height:0}}article{{height:29px;break-inside:avoid}}i{{display:block;height:1px;position:{tail}}}footer{{height:4px}}</style><main><section id=p><header></header><aside id=a><article></article><article></article><article></article></aside><i></i></section><footer id=f></footer></main>"));
        let width = 200.0 / f64::from(count);
        assert_eq!(rects(&p, "p"), [[0.0, 0.0, width, 50.0], [width, 0.0, width, 50.0], [2.0*width, 0.0, width, 50.0], [3.0*width, 0.0, width, 40.0]]);
        assert_eq!(rect(&p, "f"), [3.0*width, 40.0, width, 4.0]);
        assert_eq!(rects(&p, "a"), [[4.0, 10.0, width-8.0, 0.0], [width+4.0, 0.0, width-8.0, 0.0], [2.0*width+4.0, 0.0, width-8.0, 0.0]]);
    }
}

#[test]
fn a_parallel_break_allocates_its_own_continuation() {
    for (count, header, after) in [(3, 20, false), (4, 20, false), (3, 10, false), (3, 20, true)] {
        let before_style = if after { "" } else { "break-before:column" };
        let after_style = if after { "break-after:column" } else { "" };
        let p = page(&format!("<!doctype html><style>*{{margin:0}}main{{width:150px;columns:{count};height:100px;column-fill:auto;gap:0}}</style>\
            <main><section id=owner style='height:10px'><header style='height:{header}px;{after_style}'></header>\
            <article id=resume style='height:10px;{before_style}'></article></section><footer id=other style='height:50px'></footer></main>"));
        let width = 150.0 / f64::from(count);
        let check = |actual: [f64; 4], expected: [f64; 4]| {
            assert!((actual[0] - expected[0]).abs() <= 0.5, "{actual:?} vs {expected:?}");
            assert_eq!(actual[1..], expected[1..]);
        };
        check(rect(&p, "resume"), [width, 0.0, width, 10.0]);
        check(rect(&p, "other"), if after { [width, 0.0, width, 50.0] } else { [0.0, 10.0, width, 50.0] });
        let owner = rects(&p, "owner");
        assert_eq!(owner.len(), 2);
        check(owner[0], [0.0, 0.0, width, 10.0]);
        check(owner[1], [width, 0.0, width, 0.0]);
    }
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

/// pyro: a column spanner comes after the floats before it (they are in the row of columns it
/// ends), as in Chromium.
#[test]
fn a_spanner_comes_after_the_floats_before_it() {
    let p = page(
        "<!doctype html><body style='margin:0'>\
         <div style='columns:2;width:100px;column-gap:0;orphans:1;widows:1'>\
         <div style='float:left;width:50px;height:180px'></div><div><div id=s style='column-span:all;height:10px'></div></div></div>",
    );
    assert_eq!(rect(&p, "s"), [0.0, 90.0, 100.0, 10.0]);
}

/// pyro: a box of a fixed height with a column spanner in it spends its height on its columns
/// only: the spanner is a row of its own (Chromium; WPT css-multicol
/// `multicol-span-all-children-height-004a`).
#[test]
fn a_spanner_takes_none_of_its_boxs_height() {
    let p = page(
        "<!doctype html><body style='margin:0'><div style='columns:2;column-gap:0;width:200px'>\
         <div id=c style='height:250px'><div style='height:200px'></div>\
         <div id=s style='column-span:all;height:50px'></div><div id=b style='height:100px'></div></div></div>",
    );
    assert_eq!(rect(&p, "s"), [0.0, 100.0, 200.0, 50.0]);
    assert_eq!(rect(&p, "b")[1], 150.0, "the box goes on below the spanner");
}

/// pyro: a nested container with rows of an explicit `column-height` lays them out in its outer
/// columns: a row that does not fit what is left of one starts the next, the row gap before it
/// truncated (Chromium; WPT css-multicol `column-height-027`).
#[test]
fn nested_column_height_rows_follow_the_outer_columns() {
    let p = page(
        "<!doctype html><body style='margin:0'><div style='columns:2;column-fill:auto;height:100px;gap:0;width:100px'>\
         <div style='columns:2;column-fill:auto;column-height:25px;column-wrap:wrap;gap:10px 0'>\
         <div id=t style='height:300px'></div></div></div>",
    );
    let r = rects(&p, "t");
    assert_eq!(r.len(), 12, "{r:?}");
    assert_eq!(r[6], [50.0, 0.0, 25.0, 25.0], "the fourth row at the top of the second outer column");
}

/// pyro: the fragment cap is the container's, not each part's: hundreds of spanners with
/// columns of no height left between them make no more fragments in all than the cap.
#[test]
fn parts_between_spanners_share_the_fragment_cap() {
    let sections: String = (0..300)
        .map(|_| "<div style='height:2000px'></div><div style='column-span:all;height:1px'></div>")
        .collect();
    let p = page(&format!(
        "<!doctype html><body style='margin:0'><div style='columns:2;column-gap:0;height:10px;width:200px'>\
         <div id=all>{sections}</div></div>"
    ));
    // (One piece of the box around everything per fragment it is in: the spanners' rows too.)
    let total = p.layout.client_rects(&p.dom, p.id("all")).len();
    assert!(total > 1000 && total <= 10_000, "{total} fragments");
}

/// pyro: a text node's words are where its columns show them (Range rects, caret positions): its
/// rect spans both columns of a two-column container, as Chromium unites its line boxes, not the
/// strip's one column twice as tall.
#[test]
fn a_text_nodes_words_are_in_their_columns() {
    let p = page(
        "<!doctype html><body style='margin:0'>\
         <div id=t style='columns:2;column-fill:auto;column-gap:0;height:40px;width:200px;font:16px/20px monospace'>aaa bbb ccc ddd eee fff</div>",
    );
    let text = p.dom.first_child(p.id("t")).unwrap().unwrap();
    let r = p.layout.text_rect(&p.dom, text).unwrap();
    assert!(r.x + r.width > 100.0, "into the second column: {r:?}");
    assert!(r.height <= 40.0 + 0.5, "no taller than a column: {r:?}");
}

/// pyro: a multi-column container's scrollable overflow is its boxes where its columns show
/// them, each moved on its own: a spanner (a child, or inside one) reaches into a column's range
/// only where its row starts, and is no wider for it (Chromium; the union mapped through that
/// column made it columns wider: WPT css-multicol `multicol-list-item-003`,
/// `multicol-span-all-block-sibling-003` under classic scrollbars).
#[test]
fn a_spanner_adds_no_scrollable_overflow_beside_the_columns() {
    let p = page(
        "<!doctype html><body style='margin:0'>\
         <div id=a style='columns:3;column-gap:20px;width:490px;overflow:auto'><div style='height:150px'></div>\
         <div style='column-span:all;height:50px'></div>after</div>\
         <div id=b style='columns:2;width:400px;overflow:auto'><div><div style='column-span:all;height:20px'></div>\
         x<br>x<br>x<br>x<br>x<br>x<br>x</div></div>",
    );
    assert_eq!(p.layout.scroll_width(&p.dom, p.id("a")), 490.0, "a spanner child");
    assert_eq!(p.layout.scroll_width(&p.dom, p.id("b")), 400.0, "a spanner inside a child");
}
