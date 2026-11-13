//! Payload-owned `Persistent` cycles (default features): values a wrapper's native payload owns
//! live in GC-traced hidden slots, so a closed page's realm is collectable even while such a
//! wrapper is still reachable from it. Retention is observed with a sentinel host object on the
//! realm's global whose payload drop counter must reach 1 after the page closes and two
//! collections; a negative control (a payload holding a `Persistent`, the pre-fix shape) must NOT
//! drop it, which shows the test can fail.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard};

use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::Dom;
use std::cell::Cell;

use pyro_js::{ClassDef, Engine, JsClass, NoParent, Persistent, Realm, RealmOptions};
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

fn new_realm(engine: &Engine, class: pyro_js::ClassId, page: &Rc<PageDom>, name: &str) -> Realm {
    new_realm_with_origin(engine, class, page, name, None)
}

fn new_realm_with_origin(
    engine: &Engine,
    class: pyro_js::ClassId,
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
    window_class: pyro_js::ClassId,
    /// Declared last so it is released after the engine is dropped.
    _serial: MutexGuard<'static, ()>,
}

impl Lab {
    fn new() -> Lab {
        Lab::with_classes_first(|_| {})
    }

    /// A lab whose engine registers `first` before the regular class list, which moves every
    /// class id and the order the realm's interfaces are built in.
    fn with_classes_first(first: impl FnOnce(&Engine)) -> Lab {
        let serial = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let engine = Engine::new().expect("engine");
        first(&engine);
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
        pyro_layout::install(std::sync::Arc::new(NoFetch));
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

/// Same check as [`closed_realm_drops`], in a lab that registers `first` before the class list.
fn closed_realm_drops_with(first: fn(&Engine), script: &str) -> u32 {
    let lab = Lab::with_classes_first(first);
    let drops = Rc::new(Cell::new(0u32));
    open_run_close(&lab, &drops, script);
    lab.settle(|| drops.get() == 1);
    drops.get()
}

macro_rules! padding_class {
    ($class:ident) => {
        struct $class;
        impl JsClass for $class {
            const NAME: &'static str = stringify!($class);
            type Data = ();
            type Parent = NoParent;
            fn define(_def: &mut ClassDef<Self>) {}
        }
    };
}
padding_class!(PaddingA);
padding_class!(PaddingB);
padding_class!(PaddingC);

/// Class lists that used to decide whether a closed realm was retained: moving `Touch` and
/// `TouchEvent` changed which helper tiered up last before the close (a finished JIT plan nobody
/// picked up pins its realm, see `Engine::complete_jit_plans`).
#[test]
fn retention_does_not_depend_on_class_registration_order() {
    let orders: [(&str, fn(&Engine)); 4] = [
        ("regular", |_| {}),
        ("shifted by one", |e| {
            e.register::<PaddingA>();
        }),
        ("shifted by three", |e| {
            e.register::<PaddingA>();
            e.register::<PaddingB>();
            e.register::<PaddingC>();
        }),
        ("touch and file classes first, reversed", |e| {
            e.register::<event::TouchEvent>();
            e.register::<event::Touch>();
            e.register::<data_transfer::DataTransfer>();
            e.register::<file::FileList>();
            e.register::<file::File>();
        }),
    ];
    for (name, first) in orders {
        for script in std::iter::once(KEEP_FILELIST).chain(FD_RETENTION.iter().map(|(_, script)| *script)) {
            assert_eq!(closed_realm_drops_with(first, script), 1, "closed realm retained, class order: {name}");
        }
    }
}

/// A function that reaches its first JIT tier just before the page closes: nothing runs afterwards
/// to pick up the finished compilation, which must not keep the realm alive.
#[test]
fn a_function_tiering_up_just_before_close_does_not_retain_the_realm() {
    for calls in [60, 120, 200, 300, 400, 500, 700, 1000] {
        let script = format!(
            "(function () {{ function hot(a) {{ return a + 1; }} let n = 0; \
             const step = () => {{ n = hot(n); }}; for (let i = 0; i < {calls}; i++) step(); \
             globalThis.__hot = hot; }})()"
        );
        assert_eq!(closed_realm_drops(&script), 1, "closed realm retained after {calls} calls");
    }
}

#[inline(never)]
fn start_drag_and_close(lab: &Lab, drops: &Rc<Cell<u32>>) {
    use pyro_webapi::input::{dispatch_mouse_intercepting_drags, MouseButton, MouseInput, MouseKind};
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
const KEEP_ABORTED_SIGNAL: &str =
    "(function () { const c = new AbortController(); const reason = { signal: c.signal, deep: { n: 1 } }; \
    c.abort(reason); globalThis.__reason = reason; globalThis.__sig = c.signal; })()";

/// The same through `AbortSignal.any`, whose abort step reads the source's reason.
const KEEP_ANY_SIGNAL: &str =
    "(function () { const c = new AbortController(); const any = AbortSignal.any([c.signal]); \
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
    assert_eq!(
        eval(&realm, "[__sig.reason.marker.deep, __any.reason.via, __sig.reason === __sig.reason].join()"),
        "42,any,true"
    );
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
    run(
        &b,
        "globalThis.__anyNow = AbortSignal.any([__preA]); globalThis.__anyLater = AbortSignal.any([__ctrlA.signal]);",
    );
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

// ---- FormData files ----

/// Moves a value from one realm's global to another's (same VM).
#[inline(never)]
fn transfer(from: &Realm, to: &Realm, name: &str, as_name: &str) {
    let sf = from.scope().unwrap();
    let v = sf.global().get(name).unwrap();
    let st = to.scope().unwrap();
    st.global().set(as_name, v).unwrap();
}

const FD_SEMANTICS: &str = r##"(() => {
    const out = [];
    const file = new File(['abc'], 'a.txt', { type: 'text/plain' });
    const fd = new FormData();
    fd.append('t', 'text');
    fd.append('f', file);
    fd.append('b', new Blob(['xy'], { type: 'text/csv' }));
    fd.append('g', file, 'x.txt');
    fd.append('n', 5);
    out.push(fd.get('t'), fd.get('f') === file, fd.get('f').name, fd.get('f').size);
    out.push(fd.get('b') instanceof File, fd.get('b').name, fd.get('b').type, fd.get('b').size);
    out.push(fd.get('g') !== file, fd.get('g').name, fd.get('g') instanceof File, fd.get('n'), fd.get('missing'));
    out.push(fd.has('f'), fd.has('nope'));
    // set replaces the first entry in place and drops the duplicates
    fd.append('f', 'dup');
    fd.append('f', file);
    out.push(fd.getAll('f').length);
    fd.set('f', 'only');
    out.push(fd.getAll('f').join('|'), [...fd.keys()].join(','));
    // set with a File keeps identity, with a rename it does not
    const f2 = new File(['z'], 'z.bin');
    fd.set('s', f2);
    out.push(fd.get('s') === f2);
    fd.set('s', f2, 'renamed.bin');
    out.push(fd.get('s') === f2, fd.get('s').name);
    // delete then append reads the right objects (slots are never reused)
    fd.delete('f');
    fd.delete('g');
    const f3 = new File(['third'], 'c.txt');
    fd.append('c', f3);
    out.push([...fd.keys()].join(','), fd.get('c') === f3, fd.get('b').name, fd.get('s').name);
    // the same File twice, then delete one of the names
    const twice = new FormData();
    twice.append('a', file);
    twice.append('b', file);
    twice.delete('a');
    out.push(twice.getAll('b').length, twice.get('b') === file, twice.has('a'));
    // iteration and forEach
    const seen = [];
    fd.forEach(function (v, k) { seen.push(k + ':' + (typeof v === 'string' ? v : v.name) + ':' + (this === fd)); }, fd);
    out.push(seen.join(','));
    out.push([...fd].map(e => e[0]).join(','), [...fd.values()].length, [...fd.entries()].length);
    return out.join('|');
})()"##;

#[test]
fn formdata_semantics_are_unchanged() {
    let lab = Lab::new();
    let (_page, realm) = lab.open("<body>");
    let out = eval(&realm, FD_SEMANTICS);
    assert_eq!(
        out,
        "text|true|a.txt|3|true|blob|text/csv|2|true|x.txt|true|5||true|false|3|only|t,f,b,g,n|true|false|renamed.bin|\
         t,b,n,s,c|true|blob|renamed.bin|1|true|false|t:text:true,b:blob:true,n:5:true,s:renamed.bin:true,c:c.txt:true|\
         t,b,n,s,c|5|5"
    );
}

const FD_FORM_HTML: &str = "<body><form id=f><input name=t value=v><input id=a type=file name=a multiple>\
    <input id=b type=file name=b><input id=e type=file name=e><button id=s name=btn value=go>go</button></form>\
    <form id=g><button id=o name=o>o</button></form>";

#[test]
fn formdata_from_form_with_files_and_submitter() {
    let lab = Lab::new();
    let (_page, realm) = lab.open(FD_FORM_HTML);
    let out = eval(
        &realm,
        r##"(() => {
            const out = [];
            const mk = (names) => { const dt = new DataTransfer(); names.forEach((n, i) => dt.items.add(new File(['x'.repeat(i + 1)], n))); return dt.files; };
            document.getElementById('a').files = mk(['a1.txt', 'a2.txt']);
            document.getElementById('b').files = mk(['b1.csv']);
            const fd = new FormData(document.getElementById('f'), document.getElementById('s'));
            globalThis.__fd = fd;
            out.push([...fd].map(([k, v]) => k + '=' + (typeof v === 'string' ? v : v.name + '/' + v.size)).join('&'));
            out.push(fd.get('a') === fd.get('a'), fd.getAll('a').length, fd.get('e').name === '', fd.get('e').size);
            try { new FormData({}); } catch (e) { out.push(e instanceof TypeError); }
            try { new FormData(document.getElementById('f'), document.getElementById('o')); } catch (e) { out.push(e.name); }
            try { new FormData(document.getElementById('f'), {}); } catch (e) { out.push(e instanceof TypeError); }
            return out.join('|');
        })()"##,
    );
    assert_eq!(out, "t=v&a=a1.txt/1&a=a2.txt/2&b=b1.csv/1&e=/0&btn=go|true|2|true|0|true|NotFoundError|true");
    // The Files exist only inside the FormData: they survive collections and read back the same.
    lab.gc_live();
    lab.gc_live();
    assert_eq!(
        eval(&realm, "[...__fd].map(([k, v]) => typeof v === 'string' ? k : v.name + ':' + v.size).join()"),
        "t,a1.txt:1,a2.txt:2,b1.csv:1,:0,btn"
    );
}

