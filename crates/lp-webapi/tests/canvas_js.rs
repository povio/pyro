//! CanvasRenderingContext2D: state, styles, paths, transforms, the value
//! types made in JS on first use, and pixels (rasterized by lp-canvas).

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
    assert_eq!(eval(r, "x.fillRect(0, 0, 5, 5); x.getImageData(0, 0, 1, 1).data.join()"), "0,0,0,255");
    assert_eq!(eval(r, "x.clearRect(0, 0, c.width, c.height); x.getImageData(0, 0, 1, 1).data.join()"), "0,0,0,0");
    assert_eq!(
        eval(
            r,
            "x.fillRect(0, 0, 5, 5); c.height = 50; [x.getImageData(0, 0, 1, 1).data.join(), x.fillStyle].join('|')"
        ),
        "0,0,0,0|#000000",
        "resizing clears the bitmap and resets the state"
    );
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

/// `px(x, y)`: the pixel at (x, y) as "r,g,b,a".
const PX: &str = "var c = document.getElementById('c'), x = c.getContext('2d'); \
    var px = (X, Y, ctx) => Array.from((ctx || x).getImageData(X, Y, 1, 1).data).join();";

#[test]
fn rects_paint_exact_pixels() {
    let e = env(CANVAS);
    let r = &e.main;
    eval(r, PX);
    assert_eq!(eval(r, "x.fillStyle = 'red'; x.fillRect(10, 10, 20, 10); [px(10, 10), px(29, 19), px(30, 10), px(9, 10)].join('|')"),
        "255,0,0,255|255,0,0,255|0,0,0,0|0,0,0,0");
    assert_eq!(eval(r, "x.clearRect(12, 12, 4, 4); [px(13, 13), px(16, 13)].join('|')"), "0,0,0,0|255,0,0,255");
    // A 2px stroke centred on integer edges covers one pixel each side.
    assert_eq!(
        eval(r, "x.strokeStyle = '#00ff00'; x.lineWidth = 2; x.strokeRect(50, 10, 20, 20); [px(49, 20), px(50, 20), px(51, 20), px(60, 20)].join('|')"),
        "0,255,0,255|0,255,0,255|0,0,0,0|0,0,0,0"
    );
    // Semi-transparent colors and globalAlpha read back unpremultiplied.
    assert_eq!(eval(r, "x.fillStyle = 'rgba(0, 0, 255, 0.5)'; x.fillRect(80, 0, 5, 5); px(81, 1)"), "0,0,255,128");
    assert_eq!(
        eval(r, "x.globalAlpha = 0.5; x.fillStyle = '#0000ff'; x.fillRect(90, 0, 5, 5); x.globalAlpha = 1; px(91, 1)"),
        "0,0,255,128"
    );
}

#[test]
fn paths_transforms_and_clips() {
    let e = env(CANVAS);
    let r = &e.main;
    eval(r, PX);
    // A circle: its centre is painted, a corner of its box is not.
    assert_eq!(
        eval(r, "x.beginPath(); x.arc(20, 20, 10, 0, Math.PI * 2); x.fill(); [px(20, 20), px(11, 11), px(28, 20)].join('|')"),
        "0,0,0,255|0,0,0,0|0,0,0,255"
    );
    // evenodd leaves the inner square's hole.
    assert_eq!(
        eval(r, "x.beginPath(); x.fillStyle = 'blue'; x.rect(40, 0, 20, 20); x.rect(45, 5, 10, 10); x.fill('evenodd'); [px(42, 2), px(50, 10)].join('|')"),
        "0,0,255,255|0,0,0,0"
    );
    // save/translate/scale/restore.
    assert_eq!(
        eval(r, "x.save(); x.translate(70, 0); x.scale(2, 2); x.fillStyle = 'lime'; x.fillRect(0, 0, 5, 5); x.restore(); x.fillRect(0, 40, 2, 2); [px(79, 9), px(80, 10), px(0, 40)].join('|')"),
        "0,255,0,255|0,0,0,0|0,0,255,255"
    );
    // clip() limits drawing until restore().
    assert_eq!(
        eval(r, "x.clearRect(0, 0, 100, 50); x.save(); x.beginPath(); x.rect(0, 0, 10, 10); x.clip(); x.fillStyle = 'red'; x.fillRect(0, 0, 50, 50); x.restore(); x.fillRect(20, 20, 2, 2); [px(5, 5), px(15, 5), px(20, 20)].join('|')"),
        "255,0,0,255|0,0,0,0|0,0,255,255"
    );
    // Dashes: 4 on, 4 off along a horizontal line.
    assert_eq!(
        eval(r, "x.clearRect(0, 0, 100, 50); x.setLineDash([4, 4]); x.lineWidth = 2; x.strokeStyle = 'black'; x.beginPath(); x.moveTo(0, 31); x.lineTo(40, 31); x.stroke(); x.setLineDash([]); [px(1, 31), px(5, 31), px(9, 31)].join('|')"),
        "0,0,0,255|0,0,0,0|0,0,0,255"
    );
    // A Path2D, and a bezier.
    assert_eq!(
        eval(r, "x.clearRect(0, 0, 100, 50); var p = new Path2D('M60 0 h20 v20 h-20 z'); x.fillStyle = 'red'; x.fill(p); [px(70, 10), px(85, 10)].join('|')"),
        "255,0,0,255|0,0,0,0"
    );
    assert_eq!(
        eval(r, "x.beginPath(); x.moveTo(0, 49); x.bezierCurveTo(10, 30, 20, 30, 30, 49); x.closePath(); x.fill(); px(15, 45)"),
        "255,0,0,255"
    );
}

