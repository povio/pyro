//! Subresource fetch rules shared by scripts, preloads and frames:
//! Subresource Integrity, `<base href>` resolution and `blob:` URLs.
use lp_browser::{BrowserThread, InitScript, NavigateOpts, PageId, StaticLoader};
use std::rc::Rc;
use std::time::{Duration, Instant};

fn eval(thread: &BrowserThread, page: PageId, source: &str) -> String {
    let realm = thread.browser.page(page).unwrap().main_frame.main_realm().unwrap();
    let scope = realm.scope().unwrap();
    scope.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}
fn settle(thread: &mut BrowserThread) {
    assert_ne!(thread.run_until_idle(Some(Instant::now() + Duration::from_secs(3))), lp_runtime::RunOutcome::TimedOut);
}
fn setup(url: &str, html: &str) -> (BrowserThread, PageId, Rc<StaticLoader>) {
    let mut thread = crate::browser_thread().unwrap();
    let page = thread.browser.create_page().unwrap();
    thread.browser.add_init_script(page, InitScript {
        id: "events".into(), world: None,
        source: "globalThis.events = []; for (const type of ['load', 'error']) document.addEventListener(type, e => { if (e.target.id) events.push(e.target.id + ':' + type) }, true);".into(),
    });
    let loader = Rc::new(StaticLoader::default());
    loader.insert(url, "text/html", html, Duration::ZERO);
    thread.browser.set_loader(Some(loader.clone()));
    (thread, page, loader)
}
fn sri(source: &str) -> String {
    lp_webapi::sri::sha256_expression(source.as_bytes())
}

#[test]
fn script_integrity_blocks_mismatching_classic_and_module_scripts() {
    let good = "globalThis.ran = (globalThis.ran || []).concat('good')";
    let bad = "globalThis.ran = (globalThis.ran || []).concat('bad')";
    let module = "globalThis.ran = (globalThis.ran || []).concat('module')";
    let html = format!(
        "<script id=good src=/good.js integrity='{}'></script>\
         <script id=bad src=/bad.js integrity='{}'></script>\
         <script id=opaque src=http://cdn.test/opaque.js integrity='{}'></script>\
         <script id=module type=module src=/module.js integrity='{}'></script>\
         <script id=badmodule type=module src=/bad-module.js integrity='sha384-nope'></script>",
        sri(good),
        sri("something else"),
        sri(good),
        sri(module),
    );
    let (mut thread, page, loader) = setup("http://site.test/", &html);
    loader.insert("http://site.test/good.js", "text/javascript", good, Duration::ZERO);
    loader.insert("http://site.test/bad.js", "text/javascript", bad, Duration::ZERO);
    loader.insert("http://cdn.test/opaque.js", "text/javascript", good, Duration::ZERO);
    loader.insert("http://site.test/module.js", "text/javascript", module, Duration::ZERO);
    loader.insert("http://site.test/bad-module.js", "text/javascript", bad, Duration::ZERO);
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "ran.join()"), "good,module");
    assert_eq!(
        eval(&thread, page, "events.sort().join('|')"),
        "bad:error|badmodule:error|good:load|module:load|opaque:error"
    );
}

#[test]
fn script_inserted_scripts_and_modulepreloads_check_integrity() {
    let (mut thread, page, loader) = setup("http://site.test/", "<p>page</p>");
    let source = "globalThis.inserted = 1";
    loader.insert("http://site.test/inserted.js", "text/javascript", source, Duration::ZERO);
    loader.insert("http://site.test/chunk.js", "text/javascript", "export default 1", Duration::ZERO);
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    eval(
        &thread,
        page,
        &format!(
            "for (const [id, integrity] of [['ins-bad', 'sha256-AAAA'], ['ins-ok', '{}']]) {{
               const s = document.createElement('script'); s.id = id; s.src = '/inserted.js'; s.integrity = integrity;
               document.head.append(s);
             }}
             const l = document.createElement('link'); l.id = 'pre-bad'; l.rel = 'modulepreload';
             l.href = '/chunk.js'; l.integrity = 'sha512-AAAA'; document.head.append(l); 1",
            sri(source)
        ),
    );
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "events.sort().join('|')"), "ins-bad:error|ins-ok:load|pre-bad:error");
    assert_eq!(eval(&thread, page, "globalThis.inserted"), "1");
}

