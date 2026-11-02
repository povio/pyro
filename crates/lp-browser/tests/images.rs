//! `<img>` loading through the browser: natural size from the file header,
//! load/error events, `complete`, window-load ordering and request reuse.
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
/// PNG signature + IHDR with the given size.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    b.extend_from_slice(&width.to_be_bytes());
    b.extend_from_slice(&height.to_be_bytes());
    b.extend_from_slice(&[8, 6, 0, 0, 0]);
    b
}
fn setup(html: &str) -> (BrowserThread, PageId, Rc<StaticLoader>) {
    let mut thread = crate::browser_thread().unwrap();
    let page = thread.browser.create_page().unwrap();
    thread.browser.add_init_script(page, InitScript {
        id: "events".into(), world: None,
        source: "globalThis.events = []; for (const type of ['load', 'error']) document.addEventListener(type, e => { if (e.target.tagName === 'IMG') events.push(e.target.id + ':' + type + ':' + e.target.complete + ':' + e.target.naturalWidth + 'x' + e.target.naturalHeight) }, true); addEventListener('load', () => events.push('window:load'));".into(),
    });
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/", "text/html", html, Duration::ZERO);
    thread.browser.set_loader(Some(loader.clone()));
    (thread, page, loader)
}

#[test]
fn parsed_images_report_natural_size_and_delay_window_load() {
    let (mut thread, page, loader) = setup(
        "<img id=a src=/a.png><img id=b src=/missing.png><img id=c src=/page.html><img id=d src=/logo.svg><img id=e><img id=f src=''>",
    );
    loader.insert_bytes("http://site.test/a.png", "image/png", &png(3, 2), Duration::from_millis(30));
    loader.insert("http://site.test/page.html", "text/html", "<p>not an image</p>", Duration::ZERO);
    loader.insert(
        "http://site.test/logo.svg",
        "image/svg+xml",
        "<svg xmlns='http://www.w3.org/2000/svg' width='24' height='16'/>",
        Duration::ZERO,
    );
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    let events = eval(&thread, page, "events.join('|')");
    // Each image settles once with its final state; the slow PNG still precedes window load.
    for expected in ["a:load:true:3x2", "b:error:true:0x0", "c:error:true:0x0", "d:load:true:24x16", "f:error:true:0x0"]
    {
        assert!(events.contains(expected), "{expected} in {events}");
    }
    assert!(events.ends_with("window:load"), "{events}");
    assert_eq!(
        eval(
            &thread,
            page,
            "[document.getElementById('e').complete, document.getElementById('e').naturalWidth].join()"
        ),
        "true,0"
    );
    // One request per image source; no src and an empty src request nothing.
    assert_eq!(
        loader
            .log
            .borrow()
            .iter()
            .filter(|u| u.ends_with(".png") || u.ends_with(".svg") || u.ends_with("page.html"))
            .count(),
        4
    );
}

#[test]
fn src_changes_reload_once_and_report_pending_state() {
    let (mut thread, page, loader) = setup("<img id=a src=/a.png>");
    loader.insert_bytes("http://site.test/a.png", "image/png", &png(3, 2), Duration::ZERO);
    loader.insert_bytes("http://site.test/b.gif", "image/gif", b"GIF89a\x20\x03\x58\x02", Duration::from_millis(10));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    // While the new source loads, complete is false and the old size is gone.
    assert_eq!(
        eval(&thread, page, "const a = document.getElementById('a'); a.src = '/b.gif'; a.src = '/b.gif'; [a.complete, a.naturalWidth].join()"),
        "false,3",
        "a new src is not complete at once; the old size stays until the new load"
    );
    settle(&mut thread);
    assert_eq!(
        eval(&thread, page, "events.filter(e => e.startsWith('a:')).join('|')"),
        "a:load:true:3x2|a:load:true:800x600"
    );
    // Setting the same source again does not request it again.
    eval(&thread, page, "document.getElementById('a').src = '/b.gif'; 1");
    settle(&mut thread);
    assert_eq!(loader.log.borrow().iter().filter(|u| u.ends_with("b.gif")).count(), 1);
    // Removing src clears the image without an event.
    eval(&thread, page, "document.getElementById('a').removeAttribute('src'); 1");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "[document.getElementById('a').naturalWidth, events.length].join()"), "0,3");
}

#[test]
fn blob_and_inserted_images_load_after_window_load_without_reordering_it() {
    let (mut thread, page, _loader) = setup("<p>page</p>");
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    eval(
        &thread,
        page,
        "const bytes = new Uint8Array([0x47,0x49,0x46,0x38,0x39,0x61,0x05,0x00,0x07,0x00]); const img = document.createElement('img'); img.id = 'blob'; img.src = URL.createObjectURL(new Blob([bytes], {type: 'image/gif'})); document.body.append(img); 1",
    );
    // Incomplete as soon as src is set, before and after the load starts.
    assert_eq!(eval(&thread, page, "document.getElementById('blob').complete"), "false");
    thread.browser.checkpoint();
    assert_eq!(eval(&thread, page, "document.getElementById('blob').complete"), "false");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "events.join('|')"), "window:load|blob:load:true:5x7");
}
