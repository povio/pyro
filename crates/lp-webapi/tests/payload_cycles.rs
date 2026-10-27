//! Payload-owned `Persistent` cycles (default features): values a wrapper's native payload owns
//! live in GC-traced hidden slots, so a closed page's realm is collectable even while such a
//! wrapper is still reachable from it. Retention is observed with a sentinel host object on the
//! realm's global whose payload drop counter must reach 1 after the page closes and two
//! collections; a negative control (a payload holding a `Persistent`, the pre-fix shape) must NOT
//! drop it, which shows the test can fail.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard};

use lp_dom::parse::{parse_document, DomRef};
use lp_dom::Dom;
use std::cell::Cell;

use lp_js::{ClassDef, Engine, JsClass, NoParent, Persistent, Realm, RealmOptions};
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

/// Host object whose payload drop is observable.
struct SentinelData(Rc<Cell<u32>>);
impl Drop for SentinelData {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}
struct Sentinel;
impl JsClass for Sentinel {
    const NAME: &'static str = "Sentinel";
    type Data = SentinelData;
    type Parent = NoParent;
    fn define(_def: &mut ClassDef<Self>) {}
}

/// The pre-fix shape: a payload that owns a `Persistent` to a same-realm object.
struct HolderData(RefCell<Option<Persistent>>);
struct Holder;
impl JsClass for Holder {
    const NAME: &'static str = "PersistentHolder";
    type Data = HolderData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, _a| s.wrap_new::<Holder>(HolderData(RefCell::new(None)), nt, None));
        def.method("hold", 1, |s, d, a| {
            *d.0.borrow_mut() = Some(Persistent::new(s, a.get(0)));
            Ok(s.undefined())
        });
    }
}

/// Collections run one test at a time: with several engines collecting concurrently in one
/// process the retention checks flake (15 of 15 serial runs pass, about half of the parallel
/// runs fail), so every Lab holds this lock for its whole life.
static SERIAL: Mutex<()> = Mutex::new(());

struct Lab {
    engine: Engine,
    window_class: lp_js::ClassId,
    /// Declared last so it is released after the engine is dropped.
    _serial: MutexGuard<'static, ()>,
}

impl Lab {
    fn new() -> Lab {
        let serial = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let engine = Engine::new().expect("engine");
        register(&engine);
        let window_class = engine.register::<TestWindow>();
        engine.register::<Sentinel>();
        engine.register::<Holder>();
        Lab { engine, window_class, _serial: serial }
    }

    fn open(&self, html: &str) -> (Rc<PageDom>, Realm) {
        let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
        let doc = parse_document(&dom, "http://localhost:5173/app", html);
        dom.borrow_mut().set_document_active(doc, true).unwrap();
        let page = PageDom::new(dom, doc);
        let mut vp = page.viewport();
        vp.width = 1440;
        vp.height = 900;
        page.set_viewport(vp);
        page.layout.borrow_mut().style_mut().discover_style_elements(&page.dom.borrow());
        let realm = new_realm(&self.engine, self.window_class, &page, "");
        (page, realm)
    }

    /// Production close order (the browser's `close_page`): page, realm bookkeeping, realm.
    fn close(&self, page: &Rc<PageDom>, realm: &Realm) {
        page.dispose();
        lifecycle::realm_disposed(page, realm.id());
        realm.dispose();
    }

    /// Bounded collection rounds, stopping as soon as `done` holds. A round is a stack scrub,
    /// a synchronous full collection and a drain: the first collection finalizes wrappers, the
    /// drain unprotects their roots, a later one collects the realm. JSC scans the native stack
    /// conservatively, so a stale pointer in a dead slot can delay a collection by a round;
    /// the repo's other GC tests loop the same way. A real cycle never completes, however many
    /// rounds run, which the negative control shows.
    fn settle(&self, done: impl Fn() -> bool) {
        self.engine.drain_finalized();
        for _ in 0..ROUNDS {
            if done() {
                return;
            }
            scrub_stack();
            assert!(self.engine.collect_garbage_now(), "synchronous GC SPI unavailable");
            self.engine.drain_finalized();
        }
    }

    /// GCs while the realm is still live (hidden-slot proof).
    fn gc_live(&self) {
        for _ in 0..3 {
            self.engine.collect_garbage_now();
            self.engine.drain_finalized();
        }
    }
}

/// Overwrites the dead part of the stack below the caller: the collector scans the stack
/// conservatively, and stale pointers left by earlier deep calls (script evaluation) in
/// not-yet-overwritten slots of the GC call chain would otherwise keep a closed realm alive.
/// Collection rounds before a closed realm is declared retained.
const ROUNDS: usize = 6;

