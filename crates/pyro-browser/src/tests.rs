use super::*;
use pyro_js::{ConsoleLevel, Value};
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
    fn exception_thrown(&self, _realm: &Realm, e: &pyro_js::JsException) {
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
    assert_eq!(r, pyro_runtime::RunOutcome::Done, "{:?}", rec.events.borrow());
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

/// Animation frames a page gets while 100 timers that are always behind
/// (each busy for 1 ms and set again at once) run 300 times.
fn frames_under_a_timer_backlog(t: &mut BrowserThread, rec: &Recorder, page: PageId) -> u32 {
    eval(
        t,
        page,
        "let ticks = 0, frames = 0;
         const frame = () => { frames++; if (ticks < 300) requestAnimationFrame(frame) };
         requestAnimationFrame(frame);
         const tick = () => {
             const until = performance.now() + 1; while (performance.now() < until) {}
             if (++ticks < 300) setTimeout(tick, 0); else if (ticks === 300) console.log('frames ' + frames);
         };
         for (let i = 0; i < 100; i++) setTimeout(tick, 0);",
    );
    let r = t.run_until_idle(Some(Instant::now() + Duration::from_secs(20)));
    assert_eq!(r, RunOutcome::Idle);
    let log = rec.console.borrow();
    log.iter().find_map(|l| l.strip_prefix("log:frames ")).expect("timers ran").parse().unwrap()
}

#[test]
fn animation_frames_keep_their_cadence_on_a_page_whose_timers_are_always_behind() {
    // A frame is a rendering opportunity between tasks, not one more timer at
    // the back of the queue: in deadline order each frame waited for a whole
    // round of the 100 timers (3 frames in these 300 ms, and a client's click
    // that waits for two or three frames took a second on such a page).
    let (mut t, rec, page) = setup();
    let frames = frames_under_a_timer_backlog(&mut t, &rec, page);
    assert!(frames >= 15, "{frames} frames in 300 ms of timers");
}

/// The same for the frames a client command makes eager (their backup timer).
#[test]
fn eager_animation_frames_keep_their_cadence_on_a_page_whose_timers_are_always_behind() {
    let (mut t, rec, page) = setup();
    crate::note_client_activity();
    let frames = frames_under_a_timer_backlog(&mut t, &rec, page);
    assert!(frames >= 15, "{frames} frames in 300 ms of timers after a client command");
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
        pyro_webapi::StorageSeed::new(vec![("http://app.test:3000".into(), vec![("token".into(), "t".into())])]).unwrap();
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
        &pyro_webapi::StorageSeed::new(vec![("http://app.test:3000".into(), vec![("x".into(), "1".into())])]).unwrap(),
    );
    let other = &t.browser.contexts.iter().find(|c| c.id == fresh).unwrap().local_storage;
    assert_eq!(other.get("http://app.test:3000", "x"), None);
}

/// What layouts load is kept per network identity: the contexts that go out
/// as the browser does share it with the default context; one with a proxy or
/// user agent of its own has its own (also apart from another alike), and no
/// cross-context asset cache answers its requests.
#[test]
fn layout_bodies_and_shared_assets_follow_the_network_identity() {
    let (mut t, _rec, _page) = setup();
    let shared_assets = pyro_net::SharedAssetCache::new(1 << 20, 1 << 20);
    t.enable_network(pyro_net::NetConfig { shared_asset_cache: Some(shared_assets), ..Default::default() }).unwrap();
    let plain = t.browser.create_context().unwrap();
    let agent = || ContextNet { proxy: None, user_agent: Some("Identity/1.0".into()) };
    let (one, two) = (t.browser.create_context_with(agent()).unwrap(), t.browser.create_context_with(agent()).unwrap());
    let proxy = pyro_net::ProxyConfig::parse("http://127.0.0.1:9", None).unwrap();
    let proxied = t.browser.create_context_with(ContextNet { proxy: Some(proxy), user_agent: None }).unwrap();
    let bodies = |id: ContextId| t.browser.contexts.iter().find(|c| c.id == id).unwrap().layout_bodies.clone();
    assert!(std::sync::Arc::ptr_eq(&bodies(DEFAULT_CONTEXT), &bodies(plain)));
    for own in [one, two, proxied] {
        assert!(!std::sync::Arc::ptr_eq(&bodies(DEFAULT_CONTEXT), &bodies(own)), "{own:?}");
        assert!(t.browser.context_net(own).unwrap().config().shared_asset_cache.is_none(), "{own:?}");
    }
    assert!(!std::sync::Arc::ptr_eq(&bodies(one), &bodies(two)), "two identities share nothing, even alike");
    assert!(t.browser.context_net(plain).unwrap().config().shared_asset_cache.is_some());
}

/// A linked sheet decodes as CSS Syntax says: the response's charset, else its `@charset`, else
/// the link's `charset` (Chromium reads the obsolete attribute), else the document's encoding
/// (WPT css/CSS2/syntax/at-charset-0xx, charset-attr-001). It used to decode as UTF-8. The
/// sheets are `data:` URLs, whose `text/css` has no charset.
#[test]
fn linked_sheets_decode_by_charset_rule_link_charset_and_document_encoding() {
    let (mut t, rec, page) = setup();
    t.enable_network(pyro_net::NetConfig::default()).unwrap();
    // `.tést` in ISO-8859-1 after its `@charset`, `.平和` in Shift_JIS, `.é` in windows-1252.
    // (`%25`: the document's own data: URL decodes one level of escapes.)
    let document = "data:text/html;charset=windows-1252,\
        <link rel=stylesheet href='data:text/css,@charset \"iso-8859-1\";.t%25E9st{}'>\
        <link rel=stylesheet href='data:text/css,.%2595%25BD%2598%2561{}' charset=shift_jis>\
        <link rel=stylesheet href='data:text/css,.%25E9{}'>\
        <link rel=stylesheet href='data:text/css;charset=utf-8,@charset \"shift_jis\";.%25C3%25A9{}'>";
    t.browser.navigate(page, NavigateOpts::address_bar(document.to_owned())).unwrap();
    until_network_idle(&mut t, &rec);
    eval(&t, page, "for (const s of document.styleSheets) console.log('rule:' + s.cssRules[0].selectorText)");
    let console = rec.console.borrow();
    let rules: Vec<&str> = console.iter().filter_map(|c| c.strip_prefix("log:rule:")).collect();
    // The last one: the response's charset over its `@charset`.
    assert_eq!(rules, [".tést", ".平和", ".é", ".é"], "{console:?}");
}

