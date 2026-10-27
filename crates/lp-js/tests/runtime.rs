//! Integration tests against the real system JavaScriptCore.

use lp_js::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

fn engine() -> Engine {
    Engine::new().expect("engine")
}

fn eval_str(realm: &Realm, src: &str) -> String {
    let s = realm.scope().unwrap();
    match s.eval(src, Some("test.js"), 1) {
        Ok(v) => v.to_string().unwrap(),
        Err(e) => format!("THROW {}", e.details(&s).text()),
    }
}

// -- a small class hierarchy resembling DOM bindings -------------------------

struct NodeData {
    id: u64,
    name: RefCell<String>,
    children: RefCell<Vec<u64>>,
    drops: Rc<Cell<u32>>,
}

impl Drop for NodeData {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

type NodeRef = Rc<NodeData>;

struct EventTarget;
impl JsClass for EventTarget {
    const NAME: &'static str = "EventTarget";
    type Data = NodeRef;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("ping", 0, |s, _this, _args| Ok(s.string("pong")));
    }
}

struct Node;
impl JsClass for Node {
    const NAME: &'static str = "Node";
    type Data = NodeRef;
    type Parent = EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.constant("ELEMENT_NODE", 1.0);
        def.getter("nodeId", |s, this| Ok(s.number(this.id as f64)));
        def.accessor(
            "nodeName",
            |s, this| Ok(s.string(&this.name.borrow())),
            |_s, this, v| {
                *this.name.borrow_mut() = v.to_string()?;
                Ok(())
            },
        );
        def.method("fail", 0, |_s, _this, _args| Err(JsError::dom("NotFoundError", "no such node")));
        def.method("panics", 0, |_s, _this, _args| panic!("boom"));
        def.method("childAt", 1, |s, this, args| {
            args.require(1, "childAt")?;
            let i = args.get(0).to_number()? as usize;
            match this.children.borrow().get(i) {
                Some(id) => Ok(s.number(*id as f64)),
                None => Err(JsError::Range("out of range".into())),
            }
        });
    }
}

struct Element;
impl JsClass for Element {
    const NAME: &'static str = "Element";
    type Data = NodeRef;
    type Parent = Node;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, new_target, args| {
            let name = args.get(0).to_string()?;
            let data = Rc::new(NodeData {
                id: 1000,
                name: RefCell::new(name),
                children: RefCell::new(vec![]),
                drops: Rc::new(Cell::new(0)),
            });
            s.wrap_new::<Element>(data, new_target, None)
        });
    }
}

struct List;
impl JsClass for List {
    const NAME: &'static str = "List";
    type Data = NodeRef;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("length", |s, this| Ok(s.number(this.children.borrow().len() as f64)));
        def.method("item", 1, |s, this, args| {
            let i = args.get(0).to_number()? as usize;
            Ok(this.children.borrow().get(i).map(|v| s.number(*v as f64)).unwrap_or_else(|| s.null()))
        });
        def.indexed(|s, this, i| Ok(this.children.borrow().get(i as usize).map(|v| s.number(*v as f64))));
        def.named(false, |s, this, name| Ok(if name == "special" { Some(s.number(this.id as f64)) } else { None }));
        def.indexed_iterable();
    }
}

fn node(id: u64, drops: &Rc<Cell<u32>>) -> NodeRef {
    Rc::new(NodeData {
        id,
        name: RefCell::new(format!("n{id}")),
        children: RefCell::new(vec![10, 20, 30]),
        drops: drops.clone(),
    })
}

fn setup() -> (Engine, Realm) {
    let e = engine();
    e.register::<Element>();
    e.register::<List>();
    let realm = e.create_realm(RealmOptions::default()).unwrap();
    (e, realm)
}

fn expose<C: JsClass>(realm: &Realm, name: &str, data: C::Data, key: Option<IdentityKey>) {
    let s = realm.scope().unwrap();
    let o = s.wrap::<C>(data, key).unwrap();
    s.global().set(name, o.value()).unwrap();
}

#[test]
fn evaluates_and_reports_syntax_errors() {
    let e = engine();
    let r = e.create_realm(RealmOptions::default()).unwrap();
    assert_eq!(eval_str(&r, "1 + 2"), "3");
    assert_eq!(eval_str(&r, "throw new TypeError('x')"), "THROW TypeError: x");
    let s = r.scope().unwrap();
    let err = s.eval("let = ;", Some("bad.js"), 1).unwrap_err();
    let d = err.details(&s);
    assert_eq!(d.name.as_deref(), Some("SyntaxError"));
    assert_eq!(d.line, Some(1));
}

#[test]
fn only_one_engine_per_thread() {
    let _e = engine();
    assert!(Engine::new().is_err());
}

