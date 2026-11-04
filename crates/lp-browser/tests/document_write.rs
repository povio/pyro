//! `document.write` while the network parser loads the document, and external
//! scripts in written markup, against real JavaScriptCore.

use lp_browser::*;
use lp_js::{ConsoleLevel, EngineHooks, JsException, Realm, Value};
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
}

const IDS: &str = "[...document.querySelectorAll('[id]')].map(e => e.id).join(' ')";

/// Loads `html` at http://site.test/ with `files`, until `done` is logged.
fn load(html: &str, files: &[(&str, &str)]) -> Vec<String> {
    let mut thread = crate::browser_thread().unwrap();
    let rec = Rc::new(Recorder::default());
    thread.browser.set_observer(Some(rec.clone()));
    thread.browser.set_engine_hooks(Some(rec.clone()));
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/", "text/html", html, Duration::ZERO);
    for (path, body) in files {
        loader.insert(&format!("http://site.test{path}"), "text/javascript", body, Duration::from_millis(5));
    }
    thread.browser.set_loader(Some(loader.clone()));
    let page = thread.browser.create_page().unwrap();
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    let outcome = thread.run_until(Some(Instant::now() + Duration::from_secs(10)), |_| {
        rec.log.borrow().iter().any(|l| l.starts_with("done"))
    });
    assert_eq!(outcome, lp_runtime::RunOutcome::Done, "log: {:?}", rec.log.borrow());
    let log = rec.log.borrow().clone();
    log
}

#[test]
fn inline_write_parses_at_the_insertion_point_during_loading() {
    let log = load(
        &format!(
            r#"<!doctype html><body><p id=a></p><script id=outer>
            console.log(document.readyState, document.open() === document);
            document.write('<p id=b>t');
            document.write('ext</p><script>console.log("nested", !!document.getElementById("b"), document.currentScript.id);document.write("<p id=n></p>")<\/script>');
            console.log('after', !!document.getElementById('n'), document.currentScript.id);
            </script><p id=c></p><script>
            addEventListener('load', () => console.log('done', {IDS}));
            </script></body>"#
        ),
        &[],
    );
    assert_eq!(log, ["loading true", "nested true ", "after true outer", "done a outer b n c"], "{log:?}");
}

#[test]
fn written_external_script_blocks_like_a_parser_inserted_one() {
    let log = load(
        &format!(
            r#"<!doctype html><body><p id=a></p><script>
            document.write('<p id=b></p><script src=/one.js><\/script><p id=c></p>');
            console.log('sync', !!document.getElementById('b'), !!document.getElementById('c'), !!document.getElementById('one'));
            document.write('<p id=d></p>');
            console.log('sync2', !!document.getElementById('d'));
            </script><p id=e></p><script>
            console.log('later', typeof one);
            addEventListener('load', () => console.log('done', {IDS}));
            </script></body>"#
        ),
        &[
            // Its writes go ahead of the input queued by the outer script, and
            // it can chain another blocking script.
            ("/one.js", "var one = 1; console.log('one'); document.write('<p id=one></p><script src=/two.js><\\/script><p id=f></p>'); console.log('one-end', !!document.getElementById('one'), !!document.getElementById('two'));"),
            ("/two.js", "console.log('two'); document.write('<p id=two></p>');"),
        ],
    );
    assert_eq!(
        log,
        [
            "sync true false false",
            "sync2 false",
            "one",
            "one-end true false",
            "two",
            "later number",
            "done a b one two f c d e"
        ],
        "{log:?}"
    );
}

#[test]
fn async_and_deferred_external_writes_are_ignored() {
    let log = load(
        &format!(
            r#"<!doctype html><body><p id=a></p>
            <script src=/defer.js defer></script>
            <script src=/async.js async></script>
            <script>
            document.write('<script src=/written-defer.js defer><\/script><script src=/written-async.js async><\/script><p id=w></p>');
            console.log('sync', !!document.getElementById('w'));
            addEventListener('load', () => console.log('done', {IDS}, log.sort().join()));
            var log = [];
            </script></body>"#
        ),
        &[
            ("/defer.js", "log.push('defer'); document.write('<p id=x>defer</p>');"),
            ("/async.js", "document.write('<p id=y>async</p>');"),
            ("/written-defer.js", "log.push('written-defer'); document.write('<p id=z>w</p>');"),
            ("/written-async.js", "log.push('written-async'); document.write('<p id=v>w</p>');"),
        ],
    );
    assert_eq!(log[0], "sync true", "{log:?}");
    assert_eq!(log.last().unwrap(), "done a w defer,written-async,written-defer", "{log:?}");
    assert!(!log.iter().any(|l| l.starts_with("exception")), "{log:?}");
}

#[test]
fn write_from_a_timer_while_loading_is_reported_and_keeps_the_load() {
    let log = load(
        &format!(
            r#"<!doctype html><body><p id=a></p><script>
            setTimeout(() => {{ try {{ document.write('x'); console.log('wrote') }} catch (e) {{ console.log(e.name) }} }});
            </script><script src=/slow.js></script><p id=b></p><script>
            addEventListener('load', () => console.log('done', {IDS}));
            </script></body>"#
        ),
        &[("/slow.js", "console.log('slow');")],
    );
    assert_eq!(log, ["NotSupportedError", "slow", "done a b"], "{log:?}");
}

#[test]
fn external_script_in_markup_written_to_a_loaded_document() {
    let log = load(
        &format!(
            r#"<!doctype html><body><script>
            addEventListener('load', () => {{
                if (window.reopened) return;
                window.reopened = true;
                document.open();
                addEventListener('load', () => console.log('done', {IDS}, document.readyState));
                document.write('<p id=a></p><script src=/one.js><\/script><p id=b></p>');
                document.write('<p id=c></p>');
                document.close();
                console.log('closed', !!document.getElementById('b'), document.readyState);
            }});
            </script></body>"#
        ),
        &[("/one.js", "console.log('one'); document.write('<p id=one></p>');")],
    );
    assert_eq!(log, ["closed false loading", "one", "done a one b c complete"], "{log:?}");
}
