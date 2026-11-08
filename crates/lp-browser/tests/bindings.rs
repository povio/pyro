//! Browser-owned installation/lifecycle against real JSC and the document loader.
use lp_browser::{BrowserThread, InitScript, NavigateOpts, PageEvent, PageId, PageObserver, StaticLoader};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

fn eval(thread: &BrowserThread, page: PageId, script: &str) -> String {
    thread
        .browser
        .page(page)
        .unwrap()
        .main_frame
        .main_realm()
        .unwrap()
        .scope()
        .unwrap()
        .eval(script, None, 1)
        .unwrap()
        .to_string()
        .unwrap()
}

fn settle(thread: &mut BrowserThread) {
    thread.run_until_idle(Some(Instant::now() + Duration::from_secs(3)));
}

#[test]
fn isolated_world_event_reports_the_document_origin() {
    struct Origins(RefCell<Vec<String>>);
    impl PageObserver for Origins {
        fn on_page_event(&self, _page: PageId, event: &PageEvent) {
            if let PageEvent::RealmCreated { world: Some(_), origin, .. } = event {
                self.0.borrow_mut().push(origin.clone());
            }
        }
    }
    let mut thread = crate::browser_thread().unwrap();
    let page = thread.browser.create_page().unwrap();
    let origins = Rc::new(Origins(RefCell::new(Vec::new())));
    thread.browser.set_observer(Some(origins.clone()));
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/path", "text/html", "<p>world</p>", Duration::ZERO);
    thread.browser.set_loader(Some(loader));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/path")).unwrap();
    settle(&mut thread);
    thread.browser.isolated_world(page, "automation").unwrap();
    assert_eq!(&*origins.0.borrow(), &["http://site.test"]);
}

#[test]
fn an_isolated_world_nothing_fills_is_made_when_first_entered() {
    // Playwright's utility world: an init script with no source for every
    // document, then `Page.createIsolatedWorld`. The realm exists (id,
    // origin, event) from the document's commit; its context is made by the
    // first evaluation in it. A world with a script to run, or a binding,
    // is made with the document, as before.
    struct Worlds(RefCell<Vec<(Option<String>, String)>>);
    impl PageObserver for Worlds {
        fn on_page_event(&self, _page: PageId, event: &PageEvent) {
            if let PageEvent::RealmCreated { world, origin, .. } = event {
                self.0.borrow_mut().push((world.clone(), origin.clone()));
            }
        }
    }
    let mut thread = crate::browser_thread().unwrap();
    let page = thread.browser.create_page().unwrap();
    let worlds = Rc::new(Worlds(RefCell::new(Vec::new())));
    thread.browser.set_observer(Some(worlds.clone()));
    let script = |id: &str, world: &str, source: &str| InitScript {
        id: id.into(),
        world: Some(world.into()),
        source: source.into(),
    };
    thread.browser.add_init_script(page, script("u", "utility", ""));
    thread.browser.add_init_script(page, script("f", "filled", "globalThis.filled = 1"));
    thread.browser.add_binding(page, lp_browser::Binding { name: "bound".into(), world: Some("bound".into()) });
    thread.browser.add_init_script(page, script("b", "bound", ""));
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/path", "text/html", "<p>world</p><iframe></iframe>", Duration::ZERO);
    thread.browser.set_loader(Some(loader));
    worlds.0.borrow_mut().clear();
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/path")).unwrap();
    settle(&mut thread);
    let announced: Vec<(Option<String>, String)> =
        worlds.0.borrow().iter().filter(|(_, origin)| origin == "http://site.test").cloned().collect();
    let named = |w: &str| (Some(w.to_owned()), "http://site.test".to_owned());
    assert!(announced.contains(&(None, "http://site.test".to_owned())), "{announced:?}");
    for world in ["utility", "filled", "bound"] {
        assert!(announced.contains(&named(world)), "{world} announced at commit: {announced:?}");
    }
    let frame = thread.browser.page(page).unwrap().main_frame.clone();
    let utility = frame.world("utility").unwrap().clone();
    assert!(utility.is_deferred(), "nothing fills the utility world: deferred");
    assert!(!utility.is_disposed());
    assert!(!frame.world("filled").unwrap().is_deferred(), "a world with a script to run is made");
    assert!(!frame.world("bound").unwrap().is_deferred(), "a world with a binding is made");
    assert!(!frame.main_realm().unwrap().is_deferred());
    // `Page.createIsolatedWorld` finds the deferred realm; a world nothing
    // named is created on demand, deferred too, and announced.
    let same = thread.browser.isolated_world(page, "utility").unwrap();
    assert_eq!(same.id(), utility.id());
    assert!(same.is_deferred());
    let fresh = thread.browser.isolated_world(page, "fresh").unwrap();
    assert!(fresh.is_deferred());
    assert!(worlds.0.borrow().contains(&named("fresh")));
    // Child frames come and go without making it.
    eval(&thread, page, "document.body.appendChild(document.createElement('iframe')); 1");
    settle(&mut thread);
    assert!(utility.is_deferred());
    // The first entry makes it: the Window, its document, its frames.
    let read = utility
        .scope()
        .unwrap()
        .eval(
            "[document.querySelector('p').textContent, typeof filled, self === window, location.href, window.length, typeof window[1], typeof structuredClone].join()",
            None,
            1,
        )
        .unwrap()
        .to_string()
        .unwrap();
    assert_eq!(read, "world,undefined,true,http://site.test/path,2,object,function");
    assert!(!utility.is_deferred());
    assert_eq!(thread.browser.page(page).unwrap().main_frame.world("utility").unwrap().id(), utility.id());
    // Navigating away disposes the made one and the never-made one alike.
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/next", "text/html", "<p>next</p>", Duration::ZERO);
    thread.browser.set_loader(Some(loader));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/next")).unwrap();
    settle(&mut thread);
    assert!(utility.is_disposed() && fresh.is_disposed());
    let next = thread.browser.page(page).unwrap().main_frame.world("utility").unwrap().clone();
    assert!(next.is_deferred() && next.id() != utility.id());
    assert_eq!(eval(&thread, page, "document.querySelector('p').textContent"), "next");
}

