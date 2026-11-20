//! Classic scrollbars (`BaseDocument::set_classic_scrollbars`, the WPT runner's `--scrollbars
//! classic`): the layout arithmetic of their gutters, as Chrome 158 lays them out on Linux
//! (measured with the installed Chrome without `--hide-scrollbars`: 15px scrollbars).
use blitz_dom::DocumentConfig;
use blitz_html::HtmlDocument;

fn doc(body: &str, classic: bool) -> HtmlDocument {
    let mut doc = HtmlDocument::from_html(
        &format!("<!doctype html><style>body{{margin:0}}</style>{body}"),
        DocumentConfig::default(),
    );
    doc.viewport_mut().window_size = (800, 600);
    doc.set_classic_scrollbars(classic);
    doc.resolve(0.0);
    doc
}

fn node<'a>(doc: &'a HtmlDocument, selector: &str) -> &'a blitz_dom::Node {
    let id = doc.query_selector(selector).unwrap().unwrap();
    doc.get_node(id).unwrap()
}

fn client(doc: &HtmlDocument, selector: &str) -> (f32, f32) {
    let n = node(doc, selector);
    (n.client_width(), n.client_height())
}

#[test]
fn projected_overflow_clips_and_bounds_the_visible_horizon() {
    let document = doc("<div id=panel style='overflow:auto;width:200px;height:200px'><div style='width:100px;height:400px;transform-origin:0 0;transform:perspective(100px) rotateX(45deg)'></div></div>", true);
    let panel = node(&document, "#panel");
    assert_eq!(panel.scrollable_overflow().y1, 400.0);
    assert_eq!(client(&document, "#panel"), (185.0, 200.0));
}

#[test]
fn projected_overflow_maps_corners_with_signed_w() {
    for (width, perspective, translation, expected) in [(120, "-.01", -240, 600.0), (128, "-.0078125", 240, 368.0)] {
        let document = doc(&format!("<div id=panel style='overflow:auto;width:50px;height:50px'><div style='width:{width}px;height:40px;transform-origin:0 0;transform:matrix3d(1,0,0,{perspective},0,1,0,0,0,0,1,0,{translation},0,0,1)'></div></div>"), true);
        assert!((node(&document, "#panel").scrollable_overflow().x1 - expected).abs() < 0.01);
    }
}

#[test]
fn projected_overflow_keeps_finite_scroll_range() {
    let document = doc("<div id=panel style='overflow:auto;width:200px;height:200px'><div style='width:20px;height:160000px;transform-origin:0 0;transform:perspective(200000px) rotateX(45deg)'></div></div>", true);
    assert!((node(&document, "#panel").scrollable_overflow().y1 - 260_496.0).abs() < 1.0);
}

#[test]
fn projected_overflow_selects_the_viewport_scrollbar_axis() {
    let document = doc(
        "<div style='position:absolute;inset:50px auto auto 50px;width:400px;height:400px;perspective:300px'>\
         <div style='width:400px;height:400px;transform:rotateY(50deg);perspective:100px'>\
         <div style='width:2000px;height:200px;transform:rotateY(70deg)'></div></div></div>",
        true,
    );
    assert!(!document.viewport_scrollbars.shown.x);
    assert!(document.viewport_scrollbars.shown.y);
}

#[test]
fn overlay_scrollbars_take_no_space() {
    let d = doc("<div id=s style='width:100px;height:100px;overflow:scroll;scrollbar-gutter:stable both-edges'></div>", false);
    assert_eq!(client(&d, "#s"), (100.0, 100.0));
    assert_eq!(d.viewport_scrollbars, Default::default());
}

#[test]
fn overflow_scroll_and_stable_gutters() {
    let d = doc(
        "<div id=a style='width:100px;height:100px;overflow:scroll'></div>\
         <div id=b style='width:100px;height:100px;overflow:scroll;scrollbar-width:thin'></div>\
         <div id=c style='width:100px;height:100px;overflow:auto;scrollbar-gutter:stable'></div>\
         <div id=e style='width:100px;height:100px;overflow:auto;scrollbar-gutter:stable both-edges'></div>\
         <div id=f style='width:100px;height:100px;overflow:clip;scrollbar-gutter:stable'></div>",
        true,
    );
    assert_eq!(client(&d, "#a"), (85.0, 85.0));
    assert_eq!(client(&d, "#b"), (89.0, 89.0));
    assert_eq!(client(&d, "#c"), (85.0, 100.0));
    assert_eq!(client(&d, "#e"), (70.0, 100.0));
    assert_eq!(client(&d, "#f"), (100.0, 100.0));
}