#[test]
fn base_href_resolves_script_preload_and_iframe_urls() {
    let (mut thread, page, loader) = setup(
        "http://site.test/users/42",
        "<base href='/app/'><link id=pre rel=preload as=script href=pre.js>\
         <script id=main src=main.js></script><script id=mod type=module src=mod.js></script><iframe id=fr src=frame.html></iframe>",
    );
    loader.insert("http://site.test/app/pre.js", "text/javascript", "", Duration::ZERO);
    loader.insert("http://site.test/app/main.js", "text/javascript", "globalThis.main = 1", Duration::ZERO);
    loader.insert("http://site.test/app/mod.js", "text/javascript", "globalThis.mod = 1", Duration::ZERO);
    loader.insert("http://site.test/app/frame.html", "text/html", "<p>frame</p>", Duration::ZERO);
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/users/42")).unwrap();
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "events.sort().join('|')"), "fr:load|main:load|mod:load|pre:load");
    assert!(!loader.log.borrow().iter().skip(1).any(|u| u.contains("/users/")), "{:?}", loader.log.borrow());
    // Inserted later, still against the base URL.
    loader.insert("http://site.test/app/late.js", "text/javascript", "globalThis.late = 1", Duration::ZERO);
    eval(&thread, page, "const s = document.createElement('script'); s.src = 'late.js'; document.head.append(s); 1");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "[main, mod, late].join()"), "1,1,1");
}

