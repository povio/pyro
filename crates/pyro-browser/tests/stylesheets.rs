//! Stylesheet loading through the browser, using real JSC and deterministic resource delays.
use pyro_browser::{BrowserThread, InitScript, NavigateOpts, PageId, StaticLoader};
use std::rc::Rc;
use std::time::{Duration, Instant};

fn eval(thread: &BrowserThread, page: PageId, source: &str) -> String {
    let realm = thread.browser.page(page).unwrap().main_frame.main_realm().unwrap();
    let scope = realm.scope().unwrap();
    scope.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}
fn settle(thread: &mut BrowserThread) {
    assert_ne!(thread.run_until_idle(Some(Instant::now() + Duration::from_secs(3))), pyro_runtime::RunOutcome::TimedOut);
}
fn setup(html: &str) -> (BrowserThread, PageId, Rc<StaticLoader>) {
    let mut thread = crate::browser_thread().unwrap();
    let page = thread.browser.create_page().unwrap();
    thread.browser.add_init_script(page, InitScript {
        id: "events".into(), world: None,
        source: "globalThis.events = []; for (const type of ['load', 'error']) document.addEventListener(type, e => { if (e.target.tagName === 'LINK') events.push(e.target.id + ':' + type) }, true); addEventListener('load', () => events.push('window:load'));".into(),
    });
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/", "text/html", html, Duration::ZERO);
    thread.browser.set_loader(Some(loader.clone()));
    (thread, page, loader)
}

#[test]
fn linked_css_blocks_classic_scripts_and_load_and_reports_failure() {
    let (mut thread, page, loader) = setup("<link id=theme rel=stylesheet href=/theme.css><link id=missing rel=stylesheet href=/missing.css><script>globalThis.atScript = getComputedStyle(document.documentElement).display</script>");
    loader.insert("http://site.test/theme.css", "text/css", "html { display: none }", Duration::from_millis(25));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "atScript"), "none");
    assert_eq!(eval(&thread, page, "events.filter(x => x === 'theme:load').length"), "1");
    assert_eq!(eval(&thread, page, "events.filter(x => x === 'missing:error').length"), "1");
    assert_eq!(eval(&thread, page, "events[events.length - 1]"), "window:load");
}

#[test]
fn parser_script_waits_for_linked_sheet_imports() {
    let (mut thread, page, loader) = setup("<!doctype html><link id='theme' rel=stylesheet href=/parent.css><script>globalThis.atScript = theme.sheet.cssRules[0].styleSheet?.cssRules.length</script>");
    loader.insert("http://site.test/parent.css", "text/css", "@import url(child.css)", Duration::ZERO);
    loader.insert("http://site.test/child.css", "text/css", "#target { position: fixed }", Duration::from_millis(25));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "atScript"), "1");
}

#[test]
fn stylesheet_cascade_follows_discovery_order_not_completion_order() {
    let (mut thread, page, loader) =
        setup("<link rel=stylesheet href=/first.css><link rel=stylesheet href=/second.css>");
    loader.insert("http://site.test/first.css", "text/css", "html { display: none }", Duration::from_millis(30));
    loader.insert("http://site.test/second.css", "text/css", "html { display: block }", Duration::ZERO);
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert!(loader.log.borrow().iter().any(|url| url.ends_with("/first.css")));
    assert_eq!(eval(&thread, page, "getComputedStyle(document.documentElement).display"), "block");
}

#[test]
fn dynamic_href_replacement_ignores_old_completion_and_removal_releases_css() {
    let (mut thread, page, loader) = setup("<p>dynamic</p>");
    loader.insert("http://site.test/old.css", "text/css", "html { display: none }", Duration::from_millis(60));
    loader.insert("http://site.test/new.css", "text/css", "html { display: block }", Duration::ZERO);
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    eval(&thread, page, "globalThis.link = document.createElement('link'); link.id='dynamic'; link.rel='stylesheet'; link.href='/old.css'; document.head.append(link); 'ok'");
    thread.run_until(Some(Instant::now() + Duration::from_secs(1)), |_| {
        loader.log.borrow().iter().any(|u| u.ends_with("old.css"))
    });
    eval(&thread, page, "link.href='/new.css'; 'ok'");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "events.filter(e => e.startsWith('dynamic:')).join('|')"), "dynamic:load");
    assert_eq!(eval(&thread, page, "getComputedStyle(document.documentElement).display"), "block");
    assert!(thread.browser.page(page).unwrap().stylesheet_usage().1 > 0);
    eval(&thread, page, "link.remove(); 'ok'");
    settle(&mut thread);
    assert_eq!(thread.browser.page(page).unwrap().stylesheet_usage(), (0, 0));
}

