//! Following a hyperlink to the document's own URL with a fragment is a
//! fragment navigation (HTML "navigate"): the URL changes at once, the
//! document stays, and `hashchange` is queued. A `javascript:` link runs its
//! script in the document (dom/events/Event-dispatch-single-activation-behavior,
//! Event-dispatch-click).
use lp_browser::{BrowserThread, NavigateOpts, PageId, StaticLoader};
use std::rc::Rc;
use std::time::{Duration, Instant};

fn eval(t: &BrowserThread, page: PageId, source: &str) -> String {
    let s = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().scope().unwrap();
    s.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&s).text())).to_string().unwrap()
}

fn settle(t: &mut BrowserThread) {
    assert_eq!(t.run_until_idle(Some(Instant::now() + Duration::from_secs(2))), lp_runtime::RunOutcome::Idle);
}

fn load(html: &str) -> (BrowserThread, PageId, Rc<StaticLoader>) {
    let mut t = crate::browser_thread().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/page?q=1", "text/html", html, Duration::ZERO);
    loader.insert("http://site.test/other", "text/html", "<p id=other>other</p>", Duration::ZERO);
    t.browser.set_loader(Some(loader.clone()));
    let page = t.browser.create_page().unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar("http://site.test/page?q=1")).unwrap();
    settle(&mut t);
    loader.log.borrow_mut().clear();
    (t, page, loader)
}

const PAGE: &str = "<body><a id=link href='#section'>go</a><map><area id=area href='#spot'></map>\
    <a id=away href='/other'>away</a>\
    <script>window.marker = 'kept'; window.changes = [];\
    onhashchange = e => changes.push(e.oldURL.split('#')[1] + '>' + e.newURL.split('#')[1]);</script></body>";

#[test]
fn a_fragment_link_changes_the_url_without_reloading() {
    let (mut t, page, loader) = load(PAGE);
    // The URL has changed when click() returns; hashchange comes as a task.
    assert_eq!(
        eval(&t, page, "document.getElementById('link').click(); location.href + ' ' + changes.length"),
        "http://site.test/page?q=1#section 0"
    );
    settle(&mut t);
    assert_eq!(eval(&t, page, "[marker, changes.join(), history.length].join(' ')"), "kept undefined>section 2");
    assert_eq!(t.browser.page(page).unwrap().main_frame.url, "http://site.test/page?q=1#section");
    // An area element is a hyperlink too; the same fragment again changes nothing.
    eval(&t, page, "document.getElementById('area').click(); document.getElementById('area').click()");
    settle(&mut t);
    assert_eq!(
        eval(&t, page, "[marker, location.hash, changes.join(), history.length].join(' ')"),
        "kept #spot undefined>section,section>spot 3"
    );
    assert!(loader.log.borrow().is_empty(), "no document was fetched: {:?}", loader.log.borrow());
}

#[test]
fn a_link_elsewhere_still_loads_a_document() {
    let (mut t, page, _loader) = load(PAGE);
    eval(&t, page, "document.getElementById('away').click()");
    settle(&mut t);
    assert_eq!(eval(&t, page, "location.href + ' ' + typeof marker + ' ' + !!document.getElementById('other')"), "http://site.test/other undefined true");
}

#[test]
fn a_javascript_link_runs_its_script_in_the_document() {
    let (mut t, page, loader) = load(
        "<body><a id=outer href=\"javascript:ran.push('outer')\"><a id=inner></a></a>\
         <script>window.ran = []; document.getElementById('inner').href = \"javascript:ran.push('in%6eer')\";</script></body>",
    );
    // Only the inner link is the activation target; its URL is percent-decoded.
    eval(&t, page, "document.getElementById('inner').dispatchEvent(new MouseEvent('click', {bubbles: true}))");
    assert_eq!(eval(&t, page, "ran.join()"), "", "the script runs as a task");
    settle(&mut t);
    assert_eq!(eval(&t, page, "ran.join() + ' ' + location.href"), "inner http://site.test/page?q=1");
    assert!(loader.log.borrow().is_empty());
}
