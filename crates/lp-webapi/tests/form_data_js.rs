//! `FormData` bindings exercised from JavaScript on the real system
//! JavaScriptCore: live iteration, submitter validation, image-button
//! coordinates and Blob-to-File conversion.

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
    _engine: Engine,
    _page: Rc<PageDom>,
    main: Realm,
}

/// Loads no sub-resources (the pages here have none worth loading).
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
    let doc = parse_document(&dom, "http://localhost:5173/app", html);
    dom.borrow_mut().set_document_active(doc, true).unwrap();
    let page = PageDom::new(dom, doc);
    let mut vp = page.viewport();
    vp.width = 1440;
    vp.height = 900;
    page.set_viewport(vp);
    page.layout.borrow_mut().style_mut().discover_style_elements(&page.dom.borrow());
    let realm = engine
        .create_realm(RealmOptions { name: "main".into(), global_class: Some(window_class), origin: None })
        .expect("realm");
    let key = page.window_target().unwrap_or_else(|| page.create_window_target(page.document()));
    realm
        .set_global_data::<TestWindow>(TestWindowData {
            page: page.clone(),
            target: EventTargetData::window(&page, key),
        })
        .unwrap();
    {
        let s = realm.scope().unwrap();
        install(&s, page.clone()).unwrap();
    }
    Env { _engine: engine, _page: page, main: realm }
}

fn eval(realm: &Realm, src: &str) -> String {
    let s = realm.scope().unwrap();
    match s.eval(src, Some("test.js"), 1) {
        Ok(v) => v.to_string().unwrap(),
        Err(e) => format!("THROW {}", e.details(&s).text()),
    }
}

const FORM: &str = r#"<!doctype html><html><body>
<button name=outerNamed value=GO form=myform></button>
<form id=myform onsubmit="return false">
  <input name=n1 value=v1>
  <button name=named value=GO></button>
  <button id=unnamed value=unnamed></button>
  <button form=another name=unassociated value=unassociated></button>
  <button type=reset name=resetter></button>
  <input type=image name=namedImage src=x.png>
  <input type=image id=unnamedImage src=x.png>
  <input name=n3 value=v3>
</form>
<form id=another><button name=unassociated2 value=unassociated></button></form>
</body></html>"#;

#[test]
fn iteration_is_live_over_the_entry_list() {
    let e = env("<!doctype html><html><body></body></html>");
    let r = &e.main;
    eval(
        r,
        r#"function make(pairs) { const fd = new FormData(); for (const [k, v] of pairs) fd.append(k, v); return fd; }
           function walk(fd, during) { const keys = [], values = [];
             for (const [k, v] of fd) { keys.push(k); values.push(v); during(fd, k); }
             return keys.join(',') + '|' + values.join(','); }"#,
    );
    // Deleting while iterating skips the removed entry.
    assert_eq!(
        eval(r, "walk(make([['foo','0'],['baz','1'],['BAR','2']]), fd => fd.delete('baz'))"),
        "foo,BAR|0,2"
    );
    // Deleting an already-visited entry shifts the remainder (one is skipped).
    assert_eq!(
        eval(
            r,
            "walk(make([['foo','0'],['baz','1'],['BAR','2'],['quux','3']]), (fd, k) => { if (k === 'baz') fd.delete('foo'); })"
        ),
        "foo,baz,quux|0,1,3"
    );
    // Appending during iteration is reached.
    assert_eq!(
        eval(
            r,
            "walk(make([['foo','0'],['baz','1'],['BAR','2'],['quux','3']]), (fd, k) => { if (k === 'baz') fd.append('X-yZ', '4'); })"
        ),
        "foo,baz,BAR,quux,X-yZ|0,1,2,3,4"
    );
    // keys()/values() and forEach are live too.
    assert_eq!(
        eval(
            r,
            "{ const fd = make([['a','1'],['b','2']]); const out = []; for (const k of fd.keys()) { out.push(k); if (k === 'a') fd.append('c', '3'); } out.join(',') }"
        ),
        "a,b,c"
    );
    assert_eq!(
        eval(
            r,
            "{ const fd = make([['a','1'],['b','2']]); const out = []; for (const v of fd.values()) { out.push(v); fd.delete('b'); } out.join(',') }"
        ),
        "1"
    );
    assert_eq!(
        eval(
            r,
            "{ const fd = make([['a','1'],['b','2'],['c','3']]); const out = []; fd.forEach((v, k, self) => { out.push(k + '=' + v + (self === fd)); fd.delete('b'); }); out.join(',') }"
        ),
        "a=1true,c=3true"
    );
    // A File value is read live as the same object each time.
    assert_eq!(
        eval(
            r,
            "{ const fd = new FormData(); fd.append('f', new File(['x'], 'x.txt')); const it = fd.values(); const a = it.next().value; a === fd.get('f') && it.next().done }"
        ),
        "true"
    );
    // The iterator is exhausted for good once done, even if entries are added later.
    assert_eq!(
        eval(r, "{ const fd = make([['a','1']]); const it = fd.entries(); it.next(); const d = it.next().done; fd.append('b', '2'); d && it.next().done }"),
        "true"
    );
}

