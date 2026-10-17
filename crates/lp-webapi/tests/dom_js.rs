//! lp-webapi bindings exercised from JavaScript on the real system
//! JavaScriptCore (main world + an isolated world sharing one DOM).

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
    engine: Engine,
    page: Rc<PageDom>,
    main: Realm,
}

fn env(html: &str) -> Env {
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
    let main = new_realm(&engine, window_class, &page, "");
    Env { engine, page, main }
}

fn new_realm(engine: &Engine, class: lp_js::ClassId, page: &Rc<PageDom>, name: &str) -> Realm {
    let realm = engine
        .create_realm(RealmOptions { name: name.into(), global_class: Some(class), origin: None })
        .expect("realm");
    let key = {
        let existing = page.window_target();
        existing.unwrap_or_else(|| page.create_window_target(page.document()))
    };
    realm
        .set_global_data::<TestWindow>(TestWindowData {
            page: page.clone(),
            target: EventTargetData::window(page, key),
        })
        .unwrap();
    let s = realm.scope().unwrap();
    install(&s, page.clone()).unwrap();
    drop(s);
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
fn document_tree_and_identity() {
    let e = env("<!doctype html><html><head><title> Hi  there </title></head><body><div id=app class='a b'><p>one</p><p>two</p></div></body></html>");
    let r = &e.main;
    assert_eq!(eval(r, "document.title"), "Hi there");
    assert_eq!(eval(r, "document.querySelectorAll('p').length"), "2");
    assert_eq!(eval(r, "document.getElementById('app') === document.querySelector('#app')"), "true");
    assert_eq!(eval(r, "const app = document.getElementById('app'); app instanceof HTMLDivElement && app instanceof HTMLElement && app instanceof Element && app instanceof Node && app instanceof EventTarget"), "true");
    assert_eq!(eval(r, "app.firstChild.parentNode === app && app.childNodes[1] === app.lastChild"), "true");
    assert_eq!(eval(r, "app.childNodes === app.childNodes && app.children === app.children"), "true");
    assert_eq!(eval(r, "Object.prototype.toString.call(app)"), "[object HTMLDivElement]");
    assert_eq!(eval(r, "app.classList.contains('b') && app.classList.length"), "2");
    assert_eq!(eval(r, "app.classList.add('c'); app.classList.toggle('a'); app.className"), "b c");
    assert_eq!(eval(r, "app.dataset.fooBar = 'x'; app.getAttribute('data-foo-bar') + ':' + app.dataset.fooBar"), "x:x");
    assert_eq!(eval(r, "app.style.backgroundColor = 'red'; app.getAttribute('style')"), "background-color: red;");
    assert_eq!(
        eval(r, "const s = document.createElement('span'); s.textContent = 'new'; app.append(s, 'txt'); app.innerHTML"),
        "<p>one</p><p>two</p><span>new</span>txt"
    );
    assert_eq!(eval(r, "app.querySelector(':scope > span').closest('div') === app"), "true");
    assert_eq!(
        eval(r, "app.innerHTML = '<b>x</b><i>y</i>'; app.children.length + ':' + app.firstElementChild.tagName"),
        "2:B"
    );
    assert_eq!(
        eval(
            r,
            "try { app.appendChild(document) } catch (e) { e.name + ':' + e.code + ':' + (e instanceof DOMException) }"
        ),
        "HierarchyRequestError:3:true"
    );
    assert_eq!(eval(r, "try { document.querySelector('p[') } catch (e) { e.name }"), "SyntaxError");
    assert_eq!(eval(r, "const t = document.createTreeWalker(app, NodeFilter.SHOW_ELEMENT); let n=[]; while (t.nextNode()) n.push(t.currentNode.tagName); n.join()"), "B,I");
    assert_eq!(eval(r, "new Text('abc').data + document.createComment('c').nodeType"), "abc8");
    assert_eq!(
        eval(r, "document.body.contains(app) && app.isConnected && !document.createElement('x').isConnected"),
        "true"
    );
}

#[test]
fn events_listeners_and_activation() {
    let e = env("<body><div id=outer><button id=b>go</button></div><label for=c>lbl</label><input id=c type=checkbox><form id=f><input name=q value=1><button id=s>send</button></form></body>");
    let r = &e.main;
    let out = eval(
        r,
        r#"
        const log = [];
        const outer = document.getElementById('outer'), b = document.getElementById('b');
        outer.addEventListener('click', e => log.push('outer-capture:' + e.eventPhase), true);
        outer.addEventListener('click', function (e) { log.push('outer-bubble:' + (this === outer) + ':' + (e.target === b)); });
        b.addEventListener('click', e => log.push('b:' + e.isTrusted), { once: true });
        window.addEventListener('click', e => log.push('window:' + (e.currentTarget === window)));
        b.click(); b.click();
        log.join('|')"#,
    );
    assert_eq!(
        out,
        "outer-capture:1|b:false|outer-bubble:true:true|window:true|outer-capture:1|outer-bubble:true:true|window:true"
    );
    assert_eq!(eval(r, "let n = 0; const h = () => n++; b.addEventListener('x', h); b.addEventListener('x', h); b.dispatchEvent(new Event('x')); b.removeEventListener('x', h); b.dispatchEvent(new Event('x')); n"), "1");
    assert_eq!(eval(r, "const ev = new CustomEvent('y', {detail: {k: 1}, bubbles: true}); let got; document.addEventListener('y', e => got = e.detail.k); b.dispatchEvent(ev); got"), "1");
    assert_eq!(
        eval(r, "const ac = new AbortController(); let cnt = 0; b.addEventListener('z', () => cnt++, {signal: ac.signal}); b.dispatchEvent(new Event('z')); ac.abort(); b.dispatchEvent(new Event('z')); cnt + ':' + ac.signal.aborted"),
        "1:true"
    );
    // Checkbox activation via label click, with input/change events.
    let out = eval(
        r,
        r#"const c = document.getElementById('c'); const seen = [];
        c.addEventListener('input', () => seen.push('input'));
        c.addEventListener('change', () => seen.push('change:' + c.checked));
        document.querySelector('label').click();
        seen.join(',') + '|' + c.checked + '|' + document.activeElement.id"#,
    );
    assert_eq!(out, "input,change:true|true|c");
    // Canceled click restores the checkbox.
    assert_eq!(eval(r, "c.addEventListener('click', e => e.preventDefault()); c.click(); c.checked"), "true");
    // Submit button fires submit; preventDefault blocks navigation.
    assert_eq!(eval(r, "let sub = 0; document.getElementById('f').addEventListener('submit', e => { sub++; e.preventDefault(); }); document.getElementById('s').click(); sub"), "1");
}

#[test]
fn keyboard_and_focus_through_native_input() {
    let e = env("<body><input id=t value=abc><textarea id=ta></textarea></body>");
    let r = &e.main;
    eval(
        r,
        "window.evs = []; const t = document.getElementById('t'); for (const k of ['focus','keydown','keypress','beforeinput','input','keyup','change','blur']) t.addEventListener(k, e => evs.push(k + (e.data ? ':' + e.data : ''))); t.focus();",
    );
    let page = e.page.clone();
    // Playwright fill: select all, then insertText.
    let t = {
        let d = page.dom.borrow();
        d.get_element_by_id(page.document(), "t").unwrap().unwrap()
    };
    page.dom.borrow_mut().set_selection_range(t, 0, 3).unwrap();
    input::insert_text(&page, "xy");
    let k =
        input::KeyInput { key: "z".into(), code: "KeyZ".into(), text: "z".into(), key_code: 90, ..Default::default() };
    input::dispatch_key(&page, input::KeyKind::KeyDown, &k);
    input::dispatch_key(&page, input::KeyKind::KeyUp, &k);
    let bs = input::KeyInput { key: "Backspace".into(), code: "Backspace".into(), key_code: 8, ..Default::default() };
    input::dispatch_key(&page, input::KeyKind::RawKeyDown, &bs);
    assert_eq!(eval(r, "t.value"), "xy");
    assert_eq!(eval(r, "document.getElementById('ta').focus(); t.value + '|' + document.activeElement.id"), "xy|ta");
    assert_eq!(
        eval(r, "evs.join(',')"),
        "focus,beforeinput:xy,input:xy,keydown,keypress,beforeinput:z,input:z,keyup,keydown,beforeinput,input,change,blur"
    );
}

#[test]
fn style_layout_and_media() {
    let e = env("<html><head><style>.hidden{display:none} @media (min-width: 37.5rem){ .wide{visibility: hidden} }</style></head><body><div id=v>visible</div><div id=h class=hidden>x</div><p id=w class=wide>w</p></body></html>");
    let r = &e.main;
    assert_eq!(eval(r, "matchMedia('(min-width: 37.5rem)').matches"), "true");
    assert_eq!(eval(r, "getComputedStyle(document.getElementById('h')).display"), "none");
    assert_eq!(eval(r, "getComputedStyle(document.getElementById('w')).getPropertyValue('visibility')"), "hidden");
    assert_eq!(
        eval(r, "const rv = document.getElementById('v').getBoundingClientRect(); rv.width > 0 && rv.height > 0"),
        "true"
    );
    assert_eq!(
        eval(r, "const rh = document.getElementById('h').getBoundingClientRect(); rh.width + ':' + rh.height"),
        "0:0"
    );
    assert_eq!(
        eval(
            r,
            "document.getElementById('h').checkVisibility() + ':' + document.getElementById('v').checkVisibility()"
        ),
        "false:true"
    );
    assert_eq!(eval(r, "innerWidth + 'x' + innerHeight"), "1440x900");
    assert_eq!(eval(r, "document.body.innerText"), "visible");
    assert_eq!(
        eval(r, "const v = document.getElementById('v'); const b = v.getBoundingClientRect(); document.elementFromPoint(b.x + 1, b.y + 1) === v"),
        "true"
    );
}

#[test]
fn isolated_world_shares_dom_with_distinct_wrappers() {
    let e = env("<body><div id=a></div></body>");
    let class = e.engine.register::<TestWindow>();
    let iso = new_realm(&e.engine, class, &e.page, "__playwright_utility_world_page@1");
    eval(&e.main, "window.hits = []; document.getElementById('a').addEventListener('ping', () => hits.push('main'))");
    eval(&iso, "window.hits = []; document.getElementById('a').addEventListener('ping', () => hits.push('iso'))");
    assert_eq!(
        eval(
            &iso,
            "document.getElementById('a').dataset.x = '1'; document.getElementById('a') instanceof HTMLDivElement"
        ),
        "true"
    );
    assert_eq!(eval(&e.main, "document.getElementById('a').dataset.x"), "1");
    assert_eq!(eval(&e.main, "document.getElementById('a').expando = 5; document.getElementById('a').dispatchEvent(new Event('ping')); hits.join()"), "main");
    assert_eq!(eval(&iso, "hits.join() + ':' + document.getElementById('a').expando"), "iso:undefined");
}

#[test]
fn gc_reclaims_detached_wrapped_nodes_but_keeps_listened_connected_ones() {
    let e = env("<body><ul id=list></ul></body>");
    let r = &e.main;
    eval(r, "const list = document.getElementById('list'); for (let i = 0; i < 2000; i++) { const li = document.createElement('li'); li.textContent = 'row ' + i; list.appendChild(li); } list.firstChild.addEventListener('mark', () => { window.marked = (window.marked || 0) + 1 });");
    let before = e.page.dom.borrow().live_nodes();
    // Re-render: drop the rows except the first (which has a listener).
    eval(r, "while (list.childNodes.length > 1) list.removeChild(list.lastChild);");
    for _ in 0..3 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
        e.page.after_task();
    }
    let after = e.page.dom.borrow().live_nodes();
    assert!(after < before - 3000, "detached rows reclaimed: before={before} after={after}");
    // The connected, listener-bearing wrapper survived GC with its listener.
    assert_eq!(eval(r, "list.firstChild.dispatchEvent(new Event('mark')); window.marked"), "1");
}

