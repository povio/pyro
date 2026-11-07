//! Fragment navigations fire `popstate` synchronously, then a queued
//! `hashchange` (html/browsers/browsing-the-web/history-traversal/event-order);
//! the Location setters navigate (html/browsers/history/the-location-interface);
//! leaving or closing a document fires `pagehide`, `visibilitychange` and
//! `unload` (html/browsers/browsing-the-web/unloading-documents).
use lp_browser::{BrowserThread, NavigateOpts, PageId, StaticLoader};
use std::rc::Rc;
use std::time::{Duration, Instant};

fn eval(t: &BrowserThread, page: PageId, source: &str) -> String {
    let s = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().scope().unwrap();
    s.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&s).text())).to_string().unwrap()
}

fn settle(t: &mut BrowserThread) {
    assert_eq!(t.run_until_idle(Some(Instant::now() + Duration::from_secs(2))), lp_runtime::RunOutcome::Idle);
}

fn load(html: &str) -> (BrowserThread, PageId, Rc<StaticLoader>) {
    let mut t = crate::browser_thread().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/page?q=1", "text/html", html, Duration::ZERO);
    for url in ["http://site.test/other", "http://site.test/page?x=2", "http://site.test/elsewhere?q=1"] {
        loader.insert(url, "text/html", "<p id=other>other</p>", Duration::ZERO);
    }
    t.browser.set_loader(Some(loader.clone()));
    let page = t.browser.create_page().unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar("http://site.test/page?q=1")).unwrap();
    settle(&mut t);
    loader.log.borrow_mut().clear();
    (t, page, loader)
}

const EVENTS: &str = "<body><a id=link href='#foo'>foo</a><script>window.order = [];\
    onpopstate = e => order.push('popstate:' + e.state);\
    onhashchange = e => order.push('hashchange:' + new URL(e.newURL).hash);</script></body>";

#[test]
fn a_hash_assignment_fires_popstate_at_once_then_hashchange() {
    let (mut t, page, _loader) = load(EVENTS);
    assert_eq!(eval(&t, page, "location.hash = '#1'; order.join()"), "popstate:null");
    settle(&mut t);
    assert_eq!(eval(&t, page, "order.join()"), "popstate:null,hashchange:#1");
    // The same hash again navigates nowhere.
    assert_eq!(eval(&t, page, "order = []; location.hash = '1'; order.join() + history.length"), "2");
    // href with a fragment is a fragment navigation too.
    assert_eq!(eval(&t, page, "location.href = '#2'; order.join()"), "popstate:null");
    settle(&mut t);
    assert_eq!(eval(&t, page, "order.join() + ' ' + history.length"), "popstate:null,hashchange:#2 3");
}

#[test]
fn an_empty_hash_on_a_url_without_one_does_nothing() {
    let (mut t, page, _loader) = load(EVENTS);
    assert_eq!(eval(&t, page, "location.hash = ''; order.join() + ' ' + location.href"), " http://site.test/page?q=1");
    settle(&mut t);
    assert_eq!(eval(&t, page, "order.length"), "0");
    // From a fragment, the empty one leaves '#'.
    eval(&t, page, "location.hash = 'test'; location.hash = ''");
    assert!(eval(&t, page, "location.href").ends_with('#'));
}

#[test]
fn a_clicked_fragment_link_fires_popstate_even_for_the_same_url() {
    let (mut t, page, _loader) = load(EVENTS);
    assert_eq!(eval(&t, page, "link.click(); order.join()"), "popstate:null");
    let length = eval(&t, page, "history.length");
    assert_eq!(eval(&t, page, "link.click(); order.join()"), "popstate:null,popstate:null");
    assert_eq!(eval(&t, page, "history.length"), length, "the same URL replaces");
    settle(&mut t);
    // hashchange only when the fragment changed.
    assert_eq!(eval(&t, page, "order.filter(e => e.startsWith('hash')).join()"), "hashchange:#foo");
}

#[test]
fn location_search_and_pathname_navigate() {
    let (mut t, page, loader) = load("<p>page</p>");
    eval(&t, page, "location.search = '?x=2'");
    settle(&mut t);
    assert_eq!(
        eval(&t, page, "location.href + ' ' + !!document.getElementById('other')"),
        "http://site.test/page?x=2 true"
    );
    eval(&t, page, "history.back()");
    settle(&mut t);
    eval(&t, page, "location.pathname = 'elsewhere'");
    settle(&mut t);
    assert_eq!(eval(&t, page, "location.href"), "http://site.test/elsewhere?q=1");
    assert!(loader.log.borrow().iter().any(|u| u.contains("elsewhere")));
}

