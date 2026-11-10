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

#[test]
fn computed_style_longhands_are_prototype_attributes() {
    let e = env("<body><div id=a style='float: left; background-color: rgb(1, 2, 3)'></div></body>");
    assert_eq!(
        eval(
            &e.main,
            "(() => { const a = document.getElementById('a'), cs = getComputedStyle(a), proto = Object.getPrototypeOf(cs); \
               const d = Object.getOwnPropertyDescriptor(proto, 'backgroundColor'); \
               return [cs.display, cs['background-color'], cs.backgroundColor, cs.cssFloat, cs.float, cs.getPropertyValue('float'), \
                       typeof d.get, typeof d.set, 'display' in cs, cs.hasOwnProperty('display'), Object.getOwnPropertyDescriptor(proto, 'display') !== undefined, \
                       cs.length > 200, cs.item(0) === cs[0], getComputedStyle(document.createElement('p')).display === '', \
                       cs.display === getComputedStyle(a).display, \
                       typeof cs.margin, typeof cs.webkitLineClamp, typeof cs.WebkitTextSecurity, cs.fooBar === undefined, cs['--x'] === undefined].join(); })()"
        ),
        // `hasOwnProperty('display')` is true as in Chrome: the patched JSC
        // answers the names as own properties (`ClassDef::own_property_name`)
        // while the values stay with the prototype's attributes.
        "block,rgb(1, 2, 3),rgb(1, 2, 3),left,left,left,function,function,true,true,true,true,true,true,true,string,string,string,true,true"
    );
}

#[test]
fn id_and_class_name_follow_attribute_changes() {
    let e = env("<body><div id=a class=one></div></body>");
    assert_eq!(
        eval(
            &e.main,
            "(() => { const a = document.getElementById('a'); const seen = [a.id, a.id, a.id, a.className, a.className]; \
               a.id = 'b'; seen.push(a.id, a.id); a.setAttribute('id', 'c'); seen.push(a.id); a.removeAttribute('id'); seen.push(a.id === ''); \
               a.className = 'one two'; seen.push(a.className, a.className); a.className = 'two'; seen.push(a.className, a.className); \
               a.id = 'b'; seen.push(a.id); return seen.join(); })()"
        ),
        "a,a,a,one,one,b,b,c,true,one two,one two,two,two,b"
    );
}

#[test]
fn finalized_wrappers_are_given_back_inside_a_long_task() {
    let e = env("<body></body>");
    // One task that makes and drops wrappers by the hundred thousand: the
    // collector finalizes most of them while it runs, and the post-callback
    // hook of the outermost callback drains their payloads and frees their
    // nodes, so the DOM does not hold them all until the task ends.
    let made = 400_000usize;
    assert_eq!(
        eval(&e.main, &format!("(() => {{ let s = 0; for (let i = 0; i < {made}; i++) s += document.createElement('div').nodeType; return s; }})()")),
        (made as f64).to_string().trim_end_matches(".0").to_owned()
    );
    // How much the collector finalizes before the task ends depends on its
    // heuristics and the machine (lazy sweeping, heap growth): 76% live on an
    // idle machine, 57-74% under load. What the drain guarantees is that the
    // payloads finalized so far are given back mid-task: at least one drain
    // of 4096 (`FINALIZER_DRAIN`) ran.
    let live = e.page.dom.borrow().live_nodes();
    assert!(live <= made - 4096, "{live} nodes live after the task of {made} created (none drained before its end)");
    // The task's end releases the rest.
    e.engine.drain_finalized();
    e.page.after_task();
}
