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

#[test]
fn storage_methods_properties_and_reentrant_coercion() {
    let e = env("<body>");
    assert_eq!(
        eval(
            &e.main,
            r#"
        localStorage.setItem('first', 'one');
        localStorage.second = 'two';
        localStorage.setItem('first', 'updated');
        JSON.stringify([localStorage instanceof Storage, localStorage === localStorage,
            localStorage.length, localStorage.key(0), localStorage.key(1), localStorage.key(2),
            localStorage.getItem('missing'), localStorage.first, Object.keys(localStorage)])
    "#
        ),
        r#"[true,true,2,"first","second",null,null,"updated",["first","second"]]"#
    );
    assert_eq!(
        eval(
            &e.main,
            r#"
        localStorage.setItem({toString(){ localStorage.setItem('nested','yes'); return 'coerced'; }},
            {toString(){return localStorage.getItem('nested');}});
        delete localStorage.second;
        localStorage.removeItem('missing');
        localStorage.coerced + ':' + localStorage.length + ':' + sessionStorage.length
    "#
        ),
        "yes:3:0"
    );
    assert_eq!(
        eval(&e.main, "localStorage.clear(); localStorage.length + ':' + (localStorage.first === undefined)"),
        "0:true"
    );
    assert_eq!(eval(&e.main, "try {new Storage()} catch(e) {e.name}"), "TypeError");
}

fn storage_page(
    engine: &Engine,
    url: &str,
    local: StorageNamespace,
    session: StorageNamespace,
) -> (Rc<PageDom>, Realm) {
    let dom = Rc::new(RefCell::new(Dom::new()));
    let document = parse_document(&dom, url, "<body>");
    dom.borrow_mut().set_document_active(document, true).unwrap();
    let page = PageDom::new(dom, document);
    page.set_storage_namespaces(local, session);
    let realm = new_realm(engine, engine.register::<TestWindow>(), &page, "");
    (page, realm)
}

#[test]
fn storage_partitions_origins_contexts_and_page_sessions() {
    let engine = Engine::new().unwrap();
    register(&engine);
    let local = StorageNamespace::default();
    let session = StorageNamespace::default();
    let (_p1, r1) = storage_page(&engine, "http://example.test/a", local.clone(), session.clone());
    assert_eq!(
        eval(&r1, "localStorage.token='synthetic'; sessionStorage.tab='first'; localStorage.token"),
        "synthetic"
    );
    let (_p2, r2) = storage_page(&engine, "http://example.test/b", local.clone(), StorageNamespace::default());
    assert_eq!(eval(&r2, "localStorage.token + ':' + sessionStorage.getItem('tab')"), "synthetic:null");
    let (_p3, r3) = storage_page(&engine, "http://example.test/new-document", local.clone(), session);
    assert_eq!(eval(&r3, "localStorage.token + ':' + sessionStorage.tab"), "synthetic:first");
    let (_p4, r4) = storage_page(&engine, "https://example.test/", local, StorageNamespace::default());
    assert_eq!(eval(&r4, "localStorage.getItem('token')"), "null");
    let (_p5, r5) =
        storage_page(&engine, "http://example.test/", StorageNamespace::default(), StorageNamespace::default());
    assert_eq!(eval(&r5, "localStorage.getItem('token')"), "null");
    let (_p6, r6) =
        storage_page(&engine, "data:text/html,hello", StorageNamespace::default(), StorageNamespace::default());
    assert_eq!(eval(&r6, "try {localStorage} catch(e) {e.name}"), "SecurityError");
}

#[test]
fn storage_quota_failure_is_atomic_and_replacement_reuses_budget() {
    let engine = Engine::new().unwrap();
    register(&engine);
    let (_p, r) =
        storage_page(&engine, "http://example.test/", StorageNamespace::with_quota(12), StorageNamespace::default());
    assert_eq!(
        eval(
            &r,
            r#"
        localStorage.setItem('a','12345');
        let failure;
        try {localStorage.setItem('a','123456')} catch(e) {failure=e.name;}
        failure + ':' + localStorage.a + ':' + localStorage.length
    "#
        ),
        "QuotaExceededError:12345:1"
    );
    assert_eq!(eval(&r, "localStorage.a='1'; localStorage.b='12'; localStorage.length"), "2");
    assert_eq!(eval(&r, "localStorage.clear(); localStorage.setItem('x','😀😀'); localStorage.x"), "😀😀");
}

#[test]
fn storage_is_shared_across_worlds_and_inherited_blank_origin() {
    let e = env("<body>");
    let isolated = new_realm(&e.engine, e.engine.register::<TestWindow>(), &e.page, "isolated");
    assert_eq!(eval(&e.main, "localStorage.setItem('shared','yes'); localStorage.shared"), "yes");
    assert_eq!(eval(&isolated, "localStorage.shared"), "yes");
    let document = e.page.document();
    e.page.dom.borrow_mut().set_document_url(document, "about:blank").unwrap();
    assert_eq!(eval(&isolated, "try {sessionStorage} catch(e) {e.name}"), "SecurityError");
    let inherited = e
        .engine
        .create_realm(RealmOptions {
            global_class: Some(e.engine.register::<TestWindow>()),
            origin: Some("http://localhost:5173".into()),
            name: "inherited".into(),
        })
        .unwrap();
    inherited
        .set_global_data::<TestWindow>(TestWindowData {
            page: e.page.clone(),
            target: EventTargetData::window(&e.page, e.page.window_target().unwrap()),
        })
        .unwrap();
    install(&inherited.scope().unwrap(), e.page.clone()).unwrap();
    assert_eq!(eval(&inherited, "localStorage.shared"), "yes");
}

#[test]
fn headers_records_sequences_validation_and_cookie_order() {
    let e = env("");
    assert_eq!(
        eval(
            &e.main,
            r#"(() => {
        const h = new Headers([['X-B',' b '], ['x-a','one'], ['X-A','two'],
            ['Set-Cookie','a=1'], ['set-cookie','b=2']]);
        if (h.get('X-A') !== 'one, two' || h.get('missing') !== null) return 'get';
        if (JSON.stringify([...h]) !== JSON.stringify([
            ['set-cookie','a=1'],['set-cookie','b=2'],['x-a','one, two'],['x-b','b']])) return 'entries';
        if (h.getSetCookie().join('|') !== 'a=1|b=2') return 'cookies';
        h.set('SET-cookie','c=3'); h.delete('x-b');
        if (h.getSetCookie().join('|') !== 'c=3' || h.has('x-b')) return 'mutation';
        const copy = new Headers(h); h.set('x-a','changed');
        if (copy.get('x-a') !== 'one, two') return 'copy';
        const record = Object.create({inherited:'ignored'}); record.own = 'ok';
        if (JSON.stringify([...new Headers(record)]) !== '[["own","ok"]]') return 'record';
        let failed = 0;
        for (const run of [() => new Headers(null), () => new Headers([['x']]),
            () => new Headers([['x','y','z']]), () => h.set('bad name','x'),
            () => h.set('x','a\r\nb'), () => h.append('x','\0'),
            () => h.get(''), () => h.set('x','\u0100'), () => h.get(Symbol()),
            () => h.set('x'), () => Headers.prototype.get.call({}, 'x')]) {
            try { run(); } catch(e) { if (e instanceof TypeError) failed++; }
        }
        return failed === 11 ? 'ok' : failed;
    })()"#
        ),
        "ok"
    );
}

#[test]
fn headers_live_iteration_reentry_and_iterator_close() {
    let e = env("");
    assert_eq!(
        eval(
            &e.main,
            r#"(() => {
        const h = new Headers({a:'1',c:'3'}), it = h.entries();
        if (it[Symbol.iterator]() !== it || it.next().value.join(':') !== 'a:1') return 'iterator';
        h.set('b','2'); if (it.next().value.join(':') !== 'b:2') return 'live';
        h.delete('c'); if (!it.next().done) return 'delete';
        h.set('d','4'); if (!it.next().done) return 'done';
        let seen = [], receiver = {};
        h.forEach(function(value, key, self) {
            if (this !== receiver || self !== h) throw Error('receiver');
            seen.push(key+value); if (key === 'a') h.set('c','3');
        }, receiver);
        if (seen.join(',') !== 'a1,b2,c3,d4') return seen;
        h.set({toString(){ h.set('nested','yes'); return 'a'; }},
              {toString(){h.delete('b');return 'changed';}});
        if (h.get('a') !== 'changed' || h.get('nested') !== 'yes' || h.has('b')) return 'reentry';
        let closed = false, yielded = 0;
        function* input(){try { yielded++; yield ['good','1']; yielded++; yield ['bad','\u0100']; yielded++; } finally {closed=true;} }
        try { new Headers(input()); } catch(e) { if (!(e instanceof TypeError)) return 'error'; }
        if (!closed || yielded !== 2) return 'close';
        let converted = false;
        try { h.set('bad name', {toString(){converted=true;return 'value';}}); } catch {}
        let consumed = false;
        function* invalidName() { yield ['bad name','x']; consumed=true; }
        try { new Headers(invalidName()); } catch {}
        return converted && consumed ? 'ok' : 'conversion order';
    })()"#
        ),
        "ok"
    );
}

