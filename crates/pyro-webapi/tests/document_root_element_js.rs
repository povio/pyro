//! `Document.rootElement` (SVG 2) and XHTML `<template>` parsing through the
//! DOM bindings on the real JavaScriptCore.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::Dom;
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
    #[allow(dead_code)]
    engine: Engine,
    #[allow(dead_code)]
    page: Rc<PageDom>,
    main: Realm,
}

/// Loads no sub-resources.
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
    dom.borrow_mut().set_resource_tracking(true);
    let doc = parse_document(&dom, "http://localhost:5173/app", html);
    dom.borrow_mut().set_document_active(doc, true).unwrap();
    let page = PageDom::new(dom, doc);
    let mut vp = page.viewport();
    vp.width = 1440;
    vp.height = 900;
    page.set_viewport(vp);
    page.layout.borrow_mut().style_mut().discover_style_elements(&page.dom.borrow());
    let realm = engine
        .create_realm(RealmOptions { name: String::new(), global_class: Some(window_class), origin: None })
        .expect("realm");
    let key = page.window_target().unwrap_or_else(|| page.create_window_target(page.document()));
    realm
        .set_global_data::<TestWindow>(TestWindowData {
            page: page.clone(),
            target: EventTargetData::window(&page, key),
        })
        .unwrap();
    install(&realm.scope().unwrap(), page.clone()).unwrap();
    Env { engine, page, main: realm }
}

fn eval(realm: &Realm, src: &str) -> String {
    let s = realm.scope().unwrap();
    match s.eval(src, Some("test.js"), 1) {
        Ok(v) => v.to_string().unwrap(),
        Err(e) => format!("THROW {}", e.details(&s).text()),
    }
}

#[test]
fn root_element_is_the_svg_document_element_or_null() {
    let e = env("<!doctype html><html><body><svg xmlns='http://www.w3.org/2000/svg'></svg></body></html>");
    let r = &e.main;
    assert_eq!(eval(r, "'rootElement' in Document.prototype"), "true");
    assert_eq!(eval(r, "typeof Object.getOwnPropertyDescriptor(Document.prototype, 'rootElement').get"), "function");
    assert_eq!(eval(r, "document.rootElement"), "null", "an HTML document has no root SVG element");
    assert_eq!(eval(r, "new Document().rootElement"), "null", "no document element");
    assert_eq!(
        eval(
            r,
            "const svg = new DOMParser().parseFromString(\"<svg xmlns='http://www.w3.org/2000/svg'><script>1</script></svg>\", 'application/xml'); \
             [svg.rootElement === svg.documentElement, svg.rootElement.namespaceURI, svg.rootElement.localName].join()"
        ),
        "true,http://www.w3.org/2000/svg,svg"
    );
    assert_eq!(
        eval(r, "document.implementation.createDocument('http://www.w3.org/2000/svg', 'svg').rootElement.localName"),
        "svg"
    );
    // The name alone is not enough: the element must be in the SVG namespace.
    assert_eq!(eval(r, "new DOMParser().parseFromString('<svg/>', 'application/xml').rootElement"), "null");
    assert_eq!(
        eval(r, "new DOMParser().parseFromString(\"<svg xmlns='http://www.w3.org/1999/xhtml'/>\", 'application/xml').rootElement"),
        "null"
    );
    // Other SVG elements are not root elements, and a navigated-into SVG
    // root is reported live.
    assert_eq!(eval(r, "new DOMParser().parseFromString(\"<g xmlns='http://www.w3.org/2000/svg'/>\", 'application/xml').rootElement"), "null");
    assert_eq!(
        eval(
            r,
            "const d = new Document(); d.appendChild(d.createElementNS('http://www.w3.org/2000/svg', 'svg')); \
             const was = d.rootElement === d.documentElement; d.documentElement.remove(); [was, d.rootElement].join()"
        ),
        "true,"
    );
}

#[test]
fn xhtml_template_contents_are_reachable_through_content() {
    let e = env("<!doctype html><html><body></body></html>");
    let r = &e.main;
    assert_eq!(
        eval(
            r,
            "const x = new DOMParser().parseFromString(\"<template xmlns='http://www.w3.org/1999/xhtml'><test/></template>\", 'text/xml'); \
             const t = x.documentElement; \
             [t instanceof HTMLTemplateElement, t.childElementCount, t.content.firstChild.localName, t.content.ownerDocument !== x, \
              t.innerHTML, t.outerHTML].join('|')"
        ),
        "true|0|test|true|<test></test>|<template xmlns=\"http://www.w3.org/1999/xhtml\"><test></test></template>"
    );
    assert_eq!(
        eval(
            r,
            "const c = t.cloneNode(true); [c.childElementCount, c.content.childNodes.length, c.content.firstChild.localName, \
             document.importNode(t, true).content.firstChild.localName].join('|')"
        ),
        "0|1|test|test"
    );
    // Templates outside the XHTML namespace keep their children.
    for ns in ["", " xmlns='http://www.w3.org/2000/svg'"] {
        assert_eq!(
            eval(
                r,
                &format!(
                    "(() => {{ const n = new DOMParser().parseFromString(\"<template{ns}><test/></template>\", 'text/xml').documentElement; \
                     return [n.childElementCount, n.firstChild.localName, n.content].join('|') }})()"
                )
            ),
            "1|test|",
            "{ns:?}"
        );
    }
}
