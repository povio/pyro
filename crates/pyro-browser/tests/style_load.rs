//! `load` and `error` of `<style>` elements (HTML "update a style block"),
//! as Chromium 147 fires them: in a later task, once the sheet applies and
//! the `@import`s it starts with are fetched. The expected orders are the
//! ones Chromium gave for the same markup.
use pyro_browser::{BrowserThread, InitScript, NavigateOpts, PageId, StaticLoader};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A layout fetcher with only what the page's loader provides (imported
/// sheets), as the binary's caching fetcher keeps it.
#[derive(Default)]
struct Provided(Mutex<HashMap<String, Vec<u8>>>);
impl pyro_render::Fetcher for Provided {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let body = self.0.lock().unwrap().get(&url).cloned();
        done(body.ok_or_else(|| format!("{url}: not provided")));
    }
    fn provide(&self, url: &str, body: &[u8]) -> bool {
        self.0.lock().unwrap().insert(url.to_owned(), body.to_vec());
        true
    }
}

fn eval(thread: &BrowserThread, page: PageId, source: &str) -> String {
    let realm = thread.browser.page(page).unwrap().main_frame.main_realm().unwrap();
    let scope = realm.scope().unwrap();
    scope.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}
fn settle(thread: &mut BrowserThread) {
    assert_ne!(
        thread.run_until_idle(Some(Instant::now() + Duration::from_secs(3))),
        pyro_runtime::RunOutcome::TimedOut
    );
}
fn events(thread: &BrowserThread, page: PageId) -> String {
    eval(thread, page, "events.join('|')")
}
/// A page whose `events` records `<style>` loads and errors (by id), DOMContentLoaded and the window's load.
fn setup(html: &str) -> (BrowserThread, PageId, Rc<StaticLoader>) {
    pyro_layout::install(Arc::new(Provided::default()));
    let mut thread = BrowserThread::new().unwrap();
    let page = thread.browser.create_page().unwrap();
    thread.browser.add_init_script(page, InitScript {
        id: "events".into(),
        world: None,
        source: "globalThis.events = []; for (const type of ['load', 'error']) document.addEventListener(type, e => { if (e.target.localName === 'style') events.push(e.target.id + ':' + type) }, true); document.addEventListener('DOMContentLoaded', () => events.push('dcl')); addEventListener('load', () => events.push('window:load'));".into(),
    });
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/", "text/html", html, Duration::ZERO);
    thread.browser.set_loader(Some(loader.clone()));
    (thread, page, loader)
}
fn load(html: &str, sheets: &[(&str, &str, u64)]) -> (BrowserThread, PageId, Rc<StaticLoader>) {
    let (mut thread, page, loader) = setup(html);
    for (path, css, delay) in sheets {
        loader.insert(&format!("http://site.test{path}"), "text/css", css, Duration::from_millis(*delay));
    }
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    (thread, page, loader)
}

#[test]
fn a_parsed_style_loads_in_a_later_task_before_dom_content_loaded() {
    let (thread, page, _) = load(
        "<style id=s onload=\"events.push('onload')\">html { display: none }</style><script>events.push('script')</script>",
        &[],
    );
    // Chromium: after-style-script, style-load, DCL, window-load.
    assert_eq!(events(&thread, page), "script|s:load|onload|dcl|window:load");
}

#[test]
fn media_and_disabled_do_not_matter_and_a_type_other_than_css_fires_nothing() {
    // An empty sheet, a `media` that does not match and a `disabled`
    // attribute (not one of <style>'s) load too; a `type` other than CSS
    // makes no sheet and fires nothing. Chromium: the same.
    let (thread, page, _) = load(
        "<style id=empty></style><style id=print media=print>p {}</style><style id=disabled disabled>p {}</style><style id=plain type=text/plain>p {}</style>",
        &[],
    );
    assert_eq!(events(&thread, page), "empty:load|print:load|disabled:load|dcl|window:load");
}

#[test]
fn a_style_waits_for_its_imports_and_blocks_the_parsers_scripts() {
    let (thread, page, loader) = load(
        "<style id=s>@import url(/a.css); p { color: red }</style><script>events.push('script:' + getComputedStyle(document.documentElement).display)</script>",
        &[("/a.css", "html { display: none }", 30)],
    );
    // Chromium: the script runs once the import applies, then the style loads.
    assert_eq!(events(&thread, page), "script:none|s:load|dcl|window:load");
    assert_eq!(loader.log.borrow().iter().filter(|u| u.ends_with("/a.css")).count(), 1);
}

