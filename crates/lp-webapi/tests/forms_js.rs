//! HTML forms as a page sees them: `<form method=dialog>`, the `formdata`
//! event, form reset, `select.add`, `setRangeText`, implicit submission,
//! keyboard default actions of form controls, and the numeric/date value
//! and validation members (html/semantics/interactive-elements/
//! the-dialog-element/dialog-form-submission*, html/semantics/forms/
//! form-submission-0/{FormDataEvent.window,constructing-form-data-set,
//! implicit-submission.optional}, resetting-a-form, the-select-element/
//! select-add, textfieldselection-setRangeText, the-input-element).

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

/// Records navigations and keeps posted tasks until asked to run them.
#[derive(Default)]
struct Hooks {
    navigations: RefCell<Vec<String>>,
    tasks: RefCell<Vec<Box<dyn FnOnce()>>>,
}
impl PageHooks for Hooks {
    fn post_task(&self, task: Box<dyn FnOnce()>) {
        self.tasks.borrow_mut().push(task);
    }
    fn navigate(&self, request: NavigationRequest) {
        self.navigations.borrow_mut().push(format!("{} {}", request.method, request.url));
    }
}

struct Env {
    _engine: Engine,
    page: Rc<PageDom>,
    main: Realm,
    hooks: Rc<Hooks>,
}

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
    let hooks = Rc::new(Hooks::default());
    page.set_hooks(Some(hooks.clone()));
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
    {
        let s = realm.scope().unwrap();
        install(&s, page.clone()).unwrap();
    }
    Env { _engine: engine, page, main: realm, hooks }
}

impl Env {
    fn eval(&self, src: &str) -> String {
        let s = self.main.scope().unwrap();
        match s.eval(src, Some("test.js"), 1) {
            Ok(v) => v.to_string().unwrap(),
            Err(e) => format!("THROW {}", e.details(&s).text()),
        }
    }
    fn navigations(&self) -> Vec<String> {
        std::mem::take(&mut *self.hooks.navigations.borrow_mut())
    }
    fn run_tasks(&self) {
        loop {
            let tasks = std::mem::take(&mut *self.hooks.tasks.borrow_mut());
            if tasks.is_empty() {
                break;
            }
            for t in tasks {
                t();
            }
        }
    }
    fn key(&self, key: &str, text: &str) {
        let k = input::KeyInput { key: key.into(), code: key.into(), text: text.into(), ..Default::default() };
        let kind = if text.is_empty() { input::KeyKind::RawKeyDown } else { input::KeyKind::KeyDown };
        input::dispatch_key(&self.page, kind, &k);
        input::dispatch_key(&self.page, input::KeyKind::KeyUp, &k);
    }
}

