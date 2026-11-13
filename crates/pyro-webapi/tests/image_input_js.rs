//! `<input type=image>` as a page sees it: its `src` is fetched as an image
//! resource, the element is laid out as a replaced box at the image's
//! natural size (like `<img>`), and the coordinate an image button submits
//! is relative to that box (xhr/formdata/constructor-submitter-coordinate).

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::parse::{parse_document, DocumentParser, DomRef, ParseStep};
use pyro_dom::script::{ResourceKind, ResourceReason};
use pyro_dom::{DocumentKind, Dom};
use pyro_js::{ClassDef, Engine, JsClass, Realm, RealmOptions};
use pyro_webapi::*;

struct TestWindowData {
    page: Rc<PageDom>,
    target: EventTargetData,
}

impl WindowHost for TestWindowData {
    fn page(&self) -> &Rc<PageDom> {
        &self.page
    }
    fn event_target(&self) -> &EventTargetData {
        &self.target
    }
}

struct TestWindow;
impl JsClass for TestWindow {
    const NAME: &'static str = "Window";
    type Data = TestWindowData;
    type Parent = EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|w: &TestWindowData| &w.target);
        define_members::<TestWindow>(def);
        def.getter("window", |s, _w| Ok(s.global().value()));
    }
}

struct Env {
    _engine: Engine,
    page: Rc<PageDom>,
    main: Realm,
}

/// Loads no sub-resources: the images' natural sizes are recorded directly,
/// as the embedder does once an image has loaded.
struct NoFetch;
impl pyro_render::Fetcher for NoFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err(format!("{url}: offline")));
    }
}

fn env(html: &str) -> Env {
    pyro_layout::install(std::sync::Arc::new(NoFetch));
    let engine = Engine::new().expect("engine");
    register(&engine);
    let window_class = engine.register::<TestWindow>();
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom, "http://localhost:5173/app", html);
    dom.borrow_mut().set_document_active(doc, true).unwrap();
    let page = PageDom::new(dom, doc);
    let mut vp = page.viewport();
    vp.width = 1440;
    vp.height = 900;
    page.set_viewport(vp);
    page.layout.borrow_mut().style_mut().discover_style_elements(&page.dom.borrow());
    let realm = engine
        .create_realm(RealmOptions { name: "main".into(), global_class: Some(window_class), origin: None })
        .expect("realm");
    let key = page.window_target().unwrap_or_else(|| page.create_window_target(page.document()));
    realm
        .set_global_data::<TestWindow>(TestWindowData {
            page: page.clone(),
            target: EventTargetData::window(&page, key),
        })
        .unwrap();
    {
        let s = realm.scope().unwrap();
        install(&s, page.clone()).unwrap();
    }
    Env { _engine: engine, page, main: realm }
}

impl Env {
    /// Records `#id`'s image as loaded at `w`x`h` CSS px.
    fn loaded(&self, id: &str, w: f64, h: f64) {
        let mut dom = self.page.dom.borrow_mut();
        let el = dom.get_element_by_id(self.page.document(), id).unwrap().unwrap_or_else(|| panic!("#{id}"));
        dom.set_natural_size(el, Some((w, h)));
    }
}

fn eval(realm: &Realm, src: &str) -> String {
    let s = realm.scope().unwrap();
    match s.eval(src, Some("test.js"), 1) {
        Ok(v) => v.to_string().unwrap(),
        Err(e) => format!("THROW {}", e.details(&s).text()),
    }
}

const RECT: &str = "(el => { const r = el.getBoundingClientRect(); return [r.x, r.y, r.width, r.height].join(','); })";

