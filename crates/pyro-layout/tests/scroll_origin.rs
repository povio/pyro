//! Scroll containers whose scroll origin is at the end of an axis (CSSOM View: right-to-left,
//! `row-reverse`, `column-reverse`): their content overflows on the start side, `scrollWidth`/
//! `scrollHeight` count it, and `scrollLeft`/`scrollTop` run from 0 down to minus the scroll
//! range. Values as Chromium reports them (WPT `css-backgrounds/local-attachment-*-scroll.html`,
//! `background-attachment-local/attachment-local-positioning-3.html`).
use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::layout::Layout;
use pyro_dom::media::Viewport;
use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::{Dom, NodeId};

struct Page {
    dom: Dom,
    doc: NodeId,
    layout: Layout,
}

/// Loads nothing (the pages have no sub-resources).
struct NoFetch;
impl pyro_render::Fetcher for NoFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err(format!("{url}: offline")));
    }
}

fn page(html: &str) -> Page {
    pyro_layout::install(std::sync::Arc::new(NoFetch));
    let dom_ref: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom_ref, "http://test/", html);
    let dom = Rc::try_unwrap(dom_ref).ok().expect("sole owner").into_inner();
    let mut layout = Layout::with_viewport(doc, Viewport::new(800, 600));
    layout.style_mut().discover_style_elements(&dom);
    Page { dom, doc, layout }
}

impl Page {
    fn id(&self, id: &str) -> NodeId {
        self.dom.get_element_by_id(self.doc, id).unwrap().unwrap_or_else(|| panic!("#{id}"))
    }
}

/// A 100x100 scroll container `style` holding two 100x100 boxes side by side (or stacked, in a
/// column).
fn scroller(style: &str) -> String {
    format!(
        "<!doctype html><body style='margin:0'><div id=s style='width:100px;height:100px;overflow:auto;scrollbar-width:none;{style}'>\
         <div style='width:100px;height:100px;flex-shrink:0'></div><div style='width:100px;height:100px;flex-shrink:0'></div></div>"
    )
}

/// `scrollLeft` of a right-to-left box, and of a `row-reverse` flex container, runs from 0 (the
/// initial position, its content's end in view) to -100: writes are clamped to that range. The
/// scroll range was empty (`scrollWidth` 100, every write clamped to 0): the overflow on the
/// left was unreachable.
#[test]
fn a_horizontal_scroll_origin_at_the_right_scrolls_to_negative_offsets() {
    for style in [
        "display:flex;direction:rtl",
        "display:flex;flex-direction:row-reverse",
        "direction:rtl;white-space:nowrap;font-size:0",
    ] {
        let html = if style.contains("nowrap") {
            "<!doctype html><body style='margin:0'><div id=s style='width:100px;height:100px;overflow:auto;scrollbar-width:none;direction:rtl'>\
             <div style='width:200px;height:20px'></div></div>"
                .to_string()
        } else {
            scroller(style)
        };
        let mut p = page(&html);
        let s = p.id("s");
        assert_eq!(p.layout.scroll_width(&p.dom, s), 200.0, "{style}: scrollWidth");
        assert_eq!(p.layout.scroll_left(s) as i64, 0, "{style}: initial scrollLeft");
        assert!(p.layout.set_scroll_left(&p.dom, s, -50), "{style}: -50 moves");
        assert_eq!(p.layout.scroll_left(s) as i64, -50, "{style}: scrollLeft -50");
        p.layout.set_scroll_left(&p.dom, s, -500);
        assert_eq!(p.layout.scroll_left(s) as i64, -100, "{style}: clamped to -100");
        p.layout.set_scroll_left(&p.dom, s, 30);
        assert_eq!(p.layout.scroll_left(s) as i64, 0, "{style}: clamped to 0");
        p.layout.element_scroll_by(&p.dom, s, Some(-30), None);
        assert_eq!(p.layout.scroll_left(s) as i64, -30, "{style}: scrollBy");
        // Scrolled to the left, the content moves right.
        let child = p.dom.element_children(s).unwrap().next().unwrap();
        let x = p.layout.bounding_client_rect(&p.dom, child).x;
        let expected = if style.contains("nowrap") { -100.0 + 30.0 } else { 30.0 };
        assert_eq!(x, expected, "{style}: the first child's x");
    }
}

/// `scrollTop` of a `column-reverse` flex container runs from 0 to -100.
#[test]
fn a_vertical_scroll_origin_at_the_bottom_scrolls_to_negative_offsets() {
    let mut p = page(&scroller("display:flex;flex-direction:column-reverse"));
    let s = p.id("s");
    assert_eq!(p.layout.scroll_height(&p.dom, s), 200.0, "scrollHeight");
    assert!(p.layout.set_scroll_top(&p.dom, s, -60));
    assert_eq!(p.layout.scroll_top(s) as i64, -60);
    p.layout.set_scroll_top(&p.dom, s, -1000);
    assert_eq!(p.layout.scroll_top(s) as i64, -100);
    // A left-to-right axis of the same box still scrolls from 0 up.
    assert_eq!(p.layout.scroll_width(&p.dom, s), 100.0);
    p.layout.set_scroll_left(&p.dom, s, -20);
    assert_eq!(p.layout.scroll_left(s) as i64, 0);
}

/// A scroll container's scrollable overflow ends after its end padding, past its in-flow
/// content: Chromium's `scrollWidth`/`scrollHeight` of a 100px box with 1px borders and 20px
/// padding around a 400x400 child are 440, and it scrolls to 342 (WPT
/// `local-attachment-content-box-scroll`). They were 420 and 322: the end padding was left out.
#[test]
fn the_scroll_range_ends_after_the_end_padding() {
    let mut p = page(
        "<!doctype html><body style='margin:0'><div id=s style='width:100px;height:100px;box-sizing:border-box;overflow:hidden;border:1px solid;padding:20px'>\
         <div style='width:400px;height:400px'></div></div>",
    );
    let s = p.id("s");
    assert_eq!(p.layout.scroll_width(&p.dom, s), 440.0, "scrollWidth");
    assert_eq!(p.layout.scroll_height(&p.dom, s), 440.0, "scrollHeight");
    p.layout.set_scroll_top(&p.dom, s, 1000);
    assert_eq!(p.layout.scroll_top(s) as i64, 342, "the end of the scroll range");
}