#[test]
fn static_files_load_on_the_browser_thread_with_network_events() {
    let dir = std::env::temp_dir().join(format!("pyro-browser-static-{}", std::process::id()));
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
    let config = pyro_net::NetConfig {
        static_files: pyro_net::StaticFiles::new(vec![(origin.into(), dir.clone())]).unwrap(),
        static_direct: true,
        ..pyro_net::NetConfig::default()
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
            pyro_net::NetEvent::RequestWillBeSent { request, .. } => format!("sent:{}", request.url.path()),
            pyro_net::NetEvent::ResponseReceived { head, .. } => format!("response:{}:{}", head.url.path(), head.mime),
            pyro_net::NetEvent::LoadingFinished { encoded_len, .. } => format!("finished:{encoded_len}"),
            pyro_net::NetEvent::DataReceived { .. } => "data".into(),
            pyro_net::NetEvent::LoadingFailed { error, .. } => format!("failed:{error}"),
            pyro_net::NetEvent::WebSocket { .. } => "websocket".into(),
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
    assert!(matches!(r, Err(pyro_js::JsError::Unsupported(_))));
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
        fn unhandled_rejection(&self, _realm: &Realm, _exception: &pyro_js::JsException) {
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
    t.enable_network(pyro_net::NetConfig::default()).unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar("about:blank")).unwrap();
    until_network_idle(&mut t, &rec);
    (t, rec, page)
}

fn until_console(t: &mut BrowserThread, rec: &Recorder, want: &str) {
    let want = format!("log:{want}");
    let r = t.run_until(Some(Instant::now() + Duration::from_secs(10)), |_| {
        rec.console.borrow().iter().any(|c| c.starts_with(&want))
    });
    assert_eq!(r, pyro_runtime::RunOutcome::Done, "{want}: {:?} {:?}", rec.console.borrow(), rec.errors.borrow());
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
    assert!(pyro_webapi::join_stopped(Duration::from_secs(5)), "worker threads end");
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
    assert!(pyro_webapi::join_stopped(Duration::from_secs(5)), "worker threads end");
}

#[test]
fn module_workers_import_statically_and_dynamically() {
    let (mut t, rec, page) = worker_setup();
    let dep = data_url("export const x = 40;");
    let lazy = data_url("export default 2;");
    let module = data_url(&format!(
        "import {{ x }} from {dep:?};
         const lazy = await import({lazy:?});
         let scripts; try {{ importScripts({dep:?}); }} catch (e) {{ scripts = e.name; }}
         postMessage(['module', x + lazy.default, import.meta.url.slice(0, 5), scripts, typeof this].join());"
    ));
    let classic = data_url(&format!("import({dep:?}).then((m) => postMessage('classic:' + m.x));"));
    let broken = data_url("import 'http://127.0.0.1:1/missing.js'; postMessage('ran');");
    eval(
        &t,
        page,
        &format!(
            r#"const m = new Worker({module:?}, {{type: 'module'}});
            m.onmessage = (e) => console.log(e.data);
            m.onerror = (e) => console.log('module-error:' + e.message);
            const c = new Worker({classic:?});
            c.onmessage = (e) => console.log(e.data);
            const b = new Worker({broken:?}, {{type: 'module'}});
            b.onmessage = (e) => console.log(e.data);
            b.onerror = () => console.log('broken-error');"#
        ),
    );
    until_console(&mut t, &rec, "module,42,data:,TypeError,undefined");
    until_console(&mut t, &rec, "classic:40");
    until_console(&mut t, &rec, "broken-error");
    assert!(!rec.console.borrow().iter().any(|c| c.contains("log:ran") || c.contains("module-error")));
    t.browser.close_all();
    assert!(pyro_webapi::join_stopped(Duration::from_secs(5)), "worker threads end");
}

#[test]
fn worker_report_error_reaches_its_error_event_and_the_page() {
    let (mut t, rec, page) = worker_setup();
    let script = data_url(
        "addEventListener('error', (e) => postMessage('self:' + e.message), { once: true });
         postMessage([typeof requestIdleCallback, crossOriginIsolated, isSecureContext].join());
         reportError(new RangeError('reported'));",
    );
    eval(
        &t,
        page,
        &format!(
            "const w = new Worker({script:?});
             w.onmessage = (e) => console.log(e.data);
             w.onerror = (e) => console.log('page:' + e.message);"
        ),
    );
    until_console(&mut t, &rec, "undefined,false,true");
    until_console(&mut t, &rec, "self:Uncaught RangeError: reported");
    until_console(&mut t, &rec, "page:Uncaught RangeError: reported");
    t.browser.close_all();
    assert!(pyro_webapi::join_stopped(Duration::from_secs(5)), "worker threads end");
}

#[test]
fn message_ports_cross_to_and_from_a_worker_and_stay_entangled() {
    let (mut t, rec, page) = worker_setup();
    // The worker echoes on every port it gets, keeps a port of its own for
    // the page, and answers through a port the page made after a round trip.
    let script = data_url(
        "const own = new MessageChannel();
         own.port1.onmessage = (e) => own.port1.postMessage('own:' + e.data);
         onmessage = (e) => {
           if (e.data === 'own') return postMessage({own: own.port2}, [own.port2]);
           const [port] = e.ports;
           const shape = [e.ports.length, port instanceof MessagePort, e.data.port === port, e.data.n].join();
           port.onmessage = (m) => {
             if (m.data === 'close') return port.close();
             if (m.data === 'back') {
               const reply = new MessageChannel();
               reply.port1.onmessage = (r) => port.postMessage('via-reply:' + r.data);
               return port.postMessage({reply: reply.port2}, [reply.port2]);
             }
             port.postMessage('echo:' + m.data + ':' + m.ports.length);
           };
           port.postMessage('shape:' + shape);
         };",
    );
    eval(
        &t,
        page,
        &format!(
            r#"const w = new Worker({script:?});
            globalThis.w = w;
            const ch = new MessageChannel();
            ch.port1.onmessage = (e) => {{
                if (e.data && e.data.reply) {{
                    e.data.reply.onmessage = (r) => console.log('reply-port:' + r.data);
                    e.data.reply.postMessage('ping');
                    return;
                }}
                console.log(e.data);
                if (e.data === 'echo:two:0') ch.port1.postMessage('back');
            }};
            w.postMessage({{port: ch.port2, n: 7}}, [ch.port2]);
            // Transferred: posting it again fails, its posts go nowhere.
            try {{ w.postMessage(null, [ch.port2]); }} catch (e) {{ console.log('again:' + e.name); }}
            ch.port1.postMessage('one');
            ch.port1.postMessage('two');
            w.onmessage = (e) => {{
                const own = e.data.own;
                own.onmessage = (m) => console.log(m.data);
                // A port that came from the worker moves on within the page.
                const moved = structuredClone(own, {{transfer: [own]}});
                moved.onmessage = (m) => console.log('moved:' + m.data);
                moved.postMessage('hi');
            }};
            w.postMessage('own');"#
        ),
    );
    until_console(&mut t, &rec, "shape:1,true,true,7");
    until_console(&mut t, &rec, "echo:one:0");
    until_console(&mut t, &rec, "echo:two:0");
    until_console(&mut t, &rec, "via-reply:ping");
    until_console(&mut t, &rec, "moved:own:hi");
    let console = rec.console.borrow().join("\n");
    assert!(console.contains("log:again:DataCloneError"), "{console}");
    // FIFO on the port: shape, one, two.
    let order: Vec<_> =
        rec.console.borrow().iter().filter(|c| c.contains("shape:") || c.contains("echo:")).cloned().collect();
    assert_eq!(order, ["log:shape:1,true,true,7", "log:echo:one:0", "log:echo:two:0"]);
    drop(console);
    assert!(rec.errors.borrow().is_empty(), "{:?}", rec.errors.borrow());
    t.browser.close_all();
    assert!(pyro_webapi::join_stopped(Duration::from_secs(5)), "worker threads end");
}

#[test]
fn a_port_from_a_worker_waits_for_start_and_close_disentangles() {
    let (mut t, rec, page) = worker_setup();
    let script = data_url(
        "const ch = new MessageChannel();
         ch.port1.postMessage('a'); ch.port1.postMessage('b');
         postMessage(ch.port2, [ch.port2]);
         ch.port1.postMessage('c');
         ch.port1.onmessage = (e) => {
           if (e.data === 'close-me') { ch.port1.close(); return postMessage('closed'); }
           postMessage('worker-got:' + e.data);
         };
         onmessage = () => { ch.port1.postMessage('after-close'); postMessage('posted-after-close'); };",
    );
    eval(
        &t,
        page,
        &format!(
            r#"const w = new Worker({script:?});
            globalThis.w = w;
            const got = [];
            w.onmessage = (e) => {{
                if (e.data instanceof MessagePort) {{
                    globalThis.port = e.data;
                    // A listener does not start the port: nothing arrives yet.
                    port.addEventListener('message', (m) => {{ got.push(m.data); if (got.length === 3) console.log('got:' + got.join()); }});
                    setTimeout(() => {{ console.log('before-start:' + got.length); port.start(); port.postMessage('x'); }}, 50);
                    return;
                }}
                console.log(e.data);
                if (e.data === 'worker-got:x') port.postMessage('close-me');
                if (e.data === 'closed') w.postMessage('again');
                if (e.data === 'posted-after-close') setTimeout(() => console.log('end:' + got.join()), 50);
            }};"#
        ),
    );
    until_console(&mut t, &rec, "before-start:0");
    until_console(&mut t, &rec, "got:a,b,c");
    until_console(&mut t, &rec, "worker-got:x");
    until_console(&mut t, &rec, "end:a,b,c");
    assert!(rec.errors.borrow().is_empty(), "{:?}", rec.errors.borrow());
    t.browser.close_all();
    assert!(pyro_webapi::join_stopped(Duration::from_secs(5)), "worker threads end");
}

#[test]
fn two_workers_talk_over_a_channel_the_page_made() {
    let (mut t, rec, page) = worker_setup();
    let ping = data_url(
        "onmessage = (e) => {
           const port = e.ports[0];
           let n = 0;
           port.onmessage = (m) => { if (++n === 3) postMessage('ping-done:' + m.data); else port.postMessage(n); };
           port.postMessage(0);
         };",
    );
    let pong = data_url(
        "onmessage = (e) => { const port = e.ports[0]; port.onmessage = (m) => port.postMessage('pong' + m.data); };",
    );
    eval(
        &t,
        page,
        &format!(
            r#"const a = new Worker({ping:?}), b = new Worker({pong:?});
            globalThis.workers = [a, b];
            const ch = new MessageChannel();
            a.onmessage = (e) => console.log(e.data);
            b.postMessage(null, [ch.port2]);
            a.postMessage(null, [ch.port1]);"#
        ),
    );
    until_console(&mut t, &rec, "ping-done:pong2");
    assert!(rec.errors.borrow().is_empty(), "{:?}", rec.errors.borrow());
    t.browser.close_all();
    assert!(pyro_webapi::join_stopped(Duration::from_secs(5)), "worker threads end");
}