#[test]
fn class_shape_matches_webidl_expectations() {
    let (_e, r) = setup();
    let drops = Rc::new(Cell::new(0));
    expose::<Node>(&r, "n", node(7, &drops), Some(IdentityKey(1, 7)));
    let checks = [
        ("n.nodeId", "7"),
        ("n.nodeName", "n7"),
        ("n.nodeName = 'x'; n.nodeName", "x"),
        ("n.ping()", "pong"),
        ("n instanceof Node && n instanceof EventTarget", "true"),
        ("Object.getPrototypeOf(Node.prototype) === EventTarget.prototype", "true"),
        ("Object.getPrototypeOf(Node) === EventTarget", "true"),
        ("typeof Object.getOwnPropertyDescriptor(Node.prototype, 'nodeName').set", "function"),
        ("Object.getOwnPropertyDescriptor(Node.prototype, 'nodeName').get.name", "get nodeName"),
        ("n.hasOwnProperty('nodeName')", "false"),
        ("Object.prototype.toString.call(n)", "[object Node]"),
        ("Node.ELEMENT_NODE + n.ELEMENT_NODE", "2"),
        ("typeof Node.prototype.ping", "function"),
        ("Node.prototype.ping.call(n)", "pong"),
        ("'nodeId' in n", "true"),
        ("Object.keys(Node.prototype).includes('nodeName')", "true"),
        ("Node.prototype.childAt.length", "1"),
    ];
    for (src, want) in checks {
        assert_eq!(eval_str(&r, src), want, "{src}");
    }
}

#[test]
fn brand_checks_and_errors() {
    let (_e, r) = setup();
    let drops = Rc::new(Cell::new(0));
    expose::<Node>(&r, "n", node(1, &drops), None);
    expose::<List>(&r, "l", node(2, &drops), None);
    let checks = [
        ("try { Node.prototype.ping.call({}) } catch (e) { e.constructor.name + ': ' + e.message }", "TypeError: Illegal invocation"),
        // Member functions are native objects too, but not wrappers.
        ("try { Node.prototype.ping.call(Node.prototype.ping) } catch (e) { e.message }", "Illegal invocation"),
        ("try { Node.prototype.ping.call(Object.getOwnPropertyDescriptor(Node.prototype, 'nodeId').get) } catch (e) { e.message }", "Illegal invocation"),
        ("try { Object.getOwnPropertyDescriptor(Node.prototype, 'nodeId').get.call(l) } catch (e) { e.message }", "Illegal invocation"),
        ("try { n.fail() } catch (e) { [e instanceof DOMException, e instanceof Error, e.name, e.code, e.message].join() }", "true,true,NotFoundError,8,no such node"),
        ("try { n.childAt() } catch (e) { e.constructor.name }", "TypeError"),
        ("try { n.childAt(9) } catch (e) { e.constructor.name }", "RangeError"),
        ("try { n.panics() } catch (e) { e.message }", "Internal error: boom"),
        ("n.childAt(1)", "20"),
        ("try { new Node() } catch (e) { e.message }", "Illegal constructor"),
        ("try { Element('x') } catch (e) { e.constructor.name }", "TypeError"),
    ];
    for (src, want) in checks {
        assert_eq!(eval_str(&r, src), want, "{src}");
    }
}

#[test]
fn constructors_support_subclassing() {
    let (_e, r) = setup();
    let checks = [
        ("new Element('div').nodeName", "div"),
        ("new Element('a') instanceof Node", "true"),
        ("class Fancy extends Element { constructor() { super('fancy'); this.extra = 1 } hi() { return this.nodeName + this.extra } }; new Fancy().hi()", "fancy1"),
        ("new Fancy() instanceof Element && Object.getPrototypeOf(new Fancy()) === Fancy.prototype", "true"),
        ("Element.length", "1"),
    ];
    for (src, want) in checks {
        assert_eq!(eval_str(&r, src), want, "{src}");
    }
}

#[test]
fn indexed_and_named_properties() {
    let (_e, r) = setup();
    let drops = Rc::new(Cell::new(0));
    expose::<List>(&r, "l", node(5, &drops), None);
    let checks = [
        ("l[0] + l[2]", "40"),
        ("l[3]", "undefined"),
        ("l.length", "3"),
        ("l.item(1)", "20"),
        ("l.special", "5"),
        ("l.item === List.prototype.item", "true"),
        ("[...l].join()", "10,20,30"),
        ("Array.from(l.entries()).length", "3"),
        ("Object.keys(l).join()", "0,1,2"),
        ("l.expando = 4; l.expando", "4"),
        ("l[0] = 99; l[0]", "10"),
    ];
    for (src, want) in checks {
        assert_eq!(eval_str(&r, src), want, "{src}");
    }
}

