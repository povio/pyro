//! Custom element reactions beyond connection (`attributeChangedCallback`
//! with `observedAttributes`, `adoptedCallback`), upgrades of clones,
//! failed constructors, form-associated custom elements (`ElementInternals`)
//! and `CustomStateSet` / `:state()`, through the bindings on the real
//! JavaScriptCore.

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
    vp.width = 1024;
    vp.height = 768;
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

const PAGE: &str = "<!doctype html><html><body><div id=host></div></body></html>";

/// A class logging its observed attribute changes into `log`.
const LOGGING: &str = "var log = []; \
    class Logged extends HTMLElement { \
      static get observedAttributes() { log.push('read'); return ['title', 'r']; } \
      attributeChangedCallback(name, oldValue, newValue, ns) { log.push([name, oldValue, newValue, ns].join('|')); } \
      connectedCallback() { log.push('connected'); } \
    }; customElements.define('x-logged', Logged); \
    var take = () => { const l = log.join(); log = []; return l; };";

#[test]
fn attribute_changed_callback_runs_for_observed_attributes_only() {
    let e = env(PAGE);
    let r = &e.main;
    assert_eq!(eval(r, &format!("{LOGGING} take()")), "read");
    assert_eq!(eval(r, "var el = document.createElement('x-logged'); take()"), "");
    assert_eq!(eval(r, "el.setAttribute('title', 'a'); take()"), "title||a|");
    // A change to the same value is still a change.
    assert_eq!(eval(r, "el.setAttribute('title', 'a'); take()"), "title|a|a|");
    assert_eq!(eval(r, "el.setAttribute('id', 'x'); el.dataset.foo = '1'; take()"), "");
    assert_eq!(eval(r, "el.removeAttribute('title'); take()"), "title|a||");
    assert_eq!(eval(r, "el.toggleAttribute('title'); el.toggleAttribute('title'); take()"), "title|||,title|||");
    assert_eq!(
        eval(r, "el.setAttributeNS('http://www.w3.org/2000/svg', 'r', '5'); take()"),
        "r||5|http://www.w3.org/2000/svg"
    );
    assert_eq!(
        eval(
            r,
            "const a = document.createAttribute('title'); a.value = 'v'; el.setAttributeNode(a); a.value = 'w'; take()"
        ),
        "title||v|,title|v|w|"
    );
    // observedAttributes was read once, at definition.
    assert_eq!(eval(r, "document.createElement('x-logged').setAttribute('title', 'z'); take()"), "title||z|");
    // Without attributeChangedCallback observedAttributes is not even read.
    assert_eq!(
        eval(r, "var reads = 0; customElements.define('x-quiet', class extends HTMLElement { static get observedAttributes() { reads++; return ['a']; } }); document.createElement('x-quiet').setAttribute('a', '1'); reads"),
        "0"
    );
}

#[test]
fn upgrade_reports_existing_observed_attributes_before_connected() {
    let e = env("<!doctype html><html><body><x-logged title=t r=1 other=o></x-logged></body></html>");
    let r = &e.main;
    assert_eq!(eval(r, &format!("{LOGGING} take()")), "read,title||t|,r||1|,connected");
    // Inserting markup with innerHTML upgrades as it connects.
    assert_eq!(
        eval(r, "document.body.insertAdjacentHTML('beforeend', '<x-logged title=q></x-logged>'); take()"),
        "title||q|,connected"
    );
}

#[test]
fn adopted_callback_gets_the_old_and_new_documents() {
    let e = env(PAGE);
    let r = &e.main;
    eval(
        r,
        "var seen = []; customElements.define('x-adopt', class extends HTMLElement { \
           adoptedCallback(o, n) { seen.push(o === document, n === other, this.ownerDocument === other); } }); \
         var other = document.implementation.createHTMLDocument('t'); var el = document.createElement('x-adopt');",
    );
    assert_eq!(eval(r, "other.adoptNode(el); seen.join()"), "true,true,true");
    assert_eq!(eval(r, "seen = []; other.adoptNode(el); seen.length"), "0");
    // Inserting into another document adopts first.
    assert_eq!(eval(r, "seen = []; document.body.append(el); seen.length"), "3");
}

#[test]
fn a_constructor_that_throws_leaves_a_failed_element_never_upgraded_again() {
    let e = env("<!doctype html><html><body><x-bad id=b></x-bad></body></html>");
    let r = &e.main;
    eval(r, "var runs = 0; window.onerror = () => true; customElements.define('x-bad', class extends HTMLElement { constructor() { super(); runs++; throw new Error('no'); } });");
    assert_eq!(eval(r, "runs"), "1");
    assert_eq!(eval(r, "const b = document.getElementById('b'); b.remove(); document.body.append(b); customElements.upgrade(document.body); [runs, b.matches(':defined')].join()"), "1,false");
}