#[test]
fn reentrant_string_conversion_does_not_double_borrow() {
    let e = env("<body><div id=a></div></body>");
    let r = &e.main;
    // toString() runs JS that mutates the DOM while the setter converts it.
    let out = eval(
        r,
        r#"const a = document.getElementById('a');
        const evil = { toString() { document.body.appendChild(document.createElement('i')); return 'x'; } };
        a.id = evil; a.title = evil; a.className = evil; a.textContent = evil; a.append(evil);
        a.setAttribute('data-k', evil); a.classList.add('ok');
        a.id + '|' + document.querySelectorAll('i').length + '|' + a.textContent"#,
    );
    assert_eq!(out, "x|6|xx");
}

#[test]
fn navigation_releases_pinned_listener_wrappers_of_old_document() {
    let e = env("<body><div id=a></div></body>");
    eval(&e.main, "for (let i = 0; i < 50; i++) { const d = document.createElement('div'); d.addEventListener('x', () => {}); document.body.appendChild(d); }");
    e.page.after_task();
    let pinned_before = e.page.pinned_count();
    assert!(pinned_before >= 50, "listener wrappers of connected nodes are pinned: {pinned_before}");
    // Navigate: a new document replaces the old one in the same Dom store.
    let old = e.page.document();
    let new_doc = {
        let mut d = e.page.dom.borrow_mut();
        let nd = d.create_document(lp_dom::DocumentKind::Html, "http://localhost:5173/next");
        d.set_document_active(nd, true).unwrap();
        d.set_document_active(old, false).unwrap();
        nd
    };
    e.page.set_document(new_doc);
    lp_webapi::lifecycle::realm_disposed(&e.page, e.main.id());
    e.main.dispose();
    for _ in 0..3 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
        e.page.after_task();
    }
    assert_eq!(e.page.pinned_count(), 0);
    assert!(!e.page.dom.borrow().is_alive(old), "old document reclaimed after its realm and pins are gone");
}