#[test]
fn headers_capture_intrinsics_before_page_scripts() {
    let e = env("");
    assert_eq!(
        eval(
            &e.main,
            r#"(() => {
        const symbol = Symbol.iterator;
        Symbol = {}; Reflect.ownKeys = () => []; Reflect.apply = () => {throw Error('spoof')};
        Object.getOwnPropertyDescriptor = () => undefined;
        const h = new Headers({x:'yes'});
        const entries = h[symbol]();
        let count = 0; h.forEach(() => count++);
        return h.get('x') === 'yes' && entries.next().value[1] === 'yes' && count === 1;
    })()"#
        ),
        "true"
    );
}

#[inline(never)]
fn abandoned_abort_graph(realm: &Realm) -> std::rc::Weak<()> {
    let scope = realm.scope().unwrap();
    let source = scope
        .eval("(() => { const c = new AbortController(); AbortSignal.any([c.signal]); return c.signal; })()", None, 1)
        .unwrap();
    let keep = Rc::new(());
    let weak = Rc::downgrade(&keep);
    scope.unwrap::<lp_webapi::abort::AbortSignal>(source).unwrap().add_abort_hook(Box::new(move |_| drop(keep)));
    weak
}

#[test]
fn abandoned_abort_any_graph_releases_native_hooks() {
    let e = env("");
    let weak = abandoned_abort_graph(&e.main);
    for _ in 0..6 {
        eval(&e.main, "Array.from({length:2000}, (_,i)=>({i}))");
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
        e.page.after_task();
    }
    assert!(weak.upgrade().is_none(), "unreachable AbortSignal.any graph retained its native hook");
}

#[test]
fn removable_abort_hooks_release_payloads_and_preserve_reason() {
    let e = env("");
    let s = e.main.scope().unwrap();
    let signal = s.eval("globalThis.controller = new AbortController(); controller.signal", None, 1).unwrap();
    let signal = s.unwrap::<lp_webapi::abort::AbortSignal>(signal).unwrap();
    let keep = Rc::new(());
    let weak = Rc::downgrade(&keep);
    let removed = signal.add_abort_hook(Box::new(move |_| drop(keep))).unwrap();
    signal.remove_abort_hook(removed);
    assert!(weak.upgrade().is_none());
    let fired = Rc::new(std::cell::Cell::new(0));
    let fired2 = fired.clone();
    signal.add_abort_hook(Box::new(move |_| fired2.set(fired2.get() + 1)));
    signal.remove_abort_hook(removed); // old identity must not remove a new step
    assert_eq!(eval(&e.main, "globalThis.reason = {detail:42}; globalThis.combined = AbortSignal.any([controller.signal]); controller.abort(reason); combined.aborted && combined.reason === reason"), "true");
    assert_eq!(fired.get(), 1);
    eval(&e.main, "controller.abort('again')");
    assert_eq!(fired.get(), 1);
    let immediate = fired.clone();
    assert!(signal.add_abort_hook(Box::new(move |_| immediate.set(immediate.get() + 1))).is_none());
    assert_eq!(fired.get(), 2);
}

#[test]
fn abort_any_preserves_cross_realm_reason_identity() {
    let e = env("");
    let isolated = new_realm(&e.engine, e.engine.register::<TestWindow>(), &e.page, "isolated");
    let main_scope = e.main.scope().unwrap();
    let signal = main_scope.eval("globalThis.ac = new AbortController(); ac.signal", None, 1).unwrap();
    let isolated_scope = isolated.scope().unwrap();
    isolated_scope.global().set("source", signal).unwrap();
    assert_eq!(eval(&isolated, "globalThis.dependent = AbortSignal.any([source]); dependent.aborted"), "false");
    let reason = main_scope.eval("globalThis.reason = {id:7}; ac.abort(reason); reason", None, 1).unwrap();
    isolated_scope.global().set("expected", reason).unwrap();
    assert_eq!(eval(&isolated, "dependent.aborted && dependent.reason === expected"), "true");
}

struct RetainingHooks {
    _page: Rc<PageDom>,
    dropped: Rc<std::cell::Cell<bool>>,
}
impl PageHooks for RetainingHooks {
    fn post_task(&self, _task: Box<dyn FnOnce()>) {}
    fn navigate(&self, _request: NavigationRequest) {}
}
impl Drop for RetainingHooks {
    fn drop(&mut self) {
        self.dropped.set(true);
    }
}

#[test]
fn page_disposal_releases_hooks_registry_and_listener_roots() {
    let Env { engine, page, main } = env("<body><div id=target></div></body>");
    assert_eq!(eval(&main, "globalThis.fired=0; window.addEventListener('test',()=>fired++); document.querySelector('#target').addEventListener('test',()=>fired++); customElements.define('x-disposal', class extends HTMLElement{}); 'ok'"), "ok");
    page.after_task();
    assert!(page.pinned_count() > 0);
    let dropped = Rc::new(std::cell::Cell::new(false));
    page.set_hooks(Some(Rc::new(RetainingHooks { _page: page.clone(), dropped: dropped.clone() })));
    let weak = Rc::downgrade(&page);
    page.dispose();
    page.dispose();
    assert!(page.is_disposed());
    assert!(dropped.get(), "embedder hook cycle was not broken");
    assert_eq!(page.pinned_count(), 0);
    assert_eq!(eval(&main, "window.dispatchEvent(new Event('test')); document.querySelector('#target').dispatchEvent(new Event('test')); fired"), "0");
    assert!(install(&main.scope().unwrap(), page.clone()).is_err());
    main.dispose();
    drop(main);
    drop(page);
    for _ in 0..6 {
        engine.collect_garbage_now();
        engine.drain_finalized();
    }
    assert!(weak.upgrade().is_none(), "disposed bindings kept their PageDom alive");
}

#[test]
fn indexeddb_empty_restore_preserves_context_and_origin_isolation() {
    let engine = Engine::new().unwrap();
    register(&engine);
    let local = StorageNamespace::default();
    let (page, realm) = storage_page(&engine, "http://example.test/", local.clone(), StorageNamespace::default());
    assert_eq!(eval(&realm, "indexedDB===indexedDB && indexedDB instanceof IDBFactory"), "true");
    eval(
        &realm,
        r#"
        globalThis.result='pending';
        globalThis.savedFactory=indexedDB;
        globalThis.restore=async state=>{
            for(const db of await indexedDB.databases())indexedDB.deleteDatabase(db.name);
            await Promise.all((state.indexedDB||[]).map(async db=>indexedDB.open(db.name,db.version)));
            sessionStorage.clear();localStorage.clear();
            for(const item of state.localStorage||[])localStorage.setItem(item.name,item.value);
        };
        sessionStorage.setItem('old','old');localStorage.setItem('old','old');
        restore({localStorage:[{name:'auth-test',value:'synthetic'}]}).then(()=>result='ok',e=>result=e.name);
    "#,
    );
    assert_eq!(
        eval(
            &realm,
            "result+':'+localStorage.getItem('auth-test')+':'+localStorage.getItem('old')+':'+sessionStorage.length"
        ),
        "ok:synthetic:null:0"
    );
    eval(&realm, "indexedDB.databases().then(a=>{a.push({name:'injected',version:1});return indexedDB.databases()}).then(a=>result=String(a.length))");
    assert_eq!(eval(&realm, "result"), "0");
    // The unsupported nonempty restore must fail before clearing existing data.
    eval(&realm, "restore({indexedDB:[{name:'db',version:1,stores:[]}],localStorage:[]}).then(()=>result='WRONG',e=>result=e.name)");
    assert_eq!(eval(&realm, "result+':'+localStorage.getItem('auth-test')"), "NotSupportedError:synthetic");
    let (_next, navigated) =
        storage_page(&engine, "http://example.test/next", local.clone(), StorageNamespace::default());
    assert_eq!(eval(&navigated, "localStorage.getItem('auth-test')"), "synthetic");
    for (url, namespace) in [("https://example.test/", local), ("http://example.test/", StorageNamespace::default())] {
        let (_other, other) = storage_page(&engine, url, namespace, StorageNamespace::default());
        eval(&other, "globalThis.result='pending';indexedDB.databases().then(a=>result=a.length)");
        assert_eq!(eval(&other, "result+':'+localStorage.getItem('auth-test')"), "0:null");
    }
    page.dispose();
    eval(&realm, "savedFactory.databases().then(()=>result='WRONG',e=>result='disposed')");
    assert_eq!(eval(&realm, "result"), "disposed");
    // A captured factory also rejects after document disposal.
    assert!(eval(&realm, "indexedDB").starts_with("THROW"));
}

