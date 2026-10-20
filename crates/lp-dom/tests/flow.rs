//! Flow layout Stage 1 parity with headless Chromium (Playwright 1.59.1,
//! 800x600 viewport). Expected values were measured in Chromium on the same
//! markup (getBoundingClientRect, clientWidth/Height, scrollWidth/Height);
//! boxes use explicit sizes so approximate glyph widths do not matter.
use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::layout::{Geometry, Layout};
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

fn page(html: &str, geometry: Geometry) -> Page {
    let html = html.replace("FILLER", &filler()).replace("OPTIONS", &options());
    let dom_ref: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom_ref, "http://test/", &html);
    let dom = Rc::try_unwrap(dom_ref).ok().expect("sole owner").into_inner();
    let mut layout = Layout::with_viewport(doc, Viewport::new(800, 600));
    layout.style_mut().discover_style_elements(&dom);
    layout.set_geometry(geometry);
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
    let mut p = page(html, Geometry::Flow);
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
    let html = "<!doctype html><body style=\"margin:0\"><div id=row style=\"width:250px\"><span id=s1 style=\"display:inline-block;width:100px;height:20px\"></span><span id=s2 style=\"display:inline-block;width:100px;height:30px\"></span><span id=s3 style=\"display:inline-block;width:100px;height:20px\"></span></div><div id=after style=\"height:10px\"></div></body>";
    check(
        "inlineblock",
        html,
        0,
        &[
            ("row", [0.0, 0.0, 250.0, 58.0, 250.0, 58.0, 250.0, 58.0]),
            ("s1", [0.0, 10.0, 100.0, 20.0, 100.0, 20.0, 100.0, 20.0]),
            ("s2", [100.0, 0.0, 100.0, 30.0, 100.0, 30.0, 100.0, 30.0]),
            ("s3", [0.0, 34.0, 100.0, 20.0, 100.0, 20.0, 100.0, 20.0]),
            ("after", [0.0, 58.0, 800.0, 10.0, 800.0, 10.0, 800.0, 10.0]),
        ],
        [800.0, 600.0],
    );
}

#[test]
fn flow_hit_test_picks_the_option_under_the_point() {
    let p = page("<!doctype html><body><main>FILLER</main><div id=pop style=\"position:absolute;top:120px;left:200px;width:280px\"><div id=list role=listbox style=\"max-height:300px;overflow:auto\">OPTIONS</div></div></body>", Geometry::Flow);
    let r = p.layout.bounding_client_rect(&p.dom, p.id("o3"));
    let hit = p.layout.element_from_point(&p.dom, r.x + 10.0, r.y + 10.0);
    assert_eq!(hit, Some(p.id("o3")), "Chromium hits #o3");
}

#[test]
fn default_geometry_is_unchanged_by_the_flow_switch() {
    // The approximate layout stays the default, and switching to flow and back
    // reports exactly the approximate geometry again.
    let html = "<!doctype html><body><main>FILLER</main><div id=pop style=\"position:absolute;top:120px;left:200px;width:280px\"><div id=list role=listbox style=\"max-height:300px;overflow:auto\">OPTIONS</div></div></body>";
    let fresh = page(html, Geometry::Approximate);
    assert_eq!(fresh.layout.geometry(), Geometry::Approximate);
    let mut toggled = page(html, Geometry::Flow);
    let _ = toggled.metrics("o11");
    toggled.layout.set_geometry(Geometry::Approximate);
    for id in ["pop", "list", "o0", "o1", "o11"] {
        assert_eq!(fresh.metrics(id), toggled.metrics(id), "#{id}");
    }
    assert_eq!(fresh.layout.document_scroll_size(&fresh.dom), toggled.layout.document_scroll_size(&toggled.dom));
    // Approximate geometry is the upstream formula, not Chromium's.
    assert_ne!(fresh.metrics("o1")[0], 200.0);
}

// ----- Stage 2: flex -----------------------------------------------------------

use lp_dom::flow::{self, Align, Dim, FlexDirection, FlowStyle, FlowStyleSource, Justify, Len, TablePart, TextAlign};

/// The cascade's flow inputs plus the inline flex declarations, until the
/// style engine fills the flex fields (style_flow.rs).
struct InlineFlex<'a> {
    style: &'a lp_dom::style::StyleEngine,
}

fn flex_len(v: &str) -> Option<Len> {
    let v = v.trim();
    if v == "0" {
        return Some(Len::ZERO);
    }
    if let Some(n) = v.strip_suffix("px") {
        return n.parse().ok().map(Len::px);
    }
    if let Some(n) = v.strip_suffix('%') {
        return n.parse().ok().map(Len::percent);
    }
    None
}

fn align(v: &str) -> Option<Align> {
    Some(match v {
        "stretch" => Align::Stretch,
        "flex-start" | "start" => Align::Start,
        "flex-end" | "end" => Align::End,
        "center" => Align::Center,
        "baseline" => Align::Baseline,
        _ => return None,
    })
}

