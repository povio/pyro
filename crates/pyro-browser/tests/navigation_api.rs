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

/// navigation-api/navigate-event/navigate-form-onformdata-navigate-crash.html:
/// a navigation the `formdata` listener starts supersedes the form's own
/// submission, as in Chromium (which never sends the POST). Before, the form
/// POSTed after it, so a page submitting itself on load never stopped.
#[test]
fn a_navigation_from_formdata_supersedes_the_submission() {
    let (mut t, page, loader) = load(APP);
    loader.insert("http://site.test/posted", "text/html", "<p id=posted>posted</p>", Duration::ZERO);
    eval(
        &t,
        page,
        "const f = document.createElement('form'); f.method = 'POST'; f.action = '/posted'; \
         f.onformdata = () => navigation.navigate('/other'); document.body.append(f); f.requestSubmit();",
    );
    settle(&mut t);
    assert_eq!(eval(&t, page, "location.pathname + '|' + !!document.getElementById('other')"), "/other|true");
    let log = loader.log.borrow().join(" ");
    assert!(!log.contains("posted"), "the form did not submit after the formdata navigation: {log}");
}

#[test]
fn a_formdata_listener_that_does_not_navigate_keeps_the_submission() {
    let (mut t, page, loader) = load(APP);
    loader.insert("http://site.test/posted", "text/html", "<p id=posted>posted</p>", Duration::ZERO);
    eval(
        &t,
        page,
        "const f = document.createElement('form'); f.method = 'POST'; f.action = '/posted'; \
         f.onformdata = e => e.formData.append('a', '1'); document.body.append(f); f.requestSubmit();",
    );
    settle(&mut t);
    assert_eq!(eval(&t, page, "location.pathname + '|' + !!document.getElementById('posted')"), "/posted|true");
}

const FORMS: &str = "<body><a id=link href='/about'>about</a>\
    <form id=post method=post action='/other'><input name=a value=1><button id=send name=b value=2>send</button></form>\
    <form id=get action='/other'><input name=q value=x></form>\
    <script>window.log = [];\
    navigation.addEventListener('navigate', e => {\
      const data = e.formData ? [...e.formData].map(p => p.join('=')).join('&') : String(e.formData);\
      log.push([e.navigationType, new URL(e.destination.url).pathname + new URL(e.destination.url).search, data,\
        e.formData instanceof FormData, e.sourceElement ? e.sourceElement.id : String(e.sourceElement),\
        e.sourceElement === null || e.sourceElement === document.getElementById(e.sourceElement.id)].join(':'));\
      e.preventDefault();\
    });</script></body>";

/// HTML "submit" hands the navigation the form's entry list and its
/// submitter (WPT navigation-api/navigate-event/navigate-form*.html): a POST
/// submission's `navigate` event has them as `formData` and `sourceElement`.
#[test]
fn a_form_submission_fires_navigate_with_its_form_data_and_source_element() {
    let (mut t, page, loader) = load(FORMS);
    // The navigation is planned: its event fires from a task, not inside submit().
    assert_eq!(eval(&t, page, "post.submit(); log.length"), "0");
    settle(&mut t);
    assert_eq!(eval(&t, page, "log.join('|')"), "push:/other:a=1:true:post:true");
    eval(&t, page, "log = []; post.requestSubmit(send)");
    settle(&mut t);
    assert_eq!(eval(&t, page, "log.join('|')"), "push:/other:a=1&b=2:true:send:true");
    // A GET submission has no form data; a link is its own source; script has none.
    eval(&t, page, "log = []; get.submit()");
    settle(&mut t);
    eval(&t, page, "link.click(); location.href = '/other'");
    settle(&mut t);
    assert_eq!(
        eval(&t, page, "log.join('|')"),
        "push:/other?q=x:null:false:get:true|push:/about:null:false:link:true|push:/other:null:false:null:true"
    );
    assert!(loader.log.borrow().is_empty(), "every navigation was canceled: {:?}", loader.log.borrow());
}

