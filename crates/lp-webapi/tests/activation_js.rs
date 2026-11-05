//! Click activation as a page sees it (DOM "dispatch" steps 5 to 12, HTML
//! "activation behavior"): one activation target per click, the
//! legacy-pre-activation and legacy-canceled-activation behavior of
//! checkboxes and radio buttons, what a disabled, detached or nested control
//! does, relatedTarget retargeting and `offsetX`
//! (dom/events/Event-dispatch-click, Event-dispatch-single-activation-behavior,
//! Event-dispatch-detached-input-and-change, relatedTarget.window,
//! shadow-relatedTarget, mouse-event-retarget).

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

/// Records what the document asks to navigate to (followed links, submitted
/// forms) and runs posted tasks only when asked.
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
    let mut vp = page.viewport();
    vp.width = 1440;
    vp.height = 900;
    page.set_viewport(vp);
    page.layout.borrow_mut().style_mut().discover_style_elements(&page.dom.borrow());
    let hooks = Rc::new(Hooks::default());
    page.set_hooks(Some(hooks.clone()));
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
}

/// `make(tag, parent, props)`: a new element appended to `parent`.
const PRELUDE: &str = "var make = (tag, parent, props = {}) => Object.assign(parent.appendChild(document.createElement(tag)), props);";

#[test]
fn a_click_has_one_activation_target() {
    let e = env("<body></body>");
    // The target itself, not an ancestor with activation behavior too.
    assert_eq!(
        e.eval(&format!(
            "{PRELUDE} (() => {{
                const seen = [];
                const outer = make('input', document.body, {{type: 'checkbox'}});
                const inner = make('input', outer, {{type: 'checkbox'}});
                outer.onclick = () => seen.push('outer ' + outer.checked);
                inner.onclick = () => seen.push('inner ' + inner.checked);
                inner.dispatchEvent(new MouseEvent('click', {{bubbles: true}}));
                return seen.join() + ' -> ' + [inner.checked, outer.checked].join();
            }})()"
        )),
        "inner true,outer false -> true,false"
    );
    // The nearest ancestor with activation behavior, and only when the click bubbles.
    assert_eq!(
        e.eval(&format!(
            "{PRELUDE} (() => {{
                const box = make('input', document.body, {{type: 'checkbox'}});
                const child = make('span', box);
                child.dispatchEvent(new MouseEvent('click'));
                const still = box.checked;
                child.dispatchEvent(new MouseEvent('click', {{bubbles: true}}));
                // Not a MouseEvent: no activation at all.
                const other = make('input', document.body, {{type: 'checkbox'}});
                other.dispatchEvent(new Event('click', {{bubbles: true}}));
                return [still, box.checked, other.checked].join();
            }})()"
        )),
        "false,true,false"
    );
    // A button that does nothing (type=button), a text input or a label still
    // is the activation target: the link, submit button or details around it
    // is not activated.
    assert_eq!(
        e.eval(&format!(
            "{PRELUDE} (() => {{
                const submitted = [];
                const link = make('a', document.body, {{href: '/followed'}});
                make('button', link, {{type: 'button'}}).click();
                make('input', link, {{type: 'text'}}).click();
                const form = make('form', document.body, {{onsubmit: () => {{ submitted.push('form'); return false; }}}});
                const submit = make('button', form);
                make('button', make('label', submit), {{type: 'button'}}).click();
                const details = make('details', document.body);
                const summary = make('summary', details);
                make('input', summary, {{type: 'checkbox'}}).click();
                return [submitted.length, details.open].join();
            }})()"
        )),
        "0,false"
    );
    assert_eq!(e.navigations(), Vec::<String>::new());
    // The link itself, or content of it without activation behavior, follows it.
    e.eval(&format!(
        "{PRELUDE} (() => {{
            const link = make('a', document.body, {{href: '/followed'}});
            make('span', link).click();
            // A link without href has none: the outer link is followed.
            make('a', make('a', document.body, {{href: '/outer'}})).click();
        }})()"
    ));
    assert_eq!(e.navigations(), vec!["GET http://localhost:5173/followed", "GET http://localhost:5173/outer"]);
    e.page.dispose();
}