#[test]
fn image_input_is_a_replaced_box_at_its_natural_size() {
    let e = env(r#"<!doctype html><html><body>
        <div><input id=button type=image src=/images/100px-green-rect.svg style="display:block"></div>
        <div><img id=img src=/images/100px-green-rect.svg style="display:block"></div>
        <div><input id=sized type=image src=/images/100px-green-rect.svg width=50 style="display:block"></div>
        <div><input id=unloaded type=image src=/images/missing.svg style="display:block"></div>
        <input id=plain type=image src=/images/missing.svg>
        </body></html>"#);
    e.loaded("button", 200.0, 200.0);
    e.loaded("img", 200.0, 200.0);
    e.loaded("sized", 200.0, 200.0);
    let r = &e.main;
    // Positions are Chromium's for these block-level boxes (an inline-level
    // image button in a block sits on the baseline, a line layout detail
    // this test does not cover).
    assert_eq!(eval(r, &format!("{RECT}(document.getElementById('button'))")), "8,8,200,200");
    assert_eq!(eval(r, &format!("{RECT}(document.getElementById('img'))")), "8,208,200,200");
    // The width attribute maps to CSS width; height follows the aspect ratio.
    assert_eq!(eval(r, &format!("{RECT}(document.getElementById('sized'))")), "8,408,50,50");
    // Before any load the box is empty, not a text input's. (Chromium shows
    // a 57x16 alt-text button for an unavailable image, which is not
    // modelled; a broken `<img>` is an empty box here too.)
    assert_eq!(eval(r, &format!("{RECT}(document.getElementById('unloaded'))")), "8,458,0,0");
    // Chromium's UA sheet: no text-input border or padding on an image button.
    assert_eq!(
        eval(
            r,
            "(() => { const s = getComputedStyle(document.getElementById('plain')); return [s.borderTopWidth, s.paddingLeft, s.display].join(' '); })()"
        ),
        "0px 0px inline-block"
    );
}

#[test]
fn wpt_constructor_submitter_coordinate() {
    // xhr/formdata/constructor-submitter-coordinate.html: the click's
    // position inside a 200x200 image button at (8, 8).
    let e = env(r#"<!doctype html><html><body>
        <form><input id=submitter type=image src=/images/100px-green-rect.svg></form>
        </body></html>"#);
    e.loaded("submitter", 200.0, 200.0);
    assert_eq!(
        eval(
            &e.main,
            r#"(() => {
              const form = document.querySelector("form");
              const submitter = form.querySelector("input[type=image]");
              let formData;
              form.addEventListener("submit", e => { e.preventDefault(); formData = new FormData(form, submitter); });
              const domRect = submitter.getBoundingClientRect();
              const clientX = Math.round((domRect.left + domRect.right) / 7);
              const clientY = Math.round((domRect.top + domRect.bottom) / 4);
              submitter.dispatchEvent(new MouseEvent("click", {clientX, clientY}));
              return formData?.get("x") + "," + formData?.get("y");
            })()"#
        ),
        "23,46"
    );
}

#[test]
fn image_button_src_is_fetched_as_an_image_resource() {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    dom.borrow_mut().set_resource_tracking(true);
    let doc = dom.borrow_mut().create_document(DocumentKind::Html, "http://t/");
    dom.borrow_mut().set_document_active(doc, true).unwrap();
    let mut p = DocumentParser::new(dom.clone(), doc, true).unwrap();
    let mut step = p.feed(
        r#"<body><input id=image type=image src=i.png><input id=upper type=IMAGE src=i.png>
           <input id=text type=text src=i.png><input id=later src=i.png>"#,
    );
    while let ParseStep::Script(_) = step {
        step = p.resume();
    }
    p.end();
    let mut d = dom.borrow_mut();
    let id = |d: &Dom, id: &str| d.get_element_by_id(doc, id).unwrap().unwrap();
    let (image, upper, text, later) = (id(&d, "image"), id(&d, "upper"), id(&d, "text"), id(&d, "later"));
    let evs = d.take_resource_events();
    let summary: Vec<_> = evs.iter().map(|e| (e.node, e.kind, e.reason)).collect();
    assert!(summary.contains(&(image, ResourceKind::Img, ResourceReason::Connected)), "{summary:?}");
    assert!(summary.contains(&(upper, ResourceKind::Img, ResourceReason::Connected)), "{summary:?}");
    // Other input types do not fetch their `src`.
    assert!(!summary.iter().any(|(n, _, _)| *n == text || *n == later), "{summary:?}");
    // Becoming an image button starts the load.
    d.set_attribute(later, "type", "image").unwrap();
    let evs = d.take_resource_events();
    let summary: Vec<_> = evs.iter().map(|e| (e.node, e.kind, e.reason)).collect();
    assert_eq!(summary, [(later, ResourceKind::Img, ResourceReason::AttrChanged)]);
}