#[test]
fn streams_transfer_to_and_from_a_worker() {
    let (mut t, rec, page) = worker_setup();
    // The worker reads the page's stream, writes into the page's sink, and
    // hands the page a TransformStream that runs in the worker.
    let script = data_url(
        "onmessage = async (e) => {
           const {readable, writable} = e.data;
           const parts = [];
           for await (const chunk of readable) parts.push(chunk);
           const writer = writable.getWriter();
           await writer.write('w:' + parts.join('+'));
           await writer.close();
           const upper = new TransformStream({transform(chunk, c) { c.enqueue(self.name + ':' + chunk.toUpperCase()); }});
           postMessage({upper, ports: e.ports.length}, [upper]);
         };",
    );
    eval(
        &t,
        page,
        &format!(
            r#"const w = new Worker({script:?}, {{name: 'in-worker'}});
            globalThis.w = w;
            const readable = new ReadableStream({{start(c) {{ c.enqueue('a'); c.enqueue('b'); c.close(); }}}});
            const writable = new WritableStream({{write(chunk) {{ console.log('sink:' + chunk); }}, close() {{ console.log('sink-closed'); }}}});
            w.onmessage = async (e) => {{
                const {{upper, ports}} = e.data;
                const writer = upper.writable.getWriter();
                writer.write('hey');
                writer.close();
                const reader = upper.readable.getReader();
                console.log('upper:' + (await reader.read()).value + ':' + (await reader.read()).done + ':' + ports + ':' + e.ports.length);
            }};
            w.postMessage({{readable, writable}}, [readable, writable]);
            console.log('locked:' + readable.locked + ':' + writable.locked);"#
        ),
    );
    until_console(&mut t, &rec, "locked:true:true");
    until_console(&mut t, &rec, "sink:w:a+b");
    until_console(&mut t, &rec, "sink-closed");
    until_console(&mut t, &rec, "upper:in-worker:HEY:true:0:0");
    assert!(rec.errors.borrow().is_empty(), "{:?}", rec.errors.borrow());
    t.browser.close_all();
    assert!(pyro_webapi::join_stopped(Duration::from_secs(5)), "worker threads end");
}