#[test]
fn bindings_exist_before_init_scripts_and_follow_document_lifecycle() {
    let mut thread = crate::browser_thread().unwrap();
    let page = thread.browser.create_page().unwrap();
    assert_eq!(eval(&thread, page, "document.body.tagName + ':' + document.readyState"), "BODY:complete");
    thread.browser.add_init_script(page, InitScript {
        id: "init".into(), world: None,
        source: "import('/init.js'); globalThis.seen = ['init:' + document.readyState]; document.addEventListener('DOMContentLoaded', () => seen.push('dcl:' + document.readyState)); addEventListener('load', () => seen.push('load:' + document.readyState));".into(),
    });
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/", "text/html", r#"<!doctype html><html><head>
<style>#message { display: none }</style>
<script id="inline">seen.push(document.currentScript.id + ':' + document.readyState)</script>
<script id="defer" defer src="/defer.js"></script>
<script type="module">seen.push('module:' + document.currentScript)</script>
</head><body><p id="message">before</p><script>document.getElementById('message').textContent = 'after'; document.getElementById('defer').remove()</script></body></html>"#, Duration::ZERO);
    loader.insert(
        "http://site.test/defer.js",
        "text/javascript",
        "seen.push(document.currentScript.id);",
        Duration::from_millis(5),
    );
    loader.insert("http://site.test/init.js", "text/javascript", "globalThis.initModule = 1;", Duration::ZERO);
    thread.browser.set_loader(Some(loader));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(
        eval(&thread, page, "seen.join('|')"),
        "init:loading|inline:loading|defer|module:null|dcl:interactive|load:complete"
    );
    assert_eq!(eval(&thread, page, "document.querySelector('#message').textContent"), "after");
    assert_eq!(eval(&thread, page, "getComputedStyle(document.querySelector('#message')).display"), "none");
    assert_eq!(eval(&thread, page, "document.URL"), "http://site.test/");
    assert_eq!(eval(&thread, page, "initModule"), "1");
}

