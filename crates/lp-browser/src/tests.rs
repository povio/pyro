use super::*;
use lp_js::{ConsoleLevel, Value};
use std::time::Duration;

#[derive(Default)]
struct Recorder {
    events: RefCell<Vec<String>>,
    console: RefCell<Vec<String>>,
    errors: RefCell<Vec<String>>,
}

impl PageObserver for Recorder {
    fn on_page_event(&self, _page: PageId, event: &PageEvent) {
        let name = match event {
            PageEvent::FrameNavigate { .. } => "navigate".to_string(),
            PageEvent::FrameNavigated { .. } => "navigated".to_string(),
            PageEvent::FrameNavigateFailed { error, .. } => format!("failed:{error}"),
            PageEvent::RealmCreated { world, .. } => format!("realm:{}", world.as_deref().unwrap_or("main")),
            PageEvent::RealmDestroyed { .. } => "realm-destroyed".to_string(),
            PageEvent::FrameDomContentLoaded { .. } => "dcl".to_string(),
            PageEvent::FrameLoaded { .. } => "load".to_string(),
            PageEvent::FrameNetworkAlmostIdle { .. } => "almost-idle".to_string(),
            PageEvent::FrameNetworkIdle { .. } => "idle".to_string(),
            other => format!("{other:?}"),
        };
        self.events.borrow_mut().push(name);
    }
}

impl EngineHooks for Recorder {
    fn console_api(&self, _realm: &Realm, level: ConsoleLevel, args: &[Value<'_>]) {
        let parts: Vec<String> = args.iter().map(|a| a.to_string().unwrap_or_default()).collect();
        self.console.borrow_mut().push(format!("{}:{}", level.cdp_type(), parts.join(" ")));
    }
    fn exception_thrown(&self, _realm: &Realm, e: &lp_js::JsException) {
        self.errors.borrow_mut().push(e.text());
    }
}

fn setup() -> (BrowserThread, Rc<Recorder>, PageId) {
    let mut t = BrowserThread::new().unwrap();
    let rec = Rc::new(Recorder::default());
    t.browser.set_observer(Some(rec.clone()));
    t.browser.set_engine_hooks(Some(rec.clone()));
    let page = t.browser.create_page().unwrap();
    (t, rec, page)
}

fn eval(t: &BrowserThread, page: PageId, src: &str) {
    let realm = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
    let s = realm.scope().unwrap();
    if let Err(e) = s.eval(src, Some("test.js"), 1) {
        s.report_exception(&e);
    }
}

/// Runs the loop until the network idle milestones of the current document
/// (reported a quiet window after `load`) have been observed.
fn until_network_idle(t: &mut BrowserThread, rec: &Recorder) {
    let r = t.run_until(Some(Instant::now() + Duration::from_secs(5)), |_| {
        rec.events.borrow().last().is_some_and(|e| e == "idle")
    });
    assert_eq!(r, lp_runtime::RunOutcome::Done, "{:?}", rec.events.borrow());
}

#[test]
fn initial_navigation_event_order() {
    let (mut t, rec, _page) = setup();
    assert_eq!(*rec.events.borrow(), ["navigate", "realm:main", "navigated", "dcl", "load"]);
    until_network_idle(&mut t, &rec);
    assert_eq!(*rec.events.borrow(), ["navigate", "realm:main", "navigated", "dcl", "load", "almost-idle", "idle"]);
}

/// Elapsed ms after each of ten chained animation frames.
fn ten_frames_ms(t: &mut BrowserThread, rec: &Recorder, page: PageId) -> Vec<f64> {
    rec.console.borrow_mut().clear();
    eval(
        t,
        page,
        "{ const start = performance.now(); const at = [];
           const step = () => { at.push(performance.now() - start);
             if (at.length < 10) requestAnimationFrame(step); else console.log('frames ' + JSON.stringify(at)); };
           requestAnimationFrame(step); }",
    );
    let r = t.run_until_idle(Some(Instant::now() + Duration::from_secs(5)));
    assert_eq!(r, RunOutcome::Idle);
    let log = rec.console.borrow();
    let line = log.iter().find_map(|l| l.strip_prefix("log:frames ")).expect("frames ran");
    line.trim_matches(['[', ']']).split(',').map(|n| n.parse().unwrap()).collect()
}