/// HTML "plan to navigate": a form submitted again before its task ran
/// navigates once, with the later submission; so does the frame when
/// another form is submitted after it (the frame's latest navigation).
#[test]
fn a_later_submission_replaces_the_forms_planned_navigation() {
    let (mut t, page, _loader) = load(FORMS);
    eval(&t, page, "post.submit(); post.a.value = 'second'; post.submit()");
    settle(&mut t);
    assert_eq!(eval(&t, page, "log.join('|')"), "push:/other:a=second:true:post:true");
    eval(&t, page, "log = []; post.submit(); get.submit()");
    settle(&mut t);
    assert_eq!(eval(&t, page, "log.join('|')"), "push:/other?q=x:null:false:get:true");
}

/// HTML "Location-object navigate" and form submission: a document that
/// navigates itself before it has completely loaded (no user gesture)
/// replaces its history entry, and the `navigate` event says so (WPT
/// navigate-location.html, navigate-form.html).
#[test]
fn a_document_navigating_while_it_loads_replaces_its_entry() {
    for (script, expected) in [
        ("location.href = '/other'", "replace:/other"),
        ("document.body.appendChild(document.createElement('form')).action = '/other'; document.forms[0].submit()", "replace:/other"),
        ("location.hash = 'x'", "replace:/next#x"),
    ] {
        let html = format!(
            "<body><script>window.log = []; \
             navigation.onnavigate = e => {{ const u = new URL(e.destination.url); \
               sessionStorage.setItem('seen', e.navigationType + ':' + u.pathname + u.search + u.hash); }}; \
             if (location.pathname == '/next') {{ {script} }}</script></body>"
        );
        let (mut t, page, loader) = load("<body>start</body>");
        loader.insert("http://site.test/next", "text/html", &html, Duration::ZERO);
        loader.insert("http://site.test/other?", "text/html", "<p id=other>other</p>", Duration::ZERO);
        eval(&t, page, "location.href = '/next'");
        settle(&mut t);
        assert_eq!(eval(&t, page, "sessionStorage.getItem('seen')"), expected, "{script}");
        // /app, then /next whose entry the navigation it started replaced.
        assert_eq!(eval(&t, page, "history.length"), "2", "{script}");
    }
    // Once loaded, the same navigations push.
    let (mut t, page, _loader) = load(APP);
    eval(&t, page, "location.href = '/other'");
    settle(&mut t);
    assert_eq!(eval(&t, page, "[location.pathname, history.length].join()"), "/other,2");
}