#[test]
fn isolated_worlds_share_dom_and_navigation_releases_old_binding_state() {
    let mut thread = crate::browser_thread().unwrap();
    let page = thread.browser.create_page().unwrap();
    let old = thread.browser.page(page).unwrap().main_frame.page_dom.as_ref().unwrap().clone();
    let weak = Rc::downgrade(&old);
    drop(old);
    eval(&thread, page, "document.body.innerHTML = '<p id=shared>main</p>'; document.body.addEventListener('click', () => {}); addEventListener('message', () => {}); 'ok'");
    let isolated = thread.browser.isolated_world(page, "automation").unwrap();
    {
        let s = isolated.scope().unwrap();
        assert_eq!(
            s.eval("document.getElementById('shared').textContent", None, 1).unwrap().to_string().unwrap(),
            "main"
        );
        s.eval("document.getElementById('shared').textContent = 'isolated'; globalThis.privateValue = 42", None, 1)
            .unwrap();
    }
    assert_eq!(
        eval(&thread, page, "document.getElementById('shared').textContent + ':' + typeof privateValue"),
        "isolated:undefined"
    );
    thread.browser.navigate(page, NavigateOpts::address_bar("about:blank")).unwrap();
    assert!(isolated.scope().is_err());
    drop(isolated);
    thread.browser.engine.collect_garbage_now();
    thread.browser.engine.drain_finalized();
    assert!(weak.upgrade().is_none(), "old PageDom retained after navigation and GC");
    assert_eq!(eval(&thread, page, "document.body.childNodes.length"), "0");
}

#[test]
fn binding_tasks_are_dropped_after_their_document_is_replaced() {
    let mut thread = crate::browser_thread().unwrap();
    let page = thread.browser.create_page().unwrap();
    let weak = Rc::downgrade(thread.browser.page(page).unwrap().main_frame.page_dom.as_ref().unwrap());
    eval(
        &thread,
        page,
        "addEventListener('message', () => { throw new Error('stale task ran') }); postMessage('queued', '*'); 'ok'",
    );
    thread.browser.navigate(page, NavigateOpts::address_bar("about:blank")).unwrap();
    settle(&mut thread);
    thread.browser.engine.collect_garbage_now();
    thread.browser.engine.drain_finalized();
    assert!(weak.upgrade().is_none());
    assert_eq!(eval(&thread, page, "document.readyState"), "complete");
}

#[test]
fn async_module_graph_delays_load_until_evaluation_finishes() {
    let mut thread = crate::browser_thread().unwrap();
    let page = thread.browser.create_page().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/", "text/html", "<script>addEventListener('load', () => globalThis.atLoad = globalThis.moduleValue)</script><script async type=module src='/entry.js'></script>", Duration::ZERO);
    loader.insert(
        "http://site.test/entry.js",
        "text/javascript",
        "import { answer } from './slow.js'; globalThis.moduleValue = answer;",
        Duration::ZERO,
    );
    loader.insert(
        "http://site.test/slow.js",
        "text/javascript",
        "export const answer = 42;",
        Duration::from_millis(40),
    );
    thread.browser.set_loader(Some(loader));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "moduleValue + ':' + atLoad"), "42:42");
}

