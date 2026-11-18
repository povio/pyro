//! Navigation work from a replaced document must not affect its successor.
use pyro_browser::{BrowserThread, NavigateOpts, PageId, StaticLoader};
use std::rc::Rc;
use std::time::{Duration, Instant};

fn eval(t: &BrowserThread, page: PageId, source: &str) -> String {
    let s = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().scope().unwrap();
    s.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&s).text())).to_string().unwrap()
}
fn settle(t: &mut BrowserThread) {
    assert_eq!(t.run_until_idle(Some(Instant::now() + Duration::from_secs(2))), pyro_runtime::RunOutcome::Idle);
}
fn assert_blank(t: &BrowserThread, page: PageId) {
    assert_eq!(t.browser.page(page).unwrap().main_frame.url, "about:blank");
    assert_eq!(eval(t, page, "location.href+'|'+document.URL"), "about:blank|about:blank");
}

#[test]
fn old_fragment_notification_does_not_change_replacement_frame_url() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    eval(&t, page, "location.hash='stale'");
    t.browser.navigate(page, NavigateOpts::address_bar("about:blank")).unwrap();
    settle(&mut t);
    assert_blank(&t, page);
}

#[test]
fn old_script_navigation_and_reload_are_cancelled_before_starting_transport() {
    for script in ["location.href='http://stale.test/'", "location.reload()"] {
        let mut t = crate::browser_thread().unwrap();
        let page = t.browser.create_page().unwrap();
        let loader = Rc::new(StaticLoader::default());
        loader.insert("http://stale.test/", "text/html", "<p>stale</p>", Duration::ZERO);
        t.browser.set_loader(Some(loader.clone()));
        eval(&t, page, script);
        t.browser.navigate(page, NavigateOpts::address_bar("about:blank")).unwrap();
        let replacement = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
        settle(&mut t);
        assert_blank(&t, page);
        assert!(!replacement.is_disposed(), "stale reload replaced the new realm");
        assert!(loader.log.borrow().is_empty(), "stale document started transport");
    }
}

#[test]
fn old_history_traversal_does_not_move_replacement_document_or_index() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    eval(&t, page, "location.hash='one';location.hash='two'");
    settle(&mut t);
    eval(&t, page, "history.back()");
    t.browser.navigate(page, NavigateOpts::address_bar("about:blank")).unwrap();
    settle(&mut t);
    assert_blank(&t, page);
    eval(&t, page, "history.replaceState({fresh:true},'')");
    assert_eq!(eval(&t, page, "history.state.fresh"), "true");
}

#[test]
fn enormous_history_delta_is_ignored_without_overflow() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    eval(&t, page, "history.pushState({x:1},'')");
    assert_eq!(eval(&t, page, "history.go(Number.MAX_VALUE);history.go(-Number.MAX_VALUE);'ok'"), "ok");
    settle(&mut t);
    assert_eq!(eval(&t, page, "history.state.x"), "1");
    assert_blank(&t, page);
}

#[test]
fn queued_traversal_does_not_apply_a_forward_entry_replaced_by_push_state() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    eval(&t, page, "location.hash='one';location.hash='two'");
    settle(&mut t);
    eval(&t, page, "history.back()");
    settle(&mut t);
    assert_eq!(eval(&t, page, "location.hash"), "#one");
    eval(&t, page, "history.forward();history.pushState({newEntry:true},'')");
    settle(&mut t);
    assert_eq!(eval(&t, page, "location.hash+'|'+history.state.newEntry"), "#one|true");
    assert_eq!(t.browser.page(page).unwrap().main_frame.url, "about:blank#one");
}

#[test]
fn repeated_reload_before_navigation_fetches_once() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/", "text/html", "<p>page</p>", Duration::ZERO);
    t.browser.set_loader(Some(loader.clone()));
    t.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut t);
    loader.log.borrow_mut().clear();
    eval(&t, page, "location.reload();location.reload()");
    settle(&mut t);
    assert_eq!(loader.log.borrow().len(), 1);
}