#[test]
fn formdata_hidden_keys_are_invisible() {
    let lab = Lab::new();
    let (_page, realm) = lab.open("<body>");
    let out = eval(
        &realm,
        r#"(() => {
            const fd = new FormData(); fd.append('f', new File(['a'], 'a.txt'));
            const forIn = []; for (const k in fd) forIn.push(k);
            return [Object.getOwnPropertyNames(fd).some(k => k.startsWith('__lp')),
                    Reflect.ownKeys(fd).some(k => String(k).startsWith('__lp')),
                    JSON.stringify(fd).includes('__lp'), forIn.some(k => k.startsWith('__lp'))].join();
        })()"#,
    );
    assert_eq!(out, "false,false,false,false");
}

#[test]
fn formdata_files_survive_gc_through_the_hidden_slot_only() {
    let lab = Lab::new();
    let (_page, realm) = lab.open("<body>");
    // The Files are reachable only through the FormData.
    run(
        &realm,
        "globalThis.__fd = (function () { const fd = new FormData(); fd.append('f', new File(['abc'], 'a.txt')); \
         fd.append('b', new Blob(['xy']), 'b.bin'); fd.append('t', 'text'); return fd; })();",
    );
    lab.gc_live();
    assert_eq!(
        eval(
            &realm,
            "[__fd.get('f').name, __fd.get('f').size, __fd.get('b').name, __fd.get('t'), [...__fd].length].join()"
        ),
        "a.txt,3,b.bin,text,3"
    );
    lab.gc_live();
    assert_eq!(eval(&realm, "[__fd.get('f') === __fd.get('f'), __fd.getAll('b')[0].size].join()"), "true,2");
}