#[test]
fn identity_is_stable_and_payloads_are_released_after_gc() {
    let (e, r) = setup();
    let drops = Rc::new(Cell::new(0));
    {
        let s = r.scope().unwrap();
        let a = s.wrap::<Node>(node(1, &drops), Some(IdentityKey(9, 1))).unwrap();
        let b = s.wrap::<Node>(node(1, &drops), Some(IdentityKey(9, 1))).unwrap();
        assert!(a.value().strict_equals(b.value()));
        // The second payload was dropped immediately since the wrapper existed.
        assert_eq!(drops.get(), 1);
    }
    // Create unreachable wrappers inside a helper so no stack slot keeps them.
    #[inline(never)]
    fn make_garbage(r: &Realm, drops: &Rc<Cell<u32>>) {
        let s = r.scope().unwrap();
        for i in 0..200 {
            s.wrap::<Node>(node(100 + i, drops), Some(IdentityKey(7, i))).unwrap();
        }
    }
    make_garbage(&r, &drops);
    for _ in 0..5 {
        e.collect_garbage_now();
        e.drain_finalized();
    }
    assert!(drops.get() > 150, "expected most of 200 payloads released, got {}", drops.get());
}

#[test]
fn disposed_realm_throws_context_destroyed() {
    let (e, r) = setup();
    let other = e.create_realm(RealmOptions { name: "other".into(), ..Default::default() }).unwrap();
    let drops = Rc::new(Cell::new(0));
    expose::<Node>(&r, "n", node(3, &drops), None);
    // Leak a function from the first realm into the second.
    {
        let s = r.scope().unwrap();
        let f = s.eval("(() => n.ping())", None, 1).unwrap();
        let direct = s.eval("Node.prototype.ping.bind(n)", None, 1).unwrap();
        let so = other.scope().unwrap();
        so.global().set("leaked", f).unwrap();
        so.global().set("leakedNative", direct).unwrap();
    }
    assert_eq!(eval_str(&other, "leakedNative()"), "pong");
    r.dispose();
    assert!(r.scope().is_err());
    assert_eq!(eval_str(&other, "try { leakedNative() } catch (e) { e.message }"), "Execution context was destroyed.");
}

#[test]
fn console_and_exception_hooks() {
    #[derive(Default)]
    struct Hooks {
        log: RefCell<Vec<String>>,
        rejections: RefCell<Vec<String>>,
    }
    impl EngineHooks for Hooks {
        fn console_api(&self, _realm: &Realm, level: ConsoleLevel, args: &[Value<'_>]) {
            let parts: Vec<String> = args.iter().map(|a| a.to_string().unwrap_or_default()).collect();
            self.log.borrow_mut().push(format!("{}:{}", level.cdp_type(), parts.join(" ")));
        }
        fn unhandled_rejection(&self, _realm: &Realm, exception: &JsException) {
            self.rejections.borrow_mut().push(exception.text());
        }
    }
    let e = engine();
    let hooks = Rc::new(Hooks::default());
    e.set_hooks(Some(hooks.clone()));
    let r = e.create_realm(RealmOptions::default()).unwrap();
    eval_str(
        &r,
        "console.log('a', 1, {}); console.warn('w'); console.assert(false, 'bad'); console.assert(true, 'ok')",
    );
    assert_eq!(*hooks.log.borrow(), vec!["log:a 1 [object Object]", "warning:w", "assert:bad"]);
    if e.capabilities().unhandled_rejections {
        eval_str(&r, "Promise.reject(new Error('nope')); Promise.reject(1).catch(() => {});");
        assert_eq!(*hooks.rejections.borrow(), vec!["Error: nope"]);
    }
}

#[test]
fn native_closures_promises_and_microtasks() {
    let e = engine();
    let r = e.create_realm(RealmOptions::default()).unwrap();
    let seen = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let s = r.scope().unwrap();
        let seen2 = seen.clone();
        let f = s
            .function("record", move |s, _this, args| {
                seen2.borrow_mut().push(args.get(0).to_string()?);
                Ok(s.undefined())
            })
            .unwrap();
        s.global().set("record", f.value()).unwrap();
        // Microtasks run when the outermost API call returns.
        s.eval("record('sync'); Promise.resolve().then(() => record('micro')); record('sync2')", None, 1).unwrap();
        assert_eq!(*seen.borrow(), vec!["sync", "sync2", "micro"]);
        assert_eq!(s.eval("typeof record + record.name", None, 1).unwrap().to_string().unwrap(), "functionrecord");

        let d = s.deferred().unwrap();
        let seen3 = seen.clone();
        let seen4 = seen.clone();
        s.then(
            d.promise.get(&s),
            move |_s, v| seen3.borrow_mut().push(format!("ok:{}", v.to_string().unwrap())),
            move |_s, v| seen4.borrow_mut().push(format!("err:{}", v.to_string().unwrap())),
        )
        .unwrap();
        d.resolve(&s, s.number(42.0)).unwrap();
    }
    assert_eq!(seen.borrow().last().unwrap(), "ok:42");
}