#[test]
fn gradients_patterns_and_compositing() {
    let e = env(CANVAS);
    let r = &e.main;
    eval(r, PX);
    let ends = eval(
        r,
        "var g = x.createLinearGradient(0, 0, 100, 0); g.addColorStop(0, 'red'); g.addColorStop(1, 'blue'); x.fillStyle = g; x.fillRect(0, 0, 100, 10); [px(0, 5), px(99, 5)].join('|')",
    );
    let parts: Vec<Vec<u32>> = ends.split('|').map(|p| p.split(',').map(|v| v.parse().unwrap()).collect()).collect();
    assert!(parts[0][0] > 240 && parts[0][2] < 15 && parts[1][2] > 240 && parts[1][0] < 15, "{ends}");
    // Stops added after the style was set apply to later draws.
    assert_eq!(eval(r, "g.addColorStop(0.5, 'lime'); x.fillRect(0, 10, 100, 10); var m = x.getImageData(50, 15, 1, 1).data; m[1] > 245 && m[0] < 10 && m[2] < 10"), "true");
    assert_eq!(
        eval(r, "var rg = x.createRadialGradient(50, 35, 0, 50, 35, 10); rg.addColorStop(0, 'white'); rg.addColorStop(1, 'black'); x.fillStyle = rg; x.fillRect(40, 25, 20, 20); var m = x.getImageData(50, 35, 1, 1).data; [m[0] > 230, px(40, 25)].join()"),
        "true,0,0,0,255"
    );
    // A pattern from another canvas, repeating.
    assert_eq!(
        eval(r, "var t = document.createElement('canvas'); t.width = 2; t.height = 2; var tx = t.getContext('2d'); tx.fillStyle = 'red'; tx.fillRect(0, 0, 1, 1); x.clearRect(0, 0, 100, 50); x.imageSmoothingEnabled = false; x.fillStyle = x.createPattern(t, 'repeat'); x.fillRect(0, 0, 10, 10); [px(0, 0), px(1, 0), px(4, 4), px(5, 4)].join('|')"),
        "255,0,0,255|0,0,0,0|255,0,0,255|0,0,0,0"
    );
    // Operators: destination-out erases, copy replaces everything, source-in
    // keeps the overlap only.
    assert_eq!(
        eval(r, "x.clearRect(0, 0, 100, 50); x.fillStyle = 'red'; x.fillRect(0, 0, 20, 20); x.globalCompositeOperation = 'destination-out'; x.fillRect(0, 0, 10, 20); x.globalCompositeOperation = 'source-over'; [px(5, 5), px(15, 5)].join('|')"),
        "0,0,0,0|255,0,0,255"
    );
    assert_eq!(
        eval(r, "x.fillRect(50, 0, 10, 10); x.globalCompositeOperation = 'copy'; x.fillStyle = 'blue'; x.fillRect(0, 30, 5, 5); x.globalCompositeOperation = 'source-over'; [px(15, 5), px(55, 5), px(2, 32)].join('|')"),
        "0,0,0,0|0,0,0,0|0,0,255,255"
    );
    assert_eq!(
        eval(r, "x.fillStyle = 'lime'; x.globalCompositeOperation = 'source-in'; x.fillRect(0, 30, 10, 10); x.globalCompositeOperation = 'source-over'; [px(2, 32), px(7, 32)].join('|')"),
        "0,255,0,255|0,0,0,0"
    );
    assert_eq!(
        eval(r, "x.fillStyle = '#808080'; x.globalCompositeOperation = 'multiply'; x.fillRect(0, 30, 5, 5); x.globalCompositeOperation = 'lighter'; x.fillStyle = '#ff0000'; x.fillRect(0, 30, 5, 5); px(2, 32)"),
        "255,128,0,255"
    );
    // A hard shadow, offset.
    assert_eq!(
        eval(r, "x.globalCompositeOperation = 'source-over'; x.clearRect(0, 0, 100, 50); x.shadowColor = 'black'; x.shadowOffsetX = 10; x.fillStyle = 'red'; x.fillRect(0, 0, 5, 5); x.shadowColor = 'transparent'; [px(2, 2), px(12, 2), px(17, 2)].join('|')"),
        "255,0,0,255|0,0,0,255|0,0,0,0"
    );
}