#[test]
fn animation_frames_run_every_5_ms_without_a_client() {
    let (mut t, rec, page) = setup();
    let ms = ten_frames_ms(&mut t, &rec, page);
    // Loop timers never fire early: ten frames take at least 10 x 5 ms.
    assert!(ms[9] >= 49.0, "idle frames came too fast: {ms:?}");
}

#[test]
fn animation_frames_run_once_idle_after_a_client_command_then_on_the_clock() {
    let (mut t, rec, page) = setup();
    crate::note_client_activity();
    let ms = ten_frames_ms(&mut t, &rec, page);
    // The first frames run as soon as nothing else is ready; the 5 ms
    // cadence would need 20 ms for four.
    assert!(ms[3] < 10.0, "eager frames too slow: {ms:?}");
    // A perpetual loop then falls back to the clock: six more 5 ms frames.
    assert!(ms[9] - ms[3] >= 29.0, "frames kept running eagerly: {ms:?}");
}

#[test]
fn animation_frame_callbacks_share_a_frame_and_timestamp() {
    let (mut t, rec, page) = setup();
    eval(
        &t,
        page,
        "const seen = [];
         requestAnimationFrame(ts => seen.push(['a', ts]));
         const b = requestAnimationFrame(() => seen.push(['b']));
         requestAnimationFrame(ts => { seen.push(['c', ts]); cancelAnimationFrame(d);
           requestAnimationFrame(ts2 => { seen.push(['next', ts2 > ts]);
             console.log(JSON.stringify(seen.map(e => e[0])), seen[0][1] === seen[1][1], seen[2][1]); }); });
         const d = requestAnimationFrame(() => seen.push(['d']));
         cancelAnimationFrame(b);
         clearTimeout(requestAnimationFrame(() => seen.push(['cleared'])));",
    );
    let r = t.run_until_idle(Some(Instant::now() + Duration::from_secs(5)));
    assert_eq!(r, RunOutcome::Idle);
    assert!(
        rec.console.borrow().contains(&r#"log:["a","c","next"] true true"#.to_string()),
        "{:?}",
        rec.console.borrow()
    );
}

#[test]
fn local_storage_seed_applies_to_existing_and_new_contexts() {
    let (mut t, _rec, _page) = setup();
    let seed =
        lp_webapi::StorageSeed::new(vec![("http://app.test:3000".into(), vec![("token".into(), "t".into())])]).unwrap();
    t.set_local_storage_seed(seed);
    let fresh = t.browser.create_context().unwrap();
    for context in [DEFAULT_CONTEXT, fresh] {
        let namespace = &t.browser.contexts.iter().find(|c| c.id == context).unwrap().local_storage;
        assert_eq!(namespace.get("http://app.test:3000", "token").as_deref(), Some("t"), "{context:?}");
        assert_eq!(namespace.get("http://app.test:3000", "other"), None);
    }
    // Contexts are independent copies: a write in one is invisible in the other.
    let default = t.browser.contexts.iter().find(|c| c.id == DEFAULT_CONTEXT).unwrap().local_storage.clone();
    default.seed(
        &lp_webapi::StorageSeed::new(vec![("http://app.test:3000".into(), vec![("x".into(), "1".into())])]).unwrap(),
    );
    let other = &t.browser.contexts.iter().find(|c| c.id == fresh).unwrap().local_storage;
    assert_eq!(other.get("http://app.test:3000", "x"), None);
}

#[test]
fn static_files_load_on_the_browser_thread_with_network_events() {
    let dir = std::env::temp_dir().join(format!("lp-browser-static-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(
        dir.join("index.html"),
        "<link rel=stylesheet href=/assets/s.css><script src=/assets/a.js></script><p id=p>hi</p>",
    )
    .unwrap();
    std::fs::write(dir.join("assets/a.js"), "console.log('static:' + location.pathname)").unwrap();
    std::fs::write(dir.join("assets/s.css"), "p { color: red }").unwrap();
    // Port 1: nothing listens; every byte must come from memory.
    let origin = "http://127.0.0.1:1";
    let config = lp_net::NetConfig {
        static_files: lp_net::StaticFiles::new(vec![(origin.into(), dir.clone())]).unwrap(),
        static_direct: true,
        ..lp_net::NetConfig::default()
    };
    let (mut t, rec, page) = setup();
    t.enable_network(config).unwrap();
    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = events.clone();
    let subscription = t.browser.context_net(DEFAULT_CONTEXT).unwrap().subscribe();
    t.browser.navigate(page, NavigateOpts::address_bar(format!("{origin}/app/route"))).unwrap();
    until_network_idle(&mut t, &rec);
    while let Ok(ev) = subscription.try_recv() {
        sink.lock().unwrap().push(match ev {
            lp_net::NetEvent::RequestWillBeSent { request, .. } => format!("sent:{}", request.url.path()),
            lp_net::NetEvent::ResponseReceived { head, .. } => format!("response:{}:{}", head.url.path(), head.mime),
            lp_net::NetEvent::LoadingFinished { encoded_len, .. } => format!("finished:{encoded_len}"),
            lp_net::NetEvent::DataReceived { .. } => "data".into(),
            lp_net::NetEvent::LoadingFailed { error, .. } => format!("failed:{error}"),
            other => format!("{other:?}"),
        });
    }
    assert!(rec.console.borrow().iter().any(|c| c == "log:static:/app/route"), "{:?}", rec.console.borrow());
    let events = events.lock().unwrap();
    assert!(events.contains(&"sent:/app/route".to_string()), "{events:?}");
    assert!(events.contains(&"response:/app/route:text/html".to_string()), "{events:?}");
    assert!(events.contains(&"response:/assets/a.js:text/javascript".to_string()), "{events:?}");
    assert!(events.contains(&"response:/assets/s.css:text/css".to_string()), "{events:?}");
    assert!(!events.iter().any(|e| e.starts_with("failed:")), "{events:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn timers_run_in_order_and_clear() {
    let (mut t, rec, page) = setup();
    eval(
        &t,
        page,
        "setTimeout(() => console.log('b'), 20);
         setTimeout((x, y) => console.log('a', x, y), 5, 1, 2);
         const c = setTimeout(() => console.log('never'), 1); clearTimeout(c);
         let n = 0; const i = setInterval(() => { console.log('i' + (++n)); if (n === 3) clearInterval(i) }, 2);
         queueMicrotask(() => console.log('micro'));
         setTimeout('console.log(\"string handler\")', 1);
         requestAnimationFrame(ts => console.log('raf', typeof ts));
         console.log('sync');",
    );
    let r = t.run_until_idle(Some(Instant::now() + Duration::from_secs(5)));
    assert_eq!(r, RunOutcome::Idle);
    let log = rec.console.borrow().clone();
    assert_eq!(log[0], "log:sync");
    assert_eq!(log[1], "log:micro");
    for want in ["log:a 1 2", "log:b", "log:i1", "log:i2", "log:i3", "log:string handler", "log:raf number"] {
        assert!(log.contains(&want.to_string()), "missing {want} in {log:?}");
    }
    assert!(!log.iter().any(|l| l.contains("never")));
    let pos = |s: &str| log.iter().position(|l| l == s).unwrap();
    assert!(pos("log:a 1 2") < pos("log:b"));
}

#[test]
fn timer_exceptions_are_reported_and_do_not_stop_loop() {
    let (mut t, rec, page) = setup();
    eval(&t, page, "setTimeout(() => { throw new Error('boom') }, 1); setTimeout(() => console.log('after'), 3);");
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(5)));
    assert_eq!(*rec.errors.borrow(), ["Error: boom"]);
    assert_eq!(*rec.console.borrow(), ["log:after"]);
}

#[test]
fn init_scripts_run_per_world_before_page_scripts() {
    let (mut t, rec, page) = setup();
    t.browser.add_init_script(
        page,
        InitScript { id: "1".into(), source: "globalThis.injected = 'main'".into(), world: None },
    );
    t.browser.add_init_script(
        page,
        InitScript {
            id: "2".into(),
            source: "globalThis.injected = 'utility'".into(),
            world: Some("__playwright_utility_world__".into()),
        },
    );
    rec.events.borrow_mut().clear();
    t.browser.navigate(page, NavigateOpts::address_bar("about:blank")).unwrap();
    until_network_idle(&mut t, &rec);
    assert_eq!(
        *rec.events.borrow(),
        [
            "navigate",
            "realm-destroyed",
            "realm:main",
            "realm:__playwright_utility_world__",
            "navigated",
            "dcl",
            "load",
            "almost-idle",
            "idle"
        ]
    );
    eval(&t, page, "console.log(injected)");
    let world = t.browser.page(page).unwrap().main_frame.world("__playwright_utility_world__").unwrap().clone();
    let s = world.scope().unwrap();
    assert_eq!(s.eval("injected", None, 1).unwrap().to_string().unwrap(), "utility");
    assert_eq!(*rec.console.borrow(), ["log:main"]);
}

#[test]
fn navigation_cancels_old_timers_and_disposes_realm() {
    let (mut t, rec, page) = setup();
    eval(&t, page, "setTimeout(() => console.log('stale'), 5)");
    let old = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
    t.browser.navigate(page, NavigateOpts::address_bar("about:blank")).unwrap();
    assert!(old.is_disposed());
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(2)));
    assert!(rec.console.borrow().is_empty());
}