#[test]
fn css_rule_values_survive_reentrant_gc() {
    let e = env("<head><style id=s></style></head><body>");
    {
        let s = e.main.scope().unwrap();
        let gc = s
            .function("reviewGC", |s, _, _| {
                if let Some(f) = lp_js::sys::spi::get().synchronous_gc_for_debugging {
                    unsafe { f(s.ctx()) };
                }
                Ok(s.undefined())
            })
            .unwrap();
        s.global().set("reviewGC", gc.value()).unwrap();
    }
    assert_eq!(
        eval(
            &e.main,
            r#"
      const sh=document.getElementById('s').sheet;
      for(let i=0;i<200;i++)sh.insertRule('.r'+i+'{}',i);
      window.before=sh.cssRules.every((r,i)=>r.cssText==='.r'+i+'{}');
      Object.defineProperty(Object.prototype,'cssText',{set(v){reviewGC(); Object.defineProperty(this,'cssText',{value:v});}, configurable:true});
      const rr=sh.cssRules; delete Object.prototype.cssText;
      before && rr.length === 200 && rr.every((r,i)=>r.cssText==='.r'+i+'{}')
    "#
        ),
        "true"
    );
}

#[test]
fn regression_typed_array_subview() {
    let e = env("<body>");
    assert_eq!(eval(&e.main, "new TextDecoder().decode(new Uint8Array([65,66,67]).subarray(1,2))"), "B");
}
#[test]
fn regression_reentrant_string_coercion() {
    let e = env("<body>");
    assert_eq!(eval(&e.main, "document.body.append({toString(){ document.body.setAttribute('x','1'); return 'ok'; }}); document.body.textContent"), "ok");
}
#[test]
fn regression_reentrant_reflected_setter() {
    let e = env("<body>");
    assert_eq!(
        eval(
            &e.main,
            "document.body.id = {toString(){ document.body.setAttribute('x','1'); return 'ok'; }}; document.body.id"
        ),
        "ok"
    );
}
#[test]
fn regression_event_target_survives_gc() {
    let e = env("<body>");
    eval(&e.main, "window.saved = new Event('ping'); (() => {const n = document.createElement('div'); n.id = 'old'; n.extra = 'retained'; n.dispatchEvent(saved);})();");
    for _ in 0..4 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
        e.page.after_task();
    }
    assert_eq!(eval(&e.main, "saved.target.extra"), "retained");
    assert_eq!(eval(&e.main, "window.replacement = document.createElement('span'); replacement.id = 'new'; saved.target && saved.target.id"), "old");
}
#[test]
fn regression_abort_releases_listener_slot() {
    let e = env("<body>");
    eval(&e.main, "window.ac = new AbortController(); document.body.addEventListener('x', ()=>{}, {signal: ac.signal}); ac.abort();");
    e.page.after_task();
    let s = e.main.scope().unwrap();
    let body = s.eval("document.body", None, 1).unwrap().as_object().unwrap();
    let arr = s.get_hidden(body, "__lpListeners").unwrap().unwrap().as_object().unwrap();
    assert!(arr.get_index(0).unwrap().is_undefined(), "aborted callback is still strongly rooted");
}
#[test]
fn regression_listener_slot_growth() {
    let e = env("<body>");
    eval(&e.main, "for(let i=0;i<1000;i++){const cb=()=>{}; document.body.addEventListener('x',cb); document.body.removeEventListener('x',cb);}");
    let s = e.main.scope().unwrap();
    let body = s.eval("document.body", None, 1).unwrap().as_object().unwrap();
    let arr = s.get_hidden(body, "__lpListeners").unwrap().unwrap().as_object().unwrap();
    let len = arr.get("length").unwrap().to_number().unwrap();
    assert!(len <= 1.0, "empty listener array length grew to {len}");
}
#[test]
fn regression_range_select_element_text() {
    let e = env("<body><p id=x><b>abc</b><i>def</i></p></body>");
    assert_eq!(
        eval(
            &e.main,
            "const r=document.createRange(); r.selectNodeContents(document.getElementById('x')); r.toString()"
        ),
        "abcdef"
    );
}