#[test]
fn image_data_round_trips_and_draw_image() {
    let e = env(CANVAS);
    let r = &e.main;
    eval(r, PX);
    assert_eq!(
        eval(r, "var d = x.createImageData(2, 2); d.data.set([255,0,0,255, 0,255,0,255, 0,0,255,255, 10,20,30,255]); x.putImageData(d, 3, 4); Array.from(x.getImageData(3, 4, 2, 2).data).join()"),
        "255,0,0,255,0,255,0,255,0,0,255,255,10,20,30,255"
    );
    // The dirty rectangle, and a negative source size.
    assert_eq!(
        eval(r, "x.putImageData(d, 10, 10, 1, 1, 1, 1); [px(10, 10), px(11, 11)].join('|')"),
        "0,0,0,0|10,20,30,255"
    );
    assert_eq!(
        eval(r, "Array.from(x.getImageData(5, 6, -2, -2).data).join()"),
        "255,0,0,255,0,255,0,255,0,0,255,255,10,20,30,255"
    );
    // Outside the canvas reads transparent.
    assert_eq!(eval(r, "px(-5, -5) + '|' + px(200, 200)"), "0,0,0,0|0,0,0,0");
    // drawImage from a canvas, scaled 2x with nearest-neighbour sampling.
    assert_eq!(
        eval(r, "var t = document.createElement('canvas'); t.width = 2; t.height = 1; t.getContext('2d').putImageData(d, 0, 0); x.imageSmoothingEnabled = false; x.drawImage(t, 40, 0, 4, 2); [px(40, 0), px(41, 1), px(42, 0), px(43, 1), px(44, 0)].join('|')"),
        "255,0,0,255|255,0,0,255|0,255,0,255|0,255,0,255|0,0,0,0"
    );
    // Source rectangle (9 arguments) and drawing a canvas onto itself.
    assert_eq!(
        eval(r, "x.drawImage(t, 1, 0, 1, 1, 60, 0, 1, 1); x.drawImage(c, 40, 0, 2, 2, 70, 0, 2, 2); [px(60, 0), px(70, 0)].join('|')"),
        "0,255,0,255|255,0,0,255"
    );
    assert_eq!(eval(r, "try { x.drawImage(t, 0, 0, 1) } catch (e) { e.name }"), "TypeError");
    assert_eq!(
        eval(
            r,
            "var z = document.createElement('canvas'); z.width = 0; try { x.drawImage(z, 0, 0) } catch (e) { e.name }"
        ),
        "InvalidStateError"
    );
}

