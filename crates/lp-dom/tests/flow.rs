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

use lp_dom::flow::{
    self, Align, ContentSize, Dim, FlexDirection, FlowStyle, FlowStyleSource, Justify, Len, TablePart, TextAlign,
};

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
        // Stand-in for a loaded image's natural size (W3's Dom::natural_size).
        if let Some((w, h)) = dom
            .get_attribute(el, "data-natural")
            .ok()
            .flatten()
            .and_then(|v| v.split_once(',').map(|(w, h)| (w.trim().parse().ok(), h.trim().parse().ok())))
            .and_then(|(w, h)| Some((w?, h?)))
        {
            s.intrinsic = Some((w, h));
        }
        match decl("width").as_deref() {
            Some("fit-content") => s.width = Dim::Content(ContentSize::Fit),
            Some("max-content") => s.width = Dim::Content(ContentSize::Max),
            Some("min-content") => s.width = Dim::Content(ContentSize::Min),
            _ => {}
        }
        if let Some(w) = decl("font-weight").and_then(|v| v.parse::<u16>().ok()) {
            s.font_weight = w;
        }
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
    // Chromium 1.59.1, 800x600, system UI font. Atomic boxes with explicit sizes; the inline
    // span's own height is font-metric dependent, so it is checked on x/width.
    let html = "<!doctype html><body style=\"margin:0;font-family:-apple-system,BlinkMacSystemFont,sans-serif\"><div id=c style=\"width:400px;text-align:center\"><span id=c1 style=\"display:inline-block;width:50px;height:20px\"></span><span id=c2 style=\"display:inline-block;width:30px;height:20px\"></span></div><div id=r style=\"width:400px;text-align:right\"><span id=r1 style=\"display:inline-block;width:50px;height:20px\"></span> </div><div id=w style=\"width:400px;text-align:center\"><span id=w1 style=\"display:inline-block;width:250px;height:10px\"></span><span id=w2 style=\"display:inline-block;width:250px;height:10px\"></span></div><div id=o style=\"width:400px;text-align:center\"><span id=o1 style=\"display:inline-block;width:500px;height:10px\"></span></div><div id=e style=\"width:588px;padding:0 32px;text-align:center\"><button id=btn style=\"display:inline-flex;width:127px;height:28px;padding:0;border:0\"></button></div><div id=s style=\"width:300px;text-align:end\"><span id=sp><span id=s1 style=\"display:inline-block;width:40px;height:10px\"></span></span></div></body>";
    let p = page(html, Geometry::Flow);
    let src = InlineFlex { style: p.layout.style() };
    let result = flow::layout(&p.dom, p.doc, &src, 800.0, 600.0);
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
fn text_align_defaults_to_start() {
    assert_eq!(FlowStyle::default().text_align, TextAlign::Start);
}

