//! Streams across the realms of one origin: the internal slots are shared
//! (a same-origin frame's stream is a stream here) without page code ever
//! reaching them. Against real JavaScriptCore with the in-memory loader.

use pyro_browser::*;
use pyro_js::{ConsoleLevel, EngineHooks, JsException, Realm, Value};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

#[derive(Default)]
struct Recorder {
    log: RefCell<Vec<String>>,
}

impl PageObserver for Recorder {
    fn on_page_event(&self, _page: PageId, _event: &PageEvent) {}
}

impl EngineHooks for Recorder {
    fn console_api(&self, _realm: &Realm, _level: ConsoleLevel, args: &[Value<'_>]) {
        let parts: Vec<String> = args.iter().map(|a| a.to_string().unwrap_or_default()).collect();
        self.log.borrow_mut().push(parts.join(" "));
    }
    fn exception_thrown(&self, _realm: &Realm, e: &JsException) {
        self.log.borrow_mut().push(format!("exception: {}", e.text()));
    }
    fn unhandled_rejection(&self, _realm: &Realm, e: &JsException) {
        self.log.borrow_mut().push(format!("rejection: {}", e.text()));
    }
}

/// Loads `page` from `http://site.test/` and runs until a console line
/// starts with `done`; returns the console lines.
fn run(page: &str) -> Vec<String> {
    let mut thread = crate::browser_thread().unwrap();
    let rec = Rc::new(Recorder::default());
    thread.browser.set_observer(Some(rec.clone()));
    thread.browser.set_engine_hooks(Some(rec.clone()));
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/", "text/html", page, Duration::ZERO);
    thread.browser.set_loader(Some(loader));
    let page = thread.browser.create_page().unwrap();
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    let r = thread.run_until(Some(Instant::now() + Duration::from_secs(10)), |_| {
        rec.log.borrow().iter().any(|l| l.starts_with("done"))
    });
    assert_eq!(r, pyro_runtime::RunOutcome::Done, "log: {:?}", rec.log.borrow());
    let log = rec.log.borrow().clone();
    log
}

/// Page code that replaces `WeakMap.prototype`'s methods before Streams'
/// first use sees none of the slot maps, and cannot brand an object of its
/// own as a stream.
#[test]
fn page_code_never_reaches_the_slot_maps() {
    let log = run(r#"<!doctype html><script>
        const seen = [];
        for (const m of ['get', 'set', 'has']) {
          const original = WeakMap.prototype[m];
          WeakMap.prototype[m] = function (...args) { seen.push(this); return original.apply(this, args); };
        }
        const rs = new ReadableStream();
        rs.locked;
        rs.getReader().releaseLock();
        new WritableStream().getWriter();
        console.log('seen ' + seen.length);
        const fake = {};
        for (const map of seen) WeakMap.prototype.set.call(map, fake, { state: 'readable' });
        try {
          Object.getOwnPropertyDescriptor(ReadableStream.prototype, 'locked').get.call(fake);
          console.log('forged');
        } catch (e) {
          console.log('brand ' + e.constructor.name + ' ' + (e instanceof TypeError));
        }
        console.log('done');
        </script>"#);
    assert!(log.iter().any(|l| l == "seen 0"), "{log:?}");
    assert!(log.iter().any(|l| l == "brand TypeError true"), "{log:?}");
}

/// A same-origin frame's stream is a stream to a realm that never used its
/// own Streams globals: `new Response(stream)` reads it.
#[test]
fn a_same_origin_frames_stream_is_a_response_body_before_streams_are_used() {
    let log = run(r#"<!doctype html><iframe id=f srcdoc="<p>x"></iframe><script>
        onload = async () => {
          const w = f.contentWindow;
          const rs = new w.ReadableStream({ start(c) { c.enqueue(new w.TextEncoder().encode('hello')); c.close(); } });
          const response = new Response(rs);
          console.log('text ' + await response.text());
          console.log('done');
        };
        </script>"#);
    assert!(log.iter().any(|l| l == "text hello"), "{log:?}");
}