#[test]
fn the_overflow_a_first_scrollbar_causes_adds_the_second() {
    // Chrome: both bars (offsetWidth - clientWidth = offsetHeight - clientHeight = 15).
    let d = doc(
        "<div id=a style='width:100px;height:100px;overflow:auto'><div style='width:95px;height:200px'></div></div>\
         <div id=b style='width:100px;height:100px;overflow:auto'><div style='width:200px;height:95px'></div></div>",
        true,
    );
    assert_eq!(client(&d, "#a"), (85.0, 85.0));
    assert_eq!(client(&d, "#b"), (85.0, 85.0));
}

#[test]
fn a_box_of_no_height_shows_no_horizontal_scrollbar() {
    // WPT css-overflow `scrollbar-empty-002`: Chrome has the vertical bar (clientWidth 285) only.
    let d = doc(
        "<div style='width:300px'><div id=s style='overflow:auto'><div style='height:0;position:relative'>\
         <div style='position:absolute;width:300px;height:300px'></div></div></div></div>",
        true,
    );
    let s = node(&d, "#s");
    assert_eq!((s.client_width(), s.final_layout().size.height), (285.0, 0.0));
}

#[test]
fn both_edges_moves_inline_flex_and_grid_content_past_the_start_gutter() {
    let d = doc(
        "<div id=i style='width:100px;height:60px;overflow:hidden;scrollbar-gutter:stable both-edges'>x</div>\
         <div style='width:100px;height:60px;overflow:hidden;scrollbar-gutter:stable both-edges;display:flex'><span id=f>x</span></div>\
         <div style='width:100px;height:60px;overflow:hidden;scrollbar-gutter:stable both-edges;display:grid'><span id=g>x</span></div>\
         <div style='width:100px;height:60px;overflow:hidden;scrollbar-gutter:stable both-edges'><p id=p style='margin:0'>x</p></div>",
        true,
    );
    assert_eq!(node(&d, "#i").content_box_origin().x, 15.0);
    assert_eq!(node(&d, "#f").final_layout().location.x, 15.0);
    assert_eq!(node(&d, "#g").final_layout().location.x, 15.0);
    assert_eq!(node(&d, "#p").final_layout().location.x, 15.0);
    assert_eq!(node(&d, "#p").final_layout().size.width, 70.0);
}

#[test]
fn rtl_puts_the_vertical_scrollbar_on_the_left() {
    let d = doc(
        "<div id=s style='width:100px;height:100px;overflow-y:scroll;direction:rtl'><p id=p style='margin:0'>x</p></div>",
        true,
    );
    assert_eq!(node(&d, "#p").final_layout().location.x, 15.0);
}

#[test]
fn the_root_gutters_shrink_the_initial_containing_block_and_viewport_units() {
    let d = doc(
        "<style>html{scrollbar-gutter:stable both-edges}</style><div id=w style='width:100vw;height:10px'></div>",
        true,
    );
    let g = d.viewport_scrollbars.gutters;
    assert_eq!((g.left, g.right), (15.0, 15.0));
    // Nothing overflows: no scrollbar shows, the gutters are reserved only.
    assert!(!d.viewport_scrollbars.shown.x && !d.viewport_scrollbars.shown.y);
    let html = d.root_element();
    assert_eq!((html.final_layout().location.x, html.final_layout().size.width), (15.0, 770.0));
    // Chrome: 100vw is 770 with a root `stable both-edges` gutter.
    assert_eq!(node(&d, "#w").final_layout().size.width, 770.0);
}

#[test]
fn custom_scrollbars_use_their_own_sizes_without_reducing_viewport_units() {
    let document = doc("<style>html{overflow:scroll} html::-webkit-scrollbar{width:300px;height:8px} section{width:100px;height:100px;overflow:scroll} section::-webkit-scrollbar{width:0;height:12px} #hidden::-webkit-scrollbar{display:none}</style><div id=wide style='width:100vw'></div><section id=s></section><section id=hidden></section>", true);
    let gutters = document.viewport_scrollbars.gutters;
    assert_eq!((gutters.right, gutters.bottom), (300.0, 8.0));
    assert_eq!(node(&document, "#wide").final_layout().size.width, 800.0);
    assert_eq!(client(&document, "#s"), (100.0, 88.0));
    assert_eq!(client(&document, "#hidden"), (100.0, 100.0));
}

#[test]
fn custom_scrollbar_review_orientation() {
    let axes = "::-webkit-scrollbar:vertical{width:24px;height:40px}::-webkit-scrollbar:horizontal{width:50px;height:7px}";
    for (base, expected) in [("", (85.0, 85.0)), ("::-webkit-scrollbar{background:blue}", (76.0, 93.0))] {
        let document = doc(&format!("<style>html{{overflow:scroll}}section{{width:100px;height:100px;overflow:scroll}}{base}{axes}</style><section></section>"), true);
        assert_eq!(client(&document, "section"), expected);
        let gutters = document.viewport_scrollbars.gutters;
        assert_eq!((gutters.right, gutters.bottom), (100.0 - expected.0, 100.0 - expected.1));
    }
}

