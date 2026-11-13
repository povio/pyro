//! Scripts the parser did not insert: `<script>` elements created by script
//! (`appendChild`) during a deferred script, a `DOMContentLoaded` handler or
//! the `load` handler, plus script element `load`/`error` events. Against
//! real JavaScriptCore with the in-memory loader.

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
        let e = match event {
            PageEvent::FrameDomContentLoaded { .. } => "event:dcl".to_string(),
            PageEvent::FrameLoaded { .. } => "event:load".to_string(),
            _ => return,
        };
        self.log.borrow_mut().push(e);
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

/// Loads `page` (and `files`, `(path, body, delay ms)` JavaScript) from
/// `http://site.test/` and runs until the `load` event plus a settle period.
fn load(page: &str, files: &[(&str, &str, u64)]) -> Vec<String> {
    let mut thread = crate::browser_thread().unwrap();
    let rec = Rc::new(Recorder::default());
    thread.browser.set_observer(Some(rec.clone()));
    thread.browser.set_engine_hooks(Some(rec.clone()));
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/", "text/html", page, Duration::ZERO);
    for (path, body, delay) in files {
        loader.insert(&format!("http://site.test{path}"), "text/javascript", body, Duration::from_millis(*delay));
    }
    thread.browser.set_loader(Some(loader));
    let page = thread.browser.create_page().unwrap();
    rec.log.borrow_mut().clear();
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    let r = thread.run_until(Some(Instant::now() + Duration::from_secs(10)), |_| {
        rec.log.borrow().iter().any(|l| l == "event:load")
    });
    assert_eq!(r, pyro_runtime::RunOutcome::Done, "log: {:?}", rec.log.borrow());
    thread.run_until_idle(Some(Instant::now() + Duration::from_millis(300)));
    let log = rec.log.borrow().clone();
    log
}

fn pos(log: &[String], s: &str) -> usize {
    log.iter().position(|l| l == s).unwrap_or_else(|| panic!("missing {s:?} in {log:?}"))
}

fn assert_before(log: &[String], pairs: &[(&str, &str)]) {
    for (a, b) in pairs {
        assert!(pos(log, a) < pos(log, b), "{a:?} should precede {b:?}: {log:?}");
    }
}

const DEFER_PAGE: &str = r#"<!doctype html><html><head>
<script>
function note(s) { console.log(s); }
function deferMain() {
  note('defer-start');
  var s = document.createElement('script');
  s.src = '/second.js';
  s.onload = () => note('second-load');
  s.onerror = () => note('second-error');
  document.head.appendChild(s);
  var i = window.inserted = document.createElement('script');
  i.textContent = "note('inline-inserted'); note(document.currentScript === inserted ? 'current-ok' : 'current-bad')";
  document.head.appendChild(i);
  note('defer-end');
}
document.addEventListener('DOMContentLoaded', () => {
  note('dcl');
  var s = document.createElement('script');
  s.src = '/in-dcl.js';
  document.head.appendChild(s);
});
window.addEventListener('load', () => {
  note('load');
  var s = document.createElement('script');
  s.src = '/in-load.js';
  s.onload = () => note('in-load-load');
  document.head.appendChild(s);
});
</script>
<script defer src="/defer.js"></script>
</head><body></body></html>"#;

/// The bug behind xhr/xmlhttprequest-sync-not-hang-scriptloader.html: an
/// external script appended while a deferred script runs must execute, as
/// must ones appended from DOMContentLoaded and load handlers. Scripts
/// started before `load` delay it.
#[test]
fn scripts_inserted_during_defer_dcl_and_load_run() {
    let log = load(
        DEFER_PAGE,
        &[
            ("/defer.js", "deferMain()", 0),
            ("/second.js", "note('second-ran')", 30),
            ("/in-dcl.js", "note('in-dcl-ran')", 30),
            ("/in-load.js", "note('in-load-ran')", 0),
        ],
    );
    assert_before(
        &log,
        &[
            ("defer-start", "defer-end"),
            // The inline script runs inside `appendChild` (a post-connection
            // step), before the inserting script goes on.
            ("inline-inserted", "defer-end"),
            ("inline-inserted", "current-ok"),
            ("inline-inserted", "event:dcl"),
            ("defer-end", "second-ran"),
            ("second-ran", "second-load"),
            ("second-ran", "event:load"),
            ("event:dcl", "in-dcl-ran"),
            ("in-dcl-ran", "event:load"),
            ("in-dcl-ran", "load"),
            ("load", "in-load-ran"),
            ("in-load-ran", "in-load-load"),
        ],
    );
    assert!(!log.iter().any(|l| l == "second-error" || l == "current-bad"), "{log:?}");
    assert!(!log.iter().any(|l| l.starts_with("exception") || l.starts_with("rejection")), "{log:?}");
}

