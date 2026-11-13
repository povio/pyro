//! Navigated XML documents (XHTML here): the parser stops at each script's
//! end tag until the script has run, so what follows a script, such as an
//! `<iframe onload>` calling a function the scripts define, does not load or
//! fire before them. Against real JavaScriptCore with the in-memory loader.

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
    fn on_page_event(&self, _page: PageId, event: &PageEvent) {
        if let PageEvent::FrameLoaded { .. } = event {
            self.log.borrow_mut().push("event:load".to_string());
        }
    }
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

/// Loads `page`, served as `mime` from `http://site.test/` next to `files`
/// (`(path, mime, body, delay ms)`), until it logs `main-load` (from its
/// `load` handler), and returns what the console and uncaught exceptions
/// logged, in order.
fn load(mime: &str, page: &str, files: &[(&str, &str, &str, u64)]) -> Vec<String> {
    let mut thread = crate::browser_thread().unwrap();
    let rec = Rc::new(Recorder::default());
    thread.browser.set_observer(Some(rec.clone()));
    thread.browser.set_engine_hooks(Some(rec.clone()));
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/", mime, page, Duration::ZERO);
    for (path, mime, body, delay) in files {
        loader.insert(&format!("http://site.test{path}"), mime, body, Duration::from_millis(*delay));
    }
    thread.browser.set_loader(Some(loader));
    let page = thread.browser.create_page().unwrap();
    rec.log.borrow_mut().clear();
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    // The main frame's `load` is the last one: it waits for its iframes.
    let r = thread.run_until(Some(Instant::now() + Duration::from_secs(10)), |_| {
        rec.log.borrow().iter().any(|l| l == "main-load")
    });
    assert_eq!(r, pyro_runtime::RunOutcome::Done, "log: {:?}", rec.log.borrow());
    thread.run_until_idle(Some(Instant::now() + Duration::from_millis(300)));
    // Every frame's `load` is recorded; the tests below tell them apart by
    // what the pages log themselves.
    let log = rec.log.borrow().iter().filter(|l| *l != "event:load").cloned().collect();
    log
}

const JS: &str = "text/javascript";

/// The shape of dom/nodes/Node-isEqualNode-xhtml.xhtml: external scripts in
/// the head, the script that defines the iframes' `onload` function in the
/// body, the iframes after it.
const PAGE: &str = r#"<html xmlns="http://www.w3.org/1999/xhtml">
<head>
<title>t</title>
<script src="/harness.js"/>
<script>
note('head: ' + [String(document.body), document.getElementsByTagName('iframe').length, typeof frameLoaded].join(' '));
</script>
</head>
<body>
<div id="log"/>
<script src="/define.js"/>
<script>
note('body: ' + [document.getElementsByTagName('iframe').length, String(document.getElementById('last'))].join(' '));
</script>
<iframe id="one" onload="frameLoaded(this)" src="/child.xml"/>
<iframe id="two" onload="frameLoaded(this)" src="/child.xml"/>
<p id="last"/>
<script>
note('end: ' + [document.getElementsByTagName('iframe').length, document.getElementById('last').localName].join(' '));
window.onload = () => {
  note('document: ' + [document.childNodes.length, document.getElementsByTagName('html').length,
                       document.compatMode, document.contentType].join(' '));
  note('main-load');
};
</script>
</body>
</html>"#;

/// Before the parser stopped at scripts, the whole tree was attached at the
/// end of input: the iframes loaded their (immediate) documents while the
/// page still waited for its first script, and `onload` threw a
/// ReferenceError, which ended a WPT page with no test (15 s timeout).
#[test]
fn an_iframe_after_the_scripts_loads_after_they_ran() {
    let log = load(
        "application/xhtml+xml",
        PAGE,
        &[
            ("/harness.js", JS, "function note(s) { console.log(s); }", 120),
            ("/define.js", JS, "function frameLoaded(f) { note('frame-loaded ' + f.id); }", 120),
            ("/child.xml", "application/xml", "<root><child/></root>", 0),
        ],
    );
    assert_eq!(
        log,
        [
            "head: null 0 undefined",
            "body: 0 null",
            "end: 2 p",
            "frame-loaded one",
            "frame-loaded two",
            "document: 1 1 CSS1Compat application/xhtml+xml",
            "main-load",
        ],
        "scripts run in order, each before the content after it exists"
    );
}

/// A script that removes its own parent: the content after the script still
/// goes into the removed element (as in Chrome), and the parser goes on.
#[test]
fn a_script_may_remove_the_element_the_parser_is_in() {
    let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title></head><body>
<div id="gone"><script>
window.gone = document.getElementById('gone');
gone.remove();
</script><p id="inside"/></div>
<p id="after"/>
<script>
console.log([gone.isConnected, gone.lastChild.id, String(document.getElementById('inside')),
             document.getElementById('after').localName].join(' '));
window.onload = () => console.log('main-load');
</script>
</body></html>"#;
    let log = load("application/xhtml+xml", page, &[]);
    assert_eq!(log, ["false inside null p", "main-load"]);
}

/// `customElements.define` in one script upgrades the elements above it at
/// once and those the parser inserts later before the next script runs.
#[test]
fn custom_elements_after_their_definition_are_upgraded_before_the_next_script() {
    let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title></head><body>
<x-a id="above"/>
<script>
customElements.define('x-a', class extends HTMLElement {
  connectedCallback() { console.log('connected ' + this.id); }
});
</script>
<x-a id="below"/>
<script>
console.log('script ' + (document.getElementById('below') instanceof customElements.get('x-a')));
window.onload = () => console.log('main-load');
</script>
<x-a id="last"/>
</body></html>"#;
    let log = load("application/xhtml+xml", page, &[]);
    assert_eq!(log, ["connected above", "connected below", "script true", "connected last", "main-load"]);
}

/// A resource that is not well-formed runs none of its scripts.
#[test]
fn a_malformed_document_runs_no_script() {
    let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><script>console.log('ran')</script></head>
<body><p></body></html>"#;
    let mut thread = crate::browser_thread().unwrap();
    let rec = Rc::new(Recorder::default());
    thread.browser.set_observer(Some(rec.clone()));
    thread.browser.set_engine_hooks(Some(rec.clone()));
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/", "application/xhtml+xml", page, Duration::ZERO);
    thread.browser.set_loader(Some(loader));
    let page = thread.browser.create_page().unwrap();
    rec.log.borrow_mut().clear();
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    let r = thread.run_until(Some(Instant::now() + Duration::from_secs(10)), |_| {
        rec.log.borrow().iter().any(|l| l == "event:load")
    });
    assert_eq!(r, pyro_runtime::RunOutcome::Done, "log: {:?}", rec.log.borrow());
    thread.run_until_idle(Some(Instant::now() + Duration::from_millis(300)));
    assert_eq!(*rec.log.borrow(), ["event:load"]);
}
