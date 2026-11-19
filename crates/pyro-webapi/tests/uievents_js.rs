//! UI Events bindings exercised from JavaScript on the real JavaScriptCore:
//! the legacy `initMouseEvent`/`initKeyboardEvent` initializers,
//! `UIEvent.pseudoTarget`, `document.execCommand('insertText')` and the
//! relatedTarget of focus events that cross into a child document.

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

#[test]
fn init_mouse_event_sets_every_field_and_toggles_a_checkbox() {
    let e = env("<body><input type=checkbox id=cb><div id=rel></div></body>");
    let r = &e.main;
    eval(
        r,
        "const e = document.createEvent('MouseEvent'); \
         e.initMouseEvent('click', false, true, window, 1, 2, 3, 4, 5, true, false, true, false, 2, document.getElementById('rel'));",
    );
    assert_eq!(
        eval(
            r,
            "[e.type, e.bubbles, e.cancelable, e.view === window, e.detail, e.screenX, e.screenY, e.clientX, e.clientY, \
              e.ctrlKey, e.altKey, e.shiftKey, e.metaKey, e.button, e.relatedTarget === rel, e.isTrusted].join()"
        ),
        "click,false,true,true,1,2,3,4,5,true,false,true,false,2,true,false"
    );
    // Missing optional arguments default; a non-Window view is a TypeError.
    assert_eq!(
        eval(r, "const m = document.createEvent('MouseEvent'); m.initMouseEvent('mousedown'); [m.bubbles, m.view, m.detail, m.button, m.relatedTarget].join()"),
        "false,,0,0,"
    );
    assert!(eval(r, "m.initMouseEvent('x', false, false, document.body)").starts_with("THROW TypeError"));
    // A synthetic non-bubbling click still runs the checkbox's activation
    // behaviour, and re-initializing during dispatch is a no-op.
    assert_eq!(
        eval(
            r,
            "const cb = document.getElementById('cb'); let seen = ''; \
             cb.addEventListener('click', ev => { ev.initMouseEvent('foo', true, true, window); seen = ev.type + ':' + ev.bubbles; }); \
             cb.dispatchEvent(e); [cb.checked, seen, e.type, e.bubbles].join()"
        ),
        "true,click:false,click,false"
    );
}

#[test]
fn init_keyboard_event_sets_key_location_and_modifiers() {
    let e = env("<body></body>");
    let r = &e.main;
    assert_eq!(
        eval(
            r,
            "const k = document.createEvent('KeyboardEvent'); \
             k.initKeyboardEvent('keydown', true, true, window, 'a', 1, true, false, false, true); \
             [k.type, k.bubbles, k.cancelable, k.view === window, k.key, k.location, k.ctrlKey, k.altKey, k.shiftKey, k.metaKey, k.detail].join()"
        ),
        "keydown,true,true,true,a,1,true,false,false,true,0"
    );
    assert_eq!(
        eval(r, "const k2 = document.createEvent('KeyboardEvent'); k2.initKeyboardEvent('keyup'); JSON.stringify([k2.key, k2.location, k2.view, k2.bubbles, k2.ctrlKey])"),
        "[\"\",0,null,false,false]"
    );
    assert!(eval(r, "k2.initKeyboardEvent()").starts_with("THROW TypeError"));
}

#[test]
fn pseudo_target_is_a_uievent_prototype_getter() {
    let e = env("<body></body>");
    let r = &e.main;
    assert_eq!(
        eval(
            r,
            "const d = Object.getOwnPropertyDescriptor(UIEvent.prototype, 'pseudoTarget'); \
             [typeof d.get, Object.getOwnPropertyDescriptor(MouseEvent.prototype, 'pseudoTarget') === undefined, \
              'pseudoTarget' in MouseEvent.prototype, new MouseEvent('click').pseudoTarget, new KeyboardEvent('keydown').pseudoTarget, \
              'pseudoTarget' in Event.prototype].join()"
        ),
        "function,true,true,,,false"
    );
}