#[test]
fn unsupported_navigation_is_explicit() {
    let (mut t, rec, page) = setup();
    rec.events.borrow_mut().clear();
    let r = t.browser.navigate(page, NavigateOpts::address_bar("http://127.0.0.1:1/"));
    assert!(matches!(r, Err(lp_js::JsError::Unsupported(_))));
    assert_eq!(rec.events.borrow()[0], "navigate");
    assert!(rec.events.borrow()[1].starts_with("failed:net::ERR_NOT_IMPLEMENTED"));
}

#[test]
fn remote_tasks_reach_browser_state() {
    let (mut t, rec, page) = setup();
    let h = t.handle();
    std::thread::spawn(move || {
        h.post(move |b: &mut Browser| {
            let realm = b.page(page).unwrap().main_frame.main_realm().unwrap().clone();
            realm.scope().unwrap().eval("console.log('from remote')", None, 1).unwrap();
        })
        .unwrap();
    });
    let r = t.run_until(Some(Instant::now() + Duration::from_secs(5)), |_| !rec.console.borrow().is_empty());
    assert_eq!(r, RunOutcome::Done);
    assert_eq!(*rec.console.borrow(), ["log:from remote"]);
}

#[test]
fn window_globals() {
    let (t, rec, page) = setup();
    eval(
        &t,
        page,
        "console.log(window === globalThis, self === window, typeof setTimeout, globalThis instanceof Window, Object.prototype.toString.call(window))",
    );
    assert_eq!(*rec.console.borrow(), ["log:true true function true [object Window]"]);
}