const EVENTS_PAGE: &str = r#"<!doctype html><html><head>
<script>
function note(s) { console.log(s); }
function add(el) { document.head.appendChild(el); return el; }
var missing = document.createElement('script');
missing.src = '/missing.js';
missing.onload = () => note('missing-load');
missing.onerror = () => note('missing-error');
add(missing);
var empty = document.createElement('script');
empty.setAttribute('src', '');
empty.onerror = () => note('empty-error');
add(empty);
var once = document.createElement('script');
once.textContent = "note('once')";
add(once);
add(once); // moving an already-started script does not run it again
var fragment = document.createElement('div');
fragment.innerHTML = "<script>note('fragment-ran')<\/script>";
add(fragment);
var data = document.createElement('script');
data.type = 'text/template';
data.textContent = "note('data-block-ran')";
add(data);
var nomodule = document.createElement('script');
nomodule.setAttribute('nomodule', '');
nomodule.textContent = "note('nomodule-ran')";
add(nomodule);
var later = document.createElement('script');
later.textContent = "note('text-only-not-run')";
var inlineModule = document.createElement('script');
inlineModule.type = 'module';
inlineModule.textContent = "note('inline-module-ran')";
inlineModule.onload = () => note('inline-module-load');
add(inlineModule);
var extModule = document.createElement('script');
extModule.type = 'module';
extModule.src = '/mod.js';
extModule.onload = () => note('ext-module-load');
add(extModule);
var deferred = document.createElement('script');
deferred.src = '/forced-async.js';
deferred.defer = true; // script-inserted: force async, runs when fetched
deferred.onload = () => note('forced-async-load');
add(deferred);
var srcLater = add(document.createElement('script'));
srcLater.onload = () => note('src-later-load');
srcLater.src = '/src-later.js'; // src set while connected prepares it
</script>
<script src="/blocking.js" onload="note('blocking-load')" onerror="note('blocking-error')"></script>
<script async src="/async.js" onload="note('async-load')"></script>
<script defer src="/defer.js" onload="note('defer-load')"></script>
<script src="/missing.js" onload="note('parser-missing-load')" onerror="note('parser-missing-error')"></script>
</head><body></body></html>"#;

/// `load`/`error` events on script elements (script-inserted and
/// parser-inserted), and the "prepare the script element" exits: already
/// started, fragment-parsed, data blocks, `nomodule`, empty `src`.
#[test]
fn script_elements_fire_load_and_error_events() {
    let log = load(
        EVENTS_PAGE,
        &[
            ("/mod.js", "note('ext-module-ran')", 0),
            ("/forced-async.js", "note('forced-async-ran')", 0),
            ("/src-later.js", "note('src-later-ran')", 0),
            ("/blocking.js", "note('blocking')", 0),
            ("/async.js", "note('async')", 0),
            ("/defer.js", "note('defer')", 0),
        ],
    );
    for present in [
        "missing-error",
        "empty-error",
        "once",
        "inline-module-ran",
        "inline-module-load",
        "ext-module-ran",
        "ext-module-load",
        "forced-async-ran",
        "forced-async-load",
        "src-later-ran",
        "src-later-load",
        "blocking-load",
        "async-load",
        "defer-load",
        "parser-missing-error",
    ] {
        assert!(log.iter().any(|l| l == present), "missing {present:?} in {log:?}");
    }
    for absent in [
        "missing-load",
        "fragment-ran",
        "data-block-ran",
        "nomodule-ran",
        "text-only-not-run",
        "blocking-error",
        "parser-missing-load",
    ] {
        assert!(!log.iter().any(|l| l == absent), "unexpected {absent:?} in {log:?}");
    }
    assert_eq!(log.iter().filter(|l| *l == "once").count(), 1, "{log:?}");
    assert_before(
        &log,
        &[
            ("inline-module-ran", "inline-module-load"),
            ("ext-module-ran", "ext-module-load"),
            ("forced-async-ran", "forced-async-load"),
            ("blocking", "blocking-load"),
            ("async", "async-load"),
            ("defer", "defer-load"),
            ("defer-load", "event:dcl"),
            // Scripts inserted before load delay it.
            ("forced-async-ran", "event:load"),
            ("ext-module-load", "event:load"),
        ],
    );
}

