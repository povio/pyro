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

/// Loads no sub-resources (the layout tests' pages have none).
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
    let main = new_realm(&engine, window_class, &page, "");
    Env { engine, page, main }
}

fn new_realm(engine: &Engine, class: lp_js::ClassId, page: &Rc<PageDom>, name: &str) -> Realm {
    new_realm_with_origin(engine, class, page, name, None)
}

fn new_realm_with_origin(
    engine: &Engine,
    class: lp_js::ClassId,
    page: &Rc<PageDom>,
    name: &str,
    origin: Option<String>,
) -> Realm {
    let realm =
        engine.create_realm(RealmOptions { name: name.into(), global_class: Some(class), origin }).expect("realm");
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
fn attribute_reflection_kinds() {
    let e = env("<!doctype html><html><body><p>x</p></body></html>");
    let r = &e.main;
    // Enumerated, limited to known values: canonical case, invalid and missing
    // defaults, and the content attribute keeps the string that was set.
    assert_eq!(eval(r, "const a = document.createElement('a'); a.dir"), "");
    assert_eq!(eval(r, "a.dir = 'RTL'; a.dir + ':' + a.getAttribute('dir')"), "rtl:RTL");
    assert_eq!(eval(r, "a.dir = 'nope'; a.dir + ':' + a.getAttribute('dir')"), ":nope");
    assert_eq!(eval(r, "a.setAttribute('dir', 'r\\u017Ftl'); a.dir"), "");
    assert_eq!(
        eval(r, "const t = document.createElement('track'); t.kind + ':' + (t.kind = 'x', t.kind)"),
        "subtitles:metadata"
    );
    assert_eq!(
        eval(r, "const f = document.createElement('form'); f.enctype === f.encoding && f.method + f.autocomplete"),
        "geton"
    );
    assert_eq!(
        eval(r, "const b = document.createElement('button'); b.formMethod + ':' + (b.formMethod = 'x', b.formMethod)"),
        ":get"
    );
    // Nullable CORS settings.
    assert_eq!(eval(r, "const i = document.createElement('img'); String(i.crossOrigin)"), "null");
    assert_eq!(eval(r, "i.crossOrigin = ''; i.crossOrigin + ':' + i.getAttribute('crossorigin')"), "anonymous:");
    assert_eq!(eval(r, "i.crossOrigin = 'USE-credentials'; i.crossOrigin"), "use-credentials");
    assert_eq!(eval(r, "i.crossOrigin = null; String(i.crossOrigin) + i.hasAttribute('crossorigin')"), "nullfalse");
    // Integers parse with the HTML rules, not Rust's.
    assert_eq!(eval(r, "const o = document.createElement('ol'); o.start"), "1");
    assert_eq!(eval(r, "o.setAttribute('start', ' \\t-5x'); o.start"), "-5");
    assert_eq!(eval(r, "o.setAttribute('start', '\\u00A07'); o.start"), "1");
    assert_eq!(eval(r, "o.setAttribute('start', '2147483648'); o.start"), "1");
    assert_eq!(eval(r, "i.hspace = 2147483648; i.hspace + ':' + i.getAttribute('hspace')"), "0:0");
    assert_eq!(eval(r, "i.setAttribute('hspace', '-1'); i.hspace"), "0");
    assert_eq!(eval(r, "const n = document.createElement('input'); n.maxLength + ':' + n.size"), "-1:20");
    assert_eq!(eval(r, "try { n.maxLength = -1 } catch (x) { x.name }"), "IndexSizeError");
    assert_eq!(eval(r, "try { n.size = 0 } catch (x) { x.name }"), "IndexSizeError");
    assert_eq!(
        eval(r, "const ta = document.createElement('textarea'); ta.cols = 0; ta.cols + ':' + ta.getAttribute('cols')"),
        "20:20"
    );
    // Clamped ranges.
    assert_eq!(eval(r, "const c = document.createElement('td'); c.setAttribute('colspan', '5000'); c.colSpan"), "1000");
    assert_eq!(eval(r, "c.setAttribute('colspan', '0'); c.colSpan"), "1");
    assert_eq!(eval(r, "c.setAttribute('colspan', '-3'); c.colSpan"), "1");
    assert_eq!(eval(r, "c.setAttribute('rowspan', '0'); c.rowSpan"), "0");
    // Doubles.
    assert_eq!(eval(r, "const p = document.createElement('progress'); p.max"), "1");
    assert_eq!(eval(r, "p.setAttribute('max', '1.e2x'); p.max"), "100");
    assert_eq!(eval(r, "p.setAttribute('max', '1e999'); p.max"), "1");
    assert_eq!(eval(r, "p.setAttribute('max', '3'); p.max = -1; p.getAttribute('max')"), "3");
    assert_eq!(eval(r, "p.max = 1e25; p.getAttribute('max')"), "1e+25");
    assert_eq!(eval(r, "try { p.max = NaN } catch (x) { x.name }"), "TypeError");
    assert_eq!(
        eval(r, "const m = document.createElement('meter'); m.setAttribute('min', '2'); m.setAttribute('value', '1'); [m.min, m.max, m.value, m.low, m.high, m.optimum].join()"),
        "2,2,2,2,2,2"
    );
    // URLs, formAction and null-to-empty strings.
    assert_eq!(eval(r, "const n2 = document.createElement('input'); n2.formAction"), "http://localhost:5173/app");
    assert_eq!(eval(r, "n2.formAction = 'x'; n2.formAction"), "http://localhost:5173/x");
    assert_eq!(
        eval(r, "const fr = document.createElement('frame'); fr.marginWidth = null; fr.getAttribute('marginwidth')"),
        ""
    );
    // The elements that gained their own interface.
    for (tag, iface) in [
        ("marquee", "HTMLMarqueeElement"),
        ("frameset", "HTMLFrameSetElement"),
        ("frame", "HTMLFrameElement"),
        ("font", "HTMLFontElement"),
        ("dir", "HTMLDirectoryElement"),
        ("param", "HTMLParamElement"),
        ("track", "HTMLTrackElement"),
    ] {
        assert_eq!(eval(r, &format!("document.createElement('{tag}') instanceof {iface}")), "true", "{tag}");
    }
    // inputMode is global, nonce never reaches the content attribute.
    assert_eq!(eval(r, "const u = document.createElement('foo'); u.inputMode = 'TEL'; u.inputMode"), "tel");
    assert_eq!(eval(r, "u.nonce = 'a'; u.nonce + ':' + u.getAttribute('nonce')"), "a:null");
    assert_eq!(eval(r, "u.setAttribute('nonce', 'b'); u.nonce"), "b");
    assert_eq!(eval(r, "u.nonce = 'c'; u.nonce + ':' + u.getAttribute('nonce')"), "c:b");
    // Document color attributes and dir reflect the body and root element.
    assert_eq!(
        eval(r, "document.fgColor = 'red'; document.body.getAttribute('text') + ':' + document.body.text"),
        "red:red"
    );
    assert_eq!(eval(r, "document.bgColor = null; document.body.getAttribute('bgcolor')"), "");
    assert_eq!(
        eval(r, "document.dir = 'RTL'; document.dir + ':' + document.documentElement.getAttribute('dir')"),
        "rtl:RTL"
    );
    e.page.dispose();
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
fn controlled_unchanged_sanitized_values_preserve_selection() {
    let e = env("<body><input id=t value=abcd><textarea id=ta>ab\ncd</textarea></body>");
    assert_eq!(
        eval(
            &e.main,
            r#"(() => {
                const input = document.getElementById('t');
                const textarea = document.getElementById('ta');
                input.setSelectionRange(1, 3);
                textarea.setSelectionRange(1, 4);
                input.value = 'ab\ncd';
                textarea.value = 'ab\r\ncd';
                return JSON.stringify([input.value, input.selectionStart, input.selectionEnd,
                    textarea.value, textarea.selectionStart, textarea.selectionEnd]);
            })()"#,
        ),
        r#"["abcd",1,3,"ab\ncd",1,4]"#,
    );
}

#[test]
fn controlled_unchanged_values_still_set_the_dirty_value_flag() {
    let e = env("<body><input id=t value=abcd><textarea id=ta>abcd</textarea></body>");
    assert_eq!(
        eval(
            &e.main,
            r#"JSON.stringify(['t', 'ta'].map(id => {
                const control = document.getElementById(id);
                control.setSelectionRange(1, 3);
                control.value = control.value;
                control.defaultValue = 'replacement';
                return [control.value, control.defaultValue, control.selectionStart, control.selectionEnd];
            }))"#,
        ),
        r#"[["abcd","replacement",1,3],["abcd","replacement",1,3]]"#,
    );
}

#[test]
fn controlled_changed_values_reset_selection_to_the_utf16_end() {
    let e = env("<body><input id=t value=abcdef><textarea id=ta>abcdef</textarea></body>");
    assert_eq!(
        eval(
            &e.main,
            r#"JSON.stringify(['t', 'ta'].map(id => {
                const control = document.getElementById(id);
                control.setSelectionRange(1, 3);
                control.value = '\u{1f600}xy';
                control.defaultValue = 'replacement';
                return [control.value, control.selectionStart, control.selectionEnd];
            }))"#,
        ),
        r#"[["😀xy",4,4],["😀xy",4,4]]"#,
    );
}

#[test]
fn arrow_up_and_down_move_the_caret_in_text_controls() {
    let e = env("<body><input id=t value=hello><textarea id=ta></textarea><input id=n type=number value=5></body>");
    let r = &e.main;
    eval(r, "document.getElementById('ta').value = 'ab\\ncdef\\ng';");
    let page = e.page.clone();
    let press = |key: &str, shift: bool| {
        let k = input::KeyInput {
            key: key.into(),
            code: key.into(),
            key_code: if key == "ArrowUp" { 38 } else { 40 },
            modifiers: input::Modifiers { shift, ..Default::default() },
            ..Default::default()
        };
        input::dispatch_key(&page, input::KeyKind::RawKeyDown, &k);
        input::dispatch_key(&page, input::KeyKind::KeyUp, &k);
    };
    let sel =
        |id: &str| eval(r, &format!("(x => x.selectionStart + ',' + x.selectionEnd)(document.getElementById('{id}'))"));
    let set = |id: &str, s: u32, end: u32| {
        eval(r, &format!("(x => {{ x.focus(); x.setSelectionRange({s}, {end}); }})(document.getElementById('{id}'))"))
    };

    // Single-line input: Up to the start, Down to the end; Shift extends.
    set("t", 2, 2);
    press("ArrowUp", false);
    assert_eq!(sel("t"), "0,0");
    press("ArrowDown", false);
    assert_eq!(sel("t"), "5,5");
    set("t", 2, 2);
    press("ArrowDown", true);
    assert_eq!(sel("t"), "2,5");
    set("t", 2, 3);
    press("ArrowUp", true);
    assert_eq!(sel("t"), "0,3");

    // Textarea "ab|cdef|g": by logical line, column clamped; start/end past the edges.
    set("ta", 5, 5);
    press("ArrowUp", false);
    assert_eq!(sel("ta"), "2,2");
    press("ArrowUp", false);
    assert_eq!(sel("ta"), "0,0");
    set("ta", 1, 1);
    press("ArrowDown", false);
    assert_eq!(sel("ta"), "4,4");
    set("ta", 6, 6);
    press("ArrowDown", false);
    assert_eq!(sel("ta"), "9,9");
    press("ArrowDown", false);
    assert_eq!(sel("ta"), "9,9");
    set("ta", 4, 4);
    press("ArrowUp", true);
    assert_eq!(sel("ta"), "1,4");
    // Down with a selection moves from its end (offset 3, column 0 of line 2).
    set("ta", 0, 3);
    press("ArrowDown", false);
    assert_eq!(sel("ta"), "8,8");

    // A canceled keydown has no default action.
    eval(r, "document.getElementById('t').addEventListener('keydown', e => e.preventDefault());");
    set("t", 2, 2);
    press("ArrowDown", false);
    assert_eq!(sel("t"), "2,2");

    // Number inputs step values in browsers (not modelled): untouched here.
    eval(r, "document.getElementById('n').focus();");
    press("ArrowUp", false);
    assert_eq!(eval(r, "document.getElementById('n').value"), "5");
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
fn child_node_methods_accept_the_context_node_and_its_siblings() {
    // Upstream lightpanda-io/browser#4072 (replaceWith compliance): insertion is
    // relative to the nearest sibling that is not an argument, so passing the
    // node itself or a neighbour must not throw NotFoundError or drop nodes.
    let e = env("<body><p id=p></p></body>");
    let r = &e.main;
    eval(
        r,
        r#"function run(html, fn) {
            const p = document.getElementById('p'); p.innerHTML = html;
            try { fn(...p.children); return p.innerHTML; } catch (e) { return 'THROW ' + e.name + ' ' + p.innerHTML; }
        }"#,
    );
    for (call, want) in [
        ("(a) => a.replaceWith(a, 'x')", "<a></a>x<b></b>"),
        ("(a) => a.replaceWith('x', a)", "x<a></a><b></b>"),
        ("(a, b) => a.replaceWith(b, 't')", "<b></b>t"),
        ("(a) => a.replaceWith(a)", "<a></a><b></b>"),
        ("(a) => a.replaceWith()", "<b></b>"),
        ("(a, b) => a.after(b, 't')", "<a></a><b></b>t"),
        ("(a) => a.after(a, 'x')", "<a></a>x<b></b>"),
        ("(a, b) => b.before('x', b)", "<a></a>x<b></b>"),
        ("(a, b) => b.before(a, b)", "<a></a><b></b>"),
        ("(a, b) => a.before(b)", "<b></b><a></a>"),
    ] {
        assert_eq!(eval(r, &format!("run('<a></a><b></b>', {call})")), want, "{call}");
    }
    assert_eq!(eval(r, "run('<a></a><b></b><i></i>', (a, b, i) => a.after(b, 't'))"), "<a></a><b></b>t<i></i>");
    assert_eq!(eval(r, "run('<a></a><b></b><i></i>', (a, b, i) => i.before(a, b))"), "<a></a><b></b><i></i>");
    // Hierarchy errors still throw before the context node is touched.
    assert_eq!(
        eval(
            r,
            "const o = document.createElement('div'), t = o.appendChild(new Text('t'));
             try { t.replaceWith(o); 'no throw' } catch (e) { e.name + ':' + (t.parentNode === o) }"
        ),
        "HierarchyRequestError:true"
    );
    // A string argument's toString detaching the context node turns the call into a no-op.
    assert_eq!(
        eval(r, "run('<a></a><b></b>', (a) => a.replaceWith({ toString() { a.remove(); return 'x'; } }))"),
        "<b></b>"
    );
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
      window.before=sh.cssRules.every((r,i)=>r.cssText==='.r'+i+' { }');
      Object.defineProperty(Object.prototype,'cssText',{set(v){reviewGC(); Object.defineProperty(this,'cssText',{value:v});}, configurable:true});
      const rr=sh.cssRules; delete Object.prototype.cssText;
      before && rr.length === 200 && rr.every((r,i)=>r.cssText==='.r'+i+' { }')
    "#
        ),
        "true"
    );
}

#[test]
fn regression_typed_array_subview() {
    let e = env("<body>");
    assert_eq!(eval(&e.main, "new TextDecoder().decode(new Uint8Array([65,66,67]).subarray(1,2))"), "B");
    // A multi-byte sequence split across streaming calls, then flushed.
    assert_eq!(
        eval(
            &e.main,
            "{ const d = new TextDecoder(); const b = (...x) => new Uint8Array(x);
               d.decode(b(0xe2, 0x82), {stream: true}) + d.decode(b(0xac), {stream: true}) + '|' + d.decode(b(0xe2)) + '|' + d.decode(b(65)) }"
        ),
        "\u{20ac}|\u{fffd}|A"
    );
    // The label's own BOM is dropped; another encoding's BOM does not switch it.
    assert_eq!(eval(&e.main, "new TextDecoder('utf-16le').decode(new Uint8Array([0xff,0xfe,0x41,0])).length"), "1");
    assert_eq!(eval(&e.main, "new TextDecoder('windows-1252').decode(new Uint8Array([0xef,0xbb,0xbf])).length"), "3");
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
fn range_comparison_errors_follow_the_spec() {
    let e = env("<!doctype html><body><p id=x>abc</p></body>");
    let name = "(f) => { try { return String(f()); } catch (e) { return e.name; } }";
    eval(&e.main, &format!("var t = {name}; var x = document.getElementById('x'); var r = new Range(); r.selectNode(x); var d = document.createElement('div'); 1"));
    assert_eq!(eval(&e.main, "t(() => r.comparePoint(d, 0))"), "WrongDocumentError");
    assert_eq!(eval(&e.main, "t(() => r.comparePoint(x, 9))"), "IndexSizeError");
    assert_eq!(eval(&e.main, "t(() => r.comparePoint(document.doctype, 0))"), "InvalidNodeTypeError");
    assert_eq!(eval(&e.main, "t(() => r.isPointInRange(d, 9))"), "false");
    assert_eq!(eval(&e.main, "t(() => r.isPointInRange(x, 9))"), "IndexSizeError");
    assert_eq!(eval(&e.main, "t(() => r.intersectsNode(d))"), "false");
    assert_eq!(eval(&e.main, "t(() => r.compareBoundaryPoints(7, r))"), "NotSupportedError");
    assert_eq!(eval(&e.main, "t(() => r.compareBoundaryPoints(65536, r))"), "0");
    assert_eq!(
        eval(&e.main, "var o = new Range(); o.selectNode(d.appendChild(document.createElement('i'))); t(() => r.compareBoundaryPoints(0, o))"),
        "WrongDocumentError"
    );
    // A boundary point in another tree collapses the range onto it.
    assert_eq!(eval(&e.main, "r.setEnd(d, 0); [r.startContainer === d, r.collapsed].join()"), "true,true");
}

#[test]
fn range_content_methods() {
    let e = env("<body><div id=x><b>abc</b>mid<i>def</i></div></body>");
    eval(&e.main, "var x = document.getElementById('x'); var r = new Range(); var html = (f) => { const d = document.createElement('div'); d.append(f); return d.innerHTML; }; r.setStart(x.firstChild.firstChild, 1); r.setEnd(x.lastChild.firstChild, 2); 1");
    assert_eq!(eval(&e.main, "html(r.cloneContents())"), "<b>bc</b>mid<i>de</i>");
    assert_eq!(eval(&e.main, "x.innerHTML"), "<b>abc</b>mid<i>def</i>");
    assert_eq!(eval(&e.main, "html(r.extractContents())"), "<b>bc</b>mid<i>de</i>");
    assert_eq!(eval(&e.main, "x.innerHTML"), "<b>a</b><i>f</i>");
    assert_eq!(eval(&e.main, "[r.startContainer === x, r.startOffset, r.collapsed].join()"), "true,1,true");
    assert_eq!(
        eval(&e.main, "r.insertNode(document.createElement('u')); [x.innerHTML, r.startOffset, r.endOffset].join()"),
        "<b>a</b><u></u><i>f</i>,1,2"
    );
    assert_eq!(eval(&e.main, "r.deleteContents(); [x.innerHTML, r.collapsed].join()"), "<b>a</b><i>f</i>,true");
    assert_eq!(eval(&e.main, "r.selectNode(x.firstChild); r.surroundContents(document.createElement('s')); [x.innerHTML, r.startOffset, r.endOffset].join()"), "<s><b>a</b></s><i>f</i>,0,1");
    assert_eq!(eval(&e.main, "r.setStart(x.firstChild.firstChild, 0); r.setEnd(x, 2); try { r.surroundContents(document.createElement('s')); } catch (err) { err.name }"), "InvalidStateError");
    // insertNode into text splits it.
    assert_eq!(eval(&e.main, "x.textContent = 'hello'; r.setStart(x.firstChild, 2); r.collapse(true); r.insertNode(document.createElement('b')); [x.innerHTML, r.endContainer === x, r.endOffset].join()"), "he<b></b>llo,true,2");
}

#[test]
fn range_boundaries_follow_mutations() {
    let e = env("<body><div id=x>abcdef</div><div id=y><a></a><b></b><i></i></div></body>");
    let pts = "var pts = () => [r.startContainer === t ? 't' : r.startContainer.nodeName, r.startOffset, r.endContainer === t ? 't' : r.endContainer.nodeName, r.endOffset].join();";
    eval(&e.main, &format!("var x = document.getElementById('x'), y = document.getElementById('y'), t = x.firstChild, r = new Range(); {pts} r.setStart(t, 2); r.setEnd(t, 5); 1"));
    assert_eq!(eval(&e.main, "t.deleteData(0, 1); pts()"), "t,1,t,4");
    assert_eq!(eval(&e.main, "t.insertData(0, 'zz'); pts()"), "t,3,t,6");
    assert_eq!(eval(&e.main, "t.replaceData(1, 4, 'q'); pts()"), "t,1,t,3");
    assert_eq!(eval(&e.main, "t.data = 'abcdef'; pts()"), "t,0,t,0");
    assert_eq!(
        eval(&e.main, "r.setEnd(t, 4); var u = t.splitText(2); [r.endContainer === u, r.endOffset].join()"),
        "true,2"
    );
    assert_eq!(eval(&e.main, "x.normalize(); pts()"), "t,0,t,4");
    assert_eq!(eval(&e.main, "x.remove(); document.body.append(x); pts()"), "BODY,0,BODY,0");
    assert_eq!(
        eval(&e.main, "r.setStart(y, 2); r.setEnd(y, 3); y.prepend(document.createElement('u')); pts()"),
        "DIV,3,DIV,4"
    );
    assert_eq!(eval(&e.main, "y.firstChild.remove(); pts()"), "DIV,2,DIV,3");
    assert_eq!(eval(&e.main, "y.children[1].remove(); pts()"), "DIV,1,DIV,2");
    assert_eq!(eval(&e.main, "y.innerHTML = ''; pts()"), "DIV,0,DIV,0");
}

