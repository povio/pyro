//! HTML's promise rejection tracking through the browser (real JSC): `unhandledrejection` fires
//! from a task queued at the end of the microtask checkpoint that left the promise without a
//! handler, never for a promise that gets one before that task runs, and `rejectionhandled` from
//! a task once a handler is added to a promise reported before. Needs JavaScriptCore's tracker
//! (patch 0036): without it rejections are reported as the microtask queue empties, as before.
//! WPT html/webappapis/scripting/processing-model-2/unhandled-promise-rejections.
use pyro_browser::{BrowserThread, NavigateOpts, PageId, StaticLoader};
use std::rc::Rc;
use std::time::{Duration, Instant};

fn eval(thread: &BrowserThread, page: PageId, source: &str) -> String {
    let realm = thread.browser.page(page).unwrap().main_frame.main_realm().unwrap();
    let scope = realm.scope().unwrap();
    scope.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}
fn settle(thread: &mut BrowserThread) {
    assert_ne!(thread.run_until_idle(Some(Instant::now() + Duration::from_secs(3))), pyro_runtime::RunOutcome::TimedOut);
}
fn load(script: &str) -> (BrowserThread, PageId) {
    let mut thread = crate::browser_thread().unwrap();
    let page = thread.browser.create_page().unwrap();
    let loader = Rc::new(StaticLoader::default());
    let prelude = "var log = []; addEventListener('unhandledrejection', e => { log.push('unhandled:' + e.reason); e.preventDefault(); }); addEventListener('rejectionhandled', e => log.push('handled:' + e.reason));";
    loader.insert("http://site.test/", "text/html", &format!("<body><script>{prelude}{script}</script>"), Duration::ZERO);
    thread.browser.set_loader(Some(loader));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    (thread, page)
}

#[test]
fn unhandledrejection_fires_from_a_task_after_the_tasks_queued_before_its_checkpoint() {
    if !pyro_js::rejection_tracking_available() {
        return;
    }
    // The toggle task is queued while the script runs, the notification at its checkpoint; the
    // second rejection's notification is queued after the second toggle task.
    let (thread, page) = load(
        "function task(f) { var d = document.createElement('details'); d.ontoggle = f; d.setAttribute('open', ''); }
         Promise.reject('first');
         task(() => { log.push('task 1'); Promise.reject('second'); task(() => log.push('task 2')); });
         log.push('script');",
    );
    assert_eq!(eval(&thread, page, "log.join(', ')"), "script, task 1, unhandled:first, task 2, unhandled:second");
}

#[test]
fn a_handler_added_before_the_notification_task_prevents_unhandledrejection() {
    if !pyro_js::rejection_tracking_available() {
        return;
    }
    // The task queued before the checkpoint runs before the notification: its handler takes the
    // promise off the about-to-be-notified list.
    let (thread, page) = load(
        "function task(f) { var d = document.createElement('details'); d.ontoggle = f; d.setAttribute('open', ''); }
         var late = Promise.reject('late');
         task(() => late.catch(() => log.push('caught')));
         Promise.reject('never');",
    );
    assert_eq!(eval(&thread, page, "log.join(', ')"), "caught, unhandled:never");
}

#[test]
fn rejectionhandled_fires_from_a_task_for_a_promise_reported_before() {
    if !pyro_js::rejection_tracking_available() {
        return;
    }
    let (mut thread, page) = load("var p = Promise.reject('reported'); var q = Promise.reject('other');");
    assert_eq!(eval(&thread, page, "log.join(', ')"), "unhandled:reported, unhandled:other");
    // Not at once: from a task.
    assert_eq!(eval(&thread, page, "p.catch(() => {}); log.length"), "2");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "log.join(', ')"), "unhandled:reported, unhandled:other, handled:reported");
    // Once per promise.
    eval(&thread, page, "p.catch(() => {}); p.then(null, () => {}); 'ok'");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "log.length"), "3");
}
