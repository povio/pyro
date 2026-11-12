//! Focus and shadow DOM on the real JavaScriptCore: `activeElement`
//! retargeting, `ShadowRoot.styleSheets`/`onslotchange`, hit testing and
//! `composedPath()` across shadow boundaries, slotchange ordering, focus
//! rules (rendering, inertness, `delegatesFocus`), sequential Tab
//! navigation, mouse pointer capture, `:focus-visible` and `:active`.

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
    page: Rc<PageDom>,
    main: Realm,
}

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
    vp.width = 800;
    vp.height = 600;
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

fn tab(page: &Rc<PageDom>, shift: bool) {
    let modifiers = input::Modifiers { shift, ..Default::default() };
    let k = input::KeyInput { key: "Tab".into(), code: "Tab".into(), key_code: 9, modifiers, ..Default::default() };
    input::dispatch_key(page, input::KeyKind::RawKeyDown, &k);
    input::dispatch_key(page, input::KeyKind::KeyUp, &k);
}

fn mouse(page: &Rc<PageDom>, kind: input::MouseKind, x: f64, y: f64, buttons: u16) {
    let m = input::MouseInput {
        x,
        y,
        button: if kind == input::MouseKind::Moved { input::MouseButton::None } else { input::MouseButton::Left },
        buttons,
        click_count: 1,
        ..Default::default()
    };
    input::dispatch_mouse(page, kind, &m).unwrap();
}

/// The deepest focused element's id, through open shadow roots.
const DEEP_FOCUS: &str = "(() => { let a = document.activeElement; \
    while (a && a.shadowRoot && a.shadowRoot.activeElement) a = a.shadowRoot.activeElement; return a ? a.id : null; })()";

#[test]
fn active_element_is_retargeted_per_tree() {
    let e = env("<body><input id=outer><div id=host></div><div id=other></div></body>");
    let r = &e.main;
    eval(
        r,
        "window.sr = host.attachShadow({mode: 'open'}); sr.innerHTML = '<input id=inner><div id=nh></div>'; \
         window.nsr = sr.getElementById('nh').attachShadow({mode: 'closed'}); nsr.innerHTML = '<input id=deep>'; \
         window.osr = other.attachShadow({mode: 'open'});",
    );
    assert_eq!(eval(r, "[document.activeElement === document.body, sr.activeElement].join()"), "true,");
    eval(r, "sr.getElementById('inner').focus()");
    assert_eq!(eval(r, "[document.activeElement.id, sr.activeElement.id, osr.activeElement].join()"), "host,inner,");
    eval(r, "nsr.getElementById('deep').focus()");
    assert_eq!(
        eval(r, "[document.activeElement.id, sr.activeElement.id, nsr.activeElement.id].join()"),
        "host,nh,deep"
    );
    eval(r, "outer.focus()");
    assert_eq!(eval(r, "[document.activeElement.id, sr.activeElement, nsr.activeElement].join()"), "outer,,");
    // Removing the host leaves no focused element in its tree.
    eval(r, "sr.getElementById('inner').focus(); host.remove()");
    assert_eq!(eval(r, "[sr.activeElement, document.activeElement === document.body].join()"), ",true");
}

#[test]
fn shadow_root_style_sheets_and_onslotchange() {
    let e = env("<body><div id=host><span>x</span></div></body>");
    let r = &e.main;
    eval(
        r,
        "window.host2 = document.createElement('div'); window.sr = host2.attachShadow({mode: 'open'}); \
         sr.innerHTML = '<style>p{}</style><style>a{}</style><p>t</p>';",
    );
    assert_eq!(eval(r, "sr.styleSheets.length"), "0");
    eval(r, "document.body.append(host2)");
    assert_eq!(
        eval(r, "const s = sr.querySelectorAll('style'); [sr.styleSheets.length, sr.styleSheets[0] === s[0].sheet, sr.styleSheets[1] === s[1].sheet, sr.styleSheets === sr.styleSheets, document.styleSheets.length].join()"),
        "2,true,true,true,0"
    );
    eval(r, "host2.remove()");
    assert_eq!(eval(r, "sr.styleSheets.length"), "0");
    // ShadowRoot.onslotchange is an event handler IDL attribute.
    eval(
        r,
        "window.log = []; window.hr = host.attachShadow({mode: 'open'}); hr.innerHTML = '<slot></slot>'; \
         hr.onslotchange = e => log.push('slotchange:' + e.target.localName);",
    );
    assert_eq!(eval(r, "typeof hr.onslotchange"), "function");
    // The new slot's own assignment signalled it once already.
    assert_eq!(eval(r, "log.splice(0).join()"), "slotchange:slot");
    eval(r, "host.append(document.createElement('b'))");
    assert_eq!(eval(r, "log.join()"), "slotchange:slot");
}