#[test]
fn static_range_keeps_its_points_as_given() {
    let e = env("<body><div id=x>abc</div></body>");
    let init = "{startContainer: x, startOffset: 5, endContainer: x.firstChild, endOffset: 1}";
    assert_eq!(eval(&e.main, &format!("var x = document.getElementById('x'); var r = new StaticRange({init}); x.remove(); [r.startContainer === x, r.startOffset, r.endOffset, r.collapsed].join()")), "true,5,1,false");
    assert_eq!(eval(&e.main, "try { new StaticRange({startContainer: x}) } catch (err) { err.name }"), "TypeError");
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
fn storage_seed_preloads_local_storage_per_origin() {
    let engine = Engine::new().unwrap();
    register(&engine);
    let seed = StorageSeed::new(vec![
        // A URL normalizes to its origin; the default port is dropped.
        ("http://example.test:80/app/index.html".into(), vec![("token".into(), "synthetic".into())]),
        ("http://example.test".into(), vec![("theme".into(), "dark".into()), ("token".into(), "last".into())]),
        ("http://other.test:8080".into(), vec![("k".into(), "v".into())]),
    ])
    .unwrap();
    assert_eq!(seed.summary(), vec![("http://example.test".to_string(), 2), ("http://other.test:8080".to_string(), 1)]);
    assert!(StorageSeed::new(vec![("data:text/html,x".into(), vec![])]).is_err());
    assert!(StorageSeed::new(vec![("not a url".into(), vec![])]).is_err());
    assert!(StorageSeed::default().is_empty());

    let local = StorageNamespace::seeded(&seed);
    assert_eq!(local.get("http://example.test", "token").as_deref(), Some("last"));
    let (_p1, r1) = storage_page(&engine, "http://example.test/a", local.clone(), StorageNamespace::default());
    assert_eq!(
        eval(&r1, "localStorage.length + ':' + localStorage.token + ':' + localStorage.getItem('theme')"),
        "2:last:dark"
    );
    // Seeded bytes count against the quota of later writes.
    assert_eq!(eval(&r1, "localStorage.setItem('more', 'x'); localStorage.length"), "3");
    let (_p2, r2) = storage_page(&engine, "http://other.test:8080/", local.clone(), StorageNamespace::default());
    assert_eq!(eval(&r2, "localStorage.k + ':' + localStorage.getItem('token')"), "v:null");
    // Another namespace from the same seed is an independent copy.
    let (_p3, r3) =
        storage_page(&engine, "http://example.test/", StorageNamespace::seeded(&seed), StorageNamespace::default());
    assert_eq!(eval(&r3, "localStorage.length + ':' + localStorage.getItem('more')"), "2:null");
    // Unseeded namespaces are unaffected.
    let (_p4, r4) =
        storage_page(&engine, "http://example.test/", StorageNamespace::default(), StorageNamespace::default());
    assert_eq!(eval(&r4, "localStorage.length"), "0");
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
    // Opaque origin: per-document ephemeral area (upstream compatibility),
    // not the shared namespace.
    assert_eq!(eval(&r6, "localStorage.token='opaque'; localStorage.length"), "1");
    assert_eq!(eval(&r5, "localStorage.length"), "0");
}

#[test]
fn opaque_origin_storage_is_per_document_and_discarded_on_replacement() {
    let engine = Engine::new().unwrap();
    register(&engine);
    let local = StorageNamespace::with_quota(12);
    let session = StorageNamespace::default();
    let (p1, r1) = storage_page(&engine, "data:text/html,a", local.clone(), session.clone());
    let w1 = new_realm(&engine, engine.register::<TestWindow>(), &p1, "isolated");
    let (_p2, r2) = storage_page(&engine, "about:blank", local.clone(), session.clone());
    let (_p3, r3) = storage_page(&engine, "http://example.test/", local.clone(), session.clone());
    assert_eq!(
        eval(&r1, "localStorage.setItem('k','1'); sessionStorage.setItem('s','2'); [localStorage===localStorage, localStorage!==sessionStorage, localStorage.k, sessionStorage.s].join()"),
        "true,true,1,2"
    );
    // Shared by the worlds of the same document only.
    assert_eq!(eval(&w1, "localStorage.k + ':' + sessionStorage.s"), "1:2");
    // Never a common "null" bucket, and never the tuple-origin namespace.
    assert_eq!(eval(&r2, "localStorage.length + ':' + sessionStorage.length + ':' + localStorage.k"), "0:0:undefined");
    assert_eq!(eval(&r3, "localStorage.length + ':' + sessionStorage.length"), "0:0");
    // Quota still applies (namespace quota of 12 UTF-16 bytes).
    assert_eq!(eval(&r2, "try{localStorage.setItem('abc','defg')}catch(e){e.name}"), "QuotaExceededError");
    // Document replacement discards the areas, including through retained
    // Storage objects: they read as empty and reject writes.
    eval(&r1, "var oldLocal = localStorage, oldSession = sessionStorage;");
    let doc = parse_document(&p1.dom, "data:text/html,b", "<body>");
    p1.set_document(doc);
    assert_eq!(eval(&r1, "localStorage.length + ':' + sessionStorage.length + ':' + localStorage.k"), "0:0:undefined");
    assert_eq!(
        eval(&r1, "[oldLocal !== localStorage, oldLocal.length, oldLocal.getItem('k'), oldLocal.k, oldLocal.key(0), Object.keys(oldLocal).length, oldSession.length, oldSession.getItem('s')].map(String).join()"),
        "true,0,null,undefined,null,0,0,null"
    );
    assert_eq!(
        eval(&r1, "[(()=>{try{oldLocal.setItem('k','x')}catch(e){return e.name}})(), (()=>{try{oldSession.z='y'}catch(e){return e.name}})(), (oldLocal.removeItem('k'), oldLocal.clear(), oldLocal.length), localStorage.length].join()"),
        "InvalidStateError,InvalidStateError,0,0"
    );
    // The replacement document's area is independent of the retired one.
    assert_eq!(eval(&r1, "localStorage.k='2'; oldLocal.length + ':' + localStorage.k"), "0:2");
    // Other opaque-origin restrictions are unchanged.
    assert_eq!(eval(&r1, "try{indexedDB.open('db')}catch(e){e.name}"), "SecurityError");
    // Disposal retires the current areas too.
    eval(&w1, "var kept = localStorage;");
    assert_eq!(eval(&w1, "kept.k"), "2");
    p1.dispose();
    assert_eq!(
        eval(
            &w1,
            "kept.length + ':' + kept.getItem('k') + ':' + (()=>{try{kept.setItem('k','3')}catch(e){return e.name}})()"
        ),
        "0:null:InvalidStateError"
    );
    // Tuple-origin storage sharing the same namespaces is unaffected.
    assert_eq!(eval(&r3, "localStorage.setItem('n','1'); localStorage.n"), "1");
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

/// The default quota is Chromium's: 10 MiB per origin of keys and values
/// counted as UTF-16 (about 5.2 M characters), for local and session storage.
#[test]
fn storage_default_quota_matches_chromium() {
    let engine = Engine::new().unwrap();
    register(&engine);
    let (_p, r) =
        storage_page(&engine, "http://example.test/", StorageNamespace::default(), StorageNamespace::default());
    for area in ["localStorage", "sessionStorage"] {
        assert_eq!(
            eval(
                &r,
                &format!(
                    r#"
            {area}.setItem('k', 'x'.repeat(5e6));
            var failure = 'none';
            try {{ {area}.setItem('k', 'x'.repeat(5.3e6)) }} catch (e) {{ failure = e.name }}
            failure + ':' + {area}.getItem('k').length + ':' + {area}.length
        "#
                )
            ),
            "QuotaExceededError:5000000:1"
        );
    }
}

#[test]
fn storage_is_shared_across_worlds_and_inherited_blank_origin() {
    let e = env("<body>");
    let isolated = new_realm(&e.engine, e.engine.register::<TestWindow>(), &e.page, "isolated");
    assert_eq!(eval(&e.main, "localStorage.setItem('shared','yes'); localStorage.shared"), "yes");
    assert_eq!(eval(&isolated, "localStorage.shared"), "yes");
    let document = e.page.document();
    e.page.dom.borrow_mut().set_document_url(document, "about:blank").unwrap();
    // Opaque about:blank (no inherited origin): per-document area, empty.
    assert_eq!(eval(&isolated, "sessionStorage.length + ':' + localStorage.getItem('shared')"), "0:null");
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
    assert_eq!(eval(&e.main, "URL.createObjectURL(new Blob()).startsWith('blob:http://localhost:5173/')"), "true");
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
    fn post_delayed_task(&self, delay: std::time::Duration, task: Box<dyn FnOnce()>) {
        // Single-threaded test loop: let the delay elapse, then queue.
        std::thread::sleep(delay);
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
fn window_message_source_allows_task_schedulers_in_each_world() {
    let e = env("<body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    let class = e.engine.register::<TestWindow>();
    let isolated = new_realm(&e.engine, class, &e.page, "message-source");
    for realm in [&e.main, &isolated] {
        assert_eq!(
            eval(
                realm,
                r#"
            globalThis.delivered = [];
            addEventListener('message', event => {
                if (event.source === window && event.data === 'scheduler-task') {
                    globalThis.retainedMessage = event;
                    delivered.push(event.isTrusted && event.target === window);
                }
            });
            new MessageEvent('message').source === null
        "#
            ),
            "true"
        );
    }
    eval(&e.main, "postMessage('scheduler-task', '*')");
    assert_eq!(eval(&e.main, "delivered.length"), "0", "delivery remains a separate task");
    tasks.drain();
    for realm in [&e.main, &isolated] {
        assert_eq!(eval(realm, "delivered.join()"), "true");
        assert_eq!(eval(realm, "retainedMessage.source === window"), "true");
    }
}

#[test]
fn window_message_source_does_not_root_a_released_event_cycle() {
    let e = env("<body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    eval(
        &e.main,
        r#"
        addEventListener('message', event => {
            globalThis.eventRef = new WeakRef(event);
            event.source.retainedMessage = event;
        }, {once:true});
        postMessage('cycle', '*');
    "#,
    );
    tasks.drain();
    assert_eq!(eval(&e.main, "retainedMessage.source === window"), "true");
    eval(&e.main, "retainedMessage = null; 1");
    for _ in 0..6 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
    }
    assert_eq!(eval(&e.main, "eventRef.deref() === undefined"), "true");
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
    assert_eq!(
        eval(&e.main, "records.length===1 && records[0].isIntersecting && records[0].intersectionRatio===.25"),
        "true"
    );
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
fn web_crypto_random_and_digest() {
    let e = env("<body></body>");
    let r = &e.main;
    assert_eq!(
        eval(r, "const u=crypto.randomUUID(); [/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(u), u!==crypto.randomUUID(), crypto===window.crypto, crypto instanceof Crypto].join()"),
        "true,true,true,true"
    );
    assert_eq!(
        eval(r, "const a=new Uint32Array(8); const back=crypto.getRandomValues(a); back===a && a.some(x=>x!==0)"),
        "true"
    );
    // Subview bounds: bytes outside [8, 12) keep a sentinel. The random
    // bytes themselves may legitimately be any value (including 0xAA), so
    // only the untouched region is asserted.
    assert_eq!(
        eval(r, "const big=new Uint8Array(32).fill(0xaa); const view=new Uint8Array(big.buffer, 8, 4); const ret=crypto.getRandomValues(view); ret===view && big.subarray(0,8).every(x=>x===0xaa) && big.subarray(12).every(x=>x===0xaa)"),
        "true"
    );
    // A fresh-buffer write lands in the view: across 64 four-byte views the
    // chance of every random byte equalling the sentinel is 2^-2048.
    assert_eq!(
        eval(r, "let changed=false; for (let i=0;i<64 && !changed;i++){const b=new Uint8Array(16).fill(0xaa); crypto.getRandomValues(new Uint8Array(b.buffer,4,4)); changed=b.subarray(4,8).some(x=>x!==0xaa);} changed"),
        "true"
    );
    assert_eq!(eval(r, "try{crypto.getRandomValues(new Float32Array(2))}catch(e){e.name}"), "TypeMismatchError");
    assert_eq!(eval(r, "try{crypto.getRandomValues(new Uint8Array(65537))}catch(e){e.name}"), "QuotaExceededError");
    assert_eq!(eval(r, "crypto.getRandomValues(new Uint8Array(65536)).length"), "65536");
    let out = eval(
        r,
        r#"globalThis.res='pending';
        Promise.all([
          crypto.subtle.digest('SHA-256', new TextEncoder().encode('abc')),
          crypto.subtle.digest({name:'sha-1'}, new Uint8Array([97,98,99])),
          crypto.subtle.digest('MD5', new Uint8Array(1)).then(()=>'no',e=>e.name),
          crypto.subtle.sign().then(()=>'no',e=>e.name),
        ]).then(([a,b,md5,sign])=>{const hex=x=>[...new Uint8Array(x)].map(v=>v.toString(16).padStart(2,'0')).join('');
          res=[hex(a),hex(b),md5,sign,crypto.subtle===crypto.subtle].join('|')});
        'started'"#,
    );
    assert_eq!(out, "started");
    assert_eq!(
        eval(r, "res"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad|a9993e364706816aba3e25717850c26c9cd0d89d|NotSupportedError|NotSupportedError|true"
    );
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
fn resize_observer_reports_the_flow_content_box() {
    // @povio/ui sizes select popovers from a ResizeObserver contentRect; the
    // faux axis (5 px for an auto-width block) clipped every option. Under
    // the flow layout the entry is the content box, as in Chromium.
    let e = env(
        "<body style='margin:0'><div style='width:300px'><div id=a style='padding:4px 6px;border:2px solid'>x</div></div></body>",
    );
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    eval(
        &e.main,
        "window.seen=[]; new ResizeObserver(l => { for (const en of l) seen.push(en.contentRect.width + 'x' + en.contentRect.height) }).observe(document.getElementById('a'))",
    );
    e.page.after_task();
    assert!(e.page.pending_resize_observer_work(), "queued initial delivery must prevent quiescence");
    tasks.drain();
    assert!(!e.page.pending_resize_observer_work(), "completed delivery must not prevent quiescence");
    // 300 - 2 * (6 padding + 2 border) wide; one line tall.
    let seen = eval(&e.main, "seen.join()");
    assert!(seen.starts_with("284x"), "{seen}");
    assert_eq!(eval(&e.main, "document.getElementById('a').clientWidth"), "296");
    // Gathers are paced (each one lays out the document); a change right
    // after a gather is still delivered by the delayed wake.
    eval(&e.main, "document.getElementById('a').parentElement.style.width='200px'");
    e.page.after_task();
    assert!(e.page.pending_resize_observer_work(), "paced gather must prevent quiescence until its wake runs");
    tasks.drain();
    assert!(!e.page.pending_resize_observer_work(), "paced gather and delivery finished");
    let seen = eval(&e.main, "seen.join()");
    assert!(seen.contains(",184x"), "{seen}");
    e.page.dispose();
}

#[test]
fn resize_observer_entries_report_each_box_and_skip_disconnected_targets() {
    // Gap 47: a 150px content-box div with 4px padding reported
    // borderBoxSize 142 (Chromium 158), and a detached target that was never
    // unobserved got an entry with contentRect.width 0.
    let e = env(
        "<body style='margin:0'><div id=a style='width:150px;height:20px;padding:4px 4px 2px;border:1px solid'></div><div id=b style='width:30px;height:10px'></div></body>",
    );
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    eval(
        &e.main,
        "window.seen=[]; window.a = document.getElementById('a'); window.b = document.getElementById('b'); const box = s => s[0].inlineSize + 'x' + s[0].blockSize;
         window.ro = new ResizeObserver(l => { for (const en of l) { const r = en.contentRect;
           seen.push(en.target.id + ':' + [r.x, r.y, r.width, r.height].join(',') + ':' + box(en.contentBoxSize) + ':' + box(en.borderBoxSize) + ':' + box(en.devicePixelContentBoxSize)) } });
         ro.observe(a); ro.observe(b, {box: 'border-box'}); 'ok'",
    );
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "seen.join(' ')"), "a:4,4,150,20:150x20:160x28:150x20 b:0,0,30,10:30x10:30x10:30x10");
    // Padding grows the border box only: no entry for a content-box observation.
    assert_eq!(eval(&e.main, "seen.length = 0; a.style.padding = '8px'"), "8px");
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "seen.join(' ')"), "");
    // ... but one for a border-box observation.
    assert_eq!(eval(&e.main, "b.style.padding = '1px'"), "1px");
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "seen.join(' ')"), "b:1,1,30,10:30x10:32x12:30x10");
    // A removed target that is still observed delivers nothing.
    assert_eq!(eval(&e.main, "seen.length = 0; a.style.width = '10px'; a.remove(); a.isConnected"), "false");
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "seen.length"), "0");
    e.page.dispose();
}

#[test]
fn element_metrics_are_whole_pixels_like_chromium() {
    // Gap 36: a chip label reported scrollWidth 44.7135000001 > clientWidth
    // 44.7135 and was taken for truncated. Chromium's scroll, client and
    // offset metrics are integers: offsets round, sizes snap at the box's
    // offset (round(left + width) - round(left)).
    let e = env(
        "<body style='margin:0'><div id=a style='width:44.7135px;overflow:hidden;white-space:nowrap'><span style='display:inline-block;width:44.7135px'></span></div>\
         <div id=b style='margin-left:10.6px;width:20.6px;height:10.4px;padding-top:0.3px'></div></body>",
    );
    assert_eq!(
        eval(&e.main, "var a = document.getElementById('a'); [a.scrollWidth, a.clientWidth, a.offsetWidth].join()"),
        "45,45,45"
    );
    assert_eq!(
        eval(
            &e.main,
            "var b = document.getElementById('b'); [b.offsetLeft, b.offsetWidth, b.clientWidth, b.scrollWidth, b.offsetHeight, b.clientHeight].join()"
        ),
        "11,20,20,20,11,11"
    );
    // getBoundingClientRect keeps the fractional geometry.
    assert_eq!(eval(&e.main, "b.getBoundingClientRect().width"), "20.59375");
    e.page.dispose();
}

#[test]
fn svg_graphics_elements_report_get_bbox() {
    // TOS analytics reads recharts bar geometry with getBBox().
    let e = env(
        "<body><svg id=s width=600 height=300><g id=g transform='translate(5,5)'><path id=p d='M202,238A 4,4,0,0,0,206,242L 300,242A 4,4,0,0,0,304,238L 304,184L 202,184 Z' fill='#38bdf8'/></g><linearGradient id='lg'></linearGradient></svg></body>",
    );
    assert_eq!(
        eval(
            &e.main,
            "var b = document.getElementById('p').getBBox(); [b instanceof DOMRect, b.x, b.y, b.width, b.height, document.getElementById('p') instanceof SVGGraphicsElement, document.getElementById('s') instanceof SVGGraphicsElement, document.getElementById('lg') instanceof SVGGraphicsElement, typeof document.getElementById('lg').getBBox].join()"
        ),
        "true,202,184,102,58,true,true,false,undefined"
    );
    // A container's box is in its own user space: its own transform is excluded.
    assert_eq!(eval(&e.main, "var g = document.getElementById('g').getBBox(); [g.x, g.y].join()"), "202,184");
    e.page.dispose();
}