#[test]
fn indexeddb_opaque_origin_rejects_and_nonempty_operations_are_explicit() {
    let engine = Engine::new().unwrap();
    register(&engine);
    let (_page, realm) =
        storage_page(&engine, "data:text/html,hello", StorageNamespace::default(), StorageNamespace::default());
    eval(&realm, "globalThis.result='pending';indexedDB.databases().then(()=>result='WRONG',e=>result=e.name)");
    assert_eq!(eval(&realm, "result"), "SecurityError");
    assert_eq!(eval(&realm, "try{indexedDB.open('db')}catch(e){e.name}"), "SecurityError");
    let (_page, realm) =
        storage_page(&engine, "http://example.test", StorageNamespace::default(), StorageNamespace::default());
    assert_eq!(eval(&realm, "try{indexedDB.deleteDatabase('db')}catch(e){e.name}"), "NotSupportedError");
    assert_eq!(eval(&realm, "try{new IDBFactory()}catch(e){e.name}"), "TypeError");
}

#[test]
fn mutation_records_are_typed_static_and_keep_removed_nodes() {
    let e = env("<body><div id=target><span id=child>one</span><i></i></div></body>");
    assert_eq!(
        eval(
            &e.main,
            r#"
        var target=document.querySelector('#target'), child=target.firstChild;
        var observer=new MutationObserver(()=>{});
        observer.observe(target,{childList:true,subtree:true,attributeOldValue:true,characterDataOldValue:true});
        child.setAttribute('title','first'); child.setAttribute('title','second');
        child.firstChild.data='two'; target.removeChild(child);
        var records=observer.takeRecords();
        [records.length, records[0] instanceof MutationRecord, records[0].type,
         records[0].attributeName, records[0].attributeNamespace===null,
         records[0].oldValue===null, records[1].oldValue, records[2].oldValue,
         records[3].type, records[3].removedNodes instanceof NodeList,
         records[3].removedNodes===records[3].removedNodes,
         records[3].removedNodes[0]===child, records[3].previousSibling===null,
         records[3].nextSibling===target.firstChild,observer.takeRecords().length].join('|')
    "#
        ),
        "4|true|attributes|title|true|true|first|one|childList|true|true|true|true|true|0"
    );
    eval(&e.main, "child=null;observer.disconnect();target.remove();target=null");
    for _ in 0..3 {
        e.engine.collect_garbage_now();
        e.page.after_task();
    }
    assert_eq!(eval(&e.main, "records[3].removedNodes[0].textContent+':'+records[3].target.tagName"), "two:DIV");
}

#[test]
fn mutation_options_validate_convert_reentrantly_and_filter() {
    let e = env("<body><div></div></body>");
    assert_eq!(
        eval(
            &e.main,
            r#"
        var target=document.querySelector('div'), observer=new MutationObserver(()=>{});
        var errors=[];
        for (const options of [{},{attributes:false,attributeOldValue:true},{attributes:false,attributeFilter:[]},
          {characterData:false,characterDataOldValue:true},{attributeFilter:null},{attributeFilter:['x',Symbol()]}]) {
          try { observer.observe(target,options); errors.push('wrong'); } catch(e) { errors.push(e.name); }
        }
        observer.observe(target,{get attributeFilter(){target.setAttribute('before','1');return [{toString(){target.setAttribute('during','2');return 'title'}}]}});
        target.setAttribute('ignored','x');target.setAttribute('title','yes');
        [errors.join(','),observer.takeRecords().map(r=>r.attributeName).join(',')].join('|')
    "#
        ),
        "TypeError,TypeError,TypeError,TypeError,TypeError,TypeError|title"
    );
}

#[test]
fn mutation_delivery_batches_reentrant_changes_and_disconnects() {
    let e = env("<body><div></div></body>");
    assert_eq!(
        eval(
            &e.main,
            r#"
        var target=document.querySelector('div'), batches=[];
        var observer=new MutationObserver(function(records,self){
          batches.push([this===observer,self===observer,records.map(r=>r.attributeName).join(',')].join(':'));
          if(batches.length===1)target.setAttribute('second','2');else observer.disconnect();
        });
        observer.observe(target,{attributes:true});target.setAttribute('a','1');target.setAttribute('b','1');
        batches.length
    "#
        ),
        "0"
    );
    e.page.after_task();
    assert_eq!(eval(&e.main, "batches.join('|')"), "true:true:a,b|true:true:second");
    eval(&e.main, "target.setAttribute('c','1')");
    e.page.after_task();
    assert_eq!(eval(&e.main, "batches.length"), "2");
}

#[test]
fn mutation_detached_subtree_transients_end_at_delivery() {
    let e = env("<body><div><span></span></div></body>");
    eval(
        &e.main,
        r#"
      var target=document.querySelector('div'), child=target.firstChild, types=[];
      var observer=new MutationObserver(rs=>types.push(...rs.map(r=>r.type)));
      observer.observe(target,{childList:true,attributes:true,subtree:true});
      child.remove(); child.setAttribute('before','x');
    "#,
    );
    e.page.after_task();
    assert_eq!(eval(&e.main, "types.join()"), "childList,attributes");
    eval(&e.main, "child.setAttribute('after','x')");
    e.page.after_task();
    assert_eq!(eval(&e.main, "types.join()"), "childList,attributes");
}

#[test]
fn mutation_observers_remain_attached_without_user_reference() {
    let e = env("<body><div></div></body>");
    eval(
        &e.main,
        "var calls=0;new MutationObserver(()=>calls++).observe(document.querySelector('div'),{attributes:true})",
    );
    for _ in 0..3 {
        e.engine.collect_garbage_now();
        e.page.after_task();
    }
    eval(&e.main, "document.querySelector('div').setAttribute('a','b')");
    e.page.after_task();
    assert_eq!(eval(&e.main, "calls"), "1");
}

#[test]
fn mutation_realm_disposal_suppresses_only_its_observers() {
    let e = env("<body><div></div></body>");
    let other = new_realm(&e.engine, e.engine.register::<TestWindow>(), &e.page, "isolated");
    for realm in [&e.main, &other] {
        eval(realm,"var calls=0;var observer=new MutationObserver(()=>calls++);observer.observe(document.body,{subtree:true,attributes:true})");
    }
    eval(&e.main, "document.querySelector('div').setAttribute('before','x')");
    lifecycle::realm_disposed(&e.page, other.id());
    other.dispose();
    e.page.after_task();
    assert_eq!(eval(&e.main, "calls"), "1");
    eval(&e.main, "document.querySelector('div').setAttribute('after','x')");
    e.page.after_task();
    assert_eq!(eval(&e.main, "calls"), "2");
    e.page.dispose();
    assert!(eval(&e.main, "observer.takeRecords()").starts_with("THROW"));
}

#[test]
fn mutation_disconnect_clears_queued_records_and_reobserve_replaces_options() {
    let e = env("<body><div></div></body>");
    assert_eq!(
        eval(
            &e.main,
            r#"
      var target=document.querySelector('div'),calls=0,observer=new MutationObserver(()=>calls++);
      observer.observe(target,{attributes:true});target.id='queued';observer.disconnect();
      observer.observe(target,{attributeFilter:['title']});target.id='ignored';target.title='included';
      var result=observer.takeRecords();result.length+':'+result[0].attributeName
    "#
        ),
        "1:title"
    );
    e.page.after_task();
    assert_eq!(eval(&e.main, "calls"), "0");
}

#[test]
fn mutation_delivery_reports_callback_exception_and_continues() {
    let e = env("<body><div></div></body>");
    eval(
        &e.main,
        r#"
      var target=document.querySelector('div'),calls=[];
      new MutationObserver(()=>{calls.push('first');throw new Error('observer test')}).observe(target,{attributes:true});
      new MutationObserver(rs=>calls.push('second:'+rs.length)).observe(target,{attributes:true});
      target.id='changed';
    "#,
    );
    e.page.after_task();
    assert_eq!(eval(&e.main, "calls.join()"), "first,second:1");
}