#[test]
fn formdata_delete_and_set_release_the_replaced_files() {
    let lab = Lab::new();
    let (_page, realm) = lab.open("<body>");
    run(
        &realm,
        "globalThis.__fd = (function () { const fd = new FormData(); \
         const a = new File(['a'], 'a.txt'), b = new File(['b'], 'b.txt'), c = new File(['c'], 'c.txt'), d = new File(['d'], 'd.txt'); \
         globalThis.__wa = new WeakRef(a); globalThis.__wb = new WeakRef(b); globalThis.__wc = new WeakRef(c); globalThis.__wd = new WeakRef(d); \
         fd.append('keep', d); fd.append('a', a); fd.append('b', b); fd.append('b', c); return fd; })();",
    );
    run(&realm, "__fd.delete('a'); __fd.set('b', 'now text');");
    let gone = || {
        eval(&realm, "String(__wa.deref() === undefined && __wb.deref() === undefined && __wc.deref() === undefined)")
            == "true"
    };
    lab.settle(gone);
    assert!(gone(), "Files removed by delete/set (and the replaced duplicate) stayed reachable");
    assert_eq!(
        eval(
            &realm,
            "[__wd.deref() !== undefined, __fd.get('keep').name, __fd.get('b'), [...__fd.keys()].join('+')].join()"
        ),
        "true,d.txt,now text,keep+b"
    );
}

