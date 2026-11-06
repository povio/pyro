//! `is_visible` (the `display: none` check in front of every rect query)
//! answered from the styles Blitz keeps on its rendered tree: a hidden
//! ancestor, an ancestor hidden after its subtree was styled, an element
//! moved under a hidden parent, shadow trees and detached elements, checked
//! against the [`ComputedStyle`](lp_dom::style::ComputedStyle) derivation it
//! replaces.

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::layout::Layout;
use lp_dom::media::Viewport;
use lp_dom::node::ShadowRootMode;
use lp_dom::parse::{parse_document, DomRef};
use lp_dom::select::parse_selectors;
use lp_dom::*;

/// Loads nothing (the pages have no sub-resources).
struct NoFetch;
impl lp_render::Fetcher for NoFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err(format!("{url}: offline")));
    }
}

const PAGE: &str = r#"<!doctype html><html><head><style>
  .box { width: 100px; height: 20px }
  .gone { display: none }
  .contents { display: contents }
</style></head><body>
<div id=a class=gone><div id=a1 class=box><span id=a2>x</span></div></div>
<div id=b><div id=b1 class=box><span id=b2>x</span></div></div>
<div id=c class=gone></div>
<div id=d><div id=d1 class=box><span id=d2>x</span></div></div>
<div id=e class=contents><div id=e1 class=box></div></div>
<div id=host><p id=light class=box>l</p><p id=unslotted slot=nowhere class=box>u</p></div>
<div id=host2><p id=light2 class=box>l</p></div>
</body></html>"#;

struct Page {
    dom: Dom,
    doc: NodeId,
    layout: Layout,
}

fn page() -> Page {
    lp_layout::install(std::sync::Arc::new(NoFetch));
    let dom_ref: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom_ref, "http://test/", PAGE);
    let mut dom = Rc::try_unwrap(dom_ref).ok().expect("sole owner").into_inner();
    dom.set_document_active(doc, true).unwrap();
    let mut layout = Layout::with_viewport(doc, Viewport::new(800, 600));
    layout.style_mut().discover_style_elements(&dom);
    Page { dom, doc, layout }
}

impl Page {
    fn find(&self, css: &str) -> NodeId {
        self.dom.query_selector(self.doc, &parse_selectors(css).unwrap()).unwrap().unwrap()
    }
    fn visible(&self, css: &str) -> bool {
        self.layout.is_visible(&self.dom, self.find(css))
    }
    /// Whether the layout engine answered the check itself (no fallback to
    /// the derived computed style).
    fn engine_answers(&self, el: NodeId) -> Option<bool> {
        let source = self.layout.synced_geometry_source(&self.dom).expect("Blitz geometry");
        source.element_displayed(lp_dom::snapshot::render_key(el))
    }
    /// The derivation `is_visible` used before: `display: none` on the
    /// element or a flat-tree ancestor, from the derived computed styles.
    fn derived_hidden(&self, el: NodeId) -> bool {
        self.layout.style().computed(&self.dom, el).is_none_or(|c| c.in_display_none)
    }
    /// Every element of the light tree: the engine answers, and agrees
    /// with the derivation.
    fn check_all_agree(&self, what: &str) {
        let all = self.dom.query_selector_all(self.doc, &parse_selectors("body *").unwrap()).unwrap();
        for el in all {
            if self.dom.get_attribute(el, "slot").ok().flatten().is_some() {
                continue;
            }
            let visible = self.layout.is_visible(&self.dom, el);
            assert_eq!(!visible, self.derived_hidden(el), "{what}: {:?}", self.dom.get_attribute(el, "id"));
            assert_eq!(self.engine_answers(el), Some(visible), "{what}: {:?}", self.dom.get_attribute(el, "id"));
        }
    }
    fn rect(&self, css: &str) -> (f64, f64) {
        let r = self.layout.bounding_client_rect(&self.dom, self.find(css));
        (r.width, r.height)
    }
}

