//! A computed-style read after a DOM change syncs the styles; the rect read
//! that follows lays out from them without a second counted sync (no node
//! list or delta: nothing changed since). Alone in its binary: the sync
//! counters are process-wide.

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::layout::Layout;
use lp_dom::media::Viewport;
use lp_dom::parse::{parse_document, DomRef};
use lp_dom::select::parse_selectors;
use lp_dom::*;

struct NoFetch;
impl lp_render::Fetcher for NoFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err(format!("{url}: offline")));
    }
}

const PAGE: &str = r#"<!doctype html><html><head><style>
body { margin: 0 } .box { width: 100px; height: 20px }
</style></head><body><div id=host></div></body></html>"#;

#[test]
fn a_computed_style_read_then_a_rect_read_after_a_change_is_one_counted_sync() {
    lp_layout::install(std::sync::Arc::new(NoFetch));
    let dom_ref: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom_ref, "http://test/", PAGE);
    let mut dom = Rc::try_unwrap(dom_ref).ok().expect("sole owner").into_inner();
    dom.set_document_active(doc, true).unwrap();
    let mut layout = Layout::with_viewport(doc, Viewport::new(800, 600));
    layout.style_mut().discover_style_elements(&dom);
    let host = dom.query_selector(doc, &parse_selectors("#host").unwrap()).unwrap().unwrap();
    layout.bounding_client_rect(&dom, host);
    lp_layout::take_stats();

    for round in 0..3 {
        let el = dom.create_element(doc, "div").unwrap();
        dom.set_attribute(el, "class", "box").unwrap();
        dom.append_child(host, el).unwrap();
        // The computed-style read syncs the styles: one sync.
        assert_eq!(layout.style().stylo_value(&dom, el, "width").as_deref(), Some("100px"));
        assert_eq!(lp_layout::take_stats().0, 1, "round {round}: the style sync");
        // The rect read finishes the layout of that very state: not another.
        let r = layout.bounding_client_rect(&dom, el);
        assert_eq!((r.width, r.height, r.y), (100.0, 20.0, 20.0 * round as f64));
        assert_eq!(lp_layout::take_stats().0, 0, "round {round}: the layout after the style sync");
        // Nothing changed: nothing synced.
        layout.bounding_client_rect(&dom, el);
        assert_eq!(layout.style().stylo_value(&dom, el, "height").as_deref(), Some("20px"));
        assert_eq!(lp_layout::take_stats().0, 0, "round {round}: unchanged");
    }
    // A rect read alone after a change is one sync, as before.
    let el = dom.create_element(doc, "div").unwrap();
    dom.set_attribute(el, "class", "box").unwrap();
    dom.append_child(host, el).unwrap();
    assert_eq!(layout.bounding_client_rect(&dom, el).y, 60.0);
    assert_eq!(lp_layout::take_stats().0, 1);
}