#[test]
fn clones_of_defined_elements_are_upgraded_by_the_cloning_call() {
    let e = env(PAGE);
    let r = &e.main;
    eval(r, "customElements.define('x-clone', class extends HTMLElement { constructor() { super(); this.made = true; } }); var src = document.createElement('div'); src.innerHTML = '<x-clone></x-clone>';");
    assert_eq!(eval(r, "const c = src.cloneNode(true); [c.firstChild.made, c.isConnected].join()"), "true,false");
    assert_eq!(eval(r, "document.importNode(src, true).firstChild.made"), "true");
    assert_eq!(eval(r, "src.firstChild.cloneNode().made"), "true");
}

const FACE: &str = "class Field extends HTMLElement { \
      static formAssociated = true; \
      constructor() { super(); this.internals = this.attachInternals(); this.log = []; } \
      formAssociatedCallback(form) { this.log.push('form:' + (form ? form.id : null)); } \
      formDisabledCallback(d) { this.log.push('disabled:' + d); } \
      formResetCallback() { this.log.push('reset'); } \
    }; customElements.define('x-field', Field);";

#[test]
fn form_associated_custom_elements_submit_their_value() {
    let e = env("<!doctype html><html><body><form id=f><input name=a value=1><x-field name=b id=fld></x-field></form></body></html>");
    let r = &e.main;
    eval(r, FACE);
    assert_eq!(eval(r, "typeof ElementInternals + ',' + (fld.internals instanceof ElementInternals)"), "function,true");
    assert_eq!(eval(r, "[fld.internals.form === f, fld.log.join()].join()"), "true,form:f");
    assert_eq!(eval(r, "[...new FormData(f)].map(e => e.join('=')).join('&')"), "a=1");
    eval(r, "fld.internals.setFormValue('two')");
    assert_eq!(eval(r, "[...new FormData(f)].map(e => e.join('=')).join('&')"), "a=1&b=two");
    // A FormData value contributes its own entries, names included.
    eval(r, "const d = new FormData(); d.append('x', '1'); d.append('y', '2'); fld.internals.setFormValue(d);");
    assert_eq!(eval(r, "[...new FormData(f)].map(e => e.join('=')).join('&')"), "a=1&x=1&y=2");
    eval(r, "fld.internals.setFormValue(null)");
    assert_eq!(eval(r, "[...new FormData(f)].map(e => e.join('=')).join('&')"), "a=1");
    // Listed in form.elements, labelable.
    assert_eq!(eval(r, "[...f.elements].map(e => e.localName).join()"), "input,x-field");
    assert_eq!(eval(r, "const l = document.createElement('label'); l.htmlFor = 'fld'; document.body.append(l); [l.control === fld, fld.internals.labels.length].join()"), "true,1");
}

#[test]
fn form_associated_callbacks_follow_owner_disabled_state_and_reset() {
    let e =
        env("<!doctype html><html><body><form id=f></form><form id=g></form><fieldset id=fs></fieldset></body></html>");
    let r = &e.main;
    eval(r, FACE);
    eval(r, "var x = document.createElement('x-field');");
    assert_eq!(eval(r, "f.append(x); x.log.join()"), "form:f");
    assert_eq!(
        eval(r, "x.log = []; x.setAttribute('form', 'g'); [x.log.join(), x.internals.form.id].join()"),
        "form:g,g"
    );
    assert_eq!(eval(r, "x.log = []; x.remove(); x.removeAttribute('form'); x.log.join()"), "form:null");
    assert_eq!(
        eval(r, "x.log = []; fs.append(x); fs.disabled = true; [x.log.join(), x.matches(':disabled')].join()"),
        "disabled:true,true"
    );
    assert_eq!(eval(r, "x.log = []; fs.disabled = false; x.disabled = true; x.log.join()"), "disabled:false");
    assert_eq!(eval(r, "x.log = []; x.setAttribute('disabled', ''); x.log.join()"), "disabled:true");
    assert_eq!(
        eval(r, "x.log = []; x.removeAttribute('disabled'); g.append(x); g.reset(); x.log.join()"),
        "disabled:false,form:g,reset"
    );
}