#[test]
fn storage_survives_navigation_with_context_and_page_partitions() {
    let mut thread = crate::browser_thread().unwrap();
    let loader = Rc::new(StaticLoader::default());
    for url in ["http://site.test/one", "http://site.test/two", "http://other.test/"] {
        loader.insert(url, "text/html", "<p>storage</p>", Duration::ZERO);
    }
    thread.browser.set_loader(Some(loader));
    let context = thread.browser.create_context().unwrap();
    let a = thread.browser.create_page_in(context).unwrap();
    let b = thread.browser.create_page_in(context).unwrap();
    let isolated_context = thread.browser.create_context().unwrap();
    let c = thread.browser.create_page_in(isolated_context).unwrap();
    for page in [a, b, c] {
        thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/one")).unwrap();
    }
    settle(&mut thread);
    eval(&thread, a, "localStorage.setItem('token', 'synthetic'); sessionStorage.setItem('page', 'a'); 'ok'");
    assert_eq!(
        eval(&thread, b, "localStorage.getItem('token') + ':' + sessionStorage.getItem('page')"),
        "synthetic:null"
    );
    assert_eq!(eval(&thread, c, "localStorage.getItem('token')"), "null");
    let world = thread.browser.isolated_world(a, "automation").unwrap();
    assert_eq!(
        world
            .scope()
            .unwrap()
            .eval("localStorage.token + ':' + sessionStorage.page", None, 1)
            .unwrap()
            .to_string()
            .unwrap(),
        "synthetic:a"
    );
    let old = thread.browser.page(a).unwrap().main_frame.page_dom.as_ref().unwrap().clone();
    thread.browser.navigate(a, NavigateOpts::address_bar("http://site.test/two")).unwrap();
    settle(&mut thread);
    assert!(old.is_disposed());
    assert_eq!(eval(&thread, a, "localStorage.token + ':' + sessionStorage.page"), "synthetic:a");
    thread.browser.navigate(a, NavigateOpts::address_bar("http://other.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(eval(&thread, a, "localStorage.getItem('token') + ':' + sessionStorage.getItem('page')"), "null:null");
    thread.browser.navigate(a, NavigateOpts::address_bar("http://site.test/one")).unwrap();
    settle(&mut thread);
    assert_eq!(eval(&thread, a, "localStorage.token + ':' + sessionStorage.page"), "synthetic:a");
    let bindings = thread.browser.page(a).unwrap().main_frame.page_dom.as_ref().unwrap().clone();
    thread.browser.close_context(context);
    assert!(bindings.is_disposed());
    assert_eq!(eval(&thread, c, "localStorage.getItem('token')"), "null");
}

/// `<link rel=modulepreload>` / `<link rel=preload>` fetch their resource
/// and get `load` (or `error` for a missing one); Vite's preload helper
/// awaits these before a dynamic `import()`.
#[test]
fn preload_links_fire_load_and_error() {
    let mut thread = crate::browser_thread().unwrap();
    let page = thread.browser.create_page().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/", "text/html", r#"<!doctype html><html><head>
<link id="m" rel="modulepreload" href="/chunk.js">
<link id="p" rel="preload" as="script" href="/lib.js">
<link id="x" rel="modulepreload" href="/missing.js">
<script>globalThis.seen = []; for (const id of ['m', 'p', 'x']) { const l = document.getElementById(id); l.addEventListener('load', () => seen.push(id + ':load')); l.addEventListener('error', () => seen.push(id + ':error')); }</script>
</head><body></body></html>"#, Duration::ZERO);
    loader.insert("http://site.test/chunk.js", "text/javascript", "export const a = 1;", Duration::ZERO);
    loader.insert("http://site.test/lib.js", "text/javascript", "var b = 2;", Duration::ZERO);
    thread.browser.set_loader(Some(loader.clone()));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "seen.sort().join()"), "m:load,p:load,x:error");
    assert!(loader.log.borrow().iter().any(|u| u == "http://site.test/chunk.js"), "{:?}", loader.log.borrow());
    // A modulepreload link inserted later for the same module gets `load`
    // from the document's preloaded module; a preload link refetches.
    eval(&thread, page, "const l = document.createElement('link'); l.rel = 'modulepreload'; l.href = '/chunk.js'; l.addEventListener('load', () => seen.push('late:load')); document.head.appendChild(l);");
    eval(&thread, page, "const p = document.createElement('link'); p.rel = 'preload'; p.as = 'script'; p.href = '/lib.js'; p.addEventListener('load', () => seen.push('latep:load')); document.head.appendChild(p);");
    settle(&mut thread);
    assert!(eval(&thread, page, "seen.includes('late:load') && seen.includes('latep:load')") == "true");
    assert_eq!(loader.log.borrow().iter().filter(|u| *u == "http://site.test/chunk.js").count(), 1);
    assert_eq!(loader.log.borrow().iter().filter(|u| *u == "http://site.test/lib.js").count(), 2);
    // Once an import took the preloaded source, a link still loads from the
    // module map without a request.
    eval(&thread, page, "import('/chunk.js').then(m => seen.push('import:' + m.a));");
    settle(&mut thread);
    eval(&thread, page, "const l2 = document.createElement('link'); l2.rel = 'modulepreload'; l2.href = '/chunk.js'; l2.addEventListener('load', () => seen.push('after-import:load')); document.head.appendChild(l2);");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "seen.includes('import:1') && seen.includes('after-import:load')"), "true");
    assert_eq!(loader.log.borrow().iter().filter(|u| *u == "http://site.test/chunk.js").count(), 1);
}