#[test]
fn unhandled_rejection_dispatches_window_event_with_original_values() {
    let (mut t, rec, page) = setup();
    assert!(t.browser.engine.capabilities().unhandled_rejections);
    eval(&t, page, "globalThis.reason = {message:'owned'}; addEventListener('unhandledrejection', event => console.log(event instanceof PromiseRejectionEvent, event.promise === bad, event.reason === reason, event.cancelable, event.bubbles, event.isTrusted)); globalThis.bad = Promise.reject(reason); Promise.reject('handled').catch(() => {});");
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(2)));
    assert_eq!(*rec.console.borrow(), ["log:true true true true false true"]);
}

#[test]
fn promise_rejection_event_constructor_preserves_values() {
    let (t, rec, page) = setup();
    eval(&t, page, "const promise=Promise.resolve();const reason={x:1};const event=new PromiseRejectionEvent('custom',{promise,reason,cancelable:true});event.preventDefault();console.log(event.promise===promise,event.reason===reason,event.defaultPrevented,event.isTrusted);let rejected=false;try{new PromiseRejectionEvent('custom',{})}catch(e){rejected=e instanceof TypeError}console.log(rejected);");
    assert_eq!(*rec.console.borrow(), ["log:true true true false", "log:true"]);
    assert!(rec.errors.borrow().is_empty());
}