#[test]
fn slotchange_fires_after_mutation_observer_callbacks() {
    let e = env("<body><div id=host></div></body>");
    let r = &e.main;
    eval(
        r,
        "window.log = []; const sr = host.attachShadow({mode: 'open'}); sr.innerHTML = '<slot></slot>'; \
         sr.querySelector('slot').addEventListener('slotchange', () => log.push('slotchange')); \
         new MutationObserver(() => log.push('observer')).observe(host, {childList: true});",
    );
    eval(r, "host.append(document.createElement('i'))");
    assert_eq!(eval(r, "log.join()"), "observer,slotchange");
}

#[test]
fn hit_testing_from_the_document_is_retargeted_to_hosts() {
    let e = env(
        "<!DOCTYPE html><style>body{margin:0} .h{display:block;width:100px;height:100px}</style>\
         <body><div id=host class=h></div><div id=host2 class=h><span id=sp style='display:block;height:100px'>s</span></div></body>",
    );
    let r = &e.main;
    eval(
        r,
        "window.sr = host.attachShadow({mode: 'closed'}); \
         sr.innerHTML = '<div id=in style=\"width:100px;height:100px\"></div>'; \
         window.sr2 = host2.attachShadow({mode: 'closed'}); sr2.innerHTML = '<div id=wrap><slot></slot></div>';",
    );
    let ids = |src: &str| eval(r, &format!("{src}.map(e => e.id || e.localName).join()"));
    assert_eq!(eval(r, "sr.elementFromPoint(50, 50).id"), "in");
    assert_eq!(eval(r, "document.elementFromPoint(50, 50).id"), "host");
    assert_eq!(ids("document.elementsFromPoint(50, 50)"), "host,body,html");
    assert_eq!(ids("sr.elementsFromPoint(50, 50)"), "in,host,body,html");
    // A slotted element is in the document's tree: it is not retargeted.
    assert_eq!(ids("document.elementsFromPoint(50, 150)"), "sp,host2,body,html");
    assert_eq!(ids("sr2.elementsFromPoint(50, 150)"), "sp,wrap,host2,body,html");
}

#[test]
fn composed_path_hides_closed_shadow_internals_from_outside() {
    let e = env("<body><div id=host></div></body>");
    let r = &e.main;
    eval(
        r,
        "window.sr = host.attachShadow({mode: 'closed'}); sr.innerHTML = '<p id=inner></p>'; \
         window.outside = []; window.inside = []; \
         host.addEventListener('ping', ev => outside = ev.composedPath().map(n => n.id || n.nodeName)); \
         sr.getElementById('inner').addEventListener('ping', ev => inside = ev.composedPath().map(n => n.id || n.nodeName)); \
         window.ev = new Event('ping', {bubbles: true, composed: true}); \
         sr.getElementById('inner').dispatchEvent(ev);",
    );
    assert_eq!(eval(r, "inside.join()"), "inner,#document-fragment,host,BODY,HTML,#document,");
    assert_eq!(eval(r, "outside.join()"), "host,BODY,HTML,#document,");
    assert_eq!(eval(r, "ev.composedPath().length"), "0");
}

#[test]
fn focus_refuses_hidden_inert_and_delegates_through_hosts() {
    let e = env("<body><input id=a><div style='display:none'><input id=hidden></div>\
         <input id=invisible style='visibility:hidden'><div inert><input id=inert></div>\
         <div id=df></div><div id=empty></div></body>");
    let r = &e.main;
    eval(r, "a.focus()");
    for id in ["hidden", "invisible", "inert"] {
        eval(r, &format!("document.getElementById('{id}').focus()"));
        assert_eq!(eval(r, "document.activeElement.id"), "a", "{id} took focus");
    }
    eval(
        r,
        "window.dsr = df.attachShadow({mode: 'open', delegatesFocus: true}); \
         dsr.innerHTML = '<span>t</span><input id=first><input id=second>'; \
         empty.attachShadow({mode: 'open', delegatesFocus: true}).innerHTML = '<span>none</span>'; \
         empty.tabIndex = 0;",
    );
    eval(r, "df.focus()");
    assert_eq!(eval(r, "[document.activeElement.id, dsr.activeElement.id].join()"), "df,first");
    // Focus already inside the host stays where it is.
    eval(r, "dsr.getElementById('second').focus(); df.focus()");
    assert_eq!(eval(r, "dsr.activeElement.id"), "second");
    // A delegating host with nothing focusable inside takes no focus itself.
    eval(r, "a.focus(); empty.focus()");
    assert_eq!(eval(r, "document.activeElement.id"), "a");
}