#[test]
fn mutation_detached_cycles_collect_and_page_disposal_releases_roots() {
    let Env { engine, page, main } = env("<body></body>");
    let before = page.dom.borrow().live_nodes();
    eval(
        &main,
        r#"
      for(let i=0;i<300;i++) {
        const node=document.createElement('div');node.textContent='detached';
        const observer=new MutationObserver(()=>node.textContent);
        observer.observe(node,{attributes:true});
      }
    "#,
    );
    for _ in 0..6 {
        engine.collect_garbage_now();
        engine.drain_finalized();
        page.after_task();
    }
    assert!(page.dom.borrow().live_nodes() < before + 20, "observer/target closure cycle retained detached nodes");
    eval(&main,"new MutationObserver(rs=>{window.saved=rs}).observe(document.body,{childList:true});document.body.append(document.createElement('span'))");
    let weak = Rc::downgrade(&page);
    page.dispose();
    main.dispose();
    drop(main);
    drop(page);
    for _ in 0..6 {
        engine.collect_garbage_now();
        engine.drain_finalized();
    }
    assert!(weak.upgrade().is_none(), "observer records/connected pins retained disposed PageDom");
}

#[test]
fn mutation_microtask_order_includes_nested_native_callbacks() {
    let e = env("<body><div></div></body>");
    eval(
        &e.main,
        r#"
      var target=document.querySelector('div'),order=[];
      new MutationObserver(()=>order.push('observer')).observe(target,{attributes:true});
      Promise.resolve().then(()=>order.push('before'));
      target.setAttribute('a','value');
      Promise.resolve().then(()=>order.push('after'));
      order.push('sync');
    "#,
    );
    assert_eq!(eval(&e.main, "order.join()"), "sync,before,observer,after");
    eval(
        &e.main,
        r#"
      order=[];
      target.setAttribute('b',{toString(){
        target.setAttribute('nested','value');
        Promise.resolve().then(()=>order.push('nested-promise'));
        return 'outer';
      }});
      Promise.resolve().then(()=>order.push('outer-promise'));
    "#,
    );
    assert_eq!(eval(&e.main, "order.join()"), "observer,nested-promise,outer-promise");
}

#[test]
fn mutation_main_and_isolated_callbacks_use_their_own_realm() {
    let e = env("<body><div></div></body>");
    let other = new_realm(&e.engine, e.engine.register::<TestWindow>(), &e.page, "isolated");
    for r in [&e.main, &other] {
        eval(r,"var calls=[];new MutationObserver(rs=>calls.push((rs instanceof Array)+':'+(rs[0] instanceof MutationRecord)+':'+(rs[0].target===document.querySelector('div')))).observe(document.body,{attributes:true,subtree:true})");
    }
    eval(&other, "document.querySelector('div').id='changed'");
    assert_eq!(eval(&e.main, "calls.join()"), "true:true:true");
    assert_eq!(eval(&other, "calls.join()"), "true:true:true");
}

#[test]
fn mutation_native_safe_point_and_cancelled_delivery_realm() {
    let e = env("<body><div></div></body>");
    let other = new_realm(&e.engine, e.engine.register::<TestWindow>(), &e.page, "isolated");
    for r in [&e.main, &other] {
        eval(r, "var calls=0;new MutationObserver(()=>calls++).observe(document,{attributes:true,subtree:true})");
    }
    let node = e
        .page
        .dom
        .borrow()
        .query_selector(e.page.document(), &lp_dom::select::parse_selectors("div").unwrap())
        .unwrap()
        .unwrap();
    e.page.dom.borrow_mut().set_attribute(node, "native", "change").unwrap();
    e.page.after_task();
    assert_eq!(eval(&e.main, "calls"), "1");
    assert_eq!(eval(&other, "calls"), "1");
    let s = e.main.scope().unwrap();
    let weak = Rc::downgrade(&e.page);
    let stop = s
        .function("stopOther", move |s, _, _| {
            if let Some(page) = weak.upgrade() {
                lifecycle::realm_disposed(&page, other.id());
            }
            other.dispose();
            Ok(s.undefined())
        })
        .unwrap();
    s.global().set("stopOther", stop.value()).unwrap();
    eval(&e.main, "document.querySelector('div').id='queued';stopOther()");
    assert_eq!(eval(&e.main, "calls"), "2", "remaining observer lost delivery when scheduled realm was disposed");
}

#[test]
fn url_search_params_parses_duplicates_encoding_and_empty_names() {
    let e = env("<body></body>");
    assert_eq!(
        eval(
            &e.main,
            r#"
      var p=new URLSearchParams('?a=1&a=two+words&empty&=x&&plus=%2B&bad=%FF');
      [p.size,p.getAll('a').join(','),p.get('missing')===null,p.has('empty'),p.get(''),p.get('plus'),p.get('bad'),p.toString()].join('|')
    "#
        ),
        "6|1,two words|true|true|x|+|�|a=1&a=two+words&empty=&=x&plus=%2B&bad=%EF%BF%BD"
    );
}

#[test]
fn url_search_params_constructor_iterables_records_and_usv_strings() {
    let e = env("<body></body>");
    assert_eq!(
        eval(
            &e.main,
            r#"
      var record=Object.create({inherited:'ignored'});record.a='first';record['\ud800']='old';record['\ud801']='new';
      Object.defineProperty(record,'hidden',{value:'ignored'});
      var p=new URLSearchParams(record);
      var seq=new URLSearchParams(new Map([['x','two words'],['y','~*+']]));
      [p.toString(),seq.toString(),new URLSearchParams(seq).toString(),
       new URLSearchParams(null).toString(),new URLSearchParams().size,
       new URLSearchParams([['\ud800','\udfff']]).toString()].join('|')
    "#
        ),
        "a=first&%EF%BF%BD=new|x=two+words&y=%7E*%2B|x=two+words&y=%7E*%2B|null=|0|%EF%BF%BD=%EF%BF%BD"
    );
    assert_eq!(
        eval(
            &e.main,
            r#"
      var iteratorClosed=false;
      function* bad(){try{yield ['x',Symbol('value')]}finally{iteratorClosed=true}}
      var errors=[];
      for(const init of [[['x']],[['x','y','z']],['not-pair'],{[Symbol('key')]:'v'},bad(),{[Symbol.iterator]:5}]){
        try{new URLSearchParams(init);errors.push('bad')}catch(e){errors.push(e.name)}
      }
      errors.join(',')+'|'+iteratorClosed
    "#
        ),
        "TypeError,TypeError,TypeError,TypeError,TypeError,TypeError|true"
    );
}

#[test]
fn url_search_params_mutation_is_reentrant_and_preserves_duplicate_order() {
    let e = env("<body></body>");
    assert_eq!(
        eval(
            &e.main,
            r#"
      var p=new URLSearchParams('a=1&b=keep&a=2&a=3');
      p.set({toString(){p.append('c','side');return 'a'}},{toString(){p.append('a','side');return 'new'}});
      var first=p.toString();p.append('a','last');
      var has=[p.has('a','new'),p.has('a','missing'),p.has('a',undefined)].join(',');
      p.delete('a','new');var second=p.toString();p.delete('a',undefined);
      [first,has,second,p.toString()].join('|')
    "#
        ),
        "a=new&b=keep&c=side|true,false,true|b=keep&c=side&a=last|b=keep&c=side"
    );
    assert_eq!(
        eval(
            &e.main,
            r#"
      var errors=[];for(const name of ['append','set','delete','get','getAll','has']){
        try{p[name]();errors.push('bad')}catch(e){errors.push(e.name)}
      }
      try{p.append('x',Symbol());errors.push('bad')}catch(e){errors.push(e.name)}
      errors.join(',')
    "#
        ),
        "TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError"
    );
}

#[test]
fn url_search_params_iterators_and_foreach_are_live_and_sorted_by_utf16() {
    let e = env("<body></body>");
    assert_eq!(
        eval(
            &e.main,
            r#"
      var p=new URLSearchParams('a=1&b=2'),it=p.entries();
      var first=it.next().value.join(':');p.append('c','3');p.set('b','changed');
      var rest=[...it].map(p=>p.join(':')).join(',');p.append('d','4');
      var done=it.next().done,log=[],receiver={};
      p.forEach(function(value,key,self){log.push((this===receiver&&self===p)+':'+key+':'+value);if(key==='a')p.delete('b');if(key==='c')p.append('e','5')},receiver);
      [first,rest,done,log.join(','),[...p.keys()].join(','),[...p.values()].join(',')].join('|')
    "#
        ),
        "a:1|b:changed,c:3|true|true:a:1,true:c:3,true:d:4,true:e:5|a,c,d,e|1,3,4,5"
    );
    assert_eq!(
        eval(
            &e.main,
            r#"
      var sorted=new URLSearchParams([['\ue000','private'],['\u{10000}','astral'],['a','first'],['a','second']]);
      sorted.sort();[...sorted.values()].join(',')
    "#
        ),
        "first,second,astral,private"
    );
    assert_eq!(eval(&e.main, "Object.prototype.toString.call(p.entries())"), "[object URLSearchParams Iterator]");
    assert_eq!(eval(&e.main, "try{p.entries().next.call({})}catch(e){e.name}"), "TypeError");
    assert_eq!(eval(&e.main, "p.entries().next.call(p.keys()).value"), "a");
    assert_eq!(eval(&e.main, "try{URLSearchParams.prototype.forEach.call({},()=>{})}catch(e){e.name}"), "TypeError");
}

