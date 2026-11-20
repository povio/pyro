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
    engine: Rc<Engine>,
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
    let engine = Rc::new(Engine::new().expect("engine"));
    register(&engine);
    realm_env(engine, html)
}

fn realm_env(engine: Rc<Engine>, html: &str) -> Env {
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
    let scope = realm.scope().unwrap();
    scope.eval("const frames=[],tasks=[]; globalThis.requestAnimationFrame=fn=>frames.push(fn); globalThis.setTimeout=(fn,ms)=>ms===0?tasks.push(fn):1; globalThis.clearTimeout=()=>{}", None, 1).unwrap();
    install(&scope, page.clone()).unwrap();
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

#[test]
fn view_transition_selector_class_budget() {
    let environment = env("<!doctype html><body>");
    assert_eq!(eval(&environment.main, r#"(() => {
        const accepts = arg => CSS.supports(`selector(::view-transition-group(${arg}))`);
        return [accepts('*'), accepts('\\*'), accepts('*.tile'), accepts('*. tile'),
                accepts('*' + '.tile'.repeat(32)), accepts('*' + '.tile'.repeat(33))].join();
    })()"#), "true,true,true,false,true,false");
}

#[test]
fn view_transition_keyframe_effect_named_pseudo() {
    let environment = env("<!doctype html><body>");
    assert_eq!(eval(&environment.main, r#"(() => {
        const effect = new KeyframeEffect(document.body, [], {pseudoElement:'::view-transition-old(fruit)'});
        try { effect.pseudoElement = '::view-transition-old(a b)'; return 'accepted'; } catch (error) { return effect.pseudoElement + ':' + error.name; }
    })()"#), "::view-transition-old(fruit):SyntaxError");
}

struct Child(Rc<PageDom>);
impl PageHooks for Child {
    fn container(&self) -> FrameContainer { FrameContainer::Frame(self.0.clone(), self.0.document()) }
    fn navigate(&self, _: NavigationRequest) {}
    fn post_task(&self, _: Box<dyn FnOnce()>) {}
}

#[test]
fn view_transition_capture_bytes_and_recording_work_shared_across_frame_realms() {
    use pyro_dom::{geometry::TransitionBox, layout::{GeometrySource, ExternalBox, ExternalGeometry, Layout}};
    struct Probe(RefCell<Vec<(u64, u64)>>);
    impl GeometrySource for Probe {
        fn sync(&self, _: &Dom, _: &Layout) {}
        fn element_box(&self, _: u64) -> Option<ExternalBox> { None }
        fn geometry(&self) -> Option<ExternalGeometry> { None }
        fn document_size(&self) -> Option<(f64, f64)> { None }
        fn has_sticky(&self) -> bool { false }
        fn as_any(&self) -> &dyn std::any::Any { self }
        fn capture_transition(&self, key: u64, old: bool, pixels: u64, work: &mut u64) -> Option<Vec<TransitionBox>> {
            assert!(old);
            self.0.borrow_mut().push((pixels, *work));
            *work -= 137;
            Some(vec![TransitionBox { key, name: "root".into(), size: (3., 1.), transform: [0.; 6], ink: [0.; 4], rgba: Some(std::sync::Arc::new(vec![0; 12])) }])
        }
    }
    let parent = env("<!doctype html><body></body>");
    let child = realm_env(parent.engine.clone(), "<!doctype html><body></body>");
    child.page.set_hooks(Some(Rc::new(Child(parent.page.clone()))));
    let probe = Rc::new(Probe(RefCell::new(Vec::new())));
    for frame in [&child, &parent] {
        frame.page.layout.borrow_mut().set_geometry_source(Some(probe.clone()));
        assert_eq!(eval(&frame.main, "const start = () => {document.startViewTransition(() => new Promise(() => {})); frames.shift()()}; start(); 'ok'"), "ok");
    }
    let parent_prep = u64::from(parent.page.dom.borrow().slot_count()) + 1;
    let child_prep = u64::from(child.page.dom.borrow().slot_count()) + 1;
    assert_eq!(*probe.0.borrow(), vec![(3 << 22, (3 << 16) - child_prep), ((1 << 24) - 3, (1 << 18) - 137 - child_prep - parent_prep)]);
    child.page.detach();
    assert_eq!(eval(&parent.main, "start(); 'ok'"), "ok");
    assert_eq!(probe.0.borrow()[2..], [(1 << 24, (1 << 18) - parent_prep)]);
}

#[test]
fn view_transition_failed_capture_debit_precedes_viewport_sync() {
    use pyro_dom::layout::{GeometrySource, ExternalBox, ExternalGeometry, Layout};
    struct Failure(std::cell::Cell<u32>);
    impl GeometrySource for Failure {
        fn sync(&self, _: &Dom, _: &Layout) {}
        fn element_box(&self, _: u64) -> Option<ExternalBox> { None }
        fn geometry(&self) -> Option<ExternalGeometry> { None }
        fn document_size(&self) -> Option<(f64, f64)> { None }
        fn has_sticky(&self) -> bool { false }
        fn as_any(&self) -> &dyn std::any::Any { self }
        fn capture_transition(&self, _: u64, _: bool, _: u64, work: &mut u64) -> Option<Vec<pyro_dom::geometry::TransitionBox>> {
            self.0.update(|count| count + 1);
            *work = 0;
            None
        }
    }
    let environment = env("<!doctype html><body></body>");
    let probe = Rc::new(Failure(0.into()));
    environment.page.layout.borrow_mut().set_geometry_source(Some(probe.clone()));
    let layouts = Rc::new(std::cell::Cell::new(0));
    environment.page.set_frame_viewport_source(Some(Rc::new({
        let layouts = layouts.clone();
        move || { layouts.update(|count| count + 1); None }
    })));
    assert_eq!(eval(&environment.main, "function start(){document.startViewTransition().ready.catch(()=>{}); tasks.splice(0).forEach(fn=>fn()); frames.shift()()} start(); 'ok'"), "ok");
    let baseline = layouts.get();
    assert!(baseline > 0);
    assert_eq!(eval(&environment.main, "for(let i=0;i<32;i++)start(); 'ok'"), "ok");
    assert_eq!((probe.0.get(), layouts.get()), (1, baseline));
}

#[test]
fn view_transition_wait_until_keeps_promise_not_frame_slots() {
    let parent = env("<!doctype html><body></body>");
    let child = realm_env(parent.engine.clone(), "<!doctype html><style>*::view-transition-group(*),*::view-transition-old(*),*::view-transition-new(*){animation:none}</style><body></body>");
    child.page.set_hooks(Some(Rc::new(Child(parent.page.clone()))));
    assert_eq!(eval(&child.main, r#"let done=0,ready=0,held=[];
        function start(){let el=document.createElement('div');document.body.append(el);let t=el.startViewTransition();t.waitUntil(new Promise(()=>{}));t.ready.then(()=>++ready,()=>{});t.finished.then(()=>++done);held.push(t);return el.activeViewTransition===t}
        Array.from({length:4},start).every(Boolean)"#), "true");
    for _ in 0..4 { eval(&child.main, "frames.splice(0).forEach(fn=>fn())"); }
    assert_eq!(eval(&child.main, "[ready,done,start()].join()"), "4,0,true");
}

#[test]
fn view_transition_capture_slots_shared_across_frame_realms() {
    let parent = env("<!doctype html><body></body>");
    let child = realm_env(parent.engine.clone(), "<!doctype html><body></body>");
    child.page.set_hooks(Some(Rc::new(Child(parent.page.clone()))));
    assert_eq!(eval(&child.main, "let admitted = 0; for(let i=0;i<16;i++){let el=document.createElement('div'); document.body.appendChild(el); el.startViewTransition(); if(el.activeViewTransition) ++admitted;} [frames.length,admitted].join()"), "1,4");
    assert_eq!(eval(&parent.main, "document.startViewTransition(); [frames.length, document.activeViewTransition !== null].join()"), "1,true");
    child.page.detach();
    assert_eq!(eval(&parent.main, "document.startViewTransition(); [frames.length, document.activeViewTransition !== null].join()"), "1,true");
}

#[test]
fn view_transition_capture_pump_coalesces_rapid_starts() {
    let environment = env("<!doctype html><html><body></body></html>");
    assert_eq!(eval(&environment.main, r#"let updates = 0;
        for (let index = 0; index < 1000; ++index) document.startViewTransition(() => ++updates);
        const queued = frames.length; frames.shift()(); const before=updates;
        tasks.splice(0).forEach(fn=>fn()); const skipped=updates; frames.shift()(); [queued,before,skipped,updates].join()"#), "1,0,999,1000");
}

#[test]
fn view_transition_skipped_callback_invocation_ignores_page_scheduling() {
    let environment = env("<!doctype html><body></body>");
    assert_eq!(eval(&environment.main, r#"const original=Promise.prototype.then; const broken=()=>{throw Error('page scheduling')};
        globalThis.setTimeout=globalThis.clearTimeout=globalThis.requestAnimationFrame=Promise.prototype.then=Array.from=broken;
        let order=''; const first=document.startViewTransition(()=>{order+='a';return new Promise(()=>{})}); first.skipTransition();
        document.startViewTransition(()=>{order+='b'}); frames.shift()(); const before=order;
        tasks.shift()(); frames.shift()(); Promise.prototype.then=original; before+':'+order"#), ":ab");
}

#[test]
fn view_transition_document_element_shares_document_scope() {
    let environment = env("<!doctype html><body></body>");
    assert_eq!(eval(&environment.main, r#"globalThis.requestAnimationFrame = globalThis.setTimeout = () => 1;
        let error; const first = document.startViewTransition(() => new Promise(() => {}));
        first.ready.catch(reason => error = reason.name);
        const root = document.documentElement, second = root.startViewTransition();
        [document.activeViewTransition === second, root.activeViewTransition === second, second.transitionRoot === root].join()"#), "true,true,true");
    assert_eq!(eval(&environment.main, "error"), "AbortError");
}

#[test]
fn view_transition_active_scope_and_interface_identity() {
    let environment = env("<!doctype html><html><body></body></html>");
    eval(&environment.main, "globalThis.requestAnimationFrame = () => 1; globalThis.setTimeout = () => 1;");
    assert_eq!(eval(&environment.main, "document.activeViewTransition === null && document.body.activeViewTransition === null"), "true");
    assert_eq!(eval(&environment.main, "const transition = document.startViewTransition(); [transition instanceof ViewTransition, document.activeViewTransition === transition, document.body.activeViewTransition === null].join()"), "true,true,true");
    assert_eq!(eval(&environment.main, "const pending = new Promise(() => {}); let accepted = 0; try { for (; accepted < 1025; ++accepted) transition.waitUntil(pending); } catch (error) { if (error.name !== 'QuotaExceededError') throw error; } accepted"), "1024");
    assert_eq!(eval(&environment.main, "transition.skipTransition(); document.activeViewTransition === null"), "true");
}