/// Of the navigations of a frame started before any of them began to load,
/// the last one wins (HTML "navigate" sets the navigable's ongoing
/// navigation; a planned form navigation is one of them from `submit()` on,
/// and `document.open()` stops it). Chrome's results for each case.
#[test]
fn the_last_navigation_started_in_a_task_wins_over_a_planned_form_navigation() {
    const PAGE: &str = "<body><form id=f action='/submit' method=post><input name=a value=1>\
        <button id=go onclick=\"location.href='/landed.html'\">go</button></form>\
        <form id=g action='/submit' method=post onsubmit=\"location.href='/landed.html'\"><button id=send>send</button></form>\
        <a id=link href='/landed.html' onclick='f.submit()'>link</a>\
        <script>window.marker = 'first load';</script></body>";
    // (script, where it ends, whether /submit is requested)
    let cases = [
        // A submit button whose click handler navigates: the submission is later.
        ("go.click()", "/submit", true),
        // A submit handler that navigates: the submission is planned after it.
        ("send.click()", "/submit", true),
        ("location.href = '/landed.html'; f.submit()", "/submit", true),
        ("f.submit(); location.href = '/landed.html'", "/landed.html", false),
        // A link whose click handler submits: the link is followed after it.
        ("link.click()", "/landed.html", false),
        ("f.submit(); location.reload()", "/app reloaded", false),
        ("f.submit(); Promise.resolve().then(() => location.href = '/landed.html')", "/landed.html", false),
        ("f.submit(); document.open(); document.write('<p id=written>written</p>'); document.close()", "/app written", false),
    ];
    for (script, ends_on, submits) in cases {
        let (mut t, page, loader) = load(PAGE);
        loader.insert("http://site.test/submit", "text/html", "<p>submitted</p>", Duration::ZERO);
        loader.insert("http://site.test/landed.html", "text/html", "<p>landed</p>", Duration::ZERO);
        eval(&t, page, "window.marker = 'scripted'; 0");
        eval(&t, page, &format!("{script}; 0"));
        settle(&mut t);
        let ended = eval(
            &t,
            page,
            "location.pathname + (document.getElementById('written') ? ' written' : \
               location.pathname == '/app' && window.marker == 'first load' ? ' reloaded' : '')",
        );
        assert_eq!(ended, ends_on, "{script}");
        let requested = loader.log.borrow().iter().any(|r| format!("{r:?}").contains("/submit"));
        assert_eq!(requested, submits, "{script}: {:?}", loader.log.borrow());
    }
    // A `javascript:` URL is no navigation to another document: its task
    // runs the script whatever else was started (WPT
    // url/javascript-urls.window.js follows two links in one task), and it
    // does not stop a form submission planned before or after it.
    const LINKS: &str = "const link = (code) => { const a = document.body.appendChild(document.createElement('a')); \
        a.href = 'javascript:' + code + ';undefined'; return a; }; \
        const ran = (n) => link(`sessionStorage.setItem('ran', (sessionStorage.getItem('ran') || '') + ${n})`);";
    let cases = [
        ("ran(1).click(); ran(2).click(); ran(3).click()", "/app", "123", false),
        ("ran(1).click(); f.submit()", "/submit", "1", true),
        ("f.submit(); ran(1).click(); ran(2).click()", "/submit", "12", true),
        ("location.href = '/landed.html'; ran(1).click()", "/landed.html", "1", false),
    ];
    for (script, ends_on, ran, submits) in cases {
        let (mut t, page, loader) = load(PAGE);
        loader.insert("http://site.test/submit", "text/html", "<p>submitted</p>", Duration::ZERO);
        loader.insert("http://site.test/landed.html", "text/html", "<p>landed</p>", Duration::ZERO);
        eval(&t, page, &format!("sessionStorage.removeItem('ran'); {LINKS} {script}; 0"));
        settle(&mut t);
        assert_eq!(eval(&t, page, "location.pathname + ' ' + sessionStorage.getItem('ran')"), format!("{ends_on} {ran}"), "{script}");
        let requested = loader.log.borrow().iter().any(|r| r.contains("/submit"));
        assert_eq!(requested, submits, "{script}: {:?}", loader.log.borrow());
    }
}

#[derive(Default)]
struct Popups(std::cell::RefCell<Vec<String>>);
impl pyro_browser::PageObserver for Popups {
    fn on_page_event(&self, _page: PageId, event: &pyro_browser::PageEvent) {
        if let pyro_browser::PageEvent::PopupRequested { url, .. } = event {
            self.0.borrow_mut().push(url.clone());
        }
    }
}

