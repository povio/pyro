//! `window.postMessage` between frames and popups (source, origin,
//! targetOrigin, structured clone, ports), the cross-origin WindowProxy's
//! `location`, `postMessage` and indexed access, and structured-clone
//! history state.
use lp_browser::{BrowserThread, NavigateOpts, PageEvent, PageId, PageObserver, StaticLoader, DEFAULT_CONTEXT};
use lp_js::Realm;
use lp_webapi::PopupTicket;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

fn eval(realm: &Realm, source: &str) -> String {
    let scope = realm.scope().unwrap();
    scope
        .eval(source, Some("messaging-test.js"), 1)
        .unwrap_or_else(|e| panic!("{source}: {}", e.details(&scope).text()))
        .to_string()
        .unwrap()
}
fn main_realm(t: &BrowserThread, page: PageId) -> Realm {
    t.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone()
}
fn child_realm(t: &BrowserThread, page: PageId, index: usize) -> Realm {
    let frames: Vec<_> = t.browser.frames(page).into_iter().filter(|f| f.parent.is_some()).collect();
    t.browser.frame(page, &frames[index].id).unwrap().main_realm().unwrap().clone()
}
fn settle(t: &mut BrowserThread) {
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(3)));
}

fn loaded(urls: &[&str]) -> (BrowserThread, Rc<Events>) {
    let mut t = crate::browser_thread().unwrap();
    let loader = Rc::new(StaticLoader::default());
    for url in urls {
        loader.insert(url, "text/html", "<p>page</p>", Duration::ZERO);
    }
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

#[derive(Default)]
struct Events(RefCell<Vec<(PageId, PageEvent)>>);
impl PageObserver for Events {
    fn on_page_event(&self, page: PageId, event: &PageEvent) {
        self.0.borrow_mut().push((page, event.clone()));
    }
}
impl Events {
    fn last_popup(&self) -> (PageId, String, Option<Rc<PopupTicket>>) {
        self.0
            .borrow()
            .iter()
            .rev()
            .find_map(|(page, e)| match e {
                PageEvent::PopupRequested { url, ticket, .. } => Some((*page, url.clone(), ticket.clone())),
                _ => None,
            })
            .expect("a popup was requested")
    }
}

#[test]
fn same_origin_frames_post_cloned_messages_with_the_incumbent_source_and_origin() {
    let (mut t, _) = loaded(&["https://a.test/main", "https://a.test/child"]);
    let page = open_page(&mut t, "https://a.test/main");
    let root = main_realm(&t, page);
    eval(&root, "globalThis.f = document.createElement('iframe'); f.src = '/child'; document.body.appendChild(f)");
    settle(&mut t);
    let child = child_realm(&t, page, 0);
    eval(
        &child,
        "globalThis.got = []; addEventListener('message', e => {
           got.push([e.source === parent, e.origin, e.data.d instanceof Date, e.data.n, e.ports.length].join(':'));
           e.source.postMessage({back: e.data.n}, '*');
         })",
    );
    assert_eq!(
        eval(
            &root,
            "globalThis.replies = []; addEventListener('message', e => replies.push([e.source === f.contentWindow, e.origin, e.data.back].join(':')));
             const o = {d: new Date(1), n: 1}; f.contentWindow.postMessage(o, '/'); o.n = 2;
             const err = (fn) => { try { fn(); return 'ok'; } catch (e) { return e.name + ':' + (e instanceof f.contentWindow.DOMException); } };
             [err(() => f.contentWindow.postMessage(() => 1, '*')), err(() => f.contentWindow.postMessage('x', 'nope')),
              err(() => f.contentWindow.postMessage('x', {transfer: null})).split(':')[0]].join()",
        ),
        "DataCloneError:true,SyntaxError:true,TypeError"
    );
    // Dropped: the child is not of this target origin.
    eval(&root, "f.contentWindow.postMessage({n: 3}, 'https://b.test')");
    settle(&mut t);
    assert_eq!(eval(&child, "got.join('|')"), "true:https://a.test:true:1:0");
    assert_eq!(eval(&root, "replies.join('|')"), "true:https://a.test:1");
}

