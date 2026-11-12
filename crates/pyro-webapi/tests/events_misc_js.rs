//! Event interfaces and listener rules of DOM and HTML on the real
//! JavaScriptCore: `isTrusted` as a [LegacyUnforgeable] own accessor,
//! `addEventListener` options (conversion order, feature detection through a
//! null callback, a non-nullable `signal`), `CustomEvent.initCustomEvent`,
//! `AnimationEvent`/`TransitionEvent`, the legacy `onwebkit*` handlers and the
//! legacy types of trusted events, `AbortSignal.onabort`, `window.event` for
//! targets in shadow trees, `window.constructor`, and `beforeunload`.

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
    #[allow(dead_code)]
    hooks: Rc<Recorder>,
    #[allow(dead_code)]
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

#[test]
fn is_trusted_is_an_own_unforgeable_accessor_with_one_getter() {
    let e = env("<body></body>");
    assert_eq!(
        eval(
            &e.main,
            "(() => { const a = Object.getOwnPropertyDescriptor(new Event('x'), 'isTrusted'), \
               b = Object.getOwnPropertyDescriptor(new MouseEvent('x'), 'isTrusted'); \
             addEventListener('load', ev => { globalThis.own = [Object.getOwnPropertyDescriptor(ev, 'isTrusted').get === a.get, ev.isTrusted].join(); }); \
             return [typeof a.get, a.set, a.get === b.get, a.enumerable, a.configurable, 'isTrusted' in Event.prototype, \
                     new Event('x').isTrusted, a.get.call(new CustomEvent('y')), \
                     (() => { try { a.get.call({}); } catch (err) { return err.constructor.name; } })()].join(); })()"
        ),
        "function,,true,true,false,false,false,false,TypeError"
    );
    // A trusted event the engine wraps has the same own accessor.
    lifecycle::load(&e.page, e.page.document());
    assert_eq!(eval(&e.main, "globalThis.own"), "true,true");
}

#[test]
fn listener_options_convert_in_dictionary_order_even_for_a_null_callback() {
    let e = env("<body></body>");
    assert_eq!(
        eval(
            &e.main,
            "(() => { const read = []; const options = () => ({ get signal() { read.push('signal'); return undefined; }, \
                 get passive() { read.push('passive'); return false; }, get once() { read.push('once'); return false; }, \
                 get capture() { read.push('capture'); return true; }, get dummy() { read.push('dummy'); return 1; } }); \
               const target = new EventTarget(); \
               target.addEventListener('x', null, options()); read.push('|'); \
               target.removeEventListener('x', null, options()); read.push('|'); \
               target.addEventListener('x', () => {}, options()); \
               return read.join(' '); })()"
        ),
        "capture once passive signal | capture | capture once passive signal"
    );
    // `signal` is not nullable, and is checked whatever the callback is.
    assert_eq!(
        eval(
            &e.main,
            "[null, 1, {}, 'x'].map(signal => { try { new EventTarget().addEventListener('x', null, { signal }); return 'ok'; } \
               catch (err) { return err.constructor.name; } }).join() + '|' + \
             (() => { const c = new AbortController(), t = new EventTarget(); let n = 0; \
               t.addEventListener('x', () => n++, { signal: c.signal }); t.addEventListener('x', () => n++, { signal: undefined }); \
               t.dispatchEvent(new Event('x')); c.abort(); t.dispatchEvent(new Event('x')); return n; })()"
        ),
        "TypeError,TypeError,TypeError,TypeError|3"
    );
}

#[test]
fn init_custom_event_defaults_to_null_and_does_nothing_while_dispatching() {
    let e = env("<body></body>");
    assert_eq!(
        eval(
            &e.main,
            "(() => { const ev = new CustomEvent('a', { detail: 7, bubbles: true }); ev.initCustomEvent('b'); \
               const out = [ev.type, ev.bubbles, ev.cancelable, ev.detail]; \
               const during = new CustomEvent('c'); const t = new EventTarget(); \
               t.addEventListener('c', () => during.initCustomEvent('d', true, true, 1)); t.dispatchEvent(during); \
               out.push(during.type, during.bubbles, during.detail); \
               during.initCustomEvent('d', true, true, 1); out.push(during.type, during.detail); return out.join(); })()"
        ),
        "b,false,false,,c,false,,d,1"
    );
}

