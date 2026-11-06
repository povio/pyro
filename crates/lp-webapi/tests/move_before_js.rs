//! `Node.moveBefore()` through the bindings on the real JavaScriptCore: the
//! state a move preserves (focus, modal dialogs, popovers, selection), the
//! custom element reactions it queues, and the features its tests lean on
//! (`:modal`, `:popover-open`, `selectionDirection`, nonce hiding).

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

const PARENTS: &str = "<!doctype html><html><body><section id=a></section><section id=b></section></body></html>";

#[test]
fn pre_move_validity_allows_disconnected_trees_with_one_root() {
    let e = env(PARENTS);
    let r = &e.main;
    assert_eq!(
        eval(
            r,
            "const origin = document.createElement('div'), dest = origin.appendChild(document.createElement('div')); \
             const p = origin.appendChild(document.createElement('p')); dest.moveBefore(p, null); dest.firstChild === p"
        ),
        "true"
    );
    assert_eq!(
        eval(
            r,
            "const host = origin.appendChild(document.createElement('div')); \
             const inner = host.attachShadow({mode: 'closed'}).appendChild(document.createElement('div')); \
             inner.moveBefore(p, null); inner.firstChild === p"
        ),
        "true"
    );
    let name = |src: &str| eval(r, &format!("try {{ {src}; 'no error' }} catch (e) {{ e.name }}"));
    assert_eq!(name("document.createElement('div').moveBefore(p, null)"), "HierarchyRequestError");
    assert_eq!(name("a.moveBefore(p, null)"), "HierarchyRequestError");
    assert_eq!(name("document.createElement('div').moveBefore(a, null)"), "HierarchyRequestError");
    assert_eq!(name("p.moveBefore(origin, null)"), "HierarchyRequestError");
    assert_eq!(name("a.moveBefore(b, document.createElement('i'))"), "NotFoundError");
    // A disconnected iframe moves without a browsing context to keep.
    assert_eq!(
        name("const f = dest.appendChild(document.createElement('iframe')); origin.moveBefore(f, null)"),
        "no error"
    );
}

#[test]
fn custom_elements_get_connected_and_disconnected_reactions_when_the_call_returns() {
    let e = env(PARENTS);
    let r = &e.main;
    eval(
        r,
        "var log = []; \
         customElements.define('x-both', class extends HTMLElement { \
           connectedCallback() { log.push('connected:' + this.isConnected); } \
           disconnectedCallback() { log.push('disconnected:' + this.isConnected); } }); \
         var el = document.createElement('x-both');",
    );
    // Synchronously, not at a microtask.
    assert_eq!(eval(r, "a.append(el); log.join()"), "connected:true");
    assert_eq!(eval(r, "log = []; el.remove(); log.join()"), "disconnected:false");
    // An insertion of a connected element: both reactions run once it is
    // in its new place.
    assert_eq!(
        eval(r, "log = []; a.append(el); b.append(el); log.join()"),
        "connected:true,disconnected:true,connected:true"
    );
    // Nothing for a tree that is not connected.
    assert_eq!(eval(r, "log = []; el.remove(); log = []; document.createElement('div').append(el); log.join()"), "");
    // Callbacks are those the prototype had at definition.
    assert_eq!(
        eval(r, "customElements.get('x-both').prototype.connectedCallback = () => log.push('late'); a.append(el); log.join()"),
        "connected:true"
    );
    // A callback's own DOM calls queue reactions that run before the outer call returns.
    assert_eq!(
        eval(
            r,
            "log = []; customElements.define('x-nest', class extends HTMLElement { \
               connectedCallback() { log.push('nest'); if (!this.firstChild) this.append(document.createElement('x-both')); } }); \
             b.append(document.createElement('x-nest')); log.join()"
        ),
        "nest,connected:true"
    );
    assert_eq!(
        eval(r, "try { customElements.define('x-bad', class extends HTMLElement { get connectedCallback() { return 1; } }); 'no' } catch (e) { e.name }"),
        "TypeError"
    );
}

