//! Flow layout Stage 1 parity with headless Chromium (Playwright 1.59.1,
//! 800x600 viewport). Expected values were measured in Chromium on the same
//! markup (getBoundingClientRect, clientWidth/Height, scrollWidth/Height);
//! boxes use explicit sizes so approximate glyph widths do not matter.
use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::layout::Layout;
use lp_dom::media::Viewport;
use lp_dom::parse::{parse_document, DomRef};
use lp_dom::{Dom, NodeId};

fn filler() -> String {
    "<div class=row><span>cell</span><span>more</span></div>".repeat(300)
}

fn options() -> String {
    (0..12).map(|i| format!("<div role=option id=o{i} style=\"width:260px;height:32px\"></div>")).collect()
}

struct Page {
    dom: Dom,
    doc: NodeId,
    layout: Layout,
}

/// Loads nothing (the pages have no sub-resources).
struct NoFetch;
impl lp_render::Fetcher for NoFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err(format!("{url}: offline")));
    }
}

fn page(html: &str) -> Page {
    lp_layout::install(std::sync::Arc::new(NoFetch));
    let html = html.replace("FILLER", &filler()).replace("OPTIONS", &options());
    let dom_ref: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom_ref, "http://test/", &html);
    let mut dom = Rc::try_unwrap(dom_ref).ok().expect("sole owner").into_inner();
    // Stand-in for a loaded image's natural size (the embedder records it).
    let imgs = dom.query_selector_all(doc, &lp_dom::select::parse_selectors("img[data-natural]").unwrap()).unwrap();
    for img in imgs {
        let v = dom.get_attribute(img, "data-natural").unwrap().unwrap();
        let (w, h) = v.split_once(',').unwrap();
        dom.set_natural_size(img, Some((w.parse().unwrap(), h.parse().unwrap())));
    }
    let mut layout = Layout::with_viewport(doc, Viewport::new(800, 600));
    layout.style_mut().discover_style_elements(&dom);
    Page { dom, doc, layout }
}

impl Page {
    fn id(&self, id: &str) -> NodeId {
        self.dom.get_element_by_id(self.doc, id).unwrap().unwrap_or_else(|| panic!("#{id}"))
    }
    /// [x, y, width, height, clientWidth, clientHeight, scrollWidth, scrollHeight]
    fn metrics(&self, id: &str) -> [f64; 8] {
        let el = self.id(id);
        let r = self.layout.bounding_client_rect(&self.dom, el);
        let l = &self.layout;
        [
            r.x,
            r.y,
            r.width,
            r.height,
            l.client_width(&self.dom, el),
            l.client_height(&self.dom, el),
            l.scroll_width(&self.dom, el),
            l.scroll_height(&self.dom, el),
        ]
    }
}

fn check(name: &str, html: &str, scroll_y: i32, expected: &[(&str, [f64; 8])], doc: [f64; 2]) {
    let mut p = page(html);
    if scroll_y != 0 {
        p.layout.window_scroll_to(&p.dom, Some(0), Some(scroll_y));
    }
    let mut diffs = Vec::new();
    for (id, want) in expected {
        let got = p.metrics(id);
        if got.iter().zip(want).any(|(g, w)| (g - w).abs() > 0.01) {
            diffs.push(format!("#{id}: got {got:?}, Chromium {want:?}"));
        }
    }
    let (w, h) = p.layout.document_scroll_size(&p.dom);
    if (w - doc[0]).abs() > 0.01 || (h - doc[1]).abs() > 0.01 {
        diffs.push(format!("document: got [{w}, {h}], Chromium {doc:?}"));
    }
    assert!(diffs.is_empty(), "{name}:\n{}", diffs.join("\n"));
}