#[test]
fn preventing_unhandled_rejection_suppresses_default_reporting() {
    struct Counter(std::cell::Cell<usize>);
    impl EngineHooks for Counter {
        fn unhandled_rejection(&self, _realm: &Realm, _exception: &lp_js::JsException) {
            self.0.set(self.0.get() + 1);
        }
    }
    let (mut t, _rec, page) = setup();
    let count = Rc::new(Counter(std::cell::Cell::new(0)));
    t.browser.set_engine_hooks(Some(count.clone()));
    eval(&t, page, "onunhandledrejection=e=>e.preventDefault();Promise.reject('canceled');");
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(2)));
    assert_eq!(count.0.get(), 0);
    eval(&t, page, "onunhandledrejection=null;Promise.reject('reported');");
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(2)));
    assert_eq!(count.0.get(), 1);
}

/// A page that can start workers (`data:` URLs need the network client only).
fn worker_setup() -> (BrowserThread, Rc<Recorder>, PageId) {
    let (mut t, rec, page) = setup();
    t.enable_network(lp_net::NetConfig::default()).unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar("about:blank")).unwrap();
    until_network_idle(&mut t, &rec);
    (t, rec, page)
}

fn until_console(t: &mut BrowserThread, rec: &Recorder, want: &str) {
    let want = format!("log:{want}");
    let r = t.run_until(Some(Instant::now() + Duration::from_secs(10)), |_| {
        rec.console.borrow().iter().any(|c| c.starts_with(&want))
    });
    assert_eq!(r, lp_runtime::RunOutcome::Done, "{want}: {:?} {:?}", rec.console.borrow(), rec.errors.borrow());
}

fn data_url(script: &str) -> String {
    let encoded: String = percent_encoding::utf8_percent_encode(script, percent_encoding::NON_ALPHANUMERIC).to_string();
    format!("data:text/javascript,{encoded}")
}