#[test]
fn formdata_body_serialization_survives_gc_and_owner_realm_disposal() {
    let lab = Lab::new();
    let (page, a) = lab.open("<body>");
    let b = new_realm(&lab.engine, lab.window_class, &page, "other");
    run(
        &a,
        "globalThis.__fd = (function () { const fd = new FormData(); fd.append('t', 'txt'); \
         fd.append('f', new File(['abc'], 'a.txt', { type: 'text/plain' })); \
         fd.append('g', new Blob(['xyz'], { type: 'application/octet-stream' }), 'g.bin'); return fd; })();",
    );
    transfer(&a, &b, "__fd", "__fdA");
    lab.gc_live();
    let check = r##"(() => { const t = globalThis.__BODY; return [t.includes('name="t"'), t.includes('txt'), t.includes('filename="a.txt"'),
        t.includes('abc'), t.includes('filename="g.bin"'), t.includes('xyz')].join(); })()"##;
    run(&b, "globalThis.__body1 = 'pending'; new Response(__fdA).text().then(t => { globalThis.__body1 = t; });");
    assert_eq!(eval(&b, &check.replace("__BODY", "__body1")), "true,true,true,true,true,true");
    // The creating realm closes while another realm keeps the FormData.
    lifecycle::realm_disposed(&page, a.id());
    a.dispose();
    drop(a);
    lab.gc_live();
    run(
        &b,
        "globalThis.__body2 = 'pending'; new Response(__fdA).text().then(t => { globalThis.__body2 = t; }, e => { globalThis.__body2 = 'ERROR ' + e; });",
    );
    assert_eq!(eval(&b, &check.replace("__BODY", "__body2")), "true,true,true,true,true,true");
}

#[test]
fn formdata_methods_of_a_disposed_realm_throw_instead_of_returning_null() {
    let lab = Lab::new();
    let (page, a) = lab.open("<body>");
    let b = new_realm(&lab.engine, lab.window_class, &page, "other");
    run(&a, "globalThis.__fd = new FormData(); __fd.append('t', 'txt'); __fd.append('f', new File(['abc'], 'a.txt'));");
    transfer(&a, &b, "__fd", "__fdA");
    lifecycle::realm_disposed(&page, a.id());
    a.dispose();
    drop(a);
    lab.gc_live();
    // A FormData whose realm is disposed cannot be used through its methods at all (the class dispatch
    // fails with ContextDestroyed); no method returns a null that would look like a missing entry.
    let out = eval(
        &b,
        r#"(() => { let r = '';
            for (const f of [() => __fdA.get('f'), () => __fdA.getAll('f'), () => [...__fdA], () => __fdA.get('t'),
                             () => __fdA.has('f'), () => [...__fdA.keys()].length]) {
                try { f(); r += 'ok,'; } catch (e) { r += 'threw,'; } }
            return r; })()"#,
    );
    assert_eq!(out, "threw,threw,threw,threw,threw,threw,");
}

