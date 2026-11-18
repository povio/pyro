//! HTML "report an exception" for a Window on the real JavaScriptCore: an
//! uncaught exception of a platform-invoked callback fires `error` at the
//! Window (`lifecycle::report_exception` as the realm's exception reporter),
//! `window.onerror` takes the five-argument OnErrorEventHandler form and
//! cancels by returning true, a canceled event is not reported to the
//! engine hooks, and an exception inside the error listener fires no second
//! event.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::Dom;
use pyro_js::{ClassDef, ConsoleLevel, Engine, EngineHooks, JsClass, JsException, Realm, RealmOptions, Value};
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

/// What the engine hooks (console/CDP in the product) were told.
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
    #[allow(dead_code)]
    engine: Engine,
    page: Rc<PageDom>,
    main: Realm,
    hooks: Rc<Recorder>,
    window_class: pyro_js::ClassId,
}

/// Loads no sub-resources.
struct NoFetch;
impl pyro_render::Fetcher for NoFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err(format!("{url}: offline")));
    }
}

const URL: &str = "http://localhost:5173/app";

fn env(html: &str) -> Env {
    pyro_layout::install(std::sync::Arc::new(NoFetch));
    let engine = Engine::new().expect("engine");
    let hooks = Rc::new(Recorder::default());
    engine.set_hooks(Some(hooks.clone()));
    register(&engine);
    let window_class = engine.register::<TestWindow>();
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom, URL, html);
    dom.borrow_mut().set_document_active(doc, true).unwrap();
    let page = PageDom::new(dom, doc);
    let main = realm(&engine, &page, window_class, "");
    Env { engine, page, main, hooks, window_class }
}

/// A realm over the page's Window; a named one is an isolated world.
fn realm(engine: &Engine, page: &Rc<PageDom>, window_class: pyro_js::ClassId, name: &str) -> Realm {
    let realm = engine
        .create_realm(RealmOptions { name: name.to_owned(), global_class: Some(window_class), origin: None })
        .expect("realm");
    let key = page.window_target().unwrap_or_else(|| page.create_window_target(page.document()));
    realm
        .set_global_data::<TestWindow>(TestWindowData {
            page: page.clone(),
            target: EventTargetData::window(page, key),
        })
        .unwrap();
    install(&realm.scope().unwrap(), page.clone()).unwrap();
    realm.set_exception_reporter(Some(lifecycle::report_exception));
    realm
}

fn eval(realm: &Realm, src: &str) -> String {
    let s = realm.scope().unwrap();
    match s.eval(src, Some("test.js"), 1) {
        Ok(v) => v.to_string().unwrap(),
        Err(e) => format!("THROW {}", e.details(&s).text()),
    }
}

impl Env {
    fn exceptions(&self) -> Vec<String> {
        self.hooks.exceptions.borrow().clone()
    }
}

/// HTML "report the error": every `error` listener runs before the
/// microtasks they queue (html/semantics/scripting-1/the-script-element/
/// microtasks/checkpoint-after-window-onerror.html).
#[test]
fn error_listeners_run_before_their_microtasks() {
    let e = env("<body><div id=d></div></body>");
    let r = &e.main;
    eval(
        r,
        "globalThis.log = []; \
         addEventListener('error', ev => { ev.preventDefault(); log.push('h1'); Promise.resolve().then(() => log.push('p1')); }); \
         addEventListener('error', () => { log.push('h2'); Promise.resolve().then(() => log.push('p2')); }); \
         d.addEventListener('click', () => { throw new Error('x'); });",
    );
    // A listener invoked from outside script (no JavaScript on the stack) throws.
    let d = e.page.dom.borrow().get_element_by_id(e.page.document(), "d").unwrap().unwrap();
    let ev = pyro_webapi::event::EventCore::trusted(&e.page, pyro_webapi::event::EventInterface::Event, "click", true, false);
    pyro_webapi::target::fire_at_node(&e.page, d, &ev);
    assert_eq!(eval(r, "log.join()"), "h1,h2,p1,p2");
}

