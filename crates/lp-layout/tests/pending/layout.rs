//! Ports of upstream's faux-layout / cascade / media tests
//! (lightpanda-io/browser @ 2ad38773). Each test names the upstream file it
//! comes from. Upstream runs these as HTML pages whose scripts execute while
//! parsing; here the whole page is parsed first and the script bodies are
//! replayed against the DOM, which only shifts absolute positions (every
//! asserted value is relative or upstream-equal).

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::layout::{Axis, Layout, Rect, ScrollTarget};
use lp_dom::media::{self, ColorScheme, Pointer, Viewport};
use lp_dom::parse::{parse_document, DomRef};
use lp_dom::style::{CheckVisibilityOptions, SheetSource, TextTransform};
use lp_dom::*;

struct Page {
    dom: Dom,
    doc: NodeId,
    layout: Layout,
}

fn page(html: &str) -> Page {
    page_with(html, Viewport::default())
}

fn page_with(html: &str, viewport: Viewport) -> Page {
    let dom_ref: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom_ref, "http://test/", html);
    let dom = Rc::try_unwrap(dom_ref).ok().expect("sole owner").into_inner();
    let mut layout = Layout::with_viewport(doc, viewport);
    layout.style_mut().discover_style_elements(&dom);
    Page { dom, doc, layout }
}

const VIS: CheckVisibilityOptions = CheckVisibilityOptions { check_visibility: true, check_opacity: false };
const OPACITY: CheckVisibilityOptions = CheckVisibilityOptions { check_visibility: false, check_opacity: true };

impl Page {
    fn id(&self, id: &str) -> NodeId {
        self.dom.get_element_by_id(self.doc, id).unwrap().unwrap_or_else(|| panic!("#{id}"))
    }
    fn body(&self) -> NodeId {
        self.dom.body(self.doc).unwrap().unwrap()
    }
    fn head(&self) -> NodeId {
        self.dom.head(self.doc).unwrap().unwrap()
    }
    fn html(&self) -> NodeId {
        self.dom.document_element(self.doc).unwrap().unwrap()
    }
    fn create(&mut self, tag: &str) -> NodeId {
        self.dom.create_element(self.doc, tag).unwrap()
    }
    fn append(&mut self, parent: NodeId, child: NodeId) {
        self.dom.append_child(parent, child).unwrap();
    }
    /// createElement + append to body.
    fn add(&mut self, tag: &str) -> NodeId {
        let el = self.create(tag);
        let body = self.body();
        self.append(body, el);
        el
    }
    fn add_to(&mut self, parent: NodeId, tag: &str) -> NodeId {
        let el = self.create(tag);
        self.append(parent, el);
        el
    }
    fn set(&mut self, el: NodeId, name: &str, value: &str) {
        self.dom.set_attribute(el, name, value).unwrap();
    }
    fn unset(&mut self, el: NodeId, name: &str) {
        self.dom.remove_attribute(el, name).unwrap();
    }
    fn style(&mut self, el: NodeId, css: &str) {
        self.set(el, "style", css);
    }
    fn text(&mut self, el: NodeId, text: &str) {
        self.dom.set_text_content(el, text).unwrap();
    }
    fn remove(&mut self, el: NodeId) {
        self.dom.remove_node(el).unwrap();
    }
    /// A `<style>` appended to head, registered like StyleSheetList.add.
    fn add_style(&mut self, css: &str) -> NodeId {
        let head = self.head();
        let s = self.add_to(head, "style");
        self.text(s, css);
        self.layout.style_mut().add_sheet(s, SheetSource::StyleElement);
        s
    }
    fn visible(&self, el: NodeId) -> bool {
        self.layout.check_visibility(&self.dom, el, CheckVisibilityOptions::default())
    }
    fn visible_opts(&self, el: NodeId, opts: CheckVisibilityOptions) -> bool {
        self.layout.check_visibility(&self.dom, el, opts)
    }
    fn cs(&self, el: NodeId, prop: &str) -> String {
        self.layout.computed_style_value(&self.dom, el, prop)
    }
    fn rect(&self, el: NodeId) -> Rect {
        self.layout.bounding_client_rect(&self.dom, el)
    }
    fn center(&self, el: NodeId) -> (f64, f64) {
        let r = self.rect(el);
        ((r.left() + r.right()) / 2.0, (r.top() + r.bottom()) / 2.0)
    }
    fn efp(&self, x: f64, y: f64) -> Option<NodeId> {
        self.layout.element_from_point(&self.dom, x, y)
    }
    fn efps(&self, x: f64, y: f64) -> Vec<NodeId> {
        self.layout.elements_from_point(&self.dom, x, y)
    }
    fn contains(&self, a: NodeId, b: NodeId) -> bool {
        self.dom.contains(a, b).unwrap()
    }
    fn tag(&self, el: NodeId) -> String {
        self.dom.tag_name(el).unwrap()
    }
    fn client_w(&self, el: NodeId) -> f64 {
        self.layout.client_width(&self.dom, el)
    }
    fn client_h(&self, el: NodeId) -> f64 {
        self.layout.client_height(&self.dom, el)
    }
    fn scroll_w(&self, el: NodeId) -> f64 {
        self.layout.scroll_width(&self.dom, el)
    }
    fn scroll_h(&self, el: NodeId) -> f64 {
        self.layout.scroll_height(&self.dom, el)
    }
    fn offset_w(&self, el: NodeId) -> f64 {
        self.layout.offset_width(&self.dom, el)
    }
    fn offset_h(&self, el: NodeId) -> f64 {
        self.layout.offset_height(&self.dom, el)
    }
    fn set_scroll_top(&mut self, el: NodeId, v: i32) {
        self.layout.set_scroll_top(&self.dom, el, v);
    }
    fn set_scroll_left(&mut self, el: NodeId, v: i32) {
        self.layout.set_scroll_left(&self.dom, el, v);
    }
    fn scroll_to(&mut self, x: i32, y: i32) {
        self.layout.window_scroll_to(&self.dom, Some(x), Some(y));
    }
    fn clone_children_into_self(&mut self, el: NodeId) {
        // `el.innerHTML += el.innerHTML`
        let kids: Vec<NodeId> = self.dom.children(el).unwrap().collect();
        for k in kids {
            let c = self.dom.clone_node(k, true, None).unwrap();
            self.append(el, c);
        }
    }
}

// ---------------------------------------------------------------------------
// css/MediaQuery.zig unit tests (MediaQuery.zig:376-719)
// ---------------------------------------------------------------------------

fn mq(q: &str) -> bool {
    media::matches(q, &Viewport::default())
}

#[test]
fn media_query_zig_unit_tests() {
    // empty query is false
    assert!(!mq(""));
    assert!(!mq("   "));
    // bare media types
    for q in ["all", "screen", "ALL", "Screen"] {
        assert!(mq(q), "{q}");
    }
    for q in ["print", "speech", "tv"] {
        assert!(!mq(q), "{q}");
    }
    // unknown ident is false
    assert!(!mq("foo"));
    assert!(!mq("braille"));
    // min-width on 1920x1080
    assert!(mq("(min-width: 1px)"));
    assert!(mq("(min-width: 600px)"));
    assert!(mq("(min-width: 1920px)"));
    assert!(!mq("(min-width: 1921px)"));
    assert!(!mq("(min-width: 3000px)"));
    // max-width
    assert!(mq("(max-width: 1920px)"));
    assert!(mq("(max-width: 2000px)"));
    assert!(!mq("(max-width: 1919px)"));
    assert!(!mq("(max-width: 0)"));
    // width (exact)
    assert!(mq("(width: 1920px)"));
    assert!(!mq("(width: 1921px)"));
    assert!(!mq("(width: 1919px)"));
    // min-height / max-height / height
    assert!(mq("(min-height: 1080px)"));
    assert!(!mq("(min-height: 1081px)"));
    assert!(mq("(max-height: 1080px)"));
    assert!(!mq("(max-height: 1079px)"));
    assert!(mq("(height: 1080px)"));
    assert!(!mq("(height: 1081px)"));
    // device-width / device-height
    assert!(mq("(min-device-width: 1px)"));
    assert!(mq("(min-device-width: 1920px)"));
    assert!(!mq("(min-device-width: 1921px)"));
    assert!(mq("(max-device-width: 1920px)"));
    assert!(!mq("(max-device-width: 1919px)"));
    assert!(mq("(device-width: 1920px)"));
    assert!(!mq("(device-width: 1919px)"));
    assert!(mq("(min-device-height: 1080px)"));
    assert!(!mq("(min-device-height: 1081px)"));
    assert!(mq("(max-device-height: 1080px)"));
    assert!(!mq("(max-device-height: 1079px)"));
    assert!(mq("(device-height: 1080px)"));
    assert!(!mq("(device-height: 1081px)"));
    assert!(mq("(device-width)"));
    assert!(mq("(device-height)"));
    assert!(mq("(MIN-DEVICE-WIDTH: 1px)"));
    assert!(mq("(min-device-width: 120em)"));
    assert!(!mq("(min-device-width: 121em)"));
    assert!(!mq("(min-device-width: -1999px)"));
    assert!(!mq("(min-device-width: foo)"));
    assert!(mq("(min-device-width: 0)"));
    let small = Viewport::new(375, 667);
    assert!(!media::matches("(min-device-width: 1024px)", &small));
    assert!(media::matches("(max-device-width: 480px) and (orientation: portrait)", &small));
    // orientation
    assert!(mq("(orientation: landscape)"));
    assert!(!mq("(orientation: portrait)"));
    let portrait = Viewport::new(600, 800);
    assert!(!media::matches("(orientation: landscape)", &portrait));
    assert!(media::matches("(orientation: portrait)", &portrait));
    let square = Viewport::new(500, 500);
    assert!(media::matches("(orientation: landscape)", &square));
    assert!(!media::matches("(orientation: portrait)", &square));
    // combined with `and`
    assert!(mq("screen and (min-width: 600px)"));
    assert!(!mq("print and (min-width: 600px)"));
    assert!(mq("(min-width: 600px) and (max-width: 2000px)"));
    assert!(!mq("(min-width: 600px) and (max-width: 1000px)"));
    assert!(mq("all and (orientation: landscape)"));
    // `not` negates
    assert!(mq("not print"));
    assert!(!mq("not screen"));
    assert!(!mq("not (min-width: 600px)"));
    assert!(mq("not (min-width: 3000px)"));
    // comma is OR
    assert!(mq("print, screen"));
    assert!(mq("(max-width: 100px), (min-width: 600px)"));
    assert!(!mq("(max-width: 100px), (min-width: 3000px)"));
    assert!(mq("foo, screen"));
    // `only` is no-op
    assert!(mq("only screen"));
    assert!(mq("only screen and (min-width: 600px)"));
    assert!(!mq("only print"));
    // em units (1em=16px), rem treated as em
    assert!(mq("(min-width: 30em)"));
    assert!(mq("(min-width: 120em)"));
    assert!(!mq("(min-width: 121em)"));
    assert!(mq("(min-width: 30rem)"));
    assert!(!mq("(min-width: 121rem)"));
    // bare 0 is valid
    assert!(mq("(min-width: 0)"));
    assert!(!mq("(max-width: 0)"));
    // unknown feature is false
    assert!(!mq("(monochrome)"));
    assert!(!mq("(color)"));
    // malformed value is false
    assert!(!mq("(min-width: foo)"));
    assert!(!mq("(min-width:)"));
    assert!(!mq("(min-width: -100px)"));
    assert!(!mq("(min-width: 100xx)"));
    // boolean form
    assert!(mq("(width)"));
    assert!(mq("(height)"));
    assert!(mq("(orientation)"));
    // leading whitespace and case
    assert!(mq("  (MIN-WIDTH: 600PX)  "));
    assert!(mq("SCREEN AND (Min-Width: 600px)"));
    // malformed query is false
    assert!(!mq("("));
    assert!(!mq("(min-width: 600px"));
    assert!(!mq("@@@"));
    // common responsive breakpoint
    assert!(mq("(min-width: 768px)"));
    assert!(!mq("(max-width: 767px)"));
    // comments are stripped
    assert!(mq("screen and /*hidden*/ (min-width: 1px)"));
    assert!(mq("/* leading */ screen"));
    assert!(mq("(min-width: /* hi */ 600px)"));
    assert!(mq("screen /*, print*/"));
    assert!(!mq("/* unterminated"));
    // em / rem overflow fails closed
    assert!(!mq("(min-width: 268435456em)"));
    assert!(!mq("(min-width: 268435456rem)"));
    assert!(!mq("(min-width: 268435455em)"));
    // unimplemented units fail closed
    for q in ["(min-width: 5cm)", "(min-width: 50mm)", "(min-width: 10pt)", "(min-width: 1in)", "(min-width: 50vw)"] {
        assert!(!mq(q), "{q}");
    }
    // range syntax (Media Queries 4) at the 1920px default width; upstream rejected it
    assert!(mq("(width >= 600px)"));
    for q in ["(width <= 600px)", "(width < 100px)", "(600px <= width <= 1200px)"] {
        assert!(!mq(q), "{q}");
    }
    assert!(mq("(600px <= width <= 1920px)"));
    // whitespace-tight and -loose features
    assert!(mq("(min-width:600px)"));
    assert!(mq("( min-width : 600px )"));
    assert!(mq("(  min-width  :  600px  )"));
    // additional comment placements
    assert!(mq("screen /*a*/ /*b*/ and (min-width: 1px)"));
    assert!(mq("(/*a*/ min-width: 600px /*b*/)"));
    assert!(mq("(/* foo:bar */ min-width: 600px)"));
    assert!(mq("(min-width: 600px /* ) ( */ )"));
    // u32 boundaries on length
    assert!(!mq("(min-width: 4294967295px)"));
    assert!(!mq("(min-width: 4294967296px)"));
    assert!(!mq("(min-width: 9999999999px)"));
    // empty parens
    assert!(!mq("()"));
    assert!(!mq("(   )"));
    assert!(!mq("screen and ()"));
    // long AND chains
    assert!(mq("screen and (min-width: 600px) and (max-width: 2000px) and (orientation: landscape)"));
    assert!(!mq("screen and (min-width: 600px) and (max-width: 1000px) and (orientation: landscape)"));
    // not all is always false
    assert!(!mq("not all"));
    // not applies to the whole query
    assert!(mq("not (min-width: 3000px) and (orientation: landscape)"));
    assert!(!mq("not (min-width: 1px) and (orientation: landscape)"));
    // multibyte UTF-8 tokens fail closed
    assert!(!mq("(café-width: 600px)"));
    assert!(!mq("café"));
    // trailing unterminated comment fails closed
    assert!(!mq("screen /* unterminated"));
    assert!(!mq("(min-width: 600px) /* unterminated"));
    assert!(!mq("screen /* a */ /* b"));
}

/// Deliberate deviations from MediaQuery.zig (see media.rs module docs).
#[test]
fn media_query_deviations() {
    // The TOS e2e requirement: decimal rem breakpoints match at 1440px.
    let laptop = Viewport::new(1440, 900);
    assert!(media::matches("(min-width: 37.5rem)", &laptop));
    assert!(media::matches("screen and (min-width: 37.5rem) and (max-width: 90rem)", &laptop));
    assert!(!media::matches("(max-width: 37.5rem)", &laptop));
    // MediaQuery.zig:633 "decimal lengths are rejected" no longer holds.
    assert!(mq("(min-width: 600.5px)"));
    assert!(mq("(width: 1920.0px)"));
    assert!(!mq("(min-width: 1.px)"));
    // Emulation features (MediaQuery.zig:531-538 treated them as unknown).
    // Defaults: light, no-preference, hover, fine.
    assert!(!mq("(prefers-color-scheme: dark)"));
    assert!(mq("(prefers-color-scheme: light)"));
    assert!(!mq("(prefers-reduced-motion: reduce)"));
    assert!(mq("(prefers-reduced-motion: no-preference)"));
    assert!(mq("(hover: hover)"));
    assert!(mq("(pointer: fine)"));
    assert!(!mq("(pointer: coarse)"));
    let mobile = Viewport {
        width: 390,
        height: 844,
        device_scale_factor: 3.0,
        color_scheme: ColorScheme::Dark,
        reduced_motion: true,
        hover: false,
        pointer: Pointer::Coarse,
        ..Viewport::default()
    };
    assert!(media::matches("(prefers-color-scheme: dark)", &mobile));
    assert!(media::matches("(prefers-reduced-motion: reduce)", &mobile));
    assert!(media::matches("(hover: none) and (pointer: coarse)", &mobile));
    assert!(media::matches("(any-hover: none)", &mobile));
    assert!(!media::matches("(hover)", &mobile));
    assert!(media::matches("(max-width: 48em) and (orientation: portrait)", &mobile));
}

// tests/css/media_query_list.html
#[test]
fn match_media_list() {
    let p = page("<!DOCTYPE html><body></body>");
    let s = p.layout.style();
    assert!(s.match_media("(min-width: 600px)"));
    assert!(!s.match_media("(min-width: 3000px)"));
    assert!(!s.match_media("(max-width: 1024px)"));
    assert!(s.match_media("(max-width: 2000px)"));
    assert!(s.match_media("(orientation: landscape)"));
    assert!(!s.match_media("(orientation: portrait)"));
    assert!(s.match_media("all"));
    assert!(s.match_media("screen"));
    assert!(!s.match_media("print"));
    assert!(s.match_media("(min-device-width: 1920px)"));
    assert!(!s.match_media("(min-device-width: 1921px)"));
    assert!(s.match_media("(device-height: 1080px)"));
    assert!(!s.match_media("(prefers-color-scheme: dark)"));
    assert!(s.match_media("screen and (min-width: 600px)"));
    assert!(!s.match_media("print and (min-width: 600px)"));
    assert!(s.match_media("print, screen"));
    assert!(!s.match_media("not screen"));
    assert!(s.match_media("only screen and (min-width: 600px)"));
    assert!(s.match_media("(min-width: 30em)"));
    assert!(!s.match_media("(min-width: 121em)"));
    assert!(s.match_media("(min-width: /* hi */ 600px)"));
    assert!(!s.match_media("(min-width: 50vw)"));
}

