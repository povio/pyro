//! `window.open` popups: the opener's stand-in, `window.opener`, session
//! storage copies, named reuse, `window.close()` and `BroadcastChannel`.
//!
//! The embedder (pyro-cdp) creates the popup page from `PopupRequested` and
//! hands it to `Browser::adopt_popup`; these tests play that part.
use pyro_browser::{BrowserThread, NavigateOpts, PageEvent, PageId, PageObserver, StaticLoader, DEFAULT_CONTEXT};
use pyro_webapi::PopupTicket;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

fn eval(t: &BrowserThread, page: PageId, source: &str) -> String {
    let s = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().scope().unwrap();
    s.eval(source, Some("popups-test.js"), 1)
        .unwrap_or_else(|e| panic!("{source}: {}", e.details(&s).text()))
        .to_string()
        .unwrap()
}
fn settle(t: &mut BrowserThread) {
    assert_eq!(t.run_until_idle(Some(Instant::now() + Duration::from_secs(3))), pyro_runtime::RunOutcome::Idle);
}

#[derive(Default)]
struct Events(RefCell<Vec<(PageId, PageEvent)>>);
impl PageObserver for Events {
    fn on_page_event(&self, page: PageId, event: &PageEvent) {
        self.0.borrow_mut().push((page, event.clone()));
    }
}
impl Events {
    /// (opener page, url, keeps opener, ticket) of every popup requested so far.
    fn popups(&self) -> Vec<(PageId, String, bool, Option<Rc<PopupTicket>>)> {
        self.0
            .borrow()
            .iter()
            .filter_map(|(page, e)| match e {
                PageEvent::PopupRequested { url, opener, ticket, .. } => {
                    Some((*page, url.clone(), *opener, ticket.clone()))
                }
                _ => None,
            })
            .collect()
    }
    fn close_requests(&self) -> Vec<PageId> {
        self.0
            .borrow()
            .iter()
            .filter_map(|(page, e)| matches!(e, PageEvent::CloseRequested { .. }).then_some(*page))
            .collect()
    }
}

fn setup() -> (BrowserThread, Rc<Events>) {
    let mut t = crate::browser_thread().unwrap();
    let loader = Rc::new(StaticLoader::default());
    for url in
        ["http://app.test/a", "http://app.test/b", "http://app.test/c", "http://other.test/x", "http://other.test/y"]
    {
        loader.insert(url, "text/html", "<p>page</p>", Duration::ZERO);
    }
    loader.insert(
        "http://other.test/framed",
        "text/html",
        "<iframe name=kid src='http://app.test/c'></iframe><iframe src='/y'></iframe>",
        Duration::ZERO,
    );
    t.browser.set_loader(Some(loader));
    let events = Rc::new(Events::default());
    t.browser.set_observer(Some(events.clone()));
    (t, events)
}

fn open_page(t: &mut BrowserThread, url: &str) -> PageId {
    let page = t.browser.create_page().unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar(url)).unwrap();
    settle(t);
    page
}

/// Plays the embedder for the last popup request: a new page in the
/// opener's context adopted with the request's ticket and navigated to its URL.
fn create_popup(t: &mut BrowserThread, events: &Events) -> PageId {
    let (opener, url, _, ticket) = events.popups().pop().expect("a popup was requested");
    let page = t.browser.create_page_in(DEFAULT_CONTEXT).unwrap();
    t.browser.adopt_popup(page, opener, ticket);
    t.browser.navigate(page, NavigateOpts::address_bar(&url)).unwrap();
    settle(t);
    page
}