#[test]
fn listener_exception_fires_error_event_at_window_then_reports() {
    let e = env("<body><div id=d></div></body>");
    let r = &e.main;
    eval(
        r,
        "globalThis.seen = []; globalThis.err = new Error('boom'); \
         addEventListener('error', ev => seen.push([ev.type, ev.constructor.name, ev.isTrusted, ev.bubbles, ev.cancelable, \
           ev.target === window, ev.message, ev.filename, ev.lineno > 0, ev.colno > 0, ev.error === err].join())); \
         d.addEventListener('click', () => { throw err; }); \
         globalThis.after = 'before';\
         d.addEventListener('click', () => { after = 'second listener ran'; });",
    );
    assert_eq!(
        eval(r, "d.dispatchEvent(new Event('click')); seen.join('|')"),
        "error,ErrorEvent,true,false,true,true,Uncaught Error: boom,test.js,true,true,true"
    );
    // The event fired before the next listener of the throwing dispatch ran,
    // and the exception, not canceled, reached the engine hooks once.
    assert_eq!(eval(r, "after"), "second listener ran");
    assert_eq!(e.exceptions(), ["Error: boom"]);
}

#[test]
fn window_onerror_takes_five_arguments_and_cancels_with_true() {
    let e = env("<body><div id=d></div></body>");
    let r = &e.main;
    eval(
        r,
        "globalThis.calls = []; globalThis.err = new TypeError('bad'); \
         window.onerror = function (message, source, lineno, colno, error) { \
           calls.push([arguments.length, this === window, message, source, typeof lineno, lineno > 0, typeof colno, colno > 0, error === err].join()); return true; }; \
         d.addEventListener('click', () => { throw err; });",
    );
    assert_eq!(
        eval(r, "d.dispatchEvent(new Event('click')); calls.join('|')"),
        "5,true,Uncaught TypeError: bad,test.js,number,true,number,true,true"
    );
    // Returning true canceled the event: nothing reached the engine hooks.
    assert_eq!(e.exceptions(), Vec::<String>::new());
    // Any other return value leaves the exception reported.
    eval(r, "window.onerror = () => false; d.dispatchEvent(new Event('click'));");
    assert_eq!(e.exceptions(), ["TypeError: bad"]);
    eval(r, "window.onerror = () => {}; d.dispatchEvent(new Event('click'));");
    assert_eq!(e.exceptions(), ["TypeError: bad", "TypeError: bad"]);
    // addEventListener listeners cancel with preventDefault.
    eval(r, "window.onerror = null; addEventListener('error', ev => ev.preventDefault()); d.dispatchEvent(new Event('click'));");
    assert_eq!(e.exceptions().len(), 2);
}

#[test]
fn body_onerror_is_the_window_handler_with_five_arguments() {
    let e = env("<body><div id=d></div></body>");
    let r = &e.main;
    eval(
        r,
        "globalThis.calls = []; \
         document.body.onerror = (m, f, l, c, err) => { calls.push([m, f, err.message].join()); return true; }; \
         d.addEventListener('click', () => { throw new RangeError('r'); });",
    );
    assert_eq!(eval(r, "window.onerror === document.body.onerror"), "true");
    assert_eq!(eval(r, "d.dispatchEvent(new Event('click')); calls.join('|')"), "Uncaught RangeError: r,test.js,r");
    assert_eq!(e.exceptions(), Vec::<String>::new());
}

#[test]
fn element_onerror_keeps_the_event_argument() {
    let e = env("<body><img id=i></body>");
    let r = &e.main;
    eval(
        r,
        "globalThis.calls = []; i.onerror = function (ev) { calls.push([arguments.length, ev instanceof Event, ev.type].join()); return true; };",
    );
    assert_eq!(eval(r, "i.dispatchEvent(new Event('error', {cancelable: true})); calls.join('|')"), "1,true,error");
}

#[test]
fn synthetic_error_events_at_the_window_use_the_special_handling() {
    let e = env("<body></body>");
    let r = &e.main;
    eval(
        r,
        "globalThis.calls = []; globalThis.ret = true; \
         window.onerror = (m, f, l, c, err) => { calls.push([m, f, l, c, err].join()); return ret; };",
    );
    let fire = "dispatchEvent(new ErrorEvent('error', {message: 'm', filename: 'f.js', lineno: 3, colno: 4, error: 'e', cancelable: true}))";
    // Returning true cancels; false does not (unlike other handlers).
    assert_eq!(eval(r, &format!("[{fire}, calls.join('|')].join(';')")), "false;m,f.js,3,4,e");
    assert_eq!(eval(r, &format!("ret = false; {fire}")), "true");
    assert_eq!(eval(r, &format!("ret = undefined; {fire}")), "true");
    // A plain Event named error is not special: the event is the argument.
    assert_eq!(
        eval(r, "calls = []; window.onerror = function (ev) { calls.push(arguments.length + ':' + ev.type) }; dispatchEvent(new Event('error')); calls.join()"),
        "1:error"
    );
}