#[test]
fn a_display_none_ancestor_hides_its_whole_subtree() {
    let p = page();
    assert!(!p.visible("#a") && !p.visible("#a1") && !p.visible("#a2"));
    assert!(p.visible("#b1") && p.visible("#b2"));
    // display: contents generates no box but is not display: none.
    assert!(p.visible("#e") && p.visible("#e1"));
    assert_eq!(p.rect("#a1"), (0.0, 0.0));
    assert_eq!(p.rect("#b1"), (100.0, 20.0));
    assert_eq!(p.rect("#e1"), (100.0, 20.0));
    p.check_all_agree("initial");
}

#[test]
fn an_ancestor_hidden_after_its_subtree_was_styled_hides_it() {
    let mut p = page();
    let (b, b1) = (p.find("#b"), p.find("#b1"));
    assert_eq!(p.rect("#b1"), (100.0, 20.0));
    p.dom.set_attribute(b, "class", "gone").unwrap();
    assert!(!p.layout.is_visible(&p.dom, b1) && !p.visible("#b2"));
    assert_eq!(p.rect("#b1"), (0.0, 0.0));
    p.check_all_agree("hidden");
    p.dom.remove_attribute(b, "class").unwrap();
    assert!(p.layout.is_visible(&p.dom, b1) && p.visible("#b2"));
    assert_eq!(p.rect("#b1"), (100.0, 20.0));
    p.check_all_agree("shown again");
}

#[test]
fn an_element_styled_then_moved_under_a_hidden_parent_is_hidden() {
    let mut p = page();
    let (c, d, d1) = (p.find("#c"), p.find("#d"), p.find("#d1"));
    assert_eq!(p.rect("#d1"), (100.0, 20.0));
    // Blitz keeps d1's old styles: the traversal stops at the hidden #c.
    p.dom.append_child(c, d1).unwrap();
    assert!(!p.layout.is_visible(&p.dom, d1) && !p.visible("#d2"));
    assert_eq!(p.rect("#d1"), (0.0, 0.0));
    p.check_all_agree("moved under #c");
    p.dom.append_child(d, d1).unwrap();
    assert!(p.layout.is_visible(&p.dom, d1) && p.visible("#d2"));
    assert_eq!(p.rect("#d1"), (100.0, 20.0));
    p.check_all_agree("moved back");
}

#[test]
fn detached_elements_are_hidden() {
    let mut p = page();
    let (b, b1) = (p.find("#b"), p.find("#b1"));
    assert_eq!(p.rect("#b1"), (100.0, 20.0));
    p.dom.remove_child(b, b1).unwrap();
    assert!(!p.layout.is_visible(&p.dom, b1));
    assert_eq!(p.layout.bounding_client_rect(&p.dom, b1), Default::default());
    let fresh = p.dom.create_element(p.doc, "div").unwrap();
    assert!(!p.layout.is_visible(&p.dom, fresh));
    p.check_all_agree("detached");
}

