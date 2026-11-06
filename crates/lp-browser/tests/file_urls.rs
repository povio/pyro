//! `file:` navigation through the real network client (Playwright
//! `page.goto("file:///…")`): the document and its subresources come from
//! disk, the origin is opaque, and `fetch` does not support the scheme.
use lp_browser::{BrowserThread, NavigateOpts, PageEvent, PageId, PageObserver};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

fn wait_for(t: &mut BrowserThread, page: PageId, condition: &str) {
    let outcome = t.run_until(Some(Instant::now() + Duration::from_secs(5)), |b| {
        let scope = b.page(page).unwrap().main_frame.main_realm().unwrap().scope().unwrap();
        scope.eval(condition, None, 1).is_ok_and(|v| v.to_bool())
    });
    assert_eq!(outcome, lp_runtime::RunOutcome::Done, "timed out waiting for {condition}");
}

fn eval(t: &BrowserThread, page: PageId, script: &str) -> String {
    let scope = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().scope().unwrap();
    scope.eval(script, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}

#[derive(Default)]
struct Failures(RefCell<Vec<String>>);
impl PageObserver for Failures {
    fn on_page_event(&self, _page: PageId, event: &PageEvent) {
        if let PageEvent::FrameNavigateFailed { error, .. } = event {
            self.0.borrow_mut().push(error.clone());
        }
    }
}

struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn file_documents_load_with_their_local_subresources() {
    let dir = TempDir(std::env::temp_dir().join(format!("lp-browser-file-urls-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir.0);
    std::fs::create_dir_all(&dir.0).unwrap();
    std::fs::write(
        dir.0.join("index.html"),
        "<!doctype html><link rel=stylesheet href=style.css><script src=app.js></script><p id=p>hi</p>",
    )
    .unwrap();
    std::fs::write(dir.0.join("app.js"), "window.ran = location.protocol + '|' + origin;").unwrap();
    std::fs::write(dir.0.join("style.css"), "#p { display: none }").unwrap();

    let mut t = crate::browser_thread().unwrap();
    t.enable_network(lp_net::NetConfig::default()).unwrap();
    let failures = Rc::new(Failures::default());
    t.browser.set_observer(Some(failures.clone()));
    let page = t.browser.create_page().unwrap();
    let url = url::Url::from_file_path(dir.0.join("index.html")).unwrap().to_string();
    t.browser.navigate(page, NavigateOpts::address_bar(&url)).unwrap();
    wait_for(&mut t, page, "document.readyState === 'complete' && location.protocol === 'file:'");

    assert!(failures.0.borrow().is_empty(), "{:?}", failures.0.borrow());
    assert_eq!(eval(&t, page, "location.href"), url);
    assert_eq!(eval(&t, page, "window.ran"), "file:|null");
    assert_eq!(eval(&t, page, "getComputedStyle(document.getElementById('p')).display"), "none");
    eval(&t, page, "fetch('app.js').then(() => window.fetched = 'ok', e => window.fetched = e.name)");
    wait_for(&mut t, page, "window.fetched !== undefined");
    assert_eq!(eval(&t, page, "window.fetched"), "TypeError");
}

#[test]
fn web_pages_cannot_navigate_frames_to_local_files() {
    let dir = TempDir(std::env::temp_dir().join(format!("lp-browser-file-urls-web-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir.0);
    std::fs::create_dir_all(dir.0.join("web")).unwrap();
    std::fs::write(dir.0.join("secret.html"), "<p id=s>secret</p>").unwrap();
    let secret = url::Url::from_file_path(dir.0.join("secret.html")).unwrap().to_string();
    std::fs::write(
        dir.0.join("web/index.html"),
        format!(
            "<!doctype html><meta name=referrer content=no-referrer><iframe referrerpolicy=no-referrer src='{secret}'></iframe><a id=a rel=noreferrer href='{secret}'>x</a>"
        ),
    )
    .unwrap();
    // Port 1 has no listener: the static origin answers everything.
    let origin = "http://127.0.0.1:1";
    let config = lp_net::NetConfig {
        static_files: lp_net::StaticFiles::new(vec![(origin.to_owned(), dir.0.join("web"))]).unwrap(),
        ..lp_net::NetConfig::default()
    };
    let mut t = crate::browser_thread().unwrap();
    t.enable_network(config).unwrap();
    let failures = Rc::new(Failures::default());
    t.browser.set_observer(Some(failures.clone()));
    let page = t.browser.create_page().unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar(format!("{origin}/index.html"))).unwrap();
    wait_for(&mut t, page, "location.protocol === 'http:' && document.readyState === 'complete'");
    assert_eq!(failures.0.borrow().as_slice(), ["net::ERR_ACCESS_DENIED"]);
    eval(&t, page, "document.getElementById('a').click()");
    let _ = t.run_until(Some(Instant::now() + Duration::from_millis(300)), |_| false);
    assert_eq!(failures.0.borrow().as_slice(), ["net::ERR_ACCESS_DENIED", "net::ERR_ACCESS_DENIED"]);
    assert_eq!(eval(&t, page, "location.protocol"), "http:");
}
