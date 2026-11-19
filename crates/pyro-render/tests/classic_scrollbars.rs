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