#[test]
fn popup_keeps_its_opener_and_a_copy_of_the_session_storage() {
    let (mut t, events) = setup();
    let a = open_page(&mut t, "http://app.test/a");
    assert_eq!(
        eval(
            &t,
            a,
            "sessionStorage.setItem('FOO','BAR'); localStorage.setItem('L','1');
             globalThis.messages=[]; addEventListener('message', e => messages.push(JSON.stringify(e.data)));
             globalThis.storageEvents=0; addEventListener('storage', () => storageEvents++);
             globalThis.p = window.open('b', 'popname');
             sessionStorage.setItem('BAZ','QUX');
             [p.closed, typeof p.fetch, typeof p.document, p.name, p.window === p, p.opener === window].join()"
        ),
        "false,function,object,popname,true,true",
        "the stand-in reaches the popup's initial about:blank document at once"
    );
    settle(&mut t);
    let requests = events.popups();
    assert_eq!(requests.len(), 1);
    assert_eq!((requests[0].0, requests[0].1.as_str(), requests[0].2), (a, "http://app.test/b", true));
    assert!(requests[0].3.is_some(), "window.open hands the embedder a ticket");
    let b = create_popup(&mut t, &events);
    assert_eq!(t.browser.popup_opener(b), Some(a));
    assert_eq!(
        eval(
            &t,
            b,
            "[sessionStorage.getItem('FOO'), sessionStorage.getItem('BAZ'), localStorage.getItem('L'),
              window.opener === window.opener, window.opener.sessionStorage.getItem('BAZ'),
              window.opener.document.URL].join()"
        ),
        "BAR,,1,true,QUX,http://app.test/a",
        "session storage is copied at window.open; local storage and the opener window are shared"
    );
    eval(
        &t,
        b,
        "sessionStorage.setItem('FOO','NEW'); document.title='pop'; window.opener.postMessage({done:true}, '*')",
    );
    settle(&mut t);
    assert_eq!(
        eval(&t, a, "[sessionStorage.getItem('FOO'), storageEvents, messages.join('|')].join()"),
        "BAR,0,{\"done\":true}",
        "the popup's session storage diverged; its changes fire no storage event in the opener"
    );
    assert_eq!(
        eval(&t, a, "[p.sessionStorage.getItem('FOO'), p.document.title, p.location.href, p.length].join()"),
        "NEW,pop,http://app.test/b,0",
        "the stand-in reaches the popup's real window once it exists"
    );
    // Named reuse: the same object, no new page, navigation only with a URL.
    assert_eq!(eval(&t, a, "window.open('', 'popname') === p"), "true");
    settle(&mut t);
    assert_eq!(events.popups().len(), 1, "reopening a named popup requests no new page");
    assert_eq!(eval(&t, a, "window.open('b', 'other') === p"), "false");
    settle(&mut t);
    assert_eq!(events.popups().len(), 2);
    // Messages to the popup are cloned and delivered as a task.
    eval(
        &t,
        b,
        "globalThis.got=[]; addEventListener('message', e => got.push([e.origin, e.data.hello, e.source === opener].join(':')))",
    );
    assert_eq!(eval(&t, a, "const o={hello:1}; p.postMessage(o, '*'); o.hello=2; p.got.length"), "0");
    settle(&mut t);
    // The source is the window that posted (HTML: the incumbent's WindowProxy).
    assert_eq!(eval(&t, b, "got.join('|')"), "http://app.test:1:true");
    assert_eq!(
        eval(
            &t,
            a,
            "p.postMessage({}, 'http://other.test'); try { p.postMessage(() => 1, '*'); 'no' } catch (e) { e.name }"
        ),
        "DataCloneError"
    );
    settle(&mut t);
    assert_eq!(eval(&t, b, "got.length"), "1", "a mismatched target origin drops the message");
    // window.close() in a popup asks the embedder to close it; a dropped
    // ticket reads as closed in the opener.
    eval(&t, b, "window.close()");
    settle(&mut t);
    assert_eq!(events.close_requests(), vec![b]);
    eval(&t, a, "window.close()");
    settle(&mut t);
    assert_eq!(events.close_requests(), vec![b], "a page that is not a popup is not script-closable");
    // The ticket is held by the embedder's record of the page (here: the
    // recorded request); the page closing drops the last one.
    t.browser.close_page(b);
    drop(requests);
    events.0.borrow_mut().clear();
    assert_eq!(eval(&t, a, "p.closed"), "true");
}

