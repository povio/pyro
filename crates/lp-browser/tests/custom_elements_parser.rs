//! The main document parser and custom elements, against real
//! JavaScriptCore: elements whose name is defined before their markup are
//! constructed synchronously (HTML "create an element for a token"), get
//! their attributes after the constructor, and are connected before the
//! parser goes on.

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

/// Loads `html` at http://site.test/ (and `frame` at /frame), until `done`
/// is logged.
fn load(html: &str, frame: Option<&str>) -> Vec<String> {
    let mut thread = crate::browser_thread().unwrap();
    let rec = Rc::new(Recorder::default());
    thread.browser.set_observer(Some(rec.clone()));
    thread.browser.set_engine_hooks(Some(rec.clone()));
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/", "text/html", html, Duration::ZERO);
    if let Some(frame) = frame {
        loader.insert("http://site.test/frame", "text/html", frame, Duration::ZERO);
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
fn defined_elements_are_constructed_by_the_parser_before_their_children() {
    let log = load(
        r#"<!doctype html><head><script>
        class XEl extends HTMLElement {
          static observedAttributes = ['title'];
          constructor() {
            super();
            const c = document.getElementById('c');
            console.log('ctor', this.attributes.length, this.childNodes.length, this.isConnected, c ? c.childElementCount : -1);
          }
          attributeChangedCallback(n, o, v) { console.log('attr', n, o, v, this.isConnected); }
          connectedCallback() { console.log('connected', this.parentNode.id, this.childNodes.length); }
        }
        customElements.define('x-el', XEl);
        </script></head><body><div id=c><span></span><x-el title=t other=o><b>child</b></x-el><i></i></div>
        <script>
        const el = document.querySelector('x-el');
        console.log('script', el instanceof XEl, el.matches(':defined'), el.getAttribute('title'), el.firstChild.localName);
        console.log('done');
        </script></body>"#,
        None,
    );
    assert_eq!(
        log,
        ["ctor 0 0 false 1", "attr title null t false", "connected c 0", "script true true t b", "done"],
        "{log:?}"
    );
}

#[test]
fn constructors_cannot_write_markup_and_failures_leave_unknown_elements() {
    let log = load(
        r#"<!doctype html><head><script>
        customElements.define('x-writer', class extends HTMLElement {
          constructor() {
            super();
            try { document.write('<p id=written></p>'); console.log('wrote'); } catch (e) { console.log('write', e.name); }
          }
        });
        customElements.define('x-thrower', class extends HTMLElement {
          constructor() { super(); throw new Error('boom'); }
        });
        customElements.define('x-impostor', class extends HTMLElement {
          constructor() { super(); return document.createElement('div'); }
        });
        </script></head><body><x-writer></x-writer><x-thrower id=t></x-thrower><x-impostor id=i></x-impostor>
        <script>
        console.log('state', !!document.getElementById('written'), document.getElementById('t').matches(':defined'),
          document.getElementById('i').localName, document.getElementById('i').matches(':defined'));
        console.log('done');
        </script></body>"#,
        None,
    );
    assert_eq!(log.len(), 5, "{log:?}");
    assert_eq!(log[0], "write InvalidStateError");
    assert!(log[1].starts_with("exception:") && log[1].contains("boom"), "{log:?}");
    assert!(log[2].starts_with("exception:") && log[2].contains("NotSupportedError"), "{log:?}");
    assert_eq!(log[3..], ["state false false x-impostor false", "done"]);
}

#[test]
fn constructors_in_a_child_frame_can_reach_the_frame_tree() {
    let log = load(
        r#"<!doctype html><body><script>customElements.define("x-main", class extends HTMLElement {});</script><iframe src=/frame></iframe></body>"#,
        Some(
            r#"<!doctype html><head><script>
            customElements.define('x-frame', class extends HTMLElement {
              constructor() { super(); this.seen = [parent.frames.length, parent.document.querySelectorAll('iframe').length].join(); }
            });
            </script></head><body><x-frame></x-frame><script>
            console.log('frame', document.querySelector('x-frame').seen);
            console.log('done');
            </script></body>"#,
        ),
    );
    assert_eq!(log, ["frame 1,1", "done"], "{log:?}");
}

#[test]
fn template_contents_stay_inert_and_declarative_shadow_roots_construct() {
    let log = load(
        r#"<!doctype html><head><script>
        var made = 0;
        customElements.define('x-count', class extends HTMLElement { constructor() { super(); made++; } });
        </script></head><body>
        <template id=t><x-count></x-count></template>
        <div id=host><template shadowrootmode=open><x-count></x-count></template></div>
        <x-count></x-count>
        <script>
        const inert = document.getElementById('t').content.firstChild;
        const shadow = document.getElementById('host').shadowRoot.firstChild;
        console.log('made', made, inert.matches(':defined'), shadow.matches(':defined'));
        console.log('done');
        </script></body>"#,
        None,
    );
    assert_eq!(log, ["made 2 false true", "done"], "{log:?}");
}