impl FlowStyleSource for InlineFlex<'_> {
    fn flow_style(&self, dom: &Dom, el: NodeId) -> Option<FlowStyle> {
        let mut s = self.style.flow_style(dom, el)?;
        let decl = |n: &str| self.style.inline_declared_value(dom, el, n).map(|(v, _)| v.trim().to_owned());
        if let Some(v) = decl("flex-direction") {
            s.flex_direction = match v.as_str() {
                "column" => FlexDirection::Column,
                "row-reverse" => FlexDirection::RowReverse,
                "column-reverse" => FlexDirection::ColumnReverse,
                _ => FlexDirection::Row,
            };
        }
        s.flex_wrap = decl("flex-wrap").as_deref() == Some("wrap");
        if let Some(g) = decl("gap").and_then(|v| flex_len(&v)) {
            s.row_gap = g;
            s.column_gap = g;
        }
        if let Some(v) = decl("align-items").and_then(|v| align(&v)) {
            s.align_items = v;
        }
        s.align_self = decl("align-self").and_then(|v| align(&v));
        if let Some(v) = decl("justify-content") {
            s.justify_content = match v.as_str() {
                "flex-end" | "end" => Justify::End,
                "center" => Justify::Center,
                "space-between" => Justify::SpaceBetween,
                "space-around" => Justify::SpaceAround,
                "space-evenly" => Justify::SpaceEvenly,
                _ => Justify::Start,
            };
        }
        if let Some(v) = decl("flex") {
            // `flex: <number>` = grow N, shrink 1, basis 0%.
            if let Ok(n) = v.parse::<f64>() {
                s.flex_grow = n;
                s.flex_shrink = 1.0;
                s.flex_basis = Dim::Len(Len::percent(0.0));
            }
        }
        if let Some(n) = decl("flex-grow").and_then(|v| v.parse().ok()) {
            s.flex_grow = n;
        }
        if let Some(n) = decl("flex-shrink").and_then(|v| v.parse().ok()) {
            s.flex_shrink = n;
        }
        // Tables: display parts from the tag (UA), border-spacing (UA 2px),
        // border-collapse and table-layout from inline declarations.
        let tag = dom.element(el).map(|e| e.local_name().to_string()).unwrap_or_default();
        if decl("display").is_none() {
            s.table_part = match tag.as_str() {
                "thead" => TablePart::HeaderGroup,
                "tbody" => TablePart::BodyGroup,
                "tfoot" => TablePart::FooterGroup,
                "tr" => TablePart::Row,
                "td" | "th" => TablePart::Cell,
                "caption" => TablePart::Caption,
                _ => TablePart::None,
            };
        }
        // Grid (Stage 4) from inline declarations.
        let len = |v: &str| flex_len(v);
        if let Some(v) = decl("grid-template-columns") {
            s.grid_template_columns = flow::parse_track_list(&v, &len);
        }
        if let Some(v) = decl("grid-template-rows") {
            s.grid_template_rows = flow::parse_track_list(&v, &len);
        }
        if let Some(t) = decl("grid-auto-rows").and_then(|v| flow::parse_track(&v, &len)) {
            s.grid_auto_rows = t;
        }
        let (gc, gr) = (decl("grid-column"), decl("grid-row"));
        s.grid_column = flow::parse_grid_placement(gc.as_deref(), None, None);
        s.grid_row = flow::parse_grid_placement(gr.as_deref(), None, None);
        if let Some(g) = decl("column-gap").and_then(|v| flex_len(&v)) {
            s.column_gap = g;
        }
        if let Some(g) = decl("row-gap").and_then(|v| flex_len(&v)) {
            s.row_gap = g;
        }
        if let Some(v) = decl("justify-items").and_then(|v| align(&v)) {
            s.justify_items = v;
        }
        s.justify_self = decl("justify-self").and_then(|v| align(&v));
        if let Some(v) = decl("text-align") {
            s.text_align = match v.as_str() {
                "center" => TextAlign::Center,
                "right" | "end" => TextAlign::End,
                _ => TextAlign::Start,
            };
        }
        if tag == "table" {
            let spacing = decl("border-spacing").and_then(|v| flex_len(&v)).map_or(2.0, |l| l.px);
            s.border_spacing = (spacing, spacing);
            s.border_collapse = decl("border-collapse").as_deref() == Some("collapse");
            s.table_layout_fixed = decl("table-layout").as_deref() == Some("fixed");
        }
        Some(s)
    }
}