#[test]
fn flow_pass_memo_tracks_mutations_between_passes() {
    // Custom properties and em font sizes come from ancestors; the layout
    // pass memoizes them, so a later pass must see DOM/style changes.
    let html = "<!doctype html><style>.big{font-size:20px} .wide{--w:300px}</style><body style=\"margin:0\"><div id=outer style=\"--w:100px\"><div id=mid><div id=box style=\"width:var(--w);height:2em\"></div></div></div></body>";
    let mut p = page(html, Geometry::Flow);
    let r = p.layout.bounding_client_rect(&p.dom, p.id("box"));
    assert_eq!((r.width, r.height), (100.0, 32.0), "first pass");
    // Override the variable on a nearer ancestor and change the inherited font size.
    let mid = p.id("mid");
    p.dom.set_attribute(mid, "class", "wide big").unwrap();
    let r = p.layout.bounding_client_rect(&p.dom, p.id("box"));
    assert_eq!((r.width, r.height), (300.0, 40.0), "after class change");
    // Inline style on the outer element no longer applies below the override.
    let outer = p.id("outer");
    p.dom.set_attribute(outer, "style", "--w:50px").unwrap();
    p.dom.set_attribute(mid, "class", "").unwrap();
    let r = p.layout.bounding_client_rect(&p.dom, p.id("box"));
    assert_eq!((r.width, r.height), (50.0, 32.0), "after style and class change");
    // Same answer as a fresh layout of the mutated document.
    let fresh = lp_dom::flow::layout(&p.dom, p.doc, p.layout.style(), 800.0, 600.0);
    let b = fresh.get(p.id("box")).unwrap().rect;
    assert_eq!((b.width, b.height), (50.0, 32.0), "fresh layout");
    // getComputedStyle-side lookups (outside a pass) are unaffected.
    assert_eq!(p.layout.style().computed_value(&p.dom, p.id("box"), "width").as_deref(), Some("50px"));
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
    let p = page(html, Geometry::Flow);
    let nb = p.metrics("nb");
    assert!(nb[2] > 0.0 && nb[3] > 0.0, "nbsp span has a box: {nb:?}");
    let space = p.metrics("nb")[2];
    // Chromium (system UI font, 16px): a no-break space is 4.19px wide.
    assert!((space - 4.1872).abs() < 0.01, "as wide as a space: {space}");
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
    let p = page(html, Geometry::Flow);
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
fn track_min_max_functions_parse_and_resolve() {
    let len = |v: &str| match v.trim() {
        "100%" => Some(Len::percent(100.0)),
        "200px" => Some(Len::px(200.0)),
        _ => None,
    };
    let t = flow::parse_track("minmax(min(100%,200px),1fr)", &len).unwrap();
    assert_eq!(t.min, flow::TrackBreadth::Min(Len::percent(100.0), Len::px(200.0)));
    assert_eq!(t.min.length(Some(150.0)), Some(150.0));
    assert_eq!(t.min.length(Some(700.0)), Some(200.0));
    assert_eq!(t.min.length(None), None, "percentages need a definite size");
    let t = flow::parse_track("max(100%, 200px)", &len).unwrap();
    assert_eq!(t.max.length(Some(150.0)), Some(200.0));
}

#[test]
fn flow_parity_system_font_text_widths() {
    // Text is measured with the system UI font's advances (SF Pro on
    // macOS Chromium) by size and weight. Chromium 1.59.1 widths; the
    // per-glyph sum ignores kerning, so widths match within 1.5px while line
    // breaking and heights match exactly.
    let html = "<!doctype html><body style=\"margin:0;font-family:-apple-system,BlinkMacSystemFont,sans-serif;font-size:14px;line-height:20px\"><span id=a style=\"display:inline-block\">Payment confirmations</span><br><span id=b style=\"display:inline-block;font-weight:600\">Add Working Document</span><br><span id=c style=\"display:inline-block;font-size:12px\">Integration Only</span><br><div id=w style=\"width:120px\">Persist filters and more words here</div><div id=f style=\"display:flex;gap:8px\"><span id=f1>Persist filters</span><span id=f2 style=\"font-weight:600\">Save</span></div></body>";
    let p = page(html, Geometry::Flow);
    let src = InlineFlex { style: p.layout.style() };
    let result = flow::layout(&p.dom, p.doc, &src, 800.0, 600.0);
    let rect = |id: &str| result.get(p.id(id)).unwrap().rect;
    let mut diffs = Vec::new();
    for (id, w) in [("a", 148.38), ("b", 159.61), ("c", 90.45), ("f1", 84.28), ("f2", 32.41)] {
        let r = rect(id);
        if (r.width - w).abs() > 1.5 {
            diffs.push(format!("#{id}: width {} vs Chromium {w}", r.width));
        }
    }
    // Old fixed-advance model: "Payment confirmations" was 172.6px.
    assert!(diffs.is_empty(), "text widths:\n{}", diffs.join("\n"));
    assert_eq!(rect("w").height, 40.0, "two lines at 120px, as in Chromium");
    assert!((rect("f2").x - 92.28).abs() < 1.5, "flex item after measured text: {}", rect("f2").x);
    // A flex item sized to its max-content width keeps its text on one line
    // (no wrap from floating-point summation order).
    assert_eq!(rect("f1").height, 20.0, "max-content sized text stays on one line");
}

#[test]
fn flow_parity_intrinsic_width_keywords() {
    // TOS `w-fit min-w-full` table wrapper: fit-content, clamped by
    // min-width:100%, takes the table's max-content width. Chromium 1.59.1;
    // x/width only (line heights depend on font metrics).
    let html = "<!doctype html><body style=\"margin:0\"><div id=fit style=\"width:fit-content\"><span style=\"display:inline-block;width:120px;height:10px\"></span><span style=\"display:inline-block;width:80px;height:10px\"></span></div><div id=fitmin style=\"width:fit-content;min-width:100%\"><div style=\"width:1200px;height:10px\"></div></div><div id=max style=\"width:max-content\"><span style=\"display:inline-block;width:500px;height:10px\"></span><span style=\"display:inline-block;width:500px;height:10px\"></span></div><div id=min style=\"width:min-content\"><span style=\"display:inline-block;width:60px;height:10px\"></span><span style=\"display:inline-block;width:90px;height:10px\"></span></div><div style=\"position:relative;height:40px\"><div id=abs style=\"position:absolute;width:max-content;left:700px\"><span style=\"display:inline-block;width:150px;height:10px\"></span><span style=\"display:inline-block;width:150px;height:10px\"></span></div></div></body>";
    let p = page(html, Geometry::Flow);
    let src = InlineFlex { style: p.layout.style() };
    let result = flow::layout(&p.dom, p.doc, &src, 800.0, 600.0);
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
    let p = page(html, Geometry::Flow);
    let src = InlineFlex { style: p.layout.style() };
    let result = flow::layout(&p.dom, p.doc, &src, 800.0, 600.0);
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
fn flow_parity_text_rects() {
    // Range geometry over text: Chromium reports the text's content-area box
    // (font ascent + descent, after the top half-leading), even inside a
    // `display: contents` span that has no box of its own. TOS buttons wrap
    // their label in such a span, and Playwright's accessible-name visibility
    // check measures that text with Range.getBoundingClientRect.
    // Chromium 1.59.1, system UI font; widths within 1.5px (no kerning).
    let html = "<!doctype html><body style=\"margin:0;font-family:-apple-system,BlinkMacSystemFont,sans-serif;font-size:14px;line-height:20px\"><button id=btn style=\"display:inline-flex;padding:4px 8px;border:0;font:inherit\"><span id=c style=\"display:contents\">Files &amp; links</span></button><div id=d style=\"padding-left:10px\">Hello world</div></body>";
    let p = page(html, Geometry::Flow);
    let span = p.metrics("c");
    assert_eq!(&span[..4], &[0.0, 0.0, 0.0, 0.0], "display:contents has no box");
    let text = |id: &str| {
        let el = p.id(id);
        let t = p.dom.children(el).unwrap().next().unwrap();
        p.layout.text_rect(&p.dom, t).unwrap_or_else(|| panic!("no text rect in #{id}"))
    };
    for (id, want) in [("c", [8.0, 5.0, 76.05, 17.0]), ("d", [10.0, 29.0, 72.34, 17.0])] {
        let r = text(id);
        let ok = (r.x - want[0]).abs() < 0.01
            && (r.y - want[1]).abs() < 0.01
            && (r.width - want[2]).abs() < 1.5
            && (r.height - want[3]).abs() < 0.01;
        assert!(ok, "#{id} text: got {:?}, Chromium {want:?}", (r.x, r.y, r.width, r.height));
    }
    // Default geometry has no text boxes (Range keeps its element fallback).
    let q = page(html, Geometry::Approximate);
    let t = q.dom.children(q.id("d")).unwrap().next().unwrap();
    assert!(q.layout.text_rect(&q.dom, t).is_none());
}

#[test]
fn flow_skips_relayout_for_identical_writes_but_not_real_changes() {
    // Writing an attribute or text to its current value is still a mutation
    // (records are queued) but changes nothing layout depends on: the flow
    // result is reused. Real changes must relayout with fresh geometry.
    let html = "<!doctype html><style>.w{width:100px}.x{width:300px}</style><body style=\"margin:0\"><div id=a class=w style=\"height:10px\"></div><span id=t>abc</span></body>";
    let mut p = page(html, Geometry::Flow);
    let a = p.id("a");
    let first = p.layout.flow(&p.dom);
    assert_eq!(first.get(a).unwrap().rect.width, 100.0);
    let v = p.dom.version();
    p.dom.set_attribute(a, "class", "w").unwrap();
    p.dom.set_attribute(a, "style", "height:10px").unwrap();
    let text = p.dom.children(p.id("t")).unwrap().next().unwrap();
    p.dom.set_char_data(text, "abc").unwrap();
    assert_eq!(p.dom.version(), v, "identical writes keep the version");
    assert!(Rc::ptr_eq(&first, &p.layout.flow(&p.dom)), "flow result reused");
    // Real attribute change: fresh geometry, never the cached 100px.
    p.dom.set_attribute(a, "class", "x").unwrap();
    assert_ne!(p.dom.version(), v);
    assert_eq!(p.layout.flow(&p.dom).get(a).unwrap().rect.width, 300.0);
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
    let p = page(html, Geometry::Flow);
    let src = InlineFlex { style: p.layout.style() };
    let result = flow::layout(&p.dom, p.doc, &src, 800.0, 600.0);
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
    let cached = p.layout.flow(&p.dom);
    let mut fresh = Layout::with_viewport(p.doc, Viewport::new(800, 600));
    fresh.style_mut().discover_style_elements(&p.dom);
    fresh.set_geometry(Geometry::Flow);
    let f = fresh.flow(&p.dom);
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
    let mut p = page(&html, Geometry::Flow);
    assert_matches_fresh(&p, "initial");
    let id = |p: &Page, s: &str| p.id(s);
    // Descendant combinator: ancestor attribute.
    let par = id(&p, "par");
    p.dom.set_attribute(par, "class", "on").unwrap();
    assert_matches_fresh(&p, "ancestor class");
    assert_eq!(p.layout.flow(&p.dom).get(id(&p, "child")).unwrap().rect.width, 200.0);
    // Sibling combinator.
    let a = id(&p, "a");
    p.dom.set_attribute(a, "class", "a on").unwrap();
    assert_matches_fresh(&p, "previous sibling class");
    assert_eq!(p.layout.flow(&p.dom).get(id(&p, "b")).unwrap().rect.width, 300.0);
    // :has() anchor: a descendant's attribute restyles a sibling subtree.
    let inp = id(&p, "inp");
    p.dom.set_attribute(inp, "disabled", "").unwrap();
    assert_matches_fresh(&p, "group-has descendant attribute");
    assert_eq!(p.layout.flow(&p.dom).get(id(&p, "icon")).unwrap().rect.width, 150.0);
    // :nth-child after an insertion before the first item.
    let ul = id(&p, "ul");
    let new_li = p.dom.create_element(p.doc, "li").unwrap();
    let l1 = id(&p, "l1");
    p.dom.insert_before(ul, new_li, Some(l1)).unwrap();
    assert_matches_fresh(&p, "insertion shifts nth-child");
    assert_eq!(p.layout.flow(&p.dom).get(l1).unwrap().rect.width, 222.0);
    // Removal, then reinsertion under a different context.
    let (home, mv, ctx) = (id(&p, "home"), id(&p, "mv"), id(&p, "ctx"));
    p.dom.remove_child(home, mv).unwrap();
    assert_matches_fresh(&p, "removal");
    p.dom.append_child(ctx, mv).unwrap();
    assert_matches_fresh(&p, "reinsertion elsewhere");
    assert_eq!(p.layout.flow(&p.dom).get(mv).unwrap().rect.width, 333.0);
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
    assert_eq!(p.layout.flow(&p.dom).get(id(&p, "emc")).unwrap().rect.width, 60.0);
    // Form-control state (:checked) feeding a sibling combinator.
    let cb = id(&p, "cb");
    p.dom.set_checked(cb, true).unwrap();
    assert_matches_fresh(&p, "checked state");
    assert_eq!(p.layout.flow(&p.dom).get(id(&p, "lab")).unwrap().rect.width, 250.0);
    // Journal overflow falls back to full invalidation.
    for i in 0..lp_dom::dom::JOURNAL_CAPACITY + 100 {
        p.dom.set_attribute(par, "data-n", &i.to_string()).unwrap();
    }
    p.dom.set_attribute(par, "class", "").unwrap();
    assert_matches_fresh(&p, "after journal overflow");
    assert_eq!(p.layout.flow(&p.dom).get(id(&p, "child")).unwrap().rect.width, 100.0);
}

#[test]
fn loaded_image_natural_size_reaches_flow_through_the_dom() {
    // TOS chat: <img class="max-h-64 w-full object-contain"> in a 240px
    // button, holding the uploaded 1x1 PNG. Unloaded it is 240x0 (hidden
    // to Playwright); once the embedder records the natural size it is
    // 240x240 as in Chromium, and a later size change relayouts.
    let mut p = page(
        "<body style='margin:0'><div style='width:240px'><button style='display:block;width:100%;padding:0;border:0'><span style='display:contents'><img id=i style='max-height:256px;width:100%'></span></button></div></body>",
        Geometry::Flow,
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
    let mut p = page(&html, Geometry::Flow);
    assert_matches_fresh(&p, "initial");
    let w = |p: &Page, id: &str| p.layout.flow(&p.dom).get(p.id(id)).unwrap().rect;
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
    let mut p = page(&html, Geometry::Flow);
    assert_matches_fresh(&p, "initial");
    let w = |p: &Page, id: &str| p.layout.flow(&p.dom).get(p.id(id)).unwrap().rect;
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
    assert_eq!(p.layout.flow(&p.dom).get(k).unwrap().rect.width, 250.0);

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
        Geometry::Flow,
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
    let mut p = page(&html, Geometry::Flow);
    assert_matches_fresh(&p, "initial");
    let w = |p: &Page, id: &str| p.layout.flow(&p.dom).get(p.id(id)).unwrap().rect;
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
    let mut p = page(
        "<body style='margin:0'><div id=a class=x><span id=s>one</span></div><div id=b>two</div></body>",
        Geometry::Flow,
    );
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
    let fresh_layout =
        page("<body style='margin:0'><div id=a class=x></div><div id=b>two<i></i></div></body>", Geometry::Flow);
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
    let mut p = page(&html, Geometry::Flow);
    assert_matches_fresh(&p, "initial");
    let w = |p: &Page, id: &str| p.layout.flow(&p.dom).get(p.id(id)).unwrap().rect;
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
    p.dom.append_child(slot, holder).unwrap();
    assert_matches_fresh(&p, "focused subtree moved to w2");
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
    let mut p = page(&html, Geometry::Flow);
    assert_matches_fresh(&p, "initial");
    let w = |p: &Page, id: &str| p.layout.flow(&p.dom).get(p.id(id)).unwrap().rect;
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
    let mut p = page(&html, Geometry::Flow);
    assert_matches_fresh(&p, "initial");
    let w = |p: &Page, id: &str| p.layout.flow(&p.dom).get(p.id(id)).unwrap().rect;
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
    assert_eq!(p.layout.flow(&p.dom).get(portal).unwrap().rect.width, 200.0);
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
    let mut p = page(&html, Geometry::Flow);
    assert_matches_fresh(&p, "initial");
    let w = |p: &Page, id: &str| p.layout.flow(&p.dom).get(p.id(id)).unwrap().rect;
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
    let p = page(&html, Geometry::Flow);
    let wid = |id: &str| p.metrics(id)[2];
    let plain = wid("plain");
    assert!(plain > 50.0, "{plain}");
    assert_eq!(wid("pdi"), plain, "U+2069 adds nothing");
    assert_eq!(wid("iso"), wid("nbsp"), "isolates around both segments add nothing");
    assert_eq!(wid("zw"), plain, "ZWSP, LRM, BOM and soft hyphen add nothing");
    assert_eq!(wid("after"), wid("y"), "an isolate before normal text");
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
         </body>",
        Geometry::Flow,
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
    let mut p = page(&html, Geometry::Flow);
    assert_matches_fresh(&p, "initial");
    let w = |p: &Page, id: &str| p.layout.flow(&p.dom).get(p.id(id)).unwrap().rect;
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
    let p = page(&html, Geometry::Flow);
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
    let p = page(&html, Geometry::Flow);
    // One line is the single-line span's height; each further line adds the
    // line pitch (the step from 2 to 3 lines).
    let one = p.metrics("s-inherit")[3];
    let pitch = p.metrics("s-hs")[3] - p.metrics("s-pkg")[3];
    assert!(pitch >= one, "line pitch {pitch} vs line {one}");
    for (k, lines) in [("pkg", 2.0), ("hs", 3.0)] {
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
    let p = page(&html, Geometry::Flow);
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
    let mut p = page("<!doctype html><body><div id=a></div></body>", Geometry::Flow);
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
    let mut p = page(&html, Geometry::Flow);
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
    let p = page(&html, Geometry::Flow);
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
fn style_sharing_serves_each_elements_own_cascade() {
    // Equivalent new elements share a computed flow style (up to 4 previous
    // siblings, and the same position under a parent that shared: React
    // rows). Each table isolates one way two such elements can still differ;
    // every served style must equal the element's own cascade.
    let css = "<style>\
        td{height:12px} span,b{display:block;height:2px}\
        .t1 tr:nth-child(odd) td{padding-left:3px} .t1 tbody>tr:last-child>td:first-child{width:77px} .t1 td:empty{height:5px}\
        .t2 [data-state=open] span{font-size:20px}\
        .t3 .group:hover b{width:55px} .t3 .group:focus-within span{width:66px}\
        .t4 tr[data-hot] td{height:30px}\
        .t5 td:has(.flag){height:21px} .t5 .a+.b{width:31px} .t5 .a~.c{width:41px}\
        </style>";
    let table = |class: &str, rows: &dyn Fn(usize) -> String| {
        let body: String = (0..8).map(rows).collect();
        format!("<table class={class}><tbody>{body}</tbody></table>")
    };
    let plain =
        |_: usize| "<tr class=group><td><span>v</span></td><td><b></b><span><input></span></td></tr>".to_string();
    let t1 = table("t1", &|i| {
        if i == 5 {
            "<tr class=group><td><span>v</span></td><td></td></tr>".into()
        } else {
            plain(i)
        }
    });
    let t2 = table("t2", &|i| {
        let state = if i == 4 { "open" } else { "closed" };
        format!("<tr class=group data-state={state} data-unread={i}><td><span>v</span></td><td><b></b><span>w</span></td></tr>")
    });
    let t3 = table("t3", &plain);
    let t4 = table("t4", &plain);
    let t5 = table("t5", &|i| {
        let flag = if i == 3 { "<i class=flag></i>" } else { "" };
        format!("<tr class=group><td>{flag}<span>v</span></td><td><span class=a></span><span class=b></span><span class=c></span></td></tr>")
    });
    let html = format!("<!doctype html>{css}<body style=\"margin:0\">{t1}{t2}{t3}{t4}{t5}</body>");
    let mut p = page(&html, Geometry::Flow);
    let elements = |p: &Page, root: NodeId| -> Vec<NodeId> {
        p.dom
            .descendants(root)
            .unwrap()
            .filter(|&n| p.dom.node_type(n).ok() == Some(lp_dom::node::NodeType::Element))
            .collect()
    };
    let check = |p: &Page, step: &str| {
        use lp_dom::flow::FlowStyleSource;
        let _ = p.layout.flow(&p.dom);
        for e in elements(p, p.doc) {
            let served = p.layout.style().flow_style(&p.dom, e);
            let own = p.layout.style().flow_style_unshared(&p.dom, e);
            assert_eq!(served, own, "{step}: <{}> served a style not its own", p.dom.tag_name(e).unwrap());
        }
        assert_matches_fresh(p, step);
    };
    check(&p, "initial");
    let tables: Vec<NodeId> =
        elements(&p, p.doc).into_iter().filter(|&n| p.dom.tag_name(n).ok().as_deref() == Some("TABLE")).collect();
    let rows = |p: &Page, t: NodeId| -> Vec<NodeId> {
        elements(p, t).into_iter().filter(|&n| p.dom.tag_name(n).ok().as_deref() == Some("TR")).collect()
    };
    let first = |p: &Page, root: NodeId, tag: &str| {
        elements(p, root).into_iter().find(|&n| p.dom.tag_name(n).ok().as_deref() == Some(tag)).unwrap()
    };
    // t3: hover inside one row, focus inside the next.
    let r3 = rows(&p, tables[2]);
    let (b, input) = (first(&p, r3[2], "B"), first(&p, r3[3], "INPUT"));
    p.dom.set_hovered(p.doc, Some(b)).unwrap();
    p.dom.set_focused(p.doc, Some(input)).unwrap();
    check(&p, "hover and focus");
    // t4: the source row restyled after the next shared from it; then a cell
    // of that next row restyles alone and must not share with the stale
    // source's cell.
    let r4 = rows(&p, tables[3]);
    // (An attribute that restyles only the row, unlike `class` here,
    // which sibling rules make restyle later rows too.)
    p.dom.set_attribute(r4[0], "data-hot", "").unwrap();
    check(&p, "source row restyled");
    let cell = first(&p, r4[1], "TD");
    p.dom.set_attribute(cell, "title", "z").unwrap();
    let _ = p.layout.flow(&p.dom);
    p.dom.remove_attribute(cell, "title").unwrap();
    check(&p, "cell of a sharing row restyled alone");
    // New rows appended to t1 (React commit) share with existing ones.
    let tb1 = p.dom.parent_element(rows(&p, tables[0])[0]).unwrap().unwrap();
    for _ in 0..3 {
        let frag = p.dom.create_element(p.doc, "tbody").unwrap();
        lp_dom::parse::set_inner_html(&mut p.dom, frag, &plain(0)).unwrap();
        let tr = p.dom.children(frag).unwrap().next().unwrap();
        p.dom.append_child(tb1, tr).unwrap();
    }
    check(&p, "rows appended");
}

#[test]
fn cross_pass_measurements_never_serve_stale_geometry() {
    // Layout keeps subtree measurements (content heights, intrinsic widths,
    // baselines) across passes and drops those of changed elements and
    // their ancestors. Measured layouts (wrapping flex rows, grid auto rows,
    // shrink-to-fit abspos, inline-blocks, auto tables, images, selects)
    // must match a fresh layout after each kind of change, deep inside.
    let css = "<style>\
        .row{display:flex;flex-wrap:wrap;gap:4px;width:300px} .row>div{padding:2px}\
        .grid{display:grid;grid-template-columns:1fr 1fr;width:300px}\
        .pos{position:relative;height:200px} .abs{position:absolute;top:0;left:0}\
        .ib{display:inline-block} .big{font-size:30px} table{border-collapse:collapse}\
        .wide .w{width:250px}\
        </style>";
    let html = format!(
        "<!doctype html>{css}<body style=\"margin:0\">\
         <div class=row id=row><div><span id=t1>alpha beta</span></div><div><div><b id=t2>gamma</b></div></div><div id=cnt></div></div>\
         <div class=grid id=grid><div><p id=t3>one</p></div><div id=gcell><span class=w id=w1>two</span></div></div>\
         <div class=pos><div class=abs id=abs><div><span id=t4>shrink to fit</span></div></div></div>\
         <p id=par>text <span class=ib id=ib><span id=t5>inline block</span></span> after</p>\
         <table><tr><td id=td1><span id=t6>cell</span></td><td>x</td></tr></table>\
         <div class=row><div><img id=img></div><div><select id=sel><option id=opt>short</option></select></div></div>\
         </body>"
    );
    let mut p = page(&html, Geometry::Flow);
    let check = |p: &Page, step: &str| {
        let cached = p.layout.flow(&p.dom);
        let mut fresh = Layout::with_viewport(p.doc, p.layout.viewport().clone());
        fresh.style_mut().discover_style_elements(&p.dom);
        fresh.set_geometry(Geometry::Flow);
        let f = fresh.flow(&p.dom);
        let stale: Vec<_> = f
            .boxes
            .iter()
            .filter(|(k, b)| cached.boxes.get(k).map(|c| c.rect) != Some(b.rect))
            .map(|(k, _)| *k)
            .collect();
        assert!(stale.is_empty() && cached.boxes.len() == f.boxes.len(), "{step}: stale geometry for {stale:?}");
    };
    check(&p, "initial");
    let text = |p: &Page, id: &str| p.dom.children(p.id(id)).unwrap().next().unwrap();
    // Text deep inside measured boxes.
    for (id, new) in [
        ("t1", "alpha beta gamma delta epsilon zeta"),
        ("t2", "a much longer gamma"),
        ("t4", "x"),
        ("t6", "a wider cell text"),
    ] {
        let t = text(&p, id);
        let len = p.dom.char_data(t).unwrap().len() as u32;
        p.dom.replace_data(t, 0, len, new).unwrap();
        check(&p, &format!("text {id}"));
    }
    // Inherited style from an ancestor (descendants restyle, not journaled).
    let par = p.id("par");
    p.dom.set_attribute(par, "class", "big").unwrap();
    check(&p, "ancestor font-size");
    let grid = p.id("grid");
    p.dom.set_attribute(grid, "class", "grid wide").unwrap();
    check(&p, "ancestor class restyles a descendant");
    // Inline style on a deep element, insertion and removal.
    let t3 = p.id("t3");
    p.dom.set_attribute(t3, "style", "height:80px").unwrap();
    check(&p, "inline style");
    let cnt = p.id("cnt");
    for i in 0..3 {
        let d = p.dom.create_element(p.doc, "div").unwrap();
        let t = p.dom.create_text(p.doc, &format!("inserted {i} with words")).unwrap();
        p.dom.append_child(d, t).unwrap();
        p.dom.append_child(cnt, d).unwrap();
    }
    check(&p, "insertion");
    let first = p.dom.children(cnt).unwrap().next().unwrap();
    p.dom.remove_child(cnt, first).unwrap();
    check(&p, "removal");
    // Image natural size and select option text.
    let img = p.id("img");
    p.dom.set_natural_size(img, Some((120.0, 40.0)));
    check(&p, "image natural size");
    let opt = text(&p, "opt");
    p.dom.replace_data(opt, 0, 5, "a much longer option label").unwrap();
    check(&p, "option text");
    let sel = p.id("sel");
    p.dom.set_value(sel, "a much longer option label").unwrap();
    check(&p, "form value");
    // Stylesheet and viewport.
    let style = p.dom.create_element(p.doc, "style").unwrap();
    let rule = p.dom.create_text(p.doc, ".ib{padding:10px}").unwrap();
    p.dom.append_child(style, rule).unwrap();
    let body = p.dom.body(p.doc).unwrap().unwrap();
    p.dom.append_child(body, style).unwrap();
    p.layout.style_mut().discover_style_elements(&p.dom);
    check(&p, "stylesheet added");
    p.layout.set_viewport(Viewport::new(500, 400));
    check(&p, "viewport");
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
         </body>",
        Geometry::Flow,
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
        let mut p = page(&html, Geometry::Flow);
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
        Geometry::Flow,
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
    let mut p = page(&html, Geometry::Flow);
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
    let mut p = page(&html, Geometry::Flow);
    assert_matches_fresh(&p, "initial");
    let h = |p: &Page, id: &str| p.layout.flow(&p.dom).get(p.id(id)).unwrap().rect.height;
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
        let mut p = page(&format!("<!doctype html><body style=\"margin:0\">{body}</body>"), Geometry::Flow);
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
        "<!doctype html><body style=\"margin:0\"><div style=\"position:fixed;top:0;left:0;width:300px;height:200px\"><div style=\"height:1000px\"><div id=t style=\"position:sticky;top:10px;height:20px;margin-top:50px\">S</div></div></div></body>",
        Geometry::Flow,
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
        let p = page(&html, Geometry::Flow);
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
        let p = page(&html, Geometry::Flow);
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
    let p = page(&html, Geometry::Flow);
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