#[test]
fn stand_in_storage_before_the_page_exists_is_the_popups_area() {
    let (mut t, events) = setup();
    let a = open_page(&mut t, "http://app.test/a");
    assert_eq!(
        eval(
            &t,
            a,
            "globalThis.p = window.open('', 'w'); sessionStorage.setItem('FOO','BAR');
             p.sessionStorage.setItem('K','V');
             [p.sessionStorage.getItem('FOO'), sessionStorage.getItem('FOO'), p.localStorage === localStorage,
              p.sessionStorage === sessionStorage, p.sessionStorage.getItem('K'), sessionStorage.getItem('K'),
              p.fetch('b') instanceof Promise].join()"
        ),
        // The popup's own window answers (its initial document exists at
        // once): its Storage objects and Promise are its own.
        ",BAR,false,false,V,,false"
    );
    settle(&mut t);
    let b = create_popup(&mut t, &events);
    assert_eq!(
        eval(&t, b, "[location.href, sessionStorage.getItem('K'), sessionStorage.getItem('FOO'), origin].join()"),
        "about:blank,V,,http://app.test",
        "an about:blank popup takes the opener's origin and the area the stand-in wrote to"
    );
}

#[test]
fn noopener_and_cross_origin_popups() {
    let (mut t, events) = setup();
    let a = open_page(&mut t, "http://app.test/a");
    assert_eq!(
        eval(&t, a, "sessionStorage.setItem('FOO','BAR'); String(window.open('b', '_blank', 'noopener'))"),
        "null"
    );
    settle(&mut t);
    let (_, url, opener, ticket) = events.popups().pop().unwrap();
    assert_eq!((url.as_str(), opener, ticket.is_some()), ("http://app.test/b", false, false));
    let b = create_popup(&mut t, &events);
    assert_eq!(
        eval(&t, b, "[String(window.opener), sessionStorage.getItem('FOO')].join()"),
        "null,",
        "a noopener popup has no opener and fresh session storage"
    );
    assert_eq!(eval(&t, a, "globalThis.p = window.open('http://other.test/x'); p.closed"), "false");
    settle(&mut t);
    let x = create_popup(&mut t, &events);
    assert_eq!(
        eval(
            &t,
            x,
            "[window.opener !== null, window.opener.closed, window.opener.top === window.opener,
              (() => { try { window.opener.sessionStorage; return 'no' } catch (e) { return e.name } })()].join()"
        ),
        "true,false,true,SecurityError",
        "a cross-origin opener is a stand-in"
    );
    assert_eq!(
        eval(
            &t,
            a,
            "[p.closed, (() => { try { p.sessionStorage; return 'no' } catch (e) { return e.name } })(),
              (() => { try { p.location.href; return 'no' } catch (e) { return e.name } })(),
              (() => { try { p.document; return 'no' } catch (e) { return e.name } })()].join()"
        ),
        "false,SecurityError,SecurityError,SecurityError"
    );
}

