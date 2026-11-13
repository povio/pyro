//! Script of a document whose frame was removed (`frameElement.remove()`)
//! keeps running until its callback returns, but the document is no longer
//! fully active: its event listeners are not invoked (HTML "check if we can
//! run script") and an Observable neither subscribes nor notifies there.

use pyro_browser::*;
use pyro_js::Realm;
use std::time::{Duration, Instant};

fn eval(realm: &Realm, source: &str) -> String {
    let scope = realm.scope().unwrap();
    scope
        .eval(source, Some("detached-test.js"), 1)
        .unwrap_or_else(|e| panic!("{}: {}", source, e.details(&scope).text()))
        .to_string()
        .unwrap()
}

/// A page with one blank iframe: the page's realm and the child's.
fn page_with_child(t: &mut BrowserThread) -> (Realm, Realm) {
    let page = t.browser.create_page().unwrap();
    let root = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
    eval(&root, "globalThis.results=[];globalThis.f=document.createElement('iframe');document.body.appendChild(f)");
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(3)));
    let child = t.browser.frames(page).into_iter().find(|f| f.parent.is_some()).unwrap();
    let realm = t.browser.frame(page, &child.id).unwrap().main_realm().unwrap().clone();
    (root, realm)
}

#[test]
fn listeners_of_a_detached_document_are_not_invoked() {
    let mut t = crate::browser_thread().unwrap();
    let (root, child) = page_with_child(&mut t);
    eval(
        &child,
        "const results = parent.results; const target = new EventTarget(); \
         target.addEventListener('x', () => results.push('listener')); \
         target.dispatchEvent(new Event('x')); \
         window.frameElement.remove(); results.push('detached'); \
         target.dispatchEvent(new Event('x')); results.push('after');",
    );
    assert_eq!(eval(&root, "results.join()"), "listener,detached,after");
}

#[test]
fn observable_does_nothing_in_a_detached_document() {
    let mut t = crate::browser_thread().unwrap();
    // The producer detaches its document: no handler runs, nothing is reported.
    let (root, child) = page_with_child(&mut t);
    eval(
        &child,
        "const results = parent.results; addEventListener('error', () => results.push('reported')); \
         new Observable(s => { \
           results.push('subscribe'); s.addTeardown(() => results.push('teardown')); \
           window.frameElement.remove(); results.push('detached'); \
           s.next('next'); s.addTeardown(() => results.push('late teardown')); s.complete(); s.error('error'); \
         }).map(v => { results.push('mapped'); return v; }) \
           .subscribe({ next: v => results.push(v), complete: () => results.push('complete') });",
    );
    assert_eq!(eval(&root, "results.join()"), "subscribe,detached");
    // Subscribing, when() and the promise-returning operators after the detach.
    let (root, child) = page_with_child(&mut t);
    eval(
        &child,
        "const results = parent.results; const target = new EventTarget(); \
         const source = new Observable(() => results.push('subscribe')); \
         const events = target.when('x'); events.subscribe(() => results.push('event')); \
         target.dispatchEvent(new Event('x')); \
         window.frameElement.remove(); results.push('detached'); \
         source.subscribe(); source.takeUntil(new Observable(() => results.push('notifier'))).subscribe(); \
         source.forEach(() => results.push('visited')); \
         target.dispatchEvent(new Event('x')); results.push(String(target.when('x')));",
    );
    assert_eq!(eval(&root, "results.join()"), "event,detached,undefined");
}