#[test]
fn a_move_runs_connected_move_callback_or_disconnected_then_connected() {
    let e = env(PARENTS);
    let r = &e.main;
    eval(
        r,
        "var log = []; \
         customElements.define('x-move', class extends HTMLElement { \
           connectedMoveCallback() { log.push('move'); } connectedCallback() { log.push('connected'); } \
           disconnectedCallback() { log.push('disconnected'); } }); \
         const pair = name => class extends HTMLElement { \
           connectedCallback() { log.push(name + ' connected ' + this.isConnected); } \
           disconnectedCallback() { log.push(name + ' disconnected ' + this.isConnected); } }; \
         customElements.define('x-outer', pair('outer')); customElements.define('x-inner', pair('inner')); \
         customElements.define('x-only-d', class extends HTMLElement { disconnectedCallback() { log.push('d'); } }); \
         customElements.define('x-only-c', class extends HTMLElement { connectedCallback() { log.push('c'); } }); \
         customElements.define('x-none', class extends HTMLElement {});",
    );
    assert_eq!(
        eval(r, "var m = a.appendChild(document.createElement('x-move')); log = []; b.moveBefore(m, null); log.join()"),
        "move"
    );
    // The fallback runs per element in tree order, both callbacks while connected.
    assert_eq!(
        eval(
            r,
            "var outer = document.createElement('x-outer'); outer.append(document.createElement('x-inner')); a.append(outer); \
             log = []; b.moveBefore(outer, null); log.join()"
        ),
        "outer disconnected true,outer connected true,inner disconnected true,inner connected true"
    );
    assert_eq!(
        eval(
            r,
            "for (const n of ['x-only-d', 'x-only-c', 'x-none']) a.append(document.createElement(n)); log = []; \
             for (const n of ['x-only-d', 'x-only-c', 'x-none']) b.moveBefore(a.querySelector(n), null); log.join()"
        ),
        "d,c"
    );
    // A custom element inside a shadow tree of the moved node.
    assert_eq!(
        eval(
            r,
            "const host = a.appendChild(document.createElement('div')); \
             host.attachShadow({mode: 'open'}).append(document.createElement('x-move')); log = []; \
             b.moveBefore(host, null); log.join()"
        ),
        "move"
    );
    // A move inside a disconnected tree runs nothing.
    assert_eq!(
        eval(
            r,
            "const top = document.createElement('div'), into = top.appendChild(document.createElement('div')); \
             const lone = top.appendChild(document.createElement('x-move')); log = []; into.moveBefore(lone, null); log.join()"
        ),
        ""
    );
}

#[test]
fn the_focused_element_stays_focused_and_no_focus_event_fires() {
    let e = env("<!doctype html><html><body><section id=a><button id=btn>B</button></section><section id=b></section>\
         <section id=inert inert></section><section id=hidden hidden></section>\
         <style>#gone:has(button) { display: none }</style><section id=gone></section></body></html>");
    let r = &e.main;
    eval(
        r,
        "var events = []; for (const t of ['focus', 'blur', 'focusin', 'focusout']) \
           document.addEventListener(t, e => events.push(e.type), true); \
         var within = () => Array.from(document.querySelectorAll(':focus-within'), n => n.id || n.nodeName).join(); \
         btn.focus();",
    );
    assert_eq!(eval(r, "events.join()"), "focus,focusin");
    assert_eq!(
        eval(r, "b.moveBefore(btn, null); [document.activeElement === btn, within()].join('|')"),
        "true|HTML,BODY,b,btn"
    );
    assert_eq!(
        eval(r, "a.moveBefore(b, null); [document.activeElement === btn, within()].join('|')"),
        "true|HTML,BODY,a,b,btn"
    );
    assert_eq!(eval(r, "b.moveBefore(btn, null); document.activeElement === btn"), "true", "same parent");
    assert_eq!(eval(r, "events.join()"), "focus,focusin");
    // Still focusable where it is: the fixup leaves it alone.
    input::run_focus_fixup(&e.page);
    assert_eq!(eval(r, "document.activeElement === btn"), "true");
    // An insertion (not a move) drops focus as before.
    assert_eq!(eval(r, "a.insertBefore(btn, null); document.activeElement === document.body"), "true");
    for parent in ["inert", "hidden", "gone"] {
        eval(r, "a.append(btn); btn.focus(); events = []");
        assert_eq!(
            eval(r, &format!("{parent}.moveBefore(btn, null); [document.activeElement === btn, events.length].join()")),
            "true,0",
            "{parent}: focus is kept through the move"
        );
        input::run_focus_fixup(&e.page);
        assert_eq!(
            eval(r, "[document.activeElement === document.body, events.join(), within()].join('|')"),
            "true|blur,focusout|",
            "{parent}: the fixup blurs an element that is no longer focusable"
        );
    }
}

