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