#[test]
fn url_search_params_same_vm_worlds_and_gc_retained_iterator() {
    let e = env("<body></body>");
    let other = new_realm(&e.engine, e.engine.register::<TestWindow>(), &e.page, "isolated");
    let s = e.main.scope().unwrap();
    let params = s.eval("globalThis.params=new URLSearchParams('a=one&a=two');params", None, 1).unwrap();
    other.scope().unwrap().global().set("source", params).unwrap();
    assert_eq!(
        eval(&other, "globalThis.copy=new URLSearchParams(source);source.append('b','three');copy.toString()"),
        "a=one&a=two"
    );
    assert_eq!(eval(&e.main, "globalThis.iterator=params.values();params=null;iterator.next().value"), "one");
    for _ in 0..3 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
        e.page.after_task();
    }
    assert_eq!(eval(&e.main, "[...iterator].join(',')"), "two,three");
    let next = e.main.scope().unwrap().eval("iterator.next", None, 1).unwrap();
    other.scope().unwrap().global().set("otherNext", next).unwrap();
    assert_eq!(eval(&other, "otherNext.call(copy.values()).value"), "one");
}

#[test]
fn url_constructor_resolves_vite_preload_paths_and_components() {
    let e = env("<body></body>");
    assert_eq!(eval(&e.main,r#"
      var u=new URL('../chunk.js','https://example.test/assets/index.js');
      [u.href,u.origin,u.protocol,u.host,u.hostname,u.port,u.pathname,u.search,u.hash,u.toString(),u.toJSON()].join('|')
    "#),"https://example.test/chunk.js|https://example.test|https:|example.test|example.test||/chunk.js|||https://example.test/chunk.js|https://example.test/chunk.js");
}

#[test]
fn url_live_search_params_keep_identity_and_update_both_directions() {
    let e = env("<body></body>");
    assert_eq!(eval(&e.main,r#"
      var u=new URL('https://example.test/path?a=one%20two&a=last#part'),p=u.searchParams;
      var same=p===u.searchParams;p.sort();var sorted=u.href;
      u.search='?x=1&x=2';var changed=p.toString();var it=p.values();it.next();
      u.href='https://other.test/new?z=first&z=second';var next=it.next().value;
      p.delete('z');var cleared=u.href;p.append('space','a b');
      [same,sorted,changed,next,cleared,u.href,p===u.searchParams].join('|')
    "#),"true|https://example.test/path?a=one+two&a=last#part|x=1&x=2|second|https://other.test/new|https://other.test/new?space=a+b|true");
    assert_eq!(
        eval(
            &e.main,
            r#"
      p.set({toString(){u.search='?nested=yes';return 'outer'}},{toString(){u.hash='end';return 'value'}});
      u.href
    "#
        ),
        "https://other.test/new?nested=yes&outer=value#end"
    );
}

#[test]
fn url_component_setters_and_parse_failures_preserve_state() {
    let e = env("<body></body>");
    assert_eq!(eval(&e.main,r#"
      var u=new URL('https://user:pass@example.test:443/a?x=1#b');
      u.hostname='EXAMPLE.org';u.port='8443';u.pathname='/a b';u.username='first last';u.password='p@ss';
      u.hash='hello world';var valid=u.href;u.port='70000';u.hostname='bad host';
      var same=u.href===valid;var error;try{u.href='not absolute'}catch(e){error=e.name}
      [valid,same,error,u.href===valid,u.searchParams.get('x'),u.origin].join('|')
    "#),"https://first%20last:p%40ss@example.org:8443/a%20b?x=1#hello%20world|true|TypeError|true|1|https://example.org:8443");
    assert_eq!(
        eval(
            &e.main,
            r#"
      [URL.canParse('/x','https://example.test/'),URL.canParse('/x'),URL.parse('bad')===null,
       URL.parse('/x',new URL('https://example.test/base')).href,URL.canParse('https://example.test','bad')].join('|')
    "#
        ),
        "true|false|true|https://example.test/x|false"
    );
    assert_eq!(
        eval(
            &e.main,
            r#"
      var calls=[];var parsed=URL.parse({toString(){calls.push('input');return '/path'}},{toString(){calls.push('base');return 'https://example.test'}});
      calls.join(',')+'|'+parsed.href
    "#
        ),
        "input,base|https://example.test/path"
    );
    assert_eq!(eval(&e.main, "try{URL.canParse(Symbol())}catch(e){e.name}"), "TypeError");
    assert_eq!(eval(&e.main, "try{new URL()}catch(e){e.name}"), "TypeError");
    assert_eq!(eval(&e.main, "try{URL.createObjectURL(new Blob())}catch(e){e.name}"), "NotSupportedError");
}

#[test]
fn url_opaque_idn_ipv6_subclass_and_json_serialization() {
    let e = env("<body></body>");
    assert_eq!(
        eval(
            &e.main,
            r#"
      class MyURL extends URL{};
      var u=new MyURL('https://bücher.example:443/a');
      var opaque=new URL('mailto:user@example.test');opaque.hostname='ignored';opaque.username='ignored';
      var ipv6=new URL('http://[::1]:80/x');
      [u instanceof MyURL,u instanceof URL,JSON.stringify(u),opaque.href,opaque.origin,ipv6.host,
       new URL('file:///tmp/../file').href].join('|')
    "#
        ),
        "true|true|\"https://xn--bcher-kva.example/a\"|mailto:user@example.test|null|[::1]|file:///file"
    );
}

#[test]
fn url_search_params_identity_survives_cross_world_getters_and_gc() {
    let e = env("<body></body>");
    let other = new_realm(&e.engine, e.engine.register::<TestWindow>(), &e.page, "isolated");
    let source = e
        .main
        .scope()
        .unwrap()
        .eval("globalThis.address=new URL('https://example.test/?a=1');address", None, 1)
        .unwrap();
    other.scope().unwrap().global().set("source", source).unwrap();
    assert_eq!(eval(&other,"globalThis.params=Object.getOwnPropertyDescriptor(URL.prototype,'searchParams').get.call(source);params===source.searchParams"),"true");
    eval(&e.main, "address=null");
    eval(&other, "source=null");
    for _ in 0..3 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
        e.page.after_task();
    }
    assert_eq!(eval(&other, "params.append('b','2');params.toString()"), "a=1&b=2");
}

#[test]
fn url_params_traced_cycles_collect_but_retained_params_keep_url_alive() {
    let e = env("<body></body>");
    eval(
        &e.main,
        r#"
      var weakUrls=[];
      for(let i=0;i<100;i++){const u=new URL('https://example.test/?i='+i);weakUrls.push(new WeakRef(u));}
      var retainedParams,retainedUrl;
      {const u=new URL('https://example.test/?kept=yes');retainedParams=u.searchParams;retainedUrl=new WeakRef(u);}
    "#,
    );
    for _ in 0..6 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
        e.page.after_task();
    }
    assert_eq!(
        eval(&e.main, "weakUrls.filter(w=>w.deref()!==undefined).length<5"),
        "true",
        "unreachable URL/searchParams cycles stayed rooted"
    );
    assert_eq!(eval(&e.main, "retainedUrl.deref().searchParams===retainedParams"), "true");
    eval(&e.main, "retainedParams=null");
    for _ in 0..6 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
        e.page.after_task();
    }
    assert_eq!(eval(&e.main, "retainedUrl.deref()===undefined"), "true");
}

#[test]
fn structured_clone_tos_form_snapshot_preserves_graph_and_values() {
    let e = env("");
    assert_eq!(
        eval(
            &e.main,
            r#"
      (() => {
        const shared = {name:'Ada'}, value = {firstName:'Ada', lastName:'Lovelace',
          date: new Date(123456), nil:null, unset:undefined, negativeZero:-0,
          nan:NaN, infinity:Infinity, big:123n, shared, again:shared, sparse:[, 'x']};
        value.self=value;
        const copy=structuredClone(value);
        copy.shared.name='Changed';
        return [copy!==value,copy.self===copy,copy.shared===copy.again,shared.name,
          copy.date instanceof Date,+copy.date,copy.nil===null,'unset' in copy,
          Object.is(copy.negativeZero,-0),Number.isNaN(copy.nan),copy.infinity,copy.big,
          copy.sparse.length,0 in copy.sparse,copy.sparse[1]].join('|');
      })()
    "#
        ),
        "true|true|true|Ada|true|123456|true|true|true|true|Infinity|123|2|false|x"
    );
}

#[test]
fn structured_clone_collections_buffers_and_reentrant_snapshot() {
    let e = env("");
    assert_eq!(
        eval(
            &e.main,
            r#"
      (() => {
        const buffer=new ArrayBuffer(12), bytes=new Uint8Array(buffer); bytes[3]=77;
        const key={k:1}, map=new Map(), set=new Set();
        map.set(key,set); map.set(map,key); set.add(map);
        const re=/a+/gim; re.lastIndex=5;
        const value={map,set,key,buffer,view:new Uint8Array(buffer,2,4),data:new DataView(buffer,1,5),re,
          boxed:new Number(NaN)};
        const c=structuredClone(value); c.view[1]=88;
        const source=new Map(); source.set('first',{get v(){source.clear();return 9}});source.set('second',2);
        const snapshot=structuredClone(source);
        return [c.map.get(c.key)===c.set,c.set.has(c.map),c.map.get(c.map)===c.key,
          c.view.buffer===c.buffer,c.data.buffer===c.buffer,c.view.byteOffset,c.view.length,
          c.data.byteOffset,c.data.byteLength,new Uint8Array(c.buffer)[3],bytes[3],
          c.re.source,c.re.flags,c.re.lastIndex,c.boxed instanceof Number,Number.isNaN(+c.boxed),
          snapshot.size,snapshot.get('first').v,snapshot.get('second')].join('|');
      })()
    "#
        ),
        "true|true|true|true|true|2|4|1|5|88|77|a+|gim|0|true|true|2|9|2"
    );
}

#[test]
fn structured_clone_accessors_descriptors_and_failure_identity() {
    let e = env("");
    assert_eq!(
        eval(
            &e.main,
            r#"
      (() => {
        let calls=0; const source=Object.create(null);
        Object.defineProperties(source,{
          a:{enumerable:true,get(){calls++;delete source.b;source.late=9;return {x:1}}},
          b:{enumerable:true,configurable:true,value:2},
          hidden:{value:()=>{}}
        });
        Object.defineProperty(source,'__proto__',{value:'data',enumerable:true});
        source[Symbol('ignored')]=()=>{};
        const c=structuredClone(source), desc=Object.getOwnPropertyDescriptor(c,'a');
        const marker={};let same=false;
        try {structuredClone({get x(){throw marker}})} catch(e){same=e===marker}
        return [calls,Object.keys(c).join(','),Object.getPrototypeOf(c)===Object.prototype,
          c.a.x,desc.writable,desc.configurable,desc.get===undefined,same,c.__proto__].join('|');
      })()
    "#
        ),
        "1|a,__proto__|true|1|true|true|true|true|data"
    );
}

#[test]
fn structured_clone_rejects_unsupported_values_without_proxy_traps_or_detachment() {
    let e = env("");
    assert_eq!(
        eval(
            &e.main,
            r#"
      (() => {
        let traps=0;
        const proxy=new Proxy({a:1},{get(){traps++;throw 1},ownKeys(){traps++;throw 2},
          getPrototypeOf(){traps++;throw 3}});
        const revoked=Proxy.revocable({},{});revoked.revoke();
        const values=[proxy,revoked.proxy,document,window,()=>{},Symbol('x'),new WeakMap(),Promise.resolve(1),
          new URL('https://example.test/'),new Error('not yet supported')];
        const errors=values.map(v=>{try{structuredClone(v);return 'accepted'}catch(e){return e.name}});
        const buffer=new ArrayBuffer(4);let transfer;
        try {structuredClone({buffer},{transfer:[buffer]})}catch(e){transfer=e.name}
        let custom=0;const tagged={get [Symbol.toStringTag](){custom++;return 'Object'}};
        let customError;try{structuredClone(tagged)}catch(e){customError=e.name}
        return [errors.every(n=>n==='DataCloneError'),traps,transfer,buffer.byteLength,
          structuredClone({ok:1},{transfer:[]}).ok,custom,customError].join('|');
      })()
    "#
        ),
        "true|0|NotSupportedError|4|1|0|NotSupportedError"
    );
}

#[test]
fn structured_clone_captures_intrinsics_and_releases_temporary_graphs() {
    let e = env("");
    assert_eq!(
        eval(
            &e.main,
            r#"
      var cloneRefs=[];
      (()=>{
        const originalMap=Map, originalDate=Date, originalArray=Array;
        const m=new Map([[1,{a:2}]]),d=new Date(123);
        Object.keys=()=>{throw 'keys replaced'};
        Map.prototype.get=()=>{throw 'get replaced'};
        Map.prototype.set=()=>{throw 'set replaced'};
        Map=Date=Array=function(){throw 'constructor replaced'};
        const c=structuredClone({m,d,list:[1,2]});
        cloneRefs.push(new WeakRef(c),new WeakRef(c.m));
        return [c.m instanceof originalMap,c.d instanceof originalDate,+c.d,
          c.list instanceof originalArray,c.list.join(',')].join('|');
      })()
    "#
        ),
        "true|true|123|true|1,2"
    );
    for _ in 0..6 {
        e.engine.collect_garbage_now();
    }
    assert_eq!(eval(&e.main, "cloneRefs.every(r=>r.deref()===undefined)"), "true");
}

#[test]
fn structured_clone_cross_world_values_use_destination_intrinsics() {
    let e = env("");
    let class = e.engine.register::<TestWindow>();
    let isolated = new_realm(&e.engine, class, &e.page, "clone-world");
    {
        let source = isolated.scope().unwrap();
        let value = source.eval("({date:new Date(42),map:new Map([[1,{x:2}]]),array:[1,2]})", None, 1).unwrap();
        e.main.scope().unwrap().global().set("foreignCloneSource", value).unwrap();
        e.main.scope().unwrap().global().set("foreignCloneWindow", source.global().value()).unwrap();
    }
    assert_eq!(
        eval(
            &e.main,
            r#"
      (()=>{
        const c=structuredClone(foreignCloneSource);let error;
        try{structuredClone(foreignCloneWindow)}catch(e){error=e.name}
        return [Object.getPrototypeOf(c)===Object.prototype,c.date instanceof Date,
          c.map instanceof Map,c.array instanceof Array,+c.date,c.map.get(1).x,error].join('|');
      })()
    "#
        ),
        "true|true|true|true|42|2|DataCloneError"
    );
}

#[test]
fn structured_clone_resizable_buffers_and_explicit_limits() {
    let e = env("");
    assert_eq!(
        eval(
            &e.main,
            r#"
      (()=>{
        const source=new ArrayBuffer(2,{maxByteLength:8});new Uint8Array(source)[0]=9;
        const cloned=structuredClone(source);cloned.resize(4);
        const graph={};let tail=graph;
        for(let i=0;i<520;i++)tail=tail.next={};
        let depthError;try{structuredClone(graph)}catch(e){depthError=e.name}
        const large=new ArrayBuffer(64*1024*1024+1);
        let sizeError;try{structuredClone(large)}catch(e){sizeError=e.name}
        return [cloned.resizable,cloned.maxByteLength,cloned.byteLength,source.byteLength,
          new Uint8Array(cloned)[0],depthError,sizeError].join('|');
      })()
    "#
        ),
        "true|8|4|2|9|NotSupportedError|NotSupportedError"
    );
}

#[derive(Default)]
struct ObserverTasks(RefCell<std::collections::VecDeque<Box<dyn FnOnce()>>>);
impl PageHooks for ObserverTasks {
    fn post_task(&self, task: Box<dyn FnOnce()>) {
        self.0.borrow_mut().push_back(task);
    }
    fn navigate(&self, _: NavigationRequest) {}
}
impl ObserverTasks {
    fn drain(&self) {
        for _ in 0..100 {
            let task = self.0.borrow_mut().pop_front();
            let Some(task) = task else { return };
            task();
        }
        panic!("observer task loop did not settle");
    }
}

#[test]
fn intersection_observer_initial_task_and_geometry() {
    let e = env("<body><div id=t style='width:100px;height:100px'></div></body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    assert_eq!(
        eval(
            &e.main,
            r#"
        var target = document.getElementById('t'), entries=[], order=[];
        var observer = new IntersectionObserver(function (list, obs) {
            order.push('observer'); entries.push(...list);
            if(this!==observer || obs!==observer) throw Error('callback identity');
        }, {threshold:[1,0,.5]});
        observer.observe(target); observer.observe(target);
        Promise.resolve().then(()=>order.push('promise'));
        [entries.length, observer.root, observer.rootMargin, observer.thresholds.join(), Object.isFrozen(observer.thresholds)].join('|')
    "#
        ),
        "0||0px 0px 0px 0px|0,0.5,1|true"
    );
    e.page.after_task();
    assert_eq!(eval(&e.main, "entries.length"), "0");
    tasks.drain();
    assert_eq!(
        eval(
            &e.main,
            r#"order.join() + ':' + entries.length + ':' +
        (entries[0] instanceof IntersectionObserverEntry && entries[0].target===target &&
         entries[0].isIntersecting && entries[0].intersectionRatio===1 &&
         entries[0].boundingClientRect.width===target.getBoundingClientRect().width &&
         entries[0].rootBounds.width===innerWidth && entries[0].time>=0)"#
        ),
        "promise,observer:1:true"
    );
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "entries.length"), "1");
    let mut vp = e.page.viewport();
    vp.height = 50;
    e.page.set_viewport(vp);
    e.page.after_task();
    tasks.drain();
    assert_eq!(
        eval(&e.main, "entries.length===2 && entries[1].intersectionRatio>0 && entries[1].intersectionRatio<.5"),
        "true"
    );
    assert_eq!(eval(&e.main, "target.remove(); 'ok'"), "ok");
    e.page.after_task();
    tasks.drain();
    assert_eq!(
        eval(&e.main, "entries.length===3 && !entries[2].isIntersecting && entries[2].intersectionRatio===0"),
        "true"
    );
    e.page.dispose();
}

#[test]
fn intersection_observer_records_survive_disconnect_and_detached_gc() {
    let e = env("<body><div id=t style='width:10px;height:10px'></div></body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    assert_eq!(
        eval(
            &e.main,
            r#"
        var target=document.getElementById('t'), deliveries=[];
        var io=new IntersectionObserver(list=>deliveries.push(...list));io.observe(target);'ok'
    "#
        ),
        "ok"
    );
    e.page.after_task();
    assert_eq!(eval(&e.main, "io.disconnect(); target.remove(); target=null; 'ok'"), "ok");
    e.engine.collect_garbage_now();
    tasks.drain();
    assert_eq!(
        eval(&e.main, "deliveries.length+':'+deliveries[0].target.id+':'+deliveries[0].isIntersecting"),
        "1:t:true"
    );
    assert_eq!(eval(&e.main, "io.observe(deliveries[0].target); 'ok'"), "ok");
    e.page.after_task();
    assert_eq!(
        eval(
            &e.main,
            "var taken=io.takeRecords(); taken.length+':'+taken[0].isIntersecting+':'+io.takeRecords().length"
        ),
        "1:false:0"
    );
    tasks.drain();
    assert_eq!(eval(&e.main, "deliveries.length"), "1");
    assert_eq!(eval(&e.main, "document.body.append(taken[0].target); io.unobserve(taken[0].target); 'ok'"), "ok");
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "deliveries.length"), "1");
    e.page.dispose();
}