#[test]
fn element_internals_validation() {
    let e = env("<!doctype html><html><body><form id=f><x-field id=fld></x-field></form></body></html>");
    let r = &e.main;
    eval(r, FACE);
    let i = "fld.internals";
    assert_eq!(
        eval(r, &format!("[{i}.willValidate, {i}.validity.valid, {i}.checkValidity(), f.checkValidity()].join()")),
        "true,true,true,true"
    );
    assert_eq!(
        eval(r, &format!("try {{ {i}.setValidity({{valueMissing: true}}); 'no' }} catch (e) {{ e.name }}")),
        "TypeError"
    );
    eval(r, &format!("var invalid = 0; fld.addEventListener('invalid', () => invalid++); {i}.setValidity({{valueMissing: true}}, 'Fill me')"));
    assert_eq!(
        eval(r, &format!("[{i}.validity.valid, {i}.validity.valueMissing, {i}.validationMessage, {i}.checkValidity(), invalid, f.checkValidity(), fld.matches(':invalid'), f.matches(':invalid')].join()")),
        "false,true,Fill me,false,1,false,true,true"
    );
    eval(r, &format!("{i}.setValidity({{}})"));
    assert_eq!(eval(r, &format!("[{i}.validity.valid, {i}.validationMessage === ''].join()")), "true,true");
    // An anchor must be a shadow-including descendant.
    assert_eq!(
        eval(
            r,
            &format!(
                "try {{ {i}.setValidity({{customError: true}}, 'm', document.body); 'no' }} catch (e) {{ e.name }}"
            )
        ),
        "NotFoundError"
    );
    // A readonly element is barred from validation.
    assert_eq!(eval(r, &format!("fld.setAttribute('readonly', ''); {i}.willValidate")), "false");
}

#[test]
fn attach_internals_rules() {
    let e = env(PAGE);
    let r = &e.main;
    let name = |src: &str| eval(r, &format!("try {{ {src}; 'ok' }} catch (e) {{ e.name }}"));
    assert_eq!(name("document.createElement('div').attachInternals()"), "NotSupportedError");
    assert_eq!(name("document.createElement('x-undefined').attachInternals()"), "NotSupportedError");
    eval(r, "customElements.define('x-plain', class extends HTMLElement {}); customElements.define('x-closed', class extends HTMLElement { static disabledFeatures = ['internals']; });");
    assert_eq!(name("var p = document.createElement('x-plain'); var pi = p.attachInternals()"), "ok");
    assert_eq!(name("p.attachInternals()"), "NotSupportedError");
    assert_eq!(name("document.createElement('x-closed').attachInternals()"), "NotSupportedError");
    // Form members need a form-associated element; the rest work.
    assert_eq!(name("pi.form"), "NotSupportedError");
    assert_eq!(name("pi.setFormValue('a')"), "NotSupportedError");
    assert_eq!(eval(r, "pi.role = 'button'; pi.ariaLabel = 'Go'; [pi.role, pi.ariaLabel, p.getAttribute('role'), pi.ariaPressed].join()"), "button,Go,,");
    assert_eq!(eval(r, "const sr = p.attachShadow({mode: 'closed'}); pi.shadowRoot === sr"), "true");
    // A root attached before the upgrade is not revealed; one the upgrade
    // constructor attaches is.
    assert_eq!(
        eval(r, "var pre = document.createElement('x-pre'); pre.attachShadow({mode: 'closed'}); var up = document.createElement('x-up'); \
            customElements.define('x-pre', class extends HTMLElement { constructor() { super(); this.i = this.attachInternals(); } }); \
            customElements.define('x-up', class extends HTMLElement { constructor() { super(); this.sr = this.attachShadow({mode: 'closed'}); this.i = this.attachInternals(); } }); \
            customElements.upgrade(pre); customElements.upgrade(up); [pre.i.shadowRoot, up.i.shadowRoot === up.sr].join()"),
        ",true"
    );
}