#[test]
fn exec_command_insert_text_types_into_the_focused_editable() {
    let e = env("<body><input id=i value=x><textarea id=ta>y</textarea><div contenteditable id=ce></div><input type=checkbox id=cb></body>");
    let r = &e.main;
    eval(
        r,
        "window.log = []; for (const id of ['i', 'ta', 'ce']) { const el = document.getElementById(id); \
           for (const t of ['textInput', 'beforeinput', 'input']) el.addEventListener(t, ev => log.push(id + ':' + t + ':' + ev.data + ':' + ev.inputType + ':' + ev.isTrusted)); }",
    );
    // Nothing focused: not handled. Unknown commands: not handled.
    assert_eq!(
        eval(r, "[document.execCommand('insertText', false, 'a'), document.execCommand('bold'), log.length].join()"),
        "false,false,0"
    );
    // A fresh control's caret starts at the start of its value (as in Chromium); execCommand types there.
    assert_eq!(eval(r, "i.focus(); document.execCommand('insertText', false, 'a') + ':' + i.value"), "true:ax");
    assert_eq!(eval(r, "ta.focus(); document.execCommand('InsertText', false, 'b') + ':' + ta.value"), "true:by");
    assert_eq!(eval(r, "ce.focus(); document.execCommand('insertText', false, 'c') + ':' + ce.textContent"), "true:c");
    assert_eq!(
        eval(r, "log.join(' ')"),
        "i:beforeinput:a:insertText:true i:input:a:insertText:true \
         ta:beforeinput:b:insertText:true ta:input:b:insertText:true \
         ce:beforeinput:c:insertText:true ce:input:c:insertText:true"
    );
    // A canceled beforeinput inserts nothing and fires no input.
    assert_eq!(
        eval(
            r,
            "log.length = 0; ce.addEventListener('beforeinput', ev => ev.preventDefault(), { once: true }); \
             [document.execCommand('insertText', false, 'd'), ce.textContent, log.join()].join('|')"
        ),
        "true|c|ce:beforeinput:d:insertText:true"
    );
    assert_eq!(
        eval(
            r,
            "i.addEventListener('beforeinput', ev => ev.preventDefault(), { once: true }); i.focus(); \
             [document.execCommand('insertText', false, 'd'), i.value].join('|')"
        ),
        "true|xa"
    );
    // A focused text control wins over the selection still inside the host;
    // with a checkbox focused that selection is the target, as in Chromium.
    assert_eq!(
        eval(r, "i.focus(); document.execCommand('insertText', false, 'e'); i.value + ':' + ce.textContent"),
        "xae:c"
    );
    assert_eq!(
        eval(
            r,
            "cb.focus(); [document.execCommand('insertText', false, 'z'), cb.checked, i.value, ce.textContent].join()"
        ),
        "true,false,xae,cz"
    );
}

#[test]
fn focus_crossing_a_frame_owner_has_no_related_target() {
    let e = env("<body><input id=a><iframe id=f></iframe><input id=b></body>");
    let r = &e.main;
    eval(
        r,
        "window.evs = []; for (const el of [a, b]) for (const t of ['focus', 'blur', 'focusin', 'focusout']) \
           el.addEventListener(t, ev => evs.push(t + '@' + ev.target.id + ':' + (ev.relatedTarget ? ev.relatedTarget.id : null)));",
    );
    // Same-document moves keep the related element; moves into or out of the
    // frame owner (another document's focus) report null, as in Chromium.
    assert_eq!(
        eval(r, "a.focus(); b.focus(); evs.join()"),
        "focus@a:null,focusin@a:null,blur@a:b,focusout@a:b,focus@b:a,focusin@b:a"
    );
    assert_eq!(eval(r, "evs.length = 0; f.focus(); evs.join()"), "blur@b:null,focusout@b:null");
    assert_eq!(eval(r, "evs.length = 0; a.focus(); evs.join()"), "focus@a:null,focusin@a:null");
}