#[test]
fn cross_origin_frames_message_each_other_and_navigate_through_location() {
    let (mut t, _) =
        loaded(&["https://a.test/main", "https://b.test/child", "https://b.test/two", "https://b.test/three"]);
    let page = open_page(&mut t, "https://a.test/main");
    let root = main_realm(&t, page);
    eval(
        &root,
        "globalThis.f = document.createElement('iframe'); f.src = 'https://b.test/child'; document.body.appendChild(f)",
    );
    settle(&mut t);
    let child = child_realm(&t, page, 0);
    eval(
        &child,
        "globalThis.got = []; addEventListener('message', e => {
           got.push([e.source === parent, e.origin, e.data].join(':'));
           e.source.postMessage('back:' + e.data, 'https://a.test');
         })",
    );
    assert_eq!(
        eval(
            &root,
            "globalThis.replies = []; addEventListener('message', e => replies.push([e.source === f.contentWindow, e.origin, e.data].join(':')));
             const w = f.contentWindow; w.postMessage('hi', 'https://b.test'); w.postMessage('lost', 'https://a.test');
             const err = (fn) => { try { fn(); return 'ok'; } catch (e) { return e.name; } };
             [typeof w.postMessage, err(() => w.document), err(() => w.location.href), w[0], w.length].join()",
        ),
        "function,SecurityError,SecurityError,,0"
    );
    settle(&mut t);
    assert_eq!(eval(&child, "got.join('|')"), "true:https://a.test:hi");
    assert_eq!(eval(&root, "replies.join('|')"), "true:https://b.test:back:hi");
    // A cross-origin window can be navigated, not read.
    eval(&root, "f.contentWindow.location = 'https://b.test/two'");
    settle(&mut t);
    assert_eq!(eval(&child_realm(&t, page, 0), "location.href"), "https://b.test/two");
    eval(&root, "f.contentWindow.location.replace('https://b.test/three')");
    settle(&mut t);
    assert_eq!(eval(&child_realm(&t, page, 0), "[location.href, history.length].join()"), "https://b.test/three,2");
}

#[test]
fn cross_origin_post_message_errors_belong_to_the_posting_realm() {
    // The clone into another origin's realm must not hand the poster that
    // realm's error objects (and so its Function constructor).
    let (mut t, _) = loaded(&["https://a.test/main", "https://b.test/child"]);
    let page = open_page(&mut t, "https://a.test/main");
    let root = main_realm(&t, page);
    eval(
        &root,
        "globalThis.f = document.createElement('iframe'); f.src = 'https://b.test/child'; document.body.appendChild(f)",
    );
    settle(&mut t);
    let child = child_realm(&t, page, 0);
    eval(
        &child,
        "globalThis.got = []; addEventListener('message', e => {
           if (e.ports.length) e.ports[0].onmessage = m => got.push('port:' + JSON.stringify(m.data) + ':' + (m.data.constructor === Object));
           else got.push('win:' + JSON.stringify(e.data) + ':' + (e.data.constructor === Object));
         })",
    );
    assert_eq!(
        eval(
            &root,
            "const w = f.contentWindow;
             const err = (fn) => { try { fn(); return 'ok'; } catch (e) {
               return [e.name, e instanceof DOMException || e instanceof TypeError || e instanceof RangeError,
                       e.constructor.constructor === Function].join(':'); } };
             globalThis.c = new MessageChannel();
             [err(() => w.postMessage(() => 1, '*')), err(() => w.postMessage(1, {transfer: [1]})),
              err(() => w.postMessage({a: document}, '*')),
              err(() => w.postMessage({get x() { throw new RangeError('getter'); }}, '*')),
              err(() => w.postMessage({k: 'v'}, '*')), err(() => w.postMessage('p', '*', [c.port2]))].join()",
        ),
        "DataCloneError:true:true,TypeError:true:true,DataCloneError:true:true,RangeError:true:true,ok,ok"
    );
    settle(&mut t);
    assert_eq!(
        eval(
            &root,
            "const perr = (fn) => { try { fn(); return 'ok'; } catch (e) {
               return [e.name, e instanceof DOMException, e.constructor.constructor === Function].join(':'); } };
             [perr(() => c.port1.postMessage(() => 1)), perr(() => c.port1.postMessage({k: 'v'})), c.port2.constructor === MessagePort].join()",
        ),
        "DataCloneError:true:true,ok,true"
    );
    settle(&mut t);
    assert_eq!(eval(&child, "got.join('|')"), "win:{\"k\":\"v\"}:true|port:{\"k\":\"v\"}:true");
}