#[test]
fn canvas_encodes_png_jpeg_and_blobs() {
    let e = env(CANVAS);
    let r = &e.main;
    eval(r, PX);
    assert_eq!(eval(r, "c.toDataURL().slice(0, 22)"), "data:image/png;base64,");
    assert_eq!(eval(r, "c.toDataURL('image/jpeg', 0.5).slice(0, 23)"), "data:image/jpeg;base64,");
    assert_eq!(eval(r, "c.toDataURL('image/bogus').slice(0, 15)"), "data:image/png;");
    assert_eq!(eval(r, "var z = document.createElement('canvas'); z.width = 0; z.toDataURL()"), "data:,");
    // The PNG round-trips: decode it through createImageBitmap from a Blob.
    eval(
        r,
        "x.fillStyle = 'red'; x.fillRect(0, 0, 1, 1); window.blobInfo = null; c.toBlob(b => { window.blobInfo = [b instanceof Blob, b.type, b.size > 50].join(); window.blob = b; });",
    );
    assert_eq!(eval(r, "blobInfo"), "true,image/png,true");
    eval(
        r,
        "window.decoded = 'pending'; createImageBitmap(blob).then(bm => { var y = document.createElement('canvas').getContext('2d'); y.drawImage(bm, 0, 0); decoded = [bm.width, bm.height, px(0, 0, y), px(1, 0, y)].join('|'); }, err => decoded = 'ERR ' + err);",
    );
    assert_eq!(eval(r, "decoded"), "100|50|255,0,0,255|0,0,0,0");
}

#[test]
fn offscreen_canvas_and_image_bitmaps() {
    let e = env(CANVAS);
    let r = &e.main;
    eval(r, PX);
    assert_eq!(
        eval(r, "var o = new OffscreenCanvas(4, 4); var ox = o.getContext('2d'); [ox === o.getContext('2d'), ox.canvas === o, ox instanceof OffscreenCanvasRenderingContext2D, x instanceof OffscreenCanvasRenderingContext2D, o.getContext('webgl')].join()"),
        "true,true,true,false,"
    );
    assert_eq!(
        eval(r, "ox.fillStyle = 'lime'; ox.fillRect(0, 0, 2, 2); px(1, 1, ox) + '|' + px(3, 3, ox)"),
        "0,255,0,255|0,0,0,0"
    );
    assert_eq!(eval(r, "x.drawImage(o, 0, 0); px(1, 1)"), "0,255,0,255");
    // transferToImageBitmap takes the pixels and clears the canvas.
    assert_eq!(
        eval(r, "var bm = o.transferToImageBitmap(); x.drawImage(bm, 10, 0); [bm instanceof ImageBitmap, bm.width, px(11, 1), px(1, 1, ox)].join('|')"),
        "true|4|0,255,0,255|0,0,0,0"
    );
    assert_eq!(
        eval(
            r,
            "bm.close(); [bm.width, (() => { try { x.drawImage(bm, 0, 0) } catch (e) { return e.name } })()].join()"
        ),
        "0,InvalidStateError"
    );
    eval(r, "window.blobType = null; ox.fillRect(0, 0, 1, 1); o.convertToBlob({type: 'image/webp'}).then(b => blobType = b.type + ':' + (b.size > 0));");
    assert_eq!(eval(r, "blobType"), "image/webp:true");
    // createImageBitmap with a crop and a resize.
    eval(
        r,
        "window.made = null; x.fillStyle = 'blue'; x.fillRect(20, 20, 2, 2); createImageBitmap(c, 20, 20, 2, 2, {resizeWidth: 4, resizeHeight: 4, resizeQuality: 'pixelated'}).then(b => { x.clearRect(0, 0, 100, 50); x.drawImage(b, 0, 0); made = [b.width, b.height, px(3, 3), px(4, 4)].join('|'); });",
    );
    assert_eq!(eval(r, "made"), "4|4|0,0,255,255|0,0,0,0");
    eval(r, "window.rejected = null; createImageBitmap({}).catch(e => rejected = e.name);");
    assert_eq!(eval(r, "rejected"), "TypeError");
    // Arrays (or proxies of them) are not image sources.
    assert_eq!(
        eval(r, "var hits = 0; var fake = new Proxy([3, new Blob([])], { get(t, k) { hits++; return t[k]; } }); try { x.drawImage(fake, 0, 0); 'drawn' } catch (e) { e.name + hits }"),
        "TypeError0"
    );
}