#[inline(never)]
fn scrub_stack() {
    let mut buf = [0usize; 96 * 1024];
    for slot in buf.iter_mut() {
        // SAFETY: a valid, exclusive slot; volatile so the zeroing is not optimized away.
        unsafe { std::ptr::write_volatile(slot, 0) };
    }
    std::hint::black_box(&buf);
}

#[inline(never)]
fn plant_sentinel(realm: &Realm, drops: &Rc<Cell<u32>>) {
    let s = realm.scope().unwrap();
    let o = s.wrap::<Sentinel>(SentinelData(drops.clone()), None).unwrap();
    s.global().set("__sentinel", o.value()).unwrap();
}

#[inline(never)]
fn run(realm: &Realm, src: &str) {
    let s = realm.scope().unwrap();
    if let Err(e) = s.eval(src, Some("control.js"), 1) {
        panic!("script failed: {}", e.details(&s).text());
    }
}

/// Opens a page, plants the sentinel, runs `script` and closes the page. Its own frame is gone
/// before any collection runs, so its dead stack slots cannot pin the closed realm.
#[inline(never)]
fn open_run_close(lab: &Lab, drops: &Rc<Cell<u32>>, script: &str) {
    let (page, realm) = lab.open("<body>");
    plant_sentinel(&realm, drops);
    run(&realm, script);
    lab.close(&page, &realm);
}

/// Returns how many sentinels dropped once the closed realm had its bounded collection rounds.
fn closed_realm_drops(script: &str) -> u32 {
    let lab = Lab::new();
    let drops = Rc::new(Cell::new(0u32));
    open_run_close(&lab, &drops, script);
    lab.settle(|| drops.get() == 1);
    drops.get()
}

const KEEP_FILELIST: &str = "(function () { const dt = new DataTransfer(); dt.items.add(new File(['abc'], 'a.txt')); \
    const input = document.createElement('input'); input.type = 'file'; document.body.appendChild(input); \
    input.files = dt.files; globalThis.__list = input.files; })()";

#[test]
fn filelist_semantics_are_unchanged() {
    let lab = Lab::new();
    let (_page, realm) = lab.open("<body><input id=i type=file>");
    let out = eval(
        &realm,
        r#"(() => {
            const dt = new DataTransfer();
            dt.items.add(new File(['abc'], 'a.txt', { type: 'text/plain' }));
            dt.items.add(new File(['de'], 'b.csv'));
            const input = document.getElementById('i');
            input.files = dt.files;
            const l = input.files;
            const names = [];
            for (let i = 0; i < l.length; i++) names.push(l[i].name);
            return [l.length, l.item(0).name, l[1].name, l[2] === undefined, l.item(2) === null,
                    l[0] === l[0], l.item(0) === l[0], Array.from(l).map(f => f.name).join('+'),
                    [...l].length, names.join(), l instanceof FileList, l[0] instanceof File,
                    input.files !== input.files, dt.files !== dt.files, dt.files.length,
                    l[0].size, l[1].size].join('|');
        })()"#,
    );
    assert_eq!(out, "2|a.txt|b.csv|true|true|true|true|a.txt+b.csv|2|a.txt,b.csv|true|true|true|true|2|3|2");
    // input.files = list round trip, and replacing it with a smaller list
    assert_eq!(
        eval(
            &realm,
            "const i2 = document.getElementById('i'); const d2 = new DataTransfer(); d2.items.add(new File(['x'], 'c.txt')); \
             i2.files = d2.files; [i2.files.length, i2.files[0].name].join()"
        ),
        "1,c.txt"
    );
}

#[test]
fn filelist_files_survive_gc_through_the_hidden_slot_only() {
    let lab = Lab::new();
    let (_page, realm) = lab.open("<body>");
    run(&realm, KEEP_FILELIST);
    // The FileList wrapper is reachable only from the global; its File wrappers only through the hidden array.
    lab.gc_live();
    assert_eq!(eval(&realm, "[__list.length, __list[0].name, __list.item(0).size].join()"), "1,a.txt,3");
    lab.gc_live();
    assert_eq!(eval(&realm, "__list[0].name"), "a.txt");
}

#[test]
fn filelist_hidden_keys_are_invisible() {
    let lab = Lab::new();
    let (_page, realm) = lab.open("<body>");
    let out = eval(
        &realm,
        r#"(() => {
            const dt = new DataTransfer(); dt.items.add(new File(['a'], 'a.txt'));
            const l = dt.files;
            const forIn = []; for (const k in l) forIn.push(k);
            return [Object.getOwnPropertyNames(l).some(k => k.startsWith('__lp')),
                    Reflect.ownKeys(l).some(k => String(k).startsWith('__lp')),
                    JSON.stringify(l).includes('__lp'), forIn.some(k => k.startsWith('__lp'))].join();
        })()"#,
    );
    assert_eq!(out, "false,false,false,false");
}