#[test]
fn crypto_keys_cross_to_a_worker_and_back() {
    let (mut t, rec, page) = worker_setup();
    let script = data_url(
        "onmessage = async (e) => {
           const {key, data} = e.data;
           const iv = new Uint8Array(12);
           const sealed = await crypto.subtle.encrypt({name: 'AES-GCM', iv}, key, data);
           postMessage({key, sealed, shape: [key instanceof CryptoKey, key.type, key.extractable, key.algorithm.name,
             key.algorithm.length, key.usages.join('+')].join()});
         };",
    );
    eval(
        &t,
        page,
        &format!(
            r#"const w = new Worker({script:?});
            globalThis.w = w;
            (async () => {{
                const key = await crypto.subtle.generateKey({{name: 'AES-GCM', length: 128}}, false, ['encrypt', 'decrypt']);
                w.onmessage = async (e) => {{
                    console.log('shape:' + e.data.shape);
                    const back = e.data.key;
                    const iv = new Uint8Array(12);
                    const open = await crypto.subtle.decrypt({{name: 'AES-GCM', iv}}, key, e.data.sealed);
                    const again = await crypto.subtle.encrypt({{name: 'AES-GCM', iv}}, back, new TextEncoder().encode('secret'));
                    console.log('round:' + new TextDecoder().decode(open) + ':' + (back !== key));
                    console.log('same:' + (new Uint8Array(again).join() === new Uint8Array(e.data.sealed).join()));
                }};
                w.postMessage({{key, data: new TextEncoder().encode('secret')}});
            }})();"#
        ),
    );
    until_console(&mut t, &rec, "shape:");
    until_console(&mut t, &rec, "same:");
    let console = rec.console.borrow().join("\n");
    assert!(console.contains("log:shape:true,secret,false,AES-GCM,128,encrypt+decrypt"), "{console}");
    assert!(console.contains("log:round:secret:true"), "{console}");
    assert!(console.contains("log:same:true"), "{console}");
    drop(console);
    assert!(rec.errors.borrow().is_empty(), "{:?}", rec.errors.borrow());
    t.browser.close_all();
    assert!(pyro_webapi::join_stopped(Duration::from_secs(5)), "worker threads end");
}

#[test]
fn image_bitmaps_and_offscreen_canvases_cross_to_a_worker_and_back() {
    let (mut t, rec, page) = worker_setup();
    let script = data_url(
        "onmessage = (e) => {
           const d = e.data;
           if (d.offscreen) {
             const o = d.offscreen;
             const x = o.getContext('2d');
             x.fillStyle = 'blue'; x.fillRect(0, 0, 2, 2);
             postMessage('drawn:' + [o instanceof OffscreenCanvas, o.width, o.height].join());
             return;
           }
           const {bm, copy} = d;
           const o = new OffscreenCanvas(4, 4);
           const x = o.getContext('2d');
           x.drawImage(bm, 0, 0);
           const seen = [bm instanceof ImageBitmap, bm.width, copy.width, Array.from(x.getImageData(1, 1, 1, 1).data).join('-')].join();
           x.fillStyle = 'red'; x.fillRect(0, 0, 4, 4);
           const back = o.transferToImageBitmap();
           postMessage({seen, back}, [back]);
           postMessage('after:' + back.width);
         };",
    );
    eval(
        &t,
        page,
        &format!(
            r#"const w = new Worker({script:?});
            globalThis.w = w;
            (async () => {{
                const src = new OffscreenCanvas(4, 4);
                const sx = src.getContext('2d');
                sx.fillStyle = 'lime'; sx.fillRect(0, 0, 4, 4);
                const bm = await createImageBitmap(src);
                const copy = await createImageBitmap(src);
                const c = document.createElement('canvas');
                c.width = 4; c.height = 4;
                document.body.append(c);
                const x = c.getContext('2d');
                w.onmessage = (e) => {{
                    if (typeof e.data === 'string') return console.log(e.data);
                    x.drawImage(e.data.back, 0, 0);
                    console.log('seen:' + e.data.seen);
                    console.log('back:' + [e.data.back.width, Array.from(x.getImageData(1, 1, 1, 1).data).join('-')].join());
                }};
                w.postMessage({{bm, copy}}, [bm]);
                console.log('sent:' + [bm.width, copy.width].join());
                const placeholder = document.createElement('canvas');
                placeholder.id = 'p';
                placeholder.width = 6; placeholder.height = 3;
                document.body.append(placeholder);
                const offscreen = placeholder.transferControlToOffscreen();
                w.postMessage({{offscreen}}, [offscreen]);
                console.log('moved:' + [offscreen.width, offscreen.height].join());
            }})();"#
        ),
    );
    until_console(&mut t, &rec, "back:");
    until_console(&mut t, &rec, "drawn:");
    let console = rec.console.borrow().join("\n");
    assert!(console.contains("log:sent:0,4"), "{console}");
    assert!(console.contains("log:seen:true,4,4,0-255-0-255"), "{console}");
    assert!(console.contains("log:back:4,255-0-0-255"), "{console}");
    assert!(console.contains("log:after:0"), "{console}");
    assert!(console.contains("log:moved:0,0"), "{console}");
    assert!(console.contains("log:drawn:true,6,3"), "{console}");
    drop(console);
    // The worker committed its frame after the task that drew it: the
    // placeholder shows it, in screenshots too.
    let dom = t.browser.page(page).unwrap().main_frame.page_dom.clone().unwrap();
    let r = t.run_until(Some(Instant::now() + Duration::from_secs(5)), |_| dom.canvas_bitmaps().len() == 2);
    assert_eq!(r, pyro_runtime::RunOutcome::Done);
    let snapshot = dom.render_snapshot("about:blank", None);
    let (_, w, h, rgba) = snapshot.canvases.iter().find(|(_, w, _, _)| *w == 6).expect("placeholder frame");
    assert_eq!((*w, *h), (6, 3));
    assert_eq!(&rgba[..4], &[0, 0, 255, 255]);
    assert_eq!(&rgba[2 * 4..3 * 4], &[0, 0, 0, 0]);
    assert!(rec.errors.borrow().is_empty(), "{:?}", rec.errors.borrow());
    t.browser.close_all();
    assert!(pyro_webapi::join_stopped(Duration::from_secs(5)), "worker threads end");
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
    assert!(pyro_webapi::join_stopped(Duration::from_secs(5)), "worker threads end");
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

