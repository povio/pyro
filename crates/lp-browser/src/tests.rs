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