#[test]
fn exception_in_error_listener_fires_no_second_event() {
    let e = env("<body><div id=d></div></body>");
    let r = &e.main;
    eval(
        r,
        "globalThis.count = 0; \
         addEventListener('error', () => { count++; throw new Error('inner'); }); \
         d.addEventListener('click', () => { throw new Error('outer'); });",
    );
    assert_eq!(eval(r, "d.dispatchEvent(new Event('click')); count"), "1");
    // Both exceptions are reported; the inner one without an event.
    assert_eq!(e.exceptions(), ["Error: inner", "Error: outer"]);
}

#[test]
fn thrown_primitives_and_native_errors_fill_the_event() {
    let e = env("<body><div id=d></div></body>");
    let r = &e.main;
    eval(
        r,
        "globalThis.seen = []; \
         addEventListener('error', ev => seen.push([ev.message, ev.filename, ev.lineno, ev.colno, typeof ev.error, ev.error].join())); \
         d.addEventListener('click', () => { throw 'plain'; }, {once: true});",
    );
    // No Error object: no location, so the document's URL and 0.
    assert_eq!(
        eval(r, "d.dispatchEvent(new Event('click')); seen.join('|')"),
        format!("Uncaught plain,{URL},0,0,string,plain")
    );
    assert_eq!(e.exceptions(), ["plain"]);
    // A DOMException thrown by a binding is an error object like any other.
    eval(r, "seen = []; d.addEventListener('click', () => { document.createElement('1bad'); }, {once: true}); d.dispatchEvent(new Event('click'))");
    let seen = eval(r, "seen.join('|')");
    assert!(seen.starts_with("Uncaught InvalidCharacterError: ") && seen.contains(",object,"), "{seen}");
    // `handleEvent` that is not callable is a TypeError of the platform.
    eval(r, "seen = []; d.addEventListener('click', {handleEvent: 1}); d.dispatchEvent(new Event('click'))");
    assert_eq!(
        eval(r, "seen.length + ':' + seen[0].split(',')[0]"),
        "1:Uncaught TypeError: The provided callback is not a function."
    );
}

#[test]
fn observer_callback_exception_fires_the_event() {
    let e = env("<body><div id=d></div></body>");
    let r = &e.main;
    eval(
        r,
        "globalThis.seen = []; addEventListener('error', ev => seen.push(ev.message)); \
         new MutationObserver(() => { throw new Error('observed'); }).observe(d, {attributes: true}); \
         d.setAttribute('x', '1');",
    );
    // The microtask checkpoint after the evaluation ran the callback.
    assert_eq!(eval(r, "seen.join('|')"), "Uncaught Error: observed");
    assert_eq!(e.exceptions(), ["Error: observed"]);
}

#[test]
fn isolated_world_errors_stay_in_their_world() {
    let e = env("<body><div id=d></div></body>");
    let isolated = realm(&e.engine, &e.page, e.window_class, "automation");
    eval(&e.main, "globalThis.mainSeen = []; addEventListener('error', ev => mainSeen.push(ev.message)); window.onerror = () => { mainSeen.push('onerror'); return true; };");
    eval(&isolated, "globalThis.isoSeen = []; addEventListener('error', ev => isoSeen.push(ev.message)); d.addEventListener('click', () => { throw new Error('iso'); });");
    eval(&isolated, "d.dispatchEvent(new Event('click'))");
    assert_eq!(eval(&isolated, "isoSeen.join('|')"), "Uncaught Error: iso");
    assert_eq!(eval(&e.main, "mainSeen.join('|')"), "");
    // The page's onerror did not cancel it: the exception is reported.
    assert_eq!(e.exceptions(), ["Error: iso"]);
    // The main world's errors are the page's alone: as in Chrome, a content
    // script's `error` listener never sees them. The click runs the isolated
    // world's listener again too, whose error again stays in its world.
    eval(
        &e.main,
        "d.addEventListener('click', () => { throw new Error('main'); }); d.dispatchEvent(new Event('click'))",
    );
    assert_eq!(eval(&e.main, "mainSeen.join('|')"), "Uncaught Error: main|onerror");
    assert_eq!(eval(&isolated, "isoSeen.join('|')"), "Uncaught Error: iso|Uncaught Error: iso");
    // The page's onerror canceled its own error; the isolated world's are reported.
    assert_eq!(e.exceptions(), ["Error: iso", "Error: iso"]);
    // A synthetic ErrorEvent belongs to no world: both worlds' listeners run.
    eval(&e.main, "mainSeen = []; dispatchEvent(new ErrorEvent('error', {message: 'synthetic'}))");
    assert_eq!(eval(&e.main, "mainSeen.join('|')"), "synthetic|onerror");
    assert_eq!(eval(&isolated, "isoSeen.join('|')"), "Uncaught Error: iso|Uncaught Error: iso|synthetic");
}