#[test]
fn navigation_releases_document_while_old_stylesheet_completion_is_pending() {
    let (mut thread, page, loader) = setup("<link rel=stylesheet href=/old.css>");
    loader.insert("http://site.test/old.css", "text/css", "html { display: none }", Duration::from_millis(60));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    thread
        .run_until(Some(Instant::now() + Duration::from_secs(1)), |b| b.page(page).unwrap().stylesheet_usage().0 == 1);
    let weak = Rc::downgrade(thread.browser.page(page).unwrap().main_frame.page_dom.as_ref().unwrap());
    thread.browser.navigate(page, NavigateOpts::address_bar("about:blank")).unwrap();
    thread.browser.engine.collect_garbage_now();
    thread.browser.engine.drain_finalized();
    assert!(weak.upgrade().is_none(), "pending CSS retained old binding state");
    assert_eq!(thread.browser.page(page).unwrap().stylesheet_usage(), (0, 0));
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "getComputedStyle(document.documentElement).display"), "block");
    assert_eq!(eval(&thread, page, "events.join('|')"), "window:load");
}

#[test]
fn deferred_scripts_wait_for_css_but_dcl_alone_does_not() {
    let (mut thread, page, loader) =
        setup("<link id=theme rel=stylesheet href=/theme.css><script defer src='/defer.js'></script>");
    loader.insert("http://site.test/theme.css", "text/css", "html { display: none }", Duration::from_millis(20));
    loader.insert(
        "http://site.test/defer.js",
        "text/javascript",
        "events.push('defer:' + getComputedStyle(document.documentElement).display)",
        Duration::ZERO,
    );
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "events.join('|')"), "theme:load|defer:none|window:load");
    loader.insert("http://site.test/", "text/html", "<link id=theme rel=stylesheet href=/theme.css>", Duration::ZERO);
    thread.browser.add_init_script(
        page,
        InitScript {
            id: "dcl".into(),
            world: None,
            source: "document.addEventListener('DOMContentLoaded', () => events.push('dcl'))".into(),
        },
    );
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "events.join('|')"), "dcl|theme:load|window:load");
}

#[test]
fn stylesheet_inserted_by_dcl_listener_delays_window_load() {
    let (mut thread, page, loader) = setup("<p>content</p>");
    loader.insert("http://site.test/theme.css", "text/css", "html { display: none }", Duration::from_millis(20));
    thread.browser.add_init_script(page, InitScript { id: "late-style".into(), world: None, source: "document.addEventListener('DOMContentLoaded', () => { const link = document.createElement('link'); link.id='late'; link.rel='stylesheet'; link.href='/theme.css'; document.head.append(link); });".into() });
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "events.join('|')"), "late:load|window:load");
}

#[test]
fn invalid_mime_integrity_and_oversized_css_report_error_without_retaining_bytes() {
    // A standards-mode document: a quirks-mode one takes a same-origin sheet of any type.
    let (mut thread, page, loader) = setup("<!doctype html><link id=mime rel=stylesheet href=/wrong.css><link id=integrity rel=stylesheet href=/integrity.css integrity='sha256-mismatch'><link id=large rel=stylesheet href=/large.css>");
    loader.insert("http://site.test/wrong.css", "text/html", "html { display: none }", Duration::ZERO);
    loader.insert("http://site.test/integrity.css", "text/css", "html { display: none }", Duration::ZERO);
    loader.insert("http://site.test/large.css", "text/css", &" ".repeat(4 * 1024 * 1024 + 1), Duration::ZERO);
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(
        eval(&thread, page, "events.filter(e => e.endsWith(':error')).sort().join('|')"),
        "integrity:error|large:error|mime:error"
    );
    assert_eq!(thread.browser.page(page).unwrap().stylesheet_usage(), (0, 0));
    assert_eq!(eval(&thread, page, "getComputedStyle(document.documentElement).display"), "block");
    assert_eq!(eval(&thread, page, "events[events.length - 1]"), "window:load");
}

#[test]
fn integrity_matching_stylesheets_apply_and_opaque_ones_are_blocked() {
    let css = "html { display: none }";
    let sri = pyro_webapi::sri::sha256_expression(css.as_bytes());
    let (mut thread, page, loader) = setup(&format!(
        "<link id=ok rel=stylesheet href=/ok.css integrity='{sri}'>\
         <link id=unknown rel=stylesheet href=/unknown.css integrity='md5-whatever'>\
         <link id=opaque rel=stylesheet href=http://cdn.test/opaque.css integrity='{sri}'>\
         <link id=cors rel=stylesheet href=http://cdn.test/cors.css integrity='{sri}' crossorigin=anonymous>"
    ));
    for url in [
        "http://site.test/ok.css",
        "http://site.test/unknown.css",
        "http://cdn.test/opaque.css",
        "http://cdn.test/cors.css",
    ] {
        loader.insert(url, "text/css", css, Duration::ZERO);
    }
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(
        eval(&thread, page, "events.filter(e => e !== 'window:load').sort().join('|')"),
        "cors:load|ok:load|opaque:error|unknown:load"
    );
    assert_eq!(eval(&thread, page, "getComputedStyle(document.documentElement).display"), "none");
}

