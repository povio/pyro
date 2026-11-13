//! The hit-test index (`pyro_dom::layout`'s `HitIndex`: the boxes of one
//! layout in tree order under a grid) must answer exactly what the descent
//! of the engine's tree and the scan of every box answer, and must not
//! outlive the layout or scroll position it was built of.
//!
//! Every page of `hit_descent` and `hit_adversarial` (fixtures, random
//! pages, random pages mutated between tests) is compared three ways by
//! `Page::compare` and `lay2_compare_points`. Here: pages that load the grid
//! (hundreds of small overlapping boxes, so cells hold different lists),
//! points on the boxes' very edges (a cell boundary must not lose the box
//! whose edge is on it), tall and wide documents, scrolling, and when an
//! index is built and dropped.
use pyro_dom::media::Viewport;

use crate::hit_adversarial::{random_children, Rng};
use crate::hit_descent::{page, Page};

/// The three paths at `points`; the first mismatches are the failure.
fn three_way(p: &Page, label: &str, points: &[(f64, f64)]) -> usize {
    let mut distinct = std::collections::HashSet::new();
    let mut mismatches = Vec::new();
    for &(x, y) in points {
        let indexed = p.layout.element_from_point_indexed(&p.dom, x, y);
        let scan = p.layout.element_from_point_scan(&p.dom, x, y);
        let descent = p.layout.element_from_point_descent(&p.dom, x, y);
        let asked = p.layout.element_from_point(&p.dom, x, y);
        if scan != indexed || scan != descent || scan != asked {
            mismatches.push(format!(
                "{label} at ({x}, {y}): scan {} descent {} index {} asked {}",
                p.describe(scan),
                p.describe(descent),
                p.describe(indexed),
                p.describe(asked)
            ));
        }
        distinct.insert(scan);
    }
    assert!(
        mismatches.is_empty(),
        "{} mismatches, first:\n{}",
        mismatches.len(),
        mismatches[..mismatches.len().min(12)].join("\n")
    );
    distinct.len()
}

/// The corners and edge midpoints of some sixty boxes as the page reports
/// them, each also one layout unit and one float step to either side.
fn edge_points(p: &Page, count: u32) -> Vec<(f64, f64)> {
    let mut points = Vec::new();
    for i in (1..=count).step_by((count as usize / 60).max(1)) {
        let Some(el) = p.dom.get_element_by_id(p.doc, &format!("b{i}")).ok().flatten() else { continue };
        let r = p.layout.bounding_client_rect(&p.dom, el);
        let xs = [r.x, r.x + r.width / 2.0, r.x + r.width];
        let ys = [r.y, r.y + r.height / 2.0, r.y + r.height];
        for &x in &xs {
            for &y in &ys {
                points.push((x, y));
                for d in [1.0 / 64.0, f64::EPSILON * x.abs().max(1.0)] {
                    points.push((x - d, y));
                    points.push((x + d, y));
                    points.push((x, y - d));
                    points.push((x, y + d));
                }
            }
        }
    }
    points
}