/// The opener's popup is a WindowProxy: same origin, every property
/// operation goes to the popup's window and its functions run with that
/// window as `this`; cross origin, only CrossOriginProperties are there and
/// writes, definitions and deletions throw SecurityError without changing
/// anything.
#[test]
fn a_popup_is_a_window_proxy() {
    let (mut t, events) = setup();
    let a = open_page(&mut t, "http://app.test/a");
    eval(&t, a, "globalThis.p = window.open('b')");
    settle(&mut t);
    let b = create_popup(&mut t, &events);
    let try_ = "var t = (f) => { try { return String(f()) } catch (e) { return e.name } };";
    assert_eq!(
        eval(
            &t,
            a,
            &format!(
                "{try_} [
                  t(() => {{ let hit = 0; p.addEventListener('x', () => hit++); p.dispatchEvent(new p.Event('x')); return hit }}),
                  t(() => typeof p.setTimeout(() => {{}}, 1)),
                  t(() => {{ Object.defineProperty(p, 'defined', {{ value: 1, configurable: true }}); return p.defined }}),
                  t(() => {{ p.gone = 1; delete p.gone; return 'gone' in p }}),
                  t(() => Object.getOwnPropertyDescriptor(p, 'defined').value),
                  t(() => Reflect.ownKeys(p).includes('defined')),
                  t(() => Reflect.defineProperty(p, 'fixed', {{ value: 1, configurable: false }})),
                  t(() => p.window === p && p.self === p && p.top === p),
                ].join()"
            )
        ),
        "1,number,1,false,1,true,false,true"
    );
    assert_eq!(eval(&t, b, "[typeof defined, typeof gone].join()"), "number,undefined", "on the popup's window");
    // Cross origin.
    eval(&t, a, "globalThis.q = window.open('http://other.test/x')");
    settle(&mut t);
    let x = create_popup(&mut t, &events);
    assert_eq!(
        eval(
            &t,
            a,
            &format!(
                "{try_} [
                  t(() => q.closed), t(() => q.window === q), t(() => q.length), t(() => q.opener === window),
                  t(() => typeof q.postMessage), t(() => typeof q.then),
                  t(() => q.document),
                  t(() => {{ q.postMessage = 1 }}), t(() => typeof q.postMessage),
                  t(() => {{ q.opener = null }}), t(() => q.opener === window),
                  t(() => Object.defineProperty(q, 'x', {{ value: 1 }})),
                  t(() => delete q.close),
                  t(() => {{ q[0] = 1 }}),
                  t(() => Object.getOwnPropertyDescriptor(q, 'document')),
                  t(() => Object.getPrototypeOf(q)),
                  t(() => Object.getOwnPropertyDescriptor(q, 'location').set !== undefined),
                ].join()"
            )
        ),
        "false,true,0,true,function,undefined,SecurityError,SecurityError,function,SecurityError,true,\
         SecurityError,SecurityError,SecurityError,SecurityError,null,true"
    );
    assert_eq!(eval(&t, x, "window.opener !== null"), "true", "a refused `opener = null` changed nothing");
}

/// A cross-origin popup's WindowProxy, as HTML's CrossOriginGetOwnPropertyHelper has it: its child
/// windows by index and by name, its keys listed without page-observable hooks, a cross-origin
/// `location` with only `href` (set) and `replace()` and a null prototype, and the functions handed
/// out kept per origin pair (none handed out same origin is handed out cross origin).
#[test]
fn a_cross_origin_popup_proxy_has_its_frames_and_a_cross_origin_location() {
    let (mut t, events) = setup();
    let a = open_page(&mut t, "http://app.test/a");
    eval(&t, a, "globalThis.q = window.open('http://app.test/b')");
    settle(&mut t);
    let b = create_popup(&mut t, &events);
    // Same origin first: what it hands out.
    eval(&t, a, "globalThis.sameClose = q.close; globalThis.samePost = q.postMessage");
    // The popup goes cross origin, to a document with two child frames.
    eval(&t, b, "location.href = 'http://other.test/framed'");
    settle(&mut t);
    let try_ = "var t = (f) => { try { return String(f()) } catch (e) { return e.name } };";
    assert_eq!(
        eval(
            &t,
            a,
            &format!(
                "{try_} [
                  t(() => q.length), t(() => q[0] === q.kid), t(() => q.kid.location.href), t(() => typeof q[1]),
                  t(() => q[1].document), t(() => q[2]), t(() => q.nope),
                  t(() => Object.getOwnPropertyDescriptor(q, '0').configurable),
                  t(() => Reflect.ownKeys(q).slice(0, 2).join('/')),
                  t(() => {{ Object.defineProperty(Array.prototype, Symbol.isConcatSpreadable, {{ get() {{ throw 1 }}, configurable: true }});
                            Array.prototype.constructor = {{ [Symbol.species]: function () {{ throw 2 }} }};
                            try {{ return Reflect.ownKeys(q).length }} finally {{ delete Array.prototype[Symbol.isConcatSpreadable]; Array.prototype.constructor = Array }} }}),
                ].join()"
            )
        ),
        "2,true,http://app.test/c,object,SecurityError,SecurityError,SecurityError,true,0/1,19"
    );
    assert_eq!(
        eval(
            &t,
            a,
            &format!(
                "{try_} const l = q.location; [
                  t(() => Object.getPrototypeOf(l)), t(() => l.href), t(() => typeof l.replace), t(() => l.assign),
                  t(() => l.toString()), t(() => Reflect.ownKeys(l).length), t(() => l === q.location),
                  t(() => Object.getOwnPropertyDescriptor(l, 'href').get), t(() => {{ l.hash = 'x' }}),
                  t(() => q.close === q.close), t(() => q.close !== sameClose && q.postMessage !== samePost),
                  t(() => Object.getOwnPropertyDescriptor(q, 'closed').get === Object.getOwnPropertyDescriptor(q, 'closed').get),
                ].join()"
            )
        ),
        "null,SecurityError,function,SecurityError,SecurityError,6,true,undefined,SecurityError,true,true,true"
    );
}

