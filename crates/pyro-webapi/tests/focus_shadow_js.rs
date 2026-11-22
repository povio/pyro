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
    // A press on nothing focusable clears focus with no focus type: script focus after it
    // shows the ring again (Chrome; WPT focus-visible-script-focus-006).
    mouse(&e.page, input::MouseKind::Pressed, 400.0, 200.0, 1);
    mouse(&e.page, input::MouseKind::Released, 400.0, 200.0, 0);
    assert_eq!(eval(r, "document.activeElement === document.body"), "true");
    eval(r, "c.focus()");
    assert_eq!(eval(r, state), "c,true");
}

#[test]
fn active_restyles_only_when_a_sheet_uses_it() {
    let e = env("<!DOCTYPE html><style>button{position:absolute;width:50px;height:30px;border:0}</style>\
         <body style='margin:0'><button id=b style='left:0;top:0'>b</button></body>");
    let r = &e.main;
    eval(r, "b.focus(); b.getBoundingClientRect()");
    // The pointer arrives first: a press where it was not reconciles hover too (boundary events,
    // as Chromium does for a pointer that moved without a move event), which is not what this
    // checks.
    mouse(&e.page, input::MouseKind::Moved, 10.0, 10.0, 0);
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

/// A press and a release on the button at (10, 10).
fn click(page: &Rc<PageDom>) {
    mouse(page, input::MouseKind::Pressed, 10.0, 10.0, 1);
    mouse(page, input::MouseKind::Released, 10.0, 10.0, 0);
}

const CLICK_PAGE: &str = "<!DOCTYPE html><style>button{position:absolute;width:50px;height:30px;border:0}</style>\
     <body style='margin:0'><button id=b style='left:0;top:0'>b</button><input id=t style='position:absolute;top:100px'></body>";

/// `log` and a listener that notes each event it hears (the test page runs no
/// `<script>`).
const NOTE: &str = "window.log = []; window.note = (e) => log.push(e.type + '@' + (e.currentTarget === window ? 'window' : \
     e.currentTarget === document ? 'document' : e.currentTarget.id) + (e.isTrusted ? '' : '!'));";

/// The click page with `NOTE` evaluated and layout done.
fn click_env() -> Env {
    let e = env(CLICK_PAGE);
    eval(&e.main, NOTE);
    eval(&e.main, "b.getBoundingClientRect()");
    e
}

// The events of a click nobody listens for are not made (`target::unheard`).
// What a page can see must not depend on that.

#[test]
fn a_click_nobody_listens_for_still_does_its_default_actions() {
    let e = click_env();
    let r = &e.main;
    eval(r, "t.focus(); b.getBoundingClientRect()");
    for ty in ["pointerdown", "mousedown", "pointerup", "mouseup", "click"] {
        assert!(!e.page.dom.borrow().listens_for(ty), "{ty}");
    }
    mouse(&e.page, input::MouseKind::Pressed, 10.0, 10.0, 1);
    // Focus moved to the button, the press shows in `:active`.
    assert_eq!(eval(r, "[document.activeElement.id, b.matches(':active')].join()"), "b,true");
    mouse(&e.page, input::MouseKind::Released, 10.0, 10.0, 0);
    assert_eq!(eval(r, "[document.activeElement.id, b.matches(':active')].join()"), "b,false");
    // Activation behaviour runs with no listener anywhere: a checkbox
    // toggles, and its `change` listener (another type) hears it.
    eval(
        r,
        "b.remove(); const c = document.createElement('input'); c.type = 'checkbox'; c.id = 'c'; \
         c.style = 'position:absolute;left:0;top:0;width:20px;height:20px;margin:0'; document.body.append(c); \
         c.addEventListener('change', note); c.getBoundingClientRect()",
    );
    assert!(!e.page.dom.borrow().listens_for("click"));
    click(&e.page);
    assert_eq!(eval(r, "[c.checked, log.join()].join(' ')"), "true change@c");
}

#[test]
fn a_listener_added_during_a_click_hears_the_later_events_of_that_click() {
    let e = click_env();
    let r = &e.main;
    eval(r, "b.getBoundingClientRect()");
    // Only `pointerdown` is listened for at first. Its listener adds one for
    // `mousedown`, that one adds one for `pointerup` on the document, and so
    // on down to `click`: each event is asked about right before it fires.
    eval(
        r,
        "b.addEventListener('pointerdown', (e) => { note(e); \
           b.addEventListener('mousedown', (e) => { note(e); \
             document.addEventListener('pointerup', (e) => { note(e); \
               window.addEventListener('mouseup', (e) => { note(e); \
                 document.body.addEventListener('click', note, {capture: true, once: true}); \
               }, {once: true}); \
             }, {once: true}); \
           }, {once: true}); \
         }, {once: true});",
    );
    click(&e.page);
    assert_eq!(eval(r, "log.join()"), "pointerdown@b,mousedown@b,pointerup@document,mouseup@window,click@");
    // Every one was `once`: the next click is unheard again, from the start.
    for ty in ["pointerdown", "mousedown", "pointerup", "mouseup", "click"] {
        assert!(!e.page.dom.borrow().listens_for(ty), "{ty}");
    }
    click(&e.page);
    assert_eq!(eval(r, "log.length"), "5");
}

#[test]
fn a_listener_on_the_window_only_hears_each_event_of_a_click() {
    let e = click_env();
    let r = &e.main;
    eval(r, "b.getBoundingClientRect()");
    eval(r, "window.addEventListener('mouseup', note)");
    click(&e.page);
    assert_eq!(eval(r, "log.join()"), "mouseup@window");
    eval(r, "window.addEventListener('pointerdown', note, true); window.onclick = note; log.length = 0");
    click(&e.page);
    assert_eq!(eval(r, "log.join()"), "pointerdown@window,mouseup@window,click@window");
    // Removed again, the three ways: the events go back to unheard.
    eval(
        r,
        "window.removeEventListener('mouseup', note); window.removeEventListener('pointerdown', note, true); \
         window.onclick = null; log.length = 0",
    );
    for ty in ["pointerdown", "mouseup", "click"] {
        assert!(!e.page.dom.borrow().listens_for(ty), "{ty}");
    }
    click(&e.page);
    assert_eq!(eval(r, "log.join()"), "");
    // A removal with the wrong capture flag removes nothing.
    eval(r, "window.addEventListener('mousedown', note); window.removeEventListener('mousedown', note, true)");
    click(&e.page);
    assert_eq!(eval(r, "log.join()"), "mousedown@window");
}

#[test]
fn an_event_handler_attribute_or_property_set_later_hears_the_click() {
    let e = env(
        "<!DOCTYPE html><style>button{position:absolute;width:50px;height:30px;border:0}</style>\
         <body style='margin:0' onpointerup=\"log.push('pointerup:body-attr')\">\
         <button id=b style='left:0;top:0' onmousedown=\"log.push('mousedown:parsed-attr')\">b</button>\
         </body>",
    );
    let r = &e.main;
    eval(r, "window.log = []; b.getBoundingClientRect()");
    // Attributes the parser saw, never touched by script before the click.
    click(&e.page);
    assert_eq!(eval(r, "log.join()"), "mousedown:parsed-attr,pointerup:body-attr");
    // An attribute set later, a property set later, and one set from a
    // listener of an earlier event of the same click.
    eval(
        r,
        "log.length = 0; b.removeAttribute('onmousedown'); document.body.removeAttribute('onpointerup'); \
         b.setAttribute('onpointerdown', \"log.push('pointerdown:attr'); \
           document.documentElement.setAttribute('onmouseup', \\\"log.push('mouseup:attr-during')\\\")\"); \
         document.onclick = () => log.push('click:property');",
    );
    click(&e.page);
    assert_eq!(eval(r, "log.join()"), "pointerdown:attr,mouseup:attr-during,click:property");
    // Taken away again (attribute removed, property nulled).
    eval(
        r,
        "log.length = 0; b.removeAttribute('onpointerdown'); document.documentElement.removeAttribute('onmouseup'); \
         document.onclick = null;",
    );
    click(&e.page);
    assert_eq!(eval(r, "log.join()"), "");
    for ty in ["pointerdown", "mousedown", "pointerup", "mouseup", "click"] {
        assert!(!e.page.dom.borrow().listens_for(ty), "{ty}");
    }
}

#[test]
fn listeners_in_and_around_a_shadow_tree_hear_a_click_inside_it() {
    let e = env(
        "<!DOCTYPE html><body style='margin:0'><div id=host style='position:absolute;left:0;top:0;width:60px;height:40px'>\
         </div></body>",
    );
    let r = &e.main;
    eval(
        r,
        "window.log = []; window.note = (e) => log.push(e.type + '@' + e.currentTarget.id + '>' + e.target.id); \
         window.sr = host.attachShadow({mode: 'closed'}); \
         sr.innerHTML = '<button id=inner style=\"position:absolute;left:0;top:0;width:50px;height:30px;border:0\">x</button>'; \
         host.getBoundingClientRect()",
    );
    // Nothing listens: the click still focuses the button in the tree.
    click(&e.page);
    assert_eq!(eval(r, "[document.activeElement.id, sr.activeElement.id].join()"), "host,inner");
    // A listener inside the tree only, then one on the host only (it sees
    // the event retargeted), each for a different event of the click.
    eval(r, "sr.getElementById('inner').addEventListener('mousedown', note)");
    click(&e.page);
    assert_eq!(eval(r, "log.join()"), "mousedown@inner>inner");
    eval(r, "log.length = 0; host.addEventListener('mouseup', note); host.addEventListener('click', note)");
    click(&e.page);
    assert_eq!(eval(r, "log.join()"), "mousedown@inner>inner,mouseup@host>host,click@host>host");
    // The tree's listeners go with its nodes' listeners removed.
    eval(
        r,
        "log.length = 0; sr.getElementById('inner').removeEventListener('mousedown', note); \
         host.removeEventListener('mouseup', note); host.removeEventListener('click', note)",
    );
    click(&e.page);
    assert_eq!(eval(r, "log.join()"), "");
}

#[test]
fn unheard_moves_and_boundary_events_keep_hover_state_and_later_listeners_hear_them() {
    let e = click_env();
    let r = &e.main;
    eval(r, "b.getBoundingClientRect()");
    mouse(&e.page, input::MouseKind::Moved, 10.0, 10.0, 0);
    assert_eq!(eval(r, "b.matches(':hover')"), "true");
    eval(
        r,
        "for (const ty of ['mouseover', 'mouseout', 'mouseenter', 'mouseleave', 'pointerover', 'pointerout', \
           'pointerenter', 'pointerleave', 'mousemove', 'pointermove']) b.addEventListener(ty, note);",
    );
    mouse(&e.page, input::MouseKind::Moved, 12.0, 10.0, 0);
    mouse(&e.page, input::MouseKind::Moved, 300.0, 300.0, 0);
    mouse(&e.page, input::MouseKind::Moved, 10.0, 10.0, 0);
    assert_eq!(
        eval(r, "log.join()"),
        "pointermove@b,mousemove@b,pointerout@b,pointerleave@b,mouseout@b,mouseleave@b,\
         pointerover@b,pointerenter@b,mouseover@b,mouseenter@b,pointermove@b,mousemove@b"
    );
    assert_eq!(eval(r, "b.matches(':hover')"), "true");
}

#[test]
fn a_disabled_style_element_sheet_applies_nothing() {
    // `CSSStyleSheet.disabled` on a `<style>`'s sheet, in a shadow tree and in the document: the
    // computed style no longer has the sheet's declarations (WPT css-shadow `shadow-disabled-sheet-001`).
    let e = env("<body><style>#t{width:30px}</style><div id=t></div><div id=host></div></body>");
    let r = &e.main;
    eval(
        r,
        "window.sr = host.attachShadow({mode: 'open'}); \
         sr.innerHTML = '<style>div{width:100px}</style><style>div{width:50px}</style><div id=in></div>';",
    );
    assert_eq!(eval(r, "getComputedStyle(sr.getElementById('in')).width"), "50px");
    eval(r, "sr.styleSheets[1].disabled = true");
    assert_eq!(
        eval(r, "[sr.styleSheets[1].disabled, getComputedStyle(sr.getElementById('in')).width].join()"),
        "true,100px"
    );
    eval(r, "document.styleSheets[0].disabled = true");
    // The used width of the block now (not the sheet's 30px).
    assert_eq!(
        eval(r, "[document.styleSheets[0].disabled, getComputedStyle(t).width === '30px'].join()"),
        "true,false"
    );
    eval(r, "document.styleSheets[0].disabled = false");
    assert_eq!(eval(r, "getComputedStyle(t).width"), "30px");
}

#[test]
fn manually_assigned_slots() {
    // `slotAssignment: "manual"`: `HTMLSlotElement.assign()` decides the assigned nodes, in its order;
    // a node is in one slot at most; only children of the host count; changes fire `slotchange`.
    let e = env("<body><div id=host><b id=a>a</b><i id=b>b</i></div><p id=other>o</p></body>");
    let r = &e.main;
    eval(
        r,
        "window.sr = host.attachShadow({mode: 'open', slotAssignment: 'manual'}); \
         sr.innerHTML = '<slot id=s1></slot><slot id=s2></slot>'; \
         window.s1 = sr.getElementById('s1'); window.s2 = sr.getElementById('s2'); \
         window.log = []; sr.addEventListener('slotchange', e => log.push(e.target.id));",
    );
    assert_eq!(eval(r, "s1.assignedNodes().length + ':' + (a.assignedSlot === null)"), "0:true");
    eval(r, "s1.assign(b, a, other)");
    assert_eq!(eval(r, "s1.assignedNodes().map(n => n.id).join() + ':' + a.assignedSlot.id"), "b,a:s1");
    eval(r, "s2.assign(a)");
    assert_eq!(
        eval(r, "s1.assignedNodes().map(n => n.id).join() + '|' + s2.assignedNodes().map(n => n.id).join()"),
        "b|a"
    );
    // `slotchange` (in the mutation observer microtask): both slots changed.
    assert_eq!(eval(r, "[...new Set(log)].sort().join()"), "s1,s2");
}

/// A right or middle press activates `:active` until its release, as a left one does (Chrome;
/// WPT css/selectors `active-observable`).
#[test]
fn a_non_primary_press_activates_active() {
    let e = click_env();
    let r = &e.main;
    eval(r, "window.held = []; b.addEventListener('pointerdown', () => held.push(b.matches(':active')));");
    for (button, bits) in [(input::MouseButton::Right, 2), (input::MouseButton::Middle, 4)] {
        let m = |buttons| input::MouseInput { x: 10.0, y: 10.0, button, buttons, click_count: 1, ..Default::default() };
        input::dispatch_mouse(&e.page, input::MouseKind::Pressed, &m(bits)).unwrap();
        assert_eq!(eval(r, "b.matches(':active')"), "true", "{button:?} press");
        input::dispatch_mouse(&e.page, input::MouseKind::Released, &m(0)).unwrap();
        assert_eq!(eval(r, "b.matches(':active')"), "false", "{button:?} release");
    }
    assert_eq!(eval(r, "held.join()"), "true,true");
}

/// The boundary events of a move observe the new `:hover` state (Chrome; WPT css/selectors
/// `hover-boundary-events`).
#[test]
fn boundary_events_see_the_new_hover_state() {
    let e = env("<!DOCTYPE html><style>body{margin:0} div{position:absolute;width:50px;height:50px;top:0}</style>\
         <body><div id=a style='left:0'></div><div id=b style='left:100px'></div></body>");
    let r = &e.main;
    mouse(&e.page, input::MouseKind::Moved, 10.0, 10.0, 0);
    eval(
        r,
        "window.seen = []; const note = ev => seen.push(ev.type + ':' + a.matches(':hover') + ',' + b.matches(':hover')); \
         for (const t of ['mouseout', 'mouseleave', 'pointerout']) a.addEventListener(t, note); \
         for (const t of ['mouseover', 'mouseenter', 'pointerover']) b.addEventListener(t, note);",
    );
    mouse(&e.page, input::MouseKind::Moved, 110.0, 10.0, 0);
    assert_eq!(
        eval(r, "seen.join(' ')"),
        "pointerout:false,true pointerover:false,true mouseout:false,true mouseleave:false,true mouseover:false,true mouseenter:false,true"
    );
}
