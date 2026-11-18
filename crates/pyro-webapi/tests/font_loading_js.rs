//! CSS Font Loading (`FontFace`, `document.fonts`) through the bindings on the real
//! JavaScriptCore: descriptors, the set's CSS-connected faces, and the faces scripts add reaching
//! the style engine as `@font-face` rules.

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

fn script_fonts(e: &Env) -> String {
    let layout = e.page.layout.borrow();
    let doc = e.page.document();
    layout.style().adopted_sheets(doc).iter().map(|(_, t)| t.to_string()).collect::<Vec<_>>().join("\n")
}

#[test]
fn font_face_descriptors_parse_and_serialize() {
    let e = env("<!doctype html><p>x</p>");
    let r = &e.main;
    assert_eq!(eval(r, "typeof FontFace + typeof FontFaceSet + typeof FontFaceSetLoadEvent"), "functionfunctionfunction");
    assert_eq!(
        eval(r, "const f = new FontFace('Web Font', 'url(a.woff2)', {weight: 'bold', unicodeRange: 'u+400-4ff'}); [f.family, f.weight, f.unicodeRange, f.style, f.status].join('|')"),
        "Web Font|bold|U+400-4FF|normal|unloaded"
    );
    // Not a <family-name>: kept as a CSS string.
    assert_eq!(eval(r, "new FontFace('sans-serif', 'url(a)').family"), "\"sans-serif\"");
    assert_eq!(eval(r, "try { f.weight = 'heavy'; 'set' } catch (e) { e.name }"), "SyntaxError");
    assert_eq!(eval(r, "f.weight = '100 400'; f.weight"), "100 400");
    // A bad descriptor or source is an error status, not an exception.
    assert_eq!(eval(r, "new FontFace('a', 'url(a)', {style: 'sideways'}).status"), "error");
    assert_eq!(eval(r, "new FontFace('a', 'x').status"), "error");
    // Bytes are loading until checked.
    assert_eq!(eval(r, "new FontFace('a', new ArrayBuffer(8)).status"), "loading");
    assert_eq!(eval(r, "try { new FontFaceSet([]); 'made' } catch (e) { e.name }"), "TypeError");
}

#[test]
fn document_fonts_holds_css_connected_and_added_faces() {
    let e = env(
        "<!doctype html><style id=s>@font-face { font-family: Lato; src: url(lato.ttf); font-weight: 700 }</style><p>x</p>",
    );
    let r = &e.main;
    assert_eq!(eval(r, "document.fonts === document.fonts"), "true");
    assert_eq!(eval(r, "const css = [...document.fonts][0]; [document.fonts.size, css.family, css.weight].join()"), "1,Lato,700");
    assert_eq!(eval(r, "document.fonts.delete(css) + ',' + (document.fonts.clear(), document.fonts.size)"), "false,1");
    assert_eq!(eval(r, "[...document.fonts][0] === css"), "true", "the same object while the rule stays");
    assert_eq!(script_fonts(&e), "", "no script faces yet");

    assert_eq!(
        eval(r, "const face = new FontFace('Added', 'url(added.woff2)', {style: 'italic'}); document.fonts.add(face); [document.fonts.size, document.fonts.has(face)].join()"),
        "2,true"
    );
    let sheet = script_fonts(&e);
    assert!(sheet.contains("font-family:\"Added\""), "{sheet}");
    assert!(sheet.contains("url(\"http://localhost:5173/added.woff2\")"), "{sheet}");
    assert!(sheet.contains("font-style:italic"), "{sheet}");
    // check(): a matching face that is not loaded; no matching face at all.
    assert_eq!(eval(r, "[document.fonts.check('12px Added'), document.fonts.check('12px Other')].join()"), "false,true");
    assert_eq!(eval(r, "try { document.fonts.check('inherit') } catch (e) { e.name }"), "SyntaxError");

    assert_eq!(eval(r, "document.getElementById('s').remove(); document.fonts.size"), "1");
    assert_eq!(eval(r, "document.fonts.delete(face) + ',' + document.fonts.size"), "true,0");
    assert_eq!(script_fonts(&e), "", "deleting the face drops its rule");
}