#[test]
fn abort_does_not_remove_a_reused_listener_registration() {
    let e = env("<body>");
    assert_eq!(
        eval(
            &e.main,
            r#"
        const ac = new AbortController(); let calls = 0;
        const cb = () => calls++;
        document.body.addEventListener('x', cb, {signal: ac.signal});
        document.body.removeEventListener('x', cb);
        document.body.addEventListener('x', cb);
        ac.abort(); document.body.dispatchEvent(new Event('x')); calls
    "#
        ),
        "1"
    );
    e.page.after_task();
    assert_eq!(eval(&e.main, "document.body.dispatchEvent(new Event('x')); calls"), "2");
}

#[test]
fn dataview_brand_check_ignores_script_properties() {
    let e = env("<body>");
    assert_eq!(
        eval(
            &e.main,
            r#"
        const data = new Uint8Array([65,66,67]);
        const view = new DataView(data.buffer, 1, 1);
        Object.defineProperty(view, 'buffer', {get(){throw Error('should not run')}});
        view.constructor = null;
        new TextDecoder().decode(view)
    "#
        ),
        "B"
    );
    assert_eq!(
        eval(
            &e.main,
            r#"
        const fake = {constructor: {name: 'DataView'}, get buffer(){return this}, byteOffset:0, byteLength:1};
        try { new TextDecoder().decode(fake); 'accepted' } catch(e) { e.name }
    "#
        ),
        "TypeError"
    );
    assert_eq!(eval(&e.main, "new TextDecoder().decode(new DataView(new ArrayBuffer(0)))"), "");
}

#[test]
fn range_text_respects_nested_container_boundaries() {
    let e = env("<body><p id=x><b>abc</b><i>def</i><span>ghi</span></p><p>outside</p></body>");
    assert_eq!(eval(&e.main, "const r=document.createRange(), x=document.getElementById('x'); r.setStart(x,0); r.setEnd(x,1); r.toString()"), "abc");
    assert_eq!(eval(&e.main, "r.setStart(x,1); r.setEnd(x,2); r.toString()"), "def");
    assert_eq!(
        eval(&e.main, "r.setStart(x.firstChild.firstChild,1); r.setEnd(x.lastChild.firstChild,2); r.toString()"),
        "bcdefgh"
    );
    assert_eq!(eval(&e.main, "r.collapse(true); r.toString()"), "");
}