#[test]
fn tab_walks_shadow_scopes_slots_and_positive_tabindex() {
    let e = env("<body><input id=a><input id=p2 tabindex=2><input id=p1 tabindex=1>\
         <div id=host><input id=slotted slot=s></div>\
         <div id=skip tabindex=-1></div><input id=dis disabled><div hidden><input id=gone></div>\
         <input id=last></body>");
    let r = &e.main;
    eval(
        r,
        "host.attachShadow({mode: 'open'}).innerHTML = '<input id=s1><slot name=s></slot><input id=s2>'; \
         skip.attachShadow({mode: 'open'}).innerHTML = '<input id=skipped>';",
    );
    let mut forward = Vec::new();
    for _ in 0..8 {
        tab(&e.page, false);
        forward.push(eval(r, DEEP_FOCUS));
    }
    assert_eq!(forward, ["p1", "p2", "a", "s1", "slotted", "s2", "last", "last"]);
    let mut back = Vec::new();
    for _ in 0..7 {
        tab(&e.page, true);
        back.push(eval(r, DEEP_FOCUS));
    }
    assert_eq!(back, ["s2", "slotted", "s1", "a", "p2", "p1", "p1"]);
}

#[test]
fn mouse_pointer_capture_retargets_and_releases_after_pointerup() {
    let e = env("<!DOCTYPE html><style>body{margin:0} div{position:absolute;width:50px;height:50px}</style>\
         <body><div id=t0 style='left:0;top:0'></div><div id=t1 style='left:100px;top:0'></div></body>");
    let r = &e.main;
    eval(
        r,
        "window.log = []; for (const el of [t0, t1]) for (const t of ['pointerdown','pointermove','pointerup','gotpointercapture','lostpointercapture','mouseup','click']) \
           el.addEventListener(t, ev => log.push(t + '@' + el.id + (t === 'pointerup' ? ':' + t0.hasPointerCapture(ev.pointerId) : ''))); \
         t0.addEventListener('pointerdown', ev => { \
           log.push('has:' + t0.hasPointerCapture(ev.pointerId)); t0.setPointerCapture(ev.pointerId); \
           log.push('has:' + t0.hasPointerCapture(ev.pointerId) + ',' + t1.hasPointerCapture(ev.pointerId)); });",
    );
    assert_eq!(eval(r, "try { t0.setPointerCapture(-5) } catch (e) { e.name }"), "NotFoundError");
    // WebIDL `long` wraps: this is -157381632, not a clamped large id.
    assert_eq!(eval(r, "try { t0.releasePointerCapture(314159265358973923) } catch (e) { e.name }"), "NotFoundError");
    assert_eq!(
        eval(r, "try { document.createElement('i').setPointerCapture(1) } catch (e) { e.name }"),
        "InvalidStateError"
    );
    // No button down: capture is not taken.
    eval(r, "t0.setPointerCapture(1)");
    assert_eq!(eval(r, "t0.hasPointerCapture(1)"), "false");
    mouse(&e.page, input::MouseKind::Moved, 10.0, 10.0, 0);
    eval(r, "log.length = 0");
    mouse(&e.page, input::MouseKind::Pressed, 10.0, 10.0, 1);
    mouse(&e.page, input::MouseKind::Moved, 120.0, 10.0, 1);
    mouse(&e.page, input::MouseKind::Released, 120.0, 10.0, 0);
    assert_eq!(
        eval(r, "log.join()"),
        "pointerdown@t0,has:false,has:true,false,gotpointercapture@t0,pointermove@t0,pointerup@t0:true,\
         lostpointercapture@t0,mouseup@t0,click@t0"
    );
    assert_eq!(eval(r, "t0.hasPointerCapture(1)"), "false");
    // A removed capture target: lostpointercapture goes to the document.
    eval(
        r,
        "log.length = 0; document.addEventListener('lostpointercapture', ev => log.push('lost@' + (ev.target.id || ev.target.nodeName)));",
    );
    mouse(&e.page, input::MouseKind::Pressed, 10.0, 10.0, 1);
    mouse(&e.page, input::MouseKind::Moved, 20.0, 10.0, 1);
    eval(r, "t0.remove()");
    mouse(&e.page, input::MouseKind::Moved, 120.0, 10.0, 1);
    mouse(&e.page, input::MouseKind::Released, 120.0, 10.0, 0);
    assert_eq!(eval(r, "log.filter(l => l.startsWith('lost')).join()"), "lost@#document");
}