#[test]
fn formdata_cross_realm_files_and_forms() {
    let lab = Lab::new();
    let (page, a) = lab.open("<body>");
    let b = new_realm(&lab.engine, lab.window_class, &page, "other");
    run(
        &a,
        "globalThis.__fileA = new File(['aaa'], 'A.txt'); globalThis.__fdA = new FormData(); __fdA.append('own', new File(['o'], 'o.txt'));",
    );
    run(&b, "globalThis.__fdB = new FormData(); __fdB.append('own', new File(['p'], 'p.txt'));");
    transfer(&a, &b, "__fileA", "__fileA");
    transfer(&a, &b, "__fdA", "__fdA");
    transfer(&b, &a, "__fdB", "__fdB");
    lab.gc_live();
    // A's File into B's FormData (call in B); B's FormData modified by A's own method (call in A).
    run(&b, "__fdB.append('fromA', __fileA);");
    run(&a, "FormData.prototype.append.call(__fdB, 'viaA', new File(['v'], 'v.txt'));");
    lab.gc_live();
    assert_eq!(
        eval(
            &b,
            "[__fdB.get('fromA') === __fileA, __fdB.get('fromA').name, __fdB.get('viaA').name, __fdB.get('viaA').size, \
             __fdB.getAll('own').length, __fdA.get('own').name].join()"
        ),
        "true,A.txt,v.txt,1,1,o.txt"
    );
    // A form of the other realm.
    run(
        &b,
        "document.body.innerHTML = '<form id=ff><input name=q value=1><input type=file id=ft name=ft></form>'; \
         (function () { const dt = new DataTransfer(); dt.items.add(new File(['formfile'], 'ff.txt')); document.getElementById('ft').files = dt.files; })(); \
         globalThis.__formB = document.getElementById('ff');",
    );
    transfer(&b, &a, "__formB", "__formB");
    assert_eq!(
        eval(
            &a,
            "globalThis.__fdFromB = new FormData(__formB); [...__fdFromB].map(([k, v]) => k + '=' + (typeof v === 'string' ? v : v.name)).join('&')"
        ),
        "q=1&ft=ff.txt"
    );
    lab.gc_live();
    assert_eq!(eval(&a, "[__fdFromB.get('ft').name, __fdFromB.get('ft').size].join()"), "ff.txt,8");
}

const FD_RETENTION: &[(&str, &str)] = &[
    (
        "plain File, no expando",
        "(function () { const fd = new FormData(); fd.append('f', new File(['abc'], 'a.txt')); globalThis.__fd = fd; })()",
    ),
    (
        "File with an expando reaching the document",
        "(function () { const f = new File(['abc'], 'a.txt'); f.owner = document.body; const fd = new FormData(); \
         fd.append('f', f); globalThis.__fd = fd; globalThis.__f = f; })()",
    ),
    (
        "Blob entry",
        "(function () { const fd = new FormData(); fd.append('b', new Blob(['x']), 'x.bin'); fd.append('c', new Blob(['y'])); \
         globalThis.__fd = fd; })()",
    ),
    (
        "FormData built from a form with a file input",
        "(function () { document.body.innerHTML = '<form id=f><input type=file id=i name=i></form>'; const dt = new DataTransfer(); \
         dt.items.add(new File(['abc'], 'a.txt')); document.getElementById('i').files = dt.files; \
         globalThis.__fd = new FormData(document.getElementById('f')); })()",
    ),
    (
        "after append, delete and set",
        "(function () { const fd = new FormData(); const f = new File(['abc'], 'a.txt'); fd.append('a', f); fd.append('b', f); \
         fd.append('c', new Blob(['z'])); fd.delete('a'); fd.set('b', f, 'renamed.txt'); globalThis.__fd = fd; globalThis.__f = f; })()",
    ),
];

#[test]
fn formdata_kept_in_a_closed_page_does_not_retain_the_realm() {
    // Every variant runs (no early exit) so one log shows exactly which variants leak.
    let leaked: Vec<&str> =
        FD_RETENTION.iter().filter(|(_, script)| closed_realm_drops(script) != 1).map(|(name, _)| *name).collect();
    assert!(leaked.is_empty(), "closed realm survived the bounded collection rounds (FormData variants: {leaked:?})");
}

#[inline(never)]
fn cross_realm_setup(lab: &Lab, da: &Rc<Cell<u32>>, db: &Rc<Cell<u32>>) -> (Rc<PageDom>, Realm) {
    let (page, a) = lab.open("<body>");
    let b = new_realm(&lab.engine, lab.window_class, &page, "other");
    plant_sentinel(&a, da);
    plant_sentinel(&b, db);
    run(&a, "globalThis.__f = new File(['abc'], 'a.txt');");
    transfer(&a, &b, "__f", "__fA");
    run(&b, "globalThis.__fd = new FormData(); __fd.append('f', __fA); delete globalThis.__fA;");
    run(&a, "delete globalThis.__f;");
    // The creating realm of the File closes while the FormData of the other realm lives.
    lifecycle::realm_disposed(&page, a.id());
    a.dispose();
    (page, b)
}