#[test]
fn call_function_and_describe() {
    let e = engine();
    let r = e.create_realm(RealmOptions::default()).unwrap();
    let s = r.scope().unwrap();
    let v = s
        .call_function(
            "function (a, b) { return this.x + a + b }",
            Some(s.eval("({x: 1})", None, 1).unwrap()),
            &[s.number(2.0), s.number(3.0)],
        )
        .unwrap();
    assert_eq!(v.to_number().unwrap(), 6.0);
    let v = s.call_function("function () { return typeof this }", Some(s.number(1.0)), &[]).unwrap();
    assert_eq!(v.to_string().unwrap(), "object");
    let d = s.describe(s.eval("new Map()", None, 1).unwrap()).unwrap();
    assert_eq!((d.ty.as_str(), d.subtype.as_deref(), d.class_name.as_deref()), ("object", Some("map"), Some("Map")));
    let d = s.describe(s.eval("[1,2]", None, 1).unwrap()).unwrap();
    assert_eq!(d.description.as_deref(), Some("Array(2)"));
    let props = s
        .own_properties(s.eval("({a: 1, get b() { return 2 }})", None, 1).unwrap().as_object().unwrap(), true)
        .unwrap();
    assert_eq!(props.get("length").unwrap().to_number().unwrap(), 2.0);
}

#[test]
fn hidden_properties_on_wrappers() {
    let (_e, r) = setup();
    let s = r.scope().unwrap();
    let drops = Rc::new(Cell::new(0));
    let o = s.wrap::<Node>(node(1, &drops), None).unwrap();
    s.set_hidden(o, "listeners", s.string("L")).unwrap();
    assert_eq!(s.get_hidden(o, "listeners").unwrap().unwrap().to_string().unwrap(), "L");
    assert!(s.get_hidden(o, "other").unwrap().is_none());
    s.global().set("o", o.value()).unwrap();
    assert_eq!(s.eval("Object.getOwnPropertyNames(o).length", None, 1).unwrap().to_number().unwrap(), 0.0);
}

#[test]
fn global_object_backed_by_host_class() {
    // Window's Data differs from EventTarget's: inherited members borrow the
    // projected parent data through the declared upcast.
    struct WinData {
        title: String,
        target: NodeRef,
    }
    struct Window;
    impl JsClass for Window {
        const NAME: &'static str = "Window";
        type Data = WinData;
        type Parent = EventTarget;
        fn define(def: &mut ClassDef<Self>) {
            def.upcast(|w: &WinData| &w.target);
            def.getter("winId", |s, this| Ok(s.number(this.target.id as f64)));
            def.method("alert2", 0, |s, this, _a| Ok(s.string(&format!("alert {}", this.title))));
        }
    }
    let e = engine();
    let id = e.register::<Window>();
    let r = e.create_realm(RealmOptions { global_class: Some(id), ..Default::default() }).unwrap();
    let drops = Rc::new(Cell::new(0));
    r.set_global_data::<Window>(WinData { title: "t".into(), target: node(77, &drops) }).unwrap();
    let checks = [
        ("winId", "77"),
        ("alert2()", "alert t"),
        ("ping()", "pong"),
        ("globalThis instanceof Window && globalThis instanceof EventTarget", "true"),
        // The default JSC global (required for modules) has an immutable
        // [[Prototype]]: members are own properties of the global instead.
        ("alert2 === Window.prototype.alert2 && ping === EventTarget.prototype.ping", "true"),
        ("Object.prototype.toString.call(globalThis)", "[object Window]"),
        ("({}) instanceof Window", "false"),
        ("(function () { 'use strict'; return EventTarget.prototype.ping.call(undefined) })()", "pong"),
    ];
    for (src, want) in checks {
        assert_eq!(eval_str(&r, src), want, "{src}");
    }
    let s = r.scope().unwrap();
    let g = s.global().value();
    assert_eq!(s.unwrap::<EventTarget>(g).map(|d| d.id), Some(77));
    assert_eq!(s.unwrap::<Window>(g).map(|d| d.title.clone()), Some("t".to_string()));
    assert!(s.unwrap::<Node>(g).is_none());
    assert_eq!(s.global_data::<EventTarget>().map(|d| d.id), Some(77));
    assert!(r.set_global_data::<Window>(WinData { title: "again".into(), target: node(78, &drops) }).is_err());
    // Modules work in host-global realms.
    if e.capabilities().modules {
        let p = s.eval_module("globalThis.fromModule = winId + 1", "http://site.test/m.js").unwrap();
        assert!(p.is_object());
        assert_eq!(s.eval("fromModule", None, 1).unwrap().to_number().unwrap(), 78.0);
    }
}