#[test]
fn canvas_2d_measures_text_without_pixels() {
    // TOS terms-layout sizes Days inputs with canvas measureText. The 2D
    // context measures with the flow layout's text measure; there is no
    // rasterizer, so pixel readback is explicitly unsupported.
    let e = env("<body><canvas id=c></canvas><input id=i style='font: 600 15px system-ui'></body>");
    assert_eq!(
        eval(
            &e.main,
            "var c = document.getElementById('c'), x = c.getContext('2d'); [x === c.getContext('2d'), x.canvas === c, x instanceof CanvasRenderingContext2D, c.getContext('webgl'), x.font].join('|')"
        ),
        "true|true|true||10px sans-serif"
    );
    assert_eq!(eval(&e.main, "x.font = 'nonsense'; x.font"), "10px sans-serif");
    assert_eq!(eval(&e.main, "x.font = 'bold 16px / 24px Inter, sans-serif'; x.font"), "700 16px Inter, sans-serif");
    // Wider text measures wider; weight and size matter (the SF Pro table
    // varies by optical size, so widths are not exactly proportional).
    assert_eq!(
        eval(
            &e.main,
            "x.font = '16px system-ui'; var a = x.measureText('999').width, b = x.measureText('9999').width; x.font = '700 16px system-ui'; var w = x.measureText('999').width; x.font = '32px system-ui'; var d = x.measureText('999').width; [a > 20 && a < 40, b > a, w > a, d > 1.8 * a].join()"
        ),
        "true,true,true,true"
    );
    assert_eq!(eval(&e.main, "x.save(); x.font = '20px monospace'; x.restore(); x.font"), "32px system-ui");
    assert_eq!(
        eval(&e.main, "try { x.getImageData(0, 0, 1, 1); 'returned' } catch (err) { err.name }"),
        "NotSupportedError"
    );
    // The TOS pattern: drawing.font = getComputedStyle(input).font.
    assert_eq!(
        eval(&e.main, "x.font = getComputedStyle(document.getElementById('i')).font; x.font"),
        "600 15px system-ui"
    );
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

#[test]
fn form_data_from_form_and_submitter() {
    let e = env("<body><form id=f><input name=q value=1><input type=checkbox name=c checked><input type=checkbox name=d><select name=s><option>x</option><option selected>y</option></select><textarea name=t>a\nb</textarea><button id=b name=go value=yes>go</button></form><button id=other>o</button></body>");
    let r = &e.main;
    assert_eq!(
        eval(r, "const f=document.getElementById('f'); [...new FormData(f)].map(p=>p.join('=')).join('&')"),
        "q=1&c=on&s=y&t=a\r\nb"
    );
    assert_eq!(eval(r, "new FormData(f, document.getElementById('b')).get('go')"), "yes");
    assert_eq!(eval(r, "try{new FormData(f, document.getElementById('other'))}catch(e){e.name}"), "NotFoundError");
}

#[test]
fn dom_parser_parses_inert_html_documents() {
    let e = env("<body><div id=host></div></body>");
    let r = &e.main;
    assert_eq!(
        eval(r, "const d=new window.DOMParser().parseFromString('<body><p class=x>hi<b>!</b></p>\\n  <script>window.ran=1</script></body>','text/html'); [d instanceof Document, d!==document, d.URL===document.URL, d.contentType, d.body.firstChild.outerHTML, typeof window.ran].join()"),
        "true,true,true,text/html,<p class=\"x\">hi<b>!</b></p>,undefined"
    );
    assert_eq!(
        eval(r, "d.querySelectorAll('p').length + ':' + d.title + ':' + d.documentElement.localName"),
        "1::html"
    );
    assert_eq!(
        eval(r, "const p=document.importNode(d.querySelector('p'),true); document.getElementById('host').appendChild(p); document.getElementById('host').textContent"),
        "hi!"
    );
    assert_eq!(eval(r, "new DOMParser().parseFromString('<a/>','application/xml').documentElement.nodeName"), "a");
    assert_eq!(eval(r, "try{new DOMParser().parseFromString('x','text/plain')}catch(e){e.name}"), "TypeError");
    assert_eq!(eval(r, "try{DOMParser()}catch(e){e.name}"), "TypeError");
}

#[test]
fn element_click_is_a_virtual_pointer_click() {
    let e = env("<body><button id=b>go</button></body>");
    let r = &e.main;
    assert_eq!(
        eval(r, "let seen; const b=document.getElementById('b'); b.addEventListener('click', e => seen=[e.constructor.name, e.detail, e.button, e.buttons, e.pointerId, JSON.stringify(e.pointerType), e.isTrusted, e.bubbles, e.cancelable, e.composed].join()); b.click(); seen"),
        "PointerEvent,0,0,0,-1,\"\",false,true,true,true"
    );
}

// XPath: ports of upstream src/browser/tests/xpath/{document_evaluate,
// xpath_result,xpath_evaluator}.html plus the Playwright `xpath=` shape.
const XPATH_PAGE: &str = r#"<!doctype html><body>
<h1 id=title>Hello</h1>
<div class=group><p id=p1 class=x>First</p><p id=p2>Second</p><p id=p3 data-n=3>Third</p></div>
<span id=span1 data-x="42">x</span>
<div class="h-[2.625rem] outer"><div class="flex h-[2.625rem] row" id=row><div class=cell><span>Customer</span></div></div></div>
<svg id=s><rect id=r /></svg>
</body>"#;

#[test]
fn xpath_playwright_ancestor_locator() {
    let e = env(XPATH_PAGE);
    let r = &e.main;
    // Playwright's injected `xpath=` engine: ORDERED_NODE_ITERATOR_TYPE + iterateNext().
    assert_eq!(
        eval(
            r,
            r#"const pwRoot = [...document.querySelectorAll('span')].filter(s => s.textContent === 'Customer').pop();
               const pwIt = document.evaluate("ancestor::div[contains(@class, 'h-[2.625rem]')][1]", pwRoot, null,
                   XPathResult.ORDERED_NODE_ITERATOR_TYPE, null);
               const pwOut = [];
               for (let n = pwIt.iterateNext(); n; n = pwIt.iterateNext()) pwOut.push(n.id);
               pwIt.resultType + ':' + pwOut.join(',') + ':' + pwIt.invalidIteratorState"#
        ),
        "5:row:false"
    );
}

#[test]
fn xpath_result_types() {
    let e = env(XPATH_PAGE);
    let r = &e.main;
    assert_eq!(
        eval(r, "['ANY_TYPE', 'NUMBER_TYPE', 'STRING_TYPE', 'BOOLEAN_TYPE', 'UNORDERED_NODE_ITERATOR_TYPE', 'ORDERED_NODE_ITERATOR_TYPE', 'UNORDERED_NODE_SNAPSHOT_TYPE', 'ORDERED_NODE_SNAPSHOT_TYPE', 'ANY_UNORDERED_NODE_TYPE', 'FIRST_ORDERED_NODE_TYPE'].map(k => XPathResult[k]).join()"),
        "0,1,2,3,4,5,6,7,8,9"
    );
    assert_eq!(eval(r, "XPathResult.prototype.ORDERED_NODE_SNAPSHOT_TYPE"), "7");
    assert_eq!(eval(r, "try { new XPathResult() } catch (x) { x instanceof TypeError }"), "true");
    // Snapshots.
    assert_eq!(
        eval(r, "const snap = document.evaluate('//p', document, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null); snap instanceof XPathResult && [snap.resultType, snap.snapshotLength, snap.snapshotItem(0).id, snap.snapshotItem(2).id, snap.snapshotItem(3)].join()"),
        "7,3,p1,p3,"
    );
    assert_eq!(
        eval(r, "snap.snapshotItem(0) === document.getElementById('p1') && snap.ORDERED_NODE_SNAPSHOT_TYPE === 7"),
        "true"
    );
    assert_eq!(
        eval(
            r,
            "document.evaluate('//p', document, null, XPathResult.UNORDERED_NODE_SNAPSHOT_TYPE, null).snapshotLength"
        ),
        "3"
    );
    // Single node.
    assert_eq!(
        eval(
            r,
            "document.evaluate('//p', document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue.id"
        ),
        "p1"
    );
    assert_eq!(
        eval(
            r,
            "document.evaluate('//nope', document, null, XPathResult.ANY_UNORDERED_NODE_TYPE, null).singleNodeValue"
        ),
        "null"
    );
    // Scalars and ANY_TYPE resolution.
    assert_eq!(
        eval(r, "document.evaluate('count(//p)', document, null, XPathResult.NUMBER_TYPE, null).numberValue"),
        "3"
    );
    assert_eq!(
        eval(r, "document.evaluate('string(//h1)', document, null, XPathResult.STRING_TYPE, null).stringValue"),
        "Hello"
    );
    assert_eq!(
        eval(r, "document.evaluate('count(//p) > 2', document, null, XPathResult.BOOLEAN_TYPE, null).booleanValue"),
        "true"
    );
    assert_eq!(
        eval(r, "document.evaluate('//h1', document, null, XPathResult.STRING_TYPE, null).stringValue"),
        "Hello"
    );
    assert_eq!(
        eval(r, "document.evaluate('//p[3]/@data-n', document, null, XPathResult.NUMBER_TYPE, null).numberValue"),
        "3"
    );
    assert_eq!(
        eval(r, "document.evaluate('//p', document, null, XPathResult.BOOLEAN_TYPE, null).booleanValue"),
        "true"
    );
    assert_eq!(
        eval(r, "['//p', 'count(//p)', 'string(//h1)', 'true()'].map(x => document.evaluate(x, document, null, XPathResult.ANY_TYPE, null).resultType).join()"),
        "4,1,2,3"
    );
    assert_eq!(eval(r, "document.evaluate('//p', document).iterateNext().id"), "p1");
    assert_eq!(eval(r, "document.evaluate('//p', null, null, 7, null).snapshotLength"), "3");
    assert_eq!(
        eval(r, "const ex = document.evaluate('//p', document, null, 5, null); [ex.iterateNext().id, ex.iterateNext().id, ex.iterateNext().id, ex.iterateNext(), ex.iterateNext()].join()"),
        "p1,p2,p3,,"
    );
    // Type mismatches are TypeErrors.
    assert_eq!(
        eval(r, "const mm = document.evaluate('//p', document, null, 7, null); let mmc = 0; for (const k of ['numberValue', 'stringValue', 'booleanValue', 'singleNodeValue']) { try { mm[k] } catch (x) { if (x instanceof TypeError) mmc++ } } try { mm.iterateNext() } catch (x) { if (x instanceof TypeError) mmc++ } mmc"),
        "5"
    );
    assert_eq!(
        eval(r, "try { document.evaluate('count(//p)', document, null, XPathResult.ORDERED_NODE_ITERATOR_TYPE, null) } catch (x) { x.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        eval(
            r,
            "try { document.evaluate('1', document, null, 3, null).snapshotLength } catch (x) { x.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        eval(r, "try { document.evaluate('//p', document, null, 42, null) } catch (x) { x.name }"),
        "NotSupportedError"
    );
}

#[test]
fn xpath_axes_predicates_functions() {
    let e = env(XPATH_PAGE);
    let r = &e.main;
    let snap = |expr: &str| {
        eval(
            r,
            &format!(
                "(() => {{ const s = document.evaluate({expr:?}, document, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null); const o = []; for (let i = 0; i < s.snapshotLength; i++) {{ const n = s.snapshotItem(i); o.push(n.id || n.nodeName) }} return o.join() }})()"
            ),
        )
    };
    // The attribute axis yields Attr objects.
    assert_eq!(
        eval(r, "const at = document.evaluate('//span/@data-x', document, null, 9, null).singleNodeValue; [at instanceof Attr, at.name, at.value, at.ownerElement.id].join()"),
        "true,data-x,42,span1"
    );
    assert_eq!(snap("//p[1]/@*"), "id,class");
    assert_eq!(eval(r, "document.evaluate('//span/@data-x', document, null, 2, null).stringValue"), "42");
    // Union is in document order.
    assert_eq!(snap("//span[@id='span1'] | //h1 | //p[2]"), "title,p2,span1");
    // Positional predicates, position()/last(), reverse axes.
    assert_eq!(snap("//p[last()]"), "p3");
    assert_eq!(snap("//p[position() < last()]"), "p1,p2");
    assert_eq!(snap("//p[3]/preceding-sibling::p[1]"), "p2");
    assert_eq!(snap("(//p)[position() = 2]"), "p2");
    assert_eq!(snap("//p[@class='x']/following::*[1]"), "p2");
    // Functions and HTML name tests.
    assert_eq!(snap("//*[starts-with(@id, 'p') and string-length(@id) = 2][not(@class)]"), "p2,p3");
    assert_eq!(snap("//p[normalize-space(translate(., 'S', 's')) = 'second']"), "p2");
    assert_eq!(snap("id('p3 title')"), "title,p3");
    assert_eq!(snap("//H1"), "title");
    assert_eq!(snap("//svg"), "");
    assert_eq!(snap("//*[local-name() = 'rect']"), "r");
    assert_eq!(
        eval(r, "document.evaluate('concat(name(//p[1]), \"-\", substring-after(//p[1]/@id, \"p\"), \"-\", round(2.5), \"-\", sum(//p/@data-n))', document, null, 2, null).stringValue"),
        "p-1-3-3"
    );
    // Context node scoping, including an Attr context node.
    assert_eq!(
        eval(r, "document.evaluate('./p', document.querySelector('.group'), null, 7, null).snapshotLength"),
        "3"
    );
    assert_eq!(
        eval(
            r,
            "document.evaluate('..', document.getElementById('p1').attributes[0], null, 9, null).singleNodeValue.id"
        ),
        "p1"
    );
}

#[test]
fn xpath_errors_resolvers_and_invalidation() {
    let e = env(XPATH_PAGE);
    let r = &e.main;
    // Syntax errors are DOMException SyntaxError.
    assert_eq!(
        eval(r, "['//p[', 'bogus()', '', '//', 'a ? b'].map(x => { try { document.evaluate(x, document); return 'ok' } catch (e) { return e instanceof DOMException && e.name } }).join()"),
        "SyntaxError,SyntaxError,SyntaxError,SyntaxError,SyntaxError"
    );
    assert_eq!(eval(r, "try { document.createExpression('//p[') } catch (e) { e.name }"), "SyntaxError");
    assert_eq!(eval(r, "try { document.evaluate('1 | //p', document) } catch (e) { e.constructor.name }"), "TypeError");
    // Namespace resolvers: null, function, object, Node.
    assert_eq!(
        eval(r, "try { document.evaluate('//svg:rect', document, null, 7, null) } catch (e) { e.name }"),
        "NamespaceError"
    );
    assert_eq!(
        eval(r, "document.evaluate('//svg:svg/svg:*', document, p => p === 'svg' ? 'http://www.w3.org/2000/svg' : null, 9, null).singleNodeValue.id"),
        "r"
    );
    assert_eq!(
        eval(r, "let nsCalls = 0; document.evaluate('//h:P[@id=\"p2\"] | //h:h1', document, { lookupNamespaceURI(p) { nsCalls++; return p === 'h' ? 'http://www.w3.org/1999/xhtml' : null } }, 7, null).snapshotLength + ':' + nsCalls"),
        "2:1"
    );
    assert_eq!(
        eval(r, "try { document.evaluate('//x:p', document, () => null, 7, null) } catch (e) { e.name }"),
        "NamespaceError"
    );
    assert_eq!(
        eval(r, "try { document.evaluate('//x:p', document, () => { throw new Error('boom') }, 7, null) } catch (e) { e.message }"),
        "boom"
    );
    assert_eq!(eval(r, "document.createNSResolver(document) === document"), "true");
    assert_eq!(
        eval(r, "document.evaluate('//p', document, document.createNSResolver(document), 7, null).snapshotLength"),
        "3"
    );
    // XML document with a Node resolver (`xml` prefix via lookupNamespaceURI).
    assert_eq!(
        eval(r, "const xd = document.implementation.createDocument(null, 'mu', null); const mu = xd.documentElement; mu.setAttributeNS('http://www.w3.org/XML/1998/namespace', 'xml:id', 'id1'); const rho = xd.createElementNS(null, 'rho'); rho.setAttributeNS('http://www.w3.org/XML/1998/namespace', 'xml:lang', 'en-GB'); mu.appendChild(rho); const xr = xd.evaluate('//mu[@xml:id=\"id1\"]/rho[@xml:lang=\"en-GB\"]', mu, xd.createNSResolver(mu), XPathResult.ANY_TYPE, null); (xr.iterateNext() === rho) + ':' + xr.iterateNext()"),
        "true:null"
    );
    // Iterators are invalidated by mutation; snapshots are not.
    assert_eq!(
        eval(r, "const inv = document.evaluate('//p', document, null, 5, null); const invSnap = document.evaluate('//p', document, null, 7, null); inv.iterateNext(); document.body.appendChild(document.createElement('i')); let invName; try { inv.iterateNext() } catch (e) { invName = e.name } [inv.invalidIteratorState, invName, invSnap.snapshotItem(1).id, invSnap.invalidIteratorState].join()"),
        "true,InvalidStateError,p2,false"
    );
    assert_eq!(
        eval(r, "const inv2 = document.evaluate('//p', document, null, 4, null); document.getElementById('p1').setAttribute('data-z', '1'); inv2.invalidIteratorState"),
        "true"
    );
    assert_eq!(
        eval(r, "const inv3 = document.evaluate('//p', document, null, 4, null); document.getElementById('p2').firstChild.data = 'changed'; inv3.invalidIteratorState"),
        "true"
    );
}

#[test]
fn xpath_iterators_survive_form_state_and_focus() {
    let e = env("<!doctype html><body><input type=checkbox id=c1><input type=checkbox id=c2><input type=radio name=r id=r1><select id=s><option>a</option><option>b</option></select><input id=t></body>");
    let r = &e.main;
    // Clicking/checking/focusing/typing during iteration changes no tree,
    // attribute or character data, so the iterator stays valid (browsers).
    assert_eq!(
        eval(r, "const it = document.evaluate('//input | //select', document, null, XPathResult.ORDERED_NODE_ITERATOR_TYPE, null); const seen = []; let n; while ((n = it.iterateNext())) { seen.push(n.id); if (n.type === 'checkbox' || n.type === 'radio') n.click(); if (n.id === 's') n.selectedIndex = 1; if (n.id === 't') { n.focus(); n.value = 'typed'; } } [seen.join(), it.invalidIteratorState, document.getElementById('c2').checked, document.activeElement.id].join(':')"),
        "c1,c2,r1,s,t:false:true:t"
    );
    // Real mutations still invalidate.
    assert_eq!(
        eval(r, "const it2 = document.evaluate('//input', document, null, 5, null); it2.iterateNext(); document.body.appendChild(document.createTextNode('x')); try { it2.iterateNext(); 'no' } catch (e) { e.name }"),
        "InvalidStateError"
    );
}

#[test]
fn xpath_review_fixes() {
    let e = env(XPATH_PAGE);
    let r = &e.main;
    assert_eq!(
        eval(r, "const sn = document.evaluate('//p', document, null, 7, null); try { sn.snapshotItem() } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // Attributes are identified by (namespace, local name), not the
    // qualified name, which two attributes can share.
    assert_eq!(
        eval(r, "const ae = document.createElement('i'); ae.setAttributeNS('urn:a', 'p:x', 'one'); ae.setAttributeNS('urn:b', 'p:x', 'two'); const as = document.evaluate('@*', ae, null, 7, null); [as.snapshotLength, as.snapshotItem(0).value, as.snapshotItem(1).value, as.snapshotItem(1).namespaceURI, document.evaluate('string(..)', as.snapshotItem(1), null, 2, null).stringValue === ''].join()"),
        "2,one,two,urn:b,true"
    );
    assert_eq!(
        eval(r, "const ctxAttr = ae.attributes[1]; document.evaluate('namespace-uri(.)', ctxAttr, null, 2, null).stringValue"),
        "urn:b"
    );
    // The Attr context is located after the resolver (which may mutate) ran.
    assert_eq!(
        eval(r, "const re = document.createElement('i'); re.setAttribute('a', '1'); re.setAttribute('b', '2'); const bAttr = re.attributes[1]; document.evaluate('concat(name(.), count(//q:none))', bAttr, () => { re.removeAttribute('a'); return 'urn:q' }, 2, null).stringValue"),
        "b0"
    );
    assert_eq!(eval(r, "document.evaluate('1 div sum(//nothing)', document, null, 1, null).numberValue"), "Infinity");
    assert_eq!(eval(r, "document.evaluate('count(/node())', document, null, 1, null).numberValue"), "2");
}

#[test]
fn xpath_evaluator_and_expression() {
    let e = env(XPATH_PAGE);
    let r = &e.main;
    assert_eq!(
        eval(r, "const ev = new XPathEvaluator(); ev instanceof XPathEvaluator && [typeof ev.evaluate, typeof ev.createExpression, typeof ev.createNSResolver].join()"),
        "function,function,function"
    );
    assert_eq!(eval(r, "ev.evaluate('//p', document, null, 7, null).snapshotItem(1).id"), "p2");
    assert_eq!(eval(r, "try { ev.evaluate('//p', null) } catch (e) { e.constructor.name }"), "TypeError");
    assert_eq!(eval(r, "ev.createNSResolver(document.body) === document.body"), "true");
    assert_eq!(
        eval(r, "const xe = document.createExpression('p', null); xe instanceof XPathExpression && [xe.evaluate(document.querySelector('.group'), 7, null).snapshotLength, xe.evaluate(document.body, 7).snapshotLength, xe.evaluate(document.querySelector('h1'), XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue].join()"),
        "3,0,"
    );
    assert_eq!(
        eval(
            r,
            "ev.createExpression('count(//p)', null).evaluate(document, XPathResult.NUMBER_TYPE, null).numberValue"
        ),
        "3"
    );
    assert_eq!(
        eval(r, "document.createExpression('//svg:rect', p => 'http://www.w3.org/2000/svg').evaluate(document, 9).singleNodeValue.id"),
        "r"
    );
    // Results keep their (detached) nodes alive across GC.
    assert_eq!(
        eval(r, "(() => { const keep = document.createElement('div'); keep.innerHTML = '<b id=kept></b>'; window.keptResult = document.evaluate('.//b', keep, null, 7, null); return 'ok' })()"),
        "ok"
    );
    e.engine.collect_garbage_now();
    e.engine.drain_finalized();
    e.page.after_task();
    assert_eq!(
        eval(r, "keptResult.snapshotItem(0).id + ':' + keptResult.snapshotItem(0).parentNode.nodeName"),
        "kept:DIV"
    );
}

#[test]
fn window_same_object_attributes_keep_expandos_across_gc() {
    let e = env("<body>");
    let r = &e.main;
    // TOS init scripts override navigator.serviceWorker / crypto.subtle with
    // defineProperty; the override must outlive collection of the wrapper.
    eval(
        r,
        "(() => { for (const [o, n] of [[navigator,'navigator'], [screen,'screen'], [crypto,'crypto'], [crypto.subtle,'subtle'], [customElements,'customElements'], [getSelection(),'selection'], [indexedDB,'indexedDB']]) Object.defineProperty(o, '__mark', { configurable: true, value: n }); Object.defineProperty(navigator, 'serviceWorker', { configurable: true, value: { getRegistration: async () => null } }); })()",
    );
    for _ in 0..3 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
        e.page.after_task();
    }
    assert_eq!(
        eval(r, "[navigator.__mark, clientInformation.__mark, screen.__mark, crypto.__mark, crypto.subtle.__mark, customElements.__mark, getSelection().__mark, indexedDB.__mark, typeof navigator.serviceWorker.getRegistration, clientInformation === navigator].join()"),
        "navigator,navigator,screen,crypto,subtle,customElements,selection,indexedDB,function,true"
    );
}

#[test]
fn space_activates_focused_controls_on_keyup() {
    let e = env("<body><input type=checkbox role=switch id=c><button type=button id=b>b</button><input id=t><button id=p>p</button></body>");
    let r = &e.main;
    eval(
        r,
        "window.evs = []; for (const id of ['c','b','t','p']) { const el = document.getElementById(id); for (const k of ['keydown','keyup','click','change']) el.addEventListener(k, ev => evs.push(id + ':' + k + (k === 'click' ? '/' + ev.isTrusted + '/' + ev.detail : ''))); } document.getElementById('p').addEventListener('keyup', ev => ev.preventDefault());",
    );
    let page = e.page.clone();
    // Playwright `keyboard.press('Space')`: keyDown carrying text, then keyUp.
    let space =
        input::KeyInput { key: " ".into(), code: "Space".into(), text: " ".into(), key_code: 32, ..Default::default() };
    let press = |id: &str| {
        eval(r, &format!("document.getElementById('{id}').focus()"));
        input::dispatch_key(&page, input::KeyKind::KeyDown, &space);
        input::dispatch_key(&page, input::KeyKind::KeyUp, &space);
    };
    press("c");
    // No click until keyup: the checkbox is toggled by the keyup-driven click.
    assert_eq!(
        eval(r, "evs.join(',') + '|' + document.getElementById('c').checked"),
        "c:keydown,c:keyup,c:click/true/0,c:change|true"
    );
    eval(r, "evs = []");
    press("b");
    press("t");
    press("p"); // t's change fires on blur; p's prevented keyup does not click.
    assert_eq!(
        eval(r, "evs.join(',') + '|' + document.getElementById('t').value"),
        "b:keydown,b:keyup,b:click/true/0,t:keydown,t:keyup,t:change,p:keydown,p:keyup| "
    );
    // Focus moving between keydown and keyup cancels the activation.
    eval(r, "evs = []; document.getElementById('c').focus()");
    input::dispatch_key(&page, input::KeyKind::KeyDown, &space);
    eval(r, "document.getElementById('b').focus()");
    input::dispatch_key(&page, input::KeyKind::KeyUp, &space);
    assert_eq!(eval(r, "evs.join(',') + '|' + document.getElementById('c').checked"), "c:keydown,b:keyup|true");
}

#[test]
fn same_object_retention_ignores_page_patched_builtins() {
    let e = env("<body>");
    let r = &e.main;
    // Page code that replaces Array.prototype.push and installs indexed
    // setters on Object/Array prototypes must not defeat retention.
    eval(
        r,
        "Array.prototype.push = function () { return 0; }; for (const p of [Object.prototype, Array.prototype]) for (let i = 0; i < 8; i++) Object.defineProperty(p, i, { configurable: true, set() {}, get() { return undefined; } }); Object.defineProperty(navigator, '__mark', { configurable: true, value: 'nav' }); crypto.subtle.__mark = 'subtle'; window.firstNavigator = navigator;",
    );
    eval(r, "delete window.firstNavigator");
    for _ in 0..3 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
        e.page.after_task();
    }
    assert_eq!(
        eval(r, "[navigator.__mark, crypto.subtle.__mark, navigator === clientInformation].join()"),
        "nav,subtle,true"
    );
}

#[test]
fn screen_orientation_reports_override_or_follows_screen() {
    use lp_dom::media::{OrientationType, ScreenOrientation};
    let e = env("<body></body>");
    let probe = "screen.orientation.type + '/' + screen.orientation.angle";
    // Fallback: no override, the screen never rotates (1440x900 viewport).
    assert_eq!(eval(&e.main, probe), "landscape-primary/0");
    let mut vp = e.page.viewport();
    vp.width = 390;
    vp.height = 844;
    e.page.set_viewport(vp.clone());
    assert_eq!(eval(&e.main, probe), "portrait-primary/0");
    // Landscape screen with a portrait viewport: the fallback follows the screen.
    vp.screen_width = Some(1440);
    vp.screen_height = Some(900);
    e.page.set_viewport(vp.clone());
    assert_eq!(eval(&e.main, probe), "landscape-primary/0", "follows screen, not viewport");
    // Override wins regardless of dimensions; (orientation) still follows the viewport.
    vp.orientation = Some(ScreenOrientation { kind: OrientationType::LandscapePrimary, angle: 0 });
    e.page.set_viewport(vp.clone());
    assert_eq!(eval(&e.main, probe), "landscape-primary/0");
    assert_eq!(eval(&e.main, "String(matchMedia('(orientation: portrait)').matches)"), "true");
    vp.orientation = Some(ScreenOrientation { kind: OrientationType::PortraitSecondary, angle: 180 });
    e.page.set_viewport(vp.clone());
    assert_eq!(eval(&e.main, probe), "portrait-secondary/180");
    vp.orientation = None;
    e.page.set_viewport(vp);
    assert_eq!(eval(&e.main, probe), "landscape-primary/0", "back to the landscape screen");
    e.page.dispose();
}

struct RecordingNav(RefCell<Vec<String>>);
impl PageHooks for RecordingNav {
    fn post_task(&self, _task: Box<dyn FnOnce()>) {}
    fn navigate(&self, request: NavigationRequest) {
        self.0.borrow_mut().push(request.url);
    }
}

#[test]
fn object_urls_register_revoke_and_download_links_do_not_navigate() {
    let e = env("<body></body>");
    let r = &e.main;
    let nav = Rc::new(RecordingNav(RefCell::new(Vec::new())));
    e.page.set_hooks(Some(nav.clone()));
    let url = eval(r, "window.u1 = URL.createObjectURL(new Blob(['hi'], {type: 'text/plain'})); u1");
    assert!(
        url.starts_with("blob:http://localhost:5173/") && url.len() == "blob:http://localhost:5173/".len() + 36,
        "{url}"
    );
    assert_eq!(e.page.blob_url(&url).map(|(b, t)| (b.to_vec(), t)), Some((b"hi".to_vec(), "text/plain".into())));
    assert_eq!(
        eval(r, "const u2 = URL.createObjectURL(new File(['x'], 'f.txt')); [u2 !== u1, (()=>{try{URL.createObjectURL('nope')}catch(e){return e.name}})(), (()=>{try{URL.createObjectURL()}catch(e){return e.name}})()].join()"),
        "true,TypeError,TypeError"
    );
    // Revocation from another origin's URL string is silently ignored; the owner revokes.
    eval(r, "URL.revokeObjectURL(u1.replace('localhost:5173', 'other.test')); URL.revokeObjectURL('not a url')");
    assert!(e.page.blob_url(&url).is_some());
    eval(r, "URL.revokeObjectURL(u1)");
    assert!(e.page.blob_url(&url).is_none());
    // TOS file.utils: <a download href=blob:> clicked via a synthetic click must
    // not navigate (download is a documented gap); plain or cross-origin
    // download links still navigate.
    eval(
        r,
        "for (const [href, dl] of [[URL.createObjectURL(new Blob(['d'])), 'x.pdf'], ['data:text/plain,a', ''], ['/same', 'f'], ['http://other.test/f', 'f'], ['/plain', null]]) { const a = document.createElement('a'); a.href = href; if (dl !== null) a.download = dl; document.body.appendChild(a); a.dispatchEvent(new MouseEvent('click', {bubbles: true, cancelable: true})); a.remove(); }",
    );
    assert_eq!(
        nav.0.borrow().clone(),
        vec!["http://other.test/f".to_string(), "http://localhost:5173/plain".to_string()]
    );
    // Document replacement drops the old document's URLs (and their bytes);
    // registrations made afterwards belong to the new document.
    let old = eval(r, "window.oldUrl = URL.createObjectURL(new Blob(['old-bytes'])); oldUrl");
    let (held, _) = e.page.blob_url(&old).unwrap();
    // Finalize the temporary JS Blob first: its byte owner is independent of
    // the registry and must not be mistaken for a leaked registry reference.
    for _ in 0..6 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
    }
    assert!(!held.is_unique(), "the registry still owns the old bytes before replacement");
    assert!(e.page.blob_url_count() >= 2);
    let doc = parse_document(&e.page.dom, "http://localhost:5173/next", "<body>");
    e.page.set_document(doc);
    assert_eq!(e.page.blob_url_count(), 0);
    assert!(e.page.blob_url(&old).is_none());
    // The registry no longer holds the bytes: only our test clone remains.
    assert!(held.is_unique(), "registry released the old document's bytes");
    eval(r, "URL.revokeObjectURL(oldUrl)");
    let fresh = eval(r, "URL.createObjectURL(new Blob(['new']))");
    assert_ne!(fresh, old);
    assert_eq!(e.page.blob_url(&fresh).map(|(b, _)| b.to_vec()), Some(b"new".to_vec()));
    assert_eq!(e.page.blob_url_count(), 1);
    // Disposal drops every registered URL of the document.
    let live = eval(r, "URL.createObjectURL(new Blob(['z']))");
    assert!(e.page.blob_url(&live).is_some());
    e.page.dispose();
    assert!(e.page.blob_url(&live).is_none());
}

#[test]
fn message_channel_delivers_cloned_messages_fifo_as_tasks() {
    let e = env("<body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    let r = &e.main;
    assert_eq!(
        eval(
            r,
            "window.log = []; const mc = new MessageChannel(); window.mc = mc;
             const o = {n: 1, nested: {list: [1, 2]}};
             mc.port2.onmessage = (ev) => log.push([ev.data.n, ev.data === o, ev.data.nested.list.join('-'), JSON.stringify(ev.origin), String(ev.source), ev.ports.length, ev.isTrusted, ev instanceof MessageEvent, ev.target === mc.port2].join(':'));
             mc.port1.postMessage(o); o.n = 99; mc.port1.postMessage({n: 2, nested: {list: []}});
             Promise.resolve().then(() => log.push('microtask'));
             [mc.port1 === mc.port1, mc.port1 instanceof MessagePort, mc.port1 instanceof EventTarget, typeof mc.port1.start, log.length].join()",
        ),
        "true,true,true,function,0"
    );
    // Delivery happens in later tasks, after microtasks, one message per task.
    tasks.drain();
    assert_eq!(
        eval(r, "log.join('|')"),
        "microtask|1:false:1-2:\"\":null:0:true:true:true|2:false::\"\":null:0:true:true:true"
    );
}

#[test]
fn message_port_start_close_transfer_and_clone_errors() {
    let e = env("<body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    let r = &e.main;
    // addEventListener does not start the port; messages wait until start().
    eval(r, "window.log = []; window.a = new MessageChannel(); a.port2.addEventListener('message', (ev) => log.push('l:' + ev.data)); a.port1.postMessage('x'); a.port1.postMessage('y');");
    tasks.drain();
    assert_eq!(eval(r, "log.join()"), "");
    eval(r, "a.port2.start(); a.port2.start();");
    tasks.drain();
    assert_eq!(eval(r, "log.join()"), "l:x,l:y");
    // Both directions work; close() disentangles and drops pending messages.
    eval(r, "log = []; a.port1.onmessage = (ev) => log.push('back:' + ev.data); a.port2.postMessage('b'); a.port1.postMessage('pending'); a.port2.close(); a.port1.postMessage('after'); a.port2.postMessage('afterClose');");
    tasks.drain();
    assert_eq!(eval(r, "log.join()"), "back:b");
    // Transfers are explicitly unsupported; non-cloneable data and ports throw.
    assert_eq!(
        eval(
            r,
            "const c = new MessageChannel(); const err = (f) => { try { f(); return 'ok'; } catch (e) { return e.name; } };
             [err(() => c.port1.postMessage(1, [c.port2])), err(() => c.port1.postMessage(1, {transfer: [new ArrayBuffer(1)]})),
              err(() => c.port1.postMessage(() => 1)), err(() => c.port1.postMessage({p: c.port2})), err(() => c.port1.postMessage()),
              err(() => c.port1.postMessage(1, [])), err(() => c.port1.postMessage(1, {}))].join()",
        ),
        "NotSupportedError,NotSupportedError,DataCloneError,DataCloneError,TypeError,ok,ok"
    );
    // Page reassignment of structuredClone does not affect port cloning.
    eval(r, "log = []; structuredClone = () => { throw Error('page clone'); }; const d = new MessageChannel(); d.port2.onmessage = (ev) => log.push(ev.data.k); d.port1.postMessage({k: 'native'});");
    tasks.drain();
    assert_eq!(eval(r, "log.join()"), "native");
}

#[test]
fn message_port_lifetime_pins_started_ports_until_close() {
    let e = env("<body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    let r = &e.main;
    let base = e.page.pinned_count();
    // React-scheduler shape: only the closures reference the channel.
    eval(r, "window.got = []; (() => { const ch = new MessageChannel(); ch.port1.onmessage = (ev) => got.push(ev.data); window.send = (v) => ch.port2.postMessage(v); window.closeIt = () => ch.port1.close(); })();");
    assert_eq!(e.page.pinned_count(), base + 1, "started entangled port is pinned");
    for _ in 0..3 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
        e.page.after_task();
    }
    eval(r, "send('kept'); send('alive');");
    tasks.drain();
    assert_eq!(eval(r, "got.join()"), "kept,alive");
    eval(r, "closeIt(); send('dropped');");
    tasks.drain();
    assert_eq!(eval(r, "got.join()"), "kept,alive");
    assert_eq!(e.page.pinned_count(), base, "close() releases the pin");
    // Messages queued for an open port are not delivered after page disposal.
    eval(r, "(() => { const z = new MessageChannel(); z.port1.onmessage = () => got.push('late'); z.port2.postMessage(1); })();");
    e.page.dispose();
    tasks.drain();
}

#[test]
fn message_channel_with_self_referencing_listener_is_collected_after_close() {
    let e = env("<body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    let r = &e.main;
    let base = e.page.pinned_count();
    // The listener and an expando reference the channel: a cycle that must stay
    // visible to the GC (no native roots from channel to ports).
    eval(
        r,
        "window.refs = (() => { const ch = new MessageChannel(); ch.self = ch; ch.port1.onmessage = () => ch; ch.port2.onmessage = () => ch.port1; ch.port1.close(); ch.port2.close(); return [new WeakRef(ch), new WeakRef(ch.port1), new WeakRef(ch.port2)]; })(); 1",
    );
    assert_eq!(e.page.pinned_count(), base, "closed ports are unpinned");
    for _ in 0..4 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
        e.page.after_task();
        tasks.drain();
    }
    assert_eq!(eval(r, "refs.map((w) => w.deref() === undefined).join()"), "true,true,true");
    // A channel that is still referenced keeps both ports reachable.
    eval(
        r,
        "window.kept = new MessageChannel(); window.keptRefs = [new WeakRef(kept.port1), new WeakRef(kept.port2)];",
    );
    for _ in 0..3 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
    }
    assert_eq!(
        eval(r, "keptRefs.map((w) => w.deref() !== undefined).join() + ':' + (kept.port1 === keptRefs[0].deref())"),
        "true,true:true"
    );
}

#[test]
fn window_event_is_the_current_event_during_listener_invocation() {
    let e = env("<body><div id=o><button id=b>b</button></div><span id=other></span></body>");
    let r = &e.main;
    assert_eq!(
        eval(
            r,
            "const ev = () => window.event === undefined ? 'undefined' : window.event.type;
             const b = document.getElementById('b'), o = document.getElementById('o'), other = document.getElementById('other');
             const log = [];
             const outside = ['event' in window, ev()].join(':');
             // Listener sees the dispatched event object; restored after.
             b.addEventListener('ping', (e) => log.push('b:' + ev() + ':' + (window.event === e)));
             // Nested dispatch from a listener: inner sees the inner event, then the outer is restored.
             o.addEventListener('ping', () => { other.dispatchEvent(new Event('inner')); log.push('restored:' + ev()); });
             other.addEventListener('inner', () => log.push('nested:' + ev()));
             b.dispatchEvent(new Event('ping', {bubbles: true}));
             log.push('after:' + ev());
             // A throwing listener still restores.
             other.addEventListener('boom', () => { throw new Error('x'); });
             o.addEventListener('wrap', () => { other.dispatchEvent(new Event('boom')); log.push('afterThrowNested:' + ev()); });
             o.dispatchEvent(new Event('wrap'));
             log.push('afterThrow:' + ev());
             outside + '|' + log.join('|')",
        ),
        "true:undefined|b:ping:true|nested:inner|restored:ping|after:undefined|afterThrowNested:wrap|afterThrow:undefined"
    );
}

#[test]
fn window_event_spans_the_callback_microtask_checkpoint_for_message_ports() {
    let e = env("<body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    let r = &e.main;
    eval(
        r,
        "window.log = []; const ev = () => window.event === undefined ? 'undefined' : window.event.type + ':' + window.event.constructor.name;
         const c = new MessageChannel();
         c.port1.onmessage = (e) => { log.push('in:' + ev() + ':' + (window.event === e)); Promise.resolve().then(() => log.push('micro:' + ev())); };
         const d = new MessageChannel(); d.port1.addEventListener('message', () => log.push('listener:' + ev())); d.port1.start();
         c.port2.postMessage(1); d.port2.postMessage(2);",
    );
    tasks.drain();
    assert_eq!(
        eval(r, "log.join('|') + '|after:' + ev()"),
        "in:message:MessageEvent:true|micro:message:MessageEvent|listener:message:MessageEvent|after:undefined"
    );
}

#[test]
fn window_event_is_replaceable_and_skips_shadow_tree_targets() {
    let e = env("<body><div id=host></div><p id=p></p></body>");
    let r = &e.main;
    // Shadow-tree invocation targets do not set the current event.
    assert_eq!(
        eval(
            r,
            "const host = document.getElementById('host'); const root = host.attachShadow({mode: 'open'}); const inner = document.createElement('i'); root.appendChild(inner);
             const seen = []; inner.addEventListener('x', () => seen.push('inner:' + String(window.event))); host.addEventListener('x', () => seen.push('host:' + (window.event && window.event.type)));
             inner.dispatchEvent(new Event('x', {bubbles: true, composed: true}));
             seen.join('|')",
        ),
        "inner:undefined|host:x"
    );
    // [Replaceable]: assignment creates an own data property that shadows the accessor.
    assert_eq!(
        eval(
            r,
            "const p = document.getElementById('p'); let during;
             window.event = 'mine';
             const d = Object.getOwnPropertyDescriptor(window, 'event');
             p.addEventListener('y', () => { during = window.event; }); p.dispatchEvent(new Event('y'));
             [window.event, 'value' in d, d.writable, d.enumerable, d.configurable, during].join()",
        ),
        "mine,true,true,true,true,mine"
    );
}

#[test]
fn window_event_does_not_retain_the_event_after_dispatch() {
    let e = env("<body><b id=t></b></body>");
    let r = &e.main;
    eval(r, "window.ref = (() => { const ev = new Event('z'); const t = document.getElementById('t'); t.addEventListener('z', () => {}); t.dispatchEvent(ev); return new WeakRef(ev); })(); 1");
    for _ in 0..4 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
        e.page.after_task();
    }
    assert_eq!(eval(r, "[ref.deref() === undefined, window.event === undefined].join()"), "true,true");
}

fn gc(e: &Env, tasks: &ObserverTasks) {
    for _ in 0..3 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
        e.page.after_task();
    }
    let _ = tasks;
}

#[test]
fn message_port_keeps_queued_messages_across_gc_when_the_peer_closes() {
    let e = env("<body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    let r = &e.main;
    let base = e.page.pinned_count();
    // V1: the receiver is started; the sender posts and then closes before delivery.
    eval(r, "window.got = []; window.ref1 = (() => { const ch = new MessageChannel(); ch.port1.onmessage = (ev) => got.push(ev.data); ch.port2.postMessage('v1'); ch.port2.close(); return new WeakRef(ch.port1); })(); 1");
    gc(&e, &tasks);
    tasks.drain();
    assert_eq!(eval(r, "got.join()"), "v1", "queued message survives GC after the peer closed");
    // After draining, the disentangled receiver is no longer pinned and can be collected.
    gc(&e, &tasks);
    assert_eq!(e.page.pinned_count(), base);
    assert_eq!(eval(r, "ref1.deref() === undefined"), "true");
}

#[test]
fn message_port_started_late_with_a_collected_peer_still_delivers_across_gc() {
    let e = env("<body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    let r = &e.main;
    let base = e.page.pinned_count();
    // V2: the sender posts before the receiver starts, then the sender is collected.
    eval(r, "window.got = []; window.held = (() => { const ch = new MessageChannel(); ch.port2.postMessage('v2'); return ch.port1; })(); 1");
    gc(&e, &tasks);
    // Queued but unstarted: not pinned (it waits for start()).
    assert_eq!(e.page.pinned_count(), base);
    eval(r, "held.onmessage = (ev) => got.push(ev.data); window.ref2 = new WeakRef(held); held = null; 1");
    gc(&e, &tasks);
    tasks.drain();
    assert_eq!(eval(r, "got.join()"), "v2", "message queued on a late-started port survives GC");
    gc(&e, &tasks);
    assert_eq!(e.page.pinned_count(), base, "drained, peerless port is unpinned");
    assert_eq!(eval(r, "ref2.deref() === undefined"), "true");
}

#[test]
fn message_port_unstarted_queued_ports_and_disposal_hold_no_roots() {
    let e = env("<body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    let r = &e.main;
    let base = e.page.pinned_count();
    // Nobody can start this port, so its queued message does not keep it alive.
    eval(r, "window.refs = (() => { const ch = new MessageChannel(); ch.port2.postMessage('never'); return [new WeakRef(ch), new WeakRef(ch.port1), new WeakRef(ch.port2)]; })(); 1");
    gc(&e, &tasks);
    tasks.drain();
    assert_eq!(e.page.pinned_count(), base);
    assert_eq!(eval(r, "refs.map((w) => w.deref() === undefined).join()"), "true,true,true");
    // A started, entangled port stays pinned (React-scheduler shape) until disposal releases it.
    eval(r, "window.got2 = []; (() => { const ch = new MessageChannel(); ch.port1.onmessage = (ev) => got2.push(ev.data); window.sendIt = (v) => ch.port2.postMessage(v); })();");
    gc(&e, &tasks);
    eval(r, "sendIt('alive')");
    tasks.drain();
    assert_eq!(eval(r, "got2.join()"), "alive");
    assert_eq!(e.page.pinned_count(), base + 1);
    e.page.dispose();
    assert_eq!(e.page.pinned_count(), 0, "disposal releases port pins");
}

#[test]
fn message_port_receiver_is_released_when_its_sender_becomes_unreachable() {
    let e = env("<body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    let r = &e.main;
    let base = e.page.pinned_count();
    // Started, drained receiver; the sender and the channel become unreachable.
    eval(r, "window.got = []; window.refs = (() => { const ch = new MessageChannel(); ch.port1.onmessage = (ev) => got.push(ev.data); ch.port2.postMessage('once'); return [new WeakRef(ch), new WeakRef(ch.port1), new WeakRef(ch.port2)]; })(); 1");
    tasks.drain();
    assert_eq!(eval(r, "got.join()"), "once");
    assert_eq!(e.page.pinned_count(), base + 1, "entangled started receiver is pinned while its sender lives");
    // Collect the sender, run the reconciliation task, collect again.
    gc(&e, &tasks);
    tasks.drain();
    gc(&e, &tasks);
    assert_eq!(e.page.pinned_count(), base, "receiver unpinned once its peer was collected");
    assert_eq!(eval(r, "refs.map((w) => w.deref() === undefined).join()"), "true,true,true");
    // A queued message still keeps the receiver alive until delivered, even if the sender goes.
    eval(r, "window.got2 = []; window.refs2 = (() => { const ch = new MessageChannel(); ch.port1.onmessage = (ev) => got2.push(ev.data); ch.port2.postMessage('a'); ch.port2.postMessage('b'); return [new WeakRef(ch.port1), new WeakRef(ch.port2)]; })(); 1");
    gc(&e, &tasks);
    tasks.drain();
    gc(&e, &tasks);
    tasks.drain();
    assert_eq!(eval(r, "got2.join()"), "a,b");
    gc(&e, &tasks);
    tasks.drain();
    gc(&e, &tasks);
    assert_eq!(e.page.pinned_count(), base);
    assert_eq!(eval(r, "refs2.map((w) => w.deref() === undefined).join()"), "true,true");
}

#[test]
fn message_port_peer_collection_is_reconciled_at_after_task_without_hooks() {
    // Default configuration: no page hooks (PageDom::post_task would run inline).
    let e = env("<body>");
    let r = &e.main;
    let base = e.page.pinned_count();
    eval(r, "window.ref = (() => { const ch = new MessageChannel(); ch.port1.onmessage = () => {}; return new WeakRef(ch.port1); })(); 1");
    assert_eq!(e.page.pinned_count(), base + 1);
    for _ in 0..3 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
    }
    // The sender's drop only recorded the receiver: nothing ran from inside the drop.
    assert_eq!(e.page.pinned_count(), base + 1, "reconciliation is deferred, not inline in Drop");
    e.page.after_task();
    assert_eq!(e.page.pinned_count(), base, "reconciled at the safe point");
    for _ in 0..3 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
        e.page.after_task();
    }
    assert_eq!(eval(r, "ref.deref() === undefined"), "true");
}

#[test]
fn data_transfer_store_items_and_files() {
    let e = env("<body>");
    let r = &e.main;
    assert_eq!(
        eval(
            r,
            r#"
        const dt = new DataTransfer();
        const out = [dt.dropEffect, dt.effectAllowed, dt.items === dt.items, dt.items.length, dt.files.length];
        dt.setData('Text', 'hi');
        dt.setData('text/html', '<b>x</b>');
        dt.setData('text/plain', 'hello');
        out.push(dt.getData('text'), dt.getData('TEXT/HTML'), dt.getData('missing') === '', dt.types.join(), Object.isFrozen(dt.types));
        const it = dt.items.add(new File(['abc'], 'a.txt', { type: 'text/plain', lastModified: 5 }));
        out.push(it.kind, it.type, dt.types.join(), dt.items.length, dt.files.length, dt.files[0].name, dt.files[0].size,
                 dt.files[0].lastModified, dt.files[0] instanceof File, it.getAsFile().name, dt.items[0].getAsFile());
        try { dt.items.add('again', 'text/html'); out.push('no throw'); } catch (err) { out.push(err.name); }
        try { dt.items.add('x'); out.push('no throw'); } catch (err) { out.push(err.constructor.name); }
        dt.clearData('text/html');
        out.push(dt.types.join());
        dt.clearData();
        out.push(dt.types.join(), dt.items.length);
        dt.items.remove(0);
        out.push(it.kind === '' && it.type === '', it.getAsFile(), dt.files.length, dt.types.length);
        dt.dropEffect = 'bogus'; dt.effectAllowed = 'nope';
        out.push(dt.dropEffect, dt.effectAllowed);
        dt.dropEffect = 'copy'; dt.effectAllowed = 'copyMove';
        out.push(dt.dropEffect, dt.effectAllowed);
        dt.setData('URL', '# comment\nhttps://a.example/\nhttps://b.example/');
        out.push(dt.getData('url'), dt.types.join());
        dt.items.clear();
        out.push(dt.items.length, [...dt.items].length);
        out.join('|')
    "#
        ),
        "none|none|true|0|0|hello|<b>x</b>|true|text/html,text/plain|true|file|text/plain|text/html,text/plain,Files|3|1|a.txt|3|5|true|a.txt||NotSupportedError|TypeError|text/plain,Files|Files|1|true||0|0|none|none|copy|copyMove|https://a.example/|text/uri-list|0|0"
    );
}

#[test]
fn input_files_setter_accepts_data_transfer_files() {
    let e = env("<body><form id=fm><input type=file id=f name=up multiple><input id=t name=t value=v></form></body>");
    let r = &e.main;
    // Playwright's in-memory setInputFiles (injected script, 1.59) verbatim.
    assert_eq!(
        eval(
            r,
            r#"
        window.seen = [];
        const input = document.getElementById('f');
        input.addEventListener('input', ev => seen.push(ev.type + ':' + ev.bubbles + ':' + ev.composed + ':' + input.files.length));
        input.addEventListener('change', ev => seen.push(ev.type + ':' + ev.bubbles + ':' + input.files.length));
        const payloads = [
            { name: 'a.txt', mimeType: 'text/plain', buffer: btoa('hi'), lastModifiedMs: 1234 },
            { name: 'b.bin', mimeType: 'application/octet-stream', buffer: btoa(String.fromCharCode(0, 255, 7)) },
        ];
        const files = payloads.map((file) => {
            const bytes = Uint8Array.from(atob(file.buffer), (c) => c.charCodeAt(0));
            return new File([bytes], file.name, { type: file.mimeType, lastModified: file.lastModifiedMs });
        });
        const dt = new DataTransfer();
        for (const file of files)
            dt.items.add(file);
        input.files = dt.files;
        input.dispatchEvent(new Event('input', { bubbles: true, composed: true }));
        input.dispatchEvent(new Event('change', { bubbles: true }));
        const fl = input.files;
        [fl instanceof FileList, fl.length, fl[0] instanceof File, fl[0].name, fl[0].type, fl[0].size, fl[0].lastModified,
         fl.item(1).name, fl[1].type, fl[1].size, fl[1].lastModified > 0, input.value, seen.join(',')].join('|')
    "#
        ),
        "true|2|true|a.txt|text/plain|2|1234|b.bin|application/octet-stream|3|true|C:\\fakepath\\a.txt|input:true:true:2,change:true:2"
    );
    // The bytes survive into the selection and into form submission data.
    eval(r, "window.texts = []; input.files[0].text().then(t => texts.push(t)); input.files[1].arrayBuffer().then(b => texts.push([...new Uint8Array(b)].join('.'))); 1");
    assert_eq!(eval(r, "texts.join('|')"), "hi|0.255.7");
    assert_eq!(
        eval(r, "[...new FormData(document.getElementById('fm'))].map(([k, v]) => k + '=' + (v instanceof File ? v.name + ':' + v.size + ':' + v.type : v)).join('&')"),
        "up=a.txt:2:text/plain&up=b.bin:3:application/octet-stream&t=v"
    );
    // Assigning a replacement selection, and an empty one, replaces it.
    assert_eq!(
        eval(r, "const d2 = new DataTransfer(); d2.items.add(new File(['xyz'], 'c.csv', { type: 'text/csv' })); input.files = d2.files; [input.files.length, input.files[0].name, input.value].join('|')"),
        "1|c.csv|C:\\fakepath\\c.csv"
    );
    assert_eq!(
        eval(r, "input.files = new DataTransfer().files; [input.files.length, JSON.stringify(input.value)].join('|')"),
        "0|\"\""
    );
    // `value = ''` is the usual reset; it clears both the name and the files.
    assert_eq!(
        eval(r, "input.files = dt.files; const before = input.files.length; input.value = ''; [before, input.files.length, JSON.stringify(input.value)].join('|')"),
        "2|0|\"\""
    );
    // Null is ignored; non-FileList values throw (WebIDL `FileList?`).
    assert_eq!(
        eval(
            r,
            r#"
        input.files = dt.files;
        const out = [];
        input.files = null; out.push(input.files.length);
        input.files = undefined; out.push(input.files.length);
        for (const bad of [{}, [new File(['x'], 'x')], 'a.txt', new File(['x'], 'x')]) {
            try { input.files = bad; out.push('no throw'); } catch (err) { out.push(err instanceof TypeError); }
        }
        out.push(input.files.length, Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'files').set !== undefined);
        out.join('|')
    "#
        ),
        "2|2|true|true|true|true|2|true"
    );
    // A non-file input keeps `files === null` and ignores the assignment.
    assert_eq!(
        eval(r, "const t = document.getElementById('t'); t.files = dt.files; [t.files, t.value].join('|')"),
        "|v"
    );
}

#[test]
fn wheel_and_touch_listeners_on_window_document_html_body_default_to_passive() {
    let e = env("<body><div id=d></div></body>");
    let r = &e.main;
    assert_eq!(
        eval(
            r,
            r#"
        const out = [];
        const cancel = (target, type, opts) => {
            const seen = [];
            const fn = ev => { ev.preventDefault(); seen.push(ev.defaultPrevented); };
            target.addEventListener(type, fn, opts);
            const ev = type === 'wheel' ? new WheelEvent(type, { cancelable: true, bubbles: true }) : new Event(type, { cancelable: true, bubbles: true });
            const notCanceled = d.dispatchEvent(ev);
            target.removeEventListener(type, fn, opts);
            return seen.join() + ':' + notCanceled;
        };
        const d = document.getElementById('d');
        for (const t of [window, document, document.documentElement, document.body])
            out.push(cancel(t, 'wheel'));
        out.push(cancel(document, 'touchstart'), cancel(window, 'touchmove'), cancel(document.body, 'mousewheel'));
        // Explicit passive:false, other targets and other event types still cancel.
        out.push(cancel(document, 'wheel', { passive: false }), cancel(d, 'wheel'), cancel(document, 'click'));
        out.push(cancel(document, 'wheel', true));
        out.join('|')
    "#
        ),
        "false:true|false:true|false:true|false:true|false:true|false:true|false:true|true:false|true:false|true:false|false:true"
    );
}

#[test]
fn document_open_write_close_replaces_a_loaded_document() {
    let e = env("<body><p id=o>orig</p></body>");
    let r = &e.main;
    eval(
        r,
        "window.keep = 'kept'; window.oldClicks = 0; window.addEventListener('click', () => oldClicks++); \
         document.addEventListener('custom', () => window.oldDoc = true); window.onload = () => window.oldLoad = true; 1",
    );
    // Playwright's page.setContent, verbatim.
    assert_eq!(
        eval(
            r,
            r#"
        const returned = document.open() === document;
        document.write('<!doctype html><html><head><title>t</title></head><body><p id=a>a</p>' +
            '<script>window.log = [document.readyState, document.currentScript.tagName, typeof keep, !!document.getElementById("a"), !!document.getElementById("o")];' +
            'document.addEventListener("DOMContentLoaded", () => log.push("DCL:" + document.readyState));' +
            'window.addEventListener("load", () => log.push("load:" + document.readyState));' +
            'document.write("<p id=w>w<scr" + "ipt>log.push(\x27nested\x27)</scr" + "ipt></p>"); log.push(!!document.getElementById("w"));<\/script>');
        document.write('<p id=after>after</p>');
        document.writeln('<script type=module>log.push("module")<\/script>'.replace('module', 'text/x-data'));
        document.close();
        [returned, log.join(), document.title, [...document.body.children].map(x => x.id || x.tagName).join(), document.readyState].join('|')
    "#
        ),
        "true|loading,SCRIPT,string,true,false,nested,true,DCL:interactive,load:complete|t|a,SCRIPT,w,after,SCRIPT|complete"
    );
    // Old listeners and handlers were erased; handlers can be set again.
    assert_eq!(
        eval(
            r,
            "document.dispatchEvent(new Event('custom')); document.body.click(); \
             let n = 0; const before = window.onload; window.onload = null; document.body.onclick = () => n++; document.body.click(); \
             [String(window.oldDoc), oldClicks, before, n, String(window.oldLoad)].join()"
        ),
        "undefined,0,,1,undefined"
    );
    // Implicit open on a loaded document; external scripts are reported.
    assert_eq!(
        eval(r, "document.write('<p id=x>x</p>'); const t = [document.readyState, document.body.children.length]; document.close(); [t, document.readyState].join()"),
        "loading,1,complete"
    );
    assert_eq!(
        eval(
            r,
            r#"try { document.write('<script src=/a.js><\/script>'); 'no throw' } catch (e) { document.close(); e.name }"#
        ),
        "NotSupportedError"
    );
    assert_eq!(
        eval(
            r,
            "const x = document.implementation.createDocument(null, 'r'); try { x.open(); 'no' } catch (e) { e.name }"
        ),
        "InvalidStateError"
    );
}

/// Contenteditable editing against sequences recorded in headless Chromium
/// (Playwright 1.59.1) for `<div id=ed contenteditable><p>hello</p></div>`.
fn ce_env() -> Env {
    let e = env("<body><input id=other><div id=ed contenteditable=true><p>hello</p></div></body>");
    eval(
        &e.main,
        r#"
        window.log = [];
        const ed = document.getElementById('ed');
        for (const t of ['keydown', 'beforeinput', 'input'])
            document.addEventListener(t, ev => log.push(t + (ev.inputType ? ':' + ev.inputType + ':' + (ev.data ?? '') : '') + (t === 'keydown' ? ':' + ev.key : '')), true);
        window.sel = () => { const s = getSelection(); const n = x => x ? (x.nodeType === 3 ? '#text"' + x.data + '"' : x.nodeName) : null;
            return [s.rangeCount, n(s.anchorNode), s.anchorOffset, n(s.focusNode), s.focusOffset, document.activeElement.id].join(','); };
        window.state = () => [ed.innerHTML, sel(), log.splice(0).join(' ')].join(' | ');
        1"#,
    );
    e
}

fn key(page: &Rc<PageDom>, key: &str, text: &str, modifiers: input::Modifiers) {
    let k = input::KeyInput { key: key.into(), code: key.into(), text: text.into(), modifiers, ..Default::default() };
    input::dispatch_key(page, if text.is_empty() { input::KeyKind::RawKeyDown } else { input::KeyKind::KeyDown }, &k);
    input::dispatch_key(page, input::KeyKind::KeyUp, &k);
}

fn click_p(e: &Env) {
    let r = eval(&e.main, "const b = document.querySelector('#ed p').getBoundingClientRect(); [b.x + b.width / 2, b.y + b.height / 2].join()");
    let (x, y) = r.split_once(',').unwrap();
    let m = input::MouseInput {
        x: x.parse().unwrap(),
        y: y.parse().unwrap(),
        button: input::MouseButton::Left,
        buttons: 1,
        click_count: 1,
        ..Default::default()
    };
    input::dispatch_mouse(&e.page, input::MouseKind::Pressed, &m).unwrap();
    input::dispatch_mouse(&e.page, input::MouseKind::Released, &input::MouseInput { buttons: 0, ..m }).unwrap();
}

#[test]
fn contenteditable_typing_deleting_and_caret_keys_match_chromium() {
    let none = input::Modifiers::default();
    // click + type
    let e = ce_env();
    click_p(&e);
    key(&e.page, "X", "X", none);
    assert_eq!(
        eval(&e.main, "state()"),
        r#"<p>helloX</p> | 1,#text"helloX",6,#text"helloX",6,ed | keydown:X beforeinput:insertText:X input:insertText:X"#
    );
    // click + ArrowLeft ×2 + type
    drop(e);
    let e = ce_env();
    click_p(&e);
    key(&e.page, "ArrowLeft", "", none);
    key(&e.page, "ArrowLeft", "", none);
    key(&e.page, "Y", "Y", none);
    assert_eq!(
        eval(&e.main, "[ed.innerHTML, sel()].join(' | ')"),
        r#"<p>helYlo</p> | 1,#text"helYlo",4,#text"helYlo",4,ed"#
    );
    // click + Backspace
    drop(e);
    let e = ce_env();
    click_p(&e);
    key(&e.page, "Backspace", "", none);
    assert_eq!(
        eval(&e.main, "state()"),
        r#"<p>hell</p> | 1,#text"hell",4,#text"hell",4,ed | keydown:Backspace beforeinput:deleteContentBackward: input:deleteContentBackward:"#
    );
    // select all + Backspace leaves Chrome's placeholder
    drop(e);
    let e = ce_env();
    click_p(&e);
    key(&e.page, "a", "", input::Modifiers { meta: true, ..none });
    key(&e.page, "Backspace", "", none);
    assert_eq!(eval(&e.main, "[ed.innerHTML, sel()].join(' | ')"), "<p><br></p> | 1,P,0,P,0,ed");
    // focus puts the caret at the start; End moves it to the end
    drop(e);
    let e = ce_env();
    eval(&e.main, "ed.focus(); 1");
    assert_eq!(eval(&e.main, "sel()"), r#"1,#text"hello",0,#text"hello",0,ed"#);
    key(&e.page, "End", "", none);
    key(&e.page, "!", "!", none);
    assert_eq!(eval(&e.main, "ed.innerHTML"), "<p>hello!</p>");
    // JS collapse + type
    drop(e);
    let e = ce_env();
    eval(&e.main, "ed.focus(); getSelection().collapse(document.querySelector('#ed p').firstChild, 2); 1");
    key(&e.page, "Z", "Z", none);
    assert_eq!(
        eval(&e.main, "[ed.innerHTML, sel()].join(' | ')"),
        r#"<p>heZllo</p> | 1,#text"heZllo",3,#text"heZllo",3,ed"#
    );
    // Enter splits the paragraph
    drop(e);
    let e = ce_env();
    click_p(&e);
    key(&e.page, "Enter", "\r", none);
    key(&e.page, "W", "W", none);
    assert_eq!(
        eval(&e.main, "state()"),
        r#"<p>hello</p><p>W</p> | 1,#text"W",1,#text"W",1,ed | keydown:Enter beforeinput:insertParagraph: input:insertParagraph: keydown:W beforeinput:insertText:W input:insertText:W"#
    );
    // A canceled beforeinput leaves the DOM alone (ProseMirror handles keys itself).
    drop(e);
    let e = ce_env();
    eval(&e.main, "ed.addEventListener('beforeinput', ev => ev.preventDefault()); 1");
    click_p(&e);
    key(&e.page, "Q", "Q", none);
    assert_eq!(eval(&e.main, "ed.innerHTML"), "<p>hello</p>");
}

#[test]
fn contenteditable_fill_uses_the_document_selection_from_another_world() {
    // Playwright's fill(): focus + selectNodeContents/addRange in its utility
    // world, then Input.insertText.
    let e = ce_env();
    let class = e.engine.register::<TestWindow>();
    let utility = new_realm(&e.engine, class, &e.page, "__playwright_utility_world__");
    eval(
        &utility,
        "const el = document.getElementById('ed'); el.focus(); const r = document.createRange(); r.selectNodeContents(el); \
         const s = getSelection(); s.removeAllRanges(); s.addRange(r); 1",
    );
    assert_eq!(eval(&e.main, "getSelection().rangeCount + ':' + getSelection().toString()"), "1:hello");
    input::insert_text(&e.page, "filled");
    assert_eq!(
        eval(&e.main, "state()"),
        r#"<p>filled</p> | 1,#text"filled",6,#text"filled",6,ed | beforeinput:insertText:filled input:insertText:filled"#
    );
    // fill('') then deletes the selection, leaving the placeholder.
    eval(&utility, "const r2 = document.createRange(); r2.selectNodeContents(el); getSelection().removeAllRanges(); getSelection().addRange(r2); 1");
    input::insert_text(&e.page, "");
    assert_eq!(eval(&e.main, "[ed.innerHTML, sel()].join(' | ')"), "<p><br></p> | 1,P,0,P,0,ed");
    // Selection offsets are validated.
    assert_eq!(
        eval(&e.main, "try { getSelection().collapse(ed, 9); 'no' } catch (err) { err.name }"),
        "IndexSizeError"
    );
}

#[test]
fn drag_event_carries_its_data_transfer() {
    let e = env("<body><div id=d></div></body>");
    let r = &e.main;
    assert_eq!(
        eval(
            r,
            r#"
        const dt = new DataTransfer();
        dt.items.add(new File([new Uint8Array([1, 2])], 'x.bin', { type: 'application/octet-stream' }));
        let seen = '';
        document.body.addEventListener('drop', (ev) => {
            seen = [ev instanceof DragEvent, ev instanceof MouseEvent, ev.dataTransfer === dt, ev.dataTransfer.files[0].name,
                    ev.dataTransfer.types.join(), ev.type, ev.bubbles, ev.isTrusted, ev.clientX].join(',');
            ev.preventDefault();
        });
        const ok = document.getElementById('d').dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: dt, clientX: 3 }));
        const out = [seen, ok, new DragEvent('dragover').dataTransfer, new DragEvent('dragover', { dataTransfer: null }).dataTransfer];
        try { new DragEvent('drop', { dataTransfer: {} }); out.push('no throw'); } catch (err) { out.push(err.constructor.name); }
        const created = document.createEvent('DragEvent');
        out.push(created instanceof DragEvent, created.dataTransfer, DragEvent.prototype.__proto__ === MouseEvent.prototype);
        out.join('|')
    "#
        ),
        "true,true,true,x.bin,Files,drop,true,false,3|false|||TypeError|true||true"
    );
}