#[test]
fn worker_runs_off_the_page_thread_and_messages_structured_clones_both_ways() {
    let (mut t, rec, page) = worker_setup();
    let script = data_url(
        "const cycle = {}; cycle.self = cycle;
         postMessage({ready: [typeof document, typeof window, typeof Worker, self instanceof DedicatedWorkerGlobalScope,
           self instanceof WorkerGlobalScope, self instanceof EventTarget, self === globalThis, name,
           location.protocol, atob(btoa('x')), new TextDecoder().decode(new TextEncoder().encode('é')),
           new URL('a', 'http://h/').href, typeof fetch, typeof structuredClone, typeof performance.now(),
           typeof navigator.userAgent].join()});
         let first = true;
         onmessage = (e) => {
           if (e.data === 'stop') return close();
           const d = e.data;
           const shape = [e.isTrusted, e.constructor.name, e.origin === '', d.m instanceof Map && d.m.get('k'), d.s.has(2),
             d.d.getTime(), d.r.flags, d.n, Object.is(d.z, -0), d.u === undefined, 'u' in d, d.b, d.t[1], d.t.constructor.name,
             d.t.buffer === d.same.buffer, d.c.self === d.c, d.l.charCodeAt(0), Object.getPrototypeOf(d) === Object.prototype,
             d.arr.length, 1 in d.arr, d.big].join();
           const back = new Uint8Array([7, 8, 9]).buffer;
           postMessage({shape, cycle, back}, [back]);
           postMessage(back.byteLength);
           setTimeout((x) => postMessage('timer:' + x), 1, 'arg');
           const id = setInterval(() => { clearInterval(id); postMessage('interval'); }, 1);
         };",
    );
    eval(
        &t,
        page,
        &format!(
            r#"const script = {script:?};
            const w = new Worker(script, {{name: 'wn'}});
            globalThis.w = w;
            const got = [];
            w.onmessage = (e) => {{
                const d = e.data;
                if (d && d.ready) console.log('ready:' + d.ready);
                else if (d && d.shape) {{
                    console.log('shape:' + d.shape);
                    console.log('reply:' + [d.cycle.self === d.cycle, new Uint8Array(d.back).join('-'), d.back.byteLength].join());
                }}
                else {{
                    got.push(d);
                    if (got.length === 3) {{
                        console.log('rest:' + got.join());
                        w.postMessage('stop');
                    }}
                }}
            }};
            w.onerror = (e) => console.log('error:' + e.message);
            const c = {{}}; c.self = c;
            const t = new Uint16Array([1, 2]);
            const held = new ArrayBuffer(4);
            const big = new Uint8Array(70000).fill(1);
            w.postMessage({{m: new Map([['k', 'v']]), s: new Set([2]), d: new Date(5), r: /a/gi, n: NaN, z: -0, u: undefined,
              b: 12345678901234567890n, t, same: t, c, l: '\ud800', arr: [1, , 3], big: big.length, held}}, {{transfer: [held]}});
            console.log('detached:' + held.byteLength);
            try {{ w.postMessage(() => 1); }} catch (e) {{ console.log('clone-error:' + e.name); }}"#
        ),
    );
    until_console(&mut t, &rec, "ready:");
    until_console(&mut t, &rec, "rest:");
    let console = rec.console.borrow().join("\n");
    assert!(console.contains("log:detached:0"), "{console}");
    assert!(console.contains("log:clone-error:DataCloneError"), "{console}");
    assert!(
        console.contains(
            "log:ready:undefined,undefined,undefined,true,true,true,true,wn,data:,x,é,http://h/a,function,function,number,string"
        ),
        "{console}"
    );
    assert!(
        console.contains(
            "log:shape:true,MessageEvent,true,v,true,5,gi,NaN,true,true,true,12345678901234567890,2,Uint16Array,true,true,55296,true,3,false,70000"
        ),
        "{console}"
    );
    assert!(console.contains("log:reply:true,7-8-9,3"), "{console}");
    assert!(console.contains("log:rest:0,timer:arg,interval"), "{console}");
    drop(console);
    assert!(rec.errors.borrow().is_empty(), "{:?}", rec.errors.borrow());
    t.browser.close_all();
    assert!(lp_webapi::join_stopped(Duration::from_secs(5)), "worker threads end");
}