#[test]
fn es_modules_through_fetcher() {
    let e = engine();
    if !e.capabilities().modules {
        eprintln!("module SPI unavailable; skipping");
        return;
    }
    struct Fetcher {
        requested: RefCell<Vec<String>>,
    }
    impl ModuleFetcher for Fetcher {
        fn fetch(&self, realm: &Realm, url: String, request: ModuleRequest) {
            self.requested.borrow_mut().push(url.clone());
            let s = realm.scope().unwrap();
            let src = match url.as_str() {
                "http://site.test/lib/b.js" => "export let b = 1; export function inc() { b++ }",
                "http://site.test/c.js" => "export const c = 42; export const meta = import.meta.url;",
                _ => {
                    request.reject(&s, &JsError::Type(format!("404 {url}"))).unwrap();
                    return;
                }
            };
            request.resolve(&s, src).unwrap();
        }
    }
    let r = e.create_realm(RealmOptions { origin: Some("http://site.test".into()), ..Default::default() }).unwrap();
    let fetcher = Rc::new(Fetcher { requested: RefCell::new(vec![]) });
    r.set_module_fetcher(Some(fetcher.clone()));
    let s = r.scope().unwrap();
    let p = s
        .eval_module(
            "import { b, inc } from './lib/b.js'; inc(); globalThis.out = b + ':' + import.meta.url; \
             const m = await import('/c.js'); globalThis.dyn = m.c; globalThis.dynMeta = m.meta; \
             try { await import('./missing.js') } catch (e) { globalThis.missing = e.message }",
            "http://site.test/main.js",
        )
        .unwrap();
    assert!(p.is_object());
    assert_eq!(eval_str(&r, "out"), "2:http://site.test/main.js");
    assert_eq!(eval_str(&r, "dyn"), "42");
    // Known limit (docs/COMPATIBILITY.md #3): root-relative modules keep JSC's file:/// key.
    assert_eq!(eval_str(&r, "dynMeta"), "file:///c.js");
    assert!(eval_str(&r, "missing").contains("404 http://site.test/missing.js"), "{}", eval_str(&r, "missing"));
    assert_eq!(
        *fetcher.requested.borrow(),
        vec!["http://site.test/lib/b.js", "http://site.test/c.js", "http://site.test/missing.js"]
    );
}

#[test]
fn import_meta_url_is_set_for_entry_and_fetched_modules() {
    let e = engine();
    if !e.capabilities().modules {
        eprintln!("module SPI unavailable; skipping");
        return;
    }
    struct Fetcher;
    impl ModuleFetcher for Fetcher {
        fn fetch(&self, realm: &Realm, url: String, request: ModuleRequest) {
            let s = realm.scope().unwrap();
            if url == "http://site.test/dep.js" {
                request.resolve(&s, "export const url = import.meta.url;").unwrap();
            } else {
                request.reject(&s, &JsError::Type(format!("unexpected module {url}"))).unwrap();
            }
        }
    }

    let r = e.create_realm(RealmOptions::default()).unwrap();
    r.set_module_fetcher(Some(Rc::new(Fetcher)));
    let s = r.scope().unwrap();
    s.eval_module(
        "import { url as depUrl } from './dep.js'; \
         globalThis.metaUrls = [import.meta.url, depUrl];",
        "http://site.test/main.js",
    )
    .unwrap();

    assert_eq!(eval_str(&r, "metaUrls[0]"), "http://site.test/main.js");
    assert_eq!(eval_str(&r, "metaUrls[1]"), "http://site.test/dep.js");
}

#[test]
fn terminate_handle_stops_infinite_loop() {
    let e = engine();
    let Ok(handle) = e.terminate_handle() else {
        eprintln!("termination SPI unavailable; skipping");
        return;
    };
    let r = e.create_realm(RealmOptions::default()).unwrap();
    let t = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        handle.terminate();
    });
    let start = std::time::Instant::now();
    let s = r.scope().unwrap();
    let res = s.eval("for (;;) {}", None, 1);
    t.join().unwrap();
    assert!(res.is_err());
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    // The engine keeps working afterwards.
    assert_eq!(s.eval("1 + 1", None, 1).unwrap().to_number().unwrap(), 2.0);
}

#[test]
fn isolated_worlds_have_distinct_wrappers() {
    let (e, main) = setup();
    let world =
        e.create_realm(RealmOptions { name: "__playwright_utility_world__".into(), ..Default::default() }).unwrap();
    let drops = Rc::new(Cell::new(0));
    let shared = node(5, &drops);
    expose::<Node>(&main, "n", shared.clone(), Some(IdentityKey(1, 5)));
    expose::<Node>(&world, "n", shared.clone(), Some(IdentityKey(1, 5)));
    eval_str(&main, "n.expando = 'main'");
    assert_eq!(eval_str(&world, "n.expando"), "undefined");
    eval_str(&world, "n.nodeName = 'shared-state'");
    assert_eq!(eval_str(&main, "n.nodeName"), "shared-state");
    assert_ne!(main.id(), world.id());
}

#[test]
fn unwrap_guard_keeps_payload_alive_across_gc() {
    let (e, r) = setup();
    let drops = Rc::new(Cell::new(0));
    let s = r.scope().unwrap();
    #[inline(never)]
    fn temp<'s>(s: &Scope<'s>, drops: &Rc<Cell<u32>>) -> Value<'s> {
        s.wrap::<Node>(node(42, drops), None).unwrap().value()
    }
    let guard = s.unwrap::<Node>(temp(&s, &drops)).unwrap();
    for _ in 0..3 {
        e.collect_garbage_now();
        e.drain_finalized();
    }
    assert_eq!(drops.get(), 0, "payload freed while borrowed");
    assert_eq!(guard.id, 42);
    drop(guard);
}