#[test]
fn data_transfer_holds_no_roots_and_get_as_string_is_queued() {
    let e = env("<body>");
    let r = &e.main;
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    eval(
        r,
        "window.got = []; const dt = new DataTransfer(); dt.items.add('payload', 'text/plain'); dt.items.add(new File(['f'], 'f.txt')); \
         dt.items[0].getAsString((s) => got.push(s)); dt.items[1].getAsString(() => got.push('file item: no callback')); \
         dt.items[0].getAsString('not a function'); got.push('sync');",
    );
    assert_eq!(eval(r, "got.join()"), "sync", "the callback runs in a later task");
    tasks.drain();
    assert_eq!(eval(r, "got.join()"), "sync,payload");
    // Neither the store, its cached item list, nor an event that carried it keeps anything alive.
    eval(
        r,
        "window.refs = (() => { const dt = new DataTransfer(); dt.setData('text', 'x'); dt.items.add(new File(['abc'], 'a.txt')); \
         const items = dt.items; const ev = new DragEvent('drop', { dataTransfer: dt }); document.body.dispatchEvent(ev); \
         return [new WeakRef(dt), new WeakRef(items), new WeakRef(ev)]; })(); 1",
    );
    for _ in 0..4 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
        e.page.after_task();
        tasks.drain();
    }
    assert_eq!(eval(r, "refs.map((w) => w.deref() === undefined).join()"), "true,true,true");
}

#[test]
fn data_transfer_items_back_reference_is_collectable() {
    let e = env("<body>");
    let r = &e.main;
    eval(r, "window.kept = new DataTransfer(); kept.items.owner = kept; kept.items.marker = 17; window.refs = [new WeakRef(kept), new WeakRef(kept.items)]; 1");
    for _ in 0..6 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
    }
    assert_eq!(
        eval(r, "[kept.items === refs[1].deref(), kept.items.owner === kept, kept.items.marker].join()"),
        "true,true,17"
    );
    eval(r, "kept = null; 1");
    for _ in 0..6 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
    }
    assert_eq!(eval(r, "refs.map(w => w.deref() === undefined).join()"), "true,true");
}

#[test]
fn drag_event_data_transfer_back_reference_is_collectable() {
    let e = env("<body>");
    let r = &e.main;
    eval(r, "window.kept = (() => { const dt = new DataTransfer(); dt.setData('text', 'payload'); dt.ev = new DragEvent('drop', {dataTransfer: dt}); return dt.ev; })(); window.refs = [new WeakRef(kept), new WeakRef(kept.dataTransfer)]; 1");
    for _ in 0..6 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
    }
    assert_eq!(eval(r, "[kept.dataTransfer === refs[1].deref(), kept.dataTransfer.ev === kept, kept.dataTransfer.getData('text')].join()"), "true,true,payload");
    eval(r, "kept = null; 1");
    for _ in 0..6 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
    }
    assert_eq!(eval(r, "refs.map(w => w.deref() === undefined).join()"), "true,true");
}