/// Flow rects (document coordinates, no scroll) and client/scroll sizes.
fn flex_check(name: &str, html: &str, expected: &[(&str, [f64; 8])]) {
    let p = page(html, Geometry::Flow);
    let src = InlineFlex { style: p.layout.style() };
    let result = flow::layout(&p.dom, p.doc, &src, 800.0, 600.0);
    let mut diffs = Vec::new();
    for (id, want) in expected {
        let el = p.id(id);
        let b = result.get(el).unwrap_or_else(|| panic!("{name}: no box for #{id}"));
        let got = [
            b.rect.x,
            b.rect.y,
            b.rect.width,
            b.rect.height,
            b.client_width,
            b.client_height,
            b.scroll_width.max(b.client_width),
            b.scroll_height.max(b.client_height),
        ];
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
    let p = page(&html, Geometry::Flow);
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
    // Fractional widths: Chromium snaps client sizes to integers.
    let close = |name: &str, html: &str, expected: &[(&str, [f64; 4])]| {
        let p = page(html, Geometry::Flow);
        let src = InlineFlex { style: p.layout.style() };
        let result = flow::layout(&p.dom, p.doc, &src, 800.0, 600.0);
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
            ("a", [12.0, 12.0, 222.67, 30.0]),
            ("b", [250.67, 12.0, 461.33, 40.0]),
            ("c", [12.0, 60.0, 700.0, 20.0]),
            ("d", [12.0, 88.0, 222.67, 25.0]),
            ("p", [0.0, 125.0, 500.0, 12.0]),
            ("p1", [0.0, 125.0, 243.25, 10.0]),
            ("p2", [243.25, 125.0, 256.75, 12.0]),
        ],
    );
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
    let p = page(&html, Geometry::Flow);
    let src = InlineFlex { style: p.layout.style() };
    let result = flow::layout(&p.dom, p.doc, &src, 800.0, 600.0);
    let col = (800.0 - 84.0 - 7.0) / 7.0;
    for (id, want) in [
        ("g", [0.0, 0.0, 800.0, 383.0]),
        ("h0", [0.0, 0.0, 84.0, 20.0]),
        ("h1", [85.0, 0.0, col, 20.0]),
        ("w1", [0.0, 21.0, 84.0, 120.0]),
        ("c11", [85.0, 21.0, col, 24.0]),
        ("c17", [85.0 + 6.0 * (col + 1.0), 21.0, col, 120.0]),
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
fn flow_hit_test_prefers_sticky_header_cell_over_its_row_group() {
    // TOS contact-role: a sticky <th> inside a `position: relative` thead in
    // a positioned scroller. Chromium: elementFromPoint at the th is the th.
    let p = page("<!doctype html><body><div id=scroller style=\"position:relative;overflow:auto;height:200px\"><table style=\"border-collapse:separate;border-spacing:0;width:400px\"><thead id=head style=\"position:relative;z-index:10\"><tr><th id=name style=\"position:sticky;top:0;width:200px;height:30px;padding:0\">Name</th><th id=other style=\"position:sticky;top:0;height:30px;padding:0\">Other</th></tr></thead><tbody><tr><td id=cell style=\"height:40px\">x</td><td>y</td></tr></tbody></table></div></body>", Geometry::Flow);
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
    // Chromium 1.59.1, 800x600. Atomic boxes with explicit sizes; the inline
    // span's own height is font-metric dependent, so it is checked on x/width.
    let html = "<!doctype html><body style=\"margin:0\"><div id=c style=\"width:400px;text-align:center\"><span id=c1 style=\"display:inline-block;width:50px;height:20px\"></span><span id=c2 style=\"display:inline-block;width:30px;height:20px\"></span></div><div id=r style=\"width:400px;text-align:right\"><span id=r1 style=\"display:inline-block;width:50px;height:20px\"></span> </div><div id=w style=\"width:400px;text-align:center\"><span id=w1 style=\"display:inline-block;width:250px;height:10px\"></span><span id=w2 style=\"display:inline-block;width:250px;height:10px\"></span></div><div id=o style=\"width:400px;text-align:center\"><span id=o1 style=\"display:inline-block;width:500px;height:10px\"></span></div><div id=e style=\"width:588px;padding:0 32px;text-align:center\"><button id=btn style=\"display:inline-flex;width:127px;height:28px;padding:0;border:0\"></button></div><div id=s style=\"width:300px;text-align:end\"><span id=sp><span id=s1 style=\"display:inline-block;width:40px;height:10px\"></span></span></div></body>";
    let p = page(html, Geometry::Flow);
    let src = InlineFlex { style: p.layout.style() };
    let result = flow::layout(&p.dom, p.doc, &src, 800.0, 600.0);
    let mut diffs = Vec::new();
    for (id, want) in [
        ("c", [0.0, 0.0, 400.0, 24.0]),
        ("c1", [160.0, 0.0, 50.0, 20.0]),
        ("c2", [210.0, 0.0, 30.0, 20.0]),
        ("r", [0.0, 24.0, 400.0, 24.0]),
        ("r1", [350.0, 24.0, 50.0, 20.0]),
        ("w", [0.0, 48.0, 400.0, 36.0]),
        ("w1", [75.0, 52.0, 250.0, 10.0]),
        ("w2", [75.0, 70.0, 250.0, 10.0]),
        ("o", [0.0, 84.0, 400.0, 18.0]),
        ("o1", [0.0, 88.0, 500.0, 10.0]),
        ("e", [0.0, 102.0, 652.0, 32.0]),
        ("btn", [262.5, 102.0, 127.0, 28.0]),
        ("s", [0.0, 134.0, 300.0, 18.0]),
        ("s1", [260.0, 138.0, 40.0, 10.0]),
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
fn text_align_defaults_to_start() {
    assert_eq!(FlowStyle::default().text_align, TextAlign::Start);
}