const WRITE_PAGE: &str = r#"<!doctype html><html><head>
<script>
function note(s) { console.log(s); }
var s = document.createElement('script');
s.text = "document.write('<p id=w>w</p>'); note('inserted-ran:' + (document.getElementById('d') ? 'd-seen' : 'd-unseen'))";
document.head.appendChild(s);
note('outer-end');
</script>
</head><body><div id="d"></div>
<script>
var w = document.getElementById('w'), d = document.getElementById('d');
note('w:' + (w ? w.parentNode.tagName : 'none'));
note('order:' + (w && d && (w.compareDocumentPosition(d) & 4) ? 'ok' : 'bad'));
</script>
</body></html>"#;

/// An inline script inserted by a script the parser is running runs inside
/// the inserting call, and its `document.write` parses at the parser's
/// position instead of blowing the document away (implicit `open`).
#[test]
fn inserted_inline_script_writes_at_parser_insertion_point() {
    let log = load(WRITE_PAGE, &[]);
    assert_before(&log, &[("inserted-ran:d-unseen", "outer-end"), ("outer-end", "w:BODY"), ("w:BODY", "order:ok")]);
    assert!(log.iter().any(|l| l == "event:load"), "{log:?}");
    assert!(!log.iter().any(|l| l.starts_with("exception")), "{log:?}");
}

const MODULES_PAGE: &str = r#"<!doctype html><html><head>
<script type="module">console.log('parser-module');</script>
<script>
function note(s) { console.log(s); }
function addModule(name) {
  var m = document.createElement('script');
  m.type = 'module';
  m.textContent = "console.log('" + name + "-ran')";
  m.onload = () => note(name + '-load');
  document.head.appendChild(m);
}
addModule('m1');
addModule('m2');
document.addEventListener('DOMContentLoaded', () => addModule('m3'));
</script>
</head><body></body></html>"#;

/// Every inline module inserted by script evaluates once: their registry
/// keys never repeat (JSC evaluates a URL only once), including against the
/// parser's inline modules.
#[test]
fn inline_modules_inserted_by_script_each_run() {
    let log = load(MODULES_PAGE, &[]);
    for name in ["parser-module", "m1-ran", "m2-ran", "m3-ran", "m1-load", "m2-load", "m3-load"] {
        assert_eq!(log.iter().filter(|l| *l == name).count(), 1, "{name:?} in {log:?}");
    }
    assert_before(&log, &[("m1-ran", "m1-load"), ("m3-ran", "event:load")]);
    assert!(!log.iter().any(|l| l.starts_with("exception") || l.starts_with("rejection")), "{log:?}");
}

/// `Range.createContextualFragment` scripts run once the fragment is
/// inserted (postscribe-style injectors); `innerHTML`'s never do.
#[test]
fn contextual_fragment_scripts_run_when_inserted() {
    let log = load(
        r#"<!doctype html><body><div id=host></div><script>
        const range = document.createRange();
        range.selectNode(document.body);
        const frag = range.createContextualFragment(
            '<p id=injected>x</p><script>console.log("fragment-ran", !!document.getElementById("injected"))<\/script>');
        console.log('parsed');
        document.getElementById('host').innerHTML = '<script>console.log("inner-html-ran")<\/script>';
        document.body.appendChild(frag);
        console.log('appended');
        </script></body>"#,
        &[],
    );
    assert_before(&log, &[("parsed", "fragment-ran true"), ("fragment-ran true", "appended")]);
    assert!(!log.iter().any(|l| l == "inner-html-ran"), "{log:?}");
    assert!(!log.iter().any(|l| l.starts_with("exception")), "{log:?}");
}