#[test]
fn match_media_follows_viewport_override() {
    // MediaQueryList.zig viewportChanged: matches re-evaluates after
    // Emulation.setDeviceMetricsOverride.
    let mut p = page("<!DOCTYPE html><body></body>");
    assert!(!p.layout.style().match_media("(max-width: 500px)"));
    p.layout.set_viewport(Viewport::new(400, 800));
    assert!(p.layout.style().match_media("(max-width: 500px)"));
    assert_eq!(p.layout.inner_width(), 400);
}

// ---------------------------------------------------------------------------
// tests/css/media_at_rule_cascade.html
// ---------------------------------------------------------------------------

const MEDIA_CASCADE: &str = r#"<!DOCTYPE html><head><style>
  @media (min-width: 768px) { #hide-on-desktop { display: none; } }
  @media (max-width: 767px) { #hide-on-mobile { display: none; } }
  @media (max-width: 100px), (min-width: 600px) { #hidden-by-or { display: none; } }
  @media (max-width: 100px), (min-width: 3000px) { #not-hidden-by-or { display: none; } }
  @media not print { #hidden-by-not { display: none; } }
  @media screen { #hidden-by-screen { display: none; } }
  @media screen { @media (min-width: 600px) { #hidden-by-nested { display: none; } } }
  @media (prefers-color-scheme: dark) { #not-hidden-unsupported { display: none; } }
  @media (min-width: 1px) { #visibility-hidden-cascade { visibility: hidden; } }
  @media /* { fake */ (min-width: 1px) { #commented-prelude { display: none; } }
  @media screen /*x*/ and (min-width: 1px) { #commented-query { display: none; } }
  @media (min-width: 600px) {
    #inner-block-comments {
      /* leading comment */
      display: none; /* trailing comment */
      /* between */
      color: red;
    }
  }
  @media screen { @media print { #nested-inner-miss { display: none; } } }
  @media   screen   and    (min-width:   100px)   { #ws-heavy { display: none; } }
  @MEDIA SCREEN AND (MIN-WIDTH: 100PX) { #upper-case { display: none; } }
</style></head>
<body>
  <div id="hide-on-desktop">desktop</div>
  <div id="hide-on-mobile">mobile</div>
  <div id="hidden-by-or">or-hit</div>
  <div id="not-hidden-by-or">or-miss</div>
  <div id="hidden-by-not">not-hit</div>
  <div id="hidden-by-screen">screen-hit</div>
  <div id="hidden-by-nested">nested-hit</div>
  <div id="not-hidden-unsupported">unsupported</div>
  <div id="visibility-hidden-cascade">visibility</div>
  <div id="commented-prelude">commented-prelude</div>
  <div id="commented-query">commented-query</div>
  <div id="inner-block-comments">inner-block-comments</div>
  <div id="nested-inner-miss">nested-inner-miss</div>
  <div id="ws-heavy">ws-heavy</div>
  <div id="upper-case">upper-case</div>
  <div id="dyn-target">dyn-target</div>
  <div id="dyn2-target">dyn2-target</div>
  <div id="deep-nest-target">deep-nest-target</div>
  <style id="dyn"></style>
  <style id="dyn2"></style>
  <style id="deep-nest-style"></style>
</body>"#;

#[test]
fn media_at_rule_cascade() {
    let mut p = page(MEDIA_CASCADE);
    for id in [
        "hide-on-desktop",
        "hidden-by-or",
        "hidden-by-not",
        "hidden-by-screen",
        "hidden-by-nested",
        "commented-prelude",
        "commented-query",
        "inner-block-comments",
        "ws-heavy",
        "upper-case",
    ] {
        assert!(!p.visible(p.id(id)), "#{id} should be hidden");
    }
    for id in ["hide-on-mobile", "not-hidden-by-or", "not-hidden-unsupported", "nested-inner-miss"] {
        assert!(p.visible(p.id(id)), "#{id} should be visible");
    }
    let v = p.id("visibility-hidden-cascade");
    assert!(p.visible(v));
    assert!(!p.visible_opts(v, VIS));

    // cascade_dynamic_insertRule_matching / cascade_replaceSync_matching:
    // CSSOM edits reach the engine as new sheet text.
    let dyn1 = p.id("dyn");
    p.layout.style_mut().set_sheet_text(dyn1, "@media (min-width: 600px) { #dyn-target { display: none; } }");
    assert!(!p.visible(p.id("dyn-target")));
    let dyn2 = p.id("dyn2");
    p.layout.style_mut().set_sheet_text(dyn2, "@media (min-width: 600px) { #dyn2-target { display: none; } }");
    assert!(!p.visible(p.id("dyn2-target")));

    // cascade_deep_nesting_capped: 50 levels, past MAX_AT_RULE_NESTING.
    let mut css = String::new();
    for _ in 0..50 {
        css += "@media screen { ";
    }
    css += "#deep-nest-target { display: none; }";
    for _ in 0..50 {
        css += " }";
    }
    let deep = p.id("deep-nest-style");
    p.layout.style_mut().set_sheet_text(deep, &css);
    // 50 levels apply in Chrome; the legacy cascade capped the nesting.
    assert_eq!(p.visible(p.id("deep-nest-target")), false);
}

#[test]
fn media_rules_follow_viewport() {
    let mut p = page(MEDIA_CASCADE);
    p.layout.set_viewport(Viewport::new(500, 900));
    assert!(p.visible(p.id("hide-on-desktop")));
    assert!(!p.visible(p.id("hide-on-mobile")));
}

// ---------------------------------------------------------------------------
// tests/css/layer_at_rule_cascade.html
// ---------------------------------------------------------------------------

#[test]
fn layer_at_rule_cascade() {
    let mut p = page(
        r#"<!DOCTYPE html><head><style>
  @layer utilities { #hidden-by-layer { display: none; } }
  @layer { #hidden-by-anon-layer { display: none; } }
  @layer theme.dark { #hidden-by-dotted-layer { display: none; } }
  @layer theme, base, components, utilities;
  #hidden-after-statement { display: none; }
  @layer responsive { @media (min-width: 600px) { #hidden-media-in-layer { display: none; } } }
  @layer responsive { @media print { #not-hidden-print-in-layer { display: none; } } }
  @media screen { @layer inner { #hidden-layer-in-media { display: none; } } }
  @layer outer { @layer inner { #hidden-nested-layer { display: none; } } }
  @layer utilities { #hidden-important { display: none !important; } }
  @layer utilities { #layer-visibility-hidden { visibility: hidden; } }
</style></head>
<body>
  <div id="hidden-by-layer">named</div>
  <div id="hidden-by-anon-layer">anonymous</div>
  <div id="hidden-by-dotted-layer">dotted</div>
  <div id="hidden-after-statement">after-statement</div>
  <div id="hidden-media-in-layer">media-in-layer</div>
  <div id="not-hidden-print-in-layer">print-in-layer</div>
  <div id="hidden-layer-in-media">layer-in-media</div>
  <div id="hidden-nested-layer">nested-layer</div>
  <div id="hidden-important">important</div>
  <div id="layer-visibility-hidden">visibility</div>
  <div id="dyn-target">dyn-target</div>
  <div id="deep-nest-target">deep-nest-target</div>
  <style id="dyn"></style>
  <style id="deep-nest-style"></style>
</body>"#,
    );
    for id in [
        "hidden-by-layer",
        "hidden-by-anon-layer",
        "hidden-by-dotted-layer",
        "hidden-after-statement",
        "hidden-media-in-layer",
        "hidden-layer-in-media",
        "hidden-nested-layer",
        "hidden-important",
    ] {
        assert!(!p.visible(p.id(id)), "#{id} should be hidden");
    }
    assert_eq!(p.cs(p.id("hidden-by-layer"), "display"), "none");
    assert!(p.visible(p.id("not-hidden-print-in-layer")));
    let v = p.id("layer-visibility-hidden");
    assert!(p.visible(v));
    assert!(!p.visible_opts(v, VIS));

    let dyn1 = p.id("dyn");
    p.layout.style_mut().set_sheet_text(dyn1, "@layer utilities { #dyn-target { display: none; } }");
    assert!(!p.visible(p.id("dyn-target")));

    let mut css = String::new();
    for _ in 0..50 {
        css += "@layer a { ";
    }
    css += "#deep-nest-target { display: none; }";
    for _ in 0..50 {
        css += " }";
    }
    let deep = p.id("deep-nest-style");
    p.layout.style_mut().set_sheet_text(deep, &css);
    // 50 levels apply in Chrome; the legacy cascade capped the nesting.
    assert_eq!(p.visible(p.id("deep-nest-target")), false);
}

// tests/css/layer_order_cascade.html
#[test]
fn layer_order_cascade() {
    let mut p = page(
        r#"<!DOCTYPE html><head><style>
  @layer any { #unlayered-wins { display: none; } }
  .unlayered-wins { display: block; }
  @layer early { #later-layer-wins { display: none; } }
  @layer late { .later-layer-wins { display: block; } }
  @layer early2, late2;
  @layer late2 { .statement-order { display: block; } }
  @layer early2 { #statement-order { display: none; } }
  @layer one { }
  @layer two { .reopened { display: block; } }
  @layer one { #reopened { display: none; } }
  @layer parent {
    @layer child { #parent-direct { display: none; } }
    .parent-direct { display: block; }
  }
  @layer m { }
  @layer m.n { #dotted-sublayer { display: none; } }
  @layer m { .dotted-sublayer { display: block; } }
  @layer i1.x { }
  @layer i2 { .interleaved { display: block; } }
  @layer i1.y { #interleaved { display: none; } }
  @layer { #anon-order { display: none; } }
  @layer { .anon-order { display: block; } }
  @layer med { @media screen { #media-in-layer-rank { display: none; } } }
  .media-in-layer-rank { display: block; }
  @media print { @layer flip2, flip1; }
  @layer flip1 { .flip { display: block; } }
  @layer flip2 { #flip { display: none; } }
  @layer vis { #vis-unlayered { visibility: hidden; } }
  .vis-unlayered { visibility: visible; }
</style></head>
<body>
  <div id="unlayered-wins" class="unlayered-wins">unlayered</div>
  <div id="later-layer-wins" class="later-layer-wins">later-layer</div>
  <div id="statement-order" class="statement-order">statement</div>
  <div id="reopened" class="reopened">reopened</div>
  <div id="parent-direct" class="parent-direct">parent-direct</div>
  <div id="dotted-sublayer" class="dotted-sublayer">dotted</div>
  <div id="interleaved" class="interleaved">interleaved</div>
  <div id="anon-order" class="anon-order">anon</div>
  <div id="media-in-layer-rank" class="media-in-layer-rank">media-rank</div>
  <div id="flip" class="flip">flip</div>
  <div id="vis-unlayered" class="vis-unlayered">vis</div>
  <div id="dyn-order" class="dyn-order">dyn</div>
  <div id="dyn2-order" class="dyn2-order">dyn2</div>
  <style id="dyn"></style>
  <style id="dyn2"></style>
</body>"#,
    );
    for id in [
        "unlayered-wins",
        "later-layer-wins",
        "statement-order",
        "reopened",
        "parent-direct",
        "dotted-sublayer",
        "interleaved",
        "anon-order",
        "media-in-layer-rank",
    ] {
        assert!(p.visible(p.id(id)), "#{id} should be visible");
    }
    assert_eq!(p.cs(p.id("unlayered-wins"), "display"), "block");
    assert!(!p.visible(p.id("flip")));
    assert!(p.visible_opts(p.id("vis-unlayered"), VIS));

    let dyn1 = p.id("dyn");
    p.layout
        .style_mut()
        .set_sheet_text(dyn1, "@layer d1 { #dyn-order { display: none; } }\n.dyn-order { display: block; }");
    assert!(p.visible(p.id("dyn-order")));
    let dyn2 = p.id("dyn2");
    p.layout.style_mut().set_sheet_text(
        dyn2,
        "@layer a { #dyn2-order { display: none; } }@layer b { .dyn2-order { display: block; } }",
    );
    assert!(p.visible(p.id("dyn2-order")));
}

// tests/css/important_cascade.html (the external sheet is served by
// src/testing.zig:1136-1137).
#[test]
fn important_cascade() {
    let dom_ref: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(
        &dom_ref,
        "http://test/",
        r#"<!DOCTYPE html><head>
  <link rel="stylesheet" href="/styles/important-cascade.css">
  <style id="sheet">
    .product__media-sublist { display: block; }
    @layer first, second;
    @layer first { .layered { display: none !important; } }
    @layer second { .layered { display: block !important; } }
    @layer outer { .nested { display: block !important; } @layer inner { .nested { display: none !important; } } }
    @layer first { .over-unlayered { display: none !important; } }
    .over-unlayered { display: block !important; }
    .mixed { visibility: hidden !important; display: none; }
    .mixed-later { visibility: visible; display: block; }
    .force-flex { display: flex !important; }
    .force-visible { visibility: visible !important; }
    .force-width { width: 50px !important; }
  </style>
</head>
<body>
  <div id="gallery" class="product__media-sublist no-js-flex">hidden gallery</div>
  <div id="inline" class="no-js-flex" style="display: block">hidden despite normal inline style</div>
  <div id="inline-important" class="no-js-flex" style="display: block !important">visible</div>
  <div id="layered" class="layered">hidden by the earlier important layer</div>
  <div id="nested" class="nested">hidden by the sublayer</div>
  <div id="over-unlayered" class="over-unlayered">hidden</div>
  <div id="mixed" class="mixed mixed-later">shown, but visibility: hidden</div>
  <div id="flex-over-inline" class="force-flex" style="display: none">shown</div>
  <div id="visible-over-inline" class="force-visible" style="visibility: hidden">shown</div>
  <div id="width-over-inline" class="force-width" style="width: 10px">50px wide</div>
  <div id="width-inline-important" class="force-width" style="width: 10px !important">10px wide</div>
</body>"#,
    );
    let dom = Rc::try_unwrap(dom_ref).ok().unwrap().into_inner();
    let mut layout = Layout::new(doc);
    let link = dom.query_selector(doc, &lp_dom::select::parse_selectors("link").unwrap()).unwrap().unwrap();
    layout.style_mut().set_sheet_text(link, ".no-js-flex { display: none !important; }");
    layout.style_mut().discover_style_elements(&dom);
    let mut p = Page { dom, doc, layout };

    // author_important_cascade
    let gallery = p.id("gallery");
    assert_eq!(p.cs(gallery, "display"), "none");
    assert_eq!(p.offset_w(gallery), 0.0);
    assert_eq!(p.cs(p.id("inline"), "display"), "none");
    assert_eq!(p.cs(p.id("inline-important"), "display"), "block");
    assert_eq!(p.cs(p.id("layered"), "display"), "none");
    let sheet = p.id("sheet");
    let original = p.dom.text_content(sheet).unwrap().unwrap();
    p.layout.style_mut().set_sheet_text(sheet, &format!("{original}\n.no-js-flex {{ display: block !important; }}"));
    assert_eq!(p.cs(gallery, "display"), "block");
    p.layout.style_mut().set_sheet_text(sheet, &original);
    assert_eq!(p.cs(gallery, "display"), "none");

    // important_layer_order
    assert_eq!(p.cs(p.id("nested"), "display"), "none");
    assert_eq!(p.cs(p.id("over-unlayered"), "display"), "none");

    // mixed_importance_block
    let mixed = p.id("mixed");
    assert!(p.visible(mixed));
    assert!(!p.visible_opts(mixed, VIS));
    assert_eq!(p.cs(mixed, "visibility"), "hidden");

    // computed_style_skips_losing_inline
    let flex = p.id("flex-over-inline");
    assert!(p.visible(flex));
    assert_ne!(p.cs(flex, "display"), "none");
    assert_eq!(p.layout.style().inline_declared_value(&p.dom, flex, "display").unwrap().0, "none");
    let visible = p.id("visible-over-inline");
    assert!(p.visible_opts(visible, VIS));
    assert_eq!(p.cs(visible, "visibility"), "visible");
    let wide = p.id("width-over-inline");
    assert_eq!(p.offset_w(wide), 50.0);
    assert_eq!(p.cs(wide, "width"), "50px");
    let narrow = p.id("width-inline-important");
    assert_eq!(p.offset_w(narrow), 10.0);
    assert_eq!(p.cs(narrow, "width"), "10px");
}

// ---------------------------------------------------------------------------
// tests/element/check_visibility.html
// ---------------------------------------------------------------------------

const CHECK_VISIBILITY: &str = r#"<!DOCTYPE html><body></body>
<style id="style-basic">
  .hidden-by-class { display: none; }
  .visible-by-class { display: block; }
</style>
<style id="style-specificity">
  .spec-hidden { display: none; }
  #spec-visible { display: block; }
</style>
<style id="style-order-1">.order-test { display: none; }</style>
<style id="style-order-2">.order-test { display: block; }</style>
<style id="style-override">.should-be-hidden { display: none; }</style>
<style id="style-author-beats-ua-hidden">
  .cv-author-flex { display: flex; }
  .cv-author-none { display: none; }
</style>
<style id="style-keyword-case">
  .hidden-upper { DISPLAY: NONE; }
  .invisible-upper { Visibility: Hidden; }
</style>
<style id="style-memo">
  .memo-hide { display: none; }
  .memo-tabs input:checked + span { display: none; }
  .memo-tabs input:indeterminate + span { display: none; }
  .memo-form input:invalid + span { display: none; }
  .memo-form select:invalid + span { display: none; }
  x-memo:not(:defined) { display: none; }
  .memo-badge:empty { display: none; }
</style>
<style id="style-insert-rule-layers">
  @layer base { .layered-hide { display: none; } }
</style>
<style id="style-important-text">
  .imp-text { display: none !important; display: block; }
</style>"#;

#[test]
fn check_visibility_inline_styles() {
    let mut p = page(CHECK_VISIBILITY);
    // inline_display_none
    let el = p.add("div");
    assert!(p.visible(el));
    p.style(el, "display: none");
    assert!(!p.visible(el));
    p.style(el, "display: block");
    assert!(p.visible(el));
    p.remove(el);

    // inline_visibility_hidden
    let el = p.add("div");
    p.style(el, "visibility: hidden");
    assert!(p.visible(el));
    assert!(!p.visible_opts(el, VIS));
    p.style(el, "visibility: collapse");
    assert!(!p.visible_opts(el, VIS));
    p.style(el, "visibility: visible");
    assert!(p.visible_opts(el, VIS));
    p.remove(el);

    // inline_opacity_zero
    let el = p.add("div");
    p.style(el, "opacity: 0");
    assert!(p.visible(el));
    assert!(!p.visible_opts(el, OPACITY));
    p.style(el, "opacity: 0.5");
    assert!(p.visible_opts(el, OPACITY));
    p.style(el, "opacity: 1");
    assert!(p.visible_opts(el, OPACITY));
    p.remove(el);

    // parent_hidden_hides_child
    let parent = p.add("div");
    let child = p.add_to(parent, "span");
    assert!(p.visible(child));
    p.style(parent, "display: none");
    assert!(!p.visible(child));
    p.style(parent, "display: block; visibility: hidden");
    assert!(p.visible(child));
    assert!(!p.visible_opts(child, VIS));
    p.style(parent, "display: block; visibility: visible; opacity: 0");
    assert!(p.visible(child));
    assert!(!p.visible_opts(child, OPACITY));
    p.remove(parent);
}

#[test]
fn check_visibility_stylesheets() {
    let mut p = page(CHECK_VISIBILITY);
    // style_tag_basic
    let el = p.add("div");
    assert!(p.visible(el));
    p.set(el, "class", "hidden-by-class");
    assert!(!p.visible(el));
    p.set(el, "class", "visible-by-class");
    assert!(p.visible(el));
    p.remove(el);

    // specificity_id_beats_class
    let el = p.add("div");
    p.set(el, "id", "spec-visible");
    p.set(el, "class", "spec-hidden");
    assert!(p.visible(el));
    p.remove(el);

    // rule_order_later_wins
    let el = p.add("div");
    p.set(el, "class", "order-test");
    assert!(p.visible(el));
    p.remove(el);

    // inline_overrides_stylesheet
    let el = p.add("div");
    p.set(el, "class", "should-be-hidden");
    assert!(!p.visible(el));
    p.style(el, "display: block");
    assert!(p.visible(el));
    p.remove(el);

    // dynamic_style_element
    let el = p.add("div");
    p.set(el, "class", "dynamic-style-test");
    assert!(p.visible(el));
    let style = p.add_style(".dynamic-style-test { display: none; }");
    assert!(!p.visible(el));
    p.remove(style);
    assert!(p.visible(el));
    p.remove(el);

    // keyword_values_are_case_insensitive
    let el = p.add("div");
    p.style(el, "display: NONE");
    assert!(!p.visible(el));
    assert_eq!(p.cs(el, "display"), "none");
    p.style(el, "visibility: HIDDEN");
    assert!(p.visible(el));
    assert!(!p.visible_opts(el, VIS));
    p.unset(el, "style");
    p.set(el, "class", "hidden-upper");
    assert!(!p.visible(el));
    p.set(el, "class", "invisible-upper");
    assert!(p.visible(el));
    assert!(!p.visible_opts(el, VIS));
    p.remove(el);

    // important_survives_cssrules_materialization
    let el = p.add("div");
    p.set(el, "class", "imp-text");
    assert!(!p.visible(el));
    p.remove(el);

    // deep_nesting
    let mut cur = p.body();
    let mut els = Vec::new();
    for _ in 0..5 {
        let e = p.add_to(cur, "div");
        els.push(e);
        cur = e;
    }
    assert!(els.iter().all(|&e| p.visible(e)));
    p.style(els[2], "display: none");
    assert!(p.visible(els[0]) && p.visible(els[1]));
    assert!(!p.visible(els[2]) && !p.visible(els[3]) && !p.visible(els[4]));
    p.remove(els[0]);
}

#[test]
fn check_visibility_ua_rules() {
    let mut p = page(CHECK_VISIBILITY);
    // ua_unrendered_tags_are_display_none
    for t in [
        "area", "base", "datalist", "link", "meta", "noscript", "param", "script", "source", "style", "template",
        "title", "track",
    ] {
        let el = p.add(t);
        assert_eq!(p.cs(el, "display"), "none", "<{t}>");
        assert!(!p.visible(el), "<{t}>");
        p.remove(el);
    }
    let head = p.head();
    assert_eq!(p.cs(head, "display"), "none");
    assert!(!p.visible(head));

    // ua_input_type_hidden_is_display_none
    let inp = p.add("input");
    p.set(inp, "type", "hidden");
    assert_eq!(p.cs(inp, "display"), "none");
    assert!(!p.visible(inp));
    p.set(inp, "type", "text");
    assert!(p.visible(inp));
    p.set(inp, "type", "HIDDEN");
    assert!(!p.visible(inp));
    p.remove(inp);

    // ua_closed_details_hides_non_summary_children
    let details = p.add("details");
    let summary = p.add_to(details, "summary");
    p.text(summary, "header");
    let child = p.add_to(details, "p");
    p.text(child, "body");
    assert!(p.visible(summary));
    assert!(!p.visible(child));
    assert_eq!(p.cs(child, "display"), "none");
    p.set(details, "open", "");
    assert!(p.visible(child));
    p.unset(details, "open");
    assert!(!p.visible(child));
    let grandchild = p.add_to(child, "span");
    assert!(!p.visible(grandchild));
    p.remove(details);

    // ua_closed_dialog_is_hidden (show() sets [open])
    let dialog = p.add("dialog");
    let child = p.add_to(dialog, "p");
    assert!(!p.visible(dialog));
    assert!(!p.visible(child));
    assert_eq!(p.cs(dialog, "display"), "none");
    p.set(dialog, "open", "");
    assert!(p.visible(dialog));
    assert!(p.visible(child));
    p.unset(dialog, "open");
    assert!(!p.visible(dialog));
    p.remove(dialog);

    // ua_closed_popover_is_hidden
    let pop = p.add("div");
    p.set(pop, "popover", "");
    let child = p.add_to(pop, "p");
    assert!(!p.visible(pop));
    assert!(!p.visible(child));
    assert_eq!(p.cs(pop, "display"), "none");
    p.layout.style_mut().set_popover_open(pop, true);
    assert!(p.visible(pop));
    assert!(p.visible(child));
    p.layout.style_mut().set_popover_open(pop, false);
    assert!(!p.visible(pop));
    p.set(pop, "popover", "manual");
    assert!(!p.visible(pop));
    p.style(pop, "display: block");
    assert!(p.visible(pop));
    let dialog = p.add("dialog");
    p.set(dialog, "popover", "");
    assert!(!p.visible(dialog));
    p.set(dialog, "open", "");
    assert!(p.visible(dialog));
    p.unset(dialog, "open");
    assert!(!p.visible(dialog));
    p.layout.style_mut().set_popover_open(dialog, true);
    assert!(p.visible(dialog));
    p.layout.style_mut().set_popover_open(dialog, false);
    assert!(!p.visible(dialog));
    p.remove(dialog);
    p.remove(pop);

    // hidden_attribute_propagates_through_check_visibility
    let parent = p.add("div");
    p.set(parent, "hidden", "");
    let child = p.add_to(parent, "span");
    assert!(!p.visible(parent));
    assert!(!p.visible(child));
    p.remove(parent);

    // inline_display_overrides_ua_default
    let s = p.add("script");
    p.style(s, "display: block");
    assert!(p.visible(s));
    p.remove(s);

    // author_class_display_overrides_hidden_attr
    let el = p.add("div");
    p.set(el, "class", "cv-author-flex");
    p.set(el, "hidden", "");
    assert!(p.visible(el));
    assert_ne!(p.cs(el, "display"), "none");
    p.remove(el);

    // author_class_display_none_alone_hides / ..._with_hidden_attr_stays_hidden
    let el = p.add("div");
    p.set(el, "class", "cv-author-none");
    assert!(!p.visible(el));
    assert_eq!(p.cs(el, "display"), "none");
    p.set(el, "hidden", "");
    assert!(!p.visible(el));
    assert_eq!(p.cs(el, "display"), "none");
    p.remove(el);

    // author_wildcard_overrides_hidden_attr
    let style = p.add_style("* { display: block; }");
    let el = p.add("div");
    p.set(el, "hidden", "");
    assert!(p.visible(el));
    assert_ne!(p.cs(el, "display"), "none");
    p.remove(el);
    p.remove(style);

    // no_author_rule_hidden_attr_still_hides
    let el = p.add("div");
    p.set(el, "hidden", "");
    assert!(!p.visible(el));
    assert_eq!(p.cs(el, "display"), "none");
    p.remove(el);

    // inline_display_with_hidden_attr_regression_guard
    let el = p.add("div");
    p.set(el, "hidden", "");
    p.style(el, "display: flex");
    assert!(p.visible(el));
    assert_ne!(p.cs(el, "display"), "none");
    p.remove(el);
}

#[test]
fn check_visibility_memo() {
    let mut p = page(CHECK_VISIBILITY);
    // memo_attribute_mutations
    let el = p.add("div");
    let check = |p: &Page, visible: bool| {
        assert_eq!(p.visible(el), visible);
        assert_eq!(p.cs(el, "display"), if visible { "block" } else { "none" });
        assert_eq!(p.offset_w(el) > 0.0, visible);
    };
    check(&p, true);
    p.set(el, "hidden", "");
    check(&p, false);
    p.unset(el, "hidden");
    check(&p, true);
    p.set(el, "class", "memo-hide");
    check(&p, false);
    p.set(el, "class", "");
    check(&p, true);
    p.style(el, "display: none");
    check(&p, false);
    p.style(el, "color: red");
    check(&p, true);
    p.remove(el);

    // memo_insert_rule (insertRule/deleteRule reach the engine as sheet text)
    let el = p.add("div");
    p.set(el, "class", "memo-dyn");
    assert!(p.visible(el));
    let style = p.add_style("");
    p.layout.style_mut().set_sheet_text(style, ".memo-dyn { display: none }");
    assert!(!p.visible(el));
    p.layout.style_mut().set_sheet_text(style, "");
    assert!(p.visible(el));
    p.remove(style);
    p.remove(el);

    // memo_form_state (checkbox part; :invalid is not parsed by crate::select)
    let tabs = p.add("div");
    p.set(tabs, "class", "memo-tabs");
    let input = p.add_to(tabs, "input");
    p.set(input, "type", "checkbox");
    let panel = p.add_to(tabs, "span");
    assert!(p.visible(panel));
    p.dom.set_checked(input, true).unwrap();
    assert!(!p.visible(panel));
    p.dom.set_checked(input, false).unwrap();
    assert!(p.visible(panel));
    p.dom.control_state_mut(input).unwrap().indeterminate = true;
    p.layout.style_mut().invalidate();
    assert!(!p.visible(panel));
    p.dom.control_state_mut(input).unwrap().indeterminate = false;
    p.layout.style_mut().invalidate();
    assert!(p.visible(panel));
    p.remove(tabs);

    // memo_character_data
    let badge = p.add("span");
    p.set(badge, "class", "memo-badge");
    p.text(badge, "3");
    let text = p.dom.first_child(badge).unwrap().unwrap();
    assert!(p.visible(badge));
    p.dom.set_char_data(text, "").unwrap();
    assert!(!p.visible(badge));
    p.dom.set_char_data(text, "4").unwrap();
    assert!(p.visible(badge));
    p.remove(badge);

    // memo_reparent
    let hidden = p.add("div");
    p.style(hidden, "display: none");
    let shown = p.add("div");
    let child = p.add_to(shown, "span");
    assert!(p.visible(child));
    p.append(hidden, child);
    assert!(!p.visible(child));
    p.append(shown, child);
    assert!(p.visible(child));
    for _ in 0..2 {
        assert!(p.visible(child));
        assert_eq!(p.cs(shown, "display"), "block");
        assert_eq!(p.cs(hidden, "display"), "none");
    }
    p.remove(hidden);
    p.remove(shown);

    // insert_rule_fast_path_matches_rebuild
    let el = p.add("div");
    let first = p.add_style("");
    let last = p.add_style("");
    p.set(el, "class", "ins-a");
    let mut last_rules = vec![".ins-a { display: none }".to_owned()];
    let mut first_rules: Vec<String> = Vec::new();
    let sync = |p: &mut Page, sheet: NodeId, rules: &[String]| {
        p.layout.style_mut().set_sheet_text(sheet, &rules.join("\n"));
    };
    sync(&mut p, last, &last_rules);
    assert!(!p.visible(el));
    last_rules.push(".ins-a { display: block }".into());
    sync(&mut p, last, &last_rules);
    assert!(p.visible(el));
    last_rules.push("div.ins-a { display: none }".into());
    sync(&mut p, last, &last_rules);
    assert!(!p.visible(el));
    first_rules.push("div.ins-a { display: block }".into());
    sync(&mut p, first, &first_rules);
    assert!(!p.visible(el));
    last_rules.insert(0, "div.ins-a { display: inline }".into());
    sync(&mut p, last, &last_rules);
    assert!(!p.visible(el));
    p.set(el, "class", "layered-hide");
    assert!(!p.visible(el));
    last_rules.push(".layered-hide { display: block }".into());
    sync(&mut p, last, &last_rules);
    assert!(p.visible(el));
    last_rules.pop();
    sync(&mut p, last, &last_rules);
    assert!(!p.visible(el));
    p.remove(first);
    p.remove(last);
    p.remove(el);

    // memo_focus
    let style = p.add_style("#memo_focus_input:focus + span { display: none; }");
    let wrap = p.add("div");
    let input = p.add_to(wrap, "input");
    p.set(input, "id", "memo_focus_input");
    let span = p.add_to(wrap, "span");
    assert!(p.visible(span));
    p.dom.set_focused(p.doc, Some(input)).unwrap();
    assert!(!p.visible(span));
    p.dom.set_focused(p.doc, None).unwrap();
    assert!(p.visible(span));
    p.remove(wrap);
    p.remove(style);

    // memo_checked_rollback (the toggle part; the click/preventDefault
    // rollback is event dispatch, outside lp-dom's layout)
    let style = p.add_style("#memo_checked_input:checked + span { display: none; }");
    let wrap = p.add("div");
    let input = p.add_to(wrap, "input");
    p.set(input, "type", "checkbox");
    p.set(input, "id", "memo_checked_input");
    let span = p.add_to(wrap, "span");
    assert!(p.visible(span));
    p.dom.set_checked(input, true).unwrap();
    assert!(!p.visible(span));
    p.dom.set_checked(input, false).unwrap();
    assert!(p.visible(span));
    p.remove(wrap);
    p.remove(style);
}

#[test]
fn check_visibility_shadow() {
    let mut p = page(CHECK_VISIBILITY);
    // inherits_across_shadow_boundary
    let none = p.add("div");
    p.style(none, "display: none");
    let root = p.dom.attach_shadow(none, ShadowRootMode::Open).unwrap();
    let button = p.create("button");
    p.append(root, button);
    assert!(!p.visible(button));
    p.style(none, "display: block");
    assert!(p.visible(button));

    let hidden = p.add("div");
    p.style(hidden, "visibility: hidden; --c: red; font-size: 30px");
    let shadow = p.dom.attach_shadow(hidden, ShadowRootMode::Open).unwrap();
    let span = p.create("span");
    p.append(shadow, span);
    assert!(!p.visible_opts(span, VIS));
    assert_eq!(p.cs(span, "visibility"), "hidden");
    assert_eq!(p.cs(span, "--c"), "red");
    assert_eq!(p.cs(span, "font-size"), "30px");
}

// ---------------------------------------------------------------------------
// tests/document/element_from_point.html
// ---------------------------------------------------------------------------

const EFP: &str = r#"<!DOCTYPE html>
<script src="../testing.js"></script>

<body>
  <div id="div1" style="width: 100px; height: 50px;">Div 1</div>
  <div id="div2" style="width: 100px; height: 50px;">Div 2</div>
  <div id="hidden" style="display: none; width: 100px; height: 50px;">Hidden</div>
  <div id="parent" style="width: 100px; height: 50px;">
    <div id="child" style="width: 80px; height: 30px;">Child</div>
  </div>
</body>
<script id="basic_usage"></script>"#;

fn is_ancestor(p: &Page, anc: NodeId, of: NodeId) -> bool {
    let mut cur = p.dom.parent_element(of).unwrap();
    while let Some(c) = cur {
        if c == anc {
            return true;
        }
        cur = p.dom.parent_element(c).unwrap();
    }
    false
}

#[test]
fn element_from_point() {
    let mut p = page(EFP);
    let div1 = p.id("div1");
    let child = p.id("child");
    let parent = p.id("parent");

    // basic_usage
    let r = p.rect(div1);
    let el = p.efp(r.left() + 10.0, r.top() + 5.0).expect("hit");
    assert!(el == div1 || p.tag(el) == "BODY" || p.tag(el) == "HTML");

    // nested_elements
    let (cx, cy) = p.center(child);
    let el = p.efp(cx, cy).expect("hit");
    assert!(el == child || el == parent || p.tag(el) == "BODY" || p.tag(el) == "HTML");

    // outside_viewport
    assert_eq!(p.efp(-1000.0, -1000.0), None);

    // at_element_center
    let (cx, cy) = p.center(div1);
    assert!(p.efp(cx, cy).is_some());

    // dynamically_created
    let new_div = p.add("div");
    p.style(new_div, "width: 50px; height: 50px");
    p.text(new_div, "Dynamic");
    let (cx, cy) = p.center(new_div);
    assert_eq!(p.efp(cx, cy), Some(new_div));
    p.remove(new_div);

    // elementsFromPoint_basic
    let r = p.rect(div1);
    let els = p.efps(r.left() + 10.0, r.top() + 5.0);
    assert!(!els.is_empty());
    for w in els.windows(2) {
        assert!(is_ancestor(&p, w[1], w[0]) || p.tag(w[1]) == "HTML");
    }

    // elementsFromPoint_nested / elementsFromPoint_order
    let (cx, cy) = p.center(child);
    let els = p.efps(cx, cy);
    assert!(els.len() >= 2);
    assert!(els[0] == child || els[0] == parent || p.tag(els[0]) == "BODY" || p.tag(els[0]) == "HTML");
    if els[0] == child {
        assert!(els.contains(&parent));
    }
    for w in els.windows(2) {
        assert!(is_ancestor(&p, w[1], w[0]) || p.tag(w[1]) == "HTML");
    }

    // elementsFromPoint_outside
    assert!(p.efps(-1000.0, -1000.0).is_empty());

    // hidden_elements / elementsFromPoint_hidden (commented out upstream;
    // they hold here)
    let hidden = p.id("hidden");
    assert_eq!(p.rect(hidden), Rect::default());
    let (cx, cy) = p.center(hidden);
    assert_ne!(p.efp(cx, cy), Some(hidden));
    assert!(!p.efps(cx, cy).contains(&hidden));
}

// tests/document/element_from_point_hit_testing.html
#[test]
fn element_from_point_hit_testing() {
    let p = page(
        r#"<!DOCTYPE html>
<script src="../testing.js"></script>
<style>
  .dead { pointer-events: none; }
</style>

<body>
  <div id="plain">Plain</div>
  <div id="under">
    <span id="dead" class="dead">Dead</span>
  </div>
  <div id="wrap" style="pointer-events: none">
    <div id="inherits">Inherits</div>
    <div id="live" style="pointer-events: auto">Live</div>
  </div>
  <div id="under_hidden">
    <span id="hidden" style="visibility: hidden">Hidden</span>
    <span id="collapsed" style="visibility: collapse">Collapsed</span>
  </div>
  <div id="hidden_wrap" style="visibility: hidden">
    <div id="shown" style="visibility: visible">Shown</div>
  </div>
</body>"#,
    );
    // computed_pointer_events
    let pe = |id: &str| p.cs(p.id(id), "pointer-events");
    assert_eq!(pe("plain"), "auto");
    assert_eq!(pe("dead"), "none");
    assert_eq!(pe("wrap"), "none");
    assert_eq!(pe("inherits"), "none");
    assert_eq!(pe("live"), "auto");

    // hit_skips_pointer_events_none
    let dead = p.id("dead");
    let (x, y) = p.center(dead);
    let hit = p.efp(x, y).unwrap();
    assert!(hit != dead && p.contains(hit, dead));
    assert!(!p.efps(x, y).contains(&dead));
    let live = p.id("live");
    let (x, y) = p.center(live);
    assert_eq!(p.efp(x, y), Some(live));
    let stack = p.efps(x, y);
    assert_eq!(stack[0], live);
    assert!(!stack.contains(&p.id("wrap")));
    assert_eq!(*stack.last().unwrap(), p.html());

    // hit_skips_visibility_hidden
    for id in ["hidden", "collapsed"] {
        let el = p.id(id);
        let (x, y) = p.center(el);
        let hit = p.efp(x, y).unwrap();
        assert!(hit != el && p.contains(hit, el), "{id}");
        assert!(!p.efps(x, y).contains(&el), "{id}");
    }
    let shown = p.id("shown");
    let (x, y) = p.center(shown);
    assert_eq!(p.efp(x, y), Some(shown));
    let stack = p.efps(x, y);
    assert_eq!(stack[0], shown);
    assert!(!stack.contains(&p.id("hidden_wrap")));
}

// tests/element/bounding_rect.html
#[test]
fn bounding_rect() {
    let p = page(
        r#"<!DOCTYPE html>
<head>
  <title>Lightpanda Browser demo</title>
  <meta charset="UTF-8">
</head>
<body>
  <h1>Lightpanda Browser Demo</h1>

  <h2><a href="campfire-commerce/">Campfire Commerce</a></h2>
  <a href="campfire-commerce/"><img src="campfire-commerce/images/logo.jpg"></a>
  <p>
    Demo of an e-commerce product offer page with data loaded from XHR request.
    <br>
    All images and texts have been generated with AI.
    Template by <a href="https://codepen.io/Sunil_Pradhan/pen/qBqgLxK">Sunil Pradhan</a>
  </p>

  <h2><a href="amiibo/">Amiibo characters</a></h2>
  <a href="amiibo/"><img src="https://raw.githubusercontent.com/N3evin/AmiiboAPI/master/images/icon_04380001-03000502.png"></a>
  <p>
    Pages of Amiibo characters generated from
    <a href="https://www.amiiboapi.com">Amiibo API</a>.
  </p>
</body>
<script src="../testing.js"></script>
<script id="rect"></script>"#,
    );
    let q = |s: &str| p.dom.query_selector(p.doc, &lp_dom::select::parse_selectors(s).unwrap()).unwrap().unwrap();
    let a1 = q(r#"[href="campfire-commerce/"]"#);
    let a2 = q(r#"[href="amiibo/"]"#);
    let r1 = p.rect(a1);
    let r2 = p.rect(a2);
    assert_ne!(r1.x, r2.x);
    assert_ne!(r1.y, r2.y);
    assert_eq!(p.efp(r1.x, r1.y), Some(a1));
    assert_eq!(p.efp(r2.x, r2.y), Some(a2));
}

// tests/element/bounding_rect_scroll.html
#[test]
fn bounding_rect_scroll() {
    let mut p = page(
        r#"<!DOCTYPE html>
<script src="../testing.js"></script>
<body>
<div id="top">top</div>
<div id="filler"></div>
<button id="far">far</button>
</body>
<script id="rectIsViewportRelative"></script>"#,
    );
    let filler = p.id("filler");
    for _ in 0..300 {
        let el = p.add_to(filler, "p");
        p.text(el, "x");
    }
    let far = p.id("far");
    let before = p.rect(far);
    assert!(before.top() > p.layout.inner_height() as f64);

    p.scroll_to(0, 100);
    let after = p.rect(far);
    assert_eq!(after.top(), before.top() - 100.0);
    assert_eq!(after.x, before.x);
    assert_eq!(p.layout.client_rects(&p.dom, far)[0].top(), after.top());

    p.layout.scroll_into_view(&p.dom, far);
    let in_view = p.rect(far);
    assert!(in_view.top() >= 0.0);
    assert!(in_view.bottom() <= p.layout.inner_height() as f64);
    assert_eq!(p.efp(in_view.x, in_view.y), Some(far));

    p.scroll_to(0, 0);
    assert_eq!(p.rect(far).top(), before.top());
}

// ---------------------------------------------------------------------------
// tests/element/position.html
// ---------------------------------------------------------------------------

const POSITION: &str = r#"<!DOCTYPE html>
<script src="../testing.js"></script>

<div id="test1">Test Element</div>
<div id="test2">Another Element</div>
<style>
  .flex-row { display: flex; }
</style>
<div id="clampBox" style="width: 100px; height: 100px; overflow: scroll;">
  <div style="width: 500px; height: 500px;">content</div>
</div>
<style>
  .sheetBox { height: 100px; overflow: auto; }
  #sheetAuto { height: auto; }
  .sheetVh { height: 10vh; }
</style>
<div id="sheetBox" class="sheetBox"><div style="height: 500px;">content</div></div>
<div id="sheetAuto" class="sheetBox"><div style="height: 500px;">content</div></div>
<div id="inlineAuto" class="sheetBox" style="height: 50%"><div style="height: 500px;">content</div></div>
<div id="sheetVh" class="sheetVh"></div>
<div id="textBox" style="height: 100px; width: 200px; overflow: auto;">
  one two three four five six seven eight nine ten eleven twelve thirteen
  fourteen fifteen sixteen seventeen eighteen nineteen twenty twenty-one
  twenty-two twenty-three twenty-four twenty-five twenty-six twenty-seven
</div>
<div id="textFree">one two three four five six seven eight nine ten eleven twelve</div>
<div id="textSmall" style="width: 200px; font-size: 8px">one two three four five six seven eight nine ten eleven twelve</div>"#;

#[test]
fn position_client_and_scroll_dimensions() {
    let mut p = page(POSITION);
    let test1 = p.id("test1");
    // clientDimensions
    assert!(p.client_w(test1) >= 0.0 && p.client_h(test1) >= 0.0);
    assert_eq!(p.layout.client_top(), 0.0);
    assert_eq!(p.layout.client_left(), 0.0);
    // scrollDimensions
    assert!(p.scroll_w(test1) >= p.client_w(test1));
    assert!(p.scroll_h(test1) >= p.client_h(test1));
    assert_eq!(p.client_w(test1), p.scroll_w(test1));
    assert_eq!(p.client_h(test1), p.scroll_h(test1));

    // scrollWidthFromContent
    let empty = p.add("div");
    assert_eq!(p.client_w(empty), p.scroll_w(empty));
    let bx = p.add("div");
    p.add_to(bx, "span");
    let one_child = p.scroll_w(bx);
    assert!(one_child >= p.client_w(bx));
    p.add_to(bx, "span");
    assert!(p.scroll_w(bx) > one_child);
    assert_eq!(p.scroll_w(bx), p.client_w(bx));
    let short = p.add("div");
    p.text(short, "ab");
    let long = p.add("div");
    p.text(long, "abcdefghijklmnop");
    assert_eq!(p.client_w(short), p.scroll_w(short));
    assert_eq!(p.client_w(long), p.scroll_w(long));
    assert_eq!(p.scroll_w(short), p.scroll_w(long));
    let commented = p.add("div");
    let c = p.dom.create_comment(p.doc, " a very long comment that renders as nothing ").unwrap();
    p.append(commented, c);
    assert_eq!(p.client_w(commented), p.scroll_w(commented));
    let with_hidden = p.add("div");
    let hidden_child = p.add_to(with_hidden, "span");
    p.style(hidden_child, "display: none");
    assert_eq!(p.client_w(with_hidden), p.scroll_w(with_hidden));
    let hidden = p.add("div");
    p.style(hidden, "display: none");
    p.add_to(hidden, "span");
    assert_eq!(p.scroll_w(hidden), 0.0);
    let zero = p.add("div");
    p.style(zero, "width: 0");
    p.add_to(zero, "span");
    assert_eq!(p.client_w(zero), 0.0);
    assert!(p.scroll_w(zero) > 0.0);
    let (body, html) = (p.body(), p.html());
    assert_eq!(p.client_w(body), p.scroll_w(body));
    assert_eq!(p.client_w(html), p.layout.inner_width() as f64);
    assert_eq!(p.offset_w(html), p.scroll_w(html));

    // scrollWidthMarqueeLoop
    // A marquee viewport has an explicit size; an automatic parent
    // correctly grows with its children and cannot be outgrown by cloning.
    let track = p.add("div");
    p.style(track, "height: 100px");
    let lane = p.add_to(track, "div");
    let item = p.add_to(lane, "span");
    p.text(item, "item");
    let before = p.scroll_w(lane);
    p.clone_children_into_self(lane);
    assert_eq!(p.scroll_w(lane), before * 2.0);
    let mut guard = 0;
    while p.scroll_w(lane) < p.offset_w(track) * 2.0 && guard < 50 {
        p.clone_children_into_self(lane);
        guard += 1;
    }
    assert!(guard < 50);

    // clientWidthMarqueeLoop
    let container = p.add("div");
    let wrap = p.add_to(container, "div");
    let item = p.add_to(wrap, "span");
    let container_before = p.client_w(container);
    let mut guard = 0;
    while p.client_w(wrap) < p.client_w(container) * 2.0 && guard < 50 {
        let c = p.dom.clone_node(item, true, None).unwrap();
        p.append(wrap, c);
        guard += 1;
    }
    assert!(guard < 50);
    assert_eq!(p.client_w(container), container_before);
    let pinned = p.add("div");
    p.style(pinned, "width: 40px");
    for _ in 0..3 {
        p.add_to(pinned, "span");
    }
    assert_eq!(p.client_w(pinned), 40.0);
}

#[test]
fn position_translate_and_tickers() {
    let mut p = page(POSITION);
    // recycleTickerLoop
    let shell = p.add("div");
    let track = p.add_to(shell, "div");
    for _ in 0..4 {
        p.add_to(track, "div");
    }
    let s_rect = p.rect(shell);
    let recycle = |p: &mut Page, mut x: f64, reverse: bool| -> i32 {
        let mut guard = 0;
        while guard < 50 {
            let kids: Vec<NodeId> = p.dom.element_children(track).unwrap().collect();
            let el = if reverse { *kids.last().unwrap() } else { kids[0] };
            let r = p.rect(el);
            if if reverse { r.left() < s_rect.right() } else { r.right() > s_rect.left() } {
                break;
            }
            let w = p.offset_w(el);
            if reverse {
                p.dom.insert_before(track, el, Some(kids[0])).unwrap();
                x -= w;
            } else {
                p.append(track, el);
                x += w;
            }
            p.style(track, &format!("transform: translateX({x}px)"));
            guard += 1;
        }
        guard
    };
    p.style(track, "transform: translateX(60px)");
    assert!(recycle(&mut p, 60.0, true) < 50);
    let first = p.dom.element_children(track).unwrap().next().unwrap();
    assert!(p.rect(first).left() < s_rect.right());
    p.style(track, "transform: translateX(-60px)");
    assert!(recycle(&mut p, -60.0, false) < 50);
    let last = p.dom.element_children(track).unwrap().last().unwrap();
    assert!(p.rect(last).right() > s_rect.left());

    // translateX
    let shell = p.add("div");
    let track = p.add_to(shell, "div");
    let card = p.add_to(track, "div");
    let base = p.rect(shell).x;
    assert_eq!(p.rect(card).x, base);
    assert_eq!(p.layout.offset_left(&p.dom, card), base);
    p.style(track, "transform: translateX(-12.5px)");
    assert_eq!(p.rect(card).x, base - 12.5);
    assert_eq!(p.layout.offset_left(&p.dom, card), base - 12.5);
    p.style(card, "transform: translate3d(2px, 100px, 0) rotate(45deg) matrix(1, 0, 0, 1, 3, 9)");
    assert_eq!(p.rect(card).x, base - 7.5);
    assert_eq!(p.layout.offset_left(&p.dom, card), base - 7.5);
    p.style(track, "transform: translateX(-100%)");
    assert_eq!(p.rect(card).x, base + 5.0);

    // computedWidthMarqueeLoop
    let width = |p: &Page, el: NodeId| p.cs(el, "width").trim_end_matches("px").parse::<f64>().unwrap();
    let container = p.add("div");
    let wrap = p.add_to(container, "div");
    let item = p.add_to(wrap, "span");
    let container_width = width(&p, container);
    let mut guard = 0;
    while width(&p, wrap) < container_width * 2.0 && guard < 50 {
        let c = p.dom.clone_node(item, true, None).unwrap();
        p.append(wrap, c);
        guard += 1;
    }
    assert!(guard < 50);
    let pinned = p.add("div");
    p.style(pinned, "width: 40px");
    p.add_to(pinned, "span");
    p.add_to(pinned, "span");
    assert_eq!(p.cs(pinned, "width"), "40px");

    // flexContainerMarqueeLoop
    let container = p.add("div");
    p.set(container, "class", "flex-row");
    let item = p.add_to(container, "div");
    p.text(item, "item");
    let mut clones = 0;
    let mut i = 0;
    loop {
        let limit = (p.rect(container).width / p.rect(item).width).ceil() as i32 + 2;
        if i >= limit || clones >= 50 {
            break;
        }
        let c = p.dom.clone_node(item, true, None).unwrap();
        p.append(container, c);
        clones += 1;
        i += 1;
    }
    assert!(clones < 50);
    assert!(p.scroll_w(container) > p.client_w(container));

    // boxesSizedByTheirItems
    let grows = |p: &mut Page, css: &str, flex_parent: bool| -> bool {
        let bx = p.create("div");
        p.add_to(bx, "span");
        p.style(bx, css);
        let parent = if flex_parent {
            let row = p.add("div");
            p.set(row, "class", "flex-row");
            row
        } else {
            p.body()
        };
        p.append(parent, bx);
        let before = p.client_w(bx);
        p.add_to(bx, "span");
        p.add_to(bx, "span");
        p.client_w(bx) > before
    };
    assert!(grows(&mut p, "display: inline-flex", false));
    assert!(grows(&mut p, "display: flex; width: max-content", false));
    assert!(grows(&mut p, "display: grid; position: absolute", false));
    assert!(grows(&mut p, "display: flex; float: left", false));
    assert!(grows(&mut p, "display: flex", true));
}

#[test]
fn position_scroll_heights() {
    let mut p = page(POSITION);
    // scrollHeightFromContent
    let empty = p.add("div");
    assert_eq!(p.client_h(empty), p.scroll_h(empty));
    let bx = p.add("div");
    p.add_to(bx, "div");
    let one_child = p.scroll_h(bx);
    assert!(one_child >= p.client_h(bx));
    p.add_to(bx, "div");
    assert!(p.scroll_h(bx) > one_child);
    assert_eq!(p.scroll_h(bx), p.client_h(bx));
    let short = p.add("div");
    p.text(short, "ab");
    let long = p.add("div");
    p.text(long, "abcdefghijklmnop");
    assert_eq!(p.client_h(short), p.scroll_h(short));
    assert_eq!(p.client_h(long), p.scroll_h(long));
    assert_eq!(p.scroll_h(short), p.scroll_h(long));
    let with_hidden = p.add("div");
    let hc = p.add_to(with_hidden, "div");
    p.style(hc, "display: none");
    assert_eq!(p.client_h(with_hidden), p.scroll_h(with_hidden));
    let hidden = p.add("div");
    p.style(hidden, "display: none");
    p.add_to(hidden, "div");
    assert_eq!(p.scroll_h(hidden), 0.0);
    let panel = p.add("div");
    p.style(panel, "height: 0");
    p.add_to(panel, "div");
    p.add_to(panel, "div");
    assert_eq!(p.client_h(panel), 0.0);
    assert!(p.scroll_h(panel) > 0.0);
    let (body, html) = (p.body(), p.html());
    assert_eq!(p.client_h(body), p.scroll_h(body));
    assert_eq!(p.client_h(html), p.layout.inner_height() as f64);
    assert_eq!(p.offset_h(html).max(p.client_h(html)), p.scroll_h(html));

    // scrollHeightMarqueeLoop
    // A marquee viewport has an explicit size; an automatic parent
    // correctly grows with its children and cannot be outgrown by cloning.
    let track = p.add("div");
    p.style(track, "height: 100px");
    let lane = p.add_to(track, "div");
    let item = p.add_to(lane, "div");
    p.text(item, "item");
    let before = p.scroll_h(lane);
    p.clone_children_into_self(lane);
    assert_eq!(p.scroll_h(lane), before * 2.0);
    let mut guard = 0;
    while p.scroll_h(lane) < p.offset_h(track) * 2.0 && guard < 50 {
        p.clone_children_into_self(lane);
        guard += 1;
    }
    assert!(guard < 50);
}

#[test]
fn position_scroll_offsets() {
    let mut p = page(POSITION);
    let test1 = p.id("test1");
    let test2 = p.id("test2");
    // scrollPosition
    assert_eq!(p.layout.scroll_top(test1), 0);
    assert_eq!(p.layout.scroll_left(test1), 0);
    p.set_scroll_top(test1, 50);
    assert_eq!(p.layout.scroll_top(test1), 50);
    p.set_scroll_left(test1, 25);
    assert_eq!(p.layout.scroll_left(test1), 25);
    p.set_scroll_top(test1, -10);
    assert_eq!(p.layout.scroll_top(test1), 0);
    p.set_scroll_left(test1, -5);
    assert_eq!(p.layout.scroll_left(test1), 0);
    assert_eq!(p.layout.scroll_top(test2), 0);
    p.set_scroll_top(test2, 100);
    assert_eq!(p.layout.scroll_top(test2), 100);
    assert_eq!(p.layout.scroll_top(test1), 0);

    // scrollMethods
    let t3 = test2;
    p.set_scroll_top(t3, 0);
    p.set_scroll_left(t3, 0);
    let at = |p: &Page| (p.layout.scroll_left(t3), p.layout.scroll_top(t3));
    p.layout.element_scroll_to(&p.dom, t3, Some(30), Some(40));
    assert_eq!(at(&p), (30, 40));
    p.layout.element_scroll_to(&p.dom, t3, Some(5), Some(6));
    assert_eq!(at(&p), (5, 6));
    p.layout.element_scroll_to(&p.dom, t3, Some(11), Some(22));
    assert_eq!(at(&p), (11, 22));
    p.layout.element_scroll_to(&p.dom, t3, None, Some(99));
    assert_eq!(at(&p), (11, 99));
    p.layout.element_scroll_to(&p.dom, t3, Some(-10), Some(-10));
    assert_eq!(at(&p), (0, 0));
    p.layout.element_scroll_to(&p.dom, t3, Some(10), Some(10));
    p.layout.element_scroll_by(&p.dom, t3, Some(5), Some(7));
    assert_eq!(at(&p), (15, 17));
    p.layout.element_scroll_by(&p.dom, t3, Some(-3), Some(-2));
    assert_eq!(at(&p), (12, 15));
    p.layout.element_scroll_by(&p.dom, t3, Some(-1000), Some(-1000));
    assert_eq!(at(&p), (0, 0));

    // offsetDimensions / offsetPosition / offsetVsBounding
    assert_eq!(p.client_w(test1), p.offset_w(test1));
    assert_eq!(p.client_h(test1), p.offset_h(test1));
    let (top1, left1) = (p.layout.offset_top(&p.dom, test1), p.layout.offset_left(&p.dom, test1));
    let (top2, left2) = (p.layout.offset_top(&p.dom, test2), p.layout.offset_left(&p.dom, test2));
    assert!(top1 != top2 || left1 != left2);
    let r = p.rect(test1);
    assert_eq!(r.y, top1);
    assert_eq!(r.x, left1);
    assert_eq!(r.width, p.offset_w(test1));
    assert_eq!(r.height, p.offset_h(test1));

    // scrollClampsToExtent
    let bx = p.id("clampBox");
    let max_top = p.scroll_h(bx) - p.client_h(bx);
    let max_left = p.scroll_w(bx) - p.client_w(bx);
    assert!(max_top > 0.0);
    p.set_scroll_top(bx, 9999);
    p.set_scroll_left(bx, 9999);
    assert_eq!(p.layout.scroll_top(bx) as f64, max_top);
    assert_eq!(p.layout.scroll_left(bx) as f64, max_left);
    p.layout.element_scroll_to(&p.dom, bx, Some(10), Some(10));
    p.layout.element_scroll_by(&p.dom, bx, Some(9999), Some(9999));
    assert_eq!(p.layout.scroll_top(bx) as f64, max_top);
    assert_eq!(p.layout.scroll_left(bx) as f64, max_left);
    p.layout.element_scroll_by(&p.dom, bx, Some(-9999), Some(-9999));
    assert_eq!((p.layout.scroll_left(bx), p.layout.scroll_top(bx)), (0, 0));

    // sheetSizedBox
    let sb = p.id("sheetBox");
    assert_eq!(p.client_h(sb), 100.0);
    assert_eq!(p.scroll_h(sb), 500.0);
    assert_eq!(p.cs(sb, "height"), "100px");
    p.set_scroll_top(sb, 9999);
    assert_eq!(p.layout.scroll_top(sb), 400);
    let auto = p.id("sheetAuto");
    assert_eq!(p.client_h(auto), 505.0); // child starts 5px below its parent
    p.set_scroll_top(auto, 9999);
    assert_eq!(p.layout.scroll_top(auto), 9999);
    assert_eq!(p.client_h(p.id("inlineAuto")), 505.0);
    assert_eq!(p.client_h(p.id("sheetVh")), p.layout.inner_height() as f64 / 10.0);

    // textWrapsAtExplicitWidth
    let tb = p.id("textBox");
    assert_eq!(p.client_h(tb), 100.0);
    assert_eq!(p.scroll_w(tb), 200.0);
    assert!(p.scroll_h(tb) > p.client_h(tb));
    p.set_scroll_top(tb, 9999);
    assert!(p.layout.scroll_top(tb) > 0);
    assert_eq!(p.layout.scroll_top(tb) as f64, p.scroll_h(tb) - p.client_h(tb));
    assert_eq!(p.scroll_h(p.id("textFree")), 5.0);
    let small = p.id("textSmall");
    let before = p.client_h(small);
    p.style(small, "width: 200px; font-size: 4px");
    assert!(p.client_h(small) < before);
}

#[test]
fn position_offset_parent_and_scroll_into_view() {
    let mut p = page(POSITION);
    let test1 = p.id("test1");
    let (body, html) = (p.body(), p.html());
    // offsetParent
    assert_eq!(p.layout.offset_parent(&p.dom, test1), Some(body));
    assert_eq!(p.layout.offset_parent(&p.dom, html), None);
    assert_eq!(p.layout.offset_parent(&p.dom, body), None);
    let wrap = p.add("div");
    p.style(wrap, "position: relative");
    let inner = p.add_to(wrap, "span");
    assert_eq!(p.layout.offset_parent(&p.dom, inner), Some(wrap));
    p.style(inner, "position: fixed");
    assert_eq!(p.layout.offset_parent(&p.dom, inner), None);
    p.style(inner, "");
    let table = p.add("table");
    let tr = p.add_to(table, "tr");
    let td = p.add_to(tr, "td");
    let content = p.add_to(td, "span");
    assert_eq!(p.layout.offset_parent(&p.dom, content), Some(td));
    assert_eq!(p.layout.offset_parent(&p.dom, td), Some(table));
    p.style(content, "position: absolute");
    assert_eq!(p.layout.offset_parent(&p.dom, content), Some(body));
    let hidden = p.add("div");
    p.style(hidden, "display: none");
    let hidden_child = p.add_to(hidden, "span");
    assert_eq!(p.layout.offset_parent(&p.dom, hidden), None);
    assert_eq!(p.layout.offset_parent(&p.dom, hidden_child), None);
    let detached = p.create("div");
    assert_eq!(p.layout.offset_parent(&p.dom, detached), None);
    let host = p.add("div");
    let shadow = p.dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
    let shadow_child = p.create("div");
    p.append(shadow, shadow_child);
    assert_eq!(p.layout.offset_parent(&p.dom, shadow_child), Some(body));
    p.style(host, "position: relative");
    assert_eq!(p.layout.offset_parent(&p.dom, shadow_child), Some(host));

    // scrollIntoView
    let deep = p.add("div");
    let room = p.add("div");
    p.style(room, "height: 10000px");
    let target_y = p.rect(deep).y;
    assert!(target_y > 0.0);
    p.scroll_to(0, 999_999);
    p.layout.scroll_into_view(&p.dom, deep);
    assert_eq!(p.layout.scroll_y() as f64, target_y);
    let in_view = (target_y - 1.0).max(0.0);
    p.scroll_to(0, in_view as i32);
    p.layout.scroll_into_view_if_needed(&p.dom, deep);
    assert_eq!(p.layout.scroll_y() as f64, in_view);
    p.scroll_to(0, (target_y + p.layout.inner_height() as f64 + 100.0) as i32);
    p.layout.scroll_into_view_if_needed(&p.dom, deep);
    assert_eq!(p.layout.scroll_y() as f64, target_y);
}

// tests/window_scroll.html
#[test]
fn window_scroll() {
    let mut p = page(
        r#"<!DOCTYPE html>
<script src="testing.js"></script>
<div style="height: 10000px; width: 10000px"></div>
<script id=scrollBy_exists></script>"#,
    );
    let at = |p: &Page| (p.layout.scroll_x(), p.layout.scroll_y());
    // scrollBy_xy
    p.scroll_to(0, 0);
    assert_eq!(at(&p), (0, 0));
    p.layout.window_scroll_by(&p.dom, Some(100), Some(200));
    assert_eq!(at(&p), (100, 200));
    // scrollBy_relative
    p.scroll_to(100, 100);
    p.layout.window_scroll_by(&p.dom, Some(50), Some(50));
    assert_eq!(at(&p), (150, 150));
    // scrollBy_opts
    p.scroll_to(0, 0);
    p.layout.window_scroll_by(&p.dom, Some(30), Some(40));
    assert_eq!(at(&p), (30, 40));
    // scrollTo_opts_omitted_axis_untouched
    p.scroll_to(30, 40);
    p.layout.window_scroll_to(&p.dom, None, Some(60));
    assert_eq!(at(&p), (30, 60));
    p.layout.window_scroll_to(&p.dom, Some(10), None);
    assert_eq!(at(&p), (10, 60));
    // scrollBy_negative_clamp
    p.scroll_to(10, 10);
    p.layout.window_scroll_by(&p.dom, Some(-100), Some(-100));
    assert_eq!(at(&p), (0, 0));

    // scrollTo_clamps_to_document
    let html = p.html();
    let max = p.scroll_h(html) - p.layout.inner_height() as f64;
    assert!(max > 0.0);
    p.scroll_to(0, 99_999_999);
    assert_eq!(p.layout.scroll_y() as f64, max);
    p.layout.window_scroll_by(&p.dom, Some(0), Some(100));
    assert_eq!(p.layout.scroll_y() as f64, max);
    let max_x = p.scroll_w(html) - p.layout.inner_width() as f64;
    assert!(max_x > 0.0);
    p.scroll_to(99_999_999, 0);
    assert_eq!(p.layout.scroll_x() as f64, max_x);
    let more = p.add("div");
    p.style(more, "height: 5000px");
    p.scroll_to(0, 99_999_999);
    assert_eq!(p.layout.scroll_y() as f64, max + 5000.0);
    p.remove(more);
    p.scroll_to(0, 0);

    // scrollTo_reaches_nested_spacer
    let outer = p.add("div");
    let spacer = p.add_to(outer, "div");
    p.style(spacer, "height: 20000px");
    p.scroll_to(0, 99_999_999);
    assert!(p.layout.scroll_y() as f64 + p.layout.inner_height() as f64 >= 20000.0);
    assert_eq!(p.layout.scroll_y() as f64, p.scroll_h(html) - p.layout.inner_height() as f64);
    p.remove(outer);
    p.scroll_to(0, 0);

    // element_scrollBy_noop_does_not_fire_events: nothing moves.
    let element = p.create("div");
    assert!(!p.layout.element_scroll_by(&p.dom, element, Some(-1), Some(-1)));

    // nested_root_tags_are_not_roots
    let before = p.scroll_h(html);
    let nb = p.add("body");
    let nh = p.add("html");
    assert_eq!(p.scroll_h(html), before + 10.0);
    assert!(p.scroll_w(html) > 0.0);
    p.remove(nb);
    p.remove(nh);

    // nested_root_tags_are_ordinary_elements
    let nested_root = p.add("body");
    let nested_child = p.add_to(nested_root, "div");
    p.style(nested_child, "height: 300px");
    assert_eq!(p.client_h(nested_root), 305.0);
    assert_eq!(p.scroll_h(nested_root), 305.0);
    p.remove(nested_root);

    // nested_root_tags_extend_the_document
    let spacer_root = p.add("body");
    let root_spacer = p.add_to(spacer_root, "div");
    p.style(root_spacer, "height: 20000px");
    assert!(p.scroll_h(html) >= 20000.0);
}

#[test]
fn wheel_latches_to_scroll_container() {
    // frame/user_input.zig:576-610: the delta goes to the nearest container
    // that can still move, else the viewport.
    let mut p = page(
        r#"<!DOCTYPE html><body>
<div id="box" style="height: 100px; overflow: auto"><div id="inner" style="height: 500px"></div></div>
<div style="height: 10000px"></div></body>"#,
    );
    let inner = p.id("inner");
    let bx = p.id("box");
    assert_eq!(p.layout.wheel_scroll(&p.dom, inner, 0.0, 300.0), vec![ScrollTarget::Container(bx)]);
    assert_eq!(p.layout.scroll_top(bx), 300);
    assert_eq!(p.layout.wheel_scroll(&p.dom, inner, 0.0, 300.0), vec![ScrollTarget::Container(bx)]);
    assert_eq!(p.layout.scroll_top(bx), 400);
    assert_eq!(p.layout.wheel_scroll(&p.dom, inner, 0.0, 50.0), vec![ScrollTarget::Viewport]);
    assert_eq!(p.layout.scroll_y(), 50);
    assert!(p.layout.wheel_scroll(&p.dom, inner, f64::NAN, 0.0).is_empty());
}

// ---------------------------------------------------------------------------
// tests/element/styles.html (computed style parts)
// ---------------------------------------------------------------------------

#[test]
fn computed_style_defaults_and_inline() {
    let mut p = page("<!DOCTYPE html><body></body>");
    // computedStyleDefaults
    let div = p.add("div");
    let span = p.add("span");
    let anchor = p.add("a");
    assert_eq!(p.cs(div, "display"), "block");
    assert_eq!(p.cs(span, "display"), "inline");
    assert_eq!(p.cs(anchor, "display"), "inline");
    assert_eq!(p.cs(div, "visibility"), "visible");
    assert_eq!(p.cs(div, "opacity"), "1");
    // Without href it is not a link (Chrome: black; the legacy cascade: link blue).
    let anchor_color = "rgb(0, 0, 0)";
    assert_eq!(p.cs(anchor, "color"), anchor_color);

    // computedStyleBoxModelDefaults
    for side in ["top", "right", "bottom", "left"] {
        assert_eq!(p.cs(div, &format!("padding-{side}")), "0px");
        assert_eq!(p.cs(div, &format!("margin-{side}")), "0px");
        assert_eq!(p.cs(div, &format!("border-{side}-width")), "0px");
    }
    assert_eq!(p.cs(div, "padding"), "0px");
    assert_eq!(p.cs(div, "margin"), "0px");
    assert_eq!(p.cs(div, "border-width"), "0px");
    let styled = p.add("div");
    p.style(styled, "padding-top: 7px; margin-left: 3px");
    assert_eq!(p.cs(styled, "padding-top"), "7px");
    assert_eq!(p.cs(styled, "margin-left"), "3px");
    assert_eq!(p.cs(styled, "padding-bottom"), "0px");

    // computedStyleInitialValues
    for (name, value) in [
        ("font-weight", "400"),
        ("line-height", "normal"),
        ("letter-spacing", "normal"),
        ("word-spacing", "0px"),
        ("text-indent", "0px"),
        ("top", "auto"),
        ("right", "auto"),
        ("bottom", "auto"),
        ("left", "auto"),
        ("z-index", "auto"),
        ("box-sizing", "content-box"),
        ("min-width", "0px"),
        ("min-height", "0px"),
        ("max-width", "none"),
        ("max-height", "none"),
        ("flex-grow", "0"),
        ("flex-shrink", "1"),
        ("flex-basis", "auto"),
        ("transform", "none"),
        ("gap", "normal"),
        ("column-gap", "normal"),
        ("border-radius", "0px"),
        ("border-top-left-radius", "0px"),
        ("border-bottom-right-radius", "0px"),
        ("border-style", "none"),
        ("border-top-style", "none"),
        ("border-left-style", "none"),
    ] {
        assert_eq!(p.cs(div, name), value, "{name}");
    }
    assert_eq!(p.cs(div, "font-size"), "16px");
    let outer = p.add("div");
    p.style(outer, "font-size: 20px");
    let inner = p.add_to(outer, "span");
    let relative = p.add_to(outer, "span");
    p.style(relative, "font-size: 1.5em");
    assert_eq!(p.cs(outer, "font-size"), "20px");
    assert_eq!(p.cs(inner, "font-size"), "20px");
    assert_eq!(p.cs(relative, "font-size"), "30px");
    let sized = |p: &mut Page, value: &str| {
        let el = p.add_to(outer, "span");
        p.style(el, &format!("font-size: {value}"));
        p.cs(el, "font-size")
    };
    assert_eq!(sized(&mut p, "xx-small"), "9px");
    assert_eq!(sized(&mut p, "LARGE"), "18px");
    assert_eq!(sized(&mut p, "xxx-large"), "48px");
    assert_eq!(sized(&mut p, "larger"), "24px");
    assert_eq!(sized(&mut p, "2rem"), "32px");
    let root = p.html();
    p.style(root, "font-size: 20px");
    assert_eq!(sized(&mut p, "1.5rem"), "30px");
    p.style(root, "font-size: 2rem");
    assert_eq!(p.cs(root, "font-size"), "32px");
    assert_eq!(sized(&mut p, "1.5rem"), "48px");
    p.unset(root, "style");
    let positioned = p.add("div");
    p.style(positioned, "position: absolute; top: 4px");
    assert_eq!(p.cs(positioned, "position"), "absolute");
    assert_eq!(p.cs(positioned, "top"), "4px");

    // computedStyleInline
    let div = p.add("div");
    p.style(div, "text-transform: uppercase; font-weight: 700; color: rgb(1, 2, 3)");
    assert_eq!(p.cs(div, "text-transform"), "uppercase");
    assert_eq!(p.cs(div, "font-weight"), "700");
    assert_eq!(p.cs(div, "color"), "rgb(1, 2, 3)");
    assert_eq!(p.cs(div, "letter-spacing"), "normal");
    let plain = p.add("div");
    assert_eq!(p.cs(plain, "display"), "block");
    p.style(div, "text-transform: lowercase");
    assert_eq!(p.cs(div, "text-transform"), "lowercase");
    let imp = p.add("div");
    p.style(imp, "color: red !important; color: blue");
    // Colors serialize as Chromium computes them (upstream returned the declared "red").
    assert_eq!(p.cs(imp, "color"), "rgb(255, 0, 0)");

    // hidden boxes compute width/height to auto (CSSStyleDeclaration.zig:203-212)
    let gone = p.add("div");
    p.style(gone, "display: none");
    assert_eq!(p.cs(gone, "width"), "auto");
}

// tests/css/text_transform.html (computed part)
#[test]
fn text_transform_computed() {
    let p = page(
        r#"<!DOCTYPE html><head><style>
  .upper { text-transform: uppercase; }
  .lower { text-transform: lowercase; }
  .cap { text-transform: capitalize; }
  .reset { text-transform: none; }
  .combo { text-transform: uppercase full-width; }
  #id-wins { text-transform: lowercase; }
  .bogus { text-transform: shout; }
  .two-cases { text-transform: uppercase lowercase; }
</style></head><body>
<span id=inline style="text-transform: uppercase">unavailable</span>
<span id=class class=upper>unavailable</span>
<div class=upper><span id=inherited>unavailable</span></div>
<div class=upper><span id=reset class=reset>Mixed Case</span></div>
<span id=lower class=lower>UNAVAILABLE</span>
<span id=cap class=cap>hello</span>
<span id=id-wins class=upper>Mixed</span>
<span id=combo class=combo>wide</span>
<div class=upper><span id=bogus class=bogus>still upper</span></div>
<div class=upper><span id=two-cases class=two-cases>still upper</span></div>
<div class=upper><span id=inline-inherit style="text-transform: inherit">kept</span></div>
<div class=upper><span id=inline-initial style="text-transform: initial">plain</span></div>
<span id=plain>Plain Text</span>
</body>"#,
    );
    let tt = |id: &str| p.cs(p.id(id), "text-transform");
    assert_eq!(tt("inline"), "uppercase");
    assert_eq!(tt("class"), "uppercase");
    assert_eq!(tt("inherited"), "uppercase");
    assert_eq!(tt("reset"), "none");
    assert_eq!(tt("lower"), "lowercase");
    assert_eq!(tt("cap"), "capitalize");
    assert_eq!(tt("id-wins"), "lowercase");
    assert_eq!(tt("combo"), "uppercase");
    assert_eq!(tt("bogus"), "uppercase");
    assert_eq!(tt("two-cases"), "uppercase");
    assert_eq!(tt("inline-inherit"), "uppercase");
    assert_eq!(tt("inline-initial"), "none");
    assert_eq!(tt("plain"), "none");
    assert_eq!(p.layout.style().text_transform(&p.dom, p.id("class")), TextTransform::Uppercase);
}

// tests/css/custom_properties.html
#[test]
fn custom_properties() {
    let p = page(
        r#"<!DOCTYPE html><head><style>
  :root { --brand: #336699; --theme: light; }
  html { --from-html: html; }
  .card { --pad: 16px; --dup: from-class; }
  #card { --dup: from-id; }
  #case { --Token: upper; --token: lower; }
  #within-block { --normal-last: first; --normal-last: second; }
  #within-block { --important-first: kept !important; --important-first: ignored; }
  #dynamic:hover { --hover-only: nope; }
  #alias { --base: 4px; --derived: var(--base); }
  #outer.outer.outer { --nearest: far; }
  .inner { --nearest: near; }
</style></head>
<body>
  <div class="card" id="card" style="--inline: 8px; --pad: inline-wins">card</div>
  <div class="card" id="plain-card">plain card</div>
  <div id="case">case</div>
  <div id="within-block">within-block</div>
  <div id="dynamic">dynamic</div>
  <div id="alias">alias</div>
  <div id="outer" class="outer"><div class="inner"><span id="nearest">nearest</span></div></div>
  <div id="plain">plain</div>
</body>"#,
    );
    let v = |id: &str, name: &str| p.cs(p.id(id), name);
    // custom_property_sources
    assert_eq!(v("card", "--brand"), "#336699");
    assert_eq!(v("card", "--from-html"), "html");
    assert_eq!(v("plain-card", "--pad"), "16px");
    assert_eq!(v("card", "--pad"), "inline-wins");
    assert_eq!(v("card", "--inline"), "8px");
    assert_eq!(v("card", "--dup"), "from-id");
    assert_eq!(p.cs(p.html(), "--brand"), "#336699");
    assert_eq!(v("card", "--nope"), "");
    assert_eq!(v("plain", "--nope"), "");
    // custom_property_case_sensitive
    assert_eq!(v("case", "--Token"), "upper");
    assert_eq!(v("case", "--token"), "lower");
    assert_eq!(v("case", "--TOKEN"), "");
    // custom_property_within_block
    assert_eq!(v("within-block", "--normal-last"), "second");
    assert_eq!(v("within-block", "--important-first"), "kept");
    // custom_property_dynamic_pseudo_skipped
    assert_eq!(v("dynamic", "--hover-only"), "");
    // custom_property_var_substitution (Chrome substitutes var(); the legacy
    // cascade reports the declaration as written)
    assert_eq!(v("alias", "--base"), "4px");
    assert_eq!(v("alias", "--derived"), "4px");
    // custom_property_nearest_ancestor_wins
    assert_eq!(v("nearest", "--nearest"), "near");
    assert_eq!(v("outer", "--nearest"), "far");
}

#[test]
fn hidden_and_foreign_elements_have_no_box() {
    let mut p = page("<!DOCTYPE html><body><div id=a style='width: 10px; height: 10px'></div><template id=t><p id=in>x</p></template></body>");
    let a = p.id("a");
    p.style(a, "display: none; width: 10px");
    assert_eq!(p.layout.client_rect(&p.dom, a), None);
    assert!(p.layout.client_rects(&p.dom, a).is_empty());
    assert_eq!(p.layout.box_axis(&p.dom, a, Axis::Width), 10.0);
    // Template contents belong to the inert document: no owner frame.
    let t = p.id("t");
    let content = p.dom.template_contents(t).unwrap().unwrap();
    let inner = p.dom.first_child(content).unwrap().unwrap();
    assert!(!p.layout.is_visible(&p.dom, inner));
    assert_eq!(p.rect(inner), Rect::default());
}

/// Element scroll offsets move descendants' client rects; hit testing and
/// scrollIntoViewIfNeeded agree with the shifted rects (lightningpanda
/// deviation from upstream, which only applies the window scroll).
#[test]
fn element_scroll_moves_rects_hit_testing_and_reveal() {
    let mut p = page(
        "<body><div id=root style='overflow:auto;width:100px;height:100px'><div id=t style='width:20px;height:20px'></div><div style='height:1000px'></div><div id=far style='width:20px;height:20px'></div></div><p id=after>x</p></body>",
    );
    let (root, t, far, after) = (p.id("root"), p.id("t"), p.id("far"), p.id("after"));
    let before = p.layout.bounding_client_rect(&p.dom, t);
    let after_before = p.layout.bounding_client_rect(&p.dom, after);
    assert!(p.layout.set_scroll_top(&p.dom, root, 100));
    let moved = p.layout.bounding_client_rect(&p.dom, t);
    assert_eq!(moved.y, before.y - 100.0);
    assert_eq!(moved.x, before.x);
    assert_eq!(p.layout.bounding_client_rect(&p.dom, after), after_before, "non-descendants unaffected");
    // Hit testing agrees with the shifted rect at its new center.
    let f = p.layout.bounding_client_rect(&p.dom, far);
    let hit = p.layout.element_from_point(&p.dom, f.x + 1.0, f.y + 1.0);
    assert_eq!(hit, Some(far));
    // The container itself is still hit at its own (unscrolled) position.
    let r = p.layout.bounding_client_rect(&p.dom, root);
    assert_eq!(p.layout.element_from_point(&p.dom, r.x + 90.0, r.y + 1.0), Some(root));
    // scrollIntoViewIfNeeded reveals `t` inside the scrolled container.
    // Aligned like `block: start`: t's top lands on the container's top.
    p.layout.scroll_into_view_if_needed(&p.dom, t);
    assert!(p.layout.scroll_top(root) < 100);
    let revealed = p.layout.bounding_client_rect(&p.dom, t);
    let root_rect = p.layout.bounding_client_rect(&p.dom, root);
    assert_eq!(revealed.y, root_rect.y);
    assert_eq!(p.layout.element_from_point(&p.dom, revealed.x + 1.0, revealed.y + 1.0), Some(t));
    // Never-scrolled containers keep upstream window-only behavior.
    let mut q = page("<body><div id=c style='overflow:auto;height:10px'><b id=b>x</b></div></body>");
    let (c, b) = (q.id("c"), q.id("b"));
    q.layout.scroll_into_view_if_needed(&q.dom, b);
    assert_eq!(q.layout.scroll_top(c), 0);
}

#[test]
fn automatic_parent_box_encloses_nested_in_flow_children() {
    for pointer in ["", "style='pointer-events:none'", "class=dead"] {
        let p = page(&format!("<style>.dead{{pointer-events:none}}</style><table><tr><td style='height:26px'><div><div><div id=wrap tabindex=0 style='position:relative;display:flex'><div style='display:flex'><div {pointer}><button id=button>CPT</button></div></div></div></div></div></td></tr></table>"));
        let button = p.id("button");
        let wrap = p.id("wrap");
        let br = p.rect(button);
        let wr = p.rect(wrap);
        assert!(
            wr.top() <= br.top() && wr.bottom() >= br.bottom(),
            "automatic parent {wr:?} must enclose nested normal-flow child {br:?}"
        );
        let (x, y) = p.center(button);
        let hit = p.efp(x, y).expect("hit");
        assert!(p.contains(wrap, hit), "button center must hit its clickable subtree, not {}", p.tag(hit));
    }
}

#[test]
fn automatic_height_tracks_nested_geometry_without_overriding_explicit_height() {
    let mut p=page("<div id=auto><div><div id=child style='height:40px'>x</div></div></div><div id=fixed style='height:3px'><div style='height:40px'></div></div>");
    let a = p.id("auto");
    let child = p.id("child");
    let fixed = p.id("fixed");
    assert!(p.rect(a).bottom() >= p.rect(child).bottom());
    assert_eq!(p.rect(fixed).height, 3.0);
    assert!(p.scroll_h(a) >= p.client_h(a));
    p.style(child, "height:80px");
    assert!(p.rect(a).bottom() >= p.rect(child).bottom());
    let expanded = p.rect(a).height;
    p.remove(child);
    assert!(p.rect(a).height < expanded);
}

#[test]
#[ignore = "bounded same-host geometry timing diagnostic"]
fn automatic_height_timing_diagnostic() {
    let mut html = String::from("<div id=outer>");
    for _ in 0..1000 {
        html.push_str("<div><div><button>x</button></div></div>");
    }
    html.push_str("</div>");
    let mut p = page(&html);
    let outer = p.id("outer");
    let start = std::time::Instant::now();
    let first = p.rect(outer);
    let cold = start.elapsed();
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        std::hint::black_box(p.rect(outer));
    }
    let warm = start.elapsed();
    let start = std::time::Instant::now();
    for i in 0..10 {
        p.set(outer, "data-version", &i.to_string());
        std::hint::black_box(p.rect(outer));
    }
    eprintln!(
        "geometry 4004nodes cold_us={} warm1000_us={} mutation10_us={} height={}",
        cold.as_micros(),
        warm.as_micros(),
        start.elapsed().as_micros(),
        first.height
    );
}

#[test]
fn automatic_height_invalidates_for_hidden_subtrees_and_stylesheet_changes() {
    let mut p =
        page("<style>.tall{height:90px}</style><div id=outer><div><span id=child class=tall>x</span></div></div>");
    let outer = p.id("outer");
    let child = p.id("child");
    let tall = p.rect(outer).height;
    p.style(child, "display:none");
    assert!(p.rect(outer).height < tall);
    p.style(child, "");
    assert_eq!(p.rect(outer).height, tall);
    p.set(child, "class", "");
    assert!(p.rect(outer).height < tall);
    p.set(child, "class", "tall");
    assert_eq!(p.rect(outer).height, tall);
}

#[test]
fn focus_reveals_auto_scroller_before_viewport_and_preserves_visible_axes() {
    let mut p = page_with(&format!("<!doctype html><body><div id=c style='overflow:auto;width:100px;height:100px'>{}<button id=b style='width:20px;height:20px'>target</button><div style='height:300px'>tail</div></div></body>", "<div style='height:20px'>row</div>".repeat(50)), Viewport { width: 1440, height: 900, ..Default::default() });
    let c = p.id("c");
    let b = p.id("b");
    let before = p.layout.bounding_client_rect(&p.dom, b);
    assert!(before.top() > p.layout.bounding_client_rect(&p.dom, c).bottom());
    assert_eq!(p.layout.reveal_for_focus(&p.dom, b), vec![ScrollTarget::Container(c)]);
    let target = p.layout.bounding_client_rect(&p.dom, b);
    let container = p.layout.bounding_client_rect(&p.dom, c);
    assert!(target.top() >= container.top() && target.bottom() <= container.bottom());
    assert!((target.top() - container.top() - 40.0).abs() < 1.0);
    assert!(p.layout.reveal_for_focus(&p.dom, b).is_empty());
}

fn nested_scroll_page() -> Page {
    page(&format!(
        "<body>{}<div id=outer style='overflow:auto;width:180px;height:80px'>{}<div id=inner style='overflow:auto;width:120px;height:60px'>{}<button id=target style='height:20px;width:20px'>target</button><div style='height:100px'>tail</div></div><div style='height:100px'>tail</div></div><div style='height:900px'>end</div></body>",
        "<div style='height:20px'>before</div>".repeat(100),
        "<div style='height:20px'>outer</div>".repeat(20),
        "<div style='height:20px'>inner</div>".repeat(20),
    ))
}

fn assert_nested_scroll_reveals_target(p: &Page) {
    let (outer, inner, target) = (p.id("outer"), p.id("inner"), p.id("target"));
    assert!(p.layout.scroll_top(inner) > 0, "fresh inner overflow container must scroll");
    assert!(p.layout.scroll_top(outer) > 0, "fresh outer overflow container must scroll");
    let t = p.rect(target);
    for parent in [inner, outer] {
        let r = p.rect(parent);
        assert!(t.top() >= r.top() && t.bottom() <= r.bottom(), "target must be revealed inside every ancestor");
        assert!(t.left() >= r.left() && t.right() <= r.right(), "target must also fit horizontally");
    }
    assert!(t.top() >= 0.0 && t.bottom() <= p.layout.inner_height() as f64);
}

#[test]
fn scroll_into_view_reveals_fresh_nested_overflow_containers() {
    let mut p = nested_scroll_page();
    let target = p.id("target");
    assert!(p.layout.scroll_into_view(&p.dom, target));
    assert_nested_scroll_reveals_target(&p);
    assert!(!p.layout.scroll_into_view(&p.dom, target), "unchanged scroll is a no-op");
    assert!(p.layout.reveal_for_focus(&p.dom, target).is_empty(), "focus must not introduce a second scroll");
}

#[test]
fn scroll_into_view_if_needed_reveals_fresh_nested_overflow_containers() {
    let mut p = nested_scroll_page();
    let target = p.id("target");
    assert!(p.layout.scroll_into_view_if_needed(&p.dom, target));
    assert_nested_scroll_reveals_target(&p);
    assert!(!p.layout.scroll_into_view_if_needed(&p.dom, target), "visible target is a no-op");
}

#[test]
fn scroll_into_view_horizontal_nearest_preserves_visible_and_spanning_boxes() {
    let mut p = page("<div id=c style='overflow:auto;width:100px;height:100px'><button id=b style='width:20px;height:20px;transform:translateX(200px)'>target</button></div>");
    let (c, b) = (p.id("c"), p.id("b"));
    assert!(p.rect(b).left() > p.rect(c).right());
    p.layout.scroll_into_view(&p.dom, b);
    assert_eq!(p.rect(b).right(), p.rect(c).right(), "right-clipped small box uses nearest right edge");
    let at = p.layout.scroll_left(c);
    p.layout.scroll_into_view(&p.dom, b);
    assert_eq!(p.layout.scroll_left(c), at, "visible horizontal position stays unchanged");
    p.style(b, "width:20px;height:20px");
    assert!(p.rect(b).left() < p.rect(c).left());
    p.layout.scroll_into_view(&p.dom, b);
    assert_eq!(p.rect(b).left(), p.rect(c).left(), "left-clipped small box uses nearest left edge");
    p.style(b, "width:200px;height:20px;transform:translateX(-50px)");
    assert!(p.rect(b).left() < p.rect(c).left() && p.rect(b).right() > p.rect(c).right());
    let at = p.layout.scroll_left(c);
    p.layout.scroll_into_view(&p.dom, b);
    assert_eq!(p.layout.scroll_left(c), at, "box spanning both edges must not oscillate");
}

#[test]
fn variable_dimensions_cascade_inherit_and_invalidate() {
    let mut p = page(
        r#"<style>
      .overlay { height:var(--visual-viewport-height); width:var(--missing, var(--width, 120px)); }
      #important { height:var(--size) !important; --size:70px !important; }
    </style><div id=parent style='--visual-viewport-height:900px;--base:80px;--alias:var(--base)'>
      <div id=box class=overlay></div><div id=alias style='--base:20px;height:var(--alias)'></div>
      <div id=important style='height:15px;--size:10px'></div>
    </div>"#,
    );
    let (parent, b) = (p.id("parent"), p.id("box"));
    assert_eq!(p.rect(b).height, 900.0);
    assert_eq!(p.rect(b).width, 120.0);
    assert_eq!(p.rect(p.id("alias")).height, 80.0, "substitute before inheritance");
    assert_eq!(p.rect(p.id("important")).height, 70.0, "both declarations honor importance");
    p.style(parent, "--visual-viewport-height:700px;--width:160px");
    assert_eq!(p.rect(b).height, 700.0);
    assert_eq!(p.rect(b).width, 160.0);
    p.style(b, "height:var(--own);--own:25vh;width:var(--width)");
    assert_eq!(p.rect(b).height, 270.0);
    p.layout.set_viewport(Viewport { height: 800, ..Viewport::default() });
    assert_eq!(p.rect(b).height, 200.0);
    let other = p.add("div");
    p.style(other, "--width:90px");
    p.append(other, b);
    assert_eq!(p.rect(b).width, 90.0, "reparenting invalidates inherited values");
}

#[test]
fn variable_dimensions_invalid_values_and_cycles_use_computed_fallback() {
    let p = page(
        r#"<style>
      #missing { height:99px; height:var(--absent); }
      #cycle { --a:var(--b); --b:var(--a,7px); height:var(--a,42px); }
      #fallbackcycle { --valid:10px; --a:var(--valid,var(--a)); height:var(--a,43px); }
      #wrong { --bad:red; height:99px; height:var(--bad,40px); }
      #initial { --v:initial; height:var(--v,44px); }
    </style><div id=missing></div><div id=cycle></div><div id=fallbackcycle></div><div id=wrong></div><div id=initial></div>"#,
    );
    assert_eq!(p.rect(p.id("missing")).height, 5.0, "invalid at computed value does not revive 99px");
    assert_eq!(p.rect(p.id("cycle")).height, 42.0);
    // Chrome (and Stylo) do not count an unused fallback as a cycle: 10px.
    let fallback_cycle = 10.0;
    assert_eq!(p.rect(p.id("fallbackcycle")).height, fallback_cycle, "unused fallback");
    assert_eq!(p.rect(p.id("wrong")).height, 5.0, "existing invalid length does not use var fallback");
    assert_eq!(p.rect(p.id("initial")).height, 44.0);
}

#[test]
fn variable_sized_scroller_does_not_scroll_again_after_small_input_replacement() {
    let mut p = page(&format!("<style>.overlay{{height:var(--viewport);overflow:auto;width:200px}}</style><div id=overlay class=overlay style='--viewport:900px'>{}<div id=slot><input id=field></div></div>", "<div style='height:20px'>row</div>".repeat(200)));
    let (overlay, slot, old) = (p.id("overlay"), p.id("slot"), p.id("field"));
    assert_eq!(p.rect(overlay).height, 900.0);
    p.layout.scroll_into_view(&p.dom, old);
    p.dom.remove_node(old).unwrap();
    let wrapper = p.add_to(slot, "div");
    let replacement = p.add_to(wrapper, "input");
    let t = p.rect(replacement);
    let c = p.rect(overlay);
    assert!(
        t.top() >= c.top() && t.bottom() <= c.bottom(),
        "target={t:?} container={c:?} offset={}",
        p.layout.scroll_top(overlay)
    );
    assert!(p.layout.reveal_for_focus(&p.dom, replacement).is_empty());
}

#[test]
fn percentage_width_uses_definite_parent_and_tracks_mutation() {
    let mut p = page("<!doctype html><style>.full{width:100%}.half{width:50%}</style><body><div id=a style='width:320px'><div id=full class=full><div id=half class=half></div></div></div><div id=b style='width:120px'></div></body>");
    let a = p.id("a");
    let b = p.id("b");
    let full = p.id("full");
    let half = p.id("half");
    assert_eq!(p.rect(full).width, 320.0);
    assert_eq!(p.rect(half).width, 160.0);
    p.style(a, "width:640px");
    assert_eq!(p.rect(full).width, 640.0);
    assert_eq!(p.rect(half).width, 320.0);
    p.append(b, full);
    assert_eq!(p.rect(full).width, 120.0);
    assert_eq!(p.rect(half).width, 60.0);
    p.style(full, "width:var(--fraction, 25%)");
    assert_eq!(p.rect(full).width, 30.0);
    p.style(b, "width:120px;--fraction:75%");
    assert_eq!(p.rect(full).width, 90.0);
    p.style(full, "width:0%");
    assert_eq!(p.rect(full).width, 0.0);
    p.style(p.body(), "width:480px");
    p.add_style("body { width: 90% !important }");
    let root_child = p.add("div");
    p.style(root_child, "width:50%");
    assert_eq!(p.rect(root_child).width, 240.0, "preserve the root's inline-only width policy");
}

#[test]
fn percentage_width_does_not_resolve_circular_or_positioned_bases() {
    let mut p = page("<!doctype html><body><div id=auto><div id=percent style='width:100%;height:50%'></div></div><div id=fixed style='width:320px'><div id=positioned style='position:absolute;width:100%'></div></div></body>");
    let percent = p.id("percent");
    assert_eq!(p.rect(percent).width, 5.0, "an automatic parent is not a definite percentage basis");
    assert_eq!(p.rect(percent).height, 5.0, "percentage height remains unsupported");
    assert_eq!(p.rect(p.id("positioned")).width, 5.0, "positioned containing blocks remain unsupported");
    let mut parent = p.id("fixed");
    for _ in 0..512 {
        let child = p.add_to(parent, "div");
        p.style(child, "width:100%");
        parent = child;
    }
    assert_eq!(p.rect(parent).width, 320.0, "percentage chains resolve without recursive layout calls");
}

// ---------------------------------------------------------------------------
// Generic computed values from author stylesheets (computed.rs)
// ---------------------------------------------------------------------------

#[test]
fn computed_style_resolves_author_stylesheet_values() {
    let mut p = page("<!DOCTYPE html><body></body>");
    p.add_style(concat!(
        ":root{--spacing:.25rem;--brand:#7c3aed}",
        ".badge{background-color:var(--brand);color:white}",
        ".bg-\\[\\#7c3aed\\]{background-color:#7c3aed}",
        "@layer utilities{.gap-2{gap:calc(var(--spacing) * 2)}.flex-1{flex:1}.scroll{overflow:hidden auto}}",
        ".parent{color:rgb(1 2 3);font-family:Inter, Arial, sans-serif}",
        ".cur{border-color:currentColor}",
        ".mix{background-color:color-mix(in oklab, oklch(0.5 0 0) 50%, transparent)}",
        ".size{width:calc(var(--spacing) * 3.5);margin-left:1.5rem}",
        ".imp{color:red!important}.low{color:blue}",
        ".missing{background-color:var(--nope)}",
        ".unset{color:unset}",
    ));
    let badge = p.add("span");
    p.set(badge, "class", "badge");
    assert_eq!(p.cs(badge, "background-color"), "rgb(124, 58, 237)");
    assert_eq!(p.cs(badge, "color"), "rgb(255, 255, 255)");

    let escaped = p.add("span");
    p.set(escaped, "class", "bg-[#7c3aed]");
    assert_eq!(p.cs(escaped, "background-color"), "rgb(124, 58, 237)");

    let flex = p.add("div");
    p.set(flex, "class", "gap-2 flex-1 scroll");
    assert_eq!(p.cs(flex, "column-gap"), "8px");
    assert_eq!(p.cs(flex, "row-gap"), "8px");
    assert_eq!(p.cs(flex, "flex-grow"), "1");
    assert_eq!(p.cs(flex, "flex-basis"), "0%");
    assert_eq!(p.cs(flex, "overflow-x"), "hidden");
    assert_eq!(p.cs(flex, "overflow-y"), "auto");

    // Inherited properties come from the nearest declaring ancestor; others do not inherit.
    let parent = p.add("div");
    p.set(parent, "class", "parent");
    let child = p.add_to(parent, "span");
    assert_eq!(p.cs(child, "color"), "rgb(1, 2, 3)");
    assert_eq!(p.cs(child, "font-family"), "Inter, Arial, sans-serif");
    assert_eq!(p.cs(child, "background-color"), "rgba(0, 0, 0, 0)");
    let cur = p.add_to(parent, "span");
    p.set(cur, "class", "cur");
    assert_eq!(p.cs(cur, "border-top-color"), "rgb(1, 2, 3)");
    let unset = p.add_to(parent, "span");
    p.set(unset, "class", "unset");
    assert_eq!(p.cs(unset, "color"), "rgb(1, 2, 3)");

    let mix = p.add("div");
    p.set(mix, "class", "mix");
    assert_eq!(p.cs(mix, "background-color"), "oklab(0.5 0 0 / 0.5)");

    let sized = p.add("div");
    p.set(sized, "class", "size");
    assert_eq!(p.cs(sized, "width"), "14px");
    assert_eq!(p.cs(sized, "margin-left"), "24px");

    // !important rules beat normal inline declarations; inline beats normal rules.
    let imp = p.add("div");
    p.set(imp, "class", "imp low");
    p.style(imp, "color: green");
    assert_eq!(p.cs(imp, "color"), "rgb(255, 0, 0)");
    let low = p.add("div");
    p.set(low, "class", "low");
    p.style(low, "color: green");
    assert_eq!(p.cs(low, "color"), "rgb(0, 128, 0)");

    // Invalid at computed-value time falls back to the initial value.
    let missing = p.add("div");
    p.set(missing, "class", "missing");
    assert_eq!(p.cs(missing, "background-color"), "rgba(0, 0, 0, 0)");

    // Inline shorthands expand too; untouched properties keep their initial values.
    let inline = p.add("div");
    p.style(inline, "flex: none; padding: 1px 2px");
    assert_eq!(p.cs(inline, "flex-grow"), "0");
    assert_eq!(p.cs(inline, "padding-left"), "2px");
    assert_eq!(p.cs(inline, "max-width"), "none");
}

#[test]
fn computed_style_reads_svg_presentation_attributes() {
    let mut p = page("<!DOCTYPE html><body><div id=host style='color: rgb(9, 8, 7)'><svg id=icon stroke='currentColor' fill='none'><path id=path d='M0 0'></path></svg></div><svg id=plain><rect id=rect></rect></svg></body>");
    p.add_style("#rect{fill:#00f}");
    let (icon, path, rect, plain) = (p.id("icon"), p.id("path"), p.id("rect"), p.id("plain"));
    assert_eq!(p.cs(icon, "stroke"), "rgb(9, 8, 7)");
    assert_eq!(p.cs(path, "stroke"), "rgb(9, 8, 7)");
    assert_eq!(p.cs(path, "fill"), "none");
    // Author rules beat presentation attributes; initial values apply otherwise.
    assert_eq!(p.cs(rect, "fill"), "rgb(0, 0, 255)");
    assert_eq!(p.cs(plain, "stroke"), "none");
    assert_eq!(p.cs(plain, "fill"), "rgb(0, 0, 0)");
}

#[test]
fn computed_lengths_use_cascaded_font_sizes() {
    let mut p = page("<!DOCTYPE html><body><div id=outer><span id=inner>x</span></div></body>");
    p.add_style("html{font-size:14px} #outer{font-size:1.5em;padding-left:1em;margin-left:2rem} #inner{font-size:50%;width:calc(100% - 8px);height:10vh}");
    let (outer, inner) = (p.id("outer"), p.id("inner"));
    assert_eq!(p.cs(outer, "font-size"), "21px");
    assert_eq!(p.cs(outer, "padding-left"), "21px");
    assert_eq!(p.cs(outer, "margin-left"), "28px");
    assert_eq!(p.cs(inner, "font-size"), "10.5px");
    // vh resolves against the 1920x1080 default viewport.
    assert_eq!(p.cs(inner, "height"), "108px");
}

#[test]
fn flow_style_reports_used_values_from_the_cascade() {
    use lp_dom::flow::{Dim, FlowStyleSource, InnerDisplay, Len, OuterDisplay, Position};
    let mut p = page(concat!(
        "<!DOCTYPE html><body><div id=pop><ul id=list><li id=item>a</li></ul><span id=s>t</span>",
        "<svg id=svg><path id=path></path></svg></div><p id=para>p</p><p id=gone hidden>h</p></body>"
    ));
    p.add_style(concat!(
        "html{font-size:14px}",
        "#pop{position:absolute;top:120px;left:12.5rem;width:calc(100% - 8px);padding:4px 8px;",
        "border:1px solid red;box-sizing:border-box;overflow-y:auto;max-height:300px}",
        "#list{display:flex;margin:0 auto;padding-left:0;flex-direction:column;flex-wrap:wrap;gap:calc(var(--g, 2px) * 2) 3px;align-items:center;justify-content:space-between}",
        "#item{flex:1;align-self:flex-end}",
        "#item{line-height:1.5;white-space:nowrap;font-size:1.5em}",
        "#s{zoom:125%}#para{zoom:0.5}",
    ));
    let style = p.layout.style();
    let get = |id: &str| style.flow_style(&p.dom, p.id(id));

    let pop = get("pop").unwrap();
    assert_eq!((pop.outer, pop.inner, pop.position), (OuterDisplay::Block, InnerDisplay::Flow, Position::Absolute));
    assert_eq!(pop.inset[0], Dim::px(120.0));
    assert_eq!(pop.inset[3], Dim::px(175.0));
    assert_eq!(pop.width, Dim::Len(Len { px: -8.0, percent: 100.0 }));
    assert_eq!(pop.padding, [Len::px(4.0), Len::px(8.0), Len::px(4.0), Len::px(8.0)]);
    assert_eq!(pop.border, [1.0; 4]);
    // `overflow-y: auto` alone: `overflow-x: visible` computes to `auto`
    // (CSS Overflow 3), so both axes clip, as in Chromium.
    assert!(pop.border_box && pop.clips_y && pop.clips_x);
    assert_eq!(pop.max_height, Dim::px(300.0));
    assert_eq!(pop.font_size, 14.0);

    let list = get("list").unwrap();
    assert_eq!((list.outer, list.inner), (OuterDisplay::Block, InnerDisplay::Flex));
    assert_eq!(list.margin[1], Dim::Auto);
    // Author padding-left overrides the UA 40px; UA vertical margins are replaced by the author's 0.
    assert_eq!(list.padding[3], Len::ZERO);
    assert_eq!(list.margin[0], Dim::px(0.0));

    use lp_dom::flow::{Align, FlexDirection, Justify};
    assert_eq!((list.flex_direction, list.flex_wrap), (FlexDirection::Column, true));
    assert_eq!((list.row_gap, list.column_gap), (Len::px(4.0), Len::px(3.0)));
    assert_eq!((list.align_items, list.justify_content, list.align_self), (Align::Center, Justify::SpaceBetween, None));
    assert_eq!((list.flex_grow, list.flex_shrink, list.flex_basis), (0.0, 1.0, Dim::Auto));
    let item = get("item").unwrap();
    assert_eq!((item.flex_grow, item.flex_shrink, item.flex_basis), (1.0, 1.0, Dim::Len(Len::ZERO)));
    assert_eq!(item.align_self, Some(Align::End));
    assert_eq!(item.font_size, 21.0);
    assert_eq!(item.line_height, Some(31.5));
    assert!(item.nowrap);

    assert_eq!(get("s").unwrap().outer, OuterDisplay::Inline);
    assert_eq!((get("s").unwrap().zoom, get("para").unwrap().zoom, pop.zoom), (1.25, 0.5, 1.0));
    let svg = get("svg").unwrap();
    assert!(svg.replaced);
    assert_eq!(get("path").unwrap().outer, OuterDisplay::Contents);
    // UA margins when no author value applies: p gets 1em at its 14px
    // root-inherited size, zoomed by its `zoom: 0.5` (used values carry the
    // effective zoom, as Chromium's standardized zoom lays it out): 7px.
    assert_eq!(get("para").unwrap().margin[0], Dim::px(7.0));
    assert_eq!(get("para").unwrap().font_size, 7.0);
    assert!(get("gone").is_none());
}

#[test]
fn flow_style_reports_table_parts_and_spacing() {
    use lp_dom::flow::{FlowStyleSource, InnerDisplay, Len, TablePart};
    let mut p = page(concat!(
        "<!DOCTYPE html><body><table id=t><thead id=h><tr><th id=th>a</th></tr></thead>",
        "<tbody id=b><tr id=r><td id=td>b</td></tr></tbody></table>",
        "<table id=fixed><tr><td id=c>c</td></tr></table></body>"
    ));
    p.add_style("#fixed{border-collapse:collapse;table-layout:fixed;border-spacing:4px 6px} #c{padding:3px}");
    let style = p.layout.style();
    let get = |id: &str| style.flow_style(&p.dom, p.id(id)).unwrap();
    assert_eq!((get("t").inner, get("t").border_spacing), (InnerDisplay::Table, (2.0, 2.0)));
    assert_eq!(get("h").table_part, TablePart::HeaderGroup);
    assert_eq!(get("b").table_part, TablePart::BodyGroup);
    assert_eq!(get("r").table_part, TablePart::Row);
    assert_eq!((get("th").table_part, get("td").table_part), (TablePart::Cell, TablePart::Cell));
    assert_eq!(get("td").padding, [Len::px(1.0); 4]);
    let fixed = get("fixed");
    assert!(fixed.border_collapse && fixed.table_layout_fixed);
    assert_eq!(fixed.border_spacing, (4.0, 6.0));
    // border-collapse inherits to the cell; author padding replaces the UA 1px.
    assert!(get("c").border_collapse);
    assert_eq!(get("c").padding, [Len::px(3.0); 4]);
}

#[test]
fn computed_style_maps_logical_properties() {
    let mut p = page("<!DOCTYPE html><body><button id=b class='px-3 py-1 border-x'>x</button></body>");
    p.add_style(concat!(
        ":root{--spacing-3:.75rem;--spacing-1:.25rem}",
        ".px-3{padding-inline:var(--spacing-3)}.py-1{padding-block:var(--spacing-1)}",
        ".border-x{border-inline-style:solid;border-inline-width:1px}",
    ));
    let b = p.id("b");
    assert_eq!(p.cs(b, "padding-left"), "12px");
    assert_eq!(p.cs(b, "padding-right"), "12px");
    assert_eq!(p.cs(b, "padding-top"), "4px");
    assert_eq!(p.cs(b, "border-left-width"), "1px");
    assert_eq!(p.cs(b, "border-right-style"), "solid");
    // The UA button border (Chrome: 2px outset; the legacy cascade has none).
    assert_eq!(p.cs(b, "border-top-width"), "2px");
}

#[test]
fn flow_style_reports_inherited_text_align() {
    use lp_dom::flow::{FlowStyleSource, TextAlign};
    let mut p = page("<!DOCTYPE html><body><div id=c class=text-center><span id=s>x</span></div><p id=r style='text-align:end'>r</p><table><tr><th id=th>h</th><td id=td>d</td></tr></table></body>");
    p.add_style(".text-center{text-align:center}");
    let style = p.layout.style();
    let get = |id: &str| style.flow_style(&p.dom, p.id(id)).unwrap().text_align;
    assert_eq!((get("c"), get("s"), get("r")), (TextAlign::Center, TextAlign::Center, TextAlign::End));
    assert_eq!((get("th"), get("td")), (TextAlign::Center, TextAlign::Start));
}

#[test]
fn flow_inputs_keep_full_precision() {
    use lp_dom::flow::FlowStyleSource;
    let mut p = page("<!DOCTYPE html><body><button id=b class='text-sm'>Today</button></body>");
    p.add_style(".text-sm{font-size:.875rem;line-height:calc(1.25 / .875);padding-block:calc(.25rem * 1.5)}");
    let b = p.id("b");
    // getComputedStyle reports the resolved length (Chrome: 20px; the
    // legacy cascade: the number)...
    assert_eq!(p.cs(b, "line-height"), "20px");
    // ...but layout sees the exact value: 14px x 1.25/0.875 = 20px.
    let style = p.layout.style().flow_style(&p.dom, b).unwrap();
    assert!((style.line_height.unwrap() - 20.0).abs() < 1e-9, "{:?}", style.line_height);
    assert!((style.padding[0].px - 6.0).abs() < 1e-9);
}

#[test]
fn inert_content_is_not_hit_testable() {
    // react-aria's popover underlay is `<div inert>` covering the viewport;
    // Blink's UsedPointerEvents is `none` for inert content while
    // getComputedStyle still reports the cascaded value.
    let p = page(
        r#"<!DOCTYPE html>
<body>
  <div id="wrap" inert>
    <div id="inner">Inner</div>
  </div>
  <div id="plain">Plain</div>
</body>"#,
    );
    let inner = p.id("inner");
    assert_eq!(p.cs(inner, "pointer-events"), "auto");
    let (x, y) = p.center(inner);
    let hit = p.efp(x, y);
    assert!(hit != Some(inner) && hit != Some(p.id("wrap")), "{hit:?}");
    assert!(!p.efps(x, y).iter().any(|&n| n == inner || n == p.id("wrap")));
    let plain = p.id("plain");
    let (x, y) = p.center(plain);
    assert_eq!(p.efp(x, y), Some(plain));
}

#[test]
fn flow_line_height_snaps_to_layout_units() {
    use lp_dom::flow::FlowStyleSource;
    let mut p = page("<!DOCTYPE html><body><button id=a>Today</button><span id=b>c</span><span id=c>d</span></body>");
    p.add_style("#a{font-size:.8125rem;line-height:1.8462}#b{font-size:12px;line-height:1.3333}#c{line-height:20.3px}");
    let lh = |p: &Page, id: &str| p.layout.style().flow_style(&p.dom, p.id(id)).unwrap().line_height.unwrap();
    // Chromium lays out TOS's text-small Today button as 24px + padding, not
    // 13 x 1.8462 = 24.0006px; numbers truncate to 1/64 px, fixed lengths round.
    assert_eq!(lh(&p, "a"), 24.0);
    assert_eq!(lh(&p, "b"), 15.984375);
    assert_eq!(lh(&p, "c"), 20.296875);
}

#[test]
fn flow_style_reports_used_font_weight() {
    use lp_dom::flow::FlowStyleSource;
    let mut p = page(
        "<!DOCTYPE html><body><div id=semi><span id=inh>a</span><span id=bolder>b</span></div><b id=b>c</b><strong id=s>d</strong><h1 id=h>e</h1><table><tr><th id=th>f</th></tr></table><span id=n>g</span></body>",
    );
    p.add_style("#semi{font-weight:600}#bolder{font-weight:bolder}");
    let w = |p: &Page, id: &str| p.layout.style().flow_style(&p.dom, p.id(id)).unwrap().font_weight;
    assert_eq!(p.cs(p.id("bolder"), "font-weight"), "900");
    assert_eq!(p.cs(p.id("th"), "font-weight"), "700");
    // Chromium: inherited 600; bolder than 600 is 900; UA bold elements are 700,
    // and getComputedStyle reports the resolved number.
    assert_eq!(w(&p, "inh"), 600);
    assert_eq!(w(&p, "bolder"), 900);
    assert_eq!(w(&p, "b"), 700);
    assert_eq!(w(&p, "s"), 700);
    assert_eq!(w(&p, "h"), 700);
    assert_eq!(w(&p, "th"), 700);
    assert_eq!(w(&p, "n"), 400);
}

#[test]
fn svg_bbox_covers_shapes_paths_and_containers() {
    let p = page(
        r#"<!DOCTYPE html><body><svg width="600" height="300">
  <rect id="r" x="10" y="20" width="30" height="40"/>
  <path id="bar" d="M 96,145.3333 h 102 v 38.6667 h -102 Z"/>
  <path id="rounded" d="M202,238A 4,4,0,0,0,206,242L 300,242A 4,4,0,0,0,
        304,238L 304,184L 202,184 Z"/>
  <path id="cubic" d="M0,0 C0,100 100,100 100,0"/>
  <path id="arc" d="M0,50 A50,50 0 0 1 100,50"/>
  <path id="rel" d="m10 10l5-5q5 10 10 0t10 0zM1e1 1.5e1"/>
  <circle id="c" cx="50" cy="50" r="10"/>
  <polygon id="poly" points="0,0 10,5 3,8"/>
  <g id="g"><rect x="0" y="0" width="10" height="10" transform="translate(10,5)"/><circle cx="0" cy="0" r="2"/><rect x="99" width="5" height="5" style="display:none"/></g>
  <linearGradient id="grad"/>
</svg></body>"#,
    );
    let bb = |id: &str| {
        let r = lp_dom::svg_geometry::bbox(&p.dom, p.layout.style(), p.id(id));
        [r.x, r.y, r.width, r.height].map(|v| (v * 1e4).round() / 1e4)
    };
    assert_eq!(bb("r"), [10.0, 20.0, 30.0, 40.0]);
    assert_eq!(bb("bar"), [96.0, 145.3333, 102.0, 38.6667]);
    // recharts' rounded bar: the arcs stay inside the corners.
    assert_eq!(bb("rounded"), [202.0, 184.0, 102.0, 58.0]);
    // Curves use their extrema, not their control points.
    assert_eq!(bb("cubic"), [0.0, 0.0, 100.0, 75.0]);
    assert_eq!(bb("arc"), [0.0, 0.0, 100.0, 50.0]);
    // Relative commands, a reflected `t` control (y = -5 gives a minimum of 0)
    // and exponent numbers; a trailing moveto counts.
    assert_eq!(bb("rel"), [10.0, 0.0, 25.0, 15.0]);
    assert_eq!(bb("c"), [40.0, 40.0, 20.0, 20.0]);
    assert_eq!(bb("poly"), [0.0, 0.0, 10.0, 8.0]);
    // Children's transforms apply; display:none children do not count.
    assert_eq!(bb("g"), [-2.0, -2.0, 22.0, 17.0]);
    assert_eq!(bb("grad"), [0.0, 0.0, 0.0, 0.0]);
}

#[test]
fn canvas_font_shorthand_parses_like_chromium() {
    use lp_dom::text_metrics::parse_font;
    let f = parse_font("bold 12px sans-serif", 10.0).unwrap();
    assert_eq!(
        (f.weight, f.size, f.family.as_str(), f.serialize().as_str()),
        (700, 12.0, "sans-serif", "700 12px sans-serif")
    );
    let f = parse_font("400 16px / 24px system-ui, -apple-system", 10.0).unwrap();
    assert_eq!(
        (f.size, f.family.as_str(), f.serialize().as_str()),
        (16.0, "system-ui, -apple-system", "16px system-ui, -apple-system")
    );
    let f = parse_font("italic 600 1.5em/2 Inter", 10.0).unwrap();
    assert_eq!((f.style.as_str(), f.weight, f.size), ("italic", 600, 15.0));
    assert!(parse_font("12px", 10.0).is_none());
    assert!(parse_font("nonsense", 10.0).is_none());
    assert!(parse_font("12px Menlo, monospace", 10.0).unwrap().is_monospace());
}

// Chromium blockifies flex/grid items, floats, absolutely positioned boxes
// and the root (css-display-3 §2.7). Playwright's accessible name pads
// non-inline children, so the @povio/ui `<label class=flex>Service
// Type<span>*</span></label>` names its button "Service Type *".
#[test]
fn computed_display_blockifies_flex_and_grid_items() {
    let mut p = page(
        "<label id=label style='display:flex'>Service Type<span id=star>*</span>\
           <div id=contents style='display:contents'><b id=deep>x</b></div>\
           <i id=ib style='display:inline-flex'></i><em id=cell style='display:table-cell'></em>\
           <span id=hidden style='display:none'></span></label>\
         <div id=grid style='display:inline-grid'><a id=link>a</a></div>\
         <p><span id=plain>p</span><span id=float style='float:left'>f</span>\
           <span id=abs style='position:absolute'>a</span><button id=btn>b</button><span id=li style='display:list-item'></span></p>",
    );
    let cs = |p: &Page, id: &str| p.cs(p.id(id), "display");
    assert_eq!(cs(&p, "label"), "flex");
    assert_eq!(cs(&p, "star"), "block", "flex item");
    assert_eq!(cs(&p, "contents"), "contents", "display:contents is not blockified");
    assert_eq!(cs(&p, "deep"), "block", "a child of a display:contents flex item is a flex item");
    assert_eq!(cs(&p, "ib"), "flex");
    assert_eq!(cs(&p, "cell"), "block");
    assert_eq!(cs(&p, "hidden"), "none");
    assert_eq!(cs(&p, "grid"), "inline-grid");
    assert_eq!(cs(&p, "link"), "block", "grid item");
    assert_eq!(cs(&p, "plain"), "inline");
    assert_eq!(cs(&p, "float"), "block");
    assert_eq!(cs(&p, "abs"), "block");
    assert_eq!(cs(&p, "btn"), "inline-block", "UA default");
    assert_eq!(cs(&p, "li"), "list-item");
    let html = p.html();
    assert_eq!(p.cs(html, "display"), "block");

    // A container switching to flex blockifies existing children.
    let label = p.id("label");
    p.style(label, "display:block");
    assert_eq!(cs(&p, "star"), "inline");
    p.style(label, "display:grid");
    assert_eq!(cs(&p, "star"), "block");
}

#[test]
fn cascade_ancestor_prefilter_never_rejects_a_matching_rule() {
    // Inside a layout pass, generic rules whose ancestor compounds need a
    // tag, id or class no ancestor has are skipped before matching (an
    // ancestor Bloom filter). Differential: every element's value in a pass
    // equals full matching outside one, for each selector, in no-quirks and
    // quirks documents, including a shadow tree.
    let rules = [
        ".a .x",
        ".a > section .x",
        "#Top .x",
        "#top .x",
        ".B .x",
        ".b .x",
        "div .x",
        "DIV .x",
        "section p > span",
        "p > .x",
        ":is(.a) .x",
        ":where(.c) .x",
        ":is(.a, .zz) .x",
        ":where(.zz .q, .a) .x",
        "[data-k] .x",
        "[data-k=v] > li .x",
        "ul li i",
        "svg .x",
        "foreignObject .x",
        "foreignobject .x",
        "g > rect",
        "@namespace svg url(http://www.w3.org/2000/svg); svg|g .x",
        "*|g .x",
        ".c + ul .x",
        "section ~ div em",
        ".a .c ~ ul > li > .x",
        ":not(.zz) .x",
        ".a :not(.c) .x",
        ".a:has(.y) .x",
        "li:first-child .x",
        ".c .x",
        "span.x",
        ":host .x",
        ":host(.c) .x",
        "body :host .x",
        "div :host(.c) > .a > span",
        ".a .c .x .zz",
    ];
    // Inherited values at several levels (memoized per pass too).
    let body = r#"<div id=Top class="a B" style="color:rgb(1,2,3);font-weight:bolder;white-space:nowrap;line-height:1.5"><section class=c><p id=p class="x y" style="font-weight:lighter;white-space:normal"><span class=x>t</span><b>b</b></p></section><ul data-k=v><li class=x><i class=x>i</i></li></ul><svg><foreignObject><div class=x>f</div></foreignObject><g class=g><rect class=x /></g></svg><div class=FOO><em class=x>e</em></div></div>"#;
    let decl = "{margin-left:7px}";
    for doctype in ["<!doctype html>", ""] {
        let mut matched = 0;
        for rule in rules {
            let html = format!("{doctype}<style>{rule}{decl}</style><body>{body}</body>");
            let build = || {
                let mut p = page(&html);
                // A shadow tree under section.c: its elements match from the
                // shadow root up (full matching stops there).
                let host = p
                    .dom
                    .descendants(p.doc)
                    .unwrap()
                    .find(|&n| p.dom.tag_name(n).ok().as_deref() == Some("SECTION"))
                    .unwrap();
                let root = p.dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
                let (outer, inner) = (p.create("div"), p.create("span"));
                p.dom.set_attribute(outer, "class", "a").unwrap();
                p.dom.set_attribute(inner, "class", "x").unwrap();
                p.append(root, outer);
                p.append(outer, inner);
                // The rule in a shadow-scoped sheet too (`:host`).
                let style = p.create("style");
                let text = p.dom.create_text(p.doc, &format!("{rule}{decl}")).unwrap();
                p.append(style, text);
                p.append(root, style);
                p.layout.style_mut().discover_style_elements(&p.dom);
                let mut els: Vec<NodeId> = p.dom.descendants(p.doc).unwrap().collect();
                els.extend([outer, inner]);
                els.retain(|&n| p.dom.node_type(n).ok() == Some(NodeType::Element));
                (p, els)
            };
            let (full, els) = build();
            let props = ["margin-left", "color", "font-weight", "white-space", "line-height"];
            let values = |p: &Page| -> Vec<Vec<Option<String>>> {
                els.iter().map(|&e| props.iter().map(|n| p.layout.style().used_value(&p.dom, e, n)).collect()).collect()
            };
            let reference = values(&full);
            let (pass, els2) = build();
            assert_eq!(els, els2);
            let filtered = lp_dom::style::with_pass_memo(|| values(&pass));
            assert_eq!(filtered, reference, "{doctype:?} {rule}");
            matched += reference.iter().any(|v| v[0].as_deref() == Some("7px")) as usize;
        }
        // Not vacuous: most rules match something (no-quirks misses the
        // case-mismatched id/class/tag, @namespace, the `:host` rules, which
        // the cascade does not match today, and the impossible chain).
        assert!(matched >= rules.len() - 9, "{doctype:?}: only {matched} rules matched");
    }
}

#[test]
fn offset_parent_includes_stylesheet_positioned_drawer() {
    let p = page("<style>.drawer{position:fixed;right:0;top:0;width:200px;height:300px}.popover{position:absolute;top:20px;left:10px;width:50px;height:50px}</style><div id='drawer' class='drawer'><div style='display:contents'><div id='popover' class='popover'></div></div></div>");
    assert_eq!(p.layout.offset_parent(&p.dom, p.id("popover")), Some(p.id("drawer")));
    assert_eq!(p.layout.offset_parent(&p.dom, p.id("drawer")), None);
}