/// `import()` of a module a `<link rel=modulepreload>` fetched takes the
/// preloaded source (Chromium's module map): one request, one evaluation,
/// even when the import runs while the preload is still in flight. A failed
/// preload rejects the import.
#[test]
fn modulepreload_source_is_reused_by_import() {
    let mut thread = crate::browser_thread().unwrap();
    let page = thread.browser.create_page().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert(
        "http://site.test/",
        "text/html",
        r#"<!doctype html><html><head>
<link id="m" rel="modulepreload" href="/chunk.js">
<link id="x" rel="modulepreload" href="/missing.js">
<script>
globalThis.evals = 0; globalThis.out = [];
import('/chunk.js').then(m => out.push('first:' + m.a + ':' + evals));
import('/missing.js').then(() => out.push('missing:ok'), () => out.push('missing:rejected'));
</script>
</head><body></body></html>"#,
        Duration::ZERO,
    );
    loader.insert(
        "http://site.test/chunk.js",
        "text/javascript",
        "globalThis.evals++; export const a = 1;",
        Duration::from_millis(50),
    );
    thread.browser.set_loader(Some(loader.clone()));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "out.sort().join()"), "first:1:1,missing:rejected");
    assert_eq!(
        loader.log.borrow().iter().filter(|u| *u == "http://site.test/chunk.js").count(),
        1,
        "{:?}",
        loader.log.borrow()
    );
    // A later import gets the same module instance.
    eval(&thread, page, "import('/chunk.js').then(m => out.push('second:' + m.a + ':' + evals));");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "out.sort().join()"), "first:1:1,missing:rejected,second:1:1");
    assert_eq!(loader.log.borrow().iter().filter(|u| *u == "http://site.test/chunk.js").count(), 1);
    // A modulepreload link after the import loads from the module map.
    eval(&thread, page, "const l = document.createElement('link'); l.rel = 'modulepreload'; l.href = '/chunk.js'; l.addEventListener('load', () => out.push('link:load')); document.head.appendChild(l);");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "out.sort().join()"), "first:1:1,link:load,missing:rejected,second:1:1");
    assert_eq!(loader.log.borrow().iter().filter(|u| *u == "http://site.test/chunk.js").count(), 1);
    // The next document starts without the cache.
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "out.sort().join()"), "first:1:1,missing:rejected");
    assert_eq!(loader.log.borrow().iter().filter(|u| *u == "http://site.test/chunk.js").count(), 2);
}

/// A script without a URL uses the current document base URL, including after
/// `history.pushState`.
#[test]
fn import_without_a_script_url_uses_the_document_base() {
    if unsafe { lp_js::sys::modules::lp_jsc_import_maps_available() } != 1 {
        eprintln!("needs the resolve hook of the patched JavaScriptCore; skipping");
        return;
    }
    let mut thread = crate::browser_thread().unwrap();
    let page = thread.browser.create_page().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/a/", "text/html", "<!doctype html><title>t</title>", Duration::ZERO);
    for path in ["/a/rel.js", "/b/rel.js"] {
        loader.insert(
            &format!("http://site.test{path}"),
            "text/javascript",
            "export const url = import.meta.url;",
            Duration::ZERO,
        );
    }
    thread.browser.set_loader(Some(loader.clone()));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/a/")).unwrap();
    settle(&mut thread);
    eval(&thread, page, "globalThis.out = []; import('./rel.js').then(m => out.push(m.url), e => out.push('' + e));");
    settle(&mut thread);
    eval(
        &thread,
        page,
        "history.pushState(null, '', '/b/'); import('./rel.js').then(m => out.push(m.url), e => out.push('' + e));",
    );
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "out.join()"), "http://site.test/a/rel.js,http://site.test/b/rel.js");
}

// ---- Review (perf/realms): deferred isolated worlds against worlds made at commit ----

/// What a world sees of its window: every install.js piece, the frames, the document.
const WORLD_PROBE: &str = r#"[
    Object.getOwnPropertyNames(window).join(),
    Object.getOwnPropertyNames(window).map(k => { const d = Object.getOwnPropertyDescriptor(window, k);
        return ('value' in d ? typeof d.value : 'acc') + (d.writable ? 'w' : '') + (d.enumerable ? 'e' : '') + (d.configurable ? 'c' : ''); }).join(),
    Object.keys(window).join(), location.href, document.readyState, window.length, typeof window[0],
    window[0] && window[0].document.querySelector('span').textContent, window.origin, self === window,
    structuredClone({a: [1, {b: 2}]}).a[1].b, new TextDecoder().decode(new DataView(new Uint8Array([104, 105]).buffer)),
    new Headers({a: 'b'}).get('a'), [...new Headers([['x', 'y']])].join(), [...new URLSearchParams('a=1&b=2')].join(';'),
    (() => { const f = new FormData(); f.append('k', 'v'); let s = ''; f.forEach((v, k) => s += k + v); return [...f].join() + s; })(),
    Object.keys(Element.prototype[Symbol.unscopables]).join(), Object.keys(Document.prototype[Symbol.unscopables]).join(),
    String(new IntersectionObserver(() => {}, {threshold: [0.5, 0]}).thresholds),
    (() => { try { new MutationObserver(() => {}).observe(document.body, {attributeFilter: null}); return 'no'; } catch (e) { return e.constructor.name; } })(),
    Object.prototype.toString.call(new URLSearchParams().entries()), Object.prototype.toString.call(new FormData().entries()),
    document.querySelector('p').textContent, typeof performance.timeOrigin, Node.name, Object.getPrototypeOf(HTMLElement) === Element,
    typeof NodeFilter, 'prototype' in NodeFilter, delete window.Node, typeof Node,
].join('|')"#;