/// Uses the cross-realm FormData while its File's realm is closed and returns what it read. Its own
/// frame is gone before the final collections, so its dead stack slots cannot pin either realm.
#[inline(never)]
fn cross_realm_read(lab: &Lab, b: &Realm, db: &Rc<Cell<u32>>) -> (u32, String) {
    lab.gc_live();
    (db.get(), eval(b, "[__fd.has('f'), typeof __fd.get('f'), __fd.getAll('f').length].join()"))
}

#[test]
fn formdata_cross_realm_file_does_not_leak_after_both_realms_close() {
    let lab = Lab::new();
    let (da, db) = (Rc::new(Cell::new(0u32)), Rc::new(Cell::new(0u32)));
    let (page, b) = cross_realm_setup(&lab, &da, &db);
    // B is still open: its sentinel must live and its FormData must still hold the File of the closed realm.
    assert_eq!(
        cross_realm_read(&lab, &b, &db),
        (0, "true,object,1".to_owned()),
        "the open realm was collected or lost its File"
    );
    lab.close(&page, &b);
    drop(b);
    drop(page);
    lab.settle(|| da.get() == 1 && db.get() == 1);
    assert_eq!((da.get(), db.get()), (1, 1), "a cross-realm FormData File kept a closed realm alive");
}

#[test]
fn formdata_ignores_poisoned_array_and_object_prototype_indices() {
    let lab = Lab::new();
    let (_page, realm) = lab.open("<body>");
    // Indexed accessors on both prototypes: the hidden array has a null prototype, so its slots never see them.
    run(
        &realm,
        "globalThis.__n = 0; for (let i = 0; i < 8; i++) { \
         Object.defineProperty(Array.prototype, String(i), { configurable: true, set(v) { globalThis.__n++; }, get() { return 'poison' + i; } }); \
         Object.defineProperty(Object.prototype, String(i), { configurable: true, set(v) { globalThis.__n++; }, get() { return 'opoison' + i; } }); }",
    );
    let out = eval(
        &realm,
        "(() => { const fd = new FormData(); fd.append('a', new File(['1'], '1.txt')); fd.append('b', new File(['22'], '2.txt')); \
         fd.set('a', new File(['333'], '3.txt')); fd.delete('b'); fd.append('c', new Blob(['4444']), 'c.bin'); globalThis.__fd = fd; \
         return [fd.get('a').size, fd.getAll('c').length, fd.getAll('c')[0].name, [...fd].length, globalThis.__n].join(); })()",
    );
    assert_eq!(out, "3,1,c.bin,2,0");
    run(&realm, "globalThis.__body = 'pending'; new Response(__fd).text().then(t => { globalThis.__body = t; });");
    assert_eq!(
        eval(
            &realm,
            "[__body.includes('333'), __body.includes('4444'), __body.includes('poison'), globalThis.__n].join()"
        ),
        "true,true,false,0"
    );
}

#[test]
fn formdata_prototype_methods_on_a_disposed_realm_formdata_propagate_context_destroyed() {
    let lab = Lab::new();
    let (page, a) = lab.open("<body>");
    let b = new_realm(&lab.engine, lab.window_class, &page, "other");
    run(&a, "globalThis.__fd = new FormData(); __fd.append('t', 'txt'); __fd.append('f', new File(['abc'], 'a.txt'));");
    transfer(&a, &b, "__fd", "__fdA");
    lifecycle::realm_disposed(&page, a.id());
    a.dispose();
    drop(a);
    lab.gc_live();
    // B's own prototype methods run on A's FormData: a File entry cannot be read (ContextDestroyed, never null),
    // text entries and name-only methods still work.
    let out = eval(
        &b,
        r#"(() => { const t = (f) => { try { return String(f()); } catch (e) { return 'threw'; } };
            return [t(() => FormData.prototype.get.call(__fdA, 'f')), t(() => FormData.prototype.getAll.call(__fdA, 'f')),
                    t(() => [...FormData.prototype.entries.call(__fdA)]), t(() => FormData.prototype.get.call(__fdA, 't')),
                    t(() => FormData.prototype.has.call(__fdA, 'f')), t(() => [...FormData.prototype.keys.call(__fdA)].join('+'))].join(','); })()"#,
    );
    assert_eq!(out, "threw,threw,threw,txt,true,t+f");
}