/// Queues the page's tasks for the test to run.
#[derive(Default)]
struct Tasks(RefCell<std::collections::VecDeque<Box<dyn FnOnce()>>>);
impl PageHooks for Tasks {
    fn post_task(&self, task: Box<dyn FnOnce()>) {
        self.0.borrow_mut().push_back(task);
    }
    fn navigate(&self, _: NavigationRequest) {}
}
impl Tasks {
    fn drain(&self) -> usize {
        let mut ran = 0;
        loop {
            let task = self.0.borrow_mut().pop_front();
            let Some(task) = task else { return ran };
            task();
            ran += 1;
            assert!(ran < 100, "task loop did not settle");
        }
    }
}

#[test]
fn report_under_a_dom_borrow_is_replayed_from_a_task() {
    let e = env("<body></body>");
    let r = &e.main;
    eval(r, "globalThis.seen = []; addEventListener('error', ev => seen.push(ev.message + '@' + ev.filename)); window.onerror = m => m.includes('cancel');");
    let s = r.scope().unwrap();
    // Without an embedder to post the task, a report made while the DOM is
    // borrowed skips the event and goes to the engine hooks at once.
    {
        let _held = e.page.dom.borrow();
        s.report_exception(&pyro_js::JsError::Type("no tasks".into()));
    }
    assert_eq!(eval(r, "seen.join('|')"), "");
    assert_eq!(e.exceptions(), ["TypeError: no tasks"]);
    // With one, the event (and the report it may lead to) waits for a task
    // of its own; the thrown value is kept across.
    let tasks = Rc::new(Tasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    let thrown = s.eval("new RangeError('deferred cancel')", Some("thrower.js"), 7).unwrap();
    {
        let _held = e.page.dom.borrow_mut();
        s.report_exception(&pyro_js::JsError::thrown(pyro_js::Persistent::new(&s, thrown)));
        s.report_exception(&pyro_js::JsError::Type("deferred reported".into()));
    }
    assert_eq!(eval(r, "seen.join('|')"), "");
    assert_eq!(e.exceptions(), ["TypeError: no tasks"]);
    assert_eq!(tasks.drain(), 2);
    // A binding error made outside any script has no location: the document's URL.
    assert_eq!(
        eval(r, "seen.join('|')"),
        format!("Uncaught RangeError: deferred cancel@thrower.js|Uncaught TypeError: deferred reported@{URL}")
    );
    // `onerror` canceled the first replayed report only.
    assert_eq!(e.exceptions(), ["TypeError: no tasks", "TypeError: deferred reported"]);
}

#[test]
fn disposed_page_and_scripting_disabled_fire_nothing() {
    let e = env("<body><div id=d></div></body>");
    let r = &e.main;
    eval(r, "globalThis.seen = 0; addEventListener('error', () => seen++); d.addEventListener('click', () => { throw new Error('x'); });");
    e.page.set_scripting_disabled(true);
    // Listeners do not run at all without scripting; the reporter is not reached.
    assert_eq!(eval(r, "d.dispatchEvent(new Event('click')); seen"), "0");
    e.page.set_scripting_disabled(false);
    assert_eq!(eval(r, "d.dispatchEvent(new Event('click')); seen"), "1");
    assert_eq!(e.exceptions(), ["Error: x"]);
    // A direct report on a disposed page goes to the hooks only.
    let s = r.scope().unwrap();
    e.page.dispose();
    s.report_exception(&pyro_js::JsError::Type("late".into()));
    assert_eq!(e.exceptions(), ["Error: x", "TypeError: late"]);
}