/// What does not ask the frame for another document leaves its planned form
/// navigation (and any navigation started) alone: a fragment navigation, a
/// `javascript:` URL, a submission to another frame or a new window (that
/// navigable's own ordering). A traversal is a task of its own: a
/// submission after `history.back()` in the same task wins. Chrome's results.
#[test]
fn fragment_links_other_frames_and_traversals_leave_a_planned_form_navigation() {
    const PAGE: &str = "<body><form id=f action='/submit'><input name=a value=1></form>\
        <form id=blank action='/submit' target=_blank><input name=to value=blank></form>\
        <form id=framed action='/submit' target=kid><input name=to value=kid></form>\
        <iframe name=kid src='/kid'></iframe>\
        <a id=hash href='#' onclick='f.submit()'>#</a><a id=named href='#x' onclick='f.submit()'>#x</a>\
        <a id=totop href='#top' onclick='f.submit()'>#top</a><a id=js href='javascript:void(0)' onclick='f.submit()'>js</a>\
        <a id=frag href='#y'>y</a><a id=stay href='#z' onclick='f.submit(); return false'>z</a>\
        <a id=jssubmit href='javascript:f.submit()'>s</a></body>";
    // (script, where the page ends, /submit requests in order of their query)
    let cases: &[(&str, &str, &[&str])] = &[
        // A. Same-document and javascript: links.
        ("hash.click()", "/submit?a=1", &["a=1"]),
        ("named.click()", "/submit?a=1", &["a=1"]),
        ("totop.click()", "/submit?a=1", &["a=1"]),
        ("js.click()", "/submit?a=1", &["a=1"]),
        ("f.submit(); frag.click()", "/submit?a=1", &["a=1"]),
        ("f.submit(); location.hash = '#x'", "/submit?a=1", &["a=1"]),
        ("stay.click()", "/submit?a=1", &["a=1"]),
        ("jssubmit.click()", "/submit?a=1", &["a=1"]),
        // B. A submission to another navigable and a navigation of this one.
        ("framed.submit(); location.href = '/landed.html'", "/landed.html", &["to=kid"]),
        ("location.href = '/landed.html'; framed.submit()", "/landed.html", &["to=kid"]),
        ("blank.submit(); location.href = '/landed.html'", "/landed.html", &["popup to=blank"]),
        ("location.href = '/landed.html'; blank.submit()", "/landed.html", &["popup to=blank"]),
        // C. History.
        ("history.back(); f.submit()", "/submit?a=1", &["a=1"]),
        ("f.submit(); history.pushState(null, '', '/pushed')", "/submit?a=1", &["a=1"]),
        ("f.submit(); history.replaceState(null, '', '/replaced')", "/submit?a=1", &["a=1"]),
        ("f.submit(); history.back()", "/submit?a=1", &["a=1"]),
        ("f.submit(); history.go(0)", "/app2", &[]),
    ];
    let mut wrong = Vec::new();
    for (script, ends_on, submits) in cases {
        let (mut t, page, loader) = load("<body>first</body>");
        let popups = Rc::new(Popups::default());
        t.browser.set_observer(Some(popups.clone()));
        loader.insert("http://site.test/app2", "text/html", PAGE, Duration::ZERO);
        loader.insert("http://site.test/kid", "text/html", "<p>kid</p>", Duration::ZERO);
        for query in ["a=1", "to=kid", "to=blank"] {
            loader.insert(&format!("http://site.test/submit?{query}"), "text/html", "<p>submitted</p>", Duration::ZERO);
        }
        loader.insert("http://site.test/landed.html", "text/html", "<p>landed</p>", Duration::ZERO);
        // /app, then /app2: there is an entry to go back to.
        t.browser.navigate(page, NavigateOpts::address_bar("http://site.test/app2")).unwrap();
        settle(&mut t);
        loader.log.borrow_mut().clear();
        eval(&t, page, &format!("{script}; 0"));
        settle(&mut t);
        let ended = eval(&t, page, "location.pathname + location.search");
        let mut seen: Vec<String> = loader
            .log
            .borrow()
            .iter()
            .filter_map(|url| url.split_once("/submit?").map(|(_, query)| query.to_string()))
            .collect();
        seen.extend(popups.0.borrow().iter().filter_map(|url| Some(format!("popup {}", url.split_once("/submit?")?.1))));
        if ended != *ends_on || seen != *submits {
            wrong.push(format!("{script}: ended on {ended}, requests {seen:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
