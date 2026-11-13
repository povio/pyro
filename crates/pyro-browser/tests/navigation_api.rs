//! The Navigation API (`window.navigation`, upstream WPT navigation-api/):
//! entries, `navigate` events for every kind of navigation, `intercept()`
//! turning a load into a same-document commit, and the navigate()/back()
//! promises.
use pyro_browser::{BrowserThread, NavigateOpts, PageId, StaticLoader};
use std::rc::Rc;
use std::time::{Duration, Instant};

fn eval(t: &BrowserThread, page: PageId, source: &str) -> String {
    let s = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().scope().unwrap();
    s.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&s).text())).to_string().unwrap()
}

fn settle(t: &mut BrowserThread) {
    assert_eq!(t.run_until_idle(Some(Instant::now() + Duration::from_secs(2))), pyro_runtime::RunOutcome::Idle);
}

fn load(html: &str) -> (BrowserThread, PageId, Rc<StaticLoader>) {
    let mut t = crate::browser_thread().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/app", "text/html", html, Duration::ZERO);
    loader.insert("http://site.test/other", "text/html", "<p id=other>other</p>", Duration::ZERO);
    t.browser.set_loader(Some(loader.clone()));
    let page = t.browser.create_page().unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar("http://site.test/app")).unwrap();
    settle(&mut t);
    loader.log.borrow_mut().clear();
    (t, page, loader)
}

const APP: &str = "<body><a id=link href='/about'>about</a><a id=away href='http://elsewhere.test/'>x</a>\
    <script>window.log = []; window.marker = 'kept';\
    navigation.addEventListener('navigate', e => {\
      log.push(['navigate', e.navigationType, new URL(e.destination.url).pathname + new URL(e.destination.url).hash,\
        e.canIntercept, e.hashChange, e.cancelable].join(':'));\
      if (window.intercept && e.canIntercept) e.intercept({handler: async () => { log.push('handler:' + location.pathname); }});\
      if (window.cancel) e.preventDefault();\
    });\
    navigation.oncurrententrychange = e => log.push('change:' + e.navigationType + ':' + new URL(e.from.url).pathname);\
    navigation.addEventListener('navigatesuccess', () => log.push('success'));\
    navigation.addEventListener('navigateerror', e => log.push('error:' + e.error.name));\
    </script></body>";

#[test]
fn entries_follow_the_session_history() {
    let (mut t, page, _loader) = load(APP);
    assert_eq!(
        eval(
            &t,
            page,
            "const c = navigation.currentEntry; \
             [navigation.entries().length, c === navigation.currentEntry, c.url, c.index, c.sameDocument, \
              typeof c.key, c.getState(), navigation.canGoBack, navigation.canGoForward, navigation.transition, \
              navigation instanceof EventTarget, Object.prototype.toString.call(navigation)].join()"
        ),
        "1,true,http://site.test/app,0,true,string,,false,false,,true,[object Navigation]"
    );
    eval(&t, page, "history.pushState(null, '', '/a'); history.pushState(null, '', '/b');");
    assert_eq!(
        eval(
            &t,
            page,
            "[navigation.entries().map(e => new URL(e.url).pathname).join('|'), navigation.currentEntry.index, \
              navigation.canGoBack, log.filter(l => l.startsWith('navigate')).join('|')].join()"
        ),
        "/app|/a|/b,2,true,navigate:push:/a:true:false:true|navigate:push:/b:true:false:true"
    );
    settle(&mut t);
    // replaceState keeps the key, gets a new id.
    assert_eq!(
        eval(&t, page, "const k = navigation.currentEntry.key, i = navigation.currentEntry.id; history.replaceState(null, '', '/c'); \
             [navigation.currentEntry.key === k, navigation.currentEntry.id === i, navigation.entries().length].join()"),
        "true,false,3"
    );
    assert_eq!(eval(&t, page, "log.includes('change:push:/a') && log.includes('success')"), "true");
}

#[test]
fn intercepting_a_link_click_keeps_the_document() {
    let (mut t, page, loader) = load(APP);
    eval(&t, page, "window.intercept = true; link.click()");
    settle(&mut t);
    assert_eq!(
        eval(&t, page, "[marker, location.pathname, history.length, log.join('|')].join()"),
        "kept,/about,2,navigate:push:/about:true:false:true|change:push:/app|handler:/about|success"
    );
    assert!(loader.log.borrow().is_empty(), "nothing was fetched: {:?}", loader.log.borrow());
    // A cross-origin link cannot be intercepted: it loads.
    eval(
        &t,
        page,
        "log = []; navigation.addEventListener('navigate', e => { if (!e.canIntercept) window.sawAway = true; })",
    );
    eval(&t, page, "away.click(); sawAway");
    assert!(eval(&t, page, "log.join('|')").starts_with("navigate:push:/:false:false:true"));
}

#[test]
fn a_canceled_navigate_event_stops_the_navigation() {
    let (mut t, page, loader) = load(APP);
    eval(
        &t,
        page,
        "window.cancel = true; location.href = '/other'; history.pushState(null, '', '/p'); location.hash = 'h'",
    );
    settle(&mut t);
    assert_eq!(eval(&t, page, "[marker, location.href, history.length].join()"), "kept,http://site.test/app,1");
    assert_eq!(
        eval(&t, page, "log.filter(l => l.startsWith('error')).join()"),
        "error:AbortError,error:AbortError,error:AbortError"
    );
    assert!(loader.log.borrow().is_empty());
}

#[test]
fn navigate_and_traverse_promises() {
    let (mut t, page, _loader) = load(APP);
    eval(
        &t,
        page,
        "window.intercept = true; window.out = []; \
         const r = navigation.navigate('/x', {state: {n: 1}, info: 'i'}); \
         r.committed.then(e => out.push('committed:' + new URL(e.url).pathname + ':' + e.getState().n)); \
         r.finished.then(e => out.push('finished:' + (e === navigation.currentEntry)));",
    );
    settle(&mut t);
    assert_eq!(eval(&t, page, "out.join() + ' ' + location.pathname"), "committed:/x:1,finished:true /x");
    // Same-document traversal through navigation.back().
    eval(&t, page, "out = []; navigation.back().finished.then(e => out.push('back:' + new URL(e.url).pathname))");
    settle(&mut t);
    assert_eq!(eval(&t, page, "out.join() + ' ' + location.pathname"), "back:/app /app");
    assert!(eval(&t, page, "log.join('|')").contains("navigate:traverse:/app:true:false:true"));
    // updateCurrentEntry sets the state; a bad key rejects.
    assert_eq!(eval(&t, page, "navigation.updateCurrentEntry({state: 5}); navigation.currentEntry.getState()"), "5");
    eval(&t, page, "navigation.traverseTo('nope').finished.catch(e => out.push(e.name))");
    settle(&mut t);
    assert_eq!(eval(&t, page, "out.at(-1)"), "InvalidStateError");
    // Without intercept, navigate() to another document loads it.
    eval(&t, page, "window.intercept = false; navigation.navigate('/other')");
    settle(&mut t);
    assert_eq!(eval(&t, page, "typeof marker + ' ' + !!document.getElementById('other')"), "undefined true");
}