fn dense_boxes(r: &mut Rng, count: u32, width: u64, height: u64) -> String {
    let mut body = String::new();
    for i in 1..=count {
        let (w, h) = (1 + r.below(90), 1 + r.below(60));
        let (x, y) = (r.below(width), r.below(height));
        let mut style = format!("position:absolute;left:{x}px;top:{y}px;width:{w}px;height:{h}px;");
        if r.chance(40) {
            style += &format!("z-index:{};", r.below(9) as i64 - 4);
        }
        if r.chance(8) {
            style += "pointer-events:none;";
        }
        if r.chance(6) {
            style += "visibility:hidden;";
        }
        if r.chance(8) {
            style += &format!("transform:translate({}px,{}px) rotate({}deg);", r.below(40), r.below(40), r.below(90));
        }
        if r.chance(6) {
            style += "opacity:0.5;";
        }
        if r.chance(6) {
            style += "position:fixed;";
        }
        if r.chance(10) {
            style += "overflow:hidden;";
        }
        if r.chance(5) {
            style += "clip-path:inset(4px 6px);";
        }
        // Some with a child reaching out of them (clipped or not), some
        // with half-pixel geometry.
        if r.chance(20) {
            style += &format!("margin-left:{}.5px;margin-top:0.25px;", r.below(3));
        }
        let child = match r.below(5) {
            0 => format!(
                "<span style=\"position:absolute;left:{}px;top:{}px;width:30px;height:30px;z-index:{}\"></span>",
                r.below(80) as i64 - 20,
                r.below(60) as i64 - 20,
                r.below(5) as i64 - 2
            ),
            1 => "<i style=\"display:block;width:140px;height:8px\"></i>".to_string(),
            _ => String::new(),
        };
        body += &format!("<div id=b{i} style=\"{style}\">{child}</div>");
    }
    body
}

#[test]
fn dense_overlapping_boxes_match_on_and_around_every_edge() {
    let pages: u64 = std::env::var("PYRO_HIT_INDEX_PAGES").ok().and_then(|v| v.parse().ok()).unwrap_or(9);
    for seed in 0..pages {
        let mut r = Rng(0x9e37_79b9_7f4a_7c15 ^ seed.wrapping_mul(0x2545_f491_4f6c_dd1d));
        let count = [40, 250, 600][(seed % 3) as usize];
        // A page of the viewport's size, a tall one and a wide one.
        let (w, h) = [(760, 560), (700, 6000), (5000, 500)][((seed / 3) % 3) as usize];
        let html = format!(
            "<!doctype html><html><head><style>body {{ margin: 0 }}</style></head><body>{}\
             <div style=\"width:{w}px;height:{h}px\"></div></body></html>",
            dense_boxes(&mut r, count, w, h)
        );
        let mut p = page(&html);
        let mut points = edge_points(&p, count);
        for _ in 0..400 {
            points.push((r.below(1700) as f64 / 2.0 - 20.0, r.below(1300) as f64 / 2.0 - 20.0));
        }
        points.extend([(f64::NAN, 5.0), (5.0, f64::NAN), (f64::INFINITY, 5.0), (-1.0e9, 1.0e9), (0.0, 0.0)]);
        let distinct = three_way(&p, &format!("dense {seed}"), &points);
        assert!(distinct > count as usize / 8, "dense {seed}: only {distinct} elements hit");
        // The same page scrolled: the boxes' edges are elsewhere, fixed
        // boxes stay.
        p.layout.window_scroll_to(&p.dom, Some(r.below(300) as i32), Some(r.below(900) as i32));
        let mut points = edge_points(&p, count);
        for _ in 0..150 {
            points.push((r.below(1600) as f64 / 2.0, r.below(1200) as f64 / 2.0));
        }
        three_way(&p, &format!("dense {seed} scrolled"), &points);
        p.layout.set_viewport(Viewport::new(500, 900));
        three_way(&p, &format!("dense {seed} resized"), &edge_points(&p, count));
    }
}

#[test]
fn random_pages_match_through_the_index() {
    let pages: u64 = std::env::var("PYRO_HIT_INDEX_PAGES").ok().and_then(|v| v.parse().ok()).unwrap_or(6);
    for seed in 0..pages {
        let mut r = Rng(0x0123_4567_89ab_cdef ^ seed.wrapping_mul(0x2545_f491_4f6c_dd1d));
        let mut body = String::new();
        let mut id = 0;
        for _ in 0..3 {
            random_children(&mut r, 0, &mut body, &mut id);
        }
        let html = format!(
            "<!doctype html><html><head><style>body {{ margin: 0; font: 13px/15px sans-serif }}</style></head>\
             <body>{body}</body></html>"
        );
        let p = page(&html);
        let mut points = Vec::new();
        for y in (0..600).step_by(7) {
            for x in (0..800).step_by(11) {
                points.push((x as f64 + 0.5, y as f64));
            }
        }
        three_way(&p, &format!("random {seed}"), &points);
    }
}

