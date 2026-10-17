//! Browser-owned installation/lifecycle against real JSC and the document loader.
use lp_browser::{BrowserThread, InitScript, NavigateOpts, PageId, StaticLoader};
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
fn bindings_exist_before_init_scripts_and_follow_document_lifecycle() {
    let mut thread = BrowserThread::new().unwrap();
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
    let mut thread = BrowserThread::new().unwrap();
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
    let mut thread = BrowserThread::new().unwrap();
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
    let mut thread = BrowserThread::new().unwrap();
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
    let mut thread = BrowserThread::new().unwrap();
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