#[test]
fn popup_and_opener_see_each_other_as_message_source() {
    let (mut t, events) = loaded(&["http://app.test/a", "http://app.test/b", "http://other.test/x"]);
    let a = open_page(&mut t, "http://app.test/a");
    let opener = main_realm(&t, a);
    eval(
        &opener,
        "globalThis.got = []; addEventListener('message', e => got.push([e.source === p, e.origin, e.data].join(':')));
         globalThis.p = window.open('b', 'pop')",
    );
    settle(&mut t);
    let (from, url, ticket) = events.last_popup();
    let b = t.browser.create_page_in(DEFAULT_CONTEXT).unwrap();
    t.browser.adopt_popup(b, from, ticket);
    t.browser.navigate(b, NavigateOpts::address_bar(&url)).unwrap();
    settle(&mut t);
    let popup = main_realm(&t, b);
    eval(&popup, "opener.postMessage('same', '*')");
    settle(&mut t);
    assert_eq!(eval(&opener, "got.join('|')"), "true:http://app.test:same");
    // Cross-origin: the popup reaches its opener through the stand-in.
    t.browser.navigate(b, NavigateOpts::address_bar("http://other.test/x")).unwrap();
    settle(&mut t);
    let popup = main_realm(&t, b);
    assert_eq!(
        eval(
            &popup,
            "try { opener.document; 'no' } catch (e) { opener.postMessage('cross', 'http://app.test'); e.name }"
        ),
        "SecurityError"
    );
    settle(&mut t);
    assert_eq!(eval(&opener, "got.join('|')"), "true:http://app.test:same|true:http://other.test:cross");
}

#[test]
fn history_state_is_structured_cloned() {
    let (mut t, _) = loaded(&["https://a.test/main"]);
    let page = open_page(&mut t, "https://a.test/main");
    let root = main_realm(&t, page);
    assert_eq!(
        eval(
            &root,
            "const s = {d: new Date(5), m: new Map([[1, 'one']]), u: undefined, big: 10n, err: new RangeError('r')}; s.self = s;
             history.pushState(s, '', '#one'); s.d = null;
             const h = history.state;
             let clone; try { history.pushState({f() {}}, ''); clone = 'ok'; } catch (e) { clone = e.name; }
             [h === history.state, h !== s, h.d instanceof Date, h.d.getTime(), h.m.get(1), 'u' in h, typeof h.big,
              h.err instanceof RangeError, h.self === h, clone, location.hash].join()",
        ),
        "true,true,true,5,one,true,bigint,true,true,DataCloneError,#one"
    );
    eval(
        &root,
        "history.pushState(null, '', '#two'); globalThis.popped = [];
         addEventListener('popstate', e => popped.push([e.state === history.state, e.state.m instanceof Map, e.state.d.getTime()].join(':')));
         history.back()",
    );
    settle(&mut t);
    assert_eq!(eval(&root, "popped.join('|')"), "true:true:5");
}
