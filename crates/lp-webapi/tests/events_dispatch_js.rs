//! Dispatch bookkeeping on the real JavaScriptCore: the realm's current
//! event (`window.event`) across nested dispatches and throwing listeners,
//! the listener slots dispatch reads back (add, remove, `once`, handlers,
//! `handleEvent` objects, slot reuse), an event object dispatched again at
//! other targets, computed-style longhands as prototype attributes, the
//! strings `id` and `className` return, and the payloads of finalized
//! wrappers given back inside one long task.

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::parse::{parse_document, DomRef};
use lp_dom::Dom;
use lp_js::{ClassDef, ConsoleLevel, Engine, EngineHooks, JsClass, JsException, Realm, RealmOptions, Value};
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

#[derive(Default)]
struct Recorder {
    exceptions: RefCell<Vec<String>>,
}

impl EngineHooks for Recorder {
    fn console_api(&self, _realm: &Realm, _level: ConsoleLevel, _args: &[Value<'_>]) {}
    fn exception_thrown(&self, _realm: &Realm, e: &JsException) {
        self.exceptions.borrow_mut().push(e.text());
    }
}

struct Env {
    engine: Engine,
    page: Rc<PageDom>,
    main: Realm,
    #[allow(dead_code)]
    hooks: Rc<Recorder>,
}

struct NoFetch;
impl lp_render::Fetcher for NoFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err(format!("{url}: offline")));
    }
}

fn env(html: &str) -> Env {
    lp_layout::install(std::sync::Arc::new(NoFetch));
    let engine = Engine::new().expect("engine");
    let hooks = Rc::new(Recorder::default());
    engine.set_hooks(Some(hooks.clone()));
    register(&engine);
    let window_class = engine.register::<TestWindow>();
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom, "http://localhost:5173/app", html);
    dom.borrow_mut().set_document_active(doc, true).unwrap();
    let page = PageDom::new(dom, doc);
    let main = engine
        .create_realm(RealmOptions { name: String::new(), global_class: Some(window_class), origin: None })
        .expect("realm");
    let key = page.window_target().unwrap_or_else(|| page.create_window_target(page.document()));
    main.set_global_data::<TestWindow>(TestWindowData { page: page.clone(), target: EventTargetData::window(&page, key) })
        .unwrap();
    install(&main.scope().unwrap(), page.clone()).unwrap();
    main.set_exception_reporter(Some(lifecycle::report_exception));
    Env { engine, page, main, hooks }
}

fn eval(realm: &Realm, src: &str) -> String {
    let s = realm.scope().unwrap();
    match s.eval(src, Some("test.js"), 1) {
        Ok(v) => v.to_string().unwrap(),
        Err(e) => format!("THROW {}", e.details(&s).text()),
    }
}

#[test]
fn the_current_event_nests_and_is_restored_after_a_throwing_listener() {
    let e = env("<body><div id=a><span id=b></span></div></body>");
    assert_eq!(
        eval(
            &e.main,
            "(() => { const a = document.getElementById('a'), b = document.getElementById('b'); const seen = []; \
               const outer = new Event('o', { bubbles: true }), inner = new Event('i'); \
               b.addEventListener('o', () => { seen.push(window.event === outer); a.dispatchEvent(inner); seen.push(window.event === outer); }); \
               a.addEventListener('i', () => { seen.push(window.event === inner); throw new Error('boom'); }); \
               a.addEventListener('i', () => seen.push(window.event === inner)); \
               a.addEventListener('o', () => seen.push(window.event === outer)); \
               seen.push(window.event === undefined); b.dispatchEvent(outer); seen.push(window.event === undefined); \
               return seen.join(); })()"
        ),
        "true,true,true,true,true,true,true"
    );
    // The listener's exception was reported while its event was current.
    assert_eq!(e.hooks.exceptions.borrow().len(), 1);
}

#[test]
fn listener_slots_follow_adds_removes_once_handlers_and_reuse() {
    let e = env("<body><div id=a></div></body>");
    assert_eq!(
        eval(
            &e.main,
            "(() => { const a = document.getElementById('a'); const log = []; \
               const f = () => log.push('f'), g = () => log.push('g'), h = { handleEvent() { log.push('h:' + (this === h)); } }; \
               const fire = n => { a.dispatchEvent(new Event('t')); log.push('|' + n); }; \
               a.addEventListener('t', f); a.addEventListener('t', f); a.addEventListener('t', g); fire(1); \
               a.removeEventListener('t', f); fire(2); \
               a.addEventListener('t', h); a.addEventListener('t', f, { once: true }); fire(3); fire(4); \
               a.ontest = null; a.addEventListener('t', () => { a.removeEventListener('t', g); log.push('x'); }, { capture: true }); fire(5); \
               a.onclick = ev => log.push('on:' + ev.type); a.dispatchEvent(new Event('click')); \
               a.onclick = null; a.dispatchEvent(new Event('click')); a.onclick = () => log.push('on2'); a.dispatchEvent(new Event('click')); \
               return log.join(); })()"
        ),
        "f,g,|1,g,|2,g,h:true,f,|3,g,h:true,|4,x,h:true,|5,on:click,on2"
    );
}

#[test]
fn an_event_dispatched_again_elsewhere_reports_its_new_target() {
    let e = env("<body><div id=a></div><div id=b></div></body>");
    assert_eq!(
        eval(
            &e.main,
            "(() => { const a = document.getElementById('a'), b = document.getElementById('b'); a.tag = 'A'; b.tag = 'B'; \
               const ev = new Event('t', { bubbles: true }); const seen = []; \
               document.body.addEventListener('t', x => seen.push(x.target.tag + x.currentTarget.tagName)); \
               a.dispatchEvent(ev); seen.push(ev.target.tag); b.dispatchEvent(ev); seen.push(ev.target.tag); \
               a.dispatchEvent(ev); seen.push(ev.target.tag, ev.eventPhase, ev.currentTarget); return seen.join(); })()"
        ),
        "ABODY,A,BBODY,B,ABODY,A,0,"
    );
}