#[test]
fn custom_scrollbar_review_large_gutters() {
    for length in ["100000px", "3e38px"] {
        let document = doc(&format!("<style>html,section{{overflow:scroll;scrollbar-gutter:stable both-edges}}::-webkit-scrollbar{{width:{length};height:{length}}}section{{box-sizing:border-box;width:100px;height:80px;border:4px solid}}</style><section><div></div></section>"), true);
        let viewport = document.viewport_scrollbars;
        assert!(viewport.gutters.left + viewport.gutters.right <= 800.0 && viewport.gutters.bottom <= 600.0);
        let scroller = node(&document, "section");
        let gutters = scroller.classic_scrollbars().unwrap().gutters;
        assert_eq!((gutters.left, gutters.right, gutters.bottom), (46.0, 46.0, 72.0));
        assert_eq!(client(&document, "section"), (0.0, 0.0));
        assert_eq!(node(&document, "section > div").final_layout().location.x, 96.0);
    }
}

#[test]
fn narrow_both_edges_preserves_content_placement() {
    for (width, client_left, client_width, child_x) in [(8, 4.0, 0.0, 8.0), (20, 10.0, 0.0, 15.0), (40, 15.0, 10.0, 15.0)] {
        let document = doc(&format!("<style>section{{width:{width}px;overflow:hidden;scrollbar-gutter:stable both-edges}}section::-webkit-scrollbar{{width:15px}}</style><section><div></div></section><section id=text><span>x</span></section>"), true);
        let scroller = node(&document, "section");
        assert_eq!(scroller.classic_scrollbars().unwrap().gutters.left, client_left);
        assert_eq!(scroller.client_width(), client_width);
        assert_eq!(node(&document, "section > div").final_layout().location.x, child_x);
        assert_eq!(scroller.content_box_origin().x, child_x);
        let text = node(&document, "#text");
        assert_eq!(text.final_layout().size.width, width as f32);
        assert_eq!(text.content_box_origin().x, child_x);
    }
}

#[test]
fn custom_scrollbar_review_constraints() {
    for (rule, expected) in [("width:20px;max-width:5px", 95.0), ("width:5px;min-width:20px", 80.0), ("width:calc(5px + 20%)", 95.0)] {
        let document = doc(&format!("<style>section{{width:100px;height:80px;overflow:scroll}}section::-webkit-scrollbar{{{rule}}}</style><section></section>"), true);
        assert_eq!(client(&document, "section").0, expected, "{rule}");
    }
}

#[test]
fn custom_scrollbar_review_viewport_units() {
    for stable in [false, true] {
        for (rule, gutter) in [("", 15.0), ("color:red", 15.0), ("width:0", 0.0), ("display:none", 0.0)] {
            let gutter_style = if stable { "stable" } else { "auto" };
            let pseudo = if rule.is_empty() { String::new() } else { format!("html::-webkit-scrollbar{{{rule}}}") };
            let document = doc(&format!("<style>html{{scrollbar-gutter:{gutter_style}}}{pseudo}</style><div id=wide style='width:100vw;height:650px'></div>"), true);
            assert_eq!(document.viewport_scrollbars.gutters.right, gutter, "{gutter_style}: {rule}");
            let expected = if stable && rule.is_empty() { 785.0 } else { 800.0 };
            assert_eq!(node(&document, "#wide").final_layout().size.width, expected, "{gutter_style}: {rule}");
        }
    }
}

#[test]
fn an_overflowing_document_shows_the_viewport_scrollbar() {
    let d = doc("<div style='height:2000px'></div>", true);
    assert!(d.viewport_scrollbars.shown.y);
    assert_eq!(d.root_element().final_layout().size.width, 785.0);
}

#[test]
fn nested_overflowing_scrollers_all_show_their_scrollbars() {
    // Every level changes at once: each is laid out again (its ancestors once each).
    let depth = 40;
    let mut body = String::new();
    for i in 0..depth {
        // Each box (160px with its padding) overflows its parent's 100px.
        body.push_str(&format!("<div id=n{i} style='overflow:auto;height:100px;padding-bottom:60px'>"));
    }
    body.push_str("<div style='height:5000px;width:10px'></div>");
    for _ in 0..depth {
        body.push_str("</div>");
    }
    let d = doc(&body, true);
    for i in 0..depth {
        let n = node(&d, &format!("#n{i}"));
        assert!(n.classic_scrollbars().is_some_and(|b| b.shown.y), "n{i}");
    }
}