#[test]
fn intersection_observer_options_conversion_and_explicit_limits() {
    let e = env("<body><div id=t></div></body>");
    assert_eq!(eval(&e.main,r#"
        var errors=[];function failure(fn){try{fn();errors.push('BAD')}catch(e){errors.push(e.name)}}
        failure(()=>new IntersectionObserver({}));
        for(const threshold of [-1,1.01,NaN,Infinity])failure(()=>new IntersectionObserver(()=>{},{threshold}));
        failure(()=>new IntersectionObserver(()=>{},{root:document.createTextNode('x')}));
        failure(()=>new IntersectionObserver(()=>{},{rootMargin:'10em'}));
        failure(()=>new IntersectionObserver(()=>{},{rootMargin:'0'}));
        failure(()=>new IntersectionObserver(()=>{},{trackVisibility:true}));
        failure(()=>new IntersectionObserver(()=>{},{delay:1}));
        failure(()=>new IntersectionObserver(()=>{},{scrollMargin:'1px'}));
        var io=new IntersectionObserver(()=>{}, {threshold:[],rootMargin:'10% -2px 3pt'});
        failure(()=>io.observe(document));failure(()=>io.unobserve());
        errors.join(',')+'|'+io.thresholds.join()+'|'+io.rootMargin
    "#),"TypeError,RangeError,RangeError,TypeError,TypeError,TypeError,SyntaxError,SyntaxError,NotSupportedError,NotSupportedError,NotSupportedError,TypeError,TypeError|0|10% -2px 4px -2px");
    assert_eq!(
        eval(
            &e.main,
            r#"
        var reads=[];var options={
          get delay(){reads.push('delay');return 0}, get root(){reads.push('root');return document},
          get rootMargin(){reads.push('margin');document.body.append(document.createElement('p'));return '0px'},
          get scrollMargin(){reads.push('scroll');return '0px'},
          get threshold(){reads.push('threshold');return new Set([.5,0,.5])},
          get trackVisibility(){reads.push('visibility');return false}
        }; var converted=new IntersectionObserver(()=>{},options);
        reads.join()+':'+(converted.root===document)+':'+converted.thresholds.join()
    "#
        ),
        "delay,root,margin,scroll,threshold,visibility:true:0,0.5"
    );
    e.page.dispose();
}

#[test]
fn intersection_observer_clips_ancestors_and_tracks_scroll_styles() {
    let e=env("<style>#clip {overflow:hidden;width:50px;height:50px}</style><body><div id=clip><div id=t style='width:100px;height:100px'></div></div><div style='height:2000px'></div></body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    assert_eq!(
        eval(
            &e.main,
            r#"
        var target=document.getElementById('t'), clip=document.getElementById('clip'), records=[];
        var io=new IntersectionObserver(list=>records.push(...list),{threshold:[0,.25,.5,1]});io.observe(target);'ok'
    "#
        ),
        "ok"
    );
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main,"records.length===1 && records[0].isIntersecting && records[0].intersectionRatio>0 && records[0].intersectionRatio<.25"),"true");
    assert_eq!(eval(&e.main, "clip.style.overflow='visible'; 'ok'"), "ok");
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "records.length===2 && records[1].intersectionRatio===1"), "true");
    assert_eq!(eval(&e.main, "scrollTo(0,400); scrollY>0"), "true");
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "records.length===3 && !records[2].isIntersecting"), "true");
    assert_eq!(eval(&e.main, "scrollTo(0,0); target.style.visibility='hidden'; target.style.opacity='0'; 'ok'"), "ok");
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "records.length===4 && records[3].intersectionRatio===1"), "true");
    e.page.dispose();
}