#[test]
fn a_text_selection_and_its_direction_survive_a_move() {
    let e = env("<!doctype html><html><body><section id=a><input id=i value=0123456789></section><section id=b></section></body></html>");
    let r = &e.main;
    let state = "[i.selectionStart, i.selectionEnd, i.selectionDirection].join()";
    assert_eq!(eval(r, &format!("i.focus(); i.setSelectionRange(2, 7, 'backward'); {state}")), "2,7,backward");
    assert_eq!(
        eval(r, &format!("b.moveBefore(i, null); [document.activeElement === i, {state}].join()")),
        "true,2,7,backward"
    );
    assert_eq!(eval(r, &format!("i.selectionStart = 3; {state}")), "3,7,backward");
    assert_eq!(eval(r, &format!("i.selectionDirection = 'forward'; {state}")), "3,7,forward");
    assert_eq!(eval(r, &format!("i.setSelectionRange(1, 2); {state}")), "1,2,none");
    assert_eq!(eval(r, &format!("i.setSelectionRange(1, 2, 'sideways'); {state}")), "1,2,none");
    assert_eq!(eval(r, &format!("i.selectionDirection = 'backward'; i.select(); {state}")), "0,10,none");
    assert_eq!(eval(r, "const c = document.createElement('input'); c.type = 'checkbox'; c.selectionDirection"), "null");
    assert_eq!(eval(r, "try { c.selectionDirection = 'forward'; 'no' } catch (e) { e.name }"), "InvalidStateError");
}

#[test]
fn modal_dialogs_and_popovers_stay_open_across_a_move() {
    let e =
        env("<!doctype html><html><body><section id=a><dialog id=dlg></dialog><div id=pop popover>P</div></section>\
         <section id=b></section></body></html>");
    let r = &e.main;
    assert_eq!(eval(r, "[dlg.matches(':modal'), pop.matches(':popover-open')].join()"), "false,false");
    assert_eq!(eval(r, "dlg.showModal(); pop.showPopover(); [dlg.matches(':modal'), dlg.open, pop.matches(':popover-open')].join()"), "true,true,true");
    assert_eq!(eval(r, "getComputedStyle(pop).display"), "block");
    assert_eq!(
        eval(r, "b.moveBefore(dlg, null); b.moveBefore(pop, null); [dlg.matches(':modal'), document.querySelector(':popover-open') === pop].join()"),
        "true,true"
    );
    // Removal closes both; the dialog stays open as a non-modal one.
    assert_eq!(
        eval(r, "a.append(dlg, pop); [dlg.matches(':modal'), dlg.open, pop.matches(':popover-open'), getComputedStyle(pop).display].join()"),
        "false,true,false,none"
    );
    assert_eq!(
        eval(r, "try { dlg.showModal(); 'no' } catch (e) { e.name }"),
        "InvalidStateError",
        "open and not modal"
    );
    assert_eq!(
        eval(r, "dlg.close(); dlg.showModal(); dlg.showModal(); dlg.close(); [dlg.matches(':modal'), dlg.open].join()"),
        "false,false"
    );
    assert_eq!(
        eval(r, "try { document.createElement('dialog').showModal(); 'no' } catch (e) { e.name }"),
        "InvalidStateError"
    );
    // Popover validity and toggling.
    assert_eq!(eval(r, "try { a.showPopover(); 'no' } catch (e) { e.name }"), "NotSupportedError");
    assert_eq!(
        eval(r, "const off = document.createElement('div'); off.popover = 'auto'; try { off.showPopover(); 'no' } catch (e) { e.name }"),
        "InvalidStateError"
    );
    assert_eq!(
        eval(
            r,
            "[pop.togglePopover(), pop.togglePopover(true), pop.togglePopover(false), pop.togglePopover({force: false}), \
              pop.matches(':popover-open')].join()"
        ),
        "true,true,false,false,false"
    );
    assert_eq!(
        eval(
            r,
            "pop.showPopover(); pop.showPopover(); pop.hidePopover(); pop.hidePopover(); pop.matches(':popover-open')"
        ),
        "false"
    );
}

