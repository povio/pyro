//! CSS Typed OM (`CSSStyleValue` and its subclasses, `CSS.*`, `attributeStyleMap`) through the
//! bindings on the real JavaScriptCore, values parsed and normalized by Stylo.

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
fn values_parse_reify_and_serialize() {
    let e = env("<!doctype html><div id=d style='width: 10px; color: red'></div>");
    let r = &e.main;
    assert_eq!(eval(r, "[CSS.px(5).toString(), CSS.percent(50) + '', CSS.number(2).unit, CSS.Q(1).unit].join()"), "5px,50%,number,q");
    assert_eq!(eval(r, "CSS.px(1).add(CSS.px(2)).toString()"), "3px");
    assert_eq!(eval(r, "CSS.px(1).add(CSS.em(2)).toString()"), "calc(1px + 2em)");
    assert_eq!(eval(r, "CSS.cm(1).to('mm').value"), "10");
    assert_eq!(eval(r, "JSON.stringify(CSS.px(1).mul(CSS.px(2)).type())"), "{\"length\":2}");
    assert_eq!(eval(r, "CSSNumericValue.parse('calc(1px - 2em)').constructor.name"), "CSSMathSum");
    let v = "const v = CSSStyleValue.parse('width', '10PX'); [v.constructor.name, v.value, v.unit].join()";
    assert_eq!(eval(r, v), "CSSUnitValue,10,px");
    assert_eq!(eval(r, "CSSStyleValue.parse('display', 'BLOCK').constructor.name + CSSStyleValue.parse('display', 'block').value"), "CSSKeywordValueblock");
    assert_eq!(eval(r, "CSSStyleValue.parse('width', 'var(--w)') instanceof CSSUnparsedValue"), "true");
    assert_eq!(eval(r, "try { CSSStyleValue.parse('width', 'red') } catch (e) { e.name }"), "TypeError");
    assert_eq!(eval(r, "CSSStyleValue.parse('transform', 'translate(1px, 2px) rotate(45deg)').toString()"), "translate(1px, 2px) rotate(45deg)");
    assert_eq!(eval(r, "CSSStyleValue.parseAll('transition-duration', '1s, 2s').length"), "2");
    // CSSUnparsedValue: indexed members and serialization.
    assert_eq!(
        eval(r, "const u = new CSSUnparsedValue(['a ', new CSSVariableReferenceValue('--x')]); [u.length, u[0], String(u)].join('|')"),
        "2|a |a var(--x)"
    );
}

#[test]
fn attribute_style_map_reads_and_writes_the_inline_style() {
    let e = env("<!doctype html><div id=d style='width: 10px; margin-top: 2em'></div>");
    let r = &e.main;
    assert_eq!(eval(r, "const d = document.getElementById('d'); d.attributeStyleMap === d.attributeStyleMap"), "true");
    assert_eq!(eval(r, "String(d.attributeStyleMap.get('width'))"), "10px");
    assert_eq!(eval(r, "d.attributeStyleMap.set('height', CSS.px(20)); d.style.height"), "20px");
    assert_eq!(eval(r, "d.attributeStyleMap.set('height', '3em'); d.style.height"), "3em");
    assert_eq!(eval(r, "try { d.attributeStyleMap.set('height', CSS.deg(1)) } catch (e) { e.name }"), "TypeError");
    assert_eq!(eval(r, "[d.attributeStyleMap.has('width'), d.attributeStyleMap.delete('width'), d.attributeStyleMap.has('width')].join()"), "true,,false");
    assert_eq!(eval(r, "[...d.attributeStyleMap.keys()].join()"), "height,margin-top");
    assert_eq!(eval(r, "d.attributeStyleMap.clear(); d.attributeStyleMap.size"), "0");
}

#[test]
fn out_of_range_calc_colors_and_unsupported_values() {
    let e = env("<!doctype html><div id=d></div>");
    let r = &e.main;
    assert_eq!(eval(r, "const m = document.getElementById('d').attributeStyleMap; m.set('width', CSS.em(-3)); d.style.width"), "calc(-3em)");
    assert_eq!(eval(r, "const w = m.get('width'); [w.constructor.name, w.values[0].value].join()"), "CSSMathSum,-3");
    assert_eq!(eval(r, "try { m.set('width', CSS.number(0)) } catch (e) { e.name }"), "TypeError", "unitless 0 is a number, not a length");
    assert_eq!(eval(r, "CSSStyleValue.parse('width', 'calc(10px)').constructor.name"), "CSSMathSum");
    assert_eq!(eval(r, "CSSStyleValue.parse('color', 'red').constructor.name"), "CSSStyleValue");
    assert_eq!(eval(r, "CSSStyleValue.parse('margin', '1px 2px').constructor.name"), "CSSStyleValue");
    assert_eq!(eval(r, "const c = CSSColorValue.parse('#00bfff'); [c.constructor.name, c.g.value].join()"), "CSSRGB,191");
    assert_eq!(eval(r, "new CSSHSL(180, 0.5, CSS.percent(20)).s.value"), "50");
    assert_eq!(eval(r, "try { new CSSRGB(CSS.deg(1), 0, 0) } catch (e) { e.name }"), "SyntaxError");
}