#[test]
fn an_index_is_built_when_tests_repeat_and_dropped_with_its_layout() {
    let mut p = page(
        "<!doctype html><html><head><style>
           body { margin: 0 } #a:hover { margin-left: 200px }
           #a, #b { width: 100px; height: 100px } #s { height: 3000px }
         </style></head><body><div id=a></div><div id=b></div><div id=s></div></body></html>",
    );
    let (a, b) = (p.id("a"), p.id("b"));
    let at = |p: &Page, x: f64, y: f64| p.layout.element_from_point(&p.dom, x, y);
    // The first test of a document is a descent, and so is every test of a
    // page that changes after each one until an index was built once (its
    // price is not known before).
    assert_eq!(at(&p, 50.0, 50.0), Some(a));
    assert_eq!(p.layout.hit_index_stats(&p.dom), (0, false));
    for round in 0..40 {
        p.dom.set_attribute(b, "data-round", &round.to_string()).unwrap();
        assert_eq!(at(&p, 50.0, 150.0), Some(b));
    }
    assert_eq!(p.layout.hit_index_stats(&p.dom), (0, false), "one test per layout builds nothing");
    // A page tested as it stands is indexed (the descents pay first).
    for _ in 0..10_000 {
        assert_eq!(at(&p, 50.0, 50.0), Some(a));
        if p.layout.hit_index_stats(&p.dom).1 {
            break;
        }
    }
    let (builds, current) = p.layout.hit_index_stats(&p.dom);
    assert!(current && builds == 1, "builds {builds} current {current}");
    assert_eq!(at(&p, 50.0, 150.0), Some(b));
    assert_eq!(at(&p, 150.0, 50.0), p.dom.body(p.doc).unwrap());
    assert_eq!(p.layout.hit_index_stats(&p.dom).0, 1, "one index answers every point");
    // A window scroll: the index of the old position must not answer.
    p.layout.window_scroll_to(&p.dom, None, Some(100));
    assert!(!p.layout.hit_index_stats(&p.dom).1);
    assert_eq!(at(&p, 50.0, 50.0), Some(b));
    assert_eq!(p.layout.element_from_point_indexed(&p.dom, 50.0, 50.0), Some(b));
    p.layout.window_scroll_to(&p.dom, None, Some(0));
    assert_eq!(p.layout.element_from_point_indexed(&p.dom, 50.0, 50.0), Some(a));
    // Hover moves #a: the index of the layout before the hover is gone.
    p.dom.set_hovered(p.doc, Some(a)).unwrap();
    assert!(!p.layout.hit_index_stats(&p.dom).1);
    assert_eq!(at(&p, 50.0, 50.0), p.dom.body(p.doc).unwrap());
    assert_eq!(p.layout.element_from_point_indexed(&p.dom, 250.0, 50.0), Some(a));
    p.dom.set_hovered(p.doc, None).unwrap();
    assert_eq!(at(&p, 50.0, 50.0), Some(a));
    // A style change and a removal.
    p.layout.element_from_point_indexed(&p.dom, 50.0, 50.0);
    p.dom.set_attribute(a, "style", "pointer-events: none").unwrap();
    assert_eq!(at(&p, 50.0, 50.0), p.dom.body(p.doc).unwrap());
    p.layout.element_from_point_indexed(&p.dom, 50.0, 50.0);
    let body = p.dom.body(p.doc).unwrap().unwrap();
    p.dom.remove_child(body, a).unwrap();
    assert_eq!(at(&p, 50.0, 50.0), Some(b));
    assert_eq!(p.layout.element_from_point_indexed(&p.dom, 50.0, 50.0), Some(b));
}