/// A link whose `rel` loses `stylesheet` stops styling at once (computed style and the render
/// document), also when the token is back before the task ends; with it back the link is a new
/// stylesheet link: fetched and `load` fired again. Removing `rel` drops the sheet too.
/// WPT html/semantics/document-metadata `the-link-element/link-rel-attribute`.
#[test]
fn a_link_whose_rel_drops_stylesheet_stops_styling_and_loads_again_with_it_back() {
    let (mut thread, page, loader) = setup("<link id=theme rel=stylesheet href=/theme.css><p id=p>x</p>");
    loader.insert("http://site.test/theme.css", "text/css", "#p { color: rgb(0, 128, 0) }", Duration::ZERO);
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    let color = "getComputedStyle(document.getElementById('p')).color";
    assert_eq!(eval(&thread, page, color), "rgb(0, 128, 0)");
    let fetches = |loader: &StaticLoader| loader.log.borrow().iter().filter(|u| u.ends_with("/theme.css")).count();
    assert_eq!(fetches(&loader), 1);
    // Both writes in one task: the read between them sees the link without its sheet.
    let between = format!(
        "var theme = document.getElementById('theme'); theme.setAttribute('rel', 'no-stylesheet'); var off = {color}; theme.setAttribute('rel', 'stylesheet'); off"
    );
    assert_eq!(eval(&thread, page, &between), "rgb(0, 0, 0)");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "events.filter(x => x === 'theme:load').length"), "2");
    assert_eq!(fetches(&loader), 2);
    assert_eq!(eval(&thread, page, color), "rgb(0, 128, 0)");
    // Writing the same `rel` again changes nothing.
    eval(&thread, page, "theme.setAttribute('rel', 'stylesheet'); 'ok'");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "events.filter(x => x === 'theme:load').length"), "2");
    assert_eq!(fetches(&loader), 2);
    assert_eq!(eval(&thread, page, &format!("theme.removeAttribute('rel'); {color}")), "rgb(0, 0, 0)");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, color), "rgb(0, 0, 0)");
    assert_eq!(thread.browser.page(page).unwrap().stylesheet_usage(), (0, 0));
}

#[test]
fn base_href_resolves_stylesheet_urls() {
    let (mut thread, page, loader) = setup("<base href='/assets/'><link id=theme rel=stylesheet href=theme.css>");
    loader.insert(
        "http://site.test/deep/route/",
        "text/html",
        "<base href='/assets/'><link id=theme rel=stylesheet href=theme.css>",
        Duration::ZERO,
    );
    loader.insert("http://site.test/assets/theme.css", "text/css", "html { display: none }", Duration::ZERO);
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/deep/route/")).unwrap();
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "events.join('|')"), "theme:load|window:load");
    assert!(!loader.log.borrow().iter().any(|u| u.contains("/deep/route/theme.css")));
    assert_eq!(eval(&thread, page, "getComputedStyle(document.documentElement).display"), "none");
}

#[test]
fn source_budget_is_recovered_when_a_loaded_link_is_removed() {
    let html: String = (0..5).map(|i| format!("<link id=s{i} rel=stylesheet href=/sheet{i}.css>")).collect();
    let (mut thread, page, loader) = setup(&html);
    let text = " ".repeat(4 * 1024 * 1024);
    for i in 0..5 {
        loader.insert(&format!("http://site.test/sheet{i}.css"), "text/css", &text, Duration::ZERO);
    }
    loader.insert("http://site.test/small.css", "text/css", "html { display: none }", Duration::ZERO);
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(thread.browser.page(page).unwrap().stylesheet_usage(), (0, 16 * 1024 * 1024));
    assert_eq!(eval(&thread, page, "events.filter(e => e.endsWith(':error')).length"), "1");
    eval(
        &thread,
        page,
        "document.getElementById('s0').remove(); document.getElementById('s4').href='/small.css'; 'ok'",
    );
    settle(&mut thread);
    assert_eq!(
        thread.browser.page(page).unwrap().stylesheet_usage(),
        (0, 12 * 1024 * 1024 + "html { display: none }".len())
    );
}

#[test]
fn sheet_count_limit_rejects_work_without_leaving_pending_state() {
    let html: String = (0..129).map(|i| format!("<link id=s{i} rel=stylesheet href=/same.css>")).collect();
    let (mut thread, page, loader) = setup(&html);
    loader.insert("http://site.test/same.css", "text/css", "html {}", Duration::ZERO);
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(loader.log.borrow().iter().filter(|u| u.ends_with("same.css")).count(), 128);
    assert_eq!(eval(&thread, page, "events.filter(e => e.endsWith(':error')).length"), "1");
    assert_eq!(thread.browser.page(page).unwrap().stylesheet_usage().0, 0);
    eval(&thread, page, "document.querySelectorAll('link').forEach(l => l.remove()); 'ok'");
    settle(&mut thread);
    assert_eq!(thread.browser.page(page).unwrap().stylesheet_usage(), (0, 0));
}