#[test]
fn drag_event_data_transfer_identity_survives_isolated_world_and_gc() {
    let e = env("<body>");
    let isolated = new_realm(&e.engine, e.engine.register::<TestWindow>(), &e.page, "isolated");
    eval(&isolated, "document.body.addEventListener('drop', ev => { window.saved = ev; });");
    eval(&e.main, "window.refs = (() => { const dt = new DataTransfer(); dt.items.owner = dt; dt.items.marker = 42; dt.ev = new DragEvent('drop', {dataTransfer: dt}); document.body.dispatchEvent(dt.ev); return [new WeakRef(dt), new WeakRef(dt.items), new WeakRef(dt.ev)]; })(); 1");
    for _ in 0..6 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
    }
    assert_eq!(eval(&isolated, "[saved.dataTransfer === saved.dataTransfer.ev.dataTransfer, Object.getOwnPropertyDescriptor(DataTransfer.prototype, 'items').get.call(saved.dataTransfer) === saved.dataTransfer.items, saved.dataTransfer.items.marker].join()"), "true,true,42");
    assert_eq!(eval(&e.main, "refs.map(w => w.deref() !== undefined).join()"), "true,true,true");
    eval(&isolated, "saved = null; 1");
    for _ in 0..6 {
        e.engine.collect_garbage_now();
        e.engine.drain_finalized();
    }
    assert_eq!(eval(&e.main, "refs.map(w => w.deref() === undefined).join()"), "true,true,true");
}

#[test]
fn dom_parser_xml_office_namespaces_and_live_qualified_queries() {
    let e = env("<body>");
    assert_eq!(
        eval(
            &e.main,
            r#"globalThis.xml=new DOMParser().parseFromString('<w:document xmlns:w="urn:word" xmlns:x="urn:other"><w:body><w:p w:id="1"><w:t>Hello &amp; XML</w:t></w:p><x:p/><p/></w:body></w:document>','application/xml'); [xml instanceof XMLDocument,xml.contentType,xml.URL===document.URL,xml.documentElement.nodeName,xml.getElementsByTagName('w:p').length,xml.getElementsByTagName('p').length,xml.getElementsByTagNameNS('urn:word','p').length,xml.getElementsByTagNameNS('urn:other','p').length,xml.getElementsByTagNameNS(null,'p').length,xml.getElementsByTagNameNS('*','p').length,xml.getElementsByTagNameNS('urn:word','*').length,xml.getElementsByTagName('w:p')[0].getAttributeNS('urn:word','id'),xml.getElementsByTagName('w:t')[0].textContent].join('|')"#
        ),
        "true|application/xml|true|w:document|1|1|1|1|1|3|4|1|Hello & XML"
    );
    assert_eq!(eval(&e.main, "[xml.lookupNamespaceURI('w'),xml.getElementsByTagName('w:t')[0].firstChild.lookupNamespaceURI('x'),xml.documentElement.isDefaultNamespace(null)].join()"), "urn:word,urn:other,true");
    assert_eq!(eval(&e.main, "globalThis.paragraphs=xml.getElementsByTagNameNS('urn:word','p');xml.documentElement.appendChild(xml.createElementNS('urn:word','other:p'));paragraphs.length+':'+xml.documentElement.getElementsByTagNameNS('urn:word','p').length"), "2:2");
    assert_eq!(
        eval(&e.main, "xml.getElementsByTagName('w:body')[0].getElementsByTagNameNS('urn:other','*').length"),
        "1"
    );
}

#[test]
fn dom_parser_xml_types_case_cdata_errors_and_inert_adoption() {
    let e = env("<body>");
    assert_eq!(
        eval(
            &e.main,
            r#"['text/xml','application/xml','application/xhtml+xml','image/svg+xml'].map(type=>{const d=new DOMParser().parseFromString('<R><Item/><item/></R>',type);return [d.contentType,d.getElementsByTagName('Item').length,d.getElementsByTagName('item').length].join(':')}).join('|')"#
        ),
        "text/xml:1:1|application/xml:1:1|application/xhtml+xml:1:1|image/svg+xml:1:1"
    );
    assert_eq!(eval(&e.main, "const d=new DOMParser().parseFromString('<R>a<![CDATA[<b>]]><!--c--><?work x?></R>','text/xml'); [...d.documentElement.childNodes].map(n=>n.nodeType+':'+n.nodeValue).join('|')"), "3:a|4:<b>|8:c|7:x");
    assert_eq!(
        eval(
            &e.main,
            r#"['<r>','<r/><s/>','<r>&bad;</r>',"<!DOCTYPE r SYSTEM 'file:///unused'><r/>"].map(x=>new DOMParser().parseFromString(x,'text/xml').documentElement.localName).join()"#
        ),
        "parsererror,parsererror,parsererror,parsererror"
    );
    assert_eq!(
        eval(
            &e.main,
            r#"globalThis.inert=new DOMParser().parseFromString('<script xmlns="http://www.w3.org/1999/xhtml">window.xmlRan=1</script>','application/xhtml+xml');document.body.appendChild(document.adoptNode(inert.documentElement));typeof window.xmlRan"#
        ),
        "undefined"
    );
    let dom = e.page.dom.borrow();
    let body = dom.body(e.page.document()).unwrap().unwrap();
    let script = dom.last_child(body).unwrap().unwrap();
    assert!(dom.script_info(script).unwrap().already_started);
}

#[test]
fn dom_parser_xml_argument_reentry_and_detached_document_isolation() {
    let e = env("<body>");
    assert_eq!(eval(&e.main, "const old=document; const d=new DOMParser().parseFromString({toString(){document.body.textContent='converted';return '<r a=\"ok\"/>'}},{toString(){document.body.appendChild(document.createElement('p'));return 'text/xml'}});[d!==document,old===document,document.body.textContent,d.documentElement.getAttribute('a'),d.defaultView===null].join()"), "true,true,converted,ok,true");
}

#[derive(Default)]
struct IframeHookState {
    calls: RefCell<Vec<String>>,
    documents: RefCell<std::collections::HashMap<lp_dom::NodeId, lp_dom::NodeId>>,
    navigations: RefCell<Vec<String>>,
}
struct IframeHooks {
    page: std::rc::Weak<PageDom>,
    state: Rc<IframeHookState>,
}
impl PageHooks for IframeHooks {
    fn post_task(&self, task: Box<dyn FnOnce()>) {
        task();
    }
    fn navigate(&self, request: NavigationRequest) {
        self.state.navigations.borrow_mut().push(request.url);
    }
    fn iframe_connected(&self, iframe: lp_dom::NodeId) {
        let page = self.page.upgrade().unwrap();
        let dom = page.dom.borrow_mut(); // Must not be called under a DOM borrow.
        let name = dom.get_attribute(iframe, "name").unwrap().unwrap_or("").to_owned();
        let srcdoc = dom.get_attribute(iframe, "srcdoc").unwrap().unwrap_or("").to_owned();
        assert!(dom.is_connected(iframe).unwrap());
        drop(dom);
        let child = parse_document(&page.dom, "about:blank", "<body></body>");
        page.dom.borrow_mut().set_document_active(child, true).unwrap();
        self.state.documents.borrow_mut().insert(iframe, child);
        self.state.calls.borrow_mut().push(format!("connect:{name}:{srcdoc}"));
    }
    fn iframe_disconnected(&self, iframe: lp_dom::NodeId) {
        let page = self.page.upgrade().unwrap();
        let mut dom = page.dom.borrow_mut();
        dom.collect_garbage(); // Remaining notifications must still hold live node IDs.
        let name = dom.get_attribute(iframe, "name").unwrap().unwrap_or("").to_owned();
        if let Some(child) = self.state.documents.borrow_mut().remove(&iframe) {
            dom.set_document_active(child, false).unwrap();
        }
        self.state.calls.borrow_mut().push(format!("disconnect:{name}"));
    }
    fn iframe_attribute_changed(&self, iframe: lp_dom::NodeId, attribute: IframeAttribute) {
        let page = self.page.upgrade().unwrap();
        let dom = page.dom.borrow_mut();
        assert!(dom.is_connected(iframe).unwrap());
        self.state.calls.borrow_mut().push(format!("attr:{attribute:?}"));
    }
    fn frame_document(&self, iframe: lp_dom::NodeId) -> Option<lp_dom::NodeId> {
        self.state.documents.borrow().get(&iframe).copied()
    }
    fn frame_window<'s>(&self, s: &lp_js::Scope<'s>, iframe: lp_dom::NodeId) -> Option<lp_js::Value<'s>> {
        self.state.documents.borrow().contains_key(&iframe).then(|| s.global().value())
    }
}
fn iframe_hooks(e: &Env) -> Rc<IframeHookState> {
    let state = Rc::new(IframeHookState::default());
    e.page.set_hooks(Some(Rc::new(IframeHooks { page: Rc::downgrade(&e.page), state: state.clone() })));
    state
}

#[test]
fn iframe_hooks_connect_before_append_returns_and_report_attributes() {
    let e = env("<body>");
    let state = iframe_hooks(&e);
    assert_eq!(eval(&e.main, "globalThis.f=document.createElement('iframe'); f.name='first'; f.srcdoc='hello'; document.body.appendChild(f); !!f.contentDocument && f.contentWindow===window"), "true");
    assert_eq!(&*state.calls.borrow(), &["connect:first:hello"]);
    eval(&e.main, "f.src='/next'; f.srcdoc='next'; f.name='second'; f.sandbox.add('allow-scripts'); f.removeAttribute('srcdoc'); f.width='90'");
    assert_eq!(&state.calls.borrow()[1..], &["attr:Src", "attr:Srcdoc", "attr:Name", "attr:Sandbox", "attr:Srcdoc"]);
    eval(&e.main, "f.remove(); f.name='detached'; f.srcdoc='detached'; document.body.appendChild(f)");
    assert_eq!(&state.calls.borrow()[6..], &["disconnect:second", "connect:detached:detached"]);
    assert_eq!(eval(&e.main, "!!f.contentDocument"), "true");
}

#[test]
fn iframe_hooks_cover_subtree_fragment_inner_html_and_stale_native_events() {
    let e = env("<body>");
    let state = iframe_hooks(&e);
    eval(&e.main, "globalThis.box=document.createElement('div'); box.innerHTML='<iframe name=a></iframe><span><iframe name=b></iframe></span>'; document.body.appendChild(box)");
    assert_eq!(&*state.calls.borrow(), &["connect:a:", "connect:b:"]);
    eval(&e.main, "box.remove()");
    assert_eq!(&state.calls.borrow()[2..], &["disconnect:b", "disconnect:a"]);
    eval(&e.main, "const frag=document.createDocumentFragment(); frag.appendChild(box); document.body.appendChild(frag); box.innerHTML='<iframe name=c></iframe>'");
    assert_eq!(&state.calls.borrow()[4..], &["connect:a:", "connect:b:", "disconnect:a", "disconnect:b", "connect:c:"]);
    let f = {
        let mut dom = e.page.dom.borrow_mut();
        let f = dom.create_element(e.page.document(), "iframe").unwrap();
        let body = dom.body(e.page.document()).unwrap().unwrap();
        dom.append_child(body, f).unwrap();
        dom.remove_child(body, f).unwrap();
        f
    };
    e.page.flush_iframe_events();
    assert_eq!(state.calls.borrow().len(), 9, "do not attach a node removed before native flush");
    e.page.dom.borrow_mut().take_resource_events();
    e.page.dom.borrow_mut().collect_garbage();
    assert!(!e.page.dom.borrow().is_alive(f), "discarded notifications release their pins");
}

#[test]
fn iframe_hooks_parser_flush_and_document_routing() {
    let e = env("<body><iframe name=parser></iframe>");
    let state = iframe_hooks(&e);
    e.page.flush_iframe_events();
    assert_eq!(&*state.calls.borrow(), &["connect:parser:"]);
    let other = parse_document(&e.page.dom, "http://localhost/child", "<body><iframe name=other></iframe>");
    let other_page = PageDom::new(e.page.dom.clone(), other);
    let other_state = Rc::new(IframeHookState::default());
    other_page.set_hooks(Some(Rc::new(IframeHooks { page: Rc::downgrade(&other_page), state: other_state.clone() })));
    e.page.after_task();
    assert_eq!(state.calls.borrow().len(), 1);
    other_page.flush_iframe_events();
    assert_eq!(&*other_state.calls.borrow(), &["connect:other:"]);
    other_page.dispose();
    e.page.flush_iframe_events();
    assert_eq!(state.calls.borrow().len(), 1);
}

#[test]
fn scripting_disabled_blocks_handlers_and_javascript_navigation_but_not_eval() {
    let e = env("<body><button id=b></button><a id=a href='javascript:globalThis.ran=99'>link</a>");
    let state = iframe_hooks(&e);
    eval(&e.main, "globalThis.b=document.getElementById('b'); globalThis.a=document.getElementById('a'); globalThis.ran=0; b.setAttribute('onclick','ran++'); b.addEventListener('click',()=>ran+=10)");
    e.page.set_scripting_disabled(true);
    assert_eq!(eval(&e.main, "b.click(); a.click(); ran"), "0");
    assert!(state.navigations.borrow().is_empty());
    assert_eq!(
        eval(&e.main, "b.setAttribute('onclick','not valid {'); 6*7"),
        "42",
        "disabled attributes must not compile, inspector evaluation remains available"
    );
    e.page.set_scripting_disabled(false);
    assert_eq!(eval(&e.main, "b.setAttribute('onclick','ran++'); b.click(); ran"), "11");
    eval(&e.main, "a.click()");
    assert_eq!(state.navigations.borrow().len(), 1);
}

#[test]
fn iframe_hooks_disconnect_nested_owners_before_ancestors_and_drop_disposed_queue() {
    let e = env("<body>");
    let state = iframe_hooks(&e);
    eval(&e.main, "globalThis.outer=document.createElement('iframe'); outer.name='outer'; const inner=document.createElement('iframe'); inner.name='inner'; outer.appendChild(inner); document.body.appendChild(outer); outer.remove()");
    assert_eq!(&*state.calls.borrow(), &["connect:outer:", "connect:inner:", "disconnect:inner", "disconnect:outer"]);
    let f = {
        let mut dom = e.page.dom.borrow_mut();
        let f = dom.create_element(e.page.document(), "iframe").unwrap();
        let body = dom.body(e.page.document()).unwrap().unwrap();
        dom.append_child(body, f).unwrap();
        dom.remove_child(body, f).unwrap();
        f
    };
    e.page.dispose();
    e.page.flush_iframe_events();
    e.page.dom.borrow_mut().take_resource_events();
    e.page.dom.borrow_mut().collect_garbage();
    assert!(!e.page.dom.borrow().is_alive(f));
    assert_eq!(state.calls.borrow().len(), 4);
}

#[test]
fn iframe_hooks_ignore_inert_documents_and_survive_adoption_and_shadow_removal() {
    let e = env("<body>");
    let state = iframe_hooks(&e);
    eval(&e.main, "globalThis.parsed=new DOMParser().parseFromString('<iframe name=inert></iframe>','text/html'); globalThis.f=parsed.querySelector('iframe'); f.srcdoc='before-connect'");
    assert!(state.calls.borrow().is_empty());
    eval(&e.main, "globalThis.host=document.createElement('div'); const shadow=host.attachShadow({mode:'open'}); shadow.appendChild(f); document.body.appendChild(host)");
    assert_eq!(&*state.calls.borrow(), &["connect:inert:before-connect"]);
    eval(&e.main, "host.remove()");
    assert_eq!(&state.calls.borrow()[1..], &["disconnect:inert"]);
}

#[test]
fn iframe_hooks_defer_while_dom_is_borrowed_and_replace_document_queue() {
    let e = env("<body>");
    let state = iframe_hooks(&e);
    let replacement = parse_document(&e.page.dom, "http://localhost/new", "<body><iframe name=new></iframe>");
    {
        let mut dom = e.page.dom.borrow_mut();
        let f = dom.create_element(e.page.document(), "iframe").unwrap();
        let body = dom.body(e.page.document()).unwrap().unwrap();
        dom.append_child(body, f).unwrap();
        e.page.flush_iframe_events();
        assert!(state.calls.borrow().is_empty());
    }
    e.page.set_document(replacement);
    e.page.flush_iframe_events();
    assert_eq!(&*state.calls.borrow(), &["connect:new:"]);
}

#[test]
fn iframe_document_value_preserves_child_binding_state_and_null_denies_fallback() {
    struct ChildDocumentHook {
        document: lp_js::Persistent,
        legacy: lp_dom::NodeId,
        deny: Rc<std::cell::Cell<bool>>,
    }
    impl PageHooks for ChildDocumentHook {
        fn post_task(&self, task: Box<dyn FnOnce()>) {
            task();
        }
        fn navigate(&self, _: NavigationRequest) {}
        fn frame_document_value<'s>(
            &self,
            s: &lp_js::Scope<'s>,
            _: lp_dom::NodeId,
        ) -> lp_js::JsResult<Option<lp_js::Value<'s>>> {
            Ok(Some(if self.deny.get() { s.null() } else { self.document.get(s) }))
        }
        fn frame_document(&self, _: lp_dom::NodeId) -> Option<lp_dom::NodeId> {
            Some(self.legacy)
        }
    }
    let e = env("<body><iframe></iframe>");
    let doc = parse_document(&e.page.dom, "http://child.example/", "<body><p>child</p>");
    e.page.dom.borrow_mut().set_document_active(doc, true).unwrap();
    let child = PageDom::new(e.page.dom.clone(), doc);
    let class = e.engine.register::<TestWindow>();
    let realm = new_realm(&e.engine, class, &child, "child");
    let document = {
        let s = realm.scope().unwrap();
        lp_js::Persistent::new(&s, s.eval("document", None, 1).unwrap())
    };
    let deny = Rc::new(std::cell::Cell::new(false));
    e.page.set_hooks(Some(Rc::new(ChildDocumentHook { document, legacy: e.page.document(), deny: deny.clone() })));
    assert_eq!(eval(&e.main, "globalThis.f=document.querySelector('iframe'); f.contentDocument===f.contentDocument && f.contentDocument.body.textContent==='child'"), "true");
    {
        let s = e.main.scope().unwrap();
        let value = s.eval("f.contentDocument", None, 1).unwrap();
        let node = s.unwrap::<lp_webapi::node::Node>(value).unwrap();
        assert!(Rc::ptr_eq(node.page(), &child));
        let value = s.eval("f.contentDocument.body.firstChild", None, 1).unwrap();
        let node = s.unwrap::<lp_webapi::node::Node>(value).unwrap();
        assert!(Rc::ptr_eq(node.page(), &child));
    }
    deny.set(true);
    assert_eq!(eval(&e.main, "f.contentDocument===null"), "true");
    e.page.set_hooks(None);
    child.dispose();
}

#[test]
fn inherited_base_resolves_resources_without_changing_document_url() {
    let e = env("<body><a href='next'>next</a><script src='script.js'></script>");
    let doc = e.page.document();
    {
        let mut dom = e.page.dom.borrow_mut();
        dom.set_document_url(doc, "about:srcdoc").unwrap();
        dom.set_document_base_url(doc, Some("https://parent.example/dir/page")).unwrap();
    }
    assert_eq!(eval(&e.main, "[document.URL,document.documentURI,document.baseURI,document.body.baseURI,document.querySelector('a').href,document.querySelector('script').src,Response.redirect('next').headers.get('location')].join('|')"),
        "about:srcdoc|about:srcdoc|https://parent.example/dir/page|https://parent.example/dir/page|https://parent.example/dir/next|https://parent.example/dir/script.js|https://parent.example/dir/next");
    assert_eq!(
        eval(&e.main, "let relativeXHR=new XMLHttpRequest();relativeXHR.open('GET','data');relativeXHR.readyState"),
        "1"
    );
    let detached = parse_document(&e.page.dom, "https://other.example/", "<body><a href='next'>");
    assert_eq!(e.page.dom.borrow().document_base_url(detached).unwrap(), "https://other.example/");
    e.page.dom.borrow_mut().set_document_url(doc, "about:blank").unwrap();
    assert_eq!(
        eval(&e.main, "[document.URL,document.baseURI].join('|')"),
        "about:blank|https://parent.example/dir/page"
    );
    e.page.dom.borrow_mut().set_document_base_url(doc, None).unwrap();
    assert_eq!(eval(&e.main, "document.baseURI"), "about:blank");
}

#[test]
fn explicit_opaque_realm_blocks_http_document_cookies_and_storage() {
    let e = env("<body>");
    let class = e.engine.register::<TestWindow>();
    let opaque = new_realm_with_origin(&e.engine, class, &e.page, "opaque", Some("null".into()));
    assert_eq!(eval(&opaque, "[()=>document.cookie,()=>document.cookie='x=y'].map(f=>{try{f();return 'allowed'}catch(e){return e.name}}).join()"), "SecurityError,SecurityError");
    eval(&e.main, "localStorage.setItem('secret','parent');sessionStorage.setItem('secret','parent')");
    assert_eq!(eval(&opaque, "[localStorage.getItem('secret'),sessionStorage.getItem('secret')].join()"), ",");
    eval(&opaque, "localStorage.setItem('secret','opaque');sessionStorage.setItem('secret','opaque')");
    assert_eq!(
        eval(&e.main, "[localStorage.getItem('secret'),sessionStorage.getItem('secret')].join()"),
        "parent,parent"
    );
    assert_eq!(eval(&opaque, "document.domain"), "");
    assert_eq!(eval(&opaque, "document.URL"), "http://localhost:5173/app");
    assert_eq!(eval(&e.main, "document.domain"), "localhost");
}

#[test]
fn file_reader_array_buffer_task_order_and_reuse() {
    let e = env("<body></body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    assert_eq!(
        eval(
            &e.main,
            r#"
        var reader=new FileReader(), events=[];
        for (const type of ['loadstart','progress','load','error','abort','loadend'])
            reader.addEventListener(type,e=>events.push([e.type,reader.readyState,e.loaded,e.total,
                e.lengthComputable,e.isTrusted,e.target===reader,e instanceof ProgressEvent].join(':')));
        var initial=[reader.readyState,reader.result===null,reader.error===null,
                     reader.EMPTY,FileReader.LOADING,reader.DONE].join();
        reader.readAsArrayBuffer(new File([new Uint8Array([0,127,255])],'a.bin'));
        var repeated; try { reader.readAsText(new Blob(['wrong'])); } catch(e) { repeated=e.name; }
        [initial,reader.readyState,reader.result===null,events.length,repeated].join('|')
    "#
        ),
        "0,true,true,0,1,2|1|true|0|InvalidStateError"
    );
    tasks.drain();
    assert_eq!(eval(&e.main, "events.join('|')"),
        "loadstart:1:0:3:true:true:true:true|progress:1:3:3:true:true:true:true|load:2:3:3:true:true:true:true|loadend:2:3:3:true:true:true:true");
    assert_eq!(
        eval(
            &e.main,
            "[new Uint8Array(reader.result).join(),reader.result===reader.result,reader.error===null].join('|')"
        ),
        "0,127,255|true|true"
    );
    eval(&e.main, "events=[];reader.readAsText(new Blob(['again']));");
    tasks.drain();
    assert_eq!(eval(&e.main, "reader.result"), "again");
}

#[test]
fn file_reader_conversions_empty_bom_and_encoding() {
    let e = env("<body></body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    eval(
        &e.main,
        r#"
        var results=[], pending=[];
        function read(method,blob,label) { let r=new FileReader(); pending.push(r);
            r.onload=()=>results.push(r.result instanceof ArrayBuffer ? r.result.byteLength : r.result);
            r[method](blob,label); }
        read('readAsText',new Blob([new Uint8Array([0x80])],{type:'text/plain;charset=windows-1252'}));
        read('readAsText',new Blob([new Uint8Array([0xff,0xfe,0x61,0])]),'utf-8');
        read('readAsText',new Blob([new Uint8Array([0xc3,0xa9])]),'unknown-label');
        read('readAsDataURL',new Blob([new Uint8Array([0,255])],{type:'application/octet-stream'}));
        read('readAsDataURL',new Blob([])); read('readAsArrayBuffer',new Blob([]));
        read('readAsBinaryString',new Blob([new Uint8Array([0,255])]));
    "#,
    );
    tasks.drain();
    assert_eq!(
        eval(&e.main, "JSON.stringify(results)"),
        r#"["€","a","é","data:application/octet-stream;base64,AP8=","data:application/octet-stream;base64,",0,"\u0000ÿ"]"#
    );
}

#[test]
fn file_reader_abort_and_reentrant_read_suppress_stale_tasks() {
    let e = env("<body></body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    assert_eq!(
        eval(
            &e.main,
            r#"
        var r=new FileReader(), log=[];
        for(const t of ['loadstart','progress','abort','load','loadend']) r.addEventListener(t,()=>log.push(t));
        r.abort(); r.readAsText(new Blob(['old'])); r.abort();
        [r.readyState,r.result===null,r.error===null,log.join()].join('|')
    "#
        ),
        "2|true|true|abort,loadend"
    );
    tasks.drain();
    assert_eq!(eval(&e.main, "log.join()"), "abort,loadend");
    eval(
        &e.main,
        r#"
        log=[]; r.onloadstart=()=>{r.onloadstart=null;r.abort();r.readAsText(new Blob(['new']));};
        r.readAsText(new Blob(['discard']));
    "#,
    );
    tasks.drain();
    assert_eq!(eval(&e.main, "log.join()+'|'+r.result"), "loadstart,abort,loadend,loadstart,progress,load,loadend|new");
    eval(
        &e.main,
        r#"
        log=[]; r.onload=()=>{r.onload=null;r.readAsText(new Blob(['second']));};
        r.readAsText(new Blob(['first']));
    "#,
    );
    tasks.drain();
    assert_eq!(
        eval(&e.main, "log.join()+'|'+r.result"),
        "loadstart,progress,load,loadstart,progress,load,loadend|second"
    );
}

#[test]
fn file_reader_blob_and_result_cycles_are_traced() {
    let e = env("<body></body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    eval(
        &e.main,
        r#"
        var weakReader,weakBlob,weakResult,finished=0;
        (()=>{let r=new FileReader(),b=new Blob(['cycle']); b.owner=r;
            weakReader=new WeakRef(r); weakBlob=new WeakRef(b);
            r.onload=()=>{finished++;r.result.owner=r;weakResult=new WeakRef(r.result);};
            r.readAsArrayBuffer(b);})();
    "#,
    );
    for _ in 0..6 {
        e.engine.collect_garbage_now();
    }
    assert_eq!(eval(&e.main, "!!weakReader.deref() && !!weakBlob.deref()"), "true");
    tasks.drain();
    assert_eq!(eval(&e.main, "finished"), "1");
    for _ in 0..6 {
        e.engine.collect_garbage_now();
    }
    assert_eq!(
        eval(&e.main, "[weakReader.deref(),weakBlob.deref(),weakResult.deref()].every(x=>x===undefined)"),
        "true"
    );
}

#[test]
fn file_reader_limits_errors_and_abort_queue_are_bounded() {
    let e = env("<body></body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    assert!(e.page.set_file_reader_limits(FileReaderLimits {
        max_read_bytes: 3,
        max_pending_bytes: 3,
        max_pending_reads: 2,
    }));
    eval(
        &e.main,
        r#"
        var good=new FileReader(), bad=new FileReader(), errors=[];
        for(const t of ['loadstart','progress','load','error','loadend']) bad.addEventListener(t,e=>errors.push(e.type));
        good.readAsText(new Blob(['abc'])); bad.readAsText(new Blob(['d']));
        var cap;try {new FileReader().readAsText(new Blob([]));} catch(e) {cap=e.name;}
    "#,
    );
    assert_eq!(e.page.file_reader_usage(), (2, 3));
    assert_eq!(eval(&e.main, "cap"), "QuotaExceededError");
    assert!(!e.page.set_file_reader_limits(FileReaderLimits::default()));
    tasks.drain();
    assert_eq!(eval(&e.main, "[good.result,bad.readyState,bad.result===null,bad.error instanceof DOMException,bad.error.name,errors.join()].join('|')"), "abc|2|true|true|NotReadableError|loadstart,error,loadend");
    assert_eq!(e.page.file_reader_usage(), (0, 0));
    eval(&e.main, "for(let i=0;i<1000;i++){good.readAsText(new Blob(['abc']));good.abort();}");
    assert_eq!(tasks.0.borrow().len(), 1, "aborted reads must not enqueue unbounded weak tombstones");
    assert_eq!(e.page.file_reader_usage(), (0, 0));
    tasks.drain();
    eval(&e.main, "bad.readAsText(new Blob(['ok']));");
    tasks.drain();
    assert_eq!(eval(&e.main, "bad.result==='ok' && bad.error===null"), "true");
}

