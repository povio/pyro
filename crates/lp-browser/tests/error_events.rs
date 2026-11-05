//! Window `error` events for uncaught exceptions through the browser: page
//! realms get `lifecycle::report_exception` as their exception reporter, so
//! inline scripts, timers and microtasks that throw reach `window.onerror`
//! (five arguments, `true` cancels the console report) and an isolated
//! world's exceptions do not reach the page's handler.

use lp_browser::{NavigateOpts, PageId, StaticLoader};
use lp_js::{EngineHooks, JsException, Realm};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

#[derive(Default)]
struct Recorder {
    exceptions: RefCell<Vec<String>>,
}

impl EngineHooks for Recorder {
    fn exception_thrown(&self, _realm: &Realm, e: &JsException) {
        self.exceptions.borrow_mut().push(e.text());
    }
}

fn eval(thread: &lp_browser::BrowserThread, page: PageId, script: &str) -> String {
    let realm = thread.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
    let s = realm.scope().unwrap();
    match s.eval(script, None, 1) {
        Ok(v) => v.to_string().unwrap(),
        Err(e) => format!("THROW {}", e.details(&s).text()),
    }
}

const PAGE: &str = r#"<!doctype html><html><head><script>
globalThis.seen = [];
window.onerror = function (message, source, lineno, colno, error) {
  seen.push([arguments.length, this === window, message, source, lineno > 0, colno > 0, error && error.message].join());
  return message.includes('cancel');
};
addEventListener('error', (ev) => { if (ev.message.includes('prevent')) ev.preventDefault(); });
</script>
<script>throw new Error('script cancel');</script>
<script>
setTimeout(() => { throw new Error('timer reported'); }, 0);
queueMicrotask(() => { throw new Error('microtask prevent'); });
addEventListener('load', () => { throw new Error('load listener reported'); });
</script></head><body></body></html>"#;

#[test]
fn uncaught_exceptions_reach_window_onerror_and_cancel_the_report() {
    let mut thread = crate::browser_thread().unwrap();
    let hooks = Rc::new(Recorder::default());
    thread.browser.set_engine_hooks(Some(hooks.clone()));
    let page = thread.browser.create_page().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/", "text/html", PAGE, Duration::ZERO);
    thread.browser.set_loader(Some(loader));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    thread.run_until_idle(Some(Instant::now() + Duration::from_secs(3)));
    assert_eq!(
        eval(&thread, page, "seen.join('|')"),
        [
            "5,true,Uncaught Error: script cancel,http://site.test/,true,true,script cancel",
            "5,true,Uncaught Error: microtask prevent,http://site.test/,true,true,microtask prevent",
            "5,true,Uncaught Error: load listener reported,http://site.test/,true,true,load listener reported",
            "5,true,Uncaught Error: timer reported,http://site.test/,true,true,timer reported",
        ]
        .join("|")
    );
    // Only the events nobody canceled were reported to the engine hooks.
    assert_eq!(*hooks.exceptions.borrow(), ["Error: load listener reported", "Error: timer reported"]);
}

#[test]
fn isolated_world_exceptions_skip_the_page_handler() {
    let mut thread = crate::browser_thread().unwrap();
    let hooks = Rc::new(Recorder::default());
    thread.browser.set_engine_hooks(Some(hooks.clone()));
    let page = thread.browser.create_page().unwrap();
    eval(&thread, page, "globalThis.seen = []; window.onerror = (m) => { seen.push(m); return true; }; 'ok'");
    let isolated = thread.browser.isolated_world(page, "automation").unwrap();
    {
        let s = isolated.scope().unwrap();
        s.eval(
            "globalThis.isoSeen = []; addEventListener('error', (ev) => isoSeen.push(ev.message)); \
             setTimeout(() => { throw new Error('iso'); }, 0);",
            None,
            1,
        )
        .unwrap();
    }
    eval(&thread, page, "setTimeout(() => { throw new Error('main'); }, 0); 'ok'");
    thread.run_until_idle(Some(Instant::now() + Duration::from_secs(3)));
    assert_eq!(eval(&thread, page, "seen.join('|')"), "Uncaught Error: main");
    // Each world sees its own errors only (Chrome: a content script's
    // `error` listener never observes the page's exceptions).
    assert_eq!(
        isolated.scope().unwrap().eval("isoSeen.join('|')", None, 1).unwrap().to_string().unwrap(),
        "Uncaught Error: iso"
    );
    // The page's handler canceled its own error only.
    assert_eq!(*hooks.exceptions.borrow(), ["Error: iso"]);
}