#[test]
fn blob_urls_load_as_scripts_modules_stylesheets_and_frames() {
    // The network loader serves blob: URLs from the document's store. The
    // page is an http origin served from memory (nothing listens on port 1).
    let dir = std::env::temp_dir().join(format!("lp-browser-blob-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.html"), "<p>blob</p>").unwrap();
    let origin = "http://127.0.0.1:1";
    let mut thread = crate::browser_thread().unwrap();
    thread
        .enable_network(lp_net::NetConfig {
            static_files: lp_net::StaticFiles::new(vec![(origin.into(), dir.clone())]).unwrap(),
            static_direct: true,
            ..lp_net::NetConfig::default()
        })
        .unwrap();
    let page = thread.browser.create_page().unwrap();
    thread.browser.navigate(page, NavigateOpts::address_bar(format!("{origin}/index.html"))).unwrap();
    settle(&mut thread);
    eval(
        &thread,
        page,
        "globalThis.log = [];
         const url = (text, type) => URL.createObjectURL(new Blob([text], {type}));
         const s = document.createElement('script');
         s.onload = () => log.push('script:load'); s.onerror = () => log.push('script:error');
         s.src = url('log.push(\"classic\")', 'text/javascript'); document.head.append(s);
         import(url('export const v = 42', 'text/javascript')).then(m => log.push('import:' + m.v), e => log.push('import:' + e));
         const l = document.createElement('link'); l.rel = 'stylesheet';
         l.onload = () => log.push('css:' + getComputedStyle(document.documentElement).display);
         l.onerror = () => log.push('css:error');
         l.href = url('html { display: none }', 'text/css'); document.head.append(l);
         const f = document.createElement('iframe');
         addEventListener('message', e => log.push('frame:' + e.data));
         f.src = url('<script>parent.postMessage(`from ${location.protocol}`, `*`)</script>', 'text/html'); document.body.append(f);
         const gone = document.createElement('script'); const revoked = url('log.push(\"revoked\")');
         URL.revokeObjectURL(revoked);
         gone.onerror = () => log.push('revoked:error'); gone.src = revoked; document.head.append(gone); 1",
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    thread.run_until(Some(deadline), |b| {
        let realm = b.page(page).unwrap().main_frame.main_realm().unwrap();
        let scope = realm.scope().unwrap();
        scope.eval("log.length", None, 1).unwrap().to_string().unwrap() == "6"
    });
    assert_eq!(
        eval(&thread, page, "log.sort().join('|')"),
        "classic|css:none|frame:from blob:|import:42|revoked:error|script:load"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn url_of(thread: &BrowserThread, page: PageId) -> String {
    thread.browser.page(page).unwrap().main_frame.url.clone()
}

#[test]
fn meta_refresh_navigates_after_load_and_only_once() {
    let (mut thread, page, loader) = setup(
        "http://site.test/",
        "<meta http-equiv=Refresh content='0; URL=\"/target\"'><meta http-equiv=refresh content='0; url=/second'>\
         <script>addEventListener('load', () => sessionStorage.loadedFirst = '1')</script>",
    );
    loader.insert("http://site.test/target", "text/html", "<p>target</p>", Duration::ZERO);
    loader.insert("http://site.test/second", "text/html", "<p>second</p>", Duration::ZERO);
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(url_of(&thread, page), "http://site.test/target");
    // The first pragma wins; the refresh came after the load event; a
    // zero delay replaces the history entry.
    assert!(!loader.log.borrow().iter().any(|u| u.ends_with("/second")));
    assert_eq!(eval(&thread, page, "[sessionStorage.loadedFirst, history.length].join()"), "1,1");
}

#[test]
fn meta_refresh_inserted_by_script_applies_but_not_in_shadow_trees_or_sandboxes() {
    let (mut thread, page, loader) = setup("http://site.test/", "<p>page</p>");
    loader.insert("http://site.test/dynamic", "text/html", "<p>dynamic</p>", Duration::ZERO);
    loader.insert("http://site.test/shadow", "text/html", "<p>shadow</p>", Duration::ZERO);
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    // A shadow tree's meta does nothing, nor does a later attribute change.
    eval(
        &thread,
        page,
        "const host = document.createElement('div'); document.body.append(host);
         host.attachShadow({mode: 'open'}).innerHTML = `<meta http-equiv=refresh content='0; url=/shadow'>`;
         const late = document.createElement('meta'); late.content = '0; url=/shadow'; document.head.append(late);
         late.httpEquiv = 'refresh'; 1",
    );
    settle(&mut thread);
    assert_eq!(url_of(&thread, page), "http://site.test/");
    // Removed right after insertion: the refresh still happens.
    eval(
        &thread,
        page,
        "document.body.insertAdjacentHTML('beforeend', `<meta id=m http-equiv=refresh content='0;dynamic'>`);
         document.getElementById('m').remove(); 1",
    );
    settle(&mut thread);
    assert_eq!(url_of(&thread, page), "http://site.test/dynamic");
    assert!(!loader.log.borrow().iter().any(|u| u.ends_with("/shadow")));
    // A frame sandboxed without allow-scripts does not refresh.
    loader.insert("http://site.test/framed", "text/html", "<iframe sandbox src=/inner></iframe>", Duration::ZERO);
    loader.insert(
        "http://site.test/inner",
        "text/html",
        "<meta http-equiv=refresh content='0; url=/escaped'>",
        Duration::ZERO,
    );
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/framed")).unwrap();
    settle(&mut thread);
    assert!(!loader.log.borrow().iter().any(|u| u.ends_with("/escaped")), "{:?}", loader.log.borrow());
}

#[test]
fn http_refresh_header_navigates() {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            let (extra, body) =
                if first.contains(" / ") { ("Refresh: 0; url=/after\r\n", "before") } else { ("", "after") };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    let mut thread = crate::browser_thread().unwrap();
    thread.enable_network(Default::default()).unwrap();
    let page = thread.browser.create_page().unwrap();
    thread.browser.navigate(page, NavigateOpts::address_bar(format!("{origin}/"))).unwrap();
    let target = format!("{origin}/after");
    thread.run_until(Some(Instant::now() + Duration::from_secs(5)), |b| {
        b.page(page).unwrap().main_frame.url == target && b.page(page).unwrap().main_frame.loaded
    });
    assert_eq!(eval(&thread, page, "document.body.textContent"), "after");
    server.join().unwrap();
}