#[test]
fn the_document_load_event_waits_for_imports_imported_sheets_included() {
    let (thread, page, loader) = load(
        "<style id=s>@import '/a.css';</style>",
        &[("/a.css", "@import url(b.css); p {}", 0), ("/b.css", "html { display: none }", 40)],
    );
    // Chromium: DCL, style-load, window-load, the nested import waited for.
    assert_eq!(events(&thread, page), "dcl|s:load|window:load");
    assert!(loader.log.borrow().iter().any(|u| u == "http://site.test/b.css"));
    assert_eq!(eval(&thread, page, "getComputedStyle(document.documentElement).display"), "none");
}

#[test]
fn a_failed_import_is_an_error_and_a_failed_import_of_an_imported_sheet_is_not() {
    let (thread, page, _) = load(
        "<style id=missing>@import '/missing.css';</style><style id=one>@import '/ok.css'; @import '/missing.css';</style><style id=nested>@import '/nest.css';</style>",
        &[("/ok.css", "p {}", 0), ("/nest.css", "@import '/missing.css';", 0)],
    );
    let all = events(&thread, page);
    assert!(all.ends_with("window:load"), "{all}");
    let mut styles: Vec<&str> = all.split('|').filter(|e| e.contains(':') && !e.starts_with("window")).collect();
    styles.sort();
    assert_eq!(styles, ["missing:error", "nested:load", "one:error"]);
}

#[test]
fn imports_that_do_not_count_are_not_fetched() {
    let (thread, page, loader) = load(
        "<style id=late>p { color: red } @import '/late.css';</style><style id=unsupported>@import '/unsupported.css' supports(nonsense: 1);</style>",
        &[],
    );
    assert_eq!(events(&thread, page), "late:load|unsupported:load|dcl|window:load");
    assert!(!loader.log.borrow().iter().any(|u| u.ends_with("late.css") || u.ends_with("unsupported.css")));
}

#[test]
fn script_inserted_styles_load_again_for_new_text_and_a_css_type_but_not_for_media() {
    let (mut thread, page, loader) = load("<p>x</p>", &[("/slow.css", "p {}", 60)]);
    eval(&thread, page, "events.length = 0; globalThis.s = document.createElement('style'); s.id = 'x'; s.textContent = 'p {}'; document.head.append(s); events.push('sync'); queueMicrotask(() => events.push('micro')); 1");
    settle(&mut thread);
    assert_eq!(events(&thread, page), "sync|micro|x:load");
    let step = |thread: &mut BrowserThread, script: &str| {
        eval(thread, page, &format!("events.length = 0; {script}; 1"));
        settle(thread);
        events(thread, page)
    };
    assert_eq!(step(&mut thread, "s.media = 'print'"), "");
    assert_eq!(step(&mut thread, "s.media = 'all'; s.textContent = 'a {}'"), "x:load");
    assert_eq!(step(&mut thread, "s.firstChild.data = 'b {}'"), "x:load");
    assert_eq!(step(&mut thread, "s.type = 'text/plain'; s.textContent = 'c {}'"), "");
    assert_eq!(step(&mut thread, "s.type = 'text/css'"), "x:load");
    // Removed before its task: it still fires (Chromium too).
    eval(&thread, page, "events.length = 0; globalThis.r = document.createElement('style'); r.onload = () => events.push('r:load:' + r.isConnected); document.head.append(r); 1");
    thread.browser.checkpoint();
    eval(&thread, page, "r.remove(); 1");
    settle(&mut thread);
    assert_eq!(events(&thread, page), "r:load:false");
    // Removed while its import loads: nothing fires, nothing waits.
    assert_eq!(
        step(&mut thread, "const i = document.createElement('style'); i.id = 'i'; i.textContent = '@import \"/slow.css\";'; document.head.append(i); setTimeout(() => i.remove(), 10)"),
        ""
    );
    assert!(loader.log.borrow().iter().any(|u| u.ends_with("/slow.css")));
    // Inserted with an import: it loads once the import arrives.
    assert_eq!(
        step(&mut thread, "const j = document.createElement('style'); j.id = 'j'; j.textContent = '@import \"/slow.css\";'; document.head.append(j); setTimeout(() => events.push('10ms'), 10)"),
        "10ms|j:load"
    );
}

#[test]
fn a_style_in_a_shadow_tree_or_a_frame_loads() {
    let (mut thread, page, _) = load("<div id=host></div><iframe srcdoc=\"<style onload='parent.events.push(&quot;frame:load&quot;)'>p {}</style>\"></iframe>", &[]);
    assert!(events(&thread, page).contains("frame:load"), "{}", events(&thread, page));
    // Not composed: the document does not see it.
    eval(&thread, page, "events.length = 0; const s = document.createElement('style'); s.onload = () => events.push('shadow:load'); s.textContent = 'p {}'; host.attachShadow({ mode: 'open' }).append(s); 1");
    settle(&mut thread);
    assert_eq!(events(&thread, page), "shadow:load");
}