fn eval_string(t: &BrowserThread, page: PageId, src: &str) -> String {
    let realm = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
    let s = realm.scope().unwrap();
    let value = s.eval(src, Some("test.js"), 1).unwrap_or_else(|e| panic!("{src}: {}", e.details(&s).text()));
    value.to_string().unwrap()
}

/// Serves `http://next.test/` (a page's next document) and child frame documents.
fn serve_next_documents(t: &mut BrowserThread) {
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://next.test/", "text/html", "<p id=next>next</p>", Duration::ZERO);
    loader.insert("http://next.test/child", "text/html", "<p>child</p>", Duration::ZERO);
    loader.insert("http://next.test/written", "text/html", "<p>next</p>", Duration::ZERO);
    t.browser.set_loader(Some(loader));
}

/// Navigates the page to another document and runs the loop until it committed.
fn goto_next_document(t: &mut BrowserThread, page: PageId) {
    goto(t, page, "http://next.test/");
    assert_eq!(eval_string(t, page, "document.querySelector('#next').textContent"), "next");
}

/// Navigates the page (as `Page.navigate` does) to a URL it is not at, and
/// runs the loop until that document loaded.
fn goto(t: &mut BrowserThread, page: PageId, url: &str) {
    t.browser.navigate(page, NavigateOpts::address_bar(url)).unwrap();
    wait_for_url(t, page, url);
}

fn wait_for_url(t: &mut BrowserThread, page: PageId, url: &str) {
    let committed = t.run_until(Some(Instant::now() + Duration::from_secs(5)), |b| {
        b.page(page).is_some_and(|p| p.main_frame.url == url && p.main_frame.loaded)
    });
    assert_eq!(committed, RunOutcome::Done, "the navigation to {url} did not commit");
}

/// Sets the release gate due at every reading, or never: the footprint it
/// reads is the test process's, which the tests running alongside move.
fn release_gate_due(t: &BrowserThread, due: bool) {
    let (min, percent) = if due { (0, 0) } else { (u64::MAX, 0) };
    t.browser.engine.set_release_growth(min, percent);
}

#[test]
fn a_close_leaves_the_collection_and_the_code_drop_to_idle_moments() {
    // A close past the gate (but under its ceiling) collects nothing on the
    // closing command's stack: it leaves the collection pending for the idle
    // loop (`settle` after a quiet second), and the code drop pending for a
    // longer quiet moment (`settle_idle`), which `settle` alone keeps.
    let (mut t, _rec, page) = setup();
    release_gate_due(&t, true);
    t.browser.engine.set_release_ceiling(u64::MAX, 0);
    eval(&t, page, "for (let i = 0; i < 200; i++) document.body.appendChild(document.createElement('div'))");
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(2)));
    *t.browser.released_payloads.borrow_mut() = 0;
    assert!(t.browser.close_page(page));
    assert_eq!(*t.browser.released_payloads.borrow(), 0, "the close itself collected");
    assert!(t.browser.engine.release_pending());
    assert!(t.browser.engine.code_release_pending());
    t.browser.engine.settle();
    assert!(!t.browser.engine.release_pending());
    assert!(t.browser.engine.code_release_pending(), "a short idle moment keeps the compiled code");
    t.browser.engine.settle_idle();
    assert!(!t.browser.engine.code_release_pending());
    // Past the ceiling, the close collects at once (as every close did).
    let page = t.browser.create_page().unwrap();
    t.browser.engine.set_release_ceiling(0, 0);
    eval(&t, page, "for (let i = 0; i < 200; i++) document.body.appendChild(document.createElement('div'))");
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(2)));
    *t.browser.released_payloads.borrow_mut() = 0;
    assert!(t.browser.close_page(page));
    t.browser.checkpoint();
    assert!(t.browser.engine.release_pending(), "a second collection from the idle loop is still due");
    t.browser.engine.settle();
}

#[test]
fn a_close_after_one_left_waiting_collects_when_no_idle_moment_came() {
    // A process that never idles (back-to-back pages): the first close past
    // the gate leaves its collection to an idle moment, the next close
    // collects, so the process holds one closed page's garbage, not up to
    // the ceiling. An idle collection in between starts the count again.
    let (mut t, _rec, page) = setup();
    release_gate_due(&t, true);
    t.browser.engine.set_release_ceiling(u64::MAX, 0);
    let close = |t: &mut BrowserThread, page: PageId| {
        eval(t, page, "for (let i = 0; i < 200; i++) document.body.appendChild(document.createElement('div'))");
        t.run_until_idle(Some(Instant::now() + Duration::from_secs(2)));
        assert!(t.browser.close_page(page));
        assert!(t.browser.engine.release_pending(), "a collection from the idle loop is due after every close");
        t.browser.engine.release_put_off()
    };
    assert_eq!(close(&mut t, page), 1, "the first close waits");
    let page = t.browser.create_page().unwrap();
    assert_eq!(close(&mut t, page), 0, "the second close collected");
    let page = t.browser.create_page().unwrap();
    assert_eq!(close(&mut t, page), 1, "and the next one waits again");
    t.browser.engine.settle();
    assert_eq!(t.browser.engine.release_put_off(), 0, "an idle collection starts the count again");
    let page = t.browser.create_page().unwrap();
    assert_eq!(close(&mut t, page), 1);
    t.browser.engine.settle();
}