#[test]
fn checkbox_and_radio_legacy_activation() {
    let e = env("<body></body>");
    // A disabled control is not clicked by click(), but a dispatched click
    // toggles it (it need not be mutable), and a canceled one restores it.
    assert_eq!(
        e.eval(&format!(
            "{PRELUDE} (() => {{
                const out = [];
                for (const type of ['checkbox', 'radio']) {{
                    const input = make('input', document.body, {{type, disabled: true}});
                    input.click();
                    out.push(input.checked);
                    let during;
                    input.onclick = () => during = input.checked;
                    input.dispatchEvent(new MouseEvent('click'));
                    out.push(during, input.checked);
                    input.checked = false;
                    input.onclick = event => event.preventDefault();
                    input.dispatchEvent(new MouseEvent('click', {{cancelable: true}}));
                    out.push(input.checked);
                }}
                return out.join();
            }})()"
        )),
        "false,true,true,false,false,true,true,false"
    );
    // The canceled click of a radio button re-checks the group's previous one,
    // and a checkbox gets its indeterminate state back.
    assert_eq!(
        e.eval(&format!(
            "{PRELUDE} (() => {{
                const a = make('input', document.body, {{type: 'radio', name: 'g', checked: true}});
                const b = make('input', document.body, {{type: 'radio', name: 'g'}});
                let during;
                b.onclick = event => {{ during = [a.checked, b.checked].join('/'); event.preventDefault(); }};
                b.click();
                const box = make('input', document.body, {{type: 'checkbox', indeterminate: true}});
                let mixed;
                box.onclick = event => {{ mixed = box.indeterminate; event.preventDefault(); }};
                box.click();
                return [during, a.checked, b.checked, mixed, box.indeterminate, box.checked].join();
            }})()"
        )),
        "false/true,true,false,false,true,false"
    );
    // input and change fire for a connected control only, input composed and
    // both bubbling, also when a listener disabled the control meanwhile;
    // not for the radio button that was already checked.
    assert_eq!(
        e.eval(&format!(
            "{PRELUDE} (() => {{
                const out = [];
                const watch = input => {{
                    input.oninput = event => out.push('input ' + [event.bubbles, event.composed, input.checked].join('/'));
                    input.onchange = event => out.push('change ' + event.bubbles);
                    return input;
                }};
                for (const type of ['checkbox', 'radio']) {{
                    const detached = watch(Object.assign(document.createElement('input'), {{type}}));
                    detached.click();
                    detached.dispatchEvent(new MouseEvent('click'));
                    out.push(type + ' ' + detached.checked);
                    const attached = watch(make('input', document.body, {{type}}));
                    attached.onclick = () => attached.disabled = true;
                    attached.click();
                }}
                const radio = watch(make('input', document.body, {{type: 'radio', checked: true}}));
                radio.click();
                return out.join();
            }})()"
        )),
        "checkbox false,input true/true/true,change true,radio true,input true/true/true,change true"
    );
    e.page.dispose();
}

#[test]
fn submit_buttons_and_forms() {
    let e = env("<body></body>");
    assert_eq!(
        e.eval(&format!(
            "{PRELUDE} (() => {{
                const out = [];
                // A disconnected form is not submitted.
                const loose = document.createElement('form');
                loose.onsubmit = () => {{ out.push('loose'); return false; }};
                make('input', loose, {{type: 'submit'}}).click();
                loose.requestSubmit();
                loose.submit();
                // Nor is one whose button a click listener disabled.
                const form = make('form', document.body, {{onsubmit: () => {{ out.push('form'); return false; }}}});
                const button = make('input', form, {{type: 'submit'}});
                button.onclick = () => button.disabled = true;
                button.dispatchEvent(new MouseEvent('click', {{cancelable: true}}));
                button.disabled = false;
                button.onclick = null;
                button.click();
                // An input that became a submit button during the click submits.
                const morph = make('input', form);
                morph.onclick = () => morph.type = 'submit';
                morph.click();
                return out.join();
            }})()"
        )),
        "form,form"
    );
    assert_eq!(e.navigations(), Vec::<String>::new());
    // The submit and reset events of a nested form do not reach the handlers
    // of the form around it.
    assert_eq!(
        e.eval(&format!(
            "{PRELUDE} (() => {{
                const out = [];
                const outer = make('form', document.body);
                outer.onsubmit = () => {{ out.push('outer submit'); return false; }};
                outer.onreset = () => out.push('outer reset');
                const holder = make('button', outer, {{type: 'button'}});
                const inner = make('form', holder);
                inner.onsubmit = () => {{ out.push('inner submit'); return false; }};
                inner.onreset = () => out.push('inner reset');
                document.addEventListener('submit', () => out.push('document'));
                make('input', inner, {{type: 'submit'}}).click();
                make('button', inner, {{type: 'reset'}}).click();
                return out.join();
            }})()"
        )),
        "inner submit,inner reset"
    );
    e.page.dispose();
}