#[test]
fn intersection_observer_explicit_root_margin_zero_area_and_edge_contact() {
    let e=env("<body><div id=root style='width:100px;height:100px'><div id=t style='width:10px;height:10px'></div></div><div id=other></div></body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    assert_eq!(
        eval(
            &e.main,
            r#"
        var root=document.getElementById('root'), t=document.getElementById('t'), records=[];
        var io=new IntersectionObserver(list=>records.push(...list),{root,rootMargin:'10%'});
        io.observe(t);io.observe(document.getElementById('other'));io.observe(root);'ok'
    "#
        ),
        "ok"
    );
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main,"records.length===3 && records[0].intersectionRatio===1 && records[0].rootBounds.width===120 && !records[1].isIntersecting && !records[2].isIntersecting"),"true");
    assert_eq!(
        eval(
            &e.main,
            r#"
        var edgeRecords=[], rect=t.getBoundingClientRect();
        var edge=new IntersectionObserver(list=>edgeRecords.push(...list),{rootMargin:`0px 0px ${rect.top-innerHeight}px 0px`});edge.observe(t);'ok'
    "#
        ),
        "ok"
    );
    e.page.after_task();
    tasks.drain();
    assert_eq!(
        eval(
            &e.main,
            "edgeRecords.length===1 && edgeRecords[0].isIntersecting && edgeRecords[0].intersectionRatio===0"
        ),
        "true"
    );
    assert_eq!(eval(&e.main, "t.style.width='0px';t.style.height='0px'; 'ok'"), "ok");
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "records.length===3"), "true"); // Ratio remains 1 for zero-area intersecting targets.
    assert_eq!(eval(&e.main, "root.remove(); 'ok'"), "ok");
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "records.length===4 && !records[3].isIntersecting"), "true");
    e.page.dispose();
}

#[test]
fn intersection_observer_disposal_and_shared_worlds() {
    let e = env("<body><div id=t></div></body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    let isolated = new_realm(&e.engine, e.engine.register::<TestWindow>(), &e.page, "isolated");
    for realm in [&e.main, &isolated] {
        assert_eq!(eval(realm,"var calls=0; var io=new IntersectionObserver(()=>calls++);io.observe(document.getElementById('t')); 'ok'"),"ok");
    }
    e.page.after_task();
    lifecycle::realm_disposed(&e.page, isolated.id());
    tasks.drain();
    assert_eq!(eval(&e.main, "calls"), "1");
    assert_eq!(eval(&isolated, "calls"), "0");
    assert_eq!(eval(&e.main, "document.getElementById('t').remove(); 'ok'"), "ok");
    e.page.after_task();
    e.page.dispose();
    tasks.drain();
    assert_eq!(eval(&e.main, "calls"), "1");
    let weak = Rc::downgrade(&e.page);
    isolated.dispose();
    e.main.dispose();
    drop(isolated);
    drop(e.main);
    drop(e.page);
    for _ in 0..6 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
    }
    assert!(weak.upgrade().is_none(), "disposed observer state retained the page");
}