#[test]
fn focus_visible_follows_input_modality_and_active_tracks_the_press() {
    let e = env("<!DOCTYPE html><style>body{margin:0} button,input{position:absolute;width:50px;height:30px;border:0}\
         #t:focus-visible{color:rgb(1, 2, 3)} #b:active{color:rgb(4, 5, 6)}</style>\
         <body><button id=b style='left:0;top:0'>b</button><input id=t style='left:100px;top:0'>\
         <button id=c style='left:200px;top:0'>c</button></body>");
    let r = &e.main;
    let state = "[document.activeElement.id, document.activeElement.matches(':focus-visible')].join()";
    // Script focus before any pointer press shows the ring (Chrome).
    eval(r, "c.focus()");
    assert_eq!(eval(r, state), "c,true");
    eval(r, "window.active = []; b.addEventListener('mousedown', () => active.push(b.matches(':active'), document.body.matches(':active')));");
    mouse(&e.page, input::MouseKind::Pressed, 10.0, 10.0, 1);
    assert_eq!(
        eval(r, "[b.matches(':active'), document.body.matches(':active'), c.matches(':active')].join()"),
        "true,true,false"
    );
    assert_eq!(eval(r, state), "b,false");
    assert_eq!(eval(r, "getComputedStyle(b).color"), "rgb(4, 5, 6)");
    mouse(&e.page, input::MouseKind::Released, 10.0, 10.0, 0);
    assert_eq!(eval(r, "[active.join(), b.matches(':active')].join()"), "true,true,false");
    assert_ne!(eval(r, "getComputedStyle(b).color"), "rgb(4, 5, 6)");
    // Script focus after a pointer press keeps the pointer modality.
    eval(r, "c.focus()");
    assert_eq!(eval(r, state), "c,false");
    // A text field shows the ring even when clicked.
    mouse(&e.page, input::MouseKind::Pressed, 110.0, 10.0, 1);
    mouse(&e.page, input::MouseKind::Released, 110.0, 10.0, 0);
    assert_eq!(eval(r, state), "t,true");
    // Keyboard navigation shows it.
    mouse(&e.page, input::MouseKind::Pressed, 10.0, 10.0, 1);
    mouse(&e.page, input::MouseKind::Released, 10.0, 10.0, 0);
    assert_eq!(eval(r, state), "b,false");
    tab(&e.page, false);
    assert_eq!(eval(r, state), "t,true");
    assert_eq!(eval(r, "getComputedStyle(t).color"), "rgb(1, 2, 3)");
    // Clicking a button after the keyboard: no ring on the button.
    mouse(&e.page, input::MouseKind::Pressed, 10.0, 10.0, 1);
    mouse(&e.page, input::MouseKind::Released, 10.0, 10.0, 0);
    assert_eq!(
        eval(r, "[getComputedStyle(t).color === 'rgb(1, 2, 3)', b.matches(':focus-visible')].join()"),
        "false,false"
    );
}

#[test]
fn active_restyles_only_when_a_sheet_uses_it() {
    let e = env("<!DOCTYPE html><style>button{position:absolute;width:50px;height:30px;border:0}</style>\
         <body style='margin:0'><button id=b style='left:0;top:0'>b</button></body>");
    let r = &e.main;
    eval(r, "b.focus(); b.getBoundingClientRect()");
    // No sheet reads `:active`: the press changes the state, nothing else.
    let before = e.page.dom.borrow().version();
    mouse(&e.page, input::MouseKind::Pressed, 10.0, 10.0, 1);
    assert_eq!(e.page.dom.borrow().version(), before);
    assert_eq!(eval(r, "[b.matches(':active'), document.body.matches(':active')].join()"), "true,true");
    // A sheet that reads it, added during the press, sees the pressed state.
    eval(
        r,
        "const s = document.createElement('style'); s.textContent = '#b:active{color:rgb(4, 5, 6)}'; \
         document.head.append(s)",
    );
    assert_eq!(eval(r, "getComputedStyle(b).color"), "rgb(4, 5, 6)");
    mouse(&e.page, input::MouseKind::Released, 10.0, 10.0, 0);
    assert_eq!(eval(r, "[b.matches(':active'), getComputedStyle(b).color === 'rgb(4, 5, 6)'].join()"), "false,false");
    // From now on a press restyles.
    mouse(&e.page, input::MouseKind::Pressed, 10.0, 10.0, 1);
    assert_eq!(eval(r, "getComputedStyle(b).color"), "rgb(4, 5, 6)");
    mouse(&e.page, input::MouseKind::Released, 10.0, 10.0, 0);
    assert_ne!(eval(r, "getComputedStyle(b).color"), "rgb(4, 5, 6)");
}