#[test]
fn file_reader_disposal_releases_pending_blob_and_skips_callbacks() {
    let e = env("<body></body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    eval(
        &e.main,
        r#"
        var calls=0,wr,wb;
        (()=>{let r=new FileReader(),b=new Blob(['payload']);b.reader=r;
            wr=new WeakRef(r);wb=new WeakRef(b);r.onloadend=()=>calls++;r.readAsText(b);})();
    "#,
    );
    assert_eq!(e.page.file_reader_usage(), (1, 7));
    lifecycle::realm_disposed(&e.page, e.main.id());
    assert_eq!(e.page.file_reader_usage(), (0, 0));
    tasks.drain();
    for _ in 0..6 {
        e.engine.collect_garbage_now();
    }
    assert_eq!(eval(&e.main, "[calls,wr.deref()===undefined,wb.deref()===undefined].join()"), "0,true,true");
}

#[test]
fn file_reader_document_disposal_skips_callbacks() {
    let e = env("<body></body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    eval(&e.main, "var calls=0,r=new FileReader();r.onloadend=()=>calls++;r.readAsText(new Blob(['x']));");
    e.page.dispose();
    tasks.drain();
    assert_eq!(e.page.file_reader_usage(), (0, 0));
    assert_eq!(eval(&e.main, "[calls,r.readyState,r.result===null].join()"), "0,2,true");
}

#[test]
fn file_reader_progress_abort_conversion_reentry_and_missing_queue() {
    let e = env("<body></body>");
    assert_eq!(
        eval(&e.main, "(()=>{try{new FileReader().readAsText(new Blob([]))}catch(e){return e.name}})()"),
        "NotSupportedError"
    );
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    eval(
        &e.main,
        r#"
        var r=new FileReader(),log=[];
        for(const t of ['progress','abort','load','loadend']) r.addEventListener(t,()=>log.push(t));
        r.onprogress=()=>r.abort(); r.readAsText(new Blob(['x']));
    "#,
    );
    tasks.drain();
    assert_eq!(eval(&e.main, "log.join()+'|'+(r.result===null)"), "progress,abort,loadend|true");
    assert_eq!(
        eval(
            &e.main,
            r#"
        r.onprogress=null;var conversion;
        try{r.readAsText(new Blob(['outer']),{toString(){r.readAsText(new Blob(['inner']));return 'utf-8'}})}catch(e){conversion=e.name}
        conversion
    "#
        ),
        "InvalidStateError"
    );
    tasks.drain();
    assert_eq!(eval(&e.main, "r.result"), "inner");
}

#[test]
fn file_reader_abort_releases_traced_blob_and_isolated_world_works() {
    let e = env("<body></body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    eval(
        &e.main,
        r#"
        var weakAbortReader,weakAbortBlob;
        (()=>{let r=new FileReader(),b=new Blob(['abort']);b.reader=r;
            weakAbortReader=new WeakRef(r);weakAbortBlob=new WeakRef(b);r.readAsText(b);r.abort();})();
    "#,
    );
    for _ in 0..6 {
        e.engine.collect_garbage_now();
    }
    assert_eq!(eval(&e.main, "weakAbortReader.deref()===undefined && weakAbortBlob.deref()===undefined"), "true");
    let isolated = new_realm(&e.engine, e.engine.register::<TestWindow>(), &e.page, "file-reader-world");
    eval(&isolated, "var done=false,r=new FileReader();r.onload=e=>done=e.target===r && r.result instanceof ArrayBuffer;r.readAsArrayBuffer(new Blob(['world']));");
    tasks.drain();
    assert_eq!(eval(&isolated, "done && new Uint8Array(r.result).length===5"), "true");
    assert_eq!(e.page.file_reader_usage(), (0, 0));
}

#[test]
fn focus_transition_reentrant_blur_and_focus_do_not_overwrite_new_intent() {
    let e = env("<body><input id=a><input id=b><input id=c></body>");
    assert_eq!(
        eval(
            &e.main,
            r#"
        globalThis.events=[]; const [a,b,c]=['a','b','c'].map(id=>document.getElementById(id));
        for(const n of [a,b,c]) for(const ty of ['focus','focusin','blur','focusout'])
            n.addEventListener(ty,()=>events.push(n.id+':'+ty));
        a.focus();events=[];a.onblur=()=>c.focus();b.focus();
        document.activeElement.id+'|'+events.join(',')
    "#
        ),
        "c|a:blur,c:focus,c:focusin"
    );
    assert_eq!(
        eval(
            &e.main,
            r#"
        a.onblur=null;c.blur();events=[];a.onfocus=()=>b.focus();a.focus();
        document.activeElement.id+'|'+events.join(',')
    "#
        ),
        "b|a:focus,a:blur,a:focusout,b:focus,b:focusin"
    );
}

#[test]
fn focus_transition_revalidates_detachment_and_document_replacement() {
    let e = env("<body><input id=a><input id=b></body>");
    assert_eq!(
        eval(
            &e.main,
            r#"
        const a=document.getElementById('a'),b=document.getElementById('b');
        a.focus();a.onblur=()=>b.remove();b.focus();document.activeElement.tagName
    "#
        ),
        "BODY"
    );
    struct ReplaceOnFocus {
        page: std::rc::Weak<PageDom>,
        replacement: lp_dom::NodeId,
    }
    impl PageHooks for ReplaceOnFocus {
        fn post_task(&self, _: Box<dyn FnOnce()>) {}
        fn navigate(&self, _: NavigationRequest) {}
        fn request_focus(&self, _: FocusTarget, token: &FocusTransition) -> bool {
            let page = self.page.upgrade().unwrap();
            assert!(page.dom.try_borrow_mut().is_ok());
            page.set_document(self.replacement);
            assert!(!token.is_current());
            true
        }
    }
    let replacement = parse_document(&e.page.dom, "http://localhost/new", "<body>new</body>");
    e.page.set_hooks(Some(Rc::new(ReplaceOnFocus { page: Rc::downgrade(&e.page), replacement })));
    eval(&e.main, "a.focus()");
    assert_eq!(e.page.document(), replacement);
    assert_eq!(e.page.dom.borrow().focused(replacement).unwrap(), None);
}

#[test]
fn focus_owner_helpers_share_tokens_without_recursive_hooks_and_cleanup_selection() {
    struct Hooks {
        page: std::rc::Weak<PageDom>,
        calls: Rc<RefCell<Vec<FocusTarget>>>,
    }
    impl PageHooks for Hooks {
        fn post_task(&self, _: Box<dyn FnOnce()>) {}
        fn navigate(&self, _: NavigationRequest) {}
        fn request_focus(&self, target: FocusTarget, token: &FocusTransition) -> bool {
            self.calls.borrow_mut().push(target);
            let page = self.page.upgrade().unwrap();
            assert!(page.dom.try_borrow_mut().is_ok());
            // This also proves the PageHooks RefCell borrow was released.
            page.set_hooks(None);
            match target {
                FocusTarget::Element(node) => input::focus_element_with_transition(&page, node, token),
                FocusTarget::Window => input::focus_window_with_transition(&page, token),
                FocusTarget::Blur(_) => input::blur_document_with_transition(&page, token),
            }
            assert!(token.is_current());
            false
        }
    }
    let e = env("<body><input id=a value=abcd></body>");
    let calls = Rc::new(RefCell::new(Vec::new()));
    let hooks = Rc::new(Hooks { page: Rc::downgrade(&e.page), calls: calls.clone() });
    e.page.set_hooks(Some(hooks.clone()));
    assert_eq!(
        eval(
            &e.main,
            "const a=document.getElementById('a');a.focus();a.setSelectionRange(1,3);document.activeElement.id"
        ),
        "a"
    );
    e.page.set_hooks(Some(hooks.clone()));
    eval(&e.main, "a.focus()"); // Same-local-target must still reach owner.
    assert_eq!(calls.borrow().len(), 2);
    e.page.set_hooks(Some(hooks.clone()));
    assert_eq!(eval(&e.main, "window.focus();document.activeElement.tagName"), "BODY");
    input::insert_text(&e.page, "X");
    assert_eq!(eval(&e.main, "a.value"), "aXd");
    let token = FocusTransition::new();
    input::focus_window_with_transition(&e.page, &token);
    input::focus_window_with_transition(&e.page, &token.clone());
    assert!(token.is_current());
    input::clear_document_focus(&e.page);
    assert!(!token.is_current());
    assert!(input::remembered_editing_target(&e.page).is_none());
    input::insert_text(&e.page, "Y");
    assert_eq!(eval(&e.main, "a.value"), "aXd");
}

#[test]
fn keydown_default_action_retargets_within_document_and_stops_after_owner_switch() {
    use std::cell::Cell;
    struct Hooks(Rc<Cell<bool>>);
    impl PageHooks for Hooks {
        fn post_task(&self, _: Box<dyn FnOnce()>) {}
        fn navigate(&self, _: NavigationRequest) {}
        fn is_focused(&self) -> bool {
            self.0.get()
        }
        fn request_focus(&self, target: FocusTarget, _: &FocusTransition) -> bool {
            if target == FocusTarget::Window {
                self.0.set(false);
                return false;
            }
            true
        }
    }
    let e = env("<body><input id=a><input id=b></body>");
    eval(
        &e.main,
        "const a=document.getElementById('a'),b=document.getElementById('b');a.focus();a.onkeydown=()=>b.focus()",
    );
    let key = input::KeyInput { key: "x".into(), text: "x".into(), ..Default::default() };
    input::dispatch_key(&e.page, input::KeyKind::KeyDown, &key);
    assert_eq!(eval(&e.main, "a.value+'|'+b.value"), "|x");
    let selected = Rc::new(Cell::new(true));
    e.page.set_hooks(Some(Rc::new(Hooks(selected.clone()))));
    eval(&e.main, "b.onkeydown=()=>window.focus()");
    input::dispatch_key(&e.page, input::KeyKind::KeyDown, &key);
    assert_eq!(eval(&e.main, "a.value+'|'+b.value"), "|x");
    input::dispatch_key(&e.page, input::KeyKind::Char, &key);
    input::insert_text(&e.page, "z");
    assert_eq!(eval(&e.main, "b.value"), "x");
    selected.set(true);
    eval(&e.main, "b.onkeydown=null;b.onkeypress=()=>a.focus()");
    input::dispatch_key(&e.page, input::KeyKind::KeyDown, &key);
    assert_eq!(eval(&e.main, "a.value+'|'+b.value"), "x|x");
}

#[test]
fn focus_transition_change_reentry_and_disposal_stop_remaining_events() {
    let e = env("<body><input id=a><input id=b><input id=c></body>");
    assert_eq!(
        eval(
            &e.main,
            r#"
        const a=document.getElementById('a'),b=document.getElementById('b'),c=document.getElementById('c');
        globalThis.events=[];a.focus();a.value='changed';
        a.onchange=()=>{events.push('change');c.focus()};a.onblur=()=>events.push('stale blur');
        b.onfocus=()=>events.push('stale focus');b.focus();document.activeElement.id+'|'+events.join()
    "#
        ),
        "c|change"
    );
    let token = FocusTransition::new();
    let page = e.page.clone();
    let scope = e.main.scope().unwrap();
    let dispose = scope
        .function("dispose", move |s, _, _| {
            page.dispose();
            Ok(s.undefined())
        })
        .unwrap();
    scope.global().set("disposePage", dispose.value()).unwrap();
    eval(&e.main, "a.onchange=null;a.onblur=null;a.onfocus=()=>disposePage();a.onfocusin=()=>events.push('stale focusin');events=[]");
    let a = e.page.dom.borrow().get_element_by_id(e.page.document(), "a").unwrap().unwrap();
    input::focus_element_with_transition(&e.page, a, &token);
    assert!(!token.is_current());
    assert!(e.page.is_disposed());
    assert_eq!(eval(&e.main, "events.join()"), "");
}

#[test]
fn retired_focus_document_cannot_cancel_live_shared_transition() {
    use std::cell::Cell;
    struct Live(Rc<Cell<bool>>);
    impl PageHooks for Live {
        fn post_task(&self, _: Box<dyn FnOnce()>) {}
        fn navigate(&self, _: NavigationRequest) {}
        fn focus_document_is_live(&self) -> bool {
            self.0.get()
        }
        fn request_focus(&self, _: FocusTarget, _: &FocusTransition) -> bool {
            panic!("retired owner hook")
        }
    }
    let retired = env("<body><input id=a></body>");
    let dom = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom, "http://localhost/child", "<body><input id=b></body>");
    dom.borrow_mut().set_document_active(doc, true).unwrap();
    let live_page = PageDom::new(dom, doc);
    let live_realm = new_realm(&retired.engine, retired.engine.register::<TestWindow>(), &live_page, "child");
    let token = FocusTransition::new();
    input::focus_window_with_transition(&retired.page, &token);
    input::focus_window_with_transition(&live_page, &token);
    retired.page.set_hooks(Some(Rc::new(Live(Rc::new(Cell::new(false))))));
    eval(&retired.main, "document.getElementById('a').focus();window.focus()");
    assert!(token.is_current(), "stale requests must not cancel the owner's current transition");
    let node = live_page.dom.borrow().get_element_by_id(live_page.document(), "b").unwrap().unwrap();
    input::focus_element_with_transition(&live_page, node, &token);
    assert_eq!(eval(&live_realm, "document.activeElement.id"), "b");
    let invalid = FocusTransition::new();
    input::focus_window_with_transition(&retired.page, &invalid);
    assert!(token.is_current(), "stale helper must not adopt/cancel local state either");
}

#[test]
fn reflected_urls_follow_attributes_bases_adoption_and_document_replacement() {
    let e = env("<head><link id=r href='../asset.js'></head><body></body>");
    let doc = e.page.document();
    assert_eq!(eval(&e.main, "globalThis.r=document.getElementById('r');r.href"), "http://localhost:5173/asset.js");
    assert_eq!(eval(&e.main, "r.setAttribute('href',' second.js ');r.href"), "http://localhost:5173/second.js");
    assert_eq!(eval(&e.main, "r.removeAttribute('href');r.href"), "");
    eval(&e.main, "r.href='relative.js'");
    {
        let mut dom = e.page.dom.borrow_mut();
        dom.set_document_url(doc, "about:srcdoc").unwrap();
        dom.set_document_base_url(doc, Some("https://parent.example/dir/page")).unwrap();
    }
    assert_eq!(eval(&e.main, "r.href"), "https://parent.example/dir/relative.js");
    let isolated = new_realm(&e.engine, e.engine.register::<TestWindow>(), &e.page, "isolated");
    assert_eq!(eval(&isolated, "document.getElementById('r').href"), "https://parent.example/dir/relative.js");
    {
        let mut dom = e.page.dom.borrow_mut();
        dom.set_document_url(doc, "about:blank").unwrap();
        dom.set_document_base_url(doc, Some("https://changed.example/new/")).unwrap();
    }
    assert_eq!(eval(&isolated, "document.getElementById('r').href"), "https://changed.example/new/relative.js");
    let node = e.page.dom.borrow().get_element_by_id(doc, "r").unwrap().unwrap();
    let other = parse_document(
        &e.page.dom,
        "https://adopted.example/other/page",
        "<head><link id=new href='new.js'></head><body>",
    );
    e.page.dom.borrow_mut().adopt_node(other, node).unwrap();
    assert_eq!(eval(&e.main, "r.href"), "https://adopted.example/other/relative.js");
    e.page.dom.borrow_mut().set_document_url(other, "https://moved.example/").unwrap();
    assert_eq!(eval(&e.main, "r.href"), "https://moved.example/relative.js");
    e.page.set_document(other);
    assert_eq!(
        eval(&e.main, "[r.href,document.getElementById('new').href].join('|')"),
        "https://moved.example/relative.js|https://moved.example/new.js"
    );
    e.page.dom.borrow_mut().adopt_node(doc, node).unwrap();
    e.page.dom.borrow_mut().set_document_base_url(doc, None).unwrap();
    assert_eq!(eval(&e.main, "r.href"), "relative.js", "opaque base preserves existing fallback");
    e.page.dom.borrow_mut().set_document_url(doc, "invalid base").unwrap();
    assert_eq!(
        eval(&e.main, "r.setAttribute('href',' https://absolute.example/ ');r.href"),
        " https://absolute.example/ "
    );
    e.page.dom.borrow_mut().set_document_url(doc, "https://restored.example/base/").unwrap();
    assert_eq!(eval(&e.main, "r.href='final.js';r.href"), "https://restored.example/base/final.js");
    e.page.dispose();
    assert_eq!(eval(&e.main, "r.href"), "https://restored.example/base/final.js");
}

#[test]
fn reflected_urls_preserve_oversized_and_canonicalized_base_semantics() {
    let e = env("<head><link id=r href='asset.js'></head>");
    eval(&e.main, "globalThis.r=document.getElementById('r');r.href");
    for base in
        [format!("https://example.test/{}/", "a".repeat(5000)), format!("https://example.test/{}/", "é".repeat(1000))]
    {
        e.page.dom.borrow_mut().set_document_base_url(e.page.document(), Some(&base)).unwrap();
        let expected = lp_net::Url::parse(&base).unwrap().join("asset.js").unwrap().to_string();
        assert_eq!(eval(&e.main, "r.href"), expected);
    }
    e.page.dom.borrow_mut().set_document_base_url(e.page.document(), None).unwrap();
    assert_eq!(eval(&e.main, "r.href"), "http://localhost:5173/asset.js");
}

#[test]
fn focus_scroll_default_precedes_focus_events_and_prevent_scroll_is_observed() {
    let e = env(&format!(
        "<body>{}<button id=b>target</button><div style='height:900px'>end</div></body>",
        "<div>filler</div>".repeat(200)
    ));
    assert_eq!(
        eval(
            &e.main,
            r#"
        const b=document.getElementById('b');globalThis.seen=[];
        b.onfocus=()=>seen.push(b.getBoundingClientRect().top>=0 && b.getBoundingClientRect().bottom<=innerHeight);
        b.addEventListener('focusin',()=>seen.push(scrollY>0));
        b.focus();[scrollY>0,...seen,document.activeElement===b].join(',')
    "#
        ),
        "true,true,true,true"
    );
    assert_eq!(
        eval(
            &e.main,
            r#"
        b.blur();scrollTo(0,0);let reads=0;b.focus({get preventScroll(){reads++;return true}});
        [scrollY,reads,document.activeElement===b].join(',')
    "#
        ),
        "0,1,true"
    );
    assert_eq!(
        eval(
            &e.main,
            r#"
        b.blur();b.focus({preventScroll:false});const y=scrollY;b.focus();
        [y>0,scrollY===y].join(',')
    "#
        ),
        "true,true"
    );
}

#[test]
fn focus_scroll_option_conversion_and_reentrant_target_are_not_stale() {
    let e = env(&format!(
        "<body><button id=a>first</button>{}<button id=b>last</button><div style='height:900px'>end</div></body>",
        "<div>filler</div>".repeat(200)
    ));
    assert_eq!(
        eval(
            &e.main,
            r#"
        const a=document.getElementById('a'),b=document.getElementById('b');let calls=0;
        try{b.focus({get preventScroll(){throw new Error('option')}})}catch(e){calls++}
        [calls,scrollY,document.activeElement===b].join(',')
    "#
        ),
        "1,0,false"
    );
    assert_eq!(
        eval(
            &e.main,
            r#"
        a.focus();a.onblur=()=>b.remove();b.focus();
        [scrollY,document.activeElement.tagName].join(',')
    "#
        ),
        "0,BODY"
    );
}

#[test]
fn focus_scroll_owner_helpers_preserve_options_and_drop_replaced_document_tasks() {
    struct Hooks {
        page: std::rc::Weak<PageDom>,
        tasks: RefCell<Vec<Box<dyn FnOnce()>>>,
    }
    impl PageHooks for Hooks {
        fn navigate(&self, _: NavigationRequest) {}
        fn post_task(&self, task: Box<dyn FnOnce()>) {
            self.tasks.borrow_mut().push(task);
        }
        fn request_focus(&self, target: FocusTarget, transition: &FocusTransition) -> bool {
            let page = self.page.upgrade().unwrap();
            if let FocusTarget::Element(el) = target {
                input::focus_element_with_transition(&page, el, transition);
                false
            } else {
                true
            }
        }
    }
    let e = env(&format!(
        "<body>{}<button id=b>target</button><div style='height:900px'>end</div></body>",
        "<div>filler</div>".repeat(200)
    ));
    let hooks = Rc::new(Hooks { page: Rc::downgrade(&e.page), tasks: RefCell::new(Vec::new()) });
    e.page.set_hooks(Some(hooks.clone()));
    assert_eq!(eval(&e.main, "globalThis.scrolls=0;window.addEventListener('scroll',()=>scrolls++);const b=document.getElementById('b');b.focus({preventScroll:true});scrollY"), "0");
    assert!(hooks.tasks.borrow().is_empty());
    assert_eq!(eval(&e.main, "b.blur();b.focus();scrollY>0"), "true");
    assert_eq!(hooks.tasks.borrow().len(), 1);
    let replacement = parse_document(&e.page.dom, "http://localhost/new", "<body>new</body>");
    e.page.set_document(replacement);
    for task in hooks.tasks.take() {
        task();
    }
    assert_eq!(eval(&e.main, "scrolls"), "0");
}

#[test]
fn programmatic_scrolls_queue_scroll_then_scrollend_like_chromium() {
    // Gap 35: a column-virtualized table renders new columns from its
    // scroller's `scroll` event, so every programmatic scroll (scrollIntoView,
    // scrollTo/scrollBy, the scrollLeft/scrollTop setters) must queue one,
    // and Chromium follows it with `scrollend` once the instant scroll is
    // done. Neither fires when nothing moved.
    let e = env(
        "<body style='margin:0'><div id=s style='overflow:auto;width:200px;height:60px'><div style='display:flex;width:2000px'>\
         <div style='flex:none;width:100px;height:40px'>a</div><div style='flex:none;width:1500px'></div><div id=far style='flex:none;width:100px;height:40px'>z</div>\
         </div><div style='height:200px'></div></div><div style='height:2000px'></div></body>",
    );
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    eval(
        &e.main,
        "window.seen = []; window.s = document.getElementById('s');
         for (const type of ['scroll', 'scrollend']) {
           s.addEventListener(type, ev => seen.push(type + ':' + s.scrollLeft + ',' + s.scrollTop));
           document.addEventListener(type, ev => { if (ev.target === document) seen.push('document ' + type) });
         }",
    );
    let run = |src: &str| {
        let r = eval(&e.main, &format!("seen.length = 0; {src}; String(seen.length)"));
        assert_eq!(r, "0", "{src}: events are queued, not dispatched inline");
        tasks.drain();
        eval(&e.main, "seen.join(' ')")
    };
    assert_eq!(run("document.getElementById('far').scrollIntoView()"), "scroll:1500,0 scrollend:1500,0");
    assert_eq!(run("s.scrollLeft = 10"), "scroll:10,0 scrollend:10,0");
    assert_eq!(run("s.scrollTop = 5"), "scroll:10,5 scrollend:10,5");
    assert_eq!(run("s.scrollTo(20, 0)"), "scroll:20,0 scrollend:20,0");
    assert_eq!(run("s.scrollBy({left: 5})"), "scroll:25,0 scrollend:25,0");
    assert_eq!(run("s.scrollLeft = 25"), "", "no move, no events");
    assert_eq!(run("window.scrollTo(0, 100)"), "document scroll document scrollend");
    e.page.dispose();
}

#[test]
fn scroll_into_view_delivers_each_changed_ancestor_after_the_call() {
    struct Hooks(RefCell<Vec<Box<dyn FnOnce()>>>);
    impl PageHooks for Hooks {
        fn navigate(&self, _: NavigationRequest) {}
        fn post_task(&self, task: Box<dyn FnOnce()>) {
            self.0.borrow_mut().push(task);
        }
    }
    for primed in [false, true] {
        let e = env(&format!(
            "<body>{}<div id=outer style='overflow:auto;width:180px;height:80px'>{}<div id=inner style='overflow:auto;width:120px;height:60px'>{}<button id=target style='height:20px;width:20px'>target</button><div style='height:100px'>tail</div></div><div style='height:100px'>tail</div></div><div style='height:900px'>end</div></body>",
            "<div style='height:20px'>before</div>".repeat(100),
            "<div style='height:20px'>outer</div>".repeat(20),
            "<div style='height:20px'>inner</div>".repeat(20),
        ));
        if primed {
            let dom = e.page.dom.borrow();
            for id in ["inner", "outer"] {
                let node = dom.get_element_by_id(e.page.document(), id).unwrap().unwrap();
                assert!(e.page.layout.borrow_mut().set_scroll_top(&dom, node, 1));
            }
        }
        let hooks = Rc::new(Hooks(RefCell::new(Vec::new())));
        e.page.set_hooks(Some(hooks.clone()));
        assert_eq!(
            eval(
                &e.main,
                r#"
            globalThis.seen=[];
            const inner=document.getElementById('inner'),outer=document.getElementById('outer'),target=document.getElementById('target');
            for(const [name,node] of [['inner',inner],['outer',outer],['document',document]]) {
                node.addEventListener('scroll',event=>{if(event.target===node)seen.push(name)});
            }
            target.scrollIntoView();
            [inner.scrollTop>0,outer.scrollTop>0,scrollY>0,seen.length===0].join(',')
        "#
            ),
            "true,true,true,true"
        );
        assert_eq!(hooks.0.borrow().len(), 3);
        for task in hooks.0.take() {
            task();
        }
        assert_eq!(eval(&e.main, "seen.join(',')"), "inner,outer,document");
        let focus_scroll = eval(&e.main, "const snapshot=()=>[inner.scrollTop,outer.scrollTop,inner.scrollLeft,outer.scrollLeft,scrollX,scrollY];const before=snapshot();target.scrollIntoView();target.focus();JSON.stringify([before,snapshot()])");
        assert_eq!(eval(&e.main, "seen.length"), "3");
        assert!(hooks.0.borrow().is_empty(), "unchanged scroll and visible focus queue no events: {focus_scroll}");

        // Each container event has the same document-generation guard as the
        // viewport event; replacing the document cancels the entire old batch.
        assert_eq!(eval(&e.main, "inner.scrollTop=1;outer.scrollTop=1;scrollTo(0,0);'reset'"), "reset");
        for task in hooks.0.take() {
            task();
        }
        assert_eq!(eval(&e.main, "seen.length=0;target.scrollIntoView();seen.length"), "0");
        assert_eq!(hooks.0.borrow().len(), 3);
        let replacement = parse_document(&e.page.dom, "http://localhost/replaced", "<body>replacement</body>");
        e.page.set_document(replacement);
        for task in hooks.0.take() {
            task();
        }
        assert_eq!(eval(&e.main, "seen.length"), "0");
    }
}