#[test]
fn iterator_prototype_matches_webidl_default_iterator_objects() {
    let e = env("<!doctype html><html><body></body></html>");
    let r = &e.main;
    assert_eq!(eval(r, "typeof globalThis['FormData Iterator']"), "undefined");
    assert_eq!(eval(r, "Object.prototype.toString.call(new FormData().entries())"), "[object FormData Iterator]");
    assert_eq!(
        eval(
            r,
            "{ const proto = Object.getPrototypeOf(new FormData().entries()); const iteratorPrototype = Object.getPrototypeOf(Object.getPrototypeOf([][Symbol.iterator]())); Object.getPrototypeOf(proto) === iteratorPrototype && !proto.hasOwnProperty('constructor') && typeof proto.next === 'function' }"
        ),
        "true"
    );
    assert_eq!(eval(r, "FormData.prototype[Symbol.iterator] === FormData.prototype.entries"), "true");
    assert_eq!(eval(r, "{ const fd = new FormData(); fd.append('a', '1'); JSON.stringify([...fd]) }"), r#"[["a","1"]]"#);
}

#[test]
fn constructor_validates_the_submitter() {
    let e = env(FORM);
    let r = &e.main;
    eval(
        r,
        r#"const form = document.querySelector('#myform');
           const names = fd => [...fd].map(([k, v]) => k + '=' + v).join(',');
           const thrown = f => { try { f(); return 'no throw'; } catch (e) { return e.constructor.name + ':' + (e.name || ''); } }"#,
    );
    assert_eq!(eval(r, "names(new FormData(form, null))"), "n1=v1,n3=v3");
    assert_eq!(eval(r, "names(new FormData(undefined, undefined))"), "");
    // A non-null submitter that is not a submit button is a TypeError.
    assert_eq!(eval(r, "thrown(() => new FormData(form, document.querySelector('[name=n1]')))"), "TypeError:TypeError");
    assert_eq!(
        eval(r, "thrown(() => new FormData(form, document.querySelector('[name=resetter]')))"),
        "TypeError:TypeError"
    );
    assert_eq!(eval(r, "thrown(() => new FormData(form, document.body))"), "TypeError:TypeError");
    // A submit button owned by another form is a NotFoundError.
    assert_eq!(
        eval(r, "thrown(() => new FormData(form, document.querySelector('[name=unassociated]')))"),
        "DOMException:NotFoundError"
    );
    assert_eq!(
        eval(r, "thrown(() => new FormData(form, document.querySelector('[name=unassociated2]')))"),
        "DOMException:NotFoundError"
    );
    // Valid submitters contribute their entry in tree order.
    assert_eq!(
        eval(r, "names(new FormData(form, document.querySelector('[name=named]')))"),
        "n1=v1,named=GO,n3=v3"
    );
    assert_eq!(
        eval(r, "names(new FormData(form, document.querySelector('[name=outerNamed]')))"),
        "outerNamed=GO,n1=v1,n3=v3"
    );
    assert_eq!(eval(r, "names(new FormData(form, document.querySelector('#unnamed')))"), "n1=v1,n3=v3");
    // An image button that is not being activated submits (0, 0).
    assert_eq!(
        eval(r, "names(new FormData(form, document.querySelector('[name=namedImage]')))"),
        "n1=v1,namedImage.x=0,namedImage.y=0,n3=v3"
    );
    assert_eq!(eval(r, "names(new FormData(form, document.querySelector('#unnamedImage')))"), "n1=v1,x=0,y=0,n3=v3");
}

#[test]
fn image_button_submits_the_clicks_coordinate() {
    let e = env(FORM);
    let r = &e.main;
    eval(
        r,
        r#"const form = document.querySelector('#myform');
           const image = document.querySelector('[name=namedImage]');
           let seen = null;
           form.addEventListener('submit', e => { e.preventDefault(); const fd = new FormData(form, image); seen = fd.get('namedImage.x') + ',' + fd.get('namedImage.y'); });
           const rect = image.getBoundingClientRect();"#,
    );
    // A dispatched click with a position: offsets relative to the image.
    assert_eq!(
        eval(
            r,
            "seen = null; { const x = Math.ceil(rect.left) + 23, y = Math.ceil(rect.top) + 46;
               image.dispatchEvent(new MouseEvent('click', {clientX: x, clientY: y, bubbles: true}));
               seen === Math.floor(x - rect.left) + ',' + Math.floor(y - rect.top) }"
        ),
        "true"
    );
    // A constructed MouseEvent has integer coordinates (as in Chrome), and
    // the offset from the image's box floors.
    assert_eq!(
        eval(
            r,
            "seen = null; { const x = Math.ceil(rect.left) + 5.9, y = Math.ceil(rect.top) + 0.2;
               image.dispatchEvent(new MouseEvent('click', {clientX: x, clientY: y, bubbles: true}));
               seen === Math.floor(Math.floor(x) - rect.left) + ',' + Math.floor(Math.floor(y) - rect.top) }"
        ),
        "true"
    );
    // click() has no position: (0, 0).
    assert_eq!(eval(r, "seen = null; image.click(); seen"), "0,0");
    // Outside the activation the coordinate is (0, 0) again.
    assert_eq!(eval(r, "{ const fd = new FormData(form, image); fd.get('namedImage.x') + ',' + fd.get('namedImage.y') }"), "0,0");
    // Another submitter is unaffected by the image's coordinate.
    assert_eq!(
        eval(
            r,
            r#"{ let names = null; const button = document.querySelector('[name=named]');
                 const h = () => { names = [...new FormData(form, button)].map(([k]) => k).join(','); };
                 form.addEventListener('submit', h); button.click(); form.removeEventListener('submit', h); names }"#
        ),
        "n1,named,n3"
    );
}

#[test]
fn blobs_become_files_with_an_integer_last_modified() {
    let e = env("<!doctype html><html><body></body></html>");
    let r = &e.main;
    assert_eq!(
        eval(
            r,
            r#"{ const before = Date.now(); const fd = new FormData(); fd.append('k', new Blob(['ab']), 'blank.txt'); fd.append('b', new Blob());
                 const f = fd.get('k'), g = fd.get('b');
                 [f instanceof File, f.name, f.size, Number.isInteger(f.lastModified), f.lastModified >= before, f.lastModified <= Date.now(), g.name, Number.isInteger(g.lastModified)].join(',') }"#
        ),
        "true,blank.txt,2,true,true,true,blob,true"
    );
    // A File keeps its own (possibly fractional) lastModified when not renamed.
    assert_eq!(
        eval(r, "{ const fd = new FormData(); const f = new File([], 'a', {lastModified: 1.5}); fd.append('k', f); fd.get('k') === f && f.lastModified === 1.5 }"),
        "true"
    );
}