#[test]
fn formdata_survives_script_reentrancy_from_conversions_and_callbacks() {
    let lab = Lab::new();
    let (_page, realm) = lab.open("<body>");
    let out = eval(
        &realm,
        r#"(() => {
            const out = [];
            const fd = new FormData();
            const file = new File(['abc'], 'a.txt');
            fd.append('victim', 'v');
            fd.append('f', file);
            // filename conversion mutates the FormData before the entry is stored
            fd.append('x', file, { toString() { fd.append('re', 'entrant'); fd.delete('victim'); return 'renamed.txt'; } });
            out.push([...fd.keys()].join('+'), fd.get('x').name, fd.get('f') === file);
            // value conversion in set removes the entry being set
            fd.set('re', { toString() { fd.append('mid', 'm'); fd.delete('re'); return 'v2'; } });
            out.push([...fd.keys()].join('+'), fd.get('re'));
            // name conversion mutates
            fd.append({ toString() { fd.delete('f'); return 'n'; } }, 'nv');
            out.push([...fd.keys()].join('+'), fd.has('f'));
            // forEach callback mutates the FormData: iteration is live (deleting the current entry shifts the next one
            // into its place, so it is skipped; an appended entry is reached), values stay readable
            const seen = [];
            fd.forEach((v, k) => {
                if (k === 'x') { fd.delete('x'); fd.append('late', new File(['l'], 'l.txt')); }
                seen.push(k + ':' + (typeof v === 'string' ? v : v.name));
            });
            out.push(seen.join(','), [...fd.keys()].join('+'), fd.get('late').name);
            return out.join('|');
        })()"#,
    );
    assert_eq!(
        out,
        "f+re+x|renamed.txt|true|f+x+mid+re|v2|x+mid+re+n|false|x:renamed.txt,re:v2,n:nv,late:l.txt|mid+re+n+late|l.txt"
    );
}

#[test]
fn formdata_serializes_a_file_whose_own_realm_is_closed() {
    let lab = Lab::new();
    let (page, a) = lab.open("<body>");
    let b = new_realm(&lab.engine, lab.window_class, &page, "other");
    run(&a, "globalThis.__fileA = new File(['aaa'], 'A.txt', { type: 'text/plain' });");
    transfer(&a, &b, "__fileA", "__fileA");
    run(&b, "globalThis.__fdB = new FormData(); __fdB.append('f', __fileA); delete globalThis.__fileA;");
    run(&a, "delete globalThis.__fileA;");
    lifecycle::realm_disposed(&page, a.id());
    a.dispose();
    drop(a);
    lab.gc_live();
    run(
        &b,
        "globalThis.__body = 'pending'; new Response(__fdB).text().then(t => { globalThis.__body = t; }, e => { globalThis.__body = 'ERROR ' + e; });",
    );
    let out = eval(
        &b,
        r##"[__body.includes('filename="A.txt"'), __body.includes('aaa'), __body.startsWith('ERROR')].join()"##,
    );
    assert_eq!(out, "true,true,false");
}

/// Loads no sub-resources for the layout (the tests' pages have none).
struct NoFetch;
impl pyro_render::Fetcher for NoFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err(format!("{url}: offline")));
    }
}

// ---- Node wrappers and the pages they outlive ----

/// Every kind of entry into a node wrapper: pure getters (the per-class fast
/// path), getters returning nodes, pure methods taking names, setters, and
/// methods that mutate.
const PROBE_NODE: &str = r#"(() => { const out = [];
    for (const f of [() => __el.id, () => __el.tagName, () => __el.firstChild && __el.firstChild.nodeName,
                     () => __el.getAttribute('data-k'), () => __el.hasAttribute('data-k'), () => __el.childNodes.length,
                     () => __el.textContent, () => __el.parentNode === null, () => __el.isConnected,
                     () => { __el.className = 'z'; return __el.className; },
                     () => { __el.setAttribute('q', '1'); return __el.getAttribute('q'); },
                     () => __el.appendChild(__el.firstChild) === __el.firstChild,
                     () => { __el.removeAttribute('q'); return __el.hasAttribute('q'); }]) {
        try { out.push(String(f())); } catch (e) { out.push('threw'); } }
    return out.join('|'); })()"#;