#[test]
fn flow_parity_stack() {
    let html = "<!doctype html><body><div id=a style=\"height:30px\"></div><div id=b style=\"height:40px;margin-top:10px\"></div><div id=c style=\"width:200px;height:20px;margin:5px auto\"></div><div id=d style=\"width:50%;height:10px\"></div></body>";
    check(
        "stack",
        html,
        0,
        &[
            ("a", [8.0, 8.0, 784.0, 30.0, 784.0, 30.0, 784.0, 30.0]),
            ("b", [8.0, 48.0, 784.0, 40.0, 784.0, 40.0, 784.0, 40.0]),
            ("c", [300.0, 93.0, 200.0, 20.0, 200.0, 20.0, 200.0, 20.0]),
            ("d", [8.0, 118.0, 392.0, 10.0, 392.0, 10.0, 392.0, 10.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flow_parity_collapse() {
    let html = "<!doctype html><body style=\"margin:0\"><div id=a style=\"height:10px;margin-bottom:20px\"></div><div id=b style=\"height:10px;margin-top:30px\"></div><div id=c style=\"height:10px;margin-top:5px\"></div></body>";
    check(
        "collapse",
        html,
        0,
        &[
            ("a", [0.0, 0.0, 800.0, 10.0, 800.0, 10.0, 800.0, 10.0]),
            ("b", [0.0, 40.0, 800.0, 10.0, 800.0, 10.0, 800.0, 10.0]),
            ("c", [0.0, 55.0, 800.0, 10.0, 800.0, 10.0, 800.0, 10.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flow_parity_boxmodel() {
    let html = "<!doctype html><body style=\"margin:0\"><div id=p style=\"width:100px;height:50px;padding:10px;border:2px solid black\"></div><div id=q style=\"width:100px;height:50px;padding:10px;border:2px solid black;box-sizing:border-box\"></div><div id=r style=\"padding:5px 7px;margin:3px 11px\"><div id=r1 style=\"height:20px\"></div><div id=r2 style=\"height:25px\"></div></div></body>";
    check(
        "boxmodel",
        html,
        0,
        &[
            ("p", [0.0, 0.0, 124.0, 74.0, 120.0, 70.0, 120.0, 70.0]),
            ("q", [0.0, 74.0, 100.0, 50.0, 96.0, 46.0, 96.0, 46.0]),
            ("r", [11.0, 127.0, 778.0, 55.0, 778.0, 55.0, 778.0, 55.0]),
            ("r1", [18.0, 132.0, 764.0, 20.0, 764.0, 20.0, 764.0, 20.0]),
            ("r2", [18.0, 152.0, 764.0, 25.0, 764.0, 25.0, 764.0, 25.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flow_parity_nested() {
    let html = "<!doctype html><body style=\"margin:0\"><div id=outer style=\"width:300px;padding:10px\"><div id=mid style=\"padding:4px;border-width:1px;border-style:solid\"><div id=inner style=\"height:40px;width:100px;margin-left:15px\"></div></div></div></body>";
    check(
        "nested",
        html,
        0,
        &[
            ("outer", [0.0, 0.0, 320.0, 70.0, 320.0, 70.0, 320.0, 70.0]),
            ("mid", [10.0, 10.0, 300.0, 50.0, 298.0, 48.0, 298.0, 48.0]),
            ("inner", [30.0, 15.0, 100.0, 40.0, 100.0, 40.0, 100.0, 40.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flow_parity_popover() {
    let html = "<!doctype html><body><main>FILLER</main><div id=pop style=\"position:absolute;top:120px;left:200px;width:280px\"><div id=list role=listbox style=\"max-height:300px;overflow:auto\">OPTIONS</div></div></body>";
    check(
        "popover",
        html,
        0,
        &[
            ("pop", [200.0, 120.0, 280.0, 300.0, 280.0, 300.0, 280.0, 300.0]),
            ("list", [200.0, 120.0, 280.0, 300.0, 280.0, 300.0, 280.0, 384.0]),
            ("o0", [200.0, 120.0, 260.0, 32.0, 260.0, 32.0, 260.0, 32.0]),
            ("o1", [200.0, 152.0, 260.0, 32.0, 260.0, 32.0, 260.0, 32.0]),
            ("o2", [200.0, 184.0, 260.0, 32.0, 260.0, 32.0, 260.0, 32.0]),
            ("o3", [200.0, 216.0, 260.0, 32.0, 260.0, 32.0, 260.0, 32.0]),
            ("o4", [200.0, 248.0, 260.0, 32.0, 260.0, 32.0, 260.0, 32.0]),
            ("o5", [200.0, 280.0, 260.0, 32.0, 260.0, 32.0, 260.0, 32.0]),
            ("o6", [200.0, 312.0, 260.0, 32.0, 260.0, 32.0, 260.0, 32.0]),
            ("o7", [200.0, 344.0, 260.0, 32.0, 260.0, 32.0, 260.0, 32.0]),
            ("o8", [200.0, 376.0, 260.0, 32.0, 260.0, 32.0, 260.0, 32.0]),
            ("o9", [200.0, 408.0, 260.0, 32.0, 260.0, 32.0, 260.0, 32.0]),
            ("o10", [200.0, 440.0, 260.0, 32.0, 260.0, 32.0, 260.0, 32.0]),
            ("o11", [200.0, 472.0, 260.0, 32.0, 260.0, 32.0, 260.0, 32.0]),
        ],
        [800.0, 5416.0],
    );
}

#[test]
fn flow_parity_fixed() {
    let html = "<!doctype html><body style=\"margin:0\"><div style=\"height:3000px\"></div><div id=f style=\"position:fixed;top:10px;right:20px;width:100px;height:50px\"></div><div id=g style=\"position:fixed;bottom:0;left:0;right:0;height:40px\"></div></body>";
    check(
        "fixed",
        html,
        500,
        &[
            ("f", [680.0, 10.0, 100.0, 50.0, 100.0, 50.0, 100.0, 50.0]),
            ("g", [0.0, 560.0, 800.0, 40.0, 800.0, 40.0, 800.0, 40.0]),
        ],
        [800.0, 3000.0],
    );
}

#[test]
fn flow_parity_relative() {
    let html = "<!doctype html><body style=\"margin:0\"><div id=a style=\"height:20px\"></div><div id=b style=\"position:relative;top:7px;left:13px;height:20px\"><div id=b1 style=\"height:5px\"></div></div><div id=c style=\"height:20px\"></div></body>";
    check(
        "relative",
        html,
        0,
        &[
            ("a", [0.0, 0.0, 800.0, 20.0, 800.0, 20.0, 800.0, 20.0]),
            ("b", [13.0, 27.0, 800.0, 20.0, 800.0, 20.0, 800.0, 20.0]),
            ("b1", [13.0, 27.0, 800.0, 5.0, 800.0, 5.0, 800.0, 5.0]),
            ("c", [0.0, 40.0, 800.0, 20.0, 800.0, 20.0, 800.0, 20.0]),
        ],
        [813.0, 600.0],
    );
}

#[test]
fn flow_parity_absolute() {
    let html = "<!doctype html><body style=\"margin:0\"><div id=cb style=\"position:relative;margin:20px;width:400px;height:200px;border:5px solid\"><div id=tr style=\"position:absolute;top:10px;right:10px;width:50px;height:30px\"></div><div id=br style=\"position:absolute;bottom:0;left:0;right:0;height:20px\"></div><div id=st style=\"position:absolute;width:30px;height:30px\"></div></div></body>";
    check(
        "absolute",
        html,
        0,
        &[
            ("cb", [20.0, 20.0, 410.0, 210.0, 400.0, 200.0, 400.0, 200.0]),
            ("tr", [365.0, 35.0, 50.0, 30.0, 50.0, 30.0, 50.0, 30.0]),
            ("br", [25.0, 205.0, 400.0, 20.0, 400.0, 20.0, 400.0, 20.0]),
            ("st", [25.0, 25.0, 30.0, 30.0, 30.0, 30.0, 30.0, 30.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flow_parity_inlineblock() {
    let html = "<!doctype html><body style=\"margin:0;font-family:-apple-system,BlinkMacSystemFont,sans-serif\"><div id=row style=\"width:250px\"><span id=s1 style=\"display:inline-block;width:100px;height:20px\"></span><span id=s2 style=\"display:inline-block;width:100px;height:30px\"></span><span id=s3 style=\"display:inline-block;width:100px;height:20px\"></span></div><div id=after style=\"height:10px\"></div></body>";
    check(
        "inlineblock",
        html,
        0,
        &[
            ("row", [0.0, 0.0, 250.0, 56.0, 250.0, 56.0, 250.0, 56.0]),
            ("s1", [0.0, 10.0, 100.0, 20.0, 100.0, 20.0, 100.0, 20.0]),
            ("s2", [100.0, 0.0, 100.0, 30.0, 100.0, 30.0, 100.0, 30.0]),
            ("s3", [0.0, 33.0, 100.0, 20.0, 100.0, 20.0, 100.0, 20.0]),
            ("after", [0.0, 56.0, 800.0, 10.0, 800.0, 10.0, 800.0, 10.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flow_hit_test_picks_the_option_under_the_point() {
    let p = page("<!doctype html><body><main>FILLER</main><div id=pop style=\"position:absolute;top:120px;left:200px;width:280px\"><div id=list role=listbox style=\"max-height:300px;overflow:auto\">OPTIONS</div></div></body>");
    let r = p.layout.bounding_client_rect(&p.dom, p.id("o3"));
    let hit = p.layout.element_from_point(&p.dom, r.x + 10.0, r.y + 10.0);
    assert_eq!(hit, Some(p.id("o3")), "Chromium hits #o3");
}

#[test]
fn fixed_and_absolute_boxes_escape_overflow_clips_outside_their_containing_block() {
    // Gap 30: a fixed element's containing block is the viewport, so an
    // `overflow: hidden` ancestor between it and the viewport does not clip
    // it (Chromium: elementFromPoint(20, 20) is the BUTTON; the engine hit
    // the content underneath). The same holds for an absolutely positioned
    // box and non-positioned clipping ancestors below its containing block.
    let html = "<!doctype html><body style=\"margin:0\">\
        <div id=under style=\"width:400px;height:400px\"></div>\
        <section style=\"position:fixed;left:500px;top:0\"><div style=\"overflow:hidden;width:300px;height:300px\">\
        <div style=\"position:fixed;left:0;top:0;z-index:9999\"><button id=b style=\"width:60px;height:40px;padding:0;border:0\">B</button></div>\
        <div id=stay style=\"position:fixed;left:700px;top:350px;width:50px;height:50px\"></div>\
        </div></section>\
        <div style=\"position:relative;top:0;left:0\"><div style=\"overflow:hidden;width:50px;height:50px\">\
        <div id=abs style=\"position:absolute;left:100px;top:0;width:50px;height:50px\"></div>\
        <div id=clipped style=\"position:relative;left:0;top:0;width:200px;height:20px\"></div>\
        </div></div>\
        <div style=\"position:fixed;left:0;top:500px\"><div style=\"overflow:hidden;width:10px;height:10px\">\
        <div id=inner style=\"width:100px;height:100px\"></div></div></div>\
        </body>";
    let p = page(html);
    let r = p.layout.bounding_client_rect(&p.dom, p.id("b"));
    assert_eq!([r.x, r.y, r.width, r.height], [0.0, 0.0, 60.0, 40.0], "the fixed button sits at the viewport origin");
    let at = |x: f64, y: f64| p.layout.element_from_point(&p.dom, x, y);
    assert_eq!(at(20.0, 20.0), Some(p.id("b")), "Chromium hits the BUTTON through the overflow:hidden ancestor");
    assert_eq!(at(720.0, 370.0), Some(p.id("stay")), "a fixed box outside its clipping ancestor's rect");
    assert_eq!(at(120.0, 410.0), Some(p.id("abs")), "an absolute box escapes a non-positioned clip");
    assert_ne!(at(100.0, 410.0), Some(p.id("clipped")), "a relative box inside the clip stays clipped");
    assert_ne!(at(50.0, 550.0), Some(p.id("inner")), "a clip inside the fixed containing block still applies");
}

#[test]
fn svg_shapes_have_client_rects_from_their_bbox_like_chromium() {
    // Gap 34: SVG shapes had no CSS box, so getBoundingClientRect() was
    // empty while getBBox() was right, and visibility checks failed.
    // Chromium (800x600): a shape's client rect is its bbox (own and
    // ancestor transforms applied) mapped by the <svg>'s viewBox into the
    // svg's content box.
    let html = "<!doctype html><body style=\"margin:0\"><div style=\"height:50px\"></div>\
        <div style=\"padding-left:30px\"><svg id=s1 width=200 height=100 style=\"display:block\">\
        <rect id=r x=10 y=20 width=50 height=30 />\
        <g transform=\"translate(100,0)\"><circle id=c cx=20 cy=50 r=10 transform=\"translate(5,5)\"/></g>\
        <line id=l x1=0 y1=0 x2=10 y2=20 /></svg></div>\
        <svg id=s2 width=200 height=100 viewBox=\"0 0 100 100\" style=\"display:block\"><rect id=v x=10 y=20 width=50 height=30 /></svg>\
        <svg id=s3 width=300 height=100 viewBox=\"0 0 100 100\" preserveAspectRatio=\"none\" style=\"display:block\"><rect id=n x=10 y=20 width=50 height=30 /></svg>\
        <svg id=s4 width=200 height=100 style=\"display:block;padding:10px;box-sizing:border-box\"><rect id=p x=0 y=0 width=50 height=30 /></svg>\
        </body>";
    let p = page(html);
    let r = |id: &str| {
        let r = p.layout.bounding_client_rect(&p.dom, p.id(id));
        [r.x, r.y, r.width, r.height].map(|v| (v * 1e4).round() / 1e4)
    };
    assert_eq!(r("s1"), [30.0, 50.0, 200.0, 100.0]);
    assert_eq!(r("r"), [40.0, 70.0, 50.0, 30.0], "rect: svg origin + bbox");
    assert_eq!(r("c"), [145.0, 95.0, 20.0, 20.0], "circle: own and <g> transforms");
    assert_eq!(r("l"), [30.0, 50.0, 10.0, 20.0], "line");
    // viewBox 100x100 in a 200x100 box: xMidYMid meet scales by 1 and
    // centres horizontally (+50).
    assert_eq!(r("v"), [60.0, 170.0, 50.0, 30.0], "viewBox meet");
    // preserveAspectRatio none: 3x horizontally, 1x vertically.
    assert_eq!(r("n"), [30.0, 270.0, 150.0, 30.0], "viewBox none");
    // Shapes sit in the svg's content box, inside its padding.
    assert_eq!(r("p"), [10.0, 360.0, 50.0, 30.0], "padding");
    assert_eq!(p.layout.client_rects(&p.dom, p.id("r")).len(), 1, "getClientRects has the rect");
}

// ----- Stage 2: flex -----------------------------------------------------------

fn flex_check(name: &str, html: &str, expected: &[(&str, [f64; 8])]) {
    let p = page(html);
    let mut diffs = Vec::new();
    for (id, want) in expected {
        let got = p.metrics(id);
        if got.iter().zip(want).any(|(g, w)| (g - w).abs() > 0.01) {
            diffs.push(format!("#{id}: got {got:?}, Chromium {want:?}"));
        }
    }
    assert!(diffs.is_empty(), "{name}:\n{}", diffs.join("\n"));
}

#[test]
fn flex_parity_row() {
    flex_check(
        "row",
        "<!doctype html><body style=\"margin:0\"><div id=f style=\"display:flex;width:600px\"><div id=a style=\"width:100px;height:30px\"></div><div id=b style=\"width:150px;height:50px\"></div><div id=c style=\"width:50px;height:20px\"></div></div><div id=after style=\"height:10px\"></div></body>",
        &[
            ("f", [0.0, 0.0, 600.0, 50.0, 600.0, 50.0, 600.0, 50.0]),
            ("a", [0.0, 0.0, 100.0, 30.0, 100.0, 30.0, 100.0, 30.0]),
            ("b", [100.0, 0.0, 150.0, 50.0, 150.0, 50.0, 150.0, 50.0]),
            ("c", [250.0, 0.0, 50.0, 20.0, 50.0, 20.0, 50.0, 20.0]),
            ("after", [0.0, 50.0, 800.0, 10.0, 800.0, 10.0, 800.0, 10.0]),
        ],
    );
}

#[test]
fn flex_parity_gap() {
    flex_check(
        "gap",
        "<!doctype html><body style=\"margin:0\"><div id=f style=\"display:flex;gap:12px;width:600px;padding:8px\"><div id=a style=\"width:100px;height:30px\"></div><div id=b style=\"width:100px;height:30px\"></div></div></body>",
        &[
            ("f", [0.0, 0.0, 616.0, 46.0, 616.0, 46.0, 616.0, 46.0]),
            ("a", [8.0, 8.0, 100.0, 30.0, 100.0, 30.0, 100.0, 30.0]),
            ("b", [120.0, 8.0, 100.0, 30.0, 100.0, 30.0, 100.0, 30.0]),
        ],
    );
}

#[test]
fn flex_parity_grow() {
    flex_check(
        "grow",
        "<!doctype html><body style=\"margin:0\"><div id=f style=\"display:flex;width:600px;height:40px\"><div id=a style=\"width:100px\"></div><div id=b style=\"flex:1\"></div><div id=c style=\"flex-grow:2;width:50px\"></div></div></body>",
        &[
            ("f", [0.0, 0.0, 600.0, 40.0, 600.0, 40.0, 600.0, 40.0]),
            ("a", [0.0, 0.0, 100.0, 40.0, 100.0, 40.0, 100.0, 40.0]),
            ("b", [100.0, 0.0, 150.0, 40.0, 150.0, 40.0, 150.0, 40.0]),
            ("c", [250.0, 0.0, 350.0, 40.0, 350.0, 40.0, 350.0, 40.0]),
        ],
    );
}

#[test]
fn flex_parity_shrink() {
    flex_check(
        "shrink",
        "<!doctype html><body style=\"margin:0\"><div id=f style=\"display:flex;width:300px;height:20px\"><div id=a style=\"width:200px\"></div><div id=b style=\"width:200px;flex-shrink:3\"></div></div></body>",
        &[
            ("f", [0.0, 0.0, 300.0, 20.0, 300.0, 20.0, 300.0, 20.0]),
            ("a", [0.0, 0.0, 175.0, 20.0, 175.0, 20.0, 175.0, 20.0]),
            ("b", [175.0, 0.0, 125.0, 20.0, 125.0, 20.0, 125.0, 20.0]),
        ],
    );
}

#[test]
fn flex_parity_column() {
    flex_check(
        "column",
        "<!doctype html><body style=\"margin:0\"><div id=f style=\"display:flex;flex-direction:column;width:300px;height:400px;gap:10px\"><div id=a style=\"height:30px\"></div><div id=b style=\"flex:1;min-height:0\"></div><div id=c style=\"height:40px;width:100px\"></div></div></body>",
        &[
            ("f", [0.0, 0.0, 300.0, 400.0, 300.0, 400.0, 300.0, 400.0]),
            ("a", [0.0, 0.0, 300.0, 30.0, 300.0, 30.0, 300.0, 30.0]),
            ("b", [0.0, 40.0, 300.0, 310.0, 300.0, 310.0, 300.0, 310.0]),
            ("c", [0.0, 360.0, 100.0, 40.0, 100.0, 40.0, 100.0, 40.0]),
        ],
    );
}

#[test]
fn flex_parity_align() {
    flex_check(
        "align",
        "<!doctype html><body style=\"margin:0\"><div id=f style=\"display:flex;align-items:center;justify-content:space-between;width:500px;height:100px\"><div id=a style=\"width:100px;height:20px\"></div><div id=b style=\"width:100px;height:60px\"></div><div id=c style=\"width:100px;height:40px;align-self:flex-end\"></div></div></body>",
        &[
            ("f", [0.0, 0.0, 500.0, 100.0, 500.0, 100.0, 500.0, 100.0]),
            ("a", [0.0, 40.0, 100.0, 20.0, 100.0, 20.0, 100.0, 20.0]),
            ("b", [200.0, 20.0, 100.0, 60.0, 100.0, 60.0, 100.0, 60.0]),
            ("c", [400.0, 60.0, 100.0, 40.0, 100.0, 40.0, 100.0, 40.0]),
        ],
    );
}

#[test]
fn flex_parity_justify() {
    flex_check(
        "justify",
        "<!doctype html><body style=\"margin:0\"><div id=f1 style=\"display:flex;justify-content:center;width:500px;height:20px\"><div id=a style=\"width:100px\"></div></div><div id=f2 style=\"display:flex;justify-content:flex-end;width:500px;height:20px\"><div id=b style=\"width:100px\"></div></div></body>",
        &[
            ("f1", [0.0, 0.0, 500.0, 20.0, 500.0, 20.0, 500.0, 20.0]),
            ("a", [200.0, 0.0, 100.0, 20.0, 100.0, 20.0, 100.0, 20.0]),
            ("f2", [0.0, 20.0, 500.0, 20.0, 500.0, 20.0, 500.0, 20.0]),
            ("b", [400.0, 20.0, 100.0, 20.0, 100.0, 20.0, 100.0, 20.0]),
        ],
    );
}

#[test]
fn flex_parity_stretch() {
    flex_check(
        "stretch",
        "<!doctype html><body style=\"margin:0\"><div id=f style=\"display:flex;width:400px\"><div id=a style=\"width:100px;height:50px\"></div><div id=b style=\"width:100px\"></div></div></body>",
        &[
            ("f", [0.0, 0.0, 400.0, 50.0, 400.0, 50.0, 400.0, 50.0]),
            ("a", [0.0, 0.0, 100.0, 50.0, 100.0, 50.0, 100.0, 50.0]),
            ("b", [100.0, 0.0, 100.0, 50.0, 100.0, 50.0, 100.0, 50.0]),
        ],
    );
}

#[test]
fn flex_parity_wrap() {
    flex_check(
        "wrap",
        "<!doctype html><body style=\"margin:0\"><div id=f style=\"display:flex;flex-wrap:wrap;width:250px;gap:10px\"><div id=a style=\"width:100px;height:20px\"></div><div id=b style=\"width:100px;height:30px\"></div><div id=c style=\"width:100px;height:20px\"></div></div></body>",
        &[
            ("f", [0.0, 0.0, 250.0, 60.0, 250.0, 60.0, 250.0, 60.0]),
            ("a", [0.0, 0.0, 100.0, 20.0, 100.0, 20.0, 100.0, 20.0]),
            ("b", [110.0, 0.0, 100.0, 30.0, 100.0, 30.0, 100.0, 30.0]),
            ("c", [0.0, 40.0, 100.0, 20.0, 100.0, 20.0, 100.0, 20.0]),
        ],
    );
}

#[test]
fn flex_parity_shell() {
    flex_check(
        "shell",
        "<!doctype html><body style=\"margin:0\"><div id=app style=\"display:flex;flex-direction:column;height:600px\"><header id=h style=\"height:50px\"></header><main id=m style=\"display:flex;flex:1;min-height:0\"><nav id=n style=\"width:200px\"></nav><section id=s style=\"flex:1;overflow:auto\"><div id=content style=\"height:2000px\"></div></section></main></div></body>",
        &[
            ("app", [0.0, 0.0, 800.0, 600.0, 800.0, 600.0, 800.0, 600.0]),
            ("h", [0.0, 0.0, 800.0, 50.0, 800.0, 50.0, 800.0, 50.0]),
            ("m", [0.0, 50.0, 800.0, 550.0, 800.0, 550.0, 800.0, 550.0]),
            ("n", [0.0, 50.0, 200.0, 550.0, 200.0, 550.0, 200.0, 550.0]),
            ("s", [200.0, 50.0, 600.0, 550.0, 600.0, 550.0, 600.0, 2000.0]),
            ("content", [200.0, 50.0, 600.0, 2000.0, 600.0, 2000.0, 600.0, 2000.0]),
        ],
    );
}

// Flex through the real cascade (style_flow.rs fills the flex fields) and the
// Layout flow geometry, against the same Chromium measurements.

#[test]
fn flex_cascade_parity_row() {
    check(
        "row (cascade)",
        "<!doctype html><body style=\"margin:0\"><div id=f style=\"display:flex;width:600px\"><div id=a style=\"width:100px;height:30px\"></div><div id=b style=\"width:150px;height:50px\"></div><div id=c style=\"width:50px;height:20px\"></div></div><div id=after style=\"height:10px\"></div></body>",
        0,
        &[
            ("f", [0.0, 0.0, 600.0, 50.0, 600.0, 50.0, 600.0, 50.0]),
            ("a", [0.0, 0.0, 100.0, 30.0, 100.0, 30.0, 100.0, 30.0]),
            ("b", [100.0, 0.0, 150.0, 50.0, 150.0, 50.0, 150.0, 50.0]),
            ("c", [250.0, 0.0, 50.0, 20.0, 50.0, 20.0, 50.0, 20.0]),
            ("after", [0.0, 50.0, 800.0, 10.0, 800.0, 10.0, 800.0, 10.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flex_cascade_parity_gap() {
    check(
        "gap (cascade)",
        "<!doctype html><body style=\"margin:0\"><div id=f style=\"display:flex;gap:12px;width:600px;padding:8px\"><div id=a style=\"width:100px;height:30px\"></div><div id=b style=\"width:100px;height:30px\"></div></div></body>",
        0,
        &[
            ("f", [0.0, 0.0, 616.0, 46.0, 616.0, 46.0, 616.0, 46.0]),
            ("a", [8.0, 8.0, 100.0, 30.0, 100.0, 30.0, 100.0, 30.0]),
            ("b", [120.0, 8.0, 100.0, 30.0, 100.0, 30.0, 100.0, 30.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flex_cascade_parity_grow() {
    check(
        "grow (cascade)",
        "<!doctype html><body style=\"margin:0\"><div id=f style=\"display:flex;width:600px;height:40px\"><div id=a style=\"width:100px\"></div><div id=b style=\"flex:1\"></div><div id=c style=\"flex-grow:2;width:50px\"></div></div></body>",
        0,
        &[
            ("f", [0.0, 0.0, 600.0, 40.0, 600.0, 40.0, 600.0, 40.0]),
            ("a", [0.0, 0.0, 100.0, 40.0, 100.0, 40.0, 100.0, 40.0]),
            ("b", [100.0, 0.0, 150.0, 40.0, 150.0, 40.0, 150.0, 40.0]),
            ("c", [250.0, 0.0, 350.0, 40.0, 350.0, 40.0, 350.0, 40.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flex_cascade_parity_shrink() {
    check(
        "shrink (cascade)",
        "<!doctype html><body style=\"margin:0\"><div id=f style=\"display:flex;width:300px;height:20px\"><div id=a style=\"width:200px\"></div><div id=b style=\"width:200px;flex-shrink:3\"></div></div></body>",
        0,
        &[
            ("f", [0.0, 0.0, 300.0, 20.0, 300.0, 20.0, 300.0, 20.0]),
            ("a", [0.0, 0.0, 175.0, 20.0, 175.0, 20.0, 175.0, 20.0]),
            ("b", [175.0, 0.0, 125.0, 20.0, 125.0, 20.0, 125.0, 20.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flex_cascade_parity_column() {
    check(
        "column (cascade)",
        "<!doctype html><body style=\"margin:0\"><div id=f style=\"display:flex;flex-direction:column;width:300px;height:400px;gap:10px\"><div id=a style=\"height:30px\"></div><div id=b style=\"flex:1;min-height:0\"></div><div id=c style=\"height:40px;width:100px\"></div></div></body>",
        0,
        &[
            ("f", [0.0, 0.0, 300.0, 400.0, 300.0, 400.0, 300.0, 400.0]),
            ("a", [0.0, 0.0, 300.0, 30.0, 300.0, 30.0, 300.0, 30.0]),
            ("b", [0.0, 40.0, 300.0, 310.0, 300.0, 310.0, 300.0, 310.0]),
            ("c", [0.0, 360.0, 100.0, 40.0, 100.0, 40.0, 100.0, 40.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flex_cascade_parity_align() {
    check(
        "align (cascade)",
        "<!doctype html><body style=\"margin:0\"><div id=f style=\"display:flex;align-items:center;justify-content:space-between;width:500px;height:100px\"><div id=a style=\"width:100px;height:20px\"></div><div id=b style=\"width:100px;height:60px\"></div><div id=c style=\"width:100px;height:40px;align-self:flex-end\"></div></div></body>",
        0,
        &[
            ("f", [0.0, 0.0, 500.0, 100.0, 500.0, 100.0, 500.0, 100.0]),
            ("a", [0.0, 40.0, 100.0, 20.0, 100.0, 20.0, 100.0, 20.0]),
            ("b", [200.0, 20.0, 100.0, 60.0, 100.0, 60.0, 100.0, 60.0]),
            ("c", [400.0, 60.0, 100.0, 40.0, 100.0, 40.0, 100.0, 40.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flex_cascade_parity_justify() {
    check(
        "justify (cascade)",
        "<!doctype html><body style=\"margin:0\"><div id=f1 style=\"display:flex;justify-content:center;width:500px;height:20px\"><div id=a style=\"width:100px\"></div></div><div id=f2 style=\"display:flex;justify-content:flex-end;width:500px;height:20px\"><div id=b style=\"width:100px\"></div></div></body>",
        0,
        &[
            ("f1", [0.0, 0.0, 500.0, 20.0, 500.0, 20.0, 500.0, 20.0]),
            ("a", [200.0, 0.0, 100.0, 20.0, 100.0, 20.0, 100.0, 20.0]),
            ("f2", [0.0, 20.0, 500.0, 20.0, 500.0, 20.0, 500.0, 20.0]),
            ("b", [400.0, 20.0, 100.0, 20.0, 100.0, 20.0, 100.0, 20.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flex_cascade_parity_stretch() {
    check(
        "stretch (cascade)",
        "<!doctype html><body style=\"margin:0\"><div id=f style=\"display:flex;width:400px\"><div id=a style=\"width:100px;height:50px\"></div><div id=b style=\"width:100px\"></div></div></body>",
        0,
        &[
            ("f", [0.0, 0.0, 400.0, 50.0, 400.0, 50.0, 400.0, 50.0]),
            ("a", [0.0, 0.0, 100.0, 50.0, 100.0, 50.0, 100.0, 50.0]),
            ("b", [100.0, 0.0, 100.0, 50.0, 100.0, 50.0, 100.0, 50.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flex_cascade_parity_wrap() {
    check(
        "wrap (cascade)",
        "<!doctype html><body style=\"margin:0\"><div id=f style=\"display:flex;flex-wrap:wrap;width:250px;gap:10px\"><div id=a style=\"width:100px;height:20px\"></div><div id=b style=\"width:100px;height:30px\"></div><div id=c style=\"width:100px;height:20px\"></div></div></body>",
        0,
        &[
            ("f", [0.0, 0.0, 250.0, 60.0, 250.0, 60.0, 250.0, 60.0]),
            ("a", [0.0, 0.0, 100.0, 20.0, 100.0, 20.0, 100.0, 20.0]),
            ("b", [110.0, 0.0, 100.0, 30.0, 100.0, 30.0, 100.0, 30.0]),
            ("c", [0.0, 40.0, 100.0, 20.0, 100.0, 20.0, 100.0, 20.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flex_cascade_parity_shell() {
    check(
        "shell (cascade)",
        "<!doctype html><body style=\"margin:0\"><div id=app style=\"display:flex;flex-direction:column;height:600px\"><header id=h style=\"height:50px\"></header><main id=m style=\"display:flex;flex:1;min-height:0\"><nav id=n style=\"width:200px\"></nav><section id=s style=\"flex:1;overflow:auto\"><div id=content style=\"height:2000px\"></div></section></main></div></body>",
        0,
        &[
            ("app", [0.0, 0.0, 800.0, 600.0, 800.0, 600.0, 800.0, 600.0]),
            ("h", [0.0, 0.0, 800.0, 50.0, 800.0, 50.0, 800.0, 50.0]),
            ("m", [0.0, 50.0, 800.0, 550.0, 800.0, 550.0, 800.0, 550.0]),
            ("n", [0.0, 50.0, 200.0, 550.0, 200.0, 550.0, 200.0, 550.0]),
            ("s", [200.0, 50.0, 600.0, 550.0, 600.0, 550.0, 600.0, 2000.0]),
            ("content", [200.0, 50.0, 600.0, 2000.0, 600.0, 2000.0, 600.0, 2000.0]),
        ],
        [800.0, 600.0],
    );
}

// ----- Stage 3: tables -----------------------------------------------------------

#[test]
fn table_parity_fixedcols() {
    flex_check(
        "fixedcols",
        "<!doctype html><body style=\"margin:0\"><table id=t style=\"border-collapse:collapse;table-layout:fixed;width:600px\"><thead><tr id=hr><th id=h1 style=\"width:100px;height:30px;padding:0\"></th><th id=h2 style=\"width:200px;padding:0\"></th><th id=h3 style=\"padding:0\"></th></tr></thead><tbody><tr id=r1><td id=c1 style=\"height:40px;padding:0\"></td><td id=c2 style=\"padding:0\"></td><td id=c3 style=\"padding:0\"></td></tr><tr id=r2><td id=d1 style=\"height:25px;padding:0\"></td><td id=d2 style=\"padding:0\"></td><td id=d3 style=\"padding:0\"></td></tr></tbody></table></body>",
        &[
            ("t", [0.0, 0.0, 600.0, 95.0, 600.0, 95.0, 600.0, 95.0]),
            ("hr", [0.0, 0.0, 600.0, 30.0, 600.0, 30.0, 600.0, 30.0]),
            ("h1", [0.0, 0.0, 100.0, 30.0, 100.0, 30.0, 100.0, 30.0]),
            ("h2", [100.0, 0.0, 200.0, 30.0, 200.0, 30.0, 200.0, 30.0]),
            ("h3", [300.0, 0.0, 300.0, 30.0, 300.0, 30.0, 300.0, 30.0]),
            ("r1", [0.0, 30.0, 600.0, 40.0, 600.0, 40.0, 600.0, 40.0]),
            ("c1", [0.0, 30.0, 100.0, 40.0, 100.0, 40.0, 100.0, 40.0]),
            ("c2", [100.0, 30.0, 200.0, 40.0, 200.0, 40.0, 200.0, 40.0]),
            ("c3", [300.0, 30.0, 300.0, 40.0, 300.0, 40.0, 300.0, 40.0]),
            ("r2", [0.0, 70.0, 600.0, 25.0, 600.0, 25.0, 600.0, 25.0]),
            ("d1", [0.0, 70.0, 100.0, 25.0, 100.0, 25.0, 100.0, 25.0]),
            ("d2", [100.0, 70.0, 200.0, 25.0, 200.0, 25.0, 200.0, 25.0]),
            ("d3", [300.0, 70.0, 300.0, 25.0, 300.0, 25.0, 300.0, 25.0]),
        ],
    );
}

#[test]
fn table_parity_autocols() {
    flex_check(
        "autocols",
        "<!doctype html><body style=\"margin:0\"><table id=t style=\"border-collapse:collapse\"><tr><td id=a style=\"width:120px;height:20px;padding:0\"></td><td id=b style=\"width:80px;padding:0\"></td></tr><tr><td id=c style=\"height:30px;padding:0\"></td><td id=d style=\"padding:0\"></td></tr></table></body>",
        &[
            ("t", [0.0, 0.0, 200.0, 50.0, 200.0, 50.0, 200.0, 50.0]),
            ("a", [0.0, 0.0, 120.0, 20.0, 120.0, 20.0, 120.0, 20.0]),
            ("b", [120.0, 0.0, 80.0, 20.0, 80.0, 20.0, 80.0, 20.0]),
            ("c", [0.0, 20.0, 120.0, 30.0, 120.0, 30.0, 120.0, 30.0]),
            ("d", [120.0, 20.0, 80.0, 30.0, 80.0, 30.0, 80.0, 30.0]),
        ],
    );
}

#[test]
fn table_parity_spacing() {
    flex_check(
        "spacing",
        "<!doctype html><body style=\"margin:0\"><table id=t style=\"border-spacing:4px;width:300px;table-layout:fixed\"><tr><td id=a style=\"height:20px;padding:3px\"></td><td id=b style=\"padding:3px\"></td></tr></table></body>",
        &[
            ("t", [0.0, 0.0, 300.0, 34.0, 300.0, 34.0, 300.0, 34.0]),
            ("a", [4.0, 4.0, 144.0, 26.0, 144.0, 26.0, 144.0, 26.0]),
            ("b", [152.0, 4.0, 144.0, 26.0, 144.0, 26.0, 144.0, 26.0]),
        ],
    );
}

#[test]
fn table_parity_percent() {
    flex_check(
        "percent",
        "<!doctype html><body style=\"margin:0\"><div style=\"width:500px\"><table id=t style=\"width:100%;border-collapse:collapse;table-layout:fixed\"><tr><td id=a style=\"width:20%;height:10px;padding:0\"></td><td id=b style=\"padding:0\"></td></tr></table></div></body>",
        &[
            ("t", [0.0, 0.0, 500.0, 10.0, 500.0, 10.0, 500.0, 10.0]),
            ("a", [0.0, 0.0, 100.0, 10.0, 100.0, 10.0, 100.0, 10.0]),
            ("b", [100.0, 0.0, 400.0, 10.0, 400.0, 10.0, 400.0, 10.0]),
        ],
    );
}

#[test]
fn table_parity_scroller() {
    flex_check(
        "scroller",
        "<!doctype html><body style=\"margin:0\"><div id=sc style=\"width:400px;overflow:auto\"><table id=t style=\"border-collapse:collapse;table-layout:fixed;width:1200px\"><tr><td id=a style=\"width:300px;height:20px;padding:0\"></td><td id=b style=\"width:900px;padding:0\"></td></tr></table></div></body>",
        &[
            ("sc", [0.0, 0.0, 400.0, 20.0, 400.0, 20.0, 1200.0, 20.0]),
            ("t", [0.0, 0.0, 1200.0, 20.0, 1200.0, 20.0, 1200.0, 20.0]),
            ("a", [0.0, 0.0, 300.0, 20.0, 300.0, 20.0, 300.0, 20.0]),
            ("b", [300.0, 0.0, 900.0, 20.0, 900.0, 20.0, 900.0, 20.0]),
        ],
    );
}

#[test]
fn table_parity_spanmiddle() {
    flex_check(
        "spanmiddle",
        "<!doctype html><body style=\"margin:0\"><table id=t style=\"border-collapse:collapse\"><tr><th id=h colspan=2 style=\"height:20px;padding:0\"></th></tr><tr><td id=a style=\"width:100px;padding:0\"><div id=ad style=\"height:20px\"></div></td><td id=b style=\"width:150px;padding:0\"><div id=bd style=\"height:60px\"></div></td></tr></table></body>",
        &[
            ("t", [0.0, 0.0, 250.0, 80.0, 250.0, 80.0, 250.0, 80.0]),
            ("h", [0.0, 0.0, 250.0, 20.0, 250.0, 20.0, 250.0, 20.0]),
            ("a", [0.0, 20.0, 100.0, 60.0, 100.0, 60.0, 100.0, 60.0]),
            ("ad", [0.0, 40.0, 100.0, 20.0, 100.0, 20.0, 100.0, 20.0]),
            ("b", [100.0, 20.0, 150.0, 60.0, 150.0, 60.0, 150.0, 60.0]),
            ("bd", [100.0, 20.0, 150.0, 60.0, 150.0, 60.0, 150.0, 60.0]),
        ],
    );
}

// ----- form controls and percentage heights -----------------------------------
// Chromium 1.59.1 at 800x600 with a Tailwind-like preflight and a sans font.

const PREFLIGHT: &str = "<style>*,::before,::after{box-sizing:border-box;border:0 solid;margin:0;padding:0}input,button,textarea,select{font:inherit;color:inherit}html{font-size:16px;line-height:24px;font-family:ui-sans-serif,system-ui,sans-serif}</style>";

#[test]
fn flow_parity_form_controls() {
    let html = format!(
        "<!doctype html>{PREFLIGHT}<body><div id=bar style=\"display:flex;align-items:center;gap:8px;height:40px;padding:0 12px\"><input id=filter style=\"width:100%\"></div><div id=col style=\"display:flex;flex-direction:column;width:300px\"><input id=stretched><textarea id=ta rows=3></textarea></div><div id=line><input id=inl size=10><input id=cb type=checkbox></div><input id=blk style=\"display:block\"><select id=sel style=\"width:120px\"><option>x</option></select><button id=empty></button></body>"
    );
    check(
        "form-controls",
        &html,
        0,
        &[
            ("bar", [0.0, 0.0, 800.0, 40.0, 800.0, 40.0, 800.0, 40.0]),
            ("filter", [12.0, 8.0, 776.0, 24.0, 776.0, 24.0, 776.0, 24.0]),
            ("col", [0.0, 40.0, 300.0, 96.0, 300.0, 96.0, 300.0, 96.0]),
            ("stretched", [0.0, 40.0, 300.0, 24.0, 300.0, 24.0, 300.0, 24.0]),
            ("ta", [0.0, 64.0, 300.0, 72.0, 300.0, 72.0, 300.0, 72.0]),
            ("line", [0.0, 136.0, 800.0, 24.0, 800.0, 24.0, 800.0, 24.0]),
            ("inl", [0.0, 136.0, 91.0, 24.0, 91.0, 24.0, 91.0, 24.0]),
            ("blk", [0.0, 160.0, 175.0, 24.0, 175.0, 24.0, 175.0, 24.0]),
            ("sel", [0.0, 186.0, 120.0, 20.0, 120.0, 20.0, 120.0, 20.0]),
        ],
        [800.0, 600.0],
    );
    let p = page(&html);
    let cb = p.metrics("cb");
    assert_eq!((cb[0], cb[2], cb[3]), (91.0, 13.0, 13.0), "checkbox: {cb:?}");
    // An empty button has no box size (Chromium 0x0): Playwright treats it as hidden.
    let empty = p.metrics("empty");
    assert_eq!((empty[2], empty[3]), (0.0, 0.0), "empty button: {empty:?}");
}

#[test]
fn flow_parity_percent_heights_in_flex_shell() {
    let html = "<!doctype html><style>html,body{height:100%;margin:0}</style><body><div id=shell style=\"display:flex;flex-direction:column;height:100%\"><header id=hd style=\"height:50px\"></header><main id=main style=\"flex:1;min-height:0;display:flex\"><aside id=side style=\"width:200px;overflow:auto\"><div id=list style=\"height:2000px\"></div></aside><section id=sec style=\"flex:1;display:flex;flex-direction:column\"><div id=full style=\"height:100%\"><div id=half style=\"height:50%\"></div></div></section></main></div></body>";
    check(
        "percent-shell",
        html,
        0,
        &[
            ("shell", [0.0, 0.0, 800.0, 600.0, 800.0, 600.0, 800.0, 600.0]),
            ("hd", [0.0, 0.0, 800.0, 50.0, 800.0, 50.0, 800.0, 50.0]),
            ("main", [0.0, 50.0, 800.0, 550.0, 800.0, 550.0, 800.0, 550.0]),
            ("side", [0.0, 50.0, 200.0, 550.0, 200.0, 550.0, 200.0, 2000.0]),
            ("list", [0.0, 50.0, 200.0, 2000.0, 200.0, 2000.0, 200.0, 2000.0]),
            ("sec", [200.0, 50.0, 600.0, 550.0, 600.0, 550.0, 600.0, 550.0]),
            ("full", [200.0, 50.0, 600.0, 550.0, 600.0, 550.0, 600.0, 550.0]),
            ("half", [200.0, 50.0, 600.0, 275.0, 600.0, 275.0, 600.0, 275.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flow_parity_percent_heights_in_blocks() {
    let html = "<!doctype html><body style=\"margin:0\"><div id=wrap style=\"height:300px\"><div id=p100 style=\"height:100%\"><div id=p50 style=\"height:50%;max-height:120px\"></div><div id=pmin style=\"min-height:20%\"></div></div></div><div id=auto><div id=pauto style=\"height:50%\"></div></div></body>";
    check(
        "percent-blocks",
        html,
        0,
        &[
            ("wrap", [0.0, 0.0, 800.0, 300.0, 800.0, 300.0, 800.0, 300.0]),
            ("p100", [0.0, 0.0, 800.0, 300.0, 800.0, 300.0, 800.0, 300.0]),
            ("p50", [0.0, 0.0, 800.0, 120.0, 800.0, 120.0, 800.0, 120.0]),
            ("pmin", [0.0, 120.0, 800.0, 60.0, 800.0, 60.0, 800.0, 60.0]),
            ("auto", [0.0, 300.0, 800.0, 0.0, 800.0, 0.0, 800.0, 0.0]),
            ("pauto", [0.0, 300.0, 800.0, 0.0, 800.0, 0.0, 800.0, 0.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flow_parity_fixed_modal_with_input() {
    let html = format!(
        "<!doctype html>{PREFLIGHT}<body><div id=ov style=\"position:fixed;inset:0;display:flex;align-items:center;justify-content:center\"><div id=dlg style=\"width:400px;max-height:100%;overflow:auto;padding:16px\"><input id=din style=\"width:100%\"></div></div></body>"
    );
    check(
        "modal",
        &html,
        0,
        &[
            ("ov", [0.0, 0.0, 800.0, 600.0, 800.0, 600.0, 800.0, 600.0]),
            ("dlg", [200.0, 272.0, 400.0, 56.0, 400.0, 56.0, 400.0, 56.0]),
            ("din", [216.0, 288.0, 368.0, 24.0, 368.0, 24.0, 368.0, 24.0]),
        ],
        [800.0, 600.0],
    );
}

// ----- grid (Stage 4) -------------------------------------------------------
// Chromium 1.59.1, 800x600; inline grid declarations via InlineFlex.

#[test]
fn grid_parity_basic() {
    flex_check(
        "grid-basic",
        "<!doctype html><body style=\"margin:0\"><div id=g style=\"display:grid;grid-template-columns:100px 1fr 2fr;gap:10px;width:600px\"><div id=a style=\"height:30px\"></div><div id=b style=\"height:50px\"></div><div id=c style=\"height:20px\"></div><div id=d style=\"grid-column:span 2;height:10px\"></div><div id=e style=\"height:15px\"></div></div><div id=after style=\"height:5px\"></div></body>",
        &[
            ("g", [0.0, 0.0, 600.0, 75.0, 600.0, 75.0, 600.0, 75.0]),
            ("a", [0.0, 0.0, 100.0, 30.0, 100.0, 30.0, 100.0, 30.0]),
            ("b", [110.0, 0.0, 160.0, 50.0, 160.0, 50.0, 160.0, 50.0]),
            ("c", [280.0, 0.0, 320.0, 20.0, 320.0, 20.0, 320.0, 20.0]),
            ("d", [0.0, 60.0, 270.0, 10.0, 270.0, 10.0, 270.0, 10.0]),
            ("e", [280.0, 60.0, 320.0, 15.0, 320.0, 15.0, 320.0, 15.0]),
            ("after", [0.0, 75.0, 800.0, 5.0, 800.0, 5.0, 800.0, 5.0]),
        ],
    );
}

#[test]
fn grid_parity_tailwind_columns() {
    // Fractional widths: Chromium sizes fr tracks in 1/64 px (LayoutUnit),
    // so a third of 668px is 222.65625.
    let close = |name: &str, html: &str, expected: &[(&str, [f64; 4])]| {
        let p = page(html);
        let result = p.layout.layout_result(&p.dom);
        for (id, want) in expected {
            let r = result.get(p.id(id)).unwrap().rect;
            let got = [r.x, r.y, r.width, r.height];
            assert!(
                got.iter().zip(want).all(|(g, w)| (g - w).abs() < 0.01),
                "{name} #{id}: got {got:?}, Chromium {want:?}"
            );
        }
    };
    close(
        "grid-tailwind",
        "<!doctype html><body style=\"margin:0\"><div id=g style=\"display:grid;grid-template-columns:repeat(3,minmax(0,1fr));column-gap:16px;row-gap:8px;width:700px;padding:12px\"><div id=a style=\"height:30px\"></div><div id=b style=\"grid-column:span 2 / span 2;height:40px\"></div><div id=c style=\"grid-column:1 / -1;height:20px\"></div><div id=d style=\"height:25px\"></div></div><div id=p style=\"display:grid;grid-template-columns:48.65% 51.35%;width:500px\"><div id=p1 style=\"height:10px\"></div><div id=p2 style=\"height:12px\"></div></div></body>",
        &[
            ("g", [0.0, 0.0, 724.0, 125.0]),
            ("a", [12.0, 12.0, 222.65625, 30.0]),
            ("b", [250.65625, 12.0, 461.34375, 40.0]),
            ("c", [12.0, 60.0, 700.0, 20.0]),
            ("d", [12.0, 88.0, 222.65625, 25.0]),
            ("p", [0.0, 125.0, 500.0, 12.0]),
            ("p1", [0.0, 125.0, 243.25, 10.0]),
            ("p2", [243.25, 125.0, 256.75, 12.0]),
        ],
    );
}

#[test]
fn grid_fr_columns_with_min_width_0_items_ignore_their_content() {
    // Column sizing reads nothing of a `min-width: 0` item in an `fr` column, so Taffy skips its
    // min-content re-check after row sizing (vendor/taffy README); the columns stay an even split
    // however wide the content is, and an `auto` column beside them still takes its content's width.
    let word = "x".repeat(80);
    let html = format!(
        "<!doctype html><body style=\"margin:0\"><div style=\"display:grid;grid-template-columns:1fr 1fr;width:400px\"><div id=a style=\"min-width:0\">{word}</div><div id=b style=\"min-width:0\">short</div></div><div style=\"display:grid;grid-template-columns:auto 1fr;width:400px\"><div id=c><span id=cs>auto</span></div><div id=d style=\"min-width:0\">{word}</div></div></body>"
    );
    let p = page(&html);
    let result = p.layout.layout_result(&p.dom);
    let rect = |id: &str| result.get(p.id(id)).unwrap().rect;
    assert_eq!((rect("a").x, rect("a").width), (0.0, 200.0));
    assert_eq!((rect("b").x, rect("b").width), (200.0, 200.0));
    let (c, cs, d) = (rect("c"), rect("cs"), rect("d"));
    assert!((c.width - cs.width).abs() < 0.01, "auto column {c:?} is its content {cs:?}");
    assert!((d.x - c.width).abs() < 0.01 && (c.width + d.width - 400.0).abs() < 0.01, "{c:?} {d:?}");
}

#[test]
fn grid_parity_calendar_fr_rows_with_min_height() {
    let cells = |n: usize| "<div></div>".repeat(n);
    let html = format!(
        "<!doctype html><body style=\"margin:0\"><div id=g style=\"display:grid;gap:1px;grid-template-columns:84px repeat(7,minmax(0px,1fr));grid-template-rows:auto 1fr 1fr 1fr;min-height:300px\"><div id=h0 style=\"height:20px\"></div><div id=h1 style=\"height:20px\"></div>{}<div id=w1></div><div id=c11 style=\"height:24px\"></div>{}<div id=c17></div><div id=w2></div><div id=c21></div>{}<div id=w3></div><div id=c31 style=\"height:120px\"></div></div></body>",
        cells(6),
        cells(5),
        cells(6)
    );
    let p = page(&html);
    let result = p.layout.layout_result(&p.dom);
    // (800 - 84 - 7) / 7 floored to 1/64 px, as Chromium's LayoutUnit does.
    let col = 101.28125;
    for (id, want) in [
        ("g", [0.0, 0.0, 800.0, 383.0]),
        ("h0", [0.0, 0.0, 84.0, 20.0]),
        ("h1", [85.0, 0.0, col, 20.0]),
        ("w1", [0.0, 21.0, 84.0, 120.0]),
        ("c11", [85.0, 21.0, col, 24.0]),
        ("c17", [698.703125, 21.0, col, 120.0]),
        ("w2", [0.0, 142.0, 84.0, 120.0]),
        ("c21", [85.0, 142.0, col, 120.0]),
        ("w3", [0.0, 263.0, 84.0, 120.0]),
        ("c31", [85.0, 263.0, col, 120.0]),
    ] {
        let r = result.get(p.id(id)).unwrap().rect;
        let got = [r.x, r.y, r.width, r.height];
        assert!(
            got.iter().zip(want).all(|(g, w)| (g - w).abs() < 0.01),
            "calendar #{id}: got {got:?}, Chromium {want:?}"
        );
    }
}

#[test]
fn grid_parity_auto_fill_and_fit() {
    flex_check(
        "grid-autofill",
        "<!doctype html><body style=\"margin:0\"><div id=g style=\"display:grid;grid-template-columns:repeat(auto-fill,minmax(120px,1fr));gap:10px;width:500px\"><div id=a style=\"height:10px\"></div><div id=b style=\"height:10px\"></div><div id=c style=\"height:10px\"></div><div id=d style=\"height:10px\"></div><div id=e style=\"height:10px\"></div></div><div id=f style=\"display:grid;grid-template-columns:repeat(auto-fit,minmax(100px,1fr));width:500px\"><div id=f1 style=\"height:10px\"></div><div id=f2 style=\"height:10px\"></div></div></body>",
        &[
            ("g", [0.0, 0.0, 500.0, 30.0, 500.0, 30.0, 500.0, 30.0]),
            ("a", [0.0, 0.0, 160.0, 10.0, 160.0, 10.0, 160.0, 10.0]),
            ("b", [170.0, 0.0, 160.0, 10.0, 160.0, 10.0, 160.0, 10.0]),
            ("c", [340.0, 0.0, 160.0, 10.0, 160.0, 10.0, 160.0, 10.0]),
            ("d", [0.0, 20.0, 160.0, 10.0, 160.0, 10.0, 160.0, 10.0]),
            ("e", [170.0, 20.0, 160.0, 10.0, 160.0, 10.0, 160.0, 10.0]),
            ("f", [0.0, 30.0, 500.0, 10.0, 500.0, 10.0, 500.0, 10.0]),
            ("f1", [0.0, 30.0, 250.0, 10.0, 250.0, 10.0, 250.0, 10.0]),
            ("f2", [250.0, 30.0, 250.0, 10.0, 250.0, 10.0, 250.0, 10.0]),
        ],
    );
}

#[test]
fn grid_parity_alignment() {
    flex_check(
        "grid-align",
        "<!doctype html><body style=\"margin:0\"><div id=g style=\"display:grid;grid-template-columns:200px 200px;grid-auto-rows:80px;justify-items:center;align-items:end;width:400px\"><div id=a style=\"width:50px;height:20px\"></div><div id=b style=\"width:60px;height:30px;justify-self:end;align-self:start\"></div><div id=c style=\"height:10px;justify-self:stretch;align-self:stretch\"></div><div id=d style=\"width:40px;height:40px;align-self:center;justify-self:start\"></div></div></body>",
        &[
            ("g", [0.0, 0.0, 400.0, 160.0, 400.0, 160.0, 400.0, 160.0]),
            ("a", [75.0, 60.0, 50.0, 20.0, 50.0, 20.0, 50.0, 20.0]),
            ("b", [340.0, 0.0, 60.0, 30.0, 60.0, 30.0, 60.0, 30.0]),
            ("c", [0.0, 80.0, 200.0, 10.0, 200.0, 10.0, 200.0, 10.0]),
            ("d", [200.0, 100.0, 40.0, 40.0, 40.0, 40.0, 40.0, 40.0]),
        ],
    );
}

#[test]
fn grid_parity_explicit_lines() {
    flex_check(
        "grid-lines",
        "<!doctype html><body style=\"margin:0\"><div id=g style=\"display:grid;grid-template-columns:100px 100px 100px;grid-template-rows:40px;width:300px\"><div id=a style=\"grid-column:2 / 4;grid-row:2\"></div><div id=b style=\"height:15px\"></div><div id=c style=\"grid-column:1;grid-row:1 / 3\"></div><div id=d style=\"height:25px\"></div></div></body>",
        &[
            ("g", [0.0, 0.0, 300.0, 40.0, 300.0, 40.0, 300.0, 40.0]),
            ("a", [100.0, 40.0, 200.0, 0.0, 200.0, 0.0, 200.0, 0.0]),
            ("b", [100.0, 0.0, 100.0, 15.0, 100.0, 15.0, 100.0, 15.0]),
            ("c", [0.0, 0.0, 100.0, 40.0, 100.0, 40.0, 100.0, 40.0]),
            ("d", [200.0, 0.0, 100.0, 25.0, 100.0, 25.0, 100.0, 25.0]),
        ],
    );
}

#[test]
fn grid_parity_auto_columns() {
    flex_check(
        "grid-autocol",
        "<!doctype html><body style=\"margin:0\"><div id=g style=\"display:grid;grid-template-columns:auto 1fr;gap:12px;width:600px\"><div id=a style=\"width:120px;height:20px\"></div><div id=b style=\"height:20px\"></div><div id=c style=\"width:80px;height:30px\"></div><div id=d></div></div><div id=s style=\"display:grid;grid-template-columns:auto auto;width:600px\"><div id=s1 style=\"width:100px;height:10px\"></div><div id=s2 style=\"width:50px;height:10px\"></div></div></body>",
        &[
            ("g", [0.0, 0.0, 600.0, 62.0, 600.0, 62.0, 600.0, 62.0]),
            ("a", [0.0, 0.0, 120.0, 20.0, 120.0, 20.0, 120.0, 20.0]),
            ("b", [132.0, 0.0, 468.0, 20.0, 468.0, 20.0, 468.0, 20.0]),
            ("c", [0.0, 32.0, 80.0, 30.0, 80.0, 30.0, 80.0, 30.0]),
            ("d", [132.0, 32.0, 468.0, 30.0, 468.0, 30.0, 468.0, 30.0]),
            ("s", [0.0, 62.0, 600.0, 10.0, 600.0, 10.0, 600.0, 10.0]),
            ("s1", [0.0, 62.0, 100.0, 10.0, 100.0, 10.0, 100.0, 10.0]),
            ("s2", [325.0, 62.0, 50.0, 10.0, 50.0, 10.0, 50.0, 10.0]),
        ],
    );
}

#[test]
fn grid_auto_track_after_fr_takes_its_content_width_like_chromium() {
    // Gap 31: `grid-template-columns: 1fr auto` gave the fr track the whole
    // width, so the auto track's flex item sat past the grid's edge
    // (x 504-635 in a 516px box-sizing grid with 24px padding). Chromium
    // (800x600): the auto track is its item's max-content (131), 1fr the
    // rest (468 - 12 - 131 = 325), and the input fills its track.
    flex_check(
        "grid-fr-auto",
        "<!doctype html><body style=\"margin:0\"><style>input{width:100%;height:20px;margin:0;padding:0;border:0;box-sizing:border-box}</style>\
         <div id=g style=\"display:grid;grid-template-columns:1fr auto;gap:12px;width:516px;padding:0 24px;box-sizing:border-box\">\
         <input id=i><div id=f style=\"display:flex\"><div id=b style=\"width:131px;height:20px\"></div></div></div></body>",
        &[
            ("g", [0.0, 0.0, 516.0, 20.0, 516.0, 20.0, 516.0, 20.0]),
            ("i", [24.0, 0.0, 325.0, 20.0, 325.0, 20.0, 325.0, 20.0]),
            ("f", [361.0, 0.0, 131.0, 20.0, 131.0, 20.0, 131.0, 20.0]),
            ("b", [361.0, 0.0, 131.0, 20.0, 131.0, 20.0, 131.0, 20.0]),
        ],
    );
}

#[test]
fn flow_hit_test_prefers_sticky_header_cell_over_its_row_group() {
    // TOS contact-role: a sticky <th> inside a `position: relative` thead in
    // a positioned scroller. Chromium: elementFromPoint at the th is the th.
    let p = page("<!doctype html><body><div id=scroller style=\"position:relative;overflow:auto;height:200px\"><table style=\"border-collapse:separate;border-spacing:0;width:400px\"><thead id=head style=\"position:relative;z-index:10\"><tr><th id=name style=\"position:sticky;top:0;width:200px;height:30px;padding:0\">Name</th><th id=other style=\"position:sticky;top:0;height:30px;padding:0\">Other</th></tr></thead><tbody><tr><td id=cell style=\"height:40px\">x</td><td>y</td></tr></tbody></table></div></body>");
    let r = p.layout.bounding_client_rect(&p.dom, p.id("name"));
    assert_eq!((r.x, r.y, r.width, r.height), (8.0, 8.0, 200.0, 30.0), "Chromium rect of #name");
    let hit = p.layout.element_from_point(&p.dom, r.x + 5.0, r.y + 5.0);
    assert_eq!(hit, Some(p.id("name")), "Chromium hits #name");
}

#[test]
fn flex_parity_auto_margins() {
    // Auto margins absorb free space before justify-content and override
    // align-self (no stretch) in the cross axis. Chromium 1.59.1.
    let html = "<!doctype html><body style=\"margin:0\"><div id=row style=\"display:flex;width:600px;height:50px;justify-content:center\"><div id=a style=\"width:100px;height:20px\"></div><div id=b style=\"width:80px;height:20px;margin-left:auto\"></div><div id=c style=\"width:60px;height:20px;margin:auto 10px auto auto\"></div></div><div id=col style=\"display:flex;flex-direction:column;height:200px;width:300px\"><div id=h style=\"height:30px\"></div><div id=f style=\"height:40px;margin-top:auto;margin-left:auto;width:100px\"></div></div><div id=cen style=\"display:flex;width:400px;height:100px;align-items:stretch\"><div id=m style=\"width:50px;margin:auto\"><div style=\"height:24px\"></div></div><div id=s style=\"width:50px\"></div><div id=t style=\"width:50px;margin-top:auto\"><div style=\"height:10px\"></div></div></div></body>";
    check(
        "auto-margins",
        html,
        0,
        &[
            ("row", [0.0, 0.0, 600.0, 50.0, 600.0, 50.0, 600.0, 50.0]),
            ("a", [0.0, 0.0, 100.0, 20.0, 100.0, 20.0, 100.0, 20.0]),
            ("b", [275.0, 0.0, 80.0, 20.0, 80.0, 20.0, 80.0, 20.0]),
            ("c", [530.0, 15.0, 60.0, 20.0, 60.0, 20.0, 60.0, 20.0]),
            ("col", [0.0, 50.0, 300.0, 200.0, 300.0, 200.0, 300.0, 200.0]),
            ("h", [0.0, 50.0, 300.0, 30.0, 300.0, 30.0, 300.0, 30.0]),
            ("f", [200.0, 210.0, 100.0, 40.0, 100.0, 40.0, 100.0, 40.0]),
            ("cen", [0.0, 250.0, 400.0, 100.0, 400.0, 100.0, 400.0, 100.0]),
            ("m", [125.0, 288.0, 50.0, 24.0, 50.0, 24.0, 50.0, 24.0]),
            ("s", [300.0, 250.0, 50.0, 100.0, 50.0, 100.0, 50.0, 100.0]),
            ("t", [350.0, 340.0, 50.0, 10.0, 50.0, 10.0, 50.0, 10.0]),
        ],
        [800.0, 600.0],
    );
}

// ----- text-align -------------------------------------------------------------

#[test]
fn flow_parity_text_align() {
    // Chromium 1.59.1, 800x600, system UI font. Atomic boxes with explicit sizes; the inline
    // span's own height is font-metric dependent, so it is checked on x/width.
    let html = "<!doctype html><body style=\"margin:0;font-family:-apple-system,BlinkMacSystemFont,sans-serif\"><div id=c style=\"width:400px;text-align:center\"><span id=c1 style=\"display:inline-block;width:50px;height:20px\"></span><span id=c2 style=\"display:inline-block;width:30px;height:20px\"></span></div><div id=r style=\"width:400px;text-align:right\"><span id=r1 style=\"display:inline-block;width:50px;height:20px\"></span> </div><div id=w style=\"width:400px;text-align:center\"><span id=w1 style=\"display:inline-block;width:250px;height:10px\"></span><span id=w2 style=\"display:inline-block;width:250px;height:10px\"></span></div><div id=o style=\"width:400px;text-align:center\"><span id=o1 style=\"display:inline-block;width:500px;height:10px\"></span></div><div id=e style=\"width:588px;padding:0 32px;text-align:center\"><button id=btn style=\"display:inline-flex;width:127px;height:28px;padding:0;border:0\"></button></div><div id=s style=\"width:300px;text-align:end\"><span id=sp><span id=s1 style=\"display:inline-block;width:40px;height:10px\"></span></span></div></body>";
    let p = page(html);
    let result = p.layout.layout_result(&p.dom);
    let mut diffs = Vec::new();
    for (id, want) in [
        ("c", [0.0, 0.0, 400.0, 23.0]),
        ("c1", [160.0, 0.0, 50.0, 20.0]),
        ("c2", [210.0, 0.0, 30.0, 20.0]),
        ("r", [0.0, 23.0, 400.0, 23.0]),
        ("r1", [350.0, 23.0, 50.0, 20.0]),
        ("w", [0.0, 46.0, 400.0, 36.0]),
        ("w1", [75.0, 51.0, 250.0, 10.0]),
        ("w2", [75.0, 69.0, 250.0, 10.0]),
        ("o", [0.0, 82.0, 400.0, 18.0]),
        ("o1", [0.0, 87.0, 500.0, 10.0]),
        ("e", [0.0, 100.0, 652.0, 31.0]),
        ("btn", [262.5, 100.0, 127.0, 28.0]),
        ("s", [0.0, 131.0, 300.0, 18.0]),
        ("s1", [260.0, 136.0, 40.0, 10.0]),
    ] {
        let r = result.get(p.id(id)).unwrap().rect;
        let got = [r.x, r.y, r.width, r.height];
        if got.iter().zip(&want).any(|(g, w)| (g - w).abs() > 0.01) {
            diffs.push(format!("#{id}: got {got:?}, Chromium {want:?}"));
        }
    }
    let sp = result.get(p.id("sp")).unwrap().rect;
    if (sp.x - 260.0).abs() > 0.01 || (sp.width - 40.0).abs() > 0.01 {
        diffs.push(format!("#sp: got x={} w={}, Chromium x=260 w=40", sp.x, sp.width));
    }
    assert!(diffs.is_empty(), "text-align:\n{}", diffs.join("\n"));
}

#[test]
fn flex_parity_cross_size_min_max() {
    // TOS positions header: a `max-h-[40vh] overflow-y-auto` flex item in a
    // row container must be clamped (it pushed the page 364px down).
    // Stretched items respect min/max height; column items min/max width.
    let html = "<!doctype html><body style=\"margin:0\"><div id=row style=\"display:flex;align-items:flex-start;gap:8px;width:600px\"><div id=panel style=\"flex:1;display:flex;flex-direction:column;max-height:100px;overflow-y:auto\"><div id=tall style=\"height:300px;flex-shrink:0\"></div></div><div id=side style=\"width:50px;height:40px\"></div></div><div id=after style=\"height:10px\"></div><div id=st style=\"display:flex;height:200px;width:400px\"><div id=s1 style=\"width:50px;max-height:120px\"></div><div id=s2 style=\"width:50px;min-height:250px\"></div><div id=s3 style=\"width:50px\"></div></div><div id=col style=\"display:flex;flex-direction:column;width:400px\"><div id=c1 style=\"max-width:150px;height:10px\"></div><div id=c2 style=\"min-width:450px;height:10px\"></div></div></body>";
    check(
        "cross-min-max",
        html,
        0,
        &[
            ("row", [0.0, 0.0, 600.0, 100.0, 600.0, 100.0, 600.0, 100.0]),
            ("panel", [0.0, 0.0, 542.0, 100.0, 542.0, 100.0, 542.0, 300.0]),
            ("tall", [0.0, 0.0, 542.0, 300.0, 542.0, 300.0, 542.0, 300.0]),
            ("side", [550.0, 0.0, 50.0, 40.0, 50.0, 40.0, 50.0, 40.0]),
            ("after", [0.0, 100.0, 800.0, 10.0, 800.0, 10.0, 800.0, 10.0]),
            ("st", [0.0, 110.0, 400.0, 200.0, 400.0, 200.0, 400.0, 250.0]),
            ("s1", [0.0, 110.0, 50.0, 120.0, 50.0, 120.0, 50.0, 120.0]),
            ("s2", [50.0, 110.0, 50.0, 250.0, 50.0, 250.0, 50.0, 250.0]),
            ("s3", [100.0, 110.0, 50.0, 200.0, 50.0, 200.0, 50.0, 200.0]),
            ("col", [0.0, 310.0, 400.0, 20.0, 400.0, 20.0, 450.0, 20.0]),
            ("c1", [0.0, 310.0, 150.0, 10.0, 150.0, 10.0, 150.0, 10.0]),
            ("c2", [0.0, 320.0, 450.0, 10.0, 450.0, 10.0, 450.0, 10.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flow_nbsp_is_content_not_collapsible_space() {
    // TOS tooltips render `<span class="whitespace-nowrap">&nbsp;</span>` in a
    // flex column: Chromium gives it a space-wide box (4px at 13px Arial),
    // while ASCII white space alone produces no item.
    let html = "<!doctype html><body style=\"margin:0\"><div id=col style=\"display:flex;flex-direction:column;align-items:flex-start;font-size:16px\"><span id=nb style=\"white-space:nowrap\">&nbsp;</span><span id=ws>   </span></div><div id=f style=\"display:flex;font-size:16px\">&nbsp;</div></body>";
    let p = page(html);
    let nb = p.metrics("nb");
    assert!(nb[2] > 0.0 && nb[3] > 0.0, "nbsp span has a box: {nb:?}");
    let space = p.metrics("nb")[2];
    // Headless Chromium on macOS (default font Times, 16px): a no-break space is
    // 4px wide (the 4.19 recorded earlier came from another default font).
    assert!((space - 4.0).abs() < 0.01, "as wide as a space: {space}");
    let ws = p.metrics("ws");
    assert_eq!(ws[2], 0.0, "collapsible white space has no width: {ws:?}");
    // Text directly in a flex container that is only a no-break space is an item.
    let f = p.metrics("f");
    assert!(f[3] > 0.0, "nbsp flex item gives the container height: {f:?}");
}

#[test]
fn flow_parity_shrink_to_fit_min_content_floor() {
    // An absolutely positioned box in a 0-wide containing block keeps its
    // min-content width (TOS tooltips); with 50px available and 70px of
    // content it takes 50 and wraps. Chromium 1.59.1; x/width only (line
    // heights depend on font metrics).
    let html = "<!doctype html><body style=\"margin:0\"><div id=cb style=\"position:relative;width:0;height:20px;margin-left:100px\"><div id=tip style=\"position:absolute;top:0;left:0\"><span id=a style=\"display:inline-block;width:30px;height:10px\"></span> <span id=b style=\"display:inline-block;width:40px;height:10px\"></span></div></div><div id=cb2 style=\"position:relative;width:50px;height:20px\"><div id=tip2 style=\"position:absolute;top:0;left:0\"><span id=c style=\"display:inline-block;width:30px;height:10px\"></span><span id=d style=\"display:inline-block;width:40px;height:10px\"></span></div></div></body>";
    let p = page(html);
    let mut diffs = Vec::new();
    for (id, x, w) in [
        ("tip", 100.0, 40.0),
        ("a", 100.0, 30.0),
        ("b", 100.0, 40.0),
        ("tip2", 0.0, 50.0),
        ("c", 0.0, 30.0),
        ("d", 0.0, 40.0),
    ] {
        let m = p.metrics(id);
        if (m[0] - x).abs() > 0.01 || (m[2] - w).abs() > 0.01 {
            diffs.push(format!("#{id}: got x={} w={}, Chromium x={x} w={w}", m[0], m[2]));
        }
    }
    // b and d wrap below a and c.
    assert!(p.metrics("b")[1] > p.metrics("a")[1] && p.metrics("d")[1] > p.metrics("c")[1], "wrapped");
    assert!(diffs.is_empty(), "shrink-to-fit:\n{}", diffs.join("\n"));
}

#[test]
fn flex_parity_compressible_percentage_inputs() {
    // TOS cargo form: `w-full` inputs in flex:1 items shrink with the row
    // (no min-content contribution); inputs with auto width keep their
    // intrinsic minimum and overflow. Chromium 1.59.1.
    let html = format!(
        "<!doctype html>{PREFLIGHT}<body><div id=row style=\"display:flex;gap:16px;width:659px\"><div id=a style=\"flex:1\"><input id=ia style=\"width:100%\"></div><div id=b style=\"flex:1\"><input style=\"width:100%\"></div><div id=c style=\"flex:1\"><input style=\"width:100%\"></div><div id=d style=\"flex:1\"><input style=\"width:100%\"></div><div id=e style=\"flex:1\"><input id=ie style=\"width:100%\"></div></div><div id=fixed style=\"display:flex;width:300px\"><div id=f1 style=\"flex:1\"><input id=if1></div><div id=f2 style=\"flex:1\"><input></div></div></body>"
    );
    check(
        "compressible",
        &html,
        0,
        &[
            ("row", [0.0, 0.0, 659.0, 24.0, 659.0, 24.0, 659.0, 24.0]),
            ("a", [0.0, 0.0, 119.0, 24.0, 119.0, 24.0, 119.0, 24.0]),
            ("ia", [0.0, 0.0, 119.0, 24.0, 119.0, 24.0, 119.0, 24.0]),
            ("b", [135.0, 0.0, 119.0, 24.0, 119.0, 24.0, 119.0, 24.0]),
            ("c", [270.0, 0.0, 119.0, 24.0, 119.0, 24.0, 119.0, 24.0]),
            ("d", [405.0, 0.0, 119.0, 24.0, 119.0, 24.0, 119.0, 24.0]),
            ("e", [540.0, 0.0, 119.0, 24.0, 119.0, 24.0, 119.0, 24.0]),
            ("ie", [540.0, 0.0, 119.0, 24.0, 119.0, 24.0, 119.0, 24.0]),
            ("fixed", [0.0, 24.0, 300.0, 24.0, 300.0, 24.0, 350.0, 24.0]),
            ("f1", [0.0, 24.0, 175.0, 24.0, 175.0, 24.0, 175.0, 24.0]),
            ("if1", [0.0, 24.0, 175.0, 24.0, 175.0, 24.0, 175.0, 24.0]),
            ("f2", [175.0, 24.0, 175.0, 24.0, 175.0, 24.0, 175.0, 24.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn grid_parity_min_function_track() {
    // TOS terms form: repeat(auto-fit, minmax(min(100%, 20rem), 1fr)); the
    // min() mixes a percentage and a length, resolved against the grid.
    flex_check(
        "grid-min-track",
        "<!doctype html><body style=\"margin:0\"><div id=g style=\"display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,200px),1fr));gap:2px;width:700px\"><div id=a style=\"height:10px\"></div><div id=b style=\"height:10px\"></div><div id=c style=\"height:10px\"></div><div id=d style=\"height:10px\"></div></div><div id=n style=\"display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,200px),1fr));width:150px\"><div id=n1 style=\"height:10px\"></div><div id=n2 style=\"height:10px\"></div></div></body>",
        &[
            ("g", [0.0, 0.0, 700.0, 22.0, 700.0, 22.0, 700.0, 22.0]),
            ("a", [0.0, 0.0, 232.0, 10.0, 232.0, 10.0, 232.0, 10.0]),
            ("b", [234.0, 0.0, 232.0, 10.0, 232.0, 10.0, 232.0, 10.0]),
            ("c", [468.0, 0.0, 232.0, 10.0, 232.0, 10.0, 232.0, 10.0]),
            ("d", [0.0, 12.0, 232.0, 10.0, 232.0, 10.0, 232.0, 10.0]),
            ("n", [0.0, 22.0, 150.0, 20.0, 150.0, 20.0, 150.0, 20.0]),
            ("n1", [0.0, 22.0, 150.0, 10.0, 150.0, 10.0, 150.0, 10.0]),
            ("n2", [0.0, 32.0, 150.0, 10.0, 150.0, 10.0, 150.0, 10.0]),
        ],
    );
}

#[test]
fn flow_parity_br_line_boxes() {
    // ProseMirror's empty paragraph `<p><br></p>` is one line tall; two brs
    // make two lines; a br after content does not add a line.
    let html = "<!doctype html><body style=\"margin:0;line-height:20px\"><p id=p1 style=\"margin:0\"><br></p><p id=p2 style=\"margin:0\"><br><br></p><p id=p3 style=\"margin:0\"><span style=\"display:inline-block;width:10px;height:10px\"></span><br></p></body>";
    check(
        "br-lines",
        html,
        0,
        &[
            ("p1", [0.0, 0.0, 800.0, 20.0, 800.0, 20.0, 800.0, 20.0]),
            ("p2", [0.0, 20.0, 800.0, 40.0, 800.0, 40.0, 800.0, 40.0]),
            ("p3", [0.0, 60.0, 800.0, 20.0, 800.0, 20.0, 800.0, 20.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flow_parity_intrinsic_width_keywords() {
    // TOS `w-fit min-w-full` table wrapper: fit-content, clamped by
    // min-width:100%, takes the table's max-content width. Chromium 1.59.1;
    // x/width only (line heights depend on font metrics).
    let html = "<!doctype html><body style=\"margin:0\"><div id=fit style=\"width:fit-content\"><span style=\"display:inline-block;width:120px;height:10px\"></span><span style=\"display:inline-block;width:80px;height:10px\"></span></div><div id=fitmin style=\"width:fit-content;min-width:100%\"><div style=\"width:1200px;height:10px\"></div></div><div id=max style=\"width:max-content\"><span style=\"display:inline-block;width:500px;height:10px\"></span><span style=\"display:inline-block;width:500px;height:10px\"></span></div><div id=min style=\"width:min-content\"><span style=\"display:inline-block;width:60px;height:10px\"></span><span style=\"display:inline-block;width:90px;height:10px\"></span></div><div style=\"position:relative;height:40px\"><div id=abs style=\"position:absolute;width:max-content;left:700px\"><span style=\"display:inline-block;width:150px;height:10px\"></span><span style=\"display:inline-block;width:150px;height:10px\"></span></div></div></body>";
    let p = page(html);
    let result = p.layout.layout_result(&p.dom);
    let mut diffs = Vec::new();
    for (id, x, w) in
        [("fit", 0.0, 200.0), ("fitmin", 0.0, 1200.0), ("max", 0.0, 1000.0), ("min", 0.0, 90.0), ("abs", 700.0, 300.0)]
    {
        let r = result.get(p.id(id)).unwrap().rect;
        if (r.x - x).abs() > 0.01 || (r.width - w).abs() > 0.01 {
            diffs.push(format!("#{id}: got x={} w={}, Chromium x={x} w={w}", r.x, r.width));
        }
    }
    assert!(diffs.is_empty(), "intrinsic widths:\n{}", diffs.join("\n"));
    // min-content wraps its two boxes onto two lines; max-content keeps one.
    let h = |id: &str| result.get(p.id(id)).unwrap().rect.height;
    assert!(h("min") > h("fit") && h("max") == h("fit"), "min {} fit {} max {}", h("min"), h("fit"), h("max"));
}

#[test]
fn flow_parity_abs_border_box_min_height_and_scroll_extent() {
    // An abs box with inset:0 under border-box keeps its min-height as a
    // border-box size (TOS textarea mirror was 124 vs 120px), and a
    // positioned parent's scrollHeight covers its abs descendants.
    // Chromium 1.59.1.
    let html = "<!doctype html><style>*{box-sizing:border-box}</style><body style=\"margin:0\"><div id=rel style=\"position:relative;height:120px\"><div id=abs style=\"position:absolute;inset:0;height:100%;min-height:120px;padding:2px\"></div></div><div id=rel2 style=\"position:relative;height:50px\"><div id=abs2 style=\"position:absolute;top:0;bottom:0;min-height:80px;padding:10px;border:2px solid\"></div></div></body>";
    check(
        "abs-min-height",
        html,
        0,
        &[
            ("rel", [0.0, 0.0, 800.0, 120.0, 800.0, 120.0, 800.0, 120.0]),
            ("abs", [0.0, 0.0, 800.0, 120.0, 800.0, 120.0, 800.0, 120.0]),
            ("rel2", [0.0, 120.0, 800.0, 50.0, 800.0, 50.0, 800.0, 80.0]),
            ("abs2", [0.0, 120.0, 24.0, 80.0, 20.0, 76.0, 20.0, 76.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flow_parity_lh_unit() {
    // TOS BL-instructions textarea: `!min-h-[5lh]` (5 x 24px line height)
    // beats min-h-[8.625rem]; the `lh` unit did not resolve, so the section
    // was 68px short. Chromium 1.59.1.
    let html = "<!doctype html><style>*{box-sizing:border-box}</style><body style=\"margin:0\"><div id=a style=\"line-height:24px;min-height:5lh\"></div><div id=b style=\"line-height:20px;height:2lh;padding:4px\"></div><div id=c style=\"line-height:24px;min-height:138px;min-height:5lh !important\"></div></body>";
    check(
        "lh-unit",
        html,
        0,
        &[
            ("a", [0.0, 0.0, 800.0, 120.0, 800.0, 120.0, 800.0, 120.0]),
            ("b", [0.0, 120.0, 800.0, 40.0, 800.0, 40.0, 800.0, 40.0]),
            ("c", [0.0, 160.0, 800.0, 120.0, 800.0, 120.0, 800.0, 120.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flow_parity_registered_custom_properties() {
    // @property registrations: a non-inherited property takes its initial
    // value on children (Tailwind's --tw-leading/--tw-font-weight/...),
    // unless set on the element; unregistered and inherits:true ones
    // inherit; a registration without `inherits` is invalid and ignored.
    // Chromium 1.59.1.
    let html = "<!doctype html><style>@property --x{syntax:\"*\";inherits:false;initial-value:10px}@property --z{syntax:\"*\";inherits:true;initial-value:5px}@property --bad{syntax:\"*\"}.p{--x:40px;--y:30px;--z:20px;--bad:15px}</style><body style=\"margin:0\"><div class=p><div id=x style=\"height:var(--x)\"></div><div id=y style=\"height:var(--y)\"></div><div id=z style=\"height:var(--z)\"></div><div id=own style=\"--x:7px;height:var(--x)\"></div><div id=bad style=\"height:var(--bad)\"></div></div><div id=init style=\"height:var(--z)\"></div></body>";
    check(
        "registered-properties",
        html,
        0,
        &[
            ("x", [0.0, 0.0, 800.0, 10.0, 800.0, 10.0, 800.0, 10.0]),
            ("y", [0.0, 10.0, 800.0, 30.0, 800.0, 30.0, 800.0, 30.0]),
            ("z", [0.0, 40.0, 800.0, 20.0, 800.0, 20.0, 800.0, 20.0]),
            ("own", [0.0, 60.0, 800.0, 7.0, 800.0, 7.0, 800.0, 7.0]),
            ("bad", [0.0, 67.0, 800.0, 15.0, 800.0, 15.0, 800.0, 15.0]),
            ("init", [0.0, 82.0, 800.0, 5.0, 800.0, 5.0, 800.0, 5.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flow_parity_block_in_inline() {
    // A block-level box inside an inline element ends the line and takes
    // its own height (no strut): TOS's `<label>` around a `display:flex`
    // textarea was 7px too tall. Chromium 1.59.1; the inline span's own
    // box height is font-dependent, so it is not compared.
    let html = "<!doctype html><body style=\"margin:0\"><div id=w style=\"line-height:24px\"><label id=l><div id=b style=\"height:100px\"></div></label></div><div id=w2 style=\"line-height:24px\"><span id=s>text<div id=b2 style=\"height:50px\"></div>more</span></div></body>";
    check(
        "block-in-inline",
        html,
        0,
        &[
            ("w", [0.0, 0.0, 800.0, 100.0, 800.0, 100.0, 800.0, 100.0]),
            ("l", [0.0, 0.0, 800.0, 100.0, 0.0, 0.0, 0.0, 0.0]),
            ("b", [0.0, 0.0, 800.0, 100.0, 800.0, 100.0, 800.0, 100.0]),
            ("w2", [0.0, 100.0, 800.0, 98.0, 800.0, 98.0, 800.0, 98.0]),
            ("b2", [0.0, 124.0, 800.0, 50.0, 800.0, 50.0, 800.0, 50.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flow_parity_atomic_inline_baselines() {
    // TOS payments search field: an inline-flex icon box (only an svg) in a
    // 24px line at 15px sits on a baseline synthesized from its first item's
    // bottom edge, so the line is 27.6px (flow had 31: bottom on baseline).
    // inline-flex with text uses the text baseline; inline-block its last
    // line, or its bottom edge when empty or clipping. Chromium 1.59.1,
    // system UI font; Chromium snaps to 1/64px, so heights within 0.05px.
    let html = "<!doctype html><body style=\"margin:0;font-family:-apple-system,BlinkMacSystemFont,sans-serif;font-size:15px;line-height:24px\"><div id=a><span id=ai style=\"display:inline-flex;width:24px;height:24px;align-items:center;justify-content:center\"><span id=ag style=\"display:block;width:19.2px;height:19.2px\"></span></span></div><div id=b><span id=bi style=\"display:inline-flex;height:40px;align-items:center;padding:0 4px\">Go</span></div><div id=c><span id=ci style=\"display:inline-block;width:40px\">one two three</span></div><div id=d><span id=di style=\"display:inline-block;width:30px;height:20px\"></span></div><div id=e><span id=ei style=\"display:inline-block;width:80px;overflow:hidden\">clip</span></div><div id=f><span id=fi style=\"display:inline-flex;align-items:center;gap:4px\"><span style=\"display:block;width:16px;height:16px\"></span>Label</span></div></body>";
    let p = page(html);
    let result = p.layout.layout_result(&p.dom);
    let mut diffs = Vec::new();
    for (id, h) in [("a", 27.59), ("b", 40.0), ("c", 72.0), ("d", 26.0), ("e", 30.0), ("f", 26.0)] {
        let r = result.get(p.id(id)).unwrap().rect;
        if (r.height - h).abs() > 0.05 {
            diffs.push(format!("#{id}: height {} vs Chromium {h}", r.height));
        }
    }
    // The icon box itself starts at the line top.
    let ai = result.get(p.id("ai")).unwrap().rect;
    if ai.y.abs() > 0.05 {
        diffs.push(format!("#ai: y {} vs Chromium 0", ai.y));
    }
    assert!(diffs.is_empty(), "baselines:\n{}", diffs.join("\n"));
}

#[test]
fn flow_skips_relayout_for_identical_writes_but_not_real_changes() {
    // Writing an attribute or text to its current value is still a mutation
    // (records are queued) but changes nothing layout depends on: the flow
    // result is reused. Real changes must relayout with fresh geometry.
    let html = "<!doctype html><style>.w{width:100px}.x{width:300px}</style><body style=\"margin:0\"><div id=a class=w style=\"height:10px\"></div><span id=t>abc</span></body>";
    let mut p = page(html);
    let a = p.id("a");
    let first = p.layout.layout_result(&p.dom);
    assert_eq!(first.get(a).unwrap().rect.width, 100.0);
    let v = p.dom.version();
    p.dom.set_attribute(a, "class", "w").unwrap();
    p.dom.set_attribute(a, "style", "height:10px").unwrap();
    let text = p.dom.children(p.id("t")).unwrap().next().unwrap();
    p.dom.set_char_data(text, "abc").unwrap();
    assert_eq!(p.dom.version(), v, "identical writes keep the version");
    assert!(Rc::ptr_eq(&first, &p.layout.layout_result(&p.dom)), "flow result reused");
    // Real attribute change: fresh geometry, never the cached 100px.
    p.dom.set_attribute(a, "class", "x").unwrap();
    assert_ne!(p.dom.version(), v);
    assert_eq!(p.layout.layout_result(&p.dom).get(a).unwrap().rect.width, 300.0);
    // Real text change relayouts too (the text box grows).
    let before = p.layout.text_rect(&p.dom, text).unwrap().width;
    p.dom.set_char_data(text, "abcdef").unwrap();
    let after = p.layout.text_rect(&p.dom, text).unwrap().width;
    assert!(after > before, "text box {before} -> {after}");
}

#[test]
fn flow_parity_image_intrinsic_sizing() {
    // CSS 2.1 10.3.2 / 10.6.2 with a loaded image's natural size: the TOS
    // chat image (`w-full max-h-64`, 1x1 PNG in a 240px container) is
    // 240x240; both auto gives the natural size; one auto follows the ratio;
    // max-height with auto width keeps it. Chromium 1.59.1 with real PNGs.
    let html = "<!doctype html><body style=\"margin:0;line-height:20px\"><div id=box style=\"width:240px\"><img id=tos data-natural=\"1,1\" style=\"display:block;width:100%;max-height:256px\"></div><img id=nat data-natural=\"200,100\" style=\"display:block\"><img id=h50 data-natural=\"200,100\" style=\"display:block;height:50px\"><img id=mh data-natural=\"200,100\" style=\"display:block;max-height:40px\"><img id=w300 data-natural=\"200,100\" style=\"display:block;width:300px\"><div id=line><img id=inl data-natural=\"200,100\" style=\"height:20px\"></div><img id=none style=\"display:block\"></body>";
    let p = page(html);
    let result = p.layout.layout_result(&p.dom);
    let mut diffs = Vec::new();
    for (id, want) in [
        ("box", [0.0, 0.0, 240.0, 240.0]),
        ("tos", [0.0, 0.0, 240.0, 240.0]),
        ("nat", [0.0, 240.0, 200.0, 100.0]),
        ("h50", [0.0, 340.0, 100.0, 50.0]),
        ("mh", [0.0, 390.0, 80.0, 40.0]),
        ("w300", [0.0, 430.0, 300.0, 150.0]),
    ] {
        let r = result.get(p.id(id)).unwrap().rect;
        let got = [r.x, r.y, r.width, r.height];
        if got.iter().zip(&want).any(|(g, w)| (g - w).abs() > 0.01) {
            diffs.push(format!("#{id}: got {got:?}, Chromium {want:?}"));
        }
    }
    let inl = result.get(p.id("inl")).unwrap().rect;
    if (inl.width - 40.0).abs() > 0.01 || (inl.height - 20.0).abs() > 0.01 {
        diffs.push(format!("#inl: got {}x{}, Chromium 40x20", inl.width, inl.height));
    }
    // Without a natural size (not loaded) an image stays 0x0, as before.
    let none = result.get(p.id("none")).unwrap().rect;
    if none.width != 0.0 || none.height != 0.0 {
        diffs.push(format!("#none: got {}x{}", none.width, none.height));
    }
    assert!(diffs.is_empty(), "image sizing:\n{}", diffs.join("\n"));
}

/// Every box of `p`'s cached flow result equals a fresh layout (no caches)
/// of the same DOM.
fn assert_matches_fresh(p: &Page, step: &str) {
    let cached = p.layout.layout_result(&p.dom);
    let mut fresh = Layout::with_viewport(p.doc, Viewport::new(800, 600));
    fresh.style_mut().discover_style_elements(&p.dom);
    let f = fresh.layout_result(&p.dom);
    let mut diffs = Vec::new();
    for (k, b) in &f.boxes {
        match cached.boxes.get(k) {
            Some(c) if c.rect == b.rect => {}
            other => diffs.push(format!("{k:?}: cached {:?}, fresh {:?}", other.map(|c| c.rect), b.rect)),
        }
    }
    if cached.boxes.len() != f.boxes.len() {
        diffs.push(format!("box count cached {} fresh {}", cached.boxes.len(), f.boxes.len()));
    }
    assert!(diffs.is_empty(), "{step}: stale geometry\n{}", diffs.join("\n"));
}

#[test]
fn incremental_style_invalidation_never_serves_stale_geometry() {
    // Selective style invalidation (DOM change journal) must give exactly the
    // geometry of a full recomputation after each kind of change it handles:
    // descendant and sibling combinators, :has() anchors (group-has),
    // :nth-child after insertion, removal and reinsertion elsewhere, :empty
    // via text, inherited em sizes, form-control state, journal overflow.
    let css = "<style>\
        .on .child{width:200px} .child{width:100px;height:10px}\
        .a{height:10px} .a.on + .b{width:300px} .b{width:50px;height:10px}\
        .group:has(input:disabled) .icon{width:150px} .icon{width:20px;height:10px}\
        li{height:10px;width:10px} li:nth-child(2){width:222px}\
        .ctx .mv{width:333px} .mv{width:33px;height:10px}\
        .e:empty{height:7px} .e{width:40px}\
        .em{font-size:10px} .em.big{font-size:20px} .em span{display:block;width:3em;height:1em}\
        input:checked + .lab{width:250px} .lab{display:block;width:25px;height:10px}\
        </style>";
    let html = format!(
        "<!doctype html>{css}<body style=\"margin:0\"><div id=par><div id=child class=child></div></div><div id=a class=a></div><div id=b class=b></div><div class=group><div><input id=inp></div><div><div id=icon class=icon></div></div></div><ul id=ul><li id=l1></li><li id=l2></li></ul><div id=ctx class=ctx></div><div id=home><div id=mv class=mv></div></div><div id=e class=e></div><div id=em class=em><span id=emc>x</span></div><input id=cb type=checkbox><span id=lab class=lab></span></body>"
    );
    let mut p = page(&html);
    assert_matches_fresh(&p, "initial");
    let id = |p: &Page, s: &str| p.id(s);
    // Descendant combinator: ancestor attribute.
    let par = id(&p, "par");
    p.dom.set_attribute(par, "class", "on").unwrap();
    assert_matches_fresh(&p, "ancestor class");
    assert_eq!(p.layout.layout_result(&p.dom).get(id(&p, "child")).unwrap().rect.width, 200.0);
    // Sibling combinator.
    let a = id(&p, "a");
    p.dom.set_attribute(a, "class", "a on").unwrap();
    assert_matches_fresh(&p, "previous sibling class");
    assert_eq!(p.layout.layout_result(&p.dom).get(id(&p, "b")).unwrap().rect.width, 300.0);
    // :has() anchor: a descendant's attribute restyles a sibling subtree.
    let inp = id(&p, "inp");
    p.dom.set_attribute(inp, "disabled", "").unwrap();
    assert_matches_fresh(&p, "group-has descendant attribute");
    assert_eq!(p.layout.layout_result(&p.dom).get(id(&p, "icon")).unwrap().rect.width, 150.0);
    // :nth-child after an insertion before the first item.
    let ul = id(&p, "ul");
    let new_li = p.dom.create_element(p.doc, "li").unwrap();
    let l1 = id(&p, "l1");
    p.dom.insert_before(ul, new_li, Some(l1)).unwrap();
    assert_matches_fresh(&p, "insertion shifts nth-child");
    assert_eq!(p.layout.layout_result(&p.dom).get(l1).unwrap().rect.width, 222.0);
    // Removal, then reinsertion under a different context.
    let (home, mv, ctx) = (id(&p, "home"), id(&p, "mv"), id(&p, "ctx"));
    p.dom.remove_child(home, mv).unwrap();
    assert_matches_fresh(&p, "removal");
    p.dom.append_child(ctx, mv).unwrap();
    assert_matches_fresh(&p, "reinsertion elsewhere");
    assert_eq!(p.layout.layout_result(&p.dom).get(mv).unwrap().rect.width, 333.0);
    // :empty via text.
    let e = id(&p, "e");
    let t = p.dom.create_text(p.doc, "text").unwrap();
    p.dom.append_child(e, t).unwrap();
    assert_matches_fresh(&p, "text makes :empty false");
    p.dom.replace_data(t, 0, 4, "").unwrap();
    assert_matches_fresh(&p, "text emptied");
    // Inherited font size drives an em-sized child.
    let em = id(&p, "em");
    p.dom.set_attribute(em, "class", "em big").unwrap();
    assert_matches_fresh(&p, "inherited font-size");
    assert_eq!(p.layout.layout_result(&p.dom).get(id(&p, "emc")).unwrap().rect.width, 60.0);
    // Form-control state (:checked) feeding a sibling combinator.
    let cb = id(&p, "cb");
    p.dom.set_checked(cb, true).unwrap();
    assert_matches_fresh(&p, "checked state");
    assert_eq!(p.layout.layout_result(&p.dom).get(id(&p, "lab")).unwrap().rect.width, 250.0);
    // Journal overflow falls back to full invalidation.
    for i in 0..lp_dom::dom::JOURNAL_CAPACITY + 100 {
        p.dom.set_attribute(par, "data-n", &i.to_string()).unwrap();
    }
    p.dom.set_attribute(par, "class", "").unwrap();
    assert_matches_fresh(&p, "after journal overflow");
    assert_eq!(p.layout.layout_result(&p.dom).get(id(&p, "child")).unwrap().rect.width, 100.0);
}

#[test]
fn invalidating_most_of_the_page_stays_selective_without_stale_geometry() {
    // A React re-render of a large list restyles more than half of the cached
    // elements. Only those are dropped; the rest keep their cached styles and
    // measurements. Each step compares every box with a fresh layout: a
    // container class reaching every row (with a sibling combinator past the
    // subtree), an outside change afterwards, an inherited font size,
    // replacing every row, an insertion shifting :nth-child of all of them.
    let css = "<style>\
        .side .item{width:40px;height:10px} .side .item.on{width:41px}\
        .region{font-size:10px} .region.big{font-size:20px}\
        .row{height:10px} .region .row:first-child{height:12px}\
        .cell{display:block;width:30px;height:5px} .row:nth-child(2n) .cell{width:31px}\
        .region.dense .cell{width:20px} .cell span{display:block;width:2em;height:1em}\
        .after{width:50px;height:10px} .region.dense + .after{width:300px}\
        </style>";
    let row = |i: usize| {
        format!("<div class=row id=r{i}><div class=cell id=c{i}><span>x</span></div><div class=cell><span>y</span></div></div>")
    };
    let rows: String = (0..60).map(row).collect();
    let html = format!(
        "<!doctype html>{css}<body style=\"margin:0\"><div class=side><div id=item class=item></div></div>\
         <div id=region class=region>{rows}</div><div id=after class=after></div></body>"
    );
    let mut p = page(&html);
    assert_matches_fresh(&p, "initial");
    let (region, item, after) = (p.id("region"), p.id("item"), p.id("after"));
    let width = |p: &Page, n: NodeId| p.layout.layout_result(&p.dom).get(n).unwrap().rect.width;
    // The container's class restyles every row, and the sibling after it.
    p.dom.set_attribute(region, "class", "region dense").unwrap();
    assert_matches_fresh(&p, "container class over most of the page");
    assert_eq!(width(&p, p.id("c1")), 20.0);
    assert_eq!(width(&p, after), 300.0);
    // The elements outside it kept their entries; changing one still restyles it.
    p.dom.set_attribute(item, "class", "item on").unwrap();
    assert_matches_fresh(&p, "outside change after a large invalidation");
    assert_eq!(width(&p, item), 41.0);
    // Inherited font size into em-sized descendants of every row.
    p.dom.set_attribute(region, "class", "region dense big").unwrap();
    assert_matches_fresh(&p, "inherited font-size over most of the page");
    // Replace every row (a keyless React re-render), then shift :nth-child.
    for i in 0..60 {
        let r = p.id(&format!("r{i}"));
        p.dom.remove_child(region, r).unwrap();
    }
    for i in 0..60 {
        let r = p.dom.create_element(p.doc, "div").unwrap();
        p.dom.set_attribute(r, "class", "row").unwrap();
        p.dom.set_attribute(r, "id", &format!("n{i}")).unwrap();
        for (k, text) in ["x", "y"].into_iter().enumerate() {
            let cell = p.dom.create_element(p.doc, "div").unwrap();
            p.dom.set_attribute(cell, "class", "cell").unwrap();
            if k == 0 {
                p.dom.set_attribute(cell, "id", &format!("m{i}")).unwrap();
            }
            let span = p.dom.create_element(p.doc, "span").unwrap();
            let t = p.dom.create_text(p.doc, text).unwrap();
            p.dom.append_child(span, t).unwrap();
            p.dom.append_child(cell, span).unwrap();
            p.dom.append_child(r, cell).unwrap();
        }
        p.dom.append_child(region, r).unwrap();
    }
    assert_matches_fresh(&p, "every row replaced");
    p.dom.set_attribute(region, "class", "region").unwrap();
    assert_matches_fresh(&p, "container class removed");
    assert_eq!(width(&p, p.id("m1")), 31.0);
    let top = p.dom.create_element(p.doc, "div").unwrap();
    p.dom.set_attribute(top, "class", "row").unwrap();
    let n0 = p.id("n0");
    p.dom.insert_before(region, top, Some(n0)).unwrap();
    assert_matches_fresh(&p, "insertion shifts nth-child of every row");
    // n0 is now the second row.
    assert_eq!(width(&p, p.id("m0")), 31.0);
    assert_eq!(width(&p, item), 41.0);
}

#[test]
fn added_sheets_restyle_their_subjects_without_stale_geometry() {
    // A sheet added after layout (a lazily loaded chunk's CSS, a library's
    // injected <style>) restyles only what its rules match. Each step compares
    // every box with a fresh layout: unmatched scoped rules, a matched subset,
    // inherited font size into em sizes, generated content, a custom property
    // read through var(), sibling and :has() selectors, @media, then an edit of
    // a sheet with rules, a removed sheet and a viewport change (full restyles).
    let rows: String = (0..40)
        .map(|i| {
            format!(
                "<div class=row id=r{i}><div class=cell><span>x</span></div><div class=cell><span>y</span></div></div>"
            )
        })
        .collect();
    let html = format!(
        "<!doctype html><style>.row{{height:10px}} .cell{{display:block;width:30px;height:var(--h,5px)}} \
         .cell span{{display:block;width:2em;height:1em}}</style><body style=\"margin:0\"><div id=side style=\"height:1px\"></div>\
         <div id=list>{rows}</div><div id=after></div></body>"
    );
    let mut p = page(&html);
    assert_matches_fresh(&p, "initial");
    let add = |p: &mut Page, css: &str| {
        let style = p.dom.create_element(p.doc, "style").unwrap();
        let text = p.dom.create_text(p.doc, css).unwrap();
        p.dom.append_child(style, text).unwrap();
        let body = p.dom.body(p.doc).unwrap().unwrap();
        p.dom.append_child(body, style).unwrap();
        p.layout.style_mut().discover_style_elements(&p.dom);
        style
    };
    add(&mut p, ".doc-content p{height:99px} .doc-content>*+*{margin-top:3px} .doc-content:after{content:'';height:32px;display:block}");
    assert_matches_fresh(&p, "scoped sheet matching nothing");
    add(&mut p, ".row:nth-child(2n) .cell{width:31px}");
    assert_matches_fresh(&p, "sheet matching a subset");
    add(&mut p, "#list{font-size:20px}");
    assert_matches_fresh(&p, "inherited font size into em sizes");
    add(&mut p, "#r7::before{content:'z';display:block;height:6px}");
    assert_matches_fresh(&p, "generated content");
    add(&mut p, "#r9{--h:9px}");
    assert_matches_fresh(&p, "custom property through var()");
    add(&mut p, ".row + .row{margin-top:1px} #side:has(+ #list){height:4px} #r3:has(span){height:14px}");
    assert_matches_fresh(&p, "sibling and :has() selectors");
    let media = add(&mut p, "@media (min-width: 700px){ #after{height:12px} .row:first-child{height:15px} }");
    assert_matches_fresh(&p, "@media");
    // A DOM change in the same pass as an added sheet.
    let r2 = p.id("r2");
    p.dom.set_attribute(r2, "style", "height:22px").unwrap();
    add(&mut p, "#r2 .cell{width:40px}");
    assert_matches_fresh(&p, "DOM change with an added sheet");
    // Editing a sheet that has rules, removing one, and a viewport change.
    let text = p.dom.first_child(media).unwrap().unwrap();
    p.dom.replace_data(text, 0, 0, "#side{height:8px} ").unwrap();
    p.layout.style_mut().discover_style_elements(&p.dom);
    assert_matches_fresh(&p, "edited sheet");
    p.dom.remove_child(p.dom.parent(media).unwrap().unwrap(), media).unwrap();
    p.layout.style_mut().remove_sheet(media);
    assert_matches_fresh(&p, "removed sheet");
    p.layout.set_viewport(Viewport::new(600, 400));
    let mut fresh_vp = Layout::with_viewport(p.doc, Viewport::new(600, 400));
    fresh_vp.style_mut().discover_style_elements(&p.dom);
    let (cached, fresh) = (p.layout.layout_result(&p.dom), fresh_vp.layout_result(&p.dom));
    for (k, b) in &fresh.boxes {
        assert_eq!(cached.boxes.get(k).map(|c| c.rect), Some(b.rect), "viewport: {k:?}");
    }
}

#[test]
fn loaded_image_natural_size_reaches_flow_through_the_dom() {
    // TOS chat: <img class="max-h-64 w-full object-contain"> in a 240px
    // button, holding the uploaded 1x1 PNG. Unloaded it is 240x0 (hidden
    // to Playwright); once the embedder records the natural size it is
    // 240x240 as in Chromium, and a later size change relayouts.
    let mut p = page(
        "<body style='margin:0'><div style='width:240px'><button style='display:block;width:100%;padding:0;border:0'><span style='display:contents'><img id=i style='max-height:256px;width:100%'></span></button></div></body>"
    );
    let img = p.id("i");
    assert_eq!(&p.metrics("i")[2..4], &[240.0, 0.0]);
    p.dom.set_natural_size(img, Some((1.0, 1.0)));
    assert_eq!(&p.metrics("i")[2..4], &[240.0, 240.0]);
    p.dom.set_natural_size(img, Some((4.0, 1.0)));
    assert_eq!(&p.metrics("i")[2..4], &[240.0, 60.0]);
    p.dom.set_natural_size(img, None);
    assert_eq!(&p.metrics("i")[2..4], &[240.0, 0.0]);
}

/// Asserts the DOM journaled every change since `since` precisely (no
/// `Unknown`), so the style cache can invalidate selectively.
fn assert_journaled(p: &Page, since: u64, what: &str) {
    let changes: Vec<_> = p.dom.changes_since(since).expect("journal covers").collect();
    assert!(!changes.is_empty(), "{what}: nothing journaled");
    assert!(
        !changes.iter().any(|c| matches!(c, lp_dom::dom::Change::Unknown(_))),
        "{what}: journaled Unknown ({changes:?})"
    );
}

#[test]
fn form_state_writes_invalidate_selectively_without_stale_geometry() {
    // Form value/checked/selected writes and image natural sizes are
    // journaled precisely (they used to add an Unknown change, so every
    // React-controlled input write invalidated every style). Validity is not
    // local: a form, fieldset or select tested with :valid/:invalid follows
    // its controls, and a required radio group spans containers. Each step
    // compares every box with a fresh layout.
    let css = "<style>\
        .v:invalid{height:30px} .v{height:10px;display:block}\
        .fs:invalid{width:300px} .fs{width:100px;height:10px;display:block;border:0;padding:0;margin:0}\
        .sel:invalid{width:222px} .sel{width:22px;height:10px;display:block}\
        .r:invalid + .rl{width:250px} .rl{display:block;width:25px;height:10px}\
        .t:placeholder-shown + .tl{width:180px} .tl{display:block;width:18px;height:10px}\
        input,textarea{display:block;height:10px;width:50px}\
        img{display:block;width:40px}\
        </style>";
    let html = format!(
        "<!doctype html>{css}<body style=\"margin:0\">\
         <form id=f class=v><div><div><input id=req required></div></div></form>\
         <fieldset id=fs class=fs><div><input id=req2 required></div></fieldset>\
         <select id=sel class=sel required><option value=\"\">none</option><option id=o2 value=x>x</option></select>\
         <form><div><input type=radio class=r name=g id=r1 required><span id=rl1 class=rl></span></div>\
         <div><input type=radio class=r name=g id=r2><span id=rl2 class=rl></span></div></form>\
         <textarea id=t class=t placeholder=p></textarea><span id=tl class=tl></span>\
         <img id=img></body>"
    );
    let mut p = page(&html);
    assert_matches_fresh(&p, "initial");
    let w = |p: &Page, id: &str| p.layout.layout_result(&p.dom).get(p.id(id)).unwrap().rect;
    assert_eq!(w(&p, "f").height, 30.0);

    let v = p.dom.version();
    let req = p.id("req");
    p.dom.set_value(req, "x").unwrap();
    assert_journaled(&p, v, "set_value");
    assert_matches_fresh(&p, "value makes the form valid");
    assert_eq!(w(&p, "f").height, 10.0);

    let v = p.dom.version();
    let req2 = p.id("req2");
    p.dom.insert_text(req2, "y").unwrap();
    assert_journaled(&p, v, "insert_text");
    assert_matches_fresh(&p, "typing makes the fieldset valid");
    assert_eq!(w(&p, "fs").width, 100.0);

    let v = p.dom.version();
    let o2 = p.id("o2");
    p.dom.set_option_selected(o2, true).unwrap();
    assert_journaled(&p, v, "set_option_selected");
    assert_matches_fresh(&p, "option makes the select valid");
    assert_eq!(w(&p, "sel").width, 22.0);
    let sel = p.id("sel");
    p.dom.set_selected_index(sel, 0).unwrap();
    assert_matches_fresh(&p, "selectedIndex makes it invalid again");
    assert_eq!(w(&p, "sel").width, 222.0);

    // Checking r2 completes the required group: r1, in another container,
    // stops being :invalid.
    assert_eq!(w(&p, "rl1").width, 250.0);
    let v = p.dom.version();
    let r2 = p.id("r2");
    p.dom.set_checked(r2, true).unwrap();
    assert_journaled(&p, v, "set_checked");
    assert_matches_fresh(&p, "radio group validity");
    assert_eq!(w(&p, "rl1").width, 25.0);
    p.dom.set_checked(r2, false).unwrap();
    assert_matches_fresh(&p, "radio group invalid again");
    assert_eq!(w(&p, "rl1").width, 250.0);

    let t = p.id("t");
    p.dom.insert_text(t, "z").unwrap();
    assert_matches_fresh(&p, "typing hides the placeholder");
    assert_eq!(w(&p, "tl").width, 18.0);

    let v = p.dom.version();
    let img = p.id("img");
    p.dom.set_natural_size(img, Some((4.0, 1.0)));
    assert_journaled(&p, v, "set_natural_size");
    assert_matches_fresh(&p, "image natural size");
    assert_eq!(w(&p, "img").height, 10.0);
}

#[test]
fn sibling_and_fragment_invalidation_stays_exact() {
    // Later siblings are restyled only when the changed element can match a
    // state-dependent compound left of `+`/`~` (before or after the change);
    // type-only compounds (`div + div`, TOS `[&>*+*]`) cannot change with
    // attributes. DocumentFragments and the parser's scratch documents (all
    // innerHTML writes) are invalidated by their own subtree only. Each step
    // compares every box with a fresh layout.
    let css = "<style>\
        .peer.on ~ .t{width:210px} .t{width:21px;height:10px}\
        [data-p=start] + [data-p=center]{width:220px} .c{width:22px;height:10px}\
        .g > div + div{width:230px} .g > div{width:23px;height:10px}\
        .ins .k{width:250px} .k{width:25px;height:10px}\
        </style>";
    let html = format!(
        "<!doctype html>{css}<body style=\"margin:0\">\
         <div><span id=pe class=\"peer on\"></span><i id=other></i><div id=t class=t></div></div>\
         <div><div id=ps data-p=start></div><div id=pc class=c data-p=center></div></div>\
         <div class=g><div id=g1></div><div id=g2></div></div>\
         <div id=host class=ins></div><div id=html></div>\
         </body>"
    );
    let mut p = page(&html);
    assert_matches_fresh(&p, "initial");
    let w = |p: &Page, id: &str| p.layout.layout_result(&p.dom).get(p.id(id)).unwrap().rect;
    assert_eq!(w(&p, "t").width, 210.0);

    // An unrelated attribute on a non-matching sibling: nothing to restyle.
    let other = p.id("other");
    p.dom.set_attribute(other, "data-z", "1").unwrap();
    assert_matches_fresh(&p, "unrelated attribute");
    // The peer loses its class: it matched only before the change.
    let pe = p.id("pe");
    p.dom.set_attribute(pe, "class", "").unwrap();
    assert_matches_fresh(&p, "class removed from the ~ left compound");
    assert_eq!(w(&p, "t").width, 21.0);
    p.dom.set_attribute(pe, "class", "peer on").unwrap();
    assert_matches_fresh(&p, "class added back");
    assert_eq!(w(&p, "t").width, 210.0);

    // The keyed attribute removed: matched only before.
    let ps = p.id("ps");
    assert_eq!(w(&p, "pc").width, 220.0);
    p.dom.remove_attribute(ps, "data-p").unwrap();
    assert_matches_fresh(&p, "keyed attribute removed");
    assert_eq!(w(&p, "pc").width, 22.0);
    p.dom.set_attribute(ps, "data-p", "start").unwrap();
    assert_matches_fresh(&p, "keyed attribute restored");
    assert_eq!(w(&p, "pc").width, 220.0);

    // Type-only sibling compounds: attributes cannot change them.
    let g1 = p.id("g1");
    p.dom.set_attribute(g1, "class", "anything").unwrap();
    assert_matches_fresh(&p, "attribute under div + div");
    assert_eq!(w(&p, "g2").width, 230.0);

    // A fragment built off-document, then inserted.
    let v = p.dom.version();
    let frag = p.dom.create_fragment(p.doc).unwrap();
    let k = p.dom.create_element(p.doc, "div").unwrap();
    p.dom.set_attribute(k, "class", "k").unwrap();
    p.dom.append_child(frag, k).unwrap();
    let host = p.id("host");
    p.dom.append_child(host, frag).unwrap();
    assert_journaled(&p, v, "fragment insert");
    assert_matches_fresh(&p, "fragment inserted");
    assert_eq!(p.layout.layout_result(&p.dom).get(k).unwrap().rect.width, 250.0);

    // innerHTML parses through a scratch document and merges text.
    let v = p.dom.version();
    let target = p.id("html");
    lp_dom::parse::set_inner_html(&mut p.dom, target, "a<b></b>b&amp;c<div class=k>x</div>").unwrap();
    assert_journaled(&p, v, "innerHTML");
    assert_matches_fresh(&p, "innerHTML");

    // Refocusing the focused element changes nothing.
    let doc = p.doc;
    p.dom.set_focused(doc, Some(g1)).unwrap();
    let v = p.dom.version();
    p.dom.set_focused(doc, Some(g1)).unwrap();
    assert_eq!(p.dom.version(), v, "refocus is a no-op");

    // :nth-child(of S): any element's class can move a later sibling's
    // index (its own page, since it makes every change reach siblings).
    let mut p = page(
        "<!doctype html><style>li{height:10px;width:10px} li:nth-child(2 of .x){width:240px}</style>\
         <body style=\"margin:0\"><ul><li id=x1 class=x></li><li id=n2></li><li id=x3></li></ul></body>",
    );
    assert_eq!(w(&p, "x3").width, 10.0);
    let n2 = p.id("n2");
    p.dom.set_attribute(n2, "class", "x").unwrap();
    assert_matches_fresh(&p, "nth-child of S");
    assert_eq!(w(&p, "n2").width, 240.0);
    // An earlier sibling leaves S: n2 becomes the first of S.
    let x1 = p.id("x1");
    p.dom.set_attribute(x1, "class", "").unwrap();
    assert_matches_fresh(&p, "nth-child of S, earlier sibling leaves S");
    assert_eq!(w(&p, "n2").width, 10.0);
}

#[test]
fn data_and_aria_attributes_restyle_only_through_selectors_naming_them() {
    // Style and layout never read data-*/aria-* attributes: only selectors
    // can observe them, so a change of one no selector names restyles
    // nothing, and one a selector names (any nesting, any case) still does.
    let css = "<style>\
        [data-state=open] .k{width:260px} .k{width:26px;height:10px}\
        .w:has([aria-Expanded=true]) .m{width:270px} .m{width:27px;height:10px}\
        </style>";
    let html = format!(
        "<!doctype html>{css}<body style=\"margin:0\">\
         <div id=a><div id=k class=k></div></div>\
         <div class=w><div><button id=b></button></div><div id=m class=m></div></div>\
         </body>"
    );
    let mut p = page(&html);
    assert_matches_fresh(&p, "initial");
    let w = |p: &Page, id: &str| p.layout.layout_result(&p.dom).get(p.id(id)).unwrap().rect;
    let (a, b) = (p.id("a"), p.id("b"));
    p.dom.set_attribute(a, "data-unrelated", "1").unwrap();
    p.dom.set_attribute(b, "aria-label", "x").unwrap();
    assert_matches_fresh(&p, "unreferenced data-/aria- attributes");
    p.dom.set_attribute(a, "data-state", "open").unwrap();
    assert_matches_fresh(&p, "referenced data- attribute");
    assert_eq!(w(&p, "k").width, 260.0);
    p.dom.set_attribute(b, "aria-expanded", "true").unwrap();
    assert_matches_fresh(&p, "aria- attribute named inside :has()");
    assert_eq!(w(&p, "m").width, 270.0);
    p.dom.remove_attribute(a, "data-state").unwrap();
    p.dom.set_attribute(b, "aria-expanded", "false").unwrap();
    assert_matches_fresh(&p, "both reverted");
    assert_eq!((w(&p, "k").width, w(&p, "m").width), (26.0, 27.0));
}

#[test]
fn incremental_style_survives_garbage_collected_removed_nodes() {
    // TOS cargo/documents (flow, 0d434cf): a removed subtree freed by
    // collect_garbage before the next style pass made invalidation_roots
    // dereference a dead slot ("live slot" panic). Its freed slot can also be
    // reused by a new node before the pass.
    let mut p = page("<body style='margin:0'><div id=a class=x><span id=s>one</span></div><div id=b>two</div></body>");
    p.dom.add_ref(p.doc).unwrap();
    let _ = p.metrics("a");
    let a = p.id("a");
    let s = p.id("s");
    p.dom.remove_child(a, s).unwrap();
    p.dom.collect_garbage();
    // Reuse the freed slot with a new node, then lay out again.
    let doc = p.doc;
    let fresh = p.dom.create_element(doc, "i").unwrap();
    let b = p.id("b");
    p.dom.append_child(b, fresh).unwrap();
    let fresh_layout = page("<body style='margin:0'><div id=a class=x></div><div id=b>two<i></i></div></body>");
    assert_eq!(p.metrics("a"), fresh_layout.metrics("a"));
    assert_eq!(p.metrics("b"), fresh_layout.metrics("b"));
}

#[test]
fn focus_and_hover_moves_invalidate_selectively_without_stale_geometry() {
    // A moved focus or hover restyles the elements whose :focus,
    // :focus-within or :hover match changed (the old and new elements, and
    // the ancestors only one of them has), their subtrees and, through
    // sibling combinators, later siblings; :has() anchors above them; and
    // the ancestors a focused/hovered element leaves or joins when its
    // subtree moves.
    // (Rules whose subject compound tests :focus/:hover never apply in this
    // engine; Tailwind's group-*/peer-* put them in other compounds.)
    let css = "<style>\
        .f{display:block;width:11px;height:10px} .f:focus > b{display:block;width:110px;height:10px}\
        .w:focus-within .k{width:120px} .k{width:12px;height:10px}\
        .peer:focus ~ .s{width:130px} .s{width:13px;height:10px}\
        .g:has(:focus) .h{width:140px} .h{width:14px;height:10px}\
        .p > i{display:block;height:20px} .p:focus > i{height:30px}\
        .grp:hover .x{width:150px} .x{width:15px;height:10px}\
        .hv > b{display:block;width:16px;height:10px} .hv:hover > b{width:160px}\
        </style>";
    let html = format!(
        "<!doctype html>{css}<body style=\"margin:0\">\
         <div class=w id=w1><div><div id=a class=f tabindex=0><b id=ab></b></div></div><div id=k1 class=k></div></div>\
         <div class=w id=w2><div id=slot></div><div id=k2 class=k></div></div>\
         <div><input id=pe class=\"peer f\"><div id=s class=s></div></div>\
         <div class=g><div><input id=gi class=f></div><div id=h class=h></div></div>\
         <div id=pp class=p tabindex=0><i id=pi></i><input id=pc></div>\
         <div class=grp id=grp><div id=x class=x></div><div id=hv class=hv><b id=hb></b></div></div>\
         <div id=away></div>\
         </body>"
    );
    let mut p = page(&html);
    assert_matches_fresh(&p, "initial");
    let w = |p: &Page, id: &str| p.layout.layout_result(&p.dom).get(p.id(id)).unwrap().rect;
    let doc = p.doc;
    let focus = |p: &mut Page, id: Option<&str>| {
        let el = id.map(|i| p.id(i));
        p.dom.set_focused(doc, el).unwrap();
    };

    focus(&mut p, Some("a"));
    assert_matches_fresh(&p, "focus a");
    assert_eq!((w(&p, "ab").width, w(&p, "k1").width, w(&p, "k2").width), (110.0, 120.0, 12.0));
    focus(&mut p, Some("pe"));
    assert_matches_fresh(&p, "focus moves to the peer");
    assert_eq!((w(&p, "ab").width, w(&p, "k1").width, w(&p, "s").width), (0.0, 12.0, 130.0));
    focus(&mut p, Some("gi"));
    assert_matches_fresh(&p, "focus into a :has(:focus) group");
    assert_eq!((w(&p, "s").width, w(&p, "h").width), (13.0, 140.0));
    // From a parent to its child: the parent loses :focus only.
    focus(&mut p, Some("pp"));
    assert_matches_fresh(&p, "focus the parent");
    assert_eq!(w(&p, "pi").height, 30.0);
    focus(&mut p, Some("pc"));
    assert_matches_fresh(&p, "focus its child");
    assert_eq!(w(&p, "pi").height, 20.0);

    // The focused element's subtree moves between :focus-within groups.
    focus(&mut p, Some("a"));
    assert_matches_fresh(&p, "focus a again");
    let a = p.id("a");
    let holder = p.dom.parent(a).unwrap().unwrap();
    let w1 = p.id("w1");
    let slot = p.id("slot");
    p.dom.remove_child(w1, holder).unwrap();
    // Removal is the focus fixup rule: the document has no focused element
    // until script focuses one again (Chrome; re-insertion restores nothing).
    assert_eq!(p.dom.focused(doc).unwrap(), None);
    p.dom.append_child(slot, holder).unwrap();
    assert_matches_fresh(&p, "focused subtree moved to w2, focus lost");
    assert_eq!((w(&p, "k1").width, w(&p, "k2").width), (12.0, 12.0));
    focus(&mut p, Some("a"));
    assert_matches_fresh(&p, "refocused inside w2");
    assert_eq!((w(&p, "k1").width, w(&p, "k2").width), (12.0, 120.0));
    focus(&mut p, None);
    assert_matches_fresh(&p, "focus cleared");
    assert_eq!(w(&p, "k2").width, 12.0);

    // Hover.
    let hv = p.id("hv");
    p.dom.set_hovered(doc, Some(hv)).unwrap();
    assert_matches_fresh(&p, "hover hv");
    assert_eq!((w(&p, "x").width, w(&p, "hb").width), (150.0, 160.0));
    let away = p.id("away");
    p.dom.set_hovered(doc, Some(away)).unwrap();
    assert_matches_fresh(&p, "hover leaves the group");
    assert_eq!((w(&p, "x").width, w(&p, "hb").width), (15.0, 16.0));
    p.dom.set_hovered(doc, Some(hv)).unwrap();
    p.dom.set_hovered(doc, None).unwrap();
    assert_matches_fresh(&p, "hover cleared");
}

#[test]
fn radio_group_changes_invalidate_selectively_without_stale_geometry() {
    // Regrouping (type/name) and removing radios restyle every radio in the
    // tree and what their validity restyles, instead of everything. (No
    // form-level validity rule here: it would root the whole form.)
    let css = "<style>\
        .r:invalid + .l{width:170px} .l{display:block;width:17px;height:10px}\
        </style>";
    let html = format!(
        "<!doctype html>{css}<body style=\"margin:0\">\
         <form id=f>\
         <div><input type=radio class=r name=g id=r1 required><span id=l1 class=l></span></div>\
         <div><input type=radio class=r name=g id=r2 checked><span id=l2 class=l></span></div>\
         <div><input type=radio class=r name=h id=r3 required><span id=l3 class=l></span></div>\
         </form><div id=out></div><div>FILLER</div></body>"
    );
    let mut p = page(&html);
    assert_matches_fresh(&p, "initial");
    let w = |p: &Page, id: &str| p.layout.layout_result(&p.dom).get(p.id(id)).unwrap().rect;
    assert_eq!((w(&p, "l1").width, w(&p, "l3").width), (17.0, 170.0));
    // r2 leaves group g by type: g has no checked radio any more.
    let r2 = p.id("r2");
    p.dom.set_attribute(r2, "type", "text").unwrap();
    assert_matches_fresh(&p, "checked radio stops being a radio");
    assert_eq!(w(&p, "l1").width, 170.0);
    p.dom.set_attribute(r2, "type", "radio").unwrap();
    assert_matches_fresh(&p, "and is a radio again");
    assert_eq!(w(&p, "l1").width, 17.0);
    // r2 moves to group h by name: h becomes valid, g invalid.
    p.dom.set_attribute(r2, "name", "h").unwrap();
    assert_matches_fresh(&p, "checked radio renamed to the other group");
    assert_eq!((w(&p, "l1").width, w(&p, "l3").width), (170.0, 17.0));
    p.dom.set_attribute(r2, "name", "g").unwrap();
    assert_matches_fresh(&p, "renamed back");
    // Removing the checked radio leaves g without one.
    let holder = p.dom.parent(r2).unwrap().unwrap();
    let f = p.id("f");
    p.dom.remove_child(f, holder).unwrap();
    assert_matches_fresh(&p, "checked radio removed");
    assert_eq!(w(&p, "l1").width, 170.0);
    let out = p.id("out");
    p.dom.append_child(out, holder).unwrap();
    assert_matches_fresh(&p, "inserted outside the form: another group");
    assert_eq!(w(&p, "l1").width, 170.0);
}

#[test]
fn insertions_and_removals_invalidate_selectively_without_stale_geometry() {
    // An insertion or removal restyles the inserted/removed subtree, later
    // siblings through any sibling combinator (adjacency changes even for
    // type-only compounds), the parent's other children through structural
    // pseudo-classes, the parent through :empty, and :has() ancestors; not
    // the parent's whole subtree (a portal appended to body).
    let css = "<style>\
        .list > *{width:18px;height:10px;display:block} .list > * + *{width:180px}\
        .list > :nth-child(2){height:20px} .list > :last-child{height:15px}\
        .e:empty{height:7px} .e{width:30px}\
        .q:has(> .y) .z{width:190px} .z{width:19px;height:10px}\
        .portal{width:200px;height:10px}\
        .adj > u{display:block;width:21px;height:10px} .adj > u + u{width:210px}\
        </style>";
    let html = format!(
        "<!doctype html>{css}<body style=\"margin:0\">\
         <div class=list id=list><i id=i1></i><i id=i2></i><i id=i3></i></div>\
         <div class=e id=e></div>\
         <div class=q id=q><div id=z class=z></div></div>\
         <div class=adj id=adj><u id=u1></u><u id=u2></u></div>\
         <div id=big>FILLER</div>\
         </body>"
    );
    let mut p = page(&html);
    assert_matches_fresh(&p, "initial");
    let w = |p: &Page, id: &str| p.layout.layout_result(&p.dom).get(p.id(id)).unwrap().rect;
    let el = |p: &mut Page, class: &str| {
        let e = p.dom.create_element(p.doc, "i").unwrap();
        p.dom.set_attribute(e, "class", class).unwrap();
        e
    };
    // At the front: every later child's adjacency and index moves.
    let list = p.id("list");
    let i1 = p.id("i1");
    let front = el(&mut p, "");
    p.dom.insert_before(list, front, Some(i1)).unwrap();
    assert_matches_fresh(&p, "insert at the front");
    assert_eq!((w(&p, "i1").width, w(&p, "i1").height), (180.0, 20.0));
    // At the end: the old last child stops being :last-child.
    let end = el(&mut p, "");
    p.dom.append_child(list, end).unwrap();
    assert_matches_fresh(&p, "append at the end");
    assert_eq!(w(&p, "i3").height, 10.0);
    // Removal in the middle.
    let i2 = p.id("i2");
    p.dom.remove_child(list, i2).unwrap();
    assert_matches_fresh(&p, "remove from the middle");
    p.dom.remove_child(list, front).unwrap();
    assert_matches_fresh(&p, "remove the first");
    assert_eq!((w(&p, "i1").width, w(&p, "i3").height), (18.0, 20.0));
    // :empty parent.
    let e = p.id("e");
    let child = el(&mut p, "");
    p.dom.append_child(e, child).unwrap();
    assert_matches_fresh(&p, "parent stops being :empty");
    p.dom.remove_child(e, child).unwrap();
    assert_matches_fresh(&p, "parent :empty again");
    assert_eq!(w(&p, "e").height, 7.0);
    // :has(> .y) anchor.
    let q = p.id("q");
    let y = el(&mut p, "y");
    p.dom.append_child(q, y).unwrap();
    assert_matches_fresh(&p, ":has(> .y) gains a match");
    assert_eq!(w(&p, "z").width, 190.0);
    p.dom.remove_child(q, y).unwrap();
    assert_matches_fresh(&p, ":has(> .y) loses it");
    assert_eq!(w(&p, "z").width, 19.0);
    // Adjacency only (no structural pseudo-classes on .adj): a non-matching
    // node between two u breaks `u + u`; removing it joins them again.
    let adj = p.id("adj");
    let u2 = p.id("u2");
    assert_eq!(w(&p, "u2").width, 210.0);
    let gap = p.dom.create_element(p.doc, "s").unwrap();
    p.dom.insert_before(adj, gap, Some(u2)).unwrap();
    assert_matches_fresh(&p, "a non-u between two u");
    assert_eq!(w(&p, "u2").width, 21.0);
    p.dom.remove_child(adj, gap).unwrap();
    assert_matches_fresh(&p, "the gap removed");
    assert_eq!(w(&p, "u2").width, 210.0);
    let u0 = p.dom.create_element(p.doc, "u").unwrap();
    let u1 = p.id("u1");
    p.dom.insert_before(adj, u0, Some(u1)).unwrap();
    assert_matches_fresh(&p, "a u at the front");
    assert_eq!(w(&p, "u1").width, 210.0);

    // Portals appended to and removed from body.
    let body = p.dom.parent(q).unwrap().unwrap();
    let portal = p.dom.create_element(p.doc, "div").unwrap();
    p.dom.set_attribute(portal, "class", "portal").unwrap();
    let inner = el(&mut p, "y");
    p.dom.append_child(portal, inner).unwrap();
    p.dom.append_child(body, portal).unwrap();
    assert_matches_fresh(&p, "portal appended to body");
    assert_eq!(p.layout.layout_result(&p.dom).get(portal).unwrap().rect.width, 200.0);
    p.dom.remove_child(body, portal).unwrap();
    assert_matches_fresh(&p, "portal removed");
}

#[test]
fn has_anchor_context_bounds_universal_anchors_without_stale_geometry() {
    // TOS: `[data-is-empty=true] :has(>[data-static-segment-type])>...`
    // anchors are universal but must sit below a [data-is-empty] ancestor,
    // so changes near html/body (portals) do not root html. Changes below a
    // real anchor still restyle it.
    let css = "<style>\
        [data-is-empty=true] :has(>[data-static-segment-type])>.text-inherit{width:280px}\
        [data-is-empty=true] :has(>div>[data-static-segment-type])>span.pe{width:290px}\
        .text-inherit,.pe{display:block;width:28px;height:10px}\
        </style>";
    let html = format!(
        "<!doctype html>{css}<body style=\"margin:0\">\
         <div id=ctx data-is-empty=true><section>\
         <div id=anc><span id=ti class=text-inherit></span></div>\
         <div id=anc2><div id=mid></div><span id=pe class=pe></span></div>\
         </section></div><div>FILLER</div></body>"
    );
    let mut p = page(&html);
    assert_matches_fresh(&p, "initial");
    let w = |p: &Page, id: &str| p.layout.layout_result(&p.dom).get(p.id(id)).unwrap().rect;
    let seg = |p: &mut Page| {
        let e = p.dom.create_element(p.doc, "i").unwrap();
        p.dom.set_attribute(e, "data-static-segment-type", "day").unwrap();
        e
    };
    let anc = p.id("anc");
    let s1 = seg(&mut p);
    p.dom.append_child(anc, s1).unwrap();
    assert_matches_fresh(&p, "segment child inserted (depth 1)");
    assert_eq!(w(&p, "ti").width, 280.0);
    p.dom.remove_child(anc, s1).unwrap();
    assert_matches_fresh(&p, "segment child removed");
    assert_eq!(w(&p, "ti").width, 28.0);
    let mid = p.id("mid");
    let s2 = seg(&mut p);
    p.dom.append_child(mid, s2).unwrap();
    assert_matches_fresh(&p, "segment grandchild inserted (depth 2)");
    assert_eq!(w(&p, "pe").width, 290.0);
    p.dom.remove_attribute(s2, "data-static-segment-type").unwrap();
    assert_matches_fresh(&p, "segment attribute removed");
    assert_eq!(w(&p, "pe").width, 28.0);
    p.dom.set_attribute(s2, "data-static-segment-type", "x").unwrap();
    let ctx = p.id("ctx");
    p.dom.set_attribute(ctx, "data-is-empty", "false").unwrap();
    assert_matches_fresh(&p, "context attribute changed");
    assert_eq!(w(&p, "pe").width, 28.0);
    p.dom.set_attribute(ctx, "data-is-empty", "true").unwrap();
    assert_matches_fresh(&p, "context restored");
    assert_eq!(w(&p, "pe").width, 290.0);
    // A portal with a segment child on body: body is no anchor here.
    let body = p.dom.parent(ctx).unwrap().unwrap();
    let portal = p.dom.create_element(p.doc, "div").unwrap();
    let s3 = seg(&mut p);
    p.dom.append_child(portal, s3).unwrap();
    p.dom.append_child(body, portal).unwrap();
    assert_matches_fresh(&p, "portal appended to body");
    p.dom.remove_child(body, portal).unwrap();
    assert_matches_fresh(&p, "portal removed");
}

#[test]
fn flow_default_ignorable_characters_take_no_width() {
    // TOS date picker (react-aria DateSegments): Intl wraps literals in bidi
    // isolates, e.g. "01:00\u{2069} PM". Chromium gives U+2066..U+2069 and the
    // other default-ignorable format characters no advance; flow measured
    // them with the 0.6em non-ASCII fallback (8.4px at 14px), pushing the
    // Calendar button 3.74px past its cell. Adjacent text keeps its width.
    let css = "<style>body{margin:0;font:500 14px system-ui} .row{display:flex;white-space:nowrap} .row span{flex:none}</style>";
    let html = format!(
        "<!doctype html>{css}<body>\
         <div class=row><span id=plain>01:00 PM</span></div>\
         <div class=row><span id=pdi>01:00\u{2069} PM</span></div>\
         <div class=row><span id=iso>\u{2068}01:00\u{2069}\u{a0}\u{2066}PM\u{2069}</span></div>\
         <div class=row><span id=nbsp>01:00\u{a0}PM</span></div>\
         <div class=row><span id=zw>01\u{200b}:\u{200e}00\u{feff} \u{ad}PM</span></div>\
         <div class=row><span id=x>x</span><span id=after>\u{2069}y</span><span id=y>y</span></div>\
         </body>"
    );
    let p = page(&html);
    let wid = |id: &str| p.metrics(id)[2];
    let plain = wid("plain");
    assert!(plain > 50.0, "{plain}");
    assert_eq!(wid("pdi"), plain, "U+2069 adds nothing");
    assert_eq!(wid("iso"), wid("nbsp"), "isolates around both segments add nothing");
    assert_eq!(wid("zw"), plain, "ZWSP, LRM, BOM and soft hyphen add nothing");
    assert_eq!(wid("after"), wid("y"), "an isolate before normal text");
}

#[test]
fn a_column_flex_container_is_as_tall_as_its_hypothetical_items() {
    // Chromium (800x600): an auto-height column container is the sum of its
    // items' flex base sizes clamped by their minimum and maximum. `height`
    // only counts through the flex basis (auto), a `0px` basis gives the
    // item's minimum, and a percentage basis of the indefinite container is
    // `content`.
    let html = "<!doctype html><body style=\"margin:0\">\
        <div id=c1 style=\"display:flex;flex-direction:column\"><div style=\"flex:1 1 0%;min-height:0;height:10px\"></div></div>\
        <div id=c2 style=\"display:flex;flex-direction:column\"><div style=\"flex:1 1 0px;min-height:0;height:10px\"></div></div>\
        <div id=c3 style=\"display:flex;flex-direction:column\"><div style=\"flex:0 0 0px;min-height:0;height:10px\"></div></div>\
        <div id=c4 style=\"display:flex;flex-direction:column\"><div style=\"flex:1 1 auto;height:10px\"></div></div>\
        <div id=c5 style=\"display:flex;flex-direction:column\"><div id=j5 style=\"flex:1 1 0%;height:10px\"><div style=\"height:30px\"></div></div></div>\
        <div id=c6 style=\"display:flex;flex-direction:column\"><div id=j6 style=\"flex:1 1 0px;height:10px\"><div style=\"height:30px\"></div></div></div>\
        <div id=c7 style=\"display:flex;flex-direction:column\"><div id=j7 style=\"flex:1 1 20%;height:50px;min-height:0\"><div style=\"height:30px\"></div></div></div>\
        <div id=c8 style=\"display:flex;flex-direction:column\"><div id=j8 style=\"flex:1 1 0px;min-height:0;height:10px\"></div><div id=k8 style=\"height:20px\"></div></div>\
        </body>";
    check_rects(
        "column-hypothetical",
        html,
        &[
            ("c1", [0.0, 0.0, 800.0, 0.0]),
            ("c2", [0.0, 0.0, 800.0, 0.0]),
            ("c3", [0.0, 0.0, 800.0, 0.0]),
            ("c4", [0.0, 0.0, 800.0, 10.0]),
            ("c5", [0.0, 10.0, 800.0, 30.0]),
            ("j5", [0.0, 10.0, 800.0, 30.0]),
            ("c6", [0.0, 40.0, 800.0, 10.0]),
            ("j6", [0.0, 40.0, 800.0, 10.0]),
            ("c7", [0.0, 50.0, 800.0, 30.0]),
            ("j7", [0.0, 50.0, 800.0, 30.0]),
            ("c8", [0.0, 80.0, 800.0, 20.0]),
            ("j8", [0.0, 80.0, 800.0, 0.0]),
            ("k8", [0.0, 80.0, 800.0, 20.0]),
        ],
    );
}

#[test]
fn flow_percentage_flex_basis_in_an_indefinite_column_is_content() {
    // TOS business-partner-invoices drawer: a `flex: 1 1 0%; min-height: 0`
    // column item inside an auto-height `overflow: hidden` column flex
    // container (itself in a block). The container's main size is
    // indefinite, so the percentage basis is `content` (css-flexbox §7.2.3):
    // Chromium sizes the item and the container to the 520px content, and a
    // click inside lands on the content. Flow used a 0px basis, collapsed the
    // container and clipped the table header out of hit testing. Against a
    // definite height the 0% basis still resolves to 0 and grows. Values
    // measured in Chromium (Playwright 1.59.1, 800x600).
    let p = page(
        "<!doctype html><body style=\"margin:0\">\
         <div style=\"display:block\"><div id=c1 style=\"display:flex;flex-direction:column;overflow:hidden\">\
         <div id=i1 style=\"display:flex;flex-direction:column;flex:1 1 0%;min-height:0\"><div id=k1 style=\"min-height:520px\"></div></div></div></div>\
         <div id=c2 style=\"display:flex;flex-direction:column;height:300px;overflow:hidden\">\
         <div id=i2 style=\"display:flex;flex-direction:column;flex:1 1 0%;min-height:0\"><div style=\"min-height:520px\"></div></div></div>\
         <div id=c3 style=\"display:flex;flex-direction:column\"><div id=i3 style=\"flex:1 1 50%;min-height:0\"><div style=\"height:40px\"></div></div>\
         <div id=j3 style=\"flex:1;min-height:0;height:10px\"></div></div>\
         <div style=\"display:flex;width:400px\"><div id=i4 style=\"flex:1 1 0%;min-width:0\"><div style=\"width:300px;height:5px\"></div></div><div id=j4 style=\"flex:1 1 0%\"></div></div>\
         </body>"
    );
    let m = |id: &str| {
        let m = p.metrics(id);
        (m[1], m[2], m[3])
    };
    assert_eq!(m("c1"), (0.0, 800.0, 520.0));
    assert_eq!(m("i1"), (0.0, 800.0, 520.0));
    assert_eq!(p.layout.element_from_point(&p.dom, 10.0, 400.0), Some(p.id("k1")), "Chromium hits #k1");
    assert_eq!(m("c2"), (520.0, 800.0, 300.0), "definite height: 0% resolves and grows");
    assert_eq!(m("i2"), (520.0, 800.0, 300.0));
    assert_eq!(m("c3"), (820.0, 800.0, 40.0));
    assert_eq!(m("i3"), (820.0, 800.0, 40.0), "50% of indefinite: content");
    assert_eq!(m("j3"), (860.0, 800.0, 0.0), "content basis ignores height:10px");
    assert_eq!((m("i4").1, m("j4").1), (200.0, 200.0), "definite row width: 0% resolves");
}

#[test]
fn batched_changes_reach_depth_bounded_anchors_at_their_lowest_level() {
    // One invalidation batch walks the ancestors of many changes and stops a
    // walk at an ancestor already reached at the same or a lower level. A
    // deep change first reaches the `:has(> .y)` anchor at level 2 (beyond
    // its depth 1); a later change in the same batch reaches it at level 0
    // and must still restyle it.
    let css = "<style>\
        .q:has(> .y) .z{width:290px} .z{width:29px;height:10px}\
        .w:has(> .y) .v{width:300px} .v{width:30px;height:10px}\
        </style>";
    let html = format!(
        "<!doctype html>{css}<body style=\"margin:0\">\
         <div id=q class=q><div id=a><div id=b></div></div><div id=z class=z></div></div>\
         <div id=w class=w><div id=c><div id=d></div></div><div id=v class=v></div></div>\
         <div>FILLER</div></body>"
    );
    let mut p = page(&html);
    assert_matches_fresh(&p, "initial");
    let w = |p: &Page, id: &str| p.layout.layout_result(&p.dom).get(p.id(id)).unwrap().rect;
    let el = |p: &mut Page, class: &str| {
        let e = p.dom.create_element(p.doc, "i").unwrap();
        p.dom.set_attribute(e, "class", class).unwrap();
        e
    };
    // Deep change first, then the shallow one, with no flush in between.
    let (b, q) = (p.id("b"), p.id("q"));
    let deep = el(&mut p, "x");
    p.dom.append_child(b, deep).unwrap();
    let y = el(&mut p, "y");
    p.dom.append_child(q, y).unwrap();
    assert_matches_fresh(&p, "deep then shallow in one batch");
    assert_eq!(w(&p, "z").width, 290.0);
    // Shallow first, then deep: the deep walk stops at the anchor.
    let (d, ww) = (p.id("d"), p.id("w"));
    let y2 = el(&mut p, "y");
    p.dom.append_child(ww, y2).unwrap();
    let deep2 = el(&mut p, "x");
    p.dom.append_child(d, deep2).unwrap();
    assert_matches_fresh(&p, "shallow then deep in one batch");
    assert_eq!(w(&p, "v").width, 300.0);
}

#[test]
fn flow_overflow_wrap_breaks_long_words_like_chromium() {
    // TOS CMR cargo cells (TP-4194/4195): `wrap-anywhere` (overflow-wrap:
    // anywhere) on "ReinforcedIndustrialTransportPallet …" in a 120px cell.
    // Chromium (Playwright 1.59.1, 14px system-ui), measured on this markup:
    // - 120px block: the word breaks over two lines for anywhere /
    //   break-word / break-all (3 lines with "6x"); normal overflows (2).
    // - 120px table cell: anywhere / break-all keep the cell at 120px and the
    //   text inside it. (break-word / normal grow the column to the word's
    //   min-content; flow tables do not grow declared columns, not tested.)
    // - shrink-to-fit inline-block in 60px: anywhere / break-all are 60px
    //   wide (character min-content); break-word / normal 227.3px (the word).
    let word = "ReinforcedIndustrialTransportPallet";
    let modes = [
        ("anywhere", "overflow-wrap:anywhere"),
        ("breakword", "overflow-wrap:break-word"),
        ("breakall", "word-break:break-all"),
        ("normal", ""),
    ];
    let mut html = String::from("<!doctype html><body style=\"margin:0;font:400 14px system-ui\">");
    for (k, css) in modes {
        html.push_str(&format!("<div id=b-{k} style=\"width:120px;{css}\">{word} 6x</div>"));
        html.push_str(&format!(
            "<table style=\"border-collapse:collapse\"><tr><td id=t-{k} style=\"width:120px;padding:0;{css}\"><span id=s-{k}>{word} 6x</span></td></tr></table>"
        ));
        html.push_str(&format!(
            "<div style=\"width:60px\"><div id=f-{k} style=\"display:inline-block;{css}\">{word}</div></div>"
        ));
    }
    let p = page(&html);
    let line = p.metrics("b-normal")[3] / 2.0;
    for k in ["anywhere", "breakword", "breakall"] {
        assert_eq!(p.metrics(&format!("b-{k}"))[3], 3.0 * line, "{k}: the word breaks over two lines");
    }
    assert_eq!(p.metrics("b-normal")[3], 2.0 * line, "normal: the word overflows its line");
    for k in ["anywhere", "breakall"] {
        let cell = p.metrics(&format!("t-{k}"));
        let text = p.metrics(&format!("s-{k}"));
        assert_eq!(cell[2], 120.0, "{k}: the cell keeps its width");
        assert!(
            text[0] >= cell[0] - 1.0 && text[0] + text[2] <= cell[0] + cell[2] + 1.0,
            "{k}: text {text:?} inside cell {cell:?}"
        );
        assert_eq!(p.metrics(&format!("f-{k}"))[2], 60.0, "{k}: character min-content");
    }
    for k in ["breakword", "normal"] {
        let w = p.metrics(&format!("f-{k}"))[2];
        // The word's whole width (Chromium 227.3px; flow's text measure
        // differs for this word, which is not what this test covers).
        assert!(w > 200.0, "{k}: the word's width is the min-content ({w})");
    }
}

#[test]
fn flow_whitespace_normal_span_wraps_inside_nowrap_cell() {
    // TOS FE CMR cargo editor table (TP-4194/4195): the cell is `whitespace-
    // nowrap overflow-hidden text-ellipsis`, the value a Typography span with
    // `whitespace-normal wrap-anywhere`. Soft wraps follow the white-space of
    // the element containing the text, so the span wraps inside the cell.
    // Chromium (Playwright 1.59.1, 500 18px system-ui), on this markup:
    // - pkg (200px cell): 2 lines, span 191.4px wide (word broken anywhere,
    //   "3ddea465" continues the second line), inside the 192px content box.
    // - hs (170px cell): 3 lines, one HS code each, span 115.4px wide.
    // - inherit (span without white-space): one 347.6px line, clipped.
    let td =
        "white-space:nowrap;overflow:hidden;text-overflow:ellipsis;padding:2px 4px;line-height:1.25;position:relative";
    let sp = "font:500 18px system-ui;white-space:normal;overflow-wrap:anywhere";
    let cases = [
        ("pkg", 200, "ReinforcedIndustrialTransportPallet 3ddea465", sp),
        ("hs", 170, "9230602501, 9230603302, 9230603903", sp),
        ("inherit", 170, "9230602501, 9230603302, 9230603903", "font:500 18px system-ui"),
    ];
    let mut html = String::from("<!doctype html><body style=\"margin:0;font:400 14px system-ui\">");
    for (k, w, text, s) in cases {
        html.push_str(&format!(
            "<table style=\"table-layout:fixed;width:600px;border-collapse:collapse\"><tr><td id=c-{k} style=\"{td};width:{}px\"><span id=s-{k} style=\"{s}\">{text}</span></td><td>x</td></tr></table>",
            w - 8
        ));
    }
    let p = page(&html);
    // One line is the single-line span's height; each further line adds the
    // line pitch (the step from 2 to 3 lines).
    // The spans' `font` shorthand applies (500 18px): `line-height` is
    // `normal`, 21px (rounded ascent 17 + descent 4), as Chromium has it.
    let one = p.metrics("s-inherit")[3];
    let pkg_lines = 2.0;
    let pitch = (p.metrics("s-hs")[3] - one) / 2.0;
    assert!(pitch >= one, "line pitch {pitch} vs line {one}");
    for (k, lines) in [("pkg", pkg_lines), ("hs", 3.0)] {
        let cell = p.metrics(&format!("c-{k}"));
        let text = p.metrics(&format!("s-{k}"));
        assert_eq!(text[3], one + (lines - 1.0) * pitch, "{k}: {lines} lines, span {text:?}");
        assert!(
            text[0] >= cell[0] + 4.0 - 0.5 && text[0] + text[2] <= cell[0] + cell[2] - 4.0 + 0.5,
            "{k}: span {text:?} inside the content box of cell {cell:?}"
        );
    }
    let cell = p.metrics("c-inherit");
    assert!(p.metrics("s-inherit")[2] > cell[2], "inherit: the nowrap line overflows the cell");
}

#[test]
fn flow_max_content_keeps_whitespace_between_inline_pieces() {
    // TOS quote document preview (pdfgen PageCounter): the page counter is
    // `<p class="absolute bottom-0 right-0">Page <span>1</span>/<span>1</span></p>`
    // under the PREVIEW corner label. Its shrink-to-fit width dropped the
    // space before <span> (max-content summed trimmed text nodes), so the
    // counter wrapped to two lines and rose 18px into the label (TOS
    // quotes-document-preview-spacing: gap -11px).
    // Chromium (Playwright 1.59.1, 12px system-ui, line-height 1.5), on this
    // markup: every abs-pos <p> is one 18px line exactly as wide as the same
    // content inline (plain 46.34, spans 46.38, span1 37.14, lead 37.14,
    // blank 37.14, nospace 42.97).
    let vs = [
        ("plain", "Page 1/1"),
        ("spans", "Page <span>1</span>/<span>1</span>"),
        ("nospace", "Page1/1"),
        ("span1", "Page <span>1</span>"),
        ("lead", "<span>Page</span> 1"),
        ("blank", "<span>Page</span> <span>1</span>"),
    ];
    let mut html = String::from(
        "<!doctype html><style>*{margin:0;padding:0}html{line-height:1.5;font-family:system-ui}</style><body>",
    );
    for (k, t) in vs {
        html.push_str(&format!(
            "<div style=\"position:relative;width:600px;height:40px\"><p id={k} style=\"position:absolute;right:0;font-size:12px\">{t}</p></div>"
        ));
    }
    for (k, t) in vs {
        html.push_str(&format!("<div style=\"width:600px\"><span id=i-{k} style=\"font-size:12px\">{t}</span></div>"));
    }
    // The TOS header (W3 capture, 0267767): the counter sits at the bottom
    // of the title block on one line.
    html.push_str(
        "<div style=\"width:794px\"><div style=\"position:relative;display:flex;flex-direction:column;gap:16px;padding:32px 0 16px\">\
         <div id=label style=\"position:absolute;top:4px;right:4px;font-size:10pt;font-weight:600;letter-spacing:.025em\">PREVIEW</div>\
         <div id=title style=\"position:relative;margin:0 68px\"><h1 style=\"font-size:18px;font-weight:700;line-height:17px\">Quotation - QE2ESE260000001 V.1</h1>\
         <p id=counter style=\"position:absolute;bottom:0;right:0;font-size:12px\">Page <span class=pageNumber>1</span>/<span class=totalPages>1</span></p></div></div></div>",
    );
    let p = page(&html);
    for (k, _) in vs {
        let abs = p.metrics(k);
        assert_eq!(abs[2], p.metrics(&format!("i-{k}"))[2], "{k}: shrink-to-fit width is the inline width");
        assert_eq!(abs[3], 18.0, "{k}: one line");
    }
    let (title, counter) = (p.metrics("title"), p.metrics("counter"));
    assert_eq!(counter[3], 18.0, "counter on one line: {counter:?}");
    assert_eq!(counter[1] + counter[3], title[1] + title[3], "counter at the title block's bottom");
    let label = p.metrics("label");
    assert!(counter[1] > label[1] + label[3], "counter {counter:?} below label {label:?}");
}

#[test]
fn journal_overflow_keeps_recent_changes_covered() {
    // The DOM journal used to be cleared outright when full, so the next
    // style flush after every 8192 changes (mid React commit, whatever the
    // consumer had read) restyled the whole document: TOS Positions rows
    // rebuilt off-document journal ~4.5k changes, and the commit pass cost a
    // cold cascade. On overflow it now drops only the older half.
    let mut p = page("<!doctype html><body><div id=a></div></body>");
    let a = p.id("a");
    let cap = lp_dom::dom::JOURNAL_CAPACITY;
    let old = p.dom.version();
    for i in 0..cap - 100 {
        p.dom.set_attribute(a, "data-n", &i.to_string()).unwrap();
    }
    let recent = p.dom.version();
    for i in 0..300 {
        p.dom.set_attribute(a, "data-m", &i.to_string()).unwrap();
    }
    let since: Vec<_> = p.dom.changes_since(recent).expect("recent version still covered").collect();
    assert_eq!(since.len(), 300, "exactly the changes after it");
    assert!(p.dom.changes_since(old).is_none(), "versions before the kept half are not covered");
}

#[test]
fn large_react_like_batches_invalidate_without_stale_geometry() {
    // A list filled child by child between two flushes (TOS Positions rows
    // rebuilt off-document, then inserted): more changes than the old
    // journal capacity, many per parent (examined once per batch), with
    // :nth-child, :empty, :has() and sibling rules depending on the result.
    let css = "<style>\
        li{display:block;height:4px;width:10px} li:nth-child(3){width:33px} li:last-child{width:44px}\
        ul:empty{height:7px} ul{width:200px}\
        .box:has(> ul > li.hot){width:300px} .box{width:100px}\
        li.hot + li{width:55px}\
        </style>";
    let html = format!(
        "<!doctype html>{css}<body style=\"margin:0\"><div id=box class=box><ul id=ul></ul></div><ul id=empty></ul></body>"
    );
    let mut p = page(&html);
    assert_matches_fresh(&p, "initial");
    let (ul, empty) = (p.id("ul"), p.id("empty"));
    for round in 0..2 {
        // Rows built detached: element, attributes, nested children, text.
        for i in 0..1200 {
            let li = p.dom.create_element(p.doc, "li").unwrap();
            for k in 0..4 {
                p.dom.set_attribute(li, &format!("data-k{k}"), &i.to_string()).unwrap();
            }
            p.dom.set_attribute(li, "class", if i == 600 + round { "hot" } else { "row" }).unwrap();
            let span = p.dom.create_element(p.doc, "span").unwrap();
            let t = p.dom.create_text(p.doc, "x").unwrap();
            p.dom.append_child(span, t).unwrap();
            p.dom.append_child(li, span).unwrap();
            p.dom.append_child(ul, li).unwrap();
        }
        // The same parent also loses and regains children in the batch.
        let first = p.dom.children(ul).unwrap().next().unwrap();
        p.dom.remove_child(ul, first).unwrap();
        p.dom.insert_before(ul, first, None).unwrap();
        let li = p.dom.create_element(p.doc, "li").unwrap();
        p.dom.append_child(empty, li).unwrap();
        assert_matches_fresh(&p, &format!("round {round}: filled"));
        p.dom.remove_child(empty, li).unwrap();
        let kids: Vec<_> = p.dom.children(ul).unwrap().collect();
        for k in kids.iter().take(kids.len() / 2) {
            p.dom.remove_child(ul, *k).unwrap();
        }
        assert_matches_fresh(&p, &format!("round {round}: half removed"));
    }
}

#[test]
fn flex_wrap_compressible_controls_shrink_into_the_line() {
    // TOS calendar filters (calendar-filters.e2e.ts L96): the multi-select
    // Employee field holds its chips and a `w-full flex-1` search input in a
    // `flex flex-wrap` row; the input took its intrinsic width as automatic
    // minimum, wrapped below the chips, and the field grew from 28px.
    // Chromium (Playwright 1.59.1), two 120px chips then the item in a 300px
    // `flex-wrap` row with 8px gaps: text inputs, textareas and selects with
    // a percentage width shrink to the 44px left (CSS Sizing 3 §5.2.2,
    // compressible); auto, px or max-width-only inputs, buttons and
    // percentage-width divs keep their minimum and wrap.
    let shrink = [
        ("inputPct", "<input style=\"width:100%;flex:1\">"),
        ("inputPct50", "<input style=\"width:50%;flex:1\">"),
        ("textareaPct", "<textarea style=\"width:100%;flex:1\"></textarea>"),
        ("selectPct", "<select style=\"width:100%;flex:1\"><option>An option label here</option></select>"),
    ];
    let wrap = [
        ("inputAuto", "<input style=\"flex:1\">"),
        ("inputMaxPct", "<input style=\"max-width:100%;flex:1\">"),
        ("inputPx", "<input style=\"width:200px;flex:1\">"),
        ("buttonPct", "<button style=\"width:100%;flex:1\">Button label text</button>"),
        ("divPct", "<div style=\"width:100%;flex:1\">LongUnbreakableWordInsideTheDivXXXXXXXX</div>"),
    ];
    let mut html = String::from(
        "<!doctype html><style>*{box-sizing:border-box;margin:0;padding:0;border:0 solid}input,button,select,textarea{font:inherit}\
         .w{display:flex;flex-wrap:wrap;gap:8px;width:300px;font:15px system-ui}.c{width:120px;height:20px}</style><body>",
    );
    for (k, item) in shrink.iter().chain(wrap.iter()) {
        // `id=i-<key>` right after the tag name.
        let at = item.find(' ').unwrap();
        let item = format!("{} id=i-{k}{}", &item[..at], &item[at..]);
        html.push_str(&format!("<div class=w id={k}><div class=c></div><div class=c></div>{item}</div>"));
    }
    // The TOS field (compiled CSS, @povio/ui 5.0.0-rc.10 SelectInput): one
    // 28px line with two chips and the search input, like a single select.
    html.push_str(
        "<style>.field{position:relative;display:flex;width:400px;overflow:hidden;white-space:nowrap;border:1px solid;padding-inline:4px}\
         .row{position:relative;display:flex;flex:1;align-items:center;gap:4px;overflow:hidden;white-space:nowrap;padding-block:1px;font-size:15px;line-height:1.6}\
         .tags{display:flex;flex:1;flex-wrap:wrap;gap:8px;overflow:hidden;white-space:nowrap}\
         .tag{display:inline-flex;align-items:center;gap:2px;height:20px;align-self:center;padding:0 2px 0 4px;overflow:hidden}\
         .tag>span{line-height:20px}.tag>button{display:grid;width:16px;height:16px}</style>\
         <div id=single class=field><div class=row><label style=\"font-size:13px;font-weight:700\">Routes</label><button style=\"width:100%\">All</button></div></div>\
         <div id=multi class=field><div class=row><label style=\"font-size:13px;font-weight:700\">Employee</label><div class=tags>\
         <div class=tag><span>FE E2E</span><button></button></div><div class=tag><span>Integration Only</span><button></button></div>\
         <input id=search style=\"width:100%;flex:1\"></div></div></div>",
    );
    let p = page(&html);
    for (k, _) in shrink {
        let (w, item) = (p.metrics(k), p.metrics(&format!("i-{k}")));
        assert_eq!(item[1], w[1], "{k}: on the first line");
        assert!((item[2] - 44.0).abs() < 0.5, "{k}: shrinks to the 44px left ({item:?})");
    }
    for (k, _) in wrap {
        let (w, item) = (p.metrics(k), p.metrics(&format!("i-{k}")));
        assert!(item[1] > w[1] + 19.0, "{k}: wraps to a second line ({item:?})");
    }
    assert_eq!(p.metrics("multi")[3], p.metrics("single")[3], "multi-select as tall as a single select");
    assert_eq!(p.metrics("single")[3], 28.0);
    assert_eq!(p.metrics("search")[1], p.metrics("multi")[1] + 2.0, "search input on the chips' line");
}

#[test]
fn inline_custom_properties_resolve_through_var() {
    // An element's inline style is parsed for a custom property only when
    // it can declare it (the name, or an escape, is in its text): every
    // form of declaration must still resolve, inherited by descendants.
    let p = page(
        "<!doctype html><style>:root{--w:5px} .v{display:block;height:1px;width:var(--w)}</style><body style='margin:0'>\
         <div style='--w:11px'><i id=plain class=v></i></div>\
         <div style='color:red; --w : 12px !important ; height:2px'><i id=spaced class=v></i></div>\
         <div style='--w:13px'><div style='color:blue'><i id=inherited class=v></i></div></div>\
         <div style='height:3px;--wide:99px'><i id=prefix class=v></i></div>\
         <div style='z-index:1'><i id=other class=v></i></div>\
         <i id=own class=v style='--w:14px'></i>\
         <div style='--\\77 :15px'><i id=escaped class=v></i></div>\
         </body>"
    );
    let w = |id: &str| p.metrics(id)[2];
    assert_eq!(w("plain"), 11.0);
    assert_eq!(w("spaced"), 12.0);
    assert_eq!(w("inherited"), 13.0);
    assert_eq!(w("prefix"), 5.0, "--wide does not declare --w");
    assert_eq!(w("other"), 5.0);
    assert_eq!(w("own"), 14.0);
    assert_eq!(w("escaped"), 15.0, "an escape can spell the name");
}

#[test]
fn scroll_into_view_if_needed_centers_only_when_needed_like_chromium() {
    // CDP DOM.scrollIntoViewIfNeeded, measured in Chromium (800x600, body
    // margin 0): a fully visible box, or one covering the scrollport, does not
    // scroll; a hidden box is centered; a partly visible one comes in by its
    // nearest edge. Each overflow ancestor first, then the window. Playwright
    // sends it before every click, and react-aria closes its popovers on any
    // scroll of the trigger's ancestors, so a visible node must not move.
    // (name, inside the 200px scroller at y=100, offset, height, pre-scroll, [window, scroller])
    type Case = (&'static str, bool, u32, u32, i32, [u32; 2]);
    let cases: [Case; 11] = [
        ("visible", false, 100, 50, 0, [0, 0]),
        ("hidden-below", false, 1000, 50, 0, [725, 0]),
        ("partial-bottom", false, 580, 50, 0, [30, 0]),
        ("hidden-above", false, 300, 50, 1000, [25, 0]),
        ("partial-top", false, 480, 50, 500, [480, 0]),
        ("tall-hidden", false, 1000, 900, 0, [1150, 0]),
        ("covers", false, 0, 3000, 100, [100, 0]),
        ("box-visible", true, 50, 40, 0, [0, 0]),
        ("box-hidden-below", true, 500, 40, 0, [0, 420]),
        ("box-partial", true, 180, 40, 0, [0, 20]),
        ("box-hidden-above", true, 400, 40, 1000, [0, 320]),
    ];
    for (name, boxed, offset, height, pre, [window, scroller]) in cases {
        let target = format!("<div id=t style=\"height:{height}px;width:100px\"></div>");
        let html = if boxed {
            format!("<!doctype html><body style=\"margin:0\"><div style=\"height:100px\"></div><div id=s style=\"height:200px;overflow:auto\"><div style=\"height:{offset}px\"></div>{target}<div style=\"height:2000px\"></div></div><div style=\"height:3000px\"></div></body>")
        } else {
            format!("<!doctype html><body style=\"margin:0\"><div id=s></div><div style=\"height:{offset}px\"></div>{target}<div style=\"height:5000px\"></div></body>")
        };
        let mut p = page(&html);
        let (s, t) = (p.id("s"), p.id("t"));
        if boxed {
            p.layout.element_scroll_to(&p.dom, s, None, Some(pre));
        } else {
            p.layout.window_scroll_to(&p.dom, None, Some(pre));
        }
        let before = (p.layout.scroll_y(), p.layout.scroll_top(s));
        let moved = p.layout.scroll_into_view_if_needed_targets(&p.dom, t);
        let after = (p.layout.scroll_y(), p.layout.scroll_top(s));
        assert_eq!([after.0, after.1], [window, scroller], "{name}");
        assert_eq!(moved.is_empty(), before == after, "{name}: targets report exactly the moved scrollers");
    }
}

#[test]
fn css_zoom_scales_used_lengths_like_chromium() {
    // TOS AWB editor (positions-awb-customs "zooms the document"): the
    // article is `width: 96rem` with `zoom: calc(var(--document-editor-scale)
    // * 1.5)`, the scale `calc(210mm / 96rem)` (a length over a length). Flow
    // ignored zoom (and that division), so the article stayed 1536px at
    // every zoom step. Effective zoom multiplies absolute lengths, rem, font
    // sizes and line heights of the element and its descendants;
    // percentages stay relative.
    let mut p = page(
        "<!doctype html><body style='margin:0'>\
         <article id=doc style='width:96rem;--scale:calc(210mm / 96rem);zoom:calc(var(--scale) * 1.5)'>\
           <div id=inner style='width:100px;height:20px'></div><div id=half style='width:50%;height:1px'></div>\
           <p id=em style='margin:0;font-size:10px;width:3em;height:1rem'>x</p>\
         </article>\
         <div id=outer style='zoom:2;width:100px'><div id=nested style='zoom:1.5;width:10px;height:10px'></div>\
           <div id=text style='font-size:10px;line-height:12px'>zoomed</div></div>\
         <div id=pct style='zoom:50%;width:200px;height:10px'></div>\
         <div id=vw style='zoom:2;width:10vw;height:1px'></div>\
         </body>",
    );
    let z = 210.0 * 96.0 / 25.4 / 1536.0 * 1.5;
    let near = |a: f64, b: f64, what: &str| assert!((a - b).abs() < 0.05, "{what}: {a} vs {b}");
    let doc = p.metrics("doc");
    near(doc[2], 1536.0 * z, "article width");
    near(p.metrics("inner")[2], 100.0 * z, "px width inside");
    near(p.metrics("inner")[3], 20.0 * z, "px height inside");
    near(p.metrics("half")[2], doc[2] / 2.0, "percentage of the zoomed parent");
    near(p.metrics("em")[2], 30.0 * z, "em (zoomed font size)");
    near(p.metrics("em")[3], 16.0 * z, "rem");
    near(p.metrics("outer")[2], 200.0, "zoom 2");
    near(p.metrics("nested")[2], 30.0, "nested zoom multiplies (2 x 1.5)");
    near(p.metrics("pct")[2], 100.0, "percentage zoom");
    near(p.metrics("text")[3], 24.0, "line height x zoom");
    near(p.metrics("vw")[2], 160.0, "viewport units zoom too (800px viewport)");
    // Zoom changes (the editor's zoom buttons) relayout.
    let doc_el = p.id("doc");
    p.dom
        .set_attribute(doc_el, "style", "width:96rem;--scale:calc(210mm / 96rem);zoom:calc(var(--scale) * 1.4)")
        .unwrap();
    let z2 = 210.0 * 96.0 / 25.4 / 1536.0 * 1.4;
    near(p.metrics("doc")[2], 1536.0 * z2, "article width after zoom out");
    assert!(p.metrics("doc")[2] < doc[2]);
}

#[test]
fn sticky_boxes_stay_in_their_scrollport_like_chromium() {
    // TOS finance tables (TP-4230 master charges header, TS-88 pinned Profit /
    // rightSticky columns, sticky totals footer): `position: sticky` was laid
    // out as relative, so scrolling a panel moved its sticky header, footer
    // and pinned columns away. Sticky boxes now lay out in place and their
    // offset follows the nearest scroll container (or the viewport), bounded
    // by the containing block (the table for table parts). Chromium
    // (Playwright 1.59.1), same markup, before / after scrolling:
    // th 0 / 0, tfoot cell 180 / 180 (pulled to the panel bottom), pinned
    // left 0 / 0 and right 200 / 200, section heading 220 / 210 (stops at its
    // section's bottom), viewport-sticky 370 -> 10 after window scroll 400;
    // hit testing finds th at the panel top and the tfoot cell at the bottom.
    let rows: String = (0..30).map(|i| format!("<tr><td>r{i}</td></tr>")).collect();
    let html = format!(
        "<!doctype html><style>*{{box-sizing:border-box;margin:0}}body{{margin:0}}table{{border-collapse:separate;border-spacing:0}}td,th{{padding:0;height:20px}}\
         #v{{overflow:auto;height:200px;width:300px}}#v th{{position:sticky;top:0}}#v tfoot{{position:sticky;bottom:0}}\
         #h{{overflow-x:auto;width:300px}}#h table{{width:800px}}#h .l{{position:sticky;left:0}}#h .r{{position:sticky;right:0}}#h td{{width:100px}}\
         #s{{overflow:auto;height:150px;width:200px}}#s section{{height:100px}}#s h3{{position:sticky;top:0;height:20px;font-size:12px}}\
         #vp{{position:sticky;top:10px;height:20px;width:50px}}</style><body>\
         <div id=v><table><thead><tr><th id=th>H</th></tr></thead><tbody>{rows}</tbody><tfoot><tr><td id=tf>F</td></tr></tfoot></table></div>\
         <div id=h><table><tr><td class=l id=l>L</td><td>a</td><td>b</td><td>c</td><td>d</td><td>e</td><td>f</td><td class=r id=r>R</td></tr></table></div>\
         <div id=s><section id=sec1><h3 id=h1>One</h3></section><section><h3>Two</h3></section><section><h3>Three</h3></section></div>\
         <div id=vp>vp</div><div style=\"height:2000px\"></div></body>"
    );
    let mut p = page(&html);
    p.layout.set_viewport(Viewport::new(800, 600));
    let rect = |p: &Page, id: &str| p.layout.bounding_client_rect(&p.dom, p.id(id));
    let near = |a: f64, b: f64, what: &str| assert!((a - b).abs() < 0.5, "{what}: {a} vs {b}");
    near(rect(&p, "th").y, 0.0, "header at rest");
    near(rect(&p, "tf").y, 180.0, "footer pulled to the panel bottom at rest");
    near(rect(&p, "l").x, 0.0, "left pinned at rest");
    near(rect(&p, "r").x, 200.0, "right pinned pulled to the panel's right edge");
    near(rect(&p, "h1").y, 220.0, "section heading at rest");
    near(rect(&p, "vp").y, 370.0, "viewport sticky not yet stuck");
    let (v, h, s) = (p.id("v"), p.id("h"), p.id("s"));
    p.layout.set_scroll_top(&p.dom, v, 150);
    p.layout.set_scroll_left(&p.dom, h, 200);
    p.layout.set_scroll_top(&p.dom, s, 90);
    near(rect(&p, "th").y, 0.0, "header stays at the panel top");
    near(rect(&p, "tf").y, 180.0, "footer stays at the panel bottom");
    near(rect(&p, "l").x, 0.0, "left pinned stays at the panel's left edge");
    near(rect(&p, "r").x, 200.0, "right pinned stays at the panel's right edge");
    near(rect(&p, "sec1").y, 130.0, "the section scrolls");
    near(rect(&p, "h1").y, 210.0, "the heading stops at its section's bottom");
    let at = |p: &Page, x: f64, y: f64| p.layout.element_from_point(&p.dom, x, y);
    assert_eq!(at(&p, 5.0, 5.0), Some(p.id("th")), "hit testing follows the sticky header");
    assert_eq!(at(&p, 5.0, 190.0), Some(p.id("tf")), "and the sticky footer");
    p.layout.window_scroll_to(&p.dom, None, Some(400));
    near(rect(&p, "vp").y, 10.0, "viewport sticky sticks below the top inset");
}

#[test]
fn validity_has_anchors_invalidate_selectively_without_stale_geometry() {
    // Tailwind's has-invalid: utility as Lightning CSS serves it to TOS. A
    // `:has()` testing descendant validity made every DOM change restyle the
    // whole document (QRef: every row measurement a full restyle). Its
    // anchors now follow the descendants' value, attribute, option and
    // radio-group changes; each step compares every box with a fresh layout
    // (heights measured in Chromium on the same markup and steps).
    let css = "<style>\
        .card{display:block;height:10px}\
        .card:has(:is(:where([data-rac])[data-invalid], :where(:not([data-rac])):invalid)){height:40px}\
        input,select{display:block;height:10px;width:50px}\
        </style>";
    let html = format!(
        "<!doctype html>{css}<body style=\"margin:0\">\
         <div class=card id=c1><div><input id=req required></div></div>\
         <div class=card id=c2><div><input id=dis></div></div>\
         <div class=card id=c3><select id=sel required><option value=\"\">none</option></select></div>\
         <div class=card id=c4><input type=radio name=g id=r1 required></div>\
         <div class=card id=c5><input type=radio name=g id=r2 required></div></body>"
    );
    let mut p = page(&html);
    assert_matches_fresh(&p, "initial");
    let h = |p: &Page, id: &str| p.layout.layout_result(&p.dom).get(p.id(id)).unwrap().rect.height;
    assert_eq!([h(&p, "c1"), h(&p, "c2"), h(&p, "c3"), h(&p, "c4"), h(&p, "c5")], [40.0, 10.0, 40.0, 40.0, 40.0]);

    let v = p.dom.version();
    let req = p.id("req");
    p.dom.set_value(req, "x").unwrap();
    assert_journaled(&p, v, "value");
    assert_matches_fresh(&p, "a value satisfies required");
    assert_eq!(h(&p, "c1"), 10.0);

    let v = p.dom.version();
    let dis = p.id("dis");
    p.dom.set_attribute(dis, "required", "").unwrap();
    assert_journaled(&p, v, "required");
    assert_matches_fresh(&p, "required makes the empty input invalid");
    assert_eq!(h(&p, "c2"), 40.0);

    let v = p.dom.version();
    p.dom.set_attribute(dis, "disabled", "").unwrap();
    assert_journaled(&p, v, "disabled");
    assert_matches_fresh(&p, "disabled bars it from validation");
    assert_eq!(h(&p, "c2"), 10.0);

    let v = p.dom.version();
    let sel = p.id("sel");
    let opt = p.dom.create_element(p.doc, "option").unwrap();
    p.dom.set_attribute(opt, "value", "x").unwrap();
    p.dom.append_child(sel, opt).unwrap();
    p.dom.set_option_selected(opt, true).unwrap();
    assert_journaled(&p, v, "option");
    assert_matches_fresh(&p, "an inserted, selected option satisfies the select");
    assert_eq!(h(&p, "c3"), 10.0);

    // Checking r2 satisfies the group, so r1 in another card turns valid.
    let v = p.dom.version();
    let r2 = p.id("r2");
    p.dom.set_checked(r2, true).unwrap();
    assert_journaled(&p, v, "radio");
    assert_matches_fresh(&p, "a checked radio satisfies its group");
    assert_eq!([h(&p, "c4"), h(&p, "c5")], [10.0, 10.0]);

    // Renaming r2 out of the group leaves r1 alone and unchecked again: only
    // r2 changed, so r1's card is reached through the group (radio roots).
    let v = p.dom.version();
    p.dom.set_attribute(r2, "name", "h").unwrap();
    assert_journaled(&p, v, "regroup");
    assert_matches_fresh(&p, "a radio leaving the group");
    assert_eq!([h(&p, "c4"), h(&p, "c5")], [40.0, 10.0]);
}

#[test]
fn sticky_boxes_inside_fixed_containers_follow_their_scroller_like_chromium() {
    // TOS position finances open in a `position: fixed` drawer over a fixed
    // modal: sticky table headers and the totals footer there did not stick
    // (TP-4230 measured the header 1621px off its scroller). Boxes inside a
    // fixed subtree stick to their nearest scroll container like any other;
    // with none, the scrollport is the viewport, which the window scroll does
    // not move. Values measured in Chromium (800x600, body margin 0) after
    // scrolling `#sc` to 600.
    let rows: String = (0..40).map(|i| format!("<tr><td style=\"height:50px\">r{i}</td></tr>")).collect();
    let cases = [
        (
            "fixed > scroller > table",
            format!("<div style=\"position:fixed;inset:0;display:flex\"><div id=sc style=\"margin:40px;height:300px;width:400px;overflow:auto\"><table style=\"border-spacing:0;width:100%\"><thead><tr><th id=t style=\"position:sticky;top:0;height:25px;padding:0\">H</th></tr></thead><tbody>{rows}</tbody><tfoot><tr><td id=f style=\"position:sticky;bottom:0;height:20px;padding:0\">F</td></tr></tfoot></table></div></div>"),
            [40.0, 40.0, 320.0],
        ),
        (
            "fixed modal > fixed drawer > flex > scroller",
            format!("<div style=\"position:fixed;inset:0;overflow-y:auto;display:flex\"><div style=\"position:fixed;top:0;right:0;bottom:0;width:500px\"><div style=\"display:flex;flex-direction:column;height:100%\"><div id=sc style=\"flex:1;min-height:0;overflow:auto\"><table style=\"border-spacing:0\"><thead><tr><th id=t style=\"position:sticky;top:0;height:25px;padding:0\">H</th></tr></thead><tbody>{rows}</tbody></table></div></div></div></div>"),
            [0.0, 0.0, f64::NAN],
        ),
    ];
    for (name, body, [before, after, foot]) in cases {
        let mut p = page(&format!("<!doctype html><body style=\"margin:0\">{body}</body>"));
        let y = |p: &Page, id: &str| p.layout.bounding_client_rect(&p.dom, p.id(id)).y;
        assert_eq!(y(&p, "t"), before, "{name}: before scrolling");
        let sc = p.id("sc");
        p.layout.element_scroll_to(&p.dom, sc, None, Some(600));
        assert_eq!(y(&p, "t"), after, "{name}: header after scrolling");
        if !foot.is_nan() {
            assert_eq!(y(&p, "f"), foot, "{name}: footer");
        }
    }
    // No scroll container inside the fixed box: nothing moves (Chromium 50).
    let p = page(
        "<!doctype html><body style=\"margin:0\"><div style=\"position:fixed;top:0;left:0;width:300px;height:200px\"><div style=\"height:1000px\"><div id=t style=\"position:sticky;top:10px;height:20px;margin-top:50px\">S</div></div></div></body>"
    );
    assert_eq!(p.layout.bounding_client_rect(&p.dom, p.id("t")).y, 50.0);
}

#[test]
fn fit_content_around_a_table_shrinks_to_its_min_content_like_chromium() {
    // TOS position finances: `w-fit min-w-full` column-flex wrappers around
    // tables whose columns are sized by their headers. A table's min-content
    // width was its max-content width, so the wrappers grew to every text
    // cell's unwrapped width (2176px vs Chromium's 1822px), pushing the
    // Profit column out of view (TS-88) and misaligning the totals tables.
    // Chromium (800x600): the 600px column container's fit-content item is
    // the table's min-content, 6 columns of 150px, with or without the
    // table's min-width and in fixed layout.
    let cols: String =
        (0..6).map(|i| format!("<th style=\"width:150px;min-width:150px;padding:0\">h{i}</th>")).collect();
    let cells: String = (0..6)
        .map(|i| {
            format!("<td style=\"padding:0\">some longer cell text that would make max content much wider {i}</td>")
        })
        .collect();
    for extra in ["min-width:900px", "", "table-layout:fixed;min-width:900px"] {
        let html = format!(
            "<!doctype html><body style=\"margin:0\"><div style=\"display:flex;flex-direction:column;width:600px;height:400px\">\
             <div id=item style=\"display:flex;flex-direction:column;width:fit-content;min-width:100%;flex:1 1 0%;min-height:0\">\
             <div id=sc style=\"display:flex;flex-direction:column;width:fit-content;min-width:100%;flex:1 1 0%;min-height:0;overflow:auto\">\
             <table id=tb style=\"width:100%;border-collapse:separate;border-spacing:0;{extra}\"><thead><tr>{cols}</tr></thead>\
             <tbody><tr>{cells}</tr></tbody></table></div></div></div></body>"
        );
        let p = page(&html);
        let w = |id: &str| p.layout.bounding_client_rect(&p.dom, p.id(id)).width;
        assert_eq!([w("item"), w("sc"), w("tb")], [900.0, 900.0, 900.0], "table style {extra:?}");
    }
}

/// x and width of `id`'s border box (fixed-table regressions).
fn x_width(p: &Page, id: &str) -> [f64; 2] {
    let r = p.layout.bounding_client_rect(&p.dom, p.id(id));
    [r.x, r.width]
}

fn assert_x_widths(p: &Page, name: &str, expected: &[(&str, [f64; 2])]) {
    for (id, [x, w]) in expected {
        let [ax, aw] = x_width(p, id);
        assert!((ax - x).abs() < 0.1 && (aw - w).abs() < 0.1, "{name} #{id}: got [{ax}, {aw}], Chromium [{x}, {w}]");
    }
}

const FIXED_COLS: &str =
    "<colgroup><col style=\"width:100px\"><col style=\"width:200px\"><col style=\"width:300px\"></colgroup>";

/// A fit-content column-flex item (TOS `w-fit min-w-full`) of a `width`px
/// column container around `table`.
fn fit_wrapper(width: u32, table: &str) -> String {
    format!(
        "<!doctype html><body style=\"margin:0\"><div style=\"display:flex;flex-direction:column;width:{width}px;height:300px\">\
         <div id=item style=\"display:flex;flex-direction:column;width:fit-content;min-width:100%;flex:1 1 0%;min-height:0\">{table}</div></div></body>"
    )
}

#[test]
fn fixed_tables_size_columns_by_their_col_widths_like_chromium() {
    // TOS finance tables: `table-layout: fixed`, 100% wide, `<col>` widths
    // summing to 1822px. The columns ignored `<col>` and the table's
    // min-content counted cell contents, so `w-fit` wrappers grew to 2134px
    // and every column stretched 1.17x (TS-88: Profit out of view). Chromium
    // (800x600): columns take the `<col>` widths, scaled proportionally when
    // the table is wider; the fixed table's min-content is their sum.
    let fixed = |rows: &str, extra: &str| {
        format!("<table id=tb style=\"width:100%;table-layout:fixed;border-collapse:separate;border-spacing:0;{extra}\">{FIXED_COLS}{rows}</table>")
    };
    let head = "<thead><tr><th id=h0 style=\"padding:0\">A</th><th id=h1 style=\"padding:0\">B</th><th id=h2 style=\"padding:0\">C</th></tr></thead>";
    let body = "<tbody><tr><td id=c0 style=\"padding:0\">xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx</td><td id=c1 style=\"padding:0\">some text</td><td id=c2 style=\"padding:0\">more text here</td></tr></tbody>";
    let rows = format!("{head}{body}");
    let wide = format!(
        "<tbody><tr><td id=c0 style=\"padding:0\">{}</td><td id=c1 style=\"padding:0\">{}</td><td id=c2 style=\"padding:0\">{}</td></tr></tbody>",
        "x".repeat(60),
        "y".repeat(60),
        "z".repeat(60)
    );
    let third = 1000.0 / 6.0;
    type Case = (&'static str, String, Vec<(&'static str, [f64; 2])>);
    let cases: [Case; 4] = [
        (
            "wider than its columns",
            fit_wrapper(1000, &fixed(&rows, "min-width:600px")),
            vec![("item", [0.0, 1000.0]), ("tb", [0.0, 1000.0]), ("c0", [0.0, third]), ("c1", [third, 2.0 * third]), ("c2", [500.0, 500.0])],
        ),
        (
            "container narrower than its columns",
            fit_wrapper(400, &fixed(&rows, "min-width:600px")),
            vec![("item", [0.0, 600.0]), ("tb", [0.0, 600.0]), ("c0", [0.0, 100.0]), ("c1", [100.0, 200.0]), ("c2", [300.0, 300.0])],
        ),
        (
            "cell contents wider than every column",
            fit_wrapper(300, &fixed(&wide, "")),
            vec![("item", [0.0, 600.0]), ("tb", [0.0, 600.0]), ("c0", [0.0, 100.0]), ("c1", [100.0, 200.0]), ("c2", [300.0, 300.0])],
        ),
        (
            "<col span>",
            fit_wrapper(
                1000,
                "<table id=tb style=\"width:100%;table-layout:fixed;border-spacing:0\"><colgroup><col style=\"width:100px\"><col span=2 style=\"width:250px\"></colgroup>\
                 <tbody><tr><td id=c0 style=\"padding:0\">a</td><td id=c1 style=\"padding:0\">b</td><td id=c2 style=\"padding:0\">c</td></tr></tbody></table>",
            ),
            vec![("tb", [0.0, 1000.0]), ("c0", [0.0, third]), ("c1", [third, 2500.0 / 6.0]), ("c2", [3500.0 / 6.0, 2500.0 / 6.0])],
        ),
    ];
    for (name, html, expected) in cases {
        let p = page(&html);
        assert_x_widths(&p, name, &expected);
    }
}

#[test]
fn fixed_tables_without_a_header_keep_col_widths_across_colspans_like_chromium() {
    // TOS Grand Total table: same `<col>` widths, no thead, a first body row
    // with a colspan. Its columns split the width evenly instead (194px vs
    // 253px for the master table), so the totals tables misaligned.
    let html = fit_wrapper(
        1000,
        &format!(
            "<table id=tb style=\"width:100%;table-layout:fixed;min-width:600px;border-collapse:separate;border-spacing:0\">{FIXED_COLS}\
             <tbody><tr><td id=c0 colspan=2 style=\"padding:0\">Grand Total</td><td id=c2 style=\"padding:0\">9</td></tr>\
             <tr><td id=d0 style=\"padding:0\">a</td><td id=d1 style=\"padding:0\">b</td><td id=d2 style=\"padding:0\">c</td></tr></tbody></table>"
        ),
    );
    let p = page(&html);
    let third = 1000.0 / 6.0;
    assert_x_widths(
        &p,
        "no header, colspan first row",
        &[
            ("tb", [0.0, 1000.0]),
            ("c0", [0.0, 500.0]),
            ("c2", [500.0, 500.0]),
            ("d0", [0.0, third]),
            ("d1", [third, 2.0 * third]),
            ("d2", [500.0, 500.0]),
        ],
    );
}

/// Expected `[x, width]` per element id.
type XWidths<'a> = Vec<(&'a str, [f64; 2])>;

#[test]
fn table_cell_min_and_max_widths_size_their_columns_like_chromium() {
    // Automatic-layout columns ignored a cell's min-width / max-width while
    // the cell's own box honoured them, so a `min-w-80` cell spilled over the
    // next column and the later positioned cell there won hit testing (TOS
    // VAT Rule combobox: hover intercepted by the neighbouring colspan cell).
    // Chromium (800x600): the column takes the clamped cell (min-width wins;
    // max-width caps even wider content), free space is shared in proportion,
    // and fixed layout ignores both.
    let bx = |id: &str, w: u32| format!("<span id={id} style=\"display:inline-block;width:{w}px;height:20px\"></span>");
    let td = |id: &str, style: &str, inner: &str, attrs: &str| {
        format!("<td id={id} {attrs} style=\"padding:0;{style}\">{inner}</td>")
    };
    let table = |style: &str, rows: &str| format!("<table style=\"border-spacing:0;{style}\">{rows}</table>");
    let row2 = format!(
        "<tr>{}{}{}</tr>",
        td("c", "", &bx("cx", 10), ""),
        td("d", "", &bx("dx", 10), ""),
        td("e", "", &bx("ex", 10), "")
    );
    let narrow = format!(
        "<div style=\"width:400px\">{}</div>",
        table(
            "",
            &format!(
                "<tr>{}{}</tr>{row2}",
                td("a", "min-width:320px;position:relative", "<div id=i style=\"height:20px\"></div>", ""),
                td("b", "position:relative", &bx("bx", 10), "colspan=2")
            )
        )
    );
    let wide = table(
        "width:900px",
        &format!(
            "<tr>{}{}</tr>{row2}",
            td("a", "min-width:320px", &bx("ax", 20), ""),
            td("b", "", &bx("bx", 10), "colspan=2")
        ),
    );
    let pair = |style: &str, w: u32| {
        table("", &format!("<tr>{}{}</tr>", td("a", style, &bx("ax", w), ""), td("b", "", &bx("bx", 10), "")))
    };
    let fixed = table(
        "table-layout:fixed;width:400px",
        &format!(
            "<tr>{}{}{}</tr>",
            td("a", "min-width:320px", &bx("ax", 20), ""),
            td("b", "", &bx("bx", 10), ""),
            td("c", "", &bx("cx", 10), "")
        ),
    );
    let third = 400.0 / 3.0;
    let cases: Vec<(&str, String, XWidths)> = vec![
        (
            "narrow",
            narrow,
            vec![
                ("a", [0.0, 320.0]),
                ("b", [320.0, 20.0]),
                ("c", [0.0, 320.0]),
                ("d", [320.0, 10.0]),
                ("e", [330.0, 10.0]),
            ],
        ),
        (
            "wide",
            wide,
            vec![("a", [0.0, 847.047]), ("b", [847.047, 52.953]), ("d", [847.047, 26.469]), ("e", [873.516, 26.484])],
        ),
        ("min over width", pair("width:100px;min-width:250px", 20), vec![("a", [0.0, 250.0]), ("b", [250.0, 10.0])]),
        ("max width", pair("max-width:50px", 120), vec![("a", [0.0, 50.0]), ("b", [50.0, 10.0])]),
        (
            "border-box min",
            pair("box-sizing:border-box;min-width:200px;padding:0 10px;border:2px solid", 20),
            vec![("a", [0.0, 200.0]), ("b", [200.0, 10.0])],
        ),
        (
            "content-box min",
            pair("min-width:200px;padding:0 10px", 20),
            vec![("a", [0.0, 220.0]), ("b", [220.0, 10.0])],
        ),
        ("fixed", fixed, vec![("a", [0.0, third]), ("b", [third, third]), ("c", [2.0 * third, third])]),
    ];
    for (name, body, expected) in cases {
        let p = page(&format!("<!doctype html><body style=\"margin:0\">{body}</body>"));
        assert_x_widths(&p, name, &expected);
        if name == "narrow" {
            // Inside #a, where the spilled cell used to lie under #b.
            assert_eq!(p.layout.element_from_point(&p.dom, 20.0, 10.0), Some(p.id("i")), "Chromium hits #i");
        }
    }
}

#[test]
fn table_columns_keep_their_exact_width_after_fractional_ones_like_chromium() {
    // A cell's width was the difference of the accumulated column starts, so
    // a 150px column after fractional ones came out 149.99999999999994 (TOS
    // TS-102: a resize clamped to 150px read back below its minimum).
    // Chromium (800x600): the column keeps exactly its width (150; the
    // engine gave 149.99999999999994).
    let html = "<!doctype html><body style=\"margin:0\"><div style=\"padding-left:156.415px\">\
        <table style=\"border-spacing:0\"><tr>\
        <td id=c0 style=\"padding:0;width:128.5244px\">a</td><td id=c1 style=\"padding:0;width:125.5754px\">b</td>\
        <td id=c2 style=\"padding:0;width:150px\">c</td></tr>\
        </table></div></body>";
    let p = page(html);
    let r = p.layout.bounding_client_rect(&p.dom, p.id("c2"));
    assert_eq!(r.width, 150.0, "Chromium keeps the 150px column exact");
}

#[test]
fn table_columns_keep_their_cells_min_content_width_like_chromium() {
    // Gap 20: a cell's explicit `width` clamped its column, so
    // `<td style="width:1px;white-space:pre">E2ESE261200655</td>` was 1px
    // wide (Chromium: 120px, the unbreakable text) and a `width:100%` table
    // of nowrap cells stayed at its 200px container (Chromium: 514px).
    // Chromium's automatic layout gives a column at least its cells'
    // min-content width, explicit widths being preferred widths, and the
    // table grows past its container to the sum of its columns. Glyph
    // advances differ per font, so the assertions compare with the same text
    // measured outside the table.
    let html = "<!doctype html><body style=\"margin:0\">\
        <div><span id=m1 style=\"white-space:pre\">E2ESE261200655</span></div>\
        <div><span id=m2 style=\"white-space:nowrap\">Shipment Body 655A</span></div>\
        <div><span id=m3 style=\"white-space:nowrap\">abcdefghijklmnopqrstuvwxy</span></div>\
        <table id=t1 style=\"border-spacing:0\"><tr>\
        <td id=a style=\"padding:0;width:1px;white-space:pre\"><a href=#>E2ESE261200655</a></td>\
        <td id=b style=\"padding:0;width:1px\">Shipment Body 655A</td></tr></table>\
        <div id=w style=\"width:200px\"><table id=t2 style=\"width:100%;border-spacing:0\"><tr>\
        <td id=c style=\"padding:0;white-space:nowrap\">abcdefghijklmnopqrstuvwxy</td>\
        <td id=d style=\"padding:0;white-space:nowrap\">abcdefghijklmnopqrstuvwxy</td></tr></table></div>\
        <table id=t3 style=\"border-spacing:0\"><tr>\
        <td id=e style=\"padding:0;width:1px\"><a style=\"display:inline-flex\"><span style=\"overflow:hidden;text-overflow:ellipsis;white-space:nowrap\">E2ESE261200655</span></a></td>\
        <td id=f style=\"padding:0;width:1px\">x</td></tr></table>\
        </body>";
    let p = page(html);
    let w = |id: &str| p.layout.bounding_client_rect(&p.dom, p.id(id)).width;
    let (m1, m2, m3) = (w("m1"), w("m2"), w("m3"));
    assert!(m1 > 50.0 && m3 > 150.0, "measured text widths {m1} {m3}");
    // The pre cell holds its whole text; the breakable one at least its
    // longest word ("Shipment"), and both columns of the table are wide
    // enough for them.
    assert!((w("a") - m1).abs() < 0.5, "#a {} vs text {m1}", w("a"));
    assert!(w("b") >= m2 / 3.0 && w("b") < m2, "#b {} vs text {m2}", w("b"));
    assert!((w("t1") - (w("a") + w("b"))).abs() < 0.5, "#t1 {} = #a + #b", w("t1"));
    // The 100% table grows past its 200px container to the two nowrap cells.
    assert!((w("c") - m3).abs() < 0.5 && (w("d") - m3).abs() < 0.5, "#c {} #d {} vs text {m3}", w("c"), w("d"));
    assert!((w("t2") - 2.0 * m3).abs() < 0.5, "#t2 {} = 2 x {m3}", w("t2"));
    assert_eq!(w("w"), 200.0, "the container keeps its width");
    // An inline-flex link around a truncating nowrap span still contributes
    // the text's min-content width.
    assert!((w("e") - m1).abs() < 0.5, "#e {} vs text {m1}", w("e"));
}

/// Styles for the generated-content regressions (16px/20px text).
const GENERATED_STYLES: &str = "<style>body{margin:0;font-family:sans-serif;font-size:16px;line-height:20px}\
    .e:empty::before{content:'';display:inline-block}\
    .tw1:empty::before{content:var(--tw-content);display:inline-block}\
    .tw2:empty::before{--tw-content:'';content:var(--tw-content)}\
    .reg::before{content:var(--tw-content);display:inline-block}\
    .a::after{content:'';display:inline-block}\
    .none::before{content:none;display:inline-block}\
    .unset::before{content:var(--nope);display:inline-block}\
    .abs::before{content:'';display:inline-block;position:absolute}\
    .hide::before{content:'';display:none}\
    .blk::before{content:'';display:block}\
    .blkt::before{content:'q';display:block}\
    .txt::before{content:\"xyz\"}\
    .ib::before{content:'x';display:block}\
    @property --tw-content{syntax:'*';inherits:false;initial-value:''}</style>";

#[test]
fn generated_string_content_takes_a_line_like_chromium() {
    // TOS Cost Type filter: an empty multi-select's trigger holds only
    // `<span class="block truncate empty:before:inline-block
    // empty:before:content-['']">`. The engine generated no ::before box, so
    // the span and the button were 0px tall and Playwright reported the
    // trigger hidden. Chromium (800x600, measured): an empty-string inline-block
    // ::before/::after makes one line (the strut); none, an unresolved var(),
    // an out-of-flow or display:none pseudo-element generates nothing; form
    // controls have none.
    let html = format!(
        "<!doctype html><head>{GENERATED_STYLES}</head><body>\
         <button id=btn style=\"display:block;width:200px;padding:0;border:0;font:inherit\"><span id=s class=e style=\"display:block\"></span></button>\
         <span id=tw class=\"tw1 tw2\" style=\"display:block\"></span>\
         <span id=reg class=reg style=\"display:block\"></span>\
         <div id=a class=a></div>\
         <div id=none class=none></div>\
         <div id=unset class=unset></div>\
         <div id=abs class=abs></div>\
         <div id=hide class=hide></div>\
         <div id=blk class=blk></div>\
         <div id=blkt class=blkt></div>\
         <div style=\"width:400px\"><span id=t1 class=txt>ab</span></div>\
         <div style=\"width:400px\"><span id=t2>xyzab</span></div>\
         <div id=b1 class=txt style=\"width:400px\">ab</div><div id=b2 style=\"width:400px\"><span id=b2s>xyzab</span></div>\
         <div><div id=f1 class=txt style=\"display:inline-block\">ab</div></div><div><div id=f2 style=\"display:inline-block\">xyzab</div></div>\
         <input id=in1 class=ib><input id=in2>\
         </body>"
    );
    let p = page(&html);
    let h = |id: &str| p.layout.bounding_client_rect(&p.dom, p.id(id)).height;
    let w = |id: &str| p.layout.bounding_client_rect(&p.dom, p.id(id)).width;
    for (id, expected) in [
        ("s", 20.0),
        ("btn", 20.0),
        ("tw", 20.0),
        ("reg", 20.0),
        ("a", 20.0),
        ("none", 0.0),
        ("unset", 0.0),
        ("abs", 0.0),
        ("hide", 0.0),
        ("blk", 0.0),
        ("blkt", 20.0),
    ] {
        assert!((h(id) - expected).abs() < 0.01, "#{id} height {}, Chromium {expected}", h(id));
    }
    // Inline generated text is laid out like the element's own text, in an
    // inline element, a block's line and a shrink-to-fit box's width.
    assert!((w("t1") - w("t2")).abs() < 0.01, "#t1 {} vs #t2 {}", w("t1"), w("t2"));
    assert!((h("b1") - h("b2")).abs() < 0.01, "#b1 {} vs #b2 {}", h("b1"), h("b2"));
    assert!((w("f1") - w("f2")).abs() < 0.01, "#f1 {} vs #f2 {}", w("f1"), w("f2"));
    // An input generates no content.
    assert_eq!(h("in1"), h("in2"));
}

#[test]
fn generated_content_follows_dom_changes_like_chromium() {
    // Generated content is cached per element (found and not found). TOS's
    // trigger relies on `:empty`, so the cache must follow text added and
    // removed, class changes and the custom property behind `content`.
    // Chromium (800x600, measured): the empty trigger span stays one 20px
    // line throughout; an `:empty::before{content:'abc'}` inline-block span
    // is "abc" wide, then only its text "x" wide, then "abc" again; a class
    // adding the ::before makes a 0px span 20px and removing it 0px again;
    // `content: var(--c)` follows --c and generates nothing once it is gone.
    let html = "<!doctype html><head><style>body{margin:0;font-family:sans-serif;font-size:16px;line-height:20px}\
        .e:empty::before{content:'';display:inline-block}\
        .t:empty::before{content:'abc';display:inline-block}\
        .v::before{content:var(--c);display:inline-block}</style></head><body>\
        <button id=btn style=\"display:block;width:200px;padding:0;border:0;font:inherit\"><span id=s class=e style=\"display:block\"></span></button>\
        <div><span id=w class=t style=\"display:inline-block\"></span></div>\
        <div><span id=k style=\"display:block\"></span></div>\
        <div><span id=v class=v style=\"display:inline-block;--c:'abc'\"></span></div>\
        <div><span id=rabc style=\"display:inline-block\">abc</span><span id=rx style=\"display:inline-block\">x</span></div></body>";
    let mut p = page(html);
    let rect = |p: &Page, id: &str| p.layout.bounding_client_rect(&p.dom, p.id(id));
    let near = |a: f64, b: f64| (a - b).abs() < 0.01;
    let (abc, x) = (rect(&p, "rabc").width, rect(&p, "rx").width);
    let check = |p: &Page, step: &str, w: f64, k_h: f64, v: (f64, f64)| {
        assert!(near(rect(p, "s").height, 20.0), "{step}: #s height {}, Chromium 20", rect(p, "s").height);
        assert!(near(rect(p, "w").width, w), "{step}: #w width {}, Chromium {w}", rect(p, "w").width);
        assert!(near(rect(p, "k").height, k_h), "{step}: #k height {}, Chromium {k_h}", rect(p, "k").height);
        let r = rect(p, "v");
        assert!(
            near(r.width, v.0) && near(r.height, v.1),
            "{step}: #v {}x{}, Chromium {}x{}",
            r.width,
            r.height,
            v.0,
            v.1
        );
    };
    check(&p, "initial", abc, 0.0, (abc, 20.0));
    let doc = p.doc;
    for id in ["s", "w"] {
        let t = p.dom.create_text(doc, "x").unwrap();
        let el = p.id(id);
        p.dom.append_child(el, t).unwrap();
    }
    check(&p, "text added", x, 0.0, (abc, 20.0));
    for id in ["s", "w"] {
        let el = p.id(id);
        p.dom.set_text_content(el, "").unwrap();
    }
    check(&p, "text removed", abc, 0.0, (abc, 20.0));
    let k = p.id("k");
    p.dom.set_attribute(k, "class", "e").unwrap();
    check(&p, "class added", abc, 20.0, (abc, 20.0));
    p.dom.set_attribute(k, "class", "").unwrap();
    check(&p, "class removed", abc, 0.0, (abc, 20.0));
    let v = p.id("v");
    p.dom.set_attribute(v, "style", "display:inline-block;--c:'x'").unwrap();
    check(&p, "--c changed", abc, 0.0, (x, 20.0));
    p.dom.set_attribute(v, "style", "display:inline-block").unwrap();
    check(&p, "--c removed", abc, 0.0, (0.0, 0.0));
}

#[test]
fn scroll_anchoring_keeps_the_visible_content_in_place_like_chromium() {
    // TOS entity chat: with `overflow-anchor: auto`, a history message above
    // the viewport growing by 200px must move the timeline's scrollTop by as
    // much, keeping the visible messages in place. The engine had no scroll
    // anchoring, so scrollTop stayed. Chromium (800x600, measured): a
    // scroller moves by the change above its anchor, the first fully visible
    // element (growth, shrink and insertion above; after a user wheel scroll
    // too), reading scrollTop already adjusted; not for changes below or to
    // the anchor itself, with `overflow-anchor: none` on the scroller, or at
    // scrollTop 0. `none` on the would-be anchor moves the anchor to the next
    // element; nested scrollers anchor independently.
    let items = |n: usize, p: &str| -> String {
        (0..n).map(|i| format!("<div id={p}{i} style=\"height:50px\"></div>")).collect()
    };
    let scroller = |id: &str, h: u32, inner: &str, extra: &str| {
        format!("<div id={id} style=\"height:{h}px;overflow:auto;{extra}\">{inner}</div>")
    };
    let load = |body: &str| page(&format!("<!doctype html><body style=\"margin:0\">{body}</body>"));
    let set_style = |p: &mut Page, id: &str, style: &str| {
        let el = p.id(id);
        p.dom.set_attribute(el, "style", style).unwrap();
    };
    let one = scroller("sc", 200, &items(20, "i"), "");
    // (name, scroller style, scrollTop, change on #id, Chromium scrollTop)
    let cases: [(&str, &str, i32, &str, &str, u32); 6] = [
        ("grow above", "", 300, "i2", "height:150px", 400),
        ("shrink above", "", 300, "i2", "height:20px", 270),
        ("grow below", "", 300, "i15", "height:150px", 300),
        ("anchor grows", "", 300, "i6", "height:150px", 300),
        ("none on scroller", "overflow-anchor:none;", 300, "i2", "height:150px", 300),
        ("at top", "", 0, "i2", "height:150px", 0),
    ];
    for (name, extra, scroll, id, style, expected) in cases {
        let mut p = load(&scroller("sc", 200, &items(20, "i"), extra));
        let sc = p.id("sc");
        p.layout.set_scroll_top(&p.dom, sc, scroll);
        set_style(&mut p, id, style);
        p.layout.clamp_scroll_offsets(&p.dom);
        assert_eq!(p.layout.scroll_top(sc), expected, "{name}: Chromium scrollTop {expected}");
    }
    // `overflow-anchor: none` on #i6 before scrolling: #i7 anchors instead.
    let mut p = load(&one.replace("id=i6 style=\"height:50px\"", "id=i6 style=\"height:50px;overflow-anchor:none\""));
    let sc = p.id("sc");
    p.layout.set_scroll_top(&p.dom, sc, 300);
    set_style(&mut p, "i2", "height:150px");
    p.layout.clamp_scroll_offsets(&p.dom);
    assert_eq!(p.layout.scroll_top(sc), 400, "none on the would-be anchor: Chromium 400");
    // Insertion above.
    let mut p = load(&one);
    let sc = p.id("sc");
    p.layout.set_scroll_top(&p.dom, sc, 300);
    let doc = p.doc;
    let d = p.dom.create_element(doc, "div").unwrap();
    p.dom.set_attribute(d, "style", "height:100px").unwrap();
    let first = p.dom.first_child(sc).unwrap();
    p.dom.insert_before(sc, d, first).unwrap();
    p.layout.clamp_scroll_offsets(&p.dom);
    assert_eq!(p.layout.scroll_top(sc), 400, "insert above: Chromium 400");
    // A user wheel scroll (300 -> 400) re-anchors; growth above then moves it.
    let mut p = load(&one);
    let sc = p.id("sc");
    p.layout.set_scroll_top(&p.dom, sc, 300);
    p.layout.wheel_scroll(&p.dom, p.id("i7"), 0.0, 100.0);
    assert_eq!(p.layout.scroll_top(sc), 400, "wheel");
    set_style(&mut p, "i2", "height:150px");
    p.layout.clamp_scroll_offsets(&p.dom);
    assert_eq!(p.layout.scroll_top(sc), 500, "wheel then grow above: Chromium 500");
    // Nested scrollers anchor independently.
    let nested =
        scroller("outer", 200, &(items(6, "o") + &scroller("inner", 300, &items(20, "n"), "") + &items(10, "p")), "");
    for (change, outer_top, inner_top) in [("o1", 350, 200), ("n1", 250, 300)] {
        let mut p = load(&nested);
        let (outer, inner) = (p.id("outer"), p.id("inner"));
        p.layout.set_scroll_top(&p.dom, outer, 250);
        p.layout.set_scroll_top(&p.dom, inner, 200);
        set_style(&mut p, change, "height:150px");
        p.layout.clamp_scroll_offsets(&p.dom);
        assert_eq!(
            (p.layout.scroll_top(outer), p.layout.scroll_top(inner)),
            (outer_top, inner_top),
            "nested, #{change} grows: Chromium outer {outer_top}, inner {inner_top}"
        );
    }
}

#[test]
fn scroll_anchoring_rechecks_overflow_anchor_none_like_chromium() {
    // An anchor recorded earlier must not be reused once it or an ancestor
    // has become `overflow-anchor: none`. Chromium (800x600, measured;
    // scroller 200px, 20 x 50px children, scrollTop 300, #i2 then grows by
    // 100px): `none` on the anchor hands over to the next element (400,
    // also within the same task); `none` on a wrapper holding every element
    // leaves nothing to anchor (300); a scroller inside a `none` ancestor
    // still anchors its own content (400).
    let items: String = (0..20).map(|i| format!("<div id=i{i} style=\"height:50px\"></div>")).collect();
    let flat = format!("<div id=sc style=\"height:200px;overflow:auto\">{items}</div>");
    let wrapped = format!("<div id=sc style=\"height:200px;overflow:auto\"><div id=wrap>{items}</div></div>");
    let inside_none = format!("<div style=\"overflow-anchor:none\">{flat}</div>");
    // (name, body, element turned `none` (or ""), settle between, Chromium scrollTop after growth)
    let cases = [
        ("none on anchor, then grow", &flat, "i6", true, 400),
        ("none on anchor with growth", &flat, "i6", false, 400),
        ("none on wrapper, then grow", &wrapped, "wrap", true, 300),
        ("none on wrapper with growth", &wrapped, "wrap", false, 300),
        ("scroller inside none ancestor", &inside_none, "", false, 400),
    ];
    for (name, body, none, settle_between, expected) in cases {
        let mut p = page(&format!("<!doctype html><body style=\"margin:0\">{body}</body>"));
        let sc = p.id("sc");
        p.layout.set_scroll_top(&p.dom, sc, 300);
        if !none.is_empty() {
            let el = p.id(none);
            let style = if none == "wrap" { "overflow-anchor:none" } else { "height:50px;overflow-anchor:none" };
            p.dom.set_attribute(el, "style", style).unwrap();
            if settle_between {
                p.layout.clamp_scroll_offsets(&p.dom);
                assert_eq!(p.layout.scroll_top(sc), 300, "{name}: Chromium 300 before growth");
            }
        }
        let i2 = p.id("i2");
        p.dom.set_attribute(i2, "style", "height:150px").unwrap();
        p.layout.clamp_scroll_offsets(&p.dom);
        assert_eq!(p.layout.scroll_top(sc), expected, "{name}: Chromium scrollTop {expected}");
    }
}

#[test]
fn scroll_anchoring_inside_a_position_fixed_container_like_chromium() {
    // A scroller inside a `position: fixed` drawer anchors its content like
    // any other (TOS entity chat: the timeline lives in a fixed Drawer).
    // Chromium (800x600, measured): a 300px flex-item scroller of 20 x 50px
    // messages (article in a wrapper div) at scrollTop 300; #m2 gets
    // min-height 150px -> 400, also when the content's own min-height
    // already covers the growth (scrollHeight unchanged), without the fixed
    // drawer, and with a fixed or absolute box first in the scroller (at
    // 320px, inside its layout window), which stays excluded.
    let messages: String = (0..20)
        .map(|i| {
            format!(
                "<div><article id=m{i} style=\"display:flex;height:50px\">\
                 <div style=\"width:36px;flex-shrink:0\"></div><div style=\"flex:1\">m</div></article></div>"
            )
        })
        .collect();
    let timeline = |content_style: &str| {
        format!(
            "<div style=\"display:flex;flex-direction:column;height:100%\">\
             <div id=sc style=\"min-height:0;flex:1 1 0%;overflow-y:auto\"> \
             <div style=\"{content_style}\">{messages}</div> </div></div>"
        )
    };
    let out_of_flow = |position: &str| {
        format!(
            "<div style=\"display:flex;flex-direction:column;height:100%\">\
             <div id=sc style=\"min-height:0;flex:1 1 0%;overflow-y:auto\"> \
             <div style=\"position:{position};top:320px;left:0;width:100px;height:20px\"></div>\
             <div>{messages}</div> </div></div>"
        )
    };
    let fixed =
        |inner: String| format!("<div style=\"position:fixed;top:0;right:0;width:600px;height:300px\">{inner}</div>");
    let flat = |inner: String| format!("<div style=\"width:600px;height:300px\">{inner}</div>");
    let cases = [
        ("in a fixed drawer", fixed(timeline(""))),
        ("in a fixed drawer, content min-height covers the growth", fixed(timeline("min-height:1100px"))),
        ("not fixed", flat(timeline(""))),
        ("fixed box first", fixed(out_of_flow("fixed"))),
        ("absolute box first", fixed(out_of_flow("absolute"))),
    ];
    for (name, body) in cases {
        let mut p = page(&format!("<!doctype html><body style=\"margin:0\">{body}</body>"));
        let sc = p.id("sc");
        p.layout.set_scroll_top(&p.dom, sc, 300);
        p.layout.clamp_scroll_offsets(&p.dom);
        assert_eq!(p.layout.scroll_top(sc), 300, "{name}: settled");
        let m2 = p.id("m2");
        p.dom.set_attribute(m2, "style", "display:flex;height:50px;min-height:150px").unwrap();
        p.layout.clamp_scroll_offsets(&p.dom);
        assert_eq!(p.layout.scroll_top(sc), 400, "{name}: Chromium scrollTop 400");
    }
}

#[test]
fn scroll_anchoring_reselects_in_the_previous_layout_like_chromium() {
    // The anchor is selected in the layout the scroller was settled in,
    // with the current styles, however many earlier candidates are now
    // excluded. Chromium (800x600, measured; growth of 100px above):
    // - 400px scroller of 40 x 25px items at scrollTop 300: 7 of the 16
    //   fully visible items turned `none` -> the next anchors (400); all
    //   items `none` -> nothing anchors (300);
    // - 300px scroller holding two wrappers of 10 x 50px at scrollTop 300:
    //   the first wrapper `none` -> the second wrapper's first item anchors
    //   (400; without growth 300);
    // - the anchor made `display: none` or removed with the growth -> the
    //   next item anchors, which moved 100px down and 50px up (350).
    let items = |n: usize, h: u32, p: &str| -> String {
        (0..n).map(|i| format!("<div id={p}{i} style=\"height:{h}px\"></div>")).collect()
    };
    let load = |body: String| page(&format!("<!doctype html><body style=\"margin:0\">{body}</body>"));
    let style = |p: &mut Page, id: &str, value: &str| {
        let el = p.id(id);
        p.dom.set_attribute(el, "style", value).unwrap();
    };
    let settle_top = |p: &mut Page| {
        p.layout.clamp_scroll_offsets(&p.dom);
        let sc = p.id("sc");
        p.layout.scroll_top(sc)
    };
    let tall = || load(format!("<div id=sc style=\"height:400px;overflow:auto\">{}</div>", items(40, 25, "i")));
    let wrappers = || {
        load(format!(
            "<div id=sc style=\"height:300px;overflow:auto\"><div id=wa>{}</div><div id=wb>{}</div></div>",
            items(10, 50, "a"),
            items(10, 50, "b")
        ))
    };
    let short = || load(format!("<div id=sc style=\"height:200px;overflow:auto\">{}</div>", items(20, 50, "i")));
    let scroll = |p: &mut Page| {
        let sc = p.id("sc");
        p.layout.set_scroll_top(&p.dom, sc, 300);
    };

    let mut p = tall();
    scroll(&mut p);
    for k in 12..=18 {
        style(&mut p, &format!("i{k}"), "height:25px;overflow-anchor:none");
    }
    style(&mut p, "i2", "height:125px");
    assert_eq!(settle_top(&mut p), 400, "7 candidates excluded: Chromium 400");

    let mut p = tall();
    scroll(&mut p);
    for k in 0..40 {
        style(
            &mut p,
            &format!("i{k}"),
            if k == 2 { "height:125px;overflow-anchor:none" } else { "height:25px;overflow-anchor:none" },
        );
    }
    assert_eq!(settle_top(&mut p), 300, "all excluded: Chromium 300");

    for (grow, expected) in [(true, 400), (false, 300)] {
        let mut p = wrappers();
        scroll(&mut p);
        style(&mut p, "wa", "overflow-anchor:none");
        if grow {
            style(&mut p, "a2", "height:150px");
        }
        assert_eq!(settle_top(&mut p), expected, "wrapper excluded (growth: {grow}): Chromium {expected}");
    }

    let mut p = short();
    scroll(&mut p);
    style(&mut p, "i6", "height:50px;display:none");
    style(&mut p, "i2", "height:150px");
    assert_eq!(settle_top(&mut p), 350, "anchor display:none with growth: Chromium 350");

    let mut p = short();
    scroll(&mut p);
    let (sc, i6) = (p.id("sc"), p.id("i6"));
    p.dom.remove_child(sc, i6).unwrap();
    style(&mut p, "i2", "height:150px");
    assert_eq!(settle_top(&mut p), 350, "anchor removed with growth: Chromium 350");
}

#[test]
fn caret_from_point_takes_the_nearest_character_boundary_like_chromium() {
    // Native selection needs the caret under a point. Chromium (measured,
    // monospace 20px): a point 20% into the third character is at offset 2,
    // 80% into it at offset 3; past the end of a line, its end; on the next
    // line, that line's text. Points here use the engine's own advances.
    let html = "<!doctype html><body style=\"margin:0;font-size:20px;line-height:30px\">\
        <p id=p style=\"margin:0\">abcdefghij</p><p id=q style=\"margin:0\">klmnopqrst</p></body>";
    let p = page(html);
    let (tp, tq) = (p.dom.first_child(p.id("p")).unwrap().unwrap(), p.dom.first_child(p.id("q")).unwrap().unwrap());
    let r = p.layout.text_rect(&p.dom, tp).unwrap();
    // The engine's advances: the x of each character boundary of "abcdefghij".
    let result = p.layout.text_result(&p.dom);
    let edges = result.text_runs.iter().find(|t| t.node == tp).unwrap().edges().to_vec();
    let w = |s: &str| {
        let at = |o: u32| edges.iter().find(|e| e.0 == o).unwrap().1;
        let start = "abcdefghij".find(s).unwrap() as u32;
        at(start + s.len() as u32) - at(start)
    };
    let y = r.y + r.height / 2.0;
    let body = p.dom.parent_element(p.id("p")).unwrap().unwrap();
    let at = |x: f64, y: f64| p.layout.caret_from_point(&p.dom, body, x, y, &|_| true).map(|(n, o, _)| (n, o));
    assert_eq!(at(r.x + w("ab") + 0.2 * w("c"), y), Some((tp, 2)), "20% into the third character");
    assert_eq!(at(r.x + w("ab") + 0.8 * w("c"), y), Some((tp, 3)), "80% into the third character");
    assert_eq!(at(r.x + r.width + 40.0, y), Some((tp, 10)), "past the end of the line");
    assert_eq!(at(r.x - 5.0, y), Some((tp, 0)), "before the line");
    assert_eq!(at(r.x + 1.0, y + 30.0), Some((tq, 0)), "next line");
    // A filtered-out text node is not a candidate.
    assert_eq!(p.layout.caret_from_point(&p.dom, body, r.x + 1.0, y, &|n| n != tp).map(|(n, _, _)| n), Some(tq));
    // Only text inside the given element (the hit element) is a candidate.
    assert_eq!(p.layout.caret_from_point(&p.dom, p.id("q"), r.x + 1.0, y, &|_| true).map(|(n, _, _)| n), Some(tq));
}

#[test]
fn overflowing_text_extends_the_scroll_width_like_chromium() {
    // A box's scroll size counted only descendant boxes, so a long name as
    // the direct text of a `truncate` cell (nowrap, overflow hidden,
    // text-overflow: ellipsis) kept scrollWidth == clientWidth, and TOS's
    // ellipsis check failed. Chromium (800x600, measured): direct text
    // overflows the scroll width like the same text in a span (374 > 100),
    // also without clipping and inside a clipping box (its own scroll
    // width); padded adds both paddings (394); text clipped by a box inside
    // does not reach the outer box (100); wrapping text does not widen.
    let t = "Bartholomew Longname International Freight Forwarding";
    let clip = "width:100px;overflow:hidden;white-space:nowrap;text-overflow:ellipsis;font-size:16px;line-height:20px";
    let html = format!(
        "<!doctype html><body style=\"margin:0\">\
         <div id=direct style=\"{clip}\">{t}</div>\
         <div id=span style=\"{clip}\"><span>{t}</span></div>\
         <div id=nested style=\"width:100px;overflow:hidden;font-size:16px;line-height:20px\"><div id=inner style=\"width:60px;overflow:hidden;white-space:nowrap\">{t}</div></div>\
         <div id=wrap style=\"width:100px;overflow:hidden;font-size:16px;line-height:20px\">{t}</div>\
         <div id=visible style=\"width:100px;white-space:nowrap;font-size:16px;line-height:20px\">{t}</div>\
         <div id=padded style=\"{clip};padding:0 10px\">{t}</div></body>"
    );
    let p = page(&html);
    // [.., clientWidth, clientHeight, scrollWidth, scrollHeight]
    let m = |id: &str| p.metrics(id);
    let near = |a: f64, b: f64| (a - b).abs() < 0.01;
    let text = m("span")[6];
    assert!(text > 100.0, "span scrollWidth {text}");
    assert!(near(m("direct")[6], text) && near(m("direct")[4], 100.0), "direct: {:?}, span {text}", m("direct"));
    assert!(near(m("inner")[6], text) && near(m("inner")[4], 60.0), "inner: {:?}", m("inner"));
    assert!(near(m("nested")[6], 100.0), "nested: {:?}", m("nested"));
    assert!(near(m("visible")[6], text), "visible: {:?}", m("visible"));
    assert!(near(m("padded")[6], text + 20.0) && near(m("padded")[4], 120.0), "padded: {:?}", m("padded"));
    assert!(near(m("wrap")[6], 100.0) && near(m("wrap")[7], m("wrap")[5]), "wrap: {:?}", m("wrap"));
}

#[test]
fn generated_content_with_tailwinds_escaped_selectors_like_chromium() {
    // TOS's compiled CSS (build 1963ee2cebbd) gives the empty Cost Type
    // trigger span its line through `.empty\:before\:inline-block:is(...):before`
    // and `.empty\:before\:content-\[\'\'\]:is(...):before` in
    // `@layer utilities`. The engine serialized class names unescaped when
    // splitting off `::before`, so these selectors no longer parsed and the
    // span stayed 0px tall (the trigger hidden). Chromium (800x600,
    // measured): the span and the trigger button are 24px tall.
    let is = ":is(:where([data-rac])[data-empty],:where(:not([data-rac])):empty)";
    let css = format!(
        "@layer utilities{{.empty\\:before\\:inline-block{is}:before{{content:var(--tw-content);display:inline-block}}\
         .empty\\:before\\:content-\\[\\'\\'\\]{is}:before{{--tw-content:\"\";content:var(--tw-content)}}}}"
    );
    let html = format!(
        "<!doctype html><head><style>body{{margin:0}}</style><style>{css}</style></head><body>\
         <div style=\"display:flex;width:478px;overflow:hidden;white-space:nowrap;align-items:center;gap:8px;font-size:16px;line-height:24px\">\
         <button id=t data-rac=\"\" type=button style=\"display:block;width:100%;overflow:hidden;white-space:nowrap;text-align:start;padding:0;border:0;background:none;font:inherit;line-height:inherit\">\
         <span id=s class=\"block truncate empty:before:inline-block empty:before:content-[''] text-muted-foreground\" style=\"display:block;overflow:hidden;white-space:nowrap;text-overflow:ellipsis\"></span>\
         </button></div></body>"
    );
    let p = page(&html);
    for id in ["s", "t"] {
        let h = p.layout.bounding_client_rect(&p.dom, p.id(id)).height;
        assert!((h - 24.0).abs() < 0.01, "#{id} height {h}, Chromium 24");
    }
}

/// Cached computed `color` and `width` of `ids` equal a fresh style engine's
/// (no caches) for the same DOM, and the cached geometry equals a fresh
/// layout; returns the cached colors in `ids` order.
fn assert_styles_match_fresh(p: &Page, step: &str, ids: &[&str]) -> Vec<String> {
    assert_matches_fresh(p, step);
    let mut fresh = Layout::with_viewport(p.doc, Viewport::new(800, 600));
    fresh.style_mut().discover_style_elements(&p.dom);
    fresh.layout_result(&p.dom);
    let mut colors = Vec::new();
    for id in ids {
        let el = p.id(id);
        for prop in ["color", "width"] {
            let cached = p.layout.style().computed_value(&p.dom, el, prop);
            let expected = fresh.style().computed_value(&p.dom, el, prop);
            assert_eq!(cached, expected, "{step}: #{id} {prop} cached vs fresh");
        }
        colors.push(p.layout.style().computed_value(&p.dom, el, "color").expect("color"));
    }
    colors
}

#[test]
fn has_anchor_argument_guard_never_serves_stale_styles() {
    // `:has()` anchors are rooted only through path nodes that can satisfy
    // the argument's first compound. Every kind of change the guard looks at
    // must still give exactly the styles (color: the motivating TOS rule is a
    // color rule) and geometry of a fresh computation, and the discriminators
    // show the rules really switch both ways.
    let html = "<!doctype html><style>\
        .lit{display:block;width:10px;height:10px;color:rgb(0,0,0)} :has(> [k]) > .lit{color:rgb(1,2,3);width:30px}\
        .in{display:block;width:10px;height:10px;color:rgb(0,0,0)} .grp:has(> .m) .in{color:rgb(4,5,6);width:40px}\
        .deep{display:block;width:10px;height:10px;color:rgb(0,0,0)} .d2:has(> div > [x]) > .deep{color:rgb(7,8,9);width:50px}\
        .ui{display:block;width:10px;height:10px;color:rgb(0,0,0)} .un:has(.leaf[y]) .ui{color:rgb(10,11,12);width:60px}\
        </style><body style=\"margin:0\">\
        <div id=p1><span id=c1 class=lit>a</span><span id=s1>s</span></div>\
        <div id=p2><span id=c2 class=lit>b</span><span id=s2>s</span></div>\
        <div id=g class=grp><div id=gc>g</div><span id=in class=in>c</span></div>\
        <div id=d class=d2><div id=dw><span id=dx>x</span></div><i id=dsw><span id=dy>y</span></i><span id=deep class=deep>d</span></div>\
        <div id=u class=un><div><div><span id=leaf class=leaf>l</span></div></div><span id=ui class=ui>e</span></div>\
        <div id=q><span id=qlit class=lit>f</span></div></body>";
    let mut p = page(html);
    let subjects = ["c1", "c2", "in", "deep", "ui", "qlit"];
    let base = assert_styles_match_fresh(&p, "initial", &subjects);
    let black = base[0].clone();
    assert!(base.iter().all(|c| *c == black), "{base:?}");
    let id = |p: &Page, s: &str| p.id(s);
    let (p1, p2, s1, s2, gc, dx, dy, leaf, q) = (
        id(&p, "p1"),
        id(&p, "p2"),
        id(&p, "s1"),
        id(&p, "s2"),
        id(&p, "gc"),
        id(&p, "dx"),
        id(&p, "dy"),
        id(&p, "leaf"),
        id(&p, "q"),
    );
    // Key attribute gain and loss on a child (universal depth-1 anchor).
    p.dom.set_attribute(s1, "k", "").unwrap();
    let c = assert_styles_match_fresh(&p, "key gain", &subjects);
    assert_ne!(c[0], black, "c1 restyled through its parent anchor");
    assert_eq!(c[1], black);
    p.dom.remove_attribute(s1, "k").unwrap();
    let c = assert_styles_match_fresh(&p, "key loss", &subjects);
    assert_eq!(c[0], black, "loss invalidates the anchor too");
    // Class gain and loss (class-keyed argument).
    p.dom.set_attribute(gc, "class", "m").unwrap();
    let c = assert_styles_match_fresh(&p, "class gain", &subjects);
    assert_ne!(c[2], black);
    p.dom.set_attribute(gc, "class", "").unwrap();
    let c = assert_styles_match_fresh(&p, "class loss", &subjects);
    assert_eq!(c[2], black);
    // Matching child insertion and removal.
    let n = p.dom.create_element(p.doc, "span").unwrap();
    p.dom.set_attribute(n, "k", "").unwrap();
    p.dom.append_child(p2, n).unwrap();
    let c = assert_styles_match_fresh(&p, "matching child inserted", &subjects);
    assert_ne!(c[1], black);
    p.dom.remove_child(p2, n).unwrap();
    let c = assert_styles_match_fresh(&p, "matching child removed", &subjects);
    assert_eq!(c[1], black);
    // One journal batch: a matching child removed then changed while
    // detached, and a matching child moved from one parent to another.
    let n3 = p.dom.create_element(p.doc, "span").unwrap();
    p.dom.set_attribute(n3, "k", "").unwrap();
    p.dom.append_child(q, n3).unwrap();
    p.dom.set_attribute(s1, "k", "").unwrap();
    let c = assert_styles_match_fresh(&p, "before the batch", &subjects);
    assert_ne!(c[5], black);
    assert_ne!(c[0], black);
    p.dom.remove_child(q, n3).unwrap();
    p.dom.set_attribute(n3, "k", "changed").unwrap();
    p.dom.remove_child(p1, s1).unwrap();
    p.dom.append_child(p2, s1).unwrap();
    let c = assert_styles_match_fresh(&p, "batch: removal + detached change + move", &subjects);
    assert_eq!(c[5], black, "q lost its matching child");
    assert_eq!(c[0], black, "p1 lost its matching child");
    assert_ne!(c[1], black, "p2 gained it");
    p.dom.remove_child(p2, s1).unwrap();
    p.dom.append_child(p1, s1).unwrap();
    p.dom.remove_attribute(s1, "k").unwrap();
    let c = assert_styles_match_fresh(&p, "moved back and key removed in one batch", &subjects);
    assert_eq!(c[0], black);
    assert_eq!(c[1], black);
    // Bounded two-level chain on a class-keyed anchor: only a grandchild
    // below a `div` counts.
    p.dom.set_attribute(dx, "x", "").unwrap();
    let c = assert_styles_match_fresh(&p, "chain gain under div", &subjects);
    assert_ne!(c[3], black);
    p.dom.remove_attribute(dx, "x").unwrap();
    let c = assert_styles_match_fresh(&p, "chain loss", &subjects);
    assert_eq!(c[3], black);
    p.dom.set_attribute(dy, "x", "").unwrap();
    let c = assert_styles_match_fresh(&p, "chain under a non-div wrapper", &subjects);
    assert_eq!(c[3], black);
    p.dom.remove_attribute(dy, "x").unwrap();
    // Several changed paths in one batch, sharing ancestors: a non-admitting
    // walk through p2 first, then an admitting one, and another anchor.
    p.dom.set_attribute(s2, "style", "height:2px").unwrap();
    p.dom.set_attribute(s2, "k", "").unwrap();
    p.dom.set_attribute(s1, "k", "").unwrap();
    let c = assert_styles_match_fresh(&p, "batch: two anchors, memoised walks", &subjects);
    assert_ne!(c[0], black);
    assert_ne!(c[1], black);
    p.dom.remove_attribute(s1, "k").unwrap();
    p.dom.remove_attribute(s2, "k").unwrap();
    let c = assert_styles_match_fresh(&p, "batch: both keys lost", &subjects);
    assert_eq!(c[0], black);
    assert_eq!(c[1], black);
    // Descendant (unbounded) argument: the change reaches the anchor through
    // wrappers that do not satisfy the argument's first compound.
    p.dom.set_attribute(leaf, "y", "").unwrap();
    let c = assert_styles_match_fresh(&p, "unbounded gain through wrappers", &subjects);
    assert_ne!(c[4], black);
    p.dom.remove_attribute(leaf, "y").unwrap();
    let c = assert_styles_match_fresh(&p, "unbounded loss", &subjects);
    assert_eq!(c[4], black);
}

// ---------------------------------------------------------------------------
// Text scroll-size scan: memoized clip facts vs the reference ancestor walk.
// In debug builds `record` asserts, for every text/box pair, that the fact-based
// clip test equals the walk and that the text lies inside the recorded box, so
// these fixtures exercise the producers of text runs (block, inline, fixed,
// absolute, padding, display:contents, deep nesting, reparenting, viewport and
// style changes) and lock the observable scroll sizes.
// ---------------------------------------------------------------------------

fn sw(p: &Page, id: &str) -> f64 {
    p.layout.scroll_width(&p.dom, p.id(id))
}

#[test]
fn text_clip_facts_nested_block_and_inline_clippers() {
    let p = page(
        r#"<div id=outer style="width:200px"><div id=clip style="width:100px;overflow:hidden"><span id=s style="white-space:nowrap">a very long run of text that overflows its clipping container by far</span></div>
<div id=open style="width:100px;padding-right:7px"><span style="white-space:nowrap">another very long run of text that overflows an open container</span></div>
<div id=inl style="width:100px"><span id=ispan style="overflow:hidden;white-space:nowrap">inline overflow hidden does not clip in chromium but the walk sees the style</span></div></div>"#,
    );
    // Like Chromium, a clipping container's own scrollWidth reports its overflowing text, but the text is
    // clipped inside the outer box and does not extend it beyond its own open children.
    assert!(sw(&p, "clip") > 100.0, "clip sw {}", sw(&p, "clip"));
    // The open container is extended by its text through its inline-end padding, and so is the outer box.
    assert!(sw(&p, "open") > 107.0, "open sw {}", sw(&p, "open"));
    // An inline `overflow:hidden` span does not clip (as in Chromium), so the third child's text also
    // extends it and the outer box; the clipping container's text extends neither.
    assert!(sw(&p, "inl") > 100.0);
    let widest_open = sw(&p, "open").max(sw(&p, "inl"));
    assert!(
        (sw(&p, "outer") - widest_open).abs() < 0.01,
        "outer {} widest open child {}",
        sw(&p, "outer"),
        widest_open
    );
    assert!(sw(&p, "outer") < sw(&p, "clip") + sw(&p, "open"), "outer must not add the clipped text");
}

#[test]
fn text_clip_facts_fixed_absolute_and_contents_wrappers() {
    let p = page(
        r#"<div id=root style="width:300px;height:200px;overflow:auto;position:relative">
<div id=rel style="position:relative;width:120px;padding:4px"><div id=abs style="position:absolute;left:0;top:0;width:50px"><span style="white-space:nowrap">absolute child text that is long enough to overflow</span></div>
<div style="display:contents"><span id=c style="white-space:nowrap">text under a display contents wrapper that overflows the relative box</span></div></div>
<div id=fix style="position:fixed;left:0;top:0;width:40px"><span style="white-space:nowrap">fixed text that must not extend a non fixed ancestor</span></div>
<div id=pad style="width:80px;padding:0 9px"><span style="white-space:nowrap">padded container with overflowing text</span></div></div>"#,
    );
    assert!(sw(&p, "abs") > 50.0);
    assert!(sw(&p, "rel") > 120.0);
    assert!(sw(&p, "pad") > 89.0);
    // Fixed text does not extend the scrolling root; its own box reports its text width.
    assert!(sw(&p, "fix") > 40.0);
    let _ = sw(&p, "root");
}

#[test]
fn text_clip_facts_deep_nesting_without_recursion() {
    // On a test thread's default 2 MB stack: layout itself recurses once per
    // level (Taffy), about 2.4 KB here (5.3 KB before its per-level frames
    // were trimmed, which overflowed at about 380 levels).
    deep_nesting(400);
}

#[test]
fn deep_nesting_lays_out_on_a_browser_thread_stack() {
    // A browser thread's 8 MB stack (lp-cdp `BROWSER_STACK_BYTES`, a main
    // thread's): 2,000 nested blocks lay out, in debug builds too.
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| deep_nesting(2000))
        .unwrap()
        .join()
        .expect("no stack overflow");
}

fn deep_nesting(depth: usize) {
    let open: String = (0..depth).map(|i| format!("<div id=n{i} style=\"padding-left:1px\">")).collect();
    let close = "</div>".repeat(depth);
    let html = format!(
        "{open}<span id=leaf style=\"white-space:nowrap\">deep text that overflows the narrow nesting</span>{close}"
    );
    let p = page(&html);
    assert!(sw(&p, "n0") > 0.0);
    assert!(sw(&p, &format!("n{}", depth - 1)) > 0.0);
    assert_eq!(p.layout.bounding_client_rect(&p.dom, p.id("leaf")).x, depth as f64 + 8.0);
}

#[test]
fn text_clip_facts_are_per_pass_across_reparenting_and_style_changes() {
    let mut p = page(
        r#"<div id=wrap style="width:120px"><div id=a style="width:120px;overflow:hidden"><span id=t style="white-space:nowrap">a long run of text that overflows whichever container holds it</span></div><div id=b style="width:120px"></div></div>"#,
    );
    // Clipped inside `a`: the wrapper is not extended.
    assert!(sw(&p, "a") > 120.0);
    assert_eq!(sw(&p, "wrap"), 120.0);
    // Move the text under the open container: the new pass must not reuse the old container's clip fact.
    let (t, b) = (p.id("t"), p.id("b"));
    p.dom.append_child(b, t).unwrap();
    assert_eq!(sw(&p, "a"), 120.0);
    assert!(sw(&p, "wrap") > 120.0, "wrap sw {}", sw(&p, "wrap"));
    // Style change: the open container starts clipping and the wrapper shrinks back.
    p.dom.set_attribute(b, "style", "width:120px;overflow:hidden").unwrap();
    assert!(sw(&p, "b") > 120.0);
    assert_eq!(sw(&p, "wrap"), 120.0);
    // Viewport change: facts are recomputed with the same result.
    p.layout.set_viewport(Viewport::new(500, 400));
    assert_eq!(sw(&p, "wrap"), 120.0);
}

#[test]
fn text_clip_facts_cover_absolute_tables_inline_splits_generated_and_shadow_text() {
    // Absolute containing block above intervening (one clipping) wrappers; the absolute box's text is
    // clipped by the wrapper for every ancestor above it and extends only the absolute box itself.
    let p = page(
        r#"<div id=cb style="position:relative;width:200px"><div id=w1 style="width:150px"><div id=w2 style="width:150px;overflow:hidden"><div><div id=abs style="position:absolute;left:0;top:0;width:30px"><span style="white-space:nowrap">absolute text behind two wrappers that overflows a lot</span></div></div></div></div></div>"#,
    );
    assert!(sw(&p, "abs") > 30.0);
    // Wrappers record before the absolute box is placed, so their scroll sizes ignore it; the containing
    // block's scroll size is extended by the placed absolute boxes afterwards (box scan, unchanged here).
    assert_eq!(sw(&p, "w1"), 150.0);
    assert_eq!(sw(&p, "w2"), 150.0);
    assert!(sw(&p, "cb") >= 200.0);
    // Table cell text inside a clipping container, and a cell whose text overflows the table.
    let p = page(
        r#"<div id=tc style="width:120px;overflow:hidden"><table id=tbl style="width:100px"><tr><td style="width:40px"><span style="white-space:nowrap">a long table cell text that overflows the cell and the table</span></td><td>x</td></tr></table></div><div id=after style="width:120px">after</div>"#,
    );
    assert!(sw(&p, "tbl") >= 100.0);
    let _ = sw(&p, "tc");
    assert_eq!(sw(&p, "after"), 120.0);
    // Inline element split by a block child (anonymous block boxes around the inline runs).
    let p = page(
        r#"<div id=split style="width:100px"><span id=sp>inline start text<div style="width:50px"><span style="white-space:nowrap">block inside the inline with overflowing text</span></div>inline end text</span></div>"#,
    );
    assert!(sw(&p, "split") >= 100.0);
    // Generated ::before text through the real style engine (laid out as an inline item, not a text run).
    let p = page(
        r#"<style>#gen::before{content:"generated text long enough to overflow its narrow host box"; white-space:nowrap}</style><div id=genwrap style="width:200px"><div id=gen style="width:60px;overflow:hidden"></div></div>"#,
    );
    let _ = (sw(&p, "gen"), sw(&p, "genwrap"));
    // Shadow root with a slot: the engine lays the light text out in its DOM position; the assertions
    // in `record` still hold for every pair.
    let mut p = page(
        r#"<div id=shwrap style="width:200px"><div id=host style="width:80px"><span id=light style="white-space:nowrap">slotted light text that overflows the host</span></div></div>"#,
    );
    let host = p.id("host");
    let root = p.dom.attach_shadow(host, lp_dom::ShadowRootMode::Open).unwrap();
    let clip = p.dom.create_element(p.doc, "div").unwrap();
    p.dom.set_attribute(clip, "style", "width:40px;overflow:hidden").unwrap();
    let slot = p.dom.create_element(p.doc, "slot").unwrap();
    p.dom.append_child(clip, slot).unwrap();
    p.dom.append_child(root, clip).unwrap();
    let _ = (sw(&p, "host"), sw(&p, "shwrap"));
}

/// A textarea's `scrollHeight` follows its value (hard breaks, wrapping),
/// not its box: the autosize pattern sets `height: 0` then reads it.
#[test]
fn textarea_scroll_height_follows_its_value() {
    {
        let geometry = "blitz";
        let mut p = page(
            "<!doctype html><body><textarea id=ta rows=1 style=\"width:300px;padding:4px;border:1px solid #000;font-size:16px;line-height:20px\"></textarea></body>"
        );
        let ta = p.id("ta");
        let empty = p.layout.scroll_height(&p.dom, ta);
        assert!(empty >= 20.0, "{geometry:?}: an empty textarea is one line tall, got {empty}");
        p.dom.set_value(ta, "line1\nline2\nline3\nline4").unwrap();
        let four = p.layout.scroll_height(&p.dom, ta);
        assert!(four >= empty + 3.0 * 19.0, "{geometry:?}: four lines, got {four} (empty {empty})");
        // Autosize: height 0, scrollHeight still reports the content.
        p.dom
            .set_attribute(
                ta,
                "style",
                "width:300px;padding:4px;border:1px solid #000;font-size:16px;line-height:20px;height:0px",
            )
            .unwrap();
        let zeroed = p.layout.scroll_height(&p.dom, ta);
        assert!(zeroed >= four - 1.0, "{geometry:?}: height 0 keeps scrollHeight {four}, got {zeroed}");
        assert!(p.layout.client_height(&p.dom, ta) < zeroed, "{geometry:?}: the box is smaller than its content");
        // A long line wraps to the control's width.
        p.dom.set_value(ta, &"word ".repeat(80)).unwrap();
        let wrapped = p.layout.scroll_height(&p.dom, ta);
        assert!(wrapped > 3.0 * 19.0, "{geometry:?}: wrapped lines, got {wrapped}");
    }
}

/// An atomic inline inside an inline element is hit, not the inline (its box
/// is recorded after its children's). Chromium: elementFromPoint at an
/// `<a><svg></svg></a>` icon is the svg.
#[test]
fn atomic_inline_in_an_inline_element_is_hit_over_it() {
    let p = page(
        "<!doctype html><body style=\"margin:0\"><div><a id=a href=/x><svg id=s width=24 height=24></svg></a></div><div><span id=sp>x<svg id=s2 width=24 height=24></svg></span></div></body>"
    );
    for (id, parent) in [("s", "a"), ("s2", "sp")] {
        let r = p.layout.bounding_client_rect(&p.dom, p.id(id));
        assert_eq!((r.width, r.height), (24.0, 24.0), "#{id} has its attribute size");
        let hit = p.layout.element_from_point(&p.dom, r.x + 12.0, r.y + 12.0);
        assert_eq!(hit, Some(p.id(id)), "Chromium hits #{id}, not its inline parent #{parent}");
        let chain = p.layout.elements_from_point(&p.dom, r.x + 12.0, r.y + 12.0);
        assert_eq!(chain.first(), Some(&p.id(id)));
        assert_eq!(chain.get(1), Some(&p.id(parent)));
    }
}

/// Chat-composer autosize variants: the textarea is a flex item of a column
/// with `min-height: 0`, border-box with padding at `height: 0`, empty with
/// only a placeholder, or inside a `display: none` parent that is shown
/// later. In Chromium `scrollHeight` is never 0 for a textarea: at least
/// the line height plus padding. `rows` sizes `clientHeight`/`offsetHeight`.
#[test]
fn textarea_scroll_height_in_autosize_composers() {
    const STYLE: &str = "<style>*{box-sizing:border-box;margin:0}textarea{font:inherit;line-height:24px;padding:8px;border:1px solid #000;width:300px;resize:none;overflow:hidden}body{font-size:16px}</style>";
    for geometry in ["blitz"] {
        let mut p = page(
            &format!(
                "<!doctype html>{STYLE}<body>\
                 <div style=\"display:flex;flex-direction:column;min-height:0;height:200px\"><div style=\"flex:1;min-height:0\"></div>\
                 <textarea id=flex rows=1 style=\"height:0px;min-height:0\" placeholder=\"Message\"></textarea></div>\
                 <div style=\"display:flex;align-items:flex-end;gap:8px;width:400px\">\
                 <textarea id=row rows=1 style=\"flex:1 1 0%;width:auto;height:0px;max-height:200px\"></textarea><button>Send</button></div>\
                 <div style=\"display:grid;grid-template-columns:1fr auto;width:400px\">\
                 <textarea id=grid rows=1 style=\"width:100%;height:0px\"></textarea><button>Go</button></div>\
                 <div style=\"position:relative;height:100px\"><textarea id=abs rows=1 style=\"position:absolute;left:0;bottom:0;height:0px\"></textarea></div>\
                 <textarea id=zero rows=1 style=\"height:0px\"></textarea>\
                 <textarea id=auto rows=1 style=\"height:auto\" placeholder=\"Say something\"></textarea>\
                 <div id=hidden style=\"display:none\"><textarea id=later rows=1 style=\"height:0px\"></textarea></div>\
                 <textarea id=rows rows=3></textarea>\
                 <textarea id=minh rows=1 style=\"height:0px;min-height:40px\"></textarea>\
                 <textarea id=norows style=\"height:0px\"></textarea>\
                 </body>"
            )
        );
        // Empty (placeholder only): one line plus padding, 24 + 16 = 40.
        for id in ["flex", "row", "grid", "abs", "zero", "auto", "minh", "norows"] {
            let sh = p.layout.scroll_height(&p.dom, p.id(id));
            assert!((39.0..=41.0).contains(&sh), "{geometry:?}: #{id} empty scrollHeight {sh}");
        }
        // Three lines of text: 3 x 24 + 16 = 88, whatever the box height.
        for id in ["flex", "row", "grid", "abs", "zero", "auto", "minh", "norows"] {
            p.dom.set_value(p.id(id), "a\nb\nc").unwrap();
            let sh = p.layout.scroll_height(&p.dom, p.id(id));
            assert!((87.0..=89.0).contains(&sh), "{geometry:?}: #{id} three-line scrollHeight {sh}");
        }
        // The autosize step: height follows scrollHeight, reads stay stable.
        p.dom.set_attribute(p.id("zero"), "style", "height:88px").unwrap();
        let sh = p.layout.scroll_height(&p.dom, p.id("zero"));
        assert!((87.0..=89.0).contains(&sh), "{geometry:?}: sized to content, scrollHeight {sh}");
        let ch = p.layout.client_height(&p.dom, p.id("zero"));
        assert!((85.0..=87.0).contains(&ch), "{geometry:?}: clientHeight {ch} (88 border-box minus 2px borders)");
        // Hidden parent: 0 while hidden, the line height once shown.
        assert_eq!(p.layout.scroll_height(&p.dom, p.id("later")), 0.0, "{geometry:?}: display:none");
        p.dom.set_attribute(p.id("hidden"), "style", "").unwrap();
        let sh = p.layout.scroll_height(&p.dom, p.id("later"));
        assert!((39.0..=41.0).contains(&sh), "{geometry:?}: shown later, scrollHeight {sh}");
        // rows=3: content 72 + padding 16 = client 88, offset 90 with borders.
        let rows = p.id("rows");
        let (client, offset) = (p.layout.client_height(&p.dom, rows), p.layout.offset_height(&p.dom, rows));
        assert!((87.0..=89.0).contains(&client), "{geometry:?}: rows=3 clientHeight {client}");
        assert!((89.0..=91.0).contains(&offset), "{geometry:?}: rows=3 offsetHeight {offset}");
        let sh = p.layout.scroll_height(&p.dom, rows);
        assert!((87.0..=89.0).contains(&sh), "{geometry:?}: rows=3 empty scrollHeight is the box, {sh}");
    }
}

/// A text input's `scrollWidth` is its value's width when the value is
/// longer than the box (truncation detectors compare it with offsetWidth).
#[test]
fn text_input_scroll_width_follows_its_value() {
    {
        let geometry = "blitz";
        let mut p = page(
            "<!doctype html><body><input id=i style=\"width:80px;padding:0;border:0;font-size:14px\" value=short><textarea id=ta style=\"width:80px;padding:0;border:0;font-size:14px\"></textarea></body>"
        );
        let i = p.id("i");
        let short = p.layout.scroll_width(&p.dom, i);
        assert!((79.0..=81.0).contains(&short), "{geometry:?}: a short value fills the box, got {short}");
        p.dom.set_value(i, "E2E-ISO-DEBTOR-9C70E3E9D4").unwrap();
        let long = p.layout.scroll_width(&p.dom, i);
        assert!(long > 150.0, "{geometry:?}: the long value's width, got {long}");
        assert!(p.layout.client_width(&p.dom, i) <= 81.0, "{geometry:?}: the box stays 80px");
        let ta = p.id("ta");
        p.dom.set_value(ta, "a\nE2E-ISO-DEBTOR-9C70E3E9D4\nb").unwrap();
        // A textarea wraps (Chromium's UA sheet: `white-space: pre-wrap;
        // overflow-wrap: break-word`), breaking the long word: Chromium reads
        // scrollWidth 80 and scrollHeight 96 (six lines).
        let w = p.layout.scroll_width(&p.dom, ta);
        assert!(w <= 81.0, "{geometry:?}: the textarea's lines wrap, got {w}");
        // At least four lines: twice the two rows the box shows.
        let (h, rows) = (p.layout.scroll_height(&p.dom, ta), p.layout.client_height(&p.dom, ta));
        assert!(h >= 2.0 * rows, "{geometry:?}: the long word breaks onto lines, got {h} in {rows}");
    }
}

/// Border boxes [x, y, width, height] of the elements with these ids, all
/// compared to Chromium's (within 0.01px).
fn check_rects(name: &str, html: &str, expected: &[(&str, [f64; 4])]) {
    let p = page(html);
    let mut diffs = Vec::new();
    for (id, want) in expected {
        let r = p.layout.bounding_client_rect(&p.dom, p.id(id));
        let got = [r.x, r.y, r.width, r.height];
        if got.iter().zip(want).any(|(g, w)| (g - w).abs() > 0.01) {
            diffs.push(format!("#{id}: got {got:?}, Chromium {want:?}"));
        }
    }
    assert!(diffs.is_empty(), "{name}:\n{}", diffs.join("\n"));
}

#[test]
fn margins_collapse_through_parents_and_empty_blocks_like_chromium() {
    // CSS 2.1 8.3.1, measured in Chromium (800x600): a first child's top
    // margin collapses with its parent's (and body's) unless padding or a
    // border separates them or the parent is a new formatting context; a
    // last child's bottom margin collapses with an auto-height parent's;
    // an empty block's margins collapse through it; negative margins add to
    // the largest positive one.
    let html = "<!doctype html><body><div id=a><h1 id=h style=\"height:30px;margin:20px 0\">x</h1><p id=p1 style=\"height:10px\">x</p></div>\
        <div id=b style=\"margin-top:5px\"><div id=b1 style=\"margin-top:40px;height:10px\"></div></div><div id=e style=\"margin:15px 0\"></div>\
        <div id=c style=\"margin-top:10px;padding-top:1px\"><div id=c1 style=\"margin-top:30px;height:10px;margin-bottom:25px\"></div></div>\
        <div id=d style=\"height:5px;margin-top:-10px\"></div>\n<section id=s style=\"overflow:hidden\"><p id=s1 style=\"margin:12px 0;height:4px\"></p></section>\
        <div id=f style=\"margin-bottom:7px\"><div id=f1 style=\"height:3px;margin-bottom:33px\"></div></div><div id=g style=\"height:2px\"></div></body>";
    check_rects(
        "margins",
        html,
        &[
            ("a", [8.0, 20.0, 784.0, 60.0]),
            ("h", [8.0, 20.0, 784.0, 30.0]),
            ("p1", [8.0, 70.0, 784.0, 10.0]),
            ("b", [8.0, 120.0, 784.0, 10.0]),
            ("b1", [8.0, 120.0, 784.0, 10.0]),
            ("e", [8.0, 145.0, 784.0, 0.0]),
            ("c", [8.0, 145.0, 784.0, 41.0]),
            ("c1", [8.0, 176.0, 784.0, 10.0]),
            ("d", [8.0, 201.0, 784.0, 5.0]),
            ("s", [8.0, 206.0, 784.0, 28.0]),
            ("s1", [8.0, 218.0, 784.0, 4.0]),
            ("f", [8.0, 234.0, 784.0, 3.0]),
            ("f1", [8.0, 234.0, 784.0, 3.0]),
            ("g", [8.0, 270.0, 784.0, 2.0]),
        ],
    );
    // UA heading sizes and margins, line content between blocks, chains of
    // first children, an empty span (no line box), an empty block before
    // its sibling, a flex item (no collapsing with its children) and an
    // absolutely positioned first child (skipped).
    let html = "<!doctype html><body style=\"font-size:16px;line-height:20px;font-family:sans-serif\"><h1 id=h1>Title</h1><p id=p1>para one</p>\n\
        <p id=p2 style=\"margin-bottom:30px\">two</p>text after<div id=d1 style=\"margin-top:12px;height:5px\"></div>\n\
        <ul id=ul><li id=li1>a</li><li id=li2>b</li></ul>\n\
        <div id=n1 style=\"margin-top:-20px\"><div id=n2 style=\"margin-top:10px\"><div id=n3 style=\"margin-top:25px;height:8px;margin-bottom:-5px\"></div></div></div>\n\
        <div id=x1 style=\"margin-bottom:10px\">  <span id=sp></span>  </div><div id=x2 style=\"margin-top:20px;height:4px\"></div>\n\
        <div id=y1><div id=y2 style=\"margin:10px 0\"></div><div id=y3 style=\"margin-top:30px;height:6px\"></div></div>\n\
        <div id=z1 style=\"display:flex\"><div id=z2 style=\"margin-top:9px\"><p id=z3 style=\"margin:14px 0\">in flex</p></div></div>\n\
        <div id=w1 style=\"position:relative\"><div id=w2 style=\"position:absolute;top:0\"></div><p id=w3 style=\"margin-top:17px;height:3px\"></p></div>\n\
        <div id=last style=\"margin-bottom:50px\"><p id=lp style=\"margin-bottom:60px;height:2px\"></p></div></body>";
    let y = |id: &'static str, x: f64, y: f64, w: f64, h: f64| (id, [x, y, w, h]);
    check_rects(
        "margins in text",
        html,
        &[
            y("h1", 8.0, 21.44, 784.0, 20.0),
            y("p1", 8.0, 62.88, 784.0, 20.0),
            y("p2", 8.0, 98.88, 784.0, 20.0),
            y("d1", 8.0, 180.88, 784.0, 5.0),
            y("ul", 8.0, 201.88, 784.0, 40.0),
            y("li1", 48.0, 201.88, 744.0, 20.0),
            y("li2", 48.0, 221.88, 744.0, 20.0),
            y("n1", 8.0, 246.88, 784.0, 8.0),
            y("n2", 8.0, 246.88, 784.0, 8.0),
            y("n3", 8.0, 246.88, 784.0, 8.0),
            y("x1", 8.0, 249.88, 784.0, 0.0),
            y("sp", 8.0, 249.88, 0.0, 0.0),
            y("x2", 8.0, 269.88, 784.0, 4.0),
            y("y1", 8.0, 303.88, 784.0, 6.0),
            y("y2", 8.0, 303.88, 784.0, 0.0),
            y("y3", 8.0, 303.88, 784.0, 6.0),
            y("z1", 8.0, 309.88, 784.0, 57.0),
            y("w1", 8.0, 383.88, 784.0, 3.0),
            y("w2", 8.0, 383.88, 0.0, 0.0),
            y("w3", 8.0, 383.88, 784.0, 3.0),
            y("last", 8.0, 402.88, 784.0, 2.0),
            y("lp", 8.0, 402.88, 784.0, 2.0),
        ],
    );
}

#[test]
fn line_height_below_the_content_area_like_chromium() {
    // A 32px font in 20px lines: the half-leading is negative on both sides
    // and the line box stays 20px (it was 22: the descent was floored at 0).
    let html = "<!doctype html><body style=\"margin:0;font-family:sans-serif\"><div id=a style=\"font-size:32px;line-height:20px\">Title</div>\
        <div id=b style=\"font-size:16px;line-height:20px\"><div id=c style=\"font-size:32px\">Title</div></div>\
        <div id=d style=\"font-size:16px;line-height:20px\"><h1 id=e style=\"margin:0\">Title</h1></div></body>";
    check_rects(
        "tight lines",
        html,
        &[
            ("a", [0.0, 0.0, 800.0, 20.0]),
            ("b", [0.0, 20.0, 800.0, 20.0]),
            ("c", [0.0, 20.0, 800.0, 20.0]),
            ("d", [0.0, 40.0, 800.0, 20.0]),
            ("e", [0.0, 40.0, 800.0, 20.0]),
        ],
    );
}

#[test]
fn an_atomic_inline_wider_than_its_line_ends_it() {
    // Chromium (800x600, 16px/20px Arial): a 500px inline-block in a 400px
    // line overflows it; content after it breaks to the next line, and a box
    // that ends the paragraph leaves no empty line behind.
    let box_ = |id: &str| format!("<span id={id} style=\"display:inline-block;width:500px;height:10px\"></span>");
    let html = format!(
        "<!doctype html><body style=\"margin:0;font:16px/20px Arial\"><div id=a style=\"width:400px\">{} hello world</div><div id=b style=\"width:400px\">abc{}def</div><div id=c style=\"width:400px\">abc {}{}</div><div id=d style=\"width:400px;white-space:nowrap\">abc {} x</div></body>",
        box_("a1"),
        box_("b1"),
        box_("c1"),
        box_("c2"),
        box_("d1")
    );
    check_rects(
        "oversized-inline",
        &html,
        &[
            ("a", [0.0, 0.0, 400.0, 40.0]),
            ("a1", [0.0, 5.0, 500.0, 10.0]),
            ("b", [0.0, 40.0, 400.0, 60.0]),
            ("b1", [0.0, 65.0, 500.0, 10.0]),
            ("c", [0.0, 100.0, 400.0, 60.0]),
            ("c1", [0.0, 125.0, 500.0, 10.0]),
            ("c2", [0.0, 145.0, 500.0, 10.0]),
            ("d", [0.0, 160.0, 400.0, 20.0]),
        ],
    );
}

#[test]
fn table_rowspan_like_chromium() {
    // Chromium (800x600): a spanning cell takes the first free column of
    // later rows; when it is taller than its rows, they grow in proportion
    // to their heights (all into the last when they are empty); a rowspan
    // past the group's end stops there; a row without cells still counts.
    let html = "<!doctype html><body style=\"margin:0\"><table id=t style=\"border-spacing:2px\"><tr id=r1><td id=a rowspan=2 style=\"height:100px;width:30px;padding:0\"></td><td id=b style=\"height:10px;width:20px;padding:0\"></td></tr><tr id=r2><td id=c style=\"height:30px;padding:0\"></td></tr><tr id=r3><td id=d style=\"padding:0;height:5px\"></td><td id=e style=\"padding:0;height:5px\"></td></tr></table>\
        <table id=t2 style=\"border-spacing:0\"><tr id=q1><td id=f rowspan=3 style=\"height:90px;padding:0;width:10px\"></td><td id=g style=\"padding:0;width:10px\"></td></tr><tr id=q2><td id=h style=\"padding:0\"></td></tr><tr id=q3><td id=i style=\"padding:0;height:20px\"></td></tr></table>\
        <table style=\"border-spacing:0\"><tr id=w1><td id=w rowspan=5 style=\"height:90px;padding:0;width:10px\"></td></tr><tr id=w2></tr></table></body>";
    check_rects(
        "rowspan",
        html,
        &[
            ("t", [0.0, 0.0, 56.0, 111.0]),
            ("r1", [2.0, 2.0, 52.0, 24.5]),
            ("a", [2.0, 2.0, 30.0, 100.0]),
            ("b", [34.0, 2.0, 20.0, 24.5]),
            ("r2", [2.0, 28.5, 52.0, 73.5]),
            ("c", [34.0, 28.5, 20.0, 73.5]),
            ("r3", [2.0, 104.0, 52.0, 5.0]),
            ("d", [2.0, 104.0, 30.0, 5.0]),
            ("e", [34.0, 104.0, 20.0, 5.0]),
            ("t2", [0.0, 111.0, 20.0, 90.0]),
            ("q1", [0.0, 111.0, 20.0, 0.0]),
            ("f", [0.0, 111.0, 10.0, 90.0]),
            ("h", [10.0, 111.0, 10.0, 0.0]),
            ("q3", [0.0, 111.0, 20.0, 90.0]),
            ("i", [10.0, 111.0, 10.0, 90.0]),
            ("w1", [0.0, 201.0, 10.0, 0.0]),
            ("w2", [0.0, 201.0, 10.0, 90.0]),
            ("w", [0.0, 201.0, 10.0, 90.0]),
        ],
    );
}

#[test]
fn grid_template_areas_and_named_lines_like_chromium() {
    // Chromium (800x600): items placed by area name (`grid-area`), by
    // `[name]` lines (also the nth one and from the end, inside repeat()),
    // by an area's implicit `-start` / `-end` lines; a name no line has is
    // the first implicit line. Areas extend the explicit grid.
    let html = "<!doctype html><html><head><style>body{margin:0;font-family:sans-serif;font-size:16px;line-height:20px}#g{display:grid;width:600px;grid-template-columns:200px 1fr 100px;grid-template-rows:50px auto 40px;grid-template-areas:\"head head head\" \"side main main\" \"foot foot foot\";gap:10px}#h{grid-area:head}#s{grid-area:side}#m{grid-area:main;height:80px}#f{grid-area:foot}#g2{display:grid;width:500px;grid-auto-columns:20px;grid-template-columns:[a] 100px [b c] 1fr [d] 50px [e];grid-auto-rows:30px}#x1{grid-column:b / d}#x2{grid-column:a / e;grid-row:2}#x3{grid-column:c}#x4{grid-column:2 a / span 1}#g3{display:grid;width:400px;grid-template-columns:80px 150px;grid-auto-columns:40px;grid-template-areas:\"x . y\" \"x z z\";grid-auto-rows:25px}#y1{grid-area:y}#z1{grid-area:z}#x5{grid-area:x}#q{grid-column:nope}#q2{grid-row:x-start / x-end}#g4{display:grid;width:300px;grid-template-columns:repeat(2,[l] 50px) [r];grid-auto-rows:20px}#w1{grid-column:l 2 / r}#w2{grid-column:l -1}</style></head><body><div id=g><div id=h>H</div><div id=m>M</div><div id=s>S</div><div id=f>F</div></div><div id=g2><div id=x1>1</div><div id=x2>2</div><div id=x3>3</div><div id=x4>4</div></div><div id=g3><div id=q>q</div><div id=y1>y</div><div id=z1>z</div><div id=x5>x</div><div id=q2>q2</div></div><div id=g4><div id=w1>w1</div><div id=w2>w2</div></div></body></html>";
    check_rects(
        "grid areas",
        html,
        &[
            ("g", [0.0, 0.0, 600.0, 190.0]),
            ("h", [0.0, 0.0, 600.0, 50.0]),
            ("m", [210.0, 60.0, 390.0, 80.0]),
            ("s", [0.0, 60.0, 200.0, 80.0]),
            ("f", [0.0, 150.0, 600.0, 40.0]),
            ("g2", [0.0, 190.0, 500.0, 90.0]),
            ("x1", [100.0, 190.0, 310.0, 30.0]),
            ("x2", [0.0, 220.0, 460.0, 30.0]),
            ("x3", [100.0, 250.0, 310.0, 30.0]),
            ("x4", [480.0, 250.0, 20.0, 30.0]),
            ("g3", [0.0, 280.0, 400.0, 50.0]),
            ("q", [310.0, 280.0, 40.0, 25.0]),
            ("y1", [230.0, 280.0, 40.0, 25.0]),
            ("z1", [80.0, 305.0, 190.0, 25.0]),
            ("x5", [0.0, 280.0, 80.0, 50.0]),
            ("q2", [270.0, 280.0, 40.0, 50.0]),
            ("g4", [0.0, 330.0, 300.0, 40.0]),
            ("w1", [50.0, 330.0, 50.0, 20.0]),
            ("w2", [50.0, 350.0, 50.0, 20.0]),
        ],
    );
}

#[test]
fn floats_like_chromium() {
    // Chromium (800x600): left/right floats with margins; lines shortened
    // beside them; floats stacking and dropping below when the line is
    // full; formatting context roots (overflow, flex) beside floats or
    // below when too wide, containing their floats; clear and a clearfix
    // ::after; a float in the middle of a line; floats leaving a block
    // into its parent's context.
    check_rects(
        "floats",
        "<!doctype html><html><head><style>body{margin:0;font-family:sans-serif;font-size:16px;line-height:20px}.box{background:#ccc}#c1{width:400px}#a{float:left;width:100px;height:50px;margin:5px}#b{float:right;width:80px;height:30px}#p1{margin:0}#c2{width:300px;overflow:hidden}#d{float:left;width:120px;height:40px}#e{float:left;width:120px;height:60px}#f{float:left;width:120px;height:20px}#c3{width:300px}#g{float:left;width:50px;height:50px}#h{overflow:hidden;height:30px}#i{clear:both;height:10px}#c4{width:300px}#j{float:right;width:100px;height:20px;margin-left:10px}.cf::after{content:\"\";display:table;clear:both}#k{float:left;width:60px;height:45px}#c5{width:200px}#l{float:left;padding:4px;border:1px solid}</style></head><body><div id=c1><div id=a></div><div id=b></div><p id=p1><span id=s1>Some text that wraps around the floated boxes on both sides of the container, long enough to go below them all.</span></p></div><div id=c2><div id=d></div><div id=e></div><div id=f></div></div><div id=c3><div id=g></div><div id=h>bfc</div><div id=i></div></div><div id=c4 class=cf><span id=s2>Lead text </span><div id=j></div><div id=k></div></div><div id=c5><div id=l>shrink</div><span id=s3>after</span></div><div id=end style=\"height:5px\"></div></body></html>",
        &[
            ("c1", [0.0, 0.0, 400.0, 80.0]),
            ("a", [5.0, 5.0, 100.0, 50.0]),
            ("b", [320.0, 0.0, 80.0, 30.0]),
            ("p1", [0.0, 0.0, 400.0, 80.0]),
            ("c2", [0.0, 80.0, 300.0, 80.0]),
            ("d", [0.0, 80.0, 120.0, 40.0]),
            ("e", [120.0, 80.0, 120.0, 60.0]),
            ("f", [0.0, 140.0, 120.0, 20.0]),
            ("c3", [0.0, 160.0, 300.0, 60.0]),
            ("g", [0.0, 160.0, 50.0, 50.0]),
            ("h", [50.0, 160.0, 250.0, 30.0]),
            ("i", [0.0, 210.0, 300.0, 10.0]),
            ("c4", [0.0, 220.0, 300.0, 45.0]),
            ("j", [200.0, 220.0, 100.0, 20.0]),
            ("k", [0.0, 220.0, 60.0, 45.0]),
            ("c5", [0.0, 265.0, 200.0, 20.0]),
            ("end", [0.0, 285.0, 800.0, 5.0]),
        ],
    );
    check_rects(
        "floats 2",
        "<!doctype html><html><head><style>body{margin:0;font-family:monospace;font-size:10px;line-height:20px}div{box-sizing:border-box}#c1{width:200px}#a{display:inline-block;width:150px;height:10px}#b{float:left;width:100px;height:30px}#c2{width:200px}#m1{margin-bottom:20px;height:10px}#fl{float:left;width:50px;height:15px}#m2{margin-top:30px;height:10px}#c3{width:200px}#g{float:left;width:120px;height:40px}#h{overflow:hidden;width:100px;height:10px}#h2{display:flex;height:10px}#c4{width:200px}#inner{margin:0}#n1{float:right;width:60px;height:50px}#after{height:10px}#after2{overflow:hidden;height:10px}#c5{width:200px}#sp{}#n2{float:left;width:40px;height:40px}#c6{width:200px}#r1{float:right;width:90px;height:20px}#r2{float:right;width:90px;height:20px}#r3{float:right;width:90px;height:20px}#l1{float:left;width:30px;height:70px;clear:right}</style></head><body><div id=c1><div id=a></div><div id=b></div></div><div id=c2><div id=m1></div><div id=fl></div><div id=m2></div></div><div id=c3><div id=g></div><div id=h></div><div id=h2>x</div></div><div id=c4><div id=inner><div id=n1></div></div><div id=after></div><div id=after2></div></div><div id=c5><span id=sp>aa <div id=n2></div>bb</span></div><div id=c6 style=\"overflow:hidden\"><div id=r1></div><div id=r2></div><div id=r3></div><div id=l1></div></div><div id=end style=\"height:5px\"></div></body></html>",
        &[
            ("c1", [0.0, 0.0, 200.0, 20.0]),
            ("b", [0.0, 20.0, 100.0, 30.0]),
            ("c2", [0.0, 20.0, 200.0, 50.0]),
            ("m1", [0.0, 20.0, 200.0, 10.0]),
            ("fl", [0.0, 50.0, 50.0, 15.0]),
            ("m2", [0.0, 60.0, 200.0, 10.0]),
            ("c3", [0.0, 70.0, 200.0, 60.0]),
            ("g", [0.0, 70.0, 120.0, 40.0]),
            ("h", [0.0, 110.0, 100.0, 10.0]),
            ("h2", [0.0, 120.0, 200.0, 10.0]),
            ("c4", [0.0, 130.0, 200.0, 20.0]),
            ("inner", [0.0, 130.0, 200.0, 0.0]),
            ("n1", [140.0, 130.0, 60.0, 50.0]),
            ("after", [0.0, 130.0, 200.0, 10.0]),
            ("after2", [0.0, 140.0, 140.0, 10.0]),
            ("c5", [0.0, 150.0, 200.0, 20.0]),
            ("n2", [0.0, 150.0, 40.0, 40.0]),
            ("c6", [40.0, 180.0, 200.0, 110.0]),
            ("r1", [150.0, 180.0, 90.0, 20.0]),
            ("r2", [60.0, 180.0, 90.0, 20.0]),
            ("r3", [150.0, 200.0, 90.0, 20.0]),
            ("l1", [40.0, 220.0, 30.0, 70.0]),
            ("end", [0.0, 290.0, 800.0, 5.0]),
        ],
    );
}

#[test]
fn float_changes_relayout_their_neighbours() {
    // Subtrees beside floats are not replayed from an earlier pass: a float
    // that grows, appears or goes away moves the text and boxes around it.
    let html = "<!doctype html><body style=\"margin:0;line-height:20px\"><div id=c style=\"width:300px\"><div id=f style=\"float:left;width:50px;height:40px\"></div><p id=p style=\"margin:0\">Some words that wrap beside the float and continue below it for a while.</p><div id=b style=\"overflow:hidden;height:10px\"></div></div><div id=after style=\"height:10px\"></div></body>";
    let mut p = page(html);
    let f = p.id("f");
    assert_matches_fresh(&p, "first");
    p.dom.set_attribute(f, "style", "float:left;width:150px;height:90px").unwrap();
    assert_matches_fresh(&p, "wider float");
    p.dom.set_attribute(f, "style", "float:right;width:150px;height:90px").unwrap();
    assert_matches_fresh(&p, "right float");
    p.dom.set_attribute(f, "style", "width:150px;height:90px").unwrap();
    assert_matches_fresh(&p, "no float");
    p.dom.set_attribute(f, "style", "float:left;width:20px;height:20px").unwrap();
    assert_matches_fresh(&p, "float again");
    let c = p.id("c");
    p.dom.set_attribute(c, "style", "width:300px;overflow:hidden").unwrap();
    assert_matches_fresh(&p, "container contains its float");
}

#[test]
fn table_structure_changes_relayout_like_a_fresh_pass() {
    // Table rows, cells and spans are kept across passes until a change
    // reaches the table's structure; edits inside cells keep it, row
    // inserts, span and display changes do not.
    let html = "<!doctype html><body style=\"margin:0;line-height:20px\"><table id=t style=\"border-spacing:0\"><tbody id=b><tr id=r1><td id=a style=\"padding:0\"><span id=s>one</span></td><td id=c style=\"padding:0\">two</td></tr><tr id=r2><td id=d style=\"padding:0\">three</td><td id=e style=\"padding:0\">four</td></tr></tbody></table><div id=after style=\"height:10px\"></div></body>";
    let mut p = page(html);
    assert_matches_fresh(&p, "first");
    let span = p.id("s");
    p.dom.set_text_content(span, "a much longer first cell").unwrap();
    assert_matches_fresh(&p, "text inside a cell");
    let a = p.id("a");
    p.dom.set_attribute(a, "colspan", "2").unwrap();
    assert_matches_fresh(&p, "colspan");
    p.dom.set_attribute(p.id("d"), "rowspan", "2").unwrap();
    assert_matches_fresh(&p, "rowspan");
    let (b, r1) = (p.id("b"), p.id("r1"));
    let clone = p.dom.clone_node(r1, true, None).unwrap();
    p.dom.append_child(b, clone).unwrap();
    assert_matches_fresh(&p, "row appended");
    p.dom.set_attribute(p.id("c"), "style", "padding:0;display:none").unwrap();
    assert_matches_fresh(&p, "cell hidden");
    let r2 = p.id("r2");
    p.dom.set_attribute(r2, "style", "height:50px").unwrap();
    assert_matches_fresh(&p, "row height");
    let e = p.id("e");
    p.dom.remove_child(r2, e).unwrap();
    assert_matches_fresh(&p, "cell removed");
    p.dom.set_text_content(span, "x").unwrap();
    assert_matches_fresh(&p, "text again");
}

#[test]
fn structural_selectors_follow_child_list_changes_without_stale_geometry() {
    // Siblings restyle only as the parent's selector flags require (Blitz
    // used to restyle the parent's whole subtree): edge children for
    // `:first-child`/`:last-child`, later ones for `~`, all for
    // `:nth-last-child`. Each step compares every box with a fresh layout.
    let css = "<style>\
        li{height:10px;width:10px} .e li:last-child{width:201px} .e li:first-child{width:202px}\
        .s .on ~ li{width:203px} .n li:nth-last-child(2){width:204px}\
        </style>";
    let html = format!(
        "<!doctype html>{css}<body style=\"margin:0\">\
         <ul id=e class=e><li id=e1></li><li id=e2></li></ul>\
         <ul id=s class=s><li id=s1></li><li id=s2 class=on></li><li id=s3></li></ul>\
         <ul id=n class=n><li id=n1></li><li id=n2></li><li id=n3></li></ul>\
         <ul id=x></ul>\
         </body>"
    );
    let mut p = page(&html);
    assert_matches_fresh(&p, "initial");
    let li = |p: &mut Page| p.dom.create_element(p.doc, "li").unwrap();
    // Appended: the old last child is no longer :last-child.
    let (e, x) = (p.id("e"), p.id("x"));
    let a = li(&mut p);
    p.dom.append_child(e, a).unwrap();
    assert_matches_fresh(&p, "appended after the last child");
    // Prepended: the old first child is no longer :first-child.
    let b = li(&mut p);
    let e1 = p.id("e1");
    p.dom.insert_before(e, b, Some(e1)).unwrap();
    assert_matches_fresh(&p, "inserted before the first child");
    // Removed: the new last child is :last-child.
    p.dom.remove_child(e, a).unwrap();
    assert_matches_fresh(&p, "last child removed");
    // `~`: a sibling inserted after `.on` matches; removing `.on` unmatches.
    let (s, s1, s2) = (p.id("s"), p.id("s1"), p.id("s2"));
    let c = li(&mut p);
    p.dom.insert_before(s, c, Some(s1)).unwrap();
    assert_matches_fresh(&p, "inserted before .on");
    p.dom.remove_child(s, s2).unwrap();
    assert_matches_fresh(&p, ".on removed");
    // `:nth-last-child`: an append shifts every match.
    let n = p.id("n");
    let d = li(&mut p);
    p.dom.append_child(n, d).unwrap();
    assert_matches_fresh(&p, "appended under :nth-last-child");
    // Moved into a list without structural rules, and back.
    let n1 = p.id("n1");
    p.dom.append_child(x, n1).unwrap();
    assert_matches_fresh(&p, "moved out");
    p.dom.insert_before(n, n1, None).unwrap();
    assert_matches_fresh(&p, "moved back");
}

#[test]
fn an_attribute_change_over_a_new_child_restyles_without_stale_geometry() {
    // A class change whose rules reach descendants walks the element's
    // subtree to invalidate it; a child inserted in the same pass has no
    // computed style yet (Stylo panicked reading it).
    let css = "<style>.box{width:10px;height:10px} .on .box{width:200px}</style>";
    let html = format!("<!doctype html>{css}<body style=\"margin:0\"><div id=p><div class=box></div></div></body>");
    let mut p = page(&html);
    assert_matches_fresh(&p, "initial");
    let parent = p.id("p");
    let child = p.dom.create_element(p.doc, "div").unwrap();
    p.dom.set_attribute(child, "class", "box").unwrap();
    let inner = p.dom.create_element(p.doc, "span").unwrap();
    p.dom.append_child(child, inner).unwrap();
    p.dom.append_child(parent, child).unwrap();
    p.dom.set_attribute(parent, "class", "on").unwrap();
    assert_matches_fresh(&p, "class set over a new child");
}

#[test]
fn computed_styles_come_from_blitz_even_under_display_none() {
    // getComputedStyle answers from the page's Blitz document; elements the
    // traversal skips (a display: none subtree) are resolved there on
    // demand, and follow changes like the rest.
    let mut p = page(
        "<!doctype html><style>#h{display:none} #h b{color:rgb(4, 4, 4)} .on #h b{color:rgb(7, 7, 7)} li{color:rgb(1, 2, 3)}</style>\
         <body><ul id=u><li id=l>x</li></ul><div id=h><b id=b>y</b></div></body>",
    );
    let v = |p: &Page, id: &str, prop: &str| p.layout.computed_style_value(&p.dom, p.id(id), prop);
    assert_eq!(v(&p, "l", "color"), "rgb(1, 2, 3)");
    assert_eq!(v(&p, "h", "display"), "none");
    assert_eq!(v(&p, "b", "color"), "rgb(4, 4, 4)");
    assert_eq!(v(&p, "b", "display"), "inline");
    let body = p.dom.body(p.doc).unwrap().unwrap();
    p.dom.set_attribute(body, "class", "on").unwrap();
    assert_eq!(v(&p, "b", "color"), "rgb(7, 7, 7)");
}

#[test]
fn computed_styles_outside_a_rendered_document_are_empty() {
    // As in Chromium: an element that is not connected reads "" for every
    // property, including one that was connected when its declaration was
    // taken (Chrome's getComputedStyle is live).
    let mut p = page("<!doctype html><style>div{color:rgb(1, 2, 3)}</style><body><div id=c>x</div></body>");
    let c = p.id("c");
    assert_eq!(p.layout.computed_style_value(&p.dom, c, "color"), "rgb(1, 2, 3)");
    let detached = p.dom.create_element(p.doc, "div").unwrap();
    assert_eq!(p.layout.computed_style_value(&p.dom, detached, "display"), "");
    let body = p.dom.body(p.doc).unwrap().unwrap();
    p.dom.remove_child(body, c).unwrap();
    assert_eq!(p.layout.computed_style_value(&p.dom, c, "color"), "");
    assert_eq!(p.layout.computed_style_value(&p.dom, c, "display"), "");
}

#[test]
fn a_drop_down_select_has_a_box_like_chromium() {
    // Blitz hides a select's options (it paints the control itself), and the
    // select had no box at all (0x0): hit testing and visibility missed it.
    // Values measured in Chromium (Playwright 1.59.1).
    let p = page(
        "<!doctype html><body><div><select id=w style=\"width:120px\"><option>x</option></select></div>\
         <div><select id=a><option>x</option></select></div>",
    );
    let w = p.metrics("w");
    assert_eq!((w[2], w[3]), (120.0, 19.0), "explicit width, UA border, 13.333px: {w:?}");
    let a = p.metrics("a");
    assert!(a[2] > 20.0 && a[3] == 19.0, "intrinsic box: {a:?}");
    let pre = page(&format!(
        "<!doctype html>{PREFLIGHT}<body><select id=s style=\"width:120px\"><option>x</option></select></body>"
    ));
    let s = pre.metrics("s");
    assert_eq!((s[2], s[3]), (120.0, 20.0), "under Tailwind's preflight: {s:?}");
}

#[test]
#[cfg(target_os = "macos")]
fn text_inputs_in_system_ui_are_as_wide_as_in_chromium() {
    // Chromium (macOS, SF Text) sizes a text input in system-ui at
    // ceil(0.525em x size) + round(0.4375em). Measured in Chrome with no border or
    // padding: size 1/5/10/20/40 at 13.333px and 10/20 at 16px.
    let mut html =
        String::from("<!doctype html><style>*{margin:0} input{border:0;padding:0;display:block}</style><body>");
    for s in [1, 5, 10, 20, 40] {
        html.push_str(&format!("<input id=s{s} size={s}>"));
    }
    html.push_str("<input id=b10 size=10 style=\"font-size:16px\"><input id=b20 size=20 style=\"font-size:16px\">");
    let p = page(&html);
    for (id, want) in
        [("s1", 13.0), ("s5", 41.0), ("s10", 76.0), ("s20", 146.0), ("s40", 286.0), ("b10", 91.0), ("b20", 175.0)]
    {
        assert_eq!(p.metrics(id)[2], want, "#{id}");
    }
}

#[test]
fn table_cell_declared_height_is_minimum() {
    let p = page(
        r#"<!doctype html><style>*{box-sizing:border-box}body{margin:0}table{border-spacing:0;width:400px}td{padding:0;height:26px;overflow:hidden}</style><table><tbody><tr><td id="short"><div style="height:10px"></div></td></tr></tbody><tfoot><tr><td id="footer"><div id="content" style="display:flex;padding:24px 16px;height:68px">Empty</div></td></tr></tfoot></table>"#,
    );
    assert_eq!(p.metrics("short")[3], 26.0);
    assert!(p.metrics("footer")[3] >= 68.0, "footer {:?}, content {:?}", p.metrics("footer"), p.metrics("content"));
}
