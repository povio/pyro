//! CanvasRenderingContext2D without a rasterizer: state, styles, paths,
//! transforms and the value types made in JS on first use.

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
    #[allow(dead_code)]
    page: Rc<PageDom>,
    main: Realm,
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
    dom.borrow_mut().set_resource_tracking(true);
    let doc = parse_document(&dom, "http://localhost:5173/app", html);
    dom.borrow_mut().set_document_active(doc, true).unwrap();
    let page = PageDom::new(dom, doc);
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

const CANVAS: &str = "<!doctype html><canvas id=c width=100 height=50></canvas>";

#[test]
fn styles_gradients_and_patterns_are_kept_like_chrome() {
    let e = env(CANVAS);
    let r = &e.main;
    eval(r, "var c = document.getElementById('c'), x = c.getContext('2d');");
    assert_eq!(
        eval(
            r,
            "[x.fillStyle, x.strokeStyle, x.lineWidth, x.lineCap, x.globalAlpha, x.textBaseline, x.shadowColor, x.imageSmoothingEnabled].join()"
        ),
        "#000000,#000000,1,butt,1,alphabetic,rgba(0, 0, 0, 0),true"
    );
    assert_eq!(
        eval(
            r,
            "x.fillStyle = 'red'; x.fillStyle = 'nonsense'; x.strokeStyle = 'rgba(0,0,255,0.5)'; x.lineWidth = -1; x.lineWidth = 3; x.globalAlpha = 2; [x.fillStyle, x.strokeStyle, x.lineWidth, x.globalAlpha].join()"
        ),
        "#ff0000,rgba(0, 0, 255, 0.5),3,1"
    );
    assert_eq!(
        eval(
            r,
            "var g = x.createLinearGradient(0, 0, 10, 0); g.addColorStop(0, 'red'); g.addColorStop(1, '#00f'); x.fillStyle = g; [g instanceof CanvasGradient, x.fillStyle === g, String(g)].join()"
        ),
        "true,true,[object CanvasGradient]"
    );
    assert_eq!(eval(r, "try { g.addColorStop(2, 'red') } catch (e) { e.name }"), "IndexSizeError");
    assert_eq!(eval(r, "try { g.addColorStop(0, 'bogus') } catch (e) { e.name }"), "SyntaxError");
    assert_eq!(eval(r, "try { x.createLinearGradient(0, 0, NaN, 0) } catch (e) { e.name }"), "TypeError");
    assert_eq!(eval(r, "try { x.createRadialGradient(0, 0, -1, 0, 0, 1) } catch (e) { e.name }"), "IndexSizeError");
    assert_eq!(eval(r, "x.createConicGradient(0, 5, 5) instanceof CanvasGradient"), "true");
    assert_eq!(eval(r, "try { new CanvasGradient() } catch (e) { e.name }"), "TypeError");
    assert_eq!(
        eval(
            r,
            "var p = x.createPattern(document.createElement('canvas'), 'repeat-x'); [p instanceof CanvasPattern, x.createPattern(document.createElement('video'), null)].join()"
        ),
        "true,"
    );
    assert_eq!(eval(r, "try { x.createPattern(c, 'diagonal') } catch (e) { e.name }"), "SyntaxError");
    assert_eq!(eval(r, "try { x.createPattern({}, '') } catch (e) { e.name }"), "TypeError");
    // save/restore keeps every state member, the style objects included.
    assert_eq!(
        eval(
            r,
            "x.save(); x.fillStyle = 'blue'; x.lineJoin = 'round'; x.restore(); [x.fillStyle === g, x.lineJoin].join()"
        ),
        "true,miter"
    );
    // Resizing resets the context.
    assert_eq!(eval(r, "c.width = 100; [x.fillStyle, x.lineWidth].join()"), "#000000,1");
}