/// Opens a page, hands its `#d` to `keeper` and closes the page. Its own
/// frame is gone before any collection runs.
#[inline(never)]
fn keep_node_and_close(lab: &Lab, keeper: &Realm, drops: &Rc<Cell<u32>>) {
    let (page, realm) = lab.open("<body><div id=d data-k=v class=c><span>t</span></div>");
    plant_sentinel(&realm, drops);
    run(&realm, "globalThis.__el = document.getElementById('d'); __el.getAttribute('data-k'); __el.getAttribute('data-k');");
    transfer(&realm, keeper, "__el", "__el");
    lab.close(&page, &realm);
}

/// The wrapper invariant (`NodeData`) across a closed page: the payload owns
/// the page and a reference on the node, so a wrapper another realm kept
/// reads a node that is still there, or throws; it never reads freed memory,
/// collection after collection. Released, it lets the closed realm go.
#[test]
fn a_node_wrapper_kept_after_its_page_closed_stays_sound() {
    let lab = Lab::new();
    let (_keeper_page, keeper) = lab.open("<body>");
    let drops = Rc::new(Cell::new(0u32));
    keep_node_and_close(&lab, &keeper, &drops);
    // The wrapper's realm is disposed: every entry fails in the binding
    // layer, before the node is read.
    let first = eval(&keeper, PROBE_NODE);
    assert_eq!(first, ["threw"; 13].join("|"));
    for _ in 0..4 {
        scrub_stack();
        lab.gc_live();
        // Other pages come and go in between: their nodes and wrappers take
        // whatever memory the closed page gave back.
        let (page, realm) = lab.open("<body><p id=d data-k=other>x</p>");
        run(&realm, "for (let i = 0; i < 200; i++) { const e = document.createElement('i'); e.id = 'n' + i; document.body.appendChild(e); }");
        lab.close(&page, &realm);
        assert_eq!(eval(&keeper, PROBE_NODE), first);
    }
    run(&keeper, "globalThis.__el = null;");
    lab.settle(|| drops.get() == 1);
    assert_eq!(drops.get(), 1, "the closed realm is collected once its node wrapper is released");
}

/// One realm of a page disposed while the page and its other realm live: the
/// disposed realm's wrapper throws, the node it named is untouched, and the
/// live realm's wrapper of the same node works.
#[test]
fn a_node_wrapper_of_a_disposed_realm_throws_while_the_node_lives() {
    let lab = Lab::new();
    let (page, a) = lab.open("<body><div id=d data-k=v class=c><span>t</span></div>");
    let b = new_realm(&lab.engine, lab.window_class, &page, "other");
    run(&a, "globalThis.__el = document.getElementById('d');");
    transfer(&a, &b, "__el", "__el");
    lifecycle::realm_disposed(&page, a.id());
    a.dispose();
    drop(a);
    lab.gc_live();
    assert_eq!(eval(&b, PROBE_NODE), ["threw"; 13].join("|"));
    lab.gc_live();
    assert_eq!(
        eval(&b, "(() => { const d = document.getElementById('d'); return [d === __el, d.id, d.className, d.getAttribute('data-k'), d.hasAttribute('q'), d.firstChild.textContent].join(); })()"),
        "false,d,c,v,false,t"
    );
}

/// A name's entry roots a string, never an object: names used by a page do
/// not keep its closed realm.
#[test]
fn names_resolved_by_string_identity_do_not_retain_the_closed_realm() {
    assert_eq!(
        closed_realm_drops(
            "(function () { const d = document.createElement('div'); document.body.appendChild(d); \
             for (let i = 0; i < 8; i++) { d.setAttribute('data-kept', i); d.getAttribute('data-kept'); d.hasAttribute('data-kept'); \
             d.removeAttribute('data-gone'); document.createElement('section'); } globalThis.__d = d; })()"
        ),
        1,
        "closed realm survived the bounded collection rounds (name table roots)"
    );
}