#[test]
fn slotchange_fires_for_fallback_content_and_moved_slots() {
    let e = env(PARENTS);
    let r = &e.main;
    eval(
        r,
        "var fired = 0; var host = a.appendChild(document.createElement('div')); \
         var root = host.attachShadow({mode: 'open'}); var slot = root.appendChild(document.createElement('slot')); \
         slot.name = 'content'; slot.addEventListener('slotchange', () => fired++); \
         var fallback = root.appendChild(document.createElement('p')); slot.moveBefore(fallback, null);",
    );
    assert_eq!(eval(r, "fired"), "1", "fallback content moved into the slot");
    eval(r, "root.moveBefore(fallback, null)");
    assert_eq!(eval(r, "fired"), "2", "fallback content moved out of the slot");
    eval(r, "var lit = host.appendChild(document.createElement('p')); lit.slot = 'content';");
    assert_eq!(eval(r, "[fired, slot.assignedNodes().length].join()"), "3,1");
    eval(r, "document.body.moveBefore(slot, null)");
    assert_eq!(eval(r, "[fired, slot.assignedNodes().length].join()"), "4,0", "the slot left its shadow tree");
    eval(r, "root.moveBefore(slot, null)");
    assert_eq!(eval(r, "[fired, slot.assignedNodes().length].join()"), "5,1", "the slot came back");
}

#[test]
fn a_default_selected_option_keeps_its_selectedness_outside_the_select() {
    let e = env(
        "<!doctype html><html><body><select id=s><option id=oa>A</option><optgroup><option id=ob>B</option></optgroup>\
         <option id=oc>C</option></select></body></html>",
    );
    let r = &e.main;
    let state = "[oa.selected, ob.selected, oc.selected].join()";
    assert_eq!(eval(r, state), "true,false,false");
    assert_eq!(eval(r, &format!("document.body.moveBefore(oa, null); {state}")), "true,true,false");
    assert_eq!(eval(r, &format!("document.body.moveBefore(ob, null); {state}")), "true,true,true");
    assert_eq!(eval(r, &format!("s.moveBefore(oa, oc); {state}")), "true,true,false");
}

#[test]
fn connecting_to_a_document_with_a_csp_header_hides_the_nonce_and_a_move_does_not() {
    let e = env(PARENTS);
    let r = &e.main;
    let state = "[div.getAttribute('nonce'), div.nonce].join()";
    // Without a header-delivered policy the attribute stays.
    assert_eq!(
        eval(
            r,
            &format!(
                "var div = document.createElement('div'); div.setAttribute('nonce', 'abc'); a.append(div); {state}"
            )
        ),
        "abc,abc"
    );
    e.page.dom.borrow_mut().set_header_csp(e.page.document(), true).unwrap();
    assert_eq!(eval(r, &format!("b.moveBefore(div, null); {state}")), "abc,abc", "a move connects nothing");
    assert_eq!(eval(r, &format!("a.insertBefore(div, null); {state}")), ",abc", "an insertion does");
    assert_eq!(eval(r, &format!("div.setAttribute('nonce', 'new'); {state}")), "new,new");
    assert_eq!(
        eval(r, "var d2 = document.createElement('div'); d2.setAttribute('nonce', 'n2'); document.createElement('p').append(d2); d2.getAttribute('nonce')"),
        "n2",
        "a disconnected tree is not browsing-context connected"
    );
}

#[test]
fn a_picture_source_is_the_current_source_of_its_image() {
    let e =
        env("<!doctype html><html><body><picture id=pic><source id=src srcset='/green.png' media='(min-width: 10px)'>\
         <img id=img src='/red.png'></picture></body></html>");
    let r = &e.main;
    assert_eq!(eval(r, "img.currentSrc"), "http://localhost:5173/green.png");
    assert_eq!(eval(r, "document.body.moveBefore(src, null); img.currentSrc"), "http://localhost:5173/red.png");
    assert_eq!(eval(r, "pic.moveBefore(src, img); img.currentSrc"), "http://localhost:5173/green.png");
    assert_eq!(eval(r, "src.media = '(min-width: 99999px)'; img.currentSrc"), "http://localhost:5173/red.png");
}