#[test]
fn intersection_observer_active_gc_callback_reentry_and_exception_isolation() {
    let e = env("<body><div id=t></div></body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    assert_eq!(
        eval(
            &e.main,
            r#"
        var calls=[],ref;(()=>{
            const observer=new IntersectionObserver(function(list,self){
                calls.push('first:'+list.length);self.disconnect();throw Error('expected observer failure');
            });ref=new WeakRef(observer);observer.observe(document.getElementById('t'));
        })();
        var later=new IntersectionObserver((list,self)=>{calls.push('second');self.disconnect();self.observe(document.getElementById('t'));});
        later.observe(document.getElementById('t'));'ok'
    "#
        ),
        "ok"
    );
    for _ in 0..6 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
    }
    assert_eq!(eval(&e.main, "ref.deref() instanceof IntersectionObserver"), "true");
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "calls.join()"), "first:1,second");
    e.page.after_task();
    assert_eq!(eval(&e.main, "later.takeRecords().length"), "1");
    tasks.drain();
    for _ in 0..6 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
    }
    assert_eq!(eval(&e.main, "ref.deref()===undefined"), "true");
    e.page.dispose();
}

#[test]
fn intersection_observer_without_task_service_retains_records_without_inline_callback() {
    let e = env("<body><div id=t></div></body>");
    assert_eq!(
        eval(
            &e.main,
            "var calls=0;var io=new IntersectionObserver(()=>calls++);io.observe(document.getElementById('t'));'ok'"
        ),
        "ok"
    );
    e.page.after_task();
    assert_eq!(eval(&e.main, "calls+':'+io.takeRecords().length"), "0:1");
    assert_eq!(eval(&e.main, "io.disconnect();io.observe(document.getElementById('t'));'ok'"), "ok");
    e.page.after_task();
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "calls"), "1");
    e.page.dispose();
}

#[test]
fn intersection_observer_element_scroll_changes_geometry_without_dom_mutation() {
    let e=env("<body><div id=root style='overflow:auto;width:100px;height:100px'><div id=t style='width:20px;height:20px'></div><div style='height:1000px'></div></div></body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    assert_eq!(
        eval(
            &e.main,
            r#"
        var root=document.getElementById('root'),t=document.getElementById('t'),records=[];
        var io=new IntersectionObserver(list=>records.push(...list),{root});io.observe(t);
        var top=t.getBoundingClientRect().top;'ok'
    "#
        ),
        "ok"
    );
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "records.length===1 && records[0].isIntersecting"), "true");
    let before = e.page.dom.borrow().version();
    assert_eq!(eval(&e.main, "root.scrollTop=100;root.scrollTop"), "100");
    assert_eq!(e.page.dom.borrow().version(), before);
    e.page.after_task();
    tasks.drain();
    assert_eq!(
        eval(&e.main, "records.length===2 && !records[1].isIntersecting && t.getBoundingClientRect().top===top-100"),
        "true"
    );
    e.page.dispose();
}

#[test]
fn global_event_handler_attributes() {
    let e = env("<body><a id=a href='#x'>a</a><input id=i></body>");
    let r = &e.main;
    // ReactDOM isEventSupported('input'): 'oninput' in document, else the content-attribute probe.
    assert_eq!(eval(r, "['oninput' in document, 'onclick' in window, 'onpopstate' in window, 'onreadystatechange' in document].join()"), "true,true,true,true");
    assert_eq!(
        eval(r, "const d=document.createElement('div'); d.setAttribute('oninput','return;'); typeof d.oninput"),
        "function"
    );
    assert_eq!(eval(r, "d.removeAttribute('oninput'); d.oninput"), "null");
    // First/last entries of each single-source family: IDL accessor and
    // content-attribute recognition agree.
    assert_eq!(
        eval(r, "['abort','transitioncancel'].every(t=>{const x=document.createElement('p'); x.setAttribute('on'+t,'1'); return ('on'+t) in x && typeof x['on'+t]==='function'}) && ['afterprint','unload'].every(t=>('on'+t) in window) && ['readystatechange','visibilitychange'].every(t=>('on'+t) in document) && !('onunload' in document.body)"),
        "true"
    );
    let out = eval(
        r,
        r#"const a=document.getElementById('a'); const log=[];
        a.addEventListener('click',()=>log.push('first'));
        a.onclick=function(ev){log.push('handler:'+(this===a)+':'+(ev instanceof MouseEvent)); return false};
        a.addEventListener('click',e=>log.push('last:'+e.defaultPrevented));
        a.click();
        a.onclick=function(){log.push('replaced')}; a.click();
        a.onclick=null; a.click();
        a.onclick='not a function'; log.push(String(a.onclick));
        log.join('|')"#,
    );
    assert_eq!(out, "first|handler:true:true|last:true|first|replaced|last:false|first|last:false|null");
    assert_eq!(eval(r, "let got=0; window.onfoo=1; window.onmessage=null; document.getElementById('i').setAttribute('onclick','window.hit=(window.hit||0)+1'); document.getElementById('i').click(); window.hit"), "1");
    assert_eq!(eval(r, "let w=0; window.onhashchange=()=>w++; window.dispatchEvent(new Event('hashchange')); const same=typeof window.onhashchange; window.onhashchange=null; window.dispatchEvent(new Event('hashchange')); w+':'+same+':'+window.onhashchange"), "1:function:null");
}

#[test]
fn resize_observer_initial_changes_hidden_and_disconnect() {
    let e = env("<body><div id=t style='width:100px;height:40px'></div><div id=z style='display:none'></div></body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    // TOS table pattern: measure parent clientWidth, observe in a layout effect.
    assert_eq!(
        eval(
            &e.main,
            r#"var t=document.getElementById('t'), z=document.getElementById('z'), seen=[];
        var ro=new ResizeObserver(function(list, obs){ if(this!==ro||obs!==ro) throw Error('identity');
            for (const en of list) seen.push(en.target.id+':'+en.contentRect.width+'x'+en.contentRect.height+':'+en.borderBoxSize[0].inlineSize+'/'+en.contentBoxSize[0].blockSize+':'+(en instanceof ResizeObserverEntry)); });
        ro.observe(t); ro.observe(t); ro.observe(z, {box:'border-box'});
        try { ro.observe(t, {box:'nope'}); 'no-throw' } catch (e) { e instanceof TypeError ? 'typed' : 'other' }"#
        ),
        "typed"
    );
    e.page.after_task();
    assert_eq!(eval(&e.main, "seen.length"), "0", "delivered as a separate task, not inline");
    tasks.drain();
    assert_eq!(eval(&e.main, "seen.join()"), "t:100x40:100/40:true", "hidden 0x0 target has no initial entry");
    // No change -> no entry.
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "seen.length"), "1");
    // Style change resizes; display:none reports 0x0; shown again reports its size.
    eval(&e.main, "t.style.width='250px'");
    e.page.after_task();
    tasks.drain();
    eval(&e.main, "t.style.display='none'");
    e.page.after_task();
    tasks.drain();
    eval(&e.main, "z.style.display='block'; z.style.width='7px'; z.style.height='3px'");
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "seen.slice(1).join()"), "t:250x40:250/40:true,t:0x0:0/0:true,z:7x3:7/3:true");
    eval(&e.main, "ro.unobserve(z); z.style.width='9px'");
    e.page.after_task();
    tasks.drain();
    eval(&e.main, "ro.disconnect(); t.style.display='block'");
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "seen.length"), "4");
    assert_eq!(eval(&e.main, "try { new ResizeObserver(1) } catch (x) { x instanceof TypeError }"), "true");
    e.page.dispose();
}

#[test]
fn resize_observer_realm_disposal_releases_observers() {
    let e = env("<body><div id=t style='width:10px;height:10px'></div></body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    eval(&e.main, "window.n=0; new ResizeObserver(()=>n++).observe(document.getElementById('t'))");
    // Unreferenced but active observers survive GC (pinned while observing).
    e.engine.collect_garbage_now();
    e.engine.drain_finalized();
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "document.getElementById('t').style.width='20px'; n"), "1");
    e.engine.collect_garbage_now();
    e.engine.drain_finalized();
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "n"), "2");
    assert_eq!(e.page.resize_observer_pins(), 1);
    lp_webapi::lifecycle::realm_disposed(&e.page, e.main.id());
    e.page.after_task();
    tasks.drain();
    assert_eq!(e.page.resize_observer_pins(), 0);
    e.page.dispose();
}
