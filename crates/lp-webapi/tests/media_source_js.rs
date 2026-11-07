//! The minimal `MediaSource` surface: constructible, closed, feature
//! detection false, and `URL.createObjectURL(mediaSource)` yields a blob URL
//! that fetch and XHR cannot resolve.

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::parse::{parse_document, DomRef};
use lp_dom::Dom;
use lp_js::{ClassDef, Engine, JsClass, Realm, RealmOptions};
use lp_webapi::*;

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
    page: Rc<PageDom>,
    main: Realm,
}

/// Loads no sub-resources.
struct NoFetch;
impl lp_render::Fetcher for NoFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err(format!("{url}: offline")));
    }
}

fn env(html: &str) -> Env {
    lp_layout::install(std::sync::Arc::new(NoFetch));
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
fn media_source_is_closed_and_its_object_url_is_unfetchable() {
    let e = env("<!doctype html><title>t</title>");
    let r = &e.main;
    assert_eq!(
        eval(r, "const ms = new MediaSource(); [ms instanceof EventTarget, ms.readyState, ms.duration, ms.sourceBuffers.length, MediaSource.isTypeSupported('video/mp4'), typeof ms.onsourceopen].join()"),
        "true,closed,NaN,0,false,object"
    );
    assert_eq!(eval(r, "try { ms.addSourceBuffer('video/mp4'); 'no' } catch (e) { e.name }"), "NotSupportedError");
    assert_eq!(eval(r, "try { ms.endOfStream(); 'no' } catch (e) { e.name }"), "InvalidStateError");
    assert_eq!(
        eval(
            r,
            "const u = URL.createObjectURL(ms); [u.startsWith('blob:http://localhost:5173/'), u.length > 30].join()"
        ),
        "true,true"
    );
    // Not a blob entry: fetch and XHR resolve nothing for it.
    let url = eval(r, "u");
    assert!(e.page.blob_url(&url).is_none());
    assert_eq!(eval(r, "try { URL.createObjectURL({}); 'no' } catch (e) { e.name }"), "TypeError");
}