#[test]
fn pointer_focus_keeps_partly_clipped_button_at_the_press_point() {
    use lp_webapi::input::{dispatch_mouse, MouseButton, MouseInput, MouseKind};
    for align_label in [true, false] {
        let rows = |n| "<div style='height:10px'>row</div>".repeat(n);
        let e = env(&format!(
            "<body>{}<div id=outer style='height:120px;width:240px;overflow:auto'>{}<button id=button style='display:block;width:160px;padding:0;border:0'>{}<span id=label style='display:block;height:10px'>Click me</span></button>{}</div>{}</body>",
            rows(150), rows(30), "<span style='display:block;height:10px'>row</span>".repeat(8), rows(80), rows(150)
        ));
        eval(&e.main, "var button=document.getElementById('button'),label=document.getElementById('label'),outer=document.getElementById('outer');var clicks=0,focuses=0;button.onclick=()=>clicks++;button.onfocus=()=>focuses++;");
        eval(&e.main, if align_label { "label.scrollIntoView()" } else { "button.scrollIntoView()" });
        assert_eq!(eval(&e.main, "document.elementFromPoint(label.getBoundingClientRect().x+label.getBoundingClientRect().width/2,label.getBoundingClientRect().y+label.getBoundingClientRect().height/2)===label"), "true");
        if align_label {
            assert_eq!(eval(&e.main, "button.getBoundingClientRect().top < outer.getBoundingClientRect().top"), "true");
        }
        eval(&e.main, "var offsets=JSON.stringify([outer.scrollTop,outer.scrollLeft,scrollX,scrollY]);");
        let mouse = MouseInput {
            x: eval(&e.main, "label.getBoundingClientRect().x+label.getBoundingClientRect().width/2").parse().unwrap(),
            y: eval(&e.main, "label.getBoundingClientRect().y+label.getBoundingClientRect().height/2").parse().unwrap(),
            button: MouseButton::Left,
            buttons: 1,
            click_count: 1,
            ..Default::default()
        };
        dispatch_mouse(&e.page, MouseKind::Pressed, &mouse).unwrap();
        assert_eq!(eval(&e.main, "[document.activeElement===button,focuses,offsets===JSON.stringify([outer.scrollTop,outer.scrollLeft,scrollX,scrollY])].join(',')"), "true,1,true");
        dispatch_mouse(&e.page, MouseKind::Released, &MouseInput { buttons: 0, ..mouse }).unwrap();
        assert_eq!(eval(&e.main, "clicks"), "1");
        if align_label {
            assert_eq!(eval(&e.main, "button.blur();button.focus();offsets!==JSON.stringify([outer.scrollTop,outer.scrollLeft,scrollX,scrollY])"), "true", "programmatic focus still reveals the clipped control");
        }
    }
}

#[test]
fn pointer_focus_preserves_cancellation_and_reentrant_script_reveal() {
    use lp_webapi::input::{dispatch_mouse, MouseButton, MouseInput, MouseKind};
    for cancel in [true, false] {
        let e = env(&format!(
            "<body><button id=button>press</button>{}<button id=other>other</button>{}</body>",
            "<div style='height:20px'>row</div>".repeat(150),
            "<div style='height:20px'>tail</div>".repeat(150),
        ));
        eval(&e.main, "var button=document.getElementById('button'),other=document.getElementById('other');var events=[];button.onfocus=()=>{events.push('button');other.focus()};other.onfocus=()=>events.push('other');");
        if cancel {
            eval(&e.main, "button.onmousedown=e=>e.preventDefault()");
        }
        let mouse = MouseInput {
            x: eval(&e.main, "button.getBoundingClientRect().x+button.getBoundingClientRect().width/2")
                .parse()
                .unwrap(),
            y: eval(&e.main, "button.getBoundingClientRect().y+button.getBoundingClientRect().height/2")
                .parse()
                .unwrap(),
            button: MouseButton::Left,
            buttons: 1,
            ..Default::default()
        };
        dispatch_mouse(&e.page, MouseKind::Pressed, &mouse).unwrap();
        if cancel {
            assert_eq!(
                eval(&e.main, "[document.activeElement===document.body,events.length,scrollY].join(',')"),
                "true,0,0"
            );
        } else {
            assert_eq!(
                eval(&e.main, "[document.activeElement===other,events.join('/'),scrollY>0].join(',')"),
                "true,button/other,true",
                "author focus uses its own transition and reveal policy"
            );
        }
    }
}

#[test]
fn style_assignment_forwards_to_css_text() {
    // [PutForwards=cssText]: Chromium 1.59.1 gives
    // ["width: 10px; color: red;", true, "10px", "\"\"", "fill: red;"].
    let e = env("<!doctype html><body><div id=d style='color: blue'></div><svg id=s></svg></body>");
    let r = &e.main;
    let got = eval(
        r,
        "(() => { 'use strict'; const d = document.getElementById('d'); const before = d.style; \
         d.style = 'width: 10px; color: red'; \
         const r1 = d.getAttribute('style'); const same = d.style === before; const w = d.style.width; \
         d.style = null; const r2 = JSON.stringify(d.getAttribute('style')); \
         const s = document.getElementById('s'); s.style = 'fill: red'; const r3 = s.getAttribute('style'); \
         return JSON.stringify([r1, same, w, r2, r3]); })()",
    );
    assert_eq!(got, r#"["width: 10px; color: red;",true,"10px","\"\"","fill: red;"]"#);
}

#[test]
fn range_over_text_reports_the_text_box() {
    // Playwright's accessible-name check measures a label's text with
    // Range.selectNode(text).getBoundingClientRect(); TOS buttons put the
    // text in a `display: contents` span, which has no box. Chromium reports
    // the text box (8, 6, 74.6875 x 16 here, measured); the role query then
    // finds the button by name.
    let e = env("<!doctype html><body style=\"margin:0;font-family:Arial,sans-serif;font-size:14px;line-height:20px\"><button id=btn style=\"display:inline-flex;padding:4px 8px;border:0;font:inherit\"><span id=c style=\"display:contents\">Files &amp; links</span></button></body>");
    let r = &e.main;
    let got = eval(
        r,
        "(() => { const t = document.getElementById('c').firstChild; const range = document.createRange(); range.selectNode(t); \
         const a = range.getBoundingClientRect(); const inner = document.createRange(); inner.selectNodeContents(t); \
         const b = inner.getBoundingClientRect(); const span = document.getElementById('c').getBoundingClientRect(); \
         return [a.x, a.y, a.height, a.width, b.x, b.height, span.width, span.height].join(','); })()",
    );
    assert_eq!(got, "8,6,16,74.6875,8,16,0,0");
}

#[test]
fn scroll_offsets_round_like_chromium_so_synced_panes_settle() {
    // TOS useScrollSync mirrors two panes by ratio from their scroll events
    // (target.scrollTop = ratio * range). Chromium rounds the written offset
    // to the nearest pixel, so main 1237 -> sidebar 686 -> main 1237 settles
    // after 2 events. Truncation (ToInt32) moved each pane 1-2px per round
    // and ping-ponged ~1200 scroll tasks down to 0. Values measured in
    // Chromium (Playwright 1.59.1).
    let e = env("<!doctype html><body style=\"margin:0\"><div id=m style=\"height:500px;overflow:auto\"><div style=\"height:5003px\"></div></div><div id=s style=\"height:500px;overflow:auto\"><div style=\"height:2997px\"></div></div><div style=\"height:9000px\"></div></body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    let got = eval(
        &e.main,
        "(() => { const m = document.getElementById('m'); const out = [];
           for (const v of [10.5, 10.49, 1e10, -3, 4294967396, NaN]) { m.scrollTop = 0; m.scrollTop = v; out.push(m.scrollTop); }
           m.scrollTo({ top: 20.5 }); out.push(m.scrollTop);
           m.scrollTo(0, 30.4); out.push(m.scrollTop);
           m.scrollBy(0, 0.5); out.push(m.scrollTop);
           window.scrollTo(0, 40.5); out.push(scrollY);
           m.scrollTop = 0; return out.join(); })()",
    );
    assert_eq!(got, "11,10,4503,0,4503,0,21,30,31,41");
    tasks.drain();
    eval(
        &e.main,
        "globalThis.log = [];
         const m = document.getElementById('m'), s = document.getElementById('s');
         const sync = (src, t) => { t.scrollTop = src.scrollTop / (src.scrollHeight - src.clientHeight) * (t.scrollHeight - t.clientHeight); log.push(src.id + src.scrollTop + '>' + t.scrollTop); };
         m.addEventListener('scroll', () => sync(m, s));
         s.addEventListener('scroll', () => sync(s, m));
         m.scrollTop = 1237; 1",
    );
    tasks.drain();
    assert_eq!(eval(&e.main, "log.join(' ')"), "m1237>686 s686>1237");
}

#[test]
fn single_axis_overflow_makes_a_scroll_container_like_chromium() {
    // TOS bookkeeping exports (bookkeeping-vat-lines.e2e.ts:65): the table's
    // scroller is `flex h-full grow flex-col overflow-x-auto`. CSS Overflow 3
    // computes the other axis's `visible` to `auto` (and `clip` to `hidden`),
    // so it scrolls vertically; Rust reported overflowY "" and refused
    // scrollTop = 1000, so the virtualized rows never moved. Chromium
    // (Playwright 1.59.1), 100px boxes with 300px content, scrollTop = 120:
    // computed x/y/shorthand, scrollTop, scroll events.
    let cases = [
        ("xauto", "overflow-x:auto", "auto|auto|auto|120|1"),
        ("yauto", "overflow-y:auto", "auto|auto|auto|120|1"),
        ("xhidden", "overflow-x:hidden", "hidden|auto|hidden auto|120|1"),
        ("xclip", "overflow-x:clip", "clip|visible|clip visible|0|0"),
        ("xclipyscroll", "overflow-x:clip;overflow-y:scroll", "hidden|scroll|hidden scroll|120|1"),
        ("none", "", "visible|visible|visible|0|0"),
        ("both", "overflow:hidden auto", "hidden|auto|hidden auto|120|1"),
    ];
    let html: String = cases
        .iter()
        .map(|(k, css, _)| {
            format!(
                "<div id={k} style='height:100px;width:100px;{css}'><div style='height:300px;width:50px'></div></div>"
            )
        })
        .collect();
    let e = env(&format!("<body style='margin:0'>{html}</body>"));
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    let ids: Vec<String> = cases.iter().map(|(k, _, _)| format!("'{k}'")).collect();
    eval(
        &e.main,
        &format!(
            "globalThis.scrolls={{}};for(const k of [{}]){{const el=document.getElementById(k);scrolls[k]=0;\
             el.addEventListener('scroll',()=>scrolls[k]++);el.scrollTop=120}};'ok'",
            ids.join(",")
        ),
    );
    e.page.after_task();
    tasks.drain();
    for (k, _, want) in cases {
        let got = eval(
            &e.main,
            &format!(
                "(()=>{{const el=document.getElementById('{k}'),cs=getComputedStyle(el);\
                 return [cs.overflowX,cs.overflowY,cs.overflow,el.scrollTop,scrolls['{k}']].join('|')}})()"
            ),
        );
        assert_eq!(got, want, "{k}");
    }
    e.page.dispose();
}

#[test]
fn content_shrink_clamps_scroll_offsets_and_fires_scroll_like_chromium() {
    // Chromium clamps a scroll position when layout shrinks the scrollable
    // overflow below it and fires `scroll`; the engine kept the stale offset
    // silently. react-aria's virtualizer takes its window from that event
    // when measured rows come out shorter than estimated (TOS QRef: a stale
    // window and an extra measuring round). Chromium: 900, then 400 at the
    // read with an event, then 200 at the next frame without a read.
    let e = env("<body style='margin:0'><div id=s style='height:100px;overflow:auto'><div id=c style='height:1000px'></div></div></body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    eval(
        &e.main,
        "globalThis.s = document.getElementById('s'); globalThis.c = document.getElementById('c'); globalThis.ev = []; \
         s.addEventListener('scroll', () => ev.push(s.scrollTop)); s.scrollTop = 900; 1",
    );
    tasks.drain();
    assert_eq!(eval(&e.main, "ev.join()"), "900");
    // Shrunk, then read: clamped at once; its event comes at the next frame.
    assert_eq!(eval(&e.main, "ev = []; c.style.height = '500px'; s.scrollTop"), "400");
    assert_eq!(eval(&e.main, "ev.join()"), "", "the event is not synchronous");
    tasks.drain();
    assert_eq!(eval(&e.main, "ev.join()"), "400");
    // Shrunk without a read: the next frame clamps and fires.
    eval(&e.main, "ev = []; c.style.height = '300px'; 1");
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "[ev.join(), s.scrollTop].join('|')"), "200|200");
    // Nothing changed since: no further clamp or event.
    eval(&e.main, "ev = []; 1");
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "ev.join()"), "");
    e.page.dispose();
}

#[test]
fn abort_controller_signal_and_traversal_filters_survive_gc_through_their_wrappers() {
    let Env { engine, page, main } = env("<body><div id=a></div><div id=b></div><div id=c></div></body>");
    // Only the controller / walkers keep these values reachable.
    assert_eq!(
        eval(
            &main,
            "(function(){ const c = new AbortController(); globalThis.c = c; c.signal.foo = 7;\
             globalThis.w = document.createTreeWalker(document.body, NodeFilter.SHOW_ELEMENT,\
               { tag: 'F', acceptNode(n) { return n.id === 'a' ? NodeFilter.FILTER_SKIP : NodeFilter.FILTER_ACCEPT; } });\
             globalThis.it = document.createNodeIterator(document.body, NodeFilter.SHOW_ELEMENT,\
               n => n.id === 'a' ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT); })(); 'ok'"
        ),
        "ok"
    );
    for _ in 0..4 {
        engine.collect_garbage_now();
        engine.drain_finalized();
    }
    assert_eq!(
        eval(
            &main,
            "[c.signal.foo, c.signal === c.signal, c.signal.aborted, (c.abort('r'), c.signal.aborted), c.signal.reason,\
              w.filter.tag, w.filter === w.filter, w.firstChild().id, it.nextNode().tagName, it.nextNode().id,\
              typeof it.filter, document.createTreeWalker(document.body).filter].join()"
        ),
        "7,true,false,true,r,F,true,b,BODY,b,function,"
    );
    drop(page);
}

#[test]
fn abort_controllers_and_traversal_filters_do_not_keep_a_disposed_page_alive() {
    // A controller whose signal reaches back to it, and walkers whose filters
    // reach back to them, through the page's globals: no native root may pin
    // that cycle once the page is disposed.
    let Env { engine, page, main } = env("<body><div id=a></div></body>");
    assert_eq!(
        eval(
            &main,
            "globalThis.c = new AbortController(); c.signal.owner = c; c.signal.addEventListener('abort', () => c);\
             globalThis.w = document.createTreeWalker(document.body, NodeFilter.SHOW_ELEMENT, { acceptNode: () => (w, 1) });\
             globalThis.it = document.createNodeIterator(document.body, NodeFilter.SHOW_ALL, () => (it, 1));\
             w.firstChild(); it.nextNode(); 'ok'"
        ),
        "ok"
    );
    page.after_task();
    let weak = Rc::downgrade(&page);
    page.dispose();
    main.dispose();
    drop(main);
    drop(page);
    // Conservative stack scanning can keep a wrapper alive for a few extra collections when other
    // tests run on neighbouring threads; the page must still be released within a bounded number.
    for _ in 0..40 {
        engine.collect_garbage_now();
        engine.drain_finalized();
        if weak.upgrade().is_none() {
            break;
        }
    }
    assert!(weak.upgrade().is_none(), "an AbortController or traversal filter kept its disposed PageDom alive");
}

/// Removing (or replacing) the focused element makes `activeElement` the
/// body; keys then go nowhere instead of to a detached node, and the
/// replacement can take focus. Chromium fires `blur` first (see
/// `removing_the_focused_element_fires_blur_and_focusout_first`).
#[test]
fn removed_focused_element_is_no_longer_active() {
    let e = env("<body><div id=cell><input id=a value=x></div></body>");
    let r = &e.main;
    assert_eq!(
        eval(r, "const a = document.getElementById('a'); let blurs = 0; a.addEventListener('blur', () => blurs++); a.focus(); const before = document.activeElement.id; a.remove(); [before, document.activeElement.tagName, document.activeElement.isConnected, blurs, a.isConnected, a.parentNode === null].join(':')"),
        "a:BODY:true:1:false:true"
    );
    assert_eq!(
        eval(r, "const b = document.createElement('input'); b.id = 'b'; document.getElementById('cell').appendChild(b); b.focus(); const c = document.createElement('input'); c.id = 'c'; b.replaceWith(c); const mid = document.activeElement.tagName; c.focus(); [mid, document.activeElement.id].join(':')"),
        "BODY:c"
    );
    // Re-inserting a removed element does not restore its focus.
    assert_eq!(
        eval(r, "const c2 = document.getElementById('c'); c2.remove(); document.getElementById('cell').appendChild(c2); document.activeElement.tagName"),
        "BODY"
    );
}

/// Chromium (`Document::RemoveFocusedElementOfSubtree`) blurs the focused
/// element before a script removes it or an ancestor, while it is still
/// connected: `blur` then `focusout` (no related target), seen by a listener
/// on the document as React's root-delegated `onBlur` is. That holds for
/// `removeChild`, `replaceChild`, `replaceWith`, `innerHTML`, `textContent`
/// and a move by `appendChild`; a blur handler that moves the node makes the
/// removal throw `NotFoundError`.
#[test]
fn removing_the_focused_element_fires_blur_and_focusout_first() {
    let e = env("<body><div id=cell></div><div id=other></div></body>");
    let r = &e.main;
    eval(r, "window.log = []; for (const t of ['blur', 'focusout']) document.addEventListener(t, (e) => log.push(t + ':' + e.target.id + ':' + e.target.isConnected + ':' + (e.relatedTarget === null) + ':' + document.activeElement.tagName), true); window.make = (id) => { const i = document.createElement('input'); i.id = id; document.getElementById('cell').appendChild(i); i.focus(); return i; };");
    let cases = [
        ("const a = make('a'); a.parentNode.removeChild(a);", "a"),
        ("const b = make('b'); b.parentNode.replaceChild(document.createElement('span'), b);", "b"),
        ("make('c').replaceWith('x');", "c"),
        ("make('d'); document.getElementById('cell').innerHTML = '';", "d"),
        ("make('e'); document.getElementById('cell').textContent = '';", "e"),
        ("document.getElementById('other').appendChild(make('f'));", "f"),
    ];
    for (script, id) in cases {
        eval(r, &format!("log.length = 0; {script}"));
        assert_eq!(
            eval(r, "log.join(',') + '|' + document.activeElement.tagName"),
            format!("blur:{id}:true:true:BODY,focusout:{id}:true:true:BODY|BODY"),
            "{script}"
        );
    }
    // Removing a sibling of the focused element, or a detached one, blurs nothing.
    assert_eq!(
        eval(r, "log.length = 0; const g = make('g'); const s = document.createElement('span'); g.after(s); s.remove(); [log.length, document.activeElement.id].join(':')"),
        "0:g"
    );
    assert_eq!(
        eval(r, "const h = make('h'); h.addEventListener('blur', () => document.getElementById('other').appendChild(h), { once: true }); let err = ''; try { h.parentNode.removeChild(h); } catch (x) { err = x.name; } [err, h.parentNode.id].join(':')"),
        "NotFoundError:other"
    );
}

/// Chromium's compatibility mouse events: a canceled `pointerdown` holds
/// back `mousedown` and the `mouseup` of that press (not `click`); a
/// canceled `pointerup` holds back nothing (React Aria's press handlers
/// cancel it on links, which then still get `mouseup` and `click`).
#[test]
fn only_a_canceled_pointerdown_suppresses_the_mouseup() {
    let e = env("<body style=margin:0><a id=l href=#x style=display:block;width:100px;height:40px>link</a></body>");
    let r = &e.main;
    eval(r, "window.log = []; window.cancel = ''; const l = document.getElementById('l'); for (const t of ['pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click']) l.addEventListener(t, (e) => { log.push(t); if (t === cancel || t === 'click') e.preventDefault(); });");
    let press = || {
        let m = input::MouseInput {
            x: 20.0,
            y: 20.0,
            button: input::MouseButton::Left,
            buttons: 1,
            click_count: 1,
            ..Default::default()
        };
        input::dispatch_mouse(&e.page, input::MouseKind::Pressed, &m).unwrap();
        input::dispatch_mouse(&e.page, input::MouseKind::Released, &input::MouseInput { buttons: 0, ..m }).unwrap();
    };
    for (cancel, want) in [
        ("pointerup", "pointerdown,mousedown,pointerup,mouseup,click"),
        ("pointerdown", "pointerdown,pointerup,click"),
        ("", "pointerdown,mousedown,pointerup,mouseup,click"),
    ] {
        eval(r, &format!("log.length = 0; cancel = '{cancel}';"));
        press();
        assert_eq!(eval(r, "log.join()"), want, "pointer{cancel} canceled");
    }
}

/// `selectionchange` on the document for text controls too: focus, a
/// scripted selection, select-all, typing and caret keys (tasks run
/// synchronously in this harness, so nothing coalesces here).
#[test]
fn text_controls_fire_selectionchange() {
    let e = env("<body><input id=t value=abc><input id=u></body>");
    let r = &e.main;
    eval(r, "window.sc = 0; window.targets = []; document.addEventListener('selectionchange', (e) => { sc++; targets.push(e.target.id || e.target.nodeName); }, true); document.getElementById('t').focus();");
    assert_eq!(eval(r, "String(sc) + ':' + targets.join()"), "1:t", "fired on the control, seen at document capture");
    eval(r, "const t = document.getElementById('t'); t.setSelectionRange(1, 2); t.select();");
    assert_eq!(eval(r, "String(sc)"), "3");
    input::insert_text(&e.page, "z");
    assert_eq!(eval(r, "String(sc) + ':' + t.value"), "4:z");
    let k = input::KeyInput { key: "ArrowLeft".into(), code: "ArrowLeft".into(), key_code: 37, ..Default::default() };
    input::dispatch_key(&e.page, input::KeyKind::RawKeyDown, &k);
    assert_eq!(eval(r, "String(sc)"), "5");
}

#[test]
fn live_collections_follow_changes_in_their_own_subtree() {
    let e = env("<!doctype html><body><div id=list><p class=x>1</p><p>2</p><p class=x>3</p></div><div id=other class=x></div></body>");
    let r = &e.main;
    eval(r, "var list = document.getElementById('list'); var kids = list.children; var xs = list.getElementsByClassName('x'); var nodes = list.childNodes; var ps = document.getElementsByTagName('p')");
    assert_eq!(eval(r, "kids.length + ':' + xs.length + ':' + nodes.length + ':' + ps.length"), "3:2:3:3");
    // Text written under a child keeps `children`, and a loop over it sees
    // every child once.
    assert_eq!(
        eval(r, "let n = 0; for (const c of kids) { c.textContent = 'v' + n++; } n + ':' + kids[2].textContent"),
        "3:v2"
    );
    // A change elsewhere in the document does not affect them...
    eval(r, "document.getElementById('other').append(document.createElement('p')); document.getElementById('other').className = 'y'");
    assert_eq!(eval(r, "kids.length + ':' + xs.length + ':' + ps.length"), "3:2:4");
    // ...changes inside do: class, insertion, removal, move, text node.
    assert_eq!(eval(r, "kids[1].className = 'x'; xs.length + ':' + xs[1].textContent"), "3:v1");
    assert_eq!(eval(r, "list.append(document.createElement('p')); kids.length + ':' + ps.length"), "4:5");
    assert_eq!(eval(r, "kids[0].remove(); kids.length + ':' + xs.length + ':' + xs[0].textContent"), "3:2:v1");
    assert_eq!(eval(r, "list.prepend(kids[1]); kids[0].textContent + ':' + xs[0].textContent"), "v2:v2");
    assert_eq!(
        eval(r, "const before = nodes.length; list.append('text'); before + ':' + nodes.length + ':' + kids.length"),
        "3:4:3"
    );
    // A nested insertion counts for descendant collections.
    assert_eq!(
        eval(
            r,
            "kids[0].append(Object.assign(document.createElement('p'), {className: 'x'})); xs.length + ':' + ps.length"
        ),
        "3:5"
    );
}

#[test]
fn collections_over_the_same_root_and_name_share_members() {
    let e = env("<!doctype html><body><i class='a b'></i><i class='b'></i><i class='a'></i></body>");
    let r = &e.main;
    assert_eq!(eval(r, "document.getElementsByClassName('a').length"), "2");
    // A second object over the same root and name sees the same members,
    // and stays live on its own.
    assert_eq!(eval(r, "var c = document.getElementsByClassName('a'); document.body.lastChild.className = 'b'; c.length + ':' + document.getElementsByClassName('a').length"), "1:1");
    assert_eq!(eval(r, "document.getElementsByClassName('a b').length + ':' + document.getElementsByClassName(' b  a ').length + ':' + document.getElementsByClassName('').length"), "1:1:0");
}

#[test]
fn class_name_collections_ignore_case_in_quirks_mode() {
    // No doctype: quirks mode.
    {
        let e = env("<body><i class=Foo></i><i class=foo></i></body>");
        let r = &e.main;
        assert_eq!(
            eval(r, "document.compatMode + ':' + document.getElementsByClassName('FOO').length"),
            "BackCompat:2"
        );
    }
    let e = env("<!doctype html><body><i class=Foo></i><i class=foo></i></body>");
    let r = &e.main;
    assert_eq!(eval(r, "document.compatMode + ':' + document.getElementsByClassName('FOO').length"), "CSS1Compat:0");
}

#[test]
fn mutation_observers_still_see_every_mutation() {
    let e = env("<!doctype html><body><div id=box></div></body>");
    let r = &e.main;
    // Many mutations with an observer attached, taken synchronously.
    assert_eq!(
        eval(r, "var box = document.getElementById('box'); var mo = new MutationObserver(() => {}); mo.observe(box, {childList: true, attributes: true, characterData: true, subtree: true}); for (let i = 0; i < 100; i++) { const s = document.createElement('span'); box.append(s); s.setAttribute('data-i', i); s.textContent = 'x'; } mo.takeRecords().length"),
        "300"
    );
    // A target observed while detached is observed once connected.
    assert_eq!(
        eval(r, "var loose = document.createElement('div'); var seen = []; var mo2 = new MutationObserver(rs => seen.push(rs.length)); mo2.observe(loose, {attributes: true}); document.body.append(loose); loose.id = 'now'; mo2.takeRecords().length"),
        "1"
    );
    // After every observer disconnects, mutations are not recorded.
    assert_eq!(
        eval(
            r,
            "mo.disconnect(); mo2.disconnect(); box.append('more'); mo.takeRecords().length + mo2.takeRecords().length"
        ),
        "0"
    );
}

#[test]
fn queued_scroll_flush_preserves_target_order_and_does_not_repeat_at_fallback() {
    let e = env("<body><div id=a style='height:20px;width:20px;overflow:auto'><div style='height:200px'></div></div><div id=b style='height:20px;width:20px;overflow:auto'><div style='height:200px'></div></div></body>");
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    eval(&e.main,"window.seen=[];var a=document.getElementById('a'),b=document.getElementById('b');for(const el of [a,b])for(const type of ['scroll','scrollend'])el.addEventListener(type,()=>seen.push(el.id+':'+type+':'+el.scrollTop));a.scrollTop=10;b.scrollTop=20;a.scrollTop=30;");
    assert_eq!(eval(&e.main, "seen.join()"), "");
    lp_webapi::layout_api::flush_pending_scrolls(&e.page);
    let expected = "a:scroll:30,a:scrollend:30,b:scroll:20,b:scrollend:20";
    assert_eq!(eval(&e.main, "seen.join()"), expected);
    tasks.drain();
    assert_eq!(eval(&e.main, "seen.join()"), expected, "timer fallback must not redeliver a rendering-update flush");
}

#[test]
fn intersection_observer_fractional_full_containment_is_exact_and_partial_clipping_is_not() {
    let e = env(
        r#"<body style="margin:0"><div id="clip" style="position:absolute;left:1700px;top:10px;width:208px;height:40px;overflow:hidden"><div style="display:flex;width:208px"><div id="target" style="flex:1;height:20px"></div><div style="flex:1;height:20px"></div><div style="flex:1;height:20px"></div></div></div></body>"#,
    );
    let mut viewport = e.page.viewport();
    viewport.width = 1920;
    e.page.set_viewport(viewport);
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    eval(
        &e.main,
        r#"var records=[]; var clipper=document.getElementById('clip');
        var observer=new IntersectionObserver(entries=>records.push(...entries),{threshold:[1]});
        observer.observe(document.getElementById('target'));"#,
    );
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "records.map(entry=>entry.intersectionRatio).join()"), "1");
    assert_eq!(eval(&e.main, "records[0].intersectionRect.width===records[0].boundingClientRect.width && records[0].intersectionRect.height===records[0].boundingClientRect.height"), "true");

    eval(&e.main, "clipper.style.width='30px'");
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "records.length===2 && records[1].isIntersecting && records[1].intersectionRatio>0 && records[1].intersectionRatio<1 && records[1].intersectionRect.width===30"), "true");

    eval(&e.main, "clipper.style.width='208px'");
    e.page.after_task();
    tasks.drain();
    assert_eq!(eval(&e.main, "records.length===3 && records[2].intersectionRatio===1"), "true");
    e.page.dispose();
}