#[test]
fn transforms_and_geometry_interfaces() {
    let e = env(CANVAS);
    let r = &e.main;
    eval(r, "var x = document.getElementById('c').getContext('2d');");
    assert_eq!(
        eval(
            r,
            "x.translate(10, 20); x.scale(2, 3); var m = x.getTransform(); [m instanceof DOMMatrix, m.a, m.d, m.e, m.f, m.is2D].join()"
        ),
        "true,2,3,10,20,true"
    );
    assert_eq!(
        eval(r, "x.setTransform(new DOMMatrix([1, 0, 0, 1, 5, 6])); String(x.getTransform())"),
        "matrix(1, 0, 0, 1, 5, 6)"
    );
    assert_eq!(eval(r, "x.setTransform({a: 2, m11: 2, f: 1}); String(x.getTransform())"), "matrix(2, 0, 0, 1, 0, 1)");
    assert_eq!(eval(r, "try { x.setTransform({a: 2, m11: 3}) } catch (e) { e.name }"), "TypeError");
    assert_eq!(eval(r, "x.resetTransform(); x.getTransform().isIdentity"), "true");
    assert_eq!(
        eval(
            r,
            "var d = new DOMMatrix('translate(10px, 5px) scale(2)'); var q = d.transformPoint({x: 1, y: 1}); [q instanceof DOMPoint, q.x, q.y, d.inverse().multiply(d).isIdentity].join()"
        ),
        "true,12,7,true"
    );
    assert!(eval(r, "new DOMMatrix().rotate(90).toString()").starts_with("matrix(6.123"));
    assert_eq!(eval(r, "new DOMMatrix().rotateAxisAngle(1, 0, 0, 45).is2D"), "false");
    assert_eq!(eval(r, "try { new DOMMatrix('translate(1em)') } catch (e) { e.name }"), "SyntaxError");
    assert_eq!(eval(r, "try { new DOMMatrix([1, 2, 3]) } catch (e) { e.name }"), "TypeError");
    assert_eq!(
        eval(r, "DOMMatrix.fromMatrix({m33: 2}).is2D + ',' + new DOMMatrixReadOnly().translate(1).e"),
        "false,1"
    );
    assert_eq!(eval(r, "WebKitCSSMatrix === DOMMatrix && DOMPoint.fromPoint({x: 3}).x"), "3");
    assert_eq!(eval(r, "JSON.stringify(new DOMPointReadOnly(1, 2))"), r#"{"x":1,"y":2,"z":0,"w":1}"#);
}

#[test]
fn paths_answer_hit_tests() {
    let e = env(CANVAS);
    let r = &e.main;
    eval(r, "var x = document.getElementById('c').getContext('2d');");
    assert_eq!(
        eval(
            r,
            "x.beginPath(); x.rect(10, 10, 20, 20); [x.isPointInPath(15, 15), x.isPointInPath(5, 5), x.isPointInPath(30, 20)].join()"
        ),
        "true,false,true"
    );
    // The transform applies when points are added, not when tested.
    assert_eq!(
        eval(
            r,
            "x.beginPath(); x.translate(50, 0); x.arc(0, 25, 10, 0, Math.PI * 2); x.resetTransform(); [x.isPointInPath(50, 25), x.isPointInPath(0, 25)].join()"
        ),
        "true,false"
    );
    assert_eq!(
        eval(
            r,
            "x.beginPath(); x.rect(0, 0, 40, 40); x.rect(10, 10, 20, 20); [x.isPointInPath(20, 20), x.isPointInPath(20, 20, 'evenodd')].join()"
        ),
        "true,false"
    );
    assert_eq!(
        eval(
            r,
            "x.beginPath(); x.moveTo(0, 0); x.lineTo(40, 0); x.lineWidth = 4; [x.isPointInStroke(20, 1), x.isPointInStroke(20, 5)].join()"
        ),
        "true,false"
    );
    assert_eq!(
        eval(
            r,
            "var p = new Path2D('M0 0 H10 V10 H0 Z'); var q = new Path2D(); q.addPath(p, {e: 100}); q.roundRect(20, 0, 10, 10, [2]); [x.isPointInPath(p, 5, 5), x.isPointInPath(q, 105, 5), x.isPointInPath(q, 25, 5), x.isPointInPath(q, 5, 5)].join()"
        ),
        "true,true,true,false"
    );
    assert_eq!(eval(r, "try { new Path2D().arc(0, 0, -1, 0, 1) } catch (e) { e.name }"), "IndexSizeError");
    // Long paths (charts) copy without hitting the argument limit; a path
    // added to itself doubles.
    assert_eq!(
        eval(
            r,
            "var big = new Path2D(); for (var i = 0; i < 200000; i++) big.lineTo(i, i % 7); var c = new Path2D(big); c.addPath(c); x.isPointInStroke(c, 100, 2)"
        ),
        "true"
    );
    // Huge radii and line widths answer (flattening is bounded).
    assert_eq!(
        eval(
            r,
            "x.beginPath(); x.arc(0, 0, 1e300, 0, 6); x.lineWidth = 1e300; x.lineJoin = 'round'; x.lineCap = 'round'; [x.isPointInPath(5, 5), typeof x.isPointInStroke(5, 5), x.isPointInPath(new Path2D('M0 0 A1e300 1e300 0 1 1 1 1'), 0, 0)].join()"
        ),
        "true,boolean,false"
    );
    assert_eq!(eval(r, "try { x.roundRect(0, 0, 1, 1, [1, 2, 3, 4, 5]) } catch (e) { e.name }"), "RangeError");
    assert_eq!(eval(r, "try { x.isPointInPath({}, 1, 1) } catch (e) { e.name }"), "TypeError");
}

#[test]
fn pixels_are_transparent_until_something_is_drawn() {
    let e = env(CANVAS);
    let r = &e.main;
    eval(r, "var c = document.getElementById('c'), x = c.getContext('2d');");
    assert_eq!(
        eval(
            r,
            "var d = x.getImageData(0, 0, 4, 2); [d instanceof ImageData, d.width, d.height, d.data.length, d.data.every(v => v === 0), d.colorSpace].join()"
        ),
        "true,4,2,32,true,srgb"
    );
    assert_eq!(
        eval(
            r,
            "var i = x.createImageData(-3, 2); var j = x.createImageData(i); [i.width, i.data.length, j.width, j.height].join()"
        ),
        "3,24,3,2"
    );
    assert_eq!(eval(r, "try { x.createImageData(0, 1) } catch (e) { e.name }"), "IndexSizeError");
    assert_eq!(eval(r, "var k = new ImageData(new Uint8ClampedArray(16), 2); [k.width, k.height].join()"), "2,2");
    assert_eq!(eval(r, "try { new ImageData(new Uint8ClampedArray(6), 1) } catch (e) { e.name }"), "InvalidStateError");
    assert_eq!(eval(r, "try { new ImageData(new Uint8ClampedArray(16), 3) } catch (e) { e.name }"), "IndexSizeError");
    assert_eq!(
        eval(r, "x.fillRect(0, 0, 5, 5); try { x.getImageData(0, 0, 1, 1); 'pixels' } catch (e) { e.name }"),
        "NotSupportedError"
    );
    assert_eq!(eval(r, "x.clearRect(0, 0, c.width, c.height); x.getImageData(0, 0, 1, 1).data.join()"), "0,0,0,0");
    assert_eq!(eval(r, "x.fillRect(0, 0, 5, 5); c.height = 50; x.getImageData(0, 0, 1, 1).width"), "1");
    assert_eq!(eval(r, "try { x.putImageData({}, 0, 0) } catch (e) { e.name }"), "TypeError");
    assert_eq!(
        eval(
            r,
            "var m = x.measureText('abc'); [m instanceof TextMetrics, m.width > 0, m.alphabeticBaseline, m.fontBoundingBoxAscent > 0].join()"
        ),
        "true,true,0,true"
    );
    assert_eq!(
        eval(
            r,
            "x.textBaseline = 'top'; var t = x.measureText('abc'); [t.fontBoundingBoxAscent, t.alphabeticBaseline < 0].join()"
        ),
        "0,true"
    );
    assert_eq!(
        eval(r, "typeof c.toBlob + ',' + (() => { try { c.toBlob() } catch (e) { return e.name } })()"),
        "function,TypeError"
    );
}

#[test]
fn canvas_interfaces_are_lazy_globals() {
    let e = env(CANVAS);
    let r = &e.main;
    assert_eq!(
        eval(r, "['Path2D', 'ImageData', 'DOMMatrix', 'DOMPoint', 'TextMetrics'].map(n => typeof window[n]).join()"),
        "function,function,function,function,function"
    );
    // The first read replaces the accessor with a writable data property.
    assert_eq!(
        eval(
            r,
            "var d = Object.getOwnPropertyDescriptor(window, 'Path2D'); [typeof d.value, d.writable, d.enumerable].join()"
        ),
        "function,true,false"
    );
    assert_eq!(eval(r, "window.ImageData = 1; ImageData"), "1");
}
