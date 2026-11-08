//! A rect query (`getBoundingClientRect`, `getClientRects`, box quads) after
//! a DOM change brings the layout engine up to the document once: one full
//! sync, no style-only pass in front of it for the visibility check.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use lp_dom::layout::{ExternalBox, ExternalGeometry, ExternalQuads, GeometrySource, Layout};
use lp_dom::media::Viewport;
use lp_dom::parse::{parse_document, DomRef};
use lp_dom::select::parse_selectors;
use lp_dom::style::StyleEngine;
use lp_dom::*;

/// Loads nothing (the pages have no sub-resources).
struct NoFetch;
impl lp_render::Fetcher for NoFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err(format!("{url}: offline")));
    }
}

/// Blitz's geometry source, counting its full and style-only syncs.
struct Counted {
    inner: Rc<dyn GeometrySource>,
    syncs: Cell<usize>,
    style_syncs: Cell<usize>,
}

impl GeometrySource for Counted {
    fn sync(&self, dom: &Dom, layout: &Layout) {
        self.syncs.set(self.syncs.get() + 1);
        self.inner.sync(dom, layout)
    }
    fn sync_styles(&self, dom: &Dom, style: &StyleEngine) {
        self.style_syncs.set(self.style_syncs.get() + 1);
        self.inner.sync_styles(dom, style)
    }
    fn element_box(&self, key: u64) -> Option<ExternalBox> {
        self.inner.element_box(key)
    }
    fn element_quads(&self, key: u64) -> Option<ExternalQuads> {
        self.inner.element_quads(key)
    }
    fn geometry(&self) -> Option<ExternalGeometry> {
        self.inner.geometry()
    }
    fn document_size(&self) -> Option<(f64, f64)> {
        self.inner.document_size()
    }
    fn has_sticky(&self) -> bool {
        self.inner.has_sticky()
    }
    fn set_base_url(&self, url: &str) {
        self.inner.set_base_url(url)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self.inner.as_any()
    }
    fn active(&self) -> bool {
        self.inner.active()
    }
    fn stale(&self) -> bool {
        self.inner.stale()
    }
    fn element_style(&self, key: u64) -> Option<::style::servo_arc::Arc<::style::properties::ComputedValues>> {
        self.inner.element_style(key)
    }
    fn element_displayed(&self, key: u64) -> Option<bool> {
        self.inner.element_displayed(key)
    }
}

struct Page {
    dom: Dom,
    doc: NodeId,
    layout: Layout,
    source: Rc<Counted>,
}

const PAGE: &str = r#"<!doctype html><html><head><style>
  .box { width: 100px; height: 20px }
  .gone { display: none }
  .ghost { visibility: hidden }
  #wrap { container-type: inline-size; width: 300px }
  @container (max-width: 200px) { #cq { display: none } }
</style></head><body>
<div id=host></div>
<div id=wrap><div id=cq class=box></div></div>
</body></html>"#;

fn page() -> Page {
    lp_layout::install(std::sync::Arc::new(NoFetch));
    let dom_ref: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom_ref, "http://test/", PAGE);
    let mut dom = Rc::try_unwrap(dom_ref).ok().expect("sole owner").into_inner();
    dom.set_document_active(doc, true).unwrap();
    let mut layout = Layout::with_viewport(doc, Viewport::new(800, 600));
    layout.style_mut().discover_style_elements(&dom);
    let inner = layout.synced_geometry_source(&dom).expect("Blitz geometry").clone();
    let source = Rc::new(Counted { inner, syncs: Cell::new(0), style_syncs: Cell::new(0) });
    layout.set_geometry_source(Some(source.clone()));
    // Lay the page out once before counting.
    layout.synced_geometry_source(&dom);
    source.syncs.set(0);
    source.style_syncs.set(0);
    Page { dom, doc, layout, source }
}