#[test]
fn worker_terminate_stops_a_busy_script_and_errors_reach_the_page() {
    let (mut t, rec, page) = worker_setup();
    let busy = data_url("postMessage('busy'); while (true) {}");
    let thrower = data_url(
        "addEventListener('error', (e) => { postMessage('self-error:' + e.message + ':' + e.lineno); });
         onmessage = () => { throw new Error('in handler'); };
         throw new TypeError('at load');",
    );
    let imports = data_url(&format!(
        "importScripts({:?}); postMessage('imported:' + globalThis.fromImport);
         try {{ importScripts('http://['); }} catch (e) {{ postMessage('bad-url:' + e.name); }}
         try {{ importScripts('http://127.0.0.1:1/x.js'); }} catch (e) {{ postMessage('network:' + e.name); }}",
        data_url("globalThis.fromImport = 41 + 1;")
    ));
    eval(
        &t,
        page,
        &format!(
            r#"const busy = new Worker({busy:?});
            busy.onmessage = () => {{ busy.terminate(); busy.onmessage = () => console.log('after-terminate'); console.log('terminated'); }};
            const thrower = new Worker({thrower:?});
            thrower.onmessage = (e) => console.log(e.data);
            let errors = 0;
            thrower.onerror = (e) => {{
                console.log('page-error:' + e.constructor.name + ':' + e.message + ':' + e.cancelable);
                if (!errors++) thrower.postMessage(1);
            }};
            const imports = new Worker({imports:?});
            imports.onmessage = (e) => console.log(e.data);
            const missing = new Worker('data:,throw new Error("x")');
            missing.onerror = (e) => console.log('syntaxless:' + e.message);
            const revoked = URL.createObjectURL(new Blob(['postMessage("from-blob")']));
            const fromBlob = new Worker(revoked);
            URL.revokeObjectURL(revoked);
            fromBlob.onmessage = (e) => console.log(e.data);
            const gone = new Worker('blob:http://h/none');
            gone.onerror = (e) => console.log('gone:' + e.constructor.name);"#
        ),
    );
    until_console(&mut t, &rec, "terminated");
    until_console(&mut t, &rec, "self-error:Uncaught TypeError: at load:3");
    until_console(&mut t, &rec, "page-error:ErrorEvent:Uncaught TypeError: at load:true");
    until_console(&mut t, &rec, "page-error:ErrorEvent:Uncaught Error: in handler:true");
    until_console(&mut t, &rec, "imported:42");
    until_console(&mut t, &rec, "bad-url:SyntaxError");
    until_console(&mut t, &rec, "network:NetworkError");
    until_console(&mut t, &rec, "syntaxless:Uncaught Error: x");
    until_console(&mut t, &rec, "from-blob");
    until_console(&mut t, &rec, "gone:Event");
    assert!(!rec.console.borrow().iter().any(|c| c.contains("after-terminate")));
    t.browser.close_all();
    assert!(lp_webapi::join_stopped(Duration::from_secs(5)), "worker threads end");
}

#[test]
fn navigating_away_ends_a_documents_workers() {
    let (mut t, rec, page) = worker_setup();
    let script = data_url("postMessage('up'); setInterval(() => postMessage('tick'), 1);");
    eval(
        &t,
        page,
        &format!(
            "const w = new Worker({script:?}); w.onmessage = (e) => {{ if (e.data === 'up') console.log('up'); }};"
        ),
    );
    until_console(&mut t, &rec, "up");
    t.browser.navigate(page, NavigateOpts::address_bar("about:blank")).unwrap();
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(200)));
    // The thread ended with the document.
    assert!(lp_webapi::join_stopped(Duration::from_secs(5)), "worker threads end");
    t.browser.close_all();
}

#[test]
fn a_closed_page_is_released_whether_or_not_close_collected_it() {
    let (mut t, _rec, page) = setup();
    eval(
        &t,
        page,
        "for (let i = 0; i < 2000; i++) document.body.appendChild(document.createElement('div')).id = 'n' + i;",
    );
    assert!(t.browser.close_page(page));
    t.browser.checkpoint();
    // A small page waits for a later collection (serve's loop runs one when idle). One that
    // grew the process past the threshold, say with other tests running, was collected already.
    if t.browser.engine.release_pending() {
        for _ in 0..8 {
            t.browser.engine.collect_garbage_now();
            t.browser.checkpoint();
        }
        assert!(*t.browser.released_payloads.borrow() > 0, "closed page's wrappers were not finalized");
    }
    t.browser.engine.settle();
    assert!(!t.browser.engine.release_pending());
}
