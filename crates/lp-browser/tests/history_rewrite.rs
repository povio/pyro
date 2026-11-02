//! `history.pushState`/`replaceState` URL checks (HTML "can have its URL
//! rewritten"). Expected results were observed in Chromium 147 headless shell
//! with the same `replaceState` calls (the http cases on a local server); the
//! `:80` case relies on WHATWG URL parsing dropping the default port.
use lp_browser::{BrowserThread, NavigateOpts, PageId, StaticLoader};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// `replaceState(null, '', target)`: the resulting `location.href`, or the
/// error name; the original URL is restored after a success.
fn rewrite(thread: &BrowserThread, page: PageId, target: &str) -> String {
    let realm = thread.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
    let scope = realm.scope().unwrap();
    let source = format!(
        "(() => {{ const before = location.href;
            try {{ history.replaceState(null, '', {target:?}); const after = location.href;
                   history.replaceState(null, '', before); return 'ok ' + after; }}
            catch (e) {{ return e.name; }} }})()"
    );
    scope.eval(&source, None, 1).unwrap().to_string().unwrap()
}

fn check(thread: &BrowserThread, page: PageId, cases: &[(&str, &str)]) {
    let got: Vec<(&str, String)> = cases.iter().map(|(t, _)| (*t, rewrite(thread, page, t))).collect();
    let want: Vec<(&str, String)> = cases.iter().map(|(t, w)| (*t, w.to_string())).collect();
    assert_eq!(got, want);
}

#[test]
fn about_blank_may_change_its_query_and_fragment_only() {
    let mut thread = crate::browser_thread().unwrap();
    let page = thread.browser.create_page().unwrap();
    check(
        &thread,
        page,
        &[
            ("#f", "ok about:blank#f"),
            ("about:blank#f", "ok about:blank#f"),
            ("about:blank?q", "ok about:blank?q"),
            ("about:blank?q#f", "ok about:blank?q#f"),
            ("ABOUT:blank#f", "ok about:blank#f"),
            // Relative to an opaque path: fails to parse, a SecurityError here.
            ("?q", "SecurityError"),
            ("about:srcdoc", "SecurityError"),
            ("data:text/html,<p>d</p>#f", "SecurityError"),
            ("http://site.test/c", "SecurityError"),
            ("javascript:void 0", "SecurityError"),
            ("blob:null/x", "SecurityError"),
        ],
    );
    // pushState too, and the new URL is the document's.
    let realm = thread.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
    let scope = realm.scope().unwrap();
    let pushed = scope.eval("history.pushState({n: 1}, '', '#x'); location.href + ' ' + history.length", None, 1);
    assert_eq!(pushed.unwrap().to_string().unwrap(), "about:blank#x 2");
    thread.run_until_idle(Some(Instant::now() + Duration::from_millis(100)));
}

#[test]
fn http_documents_may_change_path_query_and_fragment_within_their_origin() {
    let mut thread = crate::browser_thread().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/a/b?x=1#h", "text/html", "<p>a</p>", Duration::ZERO);
    thread.browser.set_loader(Some(loader));
    let page = thread.browser.create_page().unwrap();
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/a/b?x=1#h")).unwrap();
    let loaded = thread.run_until(Some(Instant::now() + Duration::from_secs(5)), |b| {
        b.page(page).is_some_and(|p| p.main_frame.url.starts_with("http://site.test/"))
    });
    assert_eq!(loaded, lp_runtime::RunOutcome::Done);
    check(
        &thread,
        page,
        &[
            ("#f", "ok http://site.test/a/b?x=1#f"),
            ("?q", "ok http://site.test/a/b?q"),
            ("/c", "ok http://site.test/c"),
            ("http://site.test:80/c", "ok http://site.test/c"),
            ("https://site.test/a/b", "SecurityError"),
            ("http://other.test/a/b", "SecurityError"),
            ("http://site.test:8080/a/b", "SecurityError"),
            ("http://user@site.test/a/b", "SecurityError"),
            ("about:blank#f", "SecurityError"),
            ("data:text/html,<p>d</p>", "SecurityError"),
            ("javascript:void 0", "SecurityError"),
        ],
    );
}
