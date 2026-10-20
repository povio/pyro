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
