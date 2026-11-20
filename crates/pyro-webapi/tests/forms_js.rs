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
    /// The navigations so far: a submitted form navigates from a task
    /// (HTML "plan to navigate"), so the queued tasks run first.
    fn navigations(&self) -> Vec<String> {
        self.run_tasks();
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
fn form_reset_moves_selection_only_for_changed_text() {
    let e = env("<form id=f><input value='ab😀'><textarea>ab😀</textarea></form>");
    assert_eq!(e.eval("const f=document.querySelector('form'); Array.from(f.elements, el=>{ el.value='longer'; f.reset(); const end=el.selectionEnd; el.setSelectionRange(1,2,'backward'); f.reset(); return [end,el.selectionStart,el.selectionEnd,el.selectionDirection].join(':') }).join('|')"), "4:1:2:backward|4:1:2:backward");
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

/// `select` fires as a queued task, and only when a selection member changes
/// the selection (html/semantics/forms/textfieldselection/select-event.html);
/// selectionStart/End of a control without a selection throw.
#[test]
fn select_event_is_queued_only_when_the_selection_changes() {
    let e = env(r#"<body><input id=t value=foobar><input id=c type=color></body>"#);
    e.eval(
        "const t = document.getElementById('t'); var fired = [];
         t.addEventListener('select', ev => fired.push(ev.isTrusted + ':' + ev.bubbles + ':' + ev.cancelable));
         t.setSelectionRange(0, 0);",
    );
    e.run_tasks();
    assert_eq!(e.eval("fired.length = 0; t.selectionStart = 1; String(fired.length)"), "0");
    e.run_tasks();
    assert_eq!(e.eval("String(fired)"), "true:true:false");
    // Setting the same selection again, select() twice, setSelectionRange twice: one event each.
    assert_eq!(e.eval("fired.length = 0; t.selectionStart = 1; t.select(); t.select(); t.selectionDirection = 'backward'; t.selectionDirection = 'backward'; String(fired.length)"), "0");
    e.run_tasks();
    assert_eq!(e.eval("String(fired.length)"), "2");
    assert_eq!(
        e.eval(
            "const c = document.getElementById('c'); const out = [];
             try { c.selectionStart = 1; } catch (err) { out.push(err.name); }
             try { c.selectionEnd = 1; } catch (err) { out.push(err.name); }
             out.join()"
        ),
        "InvalidStateError,InvalidStateError"
    );
    e.page.dispose();
}

/// `<input type=color>` takes any CSS color and serializes it in its
/// `colorspace`, with its alpha when it has `alpha`
/// (html/semantics/forms/the-input-element/color{,.window}.html).
#[test]
fn color_input_value_follows_alpha_and_colorspace() {
    let e = env("<body></body>");
    assert_eq!(
        e.eval(
            "const c = document.createElement('input'); c.type = 'color'; const out = [c.value, c.colorSpace, c.alpha];
             c.value = ' crimson '; out.push(c.value);
             c.alpha = true; c.value = 'rgb(1 1 1 / 0.5)'; out.push(c.value);
             c.setAttribute('colorspace', 'DISPLAY-P3'); out.push(c.colorSpace, c.value);
             c.alpha = false; out.push(c.value);
             c.value = 'nope'; out.push(c.value);
             out.join('|')"
        ),
        "#000000|limited-srgb|false|#dc143c|color(srgb 0.003922 0.003922 0.003922 / 0.501961)|display-p3|color(display-p3 0.003922 0.003922 0.003922 / 0.501961)|color(display-p3 0.003922 0.003922 0.003922)|color(display-p3 0 0 0)"
    );
    e.page.dispose();
}

/// A `ValidityState` reports mismatches of a disabled or read-only control
/// too, though it is barred from constraint validation; only a mutable one
/// suffers from being missing (html/semantics/forms/constraints/form-validation-validity-*).
#[test]
fn validity_flags_of_barred_controls() {
    let e = env("<body></body>");
    assert_eq!(
        e.eval(
            "const i = document.createElement('input'); i.pattern = 'a'; i.value = 'b'; i.required = true; i.disabled = true;
             const t = document.createElement('textarea'); t.required = true; t.readOnly = true;
             [i.willValidate, i.validity.patternMismatch, i.validity.valid, i.checkValidity(), i.validationMessage === '',
              t.validity.valueMissing, t.validity.valid].join()"
        ),
        "false,true,false,true,true,false,true"
    );
    e.page.dispose();
}

/// The `autocomplete` IDL attribute of form controls returns the attribute's
/// autofill value (html/semantics/forms/the-form-element/form-autocomplete.html).
#[test]
fn autocomplete_returns_the_autofill_value() {
    let e = env("<body></body>");
    assert_eq!(
        e.eval(
            "const out = []; const t = document.createElement('textarea');
             for (const v of [' NAME\t', 'section-A shipping HOME tel', 'off', 'work name', 'username webauthn']) {
               t.setAttribute('autocomplete', v); out.push(t.autocomplete);
             }
             const h = document.createElement('input'); h.type = 'hidden'; h.setAttribute('autocomplete', 'on');
             out.push(h.autocomplete, h.getAttribute('autocomplete'));
             out.join('|')"
        ),
        "name|section-a shipping home tel|off||username webauthn||on"
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
    // Each submission's task runs before the next: a later submission of
    // the same form would replace its planned navigation.
    let mut navigations = e.navigations();
    e.eval("document.getElementById('s2').focus()");
    e.key("Enter", "\r");
    navigations.extend(e.navigations());
    assert_eq!(
        navigations,
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

#[test]
fn labels_follow_for_changes_insertions_removals_nesting_and_shadow_trees() {
    let e = env(r#"<body>
        <label id=l1 for=a>A1</label><label id=l2><input id=a></label><label id=l3 for=a>A3</label>
        <label id=l4><label id=l5><input id=b></label></label>
        <div id=host></div><input id=c>
    </body>"#);
    // Elements held in variables: ids change below, so named access cannot.
    let ids = "(c) => [...c].map(l => l.id).join()";
    assert_eq!(
        e.eval(&format!("const ids = {ids}; const A = a, B = b, C = c, L1 = l1, L2 = l2, L3 = l3, L4 = l4, L5 = l5, H = host; ids(A.labels)")),
        "l1,l2,l3"
    );
    assert_eq!(e.eval("ids(B.labels)"), "l4,l5");
    assert_eq!(e.eval("A.labels === A.labels"), "true");
    // Dynamic `for`: retargeted, same value, emptied, removed.
    assert_eq!(e.eval("L1.htmlFor = 'c'; [ids(A.labels), ids(C.labels)].join('|')"), "l2,l3|l1");
    assert_eq!(e.eval("L1.setAttribute('for', 'c'); ids(C.labels)"), "l1");
    assert_eq!(e.eval("L1.htmlFor = ''; [ids(A.labels), ids(C.labels), String(L1.control)].join('|')"), "l2,l3||null");
    assert_eq!(e.eval("L2.htmlFor = 'c'; [ids(A.labels), ids(C.labels)].join('|')"), "l3|l2");
    assert_eq!(e.eval("L2.removeAttribute('for'); ids(A.labels)"), "l2,l3");
    // The control's id changes.
    assert_eq!(e.eval("A.id = 'z'; [ids(A.labels), String(L3.control)].join('|')"), "l2|null");
    assert_eq!(e.eval("A.id = 'a'; ids(A.labels)"), "l2,l3");
    // Label insertion and removal, parsed and created.
    assert_eq!(
        e.eval("document.body.insertAdjacentHTML('afterbegin', '<label id=l0 for=a>A0</label>'); ids(A.labels)"),
        "l0,l2,l3"
    );
    assert_eq!(
        e.eval("const L6 = document.createElement('label'); L6.htmlFor = 'a'; L6.id = 'l6'; document.body.append(L6); ids(A.labels)"),
        "l0,l2,l3,l6"
    );
    assert_eq!(e.eval("document.getElementById('l0').remove(); L6.remove(); ids(A.labels)"), "l2,l3");
    assert_eq!(e.eval("L6.htmlFor = 'b'; document.body.prepend(L6); ids(B.labels)"), "l6,l4,l5");
    // The wrapping label pair: the inner control moves out and back.
    assert_eq!(e.eval("document.body.append(B); ids(B.labels)"), "l6");
    assert_eq!(e.eval("L5.append(B); ids(B.labels)"), "l6,l4,l5");
    // A hidden input has no labels; back to text it has them again.
    assert_eq!(
        e.eval("B.type = 'hidden'; [B.labels === null || B.labels.length === 0, String(L6.control)].join('|')"),
        "true|null"
    );
    assert_eq!(e.eval("B.type = 'text'; ids(B.labels)"), "l6,l4,l5");
    // Shadow trees: `for` resolves in the label's own tree.
    assert_eq!(
        e.eval(
            "const sr = H.attachShadow({mode: 'open'});
             sr.innerHTML = '<label id=s1 for=a>S</label><label id=s2><input id=a></label>';
             const SA = sr.querySelector('input'), S1 = sr.getElementById('s1');
             [ids(A.labels), ids(SA.labels), S1.control === SA].join('|')"
        ),
        "l2,l3|s1,s2|true"
    );
    assert_eq!(e.eval("document.body.append(S1); [ids(A.labels), ids(SA.labels)].join('|')"), "l2,l3,s1|s2");
    assert_eq!(e.eval("sr.prepend(S1); [ids(A.labels), ids(SA.labels)].join('|')"), "l2,l3|s1,s2");
    assert_eq!(e.eval("H.remove(); ids(SA.labels)"), "s1,s2");
    // A detached tree still answers.
    assert_eq!(
        e.eval("const t = document.createElement('div'); t.innerHTML = '<label id=d1 for=q>1</label><label id=d2><input id=q></label>'; ids(t.querySelector('input').labels)"),
        "d1,d2"
    );
}
