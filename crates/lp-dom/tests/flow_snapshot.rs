//! Diagnostic: lays out a static page snapshot in flow mode and prints every
//! body element's border box, one line per element in tree order:
//! `tag x y w h`. Run with
//! `FLOW_SNAPSHOT=page.html FLOW_VIEWPORT=1440x900 cargo test -p lp-dom --test flow_snapshot -- --ignored --nocapture`.
use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::layout::{Geometry, Layout};
use lp_dom::media::Viewport;
use lp_dom::node::NodeType;
use lp_dom::parse::{parse_document, DomRef};
use lp_dom::Dom;

#[test]
#[ignore]
fn dump_flow_snapshot() {
    let Ok(path) = std::env::var("FLOW_SNAPSHOT") else { return };
    let (vw, vh) = std::env::var("FLOW_VIEWPORT")
        .ok()
        .and_then(|v| v.split_once('x').map(|(w, h)| (w.parse().unwrap(), h.parse().unwrap())))
        .unwrap_or((1440, 900));
    let html = std::fs::read_to_string(&path).expect("snapshot");
    let dom_ref: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom_ref, "http://snapshot/", &html);
    let dom = Rc::try_unwrap(dom_ref).ok().expect("sole owner").into_inner();
    let mut layout = Layout::with_viewport(doc, Viewport::new(vw, vh));
    layout.style_mut().discover_style_elements(&dom);
    layout.set_geometry(Geometry::Flow);
    let t = std::time::Instant::now();
    let _ = layout.flow(&dom);
    eprintln!("flow pass: {:?}", t.elapsed());
    if let Ok(n) = std::env::var("FLOW_PROFILE_COLD_LOOP") {
        // Repeated cold passes (fresh Layout each time), for a sampler.
        let n = n.parse::<u32>().unwrap_or(1);
        let t = std::time::Instant::now();
        for _ in 0..n {
            let mut cold = Layout::with_viewport(doc, Viewport::new(vw, vh));
            cold.style_mut().discover_style_elements(&dom);
            cold.set_geometry(Geometry::Flow);
            let _ = cold.flow(&dom);
        }
        eprintln!("{n} cold passes: {:?} per pass", t.elapsed() / n);
    }
    if std::env::var("FLOW_PROFILE").is_ok() {
        use lp_dom::flow::FlowStyleSource;
        let els: Vec<_> =
            dom.descendants(doc).unwrap().filter(|&n| dom.node_type(n).ok() == Some(NodeType::Element)).collect();
        let t = std::time::Instant::now();
        for &e in &els {
            let _ = layout.style().flow_style(&dom, e);
        }
        eprintln!("flow_style x{} (warm cascade cache): {:?}", els.len(), t.elapsed());
        if let Ok(n) = std::env::var("FLOW_PROFILE_LOOP") {
            for _ in 0..n.parse::<usize>().unwrap_or(1) {
                for &e in &els {
                    let _ = layout.style().flow_style(&dom, e);
                }
            }
        }
        // A DOM mutation invalidates caches: measure a pass after one.
        let probe = els[els.len() / 2];
        let mut dom = dom;
        dom.set_attribute(probe, "data-flow-probe", "1").unwrap();
        let t = std::time::Instant::now();
        for &e in &els {
            let _ = layout.style().flow_style(&dom, e);
        }
        eprintln!("flow_style x{} after a mutation: {:?}", els.len(), t.elapsed());
        let t = std::time::Instant::now();
        let r = layout.flow(&dom);
        eprintln!("flow pass after a mutation: {:?} ({} boxes)", t.elapsed(), r.boxes.len());
        if let Ok(n) = std::env::var("FLOW_PROFILE_MUTATIONS") {
            // Repeated mutate + relayout, for sampling.
            let n = n.parse::<usize>().unwrap_or(1);
            let t = std::time::Instant::now();
            for i in 0..n {
                dom.set_attribute(probe, "data-flow-probe", &i.to_string()).unwrap();
                let _ = layout.flow(&dom);
            }
            eprintln!("{n} mutate+pass: {:?} per pass", t.elapsed() / n as u32);
        }
        if let Ok(n) = std::env::var("FLOW_PROFILE_METRIC_READS") {
            // React-like churn: insert an element deep in the page, then read
            // a layout metric (clientWidth) that forces a style flush, n times.
            let n = n.parse::<usize>().unwrap_or(1);
            let deep = els[els.len() * 3 / 4];
            let reader = els[els.len() / 3];
            let t = std::time::Instant::now();
            for i in 0..n {
                let e = dom.create_element(doc, "span").unwrap();
                dom.set_attribute(e, "class", if i % 2 == 0 { "truncate text-small" } else { "flex items-center" })
                    .unwrap();
                dom.append_child(deep, e).unwrap();
                let _ = layout.client_width(&dom, reader);
            }
            eprintln!("{n} insert+clientWidth: {:?} per iteration", t.elapsed() / n as u32);
        }
        if let Ok(n) = std::env::var("FLOW_PROFILE_STYLE_FLUSH") {
            // React-like updates of attached elements (class toggles on 20
            // elements across the page), then a style read that forces the
            // invalidation (StyleEngine::ensure), n times. No layout.
            let n = n.parse::<usize>().unwrap_or(1);
            let targets: Vec<_> = (0..20).map(|k| els[els.len() / 4 + k * (els.len() / 2) / 20]).collect();
            let reader = els[els.len() / 3];
            let t = std::time::Instant::now();
            for i in 0..n {
                for &e in &targets {
                    dom.set_attribute(e, "class", if i % 2 == 0 { "truncate text-small" } else { "flex items-center" })
                        .unwrap();
                }
                let _ = layout.style().computed(&dom, reader);
            }
            eprintln!("{n} x (20 class updates + style flush): {:?} per iteration", t.elapsed() / n as u32);
        }
        if std::env::var("FLOW_PROFILE_BOUNDARIES").is_ok() {
            // Analysis for subtree relayout: for each element, the subtree under
            // its nearest size-contained ancestor (static: px width and height,
            // overflow clipped both ways; dynamic: overflow clipped both ways,
            // its size checked after relayout) as a share of all elements.
            use lp_dom::flow::{Dim, FlowStyleSource};
            let total = els.len() as f64;
            let index: std::collections::HashMap<_, _> = els.iter().enumerate().map(|(i, &e)| (e, i)).collect();
            let mut size = vec![1usize; els.len()];
            for (i, &e) in els.iter().enumerate().rev() {
                if let Ok(Some(parent)) = dom.parent_element(e) {
                    if let Some(&pi) = index.get(&parent) {
                        size[pi] += size[i];
                    }
                }
            }
            let px = |d: &Dim| matches!(d, Dim::Len(l) if l.percent == 0.0);
            let mut stat = Vec::new();
            let mut dynamic = Vec::new();
            for &e in &els {
                let (mut s_found, mut d_found) = (None, None);
                let mut cur = dom.parent_element(e).ok().flatten();
                while let Some(a) = cur {
                    if let Some(st) = layout.style().flow_style(&dom, a) {
                        let clips = st.clips_x && st.clips_y;
                        if d_found.is_none() && clips {
                            d_found = Some(a);
                        }
                        if s_found.is_none() && clips && px(&st.width) && px(&st.height) {
                            s_found = Some(a);
                        }
                    }
                    if s_found.is_some() && d_found.is_some() {
                        break;
                    }
                    cur = dom.parent_element(a).ok().flatten();
                }
                let share =
                    |f: Option<lp_dom::NodeId>| f.and_then(|a| index.get(&a)).map_or(1.0, |&i| size[i] as f64 / total);
                stat.push(share(s_found));
                dynamic.push(share(d_found));
            }
            for (name, mut v) in [("static", stat), ("dynamic", dynamic)] {
                v.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let q = |p: f64| v[((v.len() - 1) as f64 * p) as usize];
                let mean = v.iter().sum::<f64>() / v.len() as f64;
                eprintln!(
                    "boundaries {name}: relayout share of {} elements: p10 {:.3} median {:.3} p90 {:.3} mean {:.3} (1.0 = whole document)",
                    v.len(), q(0.1), q(0.5), q(0.9), mean
                );
            }
        }
        if let Ok(n) = std::env::var("FLOW_PROFILE_PORTAL") {
            // Portal churn on body (React overlays): append, lay out, remove.
            let n = n.parse::<usize>().unwrap_or(1);
            let body = dom.body(doc).unwrap().expect("body");
            let t = std::time::Instant::now();
            for _ in 0..n {
                let portal = dom.create_element(doc, "div").unwrap();
                dom.append_child(body, portal).unwrap();
                let _ = layout.flow(&dom);
                dom.remove_child(body, portal).unwrap();
                let _ = layout.flow(&dom);
            }
            eprintln!("{n} portal append+remove: {:?} per pass", t.elapsed() / (2 * n as u32));
        }
        if let Ok(n) = std::env::var("FLOW_PROFILE_ROWS") {
            // A list render after its data arrives: every table body loses
            // its rows (loading state), then gets them back in one commit
            // (new elements: cold style), followed by forced layouts after
            // small mutations (effects reading metrics).
            let n = n.parse::<usize>().unwrap_or(1);
            let bodies: Vec<_> =
                dom.descendants(doc).unwrap().filter(|&e| dom.tag_name(e).ok().as_deref() == Some("TBODY")).collect();
            let mut rows = Vec::new();
            for &b in &bodies {
                for r in dom.children(b).unwrap() {
                    rows.push((b, r));
                }
            }
            for &(b, r) in &rows {
                dom.remove_child(b, r).unwrap();
            }
            let t = std::time::Instant::now();
            let _ = layout.flow(&dom);
            eprintln!("rows: empty pass {:?} ({} rows removed)", t.elapsed(), rows.len());
            // React builds each row off-document, node by node (every
            // creation, attribute and append journaled), then inserts it.
            fn rebuild(dom: &mut Dom, doc: lp_dom::NodeId, src: lp_dom::NodeId) -> lp_dom::NodeId {
                if dom.node_type(src).ok() != Some(NodeType::Element) {
                    let text = dom.char_data(src).unwrap_or("").to_owned();
                    return dom.create_text(doc, &text).unwrap();
                }
                let e = dom.element(src).unwrap();
                let ns = e.name.ns.to_string();
                let local = e.name.local.to_string();
                let attrs: Vec<(String, String)> =
                    e.attrs.iter().map(|a| (a.name.local.to_string(), a.value.to_string())).collect();
                let el = dom.create_element_ns(doc, Some(&ns), &local).unwrap();
                for (n, v) in attrs {
                    dom.set_attribute(el, &n, &v).unwrap();
                }
                let kids: Vec<_> = dom.children(src).unwrap().collect();
                for k in kids {
                    let c = rebuild(dom, doc, k);
                    dom.append_child(el, c).unwrap();
                }
                el
            }
            let v0 = dom.version();
            let t = std::time::Instant::now();
            for &(b, r) in &rows {
                let copy = rebuild(&mut dom, doc, r);
                dom.append_child(b, copy).unwrap();
            }
            let built = t.elapsed();
            let _ = layout.flow(&dom);
            eprintln!(
                "rows: rebuild {:?} + commit pass {:?} ({} journaled changes)",
                built,
                t.elapsed() - built,
                dom.version() - v0
            );
            if let Ok(k) = std::env::var("FLOW_PROFILE_REBUILDS") {
                // Repeated: remove the current rows, rebuild them, lay out.
                let k = k.parse::<u32>().unwrap_or(1);
                let t = std::time::Instant::now();
                for _ in 0..k {
                    for &b in &bodies {
                        let kids: Vec<_> = dom.children(b).unwrap().collect();
                        for r in kids {
                            dom.remove_child(b, r).unwrap();
                        }
                    }
                    let _ = layout.flow(&dom);
                    for &(b, r) in &rows {
                        let copy = rebuild(&mut dom, doc, r);
                        dom.append_child(b, copy).unwrap();
                    }
                    let _ = layout.flow(&dom);
                }
                eprintln!("rows: {k} remove+pass+rebuild+pass {:?} per round", t.elapsed() / k);
                // Styles served by the pass (cached, maybe shared between
                // equivalent elements) against each element's own cascade.
                use lp_dom::flow::FlowStyleSource;
                let all: Vec<_> = dom
                    .descendants(doc)
                    .unwrap()
                    .filter(|&n| dom.node_type(n).ok() == Some(NodeType::Element))
                    .collect();
                let differ = all
                    .iter()
                    .filter(|&&e| layout.style().flow_style(&dom, e) != layout.style().flow_style_unshared(&dom, e))
                    .count();
                eprintln!("rows: served vs own-cascade styles differ: {differ} of {}", all.len());
                // Incremental result against a fresh layout of the same DOM.
                let inc = layout.flow(&dom);
                let mut fresh = Layout::with_viewport(doc, Viewport::new(vw, vh));
                fresh.style_mut().discover_style_elements(&dom);
                fresh.set_geometry(Geometry::Flow);
                let f = fresh.flow(&dom);
                let same = f.boxes.len() == inc.boxes.len()
                    && f.boxes.iter().all(|(k, b)| inc.boxes.get(k).is_some_and(|c| c.rect == b.rect));
                eprintln!("rows: rebuilt vs fresh identical: {same} ({} boxes)", f.boxes.len());
            }
            let t = std::time::Instant::now();
            for i in 0..n {
                let (_, r) = rows[i % rows.len()];
                dom.set_attribute(r, "data-flow-probe", &i.to_string()).unwrap();
                let _ = layout.flow(&dom);
            }
            eprintln!("rows: {n} row mutate+pass {:?} per pass", t.elapsed() / n.max(1) as u32);
        }
        let t = std::time::Instant::now();
        let _ = layout.flow(&dom);
        eprintln!("flow pass, no change (cached): {:?}", t.elapsed());
        // The pass-scoped memo must not change any box: compare with a pass
        // computed directly (no memo).
        let memo = layout.flow(&dom);
        let (vw, vh) = (vw as f64, vh as f64);
        let plain = lp_dom::flow::layout(&dom, doc, layout.style(), vw, vh);
        let same =
            plain.boxes.len() == memo.boxes.len() && plain.boxes.iter().all(|(k, b)| memo.boxes.get(k) == Some(b));
        eprintln!("memo vs plain identical: {same} ({} boxes)", plain.boxes.len());
        assert!(same, "pass memo changed geometry");
        return;
    }
    let body = dom
        .descendants(doc)
        .unwrap()
        .find(|&n| dom.tag_name(n).is_ok_and(|t| t.eq_ignore_ascii_case("body")))
        .expect("body");
    for n in dom
        .children(doc)
        .unwrap()
        .chain(dom.children(dom.document_element(doc).unwrap().unwrap()).unwrap())
        .chain(dom.children(body).unwrap())
    {
        let r = layout.bounding_client_rect(&dom, n);
        eprintln!("top-level {:?} {:?} {:?}", dom.node_type(n).ok(), dom.tag_name(n).ok(), r);
    }
    let mut out = String::new();
    for n in dom.descendants(body).unwrap() {
        if n == body || dom.node_type(n).ok() != Some(NodeType::Element) {
            continue;
        }
        let tag = dom.tag_name(n).unwrap().to_ascii_lowercase();
        if matches!(tag.as_str(), "script" | "style" | "noscript" | "link") {
            continue;
        }
        let r = layout.bounding_client_rect(&dom, n);
        out.push_str(&format!("{tag} {:.2} {:.2} {:.2} {:.2}\n", r.x, r.y, r.width, r.height));
    }
    print!("{out}");
}