#[test]
fn nested_termination_cannot_be_swallowed() {
    let e = engine();
    let Ok(handle) = e.terminate_handle() else { return };
    let r = e.create_realm(RealmOptions::default()).unwrap();
    let s = r.scope().unwrap();
    let f = s
        .function("nativeLoop", |s, _this, _args| {
            // Native code re-entering JS that gets terminated.
            s.eval("for (;;) {}", None, 1)?;
            Ok(s.undefined())
        })
        .unwrap();
    s.global().set("nativeLoop", f.value()).unwrap();
    let t = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        handle.terminate();
    });
    let start = std::time::Instant::now();
    // Page code catches the termination and spins again: it must still end.
    let res = s.eval("try { nativeLoop() } catch (e) { for (;;) {} }", None, 1);
    t.join().unwrap();
    assert!(matches!(res, Err(JsError::Terminated)), "{res:?}");
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    // The VM is usable again afterwards.
    assert_eq!(s.eval("40 + 2", None, 1).unwrap().to_number().unwrap(), 42.0);
}

#[test]
fn engine_teardown_with_persistent_in_closure_allows_new_engine() {
    {
        let e = engine();
        let r = e.create_realm(RealmOptions::default()).unwrap();
        let s = r.scope().unwrap();
        let keep = Persistent::new(&s, s.object().value());
        let f = s
            .function("holder", move |s, _t, _a| {
                let _ = &keep;
                Ok(s.undefined())
            })
            .unwrap();
        s.global().set("holder", f.value()).unwrap();
        drop(r);
        drop(e);
    }
    let e2 = Engine::new().expect("engine slot freed after teardown");
    let r = e2.create_realm(RealmOptions::default()).unwrap();
    assert_eq!(eval_str(&r, "1"), "1");
}

#[test]
fn property_hooks_inherit_per_kind() {
    struct Child;
    impl JsClass for Child {
        const NAME: &'static str = "Child";
        type Data = NodeRef;
        type Parent = List;
        fn define(def: &mut ClassDef<Self>) {
            def.named(false, |s, _this, name| Ok((name == "childNamed").then(|| s.number(1.0))));
        }
    }
    let e = engine();
    e.register::<Child>();
    let r = e.create_realm(RealmOptions::default()).unwrap();
    let drops = Rc::new(Cell::new(0));
    expose::<Child>(&r, "c", node(3, &drops), None);
    assert_eq!(eval_str(&r, "[c[0], c.childNamed, c.special].join()"), "10,1,");
}