#[test]
fn custom_states_match_state_pseudo_class() {
    let e = env(
        "<!doctype html><html><head><style>x-plain { color: rgb(0, 0, 255) } x-plain:state(on) { color: rgb(255, 0, 0) }</style></head><body></body></html>",
    );
    let r = &e.main;
    assert_eq!(eval(r, "customElements.define('x-plain', class extends HTMLElement { constructor() { super(); this.i = this.attachInternals(); } }); var el = document.body.appendChild(document.createElement('x-plain')); var st = el.i.states; typeof st"), "object");
    assert_eq!(
        eval(r, "[st instanceof CustomStateSet, st === el.i.states, st.size, String(st)].join()"),
        "true,true,0,[object CustomStateSet]"
    );
    assert_eq!(eval(r, "getComputedStyle(el).color"), "rgb(0, 0, 255)");
    assert_eq!(eval(r, "st.add('on'); [st.has('on'), st.size, el.matches(':state(on)'), document.querySelector(':state(on)') === el, el.matches(':state(off)')].join()"), "true,1,true,true,false");
    assert_eq!(eval(r, "getComputedStyle(el).color"), "rgb(255, 0, 0)");
    assert_eq!(
        eval(r, "st.delete('on'); [el.matches(':state(on)'), getComputedStyle(el).color].join()"),
        "false,rgb(0, 0, 255)"
    );
    // Set iteration: live, like a Set's.
    assert_eq!(
        eval(r, "st.add('--one'); st.add('--two'); st.add('--three'); const it = st.values(); st.delete('--one'); const a = it.next().value; st.clear(); [a, it.next().done, [...st].length].join()"),
        "--two,true,0"
    );
    assert_eq!(eval(r, "st.add('b'); st.add('a'); st.add('b'); const out = []; st.forEach(v => out.push(v)); [[...st].join(' '), out.join(' '), [...st.entries()][0].join(':')].join()"), "b a,b a,b:b");
    // A set made again after its wrapper was collected iterates the
    // element's states.
    eval(r, "var el2 = document.body.appendChild(document.createElement('x-plain')); el2.i.states.add('x'); el2.i.states.add('y'); 0");
    e.engine.collect_garbage_now();
    assert_eq!(eval(r, "[[...el2.i.states].join(' '), el2.i.states.size].join()"), "x y,2");
}

#[test]
fn define_upgrades_the_elements_of_shadow_trees() {
    // A component's shadow tree holds a component defined after it (HTML "upgrade candidates" are
    // the document's shadow-including descendants).
    let e = env(PAGE);
    let r = &e.main;
    assert_eq!(
        eval(
            r,
            "document.body.innerHTML = '<x-a></x-a>'; \
             customElements.define('x-a', class extends HTMLElement { constructor() { super(); this.attachShadow({mode: 'open'}).innerHTML = '<x-b></x-b>'; } }); \
             customElements.define('x-b', class extends HTMLElement { constructor() { super(); this.attachShadow({mode: 'open'}).innerHTML = '<i>b</i>'; } }); \
             const b = document.querySelector('x-a').shadowRoot.querySelector('x-b'); \
             String(b.shadowRoot !== null) + ':' + b.matches(':defined')"
        ),
        "true:true"
    );
}

/// A `connectedCallback` that connects another element of its own kind nests without end in
/// HTML (each insertion runs its reactions before it returns); it ends as on a browser's stack
/// overflow, at a fixed depth, instead of running forever (WPT many-slots-crash).
#[test]
fn self_inserting_connected_callbacks_end() {
    let e = env(PAGE);
    let r = &e.main;
    let calls = eval(
        r,
        "var calls = 0; customElements.define('x-rec', class extends HTMLElement { \
           connectedCallback() { calls++; this.attachShadow({ mode: 'open' }).appendChild(document.createElement('x-rec')); } \
         }); document.body.appendChild(document.createElement('x-rec')); calls",
    );
    assert_eq!(calls, "256");
}

/// Customized built-in elements: `define(name, C, {extends})`, `new C()`, `createElement(local,
/// {is})` (an is value, no `is` attribute; serialized as one), upgrade of an element whose is value
/// names the definition, the HTML element constructor checks, and the autonomous definition of a
/// local name winning over an is value.
#[test]
fn customized_built_in_elements() {
    let e = env("<!doctype html><html><body><button is=x-late id=late></button></body></html>");
    let r = &e.main;
    assert_eq!(
        eval(
            r,
            "class XB extends HTMLButtonElement { constructor() { super(); this.made = true; } }; \
             customElements.define('x-b', XB, { extends: 'button' }); \
             const a = new XB(); const b = document.createElement('button', { is: 'x-b' }); \
             [a.localName, a instanceof XB, a instanceof HTMLButtonElement, a.outerHTML, b instanceof XB, b.made, \
              b.hasAttribute('is'), b.outerHTML].join('|')"
        ),
        "button|true|true|<button is=\"x-b\"></button>|true|true|false|<button is=\"x-b\"></button>"
    );
    // The wrong interface's constructor, `extends` naming a custom or unknown element.
    assert_eq!(
        eval(
            r,
            "class Bad extends HTMLDivElement {}; customElements.define('x-bad', Bad, { extends: 'button' }); \
             const out = []; try { new Bad(); out.push('no throw'); } catch (e) { out.push(e.name); } \
             for (const x of ['x-other', 'bogus']) { try { customElements.define('x-' + x + '-c', class extends HTMLElement {}, { extends: x }); out.push('ok'); } catch (e) { out.push(e.name); } } \
             out.join('|')"
        ),
        "TypeError|NotSupportedError|NotSupportedError"
    );
    // Upgrade on definition; an autonomous definition of the local name comes first.
    assert_eq!(
        eval(
            r,
            "class Late extends HTMLButtonElement {}; customElements.define('x-late', Late, { extends: 'button' }); \
             class Auto extends HTMLElement {}; class IsC extends HTMLElement {}; \
             customElements.define('x-auto', Auto); customElements.define('x-isc', IsC); \
             const el = document.createElement('x-auto', { is: 'x-isc' }); \
             [document.getElementById('late') instanceof Late, el instanceof Auto].join('|')"
        ),
        "true|true"
    );
}