impl Page {
    fn find(&self, css: &str) -> NodeId {
        self.dom.query_selector(self.doc, &parse_selectors(css).unwrap()).unwrap().unwrap()
    }
    /// Appends `<div class=...>` to `#host`.
    fn append(&mut self, class: &str) -> NodeId {
        let host = self.find("#host");
        let el = self.dom.create_element(self.doc, "div").unwrap();
        self.dom.set_attribute(el, "class", class).unwrap();
        self.dom.append_child(host, el).unwrap();
        el
    }
    /// (full syncs, style-only syncs) since the last call.
    fn take(&self) -> (usize, usize) {
        (self.source.syncs.replace(0), self.source.style_syncs.replace(0))
    }
}

#[test]
fn one_append_then_one_rect_read_is_one_sync() {
    let mut p = page();
    let el = p.append("box");
    let r = p.layout.bounding_client_rect(&p.dom, el);
    assert_eq!((r.width, r.height), (100.0, 20.0));
    assert_eq!(p.take(), (1, 0), "visible element");
    // Nothing changed since: no sync at all.
    p.layout.bounding_client_rect(&p.dom, el);
    assert_eq!(p.take(), (0, 0));

    let el = p.append("box");
    assert_eq!(p.layout.client_rect(&p.dom, el).map(|r| r.width), Some(100.0));
    assert_eq!(p.take(), (1, 0), "client_rect");

    let el = p.append("box");
    assert!(p.layout.box_quads(&p.dom, el).is_some());
    assert_eq!(p.take(), (1, 0), "box_quads");
}

#[test]
fn hidden_and_detached_elements_take_one_sync_and_read_as_before() {
    let mut p = page();
    let gone = p.append("box gone");
    assert_eq!(p.layout.bounding_client_rect(&p.dom, gone), Default::default());
    assert_eq!(p.layout.client_rect(&p.dom, gone), None);
    assert!(p.layout.box_quads(&p.dom, gone).is_none());
    assert_eq!(p.take(), (1, 0), "display:none");

    // visibility:hidden still has a box.
    let ghost = p.append("box ghost");
    let r = p.layout.bounding_client_rect(&p.dom, ghost);
    assert_eq!((r.width, r.height), (100.0, 20.0));
    assert_eq!(p.take(), (1, 0), "visibility:hidden");

    let detached = p.dom.create_element(p.doc, "div").unwrap();
    p.dom.set_attribute(detached, "class", "box").unwrap();
    assert_eq!(p.layout.bounding_client_rect(&p.dom, detached), Default::default());
    assert_eq!(p.layout.client_rect(&p.dom, detached), None);
    let (syncs, style_syncs) = p.take();
    assert!(syncs <= 1 && style_syncs == 0, "detached: {syncs} syncs, {style_syncs} style syncs");
}

#[test]
fn a_container_query_that_toggles_display_reads_consistently() {
    let mut p = page();
    let cq = p.find("#cq");
    let wrap = p.find("#wrap");
    let r = p.layout.bounding_client_rect(&p.dom, cq);
    assert_eq!((r.width, r.height), (100.0, 20.0));
    p.take();

    p.dom.set_attribute(wrap, "style", "width: 150px").unwrap();
    let r = p.layout.bounding_client_rect(&p.dom, cq);
    let display = p.layout.style().stylo_value(&p.dom, cq, "display");
    let hidden = display.as_deref() == Some("none");
    assert!(hidden, "a 150px container hides #cq: display {display:?}");
    assert_eq!(p.take(), (1, 0));
    // The rect agrees with the style the query answered from.
    assert_eq!(hidden, r == Default::default(), "display {display:?}, rect {r:?}");
    assert_eq!(p.layout.client_rect(&p.dom, cq).is_none(), hidden);

    p.dom.set_attribute(wrap, "style", "width: 300px").unwrap();
    let r = p.layout.bounding_client_rect(&p.dom, cq);
    assert_eq!((r.width, r.height), (100.0, 20.0));
    assert_eq!(p.take(), (1, 0));
}
