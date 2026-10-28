//! In-crate benchmark for flow layout passes on an app-shaped page (ignored by
//! default): a sticky header, a sidebar and a table of N rows with flex cells,
//! badges, icon buttons, checkboxes and inputs, styled by Tailwind-like
//! utility rules. Prints the first pass (style + layout) and the relayout
//! after typical single-element changes an E2E step makes. No speed assertion.
//! Run: cargo test --release -p lp-dom --test integration -- flow_bench:: --ignored --nocapture
//! FLOW_BENCH_ROWS=200,1000 picks the row counts.
use lp_dom::layout::{Geometry, Layout};
use lp_dom::media::Viewport;
use lp_dom::parse::{parse_document, DomRef};
use lp_dom::{Dom, NodeId};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

const CSS: &str = "
*,::before,::after{box-sizing:border-box;border:0 solid}
body{margin:0;font-family:sans-serif;font-size:14px;line-height:20px}
.flex{display:flex}.inline-flex{display:inline-flex}.grid{display:grid}.hidden{display:none}.block{display:block}
.flex-col{flex-direction:column}.flex-1{flex:1 1 0%}.shrink-0{flex-shrink:0}.items-center{align-items:center}
.justify-between{justify-content:space-between}.justify-end{justify-content:flex-end}.gap-2{gap:8px}.gap-4{gap:16px}
.p-2{padding:8px}.p-4{padding:16px}.px-3{padding-left:12px;padding-right:12px}.py-2{padding-top:8px;padding-bottom:8px}
.px-2{padding-left:8px;padding-right:8px}.py-1{padding-top:4px;padding-bottom:4px}.mt-2{margin-top:8px}.mb-4{margin-bottom:16px}
.w-64{width:256px}.w-full{width:100%}.h-14{height:56px}.h-screen{height:100vh}.h-4{height:16px}.w-4{width:16px}.h-8{height:32px}
.min-w-0{min-width:0}.overflow-auto{overflow:auto}.overflow-hidden{overflow:hidden}.truncate{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.sticky{position:sticky}.top-0{top:0}.relative{position:relative}.absolute{position:absolute}.right-0{right:0}.z-10{z-index:10}
.border{border-width:1px}.border-b{border-bottom-width:1px}.rounded{border-radius:4px}.rounded-full{border-radius:9999px}
.text-xs{font-size:12px;line-height:16px}.text-sm{font-size:14px;line-height:20px}.text-lg{font-size:18px;line-height:28px}
.font-medium{font-weight:500}.font-semibold{font-weight:600}.text-right{text-align:right}.text-left{text-align:left}
.bg-white{background:#fff}.bg-gray-50{background:#f9fafb}.text-gray-500{color:#6b7280}.text-green-700{color:#15803d}
.bg-green-50{background:#f0fdf4}.text-red-700{color:#b91c1c}.bg-red-50{background:#fef2f2}.selected{background:#eef}
table{border-collapse:collapse;width:100%}th,td{padding:8px 12px;text-align:left}
tr:hover{background:#f3f4f6}.btn:hover{background:#e5e7eb}.row-actions button:focus{outline:2px solid blue}
";

fn row(i: usize) -> String {
    let side = if i % 3 == 0 { "Buy" } else { "Sell" };
    let badge = if i % 3 == 0 { "bg-green-50 text-green-700" } else { "bg-red-50 text-red-700" };
    format!(
        "<tr class=border-b id=r{i}>\
<td><input type=checkbox aria-label=\"select {i}\"></td>\
<td><div class=\"flex items-center gap-2 min-w-0\"><svg class=\"h-4 w-4 shrink-0\" viewBox=\"0 0 16 16\"><path d=\"M0 0h16v16H0z\"/></svg>\
<div class=\"flex flex-col min-w-0\"><span class=\"truncate font-medium\" id=n{i}>Instrument {i} Holdings Ltd</span>\
<span class=\"text-xs text-gray-500\">ISIN XS{i:010}</span></div></div></td>\
<td><span class=\"inline-flex items-center rounded-full px-2 text-xs font-medium {badge}\">{side}</span></td>\
<td class=text-right>{q}.00</td><td class=text-right>{p}.{c:02}</td>\
<td><input class=\"border rounded px-2 h-8 w-full\" value=\"{q}\" id=q{i}></td>\
<td><div class=\"row-actions flex justify-end gap-2\"><button class=\"btn inline-flex items-center rounded px-2 py-1 border\">Edit</button>\
<button class=\"btn inline-flex items-center rounded px-2 py-1 border\" id=c{i}><svg class=\"h-4 w-4\" viewBox=\"0 0 16 16\"></svg>Close</button></div></td>\
</tr>",
        q = 100 + i * 7 % 900,
        p = 10 + i % 90,
        c = i % 100,
    )
}

fn doc(rows: usize) -> String {
    let mut s = format!("<!doctype html><html><head><style>{CSS}</style></head><body>");
    s.push_str("<div class=\"flex h-screen\"><nav class=\"w-64 shrink-0 border p-4 flex flex-col gap-2\">");
    for i in 0..12 {
        s.push_str(&format!("<a class=\"block px-3 py-2 rounded\" href=\"#{i}\">Section {i}</a>"));
    }
    s.push_str("</nav><main class=\"flex-1 min-w-0 flex flex-col\" id=main>");
    s.push_str("<header class=\"sticky top-0 z-10 bg-white h-14 flex items-center justify-between px-3 border-b\">\
<h1 class=\"text-lg font-semibold\">Positions</h1><div class=\"flex gap-2\"><input id=search class=\"border rounded px-2 h-8\" placeholder=Search>\
<button class=\"btn border rounded px-3\">Export</button></div></header>");
    s.push_str(
        "<div class=\"flex-1 overflow-auto p-4\" id=scroller><table><thead class=\"sticky top-0 bg-gray-50\"><tr>",
    );
    for h in ["", "Instrument", "Side", "Quantity", "Price", "Close qty", ""] {
        s.push_str(&format!("<th class=\"text-xs font-medium text-gray-500\">{h}</th>"));
    }
    s.push_str("</tr></thead><tbody id=tbody>");
    for i in 0..rows {
        s.push_str(&row(i));
    }
    s.push_str("</tbody></table></div></main></div><div id=portal></div></body></html>");
    s
}

struct Page {
    dom: Dom,
    doc: NodeId,
    layout: Layout,
}

impl Page {
    fn id(&self, id: &str) -> NodeId {
        self.dom.get_element_by_id(self.doc, id).unwrap().unwrap()
    }
    fn pass(&self) -> f64 {
        let t = Instant::now();
        let r = self.layout.flow(&self.dom);
        std::hint::black_box(r.boxes.len());
        t.elapsed().as_secs_f64() * 1000.0
    }
}

fn page(rows: usize) -> Page {
    let dom_ref: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom_ref, "http://bench/", &doc(rows));
    let dom = Rc::try_unwrap(dom_ref).ok().expect("sole owner").into_inner();
    let mut layout = Layout::with_viewport(doc, Viewport::new(1440, 900));
    layout.style_mut().discover_style_elements(&dom);
    layout.set_geometry(Geometry::Flow);
    Page { dom, doc, layout }
}

/// Median of `n` runs of `change` followed by a pass. FLOW_BENCH_ONLY=<name>
/// skips the other scenarios (to profile one, e.g. under callgrind with
/// `--toggle-collect=*flow_bench*scenario*`).
#[inline(never)]
fn scenario(p: &mut Page, name: &str, n: usize, mut change: impl FnMut(&mut Page, usize)) -> f64 {
    if std::env::var("FLOW_BENCH_ONLY").is_ok_and(|only| only != name) {
        return f64::NAN;
    }
    let mut times: Vec<f64> = (0..n)
        .map(|k| {
            change(p, k);
            p.pass()
        })
        .collect();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    times[n / 2]
}

fn bench(rows: usize) {
    let mut p = page(rows);
    let first = p.pass();
    let boxes = p.layout.flow(&p.dom).boxes.len();
    let cached = p.pass();
    let n = 9;
    let class = scenario(&mut p, "class", n, |p, k| {
        let r = p.id(&format!("r{}", rows / 2));
        let v = if k % 2 == 0 { "border-b selected" } else { "border-b" };
        p.dom.set_attribute(r, "class", v).unwrap();
    });
    let text = scenario(&mut p, "text", n, |p, k| {
        let el = p.id(&format!("n{}", rows / 3));
        p.dom.set_text_content(el, &format!("Renamed {k}")).unwrap();
    });
    let value = scenario(&mut p, "value", n, |p, k| {
        let el = p.id(&format!("q{}", rows / 4));
        p.dom.set_attribute(el, "value", &"9".repeat(k + 1)).unwrap();
    });
    let hover = scenario(&mut p, "hover", n, |p, k| {
        let el = p.id(&format!("c{}", k % rows));
        let doc = p.doc;
        p.dom.set_hovered(doc, Some(el)).unwrap();
    });
    let focus = scenario(&mut p, "focus", n, |p, k| {
        let el = p.id(&format!("q{}", (k * 7) % rows));
        let doc = p.doc;
        p.dom.set_focused(doc, Some(el)).unwrap();
    });
    let insert = scenario(&mut p, "insert", n, |p, _| {
        let tbody = p.id("tbody");
        let first = p.dom.first_child(tbody).unwrap().unwrap();
        let clone = p.dom.clone_node(first, true, None).unwrap();
        p.dom.insert_before(tbody, clone, Some(first)).unwrap();
    });
    let popover = scenario(&mut p, "popover", n, |p, k| {
        let portal = p.id("portal");
        if k % 2 == 0 {
            let doc = p.doc;
            let d = p.dom.create_element(doc, "div").unwrap();
            p.dom.set_attribute(d, "style", "position:absolute;top:120px;left:300px;width:240px").unwrap();
            for i in 0..8 {
                let o = p.dom.create_element(doc, "div").unwrap();
                p.dom.set_attribute(o, "class", "px-3 py-2").unwrap();
                p.dom.set_text_content(o, &format!("Option {i}")).unwrap();
                p.dom.append_child(d, o).unwrap();
            }
            p.dom.append_child(portal, d).unwrap();
        } else {
            p.dom.replace_all(portal, None).unwrap();
        }
    });
    println!(
        "[flow_bench rows={rows}] boxes={boxes} first={first:.2}ms cached={cached:.3}ms \
class={class:.2}ms text={text:.2}ms value={value:.2}ms hover={hover:.2}ms focus={focus:.2}ms \
insert_row={insert:.2}ms popover={popover:.2}ms"
    );
}

#[test]
#[ignore]
fn flow_bench() {
    let rows = std::env::var("FLOW_BENCH_ROWS").unwrap_or_else(|_| "200,1000".into());
    for r in rows.split(',') {
        bench(r.trim().parse().unwrap());
    }
}