#[test]
fn broadcast_channel_reaches_same_origin_documents_of_the_browser_context() {
    let (mut t, _events) = setup();
    let a = open_page(&mut t, "http://app.test/a");
    let b = open_page(&mut t, "http://app.test/b");
    let c = open_page(&mut t, "http://other.test/x");
    let other_context = t.browser.create_context().unwrap();
    let d = t.browser.create_page_in(other_context).unwrap();
    t.browser.navigate(d, NavigateOpts::address_bar("http://app.test/c")).unwrap();
    settle(&mut t);
    let listen = "globalThis.log=[]; globalThis.ch=new BroadcastChannel('ch');
        ch.onmessage = e => log.push([e.origin, JSON.stringify(e.data), e.isTrusted, e instanceof MessageEvent].join(':'));
        new BroadcastChannel('other').onmessage = () => log.push('other');
        ch.name";
    for page in [a, b, c, d] {
        assert_eq!(eval(&t, page, listen), "ch");
    }
    // A second channel of the sending document hears it; the sender does not.
    assert_eq!(
        eval(
            &t,
            a,
            "globalThis.second=new BroadcastChannel('ch'); second.onmessage = e => log.push('second:'+e.data.n);
                     const m={n:1}; ch.postMessage(m); m.n=2; log.length"
        ),
        "0",
        "delivery is a task"
    );
    settle(&mut t);
    assert_eq!(eval(&t, a, "log.join('|')"), "second:1");
    assert_eq!(eval(&t, b, "log.join('|')"), "http://app.test:{\"n\":1}:true:true");
    assert_eq!(eval(&t, c, "log.length"), "0", "other origins do not hear it");
    assert_eq!(eval(&t, d, "log.length"), "0", "other browser contexts do not hear it");
    // Closed channels neither send nor receive; cloning failures throw.
    assert_eq!(
        eval(&t, b, "ch.close(); ch.close(); try { ch.postMessage(1); 'no' } catch (e) { e.name }"),
        "InvalidStateError"
    );
    assert_eq!(
        eval(&t, a, "ch.postMessage({n:3}); try { ch.postMessage(() => 1); 'no' } catch (e) { e.name }"),
        "DataCloneError"
    );
    settle(&mut t);
    assert_eq!(eval(&t, b, "log.length"), "1", "a closed channel receives nothing");
    // A channel kept only by its listener keeps working.
    eval(&t, b, "(() => { const k = new BroadcastChannel('ch'); k.onmessage = e => log.push('kept:' + e.data.n); })()");
    eval(&t, a, "ch.postMessage({n:4})");
    settle(&mut t);
    assert_eq!(eval(&t, b, "log.join('|')"), "http://app.test:{\"n\":1}:true:true|kept:4");
}