#[test]
fn animation_and_transition_events_and_their_legacy_handlers() {
    let e = env("<body><div id=d></div></body>");
    let r = &e.main;
    assert_eq!(
        eval(
            r,
            "(() => { const a = new AnimationEvent('animationend', { animationName: 'spin', elapsedTime: 1.5, pseudoElement: '::before', bubbles: true }); \
               const t = new TransitionEvent('transitionend', { propertyName: 'width', elapsedTime: 2 }); \
               const none = new AnimationEvent('x'); \
               return [a instanceof Event, a.animationName, a.elapsedTime, a.pseudoElement, a.bubbles, t.propertyName, t.elapsedTime, \
                       t.pseudoElement === '', none.animationName === '', none.elapsedTime, AnimationEvent.length, \
                       Object.getPrototypeOf(TransitionEvent.prototype) === Event.prototype].join(); })()"
        ),
        "true,spin,1.5,::before,true,width,2,true,true,0,1,true"
    );
    // `onwebkitanimationend` is a handler of its own, for the mixed-case type.
    eval(
        r,
        "globalThis.log = []; d.onwebkitanimationend = ev => log.push('handler:' + ev.type); \
         d.addEventListener('webkitAnimationEnd', ev => log.push('listener:' + ev.type)); \
         d.addEventListener('webkitanimationend', () => log.push('lowercase')); \
         addEventListener('animationend', ev => log.push('window:' + ev.type));",
    );
    assert_eq!(
        eval(
            r,
            "[d.onanimationend, typeof d.onwebkitanimationend].join() + '|' + \
             (d.dispatchEvent(new AnimationEvent('webkitAnimationEnd')), d.dispatchEvent(new AnimationEvent('animationend', { bubbles: true })), log.join())"
        ),
        ",function|handler:webkitAnimationEnd,listener:webkitAnimationEnd,window:animationend"
    );
    // A trusted `animationend` reaches the legacy handler and listener of a
    // target without an unprefixed one, under the legacy type.
    eval(r, "log.length = 0");
    let d = e.page.dom.borrow().get_element_by_id(e.page.document(), "d").unwrap().unwrap();
    let ev = EventCore::trusted(&e.page, EventInterface::AnimationEvent, "animationend", true, false);
    target::fire_at_node(&e.page, d, &ev);
    assert_eq!(eval(r, "log.join()"), "handler:webkitAnimationEnd,listener:webkitAnimationEnd,window:animationend");
    // With an unprefixed handler the legacy ones stay out.
    eval(r, "log.length = 0; d.onanimationend = ev => log.push('unprefixed:' + ev.type)");
    let ev = EventCore::trusted(&e.page, EventInterface::AnimationEvent, "animationend", true, false);
    target::fire_at_node(&e.page, d, &ev);
    assert_eq!(eval(r, "log.join()"), "unprefixed:animationend,window:animationend");
}

#[test]
fn a_handler_takes_the_slot_of_a_removed_listener() {
    let e = env("<body></body>");
    let r = &e.main;
    eval(
        r,
        "globalThis.log = []; const f = () => log.push('listener'); addEventListener('ping', f); removeEventListener('ping', f); \
         window.onresize = () => log.push('handler');",
    );
    // The safe point releases the removed listener's slot, not the handler in it.
    e.page.after_task();
    assert_eq!(eval(r, "dispatchEvent(new Event('resize')); dispatchEvent(new Event('ping')); log.join()"), "handler");
}

#[test]
fn abort_signal_has_an_onabort_handler() {
    let e = env("<body></body>");
    assert_eq!(
        eval(
            &e.main,
            "(() => { const c = new AbortController(), log = []; const before = c.signal.onabort; \
               c.signal.addEventListener('abort', () => log.push('listener')); \
               c.signal.onabort = ev => log.push('handler:' + (ev.target === c.signal) + ':' + ev.isTrusted); \
               c.abort(); c.abort(); \
               return [before, 'onabort' in AbortSignal.prototype, log.join('+')].join(); })()"
        ),
        ",true,listener+handler:true:true"
    );
}

#[test]
fn window_event_follows_the_shadow_tree_the_path_was_built_in() {
    let e = env("<body></body>");
    assert_eq!(
        eval(
            &e.main,
            "(() => { const host = document.createElement('div'), shadow = host.attachShadow({ mode: 'open' }), \
                 child = shadow.appendChild(document.createElement('a')), inner = child.appendChild(document.createElement('b')); \
               const seen = []; const ev = new Event('hi', { composed: true, bubbles: true }); \
               inner.addEventListener('hi', () => { host.appendChild(child); seen.push(window.event === undefined); }); \
               child.addEventListener('hi', () => seen.push(window.event === undefined)); \
               host.addEventListener('hi', () => seen.push(window.event === ev)); \
               inner.dispatchEvent(ev); seen.push(window.event === undefined); \
               const plain = new EventTarget(); let during; plain.addEventListener('t', () => { during = window.event; }); \
               const t = new Event('t'); plain.dispatchEvent(t); seen.push(during === t); \
               return seen.join(); })()"
        ),
        "true,true,true,true,true"
    );
}

#[test]
fn the_global_object_reports_its_interface_as_constructor() {
    let e = env("<body></body>");
    assert_eq!(
        eval(
            &e.main,
            "[window.constructor === Window, window.constructor.name, Object.keys(window).includes('constructor'), \
              window instanceof Window].join()"
        ),
        "true,Window,false,true"
    );
}

#[test]
fn beforeunload_asks_through_the_handler_return_value_and_return_value() {
    let e = env("<body></body>");
    let r = &e.main;
    // No listener: nothing to ask.
    assert!(!lifecycle::before_unload(&e.page));
    eval(
        r,
        "globalThis.seen = []; window.onbeforeunload = ev => { seen.push(ev.constructor.name, ev.type, ev.cancelable, ev.isTrusted); \
           return { toString() { seen.push(window.event === ev); return 'leave?'; } }; }; \
         addEventListener('beforeunload', ev => seen.push(ev.returnValue, ev.defaultPrevented));",
    );
    assert!(lifecycle::before_unload(&e.page));
    assert_eq!(eval(r, "seen.join()"), "BeforeUnloadEvent,beforeunload,true,true,true,leave?,true");
    // null and undefined ask for nothing.
    eval(r, "seen.length = 0; window.onbeforeunload = () => null;");
    assert!(!lifecycle::before_unload(&e.page));
    assert_eq!(eval(r, "seen.join()"), ",false");
}
