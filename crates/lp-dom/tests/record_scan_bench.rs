//! In-crate benchmark for the text scroll-size scan in `Ctx::record` (ignored
//! by default). Prints per-pass elapsed time and, in debug builds, the work
//! counters (walk steps of the reference form vs fact resolutions and pairs).
//! Run: cargo test -p lp-dom --test record_scan_bench -- --ignored --nocapture
//! Shapes: deep (single page clipper, 25-deep nested flex), near (every text's
//! parent clips), shallow (5-deep, wide). No speed assertion.
use lp_dom::layout::{Geometry, Layout};
use lp_dom::media::Viewport;
use lp_dom::parse::{parse_document, DomRef};
use lp_dom::Dom;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

fn doc(depth: usize, rows: usize, near_clip: bool) -> String {
    let leaf_style = if near_clip { "overflow:hidden;white-space:nowrap" } else { "white-space:nowrap" };
    let mut s = String::from("<div id=page style=\"width:1200px;height:800px;overflow:auto\">");
    for r in 0..rows {
        for i in 0..depth {
            s.push_str(&format!("<div class=\"l{i}\" style=\"display:flex;padding:1px\">"));
        }
        s.push_str(&format!("<span style=\"{leaf_style}\">row {r} cell text one</span><span style=\"{leaf_style}\">cell text two</span><input value=\"v{r}\">"));
        s.push_str(&"</div>".repeat(depth));
    }
    s.push_str("</div>");
    s
}

fn run(name: &str, html: &str, passes: usize) {
    let dom_ref: DomRef = Rc::new(RefCell::new(Dom::new()));
    let docn = parse_document(&dom_ref, "http://bench/", html);
    let mut dom = Rc::try_unwrap(dom_ref).ok().expect("sole owner").into_inner();
    let mut layout = Layout::with_viewport(docn, Viewport::new(1280, 800));
    layout.style_mut().discover_style_elements(&dom);
    layout.set_geometry(Geometry::Flow);
    let page = dom.get_element_by_id(docn, "page").unwrap().unwrap();
    let boxes = layout.flow(&dom).boxes.len();
    let mut total = 0.0;
    for i in 0..passes {
        // A new pass each time: toggle a harmless attribute on the root so the flow cache misses.
        dom.set_attribute(page, "data-pass", &i.to_string()).unwrap();
        #[cfg(debug_assertions)]
        {
            use lp_dom::flow::record_scan_stats::*;
            use std::sync::atomic::Ordering::Relaxed;
            WALK_STEPS.store(0, Relaxed);
            FACT_STEPS.store(0, Relaxed);
            PAIRS.store(0, Relaxed);
        }
        let t = Instant::now();
        let _ = layout.flow(&dom);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        total += ms;
        #[cfg(debug_assertions)]
        {
            use lp_dom::flow::record_scan_stats::*;
            use std::sync::atomic::Ordering::Relaxed;
            if i == passes - 1 {
                println!(
                    "[bench {name}] boxes={boxes} pairs={} walk_steps={} fact_steps={}",
                    PAIRS.load(Relaxed),
                    WALK_STEPS.load(Relaxed),
                    FACT_STEPS.load(Relaxed)
                );
            }
        }
    }
    println!("[bench {name}] boxes={boxes} passes={passes} avg_ms={:.2}", total / passes as f64);
}

#[test]
#[ignore]
fn record_scan_deep() {
    run("deep", &doc(25, 600, false), 8);
}

#[test]
#[ignore]
fn record_scan_near_clipped() {
    run("near", &doc(25, 600, true), 8);
}

#[test]
#[ignore]
fn record_scan_shallow() {
    run("shallow", &doc(5, 3000, false), 8);
}