#[test]
fn replacing_the_untouched_initial_document_leaves_nothing_to_release() {
    // A page's first navigation (a test's first `goto`): its blank document
    // and two realms are not worth a full collection, and the initial
    // about:blank entry is replaced. Not even past the gate.
    let (mut t, _rec, page) = setup();
    serve_next_documents(&mut t);
    release_gate_due(&t, true);
    eval(&t, page, "globalThis.kept = [1, 2, 3]");
    goto_next_document(&mut t, page);
    assert!(!t.browser.engine.release_pending(), "an untouched initial document was released");
    assert_eq!(eval_string(&t, page, "history.length"), "1");
}

#[test]
fn blank_document_content_counts_the_nodes_about_blank_is_made_with() {
    let (t, _rec, page) = setup();
    let blank = t.browser.page(page).unwrap().main_frame.page_dom.clone().unwrap();
    assert!(!bindings::blank_document_has_content(&blank));
    eval(&t, page, "globalThis.meta = document.head.appendChild(document.createElement('meta'))");
    assert!(bindings::blank_document_has_content(&blank), "one more node is content");
    eval(&t, page, "meta.remove(); document.body.setAttribute('class', 'x')");
    assert!(!bindings::blank_document_has_content(&blank), "an attribute is not");
    // Not among the document's descendants.
    eval(&t, page, "document.body.attachShadow({ mode: 'closed' })");
    assert!(bindings::blank_document_has_content(&blank), "a shadow tree is");
}

#[test]
fn an_initial_document_given_content_is_released_once_the_gate_is_due() {
    // Each way the initial about:blank document gains content before the
    // first navigation: the commit that replaces it leaves a collection
    // pending (`Engine::defer_release_if_due`) when the process has grown
    // past the gate, and only then. A pending collection is what the idle
    // loop runs after a quiet second, dropping the live page's compiled code
    // with it: an init script's `<style>` or a small `setContent` must not.
    let (mut t, _rec, _page) = setup();
    serve_next_documents(&mut t);
    let written =
        "'<div>' + '<p>row</p>'.repeat(200) + '</div><script>globalThis.big = new Array(1e5).fill(7)</script>'";
    let cases = [
        // page.setContent / Page.setDocumentContent.
        ("document.open/write", format!("document.open(); document.write({written}); document.close();")),
        // An implicit open: write() on a document with no parser.
        ("document.write", format!("document.write({written}); document.close();")),
        // Runtime.evaluate or a script filling the document through DOM APIs.
        ("innerHTML", format!("document.body.innerHTML = {written};")),
        ("appendChild", "document.body.appendChild(document.createElement('p')).textContent = 'x';".into()),
        ("shadow root", format!("document.body.attachShadow({{ mode: 'open' }}).innerHTML = {written};")),
        // context.addInitScript, which runs on the initial document too.
        ("style", "document.head.appendChild(document.createElement('style')).textContent = 'p {}';".into()),
        // A child frame (and its own documents) is the old document's too.
        ("iframe", "document.body.appendChild(document.createElement('iframe'));".into()),
    ];
    for due in [false, true] {
        release_gate_due(&t, due);
        for (what, script) in &cases {
            let page = t.browser.create_page().unwrap();
            eval(&t, page, script);
            t.run_until_idle(Some(Instant::now() + Duration::from_secs(2)));
            assert!(!t.browser.engine.release_pending(), "{what}: pending before the navigation");
            goto_next_document(&mut t, page);
            assert_eq!(t.browser.engine.release_pending(), due, "{what}: gate due {due}");
            t.browser.engine.settle();
            // A loaded document is released whatever the gate.
            goto(&mut t, page, "http://next.test/child");
            assert!(t.browser.engine.release_pending(), "{what}: the loaded document was not released");
            t.browser.engine.settle();
            assert!(t.browser.close_page(page));
            t.browser.engine.settle();
        }
    }
}

#[test]
fn document_open_makes_the_initial_document_an_ordinary_history_entry() {
    // HTML "document open steps": the document is no longer the initial
    // about:blank one, so the next navigation pushes an entry instead of
    // replacing it (Chrome: history.length 2 after setContent + goto).
    let (mut t, _rec, page) = setup();
    serve_next_documents(&mut t);
    eval(&t, page, "document.open(); document.write('<p>written</p>'); document.close();");
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(2)));
    goto_next_document(&mut t, page);
    assert_eq!(eval_string(&t, page, "history.length"), "2");
    // A document made with DOM APIs is still the initial one for history.
    let page = t.browser.create_page().unwrap();
    eval(&t, page, "document.body.innerHTML = '<p>inner</p>'");
    goto_next_document(&mut t, page);
    assert_eq!(eval_string(&t, page, "history.length"), "1");
    // open() on a document of no frame leaves the frame's document alone.
    t.browser.engine.settle();
    let page = t.browser.create_page().unwrap();
    eval(
        &t,
        page,
        "const d = document.implementation.createHTMLDocument(''); d.open(); d.write('<p>x</p>'); d.close();",
    );
    goto_next_document(&mut t, page);
    assert_eq!(eval_string(&t, page, "history.length"), "1");
    assert!(!t.browser.engine.release_pending(), "a document of no frame made the frame's document released");
}

#[test]
fn open_and_close_of_a_document_of_no_frame_leave_the_frames_lifecycle_alone() {
    let (mut t, rec, page) = setup();
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(2)));
    eval(
        &t,
        page,
        "document.addEventListener('DOMContentLoaded', () => console.log('frame dcl'));
         addEventListener('load', () => console.log('frame load'));
         const d = document.implementation.createHTMLDocument('');
         d.addEventListener('DOMContentLoaded', () => console.log('own dcl'));
         d.open(); d.write('<p>x</p>'); d.close();
         console.log('written ' + d.body.innerHTML);",
    );
    rec.events.borrow_mut().clear();
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(2)));
    assert_eq!(*rec.console.borrow(), ["log:written <p>x</p>"]);
    assert!(!rec.events.borrow().iter().any(|e| e == "dcl" || e == "load"), "{:?}", rec.events.borrow());
}