#[test]
fn filelist_cross_realm_round_trip() {
    let lab = Lab::new();
    let (page, a) = lab.open("<body>");
    let b = new_realm(&lab.engine, lab.window_class, &page, "other");
    run(&a, KEEP_FILELIST);
    {
        let sa = a.scope().unwrap();
        let list = sa.global().get("__list").unwrap();
        let sb = b.scope().unwrap();
        sb.global().set("__listA", list).unwrap();
    }
    // A list made in realm A assigned to an input through realm B, read back in B, after GCs.
    lab.gc_live();
    assert_eq!(
        eval(
            &b,
            "const inp = document.createElement('input'); inp.type = 'file'; document.body.appendChild(inp); \
             inp.files = __listA; [inp.files.length, inp.files[0].name, __listA[0].name].join()"
        ),
        "1,a.txt,a.txt"
    );
}

#[test]
fn filelist_kept_in_a_closed_page_does_not_retain_the_realm() {
    assert_eq!(
        closed_realm_drops(KEEP_FILELIST),
        1,
        "closed realm survived the bounded collection rounds (FileList cycle)"
    );
}

#[test]
fn negative_control_a_payload_persistent_retains_the_closed_realm() {
    // The pre-fix shape: the sentinel must NOT drop, which shows the retention tests can fail.
    assert_eq!(
        closed_realm_drops("globalThis.__holder = new PersistentHolder(); __holder.hold({ marker: 1 });"),
        0,
        "negative control did not retain the realm: the sentinel test cannot detect a cycle"
    );
}

#[inline(never)]
fn start_drag_and_close(lab: &Lab, drops: &Rc<Cell<u32>>) {
    use lp_webapi::input::{dispatch_mouse_intercepting_drags, MouseButton, MouseInput, MouseKind};
    let (page, realm) =
        lab.open("<body style='margin:0'><div id=s draggable=true style='width:100px;height:40px'>drag me</div>");
    plant_sentinel(&realm, drops);
    // A node wrapper kept on the global pins the PageDom from inside the realm, as every real page does.
    run(&realm, "globalThis.__pin = document.getElementById('s')");
    let press =
        MouseInput { x: 10.0, y: 30.0, button: MouseButton::Left, buttons: 1, click_count: 1, ..Default::default() };
    dispatch_mouse_intercepting_drags(&page, MouseKind::Pressed, &press, true).expect("press");
    let moved = MouseInput { x: 60.0, y: 45.0, button: MouseButton::Left, buttons: 1, ..Default::default() };
    let dragged = dispatch_mouse_intercepting_drags(&page, MouseKind::Moved, &moved, true).expect("move");
    assert!(dragged.is_some(), "the native drag did not start; the test cannot observe the drag root");
    lab.close(&page, &realm);
}

#[test]
fn drag_in_flight_at_dispose_does_not_retain_the_realm() {
    let lab = Lab::new();
    let drops = Rc::new(Cell::new(0u32));
    start_drag_and_close(&lab, &drops);
    lab.settle(|| drops.get() == 1);
    assert_eq!(drops.get(), 1, "a drag in flight at dispose retained the closed realm");
}

// ---- AbortSignal.reason ----

/// A signal aborted with a reason that reaches back to the signal, kept on the global: the shape
/// that rooted the realm when the reason was a `Persistent` in the signal's payload.
const KEEP_ABORTED_SIGNAL: &str = "(function () { const c = new AbortController(); const reason = { signal: c.signal, deep: { n: 1 } }; \
    c.abort(reason); globalThis.__reason = reason; globalThis.__sig = c.signal; })()";

/// The same through `AbortSignal.any`, whose abort step reads the source's reason.
const KEEP_ANY_SIGNAL: &str = "(function () { const c = new AbortController(); const any = AbortSignal.any([c.signal]); \
    const reason = { signal: any }; c.abort(reason); globalThis.__any = any; globalThis.__src = c; })()";

