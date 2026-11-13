//! Started document fetches must not outlive the navigation that owns them.
use pyro_browser::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

#[derive(Default)]
struct Events(RefCell<Vec<String>>);
impl PageObserver for Events {
    fn on_page_event(&self, _: PageId, event: &PageEvent) {
        match event {
            PageEvent::FrameNavigated { url, .. } => self.0.borrow_mut().push(format!("commit:{url}")),
            PageEvent::FrameNavigateFailed { url, .. } => self.0.borrow_mut().push(format!("fail:{url}")),
            _ => {}
        }
    }
}

fn settle(t: &mut BrowserThread) {
    assert_eq!(t.run_until_idle(Some(Instant::now() + Duration::from_secs(2))), pyro_runtime::RunOutcome::Idle);
}
fn eval(t: &BrowserThread, page: PageId, source: &str) -> String {
    let s = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().scope().unwrap();
    s.eval(source, None, 1).unwrap().to_string().unwrap()
}

#[test]
fn blank_navigation_supersedes_started_document_success_and_failure() {
    for success in [true, false] {
        let mut t = crate::browser_thread().unwrap();
        let page = t.browser.create_page().unwrap();
        let loader = Rc::new(StaticLoader::default());
        if success {
            loader.insert("http://old.test/", "text/html", "<p>old</p>", Duration::ZERO);
        }
        t.browser.set_loader(Some(loader));
        t.browser.navigate(page, NavigateOpts::address_bar("http://old.test/")).unwrap();
        t.browser.navigate(page, NavigateOpts::address_bar("about:blank")).unwrap();
        let realm = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
        let bindings = Rc::downgrade(t.browser.page(page).unwrap().main_frame.page_dom.as_ref().unwrap());
        let events = Rc::new(Events::default());
        t.browser.set_observer(Some(events.clone()));
        settle(&mut t);
        assert_eq!(eval(&t, page, "location.href+'|'+document.URL"), "about:blank|about:blank");
        assert!(!realm.is_disposed(), "late response disposed the replacement realm");
        assert!(bindings.upgrade().is_some(), "late response replaced the live DOM");
        assert!(events.0.borrow().is_empty(), "stale document event: {:?}", events.0.borrow());
    }
}

#[test]
fn failed_replacement_does_not_replace_the_next_navigation_history_entry() {
    for asynchronous in [false, true] {
        let mut t = crate::browser_thread().unwrap();
        let page = t.browser.create_page().unwrap();
        let loader = Rc::new(StaticLoader::default());
        loader.insert("http://new.test/", "text/html", "<p>new</p>", Duration::ZERO);
        if asynchronous {
            t.browser.set_loader(Some(loader.clone()));
        }
        let result = t.browser.navigate_with_history(page, NavigateOpts::address_bar("http://missing.test/"), true);
        assert_eq!(result.is_ok(), asynchronous);
        settle(&mut t);
        t.browser.set_loader(Some(loader));
        t.browser.navigate(page, NavigateOpts::address_bar("http://new.test/")).unwrap();
        settle(&mut t);
        // The navigation replaced the initial about:blank entry (Chrome);
        // the failed replacement left no entry of its own.
        assert_eq!(eval(&t, page, "history.length"), "1", "failed replacement leaked into next navigation");
    }
}

#[test]
fn superseded_replacement_does_not_replace_the_winning_navigation_history_entry() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://old.test/", "text/html", "<p>old</p>", Duration::ZERO);
    loader.insert("http://new.test/", "text/html", "<p>new</p>", Duration::ZERO);
    t.browser.set_loader(Some(loader));
    t.browser.navigate_with_history(page, NavigateOpts::address_bar("http://old.test/"), true).unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar("http://new.test/")).unwrap();
    settle(&mut t);
    assert_eq!(eval(&t, page, "location.href+'|'+history.length"), "http://new.test/|1");
}

#[test]
fn successful_replacement_still_replaces_exactly_one_entry() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let loader = Rc::new(StaticLoader::default());
    for url in ["http://old.test/", "http://new.test/"] {
        loader.insert(url, "text/html", "<p>page</p>", Duration::ZERO);
    }
    t.browser.set_loader(Some(loader));
    t.browser.navigate(page, NavigateOpts::address_bar("http://old.test/")).unwrap();
    settle(&mut t);
    t.browser.navigate_with_history(page, NavigateOpts::address_bar("http://new.test/"), true).unwrap();
    settle(&mut t);
    // old.test replaced the initial about:blank entry; new.test replaced old.test.
    assert_eq!(eval(&t, page, "location.href+'|'+history.length"), "http://new.test/|1");
}