#[test]
fn dialog_method_closes_the_dialog() {
    let e = env(r#"<body><dialog id=d><form id=f method=dialog><button id=b value=default>OK</button>
        <input type=submit id=s value=Goodbye></form>
        <form method=post><input type=submit id=fm formmethod=dialog value=fm></form>
        <div id=host></div></dialog></body>"#);
    assert_eq!(
        e.eval(
            "const d = document.getElementById('d'), b = document.getElementById('b');
             window.closes = 0; d.addEventListener('close', () => closes++);
             d.showModal(); b.click();
             const r1 = [d.open, d.returnValue].join();
             d.returnValue = 'initial'; d.showModal(); b.removeAttribute('value'); b.click();
             const r2 = [d.open, d.returnValue].join();
             d.show(); document.getElementById('fm').click();
             const r3 = [d.open, d.returnValue].join();
             // Closing it while submitting stops the submission.
             d.returnValue = ''; d.show(); b.value = 'sushi';
             document.getElementById('f').onsubmit = () => d.close();
             b.click();
             [r1, r2, r3, d.open, JSON.stringify(d.returnValue), closes].join('|')"
        ),
        "false,default|false,initial|false,fm|false|\"\"|0"
    );
    e.run_tasks();
    assert_eq!(e.eval("closes"), "4");
    // No navigation for any of them; a form in a shadow root inside the
    // dialog does not close it.
    assert_eq!(e.navigations(), Vec::<String>::new());
    assert_eq!(
        e.eval(
            "const root = document.getElementById('host').attachShadow({mode: 'open'});
             root.innerHTML = '<form method=dialog><button>x</button></form>';
             d.showModal(); root.querySelector('button').click(); d.open"
        ),
        "true"
    );
    e.page.dispose();
}

#[test]
fn formdata_event() {
    let e = env(r#"<body><form id=f action=/go><input name=n1 value=v1></form></body>"#);
    assert_eq!(
        e.eval(
            "const fd = new FormData();
             const errors = [];
             for (const args of [[], ['foo'], ['foo', null], ['foo', {}], ['foo', {formData: 'bar'}]]) {
               try { new FormDataEvent(...args); errors.push('ok'); } catch (err) { errors.push(err.name); }
             }
             const ev = new FormDataEvent('bar', {formData: fd, bubbles: true});
             [typeof FormDataEvent, errors.join(), ev.formData === fd, ev.bubbles, ev instanceof Event].join('|')"
        ),
        "function|TypeError,TypeError,TypeError,TypeError,TypeError|true|true|true"
    );
    assert_eq!(
        e.eval(
            "const form = document.getElementById('f'); window.seen = [];
             document.addEventListener('formdata', ev => {
               seen.push([ev.isTrusted, ev.bubbles, ev.cancelable, ev.formData.get('n1')].join());
               ev.formData.append('a\\nb', 'c\\rd');
               try { new FormData(form); } catch (err) { seen.push(err.name); }
             });
             const out = new FormData(form);
             [seen.join('|'), JSON.stringify([...out])].join(' ')"
        ),
        r#"true,true,false,v1|InvalidStateError [["n1","v1"],["a\nb","c\rd"]]"#
    );
    // Entries added by a listener are submitted.
    e.eval("form.submit()");
    assert_eq!(e.navigations(), vec!["GET http://localhost:5173/go?n1=v1&a%0D%0Ab=c%0D%0Ad".to_owned()]);
    e.page.dispose();
}

#[test]
fn form_reset() {
    let e = env(
        r#"<body><form id=f><input id=a value=x><select id=s><option>1</option><option selected>2</option></select></form>
        <input id=outside form=f value=o><select id=os form=f><option selected>p</option><option>q</option></select></body>"#,
    );
    assert_eq!(
        e.eval(
            "const a = document.getElementById('a'), o = document.getElementById('outside'), os = document.getElementById('os');
             a.value = 'changed'; a.setCustomValidity('bad'); o.value = 'changed'; os.selectedIndex = 1;
             document.getElementById('s').selectedIndex = 0;
             document.getElementById('f').reset();
             [a.value, a.validationMessage, o.value, os.value, document.getElementById('s').value].join()"
        ),
        "x,bad,o,p,2"
    );
    e.page.dispose();
}

#[test]
fn select_add_and_set_range_text() {
    let e = env(r#"<body><select id=s><option>A</option><option>B</option></select><input id=t value=foobar></body>"#);
    assert_eq!(
        e.eval(
            "const s = document.getElementById('s');
             s.add(new Option('first', 'f'), 0); s.add(new Option('last', 'l'), null); s.add(new Option('mid', 'm'), s.options[2]);
             let thrown = '';
             try { s.add(document.body); } catch (err) { thrown += err.name; }
             try { s.add(new Option('x'), document.body); } catch (err) { thrown += ',' + err.name; }
             [[...s.options].map(o => o.value).join(' '), thrown].join('|')"
        ),
        "f A m B l|TypeError,NotFoundError"
    );
    assert_eq!(
        e.eval(
            "const t = document.getElementById('t'); const out = [];
             t.setSelectionRange(0, 3); t.setRangeText('foobar2');
             out.push(t.value, t.selectionStart, t.selectionEnd);
             t.value = 'foobar'; t.setSelectionRange(0, 5); t.setRangeText('', 3, 6, 'preserve');
             out.push(t.value, t.selectionStart, t.selectionEnd);
             t.value = 'foobar'; t.setRangeText('xx', 1, 2, 'select'); out.push(t.selectionStart, t.selectionEnd);
             t.setRangeText('yy', 0, 0, 'end'); out.push(t.selectionStart, t.selectionEnd);
             try { t.setRangeText('z', 2, 1); } catch (err) { out.push(err.name); }
             const n = document.createElement('input'); n.type = 'number';
             try { n.setRangeText('1'); } catch (err) { out.push(err.name); }
             t.maxLength = 2; t.setRangeText('long text', 0, 0); out.push(t.value.length);
             out.join()"
        ),
        "foobar2bar,0,7,foo,0,3,1,3,2,2,IndexSizeError,InvalidStateError,18"
    );
    e.page.dispose();
}

#[test]
fn implicit_submission_follows_chrome() {
    let e = env(r#"<body>
        <form id=disabled action=/a><input id=t1><button disabled>go</button></form>
        <form id=two action=/b><input id=t2><input id=t3></form>
        <form id=one action=/c><input id=t4><input type=checkbox id=c1></form>
        <form id=buttons action=/d><input id=t5><input type=submit id=s1 name=s value=one><input type=submit id=s2 name=s value=two></form>
        <form id=check action=/e><input type=checkbox id=c2></form>
        </body>"#);
    for id in ["t1", "t2", "t3", "c2"] {
        e.eval(&format!("document.getElementById('{id}').focus()"));
        e.key("Enter", "\r");
    }
    assert_eq!(e.navigations(), Vec::<String>::new(), "a disabled default button or two fields block it");
    e.eval("document.getElementById('t4').focus()");
    e.key("Enter", "\r");
    assert_eq!(e.navigations(), vec!["GET http://localhost:5173/c?".to_owned()]);
    e.eval("document.getElementById('t5').focus()");
    e.key("Enter", "\r");
    e.eval("document.getElementById('s2').focus()");
    e.key("Enter", "\r");
    assert_eq!(
        e.navigations(),
        vec!["GET http://localhost:5173/d?s=one".to_owned(), "GET http://localhost:5173/d?s=two".to_owned()],
        "the default button, then the focused submit button"
    );
    e.page.dispose();
}

#[test]
fn keyboard_actions_of_controls() {
    let e = env(r#"<body><input type=number id=n value=5 step=2>
        <input type=range id=r min=0 max=50 step=5 value=10>
        <input type=radio name=g id=g1 checked><input type=radio name=g id=g2 disabled><input type=radio name=g id=g3>
        <select id=s><option>Apple</option><option disabled>Banana</option><option>Cherry</option><option>Citrus</option></select>
        </body>"#);
    e.eval(
        "window.log = []; for (const el of document.querySelectorAll('input, select'))
           for (const t of ['input', 'change']) el.addEventListener(t, () => log.push(el.id + ':' + t));
         document.getElementById('n').focus();",
    );
    e.key("ArrowUp", "");
    e.key("ArrowUp", "");
    e.key("ArrowDown", "");
    assert_eq!(e.eval("document.getElementById('n').value"), "7");
    e.eval("document.getElementById('r').focus()");
    e.key("ArrowRight", "");
    let v1 = e.eval("document.getElementById('r').value");
    e.key("End", "");
    let v2 = e.eval("document.getElementById('r').value");
    e.key("PageDown", "");
    let v3 = e.eval("document.getElementById('r').value");
    e.key("Home", "");
    e.key("Home", "");
    assert_eq!([v1, v2, v3, e.eval("document.getElementById('r').value")].join(","), "15,50,45,0");
    e.eval("document.getElementById('g1').focus()");
    e.key("ArrowDown", "");
    assert_eq!(
        e.eval("[document.activeElement.id, g1.checked, g3.checked].join()"),
        "g3,false,true",
        "the disabled radio is skipped"
    );
    e.key("ArrowRight", "");
    assert_eq!(e.eval("[document.activeElement.id, g1.checked].join()"), "g1,true", "wraps around");
    e.eval("document.getElementById('s').focus()");
    e.key("ArrowDown", "");
    let s1 = e.eval("s.value");
    e.key("End", "");
    let s2 = e.eval("s.value");
    e.key("a", "a");
    let s3 = e.eval("s.value");
    e.key("c", "c");
    let s4 = e.eval("s.value");
    assert_eq!([s1, s2, s3, s4].join(","), "Cherry,Citrus,Apple,Cherry");
    assert_eq!(
        e.eval("log.join(' ')"),
        "n:input n:change n:input n:change n:input n:change r:input r:change r:input r:change r:input r:change \
         r:input r:change g3:input g3:change g1:input g1:change s:input s:change s:input s:change s:input s:change \
         s:input s:change"
    );
    e.page.dispose();
}

#[test]
fn numeric_value_members_and_validation() {
    let e = env(r#"<body><form id=f><input id=d type=date value=2019-12-10><input id=t type=time>
        <input id=p pattern="[0-9]+" value=abc><input id=x name=x required><input id=m maxlength=4></form></body>"#);
    assert_eq!(
        e.eval(
            "const d = document.getElementById('d'), t = document.getElementById('t'), out = [];
             out.push(d.valueAsDate instanceof Date, d.valueAsDate.toISOString(), d.valueAsNumber);
             d.valueAsDate = new Date('2016-02-29T00:00:00.000Z'); out.push(d.value);
             d.stepUp(3); out.push(d.value);
             t.valueAsDate = new Date('1970-01-01T23:59:00.000Z'); out.push(t.value);
             d.valueAsDate = null; out.push(JSON.stringify(d.value), String(d.valueAsDate));
             try { d.valueAsNumber = Infinity; } catch (err) { out.push(err.name); }
             const text = document.getElementById('x');
             try { text.valueAsDate = new Date(); } catch (err) { out.push(err.name); }
             try { text.stepUp(); } catch (err) { out.push(err.name); }
             try { d.valueAsDate = 5; } catch (err) { out.push(err.name); }
             out.push(String(text.valueAsDate), text.valueAsNumber);
             out.join()"
        ),
        "true,2019-12-10T00:00:00.000Z,1575936000000,2016-02-29,2016-03-03,23:59,\"\",null,TypeError,\
         InvalidStateError,InvalidStateError,TypeError,null,NaN"
    );
    assert_eq!(
        e.eval(
            "const p = document.getElementById('p');
             [p.validity.patternMismatch, p.validationMessage, p.checkValidity()].join()"
        ),
        "true,Please match the requested format.,false"
    );
    // Interactive validation focuses the first invalid control whose
    // `invalid` event was not canceled.
    assert_eq!(
        e.eval(
            "p.addEventListener('invalid', ev => ev.preventDefault());
             document.getElementById('f').requestSubmit(); document.activeElement.id"
        ),
        "x"
    );
    assert_eq!(e.navigations(), Vec::<String>::new());
    // A user insertion past maxlength is cut to the room left.
    e.eval("document.getElementById('m').focus()");
    input::insert_text(&e.page, "12345678");
    assert_eq!(e.eval("document.getElementById('m').value"), "1234");
    e.page.dispose();
}

#[test]
fn numeric_edge_cases_and_argument_order() {
    let e = env(r#"<body><input type=month id=mo value=2020-01><input type=range id=r step=1e-320 value=50>
        <input type=number id=n><select id=s readonly><option>a</option><option>b</option></select></body>"#);
    assert_eq!(
        e.eval(
            "const out = [], n = document.getElementById('n'), mo = document.getElementById('mo');
             // A Date whose valueOf leaves the representable range empties the value.
             const d = new Date(0); d.valueOf = () => 1e300;
             mo.valueAsDate = d; out.push(JSON.stringify(mo.value));
             mo.valueAsDate = new Date('2021-07-15T00:00:00Z'); out.push(mo.value);
             // A step too small to count in doubles does not snap the value.
             out.push(document.getElementById('r').value);
             for (const x of [1e21, 1.5e-7, 0.5, -1.5, 123, 1e-6]) { n.valueAsNumber = x; out.push(n.value); }
             // The arguments convert before the type check.
             const c = document.createElement('input'); c.type = 'color';
             try { c.setRangeText('a', 1); } catch (err) { out.push(err.name); }
             try { c.setRangeText('a', 0, 1); } catch (err) { out.push(err.name); }
             out.join()"
        ),
        "\"\",2021-07,50,1e+21,1.5e-7,0.5,-1.5,123,0.000001,TypeError,InvalidStateError"
    );
    // `readonly` does not apply to a select: the arrows still change it.
    e.eval("document.getElementById('s').focus()");
    e.key("ArrowDown", "");
    assert_eq!(e.eval("document.getElementById('s').value"), "b");
    e.page.dispose();
}