#[test]
fn a_child_frames_written_initial_document_is_released_when_replaced() {
    let (mut t, _rec, page) = setup();
    serve_next_documents(&mut t);
    release_gate_due(&t, true);
    // The usual iframe: appended, then given a URL. Its initial document is
    // replaced untouched.
    eval(&t, page, "globalThis.f = document.body.appendChild(document.createElement('iframe'));");
    eval(&t, page, "f.src = 'http://next.test/child'");
    let loaded = t.run_until(Some(Instant::now() + Duration::from_secs(5)), |b| {
        b.frames(page).iter().any(|f| f.url == "http://next.test/child")
    });
    assert_eq!(loaded, RunOutcome::Done);
    assert!(!t.browser.engine.release_pending(), "an untouched child initial document was released");
    // One written to (an editor's or an ad's iframe), then navigated.
    eval(&t, page, "globalThis.g = document.body.appendChild(document.createElement('iframe'));");
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(2)));
    let written = eval_string(
        &t,
        page,
        "g.contentDocument.open(); g.contentDocument.write('<p>written</p>'); g.contentDocument.close();
         g.contentDocument.body.innerHTML",
    );
    assert_eq!(written, "<p>written</p>");
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(2)));
    assert!(!t.browser.engine.release_pending());
    eval(&t, page, "g.src = 'http://next.test/written'");
    let loaded = t.run_until(Some(Instant::now() + Duration::from_secs(5)), |b| {
        b.frames(page).iter().any(|f| f.url == "http://next.test/written")
    });
    assert_eq!(loaded, RunOutcome::Done);
    assert!(t.browser.engine.release_pending(), "the child's written initial document was not released");
}

/// Serves `http://next.test/` at once and `http://next.test/slow` after
/// 300 ms, so a navigation to it is still fetching for a while.
fn serve_a_slow_document(t: &mut BrowserThread) {
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://next.test/", "text/html", "<p id=next>next</p>", Duration::ZERO);
    loader.insert("http://next.test/slow", "text/html", "<p>slow</p>", Duration::from_millis(300));
    t.browser.set_loader(Some(loader));
}

const WRITE: &str = "document.open(); document.write('<p>w</p>'); document.close();";

fn history_url_body(t: &BrowserThread, page: PageId) -> String {
    eval_string(t, page, "[history.length, location.href, document.body.innerHTML].join(' ')")
}

/// Runs the loop past the slow document's response.
fn run_past_the_slow_response(t: &mut BrowserThread) {
    t.run_until(Some(Instant::now() + Duration::from_millis(500)), |_| false);
}

#[test]
fn document_open_stops_a_navigation_its_scripts_started() {
    // HTML document open steps, "stop loading", as Chrome does: a script's
    // navigation that has not committed is dropped, queued or fetching, and
    // the written document stays (Chrome: history.length 1, about:blank,
    // the written markup, for a queued one; the same 150 ms into a fetch).
    let (mut t, rec, _page) = setup();
    serve_a_slow_document(&mut t);
    // Queued by the same script.
    let page = t.browser.create_page().unwrap();
    eval(&t, page, &format!("location.href = 'http://next.test/'; {WRITE}"));
    run_past_the_slow_response(&mut t);
    assert_eq!(history_url_body(&t, page), "1 about:blank <p>w</p>");
    // Fetching.
    let page = t.browser.create_page().unwrap();
    eval(&t, page, "location.href = 'http://next.test/slow'");
    let fetching = t.run_until(Some(Instant::now() + Duration::from_secs(2)), |b| {
        b.page(page).is_some_and(|p| p.pending_loader().is_some())
    });
    assert_eq!(fetching, RunOutcome::Done);
    rec.events.borrow_mut().clear();
    eval(&t, page, WRITE);
    run_past_the_slow_response(&mut t);
    assert_eq!(history_url_body(&t, page), "1 about:blank <p>w</p>");
    assert!(rec.events.borrow().iter().any(|e| e == "failed:net::ERR_ABORTED"), "{:?}", rec.events.borrow());
    assert!(!rec.events.borrow().iter().any(|e| e == "navigated"), "{:?}", rec.events.borrow());
    // A loaded document's, a link's.
    let page = t.browser.create_page().unwrap();
    goto_next_document(&mut t, page);
    eval(
        &t,
        page,
        &format!(
            "const a = document.body.appendChild(document.createElement('a'));
             a.href = 'http://next.test/slow'; a.click(); {WRITE}"
        ),
    );
    run_past_the_slow_response(&mut t);
    assert_eq!(history_url_body(&t, page), "1 http://next.test/ <p>w</p>");
    // A child frame's, still fetching when its document is opened.
    eval(&t, page, "globalThis.g = document.body.appendChild(document.createElement('iframe'));");
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(2)));
    eval(&t, page, "g.src = 'http://next.test/slow'");
    t.run_until(Some(Instant::now() + Duration::from_millis(50)), |_| false);
    eval(&t, page, &WRITE.replace("document.", "g.contentDocument."));
    run_past_the_slow_response(&mut t);
    assert_eq!(eval_string(&t, page, "g.contentDocument.body.innerHTML"), "<p>w</p>");
    assert!(!t.browser.frames(page).iter().any(|f| f.url == "http://next.test/slow"));
}

#[test]
fn document_open_leaves_a_navigation_started_after_it_or_by_the_client() {
    let (mut t, _rec, page) = setup();
    serve_a_slow_document(&mut t);
    // Opened, then navigated by the same script: an ordinary entry is left.
    eval(&t, page, &format!("{WRITE} location.href = 'http://next.test/';"));
    wait_for_url(&mut t, page, "http://next.test/");
    assert_eq!(history_url_body(&t, page), "2 http://next.test/ <p id=\"next\">next</p>");
    // Page.navigate goes on (Chrome too), and it replaces the initial
    // entry: that was decided when it started.
    let page = t.browser.create_page().unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar("http://next.test/slow")).unwrap();
    t.run_until(Some(Instant::now() + Duration::from_millis(50)), |_| false);
    eval(&t, page, WRITE);
    wait_for_url(&mut t, page, "http://next.test/slow");
    assert_eq!(history_url_body(&t, page), "1 http://next.test/slow <p>slow</p>");
}