#[test]
fn internal_helpers_resist_page_monkey_patching() {
    let e = engine();
    let r = e.create_realm(RealmOptions::default()).unwrap();
    eval_str(
        &r,
        "globalThis.stolen = []; const oc = Function.prototype.call, oa = Function.prototype.apply;
         Function.prototype.call = function (...a) { stolen.push(this); return oc.apply(this, a) };
         Function.prototype.apply = function (t, a) { stolen.push(this); return Reflect.apply(this, t, a) };
         globalThis.Map = function FakeMap() {};",
    );
    let s = r.scope().unwrap();
    let seen = Rc::new(Cell::new(0));
    let seen2 = seen.clone();
    s.then(s.number(1.0), move |_s, _v| seen2.set(seen2.get() + 1), |_s, _v| {}).unwrap();
    assert_eq!(seen.get(), 1);
    assert_eq!(eval_str(&r, "stolen.length"), "0");
    let s = r.scope().unwrap();
    let d = s
        .describe(
            s.eval("new (Object.getPrototypeOf(new WeakMap()).constructor === WeakMap ? WeakMap : Object)()", None, 1)
                .unwrap(),
        )
        .unwrap();
    assert_eq!(d.subtype.as_deref(), Some("weakmap"));
    let v = s.call_function("function () { return this.x }", Some(s.eval("({x: 5})", None, 1).unwrap()), &[]).unwrap();
    assert_eq!(v.to_number().unwrap(), 5.0);
    assert_eq!(eval_str(&r, "stolen.length"), "0");
    assert_eq!(eval_str(&r, "new DOMException('m', 'toString').code"), "0");
    assert_eq!(eval_str(&r, "new DOMException('m', 'NotFoundError').code"), "8");
    assert_eq!(
        eval_str(&r, "try { Object.getOwnPropertyDescriptor(DOMException.prototype, 'code').get.call({}) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn panicking_hook_does_not_abort() {
    struct Panicky;
    impl EngineHooks for Panicky {
        fn console_api(&self, _realm: &Realm, _level: ConsoleLevel, _args: &[Value<'_>]) {
            panic!("hook panic");
        }
    }
    let e = engine();
    e.set_hooks(Some(Rc::new(Panicky)));
    let r = e.create_realm(RealmOptions::default()).unwrap();
    assert_eq!(eval_str(&r, "console.log('x'); 'survived'"), "survived");
}

#[test]
fn values_outliving_engine_owner_stay_valid() {
    let (e, r) = setup();
    let drops = Rc::new(Cell::new(0));
    let s = r.scope().unwrap();
    let o = s.wrap::<Node>(node(9, &drops), None).unwrap();
    let guard = s.unwrap::<Node>(o.value()).unwrap();
    drop(e); // the realm handle is still borrowed: VM teardown is deferred
    e2e_after_owner_drop(&s, o, &guard);
    drop(guard);
    drop(r); // last realm handle: now the VM is destroyed
    let _again = Engine::new().expect("thread slot free");
}

fn e2e_after_owner_drop(s: &Scope<'_>, o: Object<'_>, guard: &NodeData) {
    // Memory stays valid; native callbacks report the engine as gone.
    assert_eq!(guard.id, 9);
    let err = o.get("nodeId").unwrap_err();
    assert!(err.details(s).text().contains("Execution context was destroyed"), "{err:?}");
    assert_eq!(s.eval("40 + 2", None, 1).unwrap().to_number().unwrap(), 42.0);
}

#[test]
fn termination_ignored_by_native_code_still_terminates() {
    let e = engine();
    let Ok(handle) = e.terminate_handle() else { return };
    let r = e.create_realm(RealmOptions::default()).unwrap();
    let s = r.scope().unwrap();
    let f = s
        .function("swallow", |s, _this, _args| {
            let _ = s.eval("for (;;) {}", None, 1); // error ignored on purpose
            Ok(s.undefined())
        })
        .unwrap();
    s.global().set("swallow", f.value()).unwrap();
    let t = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        handle.terminate();
    });
    let start = std::time::Instant::now();
    let res = s.eval("swallow(); for (;;) {}", None, 1);
    t.join().unwrap();
    assert!(matches!(res, Err(JsError::Terminated)), "{res:?}");
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(s.eval("2", None, 1).unwrap().to_number().unwrap(), 2.0);
}

#[test]
fn engine_created_during_teardown_keeps_its_thread_slot() {
    struct Spawner;
    impl JsClass for Spawner {
        const NAME: &'static str = "Spawner";
        type Data = SpawnOnDrop;
        type Parent = NoParent;
        fn define(_def: &mut ClassDef<Self>) {}
    }
    struct SpawnOnDrop;
    impl Drop for SpawnOnDrop {
        fn drop(&mut self) {
            if let Ok(e) = Engine::new() {
                std::mem::forget(e); // keep the new engine registered
            }
        }
    }
    {
        let e = engine();
        e.register::<Spawner>();
        let r = e.create_realm(RealmOptions::default()).unwrap();
        let s = r.scope().unwrap();
        let o = s.wrap::<Spawner>(SpawnOnDrop, None).unwrap();
        s.global().set("keep", o.value()).unwrap();
        drop(r);
        drop(e); // VM teardown finalizes `keep`, whose payload spawns an engine
    }
    assert!(Engine::new().is_err(), "the engine created during teardown must stay registered");
}

#[test]
fn rooted_arguments_survive_gc_during_conversion() {
    let e = engine();
    let r = e.create_realm(RealmOptions::default()).unwrap();
    let s = r.scope().unwrap();
    let gc = s
        .function("gc", |s, _t, _a| {
            // Force a synchronous collection from inside the call.
            let src = "0";
            let _ = s.eval(src, None, 1);
            Ok(s.undefined())
        })
        .unwrap();
    s.global().set("forceGc", gc.value()).unwrap();
    let mut args = RootedVec::new();
    for i in 0..64 {
        args.push(&s, s.parse_json(&format!("{{\"token\": {i}}}")).unwrap());
        e.collect_garbage_now();
    }
    let this = Persistent::new(&s, s.parse_json("{\"base\": 1000}").unwrap());
    let v = s
        .call_function_rooted(
            "function (...xs) { forceGc(); return this.base + xs.reduce((a, x) => a + x.token, 0) }",
            Some(&this),
            &args,
        )
        .unwrap();
    assert_eq!(v.to_number().unwrap(), 1000.0 + (0..64).sum::<i32>() as f64);
}

#[test]
fn deferred_modules_release_native_payloads_before_engine_drop_returns() {
    #[derive(Default)]
    struct Fetcher(RefCell<Vec<ModuleRequest>>);
    impl ModuleFetcher for Fetcher {
        fn fetch(&self, _realm: &Realm, _url: String, request: ModuleRequest) {
            self.0.borrow_mut().push(request);
        }
    }
    let drops = Rc::new(Cell::new(0));
    for cycle in 1..=32 {
        let e = engine();
        assert!(e.capabilities().modules);
        e.register::<Node>();
        let r = e.create_realm(RealmOptions::default()).unwrap();
        expose::<Node>(&r, "keep", node(cycle, &drops), None);
        let fetcher = Rc::new(Fetcher::default());
        r.set_module_fetcher(Some(fetcher.clone()));
        {
            let s = r.scope().unwrap();
            s.eval_module("import './dependency.js'; globalThis.loaded = true", "http://site.test/main.js").unwrap();
            // Resolve outside the Objective-C shim's evaluate autorelease pool,
            // just as a network task does after fetching a dependency. The
            // dependency starts another fetch while C-API microtasks drain.
            let request = fetcher.0.borrow_mut().pop().expect("pending dependency");
            request.resolve(&s, "import './nested.js'; export const answer = 42").unwrap();
            let request = fetcher.0.borrow_mut().pop().expect("pending nested dependency");
            request.resolve(&s, "export const nested = true").unwrap();
            assert!(s.eval("loaded", None, 1).unwrap().to_bool());
        }
        drop(r);
        drop(e);
        // An autoreleased JSContext/JSVirtualMachine must not keep the VM alive
        // past its Rust finalizer queue and defer native payloads to thread exit.
        assert_eq!(drops.get(), cycle as u32, "native payload remained alive after engine teardown");
    }
}

#[test]
fn dynamic_import_from_c_api_releases_native_payloads() {
    struct Fetcher;
    impl ModuleFetcher for Fetcher {
        fn fetch(&self, realm: &Realm, _url: String, request: ModuleRequest) {
            request.resolve(&realm.scope().unwrap(), "export const answer = 42").unwrap();
        }
    }
    // Import can start outside eval_module: getters, coercions and proxy traps
    // also re-enter JS through the C API and drain module work before returning.
    for entry in ["eval", "getter", "coercion", "call", "rooted_call", "construct", "proxy"] {
        let drops = Rc::new(Cell::new(0));
        let e = engine();
        e.register::<Node>();
        let r = e.create_realm(RealmOptions::default()).unwrap();
        expose::<Node>(&r, "keep", node(1, &drops), None);
        r.set_module_fetcher(Some(Rc::new(Fetcher)));
        {
            let s = r.scope().unwrap();
            s.eval(
                "globalThis.start = function() { import('http://site.test/dep.js').then(m => globalThis.answer = m.answer); }; \
                 globalThis.obj = { get value() { start(); return 1; }, valueOf() { start(); return 1; } }; \
                 globalThis.proxy = new Proxy({}, { has() { start(); return true; } });",
                None,
                1,
            ).unwrap();
            let global = s.global();
            match entry {
                "eval" => {
                    s.eval("start()", None, 1).unwrap();
                }
                "getter" => {
                    global.get("obj").unwrap().as_object().unwrap().get("value").unwrap();
                }
                "coercion" => {
                    global.get("obj").unwrap().to_number().unwrap();
                }
                "call" => {
                    global.get("start").unwrap().as_object().unwrap().call(None, &[]).unwrap();
                }
                "rooted_call" => {
                    global.get("start").unwrap().as_object().unwrap().call_rooted(None, &RootedVec::new()).unwrap();
                }
                "construct" => {
                    global.get("start").unwrap().as_object().unwrap().construct(&[]).unwrap();
                }
                "proxy" => {
                    assert!(global.get("proxy").unwrap().as_object().unwrap().has("x"));
                }
                _ => unreachable!(),
            }
            assert_eq!(s.eval("answer", None, 1).unwrap().to_number().unwrap(), 42.0, "{entry}");
        }
        drop(r);
        drop(e);
        assert_eq!(drops.get(), 1, "native payload retained by {entry}");
    }
}

#[test]
fn global_receiver_fallback_only_for_unbound_receivers() {
    struct Single;
    impl JsClass for Single {
        const NAME: &'static str = "Single";
        type Data = NodeRef;
        type Parent = NoParent;
        fn define(def: &mut ClassDef<Self>) {
            def.global_receiver(|s| s.global_data::<Holder>().map(|h| &h.0));
            def.method("who", 0, |s, d, _a| Ok(s.number(d.id as f64)));
            def.getter("idAttr", |s, d| Ok(s.number(d.id as f64)));
        }
    }
    struct HolderData(NodeRef);
    struct Holder;
    impl JsClass for Holder {
        const NAME: &'static str = "Holder";
        type Data = HolderData;
        type Parent = NoParent;
        fn define(_def: &mut ClassDef<Self>) {}
    }
    let e = engine();
    e.register::<Element>();
    e.register::<Single>();
    let gid = e.register::<Holder>();
    let r = e.create_realm(RealmOptions { global_class: Some(gid), ..Default::default() }).unwrap();
    let drops = Rc::new(Cell::new(0));
    r.set_global_data::<Holder>(HolderData(node(41, &drops))).unwrap();
    expose::<Single>(&r, "single", node(41, &drops), None);
    expose::<Node>(&r, "other", node(7, &drops), None);
    assert_eq!(
        eval_str(&r, "const w = single.who; [w(), single.who.call(undefined), single.who.call(null), single.who.call(globalThis), Object.getOwnPropertyDescriptor(Single.prototype, 'idAttr').get.call(undefined)].join()"),
        "41,41,41,41,41"
    );
    assert_eq!(eval_str(&r, "try { single.who.call(other) } catch (e) { e.message }"), "Illegal invocation");
    assert_eq!(eval_str(&r, "try { single.who.call({}) } catch (e) { e.message }"), "Illegal invocation");
    // Without a fallback, unbound receivers still throw.
    assert_eq!(eval_str(&r, "try { const p = other.ping; p() } catch (e) { e.message }"), "Illegal invocation");
}