#[test]
fn text_draws_with_real_font_metrics() {
    let e = env(CANVAS);
    let r = &e.main;
    eval(r, PX);
    assert_eq!(
        eval(r, "x.font = '30px sans-serif'; var m = x.measureText('Hello'); [m.width > 50 && m.width < 90, m.actualBoundingBoxAscent > 15, m.actualBoundingBoxAscent < m.fontBoundingBoxAscent + 1, m.actualBoundingBoxDescent < 2, m.actualBoundingBoxLeft < 3].join()"),
        "true,true,true,true,true"
    );
    assert_eq!(eval(r, "x.measureText('Hello').width === m.width && x.measureText('Hellooo').width > m.width"), "true");
    // fillText paints ink above the baseline only.
    let inked = "var ink = (y0, y1) => { var d = x.getImageData(0, y0, 100, y1 - y0).data; for (var i = 3; i < d.length; i += 4) if (d[i]) return true; return false; };";
    eval(r, inked);
    assert_eq!(eval(r, "x.fillText('Hello', 5, 35); [ink(5, 35), ink(36, 50)].join()"), "true,false");
    // textAlign right ends the text at x; maxWidth squeezes it.
    assert_eq!(
        eval(r, "x.clearRect(0, 0, 100, 50); x.textAlign = 'right'; x.fillText('Hello', 50, 35, 20); var d = x.getImageData(0, 0, 100, 50).data, minX = 100, maxX = 0; for (var i = 3; i < d.length; i += 4) if (d[i]) { var X = (i >> 2) % 100; minX = Math.min(minX, X); maxX = Math.max(maxX, X); } [minX >= 28, maxX <= 50].join()"),
        "true,true"
    );
}

#[test]
fn canvas_bitmaps_are_lazy_and_reported_for_rendering() {
    let e = env(CANVAS);
    let r = &e.main;
    eval(r, PX);
    // Reading allocates nothing; nothing is reported until something is drawn.
    eval(r, "x.getImageData(0, 0, 10, 10); x.clearRect(0, 0, 10, 10);");
    assert!(e.page.canvas_bitmaps().is_empty());
    eval(r, "x.fillStyle = 'red'; x.fillRect(0, 0, 1, 1);");
    let bitmaps = e.page.canvas_bitmaps();
    assert_eq!(bitmaps.len(), 1);
    let (_, w, h, rgba) = &bitmaps[0];
    assert_eq!((*w, *h, rgba.len()), (100, 50, 100 * 50 * 4));
    assert_eq!(&rgba[..8], &[255, 0, 0, 255, 0, 0, 0, 0]);
    // setAttribute resizes (and clears) as the IDL attribute does.
    assert_eq!(eval(r, "c.setAttribute('width', '10'); px(0, 0)"), "0,0,0,0");
    assert!(e.page.canvas_bitmaps().is_empty());
}

#[test]
fn kept_image_bytes_are_counted_as_they_come_and_go() {
    let e = env("<!doctype html><canvas></canvas><img id=a src=/a.png><img id=b src=/b.png>");
    let page = &e.page;
    let id = |s: &str| page.dom.borrow().get_element_by_id(page.document(), s).unwrap().unwrap();
    let (a, b) = (id("a"), id("b"));
    let png = lp_canvas::Surface::new(2, 3).encode(lp_canvas::Encoding::Png).unwrap();
    let n = png.len();
    let load = |el, src: &str| {
        page.set_image_state(el, Some(src), false);
        page.dom.borrow_mut().set_natural_size(el, Some((2.0, 3.0)));
        page.set_image_bytes(el, png.clone(), true);
    };
    load(a, "/a.png");
    load(b, "/b.png");
    assert_eq!(page.image_bytes_kept(), (2 * n, 0));
    // Decoded on first draw, cached and counted once.
    let draw = "var x = document.querySelector('canvas').getContext('2d'); \
        x.drawImage(document.getElementById('a'), 0, 0); x.drawImage(document.getElementById('a'), 0, 0); 'ok'";
    assert_eq!(eval(&e.main, draw), "ok");
    assert_eq!(page.image_bytes_kept(), (2 * n, 2 * 3 * 4));
    // A reload replaces the entry and its counts; a cleared src forgets it.
    load(a, "/a.png");
    assert_eq!(page.image_bytes_kept(), (2 * n, 0));
    page.set_image_state(b, None, false);
    assert_eq!(page.image_bytes_kept(), (n, 0));
}