#[test]
fn shadow_trees_follow_the_flat_tree() {
    let mut p = page();
    let (host, host2) = (p.find("#host"), p.find("#host2"));
    let div = |p: &mut Page, parent: NodeId, style: Option<&str>| {
        let el = p.dom.create_element(p.doc, "div").unwrap();
        if let Some(s) = style {
            p.dom.set_attribute(el, "style", s).unwrap();
        }
        p.dom.append_child(parent, el).unwrap();
        el
    };
    // #host: a shown wrapper around the slot, a hidden one around shadow
    // content.
    let root = p.dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
    let shown = div(&mut p, root, Some("width: 100px; height: 20px"));
    let slot = p.dom.create_element(p.doc, "slot").unwrap();
    p.dom.append_child(shown, slot).unwrap();
    let hidden = div(&mut p, root, Some("display: none"));
    let inside = div(&mut p, hidden, Some("width: 100px; height: 20px"));
    assert!(p.layout.is_visible(&p.dom, shown));
    assert!(!p.layout.is_visible(&p.dom, hidden) && !p.layout.is_visible(&p.dom, inside));
    assert_eq!(p.engine_answers(inside), Some(false));
    assert!(p.visible("#light"));
    assert_eq!(p.rect("#light"), (100.0, 20.0));
    // Not assigned to a slot: not rendered.
    assert!(!p.visible("#unslotted"));
    assert_eq!(p.rect("#unslotted"), (0.0, 0.0));
    for el in [shown, hidden, inside] {
        assert_eq!(!p.layout.is_visible(&p.dom, el), p.derived_hidden(el));
    }

    // #host2: the slot sits in a display: none wrapper, so its slotted light
    // child is in a display: none subtree of the flat tree (Chromium: an
    // empty rect, checkVisibility() false).
    let root2 = p.dom.attach_shadow(host2, ShadowRootMode::Open).unwrap();
    let wrapper = div(&mut p, root2, Some("display: none"));
    let slot2 = p.dom.create_element(p.doc, "slot").unwrap();
    p.dom.append_child(wrapper, slot2).unwrap();
    let light2 = p.find("#light2");
    assert!(!p.layout.is_visible(&p.dom, light2));
    assert!(!p.layout.check_visibility(&p.dom, light2, Default::default()));
    assert_eq!(p.rect("#light2"), (0.0, 0.0));
    // Shown again once the wrapper is.
    p.dom.remove_attribute(wrapper, "style").unwrap();
    assert!(p.layout.is_visible(&p.dom, light2));
    assert_eq!(p.rect("#light2"), (100.0, 20.0));
}

#[test]
fn a_hidden_shadow_host_hides_its_styled_shadow_tree() {
    let mut p = page();
    let host = p.find("#host");
    let root = p.dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
    let inner = p.dom.create_element(p.doc, "div").unwrap();
    p.dom.set_attribute(inner, "style", "width: 100px; height: 20px").unwrap();
    p.dom.append_child(root, inner).unwrap();
    let slot = p.dom.create_element(p.doc, "slot").unwrap();
    p.dom.set_attribute(slot, "name", "fallback").unwrap();
    p.dom.append_child(inner, slot).unwrap();
    let fb = p.dom.create_element(p.doc, "div").unwrap();
    p.dom.set_attribute(fb, "style", "width: 100px; height: 20px").unwrap();
    p.dom.append_child(slot, fb).unwrap();
    // A shadow host nested in the shadow tree.
    let nested = p.dom.create_element(p.doc, "div").unwrap();
    p.dom.append_child(root, nested).unwrap();
    let nroot = p.dom.attach_shadow(nested, ShadowRootMode::Open).unwrap();
    let ninner = p.dom.create_element(p.doc, "div").unwrap();
    p.dom.set_attribute(ninner, "style", "width: 100px; height: 20px").unwrap();
    p.dom.append_child(nroot, ninner).unwrap();
    let light = p.find("#light");
    for el in [inner, fb, ninner] {
        assert!(p.layout.is_visible(&p.dom, el));
        assert_eq!(p.layout.bounding_client_rect(&p.dom, el).width, 100.0);
    }
    p.dom.set_attribute(host, "style", "display: none").unwrap();
    for el in [inner, fb, ninner, light] {
        assert!(!p.layout.is_visible(&p.dom, el), "{el:?}");
        assert_eq!(p.layout.bounding_client_rect(&p.dom, el).width, 0.0);
    }
    p.dom.remove_attribute(host, "style").unwrap();
    p.dom.set_attribute(nested, "style", "display: none").unwrap();
    assert!(!p.layout.is_visible(&p.dom, ninner));
    assert!(p.layout.is_visible(&p.dom, fb));
    p.dom.remove_attribute(nested, "style").unwrap();
    assert!(p.layout.is_visible(&p.dom, ninner));
    // A move of a styled shadow element under a hidden light element.
    let c = p.find("#c");
    p.dom.append_child(c, inner).unwrap();
    assert!(!p.layout.is_visible(&p.dom, inner) && !p.layout.is_visible(&p.dom, fb));
    let d1 = p.find("#d1");
    assert_eq!(!p.layout.is_visible(&p.dom, d1), p.derived_hidden(d1));
}