#[test]
fn nested_timers_are_clamped_to_4ms_past_level_5() {
    let (mut t, rec, page) = setup();
    // From a task at nesting level n, `setTimeout(late, 3)` then
    // `setTimeout(soon, 0)`: unclamped, `soon` runs first; clamped (both
    // 4 ms), `late` does. A microtask after a deep callback is no timer
    // task: its timer is not clamped and runs before the clamped one.
    eval(
        &t,
        page,
        "const order = [];
         function pair(n) {
             setTimeout(() => order.push(n + ':late'), 3);
             setTimeout(() => order.push(n + ':soon'), 0);
         }
         function step(n) {
             if (n === 1 || n === 5 || n === 6 || n === 8) pair(n);
             if (n === 8) {
                 queueMicrotask(() => setTimeout(() => order.push('micro'), 0));
                 setTimeout(() => order.push('nested'), 0);
             }
             if (n < 8) setTimeout(() => step(n + 1), 0);
             else setTimeout(() => console.log(order.join()), 50);
         }
         setTimeout(() => step(1), 0);",
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(5)));
    let log = rec.console.borrow().clone();
    let order = log.last().expect("logged").strip_prefix("log:").unwrap().to_owned();
    let pos = |s: &str| order.split(',').position(|e| e == s).unwrap_or_else(|| panic!("{s} in {order}"));
    assert!(pos("1:soon") < pos("1:late"), "{order}");
    assert!(pos("5:soon") < pos("5:late"), "{order}");
    assert!(pos("6:late") < pos("6:soon"), "{order}");
    assert!(pos("8:late") < pos("8:soon"), "{order}");
    assert!(pos("micro") < pos("nested"), "{order}");
}

#[test]
fn zero_ms_intervals_slow_to_4ms() {
    let (mut t, rec, page) = setup();
    eval(
        &t,
        page,
        "let ticks = 0; const start = performance.now();
         const id = setInterval(() => {
             ticks++;
             if (performance.now() - start >= 80) { clearInterval(id); console.log('ticks', ticks) }
         }, 0);",
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(5)));
    let log = rec.console.borrow().clone();
    let ticks: u32 = log[0].strip_prefix("log:ticks ").unwrap().parse().unwrap();
    // About 5 + 75 / 4 in Chrome; 80 at the loop's 1 ms floor.
    assert!(ticks <= 40, "{ticks}");
}

#[test]
fn idle_callbacks_run_once_idle_or_on_timeout() {
    let (mut t, rec, page) = setup();
    eval(
        &t,
        page,
        "const log = [];
         const id = requestIdleCallback(d => {
             const r = d.timeRemaining();
             log.push(`idle ${d.didTimeout} ${r > 0 && r <= 50} ${d instanceof IdleDeadline}`);
             console.log(log.join());
         });
         setTimeout(() => log.push('timer'), 0);
         const cancelled = requestIdleCallback(() => log.push('cancelled'));
         cancelIdleCallback(cancelled);
         // Overdue before the loop runs: due timers come first (by deadline), so it times out.
         requestIdleCallback(d => log.push(`timeout ${d.didTimeout} ${d.timeRemaining()}`), { timeout: 1 });
         const until = performance.now() + 5; while (performance.now() < until) {}
         let ctor; try { new IdleDeadline() } catch (e) { ctor = e.constructor.name }
         let bad; try { requestIdleCallback(1) } catch (e) { bad = e.constructor.name }
         console.log(typeof id, id > 0, cancelled !== id, ctor, bad, typeof IdleDeadline,
             Object.getOwnPropertyDescriptor(window, 'IdleDeadline').enumerable,
             Object.keys(IdleDeadline.prototype).join('/'));",
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(5)));
    let log = rec.console.borrow().clone();
    assert_eq!(log[0], "log:number true true TypeError TypeError function false timeRemaining/didTimeout");
    assert_eq!(log[1], "log:timer,timeout true 0,idle false true true", "{log:?} {:?}", rec.errors.borrow());
}

#[test]
fn report_error_and_cross_origin_isolated() {
    let (t, rec, page) = setup();
    eval(
        &t,
        page,
        "const seen = [];
         addEventListener('error', e => seen.push(`${e.message}|${e.error === err}|${e.lineno > 0}`), { once: true });
         const err = new TypeError('x');
         reportError(err);
         let thrown; try { reportError() } catch (e) { thrown = e.constructor.name }
         console.log(seen.join(), thrown, crossOriginIsolated, isSecureContext);",
    );
    assert_eq!(*rec.console.borrow(), ["log:Uncaught TypeError: x|true|true TypeError false true"]);
    // Not canceled: reported like an uncaught exception (CDP pageerror).
    assert_eq!(*rec.errors.borrow(), ["TypeError: x"]);
}

#[test]
fn secure_context_follows_potentially_trustworthy_urls_of_the_frame_chain() {
    let (mut t, rec, page) = setup();
    let loader = Rc::new(StaticLoader::default());
    for url in ["http://app.localhost/", "http://[::1]/", "http://localhost.evil.test/"] {
        loader.insert(url, "text/html", "<p>x</p>", Duration::ZERO);
    }
    // Its [SecureContext] members follow isSecureContext: absent in an https frame of an http page.
    let child = "<script>console.log('child', location.protocol, isSecureContext, 'locks' in navigator, \
                 'mediaDevices' in navigator, 'paintWorklet' in CSS, 'LockManager' in window)</script>";
    loader.insert("https://secure.test/", "text/html", child, Duration::ZERO);
    loader.insert("http://insecure.test/", "text/html", "<iframe src='https://secure.test/'></iframe>", Duration::ZERO);
    t.browser.set_loader(Some(loader));
    for (url, want) in
        [("http://app.localhost/", "true"), ("http://[::1]/", "true"), ("http://localhost.evil.test/", "false")]
    {
        goto(&mut t, page, url);
        assert_eq!(eval_string(&t, page, "String(isSecureContext)"), want, "{url}");
    }
    goto(&mut t, page, "http://insecure.test/");
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(2)));
    assert_eq!(eval_string(&t, page, "String(isSecureContext)"), "false");
    // An https frame of an http page is not secure (Chrome).
    assert!(
        rec.console.borrow().contains(&"log:child https: false false false false false".to_owned()),
        "{:?}",
        rec.console.borrow()
    );
}

#[test]
fn idle_callbacks_requested_in_an_idle_period_wait_for_the_next() {
    let (mut t, rec, page) = setup();
    // A self-rescheduling idle loop (idle work queues) must not spin: a
    // callback requested during an idle period runs in the next one, which
    // starts once this one's deadline (at most 50 ms) passed, as in Chrome.
    eval(
        &t,
        page,
        "let n = 0; const start = performance.now();
         function work(d) {
             n++;
             if (performance.now() - start < 200) requestIdleCallback(work);
             else console.log('runs', n);
         }
         requestIdleCallback(work);",
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(5)));
    let log = rec.console.borrow().clone();
    let runs: u32 = log[0].strip_prefix("log:runs ").unwrap().parse().unwrap();
    assert!((2..=8).contains(&runs), "{runs}");
}