#[test]
fn image_bitmaps_clone_and_transfer_and_offscreen_canvases_transfer() {
    let e = env(CANVAS);
    let r = &e.main;
    eval(r, PX);
    eval(r, "var o = new OffscreenCanvas(4, 4); var ox = o.getContext('2d'); ox.fillStyle = 'lime'; ox.fillRect(0, 0, 4, 4); var bm = o.transferToImageBitmap();");
    // A clone shares the pixels and leaves the original as it was.
    assert_eq!(
        eval(r, "var copy = structuredClone(bm); x.drawImage(copy, 0, 0); [copy instanceof ImageBitmap, copy !== bm, copy.width, bm.width, px(1, 1)].join('|')"),
        "true|true|4|4|0,255,0,255"
    );
    // A transfer detaches the original.
    assert_eq!(
        eval(r, "var moved = structuredClone({bm}, {transfer: [bm]}).bm; [moved.width, moved.height, bm.width, bm.height].join()"),
        "4,4,0,0"
    );
    let error = "(f) => { try { f(); return 'ok'; } catch (e) { return e.name; } }";
    assert_eq!(
        eval(r, &format!("var err = {error}; [err(() => structuredClone(bm)), err(() => structuredClone(bm, {{transfer: [bm]}})), err(() => structuredClone(o)), err(() => structuredClone(o, {{transfer: [o]}}))].join()")),
        "DataCloneError,DataCloneError,DataCloneError,InvalidStateError"
    );
    // An OffscreenCanvas without a context transfers and is detached.
    assert_eq!(
        eval(r, "var fresh = new OffscreenCanvas(3, 2); var t = structuredClone(fresh, {transfer: [fresh]}); \
                 [t instanceof OffscreenCanvas, t.width, t.height, fresh.width, fresh.height, \
                  err(() => fresh.getContext('2d')), err(() => { fresh.width = 5; }), err(() => fresh.transferToImageBitmap()), \
                  err(() => structuredClone(fresh, {transfer: [fresh]})), t.getContext('2d') !== null].join()"),
        "true,3,2,0,0,InvalidStateError,InvalidStateError,InvalidStateError,DataCloneError,true"
    );
    // Nothing is detached when the clone fails.
    assert_eq!(
        eval(r, "var keep = new OffscreenCanvas(2, 2); [err(() => structuredClone({keep, f() {}}, {transfer: [keep]})), keep.width].join()"),
        "DataCloneError,2"
    );
}

#[test]
fn a_canvas_whose_control_went_offscreen_shows_its_frames() {
    let e = env("<!doctype html><canvas id=c width=8 height=4></canvas><canvas id=d width=8 height=4></canvas>");
    let r = &e.main;
    let error = "var err = (f) => { try { f(); return 'ok'; } catch (e) { return e.name; } };";
    eval(r, error);
    assert_eq!(
        eval(
            r,
            "var c = document.getElementById('c'); var o = c.transferControlToOffscreen(); \
                 [o instanceof OffscreenCanvas, o.width, o.height, err(() => c.getContext('2d')), \
                  err(() => c.transferControlToOffscreen())].join()"
        ),
        "true,8,4,InvalidStateError,InvalidStateError"
    );
    // A canvas with a context cannot give its control away.
    assert_eq!(
        eval(r, "var d = document.getElementById('d'); d.getContext('2d'); err(() => d.transferControlToOffscreen())"),
        "InvalidStateError"
    );
    // Nothing drawn: nothing shown.
    e.page.after_task();
    assert!(e.page.canvas_bitmaps().is_empty());
    // A frame is committed after the task that drew it.
    eval(r, "var ox = o.getContext('2d'); ox.fillStyle = 'red'; ox.fillRect(0, 0, 1, 1);");
    assert!(e.page.canvas_bitmaps().is_empty());
    e.page.after_task();
    let bitmaps = e.page.canvas_bitmaps();
    assert_eq!(bitmaps.len(), 1);
    let (_, w, h, rgba) = &bitmaps[0];
    assert_eq!((*w, *h), (8, 4));
    assert_eq!(&rgba[..8], &[255, 0, 0, 255, 0, 0, 0, 0]);
    // An unchanged frame is the same bitmap; drawImage and toDataURL read it.
    e.page.after_task();
    assert!(std::sync::Arc::ptr_eq(&e.page.canvas_bitmaps()[0].3, rgba));
    assert_eq!(
        eval(r, "var dx = d.getContext('2d'); dx.drawImage(c, 0, 0); [Array.from(dx.getImageData(0, 0, 1, 1).data).join(), c.toDataURL().length > 30].join('|')"),
        "255,0,0,255|true"
    );
    // A resize of the OffscreenCanvas is a new (cleared) frame of its size.
    eval(r, "o.width = 2;");
    e.page.after_task();
    let bitmaps = e.page.canvas_bitmaps();
    assert!(bitmaps.iter().all(|(_, w, _, _)| *w != 2), "a cleared frame has no pixels");
}