#[test]
fn a_label_forwards_clicks_outside_interactive_content() {
    let e = env(
        "<body><label id=label><input id=box type=checkbox><span id=text>text</span>\
         <select id=select><option>o</option></select><button id=plain type=button>b</button></label></body>",
    );
    assert_eq!(
        e.eval(
            "(() => {
                const out = [];
                const box = document.getElementById('box');
                for (const id of ['text', 'select', 'plain', 'label', 'box']) {
                    box.checked = false;
                    document.getElementById(id).click();
                    out.push(id + ' ' + box.checked);
                }
                return out.join();
            })()"
        ),
        "text true,select false,plain false,label true,box true"
    );
    e.page.dispose();
}

#[test]
fn related_target_is_retargeted_and_reset() {
    let e = env("<body><div id=host></div><input id=light></body>");
    assert_eq!(
        e.eval(
            "(() => {
                const out = [];
                const host = document.getElementById('host');
                const shadow = host.attachShadow({mode: 'closed'});
                const inside = shadow.appendChild(document.createElement('input'));
                // Targets pointing into a shadow tree are reset after dispatch.
                let event = new FocusEvent('demo', {relatedTarget: window});
                inside.dispatchEvent(event);
                out.push(event.target === null && event.relatedTarget === null);
                // A shadow-tree relatedTarget is seen as its host, during and after.
                let during;
                document.body.addEventListener('demo', e => during = e.relatedTarget === host, {once: true});
                event = new FocusEvent('demo', {relatedTarget: inside});
                document.body.dispatchEvent(event);
                out.push(during, event.target === document.body, event.relatedTarget === host);
                // Dispatched at the host itself, it is not dispatched at all.
                let called = false;
                host.addEventListener('heya', () => called = true);
                event = new FocusEvent('heya', {relatedTarget: shadow, cancelable: true});
                event.preventDefault();
                out.push(host.dispatchEvent(event), called, event.target === null && event.relatedTarget === null);
                event.initEvent('x');
                out.push(event.type);
                // The engine's own focus events: focus moving out of the shadow tree.
                inside.focus();
                const light = document.getElementById('light');
                let atWindow, atTarget;
                window.addEventListener('focus', e => atWindow = e.relatedTarget === host, {capture: true, once: true});
                light.addEventListener('focus', e => atTarget = e.relatedTarget === host, {once: true});
                light.focus();
                out.push(atWindow, atTarget);
                return out.join();
            })()"
        ),
        "true,true,true,true,false,false,true,x,true,true"
    );
    e.page.dispose();
}

#[test]
fn offset_is_relative_to_the_targets_padding_edge() {
    let e = env(
        "<!doctype html><html><body style='margin:8px'>\
         <div id=target style='border:3px solid;height:20px'>Hello</div></body></html>",
    );
    assert_eq!(
        e.eval(
            "(() => {
                const target = document.getElementById('target');
                const event = new MouseEvent('click', {clientX: 50, clientY: 30, bubbles: true});
                // Without a target it is the page position.
                const before = [event.offsetX, event.offsetY].join('/');
                let during, atBody;
                target.addEventListener('click', e => during = [e.offsetX, e.offsetY].join('/'));
                document.body.addEventListener('click', e => atBody = [e.offsetX, e.offsetY].join('/'));
                target.dispatchEvent(event);
                return [before, during, atBody].join();
            })()"
        ),
        "50/30,39/19,39/19"
    );
    e.page.dispose();
}