#[test]
fn abortsignal_semantics_are_unchanged() {
    let lab = Lab::new();
    let (_page, realm) = lab.open("<body>");
    let out = eval(
        &realm,
        r#"(() => {
            const out = [];
            const c = new AbortController(); const s = c.signal;
            out.push(s.aborted, s.reason === undefined);
            s.throwIfAborted();
            let seen; s.addEventListener('abort', e => { seen = e.target.reason; });
            c.abort();
            const r = s.reason;
            out.push(s.aborted, r instanceof DOMException, r.name, s.reason === r, seen === r);
            let thrown; try { s.throwIfAborted(); } catch (e) { thrown = e; }
            out.push(thrown === r);
            c.abort('later');
            out.push(s.reason === r);
            const custom = { a: 1 }; const c2 = new AbortController(); c2.abort(custom);
            out.push(c2.signal.reason === custom, c2.signal.reason.a);
            out.push(AbortSignal.abort('str').reason, AbortSignal.abort(null).reason === null, AbortSignal.abort(0).reason);
            out.push(AbortSignal.abort().reason.name);
            const c3 = new AbortController(); const anyLater = AbortSignal.any([c3.signal]);
            const tag = { t: 1 }; c3.abort(tag);
            out.push(anyLater.aborted, anyLater.reason === tag);
            const anyNow = AbortSignal.any([c2.signal]);
            out.push(anyNow.aborted, anyNow.reason === custom);
            out.push(AbortSignal.any([new AbortController().signal]).reason === undefined);
            return out.join('|');
        })()"#,
    );
    assert_eq!(
        out,
        "false|true|true|true|AbortError|true|true|true|true|true|1|str|true|0|AbortError|true|true|true|true|true"
    );
}

#[test]
fn abortsignal_reason_survives_gc_through_the_hidden_slot_only() {
    let lab = Lab::new();
    let (_page, realm) = lab.open("<body>");
    // The reason is reachable only from the signal's hidden slot: the controller and every other
    // reference are gone once the function returns.
    run(
        &realm,
        "globalThis.__sig = (function () { const c = new AbortController(); c.abort({ marker: { deep: 42 } }); return c.signal; })();\
         globalThis.__any = (function () { const c = new AbortController(); const a = AbortSignal.any([c.signal]); c.abort({ via: 'any' }); return a; })();",
    );
    lab.gc_live();
    assert_eq!(eval(&realm, "[__sig.reason.marker.deep, __any.reason.via].join()"), "42,any");
    lab.gc_live();
    assert_eq!(eval(&realm, "[__sig.reason.marker.deep, __any.reason.via, __sig.reason === __sig.reason].join()"), "42,any,true");
}

#[test]
fn abortsignal_hidden_keys_are_invisible() {
    let lab = Lab::new();
    let (_page, realm) = lab.open("<body>");
    let out = eval(
        &realm,
        r#"(() => {
            const c = new AbortController(); c.abort({ visible: true });
            const probe = o => { const forIn = []; for (const k in o) forIn.push(k);
                return [Object.getOwnPropertyNames(o).some(k => k.startsWith('__lp')),
                        Reflect.ownKeys(o).some(k => String(k).startsWith('__lp')),
                        JSON.stringify(o).includes('__lp'), forIn.some(k => k.startsWith('__lp'))].join(); };
            return [probe(c.signal), probe(c)].join('/');
        })()"#,
    );
    assert_eq!(out, "false,false,false,false/false,false,false,false");
}

#[test]
fn abortsignal_cross_realm_any_and_reason() {
    let lab = Lab::new();
    let (page, a) = lab.open("<body>");
    let b = new_realm(&lab.engine, lab.window_class, &page, "other");
    run(&a, "globalThis.__ctrl = new AbortController(); globalThis.__pre = AbortSignal.abort({ pre: 1 });");
    {
        let sa = a.scope().unwrap();
        let (sig, pre) = (sa.global().get("__ctrl").unwrap(), sa.global().get("__pre").unwrap());
        let sb = b.scope().unwrap();
        sb.global().set("__ctrlA", sig).unwrap();
        sb.global().set("__preA", pre).unwrap();
    }
    // `any` in realm B over a signal of realm A: aborted now, and aborted later.
    run(&b, "globalThis.__anyNow = AbortSignal.any([__preA]); globalThis.__anyLater = AbortSignal.any([__ctrlA.signal]);");
    lab.gc_live();
    run(&a, "__ctrl.abort({ late: 2 });");
    lab.gc_live();
    assert_eq!(
        eval(
            &b,
            "[__anyNow.reason === __preA.reason, __anyNow.reason.pre, __anyLater.aborted, \
              __anyLater.reason === __ctrlA.signal.reason, __anyLater.reason.late].join()"
        ),
        "true,1,true,true,2"
    );
}

#[test]
fn abortsignal_kept_in_a_closed_page_does_not_retain_the_realm() {
    assert_eq!(
        closed_realm_drops(KEEP_ABORTED_SIGNAL),
        1,
        "closed realm survived the bounded collection rounds (AbortSignal.reason cycle)"
    );
}

#[test]
fn abortsignal_any_kept_in_a_closed_page_does_not_retain_the_realm() {
    assert_eq!(
        closed_realm_drops(KEEP_ANY_SIGNAL),
        1,
        "closed realm survived the bounded collection rounds (AbortSignal.any reason cycle)"
    );
}