#[test]
fn queued_scroll_old_fallback_cannot_consume_requeued_target() {
    let e = env(
        "<body><div id=a style='height:20px;width:20px;overflow:auto'><div style='height:200px'></div></div></body>",
    );
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    eval(&e.main, "window.seen=[];var a=document.getElementById('a');a.addEventListener('scroll',()=>{seen.push(a.scrollTop);if(a.scrollTop===10)a.scrollTop=20});a.scrollTop=10;");
    let old_fallback = tasks.0.borrow_mut().pop_front().expect("first scroll fallback");
    lp_webapi::layout_api::flush_pending_scrolls(&e.page);
    assert_eq!(eval(&e.main, "seen.join()"), "10");
    old_fallback();
    assert_eq!(eval(&e.main, "seen.join()"), "10", "the previous entry's timer cannot deliver the reentrant scroll");
    lp_webapi::layout_api::flush_pending_scrolls(&e.page);
    assert_eq!(eval(&e.main, "seen.join()"), "10,20");
    tasks.drain();
    assert_eq!(eval(&e.main, "seen.join()"), "10,20", "neither old nor new fallback may duplicate a flushed event");
}

#[test]
fn unhandled_rejection_formats_reason_only_after_uncanceled_dispatch() {
    #[derive(Default)]
    struct Reports(RefCell<Vec<String>>);
    impl lp_js::EngineHooks for Reports {
        fn unhandled_rejection(&self, _realm: &Realm, exception: &lp_js::JsException) {
            self.0.borrow_mut().push(exception.message.clone());
        }
    }
    let e = env("<body></body>");
    assert!(e.engine.capabilities().unhandled_rejections);
    let reports = Rc::new(Reports::default());
    e.engine.set_hooks(Some(reports.clone()));
    eval(&e.main, "globalThis.reads=0;globalThis.seen=[];globalThis.reason={get message(){reads++;return 'owned'}};globalThis.cancel=e=>{seen.push(reads);e.preventDefault()};addEventListener('unhandledrejection',cancel);Promise.reject(reason);null;");
    assert_eq!(eval(&e.main, "seen.join()+'|'+reads"), "0|0");
    assert!(reports.0.borrow().is_empty());
    eval(&e.main, "removeEventListener('unhandledrejection',cancel);addEventListener('unhandledrejection',()=>seen.push(reads));Promise.reject(reason);null;");
    assert_eq!(eval(&e.main, "seen.join()+'|'+reads"), "0,0|1");
    assert_eq!(*reports.0.borrow(), ["owned"]);
    e.engine.set_hooks(None);
    eval(&e.main, "Promise.reject(reason);null;");
    assert_eq!(eval(&e.main, "seen.join()+'|'+reads"), "0,0,1|1", "no default reporter means no reason conversion");
}

#[test]
fn scroll_into_view_if_needed_matches_chromium_boolean_alignment() {
    // Oracle: standards-mode Chrome at400x300. True/default centers only a
    // wholly hidden axis; false uses the nearest edge. Visible axes stay put.
    for (argument, center) in [
        ("", true),
        ("true", true),
        ("undefined", true),
        ("false", false),
        ("0", false),
        ("null", false),
        ("''", false),
        ("'false'", true),
        ("({valueOf(){throw new Error('must not coerce an object')}})", true),
    ] {
        let e = env("<!doctype html><style>html{overflow:hidden}body{margin:0;width:2000px;height:2000px}#target{position:absolute;left:100px;top:500px;width:20px;height:20px}</style><div id=target>target</div>");
        let mut viewport = e.page.viewport();
        viewport.width = 400;
        viewport.height = 300;
        e.page.set_viewport(viewport);
        for (initial_y, height, centered_y, nearest_y) in [
            (400, 20, 400, 400),
            (210, 20, 220, 220),
            (0, 20, 360, 220),
            (510, 20, 500, 500),
            (900, 20, 360, 500),
            (600, 500, 600, 600),
        ] {
            let expected = if center { centered_y } else { nearest_y };
            assert_eq!(
                eval(&e.main, &format!("(()=>{{const t=document.getElementById('target');t.style.left='100px';t.style.height='{height}px';window.scrollTo(0,{initial_y});t.scrollIntoViewIfNeeded({argument});return JSON.stringify([scrollX,scrollY])}})()")),
                format!("[0,{expected}]"),
                "argument={argument}, initial_y={initial_y}, height={height}"
            );
        }
        assert_eq!(
            eval(&e.main, &format!("(()=>{{const t=document.getElementById('target');t.style.left='500px';t.style.height='20px';window.scrollTo(0,400);t.scrollIntoViewIfNeeded({argument});return JSON.stringify([scrollX,scrollY])}})()")),
            if center { "[310,400]" } else { "[120,400]" },
            "horizontal alignment must preserve the already visible vertical axis: {argument}"
        );
        e.page.dispose();
    }
}

#[test]
fn scroll_into_view_if_needed_handles_nested_overflow_and_border_origins() {
    for (overflow, border, padding, left, top, centered, nearest) in [
        ("auto", 0, 0, 200, 200, [150, 160], [100, 120]),
        ("hidden", 0, 0, 200, 200, [150, 160], [100, 120]),
        ("clip", 0, 0, 200, 200, [0, 0], [0, 0]),
        ("visible", 0, 0, 200, 200, [0, 0], [0, 0]),
        ("auto", 0, 0, 105, 90, [5, 10], [5, 10]),
        ("auto", 0, 0, 40, 40, [0, 0], [0, 0]),
        ("auto", 10, 8, 200, 200, [150, 160], [92, 112]),
    ] {
        let e = env(&format!("<!doctype html><style>html{{overflow:hidden}}body{{margin:0;width:2000px;height:2000px}}#sc{{position:absolute;left:20px;top:20px;width:120px;height:100px;border:{border}px solid;padding:{padding}px;overflow:{overflow}}}#inner{{position:relative;width:500px;height:500px}}#target{{position:absolute;left:{left}px;top:{top}px;width:20px;height:20px}}</style><div id=sc><div id=inner><div id=target>target</div></div></div>"));
        let mut viewport = e.page.viewport();
        viewport.width = 400;
        viewport.height = 300;
        e.page.set_viewport(viewport);
        for (argument, center) in [("", true), ("true", true), ("false", false)] {
            let [x, y] = if center { centered } else { nearest };
            assert_eq!(
                eval(&e.main, &format!("(()=>{{const sc=document.getElementById('sc');sc.scrollTo(0,0);window.scrollTo(0,0);document.getElementById('target').scrollIntoViewIfNeeded({argument});return JSON.stringify([sc.scrollLeft,sc.scrollTop,scrollX,scrollY])}})()")),
                format!("[{x},{y},0,0]"),
                "overflow={overflow}, border={border}, padding={padding}, argument={argument}"
            );
        }
        e.page.dispose();
    }
}

#[test]
fn scroll_into_view_if_needed_queues_only_actual_moves_and_preserves_default_scroll() {
    let e = env("<!doctype html><style>html{overflow:hidden}body{margin:0;height:2000px}#target{position:absolute;left:100px;top:100px;width:20px;height:20px}</style><div id=target>target</div>");
    let mut viewport = e.page.viewport();
    viewport.width = 400;
    viewport.height = 300;
    e.page.set_viewport(viewport);
    let tasks = Rc::new(ObserverTasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    eval(&e.main, "window.seen=[];for(const type of ['scroll','scrollend'])document.addEventListener(type,event=>{if(event.target===document)seen.push(type)})");
    assert_eq!(
        eval(
            &e.main,
            "document.getElementById('target').scrollIntoViewIfNeeded();JSON.stringify([scrollY,seen.length])"
        ),
        "[0,0]"
    );
    assert!(tasks.0.borrow().is_empty(), "an already visible target must queue no scroll event");
    assert_eq!(eval(&e.main, "document.getElementById('target').style.top='500px';document.getElementById('target').scrollIntoViewIfNeeded(false);JSON.stringify([scrollY,seen.length])"), "[220,0]");
    tasks.drain();
    assert_eq!(eval(&e.main, "seen.join(',')"), "scroll,scrollend");
    assert_eq!(eval(&e.main, "seen.length=0;document.getElementById('target').scrollIntoViewIfNeeded();JSON.stringify([scrollY,seen.length])"), "[220,0]");
    assert!(tasks.0.borrow().is_empty(), "true must not recenter an already visible target");
    eval(&e.main, "window.scrollTo(0,0)");
    tasks.drain();
    assert_eq!(
        eval(
            &e.main,
            "seen.length=0;document.getElementById('target').scrollIntoView();JSON.stringify([scrollY,seen.length])"
        ),
        "[500,0]",
        "the separate default-start API keeps its existing behavior"
    );
    tasks.drain();
    assert_eq!(eval(&e.main, "seen.join(',')"), "scroll,scrollend");
    e.page.dispose();
}

#[test]
fn resize_observer_input_update_preserves_queued_and_reentrant_callbacks() {
    #[derive(Default)]
    struct Tasks {
        ready: RefCell<std::collections::VecDeque<Box<dyn FnOnce()>>>,
        delayed: RefCell<Vec<Box<dyn FnOnce()>>>,
    }
    impl PageHooks for Tasks {
        fn navigate(&self, _: NavigationRequest) {}
        fn post_task(&self, task: Box<dyn FnOnce()>) {
            self.ready.borrow_mut().push_back(task);
        }
        fn post_delayed_task(&self, _: std::time::Duration, task: Box<dyn FnOnce()>) {
            self.delayed.borrow_mut().push(task);
        }
    }
    impl Tasks {
        fn drain_ready(&self) {
            for _ in 0..20 {
                let task = self.ready.borrow_mut().pop_front();
                let Some(task) = task else { return };
                task();
            }
            panic!("resize callback queue did not settle");
        }
    }
    let e = env("<div id=target style='width:10px;height:10px'></div>");
    let tasks = Rc::new(Tasks::default());
    e.page.set_hooks(Some(tasks.clone()));
    eval(&e.main, "window.seen=[];window.target=document.getElementById('target');window.ro=new ResizeObserver(entries=>{for(const entry of entries){seen.push(entry.contentRect.width);if(entry.contentRect.width===20)target.style.width='30px'}});ro.observe(target)");
    e.page.after_task();
    assert!(e.page.pending_resize_observer_work());
    e.page.request_resize_observer_update_for_input();
    assert_eq!(eval(&e.main, "seen.join()"), "", "input must not deliver a queued callback inline");
    tasks.drain_ready();
    assert_eq!(eval(&e.main, "seen.join()"), "10");

    eval(&e.main, "target.style.width='20px'");
    e.page.after_task();
    let old_wakes = tasks.delayed.take();
    e.page.request_resize_observer_update_for_input();
    assert!(e.page.pending_resize_observer_work(), "the real changed-size callback is still pending");
    assert_eq!(eval(&e.main, "seen.join()"), "10");
    tasks.drain_ready();
    assert_eq!(eval(&e.main, "seen.join()"), "10,20");
    assert_eq!(eval(&e.main, "target.style.width"), "30px", "the callback changed the observation again");

    e.page.after_task();
    let before = e.page.resize_observer_work_state();
    for old in old_wakes {
        old();
    }
    let after = e.page.resize_observer_work_state();
    assert_eq!((after.1, after.2), (before.1, before.2), "obsolete wakes cannot consume newer pending work");
    e.page.request_resize_observer_update_for_input();
    assert!(e.page.pending_resize_observer_work());
    assert_eq!(eval(&e.main, "seen.join()"), "10,20", "reentrant changes also retain asynchronous delivery");
    tasks.drain_ready();
    assert_eq!(eval(&e.main, "seen.join()"), "10,20,30");
    assert!(!e.page.pending_resize_observer_work());
    e.page.request_resize_observer_update_for_input();
    tasks.drain_ready();
    assert_eq!(eval(&e.main, "seen.join()"), "10,20,30", "a clean input checkpoint must not duplicate delivery");
    e.page.dispose();
}

#[test]
fn xml_serializer_doctype_html_and_namespaces() {
    let e = env("<!doctype html><p id=a title='x\"y'>t&amp;<br><span></span></p><!--c-->");
    let r = &e.main;
    let ser = "var ser=new XMLSerializer();";
    assert_eq!(eval(r, &format!("{ser} ser.serializeToString(document.doctype)")), "<!DOCTYPE html>");
    assert_eq!(
        eval(r, &format!("{ser} ser.serializeToString(document.querySelector('p'))")),
        r#"<p xmlns="http://www.w3.org/1999/xhtml" id="a" title="x&quot;y">t&amp;<br /><span></span></p>"#
    );
    assert_eq!(eval(r, &format!("{ser} ser.serializeToString(document.createComment('c'))")), "<!--c-->");
    assert_eq!(eval(r, &format!("{ser} ser.serializeToString(document.createTextNode('a<b'))")), "a&lt;b");
    assert_eq!(
        eval(r, &format!("{ser} ser.serializeToString(document.implementation.createDocumentType('a','p','s'))")),
        r#"<!DOCTYPE a PUBLIC "p" "s">"#
    );
    assert_eq!(
        eval(r, &format!("{ser} ser.serializeToString(new DOMParser().parseFromString('<w:d xmlns:w=\"urn:w\"><w:e a=\"1\"/><f/></w:d>','application/xml'))")),
        r#"<w:d xmlns:w="urn:w"><w:e a="1"/><f/></w:d>"#
    );
    assert_eq!(eval(r, "try{new XMLSerializer().serializeToString('x')}catch(e){e.name}"), "TypeError");
}

#[test]
fn inline_handler_attributes_from_parser_innerhtml_and_clone() {
    let e = env(
        "<form id=f><input name=n value=v><button id=b type=button onclick=\"window.got=[this.id,event.type,elements.n.value,typeof getElementById].join()\">x</button></form>\
         <a id=l href=#x onclick=\"return false\">l</a><div id=host></div><div id=bad onclick=\"(\">z</div>",
    );
    let r = &e.main;
    assert_eq!(eval(r, "document.getElementById('b').click(); window.got"), "b,click,v,function");
    // A false return value cancels, and the handler is readable as a function.
    assert_eq!(
        eval(r, "var ev=new MouseEvent('click',{cancelable:true}); document.getElementById('l').dispatchEvent(ev); [ev.defaultPrevented, typeof document.getElementById('l').onclick].join()"),
        "true,function"
    );
    // innerHTML, cloning, and replacing through the IDL attribute.
    assert_eq!(
        eval(r, "var h=document.getElementById('host'); h.innerHTML='<i id=i onclick=\"window.c=(window.c||0)+1\"></i>'; var i=document.getElementById('i'); i.click(); var k=i.cloneNode(); h.append(k); k.click(); i.onclick=()=>{window.c=100}; i.click(); window.c"),
        "100"
    );
    // A handler that does not compile leaves the attribute inert.
    assert_eq!(eval(r, "document.getElementById('bad').click(); document.getElementById('bad').onclick"), "null");
    // Inert documents never run handlers.
    assert_eq!(
        eval(r, "new DOMParser().parseFromString('<b onclick=\"window.inert=1\">','text/html').querySelector('b').click(); String(window.inert)"),
        "undefined"
    );
}

#[test]
fn inline_handler_runs_before_a_later_listener() {
    let e = env("<button id=b onclick=\"window.o=(window.o||'')+'a'\">x</button>");
    let r = &e.main;
    assert_eq!(
        eval(
            r,
            "var b=document.getElementById('b'); b.addEventListener('click',()=>{window.o+='l'}); b.click(); window.o"
        ),
        "al"
    );
}

#[test]
fn declarative_shadow_dom_apis() {
    let e = env("<!doctype html><body>");
    assert_eq!(
        eval(
            &e.main,
            "const h=document.createElement('div');
             const r=h.attachShadow({mode:'open',clonable:true,serializable:true,delegatesFocus:true,slotAssignment:'named'});
             r.innerHTML='<slot></slot>'; h.innerHTML='<i>l</i>';
             [r.clonable,r.serializable,r.delegatesFocus,r.slotAssignment,h.getHTML({serializableShadowRoots:true}),h.getHTML()].join('|')"
        ),
        "true|true|true|named|<template shadowrootmode=\"open\" shadowrootdelegatesfocus=\"\" shadowrootserializable=\"\" shadowrootclonable=\"\"><slot></slot></template><i>l</i>|<i>l</i>"
    );
    assert_eq!(
        eval(
            &e.main,
            "const w=document.createElement('div');
             w.setHTMLUnsafe('<div id=host><template shadowrootmode=open><slot name=a id=s></slot></template><b slot=a>x</b>tail</div>');
             const host=w.firstChild, b=host.querySelector('b'), s=host.shadowRoot.getElementById('s');
             [host.children.length, b.assignedSlot===s, s.assignedNodes().length, s.assignedElements()[0]===b, host.lastChild.assignedSlot].join('|')"
        ),
        "1|true|1|true|"
    );
    assert_eq!(
        eval(
            &e.main,
            "const d=Document.parseHTMLUnsafe('<div id=x><template shadowrootmode=open>y</template></div>');
             const t=document.createElement('template'); t.shadowRootMode='CLOSED'; t.shadowRootClonable=true;
             [d.getElementById('x').shadowRoot.textContent, t.shadowRootMode, t.outerHTML].join('|')"
        ),
        "y|closed|<template shadowrootmode=\"CLOSED\" shadowrootclonable=\"\"></template>"
    );
}

#[test]
fn base_element_sets_the_document_base_and_hyperlink_setters_rewrite_href() {
    let e = env("<head><base id=b href='http://example.org/foo/bar'><body><a id=a href='x'></a><area id=r href='y'>");
    let doc = e.page.document();
    e.page.dom.borrow_mut().set_document_url(doc, "http://page.test/dir/index.html").unwrap();
    eval(&e.main, "for(const id of ['a','b','r'])globalThis[id]=document.getElementById(id);0");
    assert_eq!(
        eval(&e.main, "[document.baseURI,b.href,a.href,r.href,a.origin].join('|')"),
        "http://example.org/foo/bar|http://example.org/foo/bar|http://example.org/foo/x|http://example.org/foo/y|http://example.org"
    );
    // The base is live, and a relative <base href> resolves against the fallback base only.
    assert_eq!(
        eval(&e.main, "b.href='sub/';[document.baseURI,b.href,a.href].join('|')"),
        "http://page.test/dir/sub/|http://page.test/dir/sub/|http://page.test/dir/sub/x"
    );
    assert_eq!(eval(&e.main, "b.href='data:text/plain,x';document.baseURI"), "http://page.test/dir/index.html");
    assert_eq!(eval(&e.main, "b.remove();document.baseURI"), "http://page.test/dir/index.html");
    assert_eq!(eval(&e.main, "a.href"), "http://page.test/dir/x");
    assert_eq!(
        eval(&e.main, "let n=document.createElement('base');n.href='http://two.test/';document.head.append(n);a.href"),
        "http://two.test/x"
    );

    assert_eq!(
        eval(&e.main, "a.href='http://u:p@h.test:8/p?q#f';a.protocol='https:';a.host='i.test:9';a.pathname='/r';a.search='s';a.hash='g';a.username='v';a.password='w';[a.getAttribute('href'),a.hostname,a.port].join('|')"),
        "https://v:w@i.test:9/r?s#g|i.test|9"
    );
    // A URL that does not parse keeps the input as href and ignores writes.
    assert_eq!(
        eval(&e.main, "a.href='http://f:b/c';a.pathname='/x';[a.href,a.protocol,a.host,a.pathname,a.origin].join('|')"),
        "http://f:b/c|:|||"
    );
    assert_eq!(eval(&e.main, "a.removeAttribute('href');[a.href,a.protocol].join('|')"), "|:");
    e.page.dispose();
}

#[test]
fn url_hosts_with_invalid_punycode_parse_when_ascii() {
    let e = env("<body>");
    assert_eq!(
        eval(&e.main, "['xn--ab-j1t','XN--0.pt','xn--a.xn--zca','a.b.c.xn--pokxncvks','xn--'].map(h=>new URL(`https://${h}/x`).host).join('|')"),
        "xn--ab-j1t|xn--0.pt|xn--a.xn--zca|a.b.c.xn--pokxncvks|xn--"
    );
    for host in ["xn--a.\u{df}", "xn--te\u{161}la", "xn--a b", "a\u{200d}.example", "xn--a%"] {
        assert_eq!(eval(&e.main, &format!("URL.canParse('https://{host}/x')")), "false", "{host}");
    }
    e.page.dispose();
}

#[test]
fn invalid_urls_throw_from_send_beacon_and_window_open_and_response_form_data_parses() {
    let e = env("<body>");
    let doc = e.page.document();
    e.page.dom.borrow_mut().set_document_url(doc, "http://page.test/").unwrap();
    let thrown =
        |call: &str| eval(&e.main, &format!("(()=>{{try{{{call};return 'none'}}catch(x){{return x.name}}}})()"));
    assert_eq!(thrown("navigator.sendBeacon('http://f:b/')"), "TypeError");
    assert_eq!(thrown("navigator.sendBeacon('data:text/plain,x')"), "TypeError");
    assert_eq!(thrown("navigator.sendBeacon('/ok')"), "none");
    assert_eq!(thrown("open('http://f:b/')"), "SyntaxError");

    eval(&e.main, "globalThis.got='';new Response('a=1&b=%F0%9F%90%BC+c&a',{headers:{'Content-Type':'application/x-www-form-urlencoded;charset=shift_jis'}}).formData().then(f=>{got=[...f].join('|')+'|'+(f instanceof FormData)});new Response('a=1').formData().catch(x=>{got2=x.name})");
    assert_eq!(eval(&e.main, "got"), "a,1|b,\u{1F43C} c|a,|true");
    assert_eq!(eval(&e.main, "got2"), "TypeError");
    e.page.dispose();
}

#[test]
fn css_rules_parse_lazily_from_the_sheet_and_stay_live() {
    let e = env("<style id=s>a{color:red} b,,{} @media all { c{top:1px} }</style><div id=d></div>");
    eval(&e.main, "globalThis.st=document.getElementById('s');globalThis.sh=st.sheet;globalThis.rs=sh.cssRules;0");
    assert_eq!(
        eval(&e.main, "[rs.length,rs[0].type,rs[0].selectorText,rs[0].cssText,rs[1].type,rs[1].conditionText,rs[1].cssRules[0].selectorText,rs[1].cssRules[0].parentRule===rs[1],rs[0].parentStyleSheet===sh,rs[0]===sh.cssRules[0]].join('|')"),
        "2|1|a|a { color: red; }|4|all|c|true|true|true"
    );

    // Same list object across edits, and invalid rules throw.
    assert_eq!(
        eval(
            &e.main,
            "sh.insertRule(\"[foo='bar' I] { top: 2px }\");[rs.length,rs===sh.cssRules,rs[0].selectorText].join('|')"
        ),
        "3|true|[foo=\"bar\" i]"
    );
    for rule in ["", "a", "b,,{}", "x{} y{}", "@nonsense;"] {
        let threw = eval(
            &e.main,
            &format!("(()=>{{try{{sh.insertRule('{rule}');return 'none'}}catch(x){{return x.name}}}})()"),
        );
        assert_eq!(threw, "SyntaxError", "{rule}");
    }
    assert_eq!(eval(&e.main, "sh.deleteRule(0);rs.length"), "2");

    // CSSOM edits keep the rules of the text, and reach the cascade.
    eval(&e.main, "globalThis.d=document.getElementById('d');0");
    assert_eq!(
        eval(&e.main, "sh.insertRule('.hit{visibility:hidden}',2);d.className='hit';globalThis.hidden=getComputedStyle(d).visibility;rs[2].selectorText='.other';rs[2].selectorText='!!';[hidden,getComputedStyle(d).visibility,rs[2].selectorText,rs[0].selectorText].join('|')"),
        "hidden|visible|.other|a"
    );

    // Rules nested in a group edit the group.
    assert_eq!(
        eval(&e.main, "globalThis.m=rs[1];m.insertRule('.in{visibility:hidden}',1);d.className='in';hidden=getComputedStyle(d).visibility;m.deleteRule(1);[hidden,getComputedStyle(d).visibility,m.cssRules.length].join('|')"),
        "hidden|visible|1"
    );

    // New text replaces the sheet, edits included.
    assert_eq!(
        eval(
            &e.main,
            "st.textContent=':nth-child(1n+0){}';[sh.cssRules.length,rs.length,rs[0].selectorText].join('|')"
        ),
        "1|1|:nth-child(n)"
    );
    assert_eq!(eval(&e.main, "st.textContent='[x y]{}';sh.cssRules.length"), "0");
    assert_eq!(
        eval(&e.main, "st.textContent=\"@namespace x 'a'; [x|foo=''  i]{}\";[...sh.cssRules].map(r=>r.type).join()"),
        "10,1"
    );

    // A removed style element keeps a working sheet.
    assert_eq!(
        eval(&e.main, "globalThis.n=document.createElement('style');document.head.append(n);globalThis.k=n.sheet;n.remove();globalThis.l=k.cssRules;k.insertRule('a{}');globalThis.one=l.length;k.deleteRule(0);[one,l.length].join('|')"),
        "1|0"
    );
    assert_eq!(
        eval(&e.main, "[CSS.supports('selector(a > b)'),CSS.supports('selector(a >)')].join('|')"),
        "true|false"
    );
    e.page.dispose();
}

#[test]
fn inner_text_follows_computed_white_space() {
    let e = env("<div id=t></div><ul><li id=l>A <span id=o>B</span> C</li></ul>");
    let text = |html: &str| {
        eval(&e.main, &format!("(()=>{{const t=document.getElementById('t');t.innerHTML={html:?};return JSON.stringify(t.innerText)}})()"))
    };
    for (html, expected) in [
        ("<div style='white-space:pre'> a  b\nc\td </div>", r#"" a  b\nc\td ""#),
        ("<span style='white-space:pre-wrap'> a\rb</span>", r#"" a\nb""#),
        ("<div style='white-space:break-spaces'> a </div>", r#"" a ""#),
        ("<pre> a  b</pre>", r#"" a  b""#),
        ("<pre style='white-space:normal'> a  b</pre>", r#""a b""#),
        ("<div style='white-space:pre'><span style='white-space:normal'> a  b</span> c</div>", r#""a b c""#),
        ("<div style='white-space:pre-line'> a  b \n\t c\t</div>", r#""a b\nc""#),
        ("<div style='white-space:pre-line'>\n\na\r\n</div>", r#""\n\na\n""#),
        ("<div style='white-space:pre-line'>a\tb</div>", r#""a b""#),
        ("a <span style='white-space:pre'>b</span>", r#""a b""#),
        ("<span>a </span><span style='white-space:pre'></span> b", r#""a b""#),
        ("<div> a  b </div>", r#""a b""#),
        ("<x>a </x> <x> b</x>", r#""a b""#),
        ("<x>a</x> <x> b</x>", r#""a b""#),
        ("<table style='white-space:pre'>  <td>a</td>  </table>", r#""a""#),
    ] {
        assert_eq!(text(html), expected, "{html}");
    }
    // The element itself can carry the style.
    assert_eq!(
        eval(&e.main, "(()=>{const t=document.getElementById('t');t.style.whiteSpace='pre';t.textContent=' x ';const v=t.innerText;t.style.whiteSpace='';return JSON.stringify(v)})()"),
        r#"" x ""#
    );
    assert_eq!(
        eval(&e.main, "document.getElementById('o').outerText='R';const li=document.getElementById('l');[li.innerHTML,li.childNodes.length].join('|')"),
        "A R C|1"
    );
    e.page.dispose();
}