#[test]
fn location_setters_follow_the_url_parser() {
    let (t, page, _loader) = load("<p>page</p>");
    // Not a scheme: SyntaxError.
    assert_eq!(
        eval(&t, page, "['', ' ', ':', 'x ', '\\u2020'].map(v => { try { location.protocol = v; return 'ok' } catch (e) { return e.name } }).join()"),
        "SyntaxError,SyntaxError,SyntaxError,SyntaxError,SyntaxError"
    );
    // A valid scheme that is not HTTP(S), or one that cannot replace it,
    // changes nothing.
    assert_eq!(
        eval(&t, page, "location.protocol = 'x'; location.protocol = 'file'; location.href"),
        "http://site.test/page?q=1"
    );
    // Setters strictly assign (no TypeError in strict code).
    assert_eq!(
        eval(&t, page, "'use strict'; (() => { 'use strict'; location.port = location.port; return typeof Object.getOwnPropertyDescriptor(Location.prototype, 'search').set })()"),
        "function"
    );
}

const UNLOAD: &str = "<body><script>\
    const log = e => { const l = JSON.parse(sessionStorage.log || '[]');\
      l.push(e.type + ':' + document.visibilityState + (e.type == 'pagehide' ? ':' + e.persisted : ''));\
      sessionStorage.log = JSON.stringify(l); };\
    addEventListener('pagehide', log); document.addEventListener('visibilitychange', log); onunload = log;\
    </script></body>";

#[test]
fn navigating_away_fires_pagehide_visibilitychange_and_unload() {
    let (mut t, page, _loader) = load(UNLOAD);
    assert_eq!(eval(&t, page, "document.visibilityState + ' ' + document.hidden"), "visible false");
    eval(&t, page, "location = '/other'");
    settle(&mut t);
    assert_eq!(
        eval(&t, page, "sessionStorage.log + ' ' + document.visibilityState"),
        r#"["pagehide:visible:false","visibilitychange:hidden","unload:hidden"] visible"#
    );
}

#[test]
fn a_child_frame_unloads_with_its_parent() {
    let mut t = crate::browser_thread().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/parent", "text/html", "<iframe src='/child'></iframe>", Duration::ZERO);
    loader.insert(
        "http://site.test/child",
        "text/html",
        "<script>onunload = () => { sessionStorage.order = (sessionStorage.order || '') + 'child,' };\
         parent.onpagehide = () => { sessionStorage.order = (sessionStorage.order || '') + 'parent,' };</script>",
        Duration::ZERO,
    );
    loader.insert("http://site.test/other", "text/html", "<p>other</p>", Duration::ZERO);
    t.browser.set_loader(Some(loader.clone()));
    let page = t.browser.create_page().unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar("http://site.test/parent")).unwrap();
    settle(&mut t);
    eval(&t, page, "location = '/other'");
    settle(&mut t);
    // The child's document unloads before its parent's.
    assert_eq!(eval(&t, page, "sessionStorage.order"), "child,parent,");
}

#[test]
fn closing_a_page_unloads_its_document() {
    let mut t = crate::browser_thread().unwrap();
    let loader = Rc::new(StaticLoader::default());
    let page_html = UNLOAD.replace("sessionStorage", "localStorage");
    loader.insert("http://site.test/page", "text/html", &page_html, Duration::ZERO);
    loader.insert("http://site.test/watch", "text/html", "<p>watch</p>", Duration::ZERO);
    t.browser.set_loader(Some(loader.clone()));
    let page = t.browser.create_page().unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar("http://site.test/page")).unwrap();
    settle(&mut t);
    assert!(t.browser.close_page(page));
    settle(&mut t);
    // localStorage outlives the page: another page of the origin reads it.
    let other = t.browser.create_page().unwrap();
    t.browser.navigate(other, NavigateOpts::address_bar("http://site.test/watch")).unwrap();
    settle(&mut t);
    assert_eq!(
        eval(&t, other, "localStorage.log"),
        r#"["pagehide:visible:false","visibilitychange:hidden","unload:hidden"]"#
    );
}