fn world_eval(realm: &lp_js::Realm, source: &str) -> String {
    let scope = realm.scope().unwrap();
    scope.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}

fn two_worlds_page(html: &str) -> (BrowserThread, PageId) {
    let mut thread = crate::browser_thread().unwrap();
    let page = thread.browser.create_page().unwrap();
    let script = |id: &str, world: &str, source: &str| InitScript { id: id.into(), world: Some(world.into()), source: source.into() };
    thread.browser.add_init_script(page, script("l", "lazy", " \n"));
    thread.browser.add_init_script(page, script("m", "made", "void 0"));
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/top", "text/html", html, Duration::ZERO);
    loader.insert("http://site.test/child", "text/html", "<span>child</span>", Duration::ZERO);
    thread.browser.set_loader(Some(loader));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/top")).unwrap();
    settle(&mut thread);
    (thread, page)
}

#[test]
fn review_a_deferred_world_sees_what_a_world_made_at_commit_sees() {
    let (mut thread, page) = two_worlds_page("<p>top</p><iframe src=/child></iframe>");
    let frame = thread.browser.page(page).unwrap().main_frame.clone();
    let lazy = frame.world("lazy").unwrap().clone();
    let made = frame.world("made").unwrap().clone();
    assert!(lazy.is_deferred() && !made.is_deferred());
    // Frames come and go while it waits.
    eval(&thread, page, "const f = document.createElement('iframe'); document.body.appendChild(f); f.remove(); 1");
    settle(&mut thread);
    assert!(lazy.is_deferred());
    let a = world_eval(&lazy, WORLD_PROBE);
    let b = world_eval(&made, WORLD_PROBE);
    assert_eq!(a, b);
    assert!(a.contains("|top|"), "{a}");
    // The child frame's worlds too.
    let child = thread.browser.frames(page).into_iter().find(|f| f.parent.is_some()).unwrap();
    let child = thread.browser.frame(page, &child.id).unwrap();
    let (cl, cm) = (child.world("lazy").unwrap().clone(), child.world("made").unwrap().clone());
    let probe = "[document.querySelector('span').textContent, location.href, window.frameElement === null, parent === top, parent.length, Object.getOwnPropertyNames(window).length].join()";
    assert_eq!(world_eval(&cl, probe), world_eval(&cm, probe));
}

#[test]
fn review_a_frame_added_after_a_deferred_world_was_made_gets_its_index() {
    let (mut thread, page) = two_worlds_page("<p>top</p>");
    let lazy = thread.browser.page(page).unwrap().main_frame.world("lazy").unwrap().clone();
    assert_eq!(world_eval(&lazy, "window.length"), "0");
    eval(&thread, page, "document.body.appendChild(document.createElement('iframe')); 1");
    settle(&mut thread);
    assert_eq!(world_eval(&lazy, "[window.length, typeof window[0], Object.keys(window).includes('0')].join()"), "1,object,true");
    eval(&thread, page, "document.querySelector('iframe').remove(); 1");
    settle(&mut thread);
    assert_eq!(world_eval(&lazy, "[window.length, typeof window[0]].join()"), "0,undefined");
}