#[test]
fn scoped_registry_identity_and_definition_follow_a_retained_element() {
    let e = env(PAGE);
    let r = &e.main;
    assert_eq!(eval(r, r#"
        class GlobalBadge extends HTMLElement {}
        customElements.define('app-badge', GlobalBadge);
        const badge = (() => {
            const local = new CustomElementRegistry();
            local.label = 'design-system';
            class LocalBadge extends HTMLElement {
                static observedAttributes = ['title'];
                attributeChangedCallback(name, before, after) { this.seen = after; }
            }
            local.define('app-badge', LocalBadge);
            return document.createElement('app-badge', { customElementRegistry: local });
        })();
        badge instanceof GlobalBadge
    "#), "false");
    e.engine.collect_garbage();
    assert_eq!(eval(r, r#"
        badge.title = 'retained';
        const local = badge.customElementRegistry;
        [local.label, badge instanceof local.get('app-badge'), badge.seen,
         document.createElement('app-badge') instanceof GlobalBadge].join('|')
    "#), "design-system|true|retained|true");
}

#[test]
fn registry_initialization_does_not_replace_other_definitions() {
    let e = env(PAGE);
    assert_eq!(eval(&e.main, r#"
        const a = new CustomElementRegistry(), b = new CustomElementRegistry();
        class A extends HTMLElement {}
        class B extends HTMLElement {}
        a.define('app-tag', A);
        b.define('app-tag', B);
        const host = document.createElement('div');
        const root = host.attachShadow({mode:'open', customElementRegistry:null});
        const first = document.createElement('app-tag', {customElementRegistry:null});
        const second = document.createElement('app-tag', {customElementRegistry:b});
        root.append(first, second);
        a.initialize(root);
        [root.customElementRegistry === a, first instanceof A,
         second instanceof B, second.customElementRegistry === b].join(',')
    "#), "true,true,true,true");
}

#[test]
fn fragment_upgrade_keeps_definition_chosen_before_sibling_adoption() {
    let e = env(PAGE);
    assert_eq!(eval(&e.main, r#"
        const inert = document.implementation.createHTMLDocument('inert');
        const host = document.createElement('div');
        const calls = [];
        let adopted;
        class First extends HTMLElement {
            constructor() {
                super();
                calls.push('first');
                adopted = inert.adoptNode(this.nextElementSibling);
            }
        }
        class Next extends HTMLElement {
            constructor() { super(); calls.push('next'); }
        }
        customElements.define('x-first', First);
        customElements.define('x-next', Next);
        host.innerHTML = '<x-first></x-first><x-next></x-next>';
        [calls.join(','), adopted.ownerDocument === inert, adopted instanceof Next,
         adopted.customElementRegistry === null].join('|')
    "#), "first,next|true|true|true");
}

#[test]
fn synchronous_autonomous_construction_reports_and_returns_a_fresh_failed_element() {
    let e = env(PAGE);
    e.main.set_exception_reporter(Some(lifecycle::report_exception));
    assert_eq!(eval(&e.main, r#"
        const registry = new CustomElementRegistry(), errors = [];
        let partial;
        window.onerror = (message, file, line, column, error) => {
            errors.push(error.name); return true;
        };
        class WithAttribute extends HTMLElement {
            constructor() { super(); partial = this; this.title = 'changed'; }
        }
        registry.define('x-with-attribute', WithAttribute);
        const result = document.createElement('x-with-attribute', {customElementRegistry: registry});
        [errors.join(','), partial !== result, partial.title, result instanceof HTMLUnknownElement,
         result.title, result.customElementRegistry === registry, result.matches(':defined')].join('|')
    "#), "NotSupportedError|true|changed|true||true|false");
}