#[test]
fn review_the_page_clock_turned_on_while_a_world_waits_reaches_it() {
    let (mut thread, page) = two_worlds_page("<p>top</p>");
    let frame = thread.browser.page(page).unwrap().main_frame.clone();
    let lazy = frame.world("lazy").unwrap().clone();
    let made = frame.world("made").unwrap().clone();
    assert!(lazy.is_deferred());
    thread.browser.advance_time_explicit(Duration::from_secs(10 * 86_400)).unwrap();
    settle(&mut thread);
    assert!(lazy.is_deferred(), "turning the clock on does not make the world");
    let skew = "Math.round((Date.now() - Date_real_now()) / 86400000)";
    let real = format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis());
    let probe = skew.replace("Date_real_now()", &real);
    assert_eq!(world_eval(&made, &probe), "10");
    assert_eq!(world_eval(&lazy, &probe), "10", "the deferred world's Date reads the page clock");
    assert_eq!(world_eval(&lazy, "Math.round((new Date().getTime() - Date.now()) / 1000)"), "0");
    assert_eq!(eval(&thread, page, &probe), "10");
}

#[test]
fn review_bindings_make_only_the_worlds_they_apply_to_and_once() {
    let (mut thread, page) = two_worlds_page("<p>top</p>");
    fn thread_world(thread: &BrowserThread, page: PageId, name: &str) -> lp_js::Realm {
        thread.browser.page(page).unwrap().main_frame.world(name).unwrap().clone()
    }
    let w = thread_world(&thread, page, "lazy");
    assert!(w.is_deferred());
    // A binding of another world, then its removal: the deferred world stays deferred.
    thread.browser.add_binding(page, lp_browser::Binding { name: "other".into(), world: Some("made".into()) });
    assert!(w.is_deferred());
    assert!(thread.browser.remove_binding(page, "other"));
    assert!(w.is_deferred());
    // A binding of this world makes it, defined once.
    thread.browser.add_binding(page, lp_browser::Binding { name: "mine".into(), world: Some("lazy".into()) });
    assert!(!w.is_deferred());
    assert_eq!(
        world_eval(&w, "[typeof mine, Object.getOwnPropertyNames(window).filter(k => k === 'mine').length, Object.keys(window).includes('mine')].join()"),
        "function,1,false"
    );
    assert_eq!(eval(&thread, page, "typeof mine"), "undefined");
    assert!(thread.browser.remove_binding(page, "mine"));
    assert_eq!(world_eval(&w, "typeof mine"), "undefined");
    // The next document's world is deferred again (no binding left).
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/next", "text/html", "<p>next</p>", Duration::ZERO);
    thread.browser.set_loader(Some(loader));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/next")).unwrap();
    settle(&mut thread);
    assert!(w.is_disposed());
    assert!(thread_world(&thread, page, "lazy").is_deferred());
    // A binding for every world (Runtime.addBinding without a name) reaches it too.
    thread.browser.add_binding(page, lp_browser::Binding { name: "all".into(), world: None });
    assert_eq!(world_eval(&thread_world(&thread, page, "lazy"), "typeof all"), "function");
}

#[test]
fn review_every_announced_deferred_world_is_destroyed_with_its_document_and_page() {
    #[derive(Default)]
    struct Life(RefCell<Vec<(bool, lp_js::RealmId)>>);
    impl PageObserver for Life {
        fn on_page_event(&self, _page: PageId, event: &PageEvent) {
            match event {
                PageEvent::RealmCreated { realm, .. } => self.0.borrow_mut().push((true, *realm)),
                PageEvent::RealmDestroyed { realm, .. } => self.0.borrow_mut().push((false, *realm)),
                _ => {}
            }
        }
    }
    let (mut thread, page) = two_worlds_page("<p>top</p><iframe src=/child></iframe>");
    let life = Rc::new(Life::default());
    thread.browser.set_observer(Some(life.clone()));
    let lazy = thread.browser.page(page).unwrap().main_frame.world("lazy").unwrap().clone();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/next", "text/html", "<p>next</p><iframe src=/child></iframe>", Duration::ZERO);
    loader.insert("http://site.test/child", "text/html", "<span>child</span>", Duration::ZERO);
    thread.browser.set_loader(Some(loader));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/next")).unwrap();
    settle(&mut thread);
    assert!(lazy.is_disposed(), "never made, disposed with its document");
    assert!(life.0.borrow().contains(&(false, lazy.id())), "{:?}", life.0.borrow());
    let created: Vec<_> = life.0.borrow().iter().filter(|(c, _)| *c).map(|(_, id)| *id).collect();
    assert!(thread.browser.close_page(page));
    settle(&mut thread);
    for id in created {
        assert!(life.0.borrow().contains(&(false, id)), "realm {id:?} announced but never destroyed: {:?}", life.0.borrow());
    }
}
